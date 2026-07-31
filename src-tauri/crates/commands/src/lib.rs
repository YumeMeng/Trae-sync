//! Trae Sync Tauri 命令层：只做输入校验、调用应用服务和事件桥接。
//!
//! 对应规格第 30 节“前后端契约”：Tauri command 只返回结构化 DTO，
//! 不传递数据库连接、原始认证内容或任意 SQL。
//!
//! T01 骨架阶段只实现 `get_workspace_state` 命令。
//!
//! 依赖方向：commands 只依赖 application + domain，不直接依赖 ports/infrastructure。
//! `WorkspaceStateProvider` trait 通过 application 重导出获得。

use traesync_application::WorkspaceStateService;
use traesync_domain::WorkspaceState;
// trait 通过 application 重导出，避免 commands 直接依赖 ports crate
use traesync_application::WorkspaceStateProvider;

/// `get_workspace_state` 命令：返回空工作台状态。
///
/// 前端通过 `@tauri-apps/api` 的 `invoke("get_workspace_state")` 调用。
/// T01 阶段返回固定的空状态——所有真实能力禁用，UI 显示诚实状态。
pub fn get_workspace_state(provider: &dyn WorkspaceStateProvider) -> WorkspaceState {
    let service = WorkspaceStateService::new(provider);
    service.get_workspace_state()
}

#[cfg(test)]
mod tests {
    use super::*;
    use traesync_application::WorkspaceStateProvider;
    use traesync_domain::{CapabilityFlags, PlatformId, WorkspaceState};

    struct FakeProvider {
        state: WorkspaceState,
    }

    impl WorkspaceStateProvider for FakeProvider {
        fn get_workspace_state(&self) -> WorkspaceState {
            self.state.clone()
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
}
