//! E2 供体路径探针：复现主库切号第 3 步 E2（凭据包 + GetUserInfo）逐段失败点。
//!
//! 背景（2026-09-03 卡死案例）：官方退出账号后切号报 switch_donor_login_missing，
//! 该错误码意味着 E2 与 E1 双路径全部失败。本探针只读运行 E2 的两个前置阶段：
//!   1. 凭据包加载（store.load，含绑定四元组校验）
//!   2. GetUserInfo 实调（与切换编排同一 HTTP 客户端与令牌）
//! 并顺带输出 needs_refresh 判定与到期剩余秒数，供续期缺口分析。
//!
//! 用法：`e2_donor_probe <profile_id>...`（可传多个）
//! 材料根从 env `TRAE_SYNC_CHECKIN_ROOT` 读取，缺省回退
//! `%LOCALAPPDATA%\Trae Sync\data\checkin`。
//!
//! 铁律：只读（零写回）；令牌/私钥不进输出；stdout 一行汇总 JSON。

use std::path::PathBuf;
use std::process::ExitCode;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::json;
use traesync_infrastructure::account_registry::{AccountRegistry, AccountRecord};
use traesync_infrastructure::checkin_credential::{
    needs_refresh, CheckinCredentialStore, CheckinProfileBinding,
};
use traesync_infrastructure::checkin_http::{
    get_user_info_full, trae_http_client, CheckinHttpError,
};

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

/// HTTP 阶段错误 -> 诊断码（与 remint_cooldown_probe 同口径）。
fn http_error_code(error: &CheckinHttpError) -> String {
    match error {
        CheckinHttpError::Business(code) => format!("business_{code}"),
        CheckinHttpError::Network => "network".to_string(),
        CheckinHttpError::Http(status) => format!("http_{status}"),
        CheckinHttpError::Protocol => "protocol".to_string(),
    }
}

/// 凭据包加载错误 -> 诊断码（保持枚举名小写蛇形）。
fn credential_error_code(error: &traesync_infrastructure::checkin_credential::CheckinCredentialError) -> String {
    format!("{error:?}")
        .to_ascii_lowercase()
        .replace('(', "_")
        .replace(')', "")
}

fn probe_record(record: &AccountRecord, store: &CheckinCredentialStore) -> serde_json::Value {
    let binding = CheckinProfileBinding::new(
        record.profile_id.clone(),
        record.account_id.clone(),
        record.device_id.clone(),
        record.device_public_key.clone(),
    );
    // ---- 阶段 1：凭据包加载（绑定四元组校验在此发生）----
    let bundle = match store.load(&binding) {
        Ok(bundle) => bundle,
        Err(error) => {
            return json!({
                "profile_id": record.profile_id,
                "screen_name": record.screen_name,
                "stage": "load",
                "error": credential_error_code(&error),
            })
        }
    };
    // ---- 阶段 1.5：续期缺口（只读判定，不执行续期）----
    let now = now_unix();
    let access_remaining = bundle
        .access_token_expires_at_unix_seconds
        .saturating_sub(now);
    let refresh_remaining = bundle
        .refresh_token_expires_at_unix_seconds
        .saturating_sub(now);
    // ---- 阶段 2：GetUserInfo 实调（与 switch_login_identity_dual_path 同源）----
    match get_user_info_full(&trae_http_client(), &bundle.access_token) {
        Ok(info) => json!({
            "profile_id": record.profile_id,
            "screen_name": record.screen_name,
            "stage": "get_user_info",
            "ok": true,
            "reported_user_id": info.user_id,
            "identity_match": info.user_id == record.account_id,
            "access_token_remaining_secs": access_remaining,
            "refresh_token_remaining_secs": refresh_remaining,
            "needs_refresh": needs_refresh(&bundle, now),
        }),
        Err(error) => json!({
            "profile_id": record.profile_id,
            "screen_name": record.screen_name,
            "stage": "get_user_info",
            "ok": false,
            "error": http_error_code(&error),
            "access_token_remaining_secs": access_remaining,
            "refresh_token_remaining_secs": refresh_remaining,
            "needs_refresh": needs_refresh(&bundle, now),
        }),
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        eprintln!("用法: e2_donor_probe <profile_id>...");
        return ExitCode::FAILURE;
    }
    // 材料根：env 优先，回退生产默认路径（与 remint_cooldown_probe 一致）。
    let material_root = match std::env::var("TRAE_SYNC_CHECKIN_ROOT") {
        Ok(root) => PathBuf::from(root),
        Err(_) => {
            let local_app_data = std::env::var("LOCALAPPDATA")
                .map(PathBuf::from)
                .unwrap_or_else(|_| PathBuf::from("."));
            local_app_data.join("Trae Sync").join("data").join("checkin")
        }
    };
    let records = AccountRegistry::new(&material_root)
        .load()
        .unwrap_or_default();
    let store = CheckinCredentialStore::new(&material_root);
    let mut rows = Vec::new();
    for profile_id in &args {
        match records.iter().find(|record| &record.profile_id == profile_id) {
            Some(record) => rows.push(probe_record(record, &store)),
            None => rows.push(json!({
                "profile_id": profile_id,
                "stage": "registry",
                "error": "profile_not_found",
            })),
        }
    }
    println!("{}", json!({ "tool": "e2-donor-probe", "accounts": rows }));
    ExitCode::SUCCESS
}
