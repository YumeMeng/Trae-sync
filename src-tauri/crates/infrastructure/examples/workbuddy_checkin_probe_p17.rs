//! P17-T4c③ 签到直连端到端探针（Rust 实现真机验收；测试 T3-Rust 用）。
//!
//! 编排：会话快照 → 解密凭据 → status 查询 → 未签则 claim → status 复核。
//! 严格遵循签到纪律：只读 status 可重复；claim 幂等（code=10001 即已签）；
//! 复核以 today_checked_in 翻转为真实入账判定。token 明文不打印不落盘。
//!
//! 用法：`workbuddy_checkin_probe_p17 [client_exe]`
//!   默认 = 常见安装路径探测（本机 `E:\软件\workbuddy\WorkBuddy.exe`）。

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use traesync_infrastructure::workbuddy_checkin::{
    checkin_claim, checkin_status, WorkBuddyCheckinCredentials, WorkBuddyCheckinOutcome,
};
use traesync_infrastructure::workbuddy_credential::{
    decrypt_session_tokens, load_session_snapshot, unix_ms_now, WorkBuddySessionStatus,
    DEFAULT_AUTH_ID,
};

fn client_exe_candidates() -> Vec<PathBuf> {
    vec![
        PathBuf::from(r"E:\软件\workbuddy\WorkBuddy.exe"),
        std::env::var("LOCALAPPDATA")
            .ok()
            .map(|local| {
                Path::new(&local)
                    .join("Programs")
                    .join("workbuddy")
                    .join("WorkBuddy.exe")
            })
            .unwrap_or_default(),
    ]
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    let client_exe = PathBuf::from(args.get(1).cloned().unwrap_or_default()).abs_or_first(
        client_exe_candidates()
            .into_iter()
            .find(|candidate| candidate.is_file()),
    );
    let http = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .expect("reqwest client");

    // 1) 会话快照（明文元数据 + 登录态判定）。
    let snapshot = match load_session_snapshot(DEFAULT_AUTH_ID) {
        Ok(snapshot) => snapshot,
        Err(error) => {
            eprintln!("中止：会话快照不可读（{}）", error.code());
            return ExitCode::FAILURE;
        }
    };
    let status_now = traesync_infrastructure::workbuddy_credential::current_session_status(
        DEFAULT_AUTH_ID,
        unix_ms_now(),
    );
    println!("会话状态：{status_now:?}（uid={}）", snapshot.uid);
    if status_now != WorkBuddySessionStatus::Active {
        eprintln!("中止：会话非 Active（过期/登出状态下不发起签到）");
        return ExitCode::FAILURE;
    }

    // 2) 解密凭据（客户端运行时取钥；客户端可关可开）。
    let info_path = traesync_infrastructure::workbuddy_credential::session_file(DEFAULT_AUTH_ID)
        .expect("LOCALAPPDATA 可用（快照已读出）");
    let tokens = match decrypt_session_tokens(&info_path, &client_exe) {
        Ok(tokens) => tokens,
        Err(error) => {
            eprintln!("中止：解密失败（{}）", error.code());
            return ExitCode::FAILURE;
        }
    };
    let credentials = WorkBuddyCheckinCredentials::from_parts(&snapshot, tokens.access_token);
    println!("凭据就绪（domain={}）", credentials.domain);

    // 3) status 查询（只读）。
    let before = match checkin_status(&http, &credentials) {
        Ok(WorkBuddyCheckinOutcome::Status(status)) => status,
        Ok(other) => {
            eprintln!("中止：status 返回异常（{other:?}）");
            return ExitCode::FAILURE;
        }
        Err(error) => {
            eprintln!("中止：status 请求失败（{}）", error.code());
            return ExitCode::FAILURE;
        }
    };
    println!(
        "查询：today_checked_in={} 连签 {} 天（今日可领 {}）",
        before.today_checked_in, before.streak_days, before.daily_credit
    );

    // 4) 未签则 claim（幂等）；已签则跳过（纪律：不重复领取）。
    if before.today_checked_in {
        println!("今日已签，跳过领取（幂等纪律）。");
        return ExitCode::SUCCESS;
    }
    match checkin_claim(&http, &credentials) {
        Ok(WorkBuddyCheckinOutcome::Claimed(claim)) => {
            println!("领取成功：+{} 积分（连签 {} 天）", claim.credit, claim.streak_days);
        }
        Ok(WorkBuddyCheckinOutcome::AlreadyCheckedIn) => {
            println!("领取返回已签（幂等空操作）。");
        }
        Ok(other) => {
            eprintln!("中止：领取未成功（{other:?}）——不自动重试");
            return ExitCode::FAILURE;
        }
        Err(error) => {
            eprintln!("中止：claim 请求失败（{}）——不自动重试", error.code());
            return ExitCode::FAILURE;
        }
    }

    // 5) status 复核（today_checked_in 翻转 = 真实入账判定）。
    match checkin_status(&http, &credentials) {
        Ok(WorkBuddyCheckinOutcome::Status(after)) => {
            if after.today_checked_in {
                println!(
                    "复核：today_checked_in 翻转 ✓（连签 {} 天，周期累计 {}）",
                    after.streak_days, after.total_credits
                );
                ExitCode::SUCCESS
            } else {
                eprintln!("异常：领取响应成功但复核未翻转——按未入账处理，需人工核对");
                ExitCode::FAILURE
            }
        }
        Ok(other) => {
            eprintln!("异常：复核 status 返回异常（{other:?}）");
            ExitCode::FAILURE
        }
        Err(error) => {
            eprintln!("异常：复核请求失败（{}）", error.code());
            ExitCode::FAILURE
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
