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

use rusqlite::{params, types::ValueRef, Connection, OpenFlags, Row};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use traesync_domain::{
    CompatibilityState, IncompatibleReason, OperationCancellation, OperationState, PlanAction,
    SyncPlan, SyncPlanExecutionOutcome,
};
use traesync_ports::{DatabaseProbePort, SyncPlanEvidencePort, SyncPlanExecutorPort};

use crate::fixture_paths::FixturePathGuard;
use crate::operation_manifest::{
    has_manual_recovery_required, reconcile_unfinished_manifests_with_recovery_handlers,
    OperationManifestJournal,
};
use crate::work_cn_schema::{check_schema, compute_schema_fingerprint, read_table_counts};

/// 嵌入式 SQLCipher 探测器：实现 `DatabaseProbePort`。
pub struct SqlCipherProbe;

/// Work CN 的最小同步执行器。
///
/// T06/T07 执行器支持受限项目跟随与会话重挂；公开入口强制 fixture 路径防护。
pub struct WorkCnSyncExecutor {
    raw_key: String,
}

/// `FollowProject` 的结构化执行结论，不暴露原始数据库错误或密钥。
pub type FollowProjectExecution = SyncPlanExecutionOutcome;

/// 已固定 guard、目标 DB 与恢复存储根的执行器；只能由 `bind_fixture` 构造。
pub struct FixtureWorkCnSyncExecutor<'a> {
    executor: &'a WorkCnSyncExecutor,
    fixture_root: PathBuf,
    target_db_path: PathBuf,
    storage_root: PathBuf,
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
}

/// 会话重挂事务结果与完整项目跟随保持相同的漂移语义。
enum AttachSessionsTransaction {
    Committed(AttachSessionsCommit),
    EvidenceChanged,
    Failed,
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

/// 统一承载两类事务结果，避免后续状态机分叉。
enum SyncTransaction {
    FollowProject(FollowProjectTransaction),
    AttachSessions(AttachSessionsTransaction),
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
        Self {
            raw_key: raw_key.into(),
        }
    }

    /// 绑定唯一生产执行入口所需的 fixture guard、目标数据库和恢复存储根。
    pub fn bind_fixture<'a>(
        &'a self,
        fixture_guard: &FixturePathGuard,
        target_db_path: &Path,
        storage_root: &Path,
    ) -> Result<FixtureWorkCnSyncExecutor<'a>, crate::FixturePathError> {
        // DB 与恢复区均由 guard 验证，避免 public API 变成真实路径的写入旁路。
        let target_db_path = fixture_guard.validate_write_target(target_db_path)?;
        let storage_root = fixture_guard.validate_fixture_storage_root(storage_root)?;
        Ok(FixtureWorkCnSyncExecutor {
            executor: self,
            fixture_root: fixture_guard.canonical_root().to_path_buf(),
            target_db_path,
            storage_root,
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
        if !backup_to_logical_copy_at_inner(target_db_path, &self.raw_key, &logical_db_path)
            || !verify_logical_backup(&logical_db_path, &self.raw_key)
        {
            return FollowProjectExecution::FailedBeforeWrite {
                backups_preserved: true,
            };
        }

        let committed = match apply_follow_project_transaction(
            target_db_path,
            &self.raw_key,
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
            &self.raw_key,
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
    /// 按 manifest 状态机执行单个受支持动作；其它动作留给后续 ticket 实现。
    fn execute_sync_plan_inner(
        &self,
        plan: &SyncPlan,
        cancellation: &OperationCancellation,
        evidence: &dyn SyncPlanEvidencePort,
    ) -> SyncPlanExecutionOutcome {
        // 先协调旧操作；任一写入中断都进入人工恢复，当前操作不碰目标库。
        if !reconcile_unfinished_manifests_with_recovery_handlers(
            &self.storage_root,
            |journal| {
                journal.data_location_id() == plan.data_location_id()
                    && journal
                        .verified_target_file_evidence()
                        .is_some_and(|expected| {
                            target_file_matches_evidence(&self.target_db_path, expected)
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
        )
            || has_manual_recovery_required(&self.storage_root)
        {
            return SyncPlanExecutionOutcome::ManualRecoveryRequired {
                backups_preserved: false,
            };
        }

        let action = match plan.actions() {
            [PlanAction::FollowProject {
                project_id,
                from_user_id,
                to_user_id,
            }] => ExecutableAction::FollowProject {
                project_id: project_id.as_str(),
                source_user_id: from_user_id.as_str(),
                target_user_id: to_user_id.as_str(),
            },
            [PlanAction::AttachSessions {
                source_project_id,
                target_project_id,
                session_ids,
            }] if attach_sessions_preflight(
                &self.target_db_path,
                &self.executor.raw_key,
                &self.fixture_root,
                source_project_id,
                target_project_id,
                session_ids,
            ) => ExecutableAction::AttachSessions {
                source_project_id: source_project_id.as_str(),
                target_project_id: target_project_id.as_str(),
                session_ids,
            },
            _ => return SyncPlanExecutionOutcome::UnsupportedPlan,
        };

        // 应用层已检查一次；执行器再检查，防止调用者绕过 service 或计划在间隙失效。
        if !evidence.is_current(plan) || !target_file_matches_plan(&self.target_db_path, plan) {
            return SyncPlanExecutionOutcome::PlanExpired {
                backups_preserved: false,
            };
        }

        let journal = match OperationManifestJournal::create(
            &self.storage_root,
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

        if journal.transition(OperationState::BackingUp).is_err()
            || !capture_raw_backup(&self.target_db_path, &raw_dir)
        {
            let _ = journal.transition(OperationState::FailedSafe);
            return SyncPlanExecutionOutcome::FailedBeforeWrite {
                backups_preserved: false,
            };
        }
        if !backup_to_logical_copy_at_inner(
            &self.target_db_path,
            &self.executor.raw_key,
            &logical_db_path,
        ) || !verify_logical_backup(&logical_db_path, &self.executor.raw_key)
        {
            let _ = journal.transition(OperationState::FailedSafe);
            return SyncPlanExecutionOutcome::FailedBeforeWrite {
                backups_preserved: true,
            };
        }
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
        if !evidence.is_current(plan) || !target_file_matches_plan(&self.target_db_path, plan) {
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

        let transaction = match &action {
            ExecutableAction::FollowProject {
                project_id,
                source_user_id,
                target_user_id,
            } => SyncTransaction::FollowProject(apply_follow_project_transaction_with_evidence(
                &self.target_db_path,
                &self.executor.raw_key,
                *project_id,
                *source_user_id,
                *target_user_id,
                || evidence.is_current(plan) && target_file_matches_plan(&self.target_db_path, plan),
            )),
            ExecutableAction::AttachSessions {
                source_project_id,
                target_project_id,
                session_ids,
            } => SyncTransaction::AttachSessions(apply_attach_sessions_transaction_with_evidence(
                &self.target_db_path,
                &self.executor.raw_key,
                *source_project_id,
                *target_project_id,
                session_ids,
                || {
                    evidence.is_current(plan)
                        && target_file_matches_plan(&self.target_db_path, plan)
                        && target_sandbox_is_available(&self.fixture_root, target_project_id)
                },
            )),
        };
        let committed = match transaction {
            SyncTransaction::FollowProject(FollowProjectTransaction::Committed(commit)) => {
                CommittedAction::FollowProject(commit)
            }
            SyncTransaction::AttachSessions(AttachSessionsTransaction::Committed(commit)) => {
                CommittedAction::AttachSessions(commit)
            }
            SyncTransaction::FollowProject(FollowProjectTransaction::EvidenceChanged)
            | SyncTransaction::AttachSessions(AttachSessionsTransaction::EvidenceChanged) => {
                let _ = journal.transition(OperationState::NotApplied);
                return SyncPlanExecutionOutcome::PlanExpired {
                    backups_preserved: true,
                };
            }
            SyncTransaction::FollowProject(FollowProjectTransaction::Failed)
            | SyncTransaction::AttachSessions(AttachSessionsTransaction::Failed) => {
                let _ = journal.transition(OperationState::NotApplied);
                return SyncPlanExecutionOutcome::FailedBeforeWrite {
                    backups_preserved: true,
                };
            }
        };

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
                &self.executor.raw_key,
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
                &self.executor.raw_key,
                source_project_id,
                target_project_id,
                session_ids,
                commit,
            ),
            _ => false,
        };
        // 提交后漂移不能撤销已确认目标，但必须继续完整验证且不报告普通成功。
        let post_commit_drift = !evidence.is_current(plan);
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
            || journal.transition(OperationState::Completed).is_err()
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

        if post_commit_drift {
            SyncPlanExecutionOutcome::CompletedWithPostCommitEvidenceDrift {
                affected_rows: committed.affected_rows(),
            }
        } else {
            SyncPlanExecutionOutcome::Completed {
                affected_rows: committed.affected_rows(),
            }
        }
    }
}

/// 复制捕获时存在的原始 DB/WAL/SHM，并用独立哈希清单验证每个副本。
fn capture_raw_backup(source_db_path: &Path, raw_dir: &Path) -> bool {
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
    let mut hashes = Vec::new();

    for (source_path, backup_name, required) in source_files {
        if !source_path.exists() {
            if required {
                return false;
            }
            continue;
        }

        let backup_path = raw_dir.join(backup_name);
        if std::fs::copy(&source_path, &backup_path).is_err() {
            return false;
        }

        let source_hash = match sha256_file_for_backup(&source_path) {
            Some(hash) => hash,
            None => return false,
        };
        let backup_hash = match sha256_file_for_backup(&backup_path) {
            Some(hash) => hash,
            None => return false,
        };
        if source_hash != backup_hash {
            return false;
        }
        hashes.push(format!("{backup_hash}  {backup_name}"));
    }

    if std::fs::write(raw_dir.join("hashes.sha256"), hashes.join("\n")).is_err() {
        return false;
    }

    verify_raw_backup(raw_dir)
}

/// 原始备份清单只允许本次固定的三个文件名，防止验证范围被清单意外扩大。
fn verify_raw_backup(raw_dir: &Path) -> bool {
    let contents = match std::fs::read_to_string(raw_dir.join("hashes.sha256")) {
        Ok(contents) => contents,
        Err(_) => return false,
    };
    let mut count = 0;

    for line in contents.lines().filter(|line| !line.is_empty()) {
        let (expected_hash, name) = match line.split_once("  ") {
            Some(parts) => parts,
            None => return false,
        };
        if !matches!(name, "database.db" | "database.db-wal" | "database.db-shm") {
            return false;
        }
        let actual_hash = match sha256_file_for_backup(&raw_dir.join(name)) {
            Some(hash) => hash,
            None => return false,
        };
        if actual_hash != expected_hash {
            return false;
        }
        count += 1;
    }

    count > 0
}

/// 生成目标数据库的 WAL 或 SHM 路径，不依赖当前文件扩展名。
fn database_sidecar_path(db_path: &Path, suffix: &str) -> PathBuf {
    PathBuf::from(format!("{}{}", db_path.to_string_lossy(), suffix))
}

/// 比较目标三件套与计划固定指纹；存在性变化也视为漂移，不允许继续写入。
fn target_file_matches_plan(target_db_path: &Path, plan: &SyncPlan) -> bool {
    target_file_matches_evidence(target_db_path, plan.target_file_evidence())
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
    journal
        .transition(OperationState::FailurePreserving)
        .is_ok()
        && capture_failure_evidence(target_db_path, storage_root, operation_id)
        && journal
            .transition(OperationState::FailureSnapshotVerified)
            .is_ok()
}

/// 仅用于备份验证的 SHA-256 计算；失败关闭，不返回部分结果。
fn sha256_file_for_backup(path: &Path) -> Option<String> {
    let bytes = std::fs::read(path).ok()?;
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    Some(hex::encode(hasher.finalize()))
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

    // SQLCipher 导出要求主连接可写；调用方已先固定原始备份，测试另行证明导出不改源三件套。
    let conn = match open_with_key(source_db, raw_key) {
        Ok(conn) => conn,
        Err(_) => return false,
    };
    // SQLCipher 的 KEY 语法不接受绑定参数；路径来自已受 fixture 防护的组合根，仍逐字转义。
    let destination_text = destination.to_string_lossy().replace('\'', "''");
    let attach_sql = format!("ATTACH DATABASE '{destination_text}' AS dst KEY \"x'{raw_key}'\";");
    if conn.execute_batch(&attach_sql).is_err() {
        return false;
    }

    let exported = conn
        .query_row("SELECT sqlcipher_export('dst')", [], |_row| Ok(()))
        .is_ok();
    let detached = conn.execute_batch("DETACH DATABASE dst;").is_ok();
    exported && detached && destination.exists()
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

/// 仅接受文件名安全且已存在的目标 sandbox 配置；缺失时不进入事务。
fn target_sandbox_is_available(fixture_root: &Path, target_project_id: &str) -> bool {
    !target_project_id.is_empty()
        && !target_project_id.contains(['/', '\\'])
        && fixture_root
            .join("sandbox")
            .join(format!("{target_project_id}.json"))
            .is_file()
}

/// 会话重挂所需映射固定在已验证 schema 中；未知直接项目关系表一律拒绝。
fn attach_relation_schema_supported(conn: &Connection) -> bool {
    const SUPPORTED_COLUMNS: &[(&str, &[&str])] = &[
        ("project", &["project_id", "user_id", "biz_project_id"]),
        ("chat_session", &["session_id", "project_id"]),
        ("chat_message", &["message_id", "session_id", "body"]),
        ("session_project", &["session_id", "project_id"]),
        ("snapshot", &["snapshot_id", "chat_session_id", "project_id"]),
        ("staging", &["staging_id", "chat_session_id", "project_id"]),
        (
            "local_artifact",
            &[
                "artifact_id",
                "source_session_id",
                "source_project_id",
                "user_id",
            ],
        ),
        (
            "local_artifact_version",
            &["version_id", "artifact_id", "source_project_id"],
        ),
        // 当前映射已经验证的 fixture FTS；其它携带直接引用的表必须先补证据。
        ("chat_fts", &["session_id", "indexed_body"]),
    ];

    for (table, supported_columns) in SUPPORTED_COLUMNS {
        let Some(columns) = table_columns(conn, table) else {
            return false;
        };
        let expected_columns = supported_columns
            .iter()
            .map(|column| (*column).to_string())
            .collect::<BTreeSet<_>>();
        if columns != expected_columns {
            return false;
        }
    }

    let mut statement = match conn.prepare(
        "SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%'",
    ) {
        Ok(statement) => statement,
        Err(_) => return false,
    };
    let tables = match statement.query_map([], |row| row.get::<_, String>(0)) {
        Ok(tables) => tables,
        Err(_) => return false,
    };
    for table in tables {
        let table = match table {
            Ok(table) => table,
            Err(_) => return false,
        };
        if !SUPPORTED_COLUMNS
            .iter()
            .any(|(supported_table, _)| *supported_table == table)
        {
            return false;
        }
    }

    true
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
    if !attach_relation_schema_supported(&conn) {
        return false;
    }

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

    let mut seen_session_ids = BTreeSet::new();
    for session in session_ids {
        let session_id = session.original_session_id.as_str();
        if session.product_history_namespace != "work_cn"
            || session_id.is_empty()
            || !seen_session_ids.insert(session_id)
        {
            return false;
        }
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
        let artifact_mismatch_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM local_artifact WHERE source_session_id = ?1 AND (source_project_id <> ?2 OR user_id <> ?3)",
                params![session_id, source_project_id, source_user_id],
                |row| row.get(0),
            )
            .unwrap_or(-1);
        let artifact_version_mismatch_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM local_artifact_version version INNER JOIN local_artifact artifact ON artifact.artifact_id = version.artifact_id WHERE artifact.source_session_id = ?1 AND version.source_project_id <> ?2",
                params![session_id, source_project_id],
                |row| row.get(0),
            )
            .unwrap_or(-1);
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
fn apply_attach_sessions_transaction_with_evidence<F>(
    target_db_path: &Path,
    raw_key: &str,
    source_project_id: &str,
    target_project_id: &str,
    session_ids: &[traesync_domain::SessionIdentity],
    before_commit: F,
) -> AttachSessionsTransaction
where
    F: FnOnce() -> bool,
{
    let mut conn = match open_with_key(target_db_path, raw_key) {
        Ok(conn) => conn,
        Err(_) => return AttachSessionsTransaction::Failed,
    };
    let tx = match conn.transaction() {
        Ok(tx) => tx,
        Err(_) => return AttachSessionsTransaction::Failed,
    };
    if !attach_relation_schema_supported(&tx) || session_ids.is_empty() {
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
        let artifact_mismatch_count: i64 = tx
            .query_row(
                "SELECT COUNT(*) FROM local_artifact WHERE source_session_id = ?1 AND (source_project_id <> ?2 OR user_id <> ?3)",
                params![session_id, source_project_id, source_user_id],
                |row| row.get(0),
            )
            .unwrap_or(-1);
        if total_relation_count != 1
            || snapshot_mismatch_count != 0
            || staging_mismatch_count != 0
            || artifact_mismatch_count != 0
        {
            return AttachSessionsTransaction::Failed;
        }

        let artifact_version_count: i64 = tx
            .query_row(
                "SELECT COUNT(*) FROM local_artifact_version version INNER JOIN local_artifact artifact ON artifact.artifact_id = version.artifact_id WHERE artifact.source_session_id = ?1 AND artifact.source_project_id = ?2 AND version.source_project_id = ?2",
                params![session_id, source_project_id],
                |row| row.get(0),
            )
            .unwrap_or(-1);
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
        let changed_artifacts = tx
            .execute(
                "UPDATE local_artifact SET source_project_id = ?1, user_id = ?2 WHERE source_session_id = ?3 AND source_project_id = ?4 AND user_id = ?5",
                params![target_project_id, target_user_id, session_id, source_project_id, source_user_id],
            )
            .unwrap_or(usize::MAX);
        let changed_artifact_versions = tx
            .execute(
                "UPDATE local_artifact_version SET source_project_id = ?1 WHERE source_project_id = ?2 AND artifact_id IN (SELECT artifact_id FROM local_artifact WHERE source_session_id = ?3 AND source_project_id = ?1)",
                params![target_project_id, source_project_id, session_id],
            )
            .unwrap_or(usize::MAX);
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
    if !attach_relation_schema_supported(&conn) {
        return false;
    }
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
        let artifact_mismatch_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM local_artifact WHERE source_session_id = ?1 AND (source_project_id <> ?2 OR user_id <> ?3)",
                params![session_id, target_project_id, target_user_id],
                |row| row.get(0),
            )
            .unwrap_or(-1);
        let artifact_version_mismatch_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM local_artifact_version version INNER JOIN local_artifact artifact ON artifact.artifact_id = version.artifact_id WHERE artifact.source_session_id = ?1 AND version.source_project_id <> ?2",
                params![session_id, target_project_id],
                |row| row.get(0),
            )
            .unwrap_or(-1);
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
        let selected_session_ids = selected_session_ids(session_ids)?;
        let selected_artifact_ids = artifact_ids_for_sessions(
            conn,
            source_project_id,
            source_user_id,
            &selected_session_ids,
        )?;
        Some(Self {
            selected_session_ids,
            selected_artifact_ids,
        })
    }

    /// 提交后从目标关系重建同一许可范围，再与提交前摘要比较。
    fn from_target(
        conn: &Connection,
        target_project_id: &str,
        session_ids: &[traesync_domain::SessionIdentity],
    ) -> Option<Self> {
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
        )?;
        Some(Self {
            selected_session_ids,
            selected_artifact_ids,
        })
    }

    /// 只有规格列出的关系单元格可归一化，其余字段、行和表必须逐字保持。
    fn allows_cell(&self, table: &str, columns: &[String], values: &[Vec<u8>], column: &str) -> bool {
        let text_at = |name: &str| {
            columns
                .iter()
                .position(|candidate| candidate == name)
                .and_then(|index| values.get(index))
                .and_then(|value| value.strip_prefix(&[b't']))
                .and_then(|value| std::str::from_utf8(value).ok())
        };
        match (table, column) {
            ("chat_session" | "session_project", "project_id") => text_at("session_id")
                .is_some_and(|session_id| self.selected_session_ids.contains(session_id)),
            ("snapshot" | "staging", "project_id") => text_at("chat_session_id")
                .is_some_and(|session_id| self.selected_session_ids.contains(session_id)),
            ("local_artifact", "source_project_id" | "user_id") => text_at("source_session_id")
                .is_some_and(|session_id| self.selected_session_ids.contains(session_id)),
            ("local_artifact_version", "source_project_id") => text_at("artifact_id")
                .is_some_and(|artifact_id| self.selected_artifact_ids.contains(artifact_id)),
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
) -> Option<BTreeSet<String>> {
    let mut artifact_ids = BTreeSet::new();
    for session_id in session_ids {
        let mut statement = conn
            .prepare(
                "SELECT artifact_id FROM local_artifact WHERE source_session_id = ?1 AND source_project_id = ?2 AND user_id = ?3",
            )
            .ok()?;
        let rows = statement
            .query_map(params![session_id, project_id, user_id], |row| row.get::<_, String>(0))
            .ok()?;
        for artifact_id in rows {
            artifact_ids.insert(artifact_id.ok()?);
        }
    }
    Some(artifact_ids)
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

/// 对单表按稳定 rowid 顺序编码。没有 rowid 或无法读取的 schema 一律拒绝写入。
fn fingerprint_table(
    conn: &Connection,
    table_name: &str,
    mutation_scope: &AttachSessionMutationScope,
) -> Option<TableFingerprint> {
    let query = format!(
        "SELECT * FROM {} ORDER BY rowid",
        quote_sql_identifier(table_name)
    );
    let mut statement = conn.prepare(&query).ok()?;
    let columns = statement
        .column_names()
        .iter()
        .map(|column| (*column).to_string())
        .collect::<Vec<_>>();
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
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tempfile::tempdir;

    /// 合成 fixture raw key（不使用 TECHNICAL_BASELINE.md 中的真实基线 key）。
    /// 仅供 fixture 测试：创建加密 fixture 并验证探测逻辑，不接触真实 TRAE 数据库。
    /// 真实基线 key 只存在于 docs/TECHNICAL_BASELINE.md，不进入源码、日志或证据。
    const TEST_RAW_KEY: &str = "deadbeefcafebabe1234567890abcdefdeadbeefcafebabe1234567890abcdef";

    /// 错误 key（与基线不同的 64 字符 hex）
    const WRONG_KEY: &str = "0000000000000000000000000000000000000000000000000000000000000000";

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

    /// 构造与本 fixture 三件套绑定的单个 `FollowProject` 不可变计划。
    fn follow_project_plan(db_path: &Path) -> SyncPlan {
        traesync_domain::build_sync_plan(traesync_domain::BuildSyncPlanInput {
            created_at: SystemTime::UNIX_EPOCH,
            platform_id: "work_cn".to_string(),
            data_location_id: "fixture-location".to_string(),
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

    /// 构造仅把选中会话挂到目标已存在项目的不可变计划。
    fn attach_sessions_plan(db_path: &Path, selected_session_ids: &[&str]) -> SyncPlan {
        traesync_domain::build_sync_plan(traesync_domain::BuildSyncPlanInput {
            created_at: SystemTime::UNIX_EPOCH,
            platform_id: "work_cn".to_string(),
            data_location_id: "fixture-location".to_string(),
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
                        identity: traesync_domain::SessionIdentity::new("work_cn", "target-session"),
                        version_available: true,
                    }],
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
        FixtureWorkCnSyncExecutor {
            executor,
            fixture_root: db_path.parent().unwrap().to_path_buf(),
            target_db_path: db_path.to_path_buf(),
            storage_root: storage_root.to_path_buf(),
        }
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
        assert_eq!(table_value(&db_path, "chat_session", "project_id", "session_id", "s1"), Some("p-target".to_string()));
        assert_eq!(table_value(&db_path, "session_project", "project_id", "session_id", "s1"), Some("p-target".to_string()));
        assert_eq!(table_value(&db_path, "snapshot", "project_id", "chat_session_id", "s1"), Some("p-target".to_string()));
        assert_eq!(table_value(&db_path, "staging", "project_id", "chat_session_id", "s1"), Some("p-target".to_string()));
        assert_eq!(table_value(&db_path, "local_artifact", "source_project_id", "artifact_id", "artifact-1"), Some("p-target".to_string()));
        assert_eq!(table_value(&db_path, "local_artifact", "user_id", "artifact_id", "artifact-1"), Some("2000000000000002".to_string()));
        assert_eq!(table_value(&db_path, "local_artifact_version", "source_project_id", "artifact_id", "artifact-1"), Some("p-target".to_string()));
        assert_eq!(table_value(&db_path, "chat_session", "project_id", "session_id", "s2"), Some("p-source".to_string()));
        assert_eq!(table_value(&db_path, "local_artifact", "source_project_id", "artifact_id", "artifact-2"), Some("p-source".to_string()));
        assert_eq!(table_value(&db_path, "chat_message", "body", "message_id", "m1"), Some("source one body".to_string()));
        assert_eq!(table_value(&db_path, "chat_fts", "indexed_body", "session_id", "s1"), Some("source one body".to_string()));
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

        assert!(matches!(
            outcome,
            SyncPlanExecutionOutcome::Completed { affected_rows } if affected_rows >= 12
        ));
        for session_id in ["s1", "s2"] {
            assert_eq!(
                table_value(&db_path, "chat_session", "project_id", "session_id", session_id),
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
    fn attach_sessions_rejects_unmapped_table_before_backup() {
        let target = tempdir().unwrap();
        let storage = tempdir().unwrap();
        let db_path = make_attach_sessions_fixture(target.path());
        let conn = open_with_key(&db_path, TEST_RAW_KEY).unwrap();
        conn.execute_batch("CREATE TABLE unmapped_cache (cache_key TEXT PRIMARY KEY, value TEXT NOT NULL);")
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
        conn.execute("DELETE FROM session_project WHERE session_id = ?1", params!["s1"])
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
            "fixture-location",
            plan.target_file_evidence(),
        )
        .unwrap();
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
            journal.transition(OperationState::FailurePreserving).unwrap();
            assert!(capture_failure_evidence(
                &db_path,
                storage.path(),
                operation_id.as_str()
            ));
            if state == OperationState::FailureSnapshotVerified {
                journal.transition(OperationState::FailureSnapshotVerified).unwrap();
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
        let backup_root = storage.path().join("backups").join(plan.operation_id().as_str());
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
        let backup_root = storage.path().join("backups").join(plan.operation_id().as_str());
        assert!(verify_raw_backup(&backup_root.join("before").join("raw")));
        assert!(verify_raw_backup(&backup_root.join("failure").join("raw")));
    }

    #[test]
    fn follow_project_creates_verified_dual_backups_and_updates_only_expected_owner() {
        let target = tempdir().unwrap();
        let storage = tempdir().unwrap();
        let db_path = make_work_cn_fixture_with_wal(target.path());
        let source_before = snapshot_db_trio(target.path(), "database.db");
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
    fn plan_execution_reports_post_commit_evidence_drift_without_hiding_successful_validation() {
        let target = tempdir().unwrap();
        let storage = tempdir().unwrap();
        let db_path = make_work_cn_fixture(target.path());
        let plan = follow_project_plan(&db_path);
        let executor = WorkCnSyncExecutor::new(TEST_RAW_KEY);
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
            SyncPlanExecutionOutcome::CompletedWithPostCommitEvidenceDrift { affected_rows: 1 }
        );
        assert_eq!(
            project_owner(&db_path, "p1"),
            Some("2000000000000002".to_string()),
            "提交后漂移不能掩盖已完成的新连接完整验证"
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
        let statement = format!(
            "SELECT {value_column} FROM {table} WHERE {key_column} = ?1 LIMIT 1"
        );
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
