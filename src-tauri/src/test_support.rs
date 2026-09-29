//! 跨域测试共享夹具（P12-R3）。
//!
//! 构造指定运行模式的测试用 AppState；被 lib.rs 内测试模块与
//! oauth_login 等域模块的测试共同使用，故按定案提为独立共享模块。
//! 仅测试编译期可用（lib.rs 以 `#[cfg(test)]` 门控整模块）。

use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

use traesync_domain::AuthorizationState;
use traesync_infrastructure::{
    AccountEvidenceReader, FixedProcessController, SqlCipherProbe, StaticWorkspaceStateProvider,
};

use crate::{
    AppState, AuthorizationSlot, CapabilityManifest, ManagedAccountRuntimeSlot, RuntimeMode,
};

/// 构造真实只读模式的测试状态；storage_root 指向临时目录。
pub(crate) fn make_real_mode_state(storage_root: &Path) -> AppState {
    AppState {
        runtime_mode: RuntimeMode::RealReadPreview,
        capabilities: CapabilityManifest::installed_0_2_1(),
        production_catalog: None,
        provider: Arc::new(StaticWorkspaceStateProvider::new()),
        workbench_probe: SqlCipherProbe::new(),
        workbench_reader: AccountEvidenceReader::new(),
        source_raw_key: "0".repeat(64),
        storage_root: storage_root.to_string_lossy().into_owned(),
        recovery_root: String::new(),
        process_controller: Arc::new(FixedProcessController::not_running()),
        authorization: Arc::new(Mutex::new(AuthorizationSlot::new(
            AuthorizationState::NotAuthorized,
        ))),
        pending_sync_plan: Arc::new(Mutex::new(None)),
        active_sync_cancellation: Arc::new(Mutex::new(None)),
        pending_deletion_plan: Arc::new(Mutex::new(None)),
        managed_account_runtime: Arc::new(Mutex::new(ManagedAccountRuntimeSlot::default())),
        checkin_cancellation: Arc::new(AtomicBool::new(false)),
        pending_login: Arc::new(Mutex::new(None)),
        active_oauth_profile: Arc::new(Mutex::new(None)),
        login_cancel: Arc::new(AtomicBool::new(false)),
        active_login_browser: Arc::new(Mutex::new(None)),
        checkin_execution_lock: Arc::new(Mutex::new(())),
    }
}
