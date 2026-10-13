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
    AccountEvidenceReader, AccountRegistry, FixedProcessController, SqlCipherProbe,
    StaticWorkspaceStateProvider,
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
        active_login_attempt: Arc::new(Mutex::new(None)),
        checkin_execution_lock: Arc::new(Mutex::new(())),
    }
}

/// 构造 fixture 模式状态，用于确认普通操作门禁早于真实读写门禁。
pub(crate) fn make_fixture_mode_state(storage_root: &Path) -> AppState {
    let mut state = make_real_mode_state(storage_root);
    state.runtime_mode = RuntimeMode::Fixture;
    state
}

/// 写入一条已登录账号的注册表档案（设备公钥为真实 P-256 SPKI PEM）。
/// P12-R3：原 real_checkin_tests 夹具，签到域迁出后由 checkin_schedule 与
/// lib.rs 留守测试（账号列表 / purge 等）共享。
pub(crate) fn registered_account(root: &Path, profile_id: &str) {
    let (_, public_pem) = traesync_infrastructure::generate_device_keypair().unwrap();
    let registry = AccountRegistry::new(root.join("checkin"));
    registry
        .upsert(&traesync_infrastructure::AccountRecord {
            profile_id: profile_id.to_string(),
            account_id: format!("account-{profile_id}"),
            screen_name: "测试账号".to_string(),
            avatar_url: String::new(),
            device_id: "1234567890123456".to_string(),
            device_public_key: public_pem,
            display_name: None,
            masked_mobile: String::new(),
            created_at_unix_seconds: 0,
            last_verified_at_unix_seconds: 0,
            device_created_at_unix_seconds: 0,
            auto_checkin_enabled: true,
            archived: false,
        })
        .unwrap();
}

/// U-6 W4 测试辅助：写入指定 account_id 的档案（原生目录按
/// `TRAE SOLO CN_{account_id}` 定位，需纯数字 account_id 才可命中）。
pub(crate) fn registered_account_with_id(root: &Path, profile_id: &str, account_id: &str) {
    let registry = AccountRegistry::new(root.join("checkin"));
    registry
        .upsert(&traesync_infrastructure::AccountRecord {
            profile_id: profile_id.to_string(),
            account_id: account_id.to_string(),
            screen_name: "测试账号".to_string(),
            avatar_url: String::new(),
            device_id: "1234567890123456".to_string(),
            device_public_key: "pub".to_string(),
            display_name: None,
            masked_mobile: String::new(),
            created_at_unix_seconds: 0,
            last_verified_at_unix_seconds: 0,
            device_created_at_unix_seconds: 0,
            auto_checkin_enabled: true,
            archived: false,
        })
        .unwrap();
}
