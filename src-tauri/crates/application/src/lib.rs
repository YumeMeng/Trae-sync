//! Trae Sync 应用服务层：编排用例、锁、进度和状态。
//!
//! T01 骨架阶段只实现 `WorkspaceStateService`——返回空工作台的诚实状态。
//! T02 新增 `WorkbenchReadService`——组合 SQLCipher 探测与账号证据，
//! 产生只读工作台状态。
//! T03/T04 新增 `history` 模块：`ScanHistoryService`、`BrowseHistoryService`、
//! `AssignProjectSourceService`，承载 P1 历史基础用例。

pub mod history;
pub mod workbench_read;

use traesync_domain::WorkspaceState;
// 重导出 port trait：commands 层只依赖 application 即可拿到 trait，
// 不需要直接依赖 ports crate——这维持“commands -> application + domain”的依赖方向。
pub use traesync_ports::WorkspaceStateProvider;
// 重导出 T02 工作台只读 port，供 commands 层构造测试 fake 使用
pub use traesync_ports::{AccountEvidenceReaderPort, DatabaseProbePort};
// 重导出 T03/T04 历史库 port，供 commands 层构造测试 fake 使用
pub use traesync_ports::{
    CatalogRepository, ContentGraphHasher, FileIdentityProvider, SnapshotStore, SourceNormalizer,
};

pub use history::{AssignProjectSourceService, BrowseHistoryService, ScanHistoryService};
pub use workbench_read::WorkbenchReadService;

/// 工作台状态服务：T01 阶段返回固定的空状态，所有真实能力禁用。
///
/// 通过 `WorkspaceStateProvider` port 获取状态，不直接读取文件或数据库。
pub struct WorkspaceStateService<'a> {
    provider: &'a dyn WorkspaceStateProvider,
}

impl<'a> WorkspaceStateService<'a> {
    pub fn new(provider: &'a dyn WorkspaceStateProvider) -> Self {
        Self { provider }
    }

    /// 返回当前工作台状态。
    /// T01 阶段状态固定：平台 Work CN（边界保留）、无数据位置、无账号、空历史、能力禁用。
    pub fn get_workspace_state(&self) -> WorkspaceState {
        self.provider.get_workspace_state()
    }
}
