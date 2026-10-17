//! P17-T4c 凭据层（准备阶段）：WorkBuddy 登录会话快照与健康检测（文件面）。
//!
//! P17-T4c 定位结论（spec 第 8 节）：登录会话在
//! `%LOCALAPPDATA%\CodeBuddyExtension\Data\Public\auth\{auth_id}.info`
//! （默认 `workbuddy-desktop.info`），敏感字段（accessToken/refreshToken/
//! nickname/phoneNumber）为 `$wbEncrypted` AES-256-GCM 信封（密钥仅存于
//! 客户端进程内存，来自魔改 Electron 原生 API——文件面无法解密），但
//! `uid/uin/expiresAt/refreshExpiresAt/scope/domain` 等**元数据为明文**。
//! 本模块只消费明文元数据：登录态健康检测不依赖密钥、不依赖客户端运行。
//!
//! 凭据捕获/账号切换走 wbipc RPC 面（`auth:getToken`/`auth:saveOauthToken`
//! 等，客户端运行中），见 examples/wbipc_probe_p17.rs 与 spec 8.4。

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Deserialize;

/// 默认会话标识（ACC 产品配置 authentication.id，P17-T4c 实测）。
pub const DEFAULT_AUTH_ID: &str = "workbuddy-desktop";
/// 存储根相对段（`%LOCALAPPDATA%` 之下）。
pub const AUTH_ROOT_SEGMENTS: &[&str] = &["CodeBuddyExtension", "Data", "Public", "auth"];
/// 登出标记后缀（FileAuthenticationStorage.writeLogoutMarker →
/// getLogoutMarkerPath = `${path}.logged-out`，asar 反编译实读）。
pub const LOGOUT_MARKER_SUFFIX: &str = ".logged-out";

/// 会话存储根（环境驱动）：`%LOCALAPPDATA%\CodeBuddyExtension\Data\Public\auth`。
/// 代码依据：FilePathServiceImpl.getBasePath win32 = `%LOCALAPPDATA%\{z._i}`，
/// `z._i = CodeBuddyExtension`（本机实测该目录存在且含 auth 会话文件）。
pub fn auth_dir() -> Option<PathBuf> {
    let local = std::env::var("LOCALAPPDATA").ok()?;
    if local.trim().is_empty() {
        return None;
    }
    Some(auth_dir_in(Path::new(&local)))
}

/// 纯路径构造（给定 `%LOCALAPPDATA%` 根；测试注入用）。
pub fn auth_dir_in(localappdata_root: &Path) -> PathBuf {
    let mut path = localappdata_root.to_path_buf();
    for segment in AUTH_ROOT_SEGMENTS {
        path.push(segment);
    }
    path
}

/// 会话文件路径（`{auth_dir}/{auth_id}.info`）。
pub fn session_file(auth_id: &str) -> Option<PathBuf> {
    auth_dir().map(|dir| session_file_in(&dir, auth_id))
}

/// 纯路径构造版本。
pub fn session_file_in(auth_dir: &Path, auth_id: &str) -> PathBuf {
    auth_dir.join(format!("{auth_id}.info"))
}

/// 登出标记路径（`{会话文件}.logged-out`，存在 = 已登出）。
pub fn logout_marker_file(auth_id: &str) -> Option<PathBuf> {
    session_file(auth_id).map(|path| logout_marker_file_in(&path))
}

/// 纯路径构造版本。
pub fn logout_marker_file_in(session_path: &Path) -> PathBuf {
    let mut name = session_path
        .file_name()
        .map(|name| name.to_os_string())
        .unwrap_or_default();
    name.push(LOGOUT_MARKER_SUFFIX);
    session_path.with_file_name(name)
}

/// 加密字段信封：`{$wbEncrypted: 1, envelope: <base64>}`（值不解析，
/// 只识别形态）。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct EncryptedField {
    #[serde(rename = "$wbEncrypted", default)]
    pub wb_encrypted: i64,
    /// 信封 base64（内层 suite/keyId/nonce/authTag/ciphertext JSON），本模块不触碰。
    #[serde(default)]
    pub envelope: String,
}

impl EncryptedField {
    fn is_encrypted(value: &serde_json::Value) -> bool {
        value
            .get("$wbEncrypted")
            .and_then(|flag| flag.as_i64())
            .map(|flag| flag == 1)
            .unwrap_or(false)
    }
}

/// `auth` 段明文元数据（token 值加密，不解析；JSON 键为 camelCase）。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct WorkBuddyAuthMeta {
    #[serde(default)]
    pub token_type: String,
    #[serde(default)]
    pub scope: String,
    #[serde(default)]
    pub domain: String,
    #[serde(default)]
    pub session_state: String,
    /// access 有效期（秒，实测 2419200 = 28 天）。
    #[serde(default)]
    pub expires_in: Option<u64>,
    /// refresh 有效期（秒，实测 2592000 = 30 天）。
    #[serde(default)]
    pub refresh_expires_in: Option<u64>,
    /// 最近刷新（ms epoch）。
    #[serde(default)]
    pub last_refresh_time: Option<u64>,
    /// access 过期时刻（ms epoch）。
    #[serde(default)]
    pub expires_at: Option<u64>,
    /// refresh 过期时刻（ms epoch）。
    #[serde(default)]
    pub refresh_expires_at: Option<u64>,
}

/// 登录会话快照（.info 文件明文面；敏感值永不落本结构）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkBuddySessionSnapshot {
    /// 当前账号 uid（本地 UUID，实测 89b16d6b-…）。
    pub uid: String,
    /// 当前账号 uin（12 位数字字符串；可为空串）。
    pub uin: String,
    pub auth: WorkBuddyAuthMeta,
    /// accessToken 是否为加密信封形态（值不读取）。
    pub has_encrypted_access_token: bool,
    pub has_encrypted_refresh_token: bool,
    /// accounts 数组条目数。
    pub account_count: usize,
    /// allAccounts 的 uid 列表（多账号并存观测）。
    pub all_account_uids: Vec<String>,
}

/// 会话快照读取错误。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkBuddySessionError {
    /// 存储根/会话文件路径不可解析（LOCALAPPDATA 缺失等）。
    PathUnavailable,
    /// 会话文件不存在（未登录过或已清理）。
    FileMissing,
    /// 文件读取失败。
    Io,
    /// JSON 损坏或形态不符。
    Malformed,
}

impl WorkBuddySessionError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::PathUnavailable => "workbuddy_session_path_unavailable",
            Self::FileMissing => "workbuddy_session_file_missing",
            Self::Io => "workbuddy_session_io",
            Self::Malformed => "workbuddy_session_malformed",
        }
    }
}

/// 读取并解析会话快照（只读；生产文件零写入）。
pub fn load_session_snapshot(
    auth_id: &str,
) -> Result<WorkBuddySessionSnapshot, WorkBuddySessionError> {
    let dir = auth_dir().ok_or(WorkBuddySessionError::PathUnavailable)?;
    load_session_snapshot_in(&dir, auth_id)
}

/// 纯路径版本（测试/隔离副本注入用）。
pub fn load_session_snapshot_in(
    auth_dir: &Path,
    auth_id: &str,
) -> Result<WorkBuddySessionSnapshot, WorkBuddySessionError> {
    let path = session_file_in(auth_dir, auth_id);
    let metadata = fs::metadata(&path).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            WorkBuddySessionError::FileMissing
        } else {
            WorkBuddySessionError::Io
        }
    })?;
    if !metadata.is_file() {
        return Err(WorkBuddySessionError::FileMissing);
    }
    let content = fs::read_to_string(&path).map_err(|_| WorkBuddySessionError::Io)?;
    parse_session_snapshot(&content)
}

/// 解析会话 JSON（结构与字段形态不符 → Malformed，不猜测）。
pub fn parse_session_snapshot(
    content: &str,
) -> Result<WorkBuddySessionSnapshot, WorkBuddySessionError> {
    let value: serde_json::Value =
        serde_json::from_str(content).map_err(|_| WorkBuddySessionError::Malformed)?;
    if !value.is_object() {
        return Err(WorkBuddySessionError::Malformed);
    }
    let auth: WorkBuddyAuthMeta =
        serde_json::from_value(value.get("auth").cloned().unwrap_or_default())
            .map_err(|_| WorkBuddySessionError::Malformed)?;
    let account: serde_json::Value = value.get("account").cloned().unwrap_or_default();
    let uid = account
        .get("uid")
        .and_then(|uid| uid.as_str())
        .unwrap_or_default()
        .to_string();
    let uin = account
        .get("uin")
        .and_then(|uin| uin.as_str())
        .unwrap_or_default()
        .to_string();
    let accounts = value
        .get("accounts")
        .and_then(|accounts| accounts.as_array())
        .ok_or(WorkBuddySessionError::Malformed)?;
    let all_accounts = value
        .get("allAccounts")
        .and_then(|accounts| accounts.as_array())
        .ok_or(WorkBuddySessionError::Malformed)?;
    Ok(WorkBuddySessionSnapshot {
        uid,
        uin,
        has_encrypted_access_token: value
            .get("auth")
            .and_then(|auth| auth.get("accessToken"))
            .map(EncryptedField::is_encrypted)
            .unwrap_or(false),
        has_encrypted_refresh_token: value
            .get("auth")
            .and_then(|auth| auth.get("refreshToken"))
            .map(EncryptedField::is_encrypted)
            .unwrap_or(false),
        account_count: accounts.len(),
        all_account_uids: all_accounts
            .iter()
            .filter_map(|entry| entry.get("uid").and_then(|uid| uid.as_str()))
            .map(str::to_string)
            .collect(),
        auth,
    })
}

/// 登录态判定（快照 + 登出标记存在性 + 当前时刻，ms epoch）。
///
/// 语义与客户端 FileAuthenticationStorage.restore 一致：登出标记命中即
/// 视为未登录（忽略存储的会话内容）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkBuddySessionStatus {
    /// 会话文件不存在。
    FileMissing,
    /// 登出标记存在（`.logged-out`）。
    LoggedOut,
    /// access 与 refresh 均已过期（须重新登录）。
    FullyExpired,
    /// access 过期但 refresh 仍有效（客户端 daemon 运行中会自动刷新）。
    AccessExpired,
    /// 登录中且 access 未过期。
    Active,
    /// 快照不可用（读取/解析失败）。
    Unknown,
}

/// 纯逻辑判定（文件系统探测由调用方注入：marker_exists）。
pub fn session_status_with(
    snapshot: Option<&WorkBuddySessionSnapshot>,
    marker_exists: bool,
    now_ms: u64,
) -> WorkBuddySessionStatus {
    if marker_exists {
        return WorkBuddySessionStatus::LoggedOut;
    }
    let Some(snapshot) = snapshot else {
        return WorkBuddySessionStatus::FileMissing;
    };
    let access_expired = snapshot.auth.expires_at.is_none_or(|at| at <= now_ms);
    let refresh_expired = snapshot
        .auth
        .refresh_expires_at
        .is_none_or(|at| at <= now_ms);
    match (access_expired, refresh_expired) {
        (true, true) => WorkBuddySessionStatus::FullyExpired,
        (true, false) => WorkBuddySessionStatus::AccessExpired,
        (false, _) => WorkBuddySessionStatus::Active,
    }
}

/// 组合入口：读快照 + 探测标记 + 判定（错误折叠进 Unknown/FileMissing）。
pub fn current_session_status(auth_id: &str, now_ms: u64) -> WorkBuddySessionStatus {
    let Some(dir) = auth_dir() else {
        return WorkBuddySessionStatus::FileMissing;
    };
    let marker_exists = logout_marker_file_in(&session_file_in(&dir, auth_id)).exists();
    match load_session_snapshot_in(&dir, auth_id) {
        Ok(snapshot) => session_status_with(Some(&snapshot), marker_exists, now_ms),
        Err(WorkBuddySessionError::FileMissing) => {
            session_status_with(None, marker_exists, now_ms)
        }
        Err(_) => WorkBuddySessionStatus::Unknown,
    }
}

/// 当前毫秒 epoch（默认时钟源）。
pub fn unix_ms_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 真机实测形态的合成样例（字段名/值形态与隔离副本一致，值替换）。
    const SAMPLE: &str = r#"{
  "auth": {
    "accessToken": { "$wbEncrypted": 1, "envelope": "eyJzdWl0ZSI6MX0" },
    "expiresIn": 2419200,
    "refreshExpiresIn": 2592000,
    "refreshToken": { "$wbEncrypted": 1, "envelope": "eyJzdWl0ZSI6MX0" },
    "tokenType": "Bearer",
    "notBeforePolicy": 1724292326,
    "sessionState": "7e57e1c9-9d02-4281-ba26-f61b7e4c655f",
    "scope": "openid profile offline_access email",
    "domain": "www.workbuddy.cn",
    "lastRefreshTime": 1792219259894,
    "expiresAt": 1794638459316,
    "refreshExpiresAt": 1794811259316
  },
  "account": {
    "uid": "89b16d6b-0549-4e1e-8ced-918b5c8152ae",
    "nickname": { "$wbEncrypted": 1, "envelope": "x" },
    "uin": "330126449009",
    "type": "",
    "lastLogin": true
  },
  "accounts": [ { "uid": "89b16d6b-0549-4e1e-8ced-918b5c8152ae", "uin": "330126449009" } ],
  "allAccounts": [ { "uid": "89b16d6b-0549-4e1e-8ced-918b5c8152ae" } ]
}"#;

    #[test]
    fn parse_extracts_plaintext_metadata_and_encrypted_flags() {
        let snapshot = parse_session_snapshot(SAMPLE).unwrap();
        assert_eq!(snapshot.uid, "89b16d6b-0549-4e1e-8ced-918b5c8152ae");
        assert_eq!(snapshot.uin, "330126449009");
        assert_eq!(snapshot.auth.token_type, "Bearer");
        assert_eq!(snapshot.auth.domain, "www.workbuddy.cn");
        assert_eq!(snapshot.auth.expires_in, Some(2419200));
        assert_eq!(snapshot.auth.expires_at, Some(1794638459316));
        assert_eq!(snapshot.auth.refresh_expires_at, Some(1794811259316));
        assert!(snapshot.has_encrypted_access_token);
        assert!(snapshot.has_encrypted_refresh_token);
        assert_eq!(snapshot.account_count, 1);
        assert_eq!(
            snapshot.all_account_uids,
            vec!["89b16d6b-0549-4e1e-8ced-918b5c8152ae"]
        );
    }

    #[test]
    fn status_active_before_expiry_and_expired_after() {
        let snapshot = parse_session_snapshot(SAMPLE).unwrap();
        // access 过期前（refresh 必然未过期）。
        assert_eq!(
            session_status_with(Some(&snapshot), false, 1792230528000),
            WorkBuddySessionStatus::Active
        );
        // access 过期、refresh 未过期。
        assert_eq!(
            session_status_with(Some(&snapshot), false, 1794700000000),
            WorkBuddySessionStatus::AccessExpired
        );
        // 双过期。
        assert_eq!(
            session_status_with(Some(&snapshot), false, 1794900000000),
            WorkBuddySessionStatus::FullyExpired
        );
    }

    /// 登出标记优先于快照内容（与客户端 restore 语义一致）。
    #[test]
    fn logout_marker_takes_precedence() {
        let snapshot = parse_session_snapshot(SAMPLE).unwrap();
        assert_eq!(
            session_status_with(Some(&snapshot), true, 1792230528000),
            WorkBuddySessionStatus::LoggedOut
        );
        // 无快照 + 无标记 = FileMissing；无快照 + 有标记 = LoggedOut。
        assert_eq!(
            session_status_with(None, false, 1792230528000),
            WorkBuddySessionStatus::FileMissing
        );
        assert_eq!(
            session_status_with(None, true, 1792230528000),
            WorkBuddySessionStatus::LoggedOut
        );
    }

    #[test]
    fn malformed_and_non_object_rejected() {
        assert_eq!(
            parse_session_snapshot("not json").unwrap_err(),
            WorkBuddySessionError::Malformed
        );
        assert_eq!(
            parse_session_snapshot("[1,2]").unwrap_err(),
            WorkBuddySessionError::Malformed
        );
        // 缺 accounts/allAccounts 数组 → Malformed（不猜测）。
        assert_eq!(
            parse_session_snapshot(r#"{"auth":{},"account":{}}"#).unwrap_err(),
            WorkBuddySessionError::Malformed
        );
    }

    /// 文件面组合入口：临时目录注入（不动 LOCALAPPDATA 环境变量）。
    #[test]
    fn load_from_injected_dir_and_status_roundtrip() {
        let root = tempfile::tempdir().unwrap();
        let dir = auth_dir_in(root.path());
        assert_eq!(
            dir,
            root.path()
                .join("CodeBuddyExtension")
                .join("Data")
                .join("Public")
                .join("auth")
        );
        // 文件缺失（目录已建但无会话文件）。
        fs::create_dir_all(&dir).unwrap();
        assert_eq!(
            load_session_snapshot_in(&dir, DEFAULT_AUTH_ID).unwrap_err(),
            WorkBuddySessionError::FileMissing
        );
        // 写入样例 → Active；加标记 → LoggedOut；删除文件留标记 → LoggedOut。
        fs::write(session_file_in(&dir, DEFAULT_AUTH_ID), SAMPLE).unwrap();
        let marker = logout_marker_file_in(&session_file_in(&dir, DEFAULT_AUTH_ID));
        assert!(marker
            .file_name()
            .unwrap()
            .to_string_lossy()
            .ends_with(&format!("{DEFAULT_AUTH_ID}.info{LOGOUT_MARKER_SUFFIX}")));
        let snapshot = load_session_snapshot_in(&dir, DEFAULT_AUTH_ID).unwrap();
        assert_eq!(
            session_status_with(Some(&snapshot), marker.exists(), 1792230528000),
            WorkBuddySessionStatus::Active
        );
        fs::write(&marker, "2026-10-17T00:00:00.000Z").unwrap();
        assert_eq!(
            session_status_with(Some(&snapshot), marker.exists(), 1792230528000),
            WorkBuddySessionStatus::LoggedOut
        );
        fs::remove_file(session_file_in(&dir, DEFAULT_AUTH_ID)).unwrap();
        assert_eq!(
            session_status_with(None, marker.exists(), 1792230528000),
            WorkBuddySessionStatus::LoggedOut
        );
    }
}
