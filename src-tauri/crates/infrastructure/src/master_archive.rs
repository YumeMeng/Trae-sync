//! P5-8a 会话归档通道（ADR-0022）：hidden_status 原生枚举借用。
//!
//! 协议事实（2026-08-31 真机实验 `.scratch/history-u6/w0-probe/`）：
//! - TRAE 会话列表过滤为**排除式枚举白名单**：只认 `scheduled_task` /
//!   `voice_discussion` 两个原生值；自造值不生效。
//! - 借用 `voice_discussion` 隐藏/恢复 25 会话全部生效，重启不被回写，
//!   恢复的会话自动并入当前同类型分组（侧栏按 work_mode 聚合）。
//! - 写入路径：在线写（主库运行中）+ busy_timeout 与 TRAE 写入错峰，
//!   绝不长锁（`batch_hide_default` 探针同款）。
//!
//! 真实删除（ADR-0022 决策 4）：消息内容三表 + chat_message + chat_session
//! + 空壳项目行，单事务；调用方保证先备份（ADR-0018 铁律，本模块
//! `delete_master_sessions` 内置 `backup_master_trio` 前置）。
//!
//! 分组合并（P5-8c）：源分组的全部会话改挂 `project_id` 到目标分组，
//! 空壳源分组行随事务清理——复用删除路径的改挂与空壳清理逻辑；
//! 批量迁移属破坏性操作，执行前同样内置备份。

use std::path::Path;
use std::time::Duration;

use rusqlite::Connection;

use crate::master_handover::{backup_master_trio, master_database_path};

/// 借用的原生隐藏枚举（会话列表排除式白名单成员）。
pub const ARCHIVE_HIDDEN_STATUS: &str = "voice_discussion";

/// 归档/恢复/删除失败（全部非敏感：不含 key、消息内容或账号信息）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MasterArchiveError {
    /// 主库 database.db 不存在（主库尚未启动/登录过）。
    DbUnavailable,
    /// 打开或解密失败（raw key 不匹配或库损坏）。
    DbOpenFailed,
    /// chat_session.hidden_status 列缺失（老 schema 不支持归档通道）。
    ArchiveUnsupported,
    /// 写入失败（事务已回滚，库保持操作前状态）。
    WriteFailed,
    /// 删除前备份失败（删除未执行，库保持原状）。
    BackupFailed,
    /// 分组合并参数无效（无来源分组，或目标分组在来源集合中/不存在）。
    MergeInvalid,
}

impl std::fmt::Display for MasterArchiveError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let message = match self {
            Self::DbUnavailable => "主库对话库不存在",
            Self::DbOpenFailed => "主库对话库打开或解密失败",
            Self::ArchiveUnsupported => "当前主库版本不支持会话归档",
            Self::WriteFailed => "会话操作写入失败（已回滚）",
            Self::BackupFailed => "删除前主库备份失败（未执行删除）",
            Self::MergeInvalid => "分组合并参数无效",
        };
        formatter.write_str(message)
    }
}

impl std::error::Error for MasterArchiveError {}

/// 真实删除回执（单事务提交成功后返回；规模供 UI 展示）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MasterDeleteOutcome {
    pub deleted_sessions: usize,
    /// 删除的消息元数据行数（含随行的内容三表行）。
    pub deleted_messages: usize,
    /// 删除后变空壳而被清理的项目行数。
    pub removed_projects: usize,
    /// 删除前自动创建的备份路径（人工恢复定位）。
    pub backup_path: String,
}

/// 以读写模式打开主库并设 key（在线写路径；busy_timeout 与 TRAE 错峰）。
fn open_with_key_readwrite(db_path: &Path, raw_key: &str) -> Result<Connection, MasterArchiveError> {
    let conn = Connection::open(db_path).map_err(|_| MasterArchiveError::DbOpenFailed)?;
    // busy_timeout 用原生 API 设置：`PRAGMA busy_timeout = N` 会返回结果行，
    // 与 PRAGMA key 同批 execute_batch 会报 ExecuteReturnedResults（与交接同款）。
    conn.busy_timeout(Duration::from_millis(10_000))
        .map_err(|_| MasterArchiveError::DbOpenFailed)?;
    conn.execute_batch(&format!("PRAGMA key = \"x'{raw_key}'\";"))
        .map_err(|_| MasterArchiveError::DbOpenFailed)?;
    Ok(conn)
}

/// 检测 chat_session.hidden_status 列是否存在（老 schema 降级报不支持）。
fn hidden_status_column_exists(conn: &Connection) -> bool {
    let Ok(mut stmt) = conn.prepare("PRAGMA table_info(chat_session)") else {
        return false;
    };
    let names: Vec<String> = match stmt.query_map([], |row| row.get::<_, String>(1)) {
        Ok(rows) => rows.filter_map(|row| row.ok()).collect(),
        Err(_) => return false,
    };
    names.iter().any(|name| name == "hidden_status")
}

/// 解密探测：SQLCipher 的 key 错误延迟到首次真实查询才报
/// "file is not a database"，PRAGMA table_info 失败又会被列防御吞掉，
/// 故归档/恢复路径先跑一次真实读取把 key 不匹配显式归位为 DbOpenFailed。
fn verify_key_readable(conn: &Connection) -> Result<(), MasterArchiveError> {
    conn.query_row("SELECT COUNT(*) FROM sqlite_master", [], |row| {
        row.get::<_, i64>(0)
    })
    .map(|_| ())
    .map_err(|_| MasterArchiveError::DbOpenFailed)
}

/// 构造 `?, ?, ...` 占位符串（批量 IN 子句用）。
fn placeholders(count: usize) -> String {
    vec!["?"; count].join(", ")
}

/// 只保留当前主库登录账号名下的会话；调用方给出的 ID 可能来自过期界面，
/// 所有写操作都必须在数据库层重新确认项目归属。
fn authorized_session_ids(
    conn: &Connection,
    session_ids: &[String],
    current_user_id: &str,
) -> Result<Vec<String>, MasterArchiveError> {
    if session_ids.is_empty() {
        return Ok(Vec::new());
    }
    let owner_param = session_ids.len() + 1;
    let sql = format!(
        "SELECT DISTINCT s.session_id \
         FROM chat_session s \
         JOIN project p ON p.project_id = s.project_id \
         WHERE s.session_id IN ({}) AND p.user_id = ?{owner_param}",
        placeholders(session_ids.len())
    );
    let mut params: Vec<&str> = session_ids.iter().map(String::as_str).collect();
    params.push(current_user_id);
    let mut statement = conn
        .prepare(&sql)
        .map_err(|_| MasterArchiveError::WriteFailed)?;
    let rows = statement
        .query_map(rusqlite::params_from_iter(params), |row| row.get(0))
        .map_err(|_| MasterArchiveError::WriteFailed)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| MasterArchiveError::WriteFailed)?;
    Ok(rows)
}

/// 归档会话（在线写）：hidden_status → 'voice_discussion'。
///
/// 只动集合内 `hidden_status IS NULL` 的行（原生已隐藏的会话不重复触碰）；
/// 返回实际受影响行数。归档可逆（restore 还原 NULL），不改变记录归属。
pub fn archive_master_sessions(
    master_data_dir: &Path,
    raw_key: &str,
    session_ids: &[String],
    current_user_id: &str,
) -> Result<usize, MasterArchiveError> {
    if session_ids.is_empty() {
        return Ok(0);
    }
    let db_path = master_database_path(master_data_dir);
    if !db_path.is_file() {
        return Err(MasterArchiveError::DbUnavailable);
    }
    let conn = open_with_key_readwrite(&db_path, raw_key)?;
    verify_key_readable(&conn)?;
    if !hidden_status_column_exists(&conn) {
        return Err(MasterArchiveError::ArchiveUnsupported);
    }
    let sql = format!(
        "UPDATE chat_session SET hidden_status = '{ARCHIVE_HIDDEN_STATUS}' \
         WHERE hidden_status IS NULL AND session_id IN ({}) \
         AND EXISTS (SELECT 1 FROM project p \
                    WHERE p.project_id = chat_session.project_id \
                      AND p.user_id = ?{})",
        placeholders(session_ids.len()),
        session_ids.len() + 1
    );
    let mut params: Vec<&str> = session_ids.iter().map(String::as_str).collect();
    params.push(current_user_id);
    let changed = conn
        .execute(&sql, rusqlite::params_from_iter(params))
        .map_err(|_| MasterArchiveError::WriteFailed)?;
    Ok(changed)
}

/// 恢复归档会话（在线写）：hidden_status 还原为 NULL。
///
/// 只动集合内 `hidden_status = 'voice_discussion'` 的行；恢复即归位
/// （侧栏按 work_mode 原生聚合，恢复的会话自动并入当前同类型分组）。
pub fn restore_master_sessions(
    master_data_dir: &Path,
    raw_key: &str,
    session_ids: &[String],
    current_user_id: &str,
) -> Result<usize, MasterArchiveError> {
    if session_ids.is_empty() {
        return Ok(0);
    }
    let db_path = master_database_path(master_data_dir);
    if !db_path.is_file() {
        return Err(MasterArchiveError::DbUnavailable);
    }
    let conn = open_with_key_readwrite(&db_path, raw_key)?;
    verify_key_readable(&conn)?;
    if !hidden_status_column_exists(&conn) {
        return Err(MasterArchiveError::ArchiveUnsupported);
    }
    let sql = format!(
        "UPDATE chat_session SET hidden_status = NULL \
         WHERE hidden_status = '{ARCHIVE_HIDDEN_STATUS}' AND session_id IN ({}) \
         AND EXISTS (SELECT 1 FROM project p \
                    WHERE p.project_id = chat_session.project_id \
                      AND p.user_id = ?{})",
        placeholders(session_ids.len()),
        session_ids.len() + 1
    );
    let mut params: Vec<&str> = session_ids.iter().map(String::as_str).collect();
    params.push(current_user_id);
    let changed = conn
        .execute(&sql, rusqlite::params_from_iter(params))
        .map_err(|_| MasterArchiveError::WriteFailed)?;
    Ok(changed)
}

/// 真实删除会话（破坏性，单事务）：备份 → 内容三表 → chat_message →
/// chat_session → 空壳项目行。
///
/// 前置条件：主库 TRAE 实例已关闭（调用方检测，与切号/手动备份同纪律）。
/// 事务内任何一步失败整体回滚，库保持删除前状态。
pub fn delete_master_sessions(
    master_data_dir: &Path,
    raw_key: &str,
    session_ids: &[String],
    current_user_id: &str,
) -> Result<MasterDeleteOutcome, MasterArchiveError> {
    if session_ids.is_empty() {
        return Ok(MasterDeleteOutcome {
            deleted_sessions: 0,
            deleted_messages: 0,
            removed_projects: 0,
            backup_path: String::new(),
        });
    }
    let db_path = master_database_path(master_data_dir);
    if !db_path.is_file() {
        return Err(MasterArchiveError::DbUnavailable);
    }
    // 先确认 ID 属于当前账号；过期界面传入他人 ID 时直接空操作，
    // 连备份链也不新增，避免把无效请求误当成破坏性操作。
    let conn = open_with_key_readwrite(&db_path, raw_key)?;
    verify_key_readable(&conn)?;
    let authorized_ids = authorized_session_ids(&conn, session_ids, current_user_id)?;
    if authorized_ids.is_empty() {
        return Ok(MasterDeleteOutcome {
            deleted_sessions: 0,
            deleted_messages: 0,
            removed_projects: 0,
            backup_path: String::new(),
        });
    }
    drop(conn);

    // ADR-0018 铁律：破坏性批量操作前必须先备份（失败则删除不执行）。
    let backup = backup_master_trio(&db_path).map_err(|_| MasterArchiveError::BackupFailed)?;

    let conn = open_with_key_readwrite(&db_path, raw_key)?;
    verify_key_readable(&conn)?;
    let params: Vec<&str> = authorized_ids.iter().map(String::as_str).collect();
    let in_clause = placeholders(authorized_ids.len());

    // 受影响会话的所属项目（空壳判定范围；事务外读取无妨——同一连接串行）。
    let affected_projects: Vec<String> = {
        let sql = format!(
            "SELECT DISTINCT project_id FROM chat_session WHERE session_id IN ({in_clause})"
        );
        let mut statement = conn
            .prepare(&sql)
            .map_err(|_| MasterArchiveError::WriteFailed)?;
        let rows = statement
            .query_map(rusqlite::params_from_iter(params.iter()), |row| {
                row.get::<_, String>(0)
            })
            .map_err(|_| MasterArchiveError::WriteFailed)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| MasterArchiveError::WriteFailed)?;
        rows
    };

    let tx = conn
        .unchecked_transaction()
        .map_err(|_| MasterArchiveError::WriteFailed)?;
    let result = (|| -> Result<(usize, usize, usize), MasterArchiveError> {
        // 1. 消息内容三表（存在则删；关联键 message_id，随消息元数据行删除）。
        let mut deleted_messages = 0usize;
        for table in ["chat_message_general", "chat_message_task", "chat_message_chat"] {
            let table_exists: bool = tx
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
                    [table],
                    |row| row.get::<_, i64>(0),
                )
                .map(|count| count > 0)
                .map_err(|_| MasterArchiveError::WriteFailed)?;
            if !table_exists {
                continue;
            }
            let sql = format!(
                "DELETE FROM {table} WHERE message_id IN (\
                 SELECT message_id FROM chat_message WHERE session_id IN ({in_clause}))"
            );
            deleted_messages += tx
                .execute(&sql, rusqlite::params_from_iter(params.iter()))
                .map_err(|_| MasterArchiveError::WriteFailed)?;
        }
        // 2. 消息元数据行。
        let sql = format!("DELETE FROM chat_message WHERE session_id IN ({in_clause})");
        deleted_messages += tx
            .execute(&sql, rusqlite::params_from_iter(params.iter()))
            .map_err(|_| MasterArchiveError::WriteFailed)?;
        // 3. 会话行。
        let sql = format!("DELETE FROM chat_session WHERE session_id IN ({in_clause})");
        let deleted_sessions = tx
            .execute(&sql, rusqlite::params_from_iter(params.iter()))
            .map_err(|_| MasterArchiveError::WriteFailed)?;
        // 4. 空壳项目行：受影响项目中已无任何会话挂接的行。
        let project_params: Vec<&str> = affected_projects.iter().map(String::as_str).collect();
        let project_clause = placeholders(affected_projects.len());
        let sql = format!(
            "DELETE FROM project WHERE project_id IN ({project_clause}) \
             AND NOT EXISTS (SELECT 1 FROM chat_session \
             WHERE chat_session.project_id = project.project_id)"
        );
        let removed_projects = if affected_projects.is_empty() {
            0
        } else {
            tx.execute(&sql, rusqlite::params_from_iter(project_params))
                .map_err(|_| MasterArchiveError::WriteFailed)?
        };
        Ok((deleted_sessions, deleted_messages, removed_projects))
    })();

    match result {
        Ok((deleted_sessions, deleted_messages, removed_projects)) => {
            tx.commit().map_err(|_| MasterArchiveError::WriteFailed)?;
            Ok(MasterDeleteOutcome {
                deleted_sessions,
                deleted_messages,
                removed_projects,
                backup_path: backup.display().to_string(),
            })
        }
        Err(error) => {
            // 回滚失败也只能放弃：库由 SQLite 事务原子性保证（连接关闭时回滚）。
            let _ = tx.rollback();
            Err(error)
        }
    }
}

/// 分组合并回执（P5-8c）：规模供 UI 展示。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MasterMergeOutcome {
    /// 改挂到目标分组的会话数（含归档/软删会话——归位保持一致）。
    pub moved_sessions: usize,
    /// 被清理的空壳源分组行数。
    pub removed_projects: usize,
    /// 合并前自动创建的备份路径（人工恢复定位）。
    pub backup_path: String,
}

/// 合并分组（P5-8c，批量迁移属破坏性操作）：源分组的全部会话改挂
/// 目标分组（含归档/软删会话，归位语义一致），空壳源分组行随事务清理。
///
/// 前置条件：主库 TRAE 实例已关闭（调用方检测，与删除/切号同纪律）；
/// 执行前自动创建 `.switch-bak-*` 备份（ADR-0018 铁律，备份失败不执行）。
/// 事务内任何一步失败整体回滚，库保持合并前状态。
pub fn merge_master_projects(
    master_data_dir: &Path,
    raw_key: &str,
    source_project_ids: &[String],
    target_project_id: &str,
    current_user_id: &str,
) -> Result<MasterMergeOutcome, MasterArchiveError> {
    // 参数防御：目标不能在来源集合中（改挂目标必须是被保留的分组）。
    if source_project_ids.is_empty() || source_project_ids.iter().any(|id| id == target_project_id)
    {
        return Err(MasterArchiveError::MergeInvalid);
    }
    let db_path = master_database_path(master_data_dir);
    if !db_path.is_file() {
        return Err(MasterArchiveError::DbUnavailable);
    }
    let conn = open_with_key_readwrite(&db_path, raw_key)?;
    verify_key_readable(&conn)?;
    // 目标与来源分组都必须属于当前账号；过期界面传入他人分组时拒绝，
    // 防止把会话改挂到错误账号或清理他人空壳分组。
    let target_owned = conn
        .query_row(
            "SELECT COUNT(*) FROM project WHERE project_id = ?1 AND user_id = ?2",
            [target_project_id, current_user_id],
            |row| row.get::<_, i64>(0),
        )
        .map(|count| count > 0)
        .map_err(|_| MasterArchiveError::WriteFailed)?;
    if !target_owned {
        return Err(MasterArchiveError::MergeInvalid);
    }

    let source_params: Vec<&str> = source_project_ids.iter().map(String::as_str).collect();
    let source_clause = placeholders(source_project_ids.len());
    let source_owner_param = source_project_ids.len() + 1;
    let mut source_owner_params = source_params.clone();
    source_owner_params.push(current_user_id);
    let owned_source_count: i64 = conn
        .query_row(
            &format!(
                "SELECT COUNT(*) FROM project WHERE project_id IN ({source_clause}) \
                 AND user_id = ?{source_owner_param}"
            ),
            rusqlite::params_from_iter(source_owner_params),
            |row| row.get(0),
        )
        .map_err(|_| MasterArchiveError::WriteFailed)?;
    if owned_source_count != source_project_ids.len() as i64 {
        return Err(MasterArchiveError::MergeInvalid);
    }
    drop(conn);

    // ADR-0018 铁律：批量迁移前必须先备份（失败则合并不执行）。
    let backup = backup_master_trio(&db_path).map_err(|_| MasterArchiveError::BackupFailed)?;

    let conn = open_with_key_readwrite(&db_path, raw_key)?;
    verify_key_readable(&conn)?;

    let tx = conn
        .unchecked_transaction()
        .map_err(|_| MasterArchiveError::WriteFailed)?;
    let result = (|| -> Result<(usize, usize), MasterArchiveError> {
        // 1. 全部会话改挂目标分组（含归档/软删行——归位语义保持一致）。
        let sql = format!(
            "UPDATE chat_session SET project_id = ?1 WHERE project_id IN ({source_clause})"
        );
        let mut params: Vec<&str> = vec![target_project_id];
        params.extend(source_params.iter().copied());
        let moved_sessions = tx
            .execute(&sql, rusqlite::params_from_iter(params))
            .map_err(|_| MasterArchiveError::WriteFailed)?;
        // 2. 空壳源分组行清理（NOT EXISTS 守卫：与删除路径同款防御）。
        let sql = format!(
            "DELETE FROM project WHERE project_id IN ({source_clause}) \
             AND NOT EXISTS (SELECT 1 FROM chat_session \
             WHERE chat_session.project_id = project.project_id)"
        );
        let removed_projects = tx
            .execute(&sql, rusqlite::params_from_iter(source_params))
            .map_err(|_| MasterArchiveError::WriteFailed)?;
        Ok((moved_sessions, removed_projects))
    })();

    match result {
        Ok((moved_sessions, removed_projects)) => {
            tx.commit().map_err(|_| MasterArchiveError::WriteFailed)?;
            Ok(MasterMergeOutcome {
                moved_sessions,
                removed_projects,
                backup_path: backup.display().to_string(),
            })
        }
        Err(error) => {
            // 回滚失败也只能放弃：库由 SQLite 事务原子性保证（连接关闭时回滚）。
            let _ = tx.rollback();
            Err(error)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::OpenFlags;

    /// 构造加密主库 fixture：两项目（p1 挂两会话，p2 挂一会话）+ 消息与内容表。
    fn create_archive_fixture(db_path: &Path, raw_key: &str) {
        let flags = OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE;
        let conn = Connection::open_with_flags(db_path, flags).unwrap();
        let pragma = format!("PRAGMA key = \"x'{}'\";", raw_key);
        conn.execute_batch(&pragma).unwrap();
        conn.execute_batch(
            "CREATE TABLE project (project_id TEXT PRIMARY KEY, user_id TEXT, biz_project_id TEXT, name TEXT, deleted_at INTEGER);
             CREATE TABLE chat_session (session_id TEXT PRIMARY KEY, project_id TEXT, session_title TEXT, updated_at INTEGER, deleted_at INTEGER, hidden_status TEXT, work_mode TEXT);
             CREATE TABLE chat_message (message_id TEXT PRIMARY KEY, session_id TEXT, message_role TEXT, message_type TEXT, created_at INTEGER, deleted_at INTEGER);
             CREATE TABLE chat_message_general (message_id TEXT PRIMARY KEY, content TEXT);
             CREATE TABLE chat_message_task (message_id TEXT PRIMARY KEY, content TEXT);
             INSERT INTO project VALUES ('p1', '111', 'biz-1', '项目甲', 0);
             INSERT INTO project VALUES ('p2', '111', 'biz-2', '项目乙', 0);
             INSERT INTO project VALUES ('p3', '222', 'biz-3', '他人项目', 0);
             INSERT INTO chat_session VALUES ('s1', 'p1', '会话一', 1770000000000, 0, NULL, NULL);
             INSERT INTO chat_session VALUES ('s2', 'p1', '会话二', 1769000000000, 0, NULL, 'work');
             INSERT INTO chat_session VALUES ('s3', 'p2', '会话三', 1768000000000, 0, 'voice_discussion', NULL);
             INSERT INTO chat_session VALUES ('s4', 'p3', '他人会话', 1771000000000, 0, NULL, NULL);
             INSERT INTO chat_message VALUES ('m1', 's1', 'user', 'text', 1, NULL);
             INSERT INTO chat_message VALUES ('m2', 's1', 'assistant', 'task', 2, NULL);
             INSERT INTO chat_message_general VALUES ('m1', '{\"blocks\":[]}');
             INSERT INTO chat_message_task VALUES ('m2', '{}');
             INSERT INTO chat_message VALUES ('m3', 's3', 'user', 'text', 3, NULL);
             INSERT INTO chat_message_general VALUES ('m3', '{}');",
        )
        .unwrap();
    }

    fn temp_dir(label: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "trae-sync-p58a-{}-{}-{}",
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

    fn hidden_status_of(conn: &Connection, session_id: &str) -> Option<String> {
        conn.query_row(
            "SELECT hidden_status FROM chat_session WHERE session_id = ?1",
            [session_id],
            |row| row.get(0),
        )
        .unwrap()
    }

    #[test]
    fn archive_and_restore_roundtrip() {
        let dir = temp_dir("roundtrip");
        let key = "aa".repeat(32);
        create_archive_fixture(&master_database_path(&dir), &key);

        // 归档：s1（NULL → voice_discussion）；s3 原生已隐藏不动。
        let affected =
            archive_master_sessions(&dir, &key, &["s1".into(), "s3".into()], "111").unwrap();
        assert_eq!(affected, 1);
        let flags = OpenFlags::SQLITE_OPEN_READ_ONLY;
        let conn = Connection::open_with_flags(master_database_path(&dir), flags).unwrap();
        conn.execute_batch(&format!("PRAGMA key = \"x'{key}'\";")).unwrap();
        assert_eq!(
            hidden_status_of(&conn, "s1").as_deref(),
            Some(ARCHIVE_HIDDEN_STATUS)
        );

        // 恢复：s1 还原 NULL。
        let affected = restore_master_sessions(&dir, &key, &["s1".into()], "111").unwrap();
        assert_eq!(affected, 1);
        assert_eq!(hidden_status_of(&conn, "s1"), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn archive_restore_and_delete_ignore_other_account_sessions() {
        let dir = temp_dir("owner");
        let key = "ab".repeat(32);
        create_archive_fixture(&master_database_path(&dir), &key);

        // 当前账号 111 不能通过过期/伪造的 ID 操作账号 222 的会话。
        assert_eq!(
            archive_master_sessions(&dir, &key, &["s4".into()], "111").unwrap(),
            0
        );
        assert_eq!(
            restore_master_sessions(&dir, &key, &["s4".into()], "111").unwrap(),
            0
        );
        let outcome = delete_master_sessions(&dir, &key, &["s4".into()], "111").unwrap();
        assert_eq!(outcome.deleted_sessions, 0);
        assert!(outcome.backup_path.is_empty(), "无授权目标不应创建备份");

        let conn = Connection::open_with_flags(
            master_database_path(&dir),
            OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .unwrap();
        conn.execute_batch(&format!("PRAGMA key = \"x'{key}'\";"))
            .unwrap();
        let session_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM chat_session WHERE session_id = 's4'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(session_count, 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn archive_missing_db_reports_unavailable() {
        let dir = temp_dir("missing");
        assert_eq!(
            archive_master_sessions(&dir, &"bb".repeat(32), &["s1".into()], "111"),
            Err(MasterArchiveError::DbUnavailable)
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn archive_wrong_key_reports_open_failed() {
        let dir = temp_dir("wrongkey");
        create_archive_fixture(&master_database_path(&dir), &"cc".repeat(32));
        assert_eq!(
            archive_master_sessions(&dir, &"dd".repeat(32), &["s1".into()], "111"),
            Err(MasterArchiveError::DbOpenFailed)
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn archive_without_hidden_status_column_reports_unsupported() {
        let dir = temp_dir("nohidden");
        let key = "ee".repeat(32);
        let db_path = master_database_path(&dir);
        let flags = OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE;
        let conn = Connection::open_with_flags(&db_path, flags).unwrap();
        conn.execute_batch(&format!("PRAGMA key = \"x'{key}'\";")).unwrap();
        conn.execute_batch(
            "CREATE TABLE chat_session (session_id TEXT PRIMARY KEY, project_id TEXT);
             INSERT INTO chat_session VALUES ('s1', 'p1');",
        )
        .unwrap();
        drop(conn);
        assert_eq!(
            archive_master_sessions(&dir, &key, &["s1".into()], "111"),
            Err(MasterArchiveError::ArchiveUnsupported)
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn delete_removes_messages_sessions_and_empty_projects_with_backup() {
        let dir = temp_dir("delete");
        let key = "ff".repeat(32);
        create_archive_fixture(&master_database_path(&dir), &key);

        // 删除 s1（p1 还挂 s2，p1 保留）与 s3（p2 变空壳被清理）。
        let outcome =
            delete_master_sessions(&dir, &key, &["s1".into(), "s3".into()], "111").unwrap();
        assert_eq!(outcome.deleted_sessions, 2);
        // 消息元数据 3 行（m1/m2/m3）+ 内容行 3 行（m1 general、m2 task、m3 general）。
        assert_eq!(outcome.deleted_messages, 6);
        assert_eq!(outcome.removed_projects, 1);
        assert!(outcome.backup_path.contains(".switch-bak-"));

        let flags = OpenFlags::SQLITE_OPEN_READ_ONLY;
        let conn = Connection::open_with_flags(master_database_path(&dir), flags).unwrap();
        conn.execute_batch(&format!("PRAGMA key = \"x'{key}'\";")).unwrap();
        let sessions: i64 = conn
            .query_row("SELECT COUNT(*) FROM chat_session", [], |row| row.get(0))
            .unwrap();
        assert_eq!(sessions, 2, "s2 与他人会话 s4 应保留");
        let projects: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM project WHERE project_id IN ('p1', 'p2')",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(projects, 1, "p1（仍挂 s2）保留，空壳 p2 清理");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn delete_missing_db_reports_unavailable_without_backup() {
        let dir = temp_dir("deletemissing");
        assert_eq!(
            delete_master_sessions(&dir, &"aa".repeat(32), &["s1".into()], "111"),
            Err(MasterArchiveError::DbUnavailable)
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn merge_moves_all_sessions_and_removes_source_projects_with_backup() {
        let dir = temp_dir("merge");
        let key = "11".repeat(32);
        create_archive_fixture(&master_database_path(&dir), &key);

        // p2（含归档会话 s3）并入 p1：全部会话改挂，p2 空壳清理。
        let outcome = merge_master_projects(&dir, &key, &["p2".into()], "p1", "111").unwrap();
        assert_eq!(outcome.moved_sessions, 1);
        assert_eq!(outcome.removed_projects, 1);
        assert!(outcome.backup_path.contains(".switch-bak-"));

        let flags = OpenFlags::SQLITE_OPEN_READ_ONLY;
        let conn = Connection::open_with_flags(master_database_path(&dir), flags).unwrap();
        conn.execute_batch(&format!("PRAGMA key = \"x'{key}'\";")).unwrap();
        // s3 归档会话同样改挂（归位语义一致），归属列不变。
        let (project_of_s3, user_of_p2): (String, Option<String>) = conn
            .query_row(
                "SELECT s.project_id, (SELECT user_id FROM project WHERE project_id = 'p2') \
                 FROM chat_session s WHERE s.session_id = 's3'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(project_of_s3, "p1");
        assert_eq!(user_of_p2, None, "空壳源分组 p2 应被清理");
        // 消息行不受合并影响（fixture 共 3 行：m1/m2/m3）。
        let messages: i64 = conn
            .query_row("SELECT COUNT(*) FROM chat_message", [], |row| row.get(0))
            .unwrap();
        assert_eq!(messages, 3);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn merge_rejects_target_in_sources_and_missing_target() {
        let dir = temp_dir("mergeinvalid");
        let key = "22".repeat(32);
        create_archive_fixture(&master_database_path(&dir), &key);

        // 目标在来源集合中：直接拒绝（不改库）。
        assert_eq!(
            merge_master_projects(&dir, &key, &["p1".into()], "p1", "111"),
            Err(MasterArchiveError::MergeInvalid)
        );
        // 目标分组不存在（UI 数据过期场景）：拒绝。
        assert_eq!(
            merge_master_projects(&dir, &key, &["p1".into()], "p-missing", "111"),
            Err(MasterArchiveError::MergeInvalid)
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn merge_rejects_other_account_projects() {
        let dir = temp_dir("mergeowner");
        let key = "55".repeat(32);
        create_archive_fixture(&master_database_path(&dir), &key);

        assert_eq!(
            merge_master_projects(&dir, &key, &["p3".into()], "p1", "111"),
            Err(MasterArchiveError::MergeInvalid)
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn merge_wrong_key_reports_open_failed() {
        let dir = temp_dir("mergewrongkey");
        create_archive_fixture(&master_database_path(&dir), &"33".repeat(32));
        assert_eq!(
            merge_master_projects(&dir, &"44".repeat(32), &["p2".into()], "p1", "111"),
            Err(MasterArchiveError::DbOpenFailed)
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
