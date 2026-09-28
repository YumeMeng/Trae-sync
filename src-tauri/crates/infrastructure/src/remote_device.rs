//! 账号级远程设备管理协议实现（ADR-0031）。
//!
//! 设备列表与退出是账号级共享能力，独立于签到通道（`checkin_http.rs`
//! 保持签到专用，本模块只引用其客户端与 OAuth 形态，不改动其逻辑）。
//!
//! 协议事实（2026-09-23，官方授权页前端脚本逆向 + 本地凭据只读探针，
//! 见 `.scratch/research-trae-device-management-20260923.md` 与
//! `.scratch/checkin-http/reports/device-list-token-probe-20260923.json`）：
//! - 设备列表：`POST {API_BASE}/cloudide/api/v3/trae/oauth/ListDevices`，
//!   官方页调用体为空对象 `{}`，鉴权头 `x-cloudide-token: <access_token>`；
//!   本机两组 Trae CN 凭据实测 HTTP 200（占用 7/10、4/10）。响应字段：
//!   `Result.Devices[]`（DeviceID/DeviceType/DeviceName/BoundProducts[]/
//!   LastActiveAt）与 `Result.MaxDeviceCount`。
//! - 退出设备：`POST {API_BASE}/trae/api/v3/oauth/ClearRefreshToken`，
//!   官方页调用体只传 `{ClientID, DeviceID}`（RefreshToken/
//!   ClearCurrentDevice 不传）；该调用会真实改变服务端状态，只能由用户
//!   单次确认后触发，禁止任何自动或批量路径。
//! - 业务码在 `ResponseMetadata.Error.Code`，服务端实际返回字符串形态
//!   （"20401"），数字形态仅为兼容保留——对齐 `checkin_http.rs`
//!   `exchange_error_code` 的 2026-09-02 实测事实，本模块自写解析、
//!   不改动 checkin_http.rs 现有逻辑。
//!
//! 日志纪律：本模块不产生任何日志输出；错误类型不携带 Token、
//! refresh token 或完整 device_id。

use std::path::PathBuf;

use serde_json::Value;
use traesync_ports::{
    RemoteDeviceEntry, RemoteDeviceError, RemoteDeviceManager, RemoteDeviceSnapshot,
};

use crate::checkin_credential::{
    CheckinCredentialBundle, CheckinCredentialError, CheckinCredentialStore, CheckinProfileBinding,
};
use crate::checkin_http::{trae_http_client, OAuthClient};

const API_BASE: &str = "https://api.trae.cn";
const LIST_DEVICES_PATH: &str = "/cloudide/api/v3/trae/oauth/ListDevices";
const CLEAR_REFRESH_TOKEN_PATH: &str = "/trae/api/v3/oauth/ClearRefreshToken";

/// 远程设备协议所需的最小 HTTP 边界。
///
/// 生产实现使用下方 reqwest 适配器；边界允许测试在不触碰真实服务端的
/// 情况下固化请求形状与错误映射（模式对齐 checkin_http.rs 的
/// `CredentialRenewalHttpAdapter`）。
pub trait RemoteDeviceHttpAdapter: Send + Sync {
    /// 拉取设备列表原始 JSON envelope（业务码检查由 manager 负责）。
    ///
    /// `client_id`/`device_id` 进请求体：服务端需要二者标记 CurrentDevice
    /// 行（2026-09-23 对照实测，见实现处注释）。
    fn list_devices(
        &self,
        cloudide_token: &str,
        client_id: &str,
        device_id: &str,
    ) -> Result<Value, RemoteDeviceError>;

    /// 退出指定设备；返回原始 JSON envelope 供 manager 检查业务码。
    fn clear_refresh_token(
        &self,
        cloudide_token: &str,
        client_id: &str,
        device_id: &str,
    ) -> Result<Value, RemoteDeviceError>;
}

/// 生产 HTTP 适配器：复用签到通道的直连客户端（15s 超时、rustls）。
pub struct ReqwestRemoteDeviceHttpAdapter {
    client: reqwest::blocking::Client,
}

impl ReqwestRemoteDeviceHttpAdapter {
    pub fn new() -> Self {
        Self {
            client: trae_http_client(),
        }
    }

    fn post_json(
        &self,
        path: &str,
        cloudide_token: &str,
        body: &Value,
    ) -> Result<Value, RemoteDeviceError> {
        let response = self
            .client
            .post(format!("{API_BASE}{path}"))
            .header("Content-Type", "application/json")
            // 协议事实：设备管理接口鉴权头是 `x-cloudide-token`（本机凭据
            // 只读探针实测 200）；对齐 checkin_http.rs get_pc_auth_code
            // 注释——普通业务头的 Authorization: Cloud-IDE-JWT 会被拒绝。
            .header("x-cloudide-token", cloudide_token)
            .json(body)
            .send()
            .map_err(|_| RemoteDeviceError::Network)?;
        let status = response.status();
        let body_text = response
            .text()
            .map_err(|_| RemoteDeviceError::Http(status.as_u16()))?;
        if !status.is_success() {
            // 非 200 先按体解析业务码（"20401" 字符串形态优先），
            // 无业务码回退 HTTP 状态码。
            return Err(http_status_error(status.as_u16(), &body_text));
        }
        serde_json::from_str(&body_text).map_err(|_| RemoteDeviceError::Protocol)
    }
}

impl Default for ReqwestRemoteDeviceHttpAdapter {
    fn default() -> Self {
        Self::new()
    }
}

impl RemoteDeviceHttpAdapter for ReqwestRemoteDeviceHttpAdapter {
    fn list_devices(
        &self,
        cloudide_token: &str,
        client_id: &str,
        device_id: &str,
    ) -> Result<Value, RemoteDeviceError> {
        // 协议事实（2026-09-23 对照实测）：空体（官方网页形态）与仅带
        // ClientID 时，响应各行 CurrentDevice 全为 false；请求体携带
        // ClientID + DeviceInfo.DeviceID（本机凭据设备指纹）后，服务端
        // 才把本机行标记为 CurrentDevice=true。DeviceInfo 其余字段实测非必需。
        self.post_json(
            LIST_DEVICES_PATH,
            cloudide_token,
            &serde_json::json!({
                "ClientID": client_id,
                "DeviceInfo": {"DeviceID": device_id},
            }),
        )
    }

    fn clear_refresh_token(
        &self,
        cloudide_token: &str,
        client_id: &str,
        device_id: &str,
    ) -> Result<Value, RemoteDeviceError> {
        // ClearRefreshToken 请求体只有 ClientID/DeviceID 两个字段
        // （官方页远程退出形态；RefreshToken/ClearCurrentDevice 不传）。
        let body = serde_json::json!({
            "ClientID": client_id,
            "DeviceID": device_id,
        });
        self.post_json(CLEAR_REFRESH_TOKEN_PATH, cloudide_token, &body)
    }
}

/// 真实远程设备管理器：解密本机凭据包获取 access token 后直连服务端。
///
/// 自持有凭据仓库（路径克隆）；每个实例绑定单一凭据档案与期望的
/// OAuth 客户端形态（ADR-0031 决策 7：产品适配器提供 OAuth client
/// 与凭据上下文，协议实现保持产品中立）。
pub struct RealRemoteDeviceManager {
    store: CheckinCredentialStore,
    binding: CheckinProfileBinding,
    expected_client: OAuthClient,
    http: Box<dyn RemoteDeviceHttpAdapter>,
}

impl RealRemoteDeviceManager {
    /// `material_root` 为 DPAPI 凭据包所在目录；生产 HTTP 适配器在此创建。
    pub fn new(
        material_root: impl Into<PathBuf>,
        binding: CheckinProfileBinding,
        expected_client: OAuthClient,
    ) -> Self {
        Self::with_http_adapter(
            material_root,
            binding,
            expected_client,
            Box::new(ReqwestRemoteDeviceHttpAdapter::new()),
        )
    }

    /// 测试注入：用 fake HTTP 适配器替换真实网络边界。
    pub fn with_http_adapter(
        material_root: impl Into<PathBuf>,
        binding: CheckinProfileBinding,
        expected_client: OAuthClient,
        http: Box<dyn RemoteDeviceHttpAdapter>,
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
    ) -> Result<CheckinCredentialBundle, RemoteDeviceError> {
        if profile_id != self.binding.profile_id {
            // 端口按 profile_id 定位账号；实例构造时绑定单一凭据档案，
            // 不一致属于凭据上下文错误（防御分支，与 load 的绑定校验互补）。
            return Err(RemoteDeviceError::Credential("profile_mismatch"));
        }
        let bundle = self
            .store
            .load(&self.binding)
            .map_err(credential_load_error)?;
        // 凭据包的 client_id 必须与期望 OAuth 客户端一致：跨产品串用
        // 凭据（如 Work CN 凭据配 Trae CN client）直接拒绝触网。
        if bundle.client_id != self.expected_client.client_id() {
            return Err(RemoteDeviceError::Credential("client_mismatch"));
        }
        Ok(bundle)
    }
}

/// 凭据包读取失败的稳定原因码映射（错误不含敏感材料）。
fn credential_load_error(error: CheckinCredentialError) -> RemoteDeviceError {
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
    RemoteDeviceError::Credential(reason)
}

impl RemoteDeviceManager for RealRemoteDeviceManager {
    fn list_devices(&self, profile_id: &str) -> Result<RemoteDeviceSnapshot, RemoteDeviceError> {
        let bundle = self.credential_bundle(profile_id)?;
        let envelope = self.http.list_devices(
            &bundle.access_token,
            &bundle.client_id,
            &bundle.device_id,
        )?;
        parse_device_snapshot(&envelope, &bundle.device_id)
    }

    fn clear_refresh_token(
        &self,
        profile_id: &str,
        target_device_id: &str,
    ) -> Result<(), RemoteDeviceError> {
        let bundle = self.credential_bundle(profile_id)?;
        // ADR-0031 决策 3 后端防线（第一道，本地体系兜底）：目标是本机
        // 当前凭据设备 ID 时拒绝，不发起任何网络请求。
        if target_device_id == bundle.device_id {
            return Err(RemoteDeviceError::LocalDeviceTargeted);
        }
        // 防线第二道（2026-09-23 修正）：服务端 DeviceID 与本地凭据
        // device_id 是两套标识体系，仅靠本地比对防不住真实本机行。退出前
        // 先拉一次只读列表，目标行带 CurrentDevice 标记即拒绝且不发起
        // 退出请求；目标不在列表中时交由服务端裁决。
        let envelope = self.http.list_devices(
            &bundle.access_token,
            &bundle.client_id,
            &bundle.device_id,
        )?;
        let snapshot = parse_device_snapshot(&envelope, &bundle.device_id)?;
        if let Some(row) = snapshot
            .devices
            .iter()
            .find(|device| device.device_id == target_device_id)
        {
            if row.is_local {
                return Err(RemoteDeviceError::LocalDeviceTargeted);
            }
        }
        let envelope =
            self.http
                .clear_refresh_token(&bundle.access_token, &bundle.client_id, target_device_id)?;
        // 退出响应：有 Error.Code 且非 0 → 业务失败；否则视为成功
        // （官方页退出后随即重拉设备列表，本模块不代拉）。
        if let Some(code) = envelope_error_code(&envelope) {
            if code != 0 {
                return Err(RemoteDeviceError::Business(code));
            }
        }
        Ok(())
    }
}

/// 从服务端 envelope 提取 `ResponseMetadata.Error.Code`。
///
/// 服务端实际把 Code 返回为 JSON 字符串（"20401"），数字形态仅为兼容
/// 保留——只认数字会吞掉 20401 等关键错误（2026-09-02 实测，解析顺序
/// 对齐 checkin_http.rs `exchange_error_code`）。
fn envelope_error_code(envelope: &Value) -> Option<i64> {
    envelope
        .get("ResponseMetadata")?
        .get("Error")?
        .get("Code")
        .and_then(|code| {
            code.as_i64()
                .or_else(|| code.as_str().and_then(|s| s.parse().ok()))
        })
}

/// 非 200 响应错误映射：优先取响应体业务码，取不到回退 HTTP 状态码
/// （对齐 checkin_http.rs `exchange_http_error`；网关 HTML 体解析失败
/// 自然落入 Http 分支）。
fn http_status_error(status: u16, body: &str) -> RemoteDeviceError {
    if let Ok(envelope) = serde_json::from_str::<Value>(body) {
        if let Some(code) = envelope_error_code(&envelope) {
            return RemoteDeviceError::Business(code);
        }
    }
    RemoteDeviceError::Http(status)
}

/// 解析 ListDevices envelope 为端口快照。
///
/// 字段降级策略（ADR-0031：实现必须保留字段兼容与失败降级）：
/// - 行内可选字段缺失 → None/空列表，不阻断整体列表；
/// - DeviceID 缺失或非字符串 → 该行跳过（无法定位与比对，不计数）；
/// - `Result` 或 `Result.Devices` 缺失/非数组 → Protocol（成功响应必有）。
fn parse_device_snapshot(
    envelope: &Value,
    local_device_id: &str,
) -> Result<RemoteDeviceSnapshot, RemoteDeviceError> {
    // 业务码非 0（"20401" 字符串或数字形态）优先于结构解析。
    if let Some(code) = envelope_error_code(envelope) {
        if code != 0 {
            return Err(RemoteDeviceError::Business(code));
        }
    }
    let result = envelope.get("Result").ok_or(RemoteDeviceError::Protocol)?;
    let rows = result
        .get("Devices")
        .and_then(Value::as_array)
        .ok_or(RemoteDeviceError::Protocol)?;
    let devices: Vec<RemoteDeviceEntry> = rows
        .iter()
        .filter_map(|row| {
            let device_id = row.get("DeviceID").and_then(Value::as_str)?;
            // 本机标记（2026-09-23 实测修正）：服务端设备行自带 CurrentDevice
            // 标记，优先采用——服务端 DeviceID（14 位记录 ID）与本地凭据
            // device_id（16 位 hex 虚拟设备指纹）是两套标识体系，逐行相等
            // 永不命中。device_id 相等保留为兜底（服务端未来若回显客户端
            // ID 仍能命中）。宽松真值：true / "true" / 1 / "1"。
            let current_flag = row
                .get("CurrentDevice")
                .map(|value| match value {
                    Value::Bool(flag) => *flag,
                    Value::Number(number) => number.as_i64() == Some(1),
                    Value::String(text) => text == "true" || text == "1",
                    _ => false,
                })
                .unwrap_or(false);
            Some(RemoteDeviceEntry {
                device_id: device_id.to_string(),
                device_type: optional_string(row, "DeviceType"),
                device_name: optional_string(row, "DeviceName"),
                bound_products: string_list(row, "BoundProducts"),
                // LastActiveAt 字符串/数字双形态原样保留（字符串
                // "2026-09-23 10:00:00" 与毫秒时间戳均可能出现），
                // JSON null 与字段缺失同样归一为 None，格式化交给 UI 层。
                last_active_at: row
                    .get("LastActiveAt")
                    .cloned()
                    .filter(|value| !value.is_null()),
                is_local: current_flag || device_id == local_device_id,
            })
        })
        .collect();
    let max_count = result
        .get("MaxDeviceCount")
        .and_then(Value::as_u64)
        .map(|count| u32::try_from(count).unwrap_or(u32::MAX));
    Ok(RemoteDeviceSnapshot {
        used_count: devices.len(),
        max_count,
        devices,
    })
}

/// 行内可选字符串字段；缺失或非字符串归一为 None。
fn optional_string(row: &Value, key: &str) -> Option<String> {
    row.get(key).and_then(Value::as_str).map(str::to_string)
}

/// 行内字符串数组字段；缺失、非数组或含非字符串元素时忽略对应项。
fn string_list(row: &Value, key: &str) -> Vec<String> {
    row.get(key)
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(all(test, windows))]
mod tests {
    use std::sync::{Arc, Mutex};

    use base64::Engine;
    use serde_json::json;
    use traesync_ports::RemoteDeviceManager;

    use super::*;

    const PROFILE_ID: &str = "profile-device";
    const LOCAL_DEVICE_ID: &str = "1234567890123456";

    /// fake HTTP 适配器：记录调用顺序与请求形状，返回预置结果。
    struct FakeRemoteDeviceHttpAdapter {
        calls: Arc<Mutex<Vec<&'static str>>>,
        /// list 请求收到的 (client_id, device_id) 记录，供请求形状断言。
        list_requests: Arc<Mutex<Vec<(String, String)>>>,
        /// clear 请求收到的 (client_id, device_id) 记录，供请求形状断言。
        clear_requests: Arc<Mutex<Vec<(String, String)>>>,
        list_result: Result<Value, RemoteDeviceError>,
        clear_result: Result<Value, RemoteDeviceError>,
    }

    impl RemoteDeviceHttpAdapter for FakeRemoteDeviceHttpAdapter {
        fn list_devices(
            &self,
            _cloudide_token: &str,
            client_id: &str,
            device_id: &str,
        ) -> Result<Value, RemoteDeviceError> {
            self.list_requests
                .lock()
                .unwrap()
                .push((client_id.to_string(), device_id.to_string()));
            self.calls.lock().unwrap().push("list_devices");
            self.list_result.clone()
        }

        fn clear_refresh_token(
            &self,
            _cloudide_token: &str,
            client_id: &str,
            device_id: &str,
        ) -> Result<Value, RemoteDeviceError> {
            self.clear_requests
                .lock()
                .unwrap()
                .push((client_id.to_string(), device_id.to_string()));
            self.calls.lock().unwrap().push("clear_refresh_token");
            self.clear_result.clone()
        }
    }

    struct Fixture {
        // 保活临时目录，凭据包在其中。
        _root: tempfile::TempDir,
        manager: RealRemoteDeviceManager,
        calls: Arc<Mutex<Vec<&'static str>>>,
        list_requests: Arc<Mutex<Vec<(String, String)>>>,
        clear_requests: Arc<Mutex<Vec<(String, String)>>>,
        client_id: String,
    }

    /// 构造内存凭据现场（DPAPI 加密落盘到临时目录）+ 注入 fake 适配器。
    /// `client_id_override` 用于构造跨产品串用凭据的负例。
    fn fixture_manager(
        list_result: Result<Value, RemoteDeviceError>,
        clear_result: Result<Value, RemoteDeviceError>,
        client_id_override: Option<&str>,
        expected_client: OAuthClient,
    ) -> Fixture {
        let root = tempfile::tempdir().unwrap();
        let store = CheckinCredentialStore::new(root.path());
        let (private_key, public_key) = crate::checkin_credential::generate_device_keypair().unwrap();
        let now = 1_800_000_000u64;
        let client_id = client_id_override
            .map(str::to_string)
            .unwrap_or_else(|| expected_client.client_id().to_string());
        let bundle = CheckinCredentialBundle {
            profile_id: PROFILE_ID.to_string(),
            account_id: "account-device".to_string(),
            device_id: LOCAL_DEVICE_ID.to_string(),
            machine_id: "machine-device".to_string(),
            device_public_key: public_key.clone(),
            device_private_key: private_key,
            access_token: test_jwt("account-device", now + 14 * 24 * 60 * 60),
            refresh_token: "refresh-test".to_string(),
            client_id: client_id.clone(),
            access_token_expires_at_unix_seconds: now + 14 * 24 * 60 * 60,
            refresh_token_expires_at_unix_seconds: now + 180 * 24 * 60 * 60,
            mobile_full: None,
        };
        store.save(&bundle).unwrap();
        let binding = CheckinProfileBinding::new(
            PROFILE_ID,
            "account-device",
            LOCAL_DEVICE_ID,
            public_key,
        );
        let calls = Arc::new(Mutex::new(Vec::new()));
        let list_requests = Arc::new(Mutex::new(Vec::new()));
        let clear_requests = Arc::new(Mutex::new(Vec::new()));
        let adapter = FakeRemoteDeviceHttpAdapter {
            calls: Arc::clone(&calls),
            list_requests: Arc::clone(&list_requests),
            clear_requests: Arc::clone(&clear_requests),
            list_result,
            clear_result,
        };
        let manager = RealRemoteDeviceManager::with_http_adapter(
            root.path(),
            binding,
            expected_client,
            Box::new(adapter),
        );
        Fixture {
            _root: root,
            manager,
            calls,
            list_requests,
            clear_requests,
            client_id,
        }
    }

    /// 测试用 JWT 形态 access token（bundle 校验要求三段式；与
    /// checkin_http.rs 测试的 test_jwt 同形态，不触真实凭据文件）。
    fn test_jwt(account_id: &str, expires_at: u64) -> String {
        let payload = json!({
            "data": {"id": account_id},
            "exp": expires_at,
        });
        let encoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(serde_json::to_vec(&payload).unwrap());
        format!("header.{encoded}.signature")
    }

    /// 成功 ListDevices envelope 骨架。
    fn list_envelope(devices: Value, max_count: Value) -> Value {
        json!({
            "ResponseMetadata": {"Error": {"Code": 0}},
            "Result": {"Devices": devices, "MaxDeviceCount": max_count},
        })
    }

    #[test]
    fn list_devices_parses_full_fields_and_both_last_active_shapes() {
        let envelope = list_envelope(
            json!([
                {
                    "DeviceID": "dev-aaa",
                    "DeviceType": "IDE_PC",
                    "DeviceName": "办公电脑",
                    "BoundProducts": ["Trae CN", "TRAE SOLO", "TRAE 移动端"],
                    "LastActiveAt": "2026-09-23 10:00:00",
                },
                {
                    "DeviceID": "dev-bbb",
                    "DeviceType": "MOBILE",
                    "DeviceName": "手机",
                    "BoundProducts": ["TRAE 移动端"],
                    "LastActiveAt": 1727071200000i64,
                },
            ]),
            json!(10),
        );
        let fixture = fixture_manager(Ok(envelope), Ok(json!({})), None, OAuthClient::Solo);
        let snapshot = fixture.manager.list_devices(PROFILE_ID).unwrap();
        assert_eq!(snapshot.devices.len(), 2);
        let first = &snapshot.devices[0];
        assert_eq!(first.device_id, "dev-aaa");
        assert_eq!(first.device_type.as_deref(), Some("IDE_PC"));
        assert_eq!(first.device_name.as_deref(), Some("办公电脑"));
        assert_eq!(first.bound_products.len(), 3);
        assert_eq!(
            first.last_active_at,
            Some(json!("2026-09-23 10:00:00")),
            "LastActiveAt 字符串形态原样保留"
        );
        assert_eq!(
            snapshot.devices[1].last_active_at,
            Some(json!(1727071200000i64)),
            "LastActiveAt 数字形态原样保留"
        );
    }

    #[test]
    fn list_devices_skips_rows_without_device_id_and_defaults_missing_fields() {
        let envelope = list_envelope(
            json!([
                {"DeviceName": "无 ID 的降级行"},
                {"DeviceID": "dev-ccc"},
            ]),
            json!(10),
        );
        let fixture = fixture_manager(Ok(envelope), Ok(json!({})), None, OAuthClient::Solo);
        let snapshot = fixture.manager.list_devices(PROFILE_ID).unwrap();
        // DeviceID 缺失的行跳过（不进入 devices，也不计入 used_count）。
        assert_eq!(snapshot.devices.len(), 1);
        let only = &snapshot.devices[0];
        assert_eq!(only.device_id, "dev-ccc");
        // 可选字段缺失 → None / 空列表，不阻断整体列表。
        assert_eq!(only.device_name, None);
        assert_eq!(only.device_type, None);
        assert!(only.bound_products.is_empty());
        assert_eq!(only.last_active_at, None);
        assert_eq!(snapshot.used_count, 1);
    }

    #[test]
    fn list_devices_maps_business_code_from_string_and_number_forms() {
        // 服务端实际返回字符串形态 "20401"；数字形态仅为兼容保留。
        let string_form = json!({
            "ResponseMetadata": {"Error": {"Code": "20401"}},
        });
        let fixture = fixture_manager(Ok(string_form), Ok(json!({})), None, OAuthClient::Solo);
        assert_eq!(
            fixture.manager.list_devices(PROFILE_ID).unwrap_err(),
            RemoteDeviceError::Business(20401)
        );

        let number_form = json!({
            "ResponseMetadata": {"Error": {"Code": 20401}},
        });
        let fixture = fixture_manager(Ok(number_form), Ok(json!({})), None, OAuthClient::Solo);
        assert_eq!(
            fixture.manager.list_devices(PROFILE_ID).unwrap_err(),
            RemoteDeviceError::Business(20401)
        );
    }

    #[test]
    fn http_status_error_falls_back_to_status_without_business_code() {
        // 网关 HTML 体：解析不出业务码 → 回退 Http 状态码。
        assert_eq!(
            http_status_error(500, "<html>Internal Server Error</html>"),
            RemoteDeviceError::Http(500)
        );
        // JSON 体但无 Error.Code → 同样回退 Http 状态码。
        assert_eq!(
            http_status_error(500, r#"{"ResponseMetadata":{}}"#),
            RemoteDeviceError::Http(500)
        );
        // 有业务码时优先业务码（"20101" 字符串形态）。
        assert_eq!(
            http_status_error(401, r#"{"ResponseMetadata":{"Error":{"Code":"20101"}}}"#),
            RemoteDeviceError::Business(20101)
        );
    }

    #[test]
    fn list_devices_marks_local_device_row_by_device_id_match() {
        let envelope = list_envelope(
            json!([
                {"DeviceID": LOCAL_DEVICE_ID, "DeviceName": "本机"},
                {"DeviceID": "dev-other", "DeviceName": "其他设备"},
            ]),
            json!(10),
        );
        let fixture = fixture_manager(Ok(envelope), Ok(json!({})), None, OAuthClient::Solo);
        let snapshot = fixture.manager.list_devices(PROFILE_ID).unwrap();
        assert!(snapshot.devices[0].is_local, "本机凭据 device_id 命中的行标记为本机（兜底路径）");
        assert!(!snapshot.devices[1].is_local);
    }

    #[test]
    fn list_devices_prefers_server_current_device_flag_over_device_id_match() {
        // 实测事实（2026-09-23）：服务端 DeviceID 与本地凭据 device_id 是
        // 两套标识体系，真实环境靠行内 CurrentDevice 标记识别本机。
        let envelope = list_envelope(
            json!([
                // 服务端标记的本机行：DeviceID 与本地凭据不同，仍应命中。
                {"DeviceID": "srv-current", "CurrentDevice": true},
                // 其他行带显式 false。
                {"DeviceID": "srv-other", "CurrentDevice": false},
                // 无标记行 → false（不影响 device_id 兜底）。
                {"DeviceID": "srv-plain"},
            ]),
            json!(10),
        );
        let fixture = fixture_manager(Ok(envelope), Ok(json!({})), None, OAuthClient::Solo);
        let snapshot = fixture.manager.list_devices(PROFILE_ID).unwrap();
        assert!(snapshot.devices[0].is_local, "CurrentDevice=true 命中本机");
        assert!(!snapshot.devices[1].is_local);
        assert!(!snapshot.devices[2].is_local, "无标记且 device_id 不等 → 非本机");
    }

    #[test]
    fn list_devices_parses_current_device_flag_truthy_forms() {
        // 宽松真值：bool true / "true" / 1 / "1" 都命中；其余值不命中。
        let envelope = list_envelope(
            json!([
                {"DeviceID": "dev-a", "CurrentDevice": "true"},
                {"DeviceID": "dev-b", "CurrentDevice": 1},
                {"DeviceID": "dev-c", "CurrentDevice": "1"},
                {"DeviceID": "dev-d", "CurrentDevice": false},
                {"DeviceID": "dev-e", "CurrentDevice": "yes"},
                {"DeviceID": "dev-f", "CurrentDevice": 2},
            ]),
            json!(10),
        );
        let fixture = fixture_manager(Ok(envelope), Ok(json!({})), None, OAuthClient::Solo);
        let snapshot = fixture.manager.list_devices(PROFILE_ID).unwrap();
        let flags: Vec<bool> = snapshot.devices.iter().map(|d| d.is_local).collect();
        assert_eq!(flags, [true, true, true, false, false, false]);
    }

    #[test]
    fn list_devices_reports_used_and_max_count() {
        let envelope = list_envelope(
            json!([
                {"DeviceID": "dev-1"},
                {"DeviceID": "dev-2"},
                {"DeviceName": "缺 ID 行不计入已用"},
            ]),
            json!(10),
        );
        let fixture = fixture_manager(Ok(envelope), Ok(json!({})), None, OAuthClient::Solo);
        let snapshot = fixture.manager.list_devices(PROFILE_ID).unwrap();
        assert_eq!(snapshot.used_count, 2);
        assert_eq!(snapshot.max_count, Some(10));
    }

    #[test]
    fn list_devices_reports_unknown_max_count_when_field_missing() {
        let envelope = json!({
            "ResponseMetadata": {"Error": {"Code": 0}},
            "Result": {"Devices": [{"DeviceID": "dev-1"}]},
        });
        let fixture = fixture_manager(Ok(envelope), Ok(json!({})), None, OAuthClient::Solo);
        let snapshot = fixture.manager.list_devices(PROFILE_ID).unwrap();
        assert_eq!(snapshot.used_count, 1);
        assert_eq!(snapshot.max_count, None, "MaxDeviceCount 缺失降级为 None");
    }

    #[test]
    fn clear_refresh_token_sends_credential_client_id_and_target_device_id() {
        let fixture = fixture_manager(
            // list 前置校验返回非本机行：目标不在其中 → 放行。
            Ok(list_envelope(
                json!([{"DeviceID": "dev-remote-1", "CurrentDevice": false}]),
                json!(10),
            )),
            Ok(json!({"ResponseMetadata": {"Error": {"Code": 0}}})),
            None,
            OAuthClient::Solo,
        );
        fixture
            .manager
            .clear_refresh_token(PROFILE_ID, "dev-remote-1")
            .unwrap();
        // 请求形状固化：client_id 与凭据包一致，device_id 为用户选定的目标。
        let requests = fixture.clear_requests.lock().unwrap();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].0, fixture.client_id);
        assert_eq!(requests[0].1, "dev-remote-1");
        // 退出前的只读前置校验（CurrentDevice 防线）+ 退出本体。
        assert_eq!(
            *fixture.calls.lock().unwrap(),
            vec!["list_devices", "clear_refresh_token"]
        );
        // list 请求形状：必须携带凭据的 ClientID 与设备指纹（服务端标记
        // CurrentDevice 的前置条件，2026-09-23 对照实测）。
        let list_requests = fixture.list_requests.lock().unwrap();
        assert_eq!(list_requests.len(), 1);
        assert_eq!(list_requests[0].0, fixture.client_id);
        assert_eq!(list_requests[0].1, LOCAL_DEVICE_ID);
    }

    #[test]
    fn clear_refresh_token_rejects_server_current_device_target() {
        // 防线第二道（真实标识体系）：目标行带服务端 CurrentDevice 标记
        // → 拒绝，且不发起退出请求（list 只读校验本身放行）。
        let fixture = fixture_manager(
            Ok(list_envelope(
                json!([
                    {"DeviceID": "srv-current", "CurrentDevice": true},
                    {"DeviceID": "srv-other", "CurrentDevice": false},
                ]),
                json!(10),
            )),
            Ok(json!({"ResponseMetadata": {"Error": {"Code": 0}}})),
            None,
            OAuthClient::Solo,
        );
        let error = fixture
            .manager
            .clear_refresh_token(PROFILE_ID, "srv-current")
            .unwrap_err();
        assert_eq!(error, RemoteDeviceError::LocalDeviceTargeted);
        assert_eq!(*fixture.calls.lock().unwrap(), vec!["list_devices"]);
        assert!(fixture.clear_requests.lock().unwrap().is_empty());
    }

    #[test]
    fn clear_refresh_token_rejects_local_target_without_http_call() {
        let fixture = fixture_manager(Ok(json!({})), Ok(json!({})), None, OAuthClient::Solo);
        let error = fixture
            .manager
            .clear_refresh_token(PROFILE_ID, LOCAL_DEVICE_ID)
            .unwrap_err();
        // ADR-0031 决策 3 后端防线：本机目标拒绝且零网络请求。
        assert_eq!(error, RemoteDeviceError::LocalDeviceTargeted);
        assert!(fixture.calls.lock().unwrap().is_empty());
        assert!(fixture.clear_requests.lock().unwrap().is_empty());
    }

    #[test]
    fn list_devices_rejects_client_id_mismatch_as_credential_error() {
        // 凭据包 client_id 与期望 OAuth 客户端不一致（跨产品串用）→ 拒绝触网。
        let fixture = fixture_manager(
            Ok(json!({})),
            Ok(json!({})),
            Some(crate::checkin_http::TRAE_CN_CLIENT_ID),
            OAuthClient::Solo,
        );
        let error = fixture.manager.list_devices(PROFILE_ID).unwrap_err();
        assert_eq!(error, RemoteDeviceError::Credential("client_mismatch"));
        assert!(fixture.calls.lock().unwrap().is_empty());
    }

    #[test]
    fn clear_refresh_token_propagates_business_code_from_response() {
        // 非 0 业务码透传（list 前置校验返回非本机行以放行）。
        let fixture = fixture_manager(
            Ok(list_envelope(
                json!([{"DeviceID": "dev-remote-1", "CurrentDevice": false}]),
                json!(10),
            )),
            Ok(json!({"ResponseMetadata": {"Error": {"Code": "20401"}}})),
            None,
            OAuthClient::Solo,
        );
        assert_eq!(
            fixture
                .manager
                .clear_refresh_token(PROFILE_ID, "dev-remote-1")
                .unwrap_err(),
            RemoteDeviceError::Business(20401)
        );
        // Error.Code = 0 视为成功（list 前置校验返回非本机行以放行）。
        let fixture = fixture_manager(
            Ok(list_envelope(
                json!([{"DeviceID": "dev-remote-1", "CurrentDevice": false}]),
                json!(10),
            )),
            Ok(json!({"ResponseMetadata": {"Error": {"Code": 0}}})),
            None,
            OAuthClient::Solo,
        );
        fixture
            .manager
            .clear_refresh_token(PROFILE_ID, "dev-remote-1")
            .unwrap();
    }
}
