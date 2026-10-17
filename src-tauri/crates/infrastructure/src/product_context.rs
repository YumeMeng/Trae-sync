//! P17-T5 并列骨架：**当前产品指针**（产品管理页/切换器的持久化状态）。
//!
//! 与 `product_account_state`（产品内当前账号指针）分层：本模块记录
//! 「用户当前操作哪个产品」（trae_cn / workbuddy），账号选择指针仍由
//! 各产品自己的 store 管理——产品切换与账号切换是不同维度（ADR-0029）。
//!
//! 纪律：文件锁 + 原子发布（tmp + rename）+ 格式版本校验 + 注册表校验
//! （product_id 必须在 product_descriptor::ALL_PRODUCTS 内，fail-closed）；
//! 未初始化时缺省 trae_cn（既有单产品用户的兼容语义）。

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::atomic_publish::publish_replacing;
use crate::operation_lease::FileOperationLock;
use crate::product_descriptor::descriptor_by_product_id;

const CURRENT_PRODUCT_FILE: &str = "current-product.json";
const MAX_STATE_BYTES: u64 = 4 * 1024;
/// 未初始化时的缺省产品（既有单产品用户兼容；协议值稳定）。
pub const DEFAULT_PRODUCT_ID: &str = "trae_cn";

/// 当前产品指针读写错误；不携带路径中的敏感内容。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CurrentProductError {
    /// 文件内容损坏或格式版本不支持。
    Invalid,
    /// product_id 未在产品注册表内（未适配产品不可选）。
    UnknownProduct,
    /// 文件系统读写失败。
    Io,
}

impl std::fmt::Display for CurrentProductError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let text = match self {
            Self::Invalid => "product_context_invalid",
            Self::UnknownProduct => "product_context_unknown_product",
            Self::Io => "product_context_io",
        };
        f.write_str(text)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CurrentProductFile {
    format_version: u32,
    product_id: String,
}

/// 当前产品指针存储（`{product-state 根}/current-product.json`）。
pub struct CurrentProductStore {
    path: PathBuf,
}

impl CurrentProductStore {
    pub fn new(product_state_root: impl Into<PathBuf>) -> Self {
        Self {
            path: product_state_root.into().join(CURRENT_PRODUCT_FILE),
        }
    }

    /// 读取当前产品；未初始化时返回缺省（trae_cn，兼容既有单产品用户）。
    pub fn load(&self) -> Result<String, CurrentProductError> {
        match self.read_file()? {
            Some(state) => Ok(state.product_id),
            None => Ok(DEFAULT_PRODUCT_ID.to_string()),
        }
    }

    /// 切换当前产品（注册表校验 fail-closed；文件锁 + 原子发布）。
    pub fn set(&self, product_id: &str) -> Result<(), CurrentProductError> {
        if descriptor_by_product_id(product_id).is_none() {
            return Err(CurrentProductError::UnknownProduct);
        }
        self.create_parent()?;
        let _lock = FileOperationLock::acquire_sibling_resource(&self.path)
            .map_err(|_| CurrentProductError::Io)?;
        self.publish(&CurrentProductFile {
            format_version: 1,
            product_id: product_id.to_string(),
        })
    }

    fn read_file(&self) -> Result<Option<CurrentProductFile>, CurrentProductError> {
        let metadata = match fs::metadata(&self.path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(_) => return Err(CurrentProductError::Io),
        };
        if !metadata.is_file() || metadata.len() > MAX_STATE_BYTES {
            return Err(CurrentProductError::Invalid);
        }
        let content = fs::read_to_string(&self.path).map_err(|_| CurrentProductError::Io)?;
        let state: CurrentProductFile =
            serde_json::from_str(&content).map_err(|_| CurrentProductError::Invalid)?;
        if state.format_version != 1 {
            return Err(CurrentProductError::Invalid);
        }
        // 读出的指针也必须过注册表校验（防手改文件绕过 fail-closed）。
        if descriptor_by_product_id(&state.product_id).is_none() {
            return Err(CurrentProductError::UnknownProduct);
        }
        Ok(Some(state))
    }

    fn create_parent(&self) -> Result<(), CurrentProductError> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent).map_err(|_| CurrentProductError::Io)?;
        }
        Ok(())
    }

    fn publish(&self, state: &CurrentProductFile) -> Result<(), CurrentProductError> {
        let content =
            serde_json::to_string_pretty(state).map_err(|_| CurrentProductError::Invalid)?;
        let temporary = self.path.with_extension("json.tmp");
        fs::write(&temporary, content.as_bytes()).map_err(|_| CurrentProductError::Io)?;
        publish_replacing(&temporary, &self.path).map_err(|_| CurrentProductError::Io)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn load_defaults_to_trae_cn_when_uninitialized() {
        let root = tempdir().unwrap();
        let store = CurrentProductStore::new(root.path());
        assert_eq!(store.load().unwrap(), DEFAULT_PRODUCT_ID);
        // 未初始化时 set 前不落任何文件。
        assert!(!root.path().join(CURRENT_PRODUCT_FILE).exists());
    }

    #[test]
    fn set_and_load_roundtrip_across_instances() {
        let root = tempdir().unwrap();
        CurrentProductStore::new(root.path())
            .set("workbuddy")
            .unwrap();
        assert_eq!(
            CurrentProductStore::new(root.path())
                .load()
                .unwrap(),
            "workbuddy"
        );
        // 切回。
        CurrentProductStore::new(root.path()).set("trae_cn").unwrap();
        assert_eq!(
            CurrentProductStore::new(root.path()).load().unwrap(),
            "trae_cn"
        );
    }

    /// 注册表外产品（含历史 work_cn）→ UnknownProduct，不落盘。
    #[test]
    fn set_rejects_unregistered_products() {
        let root = tempdir().unwrap();
        let store = CurrentProductStore::new(root.path());
        assert_eq!(
            store.set("work_cn").unwrap_err(),
            CurrentProductError::UnknownProduct
        );
        assert_eq!(
            store.set("").unwrap_err(),
            CurrentProductError::UnknownProduct
        );
        assert!(!root.path().join(CURRENT_PRODUCT_FILE).exists());
    }

    /// 手改文件指向未注册产品 → load 拒绝（fail-closed，不静默采用）。
    #[test]
    fn load_rejects_unregistered_pointer_file() {
        let root = tempdir().unwrap();
        let store = CurrentProductStore::new(root.path());
        store.set("trae_cn").unwrap();
        let path = root.path().join(CURRENT_PRODUCT_FILE);
        fs::write(
            &path,
            r#"{"format_version":1,"product_id":"work_cn"}"#,
        )
        .unwrap();
        assert_eq!(
            store.load().unwrap_err(),
            CurrentProductError::UnknownProduct
        );
        // 版本不符。
        fs::write(&path, r#"{"format_version":2,"product_id":"trae_cn"}"#).unwrap();
        assert_eq!(store.load().unwrap_err(), CurrentProductError::Invalid);
        // 损坏。
        fs::write(&path, "not json").unwrap();
        assert_eq!(store.load().unwrap_err(), CurrentProductError::Invalid);
    }
}
