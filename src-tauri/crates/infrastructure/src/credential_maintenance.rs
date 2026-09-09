//! 凭据维护策略状态：只保存非敏感的退避与尝试记录。
//!
//! 该模块不读取或写入 token；它只负责让 App 内自动维护在重启后仍遵守
//! 账号级退避和每日上限。手动刷新不读取或受这里的策略拦截，成功后可写入
//! 成功时间以清除过期的自动退避。

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::atomic_publish::publish_replacing;

const STORE_FILE: &str = "credential-maintenance.json";
const FORMAT_VERSION: u32 = 1;
const MAX_STORE_BYTES: u64 = 64 * 1024;
const SECONDS_PER_DAY: u64 = 24 * 60 * 60;

/// 自动凭据维护的最短退避阶梯：1h → 2h → 4h → 8h → 每天最多一次。
pub const CREDENTIAL_BACKOFF_STEPS_SECONDS: [u64; 4] =
    [60 * 60, 2 * 60 * 60, 4 * 60 * 60, 8 * 60 * 60];

/// 单账号每日自动维护上限；手动刷新不受此上限限制。
pub const MAX_DAILY_MAINTENANCE_ATTEMPTS: u32 = 4;

/// 凭据维护状态存储错误；不携带文件内容或认证材料。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CredentialMaintenanceStoreError {
    /// 文件格式、版本或路径类型不符合预期。
    Invalid,
    /// 文件读写失败。
    Io,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct MaintenanceEntry {
    #[serde(default)]
    failure_streak: u32,
    #[serde(default)]
    next_attempt_at_unix_seconds: u64,
    #[serde(default)]
    attempt_day: u64,
    #[serde(default)]
    attempts_today: u32,
    #[serde(default)]
    last_attempt_at_unix_seconds: Option<u64>,
    #[serde(default)]
    last_success_at_unix_seconds: Option<u64>,
    #[serde(default)]
    last_error_code: Option<String>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct MaintenanceFile {
    format_version: u32,
    #[serde(default)]
    accounts: BTreeMap<String, MaintenanceEntry>,
}

/// 账号级凭据维护策略状态；文件内不保存 token、设备密钥或手机号。
#[derive(Debug, Clone)]
pub struct CredentialMaintenanceStateStore {
    path: PathBuf,
}

impl CredentialMaintenanceStateStore {
    /// 创建位于 `<checkin_root>/credential-maintenance.json` 的状态存储。
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            path: root.into().join(STORE_FILE),
        }
    }

    /// 判断账号当前是否允许自动触发一次维护。
    pub fn is_attempt_allowed(
        &self,
        profile_id: &str,
        now_unix_seconds: u64,
    ) -> Result<bool, CredentialMaintenanceStoreError> {
        validate_profile_id(profile_id)?;
        let file = self.load()?;
        let Some(entry) = file.accounts.get(profile_id) else {
            return Ok(true);
        };
        if now_unix_seconds < entry.next_attempt_at_unix_seconds {
            return Ok(false);
        }
        if entry.attempt_day == utc_day(now_unix_seconds)
            && entry.attempts_today >= MAX_DAILY_MAINTENANCE_ATTEMPTS
        {
            return Ok(false);
        }
        Ok(true)
    }

    /// 记录自动维护失败，并持久化下一次允许尝试的时间。
    pub fn record_failure(
        &self,
        profile_id: &str,
        now_unix_seconds: u64,
        error_code: &str,
    ) -> Result<(), CredentialMaintenanceStoreError> {
        validate_profile_id(profile_id)?;
        if error_code.is_empty() || error_code.len() > 128 {
            return Err(CredentialMaintenanceStoreError::Invalid);
        }

        let mut file = self.load()?;
        let entry = file.accounts.entry(profile_id.to_string()).or_default();
        let day = utc_day(now_unix_seconds);
        if entry.attempt_day != day {
            entry.attempt_day = day;
            entry.attempts_today = 0;
        }
        entry.attempts_today = entry.attempts_today.saturating_add(1);
        entry.failure_streak = entry.failure_streak.saturating_add(1);
        entry.last_attempt_at_unix_seconds = Some(now_unix_seconds);
        entry.last_error_code = Some(error_code.to_string());
        entry.next_attempt_at_unix_seconds = next_attempt_at(
            now_unix_seconds,
            day,
            entry.failure_streak,
            entry.attempts_today,
        );
        self.write(&file)
    }

    /// 记录自动维护成功，清零当前退避；保留最后一次错误码作为历史证据。
    pub fn record_success(
        &self,
        profile_id: &str,
        now_unix_seconds: u64,
    ) -> Result<(), CredentialMaintenanceStoreError> {
        validate_profile_id(profile_id)?;
        let mut file = self.load()?;
        let entry = file.accounts.entry(profile_id.to_string()).or_default();
        entry.failure_streak = 0;
        entry.next_attempt_at_unix_seconds = 0;
        entry.attempt_day = utc_day(now_unix_seconds);
        entry.attempts_today = 0;
        entry.last_success_at_unix_seconds = Some(now_unix_seconds);
        self.write(&file)
    }

    #[cfg(test)]
    fn read_entry(
        &self,
        profile_id: &str,
    ) -> Result<Option<MaintenanceEntry>, CredentialMaintenanceStoreError> {
        Ok(self.load()?.accounts.remove(profile_id))
    }

    fn load(&self) -> Result<MaintenanceFile, CredentialMaintenanceStoreError> {
        let metadata = match fs::symlink_metadata(&self.path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(MaintenanceFile {
                    format_version: FORMAT_VERSION,
                    ..MaintenanceFile::default()
                });
            }
            Err(_) => return Err(CredentialMaintenanceStoreError::Io),
        };
        if !metadata.is_file() || metadata.len() > MAX_STORE_BYTES {
            return Err(CredentialMaintenanceStoreError::Invalid);
        }
        let content =
            fs::read_to_string(&self.path).map_err(|_| CredentialMaintenanceStoreError::Io)?;
        let file: MaintenanceFile =
            serde_json::from_str(&content).map_err(|_| CredentialMaintenanceStoreError::Invalid)?;
        if file.format_version != FORMAT_VERSION {
            return Err(CredentialMaintenanceStoreError::Invalid);
        }
        Ok(file)
    }

    fn write(&self, file: &MaintenanceFile) -> Result<(), CredentialMaintenanceStoreError> {
        let content = serde_json::to_string_pretty(file)
            .map_err(|_| CredentialMaintenanceStoreError::Invalid)?;
        if content.len() as u64 > MAX_STORE_BYTES {
            return Err(CredentialMaintenanceStoreError::Invalid);
        }
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent).map_err(|_| CredentialMaintenanceStoreError::Io)?;
        }
        let temporary = self.path.with_extension("json.tmp");
        fs::write(&temporary, content.as_bytes())
            .map_err(|_| CredentialMaintenanceStoreError::Io)?;
        publish_replacing(&temporary, &self.path).map_err(|_| CredentialMaintenanceStoreError::Io)
    }
}

fn validate_profile_id(profile_id: &str) -> Result<(), CredentialMaintenanceStoreError> {
    if profile_id.is_empty() || profile_id.len() > 256 {
        return Err(CredentialMaintenanceStoreError::Invalid);
    }
    Ok(())
}

fn utc_day(now_unix_seconds: u64) -> u64 {
    now_unix_seconds / SECONDS_PER_DAY
}

fn next_attempt_at(
    now_unix_seconds: u64,
    day: u64,
    failure_streak: u32,
    attempts_today: u32,
) -> u64 {
    if attempts_today >= MAX_DAILY_MAINTENANCE_ATTEMPTS {
        return day.saturating_add(1).saturating_mul(SECONDS_PER_DAY);
    }
    let step = failure_streak.saturating_sub(1) as usize;
    let delay = CREDENTIAL_BACKOFF_STEPS_SECONDS
        .get(step)
        .copied()
        .unwrap_or(SECONDS_PER_DAY);
    now_unix_seconds.saturating_add(delay)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failure_backoff_survives_store_reload() {
        let root = tempfile::tempdir().unwrap();
        let now = 1_800_000_000;
        let store = CredentialMaintenanceStateStore::new(root.path());

        store
            .record_failure("profile-a", now, "network_error")
            .unwrap();
        let reloaded = CredentialMaintenanceStateStore::new(root.path());
        assert!(!reloaded
            .is_attempt_allowed("profile-a", now + 3599)
            .unwrap());
        assert!(reloaded
            .is_attempt_allowed("profile-a", now + 3600)
            .unwrap());
    }

    #[test]
    fn daily_limit_survives_repeated_failures() {
        let root = tempfile::tempdir().unwrap();
        let day_start = 1_800_000_000 / SECONDS_PER_DAY * SECONDS_PER_DAY;
        let store = CredentialMaintenanceStateStore::new(root.path());

        for (offset, code) in [
            (0, "network_error"),
            (3600, "network_error"),
            (10800, "network_error"),
            (25200, "credential_refresh_failed"),
        ] {
            store
                .record_failure("profile-a", day_start + offset, code)
                .unwrap();
        }

        assert!(!store
            .is_attempt_allowed("profile-a", day_start + 86400 - 1)
            .unwrap());
        assert!(store
            .is_attempt_allowed("profile-a", day_start + 86400)
            .unwrap());
    }

    #[test]
    fn success_resets_backoff_without_erasing_failure_evidence() {
        let root = tempfile::tempdir().unwrap();
        let store = CredentialMaintenanceStateStore::new(root.path());
        store
            .record_failure("profile-a", 1_800_000_000, "network_error")
            .unwrap();
        store.record_success("profile-a", 1_800_000_100).unwrap();

        assert!(store
            .is_attempt_allowed("profile-a", 1_800_000_101)
            .unwrap());
        let entry = store.read_entry("profile-a").unwrap().unwrap();
        assert_eq!(entry.failure_streak, 0);
        assert_eq!(entry.last_error_code.as_deref(), Some("network_error"));
        assert_eq!(entry.last_success_at_unix_seconds, Some(1_800_000_100));
    }

    #[test]
    fn corrupt_state_is_not_treated_as_empty() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join(STORE_FILE), b"{not-json").unwrap();
        let store = CredentialMaintenanceStateStore::new(root.path());

        // 状态损坏必须 fail-closed，不能静默当成首次运行而再次触网。
        assert_eq!(
            store.is_attempt_allowed("profile-a", 1_800_000_000),
            Err(CredentialMaintenanceStoreError::Invalid)
        );
    }

    #[test]
    fn invalid_profile_id_is_rejected_before_file_access() {
        let root = tempfile::tempdir().unwrap();
        let store = CredentialMaintenanceStateStore::new(root.path());

        assert_eq!(
            store.is_attempt_allowed("", 1_800_000_000),
            Err(CredentialMaintenanceStoreError::Invalid)
        );
        assert_eq!(
            store.record_success(&"x".repeat(257), 1_800_000_000),
            Err(CredentialMaintenanceStoreError::Invalid)
        );
    }
}
