//! Fixture 路径保护：强制 fixture_root 位于独立测试根内，写目标只能位于 fixture_root 内部。
//!
//! 对应 AC4、AC5 与 R1（第二次修复）要求：
//! - 生产公开 API 只接受 `fixture_root`；可信测试根与默认受保护路径由内部唯一
//!   locator（`SystemRoots::from_env`）解析并失败关闭。
//! - `SystemRoots`、`PathPolicy` 不出现在公开 API；外部调用者无法构造、注入或
//!   修改"可信测试根"或"默认受保护路径"。
//! - 合成构造器仅在 `#[cfg(test)]` 模块内可见，仅用于单元测试纯逻辑核心。
//! - 集成测试通过唯一生产入口 `FixturePathGuard::new(fixture_root)` 验证，
//!   通过 RAII guard 临时设置 APPDATA/LOCALAPPDATA 构造可信测试根。
//!
//! 【AC5 强制声明】本模块不包含任何 `#[cfg(test)]` 旁路、feature flag、
//! 环境变量开关或条件编译跳过生产验证逻辑。`#[cfg(test)]` 仅隔离测试辅助
//! 构造器，不改变或跳过生产验证路径。任何尝试添加旁路都应被视为规格违反。

use std::path::{Component, Path, PathBuf};

use crate::operation_lease::{OperationLease, OperationLeaseError};

/// 默认 Work CN 活动数据库相对 APPDATA 的路径片段
const DEFAULT_WORK_CN_REL: &[&str] = &["TRAE SOLO CN", "ModularData", "ai-agent", "database.db"];

/// 测试根相对 LOCALAPPDATA 的路径片段
const TEST_ROOT_REL: &[&str] = &["Trae Sync", "tests"];

/// 所有实例共用的固定恢复区命名空间。
const RECOVERY_ROOT_REL: &[&str] = &["Trae Sync", "recovery"];

/// 系统根解析错误：失败关闭，绝不静默放行。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SystemRootsError {
    /// APPDATA 环境变量未设置，无法解析默认 Work CN 父目录
    AppdataMissing,
    /// LOCALAPPDATA 未设置或测试根不存在
    TestRootUnavailable { raw: String, source: String },
}

impl std::fmt::Display for SystemRootsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::AppdataMissing => {
                write!(f, "APPDATA 环境变量未设置，无法解析默认 Work CN 父目录")
            }
            Self::TestRootUnavailable { raw, source } => {
                write!(f, "测试根不可用: {raw} ({source})")
            }
        }
    }
}

impl std::error::Error for SystemRootsError {}

/// 系统根配置：描述可信测试根与默认 Work CN 父目录。
///
/// 【R1 修复】字段私有，仅由内部 `from_env` locator 构造。
/// 外部调用者无法构造、注入或修改信任输入。
#[derive(Debug, Clone)]
pub(crate) struct SystemRoots {
    /// 可信测试根：fixture_root 必须严格位于其内部（规范化后）
    test_root: PathBuf,
    /// 默认 Work CN 活动数据库的父目录（如 `%APPDATA%\TRAE SOLO CN\ModularData\ai-agent`）。
    /// None 表示该平台无默认路径（如非 Windows）。
    default_work_cn_dir: Option<PathBuf>,
    /// 所有实例共享的恢复区命名空间；不接受调用方自定义根目录。
    recovery_root: PathBuf,
}

impl SystemRoots {
    /// 从进程环境变量解析系统根（唯一生产构造入口）。
    ///
    /// Windows 上 APPDATA/LOCALAPPDATA 缺失或测试根不可达时返回 Err（失败关闭）。
    /// 测试根默认为 `%LOCALAPPDATA%\Trae Sync\tests`，必须存在且可规范化。
    fn from_env() -> Result<Self, SystemRootsError> {
        // APPDATA 必须存在——缺失即失败关闭
        let appdata = std::env::var_os("APPDATA").ok_or(SystemRootsError::AppdataMissing)?;

        let mut default_dir = PathBuf::from(appdata);
        for segment in DEFAULT_WORK_CN_REL {
            default_dir.push(segment);
        }
        // default_work_cn_dir 是 database.db 的父目录（ai-agent 目录）
        let default_work_cn_dir = default_dir.parent().map(Path::to_path_buf);

        // LOCALAPPDATA 必须存在——缺失即失败关闭
        let local_appdata = std::env::var_os("LOCALAPPDATA").ok_or_else(|| {
            SystemRootsError::TestRootUnavailable {
                raw: "%LOCALAPPDATA%".to_string(),
                source: "LOCALAPPDATA 环境变量未设置".to_string(),
            }
        })?;

        let mut recovery_root = PathBuf::from(&local_appdata);
        for segment in RECOVERY_ROOT_REL {
            recovery_root.push(segment);
        }

        let mut test_root = PathBuf::from(local_appdata);
        for segment in TEST_ROOT_REL {
            test_root.push(segment);
        }

        // 测试根必须存在且可规范化——失败关闭
        let canonical_test_root =
            test_root
                .canonicalize()
                .map_err(|e| SystemRootsError::TestRootUnavailable {
                    raw: test_root.to_string_lossy().into_owned(),
                    source: e.to_string(),
                })?;

        Ok(Self {
            test_root: canonical_test_root,
            default_work_cn_dir,
            recovery_root,
        })
    }
}

/// 路径验证错误
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FixturePathError {
    /// 候选路径命中默认 Work CN 活动路径
    DefaultWorkCnPath {
        raw: String,
        default_pattern: String,
    },
    /// 候选路径位于 fixture_root 之外
    OutsideFixtureRoot { raw: String },
    /// fixture_root 位于可信测试根之外
    FixtureRootOutsideTestRoot { raw: String },
    /// fixture 存储根位于可信测试根之外
    StorageRootOutsideTestRoot { raw: String },
    /// 恢复区位于固定共享命名空间之外
    RecoveryRootOutsideNamespace { raw: String },
    /// 候选路径或其父目录不存在，无法规范化
    CannotCanonicalize { raw: String, source: String },
    /// 候选路径没有父目录
    NoParent,
    /// 候选路径没有文件名
    NoFileName,
    /// fixture_root 自身命中默认 Work CN 路径
    FixtureRootIsDefaultWorkCnPath { raw: String },
    /// 系统根解析失败
    SystemRoots(SystemRootsError),
    /// 共享操作租约获取失败
    OperationLeaseUnavailable { source: String },
    /// R2：相对路径为空
    EmptyRelativePath,
    /// R2：相对路径是绝对路径（如 `C:\` 或 `/etc/passwd`）
    AbsolutePathRejected { raw: String },
    /// R2：相对路径包含 `..` 父目录遍历组件
    ParentTraversalRejected { raw: String },
}

impl std::fmt::Display for FixturePathError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DefaultWorkCnPath {
                raw,
                default_pattern,
            } => {
                write!(
                    f,
                    "候选路径命中默认 Work CN 活动路径: {raw} 命中 {default_pattern}"
                )
            }
            Self::OutsideFixtureRoot { raw } => {
                write!(f, "候选路径位于 fixture_root 之外: {raw}")
            }
            Self::FixtureRootOutsideTestRoot { raw } => {
                write!(f, "fixture_root 位于可信测试根之外: {raw}")
            }
            Self::StorageRootOutsideTestRoot { raw } => {
                write!(f, "fixture 存储根位于可信测试根之外: {raw}")
            }
            Self::RecoveryRootOutsideNamespace { raw } => {
                write!(f, "恢复区位于固定共享命名空间之外: {raw}")
            }
            Self::CannotCanonicalize { raw, source } => {
                write!(f, "无法规范化路径: {raw} ({source})")
            }
            Self::NoParent => write!(f, "候选路径没有父目录"),
            Self::NoFileName => write!(f, "候选路径没有文件名"),
            Self::FixtureRootIsDefaultWorkCnPath { raw } => {
                write!(f, "fixture_root 自身命中默认 Work CN 路径: {raw}")
            }
            Self::SystemRoots(e) => write!(f, "系统根解析失败: {e}"),
            Self::OperationLeaseUnavailable { source } => {
                write!(f, "共享操作租约不可用: {source}")
            }
            Self::EmptyRelativePath => write!(f, "数据库相对路径为空"),
            Self::AbsolutePathRejected { raw } => {
                write!(f, "数据库相对路径是绝对路径: {raw}")
            }
            Self::ParentTraversalRejected { raw } => {
                write!(f, "数据库相对路径包含父目录遍历 (`..`): {raw}")
            }
        }
    }
}

impl std::error::Error for FixturePathError {}

impl FixturePathError {
    /// 返回跨 IPC 边界可公开的稳定错误码；不携带原始路径或底层 I/O 文本。
    pub const fn code(&self) -> &'static str {
        match self {
            Self::DefaultWorkCnPath { .. } | Self::FixtureRootIsDefaultWorkCnPath { .. } => {
                "default_work_cn_path_rejected"
            }
            Self::OutsideFixtureRoot { .. } => "fixture_target_outside_root",
            Self::FixtureRootOutsideTestRoot { .. } => "fixture_root_outside_test_root",
            Self::StorageRootOutsideTestRoot { .. } => "storage_root_outside_test_root",
            Self::RecoveryRootOutsideNamespace { .. } => "recovery_root_outside_namespace",
            Self::CannotCanonicalize { .. } => "fixture_path_unavailable",
            Self::NoParent => "fixture_path_invalid",
            Self::NoFileName => "fixture_path_invalid",
            Self::SystemRoots(_) => "fixture_system_roots_unavailable",
            Self::OperationLeaseUnavailable { .. } => "operation_lease_unavailable",
            Self::EmptyRelativePath => "database_relative_path_empty",
            Self::AbsolutePathRejected { .. } => "database_relative_path_absolute",
            Self::ParentTraversalRejected { .. } => "database_relative_path_traversal",
        }
    }
}

/// Tauri command 边界统一使用脱敏错误文本；详细路径仅允许留在受控本地诊断日志。
pub fn fixture_path_error_text(error: &FixturePathError) -> String {
    format!("fixture_path_error:{}", error.code())
}

impl From<SystemRootsError> for FixturePathError {
    fn from(e: SystemRootsError) -> Self {
        Self::SystemRoots(e)
    }
}

/// 路径策略：纯逻辑核心，无进程全局副作用。
///
/// 【R1 修复】改为 `pub(crate)`，不公开。外部调用者无法构造此结构
/// 注入伪造的信任输入。
#[derive(Debug, Clone)]
pub(crate) struct PathPolicy {
    system_roots: SystemRoots,
}

impl PathPolicy {
    /// 用给定系统根构造策略。
    fn new(system_roots: SystemRoots) -> Self {
        Self { system_roots }
    }

    /// 返回可信测试根的规范化路径。
    fn canonical_test_root(&self) -> Result<PathBuf, FixturePathError> {
        // system_roots.test_root 应已规范化；这里防御性再规范化一次
        self.system_roots.test_root.canonicalize().map_err(|e| {
            FixturePathError::CannotCanonicalize {
                raw: self.system_roots.test_root.to_string_lossy().into_owned(),
                source: format!("test_root 规范化失败: {e}"),
            }
        })
    }

    /// 验证 fixture_root 是否安全：必须位于可信测试根内部，且不命中默认 Work CN 路径。
    ///
    /// 返回规范化后的 fixture_root。
    fn validate_fixture_root(&self, fixture_root: &Path) -> Result<PathBuf, FixturePathError> {
        // 1. 拒绝 fixture_root 命中默认 Work CN 路径（词法检查）
        if is_default_work_cn_path(
            fixture_root,
            self.system_roots.default_work_cn_dir.as_deref(),
        ) {
            return Err(FixturePathError::FixtureRootIsDefaultWorkCnPath {
                raw: fixture_root.to_string_lossy().into_owned(),
            });
        }

        // 2. 规范化 fixture_root（要求路径存在，跟随符号链接）
        let canonical_fixture_root =
            fixture_root
                .canonicalize()
                .map_err(|e| FixturePathError::CannotCanonicalize {
                    raw: fixture_root.to_string_lossy().into_owned(),
                    source: e.to_string(),
                })?;

        // 3. 二次检查：规范化后的路径也不得命中默认 Work CN 路径
        if is_default_work_cn_path(
            &canonical_fixture_root,
            self.system_roots.default_work_cn_dir.as_deref(),
        ) {
            return Err(FixturePathError::FixtureRootIsDefaultWorkCnPath {
                raw: canonical_fixture_root.to_string_lossy().into_owned(),
            });
        }

        // 4. 必须严格位于可信测试根内部
        let canonical_test_root = self.canonical_test_root()?;
        if !path_strictly_inside(&canonical_fixture_root, &canonical_test_root) {
            return Err(FixturePathError::FixtureRootOutsideTestRoot {
                raw: canonical_fixture_root.to_string_lossy().into_owned(),
            });
        }

        Ok(canonical_fixture_root)
    }

    /// 验证候选写目标是否安全：必须位于 fixture_root 内部，且不命中默认 Work CN 路径。
    ///
    /// `fixture_root` 必须是已规范化的路径（通常由 `validate_fixture_root` 返回）。
    fn validate_write_target(
        &self,
        fixture_root: &Path,
        candidate: &Path,
    ) -> Result<PathBuf, FixturePathError> {
        let raw = candidate.to_string_lossy().into_owned();

        // 1. 拒绝默认 Work CN 活动路径（词法检查，不依赖路径存在）
        if is_default_work_cn_path(candidate, self.system_roots.default_work_cn_dir.as_deref()) {
            return Err(FixturePathError::DefaultWorkCnPath {
                raw,
                default_pattern: default_work_cn_display(
                    self.system_roots.default_work_cn_dir.as_deref(),
                ),
            });
        }

        // 2. 规范化候选路径（跟随符号链接、解析 `..`、统一大小写与分隔符）
        let canonical_candidate = canonicalize_or_parent(candidate)?;

        // 3. 拒绝规范化后位于 fixture_root 之外的路径
        if !path_strictly_inside(&canonical_candidate, fixture_root) {
            return Err(FixturePathError::OutsideFixtureRoot { raw });
        }

        // 4. 防御性二次检查：规范化后的路径也不得命中默认 Work CN 路径
        if is_default_work_cn_path(
            &canonical_candidate,
            self.system_roots.default_work_cn_dir.as_deref(),
        ) {
            return Err(FixturePathError::DefaultWorkCnPath {
                raw,
                default_pattern: default_work_cn_display(
                    self.system_roots.default_work_cn_dir.as_deref(),
                ),
            });
        }

        Ok(canonical_candidate)
    }

    /// 验证固定恢复区位于可信测试根内，禁止把备份或 manifest 写到真实用户目录。
    fn validate_fixture_storage_root(&self, candidate: &Path) -> Result<PathBuf, FixturePathError> {
        let canonical_candidate =
            candidate
                .canonicalize()
                .map_err(|e| FixturePathError::CannotCanonicalize {
                    raw: candidate.to_string_lossy().into_owned(),
                    source: e.to_string(),
                })?;
        let canonical_test_root = self.canonical_test_root()?;
        if !path_strictly_inside(&canonical_candidate, &canonical_test_root) {
            return Err(FixturePathError::StorageRootOutsideTestRoot {
                raw: canonical_candidate.to_string_lossy().into_owned(),
            });
        }
        if is_default_work_cn_path(
            &canonical_candidate,
            self.system_roots.default_work_cn_dir.as_deref(),
        ) {
            return Err(FixturePathError::DefaultWorkCnPath {
                raw: canonical_candidate.to_string_lossy().into_owned(),
                default_pattern: default_work_cn_display(
                    self.system_roots.default_work_cn_dir.as_deref(),
                ),
            });
        }
        Ok(canonical_candidate)
    }

    /// 验证恢复区只能位于 LOCALAPPDATA 下的固定共享命名空间。
    ///
    /// 候选路径可以尚不存在，调用方随后负责创建；但其父目录必须已经存在，
    /// 这样不会通过不存在路径或符号链接把锁写到任意位置。
    fn validate_shared_recovery_root(&self, candidate: &Path) -> Result<PathBuf, FixturePathError> {
        let namespace_root = canonicalize_or_parent(&self.system_roots.recovery_root)?;
        let canonical_candidate = canonicalize_or_parent(candidate)?;
        let is_namespace_root = canonical_candidate == namespace_root;
        if !is_namespace_root && !path_strictly_inside(&canonical_candidate, &namespace_root) {
            return Err(FixturePathError::RecoveryRootOutsideNamespace {
                raw: candidate.to_string_lossy().into_owned(),
            });
        }
        Ok(canonical_candidate)
    }
}

/// Fixture 路径守卫：构造时固定 fixture_root，后续验证写目标。
///
/// 【R1 修复】唯一生产公开构造入口只接受 `fixture_root`。
/// 可信测试根与默认受保护路径由内部 `SystemRoots::from_env` locator 解析，
/// 外部调用者无法注入或修改。
pub struct FixturePathGuard {
    policy: PathPolicy,
    canonical_fixture_root: PathBuf,
}

impl std::fmt::Debug for FixturePathGuard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FixturePathGuard")
            .field("canonical_fixture_root", &self.canonical_fixture_root)
            .finish()
    }
}

impl FixturePathGuard {
    /// 创建守卫（唯一生产公开入口）。
    ///
    /// `fixture_root` 必须存在、可规范化、严格位于内部 locator 解析的可信测试根内部，
    /// 且不命中默认 Work CN 路径。可信测试根与默认受保护路径由内部 locator 从
    /// APPDATA/LOCALAPPDATA 解析；APPDATA/LOCALAPPDATA 缺失或测试根不可达时失败关闭。
    ///
    /// 外部调用者无法传入伪造的信任输入——只传入 fixture_root。
    pub fn new(fixture_root: &Path) -> Result<Self, FixturePathError> {
        let system_roots = SystemRoots::from_env()?;
        let policy = PathPolicy::new(system_roots);
        let canonical_fixture_root = policy.validate_fixture_root(fixture_root)?;
        Ok(Self {
            policy,
            canonical_fixture_root,
        })
    }

    /// 验证候选写目标是否安全。
    pub fn validate_write_target(&self, candidate: &Path) -> Result<PathBuf, FixturePathError> {
        self.policy
            .validate_write_target(&self.canonical_fixture_root, candidate)
    }

    /// 验证备份和 manifest 存储根仍位于可信 fixture 测试根，且必须预先存在。
    pub fn validate_fixture_storage_root(
        &self,
        candidate: &Path,
    ) -> Result<PathBuf, FixturePathError> {
        self.policy.validate_fixture_storage_root(candidate)
    }

    /// 验证恢复区属于所有实例共享的固定命名空间。
    pub fn validate_shared_recovery_root(
        &self,
        candidate: &Path,
    ) -> Result<PathBuf, FixturePathError> {
        self.policy.validate_shared_recovery_root(candidate)
    }

    /// 先验证固定恢复区，再取得跨进程租约；调用方不能注入任意锁目录。
    pub fn acquire_operation_lease(
        &self,
        recovery_root: &Path,
        data_location_id: &str,
    ) -> Result<OperationLease, FixturePathError> {
        let recovery_root = self.policy.validate_shared_recovery_root(recovery_root)?;
        OperationLease::acquire(&recovery_root, data_location_id).map_err(|error| {
            FixturePathError::OperationLeaseUnavailable {
                source: match error {
                    OperationLeaseError::CatalogBusy => "目录库锁忙".to_string(),
                    OperationLeaseError::DataLocationBusy => "数据位置锁忙".to_string(),
                    OperationLeaseError::LockDirectoryUnavailable => "锁目录不可用".to_string(),
                    OperationLeaseError::InvalidLocationId => "数据位置标识无效".to_string(),
                    OperationLeaseError::InvalidStorageRoot => "存储根无效".to_string(),
                    OperationLeaseError::LeaseContextUnbound => "租约未绑定存储根".to_string(),
                    OperationLeaseError::StorageRootMismatch => "租约与存储根不匹配".to_string(),
                },
            }
        })
    }

    /// 取得与 fixture 存储根绑定的目录库租约；目录库 mutation/reconcile 必须使用此入口。
    pub fn acquire_catalog_operation_lease(
        &self,
        recovery_root: &Path,
        storage_root: &Path,
        data_location_id: &str,
    ) -> Result<OperationLease, FixturePathError> {
        let recovery_root = self.policy.validate_shared_recovery_root(recovery_root)?;
        let storage_root = self.policy.validate_fixture_storage_root(storage_root)?;
        OperationLease::acquire_bound(&recovery_root, &storage_root, data_location_id).map_err(
            |error| FixturePathError::OperationLeaseUnavailable {
                source: match error {
                    OperationLeaseError::CatalogBusy => "目录库锁忙".to_string(),
                    OperationLeaseError::DataLocationBusy => "数据位置锁忙".to_string(),
                    OperationLeaseError::LockDirectoryUnavailable => "锁目录不可用".to_string(),
                    OperationLeaseError::InvalidLocationId => "数据位置标识无效".to_string(),
                    OperationLeaseError::InvalidStorageRoot => "存储根无效".to_string(),
                    OperationLeaseError::LeaseContextUnbound => "租约未绑定存储根".to_string(),
                    OperationLeaseError::StorageRootMismatch => "租约与存储根不匹配".to_string(),
                },
            },
        )
    }

    /// 返回规范化后的 fixture_root（仅供诊断使用）
    pub fn canonical_root(&self) -> &Path {
        &self.canonical_fixture_root
    }

    /// R2：验证 `db_relative_path` 与派生的 WAL/SHM 路径全部封闭在 fixture_root 内部。
    ///
    /// 强制规则（对应 handoff R2）：
    /// - `db_relative_path` 不能为空
    /// - `db_relative_path` 不能是绝对路径（词法检查拒绝 `C:\`、`/`、`\\?\` 前缀等）
    /// - `db_relative_path` 不能包含 `..` 组件（词法检查拒绝父目录遍历）
    /// - 规范化 `fixture_root.join(db_relative_path)` 必须严格位于 fixture_root 内部
    /// - 同样的封闭证明应用到 `db_relative_path-wal` 与 `db_relative_path-shm`
    /// - 跟随符号链接/junction 后逃逸 fixture_root 的路径必须被拒绝
    ///
    /// DB 文件必须存在（用于规范化跟随符号链接）；WAL/SHM 可能不存在，
    /// 不存在时跳过封闭校验，存在时必须通过封闭校验。
    ///
    /// 返回 DB 的规范化绝对路径。WAL/SHM 路径可由调用方通过 `db_path.with_extension(...)`
    /// 或字符串拼接派生——它们已经过同样的封闭证明。
    pub fn validate_db_relative_path(
        &self,
        db_relative_path: &str,
    ) -> Result<PathBuf, FixturePathError> {
        validate_db_relative_path_inside(&self.canonical_fixture_root, db_relative_path)
    }
}

/// R2：纯词法检查——拒绝空、绝对路径、包含 `..` 组件的相对路径。
///
/// 不依赖文件系统状态，可在任何层（commands/application/infrastructure）调用。
/// 返回 `Ok(())` 表示路径词法安全；`Err(_)` 表示必须拒绝。
///
/// 公开为 `pub(crate)` 以便同 crate 的 `snapshot_store`、`account_evidence` 等模块复用，
/// 不暴露到 crate 外，避免外部调用者依赖此内部规则。
pub(crate) fn reject_unsafe_relative_path(relative_path: &str) -> Result<(), FixturePathError> {
    if relative_path.is_empty() {
        return Err(FixturePathError::EmptyRelativePath);
    }
    let p = Path::new(relative_path);
    if p.is_absolute() {
        return Err(FixturePathError::AbsolutePathRejected {
            raw: relative_path.to_string(),
        });
    }
    for component in p.components() {
        match component {
            Component::ParentDir => {
                return Err(FixturePathError::ParentTraversalRejected {
                    raw: relative_path.to_string(),
                });
            }
            Component::Prefix(_) | Component::RootDir => {
                return Err(FixturePathError::AbsolutePathRejected {
                    raw: relative_path.to_string(),
                });
            }
            _ => {}
        }
    }
    Ok(())
}

/// R2：验证 `db_relative_path` 与派生 WAL/SHM 全部封闭在已规范化的 fixture_root 内部。
///
/// `canonical_fixture_root` 应为已规范化的绝对路径（由 `FixturePathGuard::new` 保证）。
/// 此函数为 defense-in-depth，会再次规范化 `canonical_fixture_root`——
/// 即使调用方传入非规范化路径（如测试 fixture 的 tempdir 路径）也能正确比较。
///
/// 此函数为 `pub(crate)`，供同 crate 的 `snapshot_store` 等模块在 defense-in-depth 路径
/// 上复用同一份封闭证明，避免在 `snapshot_store` 内重复实现较弱校验。
///
/// 返回 DB 的规范化绝对路径。WAL/SHM 不存在时跳过；存在时必须通过封闭校验。
pub(crate) fn validate_db_relative_path_inside(
    canonical_fixture_root: &Path,
    db_relative_path: &str,
) -> Result<PathBuf, FixturePathError> {
    // 1. 词法检查 DB 路径
    reject_unsafe_relative_path(db_relative_path)?;

    // 2. 派生 WAL/SHM 相对路径并执行同样的词法检查——
    //    防止 `db_relative_path` 本身安全但派生路径逃逸（如 `db-wal` 在父目录）
    let wal_relative_path = format!("{}-wal", db_relative_path);
    let shm_relative_path = format!("{}-shm", db_relative_path);
    reject_unsafe_relative_path(&wal_relative_path)?;
    reject_unsafe_relative_path(&shm_relative_path)?;

    // 3. defense-in-depth：再次规范化 fixture_root——
    //    调用方应传入已规范化路径，但此函数不信任调用方，独立完成规范化以保证比较正确
    let canonical_root = canonical_fixture_root.canonicalize().map_err(|e| {
        FixturePathError::CannotCanonicalize {
            raw: canonical_fixture_root.to_string_lossy().into_owned(),
            source: format!("fixture_root 规范化失败: {e}"),
        }
    })?;

    // 4. DB 必须存在——canonicalize 会跟随符号链接，暴露 symlink/junction 逃逸
    let db_candidate = canonical_root.join(db_relative_path);
    let canonical_db =
        db_candidate
            .canonicalize()
            .map_err(|e| FixturePathError::CannotCanonicalize {
                raw: db_candidate.to_string_lossy().into_owned(),
                source: e.to_string(),
            })?;

    // 5. DB 规范化后必须严格位于 fixture_root 内部——
    //    即使词法检查通过，符号链接/junction 仍可能让规范化路径逃逸
    if !path_strictly_inside(&canonical_db, &canonical_root) {
        return Err(FixturePathError::OutsideFixtureRoot {
            raw: db_relative_path.to_string(),
        });
    }

    // 6. WAL/SHM 可能不存在；若存在则规范化并验证封闭性
    for rel in [&wal_relative_path, &shm_relative_path] {
        let candidate = canonical_root.join(rel);
        if candidate.exists() {
            let canonical =
                candidate
                    .canonicalize()
                    .map_err(|e| FixturePathError::CannotCanonicalize {
                        raw: candidate.to_string_lossy().into_owned(),
                        source: e.to_string(),
                    })?;
            if !path_strictly_inside(&canonical, &canonical_root) {
                return Err(FixturePathError::OutsideFixtureRoot {
                    raw: rel.to_string(),
                });
            }
        }
    }

    Ok(canonical_db)
}

/// 判断 `inner` 是否严格位于 `outer` 内部（不含 outer 自身）。
///
/// R2-1：改为 `pub(crate)` 以便同 crate 内的 `account_evidence` 模块复用，
/// 封闭账号证据读取路径的 symlink/junction fixture 逃逸。
/// 仍不公开到 crate 外，避免外部调用者依赖此内部比较逻辑。
pub(crate) fn path_strictly_inside(inner: &Path, outer: &Path) -> bool {
    let outer_components: Vec<_> = outer
        .components()
        .filter_map(|c| match c {
            Component::Normal(s) => Some(s.to_string_lossy().to_lowercase()),
            Component::RootDir => Some(String::from("\\")),
            Component::Prefix(p) => Some(p.as_os_str().to_string_lossy().to_lowercase()),
            _ => None,
        })
        .collect();
    let inner_components: Vec<_> = inner
        .components()
        .filter_map(|c| match c {
            Component::Normal(s) => Some(s.to_string_lossy().to_lowercase()),
            Component::RootDir => Some(String::from("\\")),
            Component::Prefix(p) => Some(p.as_os_str().to_string_lossy().to_lowercase()),
            _ => None,
        })
        .collect();

    if inner_components.len() <= outer_components.len() {
        return false;
    }
    outer_components
        .iter()
        .zip(inner_components.iter())
        .all(|(o, i)| o == i)
}

/// 规范化候选路径：若路径存在则直接规范化；若不存在则规范化父目录后拼接文件名。
fn canonicalize_or_parent(candidate: &Path) -> Result<PathBuf, FixturePathError> {
    if let Ok(canon) = candidate.canonicalize() {
        return Ok(canon);
    }

    let parent = candidate.parent().ok_or(FixturePathError::NoParent)?;
    let file_name = candidate.file_name().ok_or(FixturePathError::NoFileName)?;

    let canon_parent = parent
        .canonicalize()
        .map_err(|e| FixturePathError::CannotCanonicalize {
            raw: candidate.to_string_lossy().into_owned(),
            source: format!("父目录规范化失败: {e}"),
        })?;

    Ok(canon_parent.join(file_name))
}

/// 默认 Work CN 路径的可读展示（用于错误消息）
fn default_work_cn_display(default_work_cn_dir: Option<&Path>) -> String {
    match default_work_cn_dir {
        Some(dir) => dir.join("database.db").to_string_lossy().into_owned(),
        None => "%APPDATA%\\TRAE SOLO CN\\ModularData\\ai-agent\\database.db".to_string(),
    }
}

/// 判断候选路径是否命中默认 Work CN 活动路径或其祖先目录。
fn is_default_work_cn_path(candidate: &Path, default_work_cn_dir: Option<&Path>) -> bool {
    let Some(default_dir) = default_work_cn_dir else {
        return false;
    };
    let default_db = default_dir.join("database.db");

    let candidate_norm = normalize_for_compare(candidate);
    let default_norm = normalize_for_compare(&default_db);
    let default_dir_norm = normalize_for_compare(default_dir);

    candidate_norm == default_norm || candidate_norm.starts_with(&default_dir_norm)
}

/// 词法规范化路径用于比较：小写化、统一分隔符为 `\`、去除 `\\?\` 前缀、去除尾分隔符。
fn normalize_for_compare(path: &Path) -> PathBuf {
    let s = path.to_string_lossy().replace('/', "\\");

    let s = s.strip_prefix(r"\\?\").unwrap_or(&s);

    let s = s.trim_end_matches('\\');
    let s = if s.len() == 2 && s.as_bytes()[1] == b':' {
        format!("{s}\\")
    } else {
        s.to_string()
    };

    PathBuf::from(s.to_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    /// 辅助：在给定 root 下创建子目录并返回其路径
    fn make_subdir(root: &Path, name: &str) -> PathBuf {
        let p = root.join(name);
        fs::create_dir_all(&p).unwrap();
        p
    }

    /// 辅助：构造合成系统根（仅 `#[cfg(test)]` 可见，不公开到 crate 外）。
    /// `test_root` 用 tempdir，`default_work_cn_dir` 用另一个 tempdir。
    fn synthetic_roots() -> (tempfile::TempDir, tempfile::TempDir, SystemRoots) {
        let test_root = tempdir().unwrap();
        let appdata = tempdir().unwrap();
        let default_dir = appdata
            .path()
            .join("TRAE SOLO CN")
            .join("ModularData")
            .join("ai-agent");
        fs::create_dir_all(&default_dir).unwrap();
        let roots = SystemRoots {
            test_root: test_root.path().to_path_buf(),
            default_work_cn_dir: Some(default_dir),
            recovery_root: test_root.path().join("recovery"),
        };
        (test_root, appdata, roots)
    }

    #[test]
    fn accept_path_inside_fixture_root_inside_test_root() {
        let (test_root, _appdata, roots) = synthetic_roots();
        let fixture_root = make_subdir(test_root.path(), "fixture");
        let policy = PathPolicy::new(roots);
        let canonical_fixture_root = policy.validate_fixture_root(&fixture_root).unwrap();
        let candidate = fixture_root.join("data").join("file.db");
        make_subdir(&fixture_root, "data");
        let result = policy
            .validate_write_target(&canonical_fixture_root, &candidate)
            .unwrap();
        assert!(result.starts_with(&canonical_fixture_root));
    }

    #[test]
    fn shared_recovery_root_rejects_sibling_namespace() {
        let (test_root, _appdata, roots) = synthetic_roots();
        let recovery_root = test_root.path().join("recovery");
        fs::create_dir_all(&recovery_root).unwrap();
        let policy = PathPolicy::new(roots);

        assert!(policy
            .validate_shared_recovery_root(&recovery_root.join("instance-a"))
            .is_ok());
        assert!(matches!(
            policy.validate_shared_recovery_root(&test_root.path().join("recovery-other")),
            Err(FixturePathError::RecoveryRootOutsideNamespace { .. })
        ));
    }

    #[test]
    fn reject_fixture_root_outside_test_root() {
        let (_test_root, _appdata, roots) = synthetic_roots();
        let outside = tempdir().unwrap();
        let policy = PathPolicy::new(roots);
        let err = policy.validate_fixture_root(outside.path()).unwrap_err();
        assert!(matches!(
            err,
            FixturePathError::FixtureRootOutsideTestRoot { .. }
        ));
    }

    #[test]
    fn reject_disk_root_as_fixture_root() {
        let (_test_root, _appdata, roots) = synthetic_roots();
        let policy = PathPolicy::new(roots);
        let err = policy.validate_fixture_root(Path::new("C:\\")).unwrap_err();
        assert!(matches!(
            err,
            FixturePathError::FixtureRootOutsideTestRoot { .. }
                | FixturePathError::CannotCanonicalize { .. }
        ));
    }

    #[test]
    fn reject_arbitrary_user_dir_as_fixture_root() {
        let (_test_root, _appdata, roots) = synthetic_roots();
        let user_dir = tempdir().unwrap();
        let policy = PathPolicy::new(roots);
        let err = policy.validate_fixture_root(user_dir.path()).unwrap_err();
        assert!(matches!(
            err,
            FixturePathError::FixtureRootOutsideTestRoot { .. }
        ));
    }

    #[test]
    fn reject_path_outside_fixture_root() {
        let (test_root, _appdata, roots) = synthetic_roots();
        let fixture_root = make_subdir(test_root.path(), "fixture");
        let policy = PathPolicy::new(roots);
        let canonical_fixture_root = policy.validate_fixture_root(&fixture_root).unwrap();
        let outside = tempdir().unwrap();
        let err = policy
            .validate_write_target(&canonical_fixture_root, outside.path())
            .unwrap_err();
        assert!(matches!(err, FixturePathError::OutsideFixtureRoot { .. }));
    }

    #[test]
    fn reject_parent_dir_escape() {
        let (test_root, _appdata, roots) = synthetic_roots();
        let fixture_root = make_subdir(test_root.path(), "fixture");
        let policy = PathPolicy::new(roots);
        let canonical_fixture_root = policy.validate_fixture_root(&fixture_root).unwrap();
        let candidate = fixture_root.join("..").join("escape.db");
        let err = policy
            .validate_write_target(&canonical_fixture_root, &candidate)
            .unwrap_err();
        assert!(matches!(
            err,
            FixturePathError::OutsideFixtureRoot { .. }
                | FixturePathError::CannotCanonicalize { .. }
        ));
    }

    #[test]
    fn accept_safe_parent_dir_usage() {
        let (test_root, _appdata, roots) = synthetic_roots();
        let fixture_root = make_subdir(test_root.path(), "fixture");
        make_subdir(&fixture_root, "sub");
        let policy = PathPolicy::new(roots);
        let canonical_fixture_root = policy.validate_fixture_root(&fixture_root).unwrap();
        let candidate = fixture_root.join("sub").join("..").join("inside.db");
        let result = policy
            .validate_write_target(&canonical_fixture_root, &candidate)
            .unwrap();
        assert!(result.starts_with(&canonical_fixture_root));
    }

    #[test]
    fn reject_default_work_cn_path_with_synthetic_roots() {
        let (test_root, appdata, roots) = synthetic_roots();
        let fixture_root = make_subdir(test_root.path(), "fixture");
        let policy = PathPolicy::new(roots);
        let canonical_fixture_root = policy.validate_fixture_root(&fixture_root).unwrap();

        let default_dir = appdata
            .path()
            .join("TRAE SOLO CN")
            .join("ModularData")
            .join("ai-agent");
        let default_db = default_dir.join("database.db");
        fs::write(&default_db, b"").unwrap();

        let err = policy
            .validate_write_target(&canonical_fixture_root, &default_db)
            .unwrap_err();
        assert!(matches!(err, FixturePathError::DefaultWorkCnPath { .. }));
    }

    #[test]
    fn reject_default_work_cn_directory_with_synthetic_roots() {
        let (test_root, appdata, roots) = synthetic_roots();
        let fixture_root = make_subdir(test_root.path(), "fixture");
        let policy = PathPolicy::new(roots);
        let canonical_fixture_root = policy.validate_fixture_root(&fixture_root).unwrap();

        let default_dir = appdata
            .path()
            .join("TRAE SOLO CN")
            .join("ModularData")
            .join("ai-agent");
        fs::create_dir_all(&default_dir).unwrap();

        let err = policy
            .validate_write_target(&canonical_fixture_root, &default_dir)
            .unwrap_err();
        assert!(matches!(err, FixturePathError::DefaultWorkCnPath { .. }));
    }

    #[test]
    fn reject_fixture_root_equal_to_default_work_cn_path() {
        let (_test_root, appdata, roots) = synthetic_roots();
        let default_dir = appdata
            .path()
            .join("TRAE SOLO CN")
            .join("ModularData")
            .join("ai-agent");
        fs::create_dir_all(&default_dir).unwrap();

        let policy = PathPolicy::new(roots);
        let err = policy.validate_fixture_root(&default_dir).unwrap_err();
        assert!(matches!(
            err,
            FixturePathError::FixtureRootIsDefaultWorkCnPath { .. }
        ));
    }

    #[test]
    fn reject_symlink_escape() {
        // R2-1 修复（handoff 第 62 行）：原测试在 symlink 创建失败时静默 `return`
        // 后宣称 PASS，违反"symlink/junction 测试不得在创建失败后静默跳过"。
        // 新设计：Windows 无开发者模式时文件 symlink 不可靠，改用目录 junction
        // （`mklink /J`，无需提升权限）作为逃逸载体；创建失败时 panic 报告 BLOCKED。
        let (test_root, _appdata, roots) = synthetic_roots();
        let fixture_root = make_subdir(test_root.path(), "fixture");
        let outside = tempdir().unwrap();
        // outside 作为目录，junction 指向它
        let outside_dir = outside.path().to_path_buf();

        let link_path = fixture_root.join("escape_link");
        let create_result = symlink_file_best_effort(&outside_dir, &link_path);
        if !create_result {
            // 创建 symlink/junction 失败时明确报告 BLOCKED，不让测试静默 PASS
            panic!(
                "BLOCKED: cannot create symlink/junction for reject_symlink_escape; symlink support unavailable"
            );
        }
        let policy = PathPolicy::new(roots);
        let canonical_fixture_root = policy.validate_fixture_root(&fixture_root).unwrap();
        let err = policy
            .validate_write_target(&canonical_fixture_root, &link_path)
            .unwrap_err();
        assert!(matches!(err, FixturePathError::OutsideFixtureRoot { .. }));
    }

    // ============== R2：db_relative_path 逃逸反例测试 ==============

    #[test]
    fn r2_rejects_empty_db_relative_path() {
        let (test_root, _appdata, roots) = synthetic_roots();
        let fixture_root = make_subdir(test_root.path(), "fixture");
        let policy = PathPolicy::new(roots);
        let canonical_fixture_root = policy.validate_fixture_root(&fixture_root).unwrap();
        let err = validate_db_relative_path_inside(&canonical_fixture_root, "").unwrap_err();
        assert!(matches!(err, FixturePathError::EmptyRelativePath));
    }

    #[test]
    fn r2_rejects_absolute_db_relative_path_windows() {
        // 绝对路径如 `C:\windows\system32\evil.db` 必须被词法拒绝——
        // 不应触发任何文件系统访问
        let (test_root, _appdata, roots) = synthetic_roots();
        let fixture_root = make_subdir(test_root.path(), "fixture");
        let policy = PathPolicy::new(roots);
        let canonical_fixture_root = policy.validate_fixture_root(&fixture_root).unwrap();
        let err = validate_db_relative_path_inside(
            &canonical_fixture_root,
            "C:\\windows\\system32\\evil.db",
        )
        .unwrap_err();
        assert!(matches!(err, FixturePathError::AbsolutePathRejected { .. }));
    }

    #[test]
    fn r2_rejects_absolute_db_relative_path_unix_style() {
        // Unix 风格绝对路径 `/etc/passwd` 在 Windows 上不是绝对路径，
        // 但仍应被拒绝——它包含 RootDir 组件
        let (test_root, _appdata, roots) = synthetic_roots();
        let fixture_root = make_subdir(test_root.path(), "fixture");
        let policy = PathPolicy::new(roots);
        let canonical_fixture_root = policy.validate_fixture_root(&fixture_root).unwrap();
        let err =
            validate_db_relative_path_inside(&canonical_fixture_root, "/etc/passwd").unwrap_err();
        assert!(matches!(err, FixturePathError::AbsolutePathRejected { .. }));
    }

    #[test]
    fn r2_rejects_parent_traversal_db_relative_path() {
        // `../secret.db` 必须被词法拒绝——不应触发任何文件系统访问
        let (test_root, _appdata, roots) = synthetic_roots();
        let fixture_root = make_subdir(test_root.path(), "fixture");
        let policy = PathPolicy::new(roots);
        let canonical_fixture_root = policy.validate_fixture_root(&fixture_root).unwrap();
        let err =
            validate_db_relative_path_inside(&canonical_fixture_root, "../secret.db").unwrap_err();
        assert!(matches!(
            err,
            FixturePathError::ParentTraversalRejected { .. }
        ));
    }

    #[test]
    fn r2_rejects_nested_parent_traversal_db_relative_path() {
        // `sub/../../escape.db` 也必须被拒绝——含 `..` 组件
        let (test_root, _appdata, roots) = synthetic_roots();
        let fixture_root = make_subdir(test_root.path(), "fixture");
        let policy = PathPolicy::new(roots);
        let canonical_fixture_root = policy.validate_fixture_root(&fixture_root).unwrap();
        let err = validate_db_relative_path_inside(&canonical_fixture_root, "sub/../../escape.db")
            .unwrap_err();
        assert!(matches!(
            err,
            FixturePathError::ParentTraversalRejected { .. }
        ));
    }

    #[test]
    fn r2_accepts_safe_db_relative_path() {
        // 安全路径：`sub/database.db`，DB 文件存在且封闭在 fixture_root 内
        let (test_root, _appdata, roots) = synthetic_roots();
        let fixture_root = make_subdir(test_root.path(), "fixture");
        let sub_dir = make_subdir(&fixture_root, "sub");
        let db_path = sub_dir.join("database.db");
        fs::write(&db_path, b"db-content").unwrap();
        let policy = PathPolicy::new(roots);
        let canonical_fixture_root = policy.validate_fixture_root(&fixture_root).unwrap();
        let canonical_db =
            validate_db_relative_path_inside(&canonical_fixture_root, "sub/database.db").unwrap();
        assert!(canonical_db.starts_with(&canonical_fixture_root));
        assert!(canonical_db.ends_with("database.db"));
    }

    #[test]
    fn r2_accepts_db_with_wal_shm_present() {
        // DB + WAL + SHM 都存在且全部封闭在 fixture_root 内 -> 通过
        let (test_root, _appdata, roots) = synthetic_roots();
        let fixture_root = make_subdir(test_root.path(), "fixture");
        fs::write(fixture_root.join("database.db"), b"db").unwrap();
        fs::write(fixture_root.join("database.db-wal"), b"wal").unwrap();
        fs::write(fixture_root.join("database.db-shm"), b"shm").unwrap();
        let policy = PathPolicy::new(roots);
        let canonical_fixture_root = policy.validate_fixture_root(&fixture_root).unwrap();
        let canonical_db =
            validate_db_relative_path_inside(&canonical_fixture_root, "database.db").unwrap();
        assert!(canonical_db.starts_with(&canonical_fixture_root));
    }

    #[test]
    fn r2_rejects_symlink_escape_via_db_relative_path() {
        // DB 路径经目录 junction 逃逸——在 fixture 内创建 junction 指向外部目录，
        // 外部目录里有 database.db。规范化后 DB 路径位于 fixture_root 外部 -> 必须拒绝。
        // 使用目录 junction（mklink /J）是因为 Windows 无开发者模式时文件 symlink 不可靠。
        let (test_root, _appdata, roots) = synthetic_roots();
        let fixture_root = make_subdir(test_root.path(), "fixture");
        let outside = tempdir().unwrap();
        // 在外部目录里放置 database.db 文件
        let outside_db = outside.path().join("database.db");
        fs::write(&outside_db, b"evil").unwrap();

        // 在 fixture 内创建 junction `escape_dir` 指向外部目录
        let link_path = fixture_root.join("escape_dir");
        let create_result = symlink_file_best_effort(outside.path(), &link_path);
        if !create_result {
            panic!(
                "BLOCKED: cannot create symlink/junction for r2_rejects_symlink_escape_via_db_relative_path"
            );
        }
        let policy = PathPolicy::new(roots);
        let canonical_fixture_root = policy.validate_fixture_root(&fixture_root).unwrap();
        // db_relative_path = "escape_dir/database.db" -> canonical 解析为 outside/database.db -> 逃逸
        let err =
            validate_db_relative_path_inside(&canonical_fixture_root, "escape_dir/database.db")
                .unwrap_err();
        assert!(matches!(err, FixturePathError::OutsideFixtureRoot { .. }));
    }

    #[test]
    fn r2_rejects_symlink_escape_via_wal() {
        // DB 安全但 WAL 经 junction 逃逸 -> 必须拒绝。
        // 在 fixture 内创建 `database.db` 文件 + `database.db-wal` junction 指向外部目录。
        // WAL 派生路径 `database.db-wal` 经 junction 解析后位于 fixture_root 外部 -> 拒绝。
        let (test_root, _appdata, roots) = synthetic_roots();
        let fixture_root = make_subdir(test_root.path(), "fixture");
        // DB 安全地放在 fixture 内
        fs::write(fixture_root.join("database.db"), b"db").unwrap();

        let outside = tempdir().unwrap();
        // 在 fixture 内创建 junction `database.db-wal` 指向外部目录
        // junction 是目录，但验证逻辑只检查 candidate.exists() 与 canonicalize 后的封闭性
        let link_path = fixture_root.join("database.db-wal");
        let create_result = symlink_file_best_effort(outside.path(), &link_path);
        if !create_result {
            panic!("BLOCKED: cannot create symlink/junction for r2_rejects_symlink_escape_via_wal");
        }
        let policy = PathPolicy::new(roots);
        let canonical_fixture_root = policy.validate_fixture_root(&fixture_root).unwrap();
        let err =
            validate_db_relative_path_inside(&canonical_fixture_root, "database.db").unwrap_err();
        assert!(matches!(err, FixturePathError::OutsideFixtureRoot { .. }));
    }

    #[test]
    fn r2_rejects_symlink_escape_via_shm() {
        // DB+WAL 安全但 SHM 经 junction 逃逸 -> 必须拒绝。
        let (test_root, _appdata, roots) = synthetic_roots();
        let fixture_root = make_subdir(test_root.path(), "fixture");
        fs::write(fixture_root.join("database.db"), b"db").unwrap();
        fs::write(fixture_root.join("database.db-wal"), b"wal").unwrap();

        let outside = tempdir().unwrap();
        // 在 fixture 内创建 junction `database.db-shm` 指向外部目录
        let link_path = fixture_root.join("database.db-shm");
        let create_result = symlink_file_best_effort(outside.path(), &link_path);
        if !create_result {
            panic!("BLOCKED: cannot create symlink/junction for r2_rejects_symlink_escape_via_shm");
        }
        let policy = PathPolicy::new(roots);
        let canonical_fixture_root = policy.validate_fixture_root(&fixture_root).unwrap();
        let err =
            validate_db_relative_path_inside(&canonical_fixture_root, "database.db").unwrap_err();
        assert!(matches!(err, FixturePathError::OutsideFixtureRoot { .. }));
    }

    #[test]
    fn r2_rejects_db_canonicalize_failure() {
        // DB 不存在 -> CannotCanonicalize（区别于词法拒绝）
        let (test_root, _appdata, roots) = synthetic_roots();
        let fixture_root = make_subdir(test_root.path(), "fixture");
        let policy = PathPolicy::new(roots);
        let canonical_fixture_root = policy.validate_fixture_root(&fixture_root).unwrap();
        let err = validate_db_relative_path_inside(&canonical_fixture_root, "nonexistent.db")
            .unwrap_err();
        assert!(matches!(err, FixturePathError::CannotCanonicalize { .. }));
    }

    #[test]
    fn fixture_ipc_error_text_excludes_paths_and_io_details() {
        let raw_path = r"C:\Users\example\AppData\Roaming\TRAE CN\database.db";
        let io_detail = "Access is denied (os error 5)";
        let errors = [
            FixturePathError::CannotCanonicalize {
                raw: raw_path.to_string(),
                source: io_detail.to_string(),
            },
            FixturePathError::OperationLeaseUnavailable {
                source: format!("{io_detail}: {raw_path}"),
            },
        ];

        for error in errors {
            let public = fixture_path_error_text(&error);
            assert!(public.starts_with("fixture_path_error:"));
            assert!(!public.contains(raw_path));
            assert!(!public.contains("AppData"));
            assert!(!public.contains(io_detail));
        }
    }
}

/// 测试辅助：尽力创建文件 symlink。
///
/// Windows 上优先尝试 `symlink_file`；权限不足或开发者模式未开启时
/// 退回到创建 junction（用于目录）。两者均失败时返回 false。
///
/// R2-1：不再静默 return；调用方需根据返回值决定是 panic 还是 continue。
/// junction 通过 `cmd /c mklink /J` 创建，无需开发者模式或提升权限。
#[cfg(test)]
#[allow(dead_code)]
fn symlink_file_best_effort(target: &Path, link: &Path) -> bool {
    #[cfg(windows)]
    {
        // 文件 symlink：Windows 上需要开发者模式或管理员权限
        if std::os::windows::fs::symlink_file(target, link).is_ok() {
            return true;
        }
        // 退回到 junction（目录 junction，不需要开发者模式）
        // 仅当 target 是目录时才尝试
        if target.is_dir() {
            if std::os::windows::fs::symlink_dir(target, link).is_ok() {
                return true;
            }
            // 通过 cmd mklink /J 创建 junction（参数分立传递，避免组合字符串的引号问题）
            let out = std::process::Command::new("cmd")
                .args([
                    "/C",
                    "mklink",
                    "/J",
                    &link.to_string_lossy(),
                    &target.to_string_lossy(),
                ])
                .output();
            if let Ok(o) = out {
                if o.status.success() {
                    return true;
                }
            }
        }
        false
    }
    #[cfg(not(windows))]
    {
        std::os::unix::fs::symlink(target, link).is_ok()
    }
}
