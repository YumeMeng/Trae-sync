//! 主库记录交接原语（P5-1，Q1.1 五步事务核心）。
//!
//! 依据（`.scratch/grill-log-20260830-env-model.md` §8-§19 实证链）：
//! - **E1b**：本地会话按 `project.user_id` 过滤；单事务 UPDATE 即完成记录
//!   归属随行，消息数据零触碰（`server_history_info.user_id` 全 NULL）。
//! - **E5 坑位**：`UNIQUE(biz_project_id, user_id)` —— 每个登录过环境的账号
//!   都会在自己名下拉一份同 biz 的空镜像行；交接前必须自动清理目标账号的
//!   空镜像行，非空冲突报人工决策（禁止静默覆盖）。
//! - **e5-r2**：活跃会话原地换腿——事务内重写本地 session/message/turn/
//!   task/history/agent_run 身份为新腿，不携带旧 `server_history_info`，
//!   新腿首发自然创建新云端 conversation（换腿 SQL 逐句移植自验证探针
//!   `.scratch/history-u6/w0-probe/src/bin/rewrite_leg_in_place.rs`）。
//! - **§19 产品化约束**：不自动清理孤儿行；本地镜像（server_history_info）
//!   保留旧身份作为证据，永不改写、永不删除。
//!
//! 本模块只做第 4 步「交接」；关实例/备份/blob 互换/重启由调用方
//! （lib.rs 五步编排）负责。交接全程单事务：任一步失败整体回滚，
//! 环境不会停留在半切换状态。

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

/// 交接失败（全部非敏感：不含 key、消息内容或账号信息）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MasterHandoverError {
    /// 主库 database.db 不存在（主库尚未启动/登录过，无库可交接）。
    DbUnavailable,
    /// 打开或解密失败（raw key 不匹配或库损坏）。
    DbOpenFailed,
    /// UNIQUE(biz_project_id, user_id) 非空冲突，需人工决策（附 biz 短指纹）。
    TargetConflict(String),
    /// 换腿后正文完整性校验失败（事务已回滚，库保持交接前状态）。
    IntegrityFailed,
    /// 文件读写失败。
    Io,
}

impl std::fmt::Display for MasterHandoverError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let message = match self {
            Self::DbUnavailable => "主库对话库不存在",
            Self::DbOpenFailed => "主库对话库打开或解密失败",
            Self::TargetConflict(_) => "目标账号名下存在同名项目的非空记录，需人工决策",
            Self::IntegrityFailed => "换腿后正文完整性校验失败（已回滚）",
            Self::Io => "交接文件读写失败",
        };
        formatter.write_str(message)
    }
}

impl std::error::Error for MasterHandoverError {}

/// 一个被换腿的会话（接力台账数据源）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HandoverSession {
    /// 新腿 session_id（换腿后身份）。
    pub session_id: String,
    /// 旧腿 session_id（换腿前身份；P5-3 接力台账 from_session_id 数据源，
    /// 历史页据此把多次换腿链回成完整接力轨迹）。
    pub previous_session_id: String,
    /// 会话所属 project_id（归属随行后不变）。
    pub project_id: String,
    /// 交接前会话归属账号 user_id（P5-5 收编台账逐会话 from 数据源；
    /// 切号路径沿用 previous_owner 单值，多账号杂居时此字段更精确）。
    pub previous_user_id: String,
    /// 交接时会话累计消息数（chat_message 行数，台账轨迹用）。
    pub message_count: i64,
}

/// 交接结果（交接事务提交成功后返回）。
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

fn copy_file_create_new(source: &Path, target: &Path) -> Result<(), MasterHandoverError> {
    let mut input = std::fs::File::open(source).map_err(|_| MasterHandoverError::Io)?;
    let mut output = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(target)
        .map_err(|_| MasterHandoverError::Io)?;
    std::io::copy(&mut input, &mut output).map_err(|_| MasterHandoverError::Io)?;
    Ok(())
}

/// 主库三件套备份：db + 存在的 wal/shm 复制为 `.switch-bak-{时间戳}` sidecar。
///
/// `create_new` 保证永不覆盖既有备份（含历史备份链）；调用方（切号编排）
/// 保证此刻主库实例已关闭（第 1 步），复制到的是静止一致的三件套。
/// 返回备份主文件路径（供 DTO 展示与人工恢复定位）。
pub fn backup_master_trio(db_path: &Path) -> Result<PathBuf, MasterHandoverError> {
    if !db_path.is_file() {
        return Err(MasterHandoverError::DbUnavailable);
    }
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let backup = sidecar(db_path, &format!(".switch-bak-{stamp}"));
    copy_file_create_new(db_path, &backup)?;
    for suffix in ["-wal", "-shm"] {
        let source = sidecar(db_path, suffix);
        if source.is_file() {
            copy_file_create_new(&source, &sidecar(&backup, suffix))?;
        }
    }
    Ok(backup)
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
}

/// 枚举主库备份链：`{db 目录}/database.db.switch-bak-*`。
///
/// 只读扫描 + 按时间戳倒序（最新在前）；命名异常的散件跳过不报错
/// （备份链展示容错：不因一个异名文件让整页失败）。不读取文件内容。
pub fn list_master_backups(db_path: &Path) -> Vec<MasterBackupEntry> {
    let Some(dir) = db_path.parent() else {
        return Vec::new();
    };
    let Some(db_name) = db_path.file_name().and_then(|name| name.to_str()) else {
        return Vec::new();
    };
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
        // rest 形如 `{stamp}` 或 `{stamp}-wal` / `{stamp}-shm`。
        let (stamp_part, side) = match rest.strip_suffix("-wal") {
            Some(stamp) => (stamp, "-wal"),
            None => match rest.strip_suffix("-shm") {
                Some(stamp) => (stamp, "-shm"),
                None => (rest, ""),
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
    if !db_path.is_file() {
        return Err(MasterHandoverError::DbUnavailable);
    }
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
                     WHERE p.user_id != ?1 AND COALESCE(s.deleted_at, 0) = 0{normal_session_filter}"
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
                 WHERE p.user_id IS NOT NULL AND p.user_id != ?1
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
                 WHERE p.user_id != ?1 AND p.biz_project_id IS NOT NULL"
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
                // 双侧都有真实记录：禁止静默覆盖，报人工决策。
                return Err(MasterHandoverError::TargetConflict(biz_fingerprint(&biz)));
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
                 WHERE user_id != ?1 AND biz_project_id IS NOT NULL
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
                Some((kept_rank, _)) if rank == 0 => {
                    if *kept_rank == 0 {
                        mirrors_to_delete.push(project);
                    }
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

    // ===== 单事务执行：镜像清理 → 归属随行 → 集合式批量换腿 =====
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
    let transferred_projects = tx
        .execute(
            &format!(
                "UPDATE project SET user_id = ?1 WHERE user_id != ?1
                 AND EXISTS (
                     SELECT 1 FROM chat_session s
                     WHERE s.project_id = project.project_id
                       AND COALESCE(s.deleted_at, 0) = 0{normal_session_filter}
                 )"
            ),
            [target_user_id],
        )
        .map_err(|_| MasterHandoverError::DbOpenFailed)? as usize;

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
    let identity_groups: [(&str, &str, &[&str]); 5] = [
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
    let group_total = identity_groups.len() + 1; // 五类身份 + 会话本体
    for (index, (label, map_table, columns_to_update)) in identity_groups.iter().enumerate() {
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
                project_id: project_id.clone(),
                previous_user_id: previous_user_id.clone(),
                message_count: before_counts
                    .get(source_session)
                    .map(|(count, _)| *count)
                    .unwrap_or(0),
            },
        )
        .collect();
    tx.commit().map_err(|_| MasterHandoverError::DbOpenFailed)?;

    // ===== 写后完整性校验（集合读取：正文指纹 + 行数守恒）=====
    let new_session_ids: Vec<String> = leg_maps
        .iter()
        .map(|(_, new_session)| new_session.clone())
        .collect();
    let after_digests = text_digests(&conn, &new_session_ids)?;
    let after_counts = session_counts(&conn, &new_session_ids)?;
    let after_server_counts = session_server_counts(&conn, &new_session_ids)?;
    for (index, (old_session, _, _)) in sessions_to_switch.iter().enumerate() {
        let new_session = &leg_maps[index].1;
        on_progress(HandoverProgress {
            phase: "verifying",
            current: index + 1,
            total: total_sessions,
            label: session_labels[index].clone(),
        });
        if after_digests.get(new_session) != before_digests.get(old_session) {
            return Err(MasterHandoverError::IntegrityFailed);
        }
        if after_counts.get(new_session) != before_counts.get(old_session) {
            return Err(MasterHandoverError::IntegrityFailed);
        }
        // 新腿不得携带旧云端镜像（e5-r2 验收标准 2）。
        if after_server_counts.get(new_session).copied().unwrap_or(0) != 0 {
            return Err(MasterHandoverError::IntegrityFailed);
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
        // 完整性校验已内含（正文指纹 + 行数守恒），未报 IntegrityFailed。

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
        assert_eq!(std::fs::read(&backup).unwrap(), b"db-bytes");
        assert_eq!(
            std::fs::read(sidecar(&backup, "-wal")).unwrap(),
            b"wal-bytes"
        );
        // 不存在的 -shm 不产生备份文件。
        assert!(!sidecar(&backup, "-shm").exists());
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
}
