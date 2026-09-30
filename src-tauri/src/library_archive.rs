//! 库历史 / 归档 / 备份域（P12-R3 自 lib.rs 纯移动）。
//!
//! 覆盖：库 id → 目录/环境归属/运行上下文解析三分支（resolve_library_dir /
//! resolve_library_env_id / resolve_library_context，ADR-0025 库抽象）、
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

use std::path::{Path, PathBuf};

use serde::Serialize;
use tauri::Emitter;

use traesync_domain::{LibraryContext, TRAE_CN_LIBRARY_ID};
use traesync_infrastructure::{
    AccountRegistry, MasterArchiveError, MasterDeleteOutcome, MasterHandover,
    MasterHandoverError, MasterMergeOutcome, RelayLedger, RelayLedgerEntry, TraeProduct,
    apply_master_archive_changes as apply_master_archive_changes_inner,
    archive_master_sessions as archive_master_sessions_inner,
    backup_master_trio_stamped, backup_master_trio_to,
    delete_master_sessions as delete_master_sessions_inner, handover_master_records,
    list_master_backups_in, master_backup_db_name, master_database_path,
    master_db_activity_detected, merge_master_projects as merge_master_projects_inner,
    merge_trae_cn_project_records_with_progress,
    preview_trae_cn_project_merge as preview_trae_cn_project_merge_infra,
    read_trae_cn_foreign_projects, restore_master_sessions as restore_master_sessions_inner,
};
use traesync_infrastructure::account_session_content::{
    read_master_session_messages_page, MAX_MESSAGES_PER_SESSION,
};
use traesync_infrastructure::account_session_index::InstanceFingerprint;
use traesync_infrastructure::archive_draft_store::{
    ArchiveDraftRecord, ArchiveDraftStore, ArchiveDraftStoreError,
};
use traesync_infrastructure::backup_retention::{
    load_backup_dir_override, migrate_master_backups as migrate_master_backups_inner,
    prune_master_backups, BackupMigrationOutcome, BackupRetentionSettings, BackupRetentionStore,
    MAX_BACKUP_DIR_BYTES,
};
use traesync_infrastructure::environment_registry::{
    environment_registry_error_code, master_data_dir, master_data_dir_for, secondary_data_dir,
    EnvironmentRegistry, MASTER_ENV_ID,
};
use traesync_infrastructure::library_account::{self, LibraryAccountResolution};
use traesync_infrastructure::master_history::{
    read_master_history, stat_master_fingerprint, MasterHistoryStatus,
};
use traesync_infrastructure::trae_instance::{
    self as trae_instance_module, close_instance, close_master_instance,
    command_line_uses_data_dir, list_trae_processes,
};

use crate::{
    checkin_material_root, ensure_real_preview, error_mapping::map_master_archive_error,
    launch_instance_common, run_blocking, AppState, RuntimeMode, TraeInstanceLaunchDto,
};

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

/// ADR-0025 Library 抽象：库 id → 数据目录解析（P11-5⑤ 泛化到副环境）。
///
/// 库是基地概念：主库与副库都是「库」的实例，库内模块面向库编程。
/// 库注册表即环境注册表（ADR-0024）：`master` = 官方目录（`None` 缺省
/// 兼容既有调用）；`env-*` = 环境派生目录 `{storage_root}\environments\{env_id}`；
/// P11-6 追加 `trae_cn` = Trae CN 官方数据根（`%APPDATA%\Trae CN`，只读库）；
/// 未登记 id 一律 `library_not_found`（不落默认库）。
fn resolve_library_dir(library_id: Option<&str>, storage_root: &Path) -> Result<PathBuf, String> {
    match resolve_library_env_id(library_id, storage_root)?.as_str() {
        MASTER_ENV_ID => {
            master_data_dir().map_err(|_| "environment_registry_invalid".to_string())
        }
        TRAE_CN_LIBRARY_ID => trae_cn_library_dir(),
        env_id => Ok(secondary_data_dir(storage_root, env_id)),
    }
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
fn resolve_library_env_id(
    library_id: Option<&str>,
    storage_root: &Path,
) -> Result<String, String> {
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
    /// 实例进程匹配口径：主库官方目录口径（含无参启动形态）；
    /// 副环境带参匹配（`--user-data-dir` 指向环境目录）。
    fn command_line_matches(&self, command_line: &str) -> bool;

    /// 关闭实例：主库用 close_master_instance（覆盖官方无参主进程），
    /// 副环境用 close_instance（带参口径）。
    fn close_instance(&self) -> Result<trae_instance_module::CloseOutcome, ()>;

    /// 重启实例（登录身份未变场景：收编/清理/归档应用收尾）。
    /// 窗口标题：主库 = 当前账号名（P5-5 既有口径）；副环境 = 环境名
    ///（与环境页启动入口一致，并行环境按名区分）。
    fn relaunch(&self, window_title: &str) -> Result<TraeInstanceLaunchDto, String>;
}

impl LibraryContextInstanceOps for LibraryContext {
    fn command_line_matches(&self, command_line: &str) -> bool {
        if self.is_master() {
            trae_instance_module::command_line_matches_master(command_line, &self.dir)
        } else {
            command_line_uses_data_dir(command_line, &self.dir)
        }
    }

    fn close_instance(&self) -> Result<trae_instance_module::CloseOutcome, ()> {
        let outcome = if self.is_master() {
            close_master_instance(&self.dir)
        } else {
            close_instance(&self.dir)
        };
        outcome.map_err(|_| ())
    }

    fn relaunch(&self, window_title: &str) -> Result<TraeInstanceLaunchDto, String> {
        // matcher 按环境口径二选一（launch_instance_common 收函数指针）。
        let matcher: fn(&str, &Path) -> bool = if self.is_master() {
            trae_instance_module::command_line_matches_master
        } else {
            command_line_uses_data_dir
        };
        launch_instance_common(&self.dir, window_title, None, matcher)
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
        // ADR-0025：库目录统一走 resolve_library_dir（指纹与读取同源）。
        let ctx = resolve_library_context(library_id.as_deref(), &storage_root)?;
        let current_account = library_account::resolve_library_account(
            &material_root,
            &storage_root,
            &ctx,
        )?
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
        let current_account = library_account::resolve_library_account(
            &material_root,
            &storage_root,
            &ctx,
        )?
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
        let current_account = library_account::resolve_library_account(
            &material_root,
            &storage_root,
            &ctx,
        )?
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

fn get_archive_draft_inner(storage_root: &Path, library_id: &str) -> Result<ArchiveDraftDto, String> {
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
        use traesync_infrastructure::master_checkup::{read_master_checkup, MasterCheckupStatus};
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
        let current = library_account::resolve_library_account(
            &material_root,
            &storage_root,
            &ctx,
        )?
        .and_then(LibraryAccountResolution::into_account);

        match read_master_checkup(&ctx.dir, &raw_key) {
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
    /// 主库重启结果。
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
    let raw_key = state.source_raw_key.clone();
    run_blocking("master_incorporate_join_failed", move || {
        let ctx = resolve_library_context(library_id.as_deref(), &storage_root)?;
        ctx.ensure_writable()?;
        incorporate_master_records_inner(&app, &material_root, &storage_root, &raw_key, &ctx)
    })
    .await
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
    ctx.close_instance()
        .map_err(|_| "master_incorporate_close_failed".to_string())?;

    // 第 2 步：三件套备份（ADR-0018 铁律；create_new 永不覆盖备份链）。
    emit("backing_up");
    // P11-4①/P11-5⑤：写入当前库作用域活动备份目录（副环境链与主库链隔离）。
    let backup_dir = resolve_library_backup_dir(storage_root, &db_path, &ctx.env_id);
    let backup_path = backup_master_trio_to(&db_path, Some(&backup_dir)).map_err(|error| match error {
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
            MasterHandoverError::IntegrityFailed => "master_incorporate_integrity_failed",
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
    let relaunch = ctx.relaunch(&window_title)?;
    emit("done");

    Ok(MasterIncorporateResultDto {
        merged_accounts: stale_count as u32,
        transferred_projects: handover.transferred_projects as u32,
        removed_mirror_rows: handover.removed_mirror_rows as u32,
        switched_sessions: handover.switched_sessions.len() as u32,
        backup_path: backup_path.display().to_string(),
        relay_ledger_written,
        relaunch_outcome: relaunch.outcome.to_string(),
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
        MasterHandoverError::IntegrityFailed => "trae_cn_merge_integrity_failed".to_string(),
        _ => "trae_cn_merge_db_open_failed".to_string(),
    }
}

/// P13-2 CN 归并命令共用前置：库守卫（仅 trae_cn）+ 当前指针账号解析。
/// 非法库 / 指针为空 / 指针悬空都以稳定错误码拒绝。
fn resolve_trae_cn_merge_context(
    material_root: &Path,
    storage_root: &Path,
    library_id: Option<&str>,
) -> Result<(LibraryContext, traesync_infrastructure::account_registry::AccountRecord), String> {
    let ctx = resolve_library_context(library_id, storage_root)?;
    ctx.ensure_project_merge_writable()?;
    let account = library_account::resolve_library_account(material_root, storage_root, &ctx)?
        .and_then(LibraryAccountResolution::into_account)
        .ok_or_else(|| "trae_cn_merge_no_current_account".to_string())?;
    Ok((ctx, account))
}

/// CN 产品注册表 user_id → 展示名（备注名优先，与账号页口径一致）。
fn trae_cn_account_name_by_user_id(trae_cn_root: &Path, user_id: &str) -> Option<String> {
    let records = AccountRegistry::new(trae_cn_root).load().ok()?;
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
        let Some(account) = library_account::resolve_library_account(
            &material_root,
            &storage_root,
            &ctx,
        )?
        .and_then(LibraryAccountResolution::into_account)
        else {
            return Ok(Vec::new());
        };
        Ok(read_trae_cn_foreign_projects(&ctx.dir, &raw_key, &account.account_id)
            .into_iter()
            .map(|entry| TraeCnForeignProjectDto {
                project_id: entry.project_id,
                name: entry.name,
                absolute_path: entry.absolute_path,
                session_count: entry.session_count,
                message_count: entry.message_count,
            })
            .collect())
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
        preview_trae_cn_project_merge_inner(&storage_root, &raw_key, &ctx, &account, &project_id)
    })
    .await
}

/// 归并预览主体（阻塞上下文，可注入测试）。
fn preview_trae_cn_project_merge_inner(
    storage_root: &Path,
    raw_key: &str,
    ctx: &LibraryContext,
    account: &traesync_infrastructure::account_registry::AccountRecord,
    project_id: &str,
) -> Result<TraeCnMergePreviewDto, String> {
    let db_path = master_database_path(&ctx.dir);
    let preview =
        preview_trae_cn_project_merge_infra(&db_path, raw_key, &account.account_id, project_id)
            .map_err(map_trae_cn_merge_error)?;
    let trae_cn_root = library_account::trae_cn_material_root_from(storage_root);
    let sources = preview
        .source_rows
        .iter()
        .map(|row| TraeCnMergeSourceDto {
            user_id: row.user_id.clone(),
            account_name: trae_cn_account_name_by_user_id(&trae_cn_root, &row.user_id),
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
    ensure_real_preview(&state)?;
    if state.source_raw_key.is_empty() {
        return Err("source_key_unavailable".to_string());
    }
    let material_root = checkin_material_root(&state)?;
    let storage_root = PathBuf::from(&state.storage_root);
    let raw_key = state.source_raw_key.clone();
    run_blocking("trae_cn_merge_join_failed", move || {
        let ctx = resolve_library_context(library_id.as_deref(), &storage_root)?;
        let (_, account) =
            resolve_trae_cn_merge_context(&material_root, &storage_root, library_id.as_deref())?;
        // Trae CN 客户端运行检测（真实进程枚举；单测注入替身）。
        let is_client_running = || {
            list_trae_processes()
                .map(|processes| {
                    processes.iter().any(|process| {
                        process
                            .exe_path
                            .as_deref()
                            .is_some_and(|exe| TraeProduct::TraeCn.matches_executable(Path::new(exe)))
                    })
                })
                .unwrap_or(false)
        };
        run_trae_cn_project_merge(
            &storage_root,
            &raw_key,
            &ctx,
            &account,
            &project_id,
            &is_client_running,
            &|event| {
                let _ = app.emit("trae-cn-merge-progress", event);
            },
        )
    })
    .await
}

/// CN 项目归并编排主体（阻塞上下文；客户端运行检测与进度回调可注入，
/// 供单测覆盖守卫矩阵与回执）。
fn run_trae_cn_project_merge(
    storage_root: &Path,
    raw_key: &str,
    ctx: &LibraryContext,
    account: &traesync_infrastructure::account_registry::AccountRecord,
    project_id: &str,
    is_client_running: &dyn Fn() -> bool,
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
    if preview.source_rows.iter().map(|row| row.normal_sessions).sum::<i64>() == 0 {
        return Err("trae_cn_merge_nothing_to_do".to_string());
    }

    // Trae CN 客户端运行中拒绝：归并需要静止一致的库做备份与事务写；
    // CN 实例不经工具托管，由用户关闭后重试。
    if is_client_running() {
        return Err("trae_cn_merge_client_running".to_string());
    }
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
    let raw_key = state.source_raw_key.clone();
    run_blocking("cleanup_master_join_failed", move || {
        let ctx = resolve_library_context(library_id.as_deref(), &storage_root)?;
        ctx.ensure_writable()?;
        cleanup_master_empty_projects_inner(&material_root, &storage_root, &raw_key, &ctx)
    })
    .await
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
    ctx.close_instance()
        .map_err(|_| "cleanup_master_close_failed".to_string())?;

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
    run_blocking("master_backup_join_failed", move || {
        let master_dir =
            master_data_dir().map_err(|_| "environment_registry_invalid".to_string())?;
        // 运行中禁止（口径同 get_environment_state_inner 的运行检测）。
        let running = list_trae_processes()
            .map(|processes| {
                processes.iter().any(|process| {
                    process.command_line.as_deref().is_some_and(|line| {
                        trae_instance_module::command_line_matches_master(line, &master_dir)
                    })
                })
            })
            .unwrap_or(false);
        if running {
            return Err("master_backup_running".to_string());
        }
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
/// 否则主库 db 同目录（原行为）。fail-soft：配置损坏回退默认位置，
/// 不阻断备份/切号主流程（与裁剪的静默容错同口径）。
pub(crate) fn resolve_master_backup_dir(storage_root: &Path, db_path: &Path) -> PathBuf {
    if let Some(dir) = load_backup_dir_override(storage_root) {
        return dir;
    }
    db_path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_default()
}

/// P11-5⑤：库作用域活动备份目录——副环境备份链与主库链物理隔离。
///
/// 主库口径不变（`resolve_master_backup_dir`：覆盖根或 db 同目录）；
/// 副环境在 backup_dir 覆盖时落到「覆盖目录/environments/{env_id}/」
/// 子目录——主库链、其他环境链互不可见，裁剪（同前缀文件名匹配）
/// 天然不越界；未覆盖时 = db 同目录（环境目录本身即隔离边界）。
/// 备份文件名保持 `{db 名}.switch-bak-{时间戳}`：恢复语义「按文件名
/// 找同目录」不变。
pub(crate) fn resolve_library_backup_dir(storage_root: &Path, db_path: &Path, env_id: &str) -> PathBuf {
    if env_id == MASTER_ENV_ID {
        return resolve_master_backup_dir(storage_root, db_path);
    }
    if let Some(dir) = load_backup_dir_override(storage_root) {
        return dir.join("environments").join(env_id);
    }
    db_path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_default()
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
const CREDENTIAL_BACKUP_ENTRIES: &[&str] =
    &["accounts.json", "profiles", "renewals", "refresh-rescue", "retired"];

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
    std::fs::create_dir_all(target)
        .map_err(|_| "master_backup_credentials_failed".to_string())?;
    let entries =
        std::fs::read_dir(source).map_err(|_| "master_backup_credentials_failed".to_string())?;
    for entry in entries {
        let entry =
            entry.map_err(|_| "master_backup_credentials_failed".to_string())?;
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
        let existing = store
            .load()
            .map_err(|_| "backup_retention_invalid".to_string())?;
        store
            .save(&BackupRetentionSettings {
                enabled,
                keep,
                backup_dir: existing.backup_dir,
            })
            .map_err(|_| "backup_retention_save_failed".to_string())
    })
    .await
}

/// P11-4①：读取备份位置覆盖配置（None = 主库同目录默认行为）。
/// 配置损坏时报错（fail-closed 展示层）：让用户看到配置坏了，而不是
/// 静默当作默认位置。
#[tauri::command]
pub(crate) async fn get_backup_dir(state: tauri::State<'_, AppState>) -> Result<Option<String>, String> {
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
        let mut settings = store
            .load()
            .map_err(|_| "backup_retention_invalid".to_string())?;
        settings.backup_dir = normalized.map(|path| path.display().to_string());
        store
            .save(&settings)
            .map_err(|_| "backup_retention_save_failed".to_string())
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
    run_blocking("master_backup_migrate_join_failed", move || {
        let storage = PathBuf::from(&storage_root);
        let target = validate_backup_dir(Some(&target_dir), &storage)?
            .ok_or_else(|| "backup_dir_invalid".to_string())?;
        let master_dir =
            master_data_dir().map_err(|_| "environment_registry_invalid".to_string())?;
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
            let chosen_path = active_dir.join(format!("{db_name}.switch-bak-{chosen_stamp}"));
            if !chosen_path.is_file() {
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
            }
            match read_session_facts(&chosen_path, &raw_key) {
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

/// 归档/恢复回执：实际受影响行数（集合内匹配状态的会话数）。
#[derive(Clone, Serialize)]
pub(crate) struct MasterArchiveResultDto {
    affected: u32,
}

/// P5-8a：兼容旧调用的单批归档原语（hidden_status → 'voice_discussion'）。
///
/// 当前界面使用 `apply_master_archive_changes` 统一完成关实例、备份、事务和重启；
/// 该命令保留给旧调用方，不作为新的逐条交互入口。
/// `library_id` 为库实例引用（ADR-0025，缺省主库）。
#[tauri::command]
pub(crate) async fn archive_master_sessions(
    session_ids: Vec<String>,
    library_id: Option<String>,
    state: tauri::State<'_, AppState>,
) -> Result<MasterArchiveResultDto, String> {
    ensure_real_preview(&state)?;
    if state.source_raw_key.is_empty() {
        return Err("source_key_unavailable".to_string());
    }
    let material_root = checkin_material_root(&state)?;
    let storage_root = PathBuf::from(&state.storage_root);
    let raw_key = state.source_raw_key.clone();
    run_blocking("master_archive_join_failed", move || {
        let ctx = resolve_library_context(library_id.as_deref(), &storage_root)?;
        ctx.ensure_writable()?;
        let current_account = require_library_account(&material_root, &storage_root, &ctx)?;
        archive_master_sessions_inner(
            &ctx.dir,
            &raw_key,
            &session_ids,
            &current_account.account_id,
        )
        .map(|affected| MasterArchiveResultDto {
            affected: affected as u32,
        })
        .map_err(map_master_archive_error)
    })
    .await
}

/// P5-8a：兼容旧调用的单批恢复原语（hidden_status 还原 NULL）。
///
/// 当前界面使用 `apply_master_archive_changes` 统一提交恢复；恢复后的会话
/// 仍按 work_mode 原生聚合归入当前账号。`library_id` 为库实例引用。
#[tauri::command]
pub(crate) async fn restore_master_sessions(
    session_ids: Vec<String>,
    library_id: Option<String>,
    state: tauri::State<'_, AppState>,
) -> Result<MasterArchiveResultDto, String> {
    ensure_real_preview(&state)?;
    if state.source_raw_key.is_empty() {
        return Err("source_key_unavailable".to_string());
    }
    let material_root = checkin_material_root(&state)?;
    let storage_root = PathBuf::from(&state.storage_root);
    let raw_key = state.source_raw_key.clone();
    run_blocking("master_archive_join_failed", move || {
        let ctx = resolve_library_context(library_id.as_deref(), &storage_root)?;
        ctx.ensure_writable()?;
        let current_account = require_library_account(&material_root, &storage_root, &ctx)?;
        restore_master_sessions_inner(
            &ctx.dir,
            &raw_key,
            &session_ids,
            &current_account.account_id,
        )
        .map(|affected| MasterArchiveResultDto {
            affected: affected as u32,
        })
        .map_err(map_master_archive_error)
    })
    .await
}

/// 归档草稿一次性提交回执；`relaunch_outcome=failed` 表示主库已更新，
/// 但实例待用户重试启动，不会再次重复写数据库。
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
    ensure_real_preview(&state)?;
    if state.source_raw_key.is_empty() {
        return Err("source_key_unavailable".to_string());
    }
    let raw_key = state.source_raw_key.clone();
    let material_root = checkin_material_root(&state)?;
    let storage_root = PathBuf::from(&state.storage_root);
    let storage_root_for_prune = state.storage_root.clone();
    run_blocking("master_archive_join_failed", move || {
        let ctx = resolve_library_context(library_id.as_deref(), &storage_root)?;
        ctx.ensure_archive_writable()?;
        let current_account = require_library_account(&material_root, &storage_root, &ctx)?;
        // P13-3：CN 库分支——Trae CN 实例不经工具托管（P13-2 同款纪律），
        // 无 close/relaunch 编排；客户端运行中拒绝（可执行文件口径），
        // 由用户关闭后重试。
        if ctx.env_id == TRAE_CN_LIBRARY_ID {
            let is_client_running = || {
                list_trae_processes()
                    .map(|processes| {
                        processes.iter().any(|process| {
                            process
                                .exe_path
                                .as_deref()
                                .is_some_and(|exe| {
                                    TraeProduct::TraeCn.matches_executable(Path::new(exe))
                                })
                        })
                    })
                    .unwrap_or(false)
            };
            return run_trae_cn_archive_apply(
                Path::new(&storage_root_for_prune),
                &raw_key,
                &ctx,
                &current_account.account_id,
                &archive_session_ids,
                &restore_session_ids,
                &is_client_running,
            );
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

        ctx.close_instance()
            .map_err(|_| "master_archive_close_failed".to_string())?;

        let restart_after_failure = || {
            let _ = ctx.relaunch(&window_title);
        };
        // P11-4①/P11-5⑤：写入当前库作用域活动备份目录（副环境链与主库链隔离）。
        let backup_dir =
            resolve_library_backup_dir(Path::new(&storage_root_for_prune), &db_path, &ctx.env_id);
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
        prune_backups_if_enabled_in(&db_path, Path::new(&storage_root_for_prune), &backup_dir);

        let outcome = match apply_master_archive_changes_inner(
            &ctx.dir,
            &raw_key,
            &archive_session_ids,
            &restore_session_ids,
            &current_account.account_id,
        ) {
            Ok(outcome) => outcome,
            Err(error) => {
                restart_after_failure();
                return Err(map_master_archive_error(error));
            }
        };

        let relaunch_outcome = match ctx.relaunch(&window_title) {
            Ok(relaunch) => relaunch.outcome.to_string(),
            Err(_) => "failed".to_string(),
        };
        Ok(MasterArchiveApplyResultDto {
            archived_sessions: outcome.archived_sessions as u32,
            restored_sessions: outcome.restored_sessions as u32,
            relaunch_outcome,
        })
    })
    .await
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
    is_client_running: &dyn Fn() -> bool,
) -> Result<MasterArchiveApplyResultDto, String> {
    // 双重守卫：命令层已校验，此处保持 inner 可独立调用（fail-closed）。
    ctx.ensure_archive_writable()?;
    let db_path = master_database_path(&ctx.dir);
    if !db_path.is_file() {
        return Err("master_archive_no_data".to_string());
    }
    // Trae CN 客户端运行中拒绝：归档写入需要静止一致的库做备份与事务。
    if is_client_running() {
        return Err("trae_cn_archive_client_running".to_string());
    }
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
    // CN 无重启编排：实例启动交还用户。回执恒为 launched（前端仅在
    // failed 时显示恢复入口，CN 路径不会出现该分支）。
    Ok(MasterArchiveApplyResultDto {
        archived_sessions: outcome.archived_sessions as u32,
        restored_sessions: outcome.restored_sessions as u32,
        relaunch_outcome: "launched".to_string(),
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
    ensure_real_preview(&state)?;
    if state.source_raw_key.is_empty() {
        return Err("source_key_unavailable".to_string());
    }
    let raw_key = state.source_raw_key.clone();
    let material_root = checkin_material_root(&state)?;
    let storage_root = PathBuf::from(&state.storage_root);
    // P5-9：删除前备份后的保留策略清理（闭包外拷出，State 引用不进闭包）。
    let storage_root_for_prune = state.storage_root.clone();
    run_blocking("master_archive_join_failed", move || {
        let ctx = resolve_library_context(library_id.as_deref(), &storage_root)?;
        ctx.ensure_writable()?;
        let current_account = require_library_account(&material_root, &storage_root, &ctx)?;
        // 运行中禁止（口径同 create_master_backup）：删除前必须能拿到静止
        // 一致的三件套备份。P11-5⑤：运行检测按库环境口径匹配。
        let running = list_trae_processes()
            .map(|processes| {
                processes.iter().any(|process| {
                    process
                        .command_line
                        .as_deref()
                        .is_some_and(|line| ctx.command_line_matches(line))
                })
            })
            .unwrap_or(false);
        if running {
            return Err("master_delete_running".to_string());
        }
        let result = delete_master_sessions_inner(
            &ctx.dir,
            &raw_key,
            &session_ids,
            &current_account.account_id,
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
            let backup_dir =
                resolve_library_backup_dir(Path::new(&storage_root_for_prune), &db_path, &ctx.env_id);
            prune_backups_if_enabled_in(&db_path, Path::new(&storage_root_for_prune), &backup_dir);
        }
        result
    })
    .await
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
    ensure_real_preview(&state)?;
    if state.source_raw_key.is_empty() {
        return Err("source_key_unavailable".to_string());
    }
    let raw_key = state.source_raw_key.clone();
    let material_root = checkin_material_root(&state)?;
    let storage_root = PathBuf::from(&state.storage_root);
    // P5-9：合并前备份后的保留策略清理（闭包外拷出，State 引用不进闭包）。
    let storage_root_for_prune = state.storage_root.clone();
    run_blocking("master_archive_join_failed", move || {
        let ctx = resolve_library_context(library_id.as_deref(), &storage_root)?;
        ctx.ensure_writable()?;
        let current_account = require_library_account(&material_root, &storage_root, &ctx)?;
        // 运行中禁止（口径同 delete_master_sessions）：合并前必须能拿到静止
        // 一致的三件套备份。P11-5⑤：运行检测按库环境口径匹配。
        let running = list_trae_processes()
            .map(|processes| {
                processes.iter().any(|process| {
                    process
                        .command_line
                        .as_deref()
                        .is_some_and(|line| ctx.command_line_matches(line))
                })
            })
            .unwrap_or(false);
        if running {
            return Err("master_merge_running".to_string());
        }
        let result = merge_master_projects_inner(
            &ctx.dir,
            &raw_key,
            &source_project_ids,
            &target_project_id,
            &current_account.account_id,
        )
        .map(|outcome: MasterMergeOutcome| MasterMergeResultDto {
            moved_sessions: outcome.moved_sessions as u32,
            removed_projects: outcome.removed_projects as u32,
            backup_path: outcome.backup_path,
        })
        .map_err(map_master_archive_error);
        // P5-9：合并链已生成新备份，按保留策略清理旧备份（静默容错）。
        // P11-5⑤：裁剪目标 = 库作用域活动目录（副环境不误伤主库链）。
        if result.is_ok() {
            let db_path = master_database_path(&ctx.dir);
            let backup_dir =
                resolve_library_backup_dir(Path::new(&storage_root_for_prune), &db_path, &ctx.env_id);
            prune_backups_if_enabled_in(&db_path, Path::new(&storage_root_for_prune), &backup_dir);
        }
        result
    })
    .await
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
    fn resolve_library_dir_maps_secondary_to_env_data_dir() {
        let fixture = ScopeFixture::new();
        let dir = resolve_library_dir(Some(&fixture.env_id), fixture.root.path()).unwrap();
        assert_eq!(dir, secondary_data_dir(fixture.root.path(), &fixture.env_id));
        // 未知库一律拒绝（不落默认库）。
        assert_eq!(
            resolve_library_dir(Some("env-none"), fixture.root.path()).unwrap_err(),
            "library_not_found"
        );
    }

    /// 副环境活动备份目录：backup_dir 覆盖时落在覆盖目录下的
    /// environments/{env_id} 子目录（与主库链隔离）；未覆盖时 = db 同目录
    ///（各环境目录天然隔离）。主库口径不变（覆盖根直用，不入子目录）。
    #[test]
    fn resolve_library_backup_dir_isolates_secondary_chains() {
        let fixture = ScopeFixture::new();
        let env_db = secondary_data_dir(fixture.root.path(), &fixture.env_id).join("ModularData")
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
        prune_backups_if_enabled_in(
            Path::new("C:/x/database.db"),
            fixture.root.path(),
            &custom,
        );
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

        prune_backups_if_enabled_in(
            Path::new("C:/x/database.db"),
            fixture.root.path(),
            &custom,
        );
        // 主备份与凭据子目录全部原样保留。
        assert!(custom.join("database.db.switch-bak-100").exists());
        assert!(custom.join("database.db.switch-bak-200").exists());
        assert!(custom.join("credentials-100").join("accounts.json").exists());
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
        let entries =
            RelayLedger::new(env_ledger.parent().unwrap().parent().unwrap().join(&fixture.env_id))
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
        let env_db = ctx.dir.join("ModularData").join("ai-agent").join("database.db");
        std::fs::create_dir_all(env_db.parent().unwrap()).unwrap();
        std::fs::write(env_db.with_file_name("database.db.switch-bak-100"), "a").unwrap();
        std::fs::write(env_db.with_file_name("database.db.switch-bak-200"), "b").unwrap();

        let chain = master_backup_chain_inner(fixture.root.path(), &ctx).unwrap();
        assert_eq!(chain.backups.len(), 2);
        // 目录即环境链所在目录（恢复语义「按文件名找同目录」）。
        assert_eq!(chain.backup_dir, env_db.parent().unwrap().display().to_string());
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

    /// P11-4①：活动备份目录解析——未配置回退 db 同目录；配置了覆盖用之；
    /// 配置损坏 fail-soft 回退 db 同目录（不阻断主流程）。
    #[test]
    fn resolve_master_backup_dir_prefers_configured_override() {
        let root = tempfile::tempdir().unwrap();
        let db_path = root.path().join("master").join("database.db");
        // 未配置：db 同目录。
        assert_eq!(
            resolve_master_backup_dir(root.path(), &db_path),
            root.path().join("master")
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
        assert_eq!(
            resolve_master_backup_dir(root.path(), &db_path),
            custom
        );
        // 配置损坏：fail-soft 回退 db 同目录。
        std::fs::write(root.path().join("backup-retention.json"), "not json").unwrap();
        assert_eq!(
            resolve_master_backup_dir(root.path(), &db_path),
            root.path().join("master")
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
        std::fs::write(material.join("profiles").join("abc.checkin"), b"TRVCKN01-cipher").unwrap();
        std::fs::write(material.join("renewals").join("op-1").join("manifest.json"), b"{}").unwrap();
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
        assert!(cred.join("renewals").join("op-1").join("bundle.before").is_file());
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
    use traesync_infrastructure::account_registry::AccountRecord;
    use traesync_infrastructure::product_account_state::ProductAccountStateStore;

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
                 INSERT INTO server_history_info (session_id, conversation_id, source, user_id)
                    VALUES ('s1', 'conv-old', 'llm_default', NULL);",
            );
            if with_sessions {
                sql.push_str(
                    "INSERT INTO chat_session VALUES ('s1', 'p1', '会话一', 1, 0, NULL);
                     INSERT INTO chat_session VALUES ('s2', 'p1', '会话二', 2, 0, NULL);
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
            // CN 产品池：指针账号（归并目标）+ 来源账号档案（预览名称反查）。
            let trae_cn_root = root.path().join("products").join("trae_cn");
            AccountRegistry::new(&trae_cn_root)
                .upsert(&account("profile-cn", "222", "指针账号"))
                .unwrap();
            AccountRegistry::new(&trae_cn_root)
                .upsert(&account("profile-src", "111", "来源账号"))
                .unwrap();
            ProductAccountStateStore::new(&trae_cn_root)
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
            &|| false,
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
        assert_eq!(Path::new(&receipt.backup_path).parent(), Some(db_dir.as_path()));
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
            .query_row("SELECT user_id FROM project WHERE project_id = 'p1'", [], |row| {
                row.get(0)
            })
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
        assert!(stages.contains(&"mapping") && stages.contains(&"executing") && stages.contains(&"verifying"));
        assert_eq!(stages.last().copied(), Some("done"));
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
            &|| true,
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
            &|| false,
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
            &|| false,
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
            fixture.root.path(),
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
        let env_id =
            traesync_infrastructure::environment_registry::EnvironmentRegistry::new(
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
            &|| false,
        )
        .unwrap();
        assert_eq!(receipt.archived_sessions, 1);
        assert_eq!(receipt.restored_sessions, 0);
        assert_eq!(receipt.relaunch_outcome, "launched");
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
            &|| false,
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
            &|| true,
        )
        .unwrap_err();
        assert_eq!(error, "trae_cn_archive_client_running");
        assert_eq!(fixture.backup_count(), 0, "拒绝路径不产生备份");
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
