//! 融合版 TRAE（Work CN + Trae CN 合并版）登录凭据铸造探针。
//!
//! 背景（2026-10-13 融合版迁移研究，ADR-0035）：融合版 Trae CN 客户端登录
//! 通道为 `OAuthClient::TraeCn` 形态（ClientID ono9krqynydwx5 / PlatformCode
//! IDE_PC），与本工具既有 Trae CN 适配完全同形态。迁移路线：用工具保存的
//! Work SOLO 凭据包（access token）以 TraeCn 形态为**主环境设备身份**
//! （icube-dc 键密钥对 + telemetry.machineId，机器级）铸造全新登录代次。
//! 不触碰工具凭据包，不轮换旧 refresh token，签到链路零影响。
//!
//! 用法:
//!   unified_mint_probe <checkin_root> <profile_id> <main_instance_dir> --dry-run
//!     只读校验：解密凭据包 + GetUserInfo 身份匹配，不铸造不写盘。
//!   unified_mint_probe <checkin_root> <profile_id> <main_instance_dir> --save-only <fused_root>
//!     阶段一迁移：铸造 → 新凭据身份复核 → DPAPI 加密写入
//!     `<fused_root>/<profile_id>.fused`；**不写任何客户端目录**。
//!   unified_mint_probe <checkin_root> <profile_id> <main_instance_dir>
//!     隔离验证：铸造并把登录 blob 写入指定实例目录（要求客户端已关闭）。
//!
//! 设备身份来源目录只读（仅读 icube-dc 键与 machineId）；第三种模式的
//! 写入目录必须与设备身份来源一致或自洽，否则客户端续期设备不匹配。
//!
//! 铁律：工具 .checkin 凭据包只读；token 类值不进入输出。

use std::path::{Path, PathBuf};

use serde_json::json;
use traesync_infrastructure::account_registry::AccountRegistry;
use traesync_infrastructure::blob_keepalive::{
    construct_auth_identity, decrypt_named_blob, ConstructAuthInput,
};
use traesync_infrastructure::checkin_credential::{CheckinCredentialStore, CheckinProfileBinding};
use traesync_infrastructure::checkin_http::{
    exchange_token_by_auth_code, get_pc_auth_code, get_user_info_full_for_oauth_client,
    DeviceInfoBlock, OAuthClient, UserInfoFull,
};
use traesync_infrastructure::checkin_login::generate_pkce_pair;

/// 融合版客户端上报的真实硬件描述（2026-10-13 主环境 ExchangeToken 日志实测）。
const FUSED_CLIENT_VERSION: &str = "3.4.2";

/// .fused 容器魔数与版本（阶段一临时容器格式；P15-4 正式建模时兼容读取）。
const FUSED_MAGIC: &[u8; 8] = b"TSFUSED1";

fn main() -> std::process::ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() < 3 {
        eprintln!(
            "用法: unified_mint_probe <checkin_root> <profile_id> <main_instance_dir> [--dry-run | --save-only <fused_root>]"
        );
        return std::process::ExitCode::FAILURE;
    }
    let checkin_root = PathBuf::from(&args[0]);
    let profile_id = args[1].clone();
    let main_dir = PathBuf::from(&args[2]);
    let dry_run = args.iter().any(|a| a == "--dry-run");
    let save_root = args
        .iter()
        .position(|a| a == "--save-only")
        .and_then(|i| args.get(i + 1))
        .map(PathBuf::from);

    let result = match (&save_root, dry_run) {
        (_, true) => run(&checkin_root, &profile_id, &main_dir, Mode::DryRun),
        (Some(root), false) => run(&checkin_root, &profile_id, &main_dir, Mode::SaveOnly(root)),
        (None, false) => run(&checkin_root, &profile_id, &main_dir, Mode::Apply),
    };
    match result {
        Ok(summary) => {
            println!("{}", serde_json::to_string_pretty(&summary).unwrap_or_default());
            std::process::ExitCode::SUCCESS
        }
        Err(error) => {
            println!("{}", json!({ "error": error }));
            std::process::ExitCode::FAILURE
        }
    }
}

enum Mode<'a> {
    /// 只读校验：不铸造不写盘。
    DryRun,
    /// 阶段一迁移：铸造 + DPAPI 容器落盘，不写客户端目录。
    SaveOnly(&'a Path),
    /// 隔离验证：铸造 + 登录 blob 写入设备身份来源目录（客户端须关闭）。
    Apply,
}

fn run(
    checkin_root: &Path,
    profile_id: &str,
    main_dir: &Path,
    mode: Mode<'_>,
) -> Result<serde_json::Value, String> {
    // ===== 1. 账号档案与凭据包（只读）=====
    let registry = AccountRegistry::new(checkin_root);
    let record = registry
        .find(profile_id)
        .map_err(|e| format!("账号档案读取失败: {e:?}"))?
        .ok_or_else(|| format!("档案不存在: {profile_id}"))?;
    let binding = CheckinProfileBinding::new(
        record.profile_id.clone(),
        record.account_id.clone(),
        record.device_id.clone(),
        record.device_public_key.clone(),
    );
    let store = CheckinCredentialStore::new(checkin_root);
    let bundle = store
        .load(&binding)
        .map_err(|e| format!("凭据包解密失败: {e:?}"))?;

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    if bundle.access_token_expires_at_unix_seconds <= now {
        return Err(format!(
            "access token 已过期（{} < {}），需要先续期",
            bundle.access_token_expires_at_unix_seconds, now
        ));
    }

    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|e| format!("HTTP 客户端构建失败: {e:?}"))?;

    // ===== 2. 只读身份校验（GetUserInfo，TraeCn 形态请求头）=====
    let full: UserInfoFull = get_user_info_full_for_oauth_client(
        &client,
        &bundle.access_token,
        OAuthClient::TraeCn,
    )
    .map_err(|e| format!("GetUserInfo 失败: {e:?}"))?;
    if full.user_id != bundle.account_id {
        return Err(format!(
            "身份不匹配: GetUserInfo={} 凭据包={}",
            full.user_id, bundle.account_id
        ));
    }
    if matches!(mode, Mode::DryRun) {
        return Ok(json!({
            "mode": "dry-run",
            "profile_id": profile_id,
            "account_id": full.user_id,
            "screen_name": full.screen_name,
            "masked_mobile": full.masked_mobile,
            "access_expires_at": bundle.access_token_expires_at_unix_seconds,
        }));
    }

    // ===== 3. 读取主环境设备身份（机器级；来源目录只读）=====
    let storage_path = main_dir.join("User").join("globalStorage").join("storage.json");
    let storage_raw = std::fs::read_to_string(&storage_path)
        .map_err(|_| format!("storage.json 不可读: {}", storage_path.display()))?;
    let storage_root: serde_json::Value = serde_json::from_str(&storage_raw)
        .map_err(|_| "storage.json 非法 JSON".to_string())?;
    let dc_key = storage_root
        .as_object()
        .and_then(|map| {
            map.keys()
                .find(|k| k.starts_with("iCubeAuthInfo://icube-dc:"))
                .cloned()
        })
        .ok_or("目录无 icube-dc 设备键（需先空启动一次官方客户端）")?;
    let device_id = dc_key
        .rsplit(':')
        .next()
        .ok_or("icube-dc 键名无设备 ID 后缀")?
        .to_string();
    let dc_plain = decrypt_named_blob(main_dir, &dc_key)
        .map_err(|e| format!("icube-dc 键解密失败: {e:?}"))?;
    let dc_json: serde_json::Value =
        serde_json::from_slice(&dc_plain).map_err(|_| "icube-dc 明文非法 JSON".to_string())?;
    let device_public_key = dc_json
        .get("publicKeyPEM")
        .and_then(|v| v.as_str())
        .ok_or("icube-dc 明文缺 publicKeyPEM")?
        .to_string();
    let machine_id = storage_root
        .get("telemetry.machineId")
        .and_then(|v| v.as_str())
        .ok_or("storage.json 缺 telemetry.machineId")?
        .to_string();

    // ===== 4. PKCE + GetPCAuthCode（TraeCn 形态，设备 ID 即主环境设备）=====
    let pkce = generate_pkce_pair().map_err(|e| format!("PKCE 生成失败: {e:?}"))?;
    let auth_code = get_pc_auth_code(
        &client,
        &bundle.access_token,
        &pkce.code_challenge,
        &device_id,
        OAuthClient::TraeCn,
    )
    .map_err(|e| format!("GetPCAuthCode 失败: {e:?}"))?;

    // ===== 5. ExchangeToken（DeviceInfo 完全对齐融合版真实续期请求）=====
    let device_info = DeviceInfoBlock {
        device_id: device_id.clone(),
        machine_id: machine_id.clone(),
        platform_code: OAuthClient::TraeCn.platform_code().to_string(),
        client_version: FUSED_CLIENT_VERSION.to_string(),
        device_public_key: device_public_key.clone(),
        device_name: "1273223663的电脑".to_string(),
        device_model: "81Q5".to_string(),
        device_brand: "LENOVO".to_string(),
        device_cpu: "Intel(R) Core(TM) i7-9750H CPU".to_string(),
        os_info: "windows".to_string(),
        os_version: "Windows 11 Home".to_string(),
    };
    let grant = exchange_token_by_auth_code(
        &client,
        &auth_code,
        &pkce.code_verifier,
        &device_info,
        OAuthClient::TraeCn,
    )
    .map_err(|e| format!("ExchangeToken 失败: {e:?}"))?;

    // ===== 6. 新凭据身份复核（用新 access token 只读查询）=====
    let verify = get_user_info_full_for_oauth_client(
        &client,
        &grant.access_token,
        OAuthClient::TraeCn,
    )
    .map_err(|e| format!("新凭据 GetUserInfo 失败: {e:?}"))?;
    if verify.user_id != bundle.account_id {
        return Err(format!(
            "新凭据身份不匹配: {} != {}（拒绝写盘）",
            verify.user_id, bundle.account_id
        ));
    }

    // ===== 7a. --save-only：DPAPI 容器落盘，不写客户端目录 =====
    if let Mode::SaveOnly(fused_root) = &mode {
        let record = json!({
            "format_version": 1,
            "profile_id": profile_id,
            "account_id": bundle.account_id,
            "screen_name": verify.screen_name,
            "masked_mobile": verify.masked_mobile,
            "access_token": grant.access_token,
            "refresh_token": grant.refresh_token,
            "access_token_expires_at_unix_seconds": grant.access_token_expires_at_unix_seconds,
            "refresh_token_expires_at_unix_seconds": grant.refresh_token_expires_at_unix_seconds,
            "device_id": device_id,
            "machine_id": machine_id,
            "device_public_key": device_public_key,
            "minted_at_unix_seconds": now,
        });
        let plain = serde_json::to_vec(&record).map_err(|e| format!("记录序列化失败: {e:?}"))?;
        let encrypted = dpapi_protect(&plain).map_err(|e| format!("DPAPI 加密失败: {e:?}"))?;
        let mut payload = Vec::with_capacity(FUSED_MAGIC.len() + 8 + encrypted.len());
        payload.extend_from_slice(FUSED_MAGIC);
        payload.extend_from_slice(&1u32.to_le_bytes());
        payload.extend_from_slice(&(encrypted.len() as u32).to_le_bytes());
        payload.extend_from_slice(&encrypted);
        std::fs::create_dir_all(fused_root).map_err(|e| format!("容器目录创建失败: {e:?}"))?;
        let dest = fused_root.join(format!("{profile_id}.fused"));
        let tmp = fused_root.join(format!("{profile_id}.fused.tmp"));
        std::fs::write(&tmp, &payload).map_err(|e| format!("临时文件写入失败: {e:?}"))?;
        std::fs::rename(&tmp, &dest).map_err(|e| format!("原子改名失败: {e:?}"))?;
        // 写后复核：解密回读并校验账号一致。
        let readback = std::fs::read(&dest).map_err(|e| format!("回读失败: {e:?}"))?;
        let decrypted = dpapi_unprotect(&readback[FUSED_MAGIC.len() + 8..])
            .map_err(|e| format!("回读解密失败: {e:?}"))?;
        let back: serde_json::Value = serde_json::from_slice(&decrypted)
            .map_err(|e| format!("回读解析失败: {e:?}"))?;
        if back.get("account_id").and_then(|v| v.as_str()) != Some(bundle.account_id.as_str()) {
            return Err("回读校验失败：账号不一致（拒绝报告成功）".to_string());
        }
        return Ok(json!({
            "mode": "mint+save-only",
            "profile_id": profile_id,
            "account_id": verify.user_id,
            "screen_name": verify.screen_name,
            "masked_mobile": verify.masked_mobile,
            "device_id": device_id,
            "container": dest.display().to_string(),
            "container_bytes": payload.len(),
            "access_expires_at": grant.access_token_expires_at_unix_seconds,
            "refresh_expires_at": grant.refresh_token_expires_at_unix_seconds,
        }));
    }

    // ===== 7b. 隔离验证：构造登录 blob 写入目标 storage.json（登出态全新密钥材料）=====
    let switch = construct_auth_identity(
        &ConstructAuthInput {
            account_id: &bundle.account_id,
            access_token: &grant.access_token,
            refresh_token: &grant.refresh_token,
            access_token_expires_at_unix_seconds: grant.access_token_expires_at_unix_seconds,
            refresh_token_expires_at_unix_seconds: grant.refresh_token_expires_at_unix_seconds,
            user_info: &verify,
        },
        main_dir,
    )
    .map_err(|e| format!("登录态构造写回失败: {e:?}"))?;

    Ok(json!({
        "mode": "mint+apply",
        "profile_id": profile_id,
        "account_id": verify.user_id,
        "screen_name": verify.screen_name,
        "device_id": device_id,
        "machine_id": machine_id,
        "from_user_id": switch.from_user_id,
        "to_user_id": switch.to_user_id,
        "access_expires_at": grant.access_token_expires_at_unix_seconds,
        "refresh_expires_at": grant.refresh_token_expires_at_unix_seconds,
    }))
}

/// DPAPI 加密（CryptProtectData，与 key_wrapper 同 API 形态；探针独立实现，
/// 不改生产代码）。仅 Windows 可用。
#[cfg(windows)]
fn dpapi_protect(plaintext: &[u8]) -> Result<Vec<u8>, String> {
    use std::ptr::null;
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Cryptography::{CryptProtectData, CRYPT_INTEGER_BLOB};

    let mut input = CRYPT_INTEGER_BLOB {
        cbData: plaintext.len() as u32,
        pbData: plaintext.as_ptr() as *mut u8,
    };
    let mut output = CRYPT_INTEGER_BLOB {
        cbData: 0,
        pbData: std::ptr::null_mut(),
    };
    let ok = unsafe { CryptProtectData(&mut input, null(), null(), null(), null(), 0, &mut output) };
    if ok == 0 || output.pbData.is_null() || output.cbData == 0 {
        return Err("CryptProtectData 返回失败".to_string());
    }
    let result = unsafe { std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec() };
    unsafe {
        LocalFree(output.pbData as *mut std::ffi::c_void);
    }
    Ok(result)
}

#[cfg(not(windows))]
fn dpapi_protect(_plaintext: &[u8]) -> Result<Vec<u8>, String> {
    Err("DPAPI 仅在 Windows 可用".to_string())
}

/// DPAPI 解密（CryptUnprotectData；与 dpapi_protect 成对）。
#[cfg(windows)]
fn dpapi_unprotect(encrypted: &[u8]) -> Result<Vec<u8>, String> {
    use std::ptr::null_mut;
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Cryptography::{CryptUnprotectData, CRYPT_INTEGER_BLOB};

    let mut input = CRYPT_INTEGER_BLOB {
        cbData: encrypted.len() as u32,
        pbData: encrypted.as_ptr() as *mut u8,
    };
    let mut output = CRYPT_INTEGER_BLOB {
        cbData: 0,
        pbData: std::ptr::null_mut(),
    };
    let ok = unsafe {
        CryptUnprotectData(
            &mut input,
            null_mut(),
            null_mut(),
            null_mut(),
            null_mut(),
            0,
            &mut output,
        )
    };
    if ok == 0 || output.pbData.is_null() || output.cbData == 0 {
        return Err("CryptUnprotectData 返回失败".to_string());
    }
    let result = unsafe { std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec() };
    unsafe {
        LocalFree(output.pbData as *mut std::ffi::c_void);
    }
    Ok(result)
}

#[cfg(not(windows))]
fn dpapi_unprotect(_encrypted: &[u8]) -> Result<Vec<u8>, String> {
    Err("DPAPI 仅在 Windows 可用".to_string())
}
