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
//! **P17-A4 真机调试定案（asar 实读修正，2026-10-19）**：
//! 1. **transcript 形态**：`encodeTranscript(role, t)` 五段 LP4BE 全部为
//!    **UTF-8 字符串**——`[roleTag, String(protocol), endpoint, clientNonce,
//!    serverNonce]`；clientNonce/serverNonce 是 **base64url 串本身**（非
//!    解码字节——初版实现误用解码字节，真机 auth_failed 根因之一）；
//! 2. **challenge**：`{type:"session_challenge", protocol:1, server_nonce,
//!    server_proof}`（server 侧角色串 "wbipc-s"）；
//! 3. **RPC 路由**：标准 JSON-RPC（`{jsonrpc,id,result|error}`）；三类方法：
//!    `broker/ListPipes` → `{pipes:[名]}`；`broker/GetPipe`（params.pipe）
//!    → 建通道 `c:{pipe}` 回 `{channel,methods}`；其余方法必须是
//!    **`c:{pipe}/{method}` 形态 + 请求顶层 `mode:"call"`**（handleChannelCall
//!    按首个 `/` 切通道，mode 缺失/不匹配 = BAD_REQUEST/METHOD_KIND_MISMATCH）；
//! 4. **切号写路径不存在**：wbipc broker 只注册三根管道——`wb.request`
//!    （唯一方法 `http.fetch`：经客户端后端会话的 HTTP 代理，自动注入
//!    Bearer/x-user-id）、entitlement、wecom-a2a。**auth:login/logout/
//!    saveOauthToken 是 Electron IPC 通道（app 内部 ipcRenderer），不在
//!    wbipc 面**——spec 8.4 的写路径前提被真机推翻，切号机制需重设计
//!    （主理人决策）；本模块现提供只读会话验证（handshake → ListPipes →
//!    GetPipe("wb.request") → http.fetch 签到状态）作为 A4 的真机证据链。
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
    RpcRejected { method: String, code: String },
    /// 切号后回读账号与目标不符（宁可不报成功，不可错登）。
    VerifyFailed,
    /// 帧超限（protocol violation）。
    FrameTooLarge,
    /// 目标账号不在工具池（未捕获/已清理）。
    AccountMissing,
    /// 所需管道在 wbipc 面不可用（auth 写方法不在该协议面——A4 调试定案）。
    PipeUnavailable,
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
            Self::PipeUnavailable => "workbuddy_switch_pipe_unavailable".to_string(),
            Self::Io => "workbuddy_switch_io".to_string(),
        }
    }
}

/// wbipc 端点（`~/.workbuddy/wbipc/endpoint.json` 的 `{endpoint, ticket}`）。
/// ticket 随客户端每次启动轮换，值永不打印/落日志。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WbipcEndpoint {
    /// 命名管道路径（Windows `\.\pipe\…`）。
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
/// encodeTranscript = concat(LP4BE 五段 **UTF-8 字符串** ["wbipc-c", "1",
/// <管道路径>, clientNonce, serverNonce])——nonce 段是 base64url 串本身
/// （asar `encodeTranscript` 实读：`Buffer.from(e,"utf8")` 逐段，P17-A4
/// 真机调试定案修正——初版误用解码后原始字节导致 auth_failed）。
pub fn client_proof(
    ticket: &str,
    pipe_path: &str,
    client_nonce_b64: &str,
    server_nonce_b64: &str,
) -> String {
    let transcript = lp4be(&[
        b"wbipc-c",
        b"1",
        pipe_path.as_bytes(),
        client_nonce_b64.as_bytes(),
        server_nonce_b64.as_bytes(),
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

/// 生产传输：Windows 命名管道（`\.\pipe\…` 经 std fs 直接打开，与探针
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
    let Some(server_nonce_b64) = nonce_from(&challenge, "server_nonce", "serverNonce") else {
        return Err(WbipcSwitchError::HandshakeFailed("missing_nonce".to_string()));
    };
    // nonce_from 返回解码字节——此处需要 base64url 串本身（transcript 形态
    // 是字符串段），从 challenge 原文取回串值。
    let server_nonce_str = challenge
        .get("server_nonce")
        .or_else(|| challenge.get("serverNonce"))
        .and_then(|v| v.as_str())
        .ok_or(WbipcSwitchError::HandshakeFailed("missing_nonce".to_string()))?;
    let prove = serde_json::json!({
        "type": "session_prove",
        "client_proof": client_proof(
            &endpoint.ticket,
            &endpoint.pipe_path,
            &b64url_encode(&client_nonce),
            server_nonce_str,
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
    method: &str,
    params: serde_json::Value,
) -> Result<serde_json::Value, WbipcSwitchError> {
    call_method(
        transport,
        id,
        method,
        serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        }),
    )
}

/// 通道调用：wbipc 的方法路由要求非 broker 方法以 `c:{pipe}/{method}`
/// 形态发起，且请求**顶层**携带 `mode:"call"`（handleChannelCall 按首个
/// `/` 切通道名；mode 缺失 = BAD_REQUEST，kind 不匹配 =
/// METHOD_KIND_MISMATCH）。
pub fn call_channel(
    transport: &mut dyn WbipcTransport,
    id: u64,
    pipe: &str,
    method: &str,
    params: serde_json::Value,
) -> Result<serde_json::Value, WbipcSwitchError> {
    call_method(
        transport,
        id,
        &format!("c:{pipe}/{method}"),
        serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": format!("c:{pipe}/{method}"),
            "params": params,
            "mode": "call",
        }),
    )
    .map_err(|error| match error {
        WbipcSwitchError::RpcRejected { code, .. } if code == "PIPE_UNKNOWN" => {
            WbipcSwitchError::PipeUnavailable
        }
        other => other,
    })
}

/// 帧发送 + 响应解析核心（call_rpc/call_channel 共用）。
fn call_method(
    transport: &mut dyn WbipcTransport,
    id: u64,
    method: &str,
    frame: serde_json::Value,
) -> Result<serde_json::Value, WbipcSwitchError> {
    let _ = id;
    transport.send_frame(&frame)?;
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
                return Err(WbipcSwitchError::RpcRejected { method: method.to_string(), code });
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
                    return Err(WbipcSwitchError::RpcRejected { method: method.to_string(), code });
                }
                // 通知/未知帧：继续读下一帧（不猜测语义）。
            }
        }
    }
}

/// 内置 HTTP 代理管道名（asar 实读：broker 唯一通用管道）。
pub const WB_REQUEST_PIPE: &str = "wb.request";
/// 内置 HTTP 代理方法名。
pub const HTTP_FETCH_METHOD: &str = "http.fetch";
/// 只读验证用的后端端点（签到活动状态，spec 8.5 定案的已知端点）。
pub const CHECKIN_STATUS_PATH: &str = "/v2/billing/meter/checkin-activity-status";

/// 只读会话验证回执（真机证据链；不含任何令牌材料）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkBuddySessionEvidence {
    /// session_hello_ack 携带的可见管道清单（升序）。
    pub pipes: Vec<String>,
    /// wb.request/http.fetch 只读往返的 HTTP 状态码（0 = 通道未返回）。
    pub http_status: u16,
    /// 响应体字节数（base64 解码后口径）。
    pub body_len: usize,
}

/// 只读会话验证（P17-A4 真机证据链；零写操作）：
/// 握手 → `broker/ListPipes`（wb.request 必须可见，防管道名漂移）→
/// `broker/GetPipe`（建通道）→ `c:wb.request/http.fetch` POST 签到状态
/// （客户端自动注入 Bearer/x-user-id——200 = 客户端会话有效且代理可达）。
pub fn verify_client_session_via_rpc(
    endpoint: &WbipcEndpoint,
    connect: &dyn Fn() -> Result<Box<dyn WbipcTransport>, WbipcSwitchError>,
) -> Result<WorkBuddySessionEvidence, WbipcSwitchError> {
    let mut transport = connect()?;
    handshake(transport.as_mut(), endpoint, "workbuddy-switch")?;
    let listed = call_rpc(transport.as_mut(), 1, "broker/ListPipes", serde_json::json!({}))?;
    let pipes: Vec<String> = listed
        .get("pipes")
        .and_then(|value| value.as_array())
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    if !pipes.iter().any(|pipe| pipe == WB_REQUEST_PIPE) {
        return Err(WbipcSwitchError::PipeUnavailable);
    }
    let got = call_rpc(
        transport.as_mut(),
        2,
        "broker/GetPipe",
        serde_json::json!({ "pipe": WB_REQUEST_PIPE }),
    )?;
    if got.get("channel").and_then(|value| value.as_str()).is_none() {
        return Err(WbipcSwitchError::Io);
    }
    let response = call_channel(
        transport.as_mut(),
        3,
        WB_REQUEST_PIPE,
        HTTP_FETCH_METHOD,
        serde_json::json!({
            "method": "POST",
            "path": CHECKIN_STATUS_PATH,
            "headers": { "content-type": "application/json" },
        }),
    )?;
    let http_status = response.get("status").and_then(|value| value.as_u64()).unwrap_or(0) as u16;
    let body_b64 = response
        .get("body_b64")
        .and_then(|value| value.as_str())
        .unwrap_or_default();
    let body_len = b64url_decode(body_b64).map(|bytes| bytes.len()).unwrap_or(0);
    Ok(WorkBuddySessionEvidence {
        pipes,
        http_status,
        body_len,
    })
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

    /// ticket_id：sha256 前 16 hex（已知向量 abc → ba78… 的前 16 位）。
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

    /// P17-A4 定案已知答案：transcript 五段为 UTF-8 字符串（nonce 段 =
    /// base64url 串本身），与 python 独立实现交叉锁定。
    #[test]
    fn client_proof_matches_transcript_known_answer() {
        let proof = client_proof(
            "ticket-utf8",
            r"\.\pipe\wb",
            "AAAAAAAAAAAAAAAAAAAAAA",
            "BBBBBBBBBBBBBBBBBBBBBB",
        );
        assert_eq!(proof, "l5sMEQwmLGuGuHCYbU_tVnrtqAu6IVi-HOmSUELVPr8");
        // server 角色（wbipc-s）侧证明形态不同。
        let server = {
            let transcript = lp4be(&[
                b"wbipc-s",
                b"1",
                r"\.\pipe\wb".as_bytes(),
                b"AAAAAAAAAAAAAAAAAAAAAA",
                b"BBBBBBBBBBBBBBBBBBBBBB",
            ]);
            b64url_encode(&hmac_sha256("ticket-utf8".as_bytes(), &transcript))
        };
        assert_eq!(server, "Hkvu68Vm_qMxwdRRW9awOjjOnV3liMtXuhTT_z0Ggpw");
        // 输入敏感：serverNonce 变化即变化。
        assert_ne!(
            proof,
            client_proof(
                "ticket-utf8",
                r"\.\pipe\wb",
                "AAAAAAAAAAAAAAAAAAAAAA",
                "CCCCCCCCCCCCCCCCCCCCCC",
            )
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
        let endpoint =
            parse_endpoint(r#"{"endpoint":"\\\\.\\pipe\\wb-1","ticket":"tkt"}"#).unwrap();
        assert_eq!(endpoint.pipe_path, r"\\.\pipe\wb-1");
        assert_eq!(endpoint.ticket, "tkt");
        assert_eq!(
            parse_endpoint(r#"{"endpoint":"\.\pipe\wb"}"#).unwrap_err(),
            WbipcSwitchError::EndpointMissing
        );
        assert_eq!(
            parse_endpoint("not json").unwrap_err(),
            WbipcSwitchError::EndpointMissing
        );
    }

    /// 握手成功路径：hello 内容（protocol/ticket_id/client_nonce）与 prove
    /// 的 client_proof 逐段符合 asar 定案；ack 携带 pipes。
    #[test]
    fn handshake_completes_three_steps() {
        let endpoint = WbipcEndpoint {
            pipe_path: r"\.\pipe\wb-broker".to_string(),
            ticket: "ticket-utf8".to_string(),
        };
        let server_nonce = b64url_encode(&[7_u8; 16]);
        let mut transport = MockTransport::scripted(&[
            serde_json::json!({
                "type": "session_challenge",
                "protocol": 1,
                "server_nonce": server_nonce,
                "server_proof": "srv-proof",
            }),
            serde_json::json!({
                "type": "session_hello_ack",
                "protocol": 1,
                "connection_epoch": "ce-41414141",
                "max_inflight": 8,
                "pipes": ["wb.request"],
            }),
        ]);
        let ack = handshake(&mut transport, &endpoint, "workbuddy-switch").unwrap();
        assert_eq!(ack.pipes, vec!["wb.request"]);
        // 帧序：hello → prove。
        assert_eq!(transport.sent.len(), 2);
        let hello = &transport.sent[0];
        assert_eq!(hello["type"], "session_hello");
        assert_eq!(hello["protocol_min"], 1);
        assert_eq!(hello["protocol_max"], 1);
        assert_eq!(hello["ticket_id"], ticket_id("ticket-utf8"));
        assert_eq!(hello["client"]["kind"], "tool");
        assert_eq!(hello["client"]["id"], "workbuddy-switch");
        // prove 的 proof 与纯函数一致（nonce 段 = base64url 串本身）。
        let prove = &transport.sent[1];
        assert_eq!(prove["type"], "session_prove");
        let expected = client_proof(
            "ticket-utf8",
            r"\.\pipe\wb-broker",
            hello["client_nonce"].as_str().unwrap(),
            server_nonce.as_str(),
        );
        assert_eq!(prove["client_proof"], expected);
    }

    /// 握手失败：hello_error 原样携带协议错误码（ticket 比对失败场景）。
    #[test]
    fn handshake_surfaces_hello_error_code() {
        let endpoint = WbipcEndpoint {
            pipe_path: r"\.\pipe\wb".to_string(),
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

    /// RPC：标准 jsonrpc result/error 解析；session_hello_error 视为会话丢失。
    #[test]
    fn call_rpc_parses_reply_frames() {
        let mut ok = MockTransport::scripted(&[serde_json::json!(
            {"jsonrpc": "2.0", "id": 1, "result": {"pipes": ["wb.request"]}}
        )]);
        let result = call_rpc(&mut ok, 1, "broker/ListPipes", serde_json::json!({})).unwrap();
        assert_eq!(result["pipes"][0], "wb.request");
        assert_eq!(ok.sent[0]["method"], "broker/ListPipes");
        assert_eq!(ok.sent[0]["jsonrpc"], "2.0");

        let mut rejected = MockTransport::scripted(&[serde_json::json!(
            {"jsonrpc": "2.0", "id": 2, "error": {"code": "PIPE_UNKNOWN", "message": "unknown pipe"}}
        )]);
        let error =
            call_rpc(&mut rejected, 2, "broker/GetPipe", serde_json::json!({})).unwrap_err();
        assert_eq!(
            error,
            WbipcSwitchError::RpcRejected {
                method: "broker/GetPipe".to_string(),
                code: "PIPE_UNKNOWN".to_string(),
            }
        );

        let mut lost = MockTransport::scripted(&[serde_json::json!(
            {"type": "session_hello_error", "code": "bad_request"}
        )]);
        let error =
            call_rpc(&mut lost, 3, "broker/ListPipes", serde_json::json!({})).unwrap_err();
        assert!(matches!(error, WbipcSwitchError::HandshakeFailed(_)));
    }

    /// 通道调用（P17-A4 定案）：方法 = `c:{pipe}/{method}` + 顶层 mode:"call"。
    #[test]
    fn call_channel_frames_channel_method_and_mode() {
        let mut transport = MockTransport::scripted(&[serde_json::json!(
            {"jsonrpc": "2.0", "id": 1, "result": {"status": 200, "body_b64": ""}}
        )]);
        let result = call_channel(
            &mut transport,
            1,
            WB_REQUEST_PIPE,
            HTTP_FETCH_METHOD,
            serde_json::json!({"method": "POST", "path": "/x"}),
        )
        .unwrap();
        assert_eq!(result["status"], 200);
        assert_eq!(
            transport.sent[0]["method"],
            "c:wb.request/http.fetch",
            "通道方法形态：c:wb.request/http.fetch"
        );
        assert_eq!(transport.sent[0]["mode"], "call", "顶层 mode 必须为 call");
        assert_eq!(transport.sent[0]["params"]["method"], "POST");
    }

    /// 只读会话验证编排：握手 → ListPipes → GetPipe → http.fetch 签到状态
    /// → 回执（pipes/status/body_len）；wb.request 不可见 = PipeUnavailable。
    #[test]
    fn verify_session_orchestration_roundtrip() {
        let endpoint = WbipcEndpoint {
            pipe_path: r"\.\pipe\wb".to_string(),
            ticket: "t".to_string(),
        };
        let body = b64url_encode(br#"{"code":0,"data":{"today_checked_in":false}}"#);
        let frames = [
            serde_json::json!({
                "type": "session_challenge",
                "protocol": 1,
                "server_nonce": b64url_encode(&[9; 16]),
                "server_proof": "srv",
            }),
            serde_json::json!({"type": "session_hello_ack", "protocol": 1, "pipes": ["wb.request"]}),
            serde_json::json!({"jsonrpc": "2.0", "id": 1, "result": {"pipes": ["wb.request"]}}),
            serde_json::json!({"jsonrpc": "2.0", "id": 2, "result": {"channel": "c:wb.request", "methods": ["http.fetch"]}}),
            serde_json::json!({"jsonrpc": "2.0", "id": 3, "result": {"status": 200, "headers": {}, "body_b64": body}}),
        ];
        let connect = move || -> Result<Box<dyn WbipcTransport>, WbipcSwitchError> {
            Ok(Box::new(MockTransport::scripted(&frames)) as Box<dyn WbipcTransport>)
        };
        let evidence = verify_client_session_via_rpc(&endpoint, &connect).unwrap();
        assert_eq!(evidence.pipes, vec!["wb.request"]);
        assert_eq!(evidence.http_status, 200);
        assert!(evidence.body_len > 0);
    }

    /// 只读会话验证：wb.request 不在可见清单 → PipeUnavailable（fail-closed）。
    #[test]
    fn verify_session_rejects_missing_request_pipe() {
        let endpoint = WbipcEndpoint {
            pipe_path: r"\.\pipe\wb".to_string(),
            ticket: "t".to_string(),
        };
        let frames = [
            serde_json::json!({
                "type": "session_challenge",
                "protocol": 1,
                "server_nonce": b64url_encode(&[9; 16]),
                "server_proof": "srv",
            }),
            serde_json::json!({"type": "session_hello_ack", "protocol": 1, "pipes": ["other"]}),
            serde_json::json!({"jsonrpc": "2.0", "id": 1, "result": {"pipes": ["other"]}}),
        ];
        let connect = move || -> Result<Box<dyn WbipcTransport>, WbipcSwitchError> {
            Ok(Box::new(MockTransport::scripted(&frames)) as Box<dyn WbipcTransport>)
        };
        assert_eq!(
            verify_client_session_via_rpc(&endpoint, &connect).unwrap_err(),
            WbipcSwitchError::PipeUnavailable
        );
    }
}
