//! P17-T4c②/P17-A1：WorkBuddy 登录代次容器（`{root}/{profile_id}.wbsess`）。
//!
//! 与 Trae 融合版容器（`fused_login_credential`，TSFUSED1）同构纪律、
//! 独立目录独立格式——跨产品凭据隔离（ADR-0029 产品分层；跨产品复制
//! 禁令继续有效）。
//!
//! **P17-A1 明文化（spec 10.3，用户拍板：个人工具明文存储，不做防御性
//! 措施）**：文件格式为**裸明文 JSON 记录**（`format_version = 2` 标记
//! 明文形态——选实现最简的裸 JSON，无需魔数头）；DPAPI 加密层移除。
//! 双格式兼容读取：新写入恒为明文 JSON；旧 `WBSESSN1` DPAPI 容器（
//! `magic(8) || version(u32 LE) || payload_len(u32 LE) || DPAPI 密文`）
//! 保留兼容读，经 [`WorkBuddyLoginStore::migrate_legacy_to_plain`] 幂等
//! 迁移后不再产生。
//!
//! 字段差异（vs FusedLoginRecord）：WorkBuddy 凭据无 ICDRS 设备三元组
//! （machine_id/device_public_key），新增 domain/session_state（官方接口
//! 调用上下文）；device_id 为 `~/.workbuddy/device-id`（机器级 UUID）。
//!
//! 纪律（与 fused 同构）：整代次原子替换（tmp + rename）+ 写后复核；
//! 读取校验 profile_id ↔ account_id 绑定；续期由 WorkBuddy 客户端
//! daemon 自动完成，工具只做整代次捕获落盘，不主动发起刷新请求。

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::key_wrapper::unprotect;
use traesync_ports::KeyWrapperError;

/// 旧 DPAPI 容器魔数（P17-A1 起仅作迁移窗口内的兼容读）。
pub const WORKBUDDY_LOGIN_MAGIC: &[u8; 8] = b"WBSESSN1";
/// 旧 DPAPI 容器格式版本。
pub const WORKBUDDY_LOGIN_FORMAT_VERSION: u32 = 1;
/// 明文 JSON 形态版本（P17-A1：2 = 裸明文 JSON 记录）。
pub const WORKBUDDY_LOGIN_PLAIN_FORMAT_VERSION: u32 = 2;
/// 单文件上限（防异常膨胀；正常记录约 1.5KB）。
const MAX_PAYLOAD_BYTES: u64 = 64 * 1024;
/// 旧容器头部长度：magic(8) + version(4) + payload_len(4)。
const LEGACY_HEADER_LEN: usize = 16;

/// 容器错误。变体不含令牌或文件内容（凭据不进错误链）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkBuddyLoginStoreError {
    /// 文件系统 IO 失败（原子写/读/建目录）。
    Io,
    /// 文件损坏：魔数/版本/长度不符、密文解不开、JSON 非法、超限。
    Invalid,
    /// 绑定不符：记录 profile_id 与期望账号不匹配（宁可不返回，不可给错）。
    BindingMismatch,
    /// 非 Windows 平台（DPAPI 不可用）。
    Unavailable,
}

impl std::fmt::Display for WorkBuddyLoginStoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let text = match self {
            Self::Io => "workbuddy_login_store_io",
            Self::Invalid => "workbuddy_login_store_invalid",
            Self::BindingMismatch => "workbuddy_login_store_binding_mismatch",
            Self::Unavailable => "workbuddy_login_store_unavailable",
        };
        f.write_str(text)
    }
}

/// WorkBuddy 登录代次（明文记录）。只允许存在于内存或 DPAPI 密文中；
/// 不实现 Debug，避免令牌进入失败输出。
#[derive(Clone)]
pub struct WorkBuddyLoginRecord {
    /// 工具侧档案标识：`wb-{uid}`（uid 唯一性传递）。
    pub profile_id: String,
    /// 账号身份 = WorkBuddy 本地 uid（UUID；同 uid 即同账号）。
    pub account_id: String,
    /// 展示名（捕获自 account-snapshot.json 的 nickname；可为空）。
    pub screen_name: String,
    /// WorkBuddy 会话无明文手机号——恒空串（占位对齐 Trae 清单形态）。
    pub masked_mobile: String,
    pub access_token: String,
    pub refresh_token: String,
    /// 过期时刻（Unix 秒；由会话毫秒值换算）。
    pub access_token_expires_at_unix_seconds: u64,
    pub refresh_token_expires_at_unix_seconds: u64,
    /// 官方接口域名（会话 `auth.domain`，如 www.workbuddy.cn）。
    pub domain: String,
    /// Keycloak 会话标识（sessionState）。
    pub session_state: String,
    /// 绑定的机器级设备标识（`~/.workbuddy/device-id` UUID）。
    pub device_id: String,
    /// 捕获/铸造时刻（Unix 秒）。
    pub minted_at_unix_seconds: u64,
    /// 最近一次整代次更新时刻。
    pub last_updated_at_unix_seconds: u64,
}

/// 阶段兼容的序列化形态。
#[derive(Serialize, Deserialize)]
struct SerializableRecord {
    format_version: u32,
    profile_id: String,
    account_id: String,
    screen_name: String,
    masked_mobile: String,
    access_token: String,
    refresh_token: String,
    #[serde(rename = "access_token_expires_at_unix_seconds")]
    access_expires_at: u64,
    #[serde(rename = "refresh_token_expires_at_unix_seconds")]
    refresh_expires_at: u64,
    domain: String,
    session_state: String,
    device_id: String,
    minted_at_unix_seconds: u64,
    last_updated_at_unix_seconds: u64,
}

/// 脱敏清单行：不含任何令牌材料，可直接进前端 DTO。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WorkBuddyLoginSummary {
    pub profile_id: String,
    pub account_id: String,
    pub screen_name: String,
    pub domain: String,
    pub access_token_expires_at_unix_seconds: u64,
    pub refresh_token_expires_at_unix_seconds: u64,
    pub minted_at_unix_seconds: u64,
    pub last_updated_at_unix_seconds: u64,
}

impl WorkBuddyLoginRecord {
    /// 工具侧档案标识惯例：`wb-{uid}`。
    pub fn profile_id_for(account_id: &str) -> String {
        format!("wb-{account_id}")
    }

    /// 脱敏摘要（清单/DTO 用；令牌材料不离开本 crate）。
    pub fn summary(&self) -> WorkBuddyLoginSummary {
        WorkBuddyLoginSummary {
            profile_id: self.profile_id.clone(),
            account_id: self.account_id.clone(),
            screen_name: self.screen_name.clone(),
            domain: self.domain.clone(),
            access_token_expires_at_unix_seconds: self.access_token_expires_at_unix_seconds,
            refresh_token_expires_at_unix_seconds: self.refresh_token_expires_at_unix_seconds,
            minted_at_unix_seconds: self.minted_at_unix_seconds,
            last_updated_at_unix_seconds: self.last_updated_at_unix_seconds,
        }
    }

    /// 身份完整性校验：关键标识与令牌材料非空。
    fn validate(&self) -> Result<(), WorkBuddyLoginStoreError> {
        if self.profile_id.is_empty()
            || self.account_id.is_empty()
            || self.access_token.is_empty()
            || self.refresh_token.is_empty()
            || self.domain.is_empty()
            || self.device_id.is_empty()
        {
            return Err(WorkBuddyLoginStoreError::Invalid);
        }
        Ok(())
    }
}

/// WorkBuddy 登录代次容器存储（`{root}/{profile_id}.wbsess`，每账号一文件）。
pub struct WorkBuddyLoginStore {
    root: PathBuf,
}

impl WorkBuddyLoginStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn record_path(&self, profile_id: &str) -> PathBuf {
        self.root.join(format!("{profile_id}.wbsess"))
    }

    /// 整代次原子写入（捕获落盘与更新共用；tmp + rename + 写后复核）。
    /// P17-A1：写入恒为明文 JSON 形态（format_version = 2）。
    pub fn save(&self, record: &WorkBuddyLoginRecord) -> Result<(), WorkBuddyLoginStoreError> {
        record.validate()?;
        let payload = encode_plain_payload(record)?;
        fs::create_dir_all(&self.root).map_err(|_| WorkBuddyLoginStoreError::Io)?;
        let destination = self.record_path(&record.profile_id);
        let temporary = self
            .root
            .join(format!("{}.wbsess.tmp", record.profile_id));
        fs::write(&temporary, &payload).map_err(|_| WorkBuddyLoginStoreError::Io)?;
        fs::rename(&temporary, &destination).map_err(|_| WorkBuddyLoginStoreError::Io)?;
        // 写后复核：回读并校验账号一致（写入半成功即在此暴露）。
        let readback = self
            .load(&record.profile_id, &record.account_id)
            .map_err(|_| WorkBuddyLoginStoreError::Io)?;
        if readback.account_id != record.account_id
            || readback.access_token != record.access_token
        {
            return Err(WorkBuddyLoginStoreError::Invalid);
        }
        Ok(())
    }

    /// 读取并校验绑定（双格式：明文 JSON 优先、旧 DPAPI 容器兼容读）；
    /// 任何失败不产生写副作用。
    pub fn load(
        &self,
        profile_id: &str,
        expected_account_id: &str,
    ) -> Result<WorkBuddyLoginRecord, WorkBuddyLoginStoreError> {
        if profile_id.is_empty() || expected_account_id.is_empty() {
            return Err(WorkBuddyLoginStoreError::Invalid);
        }
        let payload = fs::read(self.record_path(profile_id)).map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                WorkBuddyLoginStoreError::Invalid
            } else {
                WorkBuddyLoginStoreError::Io
            }
        })?;
        let record = decode_record_file(&payload)?;
        if record.profile_id != profile_id || record.account_id != expected_account_id {
            return Err(WorkBuddyLoginStoreError::BindingMismatch);
        }
        Ok(record)
    }

    /// 容器是否存在（不读取；清单展示与迁移判断用）。
    pub fn exists(&self, profile_id: &str) -> bool {
        self.record_path(profile_id).is_file()
    }

    /// P19.2① 池记录删除（彻底删除账号的代次容器清理；工具侧自有容器，
    /// 不触碰客户端 `.info` 会话文件）。幂等：容器不存在视为已删除。
    /// 返回是否实际删除了文件。
    pub fn remove(&self, profile_id: &str) -> Result<bool, WorkBuddyLoginStoreError> {
        let path = self.record_path(profile_id);
        if !path.is_file() {
            return Ok(false);
        }
        fs::remove_file(&path).map_err(|_| WorkBuddyLoginStoreError::Io)?;
        Ok(true)
    }

    /// 脱敏清单：扫描容器目录全部 `.wbsess` 文件并读取摘要。
    /// 损坏文件跳过（清单是展示面，坏一个不拖垮全部）。
    pub fn list(&self) -> Result<Vec<WorkBuddyLoginSummary>, WorkBuddyLoginStoreError> {
        let mut summaries = Vec::new();
        let entries = match fs::read_dir(&self.root) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(summaries),
            Err(_) => return Err(WorkBuddyLoginStoreError::Io),
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|extension| extension.to_str()) != Some("wbsess") {
                continue;
            }
            let Ok(payload) = fs::read(&path) else {
                continue;
            };
            if let Ok(record) = decode_record_file(&payload) {
                summaries.push(record.summary());
            }
        }
        summaries.sort_by(|a, b| a.profile_id.cmp(&b.profile_id));
        Ok(summaries)
    }

    /// P17-A1 存量迁移：旧 DPAPI 容器 → 明文 JSON（幂等）。
    ///
    /// 逐文件处理：明文形态跳过（`already_plain`）；旧 DPAPI 容器解密 →
    /// 先把旧密文字节备份到 `legacy_backup_dir`（Some 时，同文件名原样
    /// 副本——迁移前的保险，密文形态不引入明文泄漏面）→ 原子替换为明文
    /// JSON（`save` 自带 tmp + rename + 写后复核）→ 替换即旧密文消失
    /// （同路径覆盖 = 彻底删除，不走回收站）。单文件失败记录后继续
    /// （清单面纪律：坏一个不拖垮全部），失败文件的旧密文原位保留。
    pub fn migrate_legacy_to_plain(
        &self,
        legacy_backup_dir: Option<&Path>,
    ) -> Result<WorkBuddyLoginMigrationOutcome, WorkBuddyLoginStoreError> {
        let mut outcome = WorkBuddyLoginMigrationOutcome::default();
        let entries = match fs::read_dir(&self.root) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(outcome),
            Err(_) => return Err(WorkBuddyLoginStoreError::Io),
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|extension| extension.to_str()) != Some("wbsess") {
                continue;
            }
            let file_name = entry.file_name().to_string_lossy().into_owned();
            let profile_id = file_name.trim_end_matches(".wbsess").to_string();
            let Ok(payload) = fs::read(&path) else {
                outcome.failed.push(profile_id);
                continue;
            };
            if payload.first() == Some(&b'{') {
                // 已是明文形态：幂等跳过。
                outcome.already_plain += 1;
                continue;
            }
            let record = match decode_legacy_payload(&payload) {
                Ok(record) => record,
                Err(_) => {
                    outcome.failed.push(profile_id);
                    continue;
                }
            };
            // 迁移前旧密文备份（存在才拷；同内容重复覆盖安全）。
            if let Some(backup_root) = legacy_backup_dir {
                if fs::create_dir_all(backup_root).is_err()
                    || fs::copy(&path, backup_root.join(&file_name)).is_err()
                {
                    outcome.failed.push(profile_id);
                    continue;
                }
            }
            match self.save(&record) {
                Ok(()) => outcome.migrated += 1,
                Err(_) => outcome.failed.push(profile_id),
            }
        }
        Ok(outcome)
    }
}

/// P17-A1 存量迁移回执（不含任何凭据材料）。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct WorkBuddyLoginMigrationOutcome {
    /// 本次由 DPAPI 容器转换为明文 JSON 的文件数。
    pub migrated: usize,
    /// 迁移前已是明文形态、幂等跳过的文件数。
    pub already_plain: usize,
    /// 迁移失败的 profile_id 清单（旧文件原位保留，可重试）。
    pub failed: Vec<String>,
}

/// 明文 JSON 编码（P17-A1）：裸 JSON 记录，`format_version = 2` 标记
/// 明文形态。无魔数头——实现最简；格式识别靠首字节（`{` = 明文 JSON，
/// 其余按旧 DPAPI 容器兼容读）。
fn encode_plain_payload(
    record: &WorkBuddyLoginRecord,
) -> Result<Vec<u8>, WorkBuddyLoginStoreError> {
    let serialized = SerializableRecord {
        format_version: WORKBUDDY_LOGIN_PLAIN_FORMAT_VERSION,
        profile_id: record.profile_id.clone(),
        account_id: record.account_id.clone(),
        screen_name: record.screen_name.clone(),
        masked_mobile: record.masked_mobile.clone(),
        access_token: record.access_token.clone(),
        refresh_token: record.refresh_token.clone(),
        access_expires_at: record.access_token_expires_at_unix_seconds,
        refresh_expires_at: record.refresh_token_expires_at_unix_seconds,
        domain: record.domain.clone(),
        session_state: record.session_state.clone(),
        device_id: record.device_id.clone(),
        minted_at_unix_seconds: record.minted_at_unix_seconds,
        last_updated_at_unix_seconds: record.last_updated_at_unix_seconds,
    };
    let payload = serde_json::to_vec(&serialized).map_err(|_| WorkBuddyLoginStoreError::Invalid)?;
    if payload.is_empty() || payload.len() as u64 > MAX_PAYLOAD_BYTES {
        return Err(WorkBuddyLoginStoreError::Invalid);
    }
    Ok(payload)
}

/// 双格式文件解码：首字节 `{` = 明文 JSON（P17-A1）；否则按旧 DPAPI
/// 容器兼容读（迁移窗口；迁移后不再产生）。
fn decode_record_file(payload: &[u8]) -> Result<WorkBuddyLoginRecord, WorkBuddyLoginStoreError> {
    if payload.first() == Some(&b'{') {
        return decode_plain_payload(payload);
    }
    decode_legacy_payload(payload)
}

/// 明文 JSON 解码。
fn decode_plain_payload(payload: &[u8]) -> Result<WorkBuddyLoginRecord, WorkBuddyLoginStoreError> {
    if payload.is_empty() || payload.len() as u64 > MAX_PAYLOAD_BYTES {
        return Err(WorkBuddyLoginStoreError::Invalid);
    }
    let serialized: SerializableRecord =
        serde_json::from_slice(payload).map_err(|_| WorkBuddyLoginStoreError::Invalid)?;
    if serialized.format_version != WORKBUDDY_LOGIN_PLAIN_FORMAT_VERSION {
        return Err(WorkBuddyLoginStoreError::Invalid);
    }
    let record = WorkBuddyLoginRecord {
        profile_id: serialized.profile_id,
        account_id: serialized.account_id,
        screen_name: serialized.screen_name,
        masked_mobile: serialized.masked_mobile,
        access_token: serialized.access_token,
        refresh_token: serialized.refresh_token,
        access_token_expires_at_unix_seconds: serialized.access_expires_at,
        refresh_token_expires_at_unix_seconds: serialized.refresh_expires_at,
        domain: serialized.domain,
        session_state: serialized.session_state,
        device_id: serialized.device_id,
        minted_at_unix_seconds: serialized.minted_at_unix_seconds,
        last_updated_at_unix_seconds: serialized.last_updated_at_unix_seconds,
    };
    record.validate()?;
    Ok(record)
}

/// 旧 DPAPI 容器解码（P17-A1 迁移窗口兼容读；`magic(8) || version(u32 LE)
/// || payload_len(u32 LE) || DPAPI 密文`）。
fn decode_legacy_payload(
    payload: &[u8],
) -> Result<WorkBuddyLoginRecord, WorkBuddyLoginStoreError> {
    if payload.len() <= LEGACY_HEADER_LEN || &payload[..8] != WORKBUDDY_LOGIN_MAGIC {
        return Err(WorkBuddyLoginStoreError::Invalid);
    }
    let version = u32::from_le_bytes(
        payload[8..12]
            .try_into()
            .map_err(|_| WorkBuddyLoginStoreError::Invalid)?,
    );
    if version != WORKBUDDY_LOGIN_FORMAT_VERSION {
        return Err(WorkBuddyLoginStoreError::Invalid);
    }
    let encrypted_len = u32::from_le_bytes(
        payload[12..16]
            .try_into()
            .map_err(|_| WorkBuddyLoginStoreError::Invalid)?,
    ) as usize;
    if encrypted_len == 0
        || encrypted_len as u64 > MAX_PAYLOAD_BYTES
        || payload.len() != LEGACY_HEADER_LEN + encrypted_len
    {
        return Err(WorkBuddyLoginStoreError::Invalid);
    }
    let plaintext = unprotect(&payload[LEGACY_HEADER_LEN..]).map_err(map_key_wrapper_error)?;
    let serialized: SerializableRecord =
        serde_json::from_slice(&plaintext).map_err(|_| WorkBuddyLoginStoreError::Invalid)?;
    let record = WorkBuddyLoginRecord {
        profile_id: serialized.profile_id,
        account_id: serialized.account_id,
        screen_name: serialized.screen_name,
        masked_mobile: serialized.masked_mobile,
        access_token: serialized.access_token,
        refresh_token: serialized.refresh_token,
        access_token_expires_at_unix_seconds: serialized.access_expires_at,
        refresh_token_expires_at_unix_seconds: serialized.refresh_expires_at,
        domain: serialized.domain,
        session_state: serialized.session_state,
        device_id: serialized.device_id,
        minted_at_unix_seconds: serialized.minted_at_unix_seconds,
        last_updated_at_unix_seconds: serialized.last_updated_at_unix_seconds,
    };
    record.validate()?;
    Ok(record)
}

fn map_key_wrapper_error(error: KeyWrapperError) -> WorkBuddyLoginStoreError {
    match error {
        KeyWrapperError::Unavailable => WorkBuddyLoginStoreError::Unavailable,
        KeyWrapperError::Failed => WorkBuddyLoginStoreError::Invalid,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::key_wrapper::protect;
    use tempfile::tempdir;

    const FIXTURE_UID: &str = "89b16d6b-0549-4e1e-8ced-918b5c8152ae";

    fn fixture_record() -> WorkBuddyLoginRecord {
        WorkBuddyLoginRecord {
            profile_id: WorkBuddyLoginRecord::profile_id_for(FIXTURE_UID),
            account_id: FIXTURE_UID.to_string(),
            screen_name: "梦梦".to_string(),
            masked_mobile: String::new(),
            access_token: "fixture-access-jwt".to_string(),
            refresh_token: "fixture-refresh-jwt".to_string(),
            access_token_expires_at_unix_seconds: 1_800_000_000 + 28 * 86_400,
            refresh_token_expires_at_unix_seconds: 1_800_000_000 + 30 * 86_400,
            domain: "www.workbuddy.cn".to_string(),
            session_state: "7e57e1c9-9d02-4281-ba26-f61b7e4c655f".to_string(),
            device_id: "2db49f20-2b3f-4761-ab53-9d213f361a5b".to_string(),
            minted_at_unix_seconds: 1_800_000_000,
            last_updated_at_unix_seconds: 1_800_000_000,
        }
    }

    /// P17-A1：落盘为明文 JSON（首字节 `{`，format_version = 2），不再
    /// 经过 DPAPI 加密层。
    #[test]
    fn save_writes_plain_json_format() {
        let root = tempdir().expect("tempdir");
        let store = WorkBuddyLoginStore::new(root.path());
        let record = fixture_record();
        store.save(&record).expect("save");
        let path = root.path().join(format!("{}.wbsess", record.profile_id));
        let payload = fs::read(&path).expect("read");
        assert_eq!(payload.first(), Some(&b'{'), "明文 JSON 首字节应为 '{{'");
        let value: serde_json::Value = serde_json::from_slice(&payload).expect("valid json");
        assert_eq!(
            value["format_version"],
            WORKBUDDY_LOGIN_PLAIN_FORMAT_VERSION
        );
        assert_eq!(value["account_id"], FIXTURE_UID);
        // 无旧容器魔数头。
        assert!(&payload[..8] != WORKBUDDY_LOGIN_MAGIC);
    }

    /// P17-A1 旧格式兼容读：手工构造的 DPAPI 容器仍可 load（迁移窗口）。
    #[test]
    fn legacy_dpapi_container_still_loadable() {
        let record = fixture_record();
        let legacy = encode_legacy_fixture(&record);
        let root = tempdir().expect("tempdir");
        let store = WorkBuddyLoginStore::new(root.path());
        fs::write(
            root.path().join(format!("{}.wbsess", record.profile_id)),
            legacy,
        )
        .expect("write legacy container");
        let loaded = store
            .load(&record.profile_id, FIXTURE_UID)
            .expect("legacy load");
        assert_eq!(loaded.access_token, record.access_token);
    }

    /// P17-A1 迁移：DPAPI 容器 → 明文 JSON；旧密文先备份到 legacy 目录；
    /// 幂等（二次运行为 already_plain）；迁移后读取结果一致。
    #[test]
    fn migrate_legacy_to_plain_is_idempotent_and_backs_up_legacy() {
        let record = fixture_record();
        let root = tempdir().expect("tempdir");
        let store = WorkBuddyLoginStore::new(root.path());
        let path = root.path().join(format!("{}.wbsess", record.profile_id));
        fs::write(&path, encode_legacy_fixture(&record)).expect("write legacy container");
        let backup_dir = root.path().join("legacy-backup");

        let outcome = store
            .migrate_legacy_to_plain(Some(&backup_dir))
            .expect("migrate");
        assert_eq!(outcome.migrated, 1);
        assert_eq!(outcome.already_plain, 0);
        assert!(outcome.failed.is_empty());
        // 文件已替换为明文 JSON；旧密文备份在位。
        let payload = fs::read(&path).expect("read");
        assert_eq!(payload.first(), Some(&b'{'));
        let legacy_backup = fs::read(backup_dir.join(format!("{}.wbsess", record.profile_id)))
            .expect("legacy backup");
        assert_eq!(&legacy_backup[..8], WORKBUDDY_LOGIN_MAGIC, "备份为旧密文");
        // 迁移后读取一致。
        let loaded = store.load(&record.profile_id, FIXTURE_UID).expect("load");
        assert_eq!(loaded.access_token, record.access_token);
        assert_eq!(loaded.session_state, record.session_state);

        // 幂等：二次运行全部跳过，不再迁移、备份不重复产生变化。
        let outcome2 = store
            .migrate_legacy_to_plain(Some(&backup_dir))
            .expect("migrate again");
        assert_eq!(outcome2.migrated, 0);
        assert_eq!(outcome2.already_plain, 1);
        assert!(outcome2.failed.is_empty());
    }

    /// P17-A1：损坏明文 JSON 拒绝（非法 JSON / 版本不符 / 截断）。
    #[test]
    fn load_rejects_corrupted_plain_json() {
        let root = tempdir().expect("tempdir");
        let store = WorkBuddyLoginStore::new(root.path());
        let record = fixture_record();
        store.save(&record).expect("save");
        let path = root.path().join(format!("{}.wbsess", record.profile_id));
        // 非 JSON 内容（首字节非 `{` 也非魔数）。
        fs::write(&path, b"garbage").expect("write");
        assert_eq!(
            store.load(&record.profile_id, FIXTURE_UID).err(),
            Some(WorkBuddyLoginStoreError::Invalid)
        );
        // 恢复合法明文后再篡改版本号（前一步已把文件写成坏样本）。
        store.save(&record).expect("restore plain");
        // JSON 合法但版本不符（=1 的明文形态不存在，拒绝）。
        let mut value: serde_json::Value =
            serde_json::from_slice(&fs::read(&path).expect("read")).expect("json");
        value["format_version"] = serde_json::json!(1);
        fs::write(&path, value.to_string()).expect("write");
        assert_eq!(
            store.load(&record.profile_id, FIXTURE_UID).err(),
            Some(WorkBuddyLoginStoreError::Invalid)
        );
        // 截断。
        let mut payload = fs::read(&path).expect("read");
        payload.truncate(10);
        fs::write(&path, &payload).expect("write");
        assert_eq!(
            store.load(&record.profile_id, FIXTURE_UID).err(),
            Some(WorkBuddyLoginStoreError::Invalid)
        );
    }

    /// 测试辅助：按旧 DPAPI 容器格式编码 fixture 记录（兼容读/迁移用）。
    fn encode_legacy_fixture(record: &WorkBuddyLoginRecord) -> Vec<u8> {
        let serialized = SerializableRecord {
            format_version: WORKBUDDY_LOGIN_FORMAT_VERSION,
            profile_id: record.profile_id.clone(),
            account_id: record.account_id.clone(),
            screen_name: record.screen_name.clone(),
            masked_mobile: record.masked_mobile.clone(),
            access_token: record.access_token.clone(),
            refresh_token: record.refresh_token.clone(),
            access_expires_at: record.access_token_expires_at_unix_seconds,
            refresh_expires_at: record.refresh_token_expires_at_unix_seconds,
            domain: record.domain.clone(),
            session_state: record.session_state.clone(),
            device_id: record.device_id.clone(),
            minted_at_unix_seconds: record.minted_at_unix_seconds,
            last_updated_at_unix_seconds: record.last_updated_at_unix_seconds,
        };
        let plaintext = serde_json::to_vec(&serialized).expect("serialize");
        let encrypted = protect(&plaintext).expect("dpapi protect");
        let mut payload = Vec::new();
        payload.extend_from_slice(WORKBUDDY_LOGIN_MAGIC);
        payload.extend_from_slice(&WORKBUDDY_LOGIN_FORMAT_VERSION.to_le_bytes());
        payload.extend_from_slice(&(encrypted.len() as u32).to_le_bytes());
        payload.extend_from_slice(&encrypted);
        payload
    }

    #[test]
    fn save_then_load_roundtrips_identity_and_tokens() {
        let root = tempdir().expect("tempdir");
        let store = WorkBuddyLoginStore::new(root.path());
        let record = fixture_record();
        store.save(&record).expect("save");
        let loaded = store.load(&record.profile_id, FIXTURE_UID).expect("load");
        assert_eq!(loaded.account_id, FIXTURE_UID);
        assert_eq!(loaded.access_token, record.access_token);
        assert_eq!(loaded.refresh_token, record.refresh_token);
        assert_eq!(loaded.domain, record.domain);
        assert_eq!(loaded.session_state, record.session_state);
        assert_eq!(loaded.device_id, record.device_id);
        assert!(store.exists(&record.profile_id));
    }

    /// profile_id 惯例：wb-{uid}。
    #[test]
    fn profile_id_convention() {
        assert_eq!(
            WorkBuddyLoginRecord::profile_id_for(FIXTURE_UID),
            format!("wb-{FIXTURE_UID}")
        );
    }

    #[test]
    fn load_rejects_account_mismatch() {
        let root = tempdir().expect("tempdir");
        let store = WorkBuddyLoginStore::new(root.path());
        let record = fixture_record();
        store.save(&record).expect("save");
        // 错误断言不触发 Record 的 Debug（凭据不进失败输出）。
        let error = store
            .load(&record.profile_id, "other-uid")
            .err()
            .expect("binding mismatch must fail");
        assert_eq!(error, WorkBuddyLoginStoreError::BindingMismatch);
    }

    #[test]
    fn load_rejects_missing_and_corrupted_files() {
        let root = tempdir().expect("tempdir");
        let store = WorkBuddyLoginStore::new(root.path());
        let record = fixture_record();
        assert_eq!(
            store.load(&record.profile_id, FIXTURE_UID).err(),
            Some(WorkBuddyLoginStoreError::Invalid)
        );
        store.save(&record).expect("save");
        let path = root.path().join(format!("{}.wbsess", record.profile_id));
        let mut payload = fs::read(&path).expect("read");
        payload[0] = b'X';
        fs::write(&path, &payload).expect("write");
        assert_eq!(
            store.load(&record.profile_id, FIXTURE_UID).err(),
            Some(WorkBuddyLoginStoreError::Invalid)
        );
        payload.truncate(10);
        fs::write(&path, &payload).expect("write");
        assert_eq!(
            store.load(&record.profile_id, FIXTURE_UID).err(),
            Some(WorkBuddyLoginStoreError::Invalid)
        );
    }

    #[test]
    fn save_rejects_empty_identity_fields() {
        let root = tempdir().expect("tempdir");
        let store = WorkBuddyLoginStore::new(root.path());
        let mut record = fixture_record();
        record.access_token = String::new();
        assert_eq!(store.save(&record), Err(WorkBuddyLoginStoreError::Invalid));
        assert!(!store.exists(&record.profile_id));
    }

    /// 整代次更新：原子替换 + tmp 不残留。
    #[test]
    fn update_generation_replaces_tokens_atomically() {
        let root = tempdir().expect("tempdir");
        let store = WorkBuddyLoginStore::new(root.path());
        let record = fixture_record();
        store.save(&record).expect("initial save");
        let mut refreshed = fixture_record();
        refreshed.access_token = "fixture-access-jwt-v2".to_string();
        refreshed.refresh_token = "fixture-refresh-jwt-v2".to_string();
        refreshed.last_updated_at_unix_seconds = 1_800_000_000 + 3_600;
        store.save(&refreshed).expect("update");
        let loaded = store
            .load(&record.profile_id, FIXTURE_UID)
            .expect("load after update");
        assert_eq!(loaded.access_token, "fixture-access-jwt-v2");
        assert_eq!(loaded.last_updated_at_unix_seconds, 1_800_000_000 + 3_600);
        assert!(!root
            .path()
            .join(format!("{}.wbsess.tmp", record.profile_id))
            .exists());
    }

    /// 脱敏清单：summary 序列化产物不含令牌值；损坏文件跳过。
    #[test]
    fn list_returns_masked_summaries_and_skips_corrupted() {
        let root = tempdir().expect("tempdir");
        let store = WorkBuddyLoginStore::new(root.path());
        let record = fixture_record();
        store.save(&record).expect("save");
        fs::write(root.path().join("wb-broken.wbsess"), b"garbage").expect("write");
        fs::write(root.path().join("notes.txt"), b"text").expect("write");
        let summaries = store.list().expect("list");
        assert_eq!(summaries.len(), 1);
        assert_eq!(summaries[0].profile_id, record.profile_id);
        assert_eq!(summaries[0].screen_name, "梦梦");
        let json = serde_json::to_string(&summaries[0]).expect("serialize summary");
        assert!(!json.contains("fixture-access-jwt"));
        assert!(!json.contains("fixture-refresh-jwt"));
    }
}
