//! Work CN 来源 normalizer：从快照 DB 读取并规范化项目/会话/消息。
//!
//! 实现 `SourceNormalizer` port，将 Work CN 原始表（project/chat_session/chat_message）
//! 映射为 P1 历史库的规范化值对象。
//!
//! 安全约束：
//! - raw_key 在构造时注入，不出现在任何方法签名、日志或返回值中
//! - 每次调用以只读方式打开快照目录下的 database.db，不写入
//! - 表缺失或查询失败时保守返回空 Vec/None，不 panic
//!
//! 打开策略：
//! - 先用 raw_key 以 SQLCipher 方式打开（PRAGMA key），适用于生产加密快照
//! - 若读取失败（明文 DB 或 key 不匹配），回退到明文只读打开
//! - 这使生产加密快照与测试明文 fixture 都能被读取

use std::path::Path;

use rusqlite::{Connection, OpenFlags};
use traesync_domain::{
    ContentGraphHash, MessageProjection, ProjectIdentity, SessionIdentity, SessionProjection,
};
use traesync_ports::{ContentGraphHasher, SourceNormalizer};

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
    fn open_snapshot_db(&self, snapshot_dir: &Path) -> Option<Connection> {
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
                    return Some(conn);
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
                return Some(conn);
            }
        }

        None
    }
}

impl SourceNormalizer for WorkCnSourceNormalizer {
    fn read_projects(&self, snapshot_dir: &Path) -> Vec<ProjectIdentity> {
        let conn = match self.open_snapshot_db(snapshot_dir) {
            Some(c) => c,
            None => return Vec::new(),
        };
        // R6：读取全部项目（含软删除），用 deleted_at 标记 soft_deleted。
        // 软删除项目作为证据保留进入 catalog，但 browse/search/count 排除。
        let mut stmt = match conn
            .prepare("SELECT project_id, biz_project_id, user_id, deleted_at FROM project")
        {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };
        let rows = match stmt.query_map([], |row| {
            let project_id: String = row.get(0)?;
            let biz_project_id: String = row.get(1)?;
            let _user_id: String = row.get(2)?;
            let deleted_at: i64 = row.get(3).unwrap_or(0);
            // display_name 优先用 biz_project_id，为空时回退 project_id
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
        }) {
            Ok(r) => r,
            Err(_) => return Vec::new(),
        };
        rows.filter_map(|r| r.ok()).collect()
    }

    fn read_session_projections(&self, snapshot_dir: &Path) -> Vec<SessionProjection> {
        let conn = match self.open_snapshot_db(snapshot_dir) {
            Some(c) => c,
            None => return Vec::new(),
        };
        let mut stmt =
            match conn.prepare("SELECT session_id, project_id, deleted_at FROM chat_session") {
                Ok(s) => s,
                Err(_) => return Vec::new(),
            };
        let rows = match stmt.query_map([], |row| {
            let session_id: String = row.get(0)?;
            let project_id: String = row.get(1)?;
            let deleted_at: i64 = row.get(2).unwrap_or(0);
            Ok(SessionProjection {
                session_identity: SessionIdentity::new("work_cn", &session_id),
                // 快照阶段尚未计算活跃内容图哈希，留空由 catalog 填充
                active_content_graph_hash: ContentGraphHash(String::new()),
                // chat_session 无 title 列，留空
                active_title: String::new(),
                soft_deleted: deleted_at != 0,
                project_id,
            })
        }) {
            Ok(r) => r,
            Err(_) => return Vec::new(),
        };
        rows.filter_map(|r| r.ok()).collect()
    }

    fn read_messages(&self, snapshot_dir: &Path) -> Vec<MessageProjection> {
        let conn = match self.open_snapshot_db(snapshot_dir) {
            Some(c) => c,
            None => return Vec::new(),
        };
        // R4：检测 chat_message 表是否有 turn_id 列
        let has_turn_id = column_exists(&conn, "chat_message", "turn_id");

        // R4：统一使用 6 列 SQL（无 turn_id 列时用 NULL as turn_id），保证闭包类型一致
        // 不再截断 content——存储完整内容用于内容图哈希
        let sql = if has_turn_id {
            "SELECT message_id, session_id, role, content, deleted_at, turn_id \
             FROM chat_message ORDER BY message_id ASC"
        } else {
            "SELECT message_id, session_id, role, content, deleted_at, NULL AS turn_id \
             FROM chat_message ORDER BY message_id ASC"
        };
        let mut stmt = match conn.prepare(sql) {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };
        let rows = match stmt.query_map([], |row| {
            let message_id: String = row.get(0)?;
            let session_id: String = row.get(1)?;
            let role: String = row.get(2).unwrap_or_default();
            let content: String = row.get(3).unwrap_or_default();
            let deleted_at: i64 = row.get(4).unwrap_or(0);
            let turn_id: Option<String> = row.get(5).ok();
            Ok((
                message_id,
                session_id,
                role,
                content,
                deleted_at != 0,
                turn_id,
            ))
        }) {
            Ok(r) => r,
            Err(_) => return Vec::new(),
        };
        // seq 按 message_id 排序后递增
        let mut seq: u64 = 0;
        rows.filter_map(|r| {
            r.ok().map(
                |(message_id, session_id, role, content, soft_deleted, turn_id)| {
                    let m = MessageProjection {
                        message_id,
                        session_id,
                        role,
                        // R4：存储完整内容，不截断
                        content_excerpt: content,
                        soft_deleted,
                        seq,
                        turn_id,
                    };
                    seq += 1;
                    m
                },
            )
        })
        .collect()
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
