//! P3-1 账号会话索引：跨账号只读汇总各实例 data_dir 的对话库。
//!
//! 依据 ADR-0020 阶段 A（主库原语）与 `TECHNICAL_BASELINE.md`：
//! - 每账号实例库位于 `{storage_root}\trae-instances\{profile_id}\ModularData\ai-agent\database.db`；
//! - 通过隔离三件套副本只读打开（复制 DB/WAL/SHM 到临时目录，复用 `sqlcipher`
//!   模块已验证的实现）：运行中的 TRAE 实例不受影响，已提交但未 checkpoint
//!   的 WAL 数据不丢失（实测实例 WAL 可达数百 KB）；
//! - raw key 打开（baseline key 由生产启动激活注入）；单账号打开或读取失败
//!   只标记该账号，不阻断其他账号。
//!
//! 列防御：`session_title` / `updated_at` / `deleted_at` 均按需探测，缺失时
//! 降级（标题空串、时间 None、不过滤软删），兼容 schema 演进。
//!
//! U-6 W3（2026-08-29，依据 `.scratch/history-u6/w0-report.md` 实测 0/105 撕裂）
//! 新增准实时增量链路：
//! - **变化检测**：只 stat 三件套 mtime/size 指纹（不复制不读取），间隔 5 秒；
//! - **增量读取**：检测到变化才复制三件套快照，按 `chat_session.updated_at`
//!   高水位（每账号持久化 last_max_updated_at）查询 delta 行并合并入缓存；
//! - **瞬态错误语义**：读取失败丢弃本轮（缓存与指纹不动），下一轮重试兜底。

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use rusqlite::Connection;
use serde::{Deserialize, Serialize};

use crate::atomic_publish::publish_replacing;
use crate::sqlcipher::open_with_key_readonly;
use crate::trae_instance::{instance_data_dir, profile_id_is_safe};

/// 单条会话摘要（P3-1 验收：标题、时间、消息数）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionSummaryEntry {
    pub session_id: String,
    /// 展示标题；源库 `session_title` 为空或列缺失时为空串（前端显示占位）。
    pub title: String,
    /// 未软删的消息数。
    pub message_count: u32,
    /// 最后更新时间（unix 秒）；源库毫秒时间戳归一化为秒，列缺失或空值为 None。
    pub updated_at_unix_seconds: Option<i64>,
    /// 会话软删除标记（`chat_session.deleted_at` 非 0）。
    pub deleted: bool,
}

/// 单账号索引读取结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AccountSessionIndexStatus {
    /// 读取成功（可能为空列表）。
    Ready(Vec<SessionSummaryEntry>),
    /// 实例从未启动过（无 data_dir 或无 database.db）——正常状态而非错误。
    NoInstanceData,
    /// 打开或读取失败（key 不匹配、文件损坏等）。
    ReadFailed,
}

/// 一个账号的会话索引条目。
///
/// `fingerprint`：读取完成后的三件套 stat 指纹（W3）——前端保存为下一轮
/// 变化检测的 previous 依据。读取失败（ReadFailed）时调用方不应推进指纹，
/// 使下一轮轮询继续把该账号判为"变化"从而重试（瞬态错误兜底语义）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountSessionIndexEntry {
    pub profile_id: String,
    pub display_name: String,
    pub status: AccountSessionIndexStatus,
    pub fingerprint: InstanceFingerprint,
}

/// 检测表列是否存在；缺失列触发读取降级而非整体失败。
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

/// 毫秒时间戳归一化为秒。
///
/// TRAE 库 `updated_at` 实测为毫秒；秒级值（< 2001-09-09 量级阈值以下）原样返回，
/// 保证两种单位下前端都能得到可比较的秒级时间。
fn normalize_timestamp_to_seconds(raw: i64) -> i64 {
    // 1e12 秒 ≈ 33017 年，超过它的 unix 秒值不存在；毫秒时间戳必然落在此之上。
    if raw > 1_000_000_000_000 {
        raw / 1000
    } else {
        raw
    }
}

/// 账号实例对话库路径：`{instance_dir}\ModularData\ai-agent\database.db`。
/// profile_id 不合法（路径穿越防御）时返回 None，调用方按无实例数据处理。
pub(crate) fn instance_database_path(storage_root: &Path, profile_id: &str) -> Option<PathBuf> {
    instance_data_dir(storage_root, profile_id)
        .ok()
        .map(|dir| dir.join("ModularData").join("ai-agent").join("database.db"))
}

/// 会话摘要查询（列防御 + 可选增量过滤），全量与增量读取共用。
///
/// `delta_boundary_raw` 为高水位换算到源库原始单位的下界（秒库直接用秒值，
/// 毫秒库为 `高水位秒 * 1000`）；`None` 表示全量读取。增量过滤是防御性超集
/// （宁多读不漏读）：
/// - `updated_at >= 下界`——高水位以上的新会话/更新会话；
/// - `updated_at IS NULL`——无时间行无法定位，每轮重读（幂等，行数极少）；
/// - `deleted_at != 0`——软删可能不推进 updated_at，每轮重读全部软删行。
fn query_session_summaries(
    conn: &Connection,
    delta_boundary_raw: Option<i64>,
) -> Result<Vec<SessionSummaryEntry>, ()> {
    // 列探测：schema 契约只保证 session_id/project_id 等核心列，
    // 展示类列（标题/时间/软删）按需降级。
    let has_title = column_exists(conn, "chat_session", "session_title");
    let has_updated_at = column_exists(conn, "chat_session", "updated_at");
    let has_session_deleted = column_exists(conn, "chat_session", "deleted_at");
    let has_message_deleted = column_exists(conn, "chat_message", "deleted_at");

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
    let message_count_expr = if has_message_deleted {
        "(SELECT COUNT(*) FROM chat_message m \
          WHERE m.session_id = s.session_id \
          AND (m.deleted_at IS NULL OR m.deleted_at = 0))"
    } else {
        "(SELECT COUNT(*) FROM chat_message m WHERE m.session_id = s.session_id)"
    };

    // 增量过滤子句：boundary 为本函数内部计算的 i64 字面量，无注入面。
    let where_clause = match (delta_boundary_raw, has_updated_at) {
        (Some(boundary), true) => {
            let mut clause =
                format!(" WHERE (s.updated_at >= {boundary} OR s.updated_at IS NULL");
            if has_session_deleted {
                clause.push_str(" OR COALESCE(s.deleted_at, 0) != 0");
            }
            clause.push(')');
            clause
        }
        // 无 updated_at 列时无法增量定位：退化为全量（where 为空）。
        _ => String::new(),
    };

    let sql = format!(
        "SELECT s.session_id, {title_expr}, {message_count_expr}, {updated_expr}, {deleted_expr} \
         FROM chat_session s{where_clause} \
         ORDER BY {updated_expr} DESC NULLS LAST, s.session_id ASC"
    );

    let mut statement = conn.prepare(&sql).map_err(|_| ())?;
    let rows = statement
        .query_map([], |row| {
            let session_id: String = row.get(0)?;
            let title: String = row.get(1)?;
            let message_count: i64 = row.get(2)?;
            let updated_at_raw: Option<i64> = row.get(3)?;
            let deleted_at: i64 = row.get(4)?;
            Ok(SessionSummaryEntry {
                session_id,
                title,
                message_count: message_count.max(0) as u32,
                updated_at_unix_seconds: updated_at_raw.map(normalize_timestamp_to_seconds),
                deleted: deleted_at != 0,
            })
        })
        .map_err(|_| ())?;
    rows.collect::<Result<Vec<_>, _>>().map_err(|_| ())
}

/// 全量读取单个实例库的会话摘要列表。
///
/// 只读打开隔离副本；任何失败（文件无法复制、key 不匹配、表缺失）统一映射为
/// `Err(())`，由调用方聚合为 `ReadFailed`——错误细节不透传，保持索引读取的
/// 批量语义（单账号失败不影响整体）。
fn read_instance_sessions(
    db_path: &Path,
    raw_key: &str,
) -> Result<Vec<SessionSummaryEntry>, ()> {
    let conn = open_with_key_readonly(db_path, raw_key).map_err(|_| ())?;
    query_session_summaries(&conn, None)
}

/// 增量读取：只取高水位之后可能变化的会话行（delta）。
///
/// 单位检测：以库内 `MAX(updated_at)` 原始值判定源库单位（> 1e12 为毫秒库），
/// 高水位（归一化秒）换算为原始单位下界——W0 实测 TRAE 库为毫秒，检测保证
/// 两种单位下边界都正确。高水位为 None（首次/缓存缺失）时退化为全量读取。
fn read_instance_sessions_delta(
    db_path: &Path,
    raw_key: &str,
    high_water_seconds: Option<i64>,
) -> Result<Vec<SessionSummaryEntry>, ()> {
    let Some(high_water) = high_water_seconds else {
        return read_instance_sessions(db_path, raw_key);
    };
    let conn = open_with_key_readonly(db_path, raw_key).map_err(|_| ())?;
    if !column_exists(&conn, "chat_session", "updated_at") {
        // 无时间列无法增量定位：退化为全量。
        return query_session_summaries(&conn, None);
    }
    // 空表（或全 NULL 时间列）无增量可言。
    let max_raw: Option<i64> = conn
        .query_row("SELECT MAX(updated_at) FROM chat_session", [], |row| {
            row.get(0)
        })
        .map_err(|_| ())?;
    let Some(max_raw) = max_raw else {
        return Ok(Vec::new());
    };
    let boundary_raw = if max_raw > 1_000_000_000_000 {
        high_water.saturating_mul(1000)
    } else {
        high_water
    };
    query_session_summaries(&conn, Some(boundary_raw))
}

/// 批量读取全部账号的会话索引（全量，用户显式加载/刷新入口）。
///
/// 逐账号独立判定：无实例目录 → `NoInstanceData`；读取失败 → `ReadFailed`；
/// 成功 → `Ready(sessions)`。任一账号的失败不影响其他账号的读取结果。
///
/// U-6 W3：全量读取即真值——每账号成功读取后整体重建持久化缓存（顺带修复
/// 历史漂移）并重算高水位；缓存写入失败不影响返回结果（缓存只是优化）。
pub fn read_account_session_index(
    storage_root: &Path,
    accounts: &[(String, String)],
    raw_key: &str,
) -> Vec<AccountSessionIndexEntry> {
    let cache_store = SessionIndexCacheStore::new(storage_root);
    accounts
        .iter()
        .map(|(profile_id, display_name)| {
            let status = match instance_database_path(storage_root, profile_id) {
                Some(db_path) if db_path.exists() => {
                    match read_instance_sessions(&db_path, raw_key) {
                        Ok(sessions) => {
                            let index = CachedSessionIndex {
                                format_version: CACHE_FORMAT_VERSION,
                                last_max_updated_at: next_high_water_mark(&sessions),
                                sessions: sessions.clone(),
                            };
                            let _ = cache_store.save(profile_id, &index);
                            AccountSessionIndexStatus::Ready(sessions)
                        }
                        // 瞬态失败：缓存不动，下轮轮询重试兜底。
                        Err(()) => AccountSessionIndexStatus::ReadFailed,
                    }
                }
                _ => AccountSessionIndexStatus::NoInstanceData,
            };
            // 指纹在读取完成后 stat：覆盖复制期间新落的写入（下轮轮询可检测）。
            AccountSessionIndexEntry {
                profile_id: profile_id.clone(),
                display_name: display_name.clone(),
                status,
                fingerprint: stat_instance_fingerprint(storage_root, profile_id),
            }
        })
        .collect()
}

// ============================================================================
// U-6 W3：变化检测（三件套 stat 指纹）
// ============================================================================

/// 单文件 stat 指纹：mtime（秒+纳秒）+ size。文件不存在由外层 `Option` 表达。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileStatFingerprint {
    pub mtime_secs: u64,
    pub mtime_nanos: u32,
    pub size: u64,
}

/// 账号实例库三件套指纹（db + wal + shm）；组件 None = 对应文件不存在。
///
/// W0 实测：TRAE 一次对话写入必使 db 或 wal 的 mtime/size 至少其一变化
/// （8/8 事务写入全部被捕获），指纹比对作为"变化才复制"的廉价预检。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstanceFingerprint {
    pub db: Option<FileStatFingerprint>,
    pub wal: Option<FileStatFingerprint>,
    pub shm: Option<FileStatFingerprint>,
}

/// stat 单文件；不存在或元数据不可读 → None（mtime 缺失退化为 0，size 仍可比）。
pub(crate) fn stat_file_fingerprint(path: &Path) -> Option<FileStatFingerprint> {
    let metadata = std::fs::metadata(path).ok()?;
    let (mtime_secs, mtime_nanos) = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|duration| (duration.as_secs(), duration.subsec_nanos()))
        .unwrap_or((0, 0));
    Some(FileStatFingerprint {
        mtime_secs,
        mtime_nanos,
        size: metadata.len(),
    })
}

/// stat 账号实例库三件套指纹。profile_id 不合法或 db 不存在时组件为 None。
pub fn stat_instance_fingerprint(storage_root: &Path, profile_id: &str) -> InstanceFingerprint {
    let none = InstanceFingerprint {
        db: None,
        wal: None,
        shm: None,
    };
    let Some(db_path) = instance_database_path(storage_root, profile_id) else {
        return none;
    };
    InstanceFingerprint {
        db: stat_file_fingerprint(&db_path),
        wal: stat_file_fingerprint(&PathBuf::from(format!("{}-wal", db_path.display()))),
        shm: stat_file_fingerprint(&PathBuf::from(format!("{}-shm", db_path.display()))),
    }
}

/// 指纹比对：任一组件（含存在性）变化即为变化。
///
/// `previous = None` 视为首次观测：有 db 文件才算变化（无实例数据无需读取）。
pub fn instance_fingerprint_changed(
    current: &InstanceFingerprint,
    previous: Option<&InstanceFingerprint>,
) -> bool {
    match previous {
        None => current.db.is_some(),
        Some(previous) => current != previous,
    }
}

/// 批量变化检测结果：全部账号最新指纹 + 发生变化的 profile_id 列表。
pub struct AccountSessionChangeDetection {
    pub fingerprints: Vec<(String, InstanceFingerprint)>,
    pub changed: Vec<String>,
}

/// 批量 stat 并与上次指纹比对（只 stat，不复制不读取——W0 裁定的廉价预检）。
///
/// previous 中未登记的账号视为首次观测：有 db 才报告变化；registry 之外的
/// 输入条目被忽略（以账号注册表为准）。
pub fn detect_account_session_changes(
    storage_root: &Path,
    accounts: &[(String, String)],
    previous: &HashMap<String, InstanceFingerprint>,
) -> AccountSessionChangeDetection {
    let mut fingerprints = Vec::with_capacity(accounts.len());
    let mut changed = Vec::new();
    for (profile_id, _) in accounts {
        let current = stat_instance_fingerprint(storage_root, profile_id);
        if instance_fingerprint_changed(&current, previous.get(profile_id)) {
            changed.push(profile_id.clone());
        }
        fingerprints.push((profile_id.clone(), current));
    }
    AccountSessionChangeDetection {
        fingerprints,
        changed,
    }
}

// ============================================================================
// U-6 W3：高水位缓存持久化 + 增量合并
// ============================================================================

/// 缓存文件格式版本；不匹配按损坏降级（全量重建）。
const CACHE_FORMAT_VERSION: u32 = 1;
/// 缓存文件大小上限（会话标题等展示元数据，超出按损坏降级防异常膨胀）。
const MAX_CACHE_BYTES: u64 = 16 * 1024 * 1024;
/// 缓存目录名（storage_root 下）：`{storage_root}/session-index/{profile_id}.json`。
const CACHE_DIR: &str = "session-index";

/// 每账号持久化索引缓存：合并后的全量会话列表 + 高水位。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CachedSessionIndex {
    pub format_version: u32,
    pub sessions: Vec<SessionSummaryEntry>,
    /// 高水位：已观测到的最大 `updated_at`（unix 秒，归一化后）。
    pub last_max_updated_at: Option<i64>,
}

/// 高水位缓存存储：`{storage_root}/session-index/{profile_id}.json`。
///
/// 缺失/损坏/超限统一按 None 降级（全量重建），不删除文件（铁律：不删证据）。
pub struct SessionIndexCacheStore {
    dir: PathBuf,
}

impl SessionIndexCacheStore {
    pub fn new(storage_root: &Path) -> Self {
        Self {
            dir: storage_root.join(CACHE_DIR),
        }
    }

    fn cache_path(&self, profile_id: &str) -> Option<PathBuf> {
        profile_id_is_safe(profile_id).then(|| self.dir.join(format!("{profile_id}.json")))
    }

    /// 读取缓存；缺失/损坏/超限/profile_id 非法 → None（调用方按全量重建）。
    pub fn load(&self, profile_id: &str) -> Option<CachedSessionIndex> {
        let path = self.cache_path(profile_id)?;
        let metadata = std::fs::symlink_metadata(&path).ok()?;
        if !metadata.is_file() || metadata.len() > MAX_CACHE_BYTES {
            return None;
        }
        let content = std::fs::read_to_string(&path).ok()?;
        let cache: CachedSessionIndex = serde_json::from_str(&content).ok()?;
        (cache.format_version == CACHE_FORMAT_VERSION).then_some(cache)
    }

    /// 原子写入缓存（临时文件 + 替换发布）；失败返回 Err（调用方忽略即可）。
    pub fn save(&self, profile_id: &str, index: &CachedSessionIndex) -> Result<(), ()> {
        let path = self.cache_path(profile_id).ok_or(())?;
        let content = serde_json::to_string(index).map_err(|_| ())?;
        std::fs::create_dir_all(&self.dir).map_err(|_| ())?;
        let temporary = path.with_extension("json.tmp");
        std::fs::write(&temporary, content.as_bytes()).map_err(|_| ())?;
        publish_replacing(&temporary, &path).map_err(|_| ())
    }

    /// 删除该账号的缓存文件（U-6 W4 彻底删除记录命令专用：用户显式发起的
    /// 破坏性动作，与 load 的"不删证据"降级口径无关）。
    /// 文件不存在时静默成功（幂等）；profile_id 非法拒绝。
    pub fn remove(&self, profile_id: &str) -> Result<(), ()> {
        let path = self.cache_path(profile_id).ok_or(())?;
        match std::fs::remove_file(&path) {
            Ok(()) => Ok(()),
            // 缺失视为成功：purge 幂等，缓存本就不存在时无需报错。
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(_) => Err(()),
        }
    }
}

/// 高水位增量合并：delta 行按 session_id upsert 进缓存列表。
///
/// 三场景（U-6 W3 验收）：
/// - 新会话：插入；
/// - 更新会话：整体替换（标题/消息数/时间取新值）；
/// - 软删会话：替换为 `deleted = true` 的行（标记而非移除，与全量读取口径一致）。
/// 合并后按 updated_at 秒级降序（None 最后）、session_id 升序重排——
/// 与全量读取的 SQL 排序语义保持一致。
pub fn merge_session_delta(
    cached: Vec<SessionSummaryEntry>,
    delta: Vec<SessionSummaryEntry>,
) -> Vec<SessionSummaryEntry> {
    let mut merged = cached;
    for entry in delta {
        if let Some(existing) = merged
            .iter_mut()
            .find(|candidate| candidate.session_id == entry.session_id)
        {
            *existing = entry;
        } else {
            merged.push(entry);
        }
    }
    merged.sort_by(|a, b| {
        b.updated_at_unix_seconds
            .cmp(&a.updated_at_unix_seconds)
            .then_with(|| a.session_id.cmp(&b.session_id))
    });
    merged
}

/// 计算合并后高水位：全部会话（含软删）`updated_at` 的最大值（秒）。
///
/// 软删行计入：其时间戳也是已观测上界，排除反而可能回退高水位。
pub fn next_high_water_mark(sessions: &[SessionSummaryEntry]) -> Option<i64> {
    sessions
        .iter()
        .filter_map(|session| session.updated_at_unix_seconds)
        .max()
}

/// 单账号增量读取入口（变化账号触发，三件套快照 + 高水位合并 + 缓存回写）。
///
/// 语义：
/// - 无 db → `NoInstanceData`（缓存不动）；
/// - 快照读取失败 → `ReadFailed`（瞬态：缓存不动，由调用方保旧指纹下轮重试）；
/// - 成功 → delta 合并（缓存缺失时 delta 即全量）、缓存回写、返回合并后全量列表。
pub fn read_account_session_index_incremental(
    storage_root: &Path,
    profile_id: &str,
    display_name: &str,
    raw_key: &str,
) -> AccountSessionIndexEntry {
    let status = match instance_database_path(storage_root, profile_id) {
        Some(db_path) if db_path.exists() => {
            let cache_store = SessionIndexCacheStore::new(storage_root);
            let cached = cache_store.load(profile_id);
            let high_water = cached.as_ref().and_then(|cache| cache.last_max_updated_at);
            match read_instance_sessions_delta(&db_path, raw_key, high_water) {
                Ok(delta) => {
                    let sessions = match cached {
                        Some(cache) => merge_session_delta(cache.sessions, delta),
                        None => delta,
                    };
                    let index = CachedSessionIndex {
                        format_version: CACHE_FORMAT_VERSION,
                        last_max_updated_at: next_high_water_mark(&sessions),
                        sessions: sessions.clone(),
                    };
                    // 缓存回写失败不影响读取结果（缓存只是优化，下轮全量兜底）。
                    let _ = cache_store.save(profile_id, &index);
                    AccountSessionIndexStatus::Ready(sessions)
                }
                Err(()) => AccountSessionIndexStatus::ReadFailed,
            }
        }
        _ => AccountSessionIndexStatus::NoInstanceData,
    };
    // 与全量读取同口径：读取完成后 stat，供前端登记为下一轮 previous。
    AccountSessionIndexEntry {
        profile_id: profile_id.to_string(),
        display_name: display_name.to_string(),
        status,
        fingerprint: stat_instance_fingerprint(storage_root, profile_id),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::OpenFlags;

    /// 在指定路径创建明文 fixture 库（表结构对齐 Work CN 契约）。
    fn create_plain_fixture_db(path: &Path) {
        let conn = Connection::open(path).unwrap();
        conn.execute_batch(
            "CREATE TABLE project (project_id TEXT PRIMARY KEY, user_id TEXT, biz_project_id TEXT, deleted_at INTEGER);
             CREATE TABLE chat_session (session_id TEXT PRIMARY KEY, project_id TEXT, session_title TEXT, updated_at INTEGER, deleted_at INTEGER);
             CREATE TABLE chat_message (message_id TEXT PRIMARY KEY, session_id TEXT, deleted_at INTEGER);",
        )
        .unwrap();
        // 毫秒时间戳（实测 TRAE 库单位）。
        conn.execute(
            "INSERT INTO chat_session (session_id, project_id, session_title, updated_at, deleted_at) \
             VALUES ('s1', 'p1', '会话一', 1770000000000, NULL)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO chat_session (session_id, project_id, session_title, updated_at, deleted_at) \
             VALUES ('s2', 'p1', '会话二', 1769000000000, 500)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO chat_message (message_id, session_id, deleted_at) VALUES ('m1', 's1', NULL)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO chat_message (message_id, session_id, deleted_at) VALUES ('m2', 's1', 0)",
            [],
        )
        .unwrap();
        // 软删消息不计入 message_count。
        conn.execute(
            "INSERT INTO chat_message (message_id, session_id, deleted_at) VALUES ('m3', 's1', 123)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO chat_message (message_id, session_id, deleted_at) VALUES ('m4', 's2', NULL)",
            [],
        )
        .unwrap();
    }

    /// 明文库读取走 raw key 失败回退？——不：`open_with_key_readonly` 只用 key 打开，
    /// 明文 fixture 在测试中以“key 打开失败”路径覆盖 ReadFailed 语义；
    /// 成功路径使用加密库。
    #[test]
    fn reads_sessions_from_encrypted_instance_db() {
        let temp = std::env::temp_dir().join(format!(
            "trae-sync-p31-read-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(
            temp.join("trae-instances")
                .join("checkin-a")
                .join("ModularData")
                .join("ai-agent"),
        )
        .unwrap();
        let db_path = temp
            .join("trae-instances")
            .join("checkin-a")
            .join("ModularData")
            .join("ai-agent")
            .join("database.db");
        create_encrypted_fixture_db_with_data(&db_path, &"aa".repeat(32));
        let entries = read_account_session_index(
            &temp,
            &[("checkin-a".to_string(), "账号甲".to_string())],
            &"aa".repeat(32),
        );
        assert_eq!(entries.len(), 1);
        match &entries[0].status {
            AccountSessionIndexStatus::Ready(sessions) => {
                assert_eq!(sessions.len(), 1);
                assert_eq!(sessions[0].session_id, "s1");
                assert_eq!(sessions[0].title, "加密库会话");
                assert_eq!(sessions[0].message_count, 2);
                assert_eq!(
                    sessions[0].updated_at_unix_seconds,
                    Some(1770000000)
                );
            }
            other => panic!("期望 Ready，实际 {:?}", other),
        }
        let _ = std::fs::remove_dir_all(&temp);
    }

    /// 加密库构造（带数据）：独立 helper，避免与明文 fixture 混淆。
    fn create_encrypted_fixture_db_with_data(path: &Path, raw_key: &str) {
        let flags = OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE;
        let conn = Connection::open_with_flags(path, flags).unwrap();
        let pragma = format!("PRAGMA key = \"x'{}'\";", raw_key);
        conn.execute_batch(&pragma).unwrap();
        conn.execute_batch(
            "CREATE TABLE project (project_id TEXT PRIMARY KEY, user_id TEXT, biz_project_id TEXT, deleted_at INTEGER);
             CREATE TABLE chat_session (session_id TEXT PRIMARY KEY, project_id TEXT, session_title TEXT, updated_at INTEGER, deleted_at INTEGER);
             CREATE TABLE chat_message (message_id TEXT PRIMARY KEY, session_id TEXT, deleted_at INTEGER);
             INSERT INTO chat_session (session_id, project_id, session_title, updated_at, deleted_at) VALUES ('s1', 'p1', '加密库会话', 1770000000000, NULL);
             INSERT INTO chat_message (message_id, session_id, deleted_at) VALUES ('m1', 's1', NULL);
             INSERT INTO chat_message (message_id, session_id, deleted_at) VALUES ('m2', 's1', NULL);
             INSERT INTO chat_message (message_id, session_id, deleted_at) VALUES ('m3', 's1', 99);",
        )
        .unwrap();
    }

    #[test]
    fn missing_instance_dir_reports_no_instance_data() {
        let temp = std::env::temp_dir().join(format!(
            "trae-sync-p31-missing-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&temp).unwrap();
        let entries = read_account_session_index(
            &temp,
            &[("checkin-b".to_string(), "账号乙".to_string())],
            &"bb".repeat(32),
        );
        assert_eq!(entries[0].status, AccountSessionIndexStatus::NoInstanceData);
        let _ = std::fs::remove_dir_all(&temp);
    }

    #[test]
    fn wrong_key_reports_read_failed_without_blocking_others() {
        let temp = std::env::temp_dir().join(format!(
            "trae-sync-p31-wrongkey-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let good_key = "cc".repeat(32);
        let dir_a = temp
            .join("trae-instances")
            .join("checkin-a")
            .join("ModularData")
            .join("ai-agent");
        std::fs::create_dir_all(&dir_a).unwrap();
        create_encrypted_fixture_db_with_data(&dir_a.join("database.db"), &good_key);

        // 账号甲用错误 key 读取 → ReadFailed；账号乙无实例 → NoInstanceData；
        // 两者互不影响。
        let entries = read_account_session_index(
            &temp,
            &[
                ("checkin-a".to_string(), "账号甲".to_string()),
                ("checkin-b".to_string(), "账号乙".to_string()),
            ],
            &"dd".repeat(32),
        );
        assert_eq!(entries[0].status, AccountSessionIndexStatus::ReadFailed);
        assert_eq!(entries[1].status, AccountSessionIndexStatus::NoInstanceData);
        let _ = std::fs::remove_dir_all(&temp);
    }

    #[test]
    fn plain_fixture_db_reports_read_failed() {
        // 场景：明文 fixture 库（如测试环境导出副本）+ 任意 key。
        // open_with_key_readonly 对明文库 key 打开失败——当前实现无明文回退，
        // 该测试固化此行为：明文库 → ReadFailed（生产实例库恒为加密，不为明文
        // 库增加回退路径，避免绕过 key 校验读取未知来源数据）。
        let temp = std::env::temp_dir().join(format!(
            "trae-sync-p31-plain-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let dir = temp
            .join("trae-instances")
            .join("checkin-a")
            .join("ModularData")
            .join("ai-agent");
        std::fs::create_dir_all(&dir).unwrap();
        create_plain_fixture_db(&dir.join("database.db"));
        let entries = read_account_session_index(
            &temp,
            &[("checkin-a".to_string(), "账号甲".to_string())],
            &"ee".repeat(32),
        );
        assert_eq!(entries[0].status, AccountSessionIndexStatus::ReadFailed);
        let _ = std::fs::remove_dir_all(&temp);
    }

    #[test]
    fn timestamp_normalization_handles_both_units() {
        assert_eq!(normalize_timestamp_to_seconds(1770000000000), 1770000000);
        assert_eq!(normalize_timestamp_to_seconds(1770000000), 1770000000);
        assert_eq!(normalize_timestamp_to_seconds(0), 0);
    }

    #[test]
    fn unsafe_profile_id_treated_as_no_instance_data() {
        let temp = std::env::temp_dir().join(format!(
            "trae-sync-p31-unsafe-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&temp).unwrap();
        let entries = read_account_session_index(
            &temp,
            &[("../escape".to_string(), "非法账号".to_string())],
            &"ff".repeat(32),
        );
        assert_eq!(
            entries[0].status,
            AccountSessionIndexStatus::NoInstanceData
        );
        let _ = std::fs::remove_dir_all(&temp);
    }

    // ========================================================================
    // U-6 W3：变化检测 / 增量合并 / 高水位缓存
    // ========================================================================

    /// 构造唯一临时目录（W3 测试共用）。
    fn w3_temp_dir(label: &str) -> PathBuf {
        let temp = std::env::temp_dir().join(format!(
            "trae-sync-w3-{}-{}-{}",
            label,
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&temp).unwrap();
        temp
    }

    /// 会话行 fixture：(session_id, title, updated_at 原始值, deleted_at)。
    type SessionFixture = (&'static str, &'static str, i64, i64);

    /// 创建加密实例库（表结构对齐 Work CN 契约；时间戳单位由调用方给定）。
    fn create_encrypted_db_with_sessions(
        db_path: &Path,
        raw_key: &str,
        sessions: &[SessionFixture],
    ) {
        let flags = OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE;
        let conn = Connection::open_with_flags(db_path, flags).unwrap();
        let pragma = format!("PRAGMA key = \"x'{}'\";", raw_key);
        conn.execute_batch(&pragma).unwrap();
        conn.execute_batch(
            "CREATE TABLE project (project_id TEXT PRIMARY KEY, user_id TEXT, biz_project_id TEXT, deleted_at INTEGER);
             CREATE TABLE chat_session (session_id TEXT PRIMARY KEY, project_id TEXT, session_title TEXT, updated_at INTEGER, deleted_at INTEGER);
             CREATE TABLE chat_message (message_id TEXT PRIMARY KEY, session_id TEXT, deleted_at INTEGER);",
        )
        .unwrap();
        for (session_id, title, updated_at, deleted_at) in sessions {
            conn.execute(
                "INSERT INTO chat_session (session_id, project_id, session_title, updated_at, deleted_at) \
                 VALUES (?1, 'p1', ?2, ?3, ?4)",
                rusqlite::params![session_id, title, updated_at, deleted_at],
            )
            .unwrap();
        }
        conn.execute(
            "INSERT INTO chat_message (message_id, session_id, deleted_at) VALUES ('m1', 's1', NULL)",
            [],
        )
        .unwrap();
    }

    /// 以 key 打开已存在的加密库并执行写语句（增量测试造变化用）。
    fn mutate_encrypted_db(db_path: &Path, raw_key: &str, sql: &str) {
        let flags = OpenFlags::SQLITE_OPEN_READ_WRITE;
        let conn = Connection::open_with_flags(db_path, flags).unwrap();
        let pragma = format!("PRAGMA key = \"x'{}'\";", raw_key);
        conn.execute_batch(&pragma).unwrap();
        conn.execute_batch(sql).unwrap();
    }

    /// 实例库路径（在 temp 下按 profile_id 建标准目录结构）。
    fn instance_db_path(temp: &Path, profile_id: &str) -> PathBuf {
        let dir = temp
            .join("trae-instances")
            .join(profile_id)
            .join("ModularData")
            .join("ai-agent");
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("database.db")
    }

    fn ready_sessions(entry: &AccountSessionIndexEntry) -> &[SessionSummaryEntry] {
        match &entry.status {
            AccountSessionIndexStatus::Ready(sessions) => sessions,
            other => panic!("期望 Ready，实际 {:?}", other),
        }
    }

    #[test]
    fn fingerprint_change_detection_semantics() {
        // 首次观测：有 db → 变化（触发读取）；无 db → 不变化。
        let with_db = InstanceFingerprint {
            db: Some(FileStatFingerprint {
                mtime_secs: 1,
                mtime_nanos: 0,
                size: 10,
            }),
            wal: None,
            shm: None,
        };
        let no_db = InstanceFingerprint {
            db: None,
            wal: None,
            shm: None,
        };
        assert!(instance_fingerprint_changed(&with_db, None));
        assert!(!instance_fingerprint_changed(&no_db, None));

        // 完全一致 → 不变化。
        assert!(!instance_fingerprint_changed(&with_db, Some(&with_db)));

        // size 变化 → 变化（W0：TRAE 写入必动 db/wal 的 mtime 或 size）。
        let grown = InstanceFingerprint {
            db: Some(FileStatFingerprint {
                mtime_secs: 1,
                mtime_nanos: 0,
                size: 11,
            }),
            wal: None,
            shm: None,
        };
        assert!(instance_fingerprint_changed(&grown, Some(&with_db)));

        // mtime 纳秒级变化 → 变化。
        let touched = InstanceFingerprint {
            db: Some(FileStatFingerprint {
                mtime_secs: 1,
                mtime_nanos: 1,
                size: 10,
            }),
            wal: None,
            shm: None,
        };
        assert!(instance_fingerprint_changed(&touched, Some(&with_db)));

        // 存在性变化（db 出现/消失）→ 变化。
        assert!(instance_fingerprint_changed(&with_db, Some(&no_db)));
        assert!(instance_fingerprint_changed(&no_db, Some(&with_db)));

        // wal 出现 → 变化（组件级比对）。
        let with_wal = InstanceFingerprint {
            db: with_db.db,
            wal: Some(FileStatFingerprint {
                mtime_secs: 1,
                mtime_nanos: 0,
                size: 5,
            }),
            shm: None,
        };
        assert!(instance_fingerprint_changed(&with_wal, Some(&with_db)));
    }

    #[test]
    fn detect_changes_aggregates_per_account_and_ignores_stale_previous() {
        let temp = w3_temp_dir("detect");
        let key = "aa".repeat(32);
        // checkin-a 有库；checkin-b 无库。
        create_encrypted_db_with_sessions(
            &instance_db_path(&temp, "checkin-a"),
            &key,
            &[("s1", "会话一", 1770000000000, 0)],
        );

        let accounts = vec![
            ("checkin-a".to_string(), "账号甲".to_string()),
            ("checkin-b".to_string(), "账号乙".to_string()),
        ];
        // 首轮：a 有 db → 变化；b 无 db → 不变化；previous 中的 ghost 被忽略。
        let mut previous = HashMap::new();
        previous.insert(
            "ghost".to_string(),
            InstanceFingerprint {
                db: None,
                wal: None,
                shm: None,
            },
        );
        let first = detect_account_session_changes(&temp, &accounts, &previous);
        assert_eq!(first.changed, vec!["checkin-a".to_string()]);
        assert_eq!(first.fingerprints.len(), 2);
        assert!(first.fingerprints[0].1.db.is_some());
        assert_eq!(first.fingerprints[1].1.db, None);

        // 第二轮（无写入）：以第一轮指纹为 previous → 无变化。
        let previous: HashMap<String, InstanceFingerprint> =
            first.fingerprints.into_iter().collect();
        let second = detect_account_session_changes(&temp, &accounts, &previous);
        assert!(second.changed.is_empty());

        // 写入后会话库变化 → 再报告变化。
        mutate_encrypted_db(
            &instance_db_path(&temp, "checkin-a"),
            &key,
            "INSERT INTO chat_session (session_id, project_id, session_title, updated_at, deleted_at) \
             VALUES ('s2', 'p1', '会话二', 1771000000000, 0)",
        );
        let previous: HashMap<String, InstanceFingerprint> =
            second.fingerprints.into_iter().collect();
        let third = detect_account_session_changes(&temp, &accounts, &previous);
        assert_eq!(third.changed, vec!["checkin-a".to_string()]);
        let _ = std::fs::remove_dir_all(&temp);
    }

    #[test]
    fn merge_session_delta_covers_new_updated_and_soft_deleted() {
        let cached = vec![
            SessionSummaryEntry {
                session_id: "s1".to_string(),
                title: "旧标题一".to_string(),
                message_count: 1,
                updated_at_unix_seconds: Some(1000),
                deleted: false,
            },
            SessionSummaryEntry {
                session_id: "s2".to_string(),
                title: "标题二".to_string(),
                message_count: 2,
                updated_at_unix_seconds: Some(2000),
                deleted: false,
            },
        ];
        let delta = vec![
            // 更新会话：标题/消息数整体替换。
            SessionSummaryEntry {
                session_id: "s1".to_string(),
                title: "新标题一".to_string(),
                message_count: 5,
                updated_at_unix_seconds: Some(3000),
                deleted: false,
            },
            // 软删会话：标记而非移除（与全量读取口径一致）。
            SessionSummaryEntry {
                session_id: "s2".to_string(),
                title: "标题二".to_string(),
                message_count: 2,
                updated_at_unix_seconds: Some(2500),
                deleted: true,
            },
            // 新会话：插入。
            SessionSummaryEntry {
                session_id: "s3".to_string(),
                title: "标题三".to_string(),
                message_count: 0,
                updated_at_unix_seconds: Some(4000),
                deleted: false,
            },
        ];
        let merged = merge_session_delta(cached, delta);
        // 排序语义与全量读取一致：updated_at 秒级降序、session_id 升序兜底。
        let ids: Vec<(&str, bool)> = merged
            .iter()
            .map(|entry| (entry.session_id.as_str(), entry.deleted))
            .collect();
        assert_eq!(
            ids,
            vec![("s3", false), ("s1", false), ("s2", true)]
        );
        let s1 = merged.iter().find(|e| e.session_id == "s1").unwrap();
        assert_eq!((s1.title.as_str(), s1.message_count), ("新标题一", 5));
    }

    #[test]
    fn next_high_water_mark_includes_soft_deleted_and_handles_empty() {
        assert_eq!(next_high_water_mark(&[]), None);
        let sessions = vec![
            SessionSummaryEntry {
                session_id: "s1".to_string(),
                title: String::new(),
                message_count: 0,
                updated_at_unix_seconds: Some(100),
                deleted: false,
            },
            // 软删行计入高水位：其时间戳也是已观测上界。
            SessionSummaryEntry {
                session_id: "s2".to_string(),
                title: String::new(),
                message_count: 0,
                updated_at_unix_seconds: Some(500),
                deleted: true,
            },
            // 无时间行不参与。
            SessionSummaryEntry {
                session_id: "s3".to_string(),
                title: String::new(),
                message_count: 0,
                updated_at_unix_seconds: None,
                deleted: false,
            },
        ];
        assert_eq!(next_high_water_mark(&sessions), Some(500));
    }

    #[test]
    fn cache_store_roundtrip_and_degradation() {
        let temp = w3_temp_dir("cache");
        let store = SessionIndexCacheStore::new(&temp);
        // 缺失 → None（调用方按全量重建）。
        assert!(store.load("checkin-a").is_none());

        let index = CachedSessionIndex {
            format_version: CACHE_FORMAT_VERSION,
            last_max_updated_at: Some(1770000000),
            sessions: vec![SessionSummaryEntry {
                session_id: "s1".to_string(),
                title: "会话一".to_string(),
                message_count: 3,
                updated_at_unix_seconds: Some(1770000000),
                deleted: false,
            }],
        };
        store.save("checkin-a", &index).unwrap();
        // 保存路径：{storage_root}/session-index/{profile_id}.json。
        assert!(temp.join("session-index").join("checkin-a.json").is_file());
        assert_eq!(store.load("checkin-a").as_ref(), Some(&index));

        // 损坏内容 → None 降级（不删除文件——铁律：不删证据）。
        std::fs::write(
            temp.join("session-index").join("checkin-a.json"),
            "not-json",
        )
        .unwrap();
        assert!(store.load("checkin-a").is_none());

        // 版本不匹配 → None 降级。
        let stale = CachedSessionIndex {
            format_version: CACHE_FORMAT_VERSION + 1,
            last_max_updated_at: None,
            sessions: Vec::new(),
        };
        store.save("checkin-b", &stale).unwrap();
        assert!(store.load("checkin-b").is_none());

        // 非法 profile_id → 拒绝读写（路径穿越防御）。
        assert!(store.load("../escape").is_none());
        assert!(store.save("../escape", &index).is_err());
        let _ = std::fs::remove_dir_all(&temp);
    }

    #[test]
    fn incremental_read_merges_delta_and_advances_high_water() {
        let temp = w3_temp_dir("incremental");
        let key = "aa".repeat(32);
        let db_path = instance_db_path(&temp, "checkin-a");
        create_encrypted_db_with_sessions(
            &db_path,
            &key,
            &[
                ("s1", "会话一", 1770000000000, 0),
                ("s2", "会话二", 1769000000000, 0),
            ],
        );

        // 首次增量读取（无缓存）：退化为全量，写缓存 + 高水位。
        let first = read_account_session_index_incremental(&temp, "checkin-a", "账号甲", &key);
        assert_eq!(ready_sessions(&first).len(), 2);
        assert_eq!(next_high_water_mark(ready_sessions(&first)), Some(1770000000));

        // 源库变化：新增 s3 + 软删 s2（毫秒库，与实测 TRAE 库单位一致）。
        mutate_encrypted_db(
            &db_path,
            &key,
            "INSERT INTO chat_session (session_id, project_id, session_title, updated_at, deleted_at) \
             VALUES ('s3', 'p1', '会话三', 1771000000000, 0); \
             UPDATE chat_session SET deleted_at = 500, updated_at = 1770500000000 WHERE session_id = 's2';",
        );

        // 第二次增量读取：delta（s2/s3 + 防御性超集 s1）合并入缓存。
        let second = read_account_session_index_incremental(&temp, "checkin-a", "账号甲", &key);
        let sessions = ready_sessions(&second);
        let ids: Vec<(&str, bool)> = sessions
            .iter()
            .map(|entry| (entry.session_id.as_str(), entry.deleted))
            .collect();
        assert_eq!(
            ids,
            vec![("s3", false), ("s2", true), ("s1", false)]
        );
        assert_eq!(next_high_water_mark(sessions), Some(1771000000));

        // 第三次读取（无新变化）：幂等——高水位以上只剩 s3 行，upsert 后列表不变。
        let third = read_account_session_index_incremental(&temp, "checkin-a", "账号甲", &key);
        assert_eq!(ready_sessions(&third), sessions);
        let _ = std::fs::remove_dir_all(&temp);
    }

    #[test]
    fn incremental_read_seconds_unit_db_uses_second_boundary() {
        // 秒级库（防御性单位兼容）：高水位换算不得乘 1000 导致 delta 漏读。
        let temp = w3_temp_dir("seconds");
        let key = "bb".repeat(32);
        let db_path = instance_db_path(&temp, "checkin-a");
        create_encrypted_db_with_sessions(
            &db_path,
            &key,
            &[("s1", "会话一", 1770000000, 0)],
        );
        let first = read_account_session_index_incremental(&temp, "checkin-a", "账号甲", &key);
        assert_eq!(ready_sessions(&first).len(), 1);

        mutate_encrypted_db(
            &db_path,
            &key,
            "INSERT INTO chat_session (session_id, project_id, session_title, updated_at, deleted_at) \
             VALUES ('s2', 'p1', '会话二', 1771000000, 0)",
        );
        let second = read_account_session_index_incremental(&temp, "checkin-a", "账号甲", &key);
        assert_eq!(ready_sessions(&second).len(), 2);
        let _ = std::fs::remove_dir_all(&temp);
    }

    #[test]
    fn transient_read_failure_leaves_cache_untouched_for_retry() {
        // 瞬态错误语义：读取失败丢弃本轮——缓存与高水位不动，下轮重试兜底。
        let temp = w3_temp_dir("transient");
        let key = "cc".repeat(32);
        let db_path = instance_db_path(&temp, "checkin-a");
        create_encrypted_db_with_sessions(
            &db_path,
            &key,
            &[("s1", "会话一", 1770000000000, 0)],
        );

        // 成功读取 → 缓存落盘。
        let ok = read_account_session_index_incremental(&temp, "checkin-a", "账号甲", &key);
        assert_eq!(ready_sessions(&ok).len(), 1);
        let cache_before = SessionIndexCacheStore::new(&temp).load("checkin-a");

        // 库损坏（模拟复制/解密瞬态失败）→ ReadFailed。
        std::fs::write(&db_path, b"garbage-not-a-sqlite-db").unwrap();
        let failed = read_account_session_index_incremental(&temp, "checkin-a", "账号甲", &key);
        assert_eq!(failed.status, AccountSessionIndexStatus::ReadFailed);
        // 失败不回写缓存：旧缓存原样保留（重试时从此续读，不丢已观测数据）。
        assert_eq!(
            SessionIndexCacheStore::new(&temp).load("checkin-a"),
            cache_before
        );

        // 恢复后重试成功：从缓存续增量（无新变化 → 幂等同列表）。
        std::fs::remove_file(&db_path).unwrap();
        create_encrypted_db_with_sessions(
            &db_path,
            &key,
            &[("s1", "会话一", 1770000000000, 0)],
        );
        let retried = read_account_session_index_incremental(&temp, "checkin-a", "账号甲", &key);
        assert_eq!(ready_sessions(&retried).len(), 1);
        let _ = std::fs::remove_dir_all(&temp);
    }

    #[test]
    fn incremental_read_reports_no_instance_data_without_db() {
        let temp = w3_temp_dir("no-instance");
        let entry = read_account_session_index_incremental(
            &temp,
            "checkin-b",
            "账号乙",
            &"dd".repeat(32),
        );
        assert_eq!(entry.status, AccountSessionIndexStatus::NoInstanceData);
        // 无实例数据也有指纹（全 None）——db 出现后轮询可检测。
        assert_eq!(entry.fingerprint.db, None);
        let _ = std::fs::remove_dir_all(&temp);
    }
}
