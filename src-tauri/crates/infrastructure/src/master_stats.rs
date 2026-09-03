//! P5-4 主库轻量统计（总览页 + 环境卡会话胶囊数据源，只读）。
//!
//! 与 `master_history` 的分工：历史页需要全量项目/会话明细；总览与环境卡
//! 只需要聚合计数。本模块只跑 4 条聚合 SQL（列防御与只读打开方式复用
//! master_history 的隔离三件套机制，主库运行中可随时读取）。
//!
//! 口径：
//! - `project_count` / `session_count`：当前登录账号可见（`project.user_id`
//!   过滤 + 软删排除），与历史页两栏一致。
//! - `participating_account_count`：主库内出现过的全部账号（distinct
//!   `project.user_id` 非空），体现主库共享语义（ADR-0021 混居事实）。
//! - `last_active_unix_seconds`：当前账号会话最近 updated_at（毫秒/秒
//!   双单位归一化，与索引模块同口径）。

use std::path::Path;

use rusqlite::Connection;
use serde::{Deserialize, Serialize};

use crate::master_handover::master_database_path;
use crate::sqlcipher::open_with_key_readonly;

/// 主库聚合统计（DTO 直接序列化形态）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MasterLibraryStats {
    /// 当前账号可见项目数（软删排除）。
    pub project_count: u64,
    /// 当前账号可见会话数（软删排除）。
    pub session_count: u64,
    /// 当前账号可见会话的消息总数（软删排除，与历史页 message_count 同口径）。
    pub message_count: u64,
    /// 主库内出现过的账号数（含非当前账号的历史归属）。
    pub participating_account_count: u64,
    /// 当前账号会话最近活跃时间（秒）；无会话为 None。
    pub last_active_unix_seconds: Option<i64>,
}

/// 主库三件套（database.db + wal/shm 附属件）合计字节数（P5-8a-2 详情页头部）。
///
/// 纯文件系统 metadata 求和：主库运行中可随时读取，不打开数据库。
/// 主库从未启动过（无 database.db）时为 0。
pub fn master_trio_size_bytes(master_data_dir: &Path) -> u64 {
    let db_path = master_database_path(master_data_dir);
    let trio_size = |path: &Path| std::fs::metadata(path).map(|meta| meta.len()).unwrap_or(0);
    let wal = {
        let mut p = db_path.clone().into_os_string();
        p.push("-wal");
        trio_size(Path::new(&p))
    };
    let shm = {
        let mut p = db_path.clone().into_os_string();
        p.push("-shm");
        trio_size(Path::new(&p))
    };
    trio_size(&db_path) + wal + shm
}

/// 全库规模统计（P6-4 环境删除确认的数据源：删除前列明将被删除的
/// 项目/会话规模，不做账号过滤——环境内记录单一归属当前登录账号
/// （ADR-0021 语义泛化），删除即全删）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TotalLibraryScale {
    /// 全部账号的非软删项目数。
    pub project_count: u64,
    /// 全部账号的非软删会话数。
    pub session_count: u64,
}

/// 全库规模读取结果（状态语义与 `MasterStatsStatus` 一致）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TotalScaleStatus {
    Ready(TotalLibraryScale),
    /// 数据库不存在（环境从未启动过，无记录可删）。
    NoMasterData,
    /// 打开或读取失败（key 不匹配、文件损坏等）。
    ReadFailed,
}

/// 读取全库规模（无账号过滤；软删排除口径与可见统计一致）。
pub fn read_total_scale(data_dir: &Path, raw_key: &str) -> TotalScaleStatus {
    let db_path = master_database_path(data_dir);
    if !db_path.is_file() {
        return TotalScaleStatus::NoMasterData;
    }
    let Ok(conn) = open_with_key_readonly(&db_path, raw_key) else {
        return TotalScaleStatus::ReadFailed;
    };
    read_total_scale_inner(&conn).map_or(TotalScaleStatus::ReadFailed, TotalScaleStatus::Ready)
}

/// 内层聚合：任一 SQL 失败整体 ReadFailed（规模数字半缺比没有更误导）。
fn read_total_scale_inner(conn: &Connection) -> Option<TotalLibraryScale> {
    let has_project_deleted = column_exists(conn, "project", "deleted_at");
    let project_deleted_expr = if has_project_deleted {
        "COALESCE(p.deleted_at, 0)"
    } else {
        "0"
    };
    let project_count = conn
        .query_row(
            &format!("SELECT COUNT(*) FROM project p WHERE {project_deleted_expr} = 0"),
            [],
            |row| row.get::<_, i64>(0),
        )
        .ok()?
        .max(0) as u64;

    let has_session_deleted = column_exists(conn, "chat_session", "deleted_at");
    let session_deleted_expr = if has_session_deleted {
        "COALESCE(s.deleted_at, 0)"
    } else {
        "0"
    };
    let session_count = conn
        .query_row(
            &format!(
                "SELECT COUNT(*) FROM chat_session s WHERE {session_deleted_expr} = 0"
            ),
            [],
            |row| row.get::<_, i64>(0),
        )
        .ok()?
        .max(0) as u64;

    Some(TotalLibraryScale {
        project_count,
        session_count,
    })
}

/// 统计读取结果（状态语义与 `MasterHistoryStatus` 一致）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MasterStatsStatus {
    Ready(MasterLibraryStats),
    /// 主库从未启动过（无 database.db）。
    NoMasterData,
    /// 打开或读取失败（key 不匹配、文件损坏等）。
    ReadFailed,
}

/// 检测表列是否存在（master_history 同款降级防御）。
fn column_exists(conn: &Connection, table: &str, column: &str) -> bool {
    let sql = format!("PRAGMA table_info({})", table);
    let Ok(mut stmt) = conn.prepare(&sql) else {
        return false;
    };
    let names: Vec<String> = match stmt.query_map([], |row| row.get::<_, String>(1)) {
        Ok(rows) => rows.filter_map(|row| row.ok()).collect(),
        Err(_) => return false,
    };
    names.iter().any(|name| name == column)
}

/// 毫秒/秒双单位时间戳归一化为秒（索引模块同口径）。
fn normalize_timestamp_to_seconds(raw: i64) -> i64 {
    if raw > 1_000_000_000_000 {
        raw / 1000
    } else {
        raw
    }
}

/// 读取主库聚合统计（当前账号可见口径 + 全库参与账号数）。
pub fn read_master_stats(
    master_data_dir: &Path,
    raw_key: &str,
    current_user_id: &str,
) -> MasterStatsStatus {
    let db_path = master_database_path(master_data_dir);
    if !db_path.is_file() {
        return MasterStatsStatus::NoMasterData;
    }
    let Ok(conn) = open_with_key_readonly(&db_path, raw_key) else {
        return MasterStatsStatus::ReadFailed;
    };
    read_master_stats_inner(&conn, current_user_id).map_or(MasterStatsStatus::ReadFailed, MasterStatsStatus::Ready)
}

/// 内层聚合：任一 SQL 失败整体 ReadFailed（聚合统计不做半套降级，
/// 与明细读取的列级降级不同——统计数字半缺比没有更误导）。
fn read_master_stats_inner(
    conn: &Connection,
    current_user_id: &str,
) -> Option<MasterLibraryStats> {
    let has_project_deleted = column_exists(conn, "project", "deleted_at");
    let project_deleted_expr = if has_project_deleted {
        "COALESCE(p.deleted_at, 0)"
    } else {
        "0"
    };

    let project_count = conn
        .query_row(
            &format!(
                "SELECT COUNT(*) FROM project p \
                 WHERE p.user_id = ?1 AND {project_deleted_expr} = 0"
            ),
            [current_user_id],
            |row| row.get::<_, i64>(0),
        )
        .ok()?
        .max(0) as u64;

    let has_session_deleted = column_exists(conn, "chat_session", "deleted_at");
    let session_deleted_expr = if has_session_deleted {
        "COALESCE(s.deleted_at, 0)"
    } else {
        "0"
    };
    // 消息软删口径与 master_history 的 message_count 子查询一致。
    let has_message_deleted = column_exists(conn, "chat_message", "deleted_at");
    let message_deleted_expr = if has_message_deleted {
        "(m.deleted_at IS NULL OR m.deleted_at = 0)"
    } else {
        "1"
    };
    let session_count = conn
        .query_row(
            &format!(
                "SELECT COUNT(*) FROM chat_session s JOIN project p ON p.project_id = s.project_id \
                 WHERE p.user_id = ?1 AND {session_deleted_expr} = 0"
            ),
            [current_user_id],
            |row| row.get::<_, i64>(0),
        )
        .ok()?
        .max(0) as u64;

    // 消息总数：当前账号会话下的 chat_message 行（软删消息不计）。
    let message_count = conn
        .query_row(
            &format!(
                "SELECT COUNT(*) FROM chat_message m \
                 JOIN chat_session s ON s.session_id = m.session_id \
                 JOIN project p ON p.project_id = s.project_id \
                 WHERE p.user_id = ?1 AND {session_deleted_expr} = 0 AND {message_deleted_expr}"
            ),
            [current_user_id],
            |row| row.get::<_, i64>(0),
        )
        .ok()?
        .max(0) as u64;

    // 参与账号：distinct project.user_id（非空；含历史归属账号）。
    let participating_account_count = conn
        .query_row(
            "SELECT COUNT(DISTINCT p.user_id) FROM project p \
             WHERE p.user_id IS NOT NULL AND p.user_id != ''",
            [],
            |row| row.get::<_, i64>(0),
        )
        .ok()?
        .max(0) as u64;

    // 最近活跃：当前账号会话 updated_at 最大值（列缺失时 NULL 语义）。
    let has_updated_at = column_exists(conn, "chat_session", "updated_at");
    let updated_expr = if has_updated_at {
        "s.updated_at"
    } else {
        "NULL"
    };
    let last_active: Option<i64> = conn
        .query_row(
            &format!(
                "SELECT MAX({updated_expr}) FROM chat_session s \
                 JOIN project p ON p.project_id = s.project_id \
                 WHERE p.user_id = ?1 AND {session_deleted_expr} = 0"
            ),
            [current_user_id],
            |row| row.get(0),
        )
        .ok()?;

    Some(MasterLibraryStats {
        project_count,
        session_count,
        message_count,
        participating_account_count,
        last_active_unix_seconds: last_active.map(normalize_timestamp_to_seconds),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::OpenFlags;

    /// 构造加密主库 fixture：两账号归属 + 软删 + 毫秒时间戳。
    fn create_stats_fixture(db_path: &Path, raw_key: &str) {
        let flags = OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE;
        let conn = Connection::open_with_flags(db_path, flags).unwrap();
        let pragma = format!("PRAGMA key = \"x'{}'\";", raw_key);
        conn.execute_batch(&pragma).unwrap();
        conn.execute_batch(
            "CREATE TABLE project (project_id TEXT PRIMARY KEY, user_id TEXT, name TEXT, deleted_at INTEGER);
             CREATE TABLE chat_session (session_id TEXT PRIMARY KEY, project_id TEXT, session_title TEXT, updated_at INTEGER, deleted_at INTEGER);
             CREATE TABLE chat_message (message_id TEXT PRIMARY KEY, session_id TEXT, deleted_at INTEGER);
             INSERT INTO project VALUES ('p1', '111', '项目甲', 0);
             INSERT INTO project VALUES ('p2', '111', '软删项目', 500);
             INSERT INTO project VALUES ('p3', '222', '账号乙项目', 0);
             INSERT INTO project VALUES ('p4', NULL, '无归属行', 0);
             INSERT INTO chat_session VALUES ('s1', 'p1', '会话一', 1770000000000, 0);
             INSERT INTO chat_session VALUES ('s2', 'p1', '会话二', 1769000000000, 0);
             INSERT INTO chat_session VALUES ('s3', 'p1', '软删会话', 1780000000000, 99);
             INSERT INTO chat_session VALUES ('s4', 'p3', '账号乙会话', 1780000000000, 0);
             INSERT INTO chat_message VALUES ('m1', 's1', 0);
             INSERT INTO chat_message VALUES ('m2', 's1', 99);
             INSERT INTO chat_message VALUES ('m3', 's2', 0);
             INSERT INTO chat_message VALUES ('m4', 's3', 0);
             INSERT INTO chat_message VALUES ('m5', 's4', 0);",
        )
        .unwrap();
    }

    fn temp_dir(label: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "trae-sync-p54-stats-{}-{}-{}",
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
    fn aggregates_current_account_with_participating_accounts() {
        let dir = temp_dir("aggregate");
        let key = "aa".repeat(32);
        create_stats_fixture(&master_database_path(&dir), &key);

        let status = read_master_stats(&dir, &key, "111");
        match status {
            MasterStatsStatus::Ready(stats) => {
                // 当前账号：1 项目（软删排除）· 2 会话（软删排除）。
                assert_eq!(stats.project_count, 1);
                assert_eq!(stats.session_count, 2);
                // 消息：m1 + m3（软删 m2 与软删会话 s3 的 m4 不计；m4 属 s3）。
                assert_eq!(stats.message_count, 2);
                // 参与账号：111 + 222（NULL 行不计）。
                assert_eq!(stats.participating_account_count, 2);
                // 最近活跃 = s2 的毫秒时间归一化（s1 与 s2 取较新的 s1？
                // 1770000000000 > 1769000000000 → s1；软删 s3 不计）。
                assert_eq!(stats.last_active_unix_seconds, Some(1770000000));
            }
            other => panic!("期望 Ready，实际 {:?}", other),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn total_scale_counts_all_accounts_excluding_soft_deleted() {
        // P6-4 删除确认规模：跨账号全量计数（111 的 p1/s1/s2 + 222 的 p3/s4），
        // 软删行（p2/s3）与无归属行（p4）口径——p4 无 user_id 但非软删，计入。
        let dir = temp_dir("totalscale");
        let key = "ff".repeat(32);
        create_stats_fixture(&master_database_path(&dir), &key);

        match read_total_scale(&dir, &key) {
            TotalScaleStatus::Ready(scale) => {
                assert_eq!(scale.project_count, 3);
                assert_eq!(scale.session_count, 3);
            }
            other => panic!("期望 Ready，实际 {:?}", other),
        }
        // 库缺失：NoMasterData（环境从未启动）。
        let empty = temp_dir("totalscale-empty");
        assert_eq!(
            read_total_scale(&empty, &key),
            TotalScaleStatus::NoMasterData
        );
        // key 不匹配：ReadFailed（规模未知，前端提示谨慎删除）。
        assert_eq!(
            read_total_scale(&dir, &"00".repeat(32)),
            TotalScaleStatus::ReadFailed
        );
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&empty);
    }

    #[test]
    fn missing_db_reports_no_master_data() {
        let dir = temp_dir("missing");
        let status = read_master_stats(&dir, &"bb".repeat(32), "111");
        assert_eq!(status, MasterStatsStatus::NoMasterData);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn wrong_key_reports_read_failed() {
        let dir = temp_dir("wrongkey");
        create_stats_fixture(&master_database_path(&dir), &"cc".repeat(32));
        let status = read_master_stats(&dir, &"dd".repeat(32), "111");
        assert_eq!(status, MasterStatsStatus::ReadFailed);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn account_without_records_counts_participants_only() {
        // 新账号视角：可见 0 项目 0 会话，但参与账号仍统计全库归属。
        let dir = temp_dir("newaccount");
        let key = "ee".repeat(32);
        create_stats_fixture(&master_database_path(&dir), &key);
        let status = read_master_stats(&dir, &key, "999");
        match status {
            MasterStatsStatus::Ready(stats) => {
                assert_eq!(stats.project_count, 0);
                assert_eq!(stats.session_count, 0);
                assert_eq!(stats.message_count, 0);
                assert_eq!(stats.participating_account_count, 2);
                assert_eq!(stats.last_active_unix_seconds, None);
            }
            other => panic!("期望 Ready，实际 {:?}", other),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn trio_size_sums_db_and_sidecar_files() {
        // P5-8a-2 详情页体积：三件套存在文件求和，缺失附属件按 0 计。
        let dir = temp_dir("triosize");
        let db_path = master_database_path(&dir);
        std::fs::write(&db_path, vec![0u8; 1024]).unwrap();
        let mut wal = db_path.clone().into_os_string();
        wal.push("-wal");
        std::fs::write(Path::new(&wal), vec![0u8; 512]).unwrap();

        assert_eq!(master_trio_size_bytes(&dir), 1536);

        // 无任何库文件（主库从未启动）：0。
        let empty = temp_dir("triosize-empty");
        assert_eq!(master_trio_size_bytes(&empty), 0);
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&empty);
    }
}
