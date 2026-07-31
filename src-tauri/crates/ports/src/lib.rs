//! Trae Sync 端口层：application 定义、infrastructure 实现的 trait 边界。
//!
//! T01 骨架阶段只定义工作台状态查询所需的最小 port。
//! 不预建 ProductAdapter、CatalogRepository、SnapshotStore 等未来 port——
//! 那些在对应 ticket（T02+）实现时再增加。

use traesync_domain::WorkspaceState;

/// 工作台状态提供者：application 通过此 port 获取状态，不直接依赖 infrastructure。
///
/// T01 阶段唯一实现是 infrastructure 的 `StaticWorkspaceStateProvider`，
/// 返回固定的空工作台状态。未来真实实现会读取目录库与账号证据。
pub trait WorkspaceStateProvider: Send + Sync {
    fn get_workspace_state(&self) -> WorkspaceState;
}
