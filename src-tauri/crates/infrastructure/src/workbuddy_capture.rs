//! P17-T4c②：WorkBuddy 账号捕获编排（会话快照 → 解密 → 登录代次容器落盘）。
//!
//! 与 Trae 捕获同构（blob 捕获 → 容器铸造 → 档案登记）的 WorkBuddy 适配：
//! 数据源为客户端落盘会话（`%LOCALAPPDATA%\CodeBuddyExtension\...\auth`，
//! spec 8.1）+ 品牌身份文件（`~/.workbuddy` 根：account-snapshot.json 的
//! nickname、device-id），解密借客户端运行时（spec 8.4 面 A）。
//!
//! 纪律：
//! - 本模块对生产会话文件**只读**；写入仅发生在工具自有容器
//!   （workbuddy-login 根，DPAPI）；
//! - 编排入口（[`capture_workbuddy_login_from_machine`]）由组合根在用户
//!   单次确认后调用（铸造入池 = 写工具自有存储）；
//! - 明文 token 只在内存流转（快照/容器间传递），不进日志/DTO。

use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::workbuddy_credential::{
    decrypt_session_tokens, load_session_snapshot, unix_ms_now, WorkBuddyDecryptedTokens,
    WorkBuddySessionSnapshot, WorkBuddySessionStatus, DEFAULT_AUTH_ID,
};
use crate::workbuddy_login_credential::{WorkBuddyLoginRecord, WorkBuddyLoginStore};

/// WorkBuddy 数据根（`~/.workbuddy`，ACC dataFolderName）。
fn workbuddy_home() -> Option<PathBuf> {
    let profile = std::env::var("USERPROFILE").ok()?;
    if profile.trim().is_empty() {
        return None;
    }
    Some(Path::new(&profile).join(".workbuddy"))
}

/// 品牌身份（捕获记录的 screen_name 与 device_id 来源）。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Default)]
pub struct WorkBuddyAccountIdentity {
    /// 账号 uid（account-snapshot primary.uid；与会话快照 uid 交叉校验）。
    #[serde(default)]
    pub uid: String,
    /// 展示名（nickname；可空——客户端未写时不阻塞捕获）。
    #[serde(default, rename = "nickname")]
    pub screen_name: String,
}

/// 身份读取错误。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkBuddyIdentityError {
    /// USERPROFILE 不可解析。
    PathUnavailable,
    /// 文件缺失（客户端未初始化该文件）。
    FileMissing,
    /// 读取失败。
    Io,
    /// JSON 损坏。
    Malformed,
}

/// 读取品牌身份（account-snapshot.json 的 primary 节；只读）。
pub fn load_account_identity() -> Result<WorkBuddyAccountIdentity, WorkBuddyIdentityError> {
    load_account_identity_in(&workbuddy_home().ok_or(WorkBuddyIdentityError::PathUnavailable)?)
}

/// 纯路径版本（测试/隔离副本注入）。
pub fn load_account_identity_in(
    workbuddy_home: &Path,
) -> Result<WorkBuddyAccountIdentity, WorkBuddyIdentityError> {
    let path = workbuddy_home
        .join("storage")
        .join("skeleton")
        .join("account-snapshot.json");
    let content = fs::read_to_string(&path).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            WorkBuddyIdentityError::FileMissing
        } else {
            WorkBuddyIdentityError::Io
        }
    })?;
    let value: serde_json::Value =
        serde_json::from_str(&content).map_err(|_| WorkBuddyIdentityError::Malformed)?;
    let primary = value
        .get("primary")
        .ok_or(WorkBuddyIdentityError::Malformed)?;
    let identity: WorkBuddyAccountIdentity =
        serde_json::from_value(primary.clone()).map_err(|_| WorkBuddyIdentityError::Malformed)?;
    Ok(identity)
}

/// 读取机器级设备标识（`~/.workbuddy/device-id`；只读）。
pub fn load_device_id() -> Result<String, WorkBuddyIdentityError> {
    load_device_id_in(&workbuddy_home().ok_or(WorkBuddyIdentityError::PathUnavailable)?)
}

/// 纯路径版本。
pub fn load_device_id_in(workbuddy_home: &Path) -> Result<String, WorkBuddyIdentityError> {
    let content = fs::read_to_string(workbuddy_home.join("device-id")).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            WorkBuddyIdentityError::FileMissing
        } else {
            WorkBuddyIdentityError::Io
        }
    })?;
    let trimmed = content.trim();
    if trimmed.is_empty() {
        return Err(WorkBuddyIdentityError::Malformed);
    }
    Ok(trimmed.to_string())
}

/// 捕获来源（编排中间产物；明文 token 只在此结构内存流转）。
pub struct WorkBuddyCaptureSource {
    pub snapshot: WorkBuddySessionSnapshot,
    pub identity: WorkBuddyAccountIdentity,
    pub device_id: String,
    pub decrypted: WorkBuddyDecryptedTokens,
    /// P19.2② 脱敏手机号（accounts 快照 `phoneNumber`——服务端预脱敏形态
    /// 如 `133****1304`，工具直显不二次处理；best-effort 采集，失败为空）。
    pub masked_mobile: String,
}

/// 捕获编排错误。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkBuddyCaptureError {
    /// 会话不可用（缺失/登出/过期——非 Active 不捕获）。
    SessionNotCapturable(&'static str),
    /// 品牌身份读取失败。
    Identity(WorkBuddyIdentityError),
    /// 解密失败。
    Decrypt(String),
    /// uid 交叉校验不符（会话快照 vs account-snapshot——数据矛盾不猜测）。
    UidMismatch,
    /// 容器落盘失败。
    Store(String),
}

impl WorkBuddyCaptureError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::SessionNotCapturable(_) => "workbuddy_capture_session_not_capturable",
            Self::Identity(_) => "workbuddy_capture_identity_failed",
            Self::Decrypt(_) => "workbuddy_capture_decrypt_failed",
            Self::UidMismatch => "workbuddy_capture_uid_mismatch",
            Self::Store(_) => "workbuddy_capture_store_failed",
        }
    }
}

/// 纯构造：捕获来源 → 登录代次记录（幂等；now 为铸造时刻秒）。
///
/// 交叉校验：会话快照 uid 与 account-snapshot uid 必须一致（两份客户端
/// 自留数据互相印证，不一致 = 数据矛盾，宁可不捕获）。
pub fn build_workbuddy_login_record(
    source: &WorkBuddyCaptureSource,
    now_unix_seconds: u64,
) -> Result<WorkBuddyLoginRecord, WorkBuddyCaptureError> {
    if source.snapshot.uid != source.identity.uid {
        return Err(WorkBuddyCaptureError::UidMismatch);
    }
    // 毫秒 → 秒（信封明文元数据为毫秒 epoch）。
    let seconds = |ms: Option<u64>| ms.map(|value| value / 1000).unwrap_or(0);
    Ok(WorkBuddyLoginRecord {
        profile_id: WorkBuddyLoginRecord::profile_id_for(&source.snapshot.uid),
        account_id: source.snapshot.uid.clone(),
        screen_name: source.identity.screen_name.clone(),
        masked_mobile: source.masked_mobile.clone(),
        access_token: source.decrypted.access_token.clone(),
        refresh_token: source.decrypted.refresh_token.clone(),
        access_token_expires_at_unix_seconds: seconds(source.snapshot.auth.expires_at),
        refresh_token_expires_at_unix_seconds: seconds(source.snapshot.auth.refresh_expires_at),
        domain: source.snapshot.auth.domain.clone(),
        session_state: source.snapshot.auth.session_state.clone(),
        device_id: source.device_id.clone(),
        minted_at_unix_seconds: now_unix_seconds,
        last_updated_at_unix_seconds: now_unix_seconds,
    })
}

/// 全编排（从本机捕获）：会话快照（校验 Active）→ 品牌身份 → 设备标识 →
/// 解密 → 构造 → 容器落盘（写后复核由 store 保证）。
///
/// 调用纪律：组合根在用户**单次确认**后调用（铸造入池 = 写工具自有存储）；
/// 幂等可重跑（同 uid 再次捕获 = 整代次原子替换为最新）。
pub fn capture_workbuddy_login_from_machine(
    client_exe: &Path,
    login_store: &WorkBuddyLoginStore,
    now_unix_seconds: u64,
) -> Result<WorkBuddyLoginRecord, WorkBuddyCaptureError> {
    let snapshot = load_session_snapshot(DEFAULT_AUTH_ID)
        .map_err(|error| WorkBuddyCaptureError::SessionNotCapturable(error.code()))?;
    let status =
        crate::workbuddy_credential::current_session_status(DEFAULT_AUTH_ID, unix_ms_now());
    if status != WorkBuddySessionStatus::Active {
        return Err(WorkBuddyCaptureError::SessionNotCapturable(
            "session_not_active",
        ));
    }
    let identity = load_account_identity().map_err(WorkBuddyCaptureError::Identity)?;
    let device_id = load_device_id().map_err(WorkBuddyCaptureError::Identity)?;
    let info_path = crate::workbuddy_credential::session_file(DEFAULT_AUTH_ID)
        .ok_or(WorkBuddyCaptureError::SessionNotCapturable(
            "path_unavailable",
        ))?;
    let decrypted = decrypt_session_tokens(&info_path, client_exe)
        .map_err(|error| WorkBuddyCaptureError::Decrypt(error.code().to_string()))?;
    // P19.2② 脱敏手机号采集（best-effort 只读探查；失败不阻塞捕获）。
    let masked_mobile =
        fetch_masked_phone_best_effort(&snapshot.auth.domain, &decrypted.access_token, &snapshot.uid);
    let source = WorkBuddyCaptureSource {
        snapshot,
        identity,
        device_id,
        decrypted,
        masked_mobile,
    };
    let record = build_workbuddy_login_record(&source, now_unix_seconds)?;
    login_store
        .save(&record)
        .map_err(|error| WorkBuddyCaptureError::Store(error.to_string()))?;
    Ok(record)
}

/// P19.2② 脱敏手机号采集（best-effort 只读 GET /v2/plugin/accounts；
/// Bearer 取自本次解密凭据，token 不落日志；解析复用
/// workbuddy_login::parse_snapshot_accounts——快照 `phoneNumber` 为服务端
/// 预脱敏形态，工具直显；任何失败返回空串，不阻塞捕获主流程）。
fn fetch_masked_phone_best_effort(domain: &str, access_token: &str, uid: &str) -> String {
    use std::time::Duration;
    if domain.trim().is_empty() || access_token.trim().is_empty() || uid.trim().is_empty() {
        return String::new();
    }
    let url = format!("https://{}/v2/plugin/accounts", domain.trim());
    let client = match reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
    {
        Ok(client) => client,
        Err(_) => return String::new(),
    };
    let response = match client
        .get(&url)
        .header("Authorization", format!("Bearer {access_token}"))
        .header("X-Domain", domain.trim())
        .header("Content-Type", "application/json")
        .send()
    {
        Ok(response) if response.status().is_success() => response,
        _ => return String::new(),
    };
    let Ok(body) = response.text() else {
        return String::new();
    };
    crate::workbuddy_login::parse_snapshot_accounts(&body)
        .into_iter()
        .find(|account| account.uid == uid)
        .map(|account| account.masked_phone)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    const SAMPLE_SESSION: &str = r#"{
  "auth": {
    "accessToken": { "$wbEncrypted": 1, "envelope": "x" },
    "refreshToken": { "$wbEncrypted": 1, "envelope": "x" },
    "tokenType": "Bearer",
    "sessionState": "7e57e1c9-9d02-4281-ba26-f61b7e4c655f",
    "scope": "openid profile offline_access email",
    "domain": "www.workbuddy.cn",
    "lastRefreshTime": 1792219259894,
    "expiresAt": 1794638459316,
    "refreshExpiresAt": 1794811259316
  },
  "account": { "uid": "89b16d6b-0549-4e1e-8ced-918b5c8152ae", "uin": "330126449009" },
  "accounts": [ { "uid": "89b16d6b-0549-4e1e-8ced-918b5c8152ae" } ],
  "allAccounts": [ { "uid": "89b16d6b-0549-4e1e-8ced-918b5c8152ae" } ]
}"#;

    fn fixture_source() -> WorkBuddyCaptureSource {
        WorkBuddyCaptureSource {
            snapshot: crate::workbuddy_credential::parse_session_snapshot(SAMPLE_SESSION)
                .expect("parse fixture"),
            identity: WorkBuddyAccountIdentity {
                uid: "89b16d6b-0549-4e1e-8ced-918b5c8152ae".to_string(),
                screen_name: "梦梦".to_string(),
            },
            device_id: "2db49f20-2b3f-4761-ab53-9d213f361a5b".to_string(),
            decrypted: WorkBuddyDecryptedTokens {
                access_token: "fixture-access-jwt".to_string(),
                refresh_token: "fixture-refresh-jwt".to_string(),
                access_source: crate::workbuddy_credential::WorkBuddyTokenSource::Envelope,
                refresh_source: crate::workbuddy_credential::WorkBuddyTokenSource::Envelope,
            },
            masked_mobile: "133****1304".to_string(),
        }
    }

    /// 纯构造：记录字段齐备、毫秒→秒换算、profile_id 惯例。
    /// （Record 不实现 Debug——错误路径用 err().expect，成功路径 match。）
    #[test]
    fn build_record_maps_fields_and_converts_expiry() {
        let record = match build_workbuddy_login_record(&fixture_source(), 1_800_000_000) {
            Ok(record) => record,
            Err(error) => panic!("构造失败：{error:?}"),
        };
        assert_eq!(
            record.profile_id,
            "wb-89b16d6b-0549-4e1e-8ced-918b5c8152ae"
        );
        assert_eq!(record.account_id, "89b16d6b-0549-4e1e-8ced-918b5c8152ae");
        assert_eq!(record.screen_name, "梦梦");
        assert_eq!(record.access_token, "fixture-access-jwt");
        // 毫秒 → 秒（1794638459316 / 1000 = 1794638459）。
        assert_eq!(record.access_token_expires_at_unix_seconds, 1_794_638_459);
        assert_eq!(
            record.refresh_token_expires_at_unix_seconds,
            1_794_811_259
        );
        assert_eq!(record.domain, "www.workbuddy.cn");
        assert_eq!(record.device_id, "2db49f20-2b3f-4761-ab53-9d213f361a5b");
        // P19.2② 脱敏手机号透传（服务端预脱敏形态直存直显）。
        assert_eq!(record.masked_mobile, "133****1304");
        assert_eq!(record.minted_at_unix_seconds, 1_800_000_000);
    }

    /// uid 交叉校验：两份数据矛盾 → UidMismatch（宁可不捕获）。
    #[test]
    fn uid_mismatch_rejected() {
        let mut source = fixture_source();
        source.identity.uid = "other-uid".to_string();
        let error = build_workbuddy_login_record(&source, 1_800_000_000)
            .err()
            .expect("uid mismatch must fail");
        assert_eq!(error, WorkBuddyCaptureError::UidMismatch);
    }

    /// 编排落盘往返：构造 + store.save → load 回读一致（DPAPI roundtrip）。
    #[test]
    fn capture_record_saves_and_loads() {
        let record = match build_workbuddy_login_record(&fixture_source(), 1_800_000_000) {
            Ok(record) => record,
            Err(error) => panic!("构造失败：{error:?}"),
        };
        let root = tempdir().expect("tempdir");
        let store = WorkBuddyLoginStore::new(root.path());
        store.save(&record).expect("save");
        let loaded = store
            .load(&record.profile_id, record.account_id.as_str())
            .expect("load");
        assert_eq!(loaded.access_token, record.access_token);
        assert_eq!(loaded.screen_name, "梦梦");
        // 脱敏 summary 无令牌。
        let json = serde_json::to_string(&loaded.summary()).expect("summary");
        assert!(!json.contains("fixture-access-jwt"));
    }

    /// 品牌身份读取：临时根注入（缺文件 FileMissing / 正常解析）。
    #[test]
    fn identity_and_device_id_read_from_injected_root() {
        let root = tempdir().expect("tempdir");
        assert_eq!(
            load_account_identity_in(root.path()).unwrap_err(),
            WorkBuddyIdentityError::FileMissing
        );
        let skeleton = root
            .path()
            .join("storage")
            .join("skeleton")
            .join("account-snapshot.json");
        fs::create_dir_all(skeleton.parent().unwrap()).unwrap();
        fs::write(
            &skeleton,
            r#"{"primary":{"version":1,"uid":"89b16d6b-0549-4e1e-8ced-918b5c8152ae","nickname":"梦梦"}}"#,
        )
        .unwrap();
        let identity = load_account_identity_in(root.path()).unwrap();
        assert_eq!(identity.screen_name, "梦梦");
        // device-id。
        assert_eq!(
            load_device_id_in(root.path()).unwrap_err(),
            WorkBuddyIdentityError::FileMissing
        );
        fs::write(root.path().join("device-id"), "2db49f20-2b3f-4761-ab53-9d213f361a5b\n")
            .unwrap();
        assert_eq!(
            load_device_id_in(root.path()).unwrap(),
            "2db49f20-2b3f-4761-ab53-9d213f361a5b"
        );
    }
}
