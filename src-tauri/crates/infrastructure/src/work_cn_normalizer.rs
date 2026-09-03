//! Work CN 来源 normalizer：从快照 DB 读取并规范化项目/会话/消息。
//!
//! 实现 `SourceNormalizer` port，将 Work CN 原始表（project/chat_session/chat_message）
//! 映射为 P1 历史库的规范化值对象。
//!
//! 安全约束：
//! - raw_key 在构造时注入，不出现在任何方法签名、日志或返回值中
//! - 每次调用以只读方式打开快照目录下的 database.db，不写入
//! - 旧 fixture 读取方法保守返回空 Vec/None；checked 快照读取方法遇到错误直接失败
//!
//! 打开策略：
//! - 先用 raw_key 以 SQLCipher 方式打开（PRAGMA key），适用于生产加密快照
//! - 若读取失败（明文 DB 或 key 不匹配），回退到明文只读打开
//! - 这使生产加密快照与测试明文 fixture 都能被读取

use std::{collections::HashMap, path::Path};

use rusqlite::{Connection, OpenFlags};
use serde_json::Value;
use traesync_domain::{
    ContentGraphHash, MessageProjection, ProjectIdentity, SessionIdentity, SessionProjection,
};
use traesync_ports::{ContentGraphHasher, NormalizedSnapshot, SourceNormalizer, SourceReadError};

use crate::content_graph::DeterministicContentGraphHasher;

/// R4：检测 SQLite 表中是否存在指定列。
/// 用于条件性读取 turn_id 等可选关系列。
fn column_exists(conn: &Connection, table: &str, column: &str) -> bool {
    let sql = format!("PRAGMA table_info({})", table);
    let Ok(mut stmt) = conn.prepare(&sql) else {
        return false;
    };
    let names: Vec<String> = match stmt.query_map([], |row| {
        let name: String = row.get(1)?;
        Ok(name)
    }) {
        Ok(rows) => rows.filter_map(|r| r.ok()).collect(),
        Err(_) => return false,
    };
    names.iter().any(|name| name == column)
}

/// 从 TRAE 的 JSON 字段中提取可展示文本；优先使用语义明确的字段。
fn extract_serialized_text(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return String::new();
    }

    let Ok(value) = serde_json::from_str::<Value>(trimmed) else {
        return trimmed.to_string();
    };
    if let Some(query) = value.get("query").and_then(Value::as_str) {
        return query.to_string();
    }
    extract_nested_text(&value).unwrap_or_else(|| trimmed.to_string())
}

/// 兼容正文块、工具输出和历史消息的常见 JSON 结构。
fn extract_nested_text(value: &Value) -> Option<String> {
    match value {
        Value::String(text) if !text.is_empty() => Some(text.clone()),
        Value::Array(items) => {
            let parts: Vec<String> = items.iter().filter_map(extract_nested_text).collect();
            (!parts.is_empty()).then(|| parts.join("\n"))
        }
        Value::Object(object) => {
            for key in ["content", "text", "message", "output", "summary", "query"] {
                if let Some(text) = object.get(key).and_then(extract_nested_text) {
                    return Some(text);
                }
            }
            object.values().find_map(extract_nested_text)
        }
        _ => None,
    }
}

/// 将来源字段压缩为适合列表展示的短标题。
///
/// 标题只用于浏览，不参与会话身份或内容图哈希；过长正文只保留前 96 个字符。
fn clean_display_title(raw: &str) -> String {
    let extracted = extract_serialized_text(raw);
    extracted
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(96)
        .collect()
}

/// Work CN 来源 normalizer。
///
/// 持有 SQLCipher raw key（私有），实现 `SourceNormalizer` port。
pub struct WorkCnSourceNormalizer {
    /// SQLCipher raw key（64 字符 hex），私有，不通过方法暴露。
    raw_key: String,
}

impl WorkCnSourceNormalizer {
    pub fn new(raw_key: String) -> Self {
        Self { raw_key }
    }

    /// 以只读方式打开快照目录下的 database.db。
    ///
    /// 先尝试用 raw_key 打开（生产加密快照）；若解密读取失败则回退明文只读打开
    /// （测试 fixture 或明文快照）。任一方式打开后能读到 sqlite_master 即视为成功。
    fn open_snapshot_db_checked(&self, snapshot_dir: &Path) -> Result<Connection, SourceReadError> {
        let db_path = snapshot_dir.join("database.db");
        let flags = OpenFlags::SQLITE_OPEN_READ_ONLY
            | OpenFlags::SQLITE_OPEN_PRIVATE_CACHE
            | OpenFlags::SQLITE_OPEN_NO_MUTEX;

        // 1. 尝试用 raw_key 打开（生产加密快照）
        if let Ok(conn) = Connection::open_with_flags(&db_path, flags) {
            // raw key 语法：x'<hex>' —— 不进入日志
            let pragma = format!("PRAGMA key = \"x'{}'\";", self.raw_key);
            if conn.execute_batch(&pragma).is_ok() {
                // 触发解密：读取 sqlite_master 验证 key 正确
                if conn
                    .query_row("SELECT 1 FROM sqlite_master LIMIT 1", [], |_| Ok(()))
                    .is_ok()
                {
                    return Ok(conn);
                }
            }
            // key 打开失败，conn 在此 drop
        }

        // 2. 回退：明文只读打开（测试 fixture 或明文快照）
        if let Ok(conn) = Connection::open_with_flags(&db_path, flags) {
            if conn
                .query_row("SELECT 1 FROM sqlite_master LIMIT 1", [], |_| Ok(()))
                .is_ok()
            {
                return Ok(conn);
            }
        }

        Err(SourceReadError::Unavailable)
    }

    /// 兼容旧 port 方法：checked 读取失败时返回空集合。
    fn open_snapshot_db(&self, snapshot_dir: &Path) -> Option<Connection> {
        self.open_snapshot_db_checked(snapshot_dir).ok()
    }

    fn read_projects_checked(conn: &Connection) -> Result<Vec<ProjectIdentity>, SourceReadError> {
        let mut statement = conn
            .prepare("SELECT project_id, biz_project_id, user_id, deleted_at FROM project")
            .map_err(|_| SourceReadError::Unavailable)?;
        let rows = statement
            .query_map([], |row| {
                let project_id: String = row.get(0)?;
                let biz_project_id: String = row.get(1)?;
                let _user_id: String = row.get(2)?;
                let deleted_at: i64 = row.get::<_, Option<i64>>(3)?.unwrap_or(0);
                // 项目显示名沿用已验证的业务标识契约；易变 title 列不参与规范化。
                let display_name = if biz_project_id.is_empty() {
                    project_id.clone()
                } else {
                    biz_project_id.clone()
                };
                Ok(ProjectIdentity {
                    project_id,
                    biz_project_id,
                    display_name,
                    soft_deleted: deleted_at != 0,
                })
            })
            .map_err(|_| SourceReadError::Unavailable)?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|_| SourceReadError::Unavailable)
    }

    fn read_sessions_checked(conn: &Connection) -> Result<Vec<SessionProjection>, SourceReadError> {
        let titles = read_session_display_titles(conn);
        let mut statement = conn
            .prepare("SELECT session_id, project_id, deleted_at FROM chat_session")
            .map_err(|_| SourceReadError::Unavailable)?;
        let rows = statement
            .query_map([], |row| {
                let session_id: String = row.get(0)?;
                let project_id: String = row.get(1)?;
                let deleted_at: i64 = row.get::<_, Option<i64>>(2)?.unwrap_or(0);
                Ok(SessionProjection {
                    session_identity: SessionIdentity::new("work_cn", &session_id),
                    active_content_graph_hash: ContentGraphHash(String::new()),
                    active_title: titles.get(&session_id).cloned().unwrap_or_default(),
                    soft_deleted: deleted_at != 0,
                    project_id,
                })
            })
            .map_err(|_| SourceReadError::Unavailable)?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|_| SourceReadError::Unavailable)
    }

    fn read_messages_checked(
        conn: &Connection,
        require_known_session: bool,
    ) -> Result<Vec<MessageProjection>, SourceReadError> {
        let raw_messages = if column_exists(conn, "chat_message", "message_role") {
            let general_content = read_general_message_content_checked(conn)?;
            let order = if column_exists(conn, "chat_message", "message_index") {
                "ORDER BY m.session_id ASC, m.message_index ASC, m.message_id ASC"
            } else {
                "ORDER BY m.session_id ASC, m.message_id ASC"
            };
            let session_filter = if require_known_session {
                " WHERE EXISTS (SELECT 1 FROM chat_session s WHERE s.session_id = m.session_id)"
            } else {
                ""
            };
            let sql = format!(
                "SELECT m.message_id, m.session_id, m.message_role, m.user_message_context, m.deleted_at, NULL AS turn_id FROM chat_message m{session_filter} {order}"
            );
            let mut statement = conn
                .prepare(&sql)
                .map_err(|_| SourceReadError::Unavailable)?;
            let rows = statement
                .query_map([], |row| {
                    let message_id: String = row.get(0)?;
                    let session_id: String = row.get(1)?;
                    let role: String = row.get(2)?;
                    let context: Option<String> = row.get(3)?;
                    let deleted_at: i64 = row.get::<_, Option<i64>>(4)?.unwrap_or(0);
                    let content = if role == "user" {
                        extract_serialized_text(context.as_deref().unwrap_or_default())
                    } else {
                        general_content
                            .get(&message_id)
                            .cloned()
                            .unwrap_or_else(|| {
                                extract_serialized_text(context.as_deref().unwrap_or_default())
                            })
                    };
                    Ok((message_id, session_id, role, content, deleted_at != 0, None))
                })
                .map_err(|_| SourceReadError::Unavailable)?;
            rows.collect::<Result<Vec<_>, _>>()
                .map_err(|_| SourceReadError::Unavailable)?
        } else {
            let has_turn_id = column_exists(conn, "chat_message", "turn_id");
            let session_filter = if require_known_session {
                " WHERE EXISTS (SELECT 1 FROM chat_session s WHERE s.session_id = m.session_id)"
            } else {
                ""
            };
            let sql = if has_turn_id {
                format!(
                    "SELECT m.message_id, m.session_id, m.role, m.content, m.deleted_at, m.turn_id \
                     FROM chat_message m{session_filter} ORDER BY m.message_id ASC"
                )
            } else {
                format!(
                    "SELECT m.message_id, m.session_id, m.role, m.content, m.deleted_at, NULL AS turn_id \
                     FROM chat_message m{session_filter} ORDER BY m.message_id ASC"
                )
            };
            let mut statement = conn
                .prepare(&sql)
                .map_err(|_| SourceReadError::Unavailable)?;
            let rows = statement
                .query_map([], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, Option<String>>(3)?.unwrap_or_default(),
                        row.get::<_, Option<i64>>(4)?.unwrap_or(0) != 0,
                        row.get::<_, Option<String>>(5)?,
                    ))
                })
                .map_err(|_| SourceReadError::Unavailable)?;
            rows.collect::<Result<Vec<_>, _>>()
                .map_err(|_| SourceReadError::Unavailable)?
        };

        Ok(raw_messages
            .into_iter()
            .enumerate()
            .map(
                |(seq, (message_id, session_id, role, content, soft_deleted, turn_id))| {
                    MessageProjection {
                        message_id,
                        session_id,
                        role,
                        content_excerpt: content,
                        soft_deleted,
                        seq: seq as u64,
                        turn_id,
                    }
                },
            )
            .collect())
    }
}

/// 读取可选会话标题；缺少标题列时用首条用户消息作为安全回退。
///
/// 这是展示信息，不改变 `SessionIdentity`，也不把 session_id 拼进标题。
fn read_session_display_titles(conn: &Connection) -> HashMap<String, String> {
    let mut titles = HashMap::new();

    // 来源表没有可靠稳定的标题契约，复用统一消息解析逻辑提取首条用户消息。
    let mut first_user_message = HashMap::new();
    let mut first_message = HashMap::new();
    for message in WorkCnSourceNormalizer::read_messages_checked(conn, true)
        .unwrap_or_default()
        .into_iter()
        .filter(|message| !message.soft_deleted && !message.content_excerpt.trim().is_empty())
    {
        let title = clean_display_title(&message.content_excerpt);
        if title.is_empty() {
            continue;
        }
        first_message
            .entry(message.session_id.clone())
            .or_insert_with(|| title.clone());
        if message.role.eq_ignore_ascii_case("user") {
            first_user_message
                .entry(message.session_id)
                .or_insert(title);
        }
    }

    for (session_id, title) in first_user_message.into_iter().chain(first_message) {
        titles.entry(session_id).or_insert(title);
    }
    titles
}

/// 读取可选的 assistant 正文表；表不存在时允许回退到 chat_message 上下文。
fn read_general_message_content_checked(
    conn: &Connection,
) -> Result<HashMap<String, String>, SourceReadError> {
    if !column_exists(conn, "chat_message_general", "message_id")
        || !column_exists(conn, "chat_message_general", "content")
    {
        return Ok(HashMap::new());
    }

    let has_deleted_at = column_exists(conn, "chat_message_general", "deleted_at");
    let sql = if has_deleted_at {
        "SELECT message_id, content, deleted_at FROM chat_message_general ORDER BY message_id ASC"
    } else {
        "SELECT message_id, content, 0 AS deleted_at FROM chat_message_general ORDER BY message_id ASC"
    };
    let mut statement = conn
        .prepare(sql)
        .map_err(|_| SourceReadError::Unavailable)?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Option<String>>(1)?.unwrap_or_default(),
                row.get::<_, Option<i64>>(2)?.unwrap_or(0),
            ))
        })
        .map_err(|_| SourceReadError::Unavailable)?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|_| SourceReadError::Unavailable)
        .map(|rows| {
            rows.into_iter()
                .filter(|(_, _, deleted_at)| *deleted_at == 0)
                .map(|(message_id, content, _)| (message_id, extract_serialized_text(&content)))
                .collect()
        })
}

impl SourceNormalizer for WorkCnSourceNormalizer {
    fn read_snapshot_checked(
        &self,
        snapshot_dir: &Path,
    ) -> Result<NormalizedSnapshot, SourceReadError> {
        let conn = self.open_snapshot_db_checked(snapshot_dir)?;
        Ok(NormalizedSnapshot {
            projects: Self::read_projects_checked(&conn)?,
            sessions: Self::read_sessions_checked(&conn)?,
            messages: Self::read_messages_checked(&conn, true)?,
        })
    }

    fn read_projects(&self, snapshot_dir: &Path) -> Vec<ProjectIdentity> {
        let conn = match self.open_snapshot_db(snapshot_dir) {
            Some(c) => c,
            None => return Vec::new(),
        };
        Self::read_projects_checked(&conn).unwrap_or_default()
    }

    fn read_session_projections(&self, snapshot_dir: &Path) -> Vec<SessionProjection> {
        let conn = match self.open_snapshot_db(snapshot_dir) {
            Some(c) => c,
            None => return Vec::new(),
        };
        Self::read_sessions_checked(&conn).unwrap_or_default()
    }

    fn read_messages(&self, snapshot_dir: &Path) -> Vec<MessageProjection> {
        let conn = match self.open_snapshot_db(snapshot_dir) {
            Some(c) => c,
            None => return Vec::new(),
        };
        Self::read_messages_checked(&conn, false).unwrap_or_default()
    }

    fn read_project_owner(&self, snapshot_dir: &Path, project_id: &str) -> Option<String> {
        let conn = self.open_snapshot_db(snapshot_dir)?;
        conn.query_row(
            "SELECT user_id FROM project WHERE project_id = ?1",
            rusqlite::params![project_id],
            |row| {
                let user_id: String = row.get(0)?;
                Ok(user_id)
            },
        )
        .ok()
    }

    fn compute_content_graph_hash(
        &self,
        snapshot_dir: &Path,
        session: &SessionIdentity,
    ) -> Option<ContentGraphHash> {
        // 读取快照全部消息，过滤出目标会话的消息
        let messages = self.read_messages(snapshot_dir);
        let session_messages: Vec<MessageProjection> = messages
            .into_iter()
            .filter(|m| m.session_id == session.original_session_id)
            .collect();
        if session_messages.is_empty() {
            return None;
        }
        Some(
            DeterministicContentGraphHasher::new().hash_session_content(&session_messages, session),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;
    use tempfile::tempdir;

    /// 测试用 raw_key（不接触真实 TRAE 数据，仅供 fixture 测试）
    const TEST_RAW_KEY: &str = "deadbeefcafebabe1234567890abcdefdeadbeefcafebabe1234567890abcdef";

    /// 构造明文合成 fixture DB（含 deleted_at 列，符合任务约定）
    fn make_plaintext_fixture(dir: &Path) -> std::path::PathBuf {
        let db_path = dir.join("database.db");
        let conn = Connection::open(&db_path).unwrap();
        conn.execute_batch(
            r#"
            CREATE TABLE project (
                project_id TEXT PRIMARY KEY,
                user_id TEXT NOT NULL,
                biz_project_id TEXT NOT NULL,
                deleted_at INTEGER DEFAULT 0
            );
            CREATE TABLE chat_session (
                session_id TEXT PRIMARY KEY,
                project_id TEXT NOT NULL,
                deleted_at INTEGER DEFAULT 0
            );
            CREATE TABLE chat_message (
                message_id TEXT PRIMARY KEY,
                session_id TEXT NOT NULL,
                role TEXT,
                content TEXT,
                deleted_at INTEGER DEFAULT 0
            );
            INSERT INTO project VALUES ('p1', 'user-A', 'biz-1', 0);
            INSERT INTO project VALUES ('p2', 'user-B', 'biz-2', 100);
            INSERT INTO chat_session VALUES ('s1', 'p1', 0);
            INSERT INTO chat_session VALUES ('s2', 'p1', 200);
            INSERT INTO chat_message VALUES ('m1', 's1', 'user', 'hello world', 0);
            INSERT INTO chat_message VALUES ('m2', 's1', 'assistant', 'hi there', 0);
            INSERT INTO chat_message VALUES ('m3', 's1', 'user', 'deleted msg', 300);
            "#,
        )
        .unwrap();
        drop(conn);
        db_path
    }

    #[test]
    fn r6_read_projects_includes_soft_deleted_with_flag() {
        let dir = tempdir().unwrap();
        make_plaintext_fixture(dir.path());
        let normalizer = WorkCnSourceNormalizer::new(TEST_RAW_KEY.to_string());
        let projects = normalizer.read_projects(dir.path());
        // R6：全部项目都返回（含软删除 p2），用 soft_deleted 标记区分
        assert_eq!(projects.len(), 2, "应返回全部 2 个项目（含软删除）");
        let p1 = projects
            .iter()
            .find(|p| p.project_id == "p1")
            .expect("应有 p1");
        let p2 = projects
            .iter()
            .find(|p| p.project_id == "p2")
            .expect("应有 p2");
        assert!(!p1.soft_deleted, "p1 未删除");
        assert!(p2.soft_deleted, "p2 应标记为软删除");
        // display_name 用 biz_project_id
        assert_eq!(p1.display_name, "biz-1");
    }

    #[test]
    fn read_session_projections_marks_soft_deleted() {
        let dir = tempdir().unwrap();
        make_plaintext_fixture(dir.path());
        let normalizer = WorkCnSourceNormalizer::new(TEST_RAW_KEY.to_string());
        let sessions = normalizer.read_session_projections(dir.path());
        assert_eq!(sessions.len(), 2);
        // s1 未删除，s2 软删除
        let s1 = sessions
            .iter()
            .find(|s| s.session_identity.original_session_id == "s1");
        let s2 = sessions
            .iter()
            .find(|s| s.session_identity.original_session_id == "s2");
        assert!(s1.is_some());
        assert!(s2.is_some());
        assert!(!s1.unwrap().soft_deleted);
        assert!(s2.unwrap().soft_deleted);
        // product_history_namespace 固定 work_cn
        assert_eq!(
            s1.unwrap().session_identity.product_history_namespace,
            "work_cn"
        );
    }

    #[test]
    fn read_messages_marks_soft_deleted() {
        let dir = tempdir().unwrap();
        make_plaintext_fixture(dir.path());
        let normalizer = WorkCnSourceNormalizer::new(TEST_RAW_KEY.to_string());
        let messages = normalizer.read_messages(dir.path());
        assert_eq!(messages.len(), 3);
        // 按 message_id 排序：m1, m2, m3
        assert_eq!(messages[0].message_id, "m1");
        assert_eq!(messages[1].message_id, "m2");
        assert_eq!(messages[2].message_id, "m3");
        // seq 递增
        assert_eq!(messages[0].seq, 0);
        assert_eq!(messages[1].seq, 1);
        assert_eq!(messages[2].seq, 2);
        // m3 软删除
        assert!(!messages[0].soft_deleted);
        assert!(!messages[1].soft_deleted);
        assert!(messages[2].soft_deleted);
        // content_excerpt 来自 content
        assert_eq!(messages[0].content_excerpt, "hello world");
    }

    #[test]
    fn checked_snapshot_read_ignores_rows_without_a_known_session() {
        let dir = tempdir().unwrap();
        make_plaintext_fixture(dir.path());
        {
            let conn = Connection::open(dir.path().join("database.db")).unwrap();
            conn.execute(
                "INSERT INTO chat_message VALUES ('m-orphan', 'missing-session', 'user', 'orphan', 0)",
                [],
            )
            .unwrap();
        }

        let normalizer = WorkCnSourceNormalizer::new(TEST_RAW_KEY.to_string());
        let messages = normalizer
            .read_snapshot_checked(dir.path())
            .expect("快照应能完成严格规范化读取")
            .messages;

        // 不能把没有会话上下文的残留消息发布到历史投影。
        assert_eq!(messages.len(), 3);
        assert!(messages
            .iter()
            .all(|message| message.session_id != "missing-session"));
    }

    #[test]
    fn reads_real_work_cn_message_and_general_content() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("database.db");
        {
            let conn = Connection::open(&db_path).unwrap();
            conn.execute_batch(
                r#"
                CREATE TABLE chat_message (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    session_id TEXT NOT NULL,
                    message_id TEXT NOT NULL,
                    message_type TEXT NOT NULL,
                    message_role TEXT NOT NULL,
                    message_index INTEGER NOT NULL,
                    user_message_context TEXT NOT NULL,
                    deleted_at INTEGER NOT NULL DEFAULT 0
                );
                CREATE TABLE chat_message_general (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    message_id TEXT NOT NULL,
                    content TEXT NOT NULL,
                    deleted_at INTEGER NOT NULL DEFAULT 0
                );
                INSERT INTO chat_message
                    (session_id, message_id, message_type, message_role, message_index, user_message_context)
                VALUES
                    ('s1', 'm-user', 'text', 'user', 0, '{"query":"hello user"}'),
                    ('s1', 'm-assistant', 'text', 'assistant', 1, '{}'),
                    ('s1', 'm-deleted', 'text', 'user', 2, '{"query":"deleted user"}');
                INSERT INTO chat_message_general (message_id, content)
                VALUES ('m-assistant', 'hello assistant');
                UPDATE chat_message SET deleted_at = 1 WHERE message_id = 'm-deleted';
                "#,
            )
            .unwrap();
        }

        let normalizer = WorkCnSourceNormalizer::new(TEST_RAW_KEY.to_string());
        let messages = normalizer.read_messages(dir.path());
        assert_eq!(messages.len(), 3);
        assert_eq!(messages[0].content_excerpt, "hello user");
        assert_eq!(messages[1].content_excerpt, "hello assistant");
        assert!(messages[2].soft_deleted);
        assert_eq!(messages[0].role, "user");
        assert_eq!(messages[1].role, "assistant");
    }

    #[test]
    fn read_project_owner_returns_user_id() {
        let dir = tempdir().unwrap();
        make_plaintext_fixture(dir.path());
        let normalizer = WorkCnSourceNormalizer::new(TEST_RAW_KEY.to_string());
        let owner = normalizer.read_project_owner(dir.path(), "p1");
        assert_eq!(owner.as_deref(), Some("user-A"));
        // 不存在的项目返回 None
        assert!(normalizer.read_project_owner(dir.path(), "nope").is_none());
    }

    #[test]
    fn compute_content_graph_hash_is_deterministic() {
        let dir = tempdir().unwrap();
        make_plaintext_fixture(dir.path());
        let normalizer = WorkCnSourceNormalizer::new(TEST_RAW_KEY.to_string());
        let session = SessionIdentity::new("work_cn", "s1");
        let h1 = normalizer.compute_content_graph_hash(dir.path(), &session);
        let h2 = normalizer.compute_content_graph_hash(dir.path(), &session);
        assert!(h1.is_some());
        assert_eq!(h1, h2);
        // 哈希非空
        assert!(!h1.unwrap().as_str().is_empty());
        // 不存在的会话返回 None
        let missing = SessionIdentity::new("work_cn", "no-such-session");
        assert!(normalizer
            .compute_content_graph_hash(dir.path(), &missing)
            .is_none());
    }

    #[test]
    fn volatile_columns_cache_fts_and_new_tables_do_not_change_normalized_graph() {
        let dir = tempdir().unwrap();
        make_plaintext_fixture(dir.path());
        let normalizer = WorkCnSourceNormalizer::new(TEST_RAW_KEY.to_string());
        let session = SessionIdentity::new("work_cn", "s1");

        let projects_before = normalizer.read_projects(dir.path());
        let sessions_before = normalizer.read_session_projections(dir.path());
        let messages_before = normalizer.read_messages(dir.path());
        let graph_hash_before = normalizer
            .compute_content_graph_hash(dir.path(), &session)
            .expect("fixture 应生成 s1 内容图哈希");

        {
            let conn = Connection::open(dir.path().join("database.db")).unwrap();
            conn.execute_batch(
                r#"
                ALTER TABLE project ADD COLUMN updated_at INTEGER DEFAULT 0;
                ALTER TABLE project ADD COLUMN title TEXT DEFAULT '';
                ALTER TABLE chat_session ADD COLUMN updated_at INTEGER DEFAULT 0;
                ALTER TABLE chat_session ADD COLUMN title TEXT DEFAULT '';
                ALTER TABLE chat_message ADD COLUMN updated_at INTEGER DEFAULT 0;
                ALTER TABLE chat_message ADD COLUMN cache TEXT DEFAULT '';
                ALTER TABLE chat_message ADD COLUMN derived TEXT DEFAULT '';
                UPDATE project SET updated_at = 99, title = 'volatile project';
                UPDATE chat_session SET updated_at = 98, title = 'volatile session';
                UPDATE chat_message SET updated_at = 97, cache = 'cached', derived = 'derived';
                CREATE TABLE message_cache (message_id TEXT PRIMARY KEY, payload TEXT NOT NULL);
                INSERT INTO message_cache VALUES ('m1', 'cache payload');
                CREATE VIRTUAL TABLE message_fts USING fts5(message_id UNINDEXED, content);
                INSERT INTO message_fts VALUES ('m1', 'fts payload');
                "#,
            )
            .unwrap();
        }

        let projects_after = normalizer.read_projects(dir.path());
        let sessions_after = normalizer.read_session_projections(dir.path());
        let messages_after = normalizer.read_messages(dir.path());
        let graph_hash_after = normalizer
            .compute_content_graph_hash(dir.path(), &session)
            .expect("新增易变表后仍应生成 s1 内容图哈希");

        assert_eq!(projects_after, projects_before);
        assert_eq!(sessions_after, sessions_before);
        assert_eq!(messages_after, messages_before);
        assert_eq!(graph_hash_after, graph_hash_before);
    }

    #[test]
    fn missing_tables_return_empty_vec() {
        let dir = tempdir().unwrap();
        // 空 DB（无 project/chat_session/chat_message 表）
        let db_path = dir.path().join("database.db");
        {
            let conn = Connection::open(&db_path).unwrap();
            conn.execute_batch("CREATE TABLE other (id INTEGER);")
                .unwrap();
        }
        let normalizer = WorkCnSourceNormalizer::new(TEST_RAW_KEY.to_string());
        // 表不存在时保守返回空 Vec，不 panic
        assert!(normalizer.read_projects(dir.path()).is_empty());
        assert!(normalizer.read_session_projections(dir.path()).is_empty());
        assert!(normalizer.read_messages(dir.path()).is_empty());
        assert!(normalizer.read_project_owner(dir.path(), "p1").is_none());
    }

    #[test]
    fn checked_snapshot_read_rejects_missing_tables() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("database.db");
        Connection::open(&db_path)
            .unwrap()
            .execute_batch("CREATE TABLE project (project_id TEXT, user_id TEXT, biz_project_id TEXT, deleted_at INTEGER);")
            .unwrap();

        let normalizer = WorkCnSourceNormalizer::new(TEST_RAW_KEY.to_string());
        assert_eq!(
            normalizer.read_snapshot_checked(dir.path()),
            Err(SourceReadError::Unavailable)
        );
    }

    #[test]
    fn r4_content_not_truncated_full_content_preserved() {
        // R4：内容不再截断为 500 字符——完整内容被保留
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("database.db");
        {
            let conn = Connection::open(&db_path).unwrap();
            conn.execute_batch(
                r#"
                CREATE TABLE project (project_id TEXT PRIMARY KEY, user_id TEXT, biz_project_id TEXT, deleted_at INTEGER DEFAULT 0);
                CREATE TABLE chat_session (session_id TEXT PRIMARY KEY, project_id TEXT, deleted_at INTEGER DEFAULT 0);
                CREATE TABLE chat_message (message_id TEXT PRIMARY KEY, session_id TEXT, role TEXT, content TEXT, deleted_at INTEGER DEFAULT 0);
                "#,
            ).unwrap();
            // 插入一条超长消息（1000 字符）
            let long: String = "a".repeat(1000);
            conn.execute(
                "INSERT INTO chat_message VALUES ('m1', 's1', 'user', ?1, 0)",
                [&long],
            )
            .unwrap();
        }
        let normalizer = WorkCnSourceNormalizer::new(TEST_RAW_KEY.to_string());
        let messages = normalizer.read_messages(dir.path());
        assert_eq!(messages.len(), 1);
        // R4：content_excerpt 保留完整内容（1000 字符），不再截断为 500
        assert_eq!(messages[0].content_excerpt.chars().count(), 1000);
        // turn_id 为 None（表无此列）
        assert_eq!(messages[0].turn_id, None);
    }

    #[test]
    fn r4_turn_id_read_when_column_exists() {
        // R4：当 chat_message 表有 turn_id 列时，读取 turn_id
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("database.db");
        {
            let conn = Connection::open(&db_path).unwrap();
            conn.execute_batch(
                r#"
                CREATE TABLE project (project_id TEXT PRIMARY KEY, user_id TEXT, biz_project_id TEXT, deleted_at INTEGER DEFAULT 0);
                CREATE TABLE chat_session (session_id TEXT PRIMARY KEY, project_id TEXT, deleted_at INTEGER DEFAULT 0);
                CREATE TABLE chat_message (message_id TEXT PRIMARY KEY, session_id TEXT, role TEXT, content TEXT, deleted_at INTEGER DEFAULT 0, turn_id TEXT);
                INSERT INTO chat_message VALUES ('m1', 's1', 'user', 'hello', 0, 'turn-1');
                INSERT INTO chat_message VALUES ('m2', 's1', 'assistant', 'hi', 0, NULL);
                "#,
            ).unwrap();
        }
        let normalizer = WorkCnSourceNormalizer::new(TEST_RAW_KEY.to_string());
        let messages = normalizer.read_messages(dir.path());
        assert_eq!(messages.len(), 2);
        // m1 有 turn_id
        assert_eq!(messages[0].turn_id.as_deref(), Some("turn-1"));
        // m2 turn_id 为 NULL -> None
        assert_eq!(messages[1].turn_id, None);
    }
}
