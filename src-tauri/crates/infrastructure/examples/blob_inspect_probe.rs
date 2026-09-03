//! storage.json blob 明文结构探针：解密指定键的 blob 并打印明文 JSON（脱敏）。
//!
//! 用途（2026-09-03 登出态修复）：确认 `iCubeAuthInfo://usertag` 键的明文
//! 结构是否可由 GetUserInfo 资料 + 恒定值构造（与 cloudide 同法），决定
//! E2 全新构造登录态时是否需要同时写两个键。
//!
//! 用法：`blob_inspect_probe <instance_dir> <key>`
//! 例如：`blob_inspect_probe "C:\...\TRAE SOLO CN" "iCubeAuthInfo://usertag"`
//!
//! 铁律：只读；token 类长随机串截断显示，敏感字段脱敏。

use std::path::PathBuf;
use std::process::ExitCode;

use serde_json::json;
use traesync_infrastructure::decrypt_named_blob;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() != 2 {
        eprintln!("用法: blob_inspect_probe <instance_dir> <key>");
        return ExitCode::FAILURE;
    }
    let instance_dir = PathBuf::from(&args[0]);
    match decrypt_named_blob(&instance_dir, &args[1]) {
        Ok(plain) => {
            // 脱敏：长字符串（token/密钥类）截断到前 12 字符 + 长度标记。
            let value = serde_json::from_slice::<serde_json::Value>(&plain)
                .map(|v| mask_long_strings(&v))
                .unwrap_or_else(|_| {
                    json!({ "raw_utf8_lossy_prefix": String::from_utf8_lossy(&plain[..plain.len().min(400)]).to_string() })
                });
            println!("{}", json!({ "key": args[1], "plaintext": value }));
            ExitCode::SUCCESS
        }
        Err(error) => {
            println!("{}", json!({ "key": args[1], "error": format!("{error:?}") }));
            ExitCode::FAILURE
        }
    }
}

/// 递归把超过 40 字符的字符串值截断（脱敏 token 类字段）。
fn mask_long_strings(value: &serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::String(s) => {
            if s.len() > 40 {
                serde_json::Value::String(format!("{}...(len={})", &s[..12], s.len()))
            } else {
                value.clone()
            }
        }
        serde_json::Value::Object(map) => {
            let masked: serde_json::Map<String, serde_json::Value> = map
                .iter()
                .map(|(k, v)| (k.clone(), mask_long_strings(v)))
                .collect();
            serde_json::Value::Object(masked)
        }
        serde_json::Value::Array(arr) => {
            serde_json::Value::Array(arr.iter().map(mask_long_strings).collect())
        }
        _ => value.clone(),
    }
}
