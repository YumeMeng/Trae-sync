//! U-7 C1 凭据保活：TRAE 实例 `storage.json` 登录 blob 的解密、保序更新与重加密写回。
//!
//! 算法与安全约束均来自 C0 实测（`.scratch/blob-rewrite-probe/report.md`，
//! 2026-08-28 TRAE 真机验证接受 App 写回的 blob）：
//! - blob 结构 `[6字节头][32字节密钥材料][AES-128-CBC(PKCS7) 密文]`，
//!   密文 = `SHA512(明文JSON) || 明文JSON`；
//! - 密钥材料必须原样复用（重生成 = 陌生加密身份，TRAE 拒绝）；
//! - 写回只替换 `iCubeAuthInfo://icube.cloudide` 键的 Base64 值，
//!   `icube-dc` 设备键与其余键字节级不动（铁律）；
//! - 明文重序列化依赖 serde_json preserve_order 保持字段原顺序；
//! - TRAE 运行期间会并发写 storage.json，写回前必须做进程互斥检查。
//!
//! 本模块不删除任何文件；无登录 blob 时返回 Skip（无登录态可保鲜，非错误）。

use std::path::Path;

use aes::Aes128;
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use cbc::cipher::{BlockModeDecrypt, BlockModeEncrypt, KeyIvInit, block_padding::Pkcs7};
use sha2::{Digest, Sha512};

use crate::trae_instance::{command_line_uses_data_dir, instance_data_dir, TraeProcessInfo};

// byteCrypto 的两个 64 字节常量数组（main.js 逆向，手册 §3.3）。
const KIE: [u8; 64] = [
    82, 9, 106, 213, 48, 54, 165, 56, 191, 64, 163, 158, 129, 243, 215, 251, 124, 227, 57, 130,
    155, 47, 255, 135, 52, 142, 67, 68, 196, 222, 233, 203, 84, 123, 148, 50, 166, 194, 35, 61,
    238, 76, 149, 11, 66, 250, 195, 78, 8, 46, 161, 102, 40, 217, 36, 178, 118, 91, 162, 73, 109,
    139, 209, 37,
];
const DIE: [u8; 64] = [
    31, 221, 168, 51, 136, 7, 199, 49, 177, 18, 16, 89, 39, 128, 236, 95, 96, 81, 127, 169, 25,
    181, 74, 13, 45, 229, 122, 159, 147, 201, 156, 239, 160, 224, 59, 77, 174, 42, 245, 176, 200,
    235, 187, 60, 131, 83, 153, 97, 23, 43, 4, 126, 186, 119, 214, 38, 225, 105, 20, 99, 85, 33,
    12, 125,
];

// AES 版本 blob 的 6 字节头（Base64 前缀 "dGMFEAAA"）。
const HEADER: [u8; 6] = [116, 99, 5, 16, 0, 0];

// 用户认证记录在 storage.json 中的键名（勿动 icube-dc 设备键）。
const AUTH_KEY: &str = "iCubeAuthInfo://icube.cloudide";

// blob 布局常量：头 6 字节 + 密钥材料 32 字节。
const KEY_MATERIAL_LEN: usize = 32;

/// 写回结果（非错误语义均用 Skip 表达，调用方无需补救动作）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeepaliveOutcome {
    /// 已写回并通过写后自验证。
    Written,
    /// 无登录态可保鲜：实例目录无 storage.json，或文件中无 cloudide 键。
    Skipped,
    /// 该实例 TRAE 正在运行（并发写风险），本次放弃，等下个周期。
    SkippedInstanceRunning,
}

/// 保活写回失败（全部非敏感；不含 token 或文件内容）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeepaliveError {
    /// storage.json 存在但登录 blob 解密/完整性校验失败（文件未被修改）。
    AuthBlobInvalid,
    /// 旧 Base64 值在文件中出现次数 != 1，纯值替换无法保证其他键不动。
    AmbiguousBlobValue,
    /// 写回后自验证失败（写已发生，登录 blob 可能需要用户关注）。
    VerifyFailed,
    /// 文件读写或进程查询失败。
    Io,
}

impl std::fmt::Display for KeepaliveError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let message = match self {
            Self::AuthBlobInvalid => "登录 blob 解密或校验失败",
            Self::AmbiguousBlobValue => "登录 blob 值在文件中不唯一",
            Self::VerifyFailed => "写回后自验证失败",
            Self::Io => "保活写回文件读写失败",
        };
        formatter.write_str(message)
    }
}

impl std::error::Error for KeepaliveError {}

/// 解密 storage.json 中的登录 blob。
///
/// 校验链：Base64 解码 → 长度 → 6 字节头 → AES-128-CBC 解密 → SHA512 摘要。
/// 任何一环失败返回 `AuthBlobInvalid`（失败细节不外泄）。
/// 返回 (明文 JSON Value, 原密钥材料)；加密写回必须复用该密钥材料。
pub fn decrypt_auth_blob(
    storage_json: &str,
) -> Result<(serde_json::Value, [u8; 32]), KeepaliveError> {
    let root: serde_json::Value =
        serde_json::from_str(storage_json).map_err(|_| KeepaliveError::AuthBlobInvalid)?;
    let b64 = root
        .get(AUTH_KEY)
        .and_then(|value| value.as_str())
        .ok_or(KeepaliveError::AuthBlobInvalid)?;
    let blob = STANDARD
        .decode(b64)
        .map_err(|_| KeepaliveError::AuthBlobInvalid)?;
    if blob.len() < 6 + KEY_MATERIAL_LEN + 64
        || (blob.len() - 6 - KEY_MATERIAL_LEN) % 16 != 0
        || blob[0..6] != HEADER
    {
        return Err(KeepaliveError::AuthBlobInvalid);
    }
    let key_material: [u8; 32] = blob[6..38].try_into().expect("长度已校验");
    let plaintext_full = decrypt_blob(&key_material, &blob[38..])?;
    let digest = &plaintext_full[0..64];
    let plain = &plaintext_full[64..];
    if digest != Sha512::digest(plain).as_slice() {
        return Err(KeepaliveError::AuthBlobInvalid);
    }
    let plaintext: serde_json::Value =
        serde_json::from_slice(plain).map_err(|_| KeepaliveError::AuthBlobInvalid)?;
    Ok((plaintext, key_material))
}

/// 把续期后的新凭据写回实例 `storage.json` 的登录 blob。
///
/// Skip 语义：无 storage.json / 无 cloudide 键（该实例无登录态可保鲜）；
/// 实例运行中（进程命令行含该实例目录）跳过本次，不视为错误。
/// 进程互斥是硬要求：TRAE 运行期间并发写同一文件会互相覆盖（任务书陷阱 2）。
pub fn writeback_auth_blob(
    instance_dir: &Path,
    token: &str,
    refresh_token: &str,
    expired_at_unix_seconds: u64,
    refresh_expired_at_unix_seconds: u64,
) -> Result<KeepaliveOutcome, KeepaliveError> {
    // 生产入口：真实查询 TRAE 进程（PowerShell 阻塞调用，调用方应在阻塞线程）。
    // 查询失败按"运行中"处理：宁可少写一次，不冒并发覆盖风险。
    // 非 Windows 平台无 TRAE 进程语义，空列表直通（本项目生产环境为 Windows）。
    #[cfg(windows)]
    let processes = match crate::trae_instance::list_trae_processes() {
        Ok(processes) => processes,
        Err(_) => return Ok(KeepaliveOutcome::SkippedInstanceRunning),
    };
    #[cfg(not(windows))]
    let processes: Vec<TraeProcessInfo> = Vec::new();
    writeback_auth_blob_with_processes(
        instance_dir,
        token,
        refresh_token,
        expired_at_unix_seconds,
        refresh_expired_at_unix_seconds,
        &processes,
    )
}

/// `writeback_auth_blob` 的可测试版本：进程列表由调用方注入。
pub fn writeback_auth_blob_with_processes(
    instance_dir: &Path,
    token: &str,
    refresh_token: &str,
    expired_at_unix_seconds: u64,
    refresh_expired_at_unix_seconds: u64,
    processes: &[TraeProcessInfo],
) -> Result<KeepaliveOutcome, KeepaliveError> {
    let storage_path = instance_dir
        .join("User")
        .join("globalStorage")
        .join("storage.json");
    let raw = match std::fs::read_to_string(&storage_path) {
        Ok(raw) => raw,
        // 文件不存在 = 实例从未启动（或从未播种登录态），无登录态可保鲜。
        Err(_) => return Ok(KeepaliveOutcome::Skipped),
    };
    // 无 cloudide 键：只有设备 blob 或空 storage，同样 Skip 而非错误。
    let root: serde_json::Value =
        serde_json::from_str(&raw).map_err(|_| KeepaliveError::AuthBlobInvalid)?;
    let Some(b64) = root.get(AUTH_KEY).and_then(|value| value.as_str()) else {
        return Ok(KeepaliveOutcome::Skipped);
    };

    // 互斥检查：该实例目录上有任何 TRAE 进程即放弃写回。
    let running = processes.iter().any(|process| {
        process
            .command_line
            .as_deref()
            .is_some_and(|line| command_line_uses_data_dir(line, instance_dir))
    });
    if running {
        return Ok(KeepaliveOutcome::SkippedInstanceRunning);
    }

    // 解密链失败时文件未动（所有写动作都在校验之后）。
    let (mut plaintext, key_material) = decrypt_auth_blob(&raw)?;

    // 只更新明文中已存在的字段（get_mut 不新增键），其余字段与顺序原样。
    let updates: [(&str, String); 4] = [
        ("token", token.to_string()),
        ("refreshToken", refresh_token.to_string()),
        ("expiredAt", iso8601_utc(expired_at_unix_seconds)),
        (
            "refreshExpiredAt",
            iso8601_utc(refresh_expired_at_unix_seconds),
        ),
    ];
    let object = plaintext
        .as_object_mut()
        .ok_or(KeepaliveError::AuthBlobInvalid)?;
    for (field, value) in updates {
        if let Some(slot) = object.get_mut(field) {
            *slot = serde_json::Value::String(value);
        }
    }

    // preserve_order 下重序列化只变被更新字段的值，顺序与未动字段字节不变。
    let new_plain =
        serde_json::to_vec(&plaintext).map_err(|_| KeepaliveError::AuthBlobInvalid)?;
    let new_b64 = reencrypt_blob(&key_material, &new_plain);

    // 纯值替换：旧 Base64 在文件中恰好出现一次时，其余键字节级原样。
    if raw.matches(b64).count() != 1 {
        return Err(KeepaliveError::AmbiguousBlobValue);
    }
    let new_raw = raw.replacen(b64, &new_b64, 1);
    std::fs::write(&storage_path, &new_raw).map_err(|_| KeepaliveError::Io)?;

    // 写后自验证：重读文件走完整解密链，确认新值落地且设备键未被波及。
    let verify_raw =
        std::fs::read_to_string(&storage_path).map_err(|_| KeepaliveError::VerifyFailed)?;
    let (verify_plain, _) =
        decrypt_auth_blob(&verify_raw).map_err(|_| KeepaliveError::VerifyFailed)?;
    let verify_token = verify_plain.get("token").and_then(|value| value.as_str());
    if verify_token != Some(token) {
        return Err(KeepaliveError::VerifyFailed);
    }
    let verify_root: serde_json::Value =
        serde_json::from_str(&verify_raw).map_err(|_| KeepaliveError::VerifyFailed)?;
    if verify_root.get("icube-dc") != root.get("icube-dc") {
        return Err(KeepaliveError::VerifyFailed);
    }
    Ok(KeepaliveOutcome::Written)
}

/// E3 身份互换结果（P5-1 切号第三步：写入目标账号登录凭据）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthIdentitySwitch {
    /// 目标环境（主库）切换前的登录 userId。
    pub from_user_id: String,
    /// 切换后的登录 userId（= 供体账号 userId）。
    pub to_user_id: String,
    /// 供体 account 展示字段（脱敏手机号/邮箱）。
    pub to_account: String,
}

/// 身份互换失败（E3 探针语义，全部非敏感；不携带 token 或文件内容）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthSwitchError {
    /// 供体实例目录无 storage.json 或文件中无登录 blob（该账号未登录过实例）。
    DonorAuthUnavailable,
    /// 供体登录 blob 解密/完整性校验失败（文件未被修改）。
    DonorAuthInvalid,
    /// 目标（主库）无 storage.json 或无登录 blob（尚未在主库内登录过一次）。
    TargetAuthUnavailable,
    /// 目标登录 blob 解密/完整性校验失败（文件未被修改）。
    TargetAuthInvalid,
    /// 供体与目标已是同一账号，无需切换。
    SameAccount,
    /// 目标旧 Base64 值在文件中出现次数 != 1，纯值替换无法保证其他键不动。
    AmbiguousBlobValue,
    /// 写后自验证失败（写已发生，调用方应从备份恢复）。
    VerifyFailed,
    /// 文件读写失败。
    Io,
}

impl std::fmt::Display for AuthSwitchError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let message = match self {
            Self::DonorAuthUnavailable => "供体实例无可用登录凭据",
            Self::DonorAuthInvalid => "供体登录 blob 解密或校验失败",
            Self::TargetAuthUnavailable => "目标环境无登录凭据（先在主库内登录一次）",
            Self::TargetAuthInvalid => "目标登录 blob 解密或校验失败",
            Self::SameAccount => "供体与目标为同一账号",
            Self::AmbiguousBlobValue => "目标登录 blob 值在文件中不唯一",
            Self::VerifyFailed => "身份互换写后自验证失败",
            Self::Io => "身份互换文件读写失败",
        };
        formatter.write_str(message)
    }
}

impl std::error::Error for AuthSwitchError {}

/// 登录 blob 的原始字节解密：返回 (明文原字节, 密钥材料, 旧 Base64 值)。
///
/// E3 身份互换要求供体明文**原字节**移植（不重序列化，字段顺序与字节完全
/// 一致），故此入口不解析 JSON——与 `decrypt_auth_blob`（返回解析后的 Value）
/// 是同一解密链的两种形态。
fn decrypt_auth_blob_raw(
    storage_json: &str,
) -> Result<(Vec<u8>, [u8; 32], String), KeepaliveError> {
    let root: serde_json::Value =
        serde_json::from_str(storage_json).map_err(|_| KeepaliveError::AuthBlobInvalid)?;
    let b64 = root
        .get(AUTH_KEY)
        .and_then(|value| value.as_str())
        .ok_or(KeepaliveError::AuthBlobInvalid)?
        .to_string();
    let blob = STANDARD
        .decode(&b64)
        .map_err(|_| KeepaliveError::AuthBlobInvalid)?;
    let (plain, key_material) = decrypt_blob_bytes(&blob)?;
    Ok((plain, key_material, b64))
}

/// 按 blob 原始字节解密：返回 (明文, 密钥材料)。格式校验 + AES 解密 +
/// SHA512 摘要核对，与 `decrypt_auth_blob_raw` 同一解密链。
fn decrypt_blob_bytes(blob: &[u8]) -> Result<(Vec<u8>, [u8; 32]), KeepaliveError> {
    if blob.len() < 6 + KEY_MATERIAL_LEN + 64
        || (blob.len() - 6 - KEY_MATERIAL_LEN) % 16 != 0
        || blob[0..6] != HEADER
    {
        return Err(KeepaliveError::AuthBlobInvalid);
    }
    let key_material: [u8; 32] = blob[6..38].try_into().expect("长度已校验");
    let plaintext_full = decrypt_blob(&key_material, &blob[38..])?;
    let digest = &plaintext_full[0..64];
    let plain = &plaintext_full[64..];
    if digest != Sha512::digest(plain).as_slice() {
        return Err(KeepaliveError::AuthBlobInvalid);
    }
    Ok((plain.to_vec(), key_material))
}

/// 读取实例目录 storage.json 文本；无文件或无登录键时返回 None（区分
/// 「无凭据」与「凭据损坏」两种失败）。
fn read_auth_storage(instance_dir: &Path) -> Result<Option<String>, AuthSwitchError> {
    let storage_path = instance_dir
        .join("User")
        .join("globalStorage")
        .join("storage.json");
    let raw = match std::fs::read_to_string(&storage_path) {
        Ok(raw) => raw,
        Err(_) => return Ok(None),
    };
    let root: serde_json::Value =
        serde_json::from_str(&raw).map_err(|_| AuthSwitchError::TargetAuthInvalid)?;
    if root.get(AUTH_KEY).and_then(|value| value.as_str()).is_none() {
        return Ok(None);
    }
    Ok(Some(raw))
}

/// 探针支持：解密实例 storage.json 中指定键的 blob 明文（只读，不写文件）。
///
/// 用途：诊断/实验探针按需解密任意 iCube 键（如 `iCubeAuthInfo://usertag`），
/// 与 `decrypt_auth_blob_raw` 同一解密链，仅键名可指定。生产切换链路不使用。
pub fn decrypt_named_blob(
    instance_dir: &Path,
    key: &str,
) -> Result<Vec<u8>, KeepaliveError> {
    let storage_path = instance_dir
        .join("User")
        .join("globalStorage")
        .join("storage.json");
    let raw =
        std::fs::read_to_string(&storage_path).map_err(|_| KeepaliveError::AuthBlobInvalid)?;
    let root: serde_json::Value =
        serde_json::from_str(&raw).map_err(|_| KeepaliveError::AuthBlobInvalid)?;
    let b64 = root
        .get(key)
        .and_then(|value| value.as_str())
        .ok_or(KeepaliveError::AuthBlobInvalid)?
        .to_string();
    let blob = STANDARD
        .decode(&b64)
        .map_err(|_| KeepaliveError::AuthBlobInvalid)?;
    decrypt_blob_bytes(&blob).map(|(plain, _)| plain)
}

/// 读取实例目录登录档案的 userId（只读，不写任何文件）。
///
/// 用途（2026-09-01 切号死锁修复）：E1 存档移植路径报 SameAccount 时，
/// 调用方需要核对「供体存档账号 = 注册表目标账号」——存档可能过期
/// 错位（实例曾登录过其他账号），核对一致才允许跳过登录互换。
/// 任何环节不可用返回 None（调用方按不可跳过处理，宁可不切，不可切错）。
pub fn archive_login_user_id(instance_dir: &Path) -> Option<String> {
    let raw = read_auth_storage(instance_dir).ok()??;
    let (plain, _, _) = decrypt_auth_blob_raw(&raw).ok()?;
    let auth: serde_json::Value = serde_json::from_slice(&plain).ok()?;
    auth.get("userId")
        .and_then(|value| value.as_str())
        .map(String::from)
}

/// E3 身份互换（P5-1 切号第三步）：把供体实例（目标账号专属实例目录）
/// `storage.json` 登录 blob 的完整明文**原字节**移植到目标环境（主库），
/// 但保留目标环境自己的密钥材料重加密——目标环境的「加密身份」不变，
/// 「登录身份」换成供体账号。
///
/// 安全约束（E3 探针 `.scratch/blob-rewrite-probe/src/bin/switch.rs`）：
/// - 只动 `iCubeAuthInfo://icube.cloudide` 键，`icube-dc` 设备键与其余键
///   字节级不动；
/// - 纯值替换（旧 Base64 在文件中恰好出现一次）；
/// - 写后自验证：明文字节 = 供体原字节、密钥材料 = 目标原值、icube-dc 未动。
///
/// 前置条件：目标（主库）TRAE 实例已关闭（并发写互斥由调用方保证，
/// 五步事务的第 1 步）。供体文件只读不写，供体实例运行与否不影响。
pub fn switch_auth_identity(
    donor_instance_dir: &Path,
    target_instance_dir: &Path,
) -> Result<AuthIdentitySwitch, AuthSwitchError> {
    // ===== 供体：登录身份来源（目标账号的专属实例目录）=====
    let Some(donor_raw) = read_auth_storage(donor_instance_dir)
        .map_err(|_| AuthSwitchError::DonorAuthInvalid)?
    else {
        return Err(AuthSwitchError::DonorAuthUnavailable);
    };
    let (donor_plain, _donor_km, _) =
        decrypt_auth_blob_raw(&donor_raw).map_err(|_| AuthSwitchError::DonorAuthInvalid)?;
    let donor_auth: serde_json::Value = serde_json::from_slice(&donor_plain)
        .map_err(|_| AuthSwitchError::DonorAuthInvalid)?;
    let donor_user = donor_auth
        .get("userId")
        .and_then(|value| value.as_str())
        .unwrap_or("?")
        .to_string();
    let donor_account = donor_auth
        .get("account")
        .and_then(|value| value.as_str())
        .unwrap_or("?")
        .to_string();

    // ===== 目标（主库）：密钥材料保留方 =====
    let Some(target_raw) = read_auth_storage(target_instance_dir)
        .map_err(|_| AuthSwitchError::TargetAuthInvalid)?
    else {
        return Err(AuthSwitchError::TargetAuthUnavailable);
    };
    let (target_plain, target_km, target_b64) =
        decrypt_auth_blob_raw(&target_raw).map_err(|_| AuthSwitchError::TargetAuthInvalid)?;
    let target_auth: serde_json::Value = serde_json::from_slice(&target_plain)
        .map_err(|_| AuthSwitchError::TargetAuthInvalid)?;
    let target_user = target_auth
        .get("userId")
        .and_then(|value| value.as_str())
        .unwrap_or("?")
        .to_string();

    if donor_user == target_user {
        return Err(AuthSwitchError::SameAccount);
    }

    // ===== 供体明文原字节 + 目标密钥材料 → 重加密，纯值替换写回 =====
    let new_b64 = reencrypt_blob(&target_km, &donor_plain);
    if target_raw.matches(&target_b64).count() != 1 {
        return Err(AuthSwitchError::AmbiguousBlobValue);
    }
    let new_raw = target_raw.replacen(&target_b64, &new_b64, 1);
    let target_storage_path = target_instance_dir
        .join("User")
        .join("globalStorage")
        .join("storage.json");
    std::fs::write(&target_storage_path, &new_raw).map_err(|_| AuthSwitchError::Io)?;

    // ===== 写后自验证：明文字节/密钥材料/icube-dc 三重核对 =====
    let verify_raw =
        std::fs::read_to_string(&target_storage_path).map_err(|_| AuthSwitchError::VerifyFailed)?;
    let (verify_plain, verify_km, _) =
        decrypt_auth_blob_raw(&verify_raw).map_err(|_| AuthSwitchError::VerifyFailed)?;
    if verify_plain != donor_plain {
        return Err(AuthSwitchError::VerifyFailed);
    }
    if verify_km != target_km {
        return Err(AuthSwitchError::VerifyFailed);
    }
    let verify_root: serde_json::Value =
        serde_json::from_str(&verify_raw).map_err(|_| AuthSwitchError::VerifyFailed)?;
    let target_root: serde_json::Value =
        serde_json::from_str(&target_raw).map_err(|_| AuthSwitchError::VerifyFailed)?;
    if verify_root.get("icube-dc") != target_root.get("icube-dc") {
        return Err(AuthSwitchError::VerifyFailed);
    }
    Ok(AuthIdentitySwitch {
        from_user_id: target_user,
        to_user_id: donor_user,
        to_account: donor_account,
    })
}

/// E2 凭据构造输入（P7-1，ADR-0024 决策 4）：目标账号凭据包五件套 +
/// GetUserInfo 完整资料。HTTP 实调由调用方完成（本模块保持纯文件操作，
/// 便于单测）；身份校验（`user_info.user_id == account_id`）同样前置在
/// 调用方——不通过绝不进入本函数（E2 纪律：不通过绝不写库）。
pub struct ConstructAuthInput<'a> {
    /// 目标账号 ID（= 凭据包 account_id = 服务端校验通过的 userId）。
    pub account_id: &'a str,
    pub access_token: &'a str,
    pub refresh_token: &'a str,
    pub access_token_expires_at_unix_seconds: u64,
    pub refresh_token_expires_at_unix_seconds: u64,
    /// GetUserInfo 完整资料（`checkin_http::UserInfoFull`）。
    pub user_info: &'a crate::checkin_http::UserInfoFull,
}

/// E2 凭据构造身份互换（P7-1 切号第三步首选路径）：用凭据包 +
/// GetUserInfo 资料**构造**登录 blob 明文，以目标环境（主库）自己的
/// 密钥材料加密写回——与 `switch_auth_identity`（E1 存档移植）本质同构
/// （明文 → 目标密钥材料重加密），但登录身份来源是构造而非存档移植。
///
/// 字段映射表（双账号存档明文对照逆向，`.scratch/e2-credential-login/REPORT.md`）：
/// - token/refreshToken/expiredAt/refreshExpiredAt/userId ← 凭据包；
/// - username/avatar_url/email/description/nonPlainTextMobile/storeRegion/
///   migrateToSG/region/_aiRegion ← GetUserInfo；
/// - host/scope/loginScope/userTag/iss/iat 等 ← TRAE CN 恒定值；
/// - tokenReleaseAt ← 构造时刻（TRAE 无严格校验，语义自洽即可）。
///
/// 安全约束与 E1 一致：只动 `iCubeAuthInfo://icube.cloudide` 键，纯值替换，
/// 写后三重自验证（明文 = 构造明文、密钥材料 = 目标原值、icube-dc 未动）。
/// 序列化依赖本 crate 的 serde_json `preserve_order` feature（字段顺序与
/// TRAE 原生明文一致）。
///
/// 前置条件：目标（主库）TRAE 实例已关闭（调用方五步事务第 1 步保证）。
pub fn construct_auth_identity(
    input: &ConstructAuthInput<'_>,
    target_instance_dir: &Path,
) -> Result<AuthIdentitySwitch, AuthSwitchError> {
    // ===== 目标（主库）登录态：既有 blob 复用密钥材料；登出态全新生成 =====
    // 2026-09-03 卡死案例修复：官方退出会清除主库登录键，此前 E2 在此
    // 直接失败（TargetAuthUnavailable），凭据包新账号（无存档）将无路可切。
    // 证据链（blob 自包含格式）：密钥材料内嵌于 blob[6..38]，TRAE 以 blob
    // 自带材料解密；同文件 usertag/cloudide 两 blob 密钥材料互不相同 =
    // per-blob 随机生成，无外部交叉校验——全新材料构造与 TRAE 首次登录
    // 自生成的 blob 形态一致。
    let target_raw = read_auth_storage(target_instance_dir)
        .map_err(|_| AuthSwitchError::TargetAuthInvalid)?;

    // ===== 构造明文（字段顺序 = E2 实验验证的字段映射表）=====
    let now_unix = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0);
    let info = input.user_info;
    let region = if info.region.is_empty() {
        "CN"
    } else {
        info.region.as_str()
    };
    let ai_region = if info.ai_region.is_empty() {
        "CN"
    } else {
        info.ai_region.as_str()
    };
    let constructed = serde_json::json!({
        "token": input.access_token,
        "refreshToken": input.refresh_token,
        "expiredAt": iso8601_utc(input.access_token_expires_at_unix_seconds),
        "refreshExpiredAt": iso8601_utc(input.refresh_token_expires_at_unix_seconds),
        "tokenReleaseAt": iso8601_utc(now_unix),
        "userId": input.account_id,
        "host": "https://api.trae.cn",
        "userRegion": {
            "region": region,
            "_aiRegion": ai_region,
        },
        "account": {
            "username": info.screen_name,
            "iss": "",
            "iat": 0,
            "organization": "",
            "work_country": "",
            "email": info.masked_email,
            "avatar_url": info.avatar_url,
            "description": info.description,
            "scope": "marscode",
            "loginScope": "trae",
            "nonPlainTextMobile": info.masked_mobile,
            "storeCountryCode": "",
            "storeCountrySrc": "",
            "storeRegion": region,
            "userTag": "cn",
            "migrateToSG": info.migrate_to_sg,
        },
    });
    let constructed_plain =
        serde_json::to_vec(&constructed).map_err(|_| AuthSwitchError::VerifyFailed)?;

    let target_storage_path = target_instance_dir
        .join("User")
        .join("globalStorage")
        .join("storage.json");

    // to_account 取展示字段（脱敏手机号；空则回退屏幕名）。
    let to_account = if info.masked_mobile.is_empty() {
        info.screen_name.clone()
    } else {
        info.masked_mobile.clone()
    };

    match target_raw {
        Some(target_raw) => {
            // ===== 既有登录态：复用目标密钥材料，纯值替换写回（与 E1 同链路）=====
            let (target_plain, target_km, target_b64) = decrypt_auth_blob_raw(&target_raw)
                .map_err(|_| AuthSwitchError::TargetAuthInvalid)?;
            let target_auth: serde_json::Value = serde_json::from_slice(&target_plain)
                .map_err(|_| AuthSwitchError::TargetAuthInvalid)?;
            let target_user = target_auth
                .get("userId")
                .and_then(|value| value.as_str())
                .unwrap_or("?")
                .to_string();

            if input.account_id == target_user {
                return Err(AuthSwitchError::SameAccount);
            }

            let new_b64 = reencrypt_blob(&target_km, &constructed_plain);
            if target_raw.matches(&target_b64).count() != 1 {
                return Err(AuthSwitchError::AmbiguousBlobValue);
            }
            let new_raw = target_raw.replacen(&target_b64, &new_b64, 1);
            std::fs::write(&target_storage_path, &new_raw).map_err(|_| AuthSwitchError::Io)?;

            // ===== 写后三重自验证：构造明文 / 密钥材料 / icube-dc 设备键 =====
            let verify_raw = std::fs::read_to_string(&target_storage_path)
                .map_err(|_| AuthSwitchError::VerifyFailed)?;
            let (verify_plain, verify_km, _) = decrypt_auth_blob_raw(&verify_raw)
                .map_err(|_| AuthSwitchError::VerifyFailed)?;
            if verify_plain != constructed_plain {
                return Err(AuthSwitchError::VerifyFailed);
            }
            if verify_km != target_km {
                return Err(AuthSwitchError::VerifyFailed);
            }
            let verify_root: serde_json::Value = serde_json::from_str(&verify_raw)
                .map_err(|_| AuthSwitchError::VerifyFailed)?;
            let target_root: serde_json::Value = serde_json::from_str(&target_raw)
                .map_err(|_| AuthSwitchError::VerifyFailed)?;
            if verify_root.get("icube-dc") != target_root.get("icube-dc") {
                return Err(AuthSwitchError::VerifyFailed);
            }

            Ok(AuthIdentitySwitch {
                from_user_id: target_user,
                to_user_id: input.account_id.to_string(),
                to_account,
            })
        }
        None => {
            // ===== 登出态（官方退出后）：全新密钥材料构造，键插入写回 =====
            // read_auth_storage 的 None 同时覆盖「文件不存在」与「文件存在
            // 但无登录键」两种形态，此处重读文件区分：存在时插入新键
            // （其余键原字节保留），不存在时创建新文件。
            let existing_raw = std::fs::read_to_string(&target_storage_path).ok();
            if let Some(raw) = &existing_raw {
                // 与 read_auth_storage 同纪律：文件必须可解析（登出态的
                // storage.json 是 TRAE 维护的合法 JSON）。
                serde_json::from_str::<serde_json::Value>(raw)
                    .map_err(|_| AuthSwitchError::TargetAuthInvalid)?;
            }
            let fresh_km = generate_key_material().map_err(|_| AuthSwitchError::Io)?;
            let new_b64 = reencrypt_blob(&fresh_km, &constructed_plain);
            let new_raw = insert_auth_key_or_create(&target_storage_path, &new_b64, &existing_raw)
                .map_err(|_| AuthSwitchError::Io)?;
            std::fs::write(&target_storage_path, &new_raw).map_err(|_| AuthSwitchError::Io)?;

            // ===== 写后自验证：明文一致 + 既有其他键值原样保留 =====
            let verify_raw = std::fs::read_to_string(&target_storage_path)
                .map_err(|_| AuthSwitchError::VerifyFailed)?;
            let (verify_plain, _, _) = decrypt_auth_blob_raw(&verify_raw)
                .map_err(|_| AuthSwitchError::VerifyFailed)?;
            if verify_plain != constructed_plain {
                return Err(AuthSwitchError::VerifyFailed);
            }
            let verify_root: serde_json::Value = serde_json::from_str(&verify_raw)
                .map_err(|_| AuthSwitchError::VerifyFailed)?;
            if let Some(before) = &existing_raw {
                let before_root: serde_json::Value = serde_json::from_str(before)
                    .map_err(|_| AuthSwitchError::VerifyFailed)?;
                // 登出态写回只新增登录键：既有全部键值必须原样（含设备键）。
                if let (Some(before_map), Some(after_map)) =
                    (before_root.as_object(), verify_root.as_object())
                {
                    if before_map
                        .iter()
                        .any(|(key, value)| after_map.get(key) != Some(value))
                    {
                        return Err(AuthSwitchError::VerifyFailed);
                    }
                }
            }

            // from 为空串：主库登出态无先前登录（台账 from 由交接实测
            // previous_owner 优先，空串仅作无先前账号的兜底语义）。
            Ok(AuthIdentitySwitch {
                from_user_id: String::new(),
                to_user_id: input.account_id.to_string(),
                to_account,
            })
        }
    }
}

/// 生成全新 32 字节密钥材料（登出态构造用；OpenSSL CSPRNG）。
fn generate_key_material() -> Result<[u8; 32], ()> {
    let mut key_material = [0u8; 32];
    openssl::rand::rand_bytes(&mut key_material).map_err(|_| ())?;
    Ok(key_material)
}

/// 登出态键插入：storage.json 无登录键（或文件不存在）时写入全新 blob。
///
/// 字节级纪律：文件存在时仅在开括号后插入 `"键":"值",`，其余内容原字节
/// 不动；空对象 `{}` 特判避免尾逗号。文件不存在时创建父目录并写只含
/// 登录键的新文件。
fn insert_auth_key_or_create(
    storage_path: &Path,
    new_b64: &str,
    existing_raw: &Option<String>,
) -> Result<String, ()> {
    match existing_raw {
        Some(raw) => {
            // read_auth_storage 已保证可解析为 JSON 对象；防御性定位开括号。
            let Some(brace_at) = raw.find('{') else {
                return Err(());
            };
            let rest = &raw[brace_at + 1..];
            if rest.trim().starts_with('}') {
                // 空对象 {}：整文件替换为单键对象。
                return Ok(format!("{{\"{AUTH_KEY}\":\"{new_b64}\"}}"));
            }
            let mut out = String::with_capacity(raw.len() + AUTH_KEY.len() + new_b64.len() + 8);
            out.push_str(&raw[..=brace_at]);
            out.push_str(&format!("\"{AUTH_KEY}\":\"{new_b64}\","));
            out.push_str(&raw[brace_at + 1..]);
            Ok(out)
        }
        None => {
            if let Some(parent) = storage_path.parent() {
                std::fs::create_dir_all(parent).map_err(|_| ())?;
            }
            Ok(format!("{{\"{AUTH_KEY}\":\"{new_b64}\"}}"))
        }
    }
}

/// 续期成功后的保活入口：供签到续期链路调用，失败只记 stderr 日志。
///
/// material_root 为凭据仓库根（`{storage_root}/checkin`），实例目录挂在
/// storage_root 下。保活是增强能力：任何失败都不影响续期/签到主流程。
pub(crate) fn keepalive_after_renewal(
    material_root: &Path,
    profile_id: &str,
    token: &str,
    refresh_token: &str,
    expired_at_unix_seconds: u64,
    refresh_expired_at_unix_seconds: u64,
) {
    // material_root 的父目录即 storage_root（同 lib.rs 的路径约定）。
    let Some(storage_root) = material_root.parent() else {
        return;
    };
    let Ok(instance_dir) = instance_data_dir(storage_root, profile_id) else {
        return;
    };
    let result = writeback_auth_blob(
        &instance_dir,
        token,
        refresh_token,
        expired_at_unix_seconds,
        refresh_expired_at_unix_seconds,
    );
    // 只记非敏感信息（profile_id 与结果），token 绝不进日志。
    match result {
        Ok(outcome) => {
            eprintln!("[blob-keepalive] profile={} 结果={:?}", profile_id, outcome)
        }
        Err(error) => eprintln!("[blob-keepalive] profile={} 失败={}", profile_id, error),
    }
}

/// 解密 blob 密文（AES-128-CBC + PKCS7 去填充），返回 `SHA512摘要 || 明文`。
fn decrypt_blob(key_material: &[u8; 32], ciphertext: &[u8]) -> Result<Vec<u8>, KeepaliveError> {
    let (aes_key, iv) = derive_keys(key_material);
    cbc::Decryptor::<Aes128>::new(&aes_key.into(), &iv.into())
        .decrypt_padded_vec::<Pkcs7>(ciphertext)
        .map_err(|_| KeepaliveError::AuthBlobInvalid)
}

/// 用原密钥材料重拼摘要并重加密：`头 || 密钥材料 || CBC(SHA512(明文) || 明文)`，
/// 返回 Base64。摘要必须按新明文重算（旧摘要对新明文必不匹配）。
fn reencrypt_blob(key_material: &[u8; 32], plain: &[u8]) -> String {
    let digest = Sha512::digest(plain);
    let mut rebuilt = Vec::with_capacity(64 + plain.len());
    rebuilt.extend_from_slice(&digest);
    rebuilt.extend_from_slice(plain);
    let (aes_key, iv) = derive_keys(key_material);
    let ciphertext = cbc::Encryptor::<Aes128>::new(&aes_key.into(), &iv.into())
        .encrypt_padded_vec::<Pkcs7>(&rebuilt);
    let mut blob = Vec::with_capacity(6 + KEY_MATERIAL_LEN + ciphertext.len());
    blob.extend_from_slice(&HEADER);
    blob.extend_from_slice(key_material);
    blob.extend_from_slice(&ciphertext);
    STANDARD.encode(&blob)
}

/// 密钥派生（手册 §3.3）：secret = KIE XOR DIE；
/// h1 = SHA512(密钥材料)；h2 = SHA512(h1 || secret)；取 h2 前 32 字节为 key/iv。
fn derive_keys(key_material: &[u8; 32]) -> ([u8; 16], [u8; 16]) {
    let secret: Vec<u8> = KIE.iter().zip(DIE.iter()).map(|(a, b)| a ^ b).collect();
    let h1 = Sha512::digest(key_material);
    let mut buf = Vec::with_capacity(128);
    buf.extend_from_slice(&h1);
    buf.extend_from_slice(&secret);
    let h2 = Sha512::digest(&buf);
    (
        h2[0..16].try_into().expect("SHA512 输出固定 64 字节"),
        h2[16..32].try_into().expect("SHA512 输出固定 64 字节"),
    )
}

/// unix 秒 → TRAE blob 使用的时间格式 `YYYY-MM-DDTHH:MM:SS.mmmZ`（UTC）。
/// 毫秒固定为 .000：续期时间戳本身是秒级精度。
fn iso8601_utc(unix_seconds: u64) -> String {
    let days = (unix_seconds / 86_400) as i64;
    let seconds_of_day = unix_seconds % 86_400;
    let (year, month, day) = civil_from_days(days);
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.000Z",
        year,
        month,
        day,
        seconds_of_day / 3600,
        (seconds_of_day % 3600) / 60,
        seconds_of_day % 60
    )
}

/// 天数（相对 unix 纪元）→ 公历日期。Howard Hinnant civil_from_days 算法，
/// 覆盖公历全程（含闰年），纯整数运算无日期库依赖。
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if month <= 2 { year + 1 } else { year }, month, day)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use base64::engine::general_purpose::STANDARD;
    use base64::Engine;
    use tempfile::tempdir;

    use super::{
        archive_login_user_id, construct_auth_identity, decrypt_auth_blob, iso8601_utc,
        reencrypt_blob, switch_auth_identity, writeback_auth_blob_with_processes,
        AuthSwitchError, ConstructAuthInput, KeepaliveError, KeepaliveOutcome, AUTH_KEY,
    };
    use crate::trae_instance::TraeProcessInfo;

    /// 固定密钥材料：测试不追求加密强度，只求可复现。
    const TEST_KEY: [u8; 32] = [7u8; 32];

    /// 标准测试明文：9 字段按 TRAE 实测顺序排列。
    const PLAIN: &str = r#"{"token":"old-token","refreshToken":"old-refresh","expiredAt":"2026-08-01T00:00:00.000Z","refreshExpiredAt":"2026-11-01T00:00:00.000Z","tokenReleaseAt":"keep-me","userId":"123","userRegion":"cn","host":"api.trae.com.cn","account":"138****0000"}"#;

    /// 构造测试用 storage.json：与 TRAE 同格式加密明文并嵌入 cloudide 键。
    /// 顶层放三个键验证"其他键字节不动"（含 icube-dc 设备键铁律）。
    fn make_storage(plaintext_json: &str, key_material: &[u8; 32]) -> String {
        let b64 = reencrypt_blob(key_material, plaintext_json.as_bytes());
        format!(r#"{{"theme":"dark","icube-dc":"device-blob","{AUTH_KEY}":"{b64}"}}"#)
    }

    /// 建临时实例目录并写入 storage 内容，返回实例目录路径（tempdir keep）。
    fn write_instance(storage: &str) -> std::path::PathBuf {
        let dir = tempdir().unwrap();
        let instance_dir = dir.keep();
        let storage_dir = instance_dir.join("User").join("globalStorage");
        fs::create_dir_all(&storage_dir).unwrap();
        fs::write(storage_dir.join("storage.json"), storage).unwrap();
        instance_dir
    }

    fn read_storage(instance_dir: &Path) -> String {
        fs::read_to_string(instance_dir.join("User").join("globalStorage").join("storage.json"))
            .unwrap()
    }

    #[test]
    fn roundtrip_updates_credentials_and_keeps_other_fields() {
        let instance_dir = write_instance(&make_storage(PLAIN, &TEST_KEY));
        let outcome = writeback_auth_blob_with_processes(
            &instance_dir,
            "new-token",
            "new-refresh",
            1_800_000_000,
            1_900_000_000,
            &[],
        )
        .unwrap();
        assert_eq!(outcome, KeepaliveOutcome::Written);

        // 写回后解密：四个凭据字段为新值。
        let raw = read_storage(&instance_dir);
        let (plain, _) = decrypt_auth_blob(&raw).unwrap();
        assert_eq!(plain.get("token").and_then(|v| v.as_str()), Some("new-token"));
        assert_eq!(
            plain.get("refreshToken").and_then(|v| v.as_str()),
            Some("new-refresh")
        );
        assert_eq!(
            plain.get("expiredAt").and_then(|v| v.as_str()),
            Some("2027-01-15T08:00:00.000Z") // 1_800_000_000 的 UTC 表示
        );
        // 未更新字段原样保留。
        assert_eq!(
            plain.get("tokenReleaseAt").and_then(|v| v.as_str()),
            Some("keep-me")
        );
        // 顶层其他键（含 icube-dc 设备键）不动。
        let root: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(root.get("theme").and_then(|v| v.as_str()), Some("dark"));
        assert_eq!(
            root.get("icube-dc").and_then(|v| v.as_str()),
            Some("device-blob")
        );
    }

    #[test]
    fn digest_check_failure_rejects_and_leaves_file_untouched() {
        let mut storage = make_storage(PLAIN, &TEST_KEY);
        // 篡改 Base64 尾部一个字符 = 破坏密文（校验必然失败）。
        let last = storage.pop().unwrap();
        storage.push(if last == 'A' { 'B' } else { 'A' });
        let instance_dir = write_instance(&storage);

        let result = writeback_auth_blob_with_processes(
            &instance_dir,
            "new-token",
            "new-refresh",
            1,
            2,
            &[],
        );
        // Base64 破坏 → 解码失败或解密/摘要失败，均为 AuthBlobInvalid。
        assert_eq!(result.unwrap_err(), KeepaliveError::AuthBlobInvalid);
        // 文件未被修改。
        assert_eq!(read_storage(&instance_dir), storage);
    }

    #[test]
    fn key_material_is_preserved_across_writeback() {
        let instance_dir = write_instance(&make_storage(PLAIN, &TEST_KEY));
        let (_, key_before) = decrypt_auth_blob(&read_storage(&instance_dir)).unwrap();

        writeback_auth_blob_with_processes(&instance_dir, "new-token", "new-refresh", 1, 2, &[])
            .unwrap();

        let (_, key_after) = decrypt_auth_blob(&read_storage(&instance_dir)).unwrap();
        // 铁律：密钥材料不变（新密钥材料 = 陌生加密身份，TRAE 拒绝）。
        assert_eq!(key_before, key_after);
        assert_eq!(key_after, TEST_KEY);
    }

    #[test]
    fn skips_when_no_cloudide_key_or_no_storage_file() {
        // 无 cloudide 键：只有设备键的 storage.json。
        let instance_dir = write_instance(r#"{"icube-dc":"device-blob"}"#);
        let outcome =
            writeback_auth_blob_with_processes(&instance_dir, "t", "r", 1, 2, &[]).unwrap();
        assert_eq!(outcome, KeepaliveOutcome::Skipped);
        assert_eq!(read_storage(&instance_dir), r#"{"icube-dc":"device-blob"}"#);

        // 无 storage.json：实例从未启动。
        let dir = tempdir().unwrap();
        let empty_instance = dir.path().join("inst");
        fs::create_dir_all(empty_instance.join("User").join("globalStorage")).unwrap();
        let outcome = writeback_auth_blob_with_processes(&empty_instance, "t", "r", 1, 2, &[])
            .unwrap();
        assert_eq!(outcome, KeepaliveOutcome::Skipped);
    }

    #[test]
    fn idempotent_when_rewriting_with_same_parameters() {
        let instance_dir = write_instance(&make_storage(PLAIN, &TEST_KEY));
        writeback_auth_blob_with_processes(
            &instance_dir,
            "stable-token",
            "stable-refresh",
            1_800_000_000,
            1_900_000_000,
            &[],
        )
        .unwrap();
        let first = read_storage(&instance_dir);

        // 同参数再写：明文相同 → CBC 同 key/iv 同明文 → 密文相同 → 文件不变。
        writeback_auth_blob_with_processes(
            &instance_dir,
            "stable-token",
            "stable-refresh",
            1_800_000_000,
            1_900_000_000,
            &[],
        )
        .unwrap();
        assert_eq!(read_storage(&instance_dir), first);
    }

    #[test]
    fn skips_running_instance_by_command_line() {
        let instance_dir = write_instance(&make_storage(PLAIN, &TEST_KEY));
        // TRAE 主进程命令行含该实例目录（command_line_uses_data_dir 归一化引号与大小写）。
        let processes = [TraeProcessInfo {
            pid: 4242,
            exe_path: Some(r"E:\TRAE\TRAE SOLO CN.exe".to_string()),
            command_line: Some(format!(
                r#""E:\TRAE\TRAE SOLO CN.exe" --user-data-dir={}"#,
                instance_dir.display()
            )),
        }];
        let outcome = writeback_auth_blob_with_processes(
            &instance_dir,
            "new-token",
            "new-refresh",
            1,
            2,
            &processes,
        )
        .unwrap();
        assert_eq!(outcome, KeepaliveOutcome::SkippedInstanceRunning);

        // 无关目录的进程不影响写回。
        let other = [TraeProcessInfo {
            pid: 4243,
            exe_path: None,
            command_line: Some(
                r#""E:\TRAE\TRAE SOLO CN.exe" --user-data-dir=D:\other"#.to_string(),
            ),
        }];
        let outcome =
            writeback_auth_blob_with_processes(&instance_dir, "t", "r", 1, 2, &other).unwrap();
        assert_eq!(outcome, KeepaliveOutcome::Written);
    }

    #[test]
    fn missing_plaintext_fields_are_not_added() {
        // 明文缺 refreshToken：更新不得新增字段（只更新已存在字段）。
        let plain = r#"{"token":"old-token","expiredAt":"2026-08-01T00:00:00.000Z"}"#;
        let instance_dir = write_instance(&make_storage(plain, &TEST_KEY));
        writeback_auth_blob_with_processes(
            &instance_dir,
            "new-token",
            "new-refresh",
            1,
            2,
            &[],
        )
        .unwrap();
        let (after, _) = decrypt_auth_blob(&read_storage(&instance_dir)).unwrap();
        assert!(after.get("refreshToken").is_none());
        assert!(after.get("refreshExpiredAt").is_none());
        assert_eq!(after.get("token").and_then(|v| v.as_str()), Some("new-token"));
    }

    #[test]
    fn iso8601_conversion_matches_known_timestamps() {
        // 2026-01-01T00:00:00Z = 1767225600（对照闰年表手算）。
        assert_eq!(iso8601_utc(1_767_225_600), "2026-01-01T00:00:00.000Z");
        // 2000-02-29（闰日）= 951782400。
        assert_eq!(iso8601_utc(951_782_400), "2000-02-29T00:00:00.000Z");
        // 1_800_000_000 = 2027-01-15T08:00:00Z（roundtrip 测试引用）。
        assert_eq!(iso8601_utc(1_800_000_000), "2027-01-15T08:00:00.000Z");
    }

    #[test]
    fn decrypt_rejects_wrong_key_material() {
        let storage = make_storage(PLAIN, &TEST_KEY);
        let root: serde_json::Value = serde_json::from_str(&storage).unwrap();
        let b64 = root.get(AUTH_KEY).unwrap().as_str().unwrap().to_string();
        // 用错误密钥材料拼一个同结构 blob（头 + 错材料 + 原密文）。
        let blob = STANDARD.decode(b64.as_bytes()).unwrap();
        let wrong_material = [9u8; 32];
        let mut forged = Vec::new();
        forged.extend_from_slice(&blob[0..6]);
        forged.extend_from_slice(&wrong_material);
        forged.extend_from_slice(&blob[38..]);
        let forged_b64 = STANDARD.encode(&forged);
        let forged_storage = storage.replace(&b64, &forged_b64);
        // 派生密钥不同 → 解出的摘要必不匹配 → AuthBlobInvalid。
        assert_eq!(
            decrypt_auth_blob(&forged_storage).unwrap_err(),
            KeepaliveError::AuthBlobInvalid
        );
        // 对照：正确材料的原文件可解密。
        assert!(decrypt_auth_blob(&storage).is_ok());
    }

    /// 供体账号 B 的明文（与 PLAIN 仅 userId/account/token 不同）。
    const DONOR_PLAIN: &str = r#"{"token":"donor-token","refreshToken":"donor-refresh","expiredAt":"2026-10-01T00:00:00.000Z","refreshExpiredAt":"2027-01-01T00:00:00.000Z","tokenReleaseAt":"donor-release","userId":"456","userRegion":"cn","host":"api.trae.com.cn","account":"139****0000"}"#;

    #[test]
    fn switch_identity_transplants_donor_plaintext_byte_exactly() {
        // 供体（账号 B）与目标（主库，账号 A=123）用不同密钥材料。
        let donor_dir = write_instance(&make_storage(DONOR_PLAIN, &[8u8; 32]));
        let target_dir = write_instance(&make_storage(PLAIN, &TEST_KEY));
        let target_before = read_storage(&target_dir);

        let report = switch_auth_identity(&donor_dir, &target_dir).unwrap();
        assert_eq!(report.from_user_id, "123");
        assert_eq!(report.to_user_id, "456");
        assert_eq!(report.to_account, "139****0000");

        // 目标文件：登录身份 = 供体明文原字节（含全部 9 字段），密钥材料 = 目标原值。
        let raw_after = read_storage(&target_dir);
        let (plain, key) = decrypt_auth_blob(&raw_after).unwrap();
        assert_eq!(key, TEST_KEY);
        assert_eq!(
            plain.get("token").and_then(|v| v.as_str()),
            Some("donor-token")
        );
        assert_eq!(plain.get("userId").and_then(|v| v.as_str()), Some("456"));
        assert_eq!(
            plain.get("tokenReleaseAt").and_then(|v| v.as_str()),
            Some("donor-release")
        );
        // 顶层其他键（含 icube-dc 设备键）字节级不动。
        let root: serde_json::Value = serde_json::from_str(&raw_after).unwrap();
        assert_eq!(
            root.get("icube-dc").and_then(|v| v.as_str()),
            Some("device-blob")
        );
        assert_eq!(root.get("theme").and_then(|v| v.as_str()), Some("dark"));
        // 供体文件只读不写。
        assert_eq!(read_storage(&donor_dir), make_storage(DONOR_PLAIN, &[8u8; 32]));
        // 目标确有变化（非同文件幂等误判）。
        assert_ne!(raw_after, target_before);
    }

    #[test]
    fn archive_login_user_id_reads_donor_and_fails_closed() {
        // 有效存档：返回登录账号 userId（PLAIN 内 userId=123）。
        let donor_dir = write_instance(&make_storage(PLAIN, &[8u8; 32]));
        assert_eq!(archive_login_user_id(&donor_dir).as_deref(), Some("123"));

        // 目录不存在 / 无登录键：None（调用方按不可跳过处理）。
        let missing = tempdir().unwrap();
        assert_eq!(archive_login_user_id(missing.path()), None);
        let no_login_key = write_instance(r#"{"icube-dc":"device-blob"}"#);
        assert_eq!(archive_login_user_id(&no_login_key), None);
    }

    #[test]
    fn switch_identity_rejects_same_account_and_missing_donor() {
        // 同账号：供体与目标明文相同 → SameAccount，文件不动。
        let donor_dir = write_instance(&make_storage(PLAIN, &[8u8; 32]));
        let target_dir = write_instance(&make_storage(PLAIN, &TEST_KEY));
        let before = read_storage(&target_dir);
        assert_eq!(
            switch_auth_identity(&donor_dir, &target_dir).unwrap_err(),
            AuthSwitchError::SameAccount
        );
        assert_eq!(read_storage(&target_dir), before);

        // 供体无登录凭据（未登录过实例）：DonorAuthUnavailable。
        let empty_donor = {
            let dir = tempdir().unwrap();
            let instance_dir = dir.path().join("inst");
            fs::create_dir_all(instance_dir.join("User").join("globalStorage")).unwrap();
            fs::write(
                instance_dir.join("User").join("globalStorage").join("storage.json"),
                r#"{"icube-dc":"device-blob"}"#,
            )
            .unwrap();
            instance_dir
        };
        assert_eq!(
            switch_auth_identity(&empty_donor, &target_dir).unwrap_err(),
            AuthSwitchError::DonorAuthUnavailable
        );

        // 目标无登录凭据（主库尚未登录过一次）：TargetAuthUnavailable。
        let empty_target = {
            let dir = tempdir().unwrap();
            let instance_dir = dir.path().join("inst");
            fs::create_dir_all(instance_dir.join("User").join("globalStorage")).unwrap();
            instance_dir
        };
        let donor_b = write_instance(&make_storage(DONOR_PLAIN, &[8u8; 32]));
        assert_eq!(
            switch_auth_identity(&donor_b, &empty_target).unwrap_err(),
            AuthSwitchError::TargetAuthUnavailable
        );
    }

    /// E2 构造输入 fixture：账号 B（userId=456）的凭据包 + GetUserInfo 资料。
    fn make_construct_input() -> (
        String,
        String,
        u64,
        u64,
        crate::checkin_http::UserInfoFull,
    ) {
        let user_info = crate::checkin_http::UserInfoFull {
            user_id: "456".to_string(),
            screen_name: "用户B".to_string(),
            avatar_url: "https://example.com/b.png".to_string(),
            masked_mobile: "139****0000".to_string(),
            masked_email: "b***@example.com".to_string(),
            description: "简介B".to_string(),
            region: "CN".to_string(),
            ai_region: "CN".to_string(),
            migrate_to_sg: false,
        };
        (
            "456".to_string(),
            "constructed-token".to_string(),
            1_800_000_000,
            1_900_000_000,
            user_info,
        )
    }

    #[test]
    fn construct_identity_writes_full_login_state_with_target_key() {
        // E2 构造：目标（主库，账号 A=123）→ 凭据包构造的账号 B 登录态。
        let target_dir = write_instance(&make_storage(PLAIN, &TEST_KEY));
        let target_before = read_storage(&target_dir);
        let (account_id, access_token, access_exp, refresh_exp, user_info) =
            make_construct_input();

        let input = ConstructAuthInput {
            account_id: &account_id,
            access_token: &access_token,
            refresh_token: "constructed-refresh",
            access_token_expires_at_unix_seconds: access_exp,
            refresh_token_expires_at_unix_seconds: refresh_exp,
            user_info: &user_info,
        };
        let report = construct_auth_identity(&input, &target_dir).unwrap();
        assert_eq!(report.from_user_id, "123");
        assert_eq!(report.to_user_id, "456");
        assert_eq!(report.to_account, "139****0000");

        // 写回明文 = 构造的完整登录态（凭据包五件套 + GetUserInfo 资料 +
        // TRAE CN 恒定值），密钥材料 = 目标原值。
        let raw_after = read_storage(&target_dir);
        let (plain, key) = decrypt_auth_blob(&raw_after).unwrap();
        assert_eq!(key, TEST_KEY);
        assert_eq!(
            plain.get("token").and_then(|v| v.as_str()),
            Some("constructed-token")
        );
        assert_eq!(plain.get("userId").and_then(|v| v.as_str()), Some("456"));
        assert_eq!(
            plain.get("expiredAt").and_then(|v| v.as_str()),
            Some("2027-01-15T08:00:00.000Z")
        );
        // GetUserInfo 资料字段与区域结构。
        assert_eq!(
            plain.pointer("/account/username").and_then(|v| v.as_str()),
            Some("用户B")
        );
        assert_eq!(
            plain.pointer("/account/nonPlainTextMobile").and_then(|v| v.as_str()),
            Some("139****0000")
        );
        assert_eq!(
            plain.pointer("/userRegion/region").and_then(|v| v.as_str()),
            Some("CN")
        );
        // TRAE CN 恒定值。
        assert_eq!(plain.get("host").and_then(|v| v.as_str()), Some("https://api.trae.cn"));
        assert_eq!(
            plain.pointer("/account/scope").and_then(|v| v.as_str()),
            Some("marscode")
        );
        // 顶层其他键（含 icube-dc 设备键）字节级不动；目标确有变化。
        let root: serde_json::Value = serde_json::from_str(&raw_after).unwrap();
        assert_eq!(
            root.get("icube-dc").and_then(|v| v.as_str()),
            Some("device-blob")
        );
        assert_eq!(root.get("theme").and_then(|v| v.as_str()), Some("dark"));
        assert_ne!(raw_after, target_before);
    }

    #[test]
    fn construct_identity_rejects_same_account_and_missing_target() {
        let (account_id, access_token, access_exp, refresh_exp, user_info) =
            make_construct_input();

        // 同账号：主库已是账号 B（构造输入 userId）→ SameAccount，文件不动。
        let target_b = write_instance(&make_storage(DONOR_PLAIN, &TEST_KEY));
        let before = read_storage(&target_b);
        let input = ConstructAuthInput {
            account_id: &account_id,
            access_token: &access_token,
            refresh_token: "constructed-refresh",
            access_token_expires_at_unix_seconds: access_exp,
            refresh_token_expires_at_unix_seconds: refresh_exp,
            user_info: &user_info,
        };
        assert_eq!(
            construct_auth_identity(&input, &target_b).unwrap_err(),
            AuthSwitchError::SameAccount
        );
        assert_eq!(read_storage(&target_b), before);
    }

    #[test]
    fn construct_identity_into_logged_out_target() {
        // 官方退出后的主库形态（2026-09-03 卡死案例根因）：storage.json
        // 有其他键（theme/icube-dc）但 cloudide 登录键已被官方退出清除。
        // E2 应以全新密钥材料构造登录态写入，而非报 TargetAuthUnavailable。
        let instance_dir = {
            let dir = tempdir().unwrap();
            let instance_dir = dir.path().join("inst");
            fs::create_dir_all(instance_dir.join("User").join("globalStorage")).unwrap();
            fs::write(
                instance_dir
                    .join("User")
                    .join("globalStorage")
                    .join("storage.json"),
                r#"{"theme":"dark","icube-dc":"device-blob"}"#,
            )
            .unwrap();
            dir.keep().join("inst")
        };
        let (account_id, access_token, access_exp, refresh_exp, user_info) =
            make_construct_input();
        let input = ConstructAuthInput {
            account_id: &account_id,
            access_token: &access_token,
            refresh_token: "constructed-refresh",
            access_token_expires_at_unix_seconds: access_exp,
            refresh_token_expires_at_unix_seconds: refresh_exp,
            user_info: &user_info,
        };
        let report = construct_auth_identity(&input, &instance_dir).unwrap();

        // from 为空串（主库登出态无先前登录），to 为构造身份。
        assert_eq!(report.from_user_id, "");
        assert_eq!(report.to_user_id, "456");

        // 写入的登录键可解密且明文 = 构造身份（blob 自包含：密钥材料
        // 内嵌于 blob，全新材料与 TRAE 首次登录自生成的形态一致）。
        let raw_after = read_storage(&instance_dir);
        let (plain, _key) = decrypt_auth_blob(&raw_after).unwrap();
        assert_eq!(plain.get("userId").and_then(|v| v.as_str()), Some("456"));
        assert_eq!(
            plain.get("token").and_then(|v| v.as_str()),
            Some("constructed-token")
        );
        // 其他键原样保留（含设备键铁律）。
        let root: serde_json::Value = serde_json::from_str(&raw_after).unwrap();
        assert_eq!(root.get("theme").and_then(|v| v.as_str()), Some("dark"));
        assert_eq!(
            root.get("icube-dc").and_then(|v| v.as_str()),
            Some("device-blob")
        );
        assert!(root.get(AUTH_KEY).and_then(|v| v.as_str()).is_some());
    }

    #[test]
    fn construct_identity_creates_missing_storage_file() {
        // 更极端形态：实例目录连 storage.json 都没有（全新环境首次登录）。
        // 构造应创建文件并只写登录键。
        let instance_dir = {
            let dir = tempdir().unwrap();
            dir.keep().join("fresh-env").join("User").join("globalStorage")
        };
        let (account_id, access_token, access_exp, refresh_exp, user_info) =
            make_construct_input();
        let input = ConstructAuthInput {
            account_id: &account_id,
            access_token: &access_token,
            refresh_token: "constructed-refresh",
            access_token_expires_at_unix_seconds: access_exp,
            refresh_token_expires_at_unix_seconds: refresh_exp,
            user_info: &user_info,
        };
        let report = construct_auth_identity(&input, &instance_dir).unwrap();
        assert_eq!(report.from_user_id, "");
        assert_eq!(report.to_user_id, "456");

        let raw = read_storage(&instance_dir);
        let (plain, _) = decrypt_auth_blob(&raw).unwrap();
        assert_eq!(plain.get("userId").and_then(|v| v.as_str()), Some("456"));
        // 文件恰为一个键的合法 JSON。
        let root: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(root.as_object().map(|m| m.len()), Some(1));
    }
}
