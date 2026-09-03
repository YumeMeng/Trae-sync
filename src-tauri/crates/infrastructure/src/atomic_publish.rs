use std::fs;
use std::path::Path;

/// 以同卷原子替换发布临时文件；失败时清理本次调用创建的临时文件。
pub(crate) fn publish_replacing(temporary: &Path, destination: &Path) -> std::io::Result<()> {
    if fs::symlink_metadata(destination)
        .map(|metadata| metadata.file_type().is_symlink())
        .unwrap_or(false)
    {
        let result = Err(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            "拒绝替换符号链接目标",
        ));
        let _ = fs::remove_file(temporary);
        return result;
    }

    let result = publish_replacing_platform(temporary, destination);
    if result.is_err() {
        // 临时文件不是可恢复状态，失败后立即清理，避免每轮操作堆积残留。
        let _ = fs::remove_file(temporary);
    }
    result
}

#[cfg(windows)]
fn publish_replacing_platform(temporary: &Path, destination: &Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
    };

    let from: Vec<u16> = temporary
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let to: Vec<u16> = destination
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let result = unsafe {
        MoveFileExW(
            from.as_ptr(),
            to.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if result == 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(not(windows))]
fn publish_replacing_platform(temporary: &Path, destination: &Path) -> std::io::Result<()> {
    fs::rename(temporary, destination)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failed_publication_removes_temporary_file() {
        let directory = tempfile::tempdir().unwrap();
        let temporary = directory.path().join("pointer.tmp");
        let destination = directory.path().join("current.json");
        fs::write(&temporary, b"pointer").unwrap();
        fs::create_dir(&destination).unwrap();

        assert!(publish_replacing(&temporary, &destination).is_err());
        assert!(!temporary.exists());
    }
}
