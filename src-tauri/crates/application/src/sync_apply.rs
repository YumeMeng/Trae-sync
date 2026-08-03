//! T06 同步计划应用服务：在任何目标副作用前确认计划证据与取消语义。

use traesync_domain::{OperationCancellation, SyncPlan, SyncPlanExecutionOutcome};
use traesync_ports::{SyncPlanEvidencePort, SyncPlanExecutorPort};

/// 同步计划应用服务：只编排计划有效性与写前取消，不触碰文件或数据库。
pub struct ApplySyncPlanService<'a> {
    evidence: &'a dyn SyncPlanEvidencePort,
    executor: &'a dyn SyncPlanExecutorPort,
}

impl<'a> ApplySyncPlanService<'a> {
    /// 在组合根注入证据读取器和已受 fixture guard 绑定的执行器。
    pub fn new(
        evidence: &'a dyn SyncPlanEvidencePort,
        executor: &'a dyn SyncPlanExecutorPort,
    ) -> Self {
        Self { evidence, executor }
    }

    /// 写前先验证计划，再接受取消；任一失败都不得进入执行器。
    pub fn apply(
        &self,
        plan: &SyncPlan,
        cancellation: &OperationCancellation,
    ) -> SyncPlanExecutionOutcome {
        if !self.evidence.is_current(plan) {
            return SyncPlanExecutionOutcome::PlanExpired {
                backups_preserved: false,
            };
        }
        if cancellation.is_requested() {
            return SyncPlanExecutionOutcome::CancelledBeforeWrite {
                backups_preserved: false,
            };
        }

        self.executor
            .execute_sync_plan(plan, cancellation, self.evidence)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use traesync_domain::{
        build_sync_plan, BuildSyncPlanInput, OperationCancellation, PlanProjectInput,
        PlanSessionInput, ProjectIdentity, SyncPlanExecutionOutcome, SyncScope, TargetFileEvidence,
    };
    use traesync_ports::{SyncPlanEvidencePort, SyncPlanExecutorPort};

    use super::ApplySyncPlanService;

    struct FixedEvidence {
        current: bool,
    }

    impl SyncPlanEvidencePort for FixedEvidence {
        fn is_current(&self, _plan: &traesync_domain::SyncPlan) -> bool {
            self.current
        }
    }

    struct CountingExecutor {
        calls: AtomicUsize,
    }

    impl SyncPlanExecutorPort for CountingExecutor {
        fn execute_sync_plan(
            &self,
            _plan: &traesync_domain::SyncPlan,
            _cancellation: &OperationCancellation,
            _evidence: &dyn SyncPlanEvidencePort,
        ) -> SyncPlanExecutionOutcome {
            self.calls.fetch_add(1, Ordering::SeqCst);
            SyncPlanExecutionOutcome::Completed { affected_rows: 1 }
        }
    }

    fn follow_project_plan() -> traesync_domain::SyncPlan {
        build_sync_plan(BuildSyncPlanInput {
            created_at: std::time::SystemTime::UNIX_EPOCH,
            platform_id: "work_cn".to_string(),
            data_location_id: "fixture-location".to_string(),
            current_user_id: "target-user".to_string(),
            account_evidence_fingerprint: "account-fingerprint".to_string(),
            target_file_evidence: TargetFileEvidence {
                db_fingerprint: "db-fingerprint".to_string(),
                wal_fingerprint: None,
                shm_fingerprint: None,
            },
            schema_fingerprint: "schema-fingerprint".to_string(),
            mapping_version: "work_cn_v1".to_string(),
            schema_compatible: true,
            scope: SyncScope::AllHistory,
            projects: vec![PlanProjectInput {
                identity: ProjectIdentity {
                    project_id: "source-project".to_string(),
                    biz_project_id: "biz-source".to_string(),
                    display_name: "fixture project".to_string(),
                    soft_deleted: false,
                },
                display_owner: "source-user".to_string(),
                current_live_owner: "source-user".to_string(),
                sessions: vec![PlanSessionInput {
                    identity: traesync_domain::SessionIdentity::new("work_cn", "session-1"),
                    version_available: true,
                }],
                archived_only: false,
            }],
        })
    }

    #[test]
    fn expired_plan_never_reaches_executor() {
        let evidence = FixedEvidence { current: false };
        let executor = CountingExecutor {
            calls: AtomicUsize::new(0),
        };
        let service = ApplySyncPlanService::new(&evidence, &executor);

        let outcome = service.apply(&follow_project_plan(), &OperationCancellation::new());

        assert_eq!(
            outcome,
            SyncPlanExecutionOutcome::PlanExpired {
                backups_preserved: false
            }
        );
        assert_eq!(executor.calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn cancellation_before_write_never_reaches_executor() {
        let evidence = FixedEvidence { current: true };
        let executor = CountingExecutor {
            calls: AtomicUsize::new(0),
        };
        let cancellation = OperationCancellation::new();
        cancellation.request();
        let service = ApplySyncPlanService::new(&evidence, &executor);

        let outcome = service.apply(&follow_project_plan(), &cancellation);

        assert_eq!(
            outcome,
            SyncPlanExecutionOutcome::CancelledBeforeWrite {
                backups_preserved: false
            }
        );
        assert_eq!(executor.calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn current_plan_delegates_to_executor() {
        let evidence = FixedEvidence { current: true };
        let executor = CountingExecutor {
            calls: AtomicUsize::new(0),
        };
        let service = ApplySyncPlanService::new(&evidence, &executor);

        let outcome = service.apply(&follow_project_plan(), &OperationCancellation::new());

        assert_eq!(
            outcome,
            SyncPlanExecutionOutcome::Completed { affected_rows: 1 }
        );
        assert_eq!(executor.calls.load(Ordering::SeqCst), 1);
    }
}
