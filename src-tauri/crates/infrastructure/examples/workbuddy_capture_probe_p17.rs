//! P17-T4c 捕获探针（面 A 解密链真机验证；测试 T2 用）。
//!
//! 流程：会话快照 → 借 WorkBuddy.exe 运行时取钥解密 token → JWT payload
//! 本地复核（uid/exp 与信封明文元数据对照）→ 脱敏报告。**全程不打印
//! token 明文**（只打印长度/形态/前 3 字符）；生产会话文件只读。
//!
//! 用法：`workbuddy_capture_probe_p17 [client_exe]`
//!   默认 = 常见安装路径探测（本机 `E:\软件\workbuddy\WorkBuddy.exe`）。

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use traesync_infrastructure::workbuddy_credential::{
    current_session_status, decrypt_session_tokens, jwt_payload_claims, load_session_snapshot,
    unix_ms_now, WorkBuddySessionStatus, DEFAULT_AUTH_ID,
};

/// 常见安装路径候选（命中第一个存在的 WorkBuddy.exe）。
fn client_exe_candidates() -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    // 本机实测安装位置（E 盘软件旁）。
    candidates.push(PathBuf::from(r"E:\软件\workbuddy\WorkBuddy.exe"));
    if let Ok(local) = std::env::var("LOCALAPPDATA") {
        candidates.push(
            Path::new(&local)
                .join("Programs")
                .join("workbuddy")
                .join("WorkBuddy.exe"),
        );
    }
    candidates
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    let client_exe = PathBuf::from(args.get(1).cloned().unwrap_or_default()).abs_or_first(
        client_exe_candidates()
            .into_iter()
            .find(|candidate| candidate.is_file()),
    );
    println!("客户端 exe：{}", client_exe.display());
    let now_ms = unix_ms_now();

    // 第 1 步：会话快照与健康判定（无需密钥）。
    let snapshot = match load_session_snapshot(DEFAULT_AUTH_ID) {
        Ok(snapshot) => snapshot,
        Err(error) => {
            eprintln!("中止：会话快照不可读（{}）", error.code());
            return ExitCode::FAILURE;
        }
    };
    println!("uid：{}", snapshot.uid);
    println!("uin：{}", snapshot.uin);
    println!(
        "expiresAt：{}（{}）",
        snapshot
            .auth
            .expires_at
            .map(|ms| ms.to_string())
            .unwrap_or_else(|| "未知".into()),
        snapshot
            .auth
            .expires_at
            .map(|ms| format_remaining(ms, now_ms))
            .unwrap_or_default()
    );

    // 第 2 步：解密（借客户端运行时取钥；客户端无需运行）。
    let tokens = match decrypt_session_tokens(
        &traesync_infrastructure::workbuddy_credential::session_file(DEFAULT_AUTH_ID)
            .expect("LOCALAPPDATA 可用（快照已读出）"),
        &client_exe,
    ) {
        Ok(tokens) => tokens,
        Err(error) => {
            eprintln!("中止：解密失败（{}）", error.code());
            if let WorkBuddyDecryptErrorShow::HelperFailed(reason) =
                WorkBuddyDecryptErrorShow::from(&error)
            {
                eprintln!("  原因：{reason}");
            }
            return ExitCode::FAILURE;
        }
    };
    println!("解密：access({:?}) refresh({:?})", tokens.access_source, tokens.refresh_source);
    println!(
        "  access：{} 字符（{}…）",
        tokens.access_token.len(),
        safe_prefix(&tokens.access_token)
    );
    println!(
        "  refresh：{} 字符（{}…）",
        tokens.refresh_token.len(),
        safe_prefix(&tokens.refresh_token)
    );

    // 第 3 步：JWT payload 本地复核（uid/exp 与信封明文对照）。
    let claims = jwt_payload_claims(&tokens.access_token);
    match claims {
        Some(claims) => {
            println!("access JWT payload 顶层键：{:?}", claim_keys(&claims));
            if let Some(exp) = claims.get("exp").and_then(|exp| exp.as_u64()) {
                // JWT exp 为秒；信封 expiresAt 为毫秒——统一到毫秒对照。
                let exp_ms = exp.saturating_mul(1000);
                println!("  JWT exp：{exp}（{}）", format_remaining(exp_ms, now_ms));
                match snapshot.auth.expires_at {
                    Some(envelope_exp) if envelope_exp.abs_diff(exp_ms) <= 60_000 => {
                        println!("  对照：JWT exp 与信封 expiresAt 一致（差 ≤60s）✓");
                    }
                    _ => println!("  对照：JWT exp 与信封 expiresAt 存在差异（需人工判读）"),
                }
            }
        }
        None => println!("access token 不是 JWT 三段形态（需人工判读）"),
    }

    let status = current_session_status(DEFAULT_AUTH_ID, now_ms);
    println!("---- 判定 ----");
    println!(
        "会话状态：{}",
        match status {
            WorkBuddySessionStatus::Active => "Active",
            WorkBuddySessionStatus::AccessExpired => "AccessExpired",
            WorkBuddySessionStatus::FullyExpired => "FullyExpired",
            WorkBuddySessionStatus::LoggedOut => "LoggedOut",
            WorkBuddySessionStatus::FileMissing => "FileMissing",
            WorkBuddySessionStatus::Unknown => "Unknown",
        }
    );
    println!("T2 捕获解密链验证完成（token 明文未打印、未落盘）。");
    ExitCode::SUCCESS
}

fn safe_prefix(token: &str) -> String {
    token.chars().take(3).collect()
}

fn claim_keys(claims: &serde_json::Value) -> Vec<String> {
    claims
        .as_object()
        .map(|object| object.keys().cloned().collect())
        .unwrap_or_default()
}

fn format_remaining(ms: u64, now_ms: u64) -> String {
    if ms <= now_ms {
        return format!("已过期（{} 天前）", (now_ms - ms) / 86_400_000);
    }
    let remaining = ms - now_ms;
    format!(
        "剩余 {} 天 {} 小时",
        remaining / 86_400_000,
        (remaining % 86_400_000) / 3_600_000
    )
}

/// 错误原因提取（仅 HelperFailed 有可读原因；其余错误码即足够）。
enum WorkBuddyDecryptErrorShow {
    HelperFailed(String),
    Other,
}

impl From<&traesync_infrastructure::workbuddy_credential::WorkBuddyDecryptError>
    for WorkBuddyDecryptErrorShow
{
    fn from(error: &traesync_infrastructure::workbuddy_credential::WorkBuddyDecryptError) -> Self {
        use traesync_infrastructure::workbuddy_credential::WorkBuddyDecryptError;
        match error {
            WorkBuddyDecryptError::HelperFailed(reason) => Self::HelperFailed(reason.clone()),
            _ => Self::Other,
        }
    }
}

trait AbsOrFirst {
    fn abs_or_first(self, fallback: Option<PathBuf>) -> PathBuf;
}

impl AbsOrFirst for PathBuf {
    fn abs_or_first(self, fallback: Option<PathBuf>) -> PathBuf {
        match fallback {
            Some(path) => path,
            None => self,
        }
    }
}
