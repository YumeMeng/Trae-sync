//! T17/MVP OAuth 登录流：PKCE + 系统浏览器授权 + 本地回调 + AuthCode 换凭证。
//!
//! 协议来源（09 号文档逆向 + 2026-08-22 按 Trae CN v3.3.74 main.js/product.json 复核）：
//! - 登录页 `https://www.trae.cn/authorization`（`bootConfig.consoleHost`；
//!   旧文档的 `work.trae.cn` 已下线返回 404），参数含 PKCE challenge 与
//!   `auth_callback_url=http://127.0.0.1:<port>/authorize`；
//! - 回调 query 的 `authCodeInfo` 为 URL 编码 JSON（含一次性 `AuthCode`）；
//! - `ExchangeToken` AuthCode 模式免签名换取 Token(14 天)/RefreshToken(180 天)；
//! - 账号 ID 从 JWT payload `data.id` 解出，`GetUserInfo` 补全脱敏资料。
//!
//! 每个账号绑定持久虚拟设备（随机 16 位 device_id + 随机 machine_id +
//! EC P-256 密钥对，ADR-0019），随 `ExchangeToken` 注册后与账号永久绑定；
//! 设备闸门与配额规则见 `checkin_http` 模块注释。
//!
//! 形态选择（2026-09-02）：SOLO 与 Work 为同一产品的改名，服务端按
//! client_id 通道分别计设备配额；登录固定走 SOLO 通道（en1oxy7wnw8j9n）
//! ——该通道配额未被历史测试耗尽，且 SOLO 形态设备可直接首签，无需登录后
//! 再重铸。后续如通道可用性变化，按「能用的优先」原则切换，不绑定产品名。

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use openssl::rand::rand_bytes;
use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::account_registry::{AccountRecord, AccountRegistry};
use crate::checkin_credential::{
    generate_device_keypair, CheckinCredentialBundle, CheckinProfileBinding,
    CheckinCredentialStore,
};
use crate::checkin_http::{
    exchange_token_by_auth_code, get_user_info, CheckinHttpError, DeviceInfoBlock, TokenGrant,
    UserInfoSummary, TRAE_SOLO_CLIENT_ID, TRAE_SOLO_IDE_VERSION,
};

/// 登录页地址（Trae CN v3.3.74 `bootConfig.consoleHost` + `/authorization`）。
pub const LOGIN_HOST: &str = "https://www.trae.cn/authorization";
/// 回调等待上限（与客户端 TIMEOUT_MS 同量级）。
pub const CALLBACK_TIMEOUT_SECONDS: u64 = 300;

/// 登录流错误；不携带 Token、AuthCode 或回调正文。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoginError {
    /// 回调参数无效或登录页返回 error_code。
    InvalidCallback,
    /// AuthCode 换取凭证失败（网络/业务码/协议）。
    ExchangeFailed,
    /// 服务端设备数已达上限（业务码 20401）：本地删档无法释放服务端配额，
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
    // 首位取 1-9，避免前导 0 导致长度缩水。
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
            b'%' if index + 2 < bytes.len() + 1 && index + 2 <= bytes.len() - 1 + 1 => {
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
    let params = [
        ("login_version", "1".to_string()),
        ("auth_from", "solo".to_string()),
        // SOLO 通道专属参数（官方客户端行为，缺失会影响授权页登录方式）。
        ("hide_saas_login", "true".to_string()),
        ("login_channel", "native_ide".to_string()),
        ("plugin_version", "local".to_string()),
        ("auth_type", "local".to_string()),
        ("client_id", TRAE_SOLO_CLIENT_ID.to_string()),
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
        ("x_app_version", TRAE_SOLO_IDE_VERSION.to_string()),
        ("x_app_type", "trae".to_string()),
        ("code_challenge", code_challenge.to_string()),
        ("code_challenge_method", "S256".to_string()),
    ];
    let query = params
        .iter()
        .map(|(key, value)| format!("{key}={}", percent_encode(value)))
        .collect::<Vec<_>>()
        .join("&");
    format!("{LOGIN_HOST}?{query}")
}

/// 解析登录回调：成功返回一次性 AuthCode，失败/无效返回 `InvalidCallback`。
pub fn parse_authorize_callback(query: &str) -> Result<String, LoginError> {
    if query_param(query, "error_code").is_some() {
        return Err(LoginError::InvalidCallback);
    }
    let auth_code_info = query_param(query, "authCodeInfo").ok_or(LoginError::InvalidCallback)?;
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
        // 10035）：浏览器「先连接、后发请求」的间隙里立即 read 会失败，且
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

/// 登录成功回执；只含非敏感字段。
pub struct LoginReceipt {
    pub profile_id: String,
    pub account_id: String,
    pub screen_name: String,
    pub avatar_url: String,
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
    let login_url = build_login_url(
        &callback_url,
        &pkce.code_challenge,
        &trace_id,
        &device_id,
        &machine_id,
    );
    Ok(LoginHandoff {
        login_url,
        session: LoginSession {
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
/// 任何失败都不产生入库副作用。
pub fn complete_login(
    session: LoginSession,
    store: &CheckinCredentialStore,
    registry: &AccountRegistry,
    client: &reqwest::blocking::Client,
    timeout: Duration,
    should_abort: &dyn Fn() -> bool,
) -> Result<LoginReceipt, LoginError> {
    // 1. 阻塞等待浏览器回调（listener 存活在 session 中）。
    let query = session.callback.wait_for_callback(timeout, should_abort)?;
    // 2. 解析一次性 AuthCode（含 error_code/结构异常拒绝）。
    let auth_code = parse_authorize_callback(&query)?;
    // 3. 构造虚拟设备信息块（与登录 URL 中声明的设备身份一致）。
    //    登录固定 SOLO 形态（2026-09-02 决策）：SOLO 与 Work 为同一产品
    //    改名，服务端按 client_id 通道分别计设备配额——Work 通道配额易被
    //    耗尽触发 20401，且 SOLO 形态设备可直接首签，无需登录后再重铸
    //    （每次登录的设备注册消耗从 2 个降为 1 个）。
    let oauth_client = crate::checkin_http::OAuthClient::Solo;
    let device_info = oauth_client.device_info(
        &session.device_id,
        &session.machine_id,
        &session.device_public_key_pem,
    );
    // 4. AuthCode 模式换凭证（免 DeviceProof 签名）；设备上限（20401）
    //    单独归类，登录 UI 能给出针对性提示而非笼统的「换取失败」。
    let grant = exchange_token_by_auth_code(
        client,
        &auth_code,
        &session.code_verifier,
        &device_info,
        oauth_client,
    )
    .map_err(|error| {
        // 诊断日志：记录错误类别/码与服务端响应摘要（失败响应体不含
        // Token），用于排查个别账号被服务端拒绝的真实原因。
        append_exchange_diagnostic(store.root(), &session.profile_id, &error);
        match error {
            CheckinHttpError::Business(20401) => LoginError::ExchangeDeviceLimit,
            _ => LoginError::ExchangeFailed,
        }
    })?;
    // 5. GetUserInfo 补脱敏资料；失败降级（账号 ID 已可从 JWT 解出）。
    let user_info = get_user_info(client, &grant.access_token).unwrap_or_default();
    let receipt = persist_login_result(
        &session.profile_id,
        &session.device_id,
        &session.machine_id,
        &session.device_public_key_pem,
        &session.device_private_key_pem,
        grant,
        &user_info,
        store,
        registry,
    )?;
    // 登录即 SOLO 形态设备，可直接用于首签，无需再自动重铸（原 Work 形态
    // 登录后必须重铸 SOLO 设备的步骤已随形态切换取消）。
    Ok(receipt)
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
    if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = file.write_all(line.as_bytes());
    }
}

/// 将换得的凭证与资料写入加密凭据包和账号注册表（`complete_login` 的入库步骤）。
///
/// 独立成函数以便离线测试：跳过 HTTP，直接注入 `TokenGrant` 验证
/// JWT 解码、显示名兜底、凭据包与注册表落盘的完整链路。
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
    // JWT payload `data.id` 即服务端账号 ID；顺带校验 token 结构。
    let (account_id, _) = decode_account_from_jwt(&grant.access_token)?;
    // 同一账号重复登录：复用已有档案的 profile_id 覆盖更新（凭据密文与档案都
    // 原地覆盖），避免同一账号出现多条档案、多个虚拟设备重复领取积分。
    let effective_profile_id = match registry.load() {
        Ok(records) => records
            .iter()
            .find(|record| record.account_id == account_id)
            .map(|record| record.profile_id.clone())
            .unwrap_or_else(|| profile_id.to_string()),
        Err(_) => return Err(LoginError::Storage),
    };
    let now = unix_now();
    // 旧档案先读：重复登录时用于保留既有本地偏好（自动签到开关、备注名、
    // 归档标记、脱敏手机号）与凭据包中已补录的完整手机号（G11）。
    let previous = registry
        .find(&effective_profile_id)
        .map_err(|_| LoginError::Storage)?;
    // 已补录的完整手机号跟随旧凭据包：重复登录覆盖凭据时不丢（用户手工录入
    // 的数据，与新令牌无绑定关系）。读取失败（首次登录/旧包不可用）按未补录。
    let previous_mobile_full = previous
        .as_ref()
        .and_then(|old| {
            store
                .load(&CheckinProfileBinding::new(
                    old.profile_id.clone(),
                    old.account_id.clone(),
                    old.device_id.clone(),
                    old.device_public_key.clone(),
                ))
                .ok()
        })
        .and_then(|bundle| bundle.mobile_full);
    // 6. 凭据包加密入库（DPAPI；敏感字段只在密文中落盘）。
    let bundle = CheckinCredentialBundle {
        profile_id: effective_profile_id.clone(),
        account_id: account_id.clone(),
        device_id: device_id.to_string(),
        machine_id: machine_id.to_string(),
        device_public_key: device_public_key_pem.to_string(),
        device_private_key: device_private_key_pem.to_string(),
        access_token: grant.access_token,
        refresh_token: grant.refresh_token,
        // 与登录/换取所用形态一致（SOLO 通道）；refresh 续期按此还原形态。
        client_id: TRAE_SOLO_CLIENT_ID.to_string(),
        access_token_expires_at_unix_seconds: grant.access_token_expires_at_unix_seconds,
        refresh_token_expires_at_unix_seconds: grant.refresh_token_expires_at_unix_seconds,
        mobile_full: previous_mobile_full,
    };
    store.save(&bundle).map_err(|_| LoginError::Storage)?;
    // 7. 非敏感档案 upsert（profile_id 唯一，重复登录覆盖旧记录；
    //    保留既有自动签到开关、本地备注名与归档标记——均为用户本地偏好；
    //    脱敏手机号取服务端最新值，服务端未返回时保留已采值）。
    let previous_auto_enabled = previous
        .as_ref()
        .map(|old| old.auto_checkin_enabled)
        .unwrap_or(true);
    let previous_display_name = previous.as_ref().and_then(|old| old.display_name.clone());
    let previous_archived = previous.as_ref().map(|old| old.archived).unwrap_or(false);
    let previous_mobile = previous
        .as_ref()
        .map(|old| old.masked_mobile.clone())
        .unwrap_or_default();
    let masked_mobile = if user_info.masked_mobile.is_empty() {
        previous_mobile
    } else {
        user_info.masked_mobile.clone()
    };
    let screen_name = display_name(&account_id, user_info);
    let record = AccountRecord {
        profile_id: effective_profile_id,
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
    Ok(LoginReceipt {
        profile_id: record.profile_id,
        account_id: record.account_id,
        screen_name,
        avatar_url: record.avatar_url,
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
            "http://127.0.0.1:51789/authorize",
            "challenge-1",
            "trace-1",
            "1234567890123456",
            "machine-1",
        );
        assert!(url.starts_with("https://www.trae.cn/authorization?"));
        // auth_from 必须与 SOLO 通道一致，否则 AuthCode 通道绑定错误 → 20403。
        assert!(url.contains("auth_from=solo"));
        assert!(url.contains("hide_saas_login=true"));
        assert!(url.contains("code_challenge_method=S256"));
        assert!(url.contains("code_challenge=challenge-1"));
        // 登录固定 SOLO 形态（同一产品改名，选可用通道；见模块注释）。
        assert!(url.contains("client_id=en1oxy7wnw8j9n"));
        assert!(url.contains("x_app_version=0.1.54"));
        // 回调 URL 必须整体编码（: 与 / 都转义）。
        assert!(url.contains("auth_callback_url=http%3A%2F%2F127.0.0.1%3A51789%2Fauthorize"));
        assert!(url.contains("device_id=1234567890123456"));
        assert!(url.contains("x_device_id=1234567890123456"));
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
            "data": {"id": "1804778984702451", "type": "user"},
            "exp": 1788494618,
            "iat": 1787285060
        });
        let payload_b64 = URL_SAFE_NO_PAD.encode(payload.to_string());
        let token = format!("eyJhbGciOiJSUzI1NiJ9.{payload_b64}.signature");
        let (account_id, expires_at) = decode_account_from_jwt(&token).unwrap();
        assert_eq!(account_id, "1804778984702451");
        assert_eq!(expires_at, 1788494618);
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
        assert!(trace.starts_with(trace.chars().take(14).collect::<String>().as_str()));
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
        let payload = serde_json::json!({"data": {"id": "1804778984702451"}, "exp": 1_800_000_000});
        let payload_b64 = URL_SAFE_NO_PAD.encode(payload.to_string());
        let access_token = format!("eyJhbGciOiJSUzI1NiJ9.{payload_b64}.sig");
        let grant = TokenGrant {
            access_token,
            refresh_token: "refresh-1".to_string(),
            access_token_expires_at_unix_seconds: 1_800_000_000,
            refresh_token_expires_at_unix_seconds: 1_800_100_000,
        };
        let user_info = UserInfoSummary {
            screen_name: "测试用户".to_string(),
            avatar_url: "https://example.test/a.png".to_string(),
            masked_mobile: "138****0000".to_string(),
        };
        let receipt = persist_login_result(
            "profile-login",
            "1234567890123456",
            "machine-login-1",
            &public_pem,
            &private_pem,
            grant,
            &user_info,
            &store,
            &registry,
        )
        .unwrap();
        assert_eq!(receipt.profile_id, "profile-login");
        assert_eq!(receipt.account_id, "1804778984702451");
        assert_eq!(receipt.screen_name, "测试用户");
        // 非敏感档案落盘并可读回。
        let record = registry.find("profile-login").unwrap().unwrap();
        assert_eq!(record.account_id, "1804778984702451");
        assert_eq!(record.device_id, "1234567890123456");
        // U-1：登录时自动采集脱敏手机号。
        assert_eq!(record.masked_mobile, "138****0000");
        // 凭据包可用匹配绑定解密读回（绑定校验通过即结构完整）。
        let binding = crate::checkin_credential::CheckinProfileBinding::new(
            "profile-login",
            "1804778984702451",
            "1234567890123456",
            &public_pem,
        );
        let bundle = store.load(&binding).unwrap();
        assert_eq!(bundle.refresh_token, "refresh-1");
        assert_eq!(bundle.access_token_expires_at_unix_seconds, 1_800_000_000);
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
        let payload = serde_json::json!({"data": {"id": "1804778984702451"}, "exp": 1_800_000_000});
        let payload_b64 = URL_SAFE_NO_PAD.encode(payload.to_string());
        let access_token = format!("eyJhbGciOiJSUzI1NiJ9.{payload_b64}.sig");
        let user_info = UserInfoSummary::default();
        let grant = TokenGrant {
            access_token,
            refresh_token: "refresh-1".to_string(),
            access_token_expires_at_unix_seconds: 1_800_000_000,
            refresh_token_expires_at_unix_seconds: 1_800_100_000,
        };
        let first = persist_login_result(
            "profile-first",
            "1234567890123456",
            "machine-1",
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
            "6543210987654321",
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
        assert_eq!(records[0].device_id, "6543210987654321");
    }

    #[cfg(windows)]
    #[test]
    fn persist_login_result_preserves_alias_and_mobile() {
        // U-1：本地备注名与已采手机号跨重登录保留——两者均为本地
        // 状态，服务端覆盖更新（screen_name/device 等）不得清空它们。
        let root = tempfile::tempdir().unwrap();
        let store = CheckinCredentialStore::new(root.path());
        let registry = AccountRegistry::new(root.path());
        let (private_pem, public_pem) = generate_device_keypair().unwrap();
        let payload = serde_json::json!({"data": {"id": "1804778984702451"}, "exp": 1_800_000_000});
        let payload_b64 = URL_SAFE_NO_PAD.encode(payload.to_string());
        let grant = |token: &str| TokenGrant {
            access_token: format!("eyJhbGciOiJSUzI1NiJ9.{payload_b64}.{token}"),
            refresh_token: format!("refresh-{token}"),
            access_token_expires_at_unix_seconds: 1_800_000_000,
            refresh_token_expires_at_unix_seconds: 1_800_100_000,
        };
        let user_info = UserInfoSummary {
            screen_name: "用户92431183708".to_string(),
            avatar_url: String::new(),
            masked_mobile: String::new(),
        };
        persist_login_result(
            "profile-a",
            "1234567890123456",
            "machine-1",
            &public_pem,
            &private_pem,
            grant("t1"),
            &user_info,
            &store,
            &registry,
        )
        .unwrap();
        // 用户设置本地备注名 + 健康检测补采手机号（服务端未在登录时返回）。
        registry.set_display_name("profile-a", Some("主力号")).unwrap();
        registry.backfill_masked_mobile("profile-a", "138****0000").unwrap();
        // 重新登录（服务端这次返回了手机号但没返回备注名——备注名本就纯本地）。
        let user_info_again = UserInfoSummary {
            masked_mobile: "138****0000".to_string(),
            ..user_info.clone()
        };
        persist_login_result(
            "profile-a",
            "6543210987654321",
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
        assert_eq!(display_name("1804778984702451", &with_name), "张三");
        // ScreenName 缺失时退到脱敏手机号。
        let mobile_only = UserInfoSummary {
            screen_name: String::new(),
            masked_mobile: "138****0000".to_string(),
            avatar_url: String::new(),
        };
        assert_eq!(
            display_name("1804778984702451", &mobile_only),
            "138****0000"
        );
        // 全部缺失时用账号 ID 后 4 位兜底。
        assert_eq!(
            display_name("1804778984702451", &UserInfoSummary::default()),
            "账号 2451"
        );
    }

    #[test]
    fn callback_server_receives_authorize_query() {
        let server = LoginCallbackServer::start().unwrap();
        let url = server.callback_url();
        let expected_query = "authCodeInfo=%7B%22AuthCode%22%3A%22code-1%22%7D";
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

    // 回归（2026-08-27 实发故障）：重放用户真实回调（原样 1412 字符 query +
    // 浏览器请求头 + 「连接后延迟发送」时序）。Windows 上 accept 出的连接
    // 继承 listener 非阻塞模式，若 read_request 未切回阻塞模式，会在此
    // 竞态窗口读出 WouldBlock(10035) 并把登录误判为 InvalidCallback。
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
        let query = "isRedirect=true&scope=trae&authCodeInfo=%7B%22AuthCode%22%3A%22vWZsaLleIoA68mQrNQcTzD-ocPIdy9QY5BWzZNVfDbk%22%2C%22ExpireAt%22%3A1787840551747%2C%22ExpireDuration%22%3A600000%7D&loginTraceID=b79ef0b1-83f8-4026-95fd-dc72d667618f&host=https%3A%2F%2Fapi.trae.com.cn&userRegion=cn&userInfo=%7B%22AIRegion%22%3A%22CN%22%2C%22AuditInfo%22%3A%22%7B%5C%22audit_status%5C%22%3A2%2C%5C%22is_auditing%5C%22%3Afalse%2C%5C%22last_modify_time%5C%22%3A1787674673%2C%5C%22unpass_reason%5C%22%3A%5C%22%5C%22%7D%22%2C%22AvatarUrl%22%3A%22https%3A%2F%2Fp3-passport.byteacctimg.com%2Fimg%2Fuser-avatar%2Fassets%2F220a6b1e6f80eb46fbcfda18057c4447_192_192.png%7E128x128.image%22%2C%22Description%22%3A%22%22%2C%22Gender%22%3A%220%22%2C%22LastLoginTime%22%3A%222026-08-27T22%3A12%3A30%2B08%3A00%22%2C%22LastLoginType%22%3A%22sms%22%2C%22MigrateToSG%22%3Afalse%2C%22NonPlainTextEmail%22%3A%22%22%2C%22NonPlainTextMobile%22%3A%22155******03%22%2C%22Region%22%3A%22CN%22%2C%22RegisterTime%22%3A%222026-08-26T00%3A16%3A36.469%2B08%3A00%22%2C%22ScreenName%22%3A%22%E6%9D%8E%E9%80%B8%E6%99%A8%22%2C%22TenantID%22%3A%227o2d894p7dr0o4%22%2C%22UserID%22%3A%221167031637126768%22%2C%22UtmInfo%22%3A%7B%22ActivityID%22%3A%22%22%2C%22ActivityName%22%3A%22%22%2C%22Campaign%22%3A%22%22%2C%22Content%22%3A%22%22%2C%22Medium%22%3A%22%22%2C%22PromotionChannel%22%3A%22%22%2C%22Source%22%3A%22traework_client_account_page%22%2C%22Term%22%3A%22%22%7D%7D";
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
            &store,
            &registry,
            &client,
            Duration::ZERO,
            &|| false,
        );
        assert_eq!(result.err(), Some(LoginError::InvalidCallback));
        assert!(registry.load().unwrap().is_empty());
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
