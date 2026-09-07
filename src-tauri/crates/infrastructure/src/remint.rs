//! 设备重铸服务（ADR-0019 v6）：AuthCode 模式为账号铸造全新签到设备。
//!
//! v6 起重铸只由用户在账号详情手动触发（"重置签到设备"按钮）；
//! 签到链路不再自动调用（自动重铸/冷却重试/退役恢复链已于 2026-09-02 退役）。
//!
//! 2026-08-26 完整闭环验证（6/6 账号，证据 `.scratch/checkin-http/reports/remint-*.json`）：
//! ```text
//! 当前账号 access_token
//!   -> GetPCAuthCode（x-cloudide-token 头，SOLO 形态）
//!   -> 全新设备四件套（DeviceID/MachineID/EC P-256 密钥对）
//!   -> ExchangeToken（AuthCode + PKCE CodeVerifier）
//!   -> JWT 账号匹配校验
//!   -> 凭据包原子写回 + 注册表同步
//! ```
//!
//! 协议事实（实测固化）：
//! - 新设备首签仅 SOLO 形态可行（SOLO_PC / en1oxy7wnw8j9n / 完整硬件字段）；
//!   Work 形态（IDE_PC / 空硬件字段）新设备 claim 被 9074 稳定拒绝；
//! - AuthCode 一次性消费；失败后必须重新生成 PKCE 与设备材料；
//! - 9074 含频率维度且窗口不可预测（v6 起不做任何自动重试，由用户
//!   稍后手动重试或手动重置设备）。
//!
//! 安全约束：Token、私钥、AuthCode 不进日志；写回走 `CheckinCredentialStore`
//! 的原子写；任何失败零回写（旧凭据保持可用）。

use std::time::{SystemTime, UNIX_EPOCH};

use traesync_ports::CheckinDeviceRemint;

use crate::account_registry::{AccountRecord, AccountRegistry};
use crate::checkin_credential::{
    generate_device_keypair, CheckinCredentialBundle, CheckinCredentialStore,
    CheckinProfileBinding,
};
use crate::checkin_http::{
    exchange_token_by_auth_code, get_pc_auth_code, trae_http_client, OAuthClient, TokenGrant,
};
use crate::checkin_login::{
    decode_account_from_jwt, generate_pkce_pair, generate_virtual_device_id,
};

/// 重铸失败原因（非敏感；不携带 Token 或响应正文）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemintError {
    /// 账号不在注册表。
    ProfileNotFound,
    /// 凭据包读取失败（未登录/绑定不匹配）。
    CredentialUnavailable,
    /// GetPCAuthCode 失败（网络/业务码）。
    AuthCodeFailed,
    /// ExchangeToken 失败（网络/业务码/协议）。
    ExchangeFailed,
    /// ExchangeToken 返回账号与目标账号不一致（禁止写回的硬失败）。
    AccountMismatch,
    /// 新凭据写回失败（旧凭据未动）。
    WriteBackFailed,
}

impl std::fmt::Display for RemintError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let message = match self {
            Self::ProfileNotFound => "账号不在注册表",
            Self::CredentialUnavailable => "凭据包不可用（先完成登录）",
            Self::AuthCodeFailed => "AuthCode 签发失败",
            Self::ExchangeFailed => "Token 交换失败",
            Self::AccountMismatch => "返回账号不匹配",
            Self::WriteBackFailed => "凭据写回失败",
        };
        formatter.write_str(message)
    }
}

impl std::error::Error for RemintError {}

/// 真实设备重铸服务：注册表 + 凭据仓库路径克隆，实现 `CheckinDeviceRemint` 端口。
pub struct DeviceRemintService {
    registry: AccountRegistry,
    store: CheckinCredentialStore,
}

impl DeviceRemintService {
    pub fn new(material_root: &std::path::Path) -> Self {
        Self {
            registry: AccountRegistry::new(material_root),
            store: CheckinCredentialStore::new(material_root),
        }
    }

    /// 执行一次完整重铸；成功返回新设备 ID。
    ///
    /// 网络类失败单次重试（GetPCAuthCode/ExchangeToken；缓解临时 IP 限流）。
    pub fn remint(&self, profile_id: &str) -> Result<String, RemintError> {
        let record = self
            .registry
            .load()
            .map_err(|_| RemintError::ProfileNotFound)?
            .into_iter()
            .find(|record| record.profile_id == profile_id)
            .ok_or(RemintError::ProfileNotFound)?;
        let bundle = self
            .store
            .load(&CheckinProfileBinding::new(
                record.profile_id.clone(),
                record.account_id.clone(),
                record.device_id.clone(),
                record.device_public_key.clone(),
            ))
            .map_err(|_| RemintError::CredentialUnavailable)?;

        // ---- 全新设备四件套（成套生成，绝不复用旧值） ----
        let new_device_id = generate_virtual_device_id()
            .map_err(|_| RemintError::AuthCodeFailed)?;
        let new_machine_id = random_machine_id().map_err(|_| RemintError::AuthCodeFailed)?;
        let (new_private_key, new_public_key) =
            generate_device_keypair().map_err(|_| RemintError::AuthCodeFailed)?;
        let pkce = generate_pkce_pair().map_err(|_| RemintError::AuthCodeFailed)?;

        // ---- GetPCAuthCode + ExchangeToken（SOLO 形态，网络失败各重试一次） ----
        let client = trae_http_client();
        // 全链路唯一形态：SOLO（ADR-0019 v6）。
        let oauth_client = OAuthClient::Solo;
        let auth_code =
            with_network_retry(|| {
                get_pc_auth_code(
                    &client,
                    &bundle.access_token,
                    &pkce.code_challenge,
                    &new_device_id,
                    oauth_client,
                )
            })
            .map_err(|_| RemintError::AuthCodeFailed)?;
        let device_info =
            oauth_client.device_info(&new_device_id, &new_machine_id, &new_public_key);
        let grant = with_network_retry(|| {
            exchange_token_by_auth_code(
                &client,
                &auth_code,
                &pkce.code_verifier,
                &device_info,
                oauth_client,
            )
        })
        .map_err(|_| RemintError::ExchangeFailed)?;

        // ---- JWT 账号匹配校验（不匹配即硬失败，不签到不写回） ----
        let (account_from_jwt, _) = decode_account_from_jwt(&grant.access_token)
            .map_err(|_| RemintError::AccountMismatch)?;
        if account_from_jwt != record.account_id {
            return Err(RemintError::AccountMismatch);
        }

        // ---- 凭据包 + 注册表原子写回 ----
        self.write_back(&record, &grant, &new_device_id, &new_machine_id, &new_public_key, &new_private_key, oauth_client)?;
        Ok(new_device_id)
    }

    /// 写回新凭据与注册表档案；失败时保留旧凭据。
    fn write_back(
        &self,
        record: &AccountRecord,
        grant: &TokenGrant,
        new_device_id: &str,
        new_machine_id: &str,
        new_public_key: &str,
        new_private_key: &str,
        oauth_client: OAuthClient,
    ) -> Result<(), RemintError> {
        // 重铸保留已补录的完整手机号（与新设备无绑定关系；旧包读取失败按未补录）。
        let previous_mobile_full = self
            .store
            .load(&CheckinProfileBinding::new(
                record.profile_id.clone(),
                record.account_id.clone(),
                record.device_id.clone(),
                record.device_public_key.clone(),
            ))
            .ok()
            .and_then(|bundle| bundle.mobile_full);
        let bundle = CheckinCredentialBundle {
            profile_id: record.profile_id.clone(),
            account_id: record.account_id.clone(),
            device_id: new_device_id.to_string(),
            machine_id: new_machine_id.to_string(),
            device_public_key: new_public_key.to_string(),
            device_private_key: new_private_key.to_string(),
            access_token: grant.access_token.clone(),
            refresh_token: grant.refresh_token.clone(),
            client_id: oauth_client.client_id().to_string(),
            access_token_expires_at_unix_seconds: grant.access_token_expires_at_unix_seconds,
            refresh_token_expires_at_unix_seconds: grant.refresh_token_expires_at_unix_seconds,
            mobile_full: previous_mobile_full,
        };
        // save 为原子写（临时文件 + 替换）；失败时旧凭据保持不变。
        self.store
            .save(&bundle)
            .map_err(|_| RemintError::WriteBackFailed)?;
        let updated = AccountRecord {
            device_id: new_device_id.to_string(),
            device_public_key: new_public_key.to_string(),
            last_verified_at_unix_seconds: unix_now(),
            // 重铸即新设备诞生：记录铸造时刻，供 9074 报错时区分
            // 「设备太新稍后重试」与「设备被拒需要重置」。
            device_created_at_unix_seconds: unix_now(),
            ..record.clone()
        };
        self.registry
            .upsert(&updated)
            .map_err(|_| RemintError::WriteBackFailed)?;
        Ok(())
    }
}

impl CheckinDeviceRemint for DeviceRemintService {
    fn remint_device(&self, profile_id: &str) -> Result<String, String> {
        self.remint(profile_id)
            .map(|device_id| device_id)
            .map_err(|error| format!("{error:?}"))
    }
}

/// 网络类失败单次重试（缓解临时 IP 限流；签到 claim 不适用此模式）。
fn with_network_retry<T>(
    mut attempt: impl FnMut() -> Result<T, crate::checkin_http::CheckinHttpError>,
) -> Result<T, crate::checkin_http::CheckinHttpError> {
    match attempt() {
        Err(crate::checkin_http::CheckinHttpError::Network) => attempt(),
        other => other,
    }
}

/// 生成随机 16 字节 hex 机器指纹（与登录时同形态）。
fn random_machine_id() -> Result<String, RemintError> {
    let mut bytes = [0u8; 16];
    openssl::rand::rand_bytes(&mut bytes).map_err(|_| RemintError::AuthCodeFailed)?;
    Ok(hex::encode(bytes))
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn machine_id_is_32_char_lowercase_hex() {
        let machine_id = random_machine_id().unwrap();
        assert_eq!(machine_id.len(), 32);
        assert!(machine_id.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
    }

    #[test]
    fn remint_missing_profile_reports_profile_not_found() {
        let root = tempfile::tempdir().unwrap();
        let service = DeviceRemintService::new(root.path());
        assert_eq!(service.remint("missing"), Err(RemintError::ProfileNotFound));
    }

    #[cfg(windows)]
    #[test]
    fn remint_without_credential_reports_unavailable() {
        // 有档案但无凭据包：CredentialUnavailable（不触网即失败）。
        let root = tempfile::tempdir().unwrap();
        let registry = AccountRegistry::new(root.path());
        registry
            .upsert(&AccountRecord {
                profile_id: "p1".to_string(),
                account_id: "a1".to_string(),
                screen_name: "测试".to_string(),
                avatar_url: String::new(),
                device_id: "1234567890123456".to_string(),
                device_public_key: "pub".to_string(),
                display_name: None,
                masked_mobile: String::new(),
                created_at_unix_seconds: 1,
                last_verified_at_unix_seconds: 1,
                device_created_at_unix_seconds: 0,
                auto_checkin_enabled: true,
                archived: false,
            })
            .unwrap();
        let service = DeviceRemintService::new(root.path());
        assert_eq!(service.remint("p1"), Err(RemintError::CredentialUnavailable));
    }
}
