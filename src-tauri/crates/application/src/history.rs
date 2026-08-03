//! T03/T04 历史库应用服务：扫描、浏览与项目来源分配。
//!
//! 本模块承载 P1 历史基础所需的三个用例服务：
//! - `ScanHistoryService`：编排数据库探测、账号证据读取、快照捕获与目录库投影
//! - `BrowseHistoryService`：浏览账号/项目/会话/消息与搜索
//! - `AssignProjectSourceService`：用户来源分配（仅改分类，不动观察/快照）
//!
//! 安全约束：
//! - raw_key 在构造时注入，不进入方法参数、日志或返回值
//! - 所有方法只操作 fixture 路径，不访问真实 TRAE 数据
//! - Gate E：assign 不修改 project_observation 或快照
//!
//! 依赖方向：application -> domain + ports，不依赖 infrastructure/commands/tauri。

use std::path::Path;
use std::time::SystemTime;

use traesync_domain::{
    build_sync_plan, BrowseProjectNode, BrowseResult, BrowseSessionNode, BuildSyncPlanInput,
    CompatibilityState, ConversationPreview, HistoryBrowseSummary, PlanProjectInput,
    PlanSessionInput, ProcessRunningState, ProjectSourceAssignment, ScanFailureReason, ScanOutcome,
    ScanRequest, SearchHit, SessionIdentity, SyncPlan, SyncPlanContext, SyncScope,
};
use traesync_ports::{
    AccountEvidenceReaderPort, CatalogRepository, DatabaseProbePort, SnapshotStore,
    SourceNormalizer,
};

/// 扫描历史应用服务：编排数据库探测、账号证据读取、快照捕获与目录库投影。
///
/// 对应规格 5.1 节首次扫描流程：
/// 1. 探测 fixture 内数据库兼容性
/// 2. 读取账号证据，获取 user_id 引用（不携带正文）
/// 3. 构造 ScanRequest 并捕获不可变快照
/// 4. 将快照投影到目录库（事务化）
///
/// raw_key 在构造时注入，不通过 `scan` 方法参数暴露给上层，
/// 确保 key 不进入 commands 层或 UI。
pub struct ScanHistoryService<'a> {
    snapshot_store: &'a dyn SnapshotStore,
    catalog: &'a dyn CatalogRepository,
    db_probe: &'a dyn DatabaseProbePort,
    account_reader: &'a dyn AccountEvidenceReaderPort,
    /// 来源 normalizer：catalog.project_snapshot 读取快照 DB 并规范化时需要
    normalizer: &'a dyn SourceNormalizer,
    /// SQLCipher raw key hex，由组合根从环境变量读取并注入。
    /// 不进入日志、错误消息或返回值。
    raw_key: &'a str,
}

impl<'a> ScanHistoryService<'a> {
    /// 构造扫描服务。raw_key 与 normalizer 在此注入，不通过方法参数暴露。
    pub fn new(
        snapshot_store: &'a dyn SnapshotStore,
        catalog: &'a dyn CatalogRepository,
        db_probe: &'a dyn DatabaseProbePort,
        account_reader: &'a dyn AccountEvidenceReaderPort,
        normalizer: &'a dyn SourceNormalizer,
        raw_key: &'a str,
    ) -> Self {
        Self {
            snapshot_store,
            catalog,
            db_probe,
            account_reader,
            normalizer,
            raw_key,
        }
    }

    /// 执行首次扫描：探测兼容性 -> 读账号证据 -> 捕获快照 -> 投影目录库。
    ///
    /// - `fixture_root`：fixture 根目录（路径封闭由 commands/组合根保证）
    /// - `db_relative_path`：fixture_root 内的数据库相对路径
    /// - `process_state`：TRAE 进程状态（fixture 模式由调用方注入）
    /// - `now`：扫描时间（注入便于测试）
    /// - `storage_root`：存储根目录（快照发布到此目录下）
    ///
    /// 返回 `ScanOutcome`：Success / Deduplicated / Failed。
    /// raw_key 不出现在返回值中。
    pub fn scan(
        &self,
        fixture_root: &Path,
        db_relative_path: &str,
        process_state: ProcessRunningState,
        now: SystemTime,
        storage_root: &Path,
    ) -> ScanOutcome {
        // 1. 探测数据库兼容性（raw_key 仅传入 infrastructure trait）
        let db_path = fixture_root.join(db_relative_path);
        let compatibility = self.db_probe.probe_database(&db_path, self.raw_key);

        // 2. schema 不兼容直接失败——不读账号证据、不捕获快照
        let schema_fingerprint = match compatibility {
            CompatibilityState::Verified {
                ref schema_fingerprint,
                ..
            } => schema_fingerprint.0.clone(),
            CompatibilityState::Incompatible { .. } => {
                return ScanOutcome::Failed {
                    reason: ScanFailureReason::SchemaIncompatible,
                };
            }
        };

        // 3. 读取账号证据，获取 user_id 作为 account_evidence_ref（不携带正文）
        let account = self.account_reader.read_account_evidence(fixture_root, now);
        let account_evidence_ref = account.user_id.as_ref().map(|uid| uid.as_str().to_string());

        // 4. 构造 ScanRequest
        // product_version 优先使用账号证据中的，缺失时用 "unknown"
        let product_version = account
            .product_version
            .clone()
            .unwrap_or_else(|| "unknown".to_string());
        let request = ScanRequest {
            canonical_fixture_root: fixture_root.to_string_lossy().into_owned(),
            db_relative_path: db_relative_path.to_string(),
            process_state,
            now,
            schema_fingerprint,
            mapping_version: "work_cn_v1".to_string(),
            product_version,
            account_evidence_ref,
            storage_root: storage_root.to_string_lossy().into_owned(),
        };

        // 5. 捕获不可变快照
        let mut outcome = self.snapshot_store.capture_snapshot(&request);

        // 6. 成功时投影到目录库（事务化）
        if let ScanOutcome::Success {
            snapshot_meta,
            catalog_updated,
            ..
        } = &mut outcome
        {
            // 构造快照目录路径：storage_root/snapshots/<snapshot_id>/
            // 与 FilesystemSnapshotStore 的发布路径保持一致
            let snapshot_dir = storage_root
                .join("snapshots")
                .join(snapshot_meta.snapshot_id.as_str());
            let result =
                self.catalog
                    .project_snapshot(snapshot_meta, &snapshot_dir, self.normalizer);
            // project_snapshot 成功则标记 catalog_updated；失败保留 false，调用方可据此判断
            *catalog_updated = result.is_ok();
        }

        outcome
    }
}

/// 浏览历史应用服务：账号/项目/会话/消息浏览与搜索。
///
/// 纯委托到 CatalogRepository——application 层不缓存、不过滤，
/// 软删除排除由 catalog 实现保证（Gate J）。
pub struct BrowseHistoryService<'a> {
    catalog: &'a dyn CatalogRepository,
}

impl<'a> BrowseHistoryService<'a> {
    pub fn new(catalog: &'a dyn CatalogRepository) -> Self {
        Self { catalog }
    }

    /// 浏览全部历史：账号树 + 全部项目 + 全部会话（排除软删除）。
    pub fn browse(&self) -> BrowseResult {
        self.catalog.browse()
    }

    /// 浏览指定账号的项目列表。
    pub fn browse_projects_by_account(&self, user_id: &str) -> Vec<BrowseProjectNode> {
        self.catalog.browse_projects_by_account(user_id)
    }

    /// 浏览指定项目的会话列表（排除软删除）。
    pub fn browse_sessions_by_project(&self, project_id: &str) -> Vec<BrowseSessionNode> {
        self.catalog.browse_sessions_by_project(project_id)
    }

    /// 读取完整对话预览（排除软删除消息，但保留底层行用于诊断）。
    pub fn read_conversation_preview(
        &self,
        session: &SessionIdentity,
    ) -> Option<ConversationPreview> {
        self.catalog.read_conversation_preview(session)
    }

    /// 搜索消息内容（FTS，排除软删除）。
    pub fn search_messages(&self, query: &str) -> Vec<SearchHit> {
        self.catalog.search_messages(query)
    }

    /// 历史浏览摘要（仅可见项）。
    pub fn history_summary(&self) -> HistoryBrowseSummary {
        self.catalog.history_summary()
    }
}

/// 分配项目来源应用服务：用户来源分配（Gate E）。
///
/// 仅改变目录库 display_owner 分类，不修改 project_observation、快照或活动库。
/// `user_assigned_owner = None` 表示清除用户分配，回退到 first_observed_owner。
pub struct AssignProjectSourceService<'a> {
    catalog: &'a dyn CatalogRepository,
}

impl<'a> AssignProjectSourceService<'a> {
    pub fn new(catalog: &'a dyn CatalogRepository) -> Self {
        Self { catalog }
    }

    /// 分配项目来源：构造 ProjectSourceAssignment 并调用 catalog.assign_project_source。
    ///
    /// Gate E：不修改 project_observation 或快照（由 catalog 层保证）。
    /// 返回 catalog.assign_project_source 的结果（true 表示分配成功）。
    pub fn assign(
        &self,
        project_id: &str,
        user_assigned_owner: Option<&str>,
        now: SystemTime,
    ) -> bool {
        let assignment = ProjectSourceAssignment {
            project_id: project_id.to_string(),
            user_assigned_owner: user_assigned_owner.map(|s| s.to_string()),
            assigned_at: now,
        };
        self.catalog.assign_project_source(&assignment)
    }
}

/// 同步计划服务：只读取目录库并生成不可变计划，不访问或修改活动数据库。
pub struct BuildSyncPlanService<'a> {
    catalog: &'a dyn CatalogRepository,
}

impl<'a> BuildSyncPlanService<'a> {
    pub fn new(catalog: &'a dyn CatalogRepository) -> Self {
        Self { catalog }
    }

    /// 将目录库中的观察、显示归属和活动会话转换为纯领域 Planner 输入。
    pub fn build(&self, context: SyncPlanContext, scope: SyncScope) -> SyncPlan {
        let browse = self.catalog.browse();
        let display_owners: std::collections::HashMap<String, String> = browse
            .projects
            .iter()
            .map(|project| (project.project_id.clone(), project.display_owner.clone()))
            .collect();
        let mut sessions_by_project: std::collections::HashMap<String, Vec<PlanSessionInput>> =
            std::collections::HashMap::new();
        for session in browse.sessions {
            let version_available = self
                .catalog
                .read_session_projection(&session.session_identity)
                .is_some();
            sessions_by_project
                .entry(session.project_id)
                .or_default()
                .push(PlanSessionInput {
                    identity: session.session_identity,
                    version_available,
                });
        }

        let projects = self
            .catalog
            .read_all_project_observations()
            .into_iter()
            .map(|observation| {
                let project_id = observation.project_identity.project_id.clone();
                let sessions = sessions_by_project.remove(&project_id).unwrap_or_default();
                PlanProjectInput {
                    identity: observation.project_identity,
                    display_owner: display_owners
                        .get(&project_id)
                        .cloned()
                        .unwrap_or_else(|| observation.first_observed_owner.clone()),
                    current_live_owner: observation.current_live_owner,
                    archived_only: sessions.is_empty(),
                    sessions,
                }
            })
            .collect();

        build_sync_plan(BuildSyncPlanInput {
            created_at: context.created_at,
            platform_id: context.platform_id,
            data_location_id: context.data_location_id,
            current_user_id: context.current_user_id,
            account_evidence_fingerprint: context.account_evidence_fingerprint,
            target_file_evidence: context.target_file_evidence,
            schema_fingerprint: context.schema_fingerprint,
            mapping_version: context.mapping_version,
            schema_compatible: context.schema_compatible,
            scope,
            projects,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};
    use std::sync::Mutex;
    use traesync_domain::{
        AccountEvidence, AuthFingerprint, BrowseAccountNode, CompatibilityState, ContentGraphHash,
        DiagnosticIntegrityAssertion, EvidenceState, HistoryBrowseSummary, IncompatibleReason,
        MessageProjection, ProjectIdentity, ProjectObservation, ProjectSourceAssignment,
        SchemaFingerprint, SessionProjection, SessionVersion, SnapshotFileEntry, SnapshotFileKind,
        SnapshotFingerprint, SnapshotId, SourceEventSummary, SourceSnapshotMeta, TableCounts,
        UserId,
    };
    use traesync_ports::{
        AccountEvidenceReaderPort, CatalogRepository, DatabaseProbePort, SnapshotStore,
        SourceNormalizer,
    };

    // ============== 测试辅助 ==============

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
            observed_at: SystemTime::now(),
            evidence_state: EvidenceState::Verified,
        }
    }

    fn missing_account() -> AccountEvidence {
        AccountEvidence::default()
    }

    /// 构造一个 Success outcome（catalog_updated 初始为 false）
    fn success_outcome() -> ScanOutcome {
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
                captured_at: SystemTime::UNIX_EPOCH,
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

    // ============== Fake 实现 ==============

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
        fn read_account_evidence(&self, _fixture_root: &Path, _now: SystemTime) -> AccountEvidence {
            self.evidence.clone()
        }
        fn re_read_after_close(&self, _fixture_root: &Path, _now: SystemTime) -> AccountEvidence {
            self.evidence.clone()
        }
    }

    /// 假快照存储：可注入任意 ScanOutcome
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

    /// 假目录库：记录 assign_project_source 调用，可注入浏览/搜索结果
    struct FakeCatalog {
        project_calls: Mutex<Vec<ProjectSourceAssignment>>,
        project_result: bool,
        browse_result: BrowseResult,
        search_result: Vec<SearchHit>,
        conversation_result: Option<ConversationPreview>,
        project_snapshot_result: Result<(), ScanFailureReason>,
        project_observations: Vec<ProjectObservation>,
        session_projections: Vec<SessionProjection>,
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
                conversation_result: None,
                project_snapshot_result: Ok(()),
                project_observations: vec![],
                session_projections: vec![],
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
            self.project_snapshot_result.clone()
        }
        fn browse(&self) -> BrowseResult {
            self.browse_result.clone()
        }
        fn browse_projects_by_account(&self, _user_id: &str) -> Vec<BrowseProjectNode> {
            self.browse_result.projects.clone()
        }
        fn browse_sessions_by_project(&self, _project_id: &str) -> Vec<BrowseSessionNode> {
            self.browse_result.sessions.clone()
        }
        fn read_conversation_preview(
            &self,
            _session: &SessionIdentity,
        ) -> Option<ConversationPreview> {
            self.conversation_result.clone()
        }
        fn search_messages(&self, _query: &str) -> Vec<SearchHit> {
            self.search_result.clone()
        }
        fn read_project_observation(&self, _project_id: &str) -> Option<ProjectObservation> {
            None
        }
        fn read_all_project_observations(&self) -> Vec<ProjectObservation> {
            self.project_observations.clone()
        }
        fn read_all_session_versions(&self) -> Vec<SessionVersion> {
            vec![]
        }
        fn read_session_projection(&self, _session: &SessionIdentity) -> Option<SessionProjection> {
            self.session_projections
                .iter()
                .find(|projection| projection.session_identity == *_session)
                .cloned()
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

    /// 假 SourceNormalizer：空实现，仅供测试
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

    // ============== ScanHistoryService 测试 ==============

    #[test]
    fn scan_success_path() {
        // Verified + Success + project_snapshot Ok -> catalog_updated = true
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
            outcome: success_outcome(),
        };
        let catalog = FakeCatalog::default();
        let normalizer = FakeNormalizer;
        let svc = ScanHistoryService::new(&store, &catalog, &probe, &reader, &normalizer, "rawkey");
        let outcome = svc.scan(
            Path::new("/tmp/fixture"),
            "database.db",
            ProcessRunningState::NotRunning,
            SystemTime::UNIX_EPOCH,
            Path::new("/tmp/storage"),
        );
        match outcome {
            ScanOutcome::Success {
                catalog_updated, ..
            } => {
                assert!(
                    catalog_updated,
                    "project_snapshot 成功后 catalog_updated 应为 true"
                );
            }
            _ => panic!("期望 Success，实际: {:?}", outcome),
        }
    }

    #[test]
    fn scan_schema_incompatible_returns_failed() {
        // Incompatible -> 直接返回 Failed { SchemaIncompatible }，不读账号证据、不捕获快照
        let probe = FakeDbProbe {
            state: CompatibilityState::Incompatible {
                reason: IncompatibleReason::WrongKey,
            },
        };
        let reader = FakeAccountReader {
            evidence: verified_account(),
        };
        let store = FakeSnapshotStore {
            outcome: success_outcome(),
        };
        let catalog = FakeCatalog::default();
        let normalizer = FakeNormalizer;
        let svc = ScanHistoryService::new(&store, &catalog, &probe, &reader, &normalizer, "rawkey");
        let outcome = svc.scan(
            Path::new("/tmp/fixture"),
            "database.db",
            ProcessRunningState::NotRunning,
            SystemTime::UNIX_EPOCH,
            Path::new("/tmp/storage"),
        );
        assert_eq!(
            outcome,
            ScanOutcome::Failed {
                reason: ScanFailureReason::SchemaIncompatible,
            }
        );
    }

    #[test]
    fn scan_with_missing_account_evidence_still_succeeds() {
        // 账号证据缺失（user_id = None）时仍可扫描，account_evidence_ref = None
        let probe = FakeDbProbe {
            state: CompatibilityState::Verified {
                schema_fingerprint: SchemaFingerprint("fp".to_string()),
                counts: TableCounts::default(),
            },
        };
        let reader = FakeAccountReader {
            evidence: missing_account(),
        };
        let store = FakeSnapshotStore {
            outcome: success_outcome(),
        };
        let catalog = FakeCatalog::default();
        let normalizer = FakeNormalizer;
        let svc = ScanHistoryService::new(&store, &catalog, &probe, &reader, &normalizer, "rawkey");
        let outcome = svc.scan(
            Path::new("/tmp/fixture"),
            "database.db",
            ProcessRunningState::NotRunning,
            SystemTime::UNIX_EPOCH,
            Path::new("/tmp/storage"),
        );
        match outcome {
            ScanOutcome::Success { .. } => {}
            _ => panic!("期望 Success，实际: {:?}", outcome),
        }
    }

    // ============== BrowseHistoryService 测试 ==============

    #[test]
    fn browse_returns_catalog_result() {
        // browse 委托到 catalog.browse，返回一致结果
        let mut catalog = FakeCatalog::default();
        catalog.browse_result = BrowseResult {
            accounts: vec![BrowseAccountNode {
                user_id: "u1".to_string(),
                display_label: "User 1".to_string(),
                project_count: 2,
                session_count: 5,
            }],
            projects: vec![],
            sessions: vec![],
            summary: HistoryBrowseSummary::default(),
        };
        let svc = BrowseHistoryService::new(&catalog);
        let result = svc.browse();
        assert_eq!(result.accounts.len(), 1);
        assert_eq!(result.accounts[0].user_id, "u1");
    }

    // ============== AssignProjectSourceService 测试 ==============

    #[test]
    fn assign_calls_catalog_assign_project_source() {
        // assign 构造 ProjectSourceAssignment 并调用 catalog.assign_project_source
        let mut catalog = FakeCatalog::default();
        catalog.project_result = true;
        let svc = AssignProjectSourceService::new(&catalog);
        let result = svc.assign("p1", Some("u1"), SystemTime::UNIX_EPOCH);
        assert!(result);
        let calls = catalog.project_calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].project_id, "p1");
        assert_eq!(calls[0].user_assigned_owner, Some("u1".to_string()));
    }

    #[test]
    fn build_sync_plan_service_uses_catalog_observations_and_visible_sessions() {
        let mut catalog = FakeCatalog::default();
        let session_identity = SessionIdentity::new("work_cn", "session-1");
        catalog.browse_result = BrowseResult {
            accounts: vec![],
            projects: vec![BrowseProjectNode {
                project_id: "source-project".to_string(),
                display_name: "来源项目".to_string(),
                display_owner: "source-user".to_string(),
                session_count: 1,
            }],
            sessions: vec![BrowseSessionNode {
                session_identity: session_identity.clone(),
                title: "完整对话".to_string(),
                message_count: 2,
                last_captured_at: SystemTime::UNIX_EPOCH,
                project_id: "source-project".to_string(),
            }],
            summary: HistoryBrowseSummary::default(),
        };
        catalog.project_observations = vec![ProjectObservation {
            project_identity: ProjectIdentity {
                project_id: "source-project".to_string(),
                biz_project_id: "biz-source".to_string(),
                display_name: "来源项目".to_string(),
                soft_deleted: false,
            },
            first_observed_owner: "source-user".to_string(),
            first_observed_at: SystemTime::UNIX_EPOCH,
            current_live_owner: "source-user".to_string(),
            owner_observations: vec![],
        }];
        catalog.session_projections = vec![SessionProjection {
            session_identity,
            active_content_graph_hash: ContentGraphHash("hash".to_string()),
            active_title: "完整对话".to_string(),
            soft_deleted: false,
            project_id: "source-project".to_string(),
        }];
        let service = BuildSyncPlanService::new(&catalog);
        let plan = service.build(
            traesync_domain::SyncPlanContext {
                created_at: SystemTime::UNIX_EPOCH,
                platform_id: "work_cn".to_string(),
                data_location_id: "fixture-location".to_string(),
                current_user_id: "target-user".to_string(),
                account_evidence_fingerprint: "account-fingerprint".to_string(),
                target_file_evidence: traesync_domain::TargetFileEvidence {
                    db_fingerprint: "db-fingerprint".to_string(),
                    wal_fingerprint: None,
                    shm_fingerprint: None,
                },
                schema_fingerprint: "schema-fingerprint".to_string(),
                mapping_version: "work_cn_v1".to_string(),
                schema_compatible: true,
            },
            traesync_domain::SyncScope::AllHistory,
        );

        assert!(matches!(
            plan.actions(),
            [traesync_domain::PlanAction::FollowProject { project_id, .. }]
                if project_id == "source-project"
        ));
    }
}
