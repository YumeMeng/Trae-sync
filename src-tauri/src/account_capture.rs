//! P16-T4：新账号「从客户端捕获」入池编排（spec docs/specs/p16-account-capture.md）。
//!
//! 场景：用户已在 Trae 客户端登录一个未入池的新账号——工具读取客户端登录
//! blob（master_login_material），按 P15-0 批量铸造同链路（GetPCAuthCode →
//! ExchangeToken → 设备身份对齐）铸造 `.fused` 容器与 `.checkin` 凭据包
//! （入池即全功能：切号/历史/自动签到），并完成 Work 档案登记。
//!
//! 设计约束（移植性，grilling Q2 方案 C）：编排不引用任何 UI 状态；路径全部
//! 经 storage_roots 派生注入；协议差异点集中在 OAuthClient::TraeCn 形态参数。

use std::path::{Path, PathBuf};

use traesync_infrastructure::account_registry::{AccountRecord, AccountRegistry};
use traesync_infrastructure::blob_keepalive::master_login_material;
use traesync_infrastructure::checkin_credential::{
    CheckinCredentialBundle, CheckinCredentialStore, generate_device_keypair,
};
use traesync_infrastructure::checkin_http::{
    exchange_token_by_auth_code, get_pc_auth_code, get_user_info_full_for_oauth_client,
    OAuthClient,
};
use traesync_infrastructure::checkin_login::{
    generate_machine_fingerprint, generate_pkce_pair, generate_virtual_device_id,
};
use traesync_infrastructure::fused_login_credential::{FusedLoginRecord, FusedLoginStore};

use crate::oauth_login::generate_login_profile_id;

/// 捕获失败原因（错误码即前端文案键，勿随意改名）。
#[derive(Debug)]
pub(crate) enum CaptureAccountError {
    /// Trae 客户端无登录态（blob 缺失或损坏）。
    BlobMissing,
    /// 账号已在池内（去重命中），携带既有 profile_id。
    AccountAlreadyRegistered { profile_id: String },
    /// 在线身份复核失败（HTTP 错误或 user_id 不一致）。
    IdentityReviewFailed,
    /// 铸造环节失败（AuthCode/Exchange/容器写入）。
    MintFailed(&'static str),
    /// 档案登记失败（registry/checkin 存储写入）。
    RegistryFailed,
}

impl std::fmt::Display for CaptureAccountError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BlobMissing => write!(f, "capture_blob_missing"),
            Self::AccountAlreadyRegistered { profile_id } => {
                write!(f, "capture_account_already_registered:{profile_id}")
            }
            Self::IdentityReviewFailed => write!(f, "capture_identity_review_failed"),
            Self::MintFailed(stage) => write!(f, "capture_mint_failed:{stage}"),
            Self::RegistryFailed => write!(f, "capture_registry_failed"),
        }
    }
}

/// 捕获编排的全部路径注入（离线测试可指向临时目录）。
pub(crate) struct CaptureAccountPaths {
    /// Trae 客户端主目录（登录 blob 源，只读）。
    pub master_dir: PathBuf,
    /// 签到/账号池材料根（.checkin 包 + AccountRegistry）。
    pub checkin_root: PathBuf,
    /// 融合版登录代次容器根（.fused）。
    pub fused_root: PathBuf,
}

/// 捕获成功回执（前端展示字段）。
pub(crate) struct CaptureAccountOutcome {
    pub profile_id: String,
    pub account_id: String,
    pub screen_name: String,
}

/// 读取主环境设备身份（机器级）：icube-dc 键名后缀 = device_id，解密明文取
/// publicKeyPEM，storage.json 取 telemetry.machineId。P15-0 批量铸造探针
/// （unified_mint_probe 步骤 3）的生产化搬运；来源目录只读。
fn read_master_device_identity(
    master_dir: &Path,
) -> Result<(String, String, String), CaptureAccountError> {
    let storage_path = master_dir.join("User").join("globalStorage").join("storage.json");
    let storage_raw = std::fs::read_to_string(&storage_path).map_err(|_| {
        CaptureAccountError::MintFailed("device_storage_unreadable")
    })?;
    let storage_root: serde_json::Value =
        serde_json::from_str(&storage_raw).map_err(|_| {
            CaptureAccountError::MintFailed("device_storage_invalid_json")
        })?;
    let dc_key = storage_root
        .as_object()
        .and_then(|map| {
            map.keys()
                .find(|k| k.starts_with("iCubeAuthInfo://icube-dc:"))
                .cloned()
        })
        .ok_or(CaptureAccountError::MintFailed("device_key_missing"))?;
    let device_id = dc_key
        .rsplit(':')
        .next()
        .ok_or(CaptureAccountError::MintFailed("device_key_no_suffix"))?
        .to_string();
    let dc_plain = traesync_infrastructure::blob_keepalive::decrypt_named_blob(
        master_dir,
        &dc_key,
    )
    .map_err(|_| CaptureAccountError::MintFailed("device_key_decrypt_failed"))?;
    let dc_json: serde_json::Value = serde_json::from_slice(&dc_plain)
        .map_err(|_| CaptureAccountError::MintFailed("device_plain_invalid_json"))?;
    let device_public_key = dc_json
        .get("publicKeyPEM")
        .and_then(|v| v.as_str())
        .ok_or(CaptureAccountError::MintFailed("device_public_key_missing"))?
        .to_string();
    let machine_id = storage_root
        .get("telemetry.machineId")
        .and_then(|v| v.as_str())
        .ok_or(CaptureAccountError::MintFailed("machine_id_missing"))?
        .to_string();
    Ok((device_id, machine_id, device_public_key))
}

/// 捕获编排查（P15-0 铸造同链路）。HTTP 客户端内部构造（与签到/复核同源）；
/// 离线单测覆盖无 HTTP 的前置路径，复核/铸造路径由真机验收覆盖。
pub(crate) fn capture_account_with(
    paths: &CaptureAccountPaths,
) -> Result<CaptureAccountOutcome, CaptureAccountError> {
    // 1. 读取客户端登录 blob（无登录态直接失败——捕获的前提是「已在客户端登录」）。
    let material = master_login_material(&paths.master_dir).ok_or(CaptureAccountError::BlobMissing)?;

    // 1.5 HTTP 客户端（与签到/复核同源；blob 检查之后构造，缺失路径无需网络）。
    let client = traesync_infrastructure::checkin_http::trae_http_client();

    // 2. 去重：同账号已在池内则拒绝（携带既有 profile_id 供前端提示）。
    let registry = AccountRegistry::new(&paths.checkin_root);
    let records = registry.load().map_err(|_| CaptureAccountError::RegistryFailed)?;
    if let Some(existing) = records
        .iter()
        .find(|record| record.account_id == material.user_id)
    {
        return Err(CaptureAccountError::AccountAlreadyRegistered {
            profile_id: existing.profile_id.clone(),
        });
    }

    // 3. 在线复核 blob 身份（GetUserInfo TraeCn 形态；user_id 必须一致）。
    let info = get_user_info_full_for_oauth_client(
        &client,
        &material.access_token,
        OAuthClient::TraeCn,
    )
    .map_err(|_| CaptureAccountError::IdentityReviewFailed)?;
    if info.user_id.as_str() != material.user_id.as_str() {
        return Err(CaptureAccountError::IdentityReviewFailed);
    }

    // 4. 主环境设备身份（机器级；.fused 容器与 GetPCAuthCode 的设备对齐来源）。
    let (device_id, machine_id, device_public_key) =
        read_master_device_identity(&paths.master_dir)?;

    // 5-6. 换发铸造：blob access token → GetPCAuthCode → ExchangeToken（新代次）。
    let pkce = generate_pkce_pair().map_err(|_| CaptureAccountError::MintFailed("pkce"))?;
    let auth_code = get_pc_auth_code(
        &client,
        &material.access_token,
        &pkce.code_challenge,
        &device_id,
        OAuthClient::TraeCn,
    )
    .map_err(|_| CaptureAccountError::MintFailed("pc_auth_code"))?;
    let device_info = OAuthClient::TraeCn.device_info(&device_id, &machine_id, &device_public_key);
    let grant = exchange_token_by_auth_code(
        &client,
        &auth_code,
        &pkce.code_verifier,
        &device_info,
        OAuthClient::TraeCn,
    )
    .map_err(|_| CaptureAccountError::MintFailed("exchange"))?;

    // 7. 新凭据复核：铸造出的代次必须仍是同一身份（防账号错绑）。
    let minted_info = get_user_info_full_for_oauth_client(
        &client,
        &grant.access_token,
        OAuthClient::TraeCn,
    )
    .map_err(|_| CaptureAccountError::MintFailed("minted_review"))?;
    if minted_info.user_id.as_str() != material.user_id.as_str() {
        return Err(CaptureAccountError::MintFailed("minted_identity_mismatch"));
    }

    // 8. 新号 profile_id（与 OAuth 登录同一生成器，格式兼容既有账号池）。
    let profile_id = generate_login_profile_id();

    // 9. .checkin 凭据包：签到链路的虚拟设备（配额隔离维度）独立生成——
    //    与主环境机器级设备身份解耦（与 OAuth 登录入库的设备策略一致）。
    let virtual_device_id =
        generate_virtual_device_id().map_err(|_| CaptureAccountError::RegistryFailed)?;
    let virtual_machine_id =
        generate_machine_fingerprint().map_err(|_| CaptureAccountError::RegistryFailed)?;
    let (device_private_key_pem, virtual_device_public_key_pem) =
        generate_device_keypair().map_err(|_| CaptureAccountError::RegistryFailed)?;
    let checkin_store = CheckinCredentialStore::new(&paths.checkin_root);
    checkin_store
        .save(&CheckinCredentialBundle {
            profile_id: profile_id.clone(),
            account_id: material.user_id.clone(),
            device_id: virtual_device_id.clone(),
            machine_id: virtual_machine_id.clone(),
            device_public_key: virtual_device_public_key_pem.clone(),
            device_private_key: device_private_key_pem,
            access_token: grant.access_token.clone(),
            refresh_token: grant.refresh_token.clone(),
            client_id: OAuthClient::TraeCn.client_id().to_string(),
            // 手机全号 capture 阶段不可得（blob 无此字段），存量账号由「刷新额度」路径无感补采。
            mobile_full: None,
            access_token_expires_at_unix_seconds: grant.access_token_expires_at_unix_seconds,
            refresh_token_expires_at_unix_seconds: grant.refresh_token_expires_at_unix_seconds,
        })
        .map_err(|_| CaptureAccountError::RegistryFailed)?;

    // 10. .fused 容器：切号与历史身份判定的数据源（机器级设备身份绑定）。
    let now_unix = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0);
    let fused_store = FusedLoginStore::new(&paths.fused_root);
    fused_store
        .save(&FusedLoginRecord {
            profile_id: profile_id.clone(),
            account_id: material.user_id.clone(),
            screen_name: info.screen_name.clone(),
            masked_mobile: info.masked_mobile.clone(),
            access_token: grant.access_token.clone(),
            refresh_token: grant.refresh_token.clone(),
            access_token_expires_at_unix_seconds: grant.access_token_expires_at_unix_seconds,
            refresh_token_expires_at_unix_seconds: grant.refresh_token_expires_at_unix_seconds,
            device_id: device_id.clone(),
            machine_id: machine_id.clone(),
            device_public_key: device_public_key.clone(),
            minted_at_unix_seconds: now_unix,
            last_updated_at_unix_seconds: now_unix,
        })
        .map_err(|_| CaptureAccountError::RegistryFailed)?;

    // 11. Work 档案登记（入池；手机号/头像从复核资料补采，与 OAuth 登录同口径）。
    registry
        .upsert(&AccountRecord {
            profile_id: profile_id.clone(),
            account_id: material.user_id.clone(),
            screen_name: info.screen_name.clone(),
            avatar_url: info.avatar_url.clone(),
            device_id: virtual_device_id,
            device_public_key: virtual_device_public_key_pem,
            display_name: None,
            masked_mobile: info.masked_mobile.clone(),
            created_at_unix_seconds: now_unix,
            last_verified_at_unix_seconds: now_unix,
            device_created_at_unix_seconds: now_unix,
            auto_checkin_enabled: true,
            archived: false,
        })
        .map_err(|_| CaptureAccountError::RegistryFailed)?;

    Ok(CaptureAccountOutcome {
        profile_id,
        account_id: material.user_id,
        screen_name: info.screen_name,
    })
}

/// 捕获命令体（真实模式）：路径全部经 storage_roots 推导，HTTP 客户端与
/// 生产签到/复核同源。
pub(crate) fn capture_account(
    state_storage_root: &str,
    master_dir: &Path,
) -> Result<CaptureAccountOutcome, CaptureAccountError> {
    let storage_root = Path::new(state_storage_root);
    let paths = CaptureAccountPaths {
        master_dir: master_dir.to_path_buf(),
        checkin_root:
            traesync_infrastructure::storage_roots::checkin_material_root_from(storage_root),
        fused_root:
            traesync_infrastructure::storage_roots::fused_login_root_from(storage_root),
    };
    capture_account_with(&paths)
}


/// P16-T4：新账号「从客户端捕获」入池（真实模式命令）。
/// 读 Trae 客户端登录 blob → 在线复核 → 铸造 .fused/.checkin → 档案登记。
#[tauri::command]
pub(crate) async fn capture_trae_cn_account(
    state: tauri::State<'_, crate::AppState>,
) -> Result<serde_json::Value, String> {
    crate::ensure_login_real_mode(&state)?;
    let storage_root = state.storage_root.clone();
    let master_dir = traesync_infrastructure::environment_registry::master_data_dir()
        .map_err(|error| format!("{error:?}"))?;
    crate::run_blocking("capture_account_join_failed", move || {
        crate::account_capture::capture_account(&storage_root, &master_dir)
            .map(|outcome| {
                serde_json::json!({
                    "profile_id": outcome.profile_id,
                    "account_id": outcome.account_id,
                    "screen_name": outcome.screen_name,
                })
            })
            .map_err(|error| error.to_string())
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 空目录无登录 blob → BlobMissing（捕获前置：客户端必须已登录）。
    /// 去重/复核/铸造路径依赖真实 HTTP 与客户端 blob，单测盲区由隔离演练
    /// 与真机验收（新号到位后单次确认）覆盖——见 spec Testing Seams。
    #[test]
    fn capture_rejects_when_client_blob_missing() {
        let root = tempfile::tempdir().unwrap();
        let paths = CaptureAccountPaths {
            master_dir: root.path().join("master"),
            checkin_root: root.path().join("checkin"),
            fused_root: root.path().join("fused"),
        };
        std::fs::create_dir_all(&paths.master_dir).unwrap();
        let outcome = capture_account_with(&paths);
        assert!(matches!(outcome, Err(CaptureAccountError::BlobMissing)));
    }
}
