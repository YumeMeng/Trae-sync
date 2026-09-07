//! T16 签到 Profile 凭据包与无进程续期（fixture 范围）。
//!
//! 实现 ADR-0014 中可在本地验证的凭据生命周期：
//! - DPAPI 加密凭据包（账号 ID、设备 ID、设备密钥对、Token 等绑定材料）；
//! - 账号 ID、设备 ID、设备公钥绑定校验失败时零回写；
//! - `ExchangeToken` 的 `DeviceProof` 设备签名：EC P-256 密钥对 +
//!   ECDSA-SHA256（DER 签名），与真实客户端（main.js z7e 函数）一致；
//!   签名输入按 HTTP 方法、请求路径、ClientID、refresh token、
//!   时间戳、nonce 的固定顺序拼接；
//! - refresh token 轮换、`GetUserInfo` 身份校验与旧凭据保留；
//! - Profile 写回：旧凭据备份、临时文件、解密校验、原子替换、
//!   重新读取验证和中断恢复 journal。
//!
//! 真实 HTTP transport 仍由独立 CheckinCapability Gate 控制；本模块的
//! 远程端点全部为内存 fixture，不产生任何网络副作用，也不进入前端 DTO。

use std::collections::BTreeMap;
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use openssl::ec::{EcGroup, EcKey};
use openssl::error::ErrorStack;
use openssl::hash::MessageDigest;
use openssl::nid::Nid;
use openssl::pkey::{Id, PKey, Private, Public};
use openssl::sign::{Signer, Verifier};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::atomic_publish::publish_replacing;
use crate::key_wrapper::{protect_secret, unprotect_secret};

const BUNDLE_FORMAT_VERSION: u32 = 1;
const BUNDLE_MAGIC: &[u8; 8] = b"TRVCKN01";
const JOURNAL_OPERATION: &str = "checkin_credential_renewal";
const EXCHANGE_METHOD: &str = "POST";
const EXCHANGE_PATH: &str = "/trae/api/v3/oauth/ExchangeToken";
/// 访问令牌剩余寿命低于该秒数（7 天）时需要续期。
pub const ACCESS_TOKEN_REFRESH_THRESHOLD_SECONDS: u64 = 7 * 24 * 60 * 60;
/// refresh token 剩余寿命低于该秒数（30 天）时需要续期。
pub const REFRESH_TOKEN_REFRESH_THRESHOLD_SECONDS: u64 = 30 * 24 * 60 * 60;
/// 真实协议实测：`ExchangeToken` 签发的访问令牌有效 14 天。
pub const ACCESS_TOKEN_LIFETIME_SECONDS: u64 = 14 * 24 * 60 * 60;
/// 真实协议实测：`ExchangeToken` 签发的 refresh token 有效 180 天。
pub const REFRESH_TOKEN_LIFETIME_SECONDS: u64 = 180 * 24 * 60 * 60;
const MAX_BUNDLE_BYTES: u64 = 1024 * 1024;
const MAX_MANIFEST_BYTES: u64 = 64 * 1024;
const MAX_PROFILE_ID_BYTES: usize = 256;
const MAX_ID_FIELD_BYTES: usize = 512;
const MAX_TOKEN_FIELD_BYTES: usize = 4096;
/// 设备密钥 PEM 字符串长度上限（P-256 实际约 230 字节，留足余量）。
const MAX_DEVICE_KEY_PEM_BYTES: usize = 2048;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckinCredentialError {
    /// 当前平台没有 DPAPI（非 Windows），凭据包不可用。
    Unavailable,
    /// 凭据包不存在。
    Missing,
    /// 凭据包密文、格式或字段无效。
    Invalid,
    /// 账号 ID、设备 ID 或设备公钥绑定不匹配。
    BindingMismatch,
    /// `ExchangeToken` 失败（含设备签名验证失败）。
    CredentialRefreshFailed,
    /// `GetUserInfo` 身份校验失败。
    AuthMismatch,
    /// 存在中断的写回 journal，需要先恢复。
    RecoveryRequired,
}

impl fmt::Display for CheckinCredentialError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::Unavailable => "签到凭据包在当前平台不可用",
            Self::Missing => "签到凭据包不存在",
            Self::Invalid => "签到凭据包无效",
            Self::BindingMismatch => "签到凭据绑定不匹配",
            Self::CredentialRefreshFailed => "签到凭据续期失败",
            Self::AuthMismatch => "续期后账号身份校验失败",
            Self::RecoveryRequired => "签到凭据写回需要人工恢复",
        };
        formatter.write_str(message)
    }
}

impl std::error::Error for CheckinCredentialError {}

/// Profile 期望的凭据绑定；只含非敏感身份字段，不含 Token 或私钥。
///
/// 绑定中的设备公钥用于校验凭据包没有被跨 Profile/跨设备串用；账号 ID、
/// 设备 ID 或设备公钥任一不匹配时，读取与写回都必须零回写。
#[derive(Clone, PartialEq, Eq)]
pub struct CheckinProfileBinding {
    pub profile_id: String,
    pub account_id: String,
    pub device_id: String,
    /// EC P-256 设备公钥（SPKI PEM 字符串）。
    pub device_public_key: String,
}

impl CheckinProfileBinding {
    pub fn new(
        profile_id: impl Into<String>,
        account_id: impl Into<String>,
        device_id: impl Into<String>,
        device_public_key: impl Into<String>,
    ) -> Self {
        Self {
            profile_id: profile_id.into(),
            account_id: account_id.into(),
            device_id: device_id.into(),
            device_public_key: device_public_key.into(),
        }
    }
}

/// 明文签到凭据包。
///
/// 只允许存在于内存或 DPAPI 密文中：不得进入 domain 账号档案、目录库、
/// 操作 manifest、结构化日志、导出或前端 DTO。因此本类型不实现 Debug，
/// 测试断言也只使用布尔比较，避免凭据值进入失败输出。
#[derive(Clone)]
pub struct CheckinCredentialBundle {
    pub profile_id: String,
    pub account_id: String,
    pub device_id: String,
    /// 虚拟设备机器指纹（登录时生成）；refresh 续期的 DeviceInfo 需与登录一致。
    pub machine_id: String,
    pub device_public_key: String,
    pub device_private_key: String,
    pub access_token: String,
    pub refresh_token: String,
    pub client_id: String,
    /// 访问令牌过期时刻（Unix 秒）。真实协议实测：签发后 14 天。
    pub access_token_expires_at_unix_seconds: u64,
    /// refresh token 过期时刻（Unix 秒）。真实协议实测：签发后 180 天。
    pub refresh_token_expires_at_unix_seconds: u64,
    /// 完整手机号（G11 手工补录）：None = 未补录（展示回退脱敏号）。
    /// 敏感字段与令牌同级 DPAPI 加密存储；格式校验在命令层（三层校验）。
    pub mobile_full: Option<String>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SerializableBundle {
    format_version: u32,
    profile_id: String,
    account_id: String,
    device_id: String,
    /// 旧版本凭据包无此字段；`default` 兼容读取（无线上存量数据，仅防御）。
    #[serde(default)]
    machine_id: String,
    device_public_key: String,
    device_private_key: String,
    access_token: String,
    refresh_token: String,
    client_id: String,
    access_token_expires_at_unix_seconds: u64,
    refresh_token_expires_at_unix_seconds: u64,
    /// 完整手机号（G11）：旧凭据包无此字段，default 兼容读取为 None。
    #[serde(default)]
    mobile_full: Option<String>,
}

/// 按 TRAE 既有顺序拼接 `DeviceProof` 签名输入：
/// HTTP 方法、请求路径、ClientID、refresh token、时间戳、nonce。
pub fn device_proof_signing_input(
    method: &str,
    path: &str,
    client_id: &str,
    refresh_token: &str,
    timestamp_unix_seconds: u64,
    nonce: &str,
) -> Vec<u8> {
    fn push_field(input: &mut Vec<u8>, field: &str) {
        input.extend_from_slice(field.as_bytes());
        input.push(b'\n');
    }
    let mut input = Vec::new();
    push_field(&mut input, method);
    push_field(&mut input, path);
    push_field(&mut input, client_id);
    push_field(&mut input, refresh_token);
    push_field(&mut input, &timestamp_unix_seconds.to_string());
    push_field(&mut input, nonce);
    input
}

/// 生成设备密钥对（EC P-256）；返回 (私钥 PKCS#8 PEM, 公钥 SPKI PEM)。
///
/// 与真实客户端存储格式一致（`iCubeAuthInfo://icube-dc:<deviceId>` 解密后
/// 的 `privateKeyPEM`/`publicKeyPEM` 字段），真实接入时可直接互通。
pub fn generate_device_keypair() -> Result<(String, String), CheckinCredentialError> {
    let group = EcGroup::from_curve_name(Nid::X9_62_PRIME256V1)
        .map_err(|_| CheckinCredentialError::Invalid)?;
    let ec_key = EcKey::generate(&group).map_err(|_| CheckinCredentialError::Invalid)?;
    let keypair = PKey::from_ec_key(ec_key).map_err(|_| CheckinCredentialError::Invalid)?;
    // 私钥导出 PKCS#8 DER 后手工包装为 PEM（与真实客户端存储格式一致）；
    // 公钥直接导出 SPKI PEM。
    let pkcs8_der = keypair
        .private_key_to_pkcs8()
        .map_err(|_| CheckinCredentialError::Invalid)?;
    let private_pem = wrap_der_as_pem(&pkcs8_der, "PRIVATE KEY");
    let public_pem = pem_to_string(keypair.public_key_to_pem())?;
    Ok((private_pem, public_pem))
}

/// 将 DER 密钥字节按 64 列 base64 包装为标准 PEM 文本。
fn wrap_der_as_pem(der: &[u8], label: &str) -> String {
    let encoded = openssl::base64::encode_block(der);
    let mut pem = format!("-----BEGIN {label}-----\n");
    for chunk in encoded.as_bytes().chunks(64) {
        pem.push_str(std::str::from_utf8(chunk).unwrap_or(""));
        pem.push('\n');
    }
    pem.push_str(&format!("-----END {label}-----\n"));
    pem
}

/// 用设备私钥对签名输入生成 ECDSA-SHA256 签名（DER 编码）。
///
/// 输出格式与真实客户端（Node.js `crypto.sign("sha256", ...)` 默认 DER）
/// 一致；真实 HTTP transport 传输时再做 base64 编码。
pub fn sign_device_proof(
    device_private_key_pem: &str,
    input: &[u8],
) -> Result<Vec<u8>, CheckinCredentialError> {
    let private_key = parse_private_key_pem(device_private_key_pem)?;
    let mut signer = Signer::new(MessageDigest::sha256(), &private_key)
        .map_err(|_| CheckinCredentialError::Invalid)?;
    signer
        .update(input)
        .map_err(|_| CheckinCredentialError::Invalid)?;
    signer
        .sign_to_vec()
        .map_err(|_| CheckinCredentialError::Invalid)
}

/// 用设备公钥验证 `DeviceProof` 签名（ECDSA-SHA256，DER 编码）。
pub fn verify_device_proof(
    device_public_key_pem: &str,
    input: &[u8],
    signature: &[u8],
) -> Result<bool, CheckinCredentialError> {
    let public_key = parse_public_key_pem(device_public_key_pem)?;
    let mut verifier = Verifier::new(MessageDigest::sha256(), &public_key)
        .map_err(|_| CheckinCredentialError::Invalid)?;
    verifier
        .update(input)
        .map_err(|_| CheckinCredentialError::Invalid)?;
    // 签名字节损坏（DER 解析失败）属于"验证不通过"而非协议错误；
    // 与 Node.js `crypto.verify` 对非法签名返回 false 的行为一致。
    Ok(verifier.verify(signature).unwrap_or(false))
}

/// 最近一次 `ExchangeToken` 请求的非敏感摘录。
///
/// 只保留字段值与 token 指纹，用于测试断言签名输入的构成；不保留
/// Token、refresh token 或签名正文本身。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FixtureExchangeRequest {
    pub method: String,
    pub path: String,
    pub client_id: String,
    pub access_token_sha256: String,
    pub refresh_token_sha256: String,
    pub timestamp_unix_seconds: u64,
    pub nonce: String,
    pub signature_verified: bool,
}

/// 内存版 `ExchangeToken` fixture：模拟服务端设备绑定与 `DeviceProof` 验签。
///
/// 设备公钥登记与 Token 轮换值由测试配置，不产生网络副作用。
pub struct FixtureTokenEndpoint {
    registered_devices: BTreeMap<String, String>,
    issued_access_token: String,
    rotated_refresh_token: Option<String>,
    last_request: Mutex<Option<FixtureExchangeRequest>>,
}

impl FixtureTokenEndpoint {
    pub fn new(
        registered_devices: BTreeMap<String, String>,
        issued_access_token: impl Into<String>,
        rotated_refresh_token: Option<String>,
    ) -> Self {
        Self {
            registered_devices,
            issued_access_token: issued_access_token.into(),
            rotated_refresh_token,
            last_request: Mutex::new(None),
        }
    }

    /// 最近一次请求的非敏感摘录。
    pub fn last_request(&self) -> Option<FixtureExchangeRequest> {
        self.last_request
            .lock()
            .ok()
            .and_then(|guard| guard.clone())
    }

    /// 处理一次 `ExchangeToken`：按登记设备公钥重建签名输入并验签。
    fn exchange_token(
        &self,
        profile_id: &str,
        access_token: &str,
        client_id: &str,
        refresh_token: &str,
        timestamp_unix_seconds: u64,
        nonce: &str,
        signature: &[u8],
    ) -> Result<(String, Option<String>), CheckinCredentialError> {
        let Some(registered_public_key) = self.registered_devices.get(profile_id) else {
            return Err(CheckinCredentialError::CredentialRefreshFailed);
        };
        let input = device_proof_signing_input(
            EXCHANGE_METHOD,
            EXCHANGE_PATH,
            client_id,
            refresh_token,
            timestamp_unix_seconds,
            nonce,
        );
        let signature_verified =
            verify_device_proof(registered_public_key, &input, signature).unwrap_or(false);
        let request = FixtureExchangeRequest {
            method: EXCHANGE_METHOD.to_string(),
            path: EXCHANGE_PATH.to_string(),
            client_id: client_id.to_string(),
            access_token_sha256: sha256_hex(access_token.as_bytes()),
            refresh_token_sha256: sha256_hex(refresh_token.as_bytes()),
            timestamp_unix_seconds,
            nonce: nonce.to_string(),
            signature_verified,
        };
        if let Ok(mut guard) = self.last_request.lock() {
            *guard = Some(request);
        }
        if !signature_verified {
            return Err(CheckinCredentialError::CredentialRefreshFailed);
        }
        Ok((
            self.issued_access_token.clone(),
            self.rotated_refresh_token.clone(),
        ))
    }
}

/// 内存版 `GetUserInfo` fixture：access_token -> 账号 ID。
pub struct FixtureUserInfoEndpoint {
    identities: BTreeMap<String, String>,
}

impl FixtureUserInfoEndpoint {
    pub fn new(identities: BTreeMap<String, String>) -> Self {
        Self { identities }
    }

    /// 只读身份查询；返回账号 ID 或 `None`（Token 无效）。
    pub fn account_id_for(&self, access_token: &str) -> Option<String> {
        self.identities.get(access_token).cloned()
    }
}

/// 中断写回的非敏感摘要。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingRenewalRecord {
    pub operation_id: String,
    pub state: String,
    pub profile_id: String,
}

/// 续期成功回执；只含非敏感字段。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenewalReceipt {
    pub profile_id: String,
    pub operation_id: String,
    pub refresh_token_rotated: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RenewalManifest {
    format_version: u32,
    operation_id: String,
    operation: String,
    sequence: u64,
    state: String,
    profile_id: String,
    backup_file: String,
    temporary_file: String,
    before_sha256: String,
    target_sha256: String,
    created_at_unix_seconds: u64,
}

impl RenewalManifest {
    /// 推进到下一状态；状态转移合法性由调用方保证。
    fn next(&self, state: &str) -> Self {
        Self {
            sequence: self.sequence.saturating_add(1),
            state: state.to_string(),
            ..self.clone()
        }
    }
}

/// 签到 Profile 凭据存储；凭据文件为 DPAPI 密文容器。
/// 凭据仓库：只承载目录路径（DPAPI 密文在文件中），克隆零成本。
#[derive(Clone)]
pub struct CheckinCredentialStore {
    root: PathBuf,
}

impl CheckinCredentialStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// 加密保存凭据包（初始建档）。原子写入，不产生 journal。
    pub fn save(&self, bundle: &CheckinCredentialBundle) -> Result<(), CheckinCredentialError> {
        let payload = encode_bundle_payload(bundle)?;
        let destination = self.credential_path(&bundle.profile_id)?;
        write_file_atomically(&destination, &payload)
    }

    /// 解密读取并校验绑定；任何失败都不产生写副作用。
    pub fn load(
        &self,
        binding: &CheckinProfileBinding,
    ) -> Result<CheckinCredentialBundle, CheckinCredentialError> {
        validate_binding(binding)?;
        let path = self.credential_path(&binding.profile_id)?;
        let payload = read_credential_payload(&path)?;
        let bundle = decode_bundle_payload(&payload)?;
        if !bundle_matches_binding(&bundle, binding) {
            return Err(CheckinCredentialError::BindingMismatch);
        }
        Ok(bundle)
    }

    /// G11 手机号补录：更新凭据包中的完整手机号（None = 清除，回退脱敏号展示）。
    /// 读-改-写整包（DPAPI 加密，与令牌同级安全存储）；格式与脱敏号比对等
    /// 三层校验在命令层（lib.rs），此处只做长度防御（validate_bundle）。
    pub fn set_mobile_full(
        &self,
        binding: &CheckinProfileBinding,
        mobile: Option<&str>,
    ) -> Result<(), CheckinCredentialError> {
        let mut bundle = self.load(binding)?;
        bundle.mobile_full = mobile.map(str::to_string);
        self.save(&bundle)
    }

    /// 是否存在未收口的中断写回。
    pub fn has_pending_renewal(&self) -> Result<bool, CheckinCredentialError> {
        Ok(!self.pending_renewals()?.is_empty())
    }

    /// 从指定凭据文件读取并解码凭据包（退役恢复用；只读，不写任何文件）。
    ///
    /// 路径通常为退役目录 `retired/{account_id}-{时间戳}.checkin`；文件为
    /// 与标准凭据相同的 DPAPI 密文容器（TRVCKN01 格式）。
    pub fn read_bundle_at(
        &self,
        path: &Path,
    ) -> Result<CheckinCredentialBundle, CheckinCredentialError> {
        let payload = read_credential_payload(path)?;
        decode_bundle_payload(&payload)
    }

    /// 枚举中断写回的非敏感摘要；只读，不创建或清理任何文件。
    pub fn pending_renewals(&self) -> Result<Vec<PendingRenewalRecord>, CheckinCredentialError> {
        let mut records = Vec::new();
        let journal_root = self.root.join("renewals");
        let metadata = match fs::symlink_metadata(&journal_root) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(records),
            Err(_) => return Err(CheckinCredentialError::RecoveryRequired),
        };
        if is_reparse_or_symlink(&metadata) || !metadata.is_dir() {
            return Err(CheckinCredentialError::RecoveryRequired);
        }
        let entries =
            fs::read_dir(&journal_root).map_err(|_| CheckinCredentialError::RecoveryRequired)?;
        for entry in entries {
            let entry = entry.map_err(|_| CheckinCredentialError::RecoveryRequired)?;
            let metadata = fs::symlink_metadata(entry.path())
                .map_err(|_| CheckinCredentialError::RecoveryRequired)?;
            if is_reparse_or_symlink(&metadata) || !metadata.is_dir() {
                return Err(CheckinCredentialError::RecoveryRequired);
            }
            if let Some(manifest) = read_manifest_if_present(&entry.path().join("manifest.json"))? {
                if is_pending_state(&manifest.state) {
                    records.push(PendingRenewalRecord {
                        operation_id: manifest.operation_id,
                        state: manifest.state,
                        profile_id: manifest.profile_id,
                    });
                }
            }
        }
        records.sort_by(|left, right| left.operation_id.cmp(&right.operation_id));
        Ok(records)
    }

    /// 恢复中断的写回：
    /// - `prepared` 且旧凭据完好：收口 journal，凭据保持不变；
    /// - `replaced` 且新密文已落盘：从备份原子恢复旧凭据并复核；
    /// - 备份、哈希或现场无法确认：保留现场并要求人工恢复。
    ///
    /// 恢复过程不删除备份或任何失败证据。
    pub fn recover_interrupted_renewal(&self) -> Result<(), CheckinCredentialError> {
        for record in self.pending_renewals()? {
            let journal_dir = self.root.join("renewals").join(&record.operation_id);
            let manifest_path = journal_dir.join("manifest.json");
            let manifest = read_manifest_if_present(&manifest_path)?
                .ok_or(CheckinCredentialError::RecoveryRequired)?;
            let credential_path = self.credential_path(&manifest.profile_id)?;
            let current =
                fs::read(&credential_path).map_err(|_| CheckinCredentialError::RecoveryRequired)?;
            let backup = fs::read(journal_dir.join("bundle.before"))
                .map_err(|_| CheckinCredentialError::RecoveryRequired)?;
            // 备份必须与 manifest 记录的旧凭据一致，否则现场不可信。
            if manifest.before_sha256 != sha256_hex(&backup) {
                return Err(CheckinCredentialError::RecoveryRequired);
            }
            match manifest.state.as_str() {
                "prepared" => {
                    if sha256_hex(&current) == manifest.before_sha256 {
                        // 替换未发生，旧凭据完好；仅收口 journal。
                        write_manifest(&manifest_path, &manifest.next("aborted"))?;
                    } else {
                        return Err(CheckinCredentialError::RecoveryRequired);
                    }
                }
                "replaced" => {
                    if sha256_hex(&current) == manifest.before_sha256 {
                        write_manifest(&manifest_path, &manifest.next("aborted"))?;
                    } else if sha256_hex(&current) == manifest.target_sha256 {
                        // 替换已发生但未通过验证：从备份原子恢复旧凭据并复核。
                        let restored = write_file_atomically(&credential_path, &backup).is_ok()
                            && fs::read(&credential_path)
                                .map(|reloaded| reloaded == backup)
                                .unwrap_or(false);
                        if !restored {
                            let _ = write_manifest(
                                &manifest_path,
                                &manifest.next("manual_recovery_required"),
                            );
                            return Err(CheckinCredentialError::RecoveryRequired);
                        }
                        write_manifest(&manifest_path, &manifest.next("restored_verified"))?;
                    } else {
                        return Err(CheckinCredentialError::RecoveryRequired);
                    }
                }
                // 人工恢复现场只能由用户处置；恢复入口保持只读拒绝。
                _ => return Err(CheckinCredentialError::RecoveryRequired),
            }
        }
        Ok(())
    }

    /// 写回轮换后的凭据，固定顺序：
    /// 绑定校验 -> 旧凭据备份 -> 临时文件 -> 解密/绑定校验 -> 原子替换 ->
    /// 重新读取验证；每一步失败都保留旧凭据并收口 journal。
    ///
    /// 仅限本 crate 的续期编排（fixture 与真实 HTTP）复用；不对 crate 外公开，
    /// 避免绕过续期协议直接改写凭据。
    pub(crate) fn write_back(
        &self,
        binding: &CheckinProfileBinding,
        bundle: &CheckinCredentialBundle,
    ) -> Result<String, CheckinCredentialError> {
        // 写回前的绑定校验：账号 ID、设备 ID、设备公钥与 Profile 期望
        // 不一致时零回写，不创建 journal，也不产生备份。
        if !bundle_matches_binding(bundle, binding) {
            return Err(CheckinCredentialError::BindingMismatch);
        }
        validate_bundle(bundle)?;

        let credential_path = self.credential_path(&binding.profile_id)?;
        let original = read_credential_payload(&credential_path)?;
        let payload = encode_bundle_payload(bundle)?;
        let before_sha256 = sha256_hex(&original);
        let target_sha256 = sha256_hex(&payload);

        let operation_id = format!(
            "checkin-renewal-{}-{}",
            std::process::id(),
            now_unix_nanos()
        );
        let journal_dir = self.root.join("renewals").join(&operation_id);
        prepare_directory(&journal_dir)?;
        let backup_path = journal_dir.join("bundle.before");
        let manifest_path = journal_dir.join("manifest.json");
        let temporary_name = format!(
            ".{}.tmp-{}-{}",
            credential_path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("credential"),
            now_unix_nanos(),
            std::process::id()
        );
        let temporary_path = self.root.join(&temporary_name);

        let manifest = RenewalManifest {
            format_version: BUNDLE_FORMAT_VERSION,
            operation_id: operation_id.clone(),
            operation: JOURNAL_OPERATION.to_string(),
            sequence: 1,
            state: "prepared".to_string(),
            profile_id: binding.profile_id.clone(),
            backup_file: format!("renewals/{operation_id}/bundle.before"),
            temporary_file: temporary_name.clone(),
            before_sha256,
            target_sha256,
            created_at_unix_seconds: now_unix_seconds(),
        };

        // 1) 旧凭据备份：保留原样密文字节；失败证据不删除。
        if let Err(error) = write_file_create_new(&backup_path, &original)
            .and_then(|_| write_manifest(&manifest_path, &manifest))
        {
            return Err(self.abort_write_back(&temporary_path, &manifest_path, &manifest, error));
        }

        // 2) 临时文件写入新密文；3) 替换前先解密并校验绑定与完整性。
        let temporary_verified = (|| {
            write_file_create_new(&temporary_path, &payload)?;
            let written = fs::read(&temporary_path).map_err(|_| CheckinCredentialError::Invalid)?;
            let decoded = decode_bundle_payload(&written)?;
            if !bundle_matches_binding(&decoded, binding) {
                return Err(CheckinCredentialError::BindingMismatch);
            }
            Ok(())
        })();
        if let Err(error) = temporary_verified {
            return Err(self.abort_write_back(&temporary_path, &manifest_path, &manifest, error));
        }

        // 4) 原子替换当前凭据；失败时 atomic_publish 已清理临时文件。
        if publish_replacing(&temporary_path, &credential_path).is_err() {
            return Err(self.abort_write_back(
                &temporary_path,
                &manifest_path,
                &manifest,
                CheckinCredentialError::Invalid,
            ));
        }

        let replaced = manifest.next("replaced");
        if let Err(error) = write_manifest(&manifest_path, &replaced) {
            return Err(self.restore_after_replaced(
                &credential_path,
                &backup_path,
                &manifest_path,
                &replaced,
                &original,
                error,
            ));
        }

        // 5) 重新读取验证：解密后必须仍匹配绑定并等于刚写入的凭据。
        let reloaded = self.load(binding).and_then(|decoded| {
            if same_credentials(&decoded, bundle) {
                Ok(())
            } else {
                Err(CheckinCredentialError::Invalid)
            }
        });
        if let Err(error) = reloaded {
            return Err(self.restore_after_replaced(
                &credential_path,
                &backup_path,
                &manifest_path,
                &replaced,
                &original,
                error,
            ));
        }

        let verified = replaced.next("verified");
        if let Err(error) = write_manifest(&manifest_path, &verified) {
            return Err(self.restore_after_replaced(
                &credential_path,
                &backup_path,
                &manifest_path,
                &verified,
                &original,
                error,
            ));
        }
        Ok(operation_id)
    }

    /// 替换尚未发生时收口：清理本次临时文件，journal 标记 aborted，
    /// 旧凭据原样保留。
    fn abort_write_back(
        &self,
        temporary_path: &Path,
        manifest_path: &Path,
        manifest: &RenewalManifest,
        cause: CheckinCredentialError,
    ) -> CheckinCredentialError {
        let _ = fs::remove_file(temporary_path);
        if write_manifest(manifest_path, &manifest.next("aborted")).is_err() {
            return CheckinCredentialError::RecoveryRequired;
        }
        cause
    }

    /// 替换已发生后收口：优先从备份原子恢复旧凭据并复核；恢复失败时
    /// 保留现场等待人工处理。
    fn restore_after_replaced(
        &self,
        credential_path: &Path,
        backup_path: &Path,
        manifest_path: &Path,
        manifest: &RenewalManifest,
        original: &[u8],
        cause: CheckinCredentialError,
    ) -> CheckinCredentialError {
        let backup_intact = fs::read(backup_path)
            .map(|backup| backup == original)
            .unwrap_or(false);
        let restored = backup_intact
            && write_file_atomically(credential_path, original).is_ok()
            && fs::read(credential_path)
                .map(|current| current == original)
                .unwrap_or(false);
        let next_state = if restored {
            "restored"
        } else {
            "manual_recovery_required"
        };
        if write_manifest(manifest_path, &manifest.next(next_state)).is_err() {
            return CheckinCredentialError::RecoveryRequired;
        }
        if restored {
            return cause;
        }
        CheckinCredentialError::RecoveryRequired
    }

    fn credential_path(&self, profile_id: &str) -> Result<PathBuf, CheckinCredentialError> {
        if profile_id.is_empty() || profile_id.len() > MAX_PROFILE_ID_BYTES {
            return Err(CheckinCredentialError::Invalid);
        }
        let mut hasher = Sha256::new();
        hasher.update(profile_id.as_bytes());
        Ok(self
            .root
            .join(format!("{}.checkin", hex::encode(hasher.finalize()))))
    }

    /// 删除指定账号的加密凭据包文件（账号删除流程）。
    /// 文件不存在视为已删除（幂等）；其余 IO 失败原样报错。
    pub fn remove(&self, profile_id: &str) -> Result<(), CheckinCredentialError> {
        let path = self.credential_path(profile_id)?;
        match std::fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(_) => Err(CheckinCredentialError::Invalid),
        }
    }
}

/// 无进程续期 fixture 编排：读取 -> 设备签名 -> `ExchangeToken` ->
/// `GetUserInfo` 身份校验 -> 安全写回。
pub struct FixtureRenewalService<'a> {
    store: &'a CheckinCredentialStore,
    exchange: &'a FixtureTokenEndpoint,
    user_info: &'a FixtureUserInfoEndpoint,
}

impl<'a> FixtureRenewalService<'a> {
    pub fn new(
        store: &'a CheckinCredentialStore,
        exchange: &'a FixtureTokenEndpoint,
        user_info: &'a FixtureUserInfoEndpoint,
    ) -> Self {
        Self {
            store,
            exchange,
            user_info,
        }
    }

    /// 执行一次无进程续期。时间戳与 nonce 由调用方提供，便于测试复现。
    pub fn renew(
        &self,
        binding: &CheckinProfileBinding,
        timestamp_unix_seconds: u64,
        nonce: &str,
    ) -> Result<RenewalReceipt, CheckinCredentialError> {
        // 中断现场未收口前禁止新的写回。
        if self.store.has_pending_renewal()? {
            return Err(CheckinCredentialError::RecoveryRequired);
        }
        let bundle = self.store.load(binding)?;
        let input = device_proof_signing_input(
            EXCHANGE_METHOD,
            EXCHANGE_PATH,
            &bundle.client_id,
            &bundle.refresh_token,
            timestamp_unix_seconds,
            nonce,
        );
        let signature = sign_device_proof(&bundle.device_private_key, &input)?;
        let (new_access_token, new_refresh_token) = self.exchange.exchange_token(
            &binding.profile_id,
            &bundle.access_token,
            &bundle.client_id,
            &bundle.refresh_token,
            timestamp_unix_seconds,
            nonce,
            &signature,
        )?;
        // 续期成功后必须用新访问令牌做只读身份校验；账号 ID 不匹配时禁止回写。
        let reported_account = self
            .user_info
            .account_id_for(&new_access_token)
            .ok_or(CheckinCredentialError::AuthMismatch)?;
        if reported_account != binding.account_id {
            return Err(CheckinCredentialError::AuthMismatch);
        }
        let rotated = new_refresh_token.is_some();
        let mut updated = bundle;
        updated.access_token = new_access_token;
        // refresh token 轮换以服务端返回值为准；没有返回新值时保留旧值。
        if let Some(refresh_token) = new_refresh_token {
            updated.refresh_token = refresh_token;
        }
        let operation_id = self.store.write_back(binding, &updated)?;
        Ok(RenewalReceipt {
            profile_id: binding.profile_id.clone(),
            operation_id,
            refresh_token_rotated: rotated,
        })
    }
}

fn bundle_matches_binding(
    bundle: &CheckinCredentialBundle,
    binding: &CheckinProfileBinding,
) -> bool {
    bundle.profile_id == binding.profile_id
        && bundle.account_id == binding.account_id
        && bundle.device_id == binding.device_id
        && bundle.device_public_key == binding.device_public_key
}

/// 操作前阈值检查：访问令牌剩余 <7 天或 refresh token 剩余 <30 天时
/// 需要触发一次无进程续期。已过期的凭据同样返回 true。
pub fn needs_refresh(bundle: &CheckinCredentialBundle, now_unix_seconds: u64) -> bool {
    let access_remaining = bundle
        .access_token_expires_at_unix_seconds
        .saturating_sub(now_unix_seconds);
    let refresh_remaining = bundle
        .refresh_token_expires_at_unix_seconds
        .saturating_sub(now_unix_seconds);
    access_remaining < ACCESS_TOKEN_REFRESH_THRESHOLD_SECONDS
        || refresh_remaining < REFRESH_TOKEN_REFRESH_THRESHOLD_SECONDS
}

fn same_credentials(left: &CheckinCredentialBundle, right: &CheckinCredentialBundle) -> bool {
    left.profile_id == right.profile_id
        && left.account_id == right.account_id
        && left.device_id == right.device_id
        && left.machine_id == right.machine_id
        && left.device_public_key == right.device_public_key
        && left.device_private_key == right.device_private_key
        && left.access_token == right.access_token
        && left.refresh_token == right.refresh_token
        && left.client_id == right.client_id
        && left.access_token_expires_at_unix_seconds == right.access_token_expires_at_unix_seconds
        && left.refresh_token_expires_at_unix_seconds == right.refresh_token_expires_at_unix_seconds
        && left.mobile_full == right.mobile_full
}

fn validate_bundle(bundle: &CheckinCredentialBundle) -> Result<(), CheckinCredentialError> {
    validate_text_field(&bundle.profile_id, MAX_PROFILE_ID_BYTES)?;
    validate_text_field(&bundle.account_id, MAX_ID_FIELD_BYTES)?;
    validate_text_field(&bundle.device_id, MAX_ID_FIELD_BYTES)?;
    validate_text_field(&bundle.client_id, MAX_ID_FIELD_BYTES)?;
    validate_text_field(&bundle.access_token, MAX_TOKEN_FIELD_BYTES)?;
    validate_text_field(&bundle.refresh_token, MAX_TOKEN_FIELD_BYTES)?;
    // machine_id 允许为空（旧凭据包兼容），但不允许超长。
    if bundle.machine_id.len() > MAX_ID_FIELD_BYTES {
        return Err(CheckinCredentialError::Invalid);
    }
    // 补录手机号：格式校验在命令层（三层校验）；这里只做长度防御。
    if let Some(mobile) = &bundle.mobile_full {
        if mobile.len() > MAX_ID_FIELD_BYTES {
            return Err(CheckinCredentialError::Invalid);
        }
    }
    decode_key_field(&bundle.device_public_key)?;
    decode_key_field(&bundle.device_private_key)?;
    Ok(())
}

fn validate_binding(binding: &CheckinProfileBinding) -> Result<(), CheckinCredentialError> {
    validate_text_field(&binding.profile_id, MAX_PROFILE_ID_BYTES)?;
    validate_text_field(&binding.account_id, MAX_ID_FIELD_BYTES)?;
    validate_text_field(&binding.device_id, MAX_ID_FIELD_BYTES)?;
    decode_key_field(&binding.device_public_key)?;
    Ok(())
}

fn validate_text_field(value: &str, max_bytes: usize) -> Result<(), CheckinCredentialError> {
    if value.is_empty() || value.len() > max_bytes {
        return Err(CheckinCredentialError::Invalid);
    }
    Ok(())
}

/// 校验设备密钥字段为 EC P-256 PEM（私钥 PKCS#8 或公钥 SPKI）。
fn decode_key_field(value: &str) -> Result<(), CheckinCredentialError> {
    validate_text_field(value, MAX_DEVICE_KEY_PEM_BYTES)?;
    let is_private = PKey::private_key_from_pem(value.as_bytes())
        .map(|key| private_key_is_prime256v1(&key).is_ok())
        .unwrap_or(false);
    let is_public = PKey::public_key_from_pem(value.as_bytes())
        .map(|key| public_key_is_prime256v1(&key).is_ok())
        .unwrap_or(false);
    if is_private || is_public {
        Ok(())
    } else {
        Err(CheckinCredentialError::Invalid)
    }
}

/// 解析私钥 PEM（PKCS#8）；必须是 EC P-256。
fn parse_private_key_pem(value: &str) -> Result<PKey<Private>, CheckinCredentialError> {
    validate_text_field(value, MAX_DEVICE_KEY_PEM_BYTES)?;
    let key = PKey::private_key_from_pem(value.as_bytes())
        .map_err(|_| CheckinCredentialError::Invalid)?;
    private_key_is_prime256v1(&key)?;
    Ok(key)
}

/// 解析公钥 PEM（SPKI）；必须是 EC P-256。
fn parse_public_key_pem(value: &str) -> Result<PKey<Public>, CheckinCredentialError> {
    validate_text_field(value, MAX_DEVICE_KEY_PEM_BYTES)?;
    let key =
        PKey::public_key_from_pem(value.as_bytes()).map_err(|_| CheckinCredentialError::Invalid)?;
    public_key_is_prime256v1(&key)?;
    Ok(key)
}

/// 密钥必须是 EC 曲线 P-256（prime256v1/secp256r1），与真实客户端一致。
fn private_key_is_prime256v1(key: &PKey<Private>) -> Result<(), CheckinCredentialError> {
    if key.id() != Id::EC {
        return Err(CheckinCredentialError::Invalid);
    }
    let ec_key = key.ec_key().map_err(|_| CheckinCredentialError::Invalid)?;
    if ec_key.group().curve_name() != Some(Nid::X9_62_PRIME256V1) {
        return Err(CheckinCredentialError::Invalid);
    }
    Ok(())
}

fn public_key_is_prime256v1(key: &PKey<Public>) -> Result<(), CheckinCredentialError> {
    if key.id() != Id::EC {
        return Err(CheckinCredentialError::Invalid);
    }
    let ec_key = key.ec_key().map_err(|_| CheckinCredentialError::Invalid)?;
    if ec_key.group().curve_name() != Some(Nid::X9_62_PRIME256V1) {
        return Err(CheckinCredentialError::Invalid);
    }
    Ok(())
}

/// openssl 导出的 PEM 字节转为字符串；失败视为内部错误。
fn pem_to_string(pem: Result<Vec<u8>, ErrorStack>) -> Result<String, CheckinCredentialError> {
    let bytes = pem.map_err(|_| CheckinCredentialError::Invalid)?;
    String::from_utf8(bytes).map_err(|_| CheckinCredentialError::Invalid)
}

fn encode_bundle_payload(
    bundle: &CheckinCredentialBundle,
) -> Result<Vec<u8>, CheckinCredentialError> {
    validate_bundle(bundle)?;
    let serialized = SerializableBundle {
        format_version: BUNDLE_FORMAT_VERSION,
        profile_id: bundle.profile_id.clone(),
        account_id: bundle.account_id.clone(),
        device_id: bundle.device_id.clone(),
        machine_id: bundle.machine_id.clone(),
        device_public_key: bundle.device_public_key.clone(),
        device_private_key: bundle.device_private_key.clone(),
        access_token: bundle.access_token.clone(),
        refresh_token: bundle.refresh_token.clone(),
        client_id: bundle.client_id.clone(),
        access_token_expires_at_unix_seconds: bundle.access_token_expires_at_unix_seconds,
        refresh_token_expires_at_unix_seconds: bundle.refresh_token_expires_at_unix_seconds,
        mobile_full: bundle.mobile_full.clone(),
    };
    let plaintext = serde_json::to_vec(&serialized).map_err(|_| CheckinCredentialError::Invalid)?;
    let encrypted = protect_secret(&plaintext).map_err(map_key_wrapper_error)?;
    if encrypted.is_empty() || encrypted.len() as u64 > MAX_BUNDLE_BYTES {
        return Err(CheckinCredentialError::Invalid);
    }
    let mut payload = Vec::with_capacity(BUNDLE_MAGIC.len() + 8 + encrypted.len());
    payload.extend_from_slice(BUNDLE_MAGIC);
    payload.extend_from_slice(&BUNDLE_FORMAT_VERSION.to_le_bytes());
    payload.extend_from_slice(&(encrypted.len() as u32).to_le_bytes());
    payload.extend_from_slice(&encrypted);
    Ok(payload)
}

fn decode_bundle_payload(
    payload: &[u8],
) -> Result<CheckinCredentialBundle, CheckinCredentialError> {
    if payload.len() < BUNDLE_MAGIC.len() + 8 || &payload[..BUNDLE_MAGIC.len()] != BUNDLE_MAGIC {
        return Err(CheckinCredentialError::Invalid);
    }
    let version_offset = BUNDLE_MAGIC.len();
    let version = u32::from_le_bytes(
        payload[version_offset..version_offset + 4]
            .try_into()
            .map_err(|_| CheckinCredentialError::Invalid)?,
    );
    if version != BUNDLE_FORMAT_VERSION {
        return Err(CheckinCredentialError::Invalid);
    }
    let length_offset = version_offset + 4;
    let length = u32::from_le_bytes(
        payload[length_offset..length_offset + 4]
            .try_into()
            .map_err(|_| CheckinCredentialError::Invalid)?,
    ) as usize;
    let start = length_offset + 4;
    if length == 0 || length as u64 > MAX_BUNDLE_BYTES || payload.len() != start + length {
        return Err(CheckinCredentialError::Invalid);
    }
    let plaintext = unprotect_secret(&payload[start..]).map_err(map_key_wrapper_error)?;
    if plaintext.len() as u64 > MAX_BUNDLE_BYTES {
        return Err(CheckinCredentialError::Invalid);
    }
    let serialized: SerializableBundle =
        serde_json::from_slice(&plaintext).map_err(|_| CheckinCredentialError::Invalid)?;
    if serialized.format_version != BUNDLE_FORMAT_VERSION {
        return Err(CheckinCredentialError::Invalid);
    }
    let bundle = CheckinCredentialBundle {
        profile_id: serialized.profile_id,
        account_id: serialized.account_id,
        device_id: serialized.device_id,
        machine_id: serialized.machine_id,
        device_public_key: serialized.device_public_key,
        device_private_key: serialized.device_private_key,
        access_token: serialized.access_token,
        refresh_token: serialized.refresh_token,
        client_id: serialized.client_id,
        access_token_expires_at_unix_seconds: serialized.access_token_expires_at_unix_seconds,
        refresh_token_expires_at_unix_seconds: serialized.refresh_token_expires_at_unix_seconds,
        mobile_full: serialized.mobile_full,
    };
    validate_bundle(&bundle)?;
    Ok(bundle)
}

fn read_credential_payload(path: &Path) -> Result<Vec<u8>, CheckinCredentialError> {
    let parent = path.parent().ok_or(CheckinCredentialError::Invalid)?;
    reject_path_chain(parent, false)?;
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(CheckinCredentialError::Missing)
        }
        Err(_) => return Err(CheckinCredentialError::Invalid),
    };
    if is_reparse_or_symlink(&metadata) || !metadata.is_file() {
        return Err(CheckinCredentialError::Invalid);
    }
    if metadata.len() > MAX_BUNDLE_BYTES {
        return Err(CheckinCredentialError::Invalid);
    }
    read_file_exact(path, metadata.len(), MAX_BUNDLE_BYTES)
        .map_err(|_| CheckinCredentialError::Invalid)
}

fn write_file_create_new(path: &Path, bytes: &[u8]) -> Result<(), CheckinCredentialError> {
    if let Ok(metadata) = fs::symlink_metadata(path) {
        if is_reparse_or_symlink(&metadata) || metadata.is_file() || metadata.is_dir() {
            return Err(CheckinCredentialError::RecoveryRequired);
        }
    }
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|_| CheckinCredentialError::Invalid)?;
    file.write_all(bytes)
        .map_err(|_| CheckinCredentialError::RecoveryRequired)?;
    file.sync_all()
        .map_err(|_| CheckinCredentialError::RecoveryRequired)
}

fn write_file_atomically(destination: &Path, bytes: &[u8]) -> Result<(), CheckinCredentialError> {
    let parent = destination
        .parent()
        .ok_or(CheckinCredentialError::Invalid)?;
    reject_path_chain(parent, true)?;
    fs::create_dir_all(parent).map_err(|_| CheckinCredentialError::Unavailable)?;
    reject_path_chain(parent, false)?;
    if let Ok(metadata) = fs::symlink_metadata(destination) {
        if is_reparse_or_symlink(&metadata) || metadata.is_dir() {
            return Err(CheckinCredentialError::Invalid);
        }
    }
    let temporary = parent.join(format!(
        ".{}.tmp-{}-{}",
        destination
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("credential"),
        now_unix_nanos(),
        std::process::id()
    ));
    let result = write_file_create_new(&temporary, bytes).and_then(|_| {
        publish_replacing(&temporary, destination)
            .map_err(|_| CheckinCredentialError::RecoveryRequired)
    });
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

fn write_manifest(path: &Path, manifest: &RenewalManifest) -> Result<(), CheckinCredentialError> {
    validate_manifest_shape(manifest)?;
    let bytes = serde_json::to_vec_pretty(manifest)
        .map_err(|_| CheckinCredentialError::RecoveryRequired)?;
    write_file_atomically(path, &bytes)
}

fn read_manifest_if_present(
    path: &Path,
) -> Result<Option<RenewalManifest>, CheckinCredentialError> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(CheckinCredentialError::RecoveryRequired),
    };
    if is_reparse_or_symlink(&metadata) || !metadata.is_file() {
        return Err(CheckinCredentialError::RecoveryRequired);
    }
    let bytes = read_file_exact(path, metadata.len(), MAX_MANIFEST_BYTES)
        .map_err(|_| CheckinCredentialError::RecoveryRequired)?;
    let manifest: RenewalManifest =
        serde_json::from_slice(&bytes).map_err(|_| CheckinCredentialError::RecoveryRequired)?;
    validate_manifest_shape(&manifest)?;
    Ok(Some(manifest))
}

fn validate_manifest_shape(manifest: &RenewalManifest) -> Result<(), CheckinCredentialError> {
    if manifest.format_version != BUNDLE_FORMAT_VERSION
        || manifest.operation != JOURNAL_OPERATION
        || manifest.sequence == 0
        || !is_valid_state(&manifest.state)
        || !is_safe_operation_id(&manifest.operation_id)
    {
        return Err(CheckinCredentialError::RecoveryRequired);
    }
    validate_text_field(&manifest.profile_id, MAX_PROFILE_ID_BYTES)
        .map_err(|_| CheckinCredentialError::RecoveryRequired)?;
    if manifest.backup_file.is_empty()
        || manifest.temporary_file.is_empty()
        || manifest.before_sha256.is_empty()
        || manifest.target_sha256.is_empty()
    {
        return Err(CheckinCredentialError::RecoveryRequired);
    }
    Ok(())
}

fn is_safe_operation_id(operation_id: &str) -> bool {
    !operation_id.is_empty()
        && operation_id.len() <= MAX_ID_FIELD_BYTES
        && operation_id != "."
        && operation_id != ".."
        && !operation_id.contains('/')
        && !operation_id.contains('\\')
}

fn is_valid_state(state: &str) -> bool {
    matches!(
        state,
        "prepared"
            | "replaced"
            | "verified"
            | "aborted"
            | "restored"
            | "restored_verified"
            | "manual_recovery_required"
    )
}

fn is_pending_state(state: &str) -> bool {
    matches!(state, "prepared" | "replaced" | "manual_recovery_required")
}

fn prepare_directory(path: &Path) -> Result<(), CheckinCredentialError> {
    reject_path_chain(path, true)?;
    fs::create_dir_all(path).map_err(|_| CheckinCredentialError::RecoveryRequired)?;
    reject_path_chain(path, false)
}

fn read_file_exact(path: &Path, expected_len: u64, max_len: u64) -> std::io::Result<Vec<u8>> {
    let file = File::open(path)?;
    let mut bytes = Vec::with_capacity(expected_len as usize);
    file.take(max_len.saturating_add(1))
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 != expected_len || bytes.len() as u64 > max_len {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "文件长度变化",
        ));
    }
    Ok(bytes)
}

fn reject_path_chain(path: &Path, allow_missing: bool) -> Result<(), CheckinCredentialError> {
    let mut current = Some(path);
    while let Some(candidate) = current {
        match fs::symlink_metadata(candidate) {
            Ok(metadata) if is_reparse_or_symlink(&metadata) || !metadata.is_dir() => {
                return Err(CheckinCredentialError::Invalid)
            }
            Ok(_) => {}
            Err(error) if allow_missing && error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Err(CheckinCredentialError::Invalid)
            }
            Err(_) => return Err(CheckinCredentialError::Invalid),
        }
        let Some(parent) = candidate.parent() else {
            break;
        };
        if parent == candidate {
            break;
        }
        current = Some(parent);
    }
    Ok(())
}

fn is_reparse_or_symlink(metadata: &fs::Metadata) -> bool {
    if metadata.file_type().is_symlink() {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        return metadata.file_attributes() & 0x400 != 0;
    }
    #[cfg(not(windows))]
    {
        false
    }
}

fn map_key_wrapper_error(error: traesync_ports::KeyWrapperError) -> CheckinCredentialError {
    match error {
        traesync_ports::KeyWrapperError::Unavailable => CheckinCredentialError::Unavailable,
        traesync_ports::KeyWrapperError::Failed => CheckinCredentialError::Invalid,
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn now_unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

fn now_unix_nanos() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    // fixture 假材料：只用于构造测试场景，不代表真实凭据。
    const FIXTURE_ACCESS_TOKEN_A: &str = "fixture-access-token-a";
    const FIXTURE_REFRESH_TOKEN_A: &str = "fixture-refresh-token-a";
    const FIXTURE_ACCESS_TOKEN_B: &str = "fixture-access-token-b";
    const FIXTURE_REFRESH_TOKEN_B: &str = "fixture-refresh-token-b";
    const FIXTURE_ACCOUNT_A: &str = "fixture-account-a";
    const FIXTURE_DEVICE_A: &str = "fixture-device-a";
    const FIXTURE_CLIENT_ID: &str = "fixture-client-id";
    const FIXTURE_NONCE: &str = "fixture-nonce-1";

    fn fixture_binding(device_public_key: &str) -> CheckinProfileBinding {
        CheckinProfileBinding::new(
            "profile-a",
            FIXTURE_ACCOUNT_A,
            FIXTURE_DEVICE_A,
            device_public_key,
        )
    }

    fn fixture_bundle(
        device_public_key: &str,
        device_private_key: &str,
    ) -> CheckinCredentialBundle {
        CheckinCredentialBundle {
            profile_id: "profile-a".to_string(),
            account_id: FIXTURE_ACCOUNT_A.to_string(),
            device_id: FIXTURE_DEVICE_A.to_string(),
            machine_id: "machine-fixture-a".to_string(),
            device_public_key: device_public_key.to_string(),
            device_private_key: device_private_key.to_string(),
            access_token: FIXTURE_ACCESS_TOKEN_A.to_string(),
            refresh_token: FIXTURE_REFRESH_TOKEN_A.to_string(),
            client_id: FIXTURE_CLIENT_ID.to_string(),
            // 模拟真实签发模式：同一时刻（1_800_000_000）签发 14 天/180 天。
            access_token_expires_at_unix_seconds: 1_800_000_000 + ACCESS_TOKEN_LIFETIME_SECONDS,
            refresh_token_expires_at_unix_seconds: 1_800_000_000 + REFRESH_TOKEN_LIFETIME_SECONDS,
            mobile_full: None,
        }
    }

    fn registered_exchange(device_public_key: &str) -> FixtureTokenEndpoint {
        FixtureTokenEndpoint::new(
            BTreeMap::from([("profile-a".to_string(), device_public_key.to_string())]),
            FIXTURE_ACCESS_TOKEN_B,
            Some(FIXTURE_REFRESH_TOKEN_B.to_string()),
        )
    }

    fn matching_user_info() -> FixtureUserInfoEndpoint {
        FixtureUserInfoEndpoint::new(BTreeMap::from([(
            FIXTURE_ACCESS_TOKEN_B.to_string(),
            FIXTURE_ACCOUNT_A.to_string(),
        )]))
    }

    fn credential_path(store: &CheckinCredentialStore) -> PathBuf {
        store.credential_path("profile-a").unwrap()
    }

    fn assert_no_writeback(store: &CheckinCredentialStore, saved: &[u8]) {
        // 零回写：凭据字节不变，且没有产生任何 journal、备份或临时文件。
        assert_eq!(fs::read(credential_path(store)).unwrap(), saved);
        assert!(!store.root().join("renewals").exists());
    }

    /// 手工构造中断现场：备份旧密文并写入指定状态的 manifest。
    fn write_interrupted_journal(
        store: &CheckinCredentialStore,
        state: &str,
        target_payload: &[u8],
    ) -> PathBuf {
        let operation_id = "checkin-renewal-fixture-interrupted";
        let journal_dir = store.root().join("renewals").join(operation_id);
        fs::create_dir_all(&journal_dir).unwrap();
        let backup = fs::read(credential_path(store)).unwrap();
        fs::write(journal_dir.join("bundle.before"), &backup).unwrap();
        let manifest = serde_json::json!({
            "format_version": BUNDLE_FORMAT_VERSION,
            "operation_id": operation_id,
            "operation": JOURNAL_OPERATION,
            "sequence": if state == "prepared" { 1 } else { 2 },
            "state": state,
            "profile_id": "profile-a",
            "backup_file": format!("renewals/{operation_id}/bundle.before"),
            "temporary_file": ".fixture.tmp-0-0",
            "before_sha256": sha256_hex(&backup),
            "target_sha256": sha256_hex(target_payload),
            "created_at_unix_seconds": 1,
        });
        fs::write(journal_dir.join("manifest.json"), manifest.to_string()).unwrap();
        journal_dir
    }

    #[test]
    fn needs_refresh_triggers_below_thresholds() {
        let (private_pem, public_pem) = generate_device_keypair().unwrap();
        let bundle = fixture_bundle(&public_pem, &private_pem);
        let access_expire = bundle.access_token_expires_at_unix_seconds;
        let refresh_expire = bundle.refresh_token_expires_at_unix_seconds;

        // 两项剩余寿命都在阈值之上：不刷新。
        let now = access_expire - ACCESS_TOKEN_REFRESH_THRESHOLD_SECONDS;
        assert!(!needs_refresh(&bundle, now));
        // 访问令牌剩余刚好 7 天不触发；剩余 7 天差 1 秒（now 后移 1 秒）触发。
        assert!(!needs_refresh(&bundle, now));
        assert!(needs_refresh(
            &bundle,
            access_expire - ACCESS_TOKEN_REFRESH_THRESHOLD_SECONDS + 1
        ));
        // refresh token 剩余刚好 30 天不触发；差 1 秒触发。
        let refresh_boundary = refresh_expire - REFRESH_TOKEN_REFRESH_THRESHOLD_SECONDS;
        // 访问令牌剩余充足时，仅 refresh token 临近阈值触发刷新。
        let mut refresh_only = fixture_bundle(&public_pem, &private_pem);
        refresh_only.access_token_expires_at_unix_seconds =
            refresh_boundary.saturating_add(ACCESS_TOKEN_REFRESH_THRESHOLD_SECONDS + 100);
        assert!(!needs_refresh(&refresh_only, refresh_boundary));
        assert!(needs_refresh(&refresh_only, refresh_boundary + 1));
        // 已过期的凭据必须刷新。
        assert!(needs_refresh(&bundle, access_expire));
        assert!(needs_refresh(&bundle, refresh_expire + 1));
    }

    #[test]
    fn bundle_roundtrip_preserves_expiry_timestamps() {
        let (private_pem, public_pem) = generate_device_keypair().unwrap();
        let payload = encode_bundle_payload(&fixture_bundle(&public_pem, &private_pem)).unwrap();
        let decoded = decode_bundle_payload(&payload).unwrap();
        assert_eq!(
            decoded.access_token_expires_at_unix_seconds,
            1_800_000_000 + ACCESS_TOKEN_LIFETIME_SECONDS
        );
        assert_eq!(
            decoded.refresh_token_expires_at_unix_seconds,
            1_800_000_000 + REFRESH_TOKEN_LIFETIME_SECONDS
        );
    }

    #[test]
    fn device_proof_input_uses_fixed_field_order() {
        let input = device_proof_signing_input(
            "POST",
            "/trae/api/v3/oauth/ExchangeToken",
            "client-1",
            "refresh-1",
            1_700_000_000,
            "nonce-1",
        );
        let mut expected = Vec::new();
        for field in [
            "POST",
            "/trae/api/v3/oauth/ExchangeToken",
            "client-1",
            "refresh-1",
            "1700000000",
            "nonce-1",
        ] {
            expected.extend_from_slice(field.as_bytes());
            expected.push(b'\n');
        }
        assert_eq!(input, expected);
    }

    #[test]
    fn device_proof_signature_roundtrip_and_tamper_rejection() {
        let (private_pem, public_pem) = generate_device_keypair().unwrap();
        // 格式锚定：与真实客户端存储一致（PKCS#8 私钥 / SPKI 公钥 PEM）。
        assert!(private_pem.starts_with("-----BEGIN PRIVATE KEY-----"));
        assert!(public_pem.starts_with("-----BEGIN PUBLIC KEY-----"));
        let input = device_proof_signing_input("POST", "/path", "client", "refresh", 42, "nonce");
        let signature = sign_device_proof(&private_pem, &input).unwrap();
        // ECDSA 签名为 DER 编码：以 SEQUENCE (0x30) 标签开头。
        assert_eq!(signature[0], 0x30);

        assert!(verify_device_proof(&public_pem, &input, &signature).unwrap());
        let mut tampered_input = input.clone();
        tampered_input[0] ^= 0x01;
        assert!(!verify_device_proof(&public_pem, &tampered_input, &signature).unwrap());
        let mut tampered_signature = signature.clone();
        tampered_signature[0] ^= 0x01;
        assert!(!verify_device_proof(&public_pem, &input, &tampered_signature).unwrap());
        // 另一台设备的公钥不能通过验签。
        let (_other_private, other_public) = generate_device_keypair().unwrap();
        assert!(!verify_device_proof(&other_public, &input, &signature).unwrap());
    }

    /// 前序逆向实测样本（20260821，真实客户端 storage.json 解密所得）中的
    /// EC P-256 设备密钥对；不含 Token/refreshToken，仅用于验证 PEM 格式与
    /// 真实客户端互通（见交接文档 reference/credential-sample-20260821.json）。
    const SAMPLE_PRIVATE_KEY_PEM: &str = "-----BEGIN PRIVATE KEY-----\nMIGHAgEAMBMGByqGSM49AgEGCCqGSM49AwEHBG0wawIBAQQgh9I80+POJPArEG+7\nk8xmXnoUiVaRZqI5coYBk4U4/jGhRANCAAS6wqkc0Fyn7UrXlgFBqjEkikYwtwtL\nsWFMpp5sHHCQJrWHGStpEqiVkSWxi0IlAc8BX5afW2pp15Zk4+1zwYMv\n-----END PRIVATE KEY-----\n";
    const SAMPLE_PUBLIC_KEY_PEM: &str = "-----BEGIN PUBLIC KEY-----\nMFkwEwYHKoZIzj0CAQYIKoZIzj0DAQcDQgAEusKpHNBcp+1K15YBQaoxJIpGMLcL\nS7FhTKaebBxwkCa1hxkraRKolZElsYtCJQHPAV+Wn1tqadeWZOPtc8GDLw==\n-----END PUBLIC KEY-----\n";

    #[test]
    fn device_proof_accepts_real_client_key_format() {
        // 真实客户端格式的密钥对必须能直接签名/验签，证明与实测协议互通。
        let input = device_proof_signing_input("POST", "/path", "client", "refresh", 42, "nonce");
        let signature = sign_device_proof(SAMPLE_PRIVATE_KEY_PEM, &input).unwrap();
        assert!(verify_device_proof(SAMPLE_PUBLIC_KEY_PEM, &input, &signature).unwrap());
        let mut tampered = input.clone();
        tampered[0] ^= 0x01;
        assert!(!verify_device_proof(SAMPLE_PUBLIC_KEY_PEM, &tampered, &signature).unwrap());
    }

    #[cfg(windows)]
    #[test]
    fn bundle_roundtrip_encrypts_without_plaintext_on_disk() {
        let root = tempdir().unwrap();
        let store = CheckinCredentialStore::new(root.path().join("profiles"));
        let (private_pem, public_pem) = generate_device_keypair().unwrap();
        let binding = fixture_binding(&public_pem);
        let bundle = fixture_bundle(&public_pem, &private_pem);

        store.save(&bundle).unwrap();
        let ciphertext = fs::read(credential_path(&store)).unwrap();
        assert!(ciphertext.starts_with(BUNDLE_MAGIC));
        for secret in [
            &bundle.access_token,
            &bundle.refresh_token,
            &bundle.device_private_key,
        ] {
            assert!(!ciphertext
                .windows(secret.len())
                .any(|window| window == secret.as_bytes()));
        }
        let reloaded = store.load(&binding).unwrap();
        assert!(same_credentials(&reloaded, &bundle));
        assert!(!store.has_pending_renewal().unwrap());
    }

    #[cfg(windows)]
    #[test]
    fn load_rejects_wrong_binding_without_any_write() {
        let root = tempdir().unwrap();
        let store = CheckinCredentialStore::new(root.path().join("profiles"));
        let (private_pem, public_pem) = generate_device_keypair().unwrap();
        store
            .save(&fixture_bundle(&public_pem, &private_pem))
            .unwrap();
        let saved = fs::read(credential_path(&store)).unwrap();

        let wrong_account = CheckinProfileBinding::new(
            "profile-a",
            "fixture-account-other",
            FIXTURE_DEVICE_A,
            &public_pem,
        );
        let wrong_device = CheckinProfileBinding::new(
            "profile-a",
            FIXTURE_ACCOUNT_A,
            "fixture-device-other",
            &public_pem,
        );
        let (_other_private, other_public) = generate_device_keypair().unwrap();
        let wrong_public_key = CheckinProfileBinding::new(
            "profile-a",
            FIXTURE_ACCOUNT_A,
            FIXTURE_DEVICE_A,
            &other_public,
        );

        for binding in [&wrong_account, &wrong_device, &wrong_public_key] {
            // Bundle 不实现 Debug/PartialEq（避免凭据值进入失败输出），
            // 因此只对错误变体做断言。
            assert_eq!(
                store.load(binding).err(),
                Some(CheckinCredentialError::BindingMismatch)
            );
        }
        assert_no_writeback(&store, &saved);
    }

    #[cfg(windows)]
    #[test]
    fn write_back_rejects_binding_mismatch_before_creating_journal() {
        let root = tempdir().unwrap();
        let store = CheckinCredentialStore::new(root.path().join("profiles"));
        let (private_pem, public_pem) = generate_device_keypair().unwrap();
        store
            .save(&fixture_bundle(&public_pem, &private_pem))
            .unwrap();
        let saved = fs::read(credential_path(&store)).unwrap();
        let (_other_private, other_public) = generate_device_keypair().unwrap();
        // 写回的新凭据绑定 profile-a 的公钥，但期望绑定是另一台设备。
        let binding = fixture_binding(&other_public);
        let updated = fixture_bundle(&public_pem, &private_pem);

        assert_eq!(
            store.write_back(&binding, &updated),
            Err(CheckinCredentialError::BindingMismatch)
        );
        assert_no_writeback(&store, &saved);
    }

    #[cfg(windows)]
    #[test]
    fn renew_rotates_refresh_token_and_preserves_old_credential_backup() {
        let root = tempdir().unwrap();
        let store = CheckinCredentialStore::new(root.path().join("profiles"));
        let (private_pem, public_pem) = generate_device_keypair().unwrap();
        let binding = fixture_binding(&public_pem);
        store
            .save(&fixture_bundle(&public_pem, &private_pem))
            .unwrap();
        let original_ciphertext = fs::read(credential_path(&store)).unwrap();
        let exchange = registered_exchange(&public_pem);
        let user_info = matching_user_info();
        let service = FixtureRenewalService::new(&store, &exchange, &user_info);

        let receipt = service
            .renew(&binding, 1_800_000_000, FIXTURE_NONCE)
            .unwrap();
        assert!(receipt.refresh_token_rotated);

        // 轮换后的凭据生效；用布尔断言避免把凭据值打进失败输出。
        let reloaded = store.load(&binding).unwrap();
        assert!(reloaded.access_token == FIXTURE_ACCESS_TOKEN_B);
        assert!(reloaded.refresh_token == FIXTURE_REFRESH_TOKEN_B);

        // 旧凭据保留：备份是原样密文字节，且磁盘上没有明文。
        let journal_dir = store.root().join("renewals").join(&receipt.operation_id);
        let backup = fs::read(journal_dir.join("bundle.before")).unwrap();
        assert_eq!(backup, original_ciphertext);
        assert!(!backup
            .windows(FIXTURE_REFRESH_TOKEN_A.len())
            .any(|window| window == FIXTURE_REFRESH_TOKEN_A.as_bytes()));

        // journal 到达 verified 终态；记录的临时文件已被原子替换消化。
        let manifest: serde_json::Value =
            serde_json::from_slice(&fs::read(journal_dir.join("manifest.json")).unwrap()).unwrap();
        assert_eq!(manifest["state"], "verified");
        assert_eq!(manifest["sequence"], 3);
        let temporary = manifest["temporary_file"].as_str().unwrap();
        assert!(temporary.contains(".tmp-"));
        assert!(!store.root().join(temporary).exists());

        // 凭据文件已是新密文，同样不含明文。
        let new_ciphertext = fs::read(credential_path(&store)).unwrap();
        assert_ne!(new_ciphertext, original_ciphertext);
        assert!(!new_ciphertext
            .windows(FIXTURE_REFRESH_TOKEN_B.len())
            .any(|window| window == FIXTURE_REFRESH_TOKEN_B.as_bytes()));

        // fixture 端点收到的签名输入字段与指纹正确（不暴露明文）。
        let request = exchange.last_request().unwrap();
        assert_eq!(request.method, "POST");
        assert_eq!(request.path, "/trae/api/v3/oauth/ExchangeToken");
        assert_eq!(request.client_id, FIXTURE_CLIENT_ID);
        assert_eq!(request.timestamp_unix_seconds, 1_800_000_000);
        assert_eq!(request.nonce, FIXTURE_NONCE);
        assert!(request.signature_verified);
        assert_eq!(
            request.refresh_token_sha256,
            sha256_hex(FIXTURE_REFRESH_TOKEN_A.as_bytes())
        );
        assert_eq!(
            request.access_token_sha256,
            sha256_hex(FIXTURE_ACCESS_TOKEN_A.as_bytes())
        );
        assert!(!store.has_pending_renewal().unwrap());
    }

    #[cfg(windows)]
    #[test]
    fn renew_without_rotation_keeps_old_refresh_token() {
        let root = tempdir().unwrap();
        let store = CheckinCredentialStore::new(root.path().join("profiles"));
        let (private_pem, public_pem) = generate_device_keypair().unwrap();
        let binding = fixture_binding(&public_pem);
        store
            .save(&fixture_bundle(&public_pem, &private_pem))
            .unwrap();
        // 服务端没有返回新 refresh token：必须保留旧值。
        let exchange = FixtureTokenEndpoint::new(
            BTreeMap::from([("profile-a".to_string(), public_pem.clone())]),
            FIXTURE_ACCESS_TOKEN_B,
            None,
        );
        let user_info = matching_user_info();
        let service = FixtureRenewalService::new(&store, &exchange, &user_info);

        let receipt = service
            .renew(&binding, 1_800_000_000, FIXTURE_NONCE)
            .unwrap();
        assert!(!receipt.refresh_token_rotated);
        let reloaded = store.load(&binding).unwrap();
        assert!(reloaded.access_token == FIXTURE_ACCESS_TOKEN_B);
        assert!(reloaded.refresh_token == FIXTURE_REFRESH_TOKEN_A);
    }

    #[cfg(windows)]
    #[test]
    fn renew_rejects_unverified_device_proof_without_writeback() {
        let root = tempdir().unwrap();
        let store = CheckinCredentialStore::new(root.path().join("profiles"));
        let (private_pem, public_pem) = generate_device_keypair().unwrap();
        store
            .save(&fixture_bundle(&public_pem, &private_pem))
            .unwrap();
        let saved = fs::read(credential_path(&store)).unwrap();
        // 服务端登记的是另一台设备的公钥：签名无法通过设备绑定校验。
        let (_unrelated_private, unrelated_public) = generate_device_keypair().unwrap();
        let exchange = FixtureTokenEndpoint::new(
            BTreeMap::from([("profile-a".to_string(), unrelated_public)]),
            FIXTURE_ACCESS_TOKEN_B,
            Some(FIXTURE_REFRESH_TOKEN_B.to_string()),
        );
        let user_info = matching_user_info();
        let service = FixtureRenewalService::new(&store, &exchange, &user_info);

        assert_eq!(
            service.renew(&fixture_binding(&public_pem), 1_800_000_000, FIXTURE_NONCE),
            Err(CheckinCredentialError::CredentialRefreshFailed)
        );
        assert!(!exchange.last_request().unwrap().signature_verified);
        assert_no_writeback(&store, &saved);
    }

    #[cfg(windows)]
    #[test]
    fn renew_rejects_user_info_mismatch_without_writeback() {
        let root = tempdir().unwrap();
        let store = CheckinCredentialStore::new(root.path().join("profiles"));
        let (private_pem, public_pem) = generate_device_keypair().unwrap();
        store
            .save(&fixture_bundle(&public_pem, &private_pem))
            .unwrap();
        let saved = fs::read(credential_path(&store)).unwrap();
        let exchange = registered_exchange(&public_pem);
        // 新访问令牌解析出的账号不是 Profile 绑定的账号。
        let mismatched_user_info = FixtureUserInfoEndpoint::new(BTreeMap::from([(
            FIXTURE_ACCESS_TOKEN_B.to_string(),
            "fixture-account-other".to_string(),
        )]));
        let service = FixtureRenewalService::new(&store, &exchange, &mismatched_user_info);

        assert_eq!(
            service.renew(&fixture_binding(&public_pem), 1_800_000_000, FIXTURE_NONCE),
            Err(CheckinCredentialError::AuthMismatch)
        );
        assert_no_writeback(&store, &saved);
    }

    #[cfg(windows)]
    #[test]
    fn pending_journal_blocks_renew_and_preserves_credential() {
        let root = tempdir().unwrap();
        let store = CheckinCredentialStore::new(root.path().join("profiles"));
        let (private_pem, public_pem) = generate_device_keypair().unwrap();
        store
            .save(&fixture_bundle(&public_pem, &private_pem))
            .unwrap();
        let saved = fs::read(credential_path(&store)).unwrap();
        write_interrupted_journal(&store, "prepared", b"fixture-target");
        let exchange = registered_exchange(&public_pem);
        let user_info = matching_user_info();
        let service = FixtureRenewalService::new(&store, &exchange, &user_info);

        assert_eq!(
            service.renew(&fixture_binding(&public_pem), 1_800_000_000, FIXTURE_NONCE),
            Err(CheckinCredentialError::RecoveryRequired)
        );
        // 阻断不改动现场：凭据与 journal 都保持原样，等待显式恢复。
        assert_eq!(fs::read(credential_path(&store)).unwrap(), saved);
        assert!(store.has_pending_renewal().unwrap());
    }

    #[cfg(windows)]
    #[test]
    fn recovery_restores_old_credential_after_interrupted_replace() {
        let root = tempdir().unwrap();
        let store = CheckinCredentialStore::new(root.path().join("profiles"));
        let (private_pem, public_pem) = generate_device_keypair().unwrap();
        let binding = fixture_binding(&public_pem);
        store
            .save(&fixture_bundle(&public_pem, &private_pem))
            .unwrap();
        let old_ciphertext = fs::read(credential_path(&store)).unwrap();

        // 模拟"原子替换已完成、journal 停在 replaced"的崩溃现场：
        // 新密文已落盘，备份保留旧密文，manifest 未收口。
        let interrupted = CheckinCredentialBundle {
            access_token: FIXTURE_ACCESS_TOKEN_B.to_string(),
            refresh_token: FIXTURE_REFRESH_TOKEN_B.to_string(),
            ..fixture_bundle(&public_pem, &private_pem)
        };
        let new_payload = encode_bundle_payload(&interrupted).unwrap();
        // 真实崩溃现场的顺序：先备份旧密文，再发生原子替换落盘新密文。
        let journal_dir = write_interrupted_journal(&store, "replaced", &new_payload);
        fs::write(credential_path(&store), &new_payload).unwrap();
        assert!(store.has_pending_renewal().unwrap());

        store.recover_interrupted_renewal().unwrap();
        // 旧凭据已恢复，备份保留为失败证据，journal 收口。
        assert_eq!(fs::read(credential_path(&store)).unwrap(), old_ciphertext);
        let manifest: serde_json::Value =
            serde_json::from_slice(&fs::read(journal_dir.join("manifest.json")).unwrap()).unwrap();
        assert_eq!(manifest["state"], "restored_verified");
        assert!(journal_dir.join("bundle.before").exists());
        assert!(!store.has_pending_renewal().unwrap());

        // 恢复后可以再次成功续期。
        let exchange = registered_exchange(&public_pem);
        let user_info = matching_user_info();
        let service = FixtureRenewalService::new(&store, &exchange, &user_info);
        let receipt = service
            .renew(&binding, 1_800_000_001, "fixture-nonce-2")
            .unwrap();
        assert!(receipt.refresh_token_rotated);
        let reloaded = store.load(&binding).unwrap();
        assert!(reloaded.access_token == FIXTURE_ACCESS_TOKEN_B);
        assert!(reloaded.refresh_token == FIXTURE_REFRESH_TOKEN_B);
    }

    #[cfg(windows)]
    #[test]
    fn recovery_of_prepared_journal_keeps_credential_and_closes_journal() {
        let root = tempdir().unwrap();
        let store = CheckinCredentialStore::new(root.path().join("profiles"));
        let (private_pem, public_pem) = generate_device_keypair().unwrap();
        store
            .save(&fixture_bundle(&public_pem, &private_pem))
            .unwrap();
        let saved = fs::read(credential_path(&store)).unwrap();
        let journal_dir = write_interrupted_journal(&store, "prepared", b"fixture-target");
        assert!(store.has_pending_renewal().unwrap());

        store.recover_interrupted_renewal().unwrap();
        // 替换从未发生：凭据原样保留，journal 收口为 aborted。
        assert_eq!(fs::read(credential_path(&store)).unwrap(), saved);
        let manifest: serde_json::Value =
            serde_json::from_slice(&fs::read(journal_dir.join("manifest.json")).unwrap()).unwrap();
        assert_eq!(manifest["state"], "aborted");
        assert!(!store.has_pending_renewal().unwrap());
    }

    #[cfg(windows)]
    #[test]
    fn corrupted_vault_fails_closed_without_any_write() {
        let root = tempdir().unwrap();
        let store = CheckinCredentialStore::new(root.path().join("profiles"));
        let (private_pem, public_pem) = generate_device_keypair().unwrap();
        store
            .save(&fixture_bundle(&public_pem, &private_pem))
            .unwrap();
        fs::write(credential_path(&store), b"corrupted-payload").unwrap();
        let corrupted = fs::read(credential_path(&store)).unwrap();
        let exchange = registered_exchange(&public_pem);
        let user_info = matching_user_info();
        let service = FixtureRenewalService::new(&store, &exchange, &user_info);

        assert_eq!(
            service.renew(&fixture_binding(&public_pem), 1_800_000_000, FIXTURE_NONCE),
            Err(CheckinCredentialError::Invalid)
        );
        // 解密失败零回写：损坏现场原样保留，不产生 journal。
        assert_eq!(fs::read(credential_path(&store)).unwrap(), corrupted);
        assert!(!store.root().join("renewals").exists());
    }

    #[cfg(not(windows))]
    #[test]
    fn non_windows_save_fails_closed() {
        let root = tempdir().unwrap();
        let store = CheckinCredentialStore::new(root.path().join("profiles"));
        // EC 密钥对生成本身不依赖 DPAPI，可跨平台准备合法 PEM；
        // 失败必须来自凭据包加密（非 Windows 无 DPAPI）。
        let (private_pem, public_pem) = generate_device_keypair().unwrap();
        let bundle = fixture_bundle(&public_pem, &private_pem);
        assert_eq!(
            store.save(&bundle),
            Err(CheckinCredentialError::Unavailable)
        );
        assert!(!store.root().exists());
    }
}
