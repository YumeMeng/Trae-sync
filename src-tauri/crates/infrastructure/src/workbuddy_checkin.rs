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

use std::time::Duration;

use serde::Deserialize;

use crate::workbuddy_credential::WorkBuddySessionSnapshot;

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
}
