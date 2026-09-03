//! T06 同步执行端口：application 编排计划，infrastructure 保有 fixture 路径与 SQLCipher 细节。

use traesync_domain::{OperationCancellation, SyncPlan, SyncPlanExecutionOutcome};

/// 当前证据读取端口；每次调用都必须重新读取，而非复用构建计划时的缓存。
pub trait SyncPlanEvidencePort: Send + Sync {
    /// 返回 `true` 仅表示账号、文件、schema 与 mapping 仍匹配不可变计划。
    fn is_current(&self, plan: &SyncPlan) -> bool;

    /// 提交后复核账号、数据位置和 schema；目标文件指纹允许因本次写入发生变化。
    fn is_current_after_commit(&self, plan: &SyncPlan) -> bool {
        self.is_current(plan)
    }
}

/// 同步执行端口；实现必须在受 guard 保护的 fixture 目标中完成所有副作用。
pub trait SyncPlanExecutorPort: Send + Sync {
    /// 执行计划允许的最小动作集合，并在写前、提交前及提交后重读证据。
    fn execute_sync_plan(
        &self,
        plan: &SyncPlan,
        cancellation: &OperationCancellation,
        evidence: &dyn SyncPlanEvidencePort,
    ) -> SyncPlanExecutionOutcome;
}
