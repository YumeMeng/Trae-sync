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
use std::time::SystemTime;
use traesync_application::{WorkbenchReadService, WorkspaceStateService};
use traesync_domain::{WorkbenchReadState, WorkspaceState};
// trait 通过 application 重导出，避免 commands 直接依赖 ports crate
use traesync_application::WorkspaceStateProvider;
// T03/T04 历史命令所需的 application 服务与 domain 值对象
use traesync_application::{AssignProjectSourceService, BrowseHistoryService, ScanHistoryService};
use traesync_domain::{
    BrowseResult, ConversationPreview, ProcessRunningState, ScanOutcome, SearchHit, SessionIdentity,
};

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

// ============================================================================
// T03/T04 历史命令：扫描、浏览、搜索、读取对话、分配来源
// ============================================================================

/// 历史命令输入校验错误：不携带任何 secret/认证正文。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HistoryCommandError {
    /// fixture_root 为空
    EmptyFixtureRoot,
    /// 数据库相对路径为空
    EmptyDbRelativePath,
    /// 存储根为空
    EmptyStorageRoot,
    /// 搜索查询为空
    EmptyQuery,
    /// 项目 ID 为空
    EmptyProjectId,
}

impl std::fmt::Display for HistoryCommandError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EmptyFixtureRoot => write!(f, "fixture_root 不能为空"),
            Self::EmptyDbRelativePath => write!(f, "数据库相对路径不能为空"),
            Self::EmptyStorageRoot => write!(f, "存储根不能为空"),
            Self::EmptyQuery => write!(f, "搜索查询不能为空"),
            Self::EmptyProjectId => write!(f, "项目 ID 不能为空"),
        }
    }
}

impl std::error::Error for HistoryCommandError {}

/// `scan_history` 命令：调用应用服务执行首次扫描。
///
/// 输入校验：fixture_root、db_relative_path、storage_root 非空。
/// raw_key 在服务构造时已注入，不通过此函数暴露。
///
/// `now` 显式传入，便于测试注入固定时间，避免依赖不稳定 wall clock
/// （与 `build_work_cn_state` 一致）。
pub fn scan_history(
    fixture_root: &Path,
    db_relative_path: &str,
    process_state: ProcessRunningState,
    storage_root: &Path,
    now: SystemTime,
    service: &ScanHistoryService,
) -> Result<ScanOutcome, HistoryCommandError> {
    if fixture_root.as_os_str().is_empty() {
        return Err(HistoryCommandError::EmptyFixtureRoot);
    }
    if db_relative_path.is_empty() {
        return Err(HistoryCommandError::EmptyDbRelativePath);
    }
    if storage_root.as_os_str().is_empty() {
        return Err(HistoryCommandError::EmptyStorageRoot);
    }
    Ok(service.scan(
        fixture_root,
        db_relative_path,
        process_state,
        now,
        storage_root,
    ))
}

/// `browse_history` 命令：浏览全部历史。
pub fn browse_history(service: &BrowseHistoryService) -> Result<BrowseResult, HistoryCommandError> {
    Ok(service.browse())
}

/// `search_history` 命令：搜索消息内容。
///
/// 输入校验：query 非空。
pub fn search_history(
    query: &str,
    service: &BrowseHistoryService,
) -> Result<Vec<SearchHit>, HistoryCommandError> {
    if query.is_empty() {
        return Err(HistoryCommandError::EmptyQuery);
    }
    Ok(service.search_messages(query))
}

/// `read_conversation` 命令：读取完整对话预览。
pub fn read_conversation(
    session: &SessionIdentity,
    service: &BrowseHistoryService,
) -> Result<Option<ConversationPreview>, HistoryCommandError> {
    Ok(service.read_conversation_preview(session))
}

/// `assign_source` 命令：分配项目来源（Gate E）。
///
/// 输入校验：project_id 非空。
/// `now` 显式传入，便于测试注入固定时间。
pub fn assign_source(
    project_id: &str,
    user_assigned_owner: Option<&str>,
    now: SystemTime,
    service: &AssignProjectSourceService,
) -> Result<bool, HistoryCommandError> {
    if project_id.is_empty() {
        return Err(HistoryCommandError::EmptyProjectId);
    }
    Ok(service.assign(project_id, user_assigned_owner, now))
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

    // ========================================================================
    // T03/T04 历史命令测试
    // ========================================================================

    use std::sync::Mutex;
    // 端口 trait 通过 application 重导出获得
    use traesync_application::{CatalogRepository, SnapshotStore, SourceNormalizer};
    // 域类型从 domain 导入
    use traesync_domain::{
        ContentGraphHash, ConversationPreview, DiagnosticIntegrityAssertion, HistoryBrowseSummary,
        MessageProjection, ProjectIdentity, ProjectObservation, ProjectSourceAssignment,
        ScanFailureReason, ScanRequest, SessionProjection, SessionVersion, SnapshotFileEntry,
        SnapshotFileKind, SnapshotFingerprint, SnapshotId, SourceSnapshotMeta,
    };

    /// 假快照存储：返回固定 ScanOutcome
    struct FakeSnapshotStore {
        outcome: ScanOutcome,
    }

    impl SnapshotStore for FakeSnapshotStore {
        fn capture_snapshot(&self, _request: &ScanRequest) -> ScanOutcome {
            self.outcome.clone()
        }
        fn find_by_fingerprint(&self, _fingerprint: &SnapshotFingerprint) -> Option<SnapshotId> {
            None
        }
        fn read_snapshot_meta(&self, _snapshot_id: &SnapshotId) -> Option<SourceSnapshotMeta> {
            None
        }
        fn snapshot_dir(&self, _snapshot_id: &SnapshotId) -> Option<PathBuf> {
            None
        }
    }

    /// 假目录库：记录 assign_project_source 调用
    struct FakeCatalog {
        project_calls: Mutex<Vec<ProjectSourceAssignment>>,
        project_result: bool,
        browse_result: BrowseResult,
        search_result: Vec<SearchHit>,
    }

    impl Default for FakeCatalog {
        fn default() -> Self {
            Self {
                project_calls: Mutex::new(Vec::new()),
                project_result: false,
                browse_result: BrowseResult {
                    accounts: vec![],
                    projects: vec![],
                    sessions: vec![],
                    summary: HistoryBrowseSummary::default(),
                },
                search_result: vec![],
            }
        }
    }

    impl CatalogRepository for FakeCatalog {
        fn ensure_initialized(&self) -> bool {
            true
        }
        fn project_snapshot(
            &self,
            _snapshot_meta: &SourceSnapshotMeta,
            _snapshot_dir: &Path,
            _normalizer: &dyn SourceNormalizer,
        ) -> Result<(), ScanFailureReason> {
            Ok(())
        }
        fn browse(&self) -> BrowseResult {
            self.browse_result.clone()
        }
        fn browse_projects_by_account(
            &self,
            _user_id: &str,
        ) -> Vec<traesync_domain::BrowseProjectNode> {
            self.browse_result.projects.clone()
        }
        fn browse_sessions_by_project(
            &self,
            _project_id: &str,
        ) -> Vec<traesync_domain::BrowseSessionNode> {
            self.browse_result.sessions.clone()
        }
        fn read_conversation_preview(
            &self,
            _session: &SessionIdentity,
        ) -> Option<ConversationPreview> {
            None
        }
        fn search_messages(&self, _query: &str) -> Vec<SearchHit> {
            self.search_result.clone()
        }
        fn read_project_observation(&self, _project_id: &str) -> Option<ProjectObservation> {
            None
        }
        fn read_all_project_observations(&self) -> Vec<ProjectObservation> {
            vec![]
        }
        fn read_all_session_versions(&self) -> Vec<SessionVersion> {
            vec![]
        }
        fn read_session_projection(&self, _session: &SessionIdentity) -> Option<SessionProjection> {
            None
        }
        fn assign_project_source(&self, assignment: &ProjectSourceAssignment) -> bool {
            self.project_calls.lock().unwrap().push(assignment.clone());
            self.project_result
        }
        fn read_project_source_assignment(
            &self,
            _project_id: &str,
        ) -> Option<ProjectSourceAssignment> {
            None
        }
        fn diagnostic_integrity(&self) -> DiagnosticIntegrityAssertion {
            DiagnosticIntegrityAssertion {
                visible_projects: 0,
                retained_projects: 0,
                visible_sessions: 0,
                retained_sessions: 0,
                visible_messages: 0,
                retained_messages: 0,
            }
        }
        fn history_summary(&self) -> HistoryBrowseSummary {
            self.browse_result.summary.clone()
        }
    }

    /// 假 SourceNormalizer：空实现
    struct FakeNormalizer;

    impl SourceNormalizer for FakeNormalizer {
        fn read_projects(&self, _snapshot_dir: &Path) -> Vec<ProjectIdentity> {
            vec![]
        }
        fn read_session_projections(&self, _snapshot_dir: &Path) -> Vec<SessionProjection> {
            vec![]
        }
        fn read_messages(&self, _snapshot_dir: &Path) -> Vec<MessageProjection> {
            vec![]
        }
        fn read_project_owner(&self, _snapshot_dir: &Path, _project_id: &str) -> Option<String> {
            None
        }
        fn compute_content_graph_hash(
            &self,
            _snapshot_dir: &Path,
            _session: &SessionIdentity,
        ) -> Option<ContentGraphHash> {
            None
        }
    }

    /// 构造 Success outcome 供 scan_history 测试使用
    fn scan_success_outcome() -> ScanOutcome {
        let snapshot_id = SnapshotId::new();
        ScanOutcome::Success {
            snapshot_id: snapshot_id.clone(),
            snapshot_meta: SourceSnapshotMeta {
                snapshot_id,
                platform_id: "work_cn".to_string(),
                data_location_id: "loc-1".to_string(),
                product_version: "1.107.1".to_string(),
                schema_fingerprint: "fp".to_string(),
                mapping_version: "work_cn_v1".to_string(),
                account_evidence_ref: None,
                captured_at: std::time::SystemTime::UNIX_EPOCH,
                files: vec![SnapshotFileEntry {
                    kind: SnapshotFileKind::Db,
                    relative_path: "database.db".to_string(),
                    present: true,
                    size: 1024,
                    sha256: "deadbeef".to_string(),
                    file_identity: None,
                }],
                fingerprint: SnapshotFingerprint("abc".to_string()),
            },
            catalog_updated: false,
        }
    }

    #[test]
    fn scan_history_normal_path() {
        // scan_history 正常路径：Verified + Success -> 返回 Ok(Success)
        let probe = FakeDbProbe {
            state: CompatibilityState::Verified {
                schema_fingerprint: SchemaFingerprint("fp".to_string()),
                counts: TableCounts::default(),
            },
        };
        let reader = FakeAccountReader {
            evidence: verified_account(),
        };
        let store = FakeSnapshotStore {
            outcome: scan_success_outcome(),
        };
        let catalog = FakeCatalog::default();
        let normalizer = FakeNormalizer;
        let svc = ScanHistoryService::new(&store, &catalog, &probe, &reader, &normalizer, "rawkey");
        let result = scan_history(
            Path::new("/tmp/fixture"),
            "database.db",
            ProcessRunningState::NotRunning,
            Path::new("/tmp/storage"),
            std::time::SystemTime::UNIX_EPOCH,
            &svc,
        );
        assert!(result.is_ok());
        match result.unwrap() {
            ScanOutcome::Success {
                catalog_updated, ..
            } => assert!(catalog_updated),
            other => panic!("期望 Success，实际: {:?}", other),
        }
    }

    #[test]
    fn scan_history_rejects_empty_inputs() {
        // 空 fixture_root / db_relative_path / storage_root 被拒绝
        let probe = FakeDbProbe {
            state: CompatibilityState::Verified {
                schema_fingerprint: SchemaFingerprint("fp".to_string()),
                counts: TableCounts::default(),
            },
        };
        let reader = FakeAccountReader {
            evidence: verified_account(),
        };
        let store = FakeSnapshotStore {
            outcome: scan_success_outcome(),
        };
        let catalog = FakeCatalog::default();
        let normalizer = FakeNormalizer;
        let svc = ScanHistoryService::new(&store, &catalog, &probe, &reader, &normalizer, "rawkey");
        // 空 fixture_root
        assert_eq!(
            scan_history(
                Path::new(""),
                "database.db",
                ProcessRunningState::NotRunning,
                Path::new("/tmp/storage"),
                std::time::SystemTime::UNIX_EPOCH,
                &svc,
            ),
            Err(HistoryCommandError::EmptyFixtureRoot)
        );
        // 空 db_relative_path
        assert_eq!(
            scan_history(
                Path::new("/tmp/fixture"),
                "",
                ProcessRunningState::NotRunning,
                Path::new("/tmp/storage"),
                std::time::SystemTime::UNIX_EPOCH,
                &svc,
            ),
            Err(HistoryCommandError::EmptyDbRelativePath)
        );
        // 空 storage_root
        assert_eq!(
            scan_history(
                Path::new("/tmp/fixture"),
                "database.db",
                ProcessRunningState::NotRunning,
                Path::new(""),
                std::time::SystemTime::UNIX_EPOCH,
                &svc,
            ),
            Err(HistoryCommandError::EmptyStorageRoot)
        );
    }

    #[test]
    fn browse_history_returns_result() {
        // browse_history 委托到 service.browse，返回一致结果
        let mut catalog = FakeCatalog::default();
        catalog.browse_result = BrowseResult {
            accounts: vec![traesync_domain::BrowseAccountNode {
                user_id: "u1".to_string(),
                display_label: "User 1".to_string(),
                project_count: 1,
                session_count: 2,
            }],
            projects: vec![],
            sessions: vec![],
            summary: HistoryBrowseSummary::default(),
        };
        let svc = BrowseHistoryService::new(&catalog);
        let result = browse_history(&svc);
        assert!(result.is_ok());
        let browse = result.unwrap();
        assert_eq!(browse.accounts.len(), 1);
        assert_eq!(browse.accounts[0].user_id, "u1");
    }

    #[test]
    fn search_history_rejects_empty_query() {
        // 空查询被拒绝
        let catalog = FakeCatalog::default();
        let svc = BrowseHistoryService::new(&catalog);
        assert_eq!(
            search_history("", &svc),
            Err(HistoryCommandError::EmptyQuery)
        );
    }

    #[test]
    fn assign_source_normal_path() {
        // assign_source 正常路径：构造 assignment 并调用 catalog
        let mut catalog = FakeCatalog::default();
        catalog.project_result = true;
        let svc = AssignProjectSourceService::new(&catalog);
        let result = assign_source("p1", Some("u1"), std::time::SystemTime::UNIX_EPOCH, &svc);
        assert_eq!(result, Ok(true));
        let calls = catalog.project_calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].project_id, "p1");
    }
}
