//! 账号级远程设备列表真实探针（ADR-0031 切片 1，Work CN 可行性实测用）。
//!
//! 运行方式（显式授权；`#[ignore]` 保证不进常规回归、不产生自动触网路径）：
//! ```text
//! $env:TRAE_SYNC_DEVICE_PROBE_MATERIAL_ROOT = "D:\...\material-root"
//! $env:TRAE_SYNC_DEVICE_PROBE_PRODUCT = "solo"   # 或 "trae_cn"
//! # 可选：$env:TRAE_SYNC_DEVICE_PROBE_PROFILE = "<profile_id>" 只测指定账号
//! cargo test -p traesync-infrastructure --test real_device_probe -- --ignored --nocapture
//! ```
//!
//! 验证链路：账号注册表 -> 绑定构造 -> DPAPI 凭据包解密 -> 真实
//! ListDevices（x-cloudide-token 鉴权）-> 快照解析。
//!
//! 输出纪律：只打印每个账号的已用/上限计数与设备行数；不打印 token、
//! device_id、设备名等任何具体值（对齐研究文档日志纪律）。

#![cfg(windows)]

use traesync_infrastructure::{
    AccountRegistry, CheckinCredentialStore, CheckinProfileBinding, OAuthClient,
    RealRemoteDeviceManager, RemoteDeviceHttpAdapter, ReqwestRemoteDeviceHttpAdapter,
};
use traesync_ports::RemoteDeviceManager;

#[test]
#[ignore]
fn real_list_devices_probe_with_explicit_material_root() {
    let material_root = match std::env::var("TRAE_SYNC_DEVICE_PROBE_MATERIAL_ROOT") {
        Ok(value) if !value.is_empty() => value,
        _ => panic!("需要 TRAE_SYNC_DEVICE_PROBE_MATERIAL_ROOT 环境变量显式授权"),
    };
    let expected_client = match std::env::var("TRAE_SYNC_DEVICE_PROBE_PRODUCT").as_deref() {
        Ok("solo") => OAuthClient::Solo,
        Ok("trae_cn") => OAuthClient::TraeCn,
        _ => panic!("TRAE_SYNC_DEVICE_PROBE_PRODUCT 必须显式设为 solo 或 trae_cn"),
    };
    let only_profile = std::env::var("TRAE_SYNC_DEVICE_PROBE_PROFILE").ok();

    let records = AccountRegistry::new(&material_root)
        .load()
        .expect("读取账号注册表失败");
    assert!(!records.is_empty(), "注册表为空：material_root 下没有账号档案");

    for record in records {
        if let Some(profile) = &only_profile {
            if &record.profile_id != profile {
                continue;
            }
        }
        let binding = CheckinProfileBinding::new(
            &record.profile_id,
            &record.account_id,
            &record.device_id,
            &record.device_public_key,
        );
        let manager = RealRemoteDeviceManager::new(&material_root, binding, expected_client);
        match manager.list_devices(&record.profile_id) {
            Ok(snapshot) => {
                let max_text = snapshot
                    .max_count
                    .map(|count| count.to_string())
                    .unwrap_or_else(|| "未知".to_string());
                println!(
                    "账号 {} 设备占用 {}/{}，设备行数 {}",
                    record.profile_id, snapshot.used_count, max_text, snapshot.devices.len()
                );
            }
            Err(error) => {
                // 错误 Display 是稳定原因码，不含敏感材料。
                println!("账号 {} 拉取失败：{error}", record.profile_id);
            }
        }
    }
}

/// 格式摘要：长度 + 字符集类别（不含原值）。
fn shape_summary(value: &str) -> String {
    let pure_hex = value.chars().all(|c| c.is_ascii_hexdigit());
    let pure_digits = value.chars().all(|c| c.is_ascii_digit());
    let has_dash = value.contains('-');
    let upper = value.chars().any(|c| c.is_ascii_uppercase());
    format!(
        "len={} hex={} digits={} dash={} upper={}",
        value.len(),
        pure_hex,
        pure_digits,
        has_dash,
        upper
    )
}

/// SHA-256 前 8 位指纹：同值必同指纹，反推匹配关系时不暴露原值。
fn fingerprint(value: &str) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(value.as_bytes());
    digest
        .iter()
        .take(4)
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// 本机识别诊断：本地凭据 device_id 与服务端各行 DeviceID 的形状/指纹对比。
/// 用于定位「未识别到本机对应的设备」的根因（怀疑两侧不是同一标识体系）。
/// 输出纪律：只打印形状摘要与指纹，不打印任何原值。
#[test]
#[ignore]
fn diagnose_local_device_id_shape_mismatch() {
    let material_root = match std::env::var("TRAE_SYNC_DEVICE_PROBE_MATERIAL_ROOT") {
        Ok(value) if !value.is_empty() => value,
        _ => panic!("需要 TRAE_SYNC_DEVICE_PROBE_MATERIAL_ROOT 环境变量显式授权"),
    };
    let expected_client = match std::env::var("TRAE_SYNC_DEVICE_PROBE_PRODUCT").as_deref() {
        Ok("solo") => OAuthClient::Solo,
        Ok("trae_cn") => OAuthClient::TraeCn,
        _ => panic!("TRAE_SYNC_DEVICE_PROBE_PRODUCT 必须显式设为 solo 或 trae_cn"),
    };

    let records = AccountRegistry::new(&material_root)
        .load()
        .expect("读取账号注册表失败");
    // 只诊断注册表中的第一个账号，足够定位形态差异且输出最少。
    let Some(record) = records.first() else {
        panic!("注册表为空");
    };
    println!("== 账号 {} ==", record.profile_id);
    println!(
        "本地凭据 device_id：{} 指纹={}",
        shape_summary(&record.device_id),
        fingerprint(&record.device_id)
    );

    let binding = CheckinProfileBinding::new(
        &record.profile_id,
        &record.account_id,
        &record.device_id,
        &record.device_public_key,
    );
    let manager = RealRemoteDeviceManager::new(&material_root, binding, expected_client);
    match manager.list_devices(&record.profile_id) {
        Ok(snapshot) => {
            for (index, device) in snapshot.devices.iter().enumerate() {
                println!(
                    "行{index}：{} 指纹={} 与本机相等={} is_local={} 类型={:?} 产品={:?}",
                    shape_summary(&device.device_id),
                    fingerprint(&device.device_id),
                    device.device_id == record.device_id,
                    device.is_local,
                    device.device_type,
                    device.bound_products,
                );
            }
        }
        Err(error) => println!("拉取失败：{error}"),
    }

    // 第二段：响应字段名清单（只输出 key 名，不输出任何 value），
    // 检查官方响应是否携带未解析的「当前设备」标记字段（如 IsCurrent）。
    let adapter = ReqwestRemoteDeviceHttpAdapter::new();
    let store = CheckinCredentialStore::new(std::path::Path::new(&material_root));
    let bundle = store
        .load(&CheckinProfileBinding::new(
            &record.profile_id,
            &record.account_id,
            &record.device_id,
            &record.device_public_key,
        ))
        .expect("凭据解密失败");
    match adapter.list_devices(&bundle.access_token, &bundle.client_id, &record.device_id) {
        Ok(envelope) => {
            if let Some(result) = envelope.get("Result") {
                println!("Result 层字段名：{:?}", result.as_object().map(|o| o.keys().collect::<Vec<_>>()));
            }
            if let Some(rows) = envelope
                .get("Result")
                .and_then(|r| r.get("Devices"))
                .and_then(serde_json::Value::as_array)
            {
                // CurrentDevice 是服务端布尔标记，真值本身无敏感，原样输出定位解析问题。
                for (index, row) in rows.iter().enumerate() {
                    println!(
                        "行{index} CurrentDevice 原始值：{}",
                        row.get("CurrentDevice").map(|v| v.to_string()).unwrap_or("<缺失>".into())
                    );
                }
                if let Some(first) = rows.first() {
                    println!("设备行字段名：{:?}", first.as_object().map(|o| o.keys().collect::<Vec<_>>()));
                }
            }
        }
        Err(error) => println!("原始响应拉取失败：{error}"),
    }

    // 第三段：带 ClientID 的对照请求（生成客户端定义了该字段；原生客户端
    // 场景可能需要它服务端才能标记当前设备）。CurrentDevice 布尔无敏感。
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .expect("构造 HTTP 客户端失败");
    for (label, body) in [
        ("带 ClientID", serde_json::json!({"ClientID": bundle.client_id})),
        ("带 ClientID+DeviceInfo", serde_json::json!({
            "ClientID": bundle.client_id,
            "DeviceInfo": {"DeviceID": record.device_id}
        })),
    ] {
        let response = client
            .post("https://api.trae.cn/cloudide/api/v3/trae/oauth/ListDevices")
            .header("Content-Type", "application/json")
            .header("x-cloudide-token", &bundle.access_token)
            .json(&body)
            .send()
            .expect("对照请求发送失败");
        println!("== {label}：HTTP {} ==", response.status());
        let envelope: serde_json::Value = response.json().expect("对照响应解析失败");
        if let Some(rows) = envelope
            .get("Result")
            .and_then(|r| r.get("Devices"))
            .and_then(serde_json::Value::as_array)
        {
            for (index, row) in rows.iter().enumerate() {
                println!(
                    "{label} 行{index} CurrentDevice：{}",
                    row.get("CurrentDevice").map(|v| v.to_string()).unwrap_or("<缺失>".into())
                );
            }
        } else {
            println!("{label}：无 Devices（响应结构变化或业务错误）");
        }
    }
}
