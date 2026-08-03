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

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
// trait 通过 application 重导出，避免 commands 直接依赖 ports crate
use traesync_application::{
    AccountEvidenceReaderPort, ApplySyncPlanService, AssignProjectSourceService,
    BrowseHistoryService, BuildSyncPlanService, CatalogRepository, DatabaseProbePort,
    ScanHistoryService, SyncPlanEvidencePort, WorkbenchReadService, WorkspaceStateProvider,
};
use traesync_commands as commands;
// R8：组合根直接使用 commands::HistoryCommandError 构造授权失败错误消息
use traesync_commands::HistoryCommandError;
use traesync_domain::{
    AuthorizationState, BrowseResult, CompatibilityState, ConversationPreview, EvidenceState,
    OperationCancellation, ProcessRunningState, ScanOutcome, SearchHit, SessionIdentity, SyncPlan,
    SyncPlanContext, SyncPlanExecutionOutcome, SyncScope, TargetFileEvidence, WorkbenchReadState,
    WorkspaceState,
};
use traesync_infrastructure::{
    sha256_file, AccountEvidenceReader, FilesystemSnapshotStore, FixturePathGuard,
    PlatformFileIdentityProvider, SqlCipherCatalogRepository, SqlCipherProbe,
    StaticWorkspaceStateProvider, WorkCnSourceNormalizer, WorkCnSyncExecutor,
};

/// 扫描授权槽：服务端生成代次，确保晚完成的旧 grant/revoke 不能覆盖新意图。
struct AuthorizationSlot {
    generation: u64,
    state: AuthorizationState,
}

impl AuthorizationSlot {
    fn new(state: AuthorizationState) -> Self {
        Self {
            generation: 0,
            state,
        }
    }

    /// 开始一次授权请求，并立即使旧授权失效。
    fn begin_grant(&mut self) -> u64 {
        self.generation = self.generation.checked_add(1).expect("扫描授权代次溢出");
        self.state = AuthorizationState::NotAuthorized;
        self.generation
    }

    /// 仅最新授权请求可提交；旧请求晚返回时保持当前状态不变。
    fn commit_grant(&mut self, generation: u64, state: AuthorizationState) -> bool {
        if generation != self.generation {
            return false;
        }
        self.state = state;
        true
    }

    /// 撤销属于新的用户意图：递增代次，使所有更早的 pending grant 失效。
    fn revoke(&mut self) {
        self.generation = self.generation.checked_add(1).expect("扫描授权代次溢出");
        self.state = AuthorizationState::NotAuthorized;
    }
}

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
    authorization: Mutex<AuthorizationSlot>,
    /// T06：仅缓存后端生成的不可变计划，客户端不能反序列化或注入计划。
    pending_sync_plan: Mutex<Option<SyncPlan>>,
    /// T06：当前写操作的取消令牌；进入目标写入后执行器自行忽略取消。
    active_sync_cancellation: Mutex<Option<OperationCancellation>>,
}

/// 执行前和执行中都从 fixture 重读计划证据，绝不复用建计划时的内存快照。
struct RuntimeSyncPlanEvidence<'a> {
    fixture_root: &'a Path,
    target_db_path: &'a Path,
    raw_key: &'a str,
    reader: &'a AccountEvidenceReader,
    probe: &'a SqlCipherProbe,
}

impl SyncPlanEvidencePort for RuntimeSyncPlanEvidence<'_> {
    fn is_current(&self, plan: &SyncPlan) -> bool {
        read_sync_plan_context(
            self.reader,
            self.probe,
            self.raw_key,
            self.fixture_root,
            self.target_db_path,
            std::time::SystemTime::now(),
            true,
        )
        .map(|context| plan.matches_context(&context))
        .unwrap_or(false)
    }
}

/// 读取已授权 fixture 的目标数据库；未授权时在构造 guard 或访问文件系统前拒绝。
fn resolve_authorized_fixture(state: &AppState) -> Result<(FixturePathGuard, PathBuf), String> {
    let authorization = state.authorization.lock().unwrap().state.clone();
    let (fixture_root, db_relative_path) = match authorization {
        AuthorizationState::Authorized {
            canonical_fixture_root,
            db_relative_path,
        } => (canonical_fixture_root, db_relative_path),
        AuthorizationState::NotAuthorized => {
            return Err(HistoryCommandError::NotAuthorized.to_string())
        }
    };
    let guard = FixturePathGuard::new(Path::new(&fixture_root)).map_err(|e| e.to_string())?;
    let db_path = guard
        .validate_db_relative_path(&db_relative_path)
        .map_err(|e| e.to_string())?;
    Ok((guard, db_path))
}

/// 用当前账号、三件套、schema 和固定 mapping 构造计划证据上下文。
fn read_sync_plan_context(
    reader: &AccountEvidenceReader,
    probe: &SqlCipherProbe,
    raw_key: &str,
    fixture_root: &Path,
    db_path: &Path,
    now: std::time::SystemTime,
    re_read_after_close: bool,
) -> Result<SyncPlanContext, String> {
    let account = if re_read_after_close {
        reader.re_read_after_close(fixture_root, now)
    } else {
        reader.read_account_evidence(fixture_root, now)
    };
    if account.evidence_state != EvidenceState::Verified {
        return Err("当前账号证据未通过双来源验证，无法执行同步计划".to_string());
    }
    let current_user_id = account
        .user_id
        .as_ref()
        .map(|user_id| user_id.as_str().to_string())
        .ok_or_else(|| "当前账号证据不足，无法执行同步计划".to_string())?;
    let account_evidence_fingerprint = account
        .auth_fingerprint
        .as_ref()
        .map(|fingerprint| fingerprint.0.clone())
        .ok_or_else(|| "当前账号指纹缺失，无法执行同步计划".to_string())?;

    let compatibility = probe.probe_database(db_path, raw_key);
    let (schema_compatible, schema_fingerprint) = match compatibility {
        CompatibilityState::Verified {
            schema_fingerprint, ..
        } => (true, schema_fingerprint.0),
        CompatibilityState::Incompatible { .. } => (false, "incompatible".to_string()),
    };
    let wal_path = database_sidecar_path(db_path, "-wal");
    let shm_path = database_sidecar_path(db_path, "-shm");
    Ok(SyncPlanContext {
        created_at: now,
        platform_id: "work_cn".to_string(),
        data_location_id: fixture_root.to_string_lossy().into_owned(),
        current_user_id,
        account_evidence_fingerprint,
        target_file_evidence: TargetFileEvidence {
            db_fingerprint: sha256_file(db_path)
                .ok_or_else(|| "无法读取目标数据库指纹".to_string())?,
            wal_fingerprint: sha256_file(&wal_path),
            shm_fingerprint: sha256_file(&shm_path),
        },
        schema_fingerprint,
        mapping_version: "work_cn_v1".to_string(),
        schema_compatible,
    })
}

/// 生成同名 WAL 或 SHM 路径，保持 DB 三件套指纹读取的一致性。
fn database_sidecar_path(db_path: &Path, suffix: &str) -> PathBuf {
    let mut path = db_path.as_os_str().to_os_string();
    path.push(suffix);
    PathBuf::from(path)
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
    // 新授权代表新用户意图，旧计划不得继续作为可执行计划保留。
    state.pending_sync_plan.lock().unwrap().take();
    // R12-B：先登记服务端代次，再做路径验证。后发请求/撤销会使本次提交失效。
    let generation = state.authorization.lock().unwrap().begin_grant();
    // R1：用 FixturePathGuard 验证 fixture_root——拒绝真实 TRAE 路径与 disk root
    let guard = FixturePathGuard::new(Path::new(&fixture_root)).map_err(|e| e.to_string())?;
    let canonical = guard.canonical_root().to_string_lossy().into_owned();
    let authorization = AuthorizationState::Authorized {
        canonical_fixture_root: canonical.clone(),
        db_relative_path,
    };
    // 只有仍为最新代次时才能发布授权；旧请求不得覆盖后来的授权或撤销。
    if !state
        .authorization
        .lock()
        .unwrap()
        .commit_grant(generation, authorization)
    {
        return Err("授权请求已失效".to_string());
    }
    Ok(canonical)
}

/// `revoke_scan_authorization` Tauri command：撤销扫描授权。
///
/// R1：撤销后 scan_history 会立即拒绝。用于扫描完成或用户取消时清除授权。
#[tauri::command]
fn revoke_scan_authorization(state: tauri::State<AppState>) -> Result<(), String> {
    state.authorization.lock().unwrap().revoke();
    // 撤销授权同时使缓存计划失效，并请求当前操作在仍可取消时停止。
    state.pending_sync_plan.lock().unwrap().take();
    request_cancel_inner(&state);
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
    let authorization = state.authorization.lock().unwrap().state.clone();
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

/// `build_sync_plan` Tauri command：重新读取当前 fixture 账号、schema 和文件指纹，
/// 再由目录库生成不可变计划。此命令只读，不修改活动数据库或目录库。
#[tauri::command]
fn build_sync_plan(scope: SyncScope, state: tauri::State<AppState>) -> Result<SyncPlan, String> {
    if state.raw_key.is_empty() || state.storage_root.is_empty() {
        return Err("fixture 模式未启用：raw key 或 storage root 未配置".to_string());
    }
    if state.active_sync_cancellation.lock().unwrap().is_some() {
        return Err("同步操作正在进行，不能生成新计划".to_string());
    }
    // 每次重新规划先丢弃旧缓存，失败时也不能继续执行旧范围。
    state.pending_sync_plan.lock().unwrap().take();

    let now = std::time::SystemTime::now();
    let (guard, db_path) = resolve_authorized_fixture(&state)?;
    let context = read_sync_plan_context(
        &state.workbench_reader,
        &state.workbench_probe,
        &state.raw_key,
        guard.canonical_root(),
        &db_path,
        now,
        false,
    )?;

    let catalog_path = Path::new(&state.storage_root).join("catalog.db");
    let catalog = SqlCipherCatalogRepository::new(catalog_path, state.raw_key.clone());
    let service = BuildSyncPlanService::new(&catalog);
    let plan = commands::build_sync_plan(scope, context, &service).map_err(|e| e.to_string())?;
    // 仅服务端保留可执行副本；前端获得的 DTO 不能再作为执行输入传回。
    *state.pending_sync_plan.lock().unwrap() = Some(plan.clone());
    Ok(plan)
}

/// `apply_sync_plan` Tauri 命令：只执行服务端缓存的计划，不接受前端计划参数。
#[tauri::command]
fn apply_sync_plan(state: tauri::State<AppState>) -> Result<SyncPlanExecutionOutcome, String> {
    if state.raw_key.is_empty() || state.storage_root.is_empty() {
        return Err("fixture 模式未启用：raw key 或 storage root 未配置".to_string());
    }

    let cancellation = {
        let mut active = state.active_sync_cancellation.lock().unwrap();
        if active.is_some() {
            return Err("同步操作正在进行".to_string());
        }
        let cancellation = OperationCancellation::new();
        *active = Some(cancellation.clone());
        cancellation
    };
    let outcome = apply_cached_sync_plan(&state, &cancellation);
    state.active_sync_cancellation.lock().unwrap().take();
    outcome
}

/// 组合根执行缓存计划：绑定受保护路径并重新读取实时证据。
fn apply_cached_sync_plan(
    state: &AppState,
    cancellation: &OperationCancellation,
) -> Result<SyncPlanExecutionOutcome, String> {
    // 无缓存计划时不构造 guard、不访问 fixture，避免客户端把执行变成路径探测入口。
    let plan = state
        .pending_sync_plan
        .lock()
        .unwrap()
        .take()
        .ok_or_else(|| "没有可执行的同步计划，请先重新生成计划".to_string())?;
    let (guard, db_path) = resolve_authorized_fixture(state)?;
    let evidence = RuntimeSyncPlanEvidence {
        fixture_root: guard.canonical_root(),
        target_db_path: &db_path,
        raw_key: &state.raw_key,
        reader: &state.workbench_reader,
        probe: &state.workbench_probe,
    };
    let executor = WorkCnSyncExecutor::new(state.raw_key.clone());
    let executor = executor
        .bind_fixture(&guard, &db_path, Path::new(&state.storage_root))
        .map_err(|e| e.to_string())?;
    let service = ApplySyncPlanService::new(&evidence, &executor);
    Ok(commands::apply_sync_plan(&plan, cancellation, &service))
}

/// 标记当前操作取消；执行器会在进入目标写入后继续完成保护阶段。
#[tauri::command]
fn request_cancel(state: tauri::State<AppState>) -> bool {
    request_cancel_inner(&state)
}

/// 提取为纯共享逻辑，供撤销授权和组合根测试复用。
fn request_cancel_inner(state: &AppState) -> bool {
    let cancellation = state.active_sync_cancellation.lock().unwrap().clone();
    if let Some(cancellation) = cancellation {
        cancellation.request();
        true
    } else {
        false
    }
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
            authorization: Mutex::new(AuthorizationSlot::new(AuthorizationState::NotAuthorized)),
            pending_sync_plan: Mutex::new(None),
            active_sync_cancellation: Mutex::new(None),
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
            assign_source,
            build_sync_plan,
            apply_sync_plan,
            request_cancel
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
    use traesync_domain::{BuildSyncPlanInput, TargetFileEvidence};

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
            authorization: Mutex::new(AuthorizationSlot::new(authorization)),
            pending_sync_plan: Mutex::new(None),
            active_sync_cancellation: Mutex::new(None),
        }
    }

    /// 构造最小计划，仅用于验证组合根缓存与授权边界，不包含真实项目或数据。
    fn cached_test_plan() -> SyncPlan {
        traesync_domain::build_sync_plan(BuildSyncPlanInput {
            created_at: std::time::SystemTime::UNIX_EPOCH,
            platform_id: "work_cn".to_string(),
            data_location_id: "fixture-location".to_string(),
            current_user_id: "target-user".to_string(),
            account_evidence_fingerprint: "account-fingerprint".to_string(),
            target_file_evidence: TargetFileEvidence {
                db_fingerprint: "database-fingerprint".to_string(),
                wal_fingerprint: None,
                shm_fingerprint: None,
            },
            schema_fingerprint: "schema-fingerprint".to_string(),
            mapping_version: "work_cn_v1".to_string(),
            schema_compatible: true,
            scope: SyncScope::AllHistory,
            projects: vec![],
        })
    }

    #[test]
    fn t06_apply_requires_server_cached_plan_before_fixture_access() {
        let state = make_test_state(AuthorizationState::NotAuthorized);

        let err = apply_cached_sync_plan(&state, &OperationCancellation::new()).unwrap_err();

        assert!(err.contains("没有可执行的同步计划"));
    }

    #[test]
    fn t06_apply_rejects_unauthorized_cached_plan_before_fixture_access() {
        let state = make_test_state(AuthorizationState::NotAuthorized);
        *state.pending_sync_plan.lock().unwrap() = Some(cached_test_plan());

        let err = apply_cached_sync_plan(&state, &OperationCancellation::new()).unwrap_err();

        assert!(err.contains("未授权"));
    }

    #[test]
    fn t06_cancel_marks_only_current_operation_token() {
        let state = make_test_state(AuthorizationState::NotAuthorized);
        assert!(!request_cancel_inner(&state));

        let cancellation = OperationCancellation::new();
        *state.active_sync_cancellation.lock().unwrap() = Some(cancellation.clone());

        assert!(request_cancel_inner(&state));
        assert!(cancellation.is_requested());
    }

    #[test]
    fn r12_b_late_old_grant_cannot_overwrite_new_grant() {
        let mut slot = AuthorizationSlot::new(AuthorizationState::NotAuthorized);
        let generation_a = slot.begin_grant();
        let generation_b = slot.begin_grant();

        let authorization_b = AuthorizationState::Authorized {
            canonical_fixture_root: "D:/fixture-B".to_string(),
            db_relative_path: "database.db".to_string(),
        };
        assert!(slot.commit_grant(generation_b, authorization_b.clone()));

        let authorization_a = AuthorizationState::Authorized {
            canonical_fixture_root: "C:/fixture-A".to_string(),
            db_relative_path: "database.db".to_string(),
        };
        assert!(!slot.commit_grant(generation_a, authorization_a));
        assert_eq!(slot.state, authorization_b);
    }

    #[test]
    fn r12_b_revoke_invalidates_older_pending_grant() {
        let mut slot = AuthorizationSlot::new(AuthorizationState::NotAuthorized);
        let generation_a = slot.begin_grant();
        slot.revoke();

        let authorization_a = AuthorizationState::Authorized {
            canonical_fixture_root: "C:/fixture-A".to_string(),
            db_relative_path: "database.db".to_string(),
        };
        assert!(!slot.commit_grant(generation_a, authorization_a));
        assert_eq!(slot.state, AuthorizationState::NotAuthorized);
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
