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
use traesync_application::{
    ApplySyncPlanService, BuildSyncPlanService, WorkbenchReadService, WorkspaceStateService,
};
use traesync_domain::{
    OperationCancellation, SyncPlan, SyncPlanContext, SyncPlanExecutionOutcome, SyncScope,
    WorkbenchReadState, WorkspaceState,
};
// trait 通过 application 重导出，避免 commands 直接依赖 ports crate
use traesync_application::WorkspaceStateProvider;
// T03/T04 历史命令所需的 application 服务与 domain 值对象
use traesync_application::{AssignProjectSourceService, BrowseHistoryService, ScanHistoryService};
use traesync_domain::{
    AuthorizationState, BrowseResult, ConversationPreview, ProcessRunningState, ScanOutcome,
    SearchHit, SessionIdentity,
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
    /// R1：未授权扫描——后端未持有显式用户授权
    NotAuthorized,
    /// R1：授权不匹配——请求的 fixture_root/db_relative_path 与授权范围不一致
    AuthorizationMismatch,
    /// R1：TRAE 进程运行中——在任何 DB/账号证据访问前早拒
    ProcessRunning,
    /// R2：数据库相对路径是绝对路径
    DbRelativePathAbsolute,
    /// R2：数据库相对路径包含父目录遍历 (`..`)
    DbRelativePathParentTraversal,
}

impl std::fmt::Display for HistoryCommandError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EmptyFixtureRoot => write!(f, "fixture_root 不能为空"),
            Self::EmptyDbRelativePath => write!(f, "数据库相对路径不能为空"),
            Self::EmptyStorageRoot => write!(f, "存储根不能为空"),
            Self::EmptyQuery => write!(f, "搜索查询不能为空"),
            Self::EmptyProjectId => write!(f, "项目 ID 不能为空"),
            Self::NotAuthorized => write!(f, "未授权扫描：后端未持有显式用户授权"),
            Self::AuthorizationMismatch => write!(f, "授权不匹配：请求范围与授权范围不一致"),
            Self::ProcessRunning => write!(f, "TRAE 进程运行中：拒绝扫描"),
            Self::DbRelativePathAbsolute => write!(f, "数据库相对路径是绝对路径"),
            Self::DbRelativePathParentTraversal => {
                write!(f, "数据库相对路径包含父目录遍历 (`..`)")
            }
        }
    }
}

impl std::error::Error for HistoryCommandError {}

/// R1：检查扫描授权与进程边界——在任何 DB/账号证据/FS 访问之前执行。
///
/// 纯函数，可在不启动 Tauri 运行时的情况下测试反例：
/// - 未授权（NotAuthorized）→ 拒绝
/// - 授权范围不匹配（AuthorizationMismatch）→ 拒绝
/// - TRAE 运行中（ProcessRunning）→ 拒绝
///
/// 返回 Ok(()) 表示通过授权与进程边界检查，可进入 DB 探测阶段。
pub fn check_scan_authorization(
    fixture_root: &Path,
    db_relative_path: &str,
    process_state: ProcessRunningState,
    authorization: &AuthorizationState,
) -> Result<(), HistoryCommandError> {
    match authorization {
        AuthorizationState::NotAuthorized => Err(HistoryCommandError::NotAuthorized),
        AuthorizationState::Authorized {
            canonical_fixture_root,
            db_relative_path: authorized_db_path,
        } => {
            // 验证请求范围与授权范围一致——防止授权 A 路径后扫描 B 路径
            if fixture_root.to_string_lossy() != *canonical_fixture_root
                || db_relative_path != *authorized_db_path
            {
                return Err(HistoryCommandError::AuthorizationMismatch);
            }
            // R1：运行中早拒——在任何 DB probing/account-evidence 读之前
            if process_state == ProcessRunningState::Running {
                return Err(HistoryCommandError::ProcessRunning);
            }
            Ok(())
        }
    }
}

/// R2：词法检查 `db_relative_path` 是否安全——拒绝绝对路径与父目录遍历。
///
/// 纯函数，不依赖文件系统状态，可在任何层调用。返回 `Ok(())` 表示词法安全。
/// 完整的封闭证明（含符号链接/junction 解析）由 infrastructure 层的
/// `FixturePathGuard::validate_db_relative_path` 完成；此处只做早拒，避免
/// 不安全的路径字符串进入 application/infrastructure 调用链。
pub fn check_db_relative_path_lexical(db_relative_path: &str) -> Result<(), HistoryCommandError> {
    if db_relative_path.is_empty() {
        return Err(HistoryCommandError::EmptyDbRelativePath);
    }
    let p = Path::new(db_relative_path);
    if p.is_absolute() {
        return Err(HistoryCommandError::DbRelativePathAbsolute);
    }
    for component in p.components() {
        match component {
            std::path::Component::ParentDir => {
                return Err(HistoryCommandError::DbRelativePathParentTraversal);
            }
            std::path::Component::Prefix(_) | std::path::Component::RootDir => {
                return Err(HistoryCommandError::DbRelativePathAbsolute);
            }
            _ => {}
        }
    }
    Ok(())
}

/// `scan_history` 命令：调用应用服务执行首次扫描。
///
/// R1 修复：`authorization` 由后端持有，在此函数最先检查——
/// 未授权或运行中时在任何 DB/账号证据/FS 访问之前返回错误。
/// 不接受调用方控制的 process_state 作为权威——仅作为早拒信号。
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
    authorization: &AuthorizationState,
    service: &ScanHistoryService,
) -> Result<ScanOutcome, HistoryCommandError> {
    if fixture_root.as_os_str().is_empty() {
        return Err(HistoryCommandError::EmptyFixtureRoot);
    }
    if storage_root.as_os_str().is_empty() {
        return Err(HistoryCommandError::EmptyStorageRoot);
    }
    // R2：词法检查 db_relative_path——在任何 DB 探测/FS 访问之前拒绝绝对路径与 `..`
    check_db_relative_path_lexical(db_relative_path)?;
    // R1：授权与进程边界检查——在任何 DB 探测之前
    check_scan_authorization(fixture_root, db_relative_path, process_state, authorization)?;
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

/// `build_sync_plan` 命令：使用后端固定的证据上下文生成只读计划预览。
pub fn build_sync_plan(
    scope: SyncScope,
    context: SyncPlanContext,
    service: &BuildSyncPlanService,
) -> Result<SyncPlan, HistoryCommandError> {
    Ok(service.build(context, scope))
}

/// `apply_sync_plan` 命令：只委托 application 服务，计划和取消令牌均由组合根持有。
///
/// 不接受前端传入的 `SyncPlan`，避免客户端伪造或反序列化注入写入计划。
pub fn apply_sync_plan(
    plan: &SyncPlan,
    cancellation: &OperationCancellation,
    service: &ApplySyncPlanService,
) -> SyncPlanExecutionOutcome {
    service.apply(plan, cancellation)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};
    use traesync_application::{
        AccountEvidenceReaderPort, ApplySyncPlanService, DatabaseProbePort, SyncPlanEvidencePort,
        SyncPlanExecutorPort, WorkspaceStateProvider,
    };
    use traesync_domain::{
        AccountEvidence, AuthFingerprint, BuildSyncPlanInput, CapabilityFlags, CompatibilityState,
        EvidenceState, IncompatibleReason, OperationCancellation, PlatformId, ReadonlyReason,
        SchemaFingerprint, SourceEventSummary, SyncPlanExecutionOutcome, TableCounts,
        TargetFileEvidence, UserId, WorkspaceState,
    };

    struct FakeProvider {
        state: WorkspaceState,
    }

    /// 过期证据确保 application 服务不会调用执行器。
    struct ExpiredPlanEvidence;

    impl SyncPlanEvidencePort for ExpiredPlanEvidence {
        fn is_current(&self, _plan: &SyncPlan) -> bool {
            false
        }
    }

    /// 不应被调用的执行器；若命令绕过 application 服务，测试会立即失败。
    struct UnreachableExecutor;

    impl SyncPlanExecutorPort for UnreachableExecutor {
        fn execute_sync_plan(
            &self,
            _plan: &SyncPlan,
            _cancellation: &OperationCancellation,
            _evidence: &dyn SyncPlanEvidencePort,
        ) -> SyncPlanExecutionOutcome {
            panic!("过期计划不得进入执行器")
        }
    }

    /// 生成最小不可变计划，测试命令层不依赖客户端 JSON 输入。
    fn plan_for_apply_command() -> SyncPlan {
        traesync_domain::build_sync_plan(BuildSyncPlanInput {
            created_at: std::time::SystemTime::UNIX_EPOCH,
            platform_id: "work_cn".to_string(),
            data_location_id: "fixture-location".to_string(),
            current_user_id: "target-user".to_string(),
            account_evidence_fingerprint: "account-fingerprint".to_string(),
            target_file_evidence: TargetFileEvidence {
                db_fingerprint: "database-fingerprint".to_string(),
                wal_fingerprint: None,
                shm_fingerprint: None,
            },
            schema_fingerprint: "schema-fingerprint".to_string(),
            mapping_version: "work_cn_v1".to_string(),
            schema_compatible: true,
            scope: SyncScope::AllHistory,
            projects: vec![],
        })
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
    fn apply_sync_plan_delegates_expired_plan_to_application_service() {
        let evidence = ExpiredPlanEvidence;
        let executor = UnreachableExecutor;
        let service = ApplySyncPlanService::new(&evidence, &executor);

        let outcome = apply_sync_plan(
            &plan_for_apply_command(),
            &OperationCancellation::new(),
            &service,
        );

        assert_eq!(
            outcome,
            SyncPlanExecutionOutcome::PlanExpired {
                backups_preserved: false
            }
        );
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
        // scan_history 正常路径：已授权 + Verified + Success -> 返回 Ok(Success)
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
        let auth = AuthorizationState::Authorized {
            canonical_fixture_root: "/tmp/fixture".to_string(),
            db_relative_path: "database.db".to_string(),
        };
        let result = scan_history(
            Path::new("/tmp/fixture"),
            "database.db",
            ProcessRunningState::NotRunning,
            Path::new("/tmp/storage"),
            std::time::SystemTime::UNIX_EPOCH,
            &auth,
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
        // 空 fixture_root / db_relative_path / storage_root 被拒绝（在授权检查之前）
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
        let auth = AuthorizationState::NotAuthorized;
        // 空 fixture_root
        assert_eq!(
            scan_history(
                Path::new(""),
                "database.db",
                ProcessRunningState::NotRunning,
                Path::new("/tmp/storage"),
                std::time::SystemTime::UNIX_EPOCH,
                &auth,
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
                &auth,
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
                &auth,
                &svc,
            ),
            Err(HistoryCommandError::EmptyStorageRoot)
        );
    }

    // ========================================================================
    // R1 反例测试：后端授权与进程边界
    // ========================================================================

    /// 记录 DB probe 是否被调用的 FakeDbProbe——用于验证授权检查在 DB 访问之前
    struct ProbeCallTracker {
        probed: Mutex<bool>,
        state: CompatibilityState,
    }

    impl DatabaseProbePort for ProbeCallTracker {
        fn probe_database(&self, _db_path: &Path, _raw_key: &str) -> CompatibilityState {
            *self.probed.lock().unwrap() = true;
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

    /// 记录 account evidence 是否被读取的 FakeAccountReader
    struct AccountReadTracker {
        read: Mutex<bool>,
    }

    impl AccountEvidenceReaderPort for AccountReadTracker {
        fn read_account_evidence(
            &self,
            _fixture_root: &Path,
            _now: std::time::SystemTime,
        ) -> traesync_domain::AccountEvidence {
            *self.read.lock().unwrap() = true;
            traesync_domain::AccountEvidence::default()
        }
        fn re_read_after_close(
            &self,
            _fixture_root: &Path,
            _now: std::time::SystemTime,
        ) -> traesync_domain::AccountEvidence {
            traesync_domain::AccountEvidence::default()
        }
    }

    #[test]
    fn r1_direct_invoke_without_authorization_rejected_before_db_access() {
        // R1 反例：直接调用 scan_history 不携带授权 -> 在 DB probe 之前被拒绝
        let probe = ProbeCallTracker {
            probed: Mutex::new(false),
            state: CompatibilityState::Verified {
                schema_fingerprint: SchemaFingerprint("fp".to_string()),
                counts: TableCounts::default(),
            },
        };
        let reader = AccountReadTracker {
            read: Mutex::new(false),
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
            &AuthorizationState::NotAuthorized,
            &svc,
        );
        assert_eq!(result, Err(HistoryCommandError::NotAuthorized));
        // DB probe 不应被调用
        assert!(!*probe.probed.lock().unwrap(), "未授权时不应访问数据库");
        // 账号证据不应被读取
        assert!(!*reader.read.lock().unwrap(), "未授权时不应读取账号证据");
    }

    #[test]
    fn r1_running_state_rejected_before_db_access() {
        // R1 反例：已授权但 TRAE 运行中 -> 在 DB probe 之前被拒绝
        let probe = ProbeCallTracker {
            probed: Mutex::new(false),
            state: CompatibilityState::Verified {
                schema_fingerprint: SchemaFingerprint("fp".to_string()),
                counts: TableCounts::default(),
            },
        };
        let reader = AccountReadTracker {
            read: Mutex::new(false),
        };
        let store = FakeSnapshotStore {
            outcome: scan_success_outcome(),
        };
        let catalog = FakeCatalog::default();
        let normalizer = FakeNormalizer;
        let svc = ScanHistoryService::new(&store, &catalog, &probe, &reader, &normalizer, "rawkey");
        let auth = AuthorizationState::Authorized {
            canonical_fixture_root: "/tmp/fixture".to_string(),
            db_relative_path: "database.db".to_string(),
        };
        let result = scan_history(
            Path::new("/tmp/fixture"),
            "database.db",
            ProcessRunningState::Running,
            Path::new("/tmp/storage"),
            std::time::SystemTime::UNIX_EPOCH,
            &auth,
            &svc,
        );
        assert_eq!(result, Err(HistoryCommandError::ProcessRunning));
        assert!(!*probe.probed.lock().unwrap(), "运行中时不应访问数据库");
        assert!(!*reader.read.lock().unwrap(), "运行中时不应读取账号证据");
    }

    #[test]
    fn r1_authorization_mismatch_rejected() {
        // R1 反例：授权 A 路径但请求 B 路径 -> AuthorizationMismatch
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
        let auth = AuthorizationState::Authorized {
            canonical_fixture_root: "/tmp/authorized-fixture".to_string(),
            db_relative_path: "database.db".to_string(),
        };
        // 请求不同 fixture_root
        let result = scan_history(
            Path::new("/tmp/different-fixture"),
            "database.db",
            ProcessRunningState::NotRunning,
            Path::new("/tmp/storage"),
            std::time::SystemTime::UNIX_EPOCH,
            &auth,
            &svc,
        );
        assert_eq!(result, Err(HistoryCommandError::AuthorizationMismatch));
    }

    #[test]
    fn r1_check_scan_authorization_pure_function() {
        // R1：纯函数测试——不依赖 service，验证授权边界逻辑
        let auth_authorized = AuthorizationState::Authorized {
            canonical_fixture_root: "/tmp/fixture".to_string(),
            db_relative_path: "database.db".to_string(),
        };
        // 未授权
        assert_eq!(
            check_scan_authorization(
                Path::new("/tmp/fixture"),
                "database.db",
                ProcessRunningState::NotRunning,
                &AuthorizationState::NotAuthorized,
            ),
            Err(HistoryCommandError::NotAuthorized)
        );
        // 授权匹配 + 未运行 -> Ok
        assert!(check_scan_authorization(
            Path::new("/tmp/fixture"),
            "database.db",
            ProcessRunningState::NotRunning,
            &auth_authorized,
        )
        .is_ok());
        // 授权匹配 + Unknown -> Ok（fixture 模式下 Unknown 可接受）
        assert!(check_scan_authorization(
            Path::new("/tmp/fixture"),
            "database.db",
            ProcessRunningState::Unknown,
            &auth_authorized,
        )
        .is_ok());
        // 授权匹配 + 运行中 -> 拒绝
        assert_eq!(
            check_scan_authorization(
                Path::new("/tmp/fixture"),
                "database.db",
                ProcessRunningState::Running,
                &auth_authorized,
            ),
            Err(HistoryCommandError::ProcessRunning)
        );
        // fixture_root 不匹配
        assert_eq!(
            check_scan_authorization(
                Path::new("/tmp/other"),
                "database.db",
                ProcessRunningState::NotRunning,
                &auth_authorized,
            ),
            Err(HistoryCommandError::AuthorizationMismatch)
        );
        // db_relative_path 不匹配
        assert_eq!(
            check_scan_authorization(
                Path::new("/tmp/fixture"),
                "other.db",
                ProcessRunningState::NotRunning,
                &auth_authorized,
            ),
            Err(HistoryCommandError::AuthorizationMismatch)
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

    // ============== R2：db_relative_path 逃逸反例测试 ==============

    #[test]
    fn r2_check_db_relative_path_lexical_rejects_empty() {
        assert_eq!(
            check_db_relative_path_lexical(""),
            Err(HistoryCommandError::EmptyDbRelativePath)
        );
    }

    #[test]
    fn r2_check_db_relative_path_lexical_rejects_absolute_windows() {
        // `C:\windows\system32\evil.db` 是绝对路径，必须被词法拒绝
        assert_eq!(
            check_db_relative_path_lexical("C:\\windows\\system32\\evil.db"),
            Err(HistoryCommandError::DbRelativePathAbsolute)
        );
    }

    #[test]
    fn r2_check_db_relative_path_lexical_rejects_absolute_unix_style() {
        // `/etc/passwd` 包含 RootDir 组件——必须被拒绝
        assert_eq!(
            check_db_relative_path_lexical("/etc/passwd"),
            Err(HistoryCommandError::DbRelativePathAbsolute)
        );
    }

    #[test]
    fn r2_check_db_relative_path_lexical_rejects_parent_traversal() {
        // `../secret.db` 必须被词法拒绝
        assert_eq!(
            check_db_relative_path_lexical("../secret.db"),
            Err(HistoryCommandError::DbRelativePathParentTraversal)
        );
    }

    #[test]
    fn r2_check_db_relative_path_lexical_rejects_nested_parent_traversal() {
        // `sub/../../escape.db` 含 `..` 组件——必须拒绝
        assert_eq!(
            check_db_relative_path_lexical("sub/../../escape.db"),
            Err(HistoryCommandError::DbRelativePathParentTraversal)
        );
    }

    #[test]
    fn r2_check_db_relative_path_lexical_accepts_safe_relative_path() {
        // 安全路径：纯相对路径，无绝对前缀，无 `..`
        assert!(check_db_relative_path_lexical("database.db").is_ok());
        assert!(check_db_relative_path_lexical("sub/database.db").is_ok());
        assert!(check_db_relative_path_lexical("a/b/c/database.db").is_ok());
    }

    /// R2 反例：scan_history 在任何 service 调用前必须拒绝绝对路径与父目录遍历。
    /// 使用 ProbeCallTracker 验证：拒绝时不应触发任何 DB probing。
    #[test]
    fn r2_scan_history_rejects_absolute_path_before_db_access() {
        let probe = ProbeCallTracker {
            probed: Mutex::new(false),
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

        let authorization = AuthorizationState::Authorized {
            canonical_fixture_root: "/tmp/fixture".to_string(),
            db_relative_path: "C:\\windows\\evil.db".to_string(),
        };
        let result = scan_history(
            Path::new("/tmp/fixture"),
            "C:\\windows\\evil.db",
            ProcessRunningState::NotRunning,
            Path::new("/tmp/storage"),
            SystemTime::UNIX_EPOCH,
            &authorization,
            &svc,
        );
        // 词法检查在授权检查之前——返回绝对路径错误
        assert_eq!(result, Err(HistoryCommandError::DbRelativePathAbsolute));
        // 不应触发任何 DB probing
        assert!(!*probe.probed.lock().unwrap());
    }

    /// R2 反例：scan_history 拒绝父目录遍历，且不触发 DB probing。
    #[test]
    fn r2_scan_history_rejects_parent_traversal_before_db_access() {
        let probe = ProbeCallTracker {
            probed: Mutex::new(false),
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

        let authorization = AuthorizationState::Authorized {
            canonical_fixture_root: "/tmp/fixture".to_string(),
            db_relative_path: "../escape.db".to_string(),
        };
        let result = scan_history(
            Path::new("/tmp/fixture"),
            "../escape.db",
            ProcessRunningState::NotRunning,
            Path::new("/tmp/storage"),
            SystemTime::UNIX_EPOCH,
            &authorization,
            &svc,
        );
        assert_eq!(
            result,
            Err(HistoryCommandError::DbRelativePathParentTraversal)
        );
        assert!(!*probe.probed.lock().unwrap());
    }
}
