//! 文件身份提供者：读取文件身份用于捕获前后漂移检测。
//!
//! Windows 实现使用 `GetFileInformationByHandle` 获取卷序列号和文件索引。
//! 非 Windows 平台使用 inode + dev 作为退化实现（仅测试用）。
//!
//! 文件身份用于检测文件是否被替换（即使路径相同）——
//! 捕获前后文件身份变化时废弃本次快照。

use std::path::Path;
use traesync_domain::FileIdentity;
use traesync_ports::FileIdentityProvider;

/// 平台文件身份提供者。
///
/// Windows 上使用 Win32 API；其他平台用 std::fs::Metadata 的 inode。
/// fixture 测试中，文件身份用于检测捕获前后文件是否被替换。
pub struct PlatformFileIdentityProvider;

impl Default for PlatformFileIdentityProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl PlatformFileIdentityProvider {
    pub fn new() -> Self {
        Self
    }
}

impl FileIdentityProvider for PlatformFileIdentityProvider {
    fn read_file_identity(&self, path: &Path) -> Option<FileIdentity> {
        #[cfg(windows)]
        {
            read_file_identity_windows(path)
        }
        #[cfg(not(windows))]
        {
            read_file_identity_unix(path)
        }
    }
}

#[cfg(windows)]
fn read_file_identity_windows(path: &Path) -> Option<FileIdentity> {
    use std::os::windows::io::AsRawHandle;
    // 打开文件（不删除/不修改），读取 BY_HANDLE_FILE_INFORMATION
    let file = std::fs::OpenOptions::new().read(true).open(path).ok()?;

    let handle = file.as_raw_handle();
    // SAFETY: 调用 Win32 GetFileInformationByHandle，handle 来自合法的 std::fs::File
    unsafe {
        #[repr(C)]
        #[derive(Default)]
        struct ByHandleFileInformation {
            dw_file_attributes: u32,
            ft_creation_time_low: u32,
            ft_creation_time_high: u32,
            ft_last_access_time_low: u32,
            ft_last_access_time_high: u32,
            ft_last_write_time_low: u32,
            ft_last_write_time_high: u32,
            dw_volume_serial_number: u32,
            n_file_size_high: u32,
            n_file_size_low: u32,
            n_number_of_links: u32,
            n_file_index_high: u32,
            n_file_index_low: u32,
        }

        extern "system" {
            fn GetFileInformationByHandle(handle: isize, info: *mut ByHandleFileInformation)
                -> i32;
        }

        let mut info = ByHandleFileInformation::default();
        let ok = GetFileInformationByHandle(handle as isize, &mut info);
        if ok == 0 {
            return None;
        }
        Some(FileIdentity {
            volume_serial: info.dw_volume_serial_number as u64,
            file_index_high: info.n_file_index_high as u64,
            file_index_low: info.n_file_index_low as u64,
        })
    }
}

#[cfg(not(windows))]
fn read_file_identity_unix(path: &Path) -> Option<FileIdentity> {
    use std::os::unix::fs::MetadataExt;
    let meta = std::fs::metadata(path).ok()?;
    Some(FileIdentity {
        volume_serial: meta.dev(),
        file_index_high: 0,
        file_index_low: meta.ino(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_identity_for_existing_file() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("test.db");
        std::fs::write(&file_path, b"content").unwrap();
        let provider = PlatformFileIdentityProvider::new();
        let id = provider.read_file_identity(&file_path);
        assert!(id.is_some(), "应能读取存在文件的 identity");
    }

    #[test]
    fn read_identity_returns_none_for_missing_file() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("nonexistent.db");
        let provider = PlatformFileIdentityProvider::new();
        let id = provider.read_file_identity(&missing);
        assert!(id.is_none(), "不存在文件应返回 None");
    }

    #[test]
    fn same_file_same_identity() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("stable.db");
        std::fs::write(&file_path, b"content").unwrap();
        let provider = PlatformFileIdentityProvider::new();
        let id1 = provider.read_file_identity(&file_path);
        let id2 = provider.read_file_identity(&file_path);
        assert_eq!(id1, id2, "同一文件身份应稳定");
    }
}
