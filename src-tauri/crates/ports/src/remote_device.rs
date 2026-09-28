//! 账号级远程设备管理端口（ADR-0031）。
//!
//! 设备列表与退出是账号级共享能力（跨产品复用同一份服务端设备额度），
//! 协议放在产品中立的 infrastructure `remote_device` 模块实现；本端口
//! 只定义 application 可依赖的能力边界。
//!
//! 安全边界：
//! - 端口不暴露凭据材料（access token、refresh token、client_id 均不出现）；
//! - `RemoteDeviceEntry.device_id` 仅用于内部流转与退出定位，UI 不得渲染；
//! - `clear_refresh_token` 是破坏性操作，调用方必须已完成用户单次确认。

use std::fmt;

use serde::Serialize;

/// 远程设备管理能力：infrastructure 实现负责解密凭据包并直连服务端。
pub trait RemoteDeviceManager: Send + Sync {
    /// 拉取账号的远程设备列表（只读）。
    fn list_devices(&self, profile_id: &str) -> Result<RemoteDeviceSnapshot, RemoteDeviceError>;

    /// 退出指定远程设备（破坏性操作，调用方必须已完成用户单次确认）。
    ///
    /// 目标设备 ID 等于本机当前凭据的 device_id 时必须拒绝
    /// （ADR-0031 决策 3 后端防线；UI 隐藏退出按钮之外的纵深防御）。
    fn clear_refresh_token(
        &self,
        profile_id: &str,
        target_device_id: &str,
    ) -> Result<(), RemoteDeviceError>;
}

/// 设备列表快照：账号全量设备行 + 已用/上限摘要（ADR-0031 决策 5）。
#[derive(Debug, Clone, Serialize)]
pub struct RemoteDeviceSnapshot {
    pub devices: Vec<RemoteDeviceEntry>,
    /// 有效设备行数（缺少 DeviceID 的降级行不计入）。
    pub used_count: usize,
    /// 服务端设备上限；字段缺失时为 None（展示层回退"未知"）。
    pub max_count: Option<u32>,
}

/// 单台远程设备行。字段与官方授权页对齐（设备名、类型、绑定产品、最近活跃），
/// 另加 `is_local` 本机标记（ADR-0031 决策 3：本机行 UI 隐藏退出按钮）。
#[derive(Debug, Clone, Serialize)]
pub struct RemoteDeviceEntry {
    /// 服务端设备 ID：仅内部流转与退出定位，UI 不得渲染。
    pub device_id: String,
    pub device_type: Option<String>,
    pub device_name: Option<String>,
    pub bound_products: Vec<String>,
    /// 最近活跃时刻原样保留（服务端存在字符串与数字两种形态），
    /// 格式化交给展示层。
    pub last_active_at: Option<serde_json::Value>,
    /// 本机标记：优先采用服务端 CurrentDevice 标记（实测服务端 DeviceID
    /// 与本地凭据 device_id 是两套标识体系，直接相等永不命中），
    /// 本地 device_id 相等保留为兜底路径（ADR-0031 决策 3）。
    pub is_local: bool,
}

/// 远程设备能力错误；不携带 Token、refresh token 或完整 device_id。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemoteDeviceError {
    /// 网络层失败（连接/超时/DNS/TLS）。
    Network,
    /// 服务端返回非 200 且响应体无可解析业务码。
    Http(u16),
    /// 服务端业务码拒绝（如 20401 设备数量已达上限）。
    Business(i64),
    /// 响应结构不符合协议预期。
    Protocol,
    /// 本地凭据不可用或与期望产品不匹配；原因码为稳定 snake_case 标识。
    Credential(&'static str),
    /// 退出目标是本机当前设备（后端防线拒绝）。
    LocalDeviceTargeted,
}

impl fmt::Display for RemoteDeviceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        // 输出稳定 snake_case 原因码：供 UI/日志映射为可理解文案，
        // 不携带任何敏感材料。
        match self {
            Self::Network => formatter.write_str("remote_device_network"),
            Self::Http(status) => write!(formatter, "remote_device_http_{status}"),
            Self::Business(code) => write!(formatter, "remote_device_business_{code}"),
            Self::Protocol => formatter.write_str("remote_device_protocol"),
            Self::Credential(reason) => {
                write!(formatter, "remote_device_credential_{reason}")
            }
            Self::LocalDeviceTargeted => {
                formatter.write_str("remote_device_local_device_targeted")
            }
        }
    }
}

impl std::error::Error for RemoteDeviceError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_display_uses_stable_snake_case_reason_codes() {
        assert_eq!(RemoteDeviceError::Network.to_string(), "remote_device_network");
        assert_eq!(RemoteDeviceError::Http(502).to_string(), "remote_device_http_502");
        assert_eq!(
            RemoteDeviceError::Business(20401).to_string(),
            "remote_device_business_20401"
        );
        assert_eq!(RemoteDeviceError::Protocol.to_string(), "remote_device_protocol");
        assert_eq!(
            RemoteDeviceError::Credential("client_mismatch").to_string(),
            "remote_device_credential_client_mismatch"
        );
        assert_eq!(
            RemoteDeviceError::LocalDeviceTargeted.to_string(),
            "remote_device_local_device_targeted"
        );
    }
}
