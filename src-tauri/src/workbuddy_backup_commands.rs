//! P19-B8：WorkBuddy 备份链管理命令 + 探针数据面命令（fe B6 走查缺口：
//! WB 备份此前内嵌于 purge 前置，无独立管理面——本模块补齐设置页管理面
//! 数据源）。
//!
//! 命令分组：
//! - `list_workbuddy_backups`：备份链枚举（对齐 get_master_backup_chain
//!   形态：stamp/成员数/源字节/磁盘字节 + 备份目录）；
//! - `create_workbuddy_backup`：手动触发（**进程纪律：完全退出 → 写 →
//!   重启**——备份对象含 workbuddy.db，活库带 wal 备份不一致；滚动单份
//!   语义对齐 Trae，写新份后清旧）；
//! - `restore_workbuddy_backup(stamp)`：恢复（**进程纪律同上**；恢复前
//!   自动以 keep_chain 变体再备份当前态一次防误恢复——滚动语义会清掉
//!   即将恢复的目标份，前置备份必须保链）；
//! - `delete_workbuddy_backups(stamps, confirmed)`：手动删除（对齐
//!   delete_master_backups 语义：单次确认兜底/链内限定/硬删/remaining
//!   回执；备份文件为工具自有数据，与客户端运行态无关——无需关停）；
//! - `get_workbuddy_probe_info`：客户端版本/安装路径/进程运行状态
//!   （wb-explorer 探查的 18488 探针 API + 三级安装探测，只读、fail-soft，
//!   供环境页连接卡展示——客户端未运行不是错误，如实回 null）。
//!
//! 备份源与目标目录与 purge 前置一致（单一事实来源）：
//! - 源 = P19 决策 1 全纳入（workbuddy.db[-wal|-shm] + identity 材料 +
//!   projects 正文 + P17-A3 凭据白名单），见
//!   `workbuddy_library_commands::backup_sources_for_data_root`；
//! - 目标 = `storage_root/workbuddy-backups`。

use std::path::PathBuf;

use serde::Serialize;

use traesync_infrastructure::workbuddy_backup::{
    backup_workbuddy_scope_stamped, backup_workbuddy_scope_stamped_keep_chain,
    hard_delete_workbuddy_backup, restore_workbuddy_scope_from, WorkBuddyBackupSources,
    // 命令函数与 infra 枚举同名（tauri invoke 名 = 函数名），导入别名区分。
    list_workbuddy_backups as list_scope_backups,
};

use crate::run_blocking;
use crate::workbuddy_library_commands::{backup_sources_for_data_root, run_with_client_stopped};
use crate::workbuddy_commands::ensure_login_real_mode_workbuddy;
use crate::AppState;

/// 备份目标目录（与 purge 前置同一目录——storage_root 缺失拒绝）。
fn backups_target_dir(state: &tauri::State<'_, AppState>) -> Result<PathBuf, String> {
    if state.storage_root.is_empty() {
        return Err("workbuddy_storage_unavailable".to_string());
    }
    Ok(std::path::PathBuf::from(&state.storage_root).join("workbuddy-backups"))
}

// ===== 备份链枚举 =====

/// 备份链条目（对齐 get_master_backup_chain 成员形态）。
#[derive(Debug, Clone, Serialize)]
pub(crate) struct WorkBuddyBackupEntryDto {
    /// 备份时间戳（秒级 UNIX 时间；目录名 workbuddy-data-{stamp}）。
    pub stamp_unix_seconds: u64,
    /// manifest 声明的成员文件数（manifest 不可读为 None——半成品目录如实呈现）。
    pub file_count: Option<usize>,
    /// manifest 声明的源字节量。
    pub source_bytes: Option<u64>,
    /// 备份目录实际磁盘字节（含 manifest，遍历统计）。
    pub compressed_bytes: u64,
}

/// 备份链回执。
#[derive(Debug, Clone, Serialize)]
pub(crate) struct WorkBuddyBackupChainDto {
    pub backups: Vec<WorkBuddyBackupEntryDto>,
    /// 备份所在目录（展示定位）。
    pub backup_dir: String,
}

/// 枚举主体（阻塞上下文独立函数，离线单测口径）。
pub(crate) fn workbuddy_backups_list_inner(target_dir: &std::path::Path) -> WorkBuddyBackupChainDto {
    WorkBuddyBackupChainDto {
        backups: list_scope_backups(target_dir)
            .into_iter()
            .map(|entry| WorkBuddyBackupEntryDto {
                stamp_unix_seconds: entry.stamp_unix_seconds,
                file_count: entry.file_count,
                source_bytes: entry.source_bytes,
                compressed_bytes: entry.compressed_bytes,
            })
            .collect(),
        backup_dir: target_dir.display().to_string(),
    }
}

/// WB 备份链枚举（设置页备份管理面数据源，只读）。
#[tauri::command]
pub(crate) fn list_workbuddy_backups(
    state: tauri::State<'_, AppState>,
) -> Result<WorkBuddyBackupChainDto, String> {
    ensure_login_real_mode_workbuddy(&state)?;
    let target_dir = backups_target_dir(&state)?;
    Ok(workbuddy_backups_list_inner(&target_dir))
}

// ===== 手动创建 =====

/// 手动备份回执。
#[derive(Debug, Clone, Serialize)]
pub(crate) struct WorkBuddyBackupCreateDto {
    /// 发布的备份目录（workbuddy-data-{stamp}）。
    pub backup_dir: String,
    pub stamp_unix_seconds: u64,
    pub file_count: usize,
    pub source_bytes: u64,
    /// 滚动清理删除的旧份数（首份为 0；语义对齐 Trae 滚动单份）。
    pub rolled_over: usize,
}

/// 创建主体（显式 stamp 便于离线单测；生产用 unix_seconds_now）。
pub(crate) fn workbuddy_backup_create_inner(
    target_dir: &std::path::Path,
    sources: &WorkBuddyBackupSources,
    stamp: u64,
) -> Result<WorkBuddyBackupCreateDto, String> {
    let outcome = backup_workbuddy_scope_stamped(sources, target_dir, stamp)
        .map_err(|error| error.code().to_string())?;
    Ok(WorkBuddyBackupCreateDto {
        backup_dir: outcome.backup_dir.display().to_string(),
        stamp_unix_seconds: stamp,
        file_count: outcome.file_count,
        source_bytes: outcome.source_bytes,
        rolled_over: outcome.rolled_over,
    })
}

/// WB 手动备份（**单次确认入口**；进程纪律 = 完全退出 → 写 → 重启——
/// 备份对象含 workbuddy.db，活库快照不一致）。滚动单份：写新份成功后
/// 彻底删除目录内其余旧份（对齐 Trae create_master_backup 语义）。
#[tauri::command]
pub(crate) async fn create_workbuddy_backup(
    state: tauri::State<'_, AppState>,
) -> Result<WorkBuddyBackupCreateDto, String> {
    ensure_login_real_mode_workbuddy(&state)?;
    let target_dir = backups_target_dir(&state)?;
    let data_root =
        traesync_infrastructure::workbuddy_transfer::default_data_root()
            .ok_or("workbuddy_data_root_unavailable".to_string())?;
    let login_root = crate::workbuddy_commands::workbuddy_login_root(&state).ok();
    run_blocking("workbuddy_backup_create_join_failed", move || {
        let sources = backup_sources_for_data_root(&data_root, login_root.as_deref())?;
        let stamp = traesync_infrastructure::workbuddy_credential::unix_ms_now() / 1000;
        let (outcome, _tail) = run_with_client_stopped(|| {
            workbuddy_backup_create_inner(&target_dir, &sources, stamp)
        })?;
        Ok(outcome)
    })
    .await
}

// ===== 恢复 =====

/// 恢复回执。
#[derive(Debug, Clone, Serialize)]
pub(crate) struct WorkBuddyBackupRestoreDto {
    pub restored_files: usize,
    pub restored_bytes: u64,
    /// 恢复前自动再备份的当前态 stamp（防误恢复的回退点）。
    pub safety_backup_stamp: u64,
    /// 恢复的来源备份 stamp。
    pub restored_stamp: u64,
}

/// 恢复主体（显式 now 便于离线单测；纪律：链内限定 → 前置保链备份 →
/// 恢复覆盖）。
pub(crate) fn workbuddy_backup_restore_inner(
    target_dir: &std::path::Path,
    sources: &WorkBuddyBackupSources,
    stamp: u64,
    now: u64,
) -> Result<WorkBuddyBackupRestoreDto, String> {
    // 链内限定：目标 stamp 必须在当前链条（与 delete 同一口径——不按
    // 目录名猜测，异名散件/过期 stamp 不认）。
    let chain: Vec<u64> = traesync_infrastructure::workbuddy_backup::list_workbuddy_backups(
        target_dir,
    )
    .into_iter()
    .map(|entry| entry.stamp_unix_seconds)
    .collect();
    if !chain.contains(&stamp) {
        return Err("workbuddy_restore_backup_missing".to_string());
    }
    // 恢复前置：再备份当前态一次（keep_chain——滚动会清掉目标份）。
    // 同秒冲突（极小概率）顺延 stamp，永不覆盖既有备份。
    let mut safety_stamp = now;
    loop {
        let attempt = backup_workbuddy_scope_stamped_keep_chain(
            sources,
            target_dir,
            safety_stamp,
        );
        match attempt {
            Ok(_) => break,
            Err(traesync_infrastructure::workbuddy_backup::WorkBuddyBackupError::TargetExists) => {
                safety_stamp += 1;
            }
            Err(error) => return Err(error.code().to_string()),
        }
    }
    // 恢复（覆盖语义 = 目标内容替换为备份内容；sha256 逐成员校验，
    // 任一失败 fail-closed 中止——半写目标已清理，前置备份可回退）。
    let backup_dir = target_dir.join(format!("workbuddy-data-{stamp}"));
    let outcome = restore_workbuddy_scope_from(&backup_dir, sources)
        .map_err(|error| error.code().to_string())?;
    Ok(WorkBuddyBackupRestoreDto {
        restored_files: outcome.restored_files,
        restored_bytes: outcome.restored_bytes,
        safety_backup_stamp: safety_stamp,
        restored_stamp: stamp,
    })
}

/// WB 备份恢复（**单次确认入口**；进程纪律 = 完全退出 → 恢复 → 重启
/// ——恢复覆盖 workbuddy.db/凭据/正文，客户端运行中不可恢复）。恢复前
/// 自动再备份当前态一次（keep_chain 保链），回执带安全份 stamp 供 UI
/// 呈现回退点。
#[tauri::command]
pub(crate) async fn restore_workbuddy_backup(
    stamp: u64,
    state: tauri::State<'_, AppState>,
) -> Result<WorkBuddyBackupRestoreDto, String> {
    ensure_login_real_mode_workbuddy(&state)?;
    let target_dir = backups_target_dir(&state)?;
    let data_root =
        traesync_infrastructure::workbuddy_transfer::default_data_root()
            .ok_or("workbuddy_data_root_unavailable".to_string())?;
    let login_root = crate::workbuddy_commands::workbuddy_login_root(&state).ok();
    run_blocking("workbuddy_backup_restore_join_failed", move || {
        let sources = backup_sources_for_data_root(&data_root, login_root.as_deref())?;
        let now = traesync_infrastructure::workbuddy_credential::unix_ms_now() / 1000;
        let (outcome, _tail) = run_with_client_stopped(|| {
            workbuddy_backup_restore_inner(&target_dir, &sources, stamp, now)
        })?;
        Ok(outcome)
    })
    .await
}

// ===== 手动删除（对齐 delete_master_backups 语义） =====

/// 手动删除回执（链条一致性口径：删除后剩余条目数直读）。
#[derive(Debug, Clone, Serialize)]
pub(crate) struct WorkBuddyBackupDeleteDto {
    pub deleted_stamps: Vec<u64>,
    pub missing_stamps: Vec<u64>,
    pub remaining: usize,
    pub backup_dir: String,
}

/// 删除主体（单次确认兜底 + 链内限定 + 硬删；离线单测口径）。
pub(crate) fn workbuddy_backups_delete_inner(
    target_dir: &std::path::Path,
    stamps: &[u64],
    confirmed: bool,
) -> Result<WorkBuddyBackupDeleteDto, String> {
    if !confirmed {
        return Err("workbuddy_backup_delete_not_confirmed".to_string());
    }
    if stamps.is_empty() {
        return Err("workbuddy_backup_delete_empty_selection".to_string());
    }
    let chain: Vec<u64> = list_scope_backups(target_dir)
        .into_iter()
        .map(|entry| entry.stamp_unix_seconds)
        .collect();
    let mut deleted_stamps = Vec::new();
    let mut missing_stamps = Vec::new();
    for stamp in stamps {
        if chain.contains(stamp) {
            hard_delete_workbuddy_backup(target_dir, *stamp);
            deleted_stamps.push(*stamp);
        } else {
            missing_stamps.push(*stamp);
        }
    }
    let remaining = list_scope_backups(target_dir).len();
    Ok(WorkBuddyBackupDeleteDto {
        deleted_stamps,
        missing_stamps,
        remaining,
        backup_dir: target_dir.display().to_string(),
    })
}

/// WB 备份手动删除（**单次确认入口**：UI 弹窗确认后 confirmed=true）。
/// 链内限定（用户从链条勾选）；硬删直删不走回收站（备份属可再生数据，
/// spec 10.4）；滚动「仅保留一份」语义不受影响——本命令只删点名条目。
#[tauri::command]
pub(crate) async fn delete_workbuddy_backups(
    stamps: Vec<u64>,
    confirmed: bool,
    state: tauri::State<'_, AppState>,
) -> Result<WorkBuddyBackupDeleteDto, String> {
    ensure_login_real_mode_workbuddy(&state)?;
    let target_dir = backups_target_dir(&state)?;
    run_blocking("workbuddy_backup_delete_join_failed", move || {
        workbuddy_backups_delete_inner(&target_dir, &stamps, confirmed)
    })
    .await
}

// ===== 探针数据面（环境页连接卡） =====

/// 探针信息回执（全部字段 fail-soft——客户端未运行/探针不可达/安装未
/// 探测到均如实回 null，不是错误；环境页连接卡恒可渲染）。
#[derive(Debug, Clone, Serialize)]
pub(crate) struct WorkBuddyProbeInfoDto {
    /// 客户端进程运行状态（进程枚举失败 = None 不猜测）。
    pub running: Option<bool>,
    /// 进程数（枚举成功时有值）。
    pub process_count: Option<usize>,
    /// 探针 API 回执（客户端运行中且 18488 端口组可达时有值）。
    pub app: Option<String>,
    pub version: Option<String>,
    pub platform: Option<String>,
    /// 安装路径（三级探测；探测失败 None）。
    pub install_path: Option<String>,
    /// 安装探测来源（env_override / process_command_line / common_path / registry）。
    pub install_source: Option<String>,
    /// 数据根（~/.workbuddy；环境变量缺失 None）。
    pub data_root: Option<String>,
}

/// 探针信息组装内核（注入式，离线单测各 fail-soft 分支）。
pub(crate) fn workbuddy_probe_info_inner(
    process_count: Option<usize>,
    probe: Option<(String, String, String)>,
    install: Option<(PathBuf, String)>,
    data_root: Option<PathBuf>,
) -> WorkBuddyProbeInfoDto {
    WorkBuddyProbeInfoDto {
        running: process_count.map(|count| count > 0),
        process_count,
        app: probe.as_ref().map(|(app, _, _)| app.clone()),
        version: probe.as_ref().map(|(_, version, _)| version.clone()),
        platform: probe.map(|(_, _, platform)| platform),
        install_path: install.as_ref().map(|(exe, _)| exe.display().to_string()),
        install_source: install.map(|(_, source)| source),
        data_root: data_root.map(|root| root.display().to_string()),
    }
}

/// WB 探针信息（环境页连接卡数据源；只读零写副作用——进程枚举 + 本地
/// 探针 API GET + 安装路径三级探测，全部 fail-soft）。
#[tauri::command]
pub(crate) fn get_workbuddy_probe_info(
    state: tauri::State<'_, AppState>,
) -> Result<WorkBuddyProbeInfoDto, String> {
    ensure_login_real_mode_workbuddy(&state)?;
    // 进程枚举：成功 → 计数；失败（权限等）→ None 不猜测。
    let process_count =
        traesync_infrastructure::workbuddy_instance::list_workbuddy_processes()
            .ok()
            .map(|list| list.len());
    // 探针 API：best-effort（运行中但探针异常 → 版本面 None，如实呈现）。
    let probe = traesync_infrastructure::workbuddy_instance::probe_local_api_default()
        .ok()
        .map(|info| (info.app, info.version, info.platform));
    // 安装路径三级探测。
    let install = traesync_infrastructure::workbuddy_transfer::discover_install().map(
        |found| {
            let source = match found.source {
                traesync_infrastructure::workbuddy_transfer::InstallSource::EnvOverride => {
                    "env_override"
                }
                traesync_infrastructure::workbuddy_transfer::InstallSource::ProcessCommandLine => {
                    "process_command_line"
                }
                traesync_infrastructure::workbuddy_transfer::InstallSource::CommonPath => {
                    "common_path"
                }
                traesync_infrastructure::workbuddy_transfer::InstallSource::Registry => "registry",
            };
            (found.exe, source.to_string())
        },
    );
    let data_root = traesync_infrastructure::workbuddy_transfer::default_data_root();
    Ok(workbuddy_probe_info_inner(
        process_count,
        probe,
        install,
        data_root,
    ))
}
