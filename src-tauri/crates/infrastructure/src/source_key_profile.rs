//! TRAE source raw key 的本机版本档案。
//!
//! raw key 只保存为当前 Windows 用户可解密的 DPAPI 密文；公开状态仅包含
//! 版本、兼容摘要和激活状态。候选登记不替换运行中 active key，下一次启动才激活。

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::atomic_publish::publish_replacing;
use crate::key_wrapper::{protect_secret, unprotect_secret};

pub const BASELINE_SOURCE_KEY_ID: &str = "work-cn-baseline-v1";
const FORMAT_VERSION: u32 = 1;
const KEY_MAGIC: &[u8; 8] = b"TRSKEY01";
const MAX_INDEX_BYTES: u64 = 256 * 1024;
const MAX_KEY_BLOB_BYTES: u64 = 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceKeyProfileState {
    Active,
    Pending,
    Retired,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceKeyProfileMetadata {
    pub key_id: String,
    pub product_version: String,
    pub cipher_profile: String,
    pub schema_fingerprint: String,
    pub mapping_version: String,
    pub state: SourceKeyProfileState,
    pub verified_at_unix_seconds: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceKeyActivation {
    pub raw_key: String,
    pub active_profile: SourceKeyProfileMetadata,
    pub activation_changed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SourceKeyProfileStatus {
    pub active: SourceKeyProfileMetadata,
    pub pending: Option<SourceKeyProfileMetadata>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourceKeyProfileError {
    Unavailable,
    Invalid,
}

impl std::fmt::Display for SourceKeyProfileError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Unavailable => "source_key_profile_unavailable",
            Self::Invalid => "source_key_profile_invalid",
        })
    }
}

impl std::error::Error for SourceKeyProfileError {}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SourceKeyIndex {
    format_version: u32,
    active_key_id: String,
    pending_key_id: Option<String>,
    profiles: Vec<SourceKeyProfileMetadata>,
}

#[derive(Debug, Clone)]
pub struct SourceKeyProfileStore {
    root: PathBuf,
}

impl SourceKeyProfileStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn status(&self) -> Result<SourceKeyProfileStatus, SourceKeyProfileError> {
        let index = self.load_or_baseline()?;
        status_from_index(&index)
    }

    /// 登记已经由调用方完成只读数据库验证的候选 key。
    pub fn register_verified_candidate(
        &self,
        raw_key: &str,
        product_version: &str,
        schema_fingerprint: &str,
        mapping_version: &str,
    ) -> Result<SourceKeyProfileStatus, SourceKeyProfileError> {
        validate_raw_key(raw_key)?;
        if product_version.trim().is_empty()
            || schema_fingerprint.trim().is_empty()
            || mapping_version.trim().is_empty()
        {
            return Err(SourceKeyProfileError::Invalid);
        }
        let mut index = self.load_or_baseline()?;
        let key_id = candidate_key_id(raw_key);
        if key_id == index.active_key_id {
            return status_from_index(&index);
        }

        write_protected_key(&self.key_path(&key_id), raw_key)?;
        for profile in &mut index.profiles {
            if profile.state == SourceKeyProfileState::Pending {
                profile.state = SourceKeyProfileState::Retired;
            }
        }
        let candidate = SourceKeyProfileMetadata {
            key_id: key_id.clone(),
            product_version: product_version.trim().to_string(),
            cipher_profile: "sqlcipher-4-defaults".to_string(),
            schema_fingerprint: schema_fingerprint.trim().to_string(),
            mapping_version: mapping_version.trim().to_string(),
            state: SourceKeyProfileState::Pending,
            verified_at_unix_seconds: now_unix_seconds(),
        };
        if let Some(existing) = index
            .profiles
            .iter_mut()
            .find(|profile| profile.key_id == key_id)
        {
            *existing = candidate;
        } else {
            index.profiles.push(candidate);
        }
        index.pending_key_id = Some(key_id);
        self.publish_index(&index)?;
        status_from_index(&index)
    }

    /// 启动阶段激活 pending；运行中不得调用。
    pub fn activate_pending(
        &self,
        baseline_raw_key: &str,
    ) -> Result<SourceKeyActivation, SourceKeyProfileError> {
        validate_raw_key(baseline_raw_key)?;
        let mut index = self.load_or_baseline()?;
        let Some(pending_key_id) = index.pending_key_id.clone() else {
            let raw_key = self.load_key(&index.active_key_id, baseline_raw_key)?;
            let active_profile = active_profile(&index)?.clone();
            return Ok(SourceKeyActivation {
                raw_key,
                active_profile,
                activation_changed: false,
            });
        };

        // 先完成 DPAPI 解密与格式校验，再发布 active 指针，避免损坏候选取代旧 key。
        let raw_key = self.load_key(&pending_key_id, baseline_raw_key)?;
        for profile in &mut index.profiles {
            if profile.key_id == index.active_key_id {
                profile.state = SourceKeyProfileState::Retired;
            } else if profile.key_id == pending_key_id {
                profile.state = SourceKeyProfileState::Active;
            }
        }
        index.active_key_id = pending_key_id;
        index.pending_key_id = None;
        self.publish_index(&index)?;
        let active_profile = active_profile(&index)?.clone();
        Ok(SourceKeyActivation {
            raw_key,
            active_profile,
            activation_changed: true,
        })
    }

    fn load_key(
        &self,
        key_id: &str,
        baseline_raw_key: &str,
    ) -> Result<String, SourceKeyProfileError> {
        if key_id == BASELINE_SOURCE_KEY_ID {
            return Ok(baseline_raw_key.to_string());
        }
        read_protected_key(&self.key_path(key_id))
    }

    fn load_or_baseline(&self) -> Result<SourceKeyIndex, SourceKeyProfileError> {
        let path = self.root.join("index.json");
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(baseline_index())
            }
            Err(_) => return Err(SourceKeyProfileError::Unavailable),
        };
        if is_reparse_or_symlink(&metadata)
            || !metadata.is_file()
            || metadata.len() > MAX_INDEX_BYTES
        {
            return Err(SourceKeyProfileError::Invalid);
        }
        let bytes = read_exact_file(&path, metadata.len(), MAX_INDEX_BYTES)?;
        let index: SourceKeyIndex =
            serde_json::from_slice(&bytes).map_err(|_| SourceKeyProfileError::Invalid)?;
        validate_index(&index)?;
        Ok(index)
    }

    fn publish_index(&self, index: &SourceKeyIndex) -> Result<(), SourceKeyProfileError> {
        validate_index(index)?;
        let bytes = serde_json::to_vec_pretty(index).map_err(|_| SourceKeyProfileError::Invalid)?;
        write_atomically(&self.root.join("index.json"), &bytes)
    }

    fn key_path(&self, key_id: &str) -> PathBuf {
        let name = format!("{}.dpapi", hex::encode(Sha256::digest(key_id.as_bytes())));
        self.root.join("keys").join(name)
    }
}

fn baseline_index() -> SourceKeyIndex {
    SourceKeyIndex {
        format_version: FORMAT_VERSION,
        active_key_id: BASELINE_SOURCE_KEY_ID.to_string(),
        pending_key_id: None,
        profiles: vec![SourceKeyProfileMetadata {
            key_id: BASELINE_SOURCE_KEY_ID.to_string(),
            product_version: "TRAE Work CN baseline".to_string(),
            cipher_profile: "sqlcipher-4-defaults".to_string(),
            schema_fingerprint: "verified-at-runtime".to_string(),
            mapping_version: "work_cn_v1".to_string(),
            state: SourceKeyProfileState::Active,
            verified_at_unix_seconds: 0,
        }],
    }
}

fn validate_index(index: &SourceKeyIndex) -> Result<(), SourceKeyProfileError> {
    if index.format_version != FORMAT_VERSION || index.profiles.is_empty() {
        return Err(SourceKeyProfileError::Invalid);
    }
    let active_count = index
        .profiles
        .iter()
        .filter(|profile| profile.state == SourceKeyProfileState::Active)
        .count();
    if active_count != 1
        || !index.profiles.iter().any(|profile| {
            profile.key_id == index.active_key_id && profile.state == SourceKeyProfileState::Active
        })
    {
        return Err(SourceKeyProfileError::Invalid);
    }
    match index.pending_key_id.as_deref() {
        Some(pending)
            if index.profiles.iter().any(|profile| {
                profile.key_id == pending && profile.state == SourceKeyProfileState::Pending
            }) => {}
        Some(_) => return Err(SourceKeyProfileError::Invalid),
        None if index
            .profiles
            .iter()
            .any(|profile| profile.state == SourceKeyProfileState::Pending) =>
        {
            return Err(SourceKeyProfileError::Invalid)
        }
        None => {}
    }
    if index.profiles.iter().any(|profile| {
        profile.key_id.is_empty()
            || profile.product_version.is_empty()
            || profile.cipher_profile.is_empty()
            || profile.schema_fingerprint.is_empty()
            || profile.mapping_version.is_empty()
    }) {
        return Err(SourceKeyProfileError::Invalid);
    }
    Ok(())
}

fn status_from_index(
    index: &SourceKeyIndex,
) -> Result<SourceKeyProfileStatus, SourceKeyProfileError> {
    let active = active_profile(index)?.clone();
    let pending = index
        .pending_key_id
        .as_ref()
        .and_then(|key_id| {
            index
                .profiles
                .iter()
                .find(|profile| &profile.key_id == key_id)
        })
        .cloned();
    Ok(SourceKeyProfileStatus { active, pending })
}

fn active_profile(
    index: &SourceKeyIndex,
) -> Result<&SourceKeyProfileMetadata, SourceKeyProfileError> {
    index
        .profiles
        .iter()
        .find(|profile| {
            profile.key_id == index.active_key_id && profile.state == SourceKeyProfileState::Active
        })
        .ok_or(SourceKeyProfileError::Invalid)
}

fn candidate_key_id(raw_key: &str) -> String {
    let digest = hex::encode(Sha256::digest(raw_key.as_bytes()));
    format!("work-cn-{}", &digest[..16])
}

fn validate_raw_key(raw_key: &str) -> Result<(), SourceKeyProfileError> {
    if raw_key.len() != 64
        || hex::decode(raw_key)
            .map(|decoded| decoded.len() != 32)
            .unwrap_or(true)
    {
        return Err(SourceKeyProfileError::Invalid);
    }
    Ok(())
}

fn write_protected_key(path: &Path, raw_key: &str) -> Result<(), SourceKeyProfileError> {
    let encrypted =
        protect_secret(raw_key.as_bytes()).map_err(|_| SourceKeyProfileError::Unavailable)?;
    if encrypted.is_empty() || encrypted.len() as u64 > MAX_KEY_BLOB_BYTES {
        return Err(SourceKeyProfileError::Invalid);
    }
    let mut bytes = Vec::with_capacity(KEY_MAGIC.len() + 8 + encrypted.len());
    bytes.extend_from_slice(KEY_MAGIC);
    bytes.extend_from_slice(&FORMAT_VERSION.to_le_bytes());
    bytes.extend_from_slice(&(encrypted.len() as u32).to_le_bytes());
    bytes.extend_from_slice(&encrypted);
    write_atomically(path, &bytes)
}

fn read_protected_key(path: &Path) -> Result<String, SourceKeyProfileError> {
    let metadata = fs::symlink_metadata(path).map_err(|_| SourceKeyProfileError::Unavailable)?;
    if is_reparse_or_symlink(&metadata)
        || !metadata.is_file()
        || metadata.len() > MAX_KEY_BLOB_BYTES + 16
    {
        return Err(SourceKeyProfileError::Invalid);
    }
    let bytes = read_exact_file(path, metadata.len(), MAX_KEY_BLOB_BYTES + 16)?;
    if bytes.len() < KEY_MAGIC.len() + 8 || &bytes[..KEY_MAGIC.len()] != KEY_MAGIC {
        return Err(SourceKeyProfileError::Invalid);
    }
    let version_offset = KEY_MAGIC.len();
    let version = u32::from_le_bytes(
        bytes[version_offset..version_offset + 4]
            .try_into()
            .map_err(|_| SourceKeyProfileError::Invalid)?,
    );
    let length_offset = version_offset + 4;
    let length = u32::from_le_bytes(
        bytes[length_offset..length_offset + 4]
            .try_into()
            .map_err(|_| SourceKeyProfileError::Invalid)?,
    ) as usize;
    let start = length_offset + 4;
    if version != FORMAT_VERSION
        || length == 0
        || length as u64 > MAX_KEY_BLOB_BYTES
        || bytes.len() != start + length
    {
        return Err(SourceKeyProfileError::Invalid);
    }
    let plaintext =
        unprotect_secret(&bytes[start..]).map_err(|_| SourceKeyProfileError::Invalid)?;
    let raw_key = String::from_utf8(plaintext).map_err(|_| SourceKeyProfileError::Invalid)?;
    validate_raw_key(&raw_key)?;
    Ok(raw_key)
}

fn write_atomically(path: &Path, bytes: &[u8]) -> Result<(), SourceKeyProfileError> {
    let parent = path.parent().ok_or(SourceKeyProfileError::Invalid)?;
    reject_path_chain(parent, true)?;
    fs::create_dir_all(parent).map_err(|_| SourceKeyProfileError::Unavailable)?;
    reject_path_chain(parent, false)?;
    if fs::symlink_metadata(path)
        .map(|metadata| is_reparse_or_symlink(&metadata) || !metadata.is_file())
        .unwrap_or(false)
    {
        return Err(SourceKeyProfileError::Invalid);
    }
    let temporary = parent.join(format!(
        ".{}.tmp-{}-{}",
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("source-key"),
        now_unix_nanos(),
        std::process::id()
    ));
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .map_err(|_| SourceKeyProfileError::Unavailable)?;
        file.write_all(bytes)
            .map_err(|_| SourceKeyProfileError::Unavailable)?;
        file.sync_all()
            .map_err(|_| SourceKeyProfileError::Unavailable)?;
        drop(file);
        publish_replacing(&temporary, path).map_err(|_| SourceKeyProfileError::Unavailable)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

fn read_exact_file(
    path: &Path,
    expected_len: u64,
    max_len: u64,
) -> Result<Vec<u8>, SourceKeyProfileError> {
    let file = File::open(path).map_err(|_| SourceKeyProfileError::Unavailable)?;
    let mut bytes = Vec::with_capacity(expected_len as usize);
    file.take(max_len.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|_| SourceKeyProfileError::Unavailable)?;
    if bytes.len() as u64 != expected_len || bytes.len() as u64 > max_len {
        return Err(SourceKeyProfileError::Invalid);
    }
    Ok(bytes)
}

fn reject_path_chain(path: &Path, allow_missing: bool) -> Result<(), SourceKeyProfileError> {
    let mut current = Some(path);
    while let Some(candidate) = current {
        match fs::symlink_metadata(candidate) {
            Ok(metadata) if is_reparse_or_symlink(&metadata) || !metadata.is_dir() => {
                return Err(SourceKeyProfileError::Invalid)
            }
            Ok(_) => {}
            Err(error) if allow_missing && error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Err(SourceKeyProfileError::Invalid)
            }
            Err(_) => return Err(SourceKeyProfileError::Unavailable),
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

fn is_reparse_or_symlink(metadata: &fs::Metadata) -> bool {
    if metadata.file_type().is_symlink() {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        return metadata.file_attributes() & 0x400 != 0;
    }
    #[cfg(not(windows))]
    {
        false
    }
}

fn now_unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

fn now_unix_nanos() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0)
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    const BASELINE: &str = "11b5b4d0b9e0c1784c2b0f8cfa6a2d5c0e41ec87c4c654720d6dcb6207fdbf5b";
    const CANDIDATE: &str = "22b5b4d0b9e0c1784c2b0f8cfa6a2d5c0e41ec87c4c654720d6dcb6207fdbf5b";

    #[test]
    fn verified_candidate_stays_pending_until_next_startup() {
        let root = tempfile::tempdir().unwrap();
        let store = SourceKeyProfileStore::new(root.path());
        let status = store
            .register_verified_candidate(CANDIDATE, "1.108", "schema-a", "work_cn_v1")
            .unwrap();
        assert_eq!(status.active.key_id, BASELINE_SOURCE_KEY_ID);
        assert!(status.pending.is_some());

        let activation = store.activate_pending(BASELINE).unwrap();
        assert!(activation.activation_changed);
        assert_eq!(activation.raw_key, CANDIDATE);
        assert!(store.status().unwrap().pending.is_none());
    }

    #[test]
    fn candidate_file_never_contains_raw_key_plaintext() {
        let root = tempfile::tempdir().unwrap();
        let store = SourceKeyProfileStore::new(root.path());
        store
            .register_verified_candidate(CANDIDATE, "1.108", "schema-a", "work_cn_v1")
            .unwrap();
        let entry = fs::read_dir(root.path().join("keys"))
            .unwrap()
            .next()
            .unwrap()
            .unwrap();
        let bytes = fs::read(entry.path()).unwrap();
        assert!(!bytes
            .windows(CANDIDATE.len())
            .any(|window| window == CANDIDATE.as_bytes()));
    }

    #[test]
    fn damaged_pending_key_does_not_replace_active_profile() {
        let root = tempfile::tempdir().unwrap();
        let store = SourceKeyProfileStore::new(root.path());
        let status = store
            .register_verified_candidate(CANDIDATE, "1.108", "schema-a", "work_cn_v1")
            .unwrap();
        let pending = status.pending.unwrap();
        fs::write(store.key_path(&pending.key_id), b"damaged").unwrap();

        assert_eq!(
            store.activate_pending(BASELINE),
            Err(SourceKeyProfileError::Invalid)
        );
        assert_eq!(
            store.status().unwrap().active.key_id,
            BASELINE_SOURCE_KEY_ID
        );
    }
}
