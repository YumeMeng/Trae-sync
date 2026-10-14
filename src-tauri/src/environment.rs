//! 环境注册与实例生命周期域（P12-R3 自 lib.rs 纯移动）。
//!
//! 覆盖：主库实例启动/聚焦与环境状态查询（P5-0）、当前账号登记（P5-0）、
//! 多环境档案与生命周期命令（P6-4：列表/创建/重命名/删除预览/删除/启动/
//! 关闭）、副环境凭据互换登录事务（P6-4 环境登录），以及主库当前账号
//! 实测纠正 reconcile_master_current_profile（get_environment_state /
//! list_environments 两读取路径共用）与全部内嵌测试。
//! 主库切号五步事务、主库自检（G18）与库内容读写留守组合根与库域模块，
//! 分别经 `crate::` 与 `crate::library_archive::` 引用。命令体、错误码
//! 与 wire 形态零变化；环境数据目录约定保持
//! `{storage_root}\environments\{env_id}`（secondary_data_dir）。

use std::path::{Path, PathBuf};

use serde::Serialize;

use traesync_infrastructure::environment_registry::{
    self as environment_registry_module, environment_registry_error_code, master_data_dir,
    secondary_data_dir, EnvironmentRecord, EnvironmentRegistry, MASTER_ENV_ID,
};
use traesync_infrastructure::library_account::observed_profile_id;
use traesync_infrastructure::trae_instance::{
    self as trae_instance_module, close_instance, current_product_process_state,
    default_product_data_dir, dir_size_recursive, instance_data_dir, list_trae_processes,
    product_process_state, seed_login_state_from_donor, CloseOutcome, ProductProcessState,
};
use traesync_infrastructure::TraeProduct;
use traesync_infrastructure::{
    archive_login_user_id, backup_master_trio_to, handover_master_records, master_database_path,
    master_db_activity_detected, AccountRegistry, AuthSwitchError, MasterHandoverError,
    OperationLease,
};

use crate::{checkin_material_root, ensure_real_preview, run_blocking, AppState, RuntimeMode};
// P12-R3：切号凭据互换双路径与回滚还原随凭据域迁 credential_handover。
use crate::credential_handover::{
    finalize_handover_failure, rollback_master_login_bytes, switch_login_identity_dual_path,
};
// P13-5：实例工具族随迁 instance_tools。
use crate::instance_tools::{
    launch_instance_common, trae_instance_account_id, TraeInstanceLaunchDto,
};

fn acquire_environment_lease(
    recovery_root: &Path,
    resource_path: &Path,
) -> Result<OperationLease, String> {
    OperationLease::acquire_shared_for_path(recovery_root, resource_path)
        .map_err(|error| format!("operation_lease_unavailable:{error}"))
}

/// 副环境先锁定容器生命周期，再读取档案并锁定单个数据目录；取得单环境锁后释放容器锁。
fn acquire_secondary_environment_lease(
    recovery_root: &Path,
    storage_root: &Path,
    env_id: &str,
) -> Result<(EnvironmentRecord, OperationLease), String> {
    let lifecycle_lease =
        acquire_environment_lease(recovery_root, &storage_root.join("environments"))?;
    let registry = EnvironmentRegistry::new(storage_root);
    let record = registry
        .find(env_id)
        .map_err(environment_registry_error_code)?
        .ok_or_else(|| "environment_not_found".to_string())?;
    let env_dir = secondary_data_dir(storage_root, &record.env_id);
    let operation_lease = acquire_environment_lease(recovery_root, &env_dir)?;
    drop(lifecycle_lease);
    Ok((record, operation_lease))
}

/// 主库环境状态 DTO（P5-0；环境页主库实例卡数据源，前端 P5-2 接线）。
#[derive(Clone, Serialize)]
pub(crate) struct EnvironmentStateDto {
    env_id: String,
    /// 环境注册表当前记录的账号；无法读取实测身份时保留最后已知值。
    current_profile_id: Option<String>,
    /// 与 current_profile_id 对应的显示名；这是环境记录，不单独证明实时登录。
    current_account_name: Option<String>,
    /// 从 Work CN 登录材料中实测并映射到账号档案的身份；None 表示登出、不可读或未登记。
    observed_profile_id: Option<String>,
    /// 主库 data_dir 绝对路径（展示与排障用）。
    data_dir: String,
    running: bool,
    /// 三态进程结论；未知时界面不得显示为未运行。
    process_state: ProductProcessState,
    login_state: trae_instance_module::InstanceLoginState,
    created_at_unix_seconds: u64,
}

/// 启动/聚焦主库 TRAE 实例（P5-0，环境模型 Q1：主库单实例承载全部对话记录）。
///
/// 与账号实例的差异：不播种登录态（Q2 新建空主库：首启在 TRAE 内登录一次，
/// 此后登录态由切号流程 P5-1 的凭据互换维护）；窗口标题取环境档案中的
/// 当前登录账号名，未登录时显示「主库」。
#[tauri::command]
pub(crate) async fn launch_master_library(
    state: tauri::State<'_, AppState>,
) -> Result<TraeInstanceLaunchDto, String> {
    ensure_real_preview(&state)?;
    let material_root = checkin_material_root(&state)?;
    let storage_root = PathBuf::from(&state.storage_root);
    let recovery_root = PathBuf::from(&state.recovery_root);
    run_blocking("trae_launch_join_failed", move || {
        let master_dir =
            master_data_dir().map_err(|_| "environment_registry_invalid".to_string())?;
        let _lease = acquire_environment_lease(&recovery_root, &master_dir)?;
        launch_master_library_inner(&material_root, &storage_root)
    })
    .await
}

fn launch_master_library_inner(
    material_root: &Path,
    storage_root: &Path,
) -> Result<TraeInstanceLaunchDto, String> {
    let registry = EnvironmentRegistry::new(storage_root);
    let record = registry
        .ensure_master()
        .map_err(|_| "environment_registry_invalid".to_string())?;
    // 窗口标题：当前账号名（实测优先，官方登录/登出后注册表缓存过期）；
    // 未登录账号时显示「主库」。
    let accounts = AccountRegistry::new(material_root)
        .load()
        .map_err(|_| "checkin_registry_invalid".to_string())?;
    let instance_dir = master_data_dir().map_err(|_| "environment_registry_invalid".to_string())?;
    let current_profile_id = reconcile_master_current_profile(
        &registry,
        &accounts,
        &instance_dir,
        record.current_profile_id.as_deref(),
    )
    .current_profile_id;
    let window_title = current_profile_id
        .as_deref()
        .and_then(|profile_id| {
            accounts
                .iter()
                .find(|record| record.profile_id == profile_id)
                .map(|record| {
                    record
                        .display_name
                        .clone()
                        .unwrap_or_else(|| record.screen_name.clone())
                })
        })
        .unwrap_or_else(|| "主库".to_string());
    // 主库 = 官方目录（2026-08-31 修订）：用户官方启动的实例也算「已运行」，
    // 聚焦它而不是再起一个。P15-6：主库目标 = 融合版客户端（Trae CN.exe），
    // exe 定位/进程匹配随目标目录同切产品（WorkCn 规则下 crashpad 子进程
    // 带融合版 --user-data-dir 会毒化为 Unknown，实测 2026-10-14）。
    launch_instance_common(TraeProduct::TraeCn, &instance_dir, &window_title, None)
}

/// 查询主库环境状态（档案 + 运行态 + 登录态）。
/// fixture 模式返回未运行默认态（与 get_trae_instance_states 口径一致）。
#[tauri::command]
pub(crate) async fn get_environment_state(
    state: tauri::State<'_, AppState>,
) -> Result<EnvironmentStateDto, String> {
    if state.runtime_mode != RuntimeMode::RealReadPreview {
        return Ok(EnvironmentStateDto {
            env_id: environment_registry_module::MASTER_ENV_ID.to_string(),
            current_profile_id: None,
            current_account_name: None,
            observed_profile_id: None,
            data_dir: String::new(),
            running: false,
            process_state: ProductProcessState::Unknown,
            login_state: trae_instance_module::InstanceLoginState::Uninitialized,
            created_at_unix_seconds: 0,
        });
    }
    let material_root = checkin_material_root(&state)?;
    let storage_root = PathBuf::from(&state.storage_root);
    run_blocking("environment_state_join_failed", move || {
        get_environment_state_inner(&material_root, &storage_root)
    })
    .await
}

/// 主库当前账号解析（实测优先 + 注册表纠正，get_environment_state /
/// list_environments 两读取路径共用）：
/// - 实测登录已登记账号 → 返回实测 profile，并把过期的注册表缓存纠正
///   为实测值（fail-soft 幂等收敛；插件同步预检等注册表消费方一并修正）；
/// - 实测不可用（登出/blob 不可读/账号未登记）→ 保留注册表缓存
///   （最后已知账号，不写垃圾值）。
/// P12-R1b：实测源（`observed_profile_id`）随解析模块下沉共用。
struct MasterCurrentProfileResolution {
    /// 实测身份可用时取实测值，否则保留注册表中的最后已知账号。
    current_profile_id: Option<String>,
    /// 仅包含本次从登录材料中实测确认的账号。
    observed_profile_id: Option<String>,
}

fn reconcile_master_current_profile(
    registry: &EnvironmentRegistry,
    accounts: &[traesync_infrastructure::account_registry::AccountRecord],
    master_instance_dir: &Path,
    cached_profile_id: Option<&str>,
) -> MasterCurrentProfileResolution {
    let observed_profile_id = observed_profile_id(accounts, master_instance_dir);
    let current_profile_id = match observed_profile_id.as_deref() {
        Some(observed) => {
            if cached_profile_id != Some(observed) {
                let _ = registry.set_current_profile(MASTER_ENV_ID, observed);
            }
            Some(observed.to_string())
        }
        None => cached_profile_id.map(str::to_string),
    };
    MasterCurrentProfileResolution {
        current_profile_id,
        observed_profile_id,
    }
}

fn get_environment_state_inner(
    material_root: &Path,
    storage_root: &Path,
) -> Result<EnvironmentStateDto, String> {
    let registry = EnvironmentRegistry::new(storage_root);
    let record = registry
        .ensure_master()
        .map_err(|_| "environment_registry_invalid".to_string())?;
    let instance_dir = master_data_dir().map_err(|_| "environment_registry_invalid".to_string())?;
    // 运行态：主库 = 官方目录（融合版，P15-6），产品身份与目录同源——
    // WorkCn 规则会把 Trae CN 客户端全部判为不可归属（状态未知）。
    let process_state = current_product_process_state(TraeProduct::TraeCn, &instance_dir);
    // 当前账号实测优先（官方登录/登出后注册表缓存过期）：
    // 账号注册表一次加载，供 userId 反查 profile 与显示名解析。
    let accounts = AccountRegistry::new(material_root)
        .load()
        .map_err(|_| "checkin_registry_invalid".to_string())?;
    let current_profile_resolution = reconcile_master_current_profile(
        &registry,
        &accounts,
        &instance_dir,
        record.current_profile_id.as_deref(),
    );
    let observed_profile_id = current_profile_resolution.observed_profile_id;
    let current_profile_id = current_profile_resolution.current_profile_id;
    let current_account_name = current_profile_id.as_deref().and_then(|profile_id| {
        accounts
            .iter()
            .find(|record| record.profile_id == profile_id)
            .map(|record| {
                record
                    .display_name
                    .clone()
                    .unwrap_or_else(|| record.screen_name.clone())
            })
    });
    Ok(EnvironmentStateDto {
        env_id: record.env_id,
        current_profile_id,
        current_account_name,
        observed_profile_id,
        data_dir: instance_dir.display().to_string(),
        running: process_state == ProductProcessState::Running,
        process_state,
        login_state: trae_instance_module::instance_login_state(&instance_dir),
        created_at_unix_seconds: record.created_at_unix_seconds,
    })
}

/// 登记主库当前登录账号（P5-0 持久化原语：首次在主库内登录后登记；
/// 切号流程 P5-1 交接完成后经同一注册表写回）。profile 必须已在账号注册表。
#[tauri::command]
pub(crate) async fn set_environment_current_account(
    profile_id: String,
    state: tauri::State<'_, AppState>,
) -> Result<bool, String> {
    ensure_real_preview(&state)?;
    let material_root = checkin_material_root(&state)?;
    let storage_root = PathBuf::from(&state.storage_root);
    run_blocking("environment_account_join_failed", move || {
        // 校验账号存在（复用账号实例的档案查询：不存在报 trae_profile_invalid）。
        trae_instance_account_id(&material_root, &profile_id)?;
        EnvironmentRegistry::new(&storage_root)
            // P5-0 语义：登记主库环境的当前登录账号（V2 注册表带 env_id）。
            .set_current_profile(MASTER_ENV_ID, &profile_id)
            .map_err(|_| "environment_registry_invalid".to_string())
    })
    .await
}

// ===== P6-4 环境管理 V2（多环境档案 + 生命周期 + 环境登录，ADR-0024）=====

/// 环境列表项 DTO（`list_environments` 返回；环境页 V2 数据源）。
#[derive(Clone, Serialize)]
pub(crate) struct EnvironmentListItemDto {
    env_id: String,
    name: String,
    is_master: bool,
    current_profile_id: Option<String>,
    /// 当前登录账号显示名（备注名优先）；None = 尚未登录。
    current_account_name: Option<String>,
    data_dir: String,
    running: bool,
    /// 三态进程结论；未知时界面不得显示为未运行。
    process_state: ProductProcessState,
    login_state: trae_instance_module::InstanceLoginState,
    created_at_unix_seconds: u64,
    /// 环境体积（仅 include_size=true 时返回；目录遍历较重，轮询不带）。
    #[serde(skip_serializing_if = "Option::is_none")]
    size_bytes: Option<u64>,
}

/// 环境档案 DTO（创建/重命名回执）。
#[derive(Clone, Serialize)]
pub(crate) struct EnvironmentRecordDto {
    env_id: String,
    name: String,
    created_at_unix_seconds: u64,
}

/// 环境删除预览 DTO（确认弹层数据源：列明将被删除的规模）。
#[derive(Clone, Serialize)]
pub(crate) struct EnvironmentDeletePreviewDto {
    /// ready=规模读取成功；no_data=环境从未启动（无记录）；read_failed=规模未知。
    status: &'static str,
    project_count: u64,
    session_count: u64,
    /// 环境目录整体体积（删除前用户知情口径）。
    size_bytes: u64,
}

/// 环境登录回执 DTO。
#[derive(Clone, Serialize)]
pub(crate) struct EnvironmentLoginReceiptDto {
    env_id: String,
    profile_id: String,
    /// true = 空环境首登（账号存档播种，移植即登录）；false = 互换登录。
    seeded: bool,
    /// 随行归属的项目行数（单一归属泛化，ADR-0021）。
    transferred_projects: usize,
    /// 交接的会话数。
    switched_sessions: usize,
    /// 自动清理的目标账号空镜像行数（E5 坑位）。
    removed_mirror_rows: usize,
    /// 环境档案是否已记住本次切换的账号。
    current_profile_saved: bool,
}

// P12-R1b：`environment_registry_error_code` 下沉
// traesync_infrastructure::environment_registry（错误类型同模块），经导入
// 保持全部调用点口径不变。

/// 环境列表（主库置顶，副环境按创建时间升序）。include_size=true 时附带
/// 体积（主库三件套口径 / 副环境整目录递归；较重，前端仅手动刷新时请求）。
#[tauri::command]
pub(crate) async fn list_environments(
    include_size: Option<bool>,
    state: tauri::State<'_, AppState>,
) -> Result<Vec<EnvironmentListItemDto>, String> {
    if state.runtime_mode != RuntimeMode::RealReadPreview {
        // fixture：仅默认主库空态（与 get_environment_state 口径一致）。
        return Ok(vec![EnvironmentListItemDto {
            env_id: environment_registry_module::MASTER_ENV_ID.to_string(),
            name: "主库".to_string(),
            is_master: true,
            current_profile_id: None,
            current_account_name: None,
            data_dir: String::new(),
            running: false,
            process_state: ProductProcessState::Unknown,
            login_state: trae_instance_module::InstanceLoginState::Uninitialized,
            created_at_unix_seconds: 0,
            size_bytes: None,
        }]);
    }
    let material_root = checkin_material_root(&state)?;
    let storage_root = PathBuf::from(&state.storage_root);
    let include_size = include_size.unwrap_or(false);
    run_blocking("environment_list_join_failed", move || {
        list_environments_inner(&material_root, &storage_root, include_size)
    })
    .await
}

fn list_environments_inner(
    material_root: &Path,
    storage_root: &Path,
    include_size: bool,
) -> Result<Vec<EnvironmentListItemDto>, String> {
    use traesync_infrastructure::master_stats::master_trio_size_bytes;

    let registry = EnvironmentRegistry::new(storage_root);
    // 主库档案缺失时自动建档（幂等），保证列表始终含主库置顶项。
    registry
        .ensure_master()
        .map_err(|_| "environment_registry_invalid".to_string())?;
    let mut records = registry
        .load_all()
        .map_err(|_| "environment_registry_invalid".to_string())?;
    // 排序：主库置顶，副环境按创建时间升序（列表稳定，创建即追加）。
    records.sort_by(|a, b| {
        b.is_master()
            .cmp(&a.is_master())
            .then(a.created_at_unix_seconds.cmp(&b.created_at_unix_seconds))
    });

    // 账号名映射：一次加载账号注册表（profile_id → 显示名）。
    let accounts = AccountRegistry::new(material_root)
        .load()
        .map_err(|_| "checkin_registry_invalid".to_string())?;
    let account_name = |profile_id: &str| -> Option<String> {
        accounts
            .iter()
            .find(|record| record.profile_id == profile_id)
            .map(|record| {
                record
                    .display_name
                    .clone()
                    .unwrap_or_else(|| record.screen_name.clone())
            })
    };

    // 列表共享一次进程快照；查询失败时各环境状态保留 unknown。
    let processes = list_trae_processes().ok();
    // P15-6：环境实例（主库 + 副环境）都由融合版客户端承载，产品身份同切。
    let default_data_dir = default_product_data_dir(TraeProduct::TraeCn);
    let master_dir = master_data_dir().map_err(|_| "environment_registry_invalid".to_string())?;
    // 主库当前账号实测优先（官方登录/登出后注册表缓存过期）；
    // 副环境登录由工具托管，注册表值即权威，不做实测纠正。
    let master_profile_id = reconcile_master_current_profile(
        &registry,
        &accounts,
        &master_dir,
        records
            .iter()
            .find(|record| record.is_master())
            .and_then(|record| record.current_profile_id.as_deref()),
    )
    .current_profile_id;

    let items = records
        .into_iter()
        .map(|record| {
            let data_dir = if record.is_master() {
                master_dir.clone()
            } else {
                secondary_data_dir(storage_root, &record.env_id)
            };
            let process_state = processes
                .as_deref()
                .map(|processes| {
                    product_process_state(
                        processes,
                        TraeProduct::TraeCn,
                        &data_dir,
                        default_data_dir.as_deref(),
                    )
                })
                .unwrap_or(ProductProcessState::Unknown);
            // 体积口径：主库三件套（轻，metadata 求和）；副环境整目录递归。
            let size_bytes = if include_size {
                Some(if record.is_master() {
                    master_trio_size_bytes(&data_dir)
                } else {
                    dir_size_recursive(&data_dir)
                })
            } else {
                None
            };
            // 先取 is_master 再逐字段移动，避免 record 部分移动后再借用。
            let is_master = record.is_master();
            // 主库用实测解析值（reconcile 已同步纠正注册表缓存）。
            let current_profile_id = if is_master {
                master_profile_id.clone()
            } else {
                record.current_profile_id.clone()
            };
            EnvironmentListItemDto {
                env_id: record.env_id,
                name: record.name,
                is_master,
                current_account_name: current_profile_id.as_deref().and_then(account_name),
                current_profile_id,
                data_dir: data_dir.display().to_string(),
                running: process_state == ProductProcessState::Running,
                process_state,
                login_state: trae_instance_module::instance_login_state(&data_dir),
                created_at_unix_seconds: record.created_at_unix_seconds,
                size_bytes,
            }
        })
        .collect();
    Ok(items)
}

/// 创建副环境（空目录 + 档案登记；主库名保留，名称唯一）。
#[tauri::command]
pub(crate) async fn create_environment(
    name: String,
    state: tauri::State<'_, AppState>,
) -> Result<EnvironmentRecordDto, String> {
    ensure_real_preview(&state)?;
    let storage_root = PathBuf::from(&state.storage_root);
    let recovery_root = PathBuf::from(&state.recovery_root);
    run_blocking("environment_create_join_failed", move || {
        let _lifecycle_lease =
            acquire_environment_lease(&recovery_root, &storage_root.join("environments"))?;
        let registry = EnvironmentRegistry::new(&storage_root);
        let record = registry
            .create(&name)
            .map_err(environment_registry_error_code)?;
        // 空目录建档；失败回滚档案行，避免「有档案无目录」的悬空环境。
        let env_dir = secondary_data_dir(&storage_root, &record.env_id);
        if std::fs::create_dir_all(&env_dir).is_err() {
            let _ = registry.remove(&record.env_id);
            return Err("environment_dir_unavailable".to_string());
        }
        Ok(EnvironmentRecordDto {
            env_id: record.env_id,
            name: record.name,
            created_at_unix_seconds: record.created_at_unix_seconds,
        })
    })
    .await
}

/// 重命名副环境（主库不可改名；目录名跟 env_id 走，不涉及文件操作）。
#[tauri::command]
pub(crate) async fn rename_environment(
    env_id: String,
    name: String,
    state: tauri::State<'_, AppState>,
) -> Result<EnvironmentRecordDto, String> {
    ensure_real_preview(&state)?;
    let storage_root = PathBuf::from(&state.storage_root);
    let recovery_root = PathBuf::from(&state.recovery_root);
    run_blocking("environment_rename_join_failed", move || {
        let _lifecycle_lease =
            acquire_environment_lease(&recovery_root, &storage_root.join("environments"))?;
        let registry = EnvironmentRegistry::new(&storage_root);
        let record = registry
            .rename(&env_id, &name)
            .map_err(environment_registry_error_code)?;
        Ok(EnvironmentRecordDto {
            env_id: record.env_id,
            name: record.name,
            created_at_unix_seconds: record.created_at_unix_seconds,
        })
    })
    .await
}

/// 环境删除预览（破坏性操作前的规模知情口径，ADR-0018）。
#[tauri::command]
pub(crate) async fn get_environment_delete_preview(
    env_id: String,
    state: tauri::State<'_, AppState>,
) -> Result<EnvironmentDeletePreviewDto, String> {
    ensure_real_preview(&state)?;
    if state.source_raw_key.is_empty() {
        return Err("source_key_unavailable".to_string());
    }
    let storage_root = PathBuf::from(&state.storage_root);
    let raw_key = state.source_raw_key.clone();
    run_blocking("environment_preview_join_failed", move || {
        use traesync_infrastructure::master_stats::{read_total_scale, TotalScaleStatus};
        let registry = EnvironmentRegistry::new(&storage_root);
        let record = registry
            .find(&env_id)
            .map_err(environment_registry_error_code)?
            .ok_or_else(|| "environment_not_found".to_string())?;
        if record.is_master() {
            return Err("environment_master_immutable".to_string());
        }
        let env_dir = secondary_data_dir(&storage_root, &record.env_id);
        let (status, project_count, session_count) = match read_total_scale(&env_dir, &raw_key) {
            TotalScaleStatus::Ready(scale) => ("ready", scale.project_count, scale.session_count),
            TotalScaleStatus::NoMasterData => ("no_data", 0, 0),
            TotalScaleStatus::ReadFailed => ("read_failed", 0, 0),
        };
        Ok(EnvironmentDeletePreviewDto {
            status,
            project_count,
            session_count,
            size_bytes: dir_size_recursive(&env_dir),
        })
    })
    .await
}

/// 删除副环境（破坏性操作：档案行 + 数据目录连带删除；单次确认由前端
/// 携带预览规模完成）。运行中的环境拒绝删除（先关闭再删）。
#[tauri::command]
pub(crate) async fn delete_environment(
    env_id: String,
    state: tauri::State<'_, AppState>,
) -> Result<bool, String> {
    ensure_real_preview(&state)?;
    let storage_root = PathBuf::from(&state.storage_root);
    let recovery_root = PathBuf::from(&state.recovery_root);
    run_blocking("environment_delete_join_failed", move || {
        delete_environment_inner(&storage_root, &recovery_root, &env_id)
    })
    .await
}

fn delete_environment_inner(
    storage_root: &Path,
    recovery_root: &Path,
    env_id: &str,
) -> Result<bool, String> {
    delete_environment_inner_with_process_check(storage_root, recovery_root, env_id, &|env_dir| {
        // P15-6：删除守卫针对融合版实例进程（与实例启动/关闭同产品语义）。
        crate::process_guard::ensure_product_stopped(
            TraeProduct::TraeCn,
            env_dir,
            "environment_delete_running",
        )
    })
}

/// 删除时容器锁只保护环境身份解析和档案行删除；目标目录锁贯穿检查与删目录。
pub(crate) fn delete_environment_inner_with_process_check(
    storage_root: &Path,
    recovery_root: &Path,
    env_id: &str,
    ensure_stopped: &dyn Fn(&Path) -> Result<(), String>,
) -> Result<bool, String> {
    let lifecycle_lease =
        acquire_environment_lease(recovery_root, &storage_root.join("environments"))?;
    let registry = EnvironmentRegistry::new(storage_root);
    let record = registry
        .find(env_id)
        .map_err(environment_registry_error_code)?
        .ok_or_else(|| "environment_not_found".to_string())?;
    if record.is_master() {
        return Err("environment_master_immutable".to_string());
    }
    let env_dir = secondary_data_dir(storage_root, &record.env_id);
    let _lease = acquire_environment_lease(recovery_root, &env_dir)?;
    drop(lifecycle_lease);

    // 长时间进程观测和递归删除不占用整个环境容器。
    ensure_stopped(&env_dir)?;

    // 删除档案行前重新取得容器锁并复核，避免并发改名/删除绕过生命周期边界。
    let lifecycle_lease =
        acquire_environment_lease(recovery_root, &storage_root.join("environments"))?;
    let current_record = registry
        .find(env_id)
        .map_err(environment_registry_error_code)?
        .ok_or_else(|| "environment_not_found".to_string())?;
    if current_record.is_master() {
        return Err("environment_master_immutable".to_string());
    }
    registry
        .remove(env_id)
        .map_err(environment_registry_error_code)?;
    drop(lifecycle_lease);

    // 档案行先移除，再删目录；失败时保留残留供用户手动处理。
    std::fs::remove_dir_all(&env_dir)
        .map_err(|_| format!("environment_delete_failed_residue:{}", env_dir.display()))?;
    Ok(true)
}

/// 启动/聚焦环境实例（主库沿用 P5-0 既有编排；副环境 --user-data-dir
/// 指向环境目录，环境间并行、环境内单实例，matcher 用带参匹配）。
#[tauri::command]
pub(crate) async fn launch_environment(
    env_id: String,
    state: tauri::State<'_, AppState>,
) -> Result<TraeInstanceLaunchDto, String> {
    ensure_real_preview(&state)?;
    let material_root = checkin_material_root(&state)?;
    let storage_root = PathBuf::from(&state.storage_root);
    let recovery_root = PathBuf::from(&state.recovery_root);
    run_blocking("environment_launch_join_failed", move || {
        // 主库：官方目录口径 + 当前账号窗口标题（P5-0 编排原样复用）。
        if env_id == environment_registry_module::MASTER_ENV_ID {
            let master_dir =
                master_data_dir().map_err(|_| "environment_registry_invalid".to_string())?;
            let _lease = acquire_environment_lease(&recovery_root, &master_dir)?;
            return launch_master_library_inner(&material_root, &storage_root);
        }
        let (record, _lease) =
            acquire_secondary_environment_lease(&recovery_root, &storage_root, &env_id)?;
        let env_dir = secondary_data_dir(&storage_root, &record.env_id);
        // 窗口标题用环境名（并行环境按名区分；账号名可能未登录或跨环境重复）。
        // P15-6：副环境实例由融合版客户端承载（--user-data-dir 并行多开）。
        launch_instance_common(TraeProduct::TraeCn, &env_dir, &record.name, None)
    })
    .await
}

/// P11-5 环境实例关闭回执（close_environment_instance 返回）。
#[derive(Clone, Debug, Serialize)]
pub(crate) struct EnvironmentCloseDto {
    /// closed=优雅关闭完成；force_closed=优雅未成已强制结束；
    /// not_running=本就未运行（幂等，不报错）。
    outcome: &'static str,
}

/// P11-5 关闭环境实例（主库/副环境通用，界面「关闭」按钮后端）：
/// 按各环境既有的进程匹配口径（主库官方目录口径 / 副环境带参匹配）定位
/// 实例进程树，taskkill /T（WM_CLOSE 优雅关闭）先行，TRAE 驻留托盘时再
/// /T /F 强制兜底；未运行幂等返回 not_running。主库/副环境复用同一套
/// 关闭原语按 Work 产品与目标目录显式匹配。
#[tauri::command]
pub(crate) async fn close_environment_instance(
    env_id: String,
    state: tauri::State<'_, AppState>,
) -> Result<EnvironmentCloseDto, String> {
    ensure_real_preview(&state)?;
    let storage_root = PathBuf::from(&state.storage_root);
    let recovery_root = PathBuf::from(&state.recovery_root);
    run_blocking("environment_close_join_failed", move || {
        if env_id == environment_registry_module::MASTER_ENV_ID {
            let master_dir =
                master_data_dir().map_err(|_| "environment_registry_invalid".to_string())?;
            let _lease = acquire_environment_lease(&recovery_root, &master_dir)?;
            close_environment_instance_inner(&storage_root, &env_id)
        } else {
            let (_record, _lease) =
                acquire_secondary_environment_lease(&recovery_root, &storage_root, &env_id)?;
            close_environment_instance_inner(&storage_root, &env_id)
        }
    })
    .await
}

fn close_environment_instance_inner(
    storage_root: &Path,
    env_id: &str,
) -> Result<EnvironmentCloseDto, String> {
    // P15-6：主库/副环境实例都由融合版客户端承载，关闭匹配同切产品。
    let close = |instance_dir: &Path| close_instance(TraeProduct::TraeCn, instance_dir);
    close_environment_instance_inner_with(storage_root, env_id, &close)
}

/// 关闭编排可注入结果，单测据此验证拒绝路径而不操作本机 TRAE 进程。
fn close_environment_instance_inner_with(
    storage_root: &Path,
    env_id: &str,
    close: &dyn Fn(&Path) -> Result<CloseOutcome, trae_instance_module::TraeInstanceCloseError>,
) -> Result<EnvironmentCloseDto, String> {
    // 主库：官方目录口径（覆盖官方无参启动的主进程形态）；副环境：
    // 带参匹配口径（--user-data-dir 指向环境目录）。档案不存在先拒绝，
    // 不做任何进程操作。
    let instance_dir = if env_id == environment_registry_module::MASTER_ENV_ID {
        master_data_dir().map_err(|_| "environment_registry_invalid".to_string())?
    } else {
        let record = EnvironmentRegistry::new(storage_root)
            .find(env_id)
            .map_err(environment_registry_error_code)?
            .ok_or_else(|| "environment_not_found".to_string())?;
        secondary_data_dir(storage_root, &record.env_id)
    };
    let outcome = close(&instance_dir);
    match outcome {
        Ok(CloseOutcome::Closed) => Ok(EnvironmentCloseDto { outcome: "closed" }),
        Ok(CloseOutcome::ForceClosed) => Ok(EnvironmentCloseDto {
            outcome: "force_closed",
        }),
        Ok(CloseOutcome::NotRunning) => Ok(EnvironmentCloseDto {
            outcome: "not_running",
        }),
        // 强制结束后进程仍存在（如权限不足）：稳定错误码，前端提示手动关闭。
        Err(error) => Err(crate::process_guard::close_failure_code(
            error,
            "environment_close_failed",
        )),
    }
}

/// 环境登录账号（P6-4 核心事务）：选环境 + 选账号 → 凭据互换。
///
/// 与主库切号（五步事务）的差异：无插件同步、无自动重启（环境页手动
/// 启动）；其余同构——关实例 → 备份（有库时）→ 凭据互换 → 记录交接
/// （单一归属泛化，ADR-0021）→ 档案写回。
///
/// 空环境首登特例：无 storage.json 密钥材料，E1/E2 互换无从下手，
/// 从该账号登录存档整体播种（移植即登录，无互换对象）。
#[tauri::command]
pub(crate) async fn login_environment(
    env_id: String,
    profile_id: String,
    state: tauri::State<'_, AppState>,
) -> Result<EnvironmentLoginReceiptDto, String> {
    ensure_real_preview(&state)?;
    if state.source_raw_key.is_empty() {
        return Err("source_key_unavailable".to_string());
    }
    let material_root = checkin_material_root(&state)?;
    let storage_root = PathBuf::from(&state.storage_root);
    let recovery_root = PathBuf::from(&state.recovery_root);
    let raw_key = state.source_raw_key.clone();
    run_blocking("environment_login_join_failed", move || {
        login_environment_inner(
            &material_root,
            &storage_root,
            &recovery_root,
            &raw_key,
            &env_id,
            &profile_id,
        )
    })
    .await
}

fn login_environment_inner(
    material_root: &Path,
    storage_root: &Path,
    recovery_root: &Path,
    raw_key: &str,
    env_id: &str,
    profile_id: &str,
) -> Result<EnvironmentLoginReceiptDto, String> {
    let close_environment = |env_dir: &Path| {
        // P15-6：副环境登录前关停融合版实例（与启动/状态同产品语义）。
        close_instance(TraeProduct::TraeCn, env_dir).map(|_| ())
    };
    let switch_identity = |record: &traesync_infrastructure::account_registry::AccountRecord,
                           donor_dir: &Path,
                           env_dir: &Path| {
        switch_login_identity_dual_path(material_root, record, donor_dir, env_dir).map(|_| ())
    };
    login_environment_inner_with_ops(
        material_root,
        storage_root,
        recovery_root,
        raw_key,
        env_id,
        profile_id,
        &close_environment,
        &switch_identity,
        &|| {},
    )
}

/// 环境登录生产/合成数据共用编排；目标租约在主体内覆盖全部副作用。
pub(crate) fn login_environment_inner_with_ops(
    material_root: &Path,
    storage_root: &Path,
    recovery_root: &Path,
    raw_key: &str,
    env_id: &str,
    profile_id: &str,
    close_environment: &dyn Fn(&Path) -> Result<(), trae_instance_module::TraeInstanceCloseError>,
    switch_identity: &dyn Fn(
        &traesync_infrastructure::account_registry::AccountRecord,
        &Path,
        &Path,
    ) -> Result<(), AuthSwitchError>,
    before_return: &dyn Fn(),
) -> Result<EnvironmentLoginReceiptDto, String> {
    // 库作用域备份工具随库域下沉：P12-R3 起经 crate::library_archive 引用
    // （与切号/收编/清理共用同一套备份目录解析与裁剪实现）。
    use crate::library_archive::{prune_backups_if_enabled_in, resolve_library_backup_dir};

    // 主库登录 = 切号五步事务（含插件同步与重启），入口在账号页。
    if env_id == environment_registry_module::MASTER_ENV_ID {
        return Err("environment_master_use_switch".to_string());
    }
    // 前置：目标账号在注册表（account_id 即交接目标 user_id）。
    let records = AccountRegistry::new(material_root)
        .load()
        .map_err(|_| "checkin_registry_invalid".to_string())?;
    let record = records
        .iter()
        .find(|record| record.profile_id == profile_id)
        .ok_or_else(|| "trae_profile_invalid".to_string())?;
    let target_user_id = record.account_id.clone();

    let (env_record, _operation_lease) =
        acquire_secondary_environment_lease(recovery_root, storage_root, env_id)?;
    let env_dir = secondary_data_dir(storage_root, &env_record.env_id);
    let registry = EnvironmentRegistry::new(storage_root);
    let donor_dir =
        instance_data_dir(storage_root, profile_id).map_err(|e| e.code().to_string())?;
    let db_path = master_database_path(&env_dir);

    // 生成中检测（口径同主库切号 Q1.2）：WAL 活跃时拒绝，等空闲再登。
    if db_path.is_file()
        && master_db_activity_detected(&db_path, std::time::Duration::from_millis(800))
    {
        return Err("environment_login_busy".to_string());
    }
    // 关闭环境实例（幂等；凭据写库前目录必须静止）。
    close_environment(&env_dir).map_err(|error| {
        crate::process_guard::close_failure_code(error, "environment_login_close_failed")
    })?;

    let mut seeded = false;
    let (mut transferred_projects, mut switched_sessions, mut removed_mirror_rows) = (0, 0, 0);

    match trae_instance_module::instance_login_state(&env_dir) {
        trae_instance_module::InstanceLoginState::LoggedIn
        | trae_instance_module::InstanceLoginState::Stale => {
            // 既有登录 → 双路径互换（E2 凭据构造首选 / E1 存档移植降级，
            // 与主库切号第 3 步同一编排，仅目标目录不同）。
            // 备份铁律（ADR-0018）：交接动库前先留三件套副本。
            if db_path.is_file() {
                // P11-4①/P11-5⑤：写入库作用域活动备份目录——副环境链落在
                // 覆盖目录的环境子目录（未覆盖时 db 同目录），不与主库链混放。
                let backup_dir = resolve_library_backup_dir(storage_root, &db_path, env_id);
                backup_master_trio_to(&db_path, Some(&backup_dir))
                    .map_err(|_| "environment_login_backup_failed".to_string())?;
                // P5-9：备份生成后按保留策略清理旧备份（静默容错，只动该库链）。
                prune_backups_if_enabled_in(&db_path, storage_root, &backup_dir);
            }
            // 回滚材料：互换前的登录态原始字节（P7-2 杂交态防线）。
            let storage_path = env_dir
                .join("User")
                .join("globalStorage")
                .join("storage.json");
            let raw_before = std::fs::read_to_string(&storage_path).ok();
            let rollback = |code: &str| -> String {
                match raw_before.as_deref() {
                    Some(original) if rollback_master_login_bytes(&storage_path, original) => {
                        code.to_string()
                    }
                    Some(_) => "environment_login_rollback_failed".to_string(),
                    // 第 3 步从未写库：环境天然处于登录前状态，直接返回原错误码。
                    None => code.to_string(),
                }
            };
            match switch_identity(record, &donor_dir, &env_dir) {
                Ok(()) => {}
                Err(AuthSwitchError::DonorAuthUnavailable) => {
                    return Err(rollback("switch_donor_login_missing"))
                }
                // 环境内互换极少出现的其余形态（目标 blob 损坏等）：统一失败码。
                Err(_) => return Err(rollback("environment_login_failed")),
            }
            // 记录交接（单一归属泛化：库内记录随登录归一到目标账号）。
            if db_path.is_file() {
                match handover_master_records(&db_path, raw_key, &target_user_id) {
                    Ok(handover) => {
                        transferred_projects = handover.transferred_projects;
                        switched_sessions = handover.switched_sessions.len();
                        removed_mirror_rows = handover.removed_mirror_rows;
                    }
                    Err(error) => {
                        let code = match &error {
                            MasterHandoverError::CommittedNeedsAttention => {
                                "environment_login_committed_needs_attention"
                            }
                            MasterHandoverError::CommitOutcomeUnknown => {
                                "environment_login_commit_outcome_unknown"
                            }
                            MasterHandoverError::DbUnavailable => "environment_login_db_missing",
                            MasterHandoverError::DbOpenFailed => "environment_login_db_open_failed",
                            MasterHandoverError::TargetConflict(_) => "environment_login_conflict",
                            // P13-2 新增变体只在 CN 归并路径产生，环境登录
                            // 不会遇到；归入统一失败码保持穷尽匹配。
                            MasterHandoverError::ProjectNotFound
                            | MasterHandoverError::ProjectNotMergeable
                            | MasterHandoverError::NothingToMerge => "environment_login_failed",
                            MasterHandoverError::Io => "environment_login_failed",
                        };
                        return Err(finalize_handover_failure(&error, code, rollback));
                    }
                }
            }
        }
        trae_instance_module::InstanceLoginState::Uninitialized
        | trae_instance_module::InstanceLoginState::LoggedOut => {
            // 空环境首登（或启动过未登录）：从账号登录存档播种三件套，
            // 移植即登录——无互换对象，无记录可交接（空库跳过）。
            seed_environment_login_from_donor(&env_dir, &donor_dir, &target_user_id)?;
            seeded = true;
        }
    }

    // 档案写回当前登录账号（各环境各自记忆，多对多）。
    let current_profile_saved = match registry.set_current_profile(env_id, profile_id) {
        Ok(_) => true,
        Err(error) => {
            eprintln!("[environment-login] 环境档案写回失败（账号切换已完成）：{error:?}");
            false
        }
    };
    before_return();
    Ok(EnvironmentLoginReceiptDto {
        env_id: env_id.to_string(),
        profile_id: profile_id.to_string(),
        seeded,
        transferred_projects,
        switched_sessions,
        removed_mirror_rows,
        current_profile_saved,
    })
}

fn seed_environment_login_from_donor(
    environment_dir: &Path,
    donor_dir: &Path,
    expected_user_id: &str,
) -> Result<(), String> {
    if expected_user_id.is_empty()
        || archive_login_user_id(donor_dir).as_deref() != Some(expected_user_id)
    {
        return Err("environment_seed_unavailable".to_string());
    }
    seed_login_state_from_donor(environment_dir, donor_dir)
        .map(|_| ())
        .map_err(|_| "environment_seed_unavailable".to_string())
}

// ============================================================================
// 主库当前账号实测纠正（2026-09-03 卡死案例修复）：用户在 TRAE 官方
// 登录/登出后环境注册表 current_profile_id 过期，读取路径须实测优先。
// ============================================================================
#[cfg(test)]
mod master_current_account_reconcile_tests {
    use super::*;
    use crate::resolve_actual_master_account;
    use traesync_infrastructure::account_registry::AccountRecord;
    use traesync_infrastructure::{construct_auth_identity, ConstructAuthInput, UserInfoFull};

    /// 构造测试账号档案（仅身份字段有意义，其余填合法占位）。
    fn account(profile_id: &str, account_id: &str, screen_name: &str) -> AccountRecord {
        AccountRecord {
            profile_id: profile_id.to_string(),
            account_id: account_id.to_string(),
            screen_name: screen_name.to_string(),
            avatar_url: String::new(),
            device_id: format!("device-{profile_id}"),
            device_public_key: format!("pub-{profile_id}"),
            display_name: None,
            masked_mobile: String::new(),
            created_at_unix_seconds: 1,
            last_verified_at_unix_seconds: 1,
            device_created_at_unix_seconds: 0,
            auto_checkin_enabled: false,
            archived: false,
        }
    }

    /// 在临时实例目录播种登录态（E2 构造路径，登出态目录全新生成 blob）。
    fn seed_login(instance_dir: &Path, account_id: &str) {
        let info = UserInfoFull {
            user_id: account_id.to_string(),
            screen_name: format!("用户{account_id}"),
            ..Default::default()
        };
        let input = ConstructAuthInput {
            account_id,
            access_token: "test-token",
            refresh_token: "test-refresh",
            access_token_expires_at_unix_seconds: 4_000_000_000,
            refresh_token_expires_at_unix_seconds: 4_100_000_000,
            user_info: &info,
        };
        construct_auth_identity(&input, instance_dir).unwrap();
    }

    /// 测试根：环境注册表 + 账号注册表 + 主库实例目录三件套。
    struct ReconcileFixture {
        root: tempfile::TempDir,
    }

    impl ReconcileFixture {
        fn new() -> Self {
            Self {
                root: tempfile::tempdir().unwrap(),
            }
        }

        fn registry(&self) -> EnvironmentRegistry {
            EnvironmentRegistry::new(self.storage_root())
        }

        fn storage_root(&self) -> PathBuf {
            self.root.path().to_path_buf()
        }

        fn account_registry(&self) -> AccountRegistry {
            AccountRegistry::new(self.root.path().join("material"))
        }

        fn material_root(&self) -> PathBuf {
            self.root.path().join("material")
        }

        fn master_dir(&self) -> PathBuf {
            self.root.path().join("master-instance")
        }
    }

    #[test]
    fn observed_login_overrides_stale_registry_and_writes_back() {
        let fixture = ReconcileFixture::new();
        // 注册表缓存：账号 A（过期值）；主库实际登录：账号 B。
        let accounts = vec![
            account("checkin-a", "111", "账号A"),
            account("checkin-b", "222", "账号B"),
        ];
        for record in &accounts {
            fixture.account_registry().upsert(record).unwrap();
        }
        fixture
            .registry()
            .set_current_profile(MASTER_ENV_ID, "checkin-a")
            .unwrap();
        seed_login(&fixture.master_dir(), "222");

        let actual = resolve_actual_master_account(
            &fixture.material_root(),
            &fixture.storage_root(),
            &fixture.master_dir(),
        )
        .unwrap()
        .expect("已登录且已登记的主库账号应可解析");
        assert_eq!(actual.profile_id, "checkin-b");

        // 实测优先：返回 B 的 profile，而非缓存 A。
        let resolved = reconcile_master_current_profile(
            &fixture.registry(),
            &fixture.account_registry().load().unwrap(),
            &fixture.master_dir(),
            Some("checkin-a"),
        );
        assert_eq!(resolved.current_profile_id.as_deref(), Some("checkin-b"));
        assert_eq!(resolved.observed_profile_id.as_deref(), Some("checkin-b"));
        // 注册表被纠正为实测值（插件同步预检等消费方随注册表一并修正）。
        let persisted = fixture.registry().load().unwrap().unwrap();
        assert_eq!(persisted.current_profile_id.as_deref(), Some("checkin-b"));
    }

    #[test]
    fn logged_out_master_keeps_cached_profile() {
        let fixture = ReconcileFixture::new();
        fixture
            .registry()
            .set_current_profile(MASTER_ENV_ID, "checkin-a")
            .unwrap();
        // 主库登出态：无 storage.json。
        let accounts = vec![account("checkin-a", "111", "账号A")];

        let resolved = reconcile_master_current_profile(
            &fixture.registry(),
            &accounts,
            &fixture.master_dir(),
            Some("checkin-a"),
        );
        assert_eq!(resolved.current_profile_id.as_deref(), Some("checkin-a"));
        assert_eq!(resolved.observed_profile_id, None);
        // 注册表不动（最后已知账号保留，不写垃圾值）。
        let persisted = fixture.registry().load().unwrap().unwrap();
        assert_eq!(persisted.current_profile_id.as_deref(), Some("checkin-a"));
    }

    #[test]
    fn data_paths_require_observed_master_account_instead_of_cache() {
        let fixture = ReconcileFixture::new();
        let record = account("checkin-a", "111", "账号A");
        fixture.account_registry().upsert(&record).unwrap();
        fixture
            .registry()
            .set_current_profile(MASTER_ENV_ID, "checkin-a")
            .unwrap();

        // 登出后不能继续用缓存账号读取或修改主库归属。
        assert!(resolve_actual_master_account(
            &fixture.material_root(),
            &fixture.storage_root(),
            &fixture.master_dir(),
        )
        .unwrap()
        .is_none());
    }

    #[test]
    fn unregistered_login_keeps_cached_profile() {
        let fixture = ReconcileFixture::new();
        fixture
            .registry()
            .set_current_profile(MASTER_ENV_ID, "checkin-a")
            .unwrap();
        // 主库登录了未登记账号（官方登录陌生账号）：实测无法映射 profile。
        seed_login(&fixture.master_dir(), "999");
        let accounts = vec![account("checkin-a", "111", "账号A")];

        let resolved = reconcile_master_current_profile(
            &fixture.registry(),
            &accounts,
            &fixture.master_dir(),
            Some("checkin-a"),
        );
        assert_eq!(resolved.current_profile_id.as_deref(), Some("checkin-a"));
        assert_eq!(resolved.observed_profile_id, None);
        // 注册表不动（不把未登记账号写进档案）。
        let persisted = fixture.registry().load().unwrap().unwrap();
        assert_eq!(persisted.current_profile_id.as_deref(), Some("checkin-a"));
    }

    #[test]
    fn consistent_observation_skips_registry_write() {
        let fixture = ReconcileFixture::new();
        fixture
            .registry()
            .set_current_profile(MASTER_ENV_ID, "checkin-a")
            .unwrap();
        seed_login(&fixture.master_dir(), "111");
        let accounts = vec![account("checkin-a", "111", "账号A")];

        // 实测 == 缓存：返回不变（幂等，不触发注册表写）。
        let resolved = reconcile_master_current_profile(
            &fixture.registry(),
            &accounts,
            &fixture.master_dir(),
            Some("checkin-a"),
        );
        assert_eq!(resolved.current_profile_id.as_deref(), Some("checkin-a"));
        assert_eq!(resolved.observed_profile_id.as_deref(), Some("checkin-a"));
    }
}

// ============================================================================
// P11-5 环境命令 inner 层：删除残留路径错误 + 关闭实例幂等。
// close 的主库分支依赖真实 APPDATA 目录，可能触及用户正在运行的主库实例，
// 不做单测（由 e2e / 人工验证）；副环境分支用 tempdir data_dir，安全。
// ============================================================================

#[cfg(test)]
mod environment_commands_tests {
    use super::*;
    use std::io::{BufRead, BufReader, Write};
    use std::process::{Child, ChildStdin, Command, ExitStatus, Stdio};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::mpsc;

    fn poll_child_until_exit(
        child: &mut Child,
        timeout: std::time::Duration,
    ) -> Option<ExitStatus> {
        let deadline = std::time::Instant::now() + timeout;
        loop {
            match child.try_wait() {
                Ok(Some(status)) => return Some(status),
                Ok(None) if std::time::Instant::now() < deadline => {
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
                Ok(None) | Err(_) => return None,
            }
        }
    }

    fn kill_and_reap_child(child: &mut Child) {
        let _ = child.kill();
        let _ = child.wait();
    }

    fn release_and_reap_lease_holder(
        child: &mut Child,
        mut stdin: ChildStdin,
    ) -> Option<ExitStatus> {
        let _ = stdin.write_all(b"\n");
        drop(stdin);
        let status = poll_child_until_exit(child, std::time::Duration::from_secs(5));
        if status.is_none() {
            kill_and_reap_child(child);
        }
        status
    }

    /// 测试根：storage_root + 按需登记的副环境档案。
    struct EnvFixture {
        root: tempfile::TempDir,
    }

    impl EnvFixture {
        fn new() -> Self {
            Self {
                root: tempfile::tempdir().unwrap(),
            }
        }

        fn create_env(
            &self,
            name: &str,
        ) -> traesync_infrastructure::environment_registry::EnvironmentRecord {
            EnvironmentRegistry::new(self.root.path())
                .create(name)
                .unwrap()
        }
    }

    fn save_test_account(material_root: &Path, profile_id: &str, account_id: &str) {
        let record = traesync_infrastructure::account_registry::AccountRecord {
            profile_id: profile_id.to_string(),
            account_id: account_id.to_string(),
            screen_name: "测试账号".to_string(),
            avatar_url: String::new(),
            device_id: format!("device-{profile_id}"),
            device_public_key: format!("pub-{profile_id}"),
            display_name: None,
            masked_mobile: String::new(),
            created_at_unix_seconds: 1,
            last_verified_at_unix_seconds: 1,
            device_created_at_unix_seconds: 0,
            auto_checkin_enabled: false,
            archived: false,
        };
        AccountRegistry::new(material_root).upsert(&record).unwrap();
    }

    /// 子进程持有副环境数据目录租约，供真实 caller 的跨进程屏障测试使用。
    #[test]
    fn child_process_holds_secondary_environment_lease_for_caller_test() {
        let (Ok(recovery_root), Ok(storage_root), Ok(env_id)) = (
            std::env::var("TRAE_SYNC_ENV_CALLER_LOCK_ROOT"),
            std::env::var("TRAE_SYNC_ENV_CALLER_LOCK_STORAGE_ROOT"),
            std::env::var("TRAE_SYNC_ENV_CALLER_LOCK_ENV_ID"),
        ) else {
            return;
        };
        let env_dir = secondary_data_dir(std::path::Path::new(&storage_root), &env_id);
        let _lease =
            acquire_environment_lease(std::path::Path::new(&recovery_root), &env_dir).unwrap();
        let mut stdout = std::io::stdout().lock();
        writeln!(stdout, "ENVIRONMENT_CALLER_LOCK_READY").unwrap();
        stdout.flush().unwrap();
        let mut line = String::new();
        let _ = std::io::stdin().lock().read_line(&mut line);
    }

    /// 子进程执行真实环境删除 caller，断言锁竞争先于进程检查和删除。
    #[test]
    fn child_runs_environment_delete_caller_for_cross_process_test() {
        if std::env::var("TRAE_SYNC_ENV_DELETE_CALLER_RUN")
            .ok()
            .as_deref()
            != Some("1")
        {
            return;
        }
        let recovery_root = std::path::PathBuf::from(
            std::env::var("TRAE_SYNC_ENV_DELETE_CALLER_RUN_RECOVERY_ROOT").unwrap(),
        );
        let storage_root = std::path::PathBuf::from(
            std::env::var("TRAE_SYNC_ENV_DELETE_CALLER_RUN_STORAGE_ROOT").unwrap(),
        );
        let env_id = std::env::var("TRAE_SYNC_ENV_DELETE_CALLER_RUN_ENV_ID").unwrap();
        let env_dir = secondary_data_dir(&storage_root, &env_id);
        let process_checks = AtomicUsize::new(0);
        let result = delete_environment_inner_with_process_check(
            &storage_root,
            &recovery_root,
            &env_id,
            &|_| {
                process_checks.fetch_add(1, Ordering::SeqCst);
                Ok(())
            },
        );
        let error = match result {
            Err(error) => error,
            Ok(_) => panic!("其他进程持有副环境租约时不应删除环境"),
        };
        let expected = format!(
            "operation_lease_unavailable:{}",
            traesync_infrastructure::operation_lease::OperationLeaseError::DataLocationBusy
        );
        assert_eq!(error, expected);
        assert_eq!(process_checks.load(Ordering::SeqCst), 0);
        assert!(EnvironmentRegistry::new(&storage_root)
            .find(&env_id)
            .unwrap()
            .is_some());
        assert!(env_dir.join("keep.txt").is_file());
    }

    /// 子进程执行真实环境登录 caller，断言锁竞争早于关闭和认证写入。
    #[test]
    fn child_runs_environment_login_caller_for_cross_process_test() {
        if std::env::var("TRAE_SYNC_ENV_LOGIN_CALLER_RUN")
            .ok()
            .as_deref()
            != Some("1")
        {
            return;
        }
        let recovery_root = std::path::PathBuf::from(
            std::env::var("TRAE_SYNC_ENV_LOGIN_CALLER_RUN_RECOVERY_ROOT").unwrap(),
        );
        let storage_root = std::path::PathBuf::from(
            std::env::var("TRAE_SYNC_ENV_LOGIN_CALLER_RUN_STORAGE_ROOT").unwrap(),
        );
        let material_root = std::path::PathBuf::from(
            std::env::var("TRAE_SYNC_ENV_LOGIN_CALLER_RUN_MATERIAL_ROOT").unwrap(),
        );
        let env_id = std::env::var("TRAE_SYNC_ENV_LOGIN_CALLER_RUN_ENV_ID").unwrap();
        let profile_id = std::env::var("TRAE_SYNC_ENV_LOGIN_CALLER_RUN_PROFILE_ID").unwrap();
        let close_calls = AtomicUsize::new(0);
        let identity_switch_calls = AtomicUsize::new(0);
        let before_return_calls = AtomicUsize::new(0);
        let result = login_environment_inner_with_ops(
            &material_root,
            &storage_root,
            &recovery_root,
            "unused-test-key",
            &env_id,
            &profile_id,
            &|_| {
                close_calls.fetch_add(1, Ordering::SeqCst);
                Ok(())
            },
            &|_, _, _| {
                identity_switch_calls.fetch_add(1, Ordering::SeqCst);
                Ok(())
            },
            &|| {
                before_return_calls.fetch_add(1, Ordering::SeqCst);
            },
        );
        let error = match result {
            Err(error) => error,
            Ok(_) => panic!("其他进程持有副环境租约时不应执行环境登录"),
        };
        let expected = format!(
            "operation_lease_unavailable:{}",
            traesync_infrastructure::operation_lease::OperationLeaseError::DataLocationBusy
        );
        assert_eq!(error, expected);
        assert_eq!(close_calls.load(Ordering::SeqCst), 0);
        assert_eq!(identity_switch_calls.load(Ordering::SeqCst), 0);
        assert_eq!(before_return_calls.load(Ordering::SeqCst), 0);
        let env_dir = secondary_data_dir(&storage_root, &env_id);
        assert!(env_dir.join("keep.txt").is_file());
        assert!(EnvironmentRegistry::new(&storage_root)
            .find(&env_id)
            .unwrap()
            .is_some());
    }

    /// 删除与登录 caller 的跨进程锁等待、执行和退出都必须有时限及回收路径。
    #[test]
    fn environment_callers_reject_cross_process_lease_before_side_effects() {
        let fixture = EnvFixture::new();
        let environment = fixture.create_env("跨进程删除测试");
        let env_dir = secondary_data_dir(fixture.root.path(), &environment.env_id);
        std::fs::create_dir_all(&env_dir).unwrap();
        std::fs::write(env_dir.join("keep.txt"), "keep").unwrap();
        let recovery_root = tempfile::tempdir().unwrap();
        let material_root = tempfile::tempdir().unwrap();
        let profile_id = "checkin-target";
        save_test_account(material_root.path(), profile_id, "777");
        let test_exe = std::env::current_exe().unwrap();

        let mut holder = Command::new(&test_exe)
            .env("TRAE_SYNC_ENV_CALLER_LOCK_ROOT", recovery_root.path())
            .env(
                "TRAE_SYNC_ENV_CALLER_LOCK_STORAGE_ROOT",
                fixture.root.path(),
            )
            .env("TRAE_SYNC_ENV_CALLER_LOCK_ENV_ID", &environment.env_id)
            .args([
                "--exact",
                "environment::environment_commands_tests::child_process_holds_secondary_environment_lease_for_caller_test",
                "--nocapture",
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let holder_stdin = holder.stdin.take().unwrap();
        let stdout = holder.stdout.take().unwrap();
        let (ready_tx, ready_rx) = mpsc::channel();
        std::thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            let mut line = String::new();
            let mut ready_sent = false;
            loop {
                line.clear();
                match reader.read_line(&mut line) {
                    Ok(0) | Err(_) => {
                        if !ready_sent {
                            let _ = ready_tx.send(false);
                        }
                        return;
                    }
                    Ok(_) if !ready_sent && line.trim() == "ENVIRONMENT_CALLER_LOCK_READY" => {
                        let _ = ready_tx.send(true);
                        ready_sent = true;
                    }
                    Ok(_) => {}
                }
            }
        });
        if ready_rx.recv_timeout(std::time::Duration::from_secs(10)) != Ok(true) {
            let _ = release_and_reap_lease_holder(&mut holder, holder_stdin);
            panic!("持锁子进程未在 10 秒内报告就绪");
        }

        let caller = Command::new(&test_exe)
            .env("TRAE_SYNC_ENV_DELETE_CALLER_RUN", "1")
            .env(
                "TRAE_SYNC_ENV_DELETE_CALLER_RUN_RECOVERY_ROOT",
                recovery_root.path(),
            )
            .env(
                "TRAE_SYNC_ENV_DELETE_CALLER_RUN_STORAGE_ROOT",
                fixture.root.path(),
            )
            .env("TRAE_SYNC_ENV_DELETE_CALLER_RUN_ENV_ID", &environment.env_id)
            .args([
                "--exact",
                "environment::environment_commands_tests::child_runs_environment_delete_caller_for_cross_process_test",
                "--nocapture",
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .spawn();
        let mut caller = match caller {
            Ok(child) => child,
            Err(error) => {
                let _ = release_and_reap_lease_holder(&mut holder, holder_stdin);
                panic!("无法启动环境删除 caller 测试子进程：{error}");
            }
        };

        let caller_status = poll_child_until_exit(&mut caller, std::time::Duration::from_secs(10));
        if caller_status.is_none() {
            let _ = release_and_reap_lease_holder(&mut holder, holder_stdin);
            let _ = poll_child_until_exit(&mut caller, std::time::Duration::from_secs(5));
            if caller.try_wait().ok().flatten().is_none() {
                kill_and_reap_child(&mut caller);
            }
            panic!("环境删除 caller 未在持锁期间 10 秒内返回；已释放并回收测试子进程");
        }

        let login_caller = Command::new(&test_exe)
            .env("TRAE_SYNC_ENV_LOGIN_CALLER_RUN", "1")
            .env(
                "TRAE_SYNC_ENV_LOGIN_CALLER_RUN_RECOVERY_ROOT",
                recovery_root.path(),
            )
            .env(
                "TRAE_SYNC_ENV_LOGIN_CALLER_RUN_STORAGE_ROOT",
                fixture.root.path(),
            )
            .env(
                "TRAE_SYNC_ENV_LOGIN_CALLER_RUN_MATERIAL_ROOT",
                material_root.path(),
            )
            .env(
                "TRAE_SYNC_ENV_LOGIN_CALLER_RUN_ENV_ID",
                &environment.env_id,
            )
            .env("TRAE_SYNC_ENV_LOGIN_CALLER_RUN_PROFILE_ID", profile_id)
            .args([
                "--exact",
                "environment::environment_commands_tests::child_runs_environment_login_caller_for_cross_process_test",
                "--nocapture",
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .spawn();
        let mut login_caller = match login_caller {
            Ok(child) => child,
            Err(error) => {
                let _ = release_and_reap_lease_holder(&mut holder, holder_stdin);
                panic!("无法启动环境登录 caller 测试子进程：{error}");
            }
        };
        let login_status =
            poll_child_until_exit(&mut login_caller, std::time::Duration::from_secs(10));
        if login_status.is_none() {
            let _ = release_and_reap_lease_holder(&mut holder, holder_stdin);
            let _ = poll_child_until_exit(&mut login_caller, std::time::Duration::from_secs(5));
            if login_caller.try_wait().ok().flatten().is_none() {
                kill_and_reap_child(&mut login_caller);
            }
            panic!("环境登录 caller 未在持锁期间 10 秒内返回；已释放并回收测试子进程");
        }

        let holder_status = release_and_reap_lease_holder(&mut holder, holder_stdin);
        assert!(holder_status.is_some_and(|status| status.success()));
        assert!(caller_status.unwrap().success());
        assert!(login_status.unwrap().success());
        assert!(env_dir.join("keep.txt").is_file());
        assert!(EnvironmentRegistry::new(fixture.root.path())
            .find(&environment.env_id)
            .unwrap()
            .is_some());
    }

    /// 生命周期根锁保护创建/删除窗口，单环境锁允许其他环境并行。
    #[test]
    fn secondary_environment_leases_guard_lifecycle_and_allow_other_environments() {
        let fixture = EnvFixture::new();
        let first = fixture.create_env("测试环境甲");
        let second = fixture.create_env("测试环境乙");
        std::fs::create_dir_all(secondary_data_dir(fixture.root.path(), &first.env_id)).unwrap();
        std::fs::create_dir_all(secondary_data_dir(fixture.root.path(), &second.env_id)).unwrap();
        let recovery_root = tempfile::tempdir().unwrap();

        let lifecycle_lease = acquire_environment_lease(
            recovery_root.path(),
            &fixture.root.path().join("environments"),
        )
        .unwrap();
        assert!(acquire_secondary_environment_lease(
            recovery_root.path(),
            fixture.root.path(),
            &first.env_id,
        )
        .is_err());
        drop(lifecycle_lease);

        let (_first_record, first_lease) = acquire_secondary_environment_lease(
            recovery_root.path(),
            fixture.root.path(),
            &first.env_id,
        )
        .unwrap();
        assert!(acquire_secondary_environment_lease(
            recovery_root.path(),
            fixture.root.path(),
            &first.env_id,
        )
        .is_err());
        let (_second_record, second_lease) = acquire_secondary_environment_lease(
            recovery_root.path(),
            fixture.root.path(),
            &second.env_id,
        )
        .unwrap();
        drop(second_lease);
        drop(first_lease);
    }

    #[test]
    fn delete_environment_success_removes_directory() {
        let fixture = EnvFixture::new();
        let record = fixture.create_env("测试环境");
        let env_dir = secondary_data_dir(fixture.root.path(), &record.env_id);
        std::fs::create_dir_all(&env_dir).unwrap();
        let recovery_root = tempfile::tempdir().unwrap();

        delete_environment_inner_with_process_check(
            fixture.root.path(),
            recovery_root.path(),
            &record.env_id,
            &|_| Ok(()),
        )
        .unwrap();

        // 档案行与目录一并移除。
        assert!(!env_dir.exists());
        assert!(EnvironmentRegistry::new(fixture.root.path())
            .find(&record.env_id)
            .unwrap()
            .is_none());
    }

    #[test]
    fn deleting_one_environment_keeps_its_lock_and_allows_another_environment() {
        let fixture = EnvFixture::new();
        let first = fixture.create_env("测试环境甲");
        let second = fixture.create_env("测试环境乙");
        let first_dir = secondary_data_dir(fixture.root.path(), &first.env_id);
        let second_dir = secondary_data_dir(fixture.root.path(), &second.env_id);
        std::fs::create_dir_all(&first_dir).unwrap();
        std::fs::create_dir_all(&second_dir).unwrap();
        std::fs::write(first_dir.join("keep.txt"), "keep").unwrap();
        let recovery_root = tempfile::tempdir().unwrap();
        let storage_path = fixture.root.path().to_path_buf();
        let recovery_path = recovery_root.path().to_path_buf();
        let first_id = first.env_id.clone();
        let (first_started_tx, first_started_rx) = std::sync::mpsc::channel();
        let (release_first_tx, release_first_rx) = std::sync::mpsc::channel();
        let first_delete = std::thread::spawn(move || {
            delete_environment_inner_with_process_check(
                &storage_path,
                &recovery_path,
                &first_id,
                &|_| {
                    first_started_tx.send(()).unwrap();
                    release_first_rx
                        .recv_timeout(std::time::Duration::from_secs(5))
                        .map_err(|_| "test release timeout".to_string())?;
                    Ok(())
                },
            )
        });
        first_started_rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap();

        // 相同数据目录仍被保护，但其他环境可越过短暂的容器锁继续删除。
        assert!(acquire_secondary_environment_lease(
            recovery_root.path(),
            fixture.root.path(),
            &first.env_id,
        )
        .is_err());
        assert!(first_dir.join("keep.txt").is_file());
        assert!(EnvironmentRegistry::new(fixture.root.path())
            .find(&first.env_id)
            .unwrap()
            .is_some());

        let storage_path = fixture.root.path().to_path_buf();
        let recovery_path = recovery_root.path().to_path_buf();
        let second_id = second.env_id.clone();
        let (second_started_tx, second_started_rx) = std::sync::mpsc::channel();
        let second_delete = std::thread::spawn(move || {
            delete_environment_inner_with_process_check(
                &storage_path,
                &recovery_path,
                &second_id,
                &|_| {
                    second_started_tx.send(()).unwrap();
                    Ok(())
                },
            )
        });
        second_started_rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap();
        second_delete.join().unwrap().unwrap();
        assert!(!second_dir.exists());

        release_first_tx.send(()).unwrap();
        first_delete.join().unwrap().unwrap();
        assert!(!first_dir.exists());
        assert!(EnvironmentRegistry::new(fixture.root.path())
            .find(&first.env_id)
            .unwrap()
            .is_none());
    }

    #[test]
    fn delete_environment_rejects_master_and_missing() {
        let fixture = EnvFixture::new();
        let recovery_root = tempfile::tempdir().unwrap();
        // 主库先建档：命中「主库不可删除」而非「档案不存在」。
        EnvironmentRegistry::new(fixture.root.path())
            .ensure_master()
            .unwrap();
        assert_eq!(
            delete_environment_inner_with_process_check(
                fixture.root.path(),
                recovery_root.path(),
                MASTER_ENV_ID,
                &|_| panic!("主库档案不应查询进程"),
            )
            .unwrap_err(),
            "environment_master_immutable"
        );
        assert_eq!(
            delete_environment_inner_with_process_check(
                fixture.root.path(),
                recovery_root.path(),
                "env-none",
                &|_| panic!("缺失档案不应查询进程"),
            )
            .unwrap_err(),
            "environment_not_found"
        );
    }

    #[test]
    fn delete_environment_unknown_process_state_preserves_record_and_directory() {
        let fixture = EnvFixture::new();
        let record = fixture.create_env("测试环境");
        let env_dir = secondary_data_dir(fixture.root.path(), &record.env_id);
        std::fs::create_dir_all(&env_dir).unwrap();
        std::fs::write(env_dir.join("keep.txt"), "keep").unwrap();
        let recovery_root = tempfile::tempdir().unwrap();

        let error = delete_environment_inner_with_process_check(
            fixture.root.path(),
            recovery_root.path(),
            &record.env_id,
            &|_| Err("process_state_unknown".to_string()),
        )
        .unwrap_err();

        assert_eq!(error, "process_state_unknown");
        assert!(env_dir.join("keep.txt").is_file());
        assert!(EnvironmentRegistry::new(fixture.root.path())
            .find(&record.env_id)
            .unwrap()
            .is_some());
        assert!(acquire_environment_lease(recovery_root.path(), &env_dir).is_ok());
    }

    #[test]
    fn delete_environment_directory_failure_reports_residue_path() {
        let fixture = EnvFixture::new();
        let record = fixture.create_env("测试环境");
        // 在 env_dir 位置放一个普通文件：remove_dir_all 对非目录必然失败，
        // 以此构造「目录删除失败」而不依赖进程锁。
        let env_dir = secondary_data_dir(fixture.root.path(), &record.env_id);
        std::fs::create_dir_all(env_dir.parent().unwrap()).unwrap();
        std::fs::write(&env_dir, "not a directory").unwrap();
        let recovery_root = tempfile::tempdir().unwrap();

        let error = delete_environment_inner_with_process_check(
            fixture.root.path(),
            recovery_root.path(),
            &record.env_id,
            &|_| Ok(()),
        )
        .unwrap_err();

        // P11-5：稳定错误码 + 完整残留路径（冒号后为路径），供前端提示
        // 用户手动清理定位。
        assert_eq!(
            error,
            format!("environment_delete_failed_residue:{}", env_dir.display())
        );
        // 残留物仍在（不静默删除，铁律）。
        assert!(env_dir.exists());
        assert!(acquire_environment_lease(recovery_root.path(), &env_dir).is_ok());
    }

    /// close 原语仅 Windows（taskkill）；进程列表查询只读安全。
    #[cfg(windows)]
    #[test]
    fn close_environment_missing_reports_not_found_without_process_actions() {
        let fixture = EnvFixture::new();
        let process_actions = std::sync::atomic::AtomicUsize::new(0);
        let close = |_dir: &Path| {
            process_actions.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(CloseOutcome::NotRunning)
        };
        // 档案不存在：先拒绝，不做任何进程操作。
        assert_eq!(
            close_environment_instance_inner_with(fixture.root.path(), "env-none", &close)
                .unwrap_err(),
            "environment_not_found"
        );
        assert_eq!(process_actions.load(std::sync::atomic::Ordering::SeqCst), 0);
    }

    /// 未运行的关闭回执可重复得到，测试不查询或结束系统进程。
    #[cfg(windows)]
    #[test]
    fn close_environment_not_running_is_idempotent() {
        let fixture = EnvFixture::new();
        let record = fixture.create_env("测试环境");
        let process_checks = std::sync::atomic::AtomicUsize::new(0);
        let close = |_dir: &Path| {
            process_checks.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(CloseOutcome::NotRunning)
        };
        for _ in 0..2 {
            let receipt =
                close_environment_instance_inner_with(fixture.root.path(), &record.env_id, &close)
                    .unwrap();
            assert_eq!(receipt.outcome, "not_running");
        }
        assert_eq!(process_checks.load(std::sync::atomic::Ordering::SeqCst), 2);
    }

    /// 无法确认候选进程身份时保留未知原因，且不谎报未运行。
    #[cfg(windows)]
    #[test]
    fn close_environment_unknown_process_state_is_reported() {
        let fixture = EnvFixture::new();
        let record = fixture.create_env("测试环境");
        let process_checks = std::sync::atomic::AtomicUsize::new(0);
        let close = |_dir: &Path| {
            process_checks.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Err(trae_instance_module::TraeInstanceCloseError::ProcessStateUnknown)
        };

        assert_eq!(
            close_environment_instance_inner_with(fixture.root.path(), &record.env_id, &close)
                .unwrap_err(),
            "process_state_unknown"
        );
        assert_eq!(process_checks.load(std::sync::atomic::Ordering::SeqCst), 1);
    }
}

#[cfg(test)]
mod environment_seed_identity_tests {
    use super::seed_environment_login_from_donor;
    use std::path::Path;
    use traesync_infrastructure::{construct_auth_identity, ConstructAuthInput, UserInfoFull};

    fn seed_donor(instance_dir: &Path, user_id: &str) {
        let info = UserInfoFull {
            user_id: user_id.to_string(),
            screen_name: format!("用户{user_id}"),
            ..Default::default()
        };
        let input = ConstructAuthInput {
            account_id: user_id,
            access_token: "test-token",
            refresh_token: "test-refresh",
            access_token_expires_at_unix_seconds: 4_000_000_000,
            refresh_token_expires_at_unix_seconds: 4_100_000_000,
            user_info: &info,
        };
        construct_auth_identity(&input, instance_dir).unwrap();
    }

    #[test]
    fn first_environment_login_rejects_wrong_donor_before_copying() {
        let root = tempfile::tempdir().unwrap();
        let donor = root.path().join("donor");
        let environment = root.path().join("environment");
        seed_donor(&donor, "account-c");

        assert_eq!(
            seed_environment_login_from_donor(&environment, &donor, "account-b").unwrap_err(),
            "environment_seed_unavailable"
        );
        assert!(!environment.join("User/globalStorage/storage.json").exists());
    }

    #[test]
    fn first_environment_login_copies_matching_donor() {
        let root = tempfile::tempdir().unwrap();
        let donor = root.path().join("donor");
        let environment = root.path().join("environment");
        seed_donor(&donor, "account-b");

        seed_environment_login_from_donor(&environment, &donor, "account-b").unwrap();
        assert!(environment
            .join("User/globalStorage/storage.json")
            .is_file());
    }
}
