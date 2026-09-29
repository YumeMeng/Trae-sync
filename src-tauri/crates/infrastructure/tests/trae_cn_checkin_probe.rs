//! P11-6 Trae CN 签到只读探针（ADR-0032 决策 4 门禁，真实凭据实测用）。
//!
//! 运行方式（显式授权；`#[ignore]` 保证不进常规回归、不产生自动触网路径）：
//! ```text
//! $env:TRAE_SYNC_TRAE_CN_PROBE_MATERIAL_ROOT = "<storage_root>\products\trae_cn"
//! cargo test -p traesync-infrastructure --test trae_cn_checkin_probe -- --ignored --nocapture
//! ```
//!
//! 红线纪律（与 checkin_probe 模块头注释一致）：
//! - 只发 status 类**只读**查询；claim/sign 端点一律不请求；
//! - 凭据只在内存解密使用（DPAPI → Vec，用完即弃）；
//! - 报告只落形状摘要与哈希，落档 `.scratch/checkin-http/reports/`。
//!
//! 凭据不可用（未登录/解密失败）同样是有效结论：写 `credential_available:
//! false` 报告（结论「凭据不可用，探针未完成」）并如实打印，不伪造结论。

#![cfg(windows)]

use traesync_infrastructure::{
    AccountRegistry, CheckinCredentialStore, CheckinProfileBinding, OAuthClient,
    TraeCnCheckinProbeReport, TraeCnProbeFinding,
    build_probe_report, derive_probe_conclusion, probe_trae_cn_status_endpoints,
    trae_http_client, write_probe_report,
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

/// 落档并打印结果摘要（写失败 fail-soft：打印警示不中断探针）。
fn persist_and_print(report: &TraeCnCheckinProbeReport) {
    match write_probe_report(&probe_reports_dir(), report) {
        Ok(path) => println!("报告已落档：{}", path.display()),
        Err(error) => println!("报告落档失败（{error}），仅打印：结论 = {}", report.conclusion),
    }
}

/// 构造「凭据不可用」报告（未登录/解密失败均落此形态，不伪造探针结论）。
fn credential_unavailable_report(reason: &str) -> TraeCnCheckinProbeReport {
    build_probe_report(
        false,
        0,
        Vec::<TraeCnProbeFinding>::new(),
        format!("凭据不可用，探针未完成（{reason}）"),
    )
}

/// 真实探针：Trae CN 材料根 → DPAPI 解密 → 候选端点只读探测 → 落档。
#[test]
#[ignore]
fn trae_cn_checkin_status_probe_with_explicit_material_root() {
    let material_root = match std::env::var("TRAE_SYNC_TRAE_CN_PROBE_MATERIAL_ROOT") {
        Ok(value) if !value.is_empty() => std::path::PathBuf::from(value),
        _ => {
            persist_and_print(&credential_unavailable_report(
                "未设置 TRAE_SYNC_TRAE_CN_PROBE_MATERIAL_ROOT",
            ));
            return;
        }
    };

    // 第 1 步：账号注册表（Trae CN 独立账号池）。
    let records = match AccountRegistry::new(&material_root).load() {
        Ok(records) => records,
        Err(_) => {
            persist_and_print(&credential_unavailable_report("账号注册表读取失败"));
            return;
        }
    };
    if records.is_empty() {
        persist_and_print(&credential_unavailable_report("注册表为空（Trae CN 未登录过）"));
        return;
    }
    // 探针只测第一个账号（探索性实验，最小足迹；结论对端点存在性成立）。
    let record = &records[0];
    println!(
        "探针目标：账号档案 {}（{} 个候选，共 {} 个注册账号，仅探测首个）",
        record.profile_id,
        traesync_infrastructure::candidate_endpoints().len(),
        records.len()
    );

    // 第 2 步：DPAPI 解密凭据（仅内存）。
    let binding = CheckinProfileBinding::new(
        &record.profile_id,
        &record.account_id,
        &record.device_id,
        &record.device_public_key,
    );
    let bundle = match CheckinCredentialStore::new(&material_root).load(&binding) {
        Ok(bundle) => bundle,
        Err(_) => {
            persist_and_print(&credential_unavailable_report("凭据包解密失败"));
            return;
        }
    };
    if bundle.client_id != OAuthClient::TraeCn.client_id() {
        persist_and_print(&credential_unavailable_report(
            "凭据包 client_id 与 Trae CN 形态不符（跨产品串用防御）",
        ));
        return;
    }
    println!(
        "凭据形态核对：client_id 匹配 Trae CN；access 期限位形状 len={}",
        bundle.access_token.len()
    );

    // 第 3 步：候选端点只读探测（status 类查询，无 claim/sign）。
    let client = trae_http_client();
    let findings = probe_trae_cn_status_endpoints(&client, &bundle.access_token, &bundle.device_id);
    for finding in &findings {
        println!(
            "{} → {}（HTTP {:?}，业务码 {:?}，字段 {:?}，len={}，sha256:{}）",
            finding.endpoint,
            finding.outcome,
            finding.http_status,
            finding.business_code,
            finding.body_field_presence,
            finding.body_byte_len,
            finding.body_sha256_8
        );
    }

    // 第 4 步：结论推导 + 落档（脱敏报告）。
    let conclusion = derive_probe_conclusion(&findings);
    let report = build_probe_report(true, 1, findings, conclusion);
    println!("探针结论：{}", report.conclusion);
    persist_and_print(&report);

    // 凭据代次清零：显式 drop（凭据只在内存的纪律收口）。
    drop(bundle);
}
