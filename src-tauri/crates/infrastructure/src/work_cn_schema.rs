//! Work CN schema 探测：固定兼容性判断与 schema 指纹计算。
//!
//! 对应 `TECHNICAL_BASELINE.md` 已验证账号归属模型与关键唯一约束。
//! 启动时必须重新校验关键表、列、索引和唯一约束——任何未知或不兼容项
//! 使该数据位置只读（规格第 10 节）。
//!
//! R5 修复：索引/唯一约束检查在 T02 启动兼容检查中完成，不推迟到 T06。
//! 缺少索引或唯一约束时返回已有结构化不兼容原因并保持只读。
//!
//! 本模块只做纯逻辑判断，不打开数据库连接。由 `sqlcipher` 模块调用。

use rusqlite::Connection;
use sha2::{Digest, Sha256};
use traesync_domain::{IncompatibleReason, SchemaFingerprint, TableCounts};

/// Work CN 必需表
const REQUIRED_TABLES: &[&str] = &["project", "chat_session", "chat_message"];

/// Work CN 必需列（表 -> 列列表）
const REQUIRED_COLUMNS: &[(&str, &[&str])] = &[
    ("project", &["project_id", "user_id", "biz_project_id"]),
    ("chat_session", &["session_id", "project_id"]),
    ("chat_message", &["message_id", "session_id"]),
];

/// R5：Work CN 必需唯一约束（表 -> 约束名列表）。
///
/// 对应 TECHNICAL_BASELINE.md 关键唯一约束：
/// - project.project_id UNIQUE
/// - project.(biz_project_id, user_id) UNIQUE
/// - chat_session.session_id UNIQUE
/// - chat_message.message_id UNIQUE
///
/// 通过 `PRAGMA index_list` 检查 origin='u'（UNIQUE 约束）或 origin='pk'（PRIMARY KEY）。
const REQUIRED_UNIQUE_CONSTRAINTS: &[(&str, &[&str])] = &[
    (
        "project",
        &["sqlite_autoindex_project_1", "sqlite_autoindex_project_2"],
    ),
    ("chat_session", &["sqlite_autoindex_chat_session_1"]),
    ("chat_message", &["sqlite_autoindex_chat_message_1"]),
];

/// 检查 schema 兼容性：返回 Ok(()) 或 Err(IncompatibleReason)。
///
/// R5：顺序：表存在 -> 列存在 -> 唯一约束存在。
/// 索引/唯一约束检查在 T02 启动兼容检查中完成，不推迟到 T06。
/// 缺少唯一约束时返回 `MissingConstraint` 结构化原因并保持只读。
pub fn check_schema(conn: &Connection) -> Result<(), IncompatibleReason> {
    // 1. 检查必需表
    let mut missing_tables = Vec::new();
    for table in REQUIRED_TABLES {
        if !table_exists(conn, table) {
            missing_tables.push((*table).to_string());
        }
    }
    if !missing_tables.is_empty() {
        return Err(IncompatibleReason::UnknownSchema { missing_tables });
    }

    // 2. 检查必需列
    for (table, columns) in REQUIRED_COLUMNS {
        for col in *columns {
            if !column_exists(conn, table, col) {
                return Err(IncompatibleReason::MissingColumn {
                    table: (*table).to_string(),
                    column: (*col).to_string(),
                });
            }
        }
    }

    // 3. R5：检查必需唯一约束（不推迟到 T06）
    for (table, constraint_names) in REQUIRED_UNIQUE_CONSTRAINTS {
        for constraint_name in *constraint_names {
            if !unique_constraint_exists(conn, table, constraint_name) {
                return Err(IncompatibleReason::MissingConstraint {
                    table: (*table).to_string(),
                    constraint: (*constraint_name).to_string(),
                });
            }
        }
    }

    Ok(())
}

/// 判断表是否存在
fn table_exists(conn: &Connection, table: &str) -> bool {
    let mut stmt =
        match conn.prepare("SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1 LIMIT 1") {
            Ok(s) => s,
            Err(_) => return false,
        };
    let exists: Option<i64> = stmt.query_row(rusqlite::params![table], |_| Ok(1)).ok();
    exists.is_some()
}

/// 判断列是否存在
fn column_exists(conn: &Connection, table: &str, column: &str) -> bool {
    // PRAGMA table_info 返回列信息
    let pragma = format!("PRAGMA table_info({})", table);
    let mut stmt = match conn.prepare(&pragma) {
        Ok(s) => s,
        Err(_) => return false,
    };
    let rows = match stmt.query_map([], |row| {
        let name: String = row.get(1)?;
        Ok(name)
    }) {
        Ok(r) => r,
        Err(_) => return false,
    };
    for row in rows {
        if let Ok(name) = row {
            if name == column {
                return true;
            }
        }
    }
    false
}

/// R5：判断唯一约束是否存在。
///
/// 通过 `PRAGMA index_list(<table>)` 查询表的索引列表：
/// - origin='u' 表示 UNIQUE 约束自动生成的索引
/// - origin='pk' 表示 PRIMARY KEY 约束自动生成的索引
///
/// SQLite 为 PRIMARY KEY 和 UNIQUE 约束自动创建名为 `sqlite_autoindex_<table>_<n>` 的索引。
/// 检查该索引存在且 origin 为 'u' 或 'pk' 即可确认约束存在。
fn unique_constraint_exists(conn: &Connection, table: &str, constraint_name: &str) -> bool {
    let pragma = format!("PRAGMA index_list({})", table);
    let mut stmt = match conn.prepare(&pragma) {
        Ok(s) => s,
        Err(_) => return false,
    };
    // PRAGMA index_list 列：seq, name, unique, origin, partial
    let rows = match stmt.query_map([], |row| {
        let name: String = row.get(1)?;
        let origin: String = row.get(3).unwrap_or_default();
        Ok((name, origin))
    }) {
        Ok(r) => r,
        Err(_) => return false,
    };
    for row in rows {
        if let Ok((name, origin)) = row {
            if name == constraint_name && (origin == "u" || origin == "pk") {
                return true;
            }
        }
    }
    false
}

/// 计算 schema 指纹：SHA-256 hex，绑定全部表的 DDL。
///
/// 不同 SQLite 行顺序下确定——按表名排序后哈希。
pub fn compute_schema_fingerprint(conn: &Connection) -> SchemaFingerprint {
    let mut stmt = match conn
        .prepare("SELECT name, sql FROM sqlite_master WHERE type='table' ORDER BY name ASC")
    {
        Ok(s) => s,
        Err(_) => {
            return SchemaFingerprint(
                "0000000000000000000000000000000000000000000000000000000000000000".to_string(),
            );
        }
    };
    let rows = stmt.query_map([], |row| {
        let name: String = row.get(0)?;
        let sql: String = row.get(1)?;
        Ok(format!("{name}|{sql}"))
    });

    let mut hasher = Sha256::new();
    if let Ok(rows) = rows {
        for row in rows {
            if let Ok(text) = row {
                hasher.update(text.as_bytes());
                hasher.update(b"\n");
            }
        }
    }
    SchemaFingerprint(hex::encode(hasher.finalize()))
}

/// 读取关键表行数：用于证明只读打开的副本与研究基线一致。
pub fn read_table_counts(conn: &Connection) -> TableCounts {
    TableCounts {
        project_count: count_rows(conn, "project"),
        chat_session_count: count_rows(conn, "chat_session"),
        chat_message_count: count_rows(conn, "chat_message"),
    }
}

fn count_rows(conn: &Connection, table: &str) -> u64 {
    let sql = format!("SELECT COUNT(*) FROM {}", table);
    conn.query_row(&sql, [], |row| row.get::<_, i64>(0))
        .map(|n| n as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    /// 构造合成 Work CN schema 数据库（明文 SQLite，用于纯 schema 测试）
    fn make_work_cn_schema_db(path: &std::path::Path) -> Connection {
        let conn = Connection::open(path).unwrap();
        conn.execute_batch(
            r#"
            CREATE TABLE project (
                project_id TEXT PRIMARY KEY,
                user_id TEXT NOT NULL,
                biz_project_id TEXT NOT NULL,
                UNIQUE (biz_project_id, user_id)
            );
            CREATE TABLE chat_session (
                session_id TEXT PRIMARY KEY,
                project_id TEXT NOT NULL
            );
            CREATE TABLE chat_message (
                message_id TEXT PRIMARY KEY,
                session_id TEXT NOT NULL
            );
            INSERT INTO project VALUES ('p1', '1000000000000001', 'biz-1');
            INSERT INTO chat_session VALUES ('s1', 'p1');
            INSERT INTO chat_message VALUES ('m1', 's1');
            "#,
        )
        .unwrap();
        conn
    }

    #[test]
    fn check_schema_accepts_valid_work_cn_db() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        let conn = make_work_cn_schema_db(&db_path);
        let result = check_schema(&conn);
        assert!(result.is_ok());
    }

    #[test]
    fn check_schema_rejects_missing_table() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        let conn = Connection::open(&db_path).unwrap();
        conn.execute_batch("CREATE TABLE project (project_id TEXT);")
            .unwrap();
        let err = check_schema(&conn).unwrap_err();
        match err {
            IncompatibleReason::UnknownSchema { missing_tables } => {
                assert!(missing_tables.contains(&"chat_session".to_string()));
                assert!(missing_tables.contains(&"chat_message".to_string()));
            }
            _ => panic!("期望 UnknownSchema，实际 {:?}", err),
        }
    }

    #[test]
    fn check_schema_rejects_missing_column() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        let conn = Connection::open(&db_path).unwrap();
        // project 缺 user_id 列
        conn.execute_batch("CREATE TABLE project (project_id TEXT);")
            .unwrap();
        conn.execute_batch("CREATE TABLE chat_session (session_id TEXT, project_id TEXT);")
            .unwrap();
        conn.execute_batch("CREATE TABLE chat_message (message_id TEXT, session_id TEXT);")
            .unwrap();
        let err = check_schema(&conn).unwrap_err();
        match err {
            IncompatibleReason::MissingColumn { table, column } => {
                assert_eq!(table, "project");
                assert_eq!(column, "user_id");
            }
            _ => panic!("期望 MissingColumn，实际 {:?}", err),
        }
    }

    #[test]
    fn schema_fingerprint_deterministic_for_same_schema() {
        let dir1 = tempfile::tempdir().unwrap();
        let dir2 = tempfile::tempdir().unwrap();
        let conn1 = make_work_cn_schema_db(&dir1.path().join("a.db"));
        let conn2 = make_work_cn_schema_db(&dir2.path().join("b.db"));
        let fp1 = compute_schema_fingerprint(&conn1);
        let fp2 = compute_schema_fingerprint(&conn2);
        assert_eq!(fp1, fp2);
        assert_ne!(
            fp1.0,
            "0000000000000000000000000000000000000000000000000000000000000000"
        );
    }

    #[test]
    fn schema_fingerprint_changes_with_different_schema() {
        let dir1 = tempfile::tempdir().unwrap();
        let dir2 = tempfile::tempdir().unwrap();
        let conn1 = make_work_cn_schema_db(&dir1.path().join("a.db"));
        let conn2 = {
            let c = Connection::open(&dir2.path().join("b.db")).unwrap();
            c.execute_batch(
                r#"
                CREATE TABLE project (project_id TEXT PRIMARY KEY, user_id TEXT);
                CREATE TABLE chat_session (session_id TEXT, project_id TEXT);
                CREATE TABLE chat_message (message_id TEXT, session_id TEXT);
                "#,
            )
            .unwrap();
            c
        };
        let fp1 = compute_schema_fingerprint(&conn1);
        let fp2 = compute_schema_fingerprint(&conn2);
        assert_ne!(fp1, fp2);
    }

    #[test]
    fn read_table_counts_returns_correct_counts() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        let conn = make_work_cn_schema_db(&db_path);
        let counts = read_table_counts(&conn);
        assert_eq!(counts.project_count, 1);
        assert_eq!(counts.chat_session_count, 1);
        assert_eq!(counts.chat_message_count, 1);
    }
}
