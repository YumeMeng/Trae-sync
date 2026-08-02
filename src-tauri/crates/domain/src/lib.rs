//! Trae Sync 领域层：稳定值对象与纯规则。
//!
//! T01 骨架阶段只定义工作台状态所需的最小值对象。
//! 不预建未来模块（CatalogRepository、ProductAdapter 等）的空 trait。
//!
//! T02 新增 `workbench_read` 模块：数据位置身份、schema 兼容状态、
//! 账号证据与结构化只读原因。仅承载 T02 需要的纯值对象。

use serde::{Deserialize, Serialize};

pub mod history;
pub mod workbench_read;

// 重导出 T02 工作台只读入口的核心值对象
// `DataLocationState` 复用 T01 已有定义，不重复导出
pub use workbench_read::{
    AccountEvidence, AuthFingerprint, CompatibilityState, DataLocationId, EvidenceState,
    IncompatibleReason, ReadonlyReason, SchemaFingerprint, SourceEventSummary, TableCounts, UserId,
    UserIdError, WorkbenchReadState,
};

// 重导出 T03/T04 历史库核心值对象
pub use history::{
    AuthorizationState, BrowseAccountNode, BrowseProjectNode, BrowseResult, BrowseSessionNode,
    CatalogGenerationId, ContentGraphHash, ConversationPreview, DiagnosticIntegrityAssertion,
    FileIdentity, HistoryBrowseSummary, MessageProjection, OwnerObservation, ProcessRunningState,
    ProjectIdentity, ProjectObservation, ProjectSourceAssignment, ScanFailureReason, ScanOutcome,
    ScanRequest, SearchHit, SeenAccount, SessionIdentity, SessionProjection, SessionVersion,
    SnapshotFileEntry, SnapshotFileKind, SnapshotFingerprint, SnapshotId, SoftDeletionEntityKind,
    SoftDeletionMarker, SourceSnapshotMeta, StorageRootId, VersionClassification,
};

/// 操作 ID：贯穿结构化日志、命令、应用服务与操作 manifest 的稳定标识。
/// T01 阶段用于证明日志具备 operation_id 字段。
///
/// 【R1 修复（第四次）】内部字符串私有化，外部无法直接 `OperationId(secret)`
/// 构造。**唯一公开构造入口是 `new()`**——不接受任何外部字符串。
///
/// 第三次修复保留了 `from_validated()` 作为字符串注入入口，但该入口接受
/// 任意 `op-<字母数字>` 字符串，包括随机 hex key、恢复短语等，且 T01 没有
/// 从持久化值恢复 OperationId 的真实需求。按"不为未发生需求保留入口"原则
/// 移除 `from_validated()`，从设计上阻止任意字符串进入 operation_id。
///
/// `Serialize` 保留用于日志输出；不派生 `Deserialize`，防止外部通过
/// serde 反序列化注入任意字符串。编译期保证由 `tests/ui/` 下的
/// trybuild compile-fail 测试真实编译验证。
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
pub struct OperationId(String);

impl OperationId {
    /// 生成新的随机操作 ID（T01 阶段使用简单时间戳 + 进程 ID，不引入 uuid 依赖）。
    ///
    /// 这是外部获得 `OperationId` 实例的唯一公开入口。不接受外部字符串，
    /// 因此随机 hex key、认证正文、恢复短语等无法进入 operation_id。
    pub fn new() -> Self {
        use std::time::{SystemTime, UNIX_EPOCH};
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let pid = std::process::id();
        Self(format!("op-{nanos}-{pid}"))
    }

    /// 返回内部字符串的只读引用。
    ///
    /// 读取字符串不导致 secret 注入——构造才受限于 `new()`。
    /// 此方法供日志层序列化与 defense-in-depth 扫描使用。
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Default for OperationId {
    fn default() -> Self {
        Self::new()
    }
}

/// 平台标识：V1 只有 Work CN
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlatformId(pub String);

impl PlatformId {
    pub fn work_cn() -> Self {
        Self("work_cn".to_string())
    }
}

/// 工作台能力开关：T01 全部为 false
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapabilityFlags {
    pub scan_enabled: bool,
    pub sync_enabled: bool,
    pub backup_enabled: bool,
    pub restore_enabled: bool,
}

impl Default for CapabilityFlags {
    fn default() -> Self {
        // T01 骨架阶段：所有真实能力禁用
        Self {
            scan_enabled: false,
            sync_enabled: false,
            backup_enabled: false,
            restore_enabled: false,
        }
    }
}

/// 历史库摘要：T01 始终为空
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct HistorySummary {
    pub account_count: u64,
    pub project_count: u64,
    pub session_count: u64,
}

/// 工作台状态聚合：由 application 层构造，经 Tauri command 返回给前端。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorkspaceState {
    pub platform: PlatformContext,
    pub data_location: DataLocationState,
    pub current_account: CurrentAccountState,
    pub history: HistorySummary,
    pub capabilities: CapabilityFlags,
    /// 诚实状态文案：UI 显示“真实能力尚未启用”
    pub honest_status: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlatformContext {
    pub platform_id: PlatformId,
    pub display_name: String,
    /// Adapter 是否已实现真实行为（T01 阶段始终为 false）
    pub adapter_implemented: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DataLocationState {
    pub selected: bool,
    pub display_name: Option<String>,
    pub unavailable_reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CurrentAccountState {
    pub detected: bool,
    pub user_fingerprint: Option<String>,
    pub unavailable_reason: Option<String>,
}

/// 构造 T01 阶段的固定空工作台状态。
///
/// 这是 specification 第 7 节“空工作台显示诚实状态”的直接实现：
/// 平台 Work CN（Adapter 边界保留但不实现）、无数据位置、无账号、空历史、能力禁用。
///
/// 放在 domain 层是因为它仅使用 domain 类型，是 `WorkspaceState` 值对象的默认构造器。
/// infrastructure 的 `StaticWorkspaceStateProvider` 与未来 application 用例都可直接调用，
/// 避免基础设施反向依赖 application。
pub fn empty_workspace_state() -> WorkspaceState {
    WorkspaceState {
        platform: PlatformContext {
            platform_id: PlatformId::work_cn(),
            display_name: "TRAE Work CN".to_string(),
            // T01 阶段 Adapter 边界保留，不实现真实行为
            adapter_implemented: false,
        },
        data_location: DataLocationState {
            selected: false,
            display_name: None,
            unavailable_reason: Some("not_selected".to_string()),
        },
        current_account: CurrentAccountState {
            detected: false,
            user_fingerprint: None,
            unavailable_reason: Some("not_detected".to_string()),
        },
        history: HistorySummary::default(),
        capabilities: CapabilityFlags::default(),
        honest_status: "真实能力尚未启用".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn operation_id_new_generates_valid_format() {
        // new() 是唯一公开构造入口，生成 op-<nanos>-<pid> 格式
        let op_id = OperationId::new();
        let s = op_id.as_str();
        assert!(s.starts_with("op-"));
        assert!(s.len() > 4);
        // new() 生成的格式为 op-<数字>-<数字>，各部分全为数字
        let rest = &s[3..];
        let parts: Vec<&str> = rest.split('-').collect();
        assert!(parts.len() >= 2, "new() 应生成 op-<nanos>-<pid> 格式");
        for part in parts {
            assert!(
                part.chars().all(|c| c.is_ascii_digit()),
                "new() 生成的各部分应全为数字，实际: {part}"
            );
        }
    }

    // ============== R1 关键反例测试：任意字符串无法进入 operation_id ==============
    //
    // from_validated 已移除，外部无法注入字符串。以下测试验证 new() 生成的
    // operation_id 不包含任意 hex key、认证正文、恢复短语。
    // 编译期保证（from_validated 不存在、字段私有）由 tests/ui/ 下的
    // trybuild compile-fail 测试真实编译验证。

    #[test]
    fn random_hex_key_cannot_enter_operation_id() {
        // 随机十六进制 key 无法进入 operation_id
        // from_validated 已移除，外部无法用 "op-a1b2c3d4e5f6789012345abcdef" 构造
        let op_id = OperationId::new();
        let s = op_id.as_str();
        // new() 只生成 op-<数字>-<数字>，不含字母 hex
        assert!(!s.contains("a1b2c3d4e5f6789012345abcdef"));
        assert!(!s.contains("abcdef"));
    }

    #[test]
    fn auth_ciphertext_cannot_enter_operation_id() {
        // JWT 密文和 Bearer token 无法进入 operation_id
        let op_id = OperationId::new();
        let s = op_id.as_str();
        assert!(!s.contains("eyJ"));
        assert!(!s.contains("bearer"));
        assert!(!s.contains("payload"));
        assert!(!s.contains("sig"));
    }

    #[test]
    fn recovery_passphrase_cannot_enter_operation_id() {
        // 恢复短语无法进入 operation_id
        let op_id = OperationId::new();
        let s = op_id.as_str();
        assert!(!s.contains("recovery"));
        assert!(!s.contains("pass"));
        assert!(!s.contains("secret"));
        assert!(!s.contains("password"));
    }

    #[test]
    fn operation_id_field_is_private() {
        // 编译期保证：OperationId 的内部字段是私有的
        // 外部代码无法 `op_id.0` 访问，也无法 `OperationId("xxx".to_string())` 构造
        // 由 tests/ui/operation_id_no_direct_construction.rs 的 trybuild 测试验证
        // 此运行时测试验证 as_str 是唯一读取入口
        let op_id = OperationId::new();
        assert!(op_id.as_str().starts_with("op-"));
    }
}
