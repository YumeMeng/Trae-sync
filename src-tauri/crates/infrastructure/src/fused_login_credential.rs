//! 融合版登录代次容器（ADR-0035）：每账号一套融合版 TRAE 客户端登录凭据
//! （TRAE 通道 `ono9krqynydwx5`/IDE_PC，绑定机器级 ICDRS 设备身份）。
//!
//! 与签到凭据包（`checkin_credential`，SOLO 通道）是**双通道并行**的两套
//! 材料：本模块只读写自己的容器文件（`{root}/{profile_id}.fused`），绝不
//! 触碰 `.checkin` 文件；签到链路也绝不触碰本容器（跨通道复制禁令）。
//!
//! **P17-A1 明文化（spec 10.3，用户拍板：个人工具明文存储，不做防御性
//! 措施）**：文件格式为**裸明文 JSON 记录**（`format_version = 2` 标记
//! 明文形态——选实现最简的裸 JSON，无需魔数头）；DPAPI 加密层移除。
//! 双格式兼容读取：新写入恒为明文 JSON；旧 `TSFUSED1` DPAPI 容器（
//! `magic(8) || version(u32 LE) || payload_len(u32 LE) || DPAPI 密文`，
//! 与阶段一迁移探针落盘文件逐字节兼容）保留兼容读，经
//! [`FusedLoginStore::migrate_legacy_to_plain`] 幂等迁移后不再产生。
//!
//! 纪律（与 `checkin_credential` 同构）：
//! - 整代次原子替换（tmp + rename + 写后复核，ADR-0019 第 7 条同纪律）；
//! - 任何读取都校验 `profile_id ↔ account_id` 绑定，不符零返回；
//! - 续期由融合版客户端自动完成，工具只做整代次回存更新（P15-5 切出
//!   回存），不主动发起任何刷新请求。

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

// DPAPI 原语与错误类型：旧容器兼容读/迁移解码用（P17-A1 明文化后
// 新写入不再经过 DPAPI）。
use crate::key_wrapper::unprotect;
use traesync_ports::KeyWrapperError;

/// 旧 DPAPI 容器魔数：与阶段一探针一致（`TSFUSED1`；P17-A1 起仅作迁移
/// 窗口内的兼容读）。
pub const FUSED_LOGIN_MAGIC: &[u8; 8] = b"TSFUSED1";
/// 旧 DPAPI 容器格式版本。
pub const FUSED_LOGIN_FORMAT_VERSION: u32 = 1;
/// 明文 JSON 形态版本（P17-A1：2 = 裸明文 JSON 记录）。
pub const FUSED_LOGIN_PLAIN_FORMAT_VERSION: u32 = 2;
/// 单文件上限（防异常膨胀；正常记录约 2KB）。
const MAX_PAYLOAD_BYTES: u64 = 64 * 1024;
/// 旧容器头部长度：magic(8) + version(4) + payload_len(4)。
const LEGACY_HEADER_LEN: usize = 16;

/// 容器错误。变体不含令牌或文件内容（凭据不进错误链）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FusedLoginStoreError {
    /// 文件系统 IO 失败（原子写/读/建目录）。
    Io,
    /// 文件损坏：魔数/版本/长度不符、密文解不开、JSON 非法、超限。
    Invalid,
    /// 绑定不符：记录 profile_id 与期望账号不匹配（宁可不返回，不可给错）。
    BindingMismatch,
    /// 非 Windows 平台（DPAPI 不可用）。
    Unavailable,
}

impl std::fmt::Display for FusedLoginStoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let text = match self {
            Self::Io => "fused_login_store_io",
            Self::Invalid => "fused_login_store_invalid",
            Self::BindingMismatch => "fused_login_store_binding_mismatch",
            Self::Unavailable => "fused_login_store_unavailable",
        };
        f.write_str(text)
    }
}

/// 融合版登录代次（明文记录）。只允许存在于内存或 DPAPI 密文中；
/// 不实现 Debug，避免令牌进入失败输出。
#[derive(Clone)]
pub struct FusedLoginRecord {
    pub profile_id: String,
    pub account_id: String,
    pub screen_name: String,
    pub masked_mobile: String,
    pub access_token: String,
    pub refresh_token: String,
    pub access_token_expires_at_unix_seconds: u64,
    pub refresh_token_expires_at_unix_seconds: u64,
    /// 绑定的融合版设备身份：ICDRS rdid（机器级）。
    pub device_id: String,
    pub machine_id: String,
    pub device_public_key: String,
    /// 铸造时刻（Unix 秒）。
    pub minted_at_unix_seconds: u64,
    /// 最近一次整代次更新（回存/铸造）时刻；阶段一文件缺省回退铸造时刻。
    pub last_updated_at_unix_seconds: u64,
}

/// 阶段一兼容的序列化形态：字段名与 `unified_mint_probe --save-only` 写出
/// 的明文 JSON 逐字一致；`last_updated_at_unix_seconds` 为 P15-4 新增字段，
/// 旧文件经 `default` 兼容读取（回退铸造时刻）。
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
    device_id: String,
    machine_id: String,
    device_public_key: String,
    minted_at_unix_seconds: u64,
    #[serde(default)]
    last_updated_at_unix_seconds: u64,
}

/// 脱敏清单行：不含任何令牌材料，可直接进前端 DTO。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FusedLoginSummary {
    pub profile_id: String,
    pub account_id: String,
    pub screen_name: String,
    pub masked_mobile: String,
    pub access_token_expires_at_unix_seconds: u64,
    pub refresh_token_expires_at_unix_seconds: u64,
    pub minted_at_unix_seconds: u64,
    pub last_updated_at_unix_seconds: u64,
}

impl FusedLoginRecord {
    /// 脱敏摘要（清单/DTO 用；令牌材料不离开本 crate）。
    pub fn summary(&self) -> FusedLoginSummary {
        FusedLoginSummary {
            profile_id: self.profile_id.clone(),
            account_id: self.account_id.clone(),
            screen_name: self.screen_name.clone(),
            masked_mobile: self.masked_mobile.clone(),
            access_token_expires_at_unix_seconds: self.access_token_expires_at_unix_seconds,
            refresh_token_expires_at_unix_seconds: self.refresh_token_expires_at_unix_seconds,
            minted_at_unix_seconds: self.minted_at_unix_seconds,
            last_updated_at_unix_seconds: self.last_updated_at_unix_seconds,
        }
    }

    /// 身份完整性校验：关键标识与令牌材料非空。
    fn validate(&self) -> Result<(), FusedLoginStoreError> {
        if self.profile_id.is_empty()
            || self.account_id.is_empty()
            || self.access_token.is_empty()
            || self.refresh_token.is_empty()
            || self.device_id.is_empty()
        {
            return Err(FusedLoginStoreError::Invalid);
        }
        Ok(())
    }
}

/// 融合版登录代次容器存储（`{root}/{profile_id}.fused`，每账号一文件）。
pub struct FusedLoginStore {
    root: PathBuf,
}

impl FusedLoginStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn record_path(&self, profile_id: &str) -> PathBuf {
        self.root.join(format!("{profile_id}.fused"))
    }

    /// 整代次原子写入（铸造落盘与切出回存共用；tmp + rename + 写后复核）。
    /// P17-A1：写入恒为明文 JSON 形态（format_version = 2）。
    pub fn save(&self, record: &FusedLoginRecord) -> Result<(), FusedLoginStoreError> {
        record.validate()?;
        let payload = encode_plain_payload(record)?;
        fs::create_dir_all(&self.root).map_err(|_| FusedLoginStoreError::Io)?;
        let destination = self.record_path(&record.profile_id);
        let temporary = self.root.join(format!(
            "{}.fused.tmp",
            record.profile_id
        ));
        fs::write(&temporary, &payload).map_err(|_| FusedLoginStoreError::Io)?;
        fs::rename(&temporary, &destination).map_err(|_| FusedLoginStoreError::Io)?;
        // 写后复核：回读并校验账号一致（写入半成功即在此暴露）。
        let readback = self
            .load(&record.profile_id, &record.account_id)
            .map_err(|_| FusedLoginStoreError::Io)?;
        if readback.account_id != record.account_id
            || readback.access_token != record.access_token
        {
            return Err(FusedLoginStoreError::Invalid);
        }
        Ok(())
    }

    /// 读取并校验绑定（双格式：明文 JSON 优先、旧 DPAPI 容器兼容读）；
    /// 任何失败不产生写副作用。
    pub fn load(
        &self,
        profile_id: &str,
        expected_account_id: &str,
    ) -> Result<FusedLoginRecord, FusedLoginStoreError> {
        if profile_id.is_empty() || expected_account_id.is_empty() {
            return Err(FusedLoginStoreError::Invalid);
        }
        let payload = fs::read(self.record_path(profile_id)).map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                FusedLoginStoreError::Invalid
            } else {
                FusedLoginStoreError::Io
            }
        })?;
        let record = decode_record_file(&payload)?;
        if record.profile_id != profile_id || record.account_id != expected_account_id {
            return Err(FusedLoginStoreError::BindingMismatch);
        }
        Ok(record)
    }

    /// 容器是否存在（不读取；用于清单展示与迁移判断）。
    pub fn exists(&self, profile_id: &str) -> bool {
        self.record_path(profile_id).is_file()
    }

    /// 脱敏清单：扫描容器目录全部 `.fused` 文件并读取摘要。
    /// 损坏文件跳过（清单是展示面，坏一个不拖垮全部）；调用方可用
    /// `exists`/`load` 对单个账号追查。
    pub fn list(&self) -> Result<Vec<FusedLoginSummary>, FusedLoginStoreError> {
        let mut summaries = Vec::new();
        let entries = match fs::read_dir(&self.root) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(summaries),
            Err(_) => return Err(FusedLoginStoreError::Io),
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("fused") {
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
    /// （同路径覆盖 = 彻底删除，不走回收站）。单文件失败记录后继续，
    /// 失败文件的旧密文原位保留（可重试）。
    pub fn migrate_legacy_to_plain(
        &self,
        legacy_backup_dir: Option<&Path>,
    ) -> Result<FusedLoginMigrationOutcome, FusedLoginStoreError> {
        let mut outcome = FusedLoginMigrationOutcome::default();
        let entries = match fs::read_dir(&self.root) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(outcome),
            Err(_) => return Err(FusedLoginStoreError::Io),
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|extension| extension.to_str()) != Some("fused") {
                continue;
            }
            let file_name = entry.file_name().to_string_lossy().into_owned();
            let profile_id = file_name.trim_end_matches(".fused").to_string();
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
pub struct FusedLoginMigrationOutcome {
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
fn encode_plain_payload(record: &FusedLoginRecord) -> Result<Vec<u8>, FusedLoginStoreError> {
    let serialized = SerializableRecord {
        format_version: FUSED_LOGIN_PLAIN_FORMAT_VERSION,
        profile_id: record.profile_id.clone(),
        account_id: record.account_id.clone(),
        screen_name: record.screen_name.clone(),
        masked_mobile: record.masked_mobile.clone(),
        access_token: record.access_token.clone(),
        refresh_token: record.refresh_token.clone(),
        access_expires_at: record.access_token_expires_at_unix_seconds,
        refresh_expires_at: record.refresh_token_expires_at_unix_seconds,
        device_id: record.device_id.clone(),
        machine_id: record.machine_id.clone(),
        device_public_key: record.device_public_key.clone(),
        minted_at_unix_seconds: record.minted_at_unix_seconds,
        last_updated_at_unix_seconds: record.last_updated_at_unix_seconds,
    };
    let payload = serde_json::to_vec(&serialized).map_err(|_| FusedLoginStoreError::Invalid)?;
    if payload.is_empty() || payload.len() as u64 > MAX_PAYLOAD_BYTES {
        return Err(FusedLoginStoreError::Invalid);
    }
    Ok(payload)
}

/// 双格式文件解码：首字节 `{` = 明文 JSON（P17-A1）；否则按旧 DPAPI
/// 容器兼容读（迁移窗口；迁移后不再产生）。
fn decode_record_file(payload: &[u8]) -> Result<FusedLoginRecord, FusedLoginStoreError> {
    if payload.first() == Some(&b'{') {
        return decode_plain_payload(payload);
    }
    decode_legacy_payload(payload)
}

/// 明文 JSON 解码。
fn decode_plain_payload(payload: &[u8]) -> Result<FusedLoginRecord, FusedLoginStoreError> {
    if payload.is_empty() || payload.len() as u64 > MAX_PAYLOAD_BYTES {
        return Err(FusedLoginStoreError::Invalid);
    }
    let serialized: SerializableRecord =
        serde_json::from_slice(payload).map_err(|_| FusedLoginStoreError::Invalid)?;
    if serialized.format_version != FUSED_LOGIN_PLAIN_FORMAT_VERSION {
        return Err(FusedLoginStoreError::Invalid);
    }
    let record = FusedLoginRecord {
        profile_id: serialized.profile_id,
        account_id: serialized.account_id,
        screen_name: serialized.screen_name,
        masked_mobile: serialized.masked_mobile,
        access_token: serialized.access_token,
        refresh_token: serialized.refresh_token,
        access_token_expires_at_unix_seconds: serialized.access_expires_at,
        refresh_token_expires_at_unix_seconds: serialized.refresh_expires_at,
        device_id: serialized.device_id,
        machine_id: serialized.machine_id,
        device_public_key: serialized.device_public_key,
        minted_at_unix_seconds: serialized.minted_at_unix_seconds,
        last_updated_at_unix_seconds: serialized.last_updated_at_unix_seconds,
    };
    record.validate()?;
    Ok(record)
}

/// 旧 DPAPI 容器解码（P17-A1 迁移窗口兼容读；`magic(8) || version(u32 LE)
/// || payload_len(u32 LE) || DPAPI 密文`）。
fn decode_legacy_payload(payload: &[u8]) -> Result<FusedLoginRecord, FusedLoginStoreError> {
    if payload.len() <= LEGACY_HEADER_LEN || &payload[..8] != FUSED_LOGIN_MAGIC {
        return Err(FusedLoginStoreError::Invalid);
    }
    let version = u32::from_le_bytes(
        payload[8..12]
            .try_into()
            .map_err(|_| FusedLoginStoreError::Invalid)?,
    );
    if version != FUSED_LOGIN_FORMAT_VERSION {
        return Err(FusedLoginStoreError::Invalid);
    }
    let encrypted_len = u32::from_le_bytes(
        payload[12..16]
            .try_into()
            .map_err(|_| FusedLoginStoreError::Invalid)?,
    ) as usize;
    if encrypted_len == 0
        || encrypted_len as u64 > MAX_PAYLOAD_BYTES
        || payload.len() != LEGACY_HEADER_LEN + encrypted_len
    {
        return Err(FusedLoginStoreError::Invalid);
    }
    let plaintext = unprotect(&payload[LEGACY_HEADER_LEN..]).map_err(map_key_wrapper_error)?;
    let serialized: SerializableRecord =
        serde_json::from_slice(&plaintext).map_err(|_| FusedLoginStoreError::Invalid)?;
    let record = FusedLoginRecord {
        profile_id: serialized.profile_id,
        account_id: serialized.account_id,
        screen_name: serialized.screen_name,
        masked_mobile: serialized.masked_mobile,
        access_token: serialized.access_token,
        refresh_token: serialized.refresh_token,
        access_token_expires_at_unix_seconds: serialized.access_expires_at,
        refresh_token_expires_at_unix_seconds: serialized.refresh_expires_at,
        device_id: serialized.device_id,
        machine_id: serialized.machine_id,
        device_public_key: serialized.device_public_key,
        minted_at_unix_seconds: serialized.minted_at_unix_seconds,
        // 阶段一文件无此字段：回退铸造时刻（语义自洽——铸造即首次更新）。
        last_updated_at_unix_seconds: if serialized.last_updated_at_unix_seconds == 0 {
            serialized.minted_at_unix_seconds
        } else {
            serialized.last_updated_at_unix_seconds
        },
    };
    record.validate()?;
    Ok(record)
}

fn map_key_wrapper_error(error: KeyWrapperError) -> FusedLoginStoreError {
    match error {
        KeyWrapperError::Unavailable => FusedLoginStoreError::Unavailable,
        KeyWrapperError::Failed => FusedLoginStoreError::Invalid,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::key_wrapper::protect;
    use tempfile::tempdir;

    const FIXTURE_ACCOUNT: &str = "fixture-account-a";
    const FIXTURE_PROFILE: &str = "checkin-fixture0001";

    fn fixture_record() -> FusedLoginRecord {
        FusedLoginRecord {
            profile_id: FIXTURE_PROFILE.to_string(),
            account_id: FIXTURE_ACCOUNT.to_string(),
            screen_name: "测试账号".to_string(),
            masked_mobile: "138****0000".to_string(),
            access_token: "fixture-access-token".to_string(),
            refresh_token: "fixture-refresh-token".to_string(),
            access_token_expires_at_unix_seconds: 1_800_000_000 + 14 * 86_400,
            refresh_token_expires_at_unix_seconds: 1_800_000_000 + 180 * 86_400,
            device_id: "2490859781907946".to_string(),
            machine_id: "fixture-machine-id".to_string(),
            device_public_key: "-----BEGIN PUBLIC KEY-----\nfixture\n-----END PUBLIC KEY-----\n"
                .to_string(),
            minted_at_unix_seconds: 1_800_000_000,
            last_updated_at_unix_seconds: 1_800_000_000,
        }
    }

    #[test]
    fn save_then_load_roundtrips_identity_and_tokens() {
        let root = tempdir().expect("tempdir");
        let store = FusedLoginStore::new(root.path());
        let record = fixture_record();
        store.save(&record).expect("save");
        let loaded = store
            .load(FIXTURE_PROFILE, FIXTURE_ACCOUNT)
            .expect("load");
        assert_eq!(loaded.profile_id, record.profile_id);
        assert_eq!(loaded.account_id, record.account_id);
        assert_eq!(loaded.access_token, record.access_token);
        assert_eq!(loaded.refresh_token, record.refresh_token);
        assert_eq!(
            loaded.access_token_expires_at_unix_seconds,
            record.access_token_expires_at_unix_seconds
        );
        assert_eq!(loaded.device_id, record.device_id);
        assert_eq!(
            loaded.last_updated_at_unix_seconds,
            record.last_updated_at_unix_seconds
        );
        assert!(store.exists(FIXTURE_PROFILE));
    }

    #[test]
    fn load_rejects_account_mismatch() {
        let root = tempdir().expect("tempdir");
        let store = FusedLoginStore::new(root.path());
        store.save(&fixture_record()).expect("save");
        // 错误断言不触发 Record 的 Debug（凭据不进失败输出）：先取 err。
        let error = store
            .load(FIXTURE_PROFILE, "other-account")
            .err()
            .expect("binding mismatch must fail");
        assert_eq!(error, FusedLoginStoreError::BindingMismatch);
    }

    #[test]
    fn load_rejects_missing_and_corrupted_files() {
        let root = tempdir().expect("tempdir");
        let store = FusedLoginStore::new(root.path());
        // 不存在的账号：容器视为无此代次（Invalid，非 Io）。
        assert_eq!(
            store.load(FIXTURE_PROFILE, FIXTURE_ACCOUNT).err(),
            Some(FusedLoginStoreError::Invalid)
        );
        // 魔数损坏。
        store.save(&fixture_record()).expect("save");
        let path = root.path().join(format!("{FIXTURE_PROFILE}.fused"));
        let mut payload = fs::read(&path).expect("read");
        payload[0] = b'X';
        fs::write(&path, &payload).expect("write");
        assert_eq!(
            store.load(FIXTURE_PROFILE, FIXTURE_ACCOUNT).err(),
            Some(FusedLoginStoreError::Invalid)
        );
        // 长度截断。
        payload.truncate(10);
        fs::write(&path, &payload).expect("write");
        assert_eq!(
            store.load(FIXTURE_PROFILE, FIXTURE_ACCOUNT).err(),
            Some(FusedLoginStoreError::Invalid)
        );
    }

    #[test]
    fn save_rejects_empty_identity_fields() {
        let root = tempdir().expect("tempdir");
        let store = FusedLoginStore::new(root.path());
        let mut record = fixture_record();
        record.access_token = String::new();
        assert_eq!(
            store.save(&record),
            Err(FusedLoginStoreError::Invalid)
        );
        assert!(!store.exists(FIXTURE_PROFILE));
    }

    #[test]
    fn update_generation_replaces_tokens_atomically() {
        let root = tempdir().expect("tempdir");
        let store = FusedLoginStore::new(root.path());
        store.save(&fixture_record()).expect("initial save");
        // 切出回存场景：客户端续期轮换后的最新代次整替换旧记录。
        let mut refreshed = fixture_record();
        refreshed.access_token = "fixture-access-token-v2".to_string();
        refreshed.refresh_token = "fixture-refresh-token-v2".to_string();
        refreshed.last_updated_at_unix_seconds = 1_800_000_000 + 3_600;
        store.save(&refreshed).expect("update");
        let loaded = store
            .load(FIXTURE_PROFILE, FIXTURE_ACCOUNT)
            .expect("load after update");
        assert_eq!(loaded.access_token, "fixture-access-token-v2");
        assert_eq!(loaded.refresh_token, "fixture-refresh-token-v2");
        assert_eq!(loaded.last_updated_at_unix_seconds, 1_800_000_000 + 3_600);
        // tmp 文件不残留。
        assert!(!root
            .path()
            .join(format!("{FIXTURE_PROFILE}.fused.tmp"))
            .exists());
    }

    #[test]
    fn list_returns_masked_summaries_without_tokens() {
        let root = tempdir().expect("tempdir");
        let store = FusedLoginStore::new(root.path());
        store.save(&fixture_record()).expect("save");
        let summaries = store.list().expect("list");
        assert_eq!(summaries.len(), 1);
        let summary = &summaries[0];
        assert_eq!(summary.profile_id, FIXTURE_PROFILE);
        assert_eq!(summary.account_id, FIXTURE_ACCOUNT);
        // 脱敏：摘要序列化产物不含任何令牌值。
        let json = serde_json::to_string(summary).expect("serialize summary");
        assert!(!json.contains("fixture-access-token"));
        assert!(!json.contains("fixture-refresh-token"));
    }

    #[test]
    fn list_skips_corrupted_files_and_non_fused_entries() {
        let root = tempdir().expect("tempdir");
        let store = FusedLoginStore::new(root.path());
        store.save(&fixture_record()).expect("save");
        // 损坏文件：跳过不拖垮清单。
        fs::write(root.path().join("checkin-broken0001.fused"), b"garbage").expect("write");
        // 非 .fused 扩展：不进清单。
        fs::write(root.path().join("notes.txt"), b"text").expect("write");
        let summaries = store.list().expect("list");
        assert_eq!(summaries.len(), 1);
        assert_eq!(summaries[0].profile_id, FIXTURE_PROFILE);
    }

    /// P17-A1：落盘为明文 JSON（首字节 `{`，format_version = 2），不再
    /// 经过 DPAPI 加密层。
    #[test]
    fn save_writes_plain_json_format() {
        let root = tempdir().expect("tempdir");
        let store = FusedLoginStore::new(root.path());
        let record = fixture_record();
        store.save(&record).expect("save");
        let path = root.path().join(format!("{FIXTURE_PROFILE}.fused"));
        let payload = fs::read(&path).expect("read");
        assert_eq!(payload.first(), Some(&b'{'), "明文 JSON 首字节应为 '{{'");
        let value: serde_json::Value = serde_json::from_slice(&payload).expect("valid json");
        assert_eq!(value["format_version"], FUSED_LOGIN_PLAIN_FORMAT_VERSION);
        assert_eq!(value["account_id"], FIXTURE_ACCOUNT);
        // 无旧容器魔数头。
        assert!(&payload[..8] != FUSED_LOGIN_MAGIC);
    }

    /// P17-A1 迁移：DPAPI 容器 → 明文 JSON；旧密文先备份到 legacy 目录；
    /// 幂等（二次运行为 already_plain）；迁移后读取结果一致。
    #[test]
    fn migrate_legacy_to_plain_is_idempotent_and_backs_up_legacy() {
        let record = fixture_record();
        let root = tempdir().expect("tempdir");
        let store = FusedLoginStore::new(root.path());
        let path = root.path().join(format!("{FIXTURE_PROFILE}.fused"));
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
        let legacy_backup =
            fs::read(backup_dir.join(format!("{FIXTURE_PROFILE}.fused"))).expect("legacy backup");
        assert_eq!(&legacy_backup[..8], FUSED_LOGIN_MAGIC, "备份为旧密文");
        // 迁移后读取一致。
        let loaded = store
            .load(FIXTURE_PROFILE, FIXTURE_ACCOUNT)
            .expect("load");
        assert_eq!(loaded.access_token, record.access_token);
        assert_eq!(loaded.device_id, record.device_id);

        // 幂等：二次运行全部跳过。
        let outcome2 = store
            .migrate_legacy_to_plain(Some(&backup_dir))
            .expect("migrate again");
        assert_eq!(outcome2.migrated, 0);
        assert_eq!(outcome2.already_plain, 1);
        assert!(outcome2.failed.is_empty());
    }

    /// P17-A1：损坏明文 JSON 拒绝（非法内容 / 版本不符）。
    #[test]
    fn load_rejects_corrupted_plain_json() {
        let root = tempdir().expect("tempdir");
        let store = FusedLoginStore::new(root.path());
        let record = fixture_record();
        store.save(&record).expect("save");
        let path = root.path().join(format!("{FIXTURE_PROFILE}.fused"));
        // 非 JSON 内容。
        fs::write(&path, b"garbage").expect("write");
        assert_eq!(
            store.load(FIXTURE_PROFILE, FIXTURE_ACCOUNT).err(),
            Some(FusedLoginStoreError::Invalid)
        );
        // 恢复合法明文后再篡改版本号（前一步已把文件写成坏样本）。
        store.save(&record).expect("restore plain");
        // JSON 合法但版本不符（=1 的明文形态不存在，拒绝）。
        let mut value: serde_json::Value =
            serde_json::from_slice(&fs::read(&path).expect("read")).expect("json");
        value["format_version"] = serde_json::json!(1);
        fs::write(&path, value.to_string()).expect("write");
        assert_eq!(
            store.load(FIXTURE_PROFILE, FIXTURE_ACCOUNT).err(),
            Some(FusedLoginStoreError::Invalid)
        );
    }

    /// 测试辅助：按旧 DPAPI 容器格式编码 fixture 记录（兼容读/迁移用）。
    fn encode_legacy_fixture(record: &FusedLoginRecord) -> Vec<u8> {
        let serialized = SerializableRecord {
            format_version: FUSED_LOGIN_FORMAT_VERSION,
            profile_id: record.profile_id.clone(),
            account_id: record.account_id.clone(),
            screen_name: record.screen_name.clone(),
            masked_mobile: record.masked_mobile.clone(),
            access_token: record.access_token.clone(),
            refresh_token: record.refresh_token.clone(),
            access_expires_at: record.access_token_expires_at_unix_seconds,
            refresh_expires_at: record.refresh_token_expires_at_unix_seconds,
            device_id: record.device_id.clone(),
            machine_id: record.machine_id.clone(),
            device_public_key: record.device_public_key.clone(),
            minted_at_unix_seconds: record.minted_at_unix_seconds,
            last_updated_at_unix_seconds: record.last_updated_at_unix_seconds,
        };
        let plaintext = serde_json::to_vec(&serialized).expect("serialize");
        let encrypted = protect(&plaintext).expect("dpapi protect");
        let mut payload = Vec::new();
        payload.extend_from_slice(FUSED_LOGIN_MAGIC);
        payload.extend_from_slice(&FUSED_LOGIN_FORMAT_VERSION.to_le_bytes());
        payload.extend_from_slice(&(encrypted.len() as u32).to_le_bytes());
        payload.extend_from_slice(&encrypted);
        payload
    }

    #[test]
    fn stage_one_probe_file_is_readable() {
        // 阶段一兼容：探针写出的明文 JSON 字段名（无 last_updated 字段）
        // 经 default 兼容读取，last_updated 回退铸造时刻。
        let record = fixture_record();
        let stage_one_json = serde_json::json!({
            "format_version": 1,
            "profile_id": record.profile_id,
            "account_id": record.account_id,
            "screen_name": record.screen_name,
            "masked_mobile": record.masked_mobile,
            "access_token": record.access_token,
            "refresh_token": record.refresh_token,
            "access_token_expires_at_unix_seconds": record.access_token_expires_at_unix_seconds,
            "refresh_token_expires_at_unix_seconds": record.refresh_token_expires_at_unix_seconds,
            "device_id": record.device_id,
            "machine_id": record.machine_id,
            "device_public_key": record.device_public_key,
            "minted_at_unix_seconds": record.minted_at_unix_seconds,
        });
        let plaintext = serde_json::to_vec(&stage_one_json).expect("serialize");
        let encrypted = protect(&plaintext).expect("dpapi protect");
        let mut payload = Vec::new();
        payload.extend_from_slice(FUSED_LOGIN_MAGIC);
        payload.extend_from_slice(&1u32.to_le_bytes());
        payload.extend_from_slice(&(encrypted.len() as u32).to_le_bytes());
        payload.extend_from_slice(&encrypted);
        let root = tempdir().expect("tempdir");
        let path = root.path().join(format!("{FIXTURE_PROFILE}.fused"));
        fs::write(&path, &payload).expect("write stage-one file");
        let store = FusedLoginStore::new(root.path());
        let loaded = store.load(FIXTURE_PROFILE, FIXTURE_ACCOUNT).expect("load");
        assert_eq!(loaded.access_token, record.access_token);
        // 旧文件缺 last_updated：回退铸造时刻。
        assert_eq!(loaded.last_updated_at_unix_seconds, record.minted_at_unix_seconds);
    }
}
