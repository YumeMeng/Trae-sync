//! P17-T4c 会话健康探针（文件面，无需密钥、无需客户端运行）。
//!
//! 读取 WorkBuddy 登录会话（`%LOCALAPPDATA%\CodeBuddyExtension\Data\Public\auth`
//! 下 `{auth_id}.info`）的**明文元数据**并输出健康报告。只读零写入；不解析、
//! 不打印任何 token 值（信封字段仅报形态）。
//!
//! 用法：`workbuddy_session_probe_p17 [auth_id]`
//!   默认 auth_id = `workbuddy-desktop`（ACC 产品配置 authentication.id）。

use std::process::ExitCode;

use traesync_infrastructure::workbuddy_credential::{
    current_session_status, load_session_snapshot, logout_marker_file, unix_ms_now,
    WorkBuddySessionStatus, DEFAULT_AUTH_ID,
};

/// 剩余时间可读化（整数运算，不引入 chrono 依赖）。
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

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    let auth_id = args
        .get(1)
        .cloned()
        .unwrap_or_else(|| DEFAULT_AUTH_ID.to_string());
    let now_ms = unix_ms_now();
    println!("auth_id：{auth_id}");
    if let Some(marker) = logout_marker_file(&auth_id) {
        println!(
            "登出标记：{}",
            if marker.exists() {
                "存在（已登出）"
            } else {
                "无"
            }
        );
    }
    let snapshot = match load_session_snapshot(&auth_id) {
        Ok(snapshot) => snapshot,
        Err(error) => {
            eprintln!("会话快照不可用：{}（{error:?}）", error.code());
            return ExitCode::FAILURE;
        }
    };
    println!("---- 会话快照（明文元数据）----");
    println!("  uid：{}", snapshot.uid);
    println!("  uin：{}", snapshot.uin);
    println!("  tokenType：{}", snapshot.auth.token_type);
    println!("  domain：{}", snapshot.auth.domain);
    println!("  scope：{}", snapshot.auth.scope);
    println!("  sessionState：{}", snapshot.auth.session_state);
    println!("  expiresIn：{:?} 秒", snapshot.auth.expires_in);
    println!("  refreshExpiresIn：{:?} 秒", snapshot.auth.refresh_expires_in);
    match snapshot.auth.last_refresh_time {
        Some(ms) => println!("  最近刷新：{ms}（ms epoch）"),
        None => println!("  最近刷新：未知"),
    }
    match snapshot.auth.expires_at {
        Some(ms) => println!("  access 过期：{ms}（{}）", format_remaining(ms, now_ms)),
        None => println!("  access 过期：未知"),
    }
    match snapshot.auth.refresh_expires_at {
        Some(ms) => println!("  refresh 过期：{ms}（{}）", format_remaining(ms, now_ms)),
        None => println!("  refresh 过期：未知"),
    }
    println!("  accessToken 信封：{}", if snapshot.has_encrypted_access_token { "加密（值不读取）" } else { "非信封形态" });
    println!("  refreshToken 信封：{}", if snapshot.has_encrypted_refresh_token { "加密（值不读取）" } else { "非信封形态" });
    println!("  accounts 条目：{}", snapshot.account_count);
    println!("  allAccounts uid：{:?}", snapshot.all_account_uids);
    let status = current_session_status(&auth_id, now_ms);
    println!("---- 判定（now={now_ms}）----");
    match status {
        WorkBuddySessionStatus::Active => println!("  状态：Active（登录中，access 未过期）"),
        WorkBuddySessionStatus::AccessExpired => {
            println!("  状态：AccessExpired（access 过期，refresh 仍有效——客户端运行中会自动刷新）")
        }
        WorkBuddySessionStatus::FullyExpired => println!("  状态：FullyExpired（须重新登录）"),
        WorkBuddySessionStatus::LoggedOut => println!("  状态：LoggedOut（登出标记命中）"),
        WorkBuddySessionStatus::FileMissing => println!("  状态：FileMissing（无会话文件）"),
        WorkBuddySessionStatus::Unknown => println!("  状态：Unknown（读取/解析失败）"),
    }
    ExitCode::SUCCESS
}
