//! ManagedAccountSwitch 应用服务：编排账号元数据与独立切换状态机。

use std::time::SystemTime;
use traesync_domain::{
    AccountProfile, AccountSwitchPlan, AccountSwitchPreflight, AccountSwitchState,
    AccountVerificationState, CurrentAccountEvidence, ManagedAccountRuntime,
};
use traesync_ports::ManagedAccountProfileStorePort;

/// 账号管理应用服务错误。错误文本不包含认证正文。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ManagedAccountSwitchError {
    ProfileNotFound,
    PlanNotFound,
    PlanMismatch,
    PreflightBlocked,
    TargetEvidenceMismatch,
    Store(String),
}

impl std::fmt::Display for ManagedAccountSwitchError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ProfileNotFound => formatter.write_str("target_profile_not_found"),
            Self::PlanNotFound => formatter.write_str("account_switch_plan_not_found"),
            Self::PlanMismatch => formatter.write_str("account_switch_plan_mismatch"),
            Self::PreflightBlocked => formatter.write_str("account_switch_preflight_blocked"),
            Self::TargetEvidenceMismatch => {
                formatter.write_str("account_switch_target_evidence_mismatch")
            }
            Self::Store(error) => write!(formatter, "account_profile_store_unavailable:{error}"),
        }
    }
}

impl std::error::Error for ManagedAccountSwitchError {}

/// 账号切换的纯应用编排。真实 TRAE 的凭证交接由外部账号管理器完成。
pub struct ManagedAccountSwitchService<'a> {
    store: &'a dyn ManagedAccountProfileStorePort,
}

impl<'a> ManagedAccountSwitchService<'a> {
    pub fn new(store: &'a dyn ManagedAccountProfileStorePort) -> Self {
        Self { store }
    }

    pub fn load_profiles(
        &self,
        runtime: &mut ManagedAccountRuntime,
    ) -> Result<(), ManagedAccountSwitchError> {
        runtime.profiles = self
            .store
            .load_profiles()
            .map_err(ManagedAccountSwitchError::Store)?;
        Ok(())
    }

    /// 刷新当前账号并把可信的非敏感档案加入列表。
    pub fn refresh_current(
        &self,
        runtime: &mut ManagedAccountRuntime,
        evidence: CurrentAccountEvidence,
    ) -> Result<(), ManagedAccountSwitchError> {
        self.load_profiles(runtime)?;
        runtime.current_account = evidence.clone();
        if evidence.verification_state == AccountVerificationState::Verified {
            if let (
                Some(profile_id),
                Some(display_name),
                Some(user_fingerprint),
                Some(data_location_id),
                Some(last_verified_at),
            ) = (
                evidence.profile_id.clone(),
                evidence.display_name.clone(),
                evidence.user_fingerprint.clone(),
                evidence.data_location_id.clone(),
                evidence.observed_at,
            ) {
                let profile = AccountProfile {
                    profile_id,
                    display_name,
                    user_fingerprint,
                    fingerprint_version: evidence.fingerprint_version,
                    region: evidence.region.clone(),
                    data_location_id,
                    last_verified_at: Some(last_verified_at),
                    verification_state: AccountVerificationState::Verified,
                };
                // 档案合并必须由存储层在跨进程锁内完成，避免两个实例基于同一旧列表
                // 分别保存时互相覆盖。
                runtime.profiles = self
                    .store
                    .upsert_profile(profile)
                    .map_err(ManagedAccountSwitchError::Store)?;
            }
        }
        Ok(())
    }

    pub fn prepare_switch(
        &self,
        runtime: &mut ManagedAccountRuntime,
        target_profile_id: &str,
        mut preflight: AccountSwitchPreflight,
        plan_id: String,
        now: SystemTime,
    ) -> Result<(), ManagedAccountSwitchError> {
        self.load_profiles(runtime)?;
        let target = runtime
            .profiles
            .iter()
            .find(|profile| profile.profile_id == target_profile_id)
            .ok_or(ManagedAccountSwitchError::ProfileNotFound)?;
        preflight.target_known = true;
        preflight.target_verified = target.verification_state == AccountVerificationState::Verified
            && target.fingerprint_version == traesync_domain::ACCOUNT_FINGERPRINT_VERSION
            && !target.user_fingerprint.is_empty();
        preflight.target_location_known = !target.data_location_id.is_empty()
            && runtime.current_account.data_location_id.as_deref()
                == Some(target.data_location_id.as_str());
        preflight.ready = preflight.source_verified
            && preflight.target_known
            && preflight.target_verified
            && preflight.target_location_known
            && preflight.pending_sync_plan_cleared;
        if !preflight.ready && preflight.reason.is_none() {
            preflight.reason = Some(if !preflight.source_verified {
                "source_account_not_verified".to_string()
            } else if !preflight.target_verified {
                "target_account_not_verified".to_string()
            } else if !preflight.target_location_known {
                "target_data_location_mismatch".to_string()
            } else {
                "pending_sync_plan_present".to_string()
            });
        }
        runtime.switch_plan = Some(AccountSwitchPlan {
            plan_id,
            source_profile_id: runtime.current_account.profile_id.clone(),
            target_profile_id: target.profile_id.clone(),
            source_data_location_id: runtime.current_account.data_location_id.clone(),
            target_data_location_id: target.data_location_id.clone(),
            preflight,
            backup_reference: Some("managed-account-state-only".to_string()),
            expected_target_fingerprint: target.user_fingerprint.clone(),
            state: AccountSwitchState::Preflight,
            created_at: now,
            failure_reason: None,
        });
        Ok(())
    }

    pub fn mark_waiting_for_trae_closed(
        &self,
        runtime: &mut ManagedAccountRuntime,
        plan_id: &str,
    ) -> Result<(), ManagedAccountSwitchError> {
        let plan = runtime
            .switch_plan
            .as_mut()
            .ok_or(ManagedAccountSwitchError::PlanNotFound)?;
        if plan.plan_id != plan_id {
            return Err(ManagedAccountSwitchError::PlanMismatch);
        }
        if !plan.preflight.ready {
            return Err(ManagedAccountSwitchError::PreflightBlocked);
        }
        plan.preflight.trae_closed = false;
        plan.state = AccountSwitchState::WaitingForTraeClosed;
        Ok(())
    }

    /// 凭证库已完成原子替换后的中间态；等待用户重新启动 TRAE 并复核目标证据。
    pub fn mark_applying_after_credential(
        &self,
        runtime: &mut ManagedAccountRuntime,
        plan_id: &str,
    ) -> Result<(), ManagedAccountSwitchError> {
        let plan = runtime
            .switch_plan
            .as_mut()
            .ok_or(ManagedAccountSwitchError::PlanNotFound)?;
        if plan.plan_id != plan_id {
            return Err(ManagedAccountSwitchError::PlanMismatch);
        }
        if !plan.preflight.ready {
            return Err(ManagedAccountSwitchError::PreflightBlocked);
        }
        plan.preflight.trae_closed = true;
        plan.state = AccountSwitchState::Applying;
        Ok(())
    }

    /// 用重新授权后读取到的目标证据完成切换复核。
    ///
    /// 该方法只改变内存中的账号状态，不触碰 TRAE 文件或数据库；因此可用于
    /// 生产只读流程的“外部切换后复核”，也可用于 fixture 测试。
    pub fn complete_switch(
        &self,
        runtime: &mut ManagedAccountRuntime,
        plan_id: &str,
        observed: &CurrentAccountEvidence,
        now: SystemTime,
    ) -> Result<(), ManagedAccountSwitchError> {
        self.load_profiles(runtime)?;
        let plan = runtime
            .switch_plan
            .as_ref()
            .ok_or(ManagedAccountSwitchError::PlanNotFound)?;
        if plan.plan_id != plan_id {
            return Err(ManagedAccountSwitchError::PlanMismatch);
        }
        if !plan.preflight.ready {
            return Err(ManagedAccountSwitchError::PreflightBlocked);
        }
        let target = runtime
            .profiles
            .iter()
            .find(|profile| profile.profile_id == plan.target_profile_id)
            .cloned()
            .ok_or(ManagedAccountSwitchError::ProfileNotFound)?;
        let evidence_matches = observed.verification_state == AccountVerificationState::Verified
            && observed.profile_id.as_deref() == Some(target.profile_id.as_str())
            && observed.user_fingerprint.as_deref() == Some(target.user_fingerprint.as_str())
            && observed.data_location_id.as_deref() == Some(target.data_location_id.as_str());
        if !evidence_matches {
            return Err(ManagedAccountSwitchError::TargetEvidenceMismatch);
        }
        if let Some(plan) = runtime.switch_plan.as_mut() {
            plan.preflight.trae_closed = true;
            plan.state = AccountSwitchState::Applying;
        }
        if let Some(plan) = runtime.switch_plan.as_mut() {
            plan.state = AccountSwitchState::Verifying;
        }
        runtime.current_account = observed.clone();
        runtime.current_account.observed_at = Some(now);
        if let Some(plan) = runtime.switch_plan.as_mut() {
            plan.state = AccountSwitchState::Completed;
            plan.failure_reason = None;
        }
        Ok(())
    }

    /// 兼容旧 fixture 测试名称；真实命令使用 `complete_switch`。
    #[cfg(test)]
    pub fn complete_fixture_switch(
        &self,
        runtime: &mut ManagedAccountRuntime,
        plan_id: &str,
        observed: &CurrentAccountEvidence,
        now: SystemTime,
    ) -> Result<(), ManagedAccountSwitchError> {
        self.complete_switch(runtime, plan_id, observed, now)
    }

    pub fn mark_manual_recovery_required(
        &self,
        runtime: &mut ManagedAccountRuntime,
        plan_id: &str,
        reason: impl Into<String>,
    ) -> Result<(), ManagedAccountSwitchError> {
        let plan = runtime
            .switch_plan
            .as_mut()
            .ok_or(ManagedAccountSwitchError::PlanNotFound)?;
        if plan.plan_id != plan_id {
            return Err(ManagedAccountSwitchError::PlanMismatch);
        }
        plan.state = AccountSwitchState::ManualRecoveryRequired;
        plan.failure_reason = Some(reason.into());
        runtime.current_account.verification_state =
            AccountVerificationState::ManualRecoveryRequired;
        runtime.current_account.reason = plan.failure_reason.clone();
        Ok(())
    }

    pub fn mark_restored(
        &self,
        runtime: &mut ManagedAccountRuntime,
        plan_id: &str,
        source_profile_id: Option<&str>,
        now: SystemTime,
    ) -> Result<(), ManagedAccountSwitchError> {
        self.load_profiles(runtime)?;
        let source = source_profile_id
            .and_then(|id| {
                runtime
                    .profiles
                    .iter()
                    .find(|profile| profile.profile_id == id)
            })
            .cloned();
        let plan = runtime
            .switch_plan
            .as_mut()
            .ok_or(ManagedAccountSwitchError::PlanNotFound)?;
        if plan.plan_id != plan_id {
            return Err(ManagedAccountSwitchError::PlanMismatch);
        }
        plan.state = AccountSwitchState::Restored;
        plan.failure_reason = None;
        if let Some(source) = source {
            runtime.current_account = CurrentAccountEvidence::from_profile(&source, now);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    struct FakeStore(Mutex<Vec<AccountProfile>>);

    impl ManagedAccountProfileStorePort for FakeStore {
        fn load_profiles(&self) -> Result<Vec<AccountProfile>, String> {
            Ok(self.0.lock().unwrap().clone())
        }

        fn upsert_profile(&self, profile: AccountProfile) -> Result<Vec<AccountProfile>, String> {
            let mut profiles = self.0.lock().unwrap();
            if let Some(existing) = profiles.iter_mut().find(|existing| {
                existing.data_location_id == profile.data_location_id
                    && existing.user_fingerprint == profile.user_fingerprint
                    && existing.fingerprint_version == profile.fingerprint_version
            }) {
                *existing = profile;
            } else {
                profiles.push(profile);
            }
            Ok(profiles.clone())
        }
    }

    fn profile(id: &str) -> AccountProfile {
        AccountProfile {
            profile_id: id.to_string(),
            display_name: id.to_string(),
            user_fingerprint: format!("fp-{id}"),
            fingerprint_version: traesync_domain::ACCOUNT_FINGERPRINT_VERSION,
            region: None,
            data_location_id: format!("loc-{id}"),
            last_verified_at: Some(SystemTime::UNIX_EPOCH),
            verification_state: AccountVerificationState::Verified,
        }
    }

    #[test]
    fn fixture_switch_reaches_completed_and_changes_only_account_view() {
        let mut target = profile("target");
        target.data_location_id = "loc-source".to_string();
        let target_evidence = CurrentAccountEvidence::from_profile(&target, SystemTime::UNIX_EPOCH);
        let store = FakeStore(Mutex::new(vec![profile("source"), target]));
        let service = ManagedAccountSwitchService::new(&store);
        let mut runtime = ManagedAccountRuntime {
            profiles: Vec::new(),
            current_account: CurrentAccountEvidence::from_profile(
                &profile("source"),
                SystemTime::UNIX_EPOCH,
            ),
            switch_plan: None,
        };
        service
            .prepare_switch(
                &mut runtime,
                "target",
                AccountSwitchPreflight {
                    source_verified: true,
                    target_known: false,
                    target_verified: false,
                    target_location_known: false,
                    pending_sync_plan_cleared: true,
                    trae_closed: false,
                    ready: false,
                    reason: None,
                },
                "plan-1".to_string(),
                SystemTime::UNIX_EPOCH,
            )
            .unwrap();
        service
            .complete_fixture_switch(
                &mut runtime,
                "plan-1",
                &target_evidence,
                SystemTime::UNIX_EPOCH,
            )
            .unwrap();
        assert_eq!(
            runtime.current_account.profile_id.as_deref(),
            Some("target")
        );
        assert_eq!(
            runtime.switch_plan.as_ref().unwrap().state,
            AccountSwitchState::Completed
        );
    }

    #[test]
    fn real_switch_failure_is_explicit_manual_recovery() {
        let store = FakeStore(Mutex::new(vec![profile("target")]));
        let service = ManagedAccountSwitchService::new(&store);
        let mut runtime = ManagedAccountRuntime::default();
        runtime.current_account =
            CurrentAccountEvidence::from_profile(&profile("source"), SystemTime::UNIX_EPOCH);
        service
            .prepare_switch(
                &mut runtime,
                "target",
                AccountSwitchPreflight {
                    source_verified: true,
                    target_known: false,
                    target_verified: false,
                    target_location_known: false,
                    pending_sync_plan_cleared: true,
                    trae_closed: false,
                    ready: false,
                    reason: None,
                },
                "plan-2".to_string(),
                SystemTime::UNIX_EPOCH,
            )
            .unwrap();
        service
            .mark_manual_recovery_required(
                &mut runtime,
                "plan-2",
                "external_account_manager_required",
            )
            .unwrap();
        assert_eq!(
            runtime.switch_plan.as_ref().unwrap().state,
            AccountSwitchState::ManualRecoveryRequired
        );
    }

    #[test]
    fn stale_target_is_blocked_before_fixture_completion() {
        let mut target = profile("target");
        target.data_location_id = "loc-source".to_string();
        target.verification_state = AccountVerificationState::Expired;
        let store = FakeStore(Mutex::new(vec![profile("source"), target]));
        let service = ManagedAccountSwitchService::new(&store);
        let mut runtime = ManagedAccountRuntime {
            profiles: Vec::new(),
            current_account: CurrentAccountEvidence::from_profile(
                &profile("source"),
                SystemTime::UNIX_EPOCH,
            ),
            switch_plan: None,
        };

        service
            .prepare_switch(
                &mut runtime,
                "target",
                AccountSwitchPreflight {
                    source_verified: true,
                    target_known: false,
                    target_verified: false,
                    target_location_known: false,
                    pending_sync_plan_cleared: true,
                    trae_closed: false,
                    ready: false,
                    reason: None,
                },
                "plan-stale".to_string(),
                SystemTime::UNIX_EPOCH,
            )
            .unwrap();

        let plan = runtime.switch_plan.as_ref().unwrap();
        assert!(!plan.preflight.ready);
        assert!(!plan.preflight.target_verified);
        assert_eq!(
            plan.preflight.reason.as_deref(),
            Some("target_account_not_verified")
        );
    }

    #[test]
    fn legacy_fingerprint_target_is_blocked_before_fixture_completion() {
        let mut target = profile("target");
        target.data_location_id = "loc-source".to_string();
        target.fingerprint_version = traesync_domain::LEGACY_ACCOUNT_FINGERPRINT_VERSION;
        let store = FakeStore(Mutex::new(vec![profile("source"), target]));
        let service = ManagedAccountSwitchService::new(&store);
        let mut runtime = ManagedAccountRuntime {
            profiles: Vec::new(),
            current_account: CurrentAccountEvidence::from_profile(
                &profile("source"),
                SystemTime::UNIX_EPOCH,
            ),
            switch_plan: None,
        };

        service
            .prepare_switch(
                &mut runtime,
                "target",
                AccountSwitchPreflight {
                    source_verified: true,
                    target_known: false,
                    target_verified: false,
                    target_location_known: false,
                    pending_sync_plan_cleared: true,
                    trae_closed: false,
                    ready: false,
                    reason: None,
                },
                "plan-legacy-fingerprint".to_string(),
                SystemTime::UNIX_EPOCH,
            )
            .unwrap();

        let plan = runtime.switch_plan.as_ref().unwrap();
        assert!(!plan.preflight.ready);
        assert!(!plan.preflight.target_verified);
        assert_eq!(
            plan.preflight.reason.as_deref(),
            Some("target_account_not_verified")
        );
    }

    #[test]
    fn cross_location_target_is_blocked_before_fixture_completion() {
        let store = FakeStore(Mutex::new(vec![profile("source"), profile("target")]));
        let service = ManagedAccountSwitchService::new(&store);
        let mut runtime = ManagedAccountRuntime {
            profiles: Vec::new(),
            current_account: CurrentAccountEvidence::from_profile(
                &profile("source"),
                SystemTime::UNIX_EPOCH,
            ),
            switch_plan: None,
        };

        service
            .prepare_switch(
                &mut runtime,
                "target",
                AccountSwitchPreflight {
                    source_verified: true,
                    target_known: false,
                    target_verified: false,
                    target_location_known: false,
                    pending_sync_plan_cleared: true,
                    trae_closed: false,
                    ready: false,
                    reason: None,
                },
                "plan-location".to_string(),
                SystemTime::UNIX_EPOCH,
            )
            .unwrap();

        let plan = runtime.switch_plan.as_ref().unwrap();
        assert!(!plan.preflight.ready);
        assert!(!plan.preflight.target_location_known);
        assert_eq!(
            plan.preflight.reason.as_deref(),
            Some("target_data_location_mismatch")
        );
    }

    #[test]
    fn fixture_completion_rejects_mismatched_reobserved_evidence() {
        let mut target = profile("target");
        target.data_location_id = "loc-source".to_string();
        let store = FakeStore(Mutex::new(vec![profile("source"), target.clone()]));
        let service = ManagedAccountSwitchService::new(&store);
        let mut runtime = ManagedAccountRuntime {
            profiles: Vec::new(),
            current_account: CurrentAccountEvidence::from_profile(
                &profile("source"),
                SystemTime::UNIX_EPOCH,
            ),
            switch_plan: None,
        };
        service
            .prepare_switch(
                &mut runtime,
                "target",
                AccountSwitchPreflight {
                    source_verified: true,
                    target_known: false,
                    target_verified: false,
                    target_location_known: false,
                    pending_sync_plan_cleared: true,
                    trae_closed: false,
                    ready: false,
                    reason: None,
                },
                "plan-evidence".to_string(),
                SystemTime::UNIX_EPOCH,
            )
            .unwrap();

        let mut wrong_evidence =
            CurrentAccountEvidence::from_profile(&target, SystemTime::UNIX_EPOCH);
        wrong_evidence.user_fingerprint = Some("different-fingerprint".to_string());
        let error = service
            .complete_fixture_switch(
                &mut runtime,
                "plan-evidence",
                &wrong_evidence,
                SystemTime::UNIX_EPOCH,
            )
            .unwrap_err();
        assert_eq!(error, ManagedAccountSwitchError::TargetEvidenceMismatch);
        assert_ne!(
            runtime.switch_plan.as_ref().unwrap().state,
            AccountSwitchState::Completed
        );
    }

    #[test]
    fn profile_identity_uses_full_location_and_fingerprint_identity() {
        let store = FakeStore(Mutex::new(vec![profile("one")]));
        let mut same_prefix_different_identity = profile("two");
        same_prefix_different_identity.profile_id = "one".to_string();
        same_prefix_different_identity.data_location_id = "loc-one".to_string();
        let profiles = store
            .upsert_profile(same_prefix_different_identity)
            .unwrap();
        assert_eq!(profiles.len(), 2);
    }
}
