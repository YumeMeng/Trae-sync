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

use std::path::{Component, Path, PathBuf};
use std::time::SystemTime;

use traesync_domain::{
    build_sync_plan, BrowseProjectNode, BrowseResult, BrowseSessionNode, BuildSyncPlanInput,
    CompatibilityState, ConversationPreview, EvidenceState, HistoryBrowseSummary, PlanProjectInput,
    PlanSessionInput, ProcessRunningState, ProjectSourceAssignment, ScanFailureReason, ScanOutcome,
    ScanRequest, SearchHit, SessionIdentity, SnapshotFileKind, SourceSnapshotMeta, SyncPlan,
    SyncPlanContext, SyncScope,
};
use traesync_ports::{
    AccountEvidenceReaderPort, CatalogMutationOutcome, CatalogReadError, CatalogRepository,
    DatabaseProbePort, SnapshotStore, SourceNormalizer,
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
    /// 库存扫描从源库项目归属发现账号，不依赖当前登录证据。
    require_account_evidence: bool,
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
            require_account_evidence: true,
        }
    }

    /// 构造本机库存扫描服务。当前账号证据不会参与扫描门禁，账号归属由
    /// `project.user_id` 经 normalizer 写入目录库。
    pub fn new_inventory(
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
            require_account_evidence: false,
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
        self.scan_with_validation(
            fixture_root,
            db_relative_path,
            process_state,
            now,
            storage_root,
            || true,
        )
    }

    /// 执行扫描，并在发布快照和投影目录库前复核授权上下文。
    pub fn scan_with_validation<F>(
        &self,
        fixture_root: &Path,
        db_relative_path: &str,
        process_state: ProcessRunningState,
        now: SystemTime,
        storage_root: &Path,
        is_authorized: F,
    ) -> ScanOutcome
    where
        F: Fn() -> bool,
    {
        self.scan_with_context_validation(
            fixture_root,
            db_relative_path,
            process_state,
            now,
            storage_root,
            is_authorized,
            || true,
        )
    }

    /// 执行扫描，并把廉价撤销检查与完整身份检查分开。
    ///
    /// `is_authorized` 会在大文件分块复制和哈希热循环中频繁调用，必须保持廉价。
    /// `validate_context` 只在数据库探测、快照发布和目录库事务边界调用，负责完整
    /// 进程、位置和账号绑定复核。
    pub fn scan_with_context_validation<F, V>(
        &self,
        fixture_root: &Path,
        db_relative_path: &str,
        process_state: ProcessRunningState,
        now: SystemTime,
        storage_root: &Path,
        is_authorized: F,
        validate_context: V,
    ) -> ScanOutcome
    where
        F: Fn() -> bool,
        V: Fn() -> bool,
    {
        // 进入源数据库探测前再次复核绑定，避免动态授权或进程状态失效后仍读取源文件。
        if !is_authorized() || !validate_context() {
            return ScanOutcome::Failed {
                reason: ScanFailureReason::NotAuthorized,
            };
        }

        // 1. 探测数据库兼容性（raw_key 仅传入 infrastructure trait）
        let db_path = fixture_root.join(db_relative_path);
        let Some(compatibility) =
            self.db_probe
                .probe_database_with_validation(&db_path, self.raw_key, &is_authorized)
        else {
            return ScanOutcome::Failed {
                reason: ScanFailureReason::NotAuthorized,
            };
        };

        if !validate_context() {
            return ScanOutcome::Failed {
                reason: ScanFailureReason::NotAuthorized,
            };
        }

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

        // 3. 实时扫描需要当前账号证据；库存扫描直接从项目 owner 发现全部账号。
        let (account_evidence_ref, product_version) = if self.require_account_evidence {
            let account = self.account_reader.read_account_evidence(fixture_root, now);
            if account.evidence_state != EvidenceState::Verified
                || account.user_id.is_none()
                || account.auth_fingerprint.is_none()
            {
                return ScanOutcome::Failed {
                    reason: ScanFailureReason::AccountEvidenceUnavailable,
                };
            }
            if !is_authorized() {
                return ScanOutcome::Failed {
                    reason: ScanFailureReason::NotAuthorized,
                };
            }
            (
                account
                    .auth_fingerprint
                    .as_ref()
                    .map(|fingerprint| format!("auth-{}", fingerprint.0)),
                account
                    .product_version
                    .clone()
                    .unwrap_or_else(|| "unknown".to_string()),
            )
        } else {
            (None, "TRAE Work CN".to_string())
        };

        // 4. 构造 ScanRequest。
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

        // 5. 捕获不可变快照；发布前再次确认授权仍有效。
        if !is_authorized() {
            return ScanOutcome::Failed {
                reason: ScanFailureReason::NotAuthorized,
            };
        }

        if !validate_context() {
            return ScanOutcome::Failed {
                reason: ScanFailureReason::NotAuthorized,
            };
        }
        let mut outcome = self
            .snapshot_store
            .capture_snapshot_with_context_validation(&request, &is_authorized, &validate_context);

        // 6. 成功时投影到目录库（事务化）
        if let ScanOutcome::Success {
            snapshot_meta,
            catalog_updated,
            ..
        } = &mut outcome
        {
            // 根据快照元数据锁定实际 DB 父目录。生产 Work CN 的 database.db 位于嵌套目录，
            // 不能把快照发布根误当作 DB 所在目录。
            let snapshot_root = storage_root
                .join("snapshots")
                .join(snapshot_meta.snapshot_id.as_str());
            let Some(snapshot_db_dir) =
                snapshot_database_dir(&snapshot_root, snapshot_meta, Path::new(db_relative_path))
            else {
                return ScanOutcome::Failed {
                    reason: ScanFailureReason::CatalogTransactionFailed,
                };
            };
            match self.catalog.project_snapshot_with_context_validation(
                snapshot_meta,
                &snapshot_db_dir,
                self.normalizer,
                &is_authorized,
                &validate_context,
            ) {
                Ok(()) => *catalog_updated = true,
                Err(reason) => return ScanOutcome::Failed { reason },
            }
        }

        if let ScanOutcome::Deduplicated {
            existing_snapshot_id,
            snapshot_meta,
            ..
        } = &outcome
        {
            let snapshot_root = storage_root
                .join("snapshots")
                .join(existing_snapshot_id.as_str());
            let Some(snapshot_db_dir) =
                snapshot_database_dir(&snapshot_root, snapshot_meta, Path::new(db_relative_path))
            else {
                return ScanOutcome::Failed {
                    reason: ScanFailureReason::CatalogTransactionFailed,
                };
            };
            if let Err(reason) = self.catalog.project_snapshot_with_context_validation(
                snapshot_meta,
                &snapshot_db_dir,
                self.normalizer,
                &is_authorized,
                &validate_context,
            ) {
                return ScanOutcome::Failed { reason };
            }
        }

        outcome
    }
}

/// 从快照元数据中解析唯一 DB 条目的父目录，并确认其仍绑定本次授权路径。
fn snapshot_database_dir(
    snapshot_root: &Path,
    snapshot_meta: &SourceSnapshotMeta,
    authorized_db_relative_path: &Path,
) -> Option<PathBuf> {
    let mut db_entries = snapshot_meta
        .files
        .iter()
        .filter(|entry| entry.kind == SnapshotFileKind::Db && entry.present);
    let db_entry = db_entries.next()?;
    if db_entries.next().is_some() {
        return None;
    }

    let relative_path = Path::new(&db_entry.relative_path);
    if relative_path.as_os_str().is_empty()
        || relative_path.is_absolute()
        || !relative_path
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
        || relative_path != authorized_db_relative_path
    {
        return None;
    }

    Some(snapshot_root.join(relative_path.parent().unwrap_or_else(|| Path::new(""))))
}

/// 浏览历史应用服务：账号/项目/会话/消息浏览与搜索。
///
/// 纯委托到 CatalogRepository——application 层不缓存、不过滤，
/// 软删除排除由 catalog 实现保证（Gate J）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BrowseHistoryError {
    /// 目录库无法打开或读取。
    CatalogUnavailable,
}

impl std::fmt::Display for BrowseHistoryError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::CatalogUnavailable => formatter.write_str("目录库不可读"),
        }
    }
}

impl std::error::Error for BrowseHistoryError {}

pub struct BrowseHistoryService<'a> {
    catalog: &'a dyn CatalogRepository,
}

impl<'a> BrowseHistoryService<'a> {
    pub fn new(catalog: &'a dyn CatalogRepository) -> Self {
        Self { catalog }
    }

    /// 浏览全部历史：账号树 + 全部项目 + 全部会话（排除软删除）。
    pub fn browse(&self) -> Result<BrowseResult, BrowseHistoryError> {
        self.catalog.browse_checked().map_err(|error| match error {
            CatalogReadError::Unavailable => BrowseHistoryError::CatalogUnavailable,
        })
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
    ) -> Result<Option<ConversationPreview>, BrowseHistoryError> {
        self.catalog
            .read_conversation_preview_checked(session)
            .map_err(|error| match error {
                CatalogReadError::Unavailable => BrowseHistoryError::CatalogUnavailable,
            })
    }

    /// 搜索消息内容（FTS，排除软删除）；可选地限定到一个项目。
    pub fn search_messages(
        &self,
        query: &str,
        project_id: Option<&str>,
    ) -> Result<Vec<SearchHit>, BrowseHistoryError> {
        match project_id {
            Some(project_id) => self
                .catalog
                .search_messages_in_project_checked(query, project_id),
            None => self.catalog.search_messages_checked(query),
        }
        .map_err(|error| match error {
            CatalogReadError::Unavailable => BrowseHistoryError::CatalogUnavailable,
        })
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
    /// 返回目录库结构化写入结果，保留“已提交但元数据待恢复”状态。
    pub fn assign(
        &self,
        project_id: &str,
        user_assigned_owner: Option<&str>,
        now: SystemTime,
    ) -> CatalogMutationOutcome {
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
    pub fn build(
        &self,
        context: SyncPlanContext,
        scope: SyncScope,
    ) -> Result<SyncPlan, BrowseHistoryError> {
        // 目录库读取失败必须向上层传播，避免把损坏目录库误判为空计划。
        let browse = self.catalog.browse_checked().map_err(|error| match error {
            CatalogReadError::Unavailable => BrowseHistoryError::CatalogUnavailable,
        })?;
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
                .read_session_projection_checked(&session.session_identity)
                .map_err(|error| match error {
                    CatalogReadError::Unavailable => BrowseHistoryError::CatalogUnavailable,
                })?
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
            .read_all_project_observations_checked()
            .map_err(|error| match error {
                CatalogReadError::Unavailable => BrowseHistoryError::CatalogUnavailable,
            })?
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

        Ok(build_sync_plan(BuildSyncPlanInput {
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
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
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
        success_outcome_with_db_relative_path("database.db")
    }

    /// 构造指定数据库相对路径的 Success outcome，用于验证生产嵌套快照布局。
    fn success_outcome_with_db_relative_path(db_relative_path: &str) -> ScanOutcome {
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
                    relative_path: db_relative_path.to_string(),
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

    struct TrackingDbProbe {
        called: Arc<AtomicBool>,
        delegate: FakeDbProbe,
    }

    impl DatabaseProbePort for TrackingDbProbe {
        fn probe_database(&self, db_path: &Path, raw_key: &str) -> CompatibilityState {
            self.called.store(true, Ordering::SeqCst);
            self.delegate.probe_database(db_path, raw_key)
        }
        fn backup_to_logical_copy(&self, source_db: &Path, raw_key: &str) -> Option<PathBuf> {
            self.delegate.backup_to_logical_copy(source_db, raw_key)
        }
        fn verify_transaction_rollback(&self, copy_db: &Path, raw_key: &str) -> bool {
            self.delegate.verify_transaction_rollback(copy_db, raw_key)
        }
        fn run_integrity_checks(&self, db_path: &Path, raw_key: &str) -> (bool, bool) {
            self.delegate.run_integrity_checks(db_path, raw_key)
        }
        fn create_random_key_catalog(&self, fixture_root: &Path) -> Option<PathBuf> {
            self.delegate.create_random_key_catalog(fixture_root)
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

    struct TrackingAccountReader {
        called: Arc<AtomicBool>,
        delegate: FakeAccountReader,
    }

    impl AccountEvidenceReaderPort for TrackingAccountReader {
        fn read_account_evidence(&self, fixture_root: &Path, now: SystemTime) -> AccountEvidence {
            self.called.store(true, Ordering::SeqCst);
            self.delegate.read_account_evidence(fixture_root, now)
        }
        fn re_read_after_close(&self, fixture_root: &Path, now: SystemTime) -> AccountEvidence {
            self.delegate.re_read_after_close(fixture_root, now)
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
        project_snapshot_dirs: Mutex<Vec<PathBuf>>,
        project_result: CatalogMutationOutcome,
        browse_result: BrowseResult,
        search_result: Vec<SearchHit>,
        conversation_result: Option<ConversationPreview>,
        project_snapshot_result: Result<(), ScanFailureReason>,
        project_observations: Vec<ProjectObservation>,
        session_projections: Vec<SessionProjection>,
        read_error: Option<CatalogReadError>,
    }

    impl Default for FakeCatalog {
        fn default() -> Self {
            Self {
                project_calls: Mutex::new(Vec::new()),
                project_snapshot_dirs: Mutex::new(Vec::new()),
                project_result: CatalogMutationOutcome::NotCommitted,
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
                read_error: None,
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
            snapshot_dir: &Path,
            _normalizer: &dyn SourceNormalizer,
        ) -> Result<(), ScanFailureReason> {
            self.project_snapshot_dirs
                .lock()
                .unwrap()
                .push(snapshot_dir.to_path_buf());
            self.project_snapshot_result.clone()
        }
        fn browse(&self) -> BrowseResult {
            self.browse_result.clone()
        }
        fn browse_checked(&self) -> Result<BrowseResult, CatalogReadError> {
            match self.read_error {
                Some(error) => Err(error),
                None => Ok(self.browse_result.clone()),
            }
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
        fn read_conversation_preview_checked(
            &self,
            _session: &SessionIdentity,
        ) -> Result<Option<ConversationPreview>, CatalogReadError> {
            match self.read_error {
                Some(error) => Err(error),
                None => Ok(self.conversation_result.clone()),
            }
        }
        fn search_messages(&self, _query: &str) -> Vec<SearchHit> {
            self.search_result.clone()
        }
        fn search_messages_checked(
            &self,
            _query: &str,
        ) -> Result<Vec<SearchHit>, CatalogReadError> {
            match self.read_error {
                Some(error) => Err(error),
                None => Ok(self.search_result.clone()),
            }
        }
        fn search_messages_in_project_checked(
            &self,
            _query: &str,
            _project_id: &str,
        ) -> Result<Vec<SearchHit>, CatalogReadError> {
            match self.read_error {
                Some(error) => Err(error),
                None => Ok(self.search_result.clone()),
            }
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
        fn assign_project_source(
            &self,
            assignment: &ProjectSourceAssignment,
        ) -> CatalogMutationOutcome {
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
    fn scan_projects_from_nested_snapshot_database_parent() {
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
            outcome: success_outcome_with_db_relative_path("ModularData/ai-agent/database.db"),
        };
        let catalog = FakeCatalog::default();
        let normalizer = FakeNormalizer;
        let svc = ScanHistoryService::new(&store, &catalog, &probe, &reader, &normalizer, "rawkey");

        let outcome = svc.scan(
            Path::new("/tmp/fixture"),
            "ModularData/ai-agent/database.db",
            ProcessRunningState::NotRunning,
            SystemTime::UNIX_EPOCH,
            Path::new("/tmp/storage"),
        );

        assert!(matches!(outcome, ScanOutcome::Success { .. }));
        let dirs = catalog.project_snapshot_dirs.lock().unwrap();
        assert_eq!(dirs.len(), 1);
        assert_eq!(
            dirs[0],
            Path::new("/tmp/storage")
                .join("snapshots")
                .join(match &outcome {
                    ScanOutcome::Success { snapshot_id, .. } => snapshot_id.as_str(),
                    _ => unreachable!(),
                })
                .join("ModularData")
                .join("ai-agent")
        );
    }

    #[test]
    fn scan_projection_failure_returns_failed_outcome() {
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
        let catalog = FakeCatalog {
            project_snapshot_result: Err(ScanFailureReason::CatalogTransactionFailed),
            ..FakeCatalog::default()
        };
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
                reason: ScanFailureReason::CatalogTransactionFailed,
            }
        );
    }

    #[test]
    fn scan_does_not_report_failure_after_catalog_projection() {
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
        let checks = Arc::new(AtomicUsize::new(0));
        let auth_checks = checks.clone();

        let outcome = svc.scan_with_validation(
            Path::new("/tmp/fixture"),
            "database.db",
            ProcessRunningState::NotRunning,
            SystemTime::UNIX_EPOCH,
            Path::new("/tmp/storage"),
            // 目录库提交前授权保持有效；提交后不再把已完成事务改报失败。
            move || auth_checks.fetch_add(1, Ordering::SeqCst) + 1 < 9,
        );

        assert!(matches!(
            outcome,
            ScanOutcome::Success {
                catalog_updated: true,
                ..
            }
        ));
        assert_eq!(
            catalog.project_snapshot_dirs.lock().unwrap().len(),
            1,
            "目录库投影应完成一次"
        );
        assert_eq!(checks.load(Ordering::SeqCst), 8);
    }

    #[test]
    fn scan_context_validation_runs_at_stage_boundaries_only() {
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
        let context_checks = Arc::new(AtomicUsize::new(0));
        let context_checks_for_scan = context_checks.clone();

        let outcome = svc.scan_with_context_validation(
            Path::new("/tmp/fixture"),
            "database.db",
            ProcessRunningState::NotRunning,
            SystemTime::UNIX_EPOCH,
            Path::new("/tmp/storage"),
            || true,
            move || {
                context_checks_for_scan.fetch_add(1, Ordering::SeqCst);
                true
            },
        );

        assert!(matches!(
            outcome,
            ScanOutcome::Success {
                catalog_updated: true,
                ..
            }
        ));
        assert_eq!(
            context_checks.load(Ordering::SeqCst),
            7,
            "完整上下文检查应固定在探测、快照、投影阶段边界"
        );
    }

    #[test]
    fn scan_validation_rejects_before_source_probe_when_binding_is_invalid() {
        let probe_called = Arc::new(AtomicBool::new(false));
        let reader_called = Arc::new(AtomicBool::new(false));
        let probe = TrackingDbProbe {
            called: probe_called.clone(),
            delegate: FakeDbProbe {
                state: CompatibilityState::Verified {
                    schema_fingerprint: SchemaFingerprint("fp".to_string()),
                    counts: TableCounts::default(),
                },
            },
        };
        let reader = TrackingAccountReader {
            called: reader_called.clone(),
            delegate: FakeAccountReader {
                evidence: verified_account(),
            },
        };
        let store = FakeSnapshotStore {
            outcome: success_outcome(),
        };
        let catalog = FakeCatalog::default();
        let normalizer = FakeNormalizer;
        let svc = ScanHistoryService::new(&store, &catalog, &probe, &reader, &normalizer, "rawkey");

        // 动态绑定已经失效时，源数据库探测和账号证据读取都必须尚未开始。
        let outcome = svc.scan_with_validation(
            Path::new("/tmp/fixture"),
            "database.db",
            ProcessRunningState::NotRunning,
            SystemTime::UNIX_EPOCH,
            Path::new("/tmp/storage"),
            || false,
        );

        assert_eq!(
            outcome,
            ScanOutcome::Failed {
                reason: ScanFailureReason::NotAuthorized,
            }
        );
        assert!(!probe_called.load(Ordering::SeqCst));
        assert!(!reader_called.load(Ordering::SeqCst));
    }

    #[test]
    fn deduplicated_scan_stops_before_reprojection_when_authorization_expires() {
        let probe = FakeDbProbe {
            state: CompatibilityState::Verified {
                schema_fingerprint: SchemaFingerprint("fp".to_string()),
                counts: TableCounts::default(),
            },
        };
        let reader = FakeAccountReader {
            evidence: verified_account(),
        };
        let existing_snapshot_id = SnapshotId::new();
        let store = FakeSnapshotStore {
            outcome: ScanOutcome::Deduplicated {
                existing_snapshot_id: existing_snapshot_id.clone(),
                fingerprint: SnapshotFingerprint("abc".to_string()),
                snapshot_meta: SourceSnapshotMeta {
                    snapshot_id: existing_snapshot_id,
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
            },
        };
        let catalog = FakeCatalog::default();
        let normalizer = FakeNormalizer;
        let svc = ScanHistoryService::new(&store, &catalog, &probe, &reader, &normalizer, "rawkey");
        let checks = Arc::new(AtomicUsize::new(0));
        let auth_checks = checks.clone();

        let outcome = svc.scan_with_validation(
            Path::new("/tmp/fixture"),
            "database.db",
            ProcessRunningState::NotRunning,
            SystemTime::UNIX_EPOCH,
            Path::new("/tmp/storage"),
            // 授权在重新投影开始前失效，不得调用目录库投影。
            move || auth_checks.fetch_add(1, Ordering::SeqCst) + 1 < 8,
        );

        assert_eq!(
            outcome,
            ScanOutcome::Failed {
                reason: ScanFailureReason::NotAuthorized,
            }
        );
        assert_eq!(catalog.project_snapshot_dirs.lock().unwrap().len(), 0);
        assert_eq!(checks.load(Ordering::SeqCst), 8);
    }

    #[test]
    fn scan_deduplicated_snapshot_reprojects_catalog() {
        let probe = FakeDbProbe {
            state: CompatibilityState::Verified {
                schema_fingerprint: SchemaFingerprint("fp".to_string()),
                counts: TableCounts::default(),
            },
        };
        let reader = FakeAccountReader {
            evidence: verified_account(),
        };
        let existing_snapshot_id = SnapshotId::new();
        let store = FakeSnapshotStore {
            outcome: ScanOutcome::Deduplicated {
                existing_snapshot_id: existing_snapshot_id.clone(),
                fingerprint: SnapshotFingerprint("abc".to_string()),
                snapshot_meta: SourceSnapshotMeta {
                    snapshot_id: existing_snapshot_id,
                    platform_id: "work_cn".to_string(),
                    data_location_id: "loc-1".to_string(),
                    product_version: "1.107.1".to_string(),
                    schema_fingerprint: "fp".to_string(),
                    mapping_version: "work_cn_v1".to_string(),
                    account_evidence_ref: None,
                    captured_at: SystemTime::UNIX_EPOCH,
                    files: vec![SnapshotFileEntry {
                        kind: SnapshotFileKind::Db,
                        relative_path: "ModularData/ai-agent/database.db".to_string(),
                        present: true,
                        size: 1024,
                        sha256: "deadbeef".to_string(),
                        file_identity: None,
                    }],
                    fingerprint: SnapshotFingerprint("abc".to_string()),
                },
            },
        };
        let catalog = FakeCatalog::default();
        let normalizer = FakeNormalizer;
        let svc = ScanHistoryService::new(&store, &catalog, &probe, &reader, &normalizer, "rawkey");

        let outcome = svc.scan(
            Path::new("/tmp/fixture"),
            "ModularData/ai-agent/database.db",
            ProcessRunningState::NotRunning,
            SystemTime::UNIX_EPOCH,
            Path::new("/tmp/storage"),
        );

        assert!(matches!(outcome, ScanOutcome::Deduplicated { .. }));
        assert_eq!(
            catalog.project_snapshot_dirs.lock().unwrap().len(),
            1,
            "去重快照仍应补做目录库投影"
        );
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
    fn build_sync_plan_refuses_unavailable_catalog() {
        let catalog = FakeCatalog {
            read_error: Some(CatalogReadError::Unavailable),
            ..FakeCatalog::default()
        };
        let service = BuildSyncPlanService::new(&catalog);
        let result = service.build(
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

        assert_eq!(result, Err(BrowseHistoryError::CatalogUnavailable));
    }

    #[test]
    fn scan_with_missing_account_evidence_fails_closed() {
        // 账号证据缺失时必须停止，不能发布没有账号归属的历史快照。
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
        assert_eq!(
            outcome,
            ScanOutcome::Failed {
                reason: ScanFailureReason::AccountEvidenceUnavailable,
            }
        );
    }

    #[test]
    fn inventory_scan_projects_without_current_account_evidence() {
        // 库存扫描的账号归属来自源库项目 owner，不应被当前登录证据阻塞。
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
        let service = ScanHistoryService::new_inventory(
            &store,
            &catalog,
            &probe,
            &reader,
            &normalizer,
            "rawkey",
        );
        let outcome = service.scan(
            Path::new("/tmp/fixture"),
            "database.db",
            ProcessRunningState::NotRunning,
            SystemTime::UNIX_EPOCH,
            Path::new("/tmp/storage"),
        );
        assert!(matches!(
            outcome,
            ScanOutcome::Success {
                catalog_updated: true,
                ..
            }
        ));
    }

    #[test]
    fn search_and_preview_surface_catalog_unavailable() {
        let catalog = FakeCatalog {
            read_error: Some(CatalogReadError::Unavailable),
            ..FakeCatalog::default()
        };
        let service = BrowseHistoryService::new(&catalog);
        let session = SessionIdentity::new("work_cn", "session-1");

        assert_eq!(
            service.search_messages("hello", None),
            Err(BrowseHistoryError::CatalogUnavailable)
        );
        assert_eq!(
            service.read_conversation_preview(&session),
            Err(BrowseHistoryError::CatalogUnavailable)
        );
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
        let result = svc.browse().unwrap();
        assert_eq!(result.accounts.len(), 1);
        assert_eq!(result.accounts[0].user_id, "u1");
    }

    // ============== AssignProjectSourceService 测试 ==============

    #[test]
    fn assign_calls_catalog_assign_project_source() {
        // assign 构造 ProjectSourceAssignment 并调用 catalog.assign_project_source
        let mut catalog = FakeCatalog::default();
        catalog.project_result = CatalogMutationOutcome::Committed;
        let svc = AssignProjectSourceService::new(&catalog);
        let result = svc.assign("p1", Some("u1"), SystemTime::UNIX_EPOCH);
        assert_eq!(result, CatalogMutationOutcome::Committed);
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
        let plan = service
            .build(
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
            )
            .unwrap();

        assert!(matches!(
            plan.actions(),
            [traesync_domain::PlanAction::FollowProject { project_id, .. }]
                if project_id == "source-project"
        ));
    }
}
