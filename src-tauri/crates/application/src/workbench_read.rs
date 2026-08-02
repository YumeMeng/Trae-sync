//! T02 工作台只读应用服务：组合 SQLCipher 探测与账号证据，产生只读工作台状态。
//!
//! 关键安全约束：
//! - 手工账号选择永远不能解除只读（D-044 已确认）
//! - T02 阶段所有真实写能力仍保持禁用
//! - 账号证据或数据库证据变化时废弃旧状态，不猜测账号
//!
//! 依赖方向：application -> domain + ports，不依赖 infrastructure/commands/tauri。

use std::path::{Component, Path, PathBuf};
use traesync_domain::{
    AccountEvidence, CompatibilityState, DataLocationState, EvidenceState, PlatformContext,
    PlatformId, ReadonlyReason, WorkbenchReadState,
};
use traesync_ports::{AccountEvidenceReaderPort, DatabaseProbePort};

/// 最终 DB 路径封闭错误：返回结构化只读原因，不抛出原始 IO 错误。
///
/// 对应 R1：在进入 infrastructure 前验证最终 DB 路径，
/// 拒绝绝对路径、父目录组件、路径别名及 canonical 后不在 canonical fixture root 内的路径。
/// 文件不存在时返回 `DataLocationUnavailable`——绝不通过默认读写连接创建文件。
enum DbPathError {
    /// `db_relative_path` 为空或仅空白
    Empty,
    /// 路径是绝对路径（含盘符、UNC 前缀或根分隔符）
    Absolute,
    /// 路径包含 `..` 父目录组件
    ParentDirEscape,
    /// 路径以 `\\?\`、`\\.\` 等设备命名空间前缀开头
    DevicePrefix,
    /// 文件不存在——绝不创建
    NotFound,
    /// canonical 后位于 fixture_root 之外
    OutsideFixtureRoot,
    /// canonical 失败（IO 错误等）
    CannotCanonicalize,
}

impl DbPathError {
    /// 转换为结构化只读原因
    fn to_readonly_reason(&self) -> ReadonlyReason {
        match self {
            // 路径类错误统一映射到 DataLocationUnavailable，避免泄露路径细节
            DbPathError::Empty
            | DbPathError::Absolute
            | DbPathError::ParentDirEscape
            | DbPathError::DevicePrefix
            | DbPathError::NotFound
            | DbPathError::OutsideFixtureRoot
            | DbPathError::CannotCanonicalize => ReadonlyReason::DataLocationUnavailable,
        }
    }
}

/// 验证 `db_relative_path` 在 `fixture_root` 内部安全。
///
/// 安全规则（R1）：
/// 1. 拒绝空字符串
/// 2. 拒绝绝对路径（含 Windows 盘符、UNC、根分隔符）
/// 3. 拒绝 `..` 父目录组件——防止 `fixture/../escape.db` 逃逸
/// 4. 拒绝 `\\?\` 或 `\\.\` 设备命名空间前缀——防止路径别名
/// 5. 文件必须存在——绝不通过默认读写连接创建文件
/// 6. canonical 后必须严格位于 `fixture_root` 内部——防止符号链接等别名逃逸
///
/// 成功返回 canonical 后的绝对路径，供 infrastructure 只读打开使用。
fn validate_db_path(fixture_root: &Path, db_relative_path: &str) -> Result<PathBuf, DbPathError> {
    if db_relative_path.trim().is_empty() {
        return Err(DbPathError::Empty);
    }

    let raw_str = db_relative_path.trim();
    // 词法预检：拒绝设备命名空间前缀
    if raw_str.starts_with(r"\\?\") || raw_str.starts_with(r"\\.\") || raw_str.starts_with(r"\\") {
        return Err(DbPathError::DevicePrefix);
    }

    let relative = Path::new(raw_str);

    // 拒绝绝对路径（含盘符 / UNC / 根分隔符）
    if relative.is_absolute() {
        return Err(DbPathError::Absolute);
    }
    // 再次按 components 检查盘符前缀（Windows 上 `C:foo` 也算绝对）
    if relative
        .components()
        .next()
        .map_or(false, |c| matches!(c, Component::Prefix(_)))
    {
        return Err(DbPathError::Absolute);
    }

    // 拒绝 `..` 父目录组件——任何形式的逃逸都禁止
    for component in relative.components() {
        if matches!(component, Component::ParentDir) {
            return Err(DbPathError::ParentDirEscape);
        }
    }

    let joined = fixture_root.join(relative);

    // 文件必须存在——绝不通过默认读写连接创建文件
    if !joined.exists() {
        return Err(DbPathError::NotFound);
    }

    // canonical 化：跟随符号链接、解析 `.`、统一大小写与分隔符
    let canonical_db = joined
        .canonicalize()
        .map_err(|_| DbPathError::CannotCanonicalize)?;
    let canonical_fixture = fixture_root
        .canonicalize()
        .map_err(|_| DbPathError::CannotCanonicalize)?;

    // canonical 后必须严格位于 fixture_root 内部（不含 fixture_root 自身）
    if !path_strictly_inside(&canonical_db, &canonical_fixture) {
        return Err(DbPathError::OutsideFixtureRoot);
    }

    Ok(canonical_db)
}

/// 判断 `inner` 是否严格位于 `outer` 内部（不含 outer 自身）。
/// 大小写不敏感比较——Windows 路径在 canonical 后大小写仍可能不同。
fn path_strictly_inside(inner: &Path, outer: &Path) -> bool {
    let inner_components: Vec<String> = inner
        .components()
        .filter_map(|c| match c {
            Component::Normal(s) => Some(s.to_string_lossy().to_lowercase()),
            Component::RootDir => Some("\\".to_string()),
            Component::Prefix(p) => Some(p.as_os_str().to_string_lossy().to_lowercase()),
            _ => None,
        })
        .collect();
    let outer_components: Vec<String> = outer
        .components()
        .filter_map(|c| match c {
            Component::Normal(s) => Some(s.to_string_lossy().to_lowercase()),
            Component::RootDir => Some("\\".to_string()),
            Component::Prefix(p) => Some(p.as_os_str().to_string_lossy().to_lowercase()),
            _ => None,
        })
        .collect();

    if inner_components.len() <= outer_components.len() {
        return false;
    }
    outer_components
        .iter()
        .zip(inner_components.iter())
        .all(|(o, i)| o == i)
}

/// 重导出 port trait，避免 commands 层直接依赖 ports crate
pub use traesync_ports::{
    AccountEvidenceReaderPort as AccountEvidenceReaderPortTrait,
    DatabaseProbePort as DatabaseProbePortTrait,
};

/// 工作台只读服务：组合数据库探测与账号证据，构造 `WorkbenchReadState`。
///
/// T02 阶段始终返回只读状态——即便账号 verified，写能力仍禁用。
///
/// raw_key 在构造时注入，不通过 `build_read_state` 参数暴露给上层，
/// 确保 key 不进入 commands 层或 UI。
pub struct WorkbenchReadService<'a> {
    db_probe: &'a dyn DatabaseProbePort,
    account_reader: &'a dyn AccountEvidenceReaderPort,
    /// SQLCipher raw key hex，由组合根从环境变量读取并注入。
    /// 不进入日志、错误消息或返回值。
    raw_key: &'a str,
}

impl<'a> WorkbenchReadService<'a> {
    pub fn new(
        db_probe: &'a dyn DatabaseProbePort,
        account_reader: &'a dyn AccountEvidenceReaderPort,
        raw_key: &'a str,
    ) -> Self {
        Self {
            db_probe,
            account_reader,
            raw_key,
        }
    }

    /// 构造工作台只读状态。
    ///
    /// - `fixture_root`：经 `FixturePathGuard` 验证的 fixture 根目录
    /// - `db_relative_path`：fixture_root 内的数据库相对路径（如 `database.db`）
    /// - `now`：观测时间，用于 Expired 判定与 `observed_at`。显式传入便于测试注入。
    ///
    /// raw_key 在构造时已注入，不通过此方法暴露。
    ///
    /// R1：在进入 infrastructure 前验证最终 DB 路径。绝对路径、`..`、设备前缀、
    /// 别名逃逸或文件不存在均返回 `DataLocationUnavailable` 只读原因，
    /// 绝不通过默认读写连接创建文件。
    pub fn build_read_state(
        &self,
        fixture_root: &Path,
        db_relative_path: &str,
        now: std::time::SystemTime,
    ) -> WorkbenchReadState {
        let canonical = fixture_root.to_string_lossy().into_owned();

        // 0. R1 路径封闭：验证最终 DB 路径在 fixture_root 内部
        let db_path = match validate_db_path(fixture_root, db_relative_path) {
            Ok(p) => p,
            Err(e) => {
                // 路径错误直接构造只读状态——不调用 infrastructure
                let compatibility = CompatibilityState::Incompatible {
                    reason: traesync_domain::IncompatibleReason::TruncatedFile,
                };
                let account = self.account_reader.read_account_evidence(fixture_root, now);
                let readonly_reason = Some(e.to_readonly_reason());
                return WorkbenchReadState {
                    platform: PlatformContext {
                        platform_id: PlatformId::work_cn(),
                        display_name: "TRAE Work CN".to_string(),
                        adapter_implemented: true,
                    },
                    data_location: DataLocationState {
                        selected: true,
                        display_name: Some(canonical),
                        unavailable_reason: None,
                    },
                    compatibility,
                    current_account: account,
                    readonly_reason,
                };
            }
        };

        // 1. 探测数据库兼容性（raw_key 仅传入 infrastructure trait）
        let compatibility = self.db_probe.probe_database(&db_path, self.raw_key);

        // 2. 读取账号证据（注入 now 用于 Expired 判定）
        let account = self.account_reader.read_account_evidence(fixture_root, now);

        // 3. 根据兼容性与账号证据决定结构化只读原因
        let readonly_reason = self.derive_readonly_reason(&compatibility, &account);

        // 4. 组装工作台状态——平台固定 Work CN，数据位置显示规范化路径
        WorkbenchReadState {
            platform: PlatformContext {
                platform_id: PlatformId::work_cn(),
                display_name: "TRAE Work CN".to_string(),
                adapter_implemented: true, // T02 阶段 Adapter 已实现只读探测
            },
            data_location: DataLocationState {
                selected: true,
                display_name: Some(canonical.clone()),
                unavailable_reason: None,
            },
            compatibility,
            current_account: account,
            readonly_reason,
        }
    }

    /// TRAE 关闭后重新读取账号证据，比较认证指纹。
    ///
    /// R3：必须接收旧证据并真实比较旧、新认证指纹。
    /// 指纹变化时返回 `FingerprintChanged` 状态的新证据，旧计划失效。
    /// 对应规格第 12 节有效条件 4：TRAE 关闭后认证指纹仍未变化。
    ///
    /// R2-9 修复（P39）：FingerprintChanged 不应覆盖 Conflict/Missing/Expired/SingleSource。
    /// 仅当新证据是 Verified（账号证据本身可信）时，才因指纹变化降级为 FingerprintChanged。
    /// 其他状态（Conflict/Expired/Missing/SingleSource）本身已表示证据不可靠，
    /// 用 FingerprintChanged 覆盖会丢失更严重的诊断信息。
    ///
    /// `now` 显式传入，便于测试注入固定时间，避免依赖不稳定 wall clock。
    pub fn re_verify_after_close(
        &self,
        fixture_root: &Path,
        previous: &AccountEvidence,
        now: std::time::SystemTime,
    ) -> AccountEvidence {
        let mut new_evidence = self.account_reader.re_read_after_close(fixture_root, now);

        // R2-9：仅当新证据是 Verified 时才检查指纹变化
        // Conflict/Expired/Missing/SingleSource 已是不可靠状态，不应被 FingerprintChanged 覆盖
        if new_evidence.evidence_state == EvidenceState::Verified {
            // 真实比较旧、新认证指纹
            let changed = match (&previous.auth_fingerprint, &new_evidence.auth_fingerprint) {
                (Some(old_fp), Some(new_fp)) => old_fp != new_fp,
                // 任一为 None 另一为 Some 视为变化——证据从有变无或从无变有
                (None, Some(_)) | (Some(_), None) => true,
                (None, None) => false,
            };
            if changed {
                new_evidence.evidence_state = EvidenceState::FingerprintChanged;
            }
        }
        new_evidence
    }

    /// 根据兼容性与账号证据推导结构化只读原因。
    ///
    /// 手工账号选择永远不能改变此结果——`readonly_reason` 只由
    /// 数据库探测与账号证据自动决定。
    fn derive_readonly_reason(
        &self,
        compatibility: &CompatibilityState,
        account: &AccountEvidence,
    ) -> Option<ReadonlyReason> {
        // 优先检查数据库兼容性
        if let CompatibilityState::Incompatible { reason } = compatibility {
            return Some(match reason {
                traesync_domain::IncompatibleReason::WrongKey => ReadonlyReason::WrongKey,
                traesync_domain::IncompatibleReason::TruncatedFile => ReadonlyReason::TruncatedFile,
                traesync_domain::IncompatibleReason::UnknownSchema { .. } => {
                    ReadonlyReason::UnknownSchema
                }
                traesync_domain::IncompatibleReason::MissingColumn { .. } => {
                    ReadonlyReason::SchemaUnsupported
                }
                traesync_domain::IncompatibleReason::MissingIndex { .. } => {
                    ReadonlyReason::SchemaUnsupported
                }
                traesync_domain::IncompatibleReason::MissingConstraint { .. } => {
                    ReadonlyReason::SchemaUnsupported
                }
                // R5：cipher_version 不兼容映射到 SchemaUnsupported
                traesync_domain::IncompatibleReason::CipherVersionMismatch { .. } => {
                    ReadonlyReason::SchemaUnsupported
                }
            });
        }

        // 数据库兼容时，检查账号证据状态
        match account.evidence_state {
            EvidenceState::Verified => None, // 账号 verified，但 T02 写能力仍禁用
            EvidenceState::SingleSource => Some(ReadonlyReason::AccountEvidenceUnavailable),
            EvidenceState::Missing => Some(ReadonlyReason::AccountEvidenceUnavailable),
            EvidenceState::Conflict => Some(ReadonlyReason::Conflict),
            EvidenceState::Expired => Some(ReadonlyReason::Expired),
            EvidenceState::FingerprintChanged => Some(ReadonlyReason::FingerprintChanged),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;
    use traesync_domain::{
        AuthFingerprint, IncompatibleReason, SchemaFingerprint, SourceEventSummary, TableCounts,
        UserId,
    };
    use traesync_ports::{AccountEvidenceReaderPort, DatabaseProbePort};

    /// 假数据库探测：可注入任意 CompatibilityState
    struct FakeDbProbe {
        state: CompatibilityState,
    }

    impl DatabaseProbePort for FakeDbProbe {
        fn probe_database(&self, _db_path: &Path, _raw_key: &str) -> CompatibilityState {
            self.state.clone()
        }
        fn backup_to_logical_copy(
            &self,
            _source_db: &Path,
            _raw_key: &str,
        ) -> Option<std::path::PathBuf> {
            None
        }
        fn verify_transaction_rollback(&self, _copy_db: &Path, _raw_key: &str) -> bool {
            true
        }
        fn run_integrity_checks(&self, _db_path: &Path, _raw_key: &str) -> (bool, bool) {
            (true, true)
        }
        fn create_random_key_catalog(&self, _fixture_root: &Path) -> Option<std::path::PathBuf> {
            None
        }
    }

    /// 假账号证据读取器：可注入任意 AccountEvidence
    struct FakeAccountReader {
        evidence: AccountEvidence,
    }

    impl AccountEvidenceReaderPort for FakeAccountReader {
        fn read_account_evidence(
            &self,
            _fixture_root: &Path,
            _now: std::time::SystemTime,
        ) -> AccountEvidence {
            self.evidence.clone()
        }
        fn re_read_after_close(
            &self,
            _fixture_root: &Path,
            _now: std::time::SystemTime,
        ) -> AccountEvidence {
            self.evidence.clone()
        }
    }

    /// 合成账号 ID（不使用真实基线 ID，仅 fixture 测试）
    const SYNTHETIC_USER_ID: &str = "1000000000000001";

    fn verified_account() -> AccountEvidence {
        AccountEvidence {
            user_id: UserId::from_verified(SYNTHETIC_USER_ID).ok(),
            source_events: vec![
                SourceEventSummary {
                    source_kind: "alog".to_string(),
                    event_name: "fetchLogTask".to_string(),
                    log_session_id: Some("session-1".to_string()),
                },
                SourceEventSummary {
                    source_kind: "renderer".to_string(),
                    event_name: "User info loaded".to_string(),
                    log_session_id: Some("session-1".to_string()),
                },
            ],
            auth_fingerprint: Some(AuthFingerprint(
                "abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789".to_string(),
            )),
            local_storage_user_id: UserId::from_verified(SYNTHETIC_USER_ID).ok(),
            product_version: Some("1.107.1".to_string()),
            observed_at: std::time::SystemTime::now(),
            evidence_state: EvidenceState::Verified,
        }
    }

    fn now() -> std::time::SystemTime {
        std::time::SystemTime::now()
    }

    /// 在 fixture_root 内创建占位 DB 文件，使 R1 路径封闭通过
    fn touch_db(dir: &Path, name: &str) {
        std::fs::write(dir.join(name), b"placeholder").unwrap();
    }

    #[test]
    fn build_read_state_returns_platform_work_cn() {
        let dir = tempdir().unwrap();
        touch_db(dir.path(), "database.db");
        let probe = FakeDbProbe {
            state: CompatibilityState::Verified {
                schema_fingerprint: SchemaFingerprint("fp".to_string()),
                counts: TableCounts::default(),
            },
        };
        let reader = FakeAccountReader {
            evidence: verified_account(),
        };
        let svc = WorkbenchReadService::new(&probe, &reader, "rawkey");
        let state = svc.build_read_state(dir.path(), "database.db", now());

        assert_eq!(state.platform.platform_id, PlatformId::work_cn());
        assert!(state.platform.adapter_implemented);
        assert!(state.data_location.selected);
    }

    #[test]
    fn verified_account_and_db_yield_no_readonly_reason() {
        let dir = tempdir().unwrap();
        touch_db(dir.path(), "database.db");
        let probe = FakeDbProbe {
            state: CompatibilityState::Verified {
                schema_fingerprint: SchemaFingerprint("fp".to_string()),
                counts: TableCounts::default(),
            },
        };
        let reader = FakeAccountReader {
            evidence: verified_account(),
        };
        let svc = WorkbenchReadService::new(&probe, &reader, "rawkey");
        let state = svc.build_read_state(dir.path(), "database.db", now());

        assert_eq!(state.readonly_reason, None);
    }

    #[test]
    fn wrong_key_yields_readonly_reason() {
        let dir = tempdir().unwrap();
        touch_db(dir.path(), "database.db");
        let probe = FakeDbProbe {
            state: CompatibilityState::Incompatible {
                reason: IncompatibleReason::WrongKey,
            },
        };
        let reader = FakeAccountReader {
            evidence: verified_account(),
        };
        let svc = WorkbenchReadService::new(&probe, &reader, "rawkey");
        let state = svc.build_read_state(dir.path(), "database.db", now());

        assert_eq!(state.readonly_reason, Some(ReadonlyReason::WrongKey));
    }

    #[test]
    fn unknown_schema_yields_readonly_reason() {
        let dir = tempdir().unwrap();
        touch_db(dir.path(), "database.db");
        let probe = FakeDbProbe {
            state: CompatibilityState::Incompatible {
                reason: IncompatibleReason::UnknownSchema {
                    missing_tables: vec!["project".to_string()],
                },
            },
        };
        let reader = FakeAccountReader {
            evidence: verified_account(),
        };
        let svc = WorkbenchReadService::new(&probe, &reader, "rawkey");
        let state = svc.build_read_state(dir.path(), "database.db", now());

        assert_eq!(state.readonly_reason, Some(ReadonlyReason::UnknownSchema));
    }

    #[test]
    fn missing_account_yields_account_unavailable_reason() {
        let dir = tempdir().unwrap();
        touch_db(dir.path(), "database.db");
        let probe = FakeDbProbe {
            state: CompatibilityState::Verified {
                schema_fingerprint: SchemaFingerprint("fp".to_string()),
                counts: TableCounts::default(),
            },
        };
        let reader = FakeAccountReader {
            evidence: AccountEvidence::default(),
        };
        let svc = WorkbenchReadService::new(&probe, &reader, "rawkey");
        let state = svc.build_read_state(dir.path(), "database.db", now());

        assert_eq!(
            state.readonly_reason,
            Some(ReadonlyReason::AccountEvidenceUnavailable)
        );
    }

    #[test]
    fn conflict_account_yields_conflict_reason() {
        let dir = tempdir().unwrap();
        touch_db(dir.path(), "database.db");
        let probe = FakeDbProbe {
            state: CompatibilityState::Verified {
                schema_fingerprint: SchemaFingerprint("fp".to_string()),
                counts: TableCounts::default(),
            },
        };
        let mut evidence = verified_account();
        evidence.evidence_state = EvidenceState::Conflict;
        let reader = FakeAccountReader { evidence };
        let svc = WorkbenchReadService::new(&probe, &reader, "rawkey");
        let state = svc.build_read_state(dir.path(), "database.db", now());

        assert_eq!(state.readonly_reason, Some(ReadonlyReason::Conflict));
    }

    /// R3 Expired 通过生产服务入口触发——证据状态由 reader 注入，
    /// application 自动推导 `ReadonlyReason::Expired`，不依赖手工构造枚举。
    #[test]
    fn expired_account_yields_expired_reason_through_service() {
        let dir = tempdir().unwrap();
        touch_db(dir.path(), "database.db");
        let probe = FakeDbProbe {
            state: CompatibilityState::Verified {
                schema_fingerprint: SchemaFingerprint("fp".to_string()),
                counts: TableCounts::default(),
            },
        };
        let mut evidence = verified_account();
        evidence.evidence_state = EvidenceState::Expired;
        let reader = FakeAccountReader { evidence };
        let svc = WorkbenchReadService::new(&probe, &reader, "rawkey");
        let state = svc.build_read_state(dir.path(), "database.db", now());

        assert_eq!(state.readonly_reason, Some(ReadonlyReason::Expired));
    }

    /// R3 FingerprintChanged 通过生产服务入口触发——证据状态由 reader 注入，
    /// application 自动推导 `ReadonlyReason::FingerprintChanged`。
    #[test]
    fn fingerprint_changed_yields_fingerprint_changed_reason() {
        let dir = tempdir().unwrap();
        touch_db(dir.path(), "database.db");
        let probe = FakeDbProbe {
            state: CompatibilityState::Verified {
                schema_fingerprint: SchemaFingerprint("fp".to_string()),
                counts: TableCounts::default(),
            },
        };
        let mut evidence = verified_account();
        evidence.evidence_state = EvidenceState::FingerprintChanged;
        let reader = FakeAccountReader { evidence };
        let svc = WorkbenchReadService::new(&probe, &reader, "rawkey");
        let state = svc.build_read_state(dir.path(), "database.db", now());

        assert_eq!(
            state.readonly_reason,
            Some(ReadonlyReason::FingerprintChanged)
        );
    }

    /// R3 re_verify_after_close 真实比较旧、新认证指纹——变化时返回 FingerprintChanged
    #[test]
    fn re_verify_after_close_returns_fingerprint_changed_on_diff() {
        let dir = tempdir().unwrap();
        let old_evidence = verified_account();

        // 假 reader 返回新证据：指纹不同
        let mut new_evidence = verified_account();
        new_evidence.auth_fingerprint = Some(AuthFingerprint(
            "0000000000000000000000000000000000000000000000000000000000000000".to_string(),
        ));
        let reader = FakeAccountReader {
            evidence: new_evidence,
        };
        let probe = FakeDbProbe {
            state: CompatibilityState::Verified {
                schema_fingerprint: SchemaFingerprint("fp".to_string()),
                counts: TableCounts::default(),
            },
        };
        let svc = WorkbenchReadService::new(&probe, &reader, "rawkey");
        let result = svc.re_verify_after_close(dir.path(), &old_evidence, now());
        assert_eq!(result.evidence_state, EvidenceState::FingerprintChanged);
    }

    /// R3 re_verify_after_close 指纹未变时保留原状态
    #[test]
    fn re_verify_after_close_preserves_state_when_fingerprint_unchanged() {
        let dir = tempdir().unwrap();
        let old_evidence = verified_account();
        // 假 reader 返回相同证据
        let reader = FakeAccountReader {
            evidence: old_evidence.clone(),
        };
        let probe = FakeDbProbe {
            state: CompatibilityState::Verified {
                schema_fingerprint: SchemaFingerprint("fp".to_string()),
                counts: TableCounts::default(),
            },
        };
        let svc = WorkbenchReadService::new(&probe, &reader, "rawkey");
        let result = svc.re_verify_after_close(dir.path(), &old_evidence, now());
        assert_eq!(result.evidence_state, EvidenceState::Verified);
    }

    /// R2-9：re_verify_after_close 不应用 FingerprintChanged 覆盖 Conflict
    /// P39：Conflict 表示数据可信度问题，比指纹变化更严重。
    /// 即使指纹变了，新证据若是 Conflict 应保留 Conflict 状态。
    #[test]
    fn re_verify_after_close_preserves_conflict_even_when_fingerprint_changed() {
        let dir = tempdir().unwrap();
        let old_evidence = verified_account(); // Verified, fp=fp1

        // 假 reader 返回 Conflict 证据（fp=fp2，与 old 不同）
        let mut conflict_evidence = verified_account();
        conflict_evidence.evidence_state = EvidenceState::Conflict;
        // P1 修复：Conflict 状态下 user_id 为 None
        conflict_evidence.user_id = None;
        conflict_evidence.auth_fingerprint = Some(AuthFingerprint(
            "0000000000000000000000000000000000000000000000000000000000000000".to_string(),
        ));
        let reader = FakeAccountReader {
            evidence: conflict_evidence,
        };
        let probe = FakeDbProbe {
            state: CompatibilityState::Verified {
                schema_fingerprint: SchemaFingerprint("fp".to_string()),
                counts: TableCounts::default(),
            },
        };
        let svc = WorkbenchReadService::new(&probe, &reader, "rawkey");
        let result = svc.re_verify_after_close(dir.path(), &old_evidence, now());
        // P39：必须保留 Conflict，不能用 FingerprintChanged 覆盖
        assert_eq!(
            result.evidence_state,
            EvidenceState::Conflict,
            "Conflict 状态不应被 FingerprintChanged 覆盖"
        );
    }

    /// R1 路径封闭：绝对路径被拒绝
    #[test]
    fn build_read_state_rejects_absolute_db_path() {
        let dir = tempdir().unwrap();
        touch_db(dir.path(), "database.db");
        let probe = FakeDbProbe {
            state: CompatibilityState::Verified {
                schema_fingerprint: SchemaFingerprint("fp".to_string()),
                counts: TableCounts::default(),
            },
        };
        let reader = FakeAccountReader {
            evidence: verified_account(),
        };
        let svc = WorkbenchReadService::new(&probe, &reader, "rawkey");
        // 绝对路径——Windows 上 C:\... 是绝对路径
        let abs_path = if cfg!(windows) {
            "C:\\Windows\\System32\\drivers\\etc\\hosts"
        } else {
            "/etc/passwd"
        };
        let state = svc.build_read_state(dir.path(), abs_path, now());
        assert_eq!(
            state.readonly_reason,
            Some(ReadonlyReason::DataLocationUnavailable)
        );
    }

    /// R1 路径封闭：父目录逃逸被拒绝
    #[test]
    fn build_read_state_rejects_parent_dir_escape() {
        let dir = tempdir().unwrap();
        touch_db(dir.path(), "database.db");
        let probe = FakeDbProbe {
            state: CompatibilityState::Verified {
                schema_fingerprint: SchemaFingerprint("fp".to_string()),
                counts: TableCounts::default(),
            },
        };
        let reader = FakeAccountReader {
            evidence: verified_account(),
        };
        let svc = WorkbenchReadService::new(&probe, &reader, "rawkey");
        // 父目录逃逸
        let state = svc.build_read_state(dir.path(), "../escape.db", now());
        assert_eq!(
            state.readonly_reason,
            Some(ReadonlyReason::DataLocationUnavailable)
        );
    }

    /// R1 路径封闭：文件不存在时不创建
    #[test]
    fn build_read_state_rejects_missing_db_file() {
        let dir = tempdir().unwrap();
        // 不创建 database.db
        let probe = FakeDbProbe {
            state: CompatibilityState::Verified {
                schema_fingerprint: SchemaFingerprint("fp".to_string()),
                counts: TableCounts::default(),
            },
        };
        let reader = FakeAccountReader {
            evidence: verified_account(),
        };
        let svc = WorkbenchReadService::new(&probe, &reader, "rawkey");
        let state = svc.build_read_state(dir.path(), "nonexistent.db", now());
        assert_eq!(
            state.readonly_reason,
            Some(ReadonlyReason::DataLocationUnavailable)
        );
        // 验证文件未被创建
        assert!(!dir.path().join("nonexistent.db").exists());
    }

    /// R1 路径封闭：设备命名空间前缀被拒绝
    #[test]
    fn build_read_state_rejects_device_prefix() {
        let dir = tempdir().unwrap();
        touch_db(dir.path(), "database.db");
        let probe = FakeDbProbe {
            state: CompatibilityState::Verified {
                schema_fingerprint: SchemaFingerprint("fp".to_string()),
                counts: TableCounts::default(),
            },
        };
        let reader = FakeAccountReader {
            evidence: verified_account(),
        };
        let svc = WorkbenchReadService::new(&probe, &reader, "rawkey");
        let state = svc.build_read_state(dir.path(), r"\\?\C:\secret.db", now());
        assert_eq!(
            state.readonly_reason,
            Some(ReadonlyReason::DataLocationUnavailable)
        );
    }

    /// R1 路径封闭：空字符串被拒绝
    #[test]
    fn build_read_state_rejects_empty_db_relative_path() {
        let dir = tempdir().unwrap();
        touch_db(dir.path(), "database.db");
        let probe = FakeDbProbe {
            state: CompatibilityState::Verified {
                schema_fingerprint: SchemaFingerprint("fp".to_string()),
                counts: TableCounts::default(),
            },
        };
        let reader = FakeAccountReader {
            evidence: verified_account(),
        };
        let svc = WorkbenchReadService::new(&probe, &reader, "rawkey");
        let state = svc.build_read_state(dir.path(), "", now());
        assert_eq!(
            state.readonly_reason,
            Some(ReadonlyReason::DataLocationUnavailable)
        );
    }
}
