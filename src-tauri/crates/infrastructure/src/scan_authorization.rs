//! 历史读取授权落盘（U-6 W2，2026-08-29）。
//!
//! 授权是全局读取许可，不是账号档案，因此不进注册表，独立存放在
//! `storage_root` 下的 `scan_authorization.json`。内容只包含
//! data_location 身份指纹 + 账号证据指纹 + 授权时间戳，**不含任何密钥**
//! （密钥仍走现有派生链路）。
//!
//! 失效语义（复用 lib.rs 现有三失效链，不新造机制）：
//! - 位置指纹比对只使用跨进程重启稳定的身份字段（canonical_root /
//!   db_relative_path / data_location_id），不比较 wal/shm sidecar 身份——
//!   TRAE 每次重启都会重建 wal/shm，属会话内漂移，由实时读取链路负责。
//! - 账号证据指纹沿用授权时捕获的 user/auth 不可逆指纹。
//!
//! 损坏或缺失时按未授权降级（fail-closed），且不删除文件（铁律：不删证据）。

use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::atomic_publish::publish_replacing;
use crate::data_location::LocationWitness;

const STORE_FILE: &str = "scan_authorization.json";
const MAX_STORE_BYTES: u64 = 64 * 1024;

/// 授权落盘读写错误；不携带敏感内容。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScanAuthorizationStoreError {
    /// 文件损坏或格式不符。
    Invalid,
    /// 读写失败。
    Io,
}

/// 落盘的授权记录：只保存指纹与时间戳，不保存密钥或原始 user_id。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PersistedScanAuthorization {
    /// 授权建立时捕获的位置身份见证（identity-only，不含完整哈希）。
    pub location_witness: LocationWitness,
    /// 账号身份不可逆指纹（授权时捕获）。
    pub user_fingerprint: String,
    /// 账号认证不可逆指纹（授权时捕获）。
    pub auth_fingerprint: String,
    /// 授权时间戳（Unix 秒），仅用于展示与诊断。
    pub authorized_at_unix_seconds: u64,
}

#[derive(Serialize, Deserialize)]
struct StoreFile {
    format_version: u32,
    record: PersistedScanAuthorization,
}

/// 授权落盘存储：`<storage_root>/scan_authorization.json`。
pub struct ScanAuthorizationStore {
    path: PathBuf,
}

impl ScanAuthorizationStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            path: root.into().join(STORE_FILE),
        }
    }

    /// 读取落盘授权；文件不存在时返回 `None`（首次使用/已撤销）。
    pub fn load(&self) -> Result<Option<PersistedScanAuthorization>, ScanAuthorizationStoreError> {
        let metadata = match fs::symlink_metadata(&self.path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(_) => return Err(ScanAuthorizationStoreError::Io),
        };
        if !metadata.is_file() || metadata.len() > MAX_STORE_BYTES {
            return Err(ScanAuthorizationStoreError::Invalid);
        }
        let content = fs::read_to_string(&self.path).map_err(|_| ScanAuthorizationStoreError::Io)?;
        let store: StoreFile = serde_json::from_str(&content)
            .map_err(|_| ScanAuthorizationStoreError::Invalid)?;
        if store.format_version != 1 {
            return Err(ScanAuthorizationStoreError::Invalid);
        }
        Ok(Some(store.record))
    }

    /// 写入授权记录（原子替换）；重复写入同一内容幂等。
    pub fn save(
        &self,
        record: &PersistedScanAuthorization,
    ) -> Result<(), ScanAuthorizationStoreError> {
        let store = StoreFile {
            format_version: 1,
            record: record.clone(),
        };
        let content = serde_json::to_string_pretty(&store)
            .map_err(|_| ScanAuthorizationStoreError::Invalid)?;
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent).map_err(|_| ScanAuthorizationStoreError::Io)?;
        }
        let temporary = self.path.with_extension("json.tmp");
        fs::write(&temporary, content.as_bytes()).map_err(|_| ScanAuthorizationStoreError::Io)?;
        publish_replacing(&temporary, &self.path).map_err(|_| ScanAuthorizationStoreError::Io)
    }

    /// 删除落盘授权文件（设置页"撤销历史读取授权"）。
    ///
    /// 文件不存在视为已撤销（幂等）；删除失败才返回错误。
    pub fn clear(&self) -> Result<(), ScanAuthorizationStoreError> {
        match fs::remove_file(&self.path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(_) => Err(ScanAuthorizationStoreError::Io),
        }
    }
}

/// 判断落盘授权记录是否仍与当前读取材料一致（恢复时的指纹比对）。
///
/// 位置比对只看跨重启稳定的三元组（canonical_root / db_relative_path /
/// data_location_id）；wal/shm sidecar 身份随 TRAE 重启重建，不属于落盘
/// 失效判定范围。账号证据指纹与 lib.rs 实时失效链使用同一组不可逆指纹。
pub fn persisted_record_matches(
    record: &PersistedScanAuthorization,
    current_witness: &LocationWitness,
    current_user_fingerprint: &str,
    current_auth_fingerprint: &str,
) -> bool {
    let stored = &record.location_witness;
    stored.canonical_root == current_witness.canonical_root
        && stored.db_relative_path == current_witness.db_relative_path
        && stored.data_location_id == current_witness.data_location_id
        && record.user_fingerprint == current_user_fingerprint
        && record.auth_fingerprint == current_auth_fingerprint
}

#[cfg(test)]
mod tests {
    use super::*;
    use traesync_domain::FileIdentity;

    /// 构造合成位置见证：只填落盘比对消费的稳定身份字段。
    fn synthetic_witness(root: &str, location_id: &str) -> LocationWitness {
        LocationWitness {
            data_location_id: location_id.to_string(),
            canonical_root: root.to_string(),
            db_relative_path: "ModularData/ai-agent/database.db".to_string(),
            root_identity: Some(FileIdentity {
                volume_serial: 1,
                file_index_high: 0,
                file_index_low: 1,
            }),
            db_identity: Some(FileIdentity {
                volume_serial: 1,
                file_index_high: 0,
                file_index_low: 2,
            }),
            wal_identity: None,
            shm_identity: None,
            db_sha256: None,
            wal_sha256: None,
            shm_sha256: None,
        }
    }

    fn synthetic_record() -> PersistedScanAuthorization {
        PersistedScanAuthorization {
            location_witness: synthetic_witness(r"C:\TRAE", "loc-abc"),
            user_fingerprint: "user-fp".to_string(),
            auth_fingerprint: "auth-fp".to_string(),
            authorized_at_unix_seconds: 1_700_000_000,
        }
    }

    #[test]
    fn save_and_load_roundtrip_is_idempotent() {
        let root = tempfile::tempdir().unwrap();
        let store = ScanAuthorizationStore::new(root.path());
        let record = synthetic_record();

        // 写两次（幂等：重复授权覆盖写），读取结果一致。
        store.save(&record).unwrap();
        store.save(&record).unwrap();
        assert_eq!(store.load().unwrap(), Some(record));
    }

    #[test]
    fn missing_file_degrades_to_unauthorized() {
        let root = tempfile::tempdir().unwrap();
        let store = ScanAuthorizationStore::new(root.path());

        // 旧文件缺失 = 从未授权/已撤销：按未授权降级，不报错。
        assert_eq!(store.load().unwrap(), None);
    }

    #[test]
    fn corrupted_file_fails_closed_without_deletion() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join(STORE_FILE);
        fs::write(&path, "{ not json").unwrap();

        // 损坏时读取失败（按未授权降级），且不删除文件（铁律：不删证据）。
        assert!(ScanAuthorizationStore::new(root.path()).load().is_err());
        assert!(path.exists());
    }

    #[test]
    fn revoke_clears_file_and_is_idempotent() {
        let root = tempfile::tempdir().unwrap();
        let store = ScanAuthorizationStore::new(root.path());
        store.save(&synthetic_record()).unwrap();

        store.clear().unwrap();
        assert_eq!(store.load().unwrap(), None);

        // 文件已不存在时再次撤销幂等成功。
        store.clear().unwrap();
    }

    #[test]
    fn fingerprint_mismatch_is_rejected() {
        let record = synthetic_record();
        let same_witness = synthetic_witness(r"C:\TRAE", "loc-abc");
        let moved_witness = synthetic_witness(r"C:\TRAE-moved", "loc-abc");
        let replaced_witness = synthetic_witness(r"C:\TRAE", "loc-other");

        // 全部一致才允许恢复。
        assert!(persisted_record_matches(
            &record,
            &same_witness,
            "user-fp",
            "auth-fp"
        ));
        // 位置指纹三要素任一变化（移动/同路径替换）都拒绝。
        assert!(!persisted_record_matches(
            &record,
            &moved_witness,
            "user-fp",
            "auth-fp"
        ));
        assert!(!persisted_record_matches(
            &record,
            &replaced_witness,
            "user-fp",
            "auth-fp"
        ));
        // 账号证据指纹变化（user/auth 任一）都拒绝。
        assert!(!persisted_record_matches(
            &record,
            &same_witness,
            "user-fp-changed",
            "auth-fp"
        ));
        assert!(!persisted_record_matches(
            &record,
            &same_witness,
            "user-fp",
            "auth-fp-changed"
        ));
    }

    #[test]
    fn persisted_file_contains_no_raw_key_material() {
        let root = tempfile::tempdir().unwrap();
        let store = ScanAuthorizationStore::new(root.path());
        store.save(&synthetic_record()).unwrap();

        // 落盘内容只含指纹与身份见证，不含密钥正文。
        let content = fs::read_to_string(root.path().join(STORE_FILE)).unwrap();
        assert!(!content.contains("3605f669"));
        assert!(!content.contains("raw_key"));
        assert!(content.contains("user_fingerprint"));
        assert!(content.contains("auth_fingerprint"));
    }
}
