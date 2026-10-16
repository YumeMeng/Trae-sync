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
pub mod account_registry;
#[cfg(feature = "sqlcipher")]
pub mod account_session_content;
#[cfg(feature = "sqlcipher")]
pub mod account_session_index;
pub mod account_switch;
pub(crate) mod atomic_publish;
// P11-5：库归档草稿存储（localStorage 迁入 storage_root）。
pub mod archive_draft_store;
pub mod auto_checkin_store;
pub mod backup_retention;
pub mod blob_keepalive;
#[cfg(feature = "sqlcipher")]
pub mod catalog;
pub mod catalog_path;
pub mod checkin;
pub mod checkin_credential;
pub mod fused_login_credential;
pub mod checkin_http;
pub mod checkin_login;
// P13-1：Trae CN 签到协议实装（status/claim，探针形状为唯一权威来源）。
pub mod trae_cn_checkin;
// P11-6：Trae CN 签到只读探针（ADR-0032 决策 4 门禁；status 类查询先行）。
pub mod checkin_probe;
pub mod content_graph;
pub mod credential_maintenance;
pub mod credential_vault;
pub mod data_location;
pub mod environment_registry;
pub mod file_identity;
pub mod fixture_paths;
pub mod handoff_intent;
// 工具链 1.97 适配：junction/符号链接统一判定 helper（防目录逃逸判定扩展）。
// 模块私有：is_reparse_link 仅供 crate 内安全判定点使用；test_support 仅测试用。
mod link_guard;
pub mod key_wrapper;
// P12-R1b：产品×库 → 当前账号解析（组合根 resolve_library_account 系下沉）。
pub mod library_account;
pub mod logging;
#[cfg(feature = "sqlcipher")]
pub mod master_archive;
#[cfg(feature = "sqlcipher")]
pub mod master_checkup;
#[cfg(feature = "sqlcipher")]
pub mod master_handover;
#[cfg(feature = "sqlcipher")]
pub mod master_history;
#[cfg(feature = "sqlcipher")]
pub mod master_stats;
pub mod master_verification;
pub(crate) mod migration_manifest;
pub mod operation_lease;
pub mod operation_manifest;
pub mod plugin_cloud_sync;
pub mod plugin_manifest;
pub mod process_observation;
pub mod product_account_state;
pub mod product_adapter;
#[cfg(feature = "sqlcipher")]
pub mod production_catalog;
pub mod progress;
#[cfg(feature = "sqlcipher")]
pub mod recovery_package;
pub mod relay_ledger;
pub mod remint;
pub mod remote_device;
pub mod scan_authorization;
pub mod snapshot_store;
#[cfg(feature = "sqlcipher")]
pub mod sqlcipher;
pub mod storage_deletion;
// P16-T2：存储根派生路径唯一来源（checkin/products-trae_cn/product-state/fused-login）。
pub mod storage_roots;
pub mod storage_root;
pub mod trae_instance;
pub mod work_cn_location;
#[cfg(feature = "sqlcipher")]
pub mod work_cn_normalizer;
#[cfg(feature = "sqlcipher")]
pub mod work_cn_schema;
pub mod workspace;

pub use account_evidence::{
    user_id_binding_fingerprint, user_id_display_fingerprint, AccountEvidenceReader,
};
pub use account_registry::{
    account_records_by_profile_id, AccountRecord, AccountRegistry, AccountRegistryError,
};
pub use account_switch::{
    load_account_fingerprint_salt, load_or_create_account_fingerprint_salt,
    profile_store_payload_is_non_sensitive, salted_user_id_fingerprint,
    JsonManagedAccountProfileStore,
};
pub use auto_checkin_store::{
    build_staggered_plan, is_valid_hhmm, should_recover_interrupted_checkin,
    should_retry_skipped_checkin, should_trigger_auto_checkin, AutoCheckinBatchState,
    AutoCheckinLedger, AutoCheckinSettings, AutoCheckinStore, AutoCheckinStoreError,
};
pub use blob_keepalive::{
    archive_login_user_id, construct_auth_identity, decrypt_auth_blob, decrypt_named_blob,
    master_login_material, switch_auth_identity, switch_auth_identity_for_account,
    writeback_auth_blob, AuthIdentitySwitch, AuthSwitchError, ConstructAuthInput, KeepaliveError,
    KeepaliveOutcome, MasterLoginMaterial,
};
#[cfg(feature = "sqlcipher")]
pub use catalog::{
    ensure_catalog_initialized, initialize_catalog_identity, reconcile_current_catalog_sidecar,
    verify_catalog_identity, SqlCipherCatalogRepository,
};
pub use catalog_path::{
    resolve_current_catalog_generation_id, resolve_current_catalog_path, CatalogPathError,
};
pub use checkin::FixtureCheckinTransport;
pub use fused_login_credential::{
    FusedLoginRecord, FusedLoginStore, FusedLoginStoreError, FusedLoginSummary,
};
pub use checkin_credential::{
    device_proof_signing_input, generate_device_keypair, needs_refresh, sign_device_proof,
    verify_device_proof, CheckinCredentialBundle, CheckinCredentialError, CheckinCredentialStore,
    CheckinProfileBinding, CheckinProfileOperationLease, FixtureExchangeRequest,
    FixtureRenewalService, FixtureTokenEndpoint, FixtureUserInfoEndpoint, PendingRenewalRecord,
    RenewalReceipt,
};
pub use checkin_http::{
    checkin_claim_request, checkin_status_request, exchange_token_by_auth_code,
    exchange_token_by_refresh, get_pc_auth_code, get_user_info, get_user_info_for_oauth_client,
    get_user_info_full, get_user_info_full_for_oauth_client, trae_http_client, CheckinHttpError,
    CredentialRenewalHttpAdapter, DeviceInfoBlock, OAuthClient, RealCheckinRenewalService,
    RealCheckinTransport, TokenGrant, UserInfoFull, UserInfoSummary, TRAE_CN_CLIENT_ID,
    TRAE_CN_IDE_VERSION, TRAE_IDE_VERSION, TRAE_SOLO_CLIENT_ID, TRAE_SOLO_IDE_VERSION,
};
pub use checkin_login::{
    begin_login, begin_login_for, build_login_url_for, complete_login, LoginAcceptance,
    LoginAttemptCancelled, LoginAttemptControl, LoginAttemptPhase, LoginAttemptResult,
    LoginCallbackServer, LoginCancelResult, LoginError, LoginHandoff, LoginProduct, LoginReceipt,
    LoginSession, CALLBACK_TIMEOUT_SECONDS, LOGIN_HOST,
};
// P13-1：Trae CN 签到协议端口与 Real 实现（命令层经此引用）。
pub use trae_cn_checkin::{
    claim_request, parse_claim_envelope, parse_status_envelope, status_request,
    RealTraeCnCheckinClient, ReqwestTraeCnCheckinHttp, TraeCnCheckinClaim, TraeCnCheckinClient,
    TraeCnCheckinError, TraeCnCheckinHttp, TraeCnCheckinRequest, TraeCnCheckinStatus,
};
// P11-6：只读探针公开 API（真实探测由 #[ignore] 集成测试显式授权运行）。
pub use checkin_probe::{
    build_probe_report, candidate_endpoints, derive_probe_conclusion, probe_get_user_info_shape,
    probe_trae_cn_status_endpoints, write_probe_report, TraeCnCheckinProbeReport,
    TraeCnProbeFinding,
};
pub use content_graph::DeterministicContentGraphHasher;
pub use credential_maintenance::{
    CredentialMaintenanceStateStore, CredentialMaintenanceStoreError,
    CREDENTIAL_BACKOFF_STEPS_SECONDS, MAX_DAILY_MAINTENANCE_ATTEMPTS,
};
pub use credential_vault::{
    CredentialApplyOutcome, CredentialBinding, CredentialRecoveryRecord, CredentialState,
    CredentialStatus, CredentialVault, CredentialVaultError,
};
pub use data_location::{
    capture_location_identity, capture_location_witness, compare_location_identity,
    compare_location_witness, LocationWitness, WitnessError, WitnessMismatch,
};
pub use file_identity::PlatformFileIdentityProvider;
pub use fixture_paths::{
    fixture_path_error_text, FixturePathError, FixturePathGuard, SystemRootsError,
};
pub use handoff_intent::JsonHandoffIntentStore;
pub use key_wrapper::DpapiKeyWrapper;
pub use logging::{
    LogEvent, LogEventCode, LogLevel, LogSink, RedactingLogSink, SafeLogEventBuilder, SafeLogField,
};
#[cfg(feature = "sqlcipher")]
pub use master_archive::{
    apply_master_archive_changes,
    delete_master_sessions, delete_master_sessions_with_backup_dir, merge_master_projects,
    merge_master_projects_with_backup_dir,
    MasterArchiveApplyOutcome, MasterArchiveError, MasterDeleteOutcome, MasterMergeOutcome,
    ARCHIVE_HIDDEN_STATUS,
};
#[cfg(feature = "sqlcipher")]
pub use master_checkup::{
    read_master_checkup, CheckupAccountRow, MasterCheckup, MasterCheckupStatus, RegisteredAccount,
};
#[cfg(feature = "sqlcipher")]
pub use master_handover::{
    backup_master_trio, backup_master_trio_stamped, backup_master_trio_to, handover_master_records,
    handover_master_records_from_merge_with_progress, handover_master_records_from_with_progress,
    handover_master_records_with_progress, list_master_backups, list_master_backups_in,
    master_backup_db_name, master_database_path, master_db_activity_detected,
    merge_trae_cn_project_records_with_progress, preview_trae_cn_project_merge, HandoverProgress,
    HandoverSession, MasterBackupEntry, MasterHandover, MasterHandoverError, TraeCnMergeRow,
    TraeCnProjectMergePreview,
};
#[cfg(feature = "sqlcipher")]
pub use master_history::{
    read_master_history, read_trae_cn_foreign_projects, stat_master_fingerprint,
    MasterHistoryStatus, TraeCnForeignProjectEntry,
};
pub use operation_lease::{
    inspect_lock_status, OperationLease, OperationLeaseError, OperationLockStatus,
};
pub use operation_manifest::{list_operation_summaries, OperationManifestError, OperationSummary};
pub use plugin_cloud_sync::{
    fetch_installed_plugins, fetch_market_plugins, find_uninstall_target, install_market_plugin,
    reconcile_plan, sync_account_cloud_plugins, uninstall_cloud_plugin, CloudPluginItem,
    MarketPluginItem, PluginCloudSyncOutcome,
};
pub use plugin_manifest::{PluginManifest, PluginManifestEntry, PluginManifestError};
pub use process_observation::{FixedProcessController, WorkCnProcessController};
pub use product_account_state::{
    product_state_root_from, ProductAccountStateError, ProductAccountStateStore,
};
pub use product_adapter::TraeProduct;
#[cfg(feature = "sqlcipher")]
pub use production_catalog::{
    open_or_initialize_production_catalog, ProductionCatalogError, ProductionCatalogRuntime,
};
pub use progress::{
    persist_progress_snapshot, read_progress_snapshot, ProgressPhase, ProgressReporter,
    ProgressSnapshot, ThrottledProgressEmitter,
};
#[cfg(feature = "sqlcipher")]
pub use recovery_package::{
    export_recovery_package, import_recovery_package, upgrade_catalog_sidecar_with_lease,
    CatalogUpgradeResult, RecoveryPackageError, RecoveryPayload,
};
pub use relay_ledger::{RelayLedger, RelayLedgerEntry, RelayLedgerError};
pub use remint::{DeviceRemintService, RemintError};
pub use remote_device::{
    RealRemoteDeviceManager, RemoteDeviceHttpAdapter, ReqwestRemoteDeviceHttpAdapter,
};
pub use scan_authorization::{
    persisted_record_matches, PersistedScanAuthorization, ScanAuthorizationStore,
    ScanAuthorizationStoreError,
};
pub use snapshot_store::{sha256_file, FilesystemSnapshotStore};
#[cfg(feature = "sqlcipher")]
pub use sqlcipher::{
    catalog_operation_matches, FollowProjectExecution, SqlCipherProbe, WorkCnSyncExecutor,
};
pub use storage_deletion::{
    apply_deletion_plan, apply_deletion_plan_with_lease, build_deletion_plan,
    build_deletion_plan_with_lease, DeletionCandidate, DeletionError, DeletionTombstone,
    LiveReferenceIndex, StorageDeletionPlan,
};
pub use storage_root::{
    migrate_storage_root_with_lease, publish_initial_storage_root_pointer,
    read_storage_root_pointer, reserve_space, reserve_space_on_volumes, storage_space_status,
    SpaceReservation, SpaceReservationRequest, SpaceReservationSet, StorageMigrationResult,
    StorageRootBinding, StorageRootError, StorageRootPointer, StorageSpaceStatus,
    DEFAULT_STORAGE_WARNING_BYTES,
};
pub use work_cn_location::{
    TraeReadLocation, TraeReadLocationError, WorkCnReadLocation, WorkCnReadLocationError,
    DEFAULT_WORK_CN_DB_RELATIVE_PATH, DEFAULT_WORK_CN_ROOT_NAME,
};
#[cfg(feature = "sqlcipher")]
pub use work_cn_normalizer::WorkCnSourceNormalizer;
pub use workspace::StaticWorkspaceStateProvider;
#[cfg(feature = "sqlcipher")]
pub use workspace::{FixtureWorkspaceStateProvider, RealReadWorkspaceStateProvider};
