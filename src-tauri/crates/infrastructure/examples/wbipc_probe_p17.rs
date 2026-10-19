//! P17-A4 真机握手探针（修正版）：wbipc 三态握手 → broker RPC → 只读
//! http.fetch 证据链。
//!
//! 旧版探针先发 JSON-RPC（broker/ListPipes + ticket 字段）——客户端处于
//! `hello` 态收到非 session_hello 帧 → `session_hello_error/bad_request`
//! （真机实测，即本批次调试起点）。修正版按 asar 定案协议（spec 8.6 +
//! P17-A4 实读修正）执行：
//! 1. `session_hello`（protocol_min/max=1 + client_nonce b64url 16B +
//!    ticket_id=sha256(ticket)[:16hex] + client 三元组）；
//! 2. 收 `session_challenge`（server_nonce b64url）→ `session_prove`
//!    （client_proof = base64url(HMAC-SHA256(ticket, LP4BE["wbipc-c","1",
//!    endpoint,clientNonce,serverNonce]))，nonce 段为 base64url 串本身）；
//! 3. `session_hello_ack`（pipes 清单）→ `broker/ListPipes` →
//!    `broker/GetPipe("wb.request")` → `c:wb.request/http.fetch` POST
//!    签到状态（只读；响应脱敏打印）。
//!
//! 安全边界：全程只读（无 logout/saveOauthToken 写调用）；响应打印前对
//! token/ticket 类值脱敏。
//!
//! 注意：命名管道阻塞读（无 overlapped 超时）；服务器无响应时 Ctrl+C 终止。
//!
//! 前置：WorkBuddy 桌面客户端运行中（endpoint.json 的 ticket 随每次启动
//! 轮换）。
//!
//! 用法：`wbipc_probe_p17 [endpoint_json]`
//!   默认 = `~/.workbuddy/wbipc/endpoint.json`。

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use sha2::{Digest, Sha256};

#[derive(serde::Deserialize)]
struct EndpointFile {
    endpoint: String,
    ticket: String,
}

fn default_endpoint() -> PathBuf {
    let profile = std::env::var("USERPROFILE").unwrap_or_default();
    Path::new(&profile)
        .join(".workbuddy")
        .join("wbipc")
        .join("endpoint.json")
}

fn b64url(bytes: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

fn ticket_id(ticket: &str) -> String {
    let digest = Sha256::digest(ticket.as_bytes());
    digest[..8].iter().map(|byte| format!("{byte:02x}")).collect()
}

fn lp4be(segments: &[&[u8]]) -> Vec<u8> {
    let mut out = Vec::new();
    for segment in segments {
        out.extend_from_slice(&(segment.len() as u32).to_be_bytes());
        out.extend_from_slice(segment);
    }
    out
}

fn client_proof(ticket: &str, endpoint: &str, client_nonce: &str, server_nonce: &str) -> String {
    let transcript = lp4be(&[
        b"wbipc-c",
        b"1",
        endpoint.as_bytes(),
        client_nonce.as_bytes(),
        server_nonce.as_bytes(),
    ]);
    b64url(&hmac_sha256(ticket.as_bytes(), &transcript))
}

fn hmac_sha256(key: &[u8], data: &[u8]) -> [u8; 32] {
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

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    let path = PathBuf::from(args.get(1).cloned().unwrap_or_default()).abs_or(default_endpoint());
    println!("endpoint.json：{}", path.display());
    let raw = match std::fs::read_to_string(&path) {
        Ok(raw) => raw,
        Err(_) => {
            eprintln!("中止：endpoint.json 不可读（WorkBuddy 可能未运行——broker 随主进程创建）");
            return ExitCode::FAILURE;
        }
    };
    let endpoint: EndpointFile = match serde_json::from_str(&raw) {
        Ok(endpoint) => endpoint,
        Err(_) => {
            eprintln!("中止：endpoint.json 形态不符（{{endpoint, ticket}}）");
            return ExitCode::FAILURE;
        }
    };
    println!(
        "管道：{}（ticket len={}，值不打印；ticket_id={}）",
        endpoint.endpoint,
        endpoint.ticket.len(),
        ticket_id(&endpoint.ticket)
    );

    let mut pipe = match std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&endpoint.endpoint)
    {
        Ok(pipe) => pipe,
        Err(error) => {
            eprintln!("中止：管道连接失败（{error}）——客户端未运行或管道已轮换");
            return ExitCode::FAILURE;
        }
    };
    println!("管道已连接。");

    // 第 1 步：session_hello（客户端处于 hello 态，非 hello 帧即 bad_request）。
    let client_nonce = b64url(&{
        let digest = Sha256::digest(format!(
            "nonce-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|duration| duration.as_nanos())
                .unwrap_or_default()
        ));
        digest[..16].to_vec()
    });
    let hello = serde_json::json!({
        "type": "session_hello",
        "protocol_min": 1,
        "protocol_max": 1,
        "client_nonce": client_nonce,
        "ticket_id": ticket_id(&endpoint.ticket),
        "client": {"kind": "tool", "id": "traesync-probe", "version": env!("CARGO_PKG_VERSION")},
    });
    let response = match roundtrip(&mut pipe, hello.to_string().as_bytes()) {
        Ok(response) => response,
        Err(error) => {
            eprintln!("hello 往返失败：{error}");
            return ExitCode::FAILURE;
        }
    };
    if response.is_empty() {
        eprintln!("诊断：challenge 读到 EOF（0 字节）——服务端未回帧即断开");
        return ExitCode::FAILURE;
    }
    let challenge: serde_json::Value = match serde_json::from_slice(&response) {
        Ok(value) => value,
        Err(_) => {
            eprintln!("challenge 帧解析失败（{} 字节）：{}", response.len(), redact(&response));
            return ExitCode::FAILURE;
        }
    };
    if challenge["type"] != "session_challenge" {
        eprintln!("中止：非 challenge 帧（{}）", redact(&response));
        return ExitCode::FAILURE;
    }
    let Some(server_nonce) = challenge["server_nonce"].as_str() else {
        eprintln!("中止：challenge 缺 server_nonce");
        return ExitCode::FAILURE;
    };
    println!(
        "challenge 收到（server_proof 已省略打印；server_nonce len={}）",
        server_nonce.len()
    );

    // 第 2 步：session_prove。
    let prove = serde_json::json!({
        "type": "session_prove",
        "client_proof": client_proof(
            &endpoint.ticket,
            &endpoint.endpoint,
            &client_nonce,
            server_nonce,
        ),
    });
    let response = match roundtrip(&mut pipe, prove.to_string().as_bytes()) {
        Ok(response) => response,
        Err(error) => {
            eprintln!("prove 往返失败：{error}");
            return ExitCode::FAILURE;
        }
    };
    let ack: serde_json::Value = match serde_json::from_slice(&response) {
        Ok(value) => value,
        Err(_) => {
            eprintln!("ack 帧解析失败：{}", redact(&response));
            return ExitCode::FAILURE;
        }
    };
    if ack["type"] != "session_hello_ack" {
        eprintln!("中止：握手未通过（{}）", redact(&response));
        return ExitCode::FAILURE;
    }
    println!(
        "✅ session_hello_ack：epoch={} max_inflight={}",
        ack["connection_epoch"], ack["max_inflight"]
    );
    let pipe_names: Vec<String> = ack["pipes"]
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    println!("✅ 可见管道：{pipe_names:?}");

    // 第 3 步：broker/GetPipe（建立 wb.request 通道）。
    let get_pipe = serde_json::json!({
        "jsonrpc": "2.0", "id": 1, "method": "broker/GetPipe",
        "params": {"pipe": "wb.request"},
    });
    let response = match roundtrip(&mut pipe, get_pipe.to_string().as_bytes()) {
        Ok(response) => response,
        Err(error) => {
            eprintln!("GetPipe 往返失败：{error}");
            return ExitCode::FAILURE;
        }
    };
    println!("✅ GetPipe(wb.request)：{}", redact(&response));

    // 第 4 步：通道调用 http.fetch（只读：签到活动状态）。
    let fetch = serde_json::json!({
        "jsonrpc": "2.0", "id": 2,
        "method": "c:wb.request/http.fetch",
        "params": {
            "method": "POST",
            "path": "/v2/billing/meter/checkin-activity-status",
            "headers": {"content-type": "application/json"},
        },
        "mode": "call",
    });
    let response = match roundtrip(&mut pipe, fetch.to_string().as_bytes()) {
        Ok(response) => response,
        Err(error) => {
            eprintln!("http.fetch 往返失败：{error}");
            return ExitCode::FAILURE;
        }
    };
    let value: serde_json::Value = match serde_json::from_slice(&response) {
        Ok(value) => value,
        Err(_) => {
            eprintln!("http.fetch 响应解析失败：{}", redact(&response));
            return ExitCode::FAILURE;
        }
    };
    let status = value["result"]["status"].as_u64().unwrap_or(0);
    println!(
        "✅ http.fetch 签到状态：HTTP {status}（body {} B，脱敏不打印）",
        value["result"]["body_b64"]
            .as_str()
            .map(|body| body.len() * 3 / 4)
            .unwrap_or(0)
    );
    println!("---- P17-A4 真机证据链：握手 ack + 通道 + 只读代理全部通过 ----");
    ExitCode::SUCCESS
}

/// 发送请求并阻塞读回一帧（newline-JSON；收到换行或 EOF 即返回）。
fn roundtrip(pipe: &mut std::fs::File, request: &[u8]) -> std::io::Result<Vec<u8>> {
    // newline-JSON 帧：服务端按 \n 分帧——调用方忘加帧尾时在此补齐。
    let mut framed = request.to_vec();
    if !framed.ends_with(b"\n") {
        framed.push(b'\n');
    }
    pipe.write_all(&framed)?;
    pipe.flush()?;
    let mut buffer = Vec::new();
    let mut chunk = [0_u8; 4096];
    loop {
        let read = pipe.read(&mut chunk)?;
        if read == 0 {
            break;
        }
        buffer.extend_from_slice(&chunk[..read]);
        if buffer.ends_with(b"\n") {
            break;
        }
    }
    Ok(buffer)
}

/// token/ticket 类键值脱敏（保留形态可读性；顺序应用于累计串）。
fn redact(bytes: &[u8]) -> String {
    let mut text = String::from_utf8_lossy(bytes).into_owned();
    for key in ["accessToken", "access_token", "refreshToken", "refresh_token", "ticket"] {
        text = redact_key(&text, key);
    }
    text
}

/// 轻量键值脱敏（不引 regex 依赖：扫描 `"key":"value"` 字面模式）。
fn redact_key(text: &str, key: &str) -> String {
    let needle = format!("\"{key}\":\"");
    let mut output = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find(&needle) {
        let value_start = start + needle.len();
        output.push_str(&rest[..value_start]);
        let tail = &rest[value_start..];
        match tail.find('"') {
            // 值长度 > 8 才截断展示（短值如空串保持原样）。
            Some(end) if end > 8 => {
                output.push_str(&tail[..8]);
                output.push_str(&format!("…(len={end})"));
                rest = &tail[end..];
            }
            Some(_) => {
                output.push('"');
                rest = &tail[1..];
            }
            None => {
                output.push_str(tail);
                rest = "";
            }
        }
    }
    output.push_str(rest);
    output
}

trait AbsOr {
    fn abs_or(self, fallback: PathBuf) -> PathBuf;
}

impl AbsOr for PathBuf {
    fn abs_or(self, fallback: PathBuf) -> PathBuf {
        if self.as_os_str().is_empty() {
            fallback
        } else {
            self
        }
    }
}
