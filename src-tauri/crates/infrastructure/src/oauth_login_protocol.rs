//! P19-B4：OAuth 登录编排协议端口（四差异点参数化 + 共用编排骨架）。
//!
//! 背景（`.scratch/p19-oauth-survey/oauth-findings.md` §7/§8）：TRAE Sync 与
//! WorkBuddy 的网页登录是两种形态——
//! - **TRAE = OAuth 授权码流**：客户端持有 client_id 拼 authorize URL（含
//!   PKCE），本地 socket 回调拦截 AuthCode，再向 token 端点换取凭据
//!   （`checkin_login` 现有管线）；
//! - **WorkBuddy = state 轮询流**：服务端 `POST /v2/plugin/auth/state` 下发
//!   `{authUrl, state}`，系统浏览器打开 authUrl，客户端 1s 轮询
//!   `GET /v2/plugin/auth/token?state=` 取凭据（无 client_id/PKCE/redirect、
//!   轮询端点免认证）。
//!
//! 两种形态共享同一套编排步骤：**authorize 获取 → 授权页打开 → 登录等待
//! 与凭据获取 → 首登验收**。本模块把四个差异点参数化为
//! [`OAuthLoginProtocol`]，由 [`run_login_flow`] 驱动共用的编排骨架
//! （attempt 生命周期、取消传播、浏览器打开失败不中断语义）。
//!
//! 产品映射（差异点 ↔ 既有实现）：
//! | 差异点 | TRAE（`checkin_login`） | WorkBuddy（`workbuddy_login`） |
//! |---|---|---|
//! | ① authorize 获取 | `begin_login_for`（PKCE+回调端口） | `create_login_state`（auth/state） |
//! | ② 授权页打开 | 命令层隔离/本机浏览器 | 系统浏览器透传服务端 authUrl |
//! | ③ 登录等待+凭据获取 | `complete_login`（回调阻塞+AuthCode 换取+GetUserInfo） | `wait_credentials`（1s 轮询+account 补齐） |
//! | ④ 首登验收 | ADR-0027 同设备换发（complete_login 内） | 凭据校验+容器入池（工具不主动刷新，P17 纪律） |
//!
//! TRAE 管线为已验证成熟链路（本地 socket + PKCE + 换发验收一体），
//! 首个生产消费者为 WorkBuddy；TRAE 适配器映射见模块表，是否重排入
//! `run_login_flow` 由后续票评估（避免无验证收益的重构风险）。

use crate::checkin_login::LoginAttemptControl;

/// 编排错误（不含凭据材料；产品侧错误以稳定错误码字符串携带细节）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoginFlowError {
    /// ① authorize 获取失败（TRAE：PKCE/端口分配；WB：auth/state 请求失败）。
    StateCreation(String),
    /// ③ 登录等待或凭据获取失败（超时/协议/服务端拒绝）。
    CredentialFetch(String),
    /// ④ 首登验收失败（凭据不可用或入池落盘失败）。
    Acceptance(String),
    /// 编排在等待阶段被外部取消（用户取消/浏览器退出）。
    Cancelled,
}

impl LoginFlowError {
    /// 稳定错误码（命令层直接透出，前端 safeUiError 文案映射）。
    pub fn code(&self) -> String {
        match self {
            Self::StateCreation(detail) => format!("login_flow_state_creation_failed:{detail}"),
            Self::CredentialFetch(detail) => format!("login_flow_credential_fetch_failed:{detail}"),
            Self::Acceptance(detail) => format!("login_flow_acceptance_failed:{detail}"),
            Self::Cancelled => "login_flow_cancelled".to_string(),
        }
    }
}

/// OAuth 登录编排协议端口：四差异点参数化。
///
/// 实现方持有全部产品特定状态（HTTP 客户端、凭据容器根、轮询参数等）；
/// `LoginState`/`Credentials` 为产品私有中间产物，只在本 trait 的调用链内
/// 流转，不跨产品混用。凭据材料不出现在错误与回执展示面（各实现的
/// Receipt 只含非敏感字段）。
pub trait OAuthLoginProtocol {
    /// ① authorize 获取后的会话句柄（TRAE：LoginSession；WB：state UUID）。
    type LoginState;
    /// ③ 凭据获取产物（TRAE：TokenGrant+UserInfo；WB：auth 对+账号快照）。
    type Credentials;
    /// ④ 首登验收回执（只含非敏感字段；形态与既有登录回执对齐）。
    type Receipt;

    /// ① authorize 获取：创建一次登录会话，返回授权页 URL 与会话句柄。
    /// URL 由编排骨架交给 [`run_login_flow` 的 open_auth_url] 打开。
    fn create_login_state(
        &mut self,
        profile_id: &str,
    ) -> Result<(String, Self::LoginState), LoginFlowError>;

    /// ③ 登录等待 + 凭据获取：阻塞直到取得凭据、失败或被取消。
    ///
    /// `control` 为共享取消控制（与命令层 cancel 共用同一线性化原语，
    /// 语义对齐 TRAE `complete_login`）；`should_abort` 为命令层附加中止
    /// 条件（TRAE：浏览器退出检测）。实现方必须周期求值二者，取消生效时
    /// 返回 [`LoginFlowError::Cancelled`]。
    fn wait_credentials(
        &self,
        state: Self::LoginState,
        control: &LoginAttemptControl,
        should_abort: &dyn Fn() -> bool,
    ) -> Result<Self::Credentials, LoginFlowError>;

    /// ④ 首登验收：凭据可用性校验 + 入池落盘（ADR-0027 同语义：
    /// 失败时凭据与证据保留、不回滚已取得的登录产物、不自动重试）。
    fn first_login_acceptance(
        &self,
        credentials: Self::Credentials,
        now_unix_seconds: u64,
    ) -> Result<Self::Receipt, LoginFlowError>;
}

/// 共用编排骨架：驱动一次完整登录编排。
///
/// 约束（对齐官方客户端语义）：
/// - ② 授权页打开失败**不中断**编排（WorkBuddy 客户端原文
///   "keep polling so manual login can still complete"——用户可手动粘贴
///   URL 或复用既有会话态继续完成）；打开动作由调用方注入（`open_auth_url`），
///   产品差异（隔离 profile/系统默认浏览器）留在命令层；
/// - 取消只在 ③ 等待阶段生效；进入 ④ 验收（发布）后取消请求不追溯
///   （与 TRAE `LoginAttemptControl` 发布阶段纪律一致）；
/// - `now_unix_seconds` 为调用方提供的验收时刻（容器铸造时刻），保证
///   测试可注入、生产取真实时钟。
pub fn run_login_flow<P: OAuthLoginProtocol>(
    protocol: &mut P,
    profile_id: &str,
    control: &LoginAttemptControl,
    open_auth_url: &dyn Fn(&str),
    should_abort: &dyn Fn() -> bool,
    now_unix_seconds: u64,
) -> Result<P::Receipt, LoginFlowError> {
    // ① authorize 获取。
    let (auth_url, state) = protocol.create_login_state(profile_id)?;
    // ② 授权页打开：失败不中断（best-effort；URL 同时由命令层返回给前端
    // 展示，用户可手动复制）。
    open_auth_url(&auth_url);
    // ③ 登录等待 + 凭据获取（cancel-aware）。
    let credentials = protocol.wait_credentials(state, control, should_abort)?;
    // ④ 首登验收（发布阶段；取消不追溯）。
    protocol.first_login_acceptance(credentials, now_unix_seconds)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::Arc;

    /// 测试用协议：脚本化三步产物，记录调用轨迹。
    struct ScriptedProtocol {
        auth_url: String,
        wait_calls: AtomicUsize,
        fail_wait: bool,
        fail_acceptance: bool,
    }

    struct FakeState {
        marker: String,
    }
    struct FakeCredentials {
        value: u32,
    }
    /// 回执只含非敏感字段（凭据正文不进 Receipt）。
    struct FakeReceipt {
        profile_id: String,
        accepted_value: u32,
    }

    impl OAuthLoginProtocol for ScriptedProtocol {
        type LoginState = FakeState;
        type Credentials = FakeCredentials;
        type Receipt = FakeReceipt;

        fn create_login_state(
            &mut self,
            _profile_id: &str,
        ) -> Result<(String, FakeState), LoginFlowError> {
            Ok((self.auth_url.clone(), FakeState { marker: "state".into() }))
        }

        fn wait_credentials(
            &self,
            _state: FakeState,
            _control: &LoginAttemptControl,
            _should_abort: &dyn Fn() -> bool,
        ) -> Result<FakeCredentials, LoginFlowError> {
            self.wait_calls.fetch_add(1, Ordering::SeqCst);
            if self.fail_wait {
                Err(LoginFlowError::CredentialFetch("scripted".into()))
            } else {
                Ok(FakeCredentials { value: 7 })
            }
        }

        fn first_login_acceptance(
            &self,
            credentials: FakeCredentials,
            _now_unix_seconds: u64,
        ) -> Result<FakeReceipt, LoginFlowError> {
            if self.fail_acceptance {
                Err(LoginFlowError::Acceptance("scripted".into()))
            } else {
                Ok(FakeReceipt {
                    profile_id: "fake-profile".into(),
                    accepted_value: credentials.value,
                })
            }
        }
    }

    #[test]
    fn skeleton_drives_four_steps_in_order_and_returns_receipt() {
        let mut protocol = ScriptedProtocol {
            auth_url: "https://auth.example/login".into(),
            wait_calls: AtomicUsize::new(0),
            fail_wait: false,
            fail_acceptance: false,
        };
        let opened = Arc::new(AtomicUsize::new(0));
        let opened_clone = Arc::clone(&opened);
        let control = LoginAttemptControl::new();
        let receipt = run_login_flow(
            &mut protocol,
            "profile-x",
            &control,
            &move |url| {
                assert_eq!(url, "https://auth.example/login");
                opened_clone.fetch_add(1, Ordering::SeqCst);
            },
            &|| false,
            1_800_000_000,
        )
        .expect("flow success");
        assert_eq!(receipt.profile_id, "fake-profile");
        assert_eq!(receipt.accepted_value, 7);
        assert_eq!(opened.load(Ordering::SeqCst), 1, "授权页恰好打开一次");
        assert_eq!(protocol.wait_calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn wait_failure_propagates_without_acceptance() {
        let mut protocol = ScriptedProtocol {
            auth_url: "u".into(),
            wait_calls: AtomicUsize::new(0),
            fail_wait: true,
            fail_acceptance: false,
        };
        let control = LoginAttemptControl::new();
        let error = run_login_flow(&mut protocol, "p", &control, &|_| {}, &|| false, 0)
            .err()
            .expect("wait failure");
        assert_eq!(error, LoginFlowError::CredentialFetch("scripted".into()));
        assert_eq!(error.code(), "login_flow_credential_fetch_failed:scripted");
    }

    #[test]
    fn acceptance_failure_propagates_as_acceptance_error() {
        let mut protocol = ScriptedProtocol {
            auth_url: "u".into(),
            wait_calls: AtomicUsize::new(0),
            fail_wait: false,
            fail_acceptance: true,
        };
        let control = LoginAttemptControl::new();
        let error = run_login_flow(&mut protocol, "p", &control, &|_| {}, &|| false, 0)
            .err()
            .expect("acceptance failure");
        assert_eq!(error, LoginFlowError::Acceptance("scripted".into()));
    }

    #[test]
    fn cancelled_control_is_visible_to_protocol_via_control_parameter() {
        // 编排骨架把共享取消控制原样传给 ③：协议实现可观测取消请求。
        let mut protocol = ScriptedProtocol {
            auth_url: "u".into(),
            wait_calls: AtomicUsize::new(0),
            fail_wait: false,
            fail_acceptance: false,
        };
        struct CancelledProtocol {
            saw_cancel: Arc<AtomicBool>,
        }
        impl OAuthLoginProtocol for CancelledProtocol {
            type LoginState = ();
            type Credentials = ();
            type Receipt = ();
            fn create_login_state(
                &mut self,
                _profile_id: &str,
            ) -> Result<(String, ()), LoginFlowError> {
                Ok(("u".into(), ()))
            }
            fn wait_credentials(
                &self,
                _state: (),
                control: &LoginAttemptControl,
                should_abort: &dyn Fn() -> bool,
            ) -> Result<(), LoginFlowError> {
                let _ = control.cancel();
                if control.should_abort_waiting() || should_abort() {
                    return Err(LoginFlowError::Cancelled);
                }
                Ok(())
            }
            fn first_login_acceptance(
                &self,
                _credentials: (),
                _now_unix_seconds: u64,
            ) -> Result<(), LoginFlowError> {
                Ok(())
            }
        }
        let mut cancelled = CancelledProtocol {
            saw_cancel: Arc::new(AtomicBool::new(false)),
        };
        let control = LoginAttemptControl::new();
        let error = run_login_flow(&mut cancelled, "p", &control, &|_| {}, &|| false, 0)
            .err()
            .expect("cancelled");
        assert_eq!(error, LoginFlowError::Cancelled);
        assert_eq!(error.code(), "login_flow_cancelled");
    }
}
