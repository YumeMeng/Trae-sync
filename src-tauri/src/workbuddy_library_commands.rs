//! P19-B3：WorkBuddy 主库域命令（库页 WB 适配器；矩阵 #58-#75 WB 对应面）。
//!
//! 命令分组：
//! - 只读浏览（**在线可用**：WAL 读并发不阻塞 daemon）——统计/会话分组/
//!   消息正文/体检；
//! - 破坏性写（**单次确认入口**；进程纪律 = 完全退出 → 写 → 重启，同
//!   P18-B1 过户）——软删除 / 彻底删除（备份前置）/ cwd 归并。
//!
//! 红线（与 infrastructure::workbuddy_library 模块头一致）：
//! - `transport='cloud'` 一律不写（软删/彻底删除/归并 SQL 均限定 local）；
//! - 进程状态 Unknown 拒绝一切写副作用（P14-2 边界）。
//!
//! 与 TRAE 库域 18 条的对应（信息架构 §2.4）：#64→stats、#63→history、
//! #66→messages、#60→delete（软删）、#62/#65/#67/#70→checkup（孤儿正文/
//! 缺失正文/软删规模/外部目录）、#71→merge（cwd 归并）；#58/#61/#68/#73
//! 等草稿与收编语义由既有命令复用（archive_draft 产品无关、收编=
//! transfer_workbuddy_sessions），#69 launch 在 WB 一期 pending。

use std::path::PathBuf;
use std::time::{Duration, Instant};

use serde::Serialize;
use tauri::State;

use traesync_infrastructure::workbuddy_transfer::{
    close_client_real, count_client_processes_real, default_data_root, discover_install,
    observe_client_real, start_client_real, ClientProcessState,
};
use traesync_infrastructure::workbuddy_library::{
    list_sessions, merge_cwd_projects, purge_sessions, read_library_checkup, read_library_stats,
    read_session_messages, soft_delete_sessions,
};

use crate::workbuddy_commands::{ensure_login_real_mode_workbuddy, workbuddy_login_root};
use crate::{run_blocking, AppState};

/// 破坏性写回执公共尾（进程纪律面向；UI 呈现"已重启客户端/进程数"）。
#[derive(Debug, Serialize)]
pub struct WorkBuddyLibraryWriteTail {
    /// 本次是否执行了"关闭 → 重启"（写前客户端已在运行的场景为 true）。
    pub restarted: bool,
    /// 重启后客户端进程数（未重启/探测失败为 None）。
    pub process_count_after_restart: Option<usize>,
}

/// 软删除回执（UPDATE deleted_at；可逆）。
#[derive(Debug, Serialize)]
pub struct WorkBuddySoftDeleteDto {
    pub deleted_sessions: usize,
    pub skipped_ids: usize,
    pub tail: WorkBuddyLibraryWriteTail,
}

/// 彻底删除回执（决策 7 单独确认；行删除 + 正文文件删除）。
#[derive(Debug, Serialize)]
pub struct WorkBuddyPurgeDto {
    pub purged_sessions: usize,
    pub removed_body_files: usize,
    pub missing_body_files: usize,
    pub failed_body_files: Vec<String>,
    /// 前置滚动备份目录（备份失败则整单拒绝，不出现无备份的彻底删除）。
    pub backup_path: String,
    pub tail: WorkBuddyLibraryWriteTail,
}

/// cwd 归并回执。
#[derive(Debug, Serialize)]
pub struct WorkBuddyMergeCwdDto {
    pub moved_sessions: usize,
    pub moved_body_files: usize,
    pub missing_body_files: usize,
    pub conflict_body_files: Vec<String>,
    pub tail: WorkBuddyLibraryWriteTail,
}

// ===== 只读命令（在线） =====

/// WB 库统计（总览库统计卡 + 主库卡数据源；矩阵 #64）。
#[tauri::command]
pub(crate) fn get_workbuddy_library_stats(
    state: State<AppState>,
) -> Result<traesync_infrastructure::workbuddy_library::LibraryStats, String> {
    ensure_login_real_mode_workbuddy(&state)?;
    let data_root = default_data_root().ok_or("workbuddy_data_root_unavailable".to_string())?;
    read_library_stats(&data_root).map_err(|error| error.code().to_string())
}

/// WB 会话分组列表（cwd 分组；user_id=None 全库视图、含软删开关；
/// 矩阵 #63 视图面 + #70 归并入口数据源）。
#[tauri::command]
pub(crate) fn get_workbuddy_library_history(
    user_id: Option<String>,
    include_deleted: bool,
    state: State<AppState>,
) -> Result<Vec<traesync_infrastructure::workbuddy_library::CwdGroup>, String> {
    ensure_login_real_mode_workbuddy(&state)?;
    let data_root = default_data_root().ok_or("workbuddy_data_root_unavailable".to_string())?;
    list_sessions(
        &data_root,
        user_id.as_deref(),
        include_deleted,
    )
    .map_err(|error| error.code().to_string())
}

/// WB 会话消息正文（索引定位 + jsonl 解析；矩阵 #66）。
/// user_id=Some 时校验归属（UI 常规路径；None 为工具侧检查路径）。
#[tauri::command]
pub(crate) fn get_workbuddy_session_messages(
    session_id: String,
    user_id: Option<String>,
    state: State<AppState>,
) -> Result<traesync_infrastructure::workbuddy_library::SessionMessages, String> {
    ensure_login_real_mode_workbuddy(&state)?;
    let data_root = default_data_root().ok_or("workbuddy_data_root_unavailable".to_string())?;
    read_session_messages(&data_root, &session_id, user_id.as_deref())
        .map_err(|error| error.code().to_string())
}

/// WB 库体检（孤儿正文/缺失正文/软删规模/外部目录/归并候选；
/// 矩阵 #62/#65/#67/#70）。只读。
#[tauri::command]
pub(crate) fn get_workbuddy_library_checkup(
    state: State<AppState>,
) -> Result<traesync_infrastructure::workbuddy_library::LibraryCheckup, String> {
    ensure_login_real_mode_workbuddy(&state)?;
    let data_root = default_data_root().ok_or("workbuddy_data_root_unavailable".to_string())?;
    read_library_checkup(&data_root).map_err(|error| error.code().to_string())
}

// ===== 破坏性写命令（进程纪律：完全退出 → 写 → 重启） =====

/// 写前进程检查 + 写后重启补偿（P19 §4.4 纪律；注入式原语为真实进程，
/// 离线单测不覆盖本路径——单测口径见 workbuddy_transfer 19 单测）。
pub(crate) fn run_with_client_stopped<T>(
    write: impl FnOnce() -> Result<T, String>,
) -> Result<(T, WorkBuddyLibraryWriteTail), String> {
    // 1) 写前进程检查：Unknown 拒绝一切写副作用（P14-2 边界）。
    let mut need_restart = false;
    match observe_client_real() {
        ClientProcessState::NotRunning => {}
        ClientProcessState::Running => {
            // 2) 完全退出：优雅 → /F 强杀（WAL 强杀安全，B1 同纪律）。
            if !close_client_real() {
                return Err("wb_library_close_failed".to_string());
            }
            let deadline = Instant::now() + Duration::from_secs(15);
            loop {
                match observe_client_real() {
                    ClientProcessState::NotRunning => {
                        need_restart = true;
                        break;
                    }
                    ClientProcessState::Unknown => {
                        return Err("wb_library_process_state_unknown".to_string())
                    }
                    ClientProcessState::Running if Instant::now() < deadline => {
                        std::thread::sleep(Duration::from_millis(250));
                    }
                    ClientProcessState::Running => {
                        return Err("wb_library_close_failed".to_string())
                    }
                }
            }
        }
        ClientProcessState::Unknown => {
            return Err("wb_library_process_state_unknown".to_string());
        }
    }
    // 3) 写（失败直接返回——库由事务原子性保证，客户端已关不动它）。
    let value = write()?;
    // 4) 重启补偿（best-effort：探测失败不阻塞回执，用户可手动重启）。
    let mut tail = WorkBuddyLibraryWriteTail {
        restarted: need_restart,
        process_count_after_restart: None,
    };
    if need_restart {
        if let Some(found) = discover_install() {
            if start_client_real(&found.exe) {
                std::thread::sleep(Duration::from_secs(3));
                tail.process_count_after_restart = count_client_processes_real();
            }
        }
    }
    Ok((value, tail))
}

/// WB 会话软删除（**单次确认入口**；UPDATE deleted_at，可逆）。
/// cloud 行/他人账号行/已删行自动跳过（infrastructure 归属重校验）。
#[tauri::command]
pub(crate) async fn delete_workbuddy_sessions(
    session_ids: Vec<String>,
    user_id: String,
    state: State<'_, AppState>,
) -> Result<WorkBuddySoftDeleteDto, String> {
    ensure_login_real_mode_workbuddy(&state)?;
    run_blocking("workbuddy_library_delete_join_failed", move || {
        let data_root =
            default_data_root().ok_or("workbuddy_data_root_unavailable".to_string())?;
        let (outcome, tail) = run_with_client_stopped(|| {
            let now_ms = traesync_infrastructure::workbuddy_credential::unix_ms_now() as i64;
            soft_delete_sessions(&data_root, &session_ids, &user_id, now_ms)
                .map_err(|error| error.code().to_string())
        })?;
        Ok(WorkBuddySoftDeleteDto {
            deleted_sessions: outcome.deleted_sessions,
            skipped_ids: outcome.skipped_ids,
            tail,
        })
    })
    .await
}

/// WB 会话彻底删除（**决策 7 单独确认入口**；备份前置 = 滚动单份，
/// 备份失败整单拒绝——不出现无备份的破坏性终点）。
#[tauri::command]
pub(crate) async fn purge_workbuddy_sessions(
    session_ids: Vec<String>,
    user_id: String,
    state: State<'_, AppState>,
) -> Result<WorkBuddyPurgeDto, String> {
    ensure_login_real_mode_workbuddy(&state)?;
    // 备份目标目录（storage_root/workbuddy-backups；storage_root 缺失时
    // 直接拒绝——彻底删除的备份前置不可静默降级）。
    if state.storage_root.is_empty() {
        return Err("workbuddy_storage_unavailable".to_string());
    }
    let backup_target = std::path::PathBuf::from(&state.storage_root).join("workbuddy-backups");
    let login_root = workbuddy_login_root(&state).ok();
    run_blocking("workbuddy_library_purge_join_failed", move || {
        let data_root =
            default_data_root().ok_or("workbuddy_data_root_unavailable".to_string())?;
        // 备份前置（滚动单份；P19 决策 1 范围：凭据 + 库 + 正文全纳入）。
        let sources = backup_sources_for_data_root(&data_root, login_root.as_deref())?;
        let backup = traesync_infrastructure::workbuddy_backup::backup_workbuddy_scope_to(
            &sources, &backup_target,
        )
        .map_err(|error| error.code().to_string())?;
        let backup_path = backup.backup_dir.display().to_string();
        let (outcome, tail) = run_with_client_stopped(|| {
            purge_sessions(&data_root, &session_ids, &user_id)
                .map_err(|error| error.code().to_string())
        })?;
        Ok(WorkBuddyPurgeDto {
            purged_sessions: outcome.purged_sessions,
            removed_body_files: outcome.removed_body_files,
            missing_body_files: outcome.missing_body_files,
            failed_body_files: outcome.failed_body_files,
            backup_path,
            tail,
        })
    })
    .await
}

/// WB cwd 归并（**单次确认入口**；多 cwd 变体会话合并到规范 cwd）。
#[tauri::command]
pub(crate) async fn merge_workbuddy_cwd_projects(
    canonical_cwd: String,
    variant_cwds: Vec<String>,
    state: State<'_, AppState>,
) -> Result<WorkBuddyMergeCwdDto, String> {
    ensure_login_real_mode_workbuddy(&state)?;
    run_blocking("workbuddy_library_merge_join_failed", move || {
        let data_root =
            default_data_root().ok_or("workbuddy_data_root_unavailable".to_string())?;
        let (outcome, tail) = run_with_client_stopped(|| {
            merge_cwd_projects(&data_root, &canonical_cwd, &variant_cwds)
                .map_err(|error| error.code().to_string())
        })?;
        Ok(WorkBuddyMergeCwdDto {
            moved_sessions: outcome.moved_sessions,
            moved_body_files: outcome.moved_body_files,
            missing_body_files: outcome.missing_body_files,
            conflict_body_files: outcome.conflict_body_files,
            tail,
        })
    })
    .await
}

// ===== 内部 helper =====

/// 备份目标目录（storage_root/workbuddy-backups；storage_root 缺失时报错
/// ——彻底删除的备份前置不可静默降级）。
fn state_backup_root() -> Result<PathBuf, String> {
    // 通过 AppState 取：由命令体捕获后传入？此处经参数化 helper 不可达——
    // 直接由调用方在闭包外取好传入。见 purge_workbuddy_sessions。
    unreachable!("备份目录由 purge_workbuddy_sessions 直接计算")
}

/// 组装备份源（P19 决策 1 全纳入范围；app 根缺数据时报错——备份锚点纪律）。
pub(crate) fn backup_sources_for_data_root(
    data_root: &std::path::Path,
    login_root: Option<&std::path::Path>,
) -> Result<traesync_infrastructure::workbuddy_backup::WorkBuddyBackupSources, String> {
    if !data_root.is_dir() {
        return Err("workbuddy_data_root_unavailable".to_string());
    }
    Ok(traesync_infrastructure::workbuddy_backup::WorkBuddyBackupSources {
        app_config_root: data_root.join("app"),
        auth_dir: std::env::var_os("LOCALAPPDATA")
            .map(|base| {
                std::path::PathBuf::from(base)
                    .join("CodeBuddyExtension")
                    .join("Data")
                    .join("Public")
                    .join("auth")
            })
            .unwrap_or_else(|| data_root.join(".auth-missing")),
        login_container_dir: login_root
            .map(std::path::Path::to_path_buf)
            .unwrap_or_else(|| data_root.join(".containers-missing")),
        data_root: data_root.to_path_buf(),
    })
}
