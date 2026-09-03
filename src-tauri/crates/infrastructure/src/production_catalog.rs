//! RealReadPreview 的生产目录库启动。
//!
//! 源数据库 raw key 与 Trae Sync 目录库密钥严格分离。目录库密钥使用系统随机数
//! 生成，只以当前 Windows Profile 的 DPAPI 密文落盘；固定恢复区指针是启动时唯一入口。

use std::fs;
use std::path::{Path, PathBuf};

use openssl::rand::rand_bytes;
use traesync_ports::{KeyWrapperPort, KeyWrapperRequest};

use crate::catalog::{
    ensure_catalog_initialized, initialize_catalog_identity, verify_catalog_identity,
    CatalogPathError,
};
use crate::catalog_path::resolve_current_catalog_path;
use crate::key_wrapper::DpapiKeyWrapper;
use crate::operation_lease::OperationLease;
use crate::storage_root::{
    publish_initial_storage_root_pointer, read_storage_root_pointer, StorageRootBinding,
};

const KEY_GENERATION: u32 = 1;
const STARTUP_LEASE_LOCATION_ID: &str = "catalog-startup";

/// 组合根启动后持有的最小目录库运行材料；密钥不实现序列化或 Debug。
#[derive(Clone)]
pub struct ProductionCatalogRuntime {
    pub storage_root: PathBuf,
    pub recovery_root: PathBuf,
    pub catalog_id: String,
    pub catalog_key: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProductionCatalogError {
    UnsupportedPlatform,
    LocalAppDataUnavailable,
    ExistingStateIncomplete,
    RandomGenerationFailed,
    CatalogInitializationFailed,
    KeyProtectionFailed,
    PointerPublicationFailed,
    OperationLeaseUnavailable,
    CatalogWriteProtocolUpgradeRequired,
}

impl ProductionCatalogError {
    /// 返回启动层可安全传递给 UI 的稳定错误码。
    pub const fn code(self) -> &'static str {
        match self {
            Self::CatalogWriteProtocolUpgradeRequired => "catalog_write_protocol_upgrade_required",
            _ => "production_catalog_unavailable",
        }
    }
}

impl std::fmt::Display for ProductionCatalogError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let message = match self {
            Self::UnsupportedPlatform => "生产目录库仅支持 Windows",
            Self::LocalAppDataUnavailable => "本地应用数据目录不可用",
            Self::ExistingStateIncomplete => "已有目录库状态不完整，需要人工检查",
            Self::RandomGenerationFailed => "目录库身份生成失败",
            Self::CatalogInitializationFailed => "目录库初始化或验证失败",
            Self::KeyProtectionFailed => "目录库密钥保护失败",
            Self::PointerPublicationFailed => "存储根指针发布失败",
            Self::OperationLeaseUnavailable => "目录库或数据位置正在被其他实例使用",
            Self::CatalogWriteProtocolUpgradeRequired => "当前目录库采用了此版本不支持的写入协议",
        };
        formatter.write_str(message)
    }
}

impl ProductionCatalogRuntime {
    /// 重新验证生产存储根和固定恢复区后取得跨进程租约。
    ///
    /// 恢复区路径来自启动期固定运行材料，不接受前端或命令参数注入。
    pub fn acquire_operation_lease(
        &self,
        data_location_id: &str,
    ) -> Result<OperationLease, ProductionCatalogError> {
        StorageRootBinding::open_existing(&self.storage_root, None, Some(&self.catalog_id))
            .map_err(|_| ProductionCatalogError::ExistingStateIncomplete)?;
        let canonical_recovery = validate_recovery_root(&self.storage_root, &self.recovery_root)?;

        OperationLease::acquire_bound(&canonical_recovery, &self.storage_root, data_location_id)
            .map_err(|_| ProductionCatalogError::OperationLeaseUnavailable)
    }
}

impl std::error::Error for ProductionCatalogError {}

/// 打开已有生产目录库，或在完全空白状态下初始化首个目录库。
pub fn open_or_initialize_production_catalog(
    local_app_data: &Path,
) -> Result<ProductionCatalogRuntime, ProductionCatalogError> {
    #[cfg(not(windows))]
    {
        let _ = local_app_data;
        return Err(ProductionCatalogError::UnsupportedPlatform);
    }

    #[cfg(windows)]
    {
        if !local_app_data.is_dir() {
            return Err(ProductionCatalogError::LocalAppDataUnavailable);
        }
        let app_root = local_app_data.join("Trae Sync");
        let storage_root = app_root.join("data");
        let recovery_root = app_root.join("recovery");
        let pointer_path = recovery_root.join("storage-root-pointer.json");
        let wrapper_path = recovery_root.join("catalog-key.dpapi");

        fs::create_dir_all(&recovery_root)
            .map_err(|_| ProductionCatalogError::LocalAppDataUnavailable)?;
        let canonical_recovery = validate_recovery_root(&storage_root, &recovery_root)?;
        let mut startup_lease =
            OperationLease::acquire(&canonical_recovery, STARTUP_LEASE_LOCATION_ID)
                .map_err(|_| ProductionCatalogError::OperationLeaseUnavailable)?;

        // 所有首启判定都在共享租约内完成，避免两个实例各自发布一个目录库代次。
        if pointer_path.exists() {
            startup_lease
                .bind_storage_root(&storage_root)
                .map_err(|_| ProductionCatalogError::ExistingStateIncomplete)?;
            return open_existing_with_lease(
                &storage_root,
                &recovery_root,
                &pointer_path,
                &wrapper_path,
                &startup_lease,
            );
        }

        // 指针缺失时不接管任何已有内容或孤立密钥包装，避免猜测哪个根是权威根。
        if wrapper_path.exists()
            || directory_has_entries(&storage_root)?
            || recovery_has_non_lock_entries(&recovery_root)?
        {
            return Err(ProductionCatalogError::ExistingStateIncomplete);
        }

        fs::create_dir_all(&storage_root)
            .map_err(|_| ProductionCatalogError::LocalAppDataUnavailable)?;
        startup_lease
            .bind_storage_root(&storage_root)
            .map_err(|_| ProductionCatalogError::ExistingStateIncomplete)?;

        let catalog_id = format!("catalog-{}", random_hex(16)?);
        let catalog_key = random_hex(32)?;
        let catalog_path =
            ensure_catalog_initialized(&storage_root, &catalog_key, &recovery_root, &startup_lease)
                .map_err(map_catalog_error)?;
        initialize_catalog_identity(
            &catalog_path,
            &catalog_key,
            &catalog_id,
            KEY_GENERATION,
            &recovery_root,
            &startup_lease,
        )
        .map_err(map_catalog_error)?;
        let binding = StorageRootBinding::initialize(&storage_root, &catalog_id)
            .map_err(|_| ProductionCatalogError::CatalogInitializationFailed)?;

        let wrapper = DpapiKeyWrapper::new(&wrapper_path);
        wrapper
            .wrap_catalog_key(&KeyWrapperRequest::new(
                &catalog_id,
                KEY_GENERATION,
                &catalog_key,
            ))
            .map_err(|_| ProductionCatalogError::KeyProtectionFailed)?;
        publish_initial_storage_root_pointer(&pointer_path, &binding)
            .map_err(|_| ProductionCatalogError::PointerPublicationFailed)?;

        // 发布后重新从固定指针打开，证明后续启动会得到同一根和同一目录库。
        open_existing_with_lease(
            &storage_root,
            &recovery_root,
            &pointer_path,
            &wrapper_path,
            &startup_lease,
        )
    }
}

#[cfg(windows)]
fn open_existing_with_lease(
    expected_storage_root: &Path,
    recovery_root: &Path,
    pointer_path: &Path,
    wrapper_path: &Path,
    operation_lease: &OperationLease,
) -> Result<ProductionCatalogRuntime, ProductionCatalogError> {
    let binding = read_storage_root_pointer(pointer_path)
        .map_err(|_| ProductionCatalogError::ExistingStateIncomplete)?;
    let expected_canonical = fs::canonicalize(expected_storage_root)
        .map_err(|_| ProductionCatalogError::ExistingStateIncomplete)?;
    if binding.canonical_path != expected_canonical.to_string_lossy() {
        return Err(ProductionCatalogError::ExistingStateIncomplete);
    }

    let catalog_key = DpapiKeyWrapper::new(wrapper_path)
        .unwrap_catalog_key(&binding.catalog_id, KEY_GENERATION)
        .map_err(|_| ProductionCatalogError::KeyProtectionFailed)?;
    let catalog_path = resolve_current_catalog_path(expected_storage_root)
        .map_err(|_| ProductionCatalogError::ExistingStateIncomplete)?;
    verify_catalog_identity(
        &catalog_path,
        &catalog_key,
        &binding.catalog_id,
        KEY_GENERATION,
        recovery_root,
        operation_lease,
    )
    .map_err(map_catalog_error)?;

    Ok(ProductionCatalogRuntime {
        storage_root: expected_canonical,
        recovery_root: recovery_root.to_path_buf(),
        catalog_id: binding.catalog_id,
        catalog_key,
    })
}

fn map_catalog_error(error: CatalogPathError) -> ProductionCatalogError {
    match error {
        CatalogPathError::CatalogWriteProtocolUpgradeRequired => {
            ProductionCatalogError::CatalogWriteProtocolUpgradeRequired
        }
        _ => ProductionCatalogError::CatalogInitializationFailed,
    }
}

#[cfg(windows)]
fn recovery_has_non_lock_entries(path: &Path) -> Result<bool, ProductionCatalogError> {
    let metadata =
        fs::symlink_metadata(path).map_err(|_| ProductionCatalogError::ExistingStateIncomplete)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(ProductionCatalogError::ExistingStateIncomplete);
    }
    let entries =
        fs::read_dir(path).map_err(|_| ProductionCatalogError::ExistingStateIncomplete)?;
    for entry in entries {
        let entry = entry.map_err(|_| ProductionCatalogError::ExistingStateIncomplete)?;
        if entry.file_name() != "locks" {
            return Ok(true);
        }
    }
    Ok(false)
}

#[cfg(windows)]
fn validate_recovery_root(
    expected_storage_root: &Path,
    recovery_root: &Path,
) -> Result<PathBuf, ProductionCatalogError> {
    let recovery_metadata = fs::symlink_metadata(recovery_root)
        .map_err(|_| ProductionCatalogError::ExistingStateIncomplete)?;
    if !recovery_metadata.is_dir() || is_reparse_point(&recovery_metadata) {
        return Err(ProductionCatalogError::ExistingStateIncomplete);
    }
    let canonical_recovery = recovery_root
        .canonicalize()
        .map_err(|_| ProductionCatalogError::ExistingStateIncomplete)?;
    let expected_recovery = expected_storage_root
        .parent()
        .ok_or(ProductionCatalogError::ExistingStateIncomplete)?
        .join("recovery")
        .canonicalize()
        .map_err(|_| ProductionCatalogError::ExistingStateIncomplete)?;
    if !same_path(&canonical_recovery, &expected_recovery) {
        return Err(ProductionCatalogError::ExistingStateIncomplete);
    }
    Ok(canonical_recovery)
}

#[cfg(windows)]
fn directory_has_entries(path: &Path) -> Result<bool, ProductionCatalogError> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(_) => return Err(ProductionCatalogError::ExistingStateIncomplete),
    };
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(ProductionCatalogError::ExistingStateIncomplete);
    }
    fs::read_dir(path)
        .map(|mut entries| entries.next().is_some())
        .map_err(|_| ProductionCatalogError::ExistingStateIncomplete)
}

#[cfg(windows)]
fn random_hex(byte_count: usize) -> Result<String, ProductionCatalogError> {
    let mut bytes = vec![0_u8; byte_count];
    rand_bytes(&mut bytes).map_err(|_| ProductionCatalogError::RandomGenerationFailed)?;
    Ok(hex::encode(bytes))
}

fn is_reparse_point(metadata: &fs::Metadata) -> bool {
    if metadata.file_type().is_symlink() {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        return metadata.file_attributes() & 0x400 != 0;
    }
    #[cfg(not(windows))]
    {
        false
    }
}

fn same_path(left: &Path, right: &Path) -> bool {
    #[cfg(windows)]
    {
        fn key(path: &Path) -> String {
            let value = path.to_string_lossy();
            value
                .strip_prefix("\\\\?\\")
                .unwrap_or(value.as_ref())
                .to_ascii_lowercase()
        }
        key(left) == key(right)
    }
    #[cfg(not(windows))]
    {
        left == right
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use std::sync::{Arc, Barrier};

    #[test]
    fn production_catalog_initializes_and_reopens_same_identity() {
        let local_app_data = tempfile::tempdir().unwrap();

        let first = open_or_initialize_production_catalog(local_app_data.path()).unwrap();
        let second = open_or_initialize_production_catalog(local_app_data.path()).unwrap();

        assert_eq!(first.storage_root, second.storage_root);
        assert_eq!(first.catalog_id, second.catalog_id);
        assert_eq!(first.catalog_key, second.catalog_key);
        assert!(first.recovery_root.join("catalog-key.dpapi").is_file());
        assert!(first
            .recovery_root
            .join("storage-root-pointer.json")
            .is_file());
    }

    #[test]
    fn production_catalog_refuses_unbound_existing_content() {
        let local_app_data = tempfile::tempdir().unwrap();
        let storage_root = local_app_data.path().join("Trae Sync").join("data");
        fs::create_dir_all(&storage_root).unwrap();
        fs::write(storage_root.join("unknown"), b"preserve").unwrap();

        let result = open_or_initialize_production_catalog(local_app_data.path());
        assert!(matches!(
            result,
            Err(ProductionCatalogError::ExistingStateIncomplete)
        ));
        assert_eq!(fs::read(storage_root.join("unknown")).unwrap(), b"preserve");
    }

    #[test]
    fn production_catalog_refuses_missing_pointer_with_recovery_state() {
        let local_app_data = tempfile::tempdir().unwrap();
        let recovery_root = local_app_data.path().join("Trae Sync").join("recovery");
        let unfinished = recovery_root
            .join("migration-manifests")
            .join("operation-in-progress");
        fs::create_dir_all(&unfinished).unwrap();
        fs::write(unfinished.join("00000000000000000000.json"), b"preserve").unwrap();

        let result = open_or_initialize_production_catalog(local_app_data.path());

        assert!(matches!(
            result,
            Err(ProductionCatalogError::ExistingStateIncomplete)
        ));
        assert_eq!(
            fs::read(unfinished.join("00000000000000000000.json")).unwrap(),
            b"preserve"
        );
    }

    #[test]
    fn production_catalog_lease_revalidates_roots_and_blocks_second_instance() {
        let local_app_data = tempfile::tempdir().unwrap();
        let runtime = open_or_initialize_production_catalog(local_app_data.path()).unwrap();

        let lease = runtime.acquire_operation_lease("loc-one").unwrap();
        assert!(matches!(
            runtime.acquire_operation_lease("loc-one"),
            Err(ProductionCatalogError::OperationLeaseUnavailable)
        ));
        assert!(matches!(
            open_or_initialize_production_catalog(local_app_data.path()),
            Err(ProductionCatalogError::OperationLeaseUnavailable)
        ));

        drop(lease);
        assert!(runtime.acquire_operation_lease("loc-one").is_ok());
    }

    #[test]
    fn concurrent_first_start_publishes_one_authoritative_generation() {
        let local_app_data = tempfile::tempdir().unwrap();
        let root = Arc::new(local_app_data.path().to_path_buf());
        let barrier = Arc::new(Barrier::new(4));
        let handles = (0..4)
            .map(|_| {
                let root = Arc::clone(&root);
                let barrier = Arc::clone(&barrier);
                std::thread::spawn(move || {
                    barrier.wait();
                    open_or_initialize_production_catalog(&root)
                })
            })
            .collect::<Vec<_>>();
        let results = handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect::<Vec<_>>();

        let successful_catalog_ids = results
            .iter()
            .filter_map(|result| result.as_ref().ok())
            .map(|runtime| runtime.catalog_id.as_str())
            .collect::<Vec<_>>();
        assert!(!successful_catalog_ids.is_empty());
        assert!(successful_catalog_ids
            .windows(2)
            .all(|pair| pair[0] == pair[1]));
        assert!(results.iter().all(|result| matches!(
            result,
            Ok(_) | Err(ProductionCatalogError::OperationLeaseUnavailable)
        )));

        let reopened = open_or_initialize_production_catalog(local_app_data.path()).unwrap();
        assert!(successful_catalog_ids
            .iter()
            .all(|catalog_id| **catalog_id == reopened.catalog_id));
        let generations = fs::read_dir(
            local_app_data
                .path()
                .join("Trae Sync")
                .join("data")
                .join("catalog")
                .join("generations"),
        )
        .unwrap()
        .count();
        assert_eq!(generations, 1);
    }
}
