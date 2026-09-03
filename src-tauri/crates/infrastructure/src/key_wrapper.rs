//! T13 Windows DPAPI 密钥包装器。
//!
//! 这里只保存 DPAPI 密文，不保存目录库明文密钥。包装器只在应用层完成
//! 恢复包和目录库只读验证之后被调用；恢复包本身不依赖这个文件。

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use traesync_ports::{KeyWrapperError, KeyWrapperPort, KeyWrapperReceipt, KeyWrapperRequest};

const MAGIC: &[u8; 8] = b"TRSDPAPI";
const FORMAT_VERSION: u32 = 1;
const MAX_BLOB_BYTES: usize = 1024 * 1024;
const KEY_WRAPPER_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ProtectedCatalogKey {
    format_version: u32,
    catalog_id: String,
    key_generation: u32,
    catalog_key_hex: String,
}

/// 当前 Windows 用户的目录库密钥包装文件。
#[derive(Debug, Clone)]
pub struct DpapiKeyWrapper {
    path: PathBuf,
}

impl DpapiKeyWrapper {
    pub const fn wrapper_version() -> u32 {
        KEY_WRAPPER_VERSION
    }

    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// 仅用于启动恢复路径：读取并验证当前用户的 DPAPI 包装。
    pub fn unwrap_catalog_key(
        &self,
        expected_catalog_id: &str,
        expected_key_generation: u32,
    ) -> Result<String, KeyWrapperError> {
        #[cfg(windows)]
        {
            let encrypted = read_blob(&self.path)?;
            let plaintext = unprotect(&encrypted)?;
            let material: ProtectedCatalogKey =
                serde_json::from_slice(&plaintext).map_err(|_| KeyWrapperError::Failed)?;
            validate_protected_material(&material)?;
            if material.catalog_id != expected_catalog_id
                || material.key_generation != expected_key_generation
            {
                return Err(KeyWrapperError::Failed);
            }
            Ok(material.catalog_key_hex)
        }
        #[cfg(not(windows))]
        {
            let _ = (expected_catalog_id, expected_key_generation);
            Err(KeyWrapperError::Unavailable)
        }
    }
}

impl KeyWrapperPort for DpapiKeyWrapper {
    fn supported_wrapper_version(&self) -> u32 {
        KEY_WRAPPER_VERSION
    }

    fn wrap_catalog_key(
        &self,
        request: &KeyWrapperRequest<'_>,
    ) -> Result<KeyWrapperReceipt, KeyWrapperError> {
        #[cfg(windows)]
        {
            validate_material(
                request.catalog_id(),
                request.key_generation(),
                request.catalog_key_hex(),
            )?;
            let material = ProtectedCatalogKey {
                format_version: FORMAT_VERSION,
                catalog_id: request.catalog_id().to_string(),
                key_generation: request.key_generation(),
                catalog_key_hex: request.catalog_key_hex().to_string(),
            };
            let plaintext = serde_json::to_vec(&material).map_err(|_| KeyWrapperError::Failed)?;
            let encrypted = protect(&plaintext)?;
            write_blob(&self.path, &encrypted)?;
            Ok(KeyWrapperReceipt {
                wrapper_version: KEY_WRAPPER_VERSION,
            })
        }
        #[cfg(not(windows))]
        {
            let _ = request;
            Err(KeyWrapperError::Unavailable)
        }
    }
}

fn validate_material(
    catalog_id: &str,
    key_generation: u32,
    catalog_key_hex: &str,
) -> Result<(), KeyWrapperError> {
    if catalog_id.is_empty()
        || key_generation == 0
        || catalog_key_hex.len() != 64
        || hex::decode(catalog_key_hex)
            .map(|key| key.len() != 32)
            .unwrap_or(true)
    {
        return Err(KeyWrapperError::Failed);
    }
    Ok(())
}

fn validate_protected_material(material: &ProtectedCatalogKey) -> Result<(), KeyWrapperError> {
    if material.format_version != FORMAT_VERSION {
        return Err(KeyWrapperError::Failed);
    }
    validate_material(
        &material.catalog_id,
        material.key_generation,
        &material.catalog_key_hex,
    )
}

#[cfg(windows)]
fn protect(plaintext: &[u8]) -> Result<Vec<u8>, KeyWrapperError> {
    use std::ptr::null;
    use std::slice;

    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Cryptography::{CryptProtectData, CRYPT_INTEGER_BLOB};

    let mut input = CRYPT_INTEGER_BLOB {
        cbData: plaintext.len() as u32,
        pbData: plaintext.as_ptr() as *mut u8,
    };
    let mut output = CRYPT_INTEGER_BLOB {
        cbData: 0,
        pbData: std::ptr::null_mut(),
    };
    let ok =
        unsafe { CryptProtectData(&mut input, null(), null(), null(), null(), 0, &mut output) };
    if ok == 0 || output.pbData.is_null() || output.cbData == 0 {
        return Err(KeyWrapperError::Failed);
    }
    let result = unsafe { slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec() };
    unsafe {
        LocalFree(output.pbData as *mut std::ffi::c_void);
    }
    Ok(result)
}

#[cfg(windows)]
fn unprotect(encrypted: &[u8]) -> Result<Vec<u8>, KeyWrapperError> {
    use std::ptr::null_mut;
    use std::slice;

    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Cryptography::{CryptUnprotectData, CRYPT_INTEGER_BLOB};

    let mut input = CRYPT_INTEGER_BLOB {
        cbData: encrypted.len() as u32,
        pbData: encrypted.as_ptr() as *mut u8,
    };
    let mut output = CRYPT_INTEGER_BLOB {
        cbData: 0,
        pbData: std::ptr::null_mut(),
    };
    let ok = unsafe {
        CryptUnprotectData(
            &mut input,
            null_mut(),
            null_mut(),
            null_mut(),
            null_mut(),
            0,
            &mut output,
        )
    };
    if ok == 0 || output.pbData.is_null() || output.cbData == 0 {
        return Err(KeyWrapperError::Failed);
    }
    let result = unsafe { slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec() };
    unsafe {
        LocalFree(output.pbData as *mut std::ffi::c_void);
    }
    Ok(result)
}

/// 凭证库复用 DPAPI，但不复用目录库密钥材料格式。
pub(crate) fn protect_secret(plaintext: &[u8]) -> Result<Vec<u8>, KeyWrapperError> {
    #[cfg(windows)]
    {
        protect(plaintext)
    }
    #[cfg(not(windows))]
    {
        let _ = plaintext;
        Err(KeyWrapperError::Unavailable)
    }
}

/// 凭证库只在当前 Windows 用户上下文解包登录材料。
pub(crate) fn unprotect_secret(encrypted: &[u8]) -> Result<Vec<u8>, KeyWrapperError> {
    #[cfg(windows)]
    {
        unprotect(encrypted)
    }
    #[cfg(not(windows))]
    {
        let _ = encrypted;
        Err(KeyWrapperError::Unavailable)
    }
}

fn read_blob(path: &Path) -> Result<Vec<u8>, KeyWrapperError> {
    let metadata = fs::symlink_metadata(path).map_err(|_| KeyWrapperError::Unavailable)?;
    let max_file_bytes = (MAGIC.len() + 8 + MAX_BLOB_BYTES) as u64;
    if metadata.file_type().is_symlink() || !metadata.is_file() || metadata.len() > max_file_bytes {
        return Err(KeyWrapperError::Failed);
    }
    let file = File::open(path).map_err(|_| KeyWrapperError::Unavailable)?;
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.take(max_file_bytes.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|_| KeyWrapperError::Failed)?;
    // 文件可能在 metadata 检查后被替换；长度漂移时拒绝继续解析，避免无界增长。
    if bytes.len() as u64 != metadata.len() || bytes.len() as u64 > max_file_bytes {
        return Err(KeyWrapperError::Failed);
    }
    if bytes.len() < MAGIC.len() + 8 || &bytes[..MAGIC.len()] != MAGIC {
        return Err(KeyWrapperError::Failed);
    }
    let version_offset = MAGIC.len();
    let version = u32::from_le_bytes(
        bytes[version_offset..version_offset + 4]
            .try_into()
            .map_err(|_| KeyWrapperError::Failed)?,
    );
    if version != FORMAT_VERSION {
        return Err(KeyWrapperError::Failed);
    }
    let length_offset = version_offset + 4;
    let length = u32::from_le_bytes(
        bytes[length_offset..length_offset + 4]
            .try_into()
            .map_err(|_| KeyWrapperError::Failed)?,
    ) as usize;
    let start = length_offset + 4;
    if length == 0 || length > MAX_BLOB_BYTES || bytes.len() != start + length {
        return Err(KeyWrapperError::Failed);
    }
    Ok(bytes[start..].to_vec())
}

fn write_blob(path: &Path, encrypted: &[u8]) -> Result<(), KeyWrapperError> {
    if encrypted.is_empty() || encrypted.len() > MAX_BLOB_BYTES || is_symlink(path) {
        return Err(KeyWrapperError::Failed);
    }
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    reject_symlink_chain(parent)?;
    fs::create_dir_all(parent).map_err(|_| KeyWrapperError::Unavailable)?;
    reject_symlink_chain(parent)?;
    let temporary = parent.join(format!(
        ".{}.tmp-{}-{}",
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("key"),
        now_nanos(),
        std::process::id()
    ));
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .map_err(|_| KeyWrapperError::Unavailable)?;
        file.write_all(MAGIC).map_err(|_| KeyWrapperError::Failed)?;
        file.write_all(&FORMAT_VERSION.to_le_bytes())
            .map_err(|_| KeyWrapperError::Failed)?;
        file.write_all(&(encrypted.len() as u32).to_le_bytes())
            .map_err(|_| KeyWrapperError::Failed)?;
        file.write_all(encrypted)
            .map_err(|_| KeyWrapperError::Failed)?;
        file.sync_all().map_err(|_| KeyWrapperError::Failed)?;
        drop(file);
        publish_wrapper(&temporary, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

fn publish_wrapper(temporary: &Path, destination: &Path) -> Result<(), KeyWrapperError> {
    #[cfg(windows)]
    {
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
        let ok = unsafe {
            MoveFileExW(
                from.as_ptr(),
                to.as_ptr(),
                MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
            )
        };
        if ok == 0 {
            return Err(KeyWrapperError::Failed);
        }
        Ok(())
    }
    #[cfg(not(windows))]
    {
        fs::rename(temporary, destination).map_err(|_| KeyWrapperError::Failed)
    }
}

fn reject_symlink_chain(path: &Path) -> Result<(), KeyWrapperError> {
    let mut current = Some(path);
    while let Some(candidate) = current {
        match fs::symlink_metadata(candidate) {
            Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
                return Err(KeyWrapperError::Failed)
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return Err(KeyWrapperError::Failed),
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

fn is_symlink(path: &Path) -> bool {
    fs::symlink_metadata(path)
        .map(|metadata| metadata.file_type().is_symlink())
        .unwrap_or(false)
}

fn now_nanos() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0)
}

#[cfg(all(test, windows))]
mod tests {
    use std::fs;

    use tempfile::tempdir;
    use traesync_ports::{KeyWrapperPort, KeyWrapperRequest};

    use super::DpapiKeyWrapper;

    const CATALOG_KEY: &str = "deadbeefcafebabe1234567890abcdefdeadbeefcafebabe1234567890abcdef";

    #[test]
    fn dpapi_round_trip_does_not_write_plaintext_key() {
        let root = tempdir().expect("create isolated wrapper root");
        let wrapper = DpapiKeyWrapper::new(root.path().join("catalog-key.dpapi"));
        let request = KeyWrapperRequest::new("catalog-a", 7, CATALOG_KEY);

        let receipt = wrapper
            .wrap_catalog_key(&request)
            .expect("DPAPI wrapping should succeed");
        assert_eq!(receipt.wrapper_version, DpapiKeyWrapper::wrapper_version());

        let bytes = fs::read(wrapper.path()).expect("read wrapper bytes");
        assert!(!bytes
            .windows(CATALOG_KEY.len())
            .any(|window| window == CATALOG_KEY.as_bytes()));
        assert_eq!(
            wrapper
                .unwrap_catalog_key("catalog-a", 7)
                .expect("same profile should unwrap the key"),
            CATALOG_KEY
        );
    }

    #[test]
    fn dpapi_wrapper_rejects_wrong_catalog_identity_without_rewriting() {
        let root = tempdir().expect("create isolated wrapper root");
        let wrapper = DpapiKeyWrapper::new(root.path().join("catalog-key.dpapi"));
        let request = KeyWrapperRequest::new("catalog-a", 7, CATALOG_KEY);
        wrapper
            .wrap_catalog_key(&request)
            .expect("DPAPI wrapping should succeed");
        let before = fs::read(wrapper.path()).expect("snapshot wrapper bytes");

        assert!(wrapper.unwrap_catalog_key("catalog-other", 7).is_err());
        assert_eq!(
            fs::read(wrapper.path()).expect("read wrapper bytes"),
            before
        );
    }

    #[test]
    fn oversized_wrapper_blob_is_rejected_before_reading_payload() {
        let root = tempdir().expect("create isolated wrapper root");
        let path = root.path().join("oversized.dpapi");
        let file = std::fs::File::create(&path).expect("create oversized wrapper");
        file.set_len((super::MAGIC.len() + 8 + super::MAX_BLOB_BYTES + 1) as u64)
            .expect("create sparse oversized wrapper");

        assert!(super::read_blob(&path).is_err());
    }
}

#[cfg(test)]
mod format_tests {
    use super::{validate_protected_material, ProtectedCatalogKey, FORMAT_VERSION};

    fn material(format_version: u32) -> ProtectedCatalogKey {
        ProtectedCatalogKey {
            format_version,
            catalog_id: "catalog-a".to_string(),
            key_generation: 7,
            catalog_key_hex: "ab".repeat(32),
        }
    }

    #[test]
    fn protected_catalog_key_rejects_unknown_inner_format_version() {
        assert!(validate_protected_material(&material(FORMAT_VERSION + 1)).is_err());
    }

    #[test]
    fn protected_catalog_key_accepts_current_inner_format_version() {
        assert!(validate_protected_material(&material(FORMAT_VERSION)).is_ok());
    }
}
