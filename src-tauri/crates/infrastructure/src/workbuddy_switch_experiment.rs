//! P18-B2：WorkBuddy 切号实验执行器（**骨架——完整实现，真机执行入口锁定
//! 待统一验收**；本会话纪律：开发阶段只写代码+单测，不碰真机）。
//!
//! 实验命题（spec 8.4 修订的实证环节）：WorkBuddy 客户端**关闭**状态下改写
//! `.info` 登录会话文件的 token 字段 → 冷启动 → 客户端是否接受。两分支：
//! - `Plain`：token 以明文字符串写入（旧版 5.5.x 形态）；
//! - `ToolEnvelope`：token 以工具加密信封写入——用客户端运行时取钥
//!   （ELECTRON_RUN_AS_NODE helper，派生密钥只出派生值不出原始密钥）
//!   做 AES-256-GCM 正向加密，信封/AAD 与
//!   `workbuddy_decrypt_helper.cjs` 解密链逐字节互逆。
//!
//! 红线继承（10-17 文档 §6 + B2 风险知情）：
//! - 改写前必须确认客户端**完全退出**（进程 Unknown 拒绝一切写副作用）；
//! - 改写前滚动单备份原文件（`{info}.experiment-bak`，同一时刻仅一份）；
//! - 失败可重登（重捕链已有），但备份让失败现场可回滚；
//! - 冷启动接受度 = 健康复核（快照 uid + 登录态）+ 用户在客户端内的人工
//!   确认（本模块只产出回执，不做在线复核）。
//!
//! **真机执行入口**：examples/wb_switch_experiment_executor.rs（双重确认
//! 参数门禁）。组合根未注册任何 Tauri 命令；单测全部走注入依赖离线运行。

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::{Aes256Gcm, Nonce};
use sha2::{Digest, Sha256};

// ===== 密钥材料（取钥 helper + 派生） =====

/// 取钥 helper（ELECTRON_RUN_AS_NODE；输出派生密钥 hex + keyId，
/// 原始 atRestSecretKey 不出子进程——安全边界见脚本头注释）。
pub const KEY_HELPER_SCRIPT: &str = include_str!("workbuddy_key_helper.cjs");

/// 派生密钥材料（AES-256-GCM 加密 + 信封 keyId 构造所需的最小暴露面）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DerivedKey {
    pub key: [u8; 32],
    pub key_id: String,
}

/// keyId 派生（与 helper / 解密链同口径）：sha256(key).hex 前 16 位。
pub fn derive_key_id(key: &[u8; 32]) -> String {
    let digest = Sha256::digest(key);
    hex::encode(&digest[..8])
}

/// AAD 字段帧（与 workbuddy_decrypt_helper.cjs 的 fieldAad 逐字节互逆）：
/// `WB-AAD\0` || 0x01 || lp("WBEV1") || lp("sym-v1") || u32be(suite)
/// || lp(keyId) || 0x02(framing=field) || 0x00(sequence 缺省) || 0x00(final=false)。
pub fn field_aad(key_id: &str, suite: u32) -> Vec<u8> {
    fn lp(value: &str) -> Vec<u8> {
        let mut out = (value.len() as u32).to_be_bytes().to_vec();
        out.extend_from_slice(value.as_bytes());
        out
    }
    let mut aad = b"WB-AAD\0".to_vec();
    aad.push(0x01);
    aad.extend_from_slice(&lp("WBEV1"));
    aad.extend_from_slice(&lp("sym-v1"));
    aad.extend_from_slice(&suite.to_be_bytes());
    aad.extend_from_slice(&lp(key_id));
    aad.extend_from_slice(&[0x02, 0x00, 0x00]);
    aad
}

/// 加密实验错误（组合根/示例只暴露稳定错误码）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExperimentError {
    /// 客户端进程状态未知——拒绝一切写副作用（P14-2 边界）。
    ProcessStateUnknown,
    /// 关闭失败（强杀后仍非 NotRunning）。
    CloseFailed,
    /// `.info` 文件不存在。
    InfoMissing,
    /// `.info` 读取/解析失败。
    InfoMalformed,
    /// ToolEnvelope 形态要求先取钥（调用方未提供）。
    KeyRequired,
    /// 取钥失败（code 载荷来自子流程，不含密钥材料）。
    KeyFetchFailed(String),
    /// 加密失败。
    EncryptFailed,
    /// 备份/写回文件失败。
    WriteFailed,
}

impl ExperimentError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::ProcessStateUnknown => "wb_experiment_process_state_unknown",
            Self::CloseFailed => "wb_experiment_close_failed",
            Self::InfoMissing => "wb_experiment_info_missing",
            Self::InfoMalformed => "wb_experiment_info_malformed",
            Self::KeyRequired => "wb_experiment_key_required",
            Self::KeyFetchFailed(_) => "wb_experiment_key_fetch_failed",
            Self::EncryptFailed => "wb_experiment_encrypt_failed",
            Self::WriteFailed => "wb_experiment_write_failed",
        }
    }
}

/// 取钥（借 WorkBuddy.exe 自己的运行时；客户端无需运行，exe 静态存在即可）。
/// 流程与 decrypt_session_tokens 同纪律：专用临时目录 RAII 即删、
/// CREATE_NO_WINDOW、派生密钥只经 stdout 管道回传（不落盘不进日志）。
pub fn fetch_derived_key(client_exe: &Path) -> Result<DerivedKey, ExperimentError> {
    use std::os::windows::process::CommandExt;
    use std::process::Command;
    use std::time::{SystemTime, UNIX_EPOCH};
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    if !client_exe.is_file() {
        return Err(ExperimentError::KeyFetchFailed(
            "workbuddy_decrypt_client_exe_missing".to_string(),
        ));
    }
    let unique = format!(
        "traesync-wb-key-{}-{}",
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
        return Err(ExperimentError::KeyFetchFailed(
            "workbuddy_decrypt_temp_script_failed".to_string(),
        ));
    }
    let _guard = TempDirGuard(temp_dir.clone());
    let script_path = temp_dir.join("wb_key_helper.cjs");
    if fs::write(&script_path, KEY_HELPER_SCRIPT).is_err() {
        return Err(ExperimentError::KeyFetchFailed(
            "workbuddy_decrypt_temp_script_failed".to_string(),
        ));
    }
    let output = Command::new(client_exe)
        .env("ELECTRON_RUN_AS_NODE", "1")
        .arg(&script_path)
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map_err(|_| {
            ExperimentError::KeyFetchFailed("workbuddy_decrypt_spawn_failed".to_string())
        })?;
    let parsed: serde_json::Value = serde_json::from_str(String::from_utf8_lossy(&output.stdout).trim())
        .map_err(|_| {
            ExperimentError::KeyFetchFailed("workbuddy_decrypt_output_invalid".to_string())
        })?;
    if parsed.get("ok").and_then(|ok| ok.as_bool()) != Some(true) {
        let reason = parsed
            .get("error")
            .and_then(|error| error.as_str())
            .unwrap_or("unknown helper failure")
            .to_string();
        return Err(ExperimentError::KeyFetchFailed(reason));
    }
    let key_hex = parsed
        .get("key_hex")
        .and_then(|value| value.as_str())
        .ok_or_else(|| {
            ExperimentError::KeyFetchFailed("workbuddy_decrypt_output_invalid".to_string())
        })?;
    let key_bytes = hex::decode(key_hex)
        .map_err(|_| ExperimentError::KeyFetchFailed("workbuddy_decrypt_output_invalid".to_string()))?;
    let key: [u8; 32] = key_bytes
        .try_into()
        .map_err(|_| ExperimentError::KeyFetchFailed("workbuddy_decrypt_output_invalid".to_string()))?;
    let key_id = parsed
        .get("key_id")
        .and_then(|value| value.as_str())
        .ok_or_else(|| {
            ExperimentError::KeyFetchFailed("workbuddy_decrypt_output_invalid".to_string())
        })?
        .to_string();
    // 自检：keyId 必须与本端派生一致（helper 与工具链路同源校验）。
    if derive_key_id(&key) != key_id {
        return Err(ExperimentError::KeyFetchFailed(
            "wb_experiment_key_id_mismatch".to_string(),
        ));
    }
    Ok(DerivedKey { key, key_id })
}

// ===== 信封构造（解密链的正向逆变换） =====

/// 随机 12 字节 nonce（真实写回路径用；单测注入固定 nonce）。
pub fn random_nonce() -> [u8; 12] {
    use aes_gcm::aead::OsRng;
    use aes_gcm::aead::rand_core::RngCore;
    let mut nonce = [0u8; 12];
    OsRng.fill_bytes(&mut nonce);
    nonce
}

/// AES-256-GCM 正向加密 + `$wbEncrypted` 信封构造（与解密链互逆）。
/// 返回值即 .info 中 token 字段的 JSON 值：
/// `{"$wbEncrypted":1,"envelope":"<base64({suite,keyId,nonce,authTag,ciphertext})>"}`。
pub fn build_envelope_value(
    plain: &str,
    key: &DerivedKey,
    suite: u32,
    nonce: &[u8; 12],
) -> Result<serde_json::Value, ExperimentError> {
    use base64::Engine;
    let cipher = Aes256Gcm::new((&key.key).into());
    let payload = Payload {
        msg: plain.as_bytes(),
        aad: &field_aad(&key.key_id, suite),
    };
    let sealed = cipher
        .encrypt(Nonce::from_slice(nonce), payload)
        .map_err(|_| ExperimentError::EncryptFailed)?;
    // sealed = ciphertext || tag(16)（RustCrypto aead 惯例；信封分字段存放）。
    let (ciphertext, tag) = sealed.split_at(sealed.len() - 16);
    let b64 = |bytes: &[u8]| base64::engine::general_purpose::STANDARD.encode(bytes);
    let envelope = serde_json::json!({
        "suite": suite,
        "keyId": key.key_id,
        "nonce": b64(nonce),
        "authTag": b64(tag),
        "ciphertext": b64(ciphertext),
    });
    let envelope_b64 = b64(envelope.to_string().as_bytes());
    Ok(serde_json::json!({ "$wbEncrypted": 1, "envelope": envelope_b64 }))
}

// ===== .info 写回（两形态） =====

/// token 写入形态（B2 实验两分支）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteMode {
    /// 明文字符串（旧版 5.5.x 形态）。
    Plain,
    /// 工具加密信封（本模块 AES-256-GCM 正向构造，与客户端解密链互逆）。
    ToolEnvelope,
}

/// 目标账号 token 与会话元数据（来源 = 账号池记录 + 池内过期时刻）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TargetTokens {
    pub access_token: String,
    pub refresh_token: String,
    /// access 过期时刻（ms epoch；写入 auth.expiresAt）。
    pub access_expires_at_ms: u64,
    /// refresh 过期时刻（ms epoch；写入 auth.refreshExpiresAt）。
    pub refresh_expires_at_ms: u64,
    /// Keycloak 会话标识（写入 auth.sessionState）。
    pub session_state: String,
}

/// 基于原始 `.info` 内容构造改写后的会话 JSON（serde_json 全局
/// preserve_order：无关字段与键序原样保留）。
pub fn build_session_json(
    original_content: &str,
    target: &TargetTokens,
    mode: WriteMode,
    key: Option<&DerivedKey>,
) -> Result<String, ExperimentError> {
    let mut value: serde_json::Value =
        serde_json::from_str(original_content).map_err(|_| ExperimentError::InfoMalformed)?;
    let auth = value
        .get_mut("auth")
        .and_then(|auth| auth.as_object_mut())
        .ok_or(ExperimentError::InfoMalformed)?;
    let token_value = |plain: &str| -> Result<serde_json::Value, ExperimentError> {
        match mode {
            WriteMode::Plain => Ok(serde_json::Value::String(plain.to_string())),
            WriteMode::ToolEnvelope => {
                let key = key.ok_or(ExperimentError::KeyRequired)?;
                build_envelope_value(plain, key, 1, &random_nonce())
            }
        }
    };
    let access = token_value(&target.access_token)?;
    let refresh = token_value(&target.refresh_token)?;
    auth.insert("accessToken".to_string(), access);
    auth.insert("refreshToken".to_string(), refresh);
    auth.insert(
        "expiresAt".to_string(),
        serde_json::json!(target.access_expires_at_ms),
    );
    auth.insert(
        "refreshExpiresAt".to_string(),
        serde_json::json!(target.refresh_expires_at_ms),
    );
    auth.insert(
        "sessionState".to_string(),
        serde_json::json!(target.session_state),
    );
    serde_json::to_string_pretty(&value).map_err(|_| ExperimentError::WriteFailed)
}

// ===== 实验编排（进程/启动/时钟全注入；离线单测同 B1 模式） =====

/// 客户端进程三态（复用 B1 的同一语义）。
pub use crate::workbuddy_transfer::ClientProcessState;

/// 编排依赖注入。
pub struct ExperimentDeps<'a> {
    /// 进程观察（Unknown 必须如实报告）。
    pub observe: &'a dyn Fn() -> ClientProcessState,
    /// 关闭编排（优雅→强杀；退出确认由 observe 复核）。
    pub close: &'a dyn Fn() -> bool,
    /// 冷启动动作（None = 跳过冷启动，回执如实报告）。
    pub start: Option<&'a dyn Fn() -> bool>,
    /// 冷启动后等待进程树铺开的静置时长（单测传 ZERO）。
    pub restart_settle: Duration,
    /// 时钟（健康复核用；单测注入固定值）。
    pub now_ms: &'a dyn Fn() -> u64,
}

/// 实验回执。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExperimentReceipt {
    pub mode: WriteMode,
    /// 原文件备份路径（`{info}.experiment-bak`，滚动单份）。
    pub backup_path: PathBuf,
    /// 是否实际改写了 .info。
    pub wrote_info: bool,
    /// 是否发起了冷启动。
    pub cold_started: bool,
    /// 冷启动后的文件面登录态（健康复核；未改写/未启动时 None）。
    pub post_status: Option<String>,
    /// 冷启动后快照中的 uid（账号是否被客户端接受的第一证据）。
    pub post_uid: Option<String>,
}

/// 运行切号实验（**真机执行入口锁定**：组合根无命令注册；示例执行器需
/// 双重确认参数。本函数自身可在单测中以注入依赖离线运行）。
pub fn run_switch_experiment(
    auth_dir: &Path,
    auth_id: &str,
    target: &TargetTokens,
    mode: WriteMode,
    key: Option<&DerivedKey>,
    deps: &ExperimentDeps<'_>,
) -> Result<ExperimentReceipt, ExperimentError> {
    use crate::workbuddy_credential::{
        logout_marker_file_in, parse_session_snapshot, session_file_in, session_status_with,
    };
    let info_path = session_file_in(auth_dir, auth_id);
    let backup_path = {
        let mut name = info_path
            .file_name()
            .map(|name| name.to_os_string())
            .unwrap_or_default();
        name.push(".experiment-bak");
        info_path.with_file_name(name)
    };
    let receipt = |wrote: bool, cold: bool, post: Option<(String, Option<String>)>| ExperimentReceipt {
        mode,
        backup_path: backup_path.clone(),
        wrote_info: wrote,
        cold_started: cold,
        post_status: post.as_ref().map(|(status, _)| status.clone()),
        post_uid: post.map(|(_, uid)| uid).flatten(),
    };

    // 1) 完全退出前置校验（Unknown 拒绝一切写副作用；Running 先关后复核）。
    match (deps.observe)() {
        ClientProcessState::NotRunning => {}
        ClientProcessState::Running => {
            if !(deps.close)() {
                return Err(ExperimentError::CloseFailed);
            }
            if (deps.observe)() != ClientProcessState::NotRunning {
                return Err(ExperimentError::CloseFailed);
            }
        }
        ClientProcessState::Unknown => return Err(ExperimentError::ProcessStateUnknown),
    }

    // 2) 读原文件并解析（fail-fast：形态不符不写任何字节）。
    let original = fs::read_to_string(&info_path).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            ExperimentError::InfoMissing
        } else {
            ExperimentError::InfoMalformed
        }
    })?;
    parse_session_snapshot(&original).map_err(|_| ExperimentError::InfoMalformed)?;
    let next_content = build_session_json(&original, target, mode, key)?;

    // 3) 滚动单备份（覆盖旧备份；同一时刻仅一份——项目铁律口径）。
    if fs::write(&backup_path, &original).is_err() {
        return Err(ExperimentError::WriteFailed);
    }

    // 4) 写回（临时文件 + 替换；Windows rename 不能覆盖存在目标，先移除——
    //    此窗口极短且已有备份兜底）。
    let temp_path = info_path.with_extension("info.tmp");
    if fs::write(&temp_path, &next_content).is_err() {
        return Err(ExperimentError::WriteFailed);
    }
    if fs::remove_file(&info_path)
        .and_then(|()| fs::rename(&temp_path, &info_path))
        .is_err()
    {
        return Err(ExperimentError::WriteFailed);
    }

    // 5) 冷启动（best-effort：失败如实回执，post 复核照做）。
    let cold_started = deps.start.map(|start| start()).unwrap_or(false);
    if cold_started && !deps.restart_settle.is_zero() {
        std::thread::sleep(deps.restart_settle);
    }

    // 6) 健康复核：快照解析 + 登出标记 + 登录态判定。
    let post = match crate::workbuddy_credential::load_session_snapshot_in(auth_dir, auth_id) {
        Ok(snapshot) => {
            let marker_exists = logout_marker_file_in(&info_path).exists();
            let status = session_status_with(Some(&snapshot), marker_exists, (deps.now_ms)());
            Some((format!("{status:?}"), Some(snapshot.uid)))
        }
        Err(_) => None,
    };
    Ok(receipt(true, cold_started, post))
}

// ===== 单测（全部离线注入；零真机调用） =====

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workbuddy_credential::DEFAULT_AUTH_ID;
    use std::cell::Cell;
    use std::rc::Rc;

    /// 与 workbuddy_credential tests 同源的样例（形态一致、值替换）。
    const SAMPLE: &str = r#"{
  "auth": {
    "accessToken": { "$wbEncrypted": 1, "envelope": "old-access" },
    "refreshToken": { "$wbEncrypted": 1, "envelope": "old-refresh" },
    "tokenType": "Bearer",
    "sessionState": "old-session",
    "domain": "www.workbuddy.cn",
    "expiresAt": 1794638459316,
    "refreshExpiresAt": 1794811259316
  },
  "account": { "uid": "89b16d6b-0549-4e1e-8ced-918b5c8152ae", "uin": "330126449009" },
  "accounts": [ { "uid": "89b16d6b-0549-4e1e-8ced-918b5c8152ae" } ],
  "allAccounts": [ { "uid": "89b16d6b-0549-4e1e-8ced-918b5c8152ae" } ]
}"#;

    fn test_key() -> DerivedKey {
        let mut key = [0u8; 32];
        for (index, byte) in key.iter_mut().enumerate() {
            *byte = (index as u8) ^ 0xA5;
        }
        DerivedKey { key, key_id: derive_key_id(&key) }
    }

    fn target_tokens() -> TargetTokens {
        TargetTokens {
            access_token: "new-access-token".to_string(),
            refresh_token: "new-refresh-token".to_string(),
            access_expires_at_ms: 1_800_000_000_000,
            refresh_expires_at_ms: 1_810_000_000_000,
            session_state: "new-session".to_string(),
        }
    }

    #[test]
    fn key_id_derivation_matches_helper_chain() {
        // sha256(key) 前 8 字节 hex——与 helper（及解密 helper）同一派生链。
        let key = [7u8; 32];
        let expected: String = {
            let digest = Sha256::digest(key);
            hex::encode(&digest[..8])
        };
        assert_eq!(derive_key_id(&key), expected);
        assert_eq!(derive_key_id(&key).len(), 16);
    }

    #[test]
    fn field_aad_matches_decrypt_helper_byte_layout() {
        // 固定输入的完整字面量（逐段手算自 helper fieldAad）：
        // "WB-AAD\0" + 0x01 + lp("WBEV1") + lp("sym-v1") + u32be(1)
        // + lp("0123456789abcdef") + 0x02 0x00 0x00。
        let expected: Vec<u8> = [
            0x57, 0x42, 0x2D, 0x41, 0x41, 0x44, 0x00, // WB-AAD\0
            0x01, // version
            0x00, 0x00, 0x00, 0x05, b'W', b'B', b'E', b'V', b'1', // lp("WBEV1")
            0x00, 0x00, 0x00, 0x06, b's', b'y', b'm', b'-', b'v', b'1', // lp("sym-v1")
            0x00, 0x00, 0x00, 0x01, // suite = 1 (u32be)
            0x00, 0x00, 0x00, 0x10, b'0', b'1', b'2', b'3', b'4', b'5', b'6', b'7', b'8', b'9',
            b'a', b'b', b'c', b'd', b'e', b'f', // lp("0123456789abcdef")
            0x02, 0x00, 0x00, // framing=field / sequence 缺省 / final=false
        ]
        .to_vec();
        assert_eq!(field_aad("0123456789abcdef", 1), expected);
    }

    #[test]
    fn envelope_roundtrips_through_own_gcm_decrypt() {
        use base64::Engine;
        let key = test_key();
        let nonce = [9u8; 12];
        let plain = "jwt.access.token.value";
        let value = build_envelope_value(plain, &key, 1, &nonce).unwrap();
        // 信封形态：$wbEncrypted=1 + envelope base64。
        assert_eq!(value.get("$wbEncrypted").and_then(|flag| flag.as_i64()), Some(1));
        let envelope_b64 = value.get("envelope").and_then(|env| env.as_str()).unwrap();
        let envelope: serde_json::Value = serde_json::from_slice(
            &base64::engine::general_purpose::STANDARD.decode(envelope_b64).unwrap(),
        )
        .unwrap();
        assert_eq!(envelope.get("keyId").and_then(|id| id.as_str()), Some(key.key_id.as_str()));
        assert_eq!(envelope.get("suite").and_then(|suite| suite.as_u64()), Some(1));
        // 自解密：同 key/AAD/nonce 还原明文（GCM 认证标签校验通过）。
        let cipher = Aes256Gcm::new((&key.key).into());
        let nonce_b64 = envelope.get("nonce").and_then(|n| n.as_str()).unwrap();
        let tag_b64 = envelope.get("authTag").and_then(|t| t.as_str()).unwrap();
        let ct_b64 = envelope.get("ciphertext").and_then(|c| c.as_str()).unwrap();
        let mut sealed =
            base64::engine::general_purpose::STANDARD.decode(ct_b64).unwrap();
        sealed.extend_from_slice(&base64::engine::general_purpose::STANDARD.decode(tag_b64).unwrap());
        let decrypted = cipher
            .decrypt(
                Nonce::from_slice(
                    &base64::engine::general_purpose::STANDARD.decode(nonce_b64).unwrap(),
                ),
                Payload {
                    msg: &sealed,
                    aad: &field_aad(&key.key_id, 1),
                },
            )
            .unwrap();
        assert_eq!(String::from_utf8(decrypted).unwrap(), plain);
    }

    #[test]
    fn session_json_plain_mode_updates_tokens_and_preserves_rest() {
        let content = build_session_json(SAMPLE, &target_tokens(), WriteMode::Plain, None).unwrap();
        let value: serde_json::Value = serde_json::from_str(&content).unwrap();
        let auth = value.get("auth").unwrap();
        assert_eq!(auth.get("accessToken").and_then(|t| t.as_str()), Some("new-access-token"));
        assert_eq!(auth.get("refreshToken").and_then(|t| t.as_str()), Some("new-refresh-token"));
        assert_eq!(auth.get("expiresAt").and_then(|t| t.as_u64()), Some(1_800_000_000_000));
        assert_eq!(auth.get("sessionState").and_then(|t| t.as_str()), Some("new-session"));
        // 无关字段原样保留。
        assert_eq!(
            value.pointer("/account/uid").and_then(|uid| uid.as_str()),
            Some("89b16d6b-0549-4e1e-8ced-918b5c8152ae")
        );
        assert_eq!(value.get("accounts").unwrap().as_array().unwrap().len(), 1);
        // 键序保留（serde_json preserve_order 全局生效）。
        let keys: Vec<&String> = auth.as_object().unwrap().keys().collect();
        assert_eq!(keys[0], "accessToken", "原样保留 auth 首键");
    }

    #[test]
    fn session_json_envelope_mode_requires_key_and_roundtrips_shape() {
        // 未提供密钥 → KeyRequired（零写路径在编排层保障，此处纯构造层）。
        assert_eq!(
            build_session_json(SAMPLE, &target_tokens(), WriteMode::ToolEnvelope, None).unwrap_err(),
            ExperimentError::KeyRequired
        );
        let content = build_session_json(
            SAMPLE,
            &target_tokens(),
            WriteMode::ToolEnvelope,
            Some(&test_key()),
        )
        .unwrap();
        let value: serde_json::Value = serde_json::from_str(&content).unwrap();
        let access = value.pointer("/auth/accessToken").unwrap();
        assert_eq!(access.get("$wbEncrypted").and_then(|flag| flag.as_i64()), Some(1));
        assert!(access.get("envelope").and_then(|env| env.as_str()).is_some());
    }

    #[test]
    fn malformed_original_rejected() {
        assert_eq!(
            build_session_json("not json", &target_tokens(), WriteMode::Plain, None).unwrap_err(),
            ExperimentError::InfoMalformed
        );
    }

    fn sample_deps_states(
        sequence: Vec<ClientProcessState>,
        closed: Rc<Cell<bool>>,
        started: Rc<Cell<bool>>,
    ) -> ExperimentDeps<'static> {
        // 泄漏注入闭包（闭包按值持有 Cell，测试进程生命周期内有效）。
        let observe: Box<dyn Fn() -> ClientProcessState> = {
            let seq = std::cell::RefCell::new(sequence);
            Box::new(move || {
                let mut seq = seq.borrow_mut();
                if seq.len() > 1 {
                    seq.remove(0)
                } else {
                    seq[0]
                }
            })
        };
        let close: Box<dyn Fn() -> bool> = {
            let closed = closed.clone();
            Box::new(move || {
                closed.set(true);
                true
            })
        };
        let start: Box<dyn Fn() -> bool> = {
            let started = started.clone();
            Box::new(move || {
                started.set(true);
                true
            })
        };
        ExperimentDeps {
            observe: Box::leak(observe),
            close: Box::leak(close),
            start: Some(Box::leak(start)),
            restart_settle: Duration::ZERO,
            now_ms: Box::leak(Box::new(|| 1_700_000_000_000)),
        }
    }

    fn write_sample_info(dir: &Path) -> PathBuf {
        let info = crate::workbuddy_credential::session_file_in(dir, DEFAULT_AUTH_ID);
        fs::create_dir_all(dir).unwrap();
        fs::write(&info, SAMPLE).unwrap();
        info
    }

    #[test]
    fn experiment_happy_path_plain_mode_writes_backup_and_rechecks() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().to_path_buf();
        write_sample_info(&dir);
        let closed = Rc::new(Cell::new(false));
        let started = Rc::new(Cell::new(false));
        let deps = sample_deps_states(vec![ClientProcessState::NotRunning], closed.clone(), started.clone());
        let receipt = run_switch_experiment(
            &dir,
            DEFAULT_AUTH_ID,
            &target_tokens(),
            WriteMode::Plain,
            None,
            &deps,
        )
        .unwrap();
        assert!(receipt.wrote_info);
        assert!(receipt.cold_started);
        assert!(started.get());
        assert!(!closed.get(), "NotRunning 不触发关闭");
        // 备份 = 原文；新文件含明文 token。
        let info = crate::workbuddy_credential::session_file_in(&dir, DEFAULT_AUTH_ID);
        assert_eq!(fs::read_to_string(&receipt.backup_path).unwrap(), SAMPLE);
        let next = fs::read_to_string(&info).unwrap();
        assert!(next.contains("new-access-token"));
        // 健康复核：uid 换成目标账号（样例 uid 未变，验证读取口径）；状态 Active
        // （expiresAt 已更新到未来）。
        assert_eq!(receipt.post_uid.as_deref(), Some("89b16d6b-0549-4e1e-8ced-918b5c8152ae"));
        assert_eq!(receipt.post_status.as_deref(), Some("Active"));
    }

    #[test]
    fn experiment_unknown_process_refuses_with_zero_writes() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().to_path_buf();
        let info = write_sample_info(&dir);
        let closed = Rc::new(Cell::new(false));
        let started = Rc::new(Cell::new(false));
        let deps = sample_deps_states(vec![ClientProcessState::Unknown], closed.clone(), started.clone());
        let error = run_switch_experiment(
            &dir,
            DEFAULT_AUTH_ID,
            &target_tokens(),
            WriteMode::Plain,
            None,
            &deps,
        )
        .unwrap_err();
        assert_eq!(error, ExperimentError::ProcessStateUnknown);
        assert_eq!(fs::read_to_string(&info).unwrap(), SAMPLE, "零写副作用");
        assert!(!crate::workbuddy_credential::session_file_in(&dir, DEFAULT_AUTH_ID)
            .with_file_name("workbuddy-desktop.info.experiment-bak")
            .exists(), "不产生备份");
    }

    #[test]
    fn experiment_running_client_is_closed_then_rechecked() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().to_path_buf();
        write_sample_info(&dir);
        let closed = Rc::new(Cell::new(false));
        let started = Rc::new(Cell::new(false));
        // 观察序列：Running → 关闭后 NotRunning。
        let deps = sample_deps_states(
            vec![ClientProcessState::Running, ClientProcessState::NotRunning],
            closed.clone(),
            started.clone(),
        );
        let receipt = run_switch_experiment(
            &dir,
            DEFAULT_AUTH_ID,
            &target_tokens(),
            WriteMode::Plain,
            None,
            &deps,
        )
        .unwrap();
        assert!(receipt.wrote_info);
        assert!(closed.get(), "Running 先关后写");
    }

    #[test]
    fn experiment_close_failure_refuses_write() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().to_path_buf();
        let info = write_sample_info(&dir);
        let closed = Rc::new(Cell::new(false));
        let started = Rc::new(Cell::new(false));
        // 观察序列：Running → 关闭后仍 Running（关闭失败现场）。
        let deps = sample_deps_states(
            vec![ClientProcessState::Running, ClientProcessState::Running],
            closed.clone(),
            started.clone(),
        );
        let error = run_switch_experiment(
            &dir,
            DEFAULT_AUTH_ID,
            &target_tokens(),
            WriteMode::Plain,
            None,
            &deps,
        )
        .unwrap_err();
        assert_eq!(error, ExperimentError::CloseFailed);
        assert_eq!(fs::read_to_string(&info).unwrap(), SAMPLE, "拒绝写");
    }

    #[test]
    fn experiment_without_start_reports_cold_start_skipped() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().to_path_buf();
        write_sample_info(&dir);
        let closed = Rc::new(Cell::new(false));
        let started = Rc::new(Cell::new(false));
        let deps = sample_deps_states(vec![ClientProcessState::NotRunning], closed.clone(), started.clone());
        let deps = ExperimentDeps {
            observe: deps.observe,
            close: deps.close,
            start: None,
            restart_settle: Duration::ZERO,
            now_ms: deps.now_ms,
        };
        let receipt = run_switch_experiment(
            &dir,
            DEFAULT_AUTH_ID,
            &target_tokens(),
            WriteMode::Plain,
            None,
            &deps,
        )
        .unwrap();
        assert!(receipt.wrote_info);
        assert!(!receipt.cold_started, "未注入启动动作时如实报告");
    }

    /// helper 脚本静态断言：绑定名/派生链/只出派生值的边界齐备。
    #[test]
    fn key_helper_script_contains_required_parts() {
        assert!(KEY_HELPER_SCRIPT.contains("electron_browser_workbuddy_storage"));
        assert!(KEY_HELPER_SCRIPT.contains("atRestSecretKey"));
        assert!(KEY_HELPER_SCRIPT.contains("sha256"));
        assert!(KEY_HELPER_SCRIPT.contains("key_hex"));
        assert!(KEY_HELPER_SCRIPT.contains("key_id"));
    }
}
