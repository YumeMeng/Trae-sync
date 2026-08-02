//! Trae Sync Tauri 命令层：只做输入校验、调用应用服务和事件桥接。
//!
//! 对应规格第 30 节“前后端契约”：Tauri command 只返回结构化 DTO，
//! 不传递数据库连接、原始认证内容或任意 SQL。
//!
//! T01 骨架阶段只实现 `get_workspace_state` 命令。
//! T02 新增 `build_work_cn_state`：返回 Work CN 只读工作台状态。
//!
//! 依赖方向：commands 只依赖 application + domain，不直接依赖 ports/infrastructure。
//! `WorkspaceStateProvider` trait 通过 application 重导出获得。

use std::path::Path;
use traesync_application::{WorkbenchReadService, WorkspaceStateService};
use traesync_domain::{WorkbenchReadState, WorkspaceState};
// trait 通过 application 重导出，避免 commands 直接依赖 ports crate
use traesync_application::WorkspaceStateProvider;

/// `get_workspace_state` 命令：返回空工作台状态。
///
/// 前端通过 `@tauri-apps/api` 的 `invoke("get_workspace_state")` 调用。
/// T01 阶段返回固定的空状态——所有真实能力禁用，UI 显示诚实状态。
pub fn get_workspace_state(provider: &dyn WorkspaceStateProvider) -> WorkspaceState {
    let service = WorkspaceStateService::new(provider);
    service.get_workspace_state()
}

/// `read_work_cn_state` 命令的输入校验错误：不携带任何 secret/认证正文。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkbenchReadError {
    /// fixture_root 为空
    EmptyFixtureRoot,
    /// 数据库相对路径为空
    EmptyDbRelativePath,
}

impl std::fmt::Display for WorkbenchReadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EmptyFixtureRoot => write!(f, "fixture_root 不能为空"),
            Self::EmptyDbRelativePath => write!(f, "数据库相对路径不能为空"),
        }
    }
}

impl std::error::Error for WorkbenchReadError {}

/// `read_work_cn_state` 命令：调用应用服务构造 Work CN 只读工作台状态。
///
/// 前端通过 `invoke("read_work_cn_state", { fixtureRoot, dbRelativePath })` 调用。
/// 组合根（src-tauri/src/lib.rs）在调用前已用 `FixturePathGuard` 验证 fixture_root，
/// 并把 raw_key 注入 `WorkbenchReadService`——本函数不接收 raw_key，
/// 避免 key 进入 commands 层、UI 或日志。
///
/// `now` 显式传入，便于：
/// - 设置 `observed_at`
/// - 让 application/infrastructure 比较最新 session mtime 判断 Expired
/// 测试可注入固定时间，避免依赖不稳定 wall clock。
///
/// 返回 `WorkbenchReadState` 不含 raw_key、认证正文或底层错误原文。
pub fn build_work_cn_state(
    fixture_root: &Path,
    db_relative_path: &str,
    now: std::time::SystemTime,
    service: &WorkbenchReadService,
) -> Result<WorkbenchReadState, WorkbenchReadError> {
    if fixture_root.as_os_str().is_empty() {
        return Err(WorkbenchReadError::EmptyFixtureRoot);
    }
    if db_relative_path.is_empty() {
        return Err(WorkbenchReadError::EmptyDbRelativePath);
    }
    Ok(service.build_read_state(fixture_root, db_relative_path, now))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};
    use traesync_application::WorkspaceStateProvider;
    use traesync_application::{AccountEvidenceReaderPort, DatabaseProbePort};
    use traesync_domain::{
        AccountEvidence, AuthFingerprint, CapabilityFlags, CompatibilityState, EvidenceState,
        IncompatibleReason, PlatformId, ReadonlyReason, SchemaFingerprint, SourceEventSummary,
        TableCounts, UserId, WorkspaceState,
    };

    struct FakeProvider {
        state: WorkspaceState,
    }

    impl WorkspaceStateProvider for FakeProvider {
        fn get_workspace_state(&self) -> WorkspaceState {
            self.state.clone()
        }
    }

    /// 假数据库探测：可注入任意 CompatibilityState
    struct FakeDbProbe {
        state: CompatibilityState,
    }

    impl DatabaseProbePort for FakeDbProbe {
        fn probe_database(&self, _db_path: &Path, _raw_key: &str) -> CompatibilityState {
            self.state.clone()
        }
        fn backup_to_logical_copy(&self, _source_db: &Path, _raw_key: &str) -> Option<PathBuf> {
            None
        }
        fn verify_transaction_rollback(&self, _copy_db: &Path, _raw_key: &str) -> bool {
            true
        }
        fn run_integrity_checks(&self, _db_path: &Path, _raw_key: &str) -> (bool, bool) {
            (true, true)
        }
        fn create_random_key_catalog(&self, _fixture_root: &Path) -> Option<PathBuf> {
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

    fn verified_account() -> AccountEvidence {
        // 合成账号 ID（不使用真实基线 ID，仅 fixture 测试）
        const SYNTHETIC_USER_ID: &str = "1000000000000001";
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

    #[test]
    fn command_returns_state_from_provider() {
        let provider = FakeProvider {
            state: WorkspaceState {
                platform: traesync_domain::PlatformContext {
                    platform_id: PlatformId::work_cn(),
                    display_name: "TRAE Work CN".to_string(),
                    adapter_implemented: false,
                },
                data_location: traesync_domain::DataLocationState {
                    selected: false,
                    display_name: None,
                    unavailable_reason: Some("not_selected".to_string()),
                },
                current_account: traesync_domain::CurrentAccountState {
                    detected: false,
                    user_fingerprint: None,
                    unavailable_reason: Some("not_detected".to_string()),
                },
                history: traesync_domain::HistorySummary::default(),
                capabilities: CapabilityFlags::default(),
                honest_status: "真实能力尚未启用".to_string(),
            },
        };

        let result = get_workspace_state(&provider);
        assert_eq!(result.platform.platform_id, PlatformId::work_cn());
        assert!(!result.capabilities.scan_enabled);
        assert_eq!(result.honest_status, "真实能力尚未启用");
    }

    #[test]
    fn build_work_cn_state_returns_state_from_service() {
        // 验证 commands 层正确转发 application service 的结果
        // R1：fixture_root 必须真实存在且 database.db 必须存在，否则路径封闭检查返回 DataLocationUnavailable
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("database.db"), b"fake-db-content").unwrap();
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
        let result = build_work_cn_state(
            dir.path(),
            "database.db",
            std::time::SystemTime::now(),
            &svc,
        );
        assert!(result.is_ok());
        let state = result.unwrap();
        assert_eq!(state.platform.platform_id, PlatformId::work_cn());
        assert!(state.platform.adapter_implemented);
        assert_eq!(state.readonly_reason, None);
    }

    #[test]
    fn build_work_cn_state_rejects_empty_fixture_root() {
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
        let result = build_work_cn_state(
            Path::new(""),
            "database.db",
            std::time::SystemTime::now(),
            &svc,
        );
        assert_eq!(result, Err(WorkbenchReadError::EmptyFixtureRoot));
    }

    #[test]
    fn build_work_cn_state_rejects_empty_db_relative_path() {
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
        let result = build_work_cn_state(
            Path::new("/tmp/fixture"),
            "",
            std::time::SystemTime::now(),
            &svc,
        );
        assert_eq!(result, Err(WorkbenchReadError::EmptyDbRelativePath));
    }

    #[test]
    fn build_work_cn_state_wrong_key_returns_readonly_reason() {
        // 验证错误 key 时返回结构化只读原因，不泄露 key 原文
        // R1：fixture_root 必须真实存在且 database.db 必须存在，否则路径封闭检查返回 DataLocationUnavailable
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("database.db"), b"fake-db-content").unwrap();
        let probe = FakeDbProbe {
            state: CompatibilityState::Incompatible {
                reason: IncompatibleReason::WrongKey,
            },
        };
        let reader = FakeAccountReader {
            evidence: verified_account(),
        };
        let svc = WorkbenchReadService::new(&probe, &reader, "rawkey");
        let result = build_work_cn_state(
            dir.path(),
            "database.db",
            std::time::SystemTime::now(),
            &svc,
        );
        let state = result.unwrap();
        assert_eq!(state.readonly_reason, Some(ReadonlyReason::WrongKey));
        // 返回值不包含 raw_key
        let json = serde_json::to_string(&state).unwrap();
        assert!(!json.contains("rawkey"));
    }
}
