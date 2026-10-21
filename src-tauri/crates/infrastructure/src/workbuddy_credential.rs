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
///
/// P19.1 路径回退加固：LOCALAPPDATA 之下无 auth 目录（或环境变量缺失）时
/// 回退探测 `%APPDATA%` 同构路径（非标安装/便携形态的多路径实践）；两者
/// 均不可探测时保持 LOCALAPPDATA 口径（既有错误码语义不变——文件缺失即
/// [`WorkBuddySessionError::FileMissing`]，不猜路径形态）。
pub fn auth_dir() -> Option<PathBuf> {
    let local = non_empty_env("LOCALAPPDATA");
    let roaming = non_empty_env("APPDATA");
    let mut roots: Vec<PathBuf> = Vec::new();
    if let Some(local) = local {
        roots.push(PathBuf::from(local));
    }
    if let Some(roaming) = roaming {
        roots.push(PathBuf::from(roaming));
    }
    auth_dir_from_candidates(&roots)
}

/// 空白环境变量折叠为 None（既有 LOCALAPPDATA 空串防御的泛化）。
fn non_empty_env(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

/// 纯选择逻辑（候选根依序探测；测试注入用）：第一个 auth 目录存在的根
/// 胜出；全部不存在时保留首个候选（缺省口径，错误码语义不变）。
fn auth_dir_from_candidates(roots: &[PathBuf]) -> Option<PathBuf> {
    for root in roots {
        let candidate = auth_dir_in(root);
        if candidate.is_dir() {
            return Some(candidate);
        }
    }
    roots.first().map(|root| auth_dir_in(root))
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

// ===== P17-T4c 凭据解密（面 A：客户端运行时取钥 + AES-256-GCM 信封解密）=====

/// 解密 helper（内嵌随编译产物；在 WorkBuddy.exe 的 ELECTRON_RUN_AS_NODE
/// 模式下执行）。安全边界：atRestSecretKey 只存在于 node 子进程内存，
/// 明文 token 仅经 stdout 管道返回工具进程——不落盘、不进日志。
pub const DECRYPT_HELPER_SCRIPT: &str = include_str!("workbuddy_decrypt_helper.cjs");

/// token 来源形态（双形态兼容：5.5.x 明文 / 5.6.2+ 加密信封）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkBuddyTokenSource {
    /// 旧版明文字符串（无需解密）。
    Plain,
    /// 加密信封（经客户端运行时取钥解密）。
    Envelope,
}

/// 解密后的会话凭据（明文；调用方负责按凭据容器纪律立即铸造/加密，禁止
/// 落日志或导出）。
#[derive(Debug, Clone)]
pub struct WorkBuddyDecryptedTokens {
    pub access_token: String,
    pub refresh_token: String,
    pub access_source: WorkBuddyTokenSource,
    pub refresh_source: WorkBuddyTokenSource,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkBuddyDecryptError {
    /// 客户端 exe 不存在（取钥必须借 WorkBuddy 自己的运行时）。
    ClientExeMissing,
    /// 临时脚本目录创建/写入失败。
    TempScriptFailed,
    /// 子进程启动失败。
    SpawnFailed,
    /// helper 返回失败（取钥失败/keyId 不匹配/GCM 校验失败等；错误串来自
    /// helper，不含密钥或 token 明文）。
    HelperFailed(String),
    /// stdout 不是约定的 JSON 形态。
    OutputInvalid,
}

impl WorkBuddyDecryptError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::ClientExeMissing => "workbuddy_decrypt_client_exe_missing",
            Self::TempScriptFailed => "workbuddy_decrypt_temp_script_failed",
            Self::SpawnFailed => "workbuddy_decrypt_spawn_failed",
            Self::HelperFailed(_) => "workbuddy_decrypt_helper_failed",
            Self::OutputInvalid => "workbuddy_decrypt_output_invalid",
        }
    }
}

/// 解密会话文件的 token 字段（只读；借 WorkBuddy.exe 自己的运行时取钥）。
///
/// 流程：发布内嵌 helper 到独立临时目录（RAII 即删，P15 readonly 泄漏
/// 修复同纪律）→ `ELECTRON_RUN_AS_NODE=1 WorkBuddy.exe helper.cjs <info>`
/// → 解析 stdout JSON。客户端无需运行（exe 静态存在即可）；失败零副作用
/// （临时目录即删，生产会话文件只读）。
pub fn decrypt_session_tokens(
    info_path: &Path,
    client_exe: &Path,
) -> Result<WorkBuddyDecryptedTokens, WorkBuddyDecryptError> {
    use std::process::Command;
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    if !client_exe.is_file() {
        return Err(WorkBuddyDecryptError::ClientExeMissing);
    }
    // 专用临时父目录 + RAII 即删（TEMP 稳态归零纪律；.cjs 后缀避开
    // 仓库 package.json 的 "type":"module" ESM 误判）。
    let unique = format!(
        "traesync-wb-decrypt-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0)
    );
    let temp_dir = std::env::temp_dir().join(unique);
    struct TempDirGuard(PathBuf);
    impl Drop for TempDirGuard {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    if fs::create_dir_all(&temp_dir).is_err() {
        return Err(WorkBuddyDecryptError::TempScriptFailed);
    }
    let _guard = TempDirGuard(temp_dir.clone());
    let script_path = temp_dir.join("wb_decrypt_helper.cjs");
    if fs::write(&script_path, DECRYPT_HELPER_SCRIPT).is_err() {
        return Err(WorkBuddyDecryptError::TempScriptFailed);
    }

    let output = Command::new(client_exe)
        .env("ELECTRON_RUN_AS_NODE", "1")
        .arg(&script_path)
        .arg(info_path)
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map_err(|_| WorkBuddyDecryptError::SpawnFailed)?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: serde_json::Value =
        serde_json::from_str(stdout.trim()).map_err(|_| WorkBuddyDecryptError::OutputInvalid)?;
    if parsed.get("ok").and_then(|ok| ok.as_bool()) != Some(true) {
        let reason = parsed
            .get("error")
            .and_then(|error| error.as_str())
            .unwrap_or("unknown helper failure")
            .to_string();
        return Err(WorkBuddyDecryptError::HelperFailed(reason));
    }
    let access_token = parsed
        .get("access_token")
        .and_then(|token| token.as_str())
        .ok_or(WorkBuddyDecryptError::OutputInvalid)?
        .to_string();
    let refresh_token = parsed
        .get("refresh_token")
        .and_then(|token| token.as_str())
        .ok_or(WorkBuddyDecryptError::OutputInvalid)?
        .to_string();
    let source = |name: &str| {
        match parsed.get(name).and_then(|value| value.as_str()) {
            Some("plain") => WorkBuddyTokenSource::Plain,
            _ => WorkBuddyTokenSource::Envelope,
        }
    };
    Ok(WorkBuddyDecryptedTokens {
        access_token,
        refresh_token,
        access_source: source("access_source"),
        refresh_source: source("refresh_source"),
    })
}

/// 解析 JWT payload claims（base64url 第二段；仅供健康复核/脱敏展示，
/// 不校验签名——签名校验属在线复核阶段）。
pub fn jwt_payload_claims(token: &str) -> Option<serde_json::Value> {
    let payload_b64 = token.split('.').nth(1)?;
    let decoded = base64_url_decode(payload_b64)?;
    serde_json::from_slice(&decoded).ok()
}

/// 无 padding 宽容的 base64url 解码（JWT 惯例：去掉 padding、`-_` 字母表）。
fn base64_url_decode(input: &str) -> Option<Vec<u8>> {
    use base64::Engine;
    base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(input.trim_end_matches('='))
        .ok()
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

    /// helper 脚本静态断言：取钥绑定、AES-GCM、AAD、双形态兼容分支齐备。
    #[test]
    fn decrypt_helper_script_contains_required_parts() {
        assert!(DECRYPT_HELPER_SCRIPT.contains("electron_browser_workbuddy_storage"));
        assert!(DECRYPT_HELPER_SCRIPT.contains("loggerGet"));
        assert!(DECRYPT_HELPER_SCRIPT.contains("aes-256-gcm"));
        assert!(DECRYPT_HELPER_SCRIPT.contains("WB-AAD"));
        assert!(DECRYPT_HELPER_SCRIPT.contains("$wbEncrypted"));
        // 明文透传分支（旧版兼容）。
        assert!(DECRYPT_HELPER_SCRIPT.contains("typeof field === 'string'"));
    }

    /// JWT payload 解析：合法 base64url 段解码出 claims；非 JWT 拒绝。
    #[test]
    fn jwt_payload_claims_parses_and_rejects() {
        // 构造：header.payload.signature（payload={"uid":"u1","exp":123}）。
        use base64::Engine;
        let encode =
            |bytes: &[u8]| base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes);
        let payload = serde_json::json!({"uid": "u1", "exp": 123_u64});
        let token = format!(
            "{}.{}.{}",
            encode(br#"{"alg":"RS256"}"#),
            encode(payload.to_string().as_bytes()),
            "sig"
        );
        let claims = jwt_payload_claims(&token).unwrap();
        assert_eq!(claims.get("uid").and_then(|uid| uid.as_str()), Some("u1"));
        assert_eq!(claims.get("exp").and_then(|exp| exp.as_u64()), Some(123));
        // 非 JWT（无两段以上）拒绝。
        assert!(jwt_payload_claims("not-a-jwt").is_none());
    }

    /// 客户端 exe 缺失 → ClientExeMissing（取钥前置检查，零副作用）。
    #[test]
    fn decrypt_rejects_missing_client_exe() {
        let root = tempfile::tempdir().unwrap();
        let error = decrypt_session_tokens(
            root.path().join("info.json").as_path(),
            root.path().join("no-such.exe").as_path(),
        )
        .unwrap_err();
        assert_eq!(error, WorkBuddyDecryptError::ClientExeMissing);
    }

    /// P19.1 路径回退选择：首个 auth 目录存在的候选胜出；全部缺失时
    /// 保留首个候选口径（不猜、错误码语义不变）；空候选列表 → None。
    #[test]
    fn auth_dir_candidates_fallback_selection() {
        let primary = tempfile::tempdir().unwrap();
        let secondary = tempfile::tempdir().unwrap();
        // 两个候选都无 auth 目录 → 保留首个（LOCALAPPDATA 口径）。
        let roots = vec![primary.path().to_path_buf(), secondary.path().to_path_buf()];
        assert_eq!(
            auth_dir_from_candidates(&roots),
            Some(auth_dir_in(primary.path())),
            "全部缺失保留首个候选"
        );
        assert_eq!(auth_dir_from_candidates(&[]), None);
        // 首个候选补上 auth 目录 → 胜出。
        std::fs::create_dir_all(auth_dir_in(primary.path())).unwrap();
        assert_eq!(
            auth_dir_from_candidates(&roots),
            Some(auth_dir_in(primary.path()))
        );
        // 首个移除 auth 目录、第二个补上 → 回退到 %APPDATA% 同构候选。
        std::fs::remove_dir_all(auth_dir_in(primary.path())).unwrap();
        std::fs::create_dir_all(auth_dir_in(secondary.path())).unwrap();
        assert_eq!(
            auth_dir_from_candidates(&roots),
            Some(auth_dir_in(secondary.path())),
            "LOCALAPPDATA 缺失回退 %APPDATA%"
        );
    }
}
