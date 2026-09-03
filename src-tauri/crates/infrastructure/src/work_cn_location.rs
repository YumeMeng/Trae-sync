//! TRAE Work CN 默认数据位置的只读发现与边界校验。
//!
//! 生产入口只从 `APPDATA` 发现固定位置，不接受调用者传入源数据库路径。
//! 本模块不提供任何源数据库写入目标 API。

use std::env;
use std::fs;
use std::path::{Component, Path, PathBuf};

use crate::data_location::{capture_location_identity, LocationWitness, WitnessError};
use crate::file_identity::PlatformFileIdentityProvider;
use traesync_ports::FileIdentityProvider;

/// TRAE Work CN 默认用户数据根目录名。
pub const DEFAULT_WORK_CN_ROOT_NAME: &str = "TRAE SOLO CN";

/// TRAE Work CN 默认活动数据库相对路径。
pub const DEFAULT_WORK_CN_DB_RELATIVE_PATH: &str = "ModularData/ai-agent/database.db";

/// 真实只读位置发现失败原因。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkCnReadLocationError {
    /// APPDATA 未设置。
    AppdataMissing,
    /// APPDATA 必须为绝对路径。
    AppdataNotAbsolute { raw: String },
    /// APPDATA 无法规范化或不是目录。
    AppdataUnavailable { raw: String },
    /// 默认 TRAE 根目录不存在或不是目录。
    RootMissing { path: String },
    /// 默认 TRAE 根目录包含符号链接或 junction。
    RootReparsePoint { path: String },
    /// 规范化根目录不再等于 APPDATA 下固定默认根。
    RootPathMismatch { expected: String, actual: String },
    /// 默认数据库不存在或不是普通文件。
    DatabaseMissing { path: String },
    /// 数据库相对路径不符合固定安全规则。
    InvalidDatabasePath,
    /// 数据库路径或其父级包含符号链接或 junction。
    DatabaseReparsePoint { path: String },
    /// 数据库规范化后逃出固定根目录。
    DatabaseOutsideRoot { path: String },
    /// 读身份捕获失败。
    Identity(WitnessError),
}
impl std::fmt::Display for WorkCnReadLocationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::AppdataMissing => f.write_str("APPDATA 未设置"),
            Self::AppdataNotAbsolute { raw } => write!(f, "APPDATA 必须为绝对路径: {raw}"),
            Self::AppdataUnavailable { raw } => write!(f, "APPDATA 不可用: {raw}"),
            Self::RootMissing { path } => write!(f, "TRAE 默认根目录不存在: {path}"),
            Self::RootReparsePoint { path } => {
                write!(f, "TRAE 默认根目录不能是符号链接或 junction: {path}")
            }
            Self::RootPathMismatch { expected, actual } => {
                write!(
                    f,
                    "规范化 TRAE 根目录偏离默认位置: expected={expected}, actual={actual}"
                )
            }
            Self::DatabaseMissing { path } => write!(f, "TRAE 默认数据库不存在: {path}"),
            Self::InvalidDatabasePath => f.write_str("TRAE 默认数据库相对路径不安全"),
            Self::DatabaseReparsePoint { path } => {
                write!(f, "TRAE 数据库路径不能包含符号链接或 junction: {path}")
            }
            Self::DatabaseOutsideRoot { path } => {
                write!(f, "TRAE 数据库规范化后逃出默认根目录: {path}")
            }
            Self::Identity(error) => write!(f, "TRAE 数据位置身份捕获失败: {error}"),
        }
    }
}

impl std::error::Error for WorkCnReadLocationError {}

impl From<WitnessError> for WorkCnReadLocationError {
    fn from(error: WitnessError) -> Self {
        Self::Identity(error)
    }
}

/// 已通过固定路径边界校验的 TRAE Work CN 只读位置。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkCnReadLocation {
    canonical_root: PathBuf,
    db_relative_path: PathBuf,
    canonical_db_path: PathBuf,
}

impl WorkCnReadLocation {
    /// 从当前 Windows 用户 APPDATA 发现固定 TRAE Work CN 位置。
    ///
    /// 不接受调用者路径，根目录和数据库均须已存在；本方法不写入任何文件。
    pub fn discover() -> Result<Self, WorkCnReadLocationError> {
        let appdata = env::var_os("APPDATA").ok_or(WorkCnReadLocationError::AppdataMissing)?;
        Self::discover_from_appdata(Path::new(&appdata))
    }

    /// 返回已规范化的固定 TRAE 根目录。
    pub fn canonical_root(&self) -> &Path {
        &self.canonical_root
    }

    /// 返回固定数据库相对路径。
    pub fn db_relative_path(&self) -> &Path {
        &self.db_relative_path
    }

    /// 返回已规范化的固定数据库路径。
    pub fn canonical_db_path(&self) -> &Path {
        &self.canonical_db_path
    }

    /// 捕获根目录、数据库及可选 sidecar 的读身份。
    ///
    /// 调用只读取源位置，不创建、修改或删除源文件。
    pub fn capture_read_identity(
        &self,
        provider: &dyn FileIdentityProvider,
    ) -> Result<LocationWitness, WorkCnReadLocationError> {
        capture_location_identity(
            provider,
            &self.canonical_root,
            DEFAULT_WORK_CN_DB_RELATIVE_PATH,
        )
        .map_err(Into::into)
    }

    /// 使用平台身份提供器捕获读身份。
    pub fn capture_platform_read_identity(
        &self,
    ) -> Result<LocationWitness, WorkCnReadLocationError> {
        self.capture_read_identity(&PlatformFileIdentityProvider::new())
    }

    fn discover_from_appdata(appdata: &Path) -> Result<Self, WorkCnReadLocationError> {
        if !appdata.is_absolute() {
            return Err(WorkCnReadLocationError::AppdataNotAbsolute {
                raw: appdata.to_string_lossy().into_owned(),
            });
        }

        let appdata_metadata = fs::symlink_metadata(appdata).map_err(|_| {
            WorkCnReadLocationError::AppdataUnavailable {
                raw: appdata.to_string_lossy().into_owned(),
            }
        })?;
        if !appdata_metadata.is_dir() || is_reparse_point(&appdata_metadata) {
            return Err(WorkCnReadLocationError::AppdataUnavailable {
                raw: appdata.to_string_lossy().into_owned(),
            });
        }

        let canonical_appdata =
            appdata
                .canonicalize()
                .map_err(|_| WorkCnReadLocationError::AppdataUnavailable {
                    raw: appdata.to_string_lossy().into_owned(),
                })?;
        let expected_root = appdata.join(DEFAULT_WORK_CN_ROOT_NAME);
        let root_metadata = fs::symlink_metadata(&expected_root).map_err(|_| {
            WorkCnReadLocationError::RootMissing {
                path: expected_root.to_string_lossy().into_owned(),
            }
        })?;
        if is_reparse_point(&root_metadata) {
            return Err(WorkCnReadLocationError::RootReparsePoint {
                path: expected_root.to_string_lossy().into_owned(),
            });
        }
        if !root_metadata.is_dir() {
            return Err(WorkCnReadLocationError::RootMissing {
                path: expected_root.to_string_lossy().into_owned(),
            });
        }
        reject_reparse_chain(&expected_root).map_err(|path| {
            WorkCnReadLocationError::RootReparsePoint {
                path: path.to_string_lossy().into_owned(),
            }
        })?;

        let canonical_root =
            expected_root
                .canonicalize()
                .map_err(|_| WorkCnReadLocationError::RootMissing {
                    path: expected_root.to_string_lossy().into_owned(),
                })?;
        if !canonical_root.is_dir() {
            return Err(WorkCnReadLocationError::RootMissing {
                path: expected_root.to_string_lossy().into_owned(),
            });
        }

        let expected_canonical_root = canonical_appdata.join(DEFAULT_WORK_CN_ROOT_NAME);
        if !same_path(&canonical_root, &expected_canonical_root) {
            return Err(WorkCnReadLocationError::RootPathMismatch {
                expected: expected_canonical_root.to_string_lossy().into_owned(),
                actual: canonical_root.to_string_lossy().into_owned(),
            });
        }

        let db_relative_path = PathBuf::from(DEFAULT_WORK_CN_DB_RELATIVE_PATH);
        validate_relative_path(&db_relative_path)?;
        let db_path = canonical_root.join(&db_relative_path);
        let db_metadata = fs::symlink_metadata(&db_path).map_err(|_| {
            WorkCnReadLocationError::DatabaseMissing {
                path: db_path.to_string_lossy().into_owned(),
            }
        })?;
        if is_reparse_point(&db_metadata) {
            return Err(WorkCnReadLocationError::DatabaseReparsePoint {
                path: db_path.to_string_lossy().into_owned(),
            });
        }
        if !db_metadata.is_file() {
            return Err(WorkCnReadLocationError::DatabaseMissing {
                path: db_path.to_string_lossy().into_owned(),
            });
        }
        reject_reparse_chain(&db_path).map_err(|path| {
            WorkCnReadLocationError::DatabaseReparsePoint {
                path: path.to_string_lossy().into_owned(),
            }
        })?;

        let canonical_db_path =
            db_path
                .canonicalize()
                .map_err(|_| WorkCnReadLocationError::DatabaseMissing {
                    path: db_path.to_string_lossy().into_owned(),
                })?;
        if !canonical_db_path.is_file() {
            return Err(WorkCnReadLocationError::DatabaseMissing {
                path: db_path.to_string_lossy().into_owned(),
            });
        }
        if !canonical_db_path.starts_with(&canonical_root) {
            return Err(WorkCnReadLocationError::DatabaseOutsideRoot {
                path: canonical_db_path.to_string_lossy().into_owned(),
            });
        }

        Ok(Self {
            canonical_root,
            db_relative_path,
            canonical_db_path,
        })
    }
}

fn validate_relative_path(path: &Path) -> Result<(), WorkCnReadLocationError> {
    if path.as_os_str().is_empty()
        || path.is_absolute()
        || path.components().any(|component| {
            matches!(
                component,
                Component::Prefix(_) | Component::RootDir | Component::ParentDir
            )
        })
    {
        return Err(WorkCnReadLocationError::InvalidDatabasePath);
    }
    Ok(())
}

fn reject_reparse_chain(path: &Path) -> Result<(), PathBuf> {
    let mut current = path.to_path_buf();
    loop {
        if let Ok(metadata) = fs::symlink_metadata(&current) {
            if is_reparse_point(&metadata) {
                return Err(current);
            }
        }
        if !current.pop() || current.as_os_str().is_empty() {
            break;
        }
    }
    Ok(())
}

fn is_reparse_point(metadata: &fs::Metadata) -> bool {
    if metadata.file_type().is_symlink() {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        // FILE_ATTRIBUTE_REPARSE_POINT；涵盖 junction 等 Windows 重解析点。
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

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture_appdata() -> tempfile::TempDir {
        let appdata = tempfile::tempdir().unwrap();
        let db = appdata
            .path()
            .join(DEFAULT_WORK_CN_ROOT_NAME)
            .join("ModularData")
            .join("ai-agent");
        fs::create_dir_all(&db).unwrap();
        fs::write(db.join("database.db"), b"fixture database").unwrap();
        appdata
    }

    fn make_directory_link(target: &Path, link: &Path) -> bool {
        #[cfg(windows)]
        {
            std::process::Command::new("cmd")
                .args([
                    "/C",
                    "mklink",
                    "/J",
                    &link.to_string_lossy(),
                    &target.to_string_lossy(),
                ])
                .status()
                .map(|status| status.success())
                .unwrap_or(false)
        }
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(target, link).is_ok()
        }
        #[cfg(not(any(windows, unix)))]
        {
            let _ = (target, link);
            false
        }
    }

    #[test]
    fn discovers_fixed_paths_without_touching_source() {
        let appdata = fixture_appdata();
        let location = WorkCnReadLocation::discover_from_appdata(appdata.path()).unwrap();
        assert!(location
            .canonical_root()
            .ends_with(DEFAULT_WORK_CN_ROOT_NAME));
        assert_eq!(
            location.db_relative_path(),
            Path::new(DEFAULT_WORK_CN_DB_RELATIVE_PATH)
        );
        assert!(location.canonical_db_path().is_file());
    }

    #[test]
    fn missing_root_and_database_fail_closed() {
        let appdata = tempfile::tempdir().unwrap();
        assert!(matches!(
            WorkCnReadLocation::discover_from_appdata(appdata.path()),
            Err(WorkCnReadLocationError::RootMissing { .. })
        ));

        let root = appdata.path().join(DEFAULT_WORK_CN_ROOT_NAME);
        fs::create_dir_all(root.join("ModularData").join("ai-agent")).unwrap();
        assert!(matches!(
            WorkCnReadLocation::discover_from_appdata(appdata.path()),
            Err(WorkCnReadLocationError::DatabaseMissing { .. })
        ));
    }

    #[test]
    fn relative_path_rule_rejects_parent_and_absolute_components() {
        assert!(validate_relative_path(Path::new("../database.db")).is_err());
        assert!(validate_relative_path(Path::new("/database.db")).is_err());
        assert!(validate_relative_path(Path::new("C:\\database.db")).is_err());
    }

    #[test]
    fn root_reparse_point_is_rejected() {
        let appdata = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let root = appdata.path().join(DEFAULT_WORK_CN_ROOT_NAME);
        assert!(
            make_directory_link(outside.path(), &root),
            "无法建立 symlink/junction fixture"
        );
        assert!(matches!(
            WorkCnReadLocation::discover_from_appdata(appdata.path()),
            Err(WorkCnReadLocationError::RootReparsePoint { .. })
                | Err(WorkCnReadLocationError::RootPathMismatch { .. })
        ));
    }

    #[test]
    fn database_parent_reparse_point_is_rejected() {
        let appdata = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let root = appdata.path().join(DEFAULT_WORK_CN_ROOT_NAME);
        fs::create_dir_all(&root).unwrap();
        let outside_data = outside.path().join("ModularData");
        fs::create_dir_all(outside_data.join("ai-agent")).unwrap();
        fs::write(
            outside_data.join("ai-agent").join("database.db"),
            b"outside",
        )
        .unwrap();
        assert!(
            make_directory_link(&outside_data, &root.join("ModularData")),
            "无法建立 symlink/junction fixture"
        );
        assert!(matches!(
            WorkCnReadLocation::discover_from_appdata(appdata.path()),
            Err(WorkCnReadLocationError::DatabaseReparsePoint { .. })
        ));
    }

    #[test]
    fn read_identity_uses_existing_location_only() {
        let appdata = fixture_appdata();
        let location = WorkCnReadLocation::discover_from_appdata(appdata.path()).unwrap();
        let before = fs::read(location.canonical_db_path()).unwrap();
        let witness = location.capture_platform_read_identity().unwrap();
        assert!(!witness.data_location_id.is_empty());
        assert_eq!(before, fs::read(location.canonical_db_path()).unwrap());
    }
}
