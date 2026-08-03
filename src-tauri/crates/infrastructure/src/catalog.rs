//! SQLCipher 目录库实现：T04 投影、浏览、搜索、版本与 owner 观察。
//!
//! 实现 `CatalogRepository` port，将快照投影到加密目录库并提供浏览/搜索/诊断。
//!
//! 安全约束（Gate E/I/J）：
//! - raw_key 在构造时注入，不出现在任何方法签名、日志或返回值中
//! - FTS 索引位于同一 SQLCipher 内，不生成明文旁路索引
//! - 软删除项保留在 message_projection 表中，但 browse/search/统计排除（Gate J）
//! - first_observed_owner 永不更新（Gate E）
//! - 重复扫描不创建重复 session_identity/session_version 行（INSERT OR IGNORE + UNIQUE）（Gate I）
//!
//! 实现说明：
//! - 每次方法调用打开新连接，天然 Send + Sync，无共享可变状态
//! - 任何 DB 错误保守返回空 Vec/None/false，不 panic
//! - FTS5 使用独立虚拟表（非 content=message_projection 外部内容表），
//!   手动 DELETE+INSERT 同步索引，避免 rowid 关联复杂性，功能等价

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rusqlite::Connection;
use traesync_domain::{
    BrowseAccountNode, BrowseProjectNode, BrowseResult, BrowseSessionNode, ContentGraphHash,
    ConversationPreview, DiagnosticIntegrityAssertion, HistoryBrowseSummary, MessageProjection,
    OwnerObservation, ProjectIdentity, ProjectObservation, ProjectSourceAssignment,
    ScanFailureReason, SearchHit, SessionIdentity, SessionProjection, SessionVersion, SnapshotId,
    SourceSnapshotMeta, VersionClassification,
};
use traesync_ports::{CatalogRepository, ContentGraphHasher, SourceNormalizer};

use crate::content_graph::DeterministicContentGraphHasher;

/// SQLCipher 目录库实现。
///
/// 持有目录库路径与 raw key（私有），实现 `CatalogRepository` port。
pub struct SqlCipherCatalogRepository {
    db_path: PathBuf,
    /// SQLCipher raw key（64 字符 hex），私有，不通过方法暴露。
    raw_key: String,
}

impl SqlCipherCatalogRepository {
    pub fn new(db_path: PathBuf, raw_key: String) -> Self {
        Self { db_path, raw_key }
    }

    /// 打开目录库连接并设置 raw key。
    fn open_catalog(&self) -> Option<Connection> {
        let conn = Connection::open(&self.db_path).ok()?;
        // raw key 语法：x'<hex>' —— 不进入日志
        let pragma = format!("PRAGMA key = \"x'{}'\";", self.raw_key);
        conn.execute_batch(&pragma).ok()?;
        Some(conn)
    }
}

// ============================================================================
// 辅助函数
// ============================================================================

/// SystemTime -> i64 秒（自 UNIX_EPOCH）
fn system_time_to_secs(t: SystemTime) -> i64 {
    t.duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// i64 秒 -> SystemTime
fn secs_to_system_time(s: i64) -> SystemTime {
    if s >= 0 {
        UNIX_EPOCH + Duration::from_secs(s as u64)
    } else {
        UNIX_EPOCH
    }
}

/// VersionClassification -> 字符串（与 serde snake_case 一致）
fn classification_to_str(c: VersionClassification) -> &'static str {
    match c {
        VersionClassification::Identical => "identical",
        VersionClassification::FastForward => "fast_forward",
        VersionClassification::Forked => "forked",
        VersionClassification::Unclassified => "unclassified",
    }
}

/// 字符串 -> VersionClassification
fn str_to_classification(s: &str) -> VersionClassification {
    match s {
        "identical" => VersionClassification::Identical,
        "fast_forward" => VersionClassification::FastForward,
        "forked" => VersionClassification::Forked,
        _ => VersionClassification::Unclassified,
    }
}

/// 读取会话消息投影（可配置是否排除软删除）。
/// R4：读取 turn_id 列（可为 NULL）。
fn read_messages_for_session(
    conn: &Connection,
    namespace: &str,
    session_id: &str,
    exclude_soft_deleted: bool,
) -> Vec<MessageProjection> {
    let sql = if exclude_soft_deleted {
        "SELECT message_id, session_id, role, content_excerpt, soft_deleted, seq, turn_id \
         FROM message_projection WHERE namespace = ?1 AND session_id = ?2 AND soft_deleted = 0 \
         ORDER BY seq ASC"
    } else {
        "SELECT message_id, session_id, role, content_excerpt, soft_deleted, seq, turn_id \
         FROM message_projection WHERE namespace = ?1 AND session_id = ?2 \
         ORDER BY seq ASC"
    };
    let mut stmt = match conn.prepare(sql) {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };
    let rows = match stmt.query_map(rusqlite::params![namespace, session_id], |row| {
        Ok(MessageProjection {
            message_id: row.get(0)?,
            session_id: row.get(1)?,
            role: row.get(2)?,
            content_excerpt: row.get(3)?,
            soft_deleted: row.get::<_, i64>(4)? != 0,
            seq: row.get::<_, i64>(5)? as u64,
            turn_id: row.get(6).ok(),
        })
    }) {
        Ok(r) => r,
        Err(_) => return Vec::new(),
    };
    rows.filter_map(|r| r.ok()).collect()
}

impl CatalogRepository for SqlCipherCatalogRepository {
    fn ensure_initialized(&self) -> bool {
        let conn = match self.open_catalog() {
            Some(c) => c,
            None => return false,
        };
        // 检查是否已初始化（catalog_meta 有 schema_version）
        let already: bool = conn
            .query_row(
                "SELECT 1 FROM catalog_meta WHERE key = 'schema_version' LIMIT 1",
                [],
                |_| Ok(true),
            )
            .unwrap_or(false);
        if already {
            return false;
        }
        // 创建全部表（IF NOT EXISTS 保证部分初始化可补全）
        let batch = r#"
        CREATE TABLE IF NOT EXISTS catalog_meta (key TEXT PRIMARY KEY, value TEXT);
        CREATE TABLE IF NOT EXISTS seen_account (
            user_id TEXT PRIMARY KEY, first_seen_at INTEGER, last_seen_at INTEGER
        );
        CREATE TABLE IF NOT EXISTS source_snapshot (
            snapshot_id TEXT PRIMARY KEY, fingerprint TEXT, captured_at INTEGER, data_location_id TEXT
        );
        CREATE TABLE IF NOT EXISTS project_identity (
            project_id TEXT PRIMARY KEY, biz_project_id TEXT, display_name TEXT,
            soft_deleted INTEGER DEFAULT 0
        );
        CREATE TABLE IF NOT EXISTS project_observation (
            project_id TEXT PRIMARY KEY,
            first_observed_owner TEXT,
            first_observed_at INTEGER,
            current_live_owner TEXT
        );
        CREATE TABLE IF NOT EXISTS owner_observation (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            project_id TEXT, owner_user_id TEXT, observed_at INTEGER, source_snapshot_id TEXT
        );
        CREATE TABLE IF NOT EXISTS project_source_assignment (
            project_id TEXT PRIMARY KEY, user_assigned_owner TEXT, assigned_at INTEGER
        );
        CREATE TABLE IF NOT EXISTS session_identity (
            product_history_namespace TEXT, original_session_id TEXT,
            PRIMARY KEY (product_history_namespace, original_session_id)
        );
        CREATE TABLE IF NOT EXISTS session_version (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            namespace TEXT, original_session_id TEXT, source_snapshot_id TEXT,
            content_graph_hash TEXT, classification TEXT, captured_at INTEGER, title TEXT,
            UNIQUE (namespace, original_session_id, content_graph_hash)
        );
        CREATE TABLE IF NOT EXISTS session_projection (
            namespace TEXT, original_session_id TEXT,
            active_content_graph_hash TEXT, active_title TEXT,
            soft_deleted INTEGER, project_id TEXT,
            PRIMARY KEY (namespace, original_session_id)
        );
        CREATE TABLE IF NOT EXISTS message_projection (
            message_id TEXT, session_id TEXT, role TEXT,
            content_excerpt TEXT, soft_deleted INTEGER, seq INTEGER, namespace TEXT,
            turn_id TEXT
        );
        CREATE TABLE IF NOT EXISTS soft_deletion_marker (
            entity_kind TEXT, entity_id TEXT, deleted_at INTEGER
        );
        CREATE VIRTUAL TABLE IF NOT EXISTS message_fts USING fts5(
            message_id, session_id, content_excerpt, namespace
        );
        "#;
        if conn.execute_batch(batch).is_err() {
            return false;
        }
        // 写入 schema_version
        conn.execute(
            "INSERT OR REPLACE INTO catalog_meta (key, value) VALUES ('schema_version', '1')",
            [],
        )
        .is_ok()
    }

    fn project_snapshot(
        &self,
        snapshot_meta: &SourceSnapshotMeta,
        snapshot_dir: &Path,
        normalizer: &dyn SourceNormalizer,
    ) -> Result<(), ScanFailureReason> {
        // 1. 事务前用 normalizer 读取全部快照数据（打开快照 DB，不涉及 catalog 事务）
        let projects = normalizer.read_projects(snapshot_dir);
        let sessions = normalizer.read_session_projections(snapshot_dir);
        let messages = normalizer.read_messages(snapshot_dir);
        // 读取每个项目的 owner（活动库 project.user_id）
        let owners: HashMap<String, String> = projects
            .iter()
            .filter_map(|p| {
                normalizer
                    .read_project_owner(snapshot_dir, &p.project_id)
                    .map(|o| (p.project_id.clone(), o))
            })
            .collect();

        // 2. 打开 catalog 连接并开事务
        let conn = self
            .open_catalog()
            .ok_or(ScanFailureReason::CatalogTransactionFailed)?;
        if conn.execute_batch("BEGIN;").is_err() {
            return Err(ScanFailureReason::CatalogTransactionFailed);
        }

        let result = project_snapshot_tx(
            &conn,
            snapshot_meta,
            &projects,
            &sessions,
            &messages,
            &owners,
        );

        match result {
            Ok(()) => {
                if conn.execute_batch("COMMIT;").is_err() {
                    let _ = conn.execute_batch("ROLLBACK;");
                    return Err(ScanFailureReason::CatalogTransactionFailed);
                }
                Ok(())
            }
            Err(e) => {
                let _ = conn.execute_batch("ROLLBACK;");
                Err(e)
            }
        }
    }

    fn browse(&self) -> BrowseResult {
        let conn = match self.open_catalog() {
            Some(c) => c,
            None => {
                return BrowseResult {
                    accounts: vec![],
                    projects: vec![],
                    sessions: vec![],
                    summary: HistoryBrowseSummary::default(),
                }
            }
        };
        // 全部项目（含 display_owner）
        let projects = read_all_browse_projects(&conn, None);
        // 全部会话（排除软删除）
        let sessions = read_all_browse_sessions(&conn, None);
        // 账号树
        let accounts = build_account_nodes(&conn);
        // 摘要
        let summary = compute_history_summary(&conn);

        BrowseResult {
            accounts,
            projects,
            sessions,
            summary,
        }
    }

    fn browse_projects_by_account(&self, user_id: &str) -> Vec<BrowseProjectNode> {
        let conn = match self.open_catalog() {
            Some(c) => c,
            None => return vec![],
        };
        read_all_browse_projects(&conn, Some(user_id))
    }

    fn browse_sessions_by_project(&self, project_id: &str) -> Vec<BrowseSessionNode> {
        let conn = match self.open_catalog() {
            Some(c) => c,
            None => return vec![],
        };
        read_all_browse_sessions(&conn, Some(project_id))
    }

    fn read_conversation_preview(&self, session: &SessionIdentity) -> Option<ConversationPreview> {
        let conn = self.open_catalog()?;
        // 读取会话标题
        let title: String = conn
            .query_row(
                "SELECT active_title FROM session_projection \
                 WHERE namespace = ?1 AND original_session_id = ?2",
                rusqlite::params![
                    session.product_history_namespace,
                    session.original_session_id
                ],
                |row| row.get(0),
            )
            .unwrap_or_default();
        // 读取消息（排除软删除，保留底层行用于诊断）
        let messages = read_messages_for_session(
            &conn,
            &session.product_history_namespace,
            &session.original_session_id,
            true,
        );
        let total = messages.len() as u64;
        Some(ConversationPreview {
            session_identity: session.clone(),
            title,
            messages,
            total_message_count: total,
        })
    }

    fn search_messages(&self, query: &str) -> Vec<SearchHit> {
        let conn = match self.open_catalog() {
            Some(c) => c,
            None => return vec![],
        };
        // FTS5 MATCH 查询，JOIN message_projection 获取 soft_deleted/role，
        // JOIN session_projection 获取 project_id/title。排除软删除。
        let mut stmt = match conn.prepare(
            "SELECT mp.message_id, mp.session_id, mp.role, mp.content_excerpt, mp.namespace, \
                    sp.project_id, sp.active_title \
             FROM message_fts \
             JOIN message_projection mp \
               ON message_fts.message_id = mp.message_id \
              AND message_fts.session_id = mp.session_id \
              AND message_fts.namespace = mp.namespace \
             JOIN session_projection sp \
               ON sp.namespace = mp.namespace AND sp.original_session_id = mp.session_id \
             WHERE message_fts MATCH ?1 AND mp.soft_deleted = 0",
        ) {
            Ok(s) => s,
            Err(_) => return vec![],
        };
        let rows = match stmt.query_map(rusqlite::params![query], |row| {
            let message_id: String = row.get(0)?;
            let session_id: String = row.get(1)?;
            let role: String = row.get(2)?;
            let content_excerpt: String = row.get(3)?;
            let namespace: String = row.get(4)?;
            let project_id: String = row.get(5)?;
            let title: String = row.get(6).unwrap_or_default();
            Ok(SearchHit {
                session_identity: SessionIdentity::new(&namespace, &session_id),
                message_id,
                project_id,
                title,
                content_excerpt,
                role,
            })
        }) {
            Ok(r) => r,
            // FTS 查询语法错误或 FTS5 不可用时保守返回空
            Err(_) => return vec![],
        };
        rows.filter_map(|r| r.ok()).collect()
    }

    fn read_project_observation(&self, project_id: &str) -> Option<ProjectObservation> {
        let conn = self.open_catalog()?;
        // project_identity（R6：含 soft_deleted）
        let identity: ProjectIdentity = conn
            .query_row(
                "SELECT project_id, biz_project_id, display_name, soft_deleted \
                 FROM project_identity WHERE project_id = ?1",
                rusqlite::params![project_id],
                |row| {
                    Ok(ProjectIdentity {
                        project_id: row.get(0)?,
                        biz_project_id: row.get(1)?,
                        display_name: row.get(2)?,
                        soft_deleted: row.get::<_, i64>(3)? != 0,
                    })
                },
            )
            .ok()?;
        // project_observation
        let (first_owner, first_at, current_owner): (String, i64, String) = conn
            .query_row(
                "SELECT first_observed_owner, first_observed_at, current_live_owner \
                 FROM project_observation WHERE project_id = ?1",
                rusqlite::params![project_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .ok()?;
        // owner_observations
        let mut stmt = match conn.prepare(
            "SELECT owner_user_id, observed_at, source_snapshot_id \
             FROM owner_observation WHERE project_id = ?1 ORDER BY observed_at ASC",
        ) {
            Ok(s) => s,
            Err(_) => return None,
        };
        let obs_rows = match stmt.query_map(rusqlite::params![project_id], |row| {
            Ok(OwnerObservation {
                owner_user_id: row.get(0)?,
                observed_at: secs_to_system_time(row.get(1)?),
                source_snapshot_id: SnapshotId::from_db_str(&row.get::<_, String>(2)?),
            })
        }) {
            Ok(r) => r,
            Err(_) => return None,
        };
        let observations: Vec<OwnerObservation> = obs_rows.filter_map(|r| r.ok()).collect();

        Some(ProjectObservation {
            project_identity: identity,
            first_observed_owner: first_owner,
            first_observed_at: secs_to_system_time(first_at),
            current_live_owner: current_owner,
            owner_observations: observations,
        })
    }

    fn read_all_project_observations(&self) -> Vec<ProjectObservation> {
        let conn = match self.open_catalog() {
            Some(c) => c,
            None => return vec![],
        };
        let project_ids: Vec<String> =
            match conn.prepare("SELECT project_id FROM project_identity ORDER BY project_id ASC") {
                Ok(mut stmt) => stmt
                    .query_map([], |row| row.get::<_, String>(0))
                    .ok()
                    .map(|r| r.filter_map(|x| x.ok()).collect())
                    .unwrap_or_default(),
                Err(_) => return vec![],
            };
        project_ids
            .iter()
            .filter_map(|pid| self.read_project_observation(pid))
            .collect()
    }

    fn read_all_session_versions(&self) -> Vec<SessionVersion> {
        let conn = match self.open_catalog() {
            Some(c) => c,
            None => return vec![],
        };
        let mut stmt = match conn.prepare(
            "SELECT namespace, original_session_id, source_snapshot_id, \
                    content_graph_hash, classification, captured_at, title \
             FROM session_version ORDER BY captured_at ASC",
        ) {
            Ok(s) => s,
            Err(_) => return vec![],
        };
        let rows = match stmt.query_map([], |row| {
            let namespace: String = row.get(0)?;
            let original_session_id: String = row.get(1)?;
            let snapshot_id: String = row.get(2)?;
            let hash: String = row.get(3)?;
            let class: String = row.get(4)?;
            let captured_at: i64 = row.get(5)?;
            let title: String = row.get(6).unwrap_or_default();
            Ok(SessionVersion {
                session_identity: SessionIdentity::new(&namespace, &original_session_id),
                source_snapshot_id: SnapshotId::from_db_str(&snapshot_id),
                content_graph_hash: ContentGraphHash(hash),
                classification: str_to_classification(&class),
                captured_at: secs_to_system_time(captured_at),
                title,
            })
        }) {
            Ok(r) => r,
            Err(_) => return vec![],
        };
        rows.filter_map(|r| r.ok()).collect()
    }

    fn read_session_projection(&self, session: &SessionIdentity) -> Option<SessionProjection> {
        let conn = self.open_catalog()?;
        conn.query_row(
            "SELECT active_content_graph_hash, active_title, soft_deleted, project_id \
             FROM session_projection WHERE namespace = ?1 AND original_session_id = ?2",
            rusqlite::params![
                session.product_history_namespace,
                session.original_session_id
            ],
            |row| {
                Ok(SessionProjection {
                    session_identity: session.clone(),
                    active_content_graph_hash: ContentGraphHash(row.get(0)?),
                    active_title: row.get(1)?,
                    soft_deleted: row.get::<_, i64>(2)? != 0,
                    project_id: row.get(3)?,
                })
            },
        )
        .ok()
    }

    fn assign_project_source(&self, assignment: &ProjectSourceAssignment) -> bool {
        let conn = match self.open_catalog() {
            Some(c) => c,
            None => return false,
        };
        // Gate E：仅写 project_source_assignment，不修改 project_observation 或快照
        conn.execute(
            "INSERT OR REPLACE INTO project_source_assignment \
             (project_id, user_assigned_owner, assigned_at) VALUES (?1, ?2, ?3)",
            rusqlite::params![
                assignment.project_id,
                assignment.user_assigned_owner,
                system_time_to_secs(assignment.assigned_at)
            ],
        )
        .is_ok()
    }

    fn read_project_source_assignment(&self, project_id: &str) -> Option<ProjectSourceAssignment> {
        let conn = self.open_catalog()?;
        conn.query_row(
            "SELECT user_assigned_owner, assigned_at FROM project_source_assignment WHERE project_id = ?1",
            rusqlite::params![project_id],
            |row| {
                let owner: Option<String> = row.get(0).ok();
                let at: i64 = row.get(1).unwrap_or(0);
                Ok(ProjectSourceAssignment {
                    project_id: project_id.to_string(),
                    user_assigned_owner: owner,
                    assigned_at: secs_to_system_time(at),
                })
            },
        )
        .ok()
    }

    fn diagnostic_integrity(&self) -> DiagnosticIntegrityAssertion {
        let conn = match self.open_catalog() {
            Some(c) => c,
            None => {
                return DiagnosticIntegrityAssertion {
                    visible_projects: 0,
                    retained_projects: 0,
                    visible_sessions: 0,
                    retained_sessions: 0,
                    visible_messages: 0,
                    retained_messages: 0,
                }
            }
        };
        // R6：项目 visible 排除 soft_deleted，retained 含全部（含软删除证据）
        let visible_projects: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM project_identity WHERE soft_deleted = 0",
                [],
                |row| row.get(0),
            )
            .unwrap_or(0);
        let retained_projects: i64 = conn
            .query_row("SELECT COUNT(*) FROM project_identity", [], |row| {
                row.get(0)
            })
            .unwrap_or(0);
        // 会话：visible 排除 soft_deleted，retained 全部
        let visible_sessions: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM session_projection WHERE soft_deleted = 0",
                [],
                |row| row.get(0),
            )
            .unwrap_or(0);
        let retained_sessions: i64 = conn
            .query_row("SELECT COUNT(*) FROM session_projection", [], |row| {
                row.get(0)
            })
            .unwrap_or(0);
        // 消息：visible 排除 soft_deleted，retained 全部（Gate J：retained 含软删除）
        let visible_messages: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM message_projection WHERE soft_deleted = 0",
                [],
                |row| row.get(0),
            )
            .unwrap_or(0);
        let retained_messages: i64 = conn
            .query_row("SELECT COUNT(*) FROM message_projection", [], |row| {
                row.get(0)
            })
            .unwrap_or(0);

        DiagnosticIntegrityAssertion {
            visible_projects: visible_projects as u64,
            retained_projects: retained_projects as u64,
            visible_sessions: visible_sessions as u64,
            retained_sessions: retained_sessions as u64,
            visible_messages: visible_messages as u64,
            retained_messages: retained_messages as u64,
        }
    }

    fn history_summary(&self) -> HistoryBrowseSummary {
        let conn = match self.open_catalog() {
            Some(c) => c,
            None => return HistoryBrowseSummary::default(),
        };
        let account_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM seen_account", [], |row| row.get(0))
            .unwrap_or(0);
        // R6：visible 排除 soft_deleted，soft_deleted 单独计数
        let visible_projects: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM project_identity WHERE soft_deleted = 0",
                [],
                |row| row.get(0),
            )
            .unwrap_or(0);
        let soft_deleted_projects: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM project_identity WHERE soft_deleted = 1",
                [],
                |row| row.get(0),
            )
            .unwrap_or(0);
        let visible_sessions: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM session_projection WHERE soft_deleted = 0",
                [],
                |row| row.get(0),
            )
            .unwrap_or(0);
        let soft_deleted_sessions: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM session_projection WHERE soft_deleted = 1",
                [],
                |row| row.get(0),
            )
            .unwrap_or(0);
        let soft_deleted_messages: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM message_projection WHERE soft_deleted = 1",
                [],
                |row| row.get(0),
            )
            .unwrap_or(0);

        HistoryBrowseSummary {
            visible_account_count: account_count as u64,
            visible_project_count: visible_projects as u64,
            visible_session_count: visible_sessions as u64,
            soft_deleted_project_count: soft_deleted_projects as u64,
            soft_deleted_session_count: soft_deleted_sessions as u64,
            soft_deleted_message_count: soft_deleted_messages as u64,
        }
    }
}

// ============================================================================
// 事务内投影实现
// ============================================================================

/// project_snapshot 事务内实现：写入全部投影。任一步骤失败返回 Err 触发回滚。
fn project_snapshot_tx(
    conn: &Connection,
    snapshot_meta: &SourceSnapshotMeta,
    projects: &[ProjectIdentity],
    sessions: &[SessionProjection],
    messages: &[MessageProjection],
    owners: &HashMap<String, String>,
) -> Result<(), ScanFailureReason> {
    let now_secs = system_time_to_secs(snapshot_meta.captured_at);
    let snapshot_id = snapshot_meta.snapshot_id.as_str();
    let fingerprint = snapshot_meta.fingerprint.as_str();
    let data_location_id = &snapshot_meta.data_location_id;
    let fail = |_| ScanFailureReason::CatalogTransactionFailed;

    // 1. 写入 source_snapshot
    conn.execute(
        "INSERT OR REPLACE INTO source_snapshot (snapshot_id, fingerprint, captured_at, data_location_id) \
         VALUES (?1, ?2, ?3, ?4)",
        rusqlite::params![snapshot_id, fingerprint, now_secs, data_location_id],
    )
    .map_err(fail)?;

    // 2. 写入项目相关
    for p in projects {
        // project_identity（R6：含 soft_deleted 标记）
        conn.execute(
            "INSERT OR REPLACE INTO project_identity \
             (project_id, biz_project_id, display_name, soft_deleted) \
             VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![
                p.project_id,
                p.biz_project_id,
                p.display_name,
                p.soft_deleted as i64
            ],
        )
        .map_err(fail)?;

        if let Some(owner) = owners.get(&p.project_id) {
            // seen_account（首次 INSERT OR IGNORE，更新 last_seen_at）
            conn.execute(
                "INSERT OR IGNORE INTO seen_account (user_id, first_seen_at, last_seen_at) \
                 VALUES (?1, ?2, ?2)",
                rusqlite::params![owner, now_secs],
            )
            .map_err(fail)?;
            conn.execute(
                "UPDATE seen_account SET last_seen_at = ?2 WHERE user_id = ?1",
                rusqlite::params![owner, now_secs],
            )
            .map_err(fail)?;

            // project_observation：首次写入 first_observed_owner，后续只更新 current_live_owner
            let exists: bool = conn
                .query_row(
                    "SELECT 1 FROM project_observation WHERE project_id = ?1",
                    rusqlite::params![p.project_id],
                    |_| Ok(true),
                )
                .unwrap_or(false);
            if !exists {
                // Gate E：首次写入 first_observed_owner，后续永不更新
                conn.execute(
                    "INSERT INTO project_observation \
                     (project_id, first_observed_owner, first_observed_at, current_live_owner) \
                     VALUES (?1, ?2, ?3, ?2)",
                    rusqlite::params![p.project_id, owner, now_secs],
                )
                .map_err(fail)?;
            } else {
                // Gate E：只更新 current_live_owner，first_observed_owner 不变
                conn.execute(
                    "UPDATE project_observation SET current_live_owner = ?2 WHERE project_id = ?1",
                    rusqlite::params![p.project_id, owner],
                )
                .map_err(fail)?;
            }

            // owner_observation 追加（每次扫描都记录）
            conn.execute(
                "INSERT INTO owner_observation \
                 (project_id, owner_user_id, observed_at, source_snapshot_id) \
                 VALUES (?1, ?2, ?3, ?4)",
                rusqlite::params![p.project_id, owner, now_secs, snapshot_id],
            )
            .map_err(fail)?;
        }

        // R6：软删除项目记录删除证据（soft_deletion_marker）
        if p.soft_deleted {
            conn.execute(
                "INSERT OR REPLACE INTO soft_deletion_marker (entity_kind, entity_id, deleted_at) \
                 VALUES (?1, ?2, ?3)",
                rusqlite::params!["project", p.project_id, now_secs],
            )
            .map_err(fail)?;
        }
    }

    // 3. 写入会话相关（含版本分类）
    let hasher = DeterministicContentGraphHasher::new();
    for s in sessions {
        let namespace = &s.session_identity.product_history_namespace;
        let original_session_id = &s.session_identity.original_session_id;
        let project_id = &s.project_id;

        // session_identity（INSERT OR IGNORE 防重复——Gate I）
        conn.execute(
            "INSERT OR IGNORE INTO session_identity (product_history_namespace, original_session_id) \
             VALUES (?1, ?2)",
            rusqlite::params![namespace, original_session_id],
        )
        .map_err(fail)?;

        // 读取旧消息（用于版本分类）——在 DELETE 前
        let old_messages = read_messages_for_session(conn, namespace, original_session_id, false);

        // 计算新内容图哈希
        let new_messages: Vec<MessageProjection> = messages
            .iter()
            .filter(|m| m.session_id == *original_session_id)
            .cloned()
            .collect();
        let new_hash = hasher.hash_session_content(&new_messages, &s.session_identity);

        // 版本分类：首次扫描无旧消息 -> Unclassified
        let classification = if old_messages.is_empty() {
            VersionClassification::Unclassified
        } else {
            hasher.classify(&old_messages, &new_messages)
        };

        // 检查 session_projection 是否已存在（决定是否首次扫描）
        let proj_exists: bool = conn
            .query_row(
                "SELECT 1 FROM session_projection WHERE namespace = ?1 AND original_session_id = ?2",
                rusqlite::params![namespace, original_session_id],
                |_| Ok(true),
            )
            .unwrap_or(false);

        // R5：只有首次扫描、Identical、FastForward 推进活跃浏览投影。
        // Forked/Unclassified 保留旧 message_projection + FTS 不变，
        // 新版本数据通过下方 session_version INSERT 保留，供后续选择。
        // 这样 search 和 preview 解析的是活跃版本内容，而非最新导入行。
        let advance_projection = !proj_exists
            || matches!(
                classification,
                VersionClassification::Identical | VersionClassification::FastForward
            );

        if advance_projection {
            // 删除旧 message_projection + FTS（按 namespace + session_id）
            conn.execute(
                "DELETE FROM message_projection WHERE namespace = ?1 AND session_id = ?2",
                rusqlite::params![namespace, original_session_id],
            )
            .map_err(fail)?;
            conn.execute(
                "DELETE FROM message_fts WHERE namespace = ?1 AND session_id = ?2",
                rusqlite::params![namespace, original_session_id],
            )
            .map_err(fail)?;

            // 写入新 message_projection + FTS
            for m in &new_messages {
                conn.execute(
                    "INSERT INTO message_projection \
                     (message_id, session_id, role, content_excerpt, soft_deleted, seq, namespace, turn_id) \
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                    rusqlite::params![
                        m.message_id,
                        m.session_id,
                        m.role,
                        m.content_excerpt,
                        m.soft_deleted as i64,
                        m.seq as i64,
                        namespace,
                        m.turn_id
                    ],
                )
                .map_err(fail)?;
                conn.execute(
                    "INSERT INTO message_fts (message_id, session_id, content_excerpt, namespace) \
                     VALUES (?1, ?2, ?3, ?4)",
                    rusqlite::params![m.message_id, m.session_id, m.content_excerpt, namespace],
                )
                .map_err(fail)?;
            }
        }

        // session_version（INSERT OR IGNORE 防重复——Gate I：UNIQUE 约束）
        // R5：始终保留版本元数据，即使不推进活跃投影——供后续选择/重建
        conn.execute(
            "INSERT OR IGNORE INTO session_version \
             (namespace, original_session_id, source_snapshot_id, content_graph_hash, \
              classification, captured_at, title) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            rusqlite::params![
                namespace,
                original_session_id,
                snapshot_id,
                new_hash.as_str(),
                classification_to_str(classification),
                now_secs,
                s.active_title
            ],
        )
        .map_err(fail)?;

        // session_projection
        if !proj_exists {
            conn.execute(
                "INSERT INTO session_projection \
                 (namespace, original_session_id, active_content_graph_hash, active_title, \
                  soft_deleted, project_id) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                rusqlite::params![
                    namespace,
                    original_session_id,
                    new_hash.as_str(),
                    s.active_title,
                    s.soft_deleted as i64,
                    project_id
                ],
            )
            .map_err(fail)?;
        } else {
            // 更新 soft_deleted 和 project_id
            conn.execute(
                "UPDATE session_projection SET soft_deleted = ?3, project_id = ?4 \
                 WHERE namespace = ?1 AND original_session_id = ?2",
                rusqlite::params![
                    namespace,
                    original_session_id,
                    s.soft_deleted as i64,
                    project_id
                ],
            )
            .map_err(fail)?;
            // Gate I：Identical/FastForward 更新 active_content_graph_hash + active_title；
            // Forked/Unclassified 保留旧投影不变
            match classification {
                VersionClassification::Identical | VersionClassification::FastForward => {
                    conn.execute(
                        "UPDATE session_projection SET active_content_graph_hash = ?3, active_title = ?4 \
                         WHERE namespace = ?1 AND original_session_id = ?2",
                        rusqlite::params![
                            namespace,
                            original_session_id,
                            new_hash.as_str(),
                            s.active_title
                        ],
                    )
                    .map_err(fail)?;
                }
                VersionClassification::Forked | VersionClassification::Unclassified => {
                    // 保留旧投影不变
                }
            }
        }

        // soft_deletion_marker（软删除会话）
        if s.soft_deleted {
            conn.execute(
                "INSERT OR REPLACE INTO soft_deletion_marker (entity_kind, entity_id, deleted_at) \
                 VALUES (?1, ?2, ?3)",
                rusqlite::params!["session", original_session_id, now_secs],
            )
            .map_err(fail)?;
        }
    }

    Ok(())
}

// ============================================================================
// 浏览辅助函数
// ============================================================================

/// 映射项目行到 BrowseProjectNode。
fn map_project_row(row: &rusqlite::Row) -> rusqlite::Result<BrowseProjectNode> {
    Ok(BrowseProjectNode {
        project_id: row.get(0)?,
        display_name: row.get(1)?,
        display_owner: row.get(2)?,
        session_count: row.get::<_, i64>(3)? as u64,
    })
}

/// 映射会话行到 BrowseSessionNode。
fn map_session_row(row: &rusqlite::Row) -> rusqlite::Result<BrowseSessionNode> {
    Ok(BrowseSessionNode {
        session_identity: SessionIdentity::new(
            &row.get::<_, String>(0)?,
            &row.get::<_, String>(1)?,
        ),
        title: row.get(2)?,
        message_count: row.get::<_, i64>(3)? as u64,
        last_captured_at: secs_to_system_time(row.get::<_, i64>(4).unwrap_or(0)),
        project_id: row.get::<_, String>(5)?,
    })
}

/// 读取项目节点。filter_account 为 Some 时按 display_owner 过滤。
/// R6：排除 soft_deleted = 1 的项目（ browse 不显示软删除项目）。
fn read_all_browse_projects(
    conn: &Connection,
    filter_account: Option<&str>,
) -> Vec<BrowseProjectNode> {
    let sql = match filter_account {
        Some(_) => {
            "SELECT pi.project_id, pi.display_name, \
                    COALESCE(psa.user_assigned_owner, po.first_observed_owner) AS display_owner, \
                    (SELECT COUNT(*) FROM session_projection sp \
                     WHERE sp.project_id = pi.project_id AND sp.soft_deleted = 0) AS session_count \
             FROM project_identity pi \
             LEFT JOIN project_observation po ON po.project_id = pi.project_id \
             LEFT JOIN project_source_assignment psa ON psa.project_id = pi.project_id \
             WHERE pi.soft_deleted = 0 \
               AND COALESCE(psa.user_assigned_owner, po.first_observed_owner) = ?1 \
             ORDER BY pi.display_name ASC"
        }
        None => {
            "SELECT pi.project_id, pi.display_name, \
                    COALESCE(psa.user_assigned_owner, po.first_observed_owner) AS display_owner, \
                    (SELECT COUNT(*) FROM session_projection sp \
                     WHERE sp.project_id = pi.project_id AND sp.soft_deleted = 0) AS session_count \
             FROM project_identity pi \
             LEFT JOIN project_observation po ON po.project_id = pi.project_id \
             LEFT JOIN project_source_assignment psa ON psa.project_id = pi.project_id \
             WHERE pi.soft_deleted = 0 \
             ORDER BY pi.display_name ASC"
        }
    };
    let mut stmt = match conn.prepare(sql) {
        Ok(s) => s,
        Err(_) => return vec![],
    };
    let rows = match filter_account {
        Some(account) => stmt
            .query_map(rusqlite::params![account], map_project_row)
            .ok(),
        None => stmt.query_map([], map_project_row).ok(),
    };
    match rows {
        Some(r) => r.filter_map(|x| x.ok()).collect(),
        None => vec![],
    }
}

/// 读取会话节点。filter_project 为 Some 时按 project_id 过滤，排除软删除。
fn read_all_browse_sessions(
    conn: &Connection,
    filter_project: Option<&str>,
) -> Vec<BrowseSessionNode> {
    let sql = match filter_project {
        Some(_) => {
            "SELECT sp.namespace, sp.original_session_id, sp.active_title, \
                    (SELECT COUNT(*) FROM message_projection mp \
                     WHERE mp.namespace = sp.namespace AND mp.session_id = sp.original_session_id \
                     AND mp.soft_deleted = 0) AS msg_count, \
                    (SELECT MAX(sv.captured_at) FROM session_version sv \
                     WHERE sv.namespace = sp.namespace AND sv.original_session_id = sp.original_session_id) AS last_captured, \
                    sp.project_id \
             FROM session_projection sp \
             WHERE sp.soft_deleted = 0 AND sp.project_id = ?1 \
             ORDER BY sp.active_title ASC"
        }
        None => {
            "SELECT sp.namespace, sp.original_session_id, sp.active_title, \
                    (SELECT COUNT(*) FROM message_projection mp \
                     WHERE mp.namespace = sp.namespace AND mp.session_id = sp.original_session_id \
                     AND mp.soft_deleted = 0) AS msg_count, \
                    (SELECT MAX(sv.captured_at) FROM session_version sv \
                     WHERE sv.namespace = sp.namespace AND sv.original_session_id = sp.original_session_id) AS last_captured, \
                    sp.project_id \
             FROM session_projection sp \
             WHERE sp.soft_deleted = 0 \
             ORDER BY sp.active_title ASC"
        }
    };
    let mut stmt = match conn.prepare(sql) {
        Ok(s) => s,
        Err(_) => return vec![],
    };
    let rows = match filter_project {
        Some(pid) => stmt.query_map(rusqlite::params![pid], map_session_row).ok(),
        None => stmt.query_map([], map_session_row).ok(),
    };
    match rows {
        Some(r) => r.filter_map(|x| x.ok()).collect(),
        None => vec![],
    }
}

/// 构建账号树节点。
fn build_account_nodes(conn: &Connection) -> Vec<BrowseAccountNode> {
    let mut stmt = match conn.prepare("SELECT user_id FROM seen_account ORDER BY user_id ASC") {
        Ok(s) => s,
        Err(_) => return vec![],
    };
    let user_ids: Vec<String> = match stmt.query_map([], |row| row.get::<_, String>(0)) {
        Ok(r) => r.filter_map(|x| x.ok()).collect(),
        Err(_) => return vec![],
    };
    user_ids
        .iter()
        .map(|uid| {
            // 统计该账号拥有的项目数（display_owner = uid）
            // R6：排除软删除项目
            let project_count: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM project_identity pi \
                     LEFT JOIN project_observation po ON po.project_id = pi.project_id \
                     LEFT JOIN project_source_assignment psa ON psa.project_id = pi.project_id \
                     WHERE pi.soft_deleted = 0 \
                       AND COALESCE(psa.user_assigned_owner, po.first_observed_owner) = ?1",
                    rusqlite::params![uid],
                    |row| row.get(0),
                )
                .unwrap_or(0);
            // 统计这些项目下的可见会话数
            let session_count: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM session_projection sp \
                     WHERE sp.soft_deleted = 0 AND \
                     COALESCE(\
                       (SELECT psa.user_assigned_owner FROM project_source_assignment psa \
                        WHERE psa.project_id = sp.project_id), \
                       (SELECT po.first_observed_owner FROM project_observation po \
                        WHERE po.project_id = sp.project_id)\
                     ) = ?1",
                    rusqlite::params![uid],
                    |row| row.get(0),
                )
                .unwrap_or(0);
            BrowseAccountNode {
                user_id: uid.clone(),
                display_label: uid.clone(),
                project_count: project_count as u64,
                session_count: session_count as u64,
            }
        })
        .collect()
}

/// 计算历史浏览摘要。
fn compute_history_summary(conn: &Connection) -> HistoryBrowseSummary {
    let account_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM seen_account", [], |row| row.get(0))
        .unwrap_or(0);
    // R6：visible 排除 soft_deleted，soft_deleted 单独计数
    let visible_projects: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM project_identity WHERE soft_deleted = 0",
            [],
            |row| row.get(0),
        )
        .unwrap_or(0);
    let soft_deleted_projects: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM project_identity WHERE soft_deleted = 1",
            [],
            |row| row.get(0),
        )
        .unwrap_or(0);
    let visible_sessions: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM session_projection WHERE soft_deleted = 0",
            [],
            |row| row.get(0),
        )
        .unwrap_or(0);
    let soft_deleted_sessions: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM session_projection WHERE soft_deleted = 1",
            [],
            |row| row.get(0),
        )
        .unwrap_or(0);
    let soft_deleted_messages: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM message_projection WHERE soft_deleted = 1",
            [],
            |row| row.get(0),
        )
        .unwrap_or(0);
    HistoryBrowseSummary {
        visible_account_count: account_count as u64,
        visible_project_count: visible_projects as u64,
        visible_session_count: visible_sessions as u64,
        soft_deleted_project_count: soft_deleted_projects as u64,
        soft_deleted_session_count: soft_deleted_sessions as u64,
        soft_deleted_message_count: soft_deleted_messages as u64,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::work_cn_normalizer::WorkCnSourceNormalizer;
    use rusqlite::Connection;
    use tempfile::tempdir;
    use traesync_domain::SnapshotFingerprint;

    /// 目录库测试 key（合成，不接触真实 TRAE 数据）
    const TEST_CATALOG_KEY: &str =
        "aaaabbbbccccdddd1111222233334444aaaabbbbccccdddd1111222233334444";

    fn catalog_path(dir: &Path) -> PathBuf {
        dir.join("catalog.db")
    }

    /// 构造明文快照 fixture DB（可指定 project owner）
    fn make_snapshot_fixture(dir: &Path, owner: &str) {
        let db_path = dir.join("database.db");
        let conn = Connection::open(&db_path).unwrap();
        conn.execute_batch(
            r#"
            CREATE TABLE project (
                project_id TEXT PRIMARY KEY, user_id TEXT NOT NULL,
                biz_project_id TEXT NOT NULL, deleted_at INTEGER DEFAULT 0
            );
            CREATE TABLE chat_session (
                session_id TEXT PRIMARY KEY, project_id TEXT NOT NULL,
                deleted_at INTEGER DEFAULT 0
            );
            CREATE TABLE chat_message (
                message_id TEXT PRIMARY KEY, session_id TEXT NOT NULL,
                role TEXT, content TEXT, deleted_at INTEGER DEFAULT 0
            );
            "#,
        )
        .unwrap();
        conn.execute("INSERT INTO project VALUES ('p1', ?1, 'biz-1', 0)", [owner])
            .unwrap();
        conn.execute_batch(
            "INSERT INTO chat_session VALUES ('s1', 'p1', 0); \
             INSERT INTO chat_session VALUES ('s2', 'p1', 100); \
             INSERT INTO chat_message VALUES ('m1', 's1', 'user', 'hello world', 0); \
             INSERT INTO chat_message VALUES ('m2', 's1', 'assistant', 'hi there', 0); \
             INSERT INTO chat_message VALUES ('m3', 's1', 'user', 'deleted msg', 300);",
        )
        .unwrap();
        drop(conn);
    }

    fn make_snapshot_meta() -> SourceSnapshotMeta {
        SourceSnapshotMeta {
            snapshot_id: SnapshotId::new(),
            platform_id: "work_cn".to_string(),
            data_location_id: "loc-1".to_string(),
            product_version: "1.0".to_string(),
            schema_fingerprint: "fp".to_string(),
            mapping_version: "work_cn_v1".to_string(),
            account_evidence_ref: None,
            captured_at: SystemTime::now(),
            files: vec![],
            fingerprint: SnapshotFingerprint("fp".to_string()),
        }
    }

    fn setup_repo(dir: &Path) -> SqlCipherCatalogRepository {
        let repo = SqlCipherCatalogRepository::new(catalog_path(dir), TEST_CATALOG_KEY.to_string());
        assert!(repo.ensure_initialized(), "首次初始化应返回 true");
        repo
    }

    #[test]
    fn ensure_initialized_creates_tables_then_returns_false() {
        let dir = tempdir().unwrap();
        let repo =
            SqlCipherCatalogRepository::new(catalog_path(dir.path()), TEST_CATALOG_KEY.to_string());
        // 首次：创建
        assert!(repo.ensure_initialized(), "首次应返回 true");
        // 再次：已存在
        assert!(!repo.ensure_initialized(), "再次应返回 false");
    }

    #[test]
    fn project_snapshot_writes_projects_sessions_messages() {
        let dir = tempdir().unwrap();
        let snapshot_dir = dir.path().join("snap-1");
        std::fs::create_dir_all(&snapshot_dir).unwrap();
        make_snapshot_fixture(&snapshot_dir, "user-A");

        let repo = setup_repo(dir.path());
        let normalizer = WorkCnSourceNormalizer::new(TEST_CATALOG_KEY.to_string());
        let meta = make_snapshot_meta();
        let result = repo.project_snapshot(&meta, &snapshot_dir, &normalizer);
        assert!(result.is_ok(), "project_snapshot 应成功");

        // browse 应返回 1 个项目（fixture 只有 p1，未软删除）
        let browse = repo.browse();
        assert_eq!(browse.projects.len(), 1);
        assert_eq!(browse.projects[0].project_id, "p1");
        // 1 个可见会话（s2 软删除排除）
        assert_eq!(browse.sessions.len(), 1);
        assert_eq!(
            browse.sessions[0].session_identity.original_session_id,
            "s1"
        );
    }

    #[test]
    fn browse_excludes_soft_deleted() {
        let dir = tempdir().unwrap();
        let snapshot_dir = dir.path().join("snap-1");
        std::fs::create_dir_all(&snapshot_dir).unwrap();
        make_snapshot_fixture(&snapshot_dir, "user-A");

        let repo = setup_repo(dir.path());
        let normalizer = WorkCnSourceNormalizer::new(TEST_CATALOG_KEY.to_string());
        let meta = make_snapshot_meta();
        repo.project_snapshot(&meta, &snapshot_dir, &normalizer)
            .unwrap();

        let browse = repo.browse();
        // s1 可见，s2 软删除排除
        assert_eq!(browse.sessions.len(), 1);
        // 账号树有 user-A
        assert_eq!(browse.accounts.len(), 1);
        assert_eq!(browse.accounts[0].user_id, "user-A");
        // summary 可见会话数 = 1，软删除会话数 = 1
        assert_eq!(browse.summary.visible_session_count, 1);
        assert_eq!(browse.summary.soft_deleted_session_count, 1);
    }

    #[test]
    fn search_messages_fts_query() {
        let dir = tempdir().unwrap();
        let snapshot_dir = dir.path().join("snap-1");
        std::fs::create_dir_all(&snapshot_dir).unwrap();
        make_snapshot_fixture(&snapshot_dir, "user-A");

        let repo = setup_repo(dir.path());
        let normalizer = WorkCnSourceNormalizer::new(TEST_CATALOG_KEY.to_string());
        let meta = make_snapshot_meta();
        repo.project_snapshot(&meta, &snapshot_dir, &normalizer)
            .unwrap();

        // 搜索 "hello" —— 应命中 m1
        let hits = repo.search_messages("hello");
        assert!(!hits.is_empty(), "应搜到 hello");
        assert!(hits.iter().any(|h| h.message_id == "m1"));

        // 搜索 "deleted" —— m3 软删除，应被排除
        let hits_deleted = repo.search_messages("deleted");
        assert!(hits_deleted.is_empty(), "软删除消息应被排除");
    }

    #[test]
    fn owner_observation_appends_and_first_observed_owner_unchanged() {
        let dir = tempdir().unwrap();
        let repo = setup_repo(dir.path());
        let normalizer = WorkCnSourceNormalizer::new(TEST_CATALOG_KEY.to_string());

        // 第一次扫描：owner = user-A
        let snap1 = dir.path().join("snap-1");
        std::fs::create_dir_all(&snap1).unwrap();
        make_snapshot_fixture(&snap1, "user-A");
        let meta1 = make_snapshot_meta();
        repo.project_snapshot(&meta1, &snap1, &normalizer).unwrap();

        // 第二次扫描：owner = user-B（模拟账号迁移）
        let snap2 = dir.path().join("snap-2");
        std::fs::create_dir_all(&snap2).unwrap();
        make_snapshot_fixture(&snap2, "user-B");
        let meta2 = make_snapshot_meta();
        repo.project_snapshot(&meta2, &snap2, &normalizer).unwrap();

        // Gate E：first_observed_owner 永不变化
        let obs = repo.read_project_observation("p1").expect("应有观察记录");
        assert_eq!(
            obs.first_observed_owner, "user-A",
            "first_observed_owner 应保持 user-A"
        );
        assert_eq!(
            obs.current_live_owner, "user-B",
            "current_live_owner 应更新为 user-B"
        );
        // owner_observations 应有 2 条（每次扫描追加）
        assert_eq!(obs.owner_observations.len(), 2, "应有 2 条 owner 观察");
    }

    #[test]
    fn repeated_scan_does_not_create_duplicate_rows() {
        let dir = tempdir().unwrap();
        let repo = setup_repo(dir.path());
        let normalizer = WorkCnSourceNormalizer::new(TEST_CATALOG_KEY.to_string());

        let snap = dir.path().join("snap-1");
        std::fs::create_dir_all(&snap).unwrap();
        make_snapshot_fixture(&snap, "user-A");

        // 两次相同扫描
        repo.project_snapshot(&make_snapshot_meta(), &snap, &normalizer)
            .unwrap();
        repo.project_snapshot(&make_snapshot_meta(), &snap, &normalizer)
            .unwrap();

        // Gate I：session_version 不应重复（相同内容图哈希，INSERT OR IGNORE）
        let versions = repo.read_all_session_versions();
        // s1 和 s2 各一个版本，s2 软删除但仍写入 session_version
        // 内容图哈希相同 -> INSERT OR IGNORE 不重复
        assert_eq!(versions.len(), 2, "应有 2 个会话版本（s1 + s2），不应重复");

        // session_identity 也不重复
        let browse = repo.browse();
        assert_eq!(browse.sessions.len(), 1, "可见会话仍为 1（s1）");
    }

    #[test]
    fn soft_deleted_items_retained_but_excluded_from_browse_search() {
        let dir = tempdir().unwrap();
        let snapshot_dir = dir.path().join("snap-1");
        std::fs::create_dir_all(&snapshot_dir).unwrap();
        make_snapshot_fixture(&snapshot_dir, "user-A");

        let repo = setup_repo(dir.path());
        let normalizer = WorkCnSourceNormalizer::new(TEST_CATALOG_KEY.to_string());
        let meta = make_snapshot_meta();
        repo.project_snapshot(&meta, &snapshot_dir, &normalizer)
            .unwrap();

        // Gate J：诊断完整性——retained 含软删除，visible 不含
        let diag = repo.diagnostic_integrity();
        // 消息：m1/m2 可见，m3 软删除
        assert_eq!(diag.visible_messages, 2, "可见消息应为 2");
        assert_eq!(diag.retained_messages, 3, "保留消息应为 3（含软删除 m3）");
        // 会话：s1 可见，s2 软删除
        assert_eq!(diag.visible_sessions, 1, "可见会话应为 1");
        assert_eq!(diag.retained_sessions, 2, "保留会话应为 2（含软删除 s2）");

        // read_conversation_preview 排除软删除消息
        let preview = repo.read_conversation_preview(&SessionIdentity::new("work_cn", "s1"));
        assert!(preview.is_some());
        let preview = preview.unwrap();
        assert_eq!(preview.messages.len(), 2, "预览应排除软删除消息 m3");
    }

    #[test]
    fn assign_project_source_does_not_modify_project_observation() {
        let dir = tempdir().unwrap();
        let snapshot_dir = dir.path().join("snap-1");
        std::fs::create_dir_all(&snapshot_dir).unwrap();
        make_snapshot_fixture(&snapshot_dir, "user-A");

        let repo = setup_repo(dir.path());
        let normalizer = WorkCnSourceNormalizer::new(TEST_CATALOG_KEY.to_string());
        let meta = make_snapshot_meta();
        repo.project_snapshot(&meta, &snapshot_dir, &normalizer)
            .unwrap();

        // 读取分配前的观察
        let before = repo.read_project_observation("p1").unwrap();

        // 用户分配来源到 user-X
        let assignment = ProjectSourceAssignment {
            project_id: "p1".to_string(),
            user_assigned_owner: Some("user-X".to_string()),
            assigned_at: SystemTime::now(),
        };
        assert!(repo.assign_project_source(&assignment), "分配应成功");

        // Gate E：project_observation 不变
        let after = repo.read_project_observation("p1").unwrap();
        assert_eq!(after.first_observed_owner, before.first_observed_owner);
        assert_eq!(after.current_live_owner, before.current_live_owner);
        assert_eq!(
            after.owner_observations.len(),
            before.owner_observations.len()
        );

        // 但 display_owner 应变为 user-X
        let projects = repo.browse_projects_by_account("user-X");
        assert!(!projects.is_empty(), "user-X 应有项目");
        let projects_a = repo.browse_projects_by_account("user-A");
        assert!(
            projects_a.is_empty(),
            "user-A 应不再有项目（已分配给 user-X）"
        );
    }

    #[test]
    fn diagnostic_integrity_returns_visible_and_retained_counts() {
        let dir = tempdir().unwrap();
        let snapshot_dir = dir.path().join("snap-1");
        std::fs::create_dir_all(&snapshot_dir).unwrap();
        make_snapshot_fixture(&snapshot_dir, "user-A");

        let repo = setup_repo(dir.path());
        let normalizer = WorkCnSourceNormalizer::new(TEST_CATALOG_KEY.to_string());
        let meta = make_snapshot_meta();
        repo.project_snapshot(&meta, &snapshot_dir, &normalizer)
            .unwrap();

        let diag = repo.diagnostic_integrity();
        // 项目：1（p1），无软删除项目
        assert_eq!(diag.visible_projects, 1);
        assert_eq!(diag.retained_projects, 1);
        // 会话：1 可见 + 1 软删除
        assert_eq!(diag.visible_sessions, 1);
        assert_eq!(diag.retained_sessions, 2);
        // 消息：2 可见 + 1 软删除
        assert_eq!(diag.visible_messages, 2);
        assert_eq!(diag.retained_messages, 3);
    }

    // ============== TDD #6：SQLCipher 正确 key 重开、错误 key 拒绝、无明文旁路 ==============

    #[test]
    fn correct_key_reopen_succeeds_and_wrong_key_returns_empty() {
        // 正确 key 初始化并投影数据后，错误 key 重开应返回空结果（无法解密）
        let dir = tempdir().unwrap();
        let snapshot_dir = dir.path().join("snap-1");
        std::fs::create_dir_all(&snapshot_dir).unwrap();
        make_snapshot_fixture(&snapshot_dir, "user-A");

        let repo = setup_repo(dir.path());
        let normalizer = WorkCnSourceNormalizer::new(TEST_CATALOG_KEY.to_string());
        repo.project_snapshot(&make_snapshot_meta(), &snapshot_dir, &normalizer)
            .unwrap();

        // 正确 key 能读到数据
        assert_eq!(repo.browse().projects.len(), 1);
        assert_eq!(repo.browse().accounts.len(), 1);

        // 错误 key 重开——无法解密，返回空
        let wrong_key = "00000000000000000000000000000000aaaaaaaa0000000000000000000000000000";
        let wrong_repo =
            SqlCipherCatalogRepository::new(catalog_path(dir.path()), wrong_key.to_string());
        assert_eq!(
            wrong_repo.browse().projects.len(),
            0,
            "错误 key 应无法读取数据"
        );
        assert_eq!(wrong_repo.browse().accounts.len(), 0);
        assert_eq!(wrong_repo.search_messages("hello").len(), 0);
    }

    #[test]
    fn no_plaintext_sidecar_index_exists() {
        // 目录库文件本身是加密的，不存在明文旁路索引
        let dir = tempdir().unwrap();
        let snapshot_dir = dir.path().join("snap-1");
        std::fs::create_dir_all(&snapshot_dir).unwrap();
        make_snapshot_fixture(&snapshot_dir, "user-A");

        let repo = setup_repo(dir.path());
        let normalizer = WorkCnSourceNormalizer::new(TEST_CATALOG_KEY.to_string());
        repo.project_snapshot(&make_snapshot_meta(), &snapshot_dir, &normalizer)
            .unwrap();

        // 目录库只有一个 catalog.db 文件，不存在 .fts 或明文索引文件
        let entries: Vec<_> = std::fs::read_dir(dir.path()).unwrap().collect();
        let db_files: Vec<String> = entries
            .iter()
            .filter_map(|e| {
                e.as_ref()
                    .ok()
                    .and_then(|e| e.file_name().to_str().map(|s| s.to_string()))
            })
            .filter(|s| !s.starts_with("snap"))
            .collect();
        assert!(
            db_files.iter().any(|f| f == "catalog.db"),
            "应有 catalog.db"
        );
        // 不存在明文旁路文件
        assert!(
            !db_files
                .iter()
                .any(|f| f.contains("plaintext") || f.contains(".fts") || f.contains("sidecar")),
            "不应有明文旁路索引文件: {:?}",
            db_files
        );
    }

    // ============== TDD #7：目录库事务失败不留下部分投影 ==============

    #[test]
    fn project_snapshot_with_wrong_key_leaves_no_projection() {
        // 用正确 key 初始化目录库后，用错误 key 调用 project_snapshot 应失败
        // 且不留下任何部分投影
        let dir = tempdir().unwrap();
        let snapshot_dir = dir.path().join("snap-1");
        std::fs::create_dir_all(&snapshot_dir).unwrap();
        make_snapshot_fixture(&snapshot_dir, "user-A");

        // 正确 key 初始化
        let correct_repo = setup_repo(dir.path());
        let normalizer = WorkCnSourceNormalizer::new(TEST_CATALOG_KEY.to_string());

        // 错误 key 的 repo 调用 project_snapshot
        let wrong_key = "00000000000000000000000000000000aaaaaaaa0000000000000000000000000000";
        let wrong_repo =
            SqlCipherCatalogRepository::new(catalog_path(dir.path()), wrong_key.to_string());
        let result = wrong_repo.project_snapshot(&make_snapshot_meta(), &snapshot_dir, &normalizer);
        assert!(result.is_err(), "错误 key 的 project_snapshot 应失败");
        assert_eq!(
            result.unwrap_err(),
            ScanFailureReason::CatalogTransactionFailed
        );

        // 用正确 key 验证：目录库中不应有任何数据（无部分投影）
        let browse = correct_repo.browse();
        assert_eq!(browse.projects.len(), 0, "不应有部分项目投影");
        assert_eq!(browse.sessions.len(), 0, "不应有部分会话投影");
        assert_eq!(browse.accounts.len(), 0, "不应有部分账号投影");
    }

    #[test]
    fn project_snapshot_normalizer_failure_preserves_existing_data() {
        // 正常投影后，用不存在的快照目录再调 project_snapshot
        // normalizer 返回空数据，但不应破坏已有数据
        let dir = tempdir().unwrap();
        let snapshot_dir = dir.path().join("snap-1");
        std::fs::create_dir_all(&snapshot_dir).unwrap();
        make_snapshot_fixture(&snapshot_dir, "user-A");

        let repo = setup_repo(dir.path());
        let normalizer = WorkCnSourceNormalizer::new(TEST_CATALOG_KEY.to_string());
        repo.project_snapshot(&make_snapshot_meta(), &snapshot_dir, &normalizer)
            .unwrap();
        assert_eq!(repo.browse().projects.len(), 1);

        // 不存在的快照目录——normalizer 返回空 Vec
        let bad_dir = dir.path().join("nonexistent-snapshot");
        let result = repo.project_snapshot(&make_snapshot_meta(), &bad_dir, &normalizer);
        // 空数据不导致事务失败，但也不破坏已有数据
        assert!(result.is_ok(), "空投影应成功");

        // 原有数据完好
        let browse = repo.browse();
        assert_eq!(browse.projects.len(), 1, "原有项目应完好");
        assert_eq!(browse.sessions.len(), 1, "原有会话应完好");
    }

    // ============== TDD #9：相同标题不同 session_id 保持独立 ==============

    #[test]
    fn same_title_different_session_ids_remain_distinct() {
        // 两个非软删除会话有相同标题（空字符串），但 session_id 不同
        // browse 应返回两个独立会话
        let dir = tempdir().unwrap();
        let snapshot_dir = dir.path().join("snap-1");
        std::fs::create_dir_all(&snapshot_dir).unwrap();

        // 创建含两个非软删除会话的 fixture
        let db_path = snapshot_dir.join("database.db");
        let conn = Connection::open(&db_path).unwrap();
        conn.execute_batch(
            r#"
            CREATE TABLE project (project_id TEXT PRIMARY KEY, user_id TEXT NOT NULL, biz_project_id TEXT NOT NULL, deleted_at INTEGER DEFAULT 0);
            CREATE TABLE chat_session (session_id TEXT PRIMARY KEY, project_id TEXT NOT NULL, deleted_at INTEGER DEFAULT 0);
            CREATE TABLE chat_message (message_id TEXT PRIMARY KEY, session_id TEXT NOT NULL, role TEXT, content TEXT, deleted_at INTEGER DEFAULT 0);
            INSERT INTO project VALUES ('p1', 'user-A', 'biz-1', 0);
            INSERT INTO chat_session VALUES ('session-aaa', 'p1', 0);
            INSERT INTO chat_session VALUES ('session-bbb', 'p1', 0);
            INSERT INTO chat_message VALUES ('m1', 'session-aaa', 'user', 'hello', 0);
            INSERT INTO chat_message VALUES ('m2', 'session-bbb', 'user', 'world', 0);
            "#,
        ).unwrap();
        drop(conn);

        let repo = setup_repo(dir.path());
        let normalizer = WorkCnSourceNormalizer::new(TEST_CATALOG_KEY.to_string());
        repo.project_snapshot(&make_snapshot_meta(), &snapshot_dir, &normalizer)
            .unwrap();

        // browse 应返回两个独立会话
        let browse = repo.browse();
        assert_eq!(browse.sessions.len(), 2, "两个非软删除会话都应可见");
        let ids: Vec<&str> = browse
            .sessions
            .iter()
            .map(|s| s.session_identity.original_session_id.as_str())
            .collect();
        assert!(ids.contains(&"session-aaa"));
        assert!(ids.contains(&"session-bbb"));

        // 两个会话的对话预览各自独立
        let preview_aaa = repo
            .read_conversation_preview(&SessionIdentity::new("work_cn", "session-aaa"))
            .unwrap();
        assert_eq!(preview_aaa.messages.len(), 1);
        assert_eq!(preview_aaa.messages[0].content_excerpt, "hello");

        let preview_bbb = repo
            .read_conversation_preview(&SessionIdentity::new("work_cn", "session-bbb"))
            .unwrap();
        assert_eq!(preview_bbb.messages.len(), 1);
        assert_eq!(preview_bbb.messages[0].content_excerpt, "world");
    }

    // ============== TDD #10：A→B→A owner 观察，first_observed_owner 不变 ==============

    #[test]
    fn a_b_a_ownership_preserves_first_observed_owner() {
        // 三次扫描：A→B→A，first_observed_owner 始终为 A
        let dir = tempdir().unwrap();
        let repo = setup_repo(dir.path());
        let normalizer = WorkCnSourceNormalizer::new(TEST_CATALOG_KEY.to_string());

        // 第一次：owner = user-A
        let snap1 = dir.path().join("snap-1");
        std::fs::create_dir_all(&snap1).unwrap();
        make_snapshot_fixture(&snap1, "user-A");
        repo.project_snapshot(&make_snapshot_meta(), &snap1, &normalizer)
            .unwrap();

        // 第二次：owner = user-B
        let snap2 = dir.path().join("snap-2");
        std::fs::create_dir_all(&snap2).unwrap();
        make_snapshot_fixture(&snap2, "user-B");
        repo.project_snapshot(&make_snapshot_meta(), &snap2, &normalizer)
            .unwrap();

        // 第三次：owner 回到 user-A
        let snap3 = dir.path().join("snap-3");
        std::fs::create_dir_all(&snap3).unwrap();
        make_snapshot_fixture(&snap3, "user-A");
        repo.project_snapshot(&make_snapshot_meta(), &snap3, &normalizer)
            .unwrap();

        // Gate E：first_observed_owner 始终为 user-A
        let obs = repo.read_project_observation("p1").expect("应有观察记录");
        assert_eq!(
            obs.first_observed_owner, "user-A",
            "first_observed_owner 应保持 user-A（A→B→A 后仍不变）"
        );
        assert_eq!(
            obs.current_live_owner, "user-A",
            "current_live_owner 应为最后一次的 user-A"
        );
        // 三次扫描应追加 3 条 owner 观察
        assert_eq!(
            obs.owner_observations.len(),
            3,
            "应有 3 条 owner 观察（A→B→A）"
        );
    }

    // ============== TDD #15：Unclassified 保留旧投影 ==============

    #[test]
    fn unclassified_preserves_old_session_projection() {
        // 第一次扫描：会话有有效 message_id 的消息
        // 第二次扫描：会话的消息 message_id 为空 → Unclassified
        // session_projection 的 active_content_graph_hash 应保持旧值
        let dir = tempdir().unwrap();
        let repo = setup_repo(dir.path());
        let normalizer = WorkCnSourceNormalizer::new(TEST_CATALOG_KEY.to_string());

        // 第一次：有效 message_id
        let snap1 = dir.path().join("snap-1");
        std::fs::create_dir_all(&snap1).unwrap();
        {
            let conn = Connection::open(snap1.join("database.db")).unwrap();
            conn.execute_batch(
                r#"
                CREATE TABLE project (project_id TEXT PRIMARY KEY, user_id TEXT NOT NULL, biz_project_id TEXT NOT NULL, deleted_at INTEGER DEFAULT 0);
                CREATE TABLE chat_session (session_id TEXT PRIMARY KEY, project_id TEXT NOT NULL, deleted_at INTEGER DEFAULT 0);
                CREATE TABLE chat_message (message_id TEXT PRIMARY KEY, session_id TEXT NOT NULL, role TEXT, content TEXT, deleted_at INTEGER DEFAULT 0);
                INSERT INTO project VALUES ('p1', 'user-A', 'biz-1', 0);
                INSERT INTO chat_session VALUES ('s1', 'p1', 0);
                INSERT INTO chat_message VALUES ('m1', 's1', 'user', 'hello', 0);
                INSERT INTO chat_message VALUES ('m2', 's1', 'assistant', 'hi', 0);
                "#,
            ).unwrap();
        }
        repo.project_snapshot(&make_snapshot_meta(), &snap1, &normalizer)
            .unwrap();

        // 记录第一次的 active_content_graph_hash
        let proj1 = repo
            .read_session_projection(&SessionIdentity::new("work_cn", "s1"))
            .expect("应有投影");
        let hash_after_first = proj1.active_content_graph_hash.clone();

        // 第二次：消息 message_id 为空 → classify 返回 Unclassified
        let snap2 = dir.path().join("snap-2");
        std::fs::create_dir_all(&snap2).unwrap();
        {
            let conn = Connection::open(snap2.join("database.db")).unwrap();
            conn.execute_batch(
                r#"
                CREATE TABLE project (project_id TEXT PRIMARY KEY, user_id TEXT NOT NULL, biz_project_id TEXT NOT NULL, deleted_at INTEGER DEFAULT 0);
                CREATE TABLE chat_session (session_id TEXT PRIMARY KEY, project_id TEXT NOT NULL, deleted_at INTEGER DEFAULT 0);
                CREATE TABLE chat_message (message_id TEXT PRIMARY KEY, session_id TEXT NOT NULL, role TEXT, content TEXT, deleted_at INTEGER DEFAULT 0);
                INSERT INTO project VALUES ('p1', 'user-A', 'biz-1', 0);
                INSERT INTO chat_session VALUES ('s1', 'p1', 0);
                -- message_id 为空字符串 → classify 返回 Unclassified
                -- 注意：PRIMARY KEY 不允许空字符串重复，所以只插一条
                INSERT INTO chat_message VALUES ('', 's1', 'user', 'changed', 0);
                "#,
            ).unwrap();
        }
        repo.project_snapshot(&make_snapshot_meta(), &snap2, &normalizer)
            .unwrap();

        // Gate I/R5：Unclassified 应保留旧投影
        let proj2 = repo
            .read_session_projection(&SessionIdentity::new("work_cn", "s1"))
            .expect("应有投影");
        assert_eq!(
            proj2.active_content_graph_hash, hash_after_first,
            "Unclassified 应保留旧 active_content_graph_hash"
        );

        // R5：preview 必须解析活跃版本内容（旧消息），而非最新导入行
        let preview = repo
            .read_conversation_preview(&SessionIdentity::new("work_cn", "s1"))
            .expect("应有预览");
        let contents: Vec<&str> = preview
            .messages
            .iter()
            .map(|m| m.content_excerpt.as_str())
            .collect();
        assert!(
            contents.contains(&"hello"),
            "Unclassified 后预览应返回旧内容 'hello'，实际: {:?}",
            contents
        );
        assert!(
            !contents.iter().any(|c| c.contains("changed")),
            "Unclassified 后预览不应包含新内容 'changed'，实际: {:?}",
            contents
        );

        // R5：search 必须解析活跃版本内容——搜到旧 'hello'，搜不到新 'changed'
        let hits_hello = repo.search_messages("hello");
        assert!(
            !hits_hello.is_empty(),
            "Unclassified 后应仍能搜到旧内容 'hello'"
        );
        let hits_changed = repo.search_messages("changed");
        assert!(
            hits_changed.is_empty(),
            "Unclassified 后不应搜到新内容 'changed'，实际命中: {}",
            hits_changed.len()
        );
    }

    // ============== R5：Forked 保留旧活跃内容可浏览/可搜索 ==============

    #[test]
    fn r5_forked_preserves_old_content_browseable_and_searchable() {
        // 第一次扫描：m1="hello", m2="hi"
        // 第二次扫描：m1="hello FORKED", m2="hi" → 内容修改 → Forked
        // R5：Forked 后 preview/search 必须返回旧内容，新版本保留在 session_version
        let dir = tempdir().unwrap();
        let repo = setup_repo(dir.path());
        let normalizer = WorkCnSourceNormalizer::new(TEST_CATALOG_KEY.to_string());

        // 第一次扫描
        let snap1 = dir.path().join("snap-1");
        std::fs::create_dir_all(&snap1).unwrap();
        {
            let conn = Connection::open(snap1.join("database.db")).unwrap();
            conn.execute_batch(
                r#"
                CREATE TABLE project (project_id TEXT PRIMARY KEY, user_id TEXT NOT NULL, biz_project_id TEXT NOT NULL, deleted_at INTEGER DEFAULT 0);
                CREATE TABLE chat_session (session_id TEXT PRIMARY KEY, project_id TEXT NOT NULL, deleted_at INTEGER DEFAULT 0);
                CREATE TABLE chat_message (message_id TEXT PRIMARY KEY, session_id TEXT NOT NULL, role TEXT, content TEXT, deleted_at INTEGER DEFAULT 0);
                INSERT INTO project VALUES ('p1', 'user-A', 'biz-1', 0);
                INSERT INTO chat_session VALUES ('s1', 'p1', 0);
                INSERT INTO chat_message VALUES ('m1', 's1', 'user', 'hello', 0);
                INSERT INTO chat_message VALUES ('m2', 's1', 'assistant', 'hi', 0);
                "#,
            ).unwrap();
        }
        repo.project_snapshot(&make_snapshot_meta(), &snap1, &normalizer)
            .unwrap();

        // 记录第一次的活跃哈希
        let proj1 = repo
            .read_session_projection(&SessionIdentity::new("work_cn", "s1"))
            .expect("应有投影");
        let hash_after_first = proj1.active_content_graph_hash.clone();

        // 第二次扫描：m1 内容修改 → Forked
        let snap2 = dir.path().join("snap-2");
        std::fs::create_dir_all(&snap2).unwrap();
        {
            let conn = Connection::open(snap2.join("database.db")).unwrap();
            conn.execute_batch(
                r#"
                CREATE TABLE project (project_id TEXT PRIMARY KEY, user_id TEXT NOT NULL, biz_project_id TEXT NOT NULL, deleted_at INTEGER DEFAULT 0);
                CREATE TABLE chat_session (session_id TEXT PRIMARY KEY, project_id TEXT NOT NULL, deleted_at INTEGER DEFAULT 0);
                CREATE TABLE chat_message (message_id TEXT PRIMARY KEY, session_id TEXT NOT NULL, role TEXT, content TEXT, deleted_at INTEGER DEFAULT 0);
                INSERT INTO project VALUES ('p1', 'user-A', 'biz-1', 0);
                INSERT INTO chat_session VALUES ('s1', 'p1', 0);
                INSERT INTO chat_message VALUES ('m1', 's1', 'user', 'hello FORKED', 0);
                INSERT INTO chat_message VALUES ('m2', 's1', 'assistant', 'hi', 0);
                "#,
            ).unwrap();
        }
        repo.project_snapshot(&make_snapshot_meta(), &snap2, &normalizer)
            .unwrap();

        // R5：session_projection 活跃哈希应保持旧值
        let proj2 = repo
            .read_session_projection(&SessionIdentity::new("work_cn", "s1"))
            .expect("应有投影");
        assert_eq!(
            proj2.active_content_graph_hash, hash_after_first,
            "Forked 应保留旧 active_content_graph_hash"
        );

        // R5：preview 必须返回旧内容 'hello'，而非新内容 'hello FORKED'
        let preview = repo
            .read_conversation_preview(&SessionIdentity::new("work_cn", "s1"))
            .expect("应有预览");
        let contents: Vec<&str> = preview
            .messages
            .iter()
            .map(|m| m.content_excerpt.as_str())
            .collect();
        assert!(
            contents.contains(&"hello"),
            "Forked 后预览应返回旧内容 'hello'，实际: {:?}",
            contents
        );
        assert!(
            !contents.iter().any(|c| c.contains("FORKED")),
            "Forked 后预览不应包含新内容 'hello FORKED'，实际: {:?}",
            contents
        );

        // R5：search 应搜到旧 'hello'，搜不到 'FORKED'
        let hits_hello = repo.search_messages("hello");
        assert!(!hits_hello.is_empty(), "Forked 后应仍能搜到旧内容 'hello'");
        let hits_forked = repo.search_messages("FORKED");
        assert!(
            hits_forked.is_empty(),
            "Forked 后不应搜到新内容 'FORKED'，实际命中: {}",
            hits_forked.len()
        );

        // R5：session_version 应保留两个版本（旧 + 新 Forked）
        let versions = repo.read_all_session_versions();
        let s1_versions: Vec<_> = versions
            .iter()
            .filter(|v| v.session_identity.original_session_id == "s1")
            .collect();
        assert_eq!(
            s1_versions.len(),
            2,
            "Forked 后应保留 2 个会话版本（旧 + 新），实际: {}",
            s1_versions.len()
        );
        // 至少有一个 Forked 分类
        assert!(
            s1_versions
                .iter()
                .any(|v| v.classification == VersionClassification::Forked),
            "应有 Forked 分类版本"
        );
    }

    #[test]
    fn r5_fast_forward_advances_active_projection() {
        // R5 反例验证：FastForward 应推进活跃投影（删除旧 + 写入新）
        // 第一次扫描：m1="hello"
        // 第二次扫描：m1="hello", m2="hi"（追加）→ FastForward
        // preview/search 应返回新内容
        let dir = tempdir().unwrap();
        let repo = setup_repo(dir.path());
        let normalizer = WorkCnSourceNormalizer::new(TEST_CATALOG_KEY.to_string());

        // 第一次扫描
        let snap1 = dir.path().join("snap-1");
        std::fs::create_dir_all(&snap1).unwrap();
        {
            let conn = Connection::open(snap1.join("database.db")).unwrap();
            conn.execute_batch(
                r#"
                CREATE TABLE project (project_id TEXT PRIMARY KEY, user_id TEXT NOT NULL, biz_project_id TEXT NOT NULL, deleted_at INTEGER DEFAULT 0);
                CREATE TABLE chat_session (session_id TEXT PRIMARY KEY, project_id TEXT NOT NULL, deleted_at INTEGER DEFAULT 0);
                CREATE TABLE chat_message (message_id TEXT PRIMARY KEY, session_id TEXT NOT NULL, role TEXT, content TEXT, deleted_at INTEGER DEFAULT 0);
                INSERT INTO project VALUES ('p1', 'user-A', 'biz-1', 0);
                INSERT INTO chat_session VALUES ('s1', 'p1', 0);
                INSERT INTO chat_message VALUES ('m1', 's1', 'user', 'hello', 0);
                "#,
            ).unwrap();
        }
        repo.project_snapshot(&make_snapshot_meta(), &snap1, &normalizer)
            .unwrap();

        // 第二次扫描：追加 m2 → FastForward
        let snap2 = dir.path().join("snap-2");
        std::fs::create_dir_all(&snap2).unwrap();
        {
            let conn = Connection::open(snap2.join("database.db")).unwrap();
            conn.execute_batch(
                r#"
                CREATE TABLE project (project_id TEXT PRIMARY KEY, user_id TEXT NOT NULL, biz_project_id TEXT NOT NULL, deleted_at INTEGER DEFAULT 0);
                CREATE TABLE chat_session (session_id TEXT PRIMARY KEY, project_id TEXT NOT NULL, deleted_at INTEGER DEFAULT 0);
                CREATE TABLE chat_message (message_id TEXT PRIMARY KEY, session_id TEXT NOT NULL, role TEXT, content TEXT, deleted_at INTEGER DEFAULT 0);
                INSERT INTO project VALUES ('p1', 'user-A', 'biz-1', 0);
                INSERT INTO chat_session VALUES ('s1', 'p1', 0);
                INSERT INTO chat_message VALUES ('m1', 's1', 'user', 'hello', 0);
                INSERT INTO chat_message VALUES ('m2', 's1', 'assistant', 'hi', 0);
                "#,
            ).unwrap();
        }
        repo.project_snapshot(&make_snapshot_meta(), &snap2, &normalizer)
            .unwrap();

        // R5：FastForward 应推进活跃投影——preview 返回新内容（含 m2 'hi'）
        let preview = repo
            .read_conversation_preview(&SessionIdentity::new("work_cn", "s1"))
            .expect("应有预览");
        let contents: Vec<&str> = preview
            .messages
            .iter()
            .map(|m| m.content_excerpt.as_str())
            .collect();
        assert!(
            contents.contains(&"hi"),
            "FastForward 后预览应包含新消息 'hi'，实际: {:?}",
            contents
        );
        assert_eq!(preview.messages.len(), 2, "FastForward 后应有 2 条消息");

        // R5：search 应能搜到新内容 'hi'
        let hits_hi = repo.search_messages("hi");
        assert!(!hits_hi.is_empty(), "FastForward 后应能搜到新内容 'hi'");
    }

    // ============== R6：软删除项目保留为证据但排除出 browse/search/count ==============

    /// R6 fixture：含一个正常项目 p1 和一个软删除项目 p2
    fn make_snapshot_fixture_with_soft_deleted_project(dir: &Path, owner: &str) {
        let db_path = dir.join("database.db");
        let conn = Connection::open(&db_path).unwrap();
        conn.execute_batch(
            r#"
            CREATE TABLE project (
                project_id TEXT PRIMARY KEY, user_id TEXT NOT NULL,
                biz_project_id TEXT NOT NULL, deleted_at INTEGER DEFAULT 0
            );
            CREATE TABLE chat_session (
                session_id TEXT PRIMARY KEY, project_id TEXT NOT NULL,
                deleted_at INTEGER DEFAULT 0
            );
            CREATE TABLE chat_message (
                message_id TEXT PRIMARY KEY, session_id TEXT NOT NULL,
                role TEXT, content TEXT, deleted_at INTEGER DEFAULT 0
            );
            "#,
        )
        .unwrap();
        // p1 正常，p2 软删除（deleted_at = 500）
        conn.execute("INSERT INTO project VALUES ('p1', ?1, 'biz-1', 0)", [owner])
            .unwrap();
        conn.execute_batch(
            "INSERT INTO project VALUES ('p2', 'user-B', 'biz-2', 500); \
             INSERT INTO chat_session VALUES ('s1', 'p1', 0); \
             INSERT INTO chat_session VALUES ('s2', 'p2', 0); \
             INSERT INTO chat_message VALUES ('m1', 's1', 'user', 'hello', 0); \
             INSERT INTO chat_message VALUES ('m2', 's2', 'user', 'soft-deleted-project-msg', 0);",
        )
        .unwrap();
        drop(conn);
    }

    #[test]
    fn r6_soft_deleted_project_retained_but_excluded_from_browse() {
        let dir = tempdir().unwrap();
        let snapshot_dir = dir.path().join("snap-1");
        std::fs::create_dir_all(&snapshot_dir).unwrap();
        make_snapshot_fixture_with_soft_deleted_project(&snapshot_dir, "user-A");

        let repo = setup_repo(dir.path());
        let normalizer = WorkCnSourceNormalizer::new(TEST_CATALOG_KEY.to_string());
        repo.project_snapshot(&make_snapshot_meta(), &snapshot_dir, &normalizer)
            .unwrap();

        // R6：browse 只返回 p1（未软删除），p2 被排除
        let browse = repo.browse();
        assert_eq!(browse.projects.len(), 1, "browse 应只返回 1 个未软删除项目");
        assert_eq!(browse.projects[0].project_id, "p1");

        // R6：summary 的 soft_deleted_project_count 应为 1
        assert_eq!(browse.summary.visible_project_count, 1);
        assert_eq!(browse.summary.soft_deleted_project_count, 1);

        // R6：账号树中 user-B 不应有可见项目（p2 软删除）
        let accounts_b = repo.browse_projects_by_account("user-B");
        assert!(accounts_b.is_empty(), "user-B 不应有可见项目（p2 软删除）");
    }

    #[test]
    fn r6_soft_deleted_project_in_diagnostic_retained_counts() {
        let dir = tempdir().unwrap();
        let snapshot_dir = dir.path().join("snap-1");
        std::fs::create_dir_all(&snapshot_dir).unwrap();
        make_snapshot_fixture_with_soft_deleted_project(&snapshot_dir, "user-A");

        let repo = setup_repo(dir.path());
        let normalizer = WorkCnSourceNormalizer::new(TEST_CATALOG_KEY.to_string());
        repo.project_snapshot(&make_snapshot_meta(), &snapshot_dir, &normalizer)
            .unwrap();

        // R6：诊断完整性——visible_projects 排除软删除，retained_projects 含全部
        let diag = repo.diagnostic_integrity();
        assert_eq!(diag.visible_projects, 1, "可见项目应为 1（排除软删除 p2）");
        assert_eq!(
            diag.retained_projects, 2,
            "保留项目应为 2（含软删除 p2 作为证据）"
        );

        // R6：read_project_observation 仍能读取软删除项目（证据保留）
        let obs = repo.read_project_observation("p2");
        assert!(obs.is_some(), "软删除项目 p2 的观察记录应保留");
        let obs = obs.unwrap();
        assert!(
            obs.project_identity.soft_deleted,
            "p2 应标记为 soft_deleted"
        );
    }

    #[test]
    fn r6_soft_deleted_project_deletion_transition_retained() {
        // R6：第一次扫描 p1 未删除，第二次扫描 p1 被软删除
        // 软删除变化应作为证据保留，而非丢弃实体
        let dir = tempdir().unwrap();
        let repo = setup_repo(dir.path());
        let normalizer = WorkCnSourceNormalizer::new(TEST_CATALOG_KEY.to_string());

        // 第一次扫描：p1 未删除
        let snap1 = dir.path().join("snap-1");
        std::fs::create_dir_all(&snap1).unwrap();
        {
            let conn = Connection::open(snap1.join("database.db")).unwrap();
            conn.execute_batch(
                r#"
                CREATE TABLE project (project_id TEXT PRIMARY KEY, user_id TEXT NOT NULL, biz_project_id TEXT NOT NULL, deleted_at INTEGER DEFAULT 0);
                CREATE TABLE chat_session (session_id TEXT PRIMARY KEY, project_id TEXT NOT NULL, deleted_at INTEGER DEFAULT 0);
                CREATE TABLE chat_message (message_id TEXT PRIMARY KEY, session_id TEXT NOT NULL, role TEXT, content TEXT, deleted_at INTEGER DEFAULT 0);
                INSERT INTO project VALUES ('p1', 'user-A', 'biz-1', 0);
                INSERT INTO chat_session VALUES ('s1', 'p1', 0);
                INSERT INTO chat_message VALUES ('m1', 's1', 'user', 'hello', 0);
                "#,
            ).unwrap();
        }
        repo.project_snapshot(&make_snapshot_meta(), &snap1, &normalizer)
            .unwrap();

        // 第一次后：p1 可见
        let diag1 = repo.diagnostic_integrity();
        assert_eq!(diag1.visible_projects, 1);
        assert_eq!(diag1.retained_projects, 1);

        // 第二次扫描：p1 被软删除（deleted_at = 999）
        let snap2 = dir.path().join("snap-2");
        std::fs::create_dir_all(&snap2).unwrap();
        {
            let conn = Connection::open(snap2.join("database.db")).unwrap();
            conn.execute_batch(
                r#"
                CREATE TABLE project (project_id TEXT PRIMARY KEY, user_id TEXT NOT NULL, biz_project_id TEXT NOT NULL, deleted_at INTEGER DEFAULT 0);
                CREATE TABLE chat_session (session_id TEXT PRIMARY KEY, project_id TEXT NOT NULL, deleted_at INTEGER DEFAULT 0);
                CREATE TABLE chat_message (message_id TEXT PRIMARY KEY, session_id TEXT NOT NULL, role TEXT, content TEXT, deleted_at INTEGER DEFAULT 0);
                INSERT INTO project VALUES ('p1', 'user-A', 'biz-1', 999);
                INSERT INTO chat_session VALUES ('s1', 'p1', 0);
                INSERT INTO chat_message VALUES ('m1', 's1', 'user', 'hello', 0);
                "#,
            ).unwrap();
        }
        repo.project_snapshot(&make_snapshot_meta(), &snap2, &normalizer)
            .unwrap();

        // R6：第二次后——p1 软删除，visible=0，retained=1（证据保留）
        let diag2 = repo.diagnostic_integrity();
        assert_eq!(diag2.visible_projects, 0, "p1 软删除后可见项目应为 0");
        assert_eq!(
            diag2.retained_projects, 1,
            "p1 软删除后保留项目应为 1（证据保留，不丢弃）"
        );

        // R6：browse 不再显示 p1
        let browse = repo.browse();
        assert!(browse.projects.is_empty(), "p1 软删除后 browse 应为空");

        // R6：read_project_observation 仍能读取 p1（含软删除标记 + owner 历史）
        let obs = repo.read_project_observation("p1").expect("p1 应保留");
        assert!(
            obs.project_identity.soft_deleted,
            "p1 应标记为 soft_deleted"
        );
        assert_eq!(
            obs.first_observed_owner, "user-A",
            "first_observed_owner 应保留"
        );
        // owner_observations 应有 2 条（两次扫描）
        assert_eq!(
            obs.owner_observations.len(),
            2,
            "应有 2 条 owner 观察（删除前后各一次）"
        );
    }
}
