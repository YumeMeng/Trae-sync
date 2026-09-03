//! 签到 transport 的基础设施实现。
//!
//! 当前候选只提供 fixture transport，用来验收状态机、批量顺序和错误收口。
//! 真实 HTTP 必须经过独立 Gate 与显式开关，不能由此模块隐式联网。

use std::collections::BTreeMap;
use std::sync::Mutex;

use traesync_domain::{
    CheckinClaimSnapshot, CheckinStatusSnapshot, EntitlementPackSnapshot,
    EntitlementUsageSnapshot,
};
use traesync_ports::{CheckinTransport, CheckinTransportError};

#[derive(Debug, Clone)]
struct FixtureProfileState {
    status: CheckinStatusSnapshot,
}

/// 可重复、无网络副作用的签到 transport。
pub struct FixtureCheckinTransport {
    profiles: Mutex<BTreeMap<String, FixtureProfileState>>,
}

impl FixtureCheckinTransport {
    /// 为选中的档案建立独立状态；每次命令调用都会重新建立，避免跨任务串状态。
    pub fn for_profiles(profile_ids: &[String]) -> Self {
        let profiles = profile_ids
            .iter()
            .map(|profile_id| {
                (
                    profile_id.clone(),
                    FixtureProfileState {
                        status: CheckinStatusSnapshot {
                            enabled: true,
                            checked_in: false,
                            credits: Some(0),
                            business_code: Some(0),
                        },
                    },
                )
            })
            .collect();
        Self {
            profiles: Mutex::new(profiles),
        }
    }
}

impl CheckinTransport for FixtureCheckinTransport {
    fn status(&self, profile_id: &str) -> Result<CheckinStatusSnapshot, CheckinTransportError> {
        self.profiles
            .lock()
            .map_err(|_| CheckinTransportError::Runtime)?
            .get(profile_id)
            .map(|profile| profile.status.clone())
            .ok_or(CheckinTransportError::AuthMismatch)
    }

    fn claim(&self, profile_id: &str) -> Result<CheckinClaimSnapshot, CheckinTransportError> {
        let mut profiles = self
            .profiles
            .lock()
            .map_err(|_| CheckinTransportError::Runtime)?;
        let profile = profiles
            .get_mut(profile_id)
            .ok_or(CheckinTransportError::AuthMismatch)?;
        if profile.status.checked_in {
            return Ok(CheckinClaimSnapshot {
                business_code: Some(1),
                credits: profile.status.credits,
            });
        }
        profile.status.checked_in = true;
        profile.status.credits = Some(profile.status.credits.unwrap_or_default() + 10);
        Ok(CheckinClaimSnapshot {
            business_code: Some(0),
            credits: profile.status.credits,
        })
    }

    fn entitlement_usage(
        &self,
        profile_id: &str,
    ) -> Result<EntitlementUsageSnapshot, CheckinTransportError> {
        self.profiles
            .lock()
            .map_err(|_| CheckinTransportError::Runtime)?
            .get(profile_id)
            .ok_or(CheckinTransportError::AuthMismatch)?;
        // 演示值：单个 500 上限、已用 260 的示例包，剩余 240。
        Ok(EntitlementUsageSnapshot {
            remaining_credits: 240.0,
            packs: vec![EntitlementPackSnapshot {
                entitlement_id: "fixture-pack-demo".to_string(),
                group_name: "演示积分包".to_string(),
                credits_limit: 500.0,
                credits_used: 260.0,
                expires_at_unix_seconds: 0,
            }],
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixture_transport_claims_once_and_increases_credits() {
        let ids = vec!["profile-a".to_string()];
        let transport = FixtureCheckinTransport::for_profiles(&ids);
        let before = transport.status("profile-a").unwrap();
        assert!(!before.checked_in);
        assert_eq!(before.credits, Some(0));
        transport.claim("profile-a").unwrap();
        let after = transport.status("profile-a").unwrap();
        assert!(after.checked_in);
        assert_eq!(after.credits, Some(10));
        let second = transport.claim("profile-a").unwrap();
        assert_eq!(second.business_code, Some(1));
        assert_eq!(transport.status("profile-a").unwrap().credits, Some(10));
    }

    #[test]
    fn fixture_transport_rejects_unknown_profile_without_network() {
        let transport = FixtureCheckinTransport::for_profiles(&[]);
        assert_eq!(
            transport.status("missing"),
            Err(CheckinTransportError::AuthMismatch)
        );
        assert_eq!(
            transport.claim("missing"),
            Err(CheckinTransportError::AuthMismatch)
        );
    }
}
