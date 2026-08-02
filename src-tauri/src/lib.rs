//! Trae Sync 应用入口：注册 Tauri command 并启动 Tauri 运行时。
//!
//! 依赖方向：Tauri binary（组合根） -> commands + application + infrastructure + domain
//! 二进制作为组合根，负责实例化 infrastructure 的 provider 并注入到 commands。
//! `WorkspaceStateProvider` trait 通过 application 重导出获得，避免直接依赖 ports crate。
//!
//! T02 新增 `read_work_cn_state` 命令：组合根负责
//! - 用 `FixturePathGuard` 验证 fixture_root（拒绝真实 TRAE 路径）
//! - 从环境变量 `TRAE_SYNC_FIXTURE_RAW_KEY` 读取 raw_key 并注入 service
//!   raw_key 不进入 commands 层、UI 或日志

use std::path::Path;
use std::sync::Arc;
// trait 通过 application 重导出，避免 commands 直接依赖 ports crate
use traesync_application::{
    AccountEvidenceReaderPort, DatabaseProbePort, WorkbenchReadService, WorkspaceStateProvider,
};
use traesync_commands as commands;
use traesync_domain::{WorkbenchReadState, WorkspaceState};
use traesync_infrastructure::{
    AccountEvidenceReader, FixturePathGuard, SqlCipherProbe, StaticWorkspaceStateProvider,
};

/// 应用共享状态：在 Tauri command 之间共享的依赖。
///
/// T02 新增 workbench_probe / workbench_reader / raw_key——这些在 fixture 模式下
/// 用于构造 `WorkbenchReadService`。raw_key 从 env 读取，不进入 UI/日志。
struct AppState {
    provider: Arc<dyn WorkspaceStateProvider>,
    /// T02 嵌入式 SQLCipher 探测器（空 struct，无状态）
    workbench_probe: SqlCipherProbe,
    /// T02 账号证据读取器（空 struct，无状态）
    workbench_reader: AccountEvidenceReader,
    /// SQLCipher raw key hex，从 env `TRAE_SYNC_FIXTURE_RAW_KEY` 读取。
    /// 空字符串表示未配置——命令返回错误，不泄露 key 状态。
    raw_key: String,
}

/// `get_workspace_state` Tauri command。
/// T01 阶段返回固定的空工作台状态。
#[tauri::command]
fn get_workspace_state(state: tauri::State<AppState>) -> WorkspaceState {
    commands::get_workspace_state(state.provider.as_ref())
}

/// `read_work_cn_state` Tauri command：返回 Work CN 只读工作台状态。
///
/// 前端通过 `invoke("read_work_cn_state", { fixtureRoot, dbRelativePath })` 调用。
/// 组合根负责：
/// 1. 验证 raw_key 已配置（未配置时返回错误，不泄露 key 是否存在）
/// 2. 用 `FixturePathGuard` 验证 fixture_root（拒绝真实 TRAE 路径与 disk root）
/// 3. 构造 `WorkbenchReadService`（注入 raw_key）并调用 commands 层纯函数
///
/// 返回 `WorkbenchReadState` 不含 raw_key、认证正文或底层错误原文。
#[tauri::command]
fn read_work_cn_state(
    fixture_root: String,
    db_relative_path: String,
    state: tauri::State<AppState>,
) -> Result<WorkbenchReadState, String> {
    // 1. raw_key 未配置时返回错误——不泄露 key 是否存在
    if state.raw_key.is_empty() {
        return Err("fixture 模式未启用：raw key 未配置".to_string());
    }

    // 2. 用 FixturePathGuard 验证 fixture_root——拒绝真实 TRAE 路径
    let guard = FixturePathGuard::new(Path::new(&fixture_root)).map_err(|e| e.to_string())?;

    // 3. 构造 service（注入 raw_key）并调用 commands 层纯函数
    let service = WorkbenchReadService::new(
        &state.workbench_probe,
        &state.workbench_reader,
        &state.raw_key,
    );
    // now 显式传入，避免 commands/application 依赖 wall clock
    let now = std::time::SystemTime::now();
    commands::build_work_cn_state(guard.canonical_root(), &db_relative_path, now, &service)
        .map_err(|e| e.to_string())
}

/// 启动 Tauri 应用。
pub fn run() {
    let provider = Arc::new(StaticWorkspaceStateProvider::new());
    let workbench_probe = SqlCipherProbe::new();
    let workbench_reader = AccountEvidenceReader::new();
    // raw_key 从环境变量读取——T02 fixture 模式专用，生产环境不设
    // 不进入 UI、日志或证据
    let raw_key = std::env::var("TRAE_SYNC_FIXTURE_RAW_KEY").unwrap_or_default();

    tauri::Builder::default()
        .manage(AppState {
            provider,
            workbench_probe,
            workbench_reader,
            raw_key,
        })
        .invoke_handler(tauri::generate_handler![
            get_workspace_state,
            read_work_cn_state
        ])
        .run(tauri::generate_context!())
        .expect("启动 Tauri 应用时出错");
}
