//! 多产品账号的当前选择状态。
//!
//! 当前账号指针不是认证材料：它只记录用户在某个产品账号池中最后选择的
//! profile_id。认证材料仍由各产品自己的 `CheckinCredentialStore` 加密保存。

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::atomic_publish::publish_replacing;
use crate::operation_lease::FileOperationLock;

const STATE_FILE: &str = "current-account.json";
const MAX_STATE_BYTES: u64 = 8 * 1024;
const MAX_PROFILE_ID_BYTES: usize = 256;

/// 产品选择指针目录 = `{storage_root}/product-state`（P15-7，ADR-0035）。
/// 旧 CN 产品池目录（`products/trae_cn`）随 P15-7 数据删除，工具选择指针
/// 迁到独立目录，避免指针写入复活已删除目录；纯函数供组合根与库作用域
/// 账号解析共用，不落盘、无副作用。
pub fn product_state_root_from(storage_root: &Path) -> PathBuf {
    // P16-T2：路径拼接收口到 storage_roots 唯一来源（本函数保留 API 供既有调用方）。
    crate::storage_roots::product_state_root_from(storage_root)
}

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
    profile_id: Option<String>,
    /// 选择意图代次：只允许最后开始的账号选择操作发布当前指针。
    #[serde(default)]
    selection_generation: u64,
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
        Ok(self.read_file()?.and_then(|state| state.profile_id))
    }

    /// 开始一次新的账号选择意图，返回提交时必须携带的代次。
    /// OAuth begin 与用户主动切号都调用此方法；较早请求的迟到结果会被拒绝。
    pub fn begin_selection_intent(&self) -> Result<u64, ProductAccountStateError> {
        self.create_parent()?;
        let _lock = FileOperationLock::acquire_sibling_resource(&self.path)
            .map_err(|_| ProductAccountStateError::Io)?;
        let current = self.read_file()?;
        let selection_generation = current
            .as_ref()
            .map(|state| state.selection_generation)
            .unwrap_or(0)
            .checked_add(1)
            .ok_or(ProductAccountStateError::Invalid)?;
        let state = CurrentAccountFile {
            format_version: 1,
            profile_id: current.and_then(|state| state.profile_id),
            selection_generation,
        };
        self.publish_locked(&state)?;
        Ok(selection_generation)
    }

    /// 仅当 `selection_generation` 仍是最新代次时发布账号指针。
    /// 返回 false 表示期间有更新的账号选择意图，调用方应保留其选择。
    pub fn commit_selection(
        &self,
        profile_id: &str,
        selection_generation: u64,
    ) -> Result<bool, ProductAccountStateError> {
        validate_profile_id(profile_id)?;
        self.create_parent()?;
        let _lock = FileOperationLock::acquire_sibling_resource(&self.path)
            .map_err(|_| ProductAccountStateError::Io)?;
        let Some(mut state) = self.read_file()? else {
            return Ok(false);
        };
        if state.selection_generation != selection_generation {
            return Ok(false);
        }
        state.profile_id = Some(profile_id.to_string());
        self.publish_locked(&state)?;
        Ok(true)
    }

    /// 原子保存显式选择；供 fixture 与已有单步调用复用。
    /// 生产异步流程应分开调用 begin_selection_intent/commit_selection。
    pub fn save(&self, profile_id: &str) -> Result<(), ProductAccountStateError> {
        validate_profile_id(profile_id)?;
        self.create_parent()?;
        let _lock = FileOperationLock::acquire_sibling_resource(&self.path)
            .map_err(|_| ProductAccountStateError::Io)?;
        let selection_generation = self
            .read_file()?
            .map(|state| state.selection_generation)
            .unwrap_or(0)
            .checked_add(1)
            .ok_or(ProductAccountStateError::Invalid)?;
        self.publish_locked(&CurrentAccountFile {
            format_version: 1,
            profile_id: Some(profile_id.to_string()),
            selection_generation,
        })
    }

    fn read_file(&self) -> Result<Option<CurrentAccountFile>, ProductAccountStateError> {
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
        if let Some(profile_id) = state.profile_id.as_deref() {
            validate_profile_id(profile_id)?;
        }
        Ok(Some(state))
    }

    fn create_parent(&self) -> Result<(), ProductAccountStateError> {
        let Some(parent) = self.path.parent() else {
            return Err(ProductAccountStateError::Io);
        };
        fs::create_dir_all(parent).map_err(|_| ProductAccountStateError::Io)
    }

    fn publish_locked(&self, state: &CurrentAccountFile) -> Result<(), ProductAccountStateError> {
        let content =
            serde_json::to_vec_pretty(&state).map_err(|_| ProductAccountStateError::Invalid)?;
        if content.len() as u64 > MAX_STATE_BYTES {
            return Err(ProductAccountStateError::Invalid);
        }
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
    fn later_selection_intent_wins_when_earlier_login_finishes_last() {
        let root = tempdir().unwrap();
        let store = ProductAccountStateStore::new(root.path());

        let login_intent = store.begin_selection_intent().unwrap();
        let switch_intent = store.begin_selection_intent().unwrap();
        assert!(store
            .commit_selection("profile-manual", switch_intent)
            .unwrap());
        assert!(!store
            .commit_selection("profile-oauth", login_intent)
            .unwrap());
        assert_eq!(store.load().unwrap(), Some("profile-manual".to_string()));

        let earlier_switch = store.begin_selection_intent().unwrap();
        let later_login = store.begin_selection_intent().unwrap();
        assert!(!store
            .commit_selection("profile-old", earlier_switch)
            .unwrap());
        assert!(store
            .commit_selection("profile-new-login", later_login)
            .unwrap());
        assert_eq!(store.load().unwrap(), Some("profile-new-login".to_string()));
    }

    #[test]
    fn legacy_current_account_file_defaults_to_the_first_selection_generation() {
        let root = tempdir().unwrap();
        let store = ProductAccountStateStore::new(root.path());
        fs::write(
            store.path(),
            r#"{"format_version":1,"profile_id":"profile-legacy"}"#,
        )
        .unwrap();

        let intent = store.begin_selection_intent().unwrap();
        assert_eq!(intent, 1);
        assert_eq!(store.load().unwrap(), Some("profile-legacy".to_string()));
        assert!(store.commit_selection("profile-next", intent).unwrap());
        assert_eq!(store.load().unwrap(), Some("profile-next".to_string()));
    }

    #[test]
    fn save_waits_for_the_current_account_pointer_lock() {
        let root = tempdir().unwrap();
        let store = ProductAccountStateStore::new(root.path());
        store.save("profile-before").unwrap();
        let held_lock = FileOperationLock::acquire_sibling_resource(store.path()).unwrap();
        let writer_store = ProductAccountStateStore::new(root.path());
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let writer = std::thread::spawn(move || {
            let result = writer_store.save("profile-after");
            done_tx.send(result).unwrap();
        });

        assert!(done_rx
            .recv_timeout(std::time::Duration::from_millis(100))
            .is_err());
        drop(held_lock);
        done_rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap()
            .unwrap();
        writer.join().unwrap();
        assert_eq!(store.load().unwrap(), Some("profile-after".to_string()));
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
