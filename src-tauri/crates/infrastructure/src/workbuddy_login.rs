//! P19-B4：WorkBuddy 登录编排实现（state 轮询流，探查报告 §8 参数表落地）。
//!
//! 协议事实（asar 逆向 + daemon 日志佐证，`.scratch/p19-oauth-survey/
//! oauth-findings.md` §1/§3/§6）：
//! - ① `POST {origin}/v2/plugin/auth/state?platform=workbuddy`（空 body +
//!   `X-Domain` + 4 个 `X-No-*` 豁免头）→ `data = {authUrl, state}`；
//!   authUrl 由服务端下发（客户端不拼参数，无 client_id/PKCE/redirect）；
//! - ② 系统浏览器打开 authUrl（打开失败不中断——用户可手动粘贴 URL 继续）；
//! - ③ `GET {origin}/v2/plugin/auth/token?state={state}`：间隔 1s、总超时
//!   5min；信封 `code = 11217` = pending 继续；`data.accessToken` 非空即
//!   成功（`hasUsableAuthToken` 口径）；**轮询端点免认证**（无 Bearer，
//!   服务端以 state 关联会话），带同样的豁免头；
//! - ④ `GET {origin}/v2/plugin/login/account?state=`（Bearer + X-Domain）：
//!   `code = 12151` = pending；HTTP 401/403 重试 1 次；
//!   `GET {origin}/v2/plugin/accounts`（Bearer）多账号快照（best-effort）。
//!
//! 首登验收（本工具口径）：uid 非空 + token 非空 + 设备标识可读 → 构造
//! [`WorkBuddyLoginRecord`] 整代次入池（`workbuddy-login` 容器根，明文 JSON）。
//! **工具不主动发起刷新**（P17-T4c⑤ 纪律：refreshToken 一次性轮换，工具侧
//! 轮换会作废客户端副本）；到期字段由登录响应的 `expiresIn`/
//! `refreshExpiresIn` 换算。
//!
//! HTTP 链路离线口径（与签到测试同模式）：[`WorkBuddyLoginHttp`] seam 注入
//! 假实现固化请求形状与错误映射；轮询节拍经 `max_polls` + 可注入 sleep
//! 参数化，单测零真实等待。

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use serde::Deserialize;

use crate::checkin_login::{LoginAcceptance, LoginAttemptControl};
use crate::oauth_login_protocol::{LoginFlowError, OAuthLoginProtocol};
use crate::workbuddy_capture::load_device_id;
use crate::workbuddy_login_credential::{WorkBuddyLoginRecord, WorkBuddyLoginStore};

/// WorkBuddy 认证 API origin（daemon 日志实测：全部 `https://www.workbuddy.cn`）。
pub const WB_AUTH_API_ORIGIN: &str = "https://www.workbuddy.cn";
/// 授权页可信 origin 白名单（探查报告 §1.3：服务端下发 authUrl 的 origin
/// 必在此三者内；命令层开浏览器前校验，防任意 URL 透传）。
pub const WB_TRUSTED_AUTH_ORIGINS: [&str; 3] = [
    "https://www.workbuddy.cn",
    "https://copilot.tencent.com",
    "https://wb.tencentbuddy.com",
];
/// token 轮询间隔（官方 `SIGN_IN_FETCH_INTERVAL = 1e3`，daemon 日志间隔 ≈1.09s 吻合）。
pub const WB_POLL_INTERVAL: Duration = Duration::from_secs(1);
/// token 轮询总预算（官方 `SIGN_IN_PENDING_TIMEOUT = 3e5` = 5min）。
pub const WB_POLL_TIMEOUT: Duration = Duration::from_secs(300);
/// 信封业务码：token 轮询 pending。
pub const WB_CODE_TOKEN_PENDING: i64 = 11217;
/// 信封业务码：login/account pending。
pub const WB_CODE_ACCOUNT_PENDING: i64 = 12151;
/// login/account 的 401/403 重试次数（官方 `MAX_ACCOUNT_AUTH_RETRIES = 1`）。
pub const WB_ACCOUNT_AUTH_RETRIES: u32 = 1;

// ============================================================================
// HTTP seam（生产 Reqwest 适配器 + 可注入边界）
// ============================================================================

/// 登录协议 HTTP 请求描述（纯构造，测试可断言 URL/方法/头/体形状）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WbLoginHttpRequest {
    pub method: &'static str,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: String,
}

/// 登录协议 HTTP 响应（原始状态 + 体；信封解析在协议层做，便于注入测试）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WbLoginHttpResponse {
    pub status: u16,
    pub body: String,
}

/// 登录协议错误（不含凭据材料）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkBuddyLoginError {
    /// 请求构造/发送失败（网络层）。
    Network,
    /// 服务端返回非 2xx。
    Http(u16),
    /// 响应不是 JSON 信封。
    Protocol,
}

impl WorkBuddyLoginError {
    pub fn code(&self) -> String {
        match self {
            Self::Network => "workbuddy_login_network_failed".to_string(),
            Self::Http(status) => format!("workbuddy_login_http_{status}"),
            Self::Protocol => "workbuddy_login_protocol_invalid".to_string(),
        }
    }
}

/// 登录协议 HTTP seam：发送请求描述并返回原始响应。
pub trait WorkBuddyLoginHttp: Send + Sync {
    fn send(&self, request: WbLoginHttpRequest) -> Result<WbLoginHttpResponse, WorkBuddyLoginError>;
}

/// 豁免头构造（纯函数；探查报告 §1.1 authStateHeaderComposer + 4 豁免头）：
/// `X-Domain`（API origin authority）+ `X-No-Authorization`/
/// `X-No-User-Id`/`X-No-Enterprise-Id`/`X-No-Department-Info`（均 true）。
/// auth/state 与 auth/token 轮询共用（轮询端点免认证——服务端以 state
/// 关联会话，工具侧无需也无处取得「本轮登录」的 Bearer）。
pub fn wb_exemption_headers(domain: &str) -> Vec<(String, String)> {
    vec![
        ("X-Domain".to_string(), domain.to_string()),
        ("X-No-Authorization".to_string(), "true".to_string()),
        ("X-No-User-Id".to_string(), "true".to_string()),
        ("X-No-Enterprise-Id".to_string(), "true".to_string()),
        ("X-No-Department-Info".to_string(), "true".to_string()),
    ]
}

/// ① auth/state 请求构造（空 body）。
pub fn build_state_request(origin: &str) -> WbLoginHttpRequest {
    WbLoginHttpRequest {
        method: "POST",
        url: format!("{origin}/v2/plugin/auth/state?platform=workbuddy"),
        headers: wb_exemption_headers(origin.trim_start_matches("https://")),
        body: String::new(),
    }
}

/// ③ token 轮询请求构造（免认证：无 Bearer）。
pub fn build_token_poll_request(origin: &str, state: &str) -> WbLoginHttpRequest {
    WbLoginHttpRequest {
        method: "GET",
        url: format!("{origin}/v2/plugin/auth/token?state={state}"),
        headers: wb_exemption_headers(origin.trim_start_matches("https://")),
        body: String::new(),
    }
}

/// ④ login/account 请求构造（Bearer + X-Domain）。
pub fn build_account_request(origin: &str, state: &str, access_token: &str) -> WbLoginHttpRequest {
    let mut headers = wb_exemption_headers(origin.trim_start_matches("https://"));
    headers.push(("Authorization".to_string(), format!("Bearer {access_token}")));
    WbLoginHttpRequest {
        method: "GET",
        url: format!("{origin}/v2/plugin/login/account?state={state}"),
        headers,
        body: String::new(),
    }
}

/// ⑤ accounts 快照请求构造（Bearer；多账号盘点）。
pub fn build_snapshot_request(origin: &str, access_token: &str) -> WbLoginHttpRequest {
    WbLoginHttpRequest {
        method: "GET",
        url: format!("{origin}/v2/plugin/accounts"),
        headers: vec![(
            "Authorization".to_string(),
            format!("Bearer {access_token}"),
        )],
        body: String::new(),
    }
}

/// 生产 HTTP 适配器：直连客户端（30s 超时；登录链与签到链同源域）。
pub struct ReqwestWorkBuddyLoginHttp {
    client: reqwest::blocking::Client,
}

impl ReqwestWorkBuddyLoginHttp {
    pub fn new() -> Self {
        Self {
            client: reqwest::blocking::Client::builder()
                .timeout(Duration::from_secs(30))
                .build()
                .expect("reqwest client"),
        }
    }
}

impl Default for ReqwestWorkBuddyLoginHttp {
    fn default() -> Self {
        Self::new()
    }
}

impl WorkBuddyLoginHttp for ReqwestWorkBuddyLoginHttp {
    fn send(
        &self,
        request: WbLoginHttpRequest,
    ) -> Result<WbLoginHttpResponse, WorkBuddyLoginError> {
        let method = reqwest::Method::from_bytes(request.method.as_bytes())
            .map_err(|_| WorkBuddyLoginError::Protocol)?;
        let mut builder = self.client.request(method, &request.url);
        for (name, value) in &request.headers {
            builder = builder.header(name, value);
        }
        let response = builder
            .body(request.body)
            .send()
            .map_err(|_| WorkBuddyLoginError::Network)?;
        let status = response.status().as_u16();
        let body = response
            .text()
            .map_err(|_| WorkBuddyLoginError::Network)?;
        Ok(WbLoginHttpResponse { status, body })
    }
}

// ============================================================================
// 信封解析（纯函数，离线单测固化）
// ============================================================================

/// 官方响应信封（`{code, msg, data}`）。
#[derive(Debug, Clone, Deserialize)]
struct WbEnvelope {
    code: Option<i64>,
    #[serde(default)]
    msg: Option<String>,
    #[serde(default)]
    data: Option<serde_json::Value>,
}

/// token 轮询单次响应分类（纯函数）。
#[derive(Debug, Clone, PartialEq)]
pub enum TokenPollOutcome {
    /// `code = 11217`：用户尚未在授权页完成，继续轮询。
    Pending,
    /// `data.accessToken` 非空：取得凭据（`hasUsableAuthToken` 口径）。
    Success(WorkBuddyAuthTokens),
    /// 其他业务码：终态失败（含 state 过期/被撤销——不再轮询）。
    Rejected { code: i64, msg: String },
    /// HTTP 层失败（网络抖动口径：计入总预算继续重试）。
    HttpFailed(u16),
}

/// 登录响应中的 auth 对象（字段名与 `.info` auth 块一致，探查报告 §3）。
#[derive(Debug, Clone, PartialEq, Default)]
pub struct WorkBuddyAuthTokens {
    pub access_token: String,
    pub refresh_token: String,
    /// access 有效期（秒；`expiresIn`）。
    pub expires_in_secs: Option<u64>,
    /// refresh 有效期（秒；`refreshExpiresIn`）。
    pub refresh_expires_in_secs: Option<u64>,
    pub token_type: String,
    pub scope: String,
    pub domain: String,
    pub session_state: String,
}

/// token 轮询响应分类（纯函数；status 与 body 分离传入）。
pub fn classify_token_poll(http_status: u16, body: &str) -> TokenPollOutcome {
    if !(200..300).contains(&http_status) {
        return TokenPollOutcome::HttpFailed(http_status);
    }
    let Ok(envelope) = serde_json::from_str::<WbEnvelope>(body) else {
        // 非 JSON 信封：按 HTTP 失败口径继续重试（网关偶发非 JSON 页面）。
        return TokenPollOutcome::HttpFailed(http_status);
    };
    match envelope.code {
        Some(WB_CODE_TOKEN_PENDING) => TokenPollOutcome::Pending,
        Some(code) if code != 0 => TokenPollOutcome::Rejected {
            code,
            msg: envelope.msg.unwrap_or_default(),
        },
        _ => {
            let Some(data) = envelope.data else {
                return TokenPollOutcome::Rejected {
                    code: -1,
                    msg: "响应缺少 data".into(),
                };
            };
            let access_token = data
                .get("accessToken")
                .and_then(|value| value.as_str())
                .unwrap_or("")
                .to_string();
            if access_token.is_empty() {
                // code=0 但无可用 token：形态异常按终态拒绝（不空转）。
                return TokenPollOutcome::Rejected {
                    code: -1,
                    msg: "accessToken 缺失".into(),
                };
            }
            TokenPollOutcome::Success(WorkBuddyAuthTokens {
                access_token,
                refresh_token: data
                    .get("refreshToken")
                    .and_then(|value| value.as_str())
                    .unwrap_or("")
                    .to_string(),
                expires_in_secs: data.get("expiresIn").and_then(|value| value.as_u64()),
                refresh_expires_in_secs: data
                    .get("refreshExpiresIn")
                    .and_then(|value| value.as_u64()),
                token_type: data
                    .get("tokenType")
                    .and_then(|value| value.as_str())
                    .unwrap_or("Bearer")
                    .to_string(),
                scope: data
                    .get("scope")
                    .and_then(|value| value.as_str())
                    .unwrap_or("")
                    .to_string(),
                domain: data
                    .get("domain")
                    .and_then(|value| value.as_str())
                    .unwrap_or("")
                    .to_string(),
                session_state: data
                    .get("sessionState")
                    .and_then(|value| value.as_str())
                    .unwrap_or("")
                    .to_string(),
            })
        }
    }
}

/// login/account 响应分类（纯函数）：pending（12151）/ 账号 / 拒绝。
#[derive(Debug, Clone, PartialEq)]
pub enum AccountPollOutcome {
    Pending,
    Account(WorkBuddyLoginAccount),
    Rejected { code: i64, msg: String },
    HttpFailed(u16),
}

/// 登录补齐的账号身份（login/account data 节选；宽松解析）。
#[derive(Debug, Clone, PartialEq, Default)]
pub struct WorkBuddyLoginAccount {
    /// 账号 uid（本地 UUID；账号身份与容器 profile_id 来源）。
    pub uid: String,
    /// 展示名（nickname；可空不阻塞）。
    pub screen_name: String,
}

/// login/account 响应分类（纯函数）。
pub fn classify_account_poll(http_status: u16, body: &str) -> AccountPollOutcome {
    if http_status == 401 || http_status == 403 {
        // Bearer 临时不可用（轮询竞态窗口）：调用方按官方语义重试 1 次。
        return AccountPollOutcome::HttpFailed(http_status);
    }
    if !(200..300).contains(&http_status) {
        return AccountPollOutcome::HttpFailed(http_status);
    }
    let Ok(envelope) = serde_json::from_str::<WbEnvelope>(body) else {
        return AccountPollOutcome::HttpFailed(http_status);
    };
    match envelope.code {
        Some(WB_CODE_ACCOUNT_PENDING) => AccountPollOutcome::Pending,
        Some(code) if code != 0 => AccountPollOutcome::Rejected {
            code,
            msg: envelope.msg.unwrap_or_default(),
        },
        _ => {
            let Some(data) = envelope.data else {
                return AccountPollOutcome::Rejected {
                    code: -1,
                    msg: "响应缺少 data".into(),
                };
            };
            // 宽松解析：uid 兼容 uid/id 双字段名，昵称兼容 nickname/nickName。
            let uid = data
                .get("uid")
                .or_else(|| data.get("id"))
                .and_then(|value| value.as_str())
                .unwrap_or("")
                .to_string();
            let screen_name = data
                .get("nickname")
                .or_else(|| data.get("nickName"))
                .and_then(|value| value.as_str())
                .unwrap_or("")
                .to_string();
            AccountPollOutcome::Account(WorkBuddyLoginAccount { uid, screen_name })
        }
    }
}

/// accounts 快照账号条目（P19.2②：含服务端**预脱敏**手机号——真机 200
/// 实测字段名 `phoneNumber`、值形态 `133****1304`；asar 无客户端脱敏
/// 逻辑、`maskedPhone` 全量唯一命中在 AgentMail 邮箱域与账号无关）。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct WorkBuddySnapshotAccount {
    pub uid: String,
    /// 脱敏手机号（服务端下发即星号形态，工具直显；缺省空串）。
    pub masked_phone: String,
}

/// accounts 快照解析（纯函数；兼容两形态：data 为账号数组 / data.accounts
/// 数组——2026-10-21 只读探查真机 200 为后者，旧实现只认前者属失修；
/// 宽松容错：解析失败返回空清单）。
pub fn parse_snapshot_accounts(body: &str) -> Vec<WorkBuddySnapshotAccount> {
    let Ok(envelope) = serde_json::from_str::<WbEnvelope>(body) else {
        return Vec::new();
    };
    let Some(data) = envelope.data else {
        return Vec::new();
    };
    let accounts = data.get("accounts").cloned().unwrap_or(data);
    let Some(accounts) = accounts.as_array() else {
        return Vec::new();
    };
    accounts
        .iter()
        .filter_map(|account| {
            let uid = account
                .get("uid")
                .or_else(|| account.get("id"))
                .and_then(|value| value.as_str())
                .map(str::to_string)?;
            let masked_phone = account
                .get("phoneNumber")
                .and_then(|value| value.as_str())
                .unwrap_or_default()
                .to_string();
            Some(WorkBuddySnapshotAccount { uid, masked_phone })
        })
        .collect()
}

/// accounts 快照解析（纯函数；data 为账号数组，取 uid 清单；宽松容错）。
pub fn parse_account_snapshot(body: &str) -> Vec<String> {
    parse_snapshot_accounts(body)
        .into_iter()
        .map(|account| account.uid)
        .collect()
}

// ============================================================================
// 编排实现：WorkBuddyLoginProtocol（OAuthLoginProtocol 的 WB 实现）
// ============================================================================

/// ③ 凭据获取产物：auth 对 + 账号身份 + 快照（明文 token 只在此结构内存
/// 流转，不进错误/回执/日志）。
pub struct WorkBuddyLoginCredentials {
    pub auth: WorkBuddyAuthTokens,
    pub account: Option<WorkBuddyLoginAccount>,
    /// accounts 快照账号条目（多账号盘点 + 脱敏手机号；best-effort，可空）。
    pub snapshot_accounts: Vec<WorkBuddySnapshotAccount>,
}

/// ④ 首登验收回执（只含非敏感字段，与 Trae 登录回执同语义）。
pub struct WorkBuddyLoginReceipt {
    pub profile_id: String,
    pub account_id: String,
    pub screen_name: String,
    /// 验收结果：passed / failed（失败时凭据已入池、账号已注册——对齐
    /// ADR-0027「失败不回滚」语义；WB 侧失败点 = 设备标识缺失等）。
    pub acceptance: LoginAcceptance,
}

/// WorkBuddy 登录编排协议实现（state 轮询流）。
///
/// 持有全部产品特定状态；HTTP seam 与轮询节拍可注入（离线单测零等待、
/// 零真实网络）。
pub struct WorkBuddyLoginProtocol {
    http: Box<dyn WorkBuddyLoginHttp>,
    /// workbuddy-login 容器根（首登入池落盘位置）。
    login_root: PathBuf,
    /// 机器级设备标识（`~/.workbuddy/device-id`；空 = 读取失败，验收拒绝）。
    device_id: String,
    /// 认证 API origin（默认 [`WB_AUTH_API_ORIGIN`]；测试可指向假端点）。
    origin: String,
    /// 轮询间隔（生产 1s）。
    poll_interval: Duration,
    /// 轮询预算次数（生产 = 300 = 5min / 1s；测试注入小值）。
    max_polls: u32,
    /// 轮询间睡眠（生产真实睡眠；测试 no-op）。
    sleep: Arc<dyn Fn(Duration) + Send + Sync>,
}

impl WorkBuddyLoginProtocol {
    /// 生产构造：从本机读设备标识（客户端已安装时 `~/.workbuddy/device-id`
    /// 必在；读取失败记空——首登验收以稳定错误码拒绝，不猜设备身份）。
    pub fn from_machine(login_root: PathBuf) -> Self {
        let device_id = load_device_id().unwrap_or_default();
        Self::new(
            login_root,
            device_id,
            Box::new(ReqwestWorkBuddyLoginHttp::new()),
        )
    }

    /// 通用构造（生产参数默认；测试可再调 `with_poll_params`）。
    pub fn new(login_root: PathBuf, device_id: String, http: Box<dyn WorkBuddyLoginHttp>) -> Self {
        Self {
            http,
            login_root,
            device_id,
            origin: WB_AUTH_API_ORIGIN.to_string(),
            poll_interval: WB_POLL_INTERVAL,
            max_polls: (WB_POLL_TIMEOUT.as_secs() / WB_POLL_INTERVAL.as_secs()) as u32,
            sleep: Arc::new(std::thread::sleep),
        }
    }

    /// 测试注入：origin / 轮询间隔 / 预算次数 / 睡眠实现。
    pub fn with_poll_params(
        mut self,
        origin: String,
        poll_interval: Duration,
        max_polls: u32,
        sleep: Arc<dyn Fn(Duration) + Send + Sync>,
    ) -> Self {
        self.origin = origin;
        self.poll_interval = poll_interval;
        self.max_polls = max_polls;
        self.sleep = sleep;
        self
    }

    /// ① authorize 获取：POST auth/state 下发 {authUrl, state}。
    fn create_state(&self) -> Result<(String, String), LoginFlowError> {
        let response = self
            .http
            .send(build_state_request(&self.origin))
            .map_err(|error| LoginFlowError::StateCreation(error.code()))?;
        if !(200..300).contains(&response.status) {
            return Err(LoginFlowError::StateCreation(format!(
                "workbuddy_login_http_{}",
                response.status
            )));
        }
        let envelope: WbEnvelope = serde_json::from_str(&response.body)
            .map_err(|_| LoginFlowError::StateCreation("workbuddy_login_protocol_invalid".into()))?;
        if envelope.code.unwrap_or(-1) != 0 {
            return Err(LoginFlowError::StateCreation(format!(
                "workbuddy_login_state_rejected:{}",
                envelope.code.unwrap_or(-1)
            )));
        }
        let data = envelope
            .data
            .ok_or_else(|| LoginFlowError::StateCreation("workbuddy_login_protocol_invalid".into()))?;
        let auth_url = data
            .get("authUrl")
            .and_then(|value| value.as_str())
            .unwrap_or("")
            .to_string();
        let state = data
            .get("state")
            .and_then(|value| value.as_str())
            .unwrap_or("")
            .to_string();
        if auth_url.is_empty() || state.is_empty() {
            return Err(LoginFlowError::StateCreation(
                "workbuddy_login_state_incomplete".into(),
            ));
        }
        Ok((auth_url, state))
    }

    /// ④ login/account 补齐（401/403 重试 1 次；12151 pending 短等待重试）。
    fn fetch_account(&self, state: &str, access_token: &str) -> Option<WorkBuddyLoginAccount> {
        let build = || build_account_request(&self.origin, state, access_token);
        let mut outcome = self.send_and_classify_account(&build());
        // 官方 MAX_ACCOUNT_AUTH_RETRIES = 1：401/403 或 pending 重试一次。
        if matches!(outcome, AccountPollOutcome::HttpFailed(_) | AccountPollOutcome::Pending) {
            (self.sleep)(self.poll_interval);
            outcome = self.send_and_classify_account(&build());
        }
        match outcome {
            AccountPollOutcome::Account(account) => Some(account),
            _ => None,
        }
    }

    fn send_and_classify_account(&self, request: &WbLoginHttpRequest) -> AccountPollOutcome {
        match self.http.send(request.clone()) {
            Ok(response) => classify_account_poll(response.status, &response.body),
            Err(_) => AccountPollOutcome::HttpFailed(0),
        }
    }

    /// ⑤ accounts 快照（best-effort：失败不影响登录编排；P19.2② 一并
    /// 取服务端预脱敏手机号）。
    fn fetch_snapshot(&self, access_token: &str) -> Vec<WorkBuddySnapshotAccount> {
        match self.http.send(build_snapshot_request(&self.origin, access_token)) {
            Ok(response) => parse_snapshot_accounts(&response.body),
            Err(_) => Vec::new(),
        }
    }

    /// 轮询循环核心（登录取凭据；单测直接驱动，`wait_credentials` 委托）。
    fn poll_tokens_until_ready(
        &self,
        state: &str,
        control: &LoginAttemptControl,
        should_abort: &dyn Fn() -> bool,
    ) -> Result<WorkBuddyAuthTokens, LoginFlowError> {
        let request = build_token_poll_request(&self.origin, state);
        for _ in 0..self.max_polls {
            // 取消观测：控制原语（命令层 cancel）或命令层附加条件。
            if control.should_abort_waiting() || should_abort() {
                return Err(LoginFlowError::Cancelled);
            }
            match self
                .http
                .send(request.clone())
                .map_err(|error| LoginFlowError::CredentialFetch(error.code()))
            {
                Err(error) => return Err(error),
                Ok(response) => match classify_token_poll(response.status, &response.body) {
                    TokenPollOutcome::Pending => {
                        (self.sleep)(self.poll_interval);
                        continue;
                    }
                    TokenPollOutcome::HttpFailed(_) => {
                        // 网络抖动/网关瞬态：计入总预算继续轮询（不中断登录）。
                        (self.sleep)(self.poll_interval);
                        continue;
                    }
                    TokenPollOutcome::Rejected { code, msg } => {
                        // 终态业务拒绝（含 state 过期/被撤销）：立即失败。
                        return Err(LoginFlowError::CredentialFetch(format!(
                            "workbuddy_login_poll_rejected:{code}:{msg}"
                        )));
                    }
                    TokenPollOutcome::Success(tokens) => return Ok(tokens),
                },
            }
        }
        Err(LoginFlowError::CredentialFetch(
            "workbuddy_login_poll_timeout".into(),
        ))
    }
}

impl OAuthLoginProtocol for WorkBuddyLoginProtocol {
    type LoginState = String;
    type Credentials = WorkBuddyLoginCredentials;
    type Receipt = WorkBuddyLoginReceipt;

    /// ① authorize 获取：服务端下发 authUrl + state。
    fn create_login_state(
        &mut self,
        _profile_id: &str,
    ) -> Result<(String, String), LoginFlowError> {
        // profile_id 在 WB 流中无用（uid 由服务端会话决定，容器标识 =
        // wb-{uid}），保留参数仅为 trait 形状统一。
        self.create_state()
    }

    /// ③ 登录等待 + 凭据获取：1s 轮询 token → login/account → accounts 快照。
    fn wait_credentials(
        &self,
        state: String,
        control: &LoginAttemptControl,
        should_abort: &dyn Fn() -> bool,
    ) -> Result<WorkBuddyLoginCredentials, LoginFlowError> {
        let auth = self.poll_tokens_until_ready(&state, control, should_abort)?;
        let account = self.fetch_account(&state, &auth.access_token);
        let snapshot_accounts = self.fetch_snapshot(&auth.access_token);
        Ok(WorkBuddyLoginCredentials {
            auth,
            account,
            snapshot_accounts,
        })
    }

    /// ④ 首登验收：凭据校验 → 整代次容器入池。
    ///
    /// 失败不回滚（对齐 ADR-0027）：token 已取得即登录事实成立，验收只决定
    /// 回执的 acceptance 状态与错误码；稳定错误码经 [`LoginFlowError::code`]
    /// 透出，前端 safeUiError 映射文案。
    fn first_login_acceptance(
        &self,
        credentials: WorkBuddyLoginCredentials,
        now_unix_seconds: u64,
    ) -> Result<WorkBuddyLoginReceipt, LoginFlowError> {
        let WorkBuddyLoginCredentials {
            auth,
            account,
            snapshot_accounts,
        } = credentials;
        // 账号身份缺失 = 无法确定容器归属，验收失败（凭据不入池）。
        let Some(account) = account else {
            return Err(LoginFlowError::Acceptance(
                "workbuddy_login_account_missing".into(),
            ));
        };
        if account.uid.trim().is_empty() {
            return Err(LoginFlowError::Acceptance(
                "workbuddy_login_account_missing".into(),
            ));
        }
        if self.device_id.trim().is_empty() {
            // 设备标识不可读：不猜机器身份（容器绑定 `~/.workbuddy/device-id`）。
            return Err(LoginFlowError::Acceptance(
                "workbuddy_login_device_id_missing".into(),
            ));
        }
        // 到期换算：expiresIn（秒）+ 当前时刻；缺失记 0（未知，不影响入池）。
        let access_expires_at = auth
            .expires_in_secs
            .map(|seconds| now_unix_seconds.saturating_add(seconds))
            .unwrap_or(0);
        let refresh_expires_at = auth
            .refresh_expires_in_secs
            .map(|seconds| now_unix_seconds.saturating_add(seconds))
            .unwrap_or(0);
        // domain 缺省回退 API origin authority（会话 auth.domain 惯例）。
        let domain = if auth.domain.trim().is_empty() {
            WB_AUTH_API_ORIGIN
                .trim_start_matches("https://")
                .to_string()
        } else {
            auth.domain.clone()
        };
        let record = WorkBuddyLoginRecord {
            profile_id: WorkBuddyLoginRecord::profile_id_for(&account.uid),
            account_id: account.uid.clone(),
            screen_name: account.screen_name,
            // P19.2② 服务端预脱敏手机号（accounts 快照按 uid 匹配；
            // best-effort——快照缺失/失败为空串，不阻塞入池）。
            masked_mobile: snapshot_accounts
                .iter()
                .find(|snapshot| snapshot.uid == account.uid)
                .map(|snapshot| snapshot.masked_phone.clone())
                .unwrap_or_default(),
            access_token: auth.access_token,
            refresh_token: auth.refresh_token,
            access_token_expires_at_unix_seconds: access_expires_at,
            refresh_token_expires_at_unix_seconds: refresh_expires_at,
            domain,
            session_state: auth.session_state,
            device_id: self.device_id.clone(),
            minted_at_unix_seconds: now_unix_seconds,
            last_updated_at_unix_seconds: now_unix_seconds,
        };
        // 整代次入池（tmp + rename + 写后复核由 store 保证）；同 uid 重复
        // 登录 = 整代次原子替换为最新（与既有捕获路线幂等语义一致）。
        let store = WorkBuddyLoginStore::new(&self.login_root);
        let saved = store.save(&record);
        let acceptance = match saved {
            Ok(()) => LoginAcceptance {
                passed: true,
                error_code: None,
            },
            Err(_) => LoginAcceptance {
                passed: false,
                error_code: Some("workbuddy_login_store_failed"),
            },
        };
        Ok(WorkBuddyLoginReceipt {
            profile_id: record.profile_id,
            account_id: record.account_id,
            screen_name: record.screen_name,
            acceptance,
        })
    }
}

/// 系统默认浏览器打开授权页（命令层调用；cfg(windows)——本工具运行面）。
/// 打开失败不中断（调用方忽略结果；URL 已返回前端可手动粘贴）。
#[cfg(windows)]
pub fn open_auth_url_in_system_browser(url: &str) {
    let _ = std::process::Command::new("rundll32")
        .args(["url.dll,FileProtocolHandler", url])
        .spawn();
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use tempfile::tempdir;

    /// 可脚本化的假 HTTP：按序返回响应并记录收到的请求（共享日志供断言）。
    struct FakeHttp {
        responses: Mutex<Vec<Result<WbLoginHttpResponse, WorkBuddyLoginError>>>,
        requests: Arc<Mutex<Vec<WbLoginHttpRequest>>>,
    }

    impl FakeHttp {
        /// responses 末位先出（pop）——调用方按发出顺序逆序铺响应；
        /// 耗尽后兜底返回 pending（轮询类测试不必铺满预算次数）。
        fn new(
            responses: Vec<Result<WbLoginHttpResponse, WorkBuddyLoginError>>,
        ) -> (Self, Arc<Mutex<Vec<WbLoginHttpRequest>>>) {
            let requests = Arc::new(Mutex::new(Vec::new()));
            (
                Self {
                    responses: Mutex::new(responses),
                    requests: Arc::clone(&requests),
                },
                requests,
            )
        }
    }

    impl WorkBuddyLoginHttp for FakeHttp {
        fn send(
            &self,
            request: WbLoginHttpRequest,
        ) -> Result<WbLoginHttpResponse, WorkBuddyLoginError> {
            self.requests.lock().unwrap().push(request);
            match self.responses.lock().unwrap().pop() {
                Some(result) => result,
                None => Ok(WbLoginHttpResponse {
                    status: 200,
                    body: envelope_json(WB_CODE_TOKEN_PENDING, "null"),
                }),
            }
        }
    }

    fn envelope_json(code: i64, data: &str) -> String {
        format!(r#"{{"code":{code},"msg":"","data":{data}}}"#)
    }

    fn success_token_body() -> String {
        r#"{"code":0,"msg":"","data":{"accessToken":"wb-access-jwt","refreshToken":"wb-refresh-jwt","expiresIn":1800,"refreshExpiresIn":86400,"tokenType":"Bearer","scope":"openid","domain":"www.workbuddy.cn","sessionState":"sess-1"}}"#.to_string()
    }

    fn pending_body() -> String {
        envelope_json(WB_CODE_TOKEN_PENDING, "null")
    }

    fn no_sleep() -> Arc<dyn Fn(Duration) + Send + Sync> {
        Arc::new(|_| {})
    }

    fn test_protocol(http: FakeHttp, max_polls: u32) -> WorkBuddyLoginProtocol {
        let root = tempdir().expect("tempdir");
        WorkBuddyLoginProtocol::new(
            root.path().to_path_buf(),
            "2db49f20-2b3f-4761-ab53-9d213f361a5b".to_string(),
            Box::new(http),
        )
        .with_poll_params(
            WB_AUTH_API_ORIGIN.to_string(),
            Duration::from_millis(1),
            max_polls,
            no_sleep(),
        )
    }
    // ---- 豁免头与请求形状 ----

    #[test]
    fn exemption_headers_match_surveyed_shape() {
        let headers = wb_exemption_headers("www.workbuddy.cn");
        assert_eq!(
            headers,
            vec![
                ("X-Domain".to_string(), "www.workbuddy.cn".to_string()),
                ("X-No-Authorization".to_string(), "true".to_string()),
                ("X-No-User-Id".to_string(), "true".to_string()),
                ("X-No-Enterprise-Id".to_string(), "true".to_string()),
                ("X-No-Department-Info".to_string(), "true".to_string()),
            ]
        );
    }

    #[test]
    fn state_request_shape_matches_survey() {
        let request = build_state_request("https://www.workbuddy.cn");
        assert_eq!(request.method, "POST");
        assert_eq!(
            request.url,
            "https://www.workbuddy.cn/v2/plugin/auth/state?platform=workbuddy"
        );
        assert!(request.body.is_empty(), "auth/state 空 body");
        let headers = request.headers;
        assert!(headers.contains(&("X-Domain".to_string(), "www.workbuddy.cn".to_string())));
        assert!(headers.contains(&("X-No-Authorization".to_string(), "true".to_string())));
        assert!(headers.contains(&("X-No-Department-Info".to_string(), "true".to_string())));
    }

    #[test]
    fn token_poll_request_is_unauthenticated() {
        // 轮询端点免认证：带豁免头、无 Authorization（探查报告 §3）。
        let request = build_token_poll_request("https://www.workbuddy.cn", "state-1");
        assert_eq!(
            request.url,
            "https://www.workbuddy.cn/v2/plugin/auth/token?state=state-1"
        );
        assert!(!request
            .headers
            .iter()
            .any(|(name, _)| name == "Authorization"));
    }

    #[test]
    fn account_and_snapshot_requests_carry_bearer() {
        let account = build_account_request("https://www.workbuddy.cn", "s1", "tok");
        assert_eq!(
            account.url,
            "https://www.workbuddy.cn/v2/plugin/login/account?state=s1"
        );
        assert!(account
            .headers
            .contains(&("Authorization".to_string(), "Bearer tok".to_string())));
        let snapshot = build_snapshot_request("https://www.workbuddy.cn", "tok");
        assert_eq!(snapshot.url, "https://www.workbuddy.cn/v2/plugin/accounts");
        assert!(snapshot
            .headers
            .contains(&("Authorization".to_string(), "Bearer tok".to_string())));
    }

    // ---- 信封分类 ----

    #[test]
    fn classify_token_poll_three_states() {
        // pending：11217。
        assert_eq!(classify_token_poll(200, &pending_body()), TokenPollOutcome::Pending);
        // success：accessToken 非空（hasUsableAuthToken 口径）。
        let success = classify_token_poll(200, &success_token_body());
        let TokenPollOutcome::Success(tokens) = success else {
            panic!("应分类为成功");
        };
        assert_eq!(tokens.access_token, "wb-access-jwt");
        assert_eq!(tokens.refresh_token, "wb-refresh-jwt");
        assert_eq!(tokens.expires_in_secs, Some(1800));
        assert_eq!(tokens.domain, "www.workbuddy.cn");
        // 终态拒绝：非 0/11217 业务码（含 state 过期类）。
        assert_eq!(
            classify_token_poll(200, &envelope_json(11218, "null")),
            TokenPollOutcome::Rejected {
                code: 11218,
                msg: String::new()
            }
        );
        // HTTP 层失败（瞬态口径：继续轮询）。
        assert_eq!(
            classify_token_poll(502, "bad gateway"),
            TokenPollOutcome::HttpFailed(502)
        );
    }

    #[test]
    fn classify_token_poll_code_zero_without_token_is_rejected() {
        // code=0 但 accessToken 缺失：形态异常按终态拒绝（不空转）。
        assert_eq!(
            classify_token_poll(200, &envelope_json(0, r#"{"foo":1}"#)),
            TokenPollOutcome::Rejected {
                code: -1,
                msg: "accessToken 缺失".into()
            }
        );
    }

    #[test]
    fn classify_account_poll_pending_account_and_rejection() {
        assert_eq!(
            classify_account_poll(200, &envelope_json(WB_CODE_ACCOUNT_PENDING, "null")),
            AccountPollOutcome::Pending
        );
        let account = classify_account_poll(
            200,
            r#"{"code":0,"msg":"","data":{"uid":"uid-9","nickname":"梦梦","uin":"330126449009"}}"#,
        );
        assert_eq!(
            account,
            AccountPollOutcome::Account(WorkBuddyLoginAccount {
                uid: "uid-9".into(),
                screen_name: "梦梦".into(),
            })
        );
        assert!(matches!(
            classify_account_poll(401, "{}"),
            AccountPollOutcome::HttpFailed(401)
        ));
        assert!(matches!(
            classify_account_poll(200, &envelope_json(99999, "null")),
            AccountPollOutcome::Rejected { code: 99999, .. }
        ));
    }

    #[test]
    fn snapshot_parses_uid_list_leniently() {
        let body = r#"{"code":0,"msg":"","data":[{"uid":"uid-1"},{"uid":"uid-2"},{"nickname":"无uid"}]}"#;
        assert_eq!(parse_account_snapshot(body), vec!["uid-1".to_string(), "uid-2".to_string()]);
        assert!(parse_account_snapshot("not-json").is_empty());
    }

    /// P19.2② 快照双形态 + 脱敏手机号：真机 200 实测 data 为
    /// `{accounts:[…]}` 字典（旧实现只认 data 数组属失修——探查定论）；
    /// phoneNumber 为服务端预脱敏形态，原样提取。
    #[test]
    fn snapshot_accounts_parse_both_shapes_and_masked_phone() {
        // 形态一：data.accounts 字典（真机实测形态）。
        let wrapped = r#"{"code":0,"msg":"","data":{"accounts":[
            {"uid":"uid-1","phoneNumber":"133****1304","pluginEnabled":true},
            {"uid":"uid-2","nickname":"梦梦"}
        ]}}"#;
        let entries = parse_snapshot_accounts(wrapped);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].uid, "uid-1");
        assert_eq!(entries[0].masked_phone, "133****1304");
        assert_eq!(entries[1].uid, "uid-2");
        assert_eq!(entries[1].masked_phone, "", "无 phoneNumber 字段为空串");
        // 形态二：data 数组（旧形态兼容）。
        let flat = r#"{"code":0,"data":[{"uid":"uid-1","phoneNumber":"138****5678"}]}"#;
        let entries = parse_snapshot_accounts(flat);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].masked_phone, "138****5678");
        // 宽松容错：损坏输入空清单。
        assert!(parse_snapshot_accounts("not-json").is_empty());
        assert!(parse_snapshot_accounts(r#"{"code":0,"data":{"other":1}}"#).is_empty());
    }

    // ---- 轮询状态机（注入式三态）----

    #[test]
    fn poll_state_machine_pending_then_success() {
        // pending ×2 → success：非终态继续轮询，终态即返回。
        let (http, _requests) = FakeHttp::new(vec![
            Ok(WbLoginHttpResponse {
                status: 200,
                body: success_token_body(),
            }),
            Ok(WbLoginHttpResponse {
                status: 200,
                body: pending_body(),
            }),
            Ok(WbLoginHttpResponse {
                status: 200,
                body: pending_body(),
            }),
        ]);
        let protocol = test_protocol(http, 10);
        let control = LoginAttemptControl::new();
        let tokens = protocol
            .poll_tokens_until_ready("state-1", &control, &|| false)
            .expect("poll success");
        assert_eq!(tokens.access_token, "wb-access-jwt");
    }

    #[test]
    fn poll_state_machine_times_out_after_budget() {
        // 恒 pending：预算耗尽后报稳定超时码（不再空转）。
        let (http, requests) = FakeHttp::new(Vec::new());
        let protocol = test_protocol(http, 3);
        let control = LoginAttemptControl::new();
        let error = protocol
            .poll_tokens_until_ready("state-1", &control, &|| false)
            .err()
            .expect("timeout");
        assert_eq!(
            error,
            LoginFlowError::CredentialFetch("workbuddy_login_poll_timeout".into())
        );
        assert_eq!(requests.lock().unwrap().len(), 3, "恰好轮询预算次数");
    }

    #[test]
    fn poll_state_machine_cancelled_before_first_poll() {
        let (http, requests) = FakeHttp::new(Vec::new());
        let protocol = test_protocol(http, 3);
        let control = LoginAttemptControl::new();
        let error = protocol
            .poll_tokens_until_ready("state-1", &control, &|| true)
            .err()
            .expect("cancelled");
        assert_eq!(error, LoginFlowError::Cancelled);
        assert_eq!(requests.lock().unwrap().len(), 0, "取消先于任何轮询请求");
    }

    #[test]
    fn poll_state_machine_state_expired_rejects_immediately() {
        // state 过期（终态业务码）：单次响应即失败，不再轮询。
        let (http, requests) = FakeHttp::new(vec![Ok(WbLoginHttpResponse {
            status: 200,
            body: envelope_json(11218, "null"),
        })]);
        let protocol = test_protocol(http, 10);
        let control = LoginAttemptControl::new();
        let error = protocol
            .poll_tokens_until_ready("expired", &control, &|| false)
            .err()
            .expect("rejected");
        assert!(matches!(
            error,
            LoginFlowError::CredentialFetch(ref detail)
                if detail.starts_with("workbuddy_login_poll_rejected:11218")
        ));
        assert_eq!(requests.lock().unwrap().len(), 1, "终态拒绝后停止轮询");
    }

    // ---- ① state 创建 ----

    #[test]
    fn create_state_parses_auth_url_and_state() {
        let (http, _requests) = FakeHttp::new(vec![Ok(WbLoginHttpResponse {
            status: 200,
            body: r#"{"code":0,"msg":"","data":{"authUrl":"https://www.workbuddy.cn/login?x=1","state":"9805c43c-eab1-4ba5-87ec-1e91815f4b34"}}"#.to_string(),
        })]);
        let protocol = test_protocol(http, 1);
        let (auth_url, state) = protocol.create_state().expect("state created");
        assert_eq!(auth_url, "https://www.workbuddy.cn/login?x=1");
        assert_eq!(state, "9805c43c-eab1-4ba5-87ec-1e91815f4b34");
    }

    #[test]
    fn create_state_rejects_incomplete_and_business_failure() {
        // 业务码非 0。
        let (http, _requests) = FakeHttp::new(vec![Ok(WbLoginHttpResponse {
            status: 200,
            body: envelope_json(10081, "null"),
        })]);
        let protocol = test_protocol(http, 1);
        assert!(matches!(
            protocol.create_state(),
            Err(LoginFlowError::StateCreation(detail))
                if detail.starts_with("workbuddy_login_state_rejected:10081")
        ));
        // data 不完整（缺 state）。
        let (http2, _requests2) = FakeHttp::new(vec![Ok(WbLoginHttpResponse {
            status: 200,
            body: r#"{"code":0,"msg":"","data":{"authUrl":"https://x"}}"#.to_string(),
        })]);
        let protocol2 = test_protocol(http2, 1);
        assert!(matches!(
            protocol2.create_state(),
            Err(LoginFlowError::StateCreation(detail))
                if detail == "workbuddy_login_state_incomplete"
        ));
    }

    // ---- ④ 首登验收与入池 ----

    fn ready_credentials() -> WorkBuddyLoginCredentials {
        WorkBuddyLoginCredentials {
            auth: WorkBuddyAuthTokens {
                access_token: "wb-access-jwt".into(),
                refresh_token: "wb-refresh-jwt".into(),
                expires_in_secs: Some(1800),
                refresh_expires_in_secs: Some(86_400),
                token_type: "Bearer".into(),
                scope: "openid".into(),
                domain: String::new(),
                session_state: "sess-1".into(),
            },
            account: Some(WorkBuddyLoginAccount {
                uid: "89b16d6b-0549-4e1e-8ced-918b5c8152ae".into(),
                screen_name: "梦梦".into(),
            }),
            snapshot_accounts: vec![WorkBuddySnapshotAccount {
                uid: "89b16d6b-0549-4e1e-8ced-918b5c8152ae".into(),
                masked_phone: "133****1304".into(),
            }],
        }
    }

    #[test]
    fn acceptance_builds_record_and_saves_to_pool() {
        let root = tempdir().expect("tempdir");
        let (http, _requests) = FakeHttp::new(Vec::new());
        let protocol = WorkBuddyLoginProtocol::new(
            root.path().to_path_buf(),
            "2db49f20-2b3f-4761-ab53-9d213f361a5b".to_string(),
            Box::new(http),
        );
        let receipt = protocol
            .first_login_acceptance(ready_credentials(), 1_800_000_000)
            .expect("accepted");
        assert_eq!(
            receipt.profile_id,
            "wb-89b16d6b-0549-4e1e-8ced-918b5c8152ae"
        );
        assert_eq!(receipt.account_id, "89b16d6b-0549-4e1e-8ced-918b5c8152ae");
        assert_eq!(receipt.screen_name, "梦梦");
        assert!(receipt.acceptance.passed);
        assert!(receipt.acceptance.error_code.is_none());
        // 容器可回读；到期换算 = now + expiresIn。
        let store = WorkBuddyLoginStore::new(root.path());
        let loaded = store
            .load(&receipt.profile_id, receipt.account_id.as_str())
            .expect("load back");
        assert_eq!(loaded.access_token, "wb-access-jwt");
        assert_eq!(
            loaded.access_token_expires_at_unix_seconds,
            1_800_000_000 + 1800
        );
        assert_eq!(
            loaded.refresh_token_expires_at_unix_seconds,
            1_800_000_000 + 86_400
        );
        // domain 缺省回退 API origin authority。
        assert_eq!(loaded.domain, "www.workbuddy.cn");
    }

    #[test]
    fn acceptance_fails_without_account_or_device_id() {
        let root = tempdir().expect("tempdir");
        let (http, _requests) = FakeHttp::new(Vec::new());
        let protocol = WorkBuddyLoginProtocol::new(
            root.path().to_path_buf(),
            String::new(),
            Box::new(http),
        );
        // 设备标识缺失 → 验收拒绝（不猜机器身份）。
        let error = protocol
            .first_login_acceptance(ready_credentials(), 0)
            .err()
            .expect("device missing");
        assert_eq!(
            error,
            LoginFlowError::Acceptance("workbuddy_login_device_id_missing".into())
        );
        // 账号身份缺失 → 验收拒绝（容器归属不可定）。
        let (http2, _requests2) = FakeHttp::new(Vec::new());
        let protocol2 = WorkBuddyLoginProtocol::new(
            root.path().to_path_buf(),
            "device-1".to_string(),
            Box::new(http2),
        );
        let mut no_account = ready_credentials();
        no_account.account = None;
        let error2 = protocol2
            .first_login_acceptance(no_account, 0)
            .err()
            .expect("account missing");
        assert_eq!(
            error2,
            LoginFlowError::Acceptance("workbuddy_login_account_missing".into())
        );
    }

    // ---- 全编排（run_login_flow 驱动）----

    #[test]
    fn full_flow_via_run_login_flow_saves_record() {
        let (http, _requests) = FakeHttp::new(vec![
            // ⑤ accounts 快照（best-effort）。
            Ok(WbLoginHttpResponse {
                status: 200,
                body: r#"{"code":0,"msg":"","data":[{"uid":"89b16d6b-0549-4e1e-8ced-918b5c8152ae"}]}"#.to_string(),
            }),
            // ④ login/account。
            Ok(WbLoginHttpResponse {
                status: 200,
                body: r#"{"code":0,"msg":"","data":{"uid":"89b16d6b-0549-4e1e-8ced-918b5c8152ae","nickname":"梦梦"}}"#.to_string(),
            }),
            // ③ token 轮询：pending → success。
            Ok(WbLoginHttpResponse {
                status: 200,
                body: success_token_body(),
            }),
            Ok(WbLoginHttpResponse {
                status: 200,
                body: pending_body(),
            }),
            // ① auth/state。
            Ok(WbLoginHttpResponse {
                status: 200,
                body: r#"{"code":0,"msg":"","data":{"authUrl":"https://www.workbuddy.cn/login?x=1","state":"state-full"}}"#.to_string(),
            }),
        ]);
        let root = tempdir().expect("tempdir");
        let mut protocol = WorkBuddyLoginProtocol::new(
            root.path().to_path_buf(),
            "2db49f20-2b3f-4761-ab53-9d213f361a5b".to_string(),
            Box::new(http),
        )
        .with_poll_params(
            WB_AUTH_API_ORIGIN.to_string(),
            Duration::from_millis(1),
            5,
            no_sleep(),
        );
        let control = LoginAttemptControl::new();
        let receipt = crate::oauth_login_protocol::run_login_flow(
            &mut protocol,
            "wb-any",
            &control,
            &|_| {},
            &|| false,
            1_800_000_000,
        )
        .expect("flow success");
        assert!(receipt.acceptance.passed);
        assert_eq!(receipt.screen_name, "梦梦");
        let store = WorkBuddyLoginStore::new(root.path());
        assert!(store.exists(&receipt.profile_id));
    }
}
