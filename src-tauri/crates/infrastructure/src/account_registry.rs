//! MVP 非敏感账号注册表：OAuth 登录成功后的账号档案（JSON 文件）。
//!
//! 只保存可进入前端展示的字段；Token、refresh token、设备私钥等敏感材料
//! 只存在于 `CheckinCredentialStore` 的 DPAPI 加密凭据包中（见 ADR-0014）。

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::atomic_publish::publish_replacing;

const REGISTRY_FILE: &str = "accounts.json";
const MAX_REGISTRY_BYTES: u64 = 1024 * 1024;
const MAX_TEXT_FIELD_BYTES: usize = 1024;

/// 注册表错误；不携带敏感内容。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AccountRegistryError {
    /// 文件损坏或格式不符。
    Invalid,
    /// 记录字段非法（空/超长）。
    InvalidRecord,
    /// 读写失败。
    Io,
}

/// 一条非敏感账号档案。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountRecord {
    pub profile_id: String,
    /// 服务端账号 ID（GetUserInfo 校验通过的 userId）。
    pub account_id: String,
    pub screen_name: String,
    pub avatar_url: String,
    /// 该账号的虚拟设备 ID（签到配额隔离维度）。
    pub device_id: String,
    /// 设备公钥（SPKI PEM）；公钥本身非敏感，与加密凭据包交叉校验绑定。
    pub device_public_key: String,
    /// 本地备注名：用户自定义显示别名；None = 使用服务端 screen_name。
    /// 属用户本地偏好，重新登录/重铸设备均保留（与 auto_checkin_enabled 同类）。
    #[serde(default)]
    pub display_name: Option<String>,
    /// 脱敏手机号（如 138****0000）：登录时 GetUserInfo 自动采集；
    /// 空串 = 尚未采集（存量账号由"刷新额度"路径无感补采）。
    #[serde(default)]
    pub masked_mobile: String,
    pub created_at_unix_seconds: u64,
    pub last_verified_at_unix_seconds: u64,
    /// 当前签到设备的铸造/重铸时刻（Unix 秒）；0 = 未知（存量档案）。
    /// 9074 报错时用于区分「设备刚创建、稍后重试即可」与「设备被拒、
    /// 需要重置」两种引导（2026-09-02 用户4993529391 实测：新设备
    /// 铸造后 12 秒首签被 9074 拒，192 秒后同设备重试成功）。
    #[serde(default)]
    pub device_created_at_unix_seconds: u64,
    /// 该账号是否参与自动签到（grill 2026-08-23 决策 2：每账号独立开关，默认开启；
    /// 手动签到不受此开关影响）。
    #[serde(default = "default_auto_checkin_enabled")]
    pub auto_checkin_enabled: bool,
    /// 是否归档（U-6 W4 资产库三态筛选）：归档账号移入「已归档」视图，
    /// 档案与本地记录全部保留；彻底删除记录是独立动作（purge 命令）。
    /// 属用户本地偏好，重新登录/重铸设备均保留（与 display_name 同类）。
    #[serde(default)]
    pub archived: bool,
}

fn default_auto_checkin_enabled() -> bool {
    true
}

impl AccountRecord {
    /// 校验字段非空且不超长；头像允许为空（服务端可能不返回）。
    pub fn validate(&self) -> Result<(), AccountRegistryError> {
        for field in [
            &self.profile_id,
            &self.account_id,
            &self.screen_name,
            &self.device_id,
            &self.device_public_key,
        ] {
            if field.is_empty() || field.len() > MAX_TEXT_FIELD_BYTES {
                return Err(AccountRegistryError::InvalidRecord);
            }
        }
        if self.avatar_url.len() > MAX_TEXT_FIELD_BYTES {
            return Err(AccountRegistryError::InvalidRecord);
        }
        // 备注名不变式：Some = 非空且不超长（空别名由调用方归一化为 None）。
        if let Some(alias) = &self.display_name {
            if alias.is_empty() || alias.len() > MAX_TEXT_FIELD_BYTES {
                return Err(AccountRegistryError::InvalidRecord);
            }
        }
        if self.masked_mobile.len() > MAX_TEXT_FIELD_BYTES {
            return Err(AccountRegistryError::InvalidRecord);
        }
        Ok(())
    }
}

#[derive(Serialize, Deserialize)]
struct RegistryFile {
    format_version: u32,
    accounts: Vec<AccountRecord>,
}

/// 账号注册表；按 profile_id 唯一，upsert 覆盖旧记录。
pub struct AccountRegistry {
    path: PathBuf,
}

impl AccountRegistry {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            path: root.into().join(REGISTRY_FILE),
        }
    }

    /// 读取全部档案；文件不存在时返回空表（首次使用前无需预创建）。
    pub fn load(&self) -> Result<Vec<AccountRecord>, AccountRegistryError> {
        let metadata = match fs::symlink_metadata(&self.path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(_) => return Err(AccountRegistryError::Io),
        };
        if !metadata.is_file() || metadata.len() > MAX_REGISTRY_BYTES {
            return Err(AccountRegistryError::Invalid);
        }
        let content = fs::read_to_string(&self.path).map_err(|_| AccountRegistryError::Io)?;
        let registry: RegistryFile =
            serde_json::from_str(&content).map_err(|_| AccountRegistryError::Invalid)?;
        if registry.format_version != 1 {
            return Err(AccountRegistryError::Invalid);
        }
        for record in &registry.accounts {
            record.validate()?;
        }
        Ok(registry.accounts)
    }

    /// 插入或更新一条档案；原子写入，失败时保留原文件。
    pub fn upsert(&self, record: &AccountRecord) -> Result<(), AccountRegistryError> {
        record.validate()?;
        let mut accounts = self.load()?;
        let mut index = accounts.len();
        for (position, existing) in accounts.iter().enumerate() {
            if existing.profile_id == record.profile_id {
                index = position;
                break;
            }
        }
        if index == accounts.len() {
            accounts.push(record.clone());
        } else {
            accounts[index] = record.clone();
        }
        self.write_all(&accounts)
    }

    /// 按 profile_id 查找档案。
    pub fn find(&self, profile_id: &str) -> Result<Option<AccountRecord>, AccountRegistryError> {
        Ok(self
            .load()?
            .into_iter()
            .find(|record| record.profile_id == profile_id))
    }

    /// 按 profile_id 移除档案；不存在返回 false（幂等区分），存在且删除成功返回 true。
    /// 原子写入：失败时保留原文件。
    pub fn remove(&self, profile_id: &str) -> Result<bool, AccountRegistryError> {
        let accounts = self.load()?;
        let remaining: Vec<AccountRecord> = accounts
            .iter()
            .filter(|record| record.profile_id != profile_id)
            .cloned()
            .collect();
        if remaining.len() == accounts.len() {
            return Ok(false);
        }
        self.write_all(&remaining)?;
        Ok(true)
    }

    /// 更新单账号的自动签到开关（读-改-写，原子落盘）。
    /// 账号不存在返回 false；开关值无变化时跳过写入（幂等）。
    pub fn set_auto_checkin_enabled(
        &self,
        profile_id: &str,
        enabled: bool,
    ) -> Result<bool, AccountRegistryError> {
        self.update_record(profile_id, |record| {
            if record.auto_checkin_enabled == enabled {
                return false;
            }
            record.auto_checkin_enabled = enabled;
            true
        })
    }

    /// 设置/清除本地备注名（读-改-写，原子落盘）。
    /// 空串与 None 等价（清除，回退服务端 screen_name）；同值跳过写入（幂等）。
    pub fn set_display_name(
        &self,
        profile_id: &str,
        display_name: Option<&str>,
    ) -> Result<bool, AccountRegistryError> {
        let normalized: Option<String> = display_name
            .map(str::trim)
            .filter(|alias| !alias.is_empty())
            .map(str::to_string);
        self.update_record(profile_id, |record| {
            if record.display_name == normalized {
                return false;
            }
            record.display_name = normalized.clone();
            true
        })
    }

    /// 设置归档标记（读-改-写，原子落盘）。
    /// 账号不存在返回 false；同值跳过写入（幂等）。
    pub fn set_archived(
        &self,
        profile_id: &str,
        archived: bool,
    ) -> Result<bool, AccountRegistryError> {
        self.update_record(profile_id, |record| {
            if record.archived == archived {
                return false;
            }
            record.archived = archived;
            true
        })
    }

    /// 补采脱敏手机号：仅当档案值为空且入参非空时写入（首采语义，
    /// 不覆盖已有值——服务端值一旦采集即视为稳定）。幂等。
    pub fn backfill_masked_mobile(
        &self,
        profile_id: &str,
        masked_mobile: &str,
    ) -> Result<bool, AccountRegistryError> {
        if masked_mobile.is_empty() {
            return Ok(true);
        }
        self.update_record(profile_id, |record| {
            if !record.masked_mobile.is_empty() {
                return false;
            }
            record.masked_mobile = masked_mobile.to_string();
            true
        })
    }

    /// 读-改-写单账号档案的通用原语；闭包返回 true 表示有变更需落盘。
    fn update_record(
        &self,
        profile_id: &str,
        mutate: impl Fn(&mut AccountRecord) -> bool,
    ) -> Result<bool, AccountRegistryError> {
        let mut accounts = self.load()?;
        let Some(record) = accounts
            .iter_mut()
            .find(|record| record.profile_id == profile_id)
        else {
            return Ok(false);
        };
        if !mutate(record) {
            return Ok(true);
        }
        self.write_all(&accounts)?;
        Ok(true)
    }

    /// 生成凭据绑定集合（profile_id -> CheckinProfileBinding），供真实签到
    /// transport 使用。公钥来自加密凭据包之外的登记值不可得，这里不读取
    /// 凭据包——绑定由调用方（签到编排）从凭据包读取后构造。
    fn write_all(&self, accounts: &[AccountRecord]) -> Result<(), AccountRegistryError> {
        let registry = RegistryFile {
            format_version: 1,
            accounts: accounts.to_vec(),
        };
        let content =
            serde_json::to_string_pretty(&registry).map_err(|_| AccountRegistryError::Invalid)?;
        let bytes = content.as_bytes();
        if bytes.len() as u64 > MAX_REGISTRY_BYTES {
            return Err(AccountRegistryError::Invalid);
        }
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent).map_err(|_| AccountRegistryError::Io)?;
        }
        let temporary = self.path.with_extension("json.tmp");
        fs::write(&temporary, bytes).map_err(|_| AccountRegistryError::Io)?;
        publish_replacing(&temporary, &self.path).map_err(|_| AccountRegistryError::Io)
    }
}

/// 从账号档案构造唯一性索引（profile_id -> 档案），便于编排层快速查找。
pub fn account_records_by_profile_id(
    records: &[AccountRecord],
) -> BTreeMap<String, &AccountRecord> {
    records
        .iter()
        .map(|record| (record.profile_id.clone(), record))
        .collect()
}

/// 检查注册表文件路径边界（测试辅助）。
pub fn registry_path(root: &Path) -> PathBuf {
    root.join(REGISTRY_FILE)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn sample_record(profile_id: &str) -> AccountRecord {
        AccountRecord {
            profile_id: profile_id.to_string(),
            account_id: "account-1".to_string(),
            screen_name: "测试用户".to_string(),
            avatar_url: "https://example.test/avatar.png".to_string(),
            device_id: "1234567890123456".to_string(),
            device_public_key: "public-key-pem".to_string(),
            display_name: None,
            masked_mobile: String::new(),
            created_at_unix_seconds: 1_800_000_000,
            last_verified_at_unix_seconds: 1_800_000_000,
            device_created_at_unix_seconds: 1_800_000_000,
            auto_checkin_enabled: true,
            archived: false,
        }
    }

    #[test]
    fn missing_file_returns_empty_list() {
        let root = tempdir().unwrap();
        let registry = AccountRegistry::new(root.path());
        assert!(registry.load().unwrap().is_empty());
    }

    #[test]
    fn upsert_inserts_then_updates_same_profile() {
        let root = tempdir().unwrap();
        let registry = AccountRegistry::new(root.path());
        registry.upsert(&sample_record("profile-a")).unwrap();
        let mut updated = sample_record("profile-a");
        updated.screen_name = "新名字".to_string();
        registry.upsert(&updated).unwrap();

        let records = registry.load().unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].screen_name, "新名字");
    }

    #[test]
    fn upsert_keeps_distinct_profiles() {
        let root = tempdir().unwrap();
        let registry = AccountRegistry::new(root.path());
        registry.upsert(&sample_record("profile-a")).unwrap();
        registry.upsert(&sample_record("profile-b")).unwrap();
        assert_eq!(registry.load().unwrap().len(), 2);
        assert!(registry.find("profile-b").unwrap().is_some());
    }

    #[test]
    fn set_auto_checkin_enabled_updates_and_is_idempotent() {
        let root = tempdir().unwrap();
        let registry = AccountRegistry::new(root.path());
        registry.upsert(&sample_record("profile-a")).unwrap();
        // 默认开启 -> 关闭。
        assert!(registry.set_auto_checkin_enabled("profile-a", false).unwrap());
        assert!(
            !registry
                .find("profile-a")
                .unwrap()
                .unwrap()
                .auto_checkin_enabled
        );
        // 幂等：同值再写不报错、不改内容。
        assert!(registry.set_auto_checkin_enabled("profile-a", false).unwrap());
        // 不存在的账号返回 false。
        assert!(!registry.set_auto_checkin_enabled("profile-404", true).unwrap());
    }

    #[test]
    fn old_registry_files_default_auto_checkin_on() {
        // 旧版本 accounts.json 无 auto_checkin_enabled 字段：
        // serde default 兜底为 true，保证升级后现有账号自动参与。
        let root = tempdir().unwrap();
        let path = root.path().join("accounts.json");
        std::fs::write(
            &path,
            r#"{"format_version":1,"accounts":[{"profile_id":"legacy","account_id":"a1","screen_name":"旧账号","avatar_url":"","device_id":"1234567890123456","device_public_key":"k","created_at_unix_seconds":0,"last_verified_at_unix_seconds":0}]}"#,
        )
        .unwrap();
        let registry = AccountRegistry::new(root.path());
        let records = registry.load().unwrap();
        assert!(records[0].auto_checkin_enabled);
    }

    #[test]
    fn old_registry_files_default_display_name_and_mobile() {
        // U-1 旧文件兼容：无 display_name/masked_mobile 字段的 accounts.json
        // 正常加载（None / 空串），升级后无需迁移即可继续使用。
        let root = tempdir().unwrap();
        let path = root.path().join("accounts.json");
        std::fs::write(
            &path,
            r#"{"format_version":1,"accounts":[{"profile_id":"legacy","account_id":"a1","screen_name":"旧账号","avatar_url":"","device_id":"1234567890123456","device_public_key":"k","created_at_unix_seconds":0,"last_verified_at_unix_seconds":0,"auto_checkin_enabled":true}]}"#,
        )
        .unwrap();
        let registry = AccountRegistry::new(root.path());
        let records = registry.load().unwrap();
        assert_eq!(records[0].display_name, None);
        assert_eq!(records[0].masked_mobile, "");
    }

    #[test]
    fn set_display_name_set_clear_and_idempotent() {
        let root = tempdir().unwrap();
        let registry = AccountRegistry::new(root.path());
        registry.upsert(&sample_record("profile-a")).unwrap();
        // 设置：trim 后非空才生效。
        assert!(registry.set_display_name("profile-a", Some("  主力号 ")).unwrap());
        assert_eq!(
            registry.find("profile-a").unwrap().unwrap().display_name.as_deref(),
            Some("主力号")
        );
        // 同值幂等；空串等价清除。
        assert!(registry.set_display_name("profile-a", Some("主力号")).unwrap());
        assert!(registry.set_display_name("profile-a", Some("   ")).unwrap());
        assert_eq!(
            registry.find("profile-a").unwrap().unwrap().display_name,
            None
        );
        // 不存在的账号返回 false。
        assert!(!registry.set_display_name("profile-404", Some("x")).unwrap());
    }

    #[test]
    fn old_registry_files_default_archived_off_and_roundtrip() {
        // U-6 W4 旧文件兼容：无 archived 字段的 accounts.json 正常加载（false）；
        // 归档后新 JSON 往返保留 true（serde 序列化字段不丢）。
        let root = tempdir().unwrap();
        let path = root.path().join("accounts.json");
        std::fs::write(
            &path,
            r#"{"format_version":1,"accounts":[{"profile_id":"legacy","account_id":"a1","screen_name":"旧账号","avatar_url":"","device_id":"1234567890123456","device_public_key":"k","created_at_unix_seconds":0,"last_verified_at_unix_seconds":0,"auto_checkin_enabled":true}]}"#,
        )
        .unwrap();
        let registry = AccountRegistry::new(root.path());
        assert!(!registry.load().unwrap()[0].archived);

        registry.set_archived("legacy", true).unwrap();
        let reloaded = registry.load().unwrap();
        assert!(reloaded[0].archived);
        // 落盘 JSON 含归档字段（新格式），再读往返一致。
        let json = std::fs::read_to_string(&path).unwrap();
        assert!(json.contains("\"archived\": true"));
    }

    #[test]
    fn set_archived_persists_and_is_idempotent() {
        let root = tempdir().unwrap();
        let registry = AccountRegistry::new(root.path());
        registry.upsert(&sample_record("profile-a")).unwrap();
        // 归档持久化：写入后重新 load 仍为 true。
        assert!(registry.set_archived("profile-a", true).unwrap());
        assert!(registry.find("profile-a").unwrap().unwrap().archived);
        // 幂等：同值再写不报错；取消归档回 false。
        assert!(registry.set_archived("profile-a", true).unwrap());
        assert!(registry.set_archived("profile-a", false).unwrap());
        assert!(!registry.find("profile-a").unwrap().unwrap().archived);
        // 不存在的账号返回 false。
        assert!(!registry.set_archived("profile-404", true).unwrap());
    }

    #[test]
    fn backfill_masked_mobile_first_write_only() {
        let root = tempdir().unwrap();
        let registry = AccountRegistry::new(root.path());
        registry.upsert(&sample_record("profile-a")).unwrap();
        // 首采：空值 + 非空入参 -> 写入。
        assert!(registry.backfill_masked_mobile("profile-a", "138****0000").unwrap());
        assert_eq!(
            registry.find("profile-a").unwrap().unwrap().masked_mobile,
            "138****0000"
        );
        // 已有值不覆盖（首采语义）；空入参不写入。
        assert!(registry.backfill_masked_mobile("profile-a", "139****9999").unwrap());
        assert_eq!(
            registry.find("profile-a").unwrap().unwrap().masked_mobile,
            "138****0000"
        );
        assert!(registry.backfill_masked_mobile("profile-a", "").unwrap());
    }

    #[test]
    fn empty_display_name_some_is_invalid() {
        // 不变式：Some 必须非空（空别名应在写入前归一化为 None）。
        let mut record = sample_record("profile-a");
        record.display_name = Some(String::new());
        assert_eq!(record.validate(), Err(AccountRegistryError::InvalidRecord));
    }

    #[test]
    fn corrupt_file_fails_closed() {
        let root = tempdir().unwrap();
        let registry = AccountRegistry::new(root.path());
        registry.upsert(&sample_record("profile-a")).unwrap();
        fs::write(registry_path(root.path()), "not json").unwrap();
        assert_eq!(registry.load().err(), Some(AccountRegistryError::Invalid));
    }

    #[test]
    fn invalid_record_is_rejected() {
        let mut record = sample_record("profile-a");
        record.account_id = String::new();
        assert_eq!(record.validate(), Err(AccountRegistryError::InvalidRecord));
    }
}
