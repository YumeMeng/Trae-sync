//! T13 密码恢复包与目录库密钥旁路升级基础能力。
//!
//! 恢复包只携带加密后的目录库密钥材料和非敏感元数据。错误密码、损坏包或身份不匹配
//! 在返回前不会创建或覆盖任何目标文件。

use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use openssl::hash::MessageDigest;
use openssl::pkcs5;
use openssl::rand::rand_bytes;
use openssl::symm::{Cipher, Crypter, Mode};
use rusqlite::{Connection, OpenFlags};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use traesync_ports::{
    RecoveryImportError, RecoveryImportRequest, RecoveryPackageImportPort, VerifiedRecoveryMaterial,
};

use crate::catalog::{
    configure_new_catalog_write_protocol, reject_catalog_sidecars, verify_catalog_read_protocol,
    verify_existing_catalog_write_protocol, CatalogGenerationMetadata,
};
use crate::migration_manifest::{
    freeze_unfinished, MigrationJournal, MigrationKind, MigrationStage,
};
use crate::operation_lease::OperationLease;

const MAGIC: &[u8; 8] = b"TRSREC01";
const FORMAT_VERSION: u8 = 1;
const KDF_ITERATIONS: usize = 600_000;
const SALT_LEN: usize = 16;
const NONCE_LEN: usize = 12;
const TAG_LEN: usize = 16;
const RECOVERY_METADATA_VERSION: u32 = 1;
const KDF_VERSION: u32 = 1;
const CIPHER_VERSION: &str = "aes-256-gcm-v1";
const CATALOG_MAPPING_VERSION: &str = "catalog-v1";
const KEY_WRAPPER_VERSION: u32 = 1;
// 恢复包只包含少量密钥材料和元数据，超过此上限的输入不应进入解密内存。
const MAX_RECOVERY_PACKAGE_BYTES: u64 = 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecoveryPayload {
    pub catalog_id: String,
    pub key_generation: u32,
    pub catalog_key_hex: String,
}

/// 恢复包和旁路目录库共用的版本矩阵，未知 schema 仍由调用方显式提供。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecoveryPackageMetadata {
    pub metadata_version: u32,
    pub package_format_version: u32,
    pub kdf_version: u32,
    pub cipher_version: String,
    pub catalog_schema_version: u32,
    pub mapping_version: String,
    pub key_wrapper_version: u32,
}

impl RecoveryPackageMetadata {
    fn current() -> Self {
        Self {
            metadata_version: RECOVERY_METADATA_VERSION,
            package_format_version: FORMAT_VERSION as u32,
            kdf_version: KDF_VERSION,
            cipher_version: CIPHER_VERSION.to_string(),
            // 恢复包只保存密钥材料，不携带目录库正文，因此 schema 由导入请求提供。
            catalog_schema_version: 0,
            mapping_version: CATALOG_MAPPING_VERSION.to_string(),
            key_wrapper_version: KEY_WRAPPER_VERSION,
        }
    }

    fn legacy() -> Self {
        Self {
            metadata_version: 0,
            package_format_version: FORMAT_VERSION as u32,
            kdf_version: KDF_VERSION,
            cipher_version: CIPHER_VERSION.to_string(),
            catalog_schema_version: 0,
            mapping_version: CATALOG_MAPPING_VERSION.to_string(),
            key_wrapper_version: KEY_WRAPPER_VERSION,
        }
    }

    fn is_supported(&self) -> bool {
        matches!(self.metadata_version, 0 | RECOVERY_METADATA_VERSION)
            && self.package_format_version == FORMAT_VERSION as u32
            && self.kdf_version == KDF_VERSION
            && self.cipher_version == CIPHER_VERSION
            && self.mapping_version == CATALOG_MAPPING_VERSION
            && self.key_wrapper_version == KEY_WRAPPER_VERSION
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct RecoveryEnvelope {
    metadata: RecoveryPackageMetadata,
    payload: RecoveryPayload,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum RecoveryDocument {
    Envelope(RecoveryEnvelope),
    Legacy(RecoveryPayload),
}

/// 已通过包认证和目录库只读验证的结果；不包含可变写入句柄。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedRecoveryImport {
    pub payload: RecoveryPayload,
    pub metadata: RecoveryPackageMetadata,
    pub schema_version: u32,
    pub catalog_sha256: String,
    pub bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecoveryPackageError {
    EmptyPassphrase,
    InvalidCatalogKey,
    InvalidPackage,
    WrongPassphrase,
    IdentityMismatch,
    /// 目标卷无法满足升级所需空间预算；不能伪装成普通校验失败。
    InsufficientSpace,
    Io,
    Crypto,
    PayloadInvalid,
    /// 恢复包元数据不在当前支持的版本矩阵内。
    UnsupportedMetadata,
    VerificationFailed,
    MigrationManifestUnavailable,
    MigrationRecoveryRequired,
}

impl std::fmt::Display for RecoveryPackageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let message = match self {
            Self::EmptyPassphrase => "恢复密码不能为空",
            Self::InvalidCatalogKey => "目录库密钥格式无效",
            Self::InvalidPackage => "恢复包格式无效",
            Self::WrongPassphrase => "恢复密码错误或恢复包认证失败",
            Self::IdentityMismatch => "恢复包目录库身份不匹配",
            Self::InsufficientSpace => "升级所需可用空间不足",
            Self::Io => "恢复包读写失败",
            Self::Crypto => "恢复包加密操作失败",
            Self::PayloadInvalid => "恢复包内容无效",
            Self::UnsupportedMetadata => "恢复包版本矩阵不兼容",
            Self::VerificationFailed => "恢复包或目录库逐文件验证失败",
            Self::MigrationManifestUnavailable => "升级恢复记录不可用",
            Self::MigrationRecoveryRequired => "存在未完成目录库升级，需要人工恢复",
        };
        f.write_str(message)
    }
}

impl std::error::Error for RecoveryPackageError {}

pub fn export_recovery_package(
    destination: &Path,
    passphrase: &str,
    payload: &RecoveryPayload,
) -> Result<(), RecoveryPackageError> {
    validate_payload(payload)?;
    if passphrase.is_empty() {
        return Err(RecoveryPackageError::EmptyPassphrase);
    }

    let mut salt = [0_u8; SALT_LEN];
    let mut nonce = [0_u8; NONCE_LEN];
    rand_bytes(&mut salt).map_err(|_| RecoveryPackageError::Crypto)?;
    rand_bytes(&mut nonce).map_err(|_| RecoveryPackageError::Crypto)?;
    let key = derive_key(passphrase, &salt)?;
    let plaintext = serde_json::to_vec(&RecoveryEnvelope {
        metadata: RecoveryPackageMetadata::current(),
        payload: payload.clone(),
    })
    .map_err(|_| RecoveryPackageError::PayloadInvalid)?;
    let (ciphertext, tag) = encrypt(&key, &nonce, &plaintext)?;

    let parent = destination
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    reject_symlink_directory(parent)?;
    fs::create_dir_all(parent).map_err(|_| RecoveryPackageError::Io)?;
    if fs::symlink_metadata(destination).is_ok() {
        return Err(RecoveryPackageError::Io);
    }
    let temporary = temporary_path(destination, "export");
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)
        .map_err(|_| RecoveryPackageError::Io)?;
    let write_result = (|| {
        file.write_all(MAGIC)?;
        file.write_all(&[FORMAT_VERSION])?;
        file.write_all(&(KDF_ITERATIONS as u32).to_le_bytes())?;
        file.write_all(&salt)?;
        file.write_all(&nonce)?;
        file.write_all(&tag)?;
        file.write_all(&(ciphertext.len() as u32).to_le_bytes())?;
        file.write_all(&ciphertext)?;
        file.sync_all()
    })();
    drop(file);
    if write_result.is_err() {
        let _ = fs::remove_file(&temporary);
        return Err(RecoveryPackageError::Io);
    }
    publish_new_file(&temporary, destination)?;
    sync_directory(parent)
}

pub fn import_recovery_package(
    package: &Path,
    passphrase: &str,
    expected_catalog_id: &str,
    expected_key_generation: u32,
) -> Result<RecoveryPayload, RecoveryPackageError> {
    let (payload, _) = read_authenticated_package(package, passphrase)?;
    if payload.catalog_id != expected_catalog_id
        || payload.key_generation != expected_key_generation
    {
        return Err(RecoveryPackageError::IdentityMismatch);
    }
    Ok(payload)
}

/// 导入成功前只读打开目录库，并完成完整性、schema、身份和密钥代次验证。
pub fn import_recovery_package_verified(
    package: &Path,
    passphrase: &str,
    catalog_path: &Path,
    expected_catalog_id: &str,
    expected_key_generation: u32,
    expected_schema_version: u32,
) -> Result<VerifiedRecoveryImport, RecoveryPackageError> {
    let (payload, metadata) = read_authenticated_package(package, passphrase)?;
    if payload.catalog_id != expected_catalog_id
        || payload.key_generation != expected_key_generation
    {
        return Err(RecoveryPackageError::IdentityMismatch);
    }
    let (schema_version, catalog_sha256, bytes) =
        verify_recovery_catalog(catalog_path, &payload, expected_schema_version)?;
    Ok(VerifiedRecoveryImport {
        payload,
        metadata,
        schema_version,
        catalog_sha256,
        bytes,
    })
}

/// infrastructure 适配器：应用层只看端口，不直接依赖 SQLCipher 细节。
#[derive(Debug, Clone, Copy, Default)]
pub struct RecoveryPackageImporter;

impl RecoveryPackageImporter {
    pub const fn new() -> Self {
        Self
    }
}

impl RecoveryPackageImportPort for RecoveryPackageImporter {
    fn import_and_verify(
        &self,
        request: &RecoveryImportRequest<'_>,
    ) -> Result<VerifiedRecoveryMaterial, RecoveryImportError> {
        let verified = import_recovery_package_verified(
            request.package_path,
            request.passphrase,
            request.catalog_path,
            request.expected_catalog_id,
            request.expected_key_generation,
            request.expected_schema_version,
        )
        .map_err(map_import_error)?;
        Ok(VerifiedRecoveryMaterial::new(
            verified.payload.catalog_id,
            verified.payload.key_generation,
            verified.schema_version,
            verified.payload.catalog_key_hex,
            verified.metadata.key_wrapper_version,
        ))
    }
}

fn map_import_error(error: RecoveryPackageError) -> RecoveryImportError {
    match error {
        RecoveryPackageError::EmptyPassphrase => RecoveryImportError::EmptyPassphrase,
        RecoveryPackageError::WrongPassphrase => RecoveryImportError::WrongPassphrase,
        RecoveryPackageError::InvalidPackage
        | RecoveryPackageError::InvalidCatalogKey
        | RecoveryPackageError::PayloadInvalid
        | RecoveryPackageError::Crypto => RecoveryImportError::InvalidPackage,
        RecoveryPackageError::IdentityMismatch => RecoveryImportError::IdentityMismatch,
        RecoveryPackageError::InsufficientSpace => RecoveryImportError::InsufficientSpace,
        RecoveryPackageError::UnsupportedMetadata => RecoveryImportError::UnsupportedMetadata,
        RecoveryPackageError::VerificationFailed => RecoveryImportError::CatalogVerificationFailed,
        RecoveryPackageError::Io
        | RecoveryPackageError::MigrationManifestUnavailable
        | RecoveryPackageError::MigrationRecoveryRequired => RecoveryImportError::Io,
    }
}

fn read_authenticated_package(
    package: &Path,
    passphrase: &str,
) -> Result<(RecoveryPayload, RecoveryPackageMetadata), RecoveryPackageError> {
    if passphrase.is_empty() {
        return Err(RecoveryPackageError::EmptyPassphrase);
    }
    let metadata = fs::symlink_metadata(package).map_err(|_| RecoveryPackageError::Io)?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(RecoveryPackageError::Io);
    }
    if metadata.len() > MAX_RECOVERY_PACKAGE_BYTES {
        return Err(RecoveryPackageError::InvalidPackage);
    }
    let file = File::open(package).map_err(|_| RecoveryPackageError::Io)?;
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    // 限制读取上界；即使文件在打开后被替换或增长，也不会无界扩容。
    file.take(MAX_RECOVERY_PACKAGE_BYTES.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|_| RecoveryPackageError::Io)?;
    if bytes.len() as u64 != metadata.len() || bytes.len() as u64 > MAX_RECOVERY_PACKAGE_BYTES {
        return Err(RecoveryPackageError::InvalidPackage);
    }
    let (iterations, salt, nonce, tag, ciphertext) = parse_package(&bytes)?;
    let key = derive_key_with_iterations(passphrase, &salt, iterations)?;
    let plaintext = decrypt(&key, &nonce, &tag, ciphertext)
        .map_err(|_| RecoveryPackageError::WrongPassphrase)?;
    let document: RecoveryDocument =
        serde_json::from_slice(&plaintext).map_err(|_| RecoveryPackageError::PayloadInvalid)?;
    let (metadata, payload) = match document {
        RecoveryDocument::Envelope(envelope) => (envelope.metadata, envelope.payload),
        RecoveryDocument::Legacy(payload) => (RecoveryPackageMetadata::legacy(), payload),
    };
    if !metadata.is_supported() {
        return Err(RecoveryPackageError::UnsupportedMetadata);
    }
    validate_payload(&payload)?;
    Ok((payload, metadata))
}

fn verify_recovery_catalog(
    catalog_path: &Path,
    payload: &RecoveryPayload,
    expected_schema_version: u32,
) -> Result<(u32, String, u64), RecoveryPackageError> {
    // 这里必须保持 SQLITE_OPEN_READ_ONLY；包装器调用前不能打开写连接或创建空库。
    let connection = open_catalog_readonly(catalog_path, &payload.catalog_key_hex)?;
    verify_sqlite_integrity(&connection)?;
    let schema_version = read_catalog_schema_version(&connection)?;
    if schema_version != expected_schema_version {
        return Err(RecoveryPackageError::VerificationFailed);
    }

    let catalog_id = read_catalog_meta_value(&connection, "catalog_id")?;
    let key_generation = read_catalog_meta_value(&connection, "key_generation")?
        .parse::<u32>()
        .map_err(|_| RecoveryPackageError::VerificationFailed)?;
    if catalog_id != payload.catalog_id || key_generation != payload.key_generation {
        return Err(RecoveryPackageError::IdentityMismatch);
    }

    let bytes = fs::metadata(catalog_path)
        .map_err(|_| RecoveryPackageError::VerificationFailed)?
        .len();
    let catalog_sha256 = hash_file(catalog_path)?;
    Ok((schema_version, catalog_sha256, bytes))
}

fn read_catalog_meta_value(
    connection: &Connection,
    key: &str,
) -> Result<String, RecoveryPackageError> {
    connection
        .query_row(
            "SELECT value FROM catalog_meta WHERE key = ?1",
            [key],
            |row| row.get(0),
        )
        .map_err(|_| RecoveryPackageError::VerificationFailed)
}

fn map_space_reservation_error(
    error: crate::storage_root::StorageRootError,
) -> RecoveryPackageError {
    match error {
        crate::storage_root::StorageRootError::InsufficientSpace => {
            RecoveryPackageError::InsufficientSpace
        }
        _ => RecoveryPackageError::Io,
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CatalogUpgradeResult {
    pub generation_id: String,
    pub schema_version: u32,
    pub catalog_sha256: String,
    pub bytes: u64,
}

#[cfg(test)]
fn upgrade_catalog_sidecar(
    source_catalog: &Path,
    destination_root: &Path,
    recovery_root: &Path,
    raw_key: &str,
    source_schema_version: u32,
    target_schema_version: u32,
) -> Result<CatalogUpgradeResult, RecoveryPackageError> {
    // 测试辅助入口模拟生产启动后已准备好的固定恢复区；生产入口仍要求调用方先准备恢复区。
    fs::create_dir_all(recovery_root)
        .map_err(|_| RecoveryPackageError::MigrationManifestUnavailable)?;
    let lease = OperationLease::acquire(recovery_root, "catalog-upgrade")
        .map_err(|_| RecoveryPackageError::MigrationManifestUnavailable)?;
    upgrade_catalog_sidecar_with_lease(
        source_catalog,
        destination_root,
        &lease,
        raw_key,
        source_schema_version,
        target_schema_version,
    )
}

#[cfg(test)]
fn upgrade_catalog_sidecar_with_recovery_root(
    source_catalog: &Path,
    destination_root: &Path,
    recovery_root: &Path,
    raw_key: &str,
    source_schema_version: u32,
    target_schema_version: u32,
) -> Result<CatalogUpgradeResult, RecoveryPackageError> {
    upgrade_catalog_sidecar(
        source_catalog,
        destination_root,
        recovery_root,
        raw_key,
        source_schema_version,
        target_schema_version,
    )
}

/// 使用已持有的共享租约执行目录库旁路升级；恢复区不依赖新代目录库。
pub fn upgrade_catalog_sidecar_with_lease(
    source_catalog: &Path,
    destination_root: &Path,
    lease: &OperationLease,
    raw_key: &str,
    source_schema_version: u32,
    target_schema_version: u32,
) -> Result<CatalogUpgradeResult, RecoveryPackageError> {
    if is_symlink(source_catalog)
        || !source_catalog.is_file()
        || target_schema_version != source_schema_version.saturating_add(1)
    {
        return Err(RecoveryPackageError::InvalidPackage);
    }
    reject_catalog_sidecars(source_catalog)
        .map_err(|_| RecoveryPackageError::VerificationFailed)?;
    reject_symlink_directory(destination_root)?;
    validate_raw_key(raw_key)?;
    let source_hash = hash_file(source_catalog)?;
    let source_bytes = fs::metadata(source_catalog)
        .map_err(|_| RecoveryPackageError::Io)?
        .len();
    let source_semantic_counts =
        read_catalog_semantic_counts(source_catalog, raw_key, source_schema_version)?;
    match freeze_unfinished(lease.recovery_root(), MigrationKind::CatalogUpgrade) {
        Ok(true) => return Err(RecoveryPackageError::MigrationRecoveryRequired),
        Ok(false) => {}
        Err(_) => return Err(RecoveryPackageError::MigrationManifestUnavailable),
    }
    // 在创建目标目录或 staging 之前预留实际空间，失败时不产生目标副作用。
    let destination_parent = destination_root
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    reject_symlink_directory(destination_parent)?;
    let _space_reservation = crate::storage_root::reserve_space(
        destination_parent,
        source_bytes.saturating_add(crate::storage_root::required_storage_reserve_bytes(
            source_bytes,
        )),
    )
    .map_err(map_space_reservation_error)?;
    reject_symlink_directory(destination_root)?;
    fs::create_dir_all(destination_root).map_err(|_| RecoveryPackageError::Io)?;
    reject_symlink_directory(destination_root)?;
    let expected_current_generation = validate_current_pointer(destination_root, raw_key)?;
    let generations_root = destination_root.join("generations");
    reject_symlink_directory(&generations_root)?;
    fs::create_dir_all(&generations_root).map_err(|_| RecoveryPackageError::Io)?;
    reject_symlink_directory(&generations_root)?;
    let generation_id = format!("catalog-gen-{}-{}", now_nanos(), std::process::id());
    let mut migration = MigrationJournal::create(
        lease.recovery_root(),
        MigrationKind::CatalogUpgrade,
        &source_hash,
        source_bytes,
        &source_hash,
        &generation_id,
    )
    .map_err(|_| RecoveryPackageError::MigrationManifestUnavailable)?;
    migration
        .transition(MigrationStage::Staging, None)
        .map_err(|_| RecoveryPackageError::MigrationManifestUnavailable)?;
    let staging = create_staging_directory(&generations_root, &generation_id)?;
    let staged_catalog = staging.join("catalog.db");
    if fs::copy(source_catalog, &staged_catalog).is_err() {
        // 复制失败也保留 staging，方便诊断部分写入和人工恢复，不吞掉失败现场。
        return Err(RecoveryPackageError::Io);
    }
    OpenOptions::new()
        .read(true)
        .write(true)
        .open(&staged_catalog)
        .and_then(|file| file.sync_all())
        .map_err(|_| RecoveryPackageError::Io)?;
    let copied_hash = hash_file(&staged_catalog)?;
    if copied_hash != source_hash
        || hash_file(source_catalog)? != source_hash
        || fs::metadata(&staged_catalog)
            .map_err(|_| RecoveryPackageError::Io)?
            .len()
            != source_bytes
    {
        return Err(RecoveryPackageError::VerificationFailed);
    }
    migrate_catalog_one_step(
        &staged_catalog,
        raw_key,
        source_schema_version,
        target_schema_version,
    )?;
    reject_catalog_sidecars(&staged_catalog)
        .map_err(|_| RecoveryPackageError::VerificationFailed)?;
    let semantic_counts =
        read_catalog_semantic_counts(&staged_catalog, raw_key, target_schema_version)?;
    if semantic_counts != source_semantic_counts {
        return Err(RecoveryPackageError::VerificationFailed);
    }
    let catalog_hash = hash_file(&staged_catalog)?;
    let bytes = fs::metadata(&staged_catalog)
        .map_err(|_| RecoveryPackageError::Io)?
        .len();
    let content_revision = read_catalog_content_revision(&staged_catalog, raw_key)?;
    let metadata = CatalogGenerationMetadata::new(
        &generation_id,
        target_schema_version,
        content_revision,
        catalog_hash.clone(),
        bytes,
        semantic_counts,
    );
    let mut manifest =
        File::create(staging.join("generation.json")).map_err(|_| RecoveryPackageError::Io)?;
    serde_json::to_writer_pretty(&mut manifest, &metadata)
        .map_err(|_| RecoveryPackageError::PayloadInvalid)?;
    manifest
        .write_all(b"\n")
        .map_err(|_| RecoveryPackageError::Io)?;
    manifest.sync_all().map_err(|_| RecoveryPackageError::Io)?;
    drop(manifest);
    verify_generation(&staging, raw_key, &metadata)?;
    migration
        .transition(MigrationStage::StagedVerified, Some(generation_id.clone()))
        .map_err(|_| RecoveryPackageError::MigrationManifestUnavailable)?;
    let generation_dir = generations_root.join(&generation_id);
    migration
        .transition(MigrationStage::Publishing, Some(generation_id.clone()))
        .map_err(|_| RecoveryPackageError::MigrationManifestUnavailable)?;
    fs::rename(&staging, &generation_dir).map_err(|_| RecoveryPackageError::Io)?;
    sync_directory(&generations_root)?;
    verify_generation(&generation_dir, raw_key, &metadata)?;
    if validate_current_pointer(destination_root, raw_key)? != expected_current_generation {
        return Err(RecoveryPackageError::MigrationRecoveryRequired);
    }
    let pointer = destination_root.join("current.json");
    let temporary_pointer = temporary_path(&pointer, "pointer");
    let mut pointer_file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary_pointer)
        .map_err(|_| RecoveryPackageError::Io)?;
    serde_json::to_writer_pretty(
        &mut pointer_file,
        &serde_json::json!({"generation_id": generation_id}),
    )
    .map_err(|_| RecoveryPackageError::PayloadInvalid)?;
    pointer_file
        .write_all(b"\n")
        .map_err(|_| RecoveryPackageError::Io)?;
    pointer_file
        .sync_all()
        .map_err(|_| RecoveryPackageError::Io)?;
    drop(pointer_file);
    publish_pointer(&temporary_pointer, &pointer)?;
    sync_directory(destination_root)?;
    let published_generation = read_pointer(&pointer)?;
    if published_generation != metadata.generation_id {
        return Err(RecoveryPackageError::VerificationFailed);
    }
    verify_generation(
        &generations_root.join(&published_generation),
        raw_key,
        &metadata,
    )?;
    migration
        .transition(MigrationStage::Completed, Some(generation_id.clone()))
        .map_err(|_| RecoveryPackageError::MigrationManifestUnavailable)?;
    Ok(CatalogUpgradeResult {
        generation_id,
        schema_version: target_schema_version,
        catalog_sha256: metadata.catalog_sha256,
        bytes,
    })
}

fn verify_generation(
    generation_dir: &Path,
    raw_key: &str,
    expected: &CatalogGenerationMetadata,
) -> Result<(), RecoveryPackageError> {
    if is_symlink(&generation_dir.join("generation.json"))
        || is_symlink(&generation_dir.join("catalog.db"))
    {
        return Err(RecoveryPackageError::VerificationFailed);
    }
    let metadata: CatalogGenerationMetadata = serde_json::from_reader(
        File::open(generation_dir.join("generation.json"))
            .map_err(|_| RecoveryPackageError::VerificationFailed)?,
    )
    .map_err(|_| RecoveryPackageError::VerificationFailed)?;
    let catalog = generation_dir.join("catalog.db");
    let bytes = fs::metadata(&catalog)
        .map_err(|_| RecoveryPackageError::VerificationFailed)?
        .len();
    if metadata != *expected
        || bytes != expected.bytes
        || hash_file(&catalog)? != expected.catalog_sha256
    {
        return Err(RecoveryPackageError::VerificationFailed);
    }
    // 旁路升级允许验证相邻目标 schema；不能把目标版本误判为生产当前 V1。
    if !expected.version_matrix_matches_for(expected.schema_version) {
        return Err(RecoveryPackageError::VerificationFailed);
    }
    let semantic_counts = read_catalog_semantic_counts(&catalog, raw_key, expected.schema_version)?;
    if semantic_counts != expected.semantic_counts {
        return Err(RecoveryPackageError::VerificationFailed);
    }
    let content_revision = read_catalog_content_revision(&catalog, raw_key)?;
    if content_revision != expected.content_revision {
        return Err(RecoveryPackageError::VerificationFailed);
    }
    Ok(())
}

/// 读取目录库 schema 版本和语义表行数；无法打开或无法完成完整性检查时拒绝升级。
fn read_catalog_semantic_counts(
    path: &Path,
    raw_key: &str,
    expected_schema_version: u32,
) -> Result<BTreeMap<String, u64>, RecoveryPackageError> {
    let conn = open_catalog_readonly(path, raw_key)?;
    verify_sqlite_integrity(&conn)?;
    if read_catalog_schema_version(&conn)? != expected_schema_version {
        return Err(RecoveryPackageError::VerificationFailed);
    }
    let mut statement = conn
        .prepare(
            "SELECT name FROM sqlite_master \
             WHERE type = 'table' AND name NOT LIKE 'sqlite_%' \
             ORDER BY name",
        )
        .map_err(|_| RecoveryPackageError::VerificationFailed)?;
    let table_names = statement
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(|_| RecoveryPackageError::VerificationFailed)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| RecoveryPackageError::VerificationFailed)?
        .into_iter()
        .filter(|name| name != "catalog_meta" && name != "catalog_migration_log")
        .collect::<Vec<_>>();
    let mut counts = BTreeMap::new();
    for table in table_names {
        let quoted = table.replace('"', "\"\"");
        let sql = format!("SELECT COUNT(*) FROM \"{quoted}\"");
        let count: i64 = conn
            .query_row(&sql, [], |row| row.get(0))
            .map_err(|_| RecoveryPackageError::VerificationFailed)?;
        if count < 0 {
            return Err(RecoveryPackageError::VerificationFailed);
        }
        counts.insert(table, count as u64);
    }
    Ok(counts)
}

fn read_catalog_schema_version(conn: &Connection) -> Result<u32, RecoveryPackageError> {
    let value: String = conn
        .query_row(
            "SELECT value FROM catalog_meta WHERE key = 'schema_version'",
            [],
            |row| row.get(0),
        )
        .map_err(|_| RecoveryPackageError::VerificationFailed)?;
    let catalog_meta_version = value
        .parse::<u32>()
        .map_err(|_| RecoveryPackageError::VerificationFailed)?;
    let user_version: u32 = conn
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .map_err(|_| RecoveryPackageError::VerificationFailed)?;
    if user_version != catalog_meta_version {
        return Err(RecoveryPackageError::VerificationFailed);
    }
    Ok(catalog_meta_version)
}

fn read_catalog_content_revision(path: &Path, raw_key: &str) -> Result<u64, RecoveryPackageError> {
    let conn = open_catalog_readonly(path, raw_key)?;
    let revision: u64 = conn
        .query_row(
            "SELECT value FROM catalog_meta WHERE key = 'content_revision'",
            [],
            |row| row.get::<_, String>(0),
        )
        .map_err(|_| RecoveryPackageError::VerificationFailed)?
        .parse()
        .map_err(|_| RecoveryPackageError::VerificationFailed)?;
    drop(conn);
    Ok(revision)
}

fn validate_raw_key(raw_key: &str) -> Result<(), RecoveryPackageError> {
    if raw_key.len() != 64 || hex::decode(raw_key).map_or(true, |key| key.len() != 32) {
        return Err(RecoveryPackageError::InvalidCatalogKey);
    }
    Ok(())
}

fn open_catalog_readonly(path: &Path, raw_key: &str) -> Result<Connection, RecoveryPackageError> {
    validate_raw_key(raw_key)?;
    if !path.is_file() || is_symlink(path) {
        return Err(RecoveryPackageError::VerificationFailed);
    }
    reject_catalog_sidecars(path).map_err(|_| RecoveryPackageError::VerificationFailed)?;
    reject_symlink_directory(path.parent().unwrap_or_else(|| Path::new(".")))?;
    // SQLite 只读打开持久 WAL 头时仍可能创建 -shm；协议随后才会拒绝 WAL。
    // 记录打开前状态，失败后仅清理本次新建的小型普通 sidecar，避免污染源目录。
    let sidecars_before = catalog_sidecars_snapshot(path);
    let result = (|| {
        let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(|_| RecoveryPackageError::VerificationFailed)?;
        connection
            .execute_batch(&format!("PRAGMA key = \"x'{raw_key}'\";"))
            .map_err(|_| RecoveryPackageError::VerificationFailed)?;
        verify_catalog_read_protocol(&connection)
            .map_err(|_| RecoveryPackageError::VerificationFailed)?;
        Ok(connection)
    })();
    if result.is_err() {
        cleanup_readonly_created_sidecars(path, &sidecars_before);
    }
    result
}

/// 记录目录库旁的 SQLite sidecar 是否在只读打开前已经存在。
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

/// 清理只读失败路径产生的瞬态 sidecar；已有文件和异常文件一律保留。
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

fn open_catalog_readwrite(path: &Path, raw_key: &str) -> Result<Connection, RecoveryPackageError> {
    validate_raw_key(raw_key)?;
    if !path.is_file() || is_symlink(path) {
        return Err(RecoveryPackageError::VerificationFailed);
    }
    reject_catalog_sidecars(path).map_err(|_| RecoveryPackageError::VerificationFailed)?;
    reject_symlink_directory(path.parent().unwrap_or_else(|| Path::new(".")))?;
    // 已复制到 staging 的目录库仍必须显式 READ_WRITE 打开，禁止 CREATE 兜底空库。
    let connection = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_WRITE
            | OpenFlags::SQLITE_OPEN_PRIVATE_CACHE
            | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|_| RecoveryPackageError::VerificationFailed)?;
    connection
        .execute_batch(&format!("PRAGMA key = \"x'{raw_key}'\";"))
        .map_err(|_| RecoveryPackageError::VerificationFailed)?;
    verify_existing_catalog_write_protocol(&connection)
        .map_err(|_| RecoveryPackageError::VerificationFailed)?;
    Ok(connection)
}

fn verify_sqlite_integrity(conn: &Connection) -> Result<(), RecoveryPackageError> {
    // SQLCipher 检查无错误时返回 0 行；每一行都代表一个完整性错误。
    let mut cipher_statement = conn
        .prepare("PRAGMA cipher_integrity_check")
        .map_err(|_| RecoveryPackageError::VerificationFailed)?;
    let cipher_errors = cipher_statement
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(|_| RecoveryPackageError::VerificationFailed)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| RecoveryPackageError::VerificationFailed)?;
    if !cipher_errors.is_empty() {
        return Err(RecoveryPackageError::VerificationFailed);
    }
    for pragma in ["integrity_check", "quick_check"] {
        let result: String = conn
            .query_row(&format!("PRAGMA {pragma}"), [], |row| row.get(0))
            .map_err(|_| RecoveryPackageError::VerificationFailed)?;
        if result.to_ascii_lowercase() != "ok" {
            return Err(RecoveryPackageError::VerificationFailed);
        }
    }
    Ok(())
}

/// 执行一条明确的相邻版本迁移；未知版本不通过复制伪装成升级成功。
fn migrate_catalog_one_step(
    path: &Path,
    raw_key: &str,
    source_schema_version: u32,
    target_schema_version: u32,
) -> Result<(), RecoveryPackageError> {
    let mut conn = open_catalog_readwrite(path, raw_key)?;
    // staging 是尚未发布的新代次，写入前统一建立 DELETE/FULL 协议。
    configure_new_catalog_write_protocol(&conn)
        .map_err(|_| RecoveryPackageError::VerificationFailed)?;
    if read_catalog_schema_version(&conn)? != source_schema_version {
        return Err(RecoveryPackageError::VerificationFailed);
    }
    let transaction = conn
        .transaction()
        .map_err(|_| RecoveryPackageError::VerificationFailed)?;
    match (source_schema_version, target_schema_version) {
        (1, 2) => {
            transaction
                .execute_batch(
                    "CREATE TABLE catalog_migration_log (
                         step INTEGER PRIMARY KEY,
                         applied_at INTEGER NOT NULL
                     );
                     INSERT INTO catalog_migration_log(step, applied_at) VALUES (2, 0);
                     INSERT OR IGNORE INTO catalog_meta(key, value) VALUES ('content_revision', '0');
                     UPDATE catalog_meta SET value = '2' WHERE key = 'schema_version';
                     PRAGMA user_version = 2;",
                )
                .map_err(|_| RecoveryPackageError::VerificationFailed)?;
        }
        (2, 3) => {
            transaction
                .execute_batch(
                    "ALTER TABLE catalog_meta ADD COLUMN updated_at INTEGER NOT NULL DEFAULT 0;
                     UPDATE catalog_meta SET updated_at = 0;
                     INSERT OR IGNORE INTO catalog_meta(key, value) VALUES ('content_revision', '0');
                     UPDATE catalog_meta SET value = '3' WHERE key = 'schema_version';
                     PRAGMA user_version = 3;",
                )
                .map_err(|_| RecoveryPackageError::VerificationFailed)?;
        }
        _ => return Err(RecoveryPackageError::InvalidPackage),
    }
    transaction
        .commit()
        .map_err(|_| RecoveryPackageError::VerificationFailed)
}

fn read_pointer(pointer: &Path) -> Result<String, RecoveryPackageError> {
    if is_symlink(pointer) {
        return Err(RecoveryPackageError::VerificationFailed);
    }
    let value: serde_json::Value = serde_json::from_reader(
        File::open(pointer).map_err(|_| RecoveryPackageError::VerificationFailed)?,
    )
    .map_err(|_| RecoveryPackageError::VerificationFailed)?;
    value
        .get("generation_id")
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
        .ok_or(RecoveryPackageError::VerificationFailed)
}

fn validate_current_pointer(
    destination_root: &Path,
    raw_key: &str,
) -> Result<Option<String>, RecoveryPackageError> {
    let pointer = destination_root.join("current.json");
    match fs::symlink_metadata(&pointer) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(_) => Err(RecoveryPackageError::VerificationFailed),
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
            Err(RecoveryPackageError::VerificationFailed)
        }
        Ok(_) => {
            let generation_id = read_pointer(&pointer)?;
            if generation_id.is_empty()
                || generation_id == "."
                || generation_id == ".."
                || generation_id
                    .chars()
                    .any(|character| character == '/' || character == '\\' || character == ':')
            {
                return Err(RecoveryPackageError::VerificationFailed);
            }
            let generation_dir = destination_root.join("generations").join(&generation_id);
            if !generation_dir.is_dir() || is_symlink(&generation_dir) {
                return Err(RecoveryPackageError::VerificationFailed);
            }
            let metadata: CatalogGenerationMetadata = serde_json::from_reader(
                File::open(generation_dir.join("generation.json"))
                    .map_err(|_| RecoveryPackageError::VerificationFailed)?,
            )
            .map_err(|_| RecoveryPackageError::VerificationFailed)?;
            verify_generation(&generation_dir, raw_key, &metadata)?;
            Ok(Some(generation_id))
        }
    }
}

fn temporary_path(destination: &Path, kind: &str) -> std::path::PathBuf {
    let name = destination
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("artifact");
    destination
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
        .join(format!(
            ".{name}.{kind}-{}-{}",
            now_nanos(),
            std::process::id()
        ))
}

/// 只创建全新的 staging 目录；不复用旧目录，避免路径碰撞导致覆盖残留内容。
fn create_staging_directory(
    generations_root: &Path,
    generation_id: &str,
) -> Result<PathBuf, RecoveryPackageError> {
    let staging = generations_root.join(format!(".staging-{generation_id}"));
    fs::create_dir(&staging).map_err(|_| RecoveryPackageError::Io)?;
    Ok(staging)
}

fn publish_new_file(temporary: &Path, destination: &Path) -> Result<(), RecoveryPackageError> {
    if fs::symlink_metadata(destination).is_ok() {
        // 目标已存在时也要回收临时文件，避免失败重试累积残留。
        let _ = fs::remove_file(temporary);
        return Err(RecoveryPackageError::Io);
    }
    let result = (|| {
        fs::hard_link(temporary, destination).map_err(|_| RecoveryPackageError::Io)?;
        fs::remove_file(temporary).map_err(|_| RecoveryPackageError::Io)
    })();
    if result.is_err() {
        // 发布失败时临时包不是用户证据，立即清理，避免失败重试累积大文件。
        let _ = fs::remove_file(temporary);
    }
    result
}

fn publish_pointer(temporary: &Path, pointer: &Path) -> Result<(), RecoveryPackageError> {
    crate::atomic_publish::publish_replacing(temporary, pointer)
        .map_err(|_| RecoveryPackageError::Io)
}

fn sync_directory(path: &Path) -> Result<(), RecoveryPackageError> {
    #[cfg(unix)]
    {
        File::open(path)
            .map_err(|_| RecoveryPackageError::Io)?
            .sync_all()
            .map_err(|_| RecoveryPackageError::Io)?;
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
    Ok(())
}

fn is_symlink(path: &Path) -> bool {
    fs::symlink_metadata(path)
        .map(|metadata| metadata.file_type().is_symlink())
        .unwrap_or(false)
}

fn reject_symlink_directory(path: &Path) -> Result<(), RecoveryPackageError> {
    let mut current = Path::new("").to_path_buf();
    for component in path.components() {
        current.push(component.as_os_str());
        if is_symlink(&current) {
            return Err(RecoveryPackageError::Io);
        }
    }
    Ok(())
}

fn validate_payload(payload: &RecoveryPayload) -> Result<(), RecoveryPackageError> {
    if payload.catalog_id.is_empty()
        || payload.catalog_key_hex.len() != 64
        || hex::decode(&payload.catalog_key_hex).map_or(true, |key| key.len() != 32)
    {
        return if payload.catalog_key_hex.len() != 64 {
            Err(RecoveryPackageError::InvalidCatalogKey)
        } else {
            Err(RecoveryPackageError::PayloadInvalid)
        };
    }
    Ok(())
}

fn derive_key(passphrase: &str, salt: &[u8; SALT_LEN]) -> Result<[u8; 32], RecoveryPackageError> {
    derive_key_with_iterations(passphrase, salt, KDF_ITERATIONS as u32)
}

fn derive_key_with_iterations(
    passphrase: &str,
    salt: &[u8; SALT_LEN],
    iterations: u32,
) -> Result<[u8; 32], RecoveryPackageError> {
    let mut output = [0_u8; 32];
    pkcs5::pbkdf2_hmac(
        passphrase.as_bytes(),
        salt,
        iterations as usize,
        MessageDigest::sha256(),
        &mut output,
    )
    .map_err(|_| RecoveryPackageError::Crypto)?;
    Ok(output)
}

fn encrypt(
    key: &[u8; 32],
    nonce: &[u8; NONCE_LEN],
    plaintext: &[u8],
) -> Result<(Vec<u8>, [u8; TAG_LEN]), RecoveryPackageError> {
    let cipher = Cipher::aes_256_gcm();
    let mut crypter = Crypter::new(cipher, Mode::Encrypt, key, Some(nonce))
        .map_err(|_| RecoveryPackageError::Crypto)?;
    let mut output = vec![0_u8; plaintext.len() + cipher.block_size()];
    let mut count = crypter
        .update(plaintext, &mut output)
        .map_err(|_| RecoveryPackageError::Crypto)?;
    count += crypter
        .finalize(&mut output[count..])
        .map_err(|_| RecoveryPackageError::Crypto)?;
    output.truncate(count);
    let mut tag = [0_u8; TAG_LEN];
    crypter
        .get_tag(&mut tag)
        .map_err(|_| RecoveryPackageError::Crypto)?;
    Ok((output, tag))
}

fn decrypt(
    key: &[u8; 32],
    nonce: &[u8; NONCE_LEN],
    tag: &[u8; TAG_LEN],
    ciphertext: &[u8],
) -> Result<Vec<u8>, RecoveryPackageError> {
    let cipher = Cipher::aes_256_gcm();
    let mut crypter = Crypter::new(cipher, Mode::Decrypt, key, Some(nonce))
        .map_err(|_| RecoveryPackageError::Crypto)?;
    crypter
        .set_tag(tag)
        .map_err(|_| RecoveryPackageError::Crypto)?;
    let mut output = vec![0_u8; ciphertext.len() + cipher.block_size()];
    let mut count = crypter
        .update(ciphertext, &mut output)
        .map_err(|_| RecoveryPackageError::Crypto)?;
    count += crypter
        .finalize(&mut output[count..])
        .map_err(|_| RecoveryPackageError::Crypto)?;
    output.truncate(count);
    Ok(output)
}

type ParsedRecoveryPackage<'a> = (
    u32,
    [u8; SALT_LEN],
    [u8; NONCE_LEN],
    [u8; TAG_LEN],
    &'a [u8],
);

fn parse_package(bytes: &[u8]) -> Result<ParsedRecoveryPackage<'_>, RecoveryPackageError> {
    let header_len = MAGIC.len() + 1 + 4 + SALT_LEN + NONCE_LEN + TAG_LEN + 4;
    if bytes.len() < header_len || &bytes[..MAGIC.len()] != MAGIC || bytes[8] != FORMAT_VERSION {
        return Err(RecoveryPackageError::InvalidPackage);
    }
    let iterations = u32::from_le_bytes(bytes[9..13].try_into().unwrap());
    if iterations != KDF_ITERATIONS as u32 {
        return Err(RecoveryPackageError::InvalidPackage);
    }
    let mut salt = [0_u8; SALT_LEN];
    salt.copy_from_slice(&bytes[13..29]);
    let mut nonce = [0_u8; NONCE_LEN];
    nonce.copy_from_slice(&bytes[29..41]);
    let mut tag = [0_u8; TAG_LEN];
    tag.copy_from_slice(&bytes[41..57]);
    let payload_len = u32::from_le_bytes(bytes[57..61].try_into().unwrap()) as usize;
    if payload_len == 0 || bytes.len() != header_len + payload_len {
        return Err(RecoveryPackageError::InvalidPackage);
    }
    Ok((iterations, salt, nonce, tag, &bytes[header_len..]))
}

fn hash_file(path: &Path) -> Result<String, RecoveryPackageError> {
    let mut file = File::open(path).map_err(|_| RecoveryPackageError::Io)?;
    let mut digest = Sha256::new();
    // 恢复包哈希同样可能处理大文件，避免把 1 MiB 缓冲区压在线程栈上。
    let mut buffer = vec![0_u8; 1024 * 1024];
    loop {
        let count = file
            .read(&mut buffer)
            .map_err(|_| RecoveryPackageError::Io)?;
        if count == 0 {
            break;
        }
        digest.update(&buffer[..count]);
    }
    Ok(hex::encode(digest.finalize()))
}

fn now_nanos() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|value| value.as_nanos())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    const TEST_CATALOG_KEY: &str =
        "deadbeefcafebabe1234567890abcdefdeadbeefcafebabe1234567890abcdef";

    fn create_catalog(path: &Path, schema_version: u32) {
        let conn = Connection::open(path).unwrap();
        conn.execute_batch(&format!("PRAGMA key = \"x'{TEST_CATALOG_KEY}'\";"))
            .unwrap();
        conn.execute_batch(
            "CREATE TABLE catalog_meta (
                 key TEXT PRIMARY KEY,
                 value TEXT NOT NULL
             );
             CREATE TABLE history_item (
                 id INTEGER PRIMARY KEY,
                 value TEXT NOT NULL
             );
             INSERT INTO history_item(id, value) VALUES (1, 'one'), (2, 'two');",
        )
        .unwrap();
        if schema_version >= 2 {
            conn.execute_batch(
                "CREATE TABLE catalog_migration_log (
                     step INTEGER PRIMARY KEY,
                     applied_at INTEGER NOT NULL
                 );
                 INSERT INTO catalog_migration_log(step, applied_at) VALUES (2, 0);",
            )
            .unwrap();
        }
        if schema_version >= 3 {
            conn.execute_batch(
                "ALTER TABLE catalog_meta ADD COLUMN updated_at INTEGER NOT NULL DEFAULT 0;",
            )
            .unwrap();
        }
        conn.execute(
            "INSERT INTO catalog_meta(key, value) VALUES ('schema_version', ?1)",
            [schema_version.to_string()],
        )
        .unwrap();
        conn.execute_batch(&format!("PRAGMA user_version = {schema_version};"))
            .unwrap();
    }

    fn payload() -> RecoveryPayload {
        RecoveryPayload {
            catalog_id: "catalog-a".to_string(),
            key_generation: 3,
            catalog_key_hex: "ab".repeat(32),
        }
    }

    /// 构造带固定元数据的认证包，确保版本矩阵测试不依赖随机输出。
    fn write_package_with_metadata(
        path: &Path,
        passphrase: &str,
        metadata: RecoveryPackageMetadata,
    ) {
        let salt = [0x11_u8; SALT_LEN];
        let nonce = [0x22_u8; NONCE_LEN];
        let key = derive_key(passphrase, &salt).unwrap();
        let plaintext = serde_json::to_vec(&RecoveryEnvelope {
            metadata,
            payload: payload(),
        })
        .unwrap();
        let (ciphertext, tag) = encrypt(&key, &nonce, &plaintext).unwrap();
        let mut bytes = Vec::new();
        bytes.extend_from_slice(MAGIC);
        bytes.push(FORMAT_VERSION);
        bytes.extend_from_slice(&(KDF_ITERATIONS as u32).to_le_bytes());
        bytes.extend_from_slice(&salt);
        bytes.extend_from_slice(&nonce);
        bytes.extend_from_slice(&tag);
        bytes.extend_from_slice(&(ciphertext.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&ciphertext);
        fs::write(path, bytes).unwrap();
    }

    #[test]
    #[ignore = "仅生成目标设备基准，不作为常规回归"]
    fn kdf_target_device_benchmark() {
        let salt = [0x5a_u8; SALT_LEN];
        let mut samples = Vec::new();
        for _ in 0..3 {
            let started = Instant::now();
            let key =
                derive_key_with_iterations("benchmark-passphrase", &salt, KDF_ITERATIONS as u32)
                    .unwrap();
            std::hint::black_box(key);
            samples.push(started.elapsed().as_millis());
        }
        samples.sort_unstable();
        println!(
            "KDF_BASELINE iterations={} samples_ms={:?} median_ms={}",
            KDF_ITERATIONS,
            samples,
            samples[samples.len() / 2]
        );
    }

    #[test]
    fn wrong_password_and_corruption_do_not_write_output() {
        let dir = tempfile::tempdir().unwrap();
        let package = dir.path().join("catalog.traesync-recovery");
        export_recovery_package(&package, "correct-password", &payload()).unwrap();
        assert!(matches!(
            import_recovery_package(&package, "wrong-password", "catalog-a", 3),
            Err(RecoveryPackageError::WrongPassphrase)
        ));
        let mut bytes = fs::read(&package).unwrap();
        let last = bytes.len() - 1;
        bytes[last] ^= 0x01;
        fs::write(&package, bytes).unwrap();
        assert!(matches!(
            import_recovery_package(&package, "correct-password", "catalog-a", 3),
            Err(RecoveryPackageError::WrongPassphrase)
        ));
        assert!(package.exists());
    }

    #[test]
    fn oversized_recovery_package_is_rejected_before_decryption() {
        let dir = tempfile::tempdir().unwrap();
        let package = dir.path().join("oversized.traesync-recovery");
        let file = File::create(&package).unwrap();
        file.set_len(MAX_RECOVERY_PACKAGE_BYTES + 1).unwrap();

        assert!(matches!(
            import_recovery_package(&package, "password", "catalog-a", 3),
            Err(RecoveryPackageError::InvalidPackage)
        ));
    }

    #[test]
    fn staging_creation_does_not_reuse_existing_directory() {
        let dir = tempfile::tempdir().unwrap();
        let generations = dir.path().join("generations");
        fs::create_dir_all(&generations).unwrap();
        let existing = generations.join(".staging-collision");
        fs::create_dir(&existing).unwrap();

        assert!(matches!(
            create_staging_directory(&generations, "collision"),
            Err(RecoveryPackageError::Io)
        ));
        assert!(existing.is_dir());
    }

    #[test]
    fn identity_mismatch_is_rejected_after_authentication() {
        let dir = tempfile::tempdir().unwrap();
        let package = dir.path().join("catalog.traesync-recovery");
        export_recovery_package(&package, "password", &payload()).unwrap();
        assert!(matches!(
            import_recovery_package(&package, "password", "catalog-b", 3),
            Err(RecoveryPackageError::IdentityMismatch)
        ));
    }

    #[test]
    fn unsupported_metadata_is_rejected_before_catalog_verification() {
        let dir = tempfile::tempdir().unwrap();
        let package = dir.path().join("unsupported-metadata.traesync-recovery");
        let mut metadata = RecoveryPackageMetadata::current();
        metadata.mapping_version = "catalog-future".to_string();
        write_package_with_metadata(&package, "password", metadata);

        assert!(matches!(
            import_recovery_package(&package, "password", "catalog-a", 3),
            Err(RecoveryPackageError::UnsupportedMetadata)
        ));
    }

    #[test]
    fn space_reservation_failure_keeps_structured_error() {
        assert_eq!(
            map_space_reservation_error(crate::storage_root::StorageRootError::InsufficientSpace),
            RecoveryPackageError::InsufficientSpace
        );
    }

    #[test]
    fn insufficient_space_maps_to_port_error() {
        assert_eq!(
            map_import_error(RecoveryPackageError::InsufficientSpace),
            RecoveryImportError::InsufficientSpace
        );
    }

    #[test]
    fn export_does_not_overwrite_existing_package() {
        let dir = tempfile::tempdir().unwrap();
        let package = dir.path().join("catalog.traesync-recovery");
        fs::write(&package, b"existing").unwrap();
        assert!(matches!(
            export_recovery_package(&package, "password", &payload()),
            Err(RecoveryPackageError::Io)
        ));
        assert_eq!(fs::read(&package).unwrap(), b"existing");
    }

    #[test]
    fn sidecar_upgrade_preserves_old_catalog_and_switches_pointer_last() {
        let dir = tempfile::tempdir().unwrap();
        let old = dir.path().join("old.db");
        create_catalog(&old, 1);
        let destination = dir.path().join("catalog");
        let recovery = dir.path().join("fixed-recovery");
        let result =
            upgrade_catalog_sidecar(&old, &destination, &recovery, TEST_CATALOG_KEY, 1, 2).unwrap();
        assert_eq!(
            read_catalog_schema_version(&open_catalog_readonly(&old, TEST_CATALOG_KEY).unwrap())
                .unwrap(),
            1
        );
        assert!(destination.join("current.json").is_file());
        assert_eq!(result.schema_version, 2);
        assert_eq!(
            read_catalog_schema_version(
                &open_catalog_readonly(
                    &destination
                        .join("generations")
                        .join(&result.generation_id)
                        .join("catalog.db"),
                    TEST_CATALOG_KEY,
                )
                .unwrap()
            )
            .unwrap(),
            2
        );
    }

    #[test]
    fn sidecar_upgrade_replaces_pointer_atomically_and_keeps_old_generation() {
        let dir = tempfile::tempdir().unwrap();
        let old = dir.path().join("old.db");
        create_catalog(&old, 1);
        let destination = dir.path().join("catalog");
        let recovery = dir.path().join("fixed-recovery");
        let first =
            upgrade_catalog_sidecar(&old, &destination, &recovery, TEST_CATALOG_KEY, 1, 2).unwrap();

        fs::remove_file(&old).unwrap();
        create_catalog(&old, 2);
        let second =
            upgrade_catalog_sidecar(&old, &destination, &recovery, TEST_CATALOG_KEY, 2, 3).unwrap();
        let pointer: serde_json::Value =
            serde_json::from_reader(File::open(destination.join("current.json")).unwrap()).unwrap();
        assert_eq!(
            pointer["generation_id"].as_str(),
            Some(second.generation_id.as_str())
        );
        assert_ne!(first.generation_id, second.generation_id);
        assert_eq!(
            read_catalog_schema_version(
                &open_catalog_readonly(
                    &destination
                        .join("generations")
                        .join(&first.generation_id)
                        .join("catalog.db"),
                    TEST_CATALOG_KEY,
                )
                .unwrap()
            )
            .unwrap(),
            2
        );
        assert_eq!(
            read_catalog_schema_version(
                &open_catalog_readonly(
                    &destination
                        .join("generations")
                        .join(&second.generation_id)
                        .join("catalog.db"),
                    TEST_CATALOG_KEY,
                )
                .unwrap()
            )
            .unwrap(),
            3
        );
    }

    #[test]
    fn sidecar_upgrade_writes_completed_manifest_in_fixed_recovery_root() {
        let dir = tempfile::tempdir().unwrap();
        let old = dir.path().join("old.db");
        create_catalog(&old, 1);
        let destination = dir.path().join("catalog");
        let recovery = dir.path().join("fixed-recovery");
        let result = upgrade_catalog_sidecar_with_recovery_root(
            &old,
            &destination,
            &recovery,
            TEST_CATALOG_KEY,
            1,
            2,
        )
        .unwrap();

        let operation_dirs = fs::read_dir(recovery.join("migration-manifests"))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(operation_dirs.len(), 1);
        let operation_id = operation_dirs[0].file_name().to_string_lossy().into_owned();
        let latest = crate::migration_manifest::read_latest_for_test(&recovery, &operation_id)
            .unwrap()
            .unwrap();
        assert_eq!(latest.kind, MigrationKind::CatalogUpgrade);
        assert_eq!(latest.stage, MigrationStage::Completed);
        assert_eq!(
            latest.destination_id.as_deref(),
            Some(result.generation_id.as_str())
        );
    }

    #[test]
    fn unfinished_sidecar_upgrade_is_frozen_before_new_generation_is_created() {
        let dir = tempfile::tempdir().unwrap();
        let old = dir.path().join("old.db");
        create_catalog(&old, 1);
        let destination = dir.path().join("catalog");
        let recovery = dir.path().join("fixed-recovery");
        let mut journal = MigrationJournal::create(
            &recovery,
            MigrationKind::CatalogUpgrade,
            "source-catalog-hash",
            100,
            "source-catalog-hash",
            "generation-staging",
        )
        .unwrap();
        journal.transition(MigrationStage::Staging, None).unwrap();

        assert!(matches!(
            upgrade_catalog_sidecar_with_recovery_root(
                &old,
                &destination,
                &recovery,
                TEST_CATALOG_KEY,
                1,
                2,
            ),
            Err(RecoveryPackageError::MigrationRecoveryRequired)
        ));
        assert!(!destination.exists());
        assert_eq!(
            read_catalog_schema_version(&open_catalog_readonly(&old, TEST_CATALOG_KEY).unwrap())
                .unwrap(),
            1
        );
        let latest = crate::migration_manifest::read_latest_for_test(
            &recovery,
            &journal.latest().operation_id,
        )
        .unwrap()
        .unwrap();
        assert_eq!(latest.stage, MigrationStage::ManualRecoveryRequired);
    }

    #[test]
    fn sidecar_upgrade_rejects_symlink_source_without_creating_generation() {
        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let source_target = outside.path().join("catalog.db");
        let source = dir.path().join("source.db");
        fs::write(&source_target, b"catalog").unwrap();
        #[cfg(windows)]
        let link_result = std::os::windows::fs::symlink_file(&source_target, &source);
        #[cfg(unix)]
        let link_result = std::os::unix::fs::symlink(&source_target, &source);
        #[cfg(not(any(windows, unix)))]
        return;
        if link_result.is_err() {
            return;
        }

        let destination = dir.path().join("catalog");
        assert!(matches!(
            upgrade_catalog_sidecar(
                &source,
                &destination,
                &dir.path().join("fixed-recovery"),
                TEST_CATALOG_KEY,
                1,
                2,
            ),
            Err(RecoveryPackageError::InvalidPackage)
        ));
        assert!(!destination.exists());
        assert_eq!(fs::read(&source_target).unwrap(), b"catalog");
    }

    #[test]
    fn catalog_open_rejects_symlinked_parent_directory() {
        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let target = outside.path().join("catalog.db");
        create_catalog(&target, 1);
        let linked_root = dir.path().join("linked");
        #[cfg(windows)]
        let link_result = std::os::windows::fs::symlink_dir(outside.path(), &linked_root);
        #[cfg(unix)]
        let link_result = std::os::unix::fs::symlink(outside.path(), &linked_root);
        #[cfg(not(any(windows, unix)))]
        return;
        if link_result.is_err() {
            return;
        }

        assert!(matches!(
            open_catalog_readonly(&linked_root.join("catalog.db"), TEST_CATALOG_KEY),
            Err(RecoveryPackageError::Io) | Err(RecoveryPackageError::VerificationFailed)
        ));
    }

    #[test]
    fn sidecar_upgrade_rejects_symlink_destination_root() {
        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let source = dir.path().join("old.db");
        let destination = dir.path().join("catalog");
        fs::write(&source, b"catalog").unwrap();
        #[cfg(windows)]
        let link_result = std::os::windows::fs::symlink_dir(outside.path(), &destination);
        #[cfg(unix)]
        let link_result = std::os::unix::fs::symlink(outside.path(), &destination);
        #[cfg(not(any(windows, unix)))]
        return;
        if link_result.is_err() {
            return;
        }

        assert!(matches!(
            upgrade_catalog_sidecar(
                &source,
                &destination,
                &dir.path().join("fixed-recovery"),
                TEST_CATALOG_KEY,
                1,
                2,
            ),
            Err(RecoveryPackageError::Io)
        ));
        assert!(!outside.path().join("current.json").exists());
    }

    #[test]
    fn sidecar_upgrade_rejects_schema_version_jump_without_output() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("old.db");
        let destination = dir.path().join("catalog");
        fs::write(&source, b"catalog").unwrap();

        assert!(matches!(
            upgrade_catalog_sidecar(
                &source,
                &destination,
                &dir.path().join("fixed-recovery"),
                TEST_CATALOG_KEY,
                1,
                3,
            ),
            Err(RecoveryPackageError::InvalidPackage)
        ));
        assert!(!destination.exists());
    }

    #[test]
    fn sidecar_upgrade_rejects_non_sqlite_catalog_without_output() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("old.db");
        let destination = dir.path().join("catalog");
        fs::write(&source, b"not a sqlite catalog").unwrap();

        assert!(matches!(
            upgrade_catalog_sidecar(
                &source,
                &destination,
                &dir.path().join("fixed-recovery"),
                TEST_CATALOG_KEY,
                1,
                2,
            ),
            Err(RecoveryPackageError::VerificationFailed)
        ));
        assert!(!destination.exists());
        assert_eq!(fs::read(&source).unwrap(), b"not a sqlite catalog");
    }

    #[test]
    fn sidecar_upgrade_rejects_corrupt_current_pointer_without_replacing_it() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("old.db");
        let destination = dir.path().join("catalog");
        fs::write(&source, b"catalog").unwrap();
        fs::create_dir_all(&destination).unwrap();
        fs::write(
            destination.join("current.json"),
            b"{\"generation_id\":\"missing\"}",
        )
        .unwrap();

        assert!(matches!(
            upgrade_catalog_sidecar(
                &source,
                &destination,
                &dir.path().join("fixed-recovery"),
                TEST_CATALOG_KEY,
                1,
                2,
            ),
            Err(RecoveryPackageError::VerificationFailed)
        ));
        assert_eq!(
            fs::read_to_string(destination.join("current.json")).unwrap(),
            "{\"generation_id\":\"missing\"}"
        );
        assert_eq!(
            fs::read_dir(&destination)
                .unwrap()
                .filter_map(Result::ok)
                .count(),
            1
        );
    }

    #[test]
    fn failed_new_file_publication_removes_temporary_file() {
        let dir = tempfile::tempdir().unwrap();
        let temporary = dir.path().join("package.tmp");
        let destination = dir.path().join("package");
        fs::write(&temporary, b"package").unwrap();
        fs::create_dir(&destination).unwrap();

        assert!(publish_new_file(&temporary, &destination).is_err());
        assert!(!temporary.exists());
    }

    #[test]
    fn failed_pointer_publication_removes_temporary_file() {
        let dir = tempfile::tempdir().unwrap();
        let temporary = dir.path().join("pointer.tmp");
        let pointer = dir.path().join("current.json");
        fs::write(&temporary, b"pointer").unwrap();
        fs::create_dir(&pointer).unwrap();

        assert!(publish_pointer(&temporary, &pointer).is_err());
        assert!(!temporary.exists());
    }
}
