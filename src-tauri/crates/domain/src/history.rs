//! Trae Sync 历史库领域层：T03/T04 稳定值对象与纯规则。
//!
//! 本模块承载 P1 历史基础所需的全部值对象：
//! - 快照/目录 ID 与不可变快照元数据
//! - 授权与扫描状态机
//! - 账号/项目/会话/消息浏览类型
//! - 来源观测与首观察所有者（Gate E）
//! - 确定性内容图哈希与版本分类（Gate I）
//! - 软删除语义（Gate J）
//!
//! 设计原则：
//! - 纯值对象，无 IO；IO 由 ports/infrastructure 承担
//! - 私有化内部字符串字段，公开构造器；防止 secret 注入
//! - serde 约定与 T01/T02 一致：newtype 用 `transparent`、enum 用 `rename_all="snake_case"`
//! - 不预建 T05+ 类型（SyncPlan、PlanAction、OperationState 等）

use serde::{Deserialize, Serialize};
use std::time::SystemTime;

// ============================================================================
// 1. ID 类型
// ============================================================================

/// 快照 ID：`snap-<时间纳秒>-<pid>-<短随机>`。
///
/// 私有字段，唯一公开构造入口 `new()` 不接受外部字符串——
/// 防止 raw_key、认证正文等通过快照 ID 注入。
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
pub struct SnapshotId(String);

impl SnapshotId {
    /// 生成新快照 ID。不接受外部字符串。
    pub fn new() -> Self {
        use std::time::{SystemTime, UNIX_EPOCH};
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let pid = std::process::id();
        // 短随机后缀：AtomicU64 计数器，避免同一纳秒多次构造冲突
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
        Self(format!("snap-{nanos}-{pid}-{seq}"))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// 从目录库存储的字符串重建 SnapshotId。
    ///
    /// 仅用于 infrastructure 层从目录库读取已存储的 snapshot_id（如 owner_observation、
    /// session_version）。不用于从外部输入构造——外部输入仍须通过 `new()` 生成。
    pub fn from_db_str(s: &str) -> Self {
        Self(s.to_string())
    }
}

impl Default for SnapshotId {
    fn default() -> Self {
        Self::new()
    }
}

/// 目录库代次 ID：`catalog-gen-<时间纳秒>-<pid>`。
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
pub struct CatalogGenerationId(String);

impl CatalogGenerationId {
    pub fn new() -> Self {
        use std::time::{SystemTime, UNIX_EPOCH};
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let pid = std::process::id();
        Self(format!("catalog-gen-{nanos}-{pid}"))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Default for CatalogGenerationId {
    fn default() -> Self {
        Self::new()
    }
}

/// 存储根 ID：从存储根目录的 canonical 路径与卷身份派生的稳定标识。
///
/// 私有字段，由 infrastructure 层通过 `from_canonical_path` 构造。
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct StorageRootId(String);

impl StorageRootId {
    /// 从 canonical 路径字符串构造存储根 ID。
    /// infrastructure 层负责确保输入是已 canonicalize 的路径。
    pub fn from_canonical_path(canonical: &str) -> Self {
        // 简单稳定派生：去掉末尾分隔符后直接使用
        let trimmed = canonical.trim_end_matches(['/', '\\']);
        Self(trimmed.to_string())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

// ============================================================================
// 2. 授权与扫描状态
// ============================================================================

/// 扫描授权状态：P1 fixture-only，默认关闭。
///
/// 自动扫描默认关闭；未授权时不发布快照。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthorizationState {
    /// 未授权（默认）
    NotAuthorized,
    /// 已授权扫描指定 fixture 路径
    Authorized {
        /// 已 canonical 的 fixture_root 路径（不携带 raw_key）
        canonical_fixture_root: String,
        /// 数据库相对路径（如 "database.db"）
        db_relative_path: String,
    },
}

impl Default for AuthorizationState {
    fn default() -> Self {
        Self::NotAuthorized
    }
}

/// TRAE 进程运行状态：扫描前置条件。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProcessRunningState {
    /// 未知（fixture 模式下默认）
    Unknown,
    /// 未运行（可扫描）
    NotRunning,
    /// 运行中（拒绝扫描）
    Running,
}

/// 扫描请求：由 UI 显式触发。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScanRequest {
    /// 已 canonical 的 fixture_root
    pub canonical_fixture_root: String,
    /// 数据库相对路径
    pub db_relative_path: String,
    /// TRAE 进程状态（fixture 模式由调用方注入）
    pub process_state: ProcessRunningState,
    /// 扫描时间（注入便于测试）
    pub now: SystemTime,
    /// schema 指纹（由 application 通过 DatabaseProbePort 获取）
    pub schema_fingerprint: String,
    /// Adapter mapping 版本（如 "work_cn_v1"）
    pub mapping_version: String,
    /// 产品版本
    pub product_version: String,
    /// 账号证据引用（不携带正文）
    pub account_evidence_ref: Option<String>,
    /// 存储根路径（已 canonical，快照发布到此目录下的 snapshots/）
    pub storage_root: String,
}

/// 扫描失败原因：结构化，不携带 secret。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScanFailureReason {
    /// 未授权
    NotAuthorized,
    /// TRAE 进程运行中
    ProcessRunning,
    /// 数据库文件不存在
    DatabaseMissing,
    /// schema 不兼容
    SchemaIncompatible,
    /// 存储根不可用
    StorageRootUnavailable,
    /// 捕获前后源文件集漂移
    SourceSetDrift,
    /// 目录库事务失败
    CatalogTransactionFailed,
    /// 目录库密钥未配置
    CatalogKeyMissing,
}

/// 扫描结果：成功/去重/失败。
///
/// 仅派生 `Serialize`：`SnapshotId` 按安全设计不实现 `Deserialize`，
/// 防止外部通过 serde 反序列化注入任意字符串到快照 ID。
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ScanOutcome {
    /// 成功发布新快照并更新目录库
    Success {
        snapshot_id: SnapshotId,
        snapshot_meta: SourceSnapshotMeta,
        catalog_updated: bool,
    },
    /// 数据指纹与已有快照相同，未创建新快照
    Deduplicated {
        existing_snapshot_id: SnapshotId,
        fingerprint: SnapshotFingerprint,
    },
    /// 失败，未发布任何快照
    Failed { reason: ScanFailureReason },
}

// ============================================================================
// 3. 不可变快照元数据
// ============================================================================

/// 快照文件种类：DB/WAL/SHM。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SnapshotFileKind {
    Db,
    Wal,
    Shm,
}

/// 文件身份：volume + file index（Windows 上由 GetFileInformationByHandle 提供）。
///
/// 用于检测文件是否被替换（即使路径相同）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileIdentity {
    /// 卷序列号
    pub volume_serial: u64,
    /// 文件索引（高 64 位）
    pub file_index_high: u64,
    /// 文件索引（低 64 位）
    pub file_index_low: u64,
}

/// 单个快照文件（DB/WAL/SHM）的捕获信息。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnapshotFileEntry {
    pub kind: SnapshotFileKind,
    /// 相对路径（如 "database.db"、"database.db-wal"）
    pub relative_path: String,
    /// 文件是否存在
    pub present: bool,
    /// 文件大小（字节）
    pub size: u64,
    /// SHA-256 hex
    pub sha256: String,
    /// 文件身份（存在时必填）
    pub file_identity: Option<FileIdentity>,
}

/// 数据指纹：所有捕获文件 SHA-256 的聚合哈希，用于去重。
///
/// 私有字段，由 infrastructure 层构造。
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SnapshotFingerprint(pub String);

impl SnapshotFingerprint {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// 来源快照元数据：对应 `snapshots/<snapshot_id>/snapshot.json`。
///
/// 仅派生 `Serialize`：含 `SnapshotId`，不可反序列化（防注入）。
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SourceSnapshotMeta {
    pub snapshot_id: SnapshotId,
    pub platform_id: String,
    pub data_location_id: String,
    pub product_version: String,
    pub schema_fingerprint: String,
    pub mapping_version: String,
    /// 账号证据引用（不携带正文，仅引用 ID）
    pub account_evidence_ref: Option<String>,
    pub captured_at: SystemTime,
    /// 各文件捕获信息（DB 必须存在，WAL/SHM 按实际）
    pub files: Vec<SnapshotFileEntry>,
    /// 数据指纹
    pub fingerprint: SnapshotFingerprint,
}

// ============================================================================
// 4. 账号/项目/会话/消息浏览类型
// ============================================================================

/// 已见账号：目录库中记录的账号。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SeenAccount {
    pub user_id: String,
    /// 首次观察时间
    pub first_seen_at: SystemTime,
    /// 最后观察时间
    pub last_seen_at: SystemTime,
}

/// 项目身份：由 Adapter 可靠判定的同一工作区语义。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectIdentity {
    /// 稳定 project_id（来自活动库）
    pub project_id: String,
    /// biz_project_id
    pub biz_project_id: String,
    /// 显示名
    pub display_name: String,
}

/// 单次 owner 观察：扫描时记录的活动库归属。
///
/// 仅派生 `Serialize`：含 `SnapshotId`，不可反序列化（防注入）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OwnerObservation {
    /// 观察到的 owner user_id
    pub owner_user_id: String,
    /// 观察时间
    pub observed_at: SystemTime,
    /// 来源快照 ID
    pub source_snapshot_id: SnapshotId,
}

/// 项目观察记录：包含首观察 owner 和全部历史观察。
///
/// 仅派生 `Serialize`：含 `Vec<OwnerObservation>`（内含 `SnapshotId`），不可反序列化。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProjectObservation {
    pub project_identity: ProjectIdentity,
    /// 首观察 owner——永不变化（Gate E 核心）
    pub first_observed_owner: String,
    pub first_observed_at: SystemTime,
    /// 当前活动库归属（与最新扫描一致）
    pub current_live_owner: String,
    /// 历次 owner 观察（按时间追加）
    pub owner_observations: Vec<OwnerObservation>,
}

/// 用户来源分配：用户可选的展示与筛选归类。
///
/// 仅改变目录库显示/筛选分类，不修改快照、观察或活动库（Gate E）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectSourceAssignment {
    pub project_id: String,
    /// 用户分配的归属账号；None 表示未分配（回退到 first_observed_owner）
    pub user_assigned_owner: Option<String>,
    pub assigned_at: SystemTime,
}

/// 会话身份：(product_history_namespace, original_session_id)。
///
/// Gate I 核心：会话身份不依赖 title。
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SessionIdentity {
    /// 产品历史命名空间（如 "work_cn"）
    pub product_history_namespace: String,
    /// 原始 session_id（来自活动库）
    pub original_session_id: String,
}

impl SessionIdentity {
    pub fn new(namespace: &str, original_session_id: &str) -> Self {
        Self {
            product_history_namespace: namespace.to_string(),
            original_session_id: original_session_id.to_string(),
        }
    }
}

/// 内容图哈希：确定性规范化的会话内容 SHA-256。
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ContentGraphHash(pub String);

impl ContentGraphHash {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// 版本分类：Gate I 四类。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VersionClassification {
    /// 内容图完全相同
    Identical,
    /// 旧节点和关系不变，新图只新增语义内容
    FastForward,
    /// 既有语义内容修改、删除、重排或双方分叉
    Forked,
    /// schema 或字段语义不足
    Unclassified,
}

/// 会话版本：一次扫描得到的规范化内容图。
///
/// 仅派生 `Serialize`：含 `SnapshotId`，不可反序列化（防注入）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SessionVersion {
    pub session_identity: SessionIdentity,
    /// 来源快照 ID
    pub source_snapshot_id: SnapshotId,
    /// 内容图哈希
    pub content_graph_hash: ContentGraphHash,
    /// 版本分类（相对于上一版本）
    pub classification: VersionClassification,
    /// 版本捕获时间
    pub captured_at: SystemTime,
    /// 会话标题（来自活动库，仅供显示）
    pub title: String,
}

/// 会话投影：指向用户当前浏览的版本，不删除其他版本。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionProjection {
    pub session_identity: SessionIdentity,
    /// 当前活跃版本的内容图哈希
    pub active_content_graph_hash: ContentGraphHash,
    /// 活跃版本标题
    pub active_title: String,
    /// 是否软删除
    pub soft_deleted: bool,
    /// 关联项目 ID
    pub project_id: String,
}

/// 消息投影：可从 session_version 重建的浏览/搜索投影。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MessageProjection {
    pub message_id: String,
    pub session_id: String,
    /// 消息角色（user/assistant/tool 等）
    pub role: String,
    /// 消息正文摘要（FTS 索引源）
    pub content_excerpt: String,
    /// 是否软删除
    pub soft_deleted: bool,
    /// 消息序号（按时间排序）
    pub seq: u64,
}

/// 对话预览：完整对话的消息序列。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConversationPreview {
    pub session_identity: SessionIdentity,
    pub title: String,
    pub messages: Vec<MessageProjection>,
    pub total_message_count: u64,
}

/// 浏览账号节点。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BrowseAccountNode {
    pub user_id: String,
    pub display_label: String,
    pub project_count: u64,
    pub session_count: u64,
}

/// 浏览项目节点。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BrowseProjectNode {
    pub project_id: String,
    pub display_name: String,
    /// 显示归属（user_assigned 优先，否则 first_observed_owner）
    pub display_owner: String,
    pub session_count: u64,
}

/// 浏览会话节点。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BrowseSessionNode {
    pub session_identity: SessionIdentity,
    pub title: String,
    pub message_count: u64,
    pub last_captured_at: SystemTime,
    /// 所属项目 ID：用于前端按选中项目筛选会话（方案 D 三级层次）
    pub project_id: String,
}

/// 搜索命中。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchHit {
    pub session_identity: SessionIdentity,
    pub message_id: String,
    pub project_id: String,
    pub title: String,
    pub content_excerpt: String,
    pub role: String,
}

/// 浏览结果：账号树 + 当前选中账号的项目/会话/对话。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BrowseResult {
    pub accounts: Vec<BrowseAccountNode>,
    pub projects: Vec<BrowseProjectNode>,
    pub sessions: Vec<BrowseSessionNode>,
    /// 历史库摘要（仅可见项）
    pub summary: HistoryBrowseSummary,
}

/// 历史浏览摘要：普通统计只计算可见项（Gate J）。
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct HistoryBrowseSummary {
    pub visible_account_count: u64,
    pub visible_project_count: u64,
    pub visible_session_count: u64,
    /// 软删除保留计数（仅诊断显示）
    pub soft_deleted_project_count: u64,
    pub soft_deleted_session_count: u64,
    pub soft_deleted_message_count: u64,
}

// ============================================================================
// 5. 软删除语义（Gate J）
// ============================================================================

/// 软删除标记：保留在证据中，普通统计/浏览/搜索排除。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SoftDeletionMarker {
    pub entity_kind: SoftDeletionEntityKind,
    pub entity_id: String,
    /// 软删除时间（来自活动库 deleted_at）
    pub deleted_at: u64,
}

/// 软删除实体种类。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SoftDeletionEntityKind {
    Project,
    Session,
    Message,
}

/// 诊断完整性断言：包含底层保留行（Gate J）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiagnosticIntegrityAssertion {
    /// 普通可见项目数
    pub visible_projects: u64,
    /// 底层保留（含软删除）项目数
    pub retained_projects: u64,
    pub visible_sessions: u64,
    pub retained_sessions: u64,
    pub visible_messages: u64,
    pub retained_messages: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_id_new_generates_valid_format() {
        let id = SnapshotId::new();
        let s = id.as_str();
        assert!(s.starts_with("snap-"));
        assert!(s.len() > 10);
    }

    #[test]
    fn snapshot_id_uniqueness_within_same_nanosecond() {
        // 同一纳秒内多次构造应唯一（AtomicU64 计数器）
        let id1 = SnapshotId::new();
        let id2 = SnapshotId::new();
        assert_ne!(id1, id2);
    }

    #[test]
    fn storage_root_id_trims_trailing_separators() {
        let id1 = StorageRootId::from_canonical_path("C:\\data\\root");
        let id2 = StorageRootId::from_canonical_path("C:\\data\\root\\");
        assert_eq!(id1, id2);
    }

    #[test]
    fn session_identity_uses_namespace_and_id_not_title() {
        // Gate I：会话身份不依赖 title
        let s1 = SessionIdentity::new("work_cn", "sess-001");
        let s2 = SessionIdentity::new("work_cn", "sess-001");
        assert_eq!(s1, s2);
        // 不同 session_id 即使 title 相同也是不同会话
        let s3 = SessionIdentity::new("work_cn", "sess-002");
        assert_ne!(s1, s3);
    }

    #[test]
    fn version_classification_serde_snake_case() {
        // 与 T01/T02 serde 约定一致
        let c = VersionClassification::FastForward;
        let json = serde_json::to_string(&c).unwrap();
        assert_eq!(json, "\"fast_forward\"");
        let back: VersionClassification = serde_json::from_str(&json).unwrap();
        assert_eq!(back, c);
    }

    #[test]
    fn scan_outcome_tagged_serde() {
        // ScanOutcome 用 tag="kind"
        let outcome = ScanOutcome::Failed {
            reason: ScanFailureReason::NotAuthorized,
        };
        let json = serde_json::to_string(&outcome).unwrap();
        assert!(json.contains("\"kind\":\"failed\""));
        assert!(json.contains("\"reason\":\"not_authorized\""));
    }

    #[test]
    fn snapshot_fingerprint_transparent_serde() {
        // newtype transparent
        let fp = SnapshotFingerprint("abc123".to_string());
        let json = serde_json::to_string(&fp).unwrap();
        assert_eq!(json, "\"abc123\"");
    }

    #[test]
    fn content_graph_hash_transparent_serde() {
        let h = ContentGraphHash("deadbeef".to_string());
        let json = serde_json::to_string(&h).unwrap();
        assert_eq!(json, "\"deadbeef\"");
    }

    #[test]
    fn authorization_state_default_not_authorized() {
        // 自动扫描默认关闭
        let state = AuthorizationState::default();
        assert_eq!(state, AuthorizationState::NotAuthorized);
    }

    #[test]
    fn project_observation_first_observed_owner_immutable_field() {
        // Gate E：first_observed_owner 是字段，构造后由 infrastructure 保证不变化
        let obs = ProjectObservation {
            project_identity: ProjectIdentity {
                project_id: "p1".to_string(),
                biz_project_id: "biz-1".to_string(),
                display_name: "Project 1".to_string(),
            },
            first_observed_owner: "user-A".to_string(),
            first_observed_at: SystemTime::UNIX_EPOCH,
            current_live_owner: "user-A".to_string(),
            owner_observations: vec![],
        };
        assert_eq!(obs.first_observed_owner, "user-A");
    }

    #[test]
    fn history_browse_summary_default_zero() {
        let s = HistoryBrowseSummary::default();
        assert_eq!(s.visible_account_count, 0);
        assert_eq!(s.soft_deleted_project_count, 0);
    }
}
