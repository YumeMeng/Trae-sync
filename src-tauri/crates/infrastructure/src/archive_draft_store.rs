//! 归档草稿存储（P11-5）：库归档草稿从前端 localStorage 迁入工具管理数据。
//!
//! 文件布局：`<storage_root>/archive-drafts.json`，每库一键
//! （library_id → 草稿）。草稿是用户未提交的归档/恢复意图，丢失即伤用户，
//! 因此：写入走读-改-写 + 原子发布（失败保留原文件）；文件损坏 fail-closed
//! （读取报错，命令层降级为空草稿），不删除文件（铁律）。

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::atomic_publish::publish_replacing;

const STORE_FILE: &str = "archive-drafts.json";
/// 存储上限：与环境注册表同口径（256 KB）。
const MAX_STORE_BYTES: u64 = 256 * 1024;
/// 单条会话 id 上限（防垃圾数据膨胀）。
const MAX_SESSION_ID_BYTES: usize = 128;
/// 单库草稿的会话 id 数量上限（归档操作规模远小于此）。
const MAX_SESSION_IDS: usize = 10_000;
/// 库标识上限（"master" 或副库 env_id，远小于此）。
const MAX_LIBRARY_ID_BYTES: usize = 128;

/// 归档草稿存储错误；不携带敏感内容。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArchiveDraftStoreError {
    /// 文件损坏、超限或草稿记录非法。
    Invalid,
    /// 读写失败。
    Io,
}

/// 单库归档草稿（与会话归档命令入参同构；restore 不得与 archive 重叠）。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArchiveDraftRecord {
    #[serde(default)]
    pub archive_session_ids: Vec<String>,
    #[serde(default)]
    pub restore_session_ids: Vec<String>,
}

impl ArchiveDraftRecord {
    /// 是否空草稿（无归档也无恢复意图）。
    pub fn is_empty(&self) -> bool {
        self.archive_session_ids.is_empty() && self.restore_session_ids.is_empty()
    }

    /// 校验草稿字段：id 非空且不超长、数量不超限、restore 与 archive 不重叠。
    pub fn validate(&self) -> Result<(), ArchiveDraftStoreError> {
        validate_id_list(&self.archive_session_ids)?;
        validate_id_list(&self.restore_session_ids)?;
        // 同一会话不能同时处于归档与恢复意图（与前端草稿语义一致）。
        if self
            .restore_session_ids
            .iter()
            .any(|id| self.archive_session_ids.contains(id))
        {
            return Err(ArchiveDraftStoreError::Invalid);
        }
        Ok(())
    }
}

/// 校验会话 id 列表：非空字符串、长度与数量上限。
fn validate_id_list(ids: &[String]) -> Result<(), ArchiveDraftStoreError> {
    if ids.len() > MAX_SESSION_IDS {
        return Err(ArchiveDraftStoreError::Invalid);
    }
    for id in ids {
        if id.is_empty() || id.len() > MAX_SESSION_ID_BYTES {
            return Err(ArchiveDraftStoreError::Invalid);
        }
    }
    Ok(())
}

#[derive(Serialize, Deserialize)]
struct StoreFile {
    format_version: u32,
    #[serde(default)]
    drafts: BTreeMap<String, ArchiveDraftRecord>,
}

/// 归档草稿存储（storage_root 下单文件，每库一键）。
pub struct ArchiveDraftStore {
    path: PathBuf,
}

impl ArchiveDraftStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            path: root.into().join(STORE_FILE),
        }
    }

    /// 读取全部草稿；文件不存在返回空表（首次使用前无需预创建）。
    pub fn load(&self) -> Result<BTreeMap<String, ArchiveDraftRecord>, ArchiveDraftStoreError> {
        let metadata = match fs::symlink_metadata(&self.path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(BTreeMap::new());
            }
            Err(_) => return Err(ArchiveDraftStoreError::Io),
        };
        // 超限文件视为异常（防垃圾膨胀），fail-closed 拒绝读取。
        if !metadata.is_file() || metadata.len() > MAX_STORE_BYTES {
            return Err(ArchiveDraftStoreError::Invalid);
        }
        let content = fs::read_to_string(&self.path).map_err(|_| ArchiveDraftStoreError::Io)?;
        let file: StoreFile =
            serde_json::from_str(&content).map_err(|_| ArchiveDraftStoreError::Invalid)?;
        if file.format_version != 1 {
            return Err(ArchiveDraftStoreError::Invalid);
        }
        for (library_id, draft) in &file.drafts {
            if library_id.is_empty() || library_id.len() > MAX_LIBRARY_ID_BYTES {
                return Err(ArchiveDraftStoreError::Invalid);
            }
            draft.validate()?;
        }
        Ok(file.drafts)
    }

    /// 写入指定库的草稿；空草稿删除该库键。读-改-写 + 原子发布，
    /// 文件损坏时拒绝写入（不静默覆盖，也不删除文件）。
    pub fn set(
        &self,
        library_id: &str,
        draft: &ArchiveDraftRecord,
    ) -> Result<(), ArchiveDraftStoreError> {
        if library_id.is_empty() || library_id.len() > MAX_LIBRARY_ID_BYTES {
            return Err(ArchiveDraftStoreError::Invalid);
        }
        draft.validate()?;
        let mut drafts = self.load()?;
        if draft.is_empty() {
            // 空草稿即清除该库键（应用归档后草稿清空的路径）。
            drafts.remove(library_id);
        } else {
            drafts.insert(library_id.to_string(), draft.clone());
        }
        let file = StoreFile {
            format_version: 1,
            drafts,
        };
        let content =
            serde_json::to_string_pretty(&file).map_err(|_| ArchiveDraftStoreError::Invalid)?;
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent).map_err(|_| ArchiveDraftStoreError::Io)?;
        }
        let temporary = self.path.with_extension("json.tmp");
        fs::write(&temporary, content.as_bytes()).map_err(|_| ArchiveDraftStoreError::Io)?;
        publish_replacing(&temporary, &self.path).map_err(|_| ArchiveDraftStoreError::Io)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store(root: &Path) -> ArchiveDraftStore {
        ArchiveDraftStore::new(root)
    }

    fn draft(archive: &[&str], restore: &[&str]) -> ArchiveDraftRecord {
        ArchiveDraftRecord {
            archive_session_ids: archive.iter().map(|id| id.to_string()).collect(),
            restore_session_ids: restore.iter().map(|id| id.to_string()).collect(),
        }
    }

    #[test]
    fn load_missing_file_returns_empty_map() {
        let root = tempfile::tempdir().unwrap();
        assert!(store(root.path()).load().unwrap().is_empty());
    }

    #[test]
    fn set_and_reload_roundtrip() {
        let root = tempfile::tempdir().unwrap();
        let s = store(root.path());
        s.set("master", &draft(&["s1", "s2"], &["s3"])).unwrap();
        // 跨实例可恢复（App 重启后草稿不丢）。
        let loaded = store(root.path()).load().unwrap();
        assert_eq!(loaded.get("master"), Some(&draft(&["s1", "s2"], &["s3"])));
    }

    #[test]
    fn libraries_are_isolated_and_empty_draft_removes_key() {
        let root = tempfile::tempdir().unwrap();
        let s = store(root.path());
        s.set("master", &draft(&["s1"], &[])).unwrap();
        s.set("env-1-a", &draft(&[], &["s9"])).unwrap();
        let loaded = s.load().unwrap();
        assert_eq!(loaded.get("master"), Some(&draft(&["s1"], &[])));
        assert_eq!(loaded.get("env-1-a"), Some(&draft(&[], &["s9"])));

        // 覆盖写只影响目标库。
        s.set("master", &draft(&["s2"], &["s3"])).unwrap();
        let loaded = s.load().unwrap();
        assert_eq!(loaded.get("master"), Some(&draft(&["s2"], &["s3"])));
        assert_eq!(loaded.get("env-1-a"), Some(&draft(&[], &["s9"])));

        // 空草稿删除该库键（应用归档后草稿清空的路径）。
        s.set("master", &ArchiveDraftRecord::default()).unwrap();
        let loaded = s.load().unwrap();
        assert!(!loaded.contains_key("master"));
        assert_eq!(loaded.get("env-1-a"), Some(&draft(&[], &["s9"])));
    }

    #[test]
    fn corrupted_file_fails_closed_and_is_kept() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join(STORE_FILE);
        std::fs::write(&path, "{ not json").unwrap();
        // 损坏 fail-closed：读取报错（命令层降级为空草稿），文件保留（铁律）。
        assert_eq!(store(root.path()).load().unwrap_err(), ArchiveDraftStoreError::Invalid);
        assert!(path.exists());
        // 写入同样拒绝：不覆盖损坏文件，避免吞掉其他库的草稿证据。
        assert_eq!(
            store(root.path()).set("master", &draft(&["s1"], &[])).unwrap_err(),
            ArchiveDraftStoreError::Invalid
        );
        assert!(path.exists());
    }

    #[test]
    fn oversize_file_rejected() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join(STORE_FILE);
        // 超过 256 KB 视为异常文件（防垃圾膨胀），fail-closed。
        std::fs::write(&path, "x".repeat((MAX_STORE_BYTES + 1) as usize)).unwrap();
        assert_eq!(store(root.path()).load().unwrap_err(), ArchiveDraftStoreError::Invalid);
    }

    #[test]
    fn set_rejects_invalid_library_id_or_draft() {
        let root = tempfile::tempdir().unwrap();
        let s = store(root.path());
        // 库标识非法：空 / 超长。
        assert_eq!(
            s.set("", &draft(&["s1"], &[])).unwrap_err(),
            ArchiveDraftStoreError::Invalid
        );
        assert_eq!(
            s.set(&"l".repeat(MAX_LIBRARY_ID_BYTES + 1), &draft(&["s1"], &[])).unwrap_err(),
            ArchiveDraftStoreError::Invalid
        );
        // 草稿非法：空 id / 超长 id / restore 与 archive 重叠。
        assert_eq!(
            s.set("master", &draft(&[""], &[])).unwrap_err(),
            ArchiveDraftStoreError::Invalid
        );
        assert_eq!(
            s.set("master", &draft(&[&"s".repeat(MAX_SESSION_ID_BYTES + 1)], &[])).unwrap_err(),
            ArchiveDraftStoreError::Invalid
        );
        assert_eq!(
            s.set("master", &draft(&["s1"], &["s1"])).unwrap_err(),
            ArchiveDraftStoreError::Invalid
        );
    }

    #[test]
    fn set_rejects_when_session_ids_exceed_limit() {
        let root = tempfile::tempdir().unwrap();
        let s = store(root.path());
        let ids: Vec<String> = (0..=MAX_SESSION_IDS).map(|i| format!("s{i}")).collect();
        let oversized = ArchiveDraftRecord {
            archive_session_ids: ids,
            restore_session_ids: Vec::new(),
        };
        assert_eq!(
            s.set("master", &oversized).unwrap_err(),
            ArchiveDraftStoreError::Invalid
        );
    }
}
