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

use traesync_infrastructure::{
    AccountRegistry, AuthSwitchError, MasterHandoverError, backup_master_trio_to,
    handover_master_records, master_database_path, master_db_activity_detected,
};
use traesync_infrastructure::environment_registry::{
    self as environment_registry_module, environment_registry_error_code, master_data_dir,
    secondary_data_dir, EnvironmentRegistry, MASTER_ENV_ID,
};
use traesync_infrastructure::library_account::observed_profile_id;
use traesync_infrastructure::trae_instance::{
    self as trae_instance_module, close_instance, close_master_instance,
    command_line_uses_data_dir, dir_size_recursive, instance_data_dir, list_trae_processes,
    seed_login_state_from_donor, CloseOutcome,
};

use crate::{
    checkin_material_root, ensure_real_preview, launch_instance_common, run_blocking,
    trae_instance_account_id, AppState, RuntimeMode, TraeInstanceLaunchDto,
};
// P12-R3：切号凭据互换双路径与回滚还原随凭据域迁 credential_handover。
use crate::credential_handover::{rollback_master_login_bytes, switch_login_identity_dual_path};

/// 主库环境状态 DTO（P5-0；环境页主库实例卡数据源，前端 P5-2 接线）。
#[derive(Clone, Serialize)]
pub(crate) struct EnvironmentStateDto {
    env_id: String,
    /// 当前登录主库的账号 profile_id；None = 尚未登录任何账号。
    current_profile_id: Option<String>,
    /// 当前账号显示名（备注名优先，缺省服务端昵称）；None = 尚未登录。
    current_account_name: Option<String>,
    /// 主库 data_dir 绝对路径（展示与排障用）。
    data_dir: String,
    running: bool,
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
    run_blocking("trae_launch_join_failed", move || {
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
    );
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
    // 聚焦它而不是再起一个。
    launch_instance_common(
        &instance_dir,
        &window_title,
        None,
        trae_instance_module::command_line_matches_master,
    )
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
            data_dir: String::new(),
            running: false,
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
fn reconcile_master_current_profile(
    registry: &EnvironmentRegistry,
    accounts: &[traesync_infrastructure::account_registry::AccountRecord],
    master_instance_dir: &Path,
    cached_profile_id: Option<&str>,
) -> Option<String> {
    match observed_profile_id(accounts, master_instance_dir) {
        Some(observed) => {
            if cached_profile_id != Some(observed.as_str()) {
                let _ = registry.set_current_profile(MASTER_ENV_ID, &observed);
            }
            Some(observed)
        }
        None => cached_profile_id.map(str::to_string),
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
    // 运行态：主库 = 官方目录，官方快捷方式无参启动的实例同样算运行中。
    let running = list_trae_processes()
        .map(|processes| {
            processes.iter().any(|process| {
                process.command_line.as_deref().is_some_and(|line| {
                    trae_instance_module::command_line_matches_master(line, &instance_dir)
                })
            })
        })
        .unwrap_or(false);
    // 当前账号实测优先（官方登录/登出后注册表缓存过期）：
    // 账号注册表一次加载，供 userId 反查 profile 与显示名解析。
    let accounts = AccountRegistry::new(material_root)
        .load()
        .map_err(|_| "checkin_registry_invalid".to_string())?;
    let current_profile_id = reconcile_master_current_profile(
        &registry,
        &accounts,
        &instance_dir,
        record.current_profile_id.as_deref(),
    );
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
        data_dir: instance_dir.display().to_string(),
        running,
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

    // 进程快照一次：主库用官方目录口径（含无参启动），副环境用带参匹配。
    let processes = list_trae_processes().unwrap_or_default();
    let command_lines: Vec<&str> = processes
        .iter()
        .filter_map(|process| process.command_line.as_deref())
        .collect();
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
    );

    let items = records
        .into_iter()
        .map(|record| {
            let (data_dir, running) = if record.is_master() {
                let running = command_lines.iter().any(|line| {
                    trae_instance_module::command_line_matches_master(line, &master_dir)
                });
                (master_dir.clone(), running)
            } else {
                let env_dir = secondary_data_dir(storage_root, &record.env_id);
                let running = command_lines
                    .iter()
                    .any(|line| command_line_uses_data_dir(line, &env_dir));
                (env_dir, running)
            };
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
                running,
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
    run_blocking("environment_create_join_failed", move || {
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
    run_blocking("environment_rename_join_failed", move || {
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
    run_blocking("environment_delete_join_failed", move || delete_environment_inner(&storage_root, &env_id))
        .await
}

fn delete_environment_inner(storage_root: &Path, env_id: &str) -> Result<bool, String> {
    let registry = EnvironmentRegistry::new(storage_root);
    let record = registry
        .find(env_id)
        .map_err(environment_registry_error_code)?
        .ok_or_else(|| "environment_not_found".to_string())?;
    if record.is_master() {
        return Err("environment_master_immutable".to_string());
    }
    let env_dir = secondary_data_dir(storage_root, &record.env_id);
    // 运行中禁止删除（数据目录被进程占用，强删必产生半删状态）。
    let running = list_trae_processes()
        .map(|processes| {
            processes.iter().any(|process| {
                process
                    .command_line
                    .as_deref()
                    .is_some_and(|line| command_line_uses_data_dir(line, &env_dir))
            })
        })
        .unwrap_or(false);
    if running {
        return Err("environment_delete_running".to_string());
    }
    // 先删档案行（原子写）再删目录：目录删除失败时环境已从列表消失，
    // 残留目录交用户手动清理——错误信息携带完整路径（P11-5），供前端
    // 提示用户定位；路径不属于敏感信息（用户自己机器上的可见目录）。
    registry
        .remove(env_id)
        .map_err(environment_registry_error_code)?;
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
    run_blocking("environment_launch_join_failed", move || {
        // 主库：官方目录口径 + 当前账号窗口标题（P5-0 编排原样复用）。
        if env_id == environment_registry_module::MASTER_ENV_ID {
            return launch_master_library_inner(&material_root, &storage_root);
        }
        let registry = EnvironmentRegistry::new(&storage_root);
        let record = registry
            .find(&env_id)
            .map_err(environment_registry_error_code)?
            .ok_or_else(|| "environment_not_found".to_string())?;
        let env_dir = secondary_data_dir(&storage_root, &record.env_id);
        // 窗口标题用环境名（并行环境按名区分；账号名可能未登录或跨环境重复）。
        launch_instance_common(&env_dir, &record.name, None, command_line_uses_data_dir)
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
/// 关闭原语（trae_instance::close_master_instance / close_instance）。
#[tauri::command]
pub(crate) async fn close_environment_instance(
    env_id: String,
    state: tauri::State<'_, AppState>,
) -> Result<EnvironmentCloseDto, String> {
    ensure_real_preview(&state)?;
    let storage_root = PathBuf::from(&state.storage_root);
    run_blocking("environment_close_join_failed", move || {
        close_environment_instance_inner(&storage_root, &env_id)
    })
    .await
}

fn close_environment_instance_inner(
    storage_root: &Path,
    env_id: &str,
) -> Result<EnvironmentCloseDto, String> {
    // 主库：官方目录口径（覆盖官方无参启动的主进程形态）；副环境：
    // 带参匹配口径（--user-data-dir 指向环境目录）。档案不存在先拒绝，
    // 不做任何进程操作。
    let outcome = if env_id == environment_registry_module::MASTER_ENV_ID {
        let instance_dir =
            master_data_dir().map_err(|_| "environment_registry_invalid".to_string())?;
        close_master_instance(&instance_dir)
    } else {
        let record = EnvironmentRegistry::new(storage_root)
            .find(env_id)
            .map_err(environment_registry_error_code)?
            .ok_or_else(|| "environment_not_found".to_string())?;
        close_instance(&secondary_data_dir(storage_root, &record.env_id))
    };
    match outcome {
        Ok(CloseOutcome::Closed) => Ok(EnvironmentCloseDto { outcome: "closed" }),
        Ok(CloseOutcome::ForceClosed) => Ok(EnvironmentCloseDto {
            outcome: "force_closed",
        }),
        Ok(CloseOutcome::NotRunning) => Ok(EnvironmentCloseDto { outcome: "not_running" }),
        // 强制结束后进程仍存在（如权限不足）：稳定错误码，前端提示手动关闭。
        Err(_) => Err("environment_close_failed".to_string()),
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
    let raw_key = state.source_raw_key.clone();
    run_blocking("environment_login_join_failed", move || {
        login_environment_inner(
            &material_root,
            &storage_root,
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
    raw_key: &str,
    env_id: &str,
    profile_id: &str,
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

    let registry = EnvironmentRegistry::new(storage_root);
    let env_record = registry
        .find(env_id)
        .map_err(environment_registry_error_code)?
        .ok_or_else(|| "environment_not_found".to_string())?;
    let env_dir = secondary_data_dir(storage_root, &env_record.env_id);
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
    close_master_instance(&env_dir).map_err(|_| "environment_login_close_failed".to_string())?;

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
                let backup_dir =
                    resolve_library_backup_dir(storage_root, &db_path, env_id);
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
            let identity =
                switch_login_identity_dual_path(material_root, record, &donor_dir, &env_dir);
            let _identity = match identity {
                Ok(identity) => identity,
                Err(AuthSwitchError::DonorAuthUnavailable) => {
                    return Err(rollback("switch_donor_login_missing"))
                }
                // 环境内互换极少出现的其余形态（目标 blob 损坏等）：统一失败码。
                Err(_) => return Err(rollback("environment_login_failed")),
            };
            // 记录交接（单一归属泛化：库内记录随登录归一到目标账号）。
            if db_path.is_file() {
                match handover_master_records(&db_path, raw_key, &target_user_id) {
                    Ok(handover) => {
                        transferred_projects = handover.transferred_projects;
                        switched_sessions = handover.switched_sessions.len();
                        removed_mirror_rows = handover.removed_mirror_rows;
                    }
                    Err(error) => {
                        let code = match error {
                            MasterHandoverError::DbUnavailable => "environment_login_db_missing",
                            MasterHandoverError::DbOpenFailed => "environment_login_db_open_failed",
                            MasterHandoverError::TargetConflict(_) => "environment_login_conflict",
                            MasterHandoverError::IntegrityFailed => {
                                "environment_login_integrity_failed"
                            }
                            MasterHandoverError::Io => "environment_login_failed",
                        };
                        return Err(rollback(code));
                    }
                }
            }
        }
        trae_instance_module::InstanceLoginState::Uninitialized
        | trae_instance_module::InstanceLoginState::LoggedOut => {
            // 空环境首登（或启动过未登录）：从账号登录存档播种三件套，
            // 移植即登录——无互换对象，无记录可交接（空库跳过）。
            seed_login_state_from_donor(&env_dir, &donor_dir)
                .map_err(|_| "environment_seed_unavailable".to_string())?;
            seeded = true;
        }
    }

    // 档案写回当前登录账号（各环境各自记忆，多对多）。
    registry
        .set_current_profile(env_id, profile_id)
        .map_err(environment_registry_error_code)?;
    Ok(EnvironmentLoginReceiptDto {
        env_id: env_id.to_string(),
        profile_id: profile_id.to_string(),
        seeded,
        transferred_projects,
        switched_sessions,
        removed_mirror_rows,
    })
}

// ============================================================================
// 主库当前账号实测纠正（2026-09-03 卡死案例修复）：用户在 TRAE 官方
// 登录/登出后环境注册表 current_profile_id 过期，读取路径须实测优先。
// ============================================================================
#[cfg(test)]
mod master_current_account_reconcile_tests {
    use super::*;
    use traesync_infrastructure::account_registry::AccountRecord;
    use traesync_infrastructure::{construct_auth_identity, ConstructAuthInput, UserInfoFull};
    use crate::resolve_actual_master_account;

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
        assert_eq!(resolved.as_deref(), Some("checkin-b"));
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
        assert_eq!(resolved.as_deref(), Some("checkin-a"));
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
        assert_eq!(resolved.as_deref(), Some("checkin-a"));
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
        assert_eq!(resolved.as_deref(), Some("checkin-a"));
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

        fn storage_root(&self) -> PathBuf {
            self.root.path().to_path_buf()
        }

        fn create_env(&self, name: &str) -> traesync_infrastructure::environment_registry::EnvironmentRecord {
            EnvironmentRegistry::new(self.root.path()).create(name).unwrap()
        }
    }

    #[test]
    fn delete_environment_success_removes_directory() {
        let fixture = EnvFixture::new();
        let record = fixture.create_env("测试环境");
        let env_dir = secondary_data_dir(fixture.root.path(), &record.env_id);
        std::fs::create_dir_all(&env_dir).unwrap();

        delete_environment_inner(fixture.root.path(), &record.env_id).unwrap();

        // 档案行与目录一并移除。
        assert!(!env_dir.exists());
        assert!(EnvironmentRegistry::new(fixture.root.path())
            .find(&record.env_id)
            .unwrap()
            .is_none());
    }

    #[test]
    fn delete_environment_rejects_master_and_missing() {
        let fixture = EnvFixture::new();
        // 主库先建档：命中「主库不可删除」而非「档案不存在」。
        EnvironmentRegistry::new(fixture.root.path())
            .ensure_master()
            .unwrap();
        assert_eq!(
            delete_environment_inner(fixture.root.path(), MASTER_ENV_ID).unwrap_err(),
            "environment_master_immutable"
        );
        assert_eq!(
            delete_environment_inner(fixture.root.path(), "env-none").unwrap_err(),
            "environment_not_found"
        );
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

        let error = delete_environment_inner(fixture.root.path(), &record.env_id).unwrap_err();

        // P11-5：稳定错误码 + 完整残留路径（冒号后为路径），供前端提示
        // 用户手动清理定位。
        assert_eq!(
            error,
            format!("environment_delete_failed_residue:{}", env_dir.display())
        );
        // 残留物仍在（不静默删除，铁律）。
        assert!(env_dir.exists());
    }

    /// close 原语仅 Windows（taskkill）；进程列表查询只读安全。
    #[cfg(windows)]
    #[test]
    fn close_environment_missing_reports_not_found_without_process_actions() {
        let fixture = EnvFixture::new();
        // 档案不存在：先拒绝，不做任何进程操作。
        assert_eq!(
            close_environment_instance_inner(fixture.root.path(), "env-none").unwrap_err(),
            "environment_not_found"
        );
    }

    /// close 原语仅 Windows（taskkill）；tempdir data_dir 不会有真实
    /// TRAE 进程使用，走 NotRunning 幂等分支（无 taskkill、无等待）。
    #[cfg(windows)]
    #[test]
    fn close_environment_not_running_is_idempotent() {
        let fixture = EnvFixture::new();
        let record = fixture.create_env("测试环境");
        let receipt = close_environment_instance_inner(fixture.root.path(), &record.env_id).unwrap();
        assert_eq!(receipt.outcome, "not_running");
    }
}
