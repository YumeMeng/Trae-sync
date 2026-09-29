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

use chrono::{DateTime, SecondsFormat, Utc};
use serde::Serialize;
use sha2::{Digest, Sha256};
use tauri::{Emitter, Manager};
// trait 通过 application 重导出，避免 commands 直接依赖 ports crate
use traesync_application::{
    checkin_transport_error_code, AccountEvidenceReaderPort, CheckinTransport,
    ManagedAccountSwitchService, ProcessControllerPort,
    WorkbenchReadService, WorkspaceStateProvider,
};
use traesync_commands as commands;
// R8：组合根直接使用 commands::HistoryCommandError 构造授权失败错误消息
use traesync_commands::HistoryCommandError;
use traesync_domain::{
    AccountEvidence, AccountProfile, AccountSwitchPlan, AccountSwitchPreflight,
    AccountVerificationState, AuthorizationState, BrowseAccountNode, BrowseResult,
    CurrentAccountEvidence, CurrentAccountState, EvidenceState, HandoffIntent, HandoffIntentState,
    ManagedAccountRuntime, OperationCancellation, OperationId, SyncPlan, SyncScope, UserId,
    WorkbenchReadState, WorkspaceState,
};
use traesync_infrastructure::{
    capture_location_identity, capture_location_witness, compare_location_identity,
    fetch_installed_plugins, fetch_market_plugins, find_uninstall_target,
    get_user_info, install_market_plugin, load_account_fingerprint_salt,
    load_or_create_account_fingerprint_salt, open_or_initialize_production_catalog, reconcile_plan,
    resolve_current_catalog_generation_id, resolve_current_catalog_path,
    salted_user_id_fingerprint, storage_space_status, sync_account_cloud_plugins,
    uninstall_cloud_plugin, user_id_binding_fingerprint, user_id_display_fingerprint,
    AccountEvidenceReader, AccountRecord, AccountRegistry, CheckinCredentialError,
    CheckinCredentialStore, CheckinProfileBinding, CloudPluginItem, CredentialApplyOutcome,
    CredentialBinding, CredentialMaintenanceStateStore, CredentialState, CredentialStatus,
    CredentialVault, DeviceRemintService, FixtureCheckinTransport,
    FixturePathGuard, FixtureWorkspaceStateProvider, JsonHandoffIntentStore,
    JsonManagedAccountProfileStore, MarketPluginItem, OperationLease,
    PlatformFileIdentityProvider, PluginCloudSyncOutcome, PluginManifest, PluginManifestEntry,
    ProductionCatalogError, ProductionCatalogRuntime, RealCheckinRenewalService,
    RealCheckinTransport, RealReadWorkspaceStateProvider, RemintError,
    SqlCipherProbe, StorageDeletionPlan, StorageMigrationResult, StorageRootBinding,
    ProductAccountStateStore, TraeProduct, WorkCnProcessController, WorkCnReadLocation,
    DEFAULT_STORAGE_WARNING_BYTES,
};

#[cfg(test)]
use traesync_infrastructure::{FixedProcessController, StaticWorkspaceStateProvider};

use traesync_infrastructure::{
    build_deletion_plan_with_lease, DeletionCandidate, DeletionTombstone,
};
// P12-R3：OAuth 登录命令链已迁 oauth_login 模块（命令/DTO/浏览器拉起/临时档案
// 清理随域纯移动）。本节仅保留账号列表与健康检测等仍属组合根的 wire DTO。
// P7-1 追加 get_user_info_full（切号 E2 构造的实调身份校验与资料来源）。
// P7-5 追加 CheckinHttpError / UserInfoFull（健康度凭据包实调判定）。
use traesync_infrastructure::{
    get_user_info_full, get_user_info_full_for_oauth_client, trae_http_client, CheckinHttpError,
    LoginSession, OAuthClient, UserInfoFull,
};
// P2-2 TRAE 实例管理：路径发现、进程状态、窗口聚焦、登录态种子。
// U-6 W4 追加 native_account_dir / dir_size_recursive（占用统计与彻底删除定位原生目录）。
// P12-R3：close_instance / seed_login_state_from_donor / CloseOutcome 随
// 环境/库域迁出组合根，经对应域模块引用。
use traesync_infrastructure::trae_instance::{
    self as trae_instance_module, close_master_instance,
    command_line_uses_data_dir, dir_size_recursive, discover_trae_executable,
    focus_instance_windows, instance_data_dir, list_trae_processes, native_account_dir,
    seed_login_state,
};
// P5-0 主库环境注册表（环境模型 grill Q1/Q2/Q7：V1 单主库环境档案持久化）。
// P12-R3：environment_registry_error_code / master_data_dir_for /
// secondary_data_dir 随环境/库域迁出组合根，经对应域模块引用。
use traesync_infrastructure::environment_registry::{
    master_data_dir, EnvironmentRegistry, MASTER_ENV_ID,
};
// P12-R1b：产品×库 → 当前账号解析（原组合根 resolve_library_account 系下沉）。
use traesync_infrastructure::library_account::{
    self, observed_profile_id, trae_cn_material_root_from, TraeCnPointerLookup,
};
// P5-1 主库切号五步事务原语：交接（备份/活动检测/换腿）、凭据接力台账。
// P7-1 追加 E2 构造路径：construct_auth_identity（凭据包 + GetUserInfo 构造登录态）。
// P11-4 追加：跨目录三件套备份（_to 家族 + 同戳版本）、目录版备份链枚举。
// P12-R3：备份链枚举/库备份工具随库域迁 library_archive，经转发导入引用。
use traesync_infrastructure::{
    archive_login_user_id, backup_master_trio_to, construct_auth_identity,
    handover_master_records_with_progress, master_database_path, master_db_activity_detected,
    switch_auth_identity, AuthIdentitySwitch, AuthSwitchError, ConstructAuthInput,
    HandoverProgress, MasterHandover, MasterHandoverError, RelayLedger, RelayLedgerEntry,
};
// P5-9 备份保留策略：定次自动裁剪（设置存取 + 备份链清理）。
// P11-4 追加：备份位置覆盖读取、目录校验、迁移（_inner 别名避免与命令同名冲突）。
// P12-R3：备份管理命令族随库域迁 library_archive；切号/purge 引用的
// 活动目录解析与裁剪工具经转发导入。
use library_archive::{prune_backups_if_enabled, resolve_master_backup_dir};
// U-6 W3 会话索引变化检测：三件套 stat 指纹（命令 DTO 直接复用 infrastructure 类型，
// wire 格式为 snake_case 字段，与现有 DTO 口径一致）。
// U-6 W4 追加 SessionIndexCacheStore：彻底删除记录时清理会话索引缓存。
// P12-R3：InstanceFingerprint 随库域迁 library_archive。
use traesync_infrastructure::account_session_index::SessionIndexCacheStore;

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
// P12-R3：跨域测试共享夹具（AppState 构造），仅测试编译期可用。
#[cfg(test)]
mod test_support;
// P12-R3：自动签到调度线程由组合根启动；留守批量链复用统一进度事件契约与
// 串行运行器；get_relay_ledger / 账号列表 / oauth_login 沿用同一签到材料根约定。
use checkin_schedule::{
    checkin_material_root, run_serial_batch, spawn_auto_checkin_scheduler, BatchProgressEvent,
};
// P12-R2a：映射函数随迁专用模块；转发导入保持 lib.rs 调用面零改动。
// P12-R3：map_master_archive_error 随库域迁 library_archive（经该模块引用）。
use error_mapping::{
    credential_error_text, credential_maintenance_error_code, fixture_path_command_error,
    trae_cn_health_error_code, work_cn_location_command_error,
};
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

/// Tauri 账号中心专用 wire DTO；不把 domain 内部指纹、备份引用或认证材料发给前端。
#[derive(Debug, Clone, Serialize)]
struct ManagedAccountProfileWireDto {
    profile_id: String,
    display_name: String,
    region: Option<String>,
    data_location_id: String,
    last_verified_at: Option<String>,
    verification_state: AccountVerificationState,
}

#[derive(Debug, Clone, Serialize)]
struct CurrentAccountEvidenceWireDto {
    profile_id: Option<String>,
    display_name: Option<String>,
    region: Option<String>,
    data_location_id: Option<String>,
    verification_state: AccountVerificationState,
    observed_at: Option<String>,
    reason: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
struct ManagedAccountVerificationWireDto {
    verification_state: AccountVerificationState,
    checked_at: Option<String>,
    reason: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
struct AccountSwitchPreflightWireDto {
    source_verified: bool,
    target_known: bool,
    target_verified: bool,
    target_location_known: bool,
    pending_sync_plan_cleared: bool,
    trae_closed: bool,
    ready: bool,
    reason: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
struct ManagedAccountSwitchWireDto {
    plan_id: String,
    source_profile_id: Option<String>,
    target_profile_id: String,
    source_data_location_id: Option<String>,
    target_data_location_id: String,
    preflight: AccountSwitchPreflightWireDto,
    state: traesync_domain::AccountSwitchState,
    created_at: String,
    failure_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
struct ManagedAccountsWireView {
    saved_accounts: Vec<ManagedAccountProfileWireDto>,
    current_account: CurrentAccountEvidenceWireDto,
    recent_verification: ManagedAccountVerificationWireDto,
    switch_state: Option<ManagedAccountSwitchWireDto>,
    handoff_intent: Option<HandoffIntentWireDto>,
    history_is_separate: bool,
}

/// 发现账号与本机档案/凭证的只读绑定状态。库存扫描不会隐式改变这些状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum AccountRegistryAuthorizationState {
    CurrentVerified,
    CredentialSaved,
    ProfileSaved,
    CredentialStale,
    CredentialInvalid,
    ReverificationRequired,
    Conflict,
    Unbound,
}

/// 账号注册表只返回历史统计和非敏感档案状态，不返回原始凭证内容。
#[derive(Debug, Clone, Serialize)]
struct AccountRegistryEntryWireDto {
    history_account_id: String,
    display_label: String,
    project_count: u64,
    session_count: u64,
    profile_id: Option<String>,
    profile_display_name: Option<String>,
    profile_verification_state: Option<AccountVerificationState>,
    credential_state: Option<CredentialState>,
    authorization_state: AccountRegistryAuthorizationState,
}

#[derive(Debug, Clone, Serialize)]
struct AccountRegistryWireView {
    accounts: Vec<AccountRegistryEntryWireDto>,
}

#[derive(Debug, Clone, Serialize)]
struct HandoffIntentWireDto {
    intent_id: String,
    source_profile_id: Option<String>,
    target_profile_id: String,
    data_location_id: String,
    scope: SyncScope,
    state: HandoffIntentState,
    created_at: String,
    updated_at: String,
    failure_reason: Option<String>,
}

/// 产品身份只读结果：不返回 userId、token 或任何 native storage 正文。
#[derive(Debug, Clone, Serialize)]
struct ProductIdentityStateWireDto {
    product_id: String,
    display_name: String,
    identity_status: String,
    credential_status: String,
    relation_to_work: String,
}

/// 账号凭证状态只返回绑定摘要与状态，不返回密文或认证正文。
#[derive(Debug, Clone, Serialize)]
struct ManagedCredentialStatusWireDto {
    profile_id: String,
    state: CredentialState,
    data_location_id: String,
    format_version: Option<u32>,
}

/// 请求当前活动的可取消操作停止；无活动操作时直接返回 false。
///
/// 在 P6-2 会话索引/计划族退役后，仍然保留：切换账号与授权撤销时
/// 仍需要通知旧代次长任务（即使当前没有任何计划族命令）。
fn request_cancel_inner(state: &AppState) -> bool {
    let cancellation = state.active_sync_cancellation.lock().unwrap().clone();
    if let Some(cancellation) = cancellation {
        cancellation.request();
        true
    } else {
        false
    }
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

fn system_time_to_rfc3339(value: std::time::SystemTime) -> String {
    DateTime::<Utc>::from(value).to_rfc3339_opts(SecondsFormat::Millis, true)
}

fn unix_now_seconds() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

fn optional_system_time_to_rfc3339(value: Option<std::time::SystemTime>) -> Option<String> {
    value.map(system_time_to_rfc3339)
}

fn managed_profile_wire(profile: &AccountProfile) -> ManagedAccountProfileWireDto {
    ManagedAccountProfileWireDto {
        profile_id: profile.profile_id.clone(),
        display_name: profile.display_name.clone(),
        region: profile.region.clone(),
        data_location_id: profile.data_location_id.clone(),
        last_verified_at: optional_system_time_to_rfc3339(profile.last_verified_at),
        verification_state: profile.verification_state,
    }
}

fn managed_current_wire(account: &CurrentAccountEvidence) -> CurrentAccountEvidenceWireDto {
    CurrentAccountEvidenceWireDto {
        profile_id: account.profile_id.clone(),
        display_name: account.display_name.clone(),
        region: account.region.clone(),
        data_location_id: account.data_location_id.clone(),
        verification_state: account.verification_state,
        observed_at: optional_system_time_to_rfc3339(account.observed_at),
        reason: account.reason.clone(),
    }
}

fn managed_preflight_wire(preflight: &AccountSwitchPreflight) -> AccountSwitchPreflightWireDto {
    AccountSwitchPreflightWireDto {
        source_verified: preflight.source_verified,
        target_known: preflight.target_known,
        target_verified: preflight.target_verified,
        target_location_known: preflight.target_location_known,
        pending_sync_plan_cleared: preflight.pending_sync_plan_cleared,
        trae_closed: preflight.trae_closed,
        ready: preflight.ready,
        reason: preflight.reason.clone(),
    }
}

fn managed_switch_wire(plan: &AccountSwitchPlan) -> ManagedAccountSwitchWireDto {
    ManagedAccountSwitchWireDto {
        plan_id: plan.plan_id.clone(),
        source_profile_id: plan.source_profile_id.clone(),
        target_profile_id: plan.target_profile_id.clone(),
        source_data_location_id: plan.source_data_location_id.clone(),
        target_data_location_id: plan.target_data_location_id.clone(),
        preflight: managed_preflight_wire(&plan.preflight),
        state: plan.state,
        created_at: system_time_to_rfc3339(plan.created_at),
        failure_reason: plan.failure_reason.clone(),
    }
}

fn handoff_intent_wire(intent: &HandoffIntent) -> HandoffIntentWireDto {
    HandoffIntentWireDto {
        intent_id: intent.intent_id.clone(),
        source_profile_id: intent.source_profile_id.clone(),
        target_profile_id: intent.target_profile_id.clone(),
        data_location_id: intent.data_location_id.clone(),
        scope: intent.scope.clone(),
        state: intent.state,
        created_at: system_time_to_rfc3339(intent.created_at),
        updated_at: system_time_to_rfc3339(intent.updated_at),
        failure_reason: intent.failure_reason.clone(),
    }
}

fn managed_accounts_wire_view(
    runtime: &ManagedAccountRuntime,
    authorization_context_current: bool,
    handoff_intent: Option<&HandoffIntent>,
) -> ManagedAccountsWireView {
    let visible_current = if authorization_context_current {
        runtime.current_account.clone()
    } else {
        CurrentAccountEvidence::default()
    };
    let current_account = managed_current_wire(&visible_current);
    ManagedAccountsWireView {
        saved_accounts: runtime.profiles.iter().map(managed_profile_wire).collect(),
        recent_verification: ManagedAccountVerificationWireDto {
            verification_state: visible_current.verification_state,
            checked_at: visible_current.observed_at.map(system_time_to_rfc3339),
            reason: visible_current.reason.clone(),
        },
        current_account,
        // WaitingForTraeClosed 计划不含凭证；撤销旧授权后仍需显示，供用户完成外部切换。
        switch_state: runtime.switch_plan.as_ref().and_then(|plan| {
            if authorization_context_current
                || matches!(
                    plan.state,
                    traesync_domain::AccountSwitchState::WaitingForTraeClosed
                        | traesync_domain::AccountSwitchState::Applying
                        | traesync_domain::AccountSwitchState::Verifying
                        | traesync_domain::AccountSwitchState::ManualRecoveryRequired
                )
            {
                Some(managed_switch_wire(plan))
            } else {
                None
            }
        }),
        handoff_intent: handoff_intent.map(handoff_intent_wire),
        history_is_separate: true,
    }
}

fn account_registry_display_label(account: &BrowseAccountNode) -> String {
    UserId::from_verified(&account.user_id)
        .map(|value| format!("TRAE Work CN · {}", user_id_display_fingerprint(&value)))
        .unwrap_or_else(|_| {
            let label = account.display_label.trim();
            if label.is_empty() {
                "来源账号（已识别）".to_string()
            } else {
                label.to_string()
            }
        })
}

/// 把单个历史账号与账号档案做精确匹配。零匹配不阻塞历史，多匹配失败关闭。
fn build_account_registry_entry(
    account: &BrowseAccountNode,
    profiles: &[AccountProfile],
    current_profile_id: Option<&str>,
    credential_states: &std::collections::HashMap<String, CredentialState>,
    salted_fingerprint: Option<&str>,
    legacy_fingerprint: Option<&str>,
) -> AccountRegistryEntryWireDto {
    let current_matches = salted_fingerprint
        .map(|fingerprint| {
            profiles
                .iter()
                .filter(|profile| {
                    profile.fingerprint_version == traesync_domain::ACCOUNT_FINGERPRINT_VERSION
                        && profile.user_fingerprint == fingerprint
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let legacy_matches = legacy_fingerprint
        .map(|fingerprint| {
            profiles
                .iter()
                .filter(|profile| {
                    profile.fingerprint_version
                        == traesync_domain::LEGACY_ACCOUNT_FINGERPRINT_VERSION
                        && profile.user_fingerprint == fingerprint
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    let ambiguous = current_matches.len() > 1
        || legacy_matches.len() > 1
        || (!current_matches.is_empty() && !legacy_matches.is_empty());
    let (profile, credential_state, authorization_state) = if ambiguous {
        (None, None, AccountRegistryAuthorizationState::Conflict)
    } else if let Some(profile) = current_matches.first().copied() {
        let credential_state = credential_states.get(&profile.profile_id).copied();
        let authorization_state = if profile.verification_state
            != AccountVerificationState::Verified
        {
            AccountRegistryAuthorizationState::ReverificationRequired
        } else if current_profile_id == Some(profile.profile_id.as_str()) {
            AccountRegistryAuthorizationState::CurrentVerified
        } else {
            match credential_state {
                Some(CredentialState::Saved) => AccountRegistryAuthorizationState::CredentialSaved,
                Some(CredentialState::Stale) => AccountRegistryAuthorizationState::CredentialStale,
                Some(CredentialState::Invalid) => {
                    AccountRegistryAuthorizationState::CredentialInvalid
                }
                Some(CredentialState::Missing) | None => {
                    AccountRegistryAuthorizationState::ProfileSaved
                }
            }
        };
        (Some(profile), credential_state, authorization_state)
    } else if let Some(profile) = legacy_matches.first().copied() {
        (
            Some(profile),
            None,
            AccountRegistryAuthorizationState::ReverificationRequired,
        )
    } else {
        (None, None, AccountRegistryAuthorizationState::Unbound)
    };

    AccountRegistryEntryWireDto {
        history_account_id: account.user_id.clone(),
        display_label: account_registry_display_label(account),
        project_count: account.project_count,
        session_count: account.session_count,
        profile_id: profile.map(|profile| profile.profile_id.clone()),
        profile_display_name: profile.map(|profile| profile.display_name.clone()),
        profile_verification_state: profile.map(|profile| profile.verification_state),
        credential_state,
        authorization_state,
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

/// 仅撤销仍属于指定代次的读取上下文，避免旧异步读取清掉新授权。
fn invalidate_authorized_read_context_if_current(
    state: &AppState,
    generation: u64,
    expected: &AuthorizationState,
) {
    let invalidated = {
        let mut slot = state.authorization.lock().unwrap();
        let invalidated = slot.revoke_if_matches(generation, expected);
        if invalidated {
            // 与计划写入保持同一锁序：authorization -> pending_sync_plan。
            state.pending_sync_plan.lock().unwrap().take();
        }
        invalidated
    };
    if invalidated {
        request_cancel_inner(state);
    }
}

/// 账号切换确认时立即撤销旧授权、清空旧计划，并请求可取消操作停止。
///
/// 锁顺序固定为 authorization -> pending_sync_plan，避免与扫描授权和计划缓存交错。
fn invalidate_authorization_for_account_switch(state: &AppState) {
    {
        let mut authorization = state.authorization.lock().unwrap();
        authorization.revoke();
        state.pending_sync_plan.lock().unwrap().take();
    }
    // 保留 WaitingForTraeClosed 计划，供外部账号切换完成后重新授权并复核。
    state
        .managed_account_runtime
        .lock()
        .unwrap()
        .authorization_generation = None;
    request_cancel_inner(state);
}

/// 判断错误是否说明当前读取绑定已经不能继续信任。
fn should_invalidate_authorized_read_context(error: &str) -> bool {
    let normalized = error.to_ascii_lowercase();
    normalized.contains("authorization_mismatch")
        || normalized.contains("not_authorized")
        || normalized.contains("account_evidence_changed")
        || normalized.contains("data_location_changed")
        || normalized.contains("data_location_unavailable")
        || normalized.contains("account_evidence_unavailable")
        || error.contains("未授权")
        || error.contains("授权不匹配")
        || error.contains("账号证据")
        || error.contains("数据位置身份")
        || error.contains("APPDATA 未设置")
        || error.contains("APPDATA 必须")
        || error.contains("APPDATA 不可用")
        || error.contains("TRAE 默认根目录")
        || error.contains("TRAE 默认数据库")
}

/// 读取入口出现授权漂移时撤销旧绑定；普通目录库/存储错误不应误撤销用户授权。
fn invalidate_on_authorization_error<T>(
    state: &AppState,
    generation: u64,
    expected: &AuthorizationState,
    error: String,
) -> Result<T, String> {
    if should_invalidate_authorized_read_context(&error) {
        invalidate_authorized_read_context_if_current(state, generation, expected);
    }
    Err(error)
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

/// 在创建恢复区前先完成 fixture 边界校验，创建后再复核真实物理路径。
///
/// 这样即使配置指向不存在的越界路径，也不会先产生目录副作用。
fn prepare_recovery_root(state: &AppState, guard: &FixturePathGuard) -> Result<PathBuf, String> {
    let candidate = recovery_root_path(state)?.to_path_buf();
    let safe_candidate = guard
        .validate_shared_recovery_root(&candidate)
        .map_err(fixture_path_command_error)?;
    std::fs::create_dir_all(&safe_candidate).map_err(|_| "恢复区不可用".to_string())?;
    guard
        .validate_shared_recovery_root(&safe_candidate)
        .map_err(fixture_path_command_error)
}

/// `get_workspace_state` Tauri command。
/// T01 阶段返回固定的空工作台状态。
#[tauri::command]
fn get_workspace_state(state: tauri::State<AppState>) -> WorkspaceState {
    workspace_state_for_app(&state)
}

/// 只读发现已安装产品中的账号身份关系。
///
/// 该命令只解析固定产品根目录中的账号证据，不写 storage、数据库或凭据，
/// 也不做 token 交换、refresh、签到、设备注册或设备重铸。
#[tauri::command]
fn get_product_identity_state() -> Vec<ProductIdentityStateWireDto> {
    let products = [TraeProduct::WorkCn, TraeProduct::TraeCn];
    let evidence: Vec<(TraeProduct, Option<UserId>, String)> = products
        .into_iter()
        .map(|product| {
            let result = WorkCnReadLocation::discover_for(product)
                .ok()
                .map(|location| {
                    let account = AccountEvidenceReader::new().read_account_evidence(
                        location.canonical_root(),
                        std::time::SystemTime::now(),
                    );
                    let status = match account.evidence_state {
                        EvidenceState::Conflict => "conflict",
                        _ if account.user_id.is_some() => "recognized",
                        _ => "unknown",
                    };
                    (account.user_id, status.to_string())
                });
            let (user_id, identity_status) = result.unwrap_or((None, "unknown".to_string()));
            (product, user_id, identity_status)
        })
        .collect();

    let work_user_id = evidence
        .iter()
        .find(|(product, _, status)| *product == TraeProduct::WorkCn && status == "recognized")
        .and_then(|(_, user_id, _)| user_id.as_ref())
        .cloned();

    evidence
        .into_iter()
        .map(|(product, user_id, identity_status)| {
            let relation_to_work = match product {
                TraeProduct::WorkCn => "current_product".to_string(),
                TraeProduct::TraeCn => {
                    match (work_user_id.as_ref(), user_id.as_ref(), identity_status.as_str()) {
                        (Some(work), Some(cn), "recognized") if work == cn => {
                            "same_identity".to_string()
                        }
                        (Some(_), Some(_), "recognized") => "different_identity".to_string(),
                        _ => "unknown".to_string(),
                    }
                }
            };
            ProductIdentityStateWireDto {
                product_id: product.product_id().to_string(),
                display_name: product.display_name().to_string(),
                identity_status,
                credential_status: match product {
                    TraeProduct::WorkCn => "managed_by_work".to_string(),
                    TraeProduct::TraeCn => "need_authorization".to_string(),
                },
                relation_to_work,
            }
        })
        .collect()
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

/// 构造 ManagedAccountSwitch 的档案 ID；身份作用域同时绑定数据位置与完整指纹。
///
/// 前端只看到组合摘要，完整指纹仍只用于后端档案匹配，不进入 wire DTO。
fn managed_profile_id(data_location_id: &str, user_fingerprint: &str) -> String {
    let mut digest = Sha256::new();
    digest.update(data_location_id.as_bytes());
    digest.update([0]);
    digest.update(user_fingerprint.as_bytes());
    let encoded = hex::encode(digest.finalize());
    format!("profile-{}", &encoded[..16])
}

fn managed_verification_state(account: &AccountEvidence) -> AccountVerificationState {
    match account.evidence_state {
        EvidenceState::Verified => AccountVerificationState::Verified,
        EvidenceState::SingleSource => AccountVerificationState::SingleSource,
        EvidenceState::Conflict => AccountVerificationState::Conflict,
        EvidenceState::Missing => AccountVerificationState::Unknown,
        EvidenceState::Expired => AccountVerificationState::Expired,
        EvidenceState::FingerprintChanged => AccountVerificationState::FingerprintChanged,
    }
}

/// 将当前账号白名单证据转成账号中心 DTO；此 DTO 不包含认证正文。
fn managed_current_evidence(
    account: &AccountEvidence,
    data_location_id: String,
    observed_at: std::time::SystemTime,
    fingerprint_salt: Option<&[u8]>,
) -> CurrentAccountEvidence {
    let user_fingerprint = account.user_id.as_ref().and_then(|user_id| {
        fingerprint_salt.map(|salt| salted_user_id_fingerprint(salt, user_id.as_str()))
    });
    let profile_id = user_fingerprint
        .as_deref()
        .map(|fingerprint| managed_profile_id(&data_location_id, fingerprint));
    let display_name = user_fingerprint
        .as_deref()
        .map(|fingerprint| format!("TRAE Work CN · {}", &fingerprint[..16]));
    let verification_state = managed_verification_state(account);
    CurrentAccountEvidence {
        profile_id,
        display_name,
        user_fingerprint,
        fingerprint_version: if fingerprint_salt.is_some() && account.user_id.is_some() {
            traesync_domain::ACCOUNT_FINGERPRINT_VERSION
        } else {
            traesync_domain::LEGACY_ACCOUNT_FINGERPRINT_VERSION
        },
        region: Some("cn".to_string()),
        data_location_id: Some(data_location_id),
        verification_state,
        observed_at: Some(observed_at),
        reason: match verification_state {
            AccountVerificationState::Verified => None,
            _ => Some(format!("account_evidence_{verification_state:?}").to_ascii_lowercase()),
        },
    }
}

struct ManagedAccountObservation {
    account: AccountEvidence,
    data_location_id: String,
    data_root: PathBuf,
    storage_root: PathBuf,
    observed_at: std::time::SystemTime,
}

struct ManagedAccountStorePaths {
    profile_path: PathBuf,
    salt_path: PathBuf,
}

/// 解析普通账号档案读取路径。该函数不创建目录、salt、锁文件或 catalog sidecar。
fn managed_account_store_paths_for_read(
    state: &AppState,
) -> Result<Option<ManagedAccountStorePaths>, String> {
    if state.storage_root.is_empty() {
        return Err("account_profile_store_unavailable".to_string());
    }
    let (storage_root, recovery_root) = match state.runtime_mode {
        RuntimeMode::Fixture => {
            let (generation, authorization) = {
                let slot = state.authorization.lock().unwrap();
                if matches!(slot.state, AuthorizationState::NotAuthorized) {
                    return Ok(None);
                }
                (slot.generation, slot.state.clone())
            };
            let (data_root, _, storage_root) =
                resolve_authorized_read_target_for_context(state, generation, &authorization)?;
            let guard = FixturePathGuard::new(&data_root).map_err(fixture_path_command_error)?;
            let recovery_root = guard
                .validate_shared_recovery_root(recovery_root_path(state)?)
                .map_err(fixture_path_command_error)?;
            ensure_authorization_context_current(state, generation, &authorization)?;
            (storage_root, recovery_root)
        }
        RuntimeMode::RealReadPreview => {
            let runtime = state
                .production_catalog
                .as_ref()
                .ok_or_else(|| "account_profile_store_unavailable".to_string())?;
            if !runtime.storage_root.exists() {
                return Ok(None);
            }
            // 只复核既有存储根绑定；不调用可能修复 generation.json 的 catalog verify。
            StorageRootBinding::open_existing(
                &runtime.storage_root,
                None,
                Some(&runtime.catalog_id),
            )
            .map_err(|error| error.to_string())?;
            (runtime.storage_root.clone(), runtime.recovery_root.clone())
        }
    };
    Ok(Some(ManagedAccountStorePaths {
        profile_path: storage_root.join("managed-accounts.json"),
        salt_path: recovery_root.join("managed-account-fingerprint-salt.bin"),
    }))
}

fn managed_account_store_for_read(
    state: &AppState,
) -> Result<Option<JsonManagedAccountProfileStore>, String> {
    let Some(paths) = managed_account_store_paths_for_read(state)? else {
        return Ok(None);
    };
    let fingerprint_salt = load_account_fingerprint_salt(&paths.salt_path)?;
    JsonManagedAccountProfileStore::for_read(paths.profile_path, fingerprint_salt).map(Some)
}

/// 返回固定恢复区的承接意图存储。该函数不创建目录或文件。
fn handoff_intent_store_for_read(
    state: &AppState,
) -> Result<Option<JsonHandoffIntentStore>, String> {
    let root = match state.runtime_mode {
        RuntimeMode::Fixture => {
            let path = recovery_root_path(state)?;
            if !path.exists() {
                return Ok(None);
            }
            path.to_path_buf()
        }
        RuntimeMode::RealReadPreview => state
            .production_catalog
            .as_ref()
            .ok_or_else(|| "handoff_intent_unavailable".to_string())?
            .recovery_root
            .clone(),
    };
    Ok(Some(JsonHandoffIntentStore::new(root)))
}

fn load_handoff_intent_for_state(state: &AppState) -> Result<Option<HandoffIntent>, String> {
    let Some(store) = handoff_intent_store_for_read(state)? else {
        return Ok(None);
    };
    store.load()
}

/// 返回固定恢复区中的独立凭证库。凭证库与目录库使用不同命名空间，
/// 其状态不进入目录库、同步计划或操作 manifest。
fn credential_vault_for_state(state: &AppState) -> Result<CredentialVault, String> {
    let recovery_root = match state.runtime_mode {
        RuntimeMode::Fixture => PathBuf::from(&state.recovery_root),
        RuntimeMode::RealReadPreview => state
            .production_catalog
            .as_ref()
            .ok_or_else(|| "credential_vault_unavailable".to_string())?
            .recovery_root
            .clone(),
    };
    if recovery_root.as_os_str().is_empty() {
        return Err("credential_vault_unavailable".to_string());
    }
    Ok(CredentialVault::new(recovery_root.join("credential-vault")))
}

/// 启动时只恢复凭证切换的最小上下文，不恢复旧同步计划或任何历史正文。
///
/// 扫描 manifest 与承接意图前先取得固定恢复区/数据位置租约；发现冲突时保持
/// pending journal，不自动重放或覆盖 `storage.json`。
fn recover_managed_account_runtime_from_disk(
    recovery_root: &Path,
) -> Result<ManagedAccountRuntimeSlot, String> {
    let mut empty = ManagedAccountRuntimeSlot::default();
    if !recovery_root.is_dir() {
        return Ok(empty);
    }

    let vault = CredentialVault::new(recovery_root.join("credential-vault"));
    let intent_store = JsonHandoffIntentStore::new(recovery_root);
    let mut records = vault
        .pending_recoveries(recovery_root)
        .map_err(credential_error_text)?;
    let mut intent = intent_store.load()?;
    let has_active_intent = intent.as_ref().is_some_and(|value| {
        matches!(
            value.state,
            HandoffIntentState::Prepared
                | HandoffIntentState::Switching
                | HandoffIntentState::TargetVerified
                | HandoffIntentState::PreviewReady
                | HandoffIntentState::ManualRecoveryRequired
        )
    });
    if records.is_empty() && !has_active_intent {
        return Ok(empty);
    }

    let lease_location_id = intent
        .as_ref()
        .map(|value| value.data_location_id.clone())
        .or_else(|| records.first().map(|value| value.data_location_id.clone()))
        .ok_or_else(|| "credential_recovery_binding_missing".to_string())?;
    let lease = OperationLease::acquire_shared(recovery_root, &lease_location_id)
        .map_err(|error| format!("operation_lease_unavailable:{error}"))?;

    // 租约内重新读取，避免启动扫描与另一实例的 manifest 发布交错。
    records = vault
        .pending_recoveries(recovery_root)
        .map_err(credential_error_text)?;
    intent = intent_store.load()?;
    if records.len() > 1 {
        drop(lease);
        return Err("credential_multiple_recoveries".to_string());
    }

    // journal 已经收口时，pending_recoveries 不再返回记录；仍需保留承接意图，
    // 否则重启后账号中心会丢失“目标已复核、等待 fresh 预览”的状态。
    if records.is_empty() {
        if let Some(current_intent) = intent.as_ref() {
            if matches!(
                current_intent.state,
                HandoffIntentState::Prepared
                    | HandoffIntentState::Switching
                    | HandoffIntentState::TargetVerified
                    | HandoffIntentState::PreviewReady
                    | HandoffIntentState::ManualRecoveryRequired
            ) {
                let manual_recovery =
                    current_intent.state == HandoffIntentState::ManualRecoveryRequired;
                let state = match current_intent.state {
                    HandoffIntentState::Prepared => traesync_domain::AccountSwitchState::Preflight,
                    HandoffIntentState::ManualRecoveryRequired => {
                        traesync_domain::AccountSwitchState::ManualRecoveryRequired
                    }
                    _ => traesync_domain::AccountSwitchState::Verifying,
                };
                let reason = if manual_recovery {
                    "credential_manual_recovery_required"
                } else {
                    "credential_recovery_target_verified"
                }
                .to_string();
                empty.runtime.current_account = if manual_recovery {
                    CurrentAccountEvidence {
                        verification_state: AccountVerificationState::ManualRecoveryRequired,
                        reason: Some(reason.clone()),
                        ..CurrentAccountEvidence::default()
                    }
                } else {
                    CurrentAccountEvidence::default()
                };
                empty.runtime.switch_plan = Some(AccountSwitchPlan {
                    plan_id: current_intent.intent_id.clone(),
                    source_profile_id: current_intent.source_profile_id.clone(),
                    target_profile_id: current_intent.target_profile_id.clone(),
                    source_data_location_id: None,
                    target_data_location_id: current_intent.data_location_id.clone(),
                    preflight: AccountSwitchPreflight {
                        source_verified: true,
                        target_known: true,
                        target_verified: current_intent.state != HandoffIntentState::Prepared,
                        target_location_known: true,
                        pending_sync_plan_cleared: true,
                        trae_closed: true,
                        ready: !manual_recovery,
                        reason: Some(reason.clone()),
                    },
                    backup_reference: current_intent.credential_operation_id.clone(),
                    expected_target_fingerprint: String::new(),
                    state,
                    created_at: current_intent.created_at,
                    failure_reason: Some(reason),
                });
            }
        }
        drop(lease);
        return Ok(empty);
    }

    if let (Some(record), Some(current_intent)) = (records.first(), intent.as_mut()) {
        if let Some(manifest_intent_id) = record.intent_id.as_deref() {
            if manifest_intent_id != current_intent.intent_id {
                drop(lease);
                return Err("handoff_intent_conflict".to_string());
            }
        }
        if current_intent.target_profile_id != record.profile_id
            || current_intent.data_location_id != record.data_location_id
        {
            drop(lease);
            return Err("credential_binding_mismatch".to_string());
        }
        if current_intent.state == HandoffIntentState::Prepared {
            // 写入已完成但进程可能在绑定步骤崩溃；在租约内补齐唯一非敏感句柄。
            current_intent.state = if record.state == "manual_recovery_required" {
                HandoffIntentState::ManualRecoveryRequired
            } else {
                HandoffIntentState::Switching
            };
            current_intent.credential_operation_id = Some(record.operation_id.clone());
            current_intent.updated_at = std::time::SystemTime::now();
            intent_store.publish(current_intent)?;
        }
    }

    if let Some(record) = records.first() {
        let active_intent = intent.as_ref();
        let target_profile_id = active_intent
            .map(|value| value.target_profile_id.clone())
            .unwrap_or_else(|| record.profile_id.clone());
        let source_profile_id = active_intent.and_then(|value| value.source_profile_id.clone());
        let target_data_location_id = active_intent
            .map(|value| value.data_location_id.clone())
            .unwrap_or_else(|| record.data_location_id.clone());
        let manual_recovery = record.state == "manual_recovery_required"
            || active_intent
                .is_some_and(|value| value.state == HandoffIntentState::ManualRecoveryRequired);
        let plan_id = active_intent
            .map(|value| value.intent_id.clone())
            .unwrap_or_else(|| format!("credential-recovery-{}", record.operation_id));
        let now = std::time::SystemTime::now();
        let state = if manual_recovery {
            traesync_domain::AccountSwitchState::ManualRecoveryRequired
        } else {
            traesync_domain::AccountSwitchState::Applying
        };
        let reason = if manual_recovery {
            Some("credential_manual_recovery_required".to_string())
        } else {
            Some("credential_recovery_pending_refresh".to_string())
        };
        empty.runtime.current_account = if manual_recovery {
            CurrentAccountEvidence {
                verification_state: AccountVerificationState::ManualRecoveryRequired,
                reason: reason.clone(),
                ..CurrentAccountEvidence::default()
            }
        } else {
            CurrentAccountEvidence::default()
        };
        empty.runtime.switch_plan = Some(AccountSwitchPlan {
            plan_id,
            source_profile_id,
            target_profile_id,
            source_data_location_id: None,
            target_data_location_id,
            preflight: AccountSwitchPreflight {
                source_verified: true,
                target_known: true,
                target_verified: true,
                target_location_known: true,
                pending_sync_plan_cleared: true,
                trae_closed: true,
                // 这是恢复上下文，不是新计划；fresh 账号证据仍由 refresh 再次验证。
                ready: true,
                reason: reason.clone(),
            },
            backup_reference: Some(record.operation_id.clone()),
            expected_target_fingerprint: record.user_fingerprint.clone(),
            state,
            created_at: active_intent.map(|value| value.created_at).unwrap_or(now),
            failure_reason: reason,
        });
    }
    drop(lease);
    Ok(empty)
}

/// 恢复区损坏或并发冲突时，保留一个不可执行的人工恢复计划，
/// 让 UI 明确显示阻断，而不是把错误吞掉后误报为空闲状态。
fn recovery_error_runtime(recovery_root: &Path, reason: &str) -> ManagedAccountRuntimeSlot {
    let intent = JsonHandoffIntentStore::new(recovery_root)
        .load()
        .ok()
        .flatten();
    let target_profile_id = intent
        .as_ref()
        .map(|value| value.target_profile_id.clone())
        .unwrap_or_default();
    let data_location_id = intent
        .as_ref()
        .map(|value| value.data_location_id.clone())
        .unwrap_or_default();
    let source_profile_id = intent
        .as_ref()
        .and_then(|value| value.source_profile_id.clone());
    let plan_id = intent
        .as_ref()
        .map(|value| value.intent_id.clone())
        .unwrap_or_else(|| "credential-recovery-error".to_string());
    let created_at = intent
        .as_ref()
        .map(|value| value.created_at)
        .unwrap_or_else(std::time::SystemTime::now);
    let failure_reason = Some(format!("credential_recovery_failed:{reason}"));
    ManagedAccountRuntimeSlot {
        runtime: ManagedAccountRuntime {
            profiles: Vec::new(),
            current_account: CurrentAccountEvidence {
                verification_state: AccountVerificationState::ManualRecoveryRequired,
                reason: failure_reason.clone(),
                ..CurrentAccountEvidence::default()
            },
            switch_plan: Some(AccountSwitchPlan {
                plan_id,
                source_profile_id,
                target_profile_id,
                source_data_location_id: None,
                target_data_location_id: data_location_id,
                preflight: AccountSwitchPreflight {
                    source_verified: false,
                    target_known: false,
                    target_verified: false,
                    target_location_known: false,
                    pending_sync_plan_cleared: false,
                    trae_closed: false,
                    ready: false,
                    reason: failure_reason.clone(),
                },
                backup_reference: None,
                expected_target_fingerprint: String::new(),
                state: traesync_domain::AccountSwitchState::ManualRecoveryRequired,
                created_at,
                failure_reason,
            }),
        },
        authorization_generation: None,
    }
}

fn credential_status_wire(status: CredentialStatus) -> ManagedCredentialStatusWireDto {
    ManagedCredentialStatusWireDto {
        profile_id: status.profile_id,
        state: status.state,
        data_location_id: status.data_location_id,
        format_version: status.format_version,
    }
}

/// 本机凭证保存/切换的独立 Gate。检查必须发生在任何授权、vault 或目标文件访问之前。
fn ensure_local_credential_switch_enabled(state: &AppState) -> Result<(), String> {
    if state.capabilities.local_credential_switch {
        Ok(())
    } else {
        Err("gate_not_qualified".to_string())
    }
}

fn profile_credential_binding(profile: &AccountProfile) -> CredentialBinding {
    CredentialBinding::new(
        profile.profile_id.clone(),
        profile.data_location_id.clone(),
        profile.user_fingerprint.clone(),
    )
}

fn managed_credential_statuses_inner(
    state: &AppState,
) -> Result<Vec<ManagedCredentialStatusWireDto>, String> {
    let Some(store) = managed_account_store_for_read(state)? else {
        return Ok(Vec::new());
    };
    let mut runtime = ManagedAccountRuntime::default();
    ManagedAccountSwitchService::new(&store)
        .load_profiles(&mut runtime)
        .map_err(|error| error.to_string())?;
    let vault = credential_vault_for_state(state)?;
    Ok(runtime
        .profiles
        .iter()
        .map(|profile| credential_status_wire(vault.status(&profile_credential_binding(profile))))
        .collect())
}

/// 计算一次已授权数据位置的稳定路径与进程观测，不执行任何写入。
fn authorized_storage_context(
    state: &AppState,
    generation: u64,
    authorization: &AuthorizationState,
) -> Result<(PathBuf, String, String), String> {
    let (data_root, db_path, _storage_root) =
        resolve_authorized_read_target_for_context(state, generation, authorization)?;
    let db_relative_path = db_path
        .strip_prefix(&data_root)
        .map_err(|_| "data_location_changed".to_string())?
        .to_string_lossy()
        .into_owned();
    let location = capture_location_identity(
        &PlatformFileIdentityProvider::new(),
        &data_root,
        &db_relative_path,
    )
    .map_err(|error| error.to_string())?;
    let process =
        state
            .process_controller
            .observe(&location.data_location_id, &data_root, &db_relative_path);
    if let Some(error_code) = process.error_code() {
        return Err(error_code.to_string());
    }
    Ok((data_root, location.data_location_id, db_relative_path))
}

fn credential_recovery_root_for_context(
    state: &AppState,
    data_root: &Path,
) -> Result<PathBuf, String> {
    match state.runtime_mode {
        RuntimeMode::Fixture => {
            let guard = FixturePathGuard::new(data_root).map_err(fixture_path_command_error)?;
            guard
                .validate_shared_recovery_root(recovery_root_path(state)?)
                .map_err(fixture_path_command_error)
        }
        RuntimeMode::RealReadPreview => state
            .production_catalog
            .as_ref()
            .ok_or_else(|| "credential_vault_unavailable".to_string())
            .map(|runtime| runtime.recovery_root.clone()),
    }
}

fn update_handoff_intent_state(
    state: &AppState,
    next_state: HandoffIntentState,
    failure_reason: Option<String>,
) -> Result<(), String> {
    let Some(store) = handoff_intent_store_for_read(state)? else {
        return Ok(());
    };
    let Some(mut intent) = store.load()? else {
        return Ok(());
    };
    if intent.state != next_state && !handoff_state_transition_allowed(intent.state, next_state) {
        return Err("handoff_intent_state_transition_invalid".to_string());
    }
    let recovery_root = store
        .path()
        .parent()
        .ok_or_else(|| "handoff_intent_unavailable".to_string())?;
    let lease = OperationLease::acquire_shared(recovery_root, &intent.data_location_id)
        .map_err(|error| format!("operation_lease_unavailable:{error}"))?;
    // 取得租约后再次读取，避免两个实例基于同一旧状态互相覆盖。
    let current = store
        .load()?
        .ok_or_else(|| "handoff_intent_conflict".to_string())?;
    if current.intent_id != intent.intent_id
        || current.updated_at != intent.updated_at
        || current.state != intent.state
    {
        drop(lease);
        return Err("handoff_intent_conflict".to_string());
    }
    intent.state = next_state;
    intent.updated_at = std::time::SystemTime::now();
    intent.failure_reason = failure_reason;
    store.publish(&intent)?;
    drop(lease);
    Ok(())
}

fn handoff_state_transition_allowed(current: HandoffIntentState, next: HandoffIntentState) -> bool {
    matches!(
        (current, next),
        (HandoffIntentState::Prepared, HandoffIntentState::Switching)
            | (HandoffIntentState::Prepared, HandoffIntentState::Expired)
            | (
                HandoffIntentState::Prepared,
                HandoffIntentState::ManualRecoveryRequired
            )
            | (
                HandoffIntentState::Switching,
                HandoffIntentState::TargetVerified
            )
            | (HandoffIntentState::Switching, HandoffIntentState::Expired)
            | (
                HandoffIntentState::Switching,
                HandoffIntentState::ManualRecoveryRequired
            )
            | (
                HandoffIntentState::TargetVerified,
                HandoffIntentState::PreviewReady
            )
            | (
                HandoffIntentState::TargetVerified,
                HandoffIntentState::Expired
            )
            | (
                HandoffIntentState::TargetVerified,
                HandoffIntentState::ManualRecoveryRequired
            )
            | (
                HandoffIntentState::PreviewReady,
                HandoffIntentState::Expired
            )
            | (
                HandoffIntentState::PreviewReady,
                HandoffIntentState::ManualRecoveryRequired
            )
            | (
                HandoffIntentState::ManualRecoveryRequired,
                HandoffIntentState::Expired
            )
    )
}

/// 记录账号切换失败；失败状态必须同时落入内存计划与持久承接意图。
///
/// 这里采用尽力更新策略：原始切换错误仍返回给调用方，状态持久化失败不会
/// 覆盖更有价值的首个错误，但会保留已有恢复现场供下次启动协调。
fn mark_account_switch_failure(state: &AppState, target_profile_id: &str, reason: &str) {
    if let Ok(Some(store)) = managed_account_store_for_read(state) {
        let service = ManagedAccountSwitchService::new(&store);
        let mut runtime_slot = state.managed_account_runtime.lock().unwrap();
        let should_mark = runtime_slot
            .runtime
            .switch_plan
            .as_ref()
            .is_some_and(|plan| plan.target_profile_id == target_profile_id);
        if should_mark {
            if let Some(plan_id) = runtime_slot
                .runtime
                .switch_plan
                .as_ref()
                .map(|plan| plan.plan_id.clone())
            {
                let _ = service.mark_manual_recovery_required(
                    &mut runtime_slot.runtime,
                    &plan_id,
                    reason,
                );
            }
        }
    }
    if load_handoff_intent_for_state(state)
        .ok()
        .flatten()
        .is_some_and(|intent| intent.target_profile_id == target_profile_id)
    {
        let _ = update_handoff_intent_state(
            state,
            HandoffIntentState::ManualRecoveryRequired,
            Some(reason.to_string()),
        );
    }
}

fn create_handoff_intent_inner(
    state: &AppState,
    target_profile_id: &str,
    requested_scope: Option<SyncScope>,
) -> Result<HandoffIntent, String> {
    let (generation, authorization) = capture_authorized_read_context(state)?;
    let store = managed_account_store_for_read(state)?
        .ok_or_else(|| "account_profile_store_unavailable".to_string())?;
    let mut runtime = ManagedAccountRuntime::default();
    ManagedAccountSwitchService::new(&store)
        .load_profiles(&mut runtime)
        .map_err(|error| error.to_string())?;
    let source = {
        let runtime_slot = state.managed_account_runtime.lock().unwrap();
        if runtime_slot.authorization_generation != Some(generation) {
            return Err("source_account_not_verified".to_string());
        }
        runtime_slot.runtime.current_account.clone()
    };
    if source.verification_state != AccountVerificationState::Verified
        || source.data_location_id.is_none()
    {
        return Err("source_account_not_verified".to_string());
    }
    let target = runtime
        .profiles
        .iter()
        .find(|profile| profile.profile_id == target_profile_id)
        .ok_or_else(|| "target_profile_not_found".to_string())?;
    if target.verification_state != AccountVerificationState::Verified {
        return Err("target_account_not_verified".to_string());
    }
    if target.profile_id == source.profile_id.clone().unwrap_or_default() {
        return Err("target_account_is_current".to_string());
    }
    if target.data_location_id != source.data_location_id.clone().unwrap_or_default() {
        return Err("target_data_location_mismatch".to_string());
    }
    let pending = state
        .pending_sync_plan
        .lock()
        .unwrap()
        .clone()
        .ok_or_else(|| "handoff_fresh_plan_required".to_string())?;
    if pending.generation != generation {
        return Err("handoff_fresh_plan_required".to_string());
    }
    let scope = requested_scope.unwrap_or_else(|| pending.plan.scope_snapshot().clone());
    if scope != *pending.plan.scope_snapshot() {
        return Err("handoff_scope_mismatch".to_string());
    }
    let (_data_root, _db_path, storage_root) =
        resolve_authorized_read_target_for_context(state, generation, &authorization)?;
    let catalog_generation =
        resolve_current_catalog_generation_id(&storage_root).map_err(|error| error.to_string())?;
    ensure_authorization_context_current(state, generation, &authorization)?;
    let now = std::time::SystemTime::now();
    let intent = HandoffIntent {
        intent_id: OperationId::new().as_str().to_string(),
        source_profile_id: source.profile_id,
        target_profile_id: target.profile_id.clone(),
        data_location_id: target.data_location_id.clone(),
        scope,
        catalog_id: state
            .production_catalog
            .as_ref()
            .map(|runtime| runtime.catalog_id.clone()),
        catalog_generation: Some(catalog_generation),
        schema_version: Some(pending.plan.schema_fingerprint().to_string()),
        mapping_version: Some(pending.plan.mapping_version().to_string()),
        credential_operation_id: None,
        state: HandoffIntentState::Prepared,
        created_at: now,
        updated_at: now,
        failure_reason: None,
    };
    publish_handoff_intent_for_state(state, &intent)?;
    Ok(intent)
}

/// 发布前置承接意图；恢复区必须已由固定边界检查准备完成。
fn publish_handoff_intent_for_state(
    state: &AppState,
    intent: &HandoffIntent,
) -> Result<(), String> {
    let Some(store) = handoff_intent_store_for_read(state)? else {
        return Err("handoff_intent_unavailable".to_string());
    };
    let recovery_root = store
        .path()
        .parent()
        .ok_or_else(|| "handoff_intent_unavailable".to_string())?;
    let lease = OperationLease::acquire_shared(recovery_root, &intent.data_location_id)
        .map_err(|error| format!("operation_lease_unavailable:{error}"))?;
    let publish_result = store.publish(intent);
    drop(lease);
    publish_result?;
    Ok(())
}

/// 将一次已完成凭证替换绑定到承接意图；只写非敏感 operation id。
fn bind_handoff_credential_operation(state: &AppState, operation_id: &str) -> Result<(), String> {
    let Some(store) = handoff_intent_store_for_read(state)? else {
        return Err("handoff_intent_unavailable".to_string());
    };
    let Some(mut intent) = store.load()? else {
        return Err("handoff_intent_unavailable".to_string());
    };
    let recovery_root = store
        .path()
        .parent()
        .ok_or_else(|| "handoff_intent_unavailable".to_string())?;
    let lease = OperationLease::acquire_shared(recovery_root, &intent.data_location_id)
        .map_err(|error| format!("operation_lease_unavailable:{error}"))?;
    let current = store
        .load()?
        .ok_or_else(|| "handoff_intent_conflict".to_string())?;
    if current.intent_id != intent.intent_id || current.updated_at != intent.updated_at {
        drop(lease);
        return Err("handoff_intent_conflict".to_string());
    }
    intent.credential_operation_id = Some(operation_id.to_string());
    intent.updated_at = std::time::SystemTime::now();
    let result = store.publish(&intent);
    drop(lease);
    result
}

/// 账号刷新先完成位置与账号证据复核，再创建或加载账号摘要 salt。
fn managed_account_store_for_refresh(
    state: &AppState,
    generation: u64,
    authorization: &AuthorizationState,
    observation: &ManagedAccountObservation,
) -> Result<(JsonManagedAccountProfileStore, Option<Vec<u8>>), String> {
    let profile_path = observation.storage_root.join("managed-accounts.json");
    let recovery_root = match state.runtime_mode {
        RuntimeMode::Fixture => FixturePathGuard::new(&observation.data_root)
            .map_err(fixture_path_command_error)?
            .validate_shared_recovery_root(recovery_root_path(state)?)
            .map_err(fixture_path_command_error)?,
        RuntimeMode::RealReadPreview => {
            let runtime = state
                .production_catalog
                .as_ref()
                .ok_or_else(|| "account_profile_store_unavailable".to_string())?;
            if runtime.storage_root != observation.storage_root {
                return Err("authorization_mismatch".to_string());
            }
            runtime.recovery_root.clone()
        }
    };
    let salt_path = recovery_root.join("managed-account-fingerprint-salt.bin");
    let existing_salt = load_account_fingerprint_salt(&salt_path)?;

    if observation.account.evidence_state != EvidenceState::Verified {
        let store = JsonManagedAccountProfileStore::for_read(profile_path, existing_salt.clone())?;
        return Ok((store, existing_salt));
    }

    let fingerprint_salt = match existing_salt {
        Some(salt) => salt,
        None => {
            // v3 或未绑定 v2 档案会在这里失败关闭；只有空库或全 v1 legacy 可首次建 salt。
            let legacy_store = JsonManagedAccountProfileStore::for_read(&profile_path, None)?;
            let mut legacy_runtime = ManagedAccountRuntime::default();
            ManagedAccountSwitchService::new(&legacy_store)
                .load_profiles(&mut legacy_runtime)
                .map_err(|error| error.to_string())?;
            ensure_authorization_context_current(state, generation, authorization)?;
            if state.runtime_mode == RuntimeMode::Fixture {
                let guard = FixturePathGuard::new(&observation.data_root)
                    .map_err(fixture_path_command_error)?;
                let prepared = prepare_recovery_root(state, &guard)?;
                if prepared != recovery_root {
                    return Err("account_fingerprint_salt_unavailable".to_string());
                }
            }
            let salt = load_or_create_account_fingerprint_salt(&salt_path)?;
            ensure_authorization_context_current(state, generation, authorization)?;
            salt
        }
    };
    let store = JsonManagedAccountProfileStore::with_fingerprint_salt(
        profile_path,
        fingerprint_salt.clone(),
    )?;
    Ok((store, Some(fingerprint_salt)))
}

fn detect_managed_current_account(
    state: &AppState,
    generation: u64,
    authorization: &AuthorizationState,
) -> Result<ManagedAccountObservation, String> {
    let now = std::time::SystemTime::now();
    let (data_root, db_path, storage_root) =
        resolve_authorized_read_target_for_context(state, generation, authorization)?;
    let account = state
        .workbench_reader
        .read_account_evidence(&data_root, now);
    if account.evidence_state == EvidenceState::Verified && account.user_id.is_none() {
        return Err("account_evidence_unavailable".to_string());
    }
    let data_location_id = match state.runtime_mode {
        RuntimeMode::Fixture => {
            let db_relative_path = db_path
                .strip_prefix(&data_root)
                .map_err(|_| "data_location_changed".to_string())?
                .to_string_lossy()
                .into_owned();
            let witness = capture_location_witness(
                &PlatformFileIdentityProvider::new(),
                &data_root,
                &db_relative_path,
            )
            .map_err(|error| error.to_string())?;
            witness.data_location_id
        }
        RuntimeMode::RealReadPreview => {
            let (witness, user_fingerprint, auth_fingerprint) = {
                let slot = state.authorization.lock().unwrap();
                if slot.generation != generation || slot.state != *authorization {
                    return Err("authorization_mismatch".to_string());
                }
                (
                    slot.real_location_witness.clone(),
                    slot.real_user_fingerprint.clone(),
                    slot.real_auth_fingerprint.clone(),
                )
            };
            let witness = witness.ok_or_else(|| "authorization_mismatch".to_string())?;
            let user_fingerprint =
                user_fingerprint.ok_or_else(|| "authorization_mismatch".to_string())?;
            let auth_fingerprint =
                auth_fingerprint.ok_or_else(|| "authorization_mismatch".to_string())?;
            let current_user_fingerprint =
                account.user_id.as_ref().map(user_id_binding_fingerprint);
            let current_auth_fingerprint = account
                .auth_fingerprint
                .as_ref()
                .map(|value| value.0.clone());
            if account.evidence_state != EvidenceState::Verified
                || current_user_fingerprint.as_deref() != Some(user_fingerprint.as_str())
                || current_auth_fingerprint.as_deref() != Some(auth_fingerprint.as_str())
            {
                return Err("account_evidence_changed".to_string());
            }
            witness.data_location_id
        }
    };
    ensure_authorization_context_current(state, generation, authorization)?;
    Ok(ManagedAccountObservation {
        account,
        data_location_id,
        data_root,
        storage_root,
        observed_at: now,
    })
}

fn managed_account_view_inner(state: &AppState) -> Result<ManagedAccountsWireView, String> {
    // 测试夹具和热重载路径可能绕过 `run`；首次读取时再做一次幂等恢复扫描。
    let needs_recovery_scan = state
        .managed_account_runtime
        .lock()
        .unwrap()
        .runtime
        .switch_plan
        .is_none();
    if needs_recovery_scan && !state.recovery_root.is_empty() {
        let recovered =
            match recover_managed_account_runtime_from_disk(Path::new(&state.recovery_root)) {
                Ok(recovered) => recovered,
                Err(error) => recovery_error_runtime(Path::new(&state.recovery_root), &error),
            };
        let mut runtime_slot = state.managed_account_runtime.lock().unwrap();
        if runtime_slot.runtime.switch_plan.is_none() {
            *runtime_slot = recovered;
        }
    }
    let mut profiles_runtime = ManagedAccountRuntime::default();
    if let Some(store) = managed_account_store_for_read(state)? {
        ManagedAccountSwitchService::new(&store)
            .load_profiles(&mut profiles_runtime)
            .map_err(|error| error.to_string())?;
    }
    let handoff_intent = match load_handoff_intent_for_state(state) {
        Ok(intent) => intent,
        Err(error) => {
            let recovery_visible = state
                .managed_account_runtime
                .lock()
                .unwrap()
                .runtime
                .switch_plan
                .as_ref()
                .is_some_and(|plan| {
                    plan.state == traesync_domain::AccountSwitchState::ManualRecoveryRequired
                });
            if recovery_visible {
                None
            } else {
                return Err(error);
            }
        }
    };

    // 固定锁序：authorization -> runtime。投影期间不执行文件 I/O。
    let authorization = state.authorization.lock().unwrap();
    let mut runtime_slot = state.managed_account_runtime.lock().unwrap();
    runtime_slot.runtime.profiles = profiles_runtime.profiles;
    let context_current = matches!(authorization.state, AuthorizationState::Authorized { .. })
        && runtime_slot.authorization_generation == Some(authorization.generation);
    Ok(managed_accounts_wire_view(
        &runtime_slot.runtime,
        context_current,
        handoff_intent.as_ref(),
    ))
}

/// 返回独立账号中心状态。该命令不读取认证正文，也不执行账号切换。
#[tauri::command]
fn get_managed_account_state(
    state: tauri::State<AppState>,
) -> Result<ManagedAccountsWireView, String> {
    managed_account_view_inner(&state)
}

/// 返回每个已保存账号的凭证状态；不读取明文登录材料。
#[tauri::command]
fn get_managed_credential_statuses(
    state: tauri::State<AppState>,
) -> Result<Vec<ManagedCredentialStatusWireDto>, String> {
    managed_credential_statuses_inner(&state)
}

/// 保存当前已验证账号的登录命名空间。
///
/// 只在后端确认 TRAE 未运行、数据位置仍是同一见证时读取 `storage.json`；
/// 认证值进入当前 Windows 用户 DPAPI 凭证库，前端只收到状态。
#[tauri::command]
fn capture_current_account_credential(
    state: tauri::State<AppState>,
) -> Result<ManagedCredentialStatusWireDto, String> {
    capture_current_account_credential_inner(&state)
}

fn capture_current_account_credential_inner(
    state: &AppState,
) -> Result<ManagedCredentialStatusWireDto, String> {
    ensure_local_credential_switch_enabled(state)?;
    let (generation, authorization) = capture_authorized_read_context(state)?;
    let (data_root, data_location_id, db_relative_path) =
        authorized_storage_context(state, generation, &authorization)?;
    let observation = detect_managed_current_account(state, generation, &authorization)?;
    if observation.data_location_id != data_location_id
        || observation.account.evidence_state != EvidenceState::Verified
    {
        return Err("account_evidence_changed".to_string());
    }
    let (store, fingerprint_salt) =
        managed_account_store_for_refresh(state, generation, &authorization, &observation)?;
    let evidence = managed_current_evidence(
        &observation.account,
        observation.data_location_id.clone(),
        observation.observed_at,
        fingerprint_salt.as_deref(),
    );
    let profile_id = evidence
        .profile_id
        .clone()
        .ok_or_else(|| "account_evidence_unavailable".to_string())?;
    let user_fingerprint = evidence
        .user_fingerprint
        .clone()
        .ok_or_else(|| "account_evidence_unavailable".to_string())?;
    let binding = CredentialBinding::new(
        profile_id,
        observation.data_location_id.clone(),
        user_fingerprint,
    );
    let storage_path =
        CredentialVault::locate_storage_json(&data_root).map_err(credential_error_text)?;
    let vault = credential_vault_for_state(state)?;
    let recovery_root = credential_recovery_root_for_context(state, &data_root)?;
    let lease =
        acquire_credential_switch_lease(state, &data_root, &recovery_root, &data_location_id)?;
    // 捕获前再次确认 TRAE 仍关闭，避免账号材料读取与外部写入交错。
    let process =
        state
            .process_controller
            .observe(&data_location_id, &data_root, &db_relative_path);
    if let Some(error_code) = process.error_code() {
        drop(lease);
        return Err(error_code.to_string());
    }
    let status = match vault.capture(&binding, &storage_path) {
        Ok(status) => status,
        Err(error) => {
            drop(lease);
            return Err(credential_error_text(error));
        }
    };
    if let Err(error) = ensure_authorization_context_current(state, generation, &authorization) {
        drop(lease);
        return Err(error);
    }
    drop(lease);
    let _ = store;
    Ok(credential_status_wire(status))
}

/// 若目标账号已有 DPAPI 凭证，则执行受控认证命名空间替换；
/// 未保存凭证时返回 `false`，调用方可继续走外部账号管理器等待态。
fn try_apply_saved_credential(
    state: &AppState,
    target_profile_id: &str,
    bind_handoff_intent: bool,
) -> Result<Option<CredentialApplyOutcome>, String> {
    ensure_local_credential_switch_enabled(state)?;
    let (generation, authorization) = capture_authorized_read_context(state)?;
    let (data_root, data_location_id, db_relative_path) =
        authorized_storage_context(state, generation, &authorization)?;
    let store = managed_account_store_for_read(state)?
        .ok_or_else(|| "account_profile_store_unavailable".to_string())?;
    let mut runtime = ManagedAccountRuntime::default();
    ManagedAccountSwitchService::new(&store)
        .load_profiles(&mut runtime)
        .map_err(|error| error.to_string())?;
    let target = runtime
        .profiles
        .iter()
        .find(|profile| profile.profile_id == target_profile_id)
        .ok_or_else(|| "target_profile_not_found".to_string())?;
    if target.data_location_id != data_location_id {
        return Err("target_data_location_mismatch".to_string());
    }
    let binding = profile_credential_binding(target);
    let vault = credential_vault_for_state(state)?;
    let status = vault.status(&binding);
    match status.state {
        CredentialState::Missing => Ok(None),
        CredentialState::Stale => Err("credential_stale".to_string()),
        CredentialState::Invalid => Err("credential_invalid".to_string()),
        CredentialState::Saved => {
            let intent_id = if bind_handoff_intent {
                load_handoff_intent_for_state(state)?.and_then(|intent| {
                    if intent.target_profile_id == target_profile_id
                        && intent.state == HandoffIntentState::Prepared
                    {
                        Some(intent.intent_id)
                    } else {
                        None
                    }
                })
            } else {
                None
            };
            let storage_path =
                CredentialVault::locate_storage_json(&data_root).map_err(credential_error_text)?;
            let recovery_root = credential_recovery_root_for_context(state, &data_root)?;
            let lease = acquire_credential_switch_lease(
                state,
                &data_root,
                &recovery_root,
                &data_location_id,
            )?;
            // 取得共享租约后再次确认进程仍关闭，避免预检与写入之间发生 TOCTOU。
            let process =
                state
                    .process_controller
                    .observe(&data_location_id, &data_root, &db_relative_path);
            if let Some(error_code) = process.error_code() {
                drop(lease);
                return Err(error_code.to_string());
            }
            let outcome = vault
                .apply_with_intent(
                    &binding,
                    &storage_path,
                    &recovery_root,
                    true,
                    intent_id.as_deref(),
                )
                .map_err(credential_error_text)?;
            // 写后再次确认 TRAE 没有在替换窗口启动；无法确认时保留恢复现场并停止。
            let process =
                state
                    .process_controller
                    .observe(&data_location_id, &data_root, &db_relative_path);
            if let Some(error_code) = process.error_code() {
                let _ = vault.mark_manual_recovery_required_with_binding(
                    &recovery_root,
                    &outcome.operation_id,
                    &binding,
                    intent_id.as_deref(),
                );
                drop(lease);
                return Err(error_code.to_string());
            }
            if let Err(error) =
                ensure_authorization_context_current(state, generation, &authorization)
            {
                let _ = vault.mark_manual_recovery_required_with_binding(
                    &recovery_root,
                    &outcome.operation_id,
                    &binding,
                    intent_id.as_deref(),
                );
                drop(lease);
                return Err(error);
            }
            drop(lease);
            Ok(Some(outcome))
        }
    }
}

/// 凭证替换与目录库/同步操作共享固定恢复区和数据位置租约。
fn acquire_credential_switch_lease(
    state: &AppState,
    data_root: &Path,
    recovery_root: &Path,
    data_location_id: &str,
) -> Result<OperationLease, String> {
    match state.runtime_mode {
        RuntimeMode::Fixture => {
            let guard = FixturePathGuard::new(data_root).map_err(fixture_path_command_error)?;
            guard
                .acquire_operation_lease(recovery_root, data_location_id)
                .map_err(fixture_path_command_error)
        }
        RuntimeMode::RealReadPreview => {
            OperationLease::acquire_shared(recovery_root, data_location_id)
                .map_err(|error| format!("operation_lease_unavailable:{error}"))
        }
    }
}

/// 凭证 journal 收口也复用同一数据位置租约，避免与下一次切换/扫描交错。
fn mark_credential_journal(
    state: &AppState,
    data_root: &Path,
    data_location_id: &str,
    operation_id: &str,
    target_verified: bool,
    binding: &CredentialBinding,
    intent_id: Option<&str>,
) -> Result<(), String> {
    let recovery_root = credential_recovery_root_for_context(state, data_root)?;
    let lease =
        acquire_credential_switch_lease(state, data_root, &recovery_root, data_location_id)?;
    let vault = credential_vault_for_state(state)?;
    let result = if target_verified {
        vault.mark_target_verified_with_binding(&recovery_root, operation_id, binding, intent_id)
    } else {
        vault.mark_manual_recovery_required_with_binding(
            &recovery_root,
            operation_id,
            binding,
            intent_id,
        )
    };
    drop(lease);
    result.map_err(credential_error_text)
}

/// 凭证已经替换后，若后续编排步骤失败，必须把 journal 收口到人工恢复态。
/// 该辅助只在授权上下文仍有效时尝试写入；无法再次证明位置时保持 fail-closed，
/// 不自动恢复或覆盖当前 storage.json。
fn mark_credential_operation_manual_recovery_for_state(
    state: &AppState,
    target_profile_id: &str,
    operation_id: &str,
) -> Result<(), String> {
    let (generation, authorization) = capture_authorized_read_context(state)?;
    let (data_root, data_location_id, _) =
        authorized_storage_context(state, generation, &authorization)?;
    let store = managed_account_store_for_read(state)?
        .ok_or_else(|| "account_profile_store_unavailable".to_string())?;
    let mut runtime = ManagedAccountRuntime::default();
    ManagedAccountSwitchService::new(&store)
        .load_profiles(&mut runtime)
        .map_err(|error| error.to_string())?;
    let target = runtime
        .profiles
        .iter()
        .find(|profile| profile.profile_id == target_profile_id)
        .ok_or_else(|| "target_profile_not_found".to_string())?;
    let binding = profile_credential_binding(target);
    let intent_id = load_handoff_intent_for_state(state)?.and_then(|intent| {
        (intent.target_profile_id == target_profile_id
            && intent.state != HandoffIntentState::Expired
            && intent.credential_operation_id.as_deref() == Some(operation_id))
        .then_some(intent.intent_id)
    });
    mark_credential_journal(
        state,
        &data_root,
        &data_location_id,
        operation_id,
        false,
        &binding,
        intent_id.as_deref(),
    )
}

/// 把凭证写入后的失败统一映射为人工恢复，避免 UI 把半完成切换显示成普通失败。
fn credential_post_apply_failure(
    state: &AppState,
    target_profile_id: &str,
    operation_id: Option<&str>,
    reason: &str,
) -> String {
    if let Some(operation_id) = operation_id {
        let _ = mark_credential_operation_manual_recovery_for_state(
            state,
            target_profile_id,
            operation_id,
        );
        mark_account_switch_failure(
            state,
            target_profile_id,
            "credential_manual_recovery_required",
        );
        return "credential_manual_recovery_required".to_string();
    }
    mark_account_switch_failure(state, target_profile_id, reason);
    reason.to_string()
}

/// 创建账号切换计划。该命令只清理旧的待执行同步计划并更新内存状态，
/// 不读取凭证、不改写 TRAE 文件，也不执行外部账号切换。
#[tauri::command]
fn prepare_managed_account_switch(
    target_profile_id: String,
    state: tauri::State<AppState>,
) -> Result<ManagedAccountsWireView, String> {
    prepare_managed_account_switch_inner(&state, &target_profile_id)
}

fn prepare_managed_account_switch_inner(
    state: &AppState,
    target_profile_id: &str,
) -> Result<ManagedAccountsWireView, String> {
    let (generation, authorization) = capture_authorized_read_context(state)?;
    let store = managed_account_store_for_read(state)?
        .ok_or_else(|| "account_profile_store_unavailable".to_string())?;

    let (current_account, current_generation) = {
        let runtime_slot = state.managed_account_runtime.lock().unwrap();
        (
            runtime_slot.runtime.current_account.clone(),
            runtime_slot.authorization_generation,
        )
    };
    if current_generation != Some(generation)
        || current_account.verification_state != AccountVerificationState::Verified
    {
        return Err("source_account_not_verified".to_string());
    }

    // 选择新的目标账号即视为用户明确放弃旧计划；不删除快照、备份或失败证据。
    // `take()` 的返回值表示“之前是否存在计划”，不是清理结果；清理动作完成后
    // 预检字段必须为 true，否则一个被正常清掉的旧计划会错误阻断新切换。
    state.pending_sync_plan.lock().unwrap().take();
    let pending_sync_plan_cleared = true;
    let mut candidate = ManagedAccountRuntime {
        profiles: Vec::new(),
        current_account,
        switch_plan: None,
    };
    let service = ManagedAccountSwitchService::new(&store);
    service
        .prepare_switch(
            &mut candidate,
            target_profile_id,
            AccountSwitchPreflight {
                source_verified: true,
                target_known: false,
                target_verified: false,
                target_location_known: false,
                pending_sync_plan_cleared,
                trae_closed: false,
                ready: false,
                reason: None,
            },
            OperationId::new().as_str().to_string(),
            std::time::SystemTime::now(),
        )
        .map_err(|error| error.to_string())?;

    ensure_authorization_context_current(state, generation, &authorization)?;
    let mut runtime_slot = state.managed_account_runtime.lock().unwrap();
    runtime_slot.runtime = candidate;
    runtime_slot.authorization_generation = Some(generation);
    let handoff_intent = load_handoff_intent_for_state(state)?;
    Ok(managed_accounts_wire_view(
        &runtime_slot.runtime,
        true,
        handoff_intent.as_ref(),
    ))
}

/// 进入外部账号切换等待态。
///
/// 目前不直接写 TRAE 登录材料；命令会撤销旧读取授权并保留等待计划，
/// 用户完成外部切换后重新授权/扫描即可由 refresh 命令完成目标复核。
#[tauri::command]
fn switch_account(
    target_profile_id: String,
    state: tauri::State<AppState>,
) -> Result<ManagedAccountsWireView, String> {
    ensure_local_credential_switch_enabled(&state)?;
    switch_account_inner(&state, &target_profile_id)
}

fn switch_account_inner(
    state: &AppState,
    target_profile_id: &str,
) -> Result<ManagedAccountsWireView, String> {
    switch_account_inner_with_intent(state, target_profile_id, true)
}

fn switch_account_inner_with_intent(
    state: &AppState,
    target_profile_id: &str,
    expire_handoff_intent: bool,
) -> Result<ManagedAccountsWireView, String> {
    ensure_local_credential_switch_enabled(state)?;
    let store = managed_account_store_for_read(state)?
        .ok_or_else(|| "account_profile_store_unavailable".to_string())?;
    let service = ManagedAccountSwitchService::new(&store);
    // 所有目标、计划和承接意图检查必须先于凭证文件副作用。
    let plan_id =
        validate_account_switch_request(state, target_profile_id, !expire_handoff_intent)?;
    // 已保存凭证时执行真实认证命名空间替换；缺少凭证则保留外部管理器等待态。
    let credential_outcome =
        match try_apply_saved_credential(state, target_profile_id, !expire_handoff_intent) {
            Ok(outcome) => outcome,
            Err(error) => {
                mark_account_switch_failure(state, target_profile_id, &error);
                return Err(error);
            }
        };
    let credential_operation_id = credential_outcome
        .as_ref()
        .map(|outcome| outcome.operation_id.clone());
    let mut runtime_error = None;
    {
        let mut runtime_slot = state.managed_account_runtime.lock().unwrap();
        let plan_matches = runtime_slot
            .runtime
            .switch_plan
            .as_ref()
            .is_some_and(|plan| {
                plan.target_profile_id == target_profile_id && plan.plan_id == plan_id
            });
        if !plan_matches {
            runtime_error = Some("account_switch_plan_mismatch".to_string());
        } else if let Some(outcome) = credential_outcome.as_ref() {
            // 恢复句柄只留在后端计划中，供后续目标复核完成 journal 收口。
            if let Some(plan) = runtime_slot.runtime.switch_plan.as_mut() {
                plan.backup_reference = Some(outcome.operation_id.clone());
            }
            if let Err(error) =
                service.mark_applying_after_credential(&mut runtime_slot.runtime, &plan_id)
            {
                runtime_error = Some(error.to_string());
            }
        } else {
            if let Err(error) =
                service.mark_waiting_for_trae_closed(&mut runtime_slot.runtime, &plan_id)
            {
                runtime_error = Some(error.to_string());
            }
        }
    }
    if let Some(reason) = runtime_error {
        let public_error = credential_post_apply_failure(
            state,
            target_profile_id,
            credential_operation_id.as_deref(),
            &reason,
        );
        return Err(public_error);
    }
    if let Some(outcome) = credential_outcome.as_ref() {
        if !expire_handoff_intent {
            if let Err(error) = bind_handoff_credential_operation(state, &outcome.operation_id) {
                let public_error = credential_post_apply_failure(
                    state,
                    target_profile_id,
                    Some(outcome.operation_id.as_str()),
                    &error,
                );
                return Err(public_error);
            }
        }
        if expire_handoff_intent {
            if let Err(error) = update_handoff_intent_state(
                state,
                HandoffIntentState::Expired,
                Some("standalone_account_switch".to_string()),
            ) {
                let public_error = credential_post_apply_failure(
                    state,
                    target_profile_id,
                    Some(outcome.operation_id.as_str()),
                    &error,
                );
                return Err(public_error);
            }
        } else if let Err(error) =
            update_handoff_intent_state(state, HandoffIntentState::Switching, None)
        {
            let public_error = credential_post_apply_failure(
                state,
                target_profile_id,
                Some(outcome.operation_id.as_str()),
                &error,
            );
            return Err(public_error);
        }
    } else if expire_handoff_intent {
        update_handoff_intent_state(
            state,
            HandoffIntentState::Expired,
            Some("standalone_account_switch".to_string()),
        )?;
    } else {
        update_handoff_intent_state(state, HandoffIntentState::Switching, None)?;
    }
    // 凭证写入成功后只使用已提交的内存运行态生成返回视图，避免二次文件投影失败
    // 把半完成切换误报为普通错误；必要的恢复标记仍在授权上下文有效时完成。
    let handoff_intent = match load_handoff_intent_for_state(state) {
        Ok(intent) => intent,
        Err(error) => {
            let public_error = credential_post_apply_failure(
                state,
                target_profile_id,
                credential_operation_id.as_deref(),
                &error,
            );
            invalidate_authorization_for_account_switch(state);
            return Err(public_error);
        }
    };
    invalidate_authorization_for_account_switch(state);
    let runtime = state
        .managed_account_runtime
        .lock()
        .unwrap()
        .runtime
        .clone();
    Ok(managed_accounts_wire_view(
        &runtime,
        false,
        handoff_intent.as_ref(),
    ))
}

/// 绑定一次切换的不可变计划和可选承接意图；任何文件写入前完成。
fn validate_account_switch_request(
    state: &AppState,
    target_profile_id: &str,
    require_handoff_intent: bool,
) -> Result<String, String> {
    let plan = {
        let runtime_slot = state.managed_account_runtime.lock().unwrap();
        runtime_slot
            .runtime
            .switch_plan
            .clone()
            .ok_or_else(|| "account_switch_plan_not_found".to_string())?
    };
    if plan.target_profile_id != target_profile_id {
        return Err("account_switch_plan_mismatch".to_string());
    }
    if !plan.preflight.ready || plan.state != traesync_domain::AccountSwitchState::Preflight {
        return Err("account_switch_preflight_blocked".to_string());
    }
    if require_handoff_intent {
        let intent = load_handoff_intent_for_state(state)?
            .ok_or_else(|| "handoff_intent_unavailable".to_string())?;
        if intent.state != HandoffIntentState::Prepared
            || intent.target_profile_id != target_profile_id
            || intent.source_profile_id != plan.source_profile_id
            || intent.data_location_id != plan.target_data_location_id
        {
            return Err("handoff_intent_mismatch".to_string());
        }
    }
    Ok(plan.plan_id)
}

/// 保存最小承接意图，再执行账号切换；切换后必须重新授权并重新生成 fresh 计划。
#[tauri::command]
fn switch_and_handoff(
    target_profile_id: String,
    scope: Option<SyncScope>,
    state: tauri::State<AppState>,
) -> Result<ManagedAccountsWireView, String> {
    ensure_local_credential_switch_enabled(&state)?;
    let existing_intent = load_handoff_intent_for_state(&state)?;
    let can_reuse_existing =
        validate_handoff_reuse(existing_intent.as_ref(), &target_profile_id, scope.as_ref())?;
    if !can_reuse_existing {
        create_handoff_intent_inner(&state, &target_profile_id, scope)?;
    }
    match switch_account_inner_with_intent(&state, &target_profile_id, false) {
        Ok(view) => Ok(view),
        Err(error) => {
            mark_account_switch_failure(&state, &target_profile_id, &error);
            Err(error)
        }
    }
}

/// 判断已有承接意图能否复用；活动中的意图不允许被新请求静默覆盖。
fn validate_handoff_reuse(
    existing: Option<&HandoffIntent>,
    target_profile_id: &str,
    requested_scope: Option<&SyncScope>,
) -> Result<bool, String> {
    let Some(intent) = existing else {
        return Ok(false);
    };
    match intent.state {
        HandoffIntentState::Prepared => {
            if intent.target_profile_id != target_profile_id {
                return Err("handoff_target_mismatch".to_string());
            }
            if requested_scope.is_some_and(|scope| scope != &intent.scope) {
                return Err("handoff_scope_mismatch".to_string());
            }
            Ok(true)
        }
        HandoffIntentState::Switching
        | HandoffIntentState::TargetVerified
        | HandoffIntentState::PreviewReady => Err("handoff_intent_in_progress".to_string()),
        HandoffIntentState::Expired | HandoffIntentState::ManualRecoveryRequired => Ok(false),
    }
}

/// 仅保存当前选择的承接意图，不执行账号切换。
#[tauri::command]
fn prepare_handoff_intent(
    target_profile_id: String,
    scope: Option<SyncScope>,
    state: tauri::State<AppState>,
) -> Result<HandoffIntentWireDto, String> {
    let intent = create_handoff_intent_inner(&state, &target_profile_id, scope)?;
    Ok(handoff_intent_wire(&intent))
}

/// Trae CN 产品账号根目录：与 Work CN 的签到材料、官方 Trae CN 目录均隔离。
/// 这里只保存 Trae Sync 自己的加密凭据、非敏感档案和当前账号指针。
fn trae_cn_material_root(state: &AppState) -> Result<PathBuf, String> {
    if state.storage_root.is_empty() {
        return Err("trae_cn_storage_unavailable".to_string());
    }
    Ok(trae_cn_material_root_from(Path::new(&state.storage_root)))
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

/// 更新单账号的归档标记（U-6 W4 资产库三态筛选：全部/活跃/已归档）。
/// 归档只影响前端视图分组，档案、凭据与本地记录全部保留。
/// 返回账号是否存在；不存在视为错误由前端提示。
#[tauri::command]
fn set_account_archived(
    profile_id: String,
    archived: bool,
    state: tauri::State<AppState>,
) -> Result<(), String> {
    if profile_id.is_empty() || profile_id.len() > 256 {
        return Err("checkin_profile_invalid".to_string());
    }
    let material_root = checkin_material_root(&state)?;
    let updated = AccountRegistry::new(&material_root)
        .set_archived(&profile_id, archived)
        .map_err(|_| "checkin_registry_invalid".to_string())?;
    if !updated {
        return Err("checkin_profile_not_found".to_string());
    }
    Ok(())
}

// ============================================================================
// M4：OAuth 登录命令（begin/complete 两段式）已迁 oauth_login 模块（P12-R3）。
// ============================================================================

/// 已登录账号条目（非敏感白名单：不含设备 ID/公钥等绑定材料）。
#[derive(Debug, Clone, Serialize)]
struct CheckinAccountWireDto {
    profile_id: String,
    account_id: String,
    screen_name: String,
    avatar_url: String,
    /// 是否归档（U-6 W4 资产库三态筛选：前端据此分组为全部/活跃/已归档）。
    archived: bool,
    last_verified_at: Option<String>,
}

/// 登录完成回执与 Trae CN 登录回执 DTO 已迁 oauth_login 模块（P12-R3）。

/// Trae CN 独立账号池条目；不把账号 ID、设备绑定或令牌发给前端。
#[derive(Debug, Clone, Serialize)]
struct TraeCnAccountWireDto {
    profile_id: String,
    display_name: String,
    avatar_url: String,
    last_verified_at: Option<String>,
    /// 本地凭据可用性；不代替健康检测的服务端实调结果。
    status: &'static str,
    is_current: bool,
}

/// Trae CN 健康检测逐账号回执；只返回结果和稳定错误码。
#[derive(Debug, Clone, Serialize)]
struct TraeCnHealthEntryDto {
    profile_id: String,
    screen_name: String,
    healthy: bool,
    error_code: Option<String>,
}

// P12-R3：登录 profile_id 生成、OAuth 临时浏览器档案机制（根目录/隔离目录/
// 超龄清理/Chromium 定位/登录页拉起）已迁 oauth_login 模块。

/// 列出已登录账号（非敏感白名单）；注册表尚未创建时返回空表。
fn list_checkin_accounts_inner(state: &AppState) -> Result<Vec<CheckinAccountWireDto>, String> {
    ensure_login_real_mode(&state)?;
    let material_root = checkin_material_root(state)?;
    let registry = AccountRegistry::new(&material_root);
    let records = registry
        .load()
        .map_err(|_| "checkin_registry_invalid".to_string())?;
    Ok(records
        .into_iter()
        .map(|record| CheckinAccountWireDto {
            last_verified_at: std::time::SystemTime::UNIX_EPOCH
                .checked_add(std::time::Duration::from_secs(
                    record.last_verified_at_unix_seconds,
                ))
                .map(system_time_to_rfc3339),
            profile_id: record.profile_id,
            account_id: record.account_id,
            screen_name: record.screen_name,
            avatar_url: record.avatar_url,
            archived: record.archived,
        })
        .collect())
}

#[tauri::command]
fn list_checkin_accounts(
    state: tauri::State<AppState>,
) -> Result<Vec<CheckinAccountWireDto>, String> {
    list_checkin_accounts_inner(&state)
}

/// 列出 Trae CN 自己的账号池；只返回展示字段和当前选择标记。
fn list_trae_cn_accounts_inner(state: &AppState) -> Result<Vec<TraeCnAccountWireDto>, String> {
    ensure_login_real_mode(state)?;
    let material_root = trae_cn_material_root(state)?;
    let current_profile_id = ProductAccountStateStore::new(&material_root)
        .load()
        .map_err(|_| "trae_cn_account_state_invalid".to_string())?;
    let records = AccountRegistry::new(&material_root)
        .load()
        .map_err(|_| "trae_cn_registry_invalid".to_string())?;
    let store = CheckinCredentialStore::new(&material_root);
    let now = unix_now_seconds();
    Ok(records
        .into_iter()
        .map(|record| {
            let status = trae_cn_local_credential_status(&store, &record, now);
            TraeCnAccountWireDto {
                profile_id: record.profile_id.clone(),
                display_name: record
                    .display_name
                    .unwrap_or_else(|| record.screen_name.clone()),
                avatar_url: record.avatar_url,
                last_verified_at: std::time::SystemTime::UNIX_EPOCH
                    .checked_add(std::time::Duration::from_secs(
                        record.last_verified_at_unix_seconds,
                    ))
                    .map(system_time_to_rfc3339),
                status,
                is_current: current_profile_id.as_deref() == Some(record.profile_id.as_str()),
            }
        })
        .collect())
}

/// 仅根据本地加密凭据与 access token 到期时间推导列表徽章，不发起网络请求。
fn trae_cn_local_credential_status(
    store: &CheckinCredentialStore,
    record: &AccountRecord,
    now_unix_seconds: u64,
) -> &'static str {
    let binding = CheckinProfileBinding::from(record);
    match store.load(&binding) {
        Ok(bundle)
            if bundle.client_id == OAuthClient::TraeCn.client_id()
                && bundle.access_token_expires_at_unix_seconds > now_unix_seconds =>
        {
            "active"
        }
        Ok(bundle) if bundle.client_id == OAuthClient::TraeCn.client_id() => "expired",
        Err(_) => "unknown",
        // 凭据根被手工混入其他产品材料时，按未知处理，不把它显示成 CN 已登录。
        Ok(_) => "unknown",
    }
}

#[tauri::command]
fn list_trae_cn_accounts(
    state: tauri::State<AppState>,
) -> Result<Vec<TraeCnAccountWireDto>, String> {
    list_trae_cn_accounts_inner(&state)
}

/// 对 Trae CN 账号执行只读健康检测：使用 CN 自己的 OAuth client 查询账号资料，
/// 不签到、不读取额度、不打开新的 OAuth，也不把 Work 凭据带入 CN。
#[tauri::command]
async fn check_trae_cn_account_health(
    profile_ids: Option<Vec<String>>,
    state: tauri::State<'_, AppState>,
    app: tauri::AppHandle,
) -> Result<Vec<TraeCnHealthEntryDto>, String> {
    ensure_login_real_mode(&state)?;
    let selected = validate_product_profile_ids(profile_ids)?;
    let material_root = trae_cn_material_root(&state)?;
    run_blocking("trae_cn_health_join_failed", move || {
        check_trae_cn_account_health_inner(&material_root, &selected, &|event| {
            // 逐账号进度事件（P11-3）：AppHandle 借给 emit 闭包传进深层编排。
            let _ = app.emit("health-check-progress", event);
        })
    })
    .await
}

fn check_trae_cn_account_health_inner(
    material_root: &Path,
    selected: &[String],
    progress: &dyn Fn(BatchProgressEvent),
) -> Result<Vec<TraeCnHealthEntryDto>, String> {
    let registry = AccountRegistry::new(material_root);
    let records = registry
        .load()
        .map_err(|_| "trae_cn_registry_invalid".to_string())?;
    let store = CheckinCredentialStore::new(material_root);
    let client = trae_http_client();
    let now = unix_now_seconds();

    run_serial_batch(selected, &records, progress, |profile_id, record, completed, total| {
        let Some(record) = record else {
            // 缺失档案：不触网即失败，进度事件与回执同步带 credential_missing。
            return Ok((
                TraeCnHealthEntryDto {
                    profile_id: profile_id.to_string(),
                    screen_name: String::new(),
                    healthy: false,
                    error_code: Some("credential_missing".to_string()),
                },
                BatchProgressEvent::HealthCheck {
                    profile_id: profile_id.to_string(),
                    screen_name: String::new(),
                    completed,
                    total,
                    healthy: false,
                    error_code: Some("credential_missing".to_string()),
                },
            ));
        };
        let binding = CheckinProfileBinding::from(record);
        let result = store
            .load(&binding)
            .map_err(|error| credential_maintenance_error_code(&error).to_string())
            .and_then(|bundle| {
                if bundle.client_id != OAuthClient::TraeCn.client_id() {
                    return Err("credential_invalid".to_string());
                }
                get_user_info_full_for_oauth_client(
                    &client,
                    &bundle.access_token,
                    OAuthClient::TraeCn,
                )
                .map_err(|error| trae_cn_health_error_code(&error))
                .and_then(|info| {
                    if info.user_id == record.account_id {
                        Ok(())
                    } else {
                        Err("auth_mismatch".to_string())
                    }
                })
            });

        match result {
            Ok(()) => {
                let mut verified = record.clone();
                verified.last_verified_at_unix_seconds = now;
                registry
                    .upsert(&verified)
                    .map_err(|_| "trae_cn_registry_invalid".to_string())?;
                Ok((
                    TraeCnHealthEntryDto {
                        profile_id: profile_id.to_string(),
                        screen_name: record.screen_name.clone(),
                        healthy: true,
                        error_code: None,
                    },
                    BatchProgressEvent::HealthCheck {
                        profile_id: profile_id.to_string(),
                        screen_name: record.screen_name.clone(),
                        completed,
                        total,
                        healthy: true,
                        error_code: None,
                    },
                ))
            }
            Err(error_code) => Ok((
                TraeCnHealthEntryDto {
                    profile_id: profile_id.to_string(),
                    screen_name: record.screen_name.clone(),
                    healthy: false,
                    error_code: Some(error_code.clone()),
                },
                BatchProgressEvent::HealthCheck {
                    profile_id: profile_id.to_string(),
                    screen_name: record.screen_name.clone(),
                    completed,
                    total,
                    healthy: false,
                    error_code: Some(error_code),
                },
            )),
        }
    })
}

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

fn validate_product_profile_ids(profile_ids: Option<Vec<String>>) -> Result<Vec<String>, String> {
    let selected = profile_ids.unwrap_or_default();
    let mut unique = Vec::with_capacity(selected.len());
    for profile_id in selected {
        if profile_id.is_empty() || profile_id.len() > 256 {
            return Err("checkin_profile_invalid".to_string());
        }
        if !unique.contains(&profile_id) {
            unique.push(profile_id);
        }
    }
    if unique.is_empty() {
        return Err("checkin_profile_empty".to_string());
    }
    Ok(unique)
}

/// 切换 Trae CN 当前账号：先验证档案与 DPAPI 凭据绑定，再原子更新当前指针。
/// 不向官方 Trae CN 目录写入 token、blob、设备身份或数据库。
#[tauri::command]
fn switch_trae_cn_account(
    profile_id: String,
    state: tauri::State<AppState>,
) -> Result<Vec<TraeCnAccountWireDto>, String> {
    ensure_login_real_mode(&state)?;
    let material_root = trae_cn_material_root(&state)?;
    let registry = AccountRegistry::new(&material_root);
    let record = registry
        .find(&profile_id)
        .map_err(|_| "trae_cn_registry_invalid".to_string())?
        .ok_or_else(|| "trae_cn_account_not_found".to_string())?;
    let binding = CheckinProfileBinding::from(&record);
    let bundle = CheckinCredentialStore::new(&material_root)
        .load(&binding)
        .map_err(|_| "trae_cn_credential_invalid".to_string())?;
    if bundle.client_id != OAuthClient::TraeCn.client_id() {
        return Err("trae_cn_credential_invalid".to_string());
    }
    ProductAccountStateStore::new(&material_root)
        .save(&profile_id)
        .map_err(|_| "trae_cn_account_state_failed".to_string())?;
    list_trae_cn_accounts_inner(&state)
}

/// 单账号积分刷新回执（refresh_checkin_credits 逐账号结果）。
#[derive(Clone, Serialize)]
struct CreditsRefreshEntryDto {
    profile_id: String,
    screen_name: String,
    /// 最新积分；失败时为 None（保留旧缓存值）。
    credits: Option<i64>,
    /// 最新“今日已签”状态；失败时为 None。
    checked_in: Option<bool>,
    /// 真实模型额度剩余（`ide_user_ent_usage`）；失败时为 None（保留旧缓存值）。
    usage_remaining_credits: Option<f64>,
    /// 失败原因码（network_error / business_XXXX / credential_*）；成功为 None。
    error_code: Option<String>,
}

/// 手动刷新登录凭据逐账号回执；不携带 access/refresh 等敏感内容。
#[derive(Clone, Serialize)]
struct CredentialRefreshEntryDto {
    profile_id: String,
    screen_name: String,
    /// 是否已完成同设备凭据换发并安全写回。
    refreshed: bool,
    /// 失败原因码；成功为 None。
    error_code: Option<String>,
}

/// 只读刷新积分：逐账号调 status + ide_user_ent_usage（不 claim、不消耗签到资格），
/// 成功快照分别写回积分/额度缓存。串行执行、账号间无间隔（仅查询，实时完成；
/// 签到 claim 才需要 3-8 秒随机间隔防风控）。单账号失败不影响其余账号；
/// 失败账号保留旧缓存并返回原因码。
#[tauri::command]
async fn refresh_checkin_credits(
    profile_ids: Vec<String>,
    state: tauri::State<'_, AppState>,
    app: tauri::AppHandle,
) -> Result<Vec<CreditsRefreshEntryDto>, String> {
    let mut selected = Vec::new();
    for profile_id in &profile_ids {
        if profile_id.is_empty() || profile_id.len() > 256 {
            return Err("checkin_profile_invalid".to_string());
        }
        if !selected.contains(profile_id) {
            selected.push(profile_id.clone());
        }
    }
    if selected.is_empty() {
        return Err("checkin_profile_empty".to_string());
    }
    let material_root = checkin_material_root(&state)?;
    let runtime_mode = state.runtime_mode;
    run_blocking("checkin_refresh_join_failed", move || {
        refresh_checkin_credits_inner(&material_root, &selected, runtime_mode, &|event| {
            // 逐账号进度事件（P11-3）：AppHandle 借给 emit 闭包传进深层编排。
            let _ = app.emit("credits-refresh-progress", event);
        })
    })
    .await
}

fn refresh_checkin_credits_inner(
    material_root: &std::path::Path,
    selected: &[String],
    runtime_mode: RuntimeMode,
    progress: &dyn Fn(BatchProgressEvent),
) -> Result<Vec<CreditsRefreshEntryDto>, String> {
    let registry = AccountRegistry::new(material_root);
    let records = registry
        .load()
        .map_err(|_| "checkin_registry_invalid".to_string())?;
    let store = CheckinCredentialStore::new(material_root);
    // 与签到一致的中断写回阻断：存在未收口续期时拒绝刷新，先恢复。
    if store.has_pending_renewal().unwrap_or(false) {
        return Err("manual_recovery_required".to_string());
    }

    let mut snapshots: Vec<(String, traesync_domain::CheckinStatusSnapshot)> = Vec::new();
    let mut usages: Vec<(String, traesync_domain::EntitlementUsageSnapshot)> = Vec::new();
    // 失败账号原因码：写入积分缓存持久化（卡片“刷新失败”标记，成功后清除）。
    let mut failures: Vec<(String, String)> = Vec::new();

    let entries = run_serial_batch(
        selected,
        &records,
        progress,
        |profile_id, record, completed, total| {
            let Some(record) = record else {
                return Ok((
                    CreditsRefreshEntryDto {
                        profile_id: profile_id.to_string(),
                        screen_name: String::new(),
                        credits: None,
                        checked_in: None,
                        usage_remaining_credits: None,
                        error_code: Some("credential_missing".to_string()),
                    },
                    BatchProgressEvent::CreditsRefresh {
                        profile_id: profile_id.to_string(),
                        screen_name: String::new(),
                        completed,
                        total,
                        ok: false,
                        credits: None,
                        checked_in: None,
                        usage_remaining_credits: None,
                        error_code: Some("credential_missing".to_string()),
                    },
                ));
            };
            let binding = CheckinProfileBinding::from(record);
            // 手机号补采用克隆：binding 随后 move 进 transport 的绑定表。
            let backfill_binding = binding.clone();
            let refresh_result = match runtime_mode {
                RuntimeMode::Fixture => Ok(()),
                RuntimeMode::RealReadPreview => {
                    let renewal = RealCheckinRenewalService::new(&store);
                    let now = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|duration| duration.as_secs())
                        .unwrap_or(0);
                    renewal.renew_if_needed(&binding, now).map(|_| ())
                }
            };
            let transport: Box<dyn CheckinTransport> = match runtime_mode {
                // for_profiles 接收 &[String]：闭包内 profile_id 已是 &str，补拥有值。
                RuntimeMode::Fixture => Box::new(FixtureCheckinTransport::for_profiles(&[
                    profile_id.to_string(),
                ])),
                RuntimeMode::RealReadPreview => {
                    let mut bindings = std::collections::BTreeMap::new();
                    bindings.insert(record.profile_id.clone(), binding);
                    Box::new(RealCheckinTransport::new(&store, bindings))
                }
            };
            let status = match refresh_result {
                Err(_) => Err("credential_refresh_failed".to_string()),
                Ok(()) => transport
                    .status(profile_id)
                    .map_err(|error| checkin_transport_error_code(&error)),
            };
            match status {
                Ok(snapshot) => {
                    snapshots.push((profile_id.to_string(), snapshot.clone()));
                    // U-1 脱敏手机号无感补采：仅真实模式且档案缺失时调用一次
                    // GetUserInfo（只读接口，失败静默，下次刷新再试）。
                    // 健康检测复用本命令路径，存量账号由此补全。
                    if record.masked_mobile.is_empty()
                        && matches!(runtime_mode, RuntimeMode::RealReadPreview)
                    {
                        if let Ok(bundle) = store.load(&backfill_binding) {
                            let client = trae_http_client();
                            if let Ok(user_info) = get_user_info(&client, &bundle.access_token) {
                                if !user_info.masked_mobile.is_empty() {
                                    let _ = registry
                                        .backfill_masked_mobile(profile_id, &user_info.masked_mobile);
                                }
                            }
                        }
                    }
                    // status 成功后追加真实额度查询：独立失败处理，额度失败不拖垮积分回执。
                    let usage = transport
                        .entitlement_usage(profile_id)
                        .map_err(|error| checkin_transport_error_code(&error));
                    let usage_remaining_credits = match usage {
                        Ok(usage_snapshot) => {
                            let remaining = usage_snapshot.remaining_credits;
                            usages.push((profile_id.to_string(), usage_snapshot));
                            Some(remaining)
                        }
                        Err(_) => None,
                    };
                    Ok((
                        CreditsRefreshEntryDto {
                            profile_id: profile_id.to_string(),
                            screen_name: record.screen_name.clone(),
                            credits: snapshot.credits,
                            checked_in: Some(snapshot.checked_in),
                            usage_remaining_credits,
                            error_code: None,
                        },
                        BatchProgressEvent::CreditsRefresh {
                            profile_id: profile_id.to_string(),
                            screen_name: record.screen_name.clone(),
                            completed,
                            total,
                            ok: true,
                            credits: snapshot.credits,
                            checked_in: Some(snapshot.checked_in),
                            usage_remaining_credits,
                            error_code: None,
                        },
                    ))
                }
                Err(error_code) => {
                    failures.push((profile_id.to_string(), error_code.clone()));
                    Ok((
                        CreditsRefreshEntryDto {
                            profile_id: profile_id.to_string(),
                            screen_name: record.screen_name.clone(),
                            credits: None,
                            checked_in: None,
                            usage_remaining_credits: None,
                            error_code: Some(error_code.clone()),
                        },
                        BatchProgressEvent::CreditsRefresh {
                            profile_id: profile_id.to_string(),
                            screen_name: record.screen_name.clone(),
                            completed,
                            total,
                            ok: false,
                            credits: None,
                            checked_in: None,
                            usage_remaining_credits: None,
                            error_code: Some(error_code),
                        },
                    ))
                }
            }
        },
    )?;

    checkin_overview::update_credits_from_snapshots(material_root, &snapshots);
    checkin_overview::update_usage_cache(material_root, &usages);
    checkin_overview::update_refresh_failures(material_root, &failures);
    Ok(entries)
}

/// 手动刷新登录凭据：只走凭据换发链路，不签到、不查询额度、不打开用户 OAuth。
///
/// access 仍有效时由 `renew_now` 使用同设备 AuthCode 换发；access 已过期时
/// 仅允许当前凭据代次的一次 refresh 救援。账号逐个处理，单个失败不会阻塞其余账号。
#[tauri::command]
async fn refresh_checkin_credentials(
    profile_ids: Vec<String>,
    state: tauri::State<'_, AppState>,
    app: tauri::AppHandle,
) -> Result<Vec<CredentialRefreshEntryDto>, String> {
    ensure_checkin_http_enabled(&state)?;
    let mut selected = Vec::new();
    for profile_id in profile_ids {
        if profile_id.is_empty() || profile_id.len() > 256 {
            return Err("checkin_profile_invalid".to_string());
        }
        if !selected.contains(&profile_id) {
            selected.push(profile_id);
        }
    }
    if selected.is_empty() {
        return Err("checkin_profile_empty".to_string());
    }
    let material_root = checkin_material_root(&state)?;
    let execution_lock = Arc::clone(&state.checkin_execution_lock);
    run_blocking("credential_refresh_join_failed", move || {
        // 凭据换发与签到/设备操作共享互斥，避免并发写入同一凭据包。
        let _guard = execution_lock
            .try_lock()
            .map_err(|_| "credential_refresh_busy".to_string())?;
        refresh_checkin_credentials_inner(&material_root, &selected, &|event| {
            // 逐账号进度事件（P11-3）：AppHandle 借给 emit 闭包传进深层编排。
            let _ = app.emit("credential-refresh-progress", event);
        })
    })
    .await
}

/// 手动刷新 Trae CN 自己的登录凭据；使用 CN OAuth client 和独立凭据根。
#[tauri::command]
async fn refresh_trae_cn_credentials(
    profile_ids: Vec<String>,
    state: tauri::State<'_, AppState>,
    app: tauri::AppHandle,
) -> Result<Vec<CredentialRefreshEntryDto>, String> {
    ensure_checkin_http_enabled(&state)?;
    let selected = validate_product_profile_ids(Some(profile_ids))?;
    let material_root = trae_cn_material_root(&state)?;
    let execution_lock = Arc::clone(&state.checkin_execution_lock);
    run_blocking("credential_refresh_join_failed", move || {
        // Work 与 Trae CN 共享进程内写入互斥，但各自使用独立凭据根。
        let _guard = execution_lock
            .try_lock()
            .map_err(|_| "credential_refresh_busy".to_string())?;
        refresh_product_credentials_inner(&material_root, &selected, OAuthClient::TraeCn, &|event| {
            let _ = app.emit("credential-refresh-progress", event);
        })
    })
    .await
}

fn refresh_checkin_credentials_inner(
    material_root: &std::path::Path,
    selected: &[String],
    progress: &dyn Fn(BatchProgressEvent),
) -> Result<Vec<CredentialRefreshEntryDto>, String> {
    refresh_product_credentials_inner(material_root, selected, OAuthClient::Solo, progress)
}

fn refresh_product_credentials_inner(
    material_root: &std::path::Path,
    selected: &[String],
    oauth_client: OAuthClient,
    progress: &dyn Fn(BatchProgressEvent),
) -> Result<Vec<CredentialRefreshEntryDto>, String> {
    let store = CheckinCredentialStore::new(material_root);
    // 有未收口写回时整批停止，避免把现场继续推进到更难恢复的状态。
    if store
        .has_pending_renewal()
        .map_err(|_| "manual_recovery_required".to_string())?
    {
        return Err("manual_recovery_required".to_string());
    }

    let now = unix_now_seconds();
    let renewal = RealCheckinRenewalService::new_for_oauth_client(&store, oauth_client);
    refresh_checkin_credentials_inner_with(
        material_root,
        selected,
        now,
        &|binding, timestamp| renewal.renew_now(binding, timestamp).map(|_| ()),
        progress,
    )
}

/// 手动刷新命令的结果收集边界。
///
/// 生产路径传入真实续期服务；测试可以注入确定性闭包，验证选中账号去重、
/// 单账号失败后继续处理、逐账号回执和失败证据写回，而无需触碰 OAuth 网络。
fn refresh_checkin_credentials_inner_with(
    material_root: &std::path::Path,
    selected: &[String],
    now: u64,
    renew: &dyn Fn(&CheckinProfileBinding, u64) -> Result<(), CheckinCredentialError>,
    progress: &dyn Fn(BatchProgressEvent),
) -> Result<Vec<CredentialRefreshEntryDto>, String> {
    let registry = AccountRegistry::new(material_root);
    let records = registry
        .load()
        .map_err(|_| "checkin_registry_invalid".to_string())?;
    let mut refreshed = Vec::new();
    let mut failures = Vec::new();
    let maintenance_state = CredentialMaintenanceStateStore::new(material_root);

    let entries = run_serial_batch(
        selected,
        &records,
        progress,
        |profile_id, record, completed, total| {
            let Some(record) = record else {
                failures.push((profile_id.to_string(), "credential_missing".to_string()));
                return Ok((
                    CredentialRefreshEntryDto {
                        profile_id: profile_id.to_string(),
                        screen_name: String::new(),
                        refreshed: false,
                        error_code: Some("credential_missing".to_string()),
                    },
                    BatchProgressEvent::CredentialRefresh {
                        profile_id: profile_id.to_string(),
                        screen_name: String::new(),
                        completed,
                        total,
                        refreshed: false,
                        error_code: Some("credential_missing".to_string()),
                    },
                ));
            };
            let binding = CheckinProfileBinding::from(record);
            match renew(&binding, now) {
                Ok(_) => {
                    refreshed.push(profile_id.to_string());
                    // 手动成功也清除自动维护的旧退避；手动入口不读取/拦截退避，
                    // 只把确认成功的结果反馈给下一次自动维护。
                    let _ = maintenance_state.record_success(profile_id, now);
                    Ok((
                        CredentialRefreshEntryDto {
                            profile_id: profile_id.to_string(),
                            screen_name: record.screen_name.clone(),
                            refreshed: true,
                            error_code: None,
                        },
                        BatchProgressEvent::CredentialRefresh {
                            profile_id: profile_id.to_string(),
                            screen_name: record.screen_name.clone(),
                            completed,
                            total,
                            refreshed: true,
                            error_code: None,
                        },
                    ))
                }
                Err(error) => {
                    let error_code = credential_maintenance_error_code(&error).to_string();
                    failures.push((profile_id.to_string(), error_code.clone()));
                    Ok((
                        CredentialRefreshEntryDto {
                            profile_id: profile_id.to_string(),
                            screen_name: record.screen_name.clone(),
                            refreshed: false,
                            error_code: Some(error_code.clone()),
                        },
                        BatchProgressEvent::CredentialRefresh {
                            profile_id: profile_id.to_string(),
                            screen_name: record.screen_name.clone(),
                            completed,
                            total,
                            refreshed: false,
                            error_code: Some(error_code),
                        },
                    ))
                }
            }
        },
    )?;

    // 失败证据保留；成功只清除该账号的凭据维护失败标记，不碰额度与签到记录。
    checkin_overview::update_refresh_failures(material_root, &failures);
    checkin_overview::clear_refresh_failures(material_root, &refreshed);
    Ok(entries)
}

// P12-R3：login_receipt_wire_tests 三例随登录回执 DTO 迁至 oauth_login 模块。

/// P11-3 批量凭据刷新 / 健康检测的逐账号进度事件契约（P12-R2b 起载荷统一为
/// BatchProgressEvent）：载荷非敏感白名单、事件与账号一一对应、
/// completed 递增且失败不中断批次。
#[cfg(test)]
mod progress_event_contract_tests {
    use super::*;

    fn record(profile_id: &str, account_id: &str, screen_name: &str) -> AccountRecord {
        AccountRecord {
            profile_id: profile_id.to_string(),
            account_id: account_id.to_string(),
            screen_name: screen_name.to_string(),
            avatar_url: String::new(),
            device_id: "1234567890123456".to_string(),
            device_public_key: "public-key".to_string(),
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
    fn manual_refresh_emits_progress_per_account_and_continues() {
        let root = tempfile::tempdir().unwrap();
        let registry = AccountRegistry::new(root.path());
        registry
            .upsert(&record("profile-fail", "account-fail", "失败账号"))
            .unwrap();
        registry
            .upsert(&record("profile-ok", "account-ok", "成功账号"))
            .unwrap();
        let selected = vec![
            "profile-fail".to_string(),
            "profile-ok".to_string(),
            "profile-missing".to_string(),
        ];
        let events = std::cell::RefCell::new(Vec::new());
        let results = refresh_checkin_credentials_inner_with(
            root.path(),
            &selected,
            1_800_000_000,
            &|binding, _| {
                if binding.profile_id == "profile-fail" {
                    Err(CheckinCredentialError::CredentialRefreshFailed)
                } else {
                    Ok(())
                }
            },
            &|event| events.borrow_mut().push(event),
        )
        .unwrap();
        let events = events.into_inner();

        // 每个账号完成（含失败与缺失）各推送一次进度，失败不中断批次。
        assert_eq!(events.len(), 3);
        let BatchProgressEvent::CredentialRefresh {
            profile_id,
            screen_name,
            completed,
            total,
            refreshed,
            error_code,
        } = &events[0]
        else {
            panic!("应为凭据刷新链事件");
        };
        assert_eq!(profile_id, "profile-fail");
        assert_eq!(screen_name, "失败账号");
        assert_eq!(*completed, 1);
        assert_eq!(*total, 3);
        assert!(!refreshed);
        assert_eq!(error_code.as_deref(), Some("credential_refresh_failed"));
        let BatchProgressEvent::CredentialRefresh {
            profile_id,
            completed,
            refreshed,
            error_code,
            ..
        } = &events[1]
        else {
            panic!("应为凭据刷新链事件");
        };
        assert_eq!(profile_id, "profile-ok");
        assert_eq!(*completed, 2);
        assert!(*refreshed);
        assert!(error_code.is_none());
        // 缺失档案的进度事件 screen_name 为空（前端回退展示 profile_id）。
        let BatchProgressEvent::CredentialRefresh {
            profile_id,
            screen_name,
            completed,
            error_code,
            ..
        } = &events[2]
        else {
            panic!("应为凭据刷新链事件");
        };
        assert_eq!(profile_id, "profile-missing");
        assert_eq!(screen_name, "");
        assert_eq!(*completed, 3);
        assert_eq!(error_code.as_deref(), Some("credential_missing"));
        // 整批回执与进度事件一一对应，既有失败语义不回归。
        assert_eq!(results.len(), 3);
        assert!(!results[0].refreshed);
        assert!(results[1].refreshed);
    }

    #[cfg(windows)]
    #[test]
    fn trae_cn_health_emits_progress_per_account() {
        let root = tempfile::tempdir().unwrap();
        AccountRegistry::new(root.path())
            .upsert(&record("profile-h", "account-h", "健康号"))
            .unwrap();
        let events = std::cell::RefCell::new(Vec::new());
        let results = check_trae_cn_account_health_inner(
            root.path(),
            &["profile-h".to_string()],
            &|event| events.borrow_mut().push(event),
        )
        .unwrap();
        let events = events.into_inner();
        // 凭据材料不可用（绑定公钥无效 → credential_invalid）：不触网即失败，
        // 进度事件与整批回执同步如实带错误码。
        assert_eq!(events.len(), 1);
        let BatchProgressEvent::HealthCheck {
            profile_id,
            screen_name,
            completed,
            total,
            healthy,
            error_code,
        } = &events[0]
        else {
            panic!("应为健康检测链事件");
        };
        assert_eq!(profile_id, "profile-h");
        assert_eq!(screen_name, "健康号");
        assert_eq!(*completed, 1);
        assert_eq!(*total, 1);
        assert!(!healthy);
        assert_eq!(error_code.as_deref(), Some("credential_invalid"));
        assert_eq!(results.len(), 1);
        assert!(!results[0].healthy);
    }

    #[test]
    fn credits_refresh_emits_progress_per_account_and_continues() {
        let root = tempfile::tempdir().unwrap();
        AccountRegistry::new(root.path())
            .upsert(&record("profile-ok", "account-ok", "成功账号"))
            .unwrap();
        AccountRegistry::new(root.path())
            .upsert(&record("profile-later", "account-later", "后续账号"))
            .unwrap();
        let selected = vec![
            "profile-ok".to_string(),
            "profile-missing".to_string(),
            "profile-later".to_string(),
        ];
        let events = std::cell::RefCell::new(Vec::new());
        let results = refresh_checkin_credits_inner(
            root.path(),
            &selected,
            RuntimeMode::Fixture,
            &|event| events.borrow_mut().push(event),
        )
        .unwrap();
        let events = events.into_inner();

        // 每个账号完成（含失败与缺失）各推送一次进度，失败不中断批次。
        assert_eq!(events.len(), 3);
        // 成功账号：fixture 快照（credits=0、未签）；额度失败不拖垮积分回执。
        let BatchProgressEvent::CreditsRefresh {
            profile_id,
            screen_name,
            completed,
            total,
            ok,
            credits,
            checked_in,
            error_code,
            ..
        } = &events[0]
        else {
            panic!("应为积分刷新链事件");
        };
        assert_eq!(profile_id, "profile-ok");
        assert_eq!(screen_name, "成功账号");
        assert_eq!(*completed, 1);
        assert_eq!(*total, 3);
        assert!(*ok);
        assert_eq!(credits, &Some(0));
        assert_eq!(checked_in, &Some(false));
        assert!(error_code.is_none());
        // 缺失档案：事件如实带 credential_missing，批次继续。
        let BatchProgressEvent::CreditsRefresh {
            profile_id,
            completed,
            ok,
            error_code,
            ..
        } = &events[1]
        else {
            panic!("应为积分刷新链事件");
        };
        assert_eq!(profile_id, "profile-missing");
        assert_eq!(*completed, 2);
        assert!(!ok);
        assert_eq!(error_code.as_deref(), Some("credential_missing"));
        let BatchProgressEvent::CreditsRefresh {
            profile_id,
            completed,
            ok,
            ..
        } = &events[2]
        else {
            panic!("应为积分刷新链事件");
        };
        assert_eq!(profile_id, "profile-later");
        assert_eq!(*completed, 3);
        assert!(*ok);
        // 整批回执与进度事件一一对应。
        assert_eq!(results.len(), 3);
        assert!(results[0].error_code.is_none());
        assert_eq!(results[1].error_code.as_deref(), Some("credential_missing"));
        assert!(results[2].error_code.is_none());
    }
}

#[cfg(test)]
mod credential_refresh_contract_tests {
    use super::*;

    fn record(profile_id: &str, account_id: &str, screen_name: &str) -> AccountRecord {
        AccountRecord {
            profile_id: profile_id.to_string(),
            account_id: account_id.to_string(),
            screen_name: screen_name.to_string(),
            avatar_url: String::new(),
            device_id: "1234567890123456".to_string(),
            device_public_key: "public-key".to_string(),
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
    fn manual_refresh_returns_per_account_results_and_continues_after_failure() {
        let root = tempfile::tempdir().unwrap();
        let registry = AccountRegistry::new(root.path());
        registry
            .upsert(&record("profile-fail", "account-fail", "失败账号"))
            .unwrap();
        registry
            .upsert(&record("profile-ok", "account-ok", "成功账号"))
            .unwrap();
        let calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let selected = vec![
            "profile-fail".to_string(),
            "profile-ok".to_string(),
            "profile-missing".to_string(),
        ];

        let results = refresh_checkin_credentials_inner_with(
            root.path(),
            &selected,
            1_800_000_000,
            &|binding, _| {
                calls.lock().unwrap().push(binding.profile_id.clone());
                if binding.profile_id == "profile-fail" {
                    Err(CheckinCredentialError::CredentialRefreshFailed)
                } else {
                    Ok(())
                }
            },
            // 既有失败语义回归：进度事件由 progress_event_contract_tests 覆盖。
            &|_| {},
        )
        .unwrap();

        // 一个账号失败不能中断后续账号；缺失账号也必须有明确回执。
        assert_eq!(
            *calls.lock().unwrap(),
            vec!["profile-fail".to_string(), "profile-ok".to_string()]
        );
        assert_eq!(results.len(), 3);
        assert!(!results[0].refreshed);
        assert_eq!(
            results[0].error_code.as_deref(),
            Some("credential_refresh_failed")
        );
        assert!(results[1].refreshed);
        assert_eq!(results[1].screen_name, "成功账号");
        assert!(!results[2].refreshed);
        assert_eq!(results[2].error_code.as_deref(), Some("credential_missing"));
    }

}

/// 删除账号：注册表档案 + DPAPI 凭据包 + 积分缓存条目三件套清理。
/// 幂等性由前端二次确认保障；档案不存在返回错误（不静默成功）。
#[tauri::command]
fn remove_checkin_account(profile_id: String, state: tauri::State<AppState>) -> Result<(), String> {
    if profile_id.is_empty() || profile_id.len() > 256 {
        return Err("checkin_profile_invalid".to_string());
    }
    let material_root = checkin_material_root(&state)?;
    let registry = AccountRegistry::new(&material_root);
    let removed = registry
        .remove(&profile_id)
        .map_err(|_| "checkin_registry_invalid".to_string())?;
    if !removed {
        return Err("checkin_profile_invalid".to_string());
    }
    let store = CheckinCredentialStore::new(&material_root);
    store
        .remove(&profile_id)
        .map_err(|_| "checkin_storage_unavailable".to_string())?;
    checkin_overview::remove_credits_cache_entry(&material_root, &profile_id);
    Ok(())
}

/// 手动重铸账号签到设备：走网络完整流程（GetPCAuthCode → ExchangeToken →
/// 凭据写回），不执行签到。用于自动重铸失败后的手动兜底
/// （适用场景：账号设备连续多日 9074/9095，用户主动换新）。
/// 成功返回新设备 ID，前端刷新总览即可看到新设备尾号。
#[tauri::command]
fn reset_checkin_device(
    profile_id: String,
    state: tauri::State<AppState>,
) -> Result<String, String> {
    if profile_id.is_empty() || profile_id.len() > 256 {
        return Err("checkin_profile_invalid".to_string());
    }
    let material_root = checkin_material_root(&state)?;
    let service = DeviceRemintService::new(&material_root);
    service
        .remint(&profile_id)
        .map(|device_id| device_id)
        .map_err(|error: RemintError| match error {
            RemintError::ProfileNotFound => "checkin_profile_invalid".to_string(),
            other => format!("remint_failed: {other}"),
        })
}

/// 设置/清除账号本地备注名（空串或 None = 清除，回退服务端名）。
/// U-1 数据层：解决服务端 ScreenName（如「用户A」）辨识度差的问题。
#[tauri::command]
fn set_account_display_name(
    profile_id: String,
    display_name: Option<String>,
    state: tauri::State<AppState>,
) -> Result<(), String> {
    if profile_id.is_empty() || profile_id.len() > 256 {
        return Err("checkin_profile_invalid".to_string());
    }
    // trim 后归一化：空别名 = 清除；超长拒绝（注册表 validate 同口径）。
    let alias = display_name.as_deref().map(str::trim);
    if let Some(name) = alias {
        if name.len() > 256 {
            return Err("display_name_invalid".to_string());
        }
    }
    let material_root = checkin_material_root(&state)?;
    let registry = AccountRegistry::new(&material_root);
    registry
        .set_display_name(&profile_id, alias)
        .map_err(|_| "checkin_registry_invalid".to_string())?;
    Ok(())
}

/// G11 手机号补录：写入/清除账号完整手机号（存凭据包，与令牌同级 DPAPI 加密）。
/// 三层校验的前两层在本命令：①大陆手机号格式（11 位，1 开头第二位 3-9）
/// ②与服务端脱敏号首尾比对（脱敏号前缀+后缀必须完全匹配，防串号录错账号）；
/// 第三层查重提示由前端基于总览数据判断后向用户确认。
/// mobile 为 None/空白 = 清除补录（展示回退脱敏号）。
#[tauri::command]
fn set_account_mobile(
    profile_id: String,
    mobile: Option<String>,
    state: tauri::State<AppState>,
) -> Result<(), String> {
    if profile_id.is_empty() || profile_id.len() > 256 {
        return Err("checkin_profile_invalid".to_string());
    }
    let trimmed = mobile.as_deref().map(str::trim).unwrap_or("");
    let material_root = checkin_material_root(&state)?;
    let registry = AccountRegistry::new(&material_root);
    let record = registry
        .load()
        .map_err(|_| "checkin_registry_invalid".to_string())?
        .into_iter()
        .find(|record| record.profile_id == profile_id)
        .ok_or_else(|| "checkin_profile_invalid".to_string())?;

    // 清除路径：空输入 = 回退脱敏号展示（输入错了的后悔药）。
    if trimmed.is_empty() {
        let store = CheckinCredentialStore::new(&material_root);
        let binding = CheckinProfileBinding::from(&record);
        store
            .set_mobile_full(&binding, None)
            .map_err(|_| "mobile_save_failed".to_string())?;
        return Ok(());
    }

    // 校验一：大陆手机号格式（1[3-9] 开头共 11 位数字）。
    if !is_mainland_mobile(trimmed) {
        return Err("mobile_format_invalid".to_string());
    }
    // 校验二：与服务端脱敏号首尾比对（如 "138****0000" → 前 3 后 4）。
    // 脱敏号尚未采集（旧账号未刷新额度）时无基准，跳过比对只走格式校验。
    if let Some((prefix, suffix)) = masked_mobile_parts(&record.masked_mobile) {
        let mismatch = !trimmed.starts_with(prefix.as_str()) || !trimmed.ends_with(suffix.as_str());
        if mismatch {
            return Err("mobile_masked_mismatch".to_string());
        }
    }

    let store = CheckinCredentialStore::new(&material_root);
    let binding = CheckinProfileBinding::from(&record);
    store
        .set_mobile_full(&binding, Some(trimmed))
        .map_err(|_| "mobile_save_failed".to_string())?;
    Ok(())
}

/// 大陆手机号格式：1 开头、第二位 3-9、共 11 位数字。
fn is_mainland_mobile(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() == 11
        && bytes[0] == b'1'
        && (b'3'..=b'9').contains(&bytes[1])
        && bytes.iter().all(|byte| byte.is_ascii_digit())
}

/// 解析脱敏手机号的首尾明文段（"138****0000" → ("138", "0000")）。
/// 非预期形态（空串、无星号、首尾非数字）返回 None，调用方跳过比对。
fn masked_mobile_parts(masked: &str) -> Option<(String, String)> {
    let star = masked.find('*')?;
    let (prefix, rest) = masked.split_at(star);
    let suffix = rest.trim_start_matches('*');
    if prefix.is_empty() || suffix.is_empty() {
        return None;
    }
    if !prefix.bytes().all(|byte| byte.is_ascii_digit())
        || !suffix.bytes().all(|byte| byte.is_ascii_digit())
    {
        return None;
    }
    Some((prefix.to_string(), suffix.to_string()))
}

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

// ============================================================================
// P8-5 G18：主库自检四级判定。判定先保持纯函数，真实文件读取只负责组装输入，
// 这样“健康 / 可自愈 / 需人工”不会被某一条读取路径悄悄改变。
// ============================================================================
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MasterSelfCheckLevel {
    Healthy,
    SelfHealable,
    NeedsManual,
}

impl MasterSelfCheckLevel {
    fn as_str(self) -> &'static str {
        match self {
            Self::Healthy => "healthy",
            Self::SelfHealable => "self_healable",
            Self::NeedsManual => "needs_manual",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MasterSelfCheckStatus {
    Passed,
    Attention,
    Failed,
    Blocked,
}

impl MasterSelfCheckStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Passed => "passed",
            Self::Attention => "attention",
            Self::Failed => "failed",
            Self::Blocked => "blocked",
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct MasterSelfCheckInput {
    storage_exists: bool,
    storage_readable: bool,
    blob_present: bool,
    blob_decryptable: bool,
    account_registered: bool,
    cache_consistent: bool,
    instance_stopped: bool,
    donor_available: bool,
    ledger_valid: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct MasterSelfCheckAssessment {
    level: MasterSelfCheckLevel,
    read_status: MasterSelfCheckStatus,
    consistency_status: MasterSelfCheckStatus,
    switchability_status: MasterSelfCheckStatus,
    deep_status: MasterSelfCheckStatus,
}

fn assess_master_self_check(input: MasterSelfCheckInput) -> MasterSelfCheckAssessment {
    let read_status = if !input.storage_exists {
        // 尚未登录不是故障，仍需让用户知道主库目前没有登录数据。
        MasterSelfCheckStatus::Attention
    } else if !input.storage_readable || !input.blob_decryptable || !input.account_registered {
        MasterSelfCheckStatus::Failed
    } else if !input.blob_present {
        MasterSelfCheckStatus::Attention
    } else {
        MasterSelfCheckStatus::Passed
    };
    let consistency_status = if input.cache_consistent {
        MasterSelfCheckStatus::Passed
    } else if input.blob_decryptable && input.account_registered {
        MasterSelfCheckStatus::Attention
    } else {
        MasterSelfCheckStatus::Blocked
    };
    let switchability_status = if !input.instance_stopped {
        MasterSelfCheckStatus::Blocked
    } else if input.donor_available {
        MasterSelfCheckStatus::Passed
    } else {
        MasterSelfCheckStatus::Attention
    };
    let deep_status = if input.ledger_valid {
        MasterSelfCheckStatus::Passed
    } else {
        MasterSelfCheckStatus::Failed
    };

    let statuses = [
        read_status,
        consistency_status,
        switchability_status,
        deep_status,
    ];
    let level = if statuses.contains(&MasterSelfCheckStatus::Failed) {
        MasterSelfCheckLevel::NeedsManual
    } else if statuses
        .iter()
        .any(|status| *status != MasterSelfCheckStatus::Passed)
    {
        MasterSelfCheckLevel::SelfHealable
    } else {
        MasterSelfCheckLevel::Healthy
    };

    MasterSelfCheckAssessment {
        level,
        read_status,
        consistency_status,
        switchability_status,
        deep_status,
    }
}

#[derive(Clone, Serialize)]
struct MasterSelfCheckItemDto {
    key: &'static str,
    status: &'static str,
    summary: String,
}

#[derive(Clone, Serialize)]
struct MasterSelfCheckDto {
    level: &'static str,
    checks: Vec<MasterSelfCheckItemDto>,
    current_account_name: Option<String>,
    observed_account_name: Option<String>,
    can_repair: bool,
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

/// 读取主库自检报告（G18）。文件读取失败只形成报告，不在自检阶段改写用户数据。
fn get_master_self_check_inner(
    material_root: &Path,
    storage_root: &Path,
) -> Result<MasterSelfCheckDto, String> {
    let registry = EnvironmentRegistry::new(storage_root);
    let record = registry
        .ensure_master()
        .map_err(|_| "environment_registry_invalid".to_string())?;
    let accounts = AccountRegistry::new(material_root)
        .load()
        .map_err(|_| "checkin_registry_invalid".to_string())?;
    let master_dir = master_data_dir().map_err(|_| "environment_registry_invalid".to_string())?;
    let storage_path = master_dir
        .join("User")
        .join("globalStorage")
        .join("storage.json");
    let storage_exists = storage_path.is_file();
    let (storage_readable, blob_present) = if !storage_exists {
        (true, false)
    } else {
        match std::fs::read_to_string(&storage_path)
            .ok()
            .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok())
        {
            Some(value) => (true, value.get("iCubeAuthInfo://usertag").is_some()),
            None => (false, false),
        }
    };
    let observed_user_id = if blob_present {
        archive_login_user_id(&master_dir)
    } else {
        None
    };
    let observed_profile_id = observed_user_id.as_deref().and_then(|user_id| {
        accounts
            .iter()
            .find(|account| account.account_id == user_id)
            .map(|account| account.profile_id.clone())
    });
    let cache_consistent = match (
        record.current_profile_id.as_deref(),
        observed_profile_id.as_deref(),
    ) {
        (None, None) => true,
        (Some(cached), Some(observed)) => cached == observed,
        _ => false,
    };
    let running = list_trae_processes()
        .map(|processes| {
            processes.iter().any(|process| {
                process.command_line.as_deref().is_some_and(|line| {
                    trae_instance_module::command_line_matches_master(line, &master_dir)
                })
            })
        })
        .unwrap_or(true);
    let donor_profile_id = observed_profile_id
        .as_deref()
        .or(record.current_profile_id.as_deref());
    let donor_available = donor_profile_id
        .map(|profile_id| {
            instance_data_dir(storage_root, profile_id)
                .map(|path| path.is_dir())
                .unwrap_or(false)
        })
        .unwrap_or(true);
    let ledger_valid = RelayLedger::new(storage_root.join("environments"))
        .load()
        .is_ok();
    let assessment = assess_master_self_check(MasterSelfCheckInput {
        storage_exists,
        storage_readable,
        blob_present,
        blob_decryptable: !blob_present || observed_user_id.is_some(),
        account_registered: !blob_present || observed_profile_id.is_some(),
        cache_consistent,
        instance_stopped: !running,
        donor_available,
        ledger_valid,
    });
    let current_account_name = record.current_profile_id.as_deref().and_then(|profile_id| {
        accounts
            .iter()
            .find(|account| account.profile_id == profile_id)
            .map(|account| {
                account
                    .display_name
                    .clone()
                    .unwrap_or_else(|| account.screen_name.clone())
            })
    });
    let observed_account_name = observed_profile_id.as_deref().and_then(|profile_id| {
        accounts
            .iter()
            .find(|account| account.profile_id == profile_id)
            .map(|account| {
                account
                    .display_name
                    .clone()
                    .unwrap_or_else(|| account.screen_name.clone())
            })
    });
    let read_summary = if !storage_exists {
        "主库尚未产生登录数据"
    } else if !storage_readable {
        "登录数据文件无法读取"
    } else if !blob_present {
        "主库当前未登录账号"
    } else if observed_user_id.is_none() {
        "登录数据已损坏，需重新登录"
    } else if observed_profile_id.is_none() {
        "当前登录账号尚未登记"
    } else {
        "登录数据可读取，账号可反查"
    };
    let consistency_summary = if cache_consistent {
        "工具记录与实际登录一致"
    } else if observed_profile_id.is_some() {
        "实际登录账号与工具记录不一致"
    } else {
        "暂时无法确认主库当前账号"
    };
    let switchability_summary = if running {
        "请先关闭主库 TRAE 窗口"
    } else if donor_available {
        "主库已关闭，可进行切换"
    } else {
        "当前账号没有可用的登录存档"
    };
    let deep_summary = if ledger_valid {
        "接力记录可读取，展开后可进行深度核对"
    } else {
        "接力记录不可读取，需要人工保留数据后处理"
    };
    Ok(MasterSelfCheckDto {
        level: assessment.level.as_str(),
        checks: vec![
            MasterSelfCheckItemDto {
                key: "read",
                status: assessment.read_status.as_str(),
                summary: read_summary.to_string(),
            },
            MasterSelfCheckItemDto {
                key: "consistency",
                status: assessment.consistency_status.as_str(),
                summary: consistency_summary.to_string(),
            },
            MasterSelfCheckItemDto {
                key: "switchability",
                status: assessment.switchability_status.as_str(),
                summary: switchability_summary.to_string(),
            },
            MasterSelfCheckItemDto {
                key: "deep",
                status: assessment.deep_status.as_str(),
                summary: deep_summary.to_string(),
            },
        ],
        current_account_name,
        observed_account_name,
        can_repair: observed_profile_id.is_some() && !cache_consistent,
    })
}

/// 读取主库自检报告；fixture 模式不触碰真实目录。
#[tauri::command]
async fn get_master_self_check(
    state: tauri::State<'_, AppState>,
) -> Result<MasterSelfCheckDto, String> {
    ensure_real_preview(&state)?;
    let material_root = checkin_material_root(&state)?;
    let storage_root = PathBuf::from(&state.storage_root);
    run_blocking("master_self_check_join_failed", move || {
        get_master_self_check_inner(&material_root, &storage_root)
    })
    .await
}

/// 把自检观测到的账号写回环境缓存；只修正工具记录，不修改 TRAE 登录数据。
#[tauri::command]
async fn repair_master_current_account(state: tauri::State<'_, AppState>) -> Result<bool, String> {
    ensure_real_preview(&state)?;
    let material_root = checkin_material_root(&state)?;
    let storage_root = PathBuf::from(&state.storage_root);
    run_blocking("master_self_check_join_failed", move || {
        let registry = EnvironmentRegistry::new(&storage_root);
        let _ = registry
            .ensure_master()
            .map_err(|_| "environment_registry_invalid".to_string())?;
        let accounts = AccountRegistry::new(&material_root)
            .load()
            .map_err(|_| "checkin_registry_invalid".to_string())?;
        let master_dir =
            master_data_dir().map_err(|_| "environment_registry_invalid".to_string())?;
        let observed = observed_profile_id(&accounts, &master_dir)
            .ok_or_else(|| "master_observed_account_unavailable".to_string())?;
        registry
            .set_current_profile(MASTER_ENV_ID, &observed)
            .map_err(|_| "environment_registry_invalid".to_string())
    })
    .await
}

/// 强制把主库的工具缓存指向指定账号。该兜底不修复 TRAE 登录数据，且必须在
/// 主库停止后由前端完成一次明确确认，避免把运行中的主库状态继续写乱。
#[tauri::command]
async fn force_reset_master_current_account(
    profile_id: String,
    state: tauri::State<'_, AppState>,
) -> Result<bool, String> {
    ensure_real_preview(&state)?;
    let material_root = checkin_material_root(&state)?;
    let storage_root = PathBuf::from(&state.storage_root);
    run_blocking("master_self_check_join_failed", move || {
        let _ = trae_instance_account_id(&material_root, &profile_id)?;
        let master_dir =
            master_data_dir().map_err(|_| "environment_registry_invalid".to_string())?;
        let running = list_trae_processes()
            .map_err(|_| "master_self_check_process_unavailable".to_string())?
            .iter()
            .any(|process| {
                process.command_line.as_deref().is_some_and(|line| {
                    trae_instance_module::command_line_matches_master(line, &master_dir)
                })
            });
        if running {
            return Err("master_self_check_instance_running".to_string());
        }
        EnvironmentRegistry::new(&storage_root)
            .set_current_profile(MASTER_ENV_ID, &profile_id)
            .map_err(|_| "environment_registry_invalid".to_string())
    })
    .await
}

// ===== P5-1 主库切号五步事务（环境模型 Q1.1，编排层）=====

/// 主库切号进度事件（`master-switch-progress`；账号页切号弹层 P5-2 消费）。
///
/// 阶段与 Q1.1 五步一一对应（Q10 修正后的顺序）：
/// closing → backing_up → switching_login → handing_over → restarting → done。
/// P7-3：handing_over 阶段附 progress 明细（映射/执行/校验逐项上报，
/// 字段可选，向后兼容无明细的旧事件）。
#[derive(Clone, Serialize)]
struct MasterSwitchProgressEvent {
    profile_id: String,
    stage: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    progress: Option<MasterSwitchProgressDetail>,
}

/// handing_over 阶段的细粒度进度（P7-3；label 为项目名等用户可读文本）。
#[derive(Clone, Serialize)]
struct MasterSwitchProgressDetail {
    /// 交接内部阶段：mapping / executing / verifying。
    phase: String,
    current: usize,
    total: usize,
    label: String,
}

/// P7-2 切号失败回滚结果事件（`master-switch-rolled-back`）：
/// rolled_back = true → 前端失败文案附「已自动还原，可安全重试」；
/// false → 前端指向第 2 步备份链（错误码 master_switch_rollback_failed）。
#[derive(Clone, Serialize)]
struct MasterSwitchRolledBackEvent {
    profile_id: String,
    rolled_back: bool,
}

/// 主库切号结果 DTO（五步全部完成后的回执）。
#[derive(Clone, Serialize)]
struct MasterAccountSwitchDto {
    profile_id: String,
    /// 切换后主库登录账号的 TRAE user_id（= 账号注册表 account_id）。
    to_user_id: String,
    /// 切换前主库实际登录账号（凭据互换时主库 blob 内的 userId）；
    /// 供前端展示「A → B」交接轨迹。
    from_user_id: Option<String>,
    /// 归属随行的 project 行数（Q3 全量随行）。
    transferred_projects: usize,
    /// 自动清理的目标账号空镜像行数（E5 坑位）。
    removed_mirror_rows: usize,
    /// 本次换腿（交接）的会话数。
    switched_sessions: usize,
    /// 主库数据备份文件路径（三件套 .switch-bak-* 副本；人工恢复定位用）。
    backup_path: String,
    /// 接力台账写入结果：false = 交接已完成但台账追加失败
    /// （只影响历史页轨迹展示，不影响切号结果）。
    relay_ledger_written: bool,
    /// 云端插件预同步回执（fail-soft：aborted/failed 只影响插件市场
    /// 显示，不影响切号结果；目标账号可手动重装）。
    plugin_sync: PluginCloudSyncOutcome,
    /// 主库重启结果：launched=已重启（close 后必非 focused）。
    relaunch_outcome: String,
}

/// 主库切号（P5-1，Q1.1 全自动五步事务）：
/// 关实例 → 三件套备份 → 登录凭据互换 → 记录交接 → 重启。
///
/// - `force`：Q1.2 生成中切换——检测到主库 DB 活跃（回复生成中）时，
///   非 force 返回 `master_switch_busy` 交前端弹窗「等它完成还是强制切换」；
///   force=true 中断回复按失败轮次保留（备份先行，不丢弃证据）。
/// - 插件对账（第 4.5 步）按 ADR-0026 静默执行：差异直达目标账号云端；
///   含移除的差异由前端预检弹一次确认后才走到这里（纯新增不确认）。
/// - 前置条件：主库已启动并登录过一次（Q2）；目标账号实例已登录过
///   （供体登录凭据来源）。
#[tauri::command]
async fn switch_master_account(
    profile_id: String,
    force: bool,
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
) -> Result<MasterAccountSwitchDto, String> {
    ensure_real_preview(&state)?;
    if state.source_raw_key.is_empty() {
        return Err("source_key_unavailable".to_string());
    }
    let material_root = checkin_material_root(&state)?;
    let storage_root = PathBuf::from(&state.storage_root);
    let raw_key = state.source_raw_key.clone();
    // 切号尝试日志需要（storage_root 被下方闭包 move 走，先克隆）。
    let journal_storage_root = storage_root.clone();
    let journal_profile_id = profile_id.clone();
    // 阻塞执行：关实例（最长 5 秒收口）、WAL 采样、文件复制与
    // 数据库事务均为阻塞 IO/长等待。
    let inner = run_blocking(
        "master_switch_join_failed",
        move || {
            switch_master_account_inner(
                &app,
                &material_root,
                &storage_root,
                &raw_key,
                &profile_id,
                force,
            )
        },
    )
    .await;
    // 切号尝试日志：成功/失败各记一行（fail-soft，不影响切号结果）。
    let outcome_code = match &inner {
        Ok(_) => "ok",
        Err(code) => code.as_str(),
    };
    journal_switch_attempt(&journal_storage_root, &journal_profile_id, outcome_code);
    inner
}

/// 切号尝试日志（2026-09-01 稳健性补强）：每次切号一行 JSONL 追加到
/// `data/master-switch-journal.jsonl`（时间 / 目标 / 结果错误码）。
/// 此前应用无任何持久化日志，切号连续失败只能靠备份链与主库日志考古；
/// 有此日志后失败原因可直接定位。fail-soft：写失败静默跳过。
fn journal_switch_attempt(storage_root: &Path, profile_id: &str, outcome: &str) {
    use std::io::Write;
    let at_unix_seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0);
    let entry = serde_json::json!({
        "at_unix_seconds": at_unix_seconds,
        "profile_id": profile_id,
        "outcome": outcome,
    });
    let path = storage_root.join("master-switch-journal.jsonl");
    let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    else {
        return;
    };
    let _ = writeln!(file, "{entry}");
}

/// P7-1 切号第 3 步双路径编排（ADR-0024 决策 4：凭据包构造优先、
/// 存档移植降级）。
///
/// E2 首选：凭据包（DPAPI 解密）+ GetUserInfo 实调（只读）构造登录态。
/// 身份校验（服务端 user_id == 凭据包 account_id）前置——不通过绝不
/// 写库（E2 纪律），降级走 E1。
///
/// 降级触发：凭据包缺失/损坏、GetUserInfo 网络失败、身份不一致、
/// 构造写回失败——全部静默降级 E1 存档移植（主库侧错误 E1 也会同样
/// 失败，幂等无害）。
///
/// SameAccount 处置（2026-09-01 死锁修复）：主库已登录目标账号时
/// **不再报错中止**，而是跳过登录互换（文件不动）、继续执行第 4 步
/// 记录交接——否则记录归属永远停在旧账号名下，TRAE 全部历史不可见。
pub(crate) fn switch_login_identity_dual_path(
    material_root: &Path,
    record: &traesync_infrastructure::account_registry::AccountRecord,
    donor_dir: &Path,
    master_dir: &Path,
) -> Result<AuthIdentitySwitch, AuthSwitchError> {
    // ===== E2 首选：凭据包 + GetUserInfo 实调构造 =====
    let store = CheckinCredentialStore::new(material_root);
    let binding = CheckinProfileBinding::from(record);
    let e2_result = store
        .load(&binding)
        .map_err(|_| AuthSwitchError::DonorAuthUnavailable)
        .and_then(|bundle| {
            // GetUserInfo 实调（只读查询，与签到身份校验同源同路）。
            get_user_info_full(&trae_http_client(), &bundle.access_token)
                .map(|info| (bundle, info))
                .map_err(|_| AuthSwitchError::DonorAuthUnavailable)
        })
        .and_then(|(bundle, info)| {
            // 身份不一致 = 凭据包不可信（可能被吊销/错位），不写库降级。
            if info.user_id != bundle.account_id {
                return Err(AuthSwitchError::DonorAuthUnavailable);
            }
            let input = ConstructAuthInput {
                account_id: &bundle.account_id,
                access_token: &bundle.access_token,
                refresh_token: &bundle.refresh_token,
                access_token_expires_at_unix_seconds: bundle.access_token_expires_at_unix_seconds,
                refresh_token_expires_at_unix_seconds: bundle.refresh_token_expires_at_unix_seconds,
                user_info: &info,
            };
            construct_auth_identity(&input, master_dir)
        });

    match e2_result {
        Ok(identity) => Ok(identity),
        // E2 SameAccount：主库已是目标账号，且身份已凭据包绑定 +
        // GetUserInfo 服务端验证（bundle.account_id == record.account_id，
        // 链路在上方绑定构造）——证据充分，跳过互换继续交接。
        Err(AuthSwitchError::SameAccount) => Ok(same_account_identity(record)),
        // 其余失败降级 E1：存档三件套明文移植（blob 自证，不依赖网络）。
        Err(_) => match switch_auth_identity(donor_dir, master_dir) {
            Ok(identity) => Ok(identity),
            // E1 SameAccount：主库登录 = 供体存档账号，但存档可能过期
            // 错位（实例曾登录过其他账号）——存档账号与注册表目标账号
            // 一致才跳过互换；不一致维持报错（宁可不切，不可切错）。
            Err(AuthSwitchError::SameAccount)
                if archive_login_user_id(donor_dir).as_deref()
                    == Some(record.account_id.as_str()) =>
            {
                Ok(same_account_identity(record))
            }
            Err(error) => Err(error),
        },
    }
}

/// 主库已是目标账号时的恒等交换（登录互换跳过，文件不动）：
/// from = to = 目标账号。台账 from 优先取交接实测的 previous_owner，
/// 不受此恒等值影响；DTO/插件同步据此走「无差异」路径，语义自洽。
fn same_account_identity(
    record: &traesync_infrastructure::account_registry::AccountRecord,
) -> AuthIdentitySwitch {
    let account_id = record.account_id.clone();
    AuthIdentitySwitch {
        from_user_id: account_id.clone(),
        to_user_id: account_id.clone(),
        to_account: account_id,
    }
}

/// 五步编排主体（阻塞上下文；每步失败即中断并返回对应错误码）。
fn switch_master_account_inner(
    app: &tauri::AppHandle,
    material_root: &Path,
    storage_root: &Path,
    raw_key: &str,
    profile_id: &str,
    force: bool,
) -> Result<MasterAccountSwitchDto, String> {
    // 前置：目标账号在注册表（account_id 即 TRAE user_id，交接与台账都要用）。
    let records = AccountRegistry::new(material_root)
        .load()
        .map_err(|_| "checkin_registry_invalid".to_string())?;
    let record = records
        .iter()
        .find(|record| record.profile_id == profile_id)
        .ok_or_else(|| "trae_profile_invalid".to_string())?;
    let target_user_id = record.account_id.clone();
    let window_title = record
        .display_name
        .clone()
        .unwrap_or_else(|| record.screen_name.clone());

    // 主库 = 官方目录（2026-08-31 修订）：五步事务直接作用于官方数据目录。
    let master_dir = master_data_dir().map_err(|_| "environment_registry_invalid".to_string())?;
    let db_path = master_database_path(&master_dir);
    let donor_dir =
        instance_data_dir(storage_root, profile_id).map_err(|e| e.code().to_string())?;
    let emit = |stage: &'static str| {
        let _ = app.emit(
            "master-switch-progress",
            MasterSwitchProgressEvent {
                profile_id: profile_id.to_string(),
                stage,
                progress: None,
            },
        );
    };

    // Q1.2 生成中检测：双采样主库 WAL（间隔 800ms），活跃且未强制 → 报忙。
    // 检测在关实例之前（此刻实例可能还在写库，采样才有意义）。
    if db_path.is_file()
        && master_db_activity_detected(&db_path, std::time::Duration::from_millis(800))
        && !force
    {
        return Err("master_switch_busy".to_string());
    }

    // 第 1 步：关闭主库实例（幂等；优雅 3s + 强制兜底，数据无损已验证。
    // 主库 = 官方目录：用户官方启动的实例同样要能关掉）。
    emit("closing");
    close_master_instance(&master_dir).map_err(|_| "master_switch_close_failed".to_string())?;

    // 第 2 步：三件套备份（ADR-0018 铁律：破坏性批量操作前先备份；
    // create_new 保证永不覆盖既有备份链）。
    emit("backing_up");
    // P11-4①：写入当前活动备份目录（backup_dir 覆盖或 db 同目录）。
    let backup_dir = resolve_master_backup_dir(storage_root, &db_path);
    let backup_path = backup_master_trio_to(&db_path, Some(&backup_dir)).map_err(|error| match error {
        MasterHandoverError::DbUnavailable => "master_switch_db_missing".to_string(),
        _ => "master_switch_backup_failed".to_string(),
    })?;
    // P5-9：备份生成后按保留策略清理旧备份（静默容错，不影响切号流程）。
    prune_backups_if_enabled(&db_path, storage_root);

    // P7-2 回滚材料：第 3 步改写前的主库登录态原始字节。读不到（文件
    // 缺失/无登录键）时第 3 步必然报 TargetAuthUnavailable 而不会写库，
    // 无需回滚（主库天然处于原状）。
    let master_storage_path = master_dir
        .join("User")
        .join("globalStorage")
        .join("storage.json");
    let master_login_raw_before = std::fs::read_to_string(&master_storage_path).ok();
    let fail_with_rollback = |code: &str| {
        finish_switch_failure_with_rollback(
            app,
            profile_id,
            &master_storage_path,
            master_login_raw_before.as_deref(),
            code,
        )
    };

    // 第 3 步：登录凭据互换（P7-1 双路径，ADR-0024 决策 4）：
    // E2 首选——凭据包 + GetUserInfo 实调构造登录态（新账号零存档依赖）；
    // 降级 E1——存档三件套明文移植（GetUserInfo 失败/凭据包缺失/身份不一致时）。
    // 双路径均失败才报错（switch_donor_login_missing = 凭据与存档都不可用）。
    // P7-2：写库后失败（VerifyFailed 等）由 fail_with_rollback 还原原始字节。
    emit("switching_login");
    let identity =
        match switch_login_identity_dual_path(material_root, record, &donor_dir, &master_dir) {
            Ok(identity) => identity,
            Err(error) => {
                let code = match error {
                    AuthSwitchError::DonorAuthUnavailable => "switch_donor_login_missing",
                    AuthSwitchError::TargetAuthUnavailable => "master_login_missing",
                    AuthSwitchError::SameAccount => "switch_same_account",
                    AuthSwitchError::VerifyFailed => "switch_verify_failed",
                    _ => "switch_auth_failed",
                };
                return Err(fail_with_rollback(code));
            }
        };

    // 第 4 步：记录交接（归属随行 + 活跃会话原地换腿，单事务；
    // 非空 UNIQUE 冲突报人工决策，禁止静默覆盖）。
    // P7-2：事务原子失败时记录归属未动，但登录身份已换（杂交态）——
    // fail_with_rollback 立即还原主库登录态，掐死卡死态（LY 案例根因）。
    // P7-3：交接进度逐项上报（映射/执行/校验，label 用项目名）。
    emit("handing_over");
    let handover = match handover_master_records_with_progress(
        &db_path,
        raw_key,
        &target_user_id,
        &|progress: HandoverProgress| {
            let _ = app.emit(
                "master-switch-progress",
                MasterSwitchProgressEvent {
                    profile_id: profile_id.to_string(),
                    stage: "handing_over",
                    progress: Some(MasterSwitchProgressDetail {
                        phase: progress.phase.to_string(),
                        current: progress.current,
                        total: progress.total,
                        label: progress.label,
                    }),
                },
            );
        },
    ) {
        Ok(handover) => handover,
        Err(error) => {
            let code = match error {
                MasterHandoverError::DbUnavailable => "master_switch_db_missing",
                MasterHandoverError::TargetConflict(_) => "master_switch_conflict",
                MasterHandoverError::IntegrityFailed => "master_switch_integrity_failed",
                _ => "master_switch_db_open_failed",
            };
            return Err(fail_with_rollback(code));
        }
    };

    // 环境档案写回当前账号（App 重启后环境页与主库窗口标题据此恢复）。
    // P7-2：改 fail-soft——此刻登录态与记录归属已一致切到新账号，回滚
    // 反而制造新杂交态；档案写失败只影响下次启动的显示（档案有默认回退）。
    if let Err(error) =
        EnvironmentRegistry::new(storage_root).set_current_profile(MASTER_ENV_ID, profile_id)
    {
        eprintln!("[master-switch] 环境档案写回失败（不阻断）：{error:?}");
    }

    // 接力台账追加（交接成功后）：失败不阻断切号——台账只影响历史页
    // 轨迹展示，缺失可从备份核对（relay_ledger 模块契约）。
    let relay_ledger_written = append_relay_ledger(
        storage_root,
        &handover,
        &identity.from_user_id,
        &target_user_id,
        profile_id,
    );

    // 第 4.5 步：云端插件对账（ADR-0026 决策 3 静默应用，fail-soft）：
    // 吸收切换前账号云端现状进环境清单，再把差异静默应用到目标账号
    // ——缺的装上、多的卸掉，重启后 TRAE 调和本地安装时无事发生。
    // 含移除的差异已在切号弹层预检确认（单次）；任何失败都不阻断切号
    // ——插件可手动重装，切号本身必须完成。
    emit("syncing_plugins");
    let plugin_sync = sync_plugins_for_switch(
        material_root,
        storage_root,
        &records,
        &identity.from_user_id,
        record,
    );

    // 第 5 步：重启主库（新登录身份 + 新窗口标题；官方目录带参启动，
    // 与用户官方快捷方式启动等价——同一 data_dir）。
    emit("restarting");
    let relaunch = launch_instance_common(
        &master_dir,
        &window_title,
        None,
        trae_instance_module::command_line_matches_master,
    )?;
    emit("done");

    Ok(MasterAccountSwitchDto {
        profile_id: profile_id.to_string(),
        to_user_id: identity.to_user_id,
        from_user_id: Some(identity.from_user_id),
        transferred_projects: handover.transferred_projects,
        removed_mirror_rows: handover.removed_mirror_rows,
        switched_sessions: handover.switched_sessions.len(),
        backup_path: backup_path.display().to_string(),
        relay_ledger_written,
        plugin_sync,
        relaunch_outcome: relaunch.outcome.to_string(),
    })
}

/// P7-2 回滚核心：主库登录态原始字节整体写回 + 读回比对。
/// 字节一致 ⇒ 加密材料与内容均未变（字节级验证强于解密验证）。
pub(crate) fn rollback_master_login_bytes(master_storage_path: &Path, original: &str) -> bool {
    std::fs::write(master_storage_path, original)
        .and_then(|_| std::fs::read_to_string(master_storage_path))
        .map(|written_back| written_back == original)
        .unwrap_or(false)
}

/// P7-2 切号失败收尾：还原第 3 步改写前的主库登录态原始字节，
/// 掐死「登录身份 = 新账号、记录归属 = 旧账号」的杂交态（LY 案例根因）。
///
/// 三种结局：
/// 1. 无回滚材料（第 3 步未写库，读不到原始字节）→ 主库天然原状，
///    直接返回原错误码，不发事件；
/// 2. 写回成功 → 发 rolled_back=true 事件，返回原错误码，
///    前端文案附「已自动还原，可安全重试」；
/// 3. 写回失败 → 发 rolled_back=false 事件，返回
///    master_switch_rollback_failed（前端指向第 2 步备份链）。
fn finish_switch_failure_with_rollback(
    app: &tauri::AppHandle,
    profile_id: &str,
    master_storage_path: &Path,
    original_raw: Option<&str>,
    code: &str,
) -> String {
    // 结局 1：第 3 步从未写库（storage.json 缺失或读取失败），
    // 主库天然处于切换前状态。
    let Some(original) = original_raw else {
        return code.to_string();
    };

    let rolled_back = rollback_master_login_bytes(master_storage_path, original);

    let _ = app.emit(
        "master-switch-rolled-back",
        MasterSwitchRolledBackEvent {
            profile_id: profile_id.to_string(),
            rolled_back,
        },
    );

    if rolled_back {
        code.to_string()
    } else {
        "master_switch_rollback_failed".to_string()
    }
}

/// 切号编排的云端插件对账入口（ADR-0026 决策 3 静默应用，fail-soft）：
/// 源 = 切换前主库登录账号（凭据互换观察到的 from_user_id），
/// 目标 = 切换目标账号。含移除的差异已由前端预检弹一次确认，这里
/// 始终执行完整对账（吸收后应用）。源账号不在注册表或任一凭据包
/// 读取失败（含 token 失效场景由列表拉取 401 兜底）→ aborted 回执，
/// 不报错。
fn sync_plugins_for_switch(
    material_root: &Path,
    storage_root: &Path,
    records: &[traesync_infrastructure::account_registry::AccountRecord],
    from_user_id: &str,
    target_record: &traesync_infrastructure::account_registry::AccountRecord,
) -> PluginCloudSyncOutcome {
    let aborted = || PluginCloudSyncOutcome {
        source_count: 0,
        installed: 0,
        removed: 0,
        failed: 0,
        skipped: 0,
        aborted: true,
        absorbed: Vec::new(),
    };
    // 源账号：主库切换前实际登录的账号（可能是未注册进 App 的账号）
    let Some(source_record) = records.iter().find(|r| r.account_id == from_user_id) else {
        return aborted();
    };
    let store = CheckinCredentialStore::new(material_root);
    let load_token = |record: &traesync_infrastructure::account_registry::AccountRecord| {
        store
            .load(&CheckinProfileBinding::from(record))
            .map(|bundle| bundle.access_token)
    };
    let (Ok(source_token), Ok(target_token)) =
        (load_token(source_record), load_token(target_record))
    else {
        return aborted();
    };
    let outcome = sync_account_cloud_plugins(&source_token, &target_token);
    // 吸收集落盘环境清单（ADR-0023「吸收」：切换前账号云端现状即基线）。
    // 写失败 fail-soft：清单滞后可在插件 tab 手动吸收补齐，不影响切号回执。
    if !outcome.aborted {
        let entries = manifest_entries_from_cloud(&outcome.absorbed);
        let _ = PluginManifest::new(storage_root).save(&entries);
    }
    outcome
}

/// 切号插件差异预检 DTO（ADR-0026 决策 3：remove_names 非空时前端弹
/// 一次移除确认；纯新增差异静默应用，不弹确认）。
#[derive(Clone, Serialize)]
struct MasterSwitchPluginPreviewDto {
    /// 源账号云端市场插件数（吸收后的清单基数）。
    source_count: usize,
    /// 目标账号云端市场插件数。
    target_count: usize,
    /// 待安装到目标账号的插件名（纯新增，静默应用不确认）。
    install_names: Vec<String>,
    /// 待从目标账号移除的插件名（破坏性差异单列；非空才弹确认）。
    remove_names: Vec<String>,
    /// 预检未完成（环境档案/注册表/凭据/列表任一不可用）：
    /// 前端据此静默直过切号，不弹确认。
    aborted: bool,
}

/// 切号插件差异预检（P5-8b-3）：源 = 主库登录态实测当前账号（与插件 tab 同源），
/// 目标 = 切换目标账号；对账计划与执行侧共用 `reconcile_plan`，保证
/// 「确认的移除清单 = 实际应用的移除」。预检永不返回 Err（fail-soft）。
#[tauri::command]
async fn preview_master_switch_plugins(
    profile_id: String,
    state: tauri::State<'_, AppState>,
) -> Result<MasterSwitchPluginPreviewDto, String> {
    ensure_real_preview(&state)?;
    let material_root = checkin_material_root(&state)?;
    let storage_root = PathBuf::from(&state.storage_root);
    run_blocking("plugin_preview_join_failed", move || {
        Ok(preview_master_switch_plugins_inner(
            &material_root,
            &storage_root,
            &profile_id,
        ))
    })
    .await
}

/// 双方云端列表 → 预检 DTO（纯函数，tests 复用）：吸收集口径与执行侧
/// 一致（非 builtin 且有市场 ID）；remove_names 非空 = 需要移除确认。
fn switch_plugin_preview_from_lists(
    source_items: &[CloudPluginItem],
    target_items: &[CloudPluginItem],
) -> MasterSwitchPluginPreviewDto {
    let source_market: Vec<CloudPluginItem> = source_items
        .iter()
        .filter(|item| !item.builtin && item.marketplace_plugin_id.is_some())
        .cloned()
        .collect();
    let (install, remove) = reconcile_plan(&source_market, target_items);
    let display_of = |item: &CloudPluginItem| {
        let fallback = item
            .marketplace_plugin_id
            .clone()
            .unwrap_or_else(|| item.record_id.clone());
        if !item.display_name.is_empty() {
            item.display_name.clone()
        } else if !item.name.is_empty() {
            item.name.clone()
        } else {
            fallback
        }
    };
    MasterSwitchPluginPreviewDto {
        source_count: source_market.len(),
        target_count: target_items
            .iter()
            .filter(|item| !item.builtin && item.marketplace_plugin_id.is_some())
            .count(),
        install_names: install.iter().map(display_of).collect(),
        remove_names: remove.iter().map(display_of).collect(),
        aborted: false,
    }
}

/// 预检主体（阻塞上下文）：拉取双方云端列表 → 对账计划 → 差异名集合。
fn preview_master_switch_plugins_inner(
    material_root: &Path,
    storage_root: &Path,
    profile_id: &str,
) -> MasterSwitchPluginPreviewDto {
    let aborted = MasterSwitchPluginPreviewDto {
        source_count: 0,
        target_count: 0,
        install_names: Vec::new(),
        remove_names: Vec::new(),
        aborted: true,
    };
    // 源账号必须来自主库登录态实测；环境档案只作为展示缓存。
    let Ok(master_dir) = master_data_dir() else {
        return aborted;
    };
    let Ok(Some(source)) = resolve_actual_master_account(material_root, storage_root, &master_dir)
    else {
        return aborted;
    };
    let records = match AccountRegistry::new(material_root).load() {
        Ok(records) => records,
        Err(_) => return aborted,
    };
    let Some(target) = records.iter().find(|r| r.profile_id == profile_id) else {
        return aborted;
    };
    // 同一账号无从谈差异（switch_same_account 由执行侧兜底，这里直过）。
    if source.profile_id == target.profile_id {
        return aborted;
    }
    let store = CheckinCredentialStore::new(material_root);
    let load_token = |record: &traesync_infrastructure::account_registry::AccountRecord| {
        store
            .load(&CheckinProfileBinding::from(record))
            .map(|bundle| bundle.access_token)
    };
    let (Ok(source_token), Ok(target_token)) = (load_token(&source), load_token(target)) else {
        return aborted;
    };
    let (Ok(source_items), Ok(target_items)) = (
        fetch_installed_plugins(&source_token),
        fetch_installed_plugins(&target_token),
    ) else {
        return aborted;
    };
    switch_plugin_preview_from_lists(&source_items, &target_items)
}

/// 交接成功后的接力台账追加（Q3/Q5）：无会话交接视为无需记录（true）；
/// 追加失败返回 false 交 DTO 透出，不阻断切号主流程。
fn append_relay_ledger(
    storage_root: &Path,
    handover: &MasterHandover,
    fallback_from_user_id: &str,
    to_user_id: &str,
    to_profile_id: &str,
) -> bool {
    if handover.switched_sessions.is_empty() {
        return true;
    }
    // 交接前账号优先取实际归属（Q3 不变量下 = 切换前登录账号）；
    // 无归属记录时回退凭据互换观察到的主库旧登录身份。
    let from_user_id = handover
        .previous_owner_user_id
        .clone()
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| fallback_from_user_id.to_string());
    let switched_at_unix_seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0);
    let entries: Vec<RelayLedgerEntry> = handover
        .switched_sessions
        .iter()
        .map(|session| RelayLedgerEntry {
            session_id: session.session_id.clone(),
            from_session_id: Some(session.previous_session_id.clone()),
            project_id: session.project_id.clone(),
            from_user_id: from_user_id.clone(),
            to_user_id: to_user_id.to_string(),
            to_profile_id: to_profile_id.to_string(),
            message_count_at_switch: session.message_count,
            switched_at_unix_seconds,
        })
        .collect();
    RelayLedger::new(storage_root.join("environments"))
        .append(&entries)
        .is_ok()
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

/// U-6 W4 单账号本地数据占用（字节）：实例目录 + 原生账号目录两部分。
/// 目录缺失记 0（未启动过实例 / 原生客户端未登录过该账号）。
#[derive(Clone, Serialize)]
struct AccountStorageFootprintDto {
    profile_id: String,
    /// 实例数据目录（`{storage_root}/trae-instances/{profile_id}`）递归大小。
    instance_dir_bytes: u64,
    /// 原生账号目录（`{APPDATA}/TRAE SOLO CN_{account_id}`）递归大小；
    /// 档案缺失或 account_id 非纯数字时记 0（无目录可定位）。
    native_dir_bytes: u64,
}

/// 批量查询账号本地数据占用（U-6 W4 归档筛选展示与彻底删除确认弹窗）。
/// 只读统计，不发起网络请求；供前端展示「归档账号占用多少空间」。
#[tauri::command]
async fn get_account_storage_footprint(
    profile_ids: Vec<String>,
    state: tauri::State<'_, AppState>,
) -> Result<Vec<AccountStorageFootprintDto>, String> {
    ensure_real_preview(&state)?;
    for profile_id in &profile_ids {
        if profile_id.is_empty() || profile_id.len() > 256 {
            return Err("checkin_profile_invalid".to_string());
        }
    }
    let material_root = checkin_material_root(&state)?;
    let storage_root = PathBuf::from(&state.storage_root);
    // spawn_blocking：递归目录大小是阻塞 IO（实例目录可达 GB 级）。
    run_blocking("footprint_join_failed", move || {
        // 原生目录依赖 APPDATA（与 seed_login_state 同一解析方式）。
        let appdata = std::env::var_os("APPDATA")
            .map(PathBuf::from)
            .unwrap_or_default();
        get_account_storage_footprint_inner(&material_root, &storage_root, &appdata, &profile_ids)
    })
    .await
}

fn get_account_storage_footprint_inner(
    material_root: &Path,
    storage_root: &Path,
    appdata: &Path,
    profile_ids: &[String],
) -> Result<Vec<AccountStorageFootprintDto>, String> {
    let records = AccountRegistry::new(material_root)
        .load()
        .map_err(|_| "checkin_registry_invalid".to_string())?;
    Ok(profile_ids
        .iter()
        .map(|profile_id| {
            // 实例目录：profile_id 非白名单（路径穿越防御）时按 0 处理，
            // 与 get_trae_instance_states 的宽容口径一致。
            let instance_dir_bytes = instance_data_dir(storage_root, profile_id)
                .map(|dir| dir_size_recursive(&dir))
                .unwrap_or(0);
            // 原生目录：需账号档案的 account_id 定位；档案缺失记 0。
            let native_dir_bytes = records
                .iter()
                .find(|record| &record.profile_id == profile_id)
                .and_then(|record| native_account_dir(appdata, &record.account_id))
                .map(|dir| dir_size_recursive(&dir))
                .unwrap_or(0);
            AccountStorageFootprintDto {
                profile_id: profile_id.clone(),
                instance_dir_bytes,
                native_dir_bytes,
            }
        })
        .collect())
}

/// 彻底删除账号本地数据记录（U-6 W4 资产库三态筛选，用户显式发起的
/// 破坏性命令）：实例数据目录 + 原生账号目录 + 会话索引缓存三处。
///
/// 守卫（后端只做守卫，不做自动触发；确认与二次确认由前端引导）：
/// - 账号不存在拒绝（无档案即无 account_id 可定位原生目录）；
/// - 实例运行中一律拒绝（复用进程观测逻辑，防删除运行中数据损坏库文件）；
/// - 执行前备份主库三件套（ADR-0018 备份铁律，P11-4③）；备份失败拒绝执行；
/// - 归档与否不做强制（由前端引导）。
///
/// 注意：本命令不删注册表档案与凭据包——那是 remove_checkin_account
/// （删账号默认只删身份保留记录）的职责，两者是独立动作。
#[tauri::command]
async fn purge_account_records(
    profile_id: String,
    state: tauri::State<'_, AppState>,
) -> Result<(), String> {
    ensure_real_preview(&state)?;
    if profile_id.is_empty() || profile_id.len() > 256 {
        return Err("checkin_profile_invalid".to_string());
    }
    let material_root = checkin_material_root(&state)?;
    let storage_root = PathBuf::from(&state.storage_root);
    // spawn_blocking：进程查询（PowerShell）与递归删除都是阻塞 IO。
    run_blocking("purge_join_failed", move || {
        // 破坏性命令 fail-closed：进程查询失败即拒绝（无法证明实例未运行），
        // 与 launch_master_library 同口径（非 get_trae_instance_states 的宽容降级）。
        let processes = list_trae_processes().map_err(|e| e.code().to_string())?;
        let appdata = std::env::var_os("APPDATA")
            .map(PathBuf::from)
            .unwrap_or_default();
        // P11-4③：主库三件套备份定位（与切号/收编同源；解析失败 fail-closed 拒绝）。
        let master_db_path = master_data_dir()
            .map(|dir| master_database_path(&dir))
            .map_err(|_| "environment_registry_invalid".to_string())?;
        purge_account_records_inner(
            &material_root,
            &storage_root,
            &appdata,
            &master_db_path,
            &profile_id,
            &processes,
        )
    })
    .await
}

fn purge_account_records_inner(
    material_root: &Path,
    storage_root: &Path,
    appdata: &Path,
    master_db_path: &Path,
    profile_id: &str,
    processes: &[trae_instance_module::TraeProcessInfo],
) -> Result<(), String> {
    // 守卫 1：账号必须存在（同时取 account_id 定位原生目录）。
    let record = AccountRegistry::new(material_root)
        .find(profile_id)
        .map_err(|_| "checkin_registry_invalid".to_string())?
        .ok_or_else(|| "checkin_profile_not_found".to_string())?;
    let instance_dir =
        instance_data_dir(storage_root, profile_id).map_err(|e| e.code().to_string())?;
    // 守卫 2：实例运行中一律拒绝（与 get_trae_instance_states 同一判定逻辑）。
    let running = processes.iter().any(|process| {
        process
            .command_line
            .as_deref()
            .is_some_and(|line| command_line_uses_data_dir(line, &instance_dir))
    });
    if running {
        return Err("purge_instance_running".to_string());
    }
    // 备份铁律（ADR-0018 / P11-4③）：purge 前备份主库三件套（与切号/收编/
    // 环境登录同家族纪律）；主库 db 不存在（TRAE 从未启动）无需备份。
    // 备份失败一律拒绝执行——无备份不动数据。
    if master_db_path.is_file() {
        // P11-4①：写入当前活动备份目录（backup_dir 覆盖或 db 同目录）。
        let backup_dir = resolve_master_backup_dir(storage_root, master_db_path);
        backup_master_trio_to(master_db_path, Some(&backup_dir))
            .map_err(|_| "purge_backup_failed".to_string())?;
        // P5-9：备份生成后按保留策略清理旧备份（静默容错，与家族一致）。
        prune_backups_if_enabled(master_db_path, storage_root);
    }
    // 删除 1：实例数据目录（缺失视为成功——幂等）。
    remove_dir_all_if_exists(&instance_dir)?;
    // 删除 2：原生账号目录（account_id 非纯数字时无目录可删，跳过）。
    if let Some(native) = native_account_dir(appdata, &record.account_id) {
        remove_dir_all_if_exists(&native)?;
    }
    // 删除 3：会话索引缓存（`{storage_root}/session-index/{profile_id}.json`）。
    SessionIndexCacheStore::new(storage_root)
        .remove(profile_id)
        .map_err(|_| "purge_io_failed".to_string())?;
    Ok(())
}

/// 删除目录（存在时）；缺失视为成功（幂等），其他 IO 失败报错。
fn remove_dir_all_if_exists(dir: &Path) -> Result<(), String> {
    match std::fs::remove_dir_all(dir) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err("purge_io_failed".to_string()),
    }
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

/// 重新读取当前 TRAE 账号证据，并保存非敏感档案。
#[tauri::command]
fn refresh_managed_current_account(
    state: tauri::State<AppState>,
) -> Result<ManagedAccountsWireView, String> {
    refresh_managed_current_account_inner(&state)
}

fn refresh_managed_current_account_inner(
    state: &AppState,
) -> Result<ManagedAccountsWireView, String> {
    let (generation, authorization) = capture_authorized_read_context(state)?;
    let observation = match detect_managed_current_account(state, generation, &authorization) {
        Ok(observation) => observation,
        Err(error) => {
            return invalidate_on_authorization_error(state, generation, &authorization, error)
        }
    };
    let (store, fingerprint_salt) =
        match managed_account_store_for_refresh(state, generation, &authorization, &observation) {
            Ok(prepared) => prepared,
            Err(error) => {
                return invalidate_on_authorization_error(state, generation, &authorization, error)
            }
        };
    let evidence = managed_current_evidence(
        &observation.account,
        observation.data_location_id.clone(),
        observation.observed_at,
        fingerprint_salt.as_deref(),
    );
    let observed_data_root = observation.data_root.clone();
    let observed_data_location_id = observation.data_location_id.clone();
    let service = ManagedAccountSwitchService::new(&store);
    // 所有 I/O 先落到局部候选；失败时共享 runtime 完全不变。
    let mut candidate = ManagedAccountRuntime::default();
    service
        .refresh_current(&mut candidate, evidence)
        .map_err(|error| error.to_string())?;

    // 固定锁序：authorization -> runtime。只有仍属当前授权代次的候选可提交。
    let authorization_guard = state.authorization.lock().unwrap();
    if authorization_guard.generation != generation || authorization_guard.state != authorization {
        return Err("authorization_mismatch".to_string());
    }
    let mut handoff_transition = None;
    let mut runtime_slot = state.managed_account_runtime.lock().unwrap();
    // 外部切换前的计划跨越授权代次保留；重新授权后用新证据完成复核。
    candidate.switch_plan = runtime_slot.runtime.switch_plan.clone();
    if let Some(plan) = candidate.switch_plan.clone() {
        if matches!(
            plan.state,
            traesync_domain::AccountSwitchState::WaitingForTraeClosed
                | traesync_domain::AccountSwitchState::Applying
                | traesync_domain::AccountSwitchState::Verifying
        ) {
            let credential_context = plan
                .backup_reference
                .as_deref()
                .map(|operation_id| {
                    let target = candidate
                        .profiles
                        .iter()
                        .find(|profile| profile.profile_id == plan.target_profile_id)
                        .ok_or_else(|| "target_profile_not_found".to_string())?;
                    let binding = profile_credential_binding(target);
                    let intent_id = load_handoff_intent_for_state(state)?.and_then(|intent| {
                        (intent.target_profile_id == plan.target_profile_id
                            && intent.state != HandoffIntentState::Expired
                            && (intent.credential_operation_id.as_deref() == Some(operation_id)
                                || intent.credential_operation_id.is_none()))
                        .then_some(intent.intent_id)
                    });
                    Ok::<_, String>((binding, intent_id))
                })
                .transpose()?;
            let observed = candidate.current_account.clone();
            match service.complete_switch(
                &mut candidate,
                &plan.plan_id,
                &observed,
                std::time::SystemTime::now(),
            ) {
                Ok(()) => {
                    if let (Some(operation_id), Some((binding, intent_id))) = (
                        plan.backup_reference.as_deref(),
                        credential_context.as_ref(),
                    ) {
                        if let Err(error) = mark_credential_journal(
                            state,
                            &observed_data_root,
                            &observed_data_location_id,
                            operation_id,
                            true,
                            binding,
                            intent_id.as_deref(),
                        ) {
                            service
                                .mark_manual_recovery_required(
                                    &mut candidate,
                                    &plan.plan_id,
                                    &error,
                                )
                                .map_err(|mark_error| mark_error.to_string())?;
                            handoff_transition =
                                Some((HandoffIntentState::ManualRecoveryRequired, Some(error)));
                        } else {
                            handoff_transition = Some((HandoffIntentState::TargetVerified, None));
                        }
                    } else {
                        handoff_transition = Some((HandoffIntentState::TargetVerified, None));
                    }
                }
                Err(traesync_application::ManagedAccountSwitchError::TargetEvidenceMismatch) => {
                    if let (Some(operation_id), Some((binding, intent_id))) = (
                        plan.backup_reference.as_deref(),
                        credential_context.as_ref(),
                    ) {
                        let _ = mark_credential_journal(
                            state,
                            &observed_data_root,
                            &observed_data_location_id,
                            operation_id,
                            false,
                            binding,
                            intent_id.as_deref(),
                        );
                    }
                    service
                        .mark_manual_recovery_required(
                            &mut candidate,
                            &plan.plan_id,
                            "account_switch_target_evidence_mismatch",
                        )
                        .map_err(|error| error.to_string())?;
                    handoff_transition = Some((
                        HandoffIntentState::ManualRecoveryRequired,
                        Some("account_switch_target_evidence_mismatch".to_string()),
                    ));
                }
                Err(error) => {
                    let reason = error.to_string();
                    if let (Some(operation_id), Some((binding, intent_id))) = (
                        plan.backup_reference.as_deref(),
                        credential_context.as_ref(),
                    ) {
                        let _ = mark_credential_journal(
                            state,
                            &observed_data_root,
                            &observed_data_location_id,
                            operation_id,
                            false,
                            binding,
                            intent_id.as_deref(),
                        );
                    }
                    service
                        .mark_manual_recovery_required(&mut candidate, &plan.plan_id, &reason)
                        .map_err(|mark_error| mark_error.to_string())?;
                    handoff_transition =
                        Some((HandoffIntentState::ManualRecoveryRequired, Some(reason)));
                }
            }
        }
    }
    if runtime_slot.authorization_generation == Some(generation)
        && runtime_slot.runtime.current_account.observed_at > candidate.current_account.observed_at
    {
        candidate.current_account = runtime_slot.runtime.current_account.clone();
        candidate.switch_plan = runtime_slot.runtime.switch_plan.clone();
    }
    runtime_slot.runtime = candidate;
    runtime_slot.authorization_generation = Some(generation);
    drop(runtime_slot);
    drop(authorization_guard);
    if let Some((next_state, failure_reason)) = handoff_transition {
        update_handoff_intent_state(state, next_state, failure_reason)?;
    }
    let handoff_intent = load_handoff_intent_for_state(state)?;
    let runtime_slot = state.managed_account_runtime.lock().unwrap();
    Ok(managed_accounts_wire_view(
        &runtime_slot.runtime,
        true,
        handoff_intent.as_ref(),
    ))
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

fn current_verified_profile_id(state: &AppState) -> Option<String> {
    // 固定锁序：authorization -> runtime；仅当前授权代次可标记为“当前已验证”。
    let authorization = state.authorization.lock().unwrap();
    let runtime = state.managed_account_runtime.lock().unwrap();
    let context_current = matches!(authorization.state, AuthorizationState::Authorized { .. })
        && runtime.authorization_generation == Some(authorization.generation);
    if context_current
        && runtime.runtime.current_account.verification_state == AccountVerificationState::Verified
    {
        runtime.runtime.current_account.profile_id.clone()
    } else {
        None
    }
}

/// 历史浏览最小存根。
///
/// P6-2 退役会话索引与计划族后，scan/browse/search/assign 命令已移除，但
/// `account_registry_inner` 仍然需要一份“历史发现的账号集合”作为前端卡片
/// 显示的账号基线（账号发现基于主库已登录账号证据的即时读取）。
///
/// 模式约定：
/// * `RuntimeMode::RealReadPreview` —— 必须已建立生产目录库运行材料；
///   否则直接 fail-closed（保持之前真实 catalog 路径的拒斥语义）。
/// * 其他模式 —— 若授权有效且能读取主库，则读主库账号证据；否则返回空。
fn browse_history_inner(state: &AppState) -> Result<BrowseResult, String> {
    use traesync_domain::{EvidenceState, HistoryBrowseSummary};
    // 1) RealReadPreview：必须有生产目录库运行材料（与测试断言一致）。
    if matches!(state.runtime_mode, RuntimeMode::RealReadPreview)
        && state.production_catalog.is_none()
    {
        return Err("生产目录库运行材料不可用".to_string());
    }
    // 2) 从主库位置读取账号证据，若有就合成一个账号节点；否则返回空集合。
    let mut accounts: Vec<BrowseAccountNode> = Vec::new();
    if matches!(state.runtime_mode, RuntimeMode::RealReadPreview) {
        if let Ok(master_dir) = master_data_dir() {
            let account = state
                .workbench_reader
                .read_account_evidence(&master_dir, std::time::SystemTime::now());
            if account.evidence_state == EvidenceState::Verified {
                if let Some(uid) = account.user_id.as_ref() {
                    accounts.push(BrowseAccountNode {
                        user_id: uid.as_str().to_string(),
                        display_label: format!(
                            "TRAE Work CN · {}",
                            user_id_display_fingerprint(uid)
                        ),
                        project_count: 0,
                        session_count: 0,
                    });
                }
            }
        }
    }
    let visible_account_count = accounts.len() as u64;
    Ok(BrowseResult {
        accounts,
        projects: Vec::new(),
        sessions: Vec::new(),
        summary: HistoryBrowseSummary {
            visible_account_count,
            visible_project_count: 0,
            visible_session_count: 0,
            soft_deleted_project_count: 0,
            soft_deleted_session_count: 0,
            soft_deleted_message_count: 0,
        },
    })
}

fn account_registry_inner(state: &AppState) -> Result<AccountRegistryWireView, String> {
    let browse = browse_history_inner(state)?;
    let store = managed_account_store_for_read(state)?;
    let mut profile_runtime = ManagedAccountRuntime::default();
    if let Some(store) = store.as_ref() {
        ManagedAccountSwitchService::new(store)
            .load_profiles(&mut profile_runtime)
            .map_err(|error| error.to_string())?;
    }

    let mut credential_states = std::collections::HashMap::new();
    if !profile_runtime.profiles.is_empty() {
        let vault = credential_vault_for_state(state)?;
        for profile in &profile_runtime.profiles {
            credential_states.insert(
                profile.profile_id.clone(),
                vault.status(&profile_credential_binding(profile)).state,
            );
        }
    }
    let current_profile_id = current_verified_profile_id(state);
    let accounts = browse
        .accounts
        .iter()
        .map(|account| {
            let salted_fingerprint = store
                .as_ref()
                .and_then(|store| store.fingerprint_user_id(&account.user_id).ok());
            let legacy_fingerprint = UserId::from_verified(&account.user_id)
                .ok()
                .map(|user_id| user_id_binding_fingerprint(&user_id));
            build_account_registry_entry(
                account,
                &profile_runtime.profiles,
                current_profile_id.as_deref(),
                &credential_states,
                salted_fingerprint.as_deref(),
                legacy_fingerprint.as_deref(),
            )
        })
        .collect();
    Ok(AccountRegistryWireView { accounts })
}

/// 返回历史发现账号与本机授权档案的只读注册表；不启动 TRAE，不发起签到请求。
#[tauri::command]
fn get_account_registry(state: tauri::State<AppState>) -> Result<AccountRegistryWireView, String> {
    account_registry_inner(&state)
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
            get_product_identity_state,
            read_work_cn_state,
            get_account_registry,
            restore_operation,
            get_storage_root_state,
            migrate_storage_root_command,
            plan_storage_deletion,
            apply_storage_deletion,
            export_recovery_package_command,
            import_recovery_package_command,
            // 账号中心只保存非敏感档案；切换命令进入外部切换等待态，不写认证材料。
            get_managed_account_state,
            get_managed_credential_statuses,
            capture_current_account_credential,
            refresh_managed_current_account,
            prepare_managed_account_switch,
            switch_account,
            switch_and_handoff,
            prepare_handoff_intent,
            // 直连签到 + 自动签到调度域命令（P12-R3 迁 checkin_schedule）。
            checkin_schedule::get_checkin_capability,
            checkin_schedule::run_checkin,
            checkin_schedule::cancel_checkin,
            // M4：OAuth 登录两段式命令 + 已登录账号列表（P12-R3 命令迁 oauth_login）。
            oauth_login::begin_checkin_login,
            oauth_login::complete_checkin_login,
            oauth_login::begin_trae_cn_login,
            oauth_login::complete_trae_cn_login,
            oauth_login::cancel_trae_cn_login,
            list_checkin_accounts,
            list_trae_cn_accounts,
            check_trae_cn_account_health,
            switch_trae_cn_account,
            // ADR-0031 账号级远程设备管理（P10-5 切片 2/3）：列表只读 + 远程退出。
            remote_devices::list_remote_devices,
            remote_devices::clear_remote_device,
            // ADR-0031 决策 2：原生列表失败时的官方设备管理页兜底入口。
            remote_devices::open_device_management_docs,
            checkin_schedule::get_checkin_overview,
            refresh_checkin_credits,
            refresh_checkin_credentials,
            refresh_trae_cn_credentials,
            remove_checkin_account,
            // ADR-0019 v4：手动重置签到设备（生成新随机设备并设为 home）。
            reset_checkin_device,
            // P2-2（P6-2 实例功能退役后收缩）：登录存档健康度批量查询。
            get_trae_instance_states,
            // P5-0：主库环境注册表与实例原语（P12-R3 命令迁 environment）。
            environment::launch_master_library,
            environment::get_environment_state,
            get_master_self_check,
            repair_master_current_account,
            force_reset_master_current_account,
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
            switch_master_account,
            preview_master_switch_plugins,
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
            set_account_display_name,
            // G11 手机号补录：完整手机号写凭据包（DPAPI 加密，与令牌同级）。
            set_account_mobile,
            // U-6 W4：资产库三态筛选——归档开关 / 占用统计 / 彻底删除记录。
            set_account_archived,
            get_account_storage_footprint,
            purge_account_records
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

#[cfg(test)]
mod master_switch_rollback_tests {
    use super::*;

    #[test]
    fn rollback_restores_original_bytes_after_step3_write() {
        // P7-2 主场景：第 3 步已改写主库登录态后失败 → 原始字节还原。
        let root = tempfile::tempdir().unwrap();
        let storage_path = root.path().join("storage.json");
        let original = r#"{"icube-dc":{"machineId":"m"},"marscode":{"auth_blob":"AAABBB"}}"#;
        std::fs::write(&storage_path, original).unwrap();
        // 模拟第 3 步改写（杂交态：登录身份已是新账号）。
        std::fs::write(
            &storage_path,
            r#"{"icube-dc":{"machineId":"m"},"marscode":{"auth_blob":"XXXYYY"}}"#,
        )
        .unwrap();

        assert!(rollback_master_login_bytes(&storage_path, original));
        assert_eq!(std::fs::read_to_string(&storage_path).unwrap(), original);
    }

    #[test]
    fn rollback_fails_when_storage_path_unwritable() {
        // 写回失败（路径不存在）：返回 false（外层据此转备份链错误码）。
        let missing = Path::new("Z:/trae-sync-test-missing-rollback/storage.json");
        assert!(!rollback_master_login_bytes(missing, "{}"));
    }
}

/// G23 插件同步策略（ADR-0026）：切号静默应用（纯新增零确认、含移除
/// 单次确认）+ 移除全账号传播（逐账号 fail-soft、builtin 跳过）。
#[cfg(test)]
mod plugin_sync_policy_tests {
    use super::*;

    /// 构造云端已装条目（预检与传播测试共用）。
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
    fn preview_pure_install_difference_is_silent_apply() {
        // ADR-0026 决策 3 纯新增分支：源多目标少 → install_names 非空、
        // remove_names 为空（前端不弹确认，静默应用）。
        let source = vec![
            cloud_item("r1", Some("uuid-a"), "插件A"),
            cloud_item("r2", Some("uuid-b"), "插件B"),
        ];
        let target = vec![cloud_item("r9", Some("uuid-a"), "插件A")];
        let preview = switch_plugin_preview_from_lists(&source, &target);
        assert!(!preview.aborted);
        assert_eq!(preview.source_count, 2);
        assert_eq!(preview.target_count, 1);
        assert_eq!(preview.install_names, vec!["插件B".to_string()]);
        // 无移除 → 无需确认。
        assert!(preview.remove_names.is_empty());
    }

    #[test]
    fn preview_with_removal_lists_names_for_confirmation() {
        // ADR-0026 决策 3 含移除分支：目标有源无 → remove_names 非空
        // （前端据此弹一次移除确认，列明插件名）。
        let source = vec![cloud_item("r1", Some("uuid-a"), "插件A")];
        let target = vec![
            cloud_item("r9", Some("uuid-a"), "插件A"),
            cloud_item("r8", Some("uuid-d"), "插件D"),
        ];
        let preview = switch_plugin_preview_from_lists(&source, &target);
        assert_eq!(preview.remove_names, vec!["插件D".to_string()]);
        assert!(preview.install_names.is_empty());
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

#[cfg(test)]
mod set_account_mobile_tests {
    use super::*;

    /// 大陆手机号格式：1[3-9] 开头共 11 位数字。
    #[test]
    fn mainland_mobile_format() {
        assert!(is_mainland_mobile("13812345678"));
        assert!(is_mainland_mobile("19999999999"));
        // 长度不对 / 非数字 / 号段外。
        assert!(!is_mainland_mobile("1381234567"));
        assert!(!is_mainland_mobile("138123456789"));
        assert!(!is_mainland_mobile("1381234567a"));
        assert!(!is_mainland_mobile("12812345678")); // 第二位 2 不在 3-9
        assert!(!is_mainland_mobile("23812345678")); // 非 1 开头
    }

    /// 脱敏号首尾段解析：标准形态返回 (前缀, 后缀)，非预期形态返回 None。
    #[test]
    fn masked_mobile_parts_parsing() {
        assert_eq!(
            masked_mobile_parts("138****0000"),
            Some(("138".to_string(), "0000".to_string()))
        );
        assert_eq!(masked_mobile_parts(""), None);
        assert_eq!(masked_mobile_parts("13800001111"), None); // 无星号
        assert_eq!(masked_mobile_parts("****0000"), None); // 前缀空
        assert_eq!(masked_mobile_parts("138****"), None); // 后缀空
    }

    /// 凭据仓库读写往返：set_mobile_full 后 load 读出同值；None 清除。
    /// （命令层校验在纯函数测试覆盖；这里只验证存储层落盘闭环。）
    #[test]
    fn store_mobile_full_roundtrip() {
        let root = tempfile::tempdir().unwrap();
        let store = CheckinCredentialStore::new(root.path());
        let (private_pem, public_pem) = traesync_infrastructure::generate_device_keypair().unwrap();
        let bundle = traesync_infrastructure::CheckinCredentialBundle {
            profile_id: "profile-mobile".to_string(),
            account_id: "700100".to_string(),
            device_id: "1234567890123456".to_string(),
            machine_id: "machine-mobile".to_string(),
            device_public_key: public_pem,
            device_private_key: private_pem,
            access_token: "token".to_string(),
            refresh_token: "refresh".to_string(),
            client_id: "client".to_string(),
            access_token_expires_at_unix_seconds: u64::MAX,
            refresh_token_expires_at_unix_seconds: u64::MAX,
            mobile_full: None,
        };
        store.save(&bundle).unwrap();
        let binding = CheckinProfileBinding::new(
            bundle.profile_id.clone(),
            bundle.account_id.clone(),
            bundle.device_id.clone(),
            bundle.device_public_key.clone(),
        );
        store
            .set_mobile_full(&binding, Some("13812345678"))
            .unwrap();
        assert_eq!(
            store.load(&binding).unwrap().mobile_full.as_deref(),
            Some("13812345678")
        );
        // 清除后回到 None（展示回退脱敏号）。
        store.set_mobile_full(&binding, None).unwrap();
        assert_eq!(store.load(&binding).unwrap().mobile_full, None);
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
// P5-1 主库切号：接力台账追加（append_relay_ledger）——编排层唯一可离线
// 单测的纯逻辑；关实例/凭据互换/交接事务由 infrastructure 层各自测试覆盖，
// 真机五步闭环由 P5-2 UI 接线后的真机验收覆盖。
// ============================================================================

#[cfg(test)]
mod master_switch_tests {
    use super::*;
    // 仅测试消费的构造类型（lib 已不再直接使用，避免非测试构建的 unused 警告）。
    use traesync_infrastructure::HandoverSession;

    fn sample_handover(previous_owner: Option<&str>) -> MasterHandover {
        // 逐会话交接前归属与整体 previous_owner 同源（编排层台账暂不消费
        // 该字段，此处仅保持结构完整；None 场景用空串表达"归属未知"）。
        let previous_user_id = previous_owner.unwrap_or("").to_string();
        MasterHandover {
            previous_owner_user_id: previous_owner.map(str::to_string),
            transferred_projects: 2,
            removed_mirror_rows: 1,
            switched_sessions: vec![
                HandoverSession {
                    session_id: "6c-new-a".to_string(),
                    previous_session_id: "old-a".to_string(),
                    project_id: "p1".to_string(),
                    previous_user_id: previous_user_id.clone(),
                    message_count: 12,
                },
                HandoverSession {
                    session_id: "6c-new-b".to_string(),
                    previous_session_id: "old-b".to_string(),
                    project_id: "p2".to_string(),
                    previous_user_id,
                    message_count: 5,
                },
            ],
        }
    }

    #[test]
    fn relay_ledger_append_without_sessions_needs_no_file() {
        let root = tempfile::tempdir().unwrap();
        let handover = MasterHandover {
            previous_owner_user_id: None,
            transferred_projects: 0,
            removed_mirror_rows: 0,
            switched_sessions: Vec::new(),
        };
        // 无会话交接 = 无需记录（true），且不创建台账文件。
        assert!(append_relay_ledger(
            &root.path(),
            &handover,
            "111",
            "222",
            "checkin-b"
        ));
        assert!(!root
            .path()
            .join("environments")
            .join("relay-ledger.json")
            .exists());
    }

    #[test]
    fn relay_ledger_append_persists_entries_with_actual_owner() {
        let root = tempfile::tempdir().unwrap();
        let handover = sample_handover(Some("111"));
        assert!(append_relay_ledger(
            &root.path(),
            &handover,
            "999",
            "222",
            "checkin-b"
        ));

        let entries = RelayLedger::new(root.path().join("environments"))
            .load()
            .unwrap();
        assert_eq!(entries.len(), 2);
        // from 取实际归属（111），而非凭据互换回退值（999）。
        assert_eq!(entries[0].from_user_id, "111");
        assert_eq!(entries[0].to_user_id, "222");
        assert_eq!(entries[0].to_profile_id, "checkin-b");
        assert_eq!(entries[0].session_id, "6c-new-a");
        assert_eq!(entries[0].from_session_id.as_deref(), Some("old-a"));
        assert_eq!(entries[0].message_count_at_switch, 12);
        assert!(entries[0].switched_at_unix_seconds > 0);
    }

    #[test]
    fn relay_ledger_append_falls_back_to_observed_login_when_owner_unknown() {
        let root = tempfile::tempdir().unwrap();
        // 无归属记录（previous_owner=None）但有会话：回退凭据互换观察到的旧登录。
        let handover = sample_handover(None);
        assert!(append_relay_ledger(
            &root.path(),
            &handover,
            "999",
            "222",
            "checkin-b"
        ));
        let entries = RelayLedger::new(root.path().join("environments"))
            .load()
            .unwrap();
        assert_eq!(entries[0].from_user_id, "999");
    }

    #[test]
    fn relay_ledger_append_failure_returns_false_without_panic() {
        let root = tempfile::tempdir().unwrap();
        // 损坏台账文件：追加报错（证据不静默重建），编排层只降级为 false。
        let environments = root.path().join("environments");
        std::fs::create_dir_all(&environments).unwrap();
        std::fs::write(environments.join("relay-ledger.json"), "not json").unwrap();
        let handover = sample_handover(Some("111"));
        assert!(!append_relay_ledger(
            &root.path(),
            &handover,
            "999",
            "222",
            "checkin-b"
        ));
    }

}

// ============================================================================
// P8-5 G18：主库自检四级判定。
// ============================================================================
#[cfg(test)]
mod master_self_check_tests {
    use super::*;

    fn input() -> MasterSelfCheckInput {
        MasterSelfCheckInput {
            storage_exists: true,
            storage_readable: true,
            blob_present: true,
            blob_decryptable: true,
            account_registered: true,
            cache_consistent: true,
            instance_stopped: true,
            donor_available: true,
            ledger_valid: true,
        }
    }

    #[test]
    fn healthy_when_four_checks_pass() {
        let assessment = assess_master_self_check(input());

        assert_eq!(assessment.level, MasterSelfCheckLevel::Healthy);
        assert_eq!(assessment.read_status, MasterSelfCheckStatus::Passed);
        assert_eq!(assessment.consistency_status, MasterSelfCheckStatus::Passed);
        assert_eq!(
            assessment.switchability_status,
            MasterSelfCheckStatus::Passed
        );
        assert_eq!(assessment.deep_status, MasterSelfCheckStatus::Passed);
    }

    #[test]
    fn stale_registry_is_self_healable() {
        let assessment = assess_master_self_check(MasterSelfCheckInput {
            cache_consistent: false,
            ..input()
        });

        assert_eq!(assessment.level, MasterSelfCheckLevel::SelfHealable);
        assert_eq!(
            assessment.consistency_status,
            MasterSelfCheckStatus::Attention
        );
    }

    #[test]
    fn corrupt_blob_requires_manual_login() {
        let assessment = assess_master_self_check(MasterSelfCheckInput {
            blob_decryptable: false,
            ..input()
        });

        assert_eq!(assessment.level, MasterSelfCheckLevel::NeedsManual);
        assert_eq!(assessment.read_status, MasterSelfCheckStatus::Failed);
    }

    #[test]
    fn unreadable_storage_requires_manual_attention() {
        let assessment = assess_master_self_check(MasterSelfCheckInput {
            storage_readable: false,
            ..input()
        });

        assert_eq!(assessment.level, MasterSelfCheckLevel::NeedsManual);
        assert_eq!(assessment.read_status, MasterSelfCheckStatus::Failed);
    }

    #[test]
    fn running_instance_blocks_switchability() {
        let assessment = assess_master_self_check(MasterSelfCheckInput {
            instance_stopped: false,
            ..input()
        });

        assert_eq!(assessment.level, MasterSelfCheckLevel::SelfHealable);
        assert_eq!(
            assessment.switchability_status,
            MasterSelfCheckStatus::Blocked
        );
    }

    #[test]
    fn invalid_ledger_requires_manual_attention() {
        let assessment = assess_master_self_check(MasterSelfCheckInput {
            ledger_valid: false,
            ..input()
        });

        assert_eq!(assessment.level, MasterSelfCheckLevel::NeedsManual);
        assert_eq!(assessment.deep_status, MasterSelfCheckStatus::Failed);
    }
}

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

#[cfg(test)]
mod real_checkin_tests {
    use super::*;
    // P12-R3：AppState 构造夹具与注册表档案夹具为跨域共享，提至 test_support。
    use crate::test_support::{make_real_mode_state, registered_account, registered_account_with_id};

    /// U-1 真实环境验收：refresh 路径对脱敏手机号缺失的存量账号无感补采。
    /// 只读（status + usage + GetUserInfo，不 claim）；生产存储 6 账号串行。
    #[test]
    #[ignore = "U-1 真实生产存储验收：refresh + GetUserInfo 补采（只读，不 claim）"]
    fn real_refresh_backfills_masked_mobile() {
        let material_root = std::env::var_os("LOCALAPPDATA")
            .map(|base| {
                std::path::PathBuf::from(base)
                    .join("Trae Sync")
                    .join("data")
                    .join("checkin")
            })
            .expect("LOCALAPPDATA 未设置");
        let registry = AccountRegistry::new(&material_root);
        let records = registry.load().expect("生产注册表读取失败");
        assert!(!records.is_empty(), "生产注册表为空");
        let ids: Vec<String> = records.iter().map(|r| r.profile_id.clone()).collect();
        let entries = refresh_checkin_credits_inner(
            &material_root,
            &ids,
            RuntimeMode::RealReadPreview,
            // 真实验收脚本：不需要进度推送。
            &|_| {},
        )
        .expect("真实 refresh 失败");
        for entry in &entries {
            println!(
                "== refresh {} -> credits={:?} usage={:?} err={:?}",
                entry.profile_id, entry.credits, entry.usage_remaining_credits, entry.error_code
            );
        }
        let after = registry.load().expect("补采后注册表读取失败");
        for record in &after {
            println!(
                "== {}（{}）：masked_mobile={}",
                record.screen_name,
                record.profile_id,
                if record.masked_mobile.is_empty() {
                    "<空>"
                } else {
                    &record.masked_mobile
                }
            );
        }
        let missing: Vec<&str> = after
            .iter()
            .filter(|r| r.masked_mobile.is_empty())
            .map(|r| r.screen_name.as_str())
            .collect();
        assert!(
            missing.is_empty(),
            "补采后仍有账号缺手机号（服务端未返回？）：{missing:?}"
        );
    }

    // P12-R3：login_commands_reject_fixture_mode / begin_login_rejects_when_storage_root_unavailable
    // / begin_login_produces_url_and_consumable_session 三例随登录命令链迁至
    // oauth_login::login_command_tests。

    #[test]
    fn list_checkin_accounts_returns_non_sensitive_whitelist() {
        let root = tempfile::tempdir().unwrap();
        registered_account(root.path(), "profile-a");
        let state = make_real_mode_state(root.path());
        let accounts = list_checkin_accounts_inner(&state).unwrap();
        assert_eq!(accounts.len(), 1);
        assert_eq!(accounts[0].profile_id, "profile-a");
        assert_eq!(accounts[0].account_id, "account-profile-a");
        assert_eq!(accounts[0].screen_name, "测试账号");
        // wire 白名单：不含设备 ID/设备公钥等绑定材料。
        let value = serde_json::to_value(&accounts[0]).unwrap();
        let keys: std::collections::BTreeSet<String> =
            value.as_object().unwrap().keys().cloned().collect();
        assert_eq!(
            keys,
            [
                "account_id",
                "archived",
                "avatar_url",
                "last_verified_at",
                "profile_id",
                "screen_name"
            ]
            .into_iter()
            .map(str::to_string)
            .collect()
        );
        let serialized = serde_json::to_string(&value).unwrap();
        for forbidden in [
            "device_id",
            "device_public_key",
            "refresh_token",
            "access_token",
        ] {
            assert!(!serialized.contains(forbidden), "泄漏字段: {forbidden}");
        }
    }

    // ====================================================================
    // U-6 W4：资产库三态筛选后端原语（归档开关 / 占用统计 / 彻底删除记录）
    // ====================================================================

    /// U-6 W4：占用统计——实例目录 + 原生目录递归求和；目录缺失记 0。
    #[test]
    fn storage_footprint_sums_dirs_and_missing_is_zero() {
        let root = tempfile::tempdir().unwrap();
        // profile-a：纯数字 account_id 可定位原生目录；profile-b：非数字
        // account_id 且从未启动实例（两种「记 0」口径都覆盖）。
        registered_account_with_id(root.path(), "profile-a", "1234567890");
        registered_account(root.path(), "profile-b");

        let instance_dir = root.path().join("trae-instances").join("profile-a");
        std::fs::create_dir_all(instance_dir.join("User")).unwrap();
        std::fs::write(instance_dir.join("machineid"), vec![0u8; 10]).unwrap();
        std::fs::write(
            instance_dir.join("User").join("storage.json"),
            vec![0u8; 26],
        )
        .unwrap();
        let appdata = tempfile::tempdir().unwrap();
        let native = appdata.path().join("TRAE SOLO CN_1234567890");
        std::fs::create_dir_all(&native).unwrap();
        std::fs::write(native.join("machineid"), vec![0u8; 7]).unwrap();

        let entries = get_account_storage_footprint_inner(
            &root.path().join("checkin"),
            root.path(),
            appdata.path(),
            &["profile-a".to_string(), "profile-b".to_string()],
        )
        .unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].profile_id, "profile-a");
        assert_eq!(entries[0].instance_dir_bytes, 36);
        assert_eq!(entries[0].native_dir_bytes, 7);
        // 目录缺失（实例未启动）与 account_id 非纯数字（原生目录定位不到）均记 0。
        assert_eq!(entries[1].profile_id, "profile-b");
        assert_eq!(entries[1].instance_dir_bytes, 0);
        assert_eq!(entries[1].native_dir_bytes, 0);
    }

    /// U-6 W4：purge 守卫——实例运行中一律拒绝，目录原样保留。
    /// 进程观测逻辑与 get_trae_instance_states 一致（--user-data-dir 匹配）。
    #[test]
    fn purge_rejects_when_instance_running() {
        let root = tempfile::tempdir().unwrap();
        registered_account_with_id(root.path(), "profile-a", "1234567890");
        let instance_dir = root.path().join("trae-instances").join("profile-a");
        std::fs::create_dir_all(&instance_dir).unwrap();
        std::fs::write(instance_dir.join("storage.json"), b"{}").unwrap();
        // 模拟运行中的实例进程：命令行指向该实例目录（引号形态与真实
        // PowerShell 输出一致，command_line_uses_data_dir 归一化匹配）。
        let processes = vec![trae_instance_module::TraeProcessInfo {
            pid: 4242,
            exe_path: Some(r"C:\TRAE.exe".to_string()),
            command_line: Some(format!(
                r#""C:\TRAE.exe" --user-data-dir={}"#,
                instance_dir.display()
            )),
        }];
        let appdata = tempfile::tempdir().unwrap();
        let error = purge_account_records_inner(
            &root.path().join("checkin"),
            root.path(),
            appdata.path(),
            &root.path().join("master").join("database.db"),
            "profile-a",
            &processes,
        )
        .unwrap_err();
        assert_eq!(error, "purge_instance_running");
        // 守卫生效：实例目录原样保留。
        assert!(instance_dir.join("storage.json").is_file());
    }

    /// U-6 W4：purge 守卫——账号不存在拒绝（无档案即无 account_id 可定位原生目录）。
    #[test]
    fn purge_rejects_unknown_account() {
        let root = tempfile::tempdir().unwrap();
        let appdata = tempfile::tempdir().unwrap();
        let error = purge_account_records_inner(
            &root.path().join("checkin"),
            root.path(),
            appdata.path(),
            &root.path().join("master").join("database.db"),
            "profile-404",
            &[],
        )
        .unwrap_err();
        assert_eq!(error, "checkin_profile_not_found");
    }

    /// U-6 W4：purge 删除实例目录 + 原生账号目录 + 会话索引缓存三处；
    /// 档案保留（purge 不删身份），且幂等可重复执行。
    #[test]
    fn purge_removes_dirs_and_cache_and_is_idempotent() {
        let root = tempfile::tempdir().unwrap();
        registered_account_with_id(root.path(), "profile-a", "1234567890");
        // 三处现场：实例目录、原生目录、会话索引缓存。
        let instance_dir = root.path().join("trae-instances").join("profile-a");
        std::fs::create_dir_all(instance_dir.join("User").join("globalStorage")).unwrap();
        std::fs::write(instance_dir.join("machineid"), b"machine").unwrap();
        let appdata = tempfile::tempdir().unwrap();
        let native = appdata.path().join("TRAE SOLO CN_1234567890");
        std::fs::create_dir_all(native.join("User").join("globalStorage")).unwrap();
        std::fs::write(native.join("machineid"), b"machine").unwrap();
        let cache_store = SessionIndexCacheStore::new(root.path());
        cache_store
            .save(
                "profile-a",
                &traesync_infrastructure::account_session_index::CachedSessionIndex {
                    // 与 CACHE_FORMAT_VERSION（当前为 1）一致的有效缓存。
                    format_version: 1,
                    sessions: vec![],
                    last_max_updated_at: None,
                },
            )
            .unwrap();
        let cache_file = root.path().join("session-index").join("profile-a.json");
        assert!(cache_file.is_file());

        purge_account_records_inner(
            &root.path().join("checkin"),
            root.path(),
            appdata.path(),
            &root.path().join("master").join("database.db"),
            "profile-a",
            &[],
        )
        .unwrap();

        assert!(!instance_dir.exists());
        assert!(!native.exists());
        assert!(!cache_file.exists());
        // 档案保留：purge 只删数据记录，不删身份（那是 remove_checkin_account 的职责）。
        assert!(AccountRegistry::new(root.path().join("checkin"))
            .find("profile-a")
            .unwrap()
            .is_some());
        // 幂等：目录与缓存已缺失时重复执行仍成功。
        purge_account_records_inner(
            &root.path().join("checkin"),
            root.path(),
            appdata.path(),
            &root.path().join("master").join("database.db"),
            "profile-a",
            &[],
        )
        .unwrap();
    }

    /// P11-4③：purge 备份先行——主库 db 存在时先留三件套备份（create_new
    /// 永不覆盖既有链），备份成功后再执行删除。
    #[test]
    fn purge_backs_up_master_trio_before_deletion() {
        let root = tempfile::tempdir().unwrap();
        registered_account_with_id(root.path(), "profile-a", "1234567890");
        let instance_dir = root.path().join("trae-instances").join("profile-a");
        std::fs::create_dir_all(&instance_dir).unwrap();
        std::fs::write(instance_dir.join("storage.json"), b"{}").unwrap();
        // 主库 db + wal 现场（备份只做文件复制，字节内容任意）。
        let master_dir = root.path().join("master");
        std::fs::create_dir_all(&master_dir).unwrap();
        let master_db = master_dir.join("database.db");
        std::fs::write(&master_db, b"master-db-bytes").unwrap();
        std::fs::write(master_dir.join("database.db-wal"), b"wal-bytes").unwrap();
        let appdata = tempfile::tempdir().unwrap();

        purge_account_records_inner(
            &root.path().join("checkin"),
            root.path(),
            appdata.path(),
            &master_db,
            "profile-a",
            &[],
        )
        .unwrap();

        // 备份生成：主文件内容与 db 一致，且带 wal 附属件。
        let backup_main = std::fs::read_dir(&master_dir)
            .unwrap()
            .filter_map(|entry| entry.ok())
            .find(|entry| {
                let name = entry.file_name().to_string_lossy().into_owned();
                name.starts_with("database.db.switch-bak-")
                    && !name.ends_with("-wal")
                    && !name.ends_with("-shm")
            })
            .expect("备份主文件应存在");
        assert_eq!(
            std::fs::read(backup_main.path()).unwrap(),
            b"master-db-bytes"
        );
        let backup_name = backup_main.file_name().to_string_lossy().into_owned();
        assert!(master_dir.join(format!("{backup_name}-wal")).is_file());
        // 备份先行不阻断删除主流程：三处现场照常清理。
        assert!(!instance_dir.exists());
    }

    /// P11-4③：备份失败一律拒绝执行（无备份不动数据），删除零发生。
    /// 预占未来数秒的同名备份位使 create_new 撞车，确定性触发备份失败。
    #[test]
    fn purge_rejects_when_master_backup_fails() {
        let root = tempfile::tempdir().unwrap();
        registered_account_with_id(root.path(), "profile-a", "1234567890");
        let instance_dir = root.path().join("trae-instances").join("profile-a");
        std::fs::create_dir_all(&instance_dir).unwrap();
        std::fs::write(instance_dir.join("storage.json"), b"{}").unwrap();
        let master_dir = root.path().join("master");
        std::fs::create_dir_all(&master_dir).unwrap();
        let master_db = master_dir.join("database.db");
        std::fs::write(&master_db, b"master-db-bytes").unwrap();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        for stamp in now - 2..=now + 5 {
            std::fs::write(
                master_dir.join(format!("database.db.switch-bak-{stamp}")),
                b"occupied",
            )
            .unwrap();
        }
        let appdata = tempfile::tempdir().unwrap();

        let error = purge_account_records_inner(
            &root.path().join("checkin"),
            root.path(),
            appdata.path(),
            &master_db,
            "profile-a",
            &[],
        )
        .unwrap_err();

        assert_eq!(error, "purge_backup_failed");
        // 守卫生效：实例目录原样保留（未发生任何删除）。
        assert!(instance_dir.join("storage.json").is_file());
    }
}
