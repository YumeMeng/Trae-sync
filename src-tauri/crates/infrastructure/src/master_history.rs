//! P5-3 主库历史视图：项目 + 会话两栏数据源（只读）。
//!
//! 环境模型（`.scratch/grill-log-20260830-env-model.md` Q4/Q5）：
//! - 历史页数据源 = 主库（2026-08-31 修订：官方 TRAE Work CN 默认数据目录
//!   `%APPDATA%\TRAE SOLO CN`），全部对话记录归主库；正常会话按
//!   `project.user_id` 过滤当前登录账号，归档会话作为全局主库内容展示。
//! - 左栏项目列表来自 `project` 表（`name` 列为展示名，列缺失降级空串）；
//!   右栏会话来自 `chat_session`（标题/时间/软删/消息数，口径与
//!   `account_session_index` 一致：列防御 + 毫秒/秒双单位归一化）。
//! - 只读打开复用 `sqlcipher::open_with_key_readonly` 的隔离三件套机制
//!   （WAL 兼容，主库实例运行中可随时读取，不阻塞 TRAE 写入）。
//!
//! 准实时新鲜度：`stat_master_fingerprint` 只 stat 三件套 mtime/size
//! （不复制不读取），前端轮询比对，变化才触发 `read_master_history` 全量
//! 重读——主库按正常/归档状态读取，无需高水位增量。

use std::path::Path;

use rusqlite::Connection;
use serde::{Deserialize, Serialize};

use crate::account_session_index::{stat_file_fingerprint, InstanceFingerprint};
use crate::master_handover::master_database_path;
use crate::sqlcipher::open_with_key_readonly;

/// 左栏项目条目。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MasterProjectEntry {
    pub project_id: String,
    /// 项目展示名：`project.name` 可读时直接用；哈希 ID/空名回退
    /// `absolute_path` 尾段（titlepeek 探针实测：约半数项目 name 为
    /// 24 位哈希）；仍不可读为空串（前端占位「未关联文件夹」）。
    pub name: String,
    /// 项目文件夹绝对路径（悬浮提示展示；列缺失或为空为 None）。
    pub absolute_path: Option<String>,
}

/// 右栏会话条目：索引模块会话摘要 + project_id 维度（扁平形态，DTO 直接映射）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MasterSessionEntry {
    pub session_id: String,
    /// 所属项目（左栏联动筛选键）。
    pub project_id: String,
    pub title: String,
    pub message_count: u32,
    pub updated_at_unix_seconds: Option<i64>,
    pub deleted: bool,
    /// 会话隐藏状态（TRAE 原生枚举）：`voice_discussion` 借用为归档
    /// （ADR-0022），`scheduled_task` 为原生过滤值；NULL = 正常显示。
    /// 主列表过滤与归档视图收纳均由前端按此字段判定。
    pub hidden_status: Option<String>,
    /// 会话模式（code/work）：`COALESCE(s.work_mode, p.work_mode)`；
    /// 归档视图按「模式 → 分组 → 会话」层级收纳（ADR-0022 决策 2）。
    pub work_mode: Option<String>,
}

/// 主库历史读取结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MasterHistoryStatus {
    /// 读取成功（可能为空列表）。
    Ready {
        projects: Vec<MasterProjectEntry>,
        sessions: Vec<MasterSessionEntry>,
    },
    /// 主库从未启动过（无 database.db）——正常状态而非错误。
    NoMasterData,
    /// 打开或读取失败（key 不匹配、文件损坏等）。
    ReadFailed,
}

/// 检测表列是否存在；缺失列触发降级而非整体失败（与索引模块同款）。
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

/// 毫秒/秒双单位时间戳归一化为秒（阈值口径与索引模块一致）。
fn normalize_timestamp_to_seconds(raw: i64) -> i64 {
    if raw > 1_000_000_000_000 {
        raw / 1000
    } else {
        raw
    }
}

/// 判断字符串是否为不透明哈希 ID（≥16 位十六进制、无分隔符）。
/// TRAE 内部虚拟项目（work-mode-projects）的 name 与路径尾段均为该形态，
/// 不适合作为项目名展示（界面表达纪律：编号不当主信息）。
fn is_opaque_hash_id(value: &str) -> bool {
    let trimmed = value.trim();
    trimmed.len() >= 16 && trimmed.bytes().all(|b| b.is_ascii_hexdigit())
}

/// 提取路径最后一段（兼容 `\` 与 `/` 分隔，忽略尾部斜杠）。
fn path_last_segment(path: &str) -> Option<&str> {
    let trimmed = path.trim().trim_end_matches(['\\', '/']);
    let segment = trimmed.rsplit(['\\', '/']).next()?;
    (!segment.is_empty()).then_some(segment)
}

/// 项目展示名：name 可读优先；哈希/空名回退 absolute_path 尾段；
/// 两者皆不可读为空串（前端统一占位「未关联文件夹」并按名合并展示）。
fn project_display_name(name: &str, absolute_path: Option<&str>) -> String {
    let trimmed = name.trim();
    if !trimmed.is_empty() && !is_opaque_hash_id(trimmed) {
        return trimmed.to_string();
    }
    if let Some(segment) = absolute_path.and_then(path_last_segment) {
        if !is_opaque_hash_id(segment) {
            return segment.to_string();
        }
    }
    String::new()
}

/// 读取主库历史：当前账号的正常内容 + 全局归档内容。
///
/// 会话条目为索引模块会话摘要追加 `project_id` 维度后的扁平形态
/// （左栏联动筛选键；结构口径与 `account_session_index` 一致）。
pub fn read_master_history(
    master_data_dir: &Path,
    raw_key: &str,
    current_user_id: &str,
) -> MasterHistoryStatus {
    match read_master_history_inner(master_data_dir, raw_key, current_user_id) {
        Some((projects, sessions)) => MasterHistoryStatus::Ready { projects, sessions },
        // db 缺失（None）与 db 存在但打开失败（ReadFailed 内部映射）在此区分：
        // stat_unavailable 按文件存在性归位。
        None => stat_unavailable(master_data_dir),
    }
}

/// 无主库数据时按文件存在性区分状态（db 缺失 = NoMasterData，否则 ReadFailed）。
fn stat_unavailable(master_data_dir: &Path) -> MasterHistoryStatus {
    if master_database_path(master_data_dir).is_file() {
        MasterHistoryStatus::ReadFailed
    } else {
        MasterHistoryStatus::NoMasterData
    }
}

/// 内层读取：项目 + 带 project_id 的会话列表；db 缺失或读取失败返回 None
/// （由调用方按文件存在性区分 NoMasterData / ReadFailed）。
fn read_master_history_inner(
    master_data_dir: &Path,
    raw_key: &str,
    current_user_id: &str,
) -> Option<(Vec<MasterProjectEntry>, Vec<MasterSessionEntry>)> {
    let db_path = master_database_path(master_data_dir);
    if !db_path.is_file() {
        // 主库未启动过：交调用方标记 NoMasterData（历史页显示启动引导）。
        return None;
    }
    let conn = open_with_key_readonly(&db_path, raw_key).ok()?;

    // 归档内容不再受项目归属过滤，但仍通过 project_id 回到所属项目分组。
    // 老库没有 hidden_status 时保持旧的“按账号过滤”行为。
    let has_hidden_status = column_exists(&conn, "chat_session", "hidden_status");

    // 项目：name / absolute_path / deleted_at 列防御（schema 契约只保证核心列）。
    let has_project_name = column_exists(&conn, "project", "name");
    let has_project_path = column_exists(&conn, "project", "absolute_path");
    let has_project_deleted = column_exists(&conn, "project", "deleted_at");
    let name_expr = if has_project_name {
        "COALESCE(p.name, '')"
    } else {
        "''"
    };
    let path_expr = if has_project_path {
        "p.absolute_path"
    } else {
        "NULL"
    };
    let project_deleted_expr = if has_project_deleted {
        "COALESCE(p.deleted_at, 0)"
    } else {
        "0"
    };
    // G19 会话可见性子查询：至少 1 个未删除会话的项目才进列表（0 会话
    // 空壳——TRAE 自动创建的哈希名虚拟项目——被过滤，不删任何数据）。
    // 归档会话（hidden_status 借用值）算可见：归档视图仍按项目分组展示。
    let has_session_deleted = column_exists(&conn, "chat_session", "deleted_at");
    let session_visible_expr = if has_session_deleted {
        "COALESCE(s.deleted_at, 0) = 0"
    } else {
        "1 = 1"
    };
    let project_scope_expr = if has_hidden_status {
        "(p.user_id = ?1 OR EXISTS (SELECT 1 FROM chat_session archived_s \
            WHERE archived_s.project_id = p.project_id \
              AND archived_s.hidden_status = 'voice_discussion'))"
    } else {
        "p.user_id = ?1"
    };
    let mut statement = conn
        .prepare(&format!(
            "SELECT p.project_id, {name_expr}, {path_expr} FROM project p \
             WHERE {project_scope_expr} AND {project_deleted_expr} = 0 \
             AND EXISTS (SELECT 1 FROM chat_session s \
                 WHERE s.project_id = p.project_id AND {session_visible_expr}) \
             ORDER BY p.project_id ASC"
        ))
        .ok()?;
    let projects: Vec<MasterProjectEntry> = statement
        .query_map([current_user_id], |row| {
            let raw_name: String = row.get(1)?;
            let absolute_path: Option<String> = row.get(2)?;
            Ok(MasterProjectEntry {
                project_id: row.get(0)?,
                // 哈希/空名回退路径尾段（见 project_display_name 注释）。
                name: project_display_name(&raw_name, absolute_path.as_deref()),
                absolute_path,
            })
        })
        .ok()?
        .filter_map(|row| row.ok())
        .collect();

    // 会话：与 account_session_index::query_session_summaries 同口径的
    // 列防御（标题/时间/软删/消息软删），追加 project_id / hidden_status /
    // work_mode 维度（P5-8a 归档通道，ADR-0022）。
    let has_title = column_exists(&conn, "chat_session", "session_title");
    let has_updated_at = column_exists(&conn, "chat_session", "updated_at");
    // has_session_deleted 已在项目查询前判定（G19 EXISTS 子查询共用）。
    let has_message_deleted = column_exists(&conn, "chat_message", "deleted_at");
    let has_session_work_mode = column_exists(&conn, "chat_session", "work_mode");
    let has_project_work_mode = column_exists(&conn, "project", "work_mode");
    let title_expr = if has_title {
        "COALESCE(s.session_title, '')"
    } else {
        "''"
    };
    let updated_expr = if has_updated_at {
        "s.updated_at"
    } else {
        "NULL"
    };
    let deleted_expr = if has_session_deleted {
        "COALESCE(s.deleted_at, 0)"
    } else {
        "0"
    };
    let hidden_expr = if has_hidden_status {
        "s.hidden_status"
    } else {
        "NULL"
    };
    // 模式归属由会话与项目两列共同决定（TECHNICAL_BASELINE 实测）：
    // 会话列优先，缺失回退项目列；两列皆缺为 NULL（前端归「其他」桶）。
    let work_mode_expr = match (has_session_work_mode, has_project_work_mode) {
        (true, true) => "COALESCE(s.work_mode, p.work_mode)",
        (true, false) => "s.work_mode",
        (false, true) => "p.work_mode",
        (false, false) => "NULL",
    };
    let message_count_expr = if has_message_deleted {
        "(SELECT COUNT(*) FROM chat_message m \
          WHERE m.session_id = s.session_id \
          AND (m.deleted_at IS NULL OR m.deleted_at = 0))"
    } else {
        "(SELECT COUNT(*) FROM chat_message m WHERE m.session_id = s.session_id)"
    };
    let session_scope_expr = if has_hidden_status {
        "(p.user_id = ?1 OR s.hidden_status = 'voice_discussion')"
    } else {
        "p.user_id = ?1"
    };
    let mut statement = conn
        .prepare(&format!(
            "SELECT s.session_id, s.project_id, {title_expr}, {message_count_expr}, \
             {updated_expr}, {deleted_expr}, {hidden_expr}, {work_mode_expr} \
             FROM chat_session s JOIN project p ON p.project_id = s.project_id \
             WHERE {session_scope_expr} \
             ORDER BY {updated_expr} DESC NULLS LAST, s.session_id ASC"
        ))
        .ok()?;
    let sessions: Vec<MasterSessionEntry> = statement
        .query_map([current_user_id], |row| {
            let updated_at_raw: Option<i64> = row.get(4)?;
            Ok(MasterSessionEntry {
                session_id: row.get(0)?,
                project_id: row.get(1)?,
                title: row.get(2)?,
                message_count: row.get::<_, i64>(3)?.max(0) as u32,
                updated_at_unix_seconds: updated_at_raw.map(normalize_timestamp_to_seconds),
                deleted: row.get::<_, i64>(5)? != 0,
                hidden_status: row.get(6)?,
                work_mode: row.get(7)?,
            })
        })
        .ok()?
        .filter_map(|row| row.ok())
        .collect();

    Some((projects, sessions))
}

/// stat 主库三件套指纹（只 stat，不复制不读取；轮询预检用）。
pub fn stat_master_fingerprint(master_data_dir: &Path) -> InstanceFingerprint {
    let db_path = master_database_path(master_data_dir);
    InstanceFingerprint {
        db: stat_file_fingerprint(&db_path),
        wal: stat_file_fingerprint(&db_path.with_file_name(format!(
            "{}-wal",
            db_path.file_name().unwrap_or_default().to_string_lossy()
        ))),
        shm: stat_file_fingerprint(&db_path.with_file_name(format!(
            "{}-shm",
            db_path.file_name().unwrap_or_default().to_string_lossy()
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::OpenFlags;

    /// 构造加密主库 fixture：两账号各挂项目与会话。
    fn create_master_fixture(db_path: &Path, raw_key: &str) {
        let flags = OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE;
        let conn = Connection::open_with_flags(db_path, flags).unwrap();
        let pragma = format!("PRAGMA key = \"x'{}'\";", raw_key);
        conn.execute_batch(&pragma).unwrap();
        conn.execute_batch(
            "CREATE TABLE project (project_id TEXT PRIMARY KEY, user_id TEXT, biz_project_id TEXT, name TEXT, absolute_path TEXT, deleted_at INTEGER, work_mode TEXT);
             CREATE TABLE chat_session (session_id TEXT PRIMARY KEY, project_id TEXT, session_title TEXT, updated_at INTEGER, deleted_at INTEGER, hidden_status TEXT, work_mode TEXT);
             CREATE TABLE chat_message (message_id TEXT PRIMARY KEY, session_id TEXT, deleted_at INTEGER);
             INSERT INTO project VALUES ('p1', '111', 'biz-1', '项目甲', 'd:\\work\\甲', 0, 'code');
             INSERT INTO project VALUES ('p2', '111', 'biz-2', '项目乙', NULL, 500, NULL);
             INSERT INTO project VALUES ('p3', '222', 'biz-3', '他人项目', NULL, 0, NULL);
             INSERT INTO project VALUES ('p4', '111', 'biz-4', '6a4f4c489f4ebc1d4b0a2834', 'd:\\work\\Zed', 0, 'code');
             INSERT INTO project VALUES ('p5', '111', 'biz-5', '6a4b8379fdd775ba95fb7f63', 'C:\\app\\work-mode-projects\\6a4b8379fdd775ba95fb7f63', 0, 'work');
             INSERT INTO project VALUES ('p6', '111', 'biz-6', '', 'd:\\work\\Trae-sync', 0, NULL);
             INSERT INTO chat_session VALUES ('s1', 'p1', '会话一', 1770000000000, 0, NULL, NULL);
             INSERT INTO chat_session VALUES ('s2', 'p1', '会话二', 1769000000000, 0, 'voice_discussion', NULL);
             INSERT INTO chat_session VALUES ('s3', 'p3', '他人会话', 1771000000000, 0, NULL, 'work');
             INSERT INTO chat_message VALUES ('m1', 's1', NULL);
             INSERT INTO chat_message VALUES ('m2', 's1', 99);",
        )
        .unwrap();
    }

    fn temp_master_dir(label: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "trae-sync-p53-{}-{}-{}",
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

    /// G19：0 会话空壳项目被过滤——TRAE 自动创建的哈希名虚拟项目不再堆积。
    #[test]
    fn zero_session_projects_are_filtered_out() {
        let dir = temp_master_dir("g19-zero-session");
        let key = "11".repeat(32);
        create_master_fixture(&master_database_path(&dir), &key);
        // 追加两个 0 会话空壳项目：一个正常名，一个哈希名无路径。
        {
            let conn = Connection::open_with_flags(
                master_database_path(&dir),
                OpenFlags::SQLITE_OPEN_READ_WRITE,
            )
            .unwrap();
            let pragma = format!("PRAGMA key = \"x'{}'\";", key);
            conn.execute_batch(&pragma).unwrap();
            conn.execute(
                "INSERT INTO project VALUES ('p7', '111', 'biz-7', '空壳项目', NULL, 0, NULL)",
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO project VALUES ('p8', '111', 'biz-8', '6a4b8379fdd775ba95fb7f63', NULL, 0, NULL)",
                [],
            )
            .unwrap();
        }

        let status = read_master_history(&dir, &key, "111");
        match status {
            MasterHistoryStatus::Ready { projects, .. } => {
                // p7/p8 无任何会话：不进列表（fixture 中 p4 哈希名真路径也无会话，
                // 同被过滤；保留的是有会话的 p1 与空名有路径但同样无会话的
                // p6——p6 也无会话，被过滤后只剩 p1）。
                let ids: Vec<&str> = projects.iter().map(|p| p.project_id.as_str()).collect();
                assert_eq!(ids, vec!["p1"], "0 会话项目必须全部被过滤");
            }
            other => panic!("期望 Ready，实际 {:?}", other),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// G19：无路径但有会话的项目归并「未关联文件夹」分组（复用按名合并机制：
    /// 展示名空串 + 无路径，前端按 name 分组时自然归并）。
    #[test]
    fn no_path_project_with_sessions_kept_for_unlinked_group() {
        let dir = temp_master_dir("g19-unlinked");
        let key = "22".repeat(32);
        create_master_fixture(&master_database_path(&dir), &key);
        // 无路径 + 空名项目，挂一个可见会话：必须保留（归并入口由前端处理）。
        {
            let conn = Connection::open_with_flags(
                master_database_path(&dir),
                OpenFlags::SQLITE_OPEN_READ_WRITE,
            )
            .unwrap();
            let pragma = format!("PRAGMA key = \"x'{}'\";", key);
            conn.execute_batch(&pragma).unwrap();
            conn.execute(
                "INSERT INTO project VALUES ('p9', '111', 'biz-9', '', NULL, 0, NULL)",
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO chat_session VALUES ('s9', 'p9', '未关联会话', 1772000000000, 0, NULL, NULL)",
                [],
            )
            .unwrap();
        }

        let status = read_master_history(&dir, &key, "111");
        match status {
            MasterHistoryStatus::Ready { projects, sessions } => {
                let p9 = projects
                    .iter()
                    .find(|p| p.project_id == "p9")
                    .expect("有会话的无路径项目必须保留");
                assert_eq!(p9.name, "");
                assert_eq!(p9.absolute_path, None);
                let s9 = sessions
                    .iter()
                    .find(|s| s.session_id == "s9")
                    .expect("未关联项目的会话必须可见");
                assert_eq!(s9.project_id, "p9");
            }
            other => panic!("期望 Ready，实际 {:?}", other),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn reads_projects_and_sessions_filtered_by_current_user() {
        let dir = temp_master_dir("filter");
        let key = "aa".repeat(32);
        create_master_fixture(&master_database_path(&dir), &key);
        // G19 后项目需至少 1 个会话才进列表：为 p4/p5/p6 各补一个会话，
        // 保持名称回退规则在全读取链路中的断言覆盖。
        {
            let conn = Connection::open_with_flags(
                master_database_path(&dir),
                OpenFlags::SQLITE_OPEN_READ_WRITE,
            )
            .unwrap();
            let pragma = format!("PRAGMA key = \"x'{}'\";", key);
            conn.execute_batch(&pragma).unwrap();
            conn.execute(
                "INSERT INTO chat_session VALUES ('s4', 'p4', '会话四', 1773000000000, 0, NULL, NULL)",
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO chat_session VALUES ('s5', 'p5', '会话五', 1774000000000, 0, NULL, NULL)",
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO chat_session VALUES ('s6', 'p6', '会话六', 1775000000000, 0, NULL, NULL)",
                [],
            )
            .unwrap();
        }

        let status = read_master_history(&dir, &key, "111");
        match status {
            MasterHistoryStatus::Ready { projects, sessions } => {
                // 只见当前账号且有会话的项目（软删/他人/0 会话项目排除），
                // 按 project_id 排序。
                assert_eq!(projects.len(), 4);
                assert_eq!(projects[0].name, "项目甲");
                assert_eq!(projects[0].absolute_path.as_deref(), Some("d:\\work\\甲"));
                // 哈希名 + 真实路径：回退路径尾段。
                assert_eq!(projects[1].name, "Zed");
                // 哈希名 + 哈希路径（虚拟项目）：无可读名，空串交前端占位
                // （有会话仍保留，前端归并「未关联文件夹」分组）。
                assert_eq!(projects[2].name, "");
                // 空名 + 真实路径：回退路径尾段。
                assert_eq!(projects[3].name, "Trae-sync");
                // 只见当前账号的会话（他人会话不可见）；毫秒时间归一化为秒。
                assert_eq!(sessions.len(), 5);
                // 按更新时间倒序：新补的 s6 最新排首，s1/s2 用 find 定位断言。
                assert_eq!(sessions[0].session_id, "s6");
                let s1 = sessions.iter().find(|s| s.session_id == "s1").unwrap();
                assert_eq!(s1.updated_at_unix_seconds, Some(1770000000));
                // 软删消息（deleted_at=99）不计入消息数。
                assert_eq!(s1.message_count, 1);
                // P5-8a 归档维度：hidden_status 借用值与会话/项目回退模式可读。
                assert_eq!(s1.hidden_status, None);
                assert_eq!(s1.work_mode.as_deref(), Some("code"));
                let s2 = sessions.iter().find(|s| s.session_id == "s2").unwrap();
                assert_eq!(s2.hidden_status.as_deref(), Some("voice_discussion"));
                // 会话 work_mode 缺失时回退项目 work_mode（COALESCE 口径）。
                assert_eq!(s2.work_mode.as_deref(), Some("code"));
            }
            other => panic!("期望 Ready，实际 {:?}", other),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn archived_sessions_are_visible_across_accounts_but_normal_sessions_are_not() {
        let dir = temp_master_dir("global-archive");
        let key = "ab".repeat(32);
        create_master_fixture(&master_database_path(&dir), &key);
        {
            let conn = Connection::open_with_flags(
                master_database_path(&dir),
                OpenFlags::SQLITE_OPEN_READ_WRITE,
            )
            .unwrap();
            let pragma = format!("PRAGMA key = \"x'{}'\";", key);
            conn.execute_batch(&pragma).unwrap();
            conn.execute(
                "INSERT INTO project VALUES ('p7', '222', 'biz-7', '他人归档项目', NULL, 0, NULL)",
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO chat_session VALUES ('s7', 'p7', '他人归档会话', 1773000000000, 0, 'voice_discussion', NULL)",
                [],
            )
            .unwrap();
        }

        let status = read_master_history(&dir, &key, "111");
        match status {
            MasterHistoryStatus::Ready { projects, sessions } => {
                assert!(projects.iter().any(|project| project.project_id == "p7"));
                assert!(sessions.iter().any(|session| session.session_id == "s7"));
                assert!(!sessions.iter().any(|session| session.session_id == "s3"));
            }
            other => panic!("期望 Ready，实际 {:?}", other),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn project_display_name_derivation_rules() {
        // name 可读：直接用（前后空白裁剪）。
        assert_eq!(project_display_name(" Trae-sync ", None), "Trae-sync");
        // 哈希名（≥16 位十六进制）：回退路径尾段。
        assert_eq!(
            project_display_name("6a4b8379fdd775ba95fb7f63", Some("d:\\work\\Zed")),
            "Zed"
        );
        // 正斜杠路径与尾部斜杠：尾段提取同样成立。
        assert_eq!(project_display_name("", Some("/home/user/proj/")), "proj");
        // 哈希名 + 哈希尾段（虚拟项目）：空串。
        assert_eq!(
            project_display_name(
                "6a4b8379fdd775ba95fb7f63",
                Some("C:\\app\\work-mode-projects\\6a4b8379fdd775ba95fb7f63")
            ),
            ""
        );
        // 空名 + 空路径：空串。
        assert_eq!(project_display_name("", Some("")), "");
        assert_eq!(project_display_name("", None), "");
        // 短十六进制串（如 8 位）不是哈希 ID：按可读名处理。
        assert_eq!(project_display_name("deadbeef", None), "deadbeef");
    }

    #[test]
    fn missing_master_db_reports_no_master_data() {
        // 主库从未启动过：无 db → NoMasterData（历史页显示启动主库引导）。
        let dir = temp_master_dir("missing");
        let status = read_master_history(&dir, &"bb".repeat(32), "111");
        assert_eq!(status, MasterHistoryStatus::NoMasterData);
        // 指纹全 None（db 不存在）。
        assert_eq!(stat_master_fingerprint(&dir).db, None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn wrong_key_reports_read_failed() {
        let dir = temp_master_dir("wrongkey");
        create_master_fixture(&master_database_path(&dir), &"cc".repeat(32));
        let status = read_master_history(&dir, &"dd".repeat(32), "111");
        assert_eq!(
            status,
            MasterHistoryStatus::ReadFailed,
            "key 不匹配应报读取失败"
        );
        // db 存在 → 指纹 db 分量非 None（轮询可观测后续变化）。
        assert!(stat_master_fingerprint(&dir).db.is_some());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn current_user_without_projects_still_sees_global_archived_ready() {
        // 账号已登录但主库尚无正常归属记录：仍可查看主库全局归档内容。
        let dir = temp_master_dir("emptyuser");
        let key = "ee".repeat(32);
        create_master_fixture(&master_database_path(&dir), &key);
        let status = read_master_history(&dir, &key, "999");
        match status {
            MasterHistoryStatus::Ready { projects, sessions } => {
                assert_eq!(
                    projects
                        .iter()
                        .map(|project| project.project_id.as_str())
                        .collect::<Vec<_>>(),
                    vec!["p1"]
                );
                assert_eq!(
                    sessions
                        .iter()
                        .map(|session| session.session_id.as_str())
                        .collect::<Vec<_>>(),
                    vec!["s2"]
                );
            }
            other => panic!("期望 Ready 空列表，实际 {:?}", other),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
