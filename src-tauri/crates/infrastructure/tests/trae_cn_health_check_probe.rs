//! Trae CN 健康检测只读探针（真机验收：check_trae_cn_account_health_inner 请求形状）。
//!
//! 背景：健康检测用 CN 自己的 OAuth client 调 GetUserInfo——真实联网实现但
//! 从未真机验收通过；09-28 签到探针对照组曾返回 401/20310（该对照组请求形状
//! 与健康检测不同，结论存疑）。本探针直接复用生产 transport 函数
//! `get_user_info_full_for_oauth_client`（health inner 的同一调用，形状零漂移），
//! 回答一个问题：**CN 凭据链路（DPAPI 凭据 → GetUserInfo 真实查询 → 身份一致）
//! 在真机上是否可用**。
//!
//! 运行方式（显式授权；`#[ignore]` 保证不进常规回归、不产生自动触网路径）：
//! ```text
//! $env:TRAE_SYNC_CN_HEALTH_PROBE_MATERIAL_ROOT = "<storage_root>\products\trae_cn"
//! cargo test -p traesync-infrastructure --test trae_cn_health_check_probe -- --ignored --nocapture
//! ```
//! 缺省环境变量时默认 `%LOCALAPPDATA%\Trae Sync\data\products\trae_cn`。
//! 凭据为 DPAPI 加密（当前 Windows 用户内存解密），无需源库 raw key。
//!
//! 红线纪律（与 trae_cn_checkin_probe 一致）：
//! - 只发 GetUserInfo **只读幂等**查询；claim/sign 端点一律不请求；
//! - 与 health inner 的唯一刻意偏离：**不回写** `last_verified_at`
//!   （探针零写纪律，跳过 registry.upsert）；
//! - 凭据只在内存解密使用（DPAPI → 内存，用完即弃），不落日志/报告；
//! - 报告只落指纹、形状摘要、HTTP/业务码与计数，不含 token/昵称原值/
//!   用户 ID 原值/响应原文。凭据不可用或端点拒绝同样如实落档，不伪造结论。

#![cfg(windows)]

use sha2::{Digest, Sha256};
use traesync_infrastructure::account_registry::{AccountRecord, AccountRegistry};
use traesync_infrastructure::product_account_state::ProductAccountStateStore;
use traesync_infrastructure::{
    get_user_info_full_for_oauth_client, probe_get_user_info_shape, trae_http_client,
    CheckinCredentialError, CheckinCredentialStore, CheckinHttpError, CheckinProfileBinding,
    OAuthClient, TraeCnProbeFinding,
};

/// 报告落档根：`<repo>/.scratch/checkin-http/reports/`（从 crate 位置上溯）。
fn probe_reports_dir() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .expect("crate 位于 <repo>/src-tauri/crates/infrastructure")
        .join(".scratch")
        .join("checkin-http")
        .join("reports")
}

/// 字符串的 sha256 前 8 位指纹（同文同哈希，反查不出原文）。
fn fingerprint8(value: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(value.as_bytes());
    hex::encode(&hasher.finalize()[..4])
}

fn unix_now_seconds() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// 探针报告（落档形态；全部字段可安全入档）。
#[derive(serde::Serialize)]
struct HealthProbeReport {
    probe: &'static str,
    /// 红线声明（随报告固化，供审阅时直接核对）。
    red_line: &'static str,
    material_root_present: bool,
    registry_accounts: usize,
    accounts: Vec<AccountHealthFinding>,
    conclusion: String,
    generated_at_unix_seconds: u64,
}

/// 单账号健康检测发现（脱敏：昵称/用户 ID 只落指纹与长度）。
#[derive(serde::Serialize)]
struct AccountHealthFinding {
    profile_id_fingerprint: String,
    /// 注册表侧昵称指纹（与服务端 screen_name 指纹对比）。
    registry_screen_name_fingerprint: String,
    /// 注册表侧账号 ID 指纹（与服务端 user_id 指纹对比，一致即 auth 通过）。
    registry_account_id_fingerprint: String,
    /// 凭据包 client_id 是否匹配 Trae CN（不匹配时按 health inner 记 credential_invalid）。
    client_id_matches_cn: bool,
    healthy: bool,
    /// health inner 同款错误码语义（None = healthy）。
    health_error_code: Option<String>,
    /// 服务端 user_id 与注册表 account_id 是否一致（仅 GetUserInfo 成功时有值）。
    auth_matches_registry: Option<bool>,
    user_id_fingerprint: Option<String>,
    user_id_len: Option<usize>,
    screen_name_fingerprint: Option<String>,
    screen_name_len: Option<usize>,
    avatar_url_present: bool,
    masked_mobile_present: bool,
    masked_email_present: bool,
    /// 区域代码（非敏感资料字段；空串不落）。
    region: Option<String>,
    ai_region: Option<String>,
    /// 原始响应形状取证（与生产调用同端点/同头/同体；crate 内同源常量构造）。
    raw_shape: Option<TraeCnProbeFinding>,
}

/// 凭据读取错误 → health inner 同款错误码
/// （对齐主 crate error_mapping.credential_maintenance_error_code）。
fn credential_error_code(error: &CheckinCredentialError) -> &'static str {
    match error {
        CheckinCredentialError::BindingMismatch => "binding_mismatch",
        CheckinCredentialError::CredentialRefreshFailed => "credential_refresh_failed",
        CheckinCredentialError::AuthMismatch => "auth_mismatch",
        CheckinCredentialError::RecoveryRequired => "manual_recovery_required",
        CheckinCredentialError::Missing => "credential_missing",
        CheckinCredentialError::Unavailable => "credential_unavailable",
        CheckinCredentialError::Invalid => "credential_invalid",
    }
}

/// GetUserInfo 错误 → health inner 同款错误码
/// （对齐主 crate error_mapping.trae_cn_health_error_code）。
fn health_error_code(error: &CheckinHttpError) -> String {
    match error {
        CheckinHttpError::Network => "network_error".to_string(),
        CheckinHttpError::Http(401 | 403) => "credential_invalid".to_string(),
        CheckinHttpError::Http(_) => "health_check_failed".to_string(),
        CheckinHttpError::Business(code) => format!("business_{code}"),
        CheckinHttpError::Protocol => "health_check_failed".to_string(),
    }
}

fn non_empty(value: String) -> Option<String> {
    if value.is_empty() { None } else { Some(value) }
}

/// 单账号只读健康检测：完整复刻 health inner 判定链（凭据 → client_id →
/// GetUserInfo → 身份一致），只在成功后追加原始形状取证；全程不写任何目录。
fn probe_account(
    store: &CheckinCredentialStore,
    client: &reqwest::blocking::Client,
    record: &AccountRecord,
) -> AccountHealthFinding {
    let mut finding = AccountHealthFinding {
        profile_id_fingerprint: fingerprint8(&record.profile_id),
        registry_screen_name_fingerprint: fingerprint8(&record.screen_name),
        registry_account_id_fingerprint: fingerprint8(&record.account_id),
        client_id_matches_cn: false,
        healthy: false,
        health_error_code: None,
        auth_matches_registry: None,
        user_id_fingerprint: None,
        user_id_len: None,
        screen_name_fingerprint: None,
        screen_name_len: None,
        avatar_url_present: false,
        masked_mobile_present: false,
        masked_email_present: false,
        region: None,
        ai_region: None,
        raw_shape: None,
    };

    // 第 1 步：DPAPI 内存解密凭据（health inner 同款 load）。
    let binding = CheckinProfileBinding::from(record);
    let bundle = match store.load(&binding) {
        Ok(bundle) => bundle,
        Err(error) => {
            finding.health_error_code = Some(credential_error_code(&error).to_string());
            return finding;
        }
    };
    // 第 2 步：client_id 跨产品防御（不匹配不触网，health inner 同款）。
    finding.client_id_matches_cn = bundle.client_id == OAuthClient::TraeCn.client_id();
    if !finding.client_id_matches_cn {
        finding.health_error_code = Some("credential_invalid".to_string());
        return finding;
    }
    // 第 3 步：生产同款调用——与 check_trae_cn_account_health_inner 完全相同的
    // transport 函数（同端点、同 OAuth client 参数、同头、同体）。
    match get_user_info_full_for_oauth_client(client, &bundle.access_token, OAuthClient::TraeCn) {
        Ok(info) => {
            finding.auth_matches_registry = Some(info.user_id == record.account_id);
            finding.user_id_fingerprint = Some(fingerprint8(&info.user_id));
            finding.user_id_len = Some(info.user_id.len());
            finding.screen_name_fingerprint = Some(fingerprint8(&info.screen_name));
            finding.screen_name_len = Some(info.screen_name.len());
            finding.avatar_url_present = !info.avatar_url.is_empty();
            finding.masked_mobile_present = !info.masked_mobile.is_empty();
            finding.masked_email_present = !info.masked_email.is_empty();
            finding.region = non_empty(info.region);
            finding.ai_region = non_empty(info.ai_region);
            if finding.auth_matches_registry == Some(true) {
                finding.healthy = true;
            } else {
                // 身份不一致：health inner 记 auth_mismatch，不回写验证时刻。
                finding.health_error_code = Some("auth_mismatch".to_string());
            }
        }
        Err(error) => {
            finding.health_error_code = Some(health_error_code(&error));
        }
    }
    // 第 4 步：原始响应形状取证（同形状第二发只读请求；完整证据链）。
    finding.raw_shape = Some(probe_get_user_info_shape(client, &bundle.access_token));
    finding
}

/// 落档并打印结果（写失败 fail-soft：打印警示不中断探针）。
fn persist_report(report: &HealthProbeReport) {
    let json = match serde_json::to_string_pretty(report) {
        Ok(json) => json,
        Err(error) => {
            println!("报告序列化失败（{error}），结论 = {}", report.conclusion);
            return;
        }
    };
    let path = probe_reports_dir().join(format!(
        "trae-cn-health-probe-{}.json",
        report.generated_at_unix_seconds
    ));
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    match std::fs::write(&path, json) {
        Ok(()) => println!("报告已落档：{}", path.display()),
        Err(error) => println!("报告落档失败（{error}），仅打印：结论 = {}", report.conclusion),
    }
}

/// 真实探针：Trae CN 材料根 → DPAPI 解密 → 逐账号 GetUserInfo → 落档。
#[test]
#[ignore]
fn trae_cn_health_probe_with_explicit_material_root() {
    let material_root = std::env::var_os("TRAE_SYNC_CN_HEALTH_PROBE_MATERIAL_ROOT")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            std::env::var_os("LOCALAPPDATA")
                .map(|base| {
                    std::path::PathBuf::from(base)
                        .join("Trae Sync")
                        .join("data")
                        .join("products")
                        .join("trae_cn")
                })
                .expect("LOCALAPPDATA 缺失且未显式指定材料根")
        });
    let mut report = HealthProbeReport {
        probe: "trae-cn-health-probe",
        red_line: "只读探针：仅 GetUserInfo 幂等查询；未发送 claim/sign；不写产品目录（跳过 last_verified_at 回写）；报告不含 token/昵称原值/用户 ID 原值/响应原文",
        material_root_present: material_root.is_dir(),
        registry_accounts: 0,
        accounts: Vec::new(),
        conclusion: String::new(),
        generated_at_unix_seconds: unix_now_seconds(),
    };

    // 第 1 步：账号注册表（health inner 同款 load）。
    let records = match AccountRegistry::new(&material_root).load() {
        Ok(records) => records,
        Err(_) => {
            report.conclusion =
                "无法实测：Trae CN 账号注册表读取失败（材料根缺失或损坏）".to_string();
            persist_report(&report);
            return;
        }
    };
    report.registry_accounts = records.len();
    if records.is_empty() {
        report.conclusion = "无法实测：注册表为空（Trae CN 未登录过）".to_string();
        persist_report(&report);
        return;
    }
    // 当前指针（仅报告语境；health inner 按 selected 遍历，不依赖指针）。
    let pointer = ProductAccountStateStore::new(&material_root)
        .load()
        .unwrap_or(None);
    println!(
        "材料根：{}（存在={}）；注册账号 {} 个",
        material_root.display(),
        report.material_root_present,
        records.len()
    );
    println!(
        "当前指针：{}",
        match &pointer {
            Some(id) => format!("已选择（指纹 {}）", fingerprint8(id)),
            None => "未选择".to_string(),
        }
    );

    // 第 2 步：逐账号只读健康检测（对全部注册账号各测一次）。
    let store = CheckinCredentialStore::new(&material_root);
    let client = trae_http_client();
    for record in &records {
        let finding = probe_account(&store, &client, record);
        println!(
            "账号 {}（registry昵称指纹 {}）：{}；错误码 {:?}；auth一致 {:?}；服务端昵称指纹 {:?}（len {:?}）",
            finding.profile_id_fingerprint,
            finding.registry_screen_name_fingerprint,
            if finding.healthy { "健康" } else { "不健康" },
            finding.health_error_code,
            finding.auth_matches_registry,
            finding.screen_name_fingerprint,
            finding.screen_name_len,
        );
        if let Some(raw) = &finding.raw_shape {
            println!(
                "  原始形状：HTTP {:?}，业务码 {:?}，顶层字段 {:?}，len={}，sha256:{}",
                raw.http_status,
                raw.business_code,
                raw.body_top_level_keys,
                raw.body_byte_len,
                raw.body_sha256_8
            );
        }
        report.accounts.push(finding);
    }

    // 第 3 步：结论推导（凭据链路可用性判定）+ 落档。
    let total = report.accounts.len();
    let healthy = report.accounts.iter().filter(|a| a.healthy).count();
    report.conclusion = if total > 0 && healthy == total {
        format!(
            "验收通过：CN 凭据链路可用——{healthy}/{total} 个账号 GetUserInfo 真实查询成功且 user_id 与注册表一致"
        )
    } else if healthy > 0 {
        format!(
            "部分通过：{healthy}/{total} 个账号链路可用，其余账号错误码与原始响应形状见报告（需人工判读）"
        )
    } else {
        format!(
            "验收未通过：{total} 个账号全部失败（错误码与原始响应形状见报告）——CN 健康检测请求形状真机未通过"
        )
    };
    println!("探针结论：{}", report.conclusion);
    persist_report(&report);
}
