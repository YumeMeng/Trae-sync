//! P17-T4c 凭据层运行时探针（面 B：wbipc RPC 协议发现）。
//!
//! 用途：WorkBuddy **运行中**时，连接 wbipc broker（endpoint.json）做协议
//! 发现——枚举可见服务管道（broker/ListPipes）、解析目标管道方法表
//! （broker/GetPipe），为捕获编排（auth:getToken/getAccount）与账号切换
//! （auth:login/logout/saveOauthToken）的客户端实现定协议。
//!
//! 安全边界：**只读探测**——仅调用 broker/ListPipes、broker/GetPipe 与
//! 只读 auth 查询候选（auth:getAccount/auth:getCheckinStatus）；不调用
//! 任何写方法（login/logout/saveOauthToken/claimDailyCheckin/revokeAll）。
//! 响应打印前对 token/ticket 类值脱敏。
//!
//! 注意：命名管道阻塞读（无 overlapped 超时）；服务器无响应时 Ctrl+C 终止。
//!
//! 前置：WorkBuddy 桌面客户端运行中（wbipc broker 由主进程持有；
//! endpoint.json 的 ticket 随每次启动轮换）。
//!
//! 用法：`wbipc_probe_p17 [endpoint_json]`
//!   默认 = `~/.workbuddy/wbipc/endpoint.json`。

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

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

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    let path = PathBuf::from(args.get(1).cloned().unwrap_or_default()).abs_or(default_endpoint());
    println!("endpoint.json：{}", path.display());
    let Ok(raw) = std::fs::read_to_string(&path) else {
        eprintln!("中止：endpoint.json 不可读（WorkBuddy 可能未运行——broker 随主进程创建）");
        return ExitCode::FAILURE;
    };
    let endpoint: EndpointFile = match serde_json::from_str(&raw) {
        Ok(endpoint) => endpoint,
        Err(_) => {
            eprintln!("中止：endpoint.json 形态不符（{{endpoint, ticket}}）");
            return ExitCode::FAILURE;
        }
    };
    println!(
        "管道：{}（ticket len={}，值不打印）",
        endpoint.endpoint,
        endpoint.ticket.len()
    );

    // 连接命名管道（Windows：std fs 直接打开 \\.\pipe\ 路径）。
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

    // 探测 1：broker/ListPipes（无参数，只读——枚举可见服务管道）。
    let list_request = format!(
        "{{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"broker/ListPipes\",\"params\":{{}},\"ticket\":\"{}\"}}\n",
        endpoint.ticket
    );
    let response = match roundtrip(&mut pipe, list_request.as_bytes()) {
        Ok(response) => response,
        Err(error) => {
            eprintln!("ListPipes 往返失败：{error}");
            return ExitCode::FAILURE;
        }
    };
    println!(
        "ListPipes 响应（{} 字节）：{}",
        response.len(),
        redact(&response)
    );

    // 探测 2：broker/GetPipe（params.pipe = ListPipes 结果首个候选）。
    let Some(pipe_name) = first_pipe_name(&response) else {
        println!("未从 ListPipes 提取到管道名——协议形态见上方原始响应，请人工判读。");
        return ExitCode::SUCCESS;
    };
    let get_request = format!(
        "{{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"broker/GetPipe\",\"params\":{{\"pipe\":\"{pipe_name}\"}},\"ticket\":\"{}\"}}\n",
        endpoint.ticket
    );
    match roundtrip(&mut pipe, get_request.as_bytes()) {
        Ok(response) => println!(
            "GetPipe({pipe_name}) 响应（{} 字节）：{}",
            response.len(),
            redact(&response)
        ),
        Err(error) => eprintln!("GetPipe 往返失败：{error}"),
    }
    ExitCode::SUCCESS
}

/// 发送请求并阻塞读回一帧（newline-JSON 假设；收到换行或 EOF 即返回）。
fn roundtrip(pipe: &mut std::fs::File, request: &[u8]) -> std::io::Result<Vec<u8>> {
    pipe.write_all(request)?;
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

/// 从 ListPipes 响应提取首个管道名（形态假设 {"result":{"pipes":[…]}}，
/// 元素为字符串或 {name} 对象——两形态都试）。
fn first_pipe_name(response: &[u8]) -> Option<String> {
    let value: serde_json::Value = serde_json::from_slice(response).ok()?;
    let pipes = value
        .get("result")
        .and_then(|result| result.get("pipes"))
        .and_then(|pipes| pipes.as_array())?;
    for entry in pipes {
        if let Some(name) = entry.as_str() {
            return Some(name.to_string());
        }
        if let Some(name) = entry.get("name").and_then(|name| name.as_str()) {
            return Some(name.to_string());
        }
    }
    None
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
