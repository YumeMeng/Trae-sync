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
/// 约束可由 PRIMARY KEY、UNIQUE 约束或命名 UNIQUE INDEX 提供；索引名不稳定，不能作为契约。
const REQUIRED_UNIQUE_CONSTRAINTS: &[(&str, &[&[&str]])] = &[
    (
        "project",
        &[&["project_id"], &["biz_project_id", "user_id"]],
    ),
    ("chat_session", &[&["session_id"]]),
    ("chat_message", &[&["message_id"]]),
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

    // 3. R5：检查必需唯一列组合（不推迟到 T06）
    for (table, column_sets) in REQUIRED_UNIQUE_CONSTRAINTS {
        for columns in *column_sets {
            if !unique_constraint_exists(conn, table, columns) {
                return Err(IncompatibleReason::MissingConstraint {
                    table: (*table).to_string(),
                    constraint: columns.join(","),
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

/// 检查是否存在覆盖指定列组合的唯一索引。
///
/// `PRAGMA index_list` 返回 `unique=1` 的索引时，无论来源是主键、UNIQUE 约束还是命名索引，
/// 都能提供相同的写入安全保证。
fn unique_constraint_exists(conn: &Connection, table: &str, required_columns: &[&str]) -> bool {
    let pragma = format!("PRAGMA index_list({})", table);
    let mut stmt = match conn.prepare(&pragma) {
        Ok(s) => s,
        Err(_) => return false,
    };
    // PRAGMA index_list 列：seq, name, unique, origin, partial。
    let rows = match stmt.query_map([], |row| {
        let name: String = row.get(1)?;
        let is_unique: i64 = row.get(2)?;
        let is_partial: i64 = row.get(4)?;
        Ok((name, is_unique, is_partial))
    }) {
        Ok(r) => r,
        Err(_) => return false,
    };
    for row in rows {
        if let Ok((name, is_unique, is_partial)) = row {
            if is_unique != 0
                && is_partial == 0
                && index_columns_match(conn, &name, required_columns)
            {
                return true;
            }
        }
    }
    false
}

/// 比较索引列顺序，避免把包含额外列的唯一索引误判为目标约束。
fn index_columns_match(conn: &Connection, index_name: &str, required_columns: &[&str]) -> bool {
    let escaped_index_name = index_name.replace('\'', "''");
    let pragma = format!("PRAGMA index_info('{escaped_index_name}')");
    let mut stmt = match conn.prepare(&pragma) {
        Ok(s) => s,
        Err(_) => return false,
    };
    let rows = match stmt.query_map([], |row| row.get::<_, String>(2)) {
        Ok(r) => r,
        Err(_) => return false,
    };
    let columns = rows.filter_map(Result::ok).collect::<Vec<_>>();
    columns
        .iter()
        .map(String::as_str)
        .eq(required_columns.iter().copied())
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

    /// 唯一性属于列组合契约，不依赖 SQLite 自动索引名称。
    #[test]
    fn check_schema_accepts_required_unique_columns_with_named_indexes() {
        let dir = tempfile::tempdir().unwrap();
        let conn = Connection::open(dir.path().join("named-indexes.db")).unwrap();
        conn.execute_batch(
            r#"
            CREATE TABLE project (
                id INTEGER PRIMARY KEY,
                project_id TEXT NOT NULL,
                user_id TEXT NOT NULL,
                biz_project_id TEXT NOT NULL
            );
            CREATE UNIQUE INDEX project_project_id_unique ON project(project_id);
            CREATE UNIQUE INDEX project_biz_user_unique ON project(biz_project_id, user_id);
            CREATE TABLE chat_session (
                id INTEGER PRIMARY KEY,
                session_id TEXT NOT NULL,
                project_id TEXT NOT NULL
            );
            CREATE UNIQUE INDEX chat_session_id_unique ON chat_session(session_id);
            CREATE TABLE chat_message (
                id INTEGER PRIMARY KEY,
                message_id TEXT NOT NULL,
                session_id TEXT NOT NULL
            );
            CREATE UNIQUE INDEX chat_message_id_unique ON chat_message(message_id);
            "#,
        )
        .unwrap();

        assert!(check_schema(&conn).is_ok());
    }

    #[test]
    fn check_schema_rejects_partial_unique_indexes() {
        let dir = tempfile::tempdir().unwrap();
        let conn = Connection::open(dir.path().join("partial-indexes.db")).unwrap();
        conn.execute_batch(
            r#"
            CREATE TABLE project (
                project_id TEXT NOT NULL,
                user_id TEXT NOT NULL,
                biz_project_id TEXT NOT NULL
            );
            CREATE UNIQUE INDEX project_project_id_partial ON project(project_id)
                WHERE project_id IS NOT NULL;
            CREATE UNIQUE INDEX project_biz_user_partial ON project(biz_project_id, user_id)
                WHERE biz_project_id IS NOT NULL AND user_id IS NOT NULL;
            CREATE TABLE chat_session (
                session_id TEXT NOT NULL,
                project_id TEXT NOT NULL
            );
            CREATE UNIQUE INDEX chat_session_id_partial ON chat_session(session_id)
                WHERE session_id IS NOT NULL;
            CREATE TABLE chat_message (
                message_id TEXT NOT NULL,
                session_id TEXT NOT NULL
            );
            CREATE UNIQUE INDEX chat_message_id_partial ON chat_message(message_id)
                WHERE message_id IS NOT NULL;
            "#,
        )
        .unwrap();

        assert!(matches!(
            check_schema(&conn),
            Err(IncompatibleReason::MissingConstraint { table, constraint })
                if table == "project" && constraint == "project_id"
        ));
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
