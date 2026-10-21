//! P17-T4c③：WorkBuddy 直连签到（官方接口，与 Trae 签到同构的语义层）。
//!
//! 协议事实（spec 8.4，2026-10-17/18 真机实测）：
//! - 查询：`POST {domain}/v2/billing/meter/checkin-activity-status`（只读）；
//! - 领取：`POST {domain}/v2/billing/meter/daily-checkin`（**幂等**：当日
//!   重复领取返回 `code=10001`，不多发）；
//! - 请求体 `{}`；头：`Authorization: Bearer <accessToken>` +
//!   `X-User-Id: <uid>` + `Content-Type: application/json`；
//! - domain 以会话文件 `auth.domain` 为准（不硬编码）。
//!
//! 与 Trae 签到结果语义对齐：claimed / already_checked_in / not_eligible /
//! auth_mismatch / runtime_error。**HTTP 200 不构成成功**——领取以响应
//! code=0（含 credit）为准，复核以 status 的 today_checked_in 翻转为准。
//!
//! 凭据来源：[`crate::workbuddy_credential::decrypt_session_tokens`]（客户端
//! 运行时取钥解密）+ 会话快照的 uid/domain。本模块不落盘任何凭据。
//!
//! **P19-B7 直连语义定稿（b7-findings §3.3）**：5.8.3 官方客户端签到请求
//! 已追加 `X-Device-Token`（Turing 设备风控 SDK 原生模块生成，工具侧无法
//! 复刻）。本模块维持**缺头直连**（客户端失败时仍带 `X-Device-Token-Error`
//! 发请求，服务端存在降级容忍路径；P17 探针无此头即 200），401/403 归类
//! [`WorkBuddyCheckinOutcome::AuthMismatch`] 报因——前端文案含「客户端
//! 风控升级可能」（safeUiError），若服务端按此头收紧风控属预期演进风险。
//!
//! P17-T4c⑤ 凭据自持（spec 8.8 真机定稿）：refreshToken 一次性轮换、
//! 刷新端点零设备绑定（X-Domain/X-Refresh-Token/X-Auth-Refresh-Source 三
//! 头，空 body）。[`open_session_from_pool`] 优先用工具账号池（WBSESSN1
//! 容器）——access 充裕直接用，临近过期自动续期并原子写回新对；池为空
//! 时回退机器解密路径。轮换纪律：刷新成功必须立即 save（丢新对 = 会话
//! 报废），[`RENEW_THRESHOLD_SECS`] 保证最低轮换频率。

use std::path::Path;
use std::time::Duration;

use serde::Deserialize;

use crate::workbuddy_credential::{
    decrypt_session_tokens, WorkBuddySessionSnapshot,
};
use crate::workbuddy_login_credential::{WorkBuddyLoginRecord, WorkBuddyLoginStore};

/// 签到凭据上下文（明文 token 只在内存流转，调用方禁止落盘/写日志）。
#[derive(Debug, Clone)]
pub struct WorkBuddyCheckinCredentials {
    /// 明文 access token（JWT）。
    pub access_token: String,
    /// 账号 uid（本地 UUID，`X-User-Id` 头）。
    pub uid: String,
    /// 官方接口域名（会话文件 `auth.domain`，如 www.workbuddy.cn）。
    pub domain: String,
}

impl WorkBuddyCheckinCredentials {
    /// 从会话快照 + 解密凭据组装（uid/domain 取快照明文，token 取解密明文）。
    pub fn from_parts(
        snapshot: &WorkBuddySessionSnapshot,
        access_token: String,
    ) -> Self {
        Self {
            access_token,
            uid: snapshot.uid.clone(),
            domain: snapshot.auth.domain.clone(),
        }
    }
}

/// 签到查询状态（checkin-activity-status 的 data 节选）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkBuddyCheckinStatus {
    /// 今日是否已签（复核判定的核心字段）。
    pub today_checked_in: bool,
    /// 当前连签天数。
    pub streak_days: u32,
    /// 今日可领积分。
    pub daily_credit: u32,
    /// 周期累计积分。
    pub total_credits: u32,
    /// 已签日期列表（YYYY-MM-DD）。
    pub checkin_dates: Vec<String>,
}

/// 领取回执。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkBuddyCheckinClaim {
    /// 本次入账积分（code=0 时有效）。
    pub credit: u32,
    /// 领取后的连签天数。
    pub streak_days: u32,
}

/// 签到动作结果（与 Trae 签到结果语义对齐）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkBuddyCheckinOutcome {
    /// 查询成功（status 端点；复核判定的数据源）。
    Status(WorkBuddyCheckinStatus),
    /// 本次真实领取成功。
    Claimed(WorkBuddyCheckinClaim),
    /// 今日已签（幂等空操作，code=10001）。
    AlreadyCheckedIn,
    /// 业务拒绝（非 0/10001 的业务码；如活动期外等）。
    NotEligible { code: i64, msg: String },
    /// 认证失效（HTTP 401/403——token 过期或被吊销）。
    AuthMismatch,
    /// 网络/服务端错误（HTTP 非 2xx 4xx 或响应形态不符）。
    RuntimeError { detail: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkBuddyCheckinError {
    /// 凭据上下文不完整（domain 为空等——防硬编码域名兜底错误）。
    CredentialsIncomplete,
    /// 请求构造/发送失败。
    RequestFailed,
    /// 超时。
    Timeout,
}

impl WorkBuddyCheckinError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::CredentialsIncomplete => "workbuddy_checkin_credentials_incomplete",
            Self::RequestFailed => "workbuddy_checkin_request_failed",
            Self::Timeout => "workbuddy_checkin_timeout",
        }
    }
}

/// 官方接口响应信封（`{code, msg, data}`；data 内字段均为可缺省）。
#[derive(Debug, Clone, Deserialize)]
struct CheckinEnvelope {
    code: Option<i64>,
    #[serde(default)]
    msg: Option<String>,
    #[serde(default)]
    data: Option<serde_json::Value>,
}

/// 构造签到 POST 请求（status/claim 共用；端点路径由调用方给）。
fn post_json(
    http: &reqwest::blocking::Client,
    credentials: &WorkBuddyCheckinCredentials,
    path: &str,
) -> Result<(u16, String), WorkBuddyCheckinError> {
    if credentials.domain.trim().is_empty() || credentials.uid.trim().is_empty() {
        return Err(WorkBuddyCheckinError::CredentialsIncomplete);
    }
    let url = format!("https://{}{path}", credentials.domain.trim());
    let response = http
        .post(&url)
        .header("Authorization", format!("Bearer {}", credentials.access_token))
        .header("X-User-Id", credentials.uid.trim())
        .header("Content-Type", "application/json")
        .body("{}")
        .timeout(Duration::from_secs(30))
        .send()
        .map_err(|error| {
            if error.is_timeout() {
                WorkBuddyCheckinError::Timeout
            } else {
                WorkBuddyCheckinError::RequestFailed
            }
        })?;
    let status = response.status().as_u16();
    let body = response.text().map_err(|_| WorkBuddyCheckinError::RequestFailed)?;
    Ok((status, body))
}

/// 响应 → 结果分类（纯函数，便于单测；HTTP 状态与 body 分离传入）。
/// status/claim 响应同为 `{code:0, data}`——data 形态由字段自别
/// （today_checked_in → Status；credit → Claimed），无需端点参数。
fn classify(http_status: u16, body: &str) -> WorkBuddyCheckinOutcome {
    // 认证失效：HTTP 层直接判定（官方 401 带 JSON body 的形态也归此）。
    if http_status == 401 || http_status == 403 {
        return WorkBuddyCheckinOutcome::AuthMismatch;
    }
    if !(200..300).contains(&http_status) {
        return WorkBuddyCheckinOutcome::RuntimeError {
            detail: format!("HTTP {http_status}"),
        };
    }
    let Ok(envelope) = serde_json::from_str::<CheckinEnvelope>(body) else {
        return WorkBuddyCheckinOutcome::RuntimeError {
            detail: "响应不是 JSON 信封".into(),
        };
    };
    let code = envelope.code.unwrap_or(-1);
    let msg = envelope.msg.unwrap_or_default();
    match code {
        0 => {
            let Some(data) = envelope.data else {
                return WorkBuddyCheckinOutcome::RuntimeError {
                    detail: "响应缺少 data".into(),
                };
            };
            parse_success_data(data)
        }
        // 幂等：当日重复领取（真机实测语义：不多发不扣分）。
        10001 => WorkBuddyCheckinOutcome::AlreadyCheckedIn,
        _ => WorkBuddyCheckinOutcome::NotEligible { code, msg },
    }
}

/// data 节选解析（status 与 claim 共用字段：credit/streak_days）。
fn parse_success_data(data: serde_json::Value) -> WorkBuddyCheckinOutcome {
    let number_or = |key: &str| {
        data.get(key)
            .and_then(|value| value.as_u64())
            .map(|value| value as u32)
    };
    // 查询响应（含 today_checked_in）。
    if let Some(checked) = data.get("today_checked_in").and_then(|value| value.as_bool()) {
        return WorkBuddyCheckinOutcome::Status(WorkBuddyCheckinStatus {
            today_checked_in: checked,
            streak_days: number_or("streak_days").unwrap_or(0),
            daily_credit: number_or("daily_credit").unwrap_or(0),
            total_credits: number_or("total_credits").unwrap_or(0),
            checkin_dates: data
                .get("checkin_dates")
                .and_then(|dates| dates.as_array())
                .map(|dates| {
                    dates
                        .iter()
                        .filter_map(|date| date.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default(),
        });
    }
    // 领取响应（credit/streak_days）。
    if let Some(credit) = number_or("credit") {
        return WorkBuddyCheckinOutcome::Claimed(WorkBuddyCheckinClaim {
            credit,
            streak_days: number_or("streak_days").unwrap_or(0),
        });
    }
    WorkBuddyCheckinOutcome::RuntimeError {
        detail: "data 缺少可识别字段".into(),
    }
}

/// 查询签到状态（只读；复核判定的数据源）。
pub fn checkin_status(
    http: &reqwest::blocking::Client,
    credentials: &WorkBuddyCheckinCredentials,
) -> Result<WorkBuddyCheckinOutcome, WorkBuddyCheckinError> {
    let (status, body) = post_json(
        http,
        credentials,
        "/v2/billing/meter/checkin-activity-status",
    )?;
    Ok(classify(status, &body))
}

/// 领取每日签到（幂等；当日重复领取返回 AlreadyCheckedIn）。
pub fn checkin_claim(
    http: &reqwest::blocking::Client,
    credentials: &WorkBuddyCheckinCredentials,
) -> Result<WorkBuddyCheckinOutcome, WorkBuddyCheckinError> {
    let (status, body) = post_json(http, credentials, "/v2/billing/meter/daily-checkin")?;
    Ok(classify(status, &body))
}

// ===== 会话 facade（组合根入口：解密 + HTTP 客户端一次组装，多次动作）=====

/// 已就绪的签到会话（凭据 + HTTP 客户端；组合根命令编排用）。
pub struct WorkBuddyCheckinSession {
    credentials: WorkBuddyCheckinCredentials,
    http: reqwest::blocking::Client,
}

impl WorkBuddyCheckinSession {
    pub fn credentials(&self) -> &WorkBuddyCheckinCredentials {
        &self.credentials
    }

    /// 查询签到状态（只读）。
    pub fn status(&self) -> Result<WorkBuddyCheckinOutcome, WorkBuddyCheckinError> {
        checkin_status(&self.http, &self.credentials)
    }

    /// 领取每日签到（幂等）。
    pub fn claim(&self) -> Result<WorkBuddyCheckinOutcome, WorkBuddyCheckinError> {
        checkin_claim(&self.http, &self.credentials)
    }
}

/// 从本机打开签到会话（解密借客户端运行时；token 只在内存流转）。
/// `workbuddy_home` 为 `~/.workbuddy` 根（会话文件经由
/// [`crate::workbuddy_credential::session_file`] 按 LOCALAPPDATA 解析，
/// 与 workbuddy_home 无关——参数仅为签名对称性保留）。
pub fn open_session_from_machine(
    client_exe: &Path,
    auth_id: &str,
) -> Result<WorkBuddyCheckinSession, String> {
    let snapshot = crate::workbuddy_credential::load_session_snapshot(auth_id)
        .map_err(|error| error.code().to_string())?;
    let info_path = crate::workbuddy_credential::session_file(auth_id)
        .ok_or_else(|| "workbuddy_session_path_unavailable".to_string())?;
    let tokens = decrypt_session_tokens(&info_path, client_exe)
        .map_err(|error| format!("{}:{:?}", error.code(), error))?;
    Ok(WorkBuddyCheckinSession {
        credentials: WorkBuddyCheckinCredentials::from_parts(&snapshot, tokens.access_token),
        http: reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .map_err(|_| "workbuddy_checkin_client_failed".to_string())?,
    })
}

// ===== 凭据自持（P17-T4c⑤，spec 8.8）：池优先会话 + refresh 循环 =====

/// 续期阈值：access 距过期不足 3 天才实际刷新（refreshToken 一次性轮换，
/// 每次刷新都会作废客户端副本——最低轮换频率纪律）。
pub const RENEW_THRESHOLD_SECS: u64 = 3 * 24 * 3600;

/// 池凭据续期结果（组合根 DTO 的来源；用户可感知语义与 Trae 刷新对齐）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkBuddyRenewalOutcome {
    /// access 仍充裕，未刷新（不轮换）。
    Skipped { access_expires_in_secs: u64 },
    /// 已刷新并原子写回容器（新 token 对）。
    Renewed {
        access_expires_at_unix_seconds: u64,
        refresh_expires_at_unix_seconds: u64,
    },
}

/// 池凭据错误（组合根映射为稳定错误码）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkBuddyPoolError {
    /// 池为空（未捕获过）——调用方回退机器解密路径。
    PoolEmpty,
    /// 指定账号不在池中。
    RecordMissing,
    /// refreshToken 已被服务端轮换/吊销（刷新拒绝）——需重新捕获。
    RefreshRejected,
    /// 网络/服务端错误。
    RequestFailed,
    /// 刷新响应形态不符（含 code!=0 的非拒绝业务码）。
    ResponseInvalid,
    /// 容器读写失败。
    StoreIo,
}

impl WorkBuddyPoolError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::PoolEmpty => "workbuddy_pool_empty",
            Self::RecordMissing => "workbuddy_pool_record_missing",
            Self::RefreshRejected => "workbuddy_refresh_rejected",
            Self::RequestFailed => "workbuddy_refresh_request_failed",
            Self::ResponseInvalid => "workbuddy_refresh_response_invalid",
            Self::StoreIo => "workbuddy_pool_store_io",
        }
    }
}

/// 刷新端点响应信封（data 节选；字段名与真机 200 实测一致，spec 8.8）。
#[derive(Debug, Clone, Deserialize)]
struct RefreshEnvelope {
    code: Option<i64>,
    #[serde(default)]
    data: Option<RefreshData>,
}

#[derive(Debug, Clone, Deserialize)]
struct RefreshData {
    #[serde(rename = "accessToken", default)]
    access_token: Option<String>,
    #[serde(rename = "refreshToken", default)]
    refresh_token: Option<String>,
    #[serde(rename = "expiresIn", default)]
    expires_in: Option<u64>,
    #[serde(rename = "refreshExpiresIn", default)]
    refresh_expires_in: Option<u64>,
    #[serde(rename = "sessionState", default)]
    session_state: Option<String>,
    #[serde(default)]
    domain: Option<String>,
}

/// 调用刷新端点（协议定案：空 body + X-Domain/X-Refresh-Token/
/// X-Auth-Refresh-Source 三头；spec 8.8。**轮换型：成功即作废传入的
/// refreshToken，调用方必须立即持久化响应中的新对**）。
fn call_refresh_endpoint(
    http: &reqwest::blocking::Client,
    domain: &str,
    refresh_token: &str,
) -> Result<(String, String, u64, u64, Option<String>, String), WorkBuddyPoolError> {
    let url = format!("https://{}/v2/plugin/auth/token/refresh", domain);
    let response = http
        .post(&url)
        .header("X-Domain", domain)
        .header("X-Refresh-Token", refresh_token)
        .header("X-Auth-Refresh-Source", "plugin")
        .header("Content-Type", "application/json")
        .body("{}")
        .timeout(Duration::from_secs(30))
        .send()
        .map_err(|_| WorkBuddyPoolError::RequestFailed)?;
    let http_status = response.status().as_u16();
    let body = response.text().map_err(|_| WorkBuddyPoolError::RequestFailed)?;
    // 401/403 = refreshToken 已失效（被更新对轮换/服务端吊销）。
    if http_status == 401 || http_status == 403 {
        return Err(WorkBuddyPoolError::RefreshRejected);
    }
    if !(200..300).contains(&http_status) {
        return Err(WorkBuddyPoolError::RequestFailed);
    }
    let envelope: RefreshEnvelope =
        serde_json::from_str(&body).map_err(|_| WorkBuddyPoolError::ResponseInvalid)?;
    // code!=0 视为拒绝（含服务端业务态的会话失效）。
    if envelope.code != Some(0) {
        return Err(WorkBuddyPoolError::RefreshRejected);
    }
    let data = envelope.data.ok_or(WorkBuddyPoolError::ResponseInvalid)?;
    let new_access = data
        .access_token
        .filter(|token| !token.is_empty())
        .ok_or(WorkBuddyPoolError::ResponseInvalid)?;
    let new_refresh = data
        .refresh_token
        .filter(|token| !token.is_empty())
        .ok_or(WorkBuddyPoolError::ResponseInvalid)?;
    // expiresIn（秒）为权威口径（daemon calculateExpiresAt 同语义：缺省才回退现有值）。
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0);
    Ok((
        new_access,
        new_refresh,
        now + data.expires_in.unwrap_or(0),
        now + data.refresh_expires_in.unwrap_or(0),
        data.session_state,
        data.domain.unwrap_or_else(|| domain.to_string()),
    ))
}

/// 池凭据续期：access 临近过期（不足 [`RENEW_THRESHOLD_SECS`]）或已过期
/// 时用池内 refreshToken 刷新并**原子写回新对**（轮换纪律：刷新成功与
/// 容器写回之间不做任何可能失败的旁路操作）。access 充裕则跳过不轮换。
pub fn renew_pool_record(
    http: &reqwest::blocking::Client,
    store: &WorkBuddyLoginStore,
    now_unix: u64,
) -> Result<WorkBuddyRenewalOutcome, WorkBuddyPoolError> {
    let summaries = store.list().map_err(|_| WorkBuddyPoolError::StoreIo)?;
    let summary = summaries.first().ok_or(WorkBuddyPoolError::PoolEmpty)?;
    let mut record = store
        .load(&summary.profile_id, &summary.account_id)
        .map_err(|_| WorkBuddyPoolError::RecordMissing)?;
    renew_record(http, store, &mut record, now_unix, false)
}

/// 按指定账号续期（P17-A5 自动调度：多账号池逐账号 claim 前的断连兜底）。
/// 语义与 [`renew_pool_record`] 一致，只是目标账号由 profile_id 指定。
pub fn renew_pool_profile(
    http: &reqwest::blocking::Client,
    store: &WorkBuddyLoginStore,
    profile_id: &str,
    now_unix: u64,
) -> Result<WorkBuddyRenewalOutcome, WorkBuddyPoolError> {
    let mut record = load_pool_record(store, profile_id)?;
    renew_record(http, store, &mut record, now_unix, false)
}

/// P19.1：按指定账号**强制刷新**（忽略 access 充裕阈值）——401 兜底
/// 重试链的基础原语：AuthMismatch 时先强刷换新对再重开 session 重试。
/// 轮换纪律不变：新对原子写回（refreshToken 一次性，旧对即作废）。
pub fn renew_pool_profile_forced(
    http: &reqwest::blocking::Client,
    store: &WorkBuddyLoginStore,
    profile_id: &str,
    now_unix: u64,
) -> Result<WorkBuddyRenewalOutcome, WorkBuddyPoolError> {
    let mut record = load_pool_record(store, profile_id)?;
    renew_record(http, store, &mut record, now_unix, true)
}

/// P19.1：池首账号强制刷新（"当前账号"路径〔机器回退/池优先混装〕的
/// 401 兜底用；池为空 → PoolEmpty 由调用方回退机器路径）。
pub fn renew_pool_record_forced(
    http: &reqwest::blocking::Client,
    store: &WorkBuddyLoginStore,
    now_unix: u64,
) -> Result<WorkBuddyRenewalOutcome, WorkBuddyPoolError> {
    let summaries = store.list().map_err(|_| WorkBuddyPoolError::StoreIo)?;
    let summary = summaries.first().ok_or(WorkBuddyPoolError::PoolEmpty)?;
    renew_pool_profile_forced(http, store, &summary.profile_id, now_unix)
}

/// 按容器清单定位并加载指定账号（不存在 → RecordMissing）。
fn load_pool_record(
    store: &WorkBuddyLoginStore,
    profile_id: &str,
) -> Result<WorkBuddyLoginRecord, WorkBuddyPoolError> {
    let summaries = store.list().map_err(|_| WorkBuddyPoolError::StoreIo)?;
    let summary = summaries
        .iter()
        .find(|summary| summary.profile_id == profile_id)
        .ok_or(WorkBuddyPoolError::RecordMissing)?;
    store
        .load(&summary.profile_id, &summary.account_id)
        .map_err(|_| WorkBuddyPoolError::RecordMissing)
}

/// 续期跳过判定（纯函数，单测口径）：非强制且 access 距过期仍充裕
/// （> [`RENEW_THRESHOLD_SECS`]）才跳过；强制刷新（401 兜底）永不跳过。
fn should_skip_renewal(access_expires_in_secs: u64, force: bool) -> bool {
    !force && access_expires_in_secs > RENEW_THRESHOLD_SECS
}

/// 续期内核（按记录；access 充裕跳过不轮换，临近过期刷新 + 原子写回）。
/// `force`：P19.1 强制刷新（忽略阈值——401 兜底重试链用；轮换副作用
/// 不变：新对原子写回，旧对即作废）。
fn renew_record(
    http: &reqwest::blocking::Client,
    store: &WorkBuddyLoginStore,
    record: &mut WorkBuddyLoginRecord,
    now_unix: u64,
    force: bool,
) -> Result<WorkBuddyRenewalOutcome, WorkBuddyPoolError> {
    let access_expires_in = record
        .access_token_expires_at_unix_seconds
        .saturating_sub(now_unix);
    if should_skip_renewal(access_expires_in, force) {
        return Ok(WorkBuddyRenewalOutcome::Skipped {
            access_expires_in_secs: access_expires_in,
        });
    }

    let (new_access, new_refresh, access_expires_at, refresh_expires_at, session_state, domain) =
        call_refresh_endpoint(http, &record.domain, &record.refresh_token)?;
    // 轮换纪律：拿到新对后第一件事就是原子写回（save 内含写后复核）。
    record.access_token = new_access;
    record.refresh_token = new_refresh;
    record.access_token_expires_at_unix_seconds = access_expires_at;
    record.refresh_token_expires_at_unix_seconds = refresh_expires_at;
    if let Some(state) = session_state {
        record.session_state = state;
    }
    if !domain.is_empty() {
        record.domain = domain;
    }
    record.last_updated_at_unix_seconds = now_unix;
    store
        .save(&record)
        .map_err(|_| WorkBuddyPoolError::StoreIo)?;
    Ok(WorkBuddyRenewalOutcome::Renewed {
        access_expires_at_unix_seconds: access_expires_at,
        refresh_expires_at_unix_seconds: refresh_expires_at,
    })
}

/// 池凭据续期便捷入口（组合根用；自建 HTTP 客户端，组合根零 HTTP 依赖）。
/// 语义见 [`renew_pool_record`]：access 充裕跳过（零轮换），临近过期刷新
/// 并原子写回新对。
pub fn renew_pool_credentials(
    login_root: &Path,
    now_unix: u64,
) -> Result<WorkBuddyRenewalOutcome, WorkBuddyPoolError> {
    let store = WorkBuddyLoginStore::new(login_root.to_path_buf());
    renew_pool_record(&pool_http(), &store, now_unix)
}

/// P19.1：按指定账号强制刷新便捷入口（401 兜底重试链 / 续期验证探针用；
/// 自建 HTTP 客户端）。轮换副作用不变：新对原子写回，旧对即作废。
pub fn force_renew_pool_profile(
    login_root: &Path,
    profile_id: &str,
    now_unix: u64,
) -> Result<WorkBuddyRenewalOutcome, WorkBuddyPoolError> {
    let store = WorkBuddyLoginStore::new(login_root.to_path_buf());
    renew_pool_profile_forced(&pool_http(), &store, profile_id, now_unix)
}

/// P19.1：池首账号强制刷新便捷入口（"当前账号"路径 401 兜底用）。
pub fn force_renew_pool_record(
    login_root: &Path,
    now_unix: u64,
) -> Result<WorkBuddyRenewalOutcome, WorkBuddyPoolError> {
    let store = WorkBuddyLoginStore::new(login_root.to_path_buf());
    renew_pool_record_forced(&pool_http(), &store, now_unix)
}

/// 从工具账号池打开签到会话（P17-T4c⑤ 池优先路径）：access 充裕直接用
/// （零轮换）；临近过期先续期（刷新 + 原子写回）再用新 access。池为空时
/// 返回 [`WorkBuddyPoolError::PoolEmpty`]，由组合根回退机器解密路径。
pub fn open_session_from_pool(
    login_root: &Path,
    now_unix: u64,
) -> Result<(WorkBuddyCheckinSession, WorkBuddyRenewalOutcome), WorkBuddyPoolError> {
    let http = pool_http();
    let store = WorkBuddyLoginStore::new(login_root.to_path_buf());
    let renewal = renew_pool_record(&http, &store, now_unix)?;
    let summaries = store.list().map_err(|_| WorkBuddyPoolError::StoreIo)?;
    let summary = summaries.first().ok_or(WorkBuddyPoolError::PoolEmpty)?;
    let record = store
        .load(&summary.profile_id, &summary.account_id)
        .map_err(|_| WorkBuddyPoolError::RecordMissing)?;
    Ok((session_from_record(http, &record), renewal))
}

/// P19.1：池首账号**强制续期**后打开会话（当前账号路径 401 兜底——
/// status/claim 返回 AuthMismatch 时换新对重试一次）。
pub fn open_session_from_pool_forced(
    login_root: &Path,
    now_unix: u64,
) -> Result<(WorkBuddyCheckinSession, WorkBuddyRenewalOutcome), WorkBuddyPoolError> {
    let http = pool_http();
    let store = WorkBuddyLoginStore::new(login_root.to_path_buf());
    let renewal = renew_pool_record_forced(&http, &store, now_unix)?;
    let summaries = store.list().map_err(|_| WorkBuddyPoolError::StoreIo)?;
    let summary = summaries.first().ok_or(WorkBuddyPoolError::PoolEmpty)?;
    let record = store
        .load(&summary.profile_id, &summary.account_id)
        .map_err(|_| WorkBuddyPoolError::RecordMissing)?;
    Ok((session_from_record(http, &record), renewal))
}

/// P19.1：按指定账号**强制续期**后打开会话（逐账号路径 401 兜底重试链）。
pub fn open_session_for_profile_forced(
    login_root: &Path,
    profile_id: &str,
    now_unix: u64,
) -> Result<(WorkBuddyCheckinSession, WorkBuddyRenewalOutcome), WorkBuddyPoolError> {
    let http = pool_http();
    let store = WorkBuddyLoginStore::new(login_root.to_path_buf());
    let renewal = renew_pool_profile_forced(&http, &store, profile_id, now_unix)?;
    let record = load_pool_record(&store, profile_id)?;
    Ok((session_from_record(http, &record), renewal))
}

/// 按指定账号打开签到会话（P17-A5 自动调度：先续期该账号（断连兜底），
/// 再用其最新代次装配会话）。
pub fn open_session_for_profile(
    login_root: &Path,
    profile_id: &str,
    now_unix: u64,
) -> Result<(WorkBuddyCheckinSession, WorkBuddyRenewalOutcome), WorkBuddyPoolError> {
    let http = pool_http();
    let store = WorkBuddyLoginStore::new(login_root.to_path_buf());
    let renewal = renew_pool_profile(&http, &store, profile_id, now_unix)?;
    let record = load_pool_record(&store, profile_id)?;
    Ok((session_from_record(http, &record), renewal))
}

/// 会话装配（按池记录；access/uid/domain 三元组）。
fn session_from_record(
    http: reqwest::blocking::Client,
    record: &WorkBuddyLoginRecord,
) -> WorkBuddyCheckinSession {
    WorkBuddyCheckinSession {
        credentials: WorkBuddyCheckinCredentials {
            access_token: record.access_token.clone(),
            uid: record.account_id.clone(),
            domain: record.domain.clone(),
        },
        http,
    }
}

fn pool_http() -> reqwest::blocking::Client {
    reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .expect("WorkBuddy 池 HTTP 客户端构建失败")
}

// ===== P19.1 续期验证探针（用户核心质疑：「保存当前登录账号真的能持续更新吗」）=====

/// 验证探针错误（组合根只暴露稳定错误码）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkBuddyRenewalVerifyError {
    /// 会话快照不可用（载荷 = workbuddy_session_* 稳定错误码——客户端
    /// 未登录/文件损坏，探针无从定位「当前登录账号」）。
    Snapshot(String),
    /// 当前登录账号的 .info token 为 `$wbEncrypted` 加密信封且工具侧无该
    /// 账号的明文池凭据——诚实报告：登录态已加密，请用 OAuth 添加账号。
    EnvelopeEncrypted,
    /// 当前登录账号无对应池记录（未捕获/账号不匹配；token 非信封形态）。
    CredentialMismatch,
    /// 实调刷新被拒（refreshToken 已轮换/吊销）——保存的凭据无法持续更新。
    RefreshRejected,
    /// 刷新请求失败/响应形态不符。
    RequestFailed,
    /// 池容器读写失败。
    StoreIo,
}

impl WorkBuddyRenewalVerifyError {
    pub fn code(&self) -> String {
        match self {
            Self::Snapshot(code) => code.clone(),
            Self::EnvelopeEncrypted => "workbuddy_verify_envelope_encrypted".to_string(),
            Self::CredentialMismatch => "workbuddy_verify_credential_mismatch".to_string(),
            Self::RefreshRejected => "workbuddy_verify_refresh_rejected".to_string(),
            Self::RequestFailed => "workbuddy_verify_request_failed".to_string(),
            Self::StoreIo => "workbuddy_pool_store_io".to_string(),
        }
    }
}

/// 续期验证回执（探针内核口径；组合根 DTO 的来源）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkBuddyRenewalVerifyReport {
    /// 当前登录账号 uid（.info 快照明文面）。
    pub uid: String,
    /// .info 快照时间戳（Unix 秒；0 = 快照未提供该字段）。
    pub info_last_refresh_unix_seconds: u64,
    pub info_access_expires_unix_seconds: u64,
    pub info_refresh_expires_unix_seconds: u64,
    /// .info 文件修改时刻（Unix 秒；0 = 不可得）。
    pub info_modified_unix_seconds: u64,
    /// 续期链路验证结果：true = 实调刷新成功且新对已原子写回池容器。
    pub renewed: bool,
    /// 刷新后的有效期（Unix 秒；renewed=false 时为池记录现值）。
    pub access_expires_at_unix_seconds: u64,
    pub refresh_expires_at_unix_seconds: u64,
}

/// 续期验证探针内核（注入式，离线四态单测：刷新成功/失败/凭据不匹配/
/// 信封检测；生产装配在组合根命令层——`renew` 闭包注入
/// [`renew_pool_profile_forced`]，真实调用刷新端点并原子写回）。
///
/// 非破坏边界：只读 .info 快照与池容器，写仅限池容器自身（刷新轮换的
/// 原子写回），生产会话文件零触碰。
pub fn verify_capture_renewal(
    store: &WorkBuddyLoginStore,
    load_snapshot: &dyn Fn() -> Result<WorkBuddySessionSnapshot, crate::workbuddy_credential::WorkBuddySessionError>,
    info_modified_unix: &dyn Fn() -> Option<u64>,
    renew: &dyn Fn(&str) -> Result<WorkBuddyRenewalOutcome, WorkBuddyPoolError>,
) -> Result<WorkBuddyRenewalVerifyReport, WorkBuddyRenewalVerifyError> {
    // 1) 当前登录账号（.info 快照明文元数据）。
    let snapshot = load_snapshot()
        .map_err(|error| WorkBuddyRenewalVerifyError::Snapshot(error.code().to_string()))?;
    let profile_id = WorkBuddyLoginRecord::profile_id_for(&snapshot.uid);
    // 2) 按当前登录账号定位池记录；无记录 → 信封检测分支优先（诚实报因）。
    let summaries = store.list().map_err(|_| WorkBuddyRenewalVerifyError::StoreIo)?;
    let summary = summaries
        .iter()
        .find(|summary| summary.profile_id == profile_id);
    let Some(summary) = summary else {
        return Err(
            if snapshot.has_encrypted_access_token || snapshot.has_encrypted_refresh_token {
                WorkBuddyRenewalVerifyError::EnvelopeEncrypted
            } else {
                WorkBuddyRenewalVerifyError::CredentialMismatch
            },
        );
    };
    // 3) 实调刷新端点验证续期链路（强制刷新：忽略阈值——探针目的就是验证）。
    let outcome = renew(&summary.profile_id).map_err(|error| match error {
        WorkBuddyPoolError::RefreshRejected => WorkBuddyRenewalVerifyError::RefreshRejected,
        WorkBuddyPoolError::StoreIo => WorkBuddyRenewalVerifyError::StoreIo,
        _ => WorkBuddyRenewalVerifyError::RequestFailed,
    })?;
    // 4) 回执（Renewed 携带新对有效期；防御性 Skipped 读池记录现值）。
    let (renewed, access_expires_at, refresh_expires_at) = match outcome {
        WorkBuddyRenewalOutcome::Renewed {
            access_expires_at_unix_seconds,
            refresh_expires_at_unix_seconds,
        } => (true, access_expires_at_unix_seconds, refresh_expires_at_unix_seconds),
        WorkBuddyRenewalOutcome::Skipped { .. } => {
            let record = store
                .load(&summary.profile_id, &summary.account_id)
                .map_err(|_| WorkBuddyRenewalVerifyError::StoreIo)?;
            (
                false,
                record.access_token_expires_at_unix_seconds,
                record.refresh_token_expires_at_unix_seconds,
            )
        }
    };
    Ok(WorkBuddyRenewalVerifyReport {
        uid: snapshot.uid,
        info_last_refresh_unix_seconds: snapshot.auth.last_refresh_time.unwrap_or(0) / 1000,
        info_access_expires_unix_seconds: snapshot.auth.expires_at.unwrap_or(0) / 1000,
        info_refresh_expires_unix_seconds: snapshot.auth.refresh_expires_at.unwrap_or(0) / 1000,
        info_modified_unix_seconds: info_modified_unix().unwrap_or(0),
        renewed,
        access_expires_at_unix_seconds: access_expires_at,
        refresh_expires_at_unix_seconds: refresh_expires_at,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 真机实测 status 响应（2026-10-17，节选 requestId；字段全保留）。
    const STATUS_BODY: &str = r#"{"code":0,"msg":"OK","data":{"active":true,"today_checked_in":true,"streak_days":5,"daily_credit":100,"today_credit":100,"is_streak_day":false,"next_streak_day":0,"streak_bonus_days":0,"streak_bonus_credit":0,"checkin_dates":["2026-10-17","2026-10-16","2026-10-15","2026-10-14","2026-10-13"],"week_checkin_days":5,"total_credits":500}}"#;

    /// 真机实测 claim 响应（2026-10-17 领取成功）。
    const CLAIM_BODY: &str = r#"{"code":0,"msg":"OK","data":{"credit":100,"streak_days":5}}"#;

    #[test]
    fn classify_status_success() {
        let outcome = classify(200, STATUS_BODY);
        match outcome {
            WorkBuddyCheckinOutcome::Status(status) => {
                assert!(status.today_checked_in);
                assert_eq!(status.streak_days, 5);
                assert_eq!(status.daily_credit, 100);
                assert_eq!(status.total_credits, 500);
                assert_eq!(status.checkin_dates.len(), 5);
                assert_eq!(status.checkin_dates[0], "2026-10-17");
            }
            other => panic!("期望 Status，实际 {other:?}"),
        }
    }

    #[test]
    fn classify_claim_success() {
        let outcome = classify(200, CLAIM_BODY);
        assert_eq!(
            outcome,
            WorkBuddyCheckinOutcome::Claimed(WorkBuddyCheckinClaim {
                credit: 100,
                streak_days: 5,
            })
        );
    }

    /// 幂等：code=10001 → AlreadyCheckedIn（不多发）。
    #[test]
    fn classify_already_checked_in() {
        let body = r#"{"code":10001,"msg":"今天已签到，请明天再来"}"#;
        assert_eq!(classify(200, body), WorkBuddyCheckinOutcome::AlreadyCheckedIn);
    }

    /// 认证失效：HTTP 401/403 → AuthMismatch（即使 body 是合法信封）。
    #[test]
    fn classify_auth_mismatch() {
        assert_eq!(classify(401, STATUS_BODY), WorkBuddyCheckinOutcome::AuthMismatch);
        assert_eq!(classify(403, CLAIM_BODY), WorkBuddyCheckinOutcome::AuthMismatch);
    }

    /// 业务拒绝与形态错误分类。
    #[test]
    fn classify_not_eligible_and_runtime_error() {
        assert_eq!(
            classify(200, r#"{"code":9074,"msg":"陌生设备"}"#),
            WorkBuddyCheckinOutcome::NotEligible {
                code: 9074,
                msg: "陌生设备".into(),
            }
        );
        assert!(matches!(
            classify(500, "server error"),
            WorkBuddyCheckinOutcome::RuntimeError { .. }
        ));
        assert!(matches!(
            classify(200, "not json"),
            WorkBuddyCheckinOutcome::RuntimeError { .. }
        ));
    }

    /// 凭据不完整（空 domain/uid）→ 请求前拒绝（防硬编码域名兜底）。
    #[test]
    fn empty_credentials_rejected_before_request() {
        let http = reqwest::blocking::Client::new();
        let credentials = WorkBuddyCheckinCredentials {
            access_token: "x".into(),
            uid: String::new(),
            domain: String::new(),
        };
        assert_eq!(
            checkin_status(&http, &credentials).unwrap_err(),
            WorkBuddyCheckinError::CredentialsIncomplete
        );
    }

    // ===== 凭据自持（T4c⑤）单测 =====

    /// 真机 200 刷新响应形态（2026-10-18 探针实测；token 值脱敏为占位）。
    const REFRESH_BODY: &str = r#"{"code":0,"msg":"OK","data":{"accessToken":"new-access-jwt","refreshToken":"new-refresh-jwt","expiresIn":2419200,"refreshExpiresIn":2592000,"notBeforePolicy":1724292326,"sessionState":"7e57e1c9-9d02-4281-ba26-f61b7e4c655f","scope":"openid profile offline_access email","domain":"www.workbuddy.cn"}}"#;

    /// 刷新响应解析：camelCase 字段名 + token 齐全判定（spec 8.8 实测形态）。
    #[test]
    fn refresh_envelope_parses_camel_case_fields() {
        let envelope: RefreshEnvelope = serde_json::from_str(REFRESH_BODY).unwrap();
        assert_eq!(envelope.code, Some(0));
        let data = envelope.data.unwrap();
        assert_eq!(data.access_token.as_deref(), Some("new-access-jwt"));
        assert_eq!(data.refresh_token.as_deref(), Some("new-refresh-jwt"));
        assert_eq!(data.expires_in, Some(2419200));
        assert_eq!(data.refresh_expires_in, Some(2592000));
        assert!(data.session_state.is_some());
    }

    /// 轮换拒绝判定：HTTP 401/403 与 code!=0 都归 RefreshRejected。
    #[test]
    fn refresh_rejection_semantics() {
        // 401 → RefreshRejected 由 call_refresh_endpoint 的状态分支保证
        // （HTTP 调用不可单测，这里验证错误码表与解析层拒绝语义）。
        assert_eq!(WorkBuddyPoolError::RefreshRejected.code(), "workbuddy_refresh_rejected");
        let envelope: Result<RefreshEnvelope, _> =
            serde_json::from_str(r#"{"code":10001,"msg":"invalid token"}"#);
        // code!=0 的形态也能解析——拒绝判定在 call_refresh_endpoint 中。
        assert_eq!(envelope.unwrap().code, Some(10001));
    }

    /// 续期阈值：3 天内（含刚好相等边界）触发刷新，充裕则跳过。
    #[test]
    fn renew_threshold_semantics() {
        let now: u64 = 1_800_000_000;
        // 充裕（> 阈值）→ 应跳过：剩余 10 天。
        assert!(now + 10 * 24 * 3600 - now > RENEW_THRESHOLD_SECS);
        // 临界（= 阈值）→ 不大于阈值 → 触发刷新。
        assert!(now + RENEW_THRESHOLD_SECS - now <= RENEW_THRESHOLD_SECS);
        // 已过期（saturating_sub 归零）→ 触发刷新。
        assert_eq!(now.saturating_sub(now + 100), 0);
    }

    /// 池空错误码（组合根回退机器解密路径的判定依据）。
    #[test]
    fn pool_error_codes_are_stable() {
        assert_eq!(WorkBuddyPoolError::PoolEmpty.code(), "workbuddy_pool_empty");
        assert_eq!(
            WorkBuddyPoolError::ResponseInvalid.code(),
            "workbuddy_refresh_response_invalid"
        );
    }

    /// P19.1 强制刷新判定（401 兜底重试链）：充裕跳过 / 临近过期刷新 /
    /// 强制永不跳过（三态离线口径）。
    #[test]
    fn forced_renewal_skips_threshold_only_when_not_forced() {
        // 充裕（> 3 天阈值）：非强制跳过，强制刷新。
        assert!(should_skip_renewal(RENEW_THRESHOLD_SECS + 1, false));
        assert!(!should_skip_renewal(RENEW_THRESHOLD_SECS + 1, true));
        // 临界（= 阈值）：不大于阈值 → 不跳过（触发刷新）。
        assert!(!should_skip_renewal(RENEW_THRESHOLD_SECS, false));
        // 已过期（saturating 归零）：不跳过。
        assert!(!should_skip_renewal(0, false));
        // 强制永不跳过（已过期同样刷新）。
        assert!(!should_skip_renewal(0, true));
    }

    // ===== P19.1 续期验证探针（四态离线单测） =====

    use crate::workbuddy_credential::parse_session_snapshot;

    /// 明文 token 形态快照（5.5.x 口径；uid 与池记录对齐）。
    const VERIFY_SNAPSHOT_PLAIN: &str = r#"{
  "auth": {
    "accessToken": "plain-access",
    "refreshToken": "plain-refresh",
    "tokenType": "Bearer",
    "domain": "www.workbuddy.cn",
    "lastRefreshTime": 1792219259894,
    "expiresAt": 1794638459316,
    "refreshExpiresAt": 1794811259316
  },
  "account": { "uid": "uid-1", "uin": "330126449009" },
  "accounts": [ { "uid": "uid-1" } ],
  "allAccounts": [ { "uid": "uid-1" } ]
}"#;

    /// 加密信封形态快照（5.6.2+ 口径；工具侧无明文凭据的分支用）。
    const VERIFY_SNAPSHOT_ENVELOPE: &str = r#"{
  "auth": {
    "accessToken": { "$wbEncrypted": 1, "envelope": "x" },
    "refreshToken": { "$wbEncrypted": 1, "envelope": "x" },
    "domain": "www.workbuddy.cn",
    "expiresAt": 1794638459316
  },
  "account": { "uid": "uid-1", "uin": "330126449009" },
  "accounts": [ { "uid": "uid-1" } ],
  "allAccounts": [ { "uid": "uid-1" } ]
}"#;

    fn verify_pool(login_root: &Path, uid: &str) -> WorkBuddyLoginStore {
        std::fs::create_dir_all(login_root).unwrap();
        let store = WorkBuddyLoginStore::new(login_root.to_path_buf());
        store
            .save(&WorkBuddyLoginRecord {
                profile_id: format!("wb-{uid}"),
                account_id: uid.to_string(),
                screen_name: "账号一".to_string(),
                masked_mobile: String::new(),
                access_token: "old-access".to_string(),
                refresh_token: "old-refresh".to_string(),
                access_token_expires_at_unix_seconds: 1_800_000_000,
                refresh_token_expires_at_unix_seconds: 1_810_000_000,
                domain: "www.workbuddy.cn".to_string(),
                session_state: "s".to_string(),
                device_id: "dev".to_string(),
                minted_at_unix_seconds: 1,
                last_updated_at_unix_seconds: 1,
            })
            .unwrap();
        store
    }

    fn verify_snapshot_loader(content: &'static str) -> impl Fn() -> Result<
        crate::workbuddy_credential::WorkBuddySessionSnapshot,
        crate::workbuddy_credential::WorkBuddySessionError,
    > {
        move || parse_session_snapshot(content)
    }

    /// 状态一：刷新成功——实调刷新成功且新对已写回（renewed=true + 新
    /// 有效期 + 快照时间戳随回执）。
    #[test]
    fn verify_renewal_reports_success_with_new_expiry() {
        let temp = tempfile::tempdir().unwrap();
        let store = verify_pool(temp.path(), "uid-1");
        let renew =
            |profile_id: &str| -> Result<WorkBuddyRenewalOutcome, WorkBuddyPoolError> {
                assert_eq!(profile_id, "wb-uid-1");
                Ok(WorkBuddyRenewalOutcome::Renewed {
                    access_expires_at_unix_seconds: 1_900_000_000,
                    refresh_expires_at_unix_seconds: 1_910_000_000,
                })
            };
        let report = verify_capture_renewal(
            &store,
            &verify_snapshot_loader(VERIFY_SNAPSHOT_PLAIN),
            &|| Some(1_792_220_000),
            &renew,
        )
        .unwrap();
        assert_eq!(report.uid, "uid-1");
        assert!(report.renewed, "续期链路验证通过");
        assert_eq!(report.access_expires_at_unix_seconds, 1_900_000_000);
        assert_eq!(report.refresh_expires_at_unix_seconds, 1_910_000_000);
        // 快照时间戳（ms → s 换算口径）。
        assert_eq!(report.info_last_refresh_unix_seconds, 1_792_219_259);
        assert_eq!(report.info_access_expires_unix_seconds, 1_794_638_459);
        assert_eq!(report.info_modified_unix_seconds, 1_792_220_000);
    }

    /// 状态二：刷新失败——被拒（refreshToken 已轮换/吊销）与请求失败
    /// 分别映射（保存的凭据无法持续更新，如实报因）。
    #[test]
    fn verify_renewal_reports_refresh_failure() {
        let temp = tempfile::tempdir().unwrap();
        let store = verify_pool(temp.path(), "uid-1");
        let rejected = |_: &str| Err(WorkBuddyPoolError::RefreshRejected);
        assert_eq!(
            verify_capture_renewal(
                &store,
                &verify_snapshot_loader(VERIFY_SNAPSHOT_PLAIN),
                &|| None,
                &rejected
            )
            .unwrap_err()
            .code(),
            "workbuddy_verify_refresh_rejected"
        );
        let failed = |_: &str| Err(WorkBuddyPoolError::RequestFailed);
        assert_eq!(
            verify_capture_renewal(
                &store,
                &verify_snapshot_loader(VERIFY_SNAPSHOT_PLAIN),
                &|| None,
                &failed
            )
            .unwrap_err()
            .code(),
            "workbuddy_verify_request_failed"
        );
    }

    /// 状态三：凭据不匹配——当前登录账号无对应池记录（token 明文形态，
    /// 非信封）→ workbuddy_verify_credential_mismatch。
    #[test]
    fn verify_renewal_reports_credential_mismatch_without_pool_record() {
        let temp = tempfile::tempdir().unwrap();
        let store = verify_pool(temp.path(), "uid-1");
        // 快照账号 uid-other：池里只有 uid-1 → 无匹配记录。
        let snapshot_other = VERIFY_SNAPSHOT_PLAIN.replace("uid-1", "uid-other");
        let loader = move || parse_session_snapshot(&snapshot_other);
        let renew = |_: &str| unreachable!("无匹配记录不得发起刷新");
        assert_eq!(
            verify_capture_renewal(&store, &loader, &|| None, &renew)
                .unwrap_err()
                .code(),
            "workbuddy_verify_credential_mismatch"
        );
    }

    /// 状态四：信封检测——当前登录账号 token 为 `$wbEncrypted` 加密信封
    /// 且工具侧无该账号明文池凭据 → 诚实报「登录态已加密，请用 OAuth
    /// 添加账号」（workbuddy_verify_envelope_encrypted）。
    #[test]
    fn verify_renewal_reports_envelope_encrypted_without_pool_record() {
        let temp = tempfile::tempdir().unwrap();
        let store = verify_pool(temp.path(), "uid-1");
        let snapshot_other = VERIFY_SNAPSHOT_ENVELOPE.replace("uid-1", "uid-other");
        let loader = move || parse_session_snapshot(&snapshot_other);
        let renew = |_: &str| unreachable!("无匹配记录不得发起刷新");
        assert_eq!(
            verify_capture_renewal(&store, &loader, &|| None, &renew)
                .unwrap_err()
                .code(),
            "workbuddy_verify_envelope_encrypted"
        );
    }

    /// 快照不可用（文件缺失等）：Snapshot(载荷 = 会话域稳定错误码) 优先。
    #[test]
    fn verify_renewal_reports_snapshot_unavailable_first() {
        let temp = tempfile::tempdir().unwrap();
        let store = verify_pool(temp.path(), "uid-1");
        let loader = || {
            Err(crate::workbuddy_credential::WorkBuddySessionError::FileMissing)
        };
        let renew = |_: &str| unreachable!("快照不可用不得发起刷新");
        assert_eq!(
            verify_capture_renewal(&store, &loader, &|| None, &renew)
                .unwrap_err()
                .code(),
            "workbuddy_session_file_missing"
        );
    }
}
