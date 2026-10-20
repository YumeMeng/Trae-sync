//! P19-B3：WorkBuddy 主库域移植（workbuddy.db 索引 + projects jsonl 正文）。
//!
//! 移植依据：p19-architecture-draft §4 + ADR-0037「对话库/收编/归并」行 +
//! 信息架构草案 §2 库域 18 条（矩阵 #58-#75 的 WB 对应面）。
//!
//! 协议事实（workbuddy_transfer.rs 头注 + .scratch/workbuddy-survey 实测）：
//! - `workbuddy.db`：明文 SQLite WAL，daemon 持有的活库；sessions 表按
//!   `user_id` 隔离、`transport='local'/'cloud'` 双态、`deleted_at` 软删除；
//! - 正文：`projects/<escape_cwd_dir(cwd)>/<sessionId>.jsonl`，全局共享
//!   （不带归属），jsonl 每行一个 JSON 对象（`type`/`timestamp`/`role`/
//!   `content` 数组块）；
//! - **红线：`transport='cloud'` 一律不写**（本模块全部写 SQL 均带
//!   `AND transport = 'local'`）；**红线：禁止客户端运行中写库**（写操作
//!   前进程纪律由组合根命令编排：完全退出 → 写 → 重启，同 B1 过户）。
//!
//! 与 TRAE master_archive 的复用关系：事务纪律状态机共用
//! [`crate::archive_tx::run_archive_transaction`]（归属重校验 → 事务 →
//! CommitOutcomeUnknown → 提交后读回校验）；连接层差异（明文无 key 探测）
//! 收在 [`open_readwrite`] 一个函数里。备份前置（滚动单份）由组合根在
//! 彻底删除入口调用 [`crate::workbuddy_backup`] 完成（软删除可逆、归并
//! 单事务回滚，按 P17-A3 触发点收敛语义不自动备份）。

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::time::Duration;

use rusqlite::Connection;
use serde::{Deserialize, Serialize};

use crate::archive_tx::{numbered_placeholders, run_archive_transaction};
use crate::workbuddy_transfer::{db_path, escape_cwd_dir, projects_path, validate_schema};

/// cwd 转义与路径推导复用 B1 过户模块（单一事实来源）。
pub use crate::workbuddy_transfer::escape_cwd_dir as escape_cwd;

/// 库域错误（组合根只暴露稳定错误码，不透传内部细节）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkBuddyLibraryError {
    /// workbuddy.db 不存在（数据根未初始化）。
    DbMissing,
    /// workbuddy.db 打开失败（损坏/被独占）。
    DbUnavailable,
    /// sessions 表结构不符（schema 漂移防御，同 B1 validate_schema）。
    SchemaInvalid,
    /// 读路径 SQL 失败。
    ReadFailed,
    /// 写路径 SQL 失败（事务已回滚，库保持操作前状态）。
    WriteFailed,
    /// SQLite 返回提交错误，无法证明事务最终持久状态。
    CommitOutcomeUnknown,
    /// 事务已提交，但提交后读回校验未通过。
    CommittedNeedsAttention,
    /// 归并参数无效（无变体 / 规范 cwd 在变体集合中）。
    MergeInvalid,
}

impl WorkBuddyLibraryError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::DbMissing => "wb_library_db_missing",
            Self::DbUnavailable => "wb_library_db_unavailable",
            Self::SchemaInvalid => "wb_library_schema_invalid",
            Self::ReadFailed => "wb_library_read_failed",
            Self::WriteFailed => "wb_library_write_failed",
            Self::CommitOutcomeUnknown => "wb_library_commit_outcome_unknown",
            Self::CommittedNeedsAttention => "wb_library_committed_needs_attention",
            Self::MergeInvalid => "wb_library_merge_invalid",
        }
    }
}

/// 读写连接（明文库：直接 open；busy_timeout 与 daemon 写入错峰，绝不长锁）。
fn open_readwrite(db: &Path) -> Result<Connection, WorkBuddyLibraryError> {
    if !db.is_file() {
        return Err(WorkBuddyLibraryError::DbMissing);
    }
    let conn = Connection::open(db).map_err(|_| WorkBuddyLibraryError::DbUnavailable)?;
    conn.busy_timeout(Duration::from_secs(10))
        .map_err(|_| WorkBuddyLibraryError::DbUnavailable)?;
    Ok(conn)
}

/// 只读连接（WAL 读并发安全，可在线读取——不要求客户端退出）。
fn open_read_only(db: &Path) -> Result<Connection, WorkBuddyLibraryError> {
    if !db.is_file() {
        return Err(WorkBuddyLibraryError::DbMissing);
    }
    Connection::open_with_flags(db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|_| WorkBuddyLibraryError::DbUnavailable)
}

/// 毫秒/秒双单位时间戳归一化为秒（master_stats 同口径）。
fn normalize_timestamp_to_seconds(raw: i64) -> i64 {
    if raw > 1_000_000_000_000 {
        raw / 1000
    } else {
        raw
    }
}

// ===== 统计（总览库统计卡 + 主库卡数据源；矩阵 #64） =====

/// WB 库聚合统计（全库口径 + 软删规模；DTO 直接序列化形态）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LibraryStats {
    /// 库内出现过的账号数（distinct sessions.user_id 非空）。
    pub account_count: u64,
    /// 会话总数（非软删；全部账号）。
    pub session_count: u64,
    /// 软删除会话数（deleted_at IS NOT NULL；UI 隐藏但占用库）。
    pub soft_deleted_session_count: u64,
    /// 项目数（cwd 维度 distinct；非软删会话）。
    pub project_count: u64,
    /// projects 正文目录磁盘字节（递归统计；目录缺失为 0）。
    pub projects_dir_bytes: u64,
    /// 最近会话活跃时间（秒）；无会话为 None。
    pub last_active_unix_seconds: Option<i64>,
}

/// 读取库统计（只读、在线；软删行不计入可见计数但单独报告规模）。
pub fn read_library_stats(
    data_root: &Path,
) -> Result<LibraryStats, WorkBuddyLibraryError> {
    let db = db_path(data_root);
    let conn = open_read_only(&db)?;
    read_library_stats_inner(&conn, &projects_path(data_root))
}

fn read_library_stats_inner(
    conn: &Connection,
    projects_root: &Path,
) -> Result<LibraryStats, WorkBuddyLibraryError> {
    let count = |sql: &str| -> Result<u64, WorkBuddyLibraryError> {
        conn.query_row(sql, [], |row| Ok(row.get::<_, i64>(0)?.max(0) as u64))
            .map_err(|_| WorkBuddyLibraryError::ReadFailed)
    };
    let account_count = count(
        "SELECT COUNT(DISTINCT user_id) FROM sessions WHERE user_id IS NOT NULL AND user_id != ''",
    )?;
    let session_count = count("SELECT COUNT(*) FROM sessions WHERE deleted_at IS NULL")?;
    let soft_deleted_session_count = count("SELECT COUNT(*) FROM sessions WHERE deleted_at IS NOT NULL")?;
    let project_count = count(
        "SELECT COUNT(DISTINCT cwd) FROM sessions WHERE deleted_at IS NULL AND cwd != ''",
    )?;
    let last_active: Option<i64> = conn
        .query_row(
            "SELECT MAX(COALESCE(last_activity_at, updated_at, created_at)) FROM sessions \
             WHERE deleted_at IS NULL",
            [],
            |row| row.get(0),
        )
        .map_err(|_| WorkBuddyLibraryError::ReadFailed)?;
    Ok(LibraryStats {
        account_count,
        session_count,
        soft_deleted_session_count,
        project_count,
        projects_dir_bytes: dir_size_recursive(projects_root),
        last_active_unix_seconds: last_active.map(normalize_timestamp_to_seconds),
    })
}

/// 目录磁盘字节合计（递归；不可读项按 0 计——统计口径，fail-soft）。
fn dir_size_recursive(dir: &Path) -> u64 {
    let mut total = 0;
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    for entry in entries.flatten() {
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        if metadata.is_dir() {
            total += dir_size_recursive(&entry.path());
        } else if metadata.is_file() {
            total += metadata.len();
        }
    }
    total
}

// ===== 会话浏览（按账号过滤 + cwd 分组；矩阵 #63 视图面） =====

/// 会话行（列表 DTO；不含正文内容）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionRow {
    pub session_id: String,
    pub title: String,
    /// 云同步标记（transport='cloud' 行只读展示，写操作红线不碰）。
    pub transport: String,
    /// 是否软删除（deleted_at IS NOT NULL；UI 默认隐藏）。
    pub deleted: bool,
    /// 最近活跃（秒；缺失回退 created_at 再缺失为 None）。
    pub last_active_unix_seconds: Option<i64>,
}

/// cwd 分组（WB 的"项目"= cwd 转义目录维度；对齐 TRAE 项目分组 DTO 形状）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CwdGroup {
    /// 原始 cwd（归并/过户操作入口的稳定键）。
    pub cwd: String,
    /// 转义目录名（projects/<escaped>）。
    pub escaped_dir: String,
    pub session_count: u64,
    pub last_active_unix_seconds: Option<i64>,
    pub sessions: Vec<SessionRow>,
}

/// 按账号列出会话（cwd 分组；uid=None 时返回全部账号，include_deleted
/// 控制软删行——软删行过户/体检视图需要，常规浏览隐藏）。
pub fn list_sessions(
    data_root: &Path,
    user_id: Option<&str>,
    include_deleted: bool,
) -> Result<Vec<CwdGroup>, WorkBuddyLibraryError> {
    let db = db_path(data_root);
    let conn = open_read_only(&db)?;
    list_sessions_inner(&conn, user_id, include_deleted)
}

fn list_sessions_inner(
    conn: &Connection,
    user_id: Option<&str>,
    include_deleted: bool,
) -> Result<Vec<CwdGroup>, WorkBuddyLibraryError> {
    let mut sql = String::from(
        "SELECT id, COALESCE(custom_title, title, ''), transport, deleted_at, \
         COALESCE(last_activity_at, updated_at, created_at), cwd \
         FROM sessions WHERE 1=1",
    );
    let mut params: Vec<rusqlite::types::Value> = Vec::new();
    if let Some(uid) = user_id {
        sql.push_str(&format!(" AND user_id = ?{}", params.len() + 1));
        params.push(rusqlite::types::Value::Text(uid.to_string()));
    }
    if !include_deleted {
        sql.push_str(" AND deleted_at IS NULL");
    }
    sql.push_str(" ORDER BY 5 DESC");
    let mut stmt = conn
        .prepare(&sql)
        .map_err(|_| WorkBuddyLibraryError::ReadFailed)?;
    let rows = stmt
        .query_map(rusqlite::params_from_iter(params), |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Option<i64>>(3)?,
                row.get::<_, Option<i64>>(4)?,
                row.get::<_, String>(5)?,
            ))
        })
        .map_err(|_| WorkBuddyLibraryError::ReadFailed)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| WorkBuddyLibraryError::ReadFailed)?;

    // cwd 分组（保持时间倒序插入序；组间按组内最近活跃倒序）。
    let mut order: Vec<String> = Vec::new();
    let mut groups: HashMap<String, CwdGroup> = HashMap::new();
    for (id, title, transport, deleted_at, last_active, cwd) in rows {
        let group = groups.entry(cwd.clone()).or_insert_with(|| {
            order.push(cwd.clone());
            CwdGroup {
                escaped_dir: escape_cwd_dir(&cwd),
                session_count: 0,
                last_active_unix_seconds: None,
                sessions: Vec::new(),
                cwd,
            }
        });
        group.session_count += 1;
        let last_active_seconds = last_active.map(normalize_timestamp_to_seconds);
        if group.last_active_unix_seconds.is_none() {
            group.last_active_unix_seconds = last_active_seconds;
        }
        group.sessions.push(SessionRow {
            session_id: id,
            title,
            transport,
            deleted: deleted_at.is_some(),
            last_active_unix_seconds: last_active_seconds,
        });
    }
    Ok(order
        .into_iter()
        .filter_map(|cwd| groups.remove(&cwd))
        .collect())
}

// ===== 消息读取（索引定位 + jsonl 正文解析；矩阵 #66） =====

/// 单条消息（jsonl 行解析结果；仅暴露阅读面字段）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionMessage {
    /// 行序号（0 起；含跳过行的占位）。
    pub index: usize,
    /// 事件类型原文（"session-meta" / "message" / 其他透传）。
    pub kind: String,
    /// 消息角色（"user"/"assistant"/…；非 message 行为 None）。
    pub role: Option<String>,
    /// 事件时间戳（毫秒原文）。
    pub timestamp_unix_ms: Option<i64>,
    /// 文本内容（content 数组 text 块拼接 / 字符串 content；缺失为 None）。
    pub text: Option<String>,
}

/// 消息读取结果（正文文件缺失是常态降级态，不是错误）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionMessages {
    pub session_id: String,
    pub messages: Vec<SessionMessage>,
    /// 无法按 JSON 解析而被跳过的行数（容错口径，正文只读）。
    pub skipped_lines: u64,
    /// 正文文件缺失标记（true = messages 为空且文件不存在）。
    pub body_missing: bool,
}

/// 读取会话消息正文（只读）。
///
/// `expected_uid`：Some 时校验归属（UI 常规路径——uid 不匹配拒绝读取，
/// 防止跨账号窥探）；None 为工具侧检查路径（体检/抽验，全库可读）。
pub fn read_session_messages(
    data_root: &Path,
    session_id: &str,
    expected_uid: Option<&str>,
) -> Result<SessionMessages, WorkBuddyLibraryError> {
    let db = db_path(data_root);
    let conn = open_read_only(&db)?;
    let row: Option<(String, Option<String>)> = conn
        .query_row(
            "SELECT cwd, user_id FROM sessions WHERE id = ?1",
            [session_id],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?)),
        )
        .map(Some)
        .or_else(|error| match error {
            rusqlite::Error::QueryReturnedNoRows => Ok(None),
            other => Err(other),
        })
        .map_err(|_| WorkBuddyLibraryError::ReadFailed)?;
    let Some((cwd, owner)) = row else {
        return Err(WorkBuddyLibraryError::ReadFailed);
    };
    if let Some(uid) = expected_uid {
        if owner.as_deref() != Some(uid) {
            // 归属不符：与"会话不存在"同响应（不泄露他人会话存在性）。
            return Err(WorkBuddyLibraryError::ReadFailed);
        }
    }
    let body = projects_path(data_root)
        .join(escape_cwd_dir(&cwd))
        .join(format!("{session_id}.jsonl"));
    let Ok(content) = std::fs::read_to_string(&body) else {
        return Ok(SessionMessages {
            session_id: session_id.to_string(),
            messages: Vec::new(),
            skipped_lines: 0,
            body_missing: true,
        });
    };
    Ok(parse_session_messages(session_id, &content))
}

/// jsonl 内容解析（逐行 JSON；抽取 kind/role/timestamp/text 阅读面字段）。
fn parse_session_messages(session_id: &str, content: &str) -> SessionMessages {
    let mut messages = Vec::new();
    let mut skipped = 0u64;
    for (index, line) in content.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_str::<serde_json::Value>(trimmed) else {
            skipped += 1;
            continue;
        };
        let kind = value
            .get("type")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let role = value
            .get("role")
            .and_then(|v| v.as_str())
            .map(str::to_string);
        let timestamp_unix_ms = value.get("timestamp").and_then(|v| v.as_i64());
        let text = extract_message_text(&value);
        messages.push(SessionMessage {
            index,
            kind,
            role,
            timestamp_unix_ms,
            text,
        });
    }
    SessionMessages {
        session_id: session_id.to_string(),
        messages,
        skipped_lines: skipped,
        body_missing: false,
    }
}

/// 文本抽取：content 为字符串直取；为数组时拼接带 `text` 字段的块
/// （真机样例形态 `{"type":"input_text","text":"…"}`）；其余形态 None。
fn extract_message_text(value: &serde_json::Value) -> Option<String> {
    match value.get("content") {
        Some(serde_json::Value::String(text)) => Some(text.clone()),
        Some(serde_json::Value::Array(blocks)) => {
            let mut parts = Vec::new();
            for block in blocks {
                if let Some(text) = block.get("text").and_then(|t| t.as_str()) {
                    parts.push(text.to_string());
                }
            }
            if parts.is_empty() {
                None
            } else {
                Some(parts.join("\n"))
            }
        }
        _ => None,
    }
}

// ===== 软删除（写路径；进程纪律由组合根保证，矩阵 #60） =====

/// 软删除回执（UPDATE deleted_at；可逆——恢复入口走同款 UPDATE 置 NULL）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SoftDeleteOutcome {
    /// 实际置为软删除的行数（已软删行不重复计数）。
    pub deleted_sessions: usize,
    /// 未命中（不属于该账号 / cloud / 已不存在）而被跳过的 ID 数。
    pub skipped_ids: usize,
}

/// 软删除会话（**破坏性写前纪律：组合根必须先让客户端完全退出**）。
///
/// 事务纪律：归属重校验（ID 必须属于该账号且 transport='local'——界面 ID
/// 可能过期）→ 单事务 UPDATE deleted_at → CommitOutcomeUnknown → 提交后
/// 读回校验。红线：cloud 行任何情况下不碰。
pub fn soft_delete_sessions(
    data_root: &Path,
    session_ids: &[String],
    user_id: &str,
    deleted_at_unix_ms: i64,
) -> Result<SoftDeleteOutcome, WorkBuddyLibraryError> {
    if session_ids.is_empty() {
        return Ok(SoftDeleteOutcome {
            deleted_sessions: 0,
            skipped_ids: 0,
        });
    }
    let db = db_path(data_root);
    validate_schema(&db).map_err(|_| WorkBuddyLibraryError::SchemaInvalid)?;
    let conn = open_readwrite(&db)?;
    let (_authorized_ids, deleted_sessions) = run_archive_transaction(
        &conn,
        || WorkBuddyLibraryError::WriteFailed,
        |tx| -> Result<(Vec<String>, usize), WorkBuddyLibraryError> {
            // 归属重校验：只保留本账号 local 且尚未软删的行（cloud/他人/
            // 不存在/已软删 ID 跳过——已软删行无需重复置位，且提交后读回
            // 校验按本次置位集合核对，混入已删行会误报）。
            let owner_param = session_ids.len() + 1;
            let transport_param = session_ids.len() + 2;
            let sql = format!(
                "SELECT id FROM sessions WHERE id IN ({}) \
                 AND user_id = ?{owner_param} AND transport = ?{transport_param} \
                 AND deleted_at IS NULL",
                numbered_placeholders(session_ids.len(), 1)
            );
            let mut params: Vec<rusqlite::types::Value> = session_ids
                .iter()
                .cloned()
                .map(rusqlite::types::Value::Text)
                .collect();
            params.push(rusqlite::types::Value::Text(user_id.to_string()));
            params.push(rusqlite::types::Value::Text("local".to_string()));
            let mut stmt = tx
                .prepare(&sql)
                .map_err(|_| WorkBuddyLibraryError::WriteFailed)?;
            let authorized: Vec<String> = stmt
                .query_map(rusqlite::params_from_iter(params.iter()), |row| row.get(0))
                .map_err(|_| WorkBuddyLibraryError::WriteFailed)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|_| WorkBuddyLibraryError::WriteFailed)?;
            if authorized.is_empty() {
                return Ok((Vec::new(), 0));
            }
            let in_clause = numbered_placeholders(authorized.len(), 1);
            let deleted_param = authorized.len() + 1;
            let now_param = authorized.len() + 2;
            let mut update_params: Vec<rusqlite::types::Value> = authorized
                .iter()
                .cloned()
                .map(rusqlite::types::Value::Text)
                .collect();
            update_params.push(rusqlite::types::Value::Text(user_id.to_string()));
            update_params.push(rusqlite::types::Value::Integer(deleted_at_unix_ms));
            let deleted = tx
                .execute(
                    &format!(
                        "UPDATE sessions SET deleted_at = ?{now_param} \
                         WHERE id IN ({in_clause}) AND user_id = ?{deleted_param} \
                         AND transport = 'local' AND deleted_at IS NULL"
                    ),
                    rusqlite::params_from_iter(update_params),
                )
                .map_err(|_| WorkBuddyLibraryError::WriteFailed)?;
            Ok((authorized, deleted))
        },
        || WorkBuddyLibraryError::CommitOutcomeUnknown,
        |conn, (authorized, _)| {
            // 提交后读回：授权 ID 的软删位必须全部置上。
            if authorized.is_empty() {
                return Ok(());
            }
            let in_clause = numbered_placeholders(authorized.len(), 1);
            let mut params: Vec<rusqlite::types::Value> = authorized
                .iter()
                .cloned()
                .map(rusqlite::types::Value::Text)
                .collect();
            params.push(rusqlite::types::Value::Integer(deleted_at_unix_ms));
            let now_param = authorized.len() + 1;
            let actual: i64 = conn
                .query_row(
                    &format!(
                        "SELECT COUNT(*) FROM sessions WHERE id IN ({in_clause}) \
                         AND deleted_at = ?{now_param}"
                    ),
                    rusqlite::params_from_iter(params),
                    |row| row.get(0),
                )
                .map_err(|_| WorkBuddyLibraryError::CommittedNeedsAttention)?;
            if actual as usize == authorized.len() {
                Ok(())
            } else {
                Err(WorkBuddyLibraryError::CommittedNeedsAttention)
            }
        },
    )?;
    Ok(SoftDeleteOutcome {
        deleted_sessions,
        skipped_ids: session_ids.len() - deleted_sessions,
    })
}

// ===== 彻底删除（决策 7 单独确认入口；备份前置由组合根完成） =====

/// 彻底删除回执（行删除 + 正文文件删除两个面向的真实规模）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PurgeOutcome {
    /// 删除的索引行数（sessions 硬删除）。
    pub purged_sessions: usize,
    /// 删除的正文文件数。
    pub removed_body_files: usize,
    /// 未找到正文文件的会话数（行已删，文件本来就不在）。
    pub missing_body_files: usize,
    /// 正文文件删除失败的会话 ID（行已删；残留文件由体检孤儿项兜底）。
    pub failed_body_files: Vec<String>,
}

/// 彻底删除会话（**破坏性终点：组合根单次确认 + 备份前置 + 客户端完全
/// 退出后调用**）。行删除走单事务；正文文件删除在提交后进行（文件系统
/// 无事务——失败如实报告，残留文件由体检孤儿项兜底清理）。
pub fn purge_sessions(
    data_root: &Path,
    session_ids: &[String],
    user_id: &str,
) -> Result<PurgeOutcome, WorkBuddyLibraryError> {
    if session_ids.is_empty() {
        return Ok(PurgeOutcome {
            purged_sessions: 0,
            removed_body_files: 0,
            missing_body_files: 0,
            failed_body_files: Vec::new(),
        });
    }
    let db = db_path(data_root);
    validate_schema(&db).map_err(|_| WorkBuddyLibraryError::SchemaInvalid)?;
    let conn = open_readwrite(&db)?;
    let affected: Vec<(String, String)> = run_archive_transaction(
        &conn,
        || WorkBuddyLibraryError::WriteFailed,
        |tx| -> Result<Vec<(String, String)>, WorkBuddyLibraryError> {
            // 归属重校验 + 收集正文定位（id, cwd）——只删本账号 local 行。
            let owner_param = session_ids.len() + 1;
            let transport_param = session_ids.len() + 2;
            let sql = format!(
                "SELECT id, cwd FROM sessions WHERE id IN ({}) \
                 AND user_id = ?{owner_param} AND transport = ?{transport_param}",
                numbered_placeholders(session_ids.len(), 1)
            );
            let mut params: Vec<rusqlite::types::Value> = session_ids
                .iter()
                .cloned()
                .map(rusqlite::types::Value::Text)
                .collect();
            params.push(rusqlite::types::Value::Text(user_id.to_string()));
            params.push(rusqlite::types::Value::Text("local".to_string()));
            let mut stmt = tx
                .prepare(&sql)
                .map_err(|_| WorkBuddyLibraryError::WriteFailed)?;
            let affected: Vec<(String, String)> = stmt
                .query_map(rusqlite::params_from_iter(params.iter()), |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })
                .map_err(|_| WorkBuddyLibraryError::WriteFailed)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|_| WorkBuddyLibraryError::WriteFailed)?;
            if affected.is_empty() {
                return Ok(Vec::new());
            }
            let ids: Vec<String> = affected.iter().map(|(id, _)| id.clone()).collect();
            let in_clause = numbered_placeholders(ids.len(), 1);
            let deleted = tx
                .execute(
                    &format!(
                        "DELETE FROM sessions WHERE id IN ({in_clause}) \
                         AND transport = 'local'"
                    ),
                    rusqlite::params_from_iter(ids.iter().map(String::as_str)),
                )
                .map_err(|_| WorkBuddyLibraryError::WriteFailed)?;
            debug_assert_eq!(deleted, ids.len());
            Ok(affected)
        },
        || WorkBuddyLibraryError::CommitOutcomeUnknown,
        |conn, affected| {
            // 提交后读回：受影响 ID 必须全部消失。
            if affected.is_empty() {
                return Ok(());
            }
            let ids: Vec<String> = affected.iter().map(|(id, _)| id.clone()).collect();
            let in_clause = numbered_placeholders(ids.len(), 1);
            let remain: i64 = conn
                .query_row(
                    &format!("SELECT COUNT(*) FROM sessions WHERE id IN ({in_clause})"),
                    rusqlite::params_from_iter(ids.iter().map(String::as_str)),
                    |row| row.get(0),
                )
                .map_err(|_| WorkBuddyLibraryError::CommittedNeedsAttention)?;
            if remain == 0 {
                Ok(())
            } else {
                Err(WorkBuddyLibraryError::CommittedNeedsAttention)
            }
        },
    )?;

    // 正文文件删除（提交后；文件系统操作无事务，逐文件 best-effort 报告）。
    let projects_root = projects_path(data_root);
    let mut removed = 0usize;
    let mut missing = 0usize;
    let mut failed = Vec::new();
    for (session_id, cwd) in &affected {
        let body = projects_root
            .join(escape_cwd_dir(cwd))
            .join(format!("{session_id}.jsonl"));
        if !body.is_file() {
            missing += 1;
            continue;
        }
        match std::fs::remove_file(&body) {
            Ok(()) => removed += 1,
            Err(_) => failed.push(session_id.clone()),
        }
    }
    Ok(PurgeOutcome {
        purged_sessions: affected.len(),
        removed_body_files: removed,
        missing_body_files: missing,
        failed_body_files: failed,
    })
}

// ===== cwd 归并（同 cwd 不同转义形态归并到规范 cwd；矩阵 #71） =====

/// cwd 归并回执。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MergeCwdOutcome {
    /// 改挂到规范 cwd 的会话数（含软删行——归位语义与 TRAE 归并一致）。
    pub moved_sessions: usize,
    /// 迁移的正文文件数。
    pub moved_body_files: usize,
    /// 源位置无正文文件的会话数。
    pub missing_body_files: usize,
    /// 目标位置已有同名正文而放弃迁移的会话 ID（不覆盖——冲突如实报告）。
    pub conflict_body_files: Vec<String>,
}

/// cwd 归并（**破坏性写前纪律：组合根必须先让客户端完全退出**）。
///
/// 把 `variant_cwds` 名下的全部 local 会话（不分账号——cwd 是项目维度，
/// 对齐 ADR-0037「归并」行）改挂 `canonical_cwd`，并把正文文件从旧转义
/// 目录迁往新目录。单事务回滚保证库侧一致；文件迁移在提交后进行，
/// 失败如实报告（库已归并，文件残留由体检/重跑兜底）。
pub fn merge_cwd_projects(
    data_root: &Path,
    canonical_cwd: &str,
    variant_cwds: &[String],
) -> Result<MergeCwdOutcome, WorkBuddyLibraryError> {
    if canonical_cwd.trim().is_empty()
        || variant_cwds.is_empty()
        || variant_cwds.iter().any(|cwd| cwd == canonical_cwd)
    {
        return Err(WorkBuddyLibraryError::MergeInvalid);
    }
    let db = db_path(data_root);
    validate_schema(&db).map_err(|_| WorkBuddyLibraryError::SchemaInvalid)?;
    let conn = open_readwrite(&db)?;
    let affected: Vec<(String, String)> = run_archive_transaction(
        &conn,
        || WorkBuddyLibraryError::WriteFailed,
        |tx| -> Result<Vec<(String, String)>, WorkBuddyLibraryError> {
            // 归属重校验（cwd 维度）：只动 local 行（cloud 红线不碰）；
            // 软删行一并归并（与 TRAE merge 含归档/软删语义一致）。
            let transport_param = variant_cwds.len() + 1;
            let sql = format!(
                "SELECT id, cwd FROM sessions WHERE cwd IN ({}) \
                 AND transport = ?{transport_param}",
                numbered_placeholders(variant_cwds.len(), 1)
            );
            let mut params: Vec<rusqlite::types::Value> = variant_cwds
                .iter()
                .cloned()
                .map(rusqlite::types::Value::Text)
                .collect();
            params.push(rusqlite::types::Value::Text("local".to_string()));
            let mut stmt = tx
                .prepare(&sql)
                .map_err(|_| WorkBuddyLibraryError::WriteFailed)?;
            let affected: Vec<(String, String)> = stmt
                .query_map(rusqlite::params_from_iter(params.iter()), |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })
                .map_err(|_| WorkBuddyLibraryError::WriteFailed)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|_| WorkBuddyLibraryError::WriteFailed)?;
            let canonical_param = variant_cwds.len() + 1;
            tx.execute(
                    &format!(
                        "UPDATE sessions SET cwd = ?{canonical_param} \
                         WHERE cwd IN ({}) AND transport = 'local'",
                        numbered_placeholders(variant_cwds.len(), 1)
                    ),
                    rusqlite::params_from_iter(
                        variant_cwds
                            .iter()
                            .chain(std::iter::once(&canonical_cwd.to_string()))
                            .map(String::as_str),
                    ),
                )
                .map_err(|_| WorkBuddyLibraryError::WriteFailed)?;
            Ok(affected)
        },
        || WorkBuddyLibraryError::CommitOutcomeUnknown,
        |conn, _affected| {
            // 提交后读回：变体 cwd 名下不得再有任何 local 行。
            let transport_param = variant_cwds.len() + 1;
            let remain: i64 = conn
                .query_row(
                    &format!(
                        "SELECT COUNT(*) FROM sessions WHERE cwd IN ({}) \
                         AND transport = ?{transport_param}",
                        numbered_placeholders(variant_cwds.len(), 1)
                    ),
                    rusqlite::params_from_iter(
                        variant_cwds
                            .iter()
                            .chain(std::iter::once(&"local".to_string()))
                            .map(String::as_str),
                    ),
                    |row| row.get(0),
                )
                .map_err(|_| WorkBuddyLibraryError::CommittedNeedsAttention)?;
            if remain == 0 {
                Ok(())
            } else {
                Err(WorkBuddyLibraryError::CommittedNeedsAttention)
            }
        },
    )?;

    // 正文迁移（提交后）：旧转义目录 → 新转义目录；不覆盖既有目标文件。
    let projects_root = projects_path(data_root);
    let target_dir = projects_root.join(escape_cwd_dir(canonical_cwd));
    let mut moved_files = 0usize;
    let mut missing_files = 0usize;
    let mut conflicts = Vec::new();
    for (session_id, old_cwd) in &affected {
        let source = projects_root
            .join(escape_cwd_dir(old_cwd))
            .join(format!("{session_id}.jsonl"));
        if !source.is_file() {
            missing_files += 1;
            continue;
        }
        // NTFS 大小写不敏感：目标目录与源目录同实体（仅大小写差异）时
        // 无需迁移文件（重命名目录名即可见，留给目录级整理）。
        if dirs_same_entity(&source, &target_dir.join(format!("{session_id}.jsonl"))) {
            moved_files += 1;
            continue;
        }
        let target = target_dir.join(format!("{session_id}.jsonl"));
        if target.exists() {
            conflicts.push(session_id.clone());
            continue;
        }
        if std::fs::create_dir_all(&target_dir).is_err() {
            conflicts.push(session_id.clone());
            continue;
        }
        match std::fs::rename(&source, &target) {
            Ok(()) => moved_files += 1,
            Err(_) => conflicts.push(session_id.clone()),
        }
    }
    Ok(MergeCwdOutcome {
        moved_sessions: affected.len(),
        moved_body_files: moved_files,
        missing_body_files: missing_files,
        conflict_body_files: conflicts,
    })
}

/// 两个路径是否指向同一实体（NTFS 大小写不敏感场景：同目录不同写法）。
fn dirs_same_entity(left: &Path, right: &Path) -> bool {
    match (std::fs::canonicalize(left), std::fs::canonicalize(right)) {
        (Ok(left), Ok(right)) => left == right,
        // 任一端不存在（目标文件未创建）时按非同实体处理。
        _ => false,
    }
}

// ===== 体检（矩阵 #62/#65/#67/#70：孤儿正文/缺失正文/软删规模/外部目录） =====

/// 体检报告（只读；全部规模如实呈现，不做半套降级）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LibraryCheckup {
    /// 孤儿正文文件（projects 下的 jsonl 在索引中无对应会话行）。
    pub orphan_body_files: Vec<String>,
    /// 缺失正文的 local 会话数（索引有行、正文无文件；cloud 不计）。
    pub missing_body_count: u64,
    /// 缺失正文的会话 ID 样例（最多 50 条，供 UI 呈现）。
    pub missing_body_samples: Vec<String>,
    /// 软删除会话数（UI 隐藏但占库）。
    pub soft_deleted_session_count: u64,
    /// 外部目录（projects 下未被任何会话 cwd 的转义名引用的目录）。
    pub foreign_dirs: Vec<String>,
    /// cwd 归并候选：转义目录名仅大小写不同的 cwd 分组
    /// （escape_cwd_dir 历史版本差异——wb_transfer.py 整串 lower 遗留）。
    pub cwd_merge_candidates: Vec<Vec<String>>,
}

/// 库体检（只读、在线）。
pub fn read_library_checkup(data_root: &Path) -> Result<LibraryCheckup, WorkBuddyLibraryError> {
    let db = db_path(data_root);
    let conn = open_read_only(&db)?;
    read_library_checkup_inner(&conn, &projects_path(data_root))
}

fn read_library_checkup_inner(
    conn: &Connection,
    projects_root: &Path,
) -> Result<LibraryCheckup, WorkBuddyLibraryError> {
    // 索引面：全量会话（id → cwd、transport、deleted_at）。
    let mut stmt = conn
        .prepare("SELECT id, cwd, transport, deleted_at FROM sessions")
        .map_err(|_| WorkBuddyLibraryError::ReadFailed)?;
    let rows: Vec<(String, String, String, Option<i64>)> = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Option<i64>>(3)?,
            ))
        })
        .map_err(|_| WorkBuddyLibraryError::ReadFailed)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| WorkBuddyLibraryError::ReadFailed)?;

    let all_ids: HashSet<&str> = rows.iter().map(|(id, ..)| id.as_str()).collect();
    let soft_deleted = rows.iter().filter(|(.., deleted_at)| deleted_at.is_some()).count() as u64;
    // 缺失正文：local 会话逐个核对（cloud 正文本地存在性不做断言——
    // 云同步会话的本地缓存策略未逆向，不猜测）。
    let mut missing_body = Vec::new();
    for (id, cwd, transport, _) in &rows {
        if transport != "local" {
            continue;
        }
        let body = projects_root
            .join(escape_cwd_dir(cwd))
            .join(format!("{id}.jsonl"));
        if !body.is_file() {
            missing_body.push(id.clone());
        }
    }

    // 正文目录面：目录引用集 = 全部 cwd 的转义名。
    let referenced_dirs: HashSet<String> = rows
        .iter()
        .map(|(_, cwd, ..)| escape_cwd_dir(cwd))
        .collect();
    let mut orphan_files = Vec::new();
    let mut foreign_dirs = Vec::new();
    if let Ok(entries) = std::fs::read_dir(projects_root) {
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let dir_name = entry.file_name().to_string_lossy().into_owned();
            if !referenced_dirs.contains(&dir_name) {
                // 外部目录内的 jsonl 一律视为孤儿正文候选。
                if let Ok(files) = std::fs::read_dir(&path) {
                    for file in files.flatten() {
                        let name = file.file_name().to_string_lossy().into_owned();
                        if let Some(stem) = name.strip_suffix(".jsonl") {
                            if !all_ids.contains(stem) {
                                orphan_files.push(format!("{dir_name}/{name}"));
                            }
                        }
                    }
                }
                foreign_dirs.push(dir_name);
                continue;
            }
            // 引用目录内的孤儿文件（索引无该会话行）。
            if let Ok(files) = std::fs::read_dir(&path) {
                for file in files.flatten() {
                    let name = file.file_name().to_string_lossy().into_owned();
                    if let Some(stem) = name.strip_suffix(".jsonl") {
                        if !all_ids.contains(stem) {
                            orphan_files.push(format!("{dir_name}/{name}"));
                        }
                    }
                }
            }
        }
    }

    // 归并候选：不同 cwd 的转义名仅大小写不同（历史转义规则差异）。
    let mut by_lowercase: HashMap<String, HashSet<String>> = HashMap::new();
    for (_, cwd, ..) in &rows {
        if cwd.is_empty() {
            continue;
        }
        by_lowercase
            .entry(escape_cwd_dir(cwd).to_ascii_lowercase())
            .or_default()
            .insert(escape_cwd_dir(cwd));
    }
    let mut candidates: Vec<Vec<String>> = by_lowercase
        .into_values()
        .filter(|names| names.len() > 1)
        .map(|names| {
            let mut names: Vec<String> = names.into_iter().collect();
            names.sort();
            names
        })
        .collect();
    candidates.sort();

    orphan_files.sort();
    foreign_dirs.sort();
    Ok(LibraryCheckup {
        missing_body_samples: missing_body.iter().take(50).cloned().collect(),
        missing_body_count: missing_body.len() as u64,
        orphan_body_files: orphan_files,
        foreign_dirs,
        soft_deleted_session_count: soft_deleted,
        cwd_merge_candidates: candidates,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;
    use std::path::PathBuf;

    const UID_A: &str = "11e1dfbf-d66d-42d1-9d1b-d6017334f0b1";
    const UID_B: &str = "89b16d6b-0549-4e1e-8ced-918b5c8152ae";

    /// 现造最小 workbuddy.db 副本（schema 对齐真机 sessions 相关子集，
    /// 同 workbuddy_transfer 测试口径——不用真实用户库）。
    fn fixture_db(dir: &Path) -> PathBuf {
        let db = db_path(dir);
        let conn = Connection::open(&db).unwrap();
        conn.execute_batch(
            "CREATE TABLE sessions (
                id TEXT PRIMARY KEY,
                cwd TEXT,
                user_id TEXT,
                title TEXT,
                custom_title TEXT,
                created_at INTEGER,
                updated_at INTEGER,
                last_activity_at INTEGER,
                deleted_at INTEGER,
                transport TEXT
            );",
        )
        .unwrap();
        db
    }

    fn insert_session(
        conn: &Connection,
        id: &str,
        user_id: &str,
        cwd: &str,
        transport: &str,
        deleted_at: Option<i64>,
        last_activity: i64,
    ) {
        conn.execute(
            "INSERT INTO sessions (id, cwd, user_id, title, custom_title, created_at, \
             updated_at, last_activity_at, deleted_at, transport) \
             VALUES (?1, ?2, ?3, ?4, NULL, 1000, 1000, ?5, ?6, ?7)",
            rusqlite::params![id, cwd, user_id, format!("title-{id}"), last_activity, deleted_at, transport],
        )
        .unwrap();
    }

    fn write_body(data_root: &Path, cwd: &str, session_id: &str, content: &str) {
        let dir = projects_path(data_root).join(escape_cwd_dir(cwd));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(format!("{session_id}.jsonl")), content).unwrap();
    }

    // ---- 统计 ----

    #[test]
    fn library_stats_counts_accounts_projects_and_soft_deleted() {
        let temp = tempfile::tempdir().unwrap();
        {
            let conn = Connection::open(&fixture_db(temp.path())).unwrap();
            insert_session(&conn, "s1", UID_A, "D:/w", "local", None, 100);
            insert_session(&conn, "s2", UID_A, "D:/w", "local", None, 200);
            insert_session(&conn, "s3", UID_A, "D:/w", "local", Some(1), 300);
            insert_session(&conn, "s4", UID_B, "E:/x", "local", None, 400);
            insert_session(&conn, "s5", UID_B, "E:/x", "cloud", None, 500);
        }
        write_body(temp.path(), "D:/w", "s1", "{}");
        let stats = read_library_stats(temp.path()).unwrap();
        assert_eq!(stats.account_count, 2);
        assert_eq!(stats.session_count, 4, "软删 s3 不计可见会话");
        assert_eq!(stats.soft_deleted_session_count, 1);
        assert_eq!(stats.project_count, 2, "cwd 维度：D:/w 与 E:/x");
        assert_eq!(stats.last_active_unix_seconds, Some(500));
        assert!(stats.projects_dir_bytes > 0);
    }

    // ---- 会话浏览 ----

    #[test]
    fn list_sessions_groups_by_cwd_and_filters_uid_and_deleted() {
        let temp = tempfile::tempdir().unwrap();
        {
            let conn = Connection::open(&fixture_db(temp.path())).unwrap();
            insert_session(&conn, "s1", UID_A, "D:/w", "local", None, 100);
            insert_session(&conn, "s2", UID_A, "D:/w", "local", Some(1), 200);
            insert_session(&conn, "s3", UID_B, "E:/x", "cloud", None, 300);
        }
        // A 账号常规视图：只有 s1（软删与 B 账号均不出现）。
        let groups = list_sessions(temp.path(), Some(UID_A), false).unwrap();
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].cwd, "D:/w");
        assert_eq!(groups[0].escaped_dir, "d-w");
        assert_eq!(groups[0].session_count, 1);
        assert_eq!(groups[0].sessions[0].session_id, "s1");
        // 含软删视图：s2 出现且带 deleted 标记。
        let groups = list_sessions(temp.path(), Some(UID_A), true).unwrap();
        assert_eq!(groups[0].session_count, 2);
        assert!(groups[0].sessions.iter().any(|s| s.session_id == "s2" && s.deleted));
        // 全库视图（uid=None）：两组，cloud 行如实带 transport 标记。
        let groups = list_sessions(temp.path(), None, false).unwrap();
        assert_eq!(groups.len(), 2);
        let cloud = groups
            .iter()
            .flat_map(|g| g.sessions.iter())
            .find(|s| s.session_id == "s3")
            .unwrap();
        assert_eq!(cloud.transport, "cloud");
    }

    // ---- 消息读取 ----

    #[test]
    fn session_messages_parse_jsonl_lines_and_report_missing_body() {
        let temp = tempfile::tempdir().unwrap();
        {
            let conn = Connection::open(&fixture_db(temp.path())).unwrap();
            insert_session(&conn, "s1", UID_A, "D:/work/Trae-sync", "local", None, 100);
        }
        // 无正文文件：BodyMissing 降级（不是错误）。
        let empty = read_session_messages(temp.path(), "s1", Some(UID_A)).unwrap();
        assert!(empty.body_missing);
        assert!(empty.messages.is_empty());

        write_body(
            temp.path(),
            "D:/work/Trae-sync",
            "s1",
            "{\"type\":\"session-meta\",\"timestamp\":1791956378353}\n\
             {\"id\":\"m1\",\"type\":\"message\",\"role\":\"user\",\"timestamp\":1791956329992,\
             \"content\":[{\"type\":\"input_text\",\"text\":\"第一段\"},{\"type\":\"input_text\",\"text\":\"第二段\"}]}\n\
             非法行不是 JSON\n",
        );
        let messages = read_session_messages(temp.path(), "s1", Some(UID_A)).unwrap();
        assert_eq!(messages.messages.len(), 2);
        assert_eq!(messages.skipped_lines, 1);
        assert_eq!(messages.messages[0].kind, "session-meta");
        assert_eq!(messages.messages[0].role, None);
        assert_eq!(messages.messages[0].timestamp_unix_ms, Some(1791956378353));
        assert_eq!(messages.messages[1].role.as_deref(), Some("user"));
        assert_eq!(
            messages.messages[1].text.as_deref(),
            Some("第一段\n第二段"),
            "content 数组 text 块拼接"
        );
    }

    #[test]
    fn session_messages_reject_foreign_uid_without_existence_leak() {
        let temp = tempfile::tempdir().unwrap();
        {
            let conn = Connection::open(&fixture_db(temp.path())).unwrap();
            insert_session(&conn, "s1", UID_A, "D:/w", "local", None, 100);
        }
        // 他人 uid 读取：与"不存在"同错误码（不泄露会话存在性）。
        assert_eq!(
            read_session_messages(temp.path(), "s1", Some(UID_B)).unwrap_err(),
            WorkBuddyLibraryError::ReadFailed
        );
        // 工具侧检查路径（uid=None）可读。
        assert!(read_session_messages(temp.path(), "s1", None).is_ok());
    }

    // ---- 软删除 ----

    #[test]
    fn soft_delete_updates_owned_local_rows_and_never_touches_cloud() {
        let temp = tempfile::tempdir().unwrap();
        {
            let conn = Connection::open(&fixture_db(temp.path())).unwrap();
            insert_session(&conn, "a1", UID_A, "D:/w", "local", None, 10);
            insert_session(&conn, "a2", UID_A, "D:/w", "local", None, 11);
            insert_session(&conn, "a-del", UID_A, "D:/w", "local", Some(1), 12);
            insert_session(&conn, "a-cloud", UID_A, "D:/w", "cloud", None, 13);
            insert_session(&conn, "b1", UID_B, "D:/w", "local", None, 20);
        }
        let outcome = soft_delete_sessions(
            temp.path(),
            &["a1".into(), "a2".into(), "a-del".into(), "a-cloud".into(), "b1".into(), "ghost".into()],
            UID_A,
            7777,
        )
        .unwrap();
        assert_eq!(outcome.deleted_sessions, 2, "已软删 a-del 不重复计数");
        assert_eq!(outcome.skipped_ids, 4, "cloud/他人/不存在 ID 全部跳过");
        let conn = Connection::open_with_flags(
            &db_path(temp.path()),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .unwrap();
        let deleted_at = |id: &str| -> Option<i64> {
            conn.query_row(
                "SELECT deleted_at FROM sessions WHERE id = ?1",
                [id],
                |row| row.get(0),
            )
            .unwrap()
        };
        assert_eq!(deleted_at("a1"), Some(7777));
        assert_eq!(deleted_at("a2"), Some(7777));
        assert_eq!(deleted_at("a-del"), Some(1), "已软删行不被覆盖时间戳");
        assert_eq!(deleted_at("a-cloud"), None, "红线：cloud 行不碰");
        assert_eq!(deleted_at("b1"), None, "他人账号行不碰");
    }

    // ---- 彻底删除 ----

    #[test]
    fn purge_deletes_rows_and_body_files_for_owned_local_only() {
        let temp = tempfile::tempdir().unwrap();
        {
            let conn = Connection::open(&fixture_db(temp.path())).unwrap();
            insert_session(&conn, "a1", UID_A, "D:/w", "local", None, 10);
            insert_session(&conn, "a-cloud", UID_A, "D:/w", "cloud", None, 13);
            insert_session(&conn, "b1", UID_B, "D:/w", "local", None, 20);
        }
        write_body(temp.path(), "D:/w", "a1", "{\"x\":1}");
        write_body(temp.path(), "D:/w", "a-cloud", "{\"x\":2}");
        write_body(temp.path(), "D:/w", "b1", "{\"x\":3}");
        let outcome =
            purge_sessions(temp.path(), &["a1".into(), "a-cloud".into(), "ghost".into()], UID_A)
                .unwrap();
        assert_eq!(outcome.purged_sessions, 1);
        assert_eq!(outcome.removed_body_files, 1);
        assert_eq!(outcome.missing_body_files, 0);
        // cloud 与他人会话：行与正文都原样。
        let conn = Connection::open(&db_path(temp.path())).unwrap();
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM sessions", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 2, "cloud 行与 b1 行保留");
        assert!(projects_path(temp.path())
            .join(escape_cwd_dir("D:/w"))
            .join("a-cloud.jsonl")
            .is_file(), "红线：cloud 正文不碰");
        assert!(projects_path(temp.path())
            .join(escape_cwd_dir("D:/w"))
            .join("b1.jsonl")
            .is_file());
        assert!(!projects_path(temp.path())
            .join(escape_cwd_dir("D:/w"))
            .join("a1.jsonl")
            .is_file(), "a1 正文被删除");
    }

    // ---- cwd 归并 ----

    #[test]
    fn merge_cwd_moves_local_rows_of_all_accounts_and_migrates_files() {
        let temp = tempfile::tempdir().unwrap();
        {
            let conn = Connection::open(&fixture_db(temp.path())).unwrap();
            // 历史转义差异模拟：同项目两种 cwd 写法（大小写差异段）。
            insert_session(&conn, "a1", UID_A, "D:/work/Trae-sync", "local", None, 10);
            insert_session(&conn, "a2", UID_A, "D:/work/trae-sync", "local", Some(1), 11);
            insert_session(&conn, "b1", UID_B, "D:/work/trae-sync", "local", None, 20);
            insert_session(&conn, "c1", UID_A, "D:/work/Trae-sync", "cloud", None, 30);
        }
        write_body(temp.path(), "D:/work/Trae-sync", "a1", "{}");
        write_body(temp.path(), "D:/work/trae-sync", "b1", "{}");
        let outcome = merge_cwd_projects(
            temp.path(),
            "D:/work/Trae-sync",
            &["D:/work/trae-sync".to_string()],
        )
        .unwrap();
        // 变体集命中：a2（软删行一并归并）与 b1（他人账号 local 行）。
        assert_eq!(outcome.moved_sessions, 2, "变体 cwd 名下全部 local 行改挂（含软删 a2）");
        assert_eq!(outcome.moved_body_files, 1, "b1 正文迁移；a2 无正文");
        assert_eq!(outcome.missing_body_files, 1);
        let conn = Connection::open(&db_path(temp.path())).unwrap();
        let cwds: Vec<String> = {
            let mut stmt = conn
                .prepare("SELECT DISTINCT cwd FROM sessions WHERE transport = 'local'")
                .unwrap();
            stmt.query_map([], |row| row.get::<_, String>(0))
                .unwrap()
                .map(Result::unwrap)
                .collect()
        };
        assert_eq!(cwds, vec!["D:/work/Trae-sync"], "local 行 cwd 全部归并到规范值");
        // cloud 行不碰。
        let cloud_cwd: String = conn
            .query_row(
                "SELECT cwd FROM sessions WHERE id = 'c1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(cloud_cwd, "D:/work/Trae-sync");
        // b1 正文迁到规范目录。
        assert!(projects_path(temp.path())
            .join(escape_cwd_dir("D:/work/Trae-sync"))
            .join("b1.jsonl")
            .is_file());
    }

    #[test]
    fn merge_cwd_rejects_invalid_params() {
        let temp = tempfile::tempdir().unwrap();
        fixture_db(temp.path());
        // 规范 cwd 在变体集合中：拒绝。
        assert_eq!(
            merge_cwd_projects(temp.path(), "D:/w", &["D:/w".to_string()]).unwrap_err(),
            WorkBuddyLibraryError::MergeInvalid
        );
        // 空变体：拒绝。
        assert_eq!(
            merge_cwd_projects(temp.path(), "D:/w", &[]).unwrap_err(),
            WorkBuddyLibraryError::MergeInvalid
        );
        // 空 cwd：拒绝。
        assert_eq!(
            merge_cwd_projects(temp.path(), " ", &["D:/x".to_string()]).unwrap_err(),
            WorkBuddyLibraryError::MergeInvalid
        );
    }

    // ---- 体检 ----

    #[test]
    fn checkup_reports_orphans_missing_bodies_foreign_dirs_and_merge_candidates() {
        let temp = tempfile::tempdir().unwrap();
        {
            let conn = Connection::open(&fixture_db(temp.path())).unwrap();
            insert_session(&conn, "s1", UID_A, "D:/work/Trae-sync", "local", None, 10);
            insert_session(&conn, "s2", UID_A, "D:/work/Trae-sync", "local", Some(1), 11);
            insert_session(&conn, "s3", UID_B, "D:/work/trae-sync", "local", None, 20);
        }
        write_body(temp.path(), "D:/work/Trae-sync", "s1", "{}");
        write_body(temp.path(), "D:/work/trae-sync", "s3", "{}");
        // s2 缺正文；孤儿文件（索引无 o1 行）；外部目录（未被引用）。
        write_body(temp.path(), "D:/work/Trae-sync", "o1", "{}");
        let foreign = projects_path(temp.path()).join("not-a-project");
        std::fs::create_dir_all(&foreign).unwrap();
        std::fs::write(foreign.join("ghost.jsonl"), b"{}").unwrap();

        let report = read_library_checkup(temp.path()).unwrap();
        assert_eq!(report.soft_deleted_session_count, 1);
        assert_eq!(report.missing_body_count, 1, "s2 local 行缺正文");
        assert_eq!(report.missing_body_samples, vec!["s2".to_string()]);
        assert!(report
            .orphan_body_files
            .contains(&format!("{}/o1.jsonl", escape_cwd_dir("D:/work/Trae-sync"))));
        assert!(report.foreign_dirs.contains(&"not-a-project".to_string()));
        assert!(report
            .orphan_body_files
            .contains(&"not-a-project/ghost.jsonl".to_string()));
        // 归并候选：两个转义名仅大小写不同。
        assert_eq!(
            report.cwd_merge_candidates,
            vec![vec![
                escape_cwd_dir("D:/work/Trae-sync"),
                escape_cwd_dir("D:/work/trae-sync")
            ]]
        );
    }

    // ---- schema 漂移防御 ----

    #[test]
    fn write_paths_reject_drifted_schema() {
        let temp = tempfile::tempdir().unwrap();
        let db = db_path(temp.path());
        let conn = Connection::open(&db).unwrap();
        conn.execute_batch(
            "CREATE TABLE sessions (id TEXT, cwd TEXT, user_id TEXT, deleted_at INTEGER);",
        )
        .unwrap();
        drop(conn);
        assert_eq!(
            soft_delete_sessions(temp.path(), &["s1".into()], UID_A, 1).unwrap_err(),
            WorkBuddyLibraryError::SchemaInvalid
        );
        assert_eq!(
            purge_sessions(temp.path(), &["s1".into()], UID_A).unwrap_err(),
            WorkBuddyLibraryError::SchemaInvalid
        );
    }

    #[test]
    fn missing_db_reports_explicit_error() {
        let temp = tempfile::tempdir().unwrap();
        assert_eq!(
            read_library_stats(temp.path()).unwrap_err(),
            WorkBuddyLibraryError::DbMissing
        );
        assert_eq!(
            list_sessions(temp.path(), None, false).unwrap_err(),
            WorkBuddyLibraryError::DbMissing
        );
        assert_eq!(
            read_library_checkup(temp.path()).unwrap_err(),
            WorkBuddyLibraryError::DbMissing
        );
    }
}
