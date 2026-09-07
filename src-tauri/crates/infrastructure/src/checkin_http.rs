//! T17/MVP 真实 HTTP transport：TRAE CN 签到与 OAuth 端点直连（SOLO 形态，ADR-0019 v6）。
//!
//! 协议事实来源（2026-08-20/21 实测逆向，见 ADR-0019 与 `.scratch/checkin-http/` 证据）：
//! - 签到：`POST /trae/api/v2/ug/checkin_credits/{status,claim}`，
//!   头 `Authorization: Cloud-IDE-JWT <token>` 与 `x-device-id`；
//!   HTTP 200 不代表成功，业务码在响应体 `code`（9074=陌生设备门禁、
//!   9095=设备日配额）。
//! - 遥测头（2026-09-02 P1-4 决议 6）：status/claim 请求附完整客户端头集合
//!   （UA + 固定标识 + 账号派生 ID + 每请求刷新 ID），status 端点带头/裸头
//!   对比实测 PASS（报告 telemetry-headers-probe-20260902-234021.json：
//!   带头组与裸头基线均 200/code=0 且业务字段一致）。
//! - `ExchangeToken`：AuthCode 模式免签名（登录）；refresh 模式需 DeviceProof
//!   ECDSA-SHA256 签名（续期）。响应含毫秒过期时间与设备绑定状态。
//! - `GetUserInfo`：只读身份/资料查询。
//!
//! 网络错误映射为 `CheckinTransportError`，错误文本不含 Token 或响应正文。

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use sha2::{Digest, Sha256};

use base64::Engine;
use serde::Deserialize;
use traesync_domain::{
    CheckinClaimSnapshot, CheckinStatusSnapshot, EntitlementPackSnapshot,
    EntitlementUsageSnapshot,
};
use traesync_ports::{CheckinTransport, CheckinTransportError};

use crate::checkin_credential::{
    device_proof_signing_input, needs_refresh, sign_device_proof, CheckinCredentialBundle,
    CheckinCredentialError, CheckinCredentialStore, CheckinProfileBinding, RenewalReceipt,
};
use crate::checkin_login::decode_account_from_jwt;

const API_BASE: &str = "https://api.trae.cn";
const CHECKIN_STATUS_PATH: &str = "/trae/api/v2/ug/checkin_credits/status";
const CHECKIN_CLAIM_PATH: &str = "/trae/api/v2/ug/checkin_credits/claim";
const ENT_USAGE_PATH: &str = "/trae/api/v2/pay/ide_user_ent_usage";
const EXCHANGE_TOKEN_PATH: &str = "/trae/api/v3/oauth/ExchangeToken";
const GET_USER_INFO_PATH: &str = "/cloudide/api/v3/trae/GetUserInfo";
const GET_PC_AUTH_CODE_PATH: &str = "/cloudide/api/v3/trae/oauth/GetPCAuthCode";
/// GetPCAuthCode 的固定回调地址：该接口不实际回调（仅参数校验），
/// 与真实客户端请求一致（r34/r36 探针实测，2026-08-25）。
const OAUTH_REDIRECT_URI: &str = "http://127.0.0.1:18963/authorize";
/// 授权页来源头（与真实客户端一致；2026-09-02 实测网关存在动态 IP 风控，
/// 请求过密时不带这三头会被 403 拦截，带上可通过）。
const OAUTH_USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 Chrome/140.0.0.0 Safari/537.36";
/// 与真实客户端一致的 IDE 版本号（product.json `appVersion`；
/// `version`/`vscodeVersion` 1.107.x 是 VS Code 基座版本，不用于协议字段）。
/// GetUserInfo 等只读资料查询沿用该值（生产实测可行，ADR-0019 v6）。
pub const TRAE_IDE_VERSION: &str = "3.3.74";
/// TRAE SOLO CN 客户端 ID：v6 起全链路唯一客户端形态（ADR-0019 v6）。
/// 原 Work 通道（ono9krqynydwx5）设备配额已耗尽且新设备首签被 9074
/// 稳定拒绝，已于 2026-09-02 删除。
pub const TRAE_SOLO_CLIENT_ID: &str = "en1oxy7wnw8j9n";
/// TRAE SOLO CN 客户端版本（r34 探针实测值）。
pub const TRAE_SOLO_IDE_VERSION: &str = "0.1.54";
const REQUEST_TIMEOUT_SECONDS: u64 = 15;

/// 真实 HTTP 调用错误；不携带 Token、refresh token 或响应正文。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckinHttpError {
    /// 网络层失败（连接/超时/DNS/TLS）。
    Network,
    /// 服务端返回非 200。
    Http(u16),
    /// 业务码拒绝（如 9095 设备配额、20324 refresh 失效）。
    Business(i64),
    /// 响应结构不符合协议预期。
    Protocol,
}

impl CheckinHttpError {
    /// 映射到签到 transport 错误分类。
    pub fn to_transport_error(&self) -> CheckinTransportError {
        match self {
            Self::Network => CheckinTransportError::Network,
            Self::Business(code) => CheckinTransportError::Business(*code),
            Self::Http(_) | Self::Protocol => CheckinTransportError::Protocol,
        }
    }
}

/// 真实签到 transport：从 DPAPI 加密凭据包读取 Token 后直连 TRAE。
///
/// 每个账号绑定独立虚拟设备（device_id + EC P-256 密钥对），签到配额按
/// `x-device-id` 独立判定（05 号文档实证），互不影响。
/// 自持有凭据仓库（路径克隆）：设备降级改绑后需重建 transport，拥有式
/// 结构可安全装箱为 `Box<dyn CheckinTransport>`。
pub struct RealCheckinTransport {
    store: CheckinCredentialStore,
    bindings: BTreeMap<String, CheckinProfileBinding>,
    client: reqwest::blocking::Client,
}

impl RealCheckinTransport {
    /// `bindings` 为本次批量签到选中账号的凭据绑定集合。
    pub fn new(
        store: &CheckinCredentialStore,
        bindings: BTreeMap<String, CheckinProfileBinding>,
    ) -> Self {
        Self {
            store: store.clone(),
            bindings,
            client: http_client(),
        }
    }

    fn credential_context(
        &self,
        profile_id: &str,
    ) -> Result<(String, String), CheckinTransportError> {
        let binding = self
            .bindings
            .get(profile_id)
            .ok_or(CheckinTransportError::Runtime)?;
        let bundle = self
            .store
            .load(binding)
            .map_err(|_| CheckinTransportError::AuthMismatch)?;
        Ok((bundle.access_token, bundle.device_id))
    }

    fn status(&self, profile_id: &str) -> Result<CheckinStatusSnapshot, CheckinTransportError> {
        let (token, device_id) = self.credential_context(profile_id)?;
        let response = checkin_status_request(&self.client, &token, &device_id)
            .map_err(|error| error.to_transport_error())?;
        Ok(response)
    }

    fn claim(&self, profile_id: &str) -> Result<CheckinClaimSnapshot, CheckinTransportError> {
        let (token, device_id) = self.credential_context(profile_id)?;
        let response = checkin_claim_request(&self.client, &token, &device_id)
            .map_err(|error| error.to_transport_error())?;
        Ok(response)
    }

    fn entitlement_usage(
        &self,
        profile_id: &str,
    ) -> Result<EntitlementUsageSnapshot, CheckinTransportError> {
        let (token, device_id) = self.credential_context(profile_id)?;
        let snapshot = entitlement_usage_request(&self.client, &token, &device_id)
            .map_err(|error| error.to_transport_error())?;
        Ok(snapshot)
    }
}

impl CheckinTransport for RealCheckinTransport {
    fn status(&self, profile_id: &str) -> Result<CheckinStatusSnapshot, CheckinTransportError> {
        self.status(profile_id)
    }

    fn claim(&self, profile_id: &str) -> Result<CheckinClaimSnapshot, CheckinTransportError> {
        self.claim(profile_id)
    }

    fn entitlement_usage(
        &self,
        profile_id: &str,
    ) -> Result<EntitlementUsageSnapshot, CheckinTransportError> {
        self.entitlement_usage(profile_id)
    }
}

fn http_client() -> reqwest::blocking::Client {
    reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(REQUEST_TIMEOUT_SECONDS))
        .build()
        .expect("构造 HTTP 客户端失败")
}

// —— 遥测头集合（P1-4 决议 6，2026-09-02 实测后纳入）——
// 借鉴 trae-mate `build_headers` 的完整客户端形态；固定值中的版本号用本项目
// r34 探针实测值（TRAE_SOLO_IDE_VERSION）。status 端点带头/裸头对比实测：
// 2 账号 × 3 组全部 200/code=0，业务字段与基线一致（见模块头注释报告路径）。

/// VS Code 基座 UA 形态（vscodeVersion 1.107.x，见 TRAE_IDE_VERSION 注释）。
const TRAE_VSCODE_UA: &str = "VSCode 1.107.1 (TRAE SOLO CN)";
/// x-lscbd-aid 固定值（trae-mate 逆向值，全客户端一致）。
const TRAE_LSCBD_AID: &str = "787976";
/// 每请求刷新头熵源：原子递增计数器 + 纳秒时钟，经 SHA-256 展开。
static REQUEST_ID_COUNTER: AtomicU64 = AtomicU64::new(0);

/// 种子材料 → UUID v4 形状（版本位 4、变体位 10x，RFC 4122）。
fn derived_uuid_v4(seed_material: &[u8]) -> String {
    let digest = Sha256::digest(seed_material);
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex_string = hex::encode(bytes);
    format!(
        "{}-{}-{}-{}-{}",
        &hex_string[0..8],
        &hex_string[8..12],
        &hex_string[12..16],
        &hex_string[16..20],
        &hex_string[20..32]
    )
}

/// `x-market-user-id`：设备 ID 确定性派生（同设备稳定、跨设备互异）。
fn derived_market_user_id(device_id: &str) -> String {
    derived_uuid_v4(format!("market:{device_id}").as_bytes())
}

/// `vscode-sessionid`：设备 ID 确定性派生的 64 位 hex。
fn derived_session_id(device_id: &str) -> String {
    hex::encode(Sha256::digest(
        format!("session:{device_id}").as_bytes(),
    ))
}

/// 每请求刷新的 `x-request-id`（UUID v4 形状）。
fn fresh_request_id() -> String {
    let counter = REQUEST_ID_COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    derived_uuid_v4(format!("request:{nanos}:{counter}").as_bytes())
}

/// 每请求刷新的 `x-tt-trace-id`（W3C traceparent：`00-<32hex>-<16hex>-01`）。
fn fresh_traceparent() -> String {
    let counter = REQUEST_ID_COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let digest = Sha256::digest(format!("trace:{nanos}:{counter}").as_bytes());
    format!(
        "00-{}-{}-01",
        hex::encode(&digest[..16]),
        hex::encode(&digest[16..24])
    )
}

/// 给 status/claim 请求附完整遥测头集合（固定头 + 账号派生头 + 每请求刷新头）。
fn apply_telemetry_headers(
    request: reqwest::blocking::RequestBuilder,
    device_id: &str,
) -> reqwest::blocking::RequestBuilder {
    request
        .header("User-Agent", TRAE_VSCODE_UA)
        .header("x-lscbd-aid", TRAE_LSCBD_AID)
        .header("app-version", TRAE_SOLO_IDE_VERSION)
        .header("x-market-client-id", "VSCode 1.107.1")
        .header("x-user-region", "CN")
        .header("package-type", "stable_cn")
        .header("x-lgw-req-sdk-type", "3")
        .header("x-lscbd-platform", "windows")
        .header("x-market-user-id", derived_market_user_id(device_id))
        .header("vscode-sessionid", derived_session_id(device_id))
        .header("x-request-id", fresh_request_id())
        .header("x-tt-trace-id", fresh_traceparent())
}

/// 构造 TRAE 直连 HTTP 客户端（固定超时）；供登录等 crate 外编排复用，
/// 避免组合根自行拼装客户端配置。
pub fn trae_http_client() -> reqwest::blocking::Client {
    http_client()
}

#[derive(Debug, Deserialize)]
struct CheckinResponse {
    code: i64,
    #[serde(default)]
    #[allow(dead_code)]
    message: String,
    #[serde(default)]
    enable: bool,
    #[serde(default)]
    checked_in: bool,
    #[serde(default)]
    credits: Option<i64>,
}

fn checkin_body(
    client: &reqwest::blocking::Client,
    path: &str,
    token: &str,
    device_id: &str,
) -> Result<CheckinResponse, CheckinHttpError> {
    let url = format!("{API_BASE}{path}");
    let response = apply_telemetry_headers(
        client
            .post(&url)
            .header("Authorization", format!("Cloud-IDE-JWT {token}"))
            .header("x-device-id", device_id)
            .header("Content-Type", "application/json"),
        device_id,
    )
    .body("{}")
    .send()
    .map_err(|_| CheckinHttpError::Network)?;
    if !response.status().is_success() {
        return Err(CheckinHttpError::Http(response.status().as_u16()));
    }
    let body = response
        .json::<CheckinResponse>()
        .map_err(|_| CheckinHttpError::Protocol)?;
    if body.code != 0 {
        return Err(CheckinHttpError::Business(body.code));
    }
    Ok(body)
}

/// 查询账号签到状态；`enable=false` 表示当前不可领取。
pub fn checkin_status_request(
    client: &reqwest::blocking::Client,
    token: &str,
    device_id: &str,
) -> Result<CheckinStatusSnapshot, CheckinHttpError> {
    let body = checkin_body(client, CHECKIN_STATUS_PATH, token, device_id)?;
    Ok(CheckinStatusSnapshot {
        enabled: body.enable,
        checked_in: body.checked_in,
        credits: body.credits,
        business_code: Some(0),
    })
}

/// 发起一次 claim；`code != 0`（如 9074 陌生设备、9095 设备配额）按业务错误返回。
pub fn checkin_claim_request(
    client: &reqwest::blocking::Client,
    token: &str,
    device_id: &str,
) -> Result<CheckinClaimSnapshot, CheckinHttpError> {
    let body = checkin_body(client, CHECKIN_CLAIM_PATH, token, device_id)?;
    Ok(CheckinClaimSnapshot {
        business_code: Some(0),
        credits: body.credits,
    })
}

/// `ide_user_ent_usage` 响应（宽松解析：顶层成功时无 code 字段，见探测报告
/// `.scratch/checkin-http/reports/ent-usage-probe-20260823-000617.json`）。
#[derive(Debug, Deserialize)]
struct EntUsageResponse {
    #[serde(default)]
    code: Option<i64>,
    #[serde(default)]
    user_entitlement_pack_list: Vec<EntUsagePack>,
}

#[derive(Debug, Deserialize)]
struct EntUsagePack {
    #[serde(default)]
    display_desc: String,
    #[serde(default)]
    group_name: String,
    #[serde(default)]
    expire_time: u64,
    #[serde(default)]
    entitlement_base_info: Option<EntUsageBaseInfo>,
    #[serde(default)]
    usage: Option<EntUsageValues>,
}

#[derive(Debug, Deserialize)]
struct EntUsageBaseInfo {
    #[serde(default)]
    entitlement_id: String,
    #[serde(default)]
    quota: Option<EntUsageQuota>,
}

#[derive(Debug, Deserialize)]
struct EntUsageQuota {
    #[serde(default)]
    credits_limit: Option<f64>,
}

#[derive(Debug, Deserialize)]
struct EntUsageValues {
    #[serde(default)]
    credits_amount: Option<f64>,
}

/// 查询真实模型额度：`POST /trae/api/v2/pay/ide_user_ent_usage`。
/// 请求形状与 TRAE 客户端 `ICubeUsageService` 一致（`{require_usage:true}` +
/// Cloud-IDE-JWT 头）；汇总规则同客户端 `q_o`/`Zms`：剩余 = Σ max(limit - used, 0)，
/// 只统计未过期的积分包。
pub fn entitlement_usage_request(
    client: &reqwest::blocking::Client,
    token: &str,
    device_id: &str,
) -> Result<EntitlementUsageSnapshot, CheckinHttpError> {
    let url = format!("{API_BASE}{ENT_USAGE_PATH}");
    let response = client
        .post(&url)
        .header("Authorization", format!("Cloud-IDE-JWT {token}"))
        .header("x-device-id", device_id)
        .header("Content-Type", "application/json")
        .body(r#"{"require_usage":true}"#)
        .send()
        .map_err(|_| CheckinHttpError::Network)?;
    if !response.status().is_success() {
        return Err(CheckinHttpError::Http(response.status().as_u16()));
    }
    let body = response
        .json::<EntUsageResponse>()
        .map_err(|_| CheckinHttpError::Protocol)?;
    // 客户端判定：code 存在且非 0 视为业务错误；成功响应无 code 字段。
    if let Some(code) = body.code {
        if code != 0 {
            return Err(CheckinHttpError::Business(code));
        }
    }

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0);
    let mut snapshot = EntitlementUsageSnapshot::default();
    for pack in &body.user_entitlement_pack_list {
        // 无积分维度的包（如免费 solo 开关包）不参与汇总。
        let Some(base) = &pack.entitlement_base_info else {
            continue;
        };
        let Some(credits_limit) = base.quota.as_ref().and_then(|quota| quota.credits_limit)
        else {
            continue;
        };
        // 过期包不计入当前可用额度（expire_time=0 视为长期有效）。
        if pack.expire_time != 0 && pack.expire_time <= now {
            continue;
        }
        let credits_used = pack
            .usage
            .as_ref()
            .and_then(|usage| usage.credits_amount)
            .unwrap_or(0.0);
        snapshot.remaining_credits += (credits_limit - credits_used).max(0.0);
        snapshot.packs.push(EntitlementPackSnapshot {
            entitlement_id: base.entitlement_id.clone(),
            group_name: if pack.group_name.is_empty() {
                pack.display_desc.clone()
            } else {
                pack.group_name.clone()
            },
            credits_limit,
            credits_used,
            expires_at_unix_seconds: pack.expire_time,
        });
    }
    Ok(snapshot)
}

/// `ExchangeToken` 的设备信息块（08 号文档 j 函数还原）。
#[derive(Debug, Clone)]
pub struct DeviceInfoBlock {
    pub device_id: String,
    pub machine_id: String,
    pub platform_code: String,
    pub client_version: String,
    pub device_public_key: String,
    pub device_name: String,
    pub device_model: String,
    pub device_brand: String,
    pub device_cpu: String,
    pub os_info: String,
    pub os_version: String,
}

impl DeviceInfoBlock {
    /// SOLO 形态虚拟设备（v6 唯一形态；完整硬件字段，新设备首签依赖）。
    pub fn for_virtual_device(device_id: &str, machine_id: &str, device_public_key: &str) -> Self {
        OAuthClient::Solo.device_info(device_id, machine_id, device_public_key)
    }

    fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "DeviceID": self.device_id,
            "MachineID": self.machine_id,
            "PlatformCode": self.platform_code,
            "DeviceType": "PC",
            "DeviceName": self.device_name,
            "DeviceModel": self.device_model,
            "ClientVersion": self.client_version,
            "DevicePublicKey": self.device_public_key,
            "DeviceBrand": self.device_brand,
            "DeviceCPU": self.device_cpu,
            "OSInfo": self.os_info,
            "OSVersion": self.os_version,
        })
    }
}

/// OAuth 客户端形态（ADR-0019 v6：SOLO 是唯一形态）。
///
/// 协议事实（2026-08-25/26 实测）：
/// - SOLO 形态（en1oxy7wnw8j9n / SOLO_PC / 完整硬件字段）的 AuthCode
///   新设备可立即首签（手册 5.1，账号 2873473361250299 完整闭环）；
/// - 原 Work 形态（IDE_PC / 空硬件字段）设备配额耗尽且新设备首签被
///   9074 稳定拒绝，代码已删除（2026-09-02）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OAuthClient {
    /// TRAE SOLO CN 桌面客户端（登录、铸造、续期全链路唯一形态）。
    Solo,
}

impl OAuthClient {
    pub fn client_id(&self) -> &'static str {
        match self {
            Self::Solo => TRAE_SOLO_CLIENT_ID,
        }
    }

    pub fn platform_code(&self) -> &'static str {
        match self {
            Self::Solo => "SOLO_PC",
        }
    }

    pub fn ide_version(&self) -> &'static str {
        match self {
            Self::Solo => TRAE_SOLO_IDE_VERSION,
        }
    }

    /// 构造设备信息块；硬件字段与真实客户端一致（SOLO 形态
    /// 带完整本机硬件描述——r34 探针实测值，新设备首签依赖这些字段）。
    pub fn device_info(&self, device_id: &str, machine_id: &str, device_public_key: &str) -> DeviceInfoBlock {
        match self {
            Self::Solo => DeviceInfoBlock {
                device_id: device_id.to_string(),
                machine_id: machine_id.to_string(),
                platform_code: self.platform_code().to_string(),
                client_version: self.ide_version().to_string(),
                device_public_key: device_public_key.to_string(),
                // 本机真实硬件描述（TRAE SOLO CN 客户端上报值；个人工具，
                // 与已验证首签路线保持完全一致）。
                device_name: "1273223663的电脑".to_string(),
                device_model: "81Q5".to_string(),
                device_brand: "LENOVO".to_string(),
                device_cpu: "Intel(R) Core(TM) i7-9750H CPU".to_string(),
                os_info: "windows".to_string(),
                os_version: "Windows 11 Home".to_string(),
            },
        }
    }
}

/// `ExchangeToken` 成功签发的凭证（敏感字段只在后端内存/加密包中流转）。
#[derive(Clone)]
pub struct TokenGrant {
    pub access_token: String,
    pub refresh_token: String,
    pub access_token_expires_at_unix_seconds: u64,
    pub refresh_token_expires_at_unix_seconds: u64,
}

#[derive(Debug, Deserialize)]
struct ExchangeResult {
    #[serde(default)]
    Token: Option<String>,
    #[serde(default)]
    RefreshToken: Option<String>,
    #[serde(default)]
    TokenExpireAt: Option<u64>,
    #[serde(default)]
    RefreshExpireAt: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct ExchangeEnvelope {
    #[serde(default)]
    ResponseMetadata: Option<serde_json::Value>,
    #[serde(default)]
    Result: Option<ExchangeResult>,
}

fn exchange_error_code(envelope: &ExchangeEnvelope) -> Option<i64> {
    envelope
        .ResponseMetadata
        .as_ref()
        .and_then(|metadata| metadata.get("Error"))
        .and_then(|error| error.get("Code"))
        // 服务端实际把 Code 返回为 JSON 字符串（"20401"），数字形态仅为
        // 兼容保留——只认数字会吞掉 20401 等关键错误（2026-09-02 实测）。
        .and_then(|code| {
            code.as_i64()
                .or_else(|| code.as_str().and_then(|s| s.parse().ok()))
        })
}

/// 非 200 的 ExchangeToken 响应错误：优先取响应体业务码，取不到回退 Http 状态码。
fn exchange_http_error(status: u16, body: &str) -> CheckinHttpError {
    if let Ok(envelope) = serde_json::from_str::<ExchangeEnvelope>(body) {
        if let Some(code) = exchange_error_code(&envelope) {
            return CheckinHttpError::Business(code);
        }
    }
    CheckinHttpError::Http(status)
}

/// 最近一次 ExchangeToken 失败的服务端响应摘要（诊断用）。
/// 仅存错误信息，不含 Token（失败响应体无凭证）。
pub fn take_last_exchange_failure_detail() -> Option<String> {
    EXCHANGE_FAILURE_DETAIL
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
        .take()
}

fn record_exchange_failure_detail(status: u16, body: &str) {
    // 摘要截断到 500 字符：错误响应不含 Token（仅错误描述/网关 HTML），
    // 足够定位原因又不至于写爆日志。
    let detail = format!(
        "http={} body={}",
        status,
        if body.len() > 500 { &body[..500] } else { body }
    );
    *EXCHANGE_FAILURE_DETAIL
        .lock()
        .unwrap_or_else(|poison| poison.into_inner()) = Some(detail);
}

static EXCHANGE_FAILURE_DETAIL: Mutex<Option<String>> = Mutex::new(None);

fn parse_exchange_envelope(
    response: reqwest::blocking::Response,
) -> Result<TokenGrant, CheckinHttpError> {
    if !response.status().is_success() {
        let status = response.status().as_u16();
        // 非 200 也尝试解析响应体业务码：服务端把设备上限（20401）等业务
        // 拒绝以 403/400 + 响应体业务码的形式返回（2026-09-02 实测），
        // 只报 Http 状态码会吞掉真实原因（登录/重铸失败无法定位）。
        // 响应体非 JSON（如网关 HTML 页）时回退 Http 状态码。
        let body = response.text().unwrap_or_default();
        record_exchange_failure_detail(status, &body);
        return Err(exchange_http_error(status, &body));
    }
    let envelope = response
        .json::<ExchangeEnvelope>()
        .map_err(|_| CheckinHttpError::Protocol)?;
    if let Some(code) = exchange_error_code(&envelope) {
        record_exchange_failure_detail(200, &format!("business_code={code}"));
        return Err(CheckinHttpError::Business(code));
    }
    let result = envelope.Result.ok_or(CheckinHttpError::Protocol)?;
    let access_token = result.Token.ok_or(CheckinHttpError::Protocol)?;
    let refresh_token = result.RefreshToken.ok_or(CheckinHttpError::Protocol)?;
    let token_expire_ms = result.TokenExpireAt.ok_or(CheckinHttpError::Protocol)?;
    let refresh_expire_ms = result.RefreshExpireAt.ok_or(CheckinHttpError::Protocol)?;
    Ok(TokenGrant {
        access_token,
        refresh_token,
        // 服务端返回毫秒时间戳，本地统一存秒。
        access_token_expires_at_unix_seconds: token_expire_ms / 1000,
        refresh_token_expires_at_unix_seconds: refresh_expire_ms / 1000,
    })
}

/// AuthCode 模式换取凭证（免 DeviceProof 签名；OAuth 登录与设备重铸共用）。
/// `oauth_client` 决定 ClientID/IDEVersion；DeviceInfo 必须与其形态一致。
pub fn exchange_token_by_auth_code(
    client: &reqwest::blocking::Client,
    auth_code: &str,
    code_verifier: &str,
    device_info: &DeviceInfoBlock,
    oauth_client: OAuthClient,
) -> Result<TokenGrant, CheckinHttpError> {
    let body = serde_json::json!({
        "ClientID": oauth_client.client_id(),
        "AuthCode": auth_code,
        "CodeVerifier": code_verifier,
        "DeviceInfo": device_info.to_json(),
        "IDEVersion": oauth_client.ide_version(),
    });
    let response = client
        .post(format!("{API_BASE}{EXCHANGE_TOKEN_PATH}"))
        .header("Content-Type", "application/json")
        // 授权页来源头：2026-09-02 实测网关存在动态 IP 风控（测试请求过密时
        // 无浏览器头请求被 403 拦截，浏览器头可通过）；与真实客户端和
        // `get_pc_auth_code` 保持一致，降低被拦截概率。
        .header("User-Agent", OAUTH_USER_AGENT)
        .header("Referer", "https://www.trae.cn/authorization")
        .header("Origin", "https://www.trae.cn")
        .json(&body)
        .send()
        .map_err(|_| CheckinHttpError::Network)?;
    parse_exchange_envelope(response)
}

/// `GetPCAuthCode` 响应（新设备铸造入口，2026-08-25 手册实证路线）。
#[derive(Debug, Deserialize)]
struct AuthCodeEnvelope {
    #[serde(default)]
    ResponseMetadata: Option<serde_json::Value>,
    #[serde(default)]
    Result: Option<AuthCodeResult>,
}

#[derive(Debug, Deserialize)]
struct AuthCodeResult {
    #[serde(default)]
    AuthCode: Option<String>,
}

/// 用当前账号访问令牌为指定新设备签发一次性 AuthCode。
///
/// 协议事实（2026-08-25 实测，`.scratch/checkin-http/reports/p01-*.json`）：
/// - 认证头必须是 `x-cloudide-token`；普通业务接口的
///   `Authorization: Cloud-IDE-JWT` 头会被 401/20101 拒绝；
/// - `DeviceID` 为尚未注册的新设备即可（随本请求向服务端报备），
///   无需任何设备证明材料——这是 AuthCode 模式与 refresh 模式的
///   根本差异（refresh 模式新设备首签会被 9074 拦截，不可用于铸造）；
/// - AuthCode 一次性消费；失败后必须重新生成 PKCE 与设备材料，
///   不得复用旧 AuthCode。
///
/// 成功返回一次性 AuthCode 字符串。
pub fn get_pc_auth_code(
    client: &reqwest::blocking::Client,
    access_token: &str,
    code_challenge: &str,
    device_id: &str,
    oauth_client: OAuthClient,
) -> Result<String, CheckinHttpError> {
    let body = serde_json::json!({
        "ClientID": oauth_client.client_id(),
        "CodeChallenge": code_challenge,
        "CodeChallengeMethod": "S256",
        "RedirectURI": OAUTH_REDIRECT_URI,
        "DeviceID": device_id,
        "PlatformCode": oauth_client.platform_code(),
    });
    let response = client
        .post(format!("{API_BASE}{GET_PC_AUTH_CODE_PATH}"))
        .header("Content-Type", "application/json")
        .header("x-cloudide-token", access_token)
        .header("User-Agent", OAUTH_USER_AGENT)
        .header("Referer", "https://www.trae.cn/authorization")
        .header("Origin", "https://www.trae.cn")
        .json(&body)
        .send()
        .map_err(|_| CheckinHttpError::Network)?;
    if !response.status().is_success() {
        return Err(CheckinHttpError::Http(response.status().as_u16()));
    }
    let envelope = response
        .json::<AuthCodeEnvelope>()
        .map_err(|_| CheckinHttpError::Protocol)?;
    // 错误结构与 ExchangeToken 一致（ResponseMetadata.Error.Code，
    // 服务端实际返回字符串形态，需同时兼容数字）。
    if let Some(code) = envelope
        .ResponseMetadata
        .as_ref()
        .and_then(|metadata| metadata.get("Error"))
        .and_then(|error| error.get("Code"))
        .and_then(|code| {
            code.as_i64()
                .or_else(|| code.as_str().and_then(|s| s.parse().ok()))
        })
    {
        return Err(CheckinHttpError::Business(code));
    }
    envelope
        .Result
        .and_then(|result| result.AuthCode)
        .filter(|code| !code.is_empty())
        .ok_or(CheckinHttpError::Protocol)
}

/// refreshToken 模式续期（需设备私钥 ECDSA-SHA256 签名）。
/// `oauth_client` 必须与该设备注册时的形态一致（ClientID 参与 DeviceProof 签名）。
pub fn exchange_token_by_refresh(
    client: &reqwest::blocking::Client,
    current_token: &str,
    refresh_token: &str,
    device_private_key_pem: &str,
    device_info: &DeviceInfoBlock,
    oauth_client: OAuthClient,
    timestamp_unix_seconds: u64,
    nonce: &str,
) -> Result<TokenGrant, CheckinHttpError> {
    let input = device_proof_signing_input(
        "POST",
        EXCHANGE_TOKEN_PATH,
        oauth_client.client_id(),
        refresh_token,
        timestamp_unix_seconds,
        nonce,
    );
    let signature_base64 = base64::engine::general_purpose::STANDARD.encode(
        sign_device_proof(device_private_key_pem, &input)
            .map_err(|_| CheckinHttpError::Protocol)?,
    );
    let body = serde_json::json!({
        "ClientID": oauth_client.client_id(),
        "ClientSecret": "",
        "RefreshToken": refresh_token,
        "DeviceInfo": device_info.to_json(),
        "DeviceProof": {
            "Signature": signature_base64,
            "Timestamp": timestamp_unix_seconds,
            "Nonce": nonce,
        },
        "IDEVersion": oauth_client.ide_version(),
    });
    let response = client
        .post(format!("{API_BASE}{EXCHANGE_TOKEN_PATH}"))
        .header("Content-Type", "application/json")
        .header("x-cloudide-token", current_token)
        .json(&body)
        .send()
        .map_err(|_| CheckinHttpError::Network)?;
    parse_exchange_envelope(response)
}

/// `GetUserInfo` 返回的脱敏账号资料；不含原始手机号或认证材料。
#[derive(Debug, Clone, Default)]
pub struct UserInfoSummary {
    pub screen_name: String,
    pub avatar_url: String,
    pub masked_mobile: String,
}

/// `GetUserInfo` 完整资料（P7-1 切号 E2 构造输入）：身份 + 资料 + 区域字段。
/// 全部为脱敏/公开信息，不含原始手机号或认证材料。
#[derive(Debug, Clone, Default)]
pub struct UserInfoFull {
    /// 服务端账号 ID（身份校验：必须与凭据包 account_id 一致）。
    pub user_id: String,
    pub screen_name: String,
    pub avatar_url: String,
    pub masked_mobile: String,
    pub masked_email: String,
    pub description: String,
    pub region: String,
    pub ai_region: String,
    pub migrate_to_sg: bool,
}

#[derive(Debug, Deserialize)]
struct UserInfoEnvelope {
    #[serde(default)]
    Result: Option<UserInfoResult>,
}

#[derive(Debug, Deserialize)]
struct UserInfoResult {
    #[serde(default)]
    UserID: Option<String>,
    #[serde(default)]
    ScreenName: Option<String>,
    #[serde(default)]
    AvatarUrl: Option<String>,
    #[serde(default)]
    NonPlainTextMobile: Option<String>,
    #[serde(default)]
    NonPlainTextEmail: Option<String>,
    #[serde(default)]
    Description: Option<String>,
    #[serde(default)]
    Region: Option<String>,
    #[serde(default)]
    AIRegion: Option<String>,
    #[serde(default)]
    MigrateToSG: Option<bool>,
}

/// 只读查询账号资料（屏幕名/头像/脱敏手机号）。
pub fn get_user_info(
    client: &reqwest::blocking::Client,
    token: &str,
) -> Result<UserInfoSummary, CheckinHttpError> {
    let full = get_user_info_full(client, token)?;
    Ok(UserInfoSummary {
        screen_name: full.screen_name,
        avatar_url: full.avatar_url,
        masked_mobile: full.masked_mobile,
    })
}

/// 只读查询完整账号资料（P7-1 E2 构造路径输入；同协议同端点，
/// 解析全部资料与区域字段——字段映射表见 `.scratch/e2-credential-login/REPORT.md`）。
pub fn get_user_info_full(
    client: &reqwest::blocking::Client,
    token: &str,
) -> Result<UserInfoFull, CheckinHttpError> {
    let body = serde_json::json!({
        "ReqSource": "IDE",
        "IDEVersion": TRAE_IDE_VERSION,
    });
    let response = client
        .post(format!("{API_BASE}{GET_USER_INFO_PATH}"))
        .header("x-cloudide-token", token)
        .json(&body)
        .send()
        .map_err(|_| CheckinHttpError::Network)?;
    if !response.status().is_success() {
        return Err(CheckinHttpError::Http(response.status().as_u16()));
    }
    let envelope = response
        .json::<UserInfoEnvelope>()
        .map_err(|_| CheckinHttpError::Protocol)?;
    let result = envelope.Result.ok_or(CheckinHttpError::Protocol)?;
    Ok(UserInfoFull {
        user_id: result.UserID.unwrap_or_default(),
        screen_name: result.ScreenName.unwrap_or_default(),
        avatar_url: result.AvatarUrl.unwrap_or_default(),
        masked_mobile: result.NonPlainTextMobile.unwrap_or_default(),
        masked_email: result.NonPlainTextEmail.unwrap_or_default(),
        description: result.Description.unwrap_or_default(),
        region: result.Region.unwrap_or_default(),
        ai_region: result.AIRegion.unwrap_or_default(),
        migrate_to_sg: result.MigrateToSG.unwrap_or(false),
    })
}

/// 真实凭据续期编排：阈值检查 -> refresh 模式 `ExchangeToken`（设备签名）->
/// 新令牌身份校验（JWT `data.id` 必须匹配绑定账号）-> 安全写回。
///
/// 与 `FixtureRenewalService` 遵循同一生命周期契约（ADR-0014）：
/// 中断现场未收口前禁止续期；身份不一致零回写；写回走备份/临时文件/
/// 原子替换/重新验证固定顺序。
pub struct RealCheckinRenewalService<'a> {
    store: &'a CheckinCredentialStore,
    client: reqwest::blocking::Client,
}

impl<'a> RealCheckinRenewalService<'a> {
    pub fn new(store: &'a CheckinCredentialStore) -> Self {
        Self {
            store,
            client: http_client(),
        }
    }

    /// 剩余寿命高于阈值时跳过（`Ok(None)`）；需要时执行一次真实续期。
    pub fn renew_if_needed(
        &self,
        binding: &CheckinProfileBinding,
        now_unix_seconds: u64,
    ) -> Result<Option<RenewalReceipt>, CheckinCredentialError> {
        // 中断现场未收口前禁止新的写回。
        if self.store.has_pending_renewal()? {
            return Err(CheckinCredentialError::RecoveryRequired);
        }
        let bundle = self.store.load(binding)?;
        if !needs_refresh(&bundle, now_unix_seconds) {
            return Ok(None);
        }
        self.renew(binding, bundle, now_unix_seconds).map(Some)
    }

    fn renew(
        &self,
        binding: &CheckinProfileBinding,
        bundle: CheckinCredentialBundle,
        timestamp_unix_seconds: u64,
    ) -> Result<RenewalReceipt, CheckinCredentialError> {
        // 续期固定 SOLO 形态（v6 唯一形态）；旧 Work 凭据的续期会被
        // 服务端按签名/形态校验拒绝，失败即 CredentialRefreshFailed，
        // 由 UI 引导重新登录。
        let oauth_client = OAuthClient::Solo;
        // DeviceInfo 必须与登录/铸造时一致（设备绑定校验依据）。
        let device_info = oauth_client.device_info(
            &bundle.device_id,
            &bundle.machine_id,
            &bundle.device_public_key,
        );
        let nonce = random_nonce()?;
        let grant = exchange_token_by_refresh(
            &self.client,
            &bundle.access_token,
            &bundle.refresh_token,
            &bundle.device_private_key,
            &device_info,
            oauth_client,
            timestamp_unix_seconds,
            &nonce,
        )
        .map_err(|_| CheckinCredentialError::CredentialRefreshFailed)?;
        // 身份校验：新访问令牌签发的账号 ID 必须与绑定一致，否则禁止回写。
        let (account_id, _) = decode_account_from_jwt(&grant.access_token)
            .map_err(|_| CheckinCredentialError::AuthMismatch)?;
        if account_id != binding.account_id {
            return Err(CheckinCredentialError::AuthMismatch);
        }
        let mut updated = bundle;
        updated.access_token = grant.access_token;
        updated.refresh_token = grant.refresh_token;
        updated.access_token_expires_at_unix_seconds = grant.access_token_expires_at_unix_seconds;
        updated.refresh_token_expires_at_unix_seconds = grant.refresh_token_expires_at_unix_seconds;
        let operation_id = self.store.write_back(binding, &updated)?;
        // U-7 C1 保活：续期成功即把新凭据同步进实例登录 blob（TRAE 下次启动
        // 免登录）。保活是增强能力：失败只记 stderr 日志，不影响续期结果；
        // 签到失败路径在本函数之前就已返回，绝不触碰 blob。
        crate::blob_keepalive::keepalive_after_renewal(
            self.store.root(),
            &binding.profile_id,
            &updated.access_token,
            &updated.refresh_token,
            updated.access_token_expires_at_unix_seconds,
            updated.refresh_token_expires_at_unix_seconds,
        );
        Ok(RenewalReceipt {
            profile_id: binding.profile_id.clone(),
            operation_id,
            // 真实协议 refresh 模式每次返回新 refresh token（实测轮换）。
            refresh_token_rotated: true,
        })
    }
}

/// 生成 DeviceProof nonce（16 随机字节 hex）。
fn random_nonce() -> Result<String, CheckinCredentialError> {
    let mut bytes = [0u8; 16];
    openssl::rand::rand_bytes(&mut bytes)
        .map_err(|_| CheckinCredentialError::CredentialRefreshFailed)?;
    Ok(hex::encode(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn telemetry_derived_market_user_id_shape_and_stability() {
        // x-market-user-id：同设备稳定、跨设备互异、UUID v4 形状
        // （8-4-4-4-12，版本位 4，变体位 8/9/a/b）。
        let id_a = derived_market_user_id("123456789012345");
        let id_b = derived_market_user_id("123456789012345");
        let id_c = derived_market_user_id("987654321098765");
        assert_eq!(id_a, id_b, "同设备派生必须稳定");
        assert_ne!(id_a, id_c, "跨设备派生必须互异");
        assert_eq!(id_a.len(), 36);
        let segments: Vec<usize> = id_a.split('-').map(str::len).collect();
        assert_eq!(segments, vec![8, 4, 4, 4, 12]);
        assert_eq!(id_a.as_bytes()[14], b'4', "版本位必须是 4");
        let variant = id_a.as_bytes()[19];
        assert!(
            matches!(variant, b'8' | b'9' | b'a' | b'b'),
            "变体位非法：{variant}"
        );
    }

    #[test]
    fn telemetry_derived_session_id_shape() {
        // vscode-sessionid：64 位 hex，同设备稳定、跨设备互异。
        let session_a = derived_session_id("123456789012345");
        let session_b = derived_session_id("123456789012345");
        let session_c = derived_session_id("987654321098765");
        assert_eq!(session_a.len(), 64);
        assert!(session_a.chars().all(|c| c.is_ascii_hexdigit()));
        assert_eq!(session_a, session_b);
        assert_ne!(session_a, session_c);
    }

    #[test]
    fn telemetry_fresh_ids_differ_per_request() {
        // x-request-id / x-tt-trace-id：每次请求必须刷新。
        let first = fresh_request_id();
        let second = fresh_request_id();
        assert_ne!(first, second);
        assert_eq!(first.len(), 36);

        let trace_first = fresh_traceparent();
        let trace_second = fresh_traceparent();
        assert_ne!(trace_first, trace_second);
        // traceparent 形状：00-<32hex>-<16hex>-01。
        let parts: Vec<&str> = trace_first.split('-').collect();
        assert_eq!(parts.len(), 4);
        assert_eq!(parts[0], "00");
        assert_eq!(parts[1].len(), 32);
        assert_eq!(parts[2].len(), 16);
        assert_eq!(parts[3], "01");
        assert!(parts[1].chars().all(|c| c.is_ascii_hexdigit()));
        assert!(parts[2].chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn checkin_response_parses_business_rejection() {
        // 9095：HTTP 200 但业务码拒绝（05 号文档实测原文结构）。
        let body = r#"{"code":9095,"message":"当前设备今日已经签到"}"#;
        let parsed: CheckinResponse = serde_json::from_str(body).unwrap();
        assert_eq!(parsed.code, 9095);
    }

    #[test]
    fn checkin_response_parses_success_status() {
        let body =
            r#"{"code":0,"message":"success","enable":true,"checked_in":false,"credits":200}"#;
        let parsed: CheckinResponse = serde_json::from_str(body).unwrap();
        assert_eq!(parsed.code, 0);
        assert!(parsed.enable);
        assert!(!parsed.checked_in);
        assert_eq!(parsed.credits, Some(200));
    }

    #[test]
    fn exchange_envelope_parses_real_shape() {
        // 08 号文档实测成功响应结构（截取非敏感骨架）。
        let body = r#"{
            "ResponseMetadata": {"RequestId":"req-1"},
            "Result": {
                "Token": "token-1",
                "RefreshToken": "refresh-1",
                "TokenExpireAt": 1788494618314,
                "RefreshExpireAt": 1802837018314
            }
        }"#;
        let envelope: ExchangeEnvelope = serde_json::from_str(body).unwrap();
        assert!(exchange_error_code(&envelope).is_none());
        let result = envelope.Result.unwrap();
        assert_eq!(result.Token.as_deref(), Some("token-1"));
        assert_eq!(result.TokenExpireAt, Some(1788494618314));
    }

    #[test]
    fn exchange_envelope_extracts_error_code() {
        let body =
            r#"{"ResponseMetadata":{"Error":{"Code":20324,"Message":"refresh token invalid"}}}"#;
        let envelope: ExchangeEnvelope = serde_json::from_str(body).unwrap();
        assert_eq!(exchange_error_code(&envelope), Some(20324));
    }

    #[test]
    fn exchange_http_error_prefers_body_business_code() {
        // 2026-09-02 实测：设备上限 20401 以 HTTP 403 + 响应体业务码返回；
        // 只报 Http(403) 会把真实原因（设备配额）吞成笼统的网络错误。
        let body = r#"{"ResponseMetadata":{"Error":{"Code":20401,"Message":"Device limit reached."}}}"#;
        assert_eq!(exchange_http_error(403, body), CheckinHttpError::Business(20401));
        // 同日实测：服务端实际返回的 Code 是 JSON 字符串形态——
        // 只解析数字会吞掉 20401（登录诊断日志 Http(403) 误报根因）。
        let string_code_body = r#"{"ResponseMetadata":{"Error":{"Code":"20401","Message":"Device limit reached.","StandardCode":"040034"}}}"#;
        assert_eq!(
            exchange_http_error(403, string_code_body),
            CheckinHttpError::Business(20401)
        );
        // 响应体不是 JSON（如网关 HTML 页）：回退 Http 状态码。
        assert_eq!(
            exchange_http_error(403, "<html>Forbidden</html>"),
            CheckinHttpError::Http(403)
        );
        // JSON 但无业务码：同样回退。
        assert_eq!(
            exchange_http_error(400, r#"{"unrelated":"shape"}"#),
            CheckinHttpError::Http(400)
        );
    }

    #[test]
    fn authcode_envelope_parses_issued_code() {
        // r34 探针实测成功响应结构（截取非敏感骨架）。
        let body = r#"{
            "ResponseMetadata": {"RequestId": "req-1"},
            "Result": {"AuthCode": "auth-code-1"}
        }"#;
        let envelope: AuthCodeEnvelope = serde_json::from_str(body).unwrap();
        assert_eq!(
            envelope.Result.and_then(|result| result.AuthCode).as_deref(),
            Some("auth-code-1")
        );
        // 无 Error 字段（成功路径）。
        assert!(!envelope
            .ResponseMetadata
            .as_ref()
            .and_then(|metadata| metadata.get("Error"))
            .is_some());
    }

    #[test]
    fn authcode_envelope_extracts_error_code() {
        // 认证头错误：HTTP 401 + 业务码 20101（r34 实测 Authorization 头被拒）。
        let body = r#"{
            "ResponseMetadata": {"Error": {"Code": 20101, "Message": "auth failed"}},
            "Result": null
        }"#;
        let envelope: AuthCodeEnvelope = serde_json::from_str(body).unwrap();
        let code = envelope
            .ResponseMetadata
            .as_ref()
            .and_then(|metadata| metadata.get("Error"))
            .and_then(|error| error.get("Code"))
            .and_then(|code| code.as_i64());
        assert_eq!(code, Some(20101));
        assert!(envelope.Result.is_none());
    }

    #[test]
    fn error_mapping_covers_transport_kinds() {
        assert_eq!(
            CheckinHttpError::Network.to_transport_error(),
            CheckinTransportError::Network
        );
        assert_eq!(
            CheckinHttpError::Business(9095).to_transport_error(),
            CheckinTransportError::Business(9095)
        );
        assert_eq!(
            CheckinHttpError::Protocol.to_transport_error(),
            CheckinTransportError::Protocol
        );
    }

    #[test]
    fn device_info_block_matches_protocol_shape() {
        let block = DeviceInfoBlock::for_virtual_device("1234567890123456", "machine-1", "pub-1");
        let json = block.to_json();
        assert_eq!(json["DeviceID"], "1234567890123456");
        assert_eq!(json["PlatformCode"], "SOLO_PC");
        assert_eq!(json["ClientVersion"], TRAE_SOLO_IDE_VERSION);
        assert_eq!(json["DevicePublicKey"], "pub-1");
    }

    #[cfg(windows)]
    #[test]
    fn real_renewal_skips_when_above_threshold() {
        let root = tempfile::tempdir().unwrap();
        let store = CheckinCredentialStore::new(root.path());
        let (private_pem, public_pem) =
            crate::checkin_credential::generate_device_keypair().unwrap();
        let now = 1_800_000_000u64;
        // 刚签发的凭据（14 天/180 天剩余），高于 7 天/30 天阈值。
        let bundle = CheckinCredentialBundle {
            profile_id: "profile-a".to_string(),
            account_id: "account-1".to_string(),
            device_id: "1234567890123456".to_string(),
            machine_id: "machine-1".to_string(),
            device_public_key: public_pem.clone(),
            device_private_key: private_pem,
            access_token: "token-1".to_string(),
            refresh_token: "refresh-1".to_string(),
            client_id: "client-1".to_string(),
            access_token_expires_at_unix_seconds: now + 14 * 24 * 60 * 60,
            refresh_token_expires_at_unix_seconds: now + 180 * 24 * 60 * 60,
            mobile_full: None,
        };
        store.save(&bundle).unwrap();
        let binding =
            CheckinProfileBinding::new("profile-a", "account-1", "1234567890123456", &public_pem);
        let service = RealCheckinRenewalService::new(&store);
        // 剩余寿命充足：跳过续期，不发起网络请求。
        assert!(service.renew_if_needed(&binding, now).unwrap().is_none());
    }

    #[test]
    fn real_renewal_blocked_by_pending_journal() {
        let root = tempfile::tempdir().unwrap();
        let store = CheckinCredentialStore::new(root.path());
        // 手工构造中断现场：prepared 状态 journal（未收口）。
        let journal_dir = root.path().join("renewals").join("op-1");
        std::fs::create_dir_all(&journal_dir).unwrap();
        let manifest = serde_json::json!({
            "format_version": 1,
            "operation_id": "op-1",
            "operation": "checkin_credential_renewal",
            "sequence": 1,
            "state": "prepared",
            "profile_id": "profile-a",
            "backup_file": "renewals/op-1/bundle.before",
            "temporary_file": "x.tmp",
            "before_sha256": "00",
            "target_sha256": "00",
            "created_at_unix_seconds": 1,
        });
        std::fs::write(journal_dir.join("manifest.json"), manifest.to_string()).unwrap();
        let service = RealCheckinRenewalService::new(&store);
        let binding = CheckinProfileBinding::new("profile-a", "account-1", "d", "k");
        // 中断现场未收口前任何续期请求都被拒绝（先于绑定校验，无网络请求）。
        assert_eq!(
            service.renew_if_needed(&binding, 0).err(),
            Some(CheckinCredentialError::RecoveryRequired)
        );
    }
}
