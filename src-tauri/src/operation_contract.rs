//! T09 操作 command 与事件契约。
//!
//! 这些 DTO 只携带用户需要核对的状态，不携带路径、数据库正文、认证材料或 raw key。

use serde::Serialize;

// 事件桥接随旧同步计划功能线下线；常量由测试守护命名契约，事件功能回归时复用
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) const OPERATION_STAGE_EVENT: &str = "operation-stage";
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) const OPERATION_PROGRESS_EVENT: &str = "operation-progress";
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) const OPERATION_NEEDS_ATTENTION_EVENT: &str = "operation-needs-attention";
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) const OPERATION_FINISHED_EVENT: &str = "operation-finished";

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

    // 错误契约构造器由测试守护序列化形状，生产路径尚未接线
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn catalog_write_protocol_upgrade_required() -> Self {
        Self::new(
            "catalog_write_protocol_upgrade_required",
            "当前目录库采用了此版本不支持的写入协议，未写入新的历史或修改目录库。",
            "保留目录库及其 sidecar；不要手动删除 WAL/SHM，等待后续目录库旁路升级功能。",
            false,
        )
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn another_operation_running() -> Self {
        Self::new(
            "another_operation_running",
            "已有操作正在进行。",
            "等待当前操作结束后再重试。",
            true,
        )
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn no_pending_plan() -> Self {
        Self::new(
            "plan_unavailable",
            "没有可执行的同步计划。",
            "重新核对目标账号和范围，再生成计划。",
            true,
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
