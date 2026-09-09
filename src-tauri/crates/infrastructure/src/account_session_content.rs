//! P3-2 会话内容读取：单会话消息流（元数据 + 按类型分表的内容解析）。
//!
//! 依据 2026-08-24 真实库探测（原生主库 713MB 实测 692 user + 692 task 消息）：
//! - `chat_message` 是元数据表（role/type/created_at/软删），正文按 message_type 分表：
//!   - general / chat → `chat_message_{general,chat}`，content 是 JSON 块数组
//!     `[{"type":"text","text_content":"…"}]`；
//!   - task → `chat_message_task`，content 是执行轨迹
//!     `{"task_id","messages":[{"plan_item":{"thought":"…",…}}]}`。
//! - 只读打开复用 P3-1 的隔离三件套机制（WAL 兼容，不阻塞运行中实例）。

use std::path::Path;

use rusqlite::Connection;

use crate::account_session_index::instance_database_path;
use crate::sqlcipher::open_with_key_readonly;

/// 单条消息内容（按类型解析后的阅览形态）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionMessageContent {
    /// 文本消息（user 提问等）：内容块 text_content 按序拼接。
    Text(String),
    /// 任务执行轨迹（assistant 回复）：步骤数 + 各步 thought 摘要。
    TaskTrace {
        step_count: u32,
        thoughts: Vec<String>,
    },
}

/// 单条会话消息（元数据 + 解析后内容）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionMessageEntry {
    pub message_id: String,
    /// "user" / "assistant"（透传源库值，未知角色原样返回由前端兜底显示）。
    pub role: String,
    /// "general" / "task" / "chat"（决定内容解析路径）。
    pub message_type: String,
    pub created_at_unix_seconds: Option<i64>,
    pub content: SessionMessageContent,
}

/// 单会话消息读取结果（与 P3-1 索引状态语义一致）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionMessagesStatus {
    Ready(Vec<SessionMessageEntry>),
    /// 实例从未启动过（无 data_dir 或无 database.db）。
    NoInstanceData,
    /// 打开或读取失败（key 不匹配、文件损坏、表缺失等）。
    ReadFailed,
}

/// 单会话消息读取上限：防御异常巨型会话拖垮一次性返回。
pub const MAX_MESSAGES_PER_SESSION: usize = 2000;

/// 读取单个账号单个会话的消息流。
///
/// 按 message_type LEFT JOIN 三张内容表（缺表或无对应行时内容为空文本，
/// 不整体失败）；返回按 created_at 升序、rowid 兜底排序（旧数据行
/// created_at 可能为 NULL，NULL 在 ASC 中排最前即最早）。
/// 超长会话只取最近 `MAX_MESSAGES_PER_SESSION` 条（保尾部最新内容）。
pub fn read_account_session_messages(
    storage_root: &Path,
    profile_id: &str,
    session_id: &str,
    raw_key: &str,
) -> SessionMessagesStatus {
    let Some(db_path) = instance_database_path(storage_root, profile_id) else {
        return SessionMessagesStatus::NoInstanceData;
    };
    if !db_path.exists() {
        return SessionMessagesStatus::NoInstanceData;
    }
    let conn = match open_with_key_readonly(&db_path, raw_key) {
        Ok(conn) => conn,
        Err(_) => return SessionMessagesStatus::ReadFailed,
    };
    match read_session_messages(&conn, session_id) {
        Ok(messages) => SessionMessagesStatus::Ready(messages),
        Err(_) => SessionMessagesStatus::ReadFailed,
    }
}

/// 读取主库单个会话的消息流（P5-3 历史页预览数据源）。
///
/// 与账号实例读取共用解析路径（元数据 LEFT JOIN 三张内容表）；主库
/// database.db 缺失 = `NoInstanceData`（主库从未启动过），其余失败 =
/// `ReadFailed`。通过隔离三件套副本只读打开，主库实例运行中可随时读取。
pub fn read_master_session_messages(
    master_data_dir: &Path,
    session_id: &str,
    raw_key: &str,
    current_user_id: &str,
) -> SessionMessagesStatus {
    read_master_session_messages_page(master_data_dir, session_id, raw_key, 0, current_user_id)
}

/// 读取主库单个会话的一页消息；`offset` 用于向上滚动时继续加载更早消息。
pub fn read_master_session_messages_page(
    master_data_dir: &Path,
    session_id: &str,
    raw_key: &str,
    offset: usize,
    current_user_id: &str,
) -> SessionMessagesStatus {
    let db_path = crate::master_handover::master_database_path(master_data_dir);
    if !db_path.exists() {
        return SessionMessagesStatus::NoInstanceData;
    }
    let conn = match open_with_key_readonly(&db_path, raw_key) {
        Ok(conn) => conn,
        Err(_) => return SessionMessagesStatus::ReadFailed,
    };
    match read_session_messages_page(&conn, session_id, offset, Some(current_user_id)) {
        Ok(messages) => SessionMessagesStatus::Ready(messages),
        Err(_) => SessionMessagesStatus::ReadFailed,
    }
}

/// 会话消息查询：元数据 LEFT JOIN 三张内容表，一次往返取全。
fn read_session_messages(
    conn: &Connection,
    session_id: &str,
) -> Result<Vec<SessionMessageEntry>, ()> {
    read_session_messages_page(conn, session_id, 0, None)
}

/// 按最新消息倒序分页查询，再反转为时间正序供前端拼接。
fn read_session_messages_page(
    conn: &Connection,
    session_id: &str,
    offset: usize,
    current_user_id: Option<&str>,
) -> Result<Vec<SessionMessageEntry>, ()> {
    // 内容表存在性防御：TRAE schema 演进时缺表降级为空内容而非整体失败。
    // 缺表时 JOIN 与列都要以 NULL 占位：SELECT 列数必须恒定，否则行映射
    // 读固定下标会列越界、整体误报 ReadFailed。
    let has_general = table_exists(conn, "chat_message_general")?;
    let has_task = table_exists(conn, "chat_message_task")?;
    let has_chat = table_exists(conn, "chat_message_chat")?;
    let (general_join, general_col) = if has_general {
        (
            "LEFT JOIN (SELECT message_id, MAX(content) AS content \
             FROM chat_message_general GROUP BY message_id) g \
             ON g.message_id = page.message_id",
            "g.content",
        )
    } else {
        ("", "NULL")
    };
    let (task_join, task_col) = if has_task {
        (
            "LEFT JOIN (SELECT message_id, MAX(content) AS content \
             FROM chat_message_task GROUP BY message_id) t \
             ON t.message_id = page.message_id",
            "t.content",
        )
    } else {
        ("", "NULL")
    };
    let (chat_join, chat_col) = if has_chat {
        (
            "LEFT JOIN (SELECT message_id, MAX(content) AS content \
             FROM chat_message_chat GROUP BY message_id) ch \
             ON ch.message_id = page.message_id",
            "ch.content",
        )
    } else {
        ("", "NULL")
    };
    // 先对消息元数据分页，再连接内容表：内容表 message_id 非唯一时，不能
    // 让 JOIN 放大行数并改变 LIMIT/OFFSET 的消息边界。offset 由后端内部生成，
    // 不来自 SQL 字符串外部输入，避免为分页引入参数类型变化。
    let (owner_join, owner_filter) = if current_user_id.is_some() {
        (
            "JOIN chat_session s ON s.session_id = m.session_id \
             JOIN project p ON p.project_id = s.project_id",
            " AND p.user_id = ?2",
        )
    } else {
        ("", "")
    };
    let sql = format!(
        "WITH message_page AS ( \
           SELECT m.rowid AS message_rowid, m.message_id, m.message_role, \
                  m.message_type, m.created_at \
           FROM chat_message m {owner_join} \
           WHERE m.session_id = ?1 AND (m.deleted_at IS NULL OR m.deleted_at = 0) \
                 {owner_filter} \
           ORDER BY m.created_at DESC, m.rowid DESC \
           LIMIT {MAX_MESSAGES_PER_SESSION} OFFSET {offset} \
         ) \
         SELECT page.message_id, page.message_role, page.message_type, page.created_at, \
                {general_col}, {task_col}, {chat_col} \
         FROM message_page page {general_join} {task_join} {chat_join} \
         ORDER BY page.created_at DESC, page.message_rowid DESC"
    );
    let mut statement = conn.prepare(&sql).map_err(|_| ())?;
    let mut params = vec![session_id];
    if let Some(current_user_id) = current_user_id {
        params.push(current_user_id);
    }
    let rows = statement
        .query_map(rusqlite::params_from_iter(params), |row| {
            let message_id: String = row.get(0)?;
            let role: String = row.get(1)?;
            let message_type: String = row.get(2)?;
            let created_at: Option<i64> = row.get(3)?;
            let general_content: Option<String> = row.get(4)?;
            let task_content: Option<String> = row.get(5)?;
            let chat_content: Option<String> = row.get(6)?;
            let content = resolve_message_content(
                &message_type,
                general_content.as_deref(),
                task_content.as_deref(),
                chat_content.as_deref(),
            );
            Ok(SessionMessageEntry {
                message_id,
                role,
                message_type,
                created_at_unix_seconds: created_at,
                content,
            })
        })
        .map_err(|_| ())?;
    let mut messages: Vec<SessionMessageEntry> =
        rows.collect::<Result<Vec<_>, _>>().map_err(|_| ())?;
    // 反转为时间升序（对话时间线阅览方向）。
    messages.reverse();
    // 内容表与历史数据可能出现重复 message_id，保留这一层防御以兼容旧库；
    // 正常情况下 SQL 聚合已经保证重复内容不会影响分页边界。
    let mut seen = std::collections::HashSet::new();
    messages.retain(|entry| seen.insert(entry.message_id.clone()));
    Ok(messages)
}

/// 按消息类型选择内容解析路径；指定内容缺失时按文本消息空串降级
/// （元数据存在而行内容缺失属于数据演进期形态，不视为失败）。
fn resolve_message_content(
    message_type: &str,
    general_content: Option<&str>,
    task_content: Option<&str>,
    chat_content: Option<&str>,
) -> SessionMessageContent {
    match message_type {
        "task" => task_content.map(parse_task_trace).unwrap_or(SessionMessageContent::Text(String::new())),
        "chat" => chat_content
            .map(parse_text_blocks)
            .map(SessionMessageContent::Text)
            .unwrap_or(SessionMessageContent::Text(String::new())),
        // general 及未知类型：优先 general 表，回退 chat 表（防御假设两者同形，
        // 未逐一验证历史 schema 版本）。
        _ => general_content
            .or(chat_content)
            .map(parse_text_blocks)
            .map(SessionMessageContent::Text)
            .unwrap_or(SessionMessageContent::Text(String::new())),
    }
}

/// 解析 general/chat 内容（JSON 块数组）为纯文本：提取 type=="text" 块的
/// text_content 按序拼接。非 JSON 内容按原样返回（防御降级：格式未识别时
/// 不丢弃正文）。
fn parse_text_blocks(content: &str) -> String {
    match serde_json::from_str::<serde_json::Value>(content) {
        Ok(serde_json::Value::Array(blocks)) => blocks
            .iter()
            .filter_map(|block| {
                if block.get("type").and_then(|t| t.as_str()) == Some("text") {
                    block
                        .get("text_content")
                        .and_then(|t| t.as_str())
                        .map(str::to_string)
                } else {
                    None
                }
            })
            .collect::<Vec<_>>()
            .join("\n"),
        _ => content.to_string(),
    }
}

/// 解析 task 执行轨迹：步骤数 + 各步 plan_item.thought 摘要。
/// 非 JSON 的轨迹内容按文本原样返回（不静默丢弃）。
fn parse_task_trace(content: &str) -> SessionMessageContent {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(content) else {
        return SessionMessageContent::Text(content.to_string());
    };
    let Some(steps) = value.get("messages").and_then(|m| m.as_array()) else {
        // 合法 JSON 但无 messages 数组（空任务/schema 演进）：按空轨迹处理。
        return SessionMessageContent::TaskTrace {
            step_count: 0,
            thoughts: Vec::new(),
        };
    };
    let thoughts = steps
        .iter()
        .filter_map(|step| {
            step.get("plan_item")
                .and_then(|p| p.get("thought"))
                .and_then(|t| t.as_str())
                .filter(|s| !s.trim().is_empty())
                .map(str::to_string)
        })
        .collect();
    SessionMessageContent::TaskTrace {
        step_count: steps.len() as u32,
        thoughts,
    }
}

/// 检测表是否存在（sqlite_master 只读查询）。
fn table_exists(conn: &Connection, table: &str) -> Result<bool, ()> {
    let sql = "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?1";
    conn.query_row(sql, [table], |row| row.get::<_, i64>(0))
        .map(|count| count > 0)
        .map_err(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::OpenFlags;

    /// 构造加密实例库（含一条会话：user 文本消息 + assistant 任务轨迹）。
    fn create_encrypted_db_with_messages(path: &std::path::Path, raw_key: &str, session_id: &str) {
        let flags = OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE;
        let conn = Connection::open_with_flags(path, flags).unwrap();
        let pragma = format!("PRAGMA key = \"x'{}'\";", raw_key);
        conn.execute_batch(&pragma).unwrap();
        conn.execute_batch(
            "CREATE TABLE chat_session (session_id TEXT PRIMARY KEY, project_id TEXT, session_title TEXT, updated_at INTEGER, deleted_at INTEGER);
             CREATE TABLE chat_message (id INTEGER, session_id TEXT, message_id TEXT, message_type TEXT, message_role TEXT, message_index INTEGER, is_archived INTEGER, reply_to_message_id TEXT, user_message_context TEXT, created_at bigint, updated_at bigint, deleted_at bigint, revertible INTEGER);
             CREATE TABLE chat_message_general (id INTEGER, message_id TEXT, content TEXT, created_at bigint, updated_at bigint, deleted_at bigint);
             CREATE TABLE chat_message_task (id INTEGER, message_id TEXT, task_id TEXT, content TEXT, summary TEXT, created_at bigint, updated_at bigint, deleted_at bigint);
             CREATE TABLE chat_message_chat (id INTEGER, message_id TEXT, content TEXT, created_at bigint, updated_at bigint, deleted_at bigint);",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO chat_session (session_id, project_id, session_title, updated_at, deleted_at) VALUES (?1, 'p1', '会话一', 1770000000000, NULL)",
            [session_id],
        )
        .unwrap();
        // user 消息：JSON 块数组（真实库实测格式）。
        conn.execute(
            "INSERT INTO chat_message (session_id, message_id, message_type, message_role, created_at, deleted_at) \
             VALUES (?1, 'm1', 'general', 'user', 1770000000, NULL)",
            [session_id],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO chat_message_general (message_id, content, deleted_at) VALUES ('m1', \
             '[{\"type\":\"text\",\"text_content\":\"检查一下账号\"},{\"type\":\"image\",\"url\":\"x\"},{\"type\":\"text\",\"text_content\":\"的状态\"}]', NULL)",
            [],
        )
        .unwrap();
        // assistant 消息：任务执行轨迹（真实库实测格式，thought 步骤摘要）。
        conn.execute(
            "INSERT INTO chat_message (session_id, message_id, message_type, message_role, created_at, deleted_at) \
             VALUES (?1, 'm2', 'task', 'assistant', 1770000100, NULL)",
            [session_id],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO chat_message_task (message_id, content, deleted_at) VALUES ('m2', \
             '{\"task_id\":\"t1\",\"messages\":[{\"type\":\"plan_item\",\"plan_item\":{\"thought\":\"先读配置\"}},{\"type\":\"plan_item\",\"plan_item\":{\"thought\":\"\"}},{\"type\":\"plan_item\",\"plan_item\":{\"thought\":\"再对比数据\"}}]}', NULL)",
            [],
        )
        .unwrap();
        // 软删消息：不返回。
        conn.execute(
            "INSERT INTO chat_message (session_id, message_id, message_type, message_role, created_at, deleted_at) \
             VALUES (?1, 'm3', 'general', 'user', 1770000200, 500)",
            [session_id],
        )
        .unwrap();
    }

    #[test]
    fn master_messages_are_filtered_by_project_owner() {
        let temp = std::env::temp_dir().join(format!(
            "trae-sync-p53-owner-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let db_path = crate::master_handover::master_database_path(&temp);
        std::fs::create_dir_all(db_path.parent().unwrap()).unwrap();
        let raw_key = "ad".repeat(32);
        create_encrypted_db_with_messages(&db_path, &raw_key, "s1");
        {
            let flags = OpenFlags::SQLITE_OPEN_READ_WRITE;
            let conn = Connection::open_with_flags(&db_path, flags).unwrap();
            conn.execute_batch(&format!("PRAGMA key = \"x'{raw_key}'\";"))
                .unwrap();
            conn.execute_batch(
                "CREATE TABLE project (project_id TEXT PRIMARY KEY, user_id TEXT);
                 INSERT INTO project VALUES ('p1', '111');
                 INSERT INTO project VALUES ('p2', '222');
                 INSERT INTO chat_session VALUES ('s2', 'p2', '他人会话', 1770000000000, NULL);
                 INSERT INTO chat_message (session_id, message_id, message_type, message_role, created_at, deleted_at)
                    VALUES ('s2', 'm4', 'general', 'user', 1770000300, NULL);
                 INSERT INTO chat_message_general (message_id, content, deleted_at)
                    VALUES ('m4', '[{\"type\":\"text\",\"text_content\":\"他人内容\"}]', NULL);",
            )
            .unwrap();
        }

        match read_master_session_messages_page(&temp, "s1", &raw_key, 0, "111") {
            SessionMessagesStatus::Ready(messages) => assert_eq!(messages.len(), 2),
            other => panic!("当前账号应能读取自己的会话，实际 {:?}", other),
        }
        match read_master_session_messages_page(&temp, "s1", &raw_key, 0, "222") {
            SessionMessagesStatus::Ready(messages) => assert!(messages.is_empty()),
            other => panic!("跨账号读取应返回空页，实际 {:?}", other),
        }
        match read_master_session_messages_page(&temp, "s2", &raw_key, 0, "111") {
            SessionMessagesStatus::Ready(messages) => assert!(messages.is_empty()),
            other => panic!("跨账号会话不应泄露消息，实际 {:?}", other),
        }
        let _ = std::fs::remove_dir_all(&temp);
    }

    #[test]
    fn oversized_session_keeps_latest_window_and_dedupes_joined_rows() {
        let temp = std::env::temp_dir().join(format!(
            "trae-sync-p32-latest-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let dir = temp
            .join("trae-instances")
            .join("checkin-a")
            .join("ModularData")
            .join("ai-agent");
        std::fs::create_dir_all(&dir).unwrap();
        let raw_key = "ab".repeat(32);
        {
            let flags = OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE;
            let conn = Connection::open_with_flags(dir.join("database.db"), flags).unwrap();
            let pragma = format!("PRAGMA key = \"x'{}'\";", raw_key);
            conn.execute_batch(&pragma).unwrap();
            conn.execute_batch(
                "CREATE TABLE chat_message (session_id TEXT, message_id TEXT, message_type TEXT, message_role TEXT, created_at bigint, deleted_at bigint);
                 CREATE TABLE chat_message_general (message_id TEXT, content TEXT, deleted_at bigint);",
            )
            .unwrap();
            // s1：2005 条消息，验证 DESC+LIMIT 取最近 2000 条且反转为升序。
            for i in 1..=2005i64 {
                conn.execute(
                    "INSERT INTO chat_message (session_id, message_id, message_type, message_role, created_at, deleted_at) VALUES ('s1', ?1, 'general', 'user', ?2, NULL)",
                    rusqlite::params![format!("m{}", i), i],
                )
                .unwrap();
            }
            // s2：单条消息在内容表有两行（message_id 无唯一约束），验证 JOIN 去重。
            conn.execute(
                "INSERT INTO chat_message (session_id, message_id, message_type, message_role, created_at, deleted_at) VALUES ('s2', 'dup', 'general', 'user', 100, NULL)",
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO chat_message_general (message_id, content, deleted_at) VALUES ('dup', '[{\"type\":\"text\",\"text_content\":\"行一\"}]', NULL), ('dup', '[{\"type\":\"text\",\"text_content\":\"行二\"}]', NULL)",
                [],
            )
            .unwrap();
            // m7 的重复内容用于验证内容表 JOIN 不会挤占元数据分页窗口。
            conn.execute(
                "INSERT INTO chat_message_general (message_id, content, deleted_at) VALUES ('m7', '[{\"type\":\"text\",\"text_content\":\"重复内容\"}]', NULL), ('m7', '[{\"type\":\"text\",\"text_content\":\"重复内容-2\"}]', NULL)",
                [],
            )
            .unwrap();
        }

        // s1：上限 2000，保留最新窗口 m6..=m2005（最早 5 条被截断），升序返回。
        match read_account_session_messages(&temp, "checkin-a", "s1", &raw_key) {
            SessionMessagesStatus::Ready(messages) => {
                assert_eq!(messages.len(), 2000);
                assert_eq!(messages.first().unwrap().message_id, "m6");
                assert_eq!(messages.last().unwrap().message_id, "m2005");
            }
            other => panic!("期望 Ready，实际 {:?}", other),
        }
        // 第二页继续取更早消息，供主库查看器向上滚动时拼接。
        let conn = open_with_key_readonly(&dir.join("database.db"), &raw_key).unwrap();
        match read_session_messages_page(&conn, "s1", MAX_MESSAGES_PER_SESSION, None) {
            Ok(messages) => {
                assert_eq!(messages.len(), 5);
                assert_eq!(messages.first().unwrap().message_id, "m1");
                assert_eq!(messages.last().unwrap().message_id, "m5");
            }
            Err(()) => panic!("第二页消息读取失败"),
        }
        // s2：JOIN 放大的重复行被去重为一条。
        match read_account_session_messages(&temp, "checkin-a", "s2", &raw_key) {
            SessionMessagesStatus::Ready(messages) => {
                assert_eq!(messages.len(), 1);
            }
            other => panic!("期望 Ready，实际 {:?}", other),
        }
        let _ = std::fs::remove_dir_all(&temp);
    }

    #[test]
    fn reads_user_text_and_task_trace_with_ordering() {
        let temp = std::env::temp_dir().join(format!(
            "trae-sync-p32-read-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let dir = temp
            .join("trae-instances")
            .join("checkin-a")
            .join("ModularData")
            .join("ai-agent");
        std::fs::create_dir_all(&dir).unwrap();
        let raw_key = "aa".repeat(32);
        create_encrypted_db_with_messages(&dir.join("database.db"), &raw_key, "s1");

        let status = read_account_session_messages(&temp, "checkin-a", "s1", &raw_key);
        match status {
            SessionMessagesStatus::Ready(messages) => {
                // 软删消息被过滤，剩 user + assistant 两条，按时间升序。
                assert_eq!(messages.len(), 2);
                assert_eq!(messages[0].message_id, "m1");
                assert_eq!(messages[0].role, "user");
                // 块数组提取：仅 text 块的 text_content，跳过非 text 块。
                assert_eq!(messages[0].content, SessionMessageContent::Text("检查一下账号\n的状态".to_string()));
                assert_eq!(messages[0].created_at_unix_seconds, Some(1770000000));

                assert_eq!(messages[1].message_id, "m2");
                assert_eq!(messages[1].role, "assistant");
                // 轨迹解析：3 步，空 thought 被过滤，非空 thought 按序保留。
                assert_eq!(
                    messages[1].content,
                    SessionMessageContent::TaskTrace {
                        step_count: 3,
                        thoughts: vec!["先读配置".to_string(), "再对比数据".to_string()],
                    }
                );
            }
            other => panic!("期望 Ready，实际 {:?}", other),
        }
        let _ = std::fs::remove_dir_all(&temp);
    }

    #[test]
    fn missing_instance_reports_no_instance_data() {
        let temp = std::env::temp_dir().join(format!(
            "trae-sync-p32-missing-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&temp).unwrap();
        let status = read_account_session_messages(&temp, "checkin-x", "s1", &"bb".repeat(32));
        assert_eq!(status, SessionMessagesStatus::NoInstanceData);
        let _ = std::fs::remove_dir_all(&temp);
    }

    #[test]
    fn wrong_key_reports_read_failed() {
        let temp = std::env::temp_dir().join(format!(
            "trae-sync-p32-wrongkey-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let dir = temp
            .join("trae-instances")
            .join("checkin-a")
            .join("ModularData")
            .join("ai-agent");
        std::fs::create_dir_all(&dir).unwrap();
        create_encrypted_db_with_messages(&dir.join("database.db"), &"cc".repeat(32), "s1");
        let status = read_account_session_messages(&temp, "checkin-a", "s1", &"dd".repeat(32));
        assert_eq!(status, SessionMessagesStatus::ReadFailed);
        let _ = std::fs::remove_dir_all(&temp);
    }

    #[test]
    fn non_json_task_content_falls_back_to_text() {
        // 非 JSON 轨迹内容不静默丢弃：按文本原样返回。
        assert_eq!(
            parse_task_trace("not-json"),
            SessionMessageContent::Text("not-json".to_string())
        );
        // 非 JSON 文本内容同样原样返回（旧 schema 兼容）。
        assert_eq!(parse_text_blocks("plain text"), "plain text");
        // 空轨迹 / 空块数组。
        assert_eq!(
            parse_task_trace("{}"),
            SessionMessageContent::TaskTrace { step_count: 0, thoughts: vec![] }
        );
        assert_eq!(parse_text_blocks("[]"), "");
    }
}
