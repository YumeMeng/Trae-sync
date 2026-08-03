//! Trae Sync 基础设施层：实现 ports 定义的 trait，提供具体技术访问。
//!
//! T01 骨架阶段包含三部分：
//! - `workspace`：`StaticWorkspaceStateProvider`，返回固定空状态
//! - `logging`：结构化日志，含 `operation_id`，敏感内容在输出边界去敏
//! - `fixture_paths`：`FixturePathGuard`，强制 fixture_root 写目标只能在测试根内
//!
//! 【R1 修复】`SystemRoots`、`PathPolicy` 不公开；外部调用者只能通过
//! `FixturePathGuard::new(fixture_root)` 使用，无法注入伪造信任输入。
//!
//! 【R1 修复（第三次）】`LogEvent` 公开类型但字段私有且不派生 `Deserialize`——
//! 外部可接收 `&LogEvent` 实现 `LogSink`，但无法通过 serde 反序列化或直接构造。

pub mod account_evidence;
pub mod catalog;
pub mod content_graph;
pub mod file_identity;
pub mod fixture_paths;
pub mod logging;
mod operation_manifest;
pub mod snapshot_store;
pub mod sqlcipher;
pub mod work_cn_normalizer;
pub mod work_cn_schema;
pub mod workspace;

pub use account_evidence::AccountEvidenceReader;
pub use catalog::SqlCipherCatalogRepository;
pub use content_graph::DeterministicContentGraphHasher;
pub use file_identity::PlatformFileIdentityProvider;
pub use fixture_paths::{FixturePathError, FixturePathGuard, SystemRootsError};
pub use logging::{
    LogEvent, LogEventCode, LogLevel, LogSink, RedactingLogSink, SafeLogEventBuilder, SafeLogField,
};
pub use snapshot_store::{sha256_file, FilesystemSnapshotStore};
pub use sqlcipher::{FollowProjectExecution, SqlCipherProbe, WorkCnSyncExecutor};
pub use work_cn_normalizer::WorkCnSourceNormalizer;
pub use workspace::StaticWorkspaceStateProvider;
