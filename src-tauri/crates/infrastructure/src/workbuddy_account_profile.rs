//! P19.2① WorkBuddy 账号档案存储（工具侧 pool 旁表；fe 详情页「档案管理」
//! 数据源）。
//!
//! 数据源核查定论：客户端 `.info` 会话文件的 nickname/phoneNumber 为
//! `$wbEncrypted` 信封（工具纪律不解析敏感信封），且客户端无「用户备注名/
//! 归档」概念——两者均为**工具侧账号级属性**，落本旁表（与登录代次容器
//! `.wbsess` 同根存放，独立文件）。
//!
//! 与池容器（整代次，capture 重捕=整代次原子替换）的关系：本表是**账号级**
//! 状态，capture/switch 只读写 `.wbsess` 代次容器，永不触碰本表——备注名
//! 与归档标记跨重捕/切号天然存续（「capture/switch 不破坏」纪律由存储
//! 分层保证，无需代次字段透传）。
//!
//! 纪律（与 workbuddy_login_credential 同构）：tmp+rename 原子写 + 写后
//! 复核；文件缺失视为空表（不阻塞首次写入）；损坏即报错不猜测（Invalid）。
//! 归档账号的列表过滤由消费方执行（本表只存标记，不裁剪数据面）。

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// 档案存储文件名（workbuddy-login 根下，与 wb-*.wbsess 并列）。
pub const ACCOUNT_PROFILES_FILE: &str = "workbuddy-account-profiles.json";
/// 明文 JSON 形态版本。
pub const ACCOUNT_PROFILES_FORMAT_VERSION: u32 = 1;
/// 单文件上限（防异常膨胀；正常记录极小）。
const MAX_PAYLOAD_BYTES: u64 = 256 * 1024;
/// 备注名长度上限（防御异常输入；UI 侧同步约束）。
const MAX_DISPLAY_NAME_CHARS: usize = 100;

/// 档案存储错误（不含任何凭据材料）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkBuddyAccountProfileError {
    /// 文件系统 IO 失败（原子写/读/建目录）。
    Io,
    /// 文件损坏：JSON 非法/超限/形态不符。
    Invalid,
    /// 备注名超限。
    NameTooLong,
}

impl WorkBuddyAccountProfileError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::Io => "workbuddy_account_profile_io",
            Self::Invalid => "workbuddy_account_profile_invalid",
            Self::NameTooLong => "workbuddy_account_profile_name_too_long",
        }
    }
}

/// 单账号档案条目（账号级状态；不含凭据材料）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct WorkBuddyAccountProfile {
    /// 用户备注名（空串 = 未设置；展示优先级：备注名 > 捕获昵称 > 兜底标签）。
    #[serde(default)]
    pub display_name: String,
    /// 归档标记（归档账号列表过滤的依据；默认 false = 在册）。
    #[serde(default)]
    pub archived: bool,
    /// 最近一次档案更新时刻（Unix 秒）。
    #[serde(default)]
    pub updated_at_unix_seconds: u64,
}

#[derive(Serialize, Deserialize)]
struct SerializableProfiles {
    format_version: u32,
    /// account_id → 档案条目（BTreeMap 保证键序稳定，diff 友好）。
    profiles: BTreeMap<String, WorkBuddyAccountProfile>,
}

/// 账号档案存储（pool 旁表；线程不安全假设同其余存储——命令层串行调用）。
pub struct WorkBuddyAccountProfileStore {
    root: PathBuf,
}

impl WorkBuddyAccountProfileStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// 存储文件路径。
    fn path(&self) -> PathBuf {
        self.root.join(ACCOUNT_PROFILES_FILE)
    }

    /// 读取全量档案（文件缺失 = 空表；损坏 = Invalid，不猜测）。
    pub fn load_all(&self) -> Result<BTreeMap<String, WorkBuddyAccountProfile>, WorkBuddyAccountProfileError> {
        let content = match fs::read_to_string(self.path()) {
            Ok(content) => content,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(BTreeMap::new());
            }
            Err(_) => return Err(WorkBuddyAccountProfileError::Io),
        };
        let parsed: SerializableProfiles = serde_json::from_str(&content)
            .map_err(|_| WorkBuddyAccountProfileError::Invalid)?;
        if parsed.format_version != ACCOUNT_PROFILES_FORMAT_VERSION {
            return Err(WorkBuddyAccountProfileError::Invalid);
        }
        Ok(parsed.profiles)
    }

    /// 读取单账号档案（无记录 = 缺省条目）。
    pub fn get(&self, account_id: &str) -> Result<WorkBuddyAccountProfile, WorkBuddyAccountProfileError> {
        Ok(self
            .load_all()?
            .get(account_id)
            .cloned()
            .unwrap_or_default())
    }

    /// 更新单账号档案条目（存在则保留未提及字段语义由调用方组装全量条目；
    /// 本 API 语义 = 读改写单字段组）。原子写 + 写后复核。
    fn update(
        &self,
        account_id: &str,
        mutate: impl FnOnce(&mut WorkBuddyAccountProfile),
        now_unix: u64,
    ) -> Result<WorkBuddyAccountProfile, WorkBuddyAccountProfileError> {
        if account_id.trim().is_empty() {
            return Err(WorkBuddyAccountProfileError::Invalid);
        }
        let mut profiles = self.load_all()?;
        let entry = profiles.entry(account_id.to_string()).or_default();
        mutate(entry);
        entry.updated_at_unix_seconds = now_unix;
        let snapshot = entry.clone();
        self.save_all(&profiles)?;
        Ok(snapshot)
    }

    /// 设置备注名（空串 = 清除备注；trim 后超限拒绝）。
    pub fn set_display_name(
        &self,
        account_id: &str,
        display_name: &str,
        now_unix: u64,
    ) -> Result<WorkBuddyAccountProfile, WorkBuddyAccountProfileError> {
        let trimmed = display_name.trim().to_string();
        if trimmed.chars().count() > MAX_DISPLAY_NAME_CHARS {
            return Err(WorkBuddyAccountProfileError::NameTooLong);
        }
        self.update(
            account_id,
            |entry| entry.display_name = trimmed.clone(),
            now_unix,
        )
    }

    /// 设置归档标记。
    pub fn set_archived(
        &self,
        account_id: &str,
        archived: bool,
        now_unix: u64,
    ) -> Result<WorkBuddyAccountProfile, WorkBuddyAccountProfileError> {
        self.update(account_id, |entry| entry.archived = archived, now_unix)
    }

    /// 批量移除档案条目（彻底删除账号的旁表清理；返回实际移除数）。
    pub fn remove(&self, account_ids: &[String]) -> Result<usize, WorkBuddyAccountProfileError> {
        if account_ids.is_empty() {
            return Ok(0);
        }
        let mut profiles = self.load_all()?;
        let mut removed = 0;
        for account_id in account_ids {
            if profiles.remove(account_id).is_some() {
                removed += 1;
            }
        }
        if removed > 0 {
            self.save_all(&profiles)?;
        }
        Ok(removed)
    }

    /// 原子写全量档案（tmp + rename + 写后复核；与池容器同纪律）。
    fn save_all(
        &self,
        profiles: &BTreeMap<String, WorkBuddyAccountProfile>,
    ) -> Result<(), WorkBuddyAccountProfileError> {
        let payload = SerializableProfiles {
            format_version: ACCOUNT_PROFILES_FORMAT_VERSION,
            profiles: profiles.clone(),
        };
        let content = serde_json::to_string_pretty(&payload)
            .map_err(|_| WorkBuddyAccountProfileError::Invalid)?;
        if content.len() as u64 > MAX_PAYLOAD_BYTES {
            return Err(WorkBuddyAccountProfileError::Invalid);
        }
        fs::create_dir_all(&self.root).map_err(|_| WorkBuddyAccountProfileError::Io)?;
        let path = self.path();
        let temp_path = path.with_extension("json.tmp");
        fs::write(&temp_path, &content).map_err(|_| WorkBuddyAccountProfileError::Io)?;
        // Windows rename 不能覆盖存在目标，先移除旧文件（写入窗口由
        // tmp 完整性兜底——与池容器 save 同纪律）。
        let _ = fs::remove_file(&path);
        fs::rename(&temp_path, &path).map_err(|_| WorkBuddyAccountProfileError::Io)?;
        // 写后复核：回读可解析且内容一致。
        let verify = fs::read_to_string(&path).map_err(|_| WorkBuddyAccountProfileError::Io)?;
        if verify != content {
            return Err(WorkBuddyAccountProfileError::Io);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store_in(dir: &Path) -> WorkBuddyAccountProfileStore {
        WorkBuddyAccountProfileStore::new(dir.to_path_buf())
    }

    /// 设置/读取往返：备注名与归档标记落盘、缺省条目语义。
    #[test]
    fn set_and_get_roundtrip() {
        let temp = tempfile::tempdir().unwrap();
        let store = store_in(temp.path());
        // 无记录 = 缺省条目（未设置备注、未归档）。
        assert_eq!(store.get("uid-1").unwrap(), WorkBuddyAccountProfile::default());
        let entry = store.set_display_name("uid-1", "我的主力号", 100).unwrap();
        assert_eq!(entry.display_name, "我的主力号");
        assert!(!entry.archived);
        assert_eq!(entry.updated_at_unix_seconds, 100);
        // 归档翻转保留备注名。
        let entry = store.set_archived("uid-1", true, 200).unwrap();
        assert_eq!(entry.display_name, "我的主力号");
        assert!(entry.archived);
        assert_eq!(entry.updated_at_unix_seconds, 200);
        // 重开存储实例（磁盘往返）。
        let reopened = store_in(temp.path());
        let entry = reopened.get("uid-1").unwrap();
        assert_eq!(entry.display_name, "我的主力号");
        assert!(entry.archived);
        // 清除备注（空串语义）。
        let entry = reopened.set_display_name("uid-1", "  ", 300).unwrap();
        assert_eq!(entry.display_name, "", "空白 trim 为清除备注");
        assert!(entry.archived, "清除备注不动归档标记");
    }

    /// trim + 超限拒绝 + 空 account_id 拒绝。
    #[test]
    fn display_name_validation() {
        let temp = tempfile::tempdir().unwrap();
        let store = store_in(temp.path());
        let entry = store.set_display_name("uid-1", "  备注名  ", 1).unwrap();
        assert_eq!(entry.display_name, "备注名", "trim");
        let long = "长".repeat(MAX_DISPLAY_NAME_CHARS + 1);
        assert_eq!(
            store.set_display_name("uid-1", &long, 1).unwrap_err(),
            WorkBuddyAccountProfileError::NameTooLong
        );
        assert_eq!(
            store.set_display_name("", "x", 1).unwrap_err(),
            WorkBuddyAccountProfileError::Invalid
        );
        assert_eq!(
            store.set_archived("", true, 1).unwrap_err(),
            WorkBuddyAccountProfileError::Invalid
        );
    }

    /// 批量移除：只删命中项；空输入零副作用；未命中不计入。
    #[test]
    fn remove_targets_and_reports_count() {
        let temp = tempfile::tempdir().unwrap();
        let store = store_in(temp.path());
        store.set_display_name("uid-1", "一", 1).unwrap();
        store.set_display_name("uid-2", "二", 1).unwrap();
        assert_eq!(
            store
                .remove(&["uid-1".to_string(), "uid-gone".to_string()])
                .unwrap(),
            1
        );
        assert_eq!(store.get("uid-1").unwrap(), WorkBuddyAccountProfile::default());
        assert_eq!(store.get("uid-2").unwrap().display_name, "二");
        // 空输入 = 零副作用（不触发写）。
        assert_eq!(store.remove(&[]).unwrap(), 0);
    }

    /// 损坏文件拒绝（Invalid，不猜测）与文件缺失=空表。
    #[test]
    fn corrupted_rejected_and_missing_is_empty() {
        let temp = tempfile::tempdir().unwrap();
        let store = store_in(temp.path());
        // 文件缺失 = 空表（首写前置态）。
        assert!(store.load_all().unwrap().is_empty());
        // 损坏 → Invalid。
        fs::create_dir_all(temp.path()).unwrap();
        fs::write(temp.path().join(ACCOUNT_PROFILES_FILE), "not json").unwrap();
        assert_eq!(
            store.load_all().unwrap_err(),
            WorkBuddyAccountProfileError::Invalid
        );
        // 形态版本不符 → Invalid。
        fs::write(
            temp.path().join(ACCOUNT_PROFILES_FILE),
            r#"{"format_version":99,"profiles":{}}"#,
        )
        .unwrap();
        assert_eq!(
            store.load_all().unwrap_err(),
            WorkBuddyAccountProfileError::Invalid
        );
    }

    /// 多账号互不串扰 + updated_at 语义（同条目多次更新取最新）。
    #[test]
    fn accounts_are_independent_and_touch_updates_timestamp() {
        let temp = tempfile::tempdir().unwrap();
        let store = store_in(temp.path());
        store.set_archived("uid-1", true, 10).unwrap();
        store.set_display_name("uid-2", "二", 20).unwrap();
        assert!(store.get("uid-1").unwrap().archived);
        assert!(!store.get("uid-2").unwrap().archived);
        assert_eq!(store.get("uid-2").unwrap().updated_at_unix_seconds, 20);
        store.set_display_name("uid-2", "二改", 30).unwrap();
        assert_eq!(store.get("uid-2").unwrap().updated_at_unix_seconds, 30);
    }
}
