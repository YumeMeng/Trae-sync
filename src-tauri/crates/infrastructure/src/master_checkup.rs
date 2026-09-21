//! P5-5 主库体检（只读）：记录分布 + 孤儿行 + 滞留账号识别。
//!
//! 数据源：`project.user_id` 聚合；体检只把未删除、未归档且仍有会话的
//! 正常记录视为可交接范围。归档会话属于 ADR-0028 的主库冻结内容，不进入
//! 滞留账号或收编候选。
//! 空项目诊断只统计未删除且无任何会话行的 project 行——用户在 TRAE 内
//! 删除过的项目残留（软删行）不计入；清理命令复用同一删除口径。
//! 注册比对由调用方传入注册表账号映射（user_id → 账号名），本模块不读
//! 注册表文件——保持只读主库单一职责。
//!
//! 收编安全口径：孤儿行（`user_id IS NULL`）只报告不收编——无归属行
//! 可能是 TRAE 自身维护的数据，改写归属缺乏依据（保守不动）。

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::master_archive::{MasterArchiveError, open_with_key_readwrite, verify_key_readable};
use crate::master_handover::master_database_path;
use crate::sqlcipher::open_with_key_readonly;

/// 注册表账号映射条目（调用方从 AccountRegistry 投影；user_id 即 account_id）。
#[derive(Debug, Clone)]
pub struct RegisteredAccount {
    pub user_id: String,
    /// 展示名（display_name 优先，回退 screen_name；调用方决定）。
    pub account_name: String,
}

/// 一个账号在主库内的可交接记录规模（体检行；不直接序列化，DTO 层补账号名）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckupAccountRow {
    /// TRAE user_id（技术标识；UI 层只用账号名/未登记表述，此值收进悬浮提示）。
    pub user_id: String,
    /// 含至少一个正常会话的项目数。
    pub project_count: u64,
    /// 未删除且未归档的正常会话数。
    pub session_count: u64,
    /// 空项目数（不含已删除项目；只报告，不收编）。
    pub empty_project_count: u64,
}

/// 体检结果（状态语义与 MasterStatsStatus 一致）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MasterCheckupStatus {
    Ready(MasterCheckup),
    /// 主库从未启动过（无 database.db）。
    NoMasterData,
    /// 打开或读取失败（key 不匹配、文件损坏等）。
    ReadFailed,
}

/// 主库体检报告（只读聚合）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MasterCheckup {
    /// 可交接范围内的账号分布（含当前账号；按正常会话数降序），附空项目诊断。
    pub accounts: Vec<CheckupAccountRow>,
    /// 无归属（user_id IS NULL）项目行数（只报告，收编不动）。
    pub orphan_project_count: u64,
    /// 挂在无归属项目行上的会话数。
    pub orphan_session_count: u64,
}

impl MasterCheckup {
    /// 滞留账号行（非当前账号且仍有正常会话的分布行——收编目标规模）。
    pub fn stale_rows(&self, current_user_id: &str) -> Vec<&CheckupAccountRow> {
        self.accounts
            .iter()
            .filter(|row| row.user_id != current_user_id && row.session_count > 0)
            .collect()
    }

    /// 全库空项目总数（清理命令预检口径，与删除范围一致）。
    pub fn empty_project_total(&self) -> u64 {
        self.accounts
            .iter()
            .map(|row| row.empty_project_count)
            .sum()
    }
}

/// 读取主库体检报告（只统计正常可交接记录，只读打开）。
///
/// `current_user_id` 只用于调用方后续的滞留过滤，本函数不接收它——
/// 分布是全库事实，与当前账号无关（不同账号视角看到同一张报告）。
pub fn read_master_checkup(master_data_dir: &Path, raw_key: &str) -> MasterCheckupStatus {
    let db_path = master_database_path(master_data_dir);
    if !db_path.is_file() {
        return MasterCheckupStatus::NoMasterData;
    }
    let Ok(conn) = open_with_key_readonly(&db_path, raw_key) else {
        return MasterCheckupStatus::ReadFailed;
    };
    read_checkup_inner(&conn).map_or(MasterCheckupStatus::ReadFailed, MasterCheckupStatus::Ready)
}

/// 读取可选列是否存在，兼容归档功能上线前的旧主库结构。
fn column_exists(conn: &rusqlite::Connection, table: &str, column: &str) -> bool {
    let Ok(mut statement) = conn.prepare(&format!("PRAGMA table_info({table})")) else {
        return false;
    };
    let Ok(rows) = statement.query_map([], |row| row.get::<_, String>(1)) else {
        return false;
    };
    let found = rows.filter_map(Result::ok).any(|name| name == column);
    found
}

/// 内层聚合：任一 SQL 失败整体 ReadFailed（分布数字半缺比没有更误导）。
fn read_checkup_inner(conn: &rusqlite::Connection) -> Option<MasterCheckup> {
    // 兼容旧库：没有归档列时按全部未删除会话处理；没有删除列时按未删除处理。
    let has_hidden_status = column_exists(conn, "chat_session", "hidden_status");
    let has_deleted_at = column_exists(conn, "chat_session", "deleted_at");
    // 项目删除列同样兼容旧库：空项目诊断不含已删除项目（软删残留行）。
    let has_project_deleted_at = column_exists(conn, "project", "deleted_at");
    let project_not_deleted_expr = if has_project_deleted_at {
        "COALESCE(p.deleted_at, 0) = 0"
    } else {
        "1 = 1"
    };
    let hidden_status_expr = if has_hidden_status {
        "COALESCE(s.hidden_status, '')"
    } else {
        "''"
    };
    let deleted_at_expr = if has_deleted_at {
        "COALESCE(s.deleted_at, 0)"
    } else {
        "0"
    };
    let normal_session_predicate = format!(
        "s.session_id IS NOT NULL AND {deleted_at_expr} = 0 AND {hidden_status_expr} != 'voice_discussion'"
    );

    // 账号分布：LEFT JOIN 保留历史空账号行；正常规模与空项目诊断分开统计。
    // 空项目 = 无任何会话行且未删除（软删残留不计入，与清理口径一致）。
    let account_sql = format!(
        "SELECT p.user_id,
                COUNT(DISTINCT CASE WHEN {normal_session_predicate} THEN p.project_id END),
                COUNT(DISTINCT CASE WHEN {normal_session_predicate} THEN s.session_id END),
                COUNT(DISTINCT CASE WHEN s.session_id IS NULL AND {project_not_deleted_expr} THEN p.project_id END)
         FROM project p
         LEFT JOIN chat_session s ON s.project_id = p.project_id
         WHERE p.user_id IS NOT NULL AND p.user_id != ''
         GROUP BY p.user_id
         ORDER BY COUNT(DISTINCT CASE WHEN {normal_session_predicate} THEN s.session_id END) DESC,
                  p.user_id"
    );
    let mut statement = conn.prepare(&account_sql).ok()?;
    let accounts: Vec<CheckupAccountRow> = statement
        .query_map([], |row| {
            Ok(CheckupAccountRow {
                user_id: row.get::<_, String>(0)?,
                project_count: row.get::<_, i64>(1)?.max(0) as u64,
                session_count: row.get::<_, i64>(2)?.max(0) as u64,
                empty_project_count: row.get::<_, i64>(3)?.max(0) as u64,
            })
        })
        .ok()?
        .filter_map(|row| row.ok())
        .filter(|row| !row.user_id.is_empty())
        .collect();

    // 孤儿行：无归属项目行数 + 挂在其上的会话数（只报告）。
    let orphan_project_count = conn
        .query_row(
            "SELECT COUNT(*) FROM project WHERE user_id IS NULL",
            [],
            |row| row.get::<_, i64>(0),
        )
        .ok()?
        .max(0) as u64;
    let orphan_session_count = conn
        .query_row(
            "SELECT COUNT(*) FROM chat_session WHERE project_id IN
             (SELECT project_id FROM project WHERE user_id IS NULL)",
            [],
            |row| row.get::<_, i64>(0),
        )
        .ok()?
        .max(0) as u64;

    Some(MasterCheckup {
        accounts,
        orphan_project_count,
        orphan_session_count,
    })
}

/// 空项目删除范围 SQL 片段（`project` 表上下文；与体检空项目口径一致）。
///
/// 范围：user_id 非空、未删除（旧库无 deleted_at 列时全部视为未删除）、
/// 且在 chat_session 中没有任何行（归档/软删会话也算存在——有会话行的
/// 项目一律不清理）。孤儿行（user_id IS NULL）不在范围内，保持只报告
/// 不处理（保守口径）。
fn empty_project_scope(has_deleted_at: bool) -> String {
    let not_deleted = if has_deleted_at {
        "COALESCE(deleted_at, 0) = 0"
    } else {
        "1 = 1"
    };
    format!(
        "user_id IS NOT NULL AND user_id != '' AND {not_deleted} \
         AND NOT EXISTS (SELECT 1 FROM chat_session \
         WHERE chat_session.project_id = project.project_id)"
    )
}

/// 清理空项目行（破坏性）：硬删除体检诊断出的空壳 project 行。
///
/// 删除范围与体检空项目统计同口径（`empty_project_scope`）；单条 DELETE
/// 由 SQLite 保证原子性。前置条件：主库 TRAE 实例已关闭且已完成备份
/// （调用方编排，与收编同纪律——ADR-0018 铁律）。返回实际删除行数
/// （0 = 无可清理）。
pub fn delete_master_empty_projects(
    master_data_dir: &Path,
    raw_key: &str,
) -> Result<usize, MasterArchiveError> {
    let db_path = master_database_path(master_data_dir);
    if !db_path.is_file() {
        return Err(MasterArchiveError::DbUnavailable);
    }
    let conn = open_with_key_readwrite(&db_path, raw_key)?;
    verify_key_readable(&conn)?;
    let scope = empty_project_scope(column_exists(&conn, "project", "deleted_at"));
    conn.execute(&format!("DELETE FROM project WHERE {scope}"), [])
        .map_err(|_| MasterArchiveError::WriteFailed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::{Connection, OpenFlags};

    /// 构造加密主库 fixture：三账号分布 + 孤儿行（含软删行与 0 会话账号）。
    fn create_checkup_fixture(db_path: &Path, raw_key: &str) {
        let flags = OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE;
        let conn = Connection::open_with_flags(db_path, flags).unwrap();
        let pragma = format!("PRAGMA key = \"x'{}'\";", raw_key);
        conn.execute_batch(&pragma).unwrap();
        conn.execute_batch(
            "CREATE TABLE project (project_id TEXT PRIMARY KEY, user_id TEXT, name TEXT, deleted_at INTEGER);
             CREATE TABLE chat_session (session_id TEXT PRIMARY KEY, project_id TEXT, deleted_at INTEGER);
             INSERT INTO project VALUES ('p1', '111', '当前账号项目', 0);
             INSERT INTO project VALUES ('p2', '222', '滞留账号项目', 0);
             INSERT INTO project VALUES ('p3', '222', '滞留账号软删项目', 500);
             INSERT INTO project VALUES ('p4', '333', '零会话账号项目', 0);
             INSERT INTO project VALUES ('p5', NULL, '无归属行', 0);
             INSERT INTO chat_session VALUES ('s1', 'p1', 0);
             INSERT INTO chat_session VALUES ('s2', 'p2', 0);
             INSERT INTO chat_session VALUES ('s3', 'p2', 900);
             INSERT INTO chat_session VALUES ('s4', 'p5', 0);",
        )
        .unwrap();
    }

    fn temp_dir(label: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "trae-sync-p55-checkup-{}-{}-{}",
            label,
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(dir.join("ModularData").join("ai-agent")).unwrap();
        dir
    }

    #[test]
    fn aggregates_distribution_and_orphans_with_soft_deleted() {
        let dir = temp_dir("distribution");
        let key = "aa".repeat(32);
        create_checkup_fixture(&master_database_path(&dir), &key);

        match read_master_checkup(&dir, &key) {
            MasterCheckupStatus::Ready(report) => {
                // 过滤软删后 111 与 222 各有 1 个正常会话，按 user_id 稳定排序。
                assert_eq!(report.accounts.len(), 3);
                let stale_account = report
                    .accounts
                    .iter()
                    .find(|account| account.user_id == "222")
                    .expect("应包含非当前账号 222");
                // 软删会话 s3 与只剩软删会话的项目 p3 不进入可交接规模。
                assert_eq!(stale_account.project_count, 1);
                assert_eq!(stale_account.session_count, 1);
                // p3 是已删除项目残留，不再计为空项目（2026-09 口径修订）。
                assert_eq!(stale_account.empty_project_count, 0);
                // 零会话账号（333）：LEFT JOIN 保留，但不成为收编候选。
                let zero = report.accounts.iter().find(|r| r.user_id == "333").unwrap();
                assert_eq!((zero.project_count, zero.session_count), (0, 0));
                assert_eq!(zero.empty_project_count, 1);
                // 孤儿行：p5（user_id NULL）挂 s4。
                assert_eq!(report.orphan_project_count, 1);
                assert_eq!(report.orphan_session_count, 1);
                // 滞留过滤（当前账号 = 111）：只有仍有正常会话的 222。
                let stale = report.stale_rows("111");
                assert_eq!(stale.len(), 1);
            }
            other => panic!("期望 Ready，实际 {:?}", other),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn excludes_archived_sessions_from_actionable_distribution() {
        let dir = temp_dir("archived");
        let key = "ee".repeat(32);
        let db_path = master_database_path(&dir);
        create_checkup_fixture(&db_path, &key);
        {
            let flags = OpenFlags::SQLITE_OPEN_READ_WRITE;
            let conn = Connection::open_with_flags(&db_path, flags).unwrap();
            conn.execute_batch(&format!("PRAGMA key = \"x'{key}'\";"))
                .unwrap();
            conn.execute("ALTER TABLE chat_session ADD COLUMN hidden_status TEXT", [])
                .unwrap();
            // 追加一个只有归档会话的账号，验证它不会成为收编候选。
            conn.execute(
                "INSERT INTO project (project_id, user_id, name, deleted_at)
                 VALUES ('p6', '444', '仅归档账号项目', 0)",
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO chat_session (session_id, project_id, deleted_at, hidden_status)
                 VALUES ('s5', 'p2', 0, 'voice_discussion'),
                        ('s6', 'p6', 0, 'voice_discussion')",
                [],
            )
            .unwrap();
        }

        match read_master_checkup(&dir, &key) {
            MasterCheckupStatus::Ready(report) => {
                let stale = report.stale_rows("111");
                assert_eq!(stale.len(), 1);
                assert_eq!(stale[0].user_id, "222");
                assert_eq!((stale[0].project_count, stale[0].session_count), (1, 1));
                // p3 软删残留不计入空项目（口径修订后仅 p4 这类未删空行才计）。
                assert_eq!(stale[0].empty_project_count, 0);
                let archived_only = report
                    .accounts
                    .iter()
                    .find(|account| account.user_id == "444")
                    .expect("应保留仅归档账号的诊断行");
                assert_eq!(
                    (archived_only.project_count, archived_only.session_count),
                    (0, 0)
                );
                assert_eq!(archived_only.empty_project_count, 0);
            }
            other => panic!("期望 Ready，实际 {:?}", other),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn empty_project_count_excludes_soft_deleted_projects() {
        let dir = temp_dir("empty-softdel");
        let key = "ff".repeat(32);
        create_checkup_fixture(&master_database_path(&dir), &key);

        match read_master_checkup(&dir, &key) {
            MasterCheckupStatus::Ready(report) => {
                // p3（deleted_at=500，无会话）是 TRAE 内删除后的残留行，不计空项目。
                let stale = report.accounts.iter().find(|r| r.user_id == "222").unwrap();
                assert_eq!(stale.empty_project_count, 0, "软删项目残留不应计为空项目");
                // p4（deleted_at=0，无会话）是真实空项目，保留诊断。
                let zero = report.accounts.iter().find(|r| r.user_id == "333").unwrap();
                assert_eq!(zero.empty_project_count, 1, "未删除的零会话项目仍应计为空项目");
                assert_eq!(report.empty_project_total(), 1);
            }
            other => panic!("期望 Ready，实际 {:?}", other),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_db_reports_no_master_data() {
        let dir = temp_dir("missing");
        assert_eq!(
            read_master_checkup(&dir, &"bb".repeat(32)),
            MasterCheckupStatus::NoMasterData
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn wrong_key_reports_read_failed() {
        let dir = temp_dir("wrongkey");
        let key = "cc".repeat(32);
        create_checkup_fixture(&master_database_path(&dir), &key);
        assert_eq!(
            read_master_checkup(&dir, &"dd".repeat(32)),
            MasterCheckupStatus::ReadFailed
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn cleanup_deletes_empty_projects_and_keeps_others() {
        let dir = temp_dir("cleanup");
        let key = "12".repeat(32);
        create_checkup_fixture(&master_database_path(&dir), &key);

        // 范围内只有 p4（未删除、零会话）；p3 软删、p5 孤儿、p1/p2 有会话都保留。
        let deleted = delete_master_empty_projects(&dir, &key).unwrap();
        assert_eq!(deleted, 1);

        let conn = Connection::open_with_flags(
            master_database_path(&dir),
            OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .unwrap();
        conn.execute_batch(&format!("PRAGMA key = \"x'{key}'\";"))
            .unwrap();
        let survivors: Vec<String> = conn
            .prepare("SELECT project_id FROM project ORDER BY project_id")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(survivors, vec!["p1", "p2", "p3", "p5"]);

        // 幂等：空项目清零后再次清理删除 0 行。
        let deleted = delete_master_empty_projects(&dir, &key).unwrap();
        assert_eq!(deleted, 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn cleanup_without_deleted_at_column_still_clears_empty_projects() {
        // 旧库 project 表无 deleted_at 列：全部视为未删除，仍按会话行判定。
        let dir = temp_dir("cleanupoldschema");
        let key = "13".repeat(32);
        {
            let flags = OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE;
            let conn = Connection::open_with_flags(master_database_path(&dir), flags).unwrap();
            conn.execute_batch(&format!("PRAGMA key = \"x'{key}'\";"))
                .unwrap();
            conn.execute_batch(
                "CREATE TABLE project (project_id TEXT PRIMARY KEY, user_id TEXT, name TEXT);
                 CREATE TABLE chat_session (session_id TEXT PRIMARY KEY, project_id TEXT);
                 INSERT INTO project VALUES ('p1', '111', '有会话项目');
                 INSERT INTO project VALUES ('pe', '111', '空项目行');
                 INSERT INTO project VALUES ('po', NULL, '无归属空行');
                 INSERT INTO chat_session VALUES ('s1', 'p1');",
            )
            .unwrap();
        }
        // pe（有归属空行）删除；po（孤儿）保守保留。
        let deleted = delete_master_empty_projects(&dir, &key).unwrap();
        assert_eq!(deleted, 1);
        let conn = Connection::open_with_flags(
            master_database_path(&dir),
            OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .unwrap();
        conn.execute_batch(&format!("PRAGMA key = \"x'{key}'\";"))
            .unwrap();
        let survivors: Vec<String> = conn
            .prepare("SELECT project_id FROM project ORDER BY project_id")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(survivors, vec!["p1", "po"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn cleanup_missing_db_reports_unavailable() {
        let dir = temp_dir("cleanupmissing");
        assert_eq!(
            delete_master_empty_projects(&dir, &"bb".repeat(32)),
            Err(MasterArchiveError::DbUnavailable)
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn cleanup_wrong_key_reports_open_failed() {
        let dir = temp_dir("cleanupwrongkey");
        let key = "cc".repeat(32);
        create_checkup_fixture(&master_database_path(&dir), &key);
        assert_eq!(
            delete_master_empty_projects(&dir, &"dd".repeat(32)),
            Err(MasterArchiveError::DbOpenFailed)
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
