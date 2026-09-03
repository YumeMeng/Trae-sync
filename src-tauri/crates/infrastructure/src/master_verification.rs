//! P5-7 主库两层校验（只读排查，2026-09-02 grill 立项）。
//!
//! 第一层「接力台账核对」：台账条目 vs 库内实际归属。只核对仍处于最终接力
//! 位置的会话（session_id 未再被后续换腿接走的）：存在性 / 归属一致性 /
//! 消息数不减（口径与台账写入一致：`COUNT(*) FROM chat_message`，全量）；
//! 另查旧腿残留（`from_session_id` 在库内应不存在，存在即异常）。
//!
//! 第二层「备份对比」：以链上任一 `.switch-bak-{时间戳}` 为基准的会话级
//! 差异——备份有、现在没有且台账无换腿解释 = 丢失候选；现在有、备份没有
//! = 新增正常。丢失候选只报告，附人工恢复指引（设置页既有文案）。
//!
//! 两层都只读（发现异常只报告不修复）；当前库与备份库均经隔离三件套副本
//! 只读打开（`open_with_key_readonly`，主库运行中可随时读取，不触碰原文件，
//! wal 附属件随副本走、一致性有保障）。

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::master_handover::master_database_path;
use crate::relay_ledger::RelayLedgerEntry;
use crate::sqlcipher::open_with_key_readonly;

// ===== 数据结构（DTO 层在 lib.rs 组装，这里保持领域形态）=====

/// 库内一个会话的核对事实（只读快照）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionFact {
    /// project.user_id（LEFT JOIN，无项目行时为 None——归属核对自然报异常）。
    pub user_id: Option<String>,
    /// chat_message 全量行数（与台账 message_count_at_switch 同口径）。
    pub message_count: i64,
    /// 会话标题（列缺失或为空时为 None；界面主信息用）。
    pub title: Option<String>,
}

/// 台账核对异常类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LedgerIssueKind {
    /// 台账记录的会话在库内不存在（可能被真实删除或丢失）。
    MissingSession,
    /// 会话当前归属与接力记录不一致。
    OwnerMismatch,
    /// 当前消息数少于交接时记录（丢失候选）。
    MessageLoss,
    /// 已接力的旧会话仍在库中残留。
    StaleLeg,
}

/// 一条台账核对异常。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LedgerIssue {
    pub kind: LedgerIssueKind,
    /// 相关会话（新腿或旧腿的 session_id；技术标识，界面收悬浮提示）。
    pub session_id: String,
    /// 用户可读描述（界面直接展示，不含 ID 原文）。
    pub detail: String,
}

/// 第一层结果：接力台账核对报告。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelayLedgerVerification {
    /// 核对过的当前腿数（按 session_id 去重后的最终位置条目）。
    pub checked_count: usize,
    /// 被后续换腿接走、无需核对的中间腿数（接力链正常衔接的证据）。
    pub relayed_away_count: usize,
    /// 旧腿残留条数（含在 issues 中，另有计数便于徽章）。
    pub issues: Vec<LedgerIssue>,
}

/// 第二层结果：备份对比报告。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackupComparison {
    /// 使用的基准备份时间戳（秒级 UNIX）。
    pub backup_stamp: u64,
    /// 两边都有的会话数（规模感）。
    pub common_count: usize,
    /// 备份有、现在没有且台账无换腿解释 = 丢失候选。
    pub missing: Vec<MissingSession>,
    /// 备份有、现在没有、但台账解释为换腿（正常接力）的会话数。
    pub relayed_away_count: usize,
    /// 现在有、备份没有（新增，正常）。
    pub added_count: usize,
}

/// 一个丢失候选会话（备份里还在、现在没了）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MissingSession {
    /// 备份库中的会话标题（界面主信息；技术 ID 收悬浮提示由 DTO 层带出）。
    pub title: Option<String>,
    /// 备份时该会话的消息数。
    pub message_count: i64,
}

// ===== 库内会话事实读取（当前库与备份库共用）=====

/// 只读读取指定库（主库或备份主文件）的全部会话事实。
///
/// 打不开（key 不匹配/文件损坏/缺文件）返回 None，调用方转为状态字段；
/// 列缺失（session_title 等）按探针口径降级（列防御与 master_history 一致）。
pub fn read_session_facts(db_path: &Path, raw_key: &str) -> Option<HashMap<String, SessionFact>> {
    if !db_path.is_file() {
        return None;
    }
    let conn = open_with_key_readonly(db_path, raw_key).ok()?;
    let has_title = column_exists(&conn, "chat_session", "session_title");
    let title_expr = if has_title {
        "COALESCE(s.session_title, '')"
    } else {
        "''"
    };
    let mut statement = conn
        .prepare(&format!(
            "SELECT s.session_id, p.user_id, {title_expr}, \
             (SELECT COUNT(*) FROM chat_message m WHERE m.session_id = s.session_id) \
             FROM chat_session s \
             LEFT JOIN project p ON p.project_id = s.project_id"
        ))
        .ok()?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                SessionFact {
                    user_id: row.get::<_, Option<String>>(1)?,
                    message_count: row.get::<_, i64>(3)?,
                    title: {
                        let raw: String = row.get(2)?;
                        (!raw.is_empty()).then_some(raw)
                    },
                },
            ))
        })
        .ok()?;
    let mut facts = HashMap::new();
    for row in rows {
        let (session_id, fact) = row.ok()?;
        facts.insert(session_id, fact);
    }
    Some(facts)
}

/// 列存在性探针（与 master_history 同款轻量探测）。
fn column_exists(conn: &rusqlite::Connection, table: &str, column: &str) -> bool {
    conn.prepare(&format!("SELECT {column} FROM {table} LIMIT 0"))
        .is_ok()
}

// ===== 第一层：接力台账核对 =====

/// 台账 vs 库内实际归属核对（只读，发现异常只报告）。
///
/// 核对对象 = 每个 session_id 的最新一条记录（按时间戳），且该 session_id
/// 未出现在任何条目的 from_session_id 中（未被后续换腿接走，才是最终位置）。
/// 旧腿（全部 from_session_id）在库内存在即残留异常。
pub fn verify_relay_ledger(
    facts: &HashMap<String, SessionFact>,
    entries: &[RelayLedgerEntry],
) -> RelayLedgerVerification {
    // 旧腿集合：被后续换腿接走的 session_id（换腿正常解释）。
    let relayed_away: std::collections::HashSet<&str> = entries
        .iter()
        .filter_map(|entry| entry.from_session_id.as_deref())
        .collect();

    // 按 session_id 去重取最新（同 ID 多条时后写的覆盖先写的；台账天然追加序）。
    let mut latest_by_session: HashMap<&str, &RelayLedgerEntry> = HashMap::new();
    for entry in entries {
        latest_by_session.insert(entry.session_id.as_str(), entry);
    }

    let mut issues = Vec::new();
    let mut checked_count = 0usize;
    let mut relayed_away_count = 0usize;

    for (session_id, entry) in latest_by_session {
        if relayed_away.contains(session_id) {
            // 中间腿：已被后续换腿接走，库内不存在是正常的。
            relayed_away_count += 1;
            continue;
        }
        checked_count += 1;
        let Some(fact) = facts.get(session_id) else {
            issues.push(LedgerIssue {
                kind: LedgerIssueKind::MissingSession,
                session_id: session_id.to_string(),
                detail: "接力记录中的会话在主库中不存在（可能已被删除）。".to_string(),
            });
            continue;
        };
        // 归属一致性：台账最后一条 to_user_id 应等于库内 project.user_id。
        if fact.user_id.as_deref() != Some(entry.to_user_id.as_str()) {
            issues.push(LedgerIssue {
                kind: LedgerIssueKind::OwnerMismatch,
                session_id: session_id.to_string(),
                detail: "会话当前归属与接力记录不一致（可能被其他操作改写）。".to_string(),
            });
        }
        // 消息数不减：当前库消息数少于交接时记录 = 丢失候选。
        if fact.message_count < entry.message_count_at_switch {
            issues.push(LedgerIssue {
                kind: LedgerIssueKind::MessageLoss,
                session_id: session_id.to_string(),
                detail: format!(
                    "会话消息数少于交接时记录（现在 {} 条，交接时 {} 条）。",
                    fact.message_count, entry.message_count_at_switch
                ),
            });
        }
    }

    // 旧腿残留：from_session_id 在库内应不存在（换腿是 UPDATE 旧 ID 消失）。
    // 中间腿同样不应存在——只有当条目 from_session_id 指向的 ID 还在库里时才算异常。
    for old_id in relayed_away {
        if facts.contains_key(old_id) {
            issues.push(LedgerIssue {
                kind: LedgerIssueKind::StaleLeg,
                session_id: old_id.to_string(),
                detail: "已接力的旧会话仍在主库中残留（换腿未完全生效）。".to_string(),
            });
        }
    }

    RelayLedgerVerification {
        checked_count,
        relayed_away_count,
        issues,
    }
}

// ===== 第二层：备份对比 =====

/// 备份主文件路径（`.switch-bak-{stamp}`；附属件按同后缀约定）。
pub fn backup_db_path(db_path: &Path, stamp: u64) -> PathBuf {
    let dir = db_path.parent().unwrap_or_else(|| Path::new("."));
    let name = db_path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("database.db");
    dir.join(format!("{name}.switch-bak-{stamp}"))
}

/// 以指定备份为基准对比当前库（会话级，只读）。
///
/// `current`/`backup` 为两库会话事实快照（调用方先用 read_session_facts 读出，
/// 便于失败时区分状态）；`entries` 为台账（换腿解释集）。
pub fn compare_with_backup(
    current: &HashMap<String, SessionFact>,
    backup: &HashMap<String, SessionFact>,
    backup_stamp: u64,
    entries: &[RelayLedgerEntry],
) -> BackupComparison {
    let relayed_away: std::collections::HashSet<&str> = entries
        .iter()
        .filter_map(|entry| entry.from_session_id.as_deref())
        .collect();

    let mut missing = Vec::new();
    let mut relayed_away_count = 0usize;
    let mut common_count = 0usize;
    for (session_id, fact) in backup {
        if current.contains_key(session_id) {
            common_count += 1;
        } else if relayed_away.contains(session_id.as_str()) {
            // 备份里有、现在没有，但台账解释为换腿——正常接力，不是丢失。
            relayed_away_count += 1;
        } else {
            missing.push(MissingSession {
                title: fact.title.clone(),
                message_count: fact.message_count,
            });
        }
    }
    let added_count = current
        .keys()
        .filter(|session_id| !backup.contains_key(session_id.as_str()))
        .count();

    BackupComparison {
        backup_stamp,
        common_count,
        missing,
        relayed_away_count,
        added_count,
    }
}

// ===== 便捷入口（当前库路径派生）=====

/// 当前主库 db 路径（与 master_checkup 同一派生，供命令层复用）。
pub fn current_master_db_path(master_data_dir: &Path) -> PathBuf {
    master_database_path(master_data_dir)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::{Connection, OpenFlags};

    /// 构造加密库 fixture（表结构与探针口径对齐；带 session_title 列）。
    fn create_db(db_path: &Path, raw_key: &str, sql: &str) {
        let flags = OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE;
        let conn = Connection::open_with_flags(db_path, flags).unwrap();
        let pragma = format!("PRAGMA key = \"x'{}'\";", raw_key);
        conn.execute_batch(&pragma).unwrap();
        conn.execute_batch(
            "CREATE TABLE project (project_id TEXT PRIMARY KEY, user_id TEXT);
             CREATE TABLE chat_session (session_id TEXT PRIMARY KEY, project_id TEXT, session_title TEXT);
             CREATE TABLE chat_message (message_id TEXT PRIMARY KEY, session_id TEXT);",
        )
        .unwrap();
        conn.execute_batch(sql).unwrap();
    }

    fn entry(session_id: &str, from: Option<&str>, to: &str, count: i64, at: u64) -> RelayLedgerEntry {
        RelayLedgerEntry {
            session_id: session_id.to_string(),
            from_session_id: from.map(str::to_string),
            project_id: "p1".to_string(),
            from_user_id: "old".to_string(),
            to_user_id: to.to_string(),
            to_profile_id: "prof".to_string(),
            message_count_at_switch: count,
            switched_at_unix_seconds: at,
        }
    }

    fn temp_dir(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "trae-sync-p57-verify-{}-{}-{}",
            label,
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn reads_session_facts_with_owner_and_counts() {
        let dir = temp_dir("facts");
        let key = "aa".repeat(32);
        create_db(
            &dir.join("database.db"),
            &key,
            "INSERT INTO project VALUES ('p1', '111');
             INSERT INTO chat_session VALUES ('s1', 'p1', '会话一');
             INSERT INTO chat_session VALUES ('s2', 'p1', NULL);
             INSERT INTO chat_message VALUES ('m1', 's1');
             INSERT INTO chat_message VALUES ('m2', 's1');
             INSERT INTO chat_message VALUES ('m3', 's2');",
        );
        let facts = read_session_facts(&dir.join("database.db"), &key).unwrap();
        assert_eq!(facts.len(), 2);
        let s1 = &facts["s1"];
        assert_eq!(s1.user_id.as_deref(), Some("111"));
        assert_eq!(s1.message_count, 2);
        assert_eq!(s1.title.as_deref(), Some("会话一"));
        // 空标题归一化为 None。
        assert_eq!(facts["s2"].title, None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_file_returns_none() {
        let dir = temp_dir("missing");
        assert!(read_session_facts(&dir.join("database.db"), &"bb".repeat(32)).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn ledger_verification_reports_four_issue_kinds() {
        let dir = temp_dir("ledger");
        let key = "cc".repeat(32);
        // 库内：ok 腿正常 / miss 腿不存在 / owner 腿归属被改 / loss 腿消息变少 /
        // stale 旧腿仍在库里（应报残留）。
        create_db(
            &dir.join("database.db"),
            &key,
            "INSERT INTO project VALUES ('p1', '222');
             INSERT INTO chat_session VALUES ('ok', 'p1', '正常腿');
             INSERT INTO chat_session VALUES ('owner', 'p1', '归属变了');
             INSERT INTO chat_session VALUES ('loss', 'p1', '消息少了');
             INSERT INTO chat_session VALUES ('stale', 'p1', '旧腿残留');
             INSERT INTO chat_message VALUES ('m1', 'ok');
             INSERT INTO chat_message VALUES ('m2', 'loss');",
        );
        let facts = read_session_facts(&dir.join("database.db"), &key).unwrap();
        let entries = vec![
            entry("ok", None, "222", 1, 100),
            entry("miss", None, "222", 1, 100),
            entry("owner", None, "999", 0, 100),
            entry("loss", None, "222", 5, 100),
            entry("new", Some("stale"), "222", 0, 200),
        ];
        let report = verify_relay_ledger(&facts, &entries);
        // checked：ok/miss/owner/loss（new 被算作 relayed_away？不——new 是最新腿
        // 且 new 不在 from 集合……等下，new 自己是腿；from 集合 = {stale}。
        // latest_by_session = ok/miss/owner/loss/new；new ∉ from 集合 → checked。
        assert_eq!(report.checked_count, 5);
        let kinds: Vec<LedgerIssueKind> = report.issues.iter().map(|i| i.kind).collect();
        assert!(kinds.contains(&LedgerIssueKind::MissingSession));
        assert!(kinds.contains(&LedgerIssueKind::OwnerMismatch));
        assert!(kinds.contains(&LedgerIssueKind::MessageLoss));
        assert!(kinds.contains(&LedgerIssueKind::StaleLeg));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn middle_leg_not_reported_when_relayed_away() {
        // 中间腿：旧 ID 已被接走且库里确实没有——不报缺失（正常接力）。
        let mut facts = HashMap::new();
        facts.insert(
            "leg2".to_string(),
            SessionFact {
                user_id: Some("222".to_string()),
                message_count: 3,
                title: None,
            },
        );
        let entries = vec![
            entry("leg1", None, "111", 1, 100),
            entry("leg2", Some("leg1"), "222", 2, 200),
        ];
        let report = verify_relay_ledger(&facts, &entries);
        assert_eq!(report.checked_count, 1);
        assert_eq!(report.relayed_away_count, 1);
        assert!(report.issues.is_empty());
    }

    #[test]
    fn backup_comparison_separates_relayed_and_missing() {
        let mut current = HashMap::new();
        current.insert(
            "s2".to_string(),
            SessionFact {
                user_id: Some("222".to_string()),
                message_count: 2,
                title: None,
            },
        );
        current.insert(
            "s3".to_string(),
            SessionFact {
                user_id: Some("222".to_string()),
                message_count: 1,
                title: None,
            },
        );
        let mut backup = HashMap::new();
        backup.insert(
            "s1".to_string(),
            SessionFact {
                user_id: Some("111".to_string()),
                message_count: 5,
                title: Some("被换腿".to_string()),
            },
        );
        backup.insert(
            "s2".to_string(),
            SessionFact {
                user_id: Some("111".to_string()),
                message_count: 2,
                title: None,
            },
        );
        backup.insert(
            "s4".to_string(),
            SessionFact {
                user_id: Some("111".to_string()),
                message_count: 9,
                title: Some("丢失候选".to_string()),
            },
        );
        // 台账：s1 → s2 换腿（s1 有解释）；s4 无解释 = 丢失候选。
        let entries = vec![entry("s2", Some("s1"), "222", 2, 100)];
        let report = compare_with_backup(&current, &backup, 1756000000, &entries);
        assert_eq!(report.common_count, 1); // s2
        assert_eq!(report.relayed_away_count, 1); // s1
        assert_eq!(report.added_count, 1); // s3
        assert_eq!(report.missing.len(), 1); // s4
        assert_eq!(report.missing[0].title.as_deref(), Some("丢失候选"));
        assert_eq!(report.missing[0].message_count, 9);
    }

    #[test]
    fn backup_db_path_follows_switch_bak_naming() {
        let path = backup_db_path(Path::new("C:/data/database.db"), 1756000000);
        assert!(path.ends_with("database.db.switch-bak-1756000000"));
    }
}
