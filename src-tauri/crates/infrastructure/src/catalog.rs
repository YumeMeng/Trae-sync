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

use std::collections::{BTreeMap, HashMap, HashSet};
use std::ffi::OsStr;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, OpenFlags, OptionalExtension, Transaction};
use serde::{Deserialize, Serialize};
use sha2::Digest;
use traesync_domain::{
    BrowseAccountNode, BrowseProjectNode, BrowseResult, BrowseSessionNode, ContentGraphHash,
    ConversationPreview, DiagnosticIntegrityAssertion, HistoryBrowseSummary, MessageProjection,
    OwnerObservation, ProjectIdentity, ProjectObservation, ProjectSourceAssignment,
    ScanFailureReason, SearchHit, SessionIdentity, SessionProjection, SessionVersion, SnapshotId,
    SourceSnapshotMeta, VersionClassification,
};
use traesync_ports::{
    CatalogMutationOutcome, CatalogReadError, CatalogRepository, ContentGraphHasher,
    SourceNormalizer,
};

use crate::catalog_path::CatalogCurrentPointer;
pub use crate::catalog_path::{resolve_current_catalog_path, CatalogPathError};
use crate::content_graph::DeterministicContentGraphHasher;
use crate::operation_lease::OperationLease;

/// SQLCipher 目录库实现。
///
/// 持有目录库路径与 raw key（私有），实现 `CatalogRepository` port。
pub struct SqlCipherCatalogRepository {
    db_path: PathBuf,
    /// SQLCipher raw key（64 字符 hex），私有，不通过方法暴露。
    raw_key: String,
}

/// 目录库代次 sidecar 的唯一版本化结构。
///
/// 该结构同时供目录库投影和旁路升级使用。严格拒绝未知字段，避免新旧版本
/// 静默互相覆盖不认识的元数据；字段缺失同样失败关闭。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CatalogGenerationMetadata {
    pub(crate) metadata_version: u32,
    pub(crate) package_format_version: u32,
    pub(crate) generation_id: String,
    pub(crate) schema_version: u32,
    pub(crate) catalog_schema_version: u32,
    pub(crate) mapping_version: String,
    pub(crate) key_wrapper_version: u32,
    /// 目录库业务写入协议的单调修订号；用于区分可自动补偿的提交后窗口。
    pub(crate) content_revision: u64,
    pub(crate) catalog_sha256: String,
    pub(crate) bytes: u64,
    pub(crate) semantic_counts: BTreeMap<String, u64>,
}

/// 2026-08-16 之前发布的目录库 sidecar：没有内容修订号和当前版本矩阵。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
struct LegacyCatalogGenerationMetadata {
    generation_id: String,
    schema_version: u32,
    catalog_sha256: String,
    bytes: u64,
    semantic_counts: BTreeMap<String, u64>,
}

/// 2026-08-17 候选发布的目录库 sidecar：补有 content_revision，但仍没有版本矩阵。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
struct LegacyCatalogGenerationMetadataWithRevision {
    generation_id: String,
    schema_version: u32,
    catalog_sha256: String,
    bytes: u64,
    semantic_counts: BTreeMap<String, u64>,
    content_revision: u64,
}

/// 读取磁盘 sidecar 时显式区分当前格式与两个已知 legacy 格式。
///
/// 每个变体都拒绝未知字段；不能使用 `flatten`，否则未来字段会被旧分支吞掉。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(untagged)]
enum CatalogGenerationMetadataOnDisk {
    Current(CatalogGenerationMetadata),
    LegacyWithRevision(LegacyCatalogGenerationMetadataWithRevision),
    Legacy(LegacyCatalogGenerationMetadata),
}

pub(crate) const CONTENT_REVISION_KEY: &str = "content_revision";
pub(crate) const GENERATION_METADATA_VERSION: u32 = 1;
pub(crate) const GENERATION_PACKAGE_FORMAT_VERSION: u32 = 1;
pub(crate) const CATALOG_MAPPING_VERSION: &str = "catalog-v1";
pub(crate) const CATALOG_KEY_WRAPPER_VERSION: u32 = 1;
pub(crate) const CATALOG_SCHEMA_VERSION: u32 = 1;

impl CatalogGenerationMetadata {
    pub(crate) fn new(
        generation_id: &str,
        schema_version: u32,
        content_revision: u64,
        catalog_sha256: String,
        bytes: u64,
        semantic_counts: BTreeMap<String, u64>,
    ) -> Self {
        Self {
            metadata_version: GENERATION_METADATA_VERSION,
            package_format_version: GENERATION_PACKAGE_FORMAT_VERSION,
            generation_id: generation_id.to_string(),
            schema_version,
            catalog_schema_version: schema_version,
            mapping_version: CATALOG_MAPPING_VERSION.to_string(),
            key_wrapper_version: CATALOG_KEY_WRAPPER_VERSION,
            content_revision,
            catalog_sha256,
            bytes,
            semantic_counts,
        }
    }

    pub(crate) fn version_matrix_matches(&self) -> bool {
        self.version_matrix_matches_for(CATALOG_SCHEMA_VERSION)
    }

    /// 旁路升级在 staging 中可验证相邻目标 schema；生产当前代次仍只接受 V1。
    pub(crate) fn version_matrix_matches_for(&self, expected_schema_version: u32) -> bool {
        self.metadata_version == GENERATION_METADATA_VERSION
            && self.package_format_version == GENERATION_PACKAGE_FORMAT_VERSION
            && self.schema_version == expected_schema_version
            && self.catalog_schema_version == self.schema_version
            && self.mapping_version == CATALOG_MAPPING_VERSION
            && self.key_wrapper_version == CATALOG_KEY_WRAPPER_VERSION
    }
}

impl CatalogGenerationMetadataOnDisk {
    fn generation_id(&self) -> &str {
        match self {
            Self::Current(metadata) => &metadata.generation_id,
            Self::LegacyWithRevision(metadata) => &metadata.generation_id,
            Self::Legacy(metadata) => &metadata.generation_id,
        }
    }

    fn schema_version(&self) -> u32 {
        match self {
            Self::Current(metadata) => metadata.schema_version,
            Self::LegacyWithRevision(metadata) => metadata.schema_version,
            Self::Legacy(metadata) => metadata.schema_version,
        }
    }

    fn content_revision(&self) -> Option<u64> {
        match self {
            Self::Current(metadata) => Some(metadata.content_revision),
            Self::LegacyWithRevision(metadata) => Some(metadata.content_revision),
            Self::Legacy(_) => None,
        }
    }

    fn bytes(&self) -> u64 {
        match self {
            Self::Current(metadata) => metadata.bytes,
            Self::LegacyWithRevision(metadata) => metadata.bytes,
            Self::Legacy(metadata) => metadata.bytes,
        }
    }

    fn catalog_sha256(&self) -> &str {
        match self {
            Self::Current(metadata) => &metadata.catalog_sha256,
            Self::LegacyWithRevision(metadata) => &metadata.catalog_sha256,
            Self::Legacy(metadata) => &metadata.catalog_sha256,
        }
    }

    fn semantic_counts(&self) -> &BTreeMap<String, u64> {
        match self {
            Self::Current(metadata) => &metadata.semantic_counts,
            Self::LegacyWithRevision(metadata) => &metadata.semantic_counts,
            Self::Legacy(metadata) => &metadata.semantic_counts,
        }
    }

    fn is_current(&self) -> bool {
        matches!(self, Self::Current(_))
    }
}

/// 首次创建目录库并在完整初始化后发布 current 指针；已有布局只读解析。
pub fn ensure_catalog_initialized(
    storage_root: &Path,
    raw_key: &str,
    recovery_root: &Path,
    operation_lease: &OperationLease,
) -> Result<PathBuf, CatalogPathError> {
    validate_catalog_lease_for_storage(storage_root, recovery_root, operation_lease)?;
    match resolve_current_catalog_path(storage_root) {
        Ok(path) => {
            // 已有代次必须先完成 sidecar 协调；revision 落后时只修复已知提交窗口，
            // 同 revision 的未知漂移则 fail-closed，避免把外部写入误当成正常刷新。
            let repository = SqlCipherCatalogRepository::new(path.clone(), raw_key.to_string());
            repository.reconcile_generation_metadata()?;
            return Ok(path);
        }
        Err(CatalogPathError::Missing) => {}
        Err(error) => return Err(error),
    }

    let catalog_root = storage_root.join("catalog");
    match fs::symlink_metadata(&catalog_root) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
            return Err(CatalogPathError::Invalid)
        }
        Ok(_) => {
            // 缺失指针但目录已有内容，说明上次发布可能中断；保留现场并拒绝猜测。
            if fs::read_dir(&catalog_root)
                .map_err(|_| CatalogPathError::Io)?
                .next()
                .is_some()
            {
                return Err(CatalogPathError::Invalid);
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir_all(&catalog_root).map_err(|_| CatalogPathError::Io)?;
        }
        Err(_) => return Err(CatalogPathError::Io),
    }

    let generations_root = catalog_root.join("generations");
    fs::create_dir_all(&generations_root).map_err(|_| CatalogPathError::Io)?;
    let generation_id = format!("catalog-gen-{}-{}", now_nanos(), std::process::id());
    let generation_dir = generations_root.join(&generation_id);
    fs::create_dir(&generation_dir).map_err(|_| CatalogPathError::Io)?;
    let catalog_path = generation_dir.join("catalog.db");
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&catalog_path)
        .map_err(|_| CatalogPathError::Io)?;

    let catalog = SqlCipherCatalogRepository::new(catalog_path.clone(), raw_key.to_string());
    if !catalog.ensure_initialized() {
        return Err(CatalogPathError::InitializationFailed);
    }
    write_initial_generation_metadata(&generation_dir, &catalog, &generation_id)?;
    publish_initial_current_pointer(&catalog_root, &generation_id)?;
    resolve_current_catalog_path(storage_root)
}

/// 为首次创建的目录库写入稳定身份，并立即完成两层完整性验证。
///
/// 该函数只允许“两个身份字段都缺失”或“两个字段都与期望一致”；部分写入、
/// 身份漂移和错误密钥都失败关闭，不能把旧目录库静默接管为新目录库。
pub fn initialize_catalog_identity(
    catalog_path: &Path,
    catalog_key: &str,
    catalog_id: &str,
    key_generation: u32,
    recovery_root: &Path,
    operation_lease: &OperationLease,
) -> Result<(), CatalogPathError> {
    validate_catalog_lease_for_catalog_path(catalog_path, recovery_root, operation_lease)?;
    validate_catalog_identity_input(catalog_key, catalog_id, key_generation)?;
    let repository =
        SqlCipherCatalogRepository::new(catalog_path.to_path_buf(), catalog_key.into());
    // 先拒绝 sidecar，再以无 CREATE 的读写连接打开已有目录库。
    reject_catalog_sidecars(catalog_path)?;
    repository.verify_existing_catalog_write_protocol()?;
    let mut connection = repository
        .open_existing_catalog()
        .ok_or(CatalogPathError::InitializationFailed)?;
    verify_existing_catalog_write_protocol(&connection)?;
    let existing = read_catalog_identity(&connection)?;
    match existing {
        None => {
            let transaction = connection
                .transaction()
                .map_err(|_| CatalogPathError::InitializationFailed)?;
            transaction
                .execute(
                    "INSERT INTO catalog_meta(key, value) VALUES ('catalog_id', ?1)",
                    [catalog_id],
                )
                .map_err(|_| CatalogPathError::InitializationFailed)?;
            transaction
                .execute(
                    "INSERT INTO catalog_meta(key, value) VALUES ('key_generation', ?1)",
                    [key_generation.to_string()],
                )
                .map_err(|_| CatalogPathError::InitializationFailed)?;
            bump_catalog_content_revision(&transaction)
                .map_err(|_| CatalogPathError::InitializationFailed)?;
            transaction
                .commit()
                .map_err(|_| CatalogPathError::InitializationFailed)?;
        }
        Some((existing_id, existing_generation))
            if existing_id == catalog_id && existing_generation == key_generation => {}
        Some(_) => return Err(CatalogPathError::Invalid),
    }
    verify_catalog_connection(&connection)?;
    drop(connection);
    repository.refresh_generation_metadata()
}

/// 使用 DPAPI 解出的目录库密钥复核已有目录库身份和完整性；
/// 若 sidecar revision 落后，则在验证通过后原子刷新该 sidecar。
pub fn verify_catalog_identity(
    catalog_path: &Path,
    catalog_key: &str,
    expected_catalog_id: &str,
    expected_key_generation: u32,
    recovery_root: &Path,
    operation_lease: &OperationLease,
) -> Result<(), CatalogPathError> {
    validate_catalog_lease_for_catalog_path(catalog_path, recovery_root, operation_lease)?;
    validate_catalog_identity_input(catalog_key, expected_catalog_id, expected_key_generation)?;
    let repository =
        SqlCipherCatalogRepository::new(catalog_path.to_path_buf(), catalog_key.into());
    // 身份复核是只读路径：先拒绝 sidecar，再用 READ_ONLY 且无 CREATE 的连接。
    reject_catalog_sidecars(catalog_path)?;
    let connection = repository.open_catalog_readonly_checked()?;
    match read_catalog_identity(&connection)? {
        Some((catalog_id, key_generation))
            if catalog_id == expected_catalog_id && key_generation == expected_key_generation => {}
        _ => return Err(CatalogPathError::Invalid),
    }
    verify_catalog_connection(&connection)?;
    drop(connection);
    repository.refresh_generation_metadata()
}

/// 解析固定 current 指针并协调其当前代次 sidecar。
///
/// 组合根必须先持有共享目录库租约；本函数只负责复核指针、目录库完整性和
/// 当前代次元数据，不自行创建恢复区或绕过租约。
pub fn reconcile_current_catalog_sidecar(
    storage_root: &Path,
    catalog_key: &str,
    recovery_root: &Path,
    operation_lease: &OperationLease,
) -> Result<PathBuf, CatalogPathError> {
    validate_catalog_lease_for_storage(storage_root, recovery_root, operation_lease)?;
    let catalog_path = resolve_current_catalog_path(storage_root)?;
    let repository = SqlCipherCatalogRepository::new(catalog_path.clone(), catalog_key.to_string());
    repository.refresh_generation_metadata()?;
    if resolve_current_catalog_path(storage_root)? != catalog_path {
        return Err(CatalogPathError::Invalid);
    }
    Ok(catalog_path)
}

/// 目录库公开入口的最小运行时契约：租约必须在同一已验证存储根上取得，
/// 不能只传入一个“看起来持有锁”的普通 OperationLease。
fn validate_catalog_lease_for_storage(
    storage_root: &Path,
    recovery_root: &Path,
    operation_lease: &OperationLease,
) -> Result<(), CatalogPathError> {
    operation_lease
        .validate_storage_root(storage_root)
        .map_err(|_| CatalogPathError::LeaseContextMismatch)?;
    operation_lease
        .validate_recovery_root(recovery_root)
        .map_err(|_| CatalogPathError::LeaseContextMismatch)
}

fn validate_catalog_lease_for_catalog_path(
    catalog_path: &Path,
    recovery_root: &Path,
    operation_lease: &OperationLease,
) -> Result<(), CatalogPathError> {
    let generation_dir = catalog_path.parent().ok_or(CatalogPathError::Invalid)?;
    let generations_root = generation_dir.parent().ok_or(CatalogPathError::Invalid)?;
    let catalog_root = generations_root.parent().ok_or(CatalogPathError::Invalid)?;
    let storage_root = catalog_root.parent().ok_or(CatalogPathError::Invalid)?;
    validate_catalog_lease_for_storage(storage_root, recovery_root, operation_lease)
}

fn validate_catalog_identity_input(
    catalog_key: &str,
    catalog_id: &str,
    key_generation: u32,
) -> Result<(), CatalogPathError> {
    if catalog_id.is_empty()
        || key_generation == 0
        || catalog_key.len() != 64
        || hex::decode(catalog_key)
            .map(|bytes| bytes.len() != 32)
            .unwrap_or(true)
    {
        return Err(CatalogPathError::Invalid);
    }
    Ok(())
}

fn read_catalog_identity(
    connection: &Connection,
) -> Result<Option<(String, u32)>, CatalogPathError> {
    let catalog_id = connection
        .query_row(
            "SELECT value FROM catalog_meta WHERE key = 'catalog_id'",
            [],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(|_| CatalogPathError::InitializationFailed)?;
    let key_generation = connection
        .query_row(
            "SELECT value FROM catalog_meta WHERE key = 'key_generation'",
            [],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(|_| CatalogPathError::InitializationFailed)?;
    match (catalog_id, key_generation) {
        (None, None) => Ok(None),
        (Some(catalog_id), Some(key_generation)) => {
            let key_generation = key_generation
                .parse::<u32>()
                .map_err(|_| CatalogPathError::Invalid)?;
            Ok(Some((catalog_id, key_generation)))
        }
        _ => Err(CatalogPathError::Invalid),
    }
}

/// 读取目录库内事务性内容修订号；旧版本目录库可能尚未创建该键。
fn read_content_revision(connection: &Connection) -> Result<Option<u64>, CatalogPathError> {
    let value = connection
        .query_row(
            "SELECT value FROM catalog_meta WHERE key = ?1",
            [CONTENT_REVISION_KEY],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(|_| CatalogPathError::InitializationFailed)?;
    value
        .map(|value| value.parse::<u64>().map_err(|_| CatalogPathError::Invalid))
        .transpose()
}

/// 三方 schema 见证：目录库 metadata、SQLite user_version 和 sidecar 必须一致。
fn read_catalog_schema_version(connection: &Connection) -> Result<u32, CatalogPathError> {
    let catalog_meta_version: u32 = connection
        .query_row(
            "SELECT value FROM catalog_meta WHERE key = 'schema_version'",
            [],
            |row| row.get::<_, String>(0),
        )
        .map_err(|_| CatalogPathError::Invalid)?
        .parse()
        .map_err(|_| CatalogPathError::Invalid)?;
    let user_version: u32 = connection
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .map_err(|_| CatalogPathError::Invalid)?;
    if catalog_meta_version != user_version {
        return Err(CatalogPathError::Invalid);
    }
    Ok(catalog_meta_version)
}

/// 在同一 SQLite 事务中递增内容修订号；调用方必须在业务写入前后保持同一事务。
pub(crate) fn bump_catalog_content_revision(transaction: &Transaction<'_>) -> rusqlite::Result<()> {
    transaction.execute(
        "INSERT OR IGNORE INTO catalog_meta(key, value) VALUES (?1, '0')",
        [CONTENT_REVISION_KEY],
    )?;
    transaction.execute(
        "UPDATE catalog_meta
         SET value = CAST(value AS INTEGER) + 1
         WHERE key = ?1",
        [CONTENT_REVISION_KEY],
    )?;
    Ok(())
}

/// 为新建目录库/升级 staging 设置固定写协议。
///
/// 只有数据库尚未作为现有代次发布时才允许执行 `PRAGMA journal_mode = DELETE`。
/// 对已有目录库必须使用下面的只验证函数，避免在校验失败前改写数据库 header。
pub(crate) fn configure_new_catalog_write_protocol(
    connection: &Connection,
) -> rusqlite::Result<()> {
    let journal_mode: String =
        connection.query_row("PRAGMA journal_mode = DELETE", [], |row| row.get(0))?;
    if !journal_mode.eq_ignore_ascii_case("delete") {
        return Err(rusqlite::Error::InvalidQuery);
    }
    connection.execute_batch("PRAGMA synchronous = FULL;")?;
    let synchronous: i64 = connection.query_row("PRAGMA synchronous", [], |row| row.get(0))?;
    if synchronous != 2 {
        return Err(rusqlite::Error::InvalidQuery);
    }
    Ok(())
}

/// 验证已有目录库的持久 journal 协议，并只设置当前连接的同步级别。
///
/// 该函数绝不改变持久 `journal_mode`；WAL、TRUNCATE 等非 DELETE 状态会直接失败。
pub(crate) fn verify_existing_catalog_write_protocol(
    connection: &Connection,
) -> Result<(), CatalogPathError> {
    let journal_mode: String = connection
        .query_row("PRAGMA journal_mode", [], |row| row.get(0))
        .map_err(|_| CatalogPathError::InitializationFailed)?;
    if !journal_mode.eq_ignore_ascii_case("delete") {
        return Err(CatalogPathError::CatalogWriteProtocolUpgradeRequired);
    }
    connection
        .execute_batch("PRAGMA synchronous = FULL;")
        .map_err(|_| CatalogPathError::InitializationFailed)?;
    let synchronous: i64 = connection
        .query_row("PRAGMA synchronous", [], |row| row.get(0))
        .map_err(|_| CatalogPathError::InitializationFailed)?;
    if synchronous != 2 {
        return Err(CatalogPathError::InitializationFailed);
    }
    Ok(())
}

/// 只读启动/协调路径使用的 journal 协议见证。
pub(crate) fn verify_catalog_read_protocol(
    connection: &Connection,
) -> Result<(), CatalogPathError> {
    let journal_mode: String = connection
        .query_row("PRAGMA journal_mode", [], |row| row.get(0))
        .map_err(|_| CatalogPathError::InitializationFailed)?;
    if !journal_mode.eq_ignore_ascii_case("delete") {
        return Err(CatalogPathError::CatalogWriteProtocolUpgradeRequired);
    }
    Ok(())
}

/// 目录库使用 DELETE journal；只要发现任一 SQLite sidecar 就失败关闭。
pub(crate) fn reject_catalog_sidecars(catalog_path: &Path) -> Result<(), CatalogPathError> {
    let Some(file_name) = catalog_path.file_name().and_then(|name| name.to_str()) else {
        return Err(CatalogPathError::Invalid);
    };
    for suffix in ["-wal", "-shm", "-journal"] {
        let sidecar = catalog_path.with_file_name(format!("{file_name}{suffix}"));
        match fs::symlink_metadata(sidecar) {
            Ok(_) if suffix != "-journal" => {
                return Err(CatalogPathError::CatalogWriteProtocolUpgradeRequired)
            }
            Ok(_) => return Err(CatalogPathError::Invalid),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return Err(CatalogPathError::Io),
        }
    }
    Ok(())
}

fn catalog_sidecars_snapshot(catalog_path: &Path) -> [bool; 3] {
    ["-wal", "-shm", "-journal"].map(|suffix| {
        let Some(file_name) = catalog_path.file_name().and_then(|name| name.to_str()) else {
            return false;
        };
        catalog_path
            .with_file_name(format!("{file_name}{suffix}"))
            .exists()
    })
}

fn cleanup_readonly_created_sidecars(catalog_path: &Path, existed_before: &[bool; 3]) {
    let Some(file_name) = catalog_path.file_name().and_then(|name| name.to_str()) else {
        return;
    };
    for (index, suffix) in ["-wal", "-shm", "-journal"].into_iter().enumerate() {
        if existed_before[index] {
            continue;
        }
        let sidecar = catalog_path.with_file_name(format!("{file_name}{suffix}"));
        let Ok(metadata) = fs::symlink_metadata(&sidecar) else {
            continue;
        };
        if metadata.file_type().is_symlink() || !metadata.is_file() || metadata.len() > 32 * 1024 {
            continue;
        }
        let _ = fs::remove_file(sidecar);
    }
}

fn verify_catalog_connection(connection: &Connection) -> Result<(), CatalogPathError> {
    let mut cipher_statement = connection
        .prepare("PRAGMA cipher_integrity_check")
        .map_err(|_| CatalogPathError::InitializationFailed)?;
    let cipher_error_count = cipher_statement
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(|_| CatalogPathError::InitializationFailed)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| CatalogPathError::InitializationFailed)?
        .len();
    if cipher_error_count != 0 {
        return Err(CatalogPathError::InitializationFailed);
    }
    let sqlite_result = connection
        .query_row("PRAGMA integrity_check", [], |row| row.get::<_, String>(0))
        .map_err(|_| CatalogPathError::InitializationFailed)?;
    if sqlite_result != "ok" {
        return Err(CatalogPathError::InitializationFailed);
    }
    Ok(())
}

fn publish_initial_current_pointer(
    catalog_root: &Path,
    generation_id: &str,
) -> Result<(), CatalogPathError> {
    let pointer = catalog_root.join("current.json");
    let temporary = catalog_root.join(format!(
        ".current.json.tmp-{}-{}",
        now_nanos(),
        std::process::id()
    ));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)
        .map_err(|_| CatalogPathError::Io)?;
    let result = (|| {
        serde_json::to_writer_pretty(
            &mut file,
            &CatalogCurrentPointer {
                generation_id: generation_id.to_string(),
            },
        )
        .map_err(|_| CatalogPathError::Io)?;
        file.write_all(b"\n").map_err(|_| CatalogPathError::Io)?;
        file.sync_all().map_err(|_| CatalogPathError::Io)
    })();
    drop(file);
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
        return result;
    }
    let result = fs::hard_link(&temporary, &pointer)
        .map_err(|_| CatalogPathError::Io)
        .and_then(|_| fs::remove_file(&temporary).map_err(|_| CatalogPathError::Io));
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

fn write_initial_generation_metadata(
    generation_dir: &Path,
    catalog: &SqlCipherCatalogRepository,
    generation_id: &str,
) -> Result<(), CatalogPathError> {
    let catalog_path = generation_dir.join("catalog.db");
    let bytes = fs::metadata(&catalog_path)
        .map_err(|_| CatalogPathError::Io)?
        .len();
    let catalog_sha256 = sha256_file(&catalog_path)?;
    let connection = catalog
        .open_catalog_readonly()
        .ok_or(CatalogPathError::InitializationFailed)?;
    verify_catalog_read_protocol(&connection)
        .map_err(|_| CatalogPathError::InitializationFailed)?;
    let semantic_counts = read_semantic_counts(&connection)?;
    let metadata = CatalogGenerationMetadata::new(
        generation_id,
        CATALOG_SCHEMA_VERSION,
        read_content_revision(&connection)?.ok_or(CatalogPathError::Invalid)?,
        catalog_sha256,
        bytes,
        semantic_counts,
    );
    drop(connection);
    let path = generation_dir.join("generation.json");
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|_| CatalogPathError::Io)?;
    serde_json::to_writer_pretty(&mut file, &metadata).map_err(|_| CatalogPathError::Io)?;
    file.write_all(b"\n").map_err(|_| CatalogPathError::Io)?;
    file.sync_all().map_err(|_| CatalogPathError::Io)
}

/// 以临时文件加同卷原子替换更新完整性 sidecar；失败时不留下临时文件。
fn write_generation_metadata_atomically(
    path: &Path,
    metadata: &CatalogGenerationMetadata,
) -> Result<(), CatalogPathError> {
    let parent = path.parent().ok_or(CatalogPathError::Invalid)?;
    let temporary = parent.join(format!(
        ".generation.json.tmp-{}-{}",
        now_nanos(),
        std::process::id()
    ));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)
        .map_err(|_| CatalogPathError::Io)?;
    let result = (|| {
        serde_json::to_writer_pretty(&mut file, metadata).map_err(|_| CatalogPathError::Io)?;
        file.write_all(b"\n").map_err(|_| CatalogPathError::Io)?;
        file.sync_all().map_err(|_| CatalogPathError::Io)
    })();
    drop(file);
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
        return result;
    }
    crate::atomic_publish::publish_replacing(&temporary, path).map_err(|_| CatalogPathError::Io)
}

fn read_semantic_counts(
    connection: &Connection,
) -> Result<BTreeMap<String, u64>, CatalogPathError> {
    let mut statement = connection
        .prepare(
            "SELECT name FROM sqlite_master \
             WHERE type = 'table' AND name NOT LIKE 'sqlite_%' \
             ORDER BY name",
        )
        .map_err(|_| CatalogPathError::InitializationFailed)?;
    let names = statement
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(|_| CatalogPathError::InitializationFailed)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| CatalogPathError::InitializationFailed)?
        .into_iter()
        .filter(|name| name != "catalog_meta" && name != "catalog_migration_log")
        .collect::<Vec<_>>();
    let mut counts = BTreeMap::new();
    for table in names {
        let quoted = table.replace('"', "\"\"");
        let sql = format!("SELECT COUNT(*) FROM \"{quoted}\"");
        let count: i64 = connection
            .query_row(&sql, [], |row| row.get(0))
            .map_err(|_| CatalogPathError::InitializationFailed)?;
        if count < 0 {
            return Err(CatalogPathError::InitializationFailed);
        }
        counts.insert(table, count as u64);
    }
    Ok(counts)
}

fn sha256_file(path: &Path) -> Result<String, CatalogPathError> {
    let mut file = File::open(path).map_err(|_| CatalogPathError::Io)?;
    let mut digest = sha2::Sha256::new();
    // 大文件哈希缓冲区放在堆上，避免 Windows 默认线程栈因 1 MiB 局部数组溢出。
    let mut buffer = vec![0_u8; 1024 * 1024];
    loop {
        let count =
            std::io::Read::read(&mut file, &mut buffer).map_err(|_| CatalogPathError::Io)?;
        if count == 0 {
            break;
        }
        digest.update(&buffer[..count]);
    }
    Ok(hex::encode(digest.finalize()))
}

/// 构造 SQLite 只读 URI，避免验证路径隐式启用 CREATE。
fn sqlite_readonly_uri(path: &Path) -> Option<String> {
    let absolute = fs::canonicalize(path).ok()?;
    let raw = absolute.to_string_lossy();
    let raw = raw.strip_prefix(r"\\?\").unwrap_or(&raw);
    let normalized = raw.replace('\\', "/");
    let mut encoded = String::with_capacity(normalized.len());
    for byte in normalized.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'/' | b'-' | b'_' | b'.' | b'~' | b':') {
            encoded.push(byte as char);
        } else {
            encoded.push('%');
            encoded.push_str(&format!("{byte:02X}"));
        }
    }
    Some(format!("file:///{encoded}?mode=ro"))
}

impl SqlCipherCatalogRepository {
    pub fn new(db_path: PathBuf, raw_key: String) -> Self {
        Self { db_path, raw_key }
    }

    /// 以允许创建的读写方式打开目录库；仅供首次初始化路径使用。
    fn open_catalog(&self) -> Option<Connection> {
        self.open_catalog_with_flags(
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE,
        )
    }

    /// 以 READ_WRITE 且禁止 CREATE 的方式打开已有目录库。
    ///
    /// 当前代次必须先拒绝 SQLite sidecar，再打开连接，避免把缺失/漂移现场
    /// 静默创建成新的空库。
    fn open_existing_catalog(&self) -> Option<Connection> {
        reject_catalog_sidecars(&self.db_path).ok()?;
        let metadata = fs::symlink_metadata(&self.db_path).ok()?;
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return None;
        }
        self.open_catalog_with_flags(OpenFlags::SQLITE_OPEN_READ_WRITE)
    }

    /// 以 READ_ONLY 且禁止 CREATE 的方式打开已有目录库，用于启动协调和完整性验证。
    fn open_catalog_readonly(&self) -> Option<Connection> {
        self.open_catalog_readonly_checked().ok()
    }

    /// 以结构化错误打开现有目录库，保留写协议不兼容的稳定错误码。
    fn open_catalog_readonly_checked(&self) -> Result<Connection, CatalogPathError> {
        reject_catalog_sidecars(&self.db_path)?;
        let metadata =
            fs::symlink_metadata(&self.db_path).map_err(|_| CatalogPathError::Invalid)?;
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(CatalogPathError::Invalid);
        }
        let sidecars_before = catalog_sidecars_snapshot(&self.db_path);
        let result = (|| {
            let flags = OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI;
            let connection = if let Some(uri) = sqlite_readonly_uri(&self.db_path) {
                Connection::open_with_flags(&uri, flags)
                    .map_err(|_| CatalogPathError::InitializationFailed)?
            } else {
                self.open_catalog_with_flags(OpenFlags::SQLITE_OPEN_READ_ONLY)
                    .ok_or(CatalogPathError::InitializationFailed)?
            };
            let pragma = format!("PRAGMA key = \"x'{}'\";", self.raw_key);
            connection
                .execute_batch(&pragma)
                .map_err(|_| CatalogPathError::InitializationFailed)?;
            verify_catalog_read_protocol(&connection)?;
            Ok(connection)
        })();
        if result.is_err() {
            // WAL 头现场的只读关闭可能创建空锁文件；仅清理本次新生且无数据的普通文件。
            cleanup_readonly_created_sidecars(&self.db_path, &sidecars_before);
        }
        result
    }

    /// 在切换到读写连接前，用无 CREATE 的只读连接完成持久写协议预检。
    fn verify_existing_catalog_write_protocol(&self) -> Result<(), CatalogPathError> {
        let connection = self.open_catalog_readonly_checked()?;
        drop(connection);
        Ok(())
    }

    /// 按指定 SQLite 打开标志设置 raw key。
    fn open_catalog_with_flags(&self, flags: OpenFlags) -> Option<Connection> {
        let conn = Connection::open_with_flags(&self.db_path, flags).ok()?;
        // raw key 语法：x'<hex>' —— 不进入日志
        let pragma = format!("PRAGMA key = \"x'{}'\";", self.raw_key);
        conn.execute_batch(&pragma).ok()?;
        Some(conn)
    }

    /// 刷新当前代次的完整性 sidecar；独立目录库测试路径没有 sidecar 时不做任何事。
    ///
    /// `content_revision` 与目录库业务事务一起提交。只有 revision 落后时才允许
    /// 自动补发 sidecar；同 revision 但哈希/计数不一致属于未知漂移，必须拒绝覆盖。
    fn refresh_generation_metadata(&self) -> Result<(), CatalogPathError> {
        let Some((generation_dir, metadata_path)) = self.managed_generation_paths() else {
            return Ok(());
        };
        let catalog_root = generation_dir
            .parent()
            .and_then(Path::parent)
            .ok_or(CatalogPathError::Invalid)?;
        let storage_root = catalog_root.parent().ok_or(CatalogPathError::Invalid)?;
        if resolve_current_catalog_path(storage_root)? != self.db_path {
            return Err(CatalogPathError::Invalid);
        }

        let generation_metadata =
            fs::symlink_metadata(&generation_dir).map_err(|_| CatalogPathError::Invalid)?;
        if generation_metadata.file_type().is_symlink() || !generation_metadata.is_dir() {
            return Err(CatalogPathError::Invalid);
        }
        let metadata_file =
            fs::symlink_metadata(&metadata_path).map_err(|_| CatalogPathError::Invalid)?;
        if metadata_file.file_type().is_symlink() || !metadata_file.is_file() {
            return Err(CatalogPathError::Invalid);
        }
        let metadata_on_disk: CatalogGenerationMetadataOnDisk = serde_json::from_reader(
            File::open(&metadata_path).map_err(|_| CatalogPathError::Invalid)?,
        )
        .map_err(|_| CatalogPathError::Invalid)?;
        let directory_generation_id = generation_dir
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or(CatalogPathError::Invalid)?;
        if metadata_on_disk.generation_id() != directory_generation_id {
            return Err(CatalogPathError::Invalid);
        }
        // 启动/普通写入只允许当前 V1；旁路升级使用 version_matrix_matches_for
        // 在自己的 staging 校验中显式放宽到目标相邻版本。
        if metadata_on_disk.schema_version() != CATALOG_SCHEMA_VERSION {
            return Err(CatalogPathError::Invalid);
        }
        if let CatalogGenerationMetadataOnDisk::Current(metadata) = &metadata_on_disk {
            if !metadata.version_matrix_matches() {
                return Err(CatalogPathError::Invalid);
            }
        }

        let catalog_file =
            fs::symlink_metadata(&self.db_path).map_err(|_| CatalogPathError::Invalid)?;
        if catalog_file.file_type().is_symlink() || !catalog_file.is_file() {
            return Err(CatalogPathError::Invalid);
        }
        // 先拒绝 WAL/SHM/journal，再以 READ_ONLY 打开；协调路径不得创建连接或改写 DB header。
        reject_catalog_sidecars(&self.db_path)?;
        let connection = self.open_catalog_readonly_checked()?;
        verify_catalog_connection(&connection)?;
        let database_schema_version = read_catalog_schema_version(&connection)?;
        if database_schema_version != CATALOG_SCHEMA_VERSION
            || metadata_on_disk.schema_version() != database_schema_version
        {
            return Err(CatalogPathError::Invalid);
        }
        // 旧目录库可能没有 revision。按 0 读取即可，启动协调只升级 sidecar，
        // 不向 catalog_meta 回写任何字段，避免一次只读启动改变目录库字节。
        let (db_revision, database_has_revision) = match read_content_revision(&connection)? {
            Some(revision) => (revision, true),
            None => (0, false),
        };
        if metadata_on_disk
            .content_revision()
            .is_some_and(|revision| revision > db_revision)
        {
            return Err(CatalogPathError::Invalid);
        }
        let semantic_counts = read_semantic_counts(&connection)?;
        drop(connection);

        let bytes = fs::metadata(&self.db_path)
            .map_err(|_| CatalogPathError::Io)?
            .len();
        let catalog_sha256 = sha256_file(&self.db_path)?;
        let sidecar_matches_catalog = metadata_on_disk.bytes() == bytes
            && metadata_on_disk.catalog_sha256() == catalog_sha256
            && metadata_on_disk.semantic_counts() == &semantic_counts;
        let metadata_revision = metadata_on_disk.content_revision();
        let already_matches = metadata_on_disk.is_current()
            && metadata_revision == Some(db_revision)
            && sidecar_matches_catalog;
        if already_matches {
            return Ok(());
        }

        // 没有 content_revision 的 legacy sidecar 无法证明“提交后未发布”窗口；
        // 只有其字节、哈希和语义计数与当前数据库完全一致时才允许补齐格式。
        // 有 revision 的 current/legacy sidecar 则沿用 revision 单调规则：同 revision
        // 的不一致是未知漂移，落后 revision 才是可自动补偿窗口。
        if !sidecar_matches_catalog {
            let repair_is_known_commit_window = database_has_revision
                && metadata_revision.is_some_and(|revision| revision < db_revision);
            if !repair_is_known_commit_window {
                return Err(CatalogPathError::Invalid);
            }
        }

        let metadata = CatalogGenerationMetadata::new(
            directory_generation_id,
            database_schema_version,
            db_revision,
            catalog_sha256,
            bytes,
            semantic_counts,
        );
        write_generation_metadata_atomically(&metadata_path, &metadata)?;

        // 发布后重新读取，确保调用方不会继续使用半写入的 sidecar。
        let published: CatalogGenerationMetadata = serde_json::from_reader(
            File::open(&metadata_path).map_err(|_| CatalogPathError::Invalid)?,
        )
        .map_err(|_| CatalogPathError::Invalid)?;
        if published != metadata {
            return Err(CatalogPathError::Invalid);
        }
        if resolve_current_catalog_path(storage_root)? != self.db_path {
            return Err(CatalogPathError::Invalid);
        }
        reject_catalog_sidecars(&self.db_path)?;
        Ok(())
    }

    /// 启动或写入前协调已提交但尚未发布的 sidecar。
    fn reconcile_generation_metadata(&self) -> Result<(), CatalogPathError> {
        self.refresh_generation_metadata()
    }

    /// 只有标准 `<root>/catalog/generations/<id>/catalog.db` 布局才需要 sidecar。
    fn managed_generation_paths(&self) -> Option<(PathBuf, PathBuf)> {
        let generation_dir = self.db_path.parent()?;
        let generations_root = generation_dir.parent()?;
        if generations_root.file_name() != Some(OsStr::new("generations")) {
            return None;
        }
        Some((
            generation_dir.to_path_buf(),
            generation_dir.join("generation.json"),
        ))
    }

    /// 执行 checked FTS 查询；目录库连接、SQL 或行映射失败均向上报告。
    fn search_messages_with_project_checked(
        &self,
        query: &str,
        project_id: Option<&str>,
    ) -> Result<Vec<SearchHit>, CatalogReadError> {
        let conn = self
            .open_catalog_readonly()
            .ok_or(CatalogReadError::Unavailable)?;
        verify_catalog_connection(&conn).map_err(|_| CatalogReadError::Unavailable)?;
        // FTS5 MATCH 查询，JOIN message_projection 获取 soft_deleted/role，
        // JOIN session_projection 获取 project_id/title，
        // JOIN project_identity 获取 project soft_deleted。
        // R9：同时排除软删除消息、会话和项目——任一层级软删除都不出现在搜索结果。
        let mut stmt = conn
            .prepare(
                "SELECT mp.message_id, mp.session_id, mp.role, mp.content_excerpt, mp.namespace, \
                    sp.project_id, sp.active_title \
             FROM message_fts \
             JOIN message_projection mp \
               ON message_fts.message_id = mp.message_id \
              AND message_fts.session_id = mp.session_id \
              AND message_fts.namespace = mp.namespace \
             JOIN session_projection sp \
               ON sp.namespace = mp.namespace AND sp.original_session_id = mp.session_id \
             JOIN project_identity pi \
               ON pi.project_id = sp.project_id \
             WHERE message_fts MATCH ?1 \
               AND mp.soft_deleted = 0 \
               AND sp.soft_deleted = 0 \
               AND pi.soft_deleted = 0 \
               AND (?2 IS NULL OR sp.project_id = ?2)",
            )
            .map_err(|_| CatalogReadError::Unavailable)?;
        let project_id = project_id.map(str::to_string);
        let rows = stmt
            .query_map(rusqlite::params![query, project_id], |row| {
                let message_id: String = row.get(0)?;
                let session_id: String = row.get(1)?;
                let role: String = row.get(2)?;
                let content_excerpt: String = row.get(3)?;
                let namespace: String = row.get(4)?;
                let project_id: String = row.get(5)?;
                let title: String = row.get(6)?;
                Ok(SearchHit {
                    session_identity: SessionIdentity::new(&namespace, &session_id),
                    message_id,
                    project_id,
                    title,
                    content_excerpt,
                    role,
                })
            })
            .map_err(|_| CatalogReadError::Unavailable)?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|_| CatalogReadError::Unavailable)
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

fn now_nanos() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_nanos())
        .unwrap_or(0)
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
) -> rusqlite::Result<Vec<MessageProjection>> {
    let sql = if exclude_soft_deleted {
        "SELECT message_id, session_id, role, content_excerpt, soft_deleted, seq, turn_id \
         FROM message_projection WHERE namespace = ?1 AND session_id = ?2 AND soft_deleted = 0 \
         ORDER BY seq ASC"
    } else {
        "SELECT message_id, session_id, role, content_excerpt, soft_deleted, seq, turn_id \
         FROM message_projection WHERE namespace = ?1 AND session_id = ?2 \
         ORDER BY seq ASC"
    };
    let mut stmt = conn.prepare(sql)?;
    let rows = stmt.query_map(rusqlite::params![namespace, session_id], |row| {
        Ok(MessageProjection {
            message_id: row.get(0)?,
            session_id: row.get(1)?,
            role: row.get(2)?,
            content_excerpt: row.get(3)?,
            soft_deleted: row.get::<_, i64>(4)? != 0,
            seq: row.get::<_, i64>(5)? as u64,
            turn_id: row.get(6)?,
        })
    })?;
    rows.collect()
}

impl CatalogRepository for SqlCipherCatalogRepository {
    fn ensure_initialized(&self) -> bool {
        if reject_catalog_sidecars(&self.db_path).is_err() {
            return false;
        }
        let existing_file = match fs::symlink_metadata(&self.db_path) {
            Ok(metadata) => {
                if metadata.file_type().is_symlink() || !metadata.is_file() {
                    return false;
                }
                true
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
            Err(_) => return false,
        };
        let conn = match if existing_file {
            self.open_existing_catalog()
        } else {
            self.open_catalog()
        } {
            Some(c) => c,
            None => return false,
        };
        // 检查是否已初始化（catalog_meta 有 schema_version）
        let already = match conn
            .query_row(
                "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'catalog_meta' LIMIT 1",
                [],
                |row| row.get::<_, i64>(0),
            )
            .optional()
        {
            Ok(value) => value.is_some(),
            Err(_) => return false,
        };
        let protocol_valid = if already || existing_file {
            // 已有文件只验证持久 journal，不执行 journal_mode 赋值。
            verify_existing_catalog_write_protocol(&conn).is_ok()
        } else {
            configure_new_catalog_write_protocol(&conn).is_ok()
        };
        if !protocol_valid {
            return false;
        }
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
        CREATE TABLE IF NOT EXISTS operation_record (
            operation_id TEXT PRIMARY KEY,
            data_location_id TEXT NOT NULL,
            state TEXT NOT NULL,
            affected_rows INTEGER NOT NULL,
            db_fingerprint TEXT NOT NULL,
            wal_fingerprint TEXT,
            shm_fingerprint TEXT,
            updated_at INTEGER NOT NULL
        );
        CREATE VIRTUAL TABLE IF NOT EXISTS message_fts USING fts5(
            message_id, session_id, content_excerpt, namespace
        );
        "#;
        if conn.execute_batch(batch).is_err() {
            return false;
        }
        // 写入 schema_version
        if conn
            .execute(
                "INSERT OR REPLACE INTO catalog_meta (key, value) VALUES ('schema_version', '1')",
                [],
            )
            .is_err()
        {
            return false;
        }
        if conn
            .execute(
                "INSERT OR IGNORE INTO catalog_meta (key, value) VALUES (?1, '0')",
                [CONTENT_REVISION_KEY],
            )
            .is_err()
        {
            return false;
        }
        conn.execute_batch("PRAGMA user_version = 1;").is_ok()
    }

    fn project_snapshot(
        &self,
        snapshot_meta: &SourceSnapshotMeta,
        snapshot_dir: &Path,
        normalizer: &dyn SourceNormalizer,
    ) -> Result<(), ScanFailureReason> {
        self.project_snapshot_with_validation(snapshot_meta, snapshot_dir, normalizer, &|| true)
    }

    fn project_snapshot_with_validation(
        &self,
        snapshot_meta: &SourceSnapshotMeta,
        snapshot_dir: &Path,
        normalizer: &dyn SourceNormalizer,
        is_authorized: &dyn Fn() -> bool,
    ) -> Result<(), ScanFailureReason> {
        self.project_snapshot_with_context_validation(
            snapshot_meta,
            snapshot_dir,
            normalizer,
            is_authorized,
            &|| true,
        )
    }

    fn project_snapshot_with_context_validation(
        &self,
        snapshot_meta: &SourceSnapshotMeta,
        snapshot_dir: &Path,
        normalizer: &dyn SourceNormalizer,
        is_authorized: &dyn Fn() -> bool,
        validate_context: &dyn Fn() -> bool,
    ) -> Result<(), ScanFailureReason> {
        if !is_authorized() || !validate_context() {
            return Err(ScanFailureReason::NotAuthorized);
        }

        // 1. 一次性读取全部快照数据；任一读取失败都不得发布部分投影。
        let normalized = normalizer
            .read_snapshot_checked(snapshot_dir)
            .map_err(|_| ScanFailureReason::CatalogTransactionFailed)?;
        let projects = normalized.projects;
        let sessions = normalized.sessions;
        let messages = normalized.messages;
        if projects.is_empty() && sessions.is_empty() && messages.is_empty() {
            // 空投影通常表示源数据库未被正确读取；禁止把 0/0/0 发布成成功历史。
            return Err(ScanFailureReason::CatalogTransactionFailed);
        }
        // 读取每个项目的 owner；缺失 owner 会让归属证据不完整，直接失败。
        let mut owners = HashMap::new();
        for project in &projects {
            let Some(owner) = normalizer.read_project_owner(snapshot_dir, &project.project_id)
            else {
                return Err(ScanFailureReason::CatalogTransactionFailed);
            };
            if owner.trim().is_empty() {
                return Err(ScanFailureReason::CatalogTransactionFailed);
            }
            owners.insert(project.project_id.clone(), owner);
        }

        // 检查快照内部关联，防止项目、会话、消息任一层读取不完整后提交孤儿数据。
        let project_ids: HashSet<&str> = projects
            .iter()
            .map(|project| project.project_id.as_str())
            .collect();
        if project_ids.len() != projects.len()
            || sessions
                .iter()
                .any(|session| !project_ids.contains(session.project_id.as_str()))
        {
            return Err(ScanFailureReason::CatalogTransactionFailed);
        }
        let session_ids: HashSet<&str> = sessions
            .iter()
            .map(|session| session.session_identity.original_session_id.as_str())
            .collect();
        if session_ids.len() != sessions.len()
            || messages
                .iter()
                .any(|message| !session_ids.contains(message.session_id.as_str()))
        {
            return Err(ScanFailureReason::CatalogTransactionFailed);
        }

        if !is_authorized() || !validate_context() {
            return Err(ScanFailureReason::NotAuthorized);
        }

        // 2. 打开 catalog 连接并开事务
        reject_catalog_sidecars(&self.db_path)
            .map_err(|_| ScanFailureReason::CatalogTransactionFailed)?;
        let conn = self
            .open_existing_catalog()
            .ok_or(ScanFailureReason::CatalogTransactionFailed)?;
        if verify_existing_catalog_write_protocol(&conn).is_err() {
            return Err(ScanFailureReason::CatalogTransactionFailed);
        }
        if !validate_context() {
            return Err(ScanFailureReason::NotAuthorized);
        }
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
                // 提交前最后一次检查；授权失效时回滚事务，不留下半成功投影。
                if !is_authorized() || !validate_context() {
                    let _ = conn.execute_batch("ROLLBACK;");
                    return Err(ScanFailureReason::NotAuthorized);
                }
                if conn
                    .execute(
                        "INSERT OR IGNORE INTO catalog_meta(key, value) VALUES (?1, '0')",
                        [CONTENT_REVISION_KEY],
                    )
                    .and_then(|_| {
                        conn.execute(
                            "UPDATE catalog_meta
                             SET value = CAST(value AS INTEGER) + 1
                             WHERE key = ?1",
                            [CONTENT_REVISION_KEY],
                        )
                    })
                    .is_err()
                {
                    let _ = conn.execute_batch("ROLLBACK;");
                    return Err(ScanFailureReason::CatalogTransactionFailed);
                }
                if conn.execute_batch("COMMIT;").is_err() {
                    let _ = conn.execute_batch("ROLLBACK;");
                    return Err(ScanFailureReason::CatalogTransactionFailed);
                }
                drop(conn);
                self.refresh_generation_metadata()
                    .map_err(|_| ScanFailureReason::CatalogMetadataRepairRequired)?;
                Ok(())
            }
            Err(e) => {
                let _ = conn.execute_batch("ROLLBACK;");
                Err(e)
            }
        }
    }

    fn browse(&self) -> BrowseResult {
        self.browse_checked().unwrap_or_else(|_| BrowseResult {
            accounts: vec![],
            projects: vec![],
            sessions: vec![],
            summary: HistoryBrowseSummary::default(),
        })
    }

    fn browse_checked(&self) -> Result<BrowseResult, CatalogReadError> {
        let conn = self
            .open_catalog_readonly()
            .ok_or(CatalogReadError::Unavailable)?;
        verify_catalog_connection(&conn).map_err(|_| CatalogReadError::Unavailable)?;
        // 全部项目（含 display_owner）
        let projects = read_all_browse_projects_checked(&conn, None)?;
        // 全部会话（排除软删除）
        let sessions = read_all_browse_sessions_checked(&conn, None)?;
        // 账号树
        let accounts = build_account_nodes_checked(&conn)?;
        // 摘要
        let summary = compute_history_summary_checked(&conn)?;

        Ok(BrowseResult {
            accounts,
            projects,
            sessions,
            summary,
        })
    }

    fn browse_projects_by_account(&self, user_id: &str) -> Vec<BrowseProjectNode> {
        let conn = match self.open_catalog_readonly() {
            Some(c) => c,
            None => return vec![],
        };
        read_all_browse_projects(&conn, Some(user_id))
    }

    fn browse_sessions_by_project(&self, project_id: &str) -> Vec<BrowseSessionNode> {
        let conn = match self.open_catalog_readonly() {
            Some(c) => c,
            None => return vec![],
        };
        read_all_browse_sessions(&conn, Some(project_id))
    }

    fn read_conversation_preview(&self, session: &SessionIdentity) -> Option<ConversationPreview> {
        self.read_conversation_preview_checked(session)
            .ok()
            .flatten()
    }

    fn read_conversation_preview_checked(
        &self,
        session: &SessionIdentity,
    ) -> Result<Option<ConversationPreview>, CatalogReadError> {
        let conn = self
            .open_catalog_readonly()
            .ok_or(CatalogReadError::Unavailable)?;
        verify_catalog_connection(&conn).map_err(|_| CatalogReadError::Unavailable)?;
        // 读取会话标题
        let title: Option<String> = conn
            .query_row(
                "SELECT active_title FROM session_projection \
                 WHERE namespace = ?1 AND original_session_id = ?2",
                rusqlite::params![
                    session.product_history_namespace,
                    session.original_session_id
                ],
                |row| row.get(0),
            )
            .optional()
            .map_err(|_| CatalogReadError::Unavailable)?;
        let Some(title) = title else {
            return Ok(None);
        };
        // 读取消息（排除软删除，保留底层行用于诊断）
        let messages = read_messages_for_session(
            &conn,
            &session.product_history_namespace,
            &session.original_session_id,
            true,
        )
        .map_err(|_| CatalogReadError::Unavailable)?;
        let total = messages.len() as u64;
        Ok(Some(ConversationPreview {
            session_identity: session.clone(),
            title,
            messages,
            total_message_count: total,
        }))
    }

    fn search_messages(&self, query: &str) -> Vec<SearchHit> {
        self.search_messages_checked(query).unwrap_or_default()
    }

    fn search_messages_checked(&self, query: &str) -> Result<Vec<SearchHit>, CatalogReadError> {
        self.search_messages_with_project_checked(query, None)
    }

    fn search_messages_in_project(&self, query: &str, project_id: &str) -> Vec<SearchHit> {
        self.search_messages_in_project_checked(query, project_id)
            .unwrap_or_default()
    }

    fn search_messages_in_project_checked(
        &self,
        query: &str,
        project_id: &str,
    ) -> Result<Vec<SearchHit>, CatalogReadError> {
        self.search_messages_with_project_checked(query, Some(project_id))
    }

    fn read_project_observation(&self, project_id: &str) -> Option<ProjectObservation> {
        let conn = self.open_catalog_readonly()?;
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
        self.read_all_project_observations_checked()
            .unwrap_or_default()
    }

    fn read_all_project_observations_checked(
        &self,
    ) -> Result<Vec<ProjectObservation>, CatalogReadError> {
        let conn = self
            .open_catalog_readonly()
            .ok_or(CatalogReadError::Unavailable)?;
        verify_catalog_connection(&conn).map_err(|_| CatalogReadError::Unavailable)?;
        let mut statement = conn
            .prepare("SELECT project_id FROM project_identity ORDER BY project_id ASC")
            .map_err(|_| CatalogReadError::Unavailable)?;
        let project_ids = statement
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(|_| CatalogReadError::Unavailable)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| CatalogReadError::Unavailable)?;
        project_ids
            .iter()
            .map(|project_id| {
                self.read_project_observation(project_id)
                    .ok_or(CatalogReadError::Unavailable)
            })
            .collect()
    }

    fn read_all_session_versions(&self) -> Vec<SessionVersion> {
        let conn = match self.open_catalog_readonly() {
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
        self.read_session_projection_checked(session).ok().flatten()
    }

    fn read_session_projection_checked(
        &self,
        session: &SessionIdentity,
    ) -> Result<Option<SessionProjection>, CatalogReadError> {
        let conn = self
            .open_catalog_readonly()
            .ok_or(CatalogReadError::Unavailable)?;
        verify_catalog_connection(&conn).map_err(|_| CatalogReadError::Unavailable)?;
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
        .optional()
        .map_err(|_| CatalogReadError::Unavailable)
    }

    fn assign_project_source(
        &self,
        assignment: &ProjectSourceAssignment,
    ) -> CatalogMutationOutcome {
        if reject_catalog_sidecars(&self.db_path).is_err() {
            return CatalogMutationOutcome::NotCommitted;
        }
        let mut conn = match self.open_existing_catalog() {
            Some(c) => c,
            None => return CatalogMutationOutcome::NotCommitted,
        };
        if verify_existing_catalog_write_protocol(&conn).is_err() {
            return CatalogMutationOutcome::NotCommitted;
        }
        // Gate E：仅写 project_source_assignment，不修改 project_observation 或快照
        let transaction = match conn.transaction() {
            Ok(transaction) => transaction,
            Err(_) => return CatalogMutationOutcome::NotCommitted,
        };
        if transaction
            .execute(
                "INSERT OR REPLACE INTO project_source_assignment \
                 (project_id, user_assigned_owner, assigned_at) VALUES (?1, ?2, ?3)",
                rusqlite::params![
                    assignment.project_id,
                    assignment.user_assigned_owner,
                    system_time_to_secs(assignment.assigned_at)
                ],
            )
            .is_err()
        {
            return CatalogMutationOutcome::NotCommitted;
        }
        if bump_catalog_content_revision(&transaction).is_err() || transaction.commit().is_err() {
            return CatalogMutationOutcome::NotCommitted;
        }
        drop(conn);
        if self.refresh_generation_metadata().is_ok() {
            CatalogMutationOutcome::Committed
        } else {
            CatalogMutationOutcome::CommittedMetadataRepairRequired
        }
    }

    fn read_project_source_assignment(&self, project_id: &str) -> Option<ProjectSourceAssignment> {
        let conn = self.open_catalog_readonly()?;
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
        let conn = match self.open_catalog_readonly() {
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
        // R9：visible_sessions 同时排除会话自身软删除和所属项目软删除
        let visible_sessions: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM session_projection sp \
                 JOIN project_identity pi ON pi.project_id = sp.project_id \
                 WHERE sp.soft_deleted = 0 AND pi.soft_deleted = 0",
                [],
                |row| row.get(0),
            )
            .unwrap_or(0);
        let retained_sessions: i64 = conn
            .query_row("SELECT COUNT(*) FROM session_projection", [], |row| {
                row.get(0)
            })
            .unwrap_or(0);
        // R9：visible_messages 同时排除消息自身软删除、所属会话软删除和所属项目软删除
        let visible_messages: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM message_projection mp \
                 JOIN session_projection sp \
                   ON sp.namespace = mp.namespace AND sp.original_session_id = mp.session_id \
                 JOIN project_identity pi ON pi.project_id = sp.project_id \
                 WHERE mp.soft_deleted = 0 AND sp.soft_deleted = 0 AND pi.soft_deleted = 0",
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
        self.history_summary_checked().unwrap_or_default()
    }

    fn history_summary_checked(&self) -> Result<HistoryBrowseSummary, CatalogReadError> {
        let conn = self
            .open_catalog_readonly()
            .ok_or(CatalogReadError::Unavailable)?;
        verify_catalog_connection(&conn).map_err(|_| CatalogReadError::Unavailable)?;
        compute_history_summary_checked(&conn)
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
        let old_messages =
            read_messages_for_session(conn, namespace, original_session_id, false).map_err(fail)?;

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
    read_all_browse_projects_checked(conn, filter_account).unwrap_or_default()
}

fn read_all_browse_projects_checked(
    conn: &Connection,
    filter_account: Option<&str>,
) -> Result<Vec<BrowseProjectNode>, CatalogReadError> {
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
    let mut stmt = conn
        .prepare(sql)
        .map_err(|_| CatalogReadError::Unavailable)?;
    let rows = match filter_account {
        Some(account) => stmt
            .query_map(rusqlite::params![account], map_project_row)
            .map_err(|_| CatalogReadError::Unavailable)?,
        None => stmt
            .query_map([], map_project_row)
            .map_err(|_| CatalogReadError::Unavailable)?,
    };
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|_| CatalogReadError::Unavailable)
}

/// 读取会话节点。filter_project 为 Some 时按 project_id 过滤，排除软删除。
///
/// R9：全局浏览（filter_project = None）时排除所属项目已软删除的会话。
/// 按项目浏览（filter_project = Some）时由 `browse_sessions_by_project` 调用方负责只传未软删除的项目，
/// 但为 defense in depth，这里仍 JOIN project_identity 排除软删除项目。
fn read_all_browse_sessions(
    conn: &Connection,
    filter_project: Option<&str>,
) -> Vec<BrowseSessionNode> {
    read_all_browse_sessions_checked(conn, filter_project).unwrap_or_default()
}

fn read_all_browse_sessions_checked(
    conn: &Connection,
    filter_project: Option<&str>,
) -> Result<Vec<BrowseSessionNode>, CatalogReadError> {
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
             JOIN project_identity pi ON pi.project_id = sp.project_id \
             WHERE sp.soft_deleted = 0 AND pi.soft_deleted = 0 AND sp.project_id = ?1 \
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
             JOIN project_identity pi ON pi.project_id = sp.project_id \
             WHERE sp.soft_deleted = 0 AND pi.soft_deleted = 0 \
             ORDER BY sp.active_title ASC"
        }
    };
    let mut stmt = conn
        .prepare(sql)
        .map_err(|_| CatalogReadError::Unavailable)?;
    let rows = match filter_project {
        Some(pid) => stmt
            .query_map(rusqlite::params![pid], map_session_row)
            .map_err(|_| CatalogReadError::Unavailable)?,
        None => stmt
            .query_map([], map_session_row)
            .map_err(|_| CatalogReadError::Unavailable)?,
    };
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|_| CatalogReadError::Unavailable)
}

fn build_account_nodes_checked(
    conn: &Connection,
) -> Result<Vec<BrowseAccountNode>, CatalogReadError> {
    let mut stmt = conn
        .prepare("SELECT user_id FROM seen_account ORDER BY user_id ASC")
        .map_err(|_| CatalogReadError::Unavailable)?;
    let user_ids = stmt
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(|_| CatalogReadError::Unavailable)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| CatalogReadError::Unavailable)?;
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
                .map_err(|_| CatalogReadError::Unavailable)?;
            // 统计这些项目下的可见会话数
            // R9：排除软删除会话 + 排除所属项目已软删除的会话
            let session_count: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM session_projection sp \
                     JOIN project_identity pi ON pi.project_id = sp.project_id \
                     WHERE sp.soft_deleted = 0 AND pi.soft_deleted = 0 AND \
                     COALESCE(\
                       (SELECT psa.user_assigned_owner FROM project_source_assignment psa \
                        WHERE psa.project_id = sp.project_id), \
                       (SELECT po.first_observed_owner FROM project_observation po \
                        WHERE po.project_id = sp.project_id)\
                     ) = ?1",
                    rusqlite::params![uid],
                    |row| row.get(0),
                )
                .map_err(|_| CatalogReadError::Unavailable)?;
            Ok(BrowseAccountNode {
                user_id: uid.clone(),
                display_label: uid.clone(),
                project_count: project_count as u64,
                session_count: session_count as u64,
            })
        })
        .collect()
}

fn query_count_checked(conn: &Connection, sql: &str) -> Result<u64, CatalogReadError> {
    let count: i64 = conn
        .query_row(sql, [], |row| row.get(0))
        .map_err(|_| CatalogReadError::Unavailable)?;
    u64::try_from(count).map_err(|_| CatalogReadError::Unavailable)
}

fn compute_history_summary_checked(
    conn: &Connection,
) -> Result<HistoryBrowseSummary, CatalogReadError> {
    let account_count = query_count_checked(conn, "SELECT COUNT(*) FROM seen_account")?;
    // R6：visible 排除 soft_deleted，soft_deleted 单独计数
    let visible_projects = query_count_checked(
        conn,
        "SELECT COUNT(*) FROM project_identity WHERE soft_deleted = 0",
    )?;
    let soft_deleted_projects = query_count_checked(
        conn,
        "SELECT COUNT(*) FROM project_identity WHERE soft_deleted = 1",
    )?;
    // R9：visible_sessions 同时排除会话自身软删除和所属项目软删除
    let visible_sessions = query_count_checked(
        conn,
        "SELECT COUNT(*) FROM session_projection sp \
         JOIN project_identity pi ON pi.project_id = sp.project_id \
         WHERE sp.soft_deleted = 0 AND pi.soft_deleted = 0",
    )?;
    let soft_deleted_sessions = query_count_checked(
        conn,
        "SELECT COUNT(*) FROM session_projection WHERE soft_deleted = 1",
    )?;
    let soft_deleted_messages = query_count_checked(
        conn,
        "SELECT COUNT(*) FROM message_projection WHERE soft_deleted = 1",
    )?;
    Ok(HistoryBrowseSummary {
        visible_account_count: account_count,
        visible_project_count: visible_projects,
        visible_session_count: visible_sessions,
        soft_deleted_project_count: soft_deleted_projects,
        soft_deleted_session_count: soft_deleted_sessions,
        soft_deleted_message_count: soft_deleted_messages,
    })
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

    fn with_catalog_test_lease<T>(
        storage_root: &Path,
        action: impl FnOnce(&Path, &OperationLease) -> T,
    ) -> T {
        let recovery_root = tempdir().expect("创建目录库测试租约根");
        let lease =
            OperationLease::acquire_bound(recovery_root.path(), storage_root, "catalog-test")
                .expect("取得目录库测试租约");
        action(recovery_root.path(), &lease)
    }

    /// 保持既有单元测试聚焦目录库语义；公开生产 API 仍强制显式传入租约。
    fn ensure_catalog_initialized(
        storage_root: &Path,
        raw_key: &str,
    ) -> Result<PathBuf, CatalogPathError> {
        with_catalog_test_lease(storage_root, |recovery_root, lease| {
            super::ensure_catalog_initialized(storage_root, raw_key, recovery_root, lease)
        })
    }

    fn catalog_storage_root(catalog_path: &Path) -> PathBuf {
        catalog_path
            .parent()
            .and_then(Path::parent)
            .and_then(Path::parent)
            .and_then(Path::parent)
            .expect("测试目录库路径应位于 storage/catalog/generations/generation")
            .to_path_buf()
    }

    fn initialize_catalog_identity(
        catalog_path: &Path,
        catalog_key: &str,
        catalog_id: &str,
        key_generation: u32,
    ) -> Result<(), CatalogPathError> {
        let storage_root = catalog_storage_root(catalog_path);
        with_catalog_test_lease(&storage_root, |recovery_root, lease| {
            super::initialize_catalog_identity(
                catalog_path,
                catalog_key,
                catalog_id,
                key_generation,
                recovery_root,
                lease,
            )
        })
    }

    fn verify_catalog_identity(
        catalog_path: &Path,
        catalog_key: &str,
        expected_catalog_id: &str,
        expected_key_generation: u32,
    ) -> Result<(), CatalogPathError> {
        let storage_root = catalog_storage_root(catalog_path);
        with_catalog_test_lease(&storage_root, |recovery_root, lease| {
            super::verify_catalog_identity(
                catalog_path,
                catalog_key,
                expected_catalog_id,
                expected_key_generation,
                recovery_root,
                lease,
            )
        })
    }

    fn catalog_path(dir: &Path) -> PathBuf {
        dir.join("catalog.db")
    }

    fn remove_catalog_sidecars(path: &Path) {
        let file_name = path.file_name().and_then(|name| name.to_str()).unwrap();
        for suffix in ["-wal", "-shm", "-journal"] {
            let _ = std::fs::remove_file(path.with_file_name(format!("{file_name}{suffix}")));
        }
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
    fn browse_checked_surfaces_catalog_open_failure() {
        let dir = tempdir().unwrap();
        let repo = SqlCipherCatalogRepository::new(
            dir.path().join("missing-catalog.db"),
            TEST_CATALOG_KEY.to_string(),
        );

        assert_eq!(repo.browse_checked(), Err(CatalogReadError::Unavailable));
    }

    #[test]
    fn catalog_layout_publishes_current_generation_after_initialization() {
        let dir = tempdir().unwrap();
        let catalog = ensure_catalog_initialized(dir.path(), TEST_CATALOG_KEY).unwrap();

        assert!(catalog.starts_with(dir.path().join("catalog").join("generations")));
        assert!(catalog.is_file());
        assert_eq!(resolve_current_catalog_path(dir.path()).unwrap(), catalog);
        assert!(dir.path().join("catalog").join("current.json").is_file());
    }

    #[test]
    fn generation_metadata_tracks_projection_after_scan() {
        let dir = tempdir().unwrap();
        let catalog_path = ensure_catalog_initialized(dir.path(), TEST_CATALOG_KEY).unwrap();
        let repo =
            SqlCipherCatalogRepository::new(catalog_path.clone(), TEST_CATALOG_KEY.to_string());
        let snapshot_dir = dir.path().join("snapshot");
        std::fs::create_dir_all(&snapshot_dir).unwrap();
        make_snapshot_fixture(&snapshot_dir, "user-A");

        let normalizer = WorkCnSourceNormalizer::new(TEST_CATALOG_KEY.to_string());
        repo.project_snapshot(&make_snapshot_meta(), &snapshot_dir, &normalizer)
            .expect("扫描投影应成功");

        let generation_dir = catalog_path.parent().expect("目录库应位于代次目录");
        let metadata: CatalogGenerationMetadata = serde_json::from_reader(
            std::fs::File::open(generation_dir.join("generation.json")).unwrap(),
        )
        .expect("generation.json 应可读取");
        let reopened_repo =
            SqlCipherCatalogRepository::new(catalog_path.clone(), TEST_CATALOG_KEY.to_string());
        let connection = reopened_repo.open_catalog().expect("重启后目录库应可打开");
        let actual_bytes = std::fs::metadata(&catalog_path).unwrap().len();

        assert_eq!(metadata.bytes, actual_bytes, "字节数必须与当前目录库一致");
        assert_eq!(
            metadata.catalog_sha256,
            sha256_file(&catalog_path).unwrap(),
            "哈希必须与当前目录库一致"
        );
        assert_eq!(
            metadata.semantic_counts,
            read_semantic_counts(&connection).unwrap(),
            "语义计数必须与当前目录库一致"
        );
    }

    #[test]
    fn generation_metadata_tracks_catalog_identity_initialization() {
        let dir = tempdir().unwrap();
        let catalog_path = ensure_catalog_initialized(dir.path(), TEST_CATALOG_KEY).unwrap();
        initialize_catalog_identity(&catalog_path, TEST_CATALOG_KEY, "catalog-test", 1)
            .expect("目录库身份初始化应成功");

        let generation_dir = catalog_path.parent().expect("目录库应位于代次目录");
        let metadata: CatalogGenerationMetadata = serde_json::from_reader(
            std::fs::File::open(generation_dir.join("generation.json")).unwrap(),
        )
        .expect("generation.json 应可读取");
        let repo = SqlCipherCatalogRepository::new(catalog_path.clone(), TEST_CATALOG_KEY.into());
        let connection = repo.open_catalog().expect("目录库应可打开");

        assert_eq!(
            metadata.bytes,
            std::fs::metadata(&catalog_path).unwrap().len()
        );
        assert_eq!(metadata.catalog_sha256, sha256_file(&catalog_path).unwrap());
        assert_eq!(
            metadata.semantic_counts,
            read_semantic_counts(&connection).unwrap()
        );
    }

    #[test]
    fn verify_catalog_identity_repairs_stale_generation_metadata() {
        let dir = tempdir().unwrap();
        let catalog_path = ensure_catalog_initialized(dir.path(), TEST_CATALOG_KEY).unwrap();
        initialize_catalog_identity(&catalog_path, TEST_CATALOG_KEY, "catalog-test", 1)
            .expect("目录库身份初始化应成功");
        let repo = SqlCipherCatalogRepository::new(catalog_path.clone(), TEST_CATALOG_KEY.into());
        let snapshot_dir = dir.path().join("snapshot");
        std::fs::create_dir_all(&snapshot_dir).unwrap();
        make_snapshot_fixture(&snapshot_dir, "user-A");
        let normalizer = WorkCnSourceNormalizer::new(TEST_CATALOG_KEY.to_string());
        repo.project_snapshot(&make_snapshot_meta(), &snapshot_dir, &normalizer)
            .expect("扫描投影应成功");

        let generation_dir = catalog_path.parent().expect("目录库应位于代次目录");
        let metadata_path = generation_dir.join("generation.json");
        let mut stale: CatalogGenerationMetadata =
            serde_json::from_reader(std::fs::File::open(&metadata_path).unwrap()).unwrap();
        stale.content_revision = stale.content_revision.saturating_sub(1);
        stale.bytes = 0;
        stale.catalog_sha256 = "stale".to_string();
        stale.semantic_counts.clear();
        std::fs::write(&metadata_path, serde_json::to_vec(&stale).unwrap()).unwrap();

        verify_catalog_identity(&catalog_path, TEST_CATALOG_KEY, "catalog-test", 1)
            .expect("重新打开目录库时应修复陈旧 sidecar");
        let repaired: CatalogGenerationMetadata =
            serde_json::from_reader(std::fs::File::open(metadata_path).unwrap()).unwrap();
        let connection = repo.open_catalog().expect("目录库应可打开");

        assert_eq!(
            repaired.bytes,
            std::fs::metadata(&catalog_path).unwrap().len()
        );
        assert_eq!(repaired.catalog_sha256, sha256_file(&catalog_path).unwrap());
        assert_eq!(
            repaired.semantic_counts,
            read_semantic_counts(&connection).unwrap()
        );
    }

    #[test]
    fn legacy_generation_metadata_without_revision_is_upgraded_to_current_shape() {
        let dir = tempdir().unwrap();
        let catalog_path = ensure_catalog_initialized(dir.path(), TEST_CATALOG_KEY).unwrap();
        let metadata_path = catalog_path
            .parent()
            .expect("目录库应位于代次目录")
            .join("generation.json");
        let current: serde_json::Value =
            serde_json::from_reader(std::fs::File::open(&metadata_path).unwrap()).unwrap();
        let legacy = serde_json::json!({
            "generation_id": current["generation_id"],
            "schema_version": current["schema_version"],
            "catalog_sha256": current["catalog_sha256"],
            "bytes": current["bytes"],
            "semantic_counts": current["semantic_counts"],
        });
        std::fs::write(&metadata_path, serde_json::to_vec_pretty(&legacy).unwrap()).unwrap();

        ensure_catalog_initialized(dir.path(), TEST_CATALOG_KEY)
            .expect("已知无 revision legacy sidecar 应可在启动协调时升级");
        let repaired: CatalogGenerationMetadata =
            serde_json::from_reader(std::fs::File::open(metadata_path).unwrap()).unwrap();
        assert_eq!(repaired.metadata_version, GENERATION_METADATA_VERSION);
        assert_eq!(
            repaired.package_format_version,
            GENERATION_PACKAGE_FORMAT_VERSION
        );
        assert_eq!(repaired.catalog_schema_version, CATALOG_SCHEMA_VERSION);
        assert_eq!(repaired.mapping_version, CATALOG_MAPPING_VERSION);
        assert_eq!(repaired.key_wrapper_version, CATALOG_KEY_WRAPPER_VERSION);
    }

    #[test]
    fn legacy_database_without_revision_upgrades_sidecar_without_writing_database() {
        let dir = tempdir().unwrap();
        let catalog_path = ensure_catalog_initialized(dir.path(), TEST_CATALOG_KEY).unwrap();
        let repo = SqlCipherCatalogRepository::new(catalog_path.clone(), TEST_CATALOG_KEY.into());
        let metadata_path = catalog_path
            .parent()
            .expect("目录库应位于代次目录")
            .join("generation.json");

        // 模拟旧目录库：移除旧版本不存在的 revision 键，并记录删除后的确切字节见证。
        let connection = repo
            .open_existing_catalog()
            .expect("旧目录库应可用无 CREATE 连接打开");
        connection
            .execute(
                "DELETE FROM catalog_meta WHERE key = ?1",
                [CONTENT_REVISION_KEY],
            )
            .unwrap();
        let semantic_counts = read_semantic_counts(&connection).unwrap();
        drop(connection);
        let database_before = std::fs::read(&catalog_path).unwrap();
        let current: serde_json::Value =
            serde_json::from_reader(std::fs::File::open(&metadata_path).unwrap()).unwrap();
        let legacy = serde_json::json!({
            "generation_id": current["generation_id"],
            "schema_version": current["schema_version"],
            "catalog_sha256": sha256_file(&catalog_path).unwrap(),
            "bytes": database_before.len() as u64,
            "semantic_counts": semantic_counts,
        });
        std::fs::write(&metadata_path, serde_json::to_vec_pretty(&legacy).unwrap()).unwrap();

        ensure_catalog_initialized(dir.path(), TEST_CATALOG_KEY)
            .expect("无 revision 旧目录库应只升级 sidecar");

        assert_eq!(
            std::fs::read(&catalog_path).unwrap(),
            database_before,
            "旧目录库启动协调不得写入 catalog.db"
        );
        let repaired: CatalogGenerationMetadata =
            serde_json::from_reader(std::fs::File::open(metadata_path).unwrap()).unwrap();
        assert_eq!(repaired.content_revision, 0);
        assert!(repaired.version_matrix_matches());
    }

    #[test]
    fn legacy_generation_metadata_with_revision_is_upgraded_to_current_shape() {
        let dir = tempdir().unwrap();
        let catalog_path = ensure_catalog_initialized(dir.path(), TEST_CATALOG_KEY).unwrap();
        initialize_catalog_identity(&catalog_path, TEST_CATALOG_KEY, "catalog-test", 1).unwrap();
        let metadata_path = catalog_path
            .parent()
            .expect("目录库应位于代次目录")
            .join("generation.json");
        let current: serde_json::Value =
            serde_json::from_reader(std::fs::File::open(&metadata_path).unwrap()).unwrap();
        let legacy = serde_json::json!({
            "generation_id": current["generation_id"],
            "schema_version": current["schema_version"],
            "catalog_sha256": current["catalog_sha256"],
            "bytes": current["bytes"],
            "semantic_counts": current["semantic_counts"],
            "content_revision": current["content_revision"],
        });
        std::fs::write(&metadata_path, serde_json::to_vec_pretty(&legacy).unwrap()).unwrap();

        verify_catalog_identity(&catalog_path, TEST_CATALOG_KEY, "catalog-test", 1)
            .expect("已知带 revision legacy sidecar 应可在启动协调时升级");
        let repaired: CatalogGenerationMetadata =
            serde_json::from_reader(std::fs::File::open(metadata_path).unwrap()).unwrap();
        assert_eq!(repaired.metadata_version, GENERATION_METADATA_VERSION);
        assert!(repaired.content_revision > 0);
        assert!(repaired.version_matrix_matches());
    }

    #[test]
    fn non_delete_journal_fails_closed_without_converting_database_or_sidecar() {
        let dir = tempdir().unwrap();
        let catalog_path = ensure_catalog_initialized(dir.path(), TEST_CATALOG_KEY).unwrap();
        initialize_catalog_identity(&catalog_path, TEST_CATALOG_KEY, "catalog-test", 1).unwrap();
        let repo = SqlCipherCatalogRepository::new(catalog_path.clone(), TEST_CATALOG_KEY.into());
        let metadata_path = catalog_path
            .parent()
            .expect("目录库应位于代次目录")
            .join("generation.json");

        let connection = repo
            .open_existing_catalog()
            .expect("目录库应可用无 CREATE 连接打开");
        let journal_mode: String = connection
            .query_row("PRAGMA journal_mode = WAL", [], |row| row.get(0))
            .unwrap();
        assert_eq!(journal_mode.to_ascii_lowercase(), "wal");
        drop(connection);
        // 仅为模拟“活动 WAL 已被外部清理但 header 仍为 WAL”的损坏现场；
        // 协调入口仍必须依据持久 journal_mode 拒绝，而不是切换回 DELETE。
        remove_catalog_sidecars(&catalog_path);
        for suffix in ["-wal", "-shm", "-journal"] {
            assert!(
                !catalog_path
                    .with_file_name(format!("catalog.db{suffix}"))
                    .exists(),
                "测试前应已清理 {suffix}"
            );
        }
        let database_before = std::fs::read(&catalog_path).unwrap();
        let sidecar_before = std::fs::read(&metadata_path).unwrap();

        let result = verify_catalog_identity(&catalog_path, TEST_CATALOG_KEY, "catalog-test", 1);
        assert!(result.is_err(), "WAL 目录库必须 fail-closed");
        assert_eq!(std::fs::read(&catalog_path).unwrap(), database_before);
        assert_eq!(std::fs::read(&metadata_path).unwrap(), sidecar_before);
        for suffix in ["-wal", "-shm", "-journal"] {
            assert!(
                !catalog_path
                    .with_file_name(format!("catalog.db{suffix}"))
                    .exists(),
                "协议拒绝不应创建 {suffix}"
            );
        }
    }

    #[test]
    fn sidecar_is_rejected_before_open_without_creating_missing_catalog() {
        let dir = tempdir().unwrap();
        let catalog_path = catalog_path(dir.path());
        std::fs::write(catalog_path.with_file_name("catalog.db-wal"), b"fixture").unwrap();
        let repo = SqlCipherCatalogRepository::new(catalog_path.clone(), TEST_CATALOG_KEY.into());

        assert_eq!(
            repo.browse_checked(),
            Err(CatalogReadError::Unavailable),
            "存在 sidecar 时必须在打开前 fail-closed"
        );
        assert!(!catalog_path.exists(), "拒绝 sidecar 不得创建空 catalog.db");
    }

    #[test]
    fn readonly_identity_reopen_does_not_create_missing_catalog() {
        let dir = tempdir().unwrap();
        let catalog_path = catalog_path(dir.path());

        assert!(
            verify_catalog_identity(&catalog_path, TEST_CATALOG_KEY, "catalog-test", 1).is_err()
        );
        assert!(
            !catalog_path.exists(),
            "READ_ONLY 重开不得 CREATE 缺失目录库"
        );
    }

    #[test]
    fn unknown_generation_metadata_field_fails_closed() {
        let dir = tempdir().unwrap();
        let catalog_path = ensure_catalog_initialized(dir.path(), TEST_CATALOG_KEY).unwrap();
        let metadata_path = catalog_path
            .parent()
            .expect("目录库应位于代次目录")
            .join("generation.json");
        let mut metadata: serde_json::Value =
            serde_json::from_reader(std::fs::File::open(&metadata_path).unwrap()).unwrap();
        metadata["unknown_field"] = serde_json::json!(true);
        std::fs::write(
            &metadata_path,
            serde_json::to_vec_pretty(&metadata).unwrap(),
        )
        .unwrap();

        assert_eq!(
            ensure_catalog_initialized(dir.path(), TEST_CATALOG_KEY),
            Err(CatalogPathError::Invalid),
            "未知 sidecar 字段不得被 legacy 分支吞掉"
        );
    }

    #[test]
    fn future_generation_schema_fails_closed_on_current_startup() {
        let dir = tempdir().unwrap();
        let catalog_path = ensure_catalog_initialized(dir.path(), TEST_CATALOG_KEY).unwrap();
        let metadata_path = catalog_path
            .parent()
            .expect("目录库应位于代次目录")
            .join("generation.json");
        let mut metadata: serde_json::Value =
            serde_json::from_reader(std::fs::File::open(&metadata_path).unwrap()).unwrap();
        metadata["schema_version"] = serde_json::json!(2);
        metadata["catalog_schema_version"] = serde_json::json!(2);
        std::fs::write(
            &metadata_path,
            serde_json::to_vec_pretty(&metadata).unwrap(),
        )
        .unwrap();

        assert_eq!(
            ensure_catalog_initialized(dir.path(), TEST_CATALOG_KEY),
            Err(CatalogPathError::Invalid),
            "当前 V1 启动不得接受未来目录库 schema"
        );
    }

    #[test]
    fn same_revision_generation_drift_fails_closed() {
        let dir = tempdir().unwrap();
        let catalog_path = ensure_catalog_initialized(dir.path(), TEST_CATALOG_KEY).unwrap();
        initialize_catalog_identity(&catalog_path, TEST_CATALOG_KEY, "catalog-test", 1)
            .expect("目录库身份初始化应成功");

        let metadata_path = catalog_path
            .parent()
            .expect("目录库应位于代次目录")
            .join("generation.json");
        let mut metadata: CatalogGenerationMetadata =
            serde_json::from_reader(std::fs::File::open(&metadata_path).unwrap()).unwrap();
        metadata.catalog_sha256 = "unexpected-drift".to_string();
        std::fs::write(&metadata_path, serde_json::to_vec(&metadata).unwrap()).unwrap();

        assert_eq!(
            verify_catalog_identity(&catalog_path, TEST_CATALOG_KEY, "catalog-test", 1),
            Err(CatalogPathError::Invalid),
            "同 revision 的未知漂移不得被自动覆盖"
        );
        let preserved: CatalogGenerationMetadata =
            serde_json::from_reader(std::fs::File::open(metadata_path).unwrap()).unwrap();
        assert_eq!(preserved.catalog_sha256, "unexpected-drift");
    }

    #[test]
    fn refresh_generation_metadata_preserves_upgrade_extensions() {
        let dir = tempdir().unwrap();
        let catalog_path = ensure_catalog_initialized(dir.path(), TEST_CATALOG_KEY).unwrap();
        initialize_catalog_identity(&catalog_path, TEST_CATALOG_KEY, "catalog-test", 1)
            .expect("目录库身份初始化应成功");

        let metadata_path = catalog_path
            .parent()
            .expect("目录库应位于代次目录")
            .join("generation.json");
        let mut metadata: serde_json::Value =
            serde_json::from_reader(std::fs::File::open(&metadata_path).unwrap())
                .expect("generation.json 应可读取");
        // 模拟目录库旁路升级写入的扩展元数据，刷新后不得丢失。
        let extensions = [
            ("metadata_version", serde_json::json!(1)),
            ("package_format_version", serde_json::json!(1)),
            ("catalog_schema_version", serde_json::json!(1)),
            ("mapping_version", serde_json::json!("catalog-v1")),
            ("key_wrapper_version", serde_json::json!(1)),
        ];
        let object = metadata.as_object_mut().expect("generation.json 应为对象");
        // 让刷新真正重写 sidecar，验证扩展字段会跨序列化保留。
        object.insert("content_revision".to_string(), serde_json::json!(0));
        for (key, value) in &extensions {
            object.insert((*key).to_string(), value.clone());
        }
        std::fs::write(&metadata_path, serde_json::to_vec(&metadata).unwrap()).unwrap();

        verify_catalog_identity(&catalog_path, TEST_CATALOG_KEY, "catalog-test", 1)
            .expect("刷新 sidecar 应成功");
        let repaired: serde_json::Value =
            serde_json::from_reader(std::fs::File::open(metadata_path).unwrap())
                .expect("修复后的 generation.json 应可读取");
        for (key, expected) in extensions {
            assert_eq!(repaired[key], expected, "扩展字段 {key} 不得被刷新丢失");
        }
    }

    #[test]
    fn missing_catalog_pointer_does_not_create_empty_database() {
        let dir = tempdir().unwrap();

        assert_eq!(
            resolve_current_catalog_path(dir.path()),
            Err(CatalogPathError::Missing)
        );
        assert!(!dir.path().join("catalog.db").exists());
    }

    #[test]
    fn malformed_catalog_pointer_fails_closed_without_replacing_it() {
        let dir = tempdir().unwrap();
        let catalog_root = dir.path().join("catalog");
        std::fs::create_dir_all(&catalog_root).unwrap();
        std::fs::write(
            catalog_root.join("current.json"),
            b"{\"generation_id\":\"missing\"}",
        )
        .unwrap();

        assert_eq!(
            resolve_current_catalog_path(dir.path()),
            Err(CatalogPathError::Invalid)
        );
        assert_eq!(
            std::fs::read(catalog_root.join("current.json")).unwrap(),
            b"{\"generation_id\":\"missing\"}"
        );
        assert!(!catalog_root.join("catalog.db").exists());
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
        let conn = repo.open_catalog().unwrap();
        let user_version: u32 = conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(user_version, 1);
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

        let project_hits = repo.search_messages_in_project("hello", "p1");
        assert!(project_hits.iter().any(|h| h.message_id == "m1"));
        assert!(repo
            .search_messages_in_project("hello", "project-not-found")
            .is_empty());

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

        let catalog_path = ensure_catalog_initialized(dir.path(), TEST_CATALOG_KEY).unwrap();
        let repo = SqlCipherCatalogRepository::new(catalog_path, TEST_CATALOG_KEY.to_string());
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
        assert_eq!(
            repo.assign_project_source(&assignment),
            CatalogMutationOutcome::Committed,
            "分配应成功"
        );

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

        // 来源归类也是目录库写入口，完成后 sidecar 必须跟随当前 revision。
        let generation_dir = repo.db_path.parent().expect("目录库应位于代次目录");
        let metadata: CatalogGenerationMetadata = serde_json::from_reader(
            std::fs::File::open(generation_dir.join("generation.json")).unwrap(),
        )
        .unwrap();
        let connection = repo.open_catalog().unwrap();
        assert_eq!(
            metadata.content_revision,
            read_content_revision(&connection).unwrap().unwrap()
        );
        assert_eq!(
            metadata.semantic_counts,
            read_semantic_counts(&connection).unwrap()
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
        // 空投影表示源数据库未成功读取，必须失败且不得污染既有数据。
        assert_eq!(
            result,
            Err(ScanFailureReason::CatalogTransactionFailed),
            "空投影必须 fail-closed"
        );

        // 原有数据完好
        let browse = repo.browse();
        assert_eq!(browse.projects.len(), 1, "原有项目应完好");
        assert_eq!(browse.sessions.len(), 1, "原有会话应完好");
    }

    #[test]
    fn partial_normalizer_read_fails_without_new_projection() {
        let dir = tempdir().unwrap();
        let existing_snapshot = dir.path().join("existing");
        std::fs::create_dir_all(&existing_snapshot).unwrap();
        make_snapshot_fixture(&existing_snapshot, "user-A");

        let repo = setup_repo(dir.path());
        let normalizer = WorkCnSourceNormalizer::new(TEST_CATALOG_KEY.to_string());
        repo.project_snapshot(&make_snapshot_meta(), &existing_snapshot, &normalizer)
            .unwrap();
        assert_eq!(repo.browse().projects.len(), 1);

        // 只保留 project 表，缺失 chat_session/chat_message；checked 读取必须失败。
        let malformed_snapshot = dir.path().join("malformed");
        std::fs::create_dir_all(&malformed_snapshot).unwrap();
        Connection::open(malformed_snapshot.join("database.db"))
            .unwrap()
            .execute_batch(
                "CREATE TABLE project (project_id TEXT, user_id TEXT, biz_project_id TEXT, deleted_at INTEGER); \
                 INSERT INTO project VALUES ('p2', 'user-B', 'biz-2', 0);",
            )
            .unwrap();

        assert_eq!(
            repo.project_snapshot(&make_snapshot_meta(), &malformed_snapshot, &normalizer),
            Err(ScanFailureReason::CatalogTransactionFailed)
        );
        let browse = repo.browse();
        assert_eq!(browse.projects.len(), 1, "部分读取失败不得新增项目投影");
        assert_eq!(browse.projects[0].project_id, "p1");
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
        // 五次扫描：A→B→A→B→A，first_observed_owner 始终为 A
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

        // 第四次：owner 再次切到 user-B
        let snap4 = dir.path().join("snap-4");
        std::fs::create_dir_all(&snap4).unwrap();
        make_snapshot_fixture(&snap4, "user-B");
        repo.project_snapshot(&make_snapshot_meta(), &snap4, &normalizer)
            .unwrap();

        // 第五次：owner 最终回到 user-A
        let snap5 = dir.path().join("snap-5");
        std::fs::create_dir_all(&snap5).unwrap();
        make_snapshot_fixture(&snap5, "user-A");
        repo.project_snapshot(&make_snapshot_meta(), &snap5, &normalizer)
            .unwrap();

        // Gate E：first_observed_owner 始终为 user-A
        let obs = repo.read_project_observation("p1").expect("应有观察记录");
        assert_eq!(
            obs.first_observed_owner, "user-A",
            "first_observed_owner 应保持 user-A（A→B→A→B→A 后仍不变）"
        );
        assert_eq!(
            obs.current_live_owner, "user-A",
            "current_live_owner 应为最后一次的 user-A"
        );
        // 五次扫描应追加 5 条 owner 观察
        assert_eq!(
            obs.owner_observations.len(),
            5,
            "应有 5 条 owner 观察（A→B→A→B→A）"
        );
        let owner_ids: Vec<&str> = obs
            .owner_observations
            .iter()
            .map(|observation| observation.owner_user_id.as_str())
            .collect();
        assert_eq!(
            owner_ids.iter().filter(|owner| **owner == "user-A").count(),
            3,
            "A 应出现 3 次"
        );
        assert_eq!(
            owner_ids.iter().filter(|owner| **owner == "user-B").count(),
            2,
            "B 应出现 2 次"
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

    // ============== R9：软删除项目的子会话与消息从 browse/search/count 排除 ==============

    #[test]
    fn r9_soft_deleted_project_sessions_excluded_from_browse() {
        // 反例：软删除项目 p2 的会话 s2 不应出现在 browse.sessions
        let dir = tempdir().unwrap();
        let snapshot_dir = dir.path().join("snap-1");
        std::fs::create_dir_all(&snapshot_dir).unwrap();
        make_snapshot_fixture_with_soft_deleted_project(&snapshot_dir, "user-A");

        let repo = setup_repo(dir.path());
        let normalizer = WorkCnSourceNormalizer::new(TEST_CATALOG_KEY.to_string());
        repo.project_snapshot(&make_snapshot_meta(), &snapshot_dir, &normalizer)
            .unwrap();

        let browse = repo.browse();
        // browse.sessions 应只包含 s1（p1 的会话），不含 s2（p2 的会话）
        let session_ids: Vec<&str> = browse
            .sessions
            .iter()
            .map(|s| s.session_identity.original_session_id.as_str())
            .collect();
        assert!(
            session_ids.contains(&"s1"),
            "s1（p1 的会话）应出现在 browse.sessions"
        );
        assert!(
            !session_ids.contains(&"s2"),
            "s2（软删除项目 p2 的会话）不应出现在 browse.sessions"
        );
    }

    #[test]
    fn r9_soft_deleted_project_messages_excluded_from_search() {
        // 反例：搜索命中不包含软删除项目 p2 下的消息 m2
        let dir = tempdir().unwrap();
        let snapshot_dir = dir.path().join("snap-1");
        std::fs::create_dir_all(&snapshot_dir).unwrap();
        make_snapshot_fixture_with_soft_deleted_project(&snapshot_dir, "user-A");

        let repo = setup_repo(dir.path());
        let normalizer = WorkCnSourceNormalizer::new(TEST_CATALOG_KEY.to_string());
        repo.project_snapshot(&make_snapshot_meta(), &snapshot_dir, &normalizer)
            .unwrap();

        // 搜索一个会同时匹配 m1 和 m2 的关键词
        let hits = repo.search_messages("hello");
        let hit_ids: Vec<&str> = hits.iter().map(|h| h.message_id.as_str()).collect();
        assert!(hit_ids.contains(&"m1"), "m1（p1 的消息）应被搜索命中");
        assert!(
            !hit_ids.contains(&"m2"),
            "m2（软删除项目 p2 的消息）不应被搜索命中"
        );

        // 搜索 m2 独有的内容，应返回空
        let hits2 = repo.search_messages("soft-deleted-project-msg");
        assert!(hits2.is_empty(), "搜索软删除项目专属内容应返回空");
    }

    #[test]
    fn r9_soft_deleted_project_session_excluded_from_counts() {
        // 反例：visible_session_count 不计入软删除项目 p2 下的会话
        let dir = tempdir().unwrap();
        let snapshot_dir = dir.path().join("snap-1");
        std::fs::create_dir_all(&snapshot_dir).unwrap();
        make_snapshot_fixture_with_soft_deleted_project(&snapshot_dir, "user-A");

        let repo = setup_repo(dir.path());
        let normalizer = WorkCnSourceNormalizer::new(TEST_CATALOG_KEY.to_string());
        repo.project_snapshot(&make_snapshot_meta(), &snapshot_dir, &normalizer)
            .unwrap();

        let summary = repo.history_summary();
        // visible_session_count 应为 1（仅 s1），不含 s2
        assert_eq!(
            summary.visible_session_count, 1,
            "visible_session_count 不应计入软删除项目下的会话"
        );

        let browse = repo.browse();
        assert_eq!(
            browse.summary.visible_session_count, 1,
            "browse.summary.visible_session_count 不应计入软删除项目下的会话"
        );

        // 账号 user-B 的 session_count 应为 0（p2 软删除）
        let user_b = browse.accounts.iter().find(|a| a.user_id == "user-B");
        if let Some(acc) = user_b {
            assert_eq!(
                acc.session_count, 0,
                "user-B 的 session_count 不应计入软删除项目 p2 下的会话"
            );
        }
    }

    #[test]
    fn r9_soft_deleted_project_retained_in_diagnostic_counts() {
        // 反例：diagnostic retained counts 仍包含软删除项目下的会话与消息
        let dir = tempdir().unwrap();
        let snapshot_dir = dir.path().join("snap-1");
        std::fs::create_dir_all(&snapshot_dir).unwrap();
        make_snapshot_fixture_with_soft_deleted_project(&snapshot_dir, "user-A");

        let repo = setup_repo(dir.path());
        let normalizer = WorkCnSourceNormalizer::new(TEST_CATALOG_KEY.to_string());
        repo.project_snapshot(&make_snapshot_meta(), &snapshot_dir, &normalizer)
            .unwrap();

        let diag = repo.diagnostic_integrity();
        // retained 应包含全部（含软删除项目的会话和消息）
        assert_eq!(diag.retained_projects, 2, "retained_projects 含 p1+p2");
        assert_eq!(diag.retained_sessions, 2, "retained_sessions 含 s1+s2");
        assert_eq!(diag.retained_messages, 2, "retained_messages 含 m1+m2");
        // visible 排除软删除项目及其子会话
        assert_eq!(diag.visible_projects, 1, "visible_projects 仅 p1");
        assert_eq!(diag.visible_sessions, 1, "visible_sessions 仅 s1");
        assert_eq!(diag.visible_messages, 1, "visible_messages 仅 m1");
    }
}
