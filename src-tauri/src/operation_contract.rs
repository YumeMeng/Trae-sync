//! 操作 command 稳定错误契约。
//!
//! DTO 只携带用户需要核对的状态，不携带路径、数据库正文、认证材料或 raw key。
//! （S4，2026-09-29：随旧同步计划下线的事件常量与未接线构造器已删除，git 历史可查。）

use serde::Serialize;

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
    fn gate_error_contract_serializes_stable_shape() {
        let error = CommandErrorDto::gate_not_qualified("测试门禁未通过。");
        let json = serde_json::to_value(error).unwrap();
        assert_eq!(json["code"], "gate_not_qualified");
        assert_eq!(json["message"], "测试门禁未通过。");
        assert_eq!(
            json["recommended_action"],
            "当前保持只读；待对应 Gate 通过后再使用。"
        );
        assert_eq!(json["retryable"], false);
        assert!(json.get("raw_key").is_none());
    }
}
