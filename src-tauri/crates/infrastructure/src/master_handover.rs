//! 主库记录交接原语（P5-1，Q1.1 五步事务核心）。
//!
//! 依据（`.scratch/grill-log-20260830-env-model.md` §8-§19 实证链）：
//! - **E1b**：本地会话按 `project.user_id` 过滤；单事务 UPDATE 即完成记录
//!   归属随行，消息数据零触碰（`server_history_info.user_id` 全 NULL）。
//! - **E5 坑位**：`UNIQUE(biz_project_id, user_id)` —— 每个登录过环境的账号
//!   都会在自己名下拉一份同 biz 的空镜像行；交接前必须自动清理目标账号的
//!   空镜像行，普通交接的非空冲突报人工决策（禁止静默覆盖）。
//! - **e5-r2**：活跃会话原地换腿——事务内重写本地 session/message/turn/
//!   task/history/agent_run 身份为新腿，不携带旧 `server_history_info`，
//!   新腿首发自然创建新云端 conversation（换腿 SQL 逐句移植自验证探针
//!   `.scratch/history-u6/w0-probe/src/bin/rewrite_leg_in_place.rs`）。
//! - **§19 产品化约束**：不自动清理孤儿行；本地镜像（server_history_info）
//!   保留旧身份作为证据，永不改写、永不删除。
//!
//! 本模块只做第 4 步「交接」；关实例/备份/blob 互换/重启由调用方
//! （lib.rs 五步编排）负责。提交前失败由单事务整体回滚；提交结果无法确认
//! 或提交后完整性校验失败时，调用方必须保留阶段错误并检查主库。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rusqlite::Connection;

/// 主库对话库相对路径：`{master_data_dir}\ModularData\ai-agent\database.db`
/// （与账号实例库同构，见 account_session_index）。
pub fn master_database_path(master_data_dir: &Path) -> PathBuf {
    master_data_dir
        .join("ModularData")
        .join("ai-agent")
        .join("database.db")
}

/// 备份链使用的库文件名提取（P12-R2a S2 收敛：lib.rs 三处同形复制合一）。
/// 从 `master_database_path` 产出的库路径取文件名；无文件名的路径
/// fail-closed，沿用调用方既有稳定错误码 `environment_registry_invalid`。
pub fn master_backup_db_name(db_path: &Path) -> Result<String, String> {
    db_path
        .file_name()
        .and_then(|name| name.to_str())
        .map(|name| name.to_string())
        .ok_or_else(|| "environment_registry_invalid".to_string())
}

/// 交接失败（全部非敏感：不含 key、消息内容或账号信息）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MasterHandoverError {
    /// 主库 database.db 不存在（主库尚未启动/登录过，无库可交接）。
    DbUnavailable,
    /// 打开或解密失败（raw key 不匹配或库损坏）。
    DbOpenFailed,
    /// UNIQUE(biz_project_id, user_id) 非空冲突，需人工决策（附 biz 短指纹）。
    TargetConflict(String),
    /// 事务已提交，但提交后完整性校验未通过或读回失败。
    CommittedNeedsAttention,
    /// SQLite 返回提交错误，无法证明事务最终持久状态。
    CommitOutcomeUnknown,
    /// 文件读写失败。
    Io,
    /// P13-2：定位的项目行不存在（已被清理或 id 失效）。
    ProjectNotFound,
    /// P13-2：项目行不满足归并条件（无路径/无 biz/归属目标账号/已软删，
    /// ADR-0033 决策 3）。
    ProjectNotMergeable,
    /// P13-2：同目录其他账号没有可并入的正常会话（只有冻结内容）。
    NothingToMerge,
}

impl std::fmt::Display for MasterHandoverError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let message = match self {
            Self::DbUnavailable => "主库对话库不存在",
            Self::DbOpenFailed => "主库对话库打开或解密失败",
            Self::TargetConflict(_) => "目标账号名下存在同名项目的非空记录，需人工决策",
            Self::CommittedNeedsAttention => "记录交接已提交，但写后完整性校验未通过，需检查主库",
            Self::CommitOutcomeUnknown => "记录交接提交结果无法确认，需检查主库",
            Self::Io => "交接文件读写失败",
            Self::ProjectNotFound => "项目记录不存在或已被清理",
            Self::ProjectNotMergeable => "该项目没有关联文件夹路径，不能参与归并",
            Self::NothingToMerge => "同目录其他账号没有可并入的会话",
        };
        formatter.write_str(message)
    }
}

impl std::error::Error for MasterHandoverError {}

/// 统一标记提交错误，避免调用路径把未知持久状态误报为可回滚失败。
fn commit_handover_transaction(tx: rusqlite::Transaction<'_>) -> Result<(), MasterHandoverError> {
    tx.commit()
        .map_err(|_| MasterHandoverError::CommitOutcomeUnknown)
}

/// 事务提交后的查询失败只能表示“已提交但无法验证”，不能降级为普通数据库错误。
fn post_commit_verification<T>(
    result: Result<T, MasterHandoverError>,
) -> Result<T, MasterHandoverError> {
    result.map_err(|_| MasterHandoverError::CommittedNeedsAttention)
}

/// 一个被换腿的会话（接力台账数据源）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HandoverSession {
    /// 新腿 session_id（换腿后身份）。
    pub session_id: String,
    /// 旧腿 session_id（换腿前身份；P5-3 接力台账 from_session_id 数据源，
    /// 历史页据此把多次换腿链回成完整接力轨迹）。
    pub previous_session_id: String,
    /// 会话随行后的 project_id；合并时指向目标项目。
    pub project_id: String,
    /// 交接前会话归属账号 user_id（P5-5 收编台账逐会话 from 数据源；
    /// 切号路径沿用 previous_owner 单值，多账号杂居时此字段更精确）。
    pub previous_user_id: String,
    /// 交接时会话累计消息数（chat_message 行数，台账轨迹用）。
    pub message_count: i64,
}

/// 交接结果（事务提交且提交后完整性校验通过才返回）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MasterHandover {
    /// 交接前记录的实际归属账号（非目标 user_id 中项目数最多者；
    /// Q3 不变量下 = 主库切换前登录账号；无任何记录时为 None）。
    pub previous_owner_user_id: Option<String>,
    /// 归属随行的 project 行数。
    pub transferred_projects: usize,
    /// 自动清理的空镜像行数（目标账号名下 0 会话的同 biz 行）。
    pub removed_mirror_rows: usize,
    /// 换腿的全部会话。
    pub switched_sessions: Vec<HandoverSession>,
}

/// 交接进度快照（P7-3：mapping / executing / verifying 三阶段上报；
/// label 为项目名等用户可读文本，技术 ID 不进主视野）。
#[derive(Debug, Clone)]
pub struct HandoverProgress {
    /// 阶段：mapping（读旧库建映射）/ executing（事务内批量重写）/
    /// verifying（写后校验）。
    pub phase: &'static str,
    /// 当前序号（executing 阶段为身份组序号，其余为会话序号；1 起）。
    pub current: usize,
    /// 总数（会话数或身份组数）。
    pub total: usize,
    /// 用户可读标签（项目名或身份组说明）。
    pub label: String,
}

// ===== 三件套备份（ADR-0018：破坏性批量操作前备份铁律）=====

fn sidecar(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(suffix);
    path.with_file_name(name)
}

// P16-T1：原 copy_file_create_new（未压缩复制）随备份压缩化删除——
// 压缩写出统一走 compress_file_create_new。

/// 当前秒级 UNIX 时间（备份链命名与凭据子目录同戳发号）。
fn unix_seconds_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// 主库三件套备份：db + 存在的 wal/shm 复制为 `.switch-bak-{时间戳}` sidecar
/// （写入 db 同目录，原行为保留给既有调用方）。
///
/// `create_new` 保证永不覆盖既有备份（含历史备份链）；调用方（切号编排）
/// 保证此刻主库实例已关闭（第 1 步），复制到的是静止一致的三件套。
/// 返回备份主文件路径（供 DTO 展示与人工恢复定位）。
pub fn backup_master_trio(db_path: &Path) -> Result<PathBuf, MasterHandoverError> {
    backup_master_trio_to(db_path, None)
}

/// P11-4①：三件套备份写入指定目录（None = db 同目录）。
///
/// 目录不存在时创建（设置命令层已校验可创建，此处兜底）；文件名与同目录
/// 备份链一致（`{db 名}.switch-bak-{时间戳}`），恢复语义不变：按文件名
/// 找同目录。
pub fn backup_master_trio_to(
    db_path: &Path,
    target_dir: Option<&Path>,
) -> Result<PathBuf, MasterHandoverError> {
    backup_master_trio_stamped(db_path, target_dir, unix_seconds_now())
}

/// 显式时间戳版本（P11-4②）：凭据子目录（`credentials-{stamp}`）需要与
/// 三件套同戳，由组合根统一发号传入，恢复时按戳配对。
///
/// **P16-T1 zstd 压缩形态**：三件套按成员分别压缩写出（db →
/// `{db 名}.switch-bak-{stamp}.zst`、wal/shm → `…{suffix}.zst`）。
/// 格式决策（选最简且恢复可靠）：**每成员独立标准 zstd 帧、无自定义
/// 容器**——相比 tar-less 单档打包，省去成员长度表与切分逻辑，任意标准
/// zstd 工具可直接解压单个成员人工恢复；wal/shm 缺席语义与旧格式一致
/// （各自独立文件，有则备、无则跳）；单成员损坏不波及其他成员。
/// 压缩级别 3（zstd 默认）：1.6GB 主库的体积/耗时平衡点（spec 目标
/// 约 400MB；更高级别收益递减且拖慢切号路径）。恢复/回读/迁移路径对
/// 旧未压缩与新压缩双格式兼容（见 `backup_member_file_names`、
/// `list_master_backups_in`、`decompress_backup_member`）。
pub fn backup_master_trio_stamped(
    db_path: &Path,
    target_dir: Option<&Path>,
    stamp: u64,
) -> Result<PathBuf, MasterHandoverError> {
    if !db_path.is_file() {
        return Err(MasterHandoverError::DbUnavailable);
    }
    let dir = match target_dir {
        Some(dir) => {
            std::fs::create_dir_all(dir).map_err(|_| MasterHandoverError::Io)?;
            dir.to_path_buf()
        }
        None => db_path.parent().map(Path::to_path_buf).unwrap_or_default(),
    };
    let db_name = db_path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or(MasterHandoverError::Io)?;
    // 与同目录备份链同名（`{db 名}.switch-bak-{stamp}.zst`）：恢复语义不变，
    // 按文件名找同目录。create_new 永不覆盖既有备份。
    let backup = dir.join(format!("{db_name}.switch-bak-{stamp}.zst"));
    compress_file_create_new(db_path, &backup)?;
    for suffix in ["-wal", "-shm"] {
        let source = db_path.with_file_name(format!("{db_name}{suffix}"));
        if source.is_file() {
            compress_file_create_new(
                &source,
                &dir.join(format!("{db_name}.switch-bak-{stamp}{suffix}.zst")),
            )?;
        }
    }
    Ok(backup)
}

/// P17-A3 彻底删除原语：直删一份主库备份链条目（db/wal/shm 成员 +
/// 同 stamp 凭据子目录），**不走回收站**。
///
/// 纪律依据（spec 10.4，2026-10-18）：删除类操作分两档——safe-delete
/// （回收站 shim）仅用于用户可能误删、需找回的场景；备份文件属**可再生
/// 数据**（可由当前主库重新生成），进入回收站只会持续占用 E 盘水位
/// （单份 1.6GB+），因此备份清理一律走本原语 `std::fs::remove_file` /
/// `remove_dir_all` 直删。返回实际删除的成员文件数（凭据子目录不计数）。
pub fn hard_delete_master_backup(backup_dir: &Path, db_name: &str, stamp: u64) -> usize {
    let mut removed = 0;
    for name in backup_member_file_names(db_name, stamp) {
        let member = backup_dir.join(&name);
        // 直删；文件不存在时 remove 失败但也不存在（幂等，不计入）。
        if std::fs::remove_file(&member).is_ok() {
            removed += 1;
        }
    }
    // R0-P3：同 stamp 凭据子目录随主备份连带直删（存在才删，失败静默）。
    let credentials = backup_dir.join(format!("credentials-{stamp}"));
    if credentials.is_dir() {
        let _ = std::fs::remove_dir_all(&credentials);
    }
    removed
}

/// P17-A3 滚动单份备份（spec 10.2）：写新份成功后，彻底删除目录内其余
/// 全部旧链条目（含凭据子目录）——同一时刻全目录只有一份备份；keep=N
/// 链式保留退役。
///
/// 顺序决策（与任务书「先删后写」的差异，选更安全的等价实现）：
/// **先写新份、成功后再删旧份**。若先删后写，压缩中途失败会让目录内
/// 零备份（唯一的保险副本被先毁）；先写后删在最坏情况（新份写失败）
/// 下退化为保留旧份——备份系统的失败模式必须是「多留」而非「丢失」。
/// 可观察结果与任务书一致：成功路径结束后目录内只有新份一份。
pub fn backup_master_trio_rolling_stamped(
    db_path: &Path,
    target_dir: &Path,
    stamp: u64,
) -> Result<PathBuf, MasterHandoverError> {
    let backup = backup_master_trio_stamped(db_path, Some(target_dir), stamp)?;
    let db_name = db_path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or(MasterHandoverError::Io)?;
    // 滚动清理：新份之外的全部链条目（旧未压缩/旧压缩/凭据子目录）直删。
    for entry in list_master_backups_in(target_dir, db_name) {
        if entry.stamp_unix_seconds != stamp {
            hard_delete_master_backup(target_dir, db_name, entry.stamp_unix_seconds);
        }
    }
    Ok(backup)
}

/// 滚动单份备份（当前时间戳版本；语义见 [`backup_master_trio_rolling_stamped`]）。
pub fn backup_master_trio_rolling(
    db_path: &Path,
    target_dir: &Path,
) -> Result<PathBuf, MasterHandoverError> {
    backup_master_trio_rolling_stamped(db_path, target_dir, unix_seconds_now())
}

/// P16-T1：备份链成员候选文件名（双格式）——旧未压缩与新压缩 `.zst`
/// 各 3 个（db/wal/shm）。枚举、裁剪、迁移共用一份命名事实源。
pub fn backup_member_file_names(db_name: &str, stamp: u64) -> Vec<String> {
    let base = format!("{db_name}.switch-bak-{stamp}");
    let mut names = Vec::with_capacity(6);
    for suffix in ["", "-wal", "-shm"] {
        names.push(format!("{base}{suffix}"));
        names.push(format!("{base}{suffix}.zst"));
    }
    names
}

/// P16-T1：把源文件压缩为 `.zst` 写出（create_new 永不覆盖既有文件）。
/// 压缩级别 3（见 `backup_master_trio_stamped` 格式决策注释）。
/// 公开给组合根（tauri app）复用（旧→压缩迁移的暂存压缩同口径）。
pub fn compress_file_create_new(
    source: &Path,
    target: &Path,
) -> Result<(), MasterHandoverError> {
    let input = std::fs::File::open(source).map_err(|_| MasterHandoverError::Io)?;
    let output = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(target)
        .map_err(|_| MasterHandoverError::Io)?;
    let mut encoder =
        zstd::stream::write::Encoder::new(output, 3).map_err(|_| MasterHandoverError::Io)?;
    std::io::copy(&mut &input, &mut encoder).map_err(|_| MasterHandoverError::Io)?;
    // finish 落帧（不 finish 则尾部校验帧缺失，解压端不完整）。
    encoder.finish().map_err(|_| MasterHandoverError::Io)?;
    Ok(())
}

/// P16-T1：把压缩备份成员解压为普通文件（恢复/回读路径用；SQLCipher
/// 需要真实文件路径，不能流式打开）。公开给组合根复用。
pub fn decompress_backup_member(
    zst_path: &Path,
    target: &Path,
) -> Result<(), MasterHandoverError> {
    let input = std::fs::File::open(zst_path).map_err(|_| MasterHandoverError::Io)?;
    let mut decoder =
        zstd::stream::read::Decoder::new(input).map_err(|_| MasterHandoverError::Io)?;
    let output = std::fs::File::create(target).map_err(|_| MasterHandoverError::Io)?;
    let mut writer = std::io::BufWriter::new(output);
    std::io::copy(&mut decoder, &mut writer).map_err(|_| MasterHandoverError::Io)?;
    // flush 需要 Write trait 在作用域内（BufWriter 落盘缓冲）。
    std::io::Write::flush(&mut writer).map_err(|_| MasterHandoverError::Io)?;
    Ok(())
}

// ===== 备份链枚举（P5-4 设置页备份分区数据源，只读）=====

/// 一份主库数据备份（`.switch-bak-{时间戳}` 及其 wal/shm 附属件聚合）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MasterBackupEntry {
    /// 备份时间戳（文件名内秒级 UNIX 时间）。
    pub stamp_unix_seconds: u64,
    /// 备份合计字节（db + 存在的 wal/shm）。
    pub total_bytes: u64,
    /// 是否带 wal 附属件（运行中备份会缺失一致性，正常切号备份可有可无）。
    pub has_wal: bool,
    /// 是否带凭据备份子目录（`credentials-{stamp}`，P11-4②）。
    pub has_credentials: bool,
}

/// 枚举主库备份链：`{db 目录}/database.db.switch-bak-*`（db 同目录，原行为）。
pub fn list_master_backups(db_path: &Path) -> Vec<MasterBackupEntry> {
    let Some(dir) = db_path.parent() else {
        return Vec::new();
    };
    let Some(db_name) = db_path.file_name().and_then(|name| name.to_str()) else {
        return Vec::new();
    };
    list_master_backups_in(dir, db_name)
}

/// 枚举指定目录内的主库备份链（P11-4①：backup_dir 支撑——备份所在目录
/// 与 db 目录解耦后，按「活动备份目录 + db 文件名」枚举）。
///
/// 只读扫描 + 按时间戳倒序（最新在前）；命名异常的散件跳过不报错
/// （备份链展示容错：不因一个异名文件让整页失败）。不读取文件内容。
/// **P16-T1 双格式**：同时识别旧未压缩（`{stamp}`/`{stamp}-wal`/`-shm`）
/// 与新压缩（同名 + `.zst` 后缀）形态；同 stamp 两形态并存时合并为一条
/// （字节合计、有 wal 即记 true——迁移窗口内的过渡现场不丢链项）。
pub fn list_master_backups_in(dir: &Path, db_name: &str) -> Vec<MasterBackupEntry> {
    let prefix = format!("{db_name}.switch-bak-");
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    // stamp → (总字节, 是否带 wal) 聚合。
    let mut chain: HashMap<u64, (u64, bool)> = HashMap::new();
    for entry in entries.flatten() {
        let Ok(name) = entry.file_name().into_string() else {
            continue;
        };
        let Some(rest) = name.strip_prefix(&prefix) else {
            continue;
        };
        // P16-T1：先剥离压缩后缀再解析成员与时间戳——rest 形如
        // `{stamp}`、`{stamp}-wal`、`{stamp}-shm`，或压缩形态
        // `{stamp}.zst`、`{stamp}-wal.zst`、`{stamp}-shm.zst`。
        let uncompressed_rest = rest.strip_suffix(".zst").unwrap_or(rest);
        let (stamp_part, side) = match uncompressed_rest.strip_suffix("-wal") {
            Some(stamp) => (stamp, "-wal"),
            None => match uncompressed_rest.strip_suffix("-shm") {
                Some(stamp) => (stamp, "-shm"),
                None => (uncompressed_rest, ""),
            },
        };
        let Ok(stamp) = stamp_part.parse::<u64>() else {
            continue;
        };
        let Ok(size) = entry.metadata().map(|meta| meta.len()) else {
            continue;
        };
        let bucket = chain.entry(stamp).or_insert((0, false));
        bucket.0 += size;
        if side == "-wal" {
            bucket.1 = true;
        }
    }
    let mut result: Vec<MasterBackupEntry> = chain
        .into_iter()
        .map(
            |(stamp_unix_seconds, (total_bytes, has_wal))| MasterBackupEntry {
                stamp_unix_seconds,
                total_bytes,
                has_wal,
                // P11-4②：凭据子目录存在性（与三件套同戳命名）。
                has_credentials: dir
                    .join(format!("credentials-{stamp_unix_seconds}"))
                    .is_dir(),
            },
        )
        .collect();
    result.sort_by(|a, b| b.stamp_unix_seconds.cmp(&a.stamp_unix_seconds));
    result
}

// ===== 生成中检测（Q1.2：WAL 活动判定「回复生成中」）=====

/// WAL 活动采样点：mtime + 大小双指标（任一变化 = 有写入）。
#[derive(Debug, Clone, PartialEq, Eq)]
struct TrioSample {
    mtime_unix_nanos: u128,
    size: u64,
}

fn sample_wal(db_path: &Path) -> Option<TrioSample> {
    let wal = sidecar(db_path, "-wal");
    let metadata = std::fs::metadata(&wal).ok()?;
    Some(TrioSample {
        mtime_unix_nanos: metadata
            .modified()
            .ok()?
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or_default(),
        size: metadata.len(),
    })
}

/// 两次采样之间 WAL 是否有写入（纯函数，供单测）。
fn wal_activity_between(first: &Option<TrioSample>, second: &Option<TrioSample>) -> bool {
    match (first, second) {
        (Some(first), Some(second)) => first != second,
        // WAL 出现或消失都是写入活动（消失 = checkpoint 落盘）。
        (None, Some(_)) | (Some(_), None) => true,
        (None, None) => false,
    }
}

/// 生成中检测：间隔 `interval` 两次采样主库 WAL，有变化 = TRAE 正在写库
/// （典型场景 = 回复生成中）。检测为「活跃」时切号编排应提示用户
/// 「等它完成还是强制切换」（Q1.2），不自动强制。
pub fn master_db_activity_detected(db_path: &Path, interval: Duration) -> bool {
    let first = sample_wal(db_path);
    std::thread::sleep(interval);
    let second = sample_wal(db_path);
    wal_activity_between(&first, &second)
}

// ===== 原地换腿 ID 生成（e5-r2 探针同款：纳秒递增 + 6c 前缀 24 hex）=====

struct IdGenerator {
    next: u128,
}

impl IdGenerator {
    fn new() -> Self {
        Self {
            next: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos(),
        }
    }

    fn next(&mut self) -> String {
        let value = self.next;
        self.next += 1;
        format!("6c{:022x}", value & ((1_u128 << 88) - 1))
    }
}

// ===== 交接事务 =====

/// 换腿身份组（身份表 → 展示标签与引用列）：交接核心与 P13-2 CN 归并
/// 变体共用同一组清单（五类身份，重写顺序一致）。
const IDENTITY_GROUPS: [(&str, &str, &[&str]); 5] = [
    (
        "消息",
        "id_map_message",
        &["message_id", "reply_to_message_id", "response_message_id"],
    ),
    ("对话轮次", "id_map_turn", &["turn_id"]),
    ("任务", "id_map_task", &["task_id"]),
    ("历史记录", "id_map_history", &["history_v2_id"]),
    ("运行记录", "id_map_agent_run", &["agent_run_id"]),
];

fn columns(conn: &Connection, table: &str) -> Result<Vec<String>, MasterHandoverError> {
    let mut statement = conn
        .prepare(&format!("PRAGMA table_info(\"{table}\")"))
        .map_err(|_| MasterHandoverError::DbOpenFailed)?;
    let rows: Vec<String> = statement
        .query_map([], |row| row.get(1))
        .map_err(|_| MasterHandoverError::DbOpenFailed)?
        .collect::<Result<Vec<String>, _>>()
        .map_err(|_| MasterHandoverError::DbOpenFailed)?;
    Ok(rows)
}

/// 构造集合查询使用的参数占位符。
fn placeholders(count: usize) -> String {
    vec!["?"; count].join(", ")
}

fn table_names(conn: &Connection) -> Result<Vec<String>, MasterHandoverError> {
    let mut statement = conn
        .prepare(
            "SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%' ORDER BY name",
        )
        .map_err(|_| MasterHandoverError::DbOpenFailed)?;
    let rows: Vec<String> = statement
        .query_map([], |row| row.get(0))
        .map_err(|_| MasterHandoverError::DbOpenFailed)?
        .collect::<Result<Vec<String>, _>>()
        .map_err(|_| MasterHandoverError::DbOpenFailed)?;
    Ok(rows)
}

/// biz 短指纹（冲突报错用，非敏感：只取前 8 字符）。
fn biz_fingerprint(biz: &str) -> String {
    biz.chars().take(8).collect()
}

/// 主库记录交接（单事务）：空镜像清理 → 归属随行 → 全量换腿 → 完整性校验。
///
/// - 归属随行（Q3 全量）：`UPDATE project SET user_id = target WHERE user_id != target`
///   —— 主库 = 大家的对话库，账号 = 当前门票；随行前行级 UNIQUE 冲突按
///   E5 生产化要求处理（空镜像自动删、非空冲突报错）。
/// - 原地换腿（e5-r2）：挂接在被随行 project 上的全部未删除会话重写本地
///   身份；`server_history_info`（云端镜像）与全部备份永不触碰（证据保留）。
/// - 批量重写（P7-3）：全部会话的换腿映射合并进 temp 映射表，每个
///   （表 × 引用列）只发一条集合 UPDATE——语义与逐 ID 重写等价
///   （映射键全局唯一、新 ID 全新生成不与旧值相撞），条数从
///   O(会话 × 表 × 列 × ID) 降为 O(表 × 列)。
///
/// 前置条件：主库 TRAE 实例已关闭（调用方五步编排第 1 步保证）。
pub fn handover_master_records(
    db_path: &Path,
    raw_key: &str,
    target_user_id: &str,
) -> Result<MasterHandover, MasterHandoverError> {
    handover_master_records_with_progress(db_path, raw_key, target_user_id, &|_| {})
}

/// 带 progress 回调的交接入口（P7-3 进度可见化）：mapping / executing /
/// verifying 三阶段逐项上报；回调抛出的 panic 随事务一起回滚（安全）。
pub fn handover_master_records_with_progress(
    db_path: &Path,
    raw_key: &str,
    target_user_id: &str,
    on_progress: &dyn Fn(HandoverProgress),
) -> Result<MasterHandover, MasterHandoverError> {
    handover_master_records_core(db_path, raw_key, target_user_id, None, false, on_progress)
}

/// 指定来源账号的选择性交接（手动转移场景）：只把 source 名下带正常会话的
/// 项目随行到 target，库里其余账号（如更早登录过的账号）的记录原状保留。
pub fn handover_master_records_from_with_progress(
    db_path: &Path,
    raw_key: &str,
    target_user_id: &str,
    source_user_id: &str,
    on_progress: &dyn Fn(HandoverProgress),
) -> Result<MasterHandover, MasterHandoverError> {
    handover_master_records_core(
        db_path,
        raw_key,
        target_user_id,
        Some(source_user_id),
        false,
        on_progress,
    )
}

/// 指定来源账号的选择性交接 + 合并模式：同 biz 双侧都挂有真实记录时，
/// 把来源账号待交接的正常会话并入目标项目。来源项目仍有归档或软删除会话时保留，
/// 目标名下的会话和项目身份不改写。
pub fn handover_master_records_from_merge_with_progress(
    db_path: &Path,
    raw_key: &str,
    target_user_id: &str,
    source_user_id: &str,
    on_progress: &dyn Fn(HandoverProgress),
) -> Result<MasterHandover, MasterHandoverError> {
    handover_master_records_core(
        db_path,
        raw_key,
        target_user_id,
        Some(source_user_id),
        true,
        on_progress,
    )
}

// ===== P13-2 Trae CN 项目归并（ADR-0033 选择性写，同目录跨账号行合并）=====
//
// 定位键抉择（评审定案，报告理由）：**biz_project_id**（经用户点击的
// project 行解析），而非 absolute_path——
// ① 交接原语的镜像清理/冲突/合并机制全部以 biz 为键，且 UNIQUE
//    (biz_project_id, user_id) 正是「同目录跨账号天然多行」的成因，
//    biz 就是官方客户端维护的目录身份；
// ② absolute_path 是可空自由文本（CN 探针 6/17 为空），分隔符/大小写
//    无约束，相等匹配不稳；CN 实测 biz 全部非空（project_biz_nonnull=17）；
// ③ 无路径（含无 biz）项目按 ADR-0033 决策 3 不参与目录匹配，fail-closed。
//
// 归并语义（收窄自 from_merge 原语，单事务）：
// 空镜像清理（目标空镜像 + 来源空镜像）→ 来源正常会话并入承接行
// （chat_session.project_id + session_project 改挂）→ 移空来源行清理
// （残留归档/软删会话的行保留为原账号名下的冻结内容）→ 承接行归属
// 改写 → 全量换腿重写（IDENTITY_GROUPS 同款五类身份 + 会话本体）→
// 完整性校验。承接行 = 目标名下同目录真实行（UNIQUE 冲突分支）；
// 目标无真实行时 = 用户点击的来源行（整体转入，身份与路径保留）。

/// 同目录一侧的 project 行 + 规模计数（预览与归并共用的行形态）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TraeCnMergeRow {
    pub project_id: String,
    pub user_id: String,
    /// 原始项目名（可能为空；展示回退由命令层处理）。
    pub project_name: String,
    /// 将被并入的正常会话数（未删除且未归档）。
    pub normal_sessions: i64,
    /// 全部会话行数（含归档/软删，空镜像判定依据）。
    pub all_sessions: i64,
    /// 正常会话的消息总数（规模确认用）。
    pub messages: i64,
}

/// Trae CN 项目归并预览：同目录跨账号行集合（只读，确认弹层数据源）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TraeCnProjectMergePreview {
    pub biz_project_id: String,
    pub absolute_path: Option<String>,
    /// 当前账号名下的同目录行（None = 该目录只有其他账号的记录，整体转入）。
    pub target_row: Option<TraeCnMergeRow>,
    /// 其他账号名下的同目录行（归并来源，按 rowid 序）。
    pub source_rows: Vec<TraeCnMergeRow>,
}

/// 打开带 key 的库连接（busy_timeout + PRAGMA key；行为与交接核心的
/// 内联打开一致，CN 归并两入口共用）。
fn open_keyed_connection(db_path: &Path, raw_key: &str) -> Result<Connection, MasterHandoverError> {
    let conn = Connection::open(db_path).map_err(|_| MasterHandoverError::DbOpenFailed)?;
    conn.busy_timeout(Duration::from_millis(5000))
        .map_err(|_| MasterHandoverError::DbOpenFailed)?;
    conn.execute_batch(&format!("PRAGMA key = \"x'{raw_key}'\";"))
        .map_err(|_| MasterHandoverError::DbOpenFailed)?;
    Ok(conn)
}

/// 归并来源定位与门槛校验：项目行必须存在、未软删、有非空 biz 与
/// 非空 absolute_path、归属非目标账号（ADR-0033 决策 3 fail-closed）。
/// 通过后返回目录身份 biz。
fn locate_trae_cn_merge_source(
    conn: &Connection,
    target_user_id: &str,
    source_project_id: &str,
) -> Result<String, MasterHandoverError> {
    // 列防御：无 absolute_path 列的库没有任何可归并对象（fail-closed）。
    let project_columns = columns(conn, "project")?;
    if !project_columns.iter().any(|name| name == "absolute_path") {
        return Err(MasterHandoverError::ProjectNotMergeable);
    }
    let has_deleted = project_columns.iter().any(|name| name == "deleted_at");
    let deleted_expr = if has_deleted { ", deleted_at" } else { "" };
    let row: Option<(Option<String>, Option<String>, Option<String>, Option<i64>)> = {
        let sql = format!(
            "SELECT biz_project_id, absolute_path, user_id{deleted_expr} FROM project WHERE project_id = ?1"
        );
        let mut statement = conn
            .prepare(&sql)
            .map_err(|_| MasterHandoverError::DbOpenFailed)?;
        statement
            .query_row([source_project_id], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
            })
            .map(Some)
            .or_else(|error| match error {
                rusqlite::Error::QueryReturnedNoRows => Ok(None),
                _ => Err(MasterHandoverError::DbOpenFailed),
            })?
    };
    let Some((biz, path, user, deleted)) = row else {
        return Err(MasterHandoverError::ProjectNotFound);
    };
    let mergeable = biz.as_deref().is_some_and(|biz| !biz.trim().is_empty())
        && path.as_deref().is_some_and(|path| !path.trim().is_empty())
        && user.as_deref().is_some_and(|user| user != target_user_id)
        && deleted.unwrap_or(0) == 0;
    if !mergeable {
        return Err(MasterHandoverError::ProjectNotMergeable);
    }
    Ok(biz.expect("mergeable 已校验 biz 非空"))
}

/// 定位同目录跨账号行集合（行计数一次聚合查询；预览与归并共用）。
/// 返回（biz, 目标行, 来源行）；已软删项目行不参与。
fn trae_cn_merge_scope(
    conn: &Connection,
    target_user_id: &str,
    source_project_id: &str,
) -> Result<(String, Option<TraeCnMergeRow>, Vec<TraeCnMergeRow>), MasterHandoverError> {
    let biz = locate_trae_cn_merge_source(conn, target_user_id, source_project_id)?;
    let has_hidden_status = columns(conn, "chat_session")?
        .iter()
        .any(|name| name == "hidden_status");
    let has_msg_deleted = columns(conn, "chat_message")?
        .iter()
        .any(|name| name == "deleted_at");
    let hidden_cs = if has_hidden_status {
        " AND COALESCE(cs.hidden_status, '') != 'voice_discussion'"
    } else {
        ""
    };
    let msg_deleted = if has_msg_deleted {
        " AND COALESCE(m.deleted_at, 0) = 0"
    } else {
        ""
    };
    let mut statement = conn
        .prepare(&format!(
            "SELECT p.project_id, p.user_id, COALESCE(p.name, ''),
                    (SELECT COUNT(*) FROM chat_session cs WHERE cs.project_id = p.project_id
                       AND COALESCE(cs.deleted_at, 0) = 0{hidden_cs}),
                    (SELECT COUNT(*) FROM chat_session cs WHERE cs.project_id = p.project_id),
                    (SELECT COUNT(*) FROM chat_message m
                       JOIN chat_session cs ON cs.session_id = m.session_id
                     WHERE cs.project_id = p.project_id
                       AND COALESCE(cs.deleted_at, 0) = 0{hidden_cs}{msg_deleted})
             FROM project p
             WHERE p.biz_project_id = ?1 AND p.user_id IS NOT NULL
               AND COALESCE(p.deleted_at, 0) = 0
             ORDER BY p.rowid"
        ))
        .map_err(|_| MasterHandoverError::DbOpenFailed)?;
    let rows = statement
        .query_map([&biz], |row| {
            Ok(TraeCnMergeRow {
                project_id: row.get(0)?,
                user_id: row.get(1)?,
                project_name: row.get(2)?,
                normal_sessions: row.get(3)?,
                all_sessions: row.get(4)?,
                messages: row.get(5)?,
            })
        })
        .map_err(|_| MasterHandoverError::DbOpenFailed)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| MasterHandoverError::DbOpenFailed)?;
    let target_row = rows
        .iter()
        .find(|row| row.user_id == target_user_id)
        .cloned();
    let source_rows = rows
        .into_iter()
        .filter(|row| row.user_id != target_user_id)
        .collect();
    Ok((biz, target_row, source_rows))
}

/// 归并预览（只读）：同目录各账号行 / 会话数 / 消息数，供确认弹层
/// 展示规模。定位失败（无路径/未知 id 等）与归并同门槛。
pub fn preview_trae_cn_project_merge(
    db_path: &Path,
    raw_key: &str,
    target_user_id: &str,
    source_project_id: &str,
) -> Result<TraeCnProjectMergePreview, MasterHandoverError> {
    if !db_path.is_file() {
        return Err(MasterHandoverError::DbUnavailable);
    }
    let conn = open_keyed_connection(db_path, raw_key)?;
    let (biz_project_id, target_row, source_rows) =
        trae_cn_merge_scope(&conn, target_user_id, source_project_id)?;
    let absolute_path: Option<String> = conn
        .query_row(
            "SELECT absolute_path FROM project WHERE project_id = ?1",
            [source_project_id],
            |row| row.get(0),
        )
        .map_err(|_| MasterHandoverError::DbOpenFailed)?;
    Ok(TraeCnProjectMergePreview {
        biz_project_id,
        absolute_path,
        target_row,
        source_rows,
    })
}

/// Trae CN 项目归并（单事务，带进度回调）：把同目录（同 biz）其他账号
/// 的 project 行并入当前指针账号——空镜像清理 → 会话并入承接行 +
/// session_project 改挂 → 换腿重写 → 完整性校验。任一步失败整体回滚。
pub fn merge_trae_cn_project_records_with_progress(
    db_path: &Path,
    raw_key: &str,
    target_user_id: &str,
    source_project_id: &str,
    on_progress: &dyn Fn(HandoverProgress),
) -> Result<MasterHandover, MasterHandoverError> {
    if !db_path.is_file() {
        return Err(MasterHandoverError::DbUnavailable);
    }
    let conn = open_keyed_connection(db_path, raw_key)?;

    // ===== 事务外定位与规模预检（任一拒绝路径库零改动）=====
    let (biz, target_row, source_rows) =
        trae_cn_merge_scope(&conn, target_user_id, source_project_id)?;
    if source_rows
        .iter()
        .map(|row| row.normal_sessions)
        .sum::<i64>()
        == 0
    {
        return Err(MasterHandoverError::NothingToMerge);
    }

    // 承接行：目标名下同目录真实行（UNIQUE 冲突分支，目标身份保留）；
    // 否则为用户点击的来源行（整体转入/多账号并入都以点击行承接）。
    let mut mirrors_to_delete: Vec<String> = Vec::new();
    let survivor_id = match &target_row {
        Some(row) if row.all_sessions > 0 => row.project_id.clone(),
        Some(row) => {
            // 目标名下的空镜像行：与来源空镜像同样清理（E5 语义），
            // 让位给转入的承接行（UNIQUE(biz, target) 唯一键）。
            mirrors_to_delete.push(row.project_id.clone());
            source_project_id.to_string()
        }
        None => source_project_id.to_string(),
    };
    // 来源空镜像行（0 会话的历史镜像残留）：清理。
    for row in &source_rows {
        if row.all_sessions == 0 && row.project_id != survivor_id {
            mirrors_to_delete.push(row.project_id.clone());
        }
    }

    // ===== 待换腿会话集合：来源行的全部正常会话 =====
    let has_hidden_status = columns(&conn, "chat_session")?
        .iter()
        .any(|name| name == "hidden_status");
    let hidden_s = if has_hidden_status {
        " AND COALESCE(s.hidden_status, '') != 'voice_discussion'"
    } else {
        ""
    };
    let sessions_to_switch: Vec<(String, String, String)> = {
        let mut statement = conn
            .prepare(&format!(
                "SELECT s.session_id, s.project_id, p.user_id FROM chat_session s
                 JOIN project p ON p.project_id = s.project_id
                 WHERE p.biz_project_id = ?1 AND p.user_id IS NOT NULL AND p.user_id != ?2
                   AND COALESCE(s.deleted_at, 0) = 0{hidden_s}
                 ORDER BY s.session_id"
            ))
            .map_err(|_| MasterHandoverError::DbOpenFailed)?;
        let rows = statement
            .query_map(rusqlite::params![biz, target_user_id], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })
            .map_err(|_| MasterHandoverError::DbOpenFailed)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| MasterHandoverError::DbOpenFailed)?;
        rows
    };
    let session_ids: Vec<String> = sessions_to_switch
        .iter()
        .map(|(session, _, _)| session.clone())
        .collect();
    let before_digests = text_digests(&conn, &session_ids)?;
    let before_counts = session_counts(&conn, &session_ids)?;

    // 进度 label（来源行项目名；空名回退「未命名项目」）。
    let project_names: HashMap<String, String> = source_rows
        .iter()
        .map(|row| (row.project_id.clone(), row.project_name.clone()))
        .collect();
    let session_labels: Vec<String> = sessions_to_switch
        .iter()
        .map(|(_, project_id, _)| {
            project_names
                .get(project_id)
                .filter(|name| !name.is_empty())
                .cloned()
                .unwrap_or_else(|| "未命名项目".to_string())
        })
        .collect();

    // ===== 换腿身份映射（e5-r2 同款五类身份 + 会话本体）=====
    let total_sessions = sessions_to_switch.len();
    let mut ids = IdGenerator::new();
    let mut leg_maps: Vec<(String, String)> = Vec::new();
    for (index, (session, _, _)) in sessions_to_switch.iter().enumerate() {
        on_progress(HandoverProgress {
            phase: "mapping",
            current: index + 1,
            total: total_sessions,
            label: session_labels[index].clone(),
        });
        leg_maps.push((session.clone(), ids.next()));
    }
    let message_map =
        map_column_batch(&conn, "chat_message", "message_id", &session_ids, &mut ids)?;
    let turn_map = map_column_batch(&conn, "chat_turn", "turn_id", &session_ids, &mut ids)?;
    let task_map = map_column_batch(&conn, "task", "task_id", &session_ids, &mut ids)?;
    let history_map =
        map_column_batch(&conn, "history_v2", "history_v2_id", &session_ids, &mut ids)?;
    let agent_run_map =
        map_column_batch(&conn, "agent_run", "agent_run_id", &session_ids, &mut ids)?;

    // ===== 单事务：镜像清理 → 会话并入 → 行清理 → 归属改写 → 换腿 =====
    let tx = conn
        .unchecked_transaction()
        .map_err(|_| MasterHandoverError::DbOpenFailed)?;
    for mirror in &mirrors_to_delete {
        tx.execute(
            "DELETE FROM session_project WHERE project_id = ?1",
            [mirror],
        )
        .map_err(|_| MasterHandoverError::DbOpenFailed)?;
        tx.execute("DELETE FROM project WHERE project_id = ?1", [mirror])
            .map_err(|_| MasterHandoverError::DbOpenFailed)?;
    }
    // 会话并入承接行：逐来源行改挂（承接行自身的会话不动）。
    let mut merged_away = 0usize;
    for row in &source_rows {
        if row.project_id == survivor_id || row.normal_sessions == 0 {
            continue;
        }
        let merge_sessions: Vec<&str> = sessions_to_switch
            .iter()
            .filter(|(_, project_id, _)| project_id == &row.project_id)
            .map(|(session_id, _, _)| session_id.as_str())
            .collect();
        if merge_sessions.is_empty() {
            continue;
        }
        let clause = placeholders(merge_sessions.len());
        let params = || {
            rusqlite::params_from_iter(
                std::iter::once(survivor_id.as_str())
                    .chain(std::iter::once(row.project_id.as_str()))
                    .chain(merge_sessions.iter().copied()),
            )
        };
        tx.execute(
            &format!(
                "UPDATE chat_session SET project_id = ?1
                 WHERE project_id = ?2 AND session_id IN ({clause})"
            ),
            params(),
        )
        .map_err(|_| MasterHandoverError::DbOpenFailed)?;
        tx.execute(
            &format!(
                "UPDATE session_project SET project_id = ?1
                 WHERE project_id = ?2 AND session_id IN ({clause})"
            ),
            params(),
        )
        .map_err(|_| MasterHandoverError::DbOpenFailed)?;
        // 移空来源行清理；残留归档/软删会话的行保留为冻结内容（原账号名下）。
        let deleted = tx
            .execute(
                "DELETE FROM project WHERE project_id = ?1
                 AND NOT EXISTS (SELECT 1 FROM chat_session WHERE project_id = ?1)
                 AND NOT EXISTS (SELECT 1 FROM session_project WHERE project_id = ?1)",
                [row.project_id.as_str()],
            )
            .map_err(|_| MasterHandoverError::DbOpenFailed)?;
        merged_away += deleted;
    }
    // 承接行归属改写：承接行 = 点击的来源行时改挂到目标账号；UNIQUE 由
    // 目标空镜像清理保证不撞。承接行 = 目标真实行时无需改写。
    let mut survivor_transferred = 0usize;
    if survivor_id == source_project_id {
        survivor_transferred += tx
            .execute(
                "UPDATE project SET user_id = ?1 WHERE project_id = ?2",
                rusqlite::params![target_user_id, survivor_id],
            )
            .map_err(|_| MasterHandoverError::DbOpenFailed)?;
    }

    // temp 映射表（内存态，随连接销毁；ID 明文不落盘）。
    tx.execute_batch(
        "PRAGMA temp_store = MEMORY;
         CREATE TEMP TABLE id_map_session (old_id TEXT PRIMARY KEY, new_id TEXT);
         CREATE TEMP TABLE id_map_message (old_id TEXT PRIMARY KEY, new_id TEXT);
         CREATE TEMP TABLE id_map_turn (old_id TEXT PRIMARY KEY, new_id TEXT);
         CREATE TEMP TABLE id_map_task (old_id TEXT PRIMARY KEY, new_id TEXT);
         CREATE TEMP TABLE id_map_history (old_id TEXT PRIMARY KEY, new_id TEXT);
         CREATE TEMP TABLE id_map_agent_run (old_id TEXT PRIMARY KEY, new_id TEXT);",
    )
    .map_err(|_| MasterHandoverError::DbOpenFailed)?;
    insert_map_entries(&tx, "id_map_session", &leg_maps)?;
    insert_map_entries(&tx, "id_map_message", &message_map)?;
    insert_map_entries(&tx, "id_map_turn", &turn_map)?;
    insert_map_entries(&tx, "id_map_task", &task_map)?;
    insert_map_entries(&tx, "id_map_history", &history_map)?;
    insert_map_entries(&tx, "id_map_agent_run", &agent_run_map)?;

    // 集合重写（executing 阶段按身份组上报）：五类身份引用列 → 会话引用
    // 列 → chat_session 主键；server_history_info 永不改写（§19 铁律）。
    let group_total = IDENTITY_GROUPS.len() + 1;
    for (index, (label, map_table, columns_to_update)) in IDENTITY_GROUPS.iter().enumerate() {
        on_progress(HandoverProgress {
            phase: "executing",
            current: index + 1,
            total: group_total,
            label: (*label).to_string(),
        });
        apply_map_batch(&tx, map_table, columns_to_update, &["server_history_info"])?;
    }
    on_progress(HandoverProgress {
        phase: "executing",
        current: group_total,
        total: group_total,
        label: "会话".to_string(),
    });
    apply_map_batch(
        &tx,
        "id_map_session",
        &[
            "session_id",
            "chat_session_id",
            "creator_session_id",
            "writer_session_id",
        ],
        &["server_history_info", "chat_session"],
    )?;
    tx.execute(
        "UPDATE chat_session SET session_id = \
         (SELECT new_id FROM id_map_session WHERE old_id = chat_session.session_id) \
         WHERE session_id IN (SELECT old_id FROM id_map_session)",
        [],
    )
    .map_err(|_| MasterHandoverError::DbOpenFailed)?;
    commit_handover_transaction(tx)?;

    // ===== 写后完整性校验（正文指纹 + 行数守恒 + 新腿无旧镜像）=====
    let new_session_ids: Vec<String> = leg_maps
        .iter()
        .map(|(_, new_session)| new_session.clone())
        .collect();
    let after_digests = post_commit_verification(text_digests(&conn, &new_session_ids))?;
    let after_counts = post_commit_verification(session_counts(&conn, &new_session_ids))?;
    let after_server_counts =
        post_commit_verification(session_server_counts(&conn, &new_session_ids))?;
    for (index, (old_session, _, _)) in sessions_to_switch.iter().enumerate() {
        let new_session = &leg_maps[index].1;
        on_progress(HandoverProgress {
            phase: "verifying",
            current: index + 1,
            total: total_sessions,
            label: session_labels[index].clone(),
        });
        if after_digests.get(new_session) != before_digests.get(old_session) {
            return Err(MasterHandoverError::CommittedNeedsAttention);
        }
        if after_counts.get(new_session) != before_counts.get(old_session) {
            return Err(MasterHandoverError::CommittedNeedsAttention);
        }
        if after_server_counts.get(new_session).copied().unwrap_or(0) != 0 {
            return Err(MasterHandoverError::CommittedNeedsAttention);
        }
    }

    // 前归属账号：换腿会话最多的来源账号（并列取 user_id 升序，稳定输出）。
    let mut owner_counts: HashMap<String, usize> = HashMap::new();
    for (_, _, user) in &sessions_to_switch {
        *owner_counts.entry(user.clone()).or_insert(0) += 1;
    }
    let previous_owner = owner_counts
        .into_iter()
        .min_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)))
        .map(|(user, _)| user);

    Ok(MasterHandover {
        previous_owner_user_id: previous_owner,
        transferred_projects: merged_away + survivor_transferred,
        removed_mirror_rows: mirrors_to_delete.len(),
        switched_sessions: sessions_to_switch
            .iter()
            .enumerate()
            .map(
                |(index, (source_session, _, previous_user_id))| HandoverSession {
                    session_id: leg_maps[index].1.clone(),
                    previous_session_id: source_session.clone(),
                    // 全部换腿会话的落点都是承接行（并入或原地）。
                    project_id: survivor_id.clone(),
                    previous_user_id: previous_user_id.clone(),
                    message_count: before_counts
                        .get(source_session)
                        .map(|(count, _)| *count)
                        .unwrap_or(0),
                },
            )
            .collect(),
    })
}

/// 交接核心实现；`source_user_id` 为 None 时保持全量随行语义；
/// `merge_conflicts` 为 true 时同 biz 双侧非空走合并而非报错。
///
/// 前置条件：主库 TRAE 实例已关闭（调用方五步编排第 1 步保证）。
fn handover_master_records_core(
    db_path: &Path,
    raw_key: &str,
    target_user_id: &str,
    source_user_id: Option<&str>,
    merge_conflicts: bool,
    on_progress: &dyn Fn(HandoverProgress),
) -> Result<MasterHandover, MasterHandoverError> {
    if !db_path.is_file() {
        return Err(MasterHandoverError::DbUnavailable);
    }
    // 来源过滤（手动选择性交接）：服务端 user_id 恒为数字串，校验通过才允许
    // 拼进 SQL（与 normal_session_filter 同款常量拼接路径，杜绝注入面）。
    let source_filter_p = match source_user_id {
        Some(source) if source.chars().all(|c| c.is_ascii_digit()) => {
            format!(" AND p.user_id = '{source}'")
        }
        Some(_) => return Err(MasterHandoverError::DbOpenFailed),
        None => String::new(),
    };
    let source_filter_plain = match source_user_id {
        Some(source) if source.chars().all(|c| c.is_ascii_digit()) => {
            format!(" AND user_id = '{source}'")
        }
        Some(_) => return Err(MasterHandoverError::DbOpenFailed),
        None => String::new(),
    };
    let conn = Connection::open(db_path).map_err(|_| MasterHandoverError::DbOpenFailed)?;
    // busy_timeout 用原生 API 设置：`PRAGMA busy_timeout = N` 会返回结果行，
    // 与 PRAGMA key 同批 execute_batch 会报 ExecuteReturnedResults。
    conn.busy_timeout(Duration::from_millis(5000))
        .map_err(|_| MasterHandoverError::DbOpenFailed)?;
    conn.execute_batch(&format!("PRAGMA key = \"x'{raw_key}'\";"))
        .map_err(|_| MasterHandoverError::DbOpenFailed)?;

    // 归档只借用 TRAE 的会话级隐藏枚举；老库没有该列时按全部会话兼容。
    let has_hidden_status = columns(&conn, "chat_session")?
        .iter()
        .any(|name| name == "hidden_status");
    let normal_session_filter = if has_hidden_status {
        " AND COALESCE(s.hidden_status, '') != 'voice_discussion'"
    } else {
        ""
    };
    let normal_subquery_filter = if has_hidden_status {
        " AND COALESCE(cs.hidden_status, '') != 'voice_discussion'"
    } else {
        ""
    };

    // ===== 交接前快照：待随行的正常会话集合 =====
    let sessions_to_switch: Vec<(String, String, String)> = {
        let mut statement = conn
            .prepare(&format!(
                    "SELECT s.session_id, s.project_id, p.user_id FROM chat_session s
                     JOIN project p ON p.project_id = s.project_id
                     WHERE p.user_id != ?1{source_filter_p} AND COALESCE(s.deleted_at, 0) = 0{normal_session_filter}"
                ))
            .map_err(|_| MasterHandoverError::DbOpenFailed)?;
        let rows = statement
            .query_map([target_user_id], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })
            .map_err(|_| MasterHandoverError::DbOpenFailed)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| MasterHandoverError::DbOpenFailed)?;
        rows
    };
    // 归档/冻结内容没有正常候选时直接结束：不再扫描项目、建立映射或发进度。
    if sessions_to_switch.is_empty() {
        return Ok(MasterHandover {
            previous_owner_user_id: None,
            transferred_projects: 0,
            removed_mirror_rows: 0,
            switched_sessions: Vec::new(),
        });
    }
    let previous_owner: Option<String> = conn
        .query_row(
            &format!(
                "SELECT p.user_id FROM project p
                 JOIN chat_session s ON s.project_id = p.project_id
                 WHERE p.user_id IS NOT NULL AND p.user_id != ?1{source_filter_p}
                   AND COALESCE(s.deleted_at, 0) = 0{normal_session_filter}
                 GROUP BY p.user_id ORDER BY COUNT(*) DESC LIMIT 1"
            ),
            [target_user_id],
            |row| row.get(0),
        )
        .ok();
    let session_ids: Vec<String> = sessions_to_switch
        .iter()
        .map(|(session, _, _)| session.clone())
        .collect();
    let before_digests = text_digests(&conn, &session_ids)?;
    let before_counts = session_counts(&conn, &session_ids)?;

    // ===== UNIQUE 冲突预处理（E5 坑位：空镜像自动清理 + 非空冲突报人工）=====
    let mut mirrors_to_delete: Vec<String> = Vec::new();
    let mut merge_pairs: Vec<(String, String)> = Vec::new();
    // (biz, 非目标侧挂有会话的行数, biz) 分组统计：同 biz 出现多个非空行 = 冲突。
    let mut nonempty_by_biz: HashMap<String, usize> = HashMap::new();
    {
        let mut statement = conn
            .prepare(&format!(
                // 同 biz 的跨账号行对：p = 非目标行（待随行），t = 目标名下已存在行。
                "SELECT p.biz_project_id, p.project_id, t.project_id,
                        (SELECT COUNT(*) FROM chat_session cs WHERE cs.project_id = p.project_id
                         AND COALESCE(cs.deleted_at, 0) = 0{normal_subquery_filter}),
                        (SELECT COUNT(*) FROM chat_session cs WHERE cs.project_id = t.project_id
                         AND COALESCE(cs.deleted_at, 0) = 0{normal_subquery_filter}),
                        (SELECT COUNT(*) FROM chat_session cs WHERE cs.project_id = p.project_id),
                        (SELECT COUNT(*) FROM chat_session cs WHERE cs.project_id = t.project_id)
                 FROM project p
                 JOIN project t
                   ON t.biz_project_id = p.biz_project_id AND t.user_id = ?1
                 WHERE p.user_id != ?1{source_filter_p} AND p.biz_project_id IS NOT NULL"
            ))
            .map_err(|_| MasterHandoverError::DbOpenFailed)?;
        let rows = statement
            .query_map([target_user_id], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, i64>(5)?,
                    row.get::<_, i64>(6)?,
                ))
            })
            .map_err(|_| MasterHandoverError::DbOpenFailed)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| MasterHandoverError::DbOpenFailed)?;
        for (
            biz,
            source_project,
            target_project,
            source_sessions,
            target_sessions,
            source_all_sessions,
            target_all_sessions,
        ) in rows
        {
            if source_sessions > 0 {
                *nonempty_by_biz.entry(biz.clone()).or_insert(0) += 1;
            }
            // 目标名下空镜像行：自动清理（E5 rmproject 同款断言——0 会话才允许删）。
            if target_all_sessions == 0 {
                mirrors_to_delete.push(target_project);
            } else if source_sessions > 0 {
                if merge_conflicts {
                    // 合并时保留目标项目身份，只把来源账号待交接的正常会话并入。
                    merge_pairs.push((source_project, target_project));
                } else {
                    // 普通切号仍禁止静默合并同名项目。
                    return Err(MasterHandoverError::TargetConflict(biz_fingerprint(&biz)));
                }
            } else if target_sessions > 0 && source_all_sessions == 0 {
                // 目标名下已有同 biz 真实行、来源行只是空镜像：
                // 空镜像同样必须清理，否则随行 UPDATE 在来源行上撞 UNIQUE。
                mirrors_to_delete.push(source_project);
            }
        }
    }
    // 同 biz 多个非目标行都挂有会话：随行后必然互撞，报人工决策。
    for (biz, nonempty) in nonempty_by_biz {
        if nonempty > 1 {
            return Err(MasterHandoverError::TargetConflict(biz_fingerprint(&biz)));
        }
    }
    // 同 biz 的多个非目标空行（历史多账号镜像残留）：只清理 0 会话的行，
    // 否则随行 UPDATE 在非目标行之间互撞 UNIQUE。保留规则：带会话的行
    // 优先（多个带会话行已在上方冲突检测拦截，此处最多一行）；无带会话
    // 行时保留 rowid 最小行。2026-08-31 事故修复：原实现按 rowid 盲删
    // 非首行，把带活跃会话的行删成孤儿，导致切换后会话从 UI 消失。
    {
        let mut statement = conn
            .prepare(&format!(
                "SELECT biz_project_id, project_id,
                        (SELECT COUNT(*) FROM chat_session cs
                         WHERE cs.project_id = project.project_id
                           AND COALESCE(cs.deleted_at, 0) = 0{normal_subquery_filter}),
                        (SELECT COUNT(*) FROM chat_session cs
                         WHERE cs.project_id = project.project_id)
                 FROM project
                 WHERE user_id != ?1{source_filter_plain} AND biz_project_id IS NOT NULL
                 ORDER BY biz_project_id, rowid"
            ))
            .map_err(|_| MasterHandoverError::DbOpenFailed)?;
        let rows = statement
            .query_map([target_user_id], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, i64>(3)?,
                ))
            })
            .map_err(|_| MasterHandoverError::DbOpenFailed)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| MasterHandoverError::DbOpenFailed)?;
        // biz → (当前保留行级别：0 空、1 归档、2 正常, project_id)。
        let mut keep: HashMap<String, (i64, String)> = HashMap::new();
        for (biz, project, normal_sessions, all_sessions) in rows {
            let rank = if normal_sessions > 0 {
                2
            } else if all_sessions > 0 {
                1
            } else {
                0
            };
            match keep.get(&biz) {
                // 归档项目不是空镜像：即使存在同 biz 活跃项目，也不能在
                // 交接清理阶段删除它，只能让显式项目合并处理。
                Some(_) if rank == 0 => {
                    // 后出现的空镜像始终应被清理，避免正常行先出现时留下唯一键冲突。
                    mirrors_to_delete.push(project);
                }
                Some((kept_rank, kept)) if *kept_rank == 0 && rank > 0 => {
                    mirrors_to_delete.push(kept.clone());
                    keep.insert(biz, (rank, project));
                }
                Some((kept_rank, _)) if rank == 2 && *kept_rank == 1 => {
                    // 活跃行承担交接，归档行继续留在原账号下冻结。
                    keep.insert(biz, (rank, project));
                }
                None => {
                    keep.insert(biz, (rank, project));
                }
                _ => {}
            }
        }
    }
    let removed_mirror_rows = mirrors_to_delete.len();

    // ===== 换腿身份映射（事务外读取；e5-r2 探针同款五类身份）=====
    // P7-3：全部会话合并为全局映射——各身份 ID 按会话隔离收集、全局唯一，
    // 合并后集合重写与原「逐会话顺序应用」语义等价。
    let project_names: HashMap<String, String> = {
        let mut statement = conn
            .prepare("SELECT project_id, name FROM project")
            .map_err(|_| MasterHandoverError::DbOpenFailed)?;
        let rows = statement
            .query_map([], |row| {
                // 生产库 project.name 允许为 NULL，空名由进度标签统一回退。
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<String>>(1)?.unwrap_or_default(),
                ))
            })
            .map_err(|_| MasterHandoverError::DbOpenFailed)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| MasterHandoverError::DbOpenFailed)?;
        rows.into_iter().collect()
    };
    // 进度 label（项目名；空名回退「未命名项目」），mapping/verifying 共用。
    let session_labels: Vec<String> = sessions_to_switch
        .iter()
        .map(|(_, project_id, _)| {
            project_names
                .get(project_id)
                .filter(|name| !name.is_empty())
                .cloned()
                .unwrap_or_else(|| "未命名项目".to_string())
        })
        .collect();
    let total_sessions = sessions_to_switch.len();
    let mut ids = IdGenerator::new();
    let mut leg_maps: Vec<(String, String)> = Vec::new(); // (旧 session, 新 session)，与 sessions_to_switch 同序
    for (index, (session, _, _)) in sessions_to_switch.iter().enumerate() {
        on_progress(HandoverProgress {
            phase: "mapping",
            current: index + 1,
            total: total_sessions,
            label: session_labels[index].clone(),
        });
        leg_maps.push((session.clone(), ids.next()));
    }
    // 每张身份表只读取一次集合，避免“会话数 × 表数”的往返查询。
    let message_map =
        map_column_batch(&conn, "chat_message", "message_id", &session_ids, &mut ids)?;
    let turn_map = map_column_batch(&conn, "chat_turn", "turn_id", &session_ids, &mut ids)?;
    let task_map = map_column_batch(&conn, "task", "task_id", &session_ids, &mut ids)?;
    let history_map =
        map_column_batch(&conn, "history_v2", "history_v2_id", &session_ids, &mut ids)?;
    let agent_run_map =
        map_column_batch(&conn, "agent_run", "agent_run_id", &session_ids, &mut ids)?;

    // ===== 单事务执行：镜像清理 → 项目合并 → 归属随行 → 集合式批量换腿 =====
    let tx = conn
        .unchecked_transaction()
        .map_err(|_| MasterHandoverError::DbOpenFailed)?;
    for project in &mirrors_to_delete {
        tx.execute(
            "DELETE FROM session_project WHERE project_id = ?1",
            [project],
        )
        .map_err(|_| MasterHandoverError::DbOpenFailed)?;
        tx.execute("DELETE FROM project WHERE project_id = ?1", [project])
            .map_err(|_| MasterHandoverError::DbOpenFailed)?;
    }
    let mut merged_projects = 0;
    for (source_project, target_project) in &merge_pairs {
        let merge_sessions: Vec<&str> = sessions_to_switch
            .iter()
            .filter(|(_, project_id, _)| project_id == source_project)
            .map(|(session_id, _, _)| session_id.as_str())
            .collect();
        let placeholders = placeholders(merge_sessions.len());
        let params = || {
            rusqlite::params_from_iter(
                std::iter::once(target_project.as_str())
                    .chain(std::iter::once(source_project.as_str()))
                    .chain(merge_sessions.iter().copied()),
            )
        };
        let moved_sessions = tx
            .execute(
                &format!(
                    "UPDATE chat_session SET project_id = ?1
                     WHERE project_id = ?2 AND session_id IN ({placeholders})"
                ),
                params(),
            )
            .map_err(|_| MasterHandoverError::DbOpenFailed)?;
        if moved_sessions > 0 {
            tx.execute(
                &format!(
                    "UPDATE session_project SET project_id = ?1
                     WHERE project_id = ?2 AND session_id IN ({placeholders})"
                ),
                params(),
            )
            .map_err(|_| MasterHandoverError::DbOpenFailed)?;
            merged_projects += 1;
        }
        // 归档、软删除或其他残留会话仍留在来源项目；仅移空的项目行。
        tx.execute(
            "DELETE FROM project WHERE project_id = ?1
             AND NOT EXISTS (SELECT 1 FROM chat_session WHERE project_id = ?1)
             AND NOT EXISTS (SELECT 1 FROM session_project WHERE project_id = ?1)",
            [source_project],
        )
        .map_err(|_| MasterHandoverError::DbOpenFailed)?;
    }
    let transferred_projects = tx
        .execute(
            &format!(
                "UPDATE project SET user_id = ?1 WHERE user_id != ?1{source_filter_plain}
                 AND EXISTS (
                     SELECT 1 FROM chat_session s
                     WHERE s.project_id = project.project_id
                       AND COALESCE(s.deleted_at, 0) = 0{normal_session_filter}
                 )"
            ),
            [target_user_id],
        )
        .map_err(|_| MasterHandoverError::DbOpenFailed)? as usize
        + merged_projects;

    // temp 映射表（P7-3）：内存态（temp_store = MEMORY，ID 明文不落盘），
    // 随连接销毁；主库文件结构不变。
    tx.execute_batch(
        "PRAGMA temp_store = MEMORY;
         CREATE TEMP TABLE id_map_session (old_id TEXT PRIMARY KEY, new_id TEXT);
         CREATE TEMP TABLE id_map_message (old_id TEXT PRIMARY KEY, new_id TEXT);
         CREATE TEMP TABLE id_map_turn (old_id TEXT PRIMARY KEY, new_id TEXT);
         CREATE TEMP TABLE id_map_task (old_id TEXT PRIMARY KEY, new_id TEXT);
         CREATE TEMP TABLE id_map_history (old_id TEXT PRIMARY KEY, new_id TEXT);
         CREATE TEMP TABLE id_map_agent_run (old_id TEXT PRIMARY KEY, new_id TEXT);",
    )
    .map_err(|_| MasterHandoverError::DbOpenFailed)?;
    insert_map_entries(&tx, "id_map_session", &leg_maps)?;
    insert_map_entries(&tx, "id_map_message", &message_map)?;
    insert_map_entries(&tx, "id_map_turn", &turn_map)?;
    insert_map_entries(&tx, "id_map_task", &task_map)?;
    insert_map_entries(&tx, "id_map_history", &history_map)?;
    insert_map_entries(&tx, "id_map_agent_run", &agent_run_map)?;

    // 集合重写（进度：executing 阶段按身份组上报）。
    // 重写顺序与逐会话版一致：五类身份引用列 → 会话引用列 → chat_session
    // 主键；server_history_info 永不改写（云端镜像证据，§19 铁律）。
    let group_total = IDENTITY_GROUPS.len() + 1; // 五类身份 + 会话本体
    for (index, (label, map_table, columns_to_update)) in IDENTITY_GROUPS.iter().enumerate() {
        on_progress(HandoverProgress {
            phase: "executing",
            current: index + 1,
            total: group_total,
            label: (*label).to_string(),
        });
        apply_map_batch(&tx, map_table, columns_to_update, &["server_history_info"])?;
    }
    on_progress(HandoverProgress {
        phase: "executing",
        current: group_total,
        total: group_total,
        label: "会话".to_string(),
    });
    apply_map_batch(
        &tx,
        "id_map_session",
        &[
            "session_id",
            "chat_session_id",
            "creator_session_id",
            "writer_session_id",
        ],
        &["server_history_info", "chat_session"],
    )?;
    tx.execute(
        "UPDATE chat_session SET session_id = \
         (SELECT new_id FROM id_map_session WHERE old_id = chat_session.session_id) \
         WHERE session_id IN (SELECT old_id FROM id_map_session)",
        [],
    )
    .map_err(|_| MasterHandoverError::DbOpenFailed)?;

    let switched_sessions: Vec<HandoverSession> = sessions_to_switch
        .iter()
        .enumerate()
        .map(
            |(index, (source_session, project_id, previous_user_id))| HandoverSession {
                session_id: leg_maps[index].1.clone(),
                previous_session_id: source_session.clone(),
                project_id: merge_pairs
                    .iter()
                    .find(|(source_project, _)| source_project == project_id)
                    .map(|(_, target_project)| target_project.clone())
                    .unwrap_or_else(|| project_id.clone()),
                previous_user_id: previous_user_id.clone(),
                message_count: before_counts
                    .get(source_session)
                    .map(|(count, _)| *count)
                    .unwrap_or(0),
            },
        )
        .collect();
    commit_handover_transaction(tx)?;

    // ===== 写后完整性校验（集合读取：正文指纹 + 行数守恒）=====
    let new_session_ids: Vec<String> = leg_maps
        .iter()
        .map(|(_, new_session)| new_session.clone())
        .collect();
    let after_digests = post_commit_verification(text_digests(&conn, &new_session_ids))?;
    let after_counts = post_commit_verification(session_counts(&conn, &new_session_ids))?;
    let after_server_counts =
        post_commit_verification(session_server_counts(&conn, &new_session_ids))?;
    for (index, (old_session, _, _)) in sessions_to_switch.iter().enumerate() {
        let new_session = &leg_maps[index].1;
        on_progress(HandoverProgress {
            phase: "verifying",
            current: index + 1,
            total: total_sessions,
            label: session_labels[index].clone(),
        });
        if after_digests.get(new_session) != before_digests.get(old_session) {
            return Err(MasterHandoverError::CommittedNeedsAttention);
        }
        if after_counts.get(new_session) != before_counts.get(old_session) {
            return Err(MasterHandoverError::CommittedNeedsAttention);
        }
        // 新腿不得携带旧云端镜像（e5-r2 验收标准 2）。
        if after_server_counts.get(new_session).copied().unwrap_or(0) != 0 {
            return Err(MasterHandoverError::CommittedNeedsAttention);
        }
        let _ = old_session;
    }

    Ok(MasterHandover {
        previous_owner_user_id: previous_owner,
        transferred_projects,
        removed_mirror_rows,
        switched_sessions,
    })
}

/// 按会话集合读取正文指纹，保持单会话校验的字段与顺序不变。
///
/// 旧实现对每个会话执行 4 组查询；这里每张正文表只查询一次，归档会话
/// 不在传入集合中，因此不会产生额外的正文扫描或校验成本。
fn text_digests(
    conn: &Connection,
    sessions: &[String],
) -> Result<HashMap<String, String>, MasterHandoverError> {
    use std::hash::Hasher;
    if sessions.is_empty() {
        return Ok(HashMap::new());
    }
    let mut hashers: HashMap<String, std::collections::hash_map::DefaultHasher> = sessions
        .iter()
        .map(|session| {
            (
                session.clone(),
                std::collections::hash_map::DefaultHasher::new(),
            )
        })
        .collect();
    let clause = placeholders(sessions.len());
    let queries = [
        format!(
            "SELECT session_id, message_role, message_index FROM chat_message \
             WHERE session_id IN ({clause}) ORDER BY session_id, message_index, id"
        ),
        format!(
            "SELECT cm.session_id, g.content, g.created_at \
             FROM chat_message cm JOIN chat_message_general g ON g.message_id = cm.message_id \
             WHERE cm.session_id IN ({clause}) ORDER BY cm.session_id, g.id"
        ),
        format!(
            "SELECT cm.session_id, t.content, t.created_at \
             FROM chat_message cm JOIN chat_message_task t ON t.message_id = cm.message_id \
             WHERE cm.session_id IN ({clause}) ORDER BY cm.session_id, t.id"
        ),
        format!(
            "SELECT session_id, messages, created_at FROM history_v2 \
             WHERE session_id IN ({clause}) ORDER BY session_id, id"
        ),
    ];
    for sql in queries {
        let mut statement = conn
            .prepare(&sql)
            .map_err(|_| MasterHandoverError::DbOpenFailed)?;
        let mut rows = statement
            .query(rusqlite::params_from_iter(
                sessions.iter().map(String::as_str),
            ))
            .map_err(|_| MasterHandoverError::DbOpenFailed)?;
        while let Some(row) = rows.next().map_err(|_| MasterHandoverError::DbOpenFailed)? {
            let session: String = row.get(0).map_err(|_| MasterHandoverError::DbOpenFailed)?;
            let Some(hasher) = hashers.get_mut(&session) else {
                continue;
            };
            let first: String = row.get(1).unwrap_or_default();
            let second: String = row.get(2).unwrap_or_default();
            hasher.write(first.as_bytes());
            hasher.write_u8(0);
            hasher.write(second.as_bytes());
            hasher.write_u8(0xff);
        }
    }
    Ok(hashers
        .into_iter()
        .map(|(session, hasher)| (session, format!("{:016x}", hasher.finish())))
        .collect())
}

/// 批量读取会话行数，返回 session_id →（消息数、history_v2 数）。
fn session_counts(
    conn: &Connection,
    sessions: &[String],
) -> Result<HashMap<String, (i64, i64)>, MasterHandoverError> {
    if sessions.is_empty() {
        return Ok(HashMap::new());
    }
    let clause = placeholders(sessions.len());
    let mut result: HashMap<String, (i64, i64)> = sessions
        .iter()
        .map(|session| (session.clone(), (0, 0)))
        .collect();
    for (table, index) in [("chat_message", 0_usize), ("history_v2", 1_usize)] {
        let sql = format!(
            "SELECT session_id, COUNT(*) FROM {table} WHERE session_id IN ({clause}) GROUP BY session_id"
        );
        let mut statement = conn
            .prepare(&sql)
            .map_err(|_| MasterHandoverError::DbOpenFailed)?;
        let rows = statement
            .query_map(
                rusqlite::params_from_iter(sessions.iter().map(String::as_str)),
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
            )
            .map_err(|_| MasterHandoverError::DbOpenFailed)?;
        for row in rows {
            let (session, count) = row.map_err(|_| MasterHandoverError::DbOpenFailed)?;
            if let Some(entry) = result.get_mut(&session) {
                entry.0 = if index == 0 { count } else { entry.0 };
                entry.1 = if index == 1 { count } else { entry.1 };
            }
        }
    }
    Ok(result)
}

/// 批量读取云端镜像行数，用于确认新腿没有携带旧镜像。
fn session_server_counts(
    conn: &Connection,
    sessions: &[String],
) -> Result<HashMap<String, i64>, MasterHandoverError> {
    if sessions.is_empty() {
        return Ok(HashMap::new());
    }
    let clause = placeholders(sessions.len());
    let sql = format!(
        "SELECT session_id, COUNT(*) FROM server_history_info \
         WHERE session_id IN ({clause}) GROUP BY session_id"
    );
    let mut result = HashMap::new();
    let mut statement = conn
        .prepare(&sql)
        .map_err(|_| MasterHandoverError::DbOpenFailed)?;
    let rows = statement
        .query_map(
            rusqlite::params_from_iter(sessions.iter().map(String::as_str)),
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
        )
        .map_err(|_| MasterHandoverError::DbOpenFailed)?;
    for row in rows {
        let (session, count) = row.map_err(|_| MasterHandoverError::DbOpenFailed)?;
        result.insert(session, count);
    }
    Ok(result)
}

/// 收集会话集合在某表的身份列（如 chat_message.message_id）→ 新 ID 映射。
/// 表不含该身份列或不含 session_id 列时返回空映射（行不可达，无需重写）。
fn map_column_batch(
    conn: &Connection,
    table: &str,
    column: &str,
    sessions: &[String],
    ids: &mut IdGenerator,
) -> Result<Vec<(String, String)>, MasterHandoverError> {
    if sessions.is_empty() {
        return Ok(Vec::new());
    }
    let names = columns(conn, table)?;
    if !names.iter().any(|name| name == column) || !names.iter().any(|name| name == "session_id") {
        return Ok(Vec::new());
    }
    let clause = placeholders(sessions.len());
    let sql = format!("SELECT \"{column}\" FROM \"{table}\" WHERE session_id IN ({clause})");
    let mut statement = conn
        .prepare(&sql)
        .map_err(|_| MasterHandoverError::DbOpenFailed)?;
    let mut rows = statement
        .query(rusqlite::params_from_iter(
            sessions.iter().map(String::as_str),
        ))
        .map_err(|_| MasterHandoverError::DbOpenFailed)?;
    let mut result = HashMap::new();
    while let Some(row) = rows.next().map_err(|_| MasterHandoverError::DbOpenFailed)? {
        let old: Option<String> = row.get(0).map_err(|_| MasterHandoverError::DbOpenFailed)?;
        if let Some(old) = old.filter(|value| !value.is_empty()) {
            result.entry(old).or_insert_with(|| ids.next());
        }
    }
    Ok(result.into_iter().collect())
}

/// 批量插入映射条目进 temp 表（P7-3）。
fn insert_map_entries(
    tx: &Connection,
    map_table: &str,
    entries: &[(String, String)],
) -> Result<(), MasterHandoverError> {
    if entries.is_empty() {
        return Ok(());
    }
    let mut statement = tx
        .prepare(&format!(
            "INSERT INTO {map_table} (old_id, new_id) VALUES (?1, ?2)"
        ))
        .map_err(|_| MasterHandoverError::DbOpenFailed)?;
    for (old, new) in entries {
        statement
            .execute(rusqlite::params![old, new])
            .map_err(|_| MasterHandoverError::DbOpenFailed)?;
    }
    Ok(())
}

/// 集合式身份重写（P7-3）：每个 (表 × 引用列) 一条 UPDATE——
/// `SET col = (SELECT new_id FROM map WHERE old_id = col)`，替代逐 ID
/// UPDATE（引用列无索引时逐 ID 全表扫是转移慢的根因）。
/// skip_tables 中的表永不改写（server_history_info 证据保留；
/// chat_session 主键由调用方单独换）。
fn apply_map_batch(
    tx: &Connection,
    map_table: &str,
    columns_to_update: &[&str],
    skip_tables: &[&str],
) -> Result<(), MasterHandoverError> {
    for table in table_names(tx)? {
        if skip_tables.contains(&table.as_str()) {
            continue;
        }
        let names = columns(tx, &table)?;
        for column in columns_to_update {
            if !names.iter().any(|name| name == column) {
                continue;
            }
            let sql = format!(
                "UPDATE \"{table}\" SET \"{column}\" = \
                 (SELECT new_id FROM {map_table} WHERE old_id = \"{table}\".\"{column}\") \
                 WHERE \"{column}\" IN (SELECT old_id FROM {map_table})"
            );
            tx.execute(&sql, [])
                .map_err(|_| MasterHandoverError::DbOpenFailed)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::OpenFlags;
    use std::cell::RefCell;

    /// S2 收敛：db_name 提取原本在 lib.rs 三处同形复制，收敛为本 helper 后
    /// 正常路径取文件名，无文件名路径（如根目录）沿用既有稳定错误码
    /// `environment_registry_invalid` fail-closed（零行为变化）。
    #[test]
    fn master_backup_db_name_extracts_file_name_or_fails_closed() {
        let db_path = master_database_path(Path::new("C:/fixture/master"));
        assert_eq!(master_backup_db_name(&db_path).unwrap(), "database.db");

        assert_eq!(
            master_backup_db_name(Path::new("/")).unwrap_err(),
            "environment_registry_invalid"
        );
    }

    /// 构造加密 fixture 库（表结构覆盖换腿涉及的九张表 + 空镜像冲突场景）。
    fn create_fixture_db(path: &Path, raw_key: &str) {
        let flags = OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE;
        let conn = Connection::open_with_flags(path, flags).unwrap();
        conn.execute_batch(&format!("PRAGMA key = \"x'{raw_key}'\";"))
            .unwrap();
        conn.execute_batch(
            "CREATE TABLE project (
                project_id TEXT PRIMARY KEY, user_id TEXT,
                biz_project_id TEXT, name TEXT, deleted_at INTEGER);
             CREATE TABLE chat_session (
                session_id TEXT PRIMARY KEY, project_id TEXT,
                session_title TEXT, updated_at INTEGER, deleted_at INTEGER);
             CREATE TABLE chat_message (
                id INTEGER PRIMARY KEY AUTOINCREMENT, message_id TEXT UNIQUE, session_id TEXT,
                message_role TEXT, message_index INTEGER,
                reply_to_message_id TEXT, response_message_id TEXT, deleted_at INTEGER);
             CREATE TABLE chat_message_general (
                id INTEGER PRIMARY KEY AUTOINCREMENT, message_id TEXT UNIQUE, content TEXT, created_at TEXT);
             CREATE TABLE chat_message_task (
                id INTEGER PRIMARY KEY AUTOINCREMENT, message_id TEXT UNIQUE, content TEXT, created_at TEXT);
             CREATE TABLE chat_turn (
                turn_id TEXT PRIMARY KEY, session_id TEXT, deleted_at INTEGER);
             CREATE TABLE task (
                task_id TEXT PRIMARY KEY, session_id TEXT, deleted_at INTEGER);
             CREATE TABLE history_v2 (
                id INTEGER PRIMARY KEY AUTOINCREMENT, history_v2_id TEXT UNIQUE, session_id TEXT,
                messages TEXT, created_at TEXT);
             CREATE TABLE agent_run (
                agent_run_id TEXT PRIMARY KEY, session_id TEXT);
             CREATE TABLE session_project (
                session_id TEXT PRIMARY KEY, project_id TEXT);
             CREATE TABLE server_history_info (
                id INTEGER PRIMARY KEY, session_id TEXT, conversation_id TEXT,
                source TEXT, user_id TEXT);
             -- 账号 A 的项目与会话（待交接）
             INSERT INTO project VALUES ('p1', '111', 'biz-a', '项目A', NULL);
             INSERT INTO chat_session VALUES ('s1', 'p1', '会话A', 1770000000, NULL);
             INSERT INTO chat_message
                (message_id, session_id, message_role, message_index, reply_to_message_id, response_message_id, deleted_at)
                VALUES ('m1', 's1', 'user', 0, NULL, NULL, NULL);
             INSERT INTO chat_message
                (message_id, session_id, message_role, message_index, reply_to_message_id, response_message_id, deleted_at)
                VALUES ('m2', 's1', 'assistant', 1, 'm1', 'm1', NULL);
             INSERT INTO chat_message_general (message_id, content, created_at)
                VALUES ('m1', '你好', '2026-08-30');
             INSERT INTO chat_message_task (message_id, content, created_at)
                VALUES ('m2', '回复正文', '2026-08-30');
             INSERT INTO chat_turn VALUES ('t1', 's1', NULL);
             INSERT INTO task VALUES ('k1', 's1', NULL);
             INSERT INTO history_v2 (history_v2_id, session_id, messages, created_at)
                VALUES ('h1', 's1', '[]', '2026-08-30');
             INSERT INTO agent_run VALUES ('ar1', 's1');
             INSERT INTO session_project VALUES ('s1', 'p1');
             INSERT INTO server_history_info
                (session_id, conversation_id, source, user_id)
                VALUES ('s1', 'conv-old', 'llm_default', NULL);",
        )
        .unwrap();
    }

    fn temp_db_path(label: &str) -> PathBuf {
        let temp = std::env::temp_dir().join(format!(
            "trae-sync-p51-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&temp).unwrap();
        temp.join("database.db")
    }

    fn open_fixture(path: &Path, raw_key: &str) -> Connection {
        let conn = Connection::open(path).unwrap();
        conn.execute_batch(&format!("PRAGMA key = \"x'{raw_key}'\";"))
            .unwrap();
        conn
    }

    #[test]
    fn post_commit_readback_error_is_not_reported_as_precommit_failure() {
        let result: Result<(), MasterHandoverError> = Err(MasterHandoverError::DbOpenFailed);
        assert_eq!(
            post_commit_verification(result),
            Err(MasterHandoverError::CommittedNeedsAttention)
        );
    }

    /// 用延迟外键制造真实 COMMIT 错误，并确认事务释放后写入没有残留。
    #[test]
    fn failed_handover_commit_is_unknown_and_rolls_back_fixture_transaction() {
        let conn = Connection::open_in_memory().unwrap();
        let raw_key = "aa".repeat(32);
        conn.execute_batch(&format!(
            "PRAGMA key = \"x'{raw_key}'\";
             PRAGMA foreign_keys = ON;
             CREATE TABLE parent (id INTEGER PRIMARY KEY);
             CREATE TABLE child (
                parent_id INTEGER REFERENCES parent(id) DEFERRABLE INITIALLY DEFERRED
             );",
        ))
        .unwrap();
        let foreign_keys_enabled: i64 = conn
            .query_row("PRAGMA foreign_keys", [], |row| row.get(0))
            .unwrap();
        assert_eq!(foreign_keys_enabled, 1);

        let tx = conn.unchecked_transaction().unwrap();
        tx.execute("INSERT INTO child VALUES (1)", []).unwrap();
        assert_eq!(
            commit_handover_transaction(tx),
            Err(MasterHandoverError::CommitOutcomeUnknown)
        );
        let child_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM child", [], |row| row.get(0))
            .unwrap();
        assert_eq!(child_count, 0);
    }

    #[test]
    fn committed_handover_integrity_mismatch_keeps_committed_database_state() {
        let db_path = temp_db_path("handover-post-commit-mismatch");
        let raw_key = "ae".repeat(32);
        create_fixture_db(&db_path, &raw_key);
        {
            let conn = open_fixture(&db_path, &raw_key);
            // 合成提交后不一致：随账号改归属时篡改正文，事务仍会整体提交。
            conn.execute_batch(
                "CREATE TRIGGER mutate_transferred_message
                 AFTER UPDATE OF user_id ON project
                 WHEN NEW.user_id = '222'
                 BEGIN
                   UPDATE chat_message_general SET content = '合成篡改正文'
                   WHERE message_id = 'm1';
                 END;",
            )
            .unwrap();
        }

        assert_eq!(
            handover_master_records(&db_path, &raw_key, "222"),
            Err(MasterHandoverError::CommittedNeedsAttention)
        );

        let conn = open_fixture(&db_path, &raw_key);
        let owner: String = conn
            .query_row(
                "SELECT user_id FROM project WHERE project_id = 'p1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(owner, "222", "数据库已提交，不得描述为已回滚");
        let old_session_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM chat_session WHERE session_id = 's1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(old_session_count, 0, "换腿事务本身仍已提交");
        let message_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM chat_message_general WHERE content = '合成篡改正文'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(message_count, 1, "合成数据异常随事务提交并被读回发现");
        let _ = std::fs::remove_dir_all(db_path.parent().unwrap());
    }

    #[test]
    fn handover_transfers_ownership_and_rewrites_leg() {
        let db_path = temp_db_path("handover");
        let raw_key = "aa".repeat(32);
        create_fixture_db(&db_path, &raw_key);

        let outcome = handover_master_records(&db_path, &raw_key, "222").unwrap();
        // 归属随行：p1 → 账号 222。
        assert_eq!(outcome.transferred_projects, 1);
        assert_eq!(outcome.previous_owner_user_id.as_deref(), Some("111"));
        assert_eq!(outcome.removed_mirror_rows, 0);
        assert_eq!(outcome.switched_sessions.len(), 1);

        let conn = open_fixture(&db_path, &raw_key);
        let owner: String = conn
            .query_row(
                "SELECT user_id FROM project WHERE project_id = 'p1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(owner, "222");

        // 新腿身份全部重写；旧 ID 不复存在。
        let new_session = &outcome.switched_sessions[0].session_id;
        assert_ne!(new_session, "s1");
        assert_eq!(outcome.switched_sessions[0].project_id, "p1");
        assert_eq!(outcome.switched_sessions[0].message_count, 2);
        let old_session_rows: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM chat_session WHERE session_id = 's1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(old_session_rows, 0);
        // 消息行跟随新腿且引用重写（m2 的 reply_to 指向 m1 的新 ID）。
        let messages: Vec<(String, Option<String>)> = conn
            .prepare("SELECT message_id, reply_to_message_id FROM chat_message WHERE session_id = ?1 ORDER BY message_index")
            .unwrap()
            .query_map([new_session], |row| Ok((row.get(0)?, row.get(1)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(messages.len(), 2);
        assert_ne!(messages[0].0, "m1");
        assert_eq!(messages[1].1.as_deref(), Some(messages[0].0.as_str()));
        // turn/task/history/agent_run/session_project 全部跟随新腿。
        for (table, column) in [
            ("chat_turn", "turn_id"),
            ("task", "task_id"),
            ("history_v2", "history_v2_id"),
            ("agent_run", "agent_run_id"),
        ] {
            let count: i64 = conn
                .query_row(
                    &format!(
                        "SELECT COUNT(*) FROM \"{table}\" WHERE session_id = ?1 AND \"{column}\" != 'PLACEHOLDER'"
                    ),
                    [new_session],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(count, 1, "{table} 应有 1 行跟随新腿");
            let old_count: i64 = conn
                .query_row(
                    &format!("SELECT COUNT(*) FROM \"{table}\" WHERE session_id = 's1'"),
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(old_count, 0, "{table} 旧 session_id 应清零");
        }
        let mapping: (String, String) = conn
            .query_row(
                "SELECT session_id, project_id FROM session_project",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(mapping.0, new_session.as_str());
        assert_eq!(mapping.1, "p1");
        // 云端镜像证据保留：旧行原样存在，永不改写。
        let mirror: (String, String) = conn
            .query_row(
                "SELECT session_id, conversation_id FROM server_history_info",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(mirror, ("s1".to_string(), "conv-old".to_string()));
        // 正文内容原样保留（换腿不改写内容）。
        let content: String = conn
            .query_row(
                "SELECT content FROM chat_message_general WHERE message_id = ?1",
                [&messages[0].0],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(content, "你好");

        let _ = std::fs::remove_dir_all(db_path.parent().unwrap());
    }

    #[test]
    fn handover_excludes_archived_sessions_and_frozen_projects() {
        let db_path = temp_db_path("archive-boundary");
        let raw_key = "ac".repeat(32);
        create_fixture_db(&db_path, &raw_key);
        {
            let conn = open_fixture(&db_path, &raw_key);
            conn.execute("ALTER TABLE chat_session ADD COLUMN hidden_status TEXT", [])
                .unwrap();
            // p1 是混合项目：活跃 s1 应交接，归档 s2 保留原身份。
            conn.execute(
                "INSERT INTO chat_session VALUES ('s2', 'p1', '归档会话', 1770000001, NULL, 'voice_discussion')",
                [],
            )
            .unwrap();
            // p2 只有归档会话：项目归属也必须保持不变。
            conn.execute(
                "INSERT INTO project VALUES ('p2', '333', 'biz-frozen', '冻结项目', NULL)",
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO chat_session VALUES ('s3', 'p2', '冻结会话', 1770000002, NULL, 'voice_discussion')",
                [],
            )
            .unwrap();
        }

        let outcome = handover_master_records(&db_path, &raw_key, "222").unwrap();
        assert_eq!(outcome.switched_sessions.len(), 1, "归档会话不得进入交接");
        assert_eq!(outcome.switched_sessions[0].previous_session_id, "s1");

        let conn = open_fixture(&db_path, &raw_key);
        let archived_session: (String, String) = conn
            .query_row(
                "SELECT session_id, hidden_status FROM chat_session WHERE session_id = 's2'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(
            archived_session,
            ("s2".to_string(), "voice_discussion".to_string())
        );
        let frozen_owner: String = conn
            .query_row(
                "SELECT user_id FROM project WHERE project_id = 'p2'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(frozen_owner, "333", "全归档项目不应随账号切换改归属");
        let _ = std::fs::remove_dir_all(db_path.parent().unwrap());
    }

    #[test]
    fn handover_tolerates_null_project_name() {
        let db_path = temp_db_path("null-project-name");
        let raw_key = "ab".repeat(32);
        create_fixture_db(&db_path, &raw_key);
        {
            let conn = open_fixture(&db_path, &raw_key);
            // 生产库允许项目名称为空；该项目带会话，确保会进入进度映射。
            conn.execute(
                "INSERT INTO project VALUES ('p-null-name', '333', 'biz-null-name', NULL, NULL)",
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO chat_session VALUES ('s-null-name', 'p-null-name', '会话', 1, NULL)",
                [],
            )
            .unwrap();
        }

        let labels = RefCell::new(Vec::new());
        let outcome =
            handover_master_records_with_progress(&db_path, &raw_key, "222", &|progress| {
                labels.borrow_mut().push(progress.label)
            })
            .unwrap();

        assert_eq!(outcome.switched_sessions.len(), 2);
        assert!(labels.borrow().iter().any(|label| label == "未命名项目"));
        let _ = std::fs::remove_dir_all(db_path.parent().unwrap());
    }

    #[test]
    fn handover_removes_target_empty_mirror_and_conflicts_on_nonempty() {
        let raw_key = "bb".repeat(32);
        // 场景一：目标名下同 biz 空镜像行 → 自动清理后随行成功。
        let db_path = temp_db_path("mirror");
        create_fixture_db(&db_path, &raw_key);
        {
            let conn = open_fixture(&db_path, &raw_key);
            conn.execute(
                "INSERT INTO project VALUES ('p-mirror', '222', 'biz-a', '镜像', NULL)",
                [],
            )
            .unwrap();
        }
        let outcome = handover_master_records(&db_path, &raw_key, "222").unwrap();
        assert_eq!(outcome.removed_mirror_rows, 1);
        assert_eq!(outcome.transferred_projects, 1);
        let conn = open_fixture(&db_path, &raw_key);
        let mirror_rows: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM project WHERE project_id = 'p-mirror'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(mirror_rows, 0);
        let _ = std::fs::remove_dir_all(db_path.parent().unwrap());

        // 场景二：目标名下同 biz 行挂有会话 → 非空冲突报人工决策，库未改。
        let db_path = temp_db_path("conflict");
        create_fixture_db(&db_path, &raw_key);
        {
            let conn = open_fixture(&db_path, &raw_key);
            conn.execute(
                "INSERT INTO project VALUES ('p-target', '222', 'biz-a', '目标项目', NULL)",
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO chat_session VALUES ('s-target', 'p-target', '目标会话', 1, NULL)",
                [],
            )
            .unwrap();
        }
        let error = handover_master_records(&db_path, &raw_key, "222").unwrap_err();
        assert!(matches!(error, MasterHandoverError::TargetConflict(_)));
        // 冲突时库保持原状（事务未执行任何改写）。
        let conn = open_fixture(&db_path, &raw_key);
        let owner: String = conn
            .query_row(
                "SELECT user_id FROM project WHERE project_id = 'p1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(owner, "111");
        let target_rows: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM project WHERE project_id = 'p-target'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(target_rows, 1);
        let _ = std::fs::remove_dir_all(db_path.parent().unwrap());
    }

    #[test]
    fn handover_merges_selected_source_project_into_target() {
        let db_path = temp_db_path("merge-conflict");
        let raw_key = "cd".repeat(32);
        create_fixture_db(&db_path, &raw_key);
        {
            let conn = open_fixture(&db_path, &raw_key);
            conn.execute(
                "INSERT INTO project VALUES ('p-target', '222', 'biz-a', '目标项目', NULL)",
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO chat_session VALUES ('s-target', 'p-target', '当前活跃对话', 2, NULL)",
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO chat_message
                 (message_id, session_id, message_role, message_index, deleted_at)
                 VALUES ('m-target', 's-target', 'user', 0, NULL)",
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO session_project VALUES ('s-target', 'p-target')",
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO project VALUES ('p-older', '333', 'biz-older', '更早账号项目', NULL)",
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO chat_session VALUES ('s-older', 'p-older', '更早账号对话', 3, NULL)",
                [],
            )
            .unwrap();
        }

        let outcome = handover_master_records_from_merge_with_progress(
            &db_path,
            &raw_key,
            "222",
            "111",
            &|_| {},
        )
        .unwrap();

        assert_eq!(outcome.previous_owner_user_id.as_deref(), Some("111"));
        assert_eq!(outcome.transferred_projects, 1);
        assert_eq!(outcome.switched_sessions.len(), 1);
        let moved = &outcome.switched_sessions[0];
        assert_eq!(moved.previous_session_id, "s1");
        assert_eq!(moved.project_id, "p-target");
        assert_eq!(moved.message_count, 2);

        let conn = open_fixture(&db_path, &raw_key);
        let moved_project: String = conn
            .query_row(
                "SELECT project_id FROM chat_session WHERE session_id = ?1",
                [&moved.session_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(moved_project, "p-target");
        let moved_pivot: String = conn
            .query_row(
                "SELECT project_id FROM session_project WHERE session_id = ?1",
                [&moved.session_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(moved_pivot, "p-target");
        let source_rows: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM project WHERE project_id = 'p1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(source_rows, 0);
        let target_session: (String, String, String) = conn
            .query_row(
                "SELECT session_id, project_id, session_title FROM chat_session WHERE session_id = 's-target'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(
            target_session,
            (
                "s-target".to_string(),
                "p-target".to_string(),
                "当前活跃对话".to_string()
            )
        );
        let target_message_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM chat_message WHERE session_id = 's-target' AND message_id = 'm-target'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(target_message_count, 1);
        let older_owner: (String, String) = conn
            .query_row(
                "SELECT s.project_id, p.user_id FROM chat_session s
                 JOIN project p ON p.project_id = s.project_id WHERE s.session_id = 's-older'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(older_owner, ("p-older".to_string(), "333".to_string()));
        let _ = std::fs::remove_dir_all(db_path.parent().unwrap());
    }

    #[test]
    fn handover_merge_leaves_archived_source_sessions_untouched() {
        let db_path = temp_db_path("merge-archive");
        let raw_key = "ef".repeat(32);
        create_fixture_db(&db_path, &raw_key);
        {
            let conn = open_fixture(&db_path, &raw_key);
            conn.execute_batch("ALTER TABLE chat_session ADD COLUMN hidden_status TEXT;")
                .unwrap();
            conn.execute(
                "INSERT INTO project VALUES ('p-target', '222', 'biz-a', '目标项目', NULL)",
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO chat_session
                 (session_id, project_id, session_title, updated_at, deleted_at)
                 VALUES ('s-target', 'p-target', '当前对话', 2, NULL)",
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO chat_session
                 (session_id, project_id, session_title, updated_at, deleted_at, hidden_status)
                 VALUES ('s-archived', 'p1', '来源归档对话', 3, NULL, 'voice_discussion')",
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO session_project VALUES ('s-archived', 'p1')",
                [],
            )
            .unwrap();
        }

        let outcome = handover_master_records_from_merge_with_progress(
            &db_path,
            &raw_key,
            "222",
            "111",
            &|_| {},
        )
        .unwrap();

        assert_eq!(outcome.switched_sessions.len(), 1);
        assert_eq!(outcome.transferred_projects, 1);
        assert_eq!(outcome.switched_sessions[0].project_id, "p-target");
        let conn = open_fixture(&db_path, &raw_key);
        let archived: (String, String, String) = conn
            .query_row(
                "SELECT session_id, project_id, hidden_status FROM chat_session
                 WHERE session_id = 's-archived'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(
            archived,
            (
                "s-archived".to_string(),
                "p1".to_string(),
                "voice_discussion".to_string()
            )
        );
        let source_owner: String = conn
            .query_row(
                "SELECT user_id FROM project WHERE project_id = 'p1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(source_owner, "111");
        let archived_pivot: String = conn
            .query_row(
                "SELECT project_id FROM session_project WHERE session_id = 's-archived'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(archived_pivot, "p1");
        let _ = std::fs::remove_dir_all(db_path.parent().unwrap());
    }

    #[test]
    fn handover_keeps_session_bearing_row_when_deduplicating_biz() {
        // 2026-08-31 事故场景：同 biz 的非目标侧多行，rowid 最小的是空镜像，
        // 靠后的行挂有活跃会话。必须保留带会话行、只删空镜像，
        // 否则会话失去 project 归属变成孤儿，从 UI 消失。
        let db_path = temp_db_path("biz-dedup");
        let raw_key = "ee".repeat(32);
        create_fixture_db(&db_path, &raw_key);
        {
            let conn = open_fixture(&db_path, &raw_key);
            // biz-x：空镜像（rowid 更小）+ 带会话行。
            conn.execute(
                "INSERT INTO project VALUES ('p-x-empty', '333', 'biz-x', '空镜像', NULL)",
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO project VALUES ('p-x-live', '444', 'biz-x', '活跃项目', NULL)",
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO chat_session VALUES ('s-x', 'p-x-live', '活跃会话', 1770000001, NULL)",
                [],
            )
            .unwrap();
            // biz-y：来源空镜像 + 目标名下真实行。
            conn.execute(
                "INSERT INTO project VALUES ('p-y-src', '555', 'biz-y', '来源空镜像', NULL)",
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO project VALUES ('p-y-tgt', '222', 'biz-y', '目标项目', NULL)",
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO chat_session VALUES ('s-y', 'p-y-tgt', '目标会话', 1770000002, NULL)",
                [],
            )
            .unwrap();
            // biz-z：带会话行先插入、空镜像后插入，覆盖反向 rowid 顺序。
            conn.execute(
                "INSERT INTO project VALUES ('p-z-live', '666', 'biz-z', '活跃项目2', NULL)",
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO chat_session VALUES ('s-z', 'p-z-live', '活跃会话2', 1770000003, NULL)",
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO project VALUES ('p-z-empty', '777', 'biz-z', '空镜像2', NULL)",
                [],
            )
            .unwrap();
        }
        let outcome = handover_master_records(&db_path, &raw_key, "222").unwrap();

        let conn = open_fixture(&db_path, &raw_key);
        // biz-x：空镜像删除，带会话行保留并随行到 222。
        let empty_rows: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM project WHERE project_id = 'p-x-empty'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(empty_rows, 0, "空镜像行应被清理");
        let live_owner: String = conn
            .query_row(
                "SELECT user_id FROM project WHERE project_id = 'p-x-live'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(live_owner, "222", "带会话行应保留并随行");
        // 活跃会话不再孤儿：换腿后仍指向 p-x-live。
        let switched = outcome
            .switched_sessions
            .iter()
            .find(|session| session.project_id == "p-x-live")
            .expect("活跃会话应参与换腿");
        let live_session: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM chat_session WHERE session_id = ?1 AND project_id = 'p-x-live'",
                [&switched.session_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(live_session, 1, "换腿后会话应仍挂在 p-x-live 上");
        // biz-y：来源空镜像删除，目标真实行原样保留。
        let src_rows: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM project WHERE project_id = 'p-y-src'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(src_rows, 0, "来源空镜像应被清理而非撞 UNIQUE");
        let tgt_owner: String = conn
            .query_row(
                "SELECT user_id FROM project WHERE project_id = 'p-y-tgt'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(tgt_owner, "222");
        // biz-z：即使活跃行的 rowid 更小，后面的空镜像也必须清理。
        let reverse_empty_rows: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM project WHERE project_id = 'p-z-empty'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(reverse_empty_rows, 0, "反向 rowid 的空镜像也应被清理");
        let reverse_live_owner: String = conn
            .query_row(
                "SELECT user_id FROM project WHERE project_id = 'p-z-live'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(reverse_live_owner, "222");
        let _ = std::fs::remove_dir_all(db_path.parent().unwrap());
    }

    #[test]
    fn handover_batch_rewrites_multiple_sessions_with_cross_references() {
        // P7-3 批量重写等价性：多会话 + 跨会话消息引用 + 各表多行，
        // 合并映射集合重写后语义与逐会话版一致。
        let db_path = temp_db_path("batch");
        let raw_key = "ff".repeat(32);
        create_fixture_db(&db_path, &raw_key);
        {
            let conn = open_fixture(&db_path, &raw_key);
            // 同项目下第二会话 s2（两轮消息，reply 跨会话引用 s1 的 m1）。
            conn.execute(
                "INSERT INTO chat_session VALUES ('s2', 'p1', '会话B', 1770000001, NULL)",
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO chat_message (message_id, session_id, message_role, message_index, reply_to_message_id, response_message_id, deleted_at)
                 VALUES ('m3', 's2', 'user', 0, 'm1', 'm1', NULL)",
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO chat_message (message_id, session_id, message_role, message_index, reply_to_message_id, response_message_id, deleted_at)
                 VALUES ('m4', 's2', 'assistant', 1, 'm3', NULL, NULL)",
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO chat_message_general (message_id, content, created_at) VALUES ('m3', '问题', '2026-08-30')",
                [],
            )
            .unwrap();
            conn.execute("INSERT INTO chat_turn VALUES ('t2', 's2', NULL)", [])
                .unwrap();
            conn.execute("INSERT INTO task VALUES ('k2', 's2', NULL)", [])
                .unwrap();
            conn.execute(
                "INSERT INTO history_v2 (history_v2_id, session_id, messages, created_at) VALUES ('h2', 's2', '[]', '2026-08-30')",
                [],
            )
            .unwrap();
            conn.execute("INSERT INTO agent_run VALUES ('ar2', 's2')", [])
                .unwrap();
            conn.execute("INSERT INTO session_project VALUES ('s2', 'p1')", [])
                .unwrap();
        }

        let outcome = handover_master_records(&db_path, &raw_key, "222").unwrap();
        assert_eq!(outcome.switched_sessions.len(), 2);
        // 完整性校验已内含（正文指纹 + 行数守恒），返回结果代表写后验证通过。

        let conn = open_fixture(&db_path, &raw_key);
        // 旧会话 → 新会话映射（sessions_to_switch 无 ORDER BY，按 previous 定位）。
        let new_by_old: HashMap<String, String> = outcome
            .switched_sessions
            .iter()
            .map(|session| {
                (
                    session.previous_session_id.clone(),
                    session.session_id.clone(),
                )
            })
            .collect();
        let s1_new = &new_by_old["s1"];
        let s2_new = &new_by_old["s2"];
        // 两个会话都换腿，旧 ID 全部消失。
        let old_sessions: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM chat_session WHERE session_id IN ('s1', 's2')",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(old_sessions, 0);
        // 消息全部跟随各自新腿，且跨会话引用（m3 → m1）同步重写。
        let messages: Vec<(String, String, Option<String>)> = conn
            .prepare(
                "SELECT session_id, message_id, reply_to_message_id FROM chat_message ORDER BY session_id, message_index",
            )
            .unwrap()
            .query_map([], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?))
            })
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(messages.len(), 4);
        // s1 会话两条消息（m1/m2 的后继）。
        let s1_messages: Vec<&(String, String, Option<String>)> = messages
            .iter()
            .filter(|(session, _, _)| session == s1_new)
            .collect();
        assert_eq!(s1_messages.len(), 2, "s1 的消息应跟随第一条新腿");
        // s2 的 m3 后继：reply_to 指向 m1 的新 ID（跨会话引用重写）。
        let s2_first = messages
            .iter()
            .find(|(session, _, reply)| session == s2_new && reply.is_some())
            .unwrap();
        let m1_successor = s1_messages
            .iter()
            .find(|(_, _, reply)| reply.is_none())
            .map(|(_, id, _)| id.clone())
            .unwrap();
        assert_eq!(
            s2_first.2.as_deref(),
            Some(m1_successor.as_str()),
            "跨会话 reply 引用应被映射表重写"
        );
        // 全部身份旧 ID 清零（消息/轮次/任务/历史/运行）。
        for (old_id, table, column) in [
            ("m1", "chat_message", "message_id"),
            ("m3", "chat_message", "message_id"),
            ("t1", "chat_turn", "turn_id"),
            ("t2", "chat_turn", "turn_id"),
            ("k1", "task", "task_id"),
            ("k2", "task", "task_id"),
            ("h1", "history_v2", "history_v2_id"),
            ("h2", "history_v2", "history_v2_id"),
            ("ar1", "agent_run", "agent_run_id"),
            ("ar2", "agent_run", "agent_run_id"),
        ] {
            let count: i64 = conn
                .query_row(
                    &format!("SELECT COUNT(*) FROM \"{table}\" WHERE \"{column}\" = ?1"),
                    [old_id],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(count, 0, "{table}.{old_id} 旧 ID 应被重写");
        }
        let _ = std::fs::remove_dir_all(db_path.parent().unwrap());
    }

    #[test]
    fn handover_progress_reports_three_phases() {
        // P7-3 进度可见化：mapping/executing/verifying 三阶段，
        // 会话阶段按 1..N 上报且 label 为项目名。
        let db_path = temp_db_path("progress");
        let raw_key = "ab".repeat(32);
        create_fixture_db(&db_path, &raw_key);
        {
            let conn = open_fixture(&db_path, &raw_key);
            conn.execute(
                "INSERT INTO chat_session VALUES ('s2', 'p1', '会话B', 1770000001, NULL)",
                [],
            )
            .unwrap();
            conn.execute("INSERT INTO session_project VALUES ('s2', 'p1')", [])
                .unwrap();
        }

        let events = std::sync::Mutex::new(Vec::new());
        let outcome =
            handover_master_records_with_progress(&db_path, &raw_key, "222", &|progress| {
                events.lock().unwrap().push(progress)
            })
            .unwrap();
        assert_eq!(outcome.switched_sessions.len(), 2);

        let events = events.into_inner().unwrap();
        let mapping: Vec<&HandoverProgress> = events
            .iter()
            .filter(|event| event.phase == "mapping")
            .collect();
        assert_eq!(mapping.len(), 2, "mapping 逐会话上报");
        assert_eq!(mapping[0].current, 1);
        assert_eq!(mapping[0].total, 2);
        assert_eq!(mapping[0].label, "项目A", "label 用项目名");
        let executing: Vec<&HandoverProgress> = events
            .iter()
            .filter(|event| event.phase == "executing")
            .collect();
        assert!(!executing.is_empty(), "executing 按身份组上报");
        assert_eq!(
            executing.last().unwrap().current,
            executing.last().unwrap().total
        );
        let verifying: Vec<&HandoverProgress> = events
            .iter()
            .filter(|event| event.phase == "verifying")
            .collect();
        assert_eq!(verifying.len(), 2, "verifying 逐会话上报");
        assert_eq!(verifying[1].current, 2);
        assert_eq!(verifying[1].total, 2);
        let _ = std::fs::remove_dir_all(db_path.parent().unwrap());
    }

    #[test]
    fn handover_without_records_is_noop_transfer() {
        let db_path = temp_db_path("empty");
        let raw_key = "cc".repeat(32);
        create_fixture_db(&db_path, &raw_key);
        // 先随行到 222，再次向 222 交接：无记录可随行（全部已归属目标）。
        handover_master_records(&db_path, &raw_key, "222").unwrap();
        let outcome = handover_master_records(&db_path, &raw_key, "222").unwrap();
        assert_eq!(outcome.transferred_projects, 0);
        assert_eq!(outcome.switched_sessions.len(), 0);
        assert_eq!(outcome.previous_owner_user_id, None);
        let _ = std::fs::remove_dir_all(db_path.parent().unwrap());
    }

    #[test]
    fn handover_missing_db_reports_unavailable() {
        let db_path = temp_db_path("missing").with_file_name("none.db");
        assert_eq!(
            handover_master_records(&db_path, &"dd".repeat(32), "222").unwrap_err(),
            MasterHandoverError::DbUnavailable
        );
        let _ = std::fs::remove_dir_all(db_path.parent().unwrap());
    }

    #[test]
    fn backup_trio_copies_db_and_sidecars() {
        let db_path = temp_db_path("backup");
        std::fs::write(&db_path, b"db-bytes").unwrap();
        std::fs::write(sidecar(&db_path, "-wal"), b"wal-bytes").unwrap();
        let backup = backup_master_trio(&db_path).unwrap();
        assert!(backup.is_file());
        // P16-T1：备份为压缩形态——解压回读与源字节一致。
        let mut restored = backup.clone().into_os_string();
        restored.push(".restored");
        let restored = PathBuf::from(restored);
        decompress_backup_member(&backup, &restored).unwrap();
        assert_eq!(std::fs::read(&restored).unwrap(), b"db-bytes");
        // P16-T1：wal 备份为 `{原名}.zst` 去 .zst 后缀 + `-wal.zst`。
        let backup_name = backup.file_name().unwrap().to_string_lossy().into_owned();
        let wal_backup = backup.with_file_name(backup_name.replace(".zst", "-wal.zst"));
        let mut restored_wal = wal_backup.clone().into_os_string();
        restored_wal.push(".restored");
        let restored_wal = PathBuf::from(restored_wal);
        decompress_backup_member(&wal_backup, &restored_wal).unwrap();
        assert_eq!(std::fs::read(&restored_wal).unwrap(), b"wal-bytes");
        // 不存在的 -shm 不产生备份文件。
        let shm_backup = backup.with_file_name(backup_name.replace(".zst", "-shm.zst"));
        assert!(!shm_backup.exists());
        // 备份不覆盖：再次备份产生新文件（时间戳不同则路径不同；同秒内
        // create_new 拒绝覆盖，报 Io 而非静默覆盖）。
        std::fs::write(&db_path, b"db-bytes-2").unwrap();
        let second = backup_master_trio(&db_path);
        match second {
            Ok(path) => assert_ne!(path, backup),
            Err(MasterHandoverError::Io) => { /* 同秒重试：create_new 正确拒绝 */ }
            other => panic!("期望新备份或 Io，实际 {other:?}"),
        }
        let _ = std::fs::remove_dir_all(db_path.parent().unwrap());
    }

    #[test]
    fn wal_activity_detection_semantics() {
        // 双采样纯函数：无变化 = 静默；出现/消失/变化 = 活跃。
        let sample = TrioSample {
            mtime_unix_nanos: 1,
            size: 10,
        };
        assert!(!wal_activity_between(&None, &None));
        assert!(wal_activity_between(&None, &Some(sample.clone())));
        assert!(wal_activity_between(&Some(sample.clone()), &None));
        assert!(!wal_activity_between(
            &Some(sample.clone()),
            &Some(sample.clone())
        ));
        let changed = TrioSample {
            mtime_unix_nanos: 2,
            size: 10,
        };
        assert!(wal_activity_between(&Some(sample), &Some(changed)));
    }

    #[test]
    fn backup_chain_listing_aggregates_and_sorts() {
        let db_path = temp_db_path("chain");
        // 三份备份散件：1000（db+wal+shm）、900（仅 db）、异名散件（跳过）。
        let dir = db_path.parent().unwrap();
        std::fs::write(dir.join("database.db.switch-bak-1000"), [1u8; 100]).unwrap();
        std::fs::write(dir.join("database.db.switch-bak-1000-wal"), [1u8; 20]).unwrap();
        std::fs::write(dir.join("database.db.switch-bak-1000-shm"), [1u8; 5]).unwrap();
        std::fs::write(dir.join("database.db.switch-bak-900"), [1u8; 40]).unwrap();
        std::fs::write(dir.join("database.db.switch-bak-abc"), [1u8; 7]).unwrap();

        let chain = list_master_backups(&db_path);
        assert_eq!(chain.len(), 2, "异名散件不进入备份链");
        // 倒序：最新（1000）在前；附属件字节聚合，has_wal 标记。
        assert_eq!(chain[0].stamp_unix_seconds, 1000);
        assert_eq!(chain[0].total_bytes, 125);
        assert!(chain[0].has_wal);
        assert_eq!(chain[1].stamp_unix_seconds, 900);
        assert_eq!(chain[1].total_bytes, 40);
        assert!(!chain[1].has_wal);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn backup_chain_listing_empty_when_no_backups() {
        let db_path = temp_db_path("chain-empty");
        std::fs::write(&db_path, b"db").unwrap();
        // 无任何备份文件 → 空链（不报错）。
        assert!(list_master_backups(&db_path).is_empty());
        let _ = std::fs::remove_dir_all(db_path.parent().unwrap());
    }

    // ===== P11-4① 备份位置可选：跨目录备份 + 目录版枚举 + 凭据标记 =====

    /// backup_master_trio_to(Some)：三件套压缩写入目标目录（P16-T1 .zst）；
    /// db 原目录不产生任何备份文件（防误写）。解压回读与源字节一致。
    #[test]
    fn backup_master_trio_to_writes_target_dir() {
        let root = temp_db_path("trio-to");
        let db_path = root.join("data").join("database.db");
        std::fs::create_dir_all(&db_path.parent().unwrap()).unwrap();
        std::fs::write(&db_path, [1u8; 100]).unwrap();
        std::fs::write(db_path.parent().unwrap().join("database.db-wal"), [2u8; 20]).unwrap();
        let target = root.join("backups");
        let backup = backup_master_trio_to(&db_path, Some(&target)).unwrap();
        assert_eq!(backup.parent().unwrap(), target);
        assert!(backup.to_string_lossy().ends_with(".zst"));
        let stamp = backup
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .trim_start_matches("database.db.switch-bak-")
            .trim_end_matches(".zst")
            .to_string();
        // 目标目录：db + 存在的 wal 以压缩形态到位（解压回读字节一致）。
        let restored_db = root.join("restored-db");
        decompress_backup_member(
            &target.join("database.db.switch-bak-".to_string() + &stamp + ".zst"),
            &restored_db,
        )
        .unwrap();
        assert_eq!(std::fs::read(&restored_db).unwrap(), [1u8; 100]);
        let restored_wal = root.join("restored-wal");
        decompress_backup_member(
            &target.join("database.db.switch-bak-".to_string() + &stamp + "-wal.zst"),
            &restored_wal,
        )
        .unwrap();
        assert_eq!(std::fs::read(&restored_wal).unwrap(), [2u8; 20]);
        // db 原目录无新备份文件。
        let left: Vec<_> = std::fs::read_dir(db_path.parent().unwrap())
            .unwrap()
            .flatten()
            .map(|entry| entry.file_name().into_string().unwrap())
            .collect();
        assert_eq!(left.len(), 2, "原目录只剩 db 与 wal");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// P17-A3 滚动单份语义：第二次备份后旧份不存在；同目录同时刻只有
    /// 一份备份；旧链的凭据子目录与未压缩旧格式散件一并彻底删除（直删，
    /// 不进回收站——文件不存在即证）。
    #[test]
    fn rolling_backup_keeps_single_copy() {
        let root = temp_db_path("rolling");
        let db_path = root.join("data").join("database.db");
        std::fs::create_dir_all(db_path.parent().unwrap()).unwrap();
        std::fs::write(&db_path, [1u8; 64]).unwrap();
        std::fs::write(
            db_path.parent().unwrap().join("database.db-wal"),
            [2u8; 8],
        )
        .unwrap();
        let target = root.join("backups");
        std::fs::create_dir_all(&target).unwrap();
        // 旧链条目现场：未压缩旧格式散件 + 凭据子目录（P16 前遗留形态）。
        std::fs::write(target.join("database.db.switch-bak-900"), [3u8; 16]).unwrap();
        std::fs::create_dir_all(target.join("credentials-900")).unwrap();
        std::fs::write(target.join("credentials-900").join("accounts.json"), b"{}").unwrap();

        // 第一次滚动备份（显式 stamp 保证确定性）。
        let first = backup_master_trio_rolling_stamped(&db_path, &target, 1000).unwrap();
        assert!(
            first
                .file_name()
                .unwrap()
                .to_string_lossy()
                .ends_with("database.db.switch-bak-1000.zst")
        );
        // 旧份与凭据子目录已被彻底删除。
        assert!(!target.join("database.db.switch-bak-900").exists());
        assert!(!target.join("credentials-900").exists());
        // 目录内只剩新份一条链（db + 存在的 wal）。
        let chain = list_master_backups_in(&target, "database.db");
        assert_eq!(chain.len(), 1);
        assert_eq!(chain[0].stamp_unix_seconds, 1000);
        assert!(chain[0].has_wal);

        // 第二次滚动：新份替换旧份（旧份不存在 = 滚动单份核心断言）。
        std::fs::write(&db_path, [9u8; 32]).unwrap();
        backup_master_trio_rolling_stamped(&db_path, &target, 2000).unwrap();
        let chain = list_master_backups_in(&target, "database.db");
        assert_eq!(chain.len(), 1);
        assert_eq!(chain[0].stamp_unix_seconds, 2000);
        assert!(!first.exists(), "第一份备份已被滚动删除");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// P17-A3 彻底删除原语：成员与凭据子目录直删；对不存在的 stamp 幂等
    /// （无文件可删返回 0，不报错）。
    #[test]
    fn hard_delete_master_backup_is_direct_and_idempotent() {
        let dir = temp_db_path("hard-delete");
        let dir = dir.parent().unwrap().join("backups");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("database.db.switch-bak-800"), [1u8; 4]).unwrap();
        std::fs::write(dir.join("database.db.switch-bak-800-wal"), [1u8; 2]).unwrap();
        std::fs::create_dir_all(dir.join("credentials-800")).unwrap();
        std::fs::write(dir.join("credentials-800").join("profiles"), b"p").unwrap();
        // 直删：db 成员 + wal 成员 = 2 个文件；凭据子目录连带删除。
        assert_eq!(hard_delete_master_backup(&dir, "database.db", 800), 2);
        assert!(!dir.join("database.db.switch-bak-800").exists());
        assert!(!dir.join("database.db.switch-bak-800-wal").exists());
        assert!(!dir.join("database.db.switch-bak-800-shm").exists());
        assert!(!dir.join("credentials-800").exists());
        // 幂等：重复删除返回 0（无文件可删），不报错。
        assert_eq!(hard_delete_master_backup(&dir, "database.db", 800), 0);
        let _ = std::fs::remove_dir_all(dir.parent().unwrap());
    }

    /// backup_master_trio_to(None)：等价原 backup_master_trio 同目录行为
    /// （P16-T1 起写出压缩 .zst 形态）。
    #[test]
    fn backup_master_trio_none_keeps_same_dir() {
        let db_path = temp_db_path("trio-none");
        std::fs::write(&db_path, [1u8; 10]).unwrap();
        let backup = backup_master_trio_to(&db_path, None).unwrap();
        assert_eq!(backup.parent().unwrap(), db_path.parent().unwrap());
        assert!(backup
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .starts_with("database.db.switch-bak-"));
        let _ = std::fs::remove_dir_all(db_path.parent().unwrap());
    }

    /// 目录版枚举：credentials-{stamp} 子目录映射 has_credentials 标记。
    #[test]
    fn list_master_backups_in_reports_credentials() {
        let root = temp_db_path("chain-cred");
        let dir = root.parent().unwrap();
        std::fs::write(dir.join("database.db.switch-bak-1000"), [1u8; 100]).unwrap();
        std::fs::write(dir.join("database.db.switch-bak-900"), [1u8; 40]).unwrap();
        // 仅 1000 带凭据子目录。
        std::fs::create_dir_all(dir.join("credentials-1000")).unwrap();
        let chain = list_master_backups_in(dir, "database.db");
        assert_eq!(chain.len(), 2);
        assert_eq!(chain[0].stamp_unix_seconds, 1000);
        assert!(chain[0].has_credentials);
        assert_eq!(chain[1].stamp_unix_seconds, 900);
        assert!(!chain[1].has_credentials);
        let _ = std::fs::remove_dir_all(dir);
    }

    // ======================================================================
    // P13-2 Trae CN 项目归并（ADR-0033 选择性写：同目录跨账号行合并）
    // ======================================================================
    //
    // CN 库语义（探针 trae-cn-project-schema-probe 实测）：项目 = 目录
    // （absolute_path），同目录跨账号因 UNIQUE(biz_project_id, user_id)
    // 天然多行；归并 = 把同目录其他账号的行并入当前指针账号的同目录行。

    /// CN 形态 fixture：project 带 absolute_path（Work 旧 fixture 无此列，
    /// 定位复用会破坏既有位置化 INSERT，故独立建库）；chat_session 自带
    /// hidden_status（CN 探针 schema 同款）。
    /// 预置：账号 111 的 p1（biz-a，D:\\work\\demo，两个正常会话）。
    fn create_trae_cn_fixture_db(path: &Path, raw_key: &str) {
        let flags = OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE;
        let conn = Connection::open_with_flags(path, flags).unwrap();
        conn.execute_batch(&format!("PRAGMA key = \"x'{raw_key}'\";"))
            .unwrap();
        conn.execute_batch(
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
             CREATE TABLE chat_turn (
                turn_id TEXT PRIMARY KEY, session_id TEXT, deleted_at INTEGER);
             CREATE TABLE task (
                task_id TEXT PRIMARY KEY, session_id TEXT, deleted_at INTEGER);
             CREATE TABLE history_v2 (
                id INTEGER PRIMARY KEY AUTOINCREMENT, history_v2_id TEXT UNIQUE, session_id TEXT,
                messages TEXT, created_at TEXT);
             CREATE TABLE agent_run (
                agent_run_id TEXT PRIMARY KEY, session_id TEXT);
             CREATE TABLE session_project (
                session_id TEXT PRIMARY KEY, project_id TEXT);
             CREATE TABLE server_history_info (
                id INTEGER PRIMARY KEY, session_id TEXT, conversation_id TEXT,
                source TEXT, user_id TEXT);",
        )
        .unwrap();
    }

    /// 播种一条完整会话（2 条消息 + general 正文 + turn/task/history/
    /// agent_run/session_project；hidden 传 Some("voice_discussion") 即归档，
    /// deleted=true 即软删）。消息 id 形如 m-{session}-0/1，正文各 1 条。
    fn seed_cn_session(
        conn: &Connection,
        session_id: &str,
        project_id: &str,
        title: &str,
        hidden: Option<&str>,
        deleted: bool,
    ) {
        conn.execute(
            "INSERT INTO chat_session VALUES (?1, ?2, ?3, 1770000000, ?4, ?5)",
            rusqlite::params![
                session_id,
                project_id,
                title,
                if deleted { 1 } else { 0 },
                hidden,
            ],
        )
        .unwrap();
        for index in 0..2 {
            let message_id = format!("m-{session_id}-{index}");
            conn.execute(
                "INSERT INTO chat_message
                 (message_id, session_id, message_role, message_index, reply_to_message_id, response_message_id, deleted_at)
                 VALUES (?1, ?2, 'user', ?3, NULL, NULL, NULL)",
                rusqlite::params![message_id, session_id, index],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO chat_message_general (message_id, content, created_at)
                 VALUES (?1, ?2, '2026-09-29')",
                rusqlite::params![message_id, format!("正文-{message_id}")],
            )
            .unwrap();
        }
        conn.execute(
            "INSERT INTO chat_turn (turn_id, session_id, deleted_at) VALUES (?1, ?2, NULL)",
            rusqlite::params![format!("t-{session_id}"), session_id],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO task (task_id, session_id, deleted_at) VALUES (?1, ?2, NULL)",
            rusqlite::params![format!("k-{session_id}"), session_id],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO history_v2 (history_v2_id, session_id, messages, created_at)
             VALUES (?1, ?2, '[]', '2026-09-29')",
            rusqlite::params![format!("h-{session_id}"), session_id],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO agent_run (agent_run_id, session_id) VALUES (?1, ?2)",
            rusqlite::params![format!("ar-{session_id}"), session_id],
        )
        .unwrap();
        if !deleted {
            conn.execute(
                "INSERT INTO session_project VALUES (?1, ?2)",
                rusqlite::params![session_id, project_id],
            )
            .unwrap();
        }
    }

    /// 播种项目行。
    fn seed_cn_project(
        conn: &Connection,
        project_id: &str,
        user_id: &str,
        biz: Option<&str>,
        name: &str,
        absolute_path: Option<&str>,
    ) {
        conn.execute(
            "INSERT INTO project VALUES (?1, ?2, ?3, ?4, ?5, NULL)",
            rusqlite::params![project_id, user_id, biz, name, absolute_path],
        )
        .unwrap();
    }

    /// 标准场景：账号 111 的 p1（biz-a / D:\work\demo）挂两个正常会话。
    fn seed_standard_source(conn: &Connection) {
        seed_cn_project(
            conn,
            "p1",
            "111",
            Some("biz-a"),
            "演示项目",
            Some("D:\\work\\demo"),
        );
        seed_cn_session(conn, "s1", "p1", "会话一", None, false);
        seed_cn_session(conn, "s2", "p1", "会话二", None, false);
        // 云端镜像证据（E5/e5-r2：永不改写、新腿不携带）。
        conn.execute(
            "INSERT INTO server_history_info (session_id, conversation_id, source, user_id)
             VALUES ('s1', 'conv-old', 'llm_default', NULL)",
            [],
        )
        .unwrap();
    }

    /// 归并整体转入：目标账号名下无同目录行，来源行连人带会话转到目标。
    #[test]
    fn cn_merge_transfers_foreign_directory_row_to_target_account() {
        let db_path = temp_db_path("cn-merge-transfer");
        let raw_key = "aa".repeat(32);
        create_trae_cn_fixture_db(&db_path, &raw_key);
        {
            let conn = open_fixture(&db_path, &raw_key);
            seed_standard_source(&conn);
        }

        let outcome =
            merge_trae_cn_project_records_with_progress(&db_path, &raw_key, "222", "p1", &|_| {})
                .unwrap();

        assert_eq!(outcome.transferred_projects, 1, "来源行整体转入目标账号");
        assert_eq!(outcome.removed_mirror_rows, 0);
        assert_eq!(outcome.switched_sessions.len(), 2);
        assert_eq!(outcome.previous_owner_user_id.as_deref(), Some("111"));

        let conn = open_fixture(&db_path, &raw_key);
        let owner: String = conn
            .query_row(
                "SELECT user_id FROM project WHERE project_id = 'p1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(owner, "222", "来源行归属改写为当前指针账号");
        // 会话全部换腿：旧 ID 消失，身份表跟随新腿。
        let new_sessions: Vec<String> = outcome
            .switched_sessions
            .iter()
            .map(|session| session.session_id.clone())
            .collect();
        for (table, column) in [
            ("chat_message", "message_id"),
            ("chat_turn", "turn_id"),
            ("task", "task_id"),
            ("history_v2", "history_v2_id"),
            ("agent_run", "agent_run_id"),
        ] {
            let old_count: i64 = conn
                .query_row(
                    &format!("SELECT COUNT(*) FROM \"{table}\" WHERE session_id IN ('s1','s2')"),
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(old_count, 0, "{table} 旧 session 引用应清零");
            let _ = column;
        }
        let old_sessions: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM chat_session WHERE session_id IN ('s1','s2')",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(old_sessions, 0);
        // 会话仍挂在 p1 上（项目身份保留，只换归属与腿）。
        for new_session in &new_sessions {
            let project: String = conn
                .query_row(
                    "SELECT project_id FROM chat_session WHERE session_id = ?1",
                    [new_session],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(project, "p1");
            let pivot: String = conn
                .query_row(
                    "SELECT project_id FROM session_project WHERE session_id = ?1",
                    [new_session],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(pivot, "p1", "session_project 改挂跟随");
        }
        // 云端镜像证据原样保留（旧行永不改写）。
        let mirror: String = conn
            .query_row(
                "SELECT conversation_id FROM server_history_info WHERE session_id = 's1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(mirror, "conv-old");
        // 正文内容原样保留。
        let content_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM chat_message_general", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(content_count, 4);
        let _ = std::fs::remove_dir_all(db_path.parent().unwrap());
    }

    /// UNIQUE 冲突走合并分支：目标名下已有同目录真实行时，来源正常会话
    /// 并入目标行，目标身份与其既有会话不改写，来源行移空清理。
    #[test]
    fn cn_merge_moves_sessions_into_existing_target_row() {
        let db_path = temp_db_path("cn-merge-conflict");
        let raw_key = "bb".repeat(32);
        create_trae_cn_fixture_db(&db_path, &raw_key);
        {
            let conn = open_fixture(&db_path, &raw_key);
            seed_standard_source(&conn);
            seed_cn_project(
                &conn,
                "p-t",
                "222",
                Some("biz-a"),
                "我的演示项目",
                Some("D:\\work\\demo"),
            );
            seed_cn_session(&conn, "s-t", "p-t", "目标既有会话", None, false);
        }

        let outcome =
            merge_trae_cn_project_records_with_progress(&db_path, &raw_key, "222", "p1", &|_| {})
                .unwrap();

        assert_eq!(outcome.switched_sessions.len(), 2);
        let conn = open_fixture(&db_path, &raw_key);
        // 来源行移空清理；目标行身份保留。
        let source_rows: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM project WHERE project_id = 'p1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(source_rows, 0, "并入后的空来源行应被清理");
        let target_owner: String = conn
            .query_row(
                "SELECT user_id FROM project WHERE project_id = 'p-t'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(target_owner, "222");
        // 目标既有会话身份不改写。
        let target_session: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM chat_session WHERE session_id = 's-t'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(target_session, 1, "目标既有会话不换腿");
        // 来源会话全部改挂目标行且换腿。
        let moved_on_target: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM chat_session WHERE project_id = 'p-t'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(moved_on_target, 3, "目标行应挂 1 既有 + 2 并入会话");
        let moved_by_pivot: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM session_project WHERE project_id = 'p-t'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(moved_by_pivot, 3, "session_project 同步改挂");
        let _ = std::fs::remove_dir_all(db_path.parent().unwrap());
    }

    /// ADR-0033 决策 3：无路径 / 无 biz 的项目不参与归并（fail-closed，
    /// 库零改动）；未知项目 id 报 ProjectNotFound。
    #[test]
    fn cn_merge_rejects_pathless_unbiz_and_unknown_projects() {
        let db_path = temp_db_path("cn-merge-guard");
        let raw_key = "cc".repeat(32);
        create_trae_cn_fixture_db(&db_path, &raw_key);
        {
            let conn = open_fixture(&db_path, &raw_key);
            seed_standard_source(&conn);
            // 无路径项目（挂会话）：不可被转移。
            seed_cn_project(&conn, "p-nopath", "333", Some("biz-x"), "无路径项目", None);
            seed_cn_session(&conn, "s-np", "p-nopath", "无路径会话", None, false);
            // biz 为 NULL 的项目：没有目录身份，同样不可归并。
            seed_cn_project(
                &conn,
                "p-nobiz",
                "333",
                None,
                "无 biz 项目",
                Some("D:\\work\\x"),
            );
            seed_cn_session(&conn, "s-nb", "p-nobiz", "无 biz 会话", None, false);
        }

        let error = merge_trae_cn_project_records_with_progress(
            &db_path,
            &raw_key,
            "222",
            "p-nopath",
            &|_| {},
        )
        .unwrap_err();
        assert!(matches!(error, MasterHandoverError::ProjectNotMergeable));
        let error = merge_trae_cn_project_records_with_progress(
            &db_path,
            &raw_key,
            "222",
            "p-nobiz",
            &|_| {},
        )
        .unwrap_err();
        assert!(matches!(error, MasterHandoverError::ProjectNotMergeable));
        let error = merge_trae_cn_project_records_with_progress(
            &db_path,
            &raw_key,
            "222",
            "p-gone",
            &|_| {},
        )
        .unwrap_err();
        assert!(matches!(error, MasterHandoverError::ProjectNotFound));

        // 归属目标账号自己的行：不满足「归属非当前账号」门槛。
        seed_cn_project(
            &open_fixture(&db_path, &raw_key),
            "p-own",
            "222",
            Some("biz-own"),
            "自己的项目",
            Some("D:\\work\\own"),
        );
        let error = merge_trae_cn_project_records_with_progress(
            &db_path,
            &raw_key,
            "222",
            "p-own",
            &|_| {},
        )
        .unwrap_err();
        assert!(matches!(error, MasterHandoverError::ProjectNotMergeable));

        // 全部拒绝路径库零改动：无路径项目归属不变、会话原样。
        let conn = open_fixture(&db_path, &raw_key);
        let owner: String = conn
            .query_row(
                "SELECT user_id FROM project WHERE project_id = 'p-nopath'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(owner, "333");
        let sessions: i64 = conn
            .query_row("SELECT COUNT(*) FROM chat_session", [], |row| row.get(0))
            .unwrap();
        assert_eq!(
            sessions, 4,
            "拒绝路径不产生任何写（p1 两会话 + 无路径/无 biz 各一）"
        );
        let _ = std::fs::remove_dir_all(db_path.parent().unwrap());
    }

    /// 软删与归档会话是冻结内容：留在来源行、不并入、不换腿，来源行因有
    /// 残留会话而保留在原账号名下（与 Work 合并语义一致）。
    #[test]
    fn cn_merge_leaves_archived_and_soft_deleted_sessions_in_source_row() {
        let db_path = temp_db_path("cn-merge-frozen");
        let raw_key = "dd".repeat(32);
        create_trae_cn_fixture_db(&db_path, &raw_key);
        {
            let conn = open_fixture(&db_path, &raw_key);
            seed_standard_source(&conn);
            seed_cn_session(
                &conn,
                "s-arch",
                "p1",
                "归档会话",
                Some("voice_discussion"),
                false,
            );
            seed_cn_session(&conn, "s-del", "p1", "软删会话", None, true);
            seed_cn_project(
                &conn,
                "p-t",
                "222",
                Some("biz-a"),
                "我的演示项目",
                Some("D:\\work\\demo"),
            );
            seed_cn_session(&conn, "s-t", "p-t", "目标既有会话", None, false);
        }

        let outcome =
            merge_trae_cn_project_records_with_progress(&db_path, &raw_key, "222", "p1", &|_| {})
                .unwrap();

        assert_eq!(outcome.switched_sessions.len(), 2, "只有正常会话参与换腿");
        let switched_old: Vec<&str> = outcome
            .switched_sessions
            .iter()
            .map(|session| session.previous_session_id.as_str())
            .collect();
        assert!(switched_old.contains(&"s1") && switched_old.contains(&"s2"));
        assert!(
            !switched_old.contains(&"s-arch") && !switched_old.contains(&"s-del"),
            "归档/软删会话不进入换腿"
        );
        let conn = open_fixture(&db_path, &raw_key);
        // 来源行保留（残留冻结会话），归属不变。
        let source_owner: String = conn
            .query_row(
                "SELECT user_id FROM project WHERE project_id = 'p1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(source_owner, "111", "带冻结内容的来源行不得改归属");
        // 归档会话原身份、原挂接、原状态。
        let archived: (String, String, Option<String>) = conn
            .query_row(
                "SELECT session_id, project_id, hidden_status FROM chat_session WHERE session_id = 's-arch'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(archived.0, "s-arch");
        assert_eq!(archived.1, "p1");
        assert_eq!(archived.2.as_deref(), Some("voice_discussion"));
        let archived_pivot: String = conn
            .query_row(
                "SELECT project_id FROM session_project WHERE session_id = 's-arch'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(archived_pivot, "p1");
        // 软删会话原样。
        let soft_deleted: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM chat_session WHERE session_id = 's-del' AND deleted_at = 1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(soft_deleted, 1);
        // 正常会话已并入目标行。
        let target_total: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM chat_session WHERE project_id = 'p-t'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(target_total, 3, "目标既有 + 2 条并入");
        let _ = std::fs::remove_dir_all(db_path.parent().unwrap());
    }

    /// 空镜像清理：同目录其他账号的 0 会话行删除；目标名下的空镜像行
    /// 同样清理（E5 坑位语义同款）。
    #[test]
    fn cn_merge_cleans_empty_mirror_rows_in_same_directory() {
        let db_path = temp_db_path("cn-merge-mirror");
        let raw_key = "ee".repeat(32);
        create_trae_cn_fixture_db(&db_path, &raw_key);
        {
            let conn = open_fixture(&db_path, &raw_key);
            seed_standard_source(&conn);
            // 第三账号的同目录空镜像行。
            seed_cn_project(
                &conn,
                "p-m",
                "333",
                Some("biz-a"),
                "空镜像",
                Some("D:\\work\\demo"),
            );
            // 目标账号名下的同目录空镜像行（目标无真实行）。
            seed_cn_project(
                &conn,
                "p-t2",
                "222",
                Some("biz-a"),
                "目标镜像",
                Some("D:\\work\\demo"),
            );
        }

        let outcome =
            merge_trae_cn_project_records_with_progress(&db_path, &raw_key, "222", "p1", &|_| {})
                .unwrap();

        assert_eq!(outcome.removed_mirror_rows, 2, "两个空镜像行都应清理");
        assert_eq!(outcome.transferred_projects, 1, "来源行转入目标账号");
        let conn = open_fixture(&db_path, &raw_key);
        for mirror in ["p-m", "p-t2"] {
            let rows: i64 = conn
                .query_row(
                    &format!("SELECT COUNT(*) FROM project WHERE project_id = '{mirror}'"),
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(rows, 0, "{mirror} 空镜像应被清理");
        }
        let owner: String = conn
            .query_row(
                "SELECT user_id FROM project WHERE project_id = 'p1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(owner, "222");
        // 目标镜像行删除后 (biz-a, 222) 唯一键由转入的 p1 承接。
        let biz_rows: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM project WHERE biz_project_id = 'biz-a' AND user_id = '222'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(biz_rows, 1);
        let _ = std::fs::remove_dir_all(db_path.parent().unwrap());
    }

    /// 多账号同目录一次归并：两个来源账号 + 目标空镜像，单事务全部并入
    /// 用户点击的来源行（承接行 = 点击行，身份与路径保留）。
    #[test]
    fn cn_merge_merges_multiple_foreign_accounts_in_one_transaction() {
        let db_path = temp_db_path("cn-merge-multi");
        let raw_key = "ef".repeat(32);
        create_trae_cn_fixture_db(&db_path, &raw_key);
        {
            let conn = open_fixture(&db_path, &raw_key);
            seed_standard_source(&conn);
            // 第二来源账号（同目录、有会话）。
            seed_cn_project(
                &conn,
                "p3",
                "333",
                Some("biz-a"),
                "第三账号项目",
                Some("D:\\work\\demo"),
            );
            seed_cn_session(&conn, "s3", "p3", "第三账号会话", None, false);
            // 目标账号的空镜像行。
            seed_cn_project(
                &conn,
                "p-t2",
                "222",
                Some("biz-a"),
                "目标镜像",
                Some("D:\\work\\demo"),
            );
        }

        let outcome =
            merge_trae_cn_project_records_with_progress(&db_path, &raw_key, "222", "p1", &|_| {})
                .unwrap();

        assert_eq!(outcome.switched_sessions.len(), 3, "三个来源会话全部换腿");
        assert_eq!(outcome.removed_mirror_rows, 1, "目标空镜像清理");
        assert_eq!(outcome.transferred_projects, 2, "p3 并走 + p1 转入");
        assert_eq!(
            outcome.previous_owner_user_id.as_deref(),
            Some("111"),
            "会话最多者为前归属"
        );
        let conn = open_fixture(&db_path, &raw_key);
        // 承接行 = 点击的 p1，归属 222，三会话都在。
        let owner: String = conn
            .query_row(
                "SELECT user_id FROM project WHERE project_id = 'p1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(owner, "222");
        let path: String = conn
            .query_row(
                "SELECT absolute_path FROM project WHERE project_id = 'p1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(path, "D:\\work\\demo", "承接行身份与路径保留");
        let p3_rows: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM project WHERE project_id = 'p3'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(p3_rows, 0, "移空的第三账号行清理");
        let total: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM chat_session WHERE project_id = 'p1' AND COALESCE(deleted_at,0)=0",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(total, 3);
        let _ = std::fs::remove_dir_all(db_path.parent().unwrap());
    }

    /// 同目录来源只有归档/软删内容：无正常会话可并入 → NothingToMerge，
    /// 库零改动。
    #[test]
    fn cn_merge_nothing_to_merge_when_only_frozen_content() {
        let db_path = temp_db_path("cn-merge-nothing");
        let raw_key = "fe".repeat(32);
        create_trae_cn_fixture_db(&db_path, &raw_key);
        {
            let conn = open_fixture(&db_path, &raw_key);
            seed_cn_project(
                &conn,
                "p1",
                "111",
                Some("biz-a"),
                "冻结项目",
                Some("D:\\work\\demo"),
            );
            seed_cn_session(
                &conn,
                "s-arch",
                "p1",
                "归档会话",
                Some("voice_discussion"),
                false,
            );
            seed_cn_session(&conn, "s-del", "p1", "软删会话", None, true);
            seed_cn_project(
                &conn,
                "p-t",
                "222",
                Some("biz-a"),
                "我的项目",
                Some("D:\\work\\demo"),
            );
            seed_cn_session(&conn, "s-t", "p-t", "目标会话", None, false);
        }

        let error =
            merge_trae_cn_project_records_with_progress(&db_path, &raw_key, "222", "p1", &|_| {})
                .unwrap_err();
        assert!(matches!(error, MasterHandoverError::NothingToMerge));

        let conn = open_fixture(&db_path, &raw_key);
        let source_owner: String = conn
            .query_row(
                "SELECT user_id FROM project WHERE project_id = 'p1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(source_owner, "111", "无可并内容时库零改动");
        let _ = std::fs::remove_dir_all(db_path.parent().unwrap());
    }

    /// 预览：同目录各账号行 / 会话数 / 消息数（规模确认弹层数据源）；
    /// 目标账号无同目录行时 target_row 为 None。
    #[test]
    fn cn_merge_preview_reports_rows_and_counts() {
        let db_path = temp_db_path("cn-merge-preview");
        let raw_key = "ff".repeat(32);
        create_trae_cn_fixture_db(&db_path, &raw_key);
        {
            let conn = open_fixture(&db_path, &raw_key);
            seed_standard_source(&conn);
            seed_cn_session(
                &conn,
                "s-arch",
                "p1",
                "归档会话",
                Some("voice_discussion"),
                false,
            );
            seed_cn_project(
                &conn,
                "p-t",
                "222",
                Some("biz-a"),
                "我的项目",
                Some("D:\\work\\demo"),
            );
            seed_cn_session(&conn, "s-t", "p-t", "目标会话", None, false);
            // 无关目录，不应出现在预览里。
            seed_cn_project(
                &conn,
                "p-other",
                "333",
                Some("biz-z"),
                "别的目录",
                Some("D:\\elsewhere"),
            );
            seed_cn_session(&conn, "s-o", "p-other", "别的会话", None, false);
        }

        let preview = preview_trae_cn_project_merge(&db_path, &raw_key, "222", "p1").unwrap();
        assert_eq!(preview.biz_project_id, "biz-a");
        assert_eq!(preview.absolute_path.as_deref(), Some("D:\\work\\demo"));
        let target = preview.target_row.as_ref().unwrap();
        assert_eq!(target.project_id, "p-t");
        assert_eq!(target.user_id, "222");
        assert_eq!(target.normal_sessions, 1);
        assert_eq!(target.all_sessions, 1);
        assert_eq!(target.messages, 2);
        assert_eq!(preview.source_rows.len(), 1, "只含同目录来源行");
        let source = &preview.source_rows[0];
        assert_eq!(source.project_id, "p1");
        assert_eq!(source.user_id, "111");
        assert_eq!(source.normal_sessions, 2, "归档会话不计入待并入规模");
        assert_eq!(source.all_sessions, 3);
        assert_eq!(source.messages, 4);

        // 目标账号名下无同目录行：整体转入形态（对 999 而言 p1 与 p-t
        // 都是其他账号的行，全部是来源）。
        let preview = preview_trae_cn_project_merge(&db_path, &raw_key, "999", "p1").unwrap();
        assert!(preview.target_row.is_none());
        assert_eq!(preview.source_rows.len(), 2);

        // 无路径项目不可预览（与归并同门槛）。
        let error = preview_trae_cn_project_merge(&db_path, &raw_key, "222", "p-gone").unwrap_err();
        assert!(matches!(error, MasterHandoverError::ProjectNotFound));
        let _ = std::fs::remove_dir_all(db_path.parent().unwrap());
    }

    /// 进度回调：mapping/executing/verifying 三阶段逐项上报（与交接原语
    /// 同一 HandoverProgress 契约，前端进度事件直接转发）。
    #[test]
    fn cn_merge_reports_three_phase_progress() {
        let db_path = temp_db_path("cn-merge-progress");
        let raw_key = "ba".repeat(32);
        create_trae_cn_fixture_db(&db_path, &raw_key);
        {
            let conn = open_fixture(&db_path, &raw_key);
            seed_standard_source(&conn);
        }

        let events = std::sync::Mutex::new(Vec::new());
        let outcome = merge_trae_cn_project_records_with_progress(
            &db_path,
            &raw_key,
            "222",
            "p1",
            &|progress| events.lock().unwrap().push(progress),
        )
        .unwrap();
        assert_eq!(outcome.switched_sessions.len(), 2);

        let events = events.into_inner().unwrap();
        let mapping: Vec<&HandoverProgress> = events
            .iter()
            .filter(|event| event.phase == "mapping")
            .collect();
        assert_eq!(mapping.len(), 2, "mapping 逐会话上报");
        assert_eq!(mapping[0].total, 2);
        assert_eq!(mapping[0].label, "演示项目", "label 用项目名");
        let executing: Vec<&HandoverProgress> = events
            .iter()
            .filter(|event| event.phase == "executing")
            .collect();
        assert!(!executing.is_empty(), "executing 按身份组上报");
        let verifying: Vec<&HandoverProgress> = events
            .iter()
            .filter(|event| event.phase == "verifying")
            .collect();
        assert_eq!(verifying.len(), 2, "verifying 逐会话上报");
        let _ = std::fs::remove_dir_all(db_path.parent().unwrap());
    }
}
