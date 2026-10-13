//! 融合版登录代次容器（ADR-0035）：每账号一套融合版 TRAE 客户端登录凭据
//! （TRAE 通道 `ono9krqynydwx5`/IDE_PC，绑定机器级 ICDRS 设备身份）。
//!
//! 与签到凭据包（`checkin_credential`，SOLO 通道）是**双通道并行**的两套
//! 材料：本模块只读写自己的容器文件（`{root}/{profile_id}.fused`），绝不
//! 触碰 `.checkin` 文件；签到链路也绝不触碰本容器（跨通道复制禁令）。
//!
//! 格式（与阶段一迁移探针 `unified_mint_probe --save-only` 落盘文件逐字节
//! 兼容）：`magic(8) || format_version(u32 LE) || payload_len(u32 LE) ||
//! DPAPI 密文`；明文为 JSON 记录（`SerializableRecord`）。
//!
//! 纪律（与 `checkin_credential` 同构）：
//! - 整代次原子替换（tmp + rename，ADR-0019 第 7 条同纪律）；
//! - 任何读取都校验 `profile_id ↔ account_id` 绑定，不符零返回；
//! - 令牌不进入 Debug/日志/非脱敏 DTO（本类型不实现 Debug）；
//! - 续期由融合版客户端自动完成，工具只做整代次回存更新（P15-5 切出
//!   回存），不主动发起任何刷新请求。

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

// DPAPI 原语与错误类型：key_wrapper 的 crate 内共享实现（ADR-0035 复用）。
use crate::key_wrapper::{protect, unprotect};
use traesync_ports::KeyWrapperError;

/// 容器魔数：与阶段一探针一致（`TSFUSED1`）。
pub const FUSED_LOGIN_MAGIC: &[u8; 8] = b"TSFUSED1";
/// 容器格式版本。
pub const FUSED_LOGIN_FORMAT_VERSION: u32 = 1;
/// 单容器密文上限（防异常膨胀；正常记录约 2KB）。
const MAX_PAYLOAD_BYTES: u64 = 64 * 1024;
/// 头部长度：magic(8) + version(4) + payload_len(4)。
const HEADER_LEN: usize = 16;

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

    /// 整代次原子写入（铸造落盘与切出回存共用；tmp + rename）。
    pub fn save(&self, record: &FusedLoginRecord) -> Result<(), FusedLoginStoreError> {
        record.validate()?;
        let payload = encode_payload(record)?;
        fs::create_dir_all(&self.root).map_err(|_| FusedLoginStoreError::Io)?;
        let destination = self.record_path(&record.profile_id);
        let temporary = self.root.join(format!(
            "{}.fused.tmp",
            record.profile_id
        ));
        fs::write(&temporary, &payload).map_err(|_| FusedLoginStoreError::Io)?;
        fs::rename(&temporary, &destination).map_err(|_| FusedLoginStoreError::Io)?;
        // 写后复核：解密回读并校验账号一致（写入半成功即在此暴露）。
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

    /// 解密读取并校验绑定；任何失败不产生写副作用。
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
        let record = decode_payload(&payload)?;
        if record.profile_id != profile_id || record.account_id != expected_account_id {
            return Err(FusedLoginStoreError::BindingMismatch);
        }
        Ok(record)
    }

    /// 容器是否存在（不解密；用于清单展示与迁移判断）。
    pub fn exists(&self, profile_id: &str) -> bool {
        self.record_path(profile_id).is_file()
    }

    /// 脱敏清单：扫描容器目录全部 `.fused` 文件并解密取摘要。
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
            if let Ok(record) = decode_payload(&payload) {
                summaries.push(record.summary());
            }
        }
        summaries.sort_by(|a, b| a.profile_id.cmp(&b.profile_id));
        Ok(summaries)
    }
}

fn encode_payload(record: &FusedLoginRecord) -> Result<Vec<u8>, FusedLoginStoreError> {
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
    let plaintext =
        serde_json::to_vec(&serialized).map_err(|_| FusedLoginStoreError::Invalid)?;
    let encrypted = protect(&plaintext).map_err(map_key_wrapper_error)?;
    if encrypted.is_empty() || encrypted.len() as u64 > MAX_PAYLOAD_BYTES {
        return Err(FusedLoginStoreError::Invalid);
    }
    let mut payload = Vec::with_capacity(HEADER_LEN + encrypted.len());
    payload.extend_from_slice(FUSED_LOGIN_MAGIC);
    payload.extend_from_slice(&FUSED_LOGIN_FORMAT_VERSION.to_le_bytes());
    payload.extend_from_slice(&(encrypted.len() as u32).to_le_bytes());
    payload.extend_from_slice(&encrypted);
    Ok(payload)
}

fn decode_payload(payload: &[u8]) -> Result<FusedLoginRecord, FusedLoginStoreError> {
    if payload.len() <= HEADER_LEN || &payload[..8] != FUSED_LOGIN_MAGIC {
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
        || payload.len() != HEADER_LEN + encrypted_len
    {
        return Err(FusedLoginStoreError::Invalid);
    }
    let plaintext = unprotect(&payload[HEADER_LEN..]).map_err(map_key_wrapper_error)?;
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
