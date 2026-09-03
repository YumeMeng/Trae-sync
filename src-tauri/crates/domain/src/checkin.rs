//! 独立签到能力的领域模型。
//!
//! 领域层只保存业务状态与脱敏结果，不持有 Token、Cookie、设备私钥或完整认证对象。

use serde::{Deserialize, Serialize};
use std::time::SystemTime;

/// 单账号签到最终业务结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckinOutcome {
    Claimed,
    AlreadyCheckedIn,
    NotEligible,
    AuthMismatch,
    CredentialRefreshFailed,
    ProfileBusy,
    NetworkError,
    RuntimeError,
    VerificationFailed,
}

/// 单账号任务内部阶段；前端只接收最终结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckinTaskState {
    Queued,
    LoadingProfile,
    RefreshingCredential,
    StatusBefore,
    ClaimingOnce,
    StatusAfter,
    Completed,
    Cancelled,
}

/// TRAE 返回的签到状态摘要。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct CheckinStatusSnapshot {
    pub enabled: bool,
    pub checked_in: bool,
    pub credits: Option<i64>,
    pub business_code: Option<i64>,
}

/// `claim` 返回的脱敏业务摘要。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct CheckinClaimSnapshot {
    pub business_code: Option<i64>,
    pub credits: Option<i64>,
}

/// 单个权益包的额度快照（每日签到 / 每月登录 / 福利 / 邀请奖励等）。
/// 语义依据 TRAE 客户端 `workbench.desktop.main.js` 的 `Zms(limit, usage)`：
/// 剩余 = max(limit - used, 0)，`used` 为已消耗量。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EntitlementPackSnapshot {
    /// 服务端权益实例 ID（非敏感）。
    pub entitlement_id: String,
    /// 分组名（如“每日签到”“每月登录积分”）；无分组时回退展示名。
    pub group_name: String,
    /// 配额上限（credits_limit）。
    pub credits_limit: f64,
    /// 已消耗量（usage.credits_amount）。
    pub credits_used: f64,
    /// 过期时刻（Unix 秒）；0 表示长期有效。
    pub expires_at_unix_seconds: u64,
}

/// `ide_user_ent_usage` 聚合后的真实可用额度快照。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct EntitlementUsageSnapshot {
    /// 未过期积分包的剩余总和：Σ max(limit - used, 0)。
    pub remaining_credits: f64,
    /// 参与汇总的积分包明细（已过滤无积分维度与已过期条目）。
    pub packs: Vec<EntitlementPackSnapshot>,
}

/// 单账号签到结果。所有字段均可安全进入前端和本地结果列表。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckinResult {
    pub profile_id: String,
    pub outcome: CheckinOutcome,
    pub state: CheckinTaskState,
    pub claim_attempted: bool,
    pub before: Option<CheckinStatusSnapshot>,
    pub after: Option<CheckinStatusSnapshot>,
    pub detail_code: Option<String>,
    pub started_at: SystemTime,
    pub finished_at: SystemTime,
}

/// 批量签到摘要。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct CheckinBatchSummary {
    pub total: usize,
    pub completed: usize,
    pub failed: usize,
    pub cancelled: usize,
    pub results: Vec<CheckinResult>,
}
