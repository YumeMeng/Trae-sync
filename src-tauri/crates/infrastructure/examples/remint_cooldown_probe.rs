//! 重铸冷却探针：实测「重铸新设备 → 冷却 N 秒 → 首签」的最短有效冷却。
//!
//! 背景（TECHNICAL_BASELINE 2026-08-26）：9074 含频率维度，同 IP 短时间连续
//! 「铸造 + 首签」会被拒，当时以 4-10 分钟间隔的整链重试验证成功，App 侧
//! 固化为「重铸后冷却 180 秒」。本探针用当前未签账号按梯度 0/30/120 秒
//! 逐轮实测，回答「最短多少冷却够用」。
//!
//! 用法：`remint_cooldown_probe <profile_id> [cooldown_secs_csv]`
//! 默认梯度 `0,30,120`；材料根从 env `TRAE_SYNC_CHECKIN_ROOT` 读取，
//! 缺省回退 `%LOCALAPPDATA%\Trae Sync\data\checkin`。
//!
//! 每轮流程（与 App 的 RemintCheckinRunner 同语义）：
//! 1. 重读注册表构建 transport，status 确认今日仍未签（已签即终止）；
//! 2. 重铸新设备（AuthCode 模式，内联执行以暴露各阶段业务码；
//!    凭据原子写回，失败零回写——与 DeviceRemintService 一致）；
//! 3. 睡眠本轮冷却时长；
//! 4. 重建 transport（新设备绑定），status → 单次 claim → status 复核。
//!
//! 铁律：每轮 claim 最多一次（新设备的新 claim，非旧 claim 重发）；
//! Token/私钥/AuthCode 不进输出；stdout 输出一行汇总 JSON；
//! 建议总轮数 ≤ 3（2026-08-26 实测当日最多 3 次内全部成功）。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::json;
use traesync_infrastructure::account_registry::{AccountRecord, AccountRegistry};
use traesync_infrastructure::checkin_credential::{
    generate_device_keypair, CheckinCredentialBundle, CheckinCredentialStore,
    CheckinProfileBinding,
};
use traesync_infrastructure::checkin_http::{
    exchange_token_by_auth_code, get_pc_auth_code, trae_http_client, CheckinHttpError, OAuthClient,
    TokenGrant,
};
use traesync_infrastructure::checkin_http::RealCheckinTransport;
use traesync_infrastructure::checkin_login::{
    decode_account_from_jwt, generate_pkce_pair, generate_virtual_device_id,
};
use traesync_ports::{CheckinTransport, CheckinTransportError};

/// ExchangeToken 端点（与 checkin_http.rs 同源；探针内联复刻用于 403 诊断）。
const EXCHANGE_TOKEN_PATH: &str = "/trae/api/v3/oauth/ExchangeToken";
const API_BASE: &str = "https://api.trae.cn";
/// 浏览器形态 User-Agent（与 GetPCAuthCode 请求同款）。
const BROWSER_UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 Chrome/140.0.0.0 Safari/537.36";

/// DeviceInfoBlock -> 请求体 JSON（字段与 checkin_http.rs 私有 to_json 逐字段一致；
/// to_json 未导出，探针复刻以保持请求体完全相同）。
fn device_info_json(info: &traesync_infrastructure::checkin_http::DeviceInfoBlock) -> serde_json::Value {
    serde_json::json!({
        "DeviceID": info.device_id,
        "MachineID": info.machine_id,
        "PlatformCode": info.platform_code,
        "DeviceType": "PC",
        "DeviceName": info.device_name,
        "DeviceModel": info.device_model,
        "ClientVersion": info.client_version,
        "DevicePublicKey": info.device_public_key,
        "DeviceBrand": info.device_brand,
        "DeviceCPU": info.device_cpu,
        "OSInfo": info.os_info,
        "OSVersion": info.os_version,
    })
}

/// transport 错误 -> 结果码字符串（与 App 的 detail_code 同口径）。
fn error_code(error: &CheckinTransportError) -> String {
    match error {
        CheckinTransportError::Business(code) => format!("business_{code}"),
        CheckinTransportError::Network => "network_error".to_string(),
        CheckinTransportError::AuthMismatch => "auth_mismatch".to_string(),
        CheckinTransportError::CredentialRefreshFailed => "credential_refresh_failed".to_string(),
        CheckinTransportError::ProfileBusy => "profile_busy".to_string(),
        CheckinTransportError::Protocol => "protocol_error".to_string(),
        CheckinTransportError::Runtime => "runtime_error".to_string(),
    }
}

/// HTTP 阶段错误 -> 诊断码（区分网络/HTTP 状态/业务码/协议）。
fn http_error_code(error: &CheckinHttpError) -> String {
    match error {
        CheckinHttpError::Business(code) => format!("business_{code}"),
        CheckinHttpError::Network => "network".to_string(),
        CheckinHttpError::Http(status) => format!("http_{status}"),
        CheckinHttpError::Protocol => "protocol".to_string(),
    }
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

/// 随机 16 字节 hex 机器指纹（与登录时同形态；remint.rs 同款生成方式）。
fn random_machine_id() -> Result<String, String> {
    let mut bytes = [0u8; 16];
    openssl::rand::rand_bytes(&mut bytes).map_err(|_| "rand_failed".to_string())?;
    Ok(hex::encode(bytes))
}

/// ExchangeToken（含 403 诊断）。
/// 模式 `auto`：先走生产实现（无浏览器头）；403 时用浏览器头重试
///   （同一 AuthCode——403 若发生在网关层，AuthCode 应未被消费）。
/// 模式 `browser`：全新 AuthCode 直接用浏览器头请求（绕过生产实现，
///   用于验证「403 尝试是否消费了 AuthCode」假设）。
/// 诊断输出走 stderr；成功返回 TokenGrant，失败返回诊断码。
fn exchange_with_403_diagnosis(
    client: &reqwest::blocking::Client,
    auth_code: &str,
    code_verifier: &str,
    device_info: &traesync_infrastructure::checkin_http::DeviceInfoBlock,
    oauth_client: OAuthClient,
    browser_first: bool,
) -> Result<TokenGrant, String> {
    if !browser_first {
        // ---- 尝试 1：生产实现（与 App 完全一致的请求形态） ----
        match exchange_token_by_auth_code(client, auth_code, code_verifier, device_info, oauth_client) {
            Ok(grant) => {
                eprintln!("[exchange] 生产实现成功（无浏览器头）");
                return Ok(grant);
            }
            Err(CheckinHttpError::Http(403)) => {
                eprintln!("[exchange] 生产实现被 403 拒绝，尝试浏览器头重试…");
            }
            Err(error) => return Err(format!("exchange_{}", http_error_code(&error))),
        }
    }
    // ---- 浏览器头请求（auto 模式为重试；browser 模式为首次） ----
    let body = serde_json::json!({
        "ClientID": oauth_client.client_id(),
        "AuthCode": auth_code,
        "CodeVerifier": code_verifier,
        "DeviceInfo": device_info_json(device_info),
        "IDEVersion": oauth_client.ide_version(),
    });
    let response = client
        .post(format!("{API_BASE}{EXCHANGE_TOKEN_PATH}"))
        .header("Content-Type", "application/json")
        .header("User-Agent", BROWSER_UA)
        .header("Referer", "https://www.trae.cn/authorization")
        .header("Origin", "https://www.trae.cn")
        .json(&body)
        .send()
        .map_err(|_| "exchange_browser_network".to_string())?;
    let status = response.status().as_u16();
    let text = response.text().map_err(|_| "exchange_browser_body_failed".to_string())?;
    if status != 200 {
        // 响应体片段（截断；正常不含敏感值，仅为排障）。
        let snippet: String = text.chars().take(200).collect();
        eprintln!("[exchange] 浏览器头请求失败: http_{status} body={snippet}");
        return Err(format!("exchange_browser_http_{status}"));
    }
    // 解析成功响应（结构与生产 parse_exchange_envelope 相同）。
    let envelope: serde_json::Value =
        serde_json::from_str(&text).map_err(|_| "exchange_browser_protocol".to_string())?;
    if let Some(code) = envelope
        .pointer("/ResponseMetadata/Error/Code")
        .and_then(serde_json::Value::as_i64)
    {
        eprintln!("[exchange] 浏览器头请求业务码拒绝: business_{code}");
        return Err(format!("exchange_browser_business_{code}"));
    }
    let token = envelope
        .pointer("/Result/Token")
        .and_then(serde_json::Value::as_str)
        .ok_or("exchange_browser_protocol")?;
    let refresh = envelope
        .pointer("/Result/RefreshToken")
        .and_then(serde_json::Value::as_str)
        .ok_or("exchange_browser_protocol")?;
    let token_expire = envelope
        .pointer("/Result/TokenExpireAt")
        .and_then(serde_json::Value::as_u64)
        .ok_or("exchange_browser_protocol")?;
    let refresh_expire = envelope
        .pointer("/Result/RefreshExpireAt")
        .and_then(serde_json::Value::as_u64)
        .ok_or("exchange_browser_protocol")?;
    eprintln!("[exchange] 浏览器头请求成功");
    Ok(TokenGrant {
        access_token: token.to_string(),
        refresh_token: refresh.to_string(),
        access_token_expires_at_unix_seconds: token_expire / 1000,
        refresh_token_expires_at_unix_seconds: refresh_expire / 1000,
    })
}

/// 内联重铸（与 DeviceRemintService 同语义），逐段暴露业务码：
/// AuthCode 签发 → ExchangeToken → JWT 账号匹配 → 原子写回。
/// 任何失败零回写（旧凭据保持可用）；成功返回新设备 ID。
fn remint_with_diagnostics(
    material_root: &Path,
    profile_id: &str,
) -> Result<String, String> {
    let registry = AccountRegistry::new(material_root);
    let store = CheckinCredentialStore::new(material_root);
    let record = registry
        .load()
        .map_err(|_| "registry_load_failed".to_string())?
        .into_iter()
        .find(|record| record.profile_id == profile_id)
        .ok_or_else(|| "profile_not_found".to_string())?;
    let binding = CheckinProfileBinding::new(
        record.profile_id.clone(),
        record.account_id.clone(),
        record.device_id.clone(),
        record.device_public_key.clone(),
    );
    let bundle = store
        .load(&binding)
        .map_err(|_| "credential_unavailable".to_string())?;

    // ---- 全新设备四件套（成套生成，绝不复用旧值） ----
    let new_device_id = generate_virtual_device_id().map_err(|_| "device_id_failed".to_string())?;
    let new_machine_id = random_machine_id()?;
    let (new_private_key, new_public_key) =
        generate_device_keypair().map_err(|_| "keypair_failed".to_string())?;
    let pkce = generate_pkce_pair().map_err(|_| "pkce_failed".to_string())?;

    // ---- GetPCAuthCode + ExchangeToken（SOLO 形态；新设备首签仅此形态可行） ----
    let client = trae_http_client();
    let oauth_client = OAuthClient::Solo;
    let auth_code = get_pc_auth_code(
        &client,
        &bundle.access_token,
        &pkce.code_challenge,
        &new_device_id,
        oauth_client,
    )
    .map_err(|error| format!("auth_code_{}", http_error_code(&error)))?;
    let device_info =
        oauth_client.device_info(&new_device_id, &new_machine_id, &new_public_key);
    let grant = exchange_with_403_diagnosis(
        &client,
        &auth_code,
        &pkce.code_verifier,
        &device_info,
        oauth_client,
        std::env::var("PROBE_BROWSER_EXCHANGE_FIRST").is_ok(),
    )?;

    // ---- JWT 账号匹配校验（不匹配即硬失败，不写回） ----
    let (account_from_jwt, _) = decode_account_from_jwt(&grant.access_token)
        .map_err(|_| "jwt_decode_failed".to_string())?;
    if account_from_jwt != record.account_id {
        return Err("account_mismatch".to_string());
    }

    // ---- 凭据包 + 注册表原子写回（失败时保留旧凭据） ----
    let new_bundle = CheckinCredentialBundle {
        profile_id: record.profile_id.clone(),
        account_id: record.account_id.clone(),
        device_id: new_device_id.clone(),
        machine_id: new_machine_id,
        device_public_key: new_public_key.clone(),
        device_private_key: new_private_key,
        access_token: grant.access_token.clone(),
        refresh_token: grant.refresh_token.clone(),
        client_id: oauth_client.client_id().to_string(),
        access_token_expires_at_unix_seconds: grant.access_token_expires_at_unix_seconds,
        refresh_token_expires_at_unix_seconds: grant.refresh_token_expires_at_unix_seconds,
    };
    store
        .save(&new_bundle)
        .map_err(|_| "write_back_failed".to_string())?;
    let updated = AccountRecord {
        device_id: new_device_id.clone(),
        device_public_key: new_public_key,
        last_verified_at_unix_seconds: now_unix(),
        ..record
    };
    registry
        .upsert(&updated)
        .map_err(|_| "write_back_failed".to_string())?;
    Ok(new_device_id)
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        eprintln!("用法: remint_cooldown_probe <profile_id> [cooldown_secs_csv]");
        return ExitCode::FAILURE;
    }
    let profile_id = args[1].clone();
    // 冷却梯度：逗号分隔的秒数，按序逐轮尝试。
    let cooldowns: Vec<u64> = args
        .get(2)
        .map(|csv| csv.split(',').filter_map(|v| v.trim().parse().ok()).collect())
        .unwrap_or_else(|| vec![0, 30, 120]);
    if cooldowns.is_empty() {
        eprintln!("冷却梯度为空");
        return ExitCode::FAILURE;
    }

    // 材料根：env 优先，回退生产默认路径（与 App 的 storage_root/checkin 一致）。
    let material_root = match std::env::var("TRAE_SYNC_CHECKIN_ROOT") {
        Ok(root) => PathBuf::from(root),
        Err(_) => {
            let local_app_data = std::env::var("LOCALAPPDATA")
                .map(PathBuf::from)
                .unwrap_or_else(|_| PathBuf::from("."));
            local_app_data.join("Trae Sync").join("data").join("checkin")
        }
    };
    eprintln!(
        "重铸冷却探针启动: profile={} root={} cooldowns={:?}",
        profile_id,
        material_root.display(),
        cooldowns
    );

    let store = CheckinCredentialStore::new(&material_root);

    // status-only 模式：遍历注册表全部账号，只查 status（读操作），
    // 输出各账号今日签到状态——用于挑选「今日未签」的实测素材，
    // 不发起任何 claim / 重铸。
    if std::env::var("PROBE_STATUS_ONLY").is_ok() {
        let records = AccountRegistry::new(&material_root)
            .load()
            .map_err(|_| "registry_load_failed".to_string())
            .unwrap_or_default();
        let mut rows: Vec<serde_json::Value> = Vec::new();
        for record in &records {
            let binding = CheckinProfileBinding::new(
                record.profile_id.clone(),
                record.account_id.clone(),
                record.device_id.clone(),
                record.device_public_key.clone(),
            );
            let transport = RealCheckinTransport::new(
                &store,
                BTreeMap::from([(record.profile_id.clone(), binding)]),
            );
            let status = transport.status(&record.profile_id);
            // 凭据包的 client_id 决定设备形态（SOLO 可首签 / Work 被 9074 拒）；
            // client_id 是公开常量，不涉及敏感值。
            let form = store
                .load(&CheckinProfileBinding::new(
                    record.profile_id.clone(),
                    record.account_id.clone(),
                    record.device_id.clone(),
                    record.device_public_key.clone(),
                ))
                .ok()
                .map(|bundle| {
                    // v6：Work 通道已删除，client_id 只剩 SOLO 一种形态。
                    if bundle.client_id == traesync_infrastructure::TRAE_SOLO_CLIENT_ID {
                        "solo"
                    } else {
                        "unknown"
                    }
                });
            rows.push(json!({
                "profile_id": record.profile_id,
                "screen_name": record.screen_name,
                "device_form": form,
                "checked_in": status.as_ref().ok().map(|s| s.checked_in),
                "credits": status.as_ref().ok().map(|s| s.credits),
                "error": status.err().map(|error| error_code(&error)),
            }));
        }
        println!("{}", json!({"tool": "remint-cooldown-probe", "mode": "status-only", "accounts": rows}));
        return ExitCode::SUCCESS;
    }

    // 从注册表当前档案构建 transport（每轮重建：重铸后绑定指向新设备）。
    let build_transport = |material_root: &PathBuf| -> Result<RealCheckinTransport, String> {
        let record = AccountRegistry::new(material_root)
            .load()
            .map_err(|_| "registry_load_failed".to_string())?
            .into_iter()
            .find(|record| record.profile_id == profile_id)
            .ok_or_else(|| "profile_not_found".to_string())?;
        let binding = CheckinProfileBinding::new(
            record.profile_id.clone(),
            record.account_id.clone(),
            record.device_id.clone(),
            record.device_public_key.clone(),
        );
        Ok(RealCheckinTransport::new(
            &store,
            BTreeMap::from([(profile_id.clone(), binding)]),
        ))
    };

    let started = now_unix();
    let mut rounds: Vec<serde_json::Value> = Vec::new();
    let mut final_outcome = "not_attempted".to_string();
    let mut success_cooldown: Option<u64> = None;

    // claim-only 模式：跳过重铸，直接用当前设备走 status → 单次 claim → status。
    // 用于确认「当前设备是否还能正常签到」（与 App 常规签到路径一致）。
    if std::env::var("PROBE_CLAIM_ONLY").is_ok() {
        let transport = match build_transport(&material_root) {
            Ok(transport) => transport,
            Err(error) => {
                println!("{}", json!({"tool": "remint-cooldown-probe", "outcome": format!("abort:{error}")}));
                return ExitCode::SUCCESS;
            }
        };
        let before = transport.status(&profile_id);
        let claim = transport.claim(&profile_id);
        let after = transport.status(&profile_id).ok();
        let summary = json!({
            "tool": "remint-cooldown-probe",
            "mode": "claim-only",
            "executed_at_unix": started,
            "profile_id": profile_id,
            "status_before": before.as_ref().ok().map(|status| (status.checked_in, status.credits)),
            "status_before_error": before.as_ref().err().map(error_code),
            "claim_result": claim.as_ref().map(|snapshot| format!("business_{}", snapshot.business_code.unwrap_or(0))).unwrap_or_else(|error| error_code(error)),
            "checked_in_after": after.as_ref().map(|status| status.checked_in),
        });
        println!("{summary}");
        return ExitCode::SUCCESS;
    }

    for (index, cooldown) in cooldowns.iter().enumerate() {
        let round_no = index + 1;
        // ---- 阶段 1：确认今日仍未签 ----
        let transport = match build_transport(&material_root) {
            Ok(transport) => transport,
            Err(error) => {
                final_outcome = format!("abort:{error}");
                break;
            }
        };
        let before = match transport.status(&profile_id) {
            Ok(status) => status,
            Err(error) => {
                final_outcome = format!("abort:status_{}", error_code(&error));
                rounds.push(json!({
                    "round": round_no, "cooldown_secs": cooldown,
                    "stage": "status_before", "error": error_code(&error),
                }));
                break;
            }
        };
        if before.checked_in {
            // 上一轮已成功（或账号本就已签）：不再继续。
            final_outcome = "already_checked_in".to_string();
            rounds.push(json!({
                "round": round_no, "cooldown_secs": cooldown,
                "stage": "status_before", "checked_in": true,
            }));
            break;
        }

        // ---- 阶段 2：重铸新设备（内联诊断版） ----
        let mint_started = Instant::now();
        let remint = remint_with_diagnostics(&material_root, &profile_id);
        let mint_ms = mint_started.elapsed().as_millis() as u64;
        let new_device_id = match remint {
            Ok(device_id) => device_id,
            Err(error) => {
                // 重铸失败（网络/协议/业务码）：终止探针——继续重试只会
                // 叠加失败现场，具体业务码已透出，交给用户判断。
                final_outcome = format!("remint_failed:{error}");
                rounds.push(json!({
                    "round": round_no, "cooldown_secs": cooldown,
                    "stage": "remint", "mint_ms": mint_ms, "error": error,
                }));
                break;
            }
        };

        // ---- 阶段 3：冷却 ----
        if *cooldown > 0 {
            std::thread::sleep(Duration::from_secs(*cooldown));
        }

        // ---- 阶段 4：新设备重试一次完整签到（status → 单次 claim → status） ----
        let transport = match build_transport(&material_root) {
            Ok(transport) => transport,
            Err(error) => {
                final_outcome = format!("abort:{error}");
                break;
            }
        };
        let claim_result = transport.claim(&profile_id);
        let after = transport.status(&profile_id).ok();
        let claim_code = match &claim_result {
            Ok(snapshot) => format!("business_{}", snapshot.business_code.unwrap_or(0)),
            Err(error) => error_code(error),
        };
        let checked_in = after.as_ref().is_some_and(|status| status.checked_in);
        let outcome = if checked_in && claim_result.is_ok() {
            "claimed".to_string()
        } else if checked_in {
            // claim 报错但复核已签：网络抖动下的最终成功。
            "claimed_after_claim_error".to_string()
        } else {
            claim_code.clone()
        };

        rounds.push(json!({
            "round": round_no,
            "cooldown_secs": cooldown,
            "stage": "claim",
            "new_device_id": new_device_id,
            "mint_ms": mint_ms,
            "claim_result": claim_code,
            "checked_in_after": checked_in,
            "outcome": outcome,
        }));

        if checked_in {
            final_outcome = outcome;
            success_cooldown = Some(*cooldown);
            break;
        }
        // 仍被拒（典型 9074）：进入下一轮更长冷却。
    }

    let summary = json!({
        "tool": "remint-cooldown-probe",
        "executed_at_unix": started,
        "profile_id": profile_id,
        "outcome": final_outcome,
        "success_cooldown_secs": success_cooldown,
        "rounds": rounds,
    });
    println!("{summary}");
    ExitCode::SUCCESS
}
