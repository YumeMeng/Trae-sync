//! ManagedAccountSwitch 领域模型。
//!
//! 账号管理只保存非敏感元数据。真实 TRAE 凭证仍由外部账号管理器负责，
//! 本模块不定义 token、cookie 或完整认证对象，也不允许把账号切换等同于历史同步。

use serde::{Deserialize, Serialize};
use std::time::SystemTime;

use crate::SyncScope;

/// 账号档案指纹格式：1 为旧版无 salt，2 为安装级 salt 派生。
pub const LEGACY_ACCOUNT_FINGERPRINT_VERSION: u32 = 1;
pub const ACCOUNT_FINGERPRINT_VERSION: u32 = 2;

fn default_account_fingerprint_version() -> u32 {
    LEGACY_ACCOUNT_FINGERPRINT_VERSION
}

/// 账号证据的最近验证状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AccountVerificationState {
    Unknown,
    Verified,
    SingleSource,
    Conflict,
    Expired,
    FingerprintChanged,
    ManualRecoveryRequired,
}

/// ManagedAccountSwitch 的独立状态机。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AccountSwitchState {
    Idle,
    Preflight,
    WaitingForTraeClosed,
    Applying,
    Verifying,
    Completed,
    Restored,
    ManualRecoveryRequired,
}

/// 可安全保存的账号档案。所有标识均为不可逆指纹或稳定元数据。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AccountProfile {
    pub profile_id: String,
    pub display_name: String,
    pub user_fingerprint: String,
    /// 旧档案缺失该字段时按 legacy 解释，不静默当作新格式。
    #[serde(default = "default_account_fingerprint_version")]
    pub fingerprint_version: u32,
    pub region: Option<String>,
    pub data_location_id: String,
    pub last_verified_at: Option<SystemTime>,
    pub verification_state: AccountVerificationState,
}

/// 当前实时账号证据，不包含原始 user_id 或认证正文。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CurrentAccountEvidence {
    pub profile_id: Option<String>,
    pub display_name: Option<String>,
    pub user_fingerprint: Option<String>,
    #[serde(default = "default_account_fingerprint_version")]
    pub fingerprint_version: u32,
    pub region: Option<String>,
    pub data_location_id: Option<String>,
    pub verification_state: AccountVerificationState,
    pub observed_at: Option<SystemTime>,
    pub reason: Option<String>,
}

impl Default for CurrentAccountEvidence {
    fn default() -> Self {
        Self {
            profile_id: None,
            display_name: None,
            user_fingerprint: None,
            fingerprint_version: LEGACY_ACCOUNT_FINGERPRINT_VERSION,
            region: None,
            data_location_id: None,
            verification_state: AccountVerificationState::Unknown,
            observed_at: None,
            reason: Some("not_checked".to_string()),
        }
    }
}

impl CurrentAccountEvidence {
    /// 从非敏感账号档案构造隔离 fixture 的验证结果。
    pub fn from_profile(profile: &AccountProfile, observed_at: SystemTime) -> Self {
        Self {
            profile_id: Some(profile.profile_id.clone()),
            display_name: Some(profile.display_name.clone()),
            user_fingerprint: Some(profile.user_fingerprint.clone()),
            fingerprint_version: profile.fingerprint_version,
            region: profile.region.clone(),
            data_location_id: Some(profile.data_location_id.clone()),
            verification_state: AccountVerificationState::Verified,
            observed_at: Some(observed_at),
            reason: None,
        }
    }
}

/// 切换前预检结果。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountSwitchPreflight {
    pub source_verified: bool,
    pub target_known: bool,
    pub target_verified: bool,
    pub target_location_known: bool,
    pub pending_sync_plan_cleared: bool,
    pub trae_closed: bool,
    pub ready: bool,
    pub reason: Option<String>,
}

impl AccountSwitchPreflight {
    pub fn blocked(reason: impl Into<String>) -> Self {
        Self {
            source_verified: false,
            target_known: false,
            target_verified: false,
            target_location_known: false,
            pending_sync_plan_cleared: false,
            trae_closed: false,
            ready: false,
            reason: Some(reason.into()),
        }
    }
}

/// 一次账号切换计划。backup_reference 只引用安全元数据，不指向凭证材料。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountSwitchPlan {
    pub plan_id: String,
    pub source_profile_id: Option<String>,
    pub target_profile_id: String,
    pub source_data_location_id: Option<String>,
    pub target_data_location_id: String,
    pub preflight: AccountSwitchPreflight,
    pub backup_reference: Option<String>,
    pub expected_target_fingerprint: String,
    pub state: AccountSwitchState,
    pub created_at: SystemTime,
    pub failure_reason: Option<String>,
}

/// 切换并承接的持久意图状态。
///
/// 意图只保存稳定选择和证据摘要，不保存旧 SyncPlan、正文或登录材料。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HandoffIntentState {
    Prepared,
    Switching,
    TargetVerified,
    PreviewReady,
    Expired,
    ManualRecoveryRequired,
}

/// 跨重启恢复的最小承接意图。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HandoffIntent {
    pub intent_id: String,
    pub source_profile_id: Option<String>,
    pub target_profile_id: String,
    pub data_location_id: String,
    pub scope: SyncScope,
    pub catalog_id: Option<String>,
    pub catalog_generation: Option<String>,
    pub schema_version: Option<String>,
    pub mapping_version: Option<String>,
    /// 仅保存凭证操作的非敏感句柄；不保存凭证内容或恢复路径。
    #[serde(default)]
    pub credential_operation_id: Option<String>,
    pub state: HandoffIntentState,
    pub created_at: SystemTime,
    pub updated_at: SystemTime,
    pub failure_reason: Option<String>,
}

/// 账号中心后端状态。历史库不会因为切换自动改变归属。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ManagedAccountRuntime {
    pub profiles: Vec<AccountProfile>,
    pub current_account: CurrentAccountEvidence,
    pub switch_plan: Option<AccountSwitchPlan>,
}

/// 前端账号中心读取的稳定 DTO。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManagedAccountsView {
    pub profiles: Vec<AccountProfile>,
    pub current_account: CurrentAccountEvidence,
    pub switch_plan: Option<AccountSwitchPlan>,
    pub history_is_separate: bool,
}

impl From<&ManagedAccountRuntime> for ManagedAccountsView {
    fn from(runtime: &ManagedAccountRuntime) -> Self {
        Self {
            profiles: runtime.profiles.clone(),
            current_account: runtime.current_account.clone(),
            switch_plan: runtime.switch_plan.clone(),
            history_is_separate: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile(id: &str) -> AccountProfile {
        AccountProfile {
            profile_id: id.to_string(),
            display_name: format!("账号 {id}"),
            user_fingerprint: format!("fingerprint-{id}"),
            fingerprint_version: ACCOUNT_FINGERPRINT_VERSION,
            region: Some("cn".to_string()),
            data_location_id: format!("location-{id}"),
            last_verified_at: None,
            verification_state: AccountVerificationState::Verified,
        }
    }

    #[test]
    fn current_evidence_never_contains_raw_auth_fields() {
        let value = serde_json::to_value(CurrentAccountEvidence::from_profile(
            &profile("one"),
            SystemTime::UNIX_EPOCH,
        ))
        .unwrap();
        assert!(value.get("token").is_none());
        assert!(value.get("cookies").is_none());
        assert!(value.get("auth").is_none());
    }

    #[test]
    fn runtime_view_explicitly_keeps_history_separate() {
        let view = ManagedAccountsView::from(&ManagedAccountRuntime::default());
        assert!(view.history_is_separate);
    }
}
