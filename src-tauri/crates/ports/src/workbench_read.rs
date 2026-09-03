//! T02 工作台只读入口的端口层：定义数据库探测与账号证据读取的 trait 边界。
//!
//! application 层通过这些 port 调用 infrastructure 实现，不直接依赖 rusqlite
//! 或文件系统。对应规格第 7 节架构边界与第 9 节 ProductAdapter 契约的只读子集。
//!
//! 仅承载 T02 需要的最小 port，不预建 ProductAdapter 完整契约。

use std::path::Path;
use std::time::SystemTime;
use traesync_domain::{AccountEvidence, CompatibilityState};

/// 数据库探测端口：infrastructure 实现嵌入式 SQLCipher 打开与 schema 兼容判断。
///
/// 边界约束：
/// - 实现必须只接受经过 `FixturePathGuard` 验证的 fixture 路径
/// - `raw_key` 是已授权的 TRAE 技术参数；Token、cookies 和完整认证正文不得进入返回值或账号证据
/// - raw_key 不进入 OperationId、状态枚举或结构化错误代码，避免破坏审计协议；这不是密钥保密要求
/// - 错误 key、截断文件、未知 schema 必须返回结构化 `CompatibilityState::Incompatible`
pub trait DatabaseProbePort: Send + Sync {
    /// 探测指定数据库副本。
    ///
    /// `db_path` 必须是 fixture_root 内的副本路径，`raw_key` 是 SQLCipher raw key hex。
    /// 返回 `CompatibilityState::Verified` 携带 schema 指纹与行数，
    /// 或 `CompatibilityState::Incompatible` 携带结构化原因。
    ///
    /// R1：实现必须使用 SQLite/SQLCipher 只读 flags 打开，
    /// 错误 key、正确 key 与探测失败均保持 DB/WAL/SHM 字节级不变。
    fn probe_database(&self, db_path: &Path, raw_key: &str) -> CompatibilityState;

    /// 在长时间数据库探测期间持续复核授权上下文。
    ///
    /// 默认实现保留旧 mock/adapter 的兼容性；需要支持中途撤销的实现应覆盖此方法，
    /// 并在隔离副本复制、哈希和打开数据库前后调用 `is_authorized`。
    fn probe_database_with_validation(
        &self,
        db_path: &Path,
        raw_key: &str,
        is_authorized: &dyn Fn() -> bool,
    ) -> Option<CompatibilityState> {
        if !is_authorized() {
            return None;
        }
        let result = self.probe_database(db_path, raw_key);
        if !is_authorized() {
            return None;
        }
        Some(result)
    }

    /// 通过 SQLCipher `sqlcipher_export()` 生成单文件逻辑副本。
    ///
    /// 返回逻辑副本路径（位于 fixture_root 内）。未 checkpoint WAL 的已提交记录
    /// 必须进入逻辑副本。加密数据库不使用 SQLite `sqlite3_backup_*` API；失败时返回
    /// None（不抛出原始错误给上层）。
    fn backup_to_logical_copy(&self, source_db: &Path, raw_key: &str)
        -> Option<std::path::PathBuf>;

    /// 在临时副本上执行事务提交与回滚验证。
    ///
    /// 返回 true 表示提交-回滚断言通过，false 表示失败。
    /// 不得触碰活动库，只在 fixture_root 内的副本执行。
    fn verify_transaction_rollback(&self, copy_db: &Path, raw_key: &str) -> bool;

    /// 执行两层完整性检查：`cipher_integrity_check` 与 `integrity_check`。
    ///
    /// 返回 (cipher_ok, sqlite_ok)。两个均为 true 才视为完整性通过。
    fn run_integrity_checks(&self, db_path: &Path, raw_key: &str) -> (bool, bool);

    /// 创建 Trae Sync 随机密钥目录库 fixture，重开并验证完整性。
    ///
    /// 返回新建目录库路径，失败返回 None。测试和日志不得输出 key。
    fn create_random_key_catalog(&self, fixture_root: &Path) -> Option<std::path::PathBuf>;
}

/// 账号证据读取端口：infrastructure 实现白名单日志、认证字段指纹与
/// Local Storage 逻辑读取。
///
/// 安全约束：
/// - 只解析白名单事件：fetchLogTask、User info loaded、updateUserInfo/getUserInfo
/// - 严格区分 `userId` 与 `deviceId`
/// - 只持久化 SHA-256 指纹，不持久化认证正文
/// - Local Storage 使用逻辑读取接口，不扫描原始字节末次命中
///
/// R3：所有读取方法接收 `now: SystemTime`，便于 Expired 判定与可注入时间源，
/// 避免依赖不稳定 wall clock 测试。
pub trait AccountEvidenceReaderPort: Send + Sync {
    /// 读取 fixture_root 内的账号证据。
    ///
    /// fixture_root 内应包含：
    /// - `logs/<session>/alog.log`、`renderer.log`、`main.log`
    /// - `globalStorage/storage.json`
    /// - `Local Storage/leveldb/`
    ///
    /// `now` 为观测时间，用于：
    /// - 设置 `observed_at`
    /// - 与最新 session 目录 mtime 比较，过老则标记 `Expired`
    fn read_account_evidence(&self, fixture_root: &Path, now: SystemTime) -> AccountEvidence;

    /// TRAE 关闭后重新读取认证指纹，与之前证据比较。
    ///
    /// 返回新证据。application 层比较 `auth_fingerprint` 判断是否漂移。
    /// `now` 用于 `observed_at` 与 Expired 判定。
    fn re_read_after_close(&self, fixture_root: &Path, now: SystemTime) -> AccountEvidence;
}
