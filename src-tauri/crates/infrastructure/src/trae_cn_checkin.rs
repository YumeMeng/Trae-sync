//! Trae CN 签到协议实现（P13-1；产品中立 seam 模式，对齐 ADR-0031
//! `remote_device.rs` 先例：注入 seam + Reqwest 适配器 + Real 实现 +
//! 凭据产品闸。`checkin_http.rs` 保持 Work 专用，本模块不改动其逻辑）。
//!
//! 协议事实（探针实测报告 `.scratch/checkin-http/reports/trae-cn-checkin-probe-20260928.json`
//! ——协议形状唯一权威来源）：
//! - status：`POST /trae/api/v2/ug/checkin_credits/status`（GET 同响应），
//!   200 + code=0；请求形状 = `Authorization: Cloud-IDE-JWT <token>` +
//!   `x-device-id` + CN 最小标准头 + `{}` 体（探针即以此形状通过，2026-09-28）。
//! - 响应字段：checked_in / code / credits / did_checked_in / enable /
//!   extra_credits / message（较 Work 多 did_checked_in / extra_credits）。
//! - claim：探针红线未发送 claim。路径按 Work 同族推导
//!   （`/trae/api/v2/ug/checkin_credits/claim`，与 Work checkin_http.rs
//!   同前缀 + /claim），请求/响应形状**未经真机验证**——P13-4 三步验收
//!   （ADR-0033 决策 2 ③签到 claim+额度核对）可能需调整。
//! - 两族头形态不混用：`cloudide/api/v3/*`（GetUserInfo 等）对 CN token 需
//!   `x-cloudide-token` 头（健康探针报告 trae-cn-health-probe-1790684645.json
//!   结论）；`ug/checkin_credits/*` 族探针实测接受 Cloud-IDE-JWT 头。
//! - CN 凭据无 Work 的签到虚拟设备 EC 密钥对依赖：status/claim 请求不带
//!   DeviceProof 签名，仅凭 token + device_id 头（探针验证的最小形态）。
//!
//! 业务码纪律：HTTP 200 不代表成功，业务码在响应体 `code`；非 0 业务码
//! 原样透传（`Business(i64)`），不压平为笼统失败。
//! 日志纪律：本模块不产生任何日志输出；错误类型不携带 Token、refresh
//! token 或响应正文。

use std::fmt;
use std::path::PathBuf;

use serde::Serialize;
use serde_json::Value;

use crate::checkin_credential::{
    CheckinCredentialBundle, CheckinCredentialError, CheckinCredentialStore, CheckinProfileBinding,
};
use crate::checkin_http::{trae_http_client, OAuthClient, TRAE_CN_IDE_VERSION};

/// CN 签到协议基地址（与 Work 同网关；checkin_probe 探针同源）。
const API_BASE: &str = "https://api.trae.cn";
/// status 端点（探针实测 200/code=0；POST 与 GET 同响应，取 POST 与 Work 同族）。
const CHECKIN_STATUS_PATH: &str = "/trae/api/v2/ug/checkin_credits/status";
/// claim 端点（按 Work 同族推导：同前缀 + /claim；未经真机验证）。
const CHECKIN_CLAIM_PATH: &str = "/trae/api/v2/ug/checkin_credits/claim";
/// CN 客户端 UA（与 checkin_probe.rs 探针同源的公开标识，非凭据）。
const TRAE_CN_USER_AGENT: &str = "VSCode 1.107.1 (Trae CN)";
/// CN 安装包类型（与 checkin_probe.rs 探针同源）。
const TRAE_CN_PACKAGE_TYPE: &str = "stable_cn";

// ============================================================================
// 请求描述与纯函数构造（形状可被单测固化；发送由 HTTP seam 承担）
// ============================================================================

/// CN 签到请求描述：路径/头/体的显式清单，构造（纯函数）与发送（seam）分离，
/// 请求形状可被单测逐项断言（探针实测形态固化的载体）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TraeCnCheckinRequest {
    pub method: &'static str,
    pub path: &'static str,
    /// 显式头清单（含认证头；仅在本模块与测试内流转，不进日志）。
    pub headers: Vec<(&'static str, String)>,
    pub body: String,
}

/// status 请求形状（探针实测形态：Cloud-IDE-JWT + x-device-id + CN 最小标准头）。
pub fn status_request(access_token: &str, device_id: &str) -> TraeCnCheckinRequest {
    checkin_request(CHECKIN_STATUS_PATH, access_token, device_id)
}

/// claim 请求形状（Work 同族推导；未经真机验证，P13-4 验收可能需调整）。
pub fn claim_request(access_token: &str, device_id: &str) -> TraeCnCheckinRequest {
    checkin_request(CHECKIN_CLAIM_PATH, access_token, device_id)
}

/// status/claim 共同请求形状：与探针 `probe_one` 通过形态逐项同源
/// （认证头 + 设备头 + 最小标准头 + `{}` 体），不假借 Work SOLO 遥测值。
fn checkin_request(
    path: &'static str,
    access_token: &str,
    device_id: &str,
) -> TraeCnCheckinRequest {
    TraeCnCheckinRequest {
        method: "POST",
        path,
        headers: vec![
            ("Authorization", format!("Cloud-IDE-JWT {access_token}")),
            ("x-device-id", device_id.to_string()),
            ("Content-Type", "application/json".to_string()),
            ("User-Agent", TRAE_CN_USER_AGENT.to_string()),
            ("app-version", TRAE_CN_IDE_VERSION.to_string()),
            ("package-type", TRAE_CN_PACKAGE_TYPE.to_string()),
            ("x-user-region", "CN".to_string()),
        ],
        body: "{}".to_string(),
    }
}

// ============================================================================
// 错误类型与 DTO
// ============================================================================

/// CN 签到协议错误；不携带 Token、refresh token 或响应正文。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TraeCnCheckinError {
    /// 网络层失败（连接/超时/DNS/TLS）。
    Network,
    /// 服务端返回非 200 且响应体无可解析业务码。
    Http(u16),
    /// 业务码拒绝（响应体 code 非 0；原样透传不压平）。
    Business(i64),
    /// 响应结构不符合协议预期。
    Protocol,
    /// 本地凭据不可用或与期望产品不匹配；原因码为稳定 snake_case 标识。
    Credential(&'static str),
}

impl fmt::Display for TraeCnCheckinError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        // 输出稳定 snake_case 原因码：供命令层映射为前端词表文案，
        // 不携带任何敏感材料。
        match self {
            Self::Network => formatter.write_str("trae_cn_checkin_network"),
            Self::Http(status) => write!(formatter, "trae_cn_checkin_http_{status}"),
            Self::Business(code) => write!(formatter, "trae_cn_checkin_business_{code}"),
            Self::Protocol => formatter.write_str("trae_cn_checkin_protocol"),
            Self::Credential(reason) => {
                write!(formatter, "trae_cn_checkin_credential_{reason}")
            }
        }
    }
}

impl std::error::Error for TraeCnCheckinError {}

/// status 响应 DTO（探针实测字段全集；业务码透传不压平）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TraeCnCheckinStatus {
    /// 当前是否可领取（enable=false 表示不可领取）。
    pub enable: bool,
    /// 今日是否已签。
    pub checked_in: bool,
    /// 签到积分（活动积分池值）。
    pub credits: Option<i64>,
    /// CN 特有：是否曾签（探针报告顶层键；语义未验证，原样透传）。
    pub did_checked_in: Option<bool>,
    /// CN 特有：附加积分（探针报告顶层键；语义未验证，原样透传）。
    pub extra_credits: Option<i64>,
    /// 服务端消息（原样保留；展示层自行取舍）。
    pub message: String,
    /// 业务码（解析成功即 code=0；非 0 走 Business 错误路径）。
    pub business_code: i64,
}

/// claim 响应 DTO（按 Work claim 响应族 + CN status 多出字段推断；
/// 形状**未经真机验证**——P13-4 验收可能需调整）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TraeCnCheckinClaim {
    /// 领取后的签到积分（Work claim 响应族同名字段）。
    pub credits: Option<i64>,
    /// CN 特有：附加积分（按 status 字段族推断；缺席归 None）。
    pub extra_credits: Option<i64>,
    /// 领取后的已签状态（按 status 字段族推断；缺席归 None）。
    pub checked_in: Option<bool>,
    /// 服务端消息（原样保留）。
    pub message: String,
    /// 业务码（解析成功即 code=0）。
    pub business_code: i64,
}

// ============================================================================
// envelope 解析（业务码不压平）
// ============================================================================

/// 从响应体提取业务码：顶层 `code` 字段，数字/字符串双形态兼容
/// （对齐 checkin_probe `extract_business_code` 的实测口径）。
fn envelope_business_code(envelope: &Value) -> Option<i64> {
    let code = envelope.get("code")?;
    code.as_i64()
        .or_else(|| code.as_str().and_then(|text| text.parse().ok()))
}

/// 解析 status envelope：业务码非 0 → `Business`（透传不压平）；
/// 无 code 字段 → `Protocol`；可选字段缺失降级为 None/默认值。
pub fn parse_status_envelope(envelope: &Value) -> Result<TraeCnCheckinStatus, TraeCnCheckinError> {
    let code = envelope_business_code(envelope).ok_or(TraeCnCheckinError::Protocol)?;
    if code != 0 {
        return Err(TraeCnCheckinError::Business(code));
    }
    Ok(TraeCnCheckinStatus {
        enable: envelope
            .get("enable")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        checked_in: envelope
            .get("checked_in")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        credits: envelope.get("credits").and_then(Value::as_i64),
        did_checked_in: envelope.get("did_checked_in").and_then(Value::as_bool),
        extra_credits: envelope.get("extra_credits").and_then(Value::as_i64),
        message: envelope
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        business_code: code,
    })
}

/// 解析 claim envelope（推断形态，宽松解析：可选字段缺失归 None）。
pub fn parse_claim_envelope(envelope: &Value) -> Result<TraeCnCheckinClaim, TraeCnCheckinError> {
    let code = envelope_business_code(envelope).ok_or(TraeCnCheckinError::Protocol)?;
    if code != 0 {
        return Err(TraeCnCheckinError::Business(code));
    }
    Ok(TraeCnCheckinClaim {
        credits: envelope.get("credits").and_then(Value::as_i64),
        extra_credits: envelope.get("extra_credits").and_then(Value::as_i64),
        checked_in: envelope.get("checked_in").and_then(Value::as_bool),
        message: envelope
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        business_code: code,
    })
}

// ============================================================================
// HTTP seam（生产 Reqwest 适配器 + 可注入边界）
// ============================================================================

/// CN 签到协议的 HTTP seam：发送请求描述并返回原始 JSON envelope。
/// 生产实现为下方 Reqwest 适配器；测试注入假实现固化请求形状与错误映射。
pub trait TraeCnCheckinHttp: Send + Sync {
    fn send(&self, request: TraeCnCheckinRequest) -> Result<Value, TraeCnCheckinError>;
}

/// 生产 HTTP 适配器：复用签到通道的直连客户端（15s 超时，`trae_http_client`）。
pub struct ReqwestTraeCnCheckinHttp {
    client: reqwest::blocking::Client,
}

impl ReqwestTraeCnCheckinHttp {
    pub fn new() -> Self {
        Self {
            client: trae_http_client(),
        }
    }
}

impl Default for ReqwestTraeCnCheckinHttp {
    fn default() -> Self {
        Self::new()
    }
}

impl TraeCnCheckinHttp for ReqwestTraeCnCheckinHttp {
    fn send(&self, request: TraeCnCheckinRequest) -> Result<Value, TraeCnCheckinError> {
        let method = reqwest::Method::from_bytes(request.method.as_bytes())
            .map_err(|_| TraeCnCheckinError::Protocol)?;
        let mut builder = self
            .client
            .request(method, format!("{API_BASE}{}", request.path));
        for (name, value) in &request.headers {
            builder = builder.header(*name, value);
        }
        let response = builder
            .body(request.body)
            .send()
            .map_err(|_| TraeCnCheckinError::Network)?;
        let status = response.status();
        let body_text = response
            .text()
            .map_err(|_| TraeCnCheckinError::Http(status.as_u16()))?;
        if !status.is_success() {
            // 非 200 先按体解析业务码（网关偶尔回 JSON 业务拒绝），
            // 取不到回退 HTTP 状态码（对齐 checkin_http `exchange_http_error` 口径）。
            if let Ok(envelope) = serde_json::from_str::<Value>(&body_text) {
                if let Some(code) = envelope_business_code(&envelope) {
                    return Err(TraeCnCheckinError::Business(code));
                }
            }
            return Err(TraeCnCheckinError::Http(status.as_u16()));
        }
        serde_json::from_str(&body_text).map_err(|_| TraeCnCheckinError::Protocol)
    }
}

// ============================================================================
// 端口端口层：TraeCnCheckinClient + Real 实现（凭据产品闸）
// ============================================================================

/// Trae CN 签到能力端口：单账号只读状态查询与手动领取。
/// 一次 claim = 一次用户授权动作（用户点击触发），无自动/批量路径。
pub trait TraeCnCheckinClient: Send + Sync {
    /// 查询账号签到状态（只读直连）。
    fn status(&self, profile_id: &str) -> Result<TraeCnCheckinStatus, TraeCnCheckinError>;

    /// 发起一次 claim（用户单次确认后触发）。
    fn claim(&self, profile_id: &str) -> Result<TraeCnCheckinClaim, TraeCnCheckinError>;
}

/// 真实 CN 签到客户端：解密本机凭据包获取 CN token 后直连服务端。
///
/// 自持有凭据仓库（路径克隆）+ 绑定单一凭据档案 + 期望 OAuth 客户端形态
/// （对齐 `RealRemoteDeviceManager`：产品适配器提供凭据上下文，协议实现
/// 保持产品中立；跨产品串用凭据直接拒绝触网）。
pub struct RealTraeCnCheckinClient {
    store: CheckinCredentialStore,
    binding: CheckinProfileBinding,
    expected_client: OAuthClient,
    http: Box<dyn TraeCnCheckinHttp>,
}

impl RealTraeCnCheckinClient {
    /// `material_root` 为 DPAPI 凭据包所在目录；生产 HTTP 适配器在此创建。
    pub fn new(
        material_root: impl Into<PathBuf>,
        binding: CheckinProfileBinding,
        expected_client: OAuthClient,
    ) -> Self {
        Self::with_http(
            material_root,
            binding,
            expected_client,
            Box::new(ReqwestTraeCnCheckinHttp::new()),
        )
    }

    /// 测试注入：用 fake HTTP seam 替换真实网络边界。
    pub fn with_http(
        material_root: impl Into<PathBuf>,
        binding: CheckinProfileBinding,
        expected_client: OAuthClient,
        http: Box<dyn TraeCnCheckinHttp>,
    ) -> Self {
        Self {
            store: CheckinCredentialStore::new(material_root),
            binding,
            expected_client,
            http,
        }
    }

    /// 加载并校验凭据包：profile 定位、解密、产品 client 一致性三道闸，
    /// 任一失败都不产生网络请求。
    fn credential_bundle(
        &self,
        profile_id: &str,
    ) -> Result<CheckinCredentialBundle, TraeCnCheckinError> {
        if profile_id != self.binding.profile_id {
            // 端口按 profile_id 定位账号；实例构造时绑定单一凭据档案，
            // 不一致属于凭据上下文错误（防御分支，与 load 的绑定校验互补）。
            return Err(TraeCnCheckinError::Credential("profile_mismatch"));
        }
        let bundle = self
            .store
            .load(&self.binding)
            .map_err(credential_load_error)?;
        // 凭据包的 client_id 必须与期望 OAuth 客户端一致：跨产品串用
        // 凭据（如 Work CN 凭据配 Trae CN client）直接拒绝触网。
        if bundle.client_id != self.expected_client.client_id() {
            return Err(TraeCnCheckinError::Credential("client_mismatch"));
        }
        Ok(bundle)
    }
}

/// 凭据包读取失败的稳定原因码映射（错误不含敏感材料）。
fn credential_load_error(error: CheckinCredentialError) -> TraeCnCheckinError {
    let reason = match error {
        CheckinCredentialError::Unavailable => "credential_unavailable",
        CheckinCredentialError::Missing => "credential_missing",
        CheckinCredentialError::Invalid => "credential_invalid",
        CheckinCredentialError::BindingMismatch => "credential_binding_mismatch",
        CheckinCredentialError::RecoveryRequired => "credential_recovery_required",
        // load 路径不会出现续期/身份校验错误，防御性归入凭据无效。
        CheckinCredentialError::CredentialRefreshFailed | CheckinCredentialError::AuthMismatch => {
            "credential_invalid"
        }
    };
    TraeCnCheckinError::Credential(reason)
}

impl TraeCnCheckinClient for RealTraeCnCheckinClient {
    fn status(&self, profile_id: &str) -> Result<TraeCnCheckinStatus, TraeCnCheckinError> {
        let bundle = self.credential_bundle(profile_id)?;
        let envelope = self
            .http
            .send(status_request(&bundle.access_token, &bundle.device_id))?;
        parse_status_envelope(&envelope)
    }

    fn claim(&self, profile_id: &str) -> Result<TraeCnCheckinClaim, TraeCnCheckinError> {
        let bundle = self.credential_bundle(profile_id)?;
        let envelope = self
            .http
            .send(claim_request(&bundle.access_token, &bundle.device_id))?;
        parse_claim_envelope(&envelope)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // ==================== 请求形状（探针实测形态固化） ====================

    /// status 请求形状：探针实测通过的唯一形状（2026-09-28 报告）——
    /// POST + Cloud-IDE-JWT 认证头 + x-device-id + CN 最小标准头 + `{}` 体。
    #[test]
    fn status_request_shape_matches_probe_evidence() {
        let request = status_request("token-1", "device-1");
        assert_eq!(request.method, "POST");
        assert_eq!(request.path, "/trae/api/v2/ug/checkin_credits/status");
        assert_eq!(request.body, "{}");
        let header = |name: &str| {
            request
                .headers
                .iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| value.clone())
        };
        assert_eq!(
            header("Authorization").as_deref(),
            Some("Cloud-IDE-JWT token-1")
        );
        assert_eq!(header("x-device-id").as_deref(), Some("device-1"));
        assert_eq!(header("Content-Type").as_deref(), Some("application/json"));
        assert_eq!(header("x-user-region").as_deref(), Some("CN"));
        assert_eq!(header("package-type").as_deref(), Some("stable_cn"));
        assert!(header("User-Agent").unwrap_or_default().contains("Trae CN"));
        assert!(header("app-version").is_some_and(|value| !value.is_empty()));
    }

    /// claim 请求：按 Work 同族推导（同前缀 + /claim；探针红线未实测 claim，
    /// 形状未经真机验证——P13-4 三步验收可能需调整，推导依据见模块头注释）。
    #[test]
    fn claim_request_uses_work_family_path_derivation() {
        let request = claim_request("token-1", "device-1");
        assert_eq!(request.method, "POST");
        assert_eq!(request.path, "/trae/api/v2/ug/checkin_credits/claim");
        assert_eq!(request.body, "{}");
        // 认证/设备头与 status 同族（探针验证的 status 形状直接外推）。
        let header = |name: &str| {
            request
                .headers
                .iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| value.clone())
        };
        assert_eq!(
            header("Authorization").as_deref(),
            Some("Cloud-IDE-JWT token-1")
        );
        assert_eq!(header("x-device-id").as_deref(), Some("device-1"));
    }

    // ==================== status 解析（探针实测字段全集） ====================

    /// 探针报告顶层键全集：checked_in/code/credits/did_checked_in/enable/
    /// extra_credits/message——字段逐一如实用值解析。
    #[test]
    fn status_envelope_parses_probe_field_set() {
        let envelope = json!({
            "checked_in": false,
            "code": 0,
            "credits": 200,
            "did_checked_in": true,
            "enable": true,
            "extra_credits": 5,
            "message": "success",
        });
        let status = parse_status_envelope(&envelope).unwrap();
        assert!(status.enable);
        assert!(!status.checked_in);
        assert_eq!(status.credits, Some(200));
        assert_eq!(status.did_checked_in, Some(true));
        assert_eq!(status.extra_credits, Some(5));
        assert_eq!(status.message, "success");
        assert_eq!(status.business_code, 0);
    }

    /// 字段缺失降级：CN 较 Work 多出的字段在部分响应形态下可能缺席，
    /// 缺失归一 None，不阻断解析（Work checked_in/credits/enable 同构）。
    #[test]
    fn status_envelope_defaults_missing_optional_fields() {
        let envelope = json!({
            "code": 0,
            "enable": true,
            "checked_in": false,
            "credits": 200,
            "message": "success",
        });
        let status = parse_status_envelope(&envelope).unwrap();
        assert_eq!(status.did_checked_in, None);
        assert_eq!(status.extra_credits, None);
        assert_eq!(status.business_code, 0);
    }

    /// 业务码不压平（红线）：HTTP 200 + code!=0 → Business(code) 原样透传，
    /// 数字与字符串双形态兼容（对齐 checkin_probe 提取口径）。
    #[test]
    fn status_envelope_transparent_business_code_not_flattened() {
        let numeric = json!({"code": 9095, "message": "quota"});
        assert_eq!(
            parse_status_envelope(&numeric).unwrap_err(),
            TraeCnCheckinError::Business(9095)
        );
        let string_form = json!({"code": "20401", "message": "limit"});
        assert_eq!(
            parse_status_envelope(&string_form).unwrap_err(),
            TraeCnCheckinError::Business(20401)
        );
    }

    /// 无 code 字段：结构不符合协议预期 → Protocol。
    #[test]
    fn status_envelope_missing_code_is_protocol_error() {
        let envelope = json!({"checked_in": false, "enable": true});
        assert_eq!(
            parse_status_envelope(&envelope).unwrap_err(),
            TraeCnCheckinError::Protocol
        );
    }

    // ==================== claim 解析（Work 响应族 + CN 字段推断，未验证） ====================

    /// claim 成功形态（推断）：code=0 + credits/extra_credits；checked_in 缺席
    /// 归 None。推断依据：Work claim 复用 status 响应族（checkin_http.rs），
    /// CN status 多出 did_checked_in/extra_credits——真机验收前不作强约束。
    #[test]
    fn claim_envelope_parses_inferred_success_shape() {
        let envelope = json!({
            "code": 0,
            "credits": 205,
            "extra_credits": 5,
            "message": "success",
        });
        let claim = parse_claim_envelope(&envelope).unwrap();
        assert_eq!(claim.credits, Some(205));
        assert_eq!(claim.extra_credits, Some(5));
        assert_eq!(claim.checked_in, None);
        assert_eq!(claim.business_code, 0);
    }

    /// claim 业务拒绝：code!=0 原样透传（如设备/资格类拒绝码）。
    #[test]
    fn claim_envelope_rejects_business_code() {
        let envelope = json!({"code": 9074, "message": "device"});
        assert_eq!(
            parse_claim_envelope(&envelope).unwrap_err(),
            TraeCnCheckinError::Business(9074)
        );
    }

    // ==================== 错误展示（稳定 snake_case 原因码） ====================

    /// 错误 Display：稳定 snake_case 原因码，供命令层映射与前端词表匹配；
    /// 不携带 Token 或响应正文。
    #[test]
    fn error_display_uses_stable_snake_case_codes() {
        assert_eq!(
            TraeCnCheckinError::Network.to_string(),
            "trae_cn_checkin_network"
        );
        assert_eq!(
            TraeCnCheckinError::Http(502).to_string(),
            "trae_cn_checkin_http_502"
        );
        assert_eq!(
            TraeCnCheckinError::Business(9095).to_string(),
            "trae_cn_checkin_business_9095"
        );
        assert_eq!(
            TraeCnCheckinError::Protocol.to_string(),
            "trae_cn_checkin_protocol"
        );
        assert_eq!(
            TraeCnCheckinError::Credential("client_mismatch").to_string(),
            "trae_cn_checkin_credential_client_mismatch"
        );
    }

    // ==================== 客户端编排（fake HTTP seam；凭据仓库 Windows 专属） ====================

    #[cfg(windows)]
    mod with_credentials {
        use super::*;
        use base64::Engine;
        use std::sync::{Arc, Mutex};

        const PROFILE_ID: &str = "profile-cn-checkin";
        const DEVICE_ID: &str = "1234567890123456";

        /// fake HTTP seam：记录收到的请求描述（供路径/头/体形状断言），
        /// 返回预置结果。这是本模块的 mock seam 边界。
        struct FakeCheckinHttp {
            requests: Arc<Mutex<Vec<TraeCnCheckinRequest>>>,
            result: Result<serde_json::Value, TraeCnCheckinError>,
        }

        impl TraeCnCheckinHttp for FakeCheckinHttp {
            fn send(
                &self,
                request: TraeCnCheckinRequest,
            ) -> Result<serde_json::Value, TraeCnCheckinError> {
                self.requests.lock().unwrap().push(request);
                self.result.clone()
            }
        }

        struct Fixture {
            _root: tempfile::TempDir,
            client: RealTraeCnCheckinClient,
            requests: Arc<Mutex<Vec<TraeCnCheckinRequest>>>,
            access_token: String,
        }

        /// 构造内存凭据现场（DPAPI 加密落盘临时目录）+ CN client 形态 +
        /// 注入 fake HTTP。对照负例用 `client_id_override`。
        fn fixture_client(
            result: Result<serde_json::Value, TraeCnCheckinError>,
            client_id_override: Option<&str>,
        ) -> Fixture {
            let root = tempfile::tempdir().unwrap();
            let store = CheckinCredentialStore::new(root.path());
            let (private_key, public_key) =
                crate::checkin_credential::generate_device_keypair().unwrap();
            let now = 1_800_000_000u64;
            let access_token = test_jwt("account-cn", now + 14 * 24 * 60 * 60);
            let bundle = CheckinCredentialBundle {
                profile_id: PROFILE_ID.to_string(),
                account_id: "account-cn".to_string(),
                device_id: DEVICE_ID.to_string(),
                machine_id: "machine-cn".to_string(),
                device_public_key: public_key.clone(),
                device_private_key: private_key,
                access_token: access_token.clone(),
                refresh_token: "refresh-cn".to_string(),
                client_id: client_id_override
                    .map(str::to_string)
                    .unwrap_or_else(|| OAuthClient::TraeCn.client_id().to_string()),
                access_token_expires_at_unix_seconds: now + 14 * 24 * 60 * 60,
                refresh_token_expires_at_unix_seconds: now + 180 * 24 * 60 * 60,
                mobile_full: None,
            };
            store.save(&bundle).unwrap();
            let binding =
                CheckinProfileBinding::new(PROFILE_ID, "account-cn", DEVICE_ID, &public_key);
            let requests = Arc::new(Mutex::new(Vec::new()));
            let client = RealTraeCnCheckinClient::with_http(
                root.path(),
                binding,
                OAuthClient::TraeCn,
                Box::new(FakeCheckinHttp {
                    requests: Arc::clone(&requests),
                    result,
                }),
            );
            Fixture {
                _root: root,
                client,
                requests,
                access_token,
            }
        }

        /// 测试用 JWT 形态 access token（与 checkin_http.rs 测试同形态）。
        fn test_jwt(account_id: &str, expires_at: u64) -> String {
            let payload = json!({
                "data": {"id": account_id},
                "exp": expires_at,
            });
            let encoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
                .encode(serde_json::to_vec(&payload).unwrap());
            format!("header.{encoded}.signature")
        }

        #[test]
        fn client_status_sends_credential_token_and_device_and_parses() {
            let fixture = fixture_client(
                Ok(json!({
                    "code": 0, "enable": true, "checked_in": false,
                    "credits": 200, "did_checked_in": true, "extra_credits": 5,
                    "message": "success",
                })),
                None,
            );
            let status = fixture.client.status(PROFILE_ID).unwrap();
            assert!(status.enable);
            assert_eq!(status.credits, Some(200));
            // 请求形状：凭据包的 access token 与 device_id 必须进入请求头。
            let requests = fixture.requests.lock().unwrap();
            assert_eq!(requests.len(), 1);
            assert_eq!(requests[0].path, "/trae/api/v2/ug/checkin_credits/status");
            assert_eq!(
                requests[0]
                    .headers
                    .iter()
                    .find(|(key, _)| *key == "Authorization")
                    .map(|(_, value)| value.clone()),
                Some(format!("Cloud-IDE-JWT {}", fixture.access_token))
            );
            assert_eq!(
                requests[0]
                    .headers
                    .iter()
                    .find(|(key, _)| *key == "x-device-id")
                    .map(|(_, value)| value.clone()),
                Some(DEVICE_ID.to_string())
            );
            assert_eq!(requests[0].body, "{}");
        }

        #[test]
        fn client_claim_success_and_business_failure_paths() {
            let ok = fixture_client(
                Ok(json!({"code": 0, "credits": 205, "extra_credits": 5, "message": "success"})),
                None,
            );
            let claim = ok.client.claim(PROFILE_ID).unwrap();
            assert_eq!(claim.credits, Some(205));
            assert_eq!(
                ok.requests.lock().unwrap()[0].path,
                "/trae/api/v2/ug/checkin_credits/claim"
            );

            let rejected = fixture_client(Ok(json!({"code": 9074, "message": "device"})), None);
            assert_eq!(
                rejected.client.claim(PROFILE_ID).unwrap_err(),
                TraeCnCheckinError::Business(9074)
            );
        }

        #[test]
        fn client_propagates_transport_errors() {
            let network = fixture_client(Err(TraeCnCheckinError::Network), None);
            assert_eq!(
                network.client.status(PROFILE_ID).unwrap_err(),
                TraeCnCheckinError::Network
            );
            let http = fixture_client(Err(TraeCnCheckinError::Http(500)), None);
            assert_eq!(
                http.client.claim(PROFILE_ID).unwrap_err(),
                TraeCnCheckinError::Http(500)
            );
        }

        #[test]
        fn client_rejects_cross_product_credential_without_http() {
            // 凭据包是 Work SOLO client（跨产品串用）→ 拒绝触网，零 HTTP 调用。
            let fixture = fixture_client(
                Ok(json!({"code": 0})),
                Some(crate::checkin_http::TRAE_SOLO_CLIENT_ID),
            );
            assert_eq!(
                fixture.client.status(PROFILE_ID).unwrap_err(),
                TraeCnCheckinError::Credential("client_mismatch")
            );
            assert!(fixture.requests.lock().unwrap().is_empty());
        }

        #[test]
        fn client_rejects_profile_mismatch_without_http() {
            let fixture = fixture_client(Ok(json!({"code": 0})), None);
            assert_eq!(
                fixture.client.status("profile-other").unwrap_err(),
                TraeCnCheckinError::Credential("profile_mismatch")
            );
            assert!(fixture.requests.lock().unwrap().is_empty());
        }
    }
}
