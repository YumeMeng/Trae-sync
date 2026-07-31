//! 工作台状态提供者的 T01 静态实现。
//!
//! 依赖方向：infrastructure 只依赖 domain + ports，不依赖 application。
//! `empty_workspace_state` 是 domain 层提供的 `WorkspaceState` 默认构造器。

use traesync_domain::{empty_workspace_state, WorkspaceState};
use traesync_ports::WorkspaceStateProvider;

/// T01 骨架阶段的工作台状态提供者：始终返回固定的空状态。
///
/// 不读取任何文件、数据库或认证状态。
/// 未来 T02+ 会替换为读取真实目录库与账号证据的实现。
pub struct StaticWorkspaceStateProvider;

impl Default for StaticWorkspaceStateProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl StaticWorkspaceStateProvider {
    pub fn new() -> Self {
        Self
    }
}

impl WorkspaceStateProvider for StaticWorkspaceStateProvider {
    fn get_workspace_state(&self) -> WorkspaceState {
        // T01 阶段：直接返回固定空状态
        empty_workspace_state()
    }
}
