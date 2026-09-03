//! T08 数据位置身份与文件见证。
//!
//! 路径只用于展示和重新定位；真正的绑定同时包含规范化路径、文件身份、存在性和哈希。
//! 同路径替换文件会得到新的 `data_location_id` 或至少触发见证漂移，调用方必须 fail closed。

use std::fs::{self, File};
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use traesync_domain::FileIdentity;
use traesync_ports::FileIdentityProvider;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocationWitness {
    pub data_location_id: String,
    pub canonical_root: String,
    pub db_relative_path: String,
    pub root_identity: Option<FileIdentity>,
    pub db_identity: Option<FileIdentity>,
    pub wal_identity: Option<FileIdentity>,
    pub shm_identity: Option<FileIdentity>,
    pub db_sha256: Option<String>,
    pub wal_sha256: Option<String>,
    pub shm_sha256: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WitnessMismatch {
    RootPath,
    DatabasePath,
    RootIdentity,
    DatabaseIdentity,
    WalIdentity,
    ShmIdentity,
    DatabaseHash,
    WalHash,
    ShmHash,
    DataLocationId,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WitnessError {
    RootMissing,
    DatabaseMissing,
    InvalidDatabasePath,
    SymlinkRejected,
    RootCanonicalizeFailed,
    DatabaseOutsideRoot,
    IdentityUnavailable,
    HashFailed,
}

impl std::fmt::Display for WitnessError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let message = match self {
            Self::RootMissing => "数据位置根目录不存在",
            Self::DatabaseMissing => "数据位置数据库不存在",
            Self::InvalidDatabasePath => "数据位置数据库路径必须是安全相对路径",
            Self::SymlinkRejected => "数据位置文件不能是符号链接",
            Self::RootCanonicalizeFailed => "数据位置根目录无法规范化",
            Self::DatabaseOutsideRoot => "数据库不在数据位置根目录内",
            Self::IdentityUnavailable => "无法读取数据位置文件身份",
            Self::HashFailed => "无法计算数据位置文件哈希",
        };
        f.write_str(message)
    }
}

impl std::error::Error for WitnessError {}

pub fn capture_location_witness(
    provider: &dyn FileIdentityProvider,
    root: &Path,
    db_relative_path: &str,
) -> Result<LocationWitness, WitnessError> {
    capture_location_witness_inner(provider, root, db_relative_path, true)
}

/// 捕获不读取完整文件内容的位置身份。
///
/// 执行器在事务边界内会多次复核位置；这些复核只需要根目录和数据库文件身份，
/// 不应因为重复计算大文件哈希拖慢写入路径。
pub fn capture_location_identity(
    provider: &dyn FileIdentityProvider,
    root: &Path,
    db_relative_path: &str,
) -> Result<LocationWitness, WitnessError> {
    capture_location_witness_inner(provider, root, db_relative_path, false)
}

fn capture_location_witness_inner(
    provider: &dyn FileIdentityProvider,
    root: &Path,
    db_relative_path: &str,
    include_hashes: bool,
) -> Result<LocationWitness, WitnessError> {
    validate_relative_database_path(db_relative_path)?;
    let canonical_root = root
        .canonicalize()
        .map_err(|_| WitnessError::RootCanonicalizeFailed)?;
    if !canonical_root.is_dir() {
        return Err(WitnessError::RootMissing);
    }

    let db_path = canonical_root.join(db_relative_path);
    reject_symlink_components(&canonical_root, Path::new(db_relative_path))?;
    let canonical_db = db_path
        .canonicalize()
        .map_err(|_| WitnessError::DatabaseMissing)?;
    if !canonical_db.is_file() {
        return Err(WitnessError::DatabaseMissing);
    }
    if !canonical_db.starts_with(&canonical_root) {
        return Err(WitnessError::DatabaseOutsideRoot);
    }

    let wal_path = sidecar(&db_path, "-wal");
    let shm_path = sidecar(&db_path, "-shm");
    reject_optional_symlink(&wal_path)?;
    reject_optional_symlink(&shm_path)?;
    let root_identity = provider
        .read_file_identity(&canonical_root)
        .ok_or(WitnessError::IdentityUnavailable)?;
    let (db_identity, db_sha256) =
        capture_file_observation(provider, &db_path, true, include_hashes)?;
    let (wal_identity, wal_sha256) =
        capture_file_observation(provider, &wal_path, false, include_hashes)?;
    let (shm_identity, shm_sha256) =
        capture_file_observation(provider, &shm_path, false, include_hashes)?;
    let db_identity = db_identity.ok_or(WitnessError::DatabaseMissing)?;
    if provider.read_file_identity(&canonical_root) != Some(root_identity.clone())
        || db_path.canonicalize().ok().as_deref() != Some(canonical_db.as_path())
    {
        return Err(WitnessError::HashFailed);
    }

    let canonical_root_text = canonical_root.to_string_lossy().into_owned();
    let normalized_relative = canonical_db
        .strip_prefix(&canonical_root)
        .map_err(|_| WitnessError::DatabaseOutsideRoot)?
        .to_string_lossy()
        .trim_start_matches(['\\', '/'])
        .to_string();
    let data_location_id = stable_location_id(
        &canonical_root_text,
        &normalized_relative,
        &root_identity,
        &db_identity,
    );

    Ok(LocationWitness {
        data_location_id,
        canonical_root: canonical_root_text,
        db_relative_path: normalized_relative,
        root_identity: Some(root_identity),
        db_identity: Some(db_identity),
        wal_identity,
        shm_identity,
        db_sha256,
        wal_sha256,
        shm_sha256,
    })
}

pub fn compare_location_witness(
    expected: &LocationWitness,
    actual: &LocationWitness,
) -> Vec<WitnessMismatch> {
    let mut mismatches = Vec::new();
    if expected.canonical_root != actual.canonical_root {
        mismatches.push(WitnessMismatch::RootPath);
    }
    if expected.db_relative_path != actual.db_relative_path {
        mismatches.push(WitnessMismatch::DatabasePath);
    }
    if expected.root_identity != actual.root_identity {
        mismatches.push(WitnessMismatch::RootIdentity);
    }
    if expected.db_identity != actual.db_identity {
        mismatches.push(WitnessMismatch::DatabaseIdentity);
    }
    if expected.wal_identity != actual.wal_identity {
        mismatches.push(WitnessMismatch::WalIdentity);
    }
    if expected.shm_identity != actual.shm_identity {
        mismatches.push(WitnessMismatch::ShmIdentity);
    }
    if expected.db_sha256 != actual.db_sha256 {
        mismatches.push(WitnessMismatch::DatabaseHash);
    }
    if expected.wal_sha256 != actual.wal_sha256 {
        mismatches.push(WitnessMismatch::WalHash);
    }
    if expected.shm_sha256 != actual.shm_sha256 {
        mismatches.push(WitnessMismatch::ShmHash);
    }
    if expected.data_location_id != actual.data_location_id {
        mismatches.push(WitnessMismatch::DataLocationId);
    }
    mismatches
}

/// 比较不读取完整文件哈希的位置身份。
///
/// 读取入口只需要拒绝同路径替换文件；完整哈希仍由扫描见证负责，避免每次浏览、搜索
/// 和正文预览都重新读取大型数据库。
pub fn compare_location_identity(
    expected: &LocationWitness,
    actual: &LocationWitness,
) -> Vec<WitnessMismatch> {
    let mut mismatches = Vec::new();
    if expected.canonical_root != actual.canonical_root {
        mismatches.push(WitnessMismatch::RootPath);
    }
    if expected.db_relative_path != actual.db_relative_path {
        mismatches.push(WitnessMismatch::DatabasePath);
    }
    if expected.root_identity != actual.root_identity {
        mismatches.push(WitnessMismatch::RootIdentity);
    }
    if expected.db_identity != actual.db_identity {
        mismatches.push(WitnessMismatch::DatabaseIdentity);
    }
    if expected.wal_identity != actual.wal_identity {
        mismatches.push(WitnessMismatch::WalIdentity);
    }
    if expected.shm_identity != actual.shm_identity {
        mismatches.push(WitnessMismatch::ShmIdentity);
    }
    if expected.data_location_id != actual.data_location_id {
        mismatches.push(WitnessMismatch::DataLocationId);
    }
    mismatches
}

fn sidecar(path: &Path, suffix: &str) -> PathBuf {
    let mut value = path.as_os_str().to_os_string();
    value.push(suffix);
    PathBuf::from(value)
}

fn validate_relative_database_path(relative: &str) -> Result<(), WitnessError> {
    let path = Path::new(relative);
    if relative.is_empty()
        || path.is_absolute()
        || path.components().any(|component| {
            matches!(
                component,
                Component::Prefix(_) | Component::RootDir | Component::ParentDir
            )
        })
    {
        return Err(WitnessError::InvalidDatabasePath);
    }
    Ok(())
}

fn reject_symlink_components(root: &Path, relative: &Path) -> Result<(), WitnessError> {
    let mut current = root.to_path_buf();
    for component in relative.components() {
        let Component::Normal(name) = component else {
            continue;
        };
        current.push(name);
        let metadata = fs::symlink_metadata(&current).map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                WitnessError::DatabaseMissing
            } else {
                WitnessError::HashFailed
            }
        })?;
        if metadata.file_type().is_symlink() {
            return Err(WitnessError::SymlinkRejected);
        }
    }
    Ok(())
}

fn reject_optional_symlink(path: &Path) -> Result<(), WitnessError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => Err(WitnessError::SymlinkRejected),
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err(WitnessError::HashFailed),
    }
}

fn capture_file_observation(
    provider: &dyn FileIdentityProvider,
    path: &Path,
    required: bool,
    include_hash: bool,
) -> Result<(Option<FileIdentity>, Option<String>), WitnessError> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound && !required => {
            return Ok((None, None));
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(WitnessError::DatabaseMissing);
        }
        Err(_) => return Err(WitnessError::HashFailed),
    };
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(WitnessError::SymlinkRejected);
    }
    let identity_before = provider
        .read_file_identity(path)
        .ok_or(WitnessError::IdentityUnavailable)?;
    if !include_hash {
        return Ok((Some(identity_before), None));
    }
    let mut file = File::open(path).map_err(|_| WitnessError::HashFailed)?;
    let mut digest = Sha256::new();
    // 大文件哈希缓冲区放在堆上，避免位置见证阶段触发线程栈溢出。
    let mut buffer = vec![0_u8; 1024 * 1024];
    loop {
        let count =
            std::io::Read::read(&mut file, &mut buffer).map_err(|_| WitnessError::HashFailed)?;
        if count == 0 {
            break;
        }
        digest.update(&buffer[..count]);
    }
    let identity_after = provider
        .read_file_identity(path)
        .ok_or(WitnessError::IdentityUnavailable)?;
    if identity_before != identity_after {
        return Err(WitnessError::HashFailed);
    }
    Ok((Some(identity_after), Some(hex::encode(digest.finalize()))))
}

fn stable_location_id(
    canonical_root: &str,
    relative_path: &str,
    root_identity: &FileIdentity,
    db_identity: &FileIdentity,
) -> String {
    let mut digest = Sha256::new();
    digest.update(canonical_root.as_bytes());
    digest.update([0]);
    digest.update(relative_path.as_bytes());
    digest.update([0]);
    digest.update(root_identity.volume_serial.to_le_bytes());
    digest.update(root_identity.file_index_high.to_le_bytes());
    digest.update(root_identity.file_index_low.to_le_bytes());
    digest.update([0]);
    digest.update(db_identity.volume_serial.to_le_bytes());
    digest.update(db_identity.file_index_high.to_le_bytes());
    digest.update(db_identity.file_index_low.to_le_bytes());
    format!("loc-{}", hex::encode(digest.finalize()))
}

/// 使用已验证的规范化根、相对路径和数据库身份生成数据位置 ID。
///
/// 快照捕获阶段已经读取过数据库身份；复用这份身份可避免为生成 ID 再次读取文件，
/// 也不会改变捕获期间故障注入测试的读取时序。
pub fn derive_data_location_id(
    canonical_root: &Path,
    relative_path: &str,
    root_identity: Option<&FileIdentity>,
    db_identity: Option<&FileIdentity>,
) -> Option<String> {
    let root_identity = root_identity?;
    let db_identity = db_identity?;
    Some(stable_location_id(
        &canonical_root.to_string_lossy(),
        relative_path,
        root_identity,
        db_identity,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::file_identity::PlatformFileIdentityProvider;

    #[test]
    fn same_location_has_stable_id_and_no_mismatch() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("database.db"), b"db").unwrap();
        let provider = PlatformFileIdentityProvider::new();
        let first = capture_location_witness(&provider, dir.path(), "database.db").unwrap();
        let second = capture_location_witness(&provider, dir.path(), "database.db").unwrap();
        assert_eq!(first.data_location_id, second.data_location_id);
        assert!(compare_location_witness(&first, &second).is_empty());
        assert!(compare_location_identity(&first, &second).is_empty());
    }

    #[test]
    fn identity_comparison_ignores_hashes_but_detects_file_replacement() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("database.db");
        std::fs::write(&db, b"old").unwrap();
        let provider = PlatformFileIdentityProvider::new();
        let before = capture_location_witness(&provider, dir.path(), "database.db").unwrap();

        let mut same_identity_without_hashes = before.clone();
        same_identity_without_hashes.db_sha256 = None;
        same_identity_without_hashes.wal_sha256 = None;
        same_identity_without_hashes.shm_sha256 = None;
        assert!(compare_location_identity(&before, &same_identity_without_hashes).is_empty());

        std::fs::remove_file(&db).unwrap();
        std::fs::write(&db, b"replacement").unwrap();
        let after = capture_location_identity(&provider, dir.path(), "database.db").unwrap();
        assert!(
            compare_location_identity(&before, &after).contains(&WitnessMismatch::DatabaseIdentity)
        );
    }

    #[test]
    fn large_file_witness_hash_survives_small_thread_stack() {
        // 回归：大文件哈希不能把 1 MiB 缓冲区放在线程栈上。
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("database.db"),
            vec![0x5a_u8; 2 * 1024 * 1024],
        )
        .unwrap();
        let root = dir.path().to_path_buf();

        let handle = std::thread::Builder::new()
            .name("small-stack-witness-test".to_string())
            .stack_size(64 * 1024)
            .spawn(move || {
                capture_location_witness(&PlatformFileIdentityProvider::new(), &root, "database.db")
                    .is_ok()
            })
            .unwrap();

        assert!(
            handle.join().expect("小栈线程不应崩溃"),
            "大文件位置见证应完成哈希"
        );
    }

    #[test]
    fn same_path_replacement_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("database.db");
        std::fs::write(&db, b"old").unwrap();
        let provider = PlatformFileIdentityProvider::new();
        let first = capture_location_witness(&provider, dir.path(), "database.db").unwrap();
        std::fs::remove_file(&db).unwrap();
        std::fs::write(&db, b"new").unwrap();
        let second = capture_location_witness(&provider, dir.path(), "database.db").unwrap();
        let mismatch = compare_location_witness(&first, &second);
        assert!(mismatch.contains(&WitnessMismatch::DatabaseHash));
        assert!(mismatch.contains(&WitnessMismatch::DataLocationId));
    }

    #[test]
    fn moving_data_location_root_changes_witness_and_location_id() {
        let parent = tempfile::tempdir().unwrap();
        let original = parent.path().join("original");
        let moved = parent.path().join("moved");
        std::fs::create_dir_all(&original).unwrap();
        std::fs::write(original.join("database.db"), b"db").unwrap();

        let provider = PlatformFileIdentityProvider::new();
        let before = capture_location_witness(&provider, &original, "database.db").unwrap();
        std::fs::rename(&original, &moved).unwrap();
        let after = capture_location_witness(&provider, &moved, "database.db").unwrap();

        let mismatches = compare_location_witness(&before, &after);
        assert!(mismatches.contains(&WitnessMismatch::RootPath));
        assert!(mismatches.contains(&WitnessMismatch::DataLocationId));
        assert_ne!(before.data_location_id, after.data_location_id);
    }

    #[test]
    fn replacing_root_identity_changes_data_location_id_even_when_db_identity_matches() {
        let db_identity = FileIdentity {
            volume_serial: 1,
            file_index_high: 2,
            file_index_low: 3,
        };
        let first_root = FileIdentity {
            volume_serial: 1,
            file_index_high: 4,
            file_index_low: 5,
        };
        let replacement_root = FileIdentity {
            volume_serial: 1,
            file_index_high: 6,
            file_index_low: 7,
        };

        let first = stable_location_id("C:\\fixture", "database.db", &first_root, &db_identity);
        let replacement = stable_location_id(
            "C:\\fixture",
            "database.db",
            &replacement_root,
            &db_identity,
        );

        assert_ne!(first, replacement);
    }

    #[test]
    fn sidecar_presence_is_part_of_witness() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("database.db"), b"db").unwrap();
        let provider = PlatformFileIdentityProvider::new();
        let first = capture_location_witness(&provider, dir.path(), "database.db").unwrap();
        std::fs::write(dir.path().join("database.db-wal"), b"wal").unwrap();
        let second = capture_location_witness(&provider, dir.path(), "database.db").unwrap();
        assert!(compare_location_witness(&first, &second).contains(&WitnessMismatch::WalHash));
    }

    #[test]
    fn unsafe_relative_database_path_is_rejected_before_access() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            capture_location_witness(
                &PlatformFileIdentityProvider::new(),
                dir.path(),
                "../database.db",
            )
            .unwrap_err(),
            WitnessError::InvalidDatabasePath
        );
    }

    #[test]
    fn sidecar_symlink_is_rejected_instead_of_hashed() {
        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let db = dir.path().join("database.db");
        let outside_wal = outside.path().join("outside-wal");
        let wal = dir.path().join("database.db-wal");
        std::fs::write(&db, b"db").unwrap();
        std::fs::write(&outside_wal, b"wal").unwrap();

        #[cfg(windows)]
        let link_result = std::os::windows::fs::symlink_file(&outside_wal, &wal);
        #[cfg(unix)]
        let link_result = std::os::unix::fs::symlink(&outside_wal, &wal);
        #[cfg(not(any(windows, unix)))]
        return;
        if link_result.is_err() {
            return;
        }

        assert_eq!(
            capture_location_witness(
                &PlatformFileIdentityProvider::new(),
                dir.path(),
                "database.db"
            )
            .unwrap_err(),
            WitnessError::SymlinkRejected
        );
    }

    #[test]
    fn missing_required_identity_fails_closed() {
        struct MissingDatabaseIdentity;

        impl FileIdentityProvider for MissingDatabaseIdentity {
            fn read_file_identity(&self, path: &Path) -> Option<FileIdentity> {
                if path.file_name().and_then(|name| name.to_str()) == Some("database.db") {
                    None
                } else {
                    Some(FileIdentity {
                        volume_serial: 1,
                        file_index_high: 0,
                        file_index_low: 1,
                    })
                }
            }
        }

        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("database.db"), b"db").unwrap();
        assert_eq!(
            capture_location_witness(&MissingDatabaseIdentity, dir.path(), "database.db")
                .unwrap_err(),
            WitnessError::IdentityUnavailable
        );
    }
}
