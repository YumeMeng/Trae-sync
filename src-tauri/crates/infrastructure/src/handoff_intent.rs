//! 固定恢复区承接意图的原子存储。
//!
//! 文件只保存稳定选择和证据摘要，不保存正文、旧同步计划或登录材料。

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use traesync_domain::HandoffIntent;
use traesync_ports::HandoffIntentStorePort;

use crate::atomic_publish::publish_replacing;

const MAX_INTENT_BYTES: u64 = 256 * 1024;

/// JSON 承接意图存储。
#[derive(Debug, Clone)]
pub struct JsonHandoffIntentStore {
    path: PathBuf,
}

impl JsonHandoffIntentStore {
    pub fn new(recovery_root: impl Into<PathBuf>) -> Self {
        Self {
            path: recovery_root.into().join("handoff-intent.json"),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn validate_root(&self) -> Result<(), String> {
        let root = self
            .path
            .parent()
            .ok_or_else(|| "handoff_intent_root_invalid".to_string())?;
        let metadata = fs::symlink_metadata(root)
            .map_err(|_| "handoff_intent_root_unavailable".to_string())?;
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            return Err("handoff_intent_root_invalid".to_string());
        }
        Ok(())
    }

    fn read_bytes(&self) -> Result<Option<Vec<u8>>, String> {
        let metadata = match fs::symlink_metadata(&self.path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(_) => return Err("handoff_intent_unavailable".to_string()),
        };
        if metadata.file_type().is_symlink()
            || !metadata.is_file()
            || metadata.len() > MAX_INTENT_BYTES
        {
            return Err("handoff_intent_invalid".to_string());
        }
        let bytes = fs::read(&self.path).map_err(|_| "handoff_intent_unavailable".to_string())?;
        if bytes.len() as u64 != metadata.len() || bytes.len() as u64 > MAX_INTENT_BYTES {
            return Err("handoff_intent_invalid".to_string());
        }
        Ok(Some(bytes))
    }

    /// 读取当前意图；组合根只拿到脱敏领域值。
    pub fn load(&self) -> Result<Option<HandoffIntent>, String> {
        self.validate_root()?;
        let Some(bytes) = self.read_bytes()? else {
            return Ok(None);
        };
        serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|_| "handoff_intent_invalid".to_string())
    }

    /// 原子发布意图；不允许删除旧文件或覆盖符号链接。
    pub fn publish(&self, intent: &HandoffIntent) -> Result<(), String> {
        self.validate_root()?;
        let bytes = serde_json::to_vec_pretty(intent)
            .map_err(|_| "handoff_intent_serialize_failed".to_string())?;
        if bytes.len() as u64 > MAX_INTENT_BYTES {
            return Err("handoff_intent_too_large".to_string());
        }
        let temporary = self.path.with_extension(format!(
            "json.tmp-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        let result = (|| {
            let mut file = OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(&temporary)
                .map_err(|_| "handoff_intent_publish_failed".to_string())?;
            file.write_all(&bytes)
                .map_err(|_| "handoff_intent_publish_failed".to_string())?;
            file.sync_all()
                .map_err(|_| "handoff_intent_publish_failed".to_string())?;
            publish_replacing(&temporary, &self.path)
                .map_err(|_| "handoff_intent_publish_failed".to_string())
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        result
    }
}

impl HandoffIntentStorePort for JsonHandoffIntentStore {
    fn load_intent(&self) -> Result<Option<HandoffIntent>, String> {
        self.load()
    }

    fn publish_intent(&self, intent: &HandoffIntent) -> Result<(), String> {
        self.publish(intent)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::SystemTime;
    use traesync_domain::{HandoffIntentState, SyncScope};

    fn intent() -> HandoffIntent {
        HandoffIntent {
            intent_id: "intent-1".to_string(),
            source_profile_id: Some("profile-a".to_string()),
            target_profile_id: "profile-b".to_string(),
            data_location_id: "location-1".to_string(),
            scope: SyncScope::AllHistory,
            catalog_id: Some("catalog-1".to_string()),
            catalog_generation: Some("catalog-gen-3".to_string()),
            schema_version: Some("work-cn-v1".to_string()),
            mapping_version: Some("work-cn-mapping-v1".to_string()),
            credential_operation_id: None,
            state: HandoffIntentState::Prepared,
            created_at: SystemTime::UNIX_EPOCH,
            updated_at: SystemTime::UNIX_EPOCH,
            failure_reason: None,
        }
    }

    #[test]
    fn intent_round_trips_atomically() {
        let root = tempfile::tempdir().unwrap();
        let store = JsonHandoffIntentStore::new(root.path());
        store.publish_intent(&intent()).unwrap();
        assert_eq!(store.load_intent().unwrap(), Some(intent()));
    }

    #[test]
    fn missing_intent_is_empty_without_creating_file() {
        let root = tempfile::tempdir().unwrap();
        let store = JsonHandoffIntentStore::new(root.path());
        assert_eq!(store.load_intent().unwrap(), None);
        assert!(!store.path().exists());
    }
}
