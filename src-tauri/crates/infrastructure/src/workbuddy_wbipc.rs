//! P17-T6/A4：WorkBuddy 切号 RPC（wbipc 协议客户端 + 切号编排）。
//!
//! 协议事实源：spec 8.6（asar 实读 + T4 探针实测定稿）——
//! - **帧**：newline-JSON（`JSON.stringify(frame) + "\n"`；超限 protocol
//!   violation 断连）；
//! - **三态握手**：`session_hello`（protocol_min/max=1 + client_nonce
//!   base64url 16B + ticket_id = sha256(ticket utf8) 前 16 hex + client
//!   三元组）→ 服务端 `session_c*` challenge（serverNonce base64url 16B）→
//!   客户端 `session_prove`（client_proof = base64url(HMAC-SHA256(
//!   key=ticket utf8, encodeTranscript))，encodeTranscript = concat(LP4BE
//!   逐段 ["wbipc-c", "1", <endpoint 管道路径>, clientNonce, serverNonce])，
//!   nonce 段为解码后原始字节）→ `session_hello_ack`（protocol /
//!   connection_epoch / max_inflight / pipes 直接携带）；
//! - **RPC**：JSON-RPC 2.0（响应帧 `replyResult`/`replyError`；兼容标准
//!   `{result}`/`{error}` 形态——脱敏解析按形态自适应）。
//!
//! 切号编排（spec 8.4 写路径约束 + 8.8 多副本协同定案）：客户端**运行中**
//! 经 wbipc 依次 `auth:logout`（清当前会话）→ `auth:saveOauthToken`
//! （写池内最新代次——工具刷新后的最新对，客户端始终拿到最新凭据）→
//! `auth:getAccount`（回读 uid 交叉校验）。**对话跟号走天然成立**：
//! WorkBuddy 对话存 CodeBuddy 云端按账号隔离，切号=换登录态自动跟随，
//! 编排内不做任何对话同步（真机验证记录留证据）。
//!
//! 单次确认纪律：命令层由 UI 弹窗确认后调用（与捕获同构）；本模块不做
//! 二次确认，fail-closed 于任何协议/校验异常。
//!
//! 真机验证检查点（代码+单测完成后由主理人协调用户执行）：saveOauthToken
//! 参数对象的确切键形（按 .info 元数据 camelCase 形态构造，形态见
//! [`auth_object_from_record`]）与 getAccount 响应的 uid 取值路径。

use std::fs;
use std::path::{Path, PathBuf};

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use sha2::{Digest, Sha256};

use crate::workbuddy_login_credential::WorkBuddyLoginRecord;

/// 帧上限（1 MiB；超限按 protocol violation 断连——与客户端纪律对齐）。
const MAX_FRAME_BYTES: usize = 1024 * 1024;

/// 切号编排错误（错误码进 safeUiError 中心词表；不含任何令牌材料）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WbipcSwitchError {
    /// endpoint.json 不可读（WorkBuddy 未运行——broker 随主进程创建）。
    EndpointMissing,
    /// 管道连接失败（客户端未运行/管道已轮换）。
    ClientOffline,
    /// 握手失败（携带协议错误码或阶段标记）。
    HandshakeFailed(String),
    /// RPC 被服务端拒绝（携带方法与错误码）。
    RpcRejected { method: &'static str, code: String },
    /// 切号后回读账号与目标不符（宁可不报成功，不可错登）。
    VerifyFailed,
    /// 帧超限（protocol violation）。
    FrameTooLarge,
    /// 目标账号不在工具池（未捕获/已清理）。
    AccountMissing,
    /// 管道 IO 失败（读半途 EOF 等）。
    Io,
}

impl WbipcSwitchError {
    pub fn code(&self) -> String {
        match self {
            Self::EndpointMissing => "workbuddy_switch_endpoint_missing".to_string(),
            Self::ClientOffline => "workbuddy_switch_client_offline".to_string(),
            Self::HandshakeFailed(code) => {
                format!("workbuddy_switch_handshake_failed:{code}")
            }
            Self::RpcRejected { method, code } => {
                format!("workbuddy_switch_rpc_rejected:{method}:{code}")
            }
            Self::VerifyFailed => "workbuddy_switch_verify_failed".to_string(),
            Self::FrameTooLarge => "workbuddy_switch_frame_too_large".to_string(),
            Self::AccountMissing => "workbuddy_switch_account_missing".to_string(),
            Self::Io => "workbuddy_switch_io".to_string(),
        }
    }
}

/// wbipc 端点（`~/.workbuddy/wbipc/endpoint.json` 的 `{endpoint, ticket}`）。
/// ticket 随客户端每次启动轮换，值永不打印/落日志。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WbipcEndpoint {
    /// 命名管道路径（Windows `\\.\pipe\…`）。
    pub pipe_path: String,
    /// 32B base64url ticket。
    pub ticket: String,
}

/// 读取端点文件（默认 `~/.workbuddy/wbipc/endpoint.json`；给定根时
/// `{root}/wbipc/endpoint.json`——测试注入用）。
pub fn read_endpoint_in(workbuddy_root: &Path) -> Result<WbipcEndpoint, WbipcSwitchError> {
    let path = workbuddy_root.join("wbipc").join("endpoint.json");
    let raw = fs::read_to_string(&path).map_err(|_| WbipcSwitchError::EndpointMissing)?;
    parse_endpoint(&raw)
}

/// 默认端点路径（`~/.workbuddy/wbipc/endpoint.json`）。
pub fn default_endpoint_path() -> Option<PathBuf> {
    let profile = std::env::var("USERPROFILE").ok()?;
    Some(
        Path::new(&profile)
            .join(".workbuddy")
            .join("wbipc")
            .join("endpoint.json"),
    )
}

/// 解析端点 JSON（形态不符 → EndpointMissing——对调用方语义一致：无法
/// 建立切号通道）。
pub fn parse_endpoint(raw: &str) -> Result<WbipcEndpoint, WbipcSwitchError> {
    let value: serde_json::Value =
        serde_json::from_str(raw).map_err(|_| WbipcSwitchError::EndpointMissing)?;
    let endpoint = value
        .get("endpoint")
        .and_then(|v| v.as_str())
        .ok_or(WbipcSwitchError::EndpointMissing)?;
    let ticket = value
        .get("ticket")
        .and_then(|v| v.as_str())
        .ok_or(WbipcSwitchError::EndpointMissing)?;
    if endpoint.is_empty() || ticket.is_empty() {
        return Err(WbipcSwitchError::EndpointMissing);
    }
    Ok(WbipcEndpoint {
        pipe_path: endpoint.to_string(),
        ticket: ticket.to_string(),
    })
}

// ===== 编码原语（纯函数，单测含 RFC 4231 HMAC 向量与自定 HKDF 式定案）=====

/// base64url（无填充）编码。
pub fn b64url_encode(bytes: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(bytes)
}

/// base64url（无填充）解码。
pub fn b64url_decode(text: &str) -> Result<Vec<u8>, WbipcSwitchError> {
    URL_SAFE_NO_PAD
        .decode(text)
        .map_err(|_| WbipcSwitchError::HandshakeFailed("bad_nonce".to_string()))
}

/// ticket_id = sha256(ticket utf8) 前 16 hex（spec 8.6）。
pub fn ticket_id(ticket: &str) -> String {
    let digest = Sha256::digest(ticket.as_bytes());
    digest[..8].iter().map(|byte| format!("{byte:02x}")).collect()
}

/// HMAC-SHA256（RFC 2104 标准结构；sha2 已有，HMAC 手写避免新依赖——
/// 正确性由 RFC 4231 官方向量锁定）。
pub fn hmac_sha256(key: &[u8], data: &[u8]) -> [u8; 32] {
    const BLOCK: usize = 64;
    let mut normalized = [0_u8; BLOCK];
    if key.len() > BLOCK {
        let condensed = Sha256::digest(key);
        normalized[..32].copy_from_slice(&condensed);
    } else {
        normalized[..key.len()].copy_from_slice(key);
    }
    let mut ipad = [0x36_u8; BLOCK];
    let mut opad = [0x5c_u8; BLOCK];
    for index in 0..BLOCK {
        ipad[index] ^= normalized[index];
        opad[index] ^= normalized[index];
    }
    let mut inner = Sha256::new();
    inner.update(ipad);
    inner.update(data);
    let inner_digest = inner.finalize();
    let mut outer = Sha256::new();
    outer.update(opad);
    outer.update(inner_digest);
    outer.finalize().into()
}

/// LP4BE 段拼接（4 字节大端长度前缀逐段；spec 8.6 encodeTranscript 定案）。
pub fn lp4be(segments: &[&[u8]]) -> Vec<u8> {
    let mut out = Vec::new();
    for segment in segments {
        out.extend_from_slice(&(segment.len() as u32).to_be_bytes());
        out.extend_from_slice(segment);
    }
    out
}

/// client_proof = base64url(HMAC-SHA256(key=ticket utf8, encodeTranscript))；
/// encodeTranscript = concat(LP4BE ["wbipc-c", "1", <管道路径>, clientNonce,
/// serverNonce])——nonce 段为解码后原始字节（spec 8.6 实读定案）。
pub fn client_proof(
    ticket: &str,
    pipe_path: &str,
    client_nonce: &[u8],
    server_nonce: &[u8],
) -> String {
    let transcript = lp4be(&[
        b"wbipc-c",
        b"1",
        pipe_path.as_bytes(),
        client_nonce,
        server_nonce,
    ]);
    b64url_encode(&hmac_sha256(ticket.as_bytes(), &transcript))
}

/// 客户端 nonce：16 字节（时间 + 进程标识散列派生；nonce 只需会话级唯一，
/// 无需密码学随机——证明强度在 ticket HMAC 侧）。
pub fn generate_nonce() -> [u8; 16] {
    let material = format!(
        "{}-{}-{:?}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or_default(),
        std::thread::current().id()
    );
    let digest = Sha256::digest(material.as_bytes());
    digest[..16].try_into().unwrap_or([0_u8; 16])
}

// ===== 传输层（帧编解码；trait 注入使编排可离线单测）=====

/// wbipc 帧传输抽象（生产 = 命名管道；测试 = 脚本化回放）。
pub trait WbipcTransport {
    fn send_frame(&mut self, value: &serde_json::Value) -> Result<(), WbipcSwitchError>;
    fn read_frame(&mut self) -> Result<serde_json::Value, WbipcSwitchError>;
}

/// 生产传输：Windows 命名管道（`\\.\pipe\…` 经 std fs 直接打开，与探针
/// 同口径）。
pub struct PipeTransport {
    stream: fs::File,
}

impl PipeTransport {
    pub fn connect(pipe_path: &str) -> Result<Self, WbipcSwitchError> {
        let stream = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(pipe_path)
            .map_err(|_| WbipcSwitchError::ClientOffline)?;
        Ok(Self { stream })
    }
}

impl WbipcTransport for PipeTransport {
    fn send_frame(&mut self, value: &serde_json::Value) -> Result<(), WbipcSwitchError> {
        use std::io::Write;
        let mut frame = serde_json::to_vec(value).map_err(|_| WbipcSwitchError::Io)?;
        if frame.len() + 1 > MAX_FRAME_BYTES {
            return Err(WbipcSwitchError::FrameTooLarge);
        }
        frame.push(b'\n');
        self.stream
            .write_all(&frame)
            .and_then(|_| self.stream.flush())
            .map_err(|_| WbipcSwitchError::Io)
    }

    fn read_frame(&mut self) -> Result<serde_json::Value, WbipcSwitchError> {
        use std::io::Read;
        let mut buffer = Vec::new();
        let mut chunk = [0_u8; 4096];
        loop {
            let read = self.stream.read(&mut chunk).map_err(|_| WbipcSwitchError::Io)?;
            if read == 0 {
                return Err(WbipcSwitchError::Io);
            }
            buffer.extend_from_slice(&chunk[..read]);
            if buffer.len() > MAX_FRAME_BYTES {
                return Err(WbipcSwitchError::FrameTooLarge);
            }
            if buffer.ends_with(b"\n") {
                break;
            }
        }
        serde_json::from_slice(&buffer).map_err(|_| WbipcSwitchError::Io)
    }
}

/// 从 JSON-RPC error 对象提取稳定错误码（code 字符串优先，数值转串，
/// 回退 message）。
fn extract_error_code(error: &serde_json::Value) -> Option<String> {
    match error.get("code") {
        Some(serde_json::Value::String(text)) => Some(text.clone()),
        Some(other @ serde_json::Value::Number(_)) => Some(other.to_string()),
        _ => error
            .get("message")
            .and_then(|message| message.as_str())
            .map(str::to_string),
    }
}

// ===== 握手与 RPC（泛型传输，编排可注入脚本化对端离线测试）=====

/// 握手回执（pipes 由 ack 直接携带；connection_epoch 透传给调用方留痕）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HandshakeAck {
    pub connection_epoch: Option<u64>,
    pub pipes: Vec<String>,
}

/// 从帧提取 nonce：键形兼容 `server_nonce`/`serverNonce`（asar 实读命名
/// 与探针观测双形态防御）。
fn nonce_from(frame: &serde_json::Value, primary: &str, fallback: &str) -> Option<Vec<u8>> {
    frame
        .get(primary)
        .or_else(|| frame.get(fallback))
        .and_then(|value| value.as_str())
        .and_then(|text| b64url_decode(text).ok())
}

/// 三态握手（spec 8.6）：hello → challenge → prove → ack。
pub fn handshake(
    transport: &mut dyn WbipcTransport,
    endpoint: &WbipcEndpoint,
    client_id: &str,
) -> Result<HandshakeAck, WbipcSwitchError> {
    let client_nonce = generate_nonce();
    let hello = serde_json::json!({
        "type": "session_hello",
        "protocol_min": 1,
        "protocol_max": 1,
        "client_nonce": b64url_encode(&client_nonce),
        "ticket_id": ticket_id(&endpoint.ticket),
        "client": {
            "kind": "tool",
            "id": client_id,
            "version": env!("CARGO_PKG_VERSION"),
        },
    });
    transport.send_frame(&hello)?;

    let challenge = transport.read_frame()?;
    let challenge_type = challenge.get("type").and_then(|v| v.as_str()).unwrap_or("");
    if challenge_type == "session_hello_error" {
        let code = challenge
            .get("code")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .to_string();
        return Err(WbipcSwitchError::HandshakeFailed(code));
    }
    if !challenge_type.starts_with("session_c") {
        return Err(WbipcSwitchError::HandshakeFailed(format!(
            "unexpected_frame:{challenge_type}"
        )));
    }
    let Some(server_nonce) = nonce_from(&challenge, "server_nonce", "serverNonce") else {
        return Err(WbipcSwitchError::HandshakeFailed("missing_nonce".to_string()));
    };
    let prove = serde_json::json!({
        "type": "session_prove",
        "client_proof": client_proof(
            &endpoint.ticket,
            &endpoint.pipe_path,
            &client_nonce,
            &server_nonce,
        ),
    });
    transport.send_frame(&prove)?;

    let ack = transport.read_frame()?;
    if ack.get("type").and_then(|v| v.as_str()) != Some("session_hello_ack") {
        let code = ack
            .get("code")
            .and_then(|v| v.as_str())
            .unwrap_or("ack_expected");
        return Err(WbipcSwitchError::HandshakeFailed(code.to_string()));
    }
    Ok(HandshakeAck {
        connection_epoch: ack.get("connection_epoch").and_then(|v| v.as_u64()),
        pipes: ack
            .get("pipes")
            .and_then(|v| v.as_array())
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| item.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default(),
    })
}

/// JSON-RPC 2.0 调用（响应帧 `replyResult`/`replyError` 优先，标准
/// `{result}`/`{error}` 形态兼容；`session_hello_error` 视为会话丢失）。
pub fn call_rpc(
    transport: &mut dyn WbipcTransport,
    id: u64,
    method: &'static str,
    params: serde_json::Value,
) -> Result<serde_json::Value, WbipcSwitchError> {
    transport.send_frame(&serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": method,
        "params": params,
    }))?;
    loop {
        let frame = transport.read_frame()?;
        let frame_type = frame.get("type").and_then(|v| v.as_str()).unwrap_or("");
        match frame_type {
            "replyResult" => return Ok(frame.get("result").cloned().unwrap_or_default()),
            "replyError" => {
                let code = frame
                    .get("error")
                    .and_then(extract_error_code)
                    .unwrap_or_else(|| "unknown".to_string());
                return Err(WbipcSwitchError::RpcRejected { method, code });
            }
            "session_hello_error" => {
                let code = frame
                    .get("code")
                    .and_then(|v| v.as_str())
                    .unwrap_or("session_lost");
                return Err(WbipcSwitchError::HandshakeFailed(code.to_string()));
            }
            _ => {
                if frame.get("result").is_some() {
                    return Ok(frame["result"].clone());
                }
                if frame.get("error").is_some() {
                    let code = frame
                        .get("error")
                        .and_then(extract_error_code)
                        .unwrap_or_else(|| "unknown".to_string());
                    return Err(WbipcSwitchError::RpcRejected { method, code });
                }
                // 通知/未知帧：继续读下一帧（不猜测语义）。
            }
        }
    }
}

// ===== 切号编排 =====

/// 切号回执（脱敏；不含令牌材料）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkBuddySwitchOutcome {
    /// 目标账号 uid（本地 UUID；等于池记录 account_id）。
    pub account_id: String,
    /// 目标账号展示名（池记录）。
    pub screen_name: String,
    /// 本次是否先做了池凭据续期（false = access 充裕，未轮换）。
    pub renewed: bool,
}

/// 从池记录构造 saveOauthToken 的 auth 对象（.info 元数据 camelCase 同形；
/// expiresIn/refreshExpiresIn 用剩余秒数，lastRefreshTime 用当前毫秒）。
/// **真机验证检查点**：确切键形以真机 saveOauthToken 结果为准（单测锁定
/// 当前形态，偏差在真机验证时修正）。
fn auth_object_from_record(record: &WorkBuddyLoginRecord, now_unix: u64) -> serde_json::Value {
    let access_remaining = record
        .access_token_expires_at_unix_seconds
        .saturating_sub(now_unix);
    let refresh_remaining = record
        .refresh_token_expires_at_unix_seconds
        .saturating_sub(now_unix);
    serde_json::json!({
        "accessToken": record.access_token,
        "refreshToken": record.refresh_token,
        "scope": "",
        "domain": record.domain,
        "sessionState": record.session_state,
        "expiresIn": access_remaining,
        "refreshExpiresIn": refresh_remaining,
        "lastRefreshTime": now_unix.saturating_mul(1000),
    })
}

/// 从 getAccount 响应提取 uid（响应形态防御：result.data.uid / result.uid /
/// result.user.uid / result.account.uid 逐层尝试——真机验证检查点）。
fn uid_from_account_response(response: &serde_json::Value) -> Option<String> {
    let result = response.get("data").unwrap_or(response);
    for path in [
        &["uid"][..],
        &["user", "uid"][..],
        &["account", "uid"][..],
        &["data", "uid"][..],
    ] {
        let mut cursor = result;
        let mut ok = true;
        for key in path {
            match cursor.get(key) {
                Some(next) => cursor = next,
                None => {
                    ok = false;
                    break;
                }
            }
        }
        if ok {
            if let Some(uid) = cursor.as_str() {
                if !uid.is_empty() {
                    return Some(uid.to_string());
                }
            }
        }
    }
    None
}

/// 切号编排（泛型连接工厂，离线单测注入脚本化对端）：
/// 握手 → `auth:logout` → `auth:saveOauthToken`（池内最新代次）→
/// `auth:getAccount` 回读 uid 交叉校验。任何一步失败即中止（fail-closed），
/// 客户端侧可能停在登出态——UI 提示用户在客户端重登或重试切号。
pub fn switch_workbuddy_account_via_rpc(
    endpoint: &WbipcEndpoint,
    record: &WorkBuddyLoginRecord,
    renewed: bool,
    now_unix: u64,
    connect: &dyn Fn() -> Result<Box<dyn WbipcTransport>, WbipcSwitchError>,
) -> Result<WorkBuddySwitchOutcome, WbipcSwitchError> {
    let mut transport = connect()?;
    handshake(transport.as_mut(), endpoint, "workbuddy-switch")?;
    call_rpc(transport.as_mut(), 1, "auth:logout", serde_json::json!({}))?;
    call_rpc(
        transport.as_mut(),
        2,
        "auth:saveOauthToken",
        serde_json::json!({ "auth": auth_object_from_record(record, now_unix) }),
    )?;
    let account = call_rpc(transport.as_mut(), 3, "auth:getAccount", serde_json::json!({}))?;
    match uid_from_account_response(&account) {
        Some(uid) if uid == record.account_id => Ok(WorkBuddySwitchOutcome {
            account_id: record.account_id.clone(),
            screen_name: record.screen_name.clone(),
            renewed,
        }),
        _ => Err(WbipcSwitchError::VerifyFailed),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    /// 脚本化对端：按序回放响应帧，记录发出帧（离线单测传输）。
    struct MockTransport {
        inbox: VecDeque<serde_json::Value>,
        sent: Vec<serde_json::Value>,
    }

    impl MockTransport {
        fn scripted(frames: &[serde_json::Value]) -> Self {
            Self {
                inbox: frames.iter().cloned().collect(),
                sent: Vec::new(),
            }
        }
    }

    impl WbipcTransport for MockTransport {
        fn send_frame(&mut self, value: &serde_json::Value) -> Result<(), WbipcSwitchError> {
            self.sent.push(value.clone());
            Ok(())
        }

        fn read_frame(&mut self) -> Result<serde_json::Value, WbipcSwitchError> {
            self.inbox
                .pop_front()
                .ok_or(WbipcSwitchError::Io)
        }
    }

    /// RFC 4231 Test Case 1/2：HMAC-SHA256 官方向量锁定手写实现。
    #[test]
    fn hmac_sha256_matches_rfc4231_vectors() {
        let key = [0x0b_u8; 20];
        let digest = hmac_sha256(&key, b"Hi There");
        assert_eq!(
            b64url_encode(&digest),
            "sDRMYdjbOFNcqK_OrwvxK4gdwgDJgz2nJuk3bC4yz_c",
            "RFC 4231 case 1（hex b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7）"
        );
        let digest = hmac_sha256(b"Jefe", b"what do ya want for nothing?");
        assert_eq!(
            b64url_encode(&digest),
            "W9zBRr9gdU5qBCQmCJV1x1oAPwidJzmDnexYuWTsOEM",
            "RFC 4231 case 2（hex 5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843）"
        );
    }

    /// ticket_id：sha256 前 16 hex（已知向量 abc → 6ca… 的前 16 位）。
    #[test]
    fn ticket_id_is_sha256_prefix16() {
        assert_eq!(ticket_id("abc"), "ba7816bf8f01cfea");
        assert_eq!(ticket_id("").len(), 16);
    }

    /// LP4BE 段拼接：长度前缀大端 + 逐段精确。
    #[test]
    fn lp4be_frames_each_segment() {
        let framed = lp4be(&[b"ab", b"", b"c"]);
        assert_eq!(
            framed,
            vec![0, 0, 0, 2, b'a', b'b', 0, 0, 0, 0, 0, 0, 0, 1, b'c']
        );
    }

    /// client_proof 形态：base64url 无填充、确定性、输入敏感。
    #[test]
    fn client_proof_is_deterministic_base64url() {
        let proof = client_proof("ticket-1", r"\\.\pipe\wb", &[1_u8; 16], &[2_u8; 16]);
        assert!(!proof.contains('+') && !proof.contains('/') && !proof.contains('='));
        assert_eq!(
            proof,
            client_proof("ticket-1", r"\\.\pipe\wb", &[1_u8; 16], &[2_u8; 16])
        );
        assert_ne!(
            proof,
            client_proof("ticket-1", r"\\.\pipe\wb", &[1_u8; 16], &[3_u8; 16]),
            "serverNonce 参与转录"
        );
        assert_ne!(
            proof,
            client_proof("ticket-2", r"\\.\pipe\wb", &[1_u8; 16], &[2_u8; 16]),
            "ticket 参与 HMAC key"
        );
    }

    /// nonce 唯一性：同进程连续两次生成不重复（时间纳秒分辨率足够）。
    #[test]
    fn generate_nonce_is_unique_per_call() {
        let first = generate_nonce();
        let second = generate_nonce();
        assert_ne!(first, second);
        assert_eq!(first.len(), 16);
    }

    /// 端点解析：合法形态 + 缺字段拒绝。
    #[test]
    fn parse_endpoint_validates_shape() {
        let endpoint = parse_endpoint(r#"{"endpoint":"\\\\.\\pipe\\wb-1","ticket":"tkt"}"#).unwrap();
        assert_eq!(endpoint.pipe_path, r"\\.\pipe\wb-1");
        assert_eq!(endpoint.ticket, "tkt");
        assert_eq!(
            parse_endpoint(r#"{"endpoint":"\\\\.\\pipe\\wb"}"#).unwrap_err(),
            WbipcSwitchError::EndpointMissing
        );
        assert_eq!(
            parse_endpoint("not json").unwrap_err(),
            WbipcSwitchError::EndpointMissing
        );
    }

    /// 握手成功路径：hello 内容（protocol/ticket_id/client_nonce）与 prove
    /// 的 client_proof 逐段符合 spec 8.6；ack 携带 pipes。
    #[test]
    fn handshake_completes_three_steps() {
        let endpoint = WbipcEndpoint {
            pipe_path: r"\\.\pipe\wb-broker".to_string(),
            ticket: "ticket-utf8".to_string(),
        };
        let server_nonce = b64url_encode(&[7_u8; 16]);
        let mut transport = MockTransport::scripted(&[
            serde_json::json!({"type": "session_challenge", "server_nonce": server_nonce}),
            serde_json::json!({
                "type": "session_hello_ack",
                "protocol": 1,
                "connection_epoch": 42,
                "max_inflight": 8,
                "pipes": ["auth-pipe", "misc-pipe"],
            }),
        ]);
        let ack = handshake(&mut transport, &endpoint, "workbuddy-switch").unwrap();
        assert_eq!(ack.connection_epoch, Some(42));
        assert_eq!(ack.pipes, vec!["auth-pipe", "misc-pipe"]);
        // 帧序：hello → prove。
        assert_eq!(transport.sent.len(), 2);
        let hello = &transport.sent[0];
        assert_eq!(hello["type"], "session_hello");
        assert_eq!(hello["protocol_min"], 1);
        assert_eq!(hello["protocol_max"], 1);
        assert_eq!(hello["ticket_id"], ticket_id("ticket-utf8"));
        assert_eq!(hello["client"]["kind"], "tool");
        assert_eq!(hello["client"]["id"], "workbuddy-switch");
        // prove 的 proof 与纯函数一致（同 nonce/ticket/管道路径）。
        let prove = &transport.sent[1];
        assert_eq!(prove["type"], "session_prove");
        let client_nonce = b64url_decode(hello["client_nonce"].as_str().unwrap()).unwrap();
        let expected = client_proof(
            "ticket-utf8",
            r"\\.\pipe\wb-broker",
            &client_nonce,
            &[7_u8; 16],
        );
        assert_eq!(prove["client_proof"], expected);
    }

    /// 握手失败：hello_error 原样携带协议错误码（ticket 比对失败场景）。
    #[test]
    fn handshake_surfaces_hello_error_code() {
        let endpoint = WbipcEndpoint {
            pipe_path: r"\\.\pipe\wb".to_string(),
            ticket: "t".to_string(),
        };
        let mut transport = MockTransport::scripted(&[serde_json::json!({
            "type": "session_hello_error",
            "code": "auth_failed",
        })]);
        let error = handshake(&mut transport, &endpoint, "workbuddy-switch").unwrap_err();
        assert_eq!(
            error,
            WbipcSwitchError::HandshakeFailed("auth_failed".to_string())
        );
        assert!(error.code().starts_with("workbuddy_switch_handshake_failed:"));
    }

    /// RPC：replyResult 取 result；replyError 携带方法与错误码；
    /// session_hello_error 视为会话丢失。
    #[test]
    fn call_rpc_parses_reply_frames() {
        let mut ok = MockTransport::scripted(&[serde_json::json!({"type": "replyResult", "result": {"uid": "u1"}})]);
        let result = call_rpc(&mut ok, 1, "auth:getAccount", serde_json::json!({})).unwrap();
        assert_eq!(result["uid"], "u1");
        assert_eq!(ok.sent[0]["method"], "auth:getAccount");
        assert_eq!(ok.sent[0]["jsonrpc"], "2.0");

        let mut rejected = MockTransport::scripted(&[serde_json::json!({
            "type": "replyError",
            "error": {"code": "invalid_session", "message": "no session"},
        })]);
        let error = call_rpc(&mut rejected, 2, "auth:logout", serde_json::json!({})).unwrap_err();
        assert_eq!(
            error,
            WbipcSwitchError::RpcRejected {
                method: "auth:logout",
                code: "invalid_session".to_string(),
            }
        );

        let mut lost = MockTransport::scripted(&[serde_json::json!({"type": "session_hello_error", "code": "bad_request"})]);
        let error = call_rpc(&mut lost, 3, "auth:getAccount", serde_json::json!({})).unwrap_err();
        assert!(matches!(error, WbipcSwitchError::HandshakeFailed(_)));
    }

    /// 切号编排成功路径：logout → saveOauthToken（auth 对象来自池记录）→
    /// getAccount 回读 uid 与目标一致；回执带池记录展示名。
    #[test]
    fn switch_orchestration_happy_path_verifies_uid() {
        let endpoint = WbipcEndpoint {
            pipe_path: r"\\.\pipe\wb".to_string(),
            ticket: "t".to_string(),
        };
        let record = fixture_record();
        let now = 1_800_000_000_u64;
        let frames = [
            serde_json::json!({"type": "session_challenge", "server_nonce": b64url_encode(&[9; 16])}),
            serde_json::json!({"type": "session_hello_ack", "protocol": 1, "pipes": []}),
            serde_json::json!({"type": "replyResult", "result": {}}),
            serde_json::json!({"type": "replyResult", "result": {}}),
            serde_json::json!({"type": "replyResult", "result": {"uid": record.account_id}}),
        ];
        let connect = move || -> Result<Box<dyn WbipcTransport>, WbipcSwitchError> {
            Ok(Box::new(MockTransport::scripted(&frames)) as Box<dyn WbipcTransport>)
        };
        let outcome =
            switch_workbuddy_account_via_rpc(&endpoint, &record, true, now, &connect).unwrap();
        assert_eq!(outcome.account_id, "fixture-uid");
        assert_eq!(outcome.screen_name, "测试账号");
        assert!(outcome.renewed);
    }

    /// 切号编排：uid 回读不符 → VerifyFailed（宁可不报成功，不可错登）。
    #[test]
    fn switch_orchestration_rejects_uid_mismatch() {
        let endpoint = WbipcEndpoint {
            pipe_path: r"\\.\pipe\wb".to_string(),
            ticket: "t".to_string(),
        };
        let record = fixture_record();
        let frames = [
            serde_json::json!({"type": "session_challenge", "server_nonce": b64url_encode(&[9; 16])}),
            serde_json::json!({"type": "session_hello_ack", "protocol": 1, "pipes": []}),
            serde_json::json!({"type": "replyResult", "result": {}}),
            serde_json::json!({"type": "replyResult", "result": {}}),
            serde_json::json!({"type": "replyResult", "result": {"uid": "other-uid"}}),
        ];
        let connect = move || -> Result<Box<dyn WbipcTransport>, WbipcSwitchError> {
            Ok(Box::new(MockTransport::scripted(&frames)) as Box<dyn WbipcTransport>)
        };
        let error = switch_workbuddy_account_via_rpc(&endpoint, &record, false, 0, &connect)
            .unwrap_err();
        assert_eq!(error, WbipcSwitchError::VerifyFailed);
    }

    /// 切号编排：logout 被拒 → RpcRejected 原样上抛（fail-closed 中止）。
    #[test]
    fn switch_orchestration_aborts_when_logout_rejected() {
        let endpoint = WbipcEndpoint {
            pipe_path: r"\\.\pipe\wb".to_string(),
            ticket: "t".to_string(),
        };
        let record = fixture_record();
        let frames = [
            serde_json::json!({"type": "session_challenge", "server_nonce": b64url_encode(&[9; 16])}),
            serde_json::json!({"type": "session_hello_ack", "protocol": 1, "pipes": []}),
            serde_json::json!({"type": "replyError", "error": {"code": "busy"}}),
        ];
        let connect = move || -> Result<Box<dyn WbipcTransport>, WbipcSwitchError> {
            Ok(Box::new(MockTransport::scripted(&frames)) as Box<dyn WbipcTransport>)
        };
        let error = switch_workbuddy_account_via_rpc(&endpoint, &record, false, 0, &connect)
            .unwrap_err();
        assert_eq!(
            error,
            WbipcSwitchError::RpcRejected {
                method: "auth:logout",
                code: "busy".to_string(),
            }
        );
    }

    /// auth 对象形态锁定：令牌/域名/会话标识来自池记录，有效期用剩余秒。
    #[test]
    fn auth_object_shapes_from_record() {
        let record = fixture_record();
        let object = auth_object_from_record(&record, 1_800_000_000);
        assert_eq!(object["accessToken"], record.access_token);
        assert_eq!(object["refreshToken"], record.refresh_token);
        assert_eq!(object["domain"], record.domain);
        assert_eq!(object["sessionState"], record.session_state);
        assert_eq!(
            object["expiresIn"],
            record.access_token_expires_at_unix_seconds - 1_800_000_000
        );
    }

    fn fixture_record() -> WorkBuddyLoginRecord {
        WorkBuddyLoginRecord {
            profile_id: "wb-fixture".to_string(),
            account_id: "fixture-uid".to_string(),
            screen_name: "测试账号".to_string(),
            masked_mobile: String::new(),
            access_token: "at".to_string(),
            refresh_token: "rt".to_string(),
            access_token_expires_at_unix_seconds: 1_800_000_000 + 100,
            refresh_token_expires_at_unix_seconds: 1_800_000_000 + 200,
            domain: "www.workbuddy.cn".to_string(),
            session_state: "ss".to_string(),
            device_id: "dev".to_string(),
            minted_at_unix_seconds: 1_799_000_000,
            last_updated_at_unix_seconds: 1_799_000_000,
        }
    }
}
