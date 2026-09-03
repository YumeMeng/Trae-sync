//! 签到应用编排：固定 `status -> claim? -> status`，不持有敏感认证材料。
//!
//! 设备策略（ADR-0019 v6）：每账号独立持久设备；claim 被拒时直接报告失败
//! 并透传业务码，不做自动重铸/冷却重试——由用户决定"稍后重试"还是到账号
//! 详情手动"重置签到设备"（2026-09-02 实测：9074 多为新设备首签过快的
//! 频率风控，换设备反而要重新过信任窗口，自动链弊大于利）。

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime};

use traesync_domain::{CheckinBatchSummary, CheckinOutcome, CheckinResult, CheckinTaskState};
use traesync_ports::{CheckinTransport, CheckinTransportError};

/// 单账号签到应用服务。
pub struct CheckinService<'a> {
    transport: &'a dyn CheckinTransport,
}

impl<'a> CheckinService<'a> {
    pub fn new(transport: &'a dyn CheckinTransport) -> Self {
        Self { transport }
    }

    /// 执行一次单账号签到；`claim` 最多发送一次。
    pub fn run_one(&self, profile_id: &str) -> CheckinResult {
        let started_at = SystemTime::now();
        let mut result = CheckinResult {
            profile_id: profile_id.to_string(),
            outcome: CheckinOutcome::RuntimeError,
            state: CheckinTaskState::LoadingProfile,
            claim_attempted: false,
            before: None,
            after: None,
            detail_code: None,
            started_at,
            finished_at: started_at,
        };

        result.state = CheckinTaskState::StatusBefore;
        let before = match self.transport.status(profile_id) {
            Ok(status) => status,
            Err(error) => {
                result.outcome = map_transport_error(&error);
                result.detail_code = Some(error_code(&error));
                result.state = CheckinTaskState::Completed;
                result.finished_at = SystemTime::now();
                return result;
            }
        };
        result.before = Some(before.clone());

        if before.checked_in {
            result.outcome = CheckinOutcome::AlreadyCheckedIn;
            // 已签即最终状态：before 同时作为 after 快照，供积分缓存回写
            // （否则缓存停留在旧的“今日未签”，UI 账号行需手动刷新才一致）。
            result.after = Some(before.clone());
            result.state = CheckinTaskState::Completed;
            result.finished_at = SystemTime::now();
            return result;
        }
        if !before.enabled {
            result.outcome = CheckinOutcome::NotEligible;
            result.state = CheckinTaskState::Completed;
            result.finished_at = SystemTime::now();
            return result;
        }

        result.state = CheckinTaskState::ClaimingOnce;
        result.claim_attempted = true;
        let claim = self.transport.claim(profile_id);
        if let Err(error) = &claim {
            // claim 可能已经产生副作用；只能读 status 复核，禁止自动重发 claim。
            result.detail_code = Some(error_code(error));
        }

        result.state = CheckinTaskState::StatusAfter;
        let after = match self.transport.status(profile_id) {
            Ok(status) => status,
            Err(error) => {
                result.outcome = if matches!(error, CheckinTransportError::Network) {
                    CheckinOutcome::VerificationFailed
                } else {
                    map_transport_error(&error)
                };
                if result.detail_code.is_none() {
                    result.detail_code = Some(error_code(&error));
                }
                result.state = CheckinTaskState::Completed;
                result.finished_at = SystemTime::now();
                return result;
            }
        };
        result.after = Some(after.clone());

        result.outcome = if after.checked_in {
            if claim.is_ok() {
                CheckinOutcome::Claimed
            } else {
                // 网络中断后最终状态已确认，业务结果仍是已完成签到。
                CheckinOutcome::AlreadyCheckedIn
            }
        } else if let Err(error) = &claim {
            // 业务码拒绝（9074 陌生设备门禁 / 9095 设备日配额等）是服务端
            // 确定性判定，最终状态已确认，直接报告不可领取并透传业务码；
            // 网络类失败无法确认服务端是否已发放，保持失败关闭（结果待复核），
            // 绝不自动再次 claim。
            if matches!(error, CheckinTransportError::Business(_)) {
                CheckinOutcome::NotEligible
            } else {
                CheckinOutcome::VerificationFailed
            }
        } else if after.business_code.is_some_and(|code| code != 0) {
            CheckinOutcome::NotEligible
        } else {
            CheckinOutcome::VerificationFailed
        };
        result.state = CheckinTaskState::Completed;
        result.finished_at = SystemTime::now();
        result
    }

    /// 按用户选择顺序串行执行；取消只阻止尚未开始的账号。
    pub fn run_batch(&self, profile_ids: &[String], cancel: &AtomicBool) -> CheckinBatchSummary {
        let mut summary = CheckinBatchSummary {
            total: profile_ids.len(),
            ..Default::default()
        };
        for profile_id in profile_ids {
            if cancel.load(Ordering::Acquire) {
                summary.cancelled += 1;
                continue;
            }
            let result = self.run_one(profile_id);
            if matches!(
                result.outcome,
                CheckinOutcome::Claimed | CheckinOutcome::AlreadyCheckedIn
            ) {
                summary.completed += 1;
            } else {
                summary.failed += 1;
            }
            summary.results.push(result);
        }
        summary
    }
}

fn map_transport_error(error: &CheckinTransportError) -> CheckinOutcome {
    match error {
        CheckinTransportError::AuthMismatch => CheckinOutcome::AuthMismatch,
        CheckinTransportError::CredentialRefreshFailed => CheckinOutcome::CredentialRefreshFailed,
        CheckinTransportError::ProfileBusy => CheckinOutcome::ProfileBusy,
        CheckinTransportError::Network => CheckinOutcome::NetworkError,
        CheckinTransportError::Protocol | CheckinTransportError::Runtime => {
            CheckinOutcome::RuntimeError
        }
        CheckinTransportError::Business(_) => CheckinOutcome::NotEligible,
    }
}

/// transport 错误 -> 前端可映射的原因码（commands 层积分刷新等只读查询复用）。
pub fn checkin_transport_error_code(error: &CheckinTransportError) -> String {
    match error {
        CheckinTransportError::AuthMismatch => "auth_mismatch",
        CheckinTransportError::CredentialRefreshFailed => "credential_refresh_failed",
        CheckinTransportError::ProfileBusy => "profile_busy",
        CheckinTransportError::Network => "network_error",
        CheckinTransportError::Protocol => "protocol_error",
        // 业务码必须透传具体数值（9074=陌生设备门禁、9095=设备日配额、
        // 20324=refresh 失效），不得压平为单一 business_error（ADR-0019 第 4 条）。
        CheckinTransportError::Business(code) => return format!("business_{code}"),
        CheckinTransportError::Runtime => "runtime_error",
    }
    .to_string()
}

// 模块内旧调用名统一转发到公开实现，避免散落的重复 match。
fn error_code(error: &CheckinTransportError) -> String {
    checkin_transport_error_code(error)
}

/// 批量签到执行器（ADR-0019 v6）。
///
/// v6 起签到链路不再自动重铸设备：每账号就是固定的
/// `status -> 单次 claim -> status 复核`（`CheckinService::run_one`），
/// claim 被拒时直接报告失败并透传业务码，由用户决定后续动作。
///
/// 执行器只负责批量编排骨架：串行逐账号、账号间随机错峰等待、
/// 逐账号进度回调（供 UI 实时反馈）。取消只阻止尚未开始的账号。
pub struct BatchCheckinRunner<'a> {
    /// 每账号执行前重建 transport（凭据可能在上个账号执行期间变化）。
    transport_factory: &'a (dyn Fn() -> Box<dyn CheckinTransport> + 'a),
    /// 账号间随机等待区间（毫秒），None 表示不等待（测试用）。
    inter_account_delay_ms: Option<(u64, u64)>,
    /// 每账号完成（无论成败）后的进度回调；None 时不调用（默认，执行行为不变）。
    progress_callback: Option<&'a (dyn Fn(&CheckinResult) + 'a)>,
    /// 账号间等待回调：某账号完成、下一账号开始前的随机等待期调用一次
    /// （下一账号 profile_id, 等待秒数向上取整）。供 UI 显示"N 秒后开始"——
    /// 3~8 秒错峰间隔不再是无反馈的"签到中…"盲区。
    inter_wait_callback: Option<&'a (dyn Fn(&str, u64) + 'a)>,
}

impl<'a> BatchCheckinRunner<'a> {
    pub fn new(transport_factory: &'a (dyn Fn() -> Box<dyn CheckinTransport> + 'a)) -> Self {
        Self {
            transport_factory,
            inter_account_delay_ms: None,
            progress_callback: None,
            inter_wait_callback: None,
        }
    }

    /// 设置账号间随机等待区间（毫秒）；降低批量连发的机器人特征。
    pub fn with_inter_account_delay(mut self, range_ms: (u64, u64)) -> Self {
        self.inter_account_delay_ms = Some(range_ms);
        self
    }

    /// 设置逐账号进度回调：每账号完成后立即调用（含失败结果），用于 UI 实时反馈。
    pub fn with_progress_callback(
        mut self,
        callback: &'a (dyn Fn(&CheckinResult) + 'a),
    ) -> Self {
        self.progress_callback = Some(callback);
        self
    }

    /// 设置账号间等待回调：进入下一账号前的随机等待期调用一次
    /// （下一账号 profile_id, 等待秒数）。前端收到后本地倒计时。
    pub fn with_inter_wait_callback(
        mut self,
        callback: &'a (dyn Fn(&str, u64) + 'a),
    ) -> Self {
        self.inter_wait_callback = Some(callback);
        self
    }

    /// 批量执行：串行逐账号，账号间随机间隔。
    pub fn run(&self, profile_ids: &[String], cancel: &AtomicBool) -> CheckinBatchSummary {
        let mut summary = CheckinBatchSummary {
            total: profile_ids.len(),
            ..Default::default()
        };
        for (index, profile_id) in profile_ids.iter().enumerate() {
            if cancel.load(Ordering::Acquire) {
                summary.cancelled += 1;
                continue;
            }
            let transport = (self.transport_factory)();
            let result = CheckinService::new(transport.as_ref()).run_one(profile_id);
            if matches!(
                result.outcome,
                CheckinOutcome::Claimed | CheckinOutcome::AlreadyCheckedIn
            ) {
                summary.completed += 1;
            } else {
                summary.failed += 1;
            }
            summary.results.push(result);
            // 逐账号完成即回调：串行批次期间 UI 仍有实时进度可反馈。
            if let Some(on_progress) = self.progress_callback {
                on_progress(summary.results.last().expect("just pushed"));
            }
            // 账号间随机间隔（最后一个账号后不等待）。
            if index + 1 < profile_ids.len() {
                if let Some((lo, hi)) = self.inter_account_delay_ms {
                    let wait_ms = pseudo_random_range(lo, hi);
                    // 等待开始即通知 UI（下一账号 + 秒数；前端本地倒计时）。
                    if let Some(on_wait) = self.inter_wait_callback {
                        // 毫秒向上取整为秒：显示语义是"N 秒后开始"。
                        let wait_secs = wait_ms.div_ceil(1000);
                        on_wait(&profile_ids[index + 1], wait_secs);
                    }
                    std::thread::sleep(Duration::from_millis(wait_ms));
                }
            }
        }
        summary
    }
}

/// 无外部依赖的伪随机数：以单调时钟纳秒取模映射区间，仅供账号间隔抖动。
fn pseudo_random_range(lo: u64, hi: u64) -> u64 {
    if hi <= lo {
        return lo;
    }
    let nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|duration| duration.subsec_nanos() as u64)
        .unwrap_or(0);
    lo + nanos % (hi - lo + 1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::sync::{Arc, Mutex};
    use traesync_domain::{CheckinClaimSnapshot, CheckinStatusSnapshot};

    struct FakeTransport {
        state: Arc<Mutex<BTreeMap<String, CheckinStatusSnapshot>>>,
        calls: Arc<Mutex<Vec<String>>>,
        claim_error: Option<CheckinTransportError>,
    }

    impl CheckinTransport for FakeTransport {
        fn status(&self, profile_id: &str) -> Result<CheckinStatusSnapshot, CheckinTransportError> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("status:{profile_id}"));
            self.state
                .lock()
                .unwrap()
                .get(profile_id)
                .cloned()
                .ok_or(CheckinTransportError::Runtime)
        }

        fn claim(&self, profile_id: &str) -> Result<CheckinClaimSnapshot, CheckinTransportError> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("claim:{profile_id}"));
            if let Some(error) = &self.claim_error {
                return Err(error.clone());
            }
            let mut state = self.state.lock().unwrap();
            let current = state.get_mut(profile_id).unwrap();
            current.checked_in = true;
            current.credits = Some(current.credits.unwrap_or_default() + 10);
            Ok(CheckinClaimSnapshot {
                business_code: Some(0),
                credits: current.credits,
            })
        }

        fn entitlement_usage(
            &self,
            profile_id: &str,
        ) -> Result<traesync_domain::EntitlementUsageSnapshot, CheckinTransportError> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("usage:{profile_id}"));
            Ok(traesync_domain::EntitlementUsageSnapshot::default())
        }
    }

    fn service(fake: &FakeTransport) -> CheckinService<'_> {
        CheckinService::new(fake)
    }

    #[test]
    fn status_claim_status_sends_one_claim_and_reports_claimed() {
        let state = Arc::new(Mutex::new(BTreeMap::from([(
            "a".to_string(),
            CheckinStatusSnapshot {
                enabled: true,
                checked_in: false,
                credits: Some(1),
                business_code: Some(0),
            },
        )])));
        let calls = Arc::new(Mutex::new(Vec::new()));
        let fake = FakeTransport {
            state: state.clone(),
            calls: calls.clone(),
            claim_error: None,
        };
        let result = service(&fake).run_one("a");
        assert_eq!(result.outcome, CheckinOutcome::Claimed);
        assert!(result.claim_attempted);
        assert_eq!(
            calls.lock().unwrap().as_slice(),
            ["status:a", "claim:a", "status:a"]
        );
    }

    #[test]
    fn already_checked_in_never_claims() {
        let state = Arc::new(Mutex::new(BTreeMap::from([(
            "a".to_string(),
            CheckinStatusSnapshot {
                enabled: true,
                checked_in: true,
                credits: Some(10),
                business_code: Some(0),
            },
        )])));
        let calls = Arc::new(Mutex::new(Vec::new()));
        let fake = FakeTransport {
            state,
            calls: calls.clone(),
            claim_error: None,
        };
        let result = service(&fake).run_one("a");
        assert_eq!(result.outcome, CheckinOutcome::AlreadyCheckedIn);
        assert!(!result.claim_attempted);
        // 已签场景也必须带 after 快照，自动批次结束后积分缓存才会回写一致状态。
        let after = result.after.expect("already-checked-in result must carry after snapshot");
        assert_eq!(after.credits, Some(10));
        assert_eq!(calls.lock().unwrap().as_slice(), ["status:a"]);
    }

    #[test]
    fn timeout_like_claim_error_is_verified_without_retry() {
        let state = Arc::new(Mutex::new(BTreeMap::from([(
            "a".to_string(),
            CheckinStatusSnapshot {
                enabled: true,
                checked_in: false,
                credits: Some(1),
                business_code: Some(0),
            },
        )])));
        let calls = Arc::new(Mutex::new(Vec::new()));
        let fake = FakeTransport {
            state,
            calls: calls.clone(),
            claim_error: Some(CheckinTransportError::Network),
        };
        let result = service(&fake).run_one("a");
        assert_eq!(result.outcome, CheckinOutcome::VerificationFailed);
        assert_eq!(
            calls.lock().unwrap().as_slice(),
            ["status:a", "claim:a", "status:a"]
        );
    }

    #[test]
    fn claim_business_rejection_is_not_eligible_with_passthrough_code() {
        // 9074（陌生设备门禁）/9095（设备日配额）：服务端确定性拒绝，
        // 复核确认未签到后应报告 NotEligible 并透传具体业务码（ADR-0019）。
        for (code, detail) in [(9074, "business_9074"), (9095, "business_9095")] {
            let state = Arc::new(Mutex::new(BTreeMap::from([(
                "a".to_string(),
                CheckinStatusSnapshot {
                    enabled: true,
                    checked_in: false,
                    credits: Some(1),
                    business_code: Some(0),
                },
            )])));
            let calls = Arc::new(Mutex::new(Vec::new()));
            let fake = FakeTransport {
                state,
                calls,
                claim_error: Some(CheckinTransportError::Business(code)),
            };
            let result = service(&fake).run_one("a");
            assert_eq!(result.outcome, CheckinOutcome::NotEligible, "code={code}");
            assert_eq!(result.detail_code.as_deref(), Some(detail));
            assert!(result.claim_attempted);
        }
    }

    #[test]
    fn error_code_passes_business_codes_through() {
        // 业务码透传：三类已知业务码不得压平为单一 business_error。
        assert_eq!(
            error_code(&CheckinTransportError::Business(9074)),
            "business_9074"
        );
        assert_eq!(
            error_code(&CheckinTransportError::Business(9095)),
            "business_9095"
        );
        assert_eq!(
            error_code(&CheckinTransportError::Business(20324)),
            "business_20324"
        );
        // 非业务错误分类码保持原样。
        assert_eq!(
            error_code(&CheckinTransportError::Network),
            "network_error"
        );
        assert_eq!(
            error_code(&CheckinTransportError::AuthMismatch),
            "auth_mismatch"
        );
    }

    #[test]
    fn batch_is_serial_and_failure_does_not_block_next_profile() {
        let state = Arc::new(Mutex::new(BTreeMap::from([
            (
                "a".to_string(),
                CheckinStatusSnapshot {
                    enabled: true,
                    checked_in: false,
                    credits: Some(0),
                    business_code: Some(0),
                },
            ),
            (
                "b".to_string(),
                CheckinStatusSnapshot {
                    enabled: true,
                    checked_in: true,
                    credits: Some(4),
                    business_code: Some(0),
                },
            ),
        ])));
        let calls = Arc::new(Mutex::new(Vec::new()));
        let fake = FakeTransport {
            state,
            calls: calls.clone(),
            claim_error: Some(CheckinTransportError::Business(42)),
        };
        let cancel = AtomicBool::new(false);
        let result = service(&fake).run_batch(&["a".to_string(), "b".to_string()], &cancel);
        assert_eq!(result.results.len(), 2);
        assert_eq!(result.completed, 1);
        assert_eq!(result.failed, 1);
        assert_eq!(
            calls.lock().unwrap().as_slice(),
            ["status:a", "claim:a", "status:a", "status:b"]
        );
    }

    // ===== BatchCheckinRunner：批量编排（ADR-0019 v6）=====

    /// 共享测试世界：签到状态 + 每设备 claim 预期。
    struct BatchWorld {
        checked: Mutex<BTreeMap<String, bool>>,
        /// 设备 -> claim 错误（None = 成功）。
        device_errors: Mutex<BTreeMap<String, Option<CheckinTransportError>>>,
        bindings: Mutex<BTreeMap<String, String>>,
    }

    impl BatchWorld {
        fn new(initial_device: &str) -> Arc<Self> {
            Arc::new(Self {
                checked: Mutex::new(BTreeMap::new()),
                device_errors: Mutex::new(BTreeMap::new()),
                bindings: Mutex::new(BTreeMap::from([(
                    "a".to_string(),
                    initial_device.to_string(),
                )])),
            })
        }

        fn set_device_error(&self, device: &str, error: Option<CheckinTransportError>) {
            self.device_errors
                .lock()
                .unwrap()
                .insert(device.to_string(), error);
        }
    }

    struct WorldTransport {
        world: Arc<BatchWorld>,
    }

    impl CheckinTransport for WorldTransport {
        fn status(
            &self,
            profile_id: &str,
        ) -> Result<CheckinStatusSnapshot, CheckinTransportError> {
            let checked = self
                .world
                .checked
                .lock()
                .unwrap()
                .get(profile_id)
                .copied()
                .unwrap_or(false);
            Ok(CheckinStatusSnapshot {
                enabled: true,
                checked_in: checked,
                credits: Some(if checked { 200 } else { 0 }),
                business_code: Some(0),
            })
        }

        fn claim(
            &self,
            profile_id: &str,
        ) -> Result<CheckinClaimSnapshot, CheckinTransportError> {
            let device = self
                .world
                .bindings
                .lock()
                .unwrap()
                .get(profile_id)
                .cloned()
                .unwrap_or_default();
            if let Some(error) = self
                .world
                .device_errors
                .lock()
                .unwrap()
                .get(&device)
                .and_then(|error| error.clone())
            {
                return Err(error);
            }
            self.world
                .checked
                .lock()
                .unwrap()
                .insert(profile_id.to_string(), true);
            Ok(CheckinClaimSnapshot {
                business_code: Some(0),
                credits: Some(200),
            })
        }

        fn entitlement_usage(
            &self,
            _profile_id: &str,
        ) -> Result<traesync_domain::EntitlementUsageSnapshot, CheckinTransportError> {
            Ok(traesync_domain::EntitlementUsageSnapshot::default())
        }
    }

    /// 构造 factory 并执行（factory 是局部闭包，借用它的 runner
    /// 不能跨作用域返回，故 run 必须在同一作用域内完成）。
    macro_rules! run_batch {
        ($world:expr, $ids:expr, $cancel:expr) => {{
            let captured = $world.clone();
            let factory = move || -> Box<dyn CheckinTransport> {
                Box::new(WorldTransport {
                    world: captured.clone(),
                })
            };
            let runner = BatchCheckinRunner::new(&factory);
            runner.run($ids, $cancel)
        }};
    }

    #[test]
    fn gate_9074_reports_failure_without_retry() {
        // v6 核心语义：9074 拒绝后不重铸、不重试，直接报告 NotEligible
        // 并透传业务码；设备绑定保持不变（用户决定后续动作）。
        let world = BatchWorld::new("dev-old");
        world.set_device_error("dev-old", Some(CheckinTransportError::Business(9074)));
        let cancel = AtomicBool::new(false);
        let summary = run_batch!(&world, &["a".to_string()], &cancel);
        let result = &summary.results[0];
        assert_eq!(result.outcome, CheckinOutcome::NotEligible);
        assert_eq!(result.detail_code.as_deref(), Some("business_9074"));
        assert_eq!(summary.failed, 1);
        // 设备绑定不变：签到链路不再自动换设备。
        assert_eq!(
            world.bindings.lock().unwrap().get("a").map(String::as_str),
            Some("dev-old")
        );
    }

    #[test]
    fn quota_9095_reports_failure_without_retry() {
        let world = BatchWorld::new("dev-old");
        world.set_device_error("dev-old", Some(CheckinTransportError::Business(9095)));
        let cancel = AtomicBool::new(false);
        let summary = run_batch!(&world, &["a".to_string()], &cancel);
        let result = &summary.results[0];
        assert_eq!(result.outcome, CheckinOutcome::NotEligible);
        assert_eq!(result.detail_code.as_deref(), Some("business_9095"));
    }

    #[test]
    fn success_on_first_attempt_claims_once() {
        let world = BatchWorld::new("dev-ok");
        let cancel = AtomicBool::new(false);
        let summary = run_batch!(&world, &["a".to_string()], &cancel);
        assert_eq!(summary.results[0].outcome, CheckinOutcome::Claimed);
        assert_eq!(summary.completed, 1);
    }

    #[test]
    fn inter_account_wait_notifies_next_profile_before_sleeping() {
        // 账号间等待开始即回调（下一账号 id + 秒数）：供 UI 显示
        // "N 秒后开始"，错峰间隔不再是无反馈的盲区。
        let world = BatchWorld::new("dev-1");
        world
            .bindings
            .lock()
            .unwrap()
            .insert("b".to_string(), "dev-2".to_string());
        let waits: Mutex<Vec<(String, u64)>> = Mutex::new(Vec::new());
        let factory = || -> Box<dyn CheckinTransport> {
            Box::new(WorldTransport {
                world: world.clone(),
            })
        };
        let on_wait = |profile_id: &str, secs: u64| {
            waits.lock().unwrap().push((profile_id.to_string(), secs));
        };
        let cancel = AtomicBool::new(false);
        let runner = BatchCheckinRunner::new(&factory)
            // 10ms 实际等待（测试速度），ceil 后上报 1 秒（显示语义）。
            .with_inter_account_delay((10, 10))
            .with_inter_wait_callback(&on_wait);
        let summary = runner.run(&["a".to_string(), "b".to_string()], &cancel);
        assert_eq!(summary.completed, 2);
        assert_eq!(
            waits.lock().unwrap().as_slice(),
            [("b".to_string(), 1u64)]
        );
    }
}
