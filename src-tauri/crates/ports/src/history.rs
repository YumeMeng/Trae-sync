//! Trae Sync 历史库端口层：T03/T04 application 定义、infrastructure 实现的 trait 边界。
//!
//! 本模块定义 P1 历史基础所需的最小 port：
//! - `SnapshotStore`：不可变快照捕获与查询
//! - `CatalogRepository`：SQLCipher 目录库投影、浏览、搜索、版本与 owner 观察
//! - `SourceNormalizer`：Work CN 原始表到规范化历史的映射
//! - `FileIdentityProvider`：文件身份读取
//! - `ContentGraphHasher`：确定性内容图哈希
//!
//! 安全约束：
//! - 所有 port 方法不接收/返回 raw_key、认证正文或 secret
//! - raw_key 由 infrastructure 在构造时注入，application/commands 不接触
//! - fixture-only：所有方法只操作 fixture 路径，不访问真实 TRAE 数据

use std::path::Path;

use traesync_domain::{
    BrowseProjectNode, BrowseResult, BrowseSessionNode, ContentGraphHash, ConversationPreview,
    DiagnosticIntegrityAssertion, FileIdentity, HistoryBrowseSummary, MessageProjection,
    ProjectIdentity, ProjectObservation, ProjectSourceAssignment, ScanFailureReason, ScanOutcome,
    ScanRequest, SearchHit, SessionIdentity, SessionProjection, SessionVersion,
    SnapshotFingerprint, SnapshotId, SourceSnapshotMeta, VersionClassification,
};

/// 快照存储 port：捕获不可变 DB/WAL/SHM 快照并查询。
///
/// 实现约束：
/// - `capture_snapshot` 必须检查捕获前后文件集稳定性，漂移时返回 `SourceSetDrift`
/// - 发布到新 `snapshots/<snapshot_id>/`，不覆盖已发布快照
/// - 相同指纹返回 `Deduplicated`，不创建新快照
/// - 不自动删除已发布快照
pub trait SnapshotStore: Send + Sync {
    /// 捕获快照：扫描 fixture_root 下的 DB/WAL/SHM，发布到存储根。
    ///
    /// 返回 `ScanOutcome`：Success / Deduplicated / Failed。
    /// 不接收 raw_key——raw_key 由实现层在构造时注入。
    fn capture_snapshot(&self, request: &ScanRequest) -> ScanOutcome;

    /// 在快照捕获过程中复核授权；默认实现兼容旧的存储实现，
    /// 具体存储应在最终发布前再次检查，避免失效授权发布新快照。
    fn capture_snapshot_with_validation(
        &self,
        request: &ScanRequest,
        is_authorized: &dyn Fn() -> bool,
    ) -> ScanOutcome {
        if !is_authorized() {
            return ScanOutcome::Failed {
                reason: ScanFailureReason::NotAuthorized,
            };
        }
        let outcome = self.capture_snapshot(request);
        if !is_authorized() {
            return ScanOutcome::Failed {
                reason: ScanFailureReason::NotAuthorized,
            };
        }
        outcome
    }

    /// 在快照发布前后复核完整上下文；分块复制期间仍只调用廉价授权检查。
    fn capture_snapshot_with_context_validation(
        &self,
        request: &ScanRequest,
        is_authorized: &dyn Fn() -> bool,
        validate_context: &dyn Fn() -> bool,
    ) -> ScanOutcome {
        if !validate_context() {
            return ScanOutcome::Failed {
                reason: ScanFailureReason::NotAuthorized,
            };
        }
        let outcome = self.capture_snapshot_with_validation(request, is_authorized);
        if !validate_context() {
            return ScanOutcome::Failed {
                reason: ScanFailureReason::NotAuthorized,
            };
        }
        outcome
    }

    /// 查询已有快照的数据指纹，用于去重判断。
    /// 返回 (snapshot_id, fingerprint) 或 None。
    fn find_by_fingerprint(&self, fingerprint: &SnapshotFingerprint) -> Option<SnapshotId>;

    /// 读取快照元数据。
    fn read_snapshot_meta(&self, snapshot_id: &SnapshotId) -> Option<SourceSnapshotMeta>;

    /// 读取快照目录路径（用于 normalizer 读取 DB 内容）。
    fn snapshot_dir(&self, snapshot_id: &SnapshotId) -> Option<std::path::PathBuf>;
}

/// 目录库 port：SQLCipher 加密目录库的投影、浏览、搜索、版本与 owner 观察。
///
/// 实现约束：
/// - 所有写入事务化，失败不留下部分投影
/// - FTS 索引位于同一 SQLCipher 内，不生成明文旁路索引
/// - 浏览/搜索/统计排除软删除项（Gate J）
/// - owner observation 追加，first_observed_owner 不变（Gate E）
/// - 版本分类确定性，重复扫描不创建重复行（Gate I）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CatalogReadError {
    /// 目录库连接或读取材料不可用。
    Unavailable,
}

impl std::fmt::Display for CatalogReadError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unavailable => formatter.write_str("目录库不可读"),
        }
    }
}

impl std::error::Error for CatalogReadError {}

/// 来源快照读取失败；不携带数据库错误正文或敏感路径。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceReadError {
    /// 快照数据库、表结构或行映射不可用。
    Unavailable,
}

/// 一次性读取的规范化快照，避免项目、会话、消息来自不同读取状态。
#[derive(Debug, Clone, PartialEq)]
pub struct NormalizedSnapshot {
    pub projects: Vec<ProjectIdentity>,
    pub sessions: Vec<SessionProjection>,
    pub messages: Vec<MessageProjection>,
}

/// 目录库写入结果；显式区分“未提交”和“已提交但元数据待恢复”。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CatalogMutationOutcome {
    /// 业务事务和代次元数据均已完成。
    Committed,
    /// 业务事务未提交，调用方可按普通失败处理。
    NotCommitted,
    /// 业务事务已提交，但 generation.json 未完成发布；调用方不得直接重试。
    CommittedMetadataRepairRequired,
}

pub trait CatalogRepository: Send + Sync {
    /// 初始化目录库（如不存在则创建，schema 版本化）。
    /// 返回 false 表示目录库已存在且无需初始化。
    fn ensure_initialized(&self) -> bool;

    /// 投影快照到目录库：解析快照 DB，写入账号/项目/会话/消息/版本/owner 观察。
    ///
    /// 事务化：任一步骤失败回滚，不留下部分投影。
    /// `snapshot_dir` 为快照发布目录（含 database.db），由调用方通过 SnapshotStore 获取。
    fn project_snapshot(
        &self,
        snapshot_meta: &SourceSnapshotMeta,
        snapshot_dir: &Path,
        normalizer: &dyn SourceNormalizer,
    ) -> Result<(), ScanFailureReason>;

    /// 在目录库事务提交前再次确认授权，避免授权失效后返回失败但目录库已提交。
    fn project_snapshot_with_validation(
        &self,
        snapshot_meta: &SourceSnapshotMeta,
        snapshot_dir: &Path,
        normalizer: &dyn SourceNormalizer,
        is_authorized: &dyn Fn() -> bool,
    ) -> Result<(), ScanFailureReason> {
        if !is_authorized() {
            return Err(ScanFailureReason::NotAuthorized);
        }
        self.project_snapshot(snapshot_meta, snapshot_dir, normalizer)
    }

    /// 在目录库事务边界复核完整上下文；事务内部仍使用廉价授权检查响应撤销。
    fn project_snapshot_with_context_validation(
        &self,
        snapshot_meta: &SourceSnapshotMeta,
        snapshot_dir: &Path,
        normalizer: &dyn SourceNormalizer,
        is_authorized: &dyn Fn() -> bool,
        validate_context: &dyn Fn() -> bool,
    ) -> Result<(), ScanFailureReason> {
        if !validate_context() {
            return Err(ScanFailureReason::NotAuthorized);
        }
        let result = self.project_snapshot_with_validation(
            snapshot_meta,
            snapshot_dir,
            normalizer,
            is_authorized,
        );
        if result.is_ok() && !validate_context() {
            return Err(ScanFailureReason::NotAuthorized);
        }
        result
    }

    /// 浏览历史：返回账号树 + 全部项目 + 全部会话（排除软删除）。
    fn browse(&self) -> BrowseResult;

    /// 浏览历史并保留目录库不可读错误，禁止把失败伪装成空历史。
    fn browse_checked(&self) -> Result<BrowseResult, CatalogReadError> {
        Ok(self.browse())
    }

    /// 浏览指定账号的项目列表。
    fn browse_projects_by_account(&self, user_id: &str) -> Vec<BrowseProjectNode>;

    /// 浏览指定项目的会话列表（排除软删除）。
    fn browse_sessions_by_project(&self, project_id: &str) -> Vec<BrowseSessionNode>;

    /// 读取完整对话预览（排除软删除消息，但保留底层行用于诊断）。
    fn read_conversation_preview(&self, session: &SessionIdentity) -> Option<ConversationPreview>;

    /// 读取完整对话预览，并保留目录库读取错误。
    fn read_conversation_preview_checked(
        &self,
        session: &SessionIdentity,
    ) -> Result<Option<ConversationPreview>, CatalogReadError> {
        Ok(self.read_conversation_preview(session))
    }

    /// 搜索消息内容（FTS，排除软删除）。
    fn search_messages(&self, query: &str) -> Vec<SearchHit>;

    /// 搜索消息，并保留目录库读取错误。
    fn search_messages_checked(&self, query: &str) -> Result<Vec<SearchHit>, CatalogReadError> {
        Ok(self.search_messages(query))
    }

    /// 搜索指定项目中的消息；默认实现用于 fixture，生产目录库应在 SQL 查询中限定项目。
    fn search_messages_in_project(&self, query: &str, project_id: &str) -> Vec<SearchHit> {
        self.search_messages(query)
            .into_iter()
            .filter(|hit| hit.project_id == project_id)
            .collect()
    }

    /// 项目范围搜索，并保留目录库读取错误。
    fn search_messages_in_project_checked(
        &self,
        query: &str,
        project_id: &str,
    ) -> Result<Vec<SearchHit>, CatalogReadError> {
        Ok(self.search_messages_in_project(query, project_id))
    }

    /// 读取项目观察记录（含 first_observed_owner 和全部 owner observations）。
    fn read_project_observation(&self, project_id: &str) -> Option<ProjectObservation>;

    /// 读取全部项目观察（Gate E 证据用）。
    fn read_all_project_observations(&self) -> Vec<ProjectObservation>;

    /// 读取全部项目观察，并保留目录库读取错误。
    fn read_all_project_observations_checked(
        &self,
    ) -> Result<Vec<ProjectObservation>, CatalogReadError> {
        Ok(self.read_all_project_observations())
    }

    /// 读取全部会话版本（Gate I 证据用）。
    fn read_all_session_versions(&self) -> Vec<SessionVersion>;

    /// 读取会话投影。
    fn read_session_projection(&self, session: &SessionIdentity) -> Option<SessionProjection>;

    /// 读取会话投影，并保留目录库读取错误。
    fn read_session_projection_checked(
        &self,
        session: &SessionIdentity,
    ) -> Result<Option<SessionProjection>, CatalogReadError> {
        Ok(self.read_session_projection(session))
    }

    /// 用户来源分配：仅改变 display_owner 分类，不修改观察或快照（Gate E）。
    fn assign_project_source(&self, assignment: &ProjectSourceAssignment)
        -> CatalogMutationOutcome;

    /// 读取用户来源分配。
    fn read_project_source_assignment(&self, project_id: &str) -> Option<ProjectSourceAssignment>;

    /// 诊断完整性断言：包含底层保留行（Gate J）。
    fn diagnostic_integrity(&self) -> DiagnosticIntegrityAssertion;

    /// 历史浏览摘要（仅可见项）。
    fn history_summary(&self) -> HistoryBrowseSummary;

    /// 历史浏览摘要，并保留目录库读取错误。
    fn history_summary_checked(&self) -> Result<HistoryBrowseSummary, CatalogReadError> {
        Ok(self.history_summary())
    }
}

/// 来源 normalizer port：从快照 DB 读取并规范化项目/会话/消息。
///
/// Work CN 实现负责原始表到规范化历史的映射。
/// 每次调用读取快照目录下的 database.db（只读）。
pub trait SourceNormalizer: Send + Sync {
    /// 以 checked 语义读取同一快照的全部规范化数据。
    ///
    /// 默认实现兼容旧 fixture normalizer；生产适配器应覆盖此方法，
    /// 将表缺失、SQL 失败和行映射失败返回为 `Err`，禁止静默变成空集合。
    fn read_snapshot_checked(
        &self,
        snapshot_dir: &Path,
    ) -> Result<NormalizedSnapshot, SourceReadError> {
        Ok(NormalizedSnapshot {
            projects: self.read_projects(snapshot_dir),
            sessions: self.read_session_projections(snapshot_dir),
            messages: self.read_messages(snapshot_dir),
        })
    }

    /// 读取快照中的全部项目身份。
    fn read_projects(&self, snapshot_dir: &Path) -> Vec<ProjectIdentity>;

    /// 读取快照中的全部会话投影（含 project_id 关联）。
    fn read_session_projections(&self, snapshot_dir: &Path) -> Vec<SessionProjection>;

    /// 读取快照中的全部消息投影（含 soft_deleted 标记）。
    fn read_messages(&self, snapshot_dir: &Path) -> Vec<MessageProjection>;

    /// 读取项目 owner（活动库 project.user_id）。
    fn read_project_owner(&self, snapshot_dir: &Path, project_id: &str) -> Option<String>;

    /// 计算会话内容图哈希（确定性规范化）。
    fn compute_content_graph_hash(
        &self,
        snapshot_dir: &Path,
        session: &SessionIdentity,
    ) -> Option<ContentGraphHash>;
}

/// 文件身份提供者 port：读取文件身份用于漂移检测。
///
/// Windows 实现使用 GetFileInformationByHandle。
pub trait FileIdentityProvider: Send + Sync {
    /// 读取文件身份。文件不存在时返回 None。
    fn read_file_identity(&self, path: &Path) -> Option<FileIdentity>;
}

/// 内容图哈希器 port：计算确定性内容图哈希。
///
/// 确定性要求（Gate I）：
/// - SQLite 行顺序不影响哈希（按稳定 ID 排序）
/// - JSON 对象键顺序不影响哈希（按键排序后哈希）
/// - 时间戳/缓存/FTS/派生字段不进入哈希
pub trait ContentGraphHasher: Send + Sync {
    /// 计算会话内容图哈希。
    fn hash_session_content(
        &self,
        messages: &[MessageProjection],
        session: &SessionIdentity,
    ) -> ContentGraphHash;

    /// 比较两个内容图哈希并返回版本分类。
    ///
    /// - 完全相同 -> Identical
    /// - 旧消息不变且只新增 -> FastForward
    /// - 修改/删除/重排/双分支 -> Forked
    /// - 无法判定 -> Unclassified
    fn classify(
        &self,
        old_messages: &[MessageProjection],
        new_messages: &[MessageProjection],
    ) -> VersionClassification;
}
