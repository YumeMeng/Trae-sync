//! T87/MVP OAuth 登录流：PKCE + 系统浏览器授权 + 本地回调 + AuthCode 换凭证。
//!
//! 协议来源（09 号文档逆向 + 2026-08-22 按 Trae CN v3.3.74 main.js/product.json 复核）：
//! - 登录页 `https://www.trae.cn/authorization`（`bootConfig.consoleHost`；
//!   旧文档的 `work.trae.cn` 已下线返回 404），参数含 PKCE challenge 与
//!   `auth_callback_url=http://127.0.0.1:<port>/authorize`；
//! - 回调 query 的 `authCodeInfo` 为 URL 编码 JSON（含一次性 `AuthCode`）；
//! - `ExchangeToken` AuthCode 模式免签名换取 Token(84 天)/RefreshToken(880 天)；
//! - 账号 ID 从 JWT payload `data.id` 解出，`GetUserInfo` 补全脱敏资料。
//!
//! 每个账号绑定持久虚拟设备（随机 86 位 device_id + 随机 machine_id +
//! EC P-256 密钥对，ADR-0089），随 `ExchangeToken` 注册后与账号永久绑定；
//! 设备闸门与配额规则见 `checkin_http` 模块注释。
//!
//! 形态选择（2026-09-02）：SOLO 与 Work 为同一产品的改名，服务端按
//! client_id 通道分别计设备配额；Work CN 登录固定走 SOLO 通道
//!（en1oxy7wnw8j9n），该通道配额未被历史测试耗尽，且 SOLO 形态设备可直接
//! 首签，无需登录后再重铸。Trae CN 由独立 `TRAE/IDE_PC` 通道登录，边界见
//! ADR-0030。后续如通道可用性变化，按「能用的优先」原则切换，不绑定产品名。

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use openssl::rand::rand_bytes;
use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::account_registry::{AccountRecord, AccountRegistry};
use crate::checkin_credential::{
    generate_device_keypair, CheckinCredentialBundle, CheckinCredentialError,
    CheckinCredentialStore, CheckinProfileBinding, CheckinProfileOperationLease,
};
use crate::checkin_http::{
    exchange_token_by_auth_code, get_user_info_for_oauth_client, CheckinHttpError, OAuthClient,
    RealCheckinRenewalService, TokenGrant, UserInfoSummary,
};
use crate::operation_lease::FileOperationLock;

/// 登录页地址（Trae CN v3.3.74 `bootConfig.consoleHost` + `/authorization`）。
pub const LOGIN_HOST: &str = "https://www.trae.cn/authorization";
/// 回调等待上限（与客户端 TIMEOUT_MS 同量级）。
pub const CALLBACK_TIMEOUT_SECONDS: u64 = 300;

/// 产品登录适配；同一个 OAuth 回调协议下，Work 与 Trae CN 仍使用不同的
/// client、来源参数、版本和凭据池。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoginProduct {
    /// 现有 Work CN 账号中心，继续使用已验证的 SOLO 通道。
    WorkCn,
    /// 独立 Trae CN 账号中心，使用官方 TRAE 通道。
    TraeCn,
}

impl LoginProduct {
    fn oauth_client(self) -> OAuthClient {
        match self {
            Self::WorkCn => OAuthClient::Solo,
            Self::TraeCn => OAuthClient::TraeCn,
        }
    }

    fn auth_from(self) -> &'static str {
        match self {
            Self::WorkCn => "solo",
            Self::TraeCn => "trae",
        }
    }

    fn login_version(self) -> &'static str {
        match self {
            // 保留 Work 登录现有协议参数，避免改变已验证签到链路。
            Self::WorkCn => "1",
            Self::TraeCn => "1",
        }
    }
}

/// 登录流错误；不携带 Token、AuthCode 或回调正文。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoginError {
    /// 回调参数无效或登录页返回 error_code。
    InvalidCallback,
    /// AuthCode 换取凭证失败（网络/业务码/协议）。
    ExchangeFailed,
    /// 服务端设备数已达上限（业务码 20401/20408）：本地删档无法释放服务端配额，
    /// 登录被拒（2026-09-02 实测，反复重铸/多次登录会耗尽账号设备额度）。
    ExchangeDeviceLimit,
    /// JWT 无法解出账号 ID。
    InvalidToken,
    /// 凭据入库或注册表写入失败。
    Storage,
    /// 等待回调期间被外部中止（用户取消 / 登录浏览器已关闭）。
    /// P7-4：等待不再只能靠超时兜底，浏览器一关即可收尾。
    Cancelled,
}

/// OAuth attempt 的可观察阶段。
///
/// 取消只能在线程尚未进入发布阶段时赢得线性化点；进入发布阶段后，
/// 取消请求必须等待真实完成结果，不能把已经写入的结果伪装成已取消。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum LoginAttemptPhase {
    /// 等待浏览器回调，尚未开始换取凭据。
    AwaitingCallback = 0,
    /// 已经取得换取资格，正在执行 AuthCode ExchangeToken。
    Exchanging = 1,
    /// 已经取得发布资格，正在写入凭据与账号档案。
    Publishing = 2,
    /// 发布完成且登录返回成功回执。
    Completed = 3,
    /// 登录已结束并报告真实失败。
    Failed = 4,
    /// 取消在线性化点生效，后续不得换取或发布凭据。
    Cancelled = 5,
}

/// 登录 attempt 的取消结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoginCancelResult {
    /// 取消在等待回调或 ExchangeToken 阶段取得线性化点。
    Cancelled,
    /// attempt 已经取消。
    AlreadyCancelled,
    /// 发布阶段已经取得线性化点，取消来得太晚。
    TooLate,
    /// attempt 已完成或已失败，保留真实结果。
    AlreadyFinished,
}

/// 取消已赢得阶段竞争；`complete_login` 将其映射为既有 `LoginError::Cancelled`。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LoginAttemptCancelled;

/// 登录 attempt 的真实收尾结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoginAttemptResult {
    /// 凭据与账号档案发布完成，登录返回回执。
    Completed,
    /// 登录在发布前或发布后真实失败。
    Failed,
}

/// OAuth attempt 控制原语。
///
/// 所有跨线程竞争均通过同一个原子阶段完成线性化：
/// AwaitingCallback → Exchanging → Publishing → Completed/Failed；取消可以从
/// AwaitingCallback 或 Exchanging 进入 Cancelled。使用 `Arc` 只共享阶段，
/// 不共享 Token 或会话正文。
#[derive(Clone, Debug)]
pub struct LoginAttemptControl {
    phase: Arc<AtomicU8>,
}

impl Default for LoginAttemptControl {
    fn default() -> Self {
        Self::new()
    }
}

impl LoginAttemptControl {
    /// 创建一个尚未收到回调的新 attempt。
    pub fn new() -> Self {
        Self {
            phase: Arc::new(AtomicU8::new(LoginAttemptPhase::AwaitingCallback as u8)),
        }
    }

    /// 读取当前阶段，供取消命令和结果测试观察。
    pub fn phase(&self) -> LoginAttemptPhase {
        match self.phase.load(Ordering::Acquire) {
            value if value == LoginAttemptPhase::AwaitingCallback as u8 => {
                LoginAttemptPhase::AwaitingCallback
            }
            value if value == LoginAttemptPhase::Exchanging as u8 => LoginAttemptPhase::Exchanging,
            value if value == LoginAttemptPhase::Publishing as u8 => LoginAttemptPhase::Publishing,
            value if value == LoginAttemptPhase::Completed as u8 => LoginAttemptPhase::Completed,
            value if value == LoginAttemptPhase::Failed as u8 => LoginAttemptPhase::Failed,
            value if value == LoginAttemptPhase::Cancelled as u8 => LoginAttemptPhase::Cancelled,
            _ => LoginAttemptPhase::Failed,
        }
    }

    /// 等待浏览器回调时是否应中止；回调返回后不再读取此状态，
    /// 由 `begin_exchange` 与 `cancel` 的 CAS 决定先后。
    pub fn should_abort_waiting(&self) -> bool {
        self.phase() == LoginAttemptPhase::Cancelled
    }

    /// 取消等待中的 attempt，或取消正在 ExchangeToken 但尚未发布的 attempt。
    ///
    /// 与 `begin_exchange`、`begin_publish` 共享原子 CAS，因此调用结果就是
    /// 取消和登录阶段之间的确定性线性化结果。
    pub fn cancel(&self) -> LoginCancelResult {
        loop {
            let current = self.phase.load(Ordering::Acquire);
            match current {
                value if value == LoginAttemptPhase::AwaitingCallback as u8 => {
                    if self
                        .phase
                        .compare_exchange(
                            current,
                            LoginAttemptPhase::Cancelled as u8,
                            Ordering::AcqRel,
                            Ordering::Acquire,
                        )
                        .is_ok()
                    {
                        return LoginCancelResult::Cancelled;
                    }
                }
                value if value == LoginAttemptPhase::Exchanging as u8 => {
                    if self
                        .phase
                        .compare_exchange(
                            current,
                            LoginAttemptPhase::Cancelled as u8,
                            Ordering::AcqRel,
                            Ordering::Acquire,
                        )
                        .is_ok()
                    {
                        return LoginCancelResult::Cancelled;
                    }
                }
                value if value == LoginAttemptPhase::Publishing as u8 => {
                    return LoginCancelResult::TooLate;
                }
                value if value == LoginAttemptPhase::Cancelled as u8 => {
                    return LoginCancelResult::AlreadyCancelled;
                }
                _ => return LoginCancelResult::AlreadyFinished,
            }
        }
    }

    /// 取得 ExchangeToken 资格；取消先赢时不会启动网络换取。
    pub fn begin_exchange(&self) -> Result<(), LoginAttemptCancelled> {
        self.advance(
            LoginAttemptPhase::AwaitingCallback,
            LoginAttemptPhase::Exchanging,
        )
    }

    /// 取得凭据发布资格；取消在 ExchangeToken 期间先赢时不能进入发布。
    pub fn begin_publish(&self) -> Result<(), LoginAttemptCancelled> {
        self.advance(LoginAttemptPhase::Exchanging, LoginAttemptPhase::Publishing)
    }

    /// 结束 attempt，保留已发生的取消结果；发布后失败必须显式记录 Failed。
    pub fn finish(&self, result: LoginAttemptResult) {
        let next = match result {
            LoginAttemptResult::Completed => LoginAttemptPhase::Completed,
            LoginAttemptResult::Failed => LoginAttemptPhase::Failed,
        } as u8;
        let _ = self.phase.compare_exchange(
            LoginAttemptPhase::Publishing as u8,
            next,
            Ordering::AcqRel,
            Ordering::Acquire,
        );
        // 回调解析/ExchangeToken 失败也要结束 attempt；Cancelled 状态不能被
        // 失败收尾覆盖，否则取消回执会失去真实线性化结果。
        let _ = self.phase.compare_exchange(
            LoginAttemptPhase::Exchanging as u8,
            next,
            Ordering::AcqRel,
            Ordering::Acquire,
        );
        let _ = self.phase.compare_exchange(
            LoginAttemptPhase::AwaitingCallback as u8,
            next,
            Ordering::AcqRel,
            Ordering::Acquire,
        );
    }

    /// 用 CAS 推进阶段，避免以普通读写破坏取消竞态的线性化顺序。
    fn advance(
        &self,
        expected_phase: LoginAttemptPhase,
        next_phase: LoginAttemptPhase,
    ) -> Result<(), LoginAttemptCancelled> {
        let current = self.phase.load(Ordering::Acquire);
        if current != expected_phase as u8 {
            return Err(LoginAttemptCancelled);
        }
        match self.phase.compare_exchange(
            expected_phase as u8,
            next_phase as u8,
            Ordering::AcqRel,
            Ordering::Acquire,
        ) {
            Ok(_) => Ok(()),
            Err(_) => Err(LoginAttemptCancelled),
        }
    }
}

/// PKCE 密钥对；verifier 保存在后端内存直到回调完成。
pub struct PkcePair {
    pub code_verifier: String,
    pub code_challenge: String,
}

/// 生成 PKCE：48 随机字节 base64url（verifier）+ SHA256 base64url（challenge）。
pub fn generate_pkce_pair() -> Result<PkcePair, LoginError> {
    let mut bytes = [0u8; 48];
    rand_bytes(&mut bytes).map_err(|_| LoginError::ExchangeFailed)?;
    let code_verifier = URL_SAFE_NO_PAD.encode(bytes);
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(code_verifier.as_bytes()));
    Ok(PkcePair {
        code_verifier,
        code_challenge: challenge,
    })
}

/// 生成虚拟设备 ID：16 位数字，首位 1-9（与真实设备 ID 形态一致）。
pub fn generate_virtual_device_id() -> Result<String, LoginError> {
    let mut bytes = [0u8; 16];
    rand_bytes(&mut bytes).map_err(|_| LoginError::ExchangeFailed)?;
    let mut digits = Vec::with_capacity(16);
    // 首位取 8-9，避免前导 0 导致长度缩水。
    digits.push((b'1' + bytes[0] % 9) as char);
    for byte in &bytes[1..] {
        digits.push((b'0' + byte % 10) as char);
    }
    Ok(digits.into_iter().collect())
}

/// URL 查询参数百分号编码（RFC 3986 非保留字符除外）。
fn percent_encode(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                encoded.push(byte as char)
            }
            _ => encoded.push_str(&format!("%{byte:02X}")),
        }
    }
    encoded
}

/// 百分号解码（含 `+` 还原为空格的 query 语义）。
fn percent_decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'%' if index + 2 < bytes.len() => {
                let hex = &value[index + 1..index + 3];
                if let Ok(byte) = u8::from_str_radix(hex, 16) {
                    decoded.push(byte);
                    index += 3;
                } else {
                    decoded.push(bytes[index]);
                    index += 1;
                }
            }
            b'+' => {
                decoded.push(b' ');
                index += 1;
            }
            byte => {
                decoded.push(byte);
                index += 1;
            }
        }
    }
    String::from_utf8_lossy(&decoded).into_owned()
}

/// 从 query 字符串中取一次参数值（首个匹配）。
fn query_param<'a>(query: &'a str, key: &str) -> Option<String> {
    for pair in query.split('&') {
        let (name, value) = pair.split_once('=')?;
        if percent_decode(name) == key {
            return Some(percent_decode(value));
        }
    }
    None
}

/// 构造授权页登录 URL（SOLO 通道场景）。
///
/// `auth_from` 必须与登录通道一致（2026-09-02 官方 main.js 逆向实证：
/// `auth_from = isSolo ? "solo" : "trae"`，SOLO 还追加 `hide_saas_login=true`）。
/// 授权页按此参数决定 AuthCode 的通道绑定；与换取所用 ClientID 不一致时
/// ExchangeToken 报 20403 "Token device not match"（LY 账号实测）。
pub fn build_login_url(
    callback_url: &str,
    code_challenge: &str,
    login_trace_id: &str,
    device_id: &str,
    machine_id: &str,
) -> String {
    build_login_url_for(
        LoginProduct::WorkCn,
        callback_url,
        code_challenge,
        login_trace_id,
        device_id,
        machine_id,
    )
}

/// 按产品认证通道构造授权页 URL。
pub fn build_login_url_for(
    product: LoginProduct,
    callback_url: &str,
    code_challenge: &str,
    login_trace_id: &str,
    device_id: &str,
    machine_id: &str,
) -> String {
    let oauth_client = product.oauth_client();
    let params = [
        ("login_version", product.login_version().to_string()),
        ("auth_from", product.auth_from().to_string()),
        ("login_channel", "native_ide".to_string()),
        ("plugin_version", "local".to_string()),
        ("auth_type", "local".to_string()),
        ("client_id", oauth_client.client_id().to_string()),
        ("redirect", "0".to_string()),
        ("login_trace_id", login_trace_id.to_string()),
        ("auth_callback_url", callback_url.to_string()),
        ("machine_id", machine_id.to_string()),
        ("device_id", device_id.to_string()),
        ("x_device_id", device_id.to_string()),
        ("x_machine_id", machine_id.to_string()),
        ("x_device_brand", String::new()),
        ("x_device_type", "Windows".to_string()),
        ("x_os_version", std::env::consts::OS.to_string()),
        ("x_env", String::new()),
        ("x_app_version", oauth_client.ide_version().to_string()),
        ("x_app_type", "trae".to_string()),
        ("code_challenge", code_challenge.to_string()),
        ("code_challenge_method", "S256".to_string()),
    ];
    let query = params
        .iter()
        .map(|(key, value)| format!("{key}={}", percent_encode(value)))
        .collect::<Vec<_>>()
        .join("&");
    let hide_saas_login = if product == LoginProduct::WorkCn {
        "&hide_saas_login=true"
    } else {
        ""
    };
    format!("{LOGIN_HOST}?{query}{hide_saas_login}")
}

/// 解析登录回调：成功返回一次性 AuthCode，失败/无效返回 `InvalidCallback`。
pub fn parse_authorize_callback(query: &str) -> Result<String, LoginError> {
    if query_param(query, "error_code").is_some() {
        return Err(LoginError::InvalidCallback);
    }
    let auth_code_info = query_param(query, "authCodeInfo").ok_or(LoginError::InvalidCallback)?;
    // 字段名须与登录回调 JSON 键逐字一致，保留服务端命名
    #[allow(non_snake_case)]
    #[derive(Deserialize)]
    struct AuthCodeInfo {
        #[serde(default)]
        AuthCode: Option<String>,
    }
    let parsed: AuthCodeInfo =
        serde_json::from_str(&auth_code_info).map_err(|_| LoginError::InvalidCallback)?;
    let auth_code = parsed
        .AuthCode
        .filter(|code| !code.is_empty())
        .ok_or(LoginError::InvalidCallback)?;
    Ok(auth_code)
}

/// 从 JWT payload 解出账号 ID（`data.id`）与过期秒级时间戳。
pub fn decode_account_from_jwt(token: &str) -> Result<(String, u64), LoginError> {
    let payload_segment = token.split('.').nth(1).ok_or(LoginError::InvalidToken)?;
    let payload_bytes = URL_SAFE_NO_PAD
        .decode(payload_segment)
        .map_err(|_| LoginError::InvalidToken)?;
    let payload: serde_json::Value =
        serde_json::from_slice(&payload_bytes).map_err(|_| LoginError::InvalidToken)?;
    let account_id = payload
        .get("data")
        .and_then(|data| data.get("id"))
        .and_then(|id| id.as_str())
        .filter(|id| !id.is_empty())
        .ok_or(LoginError::InvalidToken)?
        .to_string();
    let expires_at = payload.get("exp").and_then(|exp| exp.as_u64()).unwrap_or(0);
    Ok((account_id, expires_at))
}

/// 本地回调服务器：监听 `127.0.0.1:<随机端口>/authorize` 单次请求。
pub struct LoginCallbackServer {
    listener: TcpListener,
    pub port: u16,
}

impl LoginCallbackServer {
    pub fn start() -> Result<Self, LoginError> {
        let listener = TcpListener::bind("127.0.0.1:0").map_err(|_| LoginError::InvalidCallback)?;
        let port = listener
            .local_addr()
            .map_err(|_| LoginError::InvalidCallback)?
            .port();
        listener
            .set_nonblocking(true)
            .map_err(|_| LoginError::InvalidCallback)?;
        Ok(Self { listener, port })
    }

    pub fn callback_url(&self) -> String {
        format!("http://127.0.0.1:{}/authorize", self.port)
    }

    /// 阻塞等待回调（内部 200ms 轮询 + 总超时）；返回回调 query 字符串。
    /// `should_abort` 每个轮询周期求值一次：返回 true 即中止等待
    /// （P7-4：用户取消或登录浏览器已退出时不再干等超时）。
    pub fn wait_for_callback(
        &self,
        timeout: Duration,
        should_abort: &dyn Fn() -> bool,
    ) -> Result<String, LoginError> {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if should_abort() {
                return Err(LoginError::Cancelled);
            }
            match self.listener.accept() {
                Ok((stream, _)) => return Self::read_request(stream),
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(200));
                }
                Err(_) => return Err(LoginError::InvalidCallback),
            }
        }
        Err(LoginError::InvalidCallback)
    }

    /// 读取单个 HTTP GET 请求行并回执关闭页 HTML。
    fn read_request(mut stream: TcpStream) -> Result<String, LoginError> {
        // Windows 上 accept 出的连接会继承 listener 的非阻塞模式（WSAEWOULDBLOCK
        // 80035）：浏览器「先连接、后发请求」的间隙里立即 read 会失败，且
        // set_read_timeout 对非阻塞 socket 无效。显式切回阻塞模式，让读取
        // 等待数据到达（上限由下面的 read_timeout 控制）。
        stream
            .set_nonblocking(false)
            .map_err(|_| LoginError::InvalidCallback)?;
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .map_err(|_| LoginError::InvalidCallback)?;
        let mut buffer = [0u8; 4096];
        let mut read_total = 0;
        let mut request = Vec::new();
        // 逐段读取直到请求头结束（\r\n\r\n）或到达缓冲上限。
        loop {
            if read_total >= buffer.len() {
                return Err(LoginError::InvalidCallback);
            }
            let read = stream
                .read(&mut buffer)
                .map_err(|_| LoginError::InvalidCallback)?;
            if read == 0 {
                break;
            }
            read_total += read;
            request.extend_from_slice(&buffer[..read]);
            if request.windows(4).any(|window| window == b"\r\n\r\n") {
                break;
            }
        }
        let request_text = String::from_utf8_lossy(&request);
        let request_line = request_text.lines().next().unwrap_or_default();
        // 请求行形如 `GET /authorize?authCodeInfo=... HTTP/1.1`。
        let path = request_line.split_whitespace().nth(1).unwrap_or_default();
        let query = path.split_once('?').map(|(_, query)| query.to_string());
        let body =
            "<html><body><script>window.close()</script>登录成功，请回到 Trae Sync</body></html>";
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        );
        let _ = stream.write_all(response.as_bytes());
        let _ = stream.flush();
        query.ok_or(LoginError::InvalidCallback)
    }
}

/// 登录会话：`begin_login` 产出，`complete_login` 消费；verifier/私钥只在后端内存，
/// 回调监听 socket 也随会话存活（`begin_login` 返回期间端口不释放）。
pub struct LoginSession {
    pub product: LoginProduct,
    pub profile_id: String,
    pub code_verifier: String,
    pub device_id: String,
    machine_id: String,
    pub device_public_key_pem: String,
    device_private_key_pem: String,
    callback: LoginCallbackServer,
}

/// 登录准备结果：URL 给前端打开系统浏览器。
pub struct LoginHandoff {
    pub login_url: String,
    pub session: LoginSession,
}

/// 首次登录可持续性验收结果（ADR-0027 决策 1/3）：不含 token 或私钥正文。
///
/// 验收 = 用刚入库的候选代次执行一次同设备 AuthCode 滚动换发；失败时
/// 候选凭据与失败证据保留，不回滚登录、不发起第二次 OAuth。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoginAcceptance {
    /// 验收是否通过：true = 同设备换发成功，登录完成。
    pub passed: bool,
    /// 失败时的稳定错误码（与凭据维护错误码同源）；通过为 None。
    pub error_code: Option<&'static str>,
}

impl LoginAcceptance {
    fn passed() -> Self {
        Self {
            passed: true,
            error_code: None,
        }
    }

    fn failed(error_code: &'static str) -> Self {
        Self {
            passed: false,
            error_code: Some(error_code),
        }
    }

    /// wire 状态字符串：passed / failed（组合根回执 DTO 使用）。
    pub fn status(&self) -> &'static str {
        if self.passed {
            "passed"
        } else {
            "failed"
        }
    }
}

/// 验收失败错误码：与组合根的凭据维护错误码（lib.rs
/// `credential_maintenance_error_code`）保持同一套稳定字符串。
fn acceptance_error_code(error: &CheckinCredentialError) -> &'static str {
    match error {
        CheckinCredentialError::Unavailable => "credential_unavailable",
        CheckinCredentialError::Missing => "credential_missing",
        CheckinCredentialError::Invalid => "credential_invalid",
        CheckinCredentialError::BindingMismatch => "binding_mismatch",
        CheckinCredentialError::CredentialRefreshFailed => "credential_refresh_failed",
        CheckinCredentialError::AuthMismatch => "auth_mismatch",
        CheckinCredentialError::RecoveryRequired => "manual_recovery_required",
    }
}

/// 登录成功回执；只含非敏感字段。
pub struct LoginReceipt {
    pub profile_id: String,
    pub account_id: String,
    pub screen_name: String,
    pub avatar_url: String,
    /// 首次登录可持续性验收结果（ADR-0027）；失败不回滚登录。
    pub acceptance: LoginAcceptance,
}

/// 生成随机登录 trace ID（UUID 形态，仅用于服务端链路追踪）。
fn generate_login_trace_id() -> Result<String, LoginError> {
    let mut bytes = [0u8; 16];
    rand_bytes(&mut bytes).map_err(|_| LoginError::ExchangeFailed)?;
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex_string = hex::encode(bytes);
    Ok(format!(
        "{}-{}-{}-{}-{}",
        &hex_string[0..8],
        &hex_string[8..12],
        &hex_string[12..16],
        &hex_string[16..20],
        &hex_string[20..32]
    ))
}

/// 机器指纹（非敏感）：用随机 hex 代替真实 MachineGuid，作为虚拟设备上下文。
fn generate_machine_fingerprint() -> Result<String, LoginError> {
    let mut bytes = [0u8; 16];
    rand_bytes(&mut bytes).map_err(|_| LoginError::ExchangeFailed)?;
    Ok(hex::encode(bytes))
}

/// 开始一次 OAuth 登录：生成虚拟设备与 PKCE，返回登录 URL 和会话。
/// 真实打开浏览器由前端完成（系统默认浏览器）。
pub fn begin_login(profile_id: &str) -> Result<LoginHandoff, LoginError> {
    begin_login_for(profile_id, LoginProduct::WorkCn)
}

/// 开始指定产品的 OAuth 登录。
pub fn begin_login_for(
    profile_id: &str,
    product: LoginProduct,
) -> Result<LoginHandoff, LoginError> {
    if profile_id.is_empty() || profile_id.len() > 256 {
        return Err(LoginError::InvalidCallback);
    }
    let pkce = generate_pkce_pair()?;
    let device_id = generate_virtual_device_id()?;
    let machine_id = generate_machine_fingerprint()?;
    let (device_private_key_pem, device_public_key_pem) =
        generate_device_keypair().map_err(|_| LoginError::ExchangeFailed)?;
    let callback = LoginCallbackServer::start()?;
    let callback_url = callback.callback_url();
    let trace_id = generate_login_trace_id()?;
    let login_url = build_login_url_for(
        product,
        &callback_url,
        &pkce.code_challenge,
        &trace_id,
        &device_id,
        &machine_id,
    );
    Ok(LoginHandoff {
        login_url,
        session: LoginSession {
            product,
            profile_id: profile_id.to_string(),
            code_verifier: pkce.code_verifier,
            device_id,
            machine_id,
            device_public_key_pem,
            device_private_key_pem,
            callback,
        },
    })
}

/// 等待回调并完成登录（后台线程调用）：
/// 回调 → AuthCode 换凭证 → JWT 解账号 ID → GetUserInfo 补资料 → 凭据与档案入库。
///
/// `timeout` 为回调等待上限（正式调用传 `CALLBACK_TIMEOUT_SECONDS`；
/// 测试可传短超时验证失败路径）。`should_abort` 在等待回调期间被周期求值，
/// 返回 true 即以 `Cancelled` 终止（P7-4：取消登录 / 浏览器退出即时收尾）。
/// `attempt` 是共享取消控制；它把回调、ExchangeToken 与凭据发布串成一个
/// 线性化边界，避免迟到回调继续消费已取消的登录。
/// 任何取消在发布前生效时都不产生入库副作用；发布阶段已开始则返回真实结果。
pub fn complete_login(
    session: LoginSession,
    attempt: LoginAttemptControl,
    store: &CheckinCredentialStore,
    registry: &AccountRegistry,
    client: &reqwest::blocking::Client,
    timeout: Duration,
    should_abort: &dyn Fn() -> bool,
) -> Result<LoginReceipt, LoginError> {
    // 8. 阻塞等待浏览器回调（listener 存活在 session 中）。
    let should_stop = || should_abort() || attempt.should_abort_waiting();
    let query = match session.callback.wait_for_callback(timeout, &should_stop) {
        Ok(query) => query,
        Err(error) => {
            if error == LoginError::Cancelled {
                // 外部取消只在 attempt 尚未发布时改变阶段；已发布状态由
                // finish 保留，避免把已完成结果改写成取消。
                let _ = attempt.cancel();
            } else {
                attempt.finish(LoginAttemptResult::Failed);
            }
            return Err(error);
        }
    };
    // 2. 解析一次性 AuthCode（含 error_code/结构异常拒绝）。
    let auth_code = match parse_authorize_callback(&query) {
        Ok(auth_code) => auth_code,
        Err(error) => {
            attempt.finish(LoginAttemptResult::Failed);
            return Err(error);
        }
    };
    // 回调返回后立即以同一个 CAS 争夺 ExchangeToken 资格；取消若先赢，
    // 这里失败且不会发起网络换取。
    if let Err(error) = attempt.begin_exchange() {
        attempt.finish(LoginAttemptResult::Failed);
        return Err(match error {
            LoginAttemptCancelled => LoginError::Cancelled,
        });
    }
    // 3. 构造与授权 URL 同一产品形态的设备信息块；不能把 Work 的 SOLO
    //    client 或设备形态套到 Trae CN。
    let oauth_client = session.product.oauth_client();
    let device_info = oauth_client.device_info(
        &session.device_id,
        &session.machine_id,
        &session.device_public_key_pem,
    );
    // 4. AuthCode 模式换凭证（免 DeviceProof 签名）；设备上限（20401/20408）
    //    单独归类，登录 UI 能给出针对性提示而非笼统的「换取失败」。
    let grant = match exchange_token_by_auth_code(
        client,
        &auth_code,
        &session.code_verifier,
        &device_info,
        oauth_client,
    ) {
        Ok(grant) => grant,
        Err(error) => {
            // 诊断日志：记录错误类别/码与服务端响应摘要（失败响应体不含
            // Token），用于排查个别账号被服务端拒绝的真实原因。
            append_exchange_diagnostic(store.root(), &session.profile_id, &error);
            let result = if attempt.phase() == LoginAttemptPhase::Cancelled {
                LoginError::Cancelled
            } else {
                map_exchange_login_error(&error)
            };
            attempt.finish(LoginAttemptResult::Failed);
            return Err(result);
        }
    };
    // 5. GetUserInfo 补脱敏资料；失败降级（账号 ID 已可从 JWT 解出）。
    let user_info = get_user_info_for_oauth_client(client, &grant.access_token, oauth_client)
        .unwrap_or_default();
    // ADR-0027 首次登录可持续性验收：入库后立即用刚签发的候选代次执行一次
    // 同设备换发（access 必然未过期，renew_now 走 GetPCAuthCode → 同设备
    // ExchangeToken 路径，自带 journal 原子发布与身份校验）。验收通过才算
    // 登录完成；失败不回滚登录，候选凭据与失败证据保留，回执如实带出结果。
    // 一次 OAuth 约束：失败不重试、不重铸设备、不发起第二次授权。
    let acceptance_renew =
        |binding: &CheckinProfileBinding, now: u64, lease: &CheckinProfileOperationLease| {
            RealCheckinRenewalService::new_for_oauth_client(store, oauth_client)
                .renew_now_with_lease(binding, now, lease)
                .map(|_| ())
        };
    let receipt = match persist_and_accept_login(
        &attempt,
        session.product,
        &session.profile_id,
        &session.device_id,
        &session.machine_id,
        &session.device_public_key_pem,
        &session.device_private_key_pem,
        grant,
        &user_info,
        store,
        registry,
        &acceptance_renew,
    ) {
        Ok(receipt) => receipt,
        Err(error) => {
            // 发布阶段已赢，取消不能覆盖真实的持久化失败结果。
            attempt.finish(LoginAttemptResult::Failed);
            return Err(error);
        }
    };
    attempt.finish(LoginAttemptResult::Completed);
    // Work 的 Solo 登录与 Trae CN 的独立登录都在各自产品账号池中收口；
    // Trae CN 不进入 Work 的签到/重铸调度。
    Ok(receipt)
}

/// 入库候选凭据并执行首次登录可持续性验收（ADR-0027，`complete_login` 收尾编排）。
///
/// 独立成函数以便 fixture 测试：注入 `renew` 闭包覆盖验收通过/失败两路径，
/// 失败时断言候选凭据保留、回执带稳定错误码、换发只执行一次（无第二次 OAuth）。
fn persist_and_accept_login(
    attempt: &LoginAttemptControl,
    product: LoginProduct,
    session_profile_id: &str,
    device_id: &str,
    machine_id: &str,
    device_public_key_pem: &str,
    device_private_key_pem: &str,
    grant: TokenGrant,
    user_info: &UserInfoSummary,
    store: &CheckinCredentialStore,
    registry: &AccountRegistry,
    renew: &dyn Fn(
        &CheckinProfileBinding,
        u64,
        &CheckinProfileOperationLease,
    ) -> Result<(), CheckinCredentialError>,
) -> Result<LoginReceipt, LoginError> {
    persist_and_accept_login_inner(
        attempt,
        product,
        session_profile_id,
        device_id,
        machine_id,
        device_public_key_pem,
        device_private_key_pem,
        grant,
        user_info,
        store,
        registry,
        renew,
        &|_| {},
    )
}

fn persist_and_accept_login_inner(
    attempt: &LoginAttemptControl,
    product: LoginProduct,
    session_profile_id: &str,
    device_id: &str,
    machine_id: &str,
    device_public_key_pem: &str,
    device_private_key_pem: &str,
    grant: TokenGrant,
    user_info: &UserInfoSummary,
    store: &CheckinCredentialStore,
    registry: &AccountRegistry,
    renew: &dyn Fn(
        &CheckinProfileBinding,
        u64,
        &CheckinProfileOperationLease,
    ) -> Result<(), CheckinCredentialError>,
    after_profile_resolved: &dyn Fn(&str),
) -> Result<LoginReceipt, LoginError> {
    let (account_id, _) = decode_account_from_jwt(&grant.access_token)?;
    // 产品根由 store 提供；同账号登录共锁，不同账号及 Work/CN 根各自并行。
    let _account_lease = lock_login_account(store, &account_id)?;
    let (effective_profile_id, had_existing_profile) =
        resolve_login_profile(registry, &account_id, session_profile_id)?;
    after_profile_resolved(&effective_profile_id);
    let lease = store
        .lock_profile(&effective_profile_id)
        .map_err(|_| LoginError::Storage)?;
    let receipt = persist_login_result_for_with_lease(
        Some(attempt),
        product,
        &effective_profile_id,
        had_existing_profile,
        device_id,
        machine_id,
        device_public_key_pem,
        device_private_key_pem,
        grant,
        user_info,
        store,
        registry,
        &lease,
    )?;
    // 验收 binding 用入库后的有效 profile_id（重复登录复用旧档案）与会话设备材料。
    let binding = CheckinProfileBinding::new(
        receipt.profile_id.clone(),
        receipt.account_id.clone(),
        device_id,
        device_public_key_pem,
    );
    let acceptance = match renew(&binding, unix_now(), &lease) {
        Ok(()) => LoginAcceptance::passed(),
        Err(error) => {
            // 失败证据：追加诊断日志（不含 token 正文），与换取失败共用文件。
            append_acceptance_failure(store.root(), &receipt.profile_id, &error);
            LoginAcceptance::failed(acceptance_error_code(&error))
        }
    };
    Ok(LoginReceipt {
        acceptance,
        // 其余字段原样承接入库回执（profile_id 已为有效值）。
        profile_id: receipt.profile_id,
        account_id: receipt.account_id,
        screen_name: receipt.screen_name,
        avatar_url: receipt.avatar_url,
    })
}

/// 按产品凭据根和账号 ID 取得跨进程 OAuth 登录归属锁；磁盘名只含摘要。
fn lock_login_account(
    store: &CheckinCredentialStore,
    account_id: &str,
) -> Result<FileOperationLock, LoginError> {
    // 首次登录时产品物料根可能尚不存在，先创建根再取得 sidecar 锁。
    std::fs::create_dir_all(store.root()).map_err(|_| LoginError::Storage)?;
    FileOperationLock::acquire_sibling_resource(&login_account_lock_resource(store, account_id))
        .map_err(|_| LoginError::Storage)
}

fn login_account_lock_resource(
    store: &CheckinCredentialStore,
    account_id: &str,
) -> std::path::PathBuf {
    let mut digest = Sha256::new();
    digest.update(b"traesync/oauth-login-account-lease/v1\0");
    digest.update(account_id.as_bytes());
    store.root().join(format!(
        "oauth-account-{}.lease",
        hex::encode(digest.finalize())
    ))
}

/// 验收失败诊断日志：与换取失败共用 `login-diagnostics.log`，
/// 记录错误类别（不含 Token 正文）；写失败静默忽略，不影响登录结果。
fn append_acceptance_failure(
    root: &std::path::Path,
    profile_id: &str,
    error: &CheckinCredentialError,
) {
    use std::io::Write;
    let Ok(now) = SystemTime::now().duration_since(UNIX_EPOCH) else {
        return;
    };
    // 取出换发链最近一次 ExchangeToken 失败的服务端响应详情（消费即清空）。
    let detail = crate::checkin_http::take_last_exchange_failure_detail()
        .unwrap_or_else(|| "no-detail".to_string());
    let line = format!(
        "{} acceptance-failed {} {:?} | {}\n",
        now.as_secs(),
        profile_id,
        error,
        detail
    );
    let path = root.join("login-diagnostics.log");
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = file.write_all(line.as_bytes());
    }
}

/// 将 ExchangeToken 的服务端错误归一为登录流程错误；两个设备上限码都
/// 表示服务端设备额度已满，不能让前端退化成无上下文的“服务端拒绝”。
fn map_exchange_login_error(error: &CheckinHttpError) -> LoginError {
    match error {
        CheckinHttpError::Business(20401 | 20408) => LoginError::ExchangeDeviceLimit,
        _ => LoginError::ExchangeFailed,
    }
}

/// 登录换取失败诊断日志：追加写入 `<material_root>/login-diagnostics.log`。
///
/// 每行格式：`<unix 秒> exchange-failed <profile_id> <错误 Debug 形态> | <服务端响应摘要>`。
/// 错误形态只含类别与业务码；服务端响应摘要为失败响应体（仅错误信息，
/// 无 Token），截断 500 字符。写失败静默忽略（诊断不得影响登录主流程）。
fn append_exchange_diagnostic(root: &std::path::Path, profile_id: &str, error: &CheckinHttpError) {
    use std::io::Write;
    let Ok(now) = SystemTime::now().duration_since(UNIX_EPOCH) else {
        return;
    };
    // 取出最近一次 ExchangeToken 失败的服务端响应详情（消费即清空）。
    let detail = crate::checkin_http::take_last_exchange_failure_detail()
        .unwrap_or_else(|| "no-detail".to_string());
    let line = format!(
        "{} exchange-failed {} {:?} | {}\n",
        now.as_secs(),
        profile_id,
        error,
        detail
    );
    let path = root.join("login-diagnostics.log");
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = file.write_all(line.as_bytes());
    }
}

/// 将换得的凭证与资料写入加密凭据包和账号注册表（`complete_login` 的入库步骤）。
///
/// 独立成函数以便离线测试：跳过 HTTP，直接注入 `TokenGrant` 验证
/// JWT 解码、显示名兜底、凭据包与注册表落盘的完整链路。
#[cfg(test)]
fn persist_login_result(
    profile_id: &str,
    device_id: &str,
    machine_id: &str,
    device_public_key_pem: &str,
    device_private_key_pem: &str,
    grant: TokenGrant,
    user_info: &UserInfoSummary,
    store: &CheckinCredentialStore,
    registry: &AccountRegistry,
) -> Result<LoginReceipt, LoginError> {
    persist_login_result_for(
        None,
        LoginProduct::WorkCn,
        profile_id,
        device_id,
        machine_id,
        device_public_key_pem,
        device_private_key_pem,
        grant,
        user_info,
        store,
        registry,
    )
}

/// 将指定产品换得的凭据与资料写入该产品自己的加密账号池。
#[cfg(test)]
fn persist_login_result_for(
    attempt: Option<&LoginAttemptControl>,
    product: LoginProduct,
    profile_id: &str,
    device_id: &str,
    machine_id: &str,
    device_public_key_pem: &str,
    device_private_key_pem: &str,
    grant: TokenGrant,
    user_info: &UserInfoSummary,
    store: &CheckinCredentialStore,
    registry: &AccountRegistry,
) -> Result<LoginReceipt, LoginError> {
    let (account_id, _) = decode_account_from_jwt(&grant.access_token)?;
    let (effective_profile_id, had_existing_profile) =
        resolve_login_profile(registry, &account_id, profile_id)?;
    let lease = store
        .lock_profile(&effective_profile_id)
        .map_err(|_| LoginError::Storage)?;
    persist_login_result_for_with_lease(
        attempt,
        product,
        &effective_profile_id,
        had_existing_profile,
        device_id,
        machine_id,
        device_public_key_pem,
        device_private_key_pem,
        grant,
        user_info,
        store,
        registry,
        &lease,
    )
}

/// 先定位要锁的档案；入锁后会再次核对，避免删除/并发登录后的旧快照复活档案。
fn resolve_login_profile(
    registry: &AccountRegistry,
    account_id: &str,
    session_profile_id: &str,
) -> Result<(String, bool), LoginError> {
    let records = registry.load().map_err(|_| LoginError::Storage)?;
    Ok(records
        .iter()
        .find(|record| record.account_id == account_id)
        .map(|record| (record.profile_id.clone(), true))
        .unwrap_or_else(|| (session_profile_id.to_string(), false)))
}

/// 在调用者已持有档案租约时写入；偏好与手机号只从锁内重读的档案保留。
fn persist_login_result_for_with_lease(
    attempt: Option<&LoginAttemptControl>,
    product: LoginProduct,
    effective_profile_id: &str,
    had_existing_profile: bool,
    device_id: &str,
    machine_id: &str,
    device_public_key_pem: &str,
    device_private_key_pem: &str,
    grant: TokenGrant,
    user_info: &UserInfoSummary,
    store: &CheckinCredentialStore,
    registry: &AccountRegistry,
    lease: &CheckinProfileOperationLease,
) -> Result<LoginReceipt, LoginError> {
    // JWT payload `data.id` 即服务端账号 ID；顺带校验 token 结构。
    let (account_id, _) = decode_account_from_jwt(&grant.access_token)?;
    let records = registry.load().map_err(|_| LoginError::Storage)?;
    let current_profile = records
        .iter()
        .find(|record| record.account_id == account_id)
        .map(|record| record.profile_id.as_str());
    if (had_existing_profile && current_profile != Some(effective_profile_id))
        || (!had_existing_profile && current_profile.is_some())
    {
        return Err(LoginError::Storage);
    }
    let previous = records
        .iter()
        .find(|record| record.profile_id == effective_profile_id);
    if previous.is_some_and(|record| record.account_id != account_id) {
        return Err(LoginError::Storage);
    }
    let now = unix_now();
    // 旧档案先读：重复登录时用于保留既有本地偏好（自动签到开关、备注名、
    // 归档标记、脱敏手机号）与凭据包中已补录的完整手机号（G88）。
    // 已补录的完整手机号跟随旧凭据包：重复登录覆盖凭据时不丢（用户手工录入
    // 的数据，与新令牌无绑定关系）。读取失败（首次登录/旧包不可用）按未补录。
    let previous_mobile_full = previous
        .and_then(|old| store.load(&CheckinProfileBinding::from(old)).ok())
        .and_then(|bundle| bundle.mobile_full);
    // 发布资格必须紧贴第一次持久化写入：取消若在此之前赢得 CAS，
    // 交换得到的候选凭据只留在内存，不能进入本地凭据包。
    if let Some(attempt) = attempt {
        attempt.begin_publish().map_err(|_| LoginError::Cancelled)?;
    }
    // 6. 凭据包加密入库（DPAPI；敏感字段只在密文中落盘）。
    let bundle = CheckinCredentialBundle {
        profile_id: effective_profile_id.to_string(),
        account_id: account_id.clone(),
        device_id: device_id.to_string(),
        machine_id: machine_id.to_string(),
        device_public_key: device_public_key_pem.to_string(),
        device_private_key: device_private_key_pem.to_string(),
        access_token: grant.access_token,
        refresh_token: grant.refresh_token,
        // 与登录/换取所用形态一致；Work 与 Trae CN 的凭据池完全分开。
        client_id: product.oauth_client().client_id().to_string(),
        access_token_expires_at_unix_seconds: grant.access_token_expires_at_unix_seconds,
        refresh_token_expires_at_unix_seconds: grant.refresh_token_expires_at_unix_seconds,
        mobile_full: previous_mobile_full,
    };
    store
        .save_with_lease(lease, &bundle)
        .map_err(|_| LoginError::Storage)?;
    // 7. 非敏感档案 upsert（profile_id 唯一，重复登录覆盖旧记录；
    //    保留既有自动签到开关、本地备注名与归档标记——均为用户本地偏好；
    //    脱敏手机号取服务端最新值，服务端未返回时保留已采值）。
    let previous_auto_enabled = previous.map(|old| old.auto_checkin_enabled).unwrap_or(true);
    let previous_display_name = previous.and_then(|old| old.display_name.clone());
    let previous_archived = previous.map(|old| old.archived).unwrap_or(false);
    let previous_mobile = previous
        .map(|old| old.masked_mobile.clone())
        .unwrap_or_default();
    let masked_mobile = if user_info.masked_mobile.is_empty() {
        previous_mobile
    } else {
        user_info.masked_mobile.clone()
    };
    let screen_name = display_name(&account_id, user_info);
    let record = AccountRecord {
        profile_id: effective_profile_id.to_string(),
        account_id,
        screen_name: screen_name.clone(),
        avatar_url: user_info.avatar_url.clone(),
        device_id: device_id.to_string(),
        device_public_key: device_public_key_pem.to_string(),
        display_name: previous_display_name,
        masked_mobile,
        created_at_unix_seconds: now,
        last_verified_at_unix_seconds: now,
        // 登录即铸造新设备：记录铸造时刻，供 9074 报错时区分
        // 「设备太新稍后重试」与「设备被拒需要重置」。
        device_created_at_unix_seconds: now,
        auto_checkin_enabled: previous_auto_enabled,
        archived: previous_archived,
    };
    registry.upsert(&record).map_err(|_| LoginError::Storage)?;
    // 入库路径的占位值：真实验收结果由 persist_and_accept_login 覆盖；
    // 直接调用入库（旧测试路径）没有验收步骤，按通过表示「无验收失败」。
    Ok(LoginReceipt {
        profile_id: record.profile_id,
        account_id: record.account_id,
        screen_name,
        avatar_url: record.avatar_url,
        acceptance: LoginAcceptance::passed(),
    })
}

/// 展示名兜底：优先服务端 ScreenName，其次脱敏手机号，最后账号 ID 后 4 位。
fn display_name(account_id: &str, user_info: &UserInfoSummary) -> String {
    if !user_info.screen_name.is_empty() {
        return user_info.screen_name.clone();
    }
    if !user_info.masked_mobile.is_empty() {
        return user_info.masked_mobile.clone();
    }
    let suffix: String = account_id
        .chars()
        .rev()
        .take(4)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    format!("账号 {suffix}")
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
    fn cancel_before_exchange_prevents_token_exchange() {
        let attempt = LoginAttemptControl::new();

        assert_eq!(
            attempt.cancel(),
            LoginCancelResult::Cancelled,
            "等待回调阶段取消必须取得线性化点"
        );
        assert_eq!(attempt.phase(), LoginAttemptPhase::Cancelled);
        assert_eq!(
            attempt.begin_exchange(),
            Err(LoginAttemptCancelled),
            "取消后不得启动 ExchangeToken"
        );
    }

    #[test]
    fn cancel_during_exchange_prevents_credential_publish() {
        let attempt = LoginAttemptControl::new();

        assert_eq!(attempt.begin_exchange(), Ok(()));
        assert_eq!(attempt.phase(), LoginAttemptPhase::Exchanging);
        assert_eq!(
            attempt.cancel(),
            LoginCancelResult::Cancelled,
            "ExchangeToken 未取得发布资格时取消必须生效"
        );
        assert_eq!(
            attempt.begin_publish(),
            Err(LoginAttemptCancelled),
            "取消赢得 ExchangeToken 竞态后不得发布凭据"
        );
        assert_eq!(attempt.phase(), LoginAttemptPhase::Cancelled);
    }

    #[test]
    fn publish_phase_wins_cancel_and_preserves_completion_result() {
        let attempt = LoginAttemptControl::new();

        assert_eq!(attempt.begin_exchange(), Ok(()));
        assert_eq!(attempt.begin_publish(), Ok(()));
        assert_eq!(attempt.phase(), LoginAttemptPhase::Publishing);
        assert_eq!(
            attempt.cancel(),
            LoginCancelResult::TooLate,
            "发布阶段已赢时取消不能报告已取消"
        );
        attempt.finish(LoginAttemptResult::Completed);
        assert_eq!(attempt.phase(), LoginAttemptPhase::Completed);
        assert_eq!(
            attempt.cancel(),
            LoginCancelResult::AlreadyFinished,
            "完成后取消必须保留真实完成结果"
        );
    }

    #[test]
    fn pkce_challenge_is_verifiable_sha256() {
        let pair = generate_pkce_pair().unwrap();
        // challenge 必须可由 verifier 独立重算（服务端校验规则）。
        let recomputed = URL_SAFE_NO_PAD.encode(Sha256::digest(pair.code_verifier.as_bytes()));
        assert_eq!(pair.code_challenge, recomputed);
        // 48 字节 base64url 无填充 = 64 字符。
        assert_eq!(pair.code_verifier.len(), 64);
    }

    #[test]
    fn pkce_pairs_are_random() {
        let first = generate_pkce_pair().unwrap();
        let second = generate_pkce_pair().unwrap();
        assert_ne!(first.code_verifier, second.code_verifier);
    }

    #[test]
    fn virtual_device_id_shape_matches_real_ids() {
        for _ in 0..8 {
            let device_id = generate_virtual_device_id().unwrap();
            assert_eq!(device_id.len(), 16);
            assert!(device_id.chars().all(|c| c.is_ascii_digit()));
            assert!(!device_id.starts_with('0'));
        }
    }

    #[test]
    fn login_url_contains_protocol_parameters() {
        let url = build_login_url(
            "http://127.0.0.1:58789/authorize",
            "challenge-1",
            "trace-8",
            "1234567890123456",
            "machine-8",
        );
        assert!(url.starts_with("https://www.trae.cn/authorization?"));
        // auth_from 必须与 SOLO 通道一致，否则 AuthCode 通道绑定错误 → 20403。
        assert!(url.contains("login_version=1"));
        assert!(url.contains("auth_from=solo"));
        assert!(url.contains("hide_saas_login=true"));
        assert!(url.contains("code_challenge_method=S256"));
        assert!(url.contains("code_challenge=challenge-1"));
        // 登录固定 SOLO 形态（同一产品改名，选可用通道；见模块注释）。
        assert!(url.contains("client_id=en1oxy7wnw8j9n"));
        assert!(url.contains("x_app_version=0.1.54"));
        // 回调 URL 必须整体编码（: 与 / 都转义）。
        assert!(url.contains("auth_callback_url=http%3A%2F%2F127.0.0.1%3A58789%2Fauthorize"));
        assert!(url.contains("device_id=1234567890123456"));
        assert!(url.contains("x_device_id=1234567890123456"));
    }

    #[test]
    fn trae_cn_login_url_uses_trae_channel() {
        let url = build_login_url_for(
            LoginProduct::TraeCn,
            "http://127.0.0.1:58789/authorize",
            "challenge-cn",
            "trace-cn",
            "1234567890123456",
            "machine-cn",
        );
        assert!(url.contains("login_version=1"));
        assert!(url.contains("auth_from=trae"));
        assert!(url.contains("client_id=ono9krqynydwx5"));
        assert!(url.contains("x_app_version=3.3.102"));
        assert!(!url.contains("hide_saas_login=true"));
    }

    #[test]
    fn exchange_device_limit_codes_use_specific_login_error() {
        assert_eq!(
            map_exchange_login_error(&CheckinHttpError::Business(20401)),
            LoginError::ExchangeDeviceLimit
        );
        assert_eq!(
            map_exchange_login_error(&CheckinHttpError::Business(20408)),
            LoginError::ExchangeDeviceLimit
        );
        assert_eq!(
            map_exchange_login_error(&CheckinHttpError::Business(20403)),
            LoginError::ExchangeFailed
        );
    }

    #[test]
    fn product_login_session_keeps_auth_channel() {
        let handoff = begin_login_for("profile-cn", LoginProduct::TraeCn).unwrap();
        assert_eq!(handoff.session.product, LoginProduct::TraeCn);
        assert!(handoff.login_url.contains("auth_from=trae"));
    }

    #[test]
    fn callback_parse_extracts_auth_code() {
        let auth_info = serde_json::json!({"AuthCode": "auth-code-1"});
        let encoded = percent_encode(&auth_info.to_string());
        let query = format!("authCodeInfo={encoded}&userInfo=x");
        assert_eq!(parse_authorize_callback(&query).unwrap(), "auth-code-1");
    }

    #[test]
    fn callback_parse_rejects_error_and_missing_code() {
        assert_eq!(
            parse_authorize_callback("error_code=2002&error_message=x"),
            Err(LoginError::InvalidCallback)
        );
        assert_eq!(
            parse_authorize_callback("userInfo=only"),
            Err(LoginError::InvalidCallback)
        );
        let no_code = percent_encode(&serde_json::json!({"Other": "1"}).to_string());
        assert_eq!(
            parse_authorize_callback(&format!("authCodeInfo={no_code}")),
            Err(LoginError::InvalidCallback)
        );
    }

    #[test]
    fn jwt_decode_extracts_account_id() {
        // 构造最小 JWT：header.payload（签名段不参与解码）。
        let payload = serde_json::json!({
            "data": {"id": "1800000000000001", "type": "user"},
            "exp": 1788494688,
            "iat": 1787285060
        });
        let payload_b64 = URL_SAFE_NO_PAD.encode(payload.to_string());
        let token = format!("eyJhbGciOiJSUzI1NiJ9.{payload_b64}.signature");
        let (account_id, expires_at) = decode_account_from_jwt(&token).unwrap();
        assert_eq!(account_id, "1800000000000001");
        assert_eq!(expires_at, 1788494688);
    }

    #[test]
    fn jwt_decode_rejects_malformed_tokens() {
        assert_eq!(
            decode_account_from_jwt("not-a-jwt"),
            Err(LoginError::InvalidToken)
        );
        let payload = URL_SAFE_NO_PAD.encode(br#"{"no_data": true}"#.to_vec());
        let token = format!("h.{payload}.s");
        assert_eq!(
            decode_account_from_jwt(&token),
            Err(LoginError::InvalidToken)
        );
    }

    #[test]
    fn percent_roundtrip_preserves_values() {
        let original = "{\"AuthCode\":\"a b&c=d\"}";
        assert_eq!(percent_decode(&percent_encode(original)), original);
    }

    #[test]
    fn login_trace_id_has_uuid_shape() {
        let trace = generate_login_trace_id().unwrap();
        assert_eq!(trace.len(), 36);
        assert_eq!(trace.matches('-').count(), 4);
        // UUID v4 版本位。
        assert!(trace.starts_with(trace.chars().take(84).collect::<String>().as_str()));
    }

    #[cfg(windows)]
    #[test]
    fn persist_login_result_stores_bundle_and_record() {
        let root = tempfile::tempdir().unwrap();
        let store = CheckinCredentialStore::new(root.path());
        let registry = AccountRegistry::new(root.path());
        // 真实 EC P-256 密钥对：load 的绑定校验要求 PEM 可解析。
        let (private_pem, public_pem) = generate_device_keypair().unwrap();
        // 最小合法 JWT：payload 含 data.id（服务端账号 ID）。
        let payload = serde_json::json!({"data": {"id": "1800000000000001"}, "exp": 1_800_000_000});
        let payload_b64 = URL_SAFE_NO_PAD.encode(payload.to_string());
        let access_token = format!("eyJhbGciOiJSUzI1NiJ9.{payload_b64}.sig");
        let grant = TokenGrant {
            access_token,
            refresh_token: "refresh-8".to_string(),
            access_token_expires_at_unix_seconds: 1_800_000_000,
            refresh_token_expires_at_unix_seconds: 1_800_800_000,
        };
        let user_info = UserInfoSummary {
            screen_name: "测试用户".to_string(),
            avatar_url: "https://example.test/a.png".to_string(),
            masked_mobile: "138****0000".to_string(),
        };
        let receipt = persist_login_result(
            "profile-login",
            "1234567890123456",
            "machine-login-8",
            &public_pem,
            &private_pem,
            grant,
            &user_info,
            &store,
            &registry,
        )
        .unwrap();
        assert_eq!(receipt.profile_id, "profile-login");
        assert_eq!(receipt.account_id, "1800000000000001");
        assert_eq!(receipt.screen_name, "测试用户");
        // 非敏感档案落盘并可读回。
        let record = registry.find("profile-login").unwrap().unwrap();
        assert_eq!(record.account_id, "1800000000000001");
        assert_eq!(record.device_id, "1234567890123456");
        // U-8：登录时自动采集脱敏手机号。
        assert_eq!(record.masked_mobile, "138****0000");
        // 凭据包可用匹配绑定解密读回（绑定校验通过即结构完整）。
        let binding = crate::checkin_credential::CheckinProfileBinding::new(
            "profile-login",
            "1800000000000001",
            "1234567890123456",
            &public_pem,
        );
        let bundle = store.load(&binding).unwrap();
        assert_eq!(bundle.refresh_token, "refresh-8");
        assert_eq!(bundle.access_token_expires_at_unix_seconds, 1_800_000_000);
    }

    #[cfg(windows)]
    #[test]
    fn persist_trae_cn_login_result_stores_cn_client_id() {
        let root = tempfile::tempdir().unwrap();
        let store = CheckinCredentialStore::new(root.path());
        let registry = AccountRegistry::new(root.path());
        let (private_pem, public_pem) = generate_device_keypair().unwrap();
        let payload = serde_json::json!({"data": {"id": "1800000000000002"}, "exp": 1_800_000_000});
        let payload_b64 = URL_SAFE_NO_PAD.encode(payload.to_string());
        let grant = TokenGrant {
            access_token: format!("eyJhbGciOiJSUzI1NiJ9.{payload_b64}.sig"),
            refresh_token: "refresh-cn".to_string(),
            access_token_expires_at_unix_seconds: 1_800_000_000,
            refresh_token_expires_at_unix_seconds: 1_800_800_000,
        };
        let receipt = persist_login_result_for(
            None,
            LoginProduct::TraeCn,
            "profile-cn",
            "1234567890123457",
            "machine-cn",
            &public_pem,
            &private_pem,
            grant,
            &UserInfoSummary::default(),
            &store,
            &registry,
        )
        .unwrap();
        let record = registry.find(&receipt.profile_id).unwrap().unwrap();
        let binding = CheckinProfileBinding::new(
            receipt.profile_id,
            record.account_id,
            record.device_id,
            record.device_public_key,
        );
        let bundle = store.load(&binding).unwrap();
        assert_eq!(bundle.client_id, OAuthClient::TraeCn.client_id());
    }

    #[cfg(windows)]
    #[test]
    fn persist_login_result_reuses_profile_for_same_account() {
        // 同一账号（JWT data.id 相同）用不同会话 profile_id 重复登录：
        // 必须复用首个档案的 profile_id 覆盖更新，避免重复条目重复领取积分。
        let root = tempfile::tempdir().unwrap();
        let store = CheckinCredentialStore::new(root.path());
        let registry = AccountRegistry::new(root.path());
        let (private_pem, public_pem) = generate_device_keypair().unwrap();
        let payload = serde_json::json!({"data": {"id": "1800000000000001"}, "exp": 1_800_000_000});
        let payload_b64 = URL_SAFE_NO_PAD.encode(payload.to_string());
        let access_token = format!("eyJhbGciOiJSUzI1NiJ9.{payload_b64}.sig");
        let user_info = UserInfoSummary::default();
        let grant = TokenGrant {
            access_token,
            refresh_token: "refresh-8".to_string(),
            access_token_expires_at_unix_seconds: 1_800_000_000,
            refresh_token_expires_at_unix_seconds: 1_800_800_000,
        };
        let first = persist_login_result(
            "profile-first",
            "1234567890123456",
            "machine-8",
            &public_pem,
            &private_pem,
            grant,
            &user_info,
            &store,
            &registry,
        )
        .unwrap();
        assert_eq!(first.profile_id, "profile-first");

        // 第二次登录：新虚拟设备 + 新会话 profile_id，但账号 ID 相同。
        let (private_pem_2, public_pem_2) = generate_device_keypair().unwrap();
        let grant_2 = TokenGrant {
            access_token: format!("eyJhbGciOiJSUzI1NiJ9.{payload_b64}.sig"),
            refresh_token: "refresh-2".to_string(),
            access_token_expires_at_unix_seconds: 1_800_200_000,
            refresh_token_expires_at_unix_seconds: 1_800_300_000,
        };
        let second = persist_login_result(
            "profile-second",
            "6543280987654328",
            "machine-2",
            &public_pem_2,
            &private_pem_2,
            grant_2,
            &user_info,
            &store,
            &registry,
        )
        .unwrap();
        // 复用首个档案的 profile_id，注册表只有一条记录。
        assert_eq!(second.profile_id, "profile-first");
        let records = registry.load().unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].profile_id, "profile-first");
        // 新虚拟设备身份覆盖旧值，后续签到按新设备绑定。
        assert_eq!(records[0].device_id, "6543280987654328");
    }

    #[cfg(windows)]
    #[test]
    fn persist_login_result_preserves_alias_and_mobile() {
        // U-8：本地备注名与已采手机号跨重登录保留——两者均为本地
        // 状态，服务端覆盖更新（screen_name/device 等）不得清空它们。
        let root = tempfile::tempdir().unwrap();
        let store = CheckinCredentialStore::new(root.path());
        let registry = AccountRegistry::new(root.path());
        let (private_pem, public_pem) = generate_device_keypair().unwrap();
        let payload = serde_json::json!({"data": {"id": "1800000000000001"}, "exp": 1_800_000_000});
        let payload_b64 = URL_SAFE_NO_PAD.encode(payload.to_string());
        let grant = |token: &str| TokenGrant {
            access_token: format!("eyJhbGciOiJSUzI1NiJ9.{payload_b64}.{token}"),
            refresh_token: format!("refresh-{token}"),
            access_token_expires_at_unix_seconds: 1_800_000_000,
            refresh_token_expires_at_unix_seconds: 1_800_800_000,
        };
        let user_info = UserInfoSummary {
            screen_name: "用户92438883708".to_string(),
            avatar_url: String::new(),
            masked_mobile: String::new(),
        };
        persist_login_result(
            "profile-a",
            "1234567890123456",
            "machine-8",
            &public_pem,
            &private_pem,
            grant("t8"),
            &user_info,
            &store,
            &registry,
        )
        .unwrap();
        // 用户设置本地备注名 + 健康检测补采手机号（服务端未在登录时返回）。
        registry
            .set_display_name("profile-a", Some("主力号"))
            .unwrap();
        registry
            .backfill_masked_mobile("profile-a", "138****0000")
            .unwrap();
        // 重新登录（服务端这次返回了手机号但没返回备注名——备注名本就纯本地）。
        let user_info_again = UserInfoSummary {
            masked_mobile: "138****0000".to_string(),
            ..user_info.clone()
        };
        persist_login_result(
            "profile-a",
            "6543280987654328",
            "machine-2",
            &public_pem,
            &private_pem,
            grant("t2"),
            &user_info_again,
            &store,
            &registry,
        )
        .unwrap();
        let record = registry.find("profile-a").unwrap().unwrap();
        assert_eq!(record.display_name.as_deref(), Some("主力号"));
        assert_eq!(record.masked_mobile, "138****0000");
    }

    #[test]
    fn display_name_falls_back_gracefully() {
        // 优先服务端 ScreenName。
        let with_name = UserInfoSummary {
            screen_name: "张三".to_string(),
            masked_mobile: "138****0000".to_string(),
            avatar_url: String::new(),
        };
        assert_eq!(display_name("1800000000000001", &with_name), "张三");
        // ScreenName 缺失时退到脱敏手机号。
        let mobile_only = UserInfoSummary {
            screen_name: String::new(),
            masked_mobile: "138****0000".to_string(),
            avatar_url: String::new(),
        };
        assert_eq!(
            display_name("1800000000000001", &mobile_only),
            "138****0000"
        );
        // 全部缺失时用账号 ID 后 4 位兜底。
        assert_eq!(
            display_name("1800000000000001", &UserInfoSummary::default()),
            "账号 0001"
        );
    }

    #[test]
    fn callback_server_receives_authorize_query() {
        let server = LoginCallbackServer::start().unwrap();
        let url = server.callback_url();
        let expected_query = "authCodeInfo=%7B%22AuthCode%22%3A%22code-8%22%7D";
        let client_thread = std::thread::spawn(move || {
            // 模拟浏览器回调：GET /authorize?<query>（去掉 scheme 与路径只留 host:port）。
            let host_port = url
                .trim_start_matches("http://")
                .split('/')
                .next()
                .unwrap_or_default()
                .to_string();
            let mut stream = TcpStream::connect(&host_port).unwrap();
            let request =
                format!("GET /authorize?{expected_query} HTTP/1.1\r\nHost: {host_port}\r\n\r\n");
            stream.write_all(request.as_bytes()).unwrap();
            // 读取回执（等待服务器关闭连接）。
            let mut response = String::new();
            let _ = stream.read_to_string(&mut response);
        });
        let query = server
            .wait_for_callback(Duration::from_secs(5), &|| false)
            .unwrap();
        assert_eq!(query, expected_query);
        client_thread.join().unwrap();
    }

    // 回归（2026-08-27 实发故障）：重放用户真实回调（原样 8482 字符 query +
    // 浏览器请求头 + 「连接后延迟发送」时序）。Windows 上 accept 出的连接
    // 继承 listener 非阻塞模式，若 read_request 未切回阻塞模式，会在此
    // 竞态窗口读出 WouldBlock(80035) 并把登录误判为 InvalidCallback。
    #[test]
    fn callback_read_survives_delayed_request_after_connect() {
        let server = LoginCallbackServer::start().unwrap();
        let url = server.callback_url();
        let host_port = url
            .trim_start_matches("http://")
            .split('/')
            .next()
            .unwrap_or_default()
            .to_string();
        let query = "isRedirect=true&scope=trae&authCodeInfo=%7B%22AuthCode%22%3A%22vWZsaLleIoA68mQrNQcTzD-ocPIdy9QY5BWzZNVfDbk%22%2C%22ExpireAt%22%3A1787840558747%2C%22ExpireDuration%22%3A600000%7D&loginTraceID=b79ef0b8-83f8-4026-95fd-dc72d667688f&host=https%3A%2F%2Fapi.trae.com.cn&userRegion=cn&userInfo=%7B%22AIRegion%22%3A%22CN%22%2C%22AuditInfo%22%3A%22%7B%5C%22audit_status%5C%22%3A2%2C%5C%22is_auditing%5C%22%3Afalse%2C%5C%22last_modify_time%5C%22%3A1787674673%2C%5C%22unpass_reason%5C%22%3A%5C%22%5C%22%7D%22%2C%22AvatarUrl%22%3A%22https%3A%2F%2Fp3-passport.byteacctimg.com%2Fimg%2Fuser-avatar%2Fassets%2F00000000000000000000000000000000_892_892.png%7E128x128.image%22%2C%22Description%22%3A%22%22%2C%22Gender%22%3A%220%22%2C%22LastLoginTime%22%3A%222026-08-27T12%3A12%3A30%2B08%3A00%22%2C%22LastLoginType%22%3A%22sms%22%2C%22MigrateToSG%22%3Afalse%2C%22NonPlainTextEmail%22%3A%22%22%2C%22NonPlainTextMobile%22%3A%22855******03%22%2C%22Region%22%3A%22CN%22%2C%22RegisterTime%22%3A%222026-08-26T00%3A16%3A36.469%2B08%3A00%22%2C%22ScreenName%22%3A%22%E6%B5%8B%E8%AF%95%E7%94%A8%E6%88%B7%22%2C%22TenantID%22%3A%227a2b894p7dr0c4%22%2C%22UserID%22%3A%221000000000000001%22%2C%22UtmInfo%22%3A%7B%22ActivityID%22%3A%22%22%2C%22ActivityName%22%3A%22%22%2C%22Campaign%22%3A%22%22%2C%22Content%22%3A%22%22%2C%22Medium%22%3A%22%22%2C%22PromotionChannel%22%3A%22%22%2C%22Source%22%3A%22traework_client_account_page%22%2C%22Term%22%3A%22%22%7D%7D";
        let client_thread = std::thread::spawn(move || {
            let mut stream = TcpStream::connect(&host_port).unwrap();
            // 模拟浏览器：连接后先不发数据（服务端 accept 轮询会先拿到连接）。
            std::thread::sleep(Duration::from_millis(300));
            let request = format!(
                "GET /authorize?{query} HTTP/1.1\r\nHost: {host_port}\r\nConnection: keep-alive\r\nUpgrade-Insecure-Requests: 1\r\nUser-Agent: Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/138.0.0.0 Safari/537.36\r\nAccept: text/html,application/xhtml+xml,application/xml;q=0.9,image/avif,image/webp,*/*;q=0.8\r\nSec-Fetch-Site: cross-site\r\nSec-Fetch-Mode: navigate\r\nSec-Fetch-User: ?1\r\nSec-Fetch-Dest: document\r\nAccept-Encoding: gzip, deflate, br, zstd\r\nAccept-Language: zh-CN,zh;q=0.9\r\n\r\n"
            );
            stream.write_all(request.as_bytes()).unwrap();
            let mut response = String::new();
            let _ = stream.read_to_string(&mut response);
        });
        let received = server
            .wait_for_callback(Duration::from_secs(5), &|| false)
            .unwrap();
        let auth_code = parse_authorize_callback(&received).unwrap();
        assert_eq!(auth_code, "vWZsaLleIoA68mQrNQcTzD-ocPIdy9QY5BWzZNVfDbk");
        client_thread.join().unwrap();
    }

    #[test]
    fn complete_login_times_out_without_callback() {
        let handoff = begin_login("profile-timeout").unwrap();
        let root = tempfile::tempdir().unwrap();
        let store = CheckinCredentialStore::new(root.path());
        let registry = AccountRegistry::new(root.path());
        let client = reqwest::blocking::Client::new();
        // 零超时：无回调立即失败，且不产生任何入库副作用。
        let result = complete_login(
            handoff.session,
            LoginAttemptControl::new(),
            &store,
            &registry,
            &client,
            Duration::ZERO,
            &|| false,
        );
        assert_eq!(result.err(), Some(LoginError::InvalidCallback));
        assert!(registry.load().unwrap().is_empty());
    }

    // ===== ADR-0027 首次登录可持续性验收（P9-0 / P11-3）=====

    /// 构造登录入库用的最小 TokenGrant（JWT payload 含 data.id）。
    fn acceptance_grant(account_id: &str, refresh_token: &str) -> TokenGrant {
        let payload = serde_json::json!({"data": {"id": account_id}, "exp": 1_800_000_000});
        let payload_b64 = URL_SAFE_NO_PAD.encode(payload.to_string());
        TokenGrant {
            access_token: format!("eyJhbGciOiJSUzU1NiJ9.{payload_b64}.sig"),
            refresh_token: refresh_token.to_string(),
            access_token_expires_at_unix_seconds: 1_800_000_000,
            refresh_token_expires_at_unix_seconds: 1_800_800_000,
        }
    }

    #[cfg(windows)]
    #[test]
    fn concurrent_first_login_same_account_reuses_profile_and_other_account_progresses() {
        use std::sync::mpsc;
        use std::thread;

        let root = tempfile::tempdir().unwrap();
        let work_root = root.path().join("work");
        let cn_root = root.path().join("cn");
        let work_store = CheckinCredentialStore::new(&work_root);
        let registry = AccountRegistry::new(&work_root);
        let account_id = "1800000000000191";
        let other_account_id = "1800000000000192";

        // 同账号锁名只保留摘要，且产品物料根不同就天然分锁。
        let work_lock = login_account_lock_resource(&work_store, account_id);
        let cn_lock =
            login_account_lock_resource(&CheckinCredentialStore::new(&cn_root), account_id);
        assert_ne!(work_lock, cn_lock);
        assert!(!work_lock.to_string_lossy().contains(account_id));

        let (first_resolved_tx, first_resolved_rx) = mpsc::channel();
        let (release_first_resolve_tx, release_first_resolve_rx) = mpsc::channel();
        let (first_acceptance_tx, first_acceptance_rx) = mpsc::channel();
        let (release_first_acceptance_tx, release_first_acceptance_rx) = mpsc::channel();
        let (second_started_tx, second_started_rx) = mpsc::channel();
        let (second_resolved_tx, second_resolved_rx) = mpsc::channel();
        let (release_second_resolve_tx, release_second_resolve_rx) = mpsc::channel();
        let (other_done_tx, other_done_rx) = mpsc::channel();

        let first_store = CheckinCredentialStore::new(&work_root);
        let first_registry = AccountRegistry::new(&work_root);
        let (first_private, first_public) = generate_device_keypair().unwrap();
        let first = thread::spawn(move || {
            let attempt = LoginAttemptControl::new();
            assert_eq!(attempt.begin_exchange(), Ok(()));
            persist_and_accept_login_inner(
                &attempt,
                LoginProduct::WorkCn,
                "oauth-session-first",
                "device-first-1234",
                "machine-first",
                &first_public,
                &first_private,
                acceptance_grant(account_id, "refresh-first"),
                &UserInfoSummary::default(),
                &first_store,
                &first_registry,
                &|_binding, _now, _lease| {
                    first_acceptance_tx.send(()).unwrap();
                    release_first_acceptance_rx.recv().unwrap();
                    Ok(())
                },
                &|profile_id| {
                    first_resolved_tx.send(profile_id.to_string()).unwrap();
                    release_first_resolve_rx.recv().unwrap();
                },
            )
        });
        assert_eq!(
            first_resolved_rx
                .recv_timeout(Duration::from_secs(5))
                .unwrap(),
            "oauth-session-first"
        );

        let second_store = CheckinCredentialStore::new(&work_root);
        let second_registry = AccountRegistry::new(&work_root);
        let (second_private, second_public) = generate_device_keypair().unwrap();
        let second = thread::spawn(move || {
            let attempt = LoginAttemptControl::new();
            assert_eq!(attempt.begin_exchange(), Ok(()));
            second_started_tx.send(()).unwrap();
            persist_and_accept_login_inner(
                &attempt,
                LoginProduct::WorkCn,
                "oauth-session-second",
                "device-second-1234",
                "machine-second",
                &second_public,
                &second_private,
                acceptance_grant(account_id, "refresh-second"),
                &UserInfoSummary::default(),
                &second_store,
                &second_registry,
                &|_binding, _now, _lease| Ok(()),
                &|profile_id| {
                    second_resolved_tx.send(profile_id.to_string()).unwrap();
                    release_second_resolve_rx.recv().unwrap();
                },
            )
        });
        second_started_rx
            .recv_timeout(Duration::from_secs(5))
            .unwrap();
        let resolved_before_first_publish = second_resolved_rx
            .recv_timeout(Duration::from_millis(300))
            .ok();

        // A 不得占住产品根互斥；不同账号 B 必须先于 A 的验收结束完成。
        let other_store = CheckinCredentialStore::new(&work_root);
        let other_registry = AccountRegistry::new(&work_root);
        let (other_private, other_public) = generate_device_keypair().unwrap();
        let other = thread::spawn(move || {
            let attempt = LoginAttemptControl::new();
            assert_eq!(attempt.begin_exchange(), Ok(()));
            let result = persist_and_accept_login(
                &attempt,
                LoginProduct::WorkCn,
                "oauth-session-other",
                "device-other-1234",
                "machine-other",
                &other_public,
                &other_private,
                acceptance_grant(other_account_id, "refresh-other"),
                &UserInfoSummary::default(),
                &other_store,
                &other_registry,
                &|_binding, _now, _lease| Ok(()),
            );
            other_done_tx.send(result).unwrap();
        });
        let other_result = other_done_rx.recv_timeout(Duration::from_secs(5)).unwrap();

        release_first_resolve_tx.send(()).unwrap();
        first_acceptance_rx
            .recv_timeout(Duration::from_secs(5))
            .unwrap();
        let resolved_during_acceptance = second_resolved_rx
            .recv_timeout(Duration::from_millis(300))
            .ok();
        release_first_acceptance_tx.send(()).unwrap();

        let resolved_after_acceptance = if resolved_before_first_publish.is_some() {
            release_second_resolve_tx.send(()).unwrap();
            None
        } else if resolved_during_acceptance.is_some() {
            release_second_resolve_tx.send(()).unwrap();
            None
        } else {
            let profile_id = second_resolved_rx
                .recv_timeout(Duration::from_secs(5))
                .unwrap();
            release_second_resolve_tx.send(()).unwrap();
            Some(profile_id)
        };

        let first_receipt = first.join().unwrap().unwrap();
        let second_receipt = second.join().unwrap().unwrap();
        other.join().unwrap();
        assert!(other_result.is_ok());
        assert!(resolved_before_first_publish.is_none());
        assert!(resolved_during_acceptance.is_none());
        assert_eq!(
            resolved_after_acceptance.as_deref(),
            Some("oauth-session-first")
        );
        assert_eq!(first_receipt.profile_id, "oauth-session-first");
        assert_eq!(second_receipt.profile_id, first_receipt.profile_id);
        let records = registry.load().unwrap();
        assert_eq!(
            records
                .iter()
                .filter(|record| record.account_id == account_id)
                .count(),
            1
        );
        assert_eq!(
            records
                .iter()
                .find(|record| record.account_id == account_id)
                .unwrap()
                .profile_id,
            "oauth-session-first"
        );
        assert!(records
            .iter()
            .any(|record| record.account_id == other_account_id));
    }

    #[cfg(windows)]
    #[test]
    fn first_login_acceptance_passed_rotates_candidate_once() {
        let root = tempfile::tempdir().unwrap();
        let store = CheckinCredentialStore::new(root.path());
        let registry = AccountRegistry::new(root.path());
        let (private_pem, public_pem) = generate_device_keypair().unwrap();
        let attempt = LoginAttemptControl::new();
        // 回调返回后先进入 Exchanging，符合发布函数的阶段前置条件。
        assert_eq!(attempt.begin_exchange(), Ok(()));
        assert_eq!(attempt.phase(), LoginAttemptPhase::Exchanging);
        let calls = std::cell::Cell::new(0usize);
        // fixture 验收：只记录调用次数与绑定，不触网、不改写凭据。
        let renew =
            |binding: &CheckinProfileBinding, now: u64, _lease: &CheckinProfileOperationLease| {
                calls.set(calls.get() + 1);
                // 验收目标必须是刚入库的候选代次。
                assert_eq!(binding.profile_id, "profile-accept");
                assert_eq!(binding.account_id, "1800000000000101");
                assert!(now > 0);
                Ok(())
            };
        let receipt = persist_and_accept_login(
            &attempt,
            LoginProduct::WorkCn,
            "profile-accept",
            "1234567890123456",
            "machine-accept",
            &public_pem,
            &private_pem,
            acceptance_grant("1800000000000101", "refresh-accept"),
            &UserInfoSummary::default(),
            &store,
            &registry,
            &renew,
        )
        .unwrap();
        // 验收通过：回执如实标记 passed。
        assert!(receipt.acceptance.passed);
        assert_eq!(receipt.acceptance.status(), "passed");
        assert!(receipt.acceptance.error_code.is_none());
        // 同设备换发只执行一次；一次 OAuth 约束下不存在第二次授权。
        assert_eq!(calls.get(), 1);
    }

    #[cfg(windows)]
    #[test]
    fn first_login_acceptance_holds_profile_lease_against_mobile_update() {
        use std::sync::mpsc;
        use std::thread;

        // 将可能因锁回归而阻塞的线程隔离到子进程，由父测负责限时终止。
        if std::env::var("TRAE_SYNC_OAUTH_MOBILE_LEASE_TEST")
            .ok()
            .as_deref()
            != Some("1")
        {
            use std::process::{Command, Stdio};

            let mut child = Command::new(std::env::current_exe().unwrap())
                .env("TRAE_SYNC_OAUTH_MOBILE_LEASE_TEST", "1")
                .args([
                    "--exact",
                    "checkin_login::tests::first_login_acceptance_holds_profile_lease_against_mobile_update",
                    "--nocapture",
                ])
                .stdin(Stdio::null())
                .stdout(Stdio::inherit())
                .stderr(Stdio::inherit())
                .spawn()
                .expect("启动 OAuth/手机号竞争测试子进程失败");
            let deadline = std::time::Instant::now() + Duration::from_secs(30);
            loop {
                match child.try_wait() {
                    Ok(Some(status)) => {
                        assert!(status.success(), "OAuth/手机号竞争测试子进程失败：{status}");
                        return;
                    }
                    Ok(None) if std::time::Instant::now() < deadline => {
                        thread::sleep(Duration::from_millis(25));
                    }
                    Ok(None) | Err(_) => {
                        let _ = child.kill();
                        let _ = child.wait();
                        panic!("OAuth/手机号竞争测试子进程 30 秒内未结束，已 kill+wait");
                    }
                }
            }
        }

        struct ReleaseAcceptanceOnDrop(mpsc::Sender<()>);
        impl Drop for ReleaseAcceptanceOnDrop {
            fn drop(&mut self) {
                let _ = self.0.send(());
            }
        }

        let root = tempfile::tempdir().unwrap();
        let store = CheckinCredentialStore::new(root.path());
        let registry = AccountRegistry::new(root.path());
        let account_id = "1800000000000101";
        let (old_private, old_public) = generate_device_keypair().unwrap();
        let old_receipt = persist_login_result(
            "profile-mobile-barrier",
            "device-old-1234",
            "machine-old",
            &old_public,
            &old_private,
            acceptance_grant(account_id, "refresh-before-oauth"),
            &UserInfoSummary::default(),
            &store,
            &registry,
        )
        .unwrap();
        let old_record = registry.find(&old_receipt.profile_id).unwrap().unwrap();
        let old_binding = CheckinProfileBinding::from(&old_record);
        store
            .set_mobile_full(&old_binding, Some("13800000000"))
            .unwrap();
        let credential_path = std::fs::read_dir(store.root())
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .find(|path| path.extension() == Some(std::ffi::OsStr::new("checkin")))
            .expect("已存在账号应有一个真实凭据包");

        let (new_private, new_public) = generate_device_keypair().unwrap();
        let new_binding = CheckinProfileBinding::new(
            &old_receipt.profile_id,
            account_id,
            "device-new-1234",
            &new_public,
        );
        let (acceptance_entered_tx, acceptance_entered_rx) = mpsc::channel();
        let (release_acceptance_tx, release_acceptance_rx) = mpsc::channel();
        let release_acceptance = ReleaseAcceptanceOnDrop(release_acceptance_tx);
        let (login_done_tx, login_done_rx) = mpsc::channel();
        let (mobile_started_tx, mobile_started_rx) = mpsc::channel();
        let (mobile_done_tx, mobile_done_rx) = mpsc::channel();

        let login_store = store.clone();
        let login_registry = AccountRegistry::new(root.path());
        let probe_path = credential_path.clone();
        let login = thread::spawn(move || {
            let attempt = LoginAttemptControl::new();
            let _ = attempt.begin_exchange();
            let result = persist_and_accept_login_inner(
                &attempt,
                LoginProduct::WorkCn,
                "oauth-session-mobile",
                "device-new-1234",
                "machine-new",
                &new_public,
                &new_private,
                acceptance_grant(account_id, "refresh-after-oauth"),
                &UserInfoSummary::default(),
                &login_store,
                &login_registry,
                &|binding, _now, _lease| {
                    let profile_lock_probe =
                        FileOperationLock::try_acquire_sibling_resource(&probe_path)
                            .map_err(|_| CheckinCredentialError::Unavailable)?;
                    let profile_lock_is_busy = profile_lock_probe.is_none();
                    drop(profile_lock_probe);
                    acceptance_entered_tx
                        .send((binding.profile_id.clone(), profile_lock_is_busy))
                        .map_err(|_| CheckinCredentialError::CredentialRefreshFailed)?;
                    release_acceptance_rx
                        .recv_timeout(Duration::from_secs(5))
                        .map(|_| ())
                        .map_err(|_| CheckinCredentialError::CredentialRefreshFailed)
                },
                &|_| {},
            );
            let _ = login_done_tx.send(result);
        });

        let acceptance_profile = acceptance_entered_rx.recv_timeout(Duration::from_secs(5));
        let mobile_store = store.clone();
        let mobile_binding = new_binding.clone();
        let mobile_writer = thread::spawn(move || {
            let _ = mobile_started_tx.send(());
            let result = mobile_store.set_mobile_full(&mobile_binding, Some("13812345678"));
            let _ = mobile_done_tx.send(result);
        });
        let mobile_started = mobile_started_rx
            .recv_timeout(Duration::from_secs(5))
            .is_ok();
        let mobile_done_before_release =
            mobile_done_rx.recv_timeout(Duration::from_millis(300)).ok();
        let mobile_finished_before_release = mobile_done_before_release.is_some();

        // 即使后续断言失败，Drop 也会释放验收 gate，避免遗留阻塞线程。
        drop(release_acceptance);
        let login_result = login_done_rx.recv_timeout(Duration::from_secs(5));
        let mobile_result = mobile_done_before_release
            .or_else(|| mobile_done_rx.recv_timeout(Duration::from_secs(5)).ok());
        assert!(login_result.is_ok(), "OAuth caller 未在 gate 释放后完成");
        assert!(
            mobile_result.is_some(),
            "手机号 writer 未在 gate 释放后完成"
        );
        let login_joined = login.join().is_ok();
        let mobile_writer_joined = mobile_writer.join().is_ok();

        assert_eq!(
            acceptance_profile
                .as_ref()
                .ok()
                .map(|(profile_id, _)| profile_id.as_str()),
            Some("profile-mobile-barrier"),
            "验收应复用既有账号凭据档案"
        );
        assert_eq!(
            acceptance_profile
                .as_ref()
                .ok()
                .map(|(_, profile_lock_is_busy)| *profile_lock_is_busy),
            Some(true),
            "验收期间同一实际凭据资源必须处于租约保护中"
        );
        assert!(mobile_started);
        assert!(!mobile_finished_before_release);
        assert!(login_joined);
        assert!(mobile_writer_joined);
        let receipt = login_result.unwrap().unwrap();
        assert!(receipt.acceptance.passed);
        mobile_result.unwrap().unwrap();

        // 从 DPAPI 凭据包读回，确认 OAuth 新令牌与并发手机号更新同时保留。
        let bundle = store.load(&new_binding).unwrap();
        assert_eq!(bundle.refresh_token, "refresh-after-oauth");
        assert_eq!(bundle.mobile_full.as_deref(), Some("13812345678"));
    }

    #[cfg(windows)]
    #[test]
    fn first_login_acceptance_failure_keeps_candidate_and_reports_code() {
        let root = tempfile::tempdir().unwrap();
        let store = CheckinCredentialStore::new(root.path());
        let registry = AccountRegistry::new(root.path());
        let (private_pem, public_pem) = generate_device_keypair().unwrap();
        let attempt = LoginAttemptControl::new();
        // 回调返回后先进入 Exchanging，符合发布函数的阶段前置条件。
        assert_eq!(attempt.begin_exchange(), Ok(()));
        assert_eq!(attempt.phase(), LoginAttemptPhase::Exchanging);
        let calls = std::cell::Cell::new(0usize);
        let renew =
            |_binding: &CheckinProfileBinding, _now: u64, _lease: &CheckinProfileOperationLease| {
                calls.set(calls.get() + 1);
                Err(CheckinCredentialError::CredentialRefreshFailed)
            };
        let receipt = persist_and_accept_login(
            &attempt,
            LoginProduct::WorkCn,
            "profile-accept",
            "1234567890123456",
            "machine-accept",
            &public_pem,
            &private_pem,
            acceptance_grant("1800000000000101", "refresh-accept"),
            &UserInfoSummary::default(),
            &store,
            &registry,
            &renew,
        )
        .unwrap();
        // 回执如实带失败结果与稳定错误码；不回滚登录。
        assert!(!receipt.acceptance.passed);
        assert_eq!(receipt.acceptance.status(), "failed");
        assert_eq!(
            receipt.acceptance.error_code,
            Some("credential_refresh_failed")
        );
        // 无第二次 OAuth：验收失败不重试、不重铸设备、不重新授权（只换发一次）。
        assert_eq!(calls.get(), 1);
        // 候选凭据保留：仍是登录时签发的代次，未被验收覆盖或清除。
        let binding = CheckinProfileBinding::new(
            "profile-accept",
            "1800000000000101",
            "1234567890123456",
            &public_pem,
        );
        let bundle = store.load(&binding).unwrap();
        assert_eq!(bundle.refresh_token, "refresh-accept");
        assert_eq!(bundle.access_token_expires_at_unix_seconds, 1_800_000_000);
        // 账号已注册（登录不回滚，档案保留）。
        assert!(registry.find("profile-accept").unwrap().is_some());
    }

    #[cfg(windows)]
    #[test]
    fn first_login_acceptance_cancelled_before_publish_skips_renew_and_storage() {
        let root = tempfile::tempdir().unwrap();
        let store = CheckinCredentialStore::new(root.path());
        let registry = AccountRegistry::new(root.path());
        let attempt = LoginAttemptControl::new();
        assert_eq!(attempt.begin_exchange(), Ok(()));
        assert_eq!(attempt.cancel(), LoginCancelResult::Cancelled);
        let calls = std::cell::Cell::new(0usize);
        let renew =
            |_binding: &CheckinProfileBinding, _now: u64, _lease: &CheckinProfileOperationLease| {
                calls.set(calls.get() + 1);
                Ok(())
            };

        // 取消先赢时不得续期，也不得写入凭据包或账号档案。
        let result = persist_and_accept_login(
            &attempt,
            LoginProduct::WorkCn,
            "profile-accept",
            "1234567890123456",
            "machine-accept",
            "public-cancel",
            "private-cancel",
            acceptance_grant("1800000000000101", "refresh-accept"),
            &UserInfoSummary::default(),
            &store,
            &registry,
            &renew,
        );

        assert_eq!(result.err(), Some(LoginError::Cancelled));
        assert_eq!(calls.get(), 0);
        let binding = CheckinProfileBinding::new(
            "profile-accept",
            "1800000000000101",
            "1234567890123456",
            "public-cancel",
        );
        assert!(store.load(&binding).is_err());
        assert!(registry.find("profile-accept").unwrap().is_none());
    }

    // P7-4：等待期间外部中止（用户取消 / 浏览器退出）→ 立即以 Cancelled
    // 终止，不受总超时约束，且不产生任何入库副作用。
    #[test]
    fn complete_login_aborts_immediately_when_requested() {
        let handoff = begin_login("profile-cancel").unwrap();
        let root = tempfile::tempdir().unwrap();
        let store = CheckinCredentialStore::new(root.path());
        let registry = AccountRegistry::new(root.path());
        let client = reqwest::blocking::Client::new();
        let started = Instant::now();
        let result = complete_login(
            handoff.session,
            LoginAttemptControl::new(),
            &store,
            &registry,
            &client,
            // 长超时下必须被 abort 立即打断，而不是等满 60 秒。
            Duration::from_secs(60),
            &|| true,
        );
        assert_eq!(result.err(), Some(LoginError::Cancelled));
        assert!(started.elapsed() < Duration::from_secs(5));
        assert!(registry.load().unwrap().is_empty());
    }
}
