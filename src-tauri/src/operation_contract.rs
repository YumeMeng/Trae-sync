//! T09 操作 command 与事件契约。
//!
//! 这些 DTO 只携带用户需要核对的状态，不携带路径、数据库正文、认证材料或 raw key。

// 根 crate 的 test cfg 不启动 Tauri runtime；生产事件 DTO 仍由编译检查覆盖。
#![cfg_attr(test, allow(dead_code))]

use serde::Serialize;
use traesync_domain::{OperationState, SyncPlanExecutionOutcome};
use traesync_infrastructure::operation_manifest::{
    OperationReconciliationStatus, OperationReconciliationSummary,
};
use traesync_infrastructure::{
    OperationManifestError, OperationSummary, ProgressPhase, ProgressSnapshot,
};

pub(crate) const OPERATION_STAGE_EVENT: &str = "operation-stage";
pub(crate) const OPERATION_PROGRESS_EVENT: &str = "operation-progress";
pub(crate) const OPERATION_NEEDS_ATTENTION_EVENT: &str = "operation-needs-attention";
pub(crate) const OPERATION_FINISHED_EVENT: &str = "operation-finished";

/// 前端操作列表使用的稳定 DTO。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct OperationDto {
    pub(crate) operation_id: String,
    pub(crate) state: OperationState,
    pub(crate) data_location_id: String,
    pub(crate) sequence: u64,
    pub(crate) has_verified_target_file_evidence: bool,
}

impl From<OperationSummary> for OperationDto {
    fn from(summary: OperationSummary) -> Self {
        Self {
            operation_id: summary.operation_id,
            state: summary.state,
            data_location_id: summary.data_location_id,
            sequence: summary.sequence,
            has_verified_target_file_evidence: summary.has_verified_target_file_evidence,
        }
    }
}

/// 未完成操作协调 command 的白名单响应；不暴露路径、哈希、指纹或备份引用。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct ReconcileUnfinishedOperationsDto {
    pub(crate) inspected_count: u64,
    pub(crate) reconciled_count: u64,
    pub(crate) not_applied_count: u64,
    pub(crate) completed_count: u64,
    pub(crate) manual_recovery_required_count: u64,
    pub(crate) unrelated_data_location_count: u64,
    pub(crate) status: OperationReconciliationStatus,
}

impl From<OperationReconciliationSummary> for ReconcileUnfinishedOperationsDto {
    fn from(summary: OperationReconciliationSummary) -> Self {
        Self {
            inspected_count: summary.inspected_count,
            reconciled_count: summary.reconciled_count,
            not_applied_count: summary.not_applied_count,
            completed_count: summary.completed_count,
            manual_recovery_required_count: summary.manual_recovery_required_count,
            unrelated_data_location_count: summary.unrelated_data_location_count,
            status: summary.status,
        }
    }
}

/// Tauri command 的稳定错误 DTO；不把底层路径或原始错误正文直接交给 UI。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) struct CommandErrorDto {
    pub(crate) code: String,
    pub(crate) message: String,
    pub(crate) recommended_action: String,
    pub(crate) retryable: bool,
}

impl CommandErrorDto {
    pub(crate) fn new(
        code: &'static str,
        message: &'static str,
        recommended_action: &'static str,
        retryable: bool,
    ) -> Self {
        Self {
            code: code.to_string(),
            message: message.to_string(),
            recommended_action: recommended_action.to_string(),
            retryable,
        }
    }

    pub(crate) fn storage_root_unavailable() -> Self {
        Self::new(
            "storage_root_unavailable",
            "恢复区不可用，无法读取操作状态。",
            "检查存储根和恢复区后重试。",
            true,
        )
    }

    pub(crate) fn operation_state_unavailable() -> Self {
        Self::new(
            "operation_state_unavailable",
            "操作状态暂时不可读取。",
            "保留现有证据，稍后重新打开操作页重试。",
            true,
        )
    }

    pub(crate) fn catalog_write_protocol_upgrade_required() -> Self {
        Self::new(
            "catalog_write_protocol_upgrade_required",
            "当前目录库采用了此版本不支持的写入协议，未写入新的历史或修改目录库。",
            "保留目录库及其 sidecar；不要手动删除 WAL/SHM，等待后续目录库旁路升级功能。",
            false,
        )
    }

    pub(crate) fn another_operation_running() -> Self {
        Self::new(
            "another_operation_running",
            "已有操作正在进行。",
            "等待当前操作结束后再重试。",
            true,
        )
    }

    pub(crate) fn no_pending_plan() -> Self {
        Self::new(
            "plan_unavailable",
            "没有可执行的同步计划。",
            "重新核对目标账号和范围，再生成计划。",
            true,
        )
    }

    pub(crate) fn reconciliation_not_authorized() -> Self {
        Self::new(
            "not_authorized",
            "当前数据位置尚未获得有效授权。",
            "重新授权当前 fixture 数据位置后重试。",
            true,
        )
    }

    pub(crate) fn reconciliation_drift() -> Self {
        Self::new(
            "data_location_changed",
            "当前账号或数据位置证据已变化，未协调任何操作记录。",
            "保留现有恢复证据，重新授权并重新生成计划。",
            false,
        )
    }

    pub(crate) fn gate_not_qualified(message: &'static str) -> Self {
        Self::new(
            "gate_not_qualified",
            message,
            "当前保持只读；待对应 Gate 通过后再使用。",
            false,
        )
    }

    pub(crate) fn from_manifest(error: &OperationManifestError) -> Self {
        match error {
            OperationManifestError::StorageRootUnavailable => Self::storage_root_unavailable(),
            OperationManifestError::DirectoryUnreadable => Self::new(
                "operation_state_unavailable",
                "操作记录目录不可读取。",
                "保留恢复区内容，检查磁盘后重试。",
                true,
            ),
            OperationManifestError::InvalidManifest => Self::new(
                "operation_record_invalid",
                "操作记录损坏，不能把它当作空记录处理。",
                "保留失败证据并进入人工恢复。",
                false,
            ),
            OperationManifestError::ReconciliationFailed => Self::new(
                "operation_reconciliation_failed",
                "未完成操作协调失败，现有恢复证据已保留。",
                "不要启动 TRAE；保留恢复区内容并进入人工恢复。",
                false,
            ),
        }
    }
}

/// 阶段事件：前端可据此更新阶段和取消边界。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct OperationStageEventDto {
    pub(crate) operation_id: String,
    pub(crate) phase: ProgressPhase,
    pub(crate) cancellable: bool,
}

/// 进度事件载荷与持久化进度快照保持同构。
pub(crate) type OperationProgressEventDto = ProgressSnapshot;

/// 需要用户关注的结果事件；错误仍使用稳定 DTO。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct OperationNeedsAttentionEventDto {
    pub(crate) operation_id: String,
    pub(crate) error: CommandErrorDto,
}

/// 终态事件携带结构化执行结果，前端丢事件后仍可通过 command 重查。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct OperationFinishedEventDto {
    pub(crate) operation_id: String,
    pub(crate) outcome: SyncPlanExecutionOutcome,
}

pub(crate) fn attention_error_for_outcome(
    outcome: &SyncPlanExecutionOutcome,
) -> Option<CommandErrorDto> {
    match outcome {
        SyncPlanExecutionOutcome::PlanExpired { .. } => Some(CommandErrorDto::new(
            "plan_expired",
            "同步计划已失效，目标或账号证据发生变化。",
            "重新确认目标账号和数据位置，再生成计划。",
            true,
        )),
        SyncPlanExecutionOutcome::FailedAfterWrite { .. } => Some(CommandErrorDto::new(
            "verification_inconclusive",
            "写入后验证未能形成可证明结论。",
            "不要启动 TRAE；保留失败现场并进入人工恢复。",
            false,
        )),
        SyncPlanExecutionOutcome::ManualRecoveryRequired { .. } => Some(CommandErrorDto::new(
            "manual_recovery_required",
            "当前操作需要人工恢复。",
            "不要覆盖现有数据库，使用保留的备份和失败证据恢复。",
            false,
        )),
        SyncPlanExecutionOutcome::FailedBeforeWrite { .. } => Some(CommandErrorDto::new(
            "operation_failed",
            "操作在写入前失败，目标未应用本次变更。",
            "保留备份并检查操作记录后重试。",
            true,
        )),
        SyncPlanExecutionOutcome::UnsupportedPlan => Some(CommandErrorDto::gate_not_qualified(
            "当前同步计划类型尚未达到可执行 Gate。",
        )),
        SyncPlanExecutionOutcome::Completed { .. }
        | SyncPlanExecutionOutcome::CancelledBeforeWrite { .. } => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn operation_contract_serializes_stable_event_names_and_error_shape() {
        assert_eq!(OPERATION_STAGE_EVENT, "operation-stage");
        assert_eq!(OPERATION_PROGRESS_EVENT, "operation-progress");
        assert_eq!(OPERATION_NEEDS_ATTENTION_EVENT, "operation-needs-attention");
        assert_eq!(OPERATION_FINISHED_EVENT, "operation-finished");

        let error = CommandErrorDto::no_pending_plan();
        let json = serde_json::to_value(error).unwrap();
        assert_eq!(json["code"], "plan_unavailable");
        assert!(json.get("raw_key").is_none());

        // busy 是公开 command 的稳定错误契约，不能降级为状态读取失败。
        let busy = serde_json::to_value(CommandErrorDto::another_operation_running()).unwrap();
        assert_eq!(busy["code"], "another_operation_running");
        assert_eq!(busy["retryable"], true);

        let protocol =
            serde_json::to_value(CommandErrorDto::catalog_write_protocol_upgrade_required())
                .unwrap();
        assert_eq!(protocol["code"], "catalog_write_protocol_upgrade_required");
        assert_eq!(
            protocol["message"],
            "当前目录库采用了此版本不支持的写入协议，未写入新的历史或修改目录库。"
        );
        assert_eq!(
            protocol["recommended_action"],
            "保留目录库及其 sidecar；不要手动删除 WAL/SHM，等待后续目录库旁路升级功能。"
        );
        assert_eq!(protocol["retryable"], false);
    }
}
