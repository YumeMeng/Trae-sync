//! Trae Sync 应用入口：注册 Tauri command 并启动 Tauri 运行时。
//!
//! 依赖方向：Tauri binary（组合根） -> commands + application + infrastructure + domain
//! 二进制作为组合根，负责实例化 infrastructure 的 provider 并注入到 commands。
//! `WorkspaceStateProvider` trait 通过 application 重导出获得，避免直接依赖 ports crate。

use std::sync::Arc;
use traesync_application::WorkspaceStateProvider;
use traesync_commands as commands;
use traesync_domain::WorkspaceState;
use traesync_infrastructure::StaticWorkspaceStateProvider;

/// 应用共享状态：在 Tauri command 之间共享的依赖。
struct AppState {
    provider: Arc<dyn WorkspaceStateProvider>,
}

/// `get_workspace_state` Tauri command。
/// T01 阶段返回固定的空工作台状态。
#[tauri::command]
fn get_workspace_state(state: tauri::State<AppState>) -> WorkspaceState {
    commands::get_workspace_state(state.provider.as_ref())
}

/// 启动 Tauri 应用。
pub fn run() {
    let provider = Arc::new(StaticWorkspaceStateProvider::new());

    tauri::Builder::default()
        .manage(AppState { provider })
        .invoke_handler(tauri::generate_handler![get_workspace_state])
        .run(tauri::generate_context!())
        .expect("启动 Tauri 应用时出错");
}
