//! T12 存储根绑定、迁移与空间预留。
//!
//! 正常打开只验证已有绑定，不会因为目录缺失而创建空目录库。迁移只复制到新根，
//! 校验完成后发布新根绑定；旧根始终保留，由用户单独处理。

use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use traesync_domain::FileIdentity;
use traesync_ports::FileIdentityProvider;

use crate::catalog_path::resolve_current_catalog_path;
use crate::file_identity::PlatformFileIdentityProvider;
use crate::migration_manifest::{
    freeze_unfinished, MigrationJournal, MigrationKind, MigrationStage,
};
use crate::operation_lease::OperationLease;

/// 默认历史库存储警戒线；达到后只暂停非必要自动扫描，不删除任何数据。
pub const DEFAULT_STORAGE_WARNING_BYTES: u64 = 5 * 1024 * 1024 * 1024;
/// 空间预算的固定安全余量下限；最终预算还会取源数据量的 20%。
pub const MINIMUM_STORAGE_RESERVE_BYTES: u64 = 512 * 1024 * 1024;

/// 按实现规格计算副作用前的安全余量：预算的 20%，且不低于 512 MiB。
pub const fn required_storage_reserve_bytes(budget_bytes: u64) -> u64 {
    let percentage = budget_bytes.saturating_mul(20) / 100;
    if percentage > MINIMUM_STORAGE_RESERVE_BYTES {
        percentage
    } else {
        MINIMUM_STORAGE_RESERVE_BYTES
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StorageRootBinding {
    pub format_version: u32,
    pub storage_root_id: String,
    pub canonical_path: String,
    pub catalog_id: String,
    /// 目录库物理文件身份；存在时必须匹配当前 generation，旧绑定可在重开时补齐。
    #[serde(default)]
    pub catalog_file_identity: Option<FileIdentity>,
    pub created_at_secs: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StorageRootError {
    RootMissing,
    RootNotDirectory,
    BindingMissing,
    BindingInvalid,
    RootIdentityMismatch,
    RootIdentityUnavailable,
    CatalogIdentityMismatch,
    DestinationNotEmpty,
    DestinationUnavailable,
    CopyFailed,
    VerificationFailed,
    InsufficientSpace,
    PointerUnavailable,
    MigrationManifestUnavailable,
    MigrationRecoveryRequired,
}

impl std::fmt::Display for StorageRootError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let message = match self {
            Self::RootMissing => "存储根不存在",
            Self::RootNotDirectory => "存储根不是目录",
            Self::BindingMissing => "存储根绑定缺失",
            Self::BindingInvalid => "存储根绑定损坏",
            Self::RootIdentityMismatch => "存储根身份不匹配",
            Self::RootIdentityUnavailable => "无法读取存储根物理身份",
            Self::CatalogIdentityMismatch => "目录库身份不匹配",
            Self::DestinationNotEmpty => "迁移目标目录非空",
            Self::DestinationUnavailable => "迁移目标不可用",
            Self::CopyFailed => "存储根复制失败",
            Self::VerificationFailed => "存储根逐文件验证失败",
            Self::InsufficientSpace => "可用空间不足",
            Self::PointerUnavailable => "存储根当前指针不可用",
            Self::MigrationManifestUnavailable => "迁移恢复记录不可用",
            Self::MigrationRecoveryRequired => "存在未完成迁移，需要人工恢复",
        };
        f.write_str(message)
    }
}

impl std::error::Error for StorageRootError {}

impl StorageRootBinding {
    pub fn initialize(root: &Path, catalog_id: &str) -> Result<Self, StorageRootError> {
        if catalog_id.is_empty() {
            return Err(StorageRootError::CatalogIdentityMismatch);
        }
        let root_preexisted = root.exists();
        reject_symlink_directory_chain(root)?;
        fs::create_dir_all(root).map_err(|_| StorageRootError::DestinationUnavailable)?;
        reject_symlink_directory_chain(root)?;
        let canonical = root
            .canonicalize()
            .map_err(|_| StorageRootError::DestinationUnavailable)?;
        // 已有绑定只能幂等打开，不能被新的目录库身份静默覆盖。
        let binding_path = canonical.join("storage-root.json");
        if fs::symlink_metadata(&binding_path)
            .map(|metadata| metadata.file_type().is_symlink())
            .unwrap_or(false)
        {
            return Err(StorageRootError::BindingInvalid);
        }
        if binding_path.is_file() {
            return Self::open_existing(&canonical, None, Some(catalog_id));
        }
        let catalog_file_identity = match read_catalog_identity(&canonical) {
            Ok(identity) => identity,
            Err(error) => {
                // 新根尚未绑定成功时不留下空目录；已有根必须保持原样，交给调用方诊断。
                if !root_preexisted {
                    let _ = fs::remove_dir(&canonical);
                }
                return Err(error);
            }
        };
        let binding = Self {
            format_version: 1,
            storage_root_id: physical_root_id(&canonical)
                .ok_or(StorageRootError::RootIdentityUnavailable)?,
            canonical_path: canonical.to_string_lossy().into_owned(),
            catalog_id: catalog_id.to_string(),
            catalog_file_identity: Some(catalog_file_identity),
            created_at_secs: now_secs(),
        };
        write_binding(&canonical, &binding)?;
        Ok(binding)
    }

    pub fn open_existing(
        root: &Path,
        expected_root_id: Option<&str>,
        expected_catalog_id: Option<&str>,
    ) -> Result<Self, StorageRootError> {
        // 打开已有根也必须拒绝符号链接链，不能让 canonicalize 把链接目标伪装成绑定根。
        reject_symlink_directory_chain(root)?;
        if !root.exists() {
            return Err(StorageRootError::RootMissing);
        }
        if !root.is_dir() {
            return Err(StorageRootError::RootNotDirectory);
        }
        let canonical = root
            .canonicalize()
            .map_err(|_| StorageRootError::RootMissing)?;
        let binding_path = canonical.join("storage-root.json");
        let binding_metadata =
            fs::symlink_metadata(&binding_path).map_err(|_| StorageRootError::BindingMissing)?;
        if binding_metadata.file_type().is_symlink() || !binding_metadata.is_file() {
            return Err(StorageRootError::BindingMissing);
        }
        let mut binding: Self = serde_json::from_reader(
            File::open(binding_path).map_err(|_| StorageRootError::BindingInvalid)?,
        )
        .map_err(|_| StorageRootError::BindingInvalid)?;
        if binding.format_version != 1
            || binding.storage_root_id.is_empty()
            || binding.catalog_id.is_empty()
            || binding.canonical_path != canonical.to_string_lossy()
        {
            return Err(StorageRootError::RootIdentityMismatch);
        }
        if binding.storage_root_id
            != physical_root_id(&canonical).ok_or(StorageRootError::RootIdentityUnavailable)?
            || expected_root_id.is_some_and(|value| value != binding.storage_root_id)
        {
            return Err(StorageRootError::RootIdentityMismatch);
        }
        if expected_catalog_id.is_some_and(|value| value != binding.catalog_id) {
            return Err(StorageRootError::CatalogIdentityMismatch);
        }
        let actual_catalog_identity = read_catalog_identity(&canonical)?;
        if binding
            .catalog_file_identity
            .as_ref()
            .is_some_and(|expected| expected != &actual_catalog_identity)
        {
            return Err(StorageRootError::CatalogIdentityMismatch);
        }
        // 旧绑定或固定恢复区指针可以不携带目录库身份；身份已由当前 generation 重新读取。
        binding.catalog_file_identity = Some(actual_catalog_identity);
        Ok(binding)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StorageMigrationResult {
    pub old_root_id: String,
    pub new_root_id: String,
    pub copied_files: u64,
    pub copied_bytes: u64,
    pub file_hashes: BTreeMap<String, String>,
}

/// 存储占用的只读状态，供 UI 展示和自动扫描前置判断使用。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct StorageSpaceStatus {
    pub used_bytes: u64,
    pub warning_threshold_bytes: u64,
    pub warning_active: bool,
    pub automatic_scan_paused: bool,
}

/// 固定恢复区中的当前存储根指针；只保存绑定元数据，不保存目录库正文。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StorageRootPointer {
    pub format_version: u32,
    pub storage_root_id: String,
    pub canonical_path: String,
    pub catalog_id: String,
    pub created_at_secs: u64,
}

impl StorageRootPointer {
    fn from_binding(binding: &StorageRootBinding) -> Self {
        Self {
            format_version: binding.format_version,
            storage_root_id: binding.storage_root_id.clone(),
            canonical_path: binding.canonical_path.clone(),
            catalog_id: binding.catalog_id.clone(),
            created_at_secs: binding.created_at_secs,
        }
    }

    fn into_binding(self) -> StorageRootBinding {
        StorageRootBinding {
            format_version: self.format_version,
            storage_root_id: self.storage_root_id,
            canonical_path: self.canonical_path,
            catalog_id: self.catalog_id,
            catalog_file_identity: None,
            created_at_secs: self.created_at_secs,
        }
    }
}

/// 通过已持有的共享租约迁移存储根；指针发布前旧根仍是唯一权威根。
pub fn migrate_storage_root_with_lease(
    source: &Path,
    destination: &Path,
    catalog_id: &str,
    minimum_free_bytes: u64,
    lease: &OperationLease,
) -> Result<StorageMigrationResult, StorageRootError> {
    let pointer = lease.recovery_root().join("storage-root-pointer.json");
    migrate_storage_root_inner(
        source,
        destination,
        catalog_id,
        minimum_free_bytes,
        &pointer,
    )
}

#[cfg(test)]
fn migrate_storage_root(
    source: &Path,
    destination: &Path,
    catalog_id: &str,
    minimum_free_bytes: u64,
    pointer: &Path,
) -> Result<StorageMigrationResult, StorageRootError> {
    let recovery_root = pointer
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .ok_or(StorageRootError::PointerUnavailable)?;
    let lease = OperationLease::acquire(recovery_root, "storage-root")
        .map_err(|_| StorageRootError::PointerUnavailable)?;
    migrate_storage_root_with_lease(source, destination, catalog_id, minimum_free_bytes, &lease)
}

/// 读取并验证固定恢复区中的当前存储根指针；根缺失时不创建替代根。
pub fn read_storage_root_pointer(pointer: &Path) -> Result<StorageRootBinding, StorageRootError> {
    let metadata =
        fs::symlink_metadata(pointer).map_err(|_| StorageRootError::PointerUnavailable)?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(StorageRootError::PointerUnavailable);
    }
    let pointer: StorageRootPointer = serde_json::from_reader(
        File::open(pointer).map_err(|_| StorageRootError::PointerUnavailable)?,
    )
    .map_err(|_| StorageRootError::PointerUnavailable)?;
    let binding = pointer.into_binding();
    StorageRootBinding::open_existing(
        Path::new(&binding.canonical_path),
        Some(&binding.storage_root_id),
        Some(&binding.catalog_id),
    )
}

/// 首次初始化时发布固定恢复区指针；已有指针只能与同一存储根完全一致。
///
/// 该入口不覆盖旧指针，避免启动过程把已有历史静默改投到新目录。
pub fn publish_initial_storage_root_pointer(
    pointer: &Path,
    binding: &StorageRootBinding,
) -> Result<(), StorageRootError> {
    match fs::symlink_metadata(pointer) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
            Err(StorageRootError::PointerUnavailable)
        }
        Ok(_) => {
            let current = read_storage_root_pointer(pointer)?;
            if current.storage_root_id != binding.storage_root_id
                || current.canonical_path != binding.canonical_path
                || current.catalog_id != binding.catalog_id
            {
                return Err(StorageRootError::RootIdentityMismatch);
            }
            Ok(())
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            write_storage_root_pointer(pointer, binding)
        }
        Err(_) => Err(StorageRootError::PointerUnavailable),
    }
}

/// 计算存储根占用；不会创建目录、指针或目录库。
pub fn storage_space_status(
    root: &Path,
    warning_threshold_bytes: Option<u64>,
) -> Result<StorageSpaceStatus, StorageRootError> {
    let threshold = warning_threshold_bytes.unwrap_or(DEFAULT_STORAGE_WARNING_BYTES);
    let used_bytes = tree_size(root)?;
    let warning_active = used_bytes >= threshold;
    Ok(StorageSpaceStatus {
        used_bytes,
        warning_threshold_bytes: threshold,
        warning_active,
        // 5 GB 警戒线只影响非必要自动扫描；显式用户操作不在此处被静默阻断。
        automatic_scan_paused: warning_active,
    })
}

fn migrate_storage_root_inner(
    source: &Path,
    destination: &Path,
    catalog_id: &str,
    minimum_free_bytes: u64,
    pointer: &Path,
) -> Result<StorageMigrationResult, StorageRootError> {
    let recovery_root = pointer
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .ok_or(StorageRootError::PointerUnavailable)?;
    // 重启后先冻结上次未完成迁移；不清理 staging，也不盲目再次复制或切换指针。
    match freeze_unfinished(recovery_root, MigrationKind::StorageRoot) {
        Ok(true) => return Err(StorageRootError::MigrationRecoveryRequired),
        Ok(false) => {}
        Err(_) => return Err(StorageRootError::MigrationManifestUnavailable),
    }
    let source_binding = StorageRootBinding::open_existing(source, None, Some(catalog_id))?;
    prepare_storage_root_pointer(pointer, &source_binding)?;
    let source_path = Path::new(&source_binding.canonical_path);
    if fs::symlink_metadata(destination).is_ok() {
        return Err(StorageRootError::DestinationNotEmpty);
    }
    let destination_parent = destination
        .parent()
        .ok_or(StorageRootError::DestinationUnavailable)?;
    reject_symlink_directory_chain(destination_parent)?;
    fs::create_dir_all(destination_parent).map_err(|_| StorageRootError::DestinationUnavailable)?;
    reject_symlink_directory_chain(destination_parent)?;
    let canonical_destination = destination_parent
        .canonicalize()
        .map_err(|_| StorageRootError::DestinationUnavailable)?
        .join(
            destination
                .file_name()
                .ok_or(StorageRootError::DestinationUnavailable)?,
        );
    if canonical_destination.starts_with(source_path) {
        return Err(StorageRootError::DestinationUnavailable);
    }
    let source_manifest = collect_tree_manifest(source_path)?;
    let source_bytes = tree_size(source_path)?;
    let reservation_bytes = source_bytes
        .saturating_add(required_storage_reserve_bytes(source_bytes).max(minimum_free_bytes));
    // 复制期间持有实际 reservation，避免只检查一次可用空间后被其他进程抢占。
    let _space_reservation = reserve_space(destination_parent, reservation_bytes)?;

    let staging_name = format!(
        ".{}-migration-staging-{}",
        destination
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("storage-root"),
        now_nanos()
    );
    let mut migration = MigrationJournal::create(
        recovery_root,
        MigrationKind::StorageRoot,
        &source_binding.storage_root_id,
        source_bytes,
        manifest_hash(&source_manifest),
        &staging_name,
    )
    .map_err(|_| StorageRootError::MigrationManifestUnavailable)?;
    migration
        .transition(MigrationStage::Staging, None)
        .map_err(|_| StorageRootError::MigrationManifestUnavailable)?;
    let staging = destination_parent.join(&staging_name);
    fs::create_dir(&staging).map_err(|_| StorageRootError::DestinationUnavailable)?;
    let result = copy_tree(source_path, &staging);
    let (file_hashes, copied_bytes) = result?;
    if copied_bytes != source_bytes || file_hashes != source_manifest {
        return Err(StorageRootError::VerificationFailed);
    }
    verify_tree(&staging, &file_hashes)?;
    let copied_files = file_hashes.len() as u64;
    let new_binding = StorageRootBinding {
        format_version: 1,
        // staging 与最终目标位于同一父目录，原子改名会保留目录本身的物理身份。
        storage_root_id: physical_root_id(&staging)
            .ok_or(StorageRootError::RootIdentityUnavailable)?,
        canonical_path: canonical_destination.to_string_lossy().into_owned(),
        catalog_id: catalog_id.to_string(),
        catalog_file_identity: Some(read_catalog_identity(&staging)?),
        created_at_secs: now_secs(),
    };
    write_binding(&staging, &new_binding)?;
    verify_staged_binding(&staging, &new_binding)?;
    migration
        .transition(
            MigrationStage::StagedVerified,
            Some(new_binding.storage_root_id.clone()),
        )
        .map_err(|_| StorageRootError::MigrationManifestUnavailable)?;
    // 目标目录不存在时，整棵 staging 目录一次发布，避免暴露半完成根。
    migration
        .transition(
            MigrationStage::Publishing,
            Some(new_binding.storage_root_id.clone()),
        )
        .map_err(|_| StorageRootError::MigrationManifestUnavailable)?;
    fs::rename(&staging, destination).map_err(|_| StorageRootError::CopyFailed)?;
    sync_directory(destination_parent)?;

    let verified = StorageRootBinding::open_existing(
        destination,
        Some(&new_binding.storage_root_id),
        Some(catalog_id),
    )?;
    verify_tree(destination, &file_hashes)?;
    if verified.storage_root_id != new_binding.storage_root_id {
        return Err(StorageRootError::VerificationFailed);
    }
    write_storage_root_pointer(pointer, &verified)?;
    let pointed = read_storage_root_pointer(pointer)?;
    if pointed.storage_root_id != verified.storage_root_id
        || pointed.canonical_path != verified.canonical_path
    {
        return Err(StorageRootError::VerificationFailed);
    }
    migration
        .transition(
            MigrationStage::Completed,
            Some(verified.storage_root_id.clone()),
        )
        .map_err(|_| StorageRootError::MigrationManifestUnavailable)?;
    Ok(StorageMigrationResult {
        old_root_id: source_binding.storage_root_id,
        new_root_id: new_binding.storage_root_id,
        copied_files,
        copied_bytes,
        file_hashes,
    })
}

fn manifest_hash(manifest: &BTreeMap<String, String>) -> String {
    let bytes = serde_json::to_vec(manifest).expect("文件清单仅包含字符串，序列化不应失败");
    let mut digest = Sha256::new();
    digest.update(bytes);
    hex::encode(digest.finalize())
}

fn prepare_storage_root_pointer(
    pointer: &Path,
    source_binding: &StorageRootBinding,
) -> Result<(), StorageRootError> {
    match fs::symlink_metadata(pointer) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
            Err(StorageRootError::PointerUnavailable)
        }
        Ok(_) => {
            let current = read_storage_root_pointer(pointer)?;
            if current.storage_root_id != source_binding.storage_root_id
                || current.canonical_path != source_binding.canonical_path
                || current.catalog_id != source_binding.catalog_id
            {
                return Err(StorageRootError::PointerUnavailable);
            }
            Ok(())
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            write_storage_root_pointer(pointer, source_binding)
        }
        Err(_) => Err(StorageRootError::PointerUnavailable),
    }
}

fn write_storage_root_pointer(
    pointer: &Path,
    binding: &StorageRootBinding,
) -> Result<(), StorageRootError> {
    let parent = pointer
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .ok_or(StorageRootError::PointerUnavailable)?;
    reject_symlink_directory_chain(parent)?;
    fs::create_dir_all(parent).map_err(|_| StorageRootError::PointerUnavailable)?;
    reject_symlink_directory_chain(parent)?;
    if fs::symlink_metadata(pointer)
        .map(|metadata| metadata.file_type().is_symlink())
        .unwrap_or(false)
    {
        return Err(StorageRootError::PointerUnavailable);
    }
    let temporary = parent.join(format!(
        ".storage-root-pointer.tmp-{}-{}",
        now_nanos(),
        std::process::id()
    ));
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .map_err(|_| StorageRootError::PointerUnavailable)?;
        let pointer_value = StorageRootPointer::from_binding(binding);
        serde_json::to_writer_pretty(&mut file, &pointer_value)
            .map_err(|_| StorageRootError::PointerUnavailable)?;
        file.write_all(b"\n")
            .map_err(|_| StorageRootError::PointerUnavailable)?;
        file.sync_all()
            .map_err(|_| StorageRootError::PointerUnavailable)?;
        drop(file);
        publish_pointer(&temporary, pointer)?;
        sync_directory(parent)
    })();
    if result.is_err() {
        // 指针发布失败时临时文件没有恢复价值，避免每次失败留下孤儿文件。
        let _ = fs::remove_file(&temporary);
    }
    result
}

fn publish_pointer(temporary: &Path, pointer: &Path) -> Result<(), StorageRootError> {
    crate::atomic_publish::publish_replacing(temporary, pointer)
        .map_err(|_| StorageRootError::PointerUnavailable)
}

fn reject_symlink_directory_chain(path: &Path) -> Result<(), StorageRootError> {
    let mut current = Some(path);
    while let Some(candidate) = current {
        match fs::symlink_metadata(candidate) {
            Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
                return Err(StorageRootError::DestinationUnavailable)
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return Err(StorageRootError::DestinationUnavailable),
        }
        let Some(parent) = candidate.parent() else {
            break;
        };
        if parent == candidate {
            break;
        }
        current = Some(parent);
    }
    Ok(())
}

/// 一个需要在指定目录所在卷预留的空间预算。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpaceReservationRequest {
    pub path: PathBuf,
    pub bytes: u64,
}

impl SpaceReservationRequest {
    pub fn new(path: impl Into<PathBuf>, bytes: u64) -> Self {
        Self {
            path: path.into(),
            bytes,
        }
    }
}

/// 一次操作的全部空间租约；同一卷上的多个预算只创建一个占位文件。
pub struct SpaceReservationSet {
    reservations: Vec<SpaceReservation>,
}

impl SpaceReservationSet {
    pub fn reservation_count(&self) -> usize {
        self.reservations.len()
    }
}

pub fn reserve_space(path: &Path, bytes: u64) -> Result<SpaceReservation, StorageRootError> {
    let mut reservations =
        reserve_space_on_volumes(&[SpaceReservationRequest::new(path.to_path_buf(), bytes)])?;
    reservations
        .reservations
        .pop()
        .ok_or(StorageRootError::DestinationUnavailable)
}

/// 按物理卷合并预算并在整个操作期间持有实际空间占用。
///
/// 事务/WAL 与备份目录可能位于不同卷；只分别检查两个路径会把同卷预算重复
/// 或把异卷预算漏掉。这里先用目录身份的卷序列号分组，再逐卷创建占位文件。
pub fn reserve_space_on_volumes(
    requests: &[SpaceReservationRequest],
) -> Result<SpaceReservationSet, StorageRootError> {
    let provider = PlatformFileIdentityProvider::new();
    let mut budgets = BTreeMap::<u64, (PathBuf, u64)>::new();
    for request in requests {
        if request.bytes == 0 {
            continue;
        }
        let identity = provider
            .read_file_identity(&request.path)
            .ok_or(StorageRootError::DestinationUnavailable)?;
        let entry = budgets
            .entry(identity.volume_serial)
            .or_insert_with(|| (request.path.clone(), 0));
        entry.1 = entry.1.saturating_add(request.bytes);
    }

    for (path, bytes) in budgets.values() {
        if available_space(path)? < *bytes {
            return Err(StorageRootError::InsufficientSpace);
        }
    }

    let mut reservations = Vec::with_capacity(budgets.len());
    for (path, bytes) in budgets.into_values() {
        let reservation = path.join(format!(
            ".space-reservation-{}-{}",
            now_nanos(),
            std::process::id()
        ));
        let file = match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&reservation)
        {
            Ok(file) => file,
            Err(_) => return Err(StorageRootError::DestinationUnavailable),
        };
        if file.set_len(bytes).is_err() {
            let _ = fs::remove_file(&reservation);
            return Err(StorageRootError::InsufficientSpace);
        }
        reservations.push(SpaceReservation { path: reservation });
    }
    Ok(SpaceReservationSet { reservations })
}

pub struct SpaceReservation {
    path: PathBuf,
}

impl Drop for SpaceReservation {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

fn write_binding(root: &Path, binding: &StorageRootBinding) -> Result<(), StorageRootError> {
    // 临时文件使用唯一名称和 create_new，避免固定路径被旧文件或 symlink 劫持。
    let temp = root.join(format!(
        ".storage-root.json.tmp-{}-{}",
        now_nanos(),
        std::process::id()
    ));
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)
            .map_err(|_| StorageRootError::DestinationUnavailable)?;
        serde_json::to_writer_pretty(&mut file, binding)
            .map_err(|_| StorageRootError::BindingInvalid)?;
        file.write_all(b"\n")
            .map_err(|_| StorageRootError::DestinationUnavailable)?;
        file.sync_all()
            .map_err(|_| StorageRootError::DestinationUnavailable)?;
        publish_binding_without_replace(&temp, &root.join("storage-root.json"))?;
        sync_directory(root)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

fn publish_binding_without_replace(
    temp: &Path,
    destination: &Path,
) -> Result<(), StorageRootError> {
    // hard link 创建是原子的且不会替换已有目标；不支持硬链接时安全拒绝发布。
    fs::hard_link(temp, destination).map_err(|_| StorageRootError::DestinationUnavailable)?;
    fs::remove_file(temp).map_err(|_| StorageRootError::DestinationUnavailable)?;
    Ok(())
}

fn copy_tree(
    source: &Path,
    destination: &Path,
) -> Result<(BTreeMap<String, String>, u64), StorageRootError> {
    let mut hashes = BTreeMap::new();
    let mut bytes = 0_u64;
    copy_tree_inner(source, source, destination, &mut hashes, &mut bytes)?;
    if collect_tree_manifest(source)? != hashes {
        return Err(StorageRootError::VerificationFailed);
    }
    Ok((hashes, bytes))
}

fn copy_tree_inner(
    source_root: &Path,
    source: &Path,
    destination: &Path,
    hashes: &mut BTreeMap<String, String>,
    bytes: &mut u64,
) -> Result<(), StorageRootError> {
    for entry in fs::read_dir(source).map_err(|_| StorageRootError::CopyFailed)? {
        let entry = entry.map_err(|_| StorageRootError::CopyFailed)?;
        let source_path = entry.path();
        let relative = source_path
            .strip_prefix(source_root)
            .map_err(|_| StorageRootError::CopyFailed)?;
        let destination_path = destination.join(relative);
        let metadata =
            fs::symlink_metadata(&source_path).map_err(|_| StorageRootError::CopyFailed)?;
        if metadata.file_type().is_symlink() {
            return Err(StorageRootError::VerificationFailed);
        }
        if metadata.is_dir() {
            fs::create_dir_all(&destination_path).map_err(|_| StorageRootError::CopyFailed)?;
            copy_tree_inner(source_root, &source_path, destination, hashes, bytes)?;
        } else if metadata.is_file() {
            if relative == Path::new("storage-root.json") {
                continue;
            }
            if let Some(parent) = destination_path.parent() {
                fs::create_dir_all(parent).map_err(|_| StorageRootError::CopyFailed)?;
            }
            let source_hash_before = hash_file(&source_path)?;
            fs::copy(&source_path, &destination_path).map_err(|_| StorageRootError::CopyFailed)?;
            let hash = hash_file(&destination_path)?;
            let source_hash_after = hash_file(&source_path)?;
            if hash != source_hash_before || source_hash_before != source_hash_after {
                return Err(StorageRootError::VerificationFailed);
            }
            *bytes = bytes.saturating_add(metadata.len());
            hashes.insert(relative.to_string_lossy().replace('\\', "/"), hash);
        }
    }
    Ok(())
}

fn collect_tree_manifest(source: &Path) -> Result<BTreeMap<String, String>, StorageRootError> {
    let mut manifest = BTreeMap::new();
    collect_tree_manifest_inner(source, source, &mut manifest)?;
    Ok(manifest)
}

fn collect_tree_manifest_inner(
    root: &Path,
    current: &Path,
    manifest: &mut BTreeMap<String, String>,
) -> Result<(), StorageRootError> {
    for entry in fs::read_dir(current).map_err(|_| StorageRootError::VerificationFailed)? {
        let entry = entry.map_err(|_| StorageRootError::VerificationFailed)?;
        let path = entry.path();
        let metadata =
            fs::symlink_metadata(&path).map_err(|_| StorageRootError::VerificationFailed)?;
        if metadata.file_type().is_symlink() {
            return Err(StorageRootError::VerificationFailed);
        }
        let relative = path
            .strip_prefix(root)
            .map_err(|_| StorageRootError::VerificationFailed)?;
        if metadata.is_dir() {
            collect_tree_manifest_inner(root, &path, manifest)?;
        } else if metadata.is_file() && relative != Path::new("storage-root.json") {
            manifest.insert(
                relative.to_string_lossy().replace('\\', "/"),
                hash_file(&path)?,
            );
        }
    }
    Ok(())
}

fn tree_size(root: &Path) -> Result<u64, StorageRootError> {
    let mut total = 0_u64;
    tree_size_inner(root, root, &mut total)?;
    Ok(total)
}

fn tree_size_inner(root: &Path, current: &Path, total: &mut u64) -> Result<(), StorageRootError> {
    for entry in fs::read_dir(current).map_err(|_| StorageRootError::VerificationFailed)? {
        let entry = entry.map_err(|_| StorageRootError::VerificationFailed)?;
        let path = entry.path();
        let metadata =
            fs::symlink_metadata(&path).map_err(|_| StorageRootError::VerificationFailed)?;
        if metadata.file_type().is_symlink() {
            return Err(StorageRootError::VerificationFailed);
        }
        let relative = path
            .strip_prefix(root)
            .map_err(|_| StorageRootError::VerificationFailed)?;
        if metadata.is_dir() {
            tree_size_inner(root, &path, total)?;
        } else if metadata.is_file() && relative != Path::new("storage-root.json") {
            *total = total.saturating_add(metadata.len());
        }
    }
    Ok(())
}

fn verify_tree(root: &Path, expected: &BTreeMap<String, String>) -> Result<(), StorageRootError> {
    if collect_tree_manifest(root)? != *expected {
        return Err(StorageRootError::VerificationFailed);
    }
    Ok(())
}

fn verify_staged_binding(
    staging: &Path,
    expected: &StorageRootBinding,
) -> Result<(), StorageRootError> {
    let binding: StorageRootBinding = serde_json::from_reader(
        File::open(staging.join("storage-root.json"))
            .map_err(|_| StorageRootError::VerificationFailed)?,
    )
    .map_err(|_| StorageRootError::VerificationFailed)?;
    if binding != *expected
        || binding.storage_root_id
            != physical_root_id(staging).ok_or(StorageRootError::RootIdentityUnavailable)?
        || binding.catalog_id.is_empty()
        || binding.catalog_file_identity.as_ref() != Some(&read_catalog_identity(staging)?)
    {
        return Err(StorageRootError::VerificationFailed);
    }
    Ok(())
}

fn hash_file(path: &Path) -> Result<String, StorageRootError> {
    let mut file = File::open(path).map_err(|_| StorageRootError::VerificationFailed)?;
    let mut digest = Sha256::new();
    // 存储根迁移校验可能读取大文件，缓冲区放在堆上避免栈溢出。
    let mut buffer = vec![0_u8; 1024 * 1024];
    loop {
        let count = file
            .read(&mut buffer)
            .map_err(|_| StorageRootError::VerificationFailed)?;
        if count == 0 {
            break;
        }
        digest.update(&buffer[..count]);
    }
    Ok(hex::encode(digest.finalize()))
}

fn physical_root_id(root: &Path) -> Option<String> {
    let mut digest = Sha256::new();
    let provider = PlatformFileIdentityProvider::new();
    let identity = provider.read_file_identity(root)?;
    digest.update(identity.volume_serial.to_le_bytes());
    digest.update(identity.file_index_high.to_le_bytes());
    digest.update(identity.file_index_low.to_le_bytes());
    Some(format!("root-{}", hex::encode(digest.finalize())))
}

fn read_catalog_identity(root: &Path) -> Result<FileIdentity, StorageRootError> {
    let catalog = resolve_current_catalog_path(root)
        .map_err(|_| StorageRootError::CatalogIdentityMismatch)?;
    PlatformFileIdentityProvider::new()
        .read_file_identity(&catalog)
        .ok_or(StorageRootError::CatalogIdentityMismatch)
}

fn available_space(path: &Path) -> Result<u64, StorageRootError> {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        let wide: Vec<u16> = path
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        let mut free = 0_u64;
        let ok = unsafe {
            windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW(
                wide.as_ptr(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                &mut free,
            )
        };
        if ok == 0 {
            return Err(StorageRootError::DestinationUnavailable);
        }
        Ok(free)
    }
    #[cfg(not(windows))]
    {
        let _ = path;
        Ok(u64::MAX)
    }
}

fn sync_directory(path: &Path) -> Result<(), StorageRootError> {
    #[cfg(unix)]
    {
        File::open(path)
            .map_err(|_| StorageRootError::DestinationUnavailable)?
            .sync_all()
            .map_err(|_| StorageRootError::DestinationUnavailable)?;
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
    Ok(())
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_secs())
        .unwrap_or(0)
}

fn now_nanos() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_nanos())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir_in;

    fn fixture_catalog_path(root: &Path) -> PathBuf {
        root.join("catalog")
            .join("generations")
            .join("generation-1")
            .join("catalog.db")
    }

    fn initialize_fixture_root(root: &Path, catalog_id: &str) -> StorageRootBinding {
        let catalog_path = fixture_catalog_path(root);
        fs::create_dir_all(catalog_path.parent().unwrap()).unwrap();
        fs::write(&catalog_path, b"catalog").unwrap();
        fs::write(
            root.join("catalog").join("current.json"),
            b"{\"generation_id\":\"generation-1\"}",
        )
        .unwrap();
        StorageRootBinding::initialize(root, catalog_id).unwrap()
    }

    #[test]
    fn missing_root_does_not_get_created_by_open() {
        let root = tempfile::tempdir().unwrap().path().join("missing");
        assert!(matches!(
            StorageRootBinding::open_existing(&root, None, None),
            Err(StorageRootError::RootMissing)
        ));
        assert!(!root.exists());
    }

    #[test]
    fn initialize_missing_catalog_removes_new_empty_root() {
        let parent = tempfile::tempdir().unwrap();
        let root = parent.path().join("new-root");

        assert!(matches!(
            StorageRootBinding::initialize(&root, "catalog-a"),
            Err(StorageRootError::CatalogIdentityMismatch)
        ));
        assert!(!root.exists());
    }

    #[test]
    fn initialize_rejects_symlinked_root_before_writing() {
        let outside = tempfile::tempdir().unwrap();
        initialize_fixture_root(outside.path(), "catalog-a");
        let parent = tempfile::tempdir().unwrap();
        let link = parent.path().join("linked-root");

        #[cfg(windows)]
        let link_result = std::os::windows::fs::symlink_dir(outside.path(), &link);
        #[cfg(unix)]
        let link_result = std::os::unix::fs::symlink(outside.path(), &link);
        #[cfg(not(any(windows, unix)))]
        return;
        if link_result.is_err() {
            return;
        }

        assert!(matches!(
            StorageRootBinding::initialize(&link, "catalog-a"),
            Err(StorageRootError::DestinationUnavailable)
        ));
    }

    #[test]
    fn open_existing_rejects_symlinked_root_before_canonicalizing() {
        let outside = tempfile::tempdir().unwrap();
        initialize_fixture_root(outside.path(), "catalog-a");
        let parent = tempfile::tempdir().unwrap();
        let link = parent.path().join("linked-root");

        #[cfg(windows)]
        let link_result = std::os::windows::fs::symlink_dir(outside.path(), &link);
        #[cfg(unix)]
        let link_result = std::os::unix::fs::symlink(outside.path(), &link);
        #[cfg(not(any(windows, unix)))]
        return;
        if link_result.is_err() {
            return;
        }

        assert!(matches!(
            StorageRootBinding::open_existing(&link, None, Some("catalog-a")),
            Err(StorageRootError::DestinationUnavailable)
        ));
    }

    #[test]
    fn migration_keeps_old_root_and_verifies_new_root() {
        let source = tempfile::tempdir().unwrap();
        initialize_fixture_root(source.path(), "catalog-a");
        fs::create_dir_all(source.path().join("snapshots")).unwrap();
        fs::write(source.path().join("snapshots").join("one"), b"snapshot").unwrap();
        let destination_parent = tempfile::tempdir().unwrap();
        let destination = destination_parent.path().join("new-root");
        let recovery = tempfile::tempdir().unwrap();
        let pointer = recovery.path().join("storage-root-pointer.json");
        let result =
            migrate_storage_root(source.path(), &destination, "catalog-a", 0, &pointer).unwrap();
        assert_ne!(result.old_root_id, result.new_root_id);
        assert_eq!(
            fs::read(fixture_catalog_path(source.path())).unwrap(),
            b"catalog"
        );
        assert_eq!(
            fs::read(fixture_catalog_path(&destination)).unwrap(),
            b"catalog"
        );
        assert!(StorageRootBinding::open_existing(
            &destination,
            Some(&result.new_root_id),
            Some("catalog-a")
        )
        .is_ok());
    }

    #[test]
    fn migration_pointer_switches_only_after_new_root_verifies() {
        let source = tempfile::tempdir().unwrap();
        let source_binding = initialize_fixture_root(source.path(), "catalog-a");
        let destination_parent = tempfile::tempdir().unwrap();
        let destination = destination_parent.path().join("new-root");
        let recovery = tempfile::tempdir().unwrap();
        let pointer = recovery.path().join("storage-root-pointer.json");

        let result =
            migrate_storage_root(source.path(), &destination, "catalog-a", 0, &pointer).unwrap();

        let current = read_storage_root_pointer(&pointer).unwrap();
        assert_eq!(current.storage_root_id, result.new_root_id);
        assert_eq!(
            current.canonical_path,
            destination.canonicalize().unwrap().to_string_lossy()
        );
        assert_ne!(current.storage_root_id, source_binding.storage_root_id);
        assert!(fixture_catalog_path(source.path()).is_file());

        let operation_dirs = fs::read_dir(recovery.path().join("migration-manifests"))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(operation_dirs.len(), 1);
        let operation_id = operation_dirs[0].file_name().to_string_lossy().into_owned();
        let manifest =
            crate::migration_manifest::read_latest_for_test(recovery.path(), &operation_id)
                .unwrap()
                .unwrap();
        assert_eq!(manifest.kind, MigrationKind::StorageRoot);
        assert_eq!(manifest.stage, MigrationStage::Completed);
        assert_eq!(
            manifest.destination_id.as_deref(),
            Some(result.new_root_id.as_str())
        );
    }

    #[cfg(windows)]
    #[test]
    fn migration_verifies_copy_and_pointer_across_volumes() {
        let source = tempfile::tempdir().unwrap();
        initialize_fixture_root(source.path(), "catalog-a");
        fs::create_dir_all(source.path().join("snapshots")).unwrap();
        fs::write(
            source.path().join("snapshots").join("cross-volume.txt"),
            b"cross-volume fixture",
        )
        .unwrap();

        let provider = PlatformFileIdentityProvider::new();
        let Some(source_volume) = provider
            .read_file_identity(source.path())
            .map(|identity| identity.volume_serial)
        else {
            return;
        };

        // 测试只寻找可写的不同卷；找不到时跳过，不把机器卷布局变成失败条件。
        let destination_parent = ('A'..='Z').find_map(|letter| {
            let volume_root = PathBuf::from(format!("{letter}:\\"));
            if !volume_root.is_dir() {
                return None;
            }
            let candidate = tempdir_in(&volume_root).ok()?;
            let candidate_volume = provider
                .read_file_identity(candidate.path())
                .map(|identity| identity.volume_serial);
            if candidate_volume != Some(source_volume) {
                Some(candidate)
            } else {
                None
            }
        });
        let Some(destination_parent) = destination_parent else {
            return;
        };

        let destination = destination_parent.path().join("new-root");
        let recovery = tempfile::tempdir().unwrap();
        let pointer = recovery.path().join("storage-root-pointer.json");
        let source_snapshot = collect_tree_manifest(source.path()).unwrap();

        let result =
            migrate_storage_root(source.path(), &destination, "catalog-a", 0, &pointer).unwrap();

        assert_ne!(result.old_root_id, result.new_root_id);
        assert_eq!(
            collect_tree_manifest(source.path()).unwrap(),
            source_snapshot
        );
        assert_eq!(
            collect_tree_manifest(&destination).unwrap(),
            source_snapshot
        );
        assert_eq!(
            read_storage_root_pointer(&pointer).unwrap().storage_root_id,
            result.new_root_id
        );
        assert!(StorageRootBinding::open_existing(
            &destination,
            Some(&result.new_root_id),
            Some("catalog-a")
        )
        .is_ok());
    }

    #[test]
    fn unfinished_migration_is_frozen_without_deleting_staging() {
        let source = tempfile::tempdir().unwrap();
        initialize_fixture_root(source.path(), "catalog-a");
        let destination_parent = tempfile::tempdir().unwrap();
        let destination = destination_parent.path().join("new-root");
        let recovery = tempfile::tempdir().unwrap();
        let pointer = recovery.path().join("storage-root-pointer.json");
        let source_binding = StorageRootBinding::open_existing(source.path(), None, None).unwrap();
        fs::write(
            &pointer,
            serde_json::to_vec(&StorageRootPointer::from_binding(&source_binding)).unwrap(),
        )
        .unwrap();

        let staging_name = ".new-root-migration-staging-crash";
        let mut journal = MigrationJournal::create(
            recovery.path(),
            MigrationKind::StorageRoot,
            &source_binding.storage_root_id,
            1,
            "source-manifest",
            staging_name,
        )
        .unwrap();
        journal.transition(MigrationStage::Staging, None).unwrap();
        let staging = destination_parent.path().join(staging_name);
        fs::create_dir(&staging).unwrap();
        fs::write(staging.join("partial"), b"preserve").unwrap();

        assert!(matches!(
            migrate_storage_root(source.path(), &destination, "catalog-a", 0, &pointer),
            Err(StorageRootError::MigrationRecoveryRequired)
        ));
        assert!(staging.is_dir());
        assert_eq!(fs::read(staging.join("partial")).unwrap(), b"preserve");
        assert_eq!(
            read_storage_root_pointer(&pointer).unwrap().storage_root_id,
            source_binding.storage_root_id
        );
        let operation_id = journal.latest().operation_id.clone();
        assert_eq!(
            crate::migration_manifest::read_latest_for_test(recovery.path(), &operation_id)
                .unwrap()
                .unwrap()
                .stage,
            MigrationStage::ManualRecoveryRequired
        );
    }

    #[test]
    fn migration_pointer_mismatch_does_not_copy_to_second_root() {
        let source = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        initialize_fixture_root(source.path(), "catalog-a");
        let other_binding = initialize_fixture_root(other.path(), "catalog-a");
        let recovery = tempfile::tempdir().unwrap();
        let pointer = recovery.path().join("storage-root-pointer.json");
        let pointer_value = StorageRootPointer::from_binding(&other_binding);
        fs::write(&pointer, serde_json::to_vec(&pointer_value).unwrap()).unwrap();
        let destination = tempfile::tempdir().unwrap().path().join("new-root");

        assert!(matches!(
            migrate_storage_root(source.path(), &destination, "catalog-a", 0, &pointer,),
            Err(StorageRootError::PointerUnavailable)
        ));
        assert!(!destination.exists());
    }

    #[test]
    fn missing_pointee_fails_closed_without_creating_replacement_root() {
        let source = tempfile::tempdir().unwrap();
        let binding = initialize_fixture_root(source.path(), "catalog-a");
        let recovery = tempfile::tempdir().unwrap();
        let pointer = recovery.path().join("storage-root-pointer.json");
        let pointer_value = StorageRootPointer::from_binding(&binding);
        fs::write(&pointer, serde_json::to_vec(&pointer_value).unwrap()).unwrap();
        fs::remove_dir_all(source.path()).unwrap();

        assert!(matches!(
            read_storage_root_pointer(&pointer),
            Err(StorageRootError::RootMissing)
                | Err(StorageRootError::DestinationUnavailable)
                | Err(StorageRootError::BindingMissing)
        ));
    }

    #[test]
    fn root_identity_does_not_change_when_contents_change() {
        let root = tempfile::tempdir().unwrap();
        let first = physical_root_id(root.path()).unwrap();
        fs::write(root.path().join("marker"), b"catalog").unwrap();
        let second = physical_root_id(root.path()).unwrap();
        assert_eq!(first, second);
    }

    #[test]
    fn replacing_root_at_same_path_does_not_reuse_old_binding() {
        let parent = tempfile::tempdir().unwrap();
        let root = parent.path().join("root");
        let original = initialize_fixture_root(&root, "catalog-a");
        let binding_bytes = fs::read(root.join("storage-root.json")).unwrap();

        fs::remove_dir_all(&root).unwrap();
        fs::create_dir_all(&root).unwrap();
        let catalog_path = fixture_catalog_path(&root);
        fs::create_dir_all(catalog_path.parent().unwrap()).unwrap();
        fs::write(&catalog_path, b"catalog").unwrap();
        fs::write(
            root.join("catalog").join("current.json"),
            b"{\"generation_id\":\"generation-1\"}",
        )
        .unwrap();
        fs::write(root.join("storage-root.json"), binding_bytes).unwrap();

        let result =
            StorageRootBinding::open_existing(&root, Some(&original.storage_root_id), None);
        assert!(matches!(
            result,
            Err(StorageRootError::RootIdentityMismatch)
                | Err(StorageRootError::CatalogIdentityMismatch)
        ));
    }

    #[test]
    fn initialize_does_not_overwrite_existing_binding() {
        let root = tempfile::tempdir().unwrap();
        let first = initialize_fixture_root(root.path(), "catalog-a");
        assert_eq!(
            StorageRootBinding::initialize(root.path(), "catalog-a").unwrap(),
            first
        );
        assert!(matches!(
            StorageRootBinding::initialize(root.path(), "catalog-b"),
            Err(StorageRootError::CatalogIdentityMismatch)
        ));
    }

    #[test]
    fn binding_write_does_not_use_preexisting_temporary_marker() {
        let root = tempfile::tempdir().unwrap();
        let marker = root.path().join("storage-root.json.tmp");
        fs::write(&marker, b"preserve").unwrap();
        let binding = StorageRootBinding {
            format_version: 1,
            storage_root_id: "root-test".to_string(),
            canonical_path: root
                .path()
                .canonicalize()
                .unwrap()
                .to_string_lossy()
                .into_owned(),
            catalog_id: "catalog-a".to_string(),
            catalog_file_identity: None,
            created_at_secs: 1,
        };

        write_binding(root.path(), &binding).unwrap();

        assert_eq!(fs::read(marker).unwrap(), b"preserve");
        assert_eq!(
            serde_json::from_reader::<_, StorageRootBinding>(
                File::open(root.path().join("storage-root.json")).unwrap()
            )
            .unwrap(),
            binding
        );
    }

    #[test]
    fn binding_publication_does_not_replace_existing_file() {
        let root = tempfile::tempdir().unwrap();
        let destination = root.path().join("storage-root.json");
        fs::write(&destination, b"existing").unwrap();
        let binding = StorageRootBinding {
            format_version: 1,
            storage_root_id: "root-test".to_string(),
            canonical_path: root
                .path()
                .canonicalize()
                .unwrap()
                .to_string_lossy()
                .into_owned(),
            catalog_id: "catalog-a".to_string(),
            catalog_file_identity: None,
            created_at_secs: 1,
        };

        assert!(write_binding(root.path(), &binding).is_err());
        assert_eq!(fs::read(destination).unwrap(), b"existing");
        assert!(!fs::read_dir(root.path())
            .unwrap()
            .filter_map(Result::ok)
            .any(|entry| entry
                .file_name()
                .to_string_lossy()
                .starts_with(".storage-root.json.tmp-")));
    }

    #[test]
    fn binding_write_does_not_follow_preexisting_temporary_symlink() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let outside_file = outside.path().join("outside");
        fs::write(&outside_file, b"outside").unwrap();
        let marker = root.path().join("storage-root.json.tmp");
        #[cfg(windows)]
        let link_result = std::os::windows::fs::symlink_file(&outside_file, &marker);
        #[cfg(unix)]
        let link_result = std::os::unix::fs::symlink(&outside_file, &marker);
        #[cfg(not(any(windows, unix)))]
        return;
        if link_result.is_err() {
            return;
        }
        let binding = StorageRootBinding {
            format_version: 1,
            storage_root_id: "root-test".to_string(),
            canonical_path: root
                .path()
                .canonicalize()
                .unwrap()
                .to_string_lossy()
                .into_owned(),
            catalog_id: "catalog-a".to_string(),
            catalog_file_identity: None,
            created_at_secs: 1,
        };

        write_binding(root.path(), &binding).unwrap();

        assert_eq!(fs::read(outside_file).unwrap(), b"outside");
        assert!(fs::symlink_metadata(marker)
            .unwrap()
            .file_type()
            .is_symlink());
    }

    #[test]
    fn space_reservation_is_removed_only_after_guard_drop() {
        let root = tempfile::tempdir().unwrap();
        let reservation = reserve_space(root.path(), 8).unwrap();
        assert!(reservation.path.exists());
        let path = reservation.path.clone();
        drop(reservation);
        assert!(!path.exists());
    }

    #[test]
    fn space_reservation_merges_budgets_on_same_volume() {
        let first = tempfile::tempdir().unwrap();
        let second = tempfile::tempdir().unwrap();
        let requests = [
            SpaceReservationRequest::new(first.path(), 8),
            SpaceReservationRequest::new(second.path(), 16),
        ];

        let reservations = reserve_space_on_volumes(&requests).unwrap();

        // 临时目录在同一测试卷时只能生成一个占位文件，证明预算已先合并。
        assert_eq!(reservations.reservation_count(), 1);
        let entries = first
            .path()
            .read_dir()
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".space-reservation-")
            })
            .collect::<Vec<_>>();
        let other_entries = second
            .path()
            .read_dir()
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".space-reservation-")
            })
            .collect::<Vec<_>>();
        let reservation_path = entries
            .first()
            .map(|entry| entry.path())
            .or_else(|| other_entries.first().map(|entry| entry.path()))
            .unwrap();
        assert_eq!(fs::metadata(reservation_path).unwrap().len(), 24);
    }

    #[test]
    fn space_reservation_failure_leaves_no_reservation_files() {
        let root = tempfile::tempdir().unwrap();

        let result = reserve_space(root.path(), u64::MAX);

        assert!(matches!(result, Err(StorageRootError::InsufficientSpace)));
        let leftovers = root
            .path()
            .read_dir()
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".space-reservation-")
            })
            .collect::<Vec<_>>();
        // 空间预留失败后，不得留下会影响后续空间判断的占位文件。
        assert!(leftovers.is_empty());
    }

    #[test]
    fn storage_warning_reports_usage_without_creating_side_effects() {
        let root = tempfile::tempdir().unwrap();
        fs::write(root.path().join("history.bin"), [0_u8; 16]).unwrap();

        let status = storage_space_status(root.path(), Some(8)).unwrap();

        assert_eq!(status.used_bytes, 16);
        assert_eq!(status.warning_threshold_bytes, 8);
        assert!(status.warning_active);
        assert!(status.automatic_scan_paused);
        assert_eq!(root.path().read_dir().unwrap().count(), 1);
    }

    #[test]
    fn storage_reserve_uses_twenty_percent_with_a_fixed_floor() {
        assert_eq!(
            required_storage_reserve_bytes(0),
            MINIMUM_STORAGE_RESERVE_BYTES
        );
        assert_eq!(
            required_storage_reserve_bytes(1024 * 1024 * 1024),
            MINIMUM_STORAGE_RESERVE_BYTES
        );
        assert_eq!(
            required_storage_reserve_bytes(5 * 1024 * 1024 * 1024),
            1024 * 1024 * 1024
        );
    }

    #[test]
    fn migration_rejects_destination_inside_source_root() {
        let source = tempfile::tempdir().unwrap();
        initialize_fixture_root(source.path(), "catalog-a");
        let destination = source.path().join("nested").join("new-root");
        let recovery = tempfile::tempdir().unwrap();
        let pointer = recovery.path().join("pointer.json");
        assert!(matches!(
            migrate_storage_root(source.path(), &destination, "catalog-a", 0, &pointer,),
            Err(StorageRootError::DestinationUnavailable)
        ));
        assert!(!destination.exists());
    }

    #[test]
    fn migration_rejects_symlink_entries_in_source() {
        let source = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        initialize_fixture_root(source.path(), "catalog-a");
        let link = source.path().join("link.db");
        #[cfg(windows)]
        let link_result = std::os::windows::fs::symlink_file(outside.path().join("outside"), &link);
        #[cfg(unix)]
        let link_result = std::os::unix::fs::symlink(outside.path().join("outside"), &link);
        #[cfg(not(any(windows, unix)))]
        return;
        if link_result.is_err() {
            return;
        }
        let destination_parent = tempfile::tempdir().unwrap();
        let destination = destination_parent.path().join("new-root");
        let recovery = tempfile::tempdir().unwrap();
        let pointer = recovery.path().join("pointer.json");
        assert!(matches!(
            migrate_storage_root(source.path(), &destination, "catalog-a", 0, &pointer,),
            Err(StorageRootError::VerificationFailed)
        ));
        assert!(!destination.exists());
        assert!(destination_parent
            .path()
            .read_dir()
            .unwrap()
            .filter_map(Result::ok)
            .any(|entry| entry
                .file_name()
                .to_string_lossy()
                .starts_with(".new-root-migration-staging-")));
    }

    #[test]
    fn failed_pointer_publication_removes_temporary_file() {
        let root = tempfile::tempdir().unwrap();
        let pointer = root.path().join("pointer.json");
        fs::create_dir(&pointer).unwrap();
        let binding = StorageRootBinding {
            format_version: 1,
            storage_root_id: "root-test".to_string(),
            canonical_path: root
                .path()
                .canonicalize()
                .unwrap()
                .to_string_lossy()
                .into_owned(),
            catalog_id: "catalog-test".to_string(),
            catalog_file_identity: None,
            created_at_secs: 1,
        };

        assert!(write_storage_root_pointer(&pointer, &binding).is_err());
        assert!(!root.path().read_dir().unwrap().any(|entry| {
            entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".storage-root-pointer.tmp-")
        }));
    }
}
