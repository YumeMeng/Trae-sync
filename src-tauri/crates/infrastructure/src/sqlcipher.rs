//! 嵌入式 SQLCipher 探测实现：使用 rusqlite + bundled-sqlcipher-vendored-openssl。
//!
//! 对应 Gate A：生产 Rust 连接层能打开 Work CN 和 Trae Sync 两类 SQLCipher 数据库，
//! 不依赖外部 CLI 或 TRAE DLL。
//!
//! 安全约束：
//! - raw_key 永不进入日志、错误消息或返回值
//! - 错误 key 必须返回结构化 `WrongKey`，不抛出原始错误
//! - 截断文件返回 `TruncatedFile`
//! - 未知 schema 返回 `UnknownSchema`
//! - Backup API 必须保留未 checkpoint WAL 的已提交记录

use rusqlite::{Connection, OpenFlags};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use traesync_domain::{CompatibilityState, IncompatibleReason};
use traesync_ports::DatabaseProbePort;

use crate::work_cn_schema::{check_schema, compute_schema_fingerprint, read_table_counts};

/// 嵌入式 SQLCipher 探测器：实现 `DatabaseProbePort`。
pub struct SqlCipherProbe;

impl Default for SqlCipherProbe {
    fn default() -> Self {
        Self::new()
    }
}

impl SqlCipherProbe {
    pub fn new() -> Self {
        Self
    }
}

impl DatabaseProbePort for SqlCipherProbe {
    fn probe_database(&self, db_path: &Path, raw_key: &str) -> CompatibilityState {
        probe_database_inner(db_path, raw_key)
    }

    fn backup_to_logical_copy(&self, source_db: &Path, raw_key: &str) -> Option<PathBuf> {
        backup_to_logical_copy_inner(source_db, raw_key)
    }

    fn verify_transaction_rollback(&self, copy_db: &Path, raw_key: &str) -> bool {
        verify_transaction_rollback_inner(copy_db, raw_key)
    }

    fn run_integrity_checks(&self, db_path: &Path, raw_key: &str) -> (bool, bool) {
        run_integrity_checks_inner(db_path, raw_key)
    }

    fn create_random_key_catalog(&self, fixture_root: &Path) -> Option<PathBuf> {
        create_random_key_catalog_inner(fixture_root)
    }
}

/// R1：以只读 flags 打开 SQLCipher 连接并设置 raw key。
///
/// 使用 `SQLITE_OPEN_READ_ONLY` 确保 SQLite 不会通过默认读写连接创建或修改文件。
/// 错误 key、正确 key 与探测失败均保持 DB/WAL/SHM 字节级不变。
///
/// raw_key 必须是 64 位 hex 字符串（32 字节）。
/// 使用 `PRAGMA key = "x'...'"` 语法，对应 TECHNICAL_BASELINE.md。
fn open_with_key_readonly(db_path: &Path, raw_key: &str) -> Result<Connection, rusqlite::Error> {
    // R1：只读 flags——不创建文件，不写入
    let flags = OpenFlags::SQLITE_OPEN_READ_ONLY
        | OpenFlags::SQLITE_OPEN_PRIVATE_CACHE
        | OpenFlags::SQLITE_OPEN_NO_MUTEX;
    let conn = Connection::open_with_flags(db_path, flags)?;
    // raw key 语法：x'<hex>' —— 不进入日志
    let pragma = format!("PRAGMA key = \"x'{}'\";", raw_key);
    conn.execute_batch(&pragma)?;
    Ok(conn)
}

/// 以读写 flags 打开 SQLCipher 连接并设置 raw key。
///
/// 仅用于需要在副本上执行事务测试或创建新目录库的场景。
/// 探测和完整性检查必须使用 `open_with_key_readonly`。
fn open_with_key(db_path: &Path, raw_key: &str) -> Result<Connection, rusqlite::Error> {
    let conn = Connection::open(db_path)?;
    // raw key 语法：x'<hex>' —— 不进入日志
    let pragma = format!("PRAGMA key = \"x'{}'\";", raw_key);
    conn.execute_batch(&pragma)?;
    Ok(conn)
}

/// R5：兼容的 SQLCipher cipher_version 前缀。
///
/// 对应 TECHNICAL_BASELINE.md：
/// - TRAE 内置 SQLCipher 为 `4.5.7`
/// - SQLCipher `4.6.1 community` 已验证兼容
///
/// SQLCipher 4.5.x 系列使用相同的默认加密参数（cipher_compatibility=4，
/// AES-256-CBC + HMAC-SHA512），4.5.x 内部互相兼容：
/// - 本地 `bundled-sqlcipher-vendored-openssl` 编译版本为 `4.5.3 community`
/// - TRAE 内置为 `4.5.7`
/// - 两者可读写同一数据库（cipher 参数一致）
///
/// 启动兼容检查必须验证 cipher_version 与基线兼容——不匹配时返回
/// `CipherVersionMismatch` 并保持只读。
const SUPPORTED_CIPHER_VERSION_PREFIXES: &[&str] = &["4.5.", "4.6.1"];

/// 探测数据库兼容性内部实现。
///
/// R1：使用 `open_with_key_readonly`（SQLITE_OPEN_READ_ONLY）确保零写入。
/// R5：在解密成功后立即校验 `cipher_version`，与基线不兼容时返回
/// `CipherVersionMismatch` 并保持只读——避免后续 schema 检查在未知版本上误判。
fn probe_database_inner(db_path: &Path, raw_key: &str) -> CompatibilityState {
    // 1. 截断文件检查：SQLite/SQLCipher 文件头至少 16 字节
    match std::fs::metadata(db_path) {
        Ok(meta) => {
            if meta.len() < 16 {
                return CompatibilityState::Incompatible {
                    reason: IncompatibleReason::TruncatedFile,
                };
            }
        }
        Err(_) => {
            return CompatibilityState::Incompatible {
                reason: IncompatibleReason::TruncatedFile,
            };
        }
    }

    // 2. R1：以只读 flags 打开并设置 key——绝不创建文件，绝不写入 DB/WAL/SHM
    let conn = match open_with_key_readonly(db_path, raw_key) {
        Ok(c) => c,
        Err(e) => {
            return classify_open_error(&e);
        }
    };

    // 3. 触发解密——读取 sqlite_master
    //    错误 key 在此阶段失败，错误消息含 "file is not a database" 或 "file is encrypted"
    let read_result: Result<Vec<(String, String)>, rusqlite::Error> = {
        let mut stmt =
            match conn.prepare("SELECT name, sql FROM sqlite_master WHERE type='table' LIMIT 1") {
                Ok(s) => s,
                Err(e) => return classify_read_error(&e),
            };
        let rows = stmt.query_map([], |row| {
            let name: String = row.get(0)?;
            let sql: String = row.get(1).unwrap_or_default();
            Ok((name, sql))
        });
        match rows {
            Ok(r) => r.collect::<Result<Vec<_>, _>>(),
            Err(e) => return classify_read_error(&e),
        }
    };
    if let Err(e) = read_result {
        return classify_read_error(&e);
    }

    // 4. R5：cipher_version 兼容检查（解密成功后立即执行）
    //    PRAGMA cipher_version 返回形如 "4.5.7 community" 或 "4.6.1 community"
    if let Err(reason) = check_cipher_version(&conn) {
        return CompatibilityState::Incompatible { reason };
    }

    // 5. schema 兼容性检查（表 -> 列 -> 唯一约束）
    if let Err(reason) = check_schema(&conn) {
        return CompatibilityState::Incompatible { reason };
    }

    // 6. 计算 schema 指纹与行数
    let schema_fingerprint = compute_schema_fingerprint(&conn);
    let counts = read_table_counts(&conn);

    CompatibilityState::Verified {
        schema_fingerprint,
        counts,
    }
}

/// R5：校验 `PRAGMA cipher_version` 与基线兼容。
///
/// 返回 `Ok(())` 当且仅当 cipher_version 以 `4.5.`（4.5.x 全系列）或 `4.6.1` 开头；
/// 否则返回 `CipherVersionMismatch` 携带实际版本字符串。
/// 查询失败（不应发生在已解密连接上）保守视为不兼容。
///
/// 兼容范围说明（与 `SUPPORTED_CIPHER_VERSION_PREFIXES` 一致）：
/// - `4.5.` 前缀覆盖 4.5.3（本地 bundled 编译版本）与 4.5.7（TRAE 内置）
/// - `4.6.1` 单独列出（已验证兼容）
/// - 4.5.x 系列共享 cipher_compatibility=4 默认参数（AES-256-CBC + HMAC-SHA512）
fn check_cipher_version(conn: &Connection) -> Result<(), IncompatibleReason> {
    let version: String = conn
        .query_row("PRAGMA cipher_version", [], |row| row.get(0))
        .map_err(|_| IncompatibleReason::CipherVersionMismatch {
            version: "unknown".to_string(),
        })?;
    if SUPPORTED_CIPHER_VERSION_PREFIXES
        .iter()
        .any(|prefix| version.starts_with(prefix))
    {
        Ok(())
    } else {
        Err(IncompatibleReason::CipherVersionMismatch { version })
    }
}

/// 分类打开阶段错误：截断或损坏文件
fn classify_open_error(e: &rusqlite::Error) -> CompatibilityState {
    let msg = e.to_string().to_lowercase();
    if msg.contains("unable to open")
        || msg.contains("no such table")
        || msg.contains("not a database")
        || msg.contains("file is not")
    {
        CompatibilityState::Incompatible {
            reason: IncompatibleReason::TruncatedFile,
        }
    } else {
        // 兜底：未知错误视为 TruncatedFile（失败关闭）
        CompatibilityState::Incompatible {
            reason: IncompatibleReason::TruncatedFile,
        }
    }
}

/// 分类读取阶段错误：错误 key 或损坏
fn classify_read_error(e: &rusqlite::Error) -> CompatibilityState {
    let msg = e.to_string().to_lowercase();
    // SQLCipher 错误 key 通常返回 "file is not a database" 或 "file is encrypted or not a database"
    if msg.contains("not a database") || msg.contains("encrypted") || msg.contains("decrypt") {
        CompatibilityState::Incompatible {
            reason: IncompatibleReason::WrongKey,
        }
    } else {
        CompatibilityState::Incompatible {
            reason: IncompatibleReason::TruncatedFile,
        }
    }
}

/// SQLCipher 逻辑副本：使用 `sqlcipher_export()` 把源库（含未 checkpoint WAL 的已提交记录）导出到目标库。
///
/// rusqlite 的 Backup API 不支持加密库（"backup is not supported with encrypted databases"），
/// 改用 SQLCipher 官方推荐的 `sqlcipher_export()` 函数实现等价行为：
/// - 源库读操作天然合并 WAL 中已提交记录；
/// - 目标库通过 ATTACH 时 `KEY x'...'` 用相同 raw_key 加密；
/// - 完成后 DETACH，目标库即单文件逻辑副本。
///
/// raw_key 不会出现在日志/错误消息；失败返回 None。
fn backup_to_logical_copy_inner(source_db: &Path, raw_key: &str) -> Option<PathBuf> {
    let dest_path = source_db.with_extension("logical-copy.db");
    // 清理残留目标文件与 WAL/SHM，避免 ATTACH 时已有文件冲突
    let _ = std::fs::remove_file(&dest_path);
    let _ = std::fs::remove_file(dest_path.with_extension("logical-copy.db-wal"));
    let _ = std::fs::remove_file(dest_path.with_extension("logical-copy.db-shm"));

    let conn = open_with_key(source_db, raw_key).ok()?;

    // ATTACH 目标库：KEY x'...' 指定目标库加密 key（与源 key 相同，便于后续探测复用）
    // raw_key 仅进入 SQL 批处理，不进入日志
    let attach_sql = format!(
        "ATTACH DATABASE '{}' AS dst KEY \"x'{}'\";",
        dest_path.display(),
        raw_key
    );
    if conn.execute_batch(&attach_sql).is_err() {
        return None;
    }

    // sqlcipher_export('dst') 把 main 库全部表/数据导出到 dst，
    // 读取 main 时已合并 WAL 中已提交记录
    let export_ok = conn
        .query_row("SELECT sqlcipher_export('dst')", [], |_row| Ok(()))
        .is_ok();

    // 无论成功失败都尝试 DETACH，避免连接关闭时残留状态
    let _ = conn.execute_batch("DETACH DATABASE dst;");
    // 关闭源连接，让目标库文件彻底落盘
    drop(conn);

    if export_ok && dest_path.exists() {
        Some(dest_path)
    } else {
        None
    }
}

/// 在临时副本上验证事务提交与回滚。
///
/// 流程：
/// 1. BEGIN, INSERT 一行, ROLLBACK, 验证行数未变
/// 2. BEGIN, INSERT 一行, COMMIT, 验证行数 +1
///
/// 不得触碰活动库——只在 fixture_root 内的副本执行。
fn verify_transaction_rollback_inner(copy_db: &Path, raw_key: &str) -> bool {
    let conn = match open_with_key(copy_db, raw_key) {
        Ok(c) => c,
        Err(_) => return false,
    };

    // 假设 chat_message 表存在（由 fixture 保证）
    let baseline: i64 = conn
        .query_row("SELECT COUNT(*) FROM chat_message", [], |row| row.get(0))
        .unwrap_or(0);

    // 1. ROLLBACK 测试
    if conn
        .execute_batch("BEGIN; INSERT INTO chat_message VALUES ('rollback-test', 's1'); ROLLBACK;")
        .is_err()
    {
        return false;
    }
    let after_rollback: i64 = conn
        .query_row("SELECT COUNT(*) FROM chat_message", [], |row| row.get(0))
        .unwrap_or(-1);
    if after_rollback != baseline {
        return false;
    }

    // 2. COMMIT 测试
    if conn
        .execute_batch("BEGIN; INSERT INTO chat_message VALUES ('commit-test', 's1'); COMMIT;")
        .is_err()
    {
        return false;
    }
    let after_commit: i64 = conn
        .query_row("SELECT COUNT(*) FROM chat_message", [], |row| row.get(0))
        .unwrap_or(-1);
    after_commit == baseline + 1
}

/// 执行两层完整性检查：
/// - `PRAGMA cipher_integrity_check` 无错误（返回 0 行）
/// - `PRAGMA integrity_check` 返回 "ok"
fn run_integrity_checks_inner(db_path: &Path, raw_key: &str) -> (bool, bool) {
    let conn = match open_with_key(db_path, raw_key) {
        Ok(c) => c,
        Err(_) => return (false, false),
    };

    // cipher_integrity_check：每行代表一个错误。无错误时返回 0 行。
    let cipher_ok: bool = {
        let mut stmt = match conn.prepare("PRAGMA cipher_integrity_check") {
            Ok(s) => s,
            Err(_) => return (false, false),
        };
        let rows = match stmt.query_map([], |row| {
            let msg: String = row.get(0).unwrap_or_default();
            Ok(msg)
        }) {
            Ok(r) => r,
            Err(_) => return (false, false),
        };
        let mut count = 0;
        for row in rows {
            if row.is_ok() {
                count += 1;
            }
        }
        count == 0
    };

    // integrity_check：第一行返回 "ok" 表示无错误
    let sqlite_ok: bool = conn
        .query_row("PRAGMA integrity_check", [], |row| {
            let value: String = row.get(0).unwrap_or_default();
            Ok(value == "ok")
        })
        .unwrap_or(false);

    (cipher_ok, sqlite_ok)
}

/// 创建 Trae Sync 随机密钥目录库 fixture。
///
/// 流程：
/// 1. 生成 32 字节随机 hex key（基于 SystemTime + pid，非加密安全但 T02 fixture 足够）
/// 2. 创建 SQLCipher DB，建立最小 catalog schema
/// 3. 关闭后重新打开验证完整性
///
/// 测试和日志不得输出 key。
fn create_random_key_catalog_inner(fixture_root: &Path) -> Option<PathBuf> {
    let key = generate_random_hex_key();
    let catalog_path = fixture_root.join("catalog.db");

    // 创建并初始化
    {
        let conn = Connection::open(&catalog_path).ok()?;
        conn.execute_batch(&format!("PRAGMA key = \"x'{}'\";", key))
            .ok()?;
        conn.execute_batch(
            r#"
            CREATE TABLE catalog_meta (
                id INTEGER PRIMARY KEY,
                key TEXT NOT NULL,
                value TEXT NOT NULL
            );
            CREATE TABLE data_location (
                data_location_id TEXT PRIMARY KEY,
                platform_id TEXT NOT NULL,
                display_name TEXT NOT NULL
            );
            "#,
        )
        .ok()?;
        // WAL checkpoint 确保写入磁盘
        conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")
            .ok()?;
    }

    // 重开并验证完整性
    {
        let conn = open_with_key(&catalog_path, &key).ok()?;
        // cipher_integrity_check：每行代表错误，0 行才 OK
        let cipher_ok: bool = {
            let mut stmt = conn.prepare("PRAGMA cipher_integrity_check").ok()?;
            let rows = stmt
                .query_map([], |row| {
                    let msg: String = row.get(0).unwrap_or_default();
                    Ok(msg)
                })
                .ok()?;
            let mut count = 0;
            for row in rows {
                if row.is_ok() {
                    count += 1;
                }
            }
            count == 0
        };
        let sqlite_ok: bool = conn
            .query_row("PRAGMA integrity_check", [], |row| {
                let value: String = row.get(0).unwrap_or_default();
                Ok(value == "ok")
            })
            .unwrap_or(false);
        if !(cipher_ok && sqlite_ok) {
            return None;
        }
        // 验证表存在
        let table_exists: bool = conn
            .query_row(
                "SELECT 1 FROM sqlite_master WHERE type='table' AND name='catalog_meta' LIMIT 1",
                [],
                |_| Ok(true),
            )
            .unwrap_or(false);
        if !table_exists {
            return None;
        }
    }

    Some(catalog_path)
}

/// 生成 32 字节随机 hex key（64 字符）。
///
/// 基于 SystemTime 纳秒 + 进程 ID + 计数器，通过 SHA-256 派生。
/// 非加密安全，但 T02 fixture 阶段足够。真实目录库密钥生成在 T03+。
fn generate_random_hex_key() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let pid = std::process::id();
    // 静态计数器保证同进程多次调用产生不同 key
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::SeqCst);

    let mut hasher = Sha256::new();
    hasher.update(nanos.to_le_bytes());
    hasher.update(pid.to_le_bytes());
    hasher.update(n.to_le_bytes());
    hex::encode(hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    /// 合成 fixture raw key（不使用 TECHNICAL_BASELINE.md 中的真实基线 key）。
    /// 仅供 fixture 测试：创建加密 fixture 并验证探测逻辑，不接触真实 TRAE 数据库。
    /// 真实基线 key 只存在于 docs/TECHNICAL_BASELINE.md，不进入源码、日志或证据。
    const TEST_RAW_KEY: &str = "deadbeefcafebabe1234567890abcdefdeadbeefcafebabe1234567890abcdef";

    /// 错误 key（与基线不同的 64 字符 hex）
    const WRONG_KEY: &str = "0000000000000000000000000000000000000000000000000000000000000000";

    /// 构造合成 Work CN SQLCipher 数据库
    fn make_work_cn_fixture(dir: &Path) -> PathBuf {
        let db_path = dir.join("database.db");
        let conn = Connection::open(&db_path).unwrap();
        conn.execute_batch(&format!("PRAGMA key = \"x'{}'\";", TEST_RAW_KEY))
            .unwrap();
        conn.execute_batch(
            r#"
            CREATE TABLE project (
                project_id TEXT PRIMARY KEY,
                user_id TEXT NOT NULL,
                biz_project_id TEXT NOT NULL,
                UNIQUE (biz_project_id, user_id)
            );
            CREATE TABLE chat_session (
                session_id TEXT PRIMARY KEY,
                project_id TEXT NOT NULL
            );
            CREATE TABLE chat_message (
                message_id TEXT PRIMARY KEY,
                session_id TEXT NOT NULL
            );
            INSERT INTO project VALUES ('p1', '1000000000000001', 'biz-1');
            INSERT INTO chat_session VALUES ('s1', 'p1');
            INSERT INTO chat_message VALUES ('m1', 's1');
            "#,
        )
        .unwrap();
        // WAL checkpoint 确保全部写入主数据库文件
        conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")
            .unwrap();
        drop(conn);
        db_path
    }

    /// 构造含未 checkpoint WAL 的 fixture
    fn make_work_cn_fixture_with_wal(dir: &Path) -> PathBuf {
        let db_path = dir.join("database.db");
        let conn = Connection::open(&db_path).unwrap();
        conn.execute_batch(&format!("PRAGMA key = \"x'{}'\";", TEST_RAW_KEY))
            .unwrap();
        conn.execute_batch("PRAGMA journal_mode=WAL;").unwrap();
        conn.execute_batch(
            r#"
            CREATE TABLE project (
                project_id TEXT PRIMARY KEY,
                user_id TEXT NOT NULL,
                biz_project_id TEXT NOT NULL,
                UNIQUE (biz_project_id, user_id)
            );
            CREATE TABLE chat_session (
                session_id TEXT PRIMARY KEY,
                project_id TEXT NOT NULL
            );
            CREATE TABLE chat_message (
                message_id TEXT PRIMARY KEY,
                session_id TEXT NOT NULL
            );
            INSERT INTO project VALUES ('p1', '1000000000000001', 'biz-1');
            INSERT INTO chat_session VALUES ('s1', 'p1');
            INSERT INTO chat_message VALUES ('m1', 's1');
            "#,
        )
        .unwrap();
        // 不 checkpoint——已提交记录留在 WAL
        // 关闭连接让 WAL 落盘
        drop(conn);
        db_path
    }

    #[test]
    fn probe_work_cn_db_with_correct_key_returns_verified() {
        let dir = tempdir().unwrap();
        let db_path = make_work_cn_fixture(dir.path());
        let probe = SqlCipherProbe::new();
        let state = probe.probe_database(&db_path, TEST_RAW_KEY);
        match state {
            CompatibilityState::Verified { counts, .. } => {
                assert_eq!(counts.project_count, 1);
                assert_eq!(counts.chat_session_count, 1);
                assert_eq!(counts.chat_message_count, 1);
            }
            CompatibilityState::Incompatible { reason } => {
                panic!("期望 Verified，实际 Incompatible: {:?}", reason);
            }
        }
    }

    #[test]
    fn probe_work_cn_db_with_wrong_key_returns_incompatible_wrong_key() {
        let dir = tempdir().unwrap();
        let db_path = make_work_cn_fixture(dir.path());
        let probe = SqlCipherProbe::new();
        let state = probe.probe_database(&db_path, WRONG_KEY);
        match state {
            CompatibilityState::Incompatible { reason } => {
                assert_eq!(reason, IncompatibleReason::WrongKey);
            }
            CompatibilityState::Verified { .. } => {
                panic!("期望 WrongKey，实际 Verified");
            }
        }
    }

    #[test]
    fn probe_truncated_file_returns_truncated_file() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("truncated.db");
        std::fs::write(&db_path, b"short").unwrap();
        let probe = SqlCipherProbe::new();
        let state = probe.probe_database(&db_path, TEST_RAW_KEY);
        match state {
            CompatibilityState::Incompatible { reason } => {
                assert_eq!(reason, IncompatibleReason::TruncatedFile);
            }
            _ => panic!("期望 TruncatedFile"),
        }
    }

    #[test]
    fn probe_unknown_schema_returns_unknown_schema() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("unknown.db");
        let conn = Connection::open(&db_path).unwrap();
        conn.execute_batch(&format!("PRAGMA key = \"x'{}'\";", TEST_RAW_KEY))
            .unwrap();
        // 缺关键表
        conn.execute_batch("CREATE TABLE other_table (id INTEGER);")
            .unwrap();
        drop(conn);
        let probe = SqlCipherProbe::new();
        let state = probe.probe_database(&db_path, TEST_RAW_KEY);
        match state {
            CompatibilityState::Incompatible { reason } => match reason {
                IncompatibleReason::UnknownSchema { missing_tables } => {
                    assert!(missing_tables.contains(&"project".to_string()));
                }
                _ => panic!("期望 UnknownSchema，实际 {:?}", reason),
            },
            _ => panic!("期望 Incompatible"),
        }
    }

    #[test]
    fn backup_to_logical_copy_preserves_wal_content() {
        let dir = tempdir().unwrap();
        let db_path = make_work_cn_fixture_with_wal(dir.path());
        let probe = SqlCipherProbe::new();

        // 逻辑副本应保留未 checkpoint WAL 的已提交记录
        let copy_path = probe.backup_to_logical_copy(&db_path, TEST_RAW_KEY);
        assert!(copy_path.is_some(), "Backup API 应成功");

        let copy = copy_path.unwrap();
        let state = probe.probe_database(&copy, TEST_RAW_KEY);
        match state {
            CompatibilityState::Verified { counts, .. } => {
                // WAL 中的已提交记录应进入逻辑副本
                assert_eq!(counts.project_count, 1);
                assert_eq!(counts.chat_session_count, 1);
                assert_eq!(counts.chat_message_count, 1);
            }
            _ => panic!("逻辑副本应可读"),
        }
    }

    #[test]
    fn verify_transaction_rollback_passes_on_fixture_copy() {
        let dir = tempdir().unwrap();
        let db_path = make_work_cn_fixture(dir.path());
        let probe = SqlCipherProbe::new();
        let copy_path = probe
            .backup_to_logical_copy(&db_path, TEST_RAW_KEY)
            .unwrap();
        let result = probe.verify_transaction_rollback(&copy_path, TEST_RAW_KEY);
        assert!(result, "事务提交与回滚应通过");
    }

    #[test]
    fn run_integrity_checks_pass_on_verified_db() {
        let dir = tempdir().unwrap();
        let db_path = make_work_cn_fixture(dir.path());
        let probe = SqlCipherProbe::new();
        let (cipher_ok, sqlite_ok) = probe.run_integrity_checks(&db_path, TEST_RAW_KEY);
        assert!(cipher_ok, "cipher_integrity_check 应无错误");
        assert!(sqlite_ok, "integrity_check 应返回 ok");
    }

    #[test]
    fn create_random_key_catalog_creates_and_reopens() {
        let dir = tempdir().unwrap();
        let probe = SqlCipherProbe::new();
        let result = probe.create_random_key_catalog(dir.path());
        assert!(result.is_some(), "应成功创建随机密钥目录库");
        let catalog_path = result.unwrap();
        assert!(catalog_path.exists());
    }

    /// R1：计算文件 SHA-256（用于零字节写证据）
    fn file_sha256(path: &std::path::Path) -> String {
        use sha2::Digest;
        let bytes = std::fs::read(path).unwrap();
        let mut hasher = sha2::Sha256::new();
        hasher.update(&bytes);
        hex::encode(hasher.finalize())
    }

    /// R1：读取 DB + WAL + SHM 三件套的字节快照（不存在的文件计为空 Vec）
    fn snapshot_db_trio(dir: &std::path::Path, db_name: &str) -> (Vec<u8>, Vec<u8>, Vec<u8>) {
        let db = dir.join(db_name);
        let wal = dir.join(format!("{}-wal", db_name));
        let shm = dir.join(format!("{}-shm", db_name));
        (
            std::fs::read(&db).unwrap_or_default(),
            std::fs::read(&wal).unwrap_or_default(),
            std::fs::read(&shm).unwrap_or_default(),
        )
    }

    /// R1：正确 key 探测保持 DB/WAL/SHM 字节级不变。
    ///
    /// 只读 flags (SQLITE_OPEN_READ_ONLY) 保证 SQLite 不会在探测期间创建/写入文件。
    /// 这是 R1 的零字节写核心证据——任何字节差异都视为只读封闭失败。
    #[test]
    fn probe_with_correct_key_has_zero_writes_to_db_trio() {
        let dir = tempdir().unwrap();
        let db_path = make_work_cn_fixture(dir.path());

        let (db_before, wal_before, shm_before) = snapshot_db_trio(dir.path(), "database.db");
        let db_hash_before = file_sha256(&db_path);

        let probe = SqlCipherProbe::new();
        let state = probe.probe_database(&db_path, TEST_RAW_KEY);
        // 确认探测成功（验证走了真正的解密路径）
        assert!(matches!(state, CompatibilityState::Verified { .. }));

        let (db_after, wal_after, shm_after) = snapshot_db_trio(dir.path(), "database.db");
        let db_hash_after = file_sha256(&db_path);

        // DB 主文件字节级不变
        assert_eq!(
            db_before, db_after,
            "R1 失败：DB 主文件字节发生变化（hash {} -> {}）",
            db_hash_before, db_hash_after
        );
        // WAL 字节不变（不应被 checkpoint 或追加）
        assert_eq!(wal_before, wal_after, "R1 失败：WAL 字节发生变化");
        // SHM 字节不变（不应被创建或修改）
        assert_eq!(shm_before, shm_after, "R1 失败：SHM 字节发生变化");
    }

    /// R1：错误 key 探测保持 DB/WAL/SHM 字节级不变。
    #[test]
    fn probe_with_wrong_key_has_zero_writes_to_db_trio() {
        let dir = tempdir().unwrap();
        let db_path = make_work_cn_fixture(dir.path());

        let (db_before, wal_before, shm_before) = snapshot_db_trio(dir.path(), "database.db");

        let probe = SqlCipherProbe::new();
        let state = probe.probe_database(&db_path, WRONG_KEY);
        // 确认返回 WrongKey（走了错误 key 失败路径）
        assert!(matches!(
            state,
            CompatibilityState::Incompatible {
                reason: IncompatibleReason::WrongKey
            }
        ));

        let (db_after, wal_after, shm_after) = snapshot_db_trio(dir.path(), "database.db");
        assert_eq!(db_before, db_after, "R1 失败：错误 key 修改了 DB 主文件");
        assert_eq!(wal_before, wal_after, "R1 失败：错误 key 修改了 WAL");
        assert_eq!(shm_before, shm_after, "R1 失败：错误 key 修改了 SHM");
    }

    /// R1：探测失败（截断文件）保持 DB/WAL/SHM 字节级不变。
    #[test]
    fn probe_with_truncated_file_has_zero_writes_to_db_trio() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("truncated.db");
        std::fs::write(&db_path, b"short").unwrap();

        let (db_before, wal_before, shm_before) = snapshot_db_trio(dir.path(), "truncated.db");

        let probe = SqlCipherProbe::new();
        let state = probe.probe_database(&db_path, TEST_RAW_KEY);
        assert!(matches!(
            state,
            CompatibilityState::Incompatible {
                reason: IncompatibleReason::TruncatedFile
            }
        ));

        let (db_after, wal_after, shm_after) = snapshot_db_trio(dir.path(), "truncated.db");
        assert_eq!(db_before, db_after, "R1 失败：截断文件路径修改了 DB");
        assert_eq!(
            wal_before, wal_after,
            "R1 失败：截断文件路径创建/修改了 WAL"
        );
        assert_eq!(
            shm_before, shm_after,
            "R1 失败：截断文件路径创建/修改了 SHM"
        );
    }

    /// R1：探测不存在的文件不创建 DB/WAL/SHM。
    #[test]
    fn probe_with_missing_file_does_not_create_db_trio() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("never-exists.db");

        let probe = SqlCipherProbe::new();
        let state = probe.probe_database(&db_path, TEST_RAW_KEY);
        assert!(matches!(
            state,
            CompatibilityState::Incompatible {
                reason: IncompatibleReason::TruncatedFile
            }
        ));

        // 文件不存在——只读 flags 不应创建任何文件
        assert!(!db_path.exists(), "R1 失败：探测创建了 DB 主文件");
        assert!(
            !dir.path().join("never-exists.db-wal").exists(),
            "R1 失败：探测创建了 WAL"
        );
        assert!(
            !dir.path().join("never-exists.db-shm").exists(),
            "R1 失败：探测创建了 SHM"
        );
    }

    /// R5：cipher_version 校验返回兼容（fixture 使用 bundled-sqlcipher，应为 4.5.x 或 4.6.x）
    #[test]
    fn probe_work_cn_db_returns_cipher_version_compatible() {
        let dir = tempdir().unwrap();
        let db_path = make_work_cn_fixture(dir.path());
        let probe = SqlCipherProbe::new();
        let state = probe.probe_database(&db_path, TEST_RAW_KEY);
        // cipher_version 校验通过——state 必须是 Verified，不能是 CipherVersionMismatch
        match state {
            CompatibilityState::Verified { .. } => {}
            CompatibilityState::Incompatible {
                reason: IncompatibleReason::CipherVersionMismatch { version },
            } => {
                panic!("cipher_version 不兼容：{}", version);
            }
            other => panic!("期望 Verified，实际 {:?}", other),
        }
    }

    #[test]
    fn generate_random_hex_key_produces_64_chars() {
        let key = generate_random_hex_key();
        assert_eq!(key.len(), 64);
        assert!(key.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn generate_random_hex_key_unique_per_call() {
        let k1 = generate_random_hex_key();
        let k2 = generate_random_hex_key();
        assert_ne!(k1, k2, "连续调用应产生不同 key");
    }
}
