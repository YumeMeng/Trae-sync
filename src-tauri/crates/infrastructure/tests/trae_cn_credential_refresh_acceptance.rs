//! Trae CN 真实续期专项验收探针（ADR-0030 门禁 / Phase 13 专项验收 4）。
//!
//! 回答一个问题：**Trae CN 凭据的真实换发链路在真机上是否可用**——
//! 同设备 AuthCode 换发（ADR-0027/P9-0 语义）→ 身份校验 → journal 原子发布 →
//! 新 token 健康复核，完整证据链各环节逐一核对。
//!
//! 直接复用生产的底层续期编排：生产命令 `refresh_trae_cn_credentials` →
//! `refresh_product_credentials_inner` → `RealCheckinRenewalService::
//! new_for_oauth_client(&store, OAuthClient::TraeCn)` + `renew_now`。
//! 本探针调用同一服务同一方法（形状零漂移）；批量循环与维护状态回写属于
//! App 编排层，与换发证据链无关且会多写产品目录，探针刻意不复制。
//!
//! 运行方式（显式授权；`#[ignore]` 保证不进常规回归）：
//! ```text
//! $env:TRAE_SYNC_CN_REFRESH_ACCEPTANCE_MATERIAL_ROOT = "<storage_root>\products\trae_cn"
//! $env:TRAE_SYNC_CN_REFRESH_ACCEPTANCE_AUTHORIZED = "yes"
//! cargo test -p traesync-infrastructure --test trae_cn_credential_refresh_acceptance -- --ignored --nocapture
//! ```
//! 缺材料根或授权位不是 `yes` 时不触网、不换发、不落验收报告。
//!
//! 红线纪律：
//! - **一次换发**：只对当前指针账号调用一次 `renew_now`，失败不重试、
//!   不批量、不伪造结论——失败形状本身就是验收结论；
//! - 探针自身不写产品目录：除生产换发路径自身的凭据写回与 journal 外，
//!   探针零写入（不回写 last_verified_at、不更新维护状态）；
//! - 报告脱敏：只落指纹、时间戳、SHA-256 与错误码，不含 token 原值、
//!   用户 ID 原值、昵称原值或响应原文；
//! - 凭据只在内存解密使用（DPAPI → 内存，用完即弃），不落日志。

#![cfg(windows)]

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use sha2::{Digest, Sha256};
use traesync_infrastructure::{
    get_user_info_full_for_oauth_client, trae_http_client, AccountRecord, AccountRegistry,
    CheckinCredentialError, CheckinCredentialStore, CheckinHttpError, CheckinProfileBinding,
    OAuthClient, ProductAccountStateStore, RealCheckinRenewalService,
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

/// 凭据读取错误 → 主 crate error_mapping 同款错误码语义。
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

/// GetUserInfo 错误 → health inner 同款错误码语义。
fn health_error_code(error: &CheckinHttpError) -> String {
    match error {
        CheckinHttpError::Network => "network_error".to_string(),
        CheckinHttpError::Http(401 | 403) => "credential_invalid".to_string(),
        CheckinHttpError::Http(_) => "health_check_failed".to_string(),
        CheckinHttpError::Business(code) => format!("business_{code}"),
        CheckinHttpError::Protocol => "health_check_failed".to_string(),
    }
}

/// JWT payload 非敏感事实：只取 iat/exp/data.id；解析失败如实记录。
/// account_id 仅用于内存比对，报告只落其指纹。
struct JwtFacts {
    parse_ok: bool,
    iat: Option<u64>,
    exp: Option<u64>,
    account_id: Option<String>,
}

fn jwt_facts(token: &str) -> JwtFacts {
    let parsed = token
        .split('.')
        .nth(1)
        .and_then(|segment| URL_SAFE_NO_PAD.decode(segment).ok())
        .and_then(|payload| serde_json::from_slice::<serde_json::Value>(&payload).ok());
    match parsed {
        Some(value) => JwtFacts {
            parse_ok: true,
            iat: value.get("iat").and_then(|v| v.as_u64()),
            exp: value.get("exp").and_then(|v| v.as_u64()),
            account_id: value
                .get("data")
                .and_then(|data| data.get("id"))
                .and_then(|id| id.as_str())
                .map(str::to_string),
        },
        None => JwtFacts {
            parse_ok: false,
            iat: None,
            exp: None,
            account_id: None,
        },
    }
}

/// 单个 JWT 的脱敏证据（进报告）。
#[derive(serde::Serialize)]
struct JwtEvidence {
    parse_ok: bool,
    iat: Option<u64>,
    exp: Option<u64>,
}

fn jwt_evidence(facts: &JwtFacts) -> JwtEvidence {
    JwtEvidence {
        parse_ok: facts.parse_ok,
        iat: facts.iat,
        exp: facts.exp,
    }
}

/// 单个凭据代次的脱敏证据（换发前后各一份）。
#[derive(serde::Serialize)]
struct GenerationEvidence {
    access_token_fingerprint: String,
    refresh_token_fingerprint: String,
    /// 整代次指纹：sha256(access + "\\n" + refresh) 前 8 位。
    generation_fingerprint: String,
    access_jwt: JwtEvidence,
    refresh_jwt: JwtEvidence,
    access_expires_at_unix_seconds: u64,
    refresh_expires_at_unix_seconds: u64,
}

fn generation_evidence(bundle: &traesync_infrastructure::CheckinCredentialBundle) -> GenerationEvidence {
    let mut hasher = Sha256::new();
    hasher.update(bundle.access_token.as_bytes());
    hasher.update(b"\n");
    hasher.update(bundle.refresh_token.as_bytes());
    let generation = hex::encode(&hasher.finalize()[..4]);
    GenerationEvidence {
        access_token_fingerprint: fingerprint8(&bundle.access_token),
        refresh_token_fingerprint: fingerprint8(&bundle.refresh_token),
        generation_fingerprint: generation,
        access_jwt: jwt_evidence(&jwt_facts(&bundle.access_token)),
        refresh_jwt: jwt_evidence(&jwt_facts(&bundle.refresh_token)),
        access_expires_at_unix_seconds: bundle.access_token_expires_at_unix_seconds,
        refresh_expires_at_unix_seconds: bundle.refresh_token_expires_at_unix_seconds,
    }
}

/// 逐项验收断言（全部通过才算验收通过）。
#[derive(serde::Serialize)]
struct CheckItem {
    name: &'static str,
    passed: bool,
}

/// journal manifest.json 的脱敏核对（manifest 类型 crate 私有，按 JSON 读）。
#[derive(serde::Serialize)]
struct JournalEvidence {
    manifest_present: bool,
    state: Option<String>,
    operation: Option<String>,
    profile_id_matches: Option<bool>,
    before_target_sha256_differ: Option<bool>,
    created_at_unix_seconds: Option<u64>,
}

#[derive(serde::Serialize)]
struct HealthRecheckEvidence {
    performed: bool,
    http_ok: bool,
    error_code: Option<String>,
    identity_matches_registry: Option<bool>,
    user_id_fingerprint: Option<String>,
    screen_name_fingerprint_matches_registry: Option<bool>,
}

/// 验收报告（落档形态；全部字段可安全入档）。
#[derive(serde::Serialize)]
struct RefreshAcceptanceReport {
    probe: &'static str,
    red_line: &'static str,
    adr_gate: &'static str,
    material_root_present: bool,
    pointer_profile_fingerprint: String,
    registry_account_id_fingerprint: String,
    renewal_path_taken: Option<&'static str>,
    pre: Option<GenerationEvidence>,
    renewal: RenewalEvidence,
    post: Option<GenerationEvidence>,
    journal: JournalEvidence,
    health_recheck: HealthRecheckEvidence,
    checks: Vec<CheckItem>,
    accepted: bool,
    conclusion: String,
    generated_at_unix_seconds: u64,
}

#[derive(serde::Serialize)]
struct RenewalEvidence {
    attempted: bool,
    success: bool,
    error_code: Option<String>,
    operation_id: Option<String>,
    refresh_token_rotated: Option<bool>,
}

impl RenewalEvidence {
    fn not_attempted() -> Self {
        Self {
            attempted: false,
            success: false,
            error_code: None,
            operation_id: None,
            refresh_token_rotated: None,
        }
    }
}

impl JournalEvidence {
    fn absent() -> Self {
        Self {
            manifest_present: false,
            state: None,
            operation: None,
            profile_id_matches: None,
            before_target_sha256_differ: None,
            created_at_unix_seconds: None,
        }
    }
}

impl HealthRecheckEvidence {
    fn not_performed() -> Self {
        Self {
            performed: false,
            http_ok: false,
            error_code: None,
            identity_matches_registry: None,
            user_id_fingerprint: None,
            screen_name_fingerprint_matches_registry: None,
        }
    }
}

/// 落档报告并返回落档路径（写失败 fail-soft：返回 None 并打印警示）。
fn persist_report(report: &RefreshAcceptanceReport) -> Option<std::path::PathBuf> {
    let json = serde_json::to_string_pretty(report).ok()?;
    let path = probe_reports_dir().join(format!(
        "trae-cn-refresh-acceptance-{}.json",
        report.generated_at_unix_seconds
    ));
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    match std::fs::write(&path, json) {
        Ok(()) => {
            println!("报告已落档：{}", path.display());
            Some(path)
        }
        Err(error) => {
            println!(
                "报告落档失败（{error}），仅打印：结论 = {}",
                report.conclusion
            );
            None
        }
    }
}

/// 阻断/失败收口：落档报告后以非零退出结束（失败形状即验收结论）。
fn fail_with(report: &RefreshAcceptanceReport) -> ! {
    let path = persist_report(report);
    panic!(
        "CN 续期专项验收未通过：{}；报告 = {}",
        report.conclusion,
        path
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "落档失败（见控制台结论）".to_string())
    );
}

/// 真实验收：当前指针账号一次 CN 凭据换发 + 完整证据链核对。
#[test]
#[ignore]
fn trae_cn_credential_refresh_acceptance_with_explicit_authorization() {
    // ===== 授权闸门：缺材料根或授权位不是 "yes" 时不触网、不换发、不落报告 =====
    let authorized = std::env::var("TRAE_SYNC_CN_REFRESH_ACCEPTANCE_AUTHORIZED")
        .map(|value| value == "yes")
        .unwrap_or(false);
    if !authorized {
        println!(
            "未获显式授权（TRAE_SYNC_CN_REFRESH_ACCEPTANCE_AUTHORIZED != \"yes\"）：\
             本次运行未触网、未换发、未落验收报告。"
        );
        return;
    }
    let material_root = match std::env::var("TRAE_SYNC_CN_REFRESH_ACCEPTANCE_MATERIAL_ROOT") {
        Ok(value) if !value.is_empty() => std::path::PathBuf::from(value),
        _ => {
            println!(
                "未设置 TRAE_SYNC_CN_REFRESH_ACCEPTANCE_MATERIAL_ROOT：\
                 本次运行未触网、未换发、未落验收报告。"
            );
            return;
        }
    };
    let mut report = RefreshAcceptanceReport {
        probe: "trae-cn-refresh-acceptance",
        red_line: "一次换发（不重试不批量）；探针自身零写产品目录（仅生产换发路径自身的凭据写回与 journal）；报告脱敏（只落指纹/时间戳/SHA-256/错误码，不含 token/用户 ID/昵称原值）",
        adr_gate: "ADR-0030 决策 4：真实 Trae CN 账号续期验收（refresh rotation + 原子发布证据）",
        material_root_present: material_root.is_dir(),
        pointer_profile_fingerprint: String::new(),
        registry_account_id_fingerprint: String::new(),
        renewal_path_taken: None,
        pre: None,
        renewal: RenewalEvidence::not_attempted(),
        post: None,
        journal: JournalEvidence::absent(),
        health_recheck: HealthRecheckEvidence::not_performed(),
        checks: Vec::new(),
        accepted: false,
        conclusion: String::new(),
        generated_at_unix_seconds: unix_now_seconds(),
    };

    // ===== 第 1 步：材料根 + 注册表 + 当前指针账号 =====
    let records = match AccountRegistry::new(&material_root).load() {
        Ok(records) => records,
        Err(_) => {
            report.conclusion = "无法实测：Trae CN 账号注册表读取失败（材料根缺失或损坏）".to_string();
            fail_with(&report);
        }
    };
    let pointer = match ProductAccountStateStore::new(&material_root).load() {
        Ok(Some(profile_id)) => profile_id,
        Ok(None) => {
            report.conclusion = "无法实测：Trae CN 当前账号指针未选择（先在 App 内选择 CN 账号）"
                .to_string();
            fail_with(&report);
        }
        Err(_) => {
            report.conclusion = "无法实测：Trae CN 当前账号指针读取失败".to_string();
            fail_with(&report);
        }
    };
    let record: &AccountRecord = match records.iter().find(|r| r.profile_id == pointer) {
        Some(record) => record,
        None => {
            report.conclusion = "无法实测：当前指针账号不在注册表中（指针与注册表不一致）".to_string();
            fail_with(&report);
        }
    };
    report.pointer_profile_fingerprint = fingerprint8(&record.profile_id);
    report.registry_account_id_fingerprint = fingerprint8(&record.account_id);
    println!(
        "验收目标：指针账号 {}（注册账号 {} 个，仅对指针账号换发一次）",
        report.pointer_profile_fingerprint,
        records.len()
    );

    // ===== 第 2 步：换发前代次取证（DPAPI 内存解密）+ 中断现场检查 =====
    let store = CheckinCredentialStore::new(&material_root);
    match store.has_pending_renewal() {
        Ok(false) => {}
        Ok(true) => {
            report.conclusion =
                "阻断：存在未收口的续期 journal（生产路径会拒绝换发并要求人工恢复）".to_string();
            fail_with(&report);
        }
        Err(_) => {
            report.conclusion = "阻断：续期 journal 状态检查失败".to_string();
            fail_with(&report);
        }
    }
    let binding = CheckinProfileBinding::from(record);
    let bundle_before = match store.load(&binding) {
        Ok(bundle) => bundle,
        Err(error) => {
            report.conclusion = format!(
                "无法实测：换发前凭据读取失败（{}）",
                credential_error_code(&error)
            );
            fail_with(&report);
        }
    };
    let client_matches_cn = bundle_before.client_id == OAuthClient::TraeCn.client_id();
    if !client_matches_cn {
        report.conclusion =
            "阻断：凭据包 client_id 与 Trae CN 形态不符（跨产品串用防御，不触网）".to_string();
        fail_with(&report);
    }
    report.renewal_path_taken = Some(
        if bundle_before.access_token_expires_at_unix_seconds > unix_now_seconds() {
            "same_device_auth_code"
        } else {
            "refresh_rescue"
        },
    );
    report.pre = Some(generation_evidence(&bundle_before));
    println!(
        "换发前代次：generation {}，access 指纹 {}，路径 = {:?}",
        report.pre.as_ref().unwrap().generation_fingerprint,
        report.pre.as_ref().unwrap().access_token_fingerprint,
        report.renewal_path_taken.unwrap()
    );

    // ===== 第 3 步：一次真实换发（生产底层编排；失败不重试） =====
    let now = unix_now_seconds();
    let renewal = RealCheckinRenewalService::new_for_oauth_client(&store, OAuthClient::TraeCn);
    let receipt = match renewal.renew_now(&binding, now) {
        Ok(receipt) => {
            report.renewal = RenewalEvidence {
                attempted: true,
                success: true,
                error_code: None,
                operation_id: Some(receipt.operation_id.clone()),
                refresh_token_rotated: Some(receipt.refresh_token_rotated),
            };
            println!(
                "换发成功：operation_id = {}，refresh_token_rotated = {}",
                receipt.operation_id, receipt.refresh_token_rotated
            );
            receipt
        }
        Err(error) => {
            // 失败如实落档：错误形状与业务码就是验收结论，不重试、不伪造。
            let code = credential_error_code(&error).to_string();
            report.renewal = RenewalEvidence {
                attempted: true,
                success: false,
                error_code: Some(code.clone()),
                operation_id: None,
                refresh_token_rotated: None,
            };
            report.conclusion =
                format!("验收未通过：真实换发失败（{code}）；错误形状已落档，不重试");
            fail_with(&report);
        }
    };

    // ===== 第 4 步：换发后代次取证 + 逐项验收断言 =====
    let bundle_after = match store.load(&binding) {
        Ok(bundle) => bundle,
        Err(error) => {
            report.conclusion = format!(
                "验收未通过：换发后凭据重读失败（{}）——原子发布后无法取回新代次",
                credential_error_code(&error)
            );
            fail_with(&report);
        }
    };
    let post = generation_evidence(&bundle_after);
    let pre = report.pre.as_ref().expect("换发前证据已落");
    let access_jwt_after = jwt_facts(&bundle_after.access_token);
    let mut checks: Vec<CheckItem> = Vec::new();
    let push_check = |checks: &mut Vec<CheckItem>, name: &'static str, passed: bool| {
        println!("断言 [{name}]：{}", if passed { "通过" } else { "失败" });
        checks.push(CheckItem { name, passed });
    };

    // 4.1 新代次：两个 token 必须整体轮换（禁止跨代次拼接的半轮换）。
    let access_rotated = bundle_after.access_token != bundle_before.access_token;
    let refresh_rotated = bundle_after.refresh_token != bundle_before.refresh_token;
    push_check(&mut checks, "access_token_rotated", access_rotated);
    push_check(&mut checks, "refresh_token_rotated", refresh_rotated);
    push_check(
        &mut checks,
        "generation_fingerprint_changed",
        post.generation_fingerprint != pre.generation_fingerprint,
    );
    push_check(
        &mut checks,
        "client_id_preserved_cn",
        bundle_after.client_id == OAuthClient::TraeCn.client_id(),
    );

    // 4.2 新 access JWT 可解出身份与寿命：data.id 与账号档案一致（ADR-0027）。
    let identity_ok = access_jwt_after
        .account_id
        .as_deref()
        .map(|id| id == record.account_id)
        .unwrap_or(false);
    push_check(&mut checks, "access_jwt_parse_ok", access_jwt_after.parse_ok);
    push_check(&mut checks, "identity_matches_registry", identity_ok);
    // 4.3 新签发证据：寿命字段整体前移（服务端重新计算有效期）。
    push_check(
        &mut checks,
        "access_lifetime_extended",
        post.access_expires_at_unix_seconds > pre.access_expires_at_unix_seconds,
    );
    push_check(
        &mut checks,
        "refresh_lifetime_extended",
        post.refresh_expires_at_unix_seconds > pre.refresh_expires_at_unix_seconds,
    );
    // iat（签发时刻）非强制字段：服务端若提供则记录并核对前移，缺失时不阻断
    //（token 整体轮换 + exp 前移已构成新签发证据）。
    if let (Some(pre_iat), Some(post_iat)) = (pre.access_jwt.iat, post.access_jwt.iat) {
        push_check(&mut checks, "access_jwt_iat_advanced", post_iat > pre_iat);
    }

    // ===== 第 5 步：journal 原子发布核对（<材料根>\renewals\{operation_id}） =====
    let journal_dir = store
        .root()
        .join("renewals")
        .join(&receipt.operation_id);
    let manifest_path = journal_dir.join("manifest.json");
    let manifest: Option<serde_json::Value> = std::fs::read(&manifest_path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok());
    match &manifest {
        Some(manifest) => {
            let state = manifest
                .get("state")
                .and_then(|v| v.as_str())
                .map(str::to_string);
            let operation = manifest
                .get("operation")
                .and_then(|v| v.as_str())
                .map(str::to_string);
            let profile_matches = manifest
                .get("profile_id")
                .and_then(|v| v.as_str())
                .map(|pid| pid == binding.profile_id);
            let sha_differ = match (
                manifest.get("before_sha256").and_then(|v| v.as_str()),
                manifest.get("target_sha256").and_then(|v| v.as_str()),
            ) {
                (Some(before), Some(target)) => Some(before != target),
                _ => None,
            };
            push_check(
                &mut checks,
                "journal_manifest_state_verified",
                state.as_deref() == Some("verified"),
            );
            push_check(
                &mut checks,
                "journal_profile_id_matches",
                profile_matches.unwrap_or(false),
            );
            push_check(
                &mut checks,
                "journal_sha256_differ",
                sha_differ.unwrap_or(false),
            );
            report.journal = JournalEvidence {
                manifest_present: true,
                state,
                operation,
                profile_id_matches: profile_matches,
                before_target_sha256_differ: sha_differ,
                created_at_unix_seconds: manifest
                    .get("created_at_unix_seconds")
                    .and_then(|v| v.as_u64()),
            };
        }
        None => {
            push_check(&mut checks, "journal_manifest_present", false);
        }
    }
    // 收口检查：全部 journal 均为终态（无 prepared/replaced 残留）。
    let no_pending = store.has_pending_renewal().unwrap_or(true) == false;
    push_check(&mut checks, "journal_no_pending_after", no_pending);
    report.post = Some(post);

    // ===== 第 6 步：健康检测复核（新 token GetUserInfo 200 + 身份一致） =====
    let client = trae_http_client();
    match get_user_info_full_for_oauth_client(&client, &bundle_after.access_token, OAuthClient::TraeCn)
    {
        Ok(info) => {
            let identity_ok = info.user_id == record.account_id;
            report.health_recheck = HealthRecheckEvidence {
                performed: true,
                http_ok: true,
                error_code: None,
                identity_matches_registry: Some(identity_ok),
                user_id_fingerprint: Some(fingerprint8(&info.user_id)),
                screen_name_fingerprint_matches_registry: Some(
                    fingerprint8(&info.screen_name) == fingerprint8(&record.screen_name),
                ),
            };
            push_check(&mut checks, "health_recheck_identity_ok", identity_ok);
        }
        Err(error) => {
            report.health_recheck = HealthRecheckEvidence {
                performed: true,
                http_ok: false,
                error_code: Some(health_error_code(&error)),
                identity_matches_registry: None,
                user_id_fingerprint: None,
                screen_name_fingerprint_matches_registry: None,
            };
            push_check(&mut checks, "health_recheck_identity_ok", false);
        }
    }
    report.checks = checks;

    // ===== 第 7 步：验收判定 + 落档 =====
    let failed: Vec<&str> = report
        .checks
        .iter()
        .filter(|item| !item.passed)
        .map(|item| item.name)
        .collect();
    report.accepted = failed.is_empty();
    report.conclusion = if report.accepted {
        format!(
            "验收通过：CN 真实续期证据链完整（路径 {:?} → 身份一致 → journal verified → 健康复核一致；\
             代次 {} → {}）",
            report.renewal_path_taken,
            pre.generation_fingerprint,
            report.post.as_ref().expect("换发后证据已落").generation_fingerprint
        )
    } else {
        format!("验收未通过：断言失败项 {failed:?}（详见报告）")
    };
    println!("探针结论：{}", report.conclusion);
    let path = persist_report(&report);
    assert!(
        report.accepted,
        "CN 续期专项验收未通过：{}；报告 = {}",
        report.conclusion,
        path
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "落档失败（见控制台结论）".to_string())
    );
}
