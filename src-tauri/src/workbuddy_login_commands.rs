//! P19-B4：WorkBuddy 登录编排命令层（begin/complete/cancel 两段式）。
//!
//! 与 Trae OAuth（oauth_login.rs）同构的命令面，但协议为 state 轮询流
//! （探查报告 §1/§8）：
//! - `begin_workbuddy_login`：POST auth/state 下发 `{authUrl, state}` →
//!   authUrl 三可信域白名单校验（§1.3，防任意 URL 透传浏览器）→ 拉起
//!   系统浏览器 → 登记活动 attempt（state + 取消控制）→ 返回 authUrl
//!   供前端展示/手动复制；
//! - `complete_workbuddy_login`：1s 轮询 auth/token（5min 预算，可取消）
//!   → login/account 补账号 → 首登验收整代次入池（workbuddy-login 容器根）
//!   → 脱敏回执；
//! - `cancel_workbuddy_login`：取消等待中的轮询并清理 attempt。
//!
//! 与既有捕获编排（`capture_workbuddy_account`）的关系 = **新增登录入口
//! 而非替换**：捕获仍服务「客户端已登录」场景（本机 .info 解密入池），
//! 本命令面服务「客户端未登录/添加新账号」场景；两者写同一容器根
//! （workbuddy-login），幂等语义一致（同 uid = 整代次原子替换）。
//!
//! 编排与协议实现在 infrastructure（`oauth_login_protocol` +
//! `workbuddy_login`）；begin/complete 跨进程两段式拆开调用 trait 的
//! ① 与 ③④（`run_login_flow` 骨架为一次性流程 API，两段式命令不经它）。

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use serde::Serialize;

use traesync_infrastructure::checkin_login::{LoginAttemptControl, LoginCancelResult};
use traesync_infrastructure::oauth_login_protocol::{LoginFlowError, OAuthLoginProtocol};
use traesync_infrastructure::workbuddy_login::WorkBuddyLoginProtocol;

use crate::run_blocking;
use crate::workbuddy_commands::{ensure_login_real_mode_workbuddy, workbuddy_login_root};
use crate::AppState;

/// WB 活动 attempt：state、取消控制与占用同进同出。
/// state 一次性消费（complete 取走后 cancel 仍可拒绝迟到清理）。
pub(crate) struct WorkBuddyLoginAttempt {
    attempt_id: u64,
    /// 服务端会话标识（轮询凭据的关联键；服务端 5min 有效）。
    state: Option<String>,
    control: LoginAttemptControl,
}

static NEXT_WB_LOGIN_ATTEMPT_ID: AtomicU64 = AtomicU64::new(1);

/// 登录开始回执：authUrl 同时给前端展示（浏览器唤起失败时用户可手动复制）。
#[derive(Debug, Clone, Serialize)]
pub(crate) struct WorkBuddyLoginBeginDto {
    auth_url: String,
    attempt_id: u64,
}

/// 登录完成回执（非敏感白名单，与 Trae 登录回执同语义）。
#[derive(Debug, Clone, Serialize)]
pub(crate) struct WorkBuddyLoginReceiptDto {
    profile_id: String,
    account_id: String,
    screen_name: String,
    /// 首登验收结果：passed / failed（失败不回滚，前端如实展示）。
    acceptance: &'static str,
    /// 验收失败的稳定错误码；通过时省略。
    #[serde(skip_serializing_if = "Option::is_none")]
    acceptance_error_code: Option<&'static str>,
}

/// authUrl origin 白名单校验（纯函数）：服务端下发的授权页 origin 必在
/// 三可信域内（探查报告 §1.3：www.workbuddy.cn / copilot.tencent.com /
/// wb.tencentbuddy.com）。域名漂移/IP 直连形态一律拒绝透传浏览器。
pub(crate) fn auth_url_origin_is_trusted(auth_url: &str) -> bool {
    let Some(rest) = auth_url.strip_prefix("https://") else {
        return false; // 非 https 一律不可信。
    };
    // authority = 到第一个 '/'（或结尾）之前的部分（忽略 userinfo 形态——
    // 服务端不会下发，出现即视为不可信）。
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    if authority.contains('@') {
        return false;
    }
    let origin = format!("https://{authority}");
    traesync_infrastructure::workbuddy_login::WB_TRUSTED_AUTH_ORIGINS.contains(&origin.as_str())
}

/// 开始 WorkBuddy 登录：创建服务端会话并拉起授权页。
#[tauri::command]
pub(crate) fn begin_workbuddy_login(
    state: tauri::State<AppState>,
) -> Result<WorkBuddyLoginBeginDto, String> {
    ensure_login_real_mode_workbuddy(&state)?;
    let login_root = workbuddy_login_root(&state)?;
    // 单槽占用：同一时间只保留一个进行中的 WB 登录（与 Trae 登录语义一致）。
    let mut slot = state
        .active_workbuddy_login
        .lock()
        .map_err(|_| "login_state_unavailable".to_string())?;
    if slot.is_some() {
        return Err("workbuddy_login_already_in_progress".to_string());
    }
    // ① authorize 获取：POST auth/state 下发 {authUrl, state}（真实网络）。
    let mut protocol = WorkBuddyLoginProtocol::from_machine(login_root);
    let (auth_url, login_state) = protocol
        .create_login_state("wb-login")
        .map_err(|error| error.code())?;
    if !auth_url_origin_is_trusted(&auth_url) {
        return Err("workbuddy_login_auth_url_untrusted".to_string());
    }
    let attempt_id = NEXT_WB_LOGIN_ATTEMPT_ID.fetch_add(1, Ordering::Relaxed);
    // ② 授权页打开：系统默认浏览器；失败不中断（URL 已返回前端，用户可
    // 手动粘贴——对齐客户端 "keep polling" 语义）。
    #[cfg(windows)]
    traesync_infrastructure::workbuddy_login::open_auth_url_in_system_browser(&auth_url);
    *slot = Some(WorkBuddyLoginAttempt {
        attempt_id,
        state: Some(login_state),
        control: LoginAttemptControl::new(),
    });
    Ok(WorkBuddyLoginBeginDto {
        auth_url,
        attempt_id,
    })
}

/// 完成 WorkBuddy 登录：轮询凭据 + 补账号 + 首登验收入池（阻塞直到
/// 成功、失败、被取消或超时）。
#[tauri::command]
pub(crate) async fn complete_workbuddy_login(
    attempt_id: u64,
    state: tauri::State<'_, AppState>,
) -> Result<WorkBuddyLoginReceiptDto, String> {
    ensure_login_real_mode_workbuddy(&state)?;
    let login_root = workbuddy_login_root(&state)?;
    // 领取 state（一次性消费）但保留 attempt 占用与取消权。
    let (login_state, control) = claim_workbuddy_login_state(&state, attempt_id)?;
    let active_attempt = Arc::clone(&state.active_workbuddy_login);
    let result = run_blocking("workbuddy_login_join_failed", move || {
        let protocol = WorkBuddyLoginProtocol::from_machine(login_root);
        // ③ 登录等待 + 凭据获取（1s 轮询，cancel-aware）。
        let credentials = protocol
            .wait_credentials(login_state, &control, &|| false)
            .map_err(|error| match error {
                LoginFlowError::Cancelled => "workbuddy_login_cancelled".to_string(),
                other => other.code(),
            })?;
        // ④ 首登验收 + 整代次入池。
        let now = traesync_infrastructure::workbuddy_credential::unix_ms_now() / 1000;
        let receipt = protocol
            .first_login_acceptance(credentials, now)
            .map_err(|error| error.code())?;
        Ok(WorkBuddyLoginReceiptDto {
            profile_id: receipt.profile_id,
            account_id: receipt.account_id,
            screen_name: receipt.screen_name,
            acceptance: receipt.acceptance.status(),
            acceptance_error_code: receipt.acceptance.error_code,
        })
    })
    .await;
    // WB 流无浏览器资源；state 已消费，无论成败都释放 attempt 槽
    // （重试语义 = 重新 begin 取新 state——服务端 state 一次性）。
    if let Ok(mut slot) = active_attempt.lock() {
        if slot.as_ref().map(|active| active.attempt_id) == Some(attempt_id) {
            *slot = None;
        }
    }
    result
}

/// 取消 WorkBuddy 登录：停止轮询等待并清理 attempt 槽。
#[tauri::command]
pub(crate) fn cancel_workbuddy_login(
    attempt_id: u64,
    state: tauri::State<AppState>,
) -> Result<(), String> {
    cancel_workbuddy_login_inner(&state, attempt_id)
}

/// complete 的领取守卫（命令体与测试共用；不触网）：领取一次性 state，
/// 保留 attempt 占用与取消权。
fn claim_workbuddy_login_state(
    state: &AppState,
    attempt_id: u64,
) -> Result<(String, LoginAttemptControl), String> {
    let mut slot = state
        .active_workbuddy_login
        .lock()
        .map_err(|_| "login_state_unavailable".to_string())?;
    let Some(active) = slot.as_mut() else {
        return Err("workbuddy_login_not_started".to_string());
    };
    if active.attempt_id != attempt_id {
        return Err("workbuddy_login_attempt_mismatch".to_string());
    }
    let Some(login_state) = active.state.take() else {
        return Err("workbuddy_login_already_completing".to_string());
    };
    Ok((login_state, active.control.clone()))
}

/// cancel 的内部逻辑（命令体与测试共用）：匹配 attempt 才允许取消，
/// 取消即清槽（WB 流无浏览器资源）。
fn cancel_workbuddy_login_inner(state: &AppState, attempt_id: u64) -> Result<(), String> {
    let mut slot = state
        .active_workbuddy_login
        .lock()
        .map_err(|_| "login_state_unavailable".to_string())?;
    let Some(active) = slot.as_mut() else {
        return Err("workbuddy_login_not_started".to_string());
    };
    if active.attempt_id != attempt_id {
        return Err("workbuddy_login_attempt_mismatch".to_string());
    }
    match active.control.cancel() {
        LoginCancelResult::Cancelled | LoginCancelResult::AlreadyCancelled => {
            *slot = None;
            Ok(())
        }
        LoginCancelResult::TooLate => Err("workbuddy_login_cancel_too_late".to_string()),
        LoginCancelResult::AlreadyFinished => Err("workbuddy_login_already_finished".to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::make_real_mode_state;
    use std::path::Path;

    // ---- authUrl 白名单校验（纯函数离线口径）----

    #[test]
    fn auth_url_whitelist_accepts_only_trusted_https_origins() {
        // 三可信域（探查报告 §1.3）。
        assert!(auth_url_origin_is_trusted(
            "https://www.workbuddy.cn/login?x=1"
        ));
        assert!(auth_url_origin_is_trusted(
            "https://copilot.tencent.com/console/login?a=b"
        ));
        assert!(auth_url_origin_is_trusted("https://wb.tencentbuddy.com"));
        // 篡改域 / 子域伪装 / 非 https / IP 直连 / userinfo 形态：全部拒绝。
        assert!(!auth_url_origin_is_trusted(
            "https://evil.workbuddy.cn/login"
        ));
        assert!(!auth_url_origin_is_trusted(
            "https://www.workbuddy.cn.evil.com/login"
        ));
        assert!(!auth_url_origin_is_trusted("http://www.workbuddy.cn/login"));
        assert!(!auth_url_origin_is_trusted("https://1.2.3.4/login"));
        assert!(!auth_url_origin_is_trusted(
            "https://user@www.workbuddy.cn/login"
        ));
        assert!(!auth_url_origin_is_trusted("not-a-url"));
    }

    // ---- 命令壳门禁与 attempt 槽机制（离线；不触网）----

    #[test]
    fn workbuddy_login_commands_reject_fixture_mode() {
        let mut state = make_real_mode_state(Path::new("/nonexistent"));
        state.runtime_mode = crate::RuntimeMode::Fixture;
        assert_eq!(
            ensure_login_real_mode_workbuddy(&state).unwrap_err(),
            "workbuddy_real_mode_required"
        );
    }

    #[test]
    fn complete_rejects_when_no_attempt_or_mismatched_id() {
        let state = make_real_mode_state(Path::new("/nonexistent"));
        // 未 begin。
        assert_eq!(
            claim_workbuddy_login_state(&state, 1).unwrap_err(),
            "workbuddy_login_not_started"
        );
        // attempt id 不匹配（单槽换新）。
        *state.active_workbuddy_login.lock().unwrap() = Some(WorkBuddyLoginAttempt {
            attempt_id: 7,
            state: Some("state-1".to_string()),
            control: LoginAttemptControl::new(),
        });
        assert_eq!(
            claim_workbuddy_login_state(&state, 8).unwrap_err(),
            "workbuddy_login_attempt_mismatch"
        );
    }

    #[test]
    fn state_is_consumed_once_by_complete_guard() {
        let state = make_real_mode_state(Path::new("/nonexistent"));
        *state.active_workbuddy_login.lock().unwrap() = Some(WorkBuddyLoginAttempt {
            attempt_id: 7,
            state: Some("state-1".to_string()),
            control: LoginAttemptControl::new(),
        });
        let (state_value, _control) = claim_workbuddy_login_state(&state, 7).expect("claim");
        assert_eq!(state_value, "state-1");
        // state 已取走：二次 complete = already_completing。
        assert_eq!(
            claim_workbuddy_login_state(&state, 7).unwrap_err(),
            "workbuddy_login_already_completing"
        );
    }

    #[test]
    fn cancel_clears_slot_and_rejects_mismatch() {
        let state = make_real_mode_state(Path::new("/nonexistent"));
        assert_eq!(
            cancel_workbuddy_login_inner(&state, 1).unwrap_err(),
            "workbuddy_login_not_started"
        );
        let control = LoginAttemptControl::new();
        *state.active_workbuddy_login.lock().unwrap() = Some(WorkBuddyLoginAttempt {
            attempt_id: 7,
            state: Some("state-1".to_string()),
            control: control.clone(),
        });
        // 不匹配的 cancel 不触碰槽与控制。
        assert_eq!(
            cancel_workbuddy_login_inner(&state, 8).unwrap_err(),
            "workbuddy_login_attempt_mismatch"
        );
        assert!(state.active_workbuddy_login.lock().unwrap().is_some());
        // 匹配的 cancel 清空槽并使等待侧可观测取消。
        assert!(cancel_workbuddy_login_inner(&state, 7).is_ok());
        assert!(state.active_workbuddy_login.lock().unwrap().is_none());
        assert!(control.should_abort_waiting());
    }
}
