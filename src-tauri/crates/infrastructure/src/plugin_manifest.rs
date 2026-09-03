//! 环境插件清单（ADR-0023，P5-8b）：环境持插件绑定基线。
//!
//! 清单角色 = 同步基线 + 恢复依据，**不是审批闸门**：切号对账时先吸收
//! 当前账号云端的增删差异（用户在 TRAE 的手动操作视为真实意图），再弹层
//! 确认应用到目标账号。本模块只管清单存取（storage_root 单 JSON 文件，
//! 与环境注册表同存），不碰网络；云端读写见 `plugin_cloud_sync`。
//!
//! 只收市场插件（有 `marketplace_plugin_id` 且非 `builtin:` 前缀）：
//! builtin 条目是客户端内置、云端无记录（实测 DELETE 404），无法跨账号
//! 同步，入清单只会制造永久差异。首次启用由命令层从当前账号云端导入。

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::atomic_publish::publish_replacing;

const MANIFEST_FILE: &str = "plugin-manifest.json";
const MAX_MANIFEST_BYTES: u64 = 256 * 1024;
const MAX_TEXT_FIELD_BYTES: usize = 1024;
/// 条目上限：单页覆盖个人账号全部已装插件（实测最大 5），500 是宽松护栏。
const MAX_ENTRIES: usize = 500;

/// 清单错误；不携带敏感内容。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PluginManifestError {
    /// 文件损坏或格式不符。
    Invalid,
    /// 条目字段非法（空/超长/重复键）。
    InvalidEntry,
    /// 读写失败。
    Io,
}

/// 清单条目：以市场 UUID 为主键（跨账号同步与安装的稳定键）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginManifestEntry {
    pub marketplace_plugin_id: String,
    pub name: String,
    pub display_name: String,
    pub version: String,
    pub registry: String,
    pub added_unix_seconds: u64,
}

impl PluginManifestEntry {
    /// 校验字段：市场 ID/名称非空且不超长；时间戳非零。
    pub fn validate(&self) -> Result<(), PluginManifestError> {
        for field in [
            &self.marketplace_plugin_id,
            &self.name,
            &self.display_name,
            &self.version,
            &self.registry,
        ] {
            if field.is_empty() || field.len() > MAX_TEXT_FIELD_BYTES {
                return Err(PluginManifestError::InvalidEntry);
            }
        }
        if self.added_unix_seconds == 0 {
            return Err(PluginManifestError::InvalidEntry);
        }
        Ok(())
    }
}

/// 校验整份清单：逐条校验 + 主键不重复。
fn validate_entries(entries: &[PluginManifestEntry]) -> Result<(), PluginManifestError> {
    if entries.len() > MAX_ENTRIES {
        return Err(PluginManifestError::InvalidEntry);
    }
    let mut seen = std::collections::HashSet::new();
    for entry in entries {
        entry.validate()?;
        if !seen.insert(entry.marketplace_plugin_id.as_str()) {
            return Err(PluginManifestError::InvalidEntry);
        }
    }
    Ok(())
}

#[derive(Serialize, Deserialize)]
struct ManifestFile {
    format_version: u32,
    plugins: Vec<PluginManifestEntry>,
}

/// 环境插件清单（storage_root 下单 JSON 文件）。
pub struct PluginManifest {
    path: PathBuf,
}

impl PluginManifest {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            path: root.into().join(MANIFEST_FILE),
        }
    }

    /// 读取清单；文件不存在返回 None（首次启用前，由命令层触发云端导入）。
    pub fn load(&self) -> Result<Option<Vec<PluginManifestEntry>>, PluginManifestError> {
        let metadata = match fs::symlink_metadata(&self.path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(_) => return Err(PluginManifestError::Io),
        };
        if !metadata.is_file() || metadata.len() > MAX_MANIFEST_BYTES {
            return Err(PluginManifestError::Invalid);
        }
        let content = fs::read_to_string(&self.path).map_err(|_| PluginManifestError::Io)?;
        let manifest: ManifestFile =
            serde_json::from_str(&content).map_err(|_| PluginManifestError::Invalid)?;
        if manifest.format_version != 1 {
            return Err(PluginManifestError::Invalid);
        }
        validate_entries(&manifest.plugins)?;
        Ok(Some(manifest.plugins))
    }

    /// 原子写入：失败时保留原文件。
    /// 文件已存在但损坏时拒绝写入（报错而非静默重建——档案纪律）。
    pub fn save(&self, entries: &[PluginManifestEntry]) -> Result<(), PluginManifestError> {
        if self.path.exists() {
            // load 返回 Ok(None) 不可能（文件已存在）；Err = 损坏，拒绝覆盖。
            if self.load()?.is_none() {
                return Err(PluginManifestError::Invalid);
            }
        }
        validate_entries(entries)?;
        let manifest = ManifestFile {
            format_version: 1,
            plugins: entries.to_vec(),
        };
        let content = serde_json::to_string_pretty(&manifest)
            .map_err(|_| PluginManifestError::Invalid)?;
        let bytes = content.as_bytes();
        if bytes.len() as u64 > MAX_MANIFEST_BYTES {
            return Err(PluginManifestError::Invalid);
        }
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent).map_err(|_| PluginManifestError::Io)?;
        }
        let temporary = self.path.with_extension("json.tmp");
        fs::write(&temporary, bytes).map_err(|_| PluginManifestError::Io)?;
        publish_replacing(&temporary, &self.path).map_err(|_| PluginManifestError::Io)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn entry(id: &str, added: u64) -> PluginManifestEntry {
        PluginManifestEntry {
            marketplace_plugin_id: id.to_string(),
            name: format!("plugin:{id}"),
            display_name: id.to_string(),
            version: "1.0.0".to_string(),
            registry: "trae-remote-official".to_string(),
            added_unix_seconds: added,
        }
    }

    #[test]
    fn load_missing_file_returns_none() {
        let root = tempdir().unwrap();
        let manifest = PluginManifest::new(root.path());
        assert_eq!(manifest.load().unwrap(), None);
    }

    #[test]
    fn save_then_load_roundtrip() {
        let root = tempdir().unwrap();
        let manifest = PluginManifest::new(root.path());
        let entries = vec![entry("uuid-a", 100), entry("uuid-b", 200)];
        manifest.save(&entries).unwrap();
        // 跨实例恢复（App 重启后清单不丢）。
        assert_eq!(manifest.load().unwrap(), Some(entries));
    }

    #[test]
    fn empty_manifest_persists_and_distinct_from_missing() {
        let root = tempdir().unwrap();
        let manifest = PluginManifest::new(root.path());
        manifest.save(&[]).unwrap();
        // 空清单是「已启用且无插件」，与 None（未启用）语义不同。
        assert_eq!(manifest.load().unwrap(), Some(vec![]));
    }

    #[test]
    fn duplicate_keys_rejected() {
        let root = tempdir().unwrap();
        let manifest = PluginManifest::new(root.path());
        let entries = vec![entry("uuid-a", 100), entry("uuid-a", 200)];
        assert_eq!(manifest.save(&entries), Err(PluginManifestError::InvalidEntry));
    }

    #[test]
    fn invalid_file_rejected_not_rebuilt() {
        let root = tempdir().unwrap();
        let manifest = PluginManifest::new(root.path());
        let path = root.path().join(MANIFEST_FILE);
        // 坏 JSON。
        std::fs::write(&path, "not json").unwrap();
        assert_eq!(manifest.load().unwrap_err(), PluginManifestError::Invalid);
        // 损坏文件不被静默覆盖：写回操作报错而非重建。
        assert_eq!(
            manifest.save(&[entry("uuid-a", 100)]),
            Err(PluginManifestError::Invalid)
        );
        // 版本不符（V2 格式倒灌防护）。
        std::fs::write(
            &path,
            r#"{"format_version":2,"plugins":[]}"#,
        )
        .unwrap();
        assert_eq!(manifest.load().unwrap_err(), PluginManifestError::Invalid);
    }

    #[test]
    fn blank_fields_rejected() {
        assert_eq!(entry("", 100).validate(), Err(PluginManifestError::InvalidEntry));
        assert_eq!(entry("uuid", 0).validate(), Err(PluginManifestError::InvalidEntry));
    }
}
