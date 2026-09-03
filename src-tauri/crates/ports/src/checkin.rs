//! 独立签到能力的 transport 端口。
//!
//! 端口不暴露凭据材料；基础设施实现负责从受保护 Profile 读取认证上下文。

use traesync_domain::{CheckinClaimSnapshot, CheckinStatusSnapshot, EntitlementUsageSnapshot};

/// transport 错误分类。错误文本不包含 Token 或远程响应正文。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckinTransportError {
    AuthMismatch,
    CredentialRefreshFailed,
    ProfileBusy,
    Network,
    Protocol,
    Business(i64),
    Runtime,
}

/// 可替换签到 transport；fixture 与真实 HTTP 共用该契约。
pub trait CheckinTransport: Send + Sync {
    fn status(&self, profile_id: &str) -> Result<CheckinStatusSnapshot, CheckinTransportError>;
    fn claim(&self, profile_id: &str) -> Result<CheckinClaimSnapshot, CheckinTransportError>;
    /// 查询真实模型额度（`ide_user_ent_usage`）：只读，不消耗签到资格。
    fn entitlement_usage(
        &self,
        profile_id: &str,
    ) -> Result<EntitlementUsageSnapshot, CheckinTransportError>;
}

/// 设备重铸端口（ADR-0019 v6：仅由用户在账号详情手动触发）。
///
/// v6 起签到链路不再自动调用本端口：claim 被拒时直接报错，
/// 由用户决定"稍后重试"还是"重置签到设备"。
///
/// 协议事实（2026-08-26 实测，证据 `.scratch/checkin-http/reports/remint-*.json`）：
/// - 重铸 = 当前账号 token → GetPCAuthCode（`x-cloudide-token` 头）→ 全新
///   设备四件套 → ExchangeToken(AuthCode+PKCE) → JWT 账号匹配 → 凭据写回；
/// - 新设备首签仅 SOLO 形态可行（SOLO_PC / en1oxy7wnw8j9n / 完整硬件字段）；
/// - 9074 含频率维度且窗口不可预测，重铸后立即 claim 可能仍被拒。
///
/// 实现方负责凭据包与注册表的原子更新；端口层不暴露凭据材料。
pub trait CheckinDeviceRemint: Send + Sync {
    /// 为账号重铸全新设备并写回凭据；成功返回新设备 ID。
    ///
    /// 失败时旧凭据保持不变（零回写），调用方按签到失败处理。
    fn remint_device(&self, profile_id: &str) -> Result<String, String>;
}
