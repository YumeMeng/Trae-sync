//! P5-5 主库体检（只读）：记录分布 + 孤儿行 + 滞留账号识别。
//!
//! 数据源：`project.user_id` 聚合（2026-08-31 探针实证 SQL，口径 = 全量
//! 含软删——归属改写与换腿均作用于全量行，体检统计必须同口径才不误导）。
//! 注册比对由调用方传入注册表账号映射（user_id → 账号名），本模块不读
//! 注册表文件——保持只读主库单一职责。
//!
//! 收编安全口径：孤儿行（`user_id IS NULL`）只报告不收编——无归属行
//! 可能是 TRAE 自身维护的数据，改写归属缺乏依据（保守不动）。

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::master_handover::master_database_path;
use crate::sqlcipher::open_with_key_readonly;

/// 注册表账号映射条目（调用方从 AccountRegistry 投影；user_id 即 account_id）。
#[derive(Debug, Clone)]
pub struct RegisteredAccount {
    pub user_id: String,
    /// 展示名（display_name 优先，回退 screen_name；调用方决定）。
    pub account_name: String,
}

/// 一个账号在主库内的记录规模（体检行；不直接序列化，DTO 层补账号名）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckupAccountRow {
    /// TRAE user_id（技术标识；UI 层只用账号名/未登记表述，此值收进悬浮提示）。
    pub user_id: String,
    /// 项目行数（全量含软删，与归属改写口径一致）。
    pub project_count: u64,
    /// 会话数（全量含软删）。
    pub session_count: u64,
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
    /// 全库账号分布（含当前账号；按会话数降序）。
    pub accounts: Vec<CheckupAccountRow>,
    /// 无归属（user_id IS NULL）项目行数（只报告，收编不动）。
    pub orphan_project_count: u64,
    /// 挂在无归属项目行上的会话数。
    pub orphan_session_count: u64,
}

impl MasterCheckup {
    /// 滞留账号行（非当前账号的分布行——收编目标规模）。
    pub fn stale_rows(&self, current_user_id: &str) -> Vec<&CheckupAccountRow> {
        self.accounts
            .iter()
            .filter(|row| row.user_id != current_user_id)
            .collect()
    }
}

/// 读取主库体检报告（探针实证口径：全量含软删，只读打开）。
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

/// 内层聚合：任一 SQL 失败整体 ReadFailed（分布数字半缺比没有更误导）。
fn read_checkup_inner(conn: &rusqlite::Connection) -> Option<MasterCheckup> {
    // 账号分布：探针同款 SQL（LEFT JOIN 保留 0 会话账号；含软删全量）。
    let mut statement = conn
        .prepare(
            "SELECT p.user_id, COUNT(DISTINCT p.project_id), COUNT(DISTINCT s.session_id)
             FROM project p
             LEFT JOIN chat_session s ON s.project_id = p.project_id
             WHERE p.user_id IS NOT NULL AND p.user_id != ''
             GROUP BY p.user_id
             ORDER BY COUNT(DISTINCT s.session_id) DESC, p.user_id",
        )
        .ok()?;
    let accounts: Vec<CheckupAccountRow> = statement
        .query_map([], |row| {
            Ok(CheckupAccountRow {
                user_id: row.get::<_, String>(0)?,
                project_count: row.get::<_, i64>(1)?.max(0) as u64,
                session_count: row.get::<_, i64>(2)?.max(0) as u64,
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
                // 分布按会话数降序：111(1) / 222(2) → 222 在前。
                assert_eq!(report.accounts.len(), 3);
                assert_eq!(report.accounts[0].user_id, "222");
                // 软删会话 s3 与软删项目 p3 全量计入（与归属改写口径一致）。
                assert_eq!(report.accounts[0].project_count, 2);
                assert_eq!(report.accounts[0].session_count, 2);
                // 零会话账号（333）：LEFT JOIN 保留，1 项目 0 会话。
                let zero = report.accounts.iter().find(|r| r.user_id == "333").unwrap();
                assert_eq!((zero.project_count, zero.session_count), (1, 0));
                // 孤儿行：p5（user_id NULL）挂 s4。
                assert_eq!(report.orphan_project_count, 1);
                assert_eq!(report.orphan_session_count, 1);
                // 滞留过滤（当前账号 = 111）：222 与 333。
                let stale = report.stale_rows("111");
                assert_eq!(stale.len(), 2);
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
}
