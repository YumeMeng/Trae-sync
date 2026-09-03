//! 工作台状态提供者的 T01 静态实现。
//!
//! 依赖方向：infrastructure 只依赖 domain + ports，不依赖 application。
//! `empty_workspace_state` 是 domain 层提供的 `WorkspaceState` 默认构造器。

use traesync_domain::{empty_workspace_state, WorkspaceState};
#[cfg(feature = "sqlcipher")]
use traesync_domain::{
    CapabilityFlags, CurrentAccountState, DataLocationState, HistorySummary, PlatformContext,
    PlatformId,
};
use traesync_ports::WorkspaceStateProvider;

#[cfg(feature = "sqlcipher")]
use std::path::PathBuf;
#[cfg(feature = "sqlcipher")]
use traesync_ports::CatalogRepository;

#[cfg(feature = "sqlcipher")]
use crate::{resolve_current_catalog_path, SqlCipherCatalogRepository};

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

/// 隔离 fixture 工作台状态：允许在合成数据库上验收安全同步流程，
/// 但不会把该能力误报成真实 TRAE 活动库写入。
#[cfg(feature = "sqlcipher")]
pub struct FixtureWorkspaceStateProvider {
    storage_root: PathBuf,
    catalog_key: String,
}

#[cfg(feature = "sqlcipher")]
impl FixtureWorkspaceStateProvider {
    pub fn new(storage_root: PathBuf, catalog_key: String) -> Self {
        Self {
            storage_root,
            catalog_key,
        }
    }
}

#[cfg(feature = "sqlcipher")]
impl WorkspaceStateProvider for FixtureWorkspaceStateProvider {
    fn get_workspace_state(&self) -> WorkspaceState {
        let summary = resolve_current_catalog_path(&self.storage_root)
            .ok()
            .and_then(|path| {
                // fixture 工作台只读取 Trae Sync 自有目录库，源数据库仍由授权命令控制。
                SqlCipherCatalogRepository::new(path, self.catalog_key.clone())
                    .history_summary_checked()
                    .ok()
            })
            .unwrap_or_default();
        WorkspaceState {
            platform: PlatformContext {
                platform_id: PlatformId::work_cn(),
                display_name: "TRAE Work CN（隔离副本）".to_string(),
                adapter_implemented: false,
            },
            data_location: DataLocationState {
                selected: false,
                display_name: Some("隔离 fixture 数据位置".to_string()),
                unavailable_reason: None,
            },
            current_account: CurrentAccountState {
                detected: false,
                user_fingerprint: None,
                unavailable_reason: Some("fixture_mode".to_string()),
            },
            history: HistorySummary {
                account_count: summary.visible_account_count,
                project_count: summary.visible_project_count,
                session_count: summary.visible_session_count,
            },
            capabilities: CapabilityFlags {
                scan_enabled: true,
                sync_enabled: true,
                backup_enabled: true,
                restore_enabled: false,
            },
            honest_status: "隔离副本安全同步已开放；生产 TRAE 活动库仍保持只读。".to_string(),
        }
    }
}

/// RealReadPreview 的工作台状态提供者。
///
/// 启动状态只读取 Trae Sync 自有目录库；当前账号证据和活动数据库必须等用户授权后
/// 才由扫描命令读取，因此这里始终把账号显示为“等待授权扫描”。
#[cfg(feature = "sqlcipher")]
pub struct RealReadWorkspaceStateProvider {
    data_location_display: Option<String>,
    catalog: Option<(PathBuf, String)>,
    startup_issue: Option<RealReadStartupIssue>,
    location_error: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RealReadStartupIssue {
    Unavailable,
    CatalogWriteProtocolUpgradeRequired,
}

#[cfg(feature = "sqlcipher")]
impl RealReadWorkspaceStateProvider {
    pub fn new(
        data_location_display: Option<String>,
        catalog_path: PathBuf,
        catalog_key: String,
    ) -> Self {
        Self {
            data_location_display,
            catalog: Some((catalog_path, catalog_key)),
            startup_issue: None,
            location_error: None,
        }
    }

    /// 记录固定生产位置发现失败；不降级到 fixture，也不开放扫描。
    pub fn with_location_error(mut self, reason: impl Into<String>) -> Self {
        self.location_error = Some(reason.into());
        self
    }

    /// 启动依赖不可用时仍返回诚实工作台状态，不创建替代目录库或开放扫描。
    pub fn unavailable(_reason: impl Into<String>) -> Self {
        Self {
            data_location_display: None,
            catalog: None,
            startup_issue: Some(RealReadStartupIssue::Unavailable),
            location_error: None,
        }
    }

    /// 旧写协议需要旁路升级时暴露固定安全提示，不携带路径、密钥或底层错误正文。
    pub fn catalog_write_protocol_upgrade_required() -> Self {
        Self {
            data_location_display: None,
            catalog: None,
            startup_issue: Some(RealReadStartupIssue::CatalogWriteProtocolUpgradeRequired),
            location_error: None,
        }
    }
}

#[cfg(feature = "sqlcipher")]
impl WorkspaceStateProvider for RealReadWorkspaceStateProvider {
    fn get_workspace_state(&self) -> WorkspaceState {
        let location_selected = self.data_location_display.is_some();
        let (summary, catalog_error) = self
            .catalog
            .as_ref()
            .map(|(path, key)| {
                SqlCipherCatalogRepository::new(path.clone(), key.clone())
                    .history_summary_checked()
                    .map(|summary| (summary, None))
                    .unwrap_or_else(|_| (Default::default(), Some("目录库不可读".to_string())))
            })
            .unwrap_or_default();
        let startup_ready = self.startup_issue.is_none();
        let runtime_ready = startup_ready && catalog_error.is_none();
        WorkspaceState {
            platform: PlatformContext {
                platform_id: PlatformId::work_cn(),
                display_name: "TRAE Work CN".to_string(),
                adapter_implemented: true,
            },
            data_location: DataLocationState {
                selected: location_selected,
                display_name: self.data_location_display.clone(),
                unavailable_reason: (!location_selected).then(|| match self.startup_issue {
                    Some(RealReadStartupIssue::CatalogWriteProtocolUpgradeRequired) => {
                        "catalog_write_protocol_upgrade_required".to_string()
                    }
                    Some(RealReadStartupIssue::Unavailable) => "startup_unavailable".to_string(),
                    None => "not_found".to_string(),
                }),
            },
            current_account: CurrentAccountState {
                detected: false,
                user_fingerprint: None,
                unavailable_reason: Some("authorization_required".to_string()),
            },
            history: HistorySummary {
                account_count: summary.visible_account_count,
                project_count: summary.visible_project_count,
                session_count: summary.visible_session_count,
            },
            capabilities: CapabilityFlags {
                scan_enabled: runtime_ready && location_selected,
                sync_enabled: false,
                backup_enabled: false,
                restore_enabled: false,
            },
            honest_status: if self.startup_issue
                == Some(RealReadStartupIssue::CatalogWriteProtocolUpgradeRequired)
            {
                "当前目录库采用了此版本不支持的写入协议，未写入新的历史或修改目录库。保留目录库及其 sidecar；不要手动删除 WAL/SHM，等待后续目录库旁路升级功能。".to_string()
            } else if self.startup_issue.is_some() {
                "真实只读 Preview 暂不可用；启动材料不可用。".to_string()
            } else if catalog_error.is_some() {
                "真实只读 Preview 暂不可用：目录库不可读。".to_string()
            } else if self.location_error.is_some() {
                "真实只读 Preview 暂不可用：未发现可用的 TRAE Work CN 数据位置。".to_string()
            } else if !location_selected {
                "真实只读 Preview；等待用户授权后发现 TRAE 数据位置；TRAE 写入保持禁用。"
                    .to_string()
            } else {
                "真实只读 Preview；TRAE 写入保持禁用".to_string()
            },
        }
    }
}

#[cfg(all(test, feature = "sqlcipher"))]
mod tests {
    use super::*;
    use traesync_ports::WorkspaceStateProvider;

    #[test]
    fn unavailable_status_does_not_expose_startup_reason() {
        let state =
            RealReadWorkspaceStateProvider::unavailable(r"D:\secret\catalog-key-or-startup-path")
                .get_workspace_state();

        assert_eq!(
            state.data_location.unavailable_reason.as_deref(),
            Some("startup_unavailable")
        );
        assert_eq!(
            state.honest_status,
            "真实只读 Preview 暂不可用；启动材料不可用。"
        );
        assert!(!state.honest_status.contains("catalog-key"));
        assert!(!state.honest_status.contains("D:\\secret"));
    }

    #[test]
    fn unsupported_catalog_write_protocol_exposes_only_stable_guidance() {
        let state = RealReadWorkspaceStateProvider::catalog_write_protocol_upgrade_required()
            .get_workspace_state();

        assert_eq!(
            state.data_location.unavailable_reason.as_deref(),
            Some("catalog_write_protocol_upgrade_required")
        );
        assert_eq!(
            state.honest_status,
            "当前目录库采用了此版本不支持的写入协议，未写入新的历史或修改目录库。保留目录库及其 sidecar；不要手动删除 WAL/SHM，等待后续目录库旁路升级功能。"
        );
        assert!(!state.capabilities.scan_enabled);
    }

    #[test]
    fn discovered_location_is_exposed_without_authorizing_account_reads() {
        let temp = tempfile::tempdir().unwrap();
        let state = RealReadWorkspaceStateProvider::new(
            Some(temp.path().to_string_lossy().into_owned()),
            temp.path().join("catalog.db"),
            "catalog-key".to_string(),
        )
        .get_workspace_state();

        assert!(state.data_location.selected);
        assert_eq!(
            state.data_location.display_name,
            Some(temp.path().to_string_lossy().into_owned())
        );
        assert_eq!(
            state.current_account.unavailable_reason.as_deref(),
            Some("authorization_required")
        );
        assert!(!state.capabilities.sync_enabled);
        assert!(!state.capabilities.backup_enabled);
        assert!(!state.capabilities.restore_enabled);
    }
}
