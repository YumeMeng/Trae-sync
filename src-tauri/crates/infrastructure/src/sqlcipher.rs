//! 嵌入式 SQLCipher 探测实现：使用 rusqlite + bundled-sqlcipher-vendored-openssl。
//!
//! 对应 Gate A：生产 Rust 连接层能打开 Work CN 和 Trae Sync 两类 SQLCipher 数据库，
//! 不依赖外部 CLI 或 TRAE DLL。
//!
//! 参数边界：
//! - raw_key 是已授权的 TRAE 技术参数，可以由项目基线或运行时配置提供
//! - raw_key 不进入 OperationId、状态枚举或结构化错误代码，避免破坏审计协议；这不是密钥保密要求
//! - 错误 key 必须返回结构化 `WrongKey`，不抛出原始错误
//! - 截断文件返回 `TruncatedFile`
//! - 未知 schema 返回 `UnknownSchema`
//! - `sqlcipher_export()` 必须保留未 checkpoint WAL 的已提交记录

use rusqlite::{params, types::ValueRef, Connection, OpenFlags, Row, TransactionBehavior};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::io::{Read, Write};
use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::time::{Instant, SystemTime, UNIX_EPOCH};
use traesync_domain::{
    CompatibilityState, IncompatibleReason, OperationCancellation, OperationState, PlanAction,
    PlanAssertion, SyncPlan, SyncPlanExecutionOutcome, TargetFileEvidence,
};
use traesync_ports::{DatabaseProbePort, SyncPlanEvidencePort, SyncPlanExecutorPort};

use crate::catalog::{
    bump_catalog_content_revision, reconcile_current_catalog_sidecar, reject_catalog_sidecars,
    resolve_current_catalog_path, verify_catalog_read_protocol,
    verify_existing_catalog_write_protocol,
};
use crate::data_location::{capture_location_identity, LocationWitness};
use crate::file_identity::PlatformFileIdentityProvider;
use crate::fixture_paths::FixturePathGuard;
use crate::operation_lease::OperationLease;
use crate::operation_manifest::{
    has_manual_recovery_required, reconcile_unfinished_manifests_with_recovery_handlers,
    OperationManifestJournal,
};
use crate::progress::{ProgressPhase, ProgressReporter, ProgressSnapshot};
use crate::storage_root::{
    required_storage_reserve_bytes, reserve_space_on_volumes, SpaceReservationRequest,
};
use crate::work_cn_schema::{check_schema, compute_schema_fingerprint, read_table_counts};

/// 嵌入式 SQLCipher 探测器：实现 `DatabaseProbePort`。
#[derive(Clone, Copy)]
pub struct SqlCipherProbe;

/// 只读探测期间持有的隔离数据库三件套。
///
/// 探测函数会先创建它、后创建连接，保证连接先关闭，再删除临时目录。
struct ReadonlyTrioCopy {
    db_path: PathBuf,
    directory: PathBuf,
}

impl Drop for ReadonlyTrioCopy {
    fn drop(&mut self) {
        // 临时副本只服务当前探测；清理失败不应覆盖原始探测结果。
        let _ = std::fs::remove_dir_all(&self.directory);
    }
}

/// 持有隔离三件套的只读 SQLCipher 连接。
///
/// `connection` 声明在前，确保析构时先关闭连接，再清理临时目录。
/// P3-1 起供 crate 内会话索引读取复用（跨账号实例库只读汇总）；
/// W0 探针经 `open_with_key_readonly_staged` 复用同一结构（字段保持私有）。
pub struct ReadonlySqlCipherConnection {
    connection: Connection,
    _trio_copy: ReadonlyTrioCopy,
}

impl Deref for ReadonlySqlCipherConnection {
    type Target = Connection;

    fn deref(&self) -> &Self::Target {
        &self.connection
    }
}

/// Work CN 的最小同步执行器。
///
/// T06/T07 执行器支持受限项目跟随与会话重挂；公开入口强制 fixture 路径防护。
pub struct WorkCnSyncExecutor {
    /// TRAE 活动数据库与其隔离备份使用的原始密钥。
    source_raw_key: String,
    /// Trae Sync 自有目录库密钥；不得复用 source_raw_key。
    catalog_key: String,
    progress_reporter: Option<ProgressReporter>,
}

/// `FollowProject` 的结构化执行结论，不暴露原始数据库错误或密钥。
pub type FollowProjectExecution = SyncPlanExecutionOutcome;

/// 已固定 guard、目标 DB 与恢复存储根的执行器；只能由 `bind_fixture` 构造。
pub struct FixtureWorkCnSyncExecutor<'a> {
    executor: &'a WorkCnSyncExecutor,
    fixture_root: PathBuf,
    target_db_path: PathBuf,
    location_witness: LocationWitness,
    storage_root: PathBuf,
    recovery_root: PathBuf,
    /// 执行器持有租约所有权，确保目录库协调和目标写入全程处于同一锁区间。
    operation_lease: OperationLease,
}

/// 事务提交时固定的最小关系断言，供新连接提交后验证使用。
struct FollowProjectCommit {
    affected_rows: u64,
    session_count: i64,
}

/// 事务结果区分证据漂移与 SQL 失败，避免把可重建计划误报为数据库故障。
enum FollowProjectTransaction {
    Committed(FollowProjectCommit),
    EvidenceChanged,
    Failed,
}

/// 会话重挂提交后必须由新连接复核的最小关系快照。
struct AttachSessionsCommit {
    affected_rows: u64,
    source_remaining_sessions: i64,
    target_original_sessions: i64,
    source_remaining_session_projects: i64,
    target_original_session_projects: i64,
    message_count: i64,
    protected_table_fingerprints: Vec<TableFingerprint>,
}

/// 不允许改写的表内容摘要；包含消息、正文、缓存、FTS 与未选中关系。
#[derive(Debug, Clone, PartialEq, Eq)]
struct TableFingerprint {
    table_name: String,
    row_count: u64,
    content_hash: String,
}

/// 将允许更新的单元格归一化后做摘要，仍会捕捉其它列或非目标行的任何变动。
struct AttachSessionMutationScope {
    selected_session_ids: BTreeSet<String>,
    selected_artifact_ids: BTreeSet<String>,
    artifact_layout: ArtifactRelationLayout,
}

/// 仅支持已由 fixture 与隔离真实库共同确认的两种 artifact 会话关系布局。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ArtifactRelationLayout {
    LegacySourceSession,
    CreatorAndWriterSession,
}

impl ArtifactRelationLayout {
    fn artifact_session_column(self) -> &'static str {
        match self {
            Self::LegacySourceSession => "source_session_id",
            Self::CreatorAndWriterSession => "creator_session_id",
        }
    }

    fn version_requires_writer_session(self) -> bool {
        matches!(self, Self::CreatorAndWriterSession)
    }
}

/// 会话重挂事务结果与完整项目跟随保持相同的漂移语义。
enum AttachSessionsTransaction {
    Committed(AttachSessionsCommit),
    EvidenceChanged,
    Failed,
}

/// 批量动作统一使用的事务结果包装，避免执行循环中丢失动作类型。
enum SyncTransaction {
    FollowProject(FollowProjectTransaction),
    AttachSessions(AttachSessionsTransaction),
}

/// 已通过写前预检的唯一可执行动作；其它计划继续显式拒绝。
enum ExecutableAction<'a> {
    FollowProject {
        project_id: &'a str,
        source_user_id: &'a str,
        target_user_id: &'a str,
    },
    AttachSessions {
        source_project_id: &'a str,
        target_project_id: &'a str,
        session_ids: &'a [traesync_domain::SessionIdentity],
    },
}

/// 事务已经提交、但还未完成新连接验证的结果。
enum CommittedAction {
    FollowProject(FollowProjectCommit),
    AttachSessions(AttachSessionsCommit),
}

impl CommittedAction {
    fn affected_rows(&self) -> u64 {
        match self {
            Self::FollowProject(commit) => commit.affected_rows,
            Self::AttachSessions(commit) => commit.affected_rows,
        }
    }
}

impl WorkCnSyncExecutor {
    /// 在组合根注入 fixture SQLCipher key，避免 key 进入 UI、日志和返回值。
    pub fn new(raw_key: impl Into<String>) -> Self {
        let raw_key = raw_key.into();
        // 兼容旧 fixture 调用；生产组合根必须使用 new_with_keys 提供独立密钥。
        Self::new_with_keys(raw_key.clone(), raw_key)
    }

    /// 显式注入 TRAE 源库密钥与 Trae Sync 目录库密钥。
    ///
    /// 两个密钥属于不同数据边界：源库密钥只用于目标库/备份，目录库密钥只用于
    /// Trae Sync 自有目录库、目录库 sidecar 与操作记录。
    pub fn new_with_keys(
        source_raw_key: impl Into<String>,
        catalog_key: impl Into<String>,
    ) -> Self {
        Self {
            source_raw_key: source_raw_key.into(),
            catalog_key: catalog_key.into(),
            progress_reporter: None,
        }
    }

    /// 在已构造的执行器上替换目录库密钥；仅供组合根在 fixture/生产边界已确认后调用。
    pub fn with_catalog_key(mut self, catalog_key: impl Into<String>) -> Self {
        self.catalog_key = catalog_key.into();
        self
    }

    /// 绑定低频阶段进度；回调只接收非敏感字节和阶段信息。
    pub fn with_progress_reporter(mut self, reporter: ProgressReporter) -> Self {
        self.progress_reporter = Some(reporter);
        self
    }

    /// 绑定唯一生产执行入口所需的 fixture guard、目标数据库、存储根和固定恢复区。
    pub fn bind_fixture<'a>(
        &'a self,
        fixture_guard: &FixturePathGuard,
        target_db_path: &Path,
        storage_root: &Path,
        recovery_root: &Path,
        operation_lease: OperationLease,
    ) -> Result<FixtureWorkCnSyncExecutor<'a>, crate::FixturePathError> {
        // DB、存储根和固定恢复区均由 guard 验证，避免 public API 变成真实路径的写入旁路。
        let target_db_path = fixture_guard.validate_write_target(target_db_path)?;
        let storage_root = fixture_guard.validate_fixture_storage_root(storage_root)?;
        let recovery_root = fixture_guard.validate_shared_recovery_root(recovery_root)?;
        let db_relative_path = target_db_path
            .strip_prefix(fixture_guard.canonical_root())
            .map_err(|_| crate::FixturePathError::OutsideFixtureRoot {
                raw: target_db_path.to_string_lossy().into_owned(),
            })?
            .to_string_lossy()
            .into_owned();
        let location_witness = capture_location_identity(
            &PlatformFileIdentityProvider::new(),
            fixture_guard.canonical_root(),
            &db_relative_path,
        )
        .map_err(|error| crate::FixturePathError::CannotCanonicalize {
            raw: target_db_path.to_string_lossy().into_owned(),
            source: format!("位置身份见证失败: {error}"),
        })?;
        // 目录库执行器必须使用同一固定恢复区、同一已验证存储根和同一数据位置租约。
        // 只比较字符串路径不足以防御路径替换或未绑定普通租约。
        if operation_lease
            .validate_recovery_root(&recovery_root)
            .is_err()
            || operation_lease
                .validate_storage_root(&storage_root)
                .is_err()
            || operation_lease.data_location_id() != location_witness.data_location_id
        {
            return Err(crate::FixturePathError::OperationLeaseUnavailable {
                source: "租约上下文不匹配".to_string(),
            });
        }
        Ok(FixtureWorkCnSyncExecutor {
            executor: self,
            fixture_root: fixture_guard.canonical_root().to_path_buf(),
            target_db_path,
            location_witness,
            storage_root,
            recovery_root,
            operation_lease,
        })
    }

    /// 受 fixture 守卫后的执行核心；仅供本模块测试与公开入口复用。
    #[cfg(test)]
    fn execute_follow_project_inner(
        &self,
        target_db_path: &Path,
        storage_root: &Path,
        operation_id: &traesync_domain::OperationId,
        project_id: &str,
        expected_source_user_id: &str,
        target_user_id: &str,
    ) -> FollowProjectExecution {
        let before_dir = storage_root
            .join("backups")
            .join(operation_id.as_str())
            .join("before");
        let raw_dir = before_dir.join("raw");
        let logical_db_path = before_dir.join("logical").join("database.db");

        // 先固定原始 DB/WAL/SHM 字节证据；主库缺失或任一复制哈希不一致时禁止写入。
        if !capture_raw_backup(target_db_path, &raw_dir) {
            return FollowProjectExecution::FailedBeforeWrite {
                backups_preserved: false,
            };
        }

        // 再生成并用新只读连接验证逻辑副本；原始证据保留，失败不触碰目标库。
        if !backup_to_logical_copy_at_inner(target_db_path, &self.source_raw_key, &logical_db_path)
            || !verify_logical_backup(&logical_db_path, &self.source_raw_key)
        {
            return FollowProjectExecution::FailedBeforeWrite {
                backups_preserved: true,
            };
        }

        let committed = match apply_follow_project_transaction(
            target_db_path,
            &self.source_raw_key,
            project_id,
            expected_source_user_id,
            target_user_id,
        ) {
            Some(rows) => rows,
            None => {
                return FollowProjectExecution::FailedBeforeWrite {
                    backups_preserved: true,
                };
            }
        };

        // 事务提交后必须重新打开数据库验证；验证失败绝不报告成功。
        if !verify_follow_project_after_commit(
            target_db_path,
            &self.source_raw_key,
            project_id,
            target_user_id,
            committed.session_count,
        ) {
            return FollowProjectExecution::FailedAfterWrite {
                backups_preserved: true,
            };
        }

        FollowProjectExecution::Completed {
            affected_rows: committed.affected_rows,
        }
    }
}

impl SyncPlanExecutorPort for FixtureWorkCnSyncExecutor<'_> {
    fn execute_sync_plan(
        &self,
        plan: &SyncPlan,
        cancellation: &OperationCancellation,
        evidence: &dyn SyncPlanEvidencePort,
    ) -> SyncPlanExecutionOutcome {
        self.execute_sync_plan_inner(plan, cancellation, evidence)
    }
}

impl FixtureWorkCnSyncExecutor<'_> {
    fn current_location_identity(&self) -> Option<LocationWitness> {
        let relative_path = self
            .target_db_path
            .strip_prefix(&self.fixture_root)
            .ok()?
            .to_string_lossy()
            .into_owned();
        capture_location_identity(
            &PlatformFileIdentityProvider::new(),
            &self.fixture_root,
            &relative_path,
        )
        .ok()
    }

    fn location_identity_matches_bound(&self) -> bool {
        self.current_location_identity()
            .is_some_and(|current| location_identity_matches(&self.location_witness, &current))
    }

    fn report_progress(
        &self,
        plan: &SyncPlan,
        phase: ProgressPhase,
        completed_bytes: u64,
        total_bytes: Option<u64>,
        cancellable: bool,
    ) {
        if let Some(reporter) = &self.executor.progress_reporter {
            reporter(ProgressSnapshot::new(
                plan.operation_id().as_str(),
                phase,
                completed_bytes,
                total_bytes,
                cancellable,
            ));
        }
    }

    /// 按 manifest 状态机执行单个受支持动作；其它动作留给后续 ticket 实现。
    fn execute_sync_plan_inner(
        &self,
        plan: &SyncPlan,
        cancellation: &OperationCancellation,
        evidence: &dyn SyncPlanEvidencePort,
    ) -> SyncPlanExecutionOutcome {
        // 先确认计划和绑定目标属于同一位置，防止错误目标触发 manifest 或备份副作用。
        if plan.data_location_id() != self.location_witness.data_location_id
            || !self.location_identity_matches_bound()
        {
            return SyncPlanExecutionOutcome::PlanExpired {
                backups_preserved: false,
            };
        }
        let bundle_size = database_bundle_size(&self.target_db_path);
        self.report_progress(plan, ProgressPhase::Preparing, 0, None, true);

        // 旧 manifest 依赖当前目录库记录完成态；先在已持有的共享目录库租约内
        // 修复/复核当前代次 sidecar，避免已提交操作因 sidecar 落后而被误判为未完成。
        // 协调失败必须立即 fail-closed，不能在元数据不可信时继续写入目标库。
        if reconcile_current_catalog_sidecar(
            &self.storage_root,
            &self.executor.catalog_key,
            &self.recovery_root,
            &self.operation_lease,
        )
        .is_err()
        {
            return SyncPlanExecutionOutcome::ManualRecoveryRequired {
                backups_preserved: false,
            };
        }

        // 先协调旧操作；任一写入中断都进入人工恢复，当前操作不碰目标库。
        if !reconcile_unfinished_manifests_with_recovery_handlers(
            &self.recovery_root,
            |journal| {
                journal.data_location_id() == plan.data_location_id()
                    && journal
                        .verified_target_file_evidence()
                        .is_some_and(|expected| {
                            target_file_matches_evidence(&self.target_db_path, expected)
                                && catalog_operation_matches(
                                    &self.storage_root,
                                    &self.executor.catalog_key,
                                    journal.operation_id(),
                                    journal.data_location_id(),
                                    expected,
                                )
                        })
            },
            |journal| {
                if journal.data_location_id() != plan.data_location_id() {
                    return false;
                }
                preserve_failure_evidence(
                    journal,
                    &self.target_db_path,
                    &self.storage_root,
                    journal.operation_id(),
                )
            },
        ) || has_manual_recovery_required(&self.recovery_root)
        {
            return SyncPlanExecutionOutcome::ManualRecoveryRequired {
                backups_preserved: false,
            };
        }

        // 计划可以包含多个项目动作；先完成全部形状和会话 sandbox 预检，避免批量执行到一半才发现
        // 某个动作根本不受支持。每个动作仍保留独立事务和写后证据，任何中途失败都会进入人工恢复。
        let actions: Vec<ExecutableAction<'_>> = plan
            .actions()
            .iter()
            .filter_map(|action| match action {
                PlanAction::FollowProject {
                    project_id,
                    from_user_id,
                    to_user_id,
                } => Some(ExecutableAction::FollowProject {
                    project_id: project_id.as_str(),
                    source_user_id: from_user_id.as_str(),
                    target_user_id: to_user_id.as_str(),
                }),
                PlanAction::AttachSessions {
                    source_project_id,
                    target_project_id,
                    session_ids,
                } if attach_sessions_preflight(
                    &self.target_db_path,
                    &self.executor.source_raw_key,
                    &self.fixture_root,
                    source_project_id,
                    target_project_id,
                    session_ids,
                ) =>
                {
                    Some(ExecutableAction::AttachSessions {
                        source_project_id: source_project_id.as_str(),
                        target_project_id: target_project_id.as_str(),
                        session_ids,
                    })
                }
                _ => None,
            })
            .collect();
        if actions.len() != plan.actions().len() || actions.is_empty() {
            return SyncPlanExecutionOutcome::UnsupportedPlan;
        }

        // 应用层已检查一次；执行器再检查，防止调用者绕过 service 或计划在间隙失效。
        if !evidence.is_current(plan)
            || !target_file_matches_plan(&self.target_db_path, plan)
            || !self.location_identity_matches_bound()
        {
            return SyncPlanExecutionOutcome::PlanExpired {
                backups_preserved: false,
            };
        }

        let bundle_size = match bundle_size {
            Some(size) => size,
            None => {
                return SyncPlanExecutionOutcome::FailedBeforeWrite {
                    backups_preserved: false,
                }
            }
        };
        // 目标库卷还需要容纳事务/WAL 增长；存储卷需要同时容纳原始备份、逻辑副本
        // 和失败现场。不同卷分别预留，同卷预算由基础设施按卷身份合并。
        let target_parent = match self.target_db_path.parent() {
            Some(parent) => parent,
            None => {
                return SyncPlanExecutionOutcome::FailedBeforeWrite {
                    backups_preserved: false,
                }
            }
        };
        let target_volume_budget =
            bundle_size.saturating_add(required_storage_reserve_bytes(bundle_size));
        let storage_copy_budget = bundle_size.saturating_mul(3);
        let storage_volume_budget =
            storage_copy_budget.saturating_add(required_storage_reserve_bytes(storage_copy_budget));
        let reservation_requests = [
            SpaceReservationRequest::new(target_parent.to_path_buf(), target_volume_budget),
            SpaceReservationRequest::new(self.storage_root.clone(), storage_volume_budget),
        ];
        let _space_reservation = match reserve_space_on_volumes(&reservation_requests) {
            Ok(reservation) => reservation,
            Err(_) => {
                return SyncPlanExecutionOutcome::FailedBeforeWrite {
                    backups_preserved: false,
                }
            }
        };

        let journal = match OperationManifestJournal::create(
            &self.recovery_root,
            plan.operation_id(),
            plan.data_location_id(),
            plan.target_file_evidence(),
        ) {
            Some(journal) => journal,
            None => {
                return SyncPlanExecutionOutcome::FailedBeforeWrite {
                    backups_preserved: false,
                }
            }
        };
        let before_dir = self
            .storage_root
            .join("backups")
            .join(plan.operation_id().as_str())
            .join("before");
        let raw_dir = before_dir.join("raw");
        let logical_db_path = before_dir.join("logical").join("database.db");

        if journal.transition(OperationState::BackingUp).is_err() {
            let _ = journal.transition(OperationState::FailedSafe);
            return SyncPlanExecutionOutcome::FailedBeforeWrite {
                backups_preserved: false,
            };
        }
        self.report_progress(plan, ProgressPhase::Copying, 0, Some(bundle_size), true);
        let mut copied_bytes = 0_u64;
        let mut hashed_bytes = 0_u64;
        if !capture_raw_backup_with_progress(
            &self.target_db_path,
            &raw_dir,
            |delta| {
                copied_bytes = copied_bytes.saturating_add(delta);
                self.report_progress(
                    plan,
                    ProgressPhase::Copying,
                    copied_bytes,
                    Some(bundle_size),
                    true,
                );
            },
            || self.report_progress(plan, ProgressPhase::Hashing, 0, Some(bundle_size), true),
            |delta| {
                hashed_bytes = hashed_bytes.saturating_add(delta);
                self.report_progress(
                    plan,
                    ProgressPhase::Hashing,
                    hashed_bytes,
                    Some(bundle_size),
                    true,
                );
            },
        ) {
            let _ = journal.transition(OperationState::FailedSafe);
            return SyncPlanExecutionOutcome::FailedBeforeWrite {
                backups_preserved: false,
            };
        }
        if !backup_to_logical_copy_at_inner(
            &self.target_db_path,
            &self.executor.source_raw_key,
            &logical_db_path,
        ) || !verify_logical_backup(&logical_db_path, &self.executor.source_raw_key)
        {
            let _ = journal.transition(OperationState::FailedSafe);
            return SyncPlanExecutionOutcome::FailedBeforeWrite {
                backups_preserved: true,
            };
        }
        self.report_progress(plan, ProgressPhase::Verifying, 0, None, true);
        if journal.transition(OperationState::BackupVerified).is_err() {
            return SyncPlanExecutionOutcome::FailedBeforeWrite {
                backups_preserved: true,
            };
        }

        // 备份完成但尚未写目标时仍可取消，双备份按规格保留。
        if cancellation.is_requested() {
            let _ = journal.transition(OperationState::CancelledBeforeWrite);
            return SyncPlanExecutionOutcome::CancelledBeforeWrite {
                backups_preserved: true,
            };
        }
        if !evidence.is_current(plan)
            || !target_file_matches_plan(&self.target_db_path, plan)
            || !self.location_identity_matches_bound()
        {
            let _ = journal.transition(OperationState::NotApplied);
            return SyncPlanExecutionOutcome::PlanExpired {
                backups_preserved: true,
            };
        }
        if journal.transition(OperationState::TargetWriting).is_err() {
            return SyncPlanExecutionOutcome::FailedBeforeWrite {
                backups_preserved: true,
            };
        }
        // 进入目标写入后不可取消；写入阶段不虚报字节百分比，直到新连接验证完成。
        self.report_progress(plan, ProgressPhase::Writing, 0, None, false);

        let mut committed_actions = Vec::with_capacity(actions.len());
        // 每个动作提交后更新一次可信文件基线；后续动作只能从该基线继续，防止动作间外部改库。
        let mut current_file_evidence = plan.target_file_evidence().clone();
        for (index, action) in actions.iter().enumerate() {
            let first_action = index == 0;
            let action_context_current = if first_action {
                evidence.is_current(plan)
            } else {
                evidence.is_current_after_commit(plan)
            };
            if !action_context_current
                || !target_file_matches_evidence(&self.target_db_path, &current_file_evidence)
                || !self.location_identity_matches_bound()
            {
                if committed_actions.is_empty() {
                    let _ = journal.transition(OperationState::NotApplied);
                    return SyncPlanExecutionOutcome::PlanExpired {
                        backups_preserved: true,
                    };
                }
                let _ = preserve_failure_evidence(
                    &journal,
                    &self.target_db_path,
                    &self.storage_root,
                    plan.operation_id().as_str(),
                );
                let _ = journal.transition(OperationState::ManualRecoveryRequired);
                return SyncPlanExecutionOutcome::ManualRecoveryRequired {
                    backups_preserved: true,
                };
            }
            let action_file_evidence = current_file_evidence.clone();
            let transaction = match action {
                ExecutableAction::FollowProject {
                    project_id,
                    source_user_id,
                    target_user_id,
                } => {
                    SyncTransaction::FollowProject(apply_follow_project_transaction_with_evidence(
                        &self.target_db_path,
                        &self.executor.source_raw_key,
                        project_id,
                        source_user_id,
                        target_user_id,
                        || {
                            self.location_identity_matches_bound()
                                && target_database_and_wal_match_evidence(
                                    &self.target_db_path,
                                    &action_file_evidence,
                                )
                        },
                    ))
                }
                ExecutableAction::AttachSessions {
                    source_project_id,
                    target_project_id,
                    session_ids,
                } => SyncTransaction::AttachSessions(
                    apply_attach_sessions_transaction_with_evidence(
                        &self.target_db_path,
                        &self.executor.source_raw_key,
                        source_project_id,
                        target_project_id,
                        session_ids,
                        || {
                            self.location_identity_matches_bound()
                                && target_database_and_wal_match_evidence(
                                    &self.target_db_path,
                                    &action_file_evidence,
                                )
                                && target_sandbox_is_available(
                                    &self.fixture_root,
                                    target_project_id,
                                )
                        },
                        || {
                            // 事务自身 DML 会改写 WAL；提交前仍需确认 DB/WAL 属于当前动作基线。
                            self.location_identity_matches_bound()
                                && target_database_and_wal_match_evidence(
                                    &self.target_db_path,
                                    &action_file_evidence,
                                )
                                && target_sandbox_is_available(
                                    &self.fixture_root,
                                    target_project_id,
                                )
                        },
                    ),
                ),
            };
            match transaction {
                SyncTransaction::FollowProject(FollowProjectTransaction::Committed(commit)) => {
                    let committed = CommittedAction::FollowProject(commit);
                    let verified = match (&action, &committed) {
                        (
                            ExecutableAction::FollowProject {
                                project_id,
                                target_user_id,
                                ..
                            },
                            CommittedAction::FollowProject(commit),
                        ) => verify_follow_project_after_commit(
                            &self.target_db_path,
                            &self.executor.source_raw_key,
                            project_id,
                            target_user_id,
                            commit.session_count,
                        ),
                        _ => false,
                    };
                    // 多动作必须在动作间立即停止；单动作保留既有失败/漂移终态，交由末尾统一收口。
                    let action_post_commit_evidence_current = if actions.len() > 1 {
                        evidence.is_current_after_commit(plan)
                    } else {
                        true
                    };
                    if ((!verified || !action_post_commit_evidence_current) && actions.len() > 1)
                        || !self.location_identity_matches_bound()
                    {
                        let _ = preserve_failure_evidence(
                            &journal,
                            &self.target_db_path,
                            &self.storage_root,
                            plan.operation_id().as_str(),
                        );
                        let _ = journal.transition(OperationState::ManualRecoveryRequired);
                        return SyncPlanExecutionOutcome::ManualRecoveryRequired {
                            backups_preserved: true,
                        };
                    }
                    let Some(next_file_evidence) = target_file_evidence(&self.target_db_path)
                    else {
                        let _ = preserve_failure_evidence(
                            &journal,
                            &self.target_db_path,
                            &self.storage_root,
                            plan.operation_id().as_str(),
                        );
                        let _ = journal.transition(OperationState::ManualRecoveryRequired);
                        return SyncPlanExecutionOutcome::ManualRecoveryRequired {
                            backups_preserved: true,
                        };
                    };
                    current_file_evidence = next_file_evidence;
                    committed_actions.push(committed);
                }
                SyncTransaction::AttachSessions(AttachSessionsTransaction::Committed(commit)) => {
                    let committed = CommittedAction::AttachSessions(commit);
                    let verified = match (&action, &committed) {
                        (
                            ExecutableAction::AttachSessions {
                                source_project_id,
                                target_project_id,
                                session_ids,
                            },
                            CommittedAction::AttachSessions(commit),
                        ) => verify_attach_sessions_after_commit(
                            &self.target_db_path,
                            &self.executor.source_raw_key,
                            source_project_id,
                            target_project_id,
                            session_ids,
                            commit,
                        ),
                        _ => false,
                    };
                    // 多动作必须在动作间立即停止；单动作保留既有失败/漂移终态，交由末尾统一收口。
                    let action_post_commit_evidence_current = if actions.len() > 1 {
                        evidence.is_current_after_commit(plan)
                    } else {
                        true
                    };
                    if ((!verified || !action_post_commit_evidence_current) && actions.len() > 1)
                        || !self.location_identity_matches_bound()
                    {
                        let _ = preserve_failure_evidence(
                            &journal,
                            &self.target_db_path,
                            &self.storage_root,
                            plan.operation_id().as_str(),
                        );
                        let _ = journal.transition(OperationState::ManualRecoveryRequired);
                        return SyncPlanExecutionOutcome::ManualRecoveryRequired {
                            backups_preserved: true,
                        };
                    }
                    let Some(next_file_evidence) = target_file_evidence(&self.target_db_path)
                    else {
                        let _ = preserve_failure_evidence(
                            &journal,
                            &self.target_db_path,
                            &self.storage_root,
                            plan.operation_id().as_str(),
                        );
                        let _ = journal.transition(OperationState::ManualRecoveryRequired);
                        return SyncPlanExecutionOutcome::ManualRecoveryRequired {
                            backups_preserved: true,
                        };
                    };
                    current_file_evidence = next_file_evidence;
                    committed_actions.push(committed);
                }
                SyncTransaction::FollowProject(FollowProjectTransaction::EvidenceChanged)
                | SyncTransaction::AttachSessions(AttachSessionsTransaction::EvidenceChanged) => {
                    if committed_actions.is_empty() {
                        let _ = journal.transition(OperationState::NotApplied);
                        return SyncPlanExecutionOutcome::PlanExpired {
                            backups_preserved: true,
                        };
                    }
                    let _ = preserve_failure_evidence(
                        &journal,
                        &self.target_db_path,
                        &self.storage_root,
                        plan.operation_id().as_str(),
                    );
                    let _ = journal.transition(OperationState::ManualRecoveryRequired);
                    return SyncPlanExecutionOutcome::ManualRecoveryRequired {
                        backups_preserved: true,
                    };
                }
                SyncTransaction::FollowProject(FollowProjectTransaction::Failed)
                | SyncTransaction::AttachSessions(AttachSessionsTransaction::Failed) => {
                    if committed_actions.is_empty() {
                        let _ = journal.transition(OperationState::NotApplied);
                        return SyncPlanExecutionOutcome::FailedBeforeWrite {
                            backups_preserved: true,
                        };
                    }
                    let _ = preserve_failure_evidence(
                        &journal,
                        &self.target_db_path,
                        &self.storage_root,
                        plan.operation_id().as_str(),
                    );
                    let _ = journal.transition(OperationState::ManualRecoveryRequired);
                    return SyncPlanExecutionOutcome::ManualRecoveryRequired {
                        backups_preserved: true,
                    };
                }
            }
        }
        let affected_rows = committed_actions
            .iter()
            .map(CommittedAction::affected_rows)
            .sum::<u64>();

        // 提交后若目标文件已被替换，不能把新文件上的验证结果归给旧事务。
        if !self.location_identity_matches_bound() {
            let _ = preserve_failure_evidence(
                &journal,
                &self.target_db_path,
                &self.storage_root,
                plan.operation_id().as_str(),
            );
            let _ = journal.transition(OperationState::ManualRecoveryRequired);
            return SyncPlanExecutionOutcome::ManualRecoveryRequired {
                backups_preserved: true,
            };
        }

        if journal
            .transition(OperationState::TargetCommittedUnverified)
            .is_err()
            || journal.transition(OperationState::TargetVerifying).is_err()
        {
            let _ = preserve_failure_evidence(
                &journal,
                &self.target_db_path,
                &self.storage_root,
                plan.operation_id().as_str(),
            );
            return SyncPlanExecutionOutcome::ManualRecoveryRequired {
                backups_preserved: true,
            };
        }

        let verified = if actions.len() == 1 {
            match (&actions[0], &committed_actions[0]) {
                (
                    ExecutableAction::FollowProject {
                        project_id,
                        target_user_id,
                        ..
                    },
                    CommittedAction::FollowProject(commit),
                ) => verify_follow_project_after_commit(
                    &self.target_db_path,
                    &self.executor.source_raw_key,
                    project_id,
                    target_user_id,
                    commit.session_count,
                ),
                (
                    ExecutableAction::AttachSessions {
                        source_project_id,
                        target_project_id,
                        session_ids,
                    },
                    CommittedAction::AttachSessions(commit),
                ) => verify_attach_sessions_after_commit(
                    &self.target_db_path,
                    &self.executor.source_raw_key,
                    source_project_id,
                    target_project_id,
                    session_ids,
                    commit,
                ),
                _ => false,
            }
        } else {
            verify_plan_assertions_after_commit(
                &self.target_db_path,
                &self.executor.source_raw_key,
                plan.expected_after(),
            )
        };
        // 提交后漂移不能撤销已确认目标，但必须继续完整验证且不报告普通成功。
        if !self.location_identity_matches_bound() {
            let _ = preserve_failure_evidence(
                &journal,
                &self.target_db_path,
                &self.storage_root,
                plan.operation_id().as_str(),
            );
            let _ = journal.transition(OperationState::ManualRecoveryRequired);
            return SyncPlanExecutionOutcome::ManualRecoveryRequired {
                backups_preserved: true,
            };
        }
        let post_commit_drift = !evidence.is_current_after_commit(plan);
        if !verified {
            if journal
                .transition(OperationState::VerificationInconclusive)
                .is_err()
                || !preserve_failure_evidence(
                    &journal,
                    &self.target_db_path,
                    &self.storage_root,
                    plan.operation_id().as_str(),
                )
                || journal
                    .transition(OperationState::ManualRecoveryRequired)
                    .is_err()
            {
                return SyncPlanExecutionOutcome::ManualRecoveryRequired {
                    backups_preserved: true,
                };
            }
            return SyncPlanExecutionOutcome::FailedAfterWrite {
                backups_preserved: true,
            };
        }
        // 写后证据漂移说明账号、数据位置或 schema 已无法继续证明；必须保留现场并交由人工恢复。
        if post_commit_drift {
            let _ = preserve_failure_evidence(
                &journal,
                &self.target_db_path,
                &self.storage_root,
                plan.operation_id().as_str(),
            );
            let _ = journal.transition(OperationState::ManualRecoveryRequired);
            return SyncPlanExecutionOutcome::ManualRecoveryRequired {
                backups_preserved: true,
            };
        }
        // 新连接验证完成后再写入完成；若崩溃在两条状态之间，journal 无独立复核凭据，只能保留现场并人工恢复。
        let verified_target_file_evidence = match target_file_evidence(&self.target_db_path) {
            Some(evidence) => evidence,
            None => {
                let _ = preserve_failure_evidence(
                    &journal,
                    &self.target_db_path,
                    &self.storage_root,
                    plan.operation_id().as_str(),
                );
                return SyncPlanExecutionOutcome::ManualRecoveryRequired {
                    backups_preserved: true,
                };
            }
        };
        if journal
            .transition_catalog_reconciling(&verified_target_file_evidence)
            .is_err()
        {
            let _ = preserve_failure_evidence(
                &journal,
                &self.target_db_path,
                &self.storage_root,
                plan.operation_id().as_str(),
            );
            return SyncPlanExecutionOutcome::ManualRecoveryRequired {
                backups_preserved: true,
            };
        }
        if !reconcile_catalog_operation(
            &self.storage_root,
            &self.executor.catalog_key,
            &self.recovery_root,
            &self.operation_lease,
            plan.operation_id().as_str(),
            plan.data_location_id(),
            affected_rows,
            &verified_target_file_evidence,
        ) {
            let _ = preserve_failure_evidence(
                &journal,
                &self.target_db_path,
                &self.storage_root,
                plan.operation_id().as_str(),
            );
            let _ = journal.transition(OperationState::ManualRecoveryRequired);
            return SyncPlanExecutionOutcome::ManualRecoveryRequired {
                backups_preserved: true,
            };
        }
        if journal.transition(OperationState::Completed).is_err() {
            let _ = preserve_failure_evidence(
                &journal,
                &self.target_db_path,
                &self.storage_root,
                plan.operation_id().as_str(),
            );
            return SyncPlanExecutionOutcome::ManualRecoveryRequired {
                backups_preserved: true,
            };
        }

        let final_size = database_bundle_size(&self.target_db_path).or(Some(bundle_size));
        self.report_progress(
            plan,
            ProgressPhase::Completed,
            final_size.unwrap_or(0),
            final_size,
            false,
        );
        SyncPlanExecutionOutcome::Completed { affected_rows }
    }
}

/// 在目录库中记录并复核目标写入结果；失败时不允许 manifest 自动收口。
fn reconcile_catalog_operation(
    storage_root: &Path,
    raw_key: &str,
    recovery_root: &Path,
    operation_lease: &OperationLease,
    operation_id: &str,
    data_location_id: &str,
    affected_rows: u64,
    target_evidence: &TargetFileEvidence,
) -> bool {
    let catalog_path = match resolve_current_catalog_path(storage_root) {
        Ok(path) => path,
        Err(_) => return false,
    };
    let metadata = match std::fs::symlink_metadata(&catalog_path) {
        Ok(metadata) => metadata,
        Err(_) => return false,
    };
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return false;
    }
    if reject_catalog_sidecars(&catalog_path).is_err() {
        return false;
    }
    // 当前代次只能以 READ_WRITE 打开；不带 CREATE，避免路径漂移时静默生成空目录库。
    let mut connection = match open_catalog_readwrite_connection(&catalog_path, raw_key) {
        Ok(connection) => connection,
        Err(_) => return false,
    };
    let (cipher_integrity_ok, sqlite_integrity_ok) =
        run_integrity_checks_on_connection(&connection);
    if !cipher_integrity_ok || !sqlite_integrity_ok {
        return false;
    }
    let transaction = match connection.transaction() {
        Ok(transaction) => transaction,
        Err(_) => return false,
    };
    if transaction
        .execute_batch(
            "CREATE TABLE IF NOT EXISTS operation_record (
                 operation_id TEXT PRIMARY KEY,
                 data_location_id TEXT NOT NULL,
                 state TEXT NOT NULL,
                 affected_rows INTEGER NOT NULL,
                 db_fingerprint TEXT NOT NULL,
                 wal_fingerprint TEXT,
                 shm_fingerprint TEXT,
                 updated_at INTEGER NOT NULL
             );",
        )
        .is_err()
    {
        return false;
    }
    let affected_rows = match i64::try_from(affected_rows) {
        Ok(value) => value,
        Err(_) => return false,
    };
    if transaction
        .execute(
            "INSERT INTO operation_record (
                 operation_id, data_location_id, state, affected_rows,
                 db_fingerprint, wal_fingerprint, shm_fingerprint, updated_at
             ) VALUES (?1, ?2, 'completed', ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(operation_id) DO UPDATE SET
                 data_location_id = excluded.data_location_id,
                 state = excluded.state,
                 affected_rows = excluded.affected_rows,
                 db_fingerprint = excluded.db_fingerprint,
                 wal_fingerprint = excluded.wal_fingerprint,
                 shm_fingerprint = excluded.shm_fingerprint,
                 updated_at = excluded.updated_at",
            rusqlite::params![
                operation_id,
                data_location_id,
                affected_rows,
                target_evidence.db_fingerprint,
                target_evidence.wal_fingerprint,
                target_evidence.shm_fingerprint,
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map(|value| value.as_secs() as i64)
                    .unwrap_or(0),
            ],
        )
        .is_err()
        || bump_catalog_content_revision(&transaction).is_err()
        || transaction.commit().is_err()
    {
        return false;
    }
    // 提交后先释放写连接，再用独立连接复核，避免 Windows/SQLCipher 锁等待。
    drop(connection);
    if reconcile_current_catalog_sidecar(storage_root, raw_key, recovery_root, operation_lease)
        .is_err()
    {
        return false;
    }
    catalog_operation_matches(
        storage_root,
        raw_key,
        operation_id,
        data_location_id,
        target_evidence,
    )
}

/// 读取目录库完成记录，供当前执行和显式重启协调共同使用。
///
/// 调用方只能据此判断既有操作是否已经完整写入目录库；该函数只读打开目录库，
/// 并在返回匹配前执行 SQLCipher 与 SQLite 完整性检查。
pub fn catalog_operation_matches(
    storage_root: &Path,
    raw_key: &str,
    operation_id: &str,
    data_location_id: &str,
    target_evidence: &TargetFileEvidence,
) -> bool {
    let catalog_path = match resolve_current_catalog_path(storage_root) {
        Ok(path) => path,
        Err(_) => return false,
    };
    if reject_catalog_sidecars(&catalog_path).is_err() {
        return false;
    }
    let connection = match open_catalog_readonly_connection(&catalog_path, raw_key) {
        Ok(connection) => connection,
        Err(_) => return false,
    };
    // 只读匹配也必须见证持久 journal 协议，禁止把 WAL/其它模式当成已完成记录。
    if verify_catalog_read_protocol(&connection).is_err() {
        return false;
    }
    let (cipher_integrity_ok, sqlite_integrity_ok) =
        run_integrity_checks_on_connection(&connection);
    if !cipher_integrity_ok || !sqlite_integrity_ok {
        return false;
    }
    let row = connection.query_row(
        "SELECT data_location_id, state, db_fingerprint, wal_fingerprint, shm_fingerprint
         FROM operation_record WHERE operation_id = ?1",
        rusqlite::params![operation_id],
        |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, Option<String>>(4)?,
            ))
        },
    );
    let Ok((stored_location, state, db, wal, shm)) = row else {
        return false;
    };
    stored_location == data_location_id
        && state == "completed"
        && db == target_evidence.db_fingerprint
        && wal == target_evidence.wal_fingerprint
        && shm == target_evidence.shm_fingerprint
}

/// 复制捕获时存在的原始 DB/WAL/SHM，并用独立哈希清单验证每个副本。
fn capture_raw_backup(source_db_path: &Path, raw_dir: &Path) -> bool {
    capture_raw_backup_with_progress(source_db_path, raw_dir, |_| {}, || {}, |_| {})
}

/// 分块复制原始三件套，并在复制与副本哈希阶段分别报告真实字节进度。
fn capture_raw_backup_with_progress(
    source_db_path: &Path,
    raw_dir: &Path,
    mut on_copy_progress: impl FnMut(u64),
    mut on_hash_phase_start: impl FnMut(),
    mut on_hash_progress: impl FnMut(u64),
) -> bool {
    if std::fs::create_dir_all(raw_dir).is_err() {
        return false;
    }

    let source_files = [
        (source_db_path.to_path_buf(), "database.db", true),
        (
            database_sidecar_path(source_db_path, "-wal"),
            "database.db-wal",
            false,
        ),
        (
            database_sidecar_path(source_db_path, "-shm"),
            "database.db-shm",
            false,
        ),
    ];
    let mut copied_files = Vec::new();

    // 先完成整套复制；复制过程同时计算源文件哈希，避免把整文件载入内存。
    for (source_path, backup_name, required) in source_files {
        if !source_path.exists() {
            if required {
                return false;
            }
            continue;
        }

        let backup_path = raw_dir.join(backup_name);
        let source_hash =
            match copy_file_and_hash(&source_path, &backup_path, &mut on_copy_progress) {
                Some(hash) => hash,
                None => return false,
            };
        copied_files.push((backup_name, source_hash));
    }

    // 再验证副本哈希，阶段切换后每个字节仍通过分块读取计入进度。
    on_hash_phase_start();
    let mut hashes = Vec::new();
    for (backup_name, source_hash) in &copied_files {
        let backup_path = raw_dir.join(backup_name);
        let backup_hash = match hash_file_with_progress(&backup_path, &mut on_hash_progress) {
            Some(hash) => hash,
            None => return false,
        };
        if *source_hash != backup_hash {
            return false;
        }
        hashes.push(format!("{backup_hash}  {backup_name}"));
    }

    if std::fs::write(raw_dir.join("hashes.sha256"), hashes.join("\n")).is_err() {
        return false;
    }

    // 复制阶段已经逐个比较源文件与副本哈希；这里仅确认清单中的文件仍存在，避免再次完整读取大文件。
    copied_files
        .iter()
        .all(|(backup_name, _)| raw_dir.join(backup_name).is_file())
}

/// 原始备份清单只允许本次固定的三个文件名，防止验证范围被清单意外扩大。
fn verify_raw_backup(raw_dir: &Path) -> bool {
    let Some(manifest) = read_raw_backup_manifest(raw_dir) else {
        return false;
    };
    for (name, expected_hash) in manifest {
        let actual_hash = match sha256_file_for_backup(&raw_dir.join(&name)) {
            Some(hash) => hash,
            None => return false,
        };
        if actual_hash != expected_hash {
            return false;
        }
    }
    true
}

/// 读取原始备份哈希清单；调用方可复用已验证哈希，避免重复扫描大文件。
fn read_raw_backup_manifest(raw_dir: &Path) -> Option<BTreeMap<String, String>> {
    let contents = std::fs::read_to_string(raw_dir.join("hashes.sha256")).ok()?;
    let mut manifest = BTreeMap::new();
    for line in contents.lines().filter(|line| !line.is_empty()) {
        let (expected_hash, name) = line.split_once("  ")?;
        if !matches!(name, "database.db" | "database.db-wal" | "database.db-shm")
            || manifest
                .insert(name.to_string(), expected_hash.to_string())
                .is_some()
        {
            return None;
        }
        if !raw_dir.join(name).is_file() {
            return None;
        }
    }
    (!manifest.is_empty()).then_some(manifest)
}

/// 生成目标数据库的 WAL 或 SHM 路径，不依赖当前文件扩展名。
fn database_sidecar_path(db_path: &Path, suffix: &str) -> PathBuf {
    PathBuf::from(format!("{}{}", db_path.to_string_lossy(), suffix))
}

/// 只统计当前数据库三件套字节数；缺失 sidecar 不算失败，读取异常则返回未知总量。
fn database_bundle_size(db_path: &Path) -> Option<u64> {
    let mut total = 0_u64;
    let mut found = false;
    for path in [
        db_path.to_path_buf(),
        database_sidecar_path(db_path, "-wal"),
        database_sidecar_path(db_path, "-shm"),
    ] {
        match std::fs::metadata(path) {
            Ok(metadata) if metadata.is_file() => {
                total = total.checked_add(metadata.len())?;
                found = true;
            }
            Ok(_) => return None,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return None,
        }
    }
    found.then_some(total)
}

/// 只比较位置身份，不比较会随正常写入变化的数据库哈希和 sidecar 内容。
fn location_identity_matches(expected: &LocationWitness, actual: &LocationWitness) -> bool {
    expected.data_location_id == actual.data_location_id
        && expected.canonical_root == actual.canonical_root
        && expected.db_relative_path == actual.db_relative_path
        && expected.root_identity == actual.root_identity
        && expected.db_identity == actual.db_identity
}

/// 比较目标三件套与计划固定指纹；存在性变化也视为漂移，不允许继续写入。
fn target_file_matches_plan(target_db_path: &Path, plan: &SyncPlan) -> bool {
    target_file_matches_evidence(target_db_path, plan.target_file_evidence())
}

/// 拿到 WAL 写锁后，SHM 的锁元数据会被 SQLite 自身更新；仍必须固定校验主库和 WAL。
#[cfg(test)]
fn target_database_and_wal_match_plan(target_db_path: &Path, plan: &SyncPlan) -> bool {
    target_database_and_wal_match_evidence(target_db_path, plan.target_file_evidence())
}

/// 事务开始前只比较主库和 WAL，允许 SQLite 在获取写锁时更新 SHM 锁元数据。
fn target_database_and_wal_match_evidence(
    target_db_path: &Path,
    expected: &TargetFileEvidence,
) -> bool {
    let wal_path = database_sidecar_path(target_db_path, "-wal");
    let wal_matches = match expected.wal_fingerprint.as_ref() {
        Some(expected) => sha256_file_for_backup(&wal_path).as_deref() == Some(expected.as_str()),
        // 已在拿锁前完整验证三件套；SQLite 取得 WAL 写锁时可创建空文件或仅写入 32 字节头。
        // 任何 frame 都超过头大小，仍作为锁窗口内的外部写入证据拒绝。
        None => match std::fs::metadata(&wal_path) {
            Ok(metadata) => metadata.len() <= 32,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => true,
            Err(_) => false,
        },
    };
    sha256_file_for_backup(target_db_path).as_deref() == Some(expected.db_fingerprint.as_str())
        && wal_matches
}

/// 读取当前目标三件套指纹；主数据库缺失或不可读时不生成可完成凭据。
fn target_file_evidence(target_db_path: &Path) -> Option<traesync_domain::TargetFileEvidence> {
    Some(traesync_domain::TargetFileEvidence {
        db_fingerprint: sha256_file_for_backup(target_db_path)?,
        wal_fingerprint: sha256_file_for_backup(&database_sidecar_path(target_db_path, "-wal")),
        shm_fingerprint: sha256_file_for_backup(&database_sidecar_path(target_db_path, "-shm")),
    })
}

/// 比较目标三件套与固定指纹；存在性变化也视为漂移，不允许自动完成。
fn target_file_matches_evidence(
    target_db_path: &Path,
    expected: &traesync_domain::TargetFileEvidence,
) -> bool {
    let current_matches = |path: PathBuf, fingerprint: Option<&String>| match fingerprint {
        Some(expected) => sha256_file_for_backup(&path).as_deref() == Some(expected.as_str()),
        None => !path.exists(),
    };

    sha256_file_for_backup(target_db_path).as_deref() == Some(expected.db_fingerprint.as_str())
        && current_matches(
            database_sidecar_path(target_db_path, "-wal"),
            expected.wal_fingerprint.as_ref(),
        )
        && current_matches(
            database_sidecar_path(target_db_path, "-shm"),
            expected.shm_fingerprint.as_ref(),
        )
}

/// 写后验证失败时捕获当前目标现场；失败也不删除已验证的写前双备份。
fn capture_failure_evidence(
    target_db_path: &Path,
    storage_root: &Path,
    operation_id: &str,
) -> bool {
    let failure_raw_dir = storage_root
        .join("backups")
        .join(operation_id)
        .join("failure")
        .join("raw");
    let Some(failure_dir) = failure_raw_dir.parent() else {
        return false;
    };
    if std::fs::create_dir_all(failure_dir).is_err() {
        return false;
    }
    match std::fs::create_dir(&failure_raw_dir) {
        // 当前调用抢到首次现场目录后才允许写入；后续恢复不得覆盖它。
        Ok(()) => capture_raw_backup(target_db_path, &failure_raw_dir),
        Err(_) => verify_raw_backup(&failure_raw_dir),
    }
}

/// 写后异常先持久化现场保存意图，再捕获目标三件套并记录验证完成。
fn preserve_failure_evidence(
    journal: &OperationManifestJournal,
    target_db_path: &Path,
    storage_root: &Path,
    operation_id: &str,
) -> bool {
    match journal.latest_state() {
        // 进程可能在“准备保存现场”后终止。恢复时不能重复追加同一状态，
        // 但可以继续完成首次现场捕获并收口到已验证状态。
        Some(OperationState::FailurePreserving) => {
            capture_failure_evidence(target_db_path, storage_root, operation_id)
                && journal
                    .transition(OperationState::FailureSnapshotVerified)
                    .is_ok()
        }
        // 现场已经完成验证时只需再次校验，不覆盖已有证据。
        Some(OperationState::FailureSnapshotVerified) => {
            capture_failure_evidence(target_db_path, storage_root, operation_id)
        }
        // 恢复发布阶段本身不能回跳到 FailurePreserving；保留当前目标现场后，
        // 外层协调器会追加 ManualRecoveryRequired。
        Some(
            OperationState::RestoreStaging
            | OperationState::RestoreStaged
            | OperationState::RestoreReplacing
            | OperationState::RestoredVerifying,
        ) => capture_failure_evidence(target_db_path, storage_root, operation_id),
        _ => {
            journal
                .transition(OperationState::FailurePreserving)
                .is_ok()
                && capture_failure_evidence(target_db_path, storage_root, operation_id)
                && journal
                    .transition(OperationState::FailureSnapshotVerified)
                    .is_ok()
        }
    }
}

/// 真实矩阵中复用同类已验证失败现场；硬链接保持字节一致，不重复复制大库。
#[cfg(test)]
fn link_reused_failure_evidence(
    journal: &OperationManifestJournal,
    canonical_raw_dir: &Path,
    storage_root: &Path,
    operation_id: &str,
) -> bool {
    let failure_raw_dir = storage_root
        .join("backups")
        .join(operation_id)
        .join("failure")
        .join("raw");
    let Some(failure_dir) = failure_raw_dir.parent() else {
        return false;
    };

    let current_state = journal.latest_state();
    let needs_snapshot_transition = match current_state {
        Some(OperationState::FailurePreserving) => true,
        Some(OperationState::FailureSnapshotVerified)
        | Some(
            OperationState::RestoreStaging
            | OperationState::RestoreStaged
            | OperationState::RestoreReplacing
            | OperationState::RestoredVerifying,
        ) => false,
        _ => {
            if journal
                .transition(OperationState::FailurePreserving)
                .is_err()
            {
                return false;
            }
            true
        }
    };
    if failure_raw_dir.exists() {
        let can_reuse_existing = matches!(
            current_state,
            Some(
                OperationState::FailureSnapshotVerified
                    | OperationState::RestoreStaging
                    | OperationState::RestoreStaged
                    | OperationState::RestoreReplacing
                    | OperationState::RestoredVerifying
            )
        );
        let result = can_reuse_existing && verify_raw_backup(&failure_raw_dir);
        return result;
    }
    if std::fs::create_dir_all(failure_dir).is_err() {
        return false;
    }
    if std::fs::create_dir(&failure_raw_dir).is_err() {
        return false;
    }

    for file_name in [
        "database.db",
        "database.db-wal",
        "database.db-shm",
        "hashes.sha256",
    ] {
        let source = canonical_raw_dir.join(file_name);
        let destination = failure_raw_dir.join(file_name);
        if source.is_file() {
            if std::fs::hard_link(&source, &destination).is_err() {
                return false;
            }
        } else if file_name == "database.db" {
            return false;
        }
    }

    if needs_snapshot_transition {
        journal
            .transition(OperationState::FailureSnapshotVerified)
            .is_ok()
    } else {
        true
    }
}

/// 仅用于备份验证的 SHA-256 计算；失败关闭，不返回部分结果。
fn sha256_file_for_backup(path: &Path) -> Option<String> {
    hash_file_with_progress(path, |_| {})
}

/// 分块读取文件并返回 SHA-256；回调只收到本次读取的字节数。
fn hash_file_with_progress(path: &Path, mut on_progress: impl FnMut(u64)) -> Option<String> {
    let mut file = std::fs::File::open(path).ok()?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; 1024 * 1024];
    loop {
        let count = file.read(&mut buffer).ok()?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
        on_progress(count as u64);
    }
    Some(hex::encode(hasher.finalize()))
}

/// 分块复制并报告真实字节进度，再独立计算源哈希；目标使用临时文件发布，避免覆盖旧证据。
fn copy_file_and_hash(
    source: &Path,
    destination: &Path,
    mut on_progress: impl FnMut(u64),
) -> Option<String> {
    if destination.exists() {
        return None;
    }
    let file_name = destination.file_name()?.to_string_lossy();
    let temporary = destination.with_file_name(format!(".{file_name}.tmp-{}", std::process::id()));
    if temporary.exists() {
        return None;
    }

    // 每个固定大小分块都回调一次，避免慢盘复制期间进度长时间停在旧值。
    // 复制失败时保留临时现场，下一轮不会误把半成品当作正式备份。
    let result = (|| {
        let mut source_file = std::fs::File::open(source).ok()?;
        let mut temporary_file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .ok()?;
        let mut buffer = vec![0_u8; 1024 * 1024];
        loop {
            let count = source_file.read(&mut buffer).ok()?;
            if count == 0 {
                break;
            }
            temporary_file.write_all(&buffer[..count]).ok()?;
            on_progress(count as u64);
        }
        temporary_file.sync_all().ok()?;
        drop(temporary_file);

        // 在发布前计算源哈希，随后由调用方独立计算临时文件发布后的哈希并比较。
        let source_hash = hash_file_with_progress(source, |_| {})?;
        std::fs::rename(&temporary, destination).ok()?;
        Some(source_hash)
    })();

    // 临时文件不是正式失败证据；任一中间步骤失败都立即清理，避免大文件残留。
    if result.is_none() {
        let _ = std::fs::remove_file(&temporary);
    }
    result
}

/// 向指定恢复目录写入 SQLCipher 逻辑副本，绝不覆盖既有备份。
fn backup_to_logical_copy_at_inner(source_db: &Path, raw_key: &str, destination: &Path) -> bool {
    let parent = match destination.parent() {
        Some(parent) => parent,
        None => return false,
    };
    if destination.exists() || std::fs::create_dir_all(parent).is_err() {
        return false;
    }

    // SQLCipher 导出要求主连接可写，因此仅在隔离副本上执行导出。
    let source_copy = match create_readonly_trio_copy(source_db) {
        Ok(copy) => copy,
        Err(_) => return false,
    };
    let conn = match open_with_key(&source_copy.db_path, raw_key) {
        Ok(conn) => conn,
        Err(_) => return false,
    };
    export_sqlcipher_logical_copy(conn, raw_key, destination)
}

/// 执行 SQLCipher 导出，并在返回成功前完成目标文件耐久同步。
fn export_sqlcipher_logical_copy(conn: Connection, raw_key: &str, destination: &Path) -> bool {
    // SQLCipher 的 KEY 语法不接受绑定参数；路径来自已受 fixture 防护的组合根，仍逐字转义。
    let destination_text = destination.to_string_lossy().replace('\'', "''");
    let attach_sql = format!("ATTACH DATABASE '{destination_text}' AS dst KEY \"x'{raw_key}'\";");
    if conn.execute_batch(&attach_sql).is_err() {
        return false;
    }

    // 备份属于恢复证据；目标连接使用 FULL，不能以吞吐量换取掉电后的未落盘窗口。
    let synchronous_full = conn.execute_batch("PRAGMA dst.synchronous = FULL;").is_ok();
    let exported = synchronous_full
        && conn
            .query_row("SELECT sqlcipher_export('dst')", [], |_row| Ok(()))
            .is_ok();
    let detached = conn.execute_batch("DETACH DATABASE dst;").is_ok();
    drop(conn);

    if !exported || !detached || !destination.is_file() {
        return false;
    }
    std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(destination)
        .and_then(|file| file.sync_all())
        .is_ok()
}

/// 用新只读连接验证逻辑副本的 schema 与两层完整性检查。
fn verify_logical_backup(logical_db_path: &Path, raw_key: &str) -> bool {
    let conn = match open_with_key_readonly(logical_db_path, raw_key) {
        Ok(conn) => conn,
        Err(_) => return false,
    };
    let (cipher_ok, sqlite_ok) = run_integrity_checks_on_connection(&conn);
    cipher_ok && sqlite_ok && check_schema(&conn).is_ok()
}

/// 旧的 fixture 纵切复用无额外证据检查的事务入口。
#[cfg(test)]
fn apply_follow_project_transaction(
    target_db_path: &Path,
    raw_key: &str,
    project_id: &str,
    expected_source_user_id: &str,
    target_user_id: &str,
) -> Option<FollowProjectCommit> {
    match apply_follow_project_transaction_with_evidence(
        target_db_path,
        raw_key,
        project_id,
        expected_source_user_id,
        target_user_id,
        || true,
    ) {
        FollowProjectTransaction::Committed(commit) => Some(commit),
        FollowProjectTransaction::EvidenceChanged | FollowProjectTransaction::Failed => None,
    }
}

/// 使用参数绑定更新 owner，并在提交前再次验证计划证据。
fn apply_follow_project_transaction_with_evidence<F>(
    target_db_path: &Path,
    raw_key: &str,
    project_id: &str,
    expected_source_user_id: &str,
    target_user_id: &str,
    before_commit: F,
) -> FollowProjectTransaction
where
    F: FnOnce() -> bool,
{
    let mut conn = match open_with_key(target_db_path, raw_key) {
        Ok(conn) => conn,
        Err(_) => return FollowProjectTransaction::Failed,
    };
    let tx = match conn.transaction() {
        Ok(tx) => tx,
        Err(_) => return FollowProjectTransaction::Failed,
    };
    let (actual_owner, biz_project_id): (String, String) = tx
        .query_row(
            "SELECT user_id, biz_project_id FROM project WHERE project_id = ?1",
            params![project_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .ok()
        .unwrap_or_else(|| (String::new(), String::new()));
    if actual_owner != expected_source_user_id {
        return FollowProjectTransaction::Failed;
    }

    // 固定写前会话数量，提交后新连接必须确认未涉及关系没有被意外改写。
    let session_count: i64 = tx
        .query_row(
            "SELECT COUNT(*) FROM chat_session WHERE project_id = ?1",
            params![project_id],
            |row| row.get(0),
        )
        .unwrap_or(-1);
    if session_count < 0 {
        return FollowProjectTransaction::Failed;
    }

    let affected_rows = tx
        .execute(
            "UPDATE project SET user_id = ?1 WHERE project_id = ?2 AND user_id = ?3",
            params![target_user_id, project_id, expected_source_user_id],
        )
        .unwrap_or(0);
    if affected_rows != 1 {
        return FollowProjectTransaction::Failed;
    }

    let matching_projects: i64 = tx
        .query_row(
            "SELECT COUNT(*) FROM project WHERE biz_project_id = ?1 AND user_id = ?2",
            params![biz_project_id, target_user_id],
            |row| row.get(0),
        )
        .unwrap_or(-1);
    if matching_projects != 1 {
        return FollowProjectTransaction::Failed;
    }

    // 更新仍未提交，对外新连接看不到本事务；证据变化时 drop 事务即回滚。
    if !before_commit() {
        return FollowProjectTransaction::EvidenceChanged;
    }
    if tx.commit().is_err() {
        return FollowProjectTransaction::Failed;
    }
    FollowProjectTransaction::Committed(FollowProjectCommit {
        affected_rows: affected_rows as u64,
        session_count,
    })
}

/// 提交后重新打开目标数据库，确认 owner 和两层完整性检查均通过。
fn verify_follow_project_after_commit(
    target_db_path: &Path,
    raw_key: &str,
    project_id: &str,
    target_user_id: &str,
    expected_session_count: i64,
) -> bool {
    let conn = match open_with_key_readonly(target_db_path, raw_key) {
        Ok(conn) => conn,
        Err(_) => return false,
    };
    let owner: Option<String> = conn
        .query_row(
            "SELECT user_id FROM project WHERE project_id = ?1",
            params![project_id],
            |row| row.get(0),
        )
        .ok();
    let session_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM chat_session WHERE project_id = ?1",
            params![project_id],
            |row| row.get(0),
        )
        .unwrap_or(-1);
    let (cipher_ok, sqlite_ok) = run_integrity_checks_on_connection(&conn);
    owner.as_deref() == Some(target_user_id)
        && session_count == expected_session_count
        && cipher_ok
        && sqlite_ok
}

/// 混合批量计划使用统一的写后断言复核，避免同一目标项目收到多个动作时
/// 复用单动作计数快照而误判。消息、FTS 和其它表仍通过 SQLCipher/SQLite 完整性检查。
fn verify_plan_assertions_after_commit(
    target_db_path: &Path,
    raw_key: &str,
    assertions: &[PlanAssertion],
) -> bool {
    let conn = match open_with_key_readonly(target_db_path, raw_key) {
        Ok(conn) => conn,
        Err(_) => return false,
    };
    for assertion in assertions {
        let valid = match assertion {
            PlanAssertion::ProjectOwner {
                project_id,
                expected_user_id,
            } => conn
                .query_row(
                    "SELECT user_id FROM project WHERE project_id = ?1",
                    params![project_id],
                    |row| row.get::<_, String>(0),
                )
                .is_ok_and(|owner| owner == *expected_user_id),
            PlanAssertion::SessionProject {
                session_id,
                expected_project_id,
            } => conn
                .query_row(
                    "SELECT project_id FROM chat_session WHERE session_id = ?1",
                    params![session_id.original_session_id.as_str()],
                    |row| row.get::<_, String>(0),
                )
                .is_ok_and(|project_id| project_id == *expected_project_id),
        };
        if !valid {
            return false;
        }
    }
    let (cipher_ok, sqlite_ok) = run_integrity_checks_on_connection(&conn);
    cipher_ok && sqlite_ok
}

/// 仅接受文件名安全且已存在的目标 sandbox 配置；缺失时不进入事务。
fn target_sandbox_is_available(fixture_root: &Path, target_project_id: &str) -> bool {
    !target_project_id.is_empty()
        && !target_project_id.contains(['/', '\\'])
        && fixture_root
            .join("sandbox")
            .join(format!("{target_project_id}.json"))
            .is_file()
}

/// 会话重挂仅接受已逐表核对过的关系列；额外的非关系列和只读表由全表摘要保护。
fn attach_relation_schema_layout(conn: &Connection) -> Option<ArtifactRelationLayout> {
    const REQUIRED_COLUMNS: &[(&str, &[&str])] = &[
        ("project", &["project_id", "user_id", "biz_project_id"]),
        ("chat_session", &["session_id", "project_id"]),
        ("chat_message", &["message_id", "session_id"]),
        ("session_project", &["session_id", "project_id"]),
        ("snapshot", &["chat_session_id", "project_id"]),
        ("staging", &["chat_session_id", "project_id"]),
    ];

    for (table, required_columns) in REQUIRED_COLUMNS {
        let columns = table_columns(conn, table)?;
        let required_columns = required_columns
            .iter()
            .map(|column| (*column).to_string())
            .collect::<BTreeSet<_>>();
        if !columns.is_superset(&required_columns) {
            return None;
        }
    }

    let artifact_columns = table_columns(conn, "local_artifact")?;
    let version_columns = table_columns(conn, "local_artifact_version")?;
    let legacy = [
        "artifact_id",
        "source_session_id",
        "source_project_id",
        "user_id",
    ]
    .into_iter()
    .all(|column| artifact_columns.contains(column))
        && ["artifact_id", "source_project_id"]
            .into_iter()
            .all(|column| version_columns.contains(column));
    let current = [
        "artifact_id",
        "creator_session_id",
        "source_project_id",
        "user_id",
    ]
    .into_iter()
    .all(|column| artifact_columns.contains(column))
        && ["artifact_id", "source_project_id", "writer_session_id"]
            .into_iter()
            .all(|column| version_columns.contains(column));
    let artifact_layout = match (legacy, current) {
        (true, false) => ArtifactRelationLayout::LegacySourceSession,
        (false, true) => ArtifactRelationLayout::CreatorAndWriterSession,
        _ => return None,
    };

    let mut statement = conn
        .prepare("SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%'")
        .ok()?;
    let tables = statement
        .query_map([], |row| row.get::<_, String>(0))
        .ok()?;
    for table in tables {
        let table = table.ok()?;
        let columns = table_columns(conn, &table)?;
        for column in columns.iter().filter(|column| is_relation_column(column)) {
            if !known_relation_column(&table, column, artifact_layout) {
                return None;
            }
        }
        // 受限重挂尚未为 SQLite 外键建立逐项映射；任意显式外键均视为未知关系而拒绝。
        if table_has_declared_foreign_key(conn, &table)? {
            return None;
        }
    }
    Some(artifact_layout)
}

/// 未被清单识别的项目、会话和账号关系可能需要随写入变化，必须在事务前拒绝。
fn is_relation_column(column: &str) -> bool {
    column == "user_id"
        || [
            "project",
            "session",
            "owner",
            "account",
            "workspace",
            "conversation",
            "artifact",
        ]
        .into_iter()
        .any(|kind| {
            column.contains(kind)
                && (column.ends_with("_id")
                    || column.ends_with("_ref")
                    || column.ends_with("_uuid"))
        })
}

/// 显式外键没有经过本次重挂的逐项证明，不能把它当作可安全保留的未知关系。
fn table_has_declared_foreign_key(conn: &Connection, table: &str) -> Option<bool> {
    let pragma = format!("PRAGMA foreign_key_list({})", quote_sql_identifier(table));
    let mut statement = conn.prepare(&pragma).ok()?;
    let mut rows = statement.query([]).ok()?;
    Some(rows.next().ok()?.is_some())
}

/// 真实 TRAE 2026-08 schema 中这些列明确保持只读或由本动作精确更新。
fn known_relation_column(
    table: &str,
    column: &str,
    artifact_layout: ArtifactRelationLayout,
) -> bool {
    match table {
        "agent" | "model_config_cache" | "user_configuration" => column == "user_id",
        "agent_run"
        | "chat_message"
        | "chat_session_goal"
        | "chat_turn"
        | "history_todo_list"
        | "history_v2"
        | "plan"
        | "proposal"
        | "task"
        | "worktree"
        | "chat_fts"
        | "fts_message_content"
        | "fts_session_title" => column == "session_id",
        "checkpoint" | "rules_attachment" | "scheduled_task_executions" => {
            column == "chat_session_id"
        }
        "core_memory" => matches!(column, "user_id" | "project_id"),
        "local_artifact" => {
            matches!(column, "artifact_id" | "user_id" | "source_project_id")
                || column == artifact_layout.artifact_session_column()
        }
        "local_artifact_version" => {
            column == "artifact_id"
                || column == "source_project_id"
                || (artifact_layout.version_requires_writer_session()
                    && column == "writer_session_id")
        }
        "project" => matches!(
            column,
            "project_id"
                | "user_id"
                | "biz_project_id"
                | "transient_fallback_project_id"
                | "remote_project_id"
        ),
        "scheduled_tasks" => matches!(column, "user_id" | "local_project_id"),
        // conversation_id 是真实 TRAE schema 中稳定的会话标识；该表只读并受全表摘要保护。
        "server_history_info" => matches!(column, "conversation_id" | "session_id" | "user_id"),
        "chat_session" | "session_project" => matches!(column, "session_id" | "project_id"),
        "snapshot" | "staging" => matches!(column, "chat_session_id" | "project_id"),
        _ => false,
    }
}

/// 按已确认布局检查 artifact 与来源会话、项目、账号之间没有漂移关系。
fn artifact_mismatch_count(
    conn: &Connection,
    artifact_layout: ArtifactRelationLayout,
    session_id: &str,
    project_id: &str,
    user_id: &str,
) -> i64 {
    let session_column = artifact_layout.artifact_session_column();
    let sql = format!(
        "SELECT COUNT(*) FROM local_artifact WHERE {session_column} = ?1 AND (source_project_id <> ?2 OR user_id <> ?3)"
    );
    conn.query_row(&sql, params![session_id, project_id, user_id], |row| {
        row.get(0)
    })
    .unwrap_or(-1)
}

/// 当前布局额外要求版本记录仍由同一选中会话写入，避免把未选中会话的关系带入事务。
fn artifact_version_mismatch_count(
    conn: &Connection,
    artifact_layout: ArtifactRelationLayout,
    session_id: &str,
    project_id: &str,
) -> i64 {
    match artifact_layout {
        ArtifactRelationLayout::LegacySourceSession => conn
            .query_row(
                "SELECT COUNT(*) FROM local_artifact_version version INNER JOIN local_artifact artifact ON artifact.artifact_id = version.artifact_id WHERE artifact.source_session_id = ?1 AND version.source_project_id <> ?2",
                params![session_id, project_id],
                |row| row.get(0),
            )
            .unwrap_or(-1),
        ArtifactRelationLayout::CreatorAndWriterSession => conn
            .query_row(
                "SELECT COUNT(*) FROM local_artifact_version version INNER JOIN local_artifact artifact ON artifact.artifact_id = version.artifact_id WHERE artifact.creator_session_id = ?1 AND (version.source_project_id <> ?2 OR version.writer_session_id <> ?1)",
                params![session_id, project_id],
                |row| row.get(0),
            )
            .unwrap_or(-1),
    }
}

/// 在创建 manifest 和备份前验证 sandbox、schema 与选中关系，避免无效计划进入写入阶段。
fn attach_sessions_preflight(
    target_db_path: &Path,
    raw_key: &str,
    fixture_root: &Path,
    source_project_id: &str,
    target_project_id: &str,
    session_ids: &[traesync_domain::SessionIdentity],
) -> bool {
    if source_project_id == target_project_id
        || session_ids.is_empty()
        || !target_sandbox_is_available(fixture_root, target_project_id)
    {
        return false;
    }
    let conn = match open_with_key_readonly(target_db_path, raw_key) {
        Ok(conn) => conn,
        Err(_) => return false,
    };
    let Some(artifact_layout) = attach_relation_schema_layout(&conn) else {
        return false;
    };
    let Some(selected_session_ids) = selected_session_ids(session_ids) else {
        return false;
    };

    let source_user_id: String = match conn.query_row(
        "SELECT user_id FROM project WHERE project_id = ?1",
        params![source_project_id],
        |row| row.get::<_, String>(0),
    ) {
        Ok(user_id) if !user_id.is_empty() => user_id,
        _ => return false,
    };
    let target_user_id: String = match conn.query_row(
        "SELECT user_id FROM project WHERE project_id = ?1",
        params![target_project_id],
        |row| row.get::<_, String>(0),
    ) {
        Ok(user_id) if !user_id.is_empty() => user_id,
        _ => return false,
    };
    if source_user_id == target_user_id {
        return false;
    }
    let source_biz_project_id: String = match conn.query_row(
        "SELECT biz_project_id FROM project WHERE project_id = ?1",
        params![source_project_id],
        |row| row.get::<_, String>(0),
    ) {
        Ok(project_id) if !project_id.is_empty() => project_id,
        _ => return false,
    };
    let target_biz_project_id: String = match conn.query_row(
        "SELECT biz_project_id FROM project WHERE project_id = ?1",
        params![target_project_id],
        |row| row.get::<_, String>(0),
    ) {
        Ok(project_id) if !project_id.is_empty() => project_id,
        _ => return false,
    };
    if source_biz_project_id != target_biz_project_id {
        return false;
    }
    if !artifact_ids_are_unambiguous(
        &conn,
        source_project_id,
        &source_user_id,
        &selected_session_ids,
        artifact_layout,
    ) {
        return false;
    }

    for session in session_ids {
        let session_id = session.original_session_id.as_str();
        let source_session_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM chat_session WHERE session_id = ?1 AND project_id = ?2",
                params![session_id, source_project_id],
                |row| row.get(0),
            )
            .unwrap_or(-1);
        let source_relation_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM session_project WHERE session_id = ?1 AND project_id = ?2",
                params![session_id, source_project_id],
                |row| row.get(0),
            )
            .unwrap_or(-1);
        let total_relation_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM session_project WHERE session_id = ?1",
                params![session_id],
                |row| row.get(0),
            )
            .unwrap_or(-1);
        let snapshot_mismatch_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM snapshot WHERE chat_session_id = ?1 AND project_id <> ?2",
                params![session_id, source_project_id],
                |row| row.get(0),
            )
            .unwrap_or(-1);
        let staging_mismatch_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM staging WHERE chat_session_id = ?1 AND project_id <> ?2",
                params![session_id, source_project_id],
                |row| row.get(0),
            )
            .unwrap_or(-1);
        let artifact_mismatch_count = artifact_mismatch_count(
            &conn,
            artifact_layout,
            session_id,
            source_project_id,
            &source_user_id,
        );
        let artifact_version_mismatch_count =
            artifact_version_mismatch_count(&conn, artifact_layout, session_id, source_project_id);
        if source_session_count != 1
            || source_relation_count != 1
            || total_relation_count != 1
            || snapshot_mismatch_count != 0
            || staging_mismatch_count != 0
            || artifact_mismatch_count != 0
            || artifact_version_mismatch_count != 0
        {
            return false;
        }
    }

    true
}

/// 使用参数绑定的单事务重挂；每张允许表按选中 session 精确更新。
fn apply_attach_sessions_transaction_with_evidence<P, C>(
    target_db_path: &Path,
    raw_key: &str,
    source_project_id: &str,
    target_project_id: &str,
    session_ids: &[traesync_domain::SessionIdentity],
    before_first_write: P,
    before_commit: C,
) -> AttachSessionsTransaction
where
    P: FnOnce() -> bool,
    C: FnOnce() -> bool,
{
    let mut conn = match open_with_key(target_db_path, raw_key) {
        Ok(conn) => conn,
        Err(_) => return AttachSessionsTransaction::Failed,
    };
    // 先拿到 SQLite 写锁，再验证计划文件证据，封住预检与第一条 DML 之间的并发写入窗口。
    let tx = match conn.transaction_with_behavior(TransactionBehavior::Immediate) {
        Ok(tx) => tx,
        Err(_) => return AttachSessionsTransaction::Failed,
    };
    if !before_first_write() {
        return AttachSessionsTransaction::EvidenceChanged;
    }
    let Some(artifact_layout) = attach_relation_schema_layout(&tx) else {
        return AttachSessionsTransaction::Failed;
    };
    if session_ids.is_empty() {
        return AttachSessionsTransaction::Failed;
    }

    let source_user_id: String = match tx.query_row(
        "SELECT user_id FROM project WHERE project_id = ?1",
        params![source_project_id],
        |row| row.get::<_, String>(0),
    ) {
        Ok(user_id) if !user_id.is_empty() => user_id,
        _ => return AttachSessionsTransaction::Failed,
    };
    let target_user_id: String = match tx.query_row(
        "SELECT user_id FROM project WHERE project_id = ?1",
        params![target_project_id],
        |row| row.get::<_, String>(0),
    ) {
        Ok(user_id) if !user_id.is_empty() => user_id,
        _ => return AttachSessionsTransaction::Failed,
    };
    if source_project_id == target_project_id || source_user_id == target_user_id {
        return AttachSessionsTransaction::Failed;
    }
    let source_biz_project_id: String = match tx.query_row(
        "SELECT biz_project_id FROM project WHERE project_id = ?1",
        params![source_project_id],
        |row| row.get::<_, String>(0),
    ) {
        Ok(project_id) if !project_id.is_empty() => project_id,
        _ => return AttachSessionsTransaction::Failed,
    };
    let target_biz_project_id: String = match tx.query_row(
        "SELECT biz_project_id FROM project WHERE project_id = ?1",
        params![target_project_id],
        |row| row.get::<_, String>(0),
    ) {
        Ok(project_id) if !project_id.is_empty() => project_id,
        _ => return AttachSessionsTransaction::Failed,
    };
    if source_biz_project_id != target_biz_project_id {
        return AttachSessionsTransaction::Failed;
    }

    let source_session_count: i64 = tx
        .query_row(
            "SELECT COUNT(*) FROM chat_session WHERE project_id = ?1",
            params![source_project_id],
            |row| row.get(0),
        )
        .unwrap_or(-1);
    let target_session_count: i64 = tx
        .query_row(
            "SELECT COUNT(*) FROM chat_session WHERE project_id = ?1",
            params![target_project_id],
            |row| row.get(0),
        )
        .unwrap_or(-1);
    let source_relation_count: i64 = tx
        .query_row(
            "SELECT COUNT(*) FROM session_project WHERE project_id = ?1",
            params![source_project_id],
            |row| row.get(0),
        )
        .unwrap_or(-1);
    let target_relation_count: i64 = tx
        .query_row(
            "SELECT COUNT(*) FROM session_project WHERE project_id = ?1",
            params![target_project_id],
            |row| row.get(0),
        )
        .unwrap_or(-1);
    let message_count: i64 = tx
        .query_row("SELECT COUNT(*) FROM chat_message", [], |row| row.get(0))
        .unwrap_or(-1);
    let mutation_scope = match AttachSessionMutationScope::from_source(
        &tx,
        source_project_id,
        &source_user_id,
        session_ids,
    ) {
        Some(scope) => scope,
        None => return AttachSessionsTransaction::Failed,
    };
    // 对每个用户表保留内容摘要，仅把明确允许的单元格归一化。
    let protected_table_fingerprints = match fingerprint_protected_tables(&tx, &mutation_scope) {
        Some(fingerprints) => fingerprints,
        None => return AttachSessionsTransaction::Failed,
    };
    let selected_count = session_ids.len() as i64;
    if source_session_count < selected_count || source_relation_count < selected_count {
        return AttachSessionsTransaction::Failed;
    }

    let mut affected_rows = 0_u64;
    let mut seen_session_ids = BTreeSet::new();
    for session in session_ids {
        let session_id = session.original_session_id.as_str();
        if session.product_history_namespace != "work_cn"
            || session_id.is_empty()
            || !seen_session_ids.insert(session_id)
        {
            return AttachSessionsTransaction::Failed;
        }

        let snapshot_mismatch_count: i64 = tx
            .query_row(
                "SELECT COUNT(*) FROM snapshot WHERE chat_session_id = ?1 AND project_id <> ?2",
                params![session_id, source_project_id],
                |row| row.get(0),
            )
            .unwrap_or(-1);
        let total_relation_count: i64 = tx
            .query_row(
                "SELECT COUNT(*) FROM session_project WHERE session_id = ?1",
                params![session_id],
                |row| row.get(0),
            )
            .unwrap_or(-1);
        let staging_mismatch_count: i64 = tx
            .query_row(
                "SELECT COUNT(*) FROM staging WHERE chat_session_id = ?1 AND project_id <> ?2",
                params![session_id, source_project_id],
                |row| row.get(0),
            )
            .unwrap_or(-1);
        let artifact_mismatch_count = artifact_mismatch_count(
            &tx,
            artifact_layout,
            session_id,
            source_project_id,
            &source_user_id,
        );
        let artifact_version_mismatch_count =
            artifact_version_mismatch_count(&tx, artifact_layout, session_id, source_project_id);
        if total_relation_count != 1
            || snapshot_mismatch_count != 0
            || staging_mismatch_count != 0
            || artifact_mismatch_count != 0
            || artifact_version_mismatch_count != 0
        {
            return AttachSessionsTransaction::Failed;
        }

        let artifact_version_count: i64 = match artifact_layout {
            ArtifactRelationLayout::LegacySourceSession => tx
                .query_row(
                    "SELECT COUNT(*) FROM local_artifact_version version INNER JOIN local_artifact artifact ON artifact.artifact_id = version.artifact_id WHERE artifact.source_session_id = ?1 AND artifact.source_project_id = ?2 AND version.source_project_id = ?2",
                    params![session_id, source_project_id],
                    |row| row.get(0),
                )
                .unwrap_or(-1),
            ArtifactRelationLayout::CreatorAndWriterSession => tx
                .query_row(
                    "SELECT COUNT(*) FROM local_artifact_version version INNER JOIN local_artifact artifact ON artifact.artifact_id = version.artifact_id WHERE artifact.creator_session_id = ?1 AND artifact.source_project_id = ?2 AND version.source_project_id = ?2 AND version.writer_session_id = ?1",
                    params![session_id, source_project_id],
                    |row| row.get(0),
                )
                .unwrap_or(-1),
        };
        if artifact_version_count < 0 {
            return AttachSessionsTransaction::Failed;
        }

        let changed_sessions = tx
            .execute(
                "UPDATE chat_session SET project_id = ?1 WHERE session_id = ?2 AND project_id = ?3",
                params![target_project_id, session_id, source_project_id],
            )
            .unwrap_or(0);
        let changed_session_projects = tx
            .execute(
                "UPDATE session_project SET project_id = ?1 WHERE session_id = ?2 AND project_id = ?3",
                params![target_project_id, session_id, source_project_id],
            )
            .unwrap_or(0);
        if changed_sessions != 1 || changed_session_projects != 1 {
            return AttachSessionsTransaction::Failed;
        }
        let changed_snapshots = tx
            .execute(
                "UPDATE snapshot SET project_id = ?1 WHERE chat_session_id = ?2 AND project_id = ?3",
                params![target_project_id, session_id, source_project_id],
            )
            .unwrap_or(usize::MAX);
        let changed_staging = tx
            .execute(
                "UPDATE staging SET project_id = ?1 WHERE chat_session_id = ?2 AND project_id = ?3",
                params![target_project_id, session_id, source_project_id],
            )
            .unwrap_or(usize::MAX);
        let artifact_session_column = artifact_layout.artifact_session_column();
        let changed_artifacts = tx
            .execute(
                &format!(
                    "UPDATE local_artifact SET source_project_id = ?1, user_id = ?2 WHERE {artifact_session_column} = ?3 AND source_project_id = ?4 AND user_id = ?5"
                ),
                params![target_project_id, target_user_id, session_id, source_project_id, source_user_id],
            )
            .unwrap_or(usize::MAX);
        let changed_artifact_versions = match artifact_layout {
            ArtifactRelationLayout::LegacySourceSession => tx
                .execute(
                    "UPDATE local_artifact_version SET source_project_id = ?1 WHERE source_project_id = ?2 AND artifact_id IN (SELECT artifact_id FROM local_artifact WHERE source_session_id = ?3 AND source_project_id = ?1 AND user_id = ?4)",
                    params![target_project_id, source_project_id, session_id, target_user_id],
                )
                .unwrap_or(usize::MAX),
            ArtifactRelationLayout::CreatorAndWriterSession => tx
                .execute(
                    "UPDATE local_artifact_version SET source_project_id = ?1 WHERE source_project_id = ?2 AND writer_session_id = ?3 AND artifact_id IN (SELECT artifact_id FROM local_artifact WHERE creator_session_id = ?3 AND source_project_id = ?1 AND user_id = ?4)",
                    params![target_project_id, source_project_id, session_id, target_user_id],
                )
                .unwrap_or(usize::MAX),
        };
        if changed_snapshots == usize::MAX
            || changed_staging == usize::MAX
            || changed_artifacts == usize::MAX
            || changed_artifact_versions == usize::MAX
            || changed_artifact_versions as i64 != artifact_version_count
        {
            return AttachSessionsTransaction::Failed;
        }
        affected_rows += (changed_sessions
            + changed_session_projects
            + changed_snapshots
            + changed_staging
            + changed_artifacts
            + changed_artifact_versions) as u64;
    }

    // 提交前最后一次证据和 sandbox 检查失败时，drop 事务保证目标零改动。
    if !before_commit() {
        return AttachSessionsTransaction::EvidenceChanged;
    }
    if tx.commit().is_err() {
        return AttachSessionsTransaction::Failed;
    }
    AttachSessionsTransaction::Committed(AttachSessionsCommit {
        affected_rows,
        source_remaining_sessions: source_session_count - selected_count,
        target_original_sessions: target_session_count,
        source_remaining_session_projects: source_relation_count - selected_count,
        target_original_session_projects: target_relation_count,
        message_count,
        protected_table_fingerprints,
    })
}

/// 提交后用新只读连接验证选中关系、未选数量、消息、FTS 和双层完整性。
fn verify_attach_sessions_after_commit(
    target_db_path: &Path,
    raw_key: &str,
    source_project_id: &str,
    target_project_id: &str,
    session_ids: &[traesync_domain::SessionIdentity],
    commit: &AttachSessionsCommit,
) -> bool {
    let conn = match open_with_key_readonly(target_db_path, raw_key) {
        Ok(conn) => conn,
        Err(_) => return false,
    };
    let Some(artifact_layout) = attach_relation_schema_layout(&conn) else {
        return false;
    };
    let target_user_id: String = match conn.query_row(
        "SELECT user_id FROM project WHERE project_id = ?1",
        params![target_project_id],
        |row| row.get::<_, String>(0),
    ) {
        Ok(user_id) if !user_id.is_empty() => user_id,
        _ => return false,
    };

    for session in session_ids {
        let session_id = session.original_session_id.as_str();
        let target_session_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM chat_session WHERE session_id = ?1 AND project_id = ?2",
                params![session_id, target_project_id],
                |row| row.get(0),
            )
            .unwrap_or(-1);
        let target_relation_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM session_project WHERE session_id = ?1 AND project_id = ?2",
                params![session_id, target_project_id],
                |row| row.get(0),
            )
            .unwrap_or(-1);
        let snapshot_mismatch_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM snapshot WHERE chat_session_id = ?1 AND project_id <> ?2",
                params![session_id, target_project_id],
                |row| row.get(0),
            )
            .unwrap_or(-1);
        let staging_mismatch_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM staging WHERE chat_session_id = ?1 AND project_id <> ?2",
                params![session_id, target_project_id],
                |row| row.get(0),
            )
            .unwrap_or(-1);
        let artifact_mismatch_count = artifact_mismatch_count(
            &conn,
            artifact_layout,
            session_id,
            target_project_id,
            &target_user_id,
        );
        let artifact_version_mismatch_count =
            artifact_version_mismatch_count(&conn, artifact_layout, session_id, target_project_id);
        if target_session_count != 1
            || target_relation_count != 1
            || snapshot_mismatch_count != 0
            || staging_mismatch_count != 0
            || artifact_mismatch_count != 0
            || artifact_version_mismatch_count != 0
        {
            return false;
        }
    }

    let selected_count = session_ids.len() as i64;
    let source_session_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM chat_session WHERE project_id = ?1",
            params![source_project_id],
            |row| row.get(0),
        )
        .unwrap_or(-1);
    let target_session_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM chat_session WHERE project_id = ?1",
            params![target_project_id],
            |row| row.get(0),
        )
        .unwrap_or(-1);
    let source_relation_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM session_project WHERE project_id = ?1",
            params![source_project_id],
            |row| row.get(0),
        )
        .unwrap_or(-1);
    let target_relation_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM session_project WHERE project_id = ?1",
            params![target_project_id],
            |row| row.get(0),
        )
        .unwrap_or(-1);
    let message_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM chat_message", [], |row| row.get(0))
        .unwrap_or(-1);
    let (cipher_ok, sqlite_ok) = run_integrity_checks_on_connection(&conn);

    source_session_count == commit.source_remaining_sessions
        && target_session_count == commit.target_original_sessions + selected_count
        && source_relation_count == commit.source_remaining_session_projects
        && target_relation_count == commit.target_original_session_projects + selected_count
        && message_count == commit.message_count
        && AttachSessionMutationScope::from_target(&conn, target_project_id, session_ids)
            .and_then(|scope| fingerprint_protected_tables(&conn, &scope))
            == Some(commit.protected_table_fingerprints.clone())
        && cipher_ok
        && sqlite_ok
}

impl AttachSessionMutationScope {
    /// 提交前从来源关系固定受许可的会话和 artifact ID，防止事务扩大更新范围。
    fn from_source(
        conn: &Connection,
        source_project_id: &str,
        source_user_id: &str,
        session_ids: &[traesync_domain::SessionIdentity],
    ) -> Option<Self> {
        let artifact_layout = attach_relation_schema_layout(conn)?;
        let selected_session_ids = selected_session_ids(session_ids)?;
        let selected_artifact_ids = artifact_ids_for_sessions(
            conn,
            source_project_id,
            source_user_id,
            &selected_session_ids,
            artifact_layout,
        )?;
        if !artifact_ids_are_unique(conn, &selected_artifact_ids) {
            return None;
        }
        Some(Self {
            selected_session_ids,
            selected_artifact_ids,
            artifact_layout,
        })
    }

    /// 提交后从目标关系重建同一许可范围，再与提交前摘要比较。
    fn from_target(
        conn: &Connection,
        target_project_id: &str,
        session_ids: &[traesync_domain::SessionIdentity],
    ) -> Option<Self> {
        let artifact_layout = attach_relation_schema_layout(conn)?;
        let selected_session_ids = selected_session_ids(session_ids)?;
        let target_user_id: String = conn
            .query_row(
                "SELECT user_id FROM project WHERE project_id = ?1",
                params![target_project_id],
                |row| row.get(0),
            )
            .ok()?;
        let selected_artifact_ids = artifact_ids_for_sessions(
            conn,
            target_project_id,
            &target_user_id,
            &selected_session_ids,
            artifact_layout,
        )?;
        if !artifact_ids_are_unique(conn, &selected_artifact_ids) {
            return None;
        }
        Some(Self {
            selected_session_ids,
            selected_artifact_ids,
            artifact_layout,
        })
    }

    /// 只有规格列出的关系单元格可归一化，其余字段、行和表必须逐字保持。
    fn allows_cell(
        &self,
        table: &str,
        columns: &[String],
        values: &[Vec<u8>],
        column: &str,
    ) -> bool {
        let text_at = |name: &str| {
            columns
                .iter()
                .position(|candidate| candidate == name)
                .and_then(|index| values.get(index))
                .and_then(|value| value.strip_prefix(b"t"))
                .and_then(|value| std::str::from_utf8(value).ok())
        };
        match (table, column) {
            ("chat_session" | "session_project", "project_id") => text_at("session_id")
                .is_some_and(|session_id| self.selected_session_ids.contains(session_id)),
            ("snapshot" | "staging", "project_id") => text_at("chat_session_id")
                .is_some_and(|session_id| self.selected_session_ids.contains(session_id)),
            ("local_artifact", "source_project_id" | "user_id") => {
                text_at(self.artifact_layout.artifact_session_column())
                    .is_some_and(|session_id| self.selected_session_ids.contains(session_id))
            }
            ("local_artifact_version", "source_project_id") => {
                let artifact_selected = text_at("artifact_id")
                    .is_some_and(|artifact_id| self.selected_artifact_ids.contains(artifact_id));
                artifact_selected
                    && (!self.artifact_layout.version_requires_writer_session()
                        || text_at("writer_session_id").is_some_and(|session_id| {
                            self.selected_session_ids.contains(session_id)
                        }))
            }
            _ => false,
        }
    }
}

/// 固化每个选中会话，拒绝空 ID、重复 ID 或非 Work CN 命名空间。
fn selected_session_ids(
    session_ids: &[traesync_domain::SessionIdentity],
) -> Option<BTreeSet<String>> {
    let mut selected_session_ids = BTreeSet::new();
    for session in session_ids {
        if session.product_history_namespace != "work_cn"
            || session.original_session_id.is_empty()
            || !selected_session_ids.insert(session.original_session_id.clone())
        {
            return None;
        }
    }
    Some(selected_session_ids)
}

/// 只收集计划会话当前归属的 artifact，避免版本表更新被同 ID 记录放大。
fn artifact_ids_for_sessions(
    conn: &Connection,
    project_id: &str,
    user_id: &str,
    session_ids: &BTreeSet<String>,
    artifact_layout: ArtifactRelationLayout,
) -> Option<BTreeSet<String>> {
    let mut artifact_ids = BTreeSet::new();
    for session_id in session_ids {
        let mut statement = conn
            .prepare(&format!(
                "SELECT artifact_id FROM local_artifact WHERE {} = ?1 AND source_project_id = ?2 AND user_id = ?3",
                artifact_layout.artifact_session_column()
            ))
            .ok()?;
        let rows = statement
            .query_map(params![session_id, project_id, user_id], |row| {
                row.get::<_, String>(0)
            })
            .ok()?;
        for artifact_id in rows {
            artifact_ids.insert(artifact_id.ok()?);
        }
    }
    Some(artifact_ids)
}

/// artifact ID 只能指向一条 artifact 记录，否则版本表更新会跨出选中会话范围。
fn artifact_ids_are_unique(conn: &Connection, artifact_ids: &BTreeSet<String>) -> bool {
    artifact_ids.iter().all(|artifact_id| {
        conn.query_row(
            "SELECT COUNT(*) FROM local_artifact WHERE artifact_id = ?1",
            params![artifact_id],
            |row| row.get::<_, i64>(0),
        )
        .is_ok_and(|count| count == 1)
    })
}

/// 读取选中会话的 artifact 后，确认不会共享到任何未选会话的 artifact 记录。
fn artifact_ids_are_unambiguous(
    conn: &Connection,
    project_id: &str,
    user_id: &str,
    session_ids: &BTreeSet<String>,
    artifact_layout: ArtifactRelationLayout,
) -> bool {
    artifact_ids_for_sessions(conn, project_id, user_id, session_ids, artifact_layout)
        .is_some_and(|artifact_ids| artifact_ids_are_unique(conn, &artifact_ids))
}

/// 遍历全部用户表，保证消息、正文、缓存、FTS 和非目标关系均未被事务或触发器改写。
fn fingerprint_protected_tables(
    conn: &Connection,
    mutation_scope: &AttachSessionMutationScope,
) -> Option<Vec<TableFingerprint>> {
    let mut statement = conn.prepare(
        "SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%' ORDER BY name",
    ).ok()?;
    let table_names = statement
        .query_map([], |row| row.get::<_, String>(0))
        .ok()?;
    let mut fingerprints = Vec::new();
    for table_name in table_names {
        let table_name = table_name.ok()?;
        fingerprints.push(fingerprint_table(conn, &table_name, mutation_scope)?);
    }
    Some(fingerprints)
}

/// 对单表按稳定顺序编码。优先 rowid；没有 rowid 时按全部可见列排序。
fn fingerprint_table(
    conn: &Connection,
    table_name: &str,
    mutation_scope: &AttachSessionMutationScope,
) -> Option<TableFingerprint> {
    let (mut statement, columns) = stable_table_select(conn, table_name)?;
    let mut rows = statement.query([]).ok()?;
    let mut row_count = 0_u64;
    let mut hasher = Sha256::new();
    hasher.update(table_name.as_bytes());
    hasher.update([0]);
    for column in &columns {
        hasher.update(column.as_bytes());
        hasher.update([0]);
    }

    while let Some(row) = rows.next().ok()? {
        let values = row_values(row, columns.len())?;
        for (index, column) in columns.iter().enumerate() {
            hasher.update(column.as_bytes());
            hasher.update([0]);
            if mutation_scope.allows_cell(table_name, &columns, &values, column) {
                hasher.update(b"allowed-relationship-change");
            } else {
                hasher.update((values[index].len() as u64).to_le_bytes());
                hasher.update(&values[index]);
            }
            hasher.update([0]);
        }
        row_count += 1;
    }

    Some(TableFingerprint {
        table_name: table_name.to_string(),
        row_count,
        content_hash: hex::encode(hasher.finalize()),
    })
}

/// 为受保护表生成稳定读取语句。
///
/// FTS 阴影表等 `WITHOUT ROWID` 表不能按 rowid 排序；此时用全部可见列排序，
/// 仍会把每个值编码进摘要。无法读取列或生成稳定顺序时返回 `None`，调用方拒绝写入。
fn stable_table_select<'connection>(
    conn: &'connection Connection,
    table_name: &str,
) -> Option<(rusqlite::Statement<'connection>, Vec<String>)> {
    let quoted_table_name = quote_sql_identifier(table_name);
    let rowid_query = format!("SELECT * FROM {quoted_table_name} ORDER BY rowid");
    if let Ok(statement) = conn.prepare(&rowid_query) {
        let columns = statement
            .column_names()
            .iter()
            .map(|column| (*column).to_string())
            .collect();
        return Some((statement, columns));
    }

    let plain_query = format!("SELECT * FROM {quoted_table_name}");
    let columns = {
        let statement = conn.prepare(&plain_query).ok()?;
        statement
            .column_names()
            .iter()
            .map(|column| (*column).to_string())
            .collect::<Vec<_>>()
    };
    if columns.is_empty() {
        return None;
    }
    let order_by = (1..=columns.len())
        .map(|index| index.to_string())
        .collect::<Vec<_>>()
        .join(", ");
    let all_columns_query = format!("SELECT * FROM {quoted_table_name} ORDER BY {order_by}");
    let statement = conn.prepare(&all_columns_query).ok()?;
    Some((statement, columns))
}

/// 将 SQLite 值编码为带类型标签的字节，避免文本、数字或 blob 间出现歧义碰撞。
fn row_values(row: &Row<'_>, column_count: usize) -> Option<Vec<Vec<u8>>> {
    (0..column_count)
        .map(|index| {
            let value = row.get_ref(index).ok()?;
            Some(match value {
                ValueRef::Null => vec![b'n'],
                ValueRef::Integer(value) => {
                    let mut bytes = vec![b'i'];
                    bytes.extend_from_slice(&value.to_le_bytes());
                    bytes
                }
                ValueRef::Real(value) => {
                    let mut bytes = vec![b'r'];
                    bytes.extend_from_slice(&value.to_bits().to_le_bytes());
                    bytes
                }
                ValueRef::Text(value) => {
                    let mut bytes = vec![b't'];
                    bytes.extend_from_slice(value);
                    bytes
                }
                ValueRef::Blob(value) => {
                    let mut bytes = vec![b'b'];
                    bytes.extend_from_slice(value);
                    bytes
                }
            })
        })
        .collect()
}

/// SQLite PRAGMA 和表计数只能插入标识符；此处通过双引号转义消除注入面。
fn quote_sql_identifier(identifier: &str) -> String {
    format!("\"{}\"", identifier.replace('"', "\"\""))
}

/// 读取表列集合；失败视为 schema 未被当前映射支持。
fn table_columns(conn: &Connection, table: &str) -> Option<BTreeSet<String>> {
    let query = format!("PRAGMA table_info({})", quote_sql_identifier(table));
    let mut statement = conn.prepare(&query).ok()?;
    let columns = statement
        .query_map([], |row| row.get::<_, String>(1))
        .ok()?;
    let mut result = BTreeSet::new();
    for column in columns {
        result.insert(column.ok()?);
    }
    Some(result)
}

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

    fn probe_database_with_validation(
        &self,
        db_path: &Path,
        raw_key: &str,
        is_authorized: &dyn Fn() -> bool,
    ) -> Option<CompatibilityState> {
        probe_database_inner_with_validation(db_path, raw_key, is_authorized)
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

/// R1：以只读 flags 打开隔离副本并设置 raw key。
///
/// `SQLITE_OPEN_READ_ONLY` 不会改写主库或 WAL，但 SQLite 仍可能更新已有 SHM。
/// 调用方必须先通过 `create_readonly_trio_copy` 隔离原始三件套。
///
/// raw_key 必须是 64 位 hex 字符串（32 字节）。
/// 使用 `PRAGMA key = "x'...'"` 语法，对应 TECHNICAL_BASELINE.md。
/// P3-1 起供 crate 内会话索引读取复用（跨账号实例库只读汇总）。
pub(crate) fn open_with_key_readonly(
    db_path: &Path,
    raw_key: &str,
) -> Result<ReadonlySqlCipherConnection, rusqlite::Error> {
    open_with_key_readonly_with_validation(db_path, raw_key, &|| true)?.ok_or_else(|| {
        rusqlite::Error::ToSqlConversionFailure(Box::new(std::io::Error::new(
            std::io::ErrorKind::Interrupted,
            "validation cancelled",
        )))
    })
}

/// W0 一致性探针入口：分阶段计时的只读打开。
///
/// 与 `open_with_key_readonly` 走完全相同的路径（分块复制三件套 → 只读 flags
/// → `PRAGMA key`），仅额外返回两段耗时（微秒）：三件套复制耗时与打开/设 key
/// 耗时，供历史页轮询定参探针区分大体积复制开销与解密读取开销。
/// key 不匹配不在本函数暴露——SQLCipher 延迟到首次查询才报
/// "file is not a database"，由探针按撕裂样本记录。
pub fn open_with_key_readonly_staged(
    db_path: &Path,
    raw_key: &str,
) -> Result<(ReadonlySqlCipherConnection, u64, u64), rusqlite::Error> {
    // 阶段一：三件套快照复制（生产同款分块复制；ReadonlyTrioCopy Drop 自清临时目录）
    let copy_started = Instant::now();
    let trio_copy = create_readonly_trio_copy(db_path)
        .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?;
    let copy_us = copy_started.elapsed().as_micros() as u64;

    // 阶段二：只读打开 + 设 key；flags 与 PRAGMA 语法镜像 R1 只读路径
    let open_started = Instant::now();
    let flags = OpenFlags::SQLITE_OPEN_READ_ONLY
        | OpenFlags::SQLITE_OPEN_PRIVATE_CACHE
        | OpenFlags::SQLITE_OPEN_NO_MUTEX;
    let connection = Connection::open_with_flags(&trio_copy.db_path, flags)?;
    let pragma = format!("PRAGMA key = \"x'{}'\";", raw_key);
    connection.execute_batch(&pragma)?;
    let open_us = open_started.elapsed().as_micros() as u64;

    Ok((
        ReadonlySqlCipherConnection {
            connection,
            _trio_copy: trio_copy,
        },
        copy_us,
        open_us,
    ))
}

/// 只读打开的可中止版本；撤销授权时不再继续复制或打开隔离副本。
fn open_with_key_readonly_with_validation(
    db_path: &Path,
    raw_key: &str,
    is_authorized: &dyn Fn() -> bool,
) -> Result<Option<ReadonlySqlCipherConnection>, rusqlite::Error> {
    let Some(trio_copy) = create_readonly_trio_copy_with_validation(db_path, is_authorized)
        .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?
    else {
        return Ok(None);
    };
    if !is_authorized() {
        return Ok(None);
    }
    // R1：只读 flags 仅作用于隔离副本。
    let flags = OpenFlags::SQLITE_OPEN_READ_ONLY
        | OpenFlags::SQLITE_OPEN_PRIVATE_CACHE
        | OpenFlags::SQLITE_OPEN_NO_MUTEX;
    let connection = Connection::open_with_flags(&trio_copy.db_path, flags)?;
    // raw key 语法：x'<hex>' —— 不进入日志
    let pragma = format!("PRAGMA key = \"x'{}'\";", raw_key);
    connection.execute_batch(&pragma)?;
    if !is_authorized() {
        return Ok(None);
    }
    Ok(Some(ReadonlySqlCipherConnection {
        connection,
        _trio_copy: trio_copy,
    }))
}

/// 为只读探测复制数据库及存在的 WAL/SHM 到独立临时目录。
///
/// 不使用 `immutable=1`，因为它会忽略 WAL，可能遗漏已提交但未 checkpoint 的记录。
fn create_readonly_trio_copy(db_path: &Path) -> std::io::Result<ReadonlyTrioCopy> {
    create_readonly_trio_copy_with_validation(db_path, &|| true)?
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::Interrupted, "validation cancelled"))
}

/// 创建隔离三件套的可中止版本；每个固定大小分块前后都复核授权。
fn create_readonly_trio_copy_with_validation(
    db_path: &Path,
    is_authorized: &dyn Fn() -> bool,
) -> std::io::Result<Option<ReadonlyTrioCopy>> {
    if !is_authorized() {
        return Ok(None);
    }
    let file_name = db_path.file_name().ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "数据库路径必须包含文件名")
    })?;
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let directory = std::env::temp_dir().join(format!(
        "trae-sync-readonly-{}-{}",
        std::process::id(),
        unique
    ));
    std::fs::create_dir(&directory)?;

    let copy = ReadonlyTrioCopy {
        db_path: directory.join(file_name),
        directory,
    };
    if !copy_file_with_validation(db_path, &copy.db_path, is_authorized)? {
        let _ = std::fs::remove_dir_all(&copy.directory);
        return Ok(None);
    }
    for suffix in ["-wal", "-shm"] {
        let source = PathBuf::from(format!("{}{}", db_path.display(), suffix));
        if source.exists() {
            let destination = PathBuf::from(format!("{}{}", copy.db_path.display(), suffix));
            if !copy_file_with_validation(&source, &destination, is_authorized)? {
                let _ = std::fs::remove_dir_all(&copy.directory);
                return Ok(None);
            }
        }
    }
    if !is_authorized() {
        let _ = std::fs::remove_dir_all(&copy.directory);
        return Ok(None);
    }
    Ok(Some(copy))
}

/// 分块复制隔离源文件，避免 `std::fs::copy` 在授权撤销后继续运行整文件复制。
fn copy_file_with_validation(
    source: &Path,
    destination: &Path,
    is_authorized: &dyn Fn() -> bool,
) -> std::io::Result<bool> {
    if !is_authorized() {
        return Ok(false);
    }
    let mut source_file = std::fs::File::open(source)?;
    let mut destination_file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination)?;
    let mut buffer = vec![0_u8; 1024 * 1024];
    loop {
        if !is_authorized() {
            return Ok(false);
        }
        let count = source_file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        destination_file.write_all(&buffer[..count])?;
        if !is_authorized() {
            return Ok(false);
        }
    }
    destination_file.sync_all()?;
    Ok(is_authorized())
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

/// 目录库完成记录只允许通过只读连接查询；不会因路径错误创建空数据库。
fn open_catalog_readonly_connection(
    db_path: &Path,
    raw_key: &str,
) -> Result<Connection, rusqlite::Error> {
    ensure_regular_catalog_file(db_path)?;
    reject_catalog_sidecars(db_path).map_err(|_| rusqlite::Error::InvalidQuery)?;
    let flags = OpenFlags::SQLITE_OPEN_READ_ONLY
        | OpenFlags::SQLITE_OPEN_PRIVATE_CACHE
        | OpenFlags::SQLITE_OPEN_NO_MUTEX;
    let connection = Connection::open_with_flags(db_path, flags)?;
    let pragma = format!("PRAGMA key = \"x'{}'\";", raw_key);
    connection.execute_batch(&pragma)?;
    Ok(connection)
}

/// 已有目录库写入入口：只允许 READ_WRITE，明确不带 CREATE。
fn open_catalog_readwrite_connection(
    db_path: &Path,
    raw_key: &str,
) -> Result<Connection, rusqlite::Error> {
    ensure_regular_catalog_file(db_path)?;
    reject_catalog_sidecars(db_path).map_err(|_| rusqlite::Error::InvalidQuery)?;
    let flags = OpenFlags::SQLITE_OPEN_READ_WRITE
        | OpenFlags::SQLITE_OPEN_PRIVATE_CACHE
        | OpenFlags::SQLITE_OPEN_NO_MUTEX;
    let connection = Connection::open_with_flags(db_path, flags)?;
    let pragma = format!("PRAGMA key = \"x'{}'\";", raw_key);
    connection.execute_batch(&pragma)?;
    verify_existing_catalog_write_protocol(&connection)
        .map_err(|_| rusqlite::Error::InvalidQuery)?;
    Ok(connection)
}

/// 目录库路径必须已存在且是普通文件；缺失时不得让 SQLite 创建新库。
fn ensure_regular_catalog_file(db_path: &Path) -> Result<(), rusqlite::Error> {
    let metadata = std::fs::symlink_metadata(db_path)
        .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(rusqlite::Error::InvalidQuery);
    }
    Ok(())
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

/// Gate A 已验证的 SQLCipher 4 参数；名称和查询均为固定白名单，不接受外部输入。
const REQUIRED_CIPHER_PRAGMAS: [(&str, &str, &str); 4] = [
    ("cipher_page_size", "PRAGMA cipher_page_size", "4096"),
    ("kdf_iter", "PRAGMA kdf_iter", "256000"),
    (
        "cipher_hmac_algorithm",
        "PRAGMA cipher_hmac_algorithm",
        "HMAC_SHA512",
    ),
    (
        "cipher_kdf_algorithm",
        "PRAGMA cipher_kdf_algorithm",
        "PBKDF2_HMAC_SHA512",
    ),
];

/// 探测数据库兼容性内部实现。
///
/// R1：先复制 DB/WAL/SHM，再用 `open_with_key_readonly` 探测副本，确保零写入。
/// R5：在解密成功后立即校验 `cipher_version`，与基线不兼容时返回
/// `CipherVersionMismatch` 并保持只读——避免后续 schema 检查在未知版本上误判。
fn probe_database_inner(db_path: &Path, raw_key: &str) -> CompatibilityState {
    probe_database_inner_with_validation(db_path, raw_key, &|| true).unwrap_or(
        CompatibilityState::Incompatible {
            reason: IncompatibleReason::TruncatedFile,
        },
    )
}

/// 数据库探测的可中止版本；所有可能触发源文件读取的阶段都复核授权。
fn probe_database_inner_with_validation(
    db_path: &Path,
    raw_key: &str,
    is_authorized: &dyn Fn() -> bool,
) -> Option<CompatibilityState> {
    if !is_authorized() {
        return None;
    }
    // 1. 截断文件检查：SQLite/SQLCipher 文件头至少 16 字节
    match std::fs::metadata(db_path) {
        Ok(meta) => {
            if meta.len() < 16 {
                return Some(CompatibilityState::Incompatible {
                    reason: IncompatibleReason::TruncatedFile,
                });
            }
        }
        Err(_) => {
            return Some(CompatibilityState::Incompatible {
                reason: IncompatibleReason::TruncatedFile,
            });
        }
    }

    // 2. R1：只把隔离副本交给 SQLite，避免只读 WAL 查询改写调用方 SHM。
    let conn = match open_with_key_readonly_with_validation(db_path, raw_key, is_authorized) {
        Ok(Some(c)) => c,
        Ok(None) => return None,
        Err(e) => {
            return Some(classify_open_error(&e));
        }
    };

    if !is_authorized() {
        return None;
    }

    // 3. 触发解密——读取 sqlite_master
    //    错误 key 在此阶段失败，错误消息含 "file is not a database" 或 "file is encrypted"
    let read_result: Result<Vec<(String, String)>, rusqlite::Error> = {
        let mut stmt =
            match conn.prepare("SELECT name, sql FROM sqlite_master WHERE type='table' LIMIT 1") {
                Ok(s) => s,
                Err(e) => return Some(classify_read_error(&e)),
            };
        let rows = stmt.query_map([], |row| {
            let name: String = row.get(0)?;
            let sql: String = row.get(1).unwrap_or_default();
            Ok((name, sql))
        });
        match rows {
            Ok(r) => r.collect::<Result<Vec<_>, _>>(),
            Err(e) => return Some(classify_read_error(&e)),
        }
    };
    if let Err(e) = read_result {
        return Some(classify_read_error(&e));
    }
    if !is_authorized() {
        return None;
    }

    // 4. R5：cipher_version 兼容检查（解密成功后立即执行）
    //    PRAGMA cipher_version 返回形如 "4.5.7 community" 或 "4.6.1 community"
    if let Err(reason) = check_cipher_version(&conn) {
        return Some(CompatibilityState::Incompatible { reason });
    }

    // 5. Gate A：版本兼容不能替代参数兼容；任一关键 PRAGMA 漂移都失败关闭。
    if let Err(reason) = check_cipher_pragmas(&conn) {
        return Some(CompatibilityState::Incompatible { reason });
    }

    // 6. schema 兼容性检查（表 -> 列 -> 唯一约束）
    if let Err(reason) = check_schema(&conn) {
        return Some(CompatibilityState::Incompatible { reason });
    }

    if !is_authorized() {
        return None;
    }

    // 7. 计算 schema 指纹与行数
    let schema_fingerprint = compute_schema_fingerprint(&conn);
    let counts = read_table_counts(&conn);

    Some(CompatibilityState::Verified {
        schema_fingerprint,
        counts,
    })
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

/// 校验 SQLCipher 4 的页大小、KDF 轮数和两个算法设置。
fn check_cipher_pragmas(conn: &Connection) -> Result<(), IncompatibleReason> {
    for (pragma, query, expected) in REQUIRED_CIPHER_PRAGMAS {
        let actual = read_pragma_scalar(conn, query).unwrap_or_else(|| "unavailable".to_string());
        if !actual.eq_ignore_ascii_case(expected) {
            return Err(IncompatibleReason::CipherPragmaMismatch {
                pragma: pragma.to_string(),
                expected: expected.to_string(),
                actual,
            });
        }
    }
    Ok(())
}

/// 把 PRAGMA 单值规范化为文本，兼容 SQLCipher 对数值设置返回 TEXT 的行为。
fn read_pragma_scalar(conn: &Connection, query: &str) -> Option<String> {
    conn.query_row(query, [], |row| {
        let value = match row.get_ref(0)? {
            ValueRef::Null => "null".to_string(),
            ValueRef::Integer(value) => value.to_string(),
            ValueRef::Real(value) => value.to_string(),
            ValueRef::Text(value) => String::from_utf8_lossy(value).into_owned(),
            ValueRef::Blob(value) => hex::encode(value),
        };
        Ok(value)
    })
    .ok()
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
/// rusqlite 的 `sqlite3_backup_*` 不支持加密库（"backup is not supported with encrypted databases"），
/// 使用 SQLCipher 官方推荐的 `sqlcipher_export()` 函数生成逻辑副本：
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

    // 导出必须在隔离副本上使用读写连接，避免 SQLite 改写调用方 SHM。
    let source_copy = create_readonly_trio_copy(source_db).ok()?;
    let conn = open_with_key(&source_copy.db_path, raw_key).ok()?;

    if export_sqlcipher_logical_copy(conn, raw_key, &dest_path) {
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
    let conn = match open_with_key_readonly(db_path, raw_key) {
        Ok(c) => c,
        Err(_) => return (false, false),
    };

    run_integrity_checks_on_connection(&conn)
}

/// 对已打开的只读连接运行两层完整性检查，供逻辑副本和提交后验证复用。
fn run_integrity_checks_on_connection(conn: &Connection) -> (bool, bool) {
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
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    };
    use tempfile::tempdir;

    /// 合成 fixture raw key（不使用 TECHNICAL_BASELINE.md 中的真实基线 key）。
    /// 仅供 fixture 测试：创建加密 fixture 并验证探测逻辑，不接触真实 TRAE 数据库。
    /// 真实基线 key 只存在于 docs/TECHNICAL_BASELINE.md，不进入源码、日志或证据。
    const TEST_RAW_KEY: &str = "deadbeefcafebabe1234567890abcdefdeadbeefcafebabe1234567890abcdef";

    /// 错误 key（与基线不同的 64 字符 hex）
    const WRONG_KEY: &str = "0000000000000000000000000000000000000000000000000000000000000000";

    /// 与 TRAE fixture raw key 不同的目录库 key，验证两个密钥边界确实独立。
    const TEST_CATALOG_KEY: &str =
        "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    fn ensure_catalog_initialized(
        storage_root: &Path,
        catalog_key: &str,
    ) -> Result<PathBuf, crate::CatalogPathError> {
        let lease = OperationLease::acquire_bound(storage_root, storage_root, "catalog-test-init")
            .expect("取得目录库测试初始化租约");
        crate::catalog::ensure_catalog_initialized(storage_root, catalog_key, storage_root, &lease)
    }

    fn reconcile_catalog_operation(
        storage_root: &Path,
        raw_key: &str,
        operation_id: &str,
        data_location_id: &str,
        affected_rows: u64,
        target_evidence: &TargetFileEvidence,
    ) -> bool {
        let lease =
            match OperationLease::acquire_bound(storage_root, storage_root, data_location_id) {
                Ok(lease) => lease,
                Err(_) => return false,
            };
        super::reconcile_catalog_operation(
            storage_root,
            raw_key,
            storage_root,
            &lease,
            operation_id,
            data_location_id,
            affected_rows,
            target_evidence,
        )
    }

    /// 计划驱动测试用证据读取器；通过调用序号模拟备份阶段之后的证据漂移。
    struct SequencedEvidence {
        current_through: usize,
        calls: AtomicUsize,
    }

    impl SyncPlanEvidencePort for SequencedEvidence {
        fn is_current(&self, _plan: &SyncPlan) -> bool {
            self.calls.fetch_add(1, Ordering::SeqCst) < self.current_through
        }
    }

    /// 仅控制提交后证据，验证批量动作不会跳过动作间的账号/schema 复核。
    struct AfterCommitSequenceEvidence {
        after_commit_current_through: usize,
        after_commit_calls: AtomicUsize,
    }

    impl SyncPlanEvidencePort for AfterCommitSequenceEvidence {
        fn is_current(&self, _plan: &SyncPlan) -> bool {
            true
        }

        fn is_current_after_commit(&self, _plan: &SyncPlan) -> bool {
            self.after_commit_calls.fetch_add(1, Ordering::SeqCst)
                < self.after_commit_current_through
        }
    }

    /// 构造与本 fixture 三件套绑定的单个 `FollowProject` 不可变计划。
    fn follow_project_plan(db_path: &Path) -> SyncPlan {
        let data_location_id = fixture_location_id(db_path);
        traesync_domain::build_sync_plan(traesync_domain::BuildSyncPlanInput {
            created_at: SystemTime::UNIX_EPOCH,
            platform_id: "work_cn".to_string(),
            data_location_id,
            current_user_id: "2000000000000002".to_string(),
            account_evidence_fingerprint: "fixture-account".to_string(),
            target_file_evidence: traesync_domain::TargetFileEvidence {
                db_fingerprint: sha256_file_for_backup(db_path).unwrap(),
                wal_fingerprint: sha256_file_for_backup(&database_sidecar_path(db_path, "-wal")),
                shm_fingerprint: sha256_file_for_backup(&database_sidecar_path(db_path, "-shm")),
            },
            schema_fingerprint: "fixture-schema".to_string(),
            mapping_version: "work_cn_v1".to_string(),
            schema_compatible: true,
            scope: traesync_domain::SyncScope::AllHistory,
            projects: vec![traesync_domain::PlanProjectInput {
                identity: traesync_domain::ProjectIdentity {
                    project_id: "p1".to_string(),
                    biz_project_id: "biz-1".to_string(),
                    display_name: "fixture project".to_string(),
                    soft_deleted: false,
                },
                display_owner: "1000000000000001".to_string(),
                current_live_owner: "1000000000000001".to_string(),
                sessions: vec![traesync_domain::PlanSessionInput {
                    identity: traesync_domain::SessionIdentity::new("work_cn", "s1"),
                    version_available: true,
                }],
                archived_only: false,
            }],
        })
    }

    /// 构造包含两个 `FollowProject` 动作的批量计划，验证批量收口而非只测单动作。
    fn multi_follow_project_plan(db_path: &Path) -> SyncPlan {
        let data_location_id = fixture_location_id(db_path);
        let project = |project_id: &str, biz_project_id: &str, session_id: &str| {
            traesync_domain::PlanProjectInput {
                identity: traesync_domain::ProjectIdentity {
                    project_id: project_id.to_string(),
                    biz_project_id: biz_project_id.to_string(),
                    display_name: format!("fixture {project_id}"),
                    soft_deleted: false,
                },
                display_owner: "1000000000000001".to_string(),
                current_live_owner: "1000000000000001".to_string(),
                sessions: vec![traesync_domain::PlanSessionInput {
                    identity: traesync_domain::SessionIdentity::new("work_cn", session_id),
                    version_available: true,
                }],
                archived_only: false,
            }
        };
        traesync_domain::build_sync_plan(traesync_domain::BuildSyncPlanInput {
            created_at: SystemTime::UNIX_EPOCH,
            platform_id: "work_cn".to_string(),
            data_location_id,
            current_user_id: "2000000000000002".to_string(),
            account_evidence_fingerprint: "fixture-account".to_string(),
            target_file_evidence: traesync_domain::TargetFileEvidence {
                db_fingerprint: sha256_file_for_backup(db_path).unwrap(),
                wal_fingerprint: sha256_file_for_backup(&database_sidecar_path(db_path, "-wal")),
                shm_fingerprint: sha256_file_for_backup(&database_sidecar_path(db_path, "-shm")),
            },
            schema_fingerprint: "fixture-schema".to_string(),
            mapping_version: "work_cn_v1".to_string(),
            schema_compatible: true,
            scope: traesync_domain::SyncScope::AllHistory,
            projects: vec![project("p1", "biz-1", "s1"), project("p2", "biz-2", "s2")],
        })
    }

    /// 构造仅把选中会话挂到目标已存在项目的不可变计划。
    fn attach_sessions_plan(db_path: &Path, selected_session_ids: &[&str]) -> SyncPlan {
        let data_location_id = fixture_location_id(db_path);
        traesync_domain::build_sync_plan(traesync_domain::BuildSyncPlanInput {
            created_at: SystemTime::UNIX_EPOCH,
            platform_id: "work_cn".to_string(),
            data_location_id,
            current_user_id: "2000000000000002".to_string(),
            account_evidence_fingerprint: "fixture-account".to_string(),
            target_file_evidence: traesync_domain::TargetFileEvidence {
                db_fingerprint: sha256_file_for_backup(db_path).unwrap(),
                wal_fingerprint: sha256_file_for_backup(&database_sidecar_path(db_path, "-wal")),
                shm_fingerprint: sha256_file_for_backup(&database_sidecar_path(db_path, "-shm")),
            },
            schema_fingerprint: "fixture-schema".to_string(),
            mapping_version: "work_cn_v1".to_string(),
            schema_compatible: true,
            scope: traesync_domain::SyncScope::Custom {
                account_ids: vec![],
                project_ids: vec![],
                session_ids: selected_session_ids
                    .iter()
                    .map(|session_id| traesync_domain::SessionIdentity::new("work_cn", session_id))
                    .collect(),
            },
            projects: vec![
                traesync_domain::PlanProjectInput {
                    identity: traesync_domain::ProjectIdentity {
                        project_id: "p-source".to_string(),
                        biz_project_id: "biz-shared".to_string(),
                        display_name: "source fixture project".to_string(),
                        soft_deleted: false,
                    },
                    display_owner: "1000000000000001".to_string(),
                    current_live_owner: "1000000000000001".to_string(),
                    sessions: vec![
                        traesync_domain::PlanSessionInput {
                            identity: traesync_domain::SessionIdentity::new("work_cn", "s1"),
                            version_available: true,
                        },
                        traesync_domain::PlanSessionInput {
                            identity: traesync_domain::SessionIdentity::new("work_cn", "s2"),
                            version_available: true,
                        },
                    ],
                    archived_only: false,
                },
                traesync_domain::PlanProjectInput {
                    identity: traesync_domain::ProjectIdentity {
                        project_id: "p-target".to_string(),
                        biz_project_id: "biz-shared".to_string(),
                        display_name: "target fixture project".to_string(),
                        soft_deleted: false,
                    },
                    display_owner: "2000000000000002".to_string(),
                    current_live_owner: "2000000000000002".to_string(),
                    sessions: vec![traesync_domain::PlanSessionInput {
                        identity: traesync_domain::SessionIdentity::new(
                            "work_cn",
                            "target-session",
                        ),
                        version_available: true,
                    }],
                    archived_only: false,
                },
            ],
        })
    }

    /// 构造同时包含项目跟随和会话重挂的批量计划，覆盖混合动作写后校验。
    fn mixed_follow_attach_plan(db_path: &Path) -> SyncPlan {
        let data_location_id = fixture_location_id(db_path);
        let session = |session_id: &str| traesync_domain::PlanSessionInput {
            identity: traesync_domain::SessionIdentity::new("work_cn", session_id),
            version_available: true,
        };
        traesync_domain::build_sync_plan(traesync_domain::BuildSyncPlanInput {
            created_at: SystemTime::UNIX_EPOCH,
            platform_id: "work_cn".to_string(),
            data_location_id,
            current_user_id: "2000000000000002".to_string(),
            account_evidence_fingerprint: "fixture-account".to_string(),
            target_file_evidence: traesync_domain::TargetFileEvidence {
                db_fingerprint: sha256_file_for_backup(db_path).unwrap(),
                wal_fingerprint: sha256_file_for_backup(&database_sidecar_path(db_path, "-wal")),
                shm_fingerprint: sha256_file_for_backup(&database_sidecar_path(db_path, "-shm")),
            },
            schema_fingerprint: "fixture-schema".to_string(),
            mapping_version: "work_cn_v1".to_string(),
            schema_compatible: true,
            scope: traesync_domain::SyncScope::Custom {
                account_ids: vec![],
                project_ids: vec![],
                session_ids: vec![
                    traesync_domain::SessionIdentity::new("work_cn", "follow-s"),
                    traesync_domain::SessionIdentity::new("work_cn", "s1"),
                ],
            },
            projects: vec![
                traesync_domain::PlanProjectInput {
                    identity: traesync_domain::ProjectIdentity {
                        project_id: "p-follow".to_string(),
                        biz_project_id: "biz-follow".to_string(),
                        display_name: "follow fixture project".to_string(),
                        soft_deleted: false,
                    },
                    display_owner: "1000000000000001".to_string(),
                    current_live_owner: "1000000000000001".to_string(),
                    sessions: vec![session("follow-s")],
                    archived_only: false,
                },
                traesync_domain::PlanProjectInput {
                    identity: traesync_domain::ProjectIdentity {
                        project_id: "p-source".to_string(),
                        biz_project_id: "biz-shared".to_string(),
                        display_name: "source fixture project".to_string(),
                        soft_deleted: false,
                    },
                    display_owner: "1000000000000001".to_string(),
                    current_live_owner: "1000000000000001".to_string(),
                    sessions: vec![session("s1"), session("s2")],
                    archived_only: false,
                },
                traesync_domain::PlanProjectInput {
                    identity: traesync_domain::ProjectIdentity {
                        project_id: "p-target".to_string(),
                        biz_project_id: "biz-shared".to_string(),
                        display_name: "target fixture project".to_string(),
                        soft_deleted: false,
                    },
                    display_owner: "2000000000000002".to_string(),
                    current_live_owner: "2000000000000002".to_string(),
                    sessions: vec![session("target-session")],
                    archived_only: false,
                },
            ],
        })
    }

    /// 测试仅直接构造私有执行核心；生产只能通过 `bind_fixture` 创建执行器。
    fn fixture_executor<'a>(
        executor: &'a WorkCnSyncExecutor,
        db_path: &Path,
        storage_root: &Path,
    ) -> FixtureWorkCnSyncExecutor<'a> {
        make_catalog_fixture(storage_root);
        fixture_executor_after_catalog_init(executor, db_path, storage_root)
    }

    /// 使用显式目录库 key 构造 fixture 执行器，覆盖源库/目录库密钥分离路径。
    fn fixture_executor_with_catalog_key<'a>(
        executor: &'a WorkCnSyncExecutor,
        db_path: &Path,
        storage_root: &Path,
        catalog_key: &str,
    ) -> FixtureWorkCnSyncExecutor<'a> {
        make_catalog_fixture_with_key(storage_root, catalog_key);
        fixture_executor_after_catalog_init(executor, db_path, storage_root)
    }

    fn fixture_executor_after_catalog_init<'a>(
        executor: &'a WorkCnSyncExecutor,
        db_path: &Path,
        storage_root: &Path,
    ) -> FixtureWorkCnSyncExecutor<'a> {
        let location_witness = fixture_location_witness(db_path);
        let operation_lease = OperationLease::acquire_bound(
            storage_root,
            storage_root,
            &location_witness.data_location_id,
        )
        .expect("取得执行器测试租约");
        FixtureWorkCnSyncExecutor {
            executor,
            fixture_root: PathBuf::from(&location_witness.canonical_root),
            target_db_path: db_path.canonicalize().unwrap(),
            location_witness,
            storage_root: storage_root.to_path_buf(),
            recovery_root: storage_root.to_path_buf(),
            operation_lease,
        }
    }

    fn fixture_location_witness(db_path: &Path) -> LocationWitness {
        let root = db_path.parent().expect("fixture DB 必须有父目录");
        let relative_path = db_path
            .file_name()
            .and_then(|name| name.to_str())
            .expect("fixture DB 必须有有效文件名");
        capture_location_identity(&PlatformFileIdentityProvider::new(), root, relative_path)
            .expect("fixture 位置身份见证失败")
    }

    fn fixture_location_id(db_path: &Path) -> String {
        fixture_location_witness(db_path).data_location_id
    }

    /// 为执行器测试建立最小 SQLCipher 目录库，覆盖 manifest 的目录库协调阶段。
    fn make_catalog_fixture(storage_root: &Path) {
        make_catalog_fixture_with_key(storage_root, TEST_RAW_KEY);
    }

    fn make_catalog_fixture_with_key(storage_root: &Path, catalog_key: &str) {
        let catalog_path = ensure_catalog_initialized(storage_root, catalog_key).unwrap();
        let connection = Connection::open(catalog_path).unwrap();
        connection
            .execute_batch(&format!("PRAGMA key = \"x'{}'\";", catalog_key))
            .unwrap();
        connection
            .execute_batch(
                "CREATE TABLE IF NOT EXISTS operation_record (
                     operation_id TEXT PRIMARY KEY,
                     data_location_id TEXT NOT NULL,
                     state TEXT NOT NULL,
                     affected_rows INTEGER NOT NULL,
                     db_fingerprint TEXT NOT NULL,
                     wal_fingerprint TEXT,
                     shm_fingerprint TEXT,
                     updated_at INTEGER NOT NULL
                 );",
            )
            .unwrap();
    }

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

    /// 构造两个可独立跟随的项目，保持 schema 与单项目 fixture 一致。
    fn make_multi_follow_fixture(dir: &Path) -> PathBuf {
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
            INSERT INTO project VALUES ('p2', '1000000000000001', 'biz-2');
            INSERT INTO chat_session VALUES ('s1', 'p1');
            INSERT INTO chat_session VALUES ('s2', 'p2');
            INSERT INTO chat_message VALUES ('m1', 's1');
            INSERT INTO chat_message VALUES ('m2', 's2');
            "#,
        )
        .unwrap();
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
                project_id TEXT NOT NULL,
                deleted_at INTEGER NOT NULL DEFAULT 0
            );
            CREATE TABLE chat_message (
                message_id TEXT PRIMARY KEY,
                session_id TEXT NOT NULL,
                deleted_at INTEGER NOT NULL DEFAULT 0
            );
            INSERT INTO project VALUES ('p1', '1000000000000001', 'biz-1');
            INSERT INTO chat_session VALUES ('s1', 'p1', 1700000001);
            INSERT INTO chat_message VALUES ('m1', 's1', 1700000002);
            "#,
        )
        .unwrap();

        // 关闭最后一个连接会清理 sidecar，先冻结有效三件套供只读回归使用。
        let db_bytes = std::fs::read(&db_path).unwrap();
        let wal_path = dir.join("database.db-wal");
        let shm_path = dir.join("database.db-shm");
        let wal_bytes = std::fs::read(&wal_path).unwrap();
        let shm_bytes = std::fs::read(&shm_path).unwrap();
        drop(conn);

        std::fs::write(&db_path, db_bytes).unwrap();
        std::fs::write(wal_path, wal_bytes).unwrap();
        std::fs::write(shm_path, shm_bytes).unwrap();
        db_path
    }

    /// 构造包含全部受限关系的会话重挂 fixture，并提供目标项目 sandbox。
    fn make_attach_sessions_fixture(dir: &Path) -> PathBuf {
        std::fs::create_dir_all(dir.join("sandbox")).unwrap();
        std::fs::write(dir.join("sandbox").join("p-target.json"), b"{}").unwrap();

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
                session_id TEXT NOT NULL,
                body TEXT NOT NULL
            );
            CREATE TABLE session_project (
                session_id TEXT PRIMARY KEY,
                project_id TEXT NOT NULL
            );
            CREATE TABLE snapshot (
                snapshot_id TEXT PRIMARY KEY,
                chat_session_id TEXT NOT NULL,
                project_id TEXT NOT NULL
            );
            CREATE TABLE staging (
                staging_id TEXT PRIMARY KEY,
                chat_session_id TEXT NOT NULL,
                project_id TEXT NOT NULL
            );
            CREATE TABLE local_artifact (
                artifact_id TEXT PRIMARY KEY,
                source_session_id TEXT NOT NULL,
                source_project_id TEXT NOT NULL,
                user_id TEXT NOT NULL
            );
            CREATE TABLE local_artifact_version (
                version_id TEXT PRIMARY KEY,
                artifact_id TEXT NOT NULL,
                source_project_id TEXT NOT NULL
            );
            CREATE TABLE chat_fts (
                session_id TEXT PRIMARY KEY,
                indexed_body TEXT NOT NULL
            );
            INSERT INTO project VALUES ('p-source', '1000000000000001', 'biz-shared');
            INSERT INTO project VALUES ('p-target', '2000000000000002', 'biz-shared');
            INSERT INTO chat_session VALUES ('s1', 'p-source');
            INSERT INTO chat_session VALUES ('s2', 'p-source');
            INSERT INTO chat_session VALUES ('target-session', 'p-target');
            INSERT INTO chat_message VALUES ('m1', 's1', 'source one body');
            INSERT INTO chat_message VALUES ('m2', 's2', 'source two body');
            INSERT INTO chat_message VALUES ('m3', 'target-session', 'target body');
            INSERT INTO session_project VALUES ('s1', 'p-source');
            INSERT INTO session_project VALUES ('s2', 'p-source');
            INSERT INTO session_project VALUES ('target-session', 'p-target');
            INSERT INTO snapshot VALUES ('snapshot-1', 's1', 'p-source');
            INSERT INTO snapshot VALUES ('snapshot-2', 's2', 'p-source');
            INSERT INTO staging VALUES ('staging-1', 's1', 'p-source');
            INSERT INTO staging VALUES ('staging-2', 's2', 'p-source');
            INSERT INTO local_artifact VALUES ('artifact-1', 's1', 'p-source', '1000000000000001');
            INSERT INTO local_artifact VALUES ('artifact-2', 's2', 'p-source', '1000000000000001');
            INSERT INTO local_artifact_version VALUES ('artifact-version-1', 'artifact-1', 'p-source');
            INSERT INTO local_artifact_version VALUES ('artifact-version-2', 'artifact-2', 'p-source');
            INSERT INTO chat_fts VALUES ('s1', 'source one body');
            INSERT INTO chat_fts VALUES ('s2', 'source two body');
            INSERT INTO chat_fts VALUES ('target-session', 'target body');
            "#,
        )
        .unwrap();
        conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")
            .unwrap();
        drop(conn);
        db_path
    }

    /// 在会话重挂 fixture 上追加一个仅用于 FollowProject 的独立项目。
    fn make_mixed_follow_attach_fixture(dir: &Path) -> PathBuf {
        let db_path = make_attach_sessions_fixture(dir);
        let conn = open_with_key(&db_path, TEST_RAW_KEY).unwrap();
        conn.execute_batch(
            "INSERT INTO project VALUES ('p-follow', '1000000000000001', 'biz-follow');\
             INSERT INTO chat_session VALUES ('follow-s', 'p-follow');\
             INSERT INTO chat_message VALUES ('m-follow', 'follow-s', 'follow body');\
             PRAGMA wal_checkpoint(TRUNCATE);",
        )
        .unwrap();
        drop(conn);
        db_path
    }

    /// 构造与真实 TRAE artifact 关系列一致的 fixture，验证映射不依赖旧测试列名。
    fn make_current_attach_sessions_fixture(dir: &Path) -> PathBuf {
        let db_path = make_attach_sessions_fixture(dir);
        let conn = open_with_key(&db_path, TEST_RAW_KEY).unwrap();
        conn.execute_batch(
            r#"
            ALTER TABLE local_artifact RENAME TO local_artifact_legacy;
            CREATE TABLE local_artifact (
                id TEXT PRIMARY KEY,
                artifact_id TEXT NOT NULL,
                user_id TEXT NOT NULL,
                source_project_id TEXT NOT NULL,
                creator_session_id TEXT NOT NULL,
                creator_turn_id TEXT NOT NULL,
                display_name TEXT NOT NULL
            );
            INSERT INTO local_artifact
                SELECT
                    'artifact-row-' || artifact_id,
                    artifact_id,
                    user_id,
                    source_project_id,
                    source_session_id,
                    'turn-' || source_session_id,
                    artifact_id
                FROM local_artifact_legacy;
            DROP TABLE local_artifact_legacy;

            ALTER TABLE local_artifact_version RENAME TO local_artifact_version_legacy;
            CREATE TABLE local_artifact_version (
                id TEXT PRIMARY KEY,
                artifact_id TEXT NOT NULL,
                source_project_id TEXT NOT NULL,
                writer_session_id TEXT NOT NULL,
                writer_turn_id TEXT NOT NULL,
                version INTEGER NOT NULL
            );
            INSERT INTO local_artifact_version
                SELECT
                    'artifact-version-row-' || legacy.version_id,
                    legacy.artifact_id,
                    legacy.source_project_id,
                    artifact.creator_session_id,
                    'turn-' || artifact.creator_session_id,
                    1
                FROM local_artifact_version_legacy legacy
                JOIN local_artifact artifact ON artifact.artifact_id = legacy.artifact_id;
            DROP TABLE local_artifact_version_legacy;

            ALTER TABLE chat_session ADD COLUMN session_type TEXT NOT NULL DEFAULT 'chat';
            CREATE TABLE history_v2 (session_id TEXT PRIMARY KEY, payload TEXT NOT NULL);
            INSERT INTO history_v2 VALUES ('s1', 'source history');
            INSERT INTO history_v2 VALUES ('s2', 'other source history');
            "#,
        )
        .unwrap();
        drop(conn);
        db_path
    }

    #[test]
    fn attach_sessions_updates_only_selected_relationships() {
        let target = tempdir().unwrap();
        let storage = tempdir().unwrap();
        let db_path = make_attach_sessions_fixture(target.path());
        let plan = attach_sessions_plan(&db_path, &["s1"]);
        let executor = WorkCnSyncExecutor::new(TEST_RAW_KEY);
        let evidence = SequencedEvidence {
            current_through: usize::MAX,
            calls: AtomicUsize::new(0),
        };

        assert!(matches!(
            plan.actions(),
            [PlanAction::AttachSessions { source_project_id, target_project_id, session_ids }]
                if source_project_id == "p-source"
                    && target_project_id == "p-target"
                    && session_ids == &[traesync_domain::SessionIdentity::new("work_cn", "s1")]
        ));

        let outcome = fixture_executor(&executor, &db_path, storage.path()).execute_sync_plan(
            &plan,
            &OperationCancellation::new(),
            &evidence,
        );

        assert!(matches!(
            outcome,
            SyncPlanExecutionOutcome::Completed { affected_rows } if affected_rows >= 6
        ));
        assert_eq!(
            table_value(&db_path, "chat_session", "project_id", "session_id", "s1"),
            Some("p-target".to_string())
        );
        assert_eq!(
            table_value(
                &db_path,
                "session_project",
                "project_id",
                "session_id",
                "s1"
            ),
            Some("p-target".to_string())
        );
        assert_eq!(
            table_value(&db_path, "snapshot", "project_id", "chat_session_id", "s1"),
            Some("p-target".to_string())
        );
        assert_eq!(
            table_value(&db_path, "staging", "project_id", "chat_session_id", "s1"),
            Some("p-target".to_string())
        );
        assert_eq!(
            table_value(
                &db_path,
                "local_artifact",
                "source_project_id",
                "artifact_id",
                "artifact-1"
            ),
            Some("p-target".to_string())
        );
        assert_eq!(
            table_value(
                &db_path,
                "local_artifact",
                "user_id",
                "artifact_id",
                "artifact-1"
            ),
            Some("2000000000000002".to_string())
        );
        assert_eq!(
            table_value(
                &db_path,
                "local_artifact_version",
                "source_project_id",
                "artifact_id",
                "artifact-1"
            ),
            Some("p-target".to_string())
        );
        assert_eq!(
            table_value(&db_path, "chat_session", "project_id", "session_id", "s2"),
            Some("p-source".to_string())
        );
        assert_eq!(
            table_value(
                &db_path,
                "local_artifact",
                "source_project_id",
                "artifact_id",
                "artifact-2"
            ),
            Some("p-source".to_string())
        );
        assert_eq!(
            table_value(&db_path, "chat_message", "body", "message_id", "m1"),
            Some("source one body".to_string())
        );
        assert_eq!(
            table_value(&db_path, "chat_fts", "indexed_body", "session_id", "s1"),
            Some("source one body".to_string())
        );
    }

    #[test]
    fn multi_action_follow_project_executes_all_actions_and_keeps_dual_backup() {
        let target = tempdir().unwrap();
        let storage = tempdir().unwrap();
        let db_path = make_multi_follow_fixture(target.path());
        let plan = multi_follow_project_plan(&db_path);
        assert_eq!(plan.actions().len(), 2);

        let outcome = fixture_executor(
            &WorkCnSyncExecutor::new(TEST_RAW_KEY),
            &db_path,
            storage.path(),
        )
        .execute_sync_plan(
            &plan,
            &OperationCancellation::new(),
            &SequencedEvidence {
                current_through: usize::MAX,
                calls: AtomicUsize::new(0),
            },
        );
        assert!(matches!(
            outcome,
            SyncPlanExecutionOutcome::Completed { affected_rows } if affected_rows == 2
        ));
        assert_eq!(
            table_value(&db_path, "project", "user_id", "project_id", "p1"),
            Some("2000000000000002".to_string())
        );
        assert_eq!(
            table_value(&db_path, "project", "user_id", "project_id", "p2"),
            Some("2000000000000002".to_string())
        );

        let backup_root = storage
            .path()
            .join("backups")
            .join(plan.operation_id().as_str());
        assert!(verify_raw_backup(&backup_root.join("before").join("raw")));
        assert!(verify_logical_backup(
            &backup_root
                .join("before")
                .join("logical")
                .join("database.db"),
            TEST_RAW_KEY
        ));
        assert!(storage
            .path()
            .join("operations")
            .join(plan.operation_id().as_str())
            .is_dir());
    }

    #[test]
    fn multi_action_stops_before_next_action_when_post_commit_evidence_drifts() {
        let target = tempdir().unwrap();
        let storage = tempdir().unwrap();
        let db_path = make_multi_follow_fixture(target.path());
        let plan = multi_follow_project_plan(&db_path);

        let outcome = fixture_executor(
            &WorkCnSyncExecutor::new(TEST_RAW_KEY),
            &db_path,
            storage.path(),
        )
        .execute_sync_plan(
            &plan,
            &OperationCancellation::new(),
            &AfterCommitSequenceEvidence {
                // 第一个动作提交后仍有效；进入第二个动作前模拟账号/schema 证据漂移。
                after_commit_current_through: 1,
                after_commit_calls: AtomicUsize::new(0),
            },
        );
        assert!(matches!(
            outcome,
            SyncPlanExecutionOutcome::ManualRecoveryRequired {
                backups_preserved: true
            }
        ));
        assert_eq!(
            table_value(&db_path, "project", "user_id", "project_id", "p1"),
            Some("2000000000000002".to_string())
        );
        assert_eq!(
            table_value(&db_path, "project", "user_id", "project_id", "p2"),
            Some("1000000000000001".to_string())
        );
    }

    #[test]
    fn mixed_follow_project_and_attach_sessions_verify_full_relations() {
        let target = tempdir().unwrap();
        let storage = tempdir().unwrap();
        let db_path = make_mixed_follow_attach_fixture(target.path());
        let plan = mixed_follow_attach_plan(&db_path);

        // 混合计划必须保留动作顺序：先移动独立项目，再重挂目标项目中的会话。
        assert_eq!(plan.actions().len(), 2);
        assert!(matches!(
            &plan.actions()[0],
            PlanAction::FollowProject { project_id, .. } if project_id == "p-follow"
        ));
        assert!(matches!(
            &plan.actions()[1],
            PlanAction::AttachSessions { source_project_id, target_project_id, session_ids }
                if source_project_id == "p-source"
                    && target_project_id == "p-target"
                    && session_ids == &[traesync_domain::SessionIdentity::new("work_cn", "s1")]
        ));

        let outcome = fixture_executor(
            &WorkCnSyncExecutor::new(TEST_RAW_KEY),
            &db_path,
            storage.path(),
        )
        .execute_sync_plan(
            &plan,
            &OperationCancellation::new(),
            &SequencedEvidence {
                current_through: usize::MAX,
                calls: AtomicUsize::new(0),
            },
        );
        assert!(matches!(
            outcome,
            SyncPlanExecutionOutcome::Completed { affected_rows } if affected_rows == 7
        ));

        // FollowProject 的项目归属必须切换到当前账号。
        assert_eq!(
            table_value(&db_path, "project", "user_id", "project_id", "p-follow"),
            Some("2000000000000002".to_string())
        );

        // AttachSessions 只能影响选中的 s1，s2 保持来源项目归属。
        assert_eq!(
            table_value(&db_path, "chat_session", "project_id", "session_id", "s1"),
            Some("p-target".to_string())
        );
        assert_eq!(
            table_value(&db_path, "chat_session", "project_id", "session_id", "s2"),
            Some("p-source".to_string())
        );
        assert_eq!(
            table_value(
                &db_path,
                "session_project",
                "project_id",
                "session_id",
                "s1"
            ),
            Some("p-target".to_string())
        );
        assert_eq!(
            table_value(&db_path, "snapshot", "project_id", "chat_session_id", "s1"),
            Some("p-target".to_string())
        );
        assert_eq!(
            table_value(&db_path, "staging", "project_id", "chat_session_id", "s1"),
            Some("p-target".to_string())
        );
        assert_eq!(
            table_value(
                &db_path,
                "local_artifact",
                "source_project_id",
                "artifact_id",
                "artifact-1"
            ),
            Some("p-target".to_string())
        );
        assert_eq!(
            table_value(
                &db_path,
                "local_artifact",
                "user_id",
                "artifact_id",
                "artifact-1"
            ),
            Some("2000000000000002".to_string())
        );
        assert_eq!(
            table_value(
                &db_path,
                "local_artifact_version",
                "source_project_id",
                "artifact_id",
                "artifact-1"
            ),
            Some("p-target".to_string())
        );
        assert_eq!(
            table_value(&db_path, "chat_fts", "indexed_body", "session_id", "s1"),
            Some("source one body".to_string())
        );
    }

    #[test]
    fn attach_sessions_allows_its_own_wal_change_after_prewrite_evidence_check() {
        let target = tempdir().unwrap();
        let storage = tempdir().unwrap();
        let db_path = make_attach_sessions_fixture(target.path());
        let conn = open_with_key(&db_path, TEST_RAW_KEY).unwrap();
        // 保持独立连接打开，确保计划固定的 WAL/SHM 证据存在直到执行器进入事务。
        conn.execute_batch(
            "PRAGMA journal_mode = WAL; \
             CREATE TABLE wal_fixture_cache (cache_key TEXT PRIMARY KEY, value TEXT NOT NULL); \
             INSERT INTO wal_fixture_cache VALUES ('cache-1', 'unchanged');",
        )
        .unwrap();
        assert!(database_sidecar_path(&db_path, "-wal").is_file());
        assert!(database_sidecar_path(&db_path, "-shm").is_file());
        let plan = attach_sessions_plan(&db_path, &["s1"]);

        let outcome = fixture_executor(
            &WorkCnSyncExecutor::new(TEST_RAW_KEY),
            &db_path,
            storage.path(),
        )
        .execute_sync_plan(
            &plan,
            &OperationCancellation::new(),
            &SequencedEvidence {
                current_through: usize::MAX,
                calls: AtomicUsize::new(0),
            },
        );

        assert!(matches!(
            outcome,
            SyncPlanExecutionOutcome::Completed { .. }
        ));
        drop(conn);
    }

    #[test]
    fn attach_sessions_allows_lock_created_wal_when_plan_had_none() {
        let target = tempdir().unwrap();
        let storage = tempdir().unwrap();
        let db_path = make_attach_sessions_fixture(target.path());
        let conn = open_with_key(&db_path, TEST_RAW_KEY).unwrap();
        // WAL 模式写入数据库头；关闭后移除 sidecar，模拟真实库计划生成时没有 WAL 的状态。
        conn.execute_batch("PRAGMA journal_mode = WAL;").unwrap();
        drop(conn);
        for suffix in ["-wal", "-shm"] {
            let sidecar = database_sidecar_path(&db_path, suffix);
            if sidecar.exists() {
                std::fs::remove_file(sidecar).unwrap();
            }
        }
        let plan = attach_sessions_plan(&db_path, &["s1"]);
        assert!(plan.target_file_evidence().wal_fingerprint.is_none());

        let outcome = fixture_executor(
            &WorkCnSyncExecutor::new(TEST_RAW_KEY),
            &db_path,
            storage.path(),
        )
        .execute_sync_plan(
            &plan,
            &OperationCancellation::new(),
            &SequencedEvidence {
                current_through: usize::MAX,
                calls: AtomicUsize::new(0),
            },
        );

        assert!(matches!(
            outcome,
            SyncPlanExecutionOutcome::Completed { .. }
        ));
    }

    #[test]
    fn copy_file_and_hash_reports_incremental_progress() {
        let root = tempdir().unwrap();
        let source = root.path().join("source.bin");
        let destination = root.path().join("backup.bin");
        let payload = vec![0x5a_u8; (2 * 1024 * 1024) + 17];
        std::fs::write(&source, &payload).unwrap();
        let mut deltas = Vec::new();

        let hash = copy_file_and_hash(&source, &destination, |delta| deltas.push(delta)).unwrap();

        assert_eq!(deltas.iter().sum::<u64>(), payload.len() as u64);
        assert_eq!(deltas.len(), 3);
        assert_eq!(std::fs::read(&destination).unwrap(), payload);
        assert_eq!(hash, hash_file_with_progress(&source, |_| {}).unwrap());
    }

    #[test]
    fn readonly_trio_copy_stops_when_authorization_expires() {
        let root = tempdir().unwrap();
        let source = root.path().join("database.db");
        std::fs::write(&source, vec![0x5a_u8; (3 * 1024 * 1024) + 17]).unwrap();
        let checks = AtomicUsize::new(0);

        let copy = create_readonly_trio_copy_with_validation(&source, &|| {
            checks.fetch_add(1, Ordering::SeqCst) < 4
        })
        .unwrap();

        assert!(copy.is_none(), "授权撤销后不得返回可用隔离副本");
        assert!(checks.load(Ordering::SeqCst) >= 4);
    }

    #[test]
    fn attach_sessions_rejects_unplanned_wal_frames_after_lock() {
        let target = tempdir().unwrap();
        let db_path = make_attach_sessions_fixture(target.path());
        let conn = open_with_key(&db_path, TEST_RAW_KEY).unwrap();
        conn.execute_batch("PRAGMA journal_mode = WAL;").unwrap();
        drop(conn);
        for suffix in ["-wal", "-shm"] {
            let sidecar = database_sidecar_path(&db_path, suffix);
            if sidecar.exists() {
                std::fs::remove_file(sidecar).unwrap();
            }
        }
        let plan = attach_sessions_plan(&db_path, &["s1"]);
        // 超过 WAL 头长度即代表至少存在一个 frame，不能视为 SQLite 自身的空锁文件。
        std::fs::write(database_sidecar_path(&db_path, "-wal"), vec![0_u8; 33]).unwrap();

        assert!(!target_database_and_wal_match_plan(&db_path, &plan));
    }

    #[test]
    fn attach_sessions_supports_current_trae_artifact_layout() {
        let target = tempdir().unwrap();
        let storage = tempdir().unwrap();
        let db_path = make_current_attach_sessions_fixture(target.path());
        let plan = attach_sessions_plan(&db_path, &["s1"]);

        let outcome = fixture_executor(
            &WorkCnSyncExecutor::new(TEST_RAW_KEY),
            &db_path,
            storage.path(),
        )
        .execute_sync_plan(
            &plan,
            &OperationCancellation::new(),
            &SequencedEvidence {
                current_through: usize::MAX,
                calls: AtomicUsize::new(0),
            },
        );

        assert!(matches!(
            outcome,
            SyncPlanExecutionOutcome::Completed { .. }
        ));
        assert_eq!(
            table_value(
                &db_path,
                "local_artifact",
                "source_project_id",
                "artifact_id",
                "artifact-1"
            ),
            Some("p-target".to_string())
        );
        assert_eq!(
            table_value(
                &db_path,
                "local_artifact",
                "user_id",
                "artifact_id",
                "artifact-1"
            ),
            Some("2000000000000002".to_string())
        );
        assert_eq!(
            table_value(
                &db_path,
                "local_artifact_version",
                "source_project_id",
                "artifact_id",
                "artifact-1"
            ),
            Some("p-target".to_string())
        );
        assert_eq!(
            table_value(
                &db_path,
                "local_artifact",
                "source_project_id",
                "artifact_id",
                "artifact-2"
            ),
            Some("p-source".to_string())
        );
        assert_eq!(
            table_value(&db_path, "history_v2", "payload", "session_id", "s1"),
            Some("source history".to_string())
        );
    }

    #[test]
    fn attach_sessions_reports_truthful_progress_phases() {
        let target = tempdir().unwrap();
        let storage = tempdir().unwrap();
        let db_path = make_attach_sessions_fixture(target.path());
        let plan = attach_sessions_plan(&db_path, &["s1"]);
        let snapshots = Arc::new(Mutex::new(Vec::<ProgressSnapshot>::new()));
        let snapshots_for_reporter = Arc::clone(&snapshots);
        let reservation_seen = Arc::new(Mutex::new(false));
        let reservation_seen_for_reporter = Arc::clone(&reservation_seen);
        let target_path = target.path().to_path_buf();
        let storage_path = storage.path().to_path_buf();
        let executor = WorkCnSyncExecutor::new(TEST_RAW_KEY).with_progress_reporter(Arc::new(
            move |snapshot| {
                if snapshot.phase == ProgressPhase::Writing {
                    // 同卷预算只生成一个预留文件，位置可能是目标卷请求或存储卷请求的首个目录。
                    let found = [&target_path, &storage_path].into_iter().any(|path| {
                        std::fs::read_dir(path)
                            .ok()
                            .into_iter()
                            .flatten()
                            .flatten()
                            .any(|entry| {
                                entry
                                    .file_name()
                                    .to_string_lossy()
                                    .starts_with(".space-reservation-")
                            })
                    });
                    if found {
                        *reservation_seen_for_reporter.lock().unwrap() = true;
                    }
                }
                snapshots_for_reporter.lock().unwrap().push(snapshot);
            },
        ));

        let outcome = fixture_executor(&executor, &db_path, storage.path()).execute_sync_plan(
            &plan,
            &OperationCancellation::new(),
            &SequencedEvidence {
                current_through: usize::MAX,
                calls: AtomicUsize::new(0),
            },
        );

        assert!(matches!(
            outcome,
            SyncPlanExecutionOutcome::Completed { .. }
        ));
        let snapshots = snapshots.lock().unwrap();
        let phases: Vec<_> = snapshots.iter().map(|snapshot| snapshot.phase).collect();
        assert!(phases.contains(&ProgressPhase::Preparing));
        assert!(phases.contains(&ProgressPhase::Copying));
        assert!(phases.contains(&ProgressPhase::Hashing));
        assert!(phases.contains(&ProgressPhase::Writing));
        assert!(phases.contains(&ProgressPhase::Verifying));
        assert!(phases.contains(&ProgressPhase::Completed));

        let copying = snapshots
            .iter()
            .find(|snapshot| snapshot.phase == ProgressPhase::Copying)
            .unwrap();
        assert!(copying.total_bytes.is_some());
        assert_eq!(copying.completed_bytes, 0);
        assert!(snapshots.iter().any(
            |snapshot| snapshot.phase == ProgressPhase::Copying && snapshot.completed_bytes > 0
        ));
        assert!(snapshots.iter().any(
            |snapshot| snapshot.phase == ProgressPhase::Hashing && snapshot.completed_bytes > 0
        ));

        let verifying = snapshots
            .iter()
            .find(|snapshot| snapshot.phase == ProgressPhase::Verifying)
            .unwrap();
        assert_eq!(verifying.total_bytes, None);
        assert_eq!(verifying.percent_basis_points, None);

        let writing = snapshots
            .iter()
            .find(|snapshot| snapshot.phase == ProgressPhase::Writing)
            .unwrap();
        assert_eq!(writing.total_bytes, None);
        assert!(!writing.cancellable);
        assert!(*reservation_seen.lock().unwrap());
        assert!(!std::fs::read_dir(storage.path())
            .unwrap()
            .flatten()
            .any(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".space-reservation-")
            }));

        let completed = snapshots
            .iter()
            .find(|snapshot| snapshot.phase == ProgressPhase::Completed)
            .unwrap();
        assert_eq!(completed.total_bytes, Some(completed.completed_bytes));
        assert!(!completed.cancellable);
    }

    #[test]
    fn attach_sessions_rejects_current_layout_version_owned_by_another_session_before_backup() {
        let target = tempdir().unwrap();
        let storage = tempdir().unwrap();
        let db_path = make_current_attach_sessions_fixture(target.path());
        let conn = open_with_key(&db_path, TEST_RAW_KEY).unwrap();
        conn.execute(
            "UPDATE local_artifact_version SET writer_session_id = ?1 WHERE artifact_id = ?2",
            params!["s2", "artifact-1"],
        )
        .unwrap();
        drop(conn);
        let before = snapshot_db_trio(target.path(), "database.db");
        let plan = attach_sessions_plan(&db_path, &["s1"]);

        let outcome = fixture_executor(
            &WorkCnSyncExecutor::new(TEST_RAW_KEY),
            &db_path,
            storage.path(),
        )
        .execute_sync_plan(
            &plan,
            &OperationCancellation::new(),
            &SequencedEvidence {
                current_through: usize::MAX,
                calls: AtomicUsize::new(0),
            },
        );

        assert_eq!(outcome, SyncPlanExecutionOutcome::UnsupportedPlan);
        assert_eq!(snapshot_db_trio(target.path(), "database.db"), before);
        assert!(!storage.path().join("operations").exists());
        assert!(!storage.path().join("backups").exists());
    }

    #[test]
    fn attach_sessions_rechecks_current_layout_version_ownership_inside_transaction() {
        let target = tempdir().unwrap();
        let db_path = make_current_attach_sessions_fixture(target.path());
        let session_ids = [traesync_domain::SessionIdentity::new("work_cn", "s1")];

        assert!(attach_sessions_preflight(
            &db_path,
            TEST_RAW_KEY,
            target.path(),
            "p-source",
            "p-target",
            &session_ids,
        ));
        let conn = open_with_key(&db_path, TEST_RAW_KEY).unwrap();
        conn.execute(
            "UPDATE local_artifact_version SET writer_session_id = ?1 WHERE artifact_id = ?2",
            params!["s2", "artifact-1"],
        )
        .unwrap();
        drop(conn);
        let before = snapshot_db_trio(target.path(), "database.db");

        let transaction = apply_attach_sessions_transaction_with_evidence(
            &db_path,
            TEST_RAW_KEY,
            "p-source",
            "p-target",
            &session_ids,
            || true,
            || true,
        );

        assert!(matches!(transaction, AttachSessionsTransaction::Failed));
        assert_eq!(snapshot_db_trio(target.path(), "database.db"), before);
    }

    #[test]
    fn attach_sessions_stops_before_dml_when_in_lock_evidence_changes() {
        let target = tempdir().unwrap();
        let db_path = make_attach_sessions_fixture(target.path());
        let session_ids = [traesync_domain::SessionIdentity::new("work_cn", "s1")];
        let before = snapshot_db_trio(target.path(), "database.db");

        let transaction = apply_attach_sessions_transaction_with_evidence(
            &db_path,
            TEST_RAW_KEY,
            "p-source",
            "p-target",
            &session_ids,
            || false,
            || true,
        );

        assert!(matches!(
            transaction,
            AttachSessionsTransaction::EvidenceChanged
        ));
        assert_eq!(snapshot_db_trio(target.path(), "database.db"), before);
    }

    #[test]
    fn attach_sessions_preserves_known_server_history_conversation_reference() {
        let target = tempdir().unwrap();
        let storage = tempdir().unwrap();
        let db_path = make_current_attach_sessions_fixture(target.path());
        let conn = open_with_key(&db_path, TEST_RAW_KEY).unwrap();
        // 真实 TRAE schema 的 conversation_id 是只读稳定会话标识，重挂后必须保持不变。
        conn.execute_batch(
            "CREATE TABLE server_history_info (session_id TEXT NOT NULL, user_id TEXT NOT NULL, conversation_id TEXT NOT NULL); \
             INSERT INTO server_history_info VALUES ('s1', '1000000000000001', 'conversation-1');",
        )
        .unwrap();
        drop(conn);
        let plan = attach_sessions_plan(&db_path, &["s1"]);

        let outcome = fixture_executor(
            &WorkCnSyncExecutor::new(TEST_RAW_KEY),
            &db_path,
            storage.path(),
        )
        .execute_sync_plan(
            &plan,
            &OperationCancellation::new(),
            &SequencedEvidence {
                current_through: usize::MAX,
                calls: AtomicUsize::new(0),
            },
        );

        assert!(matches!(
            outcome,
            SyncPlanExecutionOutcome::Completed { .. }
        ));
        assert_eq!(
            table_value(
                &db_path,
                "server_history_info",
                "conversation_id",
                "session_id",
                "s1"
            ),
            Some("conversation-1".to_string())
        );
    }

    #[test]
    fn attach_sessions_rejects_shared_current_layout_artifact_id_before_backup() {
        let target = tempdir().unwrap();
        let storage = tempdir().unwrap();
        let db_path = make_current_attach_sessions_fixture(target.path());
        let conn = open_with_key(&db_path, TEST_RAW_KEY).unwrap();
        conn.execute(
            "INSERT INTO local_artifact (id, artifact_id, user_id, source_project_id, creator_session_id, creator_turn_id, display_name) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                "artifact-row-duplicate",
                "artifact-1",
                "1000000000000001",
                "p-source",
                "s2",
                "turn-s2",
                "shared artifact"
            ],
        )
        .unwrap();
        drop(conn);
        let before = snapshot_db_trio(target.path(), "database.db");
        let plan = attach_sessions_plan(&db_path, &["s1"]);

        let outcome = fixture_executor(
            &WorkCnSyncExecutor::new(TEST_RAW_KEY),
            &db_path,
            storage.path(),
        )
        .execute_sync_plan(
            &plan,
            &OperationCancellation::new(),
            &SequencedEvidence {
                current_through: usize::MAX,
                calls: AtomicUsize::new(0),
            },
        );

        assert_eq!(outcome, SyncPlanExecutionOutcome::UnsupportedPlan);
        assert_eq!(snapshot_db_trio(target.path(), "database.db"), before);
        assert!(!storage.path().join("operations").exists());
        assert!(!storage.path().join("backups").exists());
    }

    #[test]
    fn attach_sessions_updates_every_selected_session_without_touching_content() {
        let target = tempdir().unwrap();
        let storage = tempdir().unwrap();
        let db_path = make_attach_sessions_fixture(target.path());
        let plan = attach_sessions_plan(&db_path, &["s1", "s2"]);

        let outcome = fixture_executor(
            &WorkCnSyncExecutor::new(TEST_RAW_KEY),
            &db_path,
            storage.path(),
        )
        .execute_sync_plan(
            &plan,
            &OperationCancellation::new(),
            &SequencedEvidence {
                current_through: usize::MAX,
                calls: AtomicUsize::new(0),
            },
        );

        assert!(
            matches!(
                outcome,
                SyncPlanExecutionOutcome::Completed { affected_rows } if affected_rows >= 12
            ),
            "两条会话重挂应完成且至少更新 12 行，实际结果: {outcome:?}"
        );
        for session_id in ["s1", "s2"] {
            assert_eq!(
                table_value(
                    &db_path,
                    "chat_session",
                    "project_id",
                    "session_id",
                    session_id
                ),
                Some("p-target".to_string())
            );
            assert_eq!(
                table_value(
                    &db_path,
                    "session_project",
                    "project_id",
                    "session_id",
                    session_id
                ),
                Some("p-target".to_string())
            );
        }
        assert_eq!(
            table_value(&db_path, "chat_message", "body", "message_id", "m1"),
            Some("source one body".to_string())
        );
        assert_eq!(
            table_value(&db_path, "chat_message", "body", "message_id", "m2"),
            Some("source two body".to_string())
        );
        assert_eq!(
            table_value(&db_path, "chat_fts", "indexed_body", "session_id", "s1"),
            Some("source one body".to_string())
        );
        assert_eq!(
            table_value(&db_path, "chat_fts", "indexed_body", "session_id", "s2"),
            Some("source two body".to_string())
        );
    }

    #[test]
    fn attach_sessions_rejects_missing_target_sandbox_before_backup() {
        let target = tempdir().unwrap();
        let storage = tempdir().unwrap();
        let db_path = make_attach_sessions_fixture(target.path());
        let before = snapshot_db_trio(target.path(), "database.db");
        std::fs::remove_file(target.path().join("sandbox").join("p-target.json")).unwrap();
        let plan = attach_sessions_plan(&db_path, &["s1"]);

        let outcome = fixture_executor(
            &WorkCnSyncExecutor::new(TEST_RAW_KEY),
            &db_path,
            storage.path(),
        )
        .execute_sync_plan(
            &plan,
            &OperationCancellation::new(),
            &SequencedEvidence {
                current_through: usize::MAX,
                calls: AtomicUsize::new(0),
            },
        );

        assert_eq!(outcome, SyncPlanExecutionOutcome::UnsupportedPlan);
        assert_eq!(snapshot_db_trio(target.path(), "database.db"), before);
        assert!(!storage.path().join("operations").exists());
        assert!(!storage.path().join("backups").exists());
    }

    #[test]
    fn attach_sessions_rejects_unknown_project_reference_before_backup() {
        let target = tempdir().unwrap();
        let storage = tempdir().unwrap();
        let db_path = make_attach_sessions_fixture(target.path());
        let conn = open_with_key(&db_path, TEST_RAW_KEY).unwrap();
        conn.execute_batch("CREATE TABLE unknown_project_reference (project_id TEXT NOT NULL);")
            .unwrap();
        drop(conn);
        let before = snapshot_db_trio(target.path(), "database.db");
        let plan = attach_sessions_plan(&db_path, &["s1"]);

        let outcome = fixture_executor(
            &WorkCnSyncExecutor::new(TEST_RAW_KEY),
            &db_path,
            storage.path(),
        )
        .execute_sync_plan(
            &plan,
            &OperationCancellation::new(),
            &SequencedEvidence {
                current_through: usize::MAX,
                calls: AtomicUsize::new(0),
            },
        );

        assert_eq!(outcome, SyncPlanExecutionOutcome::UnsupportedPlan);
        assert_eq!(snapshot_db_trio(target.path(), "database.db"), before);
        assert!(!storage.path().join("operations").exists());
    }

    #[test]
    fn attach_sessions_rejects_unknown_session_reference_before_backup() {
        let target = tempdir().unwrap();
        let storage = tempdir().unwrap();
        let db_path = make_attach_sessions_fixture(target.path());
        let conn = open_with_key(&db_path, TEST_RAW_KEY).unwrap();
        conn.execute_batch("CREATE TABLE unknown_session_reference (session_id TEXT NOT NULL);")
            .unwrap();
        drop(conn);
        let before = snapshot_db_trio(target.path(), "database.db");
        let plan = attach_sessions_plan(&db_path, &["s1"]);

        let outcome = fixture_executor(
            &WorkCnSyncExecutor::new(TEST_RAW_KEY),
            &db_path,
            storage.path(),
        )
        .execute_sync_plan(
            &plan,
            &OperationCancellation::new(),
            &SequencedEvidence {
                current_through: usize::MAX,
                calls: AtomicUsize::new(0),
            },
        );

        assert_eq!(outcome, SyncPlanExecutionOutcome::UnsupportedPlan);
        assert_eq!(snapshot_db_trio(target.path(), "database.db"), before);
        assert!(!storage.path().join("operations").exists());
    }

    #[test]
    fn attach_sessions_rejects_unknown_owner_reference_before_backup() {
        let target = tempdir().unwrap();
        let storage = tempdir().unwrap();
        let db_path = make_attach_sessions_fixture(target.path());
        let conn = open_with_key(&db_path, TEST_RAW_KEY).unwrap();
        conn.execute_batch("CREATE TABLE unknown_owner_reference (owner_id TEXT NOT NULL);")
            .unwrap();
        drop(conn);
        let before = snapshot_db_trio(target.path(), "database.db");
        let plan = attach_sessions_plan(&db_path, &["s1"]);

        let outcome = fixture_executor(
            &WorkCnSyncExecutor::new(TEST_RAW_KEY),
            &db_path,
            storage.path(),
        )
        .execute_sync_plan(
            &plan,
            &OperationCancellation::new(),
            &SequencedEvidence {
                current_through: usize::MAX,
                calls: AtomicUsize::new(0),
            },
        );

        assert_eq!(outcome, SyncPlanExecutionOutcome::UnsupportedPlan);
        assert_eq!(snapshot_db_trio(target.path(), "database.db"), before);
        assert!(!storage.path().join("operations").exists());
    }

    #[test]
    fn attach_sessions_rejects_unknown_declared_foreign_key_before_backup() {
        let target = tempdir().unwrap();
        let storage = tempdir().unwrap();
        let db_path = make_attach_sessions_fixture(target.path());
        let conn = open_with_key(&db_path, TEST_RAW_KEY).unwrap();
        conn.execute_batch(
            "CREATE TABLE unknown_foreign_key_reference (opaque_key TEXT REFERENCES project(project_id));",
        )
        .unwrap();
        drop(conn);
        let before = snapshot_db_trio(target.path(), "database.db");
        let plan = attach_sessions_plan(&db_path, &["s1"]);

        let outcome = fixture_executor(
            &WorkCnSyncExecutor::new(TEST_RAW_KEY),
            &db_path,
            storage.path(),
        )
        .execute_sync_plan(
            &plan,
            &OperationCancellation::new(),
            &SequencedEvidence {
                current_through: usize::MAX,
                calls: AtomicUsize::new(0),
            },
        );

        assert_eq!(outcome, SyncPlanExecutionOutcome::UnsupportedPlan);
        assert_eq!(snapshot_db_trio(target.path(), "database.db"), before);
        assert!(!storage.path().join("operations").exists());
    }

    #[test]
    fn attach_sessions_preserves_unmapped_nonrelational_table() {
        let target = tempdir().unwrap();
        let storage = tempdir().unwrap();
        let db_path = make_attach_sessions_fixture(target.path());
        let conn = open_with_key(&db_path, TEST_RAW_KEY).unwrap();
        conn.execute_batch("CREATE TABLE unmapped_cache (cache_key TEXT PRIMARY KEY, value TEXT NOT NULL); INSERT INTO unmapped_cache VALUES ('cache-1', 'unchanged');")
            .unwrap();
        drop(conn);
        let plan = attach_sessions_plan(&db_path, &["s1"]);

        let outcome = fixture_executor(
            &WorkCnSyncExecutor::new(TEST_RAW_KEY),
            &db_path,
            storage.path(),
        )
        .execute_sync_plan(
            &plan,
            &OperationCancellation::new(),
            &SequencedEvidence {
                current_through: usize::MAX,
                calls: AtomicUsize::new(0),
            },
        );

        assert!(matches!(
            outcome,
            SyncPlanExecutionOutcome::Completed { .. }
        ));
        assert_eq!(
            table_value(&db_path, "unmapped_cache", "value", "cache_key", "cache-1"),
            Some("unchanged".to_string())
        );
    }

    #[test]
    fn attach_sessions_preserves_without_rowid_nonrelational_table() {
        let target = tempdir().unwrap();
        let storage = tempdir().unwrap();
        let db_path = make_attach_sessions_fixture(target.path());
        let conn = open_with_key(&db_path, TEST_RAW_KEY).unwrap();
        // 真实 FTS 阴影表可能没有 rowid；保护摘要仍必须允许受限重挂完成。
        conn.execute_batch(
            "CREATE TABLE without_rowid_cache (cache_key TEXT PRIMARY KEY, value TEXT NOT NULL) WITHOUT ROWID; \
             INSERT INTO without_rowid_cache VALUES ('cache-1', 'unchanged'), ('cache-2', 'also unchanged');",
        )
        .unwrap();
        drop(conn);
        let plan = attach_sessions_plan(&db_path, &["s1"]);

        let outcome = fixture_executor(
            &WorkCnSyncExecutor::new(TEST_RAW_KEY),
            &db_path,
            storage.path(),
        )
        .execute_sync_plan(
            &plan,
            &OperationCancellation::new(),
            &SequencedEvidence {
                current_through: usize::MAX,
                calls: AtomicUsize::new(0),
            },
        );

        assert!(matches!(
            outcome,
            SyncPlanExecutionOutcome::Completed { .. }
        ));
        assert_eq!(
            table_value(
                &db_path,
                "without_rowid_cache",
                "value",
                "cache_key",
                "cache-1"
            ),
            Some("unchanged".to_string())
        );
    }

    #[test]
    fn attach_sessions_rejects_unmapped_column_before_backup() {
        let target = tempdir().unwrap();
        let storage = tempdir().unwrap();
        let db_path = make_attach_sessions_fixture(target.path());
        let conn = open_with_key(&db_path, TEST_RAW_KEY).unwrap();
        conn.execute_batch("ALTER TABLE local_artifact ADD COLUMN project_ref TEXT;")
            .unwrap();
        drop(conn);
        let before = snapshot_db_trio(target.path(), "database.db");
        let plan = attach_sessions_plan(&db_path, &["s1"]);

        let outcome = fixture_executor(
            &WorkCnSyncExecutor::new(TEST_RAW_KEY),
            &db_path,
            storage.path(),
        )
        .execute_sync_plan(
            &plan,
            &OperationCancellation::new(),
            &SequencedEvidence {
                current_through: usize::MAX,
                calls: AtomicUsize::new(0),
            },
        );

        assert_eq!(outcome, SyncPlanExecutionOutcome::UnsupportedPlan);
        assert_eq!(snapshot_db_trio(target.path(), "database.db"), before);
        assert!(!storage.path().join("operations").exists());
    }

    #[test]
    fn attach_sessions_rejects_ambiguous_session_relation_before_backup() {
        let target = tempdir().unwrap();
        let storage = tempdir().unwrap();
        let db_path = make_attach_sessions_fixture(target.path());
        let conn = open_with_key(&db_path, TEST_RAW_KEY).unwrap();
        conn.execute_batch(
            "ALTER TABLE session_project RENAME TO session_project_original;\
             CREATE TABLE session_project (session_id TEXT NOT NULL, project_id TEXT NOT NULL);\
             INSERT INTO session_project SELECT session_id, project_id FROM session_project_original;\
             DROP TABLE session_project_original;\
             INSERT INTO session_project (session_id, project_id) VALUES ('s1', 'p-other');",
        )
        .unwrap();
        drop(conn);
        let before = snapshot_db_trio(target.path(), "database.db");
        let plan = attach_sessions_plan(&db_path, &["s1"]);

        let outcome = fixture_executor(
            &WorkCnSyncExecutor::new(TEST_RAW_KEY),
            &db_path,
            storage.path(),
        )
        .execute_sync_plan(
            &plan,
            &OperationCancellation::new(),
            &SequencedEvidence {
                current_through: usize::MAX,
                calls: AtomicUsize::new(0),
            },
        );

        assert_eq!(outcome, SyncPlanExecutionOutcome::UnsupportedPlan);
        assert_eq!(snapshot_db_trio(target.path(), "database.db"), before);
        assert!(!storage.path().join("operations").exists());
        assert!(!storage.path().join("backups").exists());
    }

    #[test]
    fn attach_sessions_rejects_incomplete_session_relation_before_backup() {
        let target = tempdir().unwrap();
        let storage = tempdir().unwrap();
        let db_path = make_attach_sessions_fixture(target.path());
        let conn = open_with_key(&db_path, TEST_RAW_KEY).unwrap();
        conn.execute(
            "DELETE FROM session_project WHERE session_id = ?1",
            params!["s1"],
        )
        .unwrap();
        drop(conn);
        let before = snapshot_db_trio(target.path(), "database.db");
        let plan = attach_sessions_plan(&db_path, &["s1"]);

        let outcome = fixture_executor(
            &WorkCnSyncExecutor::new(TEST_RAW_KEY),
            &db_path,
            storage.path(),
        )
        .execute_sync_plan(
            &plan,
            &OperationCancellation::new(),
            &SequencedEvidence {
                current_through: usize::MAX,
                calls: AtomicUsize::new(0),
            },
        );

        assert_eq!(outcome, SyncPlanExecutionOutcome::UnsupportedPlan);
        assert_eq!(snapshot_db_trio(target.path(), "database.db"), before);
        assert!(!storage.path().join("operations").exists());
    }

    #[test]
    fn attach_sessions_rejects_target_project_with_different_identity_before_backup() {
        let target = tempdir().unwrap();
        let storage = tempdir().unwrap();
        let db_path = make_attach_sessions_fixture(target.path());
        let conn = open_with_key(&db_path, TEST_RAW_KEY).unwrap();
        conn.execute(
            "UPDATE project SET biz_project_id = ?1 WHERE project_id = ?2",
            params!["other-project", "p-target"],
        )
        .unwrap();
        drop(conn);
        let before = snapshot_db_trio(target.path(), "database.db");
        let plan = attach_sessions_plan(&db_path, &["s1"]);

        let outcome = fixture_executor(
            &WorkCnSyncExecutor::new(TEST_RAW_KEY),
            &db_path,
            storage.path(),
        )
        .execute_sync_plan(
            &plan,
            &OperationCancellation::new(),
            &SequencedEvidence {
                current_through: usize::MAX,
                calls: AtomicUsize::new(0),
            },
        );

        assert_eq!(outcome, SyncPlanExecutionOutcome::UnsupportedPlan);
        assert_eq!(snapshot_db_trio(target.path(), "database.db"), before);
        assert!(!storage.path().join("operations").exists());
    }

    #[test]
    fn restarted_postwrite_manifest_captures_failure_scene_without_replaying_attach_sessions() {
        let target = tempdir().unwrap();
        let storage = tempdir().unwrap();
        let db_path = make_attach_sessions_fixture(target.path());
        let plan = attach_sessions_plan(&db_path, &["s1"]);
        let interrupted_operation_id = traesync_domain::OperationId::new();
        let journal = crate::operation_manifest::OperationManifestJournal::create(
            storage.path(),
            &interrupted_operation_id,
            plan.data_location_id(),
            plan.target_file_evidence(),
        )
        .unwrap();
        journal.transition(OperationState::BackingUp).unwrap();
        journal.transition(OperationState::BackupVerified).unwrap();
        journal.transition(OperationState::TargetWriting).unwrap();
        let before = snapshot_db_trio(target.path(), "database.db");

        let outcome = fixture_executor(
            &WorkCnSyncExecutor::new(TEST_RAW_KEY),
            &db_path,
            storage.path(),
        )
        .execute_sync_plan(
            &plan,
            &OperationCancellation::new(),
            &SequencedEvidence {
                current_through: usize::MAX,
                calls: AtomicUsize::new(0),
            },
        );

        assert_eq!(
            outcome,
            SyncPlanExecutionOutcome::ManualRecoveryRequired {
                backups_preserved: false
            }
        );
        let failure_raw = storage
            .path()
            .join("backups")
            .join(interrupted_operation_id.as_str())
            .join("failure")
            .join("raw");
        assert!(verify_raw_backup(&failure_raw));
        assert_eq!(snapshot_db_trio(target.path(), "database.db"), before);
        assert_eq!(
            table_value(&db_path, "chat_session", "project_id", "session_id", "s1"),
            Some("p-source".to_string()),
            "协调不得重放中断操作或当前 AttachSessions 计划"
        );
    }

    #[test]
    fn restarted_manifest_from_another_data_location_does_not_capture_current_target() {
        let target = tempdir().unwrap();
        let storage = tempdir().unwrap();
        let db_path = make_attach_sessions_fixture(target.path());
        let plan = attach_sessions_plan(&db_path, &["s1"]);
        let interrupted_operation_id = traesync_domain::OperationId::new();
        let journal = crate::operation_manifest::OperationManifestJournal::create(
            storage.path(),
            &interrupted_operation_id,
            "another-fixture-location",
            plan.target_file_evidence(),
        )
        .unwrap();
        journal.transition(OperationState::BackingUp).unwrap();
        journal.transition(OperationState::BackupVerified).unwrap();
        journal.transition(OperationState::TargetWriting).unwrap();
        let before = snapshot_db_trio(target.path(), "database.db");

        let outcome = fixture_executor(
            &WorkCnSyncExecutor::new(TEST_RAW_KEY),
            &db_path,
            storage.path(),
        )
        .execute_sync_plan(
            &plan,
            &OperationCancellation::new(),
            &SequencedEvidence {
                current_through: usize::MAX,
                calls: AtomicUsize::new(0),
            },
        );

        assert_eq!(
            outcome,
            SyncPlanExecutionOutcome::ManualRecoveryRequired {
                backups_preserved: false
            }
        );
        assert_eq!(journal.latest_state(), Some(OperationState::TargetWriting));
        assert!(!storage
            .path()
            .join("backups")
            .join(interrupted_operation_id.as_str())
            .join("failure")
            .exists());
        assert_eq!(snapshot_db_trio(target.path(), "database.db"), before);
    }

    /// 复制真实隔离副本三件套；源目录只承担读取，不在源文件上打开写连接。
    fn copy_real_trio(source_dir: &Path, destination_dir: &Path) {
        std::fs::create_dir_all(destination_dir).unwrap();
        for file_name in ["database.db", "database.db-wal", "database.db-shm"] {
            let source = source_dir.join(file_name);
            if source.is_file() {
                std::fs::copy(&source, destination_dir.join(file_name)).unwrap();
            }
        }
    }

    /// 真实恢复矩阵只需建立只读证据链接，避免每个状态重复占用整库空间。
    fn link_real_trio(source_dir: &Path, destination_dir: &Path) -> bool {
        if std::fs::create_dir_all(destination_dir).is_err() {
            return false;
        }
        for file_name in ["database.db", "database.db-wal", "database.db-shm"] {
            let source = source_dir.join(file_name);
            if !source.is_file() {
                continue;
            }
            let destination = destination_dir.join(file_name);
            if std::fs::hard_link(&source, destination).is_err() {
                return false;
            }
        }
        true
    }

    /// 为真实恢复现场生成与生产校验器兼容的三件套哈希清单。
    fn link_real_failure_scene(source_dir: &Path, storage_root: &Path, operation_id: &str) -> bool {
        let failure_raw = storage_root
            .join("backups")
            .join(operation_id)
            .join("failure")
            .join("raw");
        let Some(failure_parent) = failure_raw.parent() else {
            return false;
        };
        if std::fs::create_dir_all(failure_parent).is_err() {
            return false;
        }
        if std::fs::create_dir(&failure_raw).is_err() {
            return verify_raw_backup(&failure_raw);
        }
        if !link_real_trio(source_dir, &failure_raw) {
            return false;
        }
        let mut hashes = String::new();
        for file_name in ["database.db", "database.db-wal", "database.db-shm"] {
            let path = failure_raw.join(file_name);
            if let Some(hash) = sha256_file_for_backup(&path) {
                hashes.push_str(&format!("{hash}  {file_name}\n"));
            }
        }
        std::fs::write(failure_raw.join("hashes.sha256"), hashes).is_ok()
            && verify_raw_backup(&failure_raw)
    }

    /// 真实 clone 崩溃子进程使用的状态名称解析；名称与 manifest JSON 保持一致。
    fn real_process_crash_state(name: &str) -> OperationState {
        match name {
            "planned" => OperationState::Planned,
            "backing_up" => OperationState::BackingUp,
            "backup_verified" => OperationState::BackupVerified,
            "target_writing" => OperationState::TargetWriting,
            "target_committed_unverified" => OperationState::TargetCommittedUnverified,
            "target_verifying" => OperationState::TargetVerifying,
            "catalog_reconciling" => OperationState::CatalogReconciling,
            "verification_inconclusive" => OperationState::VerificationInconclusive,
            "failure_preserving" => OperationState::FailurePreserving,
            "failure_snapshot_verified" => OperationState::FailureSnapshotVerified,
            "restore_staging" => OperationState::RestoreStaging,
            "restore_staged" => OperationState::RestoreStaged,
            "restore_replacing" => OperationState::RestoreReplacing,
            "restored_verifying" => OperationState::RestoredVerifying,
            other => panic!("未知真实 clone 崩溃状态: {other}"),
        }
    }

    /// 在真实 SQLCipher clone 上执行一条可识别 DML；写入中断时故意不提交事务。
    fn real_process_write_marker(
        db_path: &Path,
        raw_key: &str,
        marker: &str,
        terminate_before_commit: bool,
    ) {
        // 真实崩溃子进程只在测试中输出阶段耗时，定位大库 SQLCipher 写入的实际瓶颈。
        let started = std::time::Instant::now();
        let state_name =
            std::env::var("TRAE_SYNC_REAL_CRASH_STATE").unwrap_or_else(|_| "unknown".to_string());
        eprintln!("REAL_CRASH_CHILD state={state_name} step=write_marker_start elapsed_ms=0");
        let mut connection = open_with_key(db_path, raw_key).expect("真实 clone 无法打开写连接");
        eprintln!(
            "REAL_CRASH_CHILD state={state_name} step=write_connection_opened elapsed_ms={}",
            started.elapsed().as_millis()
        );
        let project_id: String = connection
            .query_row(
                "SELECT project_id FROM project WHERE project_id NOT LIKE 't07-%' ORDER BY project_id LIMIT 1",
                [],
                |row| row.get(0),
            )
            .expect("真实 clone 没有可用于崩溃注入的项目");
        eprintln!(
            "REAL_CRASH_CHILD state={state_name} step=project_queried elapsed_ms={}",
            started.elapsed().as_millis()
        );
        // 父进程按主键复核标记，避免对真实大库的 description 做通配符全表扫描。
        let project_id_path = PathBuf::from(
            std::env::var("TRAE_SYNC_REAL_CRASH_ROOT").expect("缺少真实崩溃矩阵根目录"),
        )
        .join("crash-project-id.txt");
        std::fs::write(project_id_path, &project_id).expect("无法记录真实崩溃目标项目 ID");
        let transaction = connection
            .transaction()
            .expect("真实 clone 无法开始 SQLCipher 事务");
        eprintln!(
            "REAL_CRASH_CHILD state={state_name} step=transaction_started elapsed_ms={}",
            started.elapsed().as_millis()
        );
        let changed = transaction
            .execute(
                "UPDATE project SET description = COALESCE(description, '') || ?1 WHERE project_id = ?2",
                rusqlite::params![marker, project_id],
            )
            .expect("真实 clone 崩溃注入 DML 失败");
        eprintln!(
            "REAL_CRASH_CHILD state={state_name} step=marker_updated elapsed_ms={}",
            started.elapsed().as_millis()
        );
        assert_eq!(changed, 1, "真实 clone 崩溃注入必须实际修改一行");
        if terminate_before_commit {
            // 不执行 Drop，让操作系统在进程终止时关闭连接；SQLite 应回滚未提交事务。
            std::process::exit(86);
        }
        transaction
            .commit()
            .expect("真实 clone 崩溃注入事务提交失败");
        eprintln!(
            "REAL_CRASH_CHILD state={state_name} step=marker_committed elapsed_ms={}",
            started.elapsed().as_millis()
        );
    }

    /// 真实矩阵复用一次已验证的写前双备份；hard link 只减少重复占用，不改变证据内容。
    fn link_real_before_backup(shared_before: &Path, destination_before: &Path) {
        let shared_raw = shared_before.join("raw");
        let shared_logical = shared_before.join("logical").join("database.db");
        let destination_raw = destination_before.join("raw");
        let destination_logical = destination_before.join("logical");
        std::fs::create_dir_all(&destination_raw).expect("无法创建真实崩溃写前备份目录");
        std::fs::create_dir_all(&destination_logical).expect("无法创建真实崩溃逻辑备份目录");
        for file_name in [
            "database.db",
            "database.db-wal",
            "database.db-shm",
            "hashes.sha256",
        ] {
            let source = shared_raw.join(file_name);
            if source.is_file() {
                std::fs::hard_link(&source, destination_raw.join(file_name))
                    .expect("无法复用真实崩溃写前原始备份");
            }
        }
        std::fs::hard_link(&shared_logical, destination_logical.join("database.db"))
            .expect("无法复用真实崩溃逻辑备份");
    }

    /// 对隔离副本使用只读 SQLCipher 连接，避免每次断言再次复制整套大库。
    fn real_open_query_only(db_path: &Path, raw_key: &str) -> Connection {
        let connection = Connection::open_with_flags(
            db_path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI,
        )
        .expect("真实 clone 无法只读打开 SQLCipher 数据库");
        connection
            .execute_batch(&format!(
                "PRAGMA key = \"x'{raw_key}'\"; PRAGMA query_only = ON;"
            ))
            .expect("真实 clone 只读 SQLCipher key 失败");
        connection
    }

    /// 子进程在真实 clone 上落盘指定非终态后终止；父进程随后负责独立重启协调。
    #[test]
    #[ignore = "仅由真实隔离崩溃矩阵父测试启动"]
    fn real_clone_process_crash_child_after_persisting_nonterminal_state() {
        let root = PathBuf::from(
            std::env::var("TRAE_SYNC_REAL_CRASH_ROOT").expect("缺少真实崩溃矩阵根目录"),
        );
        let raw_key =
            std::env::var("TRAE_SYNC_REAL_CRASH_RAW_KEY").expect("缺少真实崩溃矩阵 raw key");
        let relative_dir = std::env::var("TRAE_SYNC_REAL_CRASH_DB_RELATIVE")
            .unwrap_or_else(|_| "clone".to_string());
        let state_name = std::env::var("TRAE_SYNC_REAL_CRASH_STATE").expect("缺少真实崩溃矩阵状态");
        let storage = PathBuf::from(
            std::env::var("TRAE_SYNC_REAL_CRASH_STORAGE").expect("缺少真实崩溃矩阵恢复区"),
        );
        let shared_before = std::env::var("TRAE_SYNC_REAL_CRASH_SHARED_BEFORE")
            .ok()
            .filter(|value| !value.is_empty())
            .map(PathBuf::from);
        let marker =
            std::env::var("TRAE_SYNC_REAL_CRASH_MARKER").expect("缺少真实崩溃矩阵 DML 标记");
        let state = real_process_crash_state(&state_name);
        let started = std::time::Instant::now();
        let trace = |step: &str| {
            eprintln!(
                "REAL_CRASH_CHILD state={state_name} step={step} elapsed_ms={}",
                started.elapsed().as_millis()
            );
        };
        trace("start");

        let guard = FixturePathGuard::new(&root).expect("真实崩溃矩阵根目录未通过隔离 guard");
        let db_path = guard
            .validate_db_relative_path(&format!("{relative_dir}/database.db"))
            .expect("真实崩溃矩阵数据库路径未通过隔离 guard");
        let storage = guard
            .validate_fixture_storage_root(&storage)
            .expect("真实崩溃矩阵恢复区未通过隔离 guard");
        trace("paths_validated");
        // 父进程已对每个 case 的初始内容绑定同一份 baseline；没有该绑定时才回退到本地哈希。
        let target_evidence = match std::env::var("TRAE_SYNC_REAL_CRASH_INITIAL_EVIDENCE") {
            Ok(serialized) => {
                serde_json::from_str(&serialized).expect("真实崩溃初始证据 JSON 无效")
            }
            Err(_) => target_file_evidence(&db_path).expect("真实 clone 缺少 database.db"),
        };
        std::fs::create_dir_all(&storage).expect("真实崩溃矩阵无法创建恢复区");
        if state == OperationState::CatalogReconciling {
            ensure_catalog_initialized(&storage, &raw_key).expect("真实崩溃矩阵无法初始化目录库");
        }
        let operation_id = traesync_domain::OperationId::new();
        let journal = OperationManifestJournal::create(
            &storage,
            &operation_id,
            "real-isolated-t07-process-crash",
            &target_evidence,
        )
        .expect("真实崩溃矩阵无法创建操作 manifest");
        trace("manifest_created");
        let before_dir = storage
            .join("backups")
            .join(journal.operation_id())
            .join("before");
        let raw_dir = before_dir.join("raw");
        let logical_db = before_dir.join("logical").join("database.db");

        // planned 代表尚未进入备份阶段；其余状态至少按真实执行顺序写入备份证据。
        if state == OperationState::Planned {
            trace("planned_exit");
            std::process::exit(86);
        }
        journal
            .transition(OperationState::BackingUp)
            .expect("真实崩溃矩阵无法落盘 backing_up");
        trace("backing_up_written");
        if state == OperationState::BackingUp {
            trace("backing_up_exit");
            std::process::exit(86);
        }
        if let Some(shared_before) = shared_before {
            link_real_before_backup(&shared_before, &before_dir);
            // 父进程已完成双备份和独立完整性校验；子进程只复用硬链接，避免每个状态重复扫描大库。
            if std::env::var("TRAE_SYNC_REAL_CRASH_SHARED_BEFORE_VERIFIED").as_deref() != Ok("1") {
                assert!(verify_raw_backup(&raw_dir), "共享真实崩溃原始备份校验失败");
                assert!(
                    verify_logical_backup(&logical_db, &raw_key),
                    "共享真实崩溃逻辑备份校验失败"
                );
            }
        } else {
            assert!(
                capture_raw_backup(&db_path, &raw_dir),
                "真实崩溃矩阵原始备份失败"
            );
            assert!(
                backup_to_logical_copy_at_inner(&db_path, &raw_key, &logical_db)
                    && verify_logical_backup(&logical_db, &raw_key),
                "真实崩溃矩阵逻辑备份验证失败"
            );
        }
        trace("backup_linked_and_verified");
        journal
            .transition(OperationState::BackupVerified)
            .expect("真实崩溃矩阵无法落盘 backup_verified");
        trace("backup_verified_written");
        if state == OperationState::BackupVerified {
            trace("backup_verified_exit");
            std::process::exit(86);
        }
        journal
            .transition(OperationState::TargetWriting)
            .expect("真实崩溃矩阵无法落盘 target_writing");
        trace("target_writing_written");
        if state == OperationState::TargetWriting {
            real_process_write_marker(&db_path, &raw_key, &marker, true);
        }

        // 后续状态都建立在真实已提交 DML 上；每个分支在状态落盘后立即模拟进程终止。
        real_process_write_marker(&db_path, &raw_key, &marker, false);
        match state {
            OperationState::TargetCommittedUnverified => {
                journal
                    .transition(OperationState::TargetCommittedUnverified)
                    .unwrap();
            }
            OperationState::TargetVerifying => {
                journal
                    .transition(OperationState::TargetCommittedUnverified)
                    .unwrap();
                journal.transition(OperationState::TargetVerifying).unwrap();
            }
            OperationState::CatalogReconciling => {
                journal
                    .transition(OperationState::TargetCommittedUnverified)
                    .unwrap();
                journal.transition(OperationState::TargetVerifying).unwrap();
                let connection = real_open_query_only(&db_path, &raw_key);
                assert_eq!(
                    run_integrity_checks_on_connection(&connection),
                    (true, true),
                    "真实 clone 写后完整性检查失败"
                );
                drop(connection);
                let verified = target_file_evidence(&db_path).expect("真实 clone 缺少写后证据");
                journal.transition_catalog_reconciling(&verified).unwrap();
                assert!(reconcile_catalog_operation(
                    &storage,
                    &raw_key,
                    journal.operation_id(),
                    journal.data_location_id(),
                    1,
                    &verified,
                ));
            }
            OperationState::VerificationInconclusive => {
                journal
                    .transition(OperationState::TargetCommittedUnverified)
                    .unwrap();
                journal.transition(OperationState::TargetVerifying).unwrap();
                journal
                    .transition(OperationState::VerificationInconclusive)
                    .unwrap();
            }
            OperationState::FailurePreserving => {
                journal
                    .transition(OperationState::TargetCommittedUnverified)
                    .unwrap();
                journal.transition(OperationState::TargetVerifying).unwrap();
                journal
                    .transition(OperationState::FailurePreserving)
                    .unwrap();
            }
            OperationState::FailureSnapshotVerified => {
                journal
                    .transition(OperationState::TargetCommittedUnverified)
                    .unwrap();
                journal.transition(OperationState::TargetVerifying).unwrap();
                journal
                    .transition(OperationState::FailurePreserving)
                    .unwrap();
                assert!(capture_failure_evidence(
                    &db_path,
                    &storage,
                    journal.operation_id()
                ));
                journal
                    .transition(OperationState::FailureSnapshotVerified)
                    .unwrap();
            }
            OperationState::RestoreStaging
            | OperationState::RestoreStaged
            | OperationState::RestoreReplacing
            | OperationState::RestoredVerifying => {
                journal
                    .transition(OperationState::TargetCommittedUnverified)
                    .unwrap();
                journal.transition(OperationState::TargetVerifying).unwrap();
                journal
                    .transition(OperationState::FailurePreserving)
                    .unwrap();
                assert!(capture_failure_evidence(
                    &db_path,
                    &storage,
                    journal.operation_id()
                ));
                journal
                    .transition(OperationState::FailureSnapshotVerified)
                    .unwrap();
                journal.transition(OperationState::RestoreStaging).unwrap();
                if state == OperationState::RestoreStaging {
                    std::process::exit(86);
                }
                journal.transition(OperationState::RestoreStaged).unwrap();
                if state == OperationState::RestoreStaged {
                    std::process::exit(86);
                }
                journal
                    .transition(OperationState::RestoreReplacing)
                    .unwrap();
                if state == OperationState::RestoreReplacing {
                    std::process::exit(86);
                }
                journal
                    .transition(OperationState::RestoredVerifying)
                    .unwrap();
            }
            other => panic!("真实崩溃矩阵未处理状态: {other:?}"),
        }
        std::process::exit(86);
    }

    /// 归档真实崩溃现场时，重复字节通过硬链接复用；每个操作路径仍保留可核对的现场入口。
    fn archive_real_bundle(
        source_dir: &Path,
        destination_dir: &Path,
        canonical_files: &mut [Option<PathBuf>],
    ) -> serde_json::Value {
        std::fs::create_dir_all(destination_dir).expect("无法创建真实崩溃证据归档目录");
        let manifest = read_raw_backup_manifest(source_dir).expect("真实崩溃原始备份清单无效");
        let mut hashes = serde_json::Map::new();
        for (index, file_name) in ["database.db", "database.db-wal", "database.db-shm"]
            .iter()
            .enumerate()
        {
            let source = source_dir.join(file_name);
            if !source.is_file() {
                continue;
            }
            let expected_hash = manifest
                .get(*file_name)
                .expect("真实崩溃原始备份清单缺少文件")
                .clone();
            let destination = destination_dir.join(file_name);
            if let Some(canonical) = &canonical_files[index] {
                std::fs::hard_link(canonical, &destination)
                    .expect("无法为重复真实崩溃证据建立硬链接");
            } else {
                // 同一隔离根内优先硬链接；跨卷等无法建立硬链接时才复制并复核哈希。
                if std::fs::hard_link(&source, &destination).is_err() {
                    std::fs::copy(&source, &destination).expect("无法归档真实崩溃证据");
                    assert_eq!(
                        sha256_file_for_backup(&destination).as_deref(),
                        Some(expected_hash.as_str()),
                        "真实崩溃证据归档复制后哈希不一致"
                    );
                }
                canonical_files[index] = Some(destination.clone());
            }
            hashes.insert(
                (*file_name).to_string(),
                serde_json::Value::String(expected_hash),
            );
        }
        if source_dir.join("hashes.sha256").is_file() {
            std::fs::copy(
                source_dir.join("hashes.sha256"),
                destination_dir.join("hashes.sha256"),
            )
            .expect("无法归档真实崩溃哈希清单");
        }
        serde_json::Value::Object(hashes)
    }

    /// 真实 clone 崩溃恢复矩阵：每个非终态都由独立子进程落盘后终止，再由父进程重启协调。
    #[test]
    #[ignore = "仅在用户授权的真实隔离副本上显式运行"]
    fn real_clone_process_crash_recovery_matrix_acceptance() {
        let root = PathBuf::from(
            std::env::var("TRAE_SYNC_REAL_CRASH_ROOT").expect("缺少真实崩溃矩阵根目录环境变量"),
        );
        let raw_key = std::env::var("TRAE_SYNC_REAL_CRASH_RAW_KEY")
            .expect("缺少真实崩溃矩阵 raw key 环境变量");
        let relative_dir = std::env::var("TRAE_SYNC_REAL_CRASH_DB_RELATIVE")
            .unwrap_or_else(|_| "clone".to_string());
        assert_eq!(raw_key.len(), 64, "raw key 必须是 32 字节 hex");
        assert!(
            raw_key.chars().all(|value| value.is_ascii_hexdigit()),
            "raw key 必须是 hex"
        );

        let guard = FixturePathGuard::new(&root).expect("真实崩溃矩阵根目录未通过隔离 guard");
        let source_db = guard
            .validate_db_relative_path(&format!("{relative_dir}/database.db"))
            .expect("真实崩溃矩阵源数据库路径未通过隔离 guard");
        let source_dir = source_db.parent().expect("真实崩溃矩阵源目录缺失");
        for file_name in ["database.db", "database.db-wal", "database.db-shm"] {
            assert!(
                source_dir.join(file_name).is_file(),
                "真实崩溃矩阵源三件套缺少 {file_name}"
            );
        }
        let baseline = target_file_evidence(&source_db).expect("真实崩溃矩阵无法读取源哈希");
        for backup_name in ["backup-a", "backup-b"] {
            let backup_db = root.join(backup_name).join("database.db");
            assert_eq!(
                sha256_file_for_backup(&backup_db).as_deref(),
                Some(baseline.db_fingerprint.as_str()),
                "{backup_name} database.db 与 clone 基线不一致"
            );
            for (suffix, expected) in [
                ("-wal", baseline.wal_fingerprint.as_ref()),
                ("-shm", baseline.shm_fingerprint.as_ref()),
            ] {
                let path = database_sidecar_path(&backup_db, suffix);
                match expected {
                    Some(expected) => assert_eq!(
                        sha256_file_for_backup(&path).as_deref(),
                        Some(expected.as_str()),
                        "{backup_name} sidecar 与 clone 基线不一致"
                    ),
                    None => assert!(!path.exists(), "{backup_name} 不应凭空存在 {suffix}"),
                }
            }
        }

        let working = tempfile::Builder::new()
            .prefix("real-process-crash-matrix-")
            .tempdir_in(guard.canonical_root())
            .expect("无法创建真实崩溃矩阵工作目录");
        let run_id = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("系统时间异常")
            .as_nanos();
        let archive_root = guard
            .canonical_root()
            .join(format!("real-crash-evidence-{run_id}"));
        std::fs::create_dir(&archive_root).expect("无法创建真实崩溃失败证据归档根");
        let shared_before = archive_root.join("shared-before");
        let shared_raw = shared_before.join("raw");
        let shared_logical = shared_before.join("logical").join("database.db");
        assert!(
            capture_raw_backup(&source_db, &shared_raw),
            "真实矩阵共享原始备份失败"
        );
        assert!(
            backup_to_logical_copy_at_inner(&source_db, &raw_key, &shared_logical)
                && verify_raw_backup(&shared_raw)
                && verify_logical_backup(&shared_logical, &raw_key),
            "真实矩阵共享逻辑备份验证失败"
        );
        let marker = format!("T07-REAL-CRASH-MATRIX-{run_id}");
        let baseline_json = serde_json::to_string(&baseline).expect("无法序列化真实崩溃初始证据");
        let cases = [
            ("planned", OperationState::NotApplied, false, false),
            ("backing_up", OperationState::NotApplied, false, false),
            ("backup_verified", OperationState::NotApplied, false, false),
            (
                "target_writing",
                OperationState::ManualRecoveryRequired,
                true,
                false,
            ),
            (
                "target_committed_unverified",
                OperationState::ManualRecoveryRequired,
                true,
                true,
            ),
            (
                "target_verifying",
                OperationState::ManualRecoveryRequired,
                true,
                true,
            ),
            (
                "catalog_reconciling",
                OperationState::Completed,
                false,
                true,
            ),
            (
                "verification_inconclusive",
                OperationState::ManualRecoveryRequired,
                true,
                true,
            ),
            (
                "failure_preserving",
                OperationState::ManualRecoveryRequired,
                true,
                true,
            ),
            (
                "failure_snapshot_verified",
                OperationState::ManualRecoveryRequired,
                true,
                true,
            ),
            (
                "restore_staging",
                OperationState::ManualRecoveryRequired,
                true,
                true,
            ),
            (
                "restore_staged",
                OperationState::ManualRecoveryRequired,
                true,
                true,
            ),
            (
                "restore_replacing",
                OperationState::ManualRecoveryRequired,
                true,
                true,
            ),
            (
                "restored_verifying",
                OperationState::ManualRecoveryRequired,
                true,
                true,
            ),
        ];
        // 未提交回滚与已提交写入产生不同失败现场，必须分别保留 canonical 证据。
        let mut canonical_failure_files = [vec![None, None, None], vec![None, None, None]];
        let mut reusable_failure_roots: [Option<PathBuf>; 2] = [None, None];
        let mut canonical_before_files = vec![
            Some(shared_raw.join("database.db")),
            Some(shared_raw.join("database.db-wal")),
            Some(shared_raw.join("database.db-shm")),
        ];
        let canonical_logical: Option<PathBuf> = Some(shared_logical.clone());
        let mut results = Vec::new();
        let matrix_started = std::time::Instant::now();
        let trace_matrix = |state: &str, step: &str| {
            eprintln!(
                "REAL_CRASH_MATRIX state={state} step={step} elapsed_ms={}",
                matrix_started.elapsed().as_millis()
            );
        };

        for (state_name, expected_terminal, expects_failure, expects_marker) in cases {
            trace_matrix(state_name, "case_start");
            let case_root = working.path().join(state_name);
            let case_clone = case_root.join("clone");
            let storage = case_root.join("storage");
            std::fs::create_dir_all(&case_root).expect("无法创建真实崩溃用例目录");
            copy_real_trio(source_dir, &case_clone);
            std::fs::create_dir_all(&storage).expect("无法创建真实崩溃用例恢复区");
            let child = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "sqlcipher::tests::real_clone_process_crash_child_after_persisting_nonterminal_state",
                    "--ignored",
                    "--nocapture",
                ])
                .env("TRAE_SYNC_REAL_CRASH_ROOT", &case_root)
                .env("TRAE_SYNC_REAL_CRASH_RAW_KEY", &raw_key)
                .env("TRAE_SYNC_REAL_CRASH_DB_RELATIVE", "clone")
                .env("TRAE_SYNC_REAL_CRASH_STATE", state_name)
                .env("TRAE_SYNC_REAL_CRASH_STORAGE", &storage)
                .env("TRAE_SYNC_REAL_CRASH_MARKER", &marker)
                .env("TRAE_SYNC_REAL_CRASH_SHARED_BEFORE", &shared_before)
                .env("TRAE_SYNC_REAL_CRASH_SHARED_BEFORE_VERIFIED", "1")
                .env("TRAE_SYNC_REAL_CRASH_INITIAL_EVIDENCE", &baseline_json)
                .status()
                .expect("无法启动真实崩溃子进程");
            assert_eq!(child.code(), Some(86), "真实崩溃子进程必须在状态落盘后终止");
            trace_matrix(state_name, "child_finished");

            let summaries = crate::operation_manifest::list_operation_summaries(&storage)
                .expect("无法读取真实崩溃操作 manifest");
            assert_eq!(
                summaries.len(),
                1,
                "每个真实崩溃用例只能产生一个操作 manifest"
            );
            assert_eq!(summaries[0].state, real_process_crash_state(state_name));
            let operation_id = summaries[0].operation_id.clone();
            let operation_root = storage.join("backups").join(&operation_id);
            let archive_case = archive_root.join(state_name);
            std::fs::create_dir_all(&archive_case).expect("无法创建真实崩溃用例归档目录");
            let result_index = results.len();
            results.push(serde_json::json!({
                "state": state_name,
                "operation_id": operation_id,
            }));
            let before_raw = operation_root.join("before").join("raw");
            let before_logical = operation_root.join("before").join("logical");
            if before_raw.is_dir() {
                assert!(
                    read_raw_backup_manifest(&before_raw).is_some(),
                    "真实崩溃写前原始备份清单无效"
                );
                let archive_before = archive_root.join(state_name).join("before/raw");
                let before_hashes =
                    archive_real_bundle(&before_raw, &archive_before, &mut canonical_before_files);
                if before_logical.join("database.db").is_file() {
                    let archive_logical = archive_root.join(state_name).join("before/logical");
                    std::fs::create_dir_all(&archive_logical).unwrap();
                    let destination = archive_logical.join("database.db");
                    std::fs::hard_link(canonical_logical.as_ref().unwrap(), &destination)
                        .expect("无法复用真实崩溃逻辑备份");
                }
                results[result_index]["before_backup"] = before_hashes;
            }

            let target_db = case_clone.join("database.db");
            let marker_project_id = if expects_marker || state_name == "target_writing" {
                Some(
                    std::fs::read_to_string(case_root.join("crash-project-id.txt"))
                        .expect("真实崩溃缺少目标项目 ID")
                        .trim()
                        .to_string(),
                )
            } else {
                None
            };
            let target_marker_count = if let Some(project_id) = marker_project_id.as_deref() {
                let connection = real_open_query_only(&target_db, &raw_key);
                // 未提交写入后的回滚必须逐项回到已完成双备份和逻辑校验的 baseline；
                // 唯一成功状态的两层全库检查在子进程中完成，其余失败状态保留现场哈希。
                if state_name == "target_writing" {
                    let rollback_evidence =
                        target_file_evidence(&target_db).expect("真实崩溃回滚后目标数据库缺失");
                    assert_eq!(
                        rollback_evidence.db_fingerprint, baseline.db_fingerprint,
                        "真实崩溃后未提交写入未回到写前 database.db"
                    );
                    assert_eq!(
                        rollback_evidence.wal_fingerprint, baseline.wal_fingerprint,
                        "真实崩溃后未提交写入未回到写前 WAL"
                    );
                    // SHM 只保存 SQLite 锁与读标记，进程重启时允许被 SQLite 重新生成或更新。
                }
                connection
                    .query_row(
                        "SELECT COUNT(*) FROM project WHERE project_id = ?1 AND description LIKE '%' || ?2 || '%'",
                        rusqlite::params![project_id, &marker],
                        |row| row.get::<_, i64>(0),
                    )
                    .expect("真实崩溃后目标标记读取失败")
            } else {
                0
            };
            assert_eq!(target_marker_count, i64::from(expects_marker));
            trace_matrix(state_name, "target_checked");

            let mut verify_calls = 0_u32;
            let mut capture_calls = 0_u32;
            let failure_kind = usize::from(expects_marker);
            let reusable_failure_root = reusable_failure_roots[failure_kind].clone();
            assert!(reconcile_unfinished_manifests_with_recovery_handlers(
                &storage,
                |journal| {
                    verify_calls += 1;
                    journal.data_location_id() == "real-isolated-t07-process-crash"
                        && journal
                            .verified_target_file_evidence()
                            .is_some_and(|evidence| {
                                target_file_evidence(&target_db).as_ref() == Some(evidence)
                                    && catalog_operation_matches(
                                        &storage,
                                        &raw_key,
                                        journal.operation_id(),
                                        journal.data_location_id(),
                                        evidence,
                                    )
                            })
                },
                |journal| {
                    capture_calls += 1;
                    journal.data_location_id() == "real-isolated-t07-process-crash"
                        && if let Some(canonical_raw_dir) = &reusable_failure_root {
                            link_reused_failure_evidence(
                                journal,
                                canonical_raw_dir,
                                &storage,
                                journal.operation_id(),
                            )
                        } else {
                            preserve_failure_evidence(
                                journal,
                                &target_db,
                                &storage,
                                journal.operation_id(),
                            )
                        }
                },
            ));
            trace_matrix(state_name, "recovery_reconciled");
            let terminal = crate::operation_manifest::list_operation_summaries(&storage)
                .expect("无法读取真实崩溃终态 manifest");
            assert_eq!(terminal[0].state, expected_terminal);
            assert_eq!(
                verify_calls,
                if state_name == "catalog_reconciling" {
                    1
                } else {
                    0
                }
            );
            assert_eq!(capture_calls, if expects_failure { 1 } else { 0 });

            let failure_raw = operation_root.join("failure").join("raw");
            if expects_failure {
                assert!(failure_raw.is_dir(), "真实崩溃失败现场未保留");
                assert!(
                    read_raw_backup_manifest(&failure_raw).is_some(),
                    "真实崩溃失败现场清单无效"
                );
                let failure_hashes = archive_real_bundle(
                    &failure_raw,
                    &archive_root.join(state_name).join("failure/raw"),
                    &mut canonical_failure_files[failure_kind],
                );
                if reusable_failure_roots[failure_kind].is_none() {
                    reusable_failure_roots[failure_kind] =
                        Some(archive_root.join(state_name).join("failure/raw"));
                }
                let failure_marker_count = {
                    let connection =
                        real_open_query_only(&failure_raw.join("database.db"), &raw_key);
                    connection
                        .query_row(
                            "SELECT COUNT(*) FROM project WHERE project_id = ?1 AND description LIKE '%' || ?2 || '%'",
                            rusqlite::params![
                                marker_project_id
                                    .as_deref()
                                    .expect("真实崩溃失败现场缺少目标项目 ID"),
                                &marker
                            ],
                            |row| row.get::<_, i64>(0),
                        )
                        .expect("真实崩溃失败现场标记读取失败")
                };
                assert_eq!(failure_marker_count, i64::from(expects_marker));
                results[result_index]["failure_scene"] = failure_hashes;
            }
            trace_matrix(state_name, "evidence_archived");

            let record_count = std::fs::read_dir(storage.join("operations").join(&operation_id))
                .expect("无法读取真实崩溃 manifest 记录")
                .count();
            assert!(reconcile_unfinished_manifests_with_recovery_handlers(
                &storage,
                |journal| panic!(
                    "终态真实崩溃 manifest 不得再次验证: {}",
                    journal.operation_id()
                ),
                |journal| panic!(
                    "终态真实崩溃 manifest 不得再次捕获: {}",
                    journal.operation_id()
                ),
            ));
            assert_eq!(
                std::fs::read_dir(storage.join("operations").join(&operation_id))
                    .unwrap()
                    .count(),
                record_count,
                "真实崩溃终态重启不得追加记录"
            );
            std::fs::write(
                archive_case.join("operation.json"),
                serde_json::to_vec_pretty(&serde_json::json!({
                    "state": state_name,
                    "operation_id": operation_id,
                    "expected_terminal": format!("{expected_terminal:?}"),
                    "target_marker_count": target_marker_count,
                    "failure_scene": expects_failure,
                    "archive_root": archive_root,
                }))
                .unwrap(),
            )
            .expect("无法写入真实崩溃用例摘要");
            std::fs::remove_dir_all(&case_root).expect("无法清理真实崩溃用例工作副本");
            trace_matrix(state_name, "case_cleaned");
        }

        println!(
            "REAL_CLONE_PROCESS_CRASH_MATRIX={}",
            serde_json::json!({
                "data_location_id": "real-isolated-t07-process-crash",
                "states": results,
                "archive_root": archive_root,
                "source_clone_unchanged": target_file_evidence(&source_db) == Some(baseline),
                "original_data_directory_written": false,
                "double_backup_baseline_verified": true,
            })
        );
    }

    /// 真实隔离副本恢复状态的最小路径；完整 14 状态进程崩溃矩阵由 fixture 覆盖。
    fn transition_real_recovery_state(
        journal: &OperationManifestJournal,
        target: OperationState,
        evidence: &TargetFileEvidence,
    ) {
        if target == OperationState::Planned {
            return;
        }
        for state in [
            OperationState::BackingUp,
            OperationState::BackupVerified,
            OperationState::TargetWriting,
            OperationState::TargetCommittedUnverified,
            OperationState::TargetVerifying,
        ] {
            if state == target {
                journal.transition(state).unwrap();
                return;
            }
            journal.transition(state).unwrap();
        }
        if target == OperationState::CatalogReconciling {
            journal.transition_catalog_reconciling(evidence).unwrap();
            return;
        }
        assert_eq!(target, OperationState::Planned);
    }

    #[test]
    #[ignore = "仅在用户授权的真实隔离副本上显式运行"]
    fn real_clone_recovery_matrix_acceptance() {
        let root = PathBuf::from(
            std::env::var("TRAE_SYNC_REAL_RECOVERY_ROOT").expect("缺少真实隔离恢复根目录环境变量"),
        );
        let raw_key = std::env::var("TRAE_SYNC_REAL_RECOVERY_RAW_KEY")
            .expect("缺少真实隔离恢复 raw key 环境变量");
        let relative_dir = std::env::var("TRAE_SYNC_REAL_RECOVERY_DB_RELATIVE")
            .unwrap_or_else(|_| "clone".to_string());
        assert_eq!(raw_key.len(), 64, "raw key 必须是 32 字节 hex");
        assert!(
            raw_key.chars().all(|value| value.is_ascii_hexdigit()),
            "raw key 必须是 hex"
        );

        let guard = FixturePathGuard::new(&root).expect("真实恢复根目录未通过隔离 guard");
        let source_db = guard
            .validate_db_relative_path(&format!("{relative_dir}/database.db"))
            .expect("真实恢复源数据库路径未通过隔离 guard");
        let source_dir = source_db.parent().unwrap();
        let working = tempfile::Builder::new()
            .prefix("real-recovery-matrix-")
            .tempdir_in(guard.canonical_root())
            .expect("无法创建真实恢复临时根");
        let read_dir = working.path().join("read-trio");
        copy_real_trio(source_dir, &read_dir);
        let read_db = read_dir.join("database.db");
        let evidence = target_file_evidence(&read_db).expect("真实恢复副本缺少 database.db");

        // 逻辑备份只生成一次；各状态使用硬链接复用只读证据，不堆积整库副本。
        let logical_master = working.path().join("logical-master.db");
        assert!(backup_to_logical_copy_at_inner(
            &read_db,
            &raw_key,
            &logical_master
        ));
        assert!(verify_logical_backup(&logical_master, &raw_key));

        let cases = [
            ("planned", OperationState::Planned),
            ("backing_up", OperationState::BackingUp),
            ("backup_verified", OperationState::BackupVerified),
            ("target_writing", OperationState::TargetWriting),
            (
                "target_committed_unverified",
                OperationState::TargetCommittedUnverified,
            ),
            ("target_verifying", OperationState::TargetVerifying),
            ("catalog_reconciling", OperationState::CatalogReconciling),
        ];
        let mut results = Vec::new();

        for (name, state) in cases {
            let storage = working.path().join(format!("storage-{name}"));
            std::fs::create_dir_all(storage.join("backups")).unwrap();
            let operation_id = traesync_domain::OperationId::new();
            let journal = OperationManifestJournal::create(
                &storage,
                &operation_id,
                "real-isolated-t07-recovery",
                &evidence,
            )
            .unwrap();
            let before_raw = storage
                .join("backups")
                .join(operation_id.as_str())
                .join("before")
                .join("raw");
            let before_logical = before_raw.parent().unwrap().join("logical");
            assert!(link_real_trio(&read_dir, &before_raw));
            std::fs::create_dir_all(&before_logical).unwrap();
            assert!(
                std::fs::hard_link(&logical_master, before_logical.join("database.db")).is_ok()
            );
            transition_real_recovery_state(&journal, state, &evidence);

            let mut captured = false;
            assert!(reconcile_unfinished_manifests_with_recovery_handlers(
                &storage,
                |_| false,
                |current| {
                    captured = true;
                    link_real_failure_scene(&read_dir, &storage, current.operation_id())
                }
            ));
            let expected = match state {
                OperationState::Planned
                | OperationState::BackingUp
                | OperationState::BackupVerified => OperationState::NotApplied,
                _ => OperationState::ManualRecoveryRequired,
            };
            assert_eq!(journal.latest_state(), Some(expected));
            assert_eq!(captured, !matches!(expected, OperationState::NotApplied));

            // 终态重启不得再次捕获现场或改变结果。
            assert!(reconcile_unfinished_manifests_with_recovery_handlers(
                &storage,
                |_| panic!("真实恢复终态不得再次验证目录库"),
                |_| panic!("真实恢复终态不得再次覆盖失败现场")
            ));
            assert_eq!(target_file_evidence(&read_db), Some(evidence.clone()));
            results.push(serde_json::json!({
                "state": name,
                "terminal": format!("{expected:?}"),
                "failure_scene": captured,
            }));
        }

        println!(
            "REAL_CLONE_RECOVERY_MATRIX={}",
            serde_json::json!({
                "data_location_id": "real-isolated-t07-recovery",
                "states": results,
                "logical_backup_verified": true,
                "source_trio_unchanged": target_file_evidence(&read_db) == Some(evidence),
                "original_data_directory_written": false,
            })
        );
    }

    #[test]
    fn reconciliation_does_not_overwrite_existing_failure_scene_after_interruption() {
        for state in [
            OperationState::FailurePreserving,
            OperationState::FailureSnapshotVerified,
        ] {
            let target = tempdir().unwrap();
            let storage = tempdir().unwrap();
            let db_path = make_attach_sessions_fixture(target.path());
            let plan = attach_sessions_plan(&db_path, &["s1"]);
            let operation_id = traesync_domain::OperationId::new();
            let journal = crate::operation_manifest::OperationManifestJournal::create(
                storage.path(),
                &operation_id,
                "fixture-location",
                plan.target_file_evidence(),
            )
            .unwrap();
            journal.transition(OperationState::BackingUp).unwrap();
            journal.transition(OperationState::BackupVerified).unwrap();
            journal.transition(OperationState::TargetWriting).unwrap();
            journal
                .transition(OperationState::TargetCommittedUnverified)
                .unwrap();
            journal.transition(OperationState::TargetVerifying).unwrap();
            journal
                .transition(OperationState::FailurePreserving)
                .unwrap();
            assert!(capture_failure_evidence(
                &db_path,
                storage.path(),
                operation_id.as_str()
            ));
            if state == OperationState::FailureSnapshotVerified {
                journal
                    .transition(OperationState::FailureSnapshotVerified)
                    .unwrap();
            }
            let failure_raw = storage
                .path()
                .join("backups")
                .join(operation_id.as_str())
                .join("failure")
                .join("raw");
            let first_failure_scene = snapshot_db_trio(&failure_raw, "database.db");
            let conn = open_with_key(&db_path, TEST_RAW_KEY).unwrap();
            conn.execute(
                "UPDATE chat_message SET body = ?1 WHERE message_id = ?2",
                params!["changed-after-crash", "m1"],
            )
            .unwrap();
            drop(conn);

            let outcome = fixture_executor(
                &WorkCnSyncExecutor::new(TEST_RAW_KEY),
                &db_path,
                storage.path(),
            )
            .execute_sync_plan(
                &plan,
                &OperationCancellation::new(),
                &SequencedEvidence {
                    current_through: usize::MAX,
                    calls: AtomicUsize::new(0),
                },
            );

            assert_eq!(
                outcome,
                SyncPlanExecutionOutcome::ManualRecoveryRequired {
                    backups_preserved: false
                }
            );
            assert!(verify_raw_backup(&failure_raw));
            assert_eq!(
                snapshot_db_trio(&failure_raw, "database.db"),
                first_failure_scene,
                "{state:?} 重启不得覆盖首份失败现场"
            );
        }
    }

    #[test]
    fn reused_failure_evidence_resumes_failure_preserving_state() {
        let target = tempdir().unwrap();
        let canonical = tempdir().unwrap();
        let storage = tempdir().unwrap();
        let db_path = make_attach_sessions_fixture(target.path());
        let canonical_raw = canonical.path().join("raw");
        assert!(capture_raw_backup(&db_path, &canonical_raw));

        let plan = attach_sessions_plan(&db_path, &["s1"]);
        let operation_id = traesync_domain::OperationId::new();
        let journal = OperationManifestJournal::create(
            storage.path(),
            &operation_id,
            "fixture-location",
            plan.target_file_evidence(),
        )
        .unwrap();
        journal.transition(OperationState::BackingUp).unwrap();
        journal.transition(OperationState::BackupVerified).unwrap();
        journal.transition(OperationState::TargetWriting).unwrap();
        journal
            .transition(OperationState::TargetCommittedUnverified)
            .unwrap();
        journal.transition(OperationState::TargetVerifying).unwrap();
        journal
            .transition(OperationState::FailurePreserving)
            .unwrap();

        assert!(link_reused_failure_evidence(
            &journal,
            &canonical_raw,
            storage.path(),
            operation_id.as_str(),
        ));
        assert_eq!(
            journal.latest_state(),
            Some(OperationState::FailureSnapshotVerified)
        );
        let failure_raw = storage
            .path()
            .join("backups")
            .join(operation_id.as_str())
            .join("failure")
            .join("raw");
        assert!(verify_raw_backup(&failure_raw));
        assert_eq!(
            snapshot_db_trio(&failure_raw, "database.db"),
            snapshot_db_trio(&canonical_raw, "database.db")
        );
    }

    #[test]
    fn reused_failure_evidence_accepts_existing_verified_scene() {
        let target = tempdir().unwrap();
        let canonical = tempdir().unwrap();
        let storage = tempdir().unwrap();
        let db_path = make_attach_sessions_fixture(target.path());
        let canonical_raw = canonical.path().join("raw");
        assert!(capture_raw_backup(&db_path, &canonical_raw));

        let plan = attach_sessions_plan(&db_path, &["s1"]);
        let operation_id = traesync_domain::OperationId::new();
        let journal = OperationManifestJournal::create(
            storage.path(),
            &operation_id,
            "fixture-location",
            plan.target_file_evidence(),
        )
        .unwrap();
        journal.transition(OperationState::BackingUp).unwrap();
        journal.transition(OperationState::BackupVerified).unwrap();
        journal.transition(OperationState::TargetWriting).unwrap();
        journal
            .transition(OperationState::TargetCommittedUnverified)
            .unwrap();
        journal.transition(OperationState::TargetVerifying).unwrap();
        journal
            .transition(OperationState::FailurePreserving)
            .unwrap();
        let failure_raw = storage
            .path()
            .join("backups")
            .join(operation_id.as_str())
            .join("failure")
            .join("raw");
        assert!(capture_raw_backup(&db_path, &failure_raw));
        journal
            .transition(OperationState::FailureSnapshotVerified)
            .unwrap();
        let first_failure_scene = snapshot_db_trio(&failure_raw, "database.db");

        assert!(link_reused_failure_evidence(
            &journal,
            &canonical_raw,
            storage.path(),
            operation_id.as_str(),
        ));
        assert_eq!(
            journal.latest_state(),
            Some(OperationState::FailureSnapshotVerified)
        );
        assert_eq!(
            snapshot_db_trio(&failure_raw, "database.db"),
            first_failure_scene
        );
    }

    #[test]
    fn attach_sessions_rolls_back_when_evidence_drifts_before_commit() {
        let target = tempdir().unwrap();
        let storage = tempdir().unwrap();
        let db_path = make_attach_sessions_fixture(target.path());
        let before = snapshot_db_trio(target.path(), "database.db");
        let plan = attach_sessions_plan(&db_path, &["s1"]);
        let executor = WorkCnSyncExecutor::new(TEST_RAW_KEY);
        let evidence = SequencedEvidence {
            // 写前两次匹配，事务提交前复查变为漂移。
            current_through: 2,
            calls: AtomicUsize::new(0),
        };

        let outcome = fixture_executor(&executor, &db_path, storage.path()).execute_sync_plan(
            &plan,
            &OperationCancellation::new(),
            &evidence,
        );

        assert_eq!(
            outcome,
            SyncPlanExecutionOutcome::PlanExpired {
                backups_preserved: true
            }
        );
        assert_eq!(snapshot_db_trio(target.path(), "database.db"), before);
        let before_backup = storage
            .path()
            .join("backups")
            .join(plan.operation_id().as_str())
            .join("before");
        assert!(verify_raw_backup(&before_backup.join("raw")));
        assert!(verify_logical_backup(
            &before_backup.join("logical").join("database.db"),
            TEST_RAW_KEY
        ));
    }

    #[test]
    fn attach_sessions_preserves_failure_evidence_after_post_commit_relation_damage() {
        let target = tempdir().unwrap();
        let storage = tempdir().unwrap();
        let db_path = make_attach_sessions_fixture(target.path());
        let conn = open_with_key(&db_path, TEST_RAW_KEY).unwrap();
        // 模拟产品触发器在提交中损坏未允许改写的消息关系。
        conn.execute_batch(
            "CREATE TRIGGER delete_message_after_attach AFTER UPDATE ON local_artifact_version BEGIN DELETE FROM chat_message WHERE session_id = 's1'; END;",
        )
        .unwrap();
        drop(conn);
        let plan = attach_sessions_plan(&db_path, &["s1"]);
        let executor = WorkCnSyncExecutor::new(TEST_RAW_KEY);

        let outcome = fixture_executor(&executor, &db_path, storage.path()).execute_sync_plan(
            &plan,
            &OperationCancellation::new(),
            &SequencedEvidence {
                current_through: usize::MAX,
                calls: AtomicUsize::new(0),
            },
        );

        assert_eq!(
            outcome,
            SyncPlanExecutionOutcome::FailedAfterWrite {
                backups_preserved: true
            }
        );
        let backup_root = storage
            .path()
            .join("backups")
            .join(plan.operation_id().as_str());
        assert!(verify_raw_backup(&backup_root.join("before").join("raw")));
        assert!(verify_raw_backup(&backup_root.join("failure").join("raw")));
    }

    #[test]
    fn attach_sessions_preserves_failure_evidence_after_same_count_fts_mutation() {
        let target = tempdir().unwrap();
        let storage = tempdir().unwrap();
        let db_path = make_attach_sessions_fixture(target.path());
        let conn = open_with_key(&db_path, TEST_RAW_KEY).unwrap();
        // 触发器保持 FTS 行数不变，用于证明验证不能只比较计数。
        conn.execute_batch(
            "CREATE TRIGGER mutate_fts_after_attach AFTER UPDATE ON local_artifact_version BEGIN UPDATE chat_fts SET indexed_body = 'mutated' WHERE session_id = 's1'; END;",
        )
        .unwrap();
        drop(conn);
        let plan = attach_sessions_plan(&db_path, &["s1"]);

        let outcome = fixture_executor(
            &WorkCnSyncExecutor::new(TEST_RAW_KEY),
            &db_path,
            storage.path(),
        )
        .execute_sync_plan(
            &plan,
            &OperationCancellation::new(),
            &SequencedEvidence {
                current_through: usize::MAX,
                calls: AtomicUsize::new(0),
            },
        );

        assert_eq!(
            outcome,
            SyncPlanExecutionOutcome::FailedAfterWrite {
                backups_preserved: true
            }
        );
        let backup_root = storage
            .path()
            .join("backups")
            .join(plan.operation_id().as_str());
        assert!(verify_raw_backup(&backup_root.join("before").join("raw")));
        assert!(verify_raw_backup(&backup_root.join("failure").join("raw")));
    }

    #[test]
    fn attach_sessions_detects_without_rowid_table_mutation_after_write() {
        let target = tempdir().unwrap();
        let storage = tempdir().unwrap();
        let db_path = make_attach_sessions_fixture(target.path());
        let conn = open_with_key(&db_path, TEST_RAW_KEY).unwrap();
        // 模拟 FTS 阴影表在关系更新后被触发器篡改，行数保持不变。
        conn.execute_batch(
            "CREATE TABLE without_rowid_cache (cache_key TEXT PRIMARY KEY, value TEXT NOT NULL) WITHOUT ROWID; \
             INSERT INTO without_rowid_cache VALUES ('cache-1', 'unchanged'); \
             CREATE TRIGGER mutate_without_rowid_cache_after_attach AFTER UPDATE ON local_artifact_version BEGIN \
                UPDATE without_rowid_cache SET value = 'mutated' WHERE cache_key = 'cache-1'; \
             END;",
        )
        .unwrap();
        drop(conn);
        let plan = attach_sessions_plan(&db_path, &["s1"]);

        let outcome = fixture_executor(
            &WorkCnSyncExecutor::new(TEST_RAW_KEY),
            &db_path,
            storage.path(),
        )
        .execute_sync_plan(
            &plan,
            &OperationCancellation::new(),
            &SequencedEvidence {
                current_through: usize::MAX,
                calls: AtomicUsize::new(0),
            },
        );

        assert_eq!(
            outcome,
            SyncPlanExecutionOutcome::FailedAfterWrite {
                backups_preserved: true
            }
        );
        let backup_root = storage
            .path()
            .join("backups")
            .join(plan.operation_id().as_str());
        assert!(verify_raw_backup(&backup_root.join("before").join("raw")));
        assert!(verify_raw_backup(&backup_root.join("failure").join("raw")));
    }

    #[test]
    fn follow_project_creates_verified_dual_backups_and_updates_only_expected_owner() {
        let target = tempdir().unwrap();
        let storage = tempdir().unwrap();
        let db_path = make_work_cn_fixture_with_wal(target.path());
        let source_before = snapshot_db_trio(target.path(), "database.db");
        let metadata_before = session_message_metadata(&db_path);
        assert_ne!(metadata_before.0 .2, 0, "s1 deleted_at 必须为非零值");
        assert_ne!(metadata_before.1 .2, 0, "m1 deleted_at 必须为非零值");
        let operation_id = traesync_domain::OperationId::new();
        let executor = WorkCnSyncExecutor::new(TEST_RAW_KEY);

        let result = executor.execute_follow_project_inner(
            &db_path,
            storage.path(),
            &operation_id,
            "p1",
            "1000000000000001",
            "2000000000000002",
        );

        assert_eq!(
            result,
            FollowProjectExecution::Completed { affected_rows: 1 },
            "完整项目跟随应报告单行更新成功"
        );

        let before_dir = storage
            .path()
            .join("backups")
            .join(operation_id.as_str())
            .join("before");
        let raw_dir = before_dir.join("raw");
        assert!(
            verify_raw_backup(&raw_dir),
            "原始备份哈希清单必须独立可验证"
        );
        assert_eq!(
            std::fs::read(raw_dir.join("database.db")).unwrap(),
            source_before.0,
            "原始 DB 备份必须保留写入前字节"
        );
        if !source_before.1.is_empty() {
            assert_eq!(
                std::fs::read(raw_dir.join("database.db-wal")).unwrap(),
                source_before.1,
                "原始 WAL 备份必须保留写入前字节"
            );
        }
        if !source_before.2.is_empty() {
            assert_eq!(
                std::fs::read(raw_dir.join("database.db-shm")).unwrap(),
                source_before.2,
                "原始 SHM 备份必须保留写入前字节"
            );
        }

        let logical_db_path = before_dir.join("logical").join("database.db");
        assert!(verify_logical_backup(&logical_db_path, TEST_RAW_KEY));
        assert_eq!(
            project_owner(&logical_db_path, "p1"),
            Some("1000000000000001".to_string()),
            "逻辑副本必须保留写入前 owner"
        );
        assert_eq!(
            project_owner(&db_path, "p1"),
            Some("2000000000000002".to_string()),
            "目标库只应更新计划指定项目的 owner"
        );
        let metadata_after = session_message_metadata(&db_path);
        assert_eq!(
            metadata_after, metadata_before,
            "会话和消息删除标记及关联字段不得被项目跟随改写"
        );
    }

    #[test]
    fn plan_execution_cancels_after_backup_without_writing_target() {
        let target = tempdir().unwrap();
        let storage = tempdir().unwrap();
        let db_path = make_work_cn_fixture(target.path());
        let plan = follow_project_plan(&db_path);
        let cancellation = OperationCancellation::new();
        cancellation.request();
        let executor = WorkCnSyncExecutor::new(TEST_RAW_KEY);
        let evidence = SequencedEvidence {
            current_through: usize::MAX,
            calls: AtomicUsize::new(0),
        };

        let outcome = fixture_executor(&executor, &db_path, storage.path()).execute_sync_plan(
            &plan,
            &cancellation,
            &evidence,
        );

        assert_eq!(
            outcome,
            SyncPlanExecutionOutcome::CancelledBeforeWrite {
                backups_preserved: true
            }
        );
        assert_eq!(
            project_owner(&db_path, "p1"),
            Some("1000000000000001".to_string()),
            "写前取消不得修改项目归属"
        );
        let before = storage
            .path()
            .join("backups")
            .join(plan.operation_id().as_str())
            .join("before");
        assert!(verify_raw_backup(&before.join("raw")));
        assert!(verify_logical_backup(
            &before.join("logical").join("database.db"),
            TEST_RAW_KEY
        ));
    }

    #[test]
    fn plan_execution_rejects_drift_after_backup_before_target_write() {
        let target = tempdir().unwrap();
        let storage = tempdir().unwrap();
        let db_path = make_work_cn_fixture(target.path());
        let plan = follow_project_plan(&db_path);
        let executor = WorkCnSyncExecutor::new(TEST_RAW_KEY);
        let evidence = SequencedEvidence {
            // 首次进入执行器为当前；备份完成后的复查变为漂移。
            current_through: 1,
            calls: AtomicUsize::new(0),
        };

        let outcome = fixture_executor(&executor, &db_path, storage.path()).execute_sync_plan(
            &plan,
            &OperationCancellation::new(),
            &evidence,
        );

        assert_eq!(
            outcome,
            SyncPlanExecutionOutcome::PlanExpired {
                backups_preserved: true
            }
        );
        assert_eq!(
            project_owner(&db_path, "p1"),
            Some("1000000000000001".to_string())
        );
        assert!(verify_raw_backup(
            &storage
                .path()
                .join("backups")
                .join(plan.operation_id().as_str())
                .join("before")
                .join("raw")
        ));
    }

    #[test]
    fn executor_rejects_same_content_from_different_location_without_side_effects() {
        let source = tempdir().unwrap();
        let replacement = tempdir().unwrap();
        let storage = tempdir().unwrap();
        let source_db = make_work_cn_fixture(source.path());
        let replacement_db = make_work_cn_fixture(replacement.path());
        let plan = follow_project_plan(&source_db);
        let before = snapshot_db_trio(replacement.path(), "database.db");

        // 两份数据库字节相同，但根目录和数据库文件身份不同；旧计划不能指向替换目标。
        let outcome = fixture_executor(
            &WorkCnSyncExecutor::new(TEST_RAW_KEY),
            &replacement_db,
            storage.path(),
        )
        .execute_sync_plan(
            &plan,
            &OperationCancellation::new(),
            &SequencedEvidence {
                current_through: usize::MAX,
                calls: AtomicUsize::new(0),
            },
        );

        assert_eq!(
            outcome,
            SyncPlanExecutionOutcome::PlanExpired {
                backups_preserved: false
            }
        );
        assert_eq!(snapshot_db_trio(replacement.path(), "database.db"), before);
        assert!(!storage.path().join("operations").exists());
        assert!(!storage
            .path()
            .join("backups")
            .join(plan.operation_id().as_str())
            .exists());
    }

    #[test]
    fn completed_operation_keeps_location_id_across_manifest_catalog_and_backup() {
        let target = tempdir().unwrap();
        let storage = tempdir().unwrap();
        let db_path = make_work_cn_fixture(target.path());
        let plan = follow_project_plan(&db_path);

        let outcome = fixture_executor(
            &WorkCnSyncExecutor::new(TEST_RAW_KEY),
            &db_path,
            storage.path(),
        )
        .execute_sync_plan(
            &plan,
            &OperationCancellation::new(),
            &SequencedEvidence {
                current_through: usize::MAX,
                calls: AtomicUsize::new(0),
            },
        );
        assert!(matches!(
            outcome,
            SyncPlanExecutionOutcome::Completed { affected_rows: 1 }
        ));

        let summaries = crate::list_operation_summaries(storage.path()).unwrap();
        assert_eq!(summaries.len(), 1);
        assert_eq!(summaries[0].data_location_id, plan.data_location_id());

        let catalog_path = resolve_current_catalog_path(storage.path()).unwrap();
        let catalog = open_with_key(&catalog_path, TEST_RAW_KEY).unwrap();
        let catalog_location_id: String = catalog
            .query_row(
                "SELECT data_location_id FROM operation_record WHERE operation_id = ?1",
                params![plan.operation_id().as_str()],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(catalog_location_id, plan.data_location_id());

        let before_raw = storage
            .path()
            .join("backups")
            .join(plan.operation_id().as_str())
            .join("before/raw");
        assert!(verify_raw_backup(&before_raw));
        assert_eq!(
            sha256_file_for_backup(&before_raw.join("database.db")).as_deref(),
            Some(plan.target_file_evidence().db_fingerprint.as_str())
        );
    }

    #[test]
    fn plan_execution_keeps_failure_scene_when_post_commit_verification_fails() {
        let target = tempdir().unwrap();
        let storage = tempdir().unwrap();
        let db_path = make_work_cn_fixture(target.path());
        {
            let conn = open_with_key(&db_path, TEST_RAW_KEY).unwrap();
            // fixture 触发器模拟提交后会话关系被非预期改写。
            conn.execute_batch(
                "CREATE TRIGGER remove_followed_sessions AFTER UPDATE ON project BEGIN DELETE FROM chat_session WHERE project_id = NEW.project_id; END;",
            )
            .unwrap();
        }
        // 触发器属于 fixture 初始状态，计划必须在该状态固定目标指纹。
        let plan = follow_project_plan(&db_path);
        let executor = WorkCnSyncExecutor::new(TEST_RAW_KEY);
        let evidence = SequencedEvidence {
            current_through: usize::MAX,
            calls: AtomicUsize::new(0),
        };

        let outcome = fixture_executor(&executor, &db_path, storage.path()).execute_sync_plan(
            &plan,
            &OperationCancellation::new(),
            &evidence,
        );

        assert_eq!(
            outcome,
            SyncPlanExecutionOutcome::FailedAfterWrite {
                backups_preserved: true
            }
        );
        let backup_root = storage
            .path()
            .join("backups")
            .join(plan.operation_id().as_str());
        assert!(verify_raw_backup(&backup_root.join("before").join("raw")));
        assert!(verify_raw_backup(&backup_root.join("failure").join("raw")));
    }

    #[test]
    fn plan_execution_requires_manual_recovery_when_post_commit_evidence_drifts() {
        let target = tempdir().unwrap();
        let storage = tempdir().unwrap();
        let db_path = make_work_cn_fixture(target.path());
        let plan = follow_project_plan(&db_path);
        let snapshots = Arc::new(Mutex::new(Vec::<ProgressSnapshot>::new()));
        let snapshots_for_reporter = Arc::clone(&snapshots);
        let executor = WorkCnSyncExecutor::new(TEST_RAW_KEY).with_progress_reporter(Arc::new(
            move |snapshot| snapshots_for_reporter.lock().unwrap().push(snapshot),
        ));
        let evidence = SequencedEvidence {
            // 写前两次与提交前复查均匹配；提交后的最终证据读取发生漂移。
            current_through: 3,
            calls: AtomicUsize::new(0),
        };

        let outcome = fixture_executor(&executor, &db_path, storage.path()).execute_sync_plan(
            &plan,
            &OperationCancellation::new(),
            &evidence,
        );

        assert_eq!(
            outcome,
            SyncPlanExecutionOutcome::ManualRecoveryRequired {
                backups_preserved: true
            }
        );
        assert_eq!(
            project_owner(&db_path, "p1"),
            Some("2000000000000002".to_string()),
            "提交后漂移不能撤销已提交写入，也不能把不确定现场标记为成功"
        );
        let backup_root = storage
            .path()
            .join("backups")
            .join(plan.operation_id().as_str());
        assert!(verify_raw_backup(&backup_root.join("before").join("raw")));
        assert!(verify_raw_backup(&backup_root.join("failure").join("raw")));
        let summaries =
            crate::operation_manifest::list_operation_summaries(storage.path()).unwrap();
        assert_eq!(summaries.len(), 1);
        assert_eq!(summaries[0].state, OperationState::ManualRecoveryRequired);
        assert!(
            !catalog_operation_matches(
                storage.path(),
                TEST_RAW_KEY,
                plan.operation_id().as_str(),
                plan.data_location_id(),
                &target_file_evidence(&db_path).unwrap(),
            ),
            "证据漂移后不得写入完成态目录记录"
        );
        assert!(
            snapshots
                .lock()
                .unwrap()
                .iter()
                .all(|snapshot| snapshot.phase != ProgressPhase::Completed),
            "证据漂移后不得发布完成进度"
        );
    }

    #[test]
    fn logical_backup_at_requested_recovery_path_is_independently_verified() {
        let target = tempdir().unwrap();
        let storage = tempdir().unwrap();
        let db_path = make_work_cn_fixture_with_wal(target.path());
        let logical_db_path = storage
            .path()
            .join("before")
            .join("logical")
            .join("database.db");
        let source_before = snapshot_db_trio(target.path(), "database.db");

        // 逻辑副本必须写入恢复目录，而非源库同目录的临时文件。
        assert!(backup_to_logical_copy_at_inner(
            &db_path,
            TEST_RAW_KEY,
            &logical_db_path
        ));
        assert!(verify_logical_backup(&logical_db_path, TEST_RAW_KEY));
        assert_eq!(
            snapshot_db_trio(target.path(), "database.db"),
            source_before,
            "SQLCipher 导出不得改写源 DB/WAL/SHM"
        );
    }

    #[test]
    fn follow_project_stops_before_write_when_backup_root_is_unavailable() {
        let target = tempdir().unwrap();
        let storage = tempdir().unwrap();
        let db_path = make_work_cn_fixture(target.path());
        let source_before = snapshot_db_trio(target.path(), "database.db");
        let blocked_storage_root = storage.path().join("not-a-directory");
        std::fs::write(&blocked_storage_root, b"fixture").unwrap();

        let result = WorkCnSyncExecutor::new(TEST_RAW_KEY).execute_follow_project_inner(
            &db_path,
            &blocked_storage_root,
            &traesync_domain::OperationId::new(),
            "p1",
            "1000000000000001",
            "2000000000000002",
        );

        assert_eq!(
            result,
            FollowProjectExecution::FailedBeforeWrite {
                backups_preserved: false
            }
        );
        assert_eq!(
            snapshot_db_trio(target.path(), "database.db"),
            source_before,
            "备份无法创建时目标 DB/WAL/SHM 必须零修改"
        );
    }

    #[test]
    fn follow_project_rolls_back_when_target_owner_would_violate_unique_constraint() {
        let target = tempdir().unwrap();
        let storage = tempdir().unwrap();
        let db_path = make_work_cn_fixture(target.path());
        {
            let conn = open_with_key(&db_path, TEST_RAW_KEY).unwrap();
            conn.execute(
                "INSERT INTO project (project_id, user_id, biz_project_id) VALUES (?1, ?2, ?3)",
                params!["p2", "2000000000000002", "biz-1"],
            )
            .unwrap();
        }
        let operation_id = traesync_domain::OperationId::new();

        let result = WorkCnSyncExecutor::new(TEST_RAW_KEY).execute_follow_project_inner(
            &db_path,
            storage.path(),
            &operation_id,
            "p1",
            "1000000000000001",
            "2000000000000002",
        );

        assert_eq!(
            result,
            FollowProjectExecution::FailedBeforeWrite {
                backups_preserved: true
            }
        );
        assert_eq!(
            project_owner(&db_path, "p1"),
            Some("1000000000000001".to_string()),
            "唯一约束失败必须回滚 owner 更新"
        );
        let before_dir = storage
            .path()
            .join("backups")
            .join(operation_id.as_str())
            .join("before");
        assert!(verify_raw_backup(&before_dir.join("raw")));
        assert!(verify_logical_backup(
            &before_dir.join("logical").join("database.db"),
            TEST_RAW_KEY
        ));
    }

    #[test]
    fn follow_project_preserves_dual_backups_when_post_commit_verification_fails() {
        let target = tempdir().unwrap();
        let storage = tempdir().unwrap();
        let db_path = make_work_cn_fixture(target.path());
        {
            let conn = open_with_key(&db_path, TEST_RAW_KEY).unwrap();
            // fixture 触发器模拟提交后未涉及会话关系发生变化。
            conn.execute_batch(
                "CREATE TRIGGER remove_followed_sessions AFTER UPDATE ON project BEGIN DELETE FROM chat_session WHERE project_id = NEW.project_id; END;",
            )
            .unwrap();
        }
        let operation_id = traesync_domain::OperationId::new();

        let result = WorkCnSyncExecutor::new(TEST_RAW_KEY).execute_follow_project_inner(
            &db_path,
            storage.path(),
            &operation_id,
            "p1",
            "1000000000000001",
            "2000000000000002",
        );

        assert_eq!(
            result,
            FollowProjectExecution::FailedAfterWrite {
                backups_preserved: true
            },
            "提交后验证失败绝不允许报告成功"
        );
        let before_dir = storage
            .path()
            .join("backups")
            .join(operation_id.as_str())
            .join("before");
        assert!(verify_raw_backup(&before_dir.join("raw")));
        assert!(verify_logical_backup(
            &before_dir.join("logical").join("database.db"),
            TEST_RAW_KEY
        ));
    }

    #[test]
    fn catalog_reconciliation_reopens_after_commit_and_matches_evidence() {
        let target = tempdir().unwrap();
        let storage = tempdir().unwrap();
        let db_path = make_work_cn_fixture(target.path());
        let evidence = target_file_evidence(&db_path).unwrap();
        let data_location_id = fixture_location_id(&db_path);
        ensure_catalog_initialized(storage.path(), TEST_RAW_KEY).unwrap();

        assert!(reconcile_catalog_operation(
            storage.path(),
            TEST_RAW_KEY,
            "operation-catalog-reopen",
            &data_location_id,
            1,
            &evidence,
        ));
        assert!(catalog_operation_matches(
            storage.path(),
            TEST_RAW_KEY,
            "operation-catalog-reopen",
            &data_location_id,
            &evidence,
        ));
    }

    #[test]
    fn executor_repairs_stale_catalog_sidecar_before_reconciling_old_manifest() {
        let target = tempdir().unwrap();
        let storage = tempdir().unwrap();
        let db_path = make_work_cn_fixture(target.path());
        let plan = follow_project_plan(&db_path);
        let executor = WorkCnSyncExecutor::new(TEST_RAW_KEY);
        let bound = fixture_executor(&executor, &db_path, storage.path());
        let evidence = plan.target_file_evidence().clone();
        let old_operation_id = traesync_domain::OperationId::new();
        let old_journal = OperationManifestJournal::create(
            storage.path(),
            &old_operation_id,
            plan.data_location_id(),
            &evidence,
        )
        .unwrap();
        for state in [
            OperationState::BackingUp,
            OperationState::BackupVerified,
            OperationState::TargetWriting,
            OperationState::TargetCommittedUnverified,
            OperationState::TargetVerifying,
        ] {
            old_journal.transition(state).unwrap();
        }
        old_journal
            .transition_catalog_reconciling(&evidence)
            .unwrap();

        let catalog_path = resolve_current_catalog_path(storage.path()).unwrap();
        let mut connection = Connection::open(&catalog_path).unwrap();
        connection
            .execute_batch(&format!("PRAGMA key = \"x'{}'\";", TEST_RAW_KEY))
            .unwrap();
        let transaction = connection.transaction().unwrap();
        transaction
            .execute(
                "INSERT INTO operation_record(
                    operation_id, data_location_id, state, affected_rows,
                    db_fingerprint, wal_fingerprint, shm_fingerprint, updated_at
                 ) VALUES (?1, ?2, 'completed', 0, ?3, ?4, ?5, 0)",
                params![
                    old_operation_id.as_str(),
                    plan.data_location_id(),
                    evidence.db_fingerprint,
                    evidence.wal_fingerprint,
                    evidence.shm_fingerprint,
                ],
            )
            .unwrap();
        bump_catalog_content_revision(&transaction).unwrap();
        transaction.commit().unwrap();
        drop(connection);

        // 模拟目录库提交成功但 generation.json 尚未发布的提交后窗口。
        let metadata_path = catalog_path.parent().unwrap().join("generation.json");
        let mut metadata: serde_json::Value =
            serde_json::from_reader(std::fs::File::open(&metadata_path).unwrap()).unwrap();
        let current_revision = metadata["content_revision"].as_u64().unwrap();
        metadata["content_revision"] = serde_json::json!(current_revision);
        std::fs::write(
            &metadata_path,
            serde_json::to_vec_pretty(&metadata).unwrap(),
        )
        .unwrap();

        let outcome = bound.execute_sync_plan(
            &plan,
            &OperationCancellation::new(),
            &SequencedEvidence {
                current_through: usize::MAX,
                calls: AtomicUsize::new(0),
            },
        );

        assert!(matches!(
            outcome,
            SyncPlanExecutionOutcome::Completed { affected_rows: 1 }
        ));
        assert_eq!(
            old_journal.latest_state(),
            Some(OperationState::Completed),
            "旧 CatalogReconciling manifest 必须在新计划写入前收口"
        );
        let repaired: serde_json::Value =
            serde_json::from_reader(std::fs::File::open(&metadata_path).unwrap()).unwrap();
        assert!(
            repaired["content_revision"].as_u64().unwrap() > current_revision,
            "旧 manifest 协调前必须先恢复落后的 generation sidecar"
        );
    }

    #[test]
    fn executor_fails_closed_before_target_write_when_catalog_sidecar_is_invalid() {
        let target = tempdir().unwrap();
        let storage = tempdir().unwrap();
        let db_path = make_work_cn_fixture(target.path());
        let plan = follow_project_plan(&db_path);
        let executor = WorkCnSyncExecutor::new(TEST_RAW_KEY);
        let bound = fixture_executor(&executor, &db_path, storage.path());
        let before = snapshot_db_trio(target.path(), "database.db");

        let catalog_path = resolve_current_catalog_path(storage.path()).unwrap();
        let metadata_path = catalog_path.parent().unwrap().join("generation.json");
        let mut metadata: serde_json::Value =
            serde_json::from_reader(std::fs::File::open(&metadata_path).unwrap()).unwrap();
        metadata["unknown_field"] = serde_json::json!(true);
        std::fs::write(
            &metadata_path,
            serde_json::to_vec_pretty(&metadata).unwrap(),
        )
        .unwrap();

        let outcome = bound.execute_sync_plan(
            &plan,
            &OperationCancellation::new(),
            &SequencedEvidence {
                current_through: usize::MAX,
                calls: AtomicUsize::new(0),
            },
        );

        assert_eq!(
            outcome,
            SyncPlanExecutionOutcome::ManualRecoveryRequired {
                backups_preserved: false
            }
        );
        assert_eq!(
            snapshot_db_trio(target.path(), "database.db"),
            before,
            "sidecar 无法验证时不得开始目标库写入"
        );
        assert!(
            !storage
                .path()
                .join("backups")
                .join(plan.operation_id().as_str())
                .exists(),
            "sidecar 失败应在备份和 manifest 之前 fail-closed"
        );
    }

    #[test]
    fn executor_uses_independent_catalog_key_for_sidecar_and_operation_reconciliation() {
        let target = tempdir().unwrap();
        let storage = tempdir().unwrap();
        let db_path = make_work_cn_fixture(target.path());
        let plan = follow_project_plan(&db_path);
        let executor = WorkCnSyncExecutor::new_with_keys(TEST_RAW_KEY, TEST_CATALOG_KEY);
        let bound = fixture_executor_with_catalog_key(
            &executor,
            &db_path,
            storage.path(),
            TEST_CATALOG_KEY,
        );

        let outcome = bound.execute_sync_plan(
            &plan,
            &OperationCancellation::new(),
            &SequencedEvidence {
                current_through: usize::MAX,
                calls: AtomicUsize::new(0),
            },
        );

        assert!(matches!(
            outcome,
            SyncPlanExecutionOutcome::Completed { affected_rows: 1 }
        ));
        assert_eq!(
            project_owner(&db_path, "p1"),
            Some("2000000000000002".to_string())
        );
    }

    #[test]
    fn wrong_catalog_key_keeps_catalog_reconciling_manifest_uncompleted() {
        let target = tempdir().unwrap();
        let storage = tempdir().unwrap();
        let db_path = make_work_cn_fixture(target.path());
        let evidence = target_file_evidence(&db_path).unwrap();
        let data_location_id = fixture_location_id(&db_path);
        let operation_id = traesync_domain::OperationId::new();
        let journal = OperationManifestJournal::create(
            storage.path(),
            &operation_id,
            &data_location_id,
            &evidence,
        )
        .unwrap();
        for state in [
            OperationState::BackingUp,
            OperationState::BackupVerified,
            OperationState::TargetWriting,
            OperationState::TargetCommittedUnverified,
            OperationState::TargetVerifying,
        ] {
            journal.transition(state).unwrap();
        }
        journal.transition_catalog_reconciling(&evidence).unwrap();

        make_catalog_fixture_with_key(storage.path(), TEST_CATALOG_KEY);
        assert!(!reconcile_catalog_operation(
            storage.path(),
            WRONG_KEY,
            operation_id.as_str(),
            &data_location_id,
            1,
            &evidence,
        ));
        assert_eq!(
            journal.latest_state(),
            Some(OperationState::CatalogReconciling),
            "错误目录库 key 不能把已提交但未验证的 manifest 收口为 completed"
        );
    }

    /// 使用只读隔离连接读取会话和消息的归属及删除标记。
    fn session_message_metadata(db_path: &Path) -> ((String, String, i64), (String, String, i64)) {
        let conn = open_with_key_readonly(db_path, TEST_RAW_KEY).unwrap();
        let session = conn
            .query_row(
                "SELECT session_id, project_id, deleted_at FROM chat_session WHERE session_id = ?1",
                params!["s1"],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, i64>(2)?,
                    ))
                },
            )
            .unwrap();
        let message = conn
            .query_row(
                "SELECT message_id, session_id, deleted_at FROM chat_message WHERE message_id = ?1",
                params!["m1"],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, i64>(2)?,
                    ))
                },
            )
            .unwrap();
        (session, message)
    }

    /// 使用新只读连接读取项目 owner，避免复用执行器连接掩盖提交后问题。
    fn project_owner(db_path: &Path, project_id: &str) -> Option<String> {
        let conn = open_with_key_readonly(db_path, TEST_RAW_KEY).ok()?;
        conn.query_row(
            "SELECT user_id FROM project WHERE project_id = ?1",
            params![project_id],
            |row| row.get(0),
        )
        .ok()
    }

    /// 从 fixture 的指定表读取单个文本字段，供关系完整性断言使用。
    fn table_value(
        db_path: &Path,
        table: &str,
        value_column: &str,
        key_column: &str,
        key: &str,
    ) -> Option<String> {
        let conn = open_with_key_readonly(db_path, TEST_RAW_KEY).ok()?;
        let statement =
            format!("SELECT {value_column} FROM {table} WHERE {key_column} = ?1 LIMIT 1");
        conn.query_row(&statement, params![key], |row| row.get(0))
            .ok()
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
        assert!(copy_path.is_some(), "sqlcipher_export 逻辑副本应成功");

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

    /// R1：含有效 WAL/SHM 的探测同样不得触碰调用方三件套。
    #[test]
    fn probe_with_correct_key_and_wal_has_zero_writes_to_db_trio() {
        let dir = tempdir().unwrap();
        let db_path = make_work_cn_fixture_with_wal(dir.path());
        let wal_path = dir.path().join("database.db-wal");
        let shm_path = dir.path().join("database.db-shm");

        assert!(wal_path.exists(), "fixture 必须包含有效 WAL");
        assert!(shm_path.exists(), "fixture 必须包含有效 SHM");
        let before = (
            file_sha256(&db_path),
            file_sha256(&wal_path),
            file_sha256(&shm_path),
        );

        let state = SqlCipherProbe::new().probe_database(&db_path, TEST_RAW_KEY);

        assert!(matches!(state, CompatibilityState::Verified { .. }));
        let after = (
            file_sha256(&db_path),
            file_sha256(&wal_path),
            file_sha256(&shm_path),
        );
        assert_eq!(
            after, before,
            "R1 失败：含 WAL/SHM 的只读探测修改了调用方三件套"
        );
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

    /// R5/Gate A：版本和 SQLCipher 4 关键参数均与基线一致。
    #[test]
    fn probe_work_cn_db_returns_cipher_version_and_pragmas_compatible() {
        let dir = tempdir().unwrap();
        let db_path = make_work_cn_fixture(dir.path());
        let probe = SqlCipherProbe::new();
        let state = probe.probe_database(&db_path, TEST_RAW_KEY);
        // 版本或任一关键 PRAGMA 不匹配时，生产探测必须返回 Incompatible。
        match state {
            CompatibilityState::Verified { .. } => {}
            CompatibilityState::Incompatible {
                reason: IncompatibleReason::CipherVersionMismatch { version },
            } => {
                panic!("cipher_version 不兼容：{}", version);
            }
            CompatibilityState::Incompatible {
                reason:
                    IncompatibleReason::CipherPragmaMismatch {
                        pragma,
                        expected,
                        actual,
                    },
            } => {
                panic!("关键 PRAGMA 不兼容：{pragma}; expected={expected}; actual={actual}");
            }
            other => panic!("期望 Verified，实际 {:?}", other),
        }

        let conn = open_with_key_readonly(&db_path, TEST_RAW_KEY).unwrap();
        for (pragma, query, expected) in REQUIRED_CIPHER_PRAGMAS {
            assert_eq!(
                read_pragma_scalar(&conn, query).as_deref(),
                Some(expected),
                "关键 PRAGMA 基线不匹配：{pragma}"
            );
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
