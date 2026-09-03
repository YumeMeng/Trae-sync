//! 环境注册表（P5-0 建立，P6-4 泛化为多环境档案）。
//!
//! 环境模型（ADR-0024）：环境 = 独立 TRAE 数据目录 + 当前登录账号，
//! 账号 ↔ 环境多对多。主库环境绑定官方目录（`%APPDATA%\TRAE SOLO CN`，
//! 2026-08-31 用户裁定），默认、不可删除、不可改名；副环境 data_dir
//! 固定为 `{storage_root}\environments\{env_id}`（派生规则，不落盘，
//! 避免 storage_root 迁移后绝对路径漂移）。
//!
//! 档案持久化「当前登录账号」——切号/环境登录流程完成后写回，App 重启
//! 后据此恢复环境页与窗口标题。档案损坏一律报错拒绝，不静默重建。
//!
//! V2 文件格式（format_version = 2）：
//! ```json
//! { "format_version": 2, "environments": [ { "env_id", "name", ... } ] }
//! ```
//! V1 单环境格式（`master` 字段）读取时升级为 V2 内存形态，首次写回
//! 落盘为 V2（单向迁移，不回写 V1）。

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::atomic_publish::publish_replacing;

const REGISTRY_FILE: &str = "environments.json";
const MAX_REGISTRY_BYTES: u64 = 256 * 1024;
const MAX_TEXT_FIELD_BYTES: usize = 1024;
/// 环境名称上限（字符数；与账号备注名 display_name 同口径）。
const MAX_ENV_NAME_CHARS: usize = 64;
/// 环境名称保留字（主库名不可被副环境占用）。
const MASTER_ENV_NAME: &str = "主库";

/// V1 唯一环境标识（主库，Q7：默认环境，不可删除/不可修改类型）。
pub const MASTER_ENV_ID: &str = "master";

/// 环境注册表错误；不携带敏感内容。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EnvironmentRegistryError {
    /// 文件损坏或格式不符。
    Invalid,
    /// 记录字段非法（空/超长/重复/非法标识）。
    InvalidRecord,
    /// 目标环境不存在。
    NotFound,
    /// 环境名称已被其他环境占用。
    NameTaken,
    /// 主库环境不可改名/删除。
    MasterImmutable,
    /// 读写失败。
    Io,
}

/// 环境档案（V2：多环境）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnvironmentRecord {
    /// 环境标识：`master` 或 `env-<时间戳>-<随机后缀>`（副环境）。
    pub env_id: String,
    /// 环境名称（用户可见；主库固定「主库」）。
    #[serde(default = "default_master_env_name")]
    pub name: String,
    /// 当前登录该环境的账号 profile_id；None = 尚未登录任何账号。
    #[serde(default)]
    pub current_profile_id: Option<String>,
    pub created_at_unix_seconds: u64,
}

fn default_master_env_name() -> String {
    MASTER_ENV_NAME.to_string()
}

impl EnvironmentRecord {
    /// 是否主库环境。
    pub fn is_master(&self) -> bool {
        self.env_id == MASTER_ENV_ID
    }

    /// 校验字段：env_id 形态、名称非空且不超长、profile_id 非空且不超长、
    /// 时间戳非零。主库名固定；副环境不得占用「主库」名。
    pub fn validate(&self) -> Result<(), EnvironmentRegistryError> {
        if self.env_id == MASTER_ENV_ID {
            if self.name != MASTER_ENV_NAME {
                return Err(EnvironmentRegistryError::MasterImmutable);
            }
        } else if !is_valid_secondary_env_id(&self.env_id) {
            return Err(EnvironmentRegistryError::InvalidRecord);
        }
        let name = self.name.trim();
        if name.is_empty() || self.name.chars().count() > MAX_ENV_NAME_CHARS {
            return Err(EnvironmentRegistryError::InvalidRecord);
        }
        if !self.is_master() && name == MASTER_ENV_NAME {
            return Err(EnvironmentRegistryError::InvalidRecord);
        }
        if let Some(profile_id) = &self.current_profile_id {
            if profile_id.is_empty() || profile_id.len() > MAX_TEXT_FIELD_BYTES {
                return Err(EnvironmentRegistryError::InvalidRecord);
            }
        }
        if self.created_at_unix_seconds == 0 {
            return Err(EnvironmentRegistryError::InvalidRecord);
        }
        Ok(())
    }
}

/// 副环境 env_id 形态：`env-` 前缀 + 小写字母数字与连字符，总长 ≤ 64
/// （作为 data_dir 目录名使用，仅允许文件系统安全字符）。
fn is_valid_secondary_env_id(env_id: &str) -> bool {
    env_id.len() <= MAX_TEXT_FIELD_BYTES
        && env_id.starts_with("env-")
        && env_id[4..]
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

#[derive(Serialize, Deserialize)]
struct RegistryFile {
    format_version: u32,
    #[serde(default)]
    environments: Vec<EnvironmentRecord>,
}

/// 环境注册表（storage_root 下多环境 JSON 文件）。
pub struct EnvironmentRegistry {
    path: PathBuf,
}

/// 主库 data_dir：官方 TRAE Work CN 默认数据目录。
///
/// 2026-08-31 架构修订：主库 = 官方启动的环境（`%APPDATA%\TRAE SOLO CN`，
/// 与 `work_cn_location` 固定发现路径同源）。日常从官方快捷方式启动即使用
/// 主库，App 负责切号（关实例 → 备份 → 凭据互换 → 记录交接 → 重启）。
/// APPDATA 环境变量缺失时报 Io（fail-closed，不猜路径）。
pub fn master_data_dir() -> Result<PathBuf, EnvironmentRegistryError> {
    let appdata = std::env::var("APPDATA").map_err(|_| EnvironmentRegistryError::Io)?;
    Ok(official_data_dir_from_appdata(Path::new(&appdata)))
}

/// 纯函数形态（单测用）：由 APPDATA 根拼出官方目录。
fn official_data_dir_from_appdata(appdata: &Path) -> PathBuf {
    appdata.join(crate::work_cn_location::DEFAULT_WORK_CN_ROOT_NAME)
}

/// 副环境 data_dir：`{storage_root}\environments\{env_id}`（P6-4 固定规则，
/// 派生而非落盘——storage_root 迁移时随注册表文件整体走，不产生路径漂移）。
pub fn secondary_data_dir(storage_root: &Path, env_id: &str) -> PathBuf {
    storage_root.join("environments").join(env_id)
}

impl EnvironmentRegistry {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            path: root.into().join(REGISTRY_FILE),
        }
    }

    /// 读取全部环境档案；文件不存在返回空表（首次使用前无需预创建）。
    /// V1 单环境格式读取时升级为 V2 内存形态（master 档案照搬）。
    pub fn load_all(&self) -> Result<Vec<EnvironmentRecord>, EnvironmentRegistryError> {
        let metadata = match fs::symlink_metadata(&self.path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(_) => return Err(EnvironmentRegistryError::Io),
        };
        if !metadata.is_file() || metadata.len() > MAX_REGISTRY_BYTES {
            return Err(EnvironmentRegistryError::Invalid);
        }
        let content = fs::read_to_string(&self.path).map_err(|_| EnvironmentRegistryError::Io)?;
        // 先按无标签 JSON 解析，再按 format_version 分发 V1/V2 形态。
        let raw: serde_json::Value =
            serde_json::from_str(&content).map_err(|_| EnvironmentRegistryError::Invalid)?;
        let format_version = raw
            .get("format_version")
            .and_then(|value| value.as_u64())
            .ok_or(EnvironmentRegistryError::Invalid)?;
        let environments = match format_version {
            1 => {
                // V1：单 master 档案（无 name 字段，serde 默认补「主库」）。
                let record: EnvironmentRecord = serde_json::from_value(
                    raw.get("master")
                        .cloned()
                        .ok_or(EnvironmentRegistryError::Invalid)?,
                )
                .map_err(|_| EnvironmentRegistryError::Invalid)?;
                vec![record]
            }
            2 => {
                let file: RegistryFile =
                    serde_json::from_value(raw).map_err(|_| EnvironmentRegistryError::Invalid)?;
                file.environments
            }
            _ => return Err(EnvironmentRegistryError::Invalid),
        };
        validate_records(&environments)?;
        Ok(environments)
    }

    /// 读取主库档案（V1 兼容入口；不存在返回 None）。
    pub fn load(&self) -> Result<Option<EnvironmentRecord>, EnvironmentRegistryError> {
        Ok(self.load_all()?.into_iter().find(|record| record.is_master()))
    }

    /// 确保主库档案在位：缺失则以当前时间建档（幂等；已存在时不变，
    /// 其余副环境档案保持原样）。
    pub fn ensure_master(&self) -> Result<EnvironmentRecord, EnvironmentRegistryError> {
        let mut environments = self.load_all()?;
        if let Some(record) = environments.iter().find(|record| record.is_master()) {
            return Ok(record.clone());
        }
        let record = EnvironmentRecord {
            env_id: MASTER_ENV_ID.to_string(),
            name: MASTER_ENV_NAME.to_string(),
            current_profile_id: None,
            created_at_unix_seconds: current_unix_seconds(),
        };
        environments.push(record.clone());
        self.write(&environments)?;
        Ok(record)
    }

    /// 按 env_id 查找档案。
    pub fn find(&self, env_id: &str) -> Result<Option<EnvironmentRecord>, EnvironmentRegistryError> {
        Ok(self
            .load_all()?
            .into_iter()
            .find(|record| record.env_id == env_id))
    }

    /// 创建副环境（P6-4 生命周期：登记档案；data_dir 由调用方按派生规则
    /// 创建）。名称非法或被占用（含「主库」保留名）时报错。
    pub fn create(&self, name: &str) -> Result<EnvironmentRecord, EnvironmentRegistryError> {
        let name = name.trim();
        if name.is_empty() || name.chars().count() > MAX_ENV_NAME_CHARS {
            return Err(EnvironmentRegistryError::InvalidRecord);
        }
        // 保留名先于重名检查（语义区分：保留名非法 vs 普通重名占用）。
        if name == MASTER_ENV_NAME {
            return Err(EnvironmentRegistryError::InvalidRecord);
        }
        let mut environments = self.load_all()?;
        if environments
            .iter()
            .any(|record| record.name.trim() == name)
        {
            return Err(EnvironmentRegistryError::NameTaken);
        }
        let record = EnvironmentRecord {
            env_id: self.generate_env_id(&environments)?,
            name: name.to_string(),
            current_profile_id: None,
            created_at_unix_seconds: current_unix_seconds(),
        };
        environments.push(record.clone());
        self.write(&environments)?;
        Ok(record)
    }

    /// 重命名副环境（主库不可改名）。
    pub fn rename(
        &self,
        env_id: &str,
        name: &str,
    ) -> Result<EnvironmentRecord, EnvironmentRegistryError> {
        if env_id == MASTER_ENV_ID {
            return Err(EnvironmentRegistryError::MasterImmutable);
        }
        let name = name.trim();
        if name.is_empty() || name.chars().count() > MAX_ENV_NAME_CHARS {
            return Err(EnvironmentRegistryError::InvalidRecord);
        }
        // 保留名不可用于副环境（与 create 同口径）。
        if name == MASTER_ENV_NAME {
            return Err(EnvironmentRegistryError::InvalidRecord);
        }
        let mut environments = self.load_all()?;
        let index = environments
            .iter()
            .position(|record| record.env_id == env_id)
            .ok_or(EnvironmentRegistryError::NotFound)?;
        if environments
            .iter()
            .any(|other| other.env_id != env_id && other.name.trim() == name)
        {
            return Err(EnvironmentRegistryError::NameTaken);
        }
        environments[index].name = name.to_string();
        let updated = environments[index].clone();
        self.write(&environments)?;
        Ok(updated)
    }

    /// 删除副环境档案（主库不可删除）。返回被删除的档案，供调用方清理
    /// 对应 data_dir（档案与目录的删除在命令层绑定执行）。
    pub fn remove(&self, env_id: &str) -> Result<EnvironmentRecord, EnvironmentRegistryError> {
        if env_id == MASTER_ENV_ID {
            return Err(EnvironmentRegistryError::MasterImmutable);
        }
        let mut environments = self.load_all()?;
        let index = environments
            .iter()
            .position(|record| record.env_id == env_id)
            .ok_or(EnvironmentRegistryError::NotFound)?;
        let removed = environments.remove(index);
        self.write(&environments)?;
        Ok(removed)
    }

    /// 写回环境当前登录账号（切号/环境登录完成后的持久化点；
    /// 档案缺失时自动建档）。同值跳过写入（幂等），返回是否发生落盘。
    pub fn set_current_profile(
        &self,
        env_id: &str,
        profile_id: &str,
    ) -> Result<bool, EnvironmentRegistryError> {
        let profile_id = profile_id.trim();
        if profile_id.is_empty() || profile_id.len() > MAX_TEXT_FIELD_BYTES {
            return Err(EnvironmentRegistryError::InvalidRecord);
        }
        let mut environments = self.load_all()?;
        let index = match environments
            .iter()
            .position(|record| record.env_id == env_id)
        {
            Some(index) => index,
            // 主库档案缺失时自动建档（兼容旧调用点）；副环境必须先创建。
            None if env_id == MASTER_ENV_ID => {
                self.ensure_master()?;
                return self.set_current_profile(env_id, profile_id);
            }
            None => return Err(EnvironmentRegistryError::NotFound),
        };
        if environments[index].current_profile_id.as_deref() == Some(profile_id) {
            return Ok(false);
        }
        environments[index].current_profile_id = Some(profile_id.to_string());
        self.write(&environments)?;
        Ok(true)
    }

    /// 生成不与现有档案冲突的副环境 env_id：`env-{秒级时间戳}-{随机后缀}`。
    /// 随机源为单调时钟纳秒散列（无外部 rand 依赖；冲突时重试）。
    fn generate_env_id(
        &self,
        environments: &[EnvironmentRecord],
    ) -> Result<String, EnvironmentRegistryError> {
        for _ in 0..8 {
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|duration| duration.as_nanos())
                .unwrap_or(0);
            // 简易散列混合（splitmix64 风格），取低 16 位十六进制作后缀。
            let mut mixed = (nanos as u64).wrapping_add(0x9E3779B97F4A7C15);
            mixed = (mixed ^ (mixed >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
            mixed = (mixed ^ (mixed >> 27)).wrapping_mul(0x94D049BB133111EB);
            mixed ^= mixed >> 31;
            let candidate = format!(
                "env-{}-{:x}",
                current_unix_seconds(),
                mixed & 0xFFFF
            );
            if !environments
                .iter()
                .any(|record| record.env_id == candidate)
            {
                return Ok(candidate);
            }
        }
        Err(EnvironmentRegistryError::Io)
    }

    /// 原子写入：失败时保留原文件。全部记录整体校验后落盘。
    fn write(&self, environments: &[EnvironmentRecord]) -> Result<(), EnvironmentRegistryError> {
        validate_records(environments)?;
        let registry = RegistryFile {
            format_version: 2,
            environments: environments.to_vec(),
        };
        let content = serde_json::to_string_pretty(&registry)
            .map_err(|_| EnvironmentRegistryError::Invalid)?;
        let bytes = content.as_bytes();
        if bytes.len() as u64 > MAX_REGISTRY_BYTES {
            return Err(EnvironmentRegistryError::Invalid);
        }
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent).map_err(|_| EnvironmentRegistryError::Io)?;
        }
        let temporary = self.path.with_extension("json.tmp");
        fs::write(&temporary, bytes).map_err(|_| EnvironmentRegistryError::Io)?;
        publish_replacing(&temporary, &self.path).map_err(|_| EnvironmentRegistryError::Io)
    }
}

/// 整表校验：每条记录字段合法 + env_id / 名称不重复。
fn validate_records(environments: &[EnvironmentRecord]) -> Result<(), EnvironmentRegistryError> {
    for record in environments {
        record.validate()?;
    }
    let mut seen_ids = std::collections::HashSet::new();
    let mut seen_names = std::collections::HashSet::new();
    for record in environments {
        if !seen_ids.insert(record.env_id.as_str()) {
            return Err(EnvironmentRegistryError::InvalidRecord);
        }
        // 名称唯一性以主库优先：主库名固定，副环境不得重名（含与主库）。
        if !seen_names.insert(record.name.trim()) {
            return Err(EnvironmentRegistryError::InvalidRecord);
        }
    }
    Ok(())
}

fn current_unix_seconds() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn master_data_dir_shape() {
        // 官方目录 = APPDATA 下 TRAE SOLO CN（与 work_cn_location 固定发现同源）。
        let dir = official_data_dir_from_appdata(Path::new("C:/Users/u/AppData/Roaming"));
        assert_eq!(dir, Path::new("C:/Users/u/AppData/Roaming/TRAE SOLO CN"));
    }

    #[test]
    fn secondary_data_dir_shape() {
        // 副环境目录 = storage_root/environments/{env_id}（P6-4 固定规则）。
        let dir = secondary_data_dir(Path::new("D:/store"), "env-123-abcd");
        assert_eq!(dir, Path::new("D:/store/environments/env-123-abcd"));
    }

    #[test]
    fn load_missing_file_returns_empty() {
        let root = tempdir().unwrap();
        let registry = EnvironmentRegistry::new(root.path());
        assert!(registry.load_all().unwrap().is_empty());
        assert_eq!(registry.load().unwrap(), None);
    }

    #[test]
    fn ensure_master_creates_then_idempotent() {
        let root = tempdir().unwrap();
        let registry = EnvironmentRegistry::new(root.path());
        let first = registry.ensure_master().unwrap();
        assert_eq!(first.env_id, MASTER_ENV_ID);
        assert_eq!(first.name, "主库");
        assert_eq!(first.current_profile_id, None);
        // 二次读取不再建档，created_at 保持不变（幂等）。
        let second = registry.ensure_master().unwrap();
        assert_eq!(first, second);
        // 落盘可恢复（App 重启后环境档案不丢）。
        let reloaded = EnvironmentRegistry::new(root.path()).load().unwrap();
        assert_eq!(reloaded, Some(first));
    }

    #[test]
    fn v1_registry_upgrades_in_memory() {
        let root = tempdir().unwrap();
        let registry = EnvironmentRegistry::new(root.path());
        // V1 单环境格式（无 name 字段）：读取升级为 V2 形态。
        std::fs::write(
            registry_path(root.path()),
            r#"{"format_version":1,"master":{"env_id":"master","current_profile_id":"checkin-abc","created_at_unix_seconds":100}}"#,
        )
        .unwrap();
        let environments = registry.load_all().unwrap();
        assert_eq!(environments.len(), 1);
        assert_eq!(environments[0].name, "主库");
        assert_eq!(environments[0].current_profile_id.as_deref(), Some("checkin-abc"));
        // V1 档案的写入操作落盘为 V2 格式（单向迁移）。
        registry.set_current_profile(MASTER_ENV_ID, "checkin-def").unwrap();
        let content = std::fs::read_to_string(registry_path(root.path())).unwrap();
        assert!(content.contains("\"format_version\": 2"));
        assert!(content.contains("\"environments\""));
        // 迁移后可再创建副环境（同表共存）。
        let created = registry.create("工作环境").unwrap();
        assert_eq!(
            EnvironmentRegistry::new(root.path())
                .find(&created.env_id)
                .unwrap()
                .unwrap()
                .name,
            "工作环境"
        );
    }

    #[test]
    fn create_rename_remove_lifecycle() {
        let root = tempdir().unwrap();
        let registry = EnvironmentRegistry::new(root.path());
        registry.ensure_master().unwrap();

        // 创建：档案登记 + env_id 形态合法。
        let created = registry.create("测试环境").unwrap();
        assert!(created.env_id.starts_with("env-"));
        assert_eq!(created.name, "测试环境");
        assert_eq!(created.current_profile_id, None);
        // 落盘可恢复。
        let reloaded = EnvironmentRegistry::new(root.path())
            .find(&created.env_id)
            .unwrap()
            .unwrap();
        assert_eq!(reloaded, created);

        // 重命名：档案更新；旧名释放。
        let renamed = registry.rename(&created.env_id, "新名字").unwrap();
        assert_eq!(renamed.name, "新名字");
        assert!(registry.create("测试环境").is_ok());

        // 删除：档案行移除；主库不可删。
        let removed = registry.remove(&created.env_id).unwrap();
        assert_eq!(removed.env_id, created.env_id);
        assert_eq!(
            registry.remove(&created.env_id).unwrap_err(),
            EnvironmentRegistryError::NotFound
        );
        assert_eq!(
            registry.remove(MASTER_ENV_ID).unwrap_err(),
            EnvironmentRegistryError::MasterImmutable
        );
    }

    #[test]
    fn create_rejects_invalid_and_duplicate_names() {
        let root = tempdir().unwrap();
        let registry = EnvironmentRegistry::new(root.path());
        registry.ensure_master().unwrap();
        // 空名 / 超长名。
        assert_eq!(
            registry.create("   ").unwrap_err(),
            EnvironmentRegistryError::InvalidRecord
        );
        assert_eq!(
            registry.create(&"长".repeat(MAX_ENV_NAME_CHARS + 1)).unwrap_err(),
            EnvironmentRegistryError::InvalidRecord
        );
        // 保留名「主库」与重名。
        assert_eq!(
            registry.create("主库").unwrap_err(),
            EnvironmentRegistryError::InvalidRecord
        );
        registry.create("唯一名").unwrap();
        assert_eq!(
            registry.create("唯一名").unwrap_err(),
            EnvironmentRegistryError::NameTaken
        );
    }

    #[test]
    fn rename_rejects_master_and_duplicates() {
        let root = tempdir().unwrap();
        let registry = EnvironmentRegistry::new(root.path());
        registry.ensure_master().unwrap();
        let a = registry.create("环境A").unwrap();
        let b = registry.create("环境B").unwrap();
        // 主库不可改名。
        assert_eq!(
            registry.rename(MASTER_ENV_ID, "别名").unwrap_err(),
            EnvironmentRegistryError::MasterImmutable
        );
        // 重名拒绝（环境B 改成环境A 的名字）。
        assert_eq!(
            registry.rename(&b.env_id, "环境A").unwrap_err(),
            EnvironmentRegistryError::NameTaken
        );
        // 不存在的环境。
        assert_eq!(
            registry.rename("env-none", "任意").unwrap_err(),
            EnvironmentRegistryError::NotFound
        );
        // 改回自己的当前名：幂等允许（无变化）。
        assert!(registry.rename(&a.env_id, "环境A").is_ok());
    }

    #[test]
    fn set_current_profile_persists_and_is_idempotent() {
        let root = tempdir().unwrap();
        let registry = EnvironmentRegistry::new(root.path());
        assert!(registry
            .set_current_profile(MASTER_ENV_ID, "checkin-abc")
            .unwrap());
        // 同值跳过写入。
        assert!(!registry
            .set_current_profile(MASTER_ENV_ID, "checkin-abc")
            .unwrap());
        // 换账号写回并跨实例恢复（切号语义）。
        assert!(registry
            .set_current_profile(MASTER_ENV_ID, "checkin-def")
            .unwrap());
        let reloaded = EnvironmentRegistry::new(root.path())
            .load()
            .unwrap()
            .unwrap();
        assert_eq!(reloaded.current_profile_id.as_deref(), Some("checkin-def"));

        // 副环境：登录账号写回该环境档案行（多对多：各环境各自记忆）。
        let env = registry.create("副环境").unwrap();
        assert!(registry.set_current_profile(&env.env_id, "checkin-xyz").unwrap());
        let all = EnvironmentRegistry::new(root.path()).load_all().unwrap();
        assert_eq!(
            all.iter()
                .find(|record| record.env_id == env.env_id)
                .unwrap()
                .current_profile_id
                .as_deref(),
            Some("checkin-xyz")
        );
        // 主库档案不受副环境登录影响。
        assert_eq!(
            all.iter()
                .find(|record| record.is_master())
                .unwrap()
                .current_profile_id
                .as_deref(),
            Some("checkin-def")
        );
        // 不存在的副环境写回报 NotFound。
        assert_eq!(
            registry.set_current_profile("env-none", "checkin-abc").unwrap_err(),
            EnvironmentRegistryError::NotFound
        );
    }

    #[test]
    fn set_current_profile_rejects_blank() {
        let root = tempdir().unwrap();
        let registry = EnvironmentRegistry::new(root.path());
        assert_eq!(
            registry.set_current_profile(MASTER_ENV_ID, "   ").unwrap_err(),
            EnvironmentRegistryError::InvalidRecord
        );
    }

    #[test]
    fn invalid_file_rejected() {
        let root = tempdir().unwrap();
        let registry = EnvironmentRegistry::new(root.path());
        // 坏 JSON。
        std::fs::write(registry_path(root.path()), "not json").unwrap();
        assert_eq!(
            registry.load_all().unwrap_err(),
            EnvironmentRegistryError::Invalid
        );
        // 版本不符（未来的 V3 也不倒灌）。
        std::fs::write(
            registry_path(root.path()),
            r#"{"format_version":3,"environments":[]}"#,
        )
        .unwrap();
        assert_eq!(
            registry.load_all().unwrap_err(),
            EnvironmentRegistryError::Invalid
        );
        // V2 格式倒灌防护：非主库标识占用 env_id "master" 之外的形态非法
        // （如非法字符、缺前缀）。
        std::fs::write(
            registry_path(root.path()),
            r#"{"format_version":2,"environments":[{"env_id":"bad id!","name":"X","current_profile_id":null,"created_at_unix_seconds":1}]}"#,
        )
        .unwrap();
        assert_eq!(
            registry.load_all().unwrap_err(),
            EnvironmentRegistryError::InvalidRecord
        );
        // 环境名重复整表拒绝。
        std::fs::write(
            registry_path(root.path()),
            r#"{"format_version":2,"environments":[{"env_id":"master","name":"主库","current_profile_id":null,"created_at_unix_seconds":1},{"env_id":"env-1-a","name":"主库","current_profile_id":null,"created_at_unix_seconds":2}]}"#,
        )
        .unwrap();
        assert_eq!(
            registry.load_all().unwrap_err(),
            EnvironmentRegistryError::InvalidRecord
        );
        // 损坏文件不被静默覆盖：写回操作报错而非重建。
        std::fs::write(registry_path(root.path()), "not json").unwrap();
        assert_eq!(
            registry.set_current_profile(MASTER_ENV_ID, "checkin-abc"),
            Err(EnvironmentRegistryError::Invalid)
        );
    }

    /// 注册表文件路径（测试辅助）。
    fn registry_path(root: &Path) -> PathBuf {
        root.join(REGISTRY_FILE)
    }
}
