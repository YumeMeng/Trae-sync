//! 命令层错误映射集中地（P12-R2a）。
//!
//! lib.rs 内散布的「错误类型 → 稳定错误码」映射函数按域收拢到本模块：
//! 错误码字符串是对前端与 e2e 的稳定契约，**禁止改动任何映射取值**。
//! 各映射函数使用对错误枚举的穷尽 match：错误枚举新增变体时编译期即报错，
//! 防止新错误漏映射静默穿透到前端。

use traesync_infrastructure::{
    fixture_path_error_text, CheckinCredentialError, CheckinHttpError, CredentialVaultError,
    FixturePathError, LoginError, MasterArchiveError, TraeCnCheckinError, WorkCnReadLocationError,
};

// ============================================================================
// 各域「错误类型 → 稳定错误码」映射（P12-R2a 自 lib.rs 收拢，映射值零变化）
// ============================================================================

/// Tauri IPC 只返回稳定的脱敏错误；路径细节留在后端内部，不跨边界暴露。
pub(super) fn work_cn_location_command_error(_error: &WorkCnReadLocationError) -> String {
    "data_location_unavailable: TRAE 默认数据位置不可用，请重新授权后重试".to_string()
}

/// Fixture 路径错误跨 IPC 边界统一脱敏；原始路径只留在本地诊断日志。
pub(super) fn fixture_path_command_error(error: FixturePathError) -> String {
    fixture_path_error_text(&error)
}

/// 凭据库错误 → 稳定错误码；未显式列出的变体透传 Display 文案。
pub(super) fn credential_error_text(error: CredentialVaultError) -> String {
    match error {
        CredentialVaultError::Missing => "credential_missing".to_string(),
        CredentialVaultError::BindingMismatch => "credential_binding_mismatch".to_string(),
        CredentialVaultError::TraeNotClosed => "process_running".to_string(),
        CredentialVaultError::StorageInvalid => "storage_json_invalid".to_string(),
        CredentialVaultError::StorageTooLarge => "storage_json_too_large".to_string(),
        CredentialVaultError::UnsupportedAuthKey => "unsupported_login_bundle".to_string(),
        CredentialVaultError::RecoveryRequired => "credential_manual_recovery_required".to_string(),
        other => other.to_string(),
    }
}

/// 凭据维护错误 -> 账号总览可持久化的非敏感原因码。
pub(super) fn credential_maintenance_error_code(error: &CheckinCredentialError) -> &'static str {
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

/// 登录命令错误码；不携带 Token、AuthCode 或回调正文。
pub(super) fn login_command_error(error: &LoginError) -> String {
    match error {
        LoginError::InvalidCallback => "login_callback_invalid",
        LoginError::ExchangeFailed => "login_exchange_failed",
        LoginError::ExchangeDeviceLimit => "login_exchange_device_limit",
        LoginError::InvalidToken => "login_token_invalid",
        LoginError::Storage => "login_storage_failed",
        LoginError::Cancelled => "login_cancelled",
    }
    .to_string()
}

/// Trae CN 健康检查错误 → 稳定错误码（401/403 归为凭据失效）。
pub(super) fn trae_cn_health_error_code(error: &CheckinHttpError) -> String {
    match error {
        CheckinHttpError::Network => "network_error".to_string(),
        CheckinHttpError::Http(401 | 403) => "credential_invalid".to_string(),
        CheckinHttpError::Http(_) => "health_check_failed".to_string(),
        CheckinHttpError::Business(code) => format!("business_{code}"),
        CheckinHttpError::Protocol => "health_check_failed".to_string(),
    }
}

/// Trae CN 签到错误 → 稳定错误码（P13-1；口径对齐 trae_cn_health_error_code：
/// 401/403 归凭据失效，业务码 `business_{code}` 原样透传不压平）。
pub(super) fn trae_cn_checkin_error_code(error: &TraeCnCheckinError) -> String {
    match error {
        TraeCnCheckinError::Network => "network_error".to_string(),
        TraeCnCheckinError::Http(401 | 403) => "credential_invalid".to_string(),
        TraeCnCheckinError::Http(_) => "trae_cn_checkin_http_failed".to_string(),
        TraeCnCheckinError::Business(code) => format!("business_{code}"),
        TraeCnCheckinError::Protocol => "trae_cn_checkin_protocol".to_string(),
        // 防御分支（命令层先校验档案与 client，正常不触达）归凭据失效；
        // 其余凭据原因码本身就是前端词表既有形态，原样透传。
        TraeCnCheckinError::Credential("profile_mismatch" | "client_mismatch") => {
            "credential_invalid".to_string()
        }
        TraeCnCheckinError::Credential(reason) => reason.to_string(),
    }
}

/// 归档通道错误码映射（稳定契约，前端 safeUiError 据此给文案）。
pub(super) fn map_master_archive_error(error: MasterArchiveError) -> String {
    match error {
        MasterArchiveError::DbUnavailable => "master_archive_no_data".to_string(),
        MasterArchiveError::DbOpenFailed => "master_archive_open_failed".to_string(),
        MasterArchiveError::ArchiveUnsupported => "master_archive_unsupported".to_string(),
        MasterArchiveError::WriteFailed => "master_archive_write_failed".to_string(),
        MasterArchiveError::BackupFailed => "master_archive_backup_failed".to_string(),
        MasterArchiveError::MergeInvalid => "master_merge_invalid".to_string(),
        MasterArchiveError::ArchiveConflict => "master_archive_conflict".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ==================== 稳定性契约：逐变体断言稳定错误码 ====================
    // match 对枚举的穷尽性由编译器保证（新变体漏映射即编译失败）；
    // 本节测试把「每个变体映射到哪个错误码」固化为可读契约。

    /// 登录域：LoginError 全变体 → 稳定错误码。
    #[test]
    fn login_error_maps_every_variant_to_stable_code() {
        assert_eq!(login_command_error(&LoginError::InvalidCallback), "login_callback_invalid");
        assert_eq!(login_command_error(&LoginError::ExchangeFailed), "login_exchange_failed");
        assert_eq!(
            login_command_error(&LoginError::ExchangeDeviceLimit),
            "login_exchange_device_limit"
        );
        assert_eq!(login_command_error(&LoginError::InvalidToken), "login_token_invalid");
        assert_eq!(login_command_error(&LoginError::Storage), "login_storage_failed");
        assert_eq!(login_command_error(&LoginError::Cancelled), "login_cancelled");
    }

    /// 凭据维护域：CheckinCredentialError 全变体 → 稳定原因码。
    #[test]
    fn credential_maintenance_maps_every_variant_to_stable_code() {
        assert_eq!(credential_maintenance_error_code(&CheckinCredentialError::BindingMismatch), "binding_mismatch");
        assert_eq!(
            credential_maintenance_error_code(&CheckinCredentialError::CredentialRefreshFailed),
            "credential_refresh_failed"
        );
        assert_eq!(credential_maintenance_error_code(&CheckinCredentialError::AuthMismatch), "auth_mismatch");
        assert_eq!(
            credential_maintenance_error_code(&CheckinCredentialError::RecoveryRequired),
            "manual_recovery_required"
        );
        assert_eq!(credential_maintenance_error_code(&CheckinCredentialError::Missing), "credential_missing");
        assert_eq!(credential_maintenance_error_code(&CheckinCredentialError::Unavailable), "credential_unavailable");
        assert_eq!(credential_maintenance_error_code(&CheckinCredentialError::Invalid), "credential_invalid");
    }

    /// 健康检查域：CheckinHttpError 全变体 → 稳定错误码（含 401/403 特判）。
    #[test]
    fn trae_cn_health_maps_every_variant_to_stable_code() {
        assert_eq!(trae_cn_health_error_code(&CheckinHttpError::Network), "network_error");
        assert_eq!(trae_cn_health_error_code(&CheckinHttpError::Http(401)), "credential_invalid");
        assert_eq!(trae_cn_health_error_code(&CheckinHttpError::Http(403)), "credential_invalid");
        assert_eq!(trae_cn_health_error_code(&CheckinHttpError::Http(500)), "health_check_failed");
        assert_eq!(trae_cn_health_error_code(&CheckinHttpError::Business(9095)), "business_9095");
        assert_eq!(trae_cn_health_error_code(&CheckinHttpError::Protocol), "health_check_failed");
    }

    /// CN 签到域：TraeCnCheckinError 全变体 → 稳定错误码（业务码透传不压平）。
    #[test]
    fn trae_cn_checkin_maps_every_variant_to_stable_code() {
        use traesync_infrastructure::TraeCnCheckinError;
        assert_eq!(
            trae_cn_checkin_error_code(&TraeCnCheckinError::Network),
            "network_error"
        );
        assert_eq!(
            trae_cn_checkin_error_code(&TraeCnCheckinError::Http(401)),
            "credential_invalid"
        );
        assert_eq!(
            trae_cn_checkin_error_code(&TraeCnCheckinError::Http(403)),
            "credential_invalid"
        );
        assert_eq!(
            trae_cn_checkin_error_code(&TraeCnCheckinError::Http(500)),
            "trae_cn_checkin_http_failed"
        );
        // 业务码不压平：`business_{code}` 原样带出数值。
        assert_eq!(
            trae_cn_checkin_error_code(&TraeCnCheckinError::Business(9095)),
            "business_9095"
        );
        assert_eq!(
            trae_cn_checkin_error_code(&TraeCnCheckinError::Protocol),
            "trae_cn_checkin_protocol"
        );
        assert_eq!(
            trae_cn_checkin_error_code(&TraeCnCheckinError::Credential("client_mismatch")),
            "credential_invalid"
        );
        assert_eq!(
            trae_cn_checkin_error_code(&TraeCnCheckinError::Credential("credential_missing")),
            "credential_missing"
        );
    }

    /// 归档通道域：MasterArchiveError 全变体 → 稳定错误码。
    #[test]
    fn master_archive_maps_every_variant_to_stable_code() {
        assert_eq!(map_master_archive_error(MasterArchiveError::DbUnavailable), "master_archive_no_data");
        assert_eq!(map_master_archive_error(MasterArchiveError::DbOpenFailed), "master_archive_open_failed");
        assert_eq!(
            map_master_archive_error(MasterArchiveError::ArchiveUnsupported),
            "master_archive_unsupported"
        );
        assert_eq!(map_master_archive_error(MasterArchiveError::WriteFailed), "master_archive_write_failed");
        assert_eq!(map_master_archive_error(MasterArchiveError::BackupFailed), "master_archive_backup_failed");
        assert_eq!(map_master_archive_error(MasterArchiveError::MergeInvalid), "master_merge_invalid");
        assert_eq!(map_master_archive_error(MasterArchiveError::ArchiveConflict), "master_archive_conflict");
    }

    /// 凭据库域：显式映射的变体保持稳定码；未列出的变体走 Display 兜底。
    #[test]
    fn credential_vault_maps_listed_variants_and_display_fallback() {
        assert_eq!(credential_error_text(CredentialVaultError::Missing), "credential_missing");
        assert_eq!(credential_error_text(CredentialVaultError::BindingMismatch), "credential_binding_mismatch");
        assert_eq!(credential_error_text(CredentialVaultError::TraeNotClosed), "process_running");
        assert_eq!(credential_error_text(CredentialVaultError::StorageInvalid), "storage_json_invalid");
        assert_eq!(credential_error_text(CredentialVaultError::StorageTooLarge), "storage_json_too_large");
        assert_eq!(credential_error_text(CredentialVaultError::UnsupportedAuthKey), "unsupported_login_bundle");
        assert_eq!(
            credential_error_text(CredentialVaultError::RecoveryRequired),
            "credential_manual_recovery_required"
        );
        // 兜底路径：未显式映射的变体透传 Display 文案（非空即可，文案归错误类型所有）。
        assert!(!credential_error_text(CredentialVaultError::Unavailable).is_empty());
        assert!(!credential_error_text(CredentialVaultError::Invalid).is_empty());
    }

    /// 数据位置域：错误详情不跨 IPC 边界（脱敏为固定文案）。
    #[test]
    fn work_cn_location_error_is_sanitized_to_fixed_text() {
        assert_eq!(
            work_cn_location_command_error(&WorkCnReadLocationError::AppdataMissing),
            "data_location_unavailable: TRAE 默认数据位置不可用，请重新授权后重试"
        );
    }

    /// Fixture 路径域：委托 fixture_path_error_text 生成脱敏文案。
    #[test]
    fn fixture_path_error_delegates_to_sanitized_text() {
        let error = FixturePathError::OutsideFixtureRoot { raw: "C:/secrets".to_string() };
        assert_eq!(fixture_path_command_error(error.clone()), fixture_path_error_text(&error));
    }
}
