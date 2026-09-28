//! 多产品账号的当前选择状态。
//!
//! 当前账号指针不是认证材料：它只记录用户在某个产品账号池中最后选择的
//! profile_id。认证材料仍由各产品自己的 `CheckinCredentialStore` 加密保存。

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::atomic_publish::publish_replacing;

const STATE_FILE: &str = "current-account.json";
const MAX_STATE_BYTES: u64 = 8 * 1024;
const MAX_PROFILE_ID_BYTES: usize = 256;

/// 当前产品账号选择状态的读写错误；不携带路径中的敏感内容。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProductAccountStateError {
    /// 文件内容损坏或格式版本不支持。
    Invalid,
    /// profile_id 为空或超长。
    InvalidProfileId,
    /// 文件系统读写失败。
    Io,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CurrentAccountFile {
    format_version: u32,
    profile_id: String,
}

/// 某个产品账号池的当前账号指针。
pub struct ProductAccountStateStore {
    path: PathBuf,
}

impl ProductAccountStateStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            path: root.into().join(STATE_FILE),
        }
    }

    /// 读取当前选择；首次使用或尚未选择时返回 `None`。
    pub fn load(&self) -> Result<Option<String>, ProductAccountStateError> {
        let metadata = match fs::symlink_metadata(&self.path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(_) => return Err(ProductAccountStateError::Io),
        };
        if !metadata.is_file() || metadata.len() > MAX_STATE_BYTES {
            return Err(ProductAccountStateError::Invalid);
        }
        let content = fs::read_to_string(&self.path).map_err(|_| ProductAccountStateError::Io)?;
        let state: CurrentAccountFile =
            serde_json::from_str(&content).map_err(|_| ProductAccountStateError::Invalid)?;
        if state.format_version != 1 {
            return Err(ProductAccountStateError::Invalid);
        }
        validate_profile_id(&state.profile_id)?;
        Ok(Some(state.profile_id))
    }

    /// 原子保存当前账号指针；不会触碰产品官方目录或其他产品账号池。
    pub fn save(&self, profile_id: &str) -> Result<(), ProductAccountStateError> {
        validate_profile_id(profile_id)?;
        let state = CurrentAccountFile {
            format_version: 1,
            profile_id: profile_id.to_string(),
        };
        let content =
            serde_json::to_vec_pretty(&state).map_err(|_| ProductAccountStateError::Invalid)?;
        if content.len() as u64 > MAX_STATE_BYTES {
            return Err(ProductAccountStateError::Invalid);
        }
        let Some(parent) = self.path.parent() else {
            return Err(ProductAccountStateError::Io);
        };
        fs::create_dir_all(parent).map_err(|_| ProductAccountStateError::Io)?;
        let temporary = self.path.with_extension("json.tmp");
        fs::write(&temporary, content).map_err(|_| ProductAccountStateError::Io)?;
        publish_replacing(&temporary, &self.path).map_err(|_| ProductAccountStateError::Io)
    }

    /// 测试与路径边界检查使用的实际状态文件路径。
    pub fn path(&self) -> &Path {
        &self.path
    }
}

fn validate_profile_id(profile_id: &str) -> Result<(), ProductAccountStateError> {
    if profile_id.is_empty() || profile_id.len() > MAX_PROFILE_ID_BYTES {
        return Err(ProductAccountStateError::InvalidProfileId);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn missing_state_is_empty() {
        let root = tempdir().unwrap();
        let store = ProductAccountStateStore::new(root.path());
        assert_eq!(store.load().unwrap(), None);
    }

    #[test]
    fn save_and_load_roundtrip() {
        let root = tempdir().unwrap();
        let store = ProductAccountStateStore::new(root.path());
        store.save("profile-cn-1").unwrap();
        assert_eq!(store.load().unwrap(), Some("profile-cn-1".to_string()));
        assert!(store.path().is_file());
    }

    #[test]
    fn invalid_state_fails_closed() {
        let root = tempdir().unwrap();
        let store = ProductAccountStateStore::new(root.path());
        fs::create_dir_all(root.path()).unwrap();
        fs::write(store.path(), b"not-json").unwrap();
        assert_eq!(store.load(), Err(ProductAccountStateError::Invalid));
    }
}
