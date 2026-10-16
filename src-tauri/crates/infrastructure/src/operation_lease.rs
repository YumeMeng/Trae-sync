//! T08 跨进程操作租约。
//!
//! 使用锁文件保存审计标记，并使用 OS 文件锁判断当前进程是否仍持有租约。
//! 进程崩溃后只留下标记、不再持有 OS 锁时，新进程可以安全接管。

use std::fs::{self, File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use fs2::{lock_contended_error, FileExt};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use traesync_ports::FileIdentityProvider;

use crate::file_identity::PlatformFileIdentityProvider;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OperationLeaseError {
    CatalogBusy,
    DataLocationBusy,
    LockDirectoryUnavailable,
    InvalidLocationId,
    InvalidResourcePath,
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
            Self::InvalidResourcePath => "数据资源路径无效",
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
    catalog_file: Option<File>,
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
    /// 供非 fixture 的本机受控写入口复用按数据位置划分的租约。
    ///
    /// 此租约不拿目录库全局锁，避免独立 TRAE 数据目录互相阻塞。
    /// 调用方仍须完成数据位置授权、进程观测与写前复核。
    pub fn acquire_shared(
        recovery_root: &Path,
        data_location_id: &str,
    ) -> Result<Self, OperationLeaseError> {
        Self::acquire_data_only(recovery_root, data_location_id)
    }

    /// 按实际数据目录身份取得租约；同一物理父目录下同名资源共用锁，
    /// 即使调用方通过不同路径别名进入也不会分出两把锁。
    pub fn acquire_shared_for_path(
        recovery_root: &Path,
        resource_path: &Path,
    ) -> Result<Self, OperationLeaseError> {
        let resource_id = resource_location_id(resource_path)?;
        Self::acquire_data_only(recovery_root, &resource_id)
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
        let catalog_file = open_lock_file(&catalog_path)
            .map_err(|_| OperationLeaseError::LockDirectoryUnavailable)?;
        if let Err(error) = catalog_file.try_lock_exclusive() {
            return Err(map_lock_attempt_error(
                error,
                OperationLeaseError::CatalogBusy,
            ));
        }
        let data_file = match open_lock_file(&data_path) {
            Ok(file) => file,
            Err(_) => {
                let _ = catalog_file.unlock();
                drop(catalog_file);
                return Err(OperationLeaseError::LockDirectoryUnavailable);
            }
        };
        if let Err(error) = data_file.try_lock_exclusive() {
            let _ = catalog_file.unlock();
            drop(data_file);
            drop(catalog_file);
            return Err(map_lock_attempt_error(
                error,
                OperationLeaseError::DataLocationBusy,
            ));
        }
        Ok(Self {
            recovery_root: canonical_recovery_root,
            data_location_id: data_location_id.to_string(),
            catalog_path,
            data_path,
            catalog_file: Some(catalog_file),
            data_file,
            bound_storage_root: None,
        })
    }

    fn acquire_data_only(
        recovery_root: &Path,
        data_location_id: &str,
    ) -> Result<Self, OperationLeaseError> {
        validate_location_id(data_location_id)?;
        let canonical_recovery_root = canonical_storage_root(recovery_root)?;
        let lock_dir = ensure_lock_directory(&canonical_recovery_root)?;
        let catalog_path = lock_dir.join("catalog-write.lock");
        let data_path = lock_dir.join(format!("data-{}.lock", hash_id(data_location_id)));
        let data_file = open_lock_file(&data_path)
            .map_err(|_| OperationLeaseError::LockDirectoryUnavailable)?;
        if let Err(error) = data_file.try_lock_exclusive() {
            return Err(map_lock_attempt_error(
                error,
                OperationLeaseError::DataLocationBusy,
            ));
        }
        Ok(Self {
            recovery_root: canonical_recovery_root,
            data_location_id: data_location_id.to_string(),
            catalog_path,
            data_path,
            catalog_file: None,
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
    // P16-T6：防逃逸判定统一走 link_guard（junction 等 reparse point 一并拒绝）。
    if crate::link_guard::metadata_is_reparse_link(&metadata) || !metadata.is_dir() {
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
            // P16-T6：防逃逸判定统一走 link_guard（原本地 is_reparse_point 删除）。
            if crate::link_guard::metadata_is_reparse_link(&metadata) || !metadata.is_dir() {
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
    if crate::link_guard::metadata_is_reparse_link(&metadata) || !metadata.is_dir() {
        return Err(OperationLeaseError::LockDirectoryUnavailable);
    }
    Ok(lock_dir)
}

impl Drop for OperationLease {
    fn drop(&mut self) {
        let _ = self.data_file.sync_all();
        let _ = self.data_file.unlock();
        if let Some(catalog_file) = &self.catalog_file {
            let _ = catalog_file.sync_all();
            let _ = catalog_file.unlock();
        }
        // 锁文件本身是稳定的命名锚点。释放时只解除 OS 锁，不删除路径，
        // 避免解锁与下一个进程重新加锁之间发生删除竞态。
    }
}

/// P16-T3：锁文件 open 阶段对瞬时拒绝（os error 5）的退避重试时长。
///
/// 取证定性（2026-10-16）：文件系统过滤驱动（杀软/Defender 实时扫描）对
/// 新建文件 ~20ms 窗口内并发 open 返回 ERROR_ACCESS_DENIED(5)，窗口过后
/// fs2 阻塞锁完全正常——并发 5 个环境失败测试的根因。仅对此错误码做一次
/// ~50ms 退避重试，其余错误不重试（保持 fail-closed 语义）。
const LOCK_OPEN_RETRY_DELAY: Duration = Duration::from_millis(50);

/// 打开锁标记文件：拒绝 reparse point，其余按普通读写文件打开。
///
/// P16-T3：open 返回 os error 5（ERROR_ACCESS_DENIED，过滤驱动扫描窗口）
/// 时做一次 ~50ms 退避后重试；错误码语义见 `LOCK_OPEN_RETRY_DELAY`。
/// `open` 为注入缝：生产路径传真实打开逻辑，单测注入 mock open 序列。
fn open_lock_file_with_retry(
    path: &Path,
    open: &dyn Fn(&Path) -> io::Result<File>,
    retry_delay: Duration,
) -> io::Result<File> {
    match open(path) {
        Err(error) if error.raw_os_error() == Some(5) => {
            // 仅 os error 5 重试一次；注意 `io::Error::new(PermissionDenied, ...)`
            // 无 raw os error，不会落入此分支（reparse 拒绝路径保持不重试）。
            std::thread::sleep(retry_delay);
            open(path)
        }
        other => other,
    }
}

fn open_lock_file(path: &Path) -> io::Result<File> {
    open_lock_file_with_retry(path, &open_lock_file_once, LOCK_OPEN_RETRY_DELAY)
}

/// 单次打开逻辑（不含重试）；由 `open_lock_file` 包装注入重试。
fn open_lock_file_once(path: &Path) -> io::Result<File> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if crate::link_guard::metadata_is_reparse_link(&metadata) => {
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

/// 针对单个共享文件的短锁。锁文件与目标文件分开，目标可安全原子替换。
pub struct FileOperationLock {
    file: File,
}

impl FileOperationLock {
    /// 锁定目标旁的稳定 sidecar，覆盖调用方的完整读改写过程。
    pub fn acquire_sibling_resource(resource_path: &Path) -> Result<Self, OperationLeaseError> {
        Self::acquire(&sibling_resource_lock_path(resource_path)?)
    }

    /// 非阻塞获取目标旁的稳定 sidecar；仅锁竞争返回 `Ok(None)`。
    pub fn try_acquire_sibling_resource(
        resource_path: &Path,
    ) -> Result<Option<Self>, OperationLeaseError> {
        Self::try_acquire(&sibling_resource_lock_path(resource_path)?)
    }

    fn acquire(lock_path: &Path) -> Result<Self, OperationLeaseError> {
        let file =
            open_lock_file(lock_path).map_err(|_| OperationLeaseError::LockDirectoryUnavailable)?;
        file.lock_exclusive()
            .map_err(|_| OperationLeaseError::LockDirectoryUnavailable)?;
        Ok(Self { file })
    }

    fn try_acquire(lock_path: &Path) -> Result<Option<Self>, OperationLeaseError> {
        let file =
            open_lock_file(lock_path).map_err(|_| OperationLeaseError::LockDirectoryUnavailable)?;
        match file.try_lock_exclusive() {
            Ok(()) => Ok(Some(Self { file })),
            Err(error) if is_lock_contended_error(&error) => Ok(None),
            Err(_) => Err(OperationLeaseError::LockDirectoryUnavailable),
        }
    }
}

impl Drop for FileOperationLock {
    fn drop(&mut self) {
        let _ = self.file.unlock();
    }
}

fn sibling_resource_lock_path(resource_path: &Path) -> Result<PathBuf, OperationLeaseError> {
    let parent = resource_path
        .parent()
        .ok_or(OperationLeaseError::InvalidResourcePath)?
        .canonicalize()
        .map_err(|_| OperationLeaseError::InvalidResourcePath)?;
    let name = resource_path
        .file_name()
        .ok_or(OperationLeaseError::InvalidResourcePath)?;
    let mut lock_name = name.to_os_string();
    lock_name.push(".lock");
    Ok(parent.join(lock_name))
}

fn resource_location_id(resource_path: &Path) -> Result<String, OperationLeaseError> {
    let parent = resource_path
        .parent()
        .ok_or(OperationLeaseError::InvalidResourcePath)?;
    let canonical_parent = parent
        .canonicalize()
        .map_err(|_| OperationLeaseError::InvalidResourcePath)?;
    let name = resource_path
        .file_name()
        .ok_or(OperationLeaseError::InvalidResourcePath)?
        .to_string_lossy();
    // P16-T6：防逃逸判定统一走 link_guard（原本地 is_reparse_point 删除）。
    match fs::symlink_metadata(resource_path) {
        Ok(metadata) if crate::link_guard::metadata_is_reparse_link(&metadata) => {
            return Err(OperationLeaseError::InvalidResourcePath);
        }
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(_) => return Err(OperationLeaseError::InvalidResourcePath),
    }
    let identity = PlatformFileIdentityProvider::new()
        .read_file_identity(&canonical_parent)
        .ok_or(OperationLeaseError::InvalidResourcePath)?;
    let normalized_name = if cfg!(windows) {
        name.to_lowercase()
    } else {
        name.into_owned()
    };
    let mut digest = Sha256::new();
    digest.update(identity.volume_serial.to_le_bytes());
    digest.update(identity.file_index_high.to_le_bytes());
    digest.update(identity.file_index_low.to_le_bytes());
    digest.update([0]);
    digest.update(normalized_name.as_bytes());
    Ok(format!("path-{}", hex::encode(digest.finalize())))
}

/// 只把 fs2 明确标记为锁竞争的错误报为占用，其余都归为锁设施不可用。
fn map_lock_attempt_error(error: io::Error, busy: OperationLeaseError) -> OperationLeaseError {
    if is_lock_contended_error(&error) {
        busy
    } else {
        OperationLeaseError::LockDirectoryUnavailable
    }
}

fn is_lock_contended_error(error: &io::Error) -> bool {
    error.raw_os_error().is_some() && error.raw_os_error() == lock_contended_error().raw_os_error()
}

fn validate_location_id(data_location_id: &str) -> Result<(), OperationLeaseError> {
    if data_location_id.is_empty()
        || !data_location_id
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '-')
    {
        return Err(OperationLeaseError::InvalidLocationId);
    }
    Ok(())
}

fn is_lock_held(path: &Path) -> bool {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return false,
        Err(_) => return true,
    };
    // P16-T6：防逃逸判定统一走 link_guard（junction 锁标记一并 fail-closed）。
    if crate::link_guard::metadata_is_reparse_link(&metadata) || !metadata.is_file() {
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

/// 查询目录库元数据锁与数据位置锁；`write_allowed` 只表示该数据位置可写。
pub fn inspect_lock_status(
    recovery_root: &Path,
    data_location_id: &str,
) -> Result<OperationLockStatus, OperationLeaseError> {
    validate_location_id(data_location_id)?;
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
        // P16-T6：防逃逸判定统一走 link_guard。
        Ok(metadata) if crate::link_guard::metadata_is_reparse_link(&metadata) || !metadata.is_dir() => {
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
        // catalog 锁保护共享目录库元数据，不代表本数据位置不可写。
        write_allowed: !data_location_lock_held,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Write};
    use std::process::{Command, Stdio};

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
    fn try_sibling_resource_lock_returns_none_only_for_contention() {
        let parent = tempfile::tempdir().unwrap();
        let resource_path = parent.path().join("shared.json");
        let lock = FileOperationLock::acquire_sibling_resource(&resource_path).unwrap();

        assert!(
            FileOperationLock::try_acquire_sibling_resource(&resource_path)
                .unwrap()
                .is_none()
        );
        assert!(matches!(
            FileOperationLock::try_acquire_sibling_resource(Path::new("invalid")),
            Err(OperationLeaseError::InvalidResourcePath)
        ));
        drop(lock);
        assert!(
            FileOperationLock::try_acquire_sibling_resource(&resource_path)
                .unwrap()
                .is_some()
        );
    }

    #[test]
    fn child_process_cannot_acquire_existing_path_lease() {
        let (Ok(root), Ok(resource_path)) = (
            std::env::var("TRAE_SYNC_PATH_LOCK_CHILD_ROOT"),
            std::env::var("TRAE_SYNC_PATH_LOCK_CHILD_RESOURCE"),
        ) else {
            return;
        };
        assert!(matches!(
            OperationLease::acquire_shared_for_path(Path::new(&root), Path::new(&resource_path)),
            Err(OperationLeaseError::DataLocationBusy)
        ));
    }

    /// 子进程持有租约并报告就绪，供父进程验证进程退出会释放系统锁。
    #[test]
    fn child_process_holds_path_lease_until_terminated() {
        let (Ok(root), Ok(resource_path)) = (
            std::env::var("TRAE_SYNC_PATH_LOCK_HOLD_ROOT"),
            std::env::var("TRAE_SYNC_PATH_LOCK_HOLD_RESOURCE"),
        ) else {
            return;
        };
        let _lease =
            OperationLease::acquire_shared_for_path(Path::new(&root), Path::new(&resource_path))
                .unwrap();
        let mut stdout = std::io::stdout().lock();
        writeln!(stdout, "PATH_LOCK_READY").unwrap();
        stdout.flush().unwrap();
        let mut line = String::new();
        let _ = std::io::stdin().lock().read_line(&mut line);
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
    fn shared_lease_keeps_catalog_lock_independent_from_other_locations() {
        let root = tempfile::tempdir().unwrap();
        let catalog_lease = OperationLease::acquire(root.path(), "loc-one").unwrap();
        let independent_lease = OperationLease::acquire_shared(root.path(), "loc-two").unwrap();
        drop(independent_lease);
        let status = inspect_lock_status(root.path(), "loc-three").unwrap();
        assert!(status.catalog_lock_held);
        assert!(!status.data_location_lock_held);
        assert!(status.write_allowed);
        drop(catalog_lease);
    }

    #[test]
    fn path_lease_uses_parent_identity_and_allows_independent_directories() {
        let recovery_root = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir().unwrap();
        let parent = root.path().join("data");
        fs::create_dir_all(&parent).unwrap();
        let work_path = parent.join("work");
        let cn_path = parent.join("cn");
        fs::create_dir_all(&work_path).unwrap();
        fs::create_dir_all(&cn_path).unwrap();
        let alias = parent.join(".").join("work");

        let work_lease =
            OperationLease::acquire_shared_for_path(recovery_root.path(), &work_path).unwrap();
        assert!(matches!(
            OperationLease::acquire_shared_for_path(recovery_root.path(), &alias),
            Err(OperationLeaseError::DataLocationBusy)
        ));
        let cn_lease =
            OperationLease::acquire_shared_for_path(recovery_root.path(), &cn_path).unwrap();
        drop(cn_lease);
        drop(work_lease);
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

    /// 锁标记无法打开时应报告锁目录不可用，不能伪报成并发占用。
    #[test]
    fn resource_lock_file_open_failure_is_not_reported_as_busy() {
        let recovery_root = tempfile::tempdir().unwrap();
        let resource_parent = tempfile::tempdir().unwrap();
        let resource_path = resource_parent.path().join("work-data");
        fs::create_dir(&resource_path).unwrap();
        let lock_dir = recovery_root.path().join("locks");
        fs::create_dir_all(&lock_dir).unwrap();
        let resource_id = resource_location_id(&resource_path).unwrap();
        fs::create_dir(lock_dir.join(format!("data-{}.lock", hash_id(&resource_id)))).unwrap();

        assert_eq!(
            OperationLease::acquire_shared_for_path(recovery_root.path(), &resource_path)
                .unwrap_err(),
            OperationLeaseError::LockDirectoryUnavailable
        );
    }

    /// 目录库锁标记损坏也应报告锁目录不可用。
    #[test]
    fn catalog_lock_file_open_failure_is_not_reported_as_busy() {
        let root = tempfile::tempdir().unwrap();
        let lock_dir = root.path().join("locks");
        fs::create_dir_all(&lock_dir).unwrap();
        fs::create_dir(lock_dir.join("catalog-write.lock")).unwrap();

        assert_eq!(
            OperationLease::acquire(root.path(), "loc-one").unwrap_err(),
            OperationLeaseError::LockDirectoryUnavailable
        );
    }

    /// 目录库数据锁标记损坏同样应报告锁目录不可用。
    #[test]
    fn legacy_data_lock_file_open_failure_is_not_reported_as_busy() {
        let root = tempfile::tempdir().unwrap();
        let lock_dir = root.path().join("locks");
        fs::create_dir_all(&lock_dir).unwrap();
        fs::create_dir(lock_dir.join(format!("data-{}.lock", hash_id("loc-one")))).unwrap();

        assert_eq!(
            OperationLease::acquire(root.path(), "loc-one").unwrap_err(),
            OperationLeaseError::LockDirectoryUnavailable
        );
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

    // ===== P16-T3：open 阶段 os error 5 短退避重试（注入式单测）=====

    /// mock open 序列：首次返回 os error 5（过滤驱动扫描窗口），重试成功。
    #[test]
    fn open_lock_file_retries_once_on_access_denied() {
        let target = tempfile::tempdir().unwrap();
        let file_path = target.path().join("probe.lock");
        let attempts = std::cell::Cell::new(0_u32);
        // 注入 open：首次 os error 5，第二次真实打开成功。
        let mock = |path: &Path| -> io::Result<File> {
            let attempt = attempts.get();
            attempts.set(attempt + 1);
            if attempt == 0 {
                return Err(io::Error::from_raw_os_error(5));
            }
            OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(false)
                .open(path)
        };
        let file = open_lock_file_with_retry(&file_path, &mock, Duration::from_millis(1))
            .expect("os error 5 后重试应成功");
        drop(file);
        assert_eq!(attempts.get(), 2, "os error 5 应恰好重试一次（共两次 open）");
    }

    /// 非 error 5（如 NotFound）不重试：一次 open 直接返回原错误。
    #[test]
    fn open_lock_file_does_not_retry_other_errors() {
        let target = tempfile::tempdir().unwrap();
        let file_path = target.path().join("missing-parent").join("probe.lock");
        let attempts = std::cell::Cell::new(0_u32);
        let mock = |_path: &Path| -> io::Result<File> {
            attempts.set(attempts.get() + 1);
            Err(io::Error::from_raw_os_error(3)) // ERROR_PATH_NOT_FOUND
        };
        let result = open_lock_file_with_retry(&file_path, &mock, Duration::from_millis(1));
        assert_eq!(result.unwrap_err().raw_os_error(), Some(3));
        assert_eq!(attempts.get(), 1, "非 error 5 不得重试");
    }

    /// reparse 拒绝路径（无 raw os error 的 PermissionDenied）不触发重试。
    #[test]
    fn open_lock_file_does_not_retry_reparse_rejection() {
        let target = tempfile::tempdir().unwrap();
        let link_path = target.path().join("link.lock");
        let real = target.path().join("real.lock");
        OpenOptions::new()
            .write(true)
            .create(true)
            .open(&real)
            .unwrap();
        // unix 直接建符号链接；windows 无特权时降级为普通文件，此时该用例
        // 只验证「mock 返回非 5 错误不重试」语义，仍有效。
        #[cfg(unix)]
        std::os::unix::fs::symlink(&real, &link_path).unwrap();
        #[cfg(not(unix))]
        std::fs::rename(&real, &link_path).unwrap();

        let attempts = std::cell::Cell::new(0_u32);
        let mock = |_path: &Path| -> io::Result<File> {
            attempts.set(attempts.get() + 1);
            Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "锁标记不能是符号链接",
            ))
        };
        let result = open_lock_file_with_retry(&link_path, &mock, Duration::from_millis(1));
        assert!(result.is_err());
        assert_eq!(attempts.get(), 1, "无 raw os error 的拒绝不得重试");
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

    #[test]
    fn second_process_is_blocked_by_path_resource_lock() {
        let recovery_root = tempfile::tempdir().unwrap();
        let parent = tempfile::tempdir().unwrap();
        let resource_path = parent.path().join("work-data");
        fs::create_dir(&resource_path).unwrap();
        let resource_alias = parent.path().join(".").join("work-data");
        let _lease =
            OperationLease::acquire_shared_for_path(recovery_root.path(), &resource_path).unwrap();
        let status = Command::new(std::env::current_exe().unwrap())
            .env("TRAE_SYNC_PATH_LOCK_CHILD_ROOT", recovery_root.path())
            .env("TRAE_SYNC_PATH_LOCK_CHILD_RESOURCE", &resource_alias)
            .args([
                "--exact",
                "operation_lease::tests::child_process_cannot_acquire_existing_path_lease",
                "--nocapture",
            ])
            .status()
            .unwrap();

        assert!(status.success());
    }

    /// 父进程强制结束持锁子进程后，应能重新取得同一资源租约。
    #[test]
    fn terminated_process_releases_path_resource_lock() {
        let recovery_root = tempfile::tempdir().unwrap();
        let parent = tempfile::tempdir().unwrap();
        let resource_path = parent.path().join("work-data");
        fs::create_dir(&resource_path).unwrap();
        let mut child = Command::new(std::env::current_exe().unwrap())
            .env("TRAE_SYNC_PATH_LOCK_HOLD_ROOT", recovery_root.path())
            .env("TRAE_SYNC_PATH_LOCK_HOLD_RESOURCE", &resource_path)
            .args([
                "--exact",
                "operation_lease::tests::child_process_holds_path_lease_until_terminated",
                "--nocapture",
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let stdout = child.stdout.take().unwrap();
        let mut reader = BufReader::new(stdout);
        let mut line = String::new();
        loop {
            line.clear();
            assert!(reader.read_line(&mut line).unwrap() > 0);
            if line.trim() == "PATH_LOCK_READY" {
                break;
            }
        }

        child.kill().unwrap();
        child.wait().unwrap();
        assert!(
            OperationLease::acquire_shared_for_path(recovery_root.path(), &resource_path).is_ok()
        );
    }
}
