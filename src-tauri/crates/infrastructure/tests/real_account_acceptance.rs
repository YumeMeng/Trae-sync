//! 真实账号受控验收（T17/T19）：基于本机已登录账号的实测凭证样本验证真实链路。
//!
//! 运行方式（显式授权；`#[ignore]` 保证不进常规回归）：
//! ```text
//! $env:TRAE_SYNC_REAL_CREDENTIAL_SAMPLE = "E:\...\credential-sample-20260821.json"
//! # 可选：设置 TRAE_SYNC_REAL_CLAIM=1 完成完整签到闭环（真实领取当日积分）
//! cargo test -p traesync-infrastructure --test real_account_acceptance -- --ignored --nocapture
//! ```
//!
//! 验证链路：样本 JSON -> 凭据包构建 -> DPAPI 加密入库 -> 解密读取 + 绑定校验 ->
//! 真实设备密钥签名/验签 roundtrip -> 续期阈值判定 -> 真实 status 查询 ->
//! （可选）真实 claim -> GetUserInfo 身份核验。
//!
//! 安全边界：
//! - 永不调用 `ExchangeToken` refresh 模式：避免轮换真实客户端的 refreshToken 导致其掉线；
//! - claim 步骤必须由环境变量 `TRAE_SYNC_REAL_CLAIM=1` 显式授权；
//! - 输出不含 token、refreshToken 或密钥内容（只打印长度与非敏感业务字段）。

#![cfg(windows)]

use std::collections::BTreeMap;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Deserialize;
use traesync_infrastructure::{
    device_proof_signing_input, get_user_info, needs_refresh, sign_device_proof, trae_http_client,
    verify_device_proof, AccountRecord, AccountRegistry, CheckinCredentialBundle,
    CheckinCredentialStore, CheckinProfileBinding, RealCheckinTransport, TRAE_SOLO_CLIENT_ID,
};
use traesync_ports::CheckinTransport;

/// 实测凭证样本（协议逆向时从真实客户端抓取）的非敏感字段视图。
#[derive(Deserialize)]
struct CredentialSample {
    #[serde(rename = "userId")]
    user_id: String,
    #[serde(rename = "deviceId")]
    device_id: String,
    #[serde(rename = "machineId")]
    machine_id: String,
    token: String,
    #[serde(rename = "refreshToken")]
    refresh_token: String,
    #[serde(rename = "jwtPayload")]
    jwt_payload: JwtPayload,
    #[serde(rename = "refreshExpiredAt")]
    refresh_expired_at: String,
    #[serde(rename = "deviceKeys")]
    device_keys: DeviceKeys,
    account: SampleAccount,
}

#[derive(Deserialize)]
struct SampleAccount {
    username: String,
    #[serde(default)]
    avatar_url: String,
}

#[derive(Deserialize)]
struct JwtPayload {
    exp: u64,
}

#[derive(Deserialize)]
struct DeviceKeys {
    #[serde(rename = "privateKeyPEM")]
    private_key_pem: String,
    #[serde(rename = "publicKeyPEM")]
    public_key_pem: String,
}

fn now_unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

/// ISO 8601 时刻（`YYYY-MM-DDTHH:MM:SS(.mmm)Z`）转 Unix 秒（Howard Hinnant 算法）。
fn iso_to_unix_seconds(iso: &str) -> Option<u64> {
    let date_time = iso.split('.').next()?;
    let (date, time) = date_time.split_once('T')?;
    let (year, month_day) = date.split_once('-')?;
    let (month, day) = month_day.split_once('-')?;
    let (hour, min_sec) = time.split_once(':')?;
    let (minute, second) = min_sec.split_once(':')?;
    let year: i64 = year.parse().ok()?;
    let month: i64 = month.parse().ok()?;
    let day: i64 = day.parse().ok()?;
    let hour: i64 = hour.parse().ok()?;
    let minute: i64 = minute.parse().ok()?;
    let second: i64 = second.parse().ok()?;
    // days_from_civil：公历日期 -> 自 1970-01-01 起的天数
    let y = if month <= 2 { year - 1 } else { year };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146097 + doe - 719468;
    Some((days * 86400 + hour * 3600 + minute * 60 + second) as u64)
}

fn load_sample() -> CredentialSample {
    let path = std::env::var("TRAE_SYNC_REAL_CREDENTIAL_SAMPLE")
        .expect("必须设置 TRAE_SYNC_REAL_CREDENTIAL_SAMPLE 指向实测凭证样本 JSON");
    let content = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("读取样本失败（{path}）：{error}"));
    serde_json::from_str(&content).expect("样本 JSON 结构与实测格式不符")
}

#[test]
#[ignore = "真实账号受控验收：需显式提供凭证样本"]
fn real_account_readonly_acceptance() {
    let sample = load_sample();
    let now = now_unix_seconds();
    let refresh_expires_at =
        iso_to_unix_seconds(&sample.refresh_expired_at).expect("refreshExpiredAt 解析失败");

    // 1. 用真实账号材料构建凭据包（与 OAuth 登录入库时的结构完全一致）。
    let bundle = CheckinCredentialBundle {
        profile_id: format!(
            "real-acceptance-{}",
            &sample.user_id[..8.min(sample.user_id.len())]
        ),
        account_id: sample.user_id.clone(),
        device_id: sample.device_id.clone(),
        machine_id: sample.machine_id.clone(),
        device_public_key: sample.device_keys.public_key_pem.clone(),
        device_private_key: sample.device_keys.private_key_pem.clone(),
        access_token: sample.token.clone(),
        refresh_token: sample.refresh_token.clone(),
        client_id: TRAE_SOLO_CLIENT_ID.to_string(),
        access_token_expires_at_unix_seconds: sample.jwt_payload.exp,
        refresh_token_expires_at_unix_seconds: refresh_expires_at,
        mobile_full: None,
    };
    println!(
        "== 样本概要：账号 {}… 设备 {}…（token {} 字节 / refreshToken {} 字节）",
        &sample.user_id[..6.min(sample.user_id.len())],
        &sample.device_id[..6.min(sample.device_id.len())],
        sample.token.len(),
        sample.refresh_token.len(),
    );
    println!(
        "== 有效期：access 剩余 {} 天 / refresh 剩余 {} 天",
        bundle
            .access_token_expires_at_unix_seconds
            .saturating_sub(now)
            / 86400,
        bundle
            .refresh_token_expires_at_unix_seconds
            .saturating_sub(now)
            / 86400,
    );

    // 2. DPAPI 加密入库 -> 解密读取 + 绑定校验（真实密钥材料的完整 roundtrip）。
    let root = tempfile::tempdir().expect("创建临时存储根失败");
    let store = CheckinCredentialStore::new(root.path());
    store.save(&bundle).expect("真实凭据包 DPAPI 加密入库失败");
    let binding = CheckinProfileBinding::new(
        bundle.profile_id.clone(),
        bundle.account_id.clone(),
        bundle.device_id.clone(),
        bundle.device_public_key.clone(),
    );
    let loaded = store.load(&binding).expect("解密读取或绑定校验失败");
    assert_eq!(
        loaded.access_token, bundle.access_token,
        "解密后 token 不一致"
    );
    assert_eq!(
        loaded.refresh_token, bundle.refresh_token,
        "解密后 refreshToken 不一致"
    );
    assert_eq!(
        loaded.device_private_key, bundle.device_private_key,
        "解密后设备私钥不一致"
    );
    println!("== DPAPI 加密入库 -> 解密读取 + 绑定校验：通过");

    // 3. 真实设备密钥的签名/验签 roundtrip（证明真实客户端密钥可被本实现正确使用）。
    let signing_input = device_proof_signing_input(
        "POST",
        "/trae/api/v3/oauth/ExchangeToken",
        TRAE_SOLO_CLIENT_ID,
        &bundle.refresh_token,
        now,
        "real-acceptance-nonce",
    );
    let signature =
        sign_device_proof(&bundle.device_private_key, &signing_input).expect("真实私钥签名失败");
    assert!(
        verify_device_proof(&bundle.device_public_key, &signing_input, &signature)
            .expect("真实公钥验签失败"),
        "真实密钥对签名验证未通过"
    );
    // 篡改输入后必须验签失败（fail-closed 语义）。
    let mut tampered = signing_input.clone();
    tampered[0] ^= 0xFF;
    assert!(
        !verify_device_proof(&bundle.device_public_key, &tampered, &signature)
            .expect("篡改输入验签失败"),
        "篡改签名输入后仍验证通过"
    );
    println!("== 真实设备密钥签名/验签 roundtrip + 篡改拒绝：通过");

    // 4. 续期阈值判定：距过期 > 阈值时不应触发续期（保护真实客户端的 refreshToken）。
    let refresh_required = needs_refresh(&bundle, now);
    println!(
        "== 续期阈值判定：needs_refresh = {refresh_required}（access 剩余 {} 天，阈值 7 天）",
        bundle
            .access_token_expires_at_unix_seconds
            .saturating_sub(now)
            / 86400,
    );

    // 5. 真实 status 只读查询（ReadCheckinTransport 直连）。
    let mut bindings = BTreeMap::new();
    bindings.insert(bundle.profile_id.clone(), binding);
    let transport = RealCheckinTransport::new(&store, bindings);
    let status = transport
        .status(&bundle.profile_id)
        .expect("真实 status 查询失败");
    println!(
        "== 真实签到状态：enabled={} checked_in={} credits={:?}",
        status.enabled, status.checked_in, status.credits
    );

    // 6. 可选 claim：必须由 TRAE_SYNC_REAL_CLAIM=1 显式授权（真实领取当日积分）。
    if std::env::var("TRAE_SYNC_REAL_CLAIM").ok().as_deref() == Some("1") {
        let claim = transport
            .claim(&bundle.profile_id)
            .expect("真实 claim 失败");
        println!("== 真实 claim 完成：credits={:?}", claim.credits);
        let after = transport
            .status(&bundle.profile_id)
            .expect("claim 后 status 复核失败");
        assert!(
            after.checked_in,
            "claim 后复核 checked_in 仍为 false（结果待复核）"
        );
        println!(
            "== claim 后复核：checked_in={} credits={:?}",
            after.checked_in, after.credits
        );
    } else {
        println!("== 跳过 claim（未设置 TRAE_SYNC_REAL_CLAIM=1，只读验收）");
    }

    // 7. GetUserInfo 身份核验（只读）：账号 ID 必须与凭据绑定一致。
    let client = trae_http_client();
    let user_info = get_user_info(&client, &bundle.access_token).expect("GetUserInfo 失败");
    println!(
        "== 账号资料：screen_name={} avatar={} masked_mobile={}",
        user_info.screen_name,
        if user_info.avatar_url.is_empty() {
            "<空>"
        } else {
            "<已返回>"
        },
        user_info.masked_mobile,
    );
    assert!(
        !user_info.screen_name.is_empty(),
        "GetUserInfo 未返回屏幕名（协议响应结构可能变化）"
    );
    println!("== 真实账号受控验收（只读部分）全部通过");
}

/// 导入真实账号到应用签到存储（`%LOCALAPPDATA%\Trae Sync\data\checkin`），
/// 使应用 UI 无需浏览器登录即可使用该账号。
///
/// 与登录入库保持一致的语义：
/// - 同账号去重：复用已有档案的 profile_id（之后通过 OAuth 正常登录同一账号会原地覆盖，
///   自动迁移到独立虚拟设备凭证，无残留）；
/// - 导入的是真实客户端设备（与 TRAE 客户端共享签到配额与 refresh token）；
///   token 剩余 <7 天时应用会续期并轮换共享的 refreshToken，可能导致 TRAE 客户端
///   需要重新登录——正式使用前建议在应用内完成一次 OAuth 登录。
///
/// 运行方式：
/// ```text
/// $env:TRAE_SYNC_REAL_CREDENTIAL_SAMPLE = "E:\...\credential-sample-20260821.json"
/// cargo test -p traesync-infrastructure --test real_account_acceptance import_real_account -- --ignored --nocapture
/// ```
#[test]
#[ignore = "导入真实账号到应用签到存储：需显式提供凭证样本"]
fn import_real_account_into_app_store() {
    let sample = load_sample();
    let now = now_unix_seconds();
    let refresh_expires_at =
        iso_to_unix_seconds(&sample.refresh_expired_at).expect("refreshExpiredAt 解析失败");

    // 应用生产签到材料根（与组合根 checkin_material_root 一致）。
    let material_root = std::env::var_os("LOCALAPPDATA")
        .map(|base| {
            std::path::PathBuf::from(base)
                .join("Trae Sync")
                .join("data")
                .join("checkin")
        })
        .expect("LOCALAPPDATA 未设置");
    std::fs::create_dir_all(&material_root).expect("创建签到材料根失败");

    let store = CheckinCredentialStore::new(&material_root);
    let registry = AccountRegistry::new(&material_root);

    // 同账号去重：已有档案（OAuth 登录或既往导入）则复用其 profile_id。
    let effective_profile_id = registry
        .load()
        .expect("读取账号注册表失败")
        .iter()
        .find(|record| record.account_id == sample.user_id)
        .map(|record| record.profile_id.clone())
        .unwrap_or_else(|| format!("checkin-import-{}", sample.user_id));

    // 1. 凭据包 DPAPI 加密入库。
    let bundle = CheckinCredentialBundle {
        profile_id: effective_profile_id.clone(),
        account_id: sample.user_id.clone(),
        device_id: sample.device_id.clone(),
        machine_id: sample.machine_id.clone(),
        device_public_key: sample.device_keys.public_key_pem.clone(),
        device_private_key: sample.device_keys.private_key_pem.clone(),
        access_token: sample.token.clone(),
        refresh_token: sample.refresh_token.clone(),
        client_id: TRAE_SOLO_CLIENT_ID.to_string(),
        access_token_expires_at_unix_seconds: sample.jwt_payload.exp,
        refresh_token_expires_at_unix_seconds: refresh_expires_at,
        mobile_full: None, // 实收样本不含补录手机号，保持与旧结构行为一致
    };
    store.save(&bundle).expect("凭据包加密入库失败");

    // 2. 非敏感档案 upsert。
    let record = AccountRecord {
        profile_id: effective_profile_id.clone(),
        account_id: sample.user_id.clone(),
        screen_name: if sample.account.username.is_empty() {
            sample.user_id.clone()
        } else {
            sample.account.username.clone()
        },
        avatar_url: sample.account.avatar_url.clone(),
        device_id: sample.device_id.clone(),
        device_public_key: sample.device_keys.public_key_pem.clone(),
        display_name: None,
        masked_mobile: String::new(),
        created_at_unix_seconds: now,
        last_verified_at_unix_seconds: now,
        device_created_at_unix_seconds: now,
        auto_checkin_enabled: true,
        archived: false,
    };
    registry.upsert(&record).expect("账号档案写入失败");
    println!(
        "== 已导入账号 {}（profile {}）到应用签到存储",
        record.screen_name, record.profile_id
    );

    // 3. 按应用 run_real_checkin 的编排复核：注册表 -> 绑定 -> 续期阈值 -> 真实 status。
    let records = registry.load().expect("导入后读取注册表失败");
    let imported = records
        .iter()
        .find(|record| record.profile_id == effective_profile_id)
        .expect("导入的档案未在注册表中");
    assert_eq!(imported.account_id, sample.user_id);
    let binding = CheckinProfileBinding::new(
        imported.profile_id.clone(),
        imported.account_id.clone(),
        imported.device_id.clone(),
        imported.device_public_key.clone(),
    );
    let loaded = store
        .load(&binding)
        .expect("导入的凭据包读取或绑定校验失败");
    assert_eq!(loaded.access_token, sample.token);
    // 续期阈值：当前剩余 >7 天，应用签到时不会触发续期（不轮换共享 refreshToken）。
    let refresh_required = needs_refresh(&loaded, now);
    println!(
        "== 导入后续期阈值判定：needs_refresh = {refresh_required}（access 剩余 {} 天）",
        loaded
            .access_token_expires_at_unix_seconds
            .saturating_sub(now)
            / 86400,
    );

    let mut bindings = BTreeMap::new();
    bindings.insert(imported.profile_id.clone(), binding);
    let transport = RealCheckinTransport::new(&store, bindings);
    let status = transport
        .status(&imported.profile_id)
        .expect("导入后真实 status 查询失败");
    println!(
        "== 导入后真实签到状态：enabled={} checked_in={} credits={:?}",
        status.enabled, status.checked_in, status.credits
    );
    println!("== 导入完成：应用 UI 的账号列表与签到流程可直接使用该账号");
}

/// 验证应用生产存储中的全部账号（只读）：注册表档案 -> DPAPI 解密 + 绑定校验 ->
/// 凭据健康度（有效期/字段完整性）-> 真实 status 查询（token 可用性实证）。
///
/// 运行方式（无需凭证样本，直接扫描生产存储）：
/// ```text
/// cargo test -p traesync-infrastructure --test real_account_acceptance verify_stored_accounts_readonly -- --ignored --nocapture
/// ```
///
/// 安全边界：只读——不 claim、不 refresh（不轮换任何 refreshToken）；
/// 输出不含 token、refreshToken 或密钥正文（只打印长度与非敏感业务字段）。
#[test]
#[ignore = "真实生产存储只读验收：直连线上 status 接口"]
fn verify_stored_accounts_readonly() {
    // 与组合根 checkin_material_root 一致的生产签到材料根。
    let material_root = std::env::var_os("LOCALAPPDATA")
        .map(|base| {
            std::path::PathBuf::from(base)
                .join("Trae Sync")
                .join("data")
                .join("checkin")
        })
        .expect("LOCALAPPDATA 未设置");
    let store = CheckinCredentialStore::new(&material_root);
    let registry = AccountRegistry::new(&material_root);
    let records = registry.load().expect("读取账号注册表失败");
    assert!(
        !records.is_empty(),
        "注册表为空：生产存储中没有账号档案（{}）",
        material_root.display()
    );
    let now = now_unix_seconds();

    let mut bindings = BTreeMap::new();
    for record in &records {
        // 1. DPAPI 解密 + 绑定校验：凭据文件必须存在且与档案绑定一致。
        let binding = CheckinProfileBinding::new(
            record.profile_id.clone(),
            record.account_id.clone(),
            record.device_id.clone(),
            record.device_public_key.clone(),
        );
        let bundle = store
            .load(&binding)
            .unwrap_or_else(|error| panic!("账号 {}（{}）凭据读取失败：{error:?}", record.screen_name, record.profile_id));
        // 2. 凭据健康度：字段非空 + 有效期剩余（不打印正文，只打印长度）。
        assert!(!bundle.access_token.is_empty(), "access_token 为空");
        assert!(!bundle.refresh_token.is_empty(), "refresh_token 为空");
        assert!(!bundle.machine_id.is_empty(), "machine_id 为空");
        assert!(
            !bundle.device_private_key.is_empty(),
            "设备私钥为空"
        );
        println!(
            "== {}（profile {}…）：device {}… token {} 字节 / refresh {} 字节 | access 剩余 {} 天 / refresh 剩余 {} 天 | needs_refresh={}",
            record.screen_name,
            &record.profile_id[..12.min(record.profile_id.len())],
            &record.device_id[..6.min(record.device_id.len())],
            bundle.access_token.len(),
            bundle.refresh_token.len(),
            bundle.access_token_expires_at_unix_seconds.saturating_sub(now) / 86400,
            bundle.refresh_token_expires_at_unix_seconds.saturating_sub(now) / 86400,
            needs_refresh(&bundle, now),
        );
        bindings.insert(record.profile_id.clone(), binding);
    }

    // 3. 真实 status 只读查询：逐账号验证 token 当前真实可用。
    let transport = RealCheckinTransport::new(&store, bindings);
    let mut ok = 0usize;
    let mut failed: Vec<String> = Vec::new();
    for record in &records {
        match transport.status(&record.profile_id) {
            Ok(status) => {
                println!(
                    "== status 通过：{}（{}）enabled={} checked_in={} credits={:?}",
                    record.screen_name, record.account_id, status.enabled, status.checked_in, status.credits
                );
                ok += 1;
            }
            Err(error) => {
                let message = format!("{}（{}）：{error:?}", record.screen_name, record.profile_id);
                println!("== status 失败：{message}");
                failed.push(message);
            }
        }
    }
    println!(
        "== 汇总：{}/{} 账号 token 实测可用；DPAPI 解密与绑定校验全部通过",
        ok,
        records.len()
    );
    assert!(failed.is_empty(), "存在 status 失败账号：{failed:?}");
}

// —— P1-4 决议 6：遥测头探针（2026-09-02）——
// 借鉴 trae-mate `build_headers` 的完整客户端头集合，对 status 端点（幂等只读）
// 做带头/裸头对比实测。判定：带头组不引发 HTTP 拒绝或业务码拒绝，且响应业务
// 字段与裸头基线一致 → 头集合可纳入生产（checkin_http.rs）。
// 安全边界：不碰 claim（不消耗名额）、不重铸设备、不轮换 token、报告不含敏感内容。

/// 探针熵源（SplitMix64）：时间纳秒 ^ 进程 ID；探针用途足够，
/// 生产实现的账号派生 ID 走确定性派生（另行设计）。
fn probe_random_hex(count: usize) -> String {
    let mut seed = (SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as u64)
        ^ ((std::process::id() as u64) << 32);
    let mut out = String::new();
    while out.len() < count {
        seed = seed.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = seed;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^= z >> 31;
        out.push_str(&format!("{z:016x}"));
    }
    out.truncate(count);
    out
}

/// 探针用 UUID v4（版本位 4、变体位 8-a）。
fn probe_uuid_v4() -> String {
    let mut h = probe_random_hex(32).into_bytes();
    h[12] = b'4';
    h[16] = match h[16] {
        b'0'..=b'3' => b'8',
        b'4'..=b'7' => b'9',
        _ => b'a',
    };
    let h = String::from_utf8(h).expect("hex 为 ASCII");
    format!(
        "{}-{}-{}-{}-{}",
        &h[0..8],
        &h[8..12],
        &h[12..16],
        &h[16..20],
        &h[20..32]
    )
}

/// 探针用 W3C traceparent：`00-<32hex trace>-<16hex span>-01`
/// （trae-mate 的 x-tt-trace-id 同形）。
fn probe_traceparent() -> String {
    format!("00-{}-{}-01", probe_random_hex(32), probe_random_hex(16))
}

/// 单次探针请求结果（只记非敏感字段）。
struct ProbeOutcome {
    http_status: Option<u16>,
    business_code: Option<i64>,
    enabled: Option<bool>,
    checked_in: Option<bool>,
    credits: Option<i64>,
    error: Option<String>,
    duration_ms: u128,
}

/// 对 status 端点发一次带自定义头集合的请求。
fn probe_status(
    client: &reqwest::blocking::Client,
    token: &str,
    device_id: &str,
    extra_headers: &[(String, String)],
) -> ProbeOutcome {
    let url = "https://api.trae.cn/trae/api/v2/ug/checkin_credits/status";
    let started = std::time::Instant::now();
    let mut request = client
        .post(url)
        .header("Authorization", format!("Cloud-IDE-JWT {token}"))
        .header("x-device-id", device_id)
        .header("Content-Type", "application/json");
    for (name, value) in extra_headers {
        request = request.header(name.as_str(), value.as_str());
    }
    let duration_ms = started.elapsed().as_millis();
    match request.body("{}").send() {
        Ok(response) => {
            let http_status = response.status().as_u16();
            let text = response.text().unwrap_or_default();
            let json: serde_json::Value = match serde_json::from_str(&text) {
                Ok(value) => value,
                Err(_) => {
                    let head: String = text.chars().take(80).collect();
                    return ProbeOutcome {
                        http_status: Some(http_status),
                        duration_ms,
                        error: Some(format!("响应非 JSON：前 80 字符 {head:?}")),
                        ..Default::default()
                    };
                }
            };
            ProbeOutcome {
                http_status: Some(http_status),
                business_code: json.get("code").and_then(serde_json::Value::as_i64),
                enabled: json.get("enable").and_then(serde_json::Value::as_bool),
                checked_in: json.get("checked_in").and_then(serde_json::Value::as_bool),
                credits: json.get("credits").and_then(serde_json::Value::as_i64),
                error: None,
                duration_ms,
            }
        }
        Err(error) => ProbeOutcome {
            error: Some(format!("网络错误：{error}")),
            duration_ms,
            ..Default::default()
        },
    }
}

impl Default for ProbeOutcome {
    fn default() -> Self {
        Self {
            http_status: None,
            business_code: None,
            enabled: None,
            checked_in: None,
            credits: None,
            error: None,
            duration_ms: 0,
        }
    }
}

/// 三组头集合构造（组名 → 追加头）。
/// baseline：生产现状（Authorization/x-device-id/Content-Type 已在 probe_status 固定）。
/// with_ua：baseline + VSCode 客户端 UA 形态。
/// full：with_ua + trae-mate 固定头 + 账号派生头 + 每请求刷新头。
fn probe_header_groups() -> Vec<(&'static str, Vec<(String, String)>)> {
    let vscode_ua = "VSCode 1.107.1 (TRAE SOLO CN)".to_string();
    let with_ua = vec![("User-Agent".to_string(), vscode_ua.clone())];
    let full = vec![
        ("User-Agent".to_string(), vscode_ua.clone()),
        // trae-mate 固定头（app-version 用本项目 r34 探针实测值 0.1.54）
        ("x-lscbd-aid".to_string(), "787976".to_string()),
        ("app-version".to_string(), "0.1.54".to_string()),
        (
            "x-market-client-id".to_string(),
            "VSCode 1.107.1".to_string(),
        ),
        ("x-user-region".to_string(), "CN".to_string()),
        ("package-type".to_string(), "stable_cn".to_string()),
        ("x-lgw-req-sdk-type".to_string(), "3".to_string()),
        ("x-lscbd-platform".to_string(), "windows".to_string()),
        // 账号派生头（探针用随机值验证协议接受度）
        ("x-market-user-id".to_string(), probe_uuid_v4()),
        ("vscode-sessionid".to_string(), probe_random_hex(64)),
        // 每请求刷新头
        ("x-request-id".to_string(), probe_uuid_v4()),
        ("x-tt-trace-id".to_string(), probe_traceparent()),
    ];
    vec![("baseline", vec![]), ("with_ua", with_ua), ("full", full)]
}

#[test]
#[ignore = "遥测头探针实测：只打 status（幂等只读）不消耗名额；需生产存储有健康账号"]
fn telemetry_headers_status_probe() {
    let material_root = std::env::var_os("LOCALAPPDATA")
        .map(|base| {
            std::path::PathBuf::from(base)
                .join("Trae Sync")
                .join("data")
                .join("checkin")
        })
        .expect("LOCALAPPDATA 未设置");
    let store = CheckinCredentialStore::new(&material_root);
    let registry = AccountRegistry::new(&material_root);
    let records = registry.load().expect("读取账号注册表失败");
    assert!(!records.is_empty(), "注册表为空：没有可探针账号");
    let now = now_unix_seconds();

    // 选健康账号（access token 未过期），最多 2 个：跨账号复验头集合一致性。
    let mut healthy: Vec<&traesync_infrastructure::AccountRecord> = records
        .iter()
        .filter(|record| {
            let binding = CheckinProfileBinding::new(
                record.profile_id.clone(),
                record.account_id.clone(),
                record.device_id.clone(),
                record.device_public_key.clone(),
            );
            store
                .load(&binding)
                .map(|bundle| bundle.access_token_expires_at_unix_seconds > now)
                .unwrap_or(false)
        })
        .take(2)
        .collect();
    assert!(
        !healthy.is_empty(),
        "没有 access token 未过期的账号可做探针"
    );

    let client = traesync_infrastructure::trae_http_client();
    let groups = probe_header_groups();
    let mut report_accounts = serde_json::Map::new();
    let mut all_pass = true;

    for record in &healthy {
        let binding = CheckinProfileBinding::new(
            record.profile_id.clone(),
            record.account_id.clone(),
            record.device_id.clone(),
            record.device_public_key.clone(),
        );
        let bundle = store.load(&binding).expect("凭据读取失败");
        let mut group_outcomes = serde_json::Map::new();
        let mut baseline: Option<ProbeOutcome> = None;

        for (group_name, extra_headers) in &groups {
            // 每组间隔 800ms，避免连续请求触发限频干扰判定。
            if baseline.is_some() {
                std::thread::sleep(std::time::Duration::from_millis(800));
            }
            let outcome =
                probe_status(&client, &bundle.access_token, &record.device_id, extra_headers);
            println!(
                "== {} [{}] http={:?} code={:?} enable={:?} checked_in={:?} credits={:?} 耗时 {}ms",
                record.screen_name,
                group_name,
                outcome.http_status,
                outcome.business_code,
                outcome.enabled,
                outcome.checked_in,
                outcome.credits,
                outcome.duration_ms
            );
            if group_name == &"baseline" {
                // 基线必须通过，否则本账号探针无效（token/网络问题）。
                if outcome.http_status != Some(200) || outcome.business_code != Some(0) {
                    println!("== 基线未通过，跳过该账号的头组判定");
                    all_pass = false;
                }
                group_outcomes.insert(
                    "baseline".to_string(),
                    serde_json::json!({
                        "http": outcome.http_status, "code": outcome.business_code,
                        "enable": outcome.enabled, "checked_in": outcome.checked_in,
                        "credits": outcome.credits, "error": outcome.error,
                        "durationMs": outcome.duration_ms,
                    }),
                );
                baseline = Some(outcome);
                continue;
            }
            let pass = outcome.http_status == Some(200)
                && outcome.business_code == Some(0)
                && outcome.enabled == baseline.as_ref().map(|b| b.enabled).flatten()
                && outcome.checked_in == baseline.as_ref().map(|b| b.checked_in).flatten()
                && outcome.credits == baseline.as_ref().map(|b| b.credits).flatten();
            if !pass {
                all_pass = false;
            }
            group_outcomes.insert(
                (*group_name).to_string(),
                serde_json::json!({
                    "http": outcome.http_status, "code": outcome.business_code,
                    "enable": outcome.enabled, "checked_in": outcome.checked_in,
                    "credits": outcome.credits, "error": outcome.error,
                    "durationMs": outcome.duration_ms, "matchBaseline": pass,
                }),
            );
        }
        report_accounts.insert(
            record.screen_name.clone(),
            serde_json::Value::Object(group_outcomes),
        );
    }

    // 报告落档（不含 token/device_id 正文，只含脱敏前缀）。
    let report = serde_json::json!({
        "probe": "telemetry-headers status 对比实测（P1-4 决议 6，借鉴 trae-mate build_headers）",
        "localTime": chrono_like_now(),
        "verdict": if all_pass { "PASS：带头组与基线一致，头集合可纳入" } else { "FAIL：存在被拒或字段不一致，见明细" },
        "headerGroups": {
            "baseline": "Authorization + x-device-id + Content-Type（生产现状）",
            "with_ua": "baseline + User-Agent: VSCode 1.107.1 (TRAE SOLO CN)",
            "full": "with_ua + 固定头(x-lscbd-aid/app-version 0.1.54/x-market-client-id/x-user-region/package-type/x-lgw-req-sdk-type/x-lscbd-platform) + 账号派生(x-market-user-id/vscode-sessionid) + 每请求刷新(x-request-id/x-tt-trace-id)",
        },
        "accounts": report_accounts,
    });
    let report_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../.scratch/checkin-http/reports")
        .canonicalize()
        .expect("报告目录不存在");
    let report_path = report_dir.join(format!(
        "telemetry-headers-probe-{}.json",
        chrono_like_compact()
    ));
    std::fs::write(
        &report_path,
        serde_json::to_string_pretty(&report).expect("报告序列化失败"),
    )
    .expect("报告落盘失败");
    println!("== 报告已落档：{}", report_path.display());
    println!("== 判定：{}", if all_pass { "PASS" } else { "FAIL" });
    assert!(all_pass, "遥测头探针存在被拒或不一致组，见报告明细");
}

/// 本地时刻（`YYYY-MM-DDTHH:MM:SS+08:00` 语义，探针记录用）。
fn chrono_like_now() -> String {
    let seconds = now_unix_seconds();
    format_epoch(seconds, "%Y-%m-%dT%H:%M:%S+08:00")
}

fn chrono_like_compact() -> String {
    let seconds = now_unix_seconds();
    format_epoch(seconds, "%Y%m%d-%H%M%S")
}

/// Unix 秒 → 北京时刻字符串（无 chrono 依赖的简易实现，精度到秒）。
fn format_epoch(seconds: u64, pattern: &str) -> String {
    // 北京时间 = UTC+8：先秒级平移再拆日期/时刻字段。
    let adjusted = seconds + 8 * 3600;
    let (year, month, day) = civil_from_days((adjusted / 86400) as i64);
    let secs_of_day = adjusted % 86400;
    let (hour, minute, second) = (secs_of_day / 3600, (secs_of_day % 3600) / 60, secs_of_day % 60);
    match pattern {
        "%Y%m%d-%H%M%S" => format!("{year:04}{month:02}{day:02}-{hour:02}{minute:02}{second:02}"),
        _ => format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}+08:00"),
    }
}

/// Howard Hinnant civil_from_days：天数 → (年, 月, 日)。
fn civil_from_days(days: i64) -> (i64, u64, u64) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m as u64, d as u64)
}
