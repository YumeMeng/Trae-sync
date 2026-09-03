//! T08 跨进程操作租约。
//!
//! 使用锁文件保存审计标记，并使用 OS 文件锁判断当前进程是否仍持有租约。
//! 进程崩溃后只留下标记、不再持有 OS 锁时，新进程可以安全接管。

use std::fs::{self, File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};

use fs2::FileExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OperationLeaseError {
    CatalogBusy,
    DataLocationBusy,
    LockDirectoryUnavailable,
    InvalidLocationId,
    InvalidStorageRoot,
    LeaseContextUnbound,
    StorageRootMismatch,
}

impl std::fmt::Display for OperationLeaseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let message = match self {
            Self::CatalogBusy => "目录库已有其他写操作",
            Self::DataLocationBusy => "数据位置已有其他写操作",
            Self::LockDirectoryUnavailable => "锁目录不可用",
            Self::InvalidLocationId => "数据位置标识无效",
            Self::InvalidStorageRoot => "存储根无效",
            Self::LeaseContextUnbound => "租约未绑定存储根",
            Self::StorageRootMismatch => "租约与存储根不匹配",
        };
        f.write_str(message)
    }
}

impl std::error::Error for OperationLeaseError {}

/// 跨进程锁的只读状态；查询不会创建、清理或抢占任何锁。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OperationLockStatus {
    pub catalog_lock_held: bool,
    pub data_location_lock_held: bool,
    pub write_allowed: bool,
}

pub struct OperationLease {
    recovery_root: PathBuf,
    data_location_id: String,
    catalog_path: PathBuf,
    data_path: PathBuf,
    catalog_file: File,
    data_file: File,
    /// 目录库入口必须绑定到取得租约时验证过的存储根；普通数据位置租约可为空。
    bound_storage_root: Option<PathBuf>,
}

impl std::fmt::Debug for OperationLease {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // 不输出恢复根、锁文件路径或其他本地环境细节。
        formatter
            .debug_struct("OperationLease")
            .field("data_location_id", &self.data_location_id)
            .field("catalog_bound", &self.bound_storage_root.is_some())
            .finish()
    }
}

impl OperationLease {
    /// 供非 fixture 的本机受控写入口复用固定恢复区与数据位置租约。
    ///
    /// 调用方必须先完成数据位置授权与路径见证；该方法只负责取得共享锁，
    /// 不替代上层的 Gate、进程观测或写前复核。
    pub fn acquire_shared(
        recovery_root: &Path,
        data_location_id: &str,
    ) -> Result<Self, OperationLeaseError> {
        Self::acquire(recovery_root, data_location_id)
    }

    pub(crate) fn acquire(
        recovery_root: &Path,
        data_location_id: &str,
    ) -> Result<Self, OperationLeaseError> {
        if data_location_id.is_empty()
            || !data_location_id
                .chars()
                .all(|character| character.is_ascii_alphanumeric() || character == '-')
        {
            return Err(OperationLeaseError::InvalidLocationId);
        }
        // 先固定恢复区物理身份，再创建锁目录；锁目录不能通过 reparse 点逃逸。
        let canonical_recovery_root = canonical_storage_root(recovery_root)?;
        let lock_dir = ensure_lock_directory(&canonical_recovery_root)?;
        let catalog_path = lock_dir.join("catalog-write.lock");
        let data_path = lock_dir.join(format!("data-{}.lock", hash_id(data_location_id)));
        let catalog_file =
            open_lock_file(&catalog_path).map_err(|_| OperationLeaseError::CatalogBusy)?;
        if catalog_file.try_lock_exclusive().is_err() {
            return Err(OperationLeaseError::CatalogBusy);
        }
        let data_file = match open_lock_file(&data_path) {
            Ok(file) => file,
            Err(_) => {
                let _ = catalog_file.unlock();
                drop(catalog_file);
                return Err(OperationLeaseError::DataLocationBusy);
            }
        };
        if data_file.try_lock_exclusive().is_err() {
            let _ = catalog_file.unlock();
            drop(data_file);
            drop(catalog_file);
            return Err(OperationLeaseError::DataLocationBusy);
        }
        Ok(Self {
            recovery_root: canonical_recovery_root,
            data_location_id: data_location_id.to_string(),
            catalog_path,
            data_path,
            catalog_file,
            data_file,
            bound_storage_root: None,
        })
    }

    /// 取得已绑定目录库存储根的租约；目录库公开入口只接受此类租约。
    pub(crate) fn acquire_bound(
        recovery_root: &Path,
        storage_root: &Path,
        data_location_id: &str,
    ) -> Result<Self, OperationLeaseError> {
        let canonical_storage_root = canonical_storage_root(storage_root)?;
        let mut lease = Self::acquire(recovery_root, data_location_id)?;
        lease.bound_storage_root = Some(canonical_storage_root);
        Ok(lease)
    }

    /// 首次启动先取得共享租约，再创建空存储根；创建完成后绑定目录库上下文。
    pub(crate) fn bind_storage_root(
        &mut self,
        storage_root: &Path,
    ) -> Result<(), OperationLeaseError> {
        let canonical_storage_root = canonical_storage_root(storage_root)?;
        self.bound_storage_root = Some(canonical_storage_root);
        Ok(())
    }

    pub(crate) fn validate_storage_root(
        &self,
        storage_root: &Path,
    ) -> Result<(), OperationLeaseError> {
        let Some(bound_storage_root) = &self.bound_storage_root else {
            return Err(OperationLeaseError::LeaseContextUnbound);
        };
        let current = canonical_storage_root(storage_root)?;
        if &current != bound_storage_root {
            return Err(OperationLeaseError::StorageRootMismatch);
        }
        Ok(())
    }

    /// 复核调用方使用的是取得租约时同一个固定恢复区，避免仅凭字符串路径
    /// 把租约绑定到另一个等价或替换后的目录。
    pub(crate) fn validate_recovery_root(
        &self,
        recovery_root: &Path,
    ) -> Result<(), OperationLeaseError> {
        let expected = canonical_storage_root(recovery_root)?;
        let actual = canonical_storage_root(&self.recovery_root)?;
        if expected != actual {
            return Err(OperationLeaseError::StorageRootMismatch);
        }
        Ok(())
    }

    pub fn catalog_lock_path(&self) -> &Path {
        &self.catalog_path
    }

    pub fn data_location_lock_path(&self) -> &Path {
        &self.data_path
    }

    pub fn recovery_root(&self) -> &Path {
        &self.recovery_root
    }

    pub fn data_location_id(&self) -> &str {
        &self.data_location_id
    }
}

fn canonical_storage_root(path: &Path) -> Result<PathBuf, OperationLeaseError> {
    let metadata =
        fs::symlink_metadata(path).map_err(|_| OperationLeaseError::InvalidStorageRoot)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(OperationLeaseError::InvalidStorageRoot);
    }
    path.canonicalize()
        .map_err(|_| OperationLeaseError::InvalidStorageRoot)
}

/// 确保 locks 目录是普通目录；父恢复区已经在 acquire 前完成物理身份校验。
fn ensure_lock_directory(recovery_root: &Path) -> Result<PathBuf, OperationLeaseError> {
    let lock_dir = recovery_root.join("locks");
    match fs::symlink_metadata(&lock_dir) {
        Ok(metadata) => {
            if is_reparse_point(&metadata) || !metadata.is_dir() {
                return Err(OperationLeaseError::LockDirectoryUnavailable);
            }
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => match fs::create_dir(&lock_dir) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(_) => return Err(OperationLeaseError::LockDirectoryUnavailable),
        },
        Err(_) => return Err(OperationLeaseError::LockDirectoryUnavailable),
    }

    let metadata = fs::symlink_metadata(&lock_dir)
        .map_err(|_| OperationLeaseError::LockDirectoryUnavailable)?;
    if is_reparse_point(&metadata) || !metadata.is_dir() {
        return Err(OperationLeaseError::LockDirectoryUnavailable);
    }
    Ok(lock_dir)
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

impl Drop for OperationLease {
    fn drop(&mut self) {
        let _ = self.catalog_file.sync_all();
        let _ = self.data_file.sync_all();
        let _ = self.data_file.unlock();
        let _ = self.catalog_file.unlock();
        // 锁文件本身是稳定的命名锚点。释放时只解除 OS 锁，不删除路径，
        // 避免解锁与下一个进程重新加锁之间发生删除竞态。
    }
}

fn open_lock_file(path: &Path) -> io::Result<File> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "锁标记不能是符号链接",
            ));
        }
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)
}

fn is_lock_held(path: &Path) -> bool {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return false,
        Err(_) => return true,
    };
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return true;
    }
    let file = match OpenOptions::new().read(true).write(true).open(path) {
        Ok(file) => file,
        Err(_) => return true,
    };
    if file.try_lock_exclusive().is_ok() {
        let _ = file.unlock();
        false
    } else {
        true
    }
}

fn hash_id(data_location_id: &str) -> String {
    let mut digest = Sha256::new();
    digest.update(data_location_id.as_bytes());
    hex::encode(digest.finalize())
}

/// 查询目录库锁和数据位置锁，目录库锁永远先于数据位置锁参与判断。
pub fn inspect_lock_status(
    recovery_root: &Path,
    data_location_id: &str,
) -> Result<OperationLockStatus, OperationLeaseError> {
    if data_location_id.is_empty()
        || !data_location_id
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '-')
    {
        return Err(OperationLeaseError::InvalidLocationId);
    }
    let lock_dir = recovery_root.join("locks");
    match fs::symlink_metadata(&lock_dir) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(OperationLockStatus {
                catalog_lock_held: false,
                data_location_lock_held: false,
                write_allowed: true,
            });
        }
        Err(_) => return Err(OperationLeaseError::LockDirectoryUnavailable),
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
            return Err(OperationLeaseError::LockDirectoryUnavailable);
        }
        Ok(_) => {}
    }
    if !lock_dir.is_dir() {
        return Err(OperationLeaseError::LockDirectoryUnavailable);
    }
    let catalog_lock_held = is_lock_held(&lock_dir.join("catalog-write.lock"));
    let data_location_lock_held =
        is_lock_held(&lock_dir.join(format!("data-{}.lock", hash_id(data_location_id))));
    Ok(OperationLockStatus {
        catalog_lock_held,
        data_location_lock_held,
        write_allowed: !catalog_lock_held && !data_location_lock_held,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    #[test]
    fn child_process_cannot_acquire_existing_lease() {
        let Ok(root) = std::env::var("TRAE_SYNC_LOCK_CHILD_ROOT") else {
            return;
        };

        assert!(matches!(
            OperationLease::acquire(Path::new(&root), "loc-one"),
            Err(OperationLeaseError::CatalogBusy)
        ));
    }

    #[test]
    fn lock_order_is_catalog_then_location_and_drop_releases_both() {
        let root = tempfile::tempdir().unwrap();
        let lease = OperationLease::acquire(root.path(), "loc-one").unwrap();
        assert!(lease.catalog_lock_path().exists());
        assert!(lease.data_location_lock_path().exists());
        assert!(matches!(
            OperationLease::acquire(root.path(), "loc-one"),
            Err(OperationLeaseError::CatalogBusy)
        ));
        drop(lease);
        assert!(OperationLease::acquire(root.path(), "loc-one").is_ok());
    }

    #[test]
    fn invalid_location_id_fails_before_creating_locks() {
        let root = tempfile::tempdir().unwrap();
        assert!(matches!(
            OperationLease::acquire(root.path(), "../outside"),
            Err(OperationLeaseError::InvalidLocationId)
        ));
        assert!(!root.path().join("locks").exists());
    }

    #[test]
    fn reparse_or_non_directory_lock_path_fails_closed() {
        let root = tempfile::tempdir().unwrap();
        fs::write(root.path().join("locks"), b"not-a-directory").unwrap();
        assert!(matches!(
            OperationLease::acquire(root.path(), "loc-one"),
            Err(OperationLeaseError::LockDirectoryUnavailable)
        ));
    }

    #[cfg(unix)]
    #[test]
    fn symlink_lock_directory_fails_closed() {
        let root = tempfile::tempdir().unwrap();
        let target = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(target.path(), root.path().join("locks")).unwrap();
        assert!(matches!(
            OperationLease::acquire(root.path(), "loc-one"),
            Err(OperationLeaseError::LockDirectoryUnavailable)
        ));
    }

    #[test]
    fn stale_marker_without_os_lock_can_be_reclaimed() {
        let root = tempfile::tempdir().unwrap();
        let lock_dir = root.path().join("locks");
        fs::create_dir_all(&lock_dir).unwrap();
        fs::write(lock_dir.join("catalog-write.lock"), b"stale").unwrap();
        fs::write(
            lock_dir.join(format!("data-{}.lock", hash_id("loc-one"))),
            b"stale",
        )
        .unwrap();

        assert!(
            inspect_lock_status(root.path(), "loc-one")
                .unwrap()
                .write_allowed
        );
        let lease = OperationLease::acquire(root.path(), "loc-one")
            .expect("没有 OS 锁的残留标记应可被新进程接管");
        assert!(
            !inspect_lock_status(root.path(), "loc-one")
                .unwrap()
                .write_allowed
        );
        drop(lease);
        assert!(
            inspect_lock_status(root.path(), "loc-one")
                .unwrap()
                .write_allowed
        );
    }

    #[test]
    fn inspect_status_is_read_only_and_reports_catalog_first() {
        let root = tempfile::tempdir().unwrap();
        let initial = inspect_lock_status(root.path(), "loc-one").unwrap();
        assert!(initial.write_allowed);
        let lease = OperationLease::acquire(root.path(), "loc-one").unwrap();
        let status = inspect_lock_status(root.path(), "loc-one").unwrap();
        assert!(status.catalog_lock_held);
        assert!(status.data_location_lock_held);
        assert!(!status.write_allowed);
        drop(lease);
        assert!(
            inspect_lock_status(root.path(), "loc-one")
                .unwrap()
                .write_allowed
        );
    }

    #[test]
    fn releasing_lease_keeps_lock_anchors_for_next_process() {
        let root = tempfile::tempdir().unwrap();
        let lease = OperationLease::acquire(root.path(), "loc-one").unwrap();
        let catalog_path = lease.catalog_lock_path().to_path_buf();
        let data_path = lease.data_location_lock_path().to_path_buf();

        drop(lease);

        assert!(catalog_path.is_file());
        assert!(data_path.is_file());
        assert!(
            inspect_lock_status(root.path(), "loc-one")
                .unwrap()
                .write_allowed
        );
    }

    #[test]
    fn second_process_is_blocked_by_catalog_lock() {
        let root = tempfile::tempdir().unwrap();
        let _lease = OperationLease::acquire(root.path(), "loc-one").unwrap();
        let status = Command::new(std::env::current_exe().unwrap())
            .env("TRAE_SYNC_LOCK_CHILD_ROOT", root.path())
            .args([
                "--exact",
                "operation_lease::tests::child_process_cannot_acquire_existing_lease",
                "--nocapture",
            ])
            .status()
            .unwrap();

        assert!(status.success());
    }
}
