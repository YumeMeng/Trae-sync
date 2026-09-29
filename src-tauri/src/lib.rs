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
use std::sync::{atomic::AtomicBool, Arc, Mutex};

use serde::Serialize;
use tauri::Manager;
// trait 通过 application 重导出，避免 commands 直接依赖 ports crate
use traesync_application::{
    AccountEvidenceReaderPort, ProcessControllerPort, WorkbenchReadService, WorkspaceStateProvider,
};
use traesync_commands as commands;
// R8：组合根直接使用 commands::HistoryCommandError 构造授权失败错误消息
use traesync_commands::HistoryCommandError;
use traesync_domain::{
    AccountEvidence, AuthorizationState, CurrentAccountState, EvidenceState, ManagedAccountRuntime,
    OperationCancellation, SyncPlan, WorkbenchReadState, WorkspaceState,
};
use traesync_infrastructure::{
    capture_location_witness, compare_location_identity, fetch_installed_plugins,
    fetch_market_plugins, find_uninstall_target, install_market_plugin,
    open_or_initialize_production_catalog, resolve_current_catalog_path, storage_space_status,
    uninstall_cloud_plugin, user_id_binding_fingerprint, user_id_display_fingerprint,
    AccountEvidenceReader, AccountRecord, AccountRegistry, CheckinCredentialStore,
    CheckinProfileBinding, CloudPluginItem, FixturePathGuard, FixtureWorkspaceStateProvider,
    MarketPluginItem, PlatformFileIdentityProvider, PluginManifest, PluginManifestEntry,
    ProductionCatalogError, ProductionCatalogRuntime, RealReadWorkspaceStateProvider,
    SqlCipherProbe, StorageDeletionPlan, StorageMigrationResult, StorageRootBinding,
    WorkCnProcessController, WorkCnReadLocation, DEFAULT_STORAGE_WARNING_BYTES,
};

#[cfg(test)]
use traesync_infrastructure::{FixedProcessController, StaticWorkspaceStateProvider};

use traesync_infrastructure::{
    build_deletion_plan_with_lease, DeletionCandidate, DeletionTombstone,
};
// P7-1 追加 get_user_info_full（切号 E2 构造的实调身份校验与资料来源）。
// P7-5 追加 CheckinHttpError / UserInfoFull（健康度凭据包实调判定）。
// P12-R3：get_user_info_full_for_oauth_client 随凭据域迁 credential_handover。
use traesync_infrastructure::{
    get_user_info_full, trae_http_client, CheckinHttpError, LoginSession, OAuthClient, UserInfoFull,
};
// P2-2 TRAE 实例管理：路径发现、进程状态、窗口聚焦、登录态种子。
// P12-R3：close_instance / seed_login_state_from_donor / CloseOutcome 随
// 环境/库域迁出组合根，经对应域模块引用；close_master_instance /
// command_line_uses_data_dir / dir_size_recursive / native_account_dir 随凭据域
// 迁 credential_handover。
use traesync_infrastructure::trae_instance::{
    self as trae_instance_module, discover_trae_executable, focus_instance_windows,
    instance_data_dir, list_trae_processes, seed_login_state,
};
// P5-0 主库环境注册表（环境模型 grill Q1/Q2/Q7：V1 单主库环境档案持久化）。
// P12-R3：environment_registry_error_code / master_data_dir_for /
// secondary_data_dir 随环境/库域迁出组合根，经对应域模块引用；
// EnvironmentRegistry 随凭据域（自检/切号写回）迁 credential_handover。
use traesync_infrastructure::environment_registry::{master_data_dir, MASTER_ENV_ID};
// P12-R1b：产品×库 → 当前账号解析（原组合根 resolve_library_account 系下沉）。
// P12-R3：observed_profile_id / trae_cn_material_root_from 随凭据域迁
// credential_handover，经该模块引用。
use traesync_infrastructure::library_account::{self, TraeCnPointerLookup};

// 仅给 Windows unit-test 二进制声明 Common Controls v6，避免 tao 启动时找不到 TaskDialogIndirect。
#[cfg(all(test, windows))]
#[used]
#[link_section = ".drectve"]
static COMMON_CONTROLS_V6_DIRECTIVE: [u8; 167] = *b"/manifestdependency:\"type='win32' name='Microsoft.Windows.Common-Controls' version='6.0.0.0' processorArchitecture='*' publicKeyToken='6595b64144ccf1df' language='*'\"\0";

mod checkin_overview;
// P12-R3：直连签到 + 自动签到调度域自 lib.rs 纯移动；命令注册改路径形式。
// BatchProgressEvent / run_serial_batch 随 checkin 域归位，凭据刷新/健康检测/
// 积分刷新链留守组合根，经下方转发导入引用。
mod checkin_schedule;
mod operation_contract;
// P12-R2a：命令层错误映射集中地（原 lib.rs 内散布的 7 个映射函数收拢）。
mod error_mapping;
// P12-R3：远程设备域（ADR-0031）自 lib.rs 纯移动；产品账号路由留在组合根。
mod remote_devices;
// P12-R3：OAuth 登录命令链（M4/P6-3/P7-4）自 lib.rs 纯移动；命令注册改路径形式。
mod oauth_login;
// P12-R3：环境注册与实例生命周期域（P5-0 主库实例 + P6-4 多环境）自 lib.rs
// 纯移动；登录事务引用的库备份工具经 crate::library_archive 引用。
mod environment;
// P12-R3：库历史/归档/备份域（ADR-0025 库抽象 + P5-3/5-4/5-5/5-7/5-8/5-9/
// P11-4/11-5）自 lib.rs 纯移动；切号与彻底删除引用的备份工具经
// crate::library_archive 转发导入。
mod library_archive;
// P12-R3：账号/凭据/切号域（账号中心 + 账号增删改查 + 凭据刷新三链 + Trae CN
// 健康检测 + 主库切号五步 + G18 自检 + 设备重铸 + 占用统计/彻底删除）自 lib.rs
// 纯移动；命令注册改路径形式。
mod credential_handover;
// P12-R3：跨域测试共享夹具（AppState 构造），仅测试编译期可用。
#[cfg(test)]
mod test_support;
// P12-R3：自动签到调度线程由组合根启动；凭据刷新三链/健康检测随凭据域迁
// credential_handover（进度事件契约与串行运行器经该模块引用）；
// get_relay_ledger / 账号列表 / oauth_login 沿用同一签到材料根约定。
use checkin_schedule::{checkin_material_root, spawn_auto_checkin_scheduler};
// P12-R3：启动恢复扫描（run()）调用凭据域的凭证恢复原语。
use credential_handover::{recover_managed_account_runtime_from_disk, recovery_error_runtime};
// P12-R2a：映射函数随迁专用模块；转发导入保持 lib.rs 调用面零改动。
// P12-R3：map_master_archive_error 随库域迁 library_archive（经该模块引用）；
// credential_error_text / credential_maintenance_error_code / trae_cn_health_error_code
// 随凭据域迁 credential_handover（经该模块引用）。
use error_mapping::{fixture_path_command_error, work_cn_location_command_error};
// P12-R3：OAuth 临时档案周期清理（仅 Windows 有隔离档案）随域下沉后的转发导入。
#[cfg(windows)]
use oauth_login::cleanup_expired_oauth_profiles;
use operation_contract::CommandErrorDto;

// P12-R1b：LibraryContext 值对象与库环境稳定标识已下沉 domain；
// P12-R3：库作用域解析与实例操作扩展 trait 随库域迁 library_archive，
// 组合根不再直接引用 LibraryContext。

/// 已验证 Work CN 版本的 SQLCipher 技术参数；不属于登录凭证或目录库密钥。
const WORK_CN_SOURCE_RAW_KEY: &str =
    "3605f6691095a993f03d5009c918352ef5be31ae31e8f000212b81ff058da773";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RuntimeMode {
    Fixture,
    RealReadPreview,
}

/// 安装包能力清单。它与 RuntimeMode 分离，避免把只读资格误当成本机凭证写入资格。
/// 0.2.1 当前候选明确启用 LocalCredentialSwitch；测试可以构造关闭清单验证边界。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CapabilityManifest {
    local_credential_switch: bool,
}

impl CapabilityManifest {
    const fn installed_0_2_1() -> Self {
        Self {
            local_credential_switch: true,
        }
    }
}

/// 扫描授权槽：服务端生成代次，确保晚完成的旧 grant/revoke 不能覆盖新意图。
struct AuthorizationSlot {
    generation: u64,
    state: AuthorizationState,
    /// 真实只读授权成功后才记录的固定数据位置；启动阶段不提前发现真实 TRAE。
    real_location: Option<WorkCnReadLocation>,
    /// 真实只读授权建立时捕获的位置见证；只允许在每次读取前复核。
    real_location_witness: Option<traesync_infrastructure::LocationWitness>,
    /// 真实只读授权建立时捕获的账号身份不可逆指纹，不保存原始 user_id。
    real_user_fingerprint: Option<String>,
    /// 真实只读授权建立时捕获的账号认证指纹，不保存原始 user_id。
    real_auth_fingerprint: Option<String>,
}

/// 后端缓存的计划必须绑定生成它时的授权代次，避免撤销后重授权到同一位置时复用旧计划。
#[derive(Debug, Clone)]
struct PendingSyncPlan {
    generation: u64,
    plan: SyncPlan,
}

/// 账号中心的瞬时观测只在对应授权代次仍有效时对外可见。
#[derive(Default)]
struct ManagedAccountRuntimeSlot {
    runtime: ManagedAccountRuntime,
    authorization_generation: Option<u64>,
}

impl AuthorizationSlot {
    fn new(state: AuthorizationState) -> Self {
        Self {
            generation: 0,
            state,
            real_location: None,
            real_location_witness: None,
            real_user_fingerprint: None,
            real_auth_fingerprint: None,
        }
    }

    /// 撤销属于新的用户意图：递增代次，使所有更早的 pending grant 失效。
    fn revoke(&mut self) {
        self.generation = self.generation.checked_add(1).expect("扫描授权代次溢出");
        self.state = AuthorizationState::NotAuthorized;
        self.real_location = None;
        self.real_location_witness = None;
        self.real_user_fingerprint = None;
        self.real_auth_fingerprint = None;
    }

    /// 仅当失效结果仍属于同一授权代次时撤销，避免旧扫描结果误伤新授权。
    fn revoke_if_matches(&mut self, generation: u64, expected: &AuthorizationState) -> bool {
        if self.generation != generation || self.state != *expected {
            return false;
        }
        self.revoke();
        true
    }
}

/// 应用共享状态：在 Tauri command 之间共享的依赖。
///
/// T02 新增 workbench_probe / workbench_reader / source_raw_key——这些在 fixture 模式下
/// 用于构造 `WorkbenchReadService`。源密钥从 env 读取，不进入 UI/日志。
///
/// T03 新增 storage_root——从 env `TRAE_SYNC_FIXTURE_STORAGE_ROOT` 读取，
/// 作为快照发布与目录库存储根。各命令按需构造 service，不在 AppState 持有连接。
#[derive(Clone)]
struct AppState {
    runtime_mode: RuntimeMode,
    /// 启动时冻结的安装能力，不接受前端或环境变量在运行中提升。
    capabilities: CapabilityManifest,
    /// 生产目录库运行材料只由启动组合根创建，不接受命令参数注入。
    production_catalog: Option<ProductionCatalogRuntime>,
    provider: Arc<dyn WorkspaceStateProvider>,
    /// T02 嵌入式 SQLCipher 探测器（空 struct，无状态）
    workbench_probe: SqlCipherProbe,
    /// T02 账号证据读取器（空 struct，无状态）
    workbench_reader: AccountEvidenceReader,
    /// TRAE 源数据库 SQLCipher raw key；生产模式使用已验证技术基线。
    /// 空字符串表示未配置——命令返回错误，不泄露 key 状态。
    source_raw_key: String,
    /// 存储根路径，从 env `TRAE_SYNC_FIXTURE_STORAGE_ROOT` 读取。
    /// 快照发布到 `<storage_root>/snapshots/`，目录库由 `catalog/current.json` 选定。
    /// 空字符串表示未配置——扫描命令返回错误。
    storage_root: String,
    /// T08/T13 固定恢复区；未配置时所有需要恢复区的操作失败关闭。
    recovery_root: String,
    /// T08 后端进程观测器；前端传入的 process_state 不能替代它。
    process_controller: Arc<dyn ProcessControllerPort>,
    /// R1：扫描授权状态——由后端持有，不接受前端注入。
    /// 用户通过 `grant_scan_authorization` 显式授权后设为 Authorized，
    /// 默认 NotAuthorized。scan_history 在任何 FS/DB 访问前检查此状态。
    authorization: Arc<Mutex<AuthorizationSlot>>,
    /// T06：仅缓存后端生成的不可变计划，客户端不能反序列化或注入计划。
    pending_sync_plan: Arc<Mutex<Option<PendingSyncPlan>>>,
    /// T06：当前写操作的取消令牌；进入目标写入后执行器自行忽略取消。
    active_sync_cancellation: Arc<Mutex<Option<OperationCancellation>>>,
    /// T12：删除计划只保存在后端，前端不能回传伪造计划执行。
    pending_deletion_plan: Arc<Mutex<Option<StorageDeletionPlan>>>,
    /// P1：账号切换只保存非敏感元数据；真实凭证交接仍由外部账号管理器完成。
    managed_account_runtime: Arc<Mutex<ManagedAccountRuntimeSlot>>,
    /// 签到批量任务取消标记；只阻止尚未开始的 fixture 任务。
    checkin_cancellation: Arc<AtomicBool>,
    /// OAuth 登录进行中的会话（PKCE verifier + 回调监听 socket 只在后端内存）；
    /// begin 产出、complete 一次性消费，重复 begin 覆盖旧会话。
    pending_login: Arc<Mutex<Option<LoginSession>>>,
    /// P6-3：当次隔离登录的临时浏览器档案目录（begin 记录、complete 消费后清理）；
    /// 登录进行中不清理（浏览器可能正占用），失败静默留待启动兜底。
    active_oauth_profile: Arc<Mutex<Option<std::path::PathBuf>>>,
    /// P7-4：当次登录的取消标记；cancel 命令置位，complete 的等待循环
    /// 周期检查（同一时间只有一个登录会话，begin 时复位）。
    login_cancel: Arc<AtomicBool>,
    /// P7-4：当次隔离登录的浏览器子进程（仅隔离模式持有，用于「浏览器已
    /// 关闭 → 终止等待」的进程退出检测与取消时结束进程）。本机浏览器模式
    /// 下进程可能委托给已运行实例后立即退出，退出不等于放弃登录，故不持有。
    active_login_browser: Arc<Mutex<Option<std::process::Child>>>,
    /// 签到执行互斥：手动批次整批持有（阻塞等待，最多等自动单账号几秒）；
    /// 自动批次逐账号短持有（try_lock 失败即跳过该账号，grill 2026-08-23 决策 5）。
    checkin_execution_lock: Arc<Mutex<()>>,
}

/// 真实只读绑定复核：位置见证 + 账号指纹 + 认证指纹三者全部与授权代次一致才算“当前”。
///
/// 读取主位置账号证据用于绑定；如果真实位置返回失败，按 fail-closed 处理（返回 false）
/// 让调用方触发授权失效流程。
fn real_read_binding_is_current<W: ?Sized>(
    state: &AppState,
    expected_witness: &W,
    expected_user_fingerprint: &str,
    expected_auth_fingerprint: &str,
) -> bool
where
    for<'a> &'a W: Into<LocationWitnessRef<'a>>,
{
    use traesync_domain::EvidenceState;
    // 1) 位置见证比对：调用方把两种类型（WorkCnReadLocation / LocationWitness）都传进来，
    //    这里统一降成“根+db相对路径+可选身份”的引用视图后再对比真实位置。
    let witness_ref: LocationWitnessRef = expected_witness.into();
    let actual_witness = capture_location_witness(
        &PlatformFileIdentityProvider::new(),
        Path::new(witness_ref.canonical_root()),
        witness_ref.db_relative_path(),
    );
    let location_ok = match (&actual_witness, witness_ref.identity()) {
        (Ok(actual), Some(expected_id)) => {
            actual.data_location_id == expected_id
                && compare_location_identity(
                    &traesync_infrastructure::LocationWitness {
                        data_location_id: expected_id.to_string(),
                        canonical_root: witness_ref.canonical_root().to_string(),
                        db_relative_path: witness_ref.db_relative_path().to_string(),
                        root_identity: None,
                        db_identity: None,
                        wal_identity: None,
                        shm_identity: None,
                        db_sha256: None,
                        wal_sha256: None,
                        shm_sha256: None,
                    },
                    actual,
                )
                .is_empty()
        }
        (Ok(actual), None) => {
            actual.canonical_root == witness_ref.canonical_root()
                && actual.db_relative_path == witness_ref.db_relative_path()
        }
        (Err(_), _) => false,
    };
    if !location_ok {
        return false;
    }
    // 2) 账号证据：读取当前真实账号指纹与认证指纹做比对。
    let account = state.workbench_reader.read_account_evidence(
        Path::new(witness_ref.canonical_root()),
        std::time::SystemTime::now(),
    );
    if account.evidence_state != EvidenceState::Verified {
        return false;
    }
    let actual_user = account
        .user_id
        .as_ref()
        .map(|uid| user_id_binding_fingerprint(uid));
    let actual_auth = account.auth_fingerprint.as_ref().map(|af| af.0.as_str());
    actual_user.as_deref() == Some(expected_user_fingerprint)
        && actual_auth == Some(expected_auth_fingerprint)
}

/// 统一抽象：让 LocationWitness / WorkCnReadLocation 都能以“见证视图”入参。
enum LocationWitnessRef<'a> {
    FromWitness {
        canonical_root: &'a str,
        db_relative_path: &'a str,
        identity: Option<&'a str>,
    },
}

impl<'a> LocationWitnessRef<'a> {
    fn canonical_root(&self) -> &str {
        match self {
            LocationWitnessRef::FromWitness { canonical_root, .. } => canonical_root,
        }
    }
    fn db_relative_path(&self) -> &str {
        match self {
            LocationWitnessRef::FromWitness {
                db_relative_path, ..
            } => db_relative_path,
        }
    }
    fn identity(&self) -> Option<&str> {
        match self {
            LocationWitnessRef::FromWitness { identity, .. } => *identity,
        }
    }
}

impl<'a> From<&'a traesync_infrastructure::LocationWitness> for LocationWitnessRef<'a> {
    fn from(value: &'a traesync_infrastructure::LocationWitness) -> Self {
        LocationWitnessRef::FromWitness {
            canonical_root: &value.canonical_root,
            db_relative_path: &value.db_relative_path,
            identity: Some(&value.data_location_id),
        }
    }
}

impl<'a> From<&'a WorkCnReadLocation> for LocationWitnessRef<'a> {
    fn from(value: &'a WorkCnReadLocation) -> Self {
        LocationWitnessRef::FromWitness {
            canonical_root: value.canonical_root().to_str().unwrap_or(""),
            db_relative_path: value.db_relative_path().to_str().unwrap_or(""),
            identity: None,
        }
    }
}

fn ensure_fixture_runtime(state: &AppState) -> Result<(), String> {
    if state.runtime_mode != RuntimeMode::Fixture {
        return Err("生产只读模式不接受 fixture 路径命令".to_string());
    }
    Ok(())
}

/// P12-R2a 真实模式门禁（TRAE 实例/主库域）：仅 RealReadPreview 可通过。
/// fixture 模式拒绝并返回稳定错误码 `trae_real_mode_required`（前端/e2e 契约，勿改）。
/// 收敛自 41 处内联复制；登录域与签到 HTTP 域使用各自的域门禁函数。
fn ensure_real_preview(state: &AppState) -> Result<(), String> {
    if state.runtime_mode != RuntimeMode::RealReadPreview {
        return Err("trae_real_mode_required".to_string());
    }
    Ok(())
}

/// P12-R2a 真实模式门禁（登录域）：仅 RealReadPreview 可通过。
/// fixture 模式拒绝并返回稳定错误码 `login_real_mode_required`（前端/e2e 契约，勿改）。
fn ensure_login_real_mode(state: &AppState) -> Result<(), String> {
    if state.runtime_mode != RuntimeMode::RealReadPreview {
        return Err("login_real_mode_required".to_string());
    }
    Ok(())
}

/// P12-R2a 真实模式门禁（签到 HTTP 域）：仅 RealReadPreview 可通过。
/// fixture 模式拒绝并返回稳定错误码 `checkin_http_disabled`（前端/e2e 契约，勿改）。
fn ensure_checkin_http_enabled(state: &AppState) -> Result<(), String> {
    if state.runtime_mode != RuntimeMode::RealReadPreview {
        return Err("checkin_http_disabled".to_string());
    }
    Ok(())
}

/// P12-R2a 命令壳统一阻塞包装：把闭包调度到阻塞线程池并等待完成。
/// join 失败（任务 panic 或调度器关闭）映射为调用方给定的稳定
/// `{域}_join_failed` 错误码；闭包自身的 `Result` 原样透传。
/// 收敛自 spawn_blocking + `.await` + `map_err(|_| join 错误码)` 的重复样板；
/// 形态特殊（发射后不管、无 join 处理）的调用点不强行迁移。
async fn run_blocking<T, F>(join_failed_code: &str, f: F) -> Result<T, String>
where
    F: FnOnce() -> Result<T, String> + Send + 'static,
    T: Send + 'static,
{
    tauri::async_runtime::spawn_blocking(f)
        .await
        .map_err(|_| join_failed_code.to_string())
        .and_then(|inner| inner)
}

/// 捕获当前读取授权的代次与范围，供一次读取操作绑定使用。
///
/// 代次必须和范围一起捕获；只比较路径会让“撤销后重新授权到同一位置”的旧操作
/// 误把新授权当成自己的上下文。
fn capture_authorized_read_context(state: &AppState) -> Result<(u64, AuthorizationState), String> {
    let slot = state.authorization.lock().unwrap();
    if matches!(slot.state, AuthorizationState::NotAuthorized) {
        return Err(HistoryCommandError::NotAuthorized.to_string());
    }
    Ok((slot.generation, slot.state.clone()))
}

/// 判断一次读取操作是否仍属于它开始时的授权代次与授权范围。
fn authorization_context_is_current(
    state: &AppState,
    generation: u64,
    expected: &AuthorizationState,
) -> bool {
    let slot = state.authorization.lock().unwrap();
    slot.generation == generation && slot.state == *expected
}

/// 在读取结果返回或计划缓存前再次确认授权上下文没有漂移。
fn ensure_authorization_context_current(
    state: &AppState,
    generation: u64,
    expected: &AuthorizationState,
) -> Result<(), String> {
    if authorization_context_is_current(state, generation, expected) {
        Ok(())
    } else {
        Err("authorization_mismatch".to_string())
    }
}

#[derive(Debug, Clone, Serialize)]
struct StorageRootStateDto {
    configured: bool,
    available: bool,
    storage_root_id: Option<String>,
    canonical_path: Option<String>,
    catalog_id: Option<String>,
    write_enabled: bool,
    used_bytes: Option<u64>,
    warning_threshold_bytes: u64,
    warning_active: bool,
    automatic_scan_paused: bool,
    reason: Option<String>,
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
    let guard =
        FixturePathGuard::new(Path::new(&fixture_root)).map_err(fixture_path_command_error)?;
    let db_path = guard
        .validate_db_relative_path(&db_relative_path)
        .map_err(fixture_path_command_error)?;
    Ok((guard, db_path))
}

/// 解析当前运行模式下已授权的只读目标。
///
/// 生产模式在确认授权前不发现或访问真实位置，且每次重新发现必须与启动位置一致。
fn resolve_authorized_read_target(state: &AppState) -> Result<(PathBuf, PathBuf, PathBuf), String> {
    match state.runtime_mode {
        RuntimeMode::Fixture => {
            let (guard, db_path) = resolve_authorized_fixture(state)?;
            let storage_root = guard
                .validate_fixture_storage_root(Path::new(&state.storage_root))
                .map_err(fixture_path_command_error)?;
            Ok((guard.canonical_root().to_path_buf(), db_path, storage_root))
        }
        RuntimeMode::RealReadPreview => {
            let (
                authorization,
                expected_location,
                expected_witness,
                expected_user_fingerprint,
                expected_auth_fingerprint,
            ) = {
                let slot = state.authorization.lock().unwrap();
                (
                    slot.state.clone(),
                    slot.real_location.clone(),
                    slot.real_location_witness.clone(),
                    slot.real_user_fingerprint.clone(),
                    slot.real_auth_fingerprint.clone(),
                )
            };
            let (authorized_root, authorized_db_path) = match authorization {
                AuthorizationState::NotAuthorized => {
                    return Err(HistoryCommandError::NotAuthorized.to_string())
                }
                AuthorizationState::Authorized {
                    canonical_fixture_root,
                    db_relative_path,
                } => (canonical_fixture_root, db_relative_path),
            };
            let expected_location =
                expected_location.ok_or_else(|| "authorization_mismatch".to_string())?;
            let current = WorkCnReadLocation::discover()
                .map_err(|error| work_cn_location_command_error(&error))?;
            if current != expected_location
                || current.canonical_root().to_string_lossy() != authorized_root
                || current.db_relative_path().to_string_lossy() != authorized_db_path
            {
                return Err(HistoryCommandError::AuthorizationMismatch.to_string());
            }
            let Some(expected_witness) = expected_witness else {
                return Err("authorization_mismatch".to_string());
            };
            let Some(expected_user_fingerprint) = expected_user_fingerprint.as_deref() else {
                return Err("authorization_mismatch".to_string());
            };
            let Some(expected_auth_fingerprint) = expected_auth_fingerprint.as_deref() else {
                return Err("authorization_mismatch".to_string());
            };
            if !real_read_binding_is_current(
                state,
                &expected_witness,
                expected_user_fingerprint,
                expected_auth_fingerprint,
            ) {
                let current_witness = current
                    .capture_platform_read_identity()
                    .map_err(|_| "data_location_changed".to_string())?;
                if !compare_location_identity(&expected_witness, &current_witness).is_empty() {
                    return Err("data_location_changed".to_string());
                }
                return Err("account_evidence_changed".to_string());
            }
            let runtime = state
                .production_catalog
                .as_ref()
                .ok_or_else(|| "生产目录库运行材料不可用".to_string())?;
            StorageRootBinding::open_existing(
                &runtime.storage_root,
                None,
                Some(&runtime.catalog_id),
            )
            .map_err(|error| error.to_string())?;
            Ok((
                current.canonical_root().to_path_buf(),
                current.canonical_db_path().to_path_buf(),
                runtime.storage_root.clone(),
            ))
        }
    }
}

/// 在一次读取操作中绑定授权代次，并在解析目标前后各检查一次。
/// 目标解析期间即使发生撤销或重授权，调用方也只能拿到最终复核通过的结果。
fn resolve_authorized_read_target_for_context(
    state: &AppState,
    generation: u64,
    authorization: &AuthorizationState,
) -> Result<(PathBuf, PathBuf, PathBuf), String> {
    ensure_authorization_context_current(state, generation, authorization)?;
    let target = resolve_authorized_read_target(state)?;
    ensure_authorization_context_current(state, generation, authorization)?;
    Ok(target)
}

fn recovery_root_path(state: &AppState) -> Result<&Path, String> {
    if state.recovery_root.is_empty() {
        Err("恢复区未配置".to_string())
    } else {
        Ok(Path::new(&state.recovery_root))
    }
}

/// `get_workspace_state` Tauri command。
/// T01 阶段返回固定的空工作台状态。
#[tauri::command]
fn get_workspace_state(state: tauri::State<AppState>) -> WorkspaceState {
    workspace_state_for_app(&state)
}

/// 组合启动状态与授权后的实时账号证据；未授权时绝不读取真实认证目录。
fn workspace_state_for_app(state: &AppState) -> WorkspaceState {
    let mut workspace = commands::get_workspace_state(state.provider.as_ref());
    if state.runtime_mode != RuntimeMode::RealReadPreview {
        return workspace;
    }

    let (authorization, location, witness, user_fingerprint, auth_fingerprint) = {
        let slot = state.authorization.lock().unwrap();
        (
            slot.state.clone(),
            slot.real_location.clone(),
            slot.real_location_witness.clone(),
            slot.real_user_fingerprint.clone(),
            slot.real_auth_fingerprint.clone(),
        )
    };
    let AuthorizationState::Authorized {
        canonical_fixture_root,
        db_relative_path,
    } = authorization
    else {
        return workspace;
    };
    let (Some(location), Some(witness), Some(user_fingerprint), Some(auth_fingerprint)) =
        (location, witness, user_fingerprint, auth_fingerprint)
    else {
        workspace.current_account.unavailable_reason = Some("authorization_mismatch".to_string());
        workspace.capabilities.scan_enabled = false;
        return workspace;
    };
    if location.canonical_root().to_string_lossy() != canonical_fixture_root
        || location.db_relative_path().to_string_lossy() != db_relative_path
        || !real_read_binding_is_current(state, &witness, &user_fingerprint, &auth_fingerprint)
    {
        workspace.current_account.unavailable_reason = Some("authorization_mismatch".to_string());
        workspace.capabilities.scan_enabled = false;
        return workspace;
    }

    workspace.data_location.selected = true;
    workspace.data_location.display_name =
        Some(location.canonical_root().to_string_lossy().into_owned());
    workspace.data_location.unavailable_reason = None;
    workspace.capabilities.scan_enabled = true;

    let account = state
        .workbench_reader
        .read_account_evidence(location.canonical_root(), std::time::SystemTime::now());
    workspace.current_account = current_account_state(&account);
    workspace
}

/// 把账号证据转换为不暴露原始 user_id 的标题栏状态。
fn current_account_state(account: &AccountEvidence) -> CurrentAccountState {
    let user_fingerprint = account.user_id.as_ref().map(user_id_display_fingerprint);
    CurrentAccountState {
        detected: user_fingerprint.is_some(),
        user_fingerprint,
        unavailable_reason: match account.evidence_state {
            EvidenceState::Verified => None,
            EvidenceState::SingleSource => Some("single_source".to_string()),
            EvidenceState::Missing => Some("missing".to_string()),
            EvidenceState::Conflict => Some("conflict".to_string()),
            EvidenceState::Expired => Some("expired".to_string()),
            EvidenceState::FingerprintChanged => Some("fingerprint_changed".to_string()),
        },
    }
}

/// OAuth 临时档案清理线程间隔（秒）：小于保留时长，
/// 超龄目录在一个间隔内必被回收。
const OAUTH_PROFILE_CLEANUP_TICK_SECONDS: u64 = 300;

/// OAuth 临时浏览器档案周期清理（P6-3，仅 Windows 有隔离档案）：
/// 启动先清一次上次遗留，之后每 tick 清理超龄目录；
/// 浏览器占用的删除失败由后续 tick 重试，线程随进程退出。
#[cfg(windows)]
fn spawn_oauth_profile_cleanup_scheduler() {
    std::thread::spawn(move || {
        cleanup_expired_oauth_profiles();
        loop {
            std::thread::sleep(std::time::Duration::from_secs(
                OAUTH_PROFILE_CLEANUP_TICK_SECONDS,
            ));
            cleanup_expired_oauth_profiles();
        }
    });
}

// ============================================================================
// M4：OAuth 登录命令（begin/complete 两段式）已迁 oauth_login 模块（P12-R3）。
// ============================================================================

// ADR-0031 远程设备命令与 DTO 已迁 remote_devices 模块（P12-R3 纯移动）；
// 产品账号路由按定案留在组合根。

/// 产品账号路由（list_remote_devices / clear_remote_device 共用，ADR-0031）：
/// work_cn → 主库登录态解析（指定 profile 或主库当前账号）+ OAuthClient::Solo；
/// trae_cn → 产品账号池当前指针（或指定 profile）+ OAuthClient::TraeCn；
/// 其他产品 → unknown_product。
fn resolve_remote_device_account(
    material_root: &Path,
    storage_root: &str,
    product_id: &str,
    profile_id: Option<&str>,
) -> Result<(AccountRecord, OAuthClient), String> {
    match product_id {
        // Work CN：默认账号走主库登录态实测解析（与环境页/插件 tab 同一路径）。
        "work_cn" => {
            let account = match profile_id {
                Some(profile) => {
                    let records = AccountRegistry::new(material_root)
                        .load()
                        .map_err(|_| "checkin_registry_invalid".to_string())?;
                    records
                        .into_iter()
                        .find(|record| record.profile_id == profile)
                        .ok_or_else(|| "account_not_found".to_string())?
                }
                None => {
                    let master_dir = master_data_dir()
                        .map_err(|_| "environment_registry_invalid".to_string())?;
                    resolve_actual_master_account(material_root, Path::new(storage_root), &master_dir)?
                        .ok_or_else(|| "current_account_unresolved".to_string())?
                }
            };
            Ok((account, OAuthClient::Solo))
        }
        // Trae CN：默认账号取产品账号池当前指针（switch 命令写入的同一状态）。
        // P12-R1b：指针→档案查找下沉 library_account（与库作用域同源），
        // 未选择/悬空两种空形态在此映射为各自的稳定错误码。
        "trae_cn" => {
            let account = match library_account::resolve_trae_cn_pointer(material_root, profile_id)?
            {
                TraeCnPointerLookup::Registered(account) => account,
                TraeCnPointerLookup::NoSelection => {
                    return Err("current_account_unresolved".to_string())
                }
                TraeCnPointerLookup::Dangling => return Err("account_not_found".to_string()),
            };
            Ok((account, OAuthClient::TraeCn))
        }
        _ => Err("unknown_product".to_string()),
    }
}

// P12-R3：login_receipt_wire_tests 三例随登录回执 DTO 迁至 oauth_login 模块。

/// P2-2 实例启动结果：launched=本次新启动；focused=已在运行、已把窗口带到前台；
/// seeded=本次新启动且播种了原生目录登录态（前端可提示首次免登录）。
/// login_state=启动时刻的实例登录态（launched 分支据此引导未登录账号登录一次）。
#[derive(Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) struct TraeInstanceLaunchDto {
    outcome: &'static str,
    login_state: trae_instance_module::InstanceLoginState,
}

/// 单账号登录凭据健康度（P7-5 凭据包实调判定）：
/// login_state = 凭据包 token 实调 GetUserInfo 的结果（与切号 E2 首选
/// 路径同源——徽章说有效则切号必通）：logged_in=实调成功 / stale=服务端
/// 拒绝或身份不一致 / uninitialized=无凭据包；断网时降级本地存档证据。
/// archive_available = 登录存档三件套登录键是否存在（E1 降级移植路径
/// 可用性，前端收进徽章悬浮提示的次要信息）。
#[derive(Clone, Serialize)]
struct TraeInstanceStateDto {
    profile_id: String,
    login_state: trae_instance_module::InstanceLoginState,
    archive_available: bool,
}

/// P7-5 单账号凭据包实调判定（纯决策逻辑，实调用 `live_probe` 注入，
/// 供单测离线覆盖三态 + 断网降级）：
/// - 凭据包缺失/损坏 → 未登录（uninitialized）；
/// - 实调成功且 user_id 与凭据包一致 → 登录有效（logged_in）；
/// - 实调成功但身份不一致 / 服务端拒绝（401、业务码、协议异常）→ 登录失效（stale）；
/// - 网络层失败（断网）→ 无法实调，降级存档本地证据（与切号 E2→E1 同哲学）。
///
/// `archive_state` 为调用方预读的存档登录态（instance_login_state），
/// 登录键存在（logged_in/stale）即视为 E1 降级路径可用。
fn credential_login_state(
    store: &CheckinCredentialStore,
    record: &traesync_infrastructure::account_registry::AccountRecord,
    archive_state: trae_instance_module::InstanceLoginState,
    live_probe: &dyn Fn(&str) -> Result<UserInfoFull, CheckinHttpError>,
) -> (trae_instance_module::InstanceLoginState, bool) {
    let archive_available = matches!(
        archive_state,
        trae_instance_module::InstanceLoginState::LoggedIn
            | trae_instance_module::InstanceLoginState::Stale
    );
    let binding = CheckinProfileBinding::from(record);
    let Ok(bundle) = store.load(&binding) else {
        // 无凭据包 = 未登录（存档提示仍上报，老账号可能可走 E1 切换）。
        return (
            trae_instance_module::InstanceLoginState::Uninitialized,
            archive_available,
        );
    };
    match live_probe(&bundle.access_token) {
        Ok(info) if info.user_id == bundle.account_id => (
            trae_instance_module::InstanceLoginState::LoggedIn,
            archive_available,
        ),
        // 身份不一致 = 凭据错位不可信，与实调被拒同判失效。
        Ok(_) => (
            trae_instance_module::InstanceLoginState::Stale,
            archive_available,
        ),
        Err(CheckinHttpError::Network) => (archive_state, archive_available),
        Err(_) => (
            trae_instance_module::InstanceLoginState::Stale,
            archive_available,
        ),
    }
}

/// 校验 profile_id 并从账号注册表取 account_id（启动实例需要它做登录态种子）。
pub(crate) fn trae_instance_account_id(
    material_root: &Path,
    profile_id: &str,
) -> Result<(String, String), String> {
    let registry = AccountRegistry::new(material_root);
    let records = registry
        .load()
        .map_err(|_| "checkin_registry_invalid".to_string())?;
    records
        .iter()
        .find(|record| record.profile_id == profile_id)
        // 返回 (account_id, 窗口标题)：标题优先本地备注名，缺省回退服务端昵称
        // （A2：多开实例窗口按账号名区分，2026-08-29 trae-mate 对标裁定）。
        .map(|record| {
            (
                record.account_id.clone(),
                record
                    .display_name
                    .clone()
                    .unwrap_or_else(|| record.screen_name.clone()),
            )
        })
        .ok_or_else(|| "trae_profile_invalid".to_string())
}

/// 实例启动公共链路（P5-0 抽出，主库与账号实例共用）：
/// 进程查询 → 已运行则聚焦返回 → exe 发现 →（可选登录态种子）→ 建目录
/// → 窗口标题写入 → spawn。
/// matcher 决定「已运行」的判定口径：账号实例用带参匹配
/// （`command_line_uses_data_dir`），主库（官方目录）额外覆盖官方
/// 快捷方式无参启动的主进程（`command_line_matches_master`）。
pub(crate) fn launch_instance_common(
    instance_dir: &Path,
    window_title: &str,
    seed_account_id: Option<&str>,
    matcher: fn(&str, &Path) -> bool,
) -> Result<TraeInstanceLaunchDto, String> {
    let processes = list_trae_processes().map_err(|e| e.code().to_string())?;

    // 已在运行：聚焦已有实例，不重复启动（同目录幂等）。
    let running_pids: Vec<u32> = processes
        .iter()
        .filter(|process| {
            process
                .command_line
                .as_deref()
                .is_some_and(|line| matcher(line, instance_dir))
        })
        .map(|process| process.pid)
        .collect();
    if !running_pids.is_empty() {
        focus_instance_windows(&running_pids);
        return Ok(TraeInstanceLaunchDto {
            outcome: "focused",
            login_state: trae_instance_module::instance_login_state(instance_dir),
        });
    }

    let exe = discover_trae_executable(&processes).map_err(|e| e.code().to_string())?;
    // 登录态种子仅账号实例使用；主库不播种（Q2 新建空主库：首启进 TRAE
    // 登录页登录一次，此后登录态由切号流程的凭据互换维护，P5-1）。
    let seed = seed_account_id
        .map(|account_id| seed_login_state(instance_dir, account_id))
        .transpose()
        .map_err(|e: trae_instance_module::TraeInstanceError| e.code().to_string())?;
    std::fs::create_dir_all(instance_dir)
        .map_err(|_| "trae_instance_dir_unavailable".to_string())?;
    // A2 窗口标题：启动前合并写入 User/settings.json 的 window.title
    // （--title CLI 对 TRAE 无效，settings.json 是实测可靠机制）。标题是
    // 便利功能，写入失败不阻断启动（trae-mate 同款处置）。
    let _ = trae_instance_module::write_window_title(instance_dir, window_title);
    // 参数交给 std 的 Windows 引用规则：路径含空格时自动整体加引号，
    // Chromium 系（TRAE 为 Electron）两种形态均可解析。
    //
    // stdio 重定向：TRAE(Electron) 的 Chromium 内部日志（ERROR/WARNING 白字）不再
    // 经继承的 stderr 刷入 dev 终端，而是追加到实例目录下的运行日志，排查 TRAE
    // 崩溃时仍有现场可查；同时把 cwd 指向实例目录，TRAE 的 debug.log 不再落到
    // src-tauri。日志文件打开失败（极端磁盘故障）时退回继承 stdio，不阻塞启动。
    let runtime_log = instance_dir.join("trae-runtime.log");
    let stdout_handle = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&runtime_log)
        .ok()
        .map(std::process::Stdio::from);
    let stderr_handle = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&runtime_log)
        .ok()
        .map(std::process::Stdio::from);
    let mut command = std::process::Command::new(&exe);
    command
        .arg(format!("--user-data-dir={}", instance_dir.display()))
        .current_dir(instance_dir);
    if let Some(handle) = stdout_handle {
        command.stdout(handle);
    }
    if let Some(handle) = stderr_handle {
        command.stderr(handle);
    }
    command
        .spawn()
        .map_err(|_| "trae_launch_failed".to_string())?;
    Ok(TraeInstanceLaunchDto {
        outcome: match seed {
            Some(trae_instance_module::SeedOutcome::Seeded) => "seeded",
            _ => "launched",
        },
        // seed 之后判定：播种成功即已登录；NativeMissing/空壳目录即待登录引导。
        login_state: trae_instance_module::instance_login_state(instance_dir),
    })
}

// P12-R3：主库实例启动/状态/当前账号登记、多环境档案与生命周期命令、
// 副环境登录事务与主库当前账号实测纠正（reconcile）已随环境域迁至
// environment 模块（纯移动，命令注册改路径形式，invoke 名不变）。

/// 解析需要数据归属或账号凭据的主库当前账号（P5-5 语义，P11-5⑤ 起为
/// 实测分支的主库特化入口）。P12-R1b：实测分支下沉
/// `library_account::resolve_measured_env_account`，此处保留薄转发——
/// 调用点（插件预检/凭据/自检/远程设备）只持有主库目录，不持有
/// LibraryContext，语义与错误码口径保持不变。
fn resolve_actual_master_account(
    material_root: &Path,
    storage_root: &Path,
    master_instance_dir: &Path,
) -> Result<Option<traesync_infrastructure::account_registry::AccountRecord>, String> {
    library_account::resolve_measured_env_account(
        material_root,
        storage_root,
        master_instance_dir,
        MASTER_ENV_ID,
    )
}

/// 需要执行主库归属操作时的强制账号解析；未确认实测账号即拒绝操作。
/// P11-5⑤：库泛化入口（主库/副环境同义，env_id 由 LibraryContext 提供）。
/// P12-R1b：转发下沉后的实测分支。
fn require_actual_env_account(
    material_root: &Path,
    storage_root: &Path,
    instance_dir: &Path,
    env_id: &str,
) -> Result<traesync_infrastructure::account_registry::AccountRecord, String> {
    library_account::resolve_measured_env_account(material_root, storage_root, instance_dir, env_id)?
        .ok_or_else(|| "master_current_account_unavailable".to_string())
}

fn require_actual_master_account(
    material_root: &Path,
    storage_root: &Path,
    master_instance_dir: &Path,
) -> Result<traesync_infrastructure::account_registry::AccountRecord, String> {
    require_actual_env_account(
        material_root,
        storage_root,
        master_instance_dir,
        MASTER_ENV_ID,
    )
}

/// 批量查询账号登录凭据健康度（P7-5 凭据包实调判定）：
/// 逐账号并发实调 GetUserInfo（不后台轮询，仅账号页打开/手动刷新触发），
/// 断网降级本地存档证据；E1 存档可用性作为悬浮提示次要信息随行返回。
#[tauri::command]
async fn get_trae_instance_states(
    profile_ids: Vec<String>,
    state: tauri::State<'_, AppState>,
) -> Result<Vec<TraeInstanceStateDto>, String> {
    if state.runtime_mode != RuntimeMode::RealReadPreview {
        return Ok(profile_ids
            .into_iter()
            .map(|profile_id| TraeInstanceStateDto {
                profile_id,
                login_state: trae_instance_module::InstanceLoginState::Uninitialized,
                archive_available: false,
            })
            .collect());
    }
    let material_root = checkin_material_root(&state)?;
    let storage_root = PathBuf::from(&state.storage_root);
    run_blocking("trae_states_join_failed", move || {
        get_trae_instance_states_inner(&material_root, &storage_root, &profile_ids)
    })
    .await
}

fn get_trae_instance_states_inner(
    material_root: &Path,
    storage_root: &Path,
    profile_ids: &[String],
) -> Result<Vec<TraeInstanceStateDto>, String> {
    let registry = AccountRegistry::new(material_root);
    let records = registry
        .load()
        .map_err(|_| "checkin_registry_invalid".to_string())?;
    let store = CheckinCredentialStore::new(material_root);
    // 逐账号并发实调（scoped threads，HTTP 直连互不阻塞；账号量级个位数）。
    // 线程 panic 按「未登录」占位收尾，不让单账号异常拖垮整页徽章。
    let results = std::thread::scope(|scope| {
        let handles: Vec<(
            String,
            std::thread::ScopedJoinHandle<'_, TraeInstanceStateDto>,
        )> = profile_ids
            .iter()
            .map(|profile_id| {
                // 共享引用先行：move 闭包按引用捕获 records/store/storage_root，
                // 避免 FnMut 迭代时整体移出（E0507）。
                let records = &records;
                let store = &store;
                let storage_root = &*storage_root;
                let handle = scope.spawn(move || {
                    // DTO 需要所有权：&String 克隆为 String。
                    let profile_id = profile_id.clone();
                    let Some(record) = records
                        .iter()
                        .find(|record| record.profile_id == profile_id)
                    else {
                        return TraeInstanceStateDto {
                            profile_id,
                            login_state: trae_instance_module::InstanceLoginState::Uninitialized,
                            archive_available: false,
                        };
                    };
                    // 存档证据预读（断网降级判定 + E1 可用性）。
                    let archive_state = instance_data_dir(storage_root, &profile_id)
                        .map(|instance_dir| {
                            trae_instance_module::instance_login_state(&instance_dir)
                        })
                        .unwrap_or(trae_instance_module::InstanceLoginState::Uninitialized);
                    let live_probe = |token: &str| get_user_info_full(&trae_http_client(), token);
                    let (login_state, archive_available) =
                        credential_login_state(&store, record, archive_state, &live_probe);
                    TraeInstanceStateDto {
                        profile_id,
                        login_state,
                        archive_available,
                    }
                });
                // 元组侧单独克隆：闭包内的克隆已随 DTO 移走。
                (profile_id.clone(), handle)
            })
            .collect();
        handles
            .into_iter()
            .map(|(profile_id, handle)| {
                handle.join().unwrap_or(TraeInstanceStateDto {
                    profile_id,
                    login_state: trae_instance_module::InstanceLoginState::Uninitialized,
                    archive_available: false,
                })
            })
            .collect()
    });
    Ok(results)
}

// ===== P5-8b 插件 tab（ADR-0023 环境插件清单）：状态/市场/装/卸/吸收 =====

/// 云端已装条目 DTO（含清单归属标记，供前端差异提示）。
#[derive(Clone, Serialize)]
struct InstalledPluginDto {
    /// 不透明记录 ID（卸载键）。
    record_id: String,
    marketplace_plugin_id: Option<String>,
    name: String,
    display_name: String,
    version: String,
    registry: String,
    /// builtin 条目 = 客户端内置，不可卸载/同步。
    builtin: bool,
    /// 条目（按市场 UUID）是否已在环境清单内。
    in_manifest: bool,
}

/// 清单条目 DTO（含云端在装标记，供「清单待应用/已移除」提示）。
#[derive(Clone, Serialize)]
struct ManifestPluginDto {
    marketplace_plugin_id: String,
    name: String,
    display_name: String,
    version: String,
    registry: String,
    /// 清单条目在当前账号云端是否已装。
    installed_in_cloud: bool,
}

/// 插件 tab 状态（get_plugin_tab_state 返回）。
#[derive(Clone, Serialize)]
struct PluginTabStateDto {
    installed: Vec<InstalledPluginDto>,
    manifest: Vec<ManifestPluginDto>,
    /// 工具已知账号数（含当前账号；ADR-0026 决策 2 卸载确认文案
    /// 「将同时从 N 个账号移除」的 N）。
    known_account_count: usize,
}

/// 插件 tab 当前账号凭据解析：主库登录态实测 user_id → 账号注册表
/// → 凭据包 access_token。任一环节缺失返回稳定错误码（前端据此降级文案）。
fn plugin_tab_account_token(material_root: &Path, storage_root: &Path) -> Result<String, String> {
    let master_dir = master_data_dir().map_err(|_| "environment_registry_invalid".to_string())?;
    let account = resolve_actual_master_account(material_root, storage_root, &master_dir)?
        .ok_or_else(|| "plugin_tab_no_account".to_string())?;
    CheckinCredentialStore::new(material_root)
        .load(&CheckinProfileBinding::from(&account))
        .map(|bundle| bundle.access_token)
        .map_err(|_| "plugin_tab_credential_unavailable".to_string())
}

/// 从云端已装列表构造清单条目（首次导入/吸收共用，ADR-0023 决策 1/2）：
/// 只收非 builtin 且有 marketplace_plugin_id 的市场插件；空字段给回退值
/// （清单 validate 拒绝空串）。
fn manifest_entries_from_cloud(items: &[CloudPluginItem]) -> Vec<PluginManifestEntry> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0);
    items
        .iter()
        .filter(|item| !item.builtin)
        .filter_map(|item| {
            let id = item.marketplace_plugin_id.clone()?;
            Some(PluginManifestEntry {
                name: if item.name.is_empty() {
                    id.clone()
                } else {
                    item.name.clone()
                },
                display_name: if item.display_name.is_empty() {
                    item.name.clone()
                } else {
                    item.display_name.clone()
                },
                version: if item.version.is_empty() {
                    "0.0.0".into()
                } else {
                    item.version.clone()
                },
                registry: if item.registry.is_empty() {
                    "unknown".into()
                } else {
                    item.registry.clone()
                },
                marketplace_plugin_id: id,
                added_unix_seconds: now,
            })
        })
        .collect()
}

/// 读取插件 tab 状态：云端已装 + 环境清单 + 双向归属标记。
/// 清单文件不存在 = 首次启用，从当前账号云端导入后落盘（ADR-0023 决策 1）。
#[tauri::command]
async fn get_plugin_tab_state(
    state: tauri::State<'_, AppState>,
) -> Result<PluginTabStateDto, String> {
    ensure_real_preview(&state)?;
    let material_root = checkin_material_root(&state)?;
    let storage_root = PathBuf::from(&state.storage_root);
    run_blocking("plugin_tab_state_join_failed", move || {
        let token = plugin_tab_account_token(&material_root, &storage_root)?;
        let installed = fetch_installed_plugins(&token)
            .map_err(|_| "plugin_tab_cloud_unavailable".to_string())?;
        let manifest = PluginManifest::new(&storage_root);
        let entries = match manifest
            .load()
            .map_err(|_| "plugin_manifest_invalid".to_string())?
        {
            Some(entries) => entries,
            None => {
                // 首次启用：当前账号云端现状即基线。
                let imported = manifest_entries_from_cloud(&installed);
                manifest
                    .save(&imported)
                    .map_err(|_| "plugin_manifest_write_failed".to_string())?;
                imported
            }
        };
        let manifest_ids: std::collections::HashSet<&str> = entries
            .iter()
            .map(|entry| entry.marketplace_plugin_id.as_str())
            .collect();
        let cloud_ids: std::collections::HashSet<&str> = installed
            .iter()
            .filter_map(|item| item.marketplace_plugin_id.as_deref())
            .collect();
        let installed_dto = installed
            .iter()
            .map(|item| InstalledPluginDto {
                record_id: item.record_id.clone(),
                marketplace_plugin_id: item.marketplace_plugin_id.clone(),
                name: item.name.clone(),
                display_name: item.display_name.clone(),
                version: item.version.clone(),
                registry: item.registry.clone(),
                builtin: item.builtin,
                in_manifest: item
                    .marketplace_plugin_id
                    .as_deref()
                    .is_some_and(|id| manifest_ids.contains(id)),
            })
            .collect();
        let manifest_dto = entries
            .iter()
            .map(|entry| ManifestPluginDto {
                marketplace_plugin_id: entry.marketplace_plugin_id.clone(),
                name: entry.name.clone(),
                display_name: entry.display_name.clone(),
                version: entry.version.clone(),
                registry: entry.registry.clone(),
                installed_in_cloud: cloud_ids.contains(entry.marketplace_plugin_id.as_str()),
            })
            .collect();
        // 已知账号数：账号注册表全量（含归档账号——凭据仍在，云端可能
        // 装着该插件；卸载传播同样遍历全量，口径一致）。
        let known_account_count = AccountRegistry::new(&material_root)
            .load()
            .map(|records| records.len())
            .unwrap_or(0);
        Ok(PluginTabStateDto {
            installed: installed_dto,
            manifest: manifest_dto,
            known_account_count,
        })
    })
    .await
}

/// 浏览插件市场目录（GET /extensions/api/-/plugin/list，只读）。
#[tauri::command]
async fn browse_plugin_market(
    state: tauri::State<'_, AppState>,
) -> Result<Vec<MarketPluginItem>, String> {
    ensure_real_preview(&state)?;
    let material_root = checkin_material_root(&state)?;
    let storage_root = PathBuf::from(&state.storage_root);
    run_blocking("plugin_market_join_failed", move || {
        let token = plugin_tab_account_token(&material_root, &storage_root)?;
        fetch_market_plugins(&token).map_err(|_| "plugin_market_unavailable".to_string())
    })
    .await
}

/// 安装市场插件到当前账号云端（ADR-0023 决策 3：即时改云端 + 更新清单）。
/// 入参为市场条目字段（plugin_id = 市场 UUID）。
#[tauri::command]
async fn install_plugin(
    state: tauri::State<'_, AppState>,
    plugin_id: String,
    name: String,
    display_name: String,
    registry: String,
) -> Result<(), String> {
    ensure_real_preview(&state)?;
    let material_root = checkin_material_root(&state)?;
    let storage_root = PathBuf::from(&state.storage_root);
    run_blocking("plugin_install_join_failed", move || {
        let token = plugin_tab_account_token(&material_root, &storage_root)?;
        let plugin = MarketPluginItem {
            plugin_id,
            name,
            display_name,
            description: String::new(),
            registry,
            // 分类仅用于市场目录展示；安装流程不消费，构造占位值。
            categories: Vec::new(),
            category_key: String::new(),
            category_name: String::new(),
        };
        let installed_ok = install_market_plugin(&token, &plugin)
            .map_err(|_| "plugin_install_failed".to_string())?;
        if !installed_ok {
            return Err("plugin_install_failed".to_string());
        }
        // 清单更新：优先取安装后的云端权威数据（版本等字段准确）；
        // 列表滞后时回退市场条目字段（版本未知记 0.0.0）。
        let mut entry = PluginManifestEntry {
            name: if plugin.name.is_empty() {
                plugin.plugin_id.clone()
            } else {
                plugin.name.clone()
            },
            display_name: if plugin.display_name.is_empty() {
                plugin.name.clone()
            } else {
                plugin.display_name.clone()
            },
            version: "0.0.0".to_string(),
            registry: if plugin.registry.is_empty() {
                "unknown".to_string()
            } else {
                plugin.registry.clone()
            },
            marketplace_plugin_id: plugin.plugin_id.clone(),
            added_unix_seconds: 0,
        };
        if let Ok(items) = fetch_installed_plugins(&token) {
            if let Some(item) = items
                .iter()
                .find(|item| item.marketplace_plugin_id.as_deref() == Some(&plugin.plugin_id))
            {
                entry.version = item.version.clone();
                entry.registry = item.registry.clone();
            }
        }
        entry.added_unix_seconds = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_secs())
            .unwrap_or(0);
        let manifest = PluginManifest::new(&storage_root);
        let mut entries = manifest
            .load()
            .map_err(|_| "plugin_manifest_invalid".to_string())?
            .unwrap_or_default();
        entries.retain(|existing| existing.marketplace_plugin_id != entry.marketplace_plugin_id);
        entries.push(entry);
        manifest
            .save(&entries)
            .map_err(|_| "plugin_manifest_write_failed".to_string())
    })
    .await
}

/// 卸载全账号传播的逐账号回执（ADR-0026 决策 2）。
#[derive(Clone, Serialize)]
struct PluginUninstallAccountDto {
    /// 账号展示名（备注名优先，回退服务端昵称；不含内部 ID）。
    display_name: String,
    /// true = 已从云端移除；false = 无需移除（该账号没装）。
    removed: bool,
    /// 移除失败 true（fail-soft：只记失败不中断其他账号）。
    failed: bool,
    /// 失败原因码（成功时 None；技术细节收悬浮提示，不进主视野）。
    error_code: Option<String>,
}

/// uninstall_plugin_everywhere 返回：逐账号成败回执。
#[derive(Clone, Serialize)]
struct PluginUninstallEverywhereDto {
    accounts: Vec<PluginUninstallAccountDto>,
}

/// 卸载插件并传播到全部已知账号（ADR-0026 决策 2，前端已单次确认）：
/// 当前账号云端卸载（失败即整单报错，不传播）→ 清单移除 → 其他已知
/// 账号云端逐个卸载（fail-soft，逐账号尽力；builtin 跳过）。
/// 自装条目（无市场 UUID）只处理当前账号——没有跨账号的可靠匹配键。
#[tauri::command]
async fn uninstall_plugin_everywhere(
    state: tauri::State<'_, AppState>,
    record_id: String,
) -> Result<PluginUninstallEverywhereDto, String> {
    ensure_real_preview(&state)?;
    let material_root = checkin_material_root(&state)?;
    let storage_root = PathBuf::from(&state.storage_root);
    run_blocking("plugin_uninstall_join_failed", move || {
        let master_dir =
            master_data_dir().map_err(|_| "environment_registry_invalid".to_string())?;
        let current_account =
            resolve_actual_master_account(&material_root, &storage_root, &master_dir)?
                .ok_or_else(|| "plugin_tab_no_account".to_string())?;
        let token = plugin_tab_account_token(&material_root, &storage_root)?;
        // 先查条目：校验非 builtin，并取市场 UUID 供清单移除与传播匹配。
        let items = fetch_installed_plugins(&token)
            .map_err(|_| "plugin_tab_cloud_unavailable".to_string())?;
        let item = items
            .iter()
            .find(|item| item.record_id == record_id)
            .ok_or_else(|| "plugin_not_found".to_string())?;
        if item.builtin {
            return Err("plugin_builtin_uninstallable".to_string());
        }
        // 当前账号云端卸载：操作主体，失败整单报错（与安装对称的即时语义）。
        let removed_ok = uninstall_cloud_plugin(&token, &record_id)
            .map_err(|_| "plugin_uninstall_failed".to_string())?;
        if !removed_ok {
            return Err("plugin_uninstall_failed".to_string());
        }
        // 当前账号回执展示名：注册表账号名（备注名优先）；读不到时
        // 用通用兜底（回执是账号维度结果，不塞插件名）。
        let current_name = account_display_name(&current_account);
        let mut accounts = vec![PluginUninstallAccountDto {
            display_name: current_name,
            removed: true,
            failed: false,
            error_code: None,
        }];
        // 清单移除该插件（无市场 UUID 的自装条目本就不在清单内）。
        if let Some(market_id) = &item.marketplace_plugin_id {
            let manifest = PluginManifest::new(&storage_root);
            if let Some(mut entries) = manifest
                .load()
                .map_err(|_| "plugin_manifest_invalid".to_string())?
            {
                entries.retain(|entry| &entry.marketplace_plugin_id != market_id);
                manifest
                    .save(&entries)
                    .map_err(|_| "plugin_manifest_write_failed".to_string())?;
            }
            // 传播到其他已知账号：逐账号尽力，失败不中断（fail-soft）。
            let records = AccountRegistry::new(&material_root)
                .load()
                .map_err(|_| "checkin_registry_invalid".to_string())?;
            let others = propagate_uninstall_to_accounts(
                &records,
                Some(current_account.profile_id.as_str()),
                market_id,
                &material_root,
            );
            accounts.extend(others);
        }
        Ok(PluginUninstallEverywhereDto { accounts })
    })
    .await
}

/// 账号展示名：备注名优先，回退服务端昵称（界面纪律：内部 ID 不进主视野）。
fn account_display_name(
    record: &traesync_infrastructure::account_registry::AccountRecord,
) -> String {
    record
        .display_name
        .clone()
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| record.screen_name.clone())
}

/// 移除传播执行器（ADR-0026 决策 2，依赖注入版供 tests 验证 fail-soft）：
/// 逐账号 load_token → fetch 列表 → 按市场 UUID 定位（builtin 跳过）→
/// uninstall。任一账号失败只记回执不中断后续；未装不算失败。
/// current_profile_id 对应账号已单独处理，跳过。
fn propagate_uninstall_with(
    records: &[traesync_infrastructure::account_registry::AccountRecord],
    current_profile_id: Option<&str>,
    marketplace_plugin_id: &str,
    mut load_token: impl FnMut(
        &traesync_infrastructure::account_registry::AccountRecord,
    ) -> Option<String>,
    mut fetch: impl FnMut(&str) -> Option<Vec<CloudPluginItem>>,
    mut uninstall: impl FnMut(&str, &str) -> bool,
) -> Vec<PluginUninstallAccountDto> {
    let mut results = Vec::new();
    for record in records {
        if Some(record.profile_id.as_str()) == current_profile_id {
            continue;
        }
        let display_name = account_display_name(record);
        let mut entry = PluginUninstallAccountDto {
            display_name,
            removed: false,
            failed: false,
            error_code: None,
        };
        match load_token(record).filter(|token| !token.is_empty()) {
            None => {
                entry.failed = true;
                entry.error_code = Some("plugin_propagate_credential_unavailable".to_string());
            }
            Some(token) => match fetch(&token) {
                None => {
                    entry.failed = true;
                    entry.error_code = Some("plugin_propagate_cloud_unavailable".to_string());
                }
                Some(items) => match find_uninstall_target(&items, marketplace_plugin_id) {
                    // 该账号没装：无需移除（不是失败）。
                    None => {}
                    Some(other_record_id) => {
                        if uninstall(&token, &other_record_id) {
                            entry.removed = true;
                        } else {
                            entry.failed = true;
                            entry.error_code =
                                Some("plugin_propagate_uninstall_failed".to_string());
                        }
                    }
                },
            },
        }
        results.push(entry);
    }
    results
}

/// 移除传播到其他账号（生产实现）：凭据包取 token → 云端列表 → 云端卸载。
fn propagate_uninstall_to_accounts(
    records: &[traesync_infrastructure::account_registry::AccountRecord],
    current_profile_id: Option<&str>,
    marketplace_plugin_id: &str,
    material_root: &Path,
) -> Vec<PluginUninstallAccountDto> {
    let store = CheckinCredentialStore::new(material_root);
    propagate_uninstall_with(
        records,
        current_profile_id,
        marketplace_plugin_id,
        |record| {
            store
                .load(&CheckinProfileBinding::from(record))
                .ok()
                .map(|bundle| bundle.access_token)
        },
        |token| fetch_installed_plugins(token).ok(),
        |token, record_id| uninstall_cloud_plugin(token, record_id).unwrap_or(false),
    )
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
    ensure_fixture_runtime(&state)?;
    // 1. raw_key 未配置时返回错误——不泄露 key 是否存在
    if state.source_raw_key.is_empty() {
        return Err("fixture 模式未启用：raw key 未配置".to_string());
    }

    // 2. 用 FixturePathGuard 验证 fixture_root——拒绝真实 TRAE 路径
    let guard =
        FixturePathGuard::new(Path::new(&fixture_root)).map_err(fixture_path_command_error)?;

    // 3. 构造 service（注入 raw_key）并调用 commands 层纯函数
    let service = WorkbenchReadService::new(
        &state.workbench_probe,
        &state.workbench_reader,
        &state.source_raw_key,
    );
    // now 显式传入，避免 commands/application 依赖 wall clock
    let now = std::time::SystemTime::now();
    commands::build_work_cn_state(guard.canonical_root(), &db_relative_path, now, &service)
        .map_err(|e| e.to_string())
}

/// `restore_operation` Tauri 命令：恢复入口先保留稳定 command 契约；
/// Gate H/L 未达标前不得读取或写入任何目标（原实现即 Gate 拒斥桩）。
#[tauri::command]
fn restore_operation(
    _operation_id: String,
    _state: tauri::State<AppState>,
) -> Result<(), CommandErrorDto> {
    Err(CommandErrorDto::gate_not_qualified(
        "Gate H/L 未 Qualified：恢复能力保持禁用。",
    ))
}

/// 查询存储根绑定。正常查询只读，不会因为根缺失而创建空目录库。
#[tauri::command]
fn get_storage_root_state(state: tauri::State<AppState>) -> StorageRootStateDto {
    if state.storage_root.is_empty() {
        return StorageRootStateDto {
            configured: false,
            available: false,
            storage_root_id: None,
            canonical_path: None,
            catalog_id: None,
            write_enabled: false,
            used_bytes: None,
            warning_threshold_bytes: DEFAULT_STORAGE_WARNING_BYTES,
            warning_active: false,
            automatic_scan_paused: false,
            reason: Some("存储根未配置".to_string()),
        };
    }
    match StorageRootBinding::open_existing(Path::new(&state.storage_root), None, None) {
        Ok(binding) => match storage_space_status(Path::new(&binding.canonical_path), None) {
            Ok(space) => StorageRootStateDto {
                configured: true,
                available: true,
                storage_root_id: Some(binding.storage_root_id),
                canonical_path: Some(binding.canonical_path),
                catalog_id: Some(binding.catalog_id),
                // Gate L/N/C 未全部 Qualified，真实写入能力继续关闭。
                write_enabled: false,
                used_bytes: Some(space.used_bytes),
                warning_threshold_bytes: space.warning_threshold_bytes,
                warning_active: space.warning_active,
                automatic_scan_paused: space.automatic_scan_paused,
                reason: Some(if space.warning_active {
                    "存储根已绑定；已达到 5 GB 警戒线，非必要自动扫描暂停".to_string()
                } else {
                    "存储根已绑定；真实写入能力仍受 Gate 控制".to_string()
                }),
            },
            Err(error) => StorageRootStateDto {
                configured: true,
                available: false,
                storage_root_id: Some(binding.storage_root_id),
                canonical_path: Some(binding.canonical_path),
                catalog_id: Some(binding.catalog_id),
                write_enabled: false,
                used_bytes: None,
                warning_threshold_bytes: DEFAULT_STORAGE_WARNING_BYTES,
                warning_active: false,
                automatic_scan_paused: false,
                reason: Some(error.to_string()),
            },
        },
        Err(error) => StorageRootStateDto {
            configured: true,
            available: false,
            storage_root_id: None,
            canonical_path: None,
            catalog_id: None,
            write_enabled: false,
            used_bytes: None,
            warning_threshold_bytes: DEFAULT_STORAGE_WARNING_BYTES,
            warning_active: false,
            automatic_scan_paused: false,
            reason: Some(error.to_string()),
        },
    }
}

/// 存储根迁移在 Gate L Qualified 前保持关闭；接口保留用于后续接入确认令牌和进度流。
#[tauri::command]
fn migrate_storage_root_command(
    destination: String,
    _state: tauri::State<AppState>,
) -> Result<StorageMigrationResult, String> {
    migrate_storage_root_inner(&destination)
}

fn migrate_storage_root_inner(_destination: &str) -> Result<StorageMigrationResult, String> {
    Err("Gate L 未 Qualified：存储根迁移保持禁用".to_string())
}

/// 删除计划仅供 fixture 验证；RealReadPreview 不开放任何删除相关入口。
#[tauri::command]
fn plan_storage_deletion(
    candidates: Vec<DeletionCandidate>,
    state: tauri::State<AppState>,
) -> Result<StorageDeletionPlan, String> {
    plan_storage_deletion_inner(&state, candidates)
}

fn plan_storage_deletion_inner(
    state: &AppState,
    candidates: Vec<DeletionCandidate>,
) -> Result<StorageDeletionPlan, String> {
    if state.runtime_mode == RuntimeMode::RealReadPreview {
        return Err("RealReadPreview 不开放存储删除计划".to_string());
    }
    if state.storage_root.is_empty() {
        return Err("存储根未配置".to_string());
    }
    let binding = StorageRootBinding::open_existing(Path::new(&state.storage_root), None, None)
        .map_err(|error| error.to_string())?;
    let (guard, _) = resolve_authorized_fixture(state)?;
    let recovery_root_candidate = recovery_root_path(state)?.to_path_buf();
    let recovery_root = guard
        .validate_shared_recovery_root(&recovery_root_candidate)
        .map_err(fixture_path_command_error)?;
    let lease = guard
        .acquire_operation_lease(&recovery_root, "storage-deletion")
        .map_err(fixture_path_command_error)?;
    let plan = build_deletion_plan_with_lease(
        Path::new(&state.storage_root),
        &binding.storage_root_id,
        candidates,
        &lease,
    )
    .map_err(|error| error.to_string())?;
    *state.pending_deletion_plan.lock().unwrap() = Some(plan.clone());
    Ok(plan)
}

/// 删除执行必须由后端缓存计划驱动；当前 Gate L 未 Qualified，拒绝任何副作用。
#[tauri::command]
fn apply_storage_deletion(
    confirmation_token: String,
    unprotected_object_ids: Vec<String>,
    _state: tauri::State<AppState>,
) -> Result<Vec<DeletionTombstone>, String> {
    apply_storage_deletion_inner(&confirmation_token, &unprotected_object_ids)
}

fn apply_storage_deletion_inner(
    _confirmation_token: &str,
    _unprotected_object_ids: &[String],
) -> Result<Vec<DeletionTombstone>, String> {
    Err("Gate L 未 Qualified：存储对象删除保持禁用".to_string())
}

/// 恢复包导出/导入的基础设施已具备，Tauri 写入入口在 Gate H/N Qualified 前保持关闭。
#[tauri::command]
fn export_recovery_package_command(
    destination: String,
    passphrase: String,
    _state: tauri::State<AppState>,
) -> Result<(), String> {
    export_recovery_package_inner(&destination, &passphrase)
}

fn export_recovery_package_inner(_destination: &str, _passphrase: &str) -> Result<(), String> {
    Err("Gate H 未 Qualified：恢复包导出保持禁用".to_string())
}

#[tauri::command]
fn import_recovery_package_command(
    package: String,
    passphrase: String,
    _state: tauri::State<AppState>,
) -> Result<(), String> {
    import_recovery_package_inner(&package, &passphrase)
}

fn import_recovery_package_inner(_package: &str, _passphrase: &str) -> Result<(), String> {
    Err("Gate H 未 Qualified：恢复包导入保持禁用".to_string())
}

/// 崩溃诊断日志的固定路径（不依赖 storage_root 解析——解析本身可能
/// 就是 panic 源，取证工具必须比被取证人更早可用）。
fn crash_diagnostics_log_path() -> Option<PathBuf> {
    let base = std::env::var_os("LOCALAPPDATA")?;
    Some(
        PathBuf::from(base)
            .join("Trae Sync")
            .join("logs")
            .join("app-diagnostics.log"),
    )
}

/// 追加一行诊断日志（时间戳 + 内容）。所有失败静默（fail-soft）。
fn append_diagnostic(line: &str) {
    use std::io::Write;
    let Some(path) = crash_diagnostics_log_path() else {
        return;
    };
    let Some(parent) = path.parent() else { return };
    if std::fs::create_dir_all(parent).is_err() {
        return;
    }
    let at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0);
    let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    else {
        return;
    };
    let _ = writeln!(file, "[{at}] {line}");
}

/// 崩溃取证安装（详见 run() 内注释）：panic hook + 启动时间线。
/// 「有 started 无 exited 且无 panic」= 被外部杀死或原生层崩溃
/// （WebView2/内存错误），可据此与 Rust panic 区分。
fn install_crash_diagnostics() {
    let started = format!(
        "app started version={} pid={}",
        env!("CARGO_PKG_VERSION"),
        std::process::id()
    );
    append_diagnostic(&started);
    std::panic::set_hook(Box::new(|info| {
        // force_capture：release 下默认禁用 backtrace，强制采集（符号
        // 可能缺失，但文件/行号通常足够定位）。
        let backtrace = std::backtrace::Backtrace::force_capture();
        append_diagnostic(&format!(
            "PANIC version={} pid={} payload={info}\nbacktrace:\n{backtrace}",
            env!("CARGO_PKG_VERSION"),
            std::process::id()
        ));
    }));
}

pub fn run() {
    // 崩溃取证（2026-09-01 切换即崩溃调查）：必须在一切初始化之前装好。
    // 背景：release 配置 panic=abort，任何 panic 直接掀翻进程且不留
    // WER 记录/转储；GUI 子系统下 stderr 也被丢弃——应用死因完全无迹
    // 可查。此 hook 把 panic 信息与启动/退出时间线写入固定路径
    // %LOCALAPPDATA%\Trae Sync\logs\app-diagnostics.log（fail-soft，
    // 写失败绝不影响启动）。
    install_crash_diagnostics();
    let workbench_probe = SqlCipherProbe::new();
    let workbench_reader = AccountEvidenceReader::new();
    // raw_key 从环境变量读取——T02 fixture 模式专用，生产环境不设
    // 不进入 UI、日志或证据
    let fixture_raw_key = std::env::var("TRAE_SYNC_FIXTURE_RAW_KEY").unwrap_or_default();
    // storage_root 从环境变量读取——T03 fixture 模式专用
    // 快照发布到 <storage_root>/snapshots/，目录库由 <storage_root>/catalog/current.json 选定
    let fixture_storage_root = std::env::var("TRAE_SYNC_FIXTURE_STORAGE_ROOT").unwrap_or_default();
    // 所有应用实例固定使用同一恢复区命名空间；测试隔离通过 LOCALAPPDATA 隔离。
    let default_recovery_root = std::env::var_os("LOCALAPPDATA")
        .map(|base| {
            PathBuf::from(base)
                .join("Trae Sync")
                .join("recovery")
                .to_string_lossy()
                .into_owned()
        })
        .unwrap_or_default();

    let fixture_requested = !fixture_raw_key.is_empty() || !fixture_storage_root.is_empty();
    let (
        runtime_mode,
        provider,
        source_raw_key,
        _catalog_key,
        storage_root,
        recovery_root,
        production_catalog,
    ): (
        RuntimeMode,
        Arc<dyn WorkspaceStateProvider>,
        String,
        String,
        String,
        String,
        Option<ProductionCatalogRuntime>,
    ) = if fixture_requested {
        (
            RuntimeMode::Fixture,
            Arc::new(FixtureWorkspaceStateProvider::new(
                PathBuf::from(&fixture_storage_root),
                fixture_raw_key.clone(),
            )),
            fixture_raw_key.clone(),
            fixture_raw_key,
            fixture_storage_root,
            default_recovery_root,
            None,
        )
    } else {
        match std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .ok_or(ProductionCatalogError::LocalAppDataUnavailable)
            .and_then(|root| open_or_initialize_production_catalog(&root))
        {
            Ok(runtime) => {
                // 密钥机制收敛（2026-09-21）：source key 唯一来源是硬编码常量，
                // 启动不再读取/激活候选 profile，磁盘上遗留的 source-key-profiles 数据不再使用。
                let catalog_path = resolve_current_catalog_path(&runtime.storage_root)
                    .expect("生产目录库已经在启动阶段完成验证");
                // 启动阶段只做固定路径的元数据发现；未发现时保持 fail-closed，不能开放扫描。
                let provider = match WorkCnReadLocation::discover() {
                    Ok(location) => RealReadWorkspaceStateProvider::new(
                        Some(location.canonical_root().to_string_lossy().into_owned()),
                        catalog_path,
                        runtime.catalog_key.clone(),
                    ),
                    Err(_) => RealReadWorkspaceStateProvider::new(
                        None,
                        catalog_path,
                        runtime.catalog_key.clone(),
                    )
                    .with_location_error("work_cn_location_unavailable"),
                };
                let provider: Arc<dyn WorkspaceStateProvider> = Arc::new(provider);
                let catalog_runtime = runtime.clone();
                (
                    RuntimeMode::RealReadPreview,
                    provider,
                    WORK_CN_SOURCE_RAW_KEY.to_string(),
                    runtime.catalog_key.clone(),
                    runtime.storage_root.to_string_lossy().into_owned(),
                    runtime.recovery_root.to_string_lossy().into_owned(),
                    Some(catalog_runtime),
                )
            }
            Err(ProductionCatalogError::CatalogWriteProtocolUpgradeRequired) => (
                RuntimeMode::RealReadPreview,
                Arc::new(RealReadWorkspaceStateProvider::catalog_write_protocol_upgrade_required()),
                String::new(),
                String::new(),
                String::new(),
                default_recovery_root,
                None,
            ),
            Err(error) => (
                RuntimeMode::RealReadPreview,
                Arc::new(RealReadWorkspaceStateProvider::unavailable(format!(
                    "真实只读 Preview 启动失败：{error}"
                ))),
                String::new(),
                String::new(),
                String::new(),
                default_recovery_root,
                None,
            ),
        }
    };

    // 只恢复凭证切换的最小上下文；恢复失败时保留 journal 并让后续 apply 继续 fail-closed。
    let initial_managed_account_runtime = if recovery_root.is_empty() {
        ManagedAccountRuntimeSlot::default()
    } else {
        match recover_managed_account_runtime_from_disk(Path::new(&recovery_root)) {
            Ok(runtime) => runtime,
            Err(error) => recovery_error_runtime(Path::new(&recovery_root), &error),
        }
    };

    // 调度器启动需要 storage_root；先克隆再 move 进 AppState。
    let scheduler_root = storage_root.clone();
    tauri::Builder::default()
        .manage(AppState {
            runtime_mode,
            capabilities: CapabilityManifest::installed_0_2_1(),
            production_catalog,
            provider,
            workbench_probe,
            workbench_reader,
            source_raw_key,
            storage_root,
            recovery_root,
            process_controller: Arc::new(WorkCnProcessController::default()),
            // R1：默认未授权——必须由 grant_scan_authorization 显式授权后才能扫描
            authorization: Arc::new(Mutex::new(AuthorizationSlot::new(
                AuthorizationState::NotAuthorized,
            ))),
            pending_sync_plan: Arc::new(Mutex::new(None)),
            active_sync_cancellation: Arc::new(Mutex::new(None)),
            pending_deletion_plan: Arc::new(Mutex::new(None)),
            managed_account_runtime: Arc::new(Mutex::new(initial_managed_account_runtime)),
            checkin_cancellation: Arc::new(AtomicBool::new(false)),
            pending_login: Arc::new(Mutex::new(None)),
            active_oauth_profile: Arc::new(Mutex::new(None)),
            login_cancel: Arc::new(AtomicBool::new(false)),
            active_login_browser: Arc::new(Mutex::new(None)),
            checkin_execution_lock: Arc::new(Mutex::new(())),
        })
        .setup({
            // 仅真实模式启动自动签到调度器；fixture 演示模式无自动行为语义。
            let scheduler_mode = runtime_mode;
            move |app| {
                if scheduler_mode == RuntimeMode::RealReadPreview {
                    let lock = app.state::<AppState>().checkin_execution_lock.clone();
                    spawn_auto_checkin_scheduler(app.handle().clone(), scheduler_root, lock);
                }
                // P6-3：OAuth 临时档案周期清理——与运行模式无关，只按
                // 目录年龄回收，超龄目录不可能是进行中的登录。
                #[cfg(windows)]
                spawn_oauth_profile_cleanup_scheduler();
                Ok(())
            }
        })
        .invoke_handler(tauri::generate_handler![
            get_workspace_state,
            // 产品身份发现随凭据域迁 credential_handover（P12-R3）。
            credential_handover::get_product_identity_state,
            read_work_cn_state,
            // 账号注册表视图随凭据域迁移（P12-R3）。
            credential_handover::get_account_registry,
            restore_operation,
            get_storage_root_state,
            migrate_storage_root_command,
            plan_storage_deletion,
            apply_storage_deletion,
            export_recovery_package_command,
            import_recovery_package_command,
            // 账号中心只保存非敏感档案；切换命令进入外部切换等待态，不写认证材料。
            // P12-R3：账号/凭据/切号域命令迁 credential_handover。
            credential_handover::get_managed_account_state,
            credential_handover::get_managed_credential_statuses,
            credential_handover::capture_current_account_credential,
            credential_handover::refresh_managed_current_account,
            credential_handover::prepare_managed_account_switch,
            credential_handover::switch_account,
            credential_handover::switch_and_handoff,
            credential_handover::prepare_handoff_intent,
            // 直连签到 + 自动签到调度域命令（P12-R3 迁 checkin_schedule）。
            checkin_schedule::get_checkin_capability,
            checkin_schedule::run_checkin,
            checkin_schedule::cancel_checkin,
            // M4：OAuth 登录两段式命令（P12-R3 命令迁 oauth_login）。
            oauth_login::begin_checkin_login,
            oauth_login::complete_checkin_login,
            oauth_login::begin_trae_cn_login,
            oauth_login::complete_trae_cn_login,
            oauth_login::cancel_trae_cn_login,
            // 账号列表 / Trae CN 账号池 / 健康检测随凭据域迁移（P12-R3）。
            credential_handover::list_checkin_accounts,
            credential_handover::list_trae_cn_accounts,
            credential_handover::check_trae_cn_account_health,
            credential_handover::switch_trae_cn_account,
            // ADR-0031 账号级远程设备管理（P10-5 切片 2/3）：列表只读 + 远程退出。
            remote_devices::list_remote_devices,
            remote_devices::clear_remote_device,
            // ADR-0031 决策 2：原生列表失败时的官方设备管理页兜底入口。
            remote_devices::open_device_management_docs,
            checkin_schedule::get_checkin_overview,
            // 凭据刷新三链随凭据域迁移（P12-R3）。
            credential_handover::refresh_checkin_credits,
            credential_handover::refresh_checkin_credentials,
            credential_handover::refresh_trae_cn_credentials,
            credential_handover::remove_checkin_account,
            // ADR-0019 v4：手动重置签到设备（生成新随机设备并设为 home）。
            credential_handover::reset_checkin_device,
            // P2-2（P6-2 实例功能退役后收缩）：登录存档健康度批量查询。
            get_trae_instance_states,
            // P5-0：主库环境注册表与实例原语（P12-R3 命令迁 environment）。
            environment::launch_master_library,
            environment::get_environment_state,
            // G18 主库自检三命令随凭据域迁移（P12-R3，账号凭据生命周期）。
            credential_handover::get_master_self_check,
            credential_handover::repair_master_current_account,
            credential_handover::force_reset_master_current_account,
            environment::set_environment_current_account,
            // P6-4：环境管理 V2（多环境档案 + 生命周期 + 环境登录）。
            environment::list_environments,
            environment::create_environment,
            environment::rename_environment,
            environment::get_environment_delete_preview,
            environment::delete_environment,
            environment::launch_environment,
            environment::login_environment,
            // P11-5：环境实例关闭（主库/副环境通用，界面「关闭」按钮）。
            environment::close_environment_instance,
            // P5-1：主库切号五步事务（关实例→备份→凭据互换→交接→重启）。
            // P12-R3：随凭据域迁 credential_handover。
            credential_handover::switch_master_account,
            credential_handover::preview_master_switch_plugins,
            // P5-3：历史页主库视图（项目/会话两栏 + 接力台账 + 主库消息预览）。
            // P12-R3：库域命令迁 library_archive，注册改模块路径。
            library_archive::get_master_history,
            library_archive::get_master_session_messages,
            library_archive::get_relay_ledger,
            // P5-4 主库轻量统计 + 备份链（总览/环境卡/设置页数据源）。
            library_archive::get_master_library_stats,
            library_archive::get_master_backup_chain,
            library_archive::create_master_backup,
            library_archive::get_backup_retention,
            library_archive::set_backup_retention,
            library_archive::get_backup_dir,
            library_archive::set_backup_dir,
            library_archive::migrate_master_backups,
            library_archive::get_master_verification,
            // P5-8a 会话归档通道（ADR-0022）：归档/恢复/真实删除。
            library_archive::archive_master_sessions,
            library_archive::restore_master_sessions,
            library_archive::apply_master_archive_changes,
            library_archive::delete_master_sessions,
            library_archive::merge_master_projects,
            // P11-5 库归档草稿（localStorage 迁入 storage_root 持久化）。
            library_archive::get_archive_draft,
            library_archive::set_archive_draft,
            // P5-5 主库体检 + 一键收编（环境页）。
            library_archive::get_master_checkup,
            library_archive::incorporate_master_records,
            // P5-5 清理空项目（体检诊断口径的空壳行，破坏性 + 自动备份）。
            library_archive::cleanup_master_empty_projects,
            // P5-8b 插件 tab（ADR-0023 清单 + ADR-0026 实时同步）：
            // 状态/市场/装（零确认）/卸（全账号传播）。
            get_plugin_tab_state,
            browse_plugin_market,
            install_plugin,
            uninstall_plugin_everywhere,
            // 自动签到（ADR-0019 决策 5 / 2026-08-23 grill）：设置读写 + 每账号开关。
            checkin_schedule::get_auto_checkin_settings,
            checkin_schedule::set_auto_checkin_settings,
            checkin_schedule::set_account_auto_checkin,
            // U-1 数据层：本地备注名（脱敏手机号为登录/补采自动写入，无独立命令）。
            // G11 手机号补录：完整手机号写凭据包（DPAPI 加密，与令牌同级）。
            // U-6 W4：资产库三态筛选——归档开关 / 占用统计 / 彻底删除记录。
            // P12-R3：随凭据域迁 credential_handover。
            credential_handover::set_account_display_name,
            credential_handover::set_account_mobile,
            credential_handover::set_account_archived,
            credential_handover::get_account_storage_footprint,
            credential_handover::purge_account_records
        ])
        .run(tauri::generate_context!())
        .expect("启动 Tauri 应用时出错");
    // 正常退出闭环：与启动行配对。「有 started 无 exited 且无 PANIC」
    // = 被外部杀死或原生层崩溃（panic=abort 时不走此处，走 PANIC 行）。
    append_diagnostic("app exited normally");
}

// ============================================================================

// P6-3：OAuth 临时浏览器档案超龄清理逻辑与 oauth_profile_cleanup_tests 三例
// 已随域迁至 oauth_login 模块（P12-R3 纯移动）。

/// G23 插件同步策略（ADR-0026）：移除全账号传播（逐账号 fail-soft、builtin
/// 跳过）。切号预检分支（switch_plugin_preview_from_lists）随凭据域迁
/// credential_handover::switch_plugin_preview_tests（P12-R3）。
#[cfg(test)]
mod plugin_sync_policy_tests {
    use super::*;

    /// 构造云端已装条目（传播测试共用）。
    fn cloud_item(record_id: &str, market_id: Option<&str>, name: &str) -> CloudPluginItem {
        let mut item = CloudPluginItem {
            record_id: record_id.to_string(),
            marketplace_plugin_id: market_id.map(|id| id.to_string()),
            name: name.to_string(),
            display_name: name.to_string(),
            version: "1.0.0".to_string(),
            registry: "trae-remote-official".to_string(),
            builtin: false,
        };
        item.builtin = record_id.starts_with("builtin:");
        item
    }

    /// 构造注册表档案（传播测试共用；profile_id 决定当前账号判定）。
    fn account(profile_id: &str, name: &str) -> traesync_infrastructure::AccountRecord {
        traesync_infrastructure::AccountRecord {
            profile_id: profile_id.to_string(),
            account_id: format!("uid-{profile_id}"),
            screen_name: name.to_string(),
            avatar_url: String::new(),
            device_id: "1234567890123456".to_string(),
            device_public_key: "pub".to_string(),
            display_name: None,
            masked_mobile: String::new(),
            created_at_unix_seconds: 0,
            last_verified_at_unix_seconds: 0,
            device_created_at_unix_seconds: 0,
            auto_checkin_enabled: true,
            archived: false,
        }
    }

    #[test]
    fn propagate_uninstall_is_fail_soft_per_account() {
        // 逐账号尽力：甲凭据不可用、乙成功、丙云端卸载失败、丁未装——
        // 任一失败不中断后续账号，全部账号都有回执。
        let records = vec![
            account("profile-a", "账号甲"),
            account("profile-b", "账号乙"),
            account("profile-c", "账号丙"),
            account("profile-d", "账号丁"),
        ];
        let results = propagate_uninstall_with(
            &records,
            None, // 无当前账号（全部按其他账号处理）
            "uuid-x",
            |record| match record.profile_id.as_str() {
                "profile-a" => None, // 凭据不可用
                _ => Some(format!("token-{}", record.profile_id)),
            },
            |token| {
                if token == "token-profile-b" || token == "token-profile-c" {
                    Some(vec![cloud_item("r-b1", Some("uuid-x"), "目标插件")])
                } else {
                    Some(vec![]) // 丁没装
                }
            },
            |token, _record_id| token != "token-profile-c", // 丙的卸载失败
        );
        assert_eq!(results.len(), 4);
        // 甲：凭据不可用 → 失败但继续。
        assert!(results[0].failed);
        assert!(!results[0].removed);
        assert_eq!(
            results[0].error_code.as_deref(),
            Some("plugin_propagate_credential_unavailable")
        );
        assert_eq!(results[0].display_name, "账号甲");
        // 乙：成功移除。
        assert!(results[1].removed && !results[1].failed);
        // 丙：卸载失败（fail-soft 记失败，不影响丁）。
        assert!(results[2].failed && !results[2].removed);
        // 丁：未装 → 无需移除，不算失败。
        assert!(!results[3].removed && !results[3].failed);
        assert_eq!(results[3].error_code, None);
    }

    #[test]
    fn propagate_uninstall_skips_current_account_and_builtin() {
        // 当前账号已单独处理（云端卸载 + 清单移除），传播跳过；
        // 其他账号列表中的 builtin 条目即使带上同 UUID 也不可卸载。
        let records = vec![
            account("profile-current", "当前账号"),
            account("profile-other", "账号乙"),
        ];
        let mut builtin_with_same_id =
            cloud_item("builtin:trae-remote-official:x", Some("uuid-x"), "内置");
        builtin_with_same_id.builtin = true;
        let results = propagate_uninstall_with(
            &records,
            Some("profile-current"),
            "uuid-x",
            |_| Some("token".to_string()),
            |_| Some(vec![builtin_with_same_id.clone()]),
            |_, _| panic!("builtin 条目不可触发卸载"),
        );
        // 只剩账号乙（当前账号被跳过）；builtin 无可卸载目标 → 未装语义。
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].display_name, "账号乙");
        assert!(!results[0].removed && !results[0].failed);
    }
}

/// P7-5 凭据包实调判定（credential_login_state）离线单测：
/// live_probe 注入，覆盖三态 + 断网降级 + 无凭据包 + 存档可用性随行。
#[cfg(test)]
mod credential_login_state_tests {
    use super::*;

    /// 构造注册表档案 + 已保存凭据包的测试现场（真实 P-256 密钥，DPAPI 落盘）。
    fn fixture(
        root: &Path,
    ) -> (
        traesync_infrastructure::AccountRecord,
        CheckinCredentialStore,
    ) {
        let (private_pem, public_pem) = traesync_infrastructure::generate_device_keypair().unwrap();
        let record = traesync_infrastructure::AccountRecord {
            profile_id: "profile-health".to_string(),
            account_id: "700001".to_string(),
            screen_name: "测试账号".to_string(),
            avatar_url: String::new(),
            device_id: "1234567890123456".to_string(),
            device_public_key: public_pem.clone(),
            display_name: None,
            masked_mobile: String::new(),
            created_at_unix_seconds: 0,
            last_verified_at_unix_seconds: 0,
            device_created_at_unix_seconds: 0,
            auto_checkin_enabled: true,
            archived: false,
        };
        let store = CheckinCredentialStore::new(root);
        store
            .save(&traesync_infrastructure::CheckinCredentialBundle {
                profile_id: record.profile_id.clone(),
                account_id: record.account_id.clone(),
                device_id: record.device_id.clone(),
                machine_id: "machine-health".to_string(),
                device_public_key: public_pem,
                device_private_key: private_pem,
                access_token: "token-health".to_string(),
                refresh_token: "refresh-health".to_string(),
                client_id: "client-health".to_string(),
                access_token_expires_at_unix_seconds: u64::MAX,
                refresh_token_expires_at_unix_seconds: u64::MAX,
                mobile_full: None,
            })
            .unwrap();
        (record, store)
    }

    fn user_info(user_id: &str) -> UserInfoFull {
        UserInfoFull {
            user_id: user_id.to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn probe_success_with_matching_identity_maps_to_logged_in() {
        let root = tempfile::tempdir().unwrap();
        let (record, store) = fixture(root.path());
        let (state, archive) = credential_login_state(
            &store,
            &record,
            trae_instance_module::InstanceLoginState::LoggedIn,
            &|_| Ok(user_info("700001")),
        );
        // 徽章说有效则切号必通：实调成功且身份一致 = 登录有效。
        assert_eq!(state, trae_instance_module::InstanceLoginState::LoggedIn);
        assert!(archive);
    }

    #[test]
    fn probe_success_with_mismatched_identity_maps_to_stale() {
        let root = tempfile::tempdir().unwrap();
        let (record, store) = fixture(root.path());
        // 身份不一致 = 凭据错位不可信，与实调被拒同判失效。
        let (state, _) = credential_login_state(
            &store,
            &record,
            trae_instance_module::InstanceLoginState::LoggedIn,
            &|_| Ok(user_info("999999")),
        );
        assert_eq!(state, trae_instance_module::InstanceLoginState::Stale);
    }

    #[test]
    fn server_rejection_maps_to_stale() {
        let root = tempfile::tempdir().unwrap();
        let (record, store) = fixture(root.path());
        let (state, _) = credential_login_state(
            &store,
            &record,
            trae_instance_module::InstanceLoginState::LoggedIn,
            &|_| Err(CheckinHttpError::Business(20324)),
        );
        assert_eq!(state, trae_instance_module::InstanceLoginState::Stale);
    }

    #[test]
    fn network_failure_falls_back_to_archive_evidence() {
        let root = tempfile::tempdir().unwrap();
        let (record, store) = fixture(root.path());
        // 断网：无法实调，降级存档本地证据（与切号 E2→E1 同哲学）。
        let (state, archive) = credential_login_state(
            &store,
            &record,
            trae_instance_module::InstanceLoginState::Stale,
            &|_| Err(CheckinHttpError::Network),
        );
        assert_eq!(state, trae_instance_module::InstanceLoginState::Stale);
        assert!(archive);
    }

    #[test]
    fn missing_bundle_maps_to_uninitialized_but_archive_flag_survives() {
        let root = tempfile::tempdir().unwrap();
        let (record, _) = fixture(root.path());
        // 空仓库（未保存凭据包）：未登录，但存档可用性提示仍随行上报。
        let empty_store = CheckinCredentialStore::new(root.path().join("empty"));
        let (state, archive) = credential_login_state(
            &empty_store,
            &record,
            trae_instance_module::InstanceLoginState::LoggedIn,
            &|_| Ok(user_info("700001")),
        );
        assert_eq!(
            state,
            trae_instance_module::InstanceLoginState::Uninitialized
        );
        assert!(archive);
    }
}

// R8：组合根级顺序测试——scan/sync-plan/source 族命令已在 P6-2 退役，
// 对应前端零调用孤儿链路，因此本模块整体移除；运行中状态拒绝早拒语义、
// 未授权/范围不匹配早拒语义，已在退役说明中记录为“孤儿链路不提供保证”。
// ============================================================================

// ============================================================================
// M3：真实签到分支（run_real_checkin）——只覆盖不发网络请求的拦截路径；
// 真实 HTTP 直连行为由 infrastructure 层的 RealCheckinTransport 测试覆盖。
// ============================================================================

#[cfg(test)]
mod command_shell_tests {
    use super::*;

    /// 构造指定运行模式的极小测试状态（仅门禁与命令壳测试用；
    /// 完整版构造见 test_support::make_real_mode_state）。
    fn make_gate_state(mode: RuntimeMode) -> AppState {
        AppState {
            runtime_mode: mode,
            capabilities: CapabilityManifest::installed_0_2_1(),
            production_catalog: None,
            provider: Arc::new(StaticWorkspaceStateProvider::new()),
            workbench_probe: SqlCipherProbe::new(),
            workbench_reader: AccountEvidenceReader::new(),
            source_raw_key: String::new(),
            storage_root: String::new(),
            recovery_root: String::new(),
            process_controller: Arc::new(FixedProcessController::not_running()),
            authorization: Arc::new(Mutex::new(AuthorizationSlot::new(
                AuthorizationState::NotAuthorized,
            ))),
            pending_sync_plan: Arc::new(Mutex::new(None)),
            active_sync_cancellation: Arc::new(Mutex::new(None)),
            pending_deletion_plan: Arc::new(Mutex::new(None)),
            managed_account_runtime: Arc::new(Mutex::new(ManagedAccountRuntimeSlot::default())),
            checkin_cancellation: Arc::new(AtomicBool::new(false)),
            pending_login: Arc::new(Mutex::new(None)),
            active_oauth_profile: Arc::new(Mutex::new(None)),
            login_cancel: Arc::new(AtomicBool::new(false)),
            active_login_browser: Arc::new(Mutex::new(None)),
            checkin_execution_lock: Arc::new(Mutex::new(())),
        }
    }

    /// TRAE 实例/主库域门禁：真实模式通过，fixture 拒绝并返回域稳定错误码。
    #[test]
    fn ensure_real_preview_allows_real_mode_and_rejects_fixture() {
        let real = make_gate_state(RuntimeMode::RealReadPreview);
        assert!(ensure_real_preview(&real).is_ok());

        let fixture = make_gate_state(RuntimeMode::Fixture);
        assert_eq!(
            ensure_real_preview(&fixture).unwrap_err(),
            "trae_real_mode_required"
        );
    }

    /// 登录域门禁：fixture 拒绝时保持登录域自己的稳定错误码。
    #[test]
    fn ensure_login_real_mode_allows_real_mode_and_rejects_fixture() {
        let real = make_gate_state(RuntimeMode::RealReadPreview);
        assert!(ensure_login_real_mode(&real).is_ok());

        let fixture = make_gate_state(RuntimeMode::Fixture);
        assert_eq!(
            ensure_login_real_mode(&fixture).unwrap_err(),
            "login_real_mode_required"
        );
    }

    /// 签到 HTTP 域门禁：fixture 拒绝时保持签到域自己的稳定错误码。
    #[test]
    fn ensure_checkin_http_enabled_allows_real_mode_and_rejects_fixture() {
        let real = make_gate_state(RuntimeMode::RealReadPreview);
        assert!(ensure_checkin_http_enabled(&real).is_ok());

        let fixture = make_gate_state(RuntimeMode::Fixture);
        assert_eq!(
            ensure_checkin_http_enabled(&fixture).unwrap_err(),
            "checkin_http_disabled"
        );
    }

    /// fixture 门禁反向用例：fixture 模式通过，真实模式拒绝。
    #[test]
    fn ensure_fixture_runtime_allows_fixture_and_rejects_real_mode() {
        let fixture = make_gate_state(RuntimeMode::Fixture);
        assert!(ensure_fixture_runtime(&fixture).is_ok());

        let real = make_gate_state(RuntimeMode::RealReadPreview);
        assert!(ensure_fixture_runtime(&real).is_err());
    }

    /// run_blocking：闭包成功值与闭包自身错误原样透传（join 不改写业务结果）。
    #[test]
    fn run_blocking_passes_through_success_and_inner_error() {
        let ok = tauri::async_runtime::block_on(async {
            run_blocking("test_join_failed", || Ok::<u8, String>(7)).await
        });
        assert_eq!(ok, Ok(7));

        let inner_err = tauri::async_runtime::block_on(async {
            run_blocking("test_join_failed", || Err::<u8, String>("inner_code".to_string())).await
        });
        assert_eq!(inner_err, Err("inner_code".to_string()));
    }

    /// run_blocking：闭包 panic（join 失败）映射为稳定的 join 错误码。
    /// 测试日志中的 panic 输出来自本用例的故意触发，属预期现象。
    #[test]
    fn run_blocking_maps_join_failure_to_stable_error_code() {
        let joined = tauri::async_runtime::block_on(async {
            run_blocking("test_join_failed", || -> Result<u8, String> {
                panic!("故意 panic：验证 join 失败映射")
            })
            .await
        });
        assert_eq!(joined, Err("test_join_failed".to_string()));
    }
}

