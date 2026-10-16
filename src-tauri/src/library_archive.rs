//! 库历史 / 归档 / 备份域（P12-R3 自 lib.rs 纯移动）。
//!
//! 覆盖：库 id → 环境归属与运行上下文解析（resolve_library_env_id /
//! resolve_library_context，ADR-0025 库抽象）、
//! 历史页主库视图与消息预览（P5-3）、库聚合统计（P5-4）、库归档草稿
//! （P11-5）、主库体检与一键收编（P5-5）、空项目清理（P5-5）、备份链与
//! 备份管理命令族（P5-4/P5-9/P11-4：链枚举/手动备份/位置/保留/迁移）、
//! 主库两层校验（P5-7）、会话归档/恢复/真实删除/项目合并（P5-8a/8c）、
//! 接力台账读取（P5-3），以及全部内嵌测试。
//! 环境实例生命周期命令在 environment 模块；主库切号五步事务留守组合根，
//! 经 `crate::` 引用组合根原语、本模块备份工具回供
//! `crate::library_archive::{resolve_master_backup_dir, prune_backups_if_enabled,
//! resolve_library_backup_dir, prune_backups_if_enabled_in}`。命令体、
//! 错误码与 wire 形态零变化。

use std::future::Future;
use std::path::{Path, PathBuf};

use serde::Serialize;
use tauri::Emitter;

use traesync_domain::{LibraryContext, TRAE_CN_LIBRARY_ID};
use traesync_infrastructure::account_session_content::{
    read_master_session_messages_page, MAX_MESSAGES_PER_SESSION,
};
use traesync_infrastructure::account_session_index::InstanceFingerprint;
use traesync_infrastructure::archive_draft_store::{
    ArchiveDraftRecord, ArchiveDraftStore, ArchiveDraftStoreError,
};
use traesync_infrastructure::backup_retention::{
    load_backup_dir_override, migrate_master_backups as migrate_master_backups_inner,
    prune_master_backups, BackupMigrationOutcome, BackupRetentionError, BackupRetentionSettings,
    BackupRetentionStore, MAX_BACKUP_DIR_BYTES,
};
use traesync_infrastructure::environment_registry::{
    environment_registry_error_code, master_data_dir, master_data_dir_for, secondary_data_dir,
    EnvironmentRegistry, MASTER_ENV_ID,
};
use traesync_infrastructure::library_account::{self, LibraryAccountResolution};
use traesync_infrastructure::master_history::{
    read_master_history, stat_master_fingerprint, MasterHistoryStatus,
};
use traesync_infrastructure::trae_instance::{self as trae_instance_module, close_instance};
use traesync_infrastructure::{
    apply_master_archive_changes as apply_master_archive_changes_inner, backup_master_trio_stamped,
    backup_master_trio_to, delete_master_sessions as delete_master_sessions_inner,
    delete_master_sessions_with_backup_dir, handover_master_records, list_master_backups_in,
    master_backup_db_name, master_database_path, master_db_activity_detected,
    merge_master_projects_with_backup_dir, merge_trae_cn_project_records_with_progress,
    preview_trae_cn_project_merge as preview_trae_cn_project_merge_infra,
    read_trae_cn_foreign_projects, AccountRegistry, MasterArchiveError, MasterDeleteOutcome,
    MasterHandover, MasterHandoverError, MasterMergeOutcome, OperationLease, RelayLedger,
    RelayLedgerEntry, TraeProduct,
};

use crate::{
    checkin_material_root, ensure_real_preview, error_mapping::map_master_archive_error,
    run_blocking, AppState, RuntimeMode,
};
// P13-5：实例工具族随迁 instance_tools。
use crate::instance_tools::{launch_instance_common, TraeInstanceLaunchDto};

/// 锁住副环境生命周期并在保护下解析库；调用方负责接着取得目录租约。
fn prepare_library_write_context(
    recovery_root: &Path,
    storage_root: &Path,
    library_id: Option<&str>,
) -> Result<(LibraryContext, Option<OperationLease>), String> {
    let env_id = resolve_library_env_id(library_id, storage_root)?;
    let lifecycle_lease = if env_id != MASTER_ENV_ID && env_id != TRAE_CN_LIBRARY_ID {
        Some(
            OperationLease::acquire_shared_for_path(
                recovery_root,
                &storage_root.join("environments"),
            )
            .map_err(|error| format!("operation_lease_unavailable:{error}"))?,
        )
    } else {
        None
    };
    let context = resolve_library_context(library_id, storage_root)?;
    Ok((context, lifecycle_lease))
}

/// 按库数据目录取得租约；供已有写入口与归档编排共用。
fn acquire_library_data_lease(
    recovery_root: &Path,
    context: &LibraryContext,
) -> Result<OperationLease, String> {
    OperationLease::acquire_shared_for_path(recovery_root, &context.dir)
        .map_err(|error| format!("operation_lease_unavailable:{error}"))
}

/// 锁住副环境生命周期并在保护下解析库，再返回单库长租约。
fn acquire_library_write_lease(
    recovery_root: &Path,
    storage_root: &Path,
    library_id: Option<&str>,
) -> Result<(LibraryContext, OperationLease), String> {
    let (context, lifecycle_lease) =
        prepare_library_write_context(recovery_root, storage_root, library_id)?;
    let lease = acquire_library_data_lease(recovery_root, &context)?;
    drop(lifecycle_lease);
    Ok((context, lease))
}

/// P3-2/P5-3 共用：消息正文 DTO（只发送必要字段；不暴露原始 raw body）。
#[derive(Debug, Clone, Serialize)]
struct SessionMessageContentDto {
    kind: &'static str,
    text: String,
    step_count: u32,
    thoughts: Vec<String>,
}

/// P3-2/P5-3 共用：单条消息 DTO。
#[derive(Debug, Clone, Serialize)]
struct SessionMessageDto {
    message_id: String,
    role: String,
    message_type: String,
    created_at_unix_seconds: i64,
    content: SessionMessageContentDto,
}

// ===== P5-3 历史页主库视图（Q4/Q5：项目两栏 + 接力轨迹 + 消息预览）=====

/// P5-3 主库历史读取结果（两栏数据源 + 轮询指纹）。
///
/// status：ready=读取成功；unchanged=指纹与 previous 一致（前端维持现有
/// 列表，轮询预检语义）；no_master_data=主库从未启动过；no_current_account=
/// 主库尚未登记登录账号；read_failed=打开或读取失败。
#[derive(Clone, Serialize)]
pub(crate) struct MasterHistoryDto {
    status: &'static str,
    /// 主库当前登录账号的 TRAE user_id（接力轨迹账号对齐用）；未登记为 None。
    current_user_id: Option<String>,
    projects: Vec<traesync_infrastructure::master_history::MasterProjectEntry>,
    sessions: Vec<traesync_infrastructure::master_history::MasterSessionEntry>,
    /// 读取时刻的三件套 stat 指纹（前端保存为下一轮 previous）。
    fingerprint: InstanceFingerprint,
}

/// P11-6：Trae CN 官方数据根（只读库目录）。
///
/// 与主库同源规则：APPDATA 下的固定官方目录（`%APPDATA%\Trae CN`，P10
/// 研究确认的只读数据根），不接受调用者传入任意路径。APPDATA 缺失时报
/// 稳定错误码（fail-closed，不猜路径）。
fn trae_cn_library_dir() -> Result<PathBuf, String> {
    master_data_dir_for(TraeProduct::TraeCn)
        .map_err(|_| "trae_cn_data_root_unavailable".to_string())
}

/// ADR-0025：库 id → 环境归属解析。
///
/// `None` / `"master"` → 主库环境；`"trae_cn"` → Trae CN 产品只读库
///（P11-6，产品库不入环境注册表）；其余 id 必须在环境注册表在册
///（副环境 env_id 即库 id），未登记一律 `library_not_found`。
fn resolve_library_env_id(library_id: Option<&str>, storage_root: &Path) -> Result<String, String> {
    match library_id {
        None | Some(MASTER_ENV_ID) => Ok(MASTER_ENV_ID.to_string()),
        // P11-6：Trae CN 产品库是固定注册项，不查环境注册表。
        Some(TRAE_CN_LIBRARY_ID) => Ok(TRAE_CN_LIBRARY_ID.to_string()),
        Some(env_id) => EnvironmentRegistry::new(storage_root)
            .find(env_id)
            .map_err(environment_registry_error_code)?
            .ok_or_else(|| "library_not_found".to_string())
            .map(|record| record.env_id),
    }
}

/// P12-R1b：LibraryContext 的实例操作扩展（类型本体已下沉 domain）。
/// 依赖实例进程接口（infrastructure）的三个方法以私有 trait 形式留在
/// 库域模块（P12-R3 前在组合根）——Rust 固有 impl 只能在定义 crate 内，
/// trait 形式让库域 `ctx.xxx()` 调用面零改动；is_master / ensure_writable
/// 纯规则随类型迁入 domain。
trait LibraryContextInstanceOps {
    /// 关闭 Work CN 实例：主库和副环境均按产品与精确数据目录匹配。
    fn close_instance(
        &self,
    ) -> Result<trae_instance_module::CloseOutcome, trae_instance_module::TraeInstanceCloseError>;

    /// 重启实例（登录身份未变场景：收编/清理/归档应用收尾）。
    /// 窗口标题：主库 = 当前账号名（P5-5 既有口径）；副环境 = 环境名
    ///（与环境页启动入口一致，并行环境按名区分）。
    fn relaunch(&self, window_title: &str) -> Result<TraeInstanceLaunchDto, String>;
}

impl LibraryContextInstanceOps for LibraryContext {
    fn close_instance(
        &self,
    ) -> Result<trae_instance_module::CloseOutcome, trae_instance_module::TraeInstanceCloseError>
    {
        // P15-6：库上下文实例（主库/副环境）由融合版客户端承载。
        close_instance(TraeProduct::TraeCn, &self.dir)
    }

    fn relaunch(&self, window_title: &str) -> Result<TraeInstanceLaunchDto, String> {
        launch_instance_common(TraeProduct::TraeCn, &self.dir, window_title, None)
    }
}

/// ADR-0025：库 id → 运行上下文（目录 + 环境归属 + 显示名 + 只读标记）。
///
/// P11-6：`trae_cn` 分支构建产品只读库上下文——目录为 Trae CN 官方数据根，
/// 显示名取产品注册表 `display_name`，`read_only = true`（写库命令拒绝）。
fn resolve_library_context(
    library_id: Option<&str>,
    storage_root: &Path,
) -> Result<LibraryContext, String> {
    let env_id = resolve_library_env_id(library_id, storage_root)?;
    // P11-6：Trae CN 产品库独立分支（不入环境注册表；目录与名称来自产品注册表）。
    if env_id == TRAE_CN_LIBRARY_ID {
        return Ok(LibraryContext {
            env_id,
            env_name: TraeProduct::TraeCn.display_name().to_string(),
            dir: trae_cn_library_dir()?,
            read_only: true,
        });
    }
    let dir = if env_id == MASTER_ENV_ID {
        master_data_dir().map_err(|_| "environment_registry_invalid".to_string())?
    } else {
        secondary_data_dir(storage_root, &env_id)
    };
    let env_name = if env_id == MASTER_ENV_ID {
        "主库".to_string()
    } else {
        EnvironmentRegistry::new(storage_root)
            .find(&env_id)
            .map_err(environment_registry_error_code)?
            .ok_or_else(|| "library_not_found".to_string())?
            .name
    };
    Ok(LibraryContext {
        env_id,
        env_name,
        dir,
        read_only: false,
    })
}

/// 操作快照校验产品与库确实对应，避免未知库或另一产品的库被静默接受。
pub(crate) fn validate_product_library_id(
    product_id: &str,
    library_id: &str,
    storage_root: &Path,
) -> Result<String, String> {
    if !matches!(product_id, "work_cn" | "trae_cn") {
        return Err("product_not_found".to_string());
    }
    let context = resolve_library_context(Some(library_id), storage_root)?;
    let is_trae_cn_library = context.env_id == TRAE_CN_LIBRARY_ID;
    if (product_id == "trae_cn") != is_trae_cn_library {
        return Err("product_library_mismatch".to_string());
    }
    Ok(context.env_id)
}

/// 按已选产品库目录观察当前进程状态，供操作快照与写前门禁共用。
pub(crate) fn observe_library_process_state(
    product_id: &str,
    library_id: &str,
    storage_root: &Path,
) -> Result<trae_instance_module::ProductProcessState, String> {
    let product = match product_id {
        "work_cn" => TraeProduct::WorkCn,
        "trae_cn" => TraeProduct::TraeCn,
        _ => return Err("product_not_found".to_string()),
    };
    let context = resolve_library_context(Some(library_id), storage_root)?;
    Ok(trae_instance_module::current_product_process_state(
        product,
        &context.dir,
    ))
}

/// 从已解析的库归属取得产品 id，供旧库命令沿用其隐式库参数。
pub(crate) fn product_id_for_library(
    library_id: Option<&str>,
    storage_root: &Path,
) -> Result<&'static str, String> {
    let env_id = resolve_library_env_id(library_id, storage_root)?;
    Ok(if env_id == TRAE_CN_LIBRARY_ID {
        "trae_cn"
    } else {
        "work_cn"
    })
}

/// 库作用域强制账号解析（归属写操作入口）：无法确定当前账号即拒绝。
/// P12-R1b：解析本体下沉 `library_account::resolve_library_account`，
/// 此处仅保留「无账号 → 稳定错误码」的调用点策略。
fn require_library_account(
    material_root: &Path,
    storage_root: &Path,
    ctx: &LibraryContext,
) -> Result<traesync_infrastructure::account_registry::AccountRecord, String> {
    library_account::resolve_library_account(material_root, storage_root, ctx)?
        .and_then(LibraryAccountResolution::into_account)
        .ok_or_else(|| "master_current_account_unavailable".to_string())
}

/// P5-3：读取主库历史（当前账号正常会话 + 全局归档会话）。
///
/// 当前账号 = 主库登录态 blob 实测 user_id → 账号注册表 account_id；正常会话按
/// `project.user_id` 过滤，归档会话不受账号过滤影响。`previous` 为前端保存的
/// 上一轮指纹，另带当前账号身份，避免切号后文件指纹未变时错误返回 unchanged。
/// `library_id` 为库实例引用（ADR-0025，缺省主库）。P11-6：`trae_cn` 库
/// 放行（同一套 SQL；当前账号取 Trae CN 产品账号池指针，未登录引导先登录）。
#[tauri::command]
pub(crate) async fn get_master_history(
    previous: Option<InstanceFingerprint>,
    previous_current_user_id: Option<String>,
    library_id: Option<String>,
    state: tauri::State<'_, AppState>,
) -> Result<MasterHistoryDto, String> {
    ensure_real_preview(&state)?;
    if state.source_raw_key.is_empty() {
        return Err("source_key_unavailable".to_string());
    }
    let material_root = checkin_material_root(&state)?;
    let storage_root = PathBuf::from(&state.storage_root);
    let raw_key = state.source_raw_key.clone();
    run_blocking("master_history_join_failed", move || {
        // ADR-0025：库目录统一走运行上下文解析（指纹与读取同源）。
        let ctx = resolve_library_context(library_id.as_deref(), &storage_root)?;
        let current_account =
            library_account::resolve_library_account(&material_root, &storage_root, &ctx)?
                .and_then(LibraryAccountResolution::into_account);
        let current_user_id = current_account
            .as_ref()
            .map(|account| account.account_id.as_str());
        let fingerprint = stat_master_fingerprint(&ctx.dir);
        if previous == Some(fingerprint.clone())
            && previous_current_user_id.as_deref() == current_user_id
        {
            return Ok(MasterHistoryDto {
                status: "unchanged",
                current_user_id: None,
                projects: Vec::new(),
                sessions: Vec::new(),
                fingerprint,
            });
        }
        let Some(account) = current_account else {
            // 主库尚未登记登录账号：历史页引导到环境页先启动并登录。
            return Ok(MasterHistoryDto {
                status: "no_current_account",
                current_user_id: None,
                projects: Vec::new(),
                sessions: Vec::new(),
                fingerprint,
            });
        };
        let current_user_id = account.account_id.clone();
        let (status, projects, sessions) =
            match read_master_history(&ctx.dir, &raw_key, &current_user_id) {
                MasterHistoryStatus::Ready { projects, sessions } => ("ready", projects, sessions),
                MasterHistoryStatus::NoMasterData => ("no_master_data", Vec::new(), Vec::new()),
                MasterHistoryStatus::ReadFailed => ("read_failed", Vec::new(), Vec::new()),
            };
        Ok(MasterHistoryDto {
            status,
            current_user_id: Some(current_user_id),
            projects,
            sessions,
            fingerprint,
        })
    })
    .await
}

/// P5-3 主库单会话消息读取结果（状态语义与 P3-2 一致）。
#[derive(Clone, Serialize)]
pub(crate) struct MasterSessionMessagesDto {
    session_id: String,
    /// ready=读取成功；no_master_data=主库从未启动过；read_failed=打开或读取失败。
    status: &'static str,
    messages: Vec<SessionMessageDto>,
    /// 当前页之后是否还有更早消息；前端到顶部时按页继续读取。
    has_more: bool,
}

/// P5-3：读取主库单个会话的消息流（历史页预览弹层数据源）。
///
/// 与账号实例读取共用解析路径；通过隔离三件套副本只读打开，
/// 主库实例运行中可随时读取。`library_id` 为库实例引用
/// （ADR-0025，缺省主库）。P11-6：`trae_cn` 库放行（同一套 SQL，
/// 账号解析走产品账号池指针）。
#[tauri::command]
pub(crate) async fn get_master_session_messages(
    session_id: String,
    library_id: Option<String>,
    offset: Option<u32>,
    state: tauri::State<'_, AppState>,
) -> Result<MasterSessionMessagesDto, String> {
    ensure_real_preview(&state)?;
    if state.source_raw_key.is_empty() {
        return Err("source_key_unavailable".to_string());
    }
    let material_root = checkin_material_root(&state)?;
    let storage_root = PathBuf::from(&state.storage_root);
    let raw_key = state.source_raw_key.clone();
    let offset = offset.unwrap_or(0) as usize;
    run_blocking("master_session_messages_join_failed", move || {
        use traesync_infrastructure::account_session_content::SessionMessagesStatus;
        let ctx = resolve_library_context(library_id.as_deref(), &storage_root)?;
        let current_account =
            library_account::resolve_library_account(&material_root, &storage_root, &ctx)?
                .and_then(LibraryAccountResolution::into_account)
                .ok_or_else(|| "master_current_account_unavailable".to_string())?;
        let (status, messages, has_more) = match read_master_session_messages_page(
            &ctx.dir,
            &session_id,
            &raw_key,
            offset,
            &current_account.account_id,
        ) {
            SessionMessagesStatus::Ready(entries) => {
                let has_more = entries.len() == MAX_MESSAGES_PER_SESSION;
                ("ready", map_session_message_entries(entries), has_more)
            }
            SessionMessagesStatus::NoInstanceData => ("no_master_data", Vec::new(), false),
            SessionMessagesStatus::ReadFailed => ("read_failed", Vec::new(), false),
        };
        Ok(MasterSessionMessagesDto {
            session_id,
            status,
            messages,
            has_more,
        })
    })
    .await
}

// ===== P5-4 主库轻量统计 + 备份链（总览/环境卡/设置页数据源）=====

/// P5-4 主库聚合统计（get_master_library_stats 返回）。
///
/// status：ready=读取成功；no_master_data=主库从未启动过；
/// no_current_account=主库尚未登记登录账号；read_failed=打开或读取失败。
#[derive(Clone, Serialize)]
pub(crate) struct MasterLibraryStatsDto {
    status: &'static str,
    /// 主库当前登录账号的 TRAE user_id；未登记为 None。
    current_user_id: Option<String>,
    project_count: u64,
    session_count: u64,
    /// 当前账号会话的消息总数（P5-8a-2 详情页头部，与历史页同口径）。
    message_count: u64,
    /// 主库内出现过的全部账号数（含非当前账号历史归属）。
    participating_account_count: u64,
    /// 当前账号会话最近活跃时间（秒）；无会话为 None。
    last_active_unix_seconds: Option<i64>,
    /// 主库三件套合计字节（P5-8a-2 详情页头部；主库从未启动为 0）。
    size_bytes: u64,
}

/// P5-4：库聚合统计（总览页统计卡 + 环境卡会话胶囊数据源）。
///
/// 轻量只读：4 条聚合 SQL，库实例运行中可随时读取。当前账号解析与
/// get_master_history 同口径（登录态实测 → 账号注册表 account_id）。
/// P11-5⑤：`library_id` 为库实例引用（缺省主库；副环境统计自身目录，
/// 主库统计口径不变）。P11-6：`trae_cn` 库放行（同一套 SQL）——若其
/// schema 与 Work 有字段差异，聚合按现实降级（能算的算，不能算的归零
/// 并按 read_failed 返回，不 panic）；账号解析走产品账号池指针。
#[tauri::command]
pub(crate) async fn get_master_library_stats(
    library_id: Option<String>,
    state: tauri::State<'_, AppState>,
) -> Result<MasterLibraryStatsDto, String> {
    ensure_real_preview(&state)?;
    if state.source_raw_key.is_empty() {
        return Err("source_key_unavailable".to_string());
    }
    let material_root = checkin_material_root(&state)?;
    let storage_root = PathBuf::from(&state.storage_root);
    let raw_key = state.source_raw_key.clone();
    run_blocking("master_stats_join_failed", move || {
        use traesync_infrastructure::master_stats::{
            master_trio_size_bytes, read_master_stats, MasterStatsStatus,
        };
        let ctx = resolve_library_context(library_id.as_deref(), &storage_root)?;
        // 体积与库内容统计独立：库打开失败时体积仍可展示（详情页降级不空白）。
        let size_bytes = master_trio_size_bytes(&ctx.dir);
        let current_account =
            library_account::resolve_library_account(&material_root, &storage_root, &ctx)?
                .and_then(LibraryAccountResolution::into_account);
        let Some(account) = current_account else {
            // 未登记账号：统计归零 + 状态引导（总览隐藏区块，环境页提示登录）。
            return Ok(MasterLibraryStatsDto {
                status: "no_current_account",
                current_user_id: None,
                project_count: 0,
                session_count: 0,
                message_count: 0,
                participating_account_count: 0,
                last_active_unix_seconds: None,
                size_bytes,
            });
        };
        let current_user_id = account.account_id.clone();
        let dto = match read_master_stats(&ctx.dir, &raw_key, &current_user_id) {
            MasterStatsStatus::Ready(stats) => MasterLibraryStatsDto {
                status: "ready",
                current_user_id: Some(current_user_id),
                project_count: stats.project_count,
                session_count: stats.session_count,
                message_count: stats.message_count,
                participating_account_count: stats.participating_account_count,
                last_active_unix_seconds: stats.last_active_unix_seconds,
                size_bytes,
            },
            MasterStatsStatus::NoMasterData => MasterLibraryStatsDto {
                status: "no_master_data",
                current_user_id: Some(current_user_id),
                project_count: 0,
                session_count: 0,
                message_count: 0,
                participating_account_count: 0,
                last_active_unix_seconds: None,
                size_bytes,
            },
            MasterStatsStatus::ReadFailed => MasterLibraryStatsDto {
                status: "read_failed",
                current_user_id: Some(current_user_id),
                project_count: 0,
                session_count: 0,
                message_count: 0,
                participating_account_count: 0,
                last_active_unix_seconds: None,
                size_bytes,
            },
        };
        Ok(dto)
    })
    .await
}

// ===== P11-5 库归档草稿（localStorage 迁入 storage_root，命令层编排）=====

/// P11-5 库归档草稿 DTO（get_archive_draft 返回；字段与前端草稿状态同构）。
#[derive(Clone, Serialize)]
pub(crate) struct ArchiveDraftDto {
    archive_session_ids: Vec<String>,
    restore_session_ids: Vec<String>,
}

impl ArchiveDraftDto {
    fn empty() -> Self {
        Self {
            archive_session_ids: Vec::new(),
            restore_session_ids: Vec::new(),
        }
    }
}

/// P11-5 读取库归档草稿（storage_root/archive-drafts.json，每库一键）。
/// 存储文件损坏 fail-closed：返回空草稿且不删除文件（铁律），旧草稿由
/// 前端 localStorage 迁移路径兜底。
#[tauri::command]
pub(crate) async fn get_archive_draft(
    library_id: String,
    state: tauri::State<'_, AppState>,
) -> Result<ArchiveDraftDto, String> {
    if state.runtime_mode != RuntimeMode::RealReadPreview {
        // fixture 模式无真实存储：返回空草稿（迁移与写入路径在真实模式生效）。
        return Ok(ArchiveDraftDto::empty());
    }
    let storage_root = PathBuf::from(&state.storage_root);
    run_blocking("archive_draft_join_failed", move || {
        get_archive_draft_inner(&storage_root, &library_id)
    })
    .await
}

fn get_archive_draft_inner(
    storage_root: &Path,
    library_id: &str,
) -> Result<ArchiveDraftDto, String> {
    let drafts = ArchiveDraftStore::new(storage_root)
        .load()
        // 损坏 fail-closed：按空草稿处理，不删除文件（铁律）。
        .unwrap_or_default();
    Ok(drafts
        .get(library_id)
        .map(|record| ArchiveDraftDto {
            archive_session_ids: record.archive_session_ids.clone(),
            restore_session_ids: record.restore_session_ids.clone(),
        })
        .unwrap_or_else(ArchiveDraftDto::empty))
}

/// P11-5 保存库归档草稿（空草稿即清除该库键）。存储层拒绝时返回稳定
/// 错误码，前端回退 localStorage 兜底，不静默丢草稿。
#[tauri::command]
pub(crate) async fn set_archive_draft(
    library_id: String,
    archive_session_ids: Vec<String>,
    restore_session_ids: Vec<String>,
    state: tauri::State<'_, AppState>,
) -> Result<bool, String> {
    ensure_real_preview(&state)?;
    let storage_root = PathBuf::from(&state.storage_root);
    run_blocking("archive_draft_join_failed", move || {
        set_archive_draft_inner(
            &storage_root,
            &library_id,
            &archive_session_ids,
            &restore_session_ids,
        )
    })
    .await
}

fn set_archive_draft_inner(
    storage_root: &Path,
    library_id: &str,
    archive_session_ids: &[String],
    restore_session_ids: &[String],
) -> Result<bool, String> {
    let record = ArchiveDraftRecord {
        archive_session_ids: archive_session_ids.to_vec(),
        restore_session_ids: restore_session_ids.to_vec(),
    };
    ArchiveDraftStore::new(storage_root)
        .set(library_id, &record)
        .map(|_| true)
        .map_err(|error| match error {
            ArchiveDraftStoreError::Invalid => "archive_draft_store_invalid".to_string(),
            ArchiveDraftStoreError::Io => "archive_draft_store_unavailable".to_string(),
        })
}

// ===== P5-5 主库体检 + 收编（编排层）=====

/// 体检报告中的账号分布行（get_master_checkup 返回）。
///
/// 界面表达纪律：user_id 是技术标识，前端只用 account_name/registered
/// 组织主信息；user_id 收进悬浮提示。
#[derive(Clone, Serialize)]
struct MasterCheckupAccountDto {
    /// TRAE user_id（悬浮提示用，不作主信息）。
    user_id: String,
    /// 账号展示名（注册表 display_name/screen_name）；未注册为 None。
    account_name: Option<String>,
    /// 是否已在 App 账号页登记。
    registered: bool,
    /// 是否当前账号（收编目标）。
    current: bool,
    /// 含至少一个正常会话的项目数。
    project_count: u64,
    /// 未删除且未归档的正常会话数。
    session_count: u64,
    /// 空项目数（不含已删除项目；只报告，不收编）。
    empty_project_count: u64,
}

/// P5-5 主库体检报告（get_master_checkup 返回，只读）。
///
/// status：ready / no_current_account / no_master_data / read_failed；
/// P11-6 追加 `not_applicable` = Trae CN 只读库不适用体检（前端按
/// absent 渲染「该产品无此能力」）。
#[derive(Clone, Serialize)]
pub(crate) struct MasterCheckupDto {
    /// ready / no_current_account / no_master_data / read_failed / not_applicable。
    status: &'static str,
    /// 当前账号账号名（回执与引导文案用；无当前账号为 None）。
    current_account_name: Option<String>,
    /// 可交接范围内的账号分布（按正常会话数降序）。
    accounts: Vec<MasterCheckupAccountDto>,
    /// 无归属（user_id IS NULL）项目行数（只报告，收编不动）。
    orphan_project_count: u64,
    /// 挂在无归属项目行上的会话数。
    orphan_session_count: u64,
}

/// P11-6：Trae CN 只读库的体检「不适用」形态（字段全空/零，无库读取）。
fn master_checkup_not_applicable() -> MasterCheckupDto {
    MasterCheckupDto {
        status: "not_applicable",
        current_account_name: None,
        accounts: Vec::new(),
        orphan_project_count: 0,
        orphan_session_count: 0,
    }
}

/// P5-5 环境页体检：库记录分布 + 滞留账号 + 孤儿行（只读聚合）。
///
/// 进页自动执行（与统计格同读同刷）；只读打开，库实例运行中可随时读取。
/// P11-5⑤：`library_id` 为库实例引用（缺省主库；副环境对自身数据目录
/// 体检，未注册账号/空镜像语义同义——user_id 维度一致）。
#[tauri::command]
pub(crate) async fn get_master_checkup(
    library_id: Option<String>,
    state: tauri::State<'_, AppState>,
) -> Result<MasterCheckupDto, String> {
    ensure_real_preview(&state)?;
    if state.source_raw_key.is_empty() {
        return Err("source_key_unavailable".to_string());
    }
    let material_root = checkin_material_root(&state)?;
    let storage_root = PathBuf::from(&state.storage_root);
    let raw_key = state.source_raw_key.clone();
    run_blocking("master_checkup_join_failed", move || {
        use traesync_infrastructure::master_checkup::{
            read_master_checkup_cached, MasterCheckupStatus,
        };
        let ctx = resolve_library_context(library_id.as_deref(), &storage_root)?;
        // P11-6（ADR-0032 决策 5）：体检/收编是 Work CN 的账号归属治理
        // 概念，Trae CN 只读库不适用——返回「不适用」形态（前端按 absent
        // 渲染「该产品无此能力」），不做任何库读取。
        if ctx.read_only {
            return Ok(master_checkup_not_applicable());
        }
        let records = AccountRegistry::new(&material_root)
            .load()
            .map_err(|_| "checkin_registry_invalid".to_string())?;
        // 当前账号：主库由登录态实测（官方登录态可能漂移）；副环境以
        // 注册表 current_profile_id 为权威（登录由工具托管，无实测语义）。
        let current =
            library_account::resolve_library_account(&material_root, &storage_root, &ctx)?
                .and_then(LibraryAccountResolution::into_account);

        // P15-11：预聚合缓存壳——指纹/密钥未变时复用上次报告（4.0s→<1ms）；
        // 改库编排成功后经 invalidate_master_checkup_cache 显式作废。
        match read_master_checkup_cached(&ctx.dir, &raw_key) {
            MasterCheckupStatus::Ready(report) => {
                let Some(current) = current.as_ref() else {
                    return Ok(MasterCheckupDto {
                        status: "no_current_account",
                        current_account_name: None,
                        accounts: Vec::new(),
                        orphan_project_count: 0,
                        orphan_session_count: 0,
                    });
                };
                let current_user_id = Some(current.account_id.as_str());
                let accounts = report
                    .accounts
                    .iter()
                    .map(|row| {
                        let record = records.iter().find(|r| r.account_id == row.user_id);
                        MasterCheckupAccountDto {
                            user_id: row.user_id.clone(),
                            account_name: record.map(|r| {
                                r.display_name
                                    .clone()
                                    .unwrap_or_else(|| r.screen_name.clone())
                            }),
                            registered: record.is_some(),
                            current: current_user_id == Some(row.user_id.as_str()),
                            project_count: row.project_count,
                            session_count: row.session_count,
                            empty_project_count: row.empty_project_count,
                        }
                    })
                    .collect();
                Ok(MasterCheckupDto {
                    status: "ready",
                    current_account_name: Some({
                        current
                            .display_name
                            .clone()
                            .unwrap_or_else(|| current.screen_name.clone())
                    }),
                    accounts,
                    orphan_project_count: report.orphan_project_count,
                    orphan_session_count: report.orphan_session_count,
                })
            }
            MasterCheckupStatus::NoMasterData => Ok(MasterCheckupDto {
                status: "no_master_data",
                current_account_name: None,
                accounts: Vec::new(),
                orphan_project_count: 0,
                orphan_session_count: 0,
            }),
            MasterCheckupStatus::ReadFailed => Ok(MasterCheckupDto {
                status: "read_failed",
                current_account_name: None,
                accounts: Vec::new(),
                orphan_project_count: 0,
                orphan_session_count: 0,
            }),
        }
    })
    .await
}

/// 收编进度事件（`master-incorporate-progress`；环境页收编弹层消费）。
///
/// 阶段：closing → backing_up → incorporating → restarting → done。
/// P11-5⑤：载荷带 env_id 区分库实例（主库 "master" / 副环境 env_id），
/// 多库收编弹层并行时只推进自己目标的事件。
#[derive(Clone, Serialize)]
struct MasterIncorporateProgressEvent {
    stage: &'static str,
    env_id: String,
}

/// P5-5 收编回执（incorporate_master_records 返回）。
#[derive(Clone, Serialize)]
pub(crate) struct MasterIncorporateResultDto {
    /// 归入的账号数（滞留账号分布行数）。
    merged_accounts: u32,
    /// 归属随行的 project 行数。
    transferred_projects: u32,
    /// 自动清理的空镜像行数。
    removed_mirror_rows: u32,
    /// 换腿（交接）的会话数。
    switched_sessions: u32,
    /// 合并前自动创建的备份路径（人工恢复定位）。
    backup_path: String,
    /// 接力台账写入结果（false = 收编完成但台账失败，只影响轨迹展示）。
    relay_ledger_written: bool,
    /// 库实例重启结果：failed 表示收编已完成，但需要用户手动启动。
    relaunch_outcome: String,
}

/// P5-5 一键收编：非当前账号记录全部归入当前账号（P11-5⑤ 泛化到副环境）。
///
/// 编排与切号五步同构（关实例 → 备份 → 归属改写/换腿 → 重启），但不做
/// 登录互换与插件对账——收编不改登录身份，插件环境不变。
/// 破坏性批量操作：库生成中拒绝（`master_incorporate_busy`）；
/// 执行前自动 `.switch-bak-*` 备份（ADR-0018 铁律，备份失败不执行）。
/// `library_id` 为库实例引用（ADR-0025，缺省主库；副环境 env_id 即库 id）。
#[tauri::command]
pub(crate) async fn incorporate_master_records(
    app: tauri::AppHandle,
    library_id: Option<String>,
    state: tauri::State<'_, AppState>,
) -> Result<MasterIncorporateResultDto, String> {
    ensure_real_preview(&state)?;
    if state.source_raw_key.is_empty() {
        return Err("source_key_unavailable".to_string());
    }
    let material_root = checkin_material_root(&state)?;
    let storage_root = PathBuf::from(&state.storage_root);
    let recovery_root = PathBuf::from(&state.recovery_root);
    let raw_key = state.source_raw_key.clone();
    run_blocking("master_incorporate_join_failed", move || {
        let (ctx, _lease) =
            acquire_library_write_lease(&recovery_root, &storage_root, library_id.as_deref())?;
        ctx.ensure_writable()?;
        incorporate_master_records_inner(&app, &material_root, &storage_root, &raw_key, &ctx)
    })
    .await
    .inspect(|_| {
        // P15-11：收编改写主库成功后作废体检预聚合缓存（WAL checkpoint 前
        // 指纹不变，显式失效保证零陈旧窗口）。
        traesync_infrastructure::master_checkup::invalidate_master_checkup_cache();
    })
}

/// 收编编排主体（阻塞上下文）：体检预检 → 四步事务 → 台账 → 回执。
/// 库实例差异全部由 LibraryContext 承载（目录 / 关启口径 / 备份目录 / 台账）。
fn incorporate_master_records_inner(
    app: &tauri::AppHandle,
    material_root: &Path,
    storage_root: &Path,
    raw_key: &str,
    ctx: &LibraryContext,
) -> Result<MasterIncorporateResultDto, String> {
    use traesync_infrastructure::master_checkup::{read_master_checkup, MasterCheckupStatus};

    // 前置：当前账号（收编目标）按库口径解析——主库实测优先 + 注册表
    // 缓存回退；副环境注册表 current_profile_id 即权威。
    let record = library_account::resolve_library_account(material_root, storage_root, ctx)?
        .and_then(LibraryAccountResolution::into_account)
        .ok_or_else(|| "incorporate_no_current_account".to_string())?;
    let current_profile = record.profile_id.clone();
    let target_user_id = record.account_id.clone();
    // 重启窗口标题：主库 = 当前账号名（P5-5 口径）；副环境 = 环境名
    //（与环境页启动入口一致，并行环境按名区分）。
    let window_title = if ctx.is_master() {
        record
            .display_name
            .clone()
            .unwrap_or_else(|| record.screen_name.clone())
    } else {
        ctx.env_name.clone()
    };

    let db_path = master_database_path(&ctx.dir);

    // 预检（只读）：确认确有滞留记录才关实例——避免空跑打扰正在使用的库。
    let stale_count = match read_master_checkup(&ctx.dir, raw_key) {
        MasterCheckupStatus::Ready(report) => report.stale_rows(&target_user_id).len(),
        MasterCheckupStatus::NoMasterData => return Err("master_switch_db_missing".to_string()),
        MasterCheckupStatus::ReadFailed => return Err("master_incorporate_read_failed".to_string()),
    };
    if stale_count == 0 {
        return Err("incorporate_nothing_to_do".to_string());
    }

    let emit = |stage: &'static str| {
        let _ = app.emit(
            "master-incorporate-progress",
            MasterIncorporateProgressEvent {
                stage,
                env_id: ctx.env_id.clone(),
            },
        );
    };

    // 生成中拒绝（收编非紧急操作，不做强制分支——等待完成后重试更安全）。
    if db_path.is_file()
        && master_db_activity_detected(&db_path, std::time::Duration::from_millis(800))
    {
        return Err("master_incorporate_busy".to_string());
    }

    // 第 1 步：关闭库实例（与切号同纪律：改写前拿到静止一致的库）。
    emit("closing");
    ctx.close_instance().map_err(|error| {
        crate::process_guard::close_failure_code(error, "master_incorporate_close_failed")
    })?;

    // 第 2 步：三件套备份（ADR-0018 铁律；create_new 永不覆盖备份链）。
    emit("backing_up");
    // P11-4①/P11-5⑤：写入当前库作用域活动备份目录（副环境链与主库链隔离）。
    let backup_dir = resolve_library_backup_dir(storage_root, &db_path, &ctx.env_id);
    let backup_path =
        backup_master_trio_to(&db_path, Some(&backup_dir)).map_err(|error| match error {
            MasterHandoverError::DbUnavailable => "master_switch_db_missing".to_string(),
            _ => "master_switch_backup_failed".to_string(),
        })?;
    // P5-9：备份生成后按保留策略清理旧备份（静默容错，只动当前库的链）。
    prune_backups_if_enabled_in(&db_path, storage_root, &backup_dir);

    // 第 3 步：归属改写 + 换腿 + 完整性校验（交接原语单事务；非空
    // UNIQUE 冲突报人工决策，禁止静默覆盖）。
    emit("incorporating");
    let handover = handover_master_records(&db_path, raw_key, &target_user_id).map_err(
        |error| match error {
            MasterHandoverError::DbUnavailable => "master_switch_db_missing",
            MasterHandoverError::TargetConflict(_) => "master_incorporate_conflict",
            MasterHandoverError::CommittedNeedsAttention => {
                "master_incorporate_committed_needs_attention"
            }
            MasterHandoverError::CommitOutcomeUnknown => {
                "master_incorporate_commit_outcome_unknown"
            }
            _ => "master_switch_db_open_failed",
        },
    )?;

    // 接力台账（逐会话 from = 原归属账号，保溯源链；fail-soft 不阻断收编）。
    // P11-5⑤：台账按库隔离，副环境收编不混入主库轨迹。
    let relay_ledger_written = append_incorporate_ledger(
        storage_root,
        &ctx.env_id,
        &handover,
        &target_user_id,
        &current_profile,
    );

    // 第 4 步：重启库实例（登录身份未变；主库带账号名标题、副环境带环境名）。
    emit("restarting");
    let relaunch_outcome = match ctx.relaunch(&window_title) {
        Ok(relaunch) => relaunch.outcome.to_string(),
        Err(error) => {
            eprintln!("[master-incorporate] 库实例重启失败（收编已完成）：{error}");
            "failed".to_string()
        }
    };
    emit("done");

    Ok(MasterIncorporateResultDto {
        merged_accounts: stale_count as u32,
        transferred_projects: handover.transferred_projects as u32,
        removed_mirror_rows: handover.removed_mirror_rows as u32,
        switched_sessions: handover.switched_sessions.len() as u32,
        backup_path: backup_path.display().to_string(),
        relay_ledger_written,
        relaunch_outcome,
    })
}

/// 收编台账追加：逐会话 from = 交接前实际归属（多账号杂居时精确到行）。
///
/// 与切号 append_relay_ledger 的差异：切号沿用 previous_owner 单值
/// （Q3 单账号不变量），收编面向多账号杂居，逐会话取 previous_user_id。
/// P11-5⑤：台账按库隔离（relay_ledger_root）——副环境收编轨迹写
/// 独立文件，不混入主库台账（主库两层校验不受环境收编影响）。
fn append_incorporate_ledger(
    storage_root: &Path,
    env_id: &str,
    handover: &MasterHandover,
    to_user_id: &str,
    to_profile_id: &str,
) -> bool {
    if handover.switched_sessions.is_empty() {
        return true;
    }
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
            from_user_id: session.previous_user_id.clone(),
            to_user_id: to_user_id.to_string(),
            to_profile_id: to_profile_id.to_string(),
            message_count_at_switch: session.message_count,
            switched_at_unix_seconds,
        })
        .collect();
    RelayLedger::new(relay_ledger_root(storage_root, env_id))
        .append(&entries)
        .is_ok()
}

/// P11-5⑤ 台账存储根按库路由：主库 = storage_root/environments（原位置
/// 不变）；副环境 = storage_root/environments/relay-ledgers/{env_id}——
/// 各库独立台账文件（relay-ledger.json），按文件名找同库。
fn relay_ledger_root(storage_root: &Path, env_id: &str) -> PathBuf {
    if env_id == MASTER_ENV_ID {
        return storage_root.join("environments");
    }
    storage_root
        .join("environments")
        .join("relay-ledgers")
        .join(env_id)
}

// ===== P13-2 Trae CN 项目归并（ADR-0033 选择性写：归并命令族）=====
//
// 写边界守卫矩阵（扩展而非破坏既有 ensure_writable 体系；P13-3 起归档
// 类写经 ensure_archive_writable 对 trae_cn 放行）：
// - `merge_trae_cn_project` / `preview_trae_cn_project_merge` /
//   `list_trae_cn_foreign_projects`：经 `ensure_project_merge_writable`
//   仅对 trae_cn 库放行，其余库拒绝（`trae_cn_library_only`）；
// - `apply_master_archive_changes`（P13-3）：经 `ensure_archive_writable`
//   对 trae_cn 库放行归档/恢复（hidden_status 通道）；
// - trae_cn 库的其余写命令（收编/清理/删除/合并分组）仍被
//   `ensure_writable`（read_only）拒绝（`library_read_only`）——选择性写
//   不翻转只读边界，删除/合并项目在 CN 库依旧无入口。
// 备份先行（家族铁律）：备份在命令编排层执行（与收编同款），写入 CN 库
// 作用域活动备份目录（backup_dir 覆盖时隔离），保证备份链展示/恢复同源。
// Trae CN 实例不经工具托管（无自动关启与重启能力）：客户端运行中拒绝
// （fail-closed），由用户关闭后重试；故无 close/relaunch 编排步骤。

/// P13-2 CN 详情页「其他账号的项目」行（list_trae_cn_foreign_projects 返回）。
#[derive(Clone, Serialize)]
pub(crate) struct TraeCnForeignProjectDto {
    project_id: String,
    name: String,
    absolute_path: Option<String>,
    session_count: u32,
    message_count: u32,
}

/// P13-2 归并预览的来源账号行（规模确认弹层逐账号规模）。
#[derive(Clone, Debug, Serialize)]
struct TraeCnMergeSourceDto {
    /// TRAE user_id（悬浮提示用，不作主信息；界面表达纪律）。
    user_id: String,
    /// 账号展示名（CN 产品注册表反查；未登记为 None，前端占位）。
    account_name: Option<String>,
    session_count: u32,
    message_count: u32,
}

/// P13-2 归并规模预览（preview_trae_cn_project_merge 返回，只读）。
#[derive(Clone, Debug, Serialize)]
pub(crate) struct TraeCnMergePreviewDto {
    /// 项目文件夹绝对路径（弹层悬浮提示，主视野用尾段）。
    absolute_path: Option<String>,
    /// 归并目标账号展示名（当前指针账号，注册表权威）。
    target_account_name: String,
    /// 当前账号名下是否已有同目录行（false = 整体转入形态）。
    target_exists: bool,
    /// 目标行既有正常会话数（target_exists 时展示）。
    target_session_count: u32,
    /// 其他账号的行规模（逐行）。
    sources: Vec<TraeCnMergeSourceDto>,
    /// 将并入的会话总数 / 消息总数。
    total_sessions: u32,
    total_messages: u32,
}

/// P13-2 归并回执（merge_trae_cn_project 返回）。
#[derive(Clone, Debug, Serialize)]
pub(crate) struct TraeCnMergeResultDto {
    /// 归属随行 / 并走的 project 行数。
    transferred_projects: u32,
    /// 自动清理的空镜像行数。
    removed_mirror_rows: u32,
    /// 换腿（交接）的会话数。
    switched_sessions: u32,
    /// 换腿会话的消息总数。
    moved_messages: u32,
    /// 归并前自动创建的备份路径（人工恢复定位）。
    backup_path: String,
    /// 接力台账写入结果（false = 归并完成但台账失败，只影响轨迹展示）。
    relay_ledger_written: bool,
}

/// 归并进度事件（`trae-cn-merge-progress`；CN 详情页确认弹层消费）。
/// 阶段：backing_up → mapping → executing → verifying → done。
#[derive(Clone, Serialize)]
struct TraeCnMergeProgressEvent {
    stage: &'static str,
    current: usize,
    total: usize,
    label: String,
}

/// P13-2 归并错误码映射（稳定错误码，前端 safeUiError 词表承接）。
fn map_trae_cn_merge_error(error: MasterHandoverError) -> String {
    match error {
        MasterHandoverError::DbUnavailable => "trae_cn_merge_db_missing".to_string(),
        MasterHandoverError::ProjectNotFound => "trae_cn_merge_project_not_found".to_string(),
        MasterHandoverError::ProjectNotMergeable => "trae_cn_merge_not_mergeable".to_string(),
        MasterHandoverError::NothingToMerge => "trae_cn_merge_nothing_to_do".to_string(),
        MasterHandoverError::CommittedNeedsAttention => {
            "trae_cn_merge_committed_needs_attention".to_string()
        }
        MasterHandoverError::CommitOutcomeUnknown => {
            "trae_cn_merge_commit_outcome_unknown".to_string()
        }
        _ => "trae_cn_merge_db_open_failed".to_string(),
    }
}

/// P13-2 CN 归并命令共用前置：库守卫（仅 trae_cn）+ 当前指针账号解析。
/// 非法库 / 指针为空 / 指针悬空都以稳定错误码拒绝。
fn resolve_trae_cn_merge_context(
    material_root: &Path,
    storage_root: &Path,
    library_id: Option<&str>,
) -> Result<
    (
        LibraryContext,
        traesync_infrastructure::account_registry::AccountRecord,
    ),
    String,
> {
    let ctx = resolve_library_context(library_id, storage_root)?;
    ctx.ensure_project_merge_writable()?;
    let account = library_account::resolve_library_account(material_root, storage_root, &ctx)?
        .and_then(LibraryAccountResolution::into_account)
        .ok_or_else(|| "trae_cn_merge_no_current_account".to_string())?;
    Ok((ctx, account))
}

/// 归并预览 user_id → 展示名（备注名优先，与账号页口径一致）。
/// P15-7：档案源 = Work 签到注册表（融合版账号主体，旧产品池已移除）。
fn trae_cn_account_name_by_user_id(accounts_root: &Path, user_id: &str) -> Option<String> {
    let records = AccountRegistry::new(accounts_root).load().ok()?;
    let record = records.iter().find(|record| record.account_id == user_id)?;
    Some(
        record
            .display_name
            .clone()
            .unwrap_or_else(|| record.screen_name.clone()),
    )
}

/// P13-2：列出其他账号名下的可归并项目行（CN 详情页入口数据源，只读）。
/// 无当前指针账号时返回空列表（归并入口自然消失，不报错打断浏览）。
#[tauri::command]
pub(crate) async fn list_trae_cn_foreign_projects(
    library_id: Option<String>,
    state: tauri::State<'_, AppState>,
) -> Result<Vec<TraeCnForeignProjectDto>, String> {
    ensure_real_preview(&state)?;
    if state.source_raw_key.is_empty() {
        return Err("source_key_unavailable".to_string());
    }
    let material_root = checkin_material_root(&state)?;
    let storage_root = PathBuf::from(&state.storage_root);
    let raw_key = state.source_raw_key.clone();
    run_blocking("trae_cn_merge_join_failed", move || {
        let ctx = resolve_library_context(library_id.as_deref(), &storage_root)?;
        ctx.ensure_project_merge_writable()?;
        let Some(account) =
            library_account::resolve_library_account(&material_root, &storage_root, &ctx)?
                .and_then(LibraryAccountResolution::into_account)
        else {
            return Ok(Vec::new());
        };
        Ok(
            read_trae_cn_foreign_projects(&ctx.dir, &raw_key, &account.account_id)
                .into_iter()
                .map(|entry| TraeCnForeignProjectDto {
                    project_id: entry.project_id,
                    name: entry.name,
                    absolute_path: entry.absolute_path,
                    session_count: entry.session_count,
                    message_count: entry.message_count,
                })
                .collect(),
        )
    })
    .await
}

/// P13-2：归并规模预览（只读；同目录各账号行/会话数/消息数）。
#[tauri::command]
pub(crate) async fn preview_trae_cn_project_merge(
    project_id: String,
    library_id: Option<String>,
    state: tauri::State<'_, AppState>,
) -> Result<TraeCnMergePreviewDto, String> {
    ensure_real_preview(&state)?;
    if state.source_raw_key.is_empty() {
        return Err("source_key_unavailable".to_string());
    }
    let material_root = checkin_material_root(&state)?;
    let storage_root = PathBuf::from(&state.storage_root);
    let raw_key = state.source_raw_key.clone();
    run_blocking("trae_cn_merge_join_failed", move || {
        let (ctx, account) =
            resolve_trae_cn_merge_context(&material_root, &storage_root, library_id.as_deref())?;
        preview_trae_cn_project_merge_inner(
            &material_root,
            &raw_key,
            &ctx,
            &account,
            &project_id,
        )
    })
    .await
}

/// 归并预览主体（阻塞上下文，可注入测试）。
fn preview_trae_cn_project_merge_inner(
    accounts_root: &Path,
    raw_key: &str,
    ctx: &LibraryContext,
    account: &traesync_infrastructure::account_registry::AccountRecord,
    project_id: &str,
) -> Result<TraeCnMergePreviewDto, String> {
    let db_path = master_database_path(&ctx.dir);
    let preview =
        preview_trae_cn_project_merge_infra(&db_path, raw_key, &account.account_id, project_id)
            .map_err(map_trae_cn_merge_error)?;
    let sources = preview
        .source_rows
        .iter()
        .map(|row| TraeCnMergeSourceDto {
            user_id: row.user_id.clone(),
            account_name: trae_cn_account_name_by_user_id(accounts_root, &row.user_id),
            session_count: row.normal_sessions.max(0) as u32,
            message_count: row.messages.max(0) as u32,
        })
        .collect();
    Ok(TraeCnMergePreviewDto {
        absolute_path: preview.absolute_path,
        target_account_name: account
            .display_name
            .clone()
            .unwrap_or_else(|| account.screen_name.clone()),
        target_exists: preview.target_row.is_some(),
        target_session_count: preview
            .target_row
            .as_ref()
            .map(|row| row.normal_sessions.max(0) as u32)
            .unwrap_or(0),
        total_sessions: preview
            .source_rows
            .iter()
            .map(|row| row.normal_sessions)
            .sum::<i64>()
            .max(0) as u32,
        total_messages: preview
            .source_rows
            .iter()
            .map(|row| row.messages)
            .sum::<i64>()
            .max(0) as u32,
        sources,
    })
}

/// P13-2：CN 项目归并（目标 = 当前指针账号，注册表权威解析）。
/// 单次确认由前端编排；后端负责预检 → 客户端运行/生成中拒绝 → 备份先行
/// → 单事务归并 → 台账 → 回执，进度经 `trae-cn-merge-progress` 上报。
#[tauri::command]
pub(crate) async fn merge_trae_cn_project(
    app: tauri::AppHandle,
    project_id: String,
    library_id: Option<String>,
    state: tauri::State<'_, AppState>,
) -> Result<TraeCnMergeResultDto, String> {
    let progress_app = app.clone();
    merge_trae_cn_project_inner(
        project_id,
        library_id,
        state.inner(),
        move |event| {
            let _ = progress_app.emit("trae-cn-merge-progress", event);
        },
        move |request, on_progress| async move {
            let TraeCnProjectMergeRequest {
                project_id,
                library_id,
                material_root,
                storage_root,
                recovery_root,
                raw_key,
            } = request;
            run_blocking("trae_cn_merge_join_failed", move || {
                let (ctx, lifecycle_lease) = prepare_library_write_context(
                    &recovery_root,
                    &storage_root,
                    library_id.as_deref(),
                )?;
                let resolve_account = |ctx: &LibraryContext| {
                    resolve_trae_cn_merge_context(&material_root, &storage_root, Some(&ctx.env_id))
                        .map(|(_, account)| account)
                };
                // Trae CN 客户端运行检测（真实进程枚举；单测注入替身）。
                let ensure_client_stopped = |ctx: &LibraryContext| {
                    crate::process_guard::ensure_product_stopped(
                        TraeProduct::TraeCn,
                        &ctx.dir,
                        "trae_cn_merge_client_running",
                    )
                };
                run_trae_cn_project_merge_with_lease(
                    &recovery_root,
                    &storage_root,
                    ctx,
                    lifecycle_lease,
                    &resolve_account,
                    &raw_key,
                    &project_id,
                    &ensure_client_stopped,
                    &on_progress,
                )
            })
            .await
        },
    )
    .await
}

struct TraeCnProjectMergeRequest {
    project_id: String,
    library_id: Option<String>,
    material_root: PathBuf,
    storage_root: PathBuf,
    recovery_root: PathBuf,
    raw_key: String,
}

/// 普通 CN 归并命令的生产编排；执行器只在产品/库/操作门禁全部通过后调用。
async fn merge_trae_cn_project_inner<F, Fut, P>(
    project_id: String,
    library_id: Option<String>,
    state: &AppState,
    on_progress: P,
    execute: F,
) -> Result<TraeCnMergeResultDto, String>
where
    F: FnOnce(TraeCnProjectMergeRequest, P) -> Fut + Send,
    Fut: Future<Output = Result<TraeCnMergeResultDto, String>> + Send,
    P: Fn(TraeCnMergeProgressEvent) + Send + Sync + 'static,
{
    let validation_root = PathBuf::from(&state.storage_root);
    ensure_trae_cn_project_merge_ready(
        library_id.as_deref(),
        &validation_root,
        state.runtime_mode == RuntimeMode::RealReadPreview,
    )?;
    let validation_context = resolve_library_context(library_id.as_deref(), &validation_root)?;
    validation_context.ensure_project_merge_writable()?;
    ensure_real_preview(&state)?;
    if state.source_raw_key.is_empty() {
        return Err("source_key_unavailable".to_string());
    }
    let material_root = checkin_material_root(&state)?;
    execute(
        TraeCnProjectMergeRequest {
            project_id,
            library_id,
            material_root,
            storage_root: PathBuf::from(&state.storage_root),
            recovery_root: PathBuf::from(&state.recovery_root),
            raw_key: state.source_raw_key.clone(),
        },
        on_progress,
    )
    .await
    .inspect(|_| {
        // P15-11：CN 项目归并改写主库成功后作废体检预聚合缓存。
        traesync_infrastructure::master_checkup::invalidate_master_checkup_cache();
    })
}

/// Trae CN 归并只接受明确的 Trae CN 库引用，不沿用缺省主库路由。
pub(crate) fn ensure_trae_cn_project_merge_ready(
    library_id: Option<&str>,
    storage_root: &Path,
    real_environment: bool,
) -> Result<(), String> {
    if library_id != Some(TRAE_CN_LIBRARY_ID) {
        return Err("product_library_mismatch".to_string());
    }
    crate::product_operations::ensure_ready(
        "trae_cn",
        "library.trae_cn_project_merge",
        Some(TRAE_CN_LIBRARY_ID.to_string()),
        storage_root,
        real_environment,
    )
}

/// CN 归并生产/合成库共用：目标目录租约覆盖身份解析与所有数据库写副作用。
fn run_trae_cn_project_merge_with_lease(
    recovery_root: &Path,
    storage_root: &Path,
    ctx: LibraryContext,
    lifecycle_lease: Option<OperationLease>,
    resolve_account: &dyn Fn(
        &LibraryContext,
    ) -> Result<
        traesync_infrastructure::account_registry::AccountRecord,
        String,
    >,
    raw_key: &str,
    project_id: &str,
    ensure_client_stopped: &dyn Fn(&LibraryContext) -> Result<(), String>,
    on_progress: &dyn Fn(TraeCnMergeProgressEvent),
) -> Result<TraeCnMergeResultDto, String> {
    let _data_lease = acquire_library_data_lease(recovery_root, &ctx)?;
    drop(lifecycle_lease);
    let account = resolve_account(&ctx)?;
    let ensure_stopped = || ensure_client_stopped(&ctx);
    run_trae_cn_project_merge(
        storage_root,
        raw_key,
        &ctx,
        &account,
        project_id,
        &ensure_stopped,
        on_progress,
    )
}

/// CN 项目归并编排主体（阻塞上下文；客户端运行检测与进度回调可注入，
/// 供单测覆盖守卫矩阵与回执）。
fn run_trae_cn_project_merge(
    storage_root: &Path,
    raw_key: &str,
    ctx: &LibraryContext,
    account: &traesync_infrastructure::account_registry::AccountRecord,
    project_id: &str,
    ensure_client_stopped: &dyn Fn() -> Result<(), String>,
    on_progress: &dyn Fn(TraeCnMergeProgressEvent),
) -> Result<TraeCnMergeResultDto, String> {
    // 双重守卫：命令层已校验，此处保持 inner 可独立调用（fail-closed）。
    ctx.ensure_project_merge_writable()?;
    let target_user_id = account.account_id.clone();
    let db_path = master_database_path(&ctx.dir);
    if !db_path.is_file() {
        return Err("trae_cn_merge_db_missing".to_string());
    }

    // 预检（只读）：门槛与规模（无路径/无可并内容在这里 fail-fast，
    // 不产生备份与任何写）。
    let preview =
        preview_trae_cn_project_merge_infra(&db_path, raw_key, &target_user_id, project_id)
            .map_err(map_trae_cn_merge_error)?;
    if preview
        .source_rows
        .iter()
        .map(|row| row.normal_sessions)
        .sum::<i64>()
        == 0
    {
        return Err("trae_cn_merge_nothing_to_do".to_string());
    }

    // Trae CN 客户端运行中拒绝：归并需要静止一致的库做备份与事务写；
    // CN 实例不经工具托管，由用户关闭后重试。
    ensure_client_stopped()?;
    // 生成中拒绝（WAL 活动 = 官方客户端正在写库）。
    if master_db_activity_detected(&db_path, std::time::Duration::from_millis(800)) {
        return Err("trae_cn_merge_busy".to_string());
    }

    // 备份先行（ADR-0018 家族铁律；写入 CN 库作用域活动备份目录）。
    on_progress(TraeCnMergeProgressEvent {
        stage: "backing_up",
        current: 0,
        total: 0,
        label: String::new(),
    });
    let backup_dir = resolve_library_backup_dir(storage_root, &db_path, &ctx.env_id);
    let backup_path =
        backup_master_trio_to(&db_path, Some(&backup_dir)).map_err(|error| match error {
            MasterHandoverError::DbUnavailable => "trae_cn_merge_db_missing".to_string(),
            _ => "trae_cn_merge_backup_failed".to_string(),
        })?;
    // P5-9：备份生成后按保留策略清理旧备份（静默容错，只动当前库的链）。
    prune_backups_if_enabled_in(&db_path, storage_root, &backup_dir);

    // 单事务归并（mapping/executing/verifying 进度逐项转发）。
    let handover = merge_trae_cn_project_records_with_progress(
        &db_path,
        raw_key,
        &target_user_id,
        project_id,
        &|progress| {
            on_progress(TraeCnMergeProgressEvent {
                stage: progress.phase,
                current: progress.current,
                total: progress.total,
                label: progress.label,
            });
        },
    )
    .map_err(map_trae_cn_merge_error)?;

    // 接力台账（CN 库作用域独立文件 environments/relay-ledgers/trae_cn；
    // 复用收编台账追加：逐会话 from = 交接前归属。fail-soft 不阻断归并）。
    let relay_ledger_written = append_incorporate_ledger(
        storage_root,
        &ctx.env_id,
        &handover,
        &target_user_id,
        &account.profile_id,
    );

    on_progress(TraeCnMergeProgressEvent {
        stage: "done",
        current: 0,
        total: 0,
        label: String::new(),
    });

    Ok(TraeCnMergeResultDto {
        transferred_projects: handover.transferred_projects as u32,
        removed_mirror_rows: handover.removed_mirror_rows as u32,
        switched_sessions: handover.switched_sessions.len() as u32,
        moved_messages: handover
            .switched_sessions
            .iter()
            .map(|session| session.message_count)
            .sum::<i64>()
            .max(0) as u32,
        backup_path: backup_path.display().to_string(),
        relay_ledger_written,
    })
}

/// P5-5 清理空项目回执（cleanup_master_empty_projects 返回）。
#[derive(Clone, Serialize)]
pub(crate) struct MasterEmptyProjectCleanupDto {
    /// 删除的空项目记录行数。
    deleted_projects: u32,
    /// 清理前自动创建的备份路径（人工恢复定位）。
    backup_path: String,
    /// 主库重启结果。
    relaunch_outcome: String,
}

/// P5-5 清理空项目：硬删除体检诊断出的空项目记录行（破坏性批量操作）。
///
/// 编排与收编同构（关实例 → 备份 → 删除 → 重启）；执行前自动
/// `.switch-bak-*` 备份（ADR-0018 铁律，备份失败不执行）。生成中拒绝
/// （`cleanup_master_busy`）；孤儿行保持只报告不处理（保守口径）。
/// P11-5⑤：`library_id` 为库实例引用（缺省主库；副环境同义）。
#[tauri::command]
pub(crate) async fn cleanup_master_empty_projects(
    library_id: Option<String>,
    state: tauri::State<'_, AppState>,
) -> Result<MasterEmptyProjectCleanupDto, String> {
    ensure_real_preview(&state)?;
    if state.source_raw_key.is_empty() {
        return Err("source_key_unavailable".to_string());
    }
    let material_root = checkin_material_root(&state)?;
    let storage_root = PathBuf::from(&state.storage_root);
    let recovery_root = PathBuf::from(&state.recovery_root);
    let raw_key = state.source_raw_key.clone();
    run_blocking("cleanup_master_join_failed", move || {
        let (ctx, _lease) =
            acquire_library_write_lease(&recovery_root, &storage_root, library_id.as_deref())?;
        ctx.ensure_writable()?;
        cleanup_master_empty_projects_inner(&material_root, &storage_root, &raw_key, &ctx)
    })
    .await
    .inspect(|_| {
        // P15-11：清理空项目改写主库成功后作废体检预聚合缓存。
        traesync_infrastructure::master_checkup::invalidate_master_checkup_cache();
    })
}

/// 清理空项目编排主体（阻塞上下文）：预检 → 关实例 → 备份 → 删除 → 重启。
/// 库实例差异全部由 LibraryContext 承载（目录 / 关启口径 / 备份目录）。
fn cleanup_master_empty_projects_inner(
    material_root: &Path,
    storage_root: &Path,
    raw_key: &str,
    ctx: &LibraryContext,
) -> Result<MasterEmptyProjectCleanupDto, String> {
    use traesync_infrastructure::master_checkup::{
        delete_master_empty_projects, read_master_checkup, MasterCheckupStatus,
    };

    // 重启库实例需要窗口标题：与收编同源，主库取实测当前账号展示名，
    // 副环境用环境名（与启动入口一致）。
    let record = library_account::resolve_library_account(material_root, storage_root, ctx)?
        .and_then(LibraryAccountResolution::into_account)
        .ok_or_else(|| "cleanup_master_no_current_account".to_string())?;
    let window_title = if ctx.is_master() {
        record
            .display_name
            .clone()
            .unwrap_or_else(|| record.screen_name.clone())
    } else {
        ctx.env_name.clone()
    };

    let db_path = master_database_path(&ctx.dir);

    // 预检（只读）：确认确有空项目才关实例——避免空跑打扰正在使用的库。
    // 体检口径与删除范围一致（软删残留不计入），预检总数即预期删除规模。
    let empty_total = match read_master_checkup(&ctx.dir, raw_key) {
        MasterCheckupStatus::Ready(report) => report.empty_project_total(),
        MasterCheckupStatus::NoMasterData => return Err("master_switch_db_missing".to_string()),
        MasterCheckupStatus::ReadFailed => return Err("cleanup_master_read_failed".to_string()),
    };
    if empty_total == 0 {
        return Err("cleanup_master_nothing_to_do".to_string());
    }

    // 生成中拒绝（清理非紧急操作，不做强制分支——等待完成后重试更安全）。
    if db_path.is_file()
        && master_db_activity_detected(&db_path, std::time::Duration::from_millis(800))
    {
        return Err("cleanup_master_busy".to_string());
    }

    // 第 1 步：关闭库实例（与收编同纪律：删除前拿到静止一致的库）。
    ctx.close_instance().map_err(|error| {
        crate::process_guard::close_failure_code(error, "cleanup_master_close_failed")
    })?;

    // 失败恢复：删除提交前的失败路径尽力重启库实例，不把实例留在关闭态
    // （与归档提交同纪律）。
    let restart_after_failure = || {
        let _ = ctx.relaunch(&window_title);
    };

    // 第 2 步：三件套备份（ADR-0018 铁律；create_new 永不覆盖备份链）。
    // P11-4①/P11-5⑤：写入当前库作用域活动备份目录（副环境链与主库链隔离）。
    let backup_dir = resolve_library_backup_dir(storage_root, &db_path, &ctx.env_id);
    let backup_path = match backup_master_trio_to(&db_path, Some(&backup_dir)) {
        Ok(path) => path,
        Err(error) => {
            restart_after_failure();
            return Err(match error {
                MasterHandoverError::DbUnavailable => "master_switch_db_missing".to_string(),
                _ => "cleanup_master_backup_failed".to_string(),
            });
        }
    };
    // P5-9：备份生成后按保留策略清理旧备份（静默容错，只动当前库的链）。
    prune_backups_if_enabled_in(&db_path, storage_root, &backup_dir);

    // 第 3 步：单事务删除空项目行（范围与体检口径一致，返回实际删除数）。
    let deleted_projects = match delete_master_empty_projects(&ctx.dir, raw_key) {
        Ok(count) => count,
        Err(error) => {
            restart_after_failure();
            return Err(match error {
                MasterArchiveError::DbUnavailable => "master_switch_db_missing".to_string(),
                MasterArchiveError::DbOpenFailed => "cleanup_master_open_failed".to_string(),
                _ => "cleanup_master_write_failed".to_string(),
            });
        }
    };

    // 第 4 步：重启库实例（登录身份未变）。重启失败不回滚删除——数据已备份且
    // 删除已提交，按 relaunch_outcome="failed" 回执标注，前端据实提示
    // （与归档提交同口径，避免"清理未完成"的误导文案）。
    let relaunch_outcome = match ctx.relaunch(&window_title) {
        Ok(relaunch) => relaunch.outcome.to_string(),
        Err(_) => "failed".to_string(),
    };

    Ok(MasterEmptyProjectCleanupDto {
        deleted_projects: deleted_projects as u32,
        backup_path: backup_path.display().to_string(),
        relaunch_outcome,
    })
}

#[derive(Clone, Serialize)]
struct MasterBackupEntryDto {
    /// 备份时间戳（秒级 UNIX 时间）。
    stamp_unix_seconds: u64,
    /// 备份合计字节（db + 存在的 wal/shm）。
    total_bytes: u64,
    /// 是否带 wal 附属件。
    has_wal: bool,
    /// 是否带凭据备份子目录（credentials-{stamp}，P11-4②）。
    has_credentials: bool,
}

/// P5-4 主库备份链（get_master_backup_chain 返回）。
#[derive(Clone, Serialize)]
pub(crate) struct MasterBackupChainDto {
    backups: Vec<MasterBackupEntryDto>,
    /// 备份所在目录（展示与人工恢复定位）。
    backup_dir: String,
    /// 保留份数：读 backup-retention.json 实际配置（读取失败回退默认 5，
    /// P11-4④）；实际清理行为由备份保留设置决定（P5-9，ADR-0018 修订）。
    keep_policy: u32,
}

/// P5-4：枚举库备份链（设置页备份分区 + 库详情页数据源，只读）。
///
/// P11-5 收官：`library_id` 为库实例引用（缺省主库；副环境枚举自身
/// 作用域活动备份目录——覆盖时为环境子目录，未覆盖为 db 同目录）。
#[tauri::command]
pub(crate) async fn get_master_backup_chain(
    library_id: Option<String>,
    state: tauri::State<'_, AppState>,
) -> Result<MasterBackupChainDto, String> {
    ensure_real_preview(&state)?;
    let storage_root = PathBuf::from(&state.storage_root);
    run_blocking("master_backup_chain_join_failed", move || {
        let ctx = resolve_library_context(library_id.as_deref(), &storage_root)?;
        master_backup_chain_inner(&storage_root, &ctx)
    })
    .await
}

/// 备份链枚举主体（阻塞上下文）：按库作用域活动目录枚举 + 读保留配置。
fn master_backup_chain_inner(
    storage_root: &Path,
    ctx: &LibraryContext,
) -> Result<MasterBackupChainDto, String> {
    let db_path = master_database_path(&ctx.dir);
    let db_name = master_backup_db_name(&db_path)?;
    // P11-4①/P11-5⑤：枚举当前库作用域活动备份目录——sidecar 备份的
    // 恢复语义是「按文件名找同目录」，展示目录即恢复目录。
    let active_dir = resolve_library_backup_dir(storage_root, &db_path, &ctx.env_id);
    let backups: Vec<MasterBackupEntryDto> = list_master_backups_in(&active_dir, &db_name)
        .into_iter()
        .map(|entry| MasterBackupEntryDto {
            stamp_unix_seconds: entry.stamp_unix_seconds,
            total_bytes: entry.total_bytes,
            has_wal: entry.has_wal,
            has_credentials: entry.has_credentials,
        })
        .collect();
    Ok(MasterBackupChainDto {
        backup_dir: active_dir.display().to_string(),
        keep_policy: backup_keep_policy(storage_root),
        backups,
    })
}

/// P5-4：手动创建主库数据备份（设置页入口）。
///
/// 备份复制主库三件套为 `.switch-bak-{时间戳}`（create_new 永不覆盖既有
/// 备份链），写入当前活动备份目录（P11-4①：backup_dir 覆盖或 db 同目录）。
/// 主库实例运行中拒绝（`master_backup_running`）：运行中复制可能捕获不一致
/// 快照，先关闭主库再备份（与切号事务第 1 步同纪律）。
/// `include_credentials`（P11-4②）：为 true 时把签到凭据材料白名单整拷到
/// 备份目录下 `credentials-{同时间戳}/` 子目录（只拷密文与注册表）。
#[tauri::command]
pub(crate) async fn create_master_backup(
    state: tauri::State<'_, AppState>,
    include_credentials: bool,
) -> Result<String, String> {
    ensure_real_preview(&state)?;
    // P5-9：裁剪配置的存储根（闭包外拷出，State 引用不能进 spawn_blocking）。
    let storage_root_for_backup = state.storage_root.clone();
    let recovery_root = PathBuf::from(&state.recovery_root);
    run_blocking("master_backup_join_failed", move || {
        let master_dir =
            master_data_dir().map_err(|_| "environment_registry_invalid".to_string())?;
        let _lease = OperationLease::acquire_shared_for_path(&recovery_root, &master_dir)
            .map_err(|error| format!("operation_lease_unavailable:{error}"))?;
        // P15-6：备份守卫针对融合版主库实例（WorkCn 规则对 Trae CN 进程
        // 会误判 Unknown 拒绝，或客户端运行时漏判——与目录产品同源）。
        crate::process_guard::ensure_product_stopped(
            TraeProduct::TraeCn,
            &master_dir,
            "master_backup_running",
        )?;
        let db_path = master_database_path(&master_dir);
        let storage = Path::new(&storage_root_for_backup);
        let active_dir = resolve_master_backup_dir(storage, &db_path);
        // P11-4②：凭据子目录与三件套同戳（统一发号，恢复时按戳配对）。
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let backup = backup_master_trio_stamped(&db_path, Some(&active_dir), stamp)
            .map(|path| path.display().to_string())
            .map_err(|_| "master_backup_failed".to_string())?;
        if include_credentials {
            copy_credentials_backup(&storage.join("checkin"), &active_dir, stamp)?;
        }
        // P5-9：备份生成后按保留策略清理旧备份（静默容错，只动当前目录链）。
        prune_backups_if_enabled(&db_path, storage);
        Ok(backup)
    })
    .await
}

/// P11-4①：解析当前活动备份目录——配置 backup_dir 有值则写/读该目录，
/// 否则 P16-T1 新默认：**软件本体旁 `backups/`**（exe 同级推导）。
/// fail-soft：配置损坏回退新默认位置，不阻断备份/切号主流程（与裁剪的
/// 静默容错同口径）。
pub(crate) fn resolve_master_backup_dir(storage_root: &Path, db_path: &Path) -> PathBuf {
    if let Some(dir) = load_backup_dir_override(storage_root) {
        return dir;
    }
    default_master_backup_dir().unwrap_or_else(|| {
        // current_exe 不可得（理论兜底）：回退 db 同目录旧行为。
        db_path.parent().map(Path::to_path_buf).unwrap_or_default()
    })
}

/// P16-T1：默认备份目录推导——`current_exe` 同级 `backups/`。
///
/// 安装版 = `E:\trae-sync\Trae Sync\backups\`（软件本体旁，E 盘大容量盘）；
/// dev 版 = 编译产物目录下（`target\...\backups\`，仅本机开发可接受——
/// 测试与开发环境不触碰生产数据根，注释登记）。此改动使客户端目录
/// （`%APPDATA%\Trae CN\ModularData\ai-agent\`）不再产生新备份。
/// 用户配置（backup_dir 覆盖）优先于本默认推导。
pub(crate) fn default_master_backup_dir() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    exe.parent().map(|dir| dir.join("backups"))
}

/// P16-T1：解析基准备份主文件（双格式兼容）。
///
/// 旧未压缩（`{db 名}.switch-bak-{stamp}`）优先原样返回；新压缩
/// （`…{stamp}.zst`）解压为备份目录内临时件（命名 `.read-tmp-{stamp}`，
/// 用完由调用方删除）——SQLCipher 需要真实文件路径。两形态都不在时
/// 返回 None（上游报 backup_missing）。
fn materialize_backup_for_read(
    active_dir: &Path,
    db_name: &str,
    stamp: u64,
) -> Option<(PathBuf, Option<PathBuf>)> {
    let plain = active_dir.join(format!("{db_name}.switch-bak-{stamp}"));
    if plain.is_file() {
        return Some((plain, None));
    }
    let compressed = active_dir.join(format!("{db_name}.switch-bak-{stamp}.zst"));
    if !compressed.is_file() {
        return None;
    }
    // 唯一临时名（读取路径已在操作租约内串行）；残留先清（上次中断现场）。
    let temporary = active_dir.join(format!("{db_name}.switch-bak-{stamp}.read-tmp"));
    let _ = std::fs::remove_file(&temporary);
    traesync_infrastructure::master_handover::decompress_backup_member(&compressed, &temporary)
        .ok()
        .map(|_| (temporary.clone(), Some(temporary)))
}

/// P11-5⑤：库作用域活动备份目录——副环境备份链与主库链物理隔离。
///
/// 主库口径不变（`resolve_master_backup_dir`：覆盖根或 db 同目录）；
/// 副环境在 backup_dir 覆盖时落到「覆盖目录/environments/{env_id}/」
/// 子目录——主库链、其他环境链互不可见，裁剪（同前缀文件名匹配）
/// 天然不越界；未覆盖时 = db 同目录（环境目录本身即隔离边界）。
/// 备份文件名保持 `{db 名}.switch-bak-{时间戳}`：恢复语义「按文件名
/// 找同目录」不变。
pub(crate) fn resolve_library_backup_dir(
    storage_root: &Path,
    db_path: &Path,
    env_id: &str,
) -> PathBuf {
    if env_id == MASTER_ENV_ID {
        return resolve_master_backup_dir(storage_root, db_path);
    }
    if let Some(dir) = load_backup_dir_override(storage_root) {
        return dir.join("environments").join(env_id);
    }
    db_path.parent().map(Path::to_path_buf).unwrap_or_default()
}

/// P11-4① 备份位置校验（set/migrate 命令同口径）：绝对路径、长度上限、
/// 可创建（目录或其父级）、不落入签到材料根（防凭据材料与备份互混）。
/// 返回可直接使用的路径；None = 恢复默认位置。
fn validate_backup_dir(
    backup_dir: Option<&str>,
    storage_root: &Path,
) -> Result<Option<PathBuf>, String> {
    let Some(raw) = backup_dir else {
        return Ok(None);
    };
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed.len() > MAX_BACKUP_DIR_BYTES {
        return Err("backup_dir_invalid".to_string());
    }
    let path = PathBuf::from(trimmed);
    if !path.is_absolute() {
        return Err("backup_dir_invalid".to_string());
    }
    // 不允许等于或落入签到材料根内部：凭据材料与备份文件互相混入会破坏
    // 两边的目录语义（白名单拷贝、裁剪与恢复指引都以目录边界为准）。
    if path.starts_with(storage_root.join("checkin")) {
        return Err("backup_dir_conflicts_checkin".to_string());
    }
    // 目录可不存在，但须可创建（create_dir_all 幂等；父级不可达即拒绝）。
    std::fs::create_dir_all(&path).map_err(|_| "backup_dir_unavailable".to_string())?;
    Ok(Some(path))
}

/// P11-4② 凭据备份白名单：只拷密文与注册表；白名单之外的任何文件
/// （含未来的未知新增）一律不拷——防御性保证明文 token/私钥不进备份目录。
/// 各条目内容：accounts.json = 非敏感账号注册表；profiles = DPAPI 凭据包
/// （TRVCKN01 密文容器）；renewals = 续期日志（manifest 仅含哈希与阶段，
/// bundle.before 为写回前密文备份）；refresh-rescue = 救援标记（仅 sha256
/// 摘要与时间戳）；retired = 退役凭据包（密文）。
const CREDENTIAL_BACKUP_ENTRIES: &[&str] = &[
    "accounts.json",
    "profiles",
    "renewals",
    "refresh-rescue",
    "retired",
];

/// P11-4②：拷贝签到凭据材料白名单到备份目录 `credentials-{stamp}/` 子目录。
///
/// 只拷密文与注册表，任何明文 token/私钥不得出现；材料根不存在（从未登录）
/// 时不创建子目录直接成功（返回 false）。返回是否实际创建了凭据子目录。
fn copy_credentials_backup(
    material_root: &Path,
    backup_dir: &Path,
    stamp: u64,
) -> Result<bool, String> {
    if !material_root.is_dir() {
        return Ok(false);
    }
    let target_root = backup_dir.join(format!("credentials-{stamp}"));
    if target_root.exists() {
        // 同戳子目录已存在：拒绝而非覆盖（与备份链 create_new 同纪律）。
        return Err("master_backup_credentials_exists".to_string());
    }
    std::fs::create_dir_all(&target_root)
        .map_err(|_| "master_backup_credentials_failed".to_string())?;
    for entry in CREDENTIAL_BACKUP_ENTRIES {
        let source = material_root.join(entry);
        let target = target_root.join(entry);
        match std::fs::symlink_metadata(&source) {
            Ok(meta) if meta.is_dir() => copy_credentials_dir(&source, &target)?,
            Ok(meta) if meta.is_file() => {
                std::fs::copy(&source, &target)
                    .map_err(|_| "master_backup_credentials_failed".to_string())?;
            }
            // 符号链接/重解析点等非常规对象一律拒绝（fail-closed：不猜测内容）。
            Ok(_) => return Err("master_backup_credentials_failed".to_string()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(_) => return Err("master_backup_credentials_failed".to_string()),
        }
    }
    Ok(true)
}

/// 递归复制凭据材料子目录（仅常规文件与子目录；符号链接等拒绝）。
fn copy_credentials_dir(source: &Path, target: &Path) -> Result<(), String> {
    std::fs::create_dir_all(target).map_err(|_| "master_backup_credentials_failed".to_string())?;
    let entries =
        std::fs::read_dir(source).map_err(|_| "master_backup_credentials_failed".to_string())?;
    for entry in entries {
        let entry = entry.map_err(|_| "master_backup_credentials_failed".to_string())?;
        let child_meta = std::fs::symlink_metadata(entry.path())
            .map_err(|_| "master_backup_credentials_failed".to_string())?;
        let child_target = target.join(entry.file_name());
        if child_meta.is_dir() {
            copy_credentials_dir(&entry.path(), &child_target)?;
        } else if child_meta.is_file() {
            std::fs::copy(entry.path(), &child_target)
                .map_err(|_| "master_backup_credentials_failed".to_string())?;
        } else {
            return Err("master_backup_credentials_failed".to_string());
        }
    }
    Ok(())
}

/// P5-9 备份保留策略：读配置，启用时裁剪备份链（静默容错）。
///
/// 配置读取失败（损坏/IO 错误）不裁剪——保留策略失效宁可多留不可误删；
/// 裁剪本身失败也静默（下次备份生成时再试），不阻断任何主流程。
/// P11-4①：只裁剪当前活动备份目录下的链，位置变更前的旧目录不碰。
pub(crate) fn prune_backups_if_enabled(db_path: &Path, storage_root: &Path) {
    if storage_root.as_os_str().is_empty() {
        return;
    }
    let active_dir = resolve_master_backup_dir(storage_root, db_path);
    prune_backups_if_enabled_in(db_path, storage_root, &active_dir);
}

/// P11-4①：库作用域裁剪（P11-5⑤ 泛化）——裁剪指定活动备份目录内的
/// `{db 名}.switch-bak-*` 链。目录由调用方按库解析（主库 = 覆盖根或
/// db 同目录；副环境 = 覆盖目录下环境子目录或 db 同目录），互不越界。
pub(crate) fn prune_backups_if_enabled_in(db_path: &Path, storage_root: &Path, active_dir: &Path) {
    if storage_root.as_os_str().is_empty() {
        return;
    }
    let store = BackupRetentionStore::new(storage_root);
    if let Ok(settings) = store.load() {
        if settings.enabled {
            if let Some(db_name) = db_path.file_name().and_then(|name| name.to_str()) {
                prune_master_backups(active_dir, db_name, settings.keep);
            }
        }
    }
}

/// P11-4④：读取备份保留配置的保留份数（备份链展示用，fail-soft）。
/// 未配置返回默认 5；配置损坏或 IO 失败同样回退默认 5（展示字段不阻塞）。
fn backup_keep_policy(storage_root: &Path) -> u32 {
    BackupRetentionStore::new(storage_root)
        .load()
        .map(|settings| settings.keep)
        .unwrap_or(BackupRetentionSettings::default().keep)
}

/// P5-9：读取备份保留设置（设置页备份分区数据源）。
#[tauri::command]
pub(crate) async fn get_backup_retention(
    state: tauri::State<'_, AppState>,
) -> Result<BackupRetentionDto, String> {
    ensure_real_preview(&state)?;
    let storage_root = state.storage_root.clone();
    run_blocking("backup_retention_join_failed", move || {
        let settings = BackupRetentionStore::new(&storage_root)
            .load()
            .map_err(|_| "backup_retention_invalid".to_string())?;
        Ok(BackupRetentionDto {
            enabled: settings.enabled,
            keep: settings.keep,
        })
    })
    .await
}

/// P5-9：保存备份保留设置（原子写，立即生效于下一次备份生成）。
/// P11-4①：读旧配置合并保存——备份位置（backup_dir）不属于本命令的
/// 编辑范围，整体覆盖会把它抹掉。
#[tauri::command]
pub(crate) async fn set_backup_retention(
    enabled: bool,
    keep: u32,
    state: tauri::State<'_, AppState>,
) -> Result<(), String> {
    ensure_real_preview(&state)?;
    let storage_root = state.storage_root.clone();
    run_blocking("backup_retention_join_failed", move || {
        let store = BackupRetentionStore::new(&storage_root);
        store
            .update(|settings| {
                settings.enabled = enabled;
                settings.keep = keep;
            })
            .map_err(|error| match error {
                BackupRetentionError::Invalid => "backup_retention_invalid".to_string(),
                BackupRetentionError::Io => "backup_retention_save_failed".to_string(),
            })
    })
    .await
}

/// P11-4①：读取备份位置覆盖配置（None = 主库同目录默认行为）。
/// 配置损坏时报错（fail-closed 展示层）：让用户看到配置坏了，而不是
/// 静默当作默认位置。
#[tauri::command]
pub(crate) async fn get_backup_dir(
    state: tauri::State<'_, AppState>,
) -> Result<Option<String>, String> {
    ensure_real_preview(&state)?;
    let storage_root = state.storage_root.clone();
    run_blocking("backup_retention_join_failed", move || {
        BackupRetentionStore::new(&storage_root)
            .load()
            .map(|settings| settings.backup_dir)
            .map_err(|_| "backup_retention_invalid".to_string())
    })
    .await
}

/// P11-4①：设置备份位置。Some = 自定义目录（校验：绝对路径、长度上限、
/// 可创建、不落签到材料根）；None = 恢复默认（主库同目录）。
/// 立即生效于下一次备份生成；既有备份不移动（如需搬迁先调
/// migrate_master_backups 再调本命令）。
#[tauri::command]
pub(crate) async fn set_backup_dir(
    backup_dir: Option<String>,
    state: tauri::State<'_, AppState>,
) -> Result<(), String> {
    ensure_real_preview(&state)?;
    let storage_root = state.storage_root.clone();
    run_blocking("backup_retention_join_failed", move || {
        let storage = PathBuf::from(&storage_root);
        let normalized = validate_backup_dir(backup_dir.as_deref(), &storage)?;
        let store = BackupRetentionStore::new(&storage);
        store
            .update(|settings| {
                settings.backup_dir = normalized.map(|path| path.display().to_string());
            })
            .map_err(|error| match error {
                BackupRetentionError::Invalid => "backup_retention_invalid".to_string(),
                BackupRetentionError::Io => "backup_retention_save_failed".to_string(),
            })
    })
    .await
}

/// P11-4① 迁移回执 DTO（设置页展示）。
#[derive(Clone, Serialize)]
pub(crate) struct BackupMigrationDto {
    /// 本次确认落位新目录的文件数（含幂等已存在项）。
    migrated_files: usize,
    /// 失败清单（用户可读原因；出现即中止位置切换，由用户排查后重试）。
    failures: Vec<BackupMigrationFailureDto>,
}

/// 迁移失败单条记录（file_name 为备份目录内相对名，不含绝对路径）。
#[derive(Clone, Serialize)]
struct BackupMigrationFailureDto {
    file_name: String,
    reason: String,
}

/// P11-4①：把当前备份目录的全部备份产物**复制**到 targetDir（不移动、
/// 原位保留——铁律：不自动删快照），逐文件 sha256 校验；部分失败时已复制
/// 的保留并返回失败清单，不回滚。
///
/// 调用顺序约束：须在 set_backup_dir 之前调用（此刻活动目录仍是旧位置，
/// 枚举到的才是待迁备份）；重复调用幂等（目标已存在且内容一致计入成功）。
#[tauri::command]
pub(crate) async fn migrate_master_backups(
    target_dir: String,
    state: tauri::State<'_, AppState>,
) -> Result<BackupMigrationDto, String> {
    ensure_real_preview(&state)?;
    let storage_root = state.storage_root.clone();
    let recovery_root = PathBuf::from(&state.recovery_root);
    run_blocking("master_backup_migrate_join_failed", move || {
        let storage = PathBuf::from(&storage_root);
        let target = validate_backup_dir(Some(&target_dir), &storage)?
            .ok_or_else(|| "backup_dir_invalid".to_string())?;
        let master_dir =
            master_data_dir().map_err(|_| "environment_registry_invalid".to_string())?;
        let _lease = OperationLease::acquire_shared_for_path(&recovery_root, &master_dir)
            .map_err(|error| format!("operation_lease_unavailable:{error}"))?;
        let db_path = master_database_path(&master_dir);
        let db_name = master_backup_db_name(&db_path)?;
        let source = resolve_master_backup_dir(&storage, &db_path);
        let outcome: BackupMigrationOutcome =
            migrate_master_backups_inner(&source, &target, &db_name)
                .map_err(|_| "master_backup_migrate_failed".to_string())?;
        Ok(BackupMigrationDto {
            migrated_files: outcome.migrated_files,
            failures: outcome
                .failures
                .into_iter()
                .map(|failure| BackupMigrationFailureDto {
                    file_name: failure.file_name,
                    reason: failure.reason,
                })
                .collect(),
        })
    })
    .await
}

/// P5-9 备份保留设置 DTO（设置页展示与编辑）。
#[derive(Clone, Serialize)]
pub(crate) struct BackupRetentionDto {
    enabled: bool,
    keep: u32,
}

// ===== P5-7 主库两层校验（只读排查）：台账核对 + 备份对比 =====

/// P5-7 台账核对异常行（前端按类型分组展示，detail 直接可读）。
#[derive(Clone, Serialize)]
struct LedgerIssueDto {
    /// missing / owner_mismatch / message_loss / stale_leg。
    kind: String,
    /// 技术标识收悬浮提示（界面主信息只用 detail）。
    session_id: String,
    detail: String,
}

/// P5-7 第一层结果：接力台账核对。
#[derive(Clone, Serialize)]
struct LedgerVerificationDto {
    /// ready / no_ledger（从未切号，正常）/ invalid / read_failed / no_master_data。
    status: String,
    /// 核对过的当前腿数。
    checked_count: u32,
    /// 被后续换腿接走的中间腿数（接力链衔接证据）。
    relayed_away_count: u32,
    issues: Vec<LedgerIssueDto>,
}

/// P5-7 丢失候选会话行（备份里还在、现在没有）。
#[derive(Clone, Serialize)]
struct MissingSessionDto {
    /// 备份库中的会话标题（无标题时前端降级为「未命名会话」）。
    title: Option<String>,
    /// 备份时该会话的消息数。
    message_count: i64,
}

/// P5-7 第二层结果：备份对比。
#[derive(Clone, Serialize)]
struct BackupComparisonDto {
    /// ready / no_backups / backup_missing / read_failed / no_master_data。
    status: String,
    /// 实际使用的基准备份时间戳（供前端高亮当前选择）。
    backup_stamp: Option<u64>,
    /// 备份链全部可选基准点（时间戳倒序，前端切换用）。
    backup_stamps: Vec<u64>,
    /// 两边都有的会话数。
    common_count: u32,
    /// 备份有、现在没有、台账解释为换腿（正常接力）的会话数。
    relayed_away_count: u32,
    /// 现在有、备份没有（新增，正常）。
    added_count: u32,
    /// 丢失候选（只报告；人工恢复指引由前端复用设置页文案）。
    missing: Vec<MissingSessionDto>,
}

/// P5-7 两层校验合并结果（「数据校验」tab 数据源，进 tab 自动执行）。
#[derive(Clone, Serialize)]
pub(crate) struct MasterVerificationDto {
    ledger: LedgerVerificationDto,
    backup: BackupComparisonDto,
}

/// P5-7：主库两层校验（只读排查，发现异常只报告不修复）。
///
/// `backup_stamp` 为 None 时用最新备份做对比基准；指定链上其他备份点可
/// 排查更早发生的丢失。当前库/备份库均经隔离三件套副本只读打开。
/// P11-5 收官：`library_id` 为库实例引用（缺省主库；台账与备份链均按
/// 库作用域解析——副环境读自己的台账与链，不混入主库轨迹）。
#[tauri::command]
pub(crate) async fn get_master_verification(
    backup_stamp: Option<u64>,
    library_id: Option<String>,
    state: tauri::State<'_, AppState>,
) -> Result<MasterVerificationDto, String> {
    ensure_real_preview(&state)?;
    if state.source_raw_key.is_empty() {
        return Err("source_key_unavailable".to_string());
    }
    let storage_root = PathBuf::from(&state.storage_root);
    let raw_key = state.source_raw_key.clone();
    run_blocking("master_verification_join_failed", move || -> Result<MasterVerificationDto, String> {
        use traesync_infrastructure::master_verification::{
            compare_with_backup, current_master_db_path, read_session_facts, verify_relay_ledger,
        };

        // 库作用域解析：当前库 db、台账根、活动备份目录全部按库路由。
        let ctx = resolve_library_context(library_id.as_deref(), &storage_root)?;
        let db_path = current_master_db_path(&ctx.dir);
        if !db_path.is_file() {
            // 库从未启动：两层都无对象，前端引导文案。
            return Ok(MasterVerificationDto {
                ledger: LedgerVerificationDto {
                    status: "no_master_data".to_string(),
                    checked_count: 0,
                    relayed_away_count: 0,
                    issues: Vec::new(),
                },
                backup: BackupComparisonDto {
                    status: "no_master_data".to_string(),
                    backup_stamp: None,
                    backup_stamps: Vec::new(),
                    common_count: 0,
                    relayed_away_count: 0,
                    added_count: 0,
                    missing: Vec::new(),
                },
            });
        }

        // 台账：文件不存在 = 从未切号（no_ledger，正常空态）；损坏 = 报错。
        // P11-5⑤：台账按库隔离（relay_ledger_root 路由）。
        let ledger_entries =
            match RelayLedger::new(relay_ledger_root(&storage_root, &ctx.env_id)).load() {
                Ok(entries) => entries,
                Err(
                    traesync_infrastructure::relay_ledger::RelayLedgerError::Io,
                ) => return Err("relay_ledger_unavailable".to_string()),
                Err(_) => return Err("relay_ledger_invalid".to_string()),
        };

        // 第一层：当前库会话事实 + 台账核对。
        let current_facts = match read_session_facts(&db_path, &raw_key) {
            Some(facts) => facts,
            None => {
                return Ok(MasterVerificationDto {
                    ledger: LedgerVerificationDto {
                        status: "read_failed".to_string(),
                        checked_count: 0,
                        relayed_away_count: 0,
                        issues: Vec::new(),
                    },
                    backup: BackupComparisonDto {
                        status: "read_failed".to_string(),
                        backup_stamp: None,
                        backup_stamps: Vec::new(),
                        common_count: 0,
                        relayed_away_count: 0,
                        added_count: 0,
                        missing: Vec::new(),
                    },
                })
            }
        };
        let ledger_report = verify_relay_ledger(&current_facts, &ledger_entries);
        let ledger_dto = LedgerVerificationDto {
            status: if ledger_entries.is_empty() {
                "no_ledger".to_string()
            } else {
                "ready".to_string()
            },
            checked_count: ledger_report.checked_count as u32,
            relayed_away_count: ledger_report.relayed_away_count as u32,
            issues: ledger_report
                .issues
                .iter()
                .map(|issue| {
                    let kind = match issue.kind {
                        traesync_infrastructure::master_verification::LedgerIssueKind::MissingSession => "missing",
                        traesync_infrastructure::master_verification::LedgerIssueKind::OwnerMismatch => "owner_mismatch",
                        traesync_infrastructure::master_verification::LedgerIssueKind::MessageLoss => "message_loss",
                        traesync_infrastructure::master_verification::LedgerIssueKind::StaleLeg => "stale_leg",
                    };
                    LedgerIssueDto {
                        kind: kind.to_string(),
                        session_id: issue.session_id.clone(),
                        detail: issue.detail.clone(),
                    }
                })
                .collect(),
        };

        // 第二层：备份链选基准（None = 最新）。P11-4①/P11-5⑤：从当前库
        // 作用域活动备份目录取（与备份写入点同口径，副环境不混主库链）。
        let active_dir = resolve_library_backup_dir(&storage_root, &db_path, &ctx.env_id);
        let db_name = master_backup_db_name(&db_path)?;
        let chain = list_master_backups_in(&active_dir, &db_name);
        let backup_stamps: Vec<u64> = chain
            .iter()
            .map(|entry| entry.stamp_unix_seconds)
            .collect();
        let backup_dto = if chain.is_empty() {
            BackupComparisonDto {
                status: "no_backups".to_string(),
                backup_stamp: None,
                backup_stamps,
                common_count: 0,
                relayed_away_count: 0,
                added_count: 0,
                missing: Vec::new(),
            }
        } else {
            let chosen_stamp = backup_stamp
                .filter(|stamp| backup_stamps.contains(stamp))
                .unwrap_or(backup_stamps[0]);
            // P11-4①：基准备份按活动目录解析（不再假定与 db 同目录）。
            // P16-T1 双格式：旧未压缩优先；新压缩 .zst 解压为临时件供
            // SQLCipher 打开（解压落在活动备份目录——已迁软件本体旁，
            // 不落 C 盘 TEMP），用完即删。
            let Some((chosen_path, temporary)) =
                materialize_backup_for_read(&active_dir, &db_name, chosen_stamp)
            else {
                return Ok(MasterVerificationDto {
                    ledger: ledger_dto,
                    backup: BackupComparisonDto {
                        status: "backup_missing".to_string(),
                        backup_stamp: None,
                        backup_stamps,
                        common_count: 0,
                        relayed_away_count: 0,
                        added_count: 0,
                        missing: Vec::new(),
                    },
                });
            };
            let read_result = read_session_facts(&chosen_path, &raw_key);
            // 临时解压件用完即删（无论读取成败）。
            if let Some(temporary) = temporary {
                let _ = std::fs::remove_file(&temporary);
            }
            match read_result {
                Some(backup_facts) => {
                    let report =
                        compare_with_backup(&current_facts, &backup_facts, chosen_stamp, &ledger_entries);
                    BackupComparisonDto {
                        status: "ready".to_string(),
                        backup_stamp: Some(report.backup_stamp),
                        backup_stamps,
                        common_count: report.common_count as u32,
                        relayed_away_count: report.relayed_away_count as u32,
                        added_count: report.added_count as u32,
                        missing: report
                            .missing
                            .iter()
                            .map(|session| MissingSessionDto {
                                title: session.title.clone(),
                                message_count: session.message_count,
                            })
                            .collect(),
                    }
                }
                None => BackupComparisonDto {
                    status: "read_failed".to_string(),
                    backup_stamp: None,
                    backup_stamps,
                    common_count: 0,
                    relayed_away_count: 0,
                    added_count: 0,
                    missing: Vec::new(),
                },
            }
        };

        Ok(MasterVerificationDto {
            ledger: ledger_dto,
            backup: backup_dto,
        })
    })
    .await
}

// ===== P5-8a 会话归档/恢复/真实删除（ADR-0022 hidden_status 借用）=====

/// 归档草稿一次性提交回执；`failed` 表示库已更新但实例待用户启动，
/// `not_requested` 表示该产品的客户端由用户自行管理。
#[derive(Clone, Debug, Serialize)]
pub(crate) struct MasterArchiveApplyResultDto {
    archived_sessions: u32,
    restored_sessions: u32,
    relaunch_outcome: String,
}

/// P5-8a：提交归档草稿（关实例 → 备份 → 单事务 → 重启）。
///
/// 前端可以连续调整归档/恢复集合，只有点击“应用”才进入这里；提交失败时
/// 旧草稿由前端保留。数据库事务已提交但重启失败时返回成功回执与 failed
/// 状态，避免用户再次点击造成重复改写。
/// P13-3（ADR-0033）：归档/恢复经 `ensure_archive_writable` 对 trae_cn 库
/// 放行（CN 分支无实例托管编排）；删除/合并项目等其余写命令仍走
/// `ensure_writable` 被拒（`library_read_only`）。
#[tauri::command]
pub(crate) async fn apply_master_archive_changes(
    archive_session_ids: Vec<String>,
    restore_session_ids: Vec<String>,
    library_id: Option<String>,
    state: tauri::State<'_, AppState>,
) -> Result<MasterArchiveApplyResultDto, String> {
    crate::product_operations::ensure_library_ready(
        "library.archive",
        library_id.clone(),
        Path::new(&state.storage_root),
        state.runtime_mode == RuntimeMode::RealReadPreview,
    )?;
    ensure_real_preview(&state)?;
    if state.source_raw_key.is_empty() {
        return Err("source_key_unavailable".to_string());
    }
    let raw_key = state.source_raw_key.clone();
    let material_root = checkin_material_root(&state)?;
    let storage_root = PathBuf::from(&state.storage_root);
    let recovery_root = PathBuf::from(&state.recovery_root);
    run_blocking("master_archive_join_failed", move || {
        let (ctx, lifecycle_lease) =
            prepare_library_write_context(&recovery_root, &storage_root, library_id.as_deref())?;
        let close_instance = |ctx: &LibraryContext| ctx.close_instance().map(|_| ());
        let relaunch_instance = |ctx: &LibraryContext, title: &str| {
            ctx.relaunch(title)
                .map(|relaunch| relaunch.outcome.to_string())
        };
        let ensure_cn_client_stopped = |ctx: &LibraryContext| {
            crate::process_guard::ensure_product_stopped(
                TraeProduct::TraeCn,
                &ctx.dir,
                "trae_cn_archive_client_running",
            )
        };
        apply_library_archive_for_context(
            &recovery_root,
            &storage_root,
            &material_root,
            &raw_key,
            ctx,
            lifecycle_lease,
            &archive_session_ids,
            &restore_session_ids,
            &close_instance,
            &relaunch_instance,
            &ensure_cn_client_stopped,
            &|| {},
        )
    })
    .await
    .inspect(|_| {
        // P15-11：归档/恢复提交改写主库成功后作废体检预聚合缓存。
        traesync_infrastructure::master_checkup::invalidate_master_checkup_cache();
    })
}

/// 单库归档写入主体：先拿真实数据目录租约，再解析账号并执行全部副作用。
/// 测试可传入临时 LibraryContext；生产调用仍由上层按注册表解析官方目录。
fn apply_library_archive_for_context(
    recovery_root: &Path,
    storage_root: &Path,
    material_root: &Path,
    raw_key: &str,
    ctx: LibraryContext,
    lifecycle_lease: Option<OperationLease>,
    archive_session_ids: &[String],
    restore_session_ids: &[String],
    close_instance: &dyn Fn(
        &LibraryContext,
    ) -> Result<(), trae_instance_module::TraeInstanceCloseError>,
    relaunch_instance: &dyn Fn(&LibraryContext, &str) -> Result<String, String>,
    ensure_cn_client_stopped: &dyn Fn(&LibraryContext) -> Result<(), String>,
    after_commit: &dyn Fn(),
) -> Result<MasterArchiveApplyResultDto, String> {
    let _lease = acquire_library_data_lease(recovery_root, &ctx)?;
    drop(lifecycle_lease);
    ctx.ensure_archive_writable()?;
    let current_account = require_library_account(material_root, storage_root, &ctx)?;

    // P13-3：CN 客户端由用户管理；不做工具托管的关闭与重启。
    if ctx.env_id == TRAE_CN_LIBRARY_ID {
        let ensure_client_stopped = || ensure_cn_client_stopped(&ctx);
        let result = run_trae_cn_archive_apply(
            storage_root,
            raw_key,
            &ctx,
            &current_account.account_id,
            archive_session_ids,
            restore_session_ids,
            &ensure_client_stopped,
        );
        if result.is_ok() {
            after_commit();
        }
        return result;
    }

    let db_path = master_database_path(&ctx.dir);
    // 重启窗口标题：主库 = 当前账号名（P5-8a 口径）；副环境 = 环境名。
    let window_title = if ctx.is_master() {
        current_account
            .display_name
            .clone()
            .unwrap_or_else(|| current_account.screen_name.clone())
    } else {
        ctx.env_name.clone()
    };

    if db_path.is_file()
        && master_db_activity_detected(&db_path, std::time::Duration::from_millis(800))
    {
        return Err("master_archive_busy".to_string());
    }

    close_instance(&ctx).map_err(|error| {
        crate::process_guard::close_failure_code(error, "master_archive_close_failed")
    })?;

    let restart_after_failure = || {
        let _ = relaunch_instance(&ctx, &window_title);
    };
    // P11-4①/P11-5⑤：备份落在当前库作用域，副环境与主库链隔离。
    let backup_dir = resolve_library_backup_dir(storage_root, &db_path, &ctx.env_id);
    let _backup_path = match backup_master_trio_to(&db_path, Some(&backup_dir)) {
        Ok(path) => path,
        Err(error) => {
            restart_after_failure();
            return Err(match error {
                MasterHandoverError::DbUnavailable => "master_archive_no_data".to_string(),
                _ => "master_archive_backup_failed".to_string(),
            });
        }
    };
    prune_backups_if_enabled_in(&db_path, storage_root, &backup_dir);

    let outcome = match apply_master_archive_changes_inner(
        &ctx.dir,
        raw_key,
        archive_session_ids,
        restore_session_ids,
        &current_account.account_id,
    ) {
        Ok(outcome) => outcome,
        Err(error) => {
            restart_after_failure();
            return Err(map_master_archive_error(error));
        }
    };
    after_commit();

    let relaunch_outcome = match relaunch_instance(&ctx, &window_title) {
        Ok(outcome) => outcome,
        Err(_) => "failed".to_string(),
    };
    Ok(MasterArchiveApplyResultDto {
        archived_sessions: outcome.archived_sessions as u32,
        restored_sessions: outcome.restored_sessions as u32,
        relaunch_outcome,
    })
}

/// P13-3 CN 归档/恢复编排主体（阻塞上下文；客户端运行检测可注入供单测）。
///
/// 与主库/副环境路径的差异：CN 实例不经工具托管——无 close/relaunch 编排，
/// 客户端运行中拒绝（fail-closed，P13-2 归并同款），由用户关闭后重试。
/// 备份先行（家族铁律）：备份源 = 活动库路径（ctx.dir 派生，不写死主库），
/// 写入 CN 库作用域活动备份目录（backup_dir 覆盖时经 env_id 隔离）。
/// DB 层错误沿用既有 `master_archive_*` 族（同一事务原语与词表共享）；
/// CN 生命周期拒绝使用 `trae_cn_archive_*` 专属码。
fn run_trae_cn_archive_apply(
    storage_root: &Path,
    raw_key: &str,
    ctx: &LibraryContext,
    account_id: &str,
    archive_session_ids: &[String],
    restore_session_ids: &[String],
    ensure_client_stopped: &dyn Fn() -> Result<(), String>,
) -> Result<MasterArchiveApplyResultDto, String> {
    // 双重守卫：命令层已校验，此处保持 inner 可独立调用（fail-closed）。
    ctx.ensure_archive_writable()?;
    let db_path = master_database_path(&ctx.dir);
    if !db_path.is_file() {
        return Err("master_archive_no_data".to_string());
    }
    // Trae CN 客户端运行中拒绝：归档写入需要静止一致的库做备份与事务。
    ensure_client_stopped()?;
    // 生成中拒绝（WAL 活动 = 官方客户端正在写库）。
    if master_db_activity_detected(&db_path, std::time::Duration::from_millis(800)) {
        return Err("trae_cn_archive_busy".to_string());
    }
    // 备份先行（ADR-0018 家族铁律；写入 CN 库作用域活动备份目录）。
    let backup_dir = resolve_library_backup_dir(storage_root, &db_path, &ctx.env_id);
    if let Err(error) = backup_master_trio_to(&db_path, Some(&backup_dir)) {
        return Err(match error {
            MasterHandoverError::DbUnavailable => "master_archive_no_data".to_string(),
            _ => "master_archive_backup_failed".to_string(),
        });
    }
    prune_backups_if_enabled_in(&db_path, storage_root, &backup_dir);
    let outcome = apply_master_archive_changes_inner(
        &ctx.dir,
        raw_key,
        archive_session_ids,
        restore_session_ids,
        account_id,
    )
    .map_err(map_master_archive_error)?;
    // CN 无重启编排：实例生命周期交还用户，不伪报本命令已启动客户端。
    Ok(MasterArchiveApplyResultDto {
        archived_sessions: outcome.archived_sessions as u32,
        restored_sessions: outcome.restored_sessions as u32,
        relaunch_outcome: "not_requested".to_string(),
    })
}

/// P5-8a 真实删除回执（规模供 UI 二次确认与结果展示）。
#[derive(Clone, Serialize)]
pub(crate) struct MasterDeleteResultDto {
    deleted_sessions: u32,
    /// 删除的消息行数（元数据 + 内容三表随行）。
    deleted_messages: u32,
    /// 删除后变空壳而被清理的项目行数。
    removed_projects: u32,
    /// 删除前自动创建的备份路径（人工恢复定位）。
    backup_path: String,
}

/// P5-8a：真实删除会话（破坏性：消息三表 + 会话 + 空壳项目行，单事务）。
///
/// 纪律与切号/手动备份一致：主库实例运行中拒绝（`master_delete_running`）；
/// 执行前自动创建 `.switch-bak-*` 备份（ADR-0018 铁律，备份失败不删除）。
/// `library_id` 为库实例引用（ADR-0025，缺省主库）。
#[tauri::command]
pub(crate) async fn delete_master_sessions(
    session_ids: Vec<String>,
    library_id: Option<String>,
    state: tauri::State<'_, AppState>,
) -> Result<MasterDeleteResultDto, String> {
    crate::product_operations::ensure_library_ready(
        "library.delete",
        library_id.clone(),
        Path::new(&state.storage_root),
        state.runtime_mode == RuntimeMode::RealReadPreview,
    )?;
    ensure_real_preview(&state)?;
    if state.source_raw_key.is_empty() {
        return Err("source_key_unavailable".to_string());
    }
    let raw_key = state.source_raw_key.clone();
    let material_root = checkin_material_root(&state)?;
    let storage_root = PathBuf::from(&state.storage_root);
    let recovery_root = PathBuf::from(&state.recovery_root);
    // P5-9：删除前备份后的保留策略清理（闭包外拷出，State 引用不进闭包）。
    let storage_root_for_prune = state.storage_root.clone();
    run_blocking("master_archive_join_failed", move || {
        let (ctx, _lease) =
            acquire_library_write_lease(&recovery_root, &storage_root, library_id.as_deref())?;
        ctx.ensure_writable()?;
        let current_account = require_library_account(&material_root, &storage_root, &ctx)?;
        // P15-6：删除守卫针对融合版实例（与目录产品同源）。
        crate::process_guard::ensure_product_stopped(
            TraeProduct::TraeCn,
            &ctx.dir,
            "master_delete_running",
        )?;
        // P16-T1：删除内置备份写入库作用域活动备份目录（新默认 = exe 旁
        // backups/，不再落客户端目录）；裁剪目标同源。
        let db_path_for_backup = master_database_path(&ctx.dir);
        let backup_dir = resolve_library_backup_dir(&storage_root, &db_path_for_backup, &ctx.env_id);
        let result = delete_master_sessions_with_backup_dir(
            &ctx.dir,
            &raw_key,
            &session_ids,
            &current_account.account_id,
            Some(&backup_dir),
        )
        .map(|outcome: MasterDeleteOutcome| MasterDeleteResultDto {
            deleted_sessions: outcome.deleted_sessions as u32,
            deleted_messages: outcome.deleted_messages as u32,
            removed_projects: outcome.removed_projects as u32,
            backup_path: outcome.backup_path,
        })
        .map_err(map_master_archive_error);
        // P5-9：删除链已生成新备份，按保留策略清理旧备份（静默容错）。
        // P11-5⑤：裁剪目标 = 库作用域活动目录（副环境不误伤主库链）。
        if result.is_ok() {
            let db_path = master_database_path(&ctx.dir);
            let backup_dir = resolve_library_backup_dir(
                Path::new(&storage_root_for_prune),
                &db_path,
                &ctx.env_id,
            );
            prune_backups_if_enabled_in(&db_path, Path::new(&storage_root_for_prune), &backup_dir);
        }
        result
    })
    .await
    .inspect(|_| {
        // P15-11：删除会话改写主库成功后作废体检预聚合缓存。
        traesync_infrastructure::master_checkup::invalidate_master_checkup_cache();
    })
}

/// P5-8c 分组合并回执（规模供 UI 结果展示）。
#[derive(Clone, Serialize)]
pub(crate) struct MasterMergeResultDto {
    /// 改挂到目标分组的会话数。
    moved_sessions: u32,
    /// 被清理的空壳源分组行数。
    removed_projects: u32,
    /// 合并前自动创建的备份路径（人工恢复定位）。
    backup_path: String,
}

/// P5-8c：合并分组（源分组全部会话并入目标分组，空壳源行清理）。
///
/// 批量迁移属破坏性操作：主库实例运行中拒绝（`master_merge_running`，
/// 与删除/切号同纪律）；执行前自动创建 `.switch-bak-*` 备份
/// （ADR-0018 铁律，备份失败合并不执行）。`library_id` 为库实例引用
/// （ADR-0025，缺省主库）。
#[tauri::command]
pub(crate) async fn merge_master_projects(
    source_project_ids: Vec<String>,
    target_project_id: String,
    library_id: Option<String>,
    state: tauri::State<'_, AppState>,
) -> Result<MasterMergeResultDto, String> {
    crate::product_operations::ensure_library_ready(
        "library.work_project_merge",
        library_id.clone(),
        Path::new(&state.storage_root),
        state.runtime_mode == RuntimeMode::RealReadPreview,
    )?;
    ensure_real_preview(&state)?;
    if state.source_raw_key.is_empty() {
        return Err("source_key_unavailable".to_string());
    }
    let raw_key = state.source_raw_key.clone();
    let material_root = checkin_material_root(&state)?;
    let storage_root = PathBuf::from(&state.storage_root);
    let recovery_root = PathBuf::from(&state.recovery_root);
    // P5-9：合并前备份后的保留策略清理（闭包外拷出，State 引用不进闭包）。
    let storage_root_for_prune = state.storage_root.clone();
    run_blocking("master_archive_join_failed", move || {
        let (ctx, lifecycle_lease) =
            prepare_library_write_context(&recovery_root, &storage_root, library_id.as_deref())?;
        let ensure_stopped = |ctx: &LibraryContext| {
            // P15-6：合并守卫针对融合版实例（与目录产品同源）。
            crate::process_guard::ensure_product_stopped(
                TraeProduct::TraeCn,
                &ctx.dir,
                "master_merge_running",
            )
        };
        run_master_project_merge_with_lease(
            &recovery_root,
            &storage_root,
            Path::new(&storage_root_for_prune),
            &material_root,
            &raw_key,
            ctx,
            lifecycle_lease,
            &source_project_ids,
            &target_project_id,
            &ensure_stopped,
            &|| {},
        )
    })
    .await
    .inspect(|_| {
        // P15-11：主库归并改写成功后作废体检预聚合缓存。
        traesync_infrastructure::master_checkup::invalidate_master_checkup_cache();
    })
}

/// Work 项目归并生产/合成库共用：目录租约覆盖账号解析、进程检查、备份与数据库写入。
fn run_master_project_merge_with_lease(
    recovery_root: &Path,
    storage_root: &Path,
    storage_root_for_prune: &Path,
    material_root: &Path,
    raw_key: &str,
    ctx: LibraryContext,
    lifecycle_lease: Option<OperationLease>,
    source_project_ids: &[String],
    target_project_id: &str,
    ensure_stopped: &dyn Fn(&LibraryContext) -> Result<(), String>,
    after_commit: &dyn Fn(),
) -> Result<MasterMergeResultDto, String> {
    let _data_lease = acquire_library_data_lease(recovery_root, &ctx)?;
    drop(lifecycle_lease);
    ctx.ensure_writable()?;
    let current_account = require_library_account(material_root, storage_root, &ctx)?;
    ensure_stopped(&ctx)?;
    // P16-T1：合并内置备份写入库作用域活动备份目录（新默认 = exe 旁
    // backups/，不再落客户端目录）。
    let backup_dir = resolve_library_backup_dir(
        storage_root,
        &master_database_path(&ctx.dir),
        &ctx.env_id,
    );
    let result = merge_master_projects_with_backup_dir(
        &ctx.dir,
        raw_key,
        source_project_ids,
        target_project_id,
        &current_account.account_id,
        Some(&backup_dir),
    )
    .map(|outcome: MasterMergeOutcome| MasterMergeResultDto {
        moved_sessions: outcome.moved_sessions as u32,
        removed_projects: outcome.removed_projects as u32,
        backup_path: outcome.backup_path,
    })
    .map_err(map_master_archive_error);
    if result.is_ok() {
        let db_path = master_database_path(&ctx.dir);
        let backup_dir = resolve_library_backup_dir(storage_root_for_prune, &db_path, &ctx.env_id);
        prune_backups_if_enabled_in(&db_path, storage_root_for_prune, &backup_dir);
        after_commit();
    }
    result
}

/// P3-2/P5-3 共用：infrastructure 消息条目 → 前端 DTO（内容按形态拆平）。
fn map_session_message_entries(
    entries: Vec<traesync_infrastructure::account_session_content::SessionMessageEntry>,
) -> Vec<SessionMessageDto> {
    use traesync_infrastructure::account_session_content::SessionMessageContent;
    entries
        .into_iter()
        .map(|entry| {
            let content = match entry.content {
                SessionMessageContent::Text(text) => SessionMessageContentDto {
                    kind: "text",
                    text,
                    step_count: 0,
                    thoughts: Vec::new(),
                },
                SessionMessageContent::TaskTrace {
                    step_count,
                    thoughts,
                } => SessionMessageContentDto {
                    kind: "task_trace",
                    text: String::new(),
                    step_count,
                    thoughts,
                },
            };
            SessionMessageDto {
                message_id: entry.message_id,
                role: entry.role,
                message_type: entry.message_type,
                // SessionMessageEntry 中 created_at 允许缺失时以 0 补位，保证前端 DTO 字段对齐。
                created_at_unix_seconds: entry.created_at_unix_seconds.unwrap_or(0),
                content,
            }
        })
        .collect()
}

/// P5-3 接力台账条目（前端轨迹徽章数据源；账号显示名由注册表反查）。
#[derive(Clone, Serialize)]
pub(crate) struct RelayLedgerEntryDto {
    session_id: String,
    from_session_id: Option<String>,
    project_id: String,
    from_user_id: String,
    /// 交接前账号显示名（注册表反查；账号已移除时为 None，前端降级占位）。
    from_account_name: Option<String>,
    to_user_id: String,
    to_account_name: Option<String>,
    message_count_at_switch: i64,
    switched_at_unix_seconds: u64,
}

/// P5-3：读取接力台账（全部记录，前端按 session_id / from_session_id
/// 链回成完整接力轨迹）。台账损坏报错不静默重建（铁律）。
/// `library_id` 为库实例引用（ADR-0025，缺省主库）。P11-5⑤：台账按库
/// 隔离（relay_ledger_root）——主库读存储根 environments 目录原文件，
/// 副环境读 relay-ledgers/{env_id} 独立文件。
#[tauri::command]
pub(crate) async fn get_relay_ledger(
    library_id: Option<String>,
    state: tauri::State<'_, AppState>,
) -> Result<Vec<RelayLedgerEntryDto>, String> {
    ensure_real_preview(&state)?;
    let storage_root = PathBuf::from(&state.storage_root);
    // ADR-0025：库作用域校验——未知库直接拒绝，不落到默认库。
    let env_id = resolve_library_env_id(library_id.as_deref(), &storage_root)?;
    let material_root = checkin_material_root(&state)?;
    run_blocking("relay_ledger_join_failed", move || {
        let records = AccountRegistry::new(&material_root)
            .load()
            .map_err(|_| "checkin_registry_invalid".to_string())?;
        // user_id → 显示名（备注名优先，与账号页口径一致）。
        let name_by_user_id: std::collections::HashMap<&str, String> = records
            .iter()
            .map(|record| {
                (
                    record.account_id.as_str(),
                    record
                        .display_name
                        .clone()
                        .unwrap_or_else(|| record.screen_name.clone()),
                )
            })
            .collect();
        let entries = RelayLedger::new(relay_ledger_root(&storage_root, &env_id))
            .load()
            .map_err(|error| match error {
                traesync_infrastructure::relay_ledger::RelayLedgerError::Invalid => {
                    "relay_ledger_invalid"
                }
                _ => "relay_ledger_unavailable",
            })?;
        Ok(entries
            .into_iter()
            .map(|entry| RelayLedgerEntryDto {
                from_account_name: name_by_user_id.get(entry.from_user_id.as_str()).cloned(),
                to_account_name: name_by_user_id.get(entry.to_user_id.as_str()).cloned(),
                session_id: entry.session_id,
                from_session_id: entry.from_session_id,
                project_id: entry.project_id,
                from_user_id: entry.from_user_id,
                to_user_id: entry.to_user_id,
                message_count_at_switch: entry.message_count_at_switch,
                switched_at_unix_seconds: entry.switched_at_unix_seconds,
            })
            .collect())
    })
    .await
}

// ============================================================================
// P11-5⑤ 副环境全量补齐：库作用域解析 / 备份链隔离 / 环境台账 单测。
// 只测组合根的纯逻辑编排接缝；开库读写由 infrastructure 层各自覆盖。
// ============================================================================

#[cfg(test)]
mod library_scope_tests {
    use super::*;

    /// 测试夹具：storage_root + 一个已登记副环境。
    struct ScopeFixture {
        root: tempfile::TempDir,
        env_id: String,
    }

    impl ScopeFixture {
        fn new() -> Self {
            let root = tempfile::tempdir().unwrap();
            let record = EnvironmentRegistry::new(root.path())
                .create("测试环境")
                .unwrap();
            Self {
                env_id: record.env_id,
                root,
            }
        }
    }

    /// Library 写入口须把注册库上下文绑定到其实际副环境目录。
    #[test]
    fn secondary_library_write_lease_uses_its_registered_data_directory() {
        let fixture = ScopeFixture::new();
        let data_dir = secondary_data_dir(fixture.root.path(), &fixture.env_id);
        std::fs::create_dir_all(&data_dir).unwrap();
        let recovery_root = tempfile::tempdir().unwrap();

        let (context, _lease) = acquire_library_write_lease(
            recovery_root.path(),
            fixture.root.path(),
            Some(&fixture.env_id),
        )
        .unwrap();
        assert_eq!(context.dir, data_dir);
    }

    /// 库 id → 环境归属：缺省/主库 → master；副环境 → env_id；未知库拒绝。
    #[test]
    fn resolve_library_env_id_routes_master_and_secondary() {
        let fixture = ScopeFixture::new();
        assert_eq!(
            resolve_library_env_id(None, fixture.root.path()).unwrap(),
            "master"
        );
        assert_eq!(
            resolve_library_env_id(Some("master"), fixture.root.path()).unwrap(),
            "master"
        );
        assert_eq!(
            resolve_library_env_id(Some(&fixture.env_id), fixture.root.path()).unwrap(),
            fixture.env_id
        );
        assert_eq!(
            resolve_library_env_id(Some("env-none"), fixture.root.path()).unwrap_err(),
            "library_not_found"
        );
    }

    /// 副环境库目录 = 环境派生目录（ADR-0024 data_dir 规则），不指向官方目录。
    #[test]
    fn resolve_library_context_maps_secondary_to_env_data_dir() {
        let fixture = ScopeFixture::new();
        let context = resolve_library_context(Some(&fixture.env_id), fixture.root.path()).unwrap();
        assert_eq!(
            context.dir,
            secondary_data_dir(fixture.root.path(), &fixture.env_id)
        );
        assert_eq!(context.env_id, fixture.env_id);
        assert!(!context.read_only);
        // 未知库一律拒绝（不落默认库）。
        assert_eq!(
            resolve_library_context(Some("env-none"), fixture.root.path())
                .err()
                .unwrap(),
            "library_not_found"
        );
    }

    /// 副环境活动备份目录：backup_dir 覆盖时落在覆盖目录下的
    /// environments/{env_id} 子目录（与主库链隔离）；未覆盖时 = db 同目录
    ///（各环境目录天然隔离）。主库口径不变（覆盖根直用，不入子目录）。
    #[test]
    fn resolve_library_backup_dir_isolates_secondary_chains() {
        let fixture = ScopeFixture::new();
        let env_db = secondary_data_dir(fixture.root.path(), &fixture.env_id)
            .join("ModularData")
            .join("ai-agent")
            .join("database.db");
        // 未覆盖：db 同目录。
        assert_eq!(
            resolve_library_backup_dir(fixture.root.path(), &env_db, &fixture.env_id),
            env_db.parent().unwrap()
        );
        // 覆盖：覆盖目录下按环境分目录，不与主库链混放。
        let custom = fixture.root.path().join("custom-backups");
        BackupRetentionStore::new(fixture.root.path())
            .save(&BackupRetentionSettings {
                enabled: true,
                keep: 5,
                backup_dir: Some(custom.display().to_string()),
            })
            .unwrap();
        assert_eq!(
            resolve_library_backup_dir(fixture.root.path(), &env_db, &fixture.env_id),
            custom.join("environments").join(&fixture.env_id)
        );
        // 主库备份目录仍是覆盖根（P11-4 行为不变，不受环境子目录影响）。
        let master_db = fixture.root.path().join("master").join("database.db");
        assert_eq!(
            resolve_library_backup_dir(fixture.root.path(), &master_db, MASTER_ENV_ID),
            custom
        );
    }

    /// 裁剪只动指定活动目录内的链：主库裁剪不误伤环境链，环境裁剪不误伤主库链。
    #[test]
    fn prune_backups_if_enabled_in_only_touches_given_chain_dir() {
        let fixture = ScopeFixture::new();
        BackupRetentionStore::new(fixture.root.path())
            .save(&BackupRetentionSettings {
                enabled: true,
                keep: 1,
                backup_dir: None,
            })
            .unwrap();
        let custom = fixture.root.path().join("custom-backups");
        let env_backup_dir = custom.join("environments").join(&fixture.env_id);
        std::fs::create_dir_all(&env_backup_dir).unwrap();

        // 主库链 2 份 + 环境链 2 份（不同目录，同名文件形态）。
        std::fs::write(custom.join("database.db.switch-bak-100"), "a").unwrap();
        std::fs::write(custom.join("database.db.switch-bak-200"), "b").unwrap();
        std::fs::write(env_backup_dir.join("database.db.switch-bak-100"), "a").unwrap();
        std::fs::write(env_backup_dir.join("database.db.switch-bak-200"), "b").unwrap();

        // 主库裁剪（keep=1）：只清主库目录旧链，环境子目录不动。
        prune_backups_if_enabled_in(Path::new("C:/x/database.db"), fixture.root.path(), &custom);
        assert!(!custom.join("database.db.switch-bak-100").exists());
        assert!(custom.join("database.db.switch-bak-200").exists());
        assert!(env_backup_dir.join("database.db.switch-bak-100").exists());

        // 环境裁剪：只清环境子目录旧链，主库目录不动（主库链经第一轮
        // 裁剪后只剩最新一份 200，验证它未被环境裁剪误伤）。
        prune_backups_if_enabled_in(
            Path::new("C:/x/database.db"),
            fixture.root.path(),
            &env_backup_dir,
        );
        assert!(!env_backup_dir.join("database.db.switch-bak-100").exists());
        assert!(env_backup_dir.join("database.db.switch-bak-200").exists());
        assert!(custom.join("database.db.switch-bak-200").exists());
    }

    /// R0-P3：保留裁剪关闭（enabled=false）时全不动——主备份与凭据
    /// 子目录一并保留（铁律：永不自动删）。
    #[test]
    fn prune_disabled_leaves_master_and_credentials_untouched() {
        let fixture = ScopeFixture::new();
        BackupRetentionStore::new(fixture.root.path())
            .save(&BackupRetentionSettings {
                enabled: false,
                keep: 1,
                backup_dir: None,
            })
            .unwrap();
        let custom = fixture.root.path().join("custom-backups");
        std::fs::create_dir_all(custom.join("credentials-100")).unwrap();
        std::fs::write(custom.join("database.db.switch-bak-100"), "a").unwrap();
        std::fs::write(custom.join("database.db.switch-bak-200"), "b").unwrap();
        std::fs::write(custom.join("credentials-100").join("accounts.json"), "{}").unwrap();

        prune_backups_if_enabled_in(Path::new("C:/x/database.db"), fixture.root.path(), &custom);
        // 主备份与凭据子目录全部原样保留。
        assert!(custom.join("database.db.switch-bak-100").exists());
        assert!(custom.join("database.db.switch-bak-200").exists());
        assert!(custom
            .join("credentials-100")
            .join("accounts.json")
            .exists());
    }

    /// 收编台账按库隔离：主库仍写 environments/relay-ledger.json；
    /// 副环境写 environments/relay-ledgers/{env_id}/relay-ledger.json，
    /// 不污染主库轨迹（主库校验不受环境收编影响）。
    #[test]
    fn incorporate_ledger_routes_by_library_scope() {
        let fixture = ScopeFixture::new();
        let handover = sample_handover(Some("111"));

        // 主库：原位置不变。
        assert!(append_incorporate_ledger(
            fixture.root.path(),
            MASTER_ENV_ID,
            &handover,
            "999",
            "222"
        ));
        assert!(fixture
            .root
            .path()
            .join("environments")
            .join("relay-ledger.json")
            .exists());

        // 副环境：独立文件，主库台账不受影响。
        assert!(append_incorporate_ledger(
            fixture.root.path(),
            &fixture.env_id,
            &handover,
            "999",
            "222"
        ));
        let env_ledger = fixture
            .root
            .path()
            .join("environments")
            .join("relay-ledgers")
            .join(&fixture.env_id)
            .join("relay-ledger.json");
        assert!(env_ledger.exists());
        let entries = RelayLedger::new(
            env_ledger
                .parent()
                .unwrap()
                .parent()
                .unwrap()
                .join(&fixture.env_id),
        )
        .load()
        .unwrap();
        assert_eq!(entries.len(), 2);
        // 第 4 参 = 归入目标 user_id，第 5 参 = 目标 profile_id（参数位次
        // 与 append_incorporate_ledger 签名对齐）。
        assert_eq!(entries[0].to_user_id, "999");
        assert_eq!(entries[0].to_profile_id, "222");
    }

    /// 台账读取根按库路由：环境库读 relay-ledgers/{env_id}，主库读原文件。
    #[test]
    fn relay_ledger_root_routes_by_library_scope() {
        let fixture = ScopeFixture::new();
        assert_eq!(
            relay_ledger_root(fixture.root.path(), MASTER_ENV_ID),
            fixture.root.path().join("environments")
        );
        assert_eq!(
            relay_ledger_root(fixture.root.path(), &fixture.env_id),
            fixture
                .root
                .path()
                .join("environments")
                .join("relay-ledgers")
                .join(&fixture.env_id)
        );
    }

    // ===== P11-5 收官：副环境当前账号注册表权威 + 备份链库路由 =====

    /// 构造两会话交接样本（仅台账消费的字段有意义；master_switch_tests
    /// 模块的 helper 为私有，此处本地重建同款形态）。
    fn sample_handover(previous_owner: Option<&str>) -> MasterHandover {
        use traesync_infrastructure::HandoverSession;
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

    // P12-R1b：库账号解析三分支语义测试随实现下沉
    // traesync_infrastructure::library_account（含 into_account 两态与
    // Trae CN 指针三态契约）；写守卫测试随 LibraryContext 下沉 domain。

    /// 备份链按库路由：副环境链枚举环境作用域目录（db 同目录），主库
    /// 口径不变（覆盖根或官方目录 db 同目录）。
    #[test]
    fn master_backup_chain_inner_reads_library_scoped_dir() {
        let fixture = ScopeFixture::new();
        let ctx = resolve_library_context(Some(&fixture.env_id), fixture.root.path()).unwrap();
        let env_db = ctx
            .dir
            .join("ModularData")
            .join("ai-agent")
            .join("database.db");
        std::fs::create_dir_all(env_db.parent().unwrap()).unwrap();
        std::fs::write(env_db.with_file_name("database.db.switch-bak-100"), "a").unwrap();
        std::fs::write(env_db.with_file_name("database.db.switch-bak-200"), "b").unwrap();

        let chain = master_backup_chain_inner(fixture.root.path(), &ctx).unwrap();
        assert_eq!(chain.backups.len(), 2);
        // 目录即环境链所在目录（恢复语义「按文件名找同目录」）。
        assert_eq!(
            chain.backup_dir,
            env_db.parent().unwrap().display().to_string()
        );
        // 时间戳倒序（最新在前）。
        assert_eq!(chain.backups[0].stamp_unix_seconds, 200);
        // 未配置保留策略时读默认值 5。
        assert_eq!(chain.keep_policy, 5);
    }
    // ===== P11-6：Trae CN 只读库（ADR-0032 决策 5）=====

    /// 库 id → 环境归属：trae_cn 是固定注册项（不查环境注册表、不落默认库）。
    #[test]
    fn resolve_library_env_id_registers_trae_cn() {
        let root = tempfile::tempdir().unwrap();
        assert_eq!(
            resolve_library_env_id(Some(TRAE_CN_LIBRARY_ID), root.path()).unwrap(),
            TRAE_CN_LIBRARY_ID
        );
        // 其他未登记 id 仍拒绝（trae_cn 注册不改变 fail-closed 口径）。
        assert_eq!(
            resolve_library_env_id(Some("env-none"), root.path()).unwrap_err(),
            "library_not_found"
        );
    }

    /// Trae CN 只读库目录 = APPDATA 下的产品官方数据根（与主库同源规则）。
    #[test]
    fn trae_cn_library_dir_shape_from_appdata() {
        let appdata = Path::new("C:/Users/u/AppData/Roaming");
        assert_eq!(
            traesync_infrastructure::environment_registry::product_data_dir_from_appdata(
                appdata,
                TraeProduct::TraeCn
            ),
            appdata.join("Trae CN")
        );
    }

    /// 体检对 trae_cn 库「不适用」：状态 + 全零/空字段形态固定。
    #[test]
    fn master_checkup_not_applicable_shape() {
        let dto = master_checkup_not_applicable();
        assert_eq!(dto.status, "not_applicable");
        assert_eq!(dto.current_account_name, None);
        assert!(dto.accounts.is_empty());
        assert_eq!(dto.orphan_project_count, 0);
        assert_eq!(dto.orphan_session_count, 0);
    }
}

// ============================================================================
// P11-4 备份位置/保留/凭据白名单策略单测（自 master_switch_tests 随函数
// 迁入，P12-R3 库域纯移动——备份族函数已归位本模块）。
// ============================================================================

#[cfg(test)]
mod backup_policy_tests {
    use super::*;

    /// P11-4④：keep_policy 读实际备份保留配置；未配置/损坏时 fail-soft
    /// 回退默认 5（展示字段不阻塞）。
    #[test]
    fn backup_keep_policy_reads_actual_config_with_fallback() {
        let root = tempfile::tempdir().unwrap();
        // 未配置（文件不存在）：默认 5。
        assert_eq!(backup_keep_policy(root.path()), 5);
        // 已配置：读实际值。
        BackupRetentionStore::new(root.path())
            .save(&BackupRetentionSettings {
                enabled: true,
                keep: 12,
                backup_dir: None,
            })
            .unwrap();
        assert_eq!(backup_keep_policy(root.path()), 12);
        // 配置损坏：fail-soft 回退默认 5。
        std::fs::write(root.path().join("backup-retention.json"), "not json").unwrap();
        assert_eq!(backup_keep_policy(root.path()), 5);
    }

    /// P11-4① + P16-T1：活动备份目录解析——未配置回退软件本体旁
    /// backups/（exe 同级推导）；配置了覆盖用之；配置损坏 fail-soft 回退
    /// 同上（不阻断主流程）。
    #[test]
    fn resolve_master_backup_dir_prefers_configured_override() {
        let root = tempfile::tempdir().unwrap();
        let db_path = root.path().join("master").join("database.db");
        // 未配置：exe 同级 backups/（P16-T1 新默认；客户端目录不再产生备份）。
        let exe_adjacent = default_master_backup_dir().unwrap();
        assert_eq!(
            resolve_master_backup_dir(root.path(), &db_path),
            exe_adjacent
        );
        // 配置了覆盖：用它。
        let custom = root.path().join("custom-backups");
        BackupRetentionStore::new(root.path())
            .save(&BackupRetentionSettings {
                enabled: true,
                keep: 5,
                backup_dir: Some(custom.display().to_string()),
            })
            .unwrap();
        assert_eq!(resolve_master_backup_dir(root.path(), &db_path), custom);
        // 配置损坏：fail-soft 回退 exe 同级 backups/。
        std::fs::write(root.path().join("backup-retention.json"), "not json").unwrap();
        assert_eq!(
            resolve_master_backup_dir(root.path(), &db_path),
            exe_adjacent
        );
    }

    /// P11-4①：备份位置校验（set/migrate 命令同口径）——None 合法；
    /// 相对路径/空/超长拒绝；可创建目录通过并建立；签到材料根（含其内部）
    /// 拒绝。
    #[test]
    fn validate_backup_dir_rejects_invalid_and_accepts_creatable() {
        let root = tempfile::tempdir().unwrap();
        // None：恢复默认位置。
        assert_eq!(validate_backup_dir(None, root.path()).unwrap(), None);
        // 相对路径 / 空白 / 超长：拒绝。
        assert!(validate_backup_dir(Some("relative/dir"), root.path()).is_err());
        assert!(validate_backup_dir(Some("   "), root.path()).is_err());
        let too_long = format!("D:\\{}", "a".repeat(MAX_BACKUP_DIR_BYTES + 1));
        assert!(validate_backup_dir(Some(&too_long), root.path()).is_err());
        // 可创建（父级存在）：通过并创建目录。
        let target = root.path().join("new-backups");
        assert_eq!(
            validate_backup_dir(Some(target.to_str().unwrap()), root.path()).unwrap(),
            Some(target.clone())
        );
        assert!(target.is_dir());
        // 等于签到材料根与其内部子目录：拒绝。
        let checkin = root.path().join("checkin");
        assert!(validate_backup_dir(Some(checkin.to_str().unwrap()), root.path()).is_err());
        let inner = checkin.join("profiles");
        assert!(validate_backup_dir(Some(inner.to_str().unwrap()), root.path()).is_err());
    }

    /// P11-4②：凭据备份白名单——密文与注册表在、模拟明文与未知目录不在
    /// （明文 token 永不落备份目录的安全断言）。
    #[test]
    fn credentials_backup_copies_whitelist_only() {
        let root = tempfile::tempdir().unwrap();
        let material = root.path().join("checkin");
        std::fs::create_dir_all(material.join("profiles")).unwrap();
        std::fs::create_dir_all(material.join("renewals").join("op-1")).unwrap();
        std::fs::create_dir_all(material.join("refresh-rescue")).unwrap();
        std::fs::create_dir_all(material.join("retired")).unwrap();
        std::fs::write(material.join("accounts.json"), b"{}").unwrap();
        std::fs::write(
            material.join("profiles").join("abc.checkin"),
            b"TRVCKN01-cipher",
        )
        .unwrap();
        std::fs::write(
            material.join("renewals").join("op-1").join("manifest.json"),
            b"{}",
        )
        .unwrap();
        std::fs::write(
            material.join("renewals").join("op-1").join("bundle.before"),
            b"TRVCKN01-cipher2",
        )
        .unwrap();
        std::fs::write(material.join("refresh-rescue").join("m.json"), b"{}").unwrap();
        std::fs::write(material.join("retired").join("x.checkin"), b"TRVCKN01-old").unwrap();
        // 模拟异常明文与未知目录：白名单之外，绝不入备份。
        std::fs::write(material.join("plaintext-token.txt"), b"secret-token").unwrap();
        std::fs::create_dir_all(material.join("unknown-dir")).unwrap();
        std::fs::write(material.join("unknown-dir").join("x.bin"), b"?").unwrap();

        let backup_dir = root.path().join("backups");
        std::fs::create_dir_all(&backup_dir).unwrap();
        let created = copy_credentials_backup(&material, &backup_dir, 1234).unwrap();
        assert!(created);
        let cred = backup_dir.join("credentials-1234");
        assert!(cred.join("accounts.json").is_file());
        assert!(cred.join("profiles").join("abc.checkin").is_file());
        assert!(cred
            .join("renewals")
            .join("op-1")
            .join("bundle.before")
            .is_file());
        assert!(cred.join("refresh-rescue").join("m.json").is_file());
        assert!(cred.join("retired").join("x.checkin").is_file());
        // 白名单之外一律不拷（防御性：明文 token 永不进备份目录）。
        assert!(!cred.join("plaintext-token.txt").exists());
        assert!(!cred.join("unknown-dir").exists());
        // 内容字节一致。
        assert_eq!(
            std::fs::read(cred.join("profiles").join("abc.checkin")).unwrap(),
            b"TRVCKN01-cipher"
        );
    }

    /// P11-4②：材料根不存在（从未登录）不创建凭据子目录，直接成功。
    #[test]
    fn credentials_backup_missing_root_creates_nothing() {
        let root = tempfile::tempdir().unwrap();
        let backup_dir = root.path().join("backups");
        std::fs::create_dir_all(&backup_dir).unwrap();
        let created =
            copy_credentials_backup(&root.path().join("checkin"), &backup_dir, 7).unwrap();
        assert!(!created);
        assert!(!backup_dir.join("credentials-7").exists());
    }
}

// ============================================================================
// P13-2 Trae CN 项目归并命令层单测：守卫矩阵 / 编排回执 / 台账与备份证据。
// 库读写语义由 infrastructure 层覆盖（master_handover::tests cn_merge_*）；
// 此处只测组合根编排接缝（守卫、备份先行、台账路由、进度事件）。
// ============================================================================

#[cfg(test)]
mod trae_cn_merge_tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::mpsc;
    use std::sync::{Arc, Mutex};
    use std::thread;
    use std::time::{Duration, Instant};
    use traesync_infrastructure::account_registry::AccountRecord;
    use traesync_infrastructure::product_account_state::ProductAccountStateStore;
    use traesync_infrastructure::product_state_root_from;

    #[test]
    fn cn_merge_command_orchestration_rejects_missing_wrong_or_pending_library_before_side_effects()
    {
        let root = tempfile::tempdir().unwrap();
        let state = crate::test_support::make_fixture_mode_state(root.path());

        for (library_id, expected_error) in [
            (None, "product_library_mismatch"),
            (Some("master"), "product_library_mismatch"),
            (Some("trae_cn"), "product_operation_pending"),
        ] {
            let executor_calls = Arc::new(AtomicUsize::new(0));
            let process_checks = Arc::new(AtomicUsize::new(0));
            let backup_writes = Arc::new(AtomicUsize::new(0));
            let database_writes = Arc::new(AtomicUsize::new(0));
            let progress_events = Arc::new(AtomicUsize::new(0));

            let progress_counter = Arc::clone(&progress_events);
            let on_progress = move |_event: TraeCnMergeProgressEvent| {
                progress_counter.fetch_add(1, Ordering::SeqCst);
            };
            let execute_calls = Arc::clone(&executor_calls);
            let execute_process_checks = Arc::clone(&process_checks);
            let execute_backup_writes = Arc::clone(&backup_writes);
            let execute_database_writes = Arc::clone(&database_writes);
            let execute = move |_request: TraeCnProjectMergeRequest, _on_progress| {
                execute_calls.fetch_add(1, Ordering::SeqCst);
                async move {
                    execute_process_checks.fetch_add(1, Ordering::SeqCst);
                    execute_backup_writes.fetch_add(1, Ordering::SeqCst);
                    execute_database_writes.fetch_add(1, Ordering::SeqCst);
                    Err("merge_executor_should_not_run".to_string())
                }
            };

            let result = tauri::async_runtime::block_on(merge_trae_cn_project_inner(
                "source-project".to_string(),
                library_id.map(|id| id.to_string()),
                &state,
                on_progress,
                execute,
            ));

            assert_eq!(result.unwrap_err(), expected_error);
            assert_eq!(executor_calls.load(Ordering::SeqCst), 0);
            assert_eq!(process_checks.load(Ordering::SeqCst), 0);
            assert_eq!(backup_writes.load(Ordering::SeqCst), 0);
            assert_eq!(database_writes.load(Ordering::SeqCst), 0);
            assert_eq!(progress_events.load(Ordering::SeqCst), 0);
            assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 0);
        }
    }

    /// 测试账号档案（与 library_account 测试同款占位形态）。
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

    /// 测试夹具：storage_root + CN 数据根库（账号 111 的 p1 目录项目，
    /// 两个正常会话 + 一个空镜像行）+ CN 产品池指针账号 222。
    struct CnMergeFixture {
        root: tempfile::TempDir,
        cn_dir: PathBuf,
        raw_key: String,
        current: AccountRecord,
    }

    impl CnMergeFixture {
        fn new() -> Self {
            Self::with_sessions(true)
        }

        /// `with_sessions = false` 时不播种正常会话（无可并规模的守卫用例）。
        fn with_sessions(with_sessions: bool) -> Self {
            use rusqlite::{Connection, OpenFlags};

            let root = tempfile::tempdir().unwrap();
            let cn_dir = root.path().join("Trae CN");
            let db_dir = cn_dir.join("ModularData").join("ai-agent");
            std::fs::create_dir_all(&db_dir).unwrap();
            let raw_key = "77".repeat(32);
            let flags = OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE;
            let conn = Connection::open_with_flags(db_dir.join("database.db"), flags).unwrap();
            conn.execute_batch(&format!("PRAGMA key = \"x'{}'\";", raw_key))
                .unwrap();
            // CN 形态最小 schema + 跨账号行集（语义覆盖在 infrastructure 层
            // 测试完成；此处支撑命令编排全链路：预检 → 备份 → 事务 → 台账）。
            let mut sql = String::from(
                "CREATE TABLE project (
                    project_id TEXT PRIMARY KEY, user_id TEXT,
                    biz_project_id TEXT, name TEXT, absolute_path TEXT, deleted_at INTEGER);
                 CREATE TABLE chat_session (
                    session_id TEXT PRIMARY KEY, project_id TEXT,
                    session_title TEXT, updated_at INTEGER, deleted_at INTEGER, hidden_status TEXT);
                 CREATE TABLE chat_message (
                    id INTEGER PRIMARY KEY AUTOINCREMENT, message_id TEXT UNIQUE, session_id TEXT,
                    message_role TEXT, message_index INTEGER,
                    reply_to_message_id TEXT, response_message_id TEXT, deleted_at INTEGER);
                 CREATE TABLE chat_message_general (
                    id INTEGER PRIMARY KEY AUTOINCREMENT, message_id TEXT UNIQUE, content TEXT, created_at TEXT);
                 CREATE TABLE chat_message_task (
                    id INTEGER PRIMARY KEY AUTOINCREMENT, message_id TEXT UNIQUE, content TEXT, created_at TEXT);
                 CREATE TABLE chat_turn (turn_id TEXT PRIMARY KEY, session_id TEXT, deleted_at INTEGER);
                 CREATE TABLE task (task_id TEXT PRIMARY KEY, session_id TEXT, deleted_at INTEGER);
                 CREATE TABLE history_v2 (
                    id INTEGER PRIMARY KEY AUTOINCREMENT, history_v2_id TEXT UNIQUE,
                    session_id TEXT, messages TEXT, created_at TEXT);
                 CREATE TABLE agent_run (agent_run_id TEXT PRIMARY KEY, session_id TEXT);
                 CREATE TABLE session_project (session_id TEXT PRIMARY KEY, project_id TEXT);
                 CREATE TABLE server_history_info (
                    id INTEGER PRIMARY KEY, session_id TEXT, conversation_id TEXT,
                    source TEXT, user_id TEXT);
                 INSERT INTO project VALUES ('p1', '111', 'biz-a', '目录项目', 'D:\\work\\demo', NULL);
                 INSERT INTO project VALUES ('p-m', '333', 'biz-a', '空镜像', 'D:\\work\\demo', NULL);
                 INSERT INTO project VALUES ('p-current', '222', 'biz-current', '当前账号项目', 'D:\\work\\current', NULL);
                 INSERT INTO server_history_info (session_id, conversation_id, source, user_id)
                    VALUES ('s1', 'conv-old', 'llm_default', NULL);",
            );
            if with_sessions {
                sql.push_str(
                    "INSERT INTO chat_session VALUES ('s1', 'p1', '会话一', 1, 0, NULL);
                     INSERT INTO chat_session VALUES ('s2', 'p1', '会话二', 2, 0, NULL);
                     INSERT INTO chat_session VALUES ('s-current', 'p-current', '当前账号会话', 3, 0, NULL);
                     INSERT INTO chat_message
                        (message_id, session_id, message_role, message_index, deleted_at)
                        VALUES ('m1', 's1', 'user', 0, NULL);
                     INSERT INTO chat_message
                        (message_id, session_id, message_role, message_index, deleted_at)
                        VALUES ('m2', 's1', 'assistant', 1, NULL);
                     INSERT INTO chat_message_general (message_id, content, created_at)
                        VALUES ('m1', '正文', '2026-09-29');
                     INSERT INTO chat_turn VALUES ('t1', 's1', NULL);
                     INSERT INTO session_project VALUES ('s1', 'p1');
                     INSERT INTO session_project VALUES ('s2', 'p1');",
                );
            }
            conn.execute_batch(&sql).unwrap();
            // P15-7：指针账号（归并目标）+ 来源账号档案（预览名称反查），
            // 档案在 Work 签到注册表根、指针在 product-state 根（迁出旧池）。
            let checkin_root = root.path().join("checkin");
            AccountRegistry::new(&checkin_root)
                .upsert(&account("profile-cn", "222", "指针账号"))
                .unwrap();
            AccountRegistry::new(&checkin_root)
                .upsert(&account("profile-src", "111", "来源账号"))
                .unwrap();
            ProductAccountStateStore::new(product_state_root_from(root.path()))
                .save("profile-cn")
                .unwrap();
            Self {
                root,
                cn_dir,
                raw_key,
                current: account("profile-cn", "222", "指针账号"),
            }
        }

        fn ctx(&self) -> LibraryContext {
            LibraryContext {
                env_id: TRAE_CN_LIBRARY_ID.to_string(),
                env_name: "Trae CN".to_string(),
                dir: self.cn_dir.clone(),
                read_only: true,
            }
        }

        fn backup_count(&self) -> usize {
            std::fs::read_dir(self.cn_dir.join("ModularData").join("ai-agent"))
                .unwrap()
                .flatten()
                .filter(|entry| {
                    entry
                        .file_name()
                        .to_string_lossy()
                        .starts_with("database.db.switch-bak-")
                })
                .count()
        }

        fn project_owner(&self) -> String {
            let db_path = master_database_path(&self.cn_dir);
            let conn = rusqlite::Connection::open(db_path).unwrap();
            conn.execute_batch(&format!("PRAGMA key = \"x'{}'\";", self.raw_key))
                .unwrap();
            conn.query_row(
                "SELECT user_id FROM project WHERE project_id = 'p1'",
                [],
                |row| row.get(0),
            )
            .unwrap()
        }

        fn current_account_session_id(&self) -> String {
            let db_path = master_database_path(&self.cn_dir);
            let conn = rusqlite::Connection::open(db_path).unwrap();
            conn.execute_batch(&format!("PRAGMA key = \"x'{}'\";", self.raw_key))
                .unwrap();
            conn.query_row(
                "SELECT chat_session.session_id FROM chat_session
                 JOIN project USING (project_id)
                 WHERE project.user_id = '222' AND chat_session.deleted_at = 0
                 ORDER BY chat_session.updated_at DESC LIMIT 1",
                [],
                |row| row.get(0),
            )
            .unwrap()
        }

        fn hidden_session_count(&self) -> i64 {
            let db_path = master_database_path(&self.cn_dir);
            let conn = rusqlite::Connection::open(db_path).unwrap();
            conn.execute_batch(&format!("PRAGMA key = \"x'{}'\";", self.raw_key))
                .unwrap();
            conn.query_row(
                "SELECT COUNT(*) FROM chat_session WHERE hidden_status IS NOT NULL",
                [],
                |row| row.get(0),
            )
            .unwrap()
        }
    }

    /// 与生产 CN 归并命令保持同序：先取得物理库租约，再解析账号并执行归并。
    fn run_cn_merge_with_lease(
        storage_root: &Path,
        recovery_root: &Path,
        ctx: LibraryContext,
        account: AccountRecord,
        raw_key: &str,
        on_progress: &dyn Fn(TraeCnMergeProgressEvent),
    ) -> Result<TraeCnMergeResultDto, String> {
        let resolve_account = |_ctx: &LibraryContext| Ok(account.clone());
        let ensure_stopped = |_ctx: &LibraryContext| Ok(());
        run_trae_cn_project_merge_with_lease(
            recovery_root,
            storage_root,
            ctx,
            None,
            &resolve_account,
            raw_key,
            "p1",
            &ensure_stopped,
            on_progress,
        )
    }

    fn run_cn_archive_with_lease(
        storage_root: &Path,
        recovery_root: &Path,
        raw_key: &str,
        ctx: LibraryContext,
        session_id: &str,
        stop_calls: &std::sync::atomic::AtomicUsize,
        after_commit: &dyn Fn(),
    ) -> Result<MasterArchiveApplyResultDto, String> {
        use std::sync::atomic::Ordering;

        let close = |_ctx: &LibraryContext| Ok(());
        let relaunch = |_ctx: &LibraryContext, _title: &str| Ok("not_requested".to_string());
        let ensure_stopped = |_ctx: &LibraryContext| {
            stop_calls.fetch_add(1, Ordering::SeqCst);
            Ok(())
        };
        apply_library_archive_for_context(
            recovery_root,
            storage_root,
            // P15-7：账号档案在 Work 签到注册表根（fixture 布局 {root}/checkin），
            // 与存储根解耦（旧产品池已移除）。
            &storage_root.join("checkin"),
            raw_key,
            ctx,
            None,
            &[session_id.to_string()],
            &[],
            &close,
            &relaunch,
            &ensure_stopped,
            after_commit,
        )
    }

    /// 子进程以合成 Trae CN 数据运行归档和归并共用 caller。
    #[test]
    fn child_runs_cn_archive_and_merge_callers_for_cross_process_test() {
        let Ok(storage_root) = std::env::var("TRAE_SYNC_CN_CALLER_TEST_ROOT") else {
            return;
        };
        let storage_root = PathBuf::from(storage_root);
        let recovery_root = storage_root.join("recovery");
        let raw_key = std::env::var("TRAE_SYNC_CN_CALLER_TEST_RAW_KEY").unwrap();
        let session_id = std::env::var("TRAE_SYNC_CN_CALLER_TEST_SESSION_ID").unwrap();
        let expected_busy = format!(
            "operation_lease_unavailable:{}",
            traesync_infrastructure::operation_lease::OperationLeaseError::DataLocationBusy
        );

        // 两个 CN 写 caller 都必须在解析账号、检查客户端或提交回调前被同库租约拒绝。
        let archive_stop_calls = std::sync::atomic::AtomicUsize::new(0);
        let archive_after_commit_calls = std::sync::atomic::AtomicUsize::new(0);
        let archive_result = run_cn_archive_with_lease(
            &storage_root,
            &recovery_root,
            &raw_key,
            LibraryContext {
                env_id: TRAE_CN_LIBRARY_ID.to_string(),
                env_name: "Trae CN".to_string(),
                dir: storage_root.join("Trae CN"),
                read_only: true,
            },
            &session_id,
            &archive_stop_calls,
            &|| {
                archive_after_commit_calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            },
        );
        assert_eq!(
            archive_result.err().as_deref(),
            Some(expected_busy.as_str())
        );
        assert_eq!(
            archive_stop_calls.load(std::sync::atomic::Ordering::SeqCst),
            0
        );
        assert_eq!(
            archive_after_commit_calls.load(std::sync::atomic::Ordering::SeqCst),
            0
        );

        let resolve_calls = std::sync::atomic::AtomicUsize::new(0);
        let process_check_calls = std::sync::atomic::AtomicUsize::new(0);
        let merge_events = Mutex::new(Vec::new());
        let account_for_merge = account("profile-cn", "222", "指针账号");
        let resolve_account = |_ctx: &LibraryContext| {
            resolve_calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(account_for_merge.clone())
        };
        let ensure_stopped = |_ctx: &LibraryContext| {
            process_check_calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(())
        };
        let merge_result = run_trae_cn_project_merge_with_lease(
            &recovery_root,
            &storage_root,
            LibraryContext {
                env_id: TRAE_CN_LIBRARY_ID.to_string(),
                env_name: "Trae CN".to_string(),
                dir: storage_root.join("Trae CN"),
                read_only: true,
            },
            None,
            &resolve_account,
            &raw_key,
            "p1",
            &ensure_stopped,
            &|event| merge_events.lock().unwrap().push(event.stage),
        );
        assert_eq!(merge_result.err().as_deref(), Some(expected_busy.as_str()));
        assert_eq!(resolve_calls.load(std::sync::atomic::Ordering::SeqCst), 0);
        assert_eq!(
            process_check_calls.load(std::sync::atomic::Ordering::SeqCst),
            0
        );
        assert!(merge_events.lock().unwrap().is_empty());
    }

    struct ReleaseBarrierOnDrop(mpsc::Sender<()>);

    impl Drop for ReleaseBarrierOnDrop {
        fn drop(&mut self) {
            let _ = self.0.send(());
        }
    }

    /// 归并编排回执：库归属改写 + 备份先行证据 + 台账路由 + 进度事件。
    #[test]
    fn run_trae_cn_merge_writes_receipt_backup_and_ledger() {
        let fixture = CnMergeFixture::new();
        let events = std::sync::Mutex::new(Vec::new());
        let receipt = run_trae_cn_project_merge(
            fixture.root.path(),
            &fixture.raw_key,
            &fixture.ctx(),
            &fixture.current,
            "p1",
            &|| Ok(()),
            &|event| events.lock().unwrap().push(event),
        )
        .unwrap();

        assert_eq!(receipt.transferred_projects, 1);
        assert_eq!(receipt.removed_mirror_rows, 1, "来源侧空镜像行清理");
        assert_eq!(receipt.switched_sessions, 2);
        assert_eq!(receipt.moved_messages, 2);
        assert!(receipt.relay_ledger_written);
        // 备份先行：CN 库作用域活动目录（db 同目录）落了 .switch-bak-* 链。
        let db_dir = fixture.cn_dir.join("ModularData").join("ai-agent");
        let backups: Vec<_> = std::fs::read_dir(&db_dir)
            .unwrap()
            .flatten()
            .map(|entry| entry.file_name().into_string().unwrap())
            .filter(|name| name.starts_with("database.db.switch-bak-"))
            .collect();
        assert_eq!(backups.len(), 1, "归并前应生成一份三件套备份");
        assert_eq!(
            Path::new(&receipt.backup_path).parent(),
            Some(db_dir.as_path())
        );
        // 接力台账：CN 库作用域独立文件（environments/relay-ledgers/trae_cn）。
        assert!(fixture
            .root
            .path()
            .join("environments")
            .join("relay-ledgers")
            .join("trae_cn")
            .join("relay-ledger.json")
            .exists());
        // 库状态：p1 归属 222。
        let conn = rusqlite::Connection::open(db_dir.join("database.db")).unwrap();
        conn.execute_batch(&format!("PRAGMA key = \"x'{}'\";", fixture.raw_key))
            .unwrap();
        let owner: String = conn
            .query_row(
                "SELECT user_id FROM project WHERE project_id = 'p1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(owner, "222");
        // 进度事件：backing_up → mapping/executing/verifying → done 全链路。
        let stages: Vec<&str> = events
            .lock()
            .unwrap()
            .iter()
            .map(|event| event.stage)
            .collect();
        assert_eq!(stages.first().copied(), Some("backing_up"));
        assert!(
            stages.contains(&"mapping")
                && stages.contains(&"executing")
                && stages.contains(&"verifying")
        );
        assert_eq!(stages.last().copied(), Some("done"));
    }

    /// 父测锁定 CN 实际数据目录，独立进程中的归档/归并 caller 必须无副作用拒绝。
    #[test]
    fn cn_archive_and_merge_callers_reject_cross_process_library_lease_before_side_effects() {
        let fixture = CnMergeFixture::new();
        let storage_root = fixture.root.path().to_path_buf();
        let recovery_root = storage_root.join("recovery");
        std::fs::create_dir_all(&recovery_root).unwrap();
        let lease = acquire_library_data_lease(&recovery_root, &fixture.ctx())
            .expect("父测应持有 Trae CN 实际数据目录租约");
        let caller = std::process::Command::new(std::env::current_exe().unwrap())
            .env("TRAE_SYNC_CN_CALLER_TEST_ROOT", &storage_root)
            .env("TRAE_SYNC_CN_CALLER_TEST_RAW_KEY", &fixture.raw_key)
            .env(
                "TRAE_SYNC_CN_CALLER_TEST_SESSION_ID",
                fixture.current_account_session_id(),
            )
            .args([
                "--exact",
                "library_archive::trae_cn_merge_tests::child_runs_cn_archive_and_merge_callers_for_cross_process_test",
                "--nocapture",
            ])
            .spawn();
        let mut caller = caller.expect("启动 Trae CN caller 测试子进程失败");
        let deadline = Instant::now() + Duration::from_secs(10);
        let caller_status = loop {
            match caller.try_wait() {
                Ok(Some(status)) => break Some(status),
                Ok(None) if Instant::now() < deadline => {
                    thread::sleep(Duration::from_millis(25));
                }
                Ok(None) | Err(_) => break None,
            }
        };
        if caller_status.is_none() {
            let _ = caller.kill();
            let _ = caller.wait();
            drop(lease);
            panic!(
                "Trae CN 归档/归并 caller 未在持锁期间 10 秒内返回；已终止 caller 并释放测试租约"
            );
        }
        assert!(caller_status.unwrap().success());
        assert_eq!(fixture.project_owner(), "111");
        assert_eq!(fixture.hidden_session_count(), 0);
        assert_eq!(fixture.backup_count(), 0);
        assert!(!storage_root
            .join("environments")
            .join("relay-ledgers")
            .join("trae_cn")
            .join("relay-ledger.json")
            .exists());
    }

    #[test]
    fn cn_project_merge_and_archive_callers_share_the_same_database_lease() {
        let fixture = CnMergeFixture::new();
        let storage_root = fixture.root.path().to_path_buf();
        let recovery_root = storage_root.join("recovery");
        std::fs::create_dir_all(&recovery_root).unwrap();
        let raw_key = fixture.raw_key.clone();
        let merge_storage = storage_root.clone();
        let merge_recovery = recovery_root.clone();
        let merge_ctx = fixture.ctx();
        let merge_account = fixture.current.clone();
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let merge = thread::spawn(move || {
            let on_progress = |event: TraeCnMergeProgressEvent| {
                if event.stage == "done" {
                    // 此阶段在 SQLCipher 与台账写入后触发，caller 租约仍未释放。
                    entered_tx.send(()).unwrap();
                    release_rx.recv().unwrap();
                }
            };
            run_cn_merge_with_lease(
                &merge_storage,
                &merge_recovery,
                merge_ctx,
                merge_account,
                &raw_key,
                &on_progress,
            )
        });
        let _release_merge_if_test_panics = ReleaseBarrierOnDrop(release_tx.clone());
        if entered_rx.recv_timeout(Duration::from_secs(10)).is_err() {
            let _ = release_tx.send(());
            let merge_error = merge.join().unwrap().err();
            panic!("CN 归并未在提交后到达租约屏障；错误={merge_error:?}");
        }

        let archive_stop_calls = std::sync::atomic::AtomicUsize::new(0);
        let session_id = fixture.current_account_session_id();
        let archive_while_merge_holds_lease = run_cn_archive_with_lease(
            &storage_root,
            &recovery_root,
            &fixture.raw_key,
            fixture.ctx(),
            &session_id,
            &archive_stop_calls,
            &|| {},
        );
        assert!(archive_while_merge_holds_lease
            .err()
            .expect("CN 归并期间，归档必须因相同数据库租约拒绝")
            .starts_with("operation_lease_unavailable:"));
        assert_eq!(
            archive_stop_calls.load(std::sync::atomic::Ordering::SeqCst),
            0
        );
        assert_eq!(fixture.project_owner(), "222");
        assert_eq!(fixture.hidden_session_count(), 0);
        assert_eq!(fixture.backup_count(), 1);

        release_tx.send(()).unwrap();
        assert!(merge.join().unwrap().unwrap().relay_ledger_written);
        // 备份名按秒生成，等待到下一秒避免测试中两份合法备份撞名。
        thread::sleep(Duration::from_millis(1100));
        let archive_retry = run_cn_archive_with_lease(
            &storage_root,
            &recovery_root,
            &fixture.raw_key,
            fixture.ctx(),
            &session_id,
            &archive_stop_calls,
            &|| {},
        );
        assert!(
            archive_retry.is_ok(),
            "CN 归档释放后重试失败：{archive_retry:?}"
        );
        assert_eq!(
            archive_stop_calls.load(std::sync::atomic::Ordering::SeqCst),
            1
        );
        assert_eq!(fixture.hidden_session_count(), 1);
        assert_eq!(fixture.backup_count(), 2);
    }

    #[test]
    fn concurrent_duplicate_cn_archive_request_has_no_duplicate_side_effects() {
        let fixture = CnMergeFixture::new();
        let storage_root = fixture.root.path().to_path_buf();
        let recovery_root = storage_root.join("recovery");
        std::fs::create_dir_all(&recovery_root).unwrap();
        let archive_storage = storage_root.clone();
        let archive_recovery = recovery_root.clone();
        let archive_ctx = fixture.ctx();
        let session_id = fixture.current_account_session_id();
        let archive_session_id = session_id.clone();
        let raw_key = fixture.raw_key.clone();
        let stop_calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let stop_calls_thread = stop_calls.clone();
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let archive = thread::spawn(move || {
            let after_commit = || {
                entered_tx.send(()).unwrap();
                release_rx.recv().unwrap();
            };
            run_cn_archive_with_lease(
                &archive_storage,
                &archive_recovery,
                &raw_key,
                archive_ctx,
                &archive_session_id,
                &stop_calls_thread,
                &after_commit,
            )
        });
        let _release_archive_if_test_panics = ReleaseBarrierOnDrop(release_tx.clone());
        if entered_rx.recv_timeout(Duration::from_secs(10)).is_err() {
            let _ = release_tx.send(());
            let archive_error = archive.join().unwrap().err();
            panic!("首个 CN 归档未到达提交后租约屏障；错误={archive_error:?}");
        }

        let duplicate_stop_calls = std::sync::atomic::AtomicUsize::new(0);
        let duplicate_commit_calls = std::sync::atomic::AtomicUsize::new(0);
        let duplicate = run_cn_archive_with_lease(
            &storage_root,
            &recovery_root,
            &fixture.raw_key,
            fixture.ctx(),
            &session_id,
            &duplicate_stop_calls,
            &|| {
                duplicate_commit_calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            },
        );
        assert!(duplicate
            .err()
            .expect("并发重复 CN 归档应被同库租约拒绝")
            .starts_with("operation_lease_unavailable:"));
        assert_eq!(
            duplicate_stop_calls.load(std::sync::atomic::Ordering::SeqCst),
            0
        );
        assert_eq!(
            duplicate_commit_calls.load(std::sync::atomic::Ordering::SeqCst),
            0
        );
        assert_eq!(fixture.hidden_session_count(), 1);
        assert_eq!(fixture.backup_count(), 1);

        release_tx.send(()).unwrap();
        assert_eq!(archive.join().unwrap().unwrap().archived_sessions, 1);
        assert_eq!(stop_calls.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert_eq!(fixture.hidden_session_count(), 1);
        assert_eq!(fixture.backup_count(), 1);
    }

    #[test]
    fn concurrent_duplicate_cn_merge_request_does_not_repeat_backup_or_ledger() {
        let fixture = CnMergeFixture::new();
        let storage_root = fixture.root.path().to_path_buf();
        let recovery_root = storage_root.join("recovery");
        std::fs::create_dir_all(&recovery_root).unwrap();
        let ledger_root = storage_root
            .join("environments")
            .join("relay-ledgers")
            .join("trae_cn");
        let merge_storage = storage_root.clone();
        let merge_recovery = recovery_root.clone();
        let merge_ctx = fixture.ctx();
        let merge_account = fixture.current.clone();
        let raw_key = fixture.raw_key.clone();
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let merge = thread::spawn(move || {
            let on_progress = |event: TraeCnMergeProgressEvent| {
                if event.stage == "done" {
                    entered_tx.send(()).unwrap();
                    release_rx.recv().unwrap();
                }
            };
            run_cn_merge_with_lease(
                &merge_storage,
                &merge_recovery,
                merge_ctx,
                merge_account,
                &raw_key,
                &on_progress,
            )
        });
        let _release_merge_if_test_panics = ReleaseBarrierOnDrop(release_tx.clone());
        if entered_rx.recv_timeout(Duration::from_secs(10)).is_err() {
            let _ = release_tx.send(());
            let merge_error = merge.join().unwrap().err();
            panic!("首个 CN 归并未到达提交后租约屏障；错误={merge_error:?}");
        }

        let committed_ledger_count = RelayLedger::new(&ledger_root).load().unwrap().len();
        let duplicate_events = Mutex::new(Vec::new());
        let duplicate = run_cn_merge_with_lease(
            &storage_root,
            &recovery_root,
            fixture.ctx(),
            fixture.current.clone(),
            &fixture.raw_key,
            &|event| duplicate_events.lock().unwrap().push(event.stage),
        );
        assert!(duplicate
            .err()
            .expect("并发重复 CN 归并应被同库租约拒绝")
            .starts_with("operation_lease_unavailable:"));
        assert!(duplicate_events.lock().unwrap().is_empty());
        assert_eq!(fixture.project_owner(), "222");
        assert_eq!(fixture.backup_count(), 1);
        assert_eq!(
            RelayLedger::new(&ledger_root).load().unwrap().len(),
            committed_ledger_count
        );

        release_tx.send(()).unwrap();
        assert!(merge.join().unwrap().unwrap().relay_ledger_written);
        assert_eq!(fixture.project_owner(), "222");
        assert_eq!(fixture.backup_count(), 1);
        assert_eq!(
            RelayLedger::new(&ledger_root).load().unwrap().len(),
            committed_ledger_count
        );
    }

    #[test]
    fn cn_archive_and_project_merge_callers_share_the_same_database_lease_in_reverse_order() {
        let fixture = CnMergeFixture::new();
        let storage_root = fixture.root.path().to_path_buf();
        let recovery_root = storage_root.join("recovery");
        std::fs::create_dir_all(&recovery_root).unwrap();
        let archive_storage = storage_root.clone();
        let archive_recovery = recovery_root.clone();
        let archive_ctx = fixture.ctx();
        let archive_session_id = fixture.current_account_session_id();
        let raw_key = fixture.raw_key.clone();
        let archive_stop_calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let archive_stop_thread = archive_stop_calls.clone();
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let archive = thread::spawn(move || {
            let after_commit = || {
                entered_tx.send(()).unwrap();
                release_rx.recv().unwrap();
            };
            run_cn_archive_with_lease(
                &archive_storage,
                &archive_recovery,
                &raw_key,
                archive_ctx,
                &archive_session_id,
                &archive_stop_thread,
                &after_commit,
            )
        });
        let _release_archive_if_test_panics = ReleaseBarrierOnDrop(release_tx.clone());
        if entered_rx.recv_timeout(Duration::from_secs(10)).is_err() {
            let _ = release_tx.send(());
            let archive_error = archive.join().unwrap().err();
            panic!("CN 归档未在提交后到达租约屏障；错误={archive_error:?}");
        }

        let merge_events = Mutex::new(Vec::new());
        let merge_while_archive_holds_lease = run_cn_merge_with_lease(
            &storage_root,
            &recovery_root,
            fixture.ctx(),
            fixture.current.clone(),
            &fixture.raw_key,
            &|event| merge_events.lock().unwrap().push(event.stage),
        );
        let merge_error = merge_while_archive_holds_lease
            .err()
            .expect("CN 归档期间，归并必须因相同数据库租约拒绝");
        assert!(
            merge_error.starts_with("operation_lease_unavailable:"),
            "实际返回的归并错误码：{merge_error}"
        );
        assert!(merge_events.lock().unwrap().is_empty());
        assert_eq!(
            archive_stop_calls.load(std::sync::atomic::Ordering::SeqCst),
            1
        );
        assert_eq!(fixture.project_owner(), "111");
        assert_eq!(fixture.hidden_session_count(), 1);
        assert_eq!(fixture.backup_count(), 1);
        assert!(!storage_root
            .join("environments/relay-ledgers/trae_cn/relay-ledger.json")
            .exists());

        release_tx.send(()).unwrap();
        assert_eq!(archive.join().unwrap().unwrap().archived_sessions, 1);
        // 备份名按秒生成，等待到下一秒避免测试中两份合法备份撞名。
        thread::sleep(Duration::from_millis(1100));
        let merge_retry = run_cn_merge_with_lease(
            &storage_root,
            &recovery_root,
            fixture.ctx(),
            fixture.current.clone(),
            &fixture.raw_key,
            &|event| merge_events.lock().unwrap().push(event.stage),
        );
        assert!(
            merge_retry.unwrap().relay_ledger_written,
            "CN 归并释放后重试应成功"
        );
        assert_eq!(fixture.project_owner(), "222");
        assert_eq!(fixture.hidden_session_count(), 1);
        assert_eq!(fixture.backup_count(), 2);
        assert!(!merge_events.lock().unwrap().is_empty());
    }

    /// 守卫矩阵：Trae CN 客户端运行中拒绝（fail-closed，不产生备份）。
    #[test]
    fn run_trae_cn_merge_rejects_while_client_running() {
        let fixture = CnMergeFixture::new();
        let error = run_trae_cn_project_merge(
            fixture.root.path(),
            &fixture.raw_key,
            &fixture.ctx(),
            &fixture.current,
            "p1",
            &|| Err("trae_cn_merge_client_running".to_string()),
            &|_| {},
        )
        .unwrap_err();
        assert_eq!(error, "trae_cn_merge_client_running");
        // 拒绝路径不产生备份（备份在守卫之后）。
        let db_dir = fixture.cn_dir.join("ModularData").join("ai-agent");
        let backups: Vec<_> = std::fs::read_dir(&db_dir)
            .unwrap()
            .flatten()
            .filter(|entry| {
                entry
                    .file_name()
                    .into_string()
                    .unwrap()
                    .starts_with("database.db.switch-bak-")
            })
            .collect();
        assert!(backups.is_empty());
    }

    #[test]
    fn run_trae_cn_merge_rejects_unknown_process_state_before_backup() {
        let fixture = CnMergeFixture::new();
        let error = run_trae_cn_project_merge(
            fixture.root.path(),
            &fixture.raw_key,
            &fixture.ctx(),
            &fixture.current,
            "p1",
            &|| Err("process_state_unknown".to_string()),
            &|_| {},
        )
        .unwrap_err();
        assert_eq!(error, "process_state_unknown");
        let db_dir = fixture.cn_dir.join("ModularData").join("ai-agent");
        assert_eq!(
            std::fs::read_dir(db_dir)
                .unwrap()
                .flatten()
                .filter(|entry| entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with("database.db.switch-bak-"))
                .count(),
            0
        );
    }

    /// 守卫矩阵：非 trae_cn 库的 ctx 直接拒绝（run inner 双重守卫）。
    #[test]
    fn run_trae_cn_merge_rejects_non_trae_cn_library() {
        let fixture = CnMergeFixture::new();
        let master_ctx = LibraryContext {
            env_id: MASTER_ENV_ID.to_string(),
            env_name: "主库".to_string(),
            dir: fixture.cn_dir.clone(),
            read_only: false,
        };
        let error = run_trae_cn_project_merge(
            fixture.root.path(),
            &fixture.raw_key,
            &master_ctx,
            &fixture.current,
            "p1",
            &|| Ok(()),
            &|_| {},
        )
        .unwrap_err();
        assert_eq!(error, "trae_cn_library_only");
    }

    /// 守卫矩阵：无并内容（无可并规模）→ nothing_to_do，不产生备份。
    #[test]
    fn run_trae_cn_merge_rejects_when_nothing_to_merge() {
        let fixture = CnMergeFixture::with_sessions(false);
        let error = run_trae_cn_project_merge(
            fixture.root.path(),
            &fixture.raw_key,
            &fixture.ctx(),
            &fixture.current,
            "p1",
            &|| Ok(()),
            &|_| {},
        )
        .unwrap_err();
        assert_eq!(error, "trae_cn_merge_nothing_to_do");
        // 拒绝路径不产生备份。
        let db_dir = fixture.cn_dir.join("ModularData").join("ai-agent");
        let backups: Vec<_> = std::fs::read_dir(&db_dir)
            .unwrap()
            .flatten()
            .filter(|entry| {
                entry
                    .file_name()
                    .into_string()
                    .unwrap()
                    .starts_with("database.db.switch-bak-")
            })
            .collect();
        assert!(backups.is_empty());
    }

    /// 预览主体：规模聚合 + 注册表反查账号名（界面主信息用，user_id 只进悬浮）。
    #[test]
    fn preview_inner_aggregates_rows_and_maps_account_names() {
        let fixture = CnMergeFixture::new();
        let preview = preview_trae_cn_project_merge_inner(
            &fixture.root.path().join("checkin"),
            &fixture.raw_key,
            &fixture.ctx(),
            &fixture.current,
            "p1",
        )
        .unwrap();
        assert_eq!(preview.absolute_path.as_deref(), Some("D:\\work\\demo"));
        assert_eq!(preview.target_account_name, "指针账号");
        assert!(!preview.target_exists, "222 名下无同目录行 → 整体转入形态");
        assert_eq!(preview.total_sessions, 2);
        assert_eq!(preview.total_messages, 2);
        // 两个来源账号行（111 有会话 + 333 空镜像）。
        assert_eq!(preview.sources.len(), 2);
        let named = preview
            .sources
            .iter()
            .find(|source| source.user_id == "111")
            .unwrap();
        assert_eq!(named.account_name.as_deref(), Some("来源账号"));
        assert_eq!(named.session_count, 2);
        assert_eq!(named.message_count, 2);
        let mirror = preview
            .sources
            .iter()
            .find(|source| source.user_id == "333")
            .unwrap();
        assert_eq!(mirror.session_count, 0, "空镜像行 0 会话");
        assert_eq!(mirror.account_name, None, "未登记账号展示降级");
    }

    /// 共用前置：trae_cn 库放行并解析指针账号；其他库拒绝（稳定错误码）。
    #[test]
    fn resolve_merge_context_guards_library_scope() {
        let fixture = CnMergeFixture::new();
        // trae_cn：放行，账号来自产品池指针。
        let account = resolve_trae_cn_merge_context(
            fixture.root.path().join("checkin").as_path(),
            fixture.root.path(),
            Some(TRAE_CN_LIBRARY_ID),
        )
        .map(|(_, account)| account.account_id)
        .unwrap();
        assert_eq!(account, "222");
        // 已登记副环境：非 trae_cn 库 → 稳定错误码。
        let env_id = traesync_infrastructure::environment_registry::EnvironmentRegistry::new(
            fixture.root.path(),
        )
        .create("副环境")
        .unwrap()
        .env_id;
        let error = resolve_trae_cn_merge_context(
            fixture.root.path().join("checkin").as_path(),
            fixture.root.path(),
            Some(&env_id),
        )
        .map(|_| ())
        .unwrap_err();
        assert_eq!(error, "trae_cn_library_only");
    }
}

// ============================================================================
// P13-3 Trae CN 归档/恢复命令层单测（ADR-0033 决策 1 第二类写）：
// hidden_status 写入属首例——CN fixture 覆盖归档/恢复往返、备份先行证据、
// 客户端运行拒绝与「归档放行 / 删除合并仍拒」三态边界。库读写原语本体由
// infrastructure 层（master_archive::tests）覆盖，此处只测组合根编排接缝。
// ============================================================================

#[cfg(test)]
mod trae_cn_archive_tests {
    use super::*;
    use traesync_infrastructure::ARCHIVE_HIDDEN_STATUS;

    /// 测试夹具：CN 数据根库——账号 111 的项目 p1 挂两个正常会话；
    /// 账号 222 的项目 p2 挂一条归档会话（恢复用例的跨账号行）。
    struct CnArchiveFixture {
        root: tempfile::TempDir,
        cn_dir: PathBuf,
        raw_key: String,
    }

    impl CnArchiveFixture {
        fn new() -> Self {
            use rusqlite::{Connection, OpenFlags};

            let root = tempfile::tempdir().unwrap();
            let cn_dir = root.path().join("Trae CN");
            let db_dir = cn_dir.join("ModularData").join("ai-agent");
            std::fs::create_dir_all(&db_dir).unwrap();
            let raw_key = "88".repeat(32);
            let flags = OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE;
            let conn = Connection::open_with_flags(db_dir.join("database.db"), flags).unwrap();
            conn.execute_batch(&format!("PRAGMA key = \"x'{}'\";", raw_key))
                .unwrap();
            // CN 形态最小 schema（hidden_status 通道：project 归属 +
            // chat_session.hidden_status；与 infrastructure 层往返测试同构）。
            conn.execute_batch(
                "CREATE TABLE project (
                    project_id TEXT PRIMARY KEY, user_id TEXT,
                    biz_project_id TEXT, name TEXT, absolute_path TEXT, deleted_at INTEGER);
                 CREATE TABLE chat_session (
                    session_id TEXT PRIMARY KEY, project_id TEXT,
                    session_title TEXT, updated_at INTEGER, deleted_at INTEGER, hidden_status TEXT);
                 INSERT INTO project VALUES ('p1', '111', 'biz-a', '目录项目', 'D:\\work\\demo', NULL);
                 INSERT INTO project VALUES ('p2', '222', 'biz-b', '归档项目', 'D:\\work\\arch', NULL);
                 INSERT INTO chat_session VALUES ('s1', 'p1', '会话一', 1, 0, NULL);
                 INSERT INTO chat_session VALUES ('s2', 'p1', '会话二', 2, 0, NULL);
                 INSERT INTO chat_session VALUES ('s3', 'p2', '已归档会话', 3, 0, 'voice_discussion');",
            )
            .unwrap();
            Self {
                root,
                cn_dir,
                raw_key,
            }
        }

        fn ctx(&self) -> LibraryContext {
            LibraryContext {
                env_id: TRAE_CN_LIBRARY_ID.to_string(),
                env_name: "Trae CN".to_string(),
                dir: self.cn_dir.clone(),
                read_only: true,
            }
        }

        fn open_db(&self) -> rusqlite::Connection {
            use rusqlite::{Connection, OpenFlags};

            let conn = Connection::open_with_flags(
                self.cn_dir
                    .join("ModularData")
                    .join("ai-agent")
                    .join("database.db"),
                OpenFlags::SQLITE_OPEN_READ_WRITE,
            )
            .unwrap();
            conn.execute_batch(&format!("PRAGMA key = \"x'{}'\";", self.raw_key))
                .unwrap();
            conn
        }

        /// CN 库作用域活动目录内的备份链份数（备份先行证据）。
        fn backup_count(&self) -> usize {
            let db_dir = self.cn_dir.join("ModularData").join("ai-agent");
            std::fs::read_dir(&db_dir)
                .unwrap()
                .flatten()
                .filter(|entry| {
                    entry
                        .file_name()
                        .into_string()
                        .unwrap()
                        .starts_with("database.db.switch-bak-")
                })
                .count()
        }
    }

    /// CN fixture 归档/恢复往返（hidden_status 首例写）：归档当前指针账号
    /// 名下会话 → 备份先行证据 → 恢复跨账号归档行并切归属。
    #[test]
    fn cn_archive_apply_roundtrip_writes_hidden_status_with_backup() {
        let fixture = CnArchiveFixture::new();
        // 归档：s1（111 名下，NULL → voice_discussion）。
        let receipt = run_trae_cn_archive_apply(
            fixture.root.path(),
            &fixture.raw_key,
            &fixture.ctx(),
            "111",
            &["s1".to_string()],
            &[],
            &|| Ok(()),
        )
        .unwrap();
        assert_eq!(receipt.archived_sessions, 1);
        assert_eq!(receipt.restored_sessions, 0);
        assert_eq!(receipt.relaunch_outcome, "not_requested");
        // 备份先行：CN 库作用域活动目录（db 同目录）落了 .switch-bak-* 链。
        assert_eq!(fixture.backup_count(), 1, "归档应用前应生成一份三件套备份");
        {
            let conn = fixture.open_db();
            let hidden: String = conn
                .query_row(
                    "SELECT hidden_status FROM chat_session WHERE session_id = 's1'",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(hidden, ARCHIVE_HIDDEN_STATUS);
        }

        // 恢复：跨账号归档行 s3 还原 NULL，项目归属随恢复切到当前账号
        //（apply 原语既有语义：恢复 = 归位当前账号的正常会话范围）。
        // 备份链命名按秒级时间戳且 create_new 永不覆盖：两次应用须跨秒，
        // 否则同戳备份冲突（与生产同款约束，等待 1.1s 错开）。
        std::thread::sleep(std::time::Duration::from_millis(1100));
        let receipt = run_trae_cn_archive_apply(
            fixture.root.path(),
            &fixture.raw_key,
            &fixture.ctx(),
            "111",
            &[],
            &["s3".to_string()],
            &|| Ok(()),
        )
        .unwrap();
        assert_eq!(receipt.restored_sessions, 1);
        {
            let conn = fixture.open_db();
            let hidden: Option<String> = conn
                .query_row(
                    "SELECT hidden_status FROM chat_session WHERE session_id = 's3'",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(hidden, None);
            let owner: String = conn
                .query_row(
                    "SELECT user_id FROM project WHERE project_id = 'p2'",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(owner, "111", "恢复的会话项目归属切到当前账号");
        }
    }

    /// 守卫矩阵：Trae CN 客户端运行中拒绝（fail-closed，不产生备份）。
    #[test]
    fn cn_archive_apply_rejects_while_client_running() {
        let fixture = CnArchiveFixture::new();
        let error = run_trae_cn_archive_apply(
            fixture.root.path(),
            &fixture.raw_key,
            &fixture.ctx(),
            "111",
            &["s1".to_string()],
            &[],
            &|| Err("trae_cn_archive_client_running".to_string()),
        )
        .unwrap_err();
        assert_eq!(error, "trae_cn_archive_client_running");
        assert_eq!(fixture.backup_count(), 0, "拒绝路径不产生备份");
    }

    #[test]
    fn cn_archive_apply_rejects_unknown_process_state_before_backup() {
        let fixture = CnArchiveFixture::new();
        let error = run_trae_cn_archive_apply(
            fixture.root.path(),
            &fixture.raw_key,
            &fixture.ctx(),
            "111",
            &["s1".to_string()],
            &[],
            &|| Err("process_state_unknown".to_string()),
        )
        .unwrap_err();
        assert_eq!(error, "process_state_unknown");
        assert_eq!(fixture.backup_count(), 0);
    }

    /// 三态边界：CN 库归档/恢复放行（P13-3），删除/合并项目命令走的原写
    /// 边界守卫仍拒绝（library_read_only）——归档开放不翻转只读边界。
    #[test]
    fn cn_archive_context_allows_archive_but_blocks_delete_and_merge() {
        let fixture = CnArchiveFixture::new();
        let ctx = fixture.ctx();
        assert!(ctx.ensure_archive_writable().is_ok());
        assert_eq!(ctx.ensure_writable().unwrap_err(), "library_read_only");
    }
}

#[cfg(test)]
mod p14_4c_archive_caller_tests {
    use super::{
        acquire_library_data_lease, apply_library_archive_for_context,
        run_master_project_merge_with_lease,
    };
    use crate::credential_handover::{
        switch_master_account_at_with_identity_switch, sync_plugins_for_switch,
        MasterAccountSwitchDto,
    };
    use crate::environment::{
        delete_environment_inner_with_process_check, login_environment_inner_with_ops,
    };
    use rusqlite::{Connection, OpenFlags};
    use std::io::{BufRead, BufReader, Write};
    use std::path::Path;
    use std::path::PathBuf;
    use std::process::{Child, Command, ExitStatus, Stdio};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::mpsc;
    use std::sync::Arc;
    use std::thread;
    use std::time::{Duration, Instant};
    use tempfile::TempDir;
    use traesync_domain::LibraryContext;
    use traesync_infrastructure::environment_registry::{secondary_data_dir, EnvironmentRegistry};
    use traesync_infrastructure::trae_instance::TraeInstanceCloseError;
    use traesync_infrastructure::{
        archive_login_user_id, construct_auth_identity, master_database_path,
        plugin_cloud_sync::PluginCloudSyncOutcome, switch_auth_identity_for_account,
        AccountRecord, AccountRegistry, ConstructAuthInput, OperationLease, OperationLeaseError,
        UserInfoFull,
    };

    /// 离线合成切号入口（P15-5）：生产默认身份路径已改为 `.fused` 容器
    /// 构造（含在线 GetUserInfo 复核），租约/回滚/交接矩阵测试注入固定
    /// 恒等交换以聚焦本票语义——与 P14-3 故障注入同模式。参数与生产
    /// `switch_master_account_at` 完全一致。
    #[allow(clippy::too_many_arguments)]
    fn switch_master_account_at_synthetic<R: tauri::Runtime>(
        app: &tauri::AppHandle<R>,
        material_root: &Path,
        storage_root: &Path,
        recovery_root: &Path,
        raw_key: &str,
        profile_id: &str,
        master_dir: &Path,
        force: bool,
        close_master: &dyn Fn(&Path) -> Result<(), TraeInstanceCloseError>,
        launch_master: &dyn Fn(&Path, &str) -> Result<String, String>,
        before_return: &dyn Fn(),
    ) -> Result<MasterAccountSwitchDto, String> {
        let identity_switch = |record: &AccountRecord, donor_dir: &Path, target_dir: &Path| {
            // 离线 E1 存档移植（fixture 提供供体存档，真实写主库 blob）：
            // 保持租约/交接/回滚矩阵对 blob 身份的断言语义与旧默认路径一致。
            switch_auth_identity_for_account(donor_dir, target_dir, &record.account_id)
        };
        // 生产同源插件对账闭包：fixture 凭据不可用时对账自终止（aborted），
        // 与原编排内联实现的 fail-soft 语义一致。
        let plugin_sync =
            |from_user_id: &str,
             target_record: &AccountRecord,
             records: &[AccountRecord]|
             -> PluginCloudSyncOutcome {
                sync_plugins_for_switch(material_root, storage_root, records, from_user_id, target_record)
            };
        switch_master_account_at_with_identity_switch(
            app,
            material_root,
            storage_root,
            recovery_root,
            raw_key,
            profile_id,
            master_dir,
            force,
            close_master,
            launch_master,
            before_return,
            &identity_switch,
            &plugin_sync,
        )
    }

    struct Fixture {
        _root: TempDir,
        recovery_root: PathBuf,
        storage_root: PathBuf,
        material_root: PathBuf,
        master_dir: PathBuf,
        env_id: String,
        env_name: String,
        raw_key: String,
    }

    impl Fixture {
        fn new() -> Self {
            let root = tempfile::tempdir().unwrap();
            let recovery_root = root.path().join("recovery");
            let storage_root = root.path().join("storage");
            let material_root = root.path().join("material");
            let master_dir = root.path().join("synthetic-master");
            let raw_key = "ab".repeat(32);
            std::fs::create_dir_all(&recovery_root).unwrap();
            std::fs::create_dir_all(&storage_root).unwrap();
            let db_path = master_database_path(&master_dir);
            std::fs::create_dir_all(db_path.parent().unwrap()).unwrap();
            let conn = Connection::open_with_flags(
                &db_path,
                OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE,
            )
            .unwrap();
            conn.execute_batch(&format!("PRAGMA key = \"x'{raw_key}'\";"))
                .unwrap();
            conn.execute_batch(
                "CREATE TABLE project (
                    project_id TEXT PRIMARY KEY, user_id TEXT,
                    biz_project_id TEXT, name TEXT, deleted_at INTEGER);
                 CREATE TABLE chat_session (
                    session_id TEXT PRIMARY KEY, project_id TEXT, session_title TEXT,
                    updated_at INTEGER, deleted_at INTEGER, hidden_status TEXT);
                 CREATE TABLE chat_message (
                    id INTEGER PRIMARY KEY AUTOINCREMENT, message_id TEXT UNIQUE,
                    session_id TEXT, message_role TEXT, message_index INTEGER,
                    reply_to_message_id TEXT, response_message_id TEXT, deleted_at INTEGER);
                 CREATE TABLE chat_message_general (
                    id INTEGER PRIMARY KEY AUTOINCREMENT, message_id TEXT UNIQUE,
                    content TEXT, created_at TEXT);
                 CREATE TABLE chat_message_task (
                    id INTEGER PRIMARY KEY AUTOINCREMENT, message_id TEXT UNIQUE,
                    content TEXT, created_at TEXT);
                 CREATE TABLE chat_turn (turn_id TEXT PRIMARY KEY, session_id TEXT, deleted_at INTEGER);
                 CREATE TABLE task (task_id TEXT PRIMARY KEY, session_id TEXT, deleted_at INTEGER);
                 CREATE TABLE history_v2 (
                    id INTEGER PRIMARY KEY AUTOINCREMENT, history_v2_id TEXT UNIQUE,
                    session_id TEXT, messages TEXT, created_at TEXT);
                 CREATE TABLE agent_run (agent_run_id TEXT PRIMARY KEY, session_id TEXT);
                 CREATE TABLE session_project (session_id TEXT PRIMARY KEY, project_id TEXT);
                 CREATE TABLE server_history_info (
                    id INTEGER PRIMARY KEY, session_id TEXT, conversation_id TEXT,
                    source TEXT, user_id TEXT);
                 INSERT INTO project VALUES ('project-1', 'account-1', 'biz-1', '项目一', NULL);
                 INSERT INTO chat_session VALUES (
                    'session-1', 'project-1', '测试会话', 1, NULL, NULL);",
            )
            .unwrap();
            drop(conn);

            let source = account("profile-1", "account-1");
            let target = account("profile-2", "account-2");
            let registry = AccountRegistry::new(&material_root);
            registry.upsert(&source).unwrap();
            registry.upsert(&target).unwrap();
            seed_auth(&master_dir, &source);
            seed_auth(
                &storage_root.join("trae-instances").join("profile-2"),
                &target,
            );

            // P16-T1：主库默认备份目录已改为 exe 同级 backups/ 推导——测试
            // 必须配置覆盖把备份隔离在夹具内（与生产「用户配置优先于默认
            // 推导」同口径），否则多测试共享 exe 旁目录会互相污染计数。
            traesync_infrastructure::backup_retention::BackupRetentionStore::new(&storage_root)
                .save(&traesync_infrastructure::backup_retention::BackupRetentionSettings {
                    enabled: true,
                    keep: 5,
                    backup_dir: Some(root.path().join("backups").display().to_string()),
                })
                .unwrap();

            Self {
                _root: root,
                recovery_root,
                storage_root,
                material_root,
                master_dir,
                env_id: "master".to_string(),
                env_name: "主库".to_string(),
                raw_key,
            }
        }

        fn as_secondary_environment(mut self) -> Self {
            let record = traesync_infrastructure::environment_registry::EnvironmentRegistry::new(
                &self.storage_root,
            )
            .create("测试环境")
            .unwrap();
            let env_dir = traesync_infrastructure::environment_registry::secondary_data_dir(
                &self.storage_root,
                &record.env_id,
            );
            std::fs::create_dir_all(env_dir.parent().unwrap()).unwrap();
            std::fs::rename(&self.master_dir, &env_dir).unwrap();
            let storage_path = env_dir.join("User/globalStorage/storage.json");
            let mut storage: serde_json::Value =
                serde_json::from_slice(&std::fs::read(&storage_path).unwrap()).unwrap();
            // 登录态判定只要求 usertag 键存在；使用独立占位值，避免与 cloudide 密文重复。
            storage.as_object_mut().unwrap().insert(
                "iCubeAuthInfo://usertag".to_string(),
                serde_json::Value::String("fixture-usertag".to_string()),
            );
            std::fs::write(&storage_path, serde_json::to_vec(&storage).unwrap()).unwrap();
            self.master_dir = env_dir;
            self.env_id = record.env_id.clone();
            self.env_name = record.name;
            traesync_infrastructure::environment_registry::EnvironmentRegistry::new(
                &self.storage_root,
            )
            .set_current_profile(&self.env_id, "profile-1")
            .unwrap();
            self
        }

        fn context(&self) -> LibraryContext {
            LibraryContext {
                env_id: self.env_id.clone(),
                env_name: self.env_name.clone(),
                dir: self.master_dir.clone(),
                read_only: false,
            }
        }

        fn apply(
            &self,
            close_calls: &AtomicUsize,
            relaunch_calls: &AtomicUsize,
        ) -> Result<super::MasterArchiveApplyResultDto, String> {
            self.apply_session(&self.session_id(), close_calls, relaunch_calls)
        }

        fn apply_session(
            &self,
            session_id: &str,
            close_calls: &AtomicUsize,
            relaunch_calls: &AtomicUsize,
        ) -> Result<super::MasterArchiveApplyResultDto, String> {
            self.apply_session_after_commit(session_id, close_calls, relaunch_calls, &|| {})
        }

        fn apply_session_after_commit(
            &self,
            session_id: &str,
            close_calls: &AtomicUsize,
            relaunch_calls: &AtomicUsize,
            after_commit: &dyn Fn(),
        ) -> Result<super::MasterArchiveApplyResultDto, String> {
            self.apply_with_context(
                self.context(),
                None,
                session_id,
                close_calls,
                relaunch_calls,
                after_commit,
            )
        }

        fn apply_environment_session_after_commit(
            &self,
            session_id: &str,
            close_calls: &AtomicUsize,
            relaunch_calls: &AtomicUsize,
            after_commit: &dyn Fn(),
        ) -> Result<super::MasterArchiveApplyResultDto, String> {
            run_environment_archive(
                &self.recovery_root,
                &self.storage_root,
                &self.material_root,
                &self.raw_key,
                &self.env_id,
                session_id,
                close_calls,
                relaunch_calls,
                after_commit,
            )
        }

        fn apply_with_context(
            &self,
            ctx: LibraryContext,
            lifecycle_lease: Option<traesync_infrastructure::OperationLease>,
            session_id: &str,
            close_calls: &AtomicUsize,
            relaunch_calls: &AtomicUsize,
            after_commit: &dyn Fn(),
        ) -> Result<super::MasterArchiveApplyResultDto, String> {
            let close = |_ctx: &LibraryContext| {
                close_calls.fetch_add(1, Ordering::SeqCst);
                Ok(())
            };
            let relaunch = |_ctx: &LibraryContext, _title: &str| {
                relaunch_calls.fetch_add(1, Ordering::SeqCst);
                Ok("launched".to_string())
            };
            let ensure_cn_stopped = |_ctx: &LibraryContext| Ok(());
            apply_library_archive_for_context(
                &self.recovery_root,
                &self.storage_root,
                &self.material_root,
                &self.raw_key,
                ctx,
                lifecycle_lease,
                &[session_id.to_string()],
                &[],
                &close,
                &relaunch,
                &ensure_cn_stopped,
                after_commit,
            )
        }

        fn hidden_status(&self) -> Option<String> {
            let conn = Connection::open_with_flags(
                master_database_path(&self.master_dir),
                OpenFlags::SQLITE_OPEN_READ_WRITE,
            )
            .unwrap();
            conn.execute_batch(&format!("PRAGMA key = \"x'{0}'\";", self.raw_key))
                .unwrap();
            conn.query_row(
                "SELECT hidden_status FROM chat_session WHERE project_id = 'project-1'",
                [],
                |row| row.get(0),
            )
            .unwrap()
        }

        fn session_id(&self) -> String {
            let conn = Connection::open_with_flags(
                master_database_path(&self.master_dir),
                OpenFlags::SQLITE_OPEN_READ_WRITE,
            )
            .unwrap();
            conn.execute_batch(&format!("PRAGMA key = \"x'{0}'\";", self.raw_key))
                .unwrap();
            conn.query_row(
                "SELECT session_id FROM chat_session WHERE project_id = 'project-1'",
                [],
                |row| row.get(0),
            )
            .unwrap()
        }

        fn backup_count(&self) -> usize {
            let db_path = master_database_path(&self.master_dir);
            std::fs::read_dir(super::resolve_library_backup_dir(
                &self.storage_root,
                &db_path,
                &self.env_id,
            ))
            .ok()
            .into_iter()
            .flatten()
            .filter_map(Result::ok)
            .filter(|entry| {
                let name = entry.file_name().to_string_lossy().into_owned();
                name.starts_with("database.db.switch-bak-")
                    && !name.ends_with("-wal")
                    && !name.ends_with("-shm")
            })
            .count()
        }

        fn project_owner(&self) -> String {
            let conn = Connection::open_with_flags(
                master_database_path(&self.master_dir),
                OpenFlags::SQLITE_OPEN_READ_WRITE,
            )
            .unwrap();
            conn.execute_batch(&format!("PRAGMA key = \"x'{0}'\";", self.raw_key))
                .unwrap();
            conn.query_row(
                "SELECT user_id FROM project WHERE project_id = 'project-1'",
                [],
                |row| row.get(0),
            )
            .unwrap()
        }

        fn seed_merge_source(&self) {
            let conn = Connection::open_with_flags(
                master_database_path(&self.master_dir),
                OpenFlags::SQLITE_OPEN_READ_WRITE,
            )
            .unwrap();
            conn.execute_batch(&format!("PRAGMA key = \"x'{0}'\";", self.raw_key))
                .unwrap();
            conn.execute(
                "INSERT INTO project VALUES ('project-2', 'account-1', 'biz-2', '项目二', NULL)",
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO chat_session VALUES ('session-2', 'project-2', '归并源会话', 2, NULL, NULL)",
                [],
            )
            .unwrap();
        }

        fn session_project_for_title(&self, session_title: &str) -> String {
            let conn = Connection::open_with_flags(
                master_database_path(&self.master_dir),
                OpenFlags::SQLITE_OPEN_READ_WRITE,
            )
            .unwrap();
            conn.execute_batch(&format!("PRAGMA key = \"x'{0}'\";", self.raw_key))
                .unwrap();
            conn.query_row(
                "SELECT project_id FROM chat_session WHERE session_title = ?1",
                [session_title],
                |row| row.get(0),
            )
            .unwrap()
        }

        fn project_exists(&self, project_id: &str) -> bool {
            let conn = Connection::open_with_flags(
                master_database_path(&self.master_dir),
                OpenFlags::SQLITE_OPEN_READ_WRITE,
            )
            .unwrap();
            conn.execute_batch(&format!("PRAGMA key = \"x'{0}'\";", self.raw_key))
                .unwrap();
            conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM project WHERE project_id = ?1)",
                [project_id],
                |row| row.get(0),
            )
            .unwrap()
        }
    }

    fn run_environment_login(
        material_root: &Path,
        storage_root: &Path,
        recovery_root: &Path,
        raw_key: &str,
        env_id: &str,
        profile_id: &str,
        close_calls: &AtomicUsize,
        auth_calls: &AtomicUsize,
        before_return: &dyn Fn(),
    ) -> Result<crate::environment::EnvironmentLoginReceiptDto, String> {
        let close = |_dir: &Path| {
            close_calls.fetch_add(1, Ordering::SeqCst);
            Ok(())
        };
        let switch_identity = |record: &AccountRecord, donor_dir: &Path, env_dir: &Path| {
            auth_calls.fetch_add(1, Ordering::SeqCst);
            switch_auth_identity_for_account(donor_dir, env_dir, &record.account_id).map(|_| ())
        };
        login_environment_inner_with_ops(
            material_root,
            storage_root,
            recovery_root,
            raw_key,
            env_id,
            profile_id,
            &close,
            &switch_identity,
            before_return,
        )
    }

    fn run_environment_archive(
        recovery_root: &Path,
        storage_root: &Path,
        material_root: &Path,
        raw_key: &str,
        env_id: &str,
        session_id: &str,
        close_calls: &AtomicUsize,
        relaunch_calls: &AtomicUsize,
        after_commit: &dyn Fn(),
    ) -> Result<super::MasterArchiveApplyResultDto, String> {
        let (ctx, lifecycle_lease) =
            super::prepare_library_write_context(recovery_root, storage_root, Some(env_id))?;
        let close = |_ctx: &LibraryContext| {
            close_calls.fetch_add(1, Ordering::SeqCst);
            Ok(())
        };
        let relaunch = |_ctx: &LibraryContext, _title: &str| {
            relaunch_calls.fetch_add(1, Ordering::SeqCst);
            Ok("launched".to_string())
        };
        let ensure_cn_stopped = |_ctx: &LibraryContext| Ok(());
        apply_library_archive_for_context(
            recovery_root,
            storage_root,
            material_root,
            raw_key,
            ctx,
            lifecycle_lease,
            &[session_id.to_string()],
            &[],
            &close,
            &relaunch,
            &ensure_cn_stopped,
            after_commit,
        )
    }

    fn run_work_project_merge(
        fixture: &Fixture,
        source_project_id: &str,
        target_project_id: &str,
        process_check_calls: &AtomicUsize,
        after_commit: &dyn Fn(),
    ) -> Result<super::MasterMergeResultDto, String> {
        let (ctx, lifecycle_lease) = super::prepare_library_write_context(
            &fixture.recovery_root,
            &fixture.storage_root,
            Some(&fixture.env_id),
        )?;
        let ensure_stopped = |_ctx: &LibraryContext| {
            process_check_calls.fetch_add(1, Ordering::SeqCst);
            Ok(())
        };
        run_master_project_merge_with_lease(
            &fixture.recovery_root,
            &fixture.storage_root,
            &fixture.storage_root,
            &fixture.material_root,
            &fixture.raw_key,
            ctx,
            lifecycle_lease,
            &[source_project_id.to_string()],
            target_project_id,
            &ensure_stopped,
            after_commit,
        )
    }

    #[test]
    fn concurrent_duplicate_work_project_merge_is_rejected_without_repeating_side_effects() {
        let fixture = Fixture::new().as_secondary_environment();
        fixture.seed_merge_source();
        let process_check_calls = Arc::new(AtomicUsize::new(0));
        let process_check_thread = process_check_calls.clone();
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();

        let receipt = thread::scope(|scope| {
            let _release_merge_if_test_panics = ReleaseSwitchOnDrop(release_tx.clone());
            let fixture_ref = &fixture;
            let merge = scope.spawn(move || {
                let after_commit = move || {
                    entered_tx.send(()).unwrap();
                    release_rx.recv().unwrap();
                };
                run_work_project_merge(
                    fixture_ref,
                    "project-2",
                    "project-1",
                    &process_check_thread,
                    &after_commit,
                )
            });
            if entered_rx.recv_timeout(Duration::from_secs(10)).is_err() {
                let _ = release_tx.send(());
                let merge_error = merge.join().unwrap().err();
                panic!("首个 Work 归并未到达提交后屏障；错误={merge_error:?}");
            }

            let duplicate_process_checks = AtomicUsize::new(0);
            let duplicate = run_work_project_merge(
                &fixture,
                "project-2",
                "project-1",
                &duplicate_process_checks,
                &|| {},
            );
            assert!(duplicate
                .err()
                .expect("同一库上的重复 Work 归并必须被租约拒绝")
                .starts_with("operation_lease_unavailable:"));
            assert_eq!(duplicate_process_checks.load(Ordering::SeqCst), 0);
            assert_eq!(process_check_calls.load(Ordering::SeqCst), 1);
            assert!(!fixture.project_exists("project-2"));
            assert_eq!(fixture.session_project_for_title("归并源会话"), "project-1");
            assert_eq!(fixture.backup_count(), 1);

            release_tx.send(()).unwrap();
            merge.join().unwrap().unwrap()
        });

        assert_eq!(receipt.moved_sessions, 1);
        assert!(!fixture.project_exists("project-2"));
        assert_eq!(fixture.session_project_for_title("归并源会话"), "project-1");
        assert_eq!(fixture.backup_count(), 1);
        assert_eq!(process_check_calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn account_switch_and_archive_callers_share_the_master_lease() {
        let fixture = Fixture::new();
        let stale_session_id = fixture.session_id();
        let app = tauri::test::mock_builder()
            .build(tauri::generate_context!())
            .unwrap();
        let app_handle = app.handle().clone();
        let material_root = fixture.material_root.clone();
        let storage_root = fixture.storage_root.clone();
        let recovery_root = fixture.recovery_root.clone();
        let master_dir = fixture.master_dir.clone();
        let raw_key = fixture.raw_key.clone();
        let switch_close_calls = Arc::new(AtomicUsize::new(0));
        let switch_relaunch_calls = Arc::new(AtomicUsize::new(0));
        let switch_close_calls_in_thread = switch_close_calls.clone();
        let switch_relaunch_calls_in_thread = switch_relaunch_calls.clone();
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let release_on_drop = release_tx.clone();
        let switch = thread::spawn(move || {
            let close = |_dir: &std::path::Path| {
                switch_close_calls_in_thread.fetch_add(1, Ordering::SeqCst);
                Ok(())
            };
            let launch = |_dir: &std::path::Path, _title: &str| {
                switch_relaunch_calls_in_thread.fetch_add(1, Ordering::SeqCst);
                Ok("test-launched".to_string())
            };
            let before_return = || {
                entered_tx.send(()).unwrap();
                release_rx.recv().unwrap();
            };
            switch_master_account_at_synthetic(
                &app_handle,
                &material_root,
                &storage_root,
                &recovery_root,
                &raw_key,
                "profile-2",
                &master_dir,
                true,
                &close,
                &launch,
                &before_return,
            )
        });
        let _release_switch_if_test_panics = ReleaseSwitchOnDrop(release_on_drop);
        if entered_rx.recv_timeout(Duration::from_secs(10)).is_err() {
            let _ = release_tx.send(());
            let _ = switch.join();
            panic!("合成切号未到达持锁提交后的屏障");
        }

        let close_calls = AtomicUsize::new(0);
        let relaunch_calls = AtomicUsize::new(0);
        let archive_while_switch_holds_lease =
            fixture.apply_session(&stale_session_id, &close_calls, &relaunch_calls);
        let auth_while_switch_holds_lease = archive_login_user_id(&fixture.master_dir);
        let owner_while_switch_holds_lease = fixture.project_owner();
        let status_while_switch_holds_lease = fixture.hidden_status();
        let backups_while_switch_holds_lease = fixture.backup_count();
        release_tx.send(()).unwrap();
        let switch_result = switch.join().unwrap();

        let error = archive_while_switch_holds_lease
            .err()
            .expect("竞争中的归档应在副作用前因租约失败");
        assert!(error.starts_with("operation_lease_unavailable:"));
        assert_eq!(close_calls.load(Ordering::SeqCst), 0);
        assert_eq!(relaunch_calls.load(Ordering::SeqCst), 0);
        assert_eq!(auth_while_switch_holds_lease.as_deref(), Some("account-2"));
        assert_eq!(owner_while_switch_holds_lease, "account-2");
        assert_eq!(status_while_switch_holds_lease, None);
        assert_eq!(backups_while_switch_holds_lease, 1);
        assert!(switch_result.is_ok());
        assert_eq!(switch_close_calls.load(Ordering::SeqCst), 1);
        assert_eq!(switch_relaunch_calls.load(Ordering::SeqCst), 1);

        // 备份名按 Unix 秒生成，等待跨秒以免测试夹具撞上同名备份。
        thread::sleep(Duration::from_millis(1100));
        let result = fixture
            .apply(&close_calls, &relaunch_calls)
            .expect("释放租约后真实归档编排应可完成");
        assert_eq!(result.archived_sessions, 1);
        assert_eq!(result.relaunch_outcome, "launched");
        assert_eq!(close_calls.load(Ordering::SeqCst), 1);
        assert_eq!(relaunch_calls.load(Ordering::SeqCst), 1);
        assert!(fixture.hidden_status().is_some());
        assert_eq!(fixture.backup_count(), 2);
    }

    /// 子进程执行真实主库切号，并在提交后持锁等待父进程释放。
    #[test]
    fn child_runs_master_switch_for_cross_process_archive_test() {
        if std::env::var("TRAE_SYNC_SWITCH_ARCHIVE_RUN")
            .ok()
            .as_deref()
            != Some("1")
        {
            return;
        }
        let fixture = Fixture {
            _root: tempfile::tempdir().unwrap(),
            recovery_root: std::env::var_os("TRAE_SYNC_SWITCH_ARCHIVE_RECOVERY_ROOT")
                .unwrap()
                .into(),
            storage_root: std::env::var_os("TRAE_SYNC_SWITCH_ARCHIVE_STORAGE_ROOT")
                .unwrap()
                .into(),
            material_root: std::env::var_os("TRAE_SYNC_SWITCH_ARCHIVE_MATERIAL_ROOT")
                .unwrap()
                .into(),
            master_dir: std::env::var_os("TRAE_SYNC_SWITCH_ARCHIVE_DATA_DIR")
                .unwrap()
                .into(),
            env_id: "master".to_string(),
            env_name: "主库".to_string(),
            raw_key: std::env::var("TRAE_SYNC_SWITCH_ARCHIVE_RAW_KEY").unwrap(),
        };
        let app = tauri::test::mock_builder()
            .build(tauri::generate_context!())
            .unwrap();
        let close = |_dir: &Path| Ok(());
        let launch = |_dir: &Path, _title: &str| Ok("test-launched".to_string());
        let after_commit = || {
            let mut stdout = std::io::stdout().lock();
            writeln!(stdout, "SWITCH_ARCHIVE_READY").unwrap();
            stdout.flush().unwrap();
            let mut line = String::new();
            let _ = std::io::stdin().lock().read_line(&mut line);
        };
        let result = switch_master_account_at_synthetic(
            app.handle(),
            &fixture.material_root,
            &fixture.storage_root,
            &fixture.recovery_root,
            &fixture.raw_key,
            "profile-2",
            &fixture.master_dir,
            true,
            &close,
            &launch,
            &after_commit,
        );
        assert!(result.is_ok(), "合成主库切号应提交成功");
    }

    /// 验证真实切号持锁提交后，另一进程的真实归档 caller 精确拒绝。
    #[test]
    fn master_switch_blocks_cross_process_work_archive_until_post_commit_release() {
        let fixture = Fixture::new();
        let mut switch = ChildProcessGuard(
            Command::new(std::env::current_exe().unwrap())
                .env("TRAE_SYNC_SWITCH_ARCHIVE_RUN", "1")
                .env("TRAE_SYNC_SWITCH_ARCHIVE_RECOVERY_ROOT", &fixture.recovery_root)
                .env("TRAE_SYNC_SWITCH_ARCHIVE_STORAGE_ROOT", &fixture.storage_root)
                .env("TRAE_SYNC_SWITCH_ARCHIVE_MATERIAL_ROOT", &fixture.material_root)
                .env("TRAE_SYNC_SWITCH_ARCHIVE_DATA_DIR", &fixture.master_dir)
                .env("TRAE_SYNC_SWITCH_ARCHIVE_RAW_KEY", &fixture.raw_key)
                .args([
                    "--exact",
                    "library_archive::p14_4c_archive_caller_tests::child_runs_master_switch_for_cross_process_archive_test",
                    "--nocapture",
                ])
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::inherit())
                .spawn()
                .expect("无法启动主库切号测试子进程"),
        );
        let mut switch_stdin = switch.0.stdin.take().unwrap();
        let switch_stdout = switch.0.stdout.take().unwrap();
        let (ready_tx, ready_rx) = mpsc::channel();
        let switch_reader = thread::spawn(move || {
            let mut reader = BufReader::new(switch_stdout);
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
                    Ok(_) if !ready_sent && line.trim() == "SWITCH_ARCHIVE_READY" => {
                        let _ = ready_tx.send(true);
                        ready_sent = true;
                    }
                    Ok(_) => {}
                }
            }
        });
        if ready_rx.recv_timeout(Duration::from_secs(10)) != Ok(true) {
            let _ = switch.0.kill();
            let _ = switch.0.wait();
            drop(switch_stdin);
            let _ = switch_reader.join();
            panic!("主库切号子进程未在 10 秒内到达提交后持锁屏障；已终止并回收");
        }

        let archive_status = match Command::new(std::env::current_exe().unwrap())
            .env("TRAE_SYNC_ARCHIVE_CALLER_RUN", "1")
            .env(
                "TRAE_SYNC_ARCHIVE_CALLER_RUN_RECOVERY_ROOT",
                &fixture.recovery_root,
            )
            .env(
                "TRAE_SYNC_ARCHIVE_CALLER_RUN_STORAGE_ROOT",
                &fixture.storage_root,
            )
            .env(
                "TRAE_SYNC_ARCHIVE_CALLER_RUN_MATERIAL_ROOT",
                &fixture.material_root,
            )
            .env(
                "TRAE_SYNC_ARCHIVE_CALLER_RUN_DATA_DIR",
                &fixture.master_dir,
            )
            .env("TRAE_SYNC_ARCHIVE_CALLER_RUN_RAW_KEY", &fixture.raw_key)
            .args([
                "--exact",
                "library_archive::p14_4c_archive_caller_tests::child_runs_work_archive_caller_for_cross_process_test",
                "--nocapture",
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .spawn()
        {
            Ok(child) => {
                let mut caller = ChildProcessGuard(child);
                wait_child_with_timeout(&mut caller.0, Duration::from_secs(10))
                    .map_err(|error| error.to_string())
            }
            Err(error) => Err(format!("无法启动 Work 归档 caller 测试子进程：{error}")),
        };

        let auth_while_switch_holds_lease = archive_login_user_id(&fixture.master_dir);
        let owner_while_switch_holds_lease = fixture.project_owner();
        let status_while_switch_holds_lease = fixture.hidden_status();
        let backups_while_switch_holds_lease = fixture.backup_count();
        let _ = switch_stdin.write_all(b"\n");
        drop(switch_stdin);
        let switch_status = wait_child_with_timeout(&mut switch.0, Duration::from_secs(10));
        let switch_reader_result = switch_reader.join();

        assert!(archive_status
            .expect("Work 归档 caller 必须在限时内完成")
            .success());
        assert!(switch_status
            .expect("主库切号 caller 必须在限时内完成")
            .success());
        assert!(switch_reader_result.is_ok());
        assert_eq!(auth_while_switch_holds_lease.as_deref(), Some("account-2"));
        assert_eq!(owner_while_switch_holds_lease, "account-2");
        assert_eq!(status_while_switch_holds_lease, None);
        assert_eq!(backups_while_switch_holds_lease, 1);
        assert_eq!(
            archive_login_user_id(&fixture.master_dir).as_deref(),
            Some("account-2")
        );
        assert_eq!(fixture.project_owner(), "account-2");
        assert_eq!(fixture.hidden_status(), None);
        assert_eq!(fixture.backup_count(), 1);
    }

    #[test]
    fn archive_and_account_switch_callers_share_the_master_lease_in_reverse_order() {
        let fixture = Fixture::new();
        let app = tauri::test::mock_builder()
            .build(tauri::generate_context!())
            .unwrap();
        let app_handle = app.handle().clone();
        let recovery_root = fixture.recovery_root.clone();
        let storage_root = fixture.storage_root.clone();
        let material_root = fixture.material_root.clone();
        let master_dir = fixture.master_dir.clone();
        let raw_key = fixture.raw_key.clone();
        let session_id = fixture.session_id();
        let archive_close_calls = Arc::new(AtomicUsize::new(0));
        let archive_relaunch_calls = Arc::new(AtomicUsize::new(0));
        let archive_close_calls_in_thread = archive_close_calls.clone();
        let archive_relaunch_calls_in_thread = archive_relaunch_calls.clone();
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let release_on_drop = release_tx.clone();
        let archive = thread::spawn(move || {
            let close = |_ctx: &LibraryContext| {
                archive_close_calls_in_thread.fetch_add(1, Ordering::SeqCst);
                Ok(())
            };
            let relaunch = |_ctx: &LibraryContext, _title: &str| {
                archive_relaunch_calls_in_thread.fetch_add(1, Ordering::SeqCst);
                Ok("launched".to_string())
            };
            let ensure_cn_stopped = |_ctx: &LibraryContext| Ok(());
            let after_commit = || {
                entered_tx.send(()).unwrap();
                release_rx.recv().unwrap();
            };
            apply_library_archive_for_context(
                &recovery_root,
                &storage_root,
                &material_root,
                &raw_key,
                LibraryContext {
                    env_id: "master".to_string(),
                    env_name: "主库".to_string(),
                    dir: master_dir.clone(),
                    read_only: false,
                },
                None,
                &[session_id],
                &[],
                &close,
                &relaunch,
                &ensure_cn_stopped,
                &after_commit,
            )
        });
        let _release_archive_if_test_panics = ReleaseSwitchOnDrop(release_on_drop);
        if entered_rx.recv_timeout(Duration::from_secs(10)).is_err() {
            let _ = release_tx.send(());
            let _ = archive.join();
            panic!("合成归档未到达提交后持锁屏障");
        }

        let switch_close_calls = AtomicUsize::new(0);
        let switch_launch_calls = AtomicUsize::new(0);
        let close_switch = |_dir: &std::path::Path| {
            switch_close_calls.fetch_add(1, Ordering::SeqCst);
            Ok(())
        };
        let launch_switch = |_dir: &std::path::Path, _title: &str| {
            switch_launch_calls.fetch_add(1, Ordering::SeqCst);
            Ok("test-launched".to_string())
        };
        let switch_while_archive_holds_lease = switch_master_account_at_synthetic(
            &app_handle,
            &fixture.material_root,
            &fixture.storage_root,
            &fixture.recovery_root,
            &fixture.raw_key,
            "profile-2",
            &fixture.master_dir,
            true,
            &close_switch,
            &launch_switch,
            &|| {},
        );
        let auth_while_archive_holds_lease = archive_login_user_id(&fixture.master_dir);
        let owner_while_archive_holds_lease = fixture.project_owner();
        let status_while_archive_holds_lease = fixture.hidden_status();
        let backups_while_archive_holds_lease = fixture.backup_count();
        release_tx.send(()).unwrap();
        let archive_result = archive.join().unwrap();

        let error = switch_while_archive_holds_lease
            .err()
            .expect("竞争中的切号应在副作用前因租约失败");
        assert!(error.starts_with("operation_lease_unavailable:"));
        assert_eq!(switch_close_calls.load(Ordering::SeqCst), 0);
        assert_eq!(switch_launch_calls.load(Ordering::SeqCst), 0);
        assert_eq!(auth_while_archive_holds_lease.as_deref(), Some("account-1"));
        assert_eq!(owner_while_archive_holds_lease, "account-1");
        assert!(status_while_archive_holds_lease.is_some());
        assert_eq!(backups_while_archive_holds_lease, 1);
        assert_eq!(archive_result.unwrap().archived_sessions, 1);
        assert_eq!(archive_close_calls.load(Ordering::SeqCst), 1);
        assert_eq!(archive_relaunch_calls.load(Ordering::SeqCst), 1);

        // 备份名按 Unix 秒生成，等待跨秒以免测试夹具撞上同名备份。
        thread::sleep(Duration::from_millis(1100));
        let switch_result = switch_master_account_at_synthetic(
            &app_handle,
            &fixture.material_root,
            &fixture.storage_root,
            &fixture.recovery_root,
            &fixture.raw_key,
            "profile-2",
            &fixture.master_dir,
            true,
            &close_switch,
            &launch_switch,
            &|| {},
        );
        assert!(
            switch_result.is_ok(),
            "归档完成后切号重试失败：{}",
            switch_result
                .as_ref()
                .err()
                .map(String::as_str)
                .unwrap_or("未知错误")
        );
        assert_eq!(switch_close_calls.load(Ordering::SeqCst), 1);
        assert_eq!(switch_launch_calls.load(Ordering::SeqCst), 1);
        assert_eq!(
            archive_login_user_id(&fixture.master_dir).as_deref(),
            Some("account-2")
        );
        assert_eq!(fixture.project_owner(), "account-1");
        assert!(fixture.hidden_status().is_some());
        assert_eq!(fixture.backup_count(), 2);
    }

    /// 关闭失败发生在备份之前，真实 caller 应保留切换前状态且不继续后续步骤。
    #[test]
    fn master_switch_close_failure_leaves_master_unchanged() {
        for (close_error, expected_error) in [
            (
                TraeInstanceCloseError::CloseFailed,
                "master_switch_close_failed",
            ),
            (
                TraeInstanceCloseError::ProcessStateUnknown,
                "process_state_unknown",
            ),
        ] {
            let fixture = Fixture::new();
            let registry = EnvironmentRegistry::new(&fixture.storage_root);
            registry.set_current_profile("master", "profile-1").unwrap();
            let app = tauri::test::mock_builder()
                .build(tauri::generate_context!())
                .unwrap();
            let close_calls = AtomicUsize::new(0);
            let launch_calls = AtomicUsize::new(0);
            let before_return_calls = AtomicUsize::new(0);
            let close = |_dir: &Path| {
                close_calls.fetch_add(1, Ordering::SeqCst);
                Err(close_error)
            };
            let launch = |_dir: &Path, _title: &str| {
                launch_calls.fetch_add(1, Ordering::SeqCst);
                Ok("unexpected_launch".to_string())
            };
            let before_return = || {
                before_return_calls.fetch_add(1, Ordering::SeqCst);
            };

            let error = switch_master_account_at_synthetic(
                app.handle(),
                &fixture.material_root,
                &fixture.storage_root,
                &fixture.recovery_root,
                &fixture.raw_key,
                "profile-2",
                &fixture.master_dir,
                true,
                &close,
                &launch,
                &before_return,
            )
            .err()
            .expect("关闭失败或进程状态未知时切号应返回错误");

            assert_eq!(error, expected_error);
            assert_eq!(close_calls.load(Ordering::SeqCst), 1);
            assert_eq!(launch_calls.load(Ordering::SeqCst), 0);
            assert_eq!(before_return_calls.load(Ordering::SeqCst), 0);
            assert_eq!(
                archive_login_user_id(&fixture.master_dir).as_deref(),
                Some("account-1")
            );
            assert_eq!(fixture.project_owner(), "account-1");
            assert_eq!(fixture.backup_count(), 0);
            assert_eq!(
                registry
                    .find("master")
                    .unwrap()
                    .unwrap()
                    .current_profile_id
                    .as_deref(),
                Some("profile-1")
            );
        }
    }

    /// Work 归档无法确认实例身份时保留原因码，并在备份和数据库提交前停止。
    #[test]
    fn work_archive_unknown_process_identity_stops_before_backup() {
        let fixture = Fixture::new();
        let close_calls = AtomicUsize::new(0);
        let relaunch_calls = AtomicUsize::new(0);
        let ctx = fixture.context();
        let session_id = fixture.session_id();
        let close = |_ctx: &LibraryContext| {
            close_calls.fetch_add(1, Ordering::SeqCst);
            Err(TraeInstanceCloseError::ProcessStateUnknown)
        };
        let relaunch = |_ctx: &LibraryContext, _title: &str| {
            relaunch_calls.fetch_add(1, Ordering::SeqCst);
            Ok("unexpected_launch".to_string())
        };
        let ensure_cn_stopped = |_ctx: &LibraryContext| Ok(());

        let error = apply_library_archive_for_context(
            &fixture.recovery_root,
            &fixture.storage_root,
            &fixture.material_root,
            &fixture.raw_key,
            ctx,
            None,
            &[session_id],
            &[],
            &close,
            &relaunch,
            &ensure_cn_stopped,
            &|| {},
        )
        .unwrap_err();

        assert_eq!(error, "process_state_unknown");
        assert_eq!(close_calls.load(Ordering::SeqCst), 1);
        assert_eq!(relaunch_calls.load(Ordering::SeqCst), 0);
        assert_eq!(fixture.hidden_status(), None);
        assert_eq!(fixture.backup_count(), 0);
    }

    /// Work 归并的进程状态未知时，不备份也不移动源项目会话。
    #[test]
    fn work_project_merge_unknown_process_state_stops_before_backup() {
        let fixture = Fixture::new().as_secondary_environment();
        fixture.seed_merge_source();
        let (ctx, lifecycle_lease) = super::prepare_library_write_context(
            &fixture.recovery_root,
            &fixture.storage_root,
            Some(&fixture.env_id),
        )
        .unwrap();
        let process_check_calls = AtomicUsize::new(0);
        let ensure_stopped = |_ctx: &LibraryContext| {
            process_check_calls.fetch_add(1, Ordering::SeqCst);
            Err("process_state_unknown".to_string())
        };

        let error = run_master_project_merge_with_lease(
            &fixture.recovery_root,
            &fixture.storage_root,
            &fixture.storage_root,
            &fixture.material_root,
            &fixture.raw_key,
            ctx,
            lifecycle_lease,
            &["project-2".to_string()],
            "project-1",
            &ensure_stopped,
            &|| {},
        )
        .err()
        .expect("进程状态未知时 Work 项目归并应拒绝");

        assert_eq!(error, "process_state_unknown");
        assert_eq!(process_check_calls.load(Ordering::SeqCst), 1);
        assert_eq!(fixture.backup_count(), 0);
        assert_eq!(fixture.session_project_for_title("归并源会话"), "project-2");
    }

    /// 副环境登录遇到未知进程身份时，不备份、不写认证身份或账号归属。
    #[test]
    fn environment_login_unknown_process_state_stops_before_backup_or_auth_write() {
        let fixture = Fixture::new().as_secondary_environment();
        let close_calls = AtomicUsize::new(0);
        let identity_switch_calls = AtomicUsize::new(0);
        let before_return_calls = AtomicUsize::new(0);
        let close = |_dir: &Path| {
            close_calls.fetch_add(1, Ordering::SeqCst);
            Err(TraeInstanceCloseError::ProcessStateUnknown)
        };
        let switch_identity = |_record: &AccountRecord, _donor: &Path, _env: &Path| {
            identity_switch_calls.fetch_add(1, Ordering::SeqCst);
            Ok(())
        };
        let before_return = || {
            before_return_calls.fetch_add(1, Ordering::SeqCst);
        };

        let error = login_environment_inner_with_ops(
            &fixture.material_root,
            &fixture.storage_root,
            &fixture.recovery_root,
            &fixture.raw_key,
            &fixture.env_id,
            "profile-2",
            &close,
            &switch_identity,
            &before_return,
        )
        .err()
        .expect("进程状态未知时副环境登录应拒绝");

        assert_eq!(error, "process_state_unknown");
        assert_eq!(close_calls.load(Ordering::SeqCst), 1);
        assert_eq!(identity_switch_calls.load(Ordering::SeqCst), 0);
        assert_eq!(before_return_calls.load(Ordering::SeqCst), 0);
        assert_eq!(
            archive_login_user_id(&fixture.master_dir).as_deref(),
            Some("account-1")
        );
        assert_eq!(fixture.project_owner(), "account-1");
        assert_eq!(fixture.backup_count(), 0);
        assert_eq!(
            EnvironmentRegistry::new(&fixture.storage_root)
                .find(&fixture.env_id)
                .unwrap()
                .unwrap()
                .current_profile_id
                .as_deref(),
            Some("profile-1")
        );
    }

    /// 目标账号存在同目录非空项目时应拒绝交接并恢复先前改写的认证。
    #[test]
    fn master_switch_precommit_conflict_rolls_back_auth_and_database() {
        let fixture = Fixture::new();
        let registry = EnvironmentRegistry::new(&fixture.storage_root);
        registry.set_current_profile("master", "profile-1").unwrap();
        let old_session_id = fixture.session_id();
        let conn = Connection::open_with_flags(
            master_database_path(&fixture.master_dir),
            OpenFlags::SQLITE_OPEN_READ_WRITE,
        )
        .unwrap();
        conn.execute_batch(&format!("PRAGMA key = \"x'{}'\";", fixture.raw_key))
            .unwrap();
        conn.execute_batch(
            "INSERT INTO project VALUES ('project-conflict', 'account-2', 'biz-1', '目标项目', NULL);
             INSERT INTO chat_session VALUES (
                'session-conflict', 'project-conflict', '目标会话', 2, NULL, NULL);",
        )
        .unwrap();
        drop(conn);

        let app = tauri::test::mock_builder()
            .build(tauri::generate_context!())
            .unwrap();
        let close_calls = AtomicUsize::new(0);
        let launch_calls = AtomicUsize::new(0);
        let before_return_calls = AtomicUsize::new(0);
        let close = |_dir: &Path| {
            close_calls.fetch_add(1, Ordering::SeqCst);
            Ok(())
        };
        let launch = |_dir: &Path, _title: &str| {
            launch_calls.fetch_add(1, Ordering::SeqCst);
            Ok("unexpected_launch".to_string())
        };
        let before_return = || {
            before_return_calls.fetch_add(1, Ordering::SeqCst);
        };

        let error = switch_master_account_at_synthetic(
            app.handle(),
            &fixture.material_root,
            &fixture.storage_root,
            &fixture.recovery_root,
            &fixture.raw_key,
            "profile-2",
            &fixture.master_dir,
            true,
            &close,
            &launch,
            &before_return,
        )
        .err()
        .expect("同目录非空项目冲突应拒绝主库切换");

        assert_eq!(error, "master_switch_conflict");
        assert_eq!(close_calls.load(Ordering::SeqCst), 1);
        assert_eq!(launch_calls.load(Ordering::SeqCst), 0);
        assert_eq!(before_return_calls.load(Ordering::SeqCst), 0);
        assert_eq!(
            archive_login_user_id(&fixture.master_dir).as_deref(),
            Some("account-1")
        );
        assert_eq!(fixture.project_owner(), "account-1");
        assert_eq!(fixture.session_id(), old_session_id);
        assert_eq!(fixture.backup_count(), 1);
        let conn = Connection::open_with_flags(
            master_database_path(&fixture.master_dir),
            OpenFlags::SQLITE_OPEN_READ_WRITE,
        )
        .unwrap();
        conn.execute_batch(&format!("PRAGMA key = \"x'{}'\";", fixture.raw_key))
            .unwrap();
        let target_owner: String = conn
            .query_row(
                "SELECT user_id FROM project WHERE project_id = 'project-conflict'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(target_owner, "account-2");
        assert_eq!(
            registry
                .find("master")
                .unwrap()
                .unwrap()
                .current_profile_id
                .as_deref(),
            Some("profile-1")
        );
    }

    /// 事务内较早的归属更新后再失败时，事务与已写认证都应一同恢复。
    #[test]
    fn master_switch_precommit_db_failure_rolls_back_auth_and_transaction() {
        let fixture = Fixture::new();
        let registry = EnvironmentRegistry::new(&fixture.storage_root);
        registry.set_current_profile("master", "profile-1").unwrap();
        let old_session_id = fixture.session_id();
        let conn = Connection::open_with_flags(
            master_database_path(&fixture.master_dir),
            OpenFlags::SQLITE_OPEN_READ_WRITE,
        )
        .unwrap();
        conn.execute_batch(&format!("PRAGMA key = \"x'{}'\";", fixture.raw_key))
            .unwrap();
        conn.execute_batch(
            "CREATE TRIGGER reject_session_id_rewrite
             BEFORE UPDATE OF session_id ON chat_session
             WHEN OLD.session_id = 'session-1'
             BEGIN
               SELECT RAISE(ABORT, 'synthetic_precommit_failure');
             END;",
        )
        .unwrap();
        drop(conn);

        let app = tauri::test::mock_builder()
            .build(tauri::generate_context!())
            .unwrap();
        let close_calls = AtomicUsize::new(0);
        let launch_calls = AtomicUsize::new(0);
        let before_return_calls = AtomicUsize::new(0);
        let close = |_dir: &Path| {
            close_calls.fetch_add(1, Ordering::SeqCst);
            Ok(())
        };
        let launch = |_dir: &Path, _title: &str| {
            launch_calls.fetch_add(1, Ordering::SeqCst);
            Ok("unexpected_launch".to_string())
        };
        let before_return = || {
            before_return_calls.fetch_add(1, Ordering::SeqCst);
        };

        let error = switch_master_account_at_synthetic(
            app.handle(),
            &fixture.material_root,
            &fixture.storage_root,
            &fixture.recovery_root,
            &fixture.raw_key,
            "profile-2",
            &fixture.master_dir,
            true,
            &close,
            &launch,
            &before_return,
        )
        .err()
        .expect("事务内写入失败应返回错误");

        assert_eq!(error, "master_switch_db_open_failed");
        assert_eq!(close_calls.load(Ordering::SeqCst), 1);
        assert_eq!(launch_calls.load(Ordering::SeqCst), 0);
        assert_eq!(before_return_calls.load(Ordering::SeqCst), 0);
        assert_eq!(
            archive_login_user_id(&fixture.master_dir).as_deref(),
            Some("account-1")
        );
        assert_eq!(fixture.project_owner(), "account-1");
        assert_eq!(fixture.session_id(), old_session_id);
        assert_eq!(fixture.backup_count(), 1);
        assert_eq!(
            registry
                .find("master")
                .unwrap()
                .unwrap()
                .current_profile_id
                .as_deref(),
            Some("profile-1")
        );
    }

    /// 写后完整性校验失败表示事务已提交，caller 不得回滚认证或伪报未提交。
    #[test]
    fn master_switch_committed_integrity_failure_preserves_committed_state() {
        let fixture = Fixture::new();
        let registry = EnvironmentRegistry::new(&fixture.storage_root);
        registry.set_current_profile("master", "profile-1").unwrap();
        let old_session_id = fixture.session_id();
        let conn = Connection::open_with_flags(
            master_database_path(&fixture.master_dir),
            OpenFlags::SQLITE_OPEN_READ_WRITE,
        )
        .unwrap();
        conn.execute_batch(&format!("PRAGMA key = \"x'{}'\";", fixture.raw_key))
            .unwrap();
        conn.execute(
            "INSERT INTO chat_message
                (message_id, session_id, message_role, message_index,
                 reply_to_message_id, response_message_id, deleted_at)
             VALUES (?1, ?2, 'user', 0, NULL, NULL, NULL)",
            rusqlite::params!["integrity-message", old_session_id],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO chat_message_general (message_id, content, created_at)
             VALUES (?1, '交接前正文', '2026-10-03')",
            ["integrity-message"],
        )
        .unwrap();
        conn.execute_batch(
            "CREATE TRIGGER mutate_transferred_message
             AFTER UPDATE OF user_id ON project
             WHEN NEW.user_id = 'account-2'
             BEGIN
               UPDATE chat_message_general SET content = '合成写后不一致'
               WHERE message_id = 'integrity-message';
             END;",
        )
        .unwrap();
        drop(conn);

        let app = tauri::test::mock_builder()
            .build(tauri::generate_context!())
            .unwrap();
        let close_calls = AtomicUsize::new(0);
        let launch_calls = AtomicUsize::new(0);
        let before_return_calls = AtomicUsize::new(0);
        let close = |_dir: &Path| {
            close_calls.fetch_add(1, Ordering::SeqCst);
            Ok(())
        };
        let launch = |_dir: &Path, _title: &str| {
            launch_calls.fetch_add(1, Ordering::SeqCst);
            Ok("unexpected_launch".to_string())
        };
        let before_return = || {
            before_return_calls.fetch_add(1, Ordering::SeqCst);
        };

        let error = switch_master_account_at_synthetic(
            app.handle(),
            &fixture.material_root,
            &fixture.storage_root,
            &fixture.recovery_root,
            &fixture.raw_key,
            "profile-2",
            &fixture.master_dir,
            true,
            &close,
            &launch,
            &before_return,
        )
        .err()
        .expect("写后完整性校验失败应返回需检查错误");

        assert_eq!(error, "master_switch_committed_needs_attention");
        assert_eq!(close_calls.load(Ordering::SeqCst), 1);
        assert_eq!(launch_calls.load(Ordering::SeqCst), 0);
        assert_eq!(before_return_calls.load(Ordering::SeqCst), 0);
        assert_eq!(
            archive_login_user_id(&fixture.master_dir).as_deref(),
            Some("account-2")
        );
        assert_eq!(fixture.project_owner(), "account-2");
        assert_ne!(fixture.session_id(), old_session_id);
        assert_eq!(fixture.backup_count(), 1);
        assert_eq!(
            registry
                .find("master")
                .unwrap()
                .unwrap()
                .current_profile_id
                .as_deref(),
            Some("profile-1")
        );
    }

    /// 主库数据已切换但重启失败时，真实 caller 应返回已提交的失败回执。
    #[test]
    fn master_switch_relaunch_failure_preserves_committed_receipt() {
        let fixture = Fixture::new();
        let app = tauri::test::mock_builder()
            .build(tauri::generate_context!())
            .unwrap();
        let close_calls = AtomicUsize::new(0);
        let launch_calls = AtomicUsize::new(0);
        let before_return_calls = AtomicUsize::new(0);
        let close = |_dir: &Path| {
            close_calls.fetch_add(1, Ordering::SeqCst);
            Ok(())
        };
        let launch = |_dir: &Path, _title: &str| {
            launch_calls.fetch_add(1, Ordering::SeqCst);
            Err("synthetic_launch_failure".to_string())
        };
        let before_return = || {
            before_return_calls.fetch_add(1, Ordering::SeqCst);
        };

        let receipt = switch_master_account_at_synthetic(
            app.handle(),
            &fixture.material_root,
            &fixture.storage_root,
            &fixture.recovery_root,
            &fixture.raw_key,
            "profile-2",
            &fixture.master_dir,
            true,
            &close,
            &launch,
            &before_return,
        )
        .expect("重启失败不能把已提交的主库切换报告为整笔失败");
        let receipt = serde_json::to_value(receipt).unwrap();

        assert_eq!(receipt["relaunch_outcome"].as_str(), Some("failed"));
        assert_eq!(receipt["current_profile_saved"].as_bool(), Some(true));
        assert_eq!(receipt["relay_ledger_written"].as_bool(), Some(true));
        assert_eq!(close_calls.load(Ordering::SeqCst), 1);
        assert_eq!(launch_calls.load(Ordering::SeqCst), 1);
        assert_eq!(before_return_calls.load(Ordering::SeqCst), 1);
        assert_eq!(
            archive_login_user_id(&fixture.master_dir).as_deref(),
            Some("account-2")
        );
        assert_eq!(fixture.project_owner(), "account-2");
        assert_eq!(fixture.backup_count(), 1);
        assert_eq!(
            EnvironmentRegistry::new(&fixture.storage_root)
                .find("master")
                .unwrap()
                .unwrap()
                .current_profile_id
                .as_deref(),
            Some("profile-2")
        );
    }

    /// 副环境登录的 handover 已提交但校验失败时，保留认证与数据库实际状态。
    #[test]
    fn environment_login_committed_integrity_failure_preserves_state() {
        let fixture = Fixture::new().as_secondary_environment();
        let old_session_id = fixture.session_id();
        let conn = Connection::open_with_flags(
            master_database_path(&fixture.master_dir),
            OpenFlags::SQLITE_OPEN_READ_WRITE,
        )
        .unwrap();
        conn.execute_batch(&format!("PRAGMA key = \"x'{}'\";", fixture.raw_key))
            .unwrap();
        conn.execute(
            "INSERT INTO chat_message
                (message_id, session_id, message_role, message_index,
                 reply_to_message_id, response_message_id, deleted_at)
             VALUES (?1, ?2, 'user', 0, NULL, NULL, NULL)",
            rusqlite::params!["environment-integrity-message", old_session_id],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO chat_message_general (message_id, content, created_at)
             VALUES (?1, '交接前正文', '2026-10-03')",
            ["environment-integrity-message"],
        )
        .unwrap();
        conn.execute_batch(
            "CREATE TRIGGER mutate_environment_transferred_message
             AFTER UPDATE OF user_id ON project
             WHEN NEW.user_id = 'account-2'
             BEGIN
               UPDATE chat_message_general SET content = '合成写后不一致'
               WHERE message_id = 'environment-integrity-message';
             END;",
        )
        .unwrap();
        drop(conn);

        let close_calls = AtomicUsize::new(0);
        let identity_calls = AtomicUsize::new(0);
        let before_return_calls = AtomicUsize::new(0);
        let before_return = || {
            before_return_calls.fetch_add(1, Ordering::SeqCst);
        };
        let error = run_environment_login(
            &fixture.material_root,
            &fixture.storage_root,
            &fixture.recovery_root,
            &fixture.raw_key,
            &fixture.env_id,
            "profile-2",
            &close_calls,
            &identity_calls,
            &before_return,
        )
        .err()
        .expect("提交后完整性失败应保留已提交状态并报告需检查");

        assert_eq!(error, "environment_login_committed_needs_attention");
        assert_eq!(close_calls.load(Ordering::SeqCst), 1);
        assert_eq!(identity_calls.load(Ordering::SeqCst), 1);
        assert_eq!(before_return_calls.load(Ordering::SeqCst), 0);
        assert_eq!(
            archive_login_user_id(&fixture.master_dir).as_deref(),
            Some("account-2")
        );
        assert_eq!(fixture.project_owner(), "account-2");
        assert_ne!(fixture.session_id(), old_session_id);
        assert_eq!(fixture.backup_count(), 1);
        assert_eq!(
            EnvironmentRegistry::new(&fixture.storage_root)
                .find(&fixture.env_id)
                .unwrap()
                .unwrap()
                .current_profile_id
                .as_deref(),
            Some("profile-1")
        );
    }

    /// 副环境登录在 handover 写事务开始前遇到目录冲突时，应恢复认证且不交接数据库。
    #[test]
    fn environment_login_precommit_conflict_rolls_back_auth_and_database() {
        let fixture = Fixture::new().as_secondary_environment();
        let old_session_id = fixture.session_id();
        let conn = Connection::open_with_flags(
            master_database_path(&fixture.master_dir),
            OpenFlags::SQLITE_OPEN_READ_WRITE,
        )
        .unwrap();
        conn.execute_batch(&format!("PRAGMA key = \"x'{}'\";", fixture.raw_key))
            .unwrap();
        conn.execute_batch(
            "INSERT INTO project VALUES ('project-conflict', 'account-2', 'biz-1', '目标项目', NULL);
             INSERT INTO chat_session VALUES (
                'session-conflict', 'project-conflict', '目标会话', 2, NULL, NULL);",
        )
        .unwrap();
        drop(conn);

        let close_calls = AtomicUsize::new(0);
        let identity_calls = AtomicUsize::new(0);
        let before_return_calls = AtomicUsize::new(0);
        let before_return = || {
            before_return_calls.fetch_add(1, Ordering::SeqCst);
        };
        let error = run_environment_login(
            &fixture.material_root,
            &fixture.storage_root,
            &fixture.recovery_root,
            &fixture.raw_key,
            &fixture.env_id,
            "profile-2",
            &close_calls,
            &identity_calls,
            &before_return,
        )
        .err()
        .expect("同目录非空项目冲突应拒绝副环境登录");

        assert_eq!(error, "environment_login_conflict");
        assert_eq!(close_calls.load(Ordering::SeqCst), 1);
        assert_eq!(identity_calls.load(Ordering::SeqCst), 1);
        assert_eq!(before_return_calls.load(Ordering::SeqCst), 0);
        assert_eq!(
            archive_login_user_id(&fixture.master_dir).as_deref(),
            Some("account-1")
        );
        assert_eq!(fixture.project_owner(), "account-1");
        assert_eq!(fixture.session_id(), old_session_id);
        assert_eq!(fixture.backup_count(), 1);
        let conn = Connection::open_with_flags(
            master_database_path(&fixture.master_dir),
            OpenFlags::SQLITE_OPEN_READ_WRITE,
        )
        .unwrap();
        conn.execute_batch(&format!("PRAGMA key = \"x'{}'\";", fixture.raw_key))
            .unwrap();
        let target_owner: String = conn
            .query_row(
                "SELECT user_id FROM project WHERE project_id = 'project-conflict'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(target_owner, "account-2");
        assert_eq!(
            EnvironmentRegistry::new(&fixture.storage_root)
                .find(&fixture.env_id)
                .unwrap()
                .unwrap()
                .current_profile_id
                .as_deref(),
            Some("profile-1")
        );
    }

    #[test]
    fn environment_login_and_archive_share_the_secondary_environment_lease() {
        let fixture = Fixture::new().as_secondary_environment();
        let session_id_before_login = fixture.session_id();
        let material_root = fixture.material_root.clone();
        let storage_root = fixture.storage_root.clone();
        let recovery_root = fixture.recovery_root.clone();
        let raw_key = fixture.raw_key.clone();
        let env_id = fixture.env_id.clone();
        let login_close_calls = Arc::new(AtomicUsize::new(0));
        let login_auth_calls = Arc::new(AtomicUsize::new(0));
        let login_close_calls_thread = login_close_calls.clone();
        let login_auth_calls_thread = login_auth_calls.clone();
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let login = thread::spawn(move || {
            let before_return = || {
                entered_tx.send(()).unwrap();
                release_rx.recv().unwrap();
            };
            run_environment_login(
                &material_root,
                &storage_root,
                &recovery_root,
                &raw_key,
                &env_id,
                "profile-2",
                &login_close_calls_thread,
                &login_auth_calls_thread,
                &before_return,
            )
        });
        let _release_login_if_test_panics = ReleaseSwitchOnDrop(release_tx.clone());
        if entered_rx.recv_timeout(Duration::from_secs(10)).is_err() {
            let _ = release_tx.send(());
            let login_error = login.join().unwrap().err();
            panic!("合成环境登录未到达持锁完成屏障；错误={login_error:?}");
        }

        let archive_close_calls = AtomicUsize::new(0);
        let archive_relaunch_calls = AtomicUsize::new(0);
        let archive_while_login_holds_lease = fixture.apply_environment_session_after_commit(
            &fixture.session_id(),
            &archive_close_calls,
            &archive_relaunch_calls,
            &|| {},
        );
        assert!(archive_while_login_holds_lease
            .err()
            .expect("竞争归档必须因真实环境租约拒绝")
            .starts_with("operation_lease_unavailable:"));
        assert_eq!(archive_close_calls.load(Ordering::SeqCst), 0);
        assert_eq!(archive_relaunch_calls.load(Ordering::SeqCst), 0);
        assert_eq!(login_close_calls.load(Ordering::SeqCst), 1);
        assert_eq!(login_auth_calls.load(Ordering::SeqCst), 1);
        assert_eq!(
            archive_login_user_id(&fixture.master_dir).as_deref(),
            Some("account-2")
        );
        assert_eq!(fixture.project_owner(), "account-2");
        assert_eq!(fixture.hidden_status(), None);
        assert_eq!(fixture.backup_count(), 1);

        release_tx.send(()).unwrap();
        assert!(login.join().unwrap().is_ok());

        // 备份名精度为秒，确保测试里的后续归档不会与登录备份撞名。
        thread::sleep(Duration::from_millis(1100));
        let archive_result = fixture
            .apply_environment_session_after_commit(
                &fixture.session_id(),
                &archive_close_calls,
                &archive_relaunch_calls,
                &|| {},
            )
            .expect("环境登录释放租约后，归档 writer 应可成功");
        assert_eq!(archive_result.archived_sessions, 1);
        assert!(fixture.hidden_status().is_some());
        assert_eq!(archive_close_calls.load(Ordering::SeqCst), 1);
        assert_eq!(archive_relaunch_calls.load(Ordering::SeqCst), 1);
        assert_eq!(fixture.backup_count(), 2);
        assert_ne!(session_id_before_login, fixture.session_id());
    }

    #[test]
    fn archive_and_environment_login_share_the_secondary_environment_lease_in_reverse_order() {
        let fixture = Fixture::new().as_secondary_environment();
        let session_id = fixture.session_id();
        let recovery_root = fixture.recovery_root.clone();
        let storage_root = fixture.storage_root.clone();
        let material_root = fixture.material_root.clone();
        let raw_key = fixture.raw_key.clone();
        let env_id = fixture.env_id.clone();
        let archive_close_calls = Arc::new(AtomicUsize::new(0));
        let archive_relaunch_calls = Arc::new(AtomicUsize::new(0));
        let archive_close_calls_thread = archive_close_calls.clone();
        let archive_relaunch_calls_thread = archive_relaunch_calls.clone();
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let archive = thread::spawn(move || {
            let after_commit = || {
                entered_tx.send(()).unwrap();
                release_rx.recv().unwrap();
            };
            run_environment_archive(
                &recovery_root,
                &storage_root,
                &material_root,
                &raw_key,
                &env_id,
                &session_id,
                &archive_close_calls_thread,
                &archive_relaunch_calls_thread,
                &after_commit,
            )
        });
        let _release_archive_if_test_panics = ReleaseSwitchOnDrop(release_tx.clone());
        if entered_rx.recv_timeout(Duration::from_secs(10)).is_err() {
            let _ = release_tx.send(());
            let _ = archive.join();
            panic!("合成环境归档未到达提交后持锁屏障");
        }

        let login_close_calls = AtomicUsize::new(0);
        let login_auth_calls = AtomicUsize::new(0);
        let busy = run_environment_login(
            &fixture.material_root,
            &fixture.storage_root,
            &fixture.recovery_root,
            &fixture.raw_key,
            &fixture.env_id,
            "profile-2",
            &login_close_calls,
            &login_auth_calls,
            &|| {},
        );
        assert!(busy
            .err()
            .expect("归档提交期间，环境登录必须因目标目录租约拒绝")
            .starts_with("operation_lease_unavailable:"));
        assert_eq!(login_close_calls.load(Ordering::SeqCst), 0);
        assert_eq!(login_auth_calls.load(Ordering::SeqCst), 0);
        assert_eq!(
            archive_login_user_id(&fixture.master_dir).as_deref(),
            Some("account-1")
        );
        assert_eq!(fixture.project_owner(), "account-1");
        assert!(fixture.hidden_status().is_some());
        assert_eq!(fixture.backup_count(), 1);

        release_tx.send(()).unwrap();
        assert_eq!(archive.join().unwrap().unwrap().archived_sessions, 1);
        assert_eq!(archive_close_calls.load(Ordering::SeqCst), 1);
        assert_eq!(archive_relaunch_calls.load(Ordering::SeqCst), 1);

        // 备份文件名按 Unix 秒唯一；让归档和后续登录的串行备份落在不同秒。
        thread::sleep(Duration::from_millis(1100));
        let login_after_archive = run_environment_login(
            &fixture.material_root,
            &fixture.storage_root,
            &fixture.recovery_root,
            &fixture.raw_key,
            &fixture.env_id,
            "profile-2",
            &login_close_calls,
            &login_auth_calls,
            &|| {},
        );
        assert!(
            login_after_archive.is_ok(),
            "归档释放租约后登录重试失败：{:?}",
            login_after_archive.err()
        );
        assert_eq!(login_close_calls.load(Ordering::SeqCst), 1);
        assert_eq!(login_auth_calls.load(Ordering::SeqCst), 1);
        assert_eq!(
            EnvironmentRegistry::new(&fixture.storage_root)
                .find(&fixture.env_id)
                .unwrap()
                .unwrap()
                .current_profile_id
                .as_deref(),
            Some("profile-2")
        );
        assert_eq!(
            archive_login_user_id(&fixture.master_dir).as_deref(),
            Some("account-2")
        );
        assert_eq!(fixture.project_owner(), "account-1");
        assert!(fixture.hidden_status().is_some());
        assert_eq!(fixture.backup_count(), 2);
    }

    #[test]
    fn environment_login_and_work_project_merge_share_the_secondary_environment_lease() {
        let fixture = Fixture::new().as_secondary_environment();
        fixture.seed_merge_source();
        let material_root = fixture.material_root.clone();
        let storage_root = fixture.storage_root.clone();
        let recovery_root = fixture.recovery_root.clone();
        let raw_key = fixture.raw_key.clone();
        let env_id = fixture.env_id.clone();
        let login_close_calls = Arc::new(AtomicUsize::new(0));
        let login_auth_calls = Arc::new(AtomicUsize::new(0));
        let login_close_thread = login_close_calls.clone();
        let login_auth_thread = login_auth_calls.clone();
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let login = thread::spawn(move || {
            let before_return = || {
                entered_tx.send(()).unwrap();
                release_rx.recv().unwrap();
            };
            run_environment_login(
                &material_root,
                &storage_root,
                &recovery_root,
                &raw_key,
                &env_id,
                "profile-2",
                &login_close_thread,
                &login_auth_thread,
                &before_return,
            )
        });
        let _release_login_if_test_panics = ReleaseSwitchOnDrop(release_tx.clone());
        if entered_rx.recv_timeout(Duration::from_secs(10)).is_err() {
            let _ = release_tx.send(());
            let login_error = login.join().unwrap().err();
            panic!("合成环境登录未到达归并竞争屏障；错误={login_error:?}");
        }

        let process_check_calls = AtomicUsize::new(0);
        let merge_while_login_holds_lease = run_work_project_merge(
            &fixture,
            "project-2",
            "project-1",
            &process_check_calls,
            &|| {},
        );
        assert!(merge_while_login_holds_lease
            .err()
            .expect("环境登录期间，项目归并应在进程检查前因租约拒绝")
            .starts_with("operation_lease_unavailable:"));
        assert_eq!(process_check_calls.load(Ordering::SeqCst), 0);
        assert_eq!(login_close_calls.load(Ordering::SeqCst), 1);
        assert_eq!(login_auth_calls.load(Ordering::SeqCst), 1);
        assert_eq!(
            archive_login_user_id(&fixture.master_dir).as_deref(),
            Some("account-2")
        );
        assert_eq!(fixture.project_owner(), "account-2");
        assert!(fixture.project_exists("project-2"));
        assert_eq!(fixture.session_project_for_title("归并源会话"), "project-2");
        assert_eq!(fixture.backup_count(), 1);

        release_tx.send(()).unwrap();
        assert!(login.join().unwrap().is_ok());
        // 备份文件名按 Unix 秒唯一；让两个串行写操作落在不同秒。
        thread::sleep(Duration::from_millis(1100));
        let merged = run_work_project_merge(
            &fixture,
            "project-2",
            "project-1",
            &process_check_calls,
            &|| {},
        )
        .unwrap();
        assert_eq!(merged.moved_sessions, 1);
        assert_eq!(process_check_calls.load(Ordering::SeqCst), 1);
        assert!(!fixture.project_exists("project-2"));
        assert_eq!(fixture.session_project_for_title("归并源会话"), "project-1");
        assert_eq!(fixture.backup_count(), 2);
    }

    #[test]
    fn work_project_merge_and_environment_login_share_the_secondary_environment_lease_in_reverse_order(
    ) {
        let fixture = Fixture::new().as_secondary_environment();
        fixture.seed_merge_source();
        let (ctx, lifecycle_lease) = super::prepare_library_write_context(
            &fixture.recovery_root,
            &fixture.storage_root,
            Some(&fixture.env_id),
        )
        .unwrap();
        let recovery_root = fixture.recovery_root.clone();
        let storage_root = fixture.storage_root.clone();
        let material_root = fixture.material_root.clone();
        let raw_key = fixture.raw_key.clone();
        let process_check_calls = Arc::new(AtomicUsize::new(0));
        let process_check_thread = process_check_calls.clone();
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let merge = thread::spawn(move || {
            let after_commit = || {
                entered_tx.send(()).unwrap();
                release_rx.recv().unwrap();
            };
            let ensure_stopped = |_ctx: &LibraryContext| {
                process_check_thread.fetch_add(1, Ordering::SeqCst);
                Ok(())
            };
            run_master_project_merge_with_lease(
                &recovery_root,
                &storage_root,
                &storage_root,
                &material_root,
                &raw_key,
                ctx,
                lifecycle_lease,
                &["project-2".to_string()],
                "project-1",
                &ensure_stopped,
                &after_commit,
            )
        });
        let _release_merge_if_test_panics = ReleaseSwitchOnDrop(release_tx.clone());
        if entered_rx.recv_timeout(Duration::from_secs(10)).is_err() {
            let _ = release_tx.send(());
            let merge_error = merge.join().unwrap().err();
            panic!("合成项目归并未到达提交后竞争屏障；错误={merge_error:?}");
        }

        let login_close_calls = AtomicUsize::new(0);
        let login_auth_calls = AtomicUsize::new(0);
        let login_while_merge_holds_lease = run_environment_login(
            &fixture.material_root,
            &fixture.storage_root,
            &fixture.recovery_root,
            &fixture.raw_key,
            &fixture.env_id,
            "profile-2",
            &login_close_calls,
            &login_auth_calls,
            &|| {},
        );
        assert!(login_while_merge_holds_lease
            .err()
            .expect("项目归并提交期间，环境登录应在副作用前因租约拒绝")
            .starts_with("operation_lease_unavailable:"));
        assert_eq!(login_close_calls.load(Ordering::SeqCst), 0);
        assert_eq!(login_auth_calls.load(Ordering::SeqCst), 0);
        assert_eq!(process_check_calls.load(Ordering::SeqCst), 1);
        assert_eq!(
            archive_login_user_id(&fixture.master_dir).as_deref(),
            Some("account-1")
        );
        assert_eq!(fixture.project_owner(), "account-1");
        assert!(!fixture.project_exists("project-2"));
        assert_eq!(fixture.session_project_for_title("归并源会话"), "project-1");
        assert_eq!(fixture.backup_count(), 1);

        release_tx.send(()).unwrap();
        assert_eq!(merge.join().unwrap().unwrap().moved_sessions, 1);
        // 备份文件名按 Unix 秒唯一；让两个串行写操作落在不同秒。
        thread::sleep(Duration::from_millis(1100));
        assert!(run_environment_login(
            &fixture.material_root,
            &fixture.storage_root,
            &fixture.recovery_root,
            &fixture.raw_key,
            &fixture.env_id,
            "profile-2",
            &login_close_calls,
            &login_auth_calls,
            &|| {},
        )
        .is_ok());
        assert_eq!(login_close_calls.load(Ordering::SeqCst), 1);
        assert_eq!(login_auth_calls.load(Ordering::SeqCst), 1);
        assert_eq!(
            EnvironmentRegistry::new(&fixture.storage_root)
                .find(&fixture.env_id)
                .unwrap()
                .unwrap()
                .current_profile_id
                .as_deref(),
            Some("profile-2")
        );
        assert_eq!(
            archive_login_user_id(&fixture.master_dir).as_deref(),
            Some("account-2")
        );
        assert_eq!(fixture.project_owner(), "account-2");
        assert_eq!(fixture.session_project_for_title("归并源会话"), "project-1");
        assert_eq!(fixture.backup_count(), 2);
    }

    #[test]
    fn environment_login_and_delete_share_the_secondary_environment_lease() {
        let fixture = Fixture::new().as_secondary_environment();
        let material_root = fixture.material_root.clone();
        let storage_root = fixture.storage_root.clone();
        let recovery_root = fixture.recovery_root.clone();
        let raw_key = fixture.raw_key.clone();
        let env_id = fixture.env_id.clone();
        let login_close_calls = Arc::new(AtomicUsize::new(0));
        let login_auth_calls = Arc::new(AtomicUsize::new(0));
        let login_close_thread = login_close_calls.clone();
        let login_auth_thread = login_auth_calls.clone();
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let login = thread::spawn(move || {
            let before_return = || {
                entered_tx.send(()).unwrap();
                release_rx.recv().unwrap();
            };
            run_environment_login(
                &material_root,
                &storage_root,
                &recovery_root,
                &raw_key,
                &env_id,
                "profile-2",
                &login_close_thread,
                &login_auth_thread,
                &before_return,
            )
        });
        let _release_login_if_test_panics = ReleaseSwitchOnDrop(release_tx.clone());
        if entered_rx.recv_timeout(Duration::from_secs(10)).is_err() {
            let _ = release_tx.send(());
            let login_error = login.join().unwrap().err();
            panic!("合成环境登录未到达持锁完成屏障；错误={login_error:?}");
        }

        let process_check_calls = AtomicUsize::new(0);
        let delete_while_login_holds_lease = delete_environment_inner_with_process_check(
            &fixture.storage_root,
            &fixture.recovery_root,
            &fixture.env_id,
            &|_| {
                process_check_calls.fetch_add(1, Ordering::SeqCst);
                Ok(())
            },
        );
        assert!(delete_while_login_holds_lease
            .err()
            .expect("环境登录期间，删除必须因目标目录租约拒绝")
            .starts_with("operation_lease_unavailable:"));
        assert_eq!(process_check_calls.load(Ordering::SeqCst), 0);
        assert!(fixture.master_dir.is_dir());
        assert!(EnvironmentRegistry::new(&fixture.storage_root)
            .find(&fixture.env_id)
            .unwrap()
            .is_some());
        assert_eq!(login_close_calls.load(Ordering::SeqCst), 1);
        assert_eq!(login_auth_calls.load(Ordering::SeqCst), 1);
        assert_eq!(
            archive_login_user_id(&fixture.master_dir).as_deref(),
            Some("account-2")
        );
        assert_eq!(fixture.project_owner(), "account-2");
        assert_eq!(fixture.backup_count(), 1);

        release_tx.send(()).unwrap();
        assert!(login.join().unwrap().is_ok());
        assert!(delete_environment_inner_with_process_check(
            &fixture.storage_root,
            &fixture.recovery_root,
            &fixture.env_id,
            &|_| Ok(()),
        )
        .unwrap());
        assert!(!fixture.master_dir.exists());
        assert!(EnvironmentRegistry::new(&fixture.storage_root)
            .find(&fixture.env_id)
            .unwrap()
            .is_none());
    }

    #[test]
    fn delete_and_environment_login_share_the_secondary_environment_lease_in_reverse_order() {
        let fixture = Fixture::new().as_secondary_environment();
        let storage_root = fixture.storage_root.clone();
        let recovery_root = fixture.recovery_root.clone();
        let env_id = fixture.env_id.clone();
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let delete = thread::spawn(move || {
            let ensure_stopped = |_: &Path| {
                entered_tx.send(()).unwrap();
                release_rx.recv().unwrap();
                Ok(())
            };
            delete_environment_inner_with_process_check(
                &storage_root,
                &recovery_root,
                &env_id,
                &ensure_stopped,
            )
        });
        let _release_delete_if_test_panics = ReleaseSwitchOnDrop(release_tx.clone());
        if entered_rx.recv_timeout(Duration::from_secs(10)).is_err() {
            let _ = release_tx.send(());
            let delete_error = delete.join().unwrap().err();
            panic!("副环境删除未到达持锁进程检查屏障；错误={delete_error:?}");
        }

        let login_close_calls = AtomicUsize::new(0);
        let login_auth_calls = AtomicUsize::new(0);
        let login_while_delete_holds_lease = run_environment_login(
            &fixture.material_root,
            &fixture.storage_root,
            &fixture.recovery_root,
            &fixture.raw_key,
            &fixture.env_id,
            "profile-2",
            &login_close_calls,
            &login_auth_calls,
            &|| {},
        );
        assert!(login_while_delete_holds_lease
            .err()
            .expect("环境删除期间，登录必须因目标目录租约拒绝")
            .starts_with("operation_lease_unavailable:"));
        assert_eq!(login_close_calls.load(Ordering::SeqCst), 0);
        assert_eq!(login_auth_calls.load(Ordering::SeqCst), 0);
        assert!(fixture.master_dir.is_dir());
        assert!(EnvironmentRegistry::new(&fixture.storage_root)
            .find(&fixture.env_id)
            .unwrap()
            .is_some());
        assert_eq!(
            archive_login_user_id(&fixture.master_dir).as_deref(),
            Some("account-1")
        );
        assert_eq!(fixture.project_owner(), "account-1");
        assert_eq!(fixture.backup_count(), 0);

        release_tx.send(()).unwrap();
        assert!(delete.join().unwrap().unwrap());
        assert!(!fixture.master_dir.exists());
        assert!(EnvironmentRegistry::new(&fixture.storage_root)
            .find(&fixture.env_id)
            .unwrap()
            .is_none());
        let login_after_delete = run_environment_login(
            &fixture.material_root,
            &fixture.storage_root,
            &fixture.recovery_root,
            &fixture.raw_key,
            &fixture.env_id,
            "profile-2",
            &login_close_calls,
            &login_auth_calls,
            &|| {},
        );
        assert_eq!(
            login_after_delete.err().as_deref(),
            Some("environment_not_found")
        );
        assert_eq!(login_close_calls.load(Ordering::SeqCst), 0);
        assert_eq!(login_auth_calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn concurrent_environment_login_requests_do_not_repeat_side_effects() {
        let fixture = Fixture::new().as_secondary_environment();
        let material_root = fixture.material_root.clone();
        let storage_root = fixture.storage_root.clone();
        let recovery_root = fixture.recovery_root.clone();
        let raw_key = fixture.raw_key.clone();
        let env_id = fixture.env_id.clone();
        let first_close_calls = Arc::new(AtomicUsize::new(0));
        let first_auth_calls = Arc::new(AtomicUsize::new(0));
        let first_close_thread = first_close_calls.clone();
        let first_auth_thread = first_auth_calls.clone();
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let first_login = thread::spawn(move || {
            let before_return = || {
                entered_tx.send(()).unwrap();
                release_rx.recv().unwrap();
            };
            run_environment_login(
                &material_root,
                &storage_root,
                &recovery_root,
                &raw_key,
                &env_id,
                "profile-2",
                &first_close_thread,
                &first_auth_thread,
                &before_return,
            )
        });
        let _release_login_if_test_panics = ReleaseSwitchOnDrop(release_tx.clone());
        if entered_rx.recv_timeout(Duration::from_secs(10)).is_err() {
            let _ = release_tx.send(());
            let login_error = first_login.join().unwrap().err();
            panic!("首个环境登录未到达持锁完成屏障；错误={login_error:?}");
        }

        let duplicate_close_calls = AtomicUsize::new(0);
        let duplicate_auth_calls = AtomicUsize::new(0);
        let duplicate = run_environment_login(
            &fixture.material_root,
            &fixture.storage_root,
            &fixture.recovery_root,
            &fixture.raw_key,
            &fixture.env_id,
            "profile-2",
            &duplicate_close_calls,
            &duplicate_auth_calls,
            &|| {},
        );
        assert!(duplicate
            .err()
            .expect("同一环境的并发登录请求必须被目标租约串行化")
            .starts_with("operation_lease_unavailable:"));
        assert_eq!(duplicate_close_calls.load(Ordering::SeqCst), 0);
        assert_eq!(duplicate_auth_calls.load(Ordering::SeqCst), 0);
        assert_eq!(first_close_calls.load(Ordering::SeqCst), 1);
        assert_eq!(first_auth_calls.load(Ordering::SeqCst), 1);
        assert_eq!(
            archive_login_user_id(&fixture.master_dir).as_deref(),
            Some("account-2")
        );
        assert_eq!(fixture.project_owner(), "account-2");
        assert_eq!(fixture.backup_count(), 1);
        assert_eq!(
            EnvironmentRegistry::new(&fixture.storage_root)
                .find(&fixture.env_id)
                .unwrap()
                .unwrap()
                .current_profile_id
                .as_deref(),
            Some("profile-2")
        );

        release_tx.send(()).unwrap();
        assert!(first_login.join().unwrap().is_ok());
    }

    #[test]
    fn empty_environment_logins_seed_and_run_in_parallel() {
        let fixture = Fixture::new().as_secondary_environment();
        std::fs::remove_dir_all(&fixture.master_dir).unwrap();
        let second = EnvironmentRegistry::new(&fixture.storage_root)
            .create("并行环境")
            .unwrap();
        let close_first = Arc::new(AtomicUsize::new(0));
        let close_second = Arc::new(AtomicUsize::new(0));
        let auth_first = Arc::new(AtomicUsize::new(0));
        let auth_second = Arc::new(AtomicUsize::new(0));
        let (first_entered_tx, first_entered_rx) = mpsc::channel();
        let (second_entered_tx, second_entered_rx) = mpsc::channel();
        let (release_first_tx, release_first_rx) = mpsc::channel();
        let (release_second_tx, release_second_rx) = mpsc::channel();

        let material_first = fixture.material_root.clone();
        let storage_first = fixture.storage_root.clone();
        let recovery_first = fixture.recovery_root.clone();
        let raw_key_first = fixture.raw_key.clone();
        let first_id = fixture.env_id.clone();
        let close_first_thread = close_first.clone();
        let auth_first_thread = auth_first.clone();
        let first = thread::spawn(move || {
            let before_return = || {
                first_entered_tx.send(()).unwrap();
                release_first_rx.recv().unwrap();
            };
            run_environment_login(
                &material_first,
                &storage_first,
                &recovery_first,
                &raw_key_first,
                &first_id,
                "profile-2",
                &close_first_thread,
                &auth_first_thread,
                &before_return,
            )
        });
        let _release_first_if_test_panics = ReleaseSwitchOnDrop(release_first_tx.clone());

        first_entered_rx
            .recv_timeout(Duration::from_secs(10))
            .expect("第一个空环境登录应先到达持锁完成屏障");

        let material_second = fixture.material_root.clone();
        let storage_second = fixture.storage_root.clone();
        let recovery_second = fixture.recovery_root.clone();
        let raw_key_second = fixture.raw_key.clone();
        let second_id = second.env_id.clone();
        let close_second_thread = close_second.clone();
        let auth_second_thread = auth_second.clone();
        let second_login = thread::spawn(move || {
            let before_return = || {
                second_entered_tx.send(()).unwrap();
                release_second_rx.recv().unwrap();
            };
            run_environment_login(
                &material_second,
                &storage_second,
                &recovery_second,
                &raw_key_second,
                &second_id,
                "profile-2",
                &close_second_thread,
                &auth_second_thread,
                &before_return,
            )
        });
        let _release_second_if_test_panics = ReleaseSwitchOnDrop(release_second_tx.clone());

        let second_wait = second_entered_rx.recv_timeout(Duration::from_secs(10));
        if second_wait.is_err() {
            let _ = release_second_tx.send(());
            let second_result = second_login.join().unwrap();
            panic!(
                "不同环境的合成登录应能同时到达持锁完成屏障；second={:?}",
                second_result.err()
            );
        }

        assert_eq!(close_first.load(Ordering::SeqCst), 1);
        assert_eq!(close_second.load(Ordering::SeqCst), 1);
        assert_eq!(auth_first.load(Ordering::SeqCst), 0);
        assert_eq!(auth_second.load(Ordering::SeqCst), 0);
        release_first_tx.send(()).unwrap();
        release_second_tx.send(()).unwrap();
        assert!(first.join().unwrap().is_ok());
        assert!(second_login.join().unwrap().is_ok());

        for env_id in [&fixture.env_id, &second.env_id] {
            let env_dir = secondary_data_dir(&fixture.storage_root, env_id);
            assert_eq!(
                archive_login_user_id(&env_dir).as_deref(),
                Some("account-2")
            );
            assert_eq!(
                EnvironmentRegistry::new(&fixture.storage_root)
                    .find(env_id)
                    .unwrap()
                    .unwrap()
                    .current_profile_id
                    .as_deref(),
                Some("profile-2")
            );
            assert!(!master_database_path(&env_dir).exists());
        }
    }

    fn account(profile_id: &str, account_id: &str) -> AccountRecord {
        AccountRecord {
            profile_id: profile_id.to_string(),
            account_id: account_id.to_string(),
            screen_name: format!("用户{account_id}"),
            avatar_url: String::new(),
            device_id: format!("device-{profile_id}"),
            device_public_key: format!("public-{profile_id}"),
            display_name: None,
            masked_mobile: String::new(),
            created_at_unix_seconds: 1,
            last_verified_at_unix_seconds: 1,
            device_created_at_unix_seconds: 0,
            auto_checkin_enabled: false,
            archived: false,
        }
    }

    fn seed_auth(instance_dir: &std::path::Path, account: &AccountRecord) {
        let user_info = UserInfoFull {
            user_id: account.account_id.clone(),
            screen_name: account.screen_name.clone(),
            ..Default::default()
        };
        construct_auth_identity(
            &ConstructAuthInput {
                account_id: &account.account_id,
                access_token: "test-token",
                refresh_token: "test-refresh",
                access_token_expires_at_unix_seconds: 4_000_000_000,
                refresh_token_expires_at_unix_seconds: 4_100_000_000,
                user_info: &user_info,
            },
            instance_dir,
        )
        .unwrap();
    }

    struct ReleaseSwitchOnDrop(mpsc::Sender<()>);

    impl Drop for ReleaseSwitchOnDrop {
        fn drop(&mut self) {
            let _ = self.0.send(());
        }
    }

    /// 测试异常退出时终止并回收持锁子进程。
    struct ChildProcessGuard(Child);

    /// 等待子进程到期后先终止并回收，避免 TempDir 被仍在运行的进程访问。
    fn wait_child_with_timeout(
        child: &mut Child,
        timeout: Duration,
    ) -> std::io::Result<ExitStatus> {
        let deadline = Instant::now() + timeout;
        loop {
            match child.try_wait() {
                Ok(Some(status)) => return Ok(status),
                Ok(None) if Instant::now() < deadline => {
                    thread::sleep(Duration::from_millis(25));
                }
                Ok(None) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::TimedOut,
                        "子进程未在限定时间内退出，已终止并回收",
                    ));
                }
                Err(error) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(error);
                }
            }
        }
    }

    impl Drop for ChildProcessGuard {
        fn drop(&mut self) {
            if self.0.try_wait().ok().flatten().is_none() {
                let _ = self.0.kill();
            }
            let _ = self.0.wait();
        }
    }

    /// 子进程持有临时库目录租约，供真实归档编排验证跨进程互斥。
    #[test]
    fn child_process_holds_library_data_lease_for_caller_test() {
        let (Ok(recovery_root), Ok(resource_path)) = (
            std::env::var("TRAE_SYNC_ARCHIVE_CALLER_LOCK_ROOT"),
            std::env::var("TRAE_SYNC_ARCHIVE_CALLER_LOCK_RESOURCE"),
        ) else {
            return;
        };
        let _lease = OperationLease::acquire_shared_for_path(
            Path::new(&recovery_root),
            Path::new(&resource_path),
        )
        .unwrap();
        let mut stdout = std::io::stdout().lock();
        writeln!(stdout, "ARCHIVE_CALLER_LOCK_READY").unwrap();
        stdout.flush().unwrap();
        let mut line = String::new();
        let _ = std::io::stdin().lock().read_line(&mut line);
    }

    /// 子进程执行真实 Work 归档 caller，供父进程限制锁竞争等待时间。
    #[test]
    fn child_runs_work_archive_caller_for_cross_process_test() {
        if std::env::var("TRAE_SYNC_ARCHIVE_CALLER_RUN")
            .ok()
            .as_deref()
            != Some("1")
        {
            return;
        }
        let fixture = Fixture {
            _root: tempfile::tempdir().unwrap(),
            recovery_root: std::env::var_os("TRAE_SYNC_ARCHIVE_CALLER_RUN_RECOVERY_ROOT")
                .unwrap()
                .into(),
            storage_root: std::env::var_os("TRAE_SYNC_ARCHIVE_CALLER_RUN_STORAGE_ROOT")
                .unwrap()
                .into(),
            material_root: std::env::var_os("TRAE_SYNC_ARCHIVE_CALLER_RUN_MATERIAL_ROOT")
                .unwrap()
                .into(),
            master_dir: std::env::var_os("TRAE_SYNC_ARCHIVE_CALLER_RUN_DATA_DIR")
                .unwrap()
                .into(),
            env_id: "master".to_string(),
            env_name: "主库".to_string(),
            raw_key: std::env::var("TRAE_SYNC_ARCHIVE_CALLER_RUN_RAW_KEY").unwrap(),
        };
        let close_calls = AtomicUsize::new(0);
        let relaunch_calls = AtomicUsize::new(0);
        let error = fixture
            .apply(&close_calls, &relaunch_calls)
            .err()
            .expect("另一进程持有同库租约时，Work 归档必须拒绝");
        assert_eq!(
            error,
            format!(
                "operation_lease_unavailable:{}",
                OperationLeaseError::DataLocationBusy
            )
        );
        assert_eq!(close_calls.load(Ordering::SeqCst), 0);
        assert_eq!(relaunch_calls.load(Ordering::SeqCst), 0);
    }

    /// 子进程执行真实 Work 项目归并 caller，供父进程限制锁竞争等待时间。
    #[test]
    fn child_runs_work_project_merge_caller_for_cross_process_test() {
        if std::env::var("TRAE_SYNC_ARCHIVE_MERGE_CALLER_RUN")
            .ok()
            .as_deref()
            != Some("1")
        {
            return;
        }
        let fixture = Fixture {
            _root: tempfile::tempdir().unwrap(),
            recovery_root: std::env::var_os("TRAE_SYNC_ARCHIVE_MERGE_CALLER_RUN_RECOVERY_ROOT")
                .unwrap()
                .into(),
            storage_root: std::env::var_os("TRAE_SYNC_ARCHIVE_MERGE_CALLER_RUN_STORAGE_ROOT")
                .unwrap()
                .into(),
            material_root: std::env::var_os("TRAE_SYNC_ARCHIVE_MERGE_CALLER_RUN_MATERIAL_ROOT")
                .unwrap()
                .into(),
            master_dir: std::env::var_os("TRAE_SYNC_ARCHIVE_MERGE_CALLER_RUN_DATA_DIR")
                .unwrap()
                .into(),
            env_id: std::env::var("TRAE_SYNC_ARCHIVE_MERGE_CALLER_RUN_ENV_ID").unwrap(),
            env_name: "跨进程合成环境".to_string(),
            raw_key: std::env::var("TRAE_SYNC_ARCHIVE_MERGE_CALLER_RUN_RAW_KEY").unwrap(),
        };
        let process_check_calls = AtomicUsize::new(0);
        let error = run_work_project_merge(
            &fixture,
            "project-2",
            "project-1",
            &process_check_calls,
            &|| {},
        )
        .err()
        .expect("持锁子进程仍在运行时，Work 项目归并必须拒绝");
        assert_eq!(
            error,
            format!(
                "operation_lease_unavailable:{}",
                OperationLeaseError::DataLocationBusy
            )
        );
        assert_eq!(process_check_calls.load(Ordering::SeqCst), 0);
    }

    /// 父测持有真实库租约，由独立进程运行归档 caller 并限制其等待时间。
    #[test]
    fn work_archive_caller_rejects_cross_process_library_lease_before_side_effects() {
        let fixture = Fixture::new();
        let lease = acquire_library_data_lease(&fixture.recovery_root, &fixture.context())
            .expect("测试父进程应持有实际 Work 库租约");

        let caller = Command::new(std::env::current_exe().unwrap())
            .env("TRAE_SYNC_ARCHIVE_CALLER_RUN", "1")
            .env(
                "TRAE_SYNC_ARCHIVE_CALLER_RUN_RECOVERY_ROOT",
                &fixture.recovery_root,
            )
            .env(
                "TRAE_SYNC_ARCHIVE_CALLER_RUN_STORAGE_ROOT",
                &fixture.storage_root,
            )
            .env(
                "TRAE_SYNC_ARCHIVE_CALLER_RUN_MATERIAL_ROOT",
                &fixture.material_root,
            )
            .env(
                "TRAE_SYNC_ARCHIVE_CALLER_RUN_DATA_DIR",
                &fixture.master_dir,
            )
            .env("TRAE_SYNC_ARCHIVE_CALLER_RUN_RAW_KEY", &fixture.raw_key)
            .args([
                "--exact",
                "library_archive::p14_4c_archive_caller_tests::child_runs_work_archive_caller_for_cross_process_test",
                "--nocapture",
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .spawn();
        let mut caller = match caller {
            Ok(child) => ChildProcessGuard(child),
            Err(error) => {
                drop(lease);
                panic!("无法启动 Work 归档 caller 测试子进程：{error}");
            }
        };
        let caller_deadline = Instant::now() + Duration::from_secs(10);
        let caller_status = loop {
            match caller.0.try_wait() {
                Ok(Some(status)) => break Some(status),
                Ok(None) if Instant::now() < caller_deadline => {
                    thread::sleep(Duration::from_millis(25));
                }
                Ok(None) | Err(_) => break None,
            }
        };
        if caller_status.is_none() {
            let _ = caller.0.kill();
            let _ = caller.0.wait();
            drop(lease);
            panic!("Work 归档 caller 未在持锁期间 10 秒内返回；已终止 caller 并释放测试租约");
        }

        assert!(caller_status.unwrap().success());
        assert_eq!(fixture.hidden_status(), None);
        assert_eq!(fixture.backup_count(), 0);
        drop(lease);

        let close_calls = AtomicUsize::new(0);
        let relaunch_calls = AtomicUsize::new(0);
        let receipt = fixture.apply(&close_calls, &relaunch_calls).unwrap();
        assert_eq!(receipt.archived_sessions, 1);
        assert!(fixture.hidden_status().is_some());
        assert_eq!(fixture.backup_count(), 1);
        assert_eq!(close_calls.load(Ordering::SeqCst), 1);
        assert_eq!(relaunch_calls.load(Ordering::SeqCst), 1);
    }

    /// 验证 Work 项目归并与另一进程持有同一库目录租约时，在副作用前精确拒绝。
    #[test]
    fn work_project_merge_caller_rejects_cross_process_library_lease_before_side_effects() {
        let fixture = Fixture::new().as_secondary_environment();
        fixture.seed_merge_source();
        let resource_path = secondary_data_dir(&fixture.storage_root, &fixture.env_id);
        assert_eq!(resource_path, fixture.master_dir);

        let mut child = ChildProcessGuard(
            Command::new(std::env::current_exe().unwrap())
                .env("TRAE_SYNC_ARCHIVE_CALLER_LOCK_ROOT", &fixture.recovery_root)
                .env("TRAE_SYNC_ARCHIVE_CALLER_LOCK_RESOURCE", &resource_path)
                .args([
                    "--exact",
                    "library_archive::p14_4c_archive_caller_tests::child_process_holds_library_data_lease_for_caller_test",
                    "--nocapture",
                ])
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .spawn()
                .unwrap(),
        );
        let mut child_stdin = child.0.stdin.take().unwrap();
        let stdout = child.0.stdout.take().unwrap();
        let (ready_tx, ready_rx) = mpsc::channel();
        let reader = thread::spawn(move || {
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
                    Ok(_) if !ready_sent && line.trim() == "ARCHIVE_CALLER_LOCK_READY" => {
                        let _ = ready_tx.send(true);
                        ready_sent = true;
                    }
                    Ok(_) => {}
                }
            }
        });
        assert_eq!(
            ready_rx.recv_timeout(Duration::from_secs(10)),
            Ok(true),
            "持锁子进程未在 10 秒内报告就绪"
        );

        let caller = Command::new(std::env::current_exe().unwrap())
            .env("TRAE_SYNC_ARCHIVE_MERGE_CALLER_RUN", "1")
            .env(
                "TRAE_SYNC_ARCHIVE_MERGE_CALLER_RUN_RECOVERY_ROOT",
                &fixture.recovery_root,
            )
            .env(
                "TRAE_SYNC_ARCHIVE_MERGE_CALLER_RUN_STORAGE_ROOT",
                &fixture.storage_root,
            )
            .env(
                "TRAE_SYNC_ARCHIVE_MERGE_CALLER_RUN_MATERIAL_ROOT",
                &fixture.material_root,
            )
            .env(
                "TRAE_SYNC_ARCHIVE_MERGE_CALLER_RUN_DATA_DIR",
                &fixture.master_dir,
            )
            .env(
                "TRAE_SYNC_ARCHIVE_MERGE_CALLER_RUN_ENV_ID",
                &fixture.env_id,
            )
            .env(
                "TRAE_SYNC_ARCHIVE_MERGE_CALLER_RUN_RAW_KEY",
                &fixture.raw_key,
            )
            .args([
                "--exact",
                "library_archive::p14_4c_archive_caller_tests::child_runs_work_project_merge_caller_for_cross_process_test",
                "--nocapture",
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .spawn();
        let mut caller = match caller {
            Ok(child) => ChildProcessGuard(child),
            Err(error) => {
                let _ = child_stdin.write_all(b"\n");
                drop(child_stdin);
                let deadline = Instant::now() + Duration::from_secs(5);
                while child.0.try_wait().ok().flatten().is_none() && Instant::now() < deadline {
                    thread::sleep(Duration::from_millis(25));
                }
                if child.0.try_wait().ok().flatten().is_none() {
                    let _ = child.0.kill();
                    let _ = child.0.wait();
                }
                let _ = reader.join();
                panic!("无法启动 Work 项目归并 caller 测试子进程：{error}");
            }
        };
        let caller_deadline = Instant::now() + Duration::from_secs(10);
        let caller_status = loop {
            match caller.0.try_wait() {
                Ok(Some(status)) => break Some(status),
                Ok(None) if Instant::now() < caller_deadline => {
                    thread::sleep(Duration::from_millis(25));
                }
                Ok(None) | Err(_) => break None,
            }
        };
        if caller_status.is_none() {
            let _ = child_stdin.write_all(b"\n");
            drop(child_stdin);
            let holder_deadline = Instant::now() + Duration::from_secs(5);
            while child.0.try_wait().ok().flatten().is_none() && Instant::now() < holder_deadline {
                thread::sleep(Duration::from_millis(25));
            }
            if child.0.try_wait().ok().flatten().is_none() {
                let _ = child.0.kill();
                let _ = child.0.wait();
            }
            let caller_cleanup_deadline = Instant::now() + Duration::from_secs(5);
            while caller.0.try_wait().ok().flatten().is_none()
                && Instant::now() < caller_cleanup_deadline
            {
                thread::sleep(Duration::from_millis(25));
            }
            if caller.0.try_wait().ok().flatten().is_none() {
                let _ = caller.0.kill();
                let _ = caller.0.wait();
            }
            let _ = reader.join();
            panic!("Work 项目归并 caller 未在持锁期间 10 秒内返回；已释放并回收测试子进程");
        }

        child_stdin.write_all(b"\n").unwrap();
        drop(child_stdin);
        let deadline = Instant::now() + Duration::from_secs(10);
        let status = loop {
            match child.0.try_wait().unwrap() {
                Some(status) => break status,
                None if Instant::now() < deadline => thread::sleep(Duration::from_millis(25)),
                None => {
                    let _ = child.0.kill();
                    let _ = child.0.wait();
                    panic!("持锁子进程释放后未在 10 秒内退出");
                }
            }
        };
        assert!(status.success());
        reader.join().unwrap();
        assert!(caller_status.unwrap().success());

        assert_eq!(fixture.backup_count(), 0);
        assert!(fixture.project_exists("project-2"));
        assert_eq!(fixture.session_project_for_title("归并源会话"), "project-2");
        let conn = Connection::open_with_flags(
            master_database_path(&fixture.master_dir),
            OpenFlags::SQLITE_OPEN_READ_WRITE,
        )
        .unwrap();
        conn.execute_batch(&format!("PRAGMA key = \"x'{}'\";", fixture.raw_key))
            .unwrap();
        let source_owner: String = conn
            .query_row(
                "SELECT user_id FROM project WHERE project_id = 'project-2'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(source_owner, "account-1");
    }
}

/// P15-8 主库真实写操作验收驱动（归档/恢复、项目归并、删除会话）。
///
/// 仅 `--features probe` 编译；与 UI 命令共享同一编排核心（本文件私有
/// 函数），仅绕过 Tauri State 与就绪注册表闸门——注册表三项的解锁依据
/// 正是本探针产出的真实验收证据。每个写入口执行前自动三件套备份
/// （ADR-0018 铁律），删除/归并核心另有内建备份。
#[cfg(feature = "probe")]
pub mod acceptance_probe {
    use super::*;
    use serde_json::{json, Value};

    /// 生产同源根目录推导（与 real_switch_probe 同规则）。
    fn production_roots() -> Result<(PathBuf, PathBuf, PathBuf), String> {
        let app_root = std::env::var_os("LOCALAPPDATA")
            .map(|base| PathBuf::from(base).join("Trae Sync"))
            .ok_or_else(|| "localappdata_unavailable".to_string())?;
        let storage_root = app_root.join("data");
        let recovery_root = app_root.join("recovery");
        let material_root = storage_root.join("checkin");
        if !material_root.is_dir() {
            return Err("checkin_registry_missing".to_string());
        }
        Ok((storage_root, recovery_root, material_root))
    }

    fn source_key() -> Result<String, String> {
        let key = crate::WORK_CN_SOURCE_RAW_KEY;
        if key.is_empty() {
            return Err("source_key_unavailable".to_string());
        }
        Ok(key.to_string())
    }

    /// 验收安全网：三件套手动备份（独立于各写入口的内建备份）。
    fn guard_backup(storage_root: &Path) -> Result<String, String> {
        let ctx = resolve_library_context(None, storage_root)?;
        let db_path = master_database_path(&ctx.dir);
        let backup_dir = resolve_library_backup_dir(storage_root, &db_path, &ctx.env_id);
        let path = backup_master_trio_to(&db_path, Some(&backup_dir))
            .map_err(|error| format!("pre_acceptance_backup_failed:{error:?}"))?;
        Ok(path.display().to_string())
    }

    /// 主库项目/会话清单（验收对象选择；只读）。
    pub fn list_snapshot() -> Result<Value, String> {
        let (storage_root, _recovery_root, material_root) = production_roots()?;
        let raw_key = source_key()?;
        let ctx = resolve_library_context(None, &storage_root)?;
        let account = library_account::resolve_library_account(
            &material_root,
            &storage_root,
            &ctx,
        )?
        .and_then(LibraryAccountResolution::into_account)
        .ok_or_else(|| "no_current_account".to_string())?;
        match read_master_history(&ctx.dir, &raw_key, &account.account_id) {
            MasterHistoryStatus::Ready { projects, sessions } => Ok(json!({
                "status": "ready",
                "account_id": account.account_id,
                "master_dir": ctx.dir.display().to_string(),
                "projects": projects,
                "sessions": sessions,
            })),
            MasterHistoryStatus::NoMasterData => {
                Ok(json!({ "status": "no_master_data" }))
            }
            MasterHistoryStatus::ReadFailed => {
                Err("master_history_read_failed".to_string())
            }
        }
    }

    /// 手动三件套备份（验收安全网入口）。
    pub fn backup_now() -> Result<Value, String> {
        let (storage_root, _recovery_root, _material_root) = production_roots()?;
        let path = guard_backup(&storage_root)?;
        Ok(json!({ "backup_path": path }))
    }

    /// 归档/恢复提交（真实编排：租约 → 关实例 → 备份 → 单事务 → 重启）。
    pub fn archive_apply(
        archive_session_ids: Vec<String>,
        restore_session_ids: Vec<String>,
    ) -> Result<Value, String> {
        let (storage_root, recovery_root, material_root) = production_roots()?;
        let raw_key = source_key()?;
        let guard = guard_backup(&storage_root)?;
        let (ctx, lifecycle_lease) =
            prepare_library_write_context(&recovery_root, &storage_root, None)?;
        let close_instance =
            |ctx: &LibraryContext| ctx.close_instance().map(|_| ());
        let relaunch_instance = |ctx: &LibraryContext, title: &str| {
            ctx.relaunch(title).map(|relaunch| relaunch.outcome.to_string())
        };
        let ensure_stopped = |ctx: &LibraryContext| {
            crate::process_guard::ensure_product_stopped(
                TraeProduct::TraeCn,
                &ctx.dir,
                "trae_cn_archive_client_running",
            )
        };
        let result = apply_library_archive_for_context(
            &recovery_root,
            &storage_root,
            &material_root,
            &raw_key,
            ctx,
            lifecycle_lease,
            &archive_session_ids,
            &restore_session_ids,
            &close_instance,
            &relaunch_instance,
            &ensure_stopped,
            &|| {},
        );
        traesync_infrastructure::master_checkup::invalidate_master_checkup_cache();
        let result = result?;
        Ok(json!({
            "archived_sessions": result.archived_sessions,
            "restored_sessions": result.restored_sessions,
            "relaunch_outcome": result.relaunch_outcome,
            "guard_backup": guard,
        }))
    }

    /// 项目归并（源分组会话并入目标分组，内建自动备份）。
    pub fn merge_projects(
        source_project_ids: Vec<String>,
        target_project_id: String,
    ) -> Result<Value, String> {
        let (storage_root, recovery_root, material_root) = production_roots()?;
        let raw_key = source_key()?;
        let (ctx, lifecycle_lease) =
            prepare_library_write_context(&recovery_root, &storage_root, None)?;
        let ensure_stopped = |ctx: &LibraryContext| {
            crate::process_guard::ensure_product_stopped(
                TraeProduct::TraeCn,
                &ctx.dir,
                "master_merge_running",
            )
        };
        let result = run_master_project_merge_with_lease(
            &recovery_root,
            &storage_root,
            &storage_root,
            &material_root,
            &raw_key,
            ctx,
            lifecycle_lease,
            &source_project_ids,
            &target_project_id,
            &ensure_stopped,
            &|| {},
        );
        traesync_infrastructure::master_checkup::invalidate_master_checkup_cache();
        let result = result?;
        Ok(json!({
            "moved_sessions": result.moved_sessions,
            "removed_projects": result.removed_projects,
            "backup_path": result.backup_path,
        }))
    }

    /// 删除会话（含备份与保留策略清理）。
    pub fn delete_sessions(session_ids: Vec<String>) -> Result<Value, String> {
        let (storage_root, recovery_root, material_root) = production_roots()?;
        let raw_key = source_key()?;
        let (ctx, _lease) =
            acquire_library_write_lease(&recovery_root, &storage_root, None)?;
        ctx.ensure_writable()?;
        let current_account =
            require_library_account(&material_root, &storage_root, &ctx)?;
        crate::process_guard::ensure_product_stopped(
            TraeProduct::TraeCn,
            &ctx.dir,
            "master_delete_running",
        )?;
        let outcome = delete_master_sessions_inner(
            &ctx.dir,
            &raw_key,
            &session_ids,
            &current_account.account_id,
        )
        .map_err(map_master_archive_error)?;
        let db_path = master_database_path(&ctx.dir);
        let backup_dir = resolve_library_backup_dir(&storage_root, &db_path, &ctx.env_id);
        prune_backups_if_enabled_in(&db_path, &storage_root, &backup_dir);
        traesync_infrastructure::master_checkup::invalidate_master_checkup_cache();
        Ok(json!({
            "deleted_sessions": outcome.deleted_sessions,
            "deleted_messages": outcome.deleted_messages,
            "removed_projects": outcome.removed_projects,
            "backup_path": outcome.backup_path,
        }))
    }

    /// P16-T1：旧备份链压缩迁移（真机一次性执行；显式传入目标目录，
    /// 避免探针 exe 路径干扰默认推导——生产默认推导见 default_master_backup_dir）。
    pub fn migrate_backups(target_dir: &Path) -> Result<Value, String> {
        let (storage_root, _recovery_root, _material_root) = production_roots()?;
        let ctx = resolve_library_context(None, &storage_root)?;
        let db_path = master_database_path(&ctx.dir);
        let source_dir = db_path
            .parent()
            .ok_or_else(|| "master_db_no_parent".to_string())?;
        let outcome = traesync_infrastructure::backup_retention::migrate_legacy_backups_to_compressed(
            source_dir,
            target_dir,
            "database.db",
        )
        .map_err(|error| format!("{error:?}"))?;
        Ok(json!({
            "migrated": outcome.migrated_backups,
            "idempotent": outcome.idempotent_backups,
            "freed_bytes": outcome.freed_bytes,
            "failures": outcome.failures,
            "source_dir": source_dir.display().to_string(),
            "target_dir": target_dir.display().to_string(),
        }))
    }
}
