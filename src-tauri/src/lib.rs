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
//!
//! T03 新增 `scan_history` / `browse_history` / `search_history` / `read_conversation`
//! / `assign_source` 命令：组合根负责
//! - 从环境变量 `TRAE_SYNC_FIXTURE_STORAGE_ROOT` 读取存储根
//! - 按命令构造 application service（注入 raw_key 与 normalizer）
//! - raw_key 不进入 commands 层、UI 或日志

use std::path::Path;
use std::sync::{Arc, Mutex};
// trait 通过 application 重导出，避免 commands 直接依赖 ports crate
use traesync_application::{
    AssignProjectSourceService, BrowseHistoryService, CatalogRepository, ScanHistoryService,
    WorkbenchReadService, WorkspaceStateProvider,
};
use traesync_commands as commands;
// R8：组合根直接使用 commands::HistoryCommandError 构造授权失败错误消息
use traesync_commands::HistoryCommandError;
use traesync_domain::{
    AuthorizationState, BrowseResult, ConversationPreview, ProcessRunningState, ScanOutcome,
    SearchHit, SessionIdentity, WorkbenchReadState, WorkspaceState,
};
use traesync_infrastructure::{
    AccountEvidenceReader, FilesystemSnapshotStore, FixturePathGuard, PlatformFileIdentityProvider,
    SqlCipherCatalogRepository, SqlCipherProbe, StaticWorkspaceStateProvider,
    WorkCnSourceNormalizer,
};

/// 应用共享状态：在 Tauri command 之间共享的依赖。
///
/// T02 新增 workbench_probe / workbench_reader / raw_key——这些在 fixture 模式下
/// 用于构造 `WorkbenchReadService`。raw_key 从 env 读取，不进入 UI/日志。
///
/// T03 新增 storage_root——从 env `TRAE_SYNC_FIXTURE_STORAGE_ROOT` 读取，
/// 作为快照发布与目录库存储根。各命令按需构造 service，不在 AppState 持有连接。
struct AppState {
    provider: Arc<dyn WorkspaceStateProvider>,
    /// T02 嵌入式 SQLCipher 探测器（空 struct，无状态）
    workbench_probe: SqlCipherProbe,
    /// T02 账号证据读取器（空 struct，无状态）
    workbench_reader: AccountEvidenceReader,
    /// SQLCipher raw key hex，从 env `TRAE_SYNC_FIXTURE_RAW_KEY` 读取。
    /// 空字符串表示未配置——命令返回错误，不泄露 key 状态。
    raw_key: String,
    /// 存储根路径，从 env `TRAE_SYNC_FIXTURE_STORAGE_ROOT` 读取。
    /// 快照发布到 `<storage_root>/snapshots/`，目录库位于 `<storage_root>/catalog.db`。
    /// 空字符串表示未配置——扫描命令返回错误。
    storage_root: String,
    /// R1：扫描授权状态——由后端持有，不接受前端注入。
    /// 用户通过 `grant_scan_authorization` 显式授权后设为 Authorized，
    /// 默认 NotAuthorized。scan_history 在任何 FS/DB 访问前检查此状态。
    authorization: Mutex<AuthorizationState>,
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

/// `grant_scan_authorization` Tauri command：用户显式授权扫描指定 fixture 路径。
///
/// R1 修复：后端拥有并验证显式用户授权。前端不能直接扫描——
/// 必须先调用此命令获得后端授权，授权绑定到 canonical fixture_root + db_relative_path。
/// 组合根负责用 `FixturePathGuard` 验证 fixture_root（拒绝真实 TRAE 路径），
/// 然后存储授权状态供后续 scan_history 检查。
///
/// 返回授权后的 canonical fixture_root（供前端显示与确认）。
#[tauri::command]
fn grant_scan_authorization(
    fixture_root: String,
    db_relative_path: String,
    state: tauri::State<AppState>,
) -> Result<String, String> {
    if state.raw_key.is_empty() || state.storage_root.is_empty() {
        return Err("fixture 模式未启用：raw key 或 storage root 未配置".to_string());
    }
    if db_relative_path.is_empty() {
        return Err("数据库相对路径不能为空".to_string());
    }
    // R1：用 FixturePathGuard 验证 fixture_root——拒绝真实 TRAE 路径与 disk root
    let guard = FixturePathGuard::new(Path::new(&fixture_root)).map_err(|e| e.to_string())?;
    let canonical = guard.canonical_root().to_string_lossy().into_owned();
    // 存储授权状态——绑定到 canonical fixture_root + db_relative_path
    *state.authorization.lock().unwrap() = AuthorizationState::Authorized {
        canonical_fixture_root: canonical.clone(),
        db_relative_path,
    };
    Ok(canonical)
}

/// `revoke_scan_authorization` Tauri command：撤销扫描授权。
///
/// R1：撤销后 scan_history 会立即拒绝。用于扫描完成或用户取消时清除授权。
#[tauri::command]
fn revoke_scan_authorization(state: tauri::State<AppState>) -> Result<(), String> {
    *state.authorization.lock().unwrap() = AuthorizationState::NotAuthorized;
    Ok(())
}

/// `scan_history` Tauri command：执行首次扫描。
///
/// R1 修复：后端在任何 FS/DB 访问之前检查显式用户授权与进程边界。
/// R8 修复：授权检查移到 `FixturePathGuard::new` 之前——未授权/范围不匹配/运行中
///   时不构造 guard、不 canonicalize、不访问文件系统。授权状态中已保存的
///   canonical_fixture_root 直接用作 canonical_root，避免对未授权请求的任意路径
///   做 canonicalize。
/// 前端通过 `invoke("scan_history", { fixtureRoot, dbRelativePath, processState })` 调用。
/// 组合根负责：
/// 1. 验证 raw_key 与 storage_root 已配置（storage_root 来自 env，不接受前端注入）
/// 2. R8：先锁定授权状态，未授权/范围不匹配/运行中时立即返回，不构造 guard
/// 3. R8：用授权状态中的 canonical_fixture_root 作为 canonical_root，跳过再次 canonicalize
/// 4. R2：用 `FixturePathGuard::new(canonical_root)` 构造 guard（此时路径已验证为授权范围）
///    并用 `validate_db_relative_path` 验证 db_relative_path 与派生 WAL/SHM 全部封闭
/// 5. 构造 SnapshotStore / Catalog / Normalizer / Service 并注入 raw_key
/// 6. 调用 `catalog.ensure_initialized()` 确保目录库表存在
/// 7. 调用 commands 层纯函数（内含二次授权检查——defense in depth）
///
/// raw_key 不进入 commands 层、UI 或日志。
/// storage_root 从 env `TRAE_SYNC_FIXTURE_STORAGE_ROOT` 读取，与 browse/search 等命令一致，
/// 不接受前端注入，避免路径注入面。
#[tauri::command]
fn scan_history(
    fixture_root: String,
    db_relative_path: String,
    process_state: ProcessRunningState,
    state: tauri::State<AppState>,
) -> Result<ScanOutcome, String> {
    scan_history_inner(&state, &fixture_root, &db_relative_path, process_state)
}

/// R8：`scan_history` 核心逻辑——提取为 `pub(crate)` 以便组合根级顺序测试。
///
/// 此函数完整复现 Tauri command 的行为，但接收 `&AppState` 而非 `tauri::State<AppState>`，
/// 使得 `#[cfg(test)]` 模块可直接调用并验证授权检查顺序。
fn scan_history_inner(
    state: &AppState,
    fixture_root: &str,
    db_relative_path: &str,
    process_state: ProcessRunningState,
) -> Result<ScanOutcome, String> {
    // 1. raw_key 或 storage_root 未配置时返回错误——不泄露 key 是否存在
    if state.raw_key.is_empty() || state.storage_root.is_empty() {
        return Err("fixture 模式未启用：raw key 或 storage root 未配置".to_string());
    }

    // 2. R8：先锁定授权状态进行检查——在任何 FS/DB 访问、guard 构造、canonicalize 之前
    let authorization = state.authorization.lock().unwrap().clone();
    let canonical_root_str = match &authorization {
        AuthorizationState::NotAuthorized => {
            // 未授权：立即拒绝，不构造 guard、不访问文件系统
            return Err(HistoryCommandError::NotAuthorized.to_string());
        }
        AuthorizationState::Authorized {
            canonical_fixture_root,
            db_relative_path: authorized_db_path,
        } => {
            // R8：使用授权建立阶段已保存的 canonical 范围完成早拒
            // 不对未授权请求的任意路径做 canonicalize
            if fixture_root != *canonical_fixture_root || db_relative_path != *authorized_db_path {
                return Err(HistoryCommandError::AuthorizationMismatch.to_string());
            }
            // R1：运行中早拒——在任何 DB probing/account-evidence 读之前
            if process_state == ProcessRunningState::Running {
                return Err(HistoryCommandError::ProcessRunning.to_string());
            }
            canonical_fixture_root.clone()
        }
    };

    // 3. R8：用授权状态中的 canonical_root 构造 guard——此时路径已验证为授权范围
    //    guard 不会对未授权路径做 canonicalize；此处用于 R2 完整封闭证明（DB+WAL+SHM+symlink）
    let guard = FixturePathGuard::new(Path::new(&canonical_root_str)).map_err(|e| e.to_string())?;
    let canonical_root = guard.canonical_root();

    // 4. R2：用 FixturePathGuard 验证 db_relative_path 与派生 WAL/SHM 全部封闭在 fixture_root 内
    //    在任何 DB probing / 文件复制之前完成，拒绝绝对路径、父目录遍历与 symlink/junction 逃逸
    //    返回的规范化 DB 路径严格位于 canonical_root 内部——后续 infrastructure 调用可信任此路径
    let _canonical_db_path = guard
        .validate_db_relative_path(db_relative_path)
        .map_err(|e| e.to_string())?;

    // 5. 构造 infrastructure 组件——raw_key 仅在此注入，不进入 commands 层
    let file_identity = PlatformFileIdentityProvider::new();
    let snapshot_store = FilesystemSnapshotStore::new(Box::new(file_identity));
    let catalog_path = Path::new(&state.storage_root).join("catalog.db");
    let catalog = SqlCipherCatalogRepository::new(catalog_path, state.raw_key.clone());
    let normalizer = WorkCnSourceNormalizer::new(state.raw_key.clone());

    // 6. 确保目录库表已初始化（首次扫描前必须完成，否则 project_snapshot 会失败）
    catalog.ensure_initialized();

    // 7. 构造 application service
    let service = ScanHistoryService::new(
        &snapshot_store,
        &catalog,
        &state.workbench_probe,
        &state.workbench_reader,
        &normalizer,
        &state.raw_key,
    );

    // 8. 调用 commands 层纯函数——内含二次授权检查（defense in depth）
    let now = std::time::SystemTime::now();
    commands::scan_history(
        canonical_root,
        db_relative_path,
        process_state,
        Path::new(&state.storage_root),
        now,
        &authorization,
        &service,
    )
    .map_err(|e| e.to_string())
}

/// `browse_history` Tauri command：浏览全部历史。
///
/// 前端通过 `invoke("browse_history")` 调用。
/// 组合根构造 catalog（注入 raw_key）并委托 BrowseHistoryService。
#[tauri::command]
fn browse_history(state: tauri::State<AppState>) -> Result<BrowseResult, String> {
    if state.raw_key.is_empty() || state.storage_root.is_empty() {
        return Err("fixture 模式未启用：raw key 或 storage root 未配置".to_string());
    }
    let catalog_path = Path::new(&state.storage_root).join("catalog.db");
    let catalog = SqlCipherCatalogRepository::new(catalog_path, state.raw_key.clone());
    let service = BrowseHistoryService::new(&catalog);
    commands::browse_history(&service).map_err(|e| e.to_string())
}

/// `search_history` Tauri command：搜索消息内容。
///
/// 前端通过 `invoke("search_history", { query })` 调用。
#[tauri::command]
fn search_history(query: String, state: tauri::State<AppState>) -> Result<Vec<SearchHit>, String> {
    if state.raw_key.is_empty() || state.storage_root.is_empty() {
        return Err("fixture 模式未启用：raw key 或 storage root 未配置".to_string());
    }
    let catalog_path = Path::new(&state.storage_root).join("catalog.db");
    let catalog = SqlCipherCatalogRepository::new(catalog_path, state.raw_key.clone());
    let service = BrowseHistoryService::new(&catalog);
    commands::search_history(&query, &service).map_err(|e| e.to_string())
}

/// `read_conversation` Tauri command：读取完整对话预览。
///
/// 前端通过 `invoke("read_conversation", { session })` 调用。
/// session 为 SessionIdentity { product_history_namespace, original_session_id }。
#[tauri::command]
fn read_conversation(
    session: SessionIdentity,
    state: tauri::State<AppState>,
) -> Result<Option<ConversationPreview>, String> {
    if state.raw_key.is_empty() || state.storage_root.is_empty() {
        return Err("fixture 模式未启用：raw key 或 storage root 未配置".to_string());
    }
    let catalog_path = Path::new(&state.storage_root).join("catalog.db");
    let catalog = SqlCipherCatalogRepository::new(catalog_path, state.raw_key.clone());
    let service = BrowseHistoryService::new(&catalog);
    commands::read_conversation(&session, &service).map_err(|e| e.to_string())
}

/// `assign_source` Tauri command：分配项目来源（Gate E）。
///
/// 前端通过 `invoke("assign_source", { projectId, userAssignedOwner })` 调用。
/// user_assigned_owner 为 null 表示清除用户分配。
#[tauri::command]
fn assign_source(
    project_id: String,
    user_assigned_owner: Option<String>,
    state: tauri::State<AppState>,
) -> Result<bool, String> {
    if state.raw_key.is_empty() || state.storage_root.is_empty() {
        return Err("fixture 模式未启用：raw key 或 storage root 未配置".to_string());
    }
    let catalog_path = Path::new(&state.storage_root).join("catalog.db");
    let catalog = SqlCipherCatalogRepository::new(catalog_path, state.raw_key.clone());
    let service = AssignProjectSourceService::new(&catalog);
    let now = std::time::SystemTime::now();
    commands::assign_source(&project_id, user_assigned_owner.as_deref(), now, &service)
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
    // storage_root 从环境变量读取——T03 fixture 模式专用
    // 快照发布到 <storage_root>/snapshots/，目录库位于 <storage_root>/catalog.db
    let storage_root = std::env::var("TRAE_SYNC_FIXTURE_STORAGE_ROOT").unwrap_or_default();

    tauri::Builder::default()
        .manage(AppState {
            provider,
            workbench_probe,
            workbench_reader,
            raw_key,
            storage_root,
            // R1：默认未授权——必须由 grant_scan_authorization 显式授权后才能扫描
            authorization: Mutex::new(AuthorizationState::NotAuthorized),
        })
        .invoke_handler(tauri::generate_handler![
            get_workspace_state,
            read_work_cn_state,
            // R1：授权命令——前端必须先调用 grant_scan_authorization 才能 scan_history
            grant_scan_authorization,
            revoke_scan_authorization,
            scan_history,
            browse_history,
            search_history,
            read_conversation,
            assign_source
        ])
        .run(tauri::generate_context!())
        .expect("启动 Tauri 应用时出错");
}

// ============================================================================
// R8：组合根级顺序测试——证明拒绝发生时没有构造 guard 或访问文件系统
// ============================================================================

#[cfg(test)]
mod r8_order_tests {
    use super::*;

    /// 构造测试用 AppState——使用不存在的 fixture 路径与临时 storage_root。
    /// R8 反例：未授权时即使 fixture_root 是不存在的路径，也应返回 NotAuthorized，
    /// 而不是 guard 构造错误。这间接证明授权检查在 guard 之前——guard 未被构造。
    fn make_test_state(authorization: AuthorizationState) -> AppState {
        AppState {
            provider: Arc::new(StaticWorkspaceStateProvider::new()),
            workbench_probe: SqlCipherProbe::new(),
            workbench_reader: AccountEvidenceReader::new(),
            raw_key: "0".repeat(64),
            storage_root: "/nonexistent/storage-root".to_string(),
            authorization: Mutex::new(authorization),
        }
    }

    #[test]
    fn r8_unauthorized_returns_not_authorized_without_guard_construction() {
        // 未授权状态——fixture_root 指向不存在的路径
        // 若授权检查在 guard 之后，FixturePathGuard::new 会因路径不存在而失败，
        // 返回 guard 错误而非 NotAuthorized。
        // R8 修复后：授权检查在 guard 之前，返回 NotAuthorized，guard 未被构造。
        let state = make_test_state(AuthorizationState::NotAuthorized);
        let result = scan_history_inner(
            &state,
            "/nonexistent/fixture-root",
            "database.db",
            ProcessRunningState::NotRunning,
        );
        // 必须返回 NotAuthorized 错误——不是 guard 错误
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            err.contains("未授权") || err.contains("not_authorized"),
            "未授权时应返回 NotAuthorized，实际: {err}"
        );
        // 不应包含 guard 错误特征（如 "canonical" 或 "fixture"）
        assert!(
            !err.to_lowercase().contains("canonical"),
            "未授权时不应触发 guard 构造（canonicalize），实际: {err}"
        );
    }

    #[test]
    fn r8_authorization_mismatch_returns_mismatch_without_guard_construction() {
        // 已授权路径 A，但请求路径 B（不存在）
        // R8：授权范围不匹配时立即返回 AuthorizationMismatch，不构造 guard
        let state = make_test_state(AuthorizationState::Authorized {
            canonical_fixture_root: "/authorized/fixture-root".to_string(),
            db_relative_path: "database.db".to_string(),
        });
        let result = scan_history_inner(
            &state,
            "/nonexistent/different-fixture-root",
            "database.db",
            ProcessRunningState::NotRunning,
        );
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            err.contains("授权不匹配") || err.contains("authorization"),
            "范围不匹配时应返回 AuthorizationMismatch，实际: {err}"
        );
        // 不应触发 guard 构造
        assert!(
            !err.to_lowercase().contains("canonical"),
            "范围不匹配时不应触发 guard 构造，实际: {err}"
        );
    }

    #[test]
    fn r8_process_running_returns_process_running_without_guard_construction() {
        // 已授权但 TRAE 运行中——R8：返回 ProcessRunning，不构造 guard
        let state = make_test_state(AuthorizationState::Authorized {
            canonical_fixture_root: "/authorized/fixture-root".to_string(),
            db_relative_path: "database.db".to_string(),
        });
        let result = scan_history_inner(
            &state,
            "/authorized/fixture-root",
            "database.db",
            ProcessRunningState::Running,
        );
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            err.contains("运行中") || err.contains("running"),
            "运行中时应返回 ProcessRunning，实际: {err}"
        );
        // 不应触发 guard 构造
        assert!(
            !err.to_lowercase().contains("canonical"),
            "运行中时不应触发 guard 构造，实际: {err}"
        );
    }
}
