//! 账号 / 凭据 / 切号域（P12-R3 自 lib.rs 纯移动）。
//!
//! 覆盖：账号中心命令群（账号状态/凭证状态/凭据捕获/当前账号刷新/账号切换与
//! 承接意图/账号注册表视图/产品身份发现）、账号增删改查（列表/删除/备注名/
//! 手机号/归档）、凭据刷新三链（积分刷新/凭据换发/Trae CN 凭据刷新）与
//! Trae CN 健康检测、主库切号五步事务（切号编排/插件预检/接力台账/回滚）、
//! G18 主库自检三命令、设备重铸、本地占用统计与彻底删除记录，以及内嵌测试。
//! 命令体、错误码与 wire 形态零变化；授权上下文原语（capture_authorized_read_context /
//! ensure_authorization_context_current 族）、门禁三函数、组合根策略函数
//! （resolve_actual_master_account / resolve_remote_device_account）、实例状态
//! 工具（launch_instance_common / trae_instance_account_id / credential_login_state）
//! 留守组合根，经 `crate::` 引用（同 crate 内 mod 互引）。

use std::path::{Path, PathBuf};
use std::sync::Arc;

use chrono::{DateTime, SecondsFormat, Utc};
use serde::Serialize;
use sha2::{Digest, Sha256};
use tauri::Emitter;

use traesync_application::{
    checkin_transport_error_code, AccountEvidenceReaderPort, CheckinTransport,
    ManagedAccountSwitchService,
};
use traesync_domain::{
    AccountEvidence, AccountProfile, AccountSwitchPlan, AccountSwitchPreflight,
    AccountVerificationState, AuthorizationState, BrowseAccountNode, BrowseResult,
    CurrentAccountEvidence, EvidenceState, HandoffIntent, HandoffIntentState,
    ManagedAccountRuntime, OperationId, SyncScope, UserId,
};
use traesync_infrastructure::account_session_index::SessionIndexCacheStore;
use traesync_infrastructure::environment_registry::{
    master_data_dir, EnvironmentRegistry, MASTER_ENV_ID,
};
use traesync_infrastructure::library_account::{observed_profile_id, trae_cn_material_root_from};
use traesync_infrastructure::operation_lease::FileOperationLock;
// P15-7：产品选择指针目录派生单点在 infrastructure。
use traesync_infrastructure::product_state_root_from;
use traesync_infrastructure::trae_instance::{
    self as trae_instance_module, close_instance, dir_size_recursive, instance_data_dir,
    list_trae_processes, native_account_dir,
};
use traesync_infrastructure::{
    archive_login_user_id, backup_master_trio_to, capture_location_identity,
    capture_location_witness, construct_auth_identity, fetch_installed_plugins, get_user_info,
    get_user_info_full, get_user_info_full_for_oauth_client, handover_master_records_with_progress,
    load_account_fingerprint_salt, load_or_create_account_fingerprint_salt, master_login_material,
    master_database_path, master_db_activity_detected, reconcile_plan,
    resolve_current_catalog_generation_id, salted_user_id_fingerprint,
    switch_auth_identity_for_account, sync_account_cloud_plugins, trae_http_client,
    user_id_binding_fingerprint, user_id_display_fingerprint, AccountEvidenceReader,
    AccountRecord, AccountRegistry, AuthIdentitySwitch, AuthSwitchError, CheckinCredentialError,
    CheckinCredentialStore, CheckinProfileBinding, CloudPluginItem, ConstructAuthInput,
    CredentialApplyOutcome, CredentialBinding, CredentialMaintenanceStateStore, CredentialState,
    CredentialStatus, CredentialVault, DeviceRemintService, FixtureCheckinTransport,
    FixturePathGuard, FusedLoginStore, HandoverProgress, JsonHandoffIntentStore,
    JsonManagedAccountProfileStore, MasterHandover, MasterHandoverError, OAuthClient,
    OperationLease, PlatformFileIdentityProvider, PluginCloudSyncOutcome, PluginManifest,
    ProductAccountStateStore, RealCheckinRenewalService, RealCheckinTransport, RelayLedger,
    RelayLedgerEntry, RemintError, StorageRootBinding, TraeProduct, WorkCnReadLocation,
};

use crate::{
    capture_authorized_read_context, checkin_material_root, checkin_overview,
    ensure_authorization_context_current, ensure_checkin_http_enabled, ensure_login_real_mode,
    ensure_real_preview, recovery_root_path, resolve_actual_master_account,
    resolve_authorized_read_target_for_context, run_blocking, AppState, ManagedAccountRuntimeSlot,
    RuntimeMode,
};
// P13-5：实例工具族随迁 instance_tools。
use crate::checkin_schedule::{run_serial_batch, BatchProgressEvent, ProductBatchProgressEvent};
use crate::error_mapping::{
    credential_error_text, credential_maintenance_error_code, fixture_path_command_error,
    trae_cn_health_error_code,
};
use crate::instance_tools::{launch_instance_common, trae_instance_account_id};
use crate::library_archive::{prune_backups_if_enabled, resolve_master_backup_dir};
// P12-R3：清单构造原语随插件域迁 plugins。
use crate::plugins::manifest_entries_from_cloud;

/// Tauri 账号中心专用 wire DTO；不把 domain 内部指纹、备份引用或认证材料发给前端。
#[derive(Debug, Clone, Serialize)]
struct ManagedAccountProfileWireDto {
    profile_id: String,
    display_name: String,
    region: Option<String>,
    data_location_id: String,
    last_verified_at: Option<String>,
    verification_state: AccountVerificationState,
}

#[derive(Debug, Clone, Serialize)]
struct CurrentAccountEvidenceWireDto {
    profile_id: Option<String>,
    display_name: Option<String>,
    region: Option<String>,
    data_location_id: Option<String>,
    verification_state: AccountVerificationState,
    observed_at: Option<String>,
    reason: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
struct ManagedAccountVerificationWireDto {
    verification_state: AccountVerificationState,
    checked_at: Option<String>,
    reason: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
struct AccountSwitchPreflightWireDto {
    source_verified: bool,
    target_known: bool,
    target_verified: bool,
    target_location_known: bool,
    pending_sync_plan_cleared: bool,
    trae_closed: bool,
    ready: bool,
    reason: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
struct ManagedAccountSwitchWireDto {
    plan_id: String,
    source_profile_id: Option<String>,
    target_profile_id: String,
    source_data_location_id: Option<String>,
    target_data_location_id: String,
    preflight: AccountSwitchPreflightWireDto,
    state: traesync_domain::AccountSwitchState,
    created_at: String,
    failure_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct ManagedAccountsWireView {
    saved_accounts: Vec<ManagedAccountProfileWireDto>,
    current_account: CurrentAccountEvidenceWireDto,
    recent_verification: ManagedAccountVerificationWireDto,
    switch_state: Option<ManagedAccountSwitchWireDto>,
    handoff_intent: Option<HandoffIntentWireDto>,
    history_is_separate: bool,
}

/// 发现账号与本机档案/凭证的只读绑定状态。库存扫描不会隐式改变这些状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum AccountRegistryAuthorizationState {
    CurrentVerified,
    CredentialSaved,
    ProfileSaved,
    CredentialStale,
    CredentialInvalid,
    ReverificationRequired,
    Conflict,
    Unbound,
}

/// 账号注册表只返回历史统计和非敏感档案状态，不返回原始凭证内容。
#[derive(Debug, Clone, Serialize)]
struct AccountRegistryEntryWireDto {
    history_account_id: String,
    display_label: String,
    project_count: u64,
    session_count: u64,
    profile_id: Option<String>,
    profile_display_name: Option<String>,
    profile_verification_state: Option<AccountVerificationState>,
    credential_state: Option<CredentialState>,
    authorization_state: AccountRegistryAuthorizationState,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct AccountRegistryWireView {
    accounts: Vec<AccountRegistryEntryWireDto>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct HandoffIntentWireDto {
    intent_id: String,
    source_profile_id: Option<String>,
    target_profile_id: String,
    data_location_id: String,
    scope: SyncScope,
    state: HandoffIntentState,
    created_at: String,
    updated_at: String,
    failure_reason: Option<String>,
}

/// 产品身份只读结果：不返回 userId、token 或任何 native storage 正文。
#[derive(Debug, Clone, Serialize)]
pub(crate) struct ProductIdentityStateWireDto {
    product_id: String,
    display_name: String,
    identity_status: String,
    credential_status: String,
    relation_to_work: String,
}

/// 账号凭证状态只返回绑定摘要与状态，不返回密文或认证正文。
#[derive(Debug, Clone, Serialize)]
pub(crate) struct ManagedCredentialStatusWireDto {
    profile_id: String,
    state: CredentialState,
    data_location_id: String,
    format_version: Option<u32>,
}

/// 请求当前活动的可取消操作停止；无活动操作时直接返回 false。
///
/// 在 P6-2 会话索引/计划族退役后，仍然保留：切换账号与授权撤销时
/// 仍需要通知旧代次长任务（即使当前没有任何计划族命令）。
fn request_cancel_inner(state: &AppState) -> bool {
    let cancellation = state.active_sync_cancellation.lock().unwrap().clone();
    if let Some(cancellation) = cancellation {
        cancellation.request();
        true
    } else {
        false
    }
}

pub(crate) fn system_time_to_rfc3339(value: std::time::SystemTime) -> String {
    DateTime::<Utc>::from(value).to_rfc3339_opts(SecondsFormat::Millis, true)
}

fn unix_now_seconds() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

fn optional_system_time_to_rfc3339(value: Option<std::time::SystemTime>) -> Option<String> {
    value.map(system_time_to_rfc3339)
}

fn managed_profile_wire(profile: &AccountProfile) -> ManagedAccountProfileWireDto {
    ManagedAccountProfileWireDto {
        profile_id: profile.profile_id.clone(),
        display_name: profile.display_name.clone(),
        region: profile.region.clone(),
        data_location_id: profile.data_location_id.clone(),
        last_verified_at: optional_system_time_to_rfc3339(profile.last_verified_at),
        verification_state: profile.verification_state,
    }
}

fn managed_current_wire(account: &CurrentAccountEvidence) -> CurrentAccountEvidenceWireDto {
    CurrentAccountEvidenceWireDto {
        profile_id: account.profile_id.clone(),
        display_name: account.display_name.clone(),
        region: account.region.clone(),
        data_location_id: account.data_location_id.clone(),
        verification_state: account.verification_state,
        observed_at: optional_system_time_to_rfc3339(account.observed_at),
        reason: account.reason.clone(),
    }
}

fn managed_preflight_wire(preflight: &AccountSwitchPreflight) -> AccountSwitchPreflightWireDto {
    AccountSwitchPreflightWireDto {
        source_verified: preflight.source_verified,
        target_known: preflight.target_known,
        target_verified: preflight.target_verified,
        target_location_known: preflight.target_location_known,
        pending_sync_plan_cleared: preflight.pending_sync_plan_cleared,
        trae_closed: preflight.trae_closed,
        ready: preflight.ready,
        reason: preflight.reason.clone(),
    }
}

fn managed_switch_wire(plan: &AccountSwitchPlan) -> ManagedAccountSwitchWireDto {
    ManagedAccountSwitchWireDto {
        plan_id: plan.plan_id.clone(),
        source_profile_id: plan.source_profile_id.clone(),
        target_profile_id: plan.target_profile_id.clone(),
        source_data_location_id: plan.source_data_location_id.clone(),
        target_data_location_id: plan.target_data_location_id.clone(),
        preflight: managed_preflight_wire(&plan.preflight),
        state: plan.state,
        created_at: system_time_to_rfc3339(plan.created_at),
        failure_reason: plan.failure_reason.clone(),
    }
}

fn handoff_intent_wire(intent: &HandoffIntent) -> HandoffIntentWireDto {
    HandoffIntentWireDto {
        intent_id: intent.intent_id.clone(),
        source_profile_id: intent.source_profile_id.clone(),
        target_profile_id: intent.target_profile_id.clone(),
        data_location_id: intent.data_location_id.clone(),
        scope: intent.scope.clone(),
        state: intent.state,
        created_at: system_time_to_rfc3339(intent.created_at),
        updated_at: system_time_to_rfc3339(intent.updated_at),
        failure_reason: intent.failure_reason.clone(),
    }
}

fn managed_accounts_wire_view(
    runtime: &ManagedAccountRuntime,
    authorization_context_current: bool,
    handoff_intent: Option<&HandoffIntent>,
) -> ManagedAccountsWireView {
    let visible_current = if authorization_context_current {
        runtime.current_account.clone()
    } else {
        CurrentAccountEvidence::default()
    };
    let current_account = managed_current_wire(&visible_current);
    ManagedAccountsWireView {
        saved_accounts: runtime.profiles.iter().map(managed_profile_wire).collect(),
        recent_verification: ManagedAccountVerificationWireDto {
            verification_state: visible_current.verification_state,
            checked_at: visible_current.observed_at.map(system_time_to_rfc3339),
            reason: visible_current.reason.clone(),
        },
        current_account,
        // WaitingForTraeClosed 计划不含凭证；撤销旧授权后仍需显示，供用户完成外部切换。
        switch_state: runtime.switch_plan.as_ref().and_then(|plan| {
            if authorization_context_current
                || matches!(
                    plan.state,
                    traesync_domain::AccountSwitchState::WaitingForTraeClosed
                        | traesync_domain::AccountSwitchState::Applying
                        | traesync_domain::AccountSwitchState::Verifying
                        | traesync_domain::AccountSwitchState::ManualRecoveryRequired
                )
            {
                Some(managed_switch_wire(plan))
            } else {
                None
            }
        }),
        handoff_intent: handoff_intent.map(handoff_intent_wire),
        history_is_separate: true,
    }
}

fn account_registry_display_label(account: &BrowseAccountNode) -> String {
    UserId::from_verified(&account.user_id)
        // P15-6：融合版账号卡的来源标注（Work CN 客户端已停服迁移）。
        .map(|value| format!("{} · {}", TraeProduct::TraeCn.display_name(), user_id_display_fingerprint(&value)))
        .unwrap_or_else(|_| {
            let label = account.display_label.trim();
            if label.is_empty() {
                "来源账号（已识别）".to_string()
            } else {
                label.to_string()
            }
        })
}

/// 把单个历史账号与账号档案做精确匹配。零匹配不阻塞历史，多匹配失败关闭。
fn build_account_registry_entry(
    account: &BrowseAccountNode,
    profiles: &[AccountProfile],
    current_profile_id: Option<&str>,
    credential_states: &std::collections::HashMap<String, CredentialState>,
    salted_fingerprint: Option<&str>,
    legacy_fingerprint: Option<&str>,
) -> AccountRegistryEntryWireDto {
    let current_matches = salted_fingerprint
        .map(|fingerprint| {
            profiles
                .iter()
                .filter(|profile| {
                    profile.fingerprint_version == traesync_domain::ACCOUNT_FINGERPRINT_VERSION
                        && profile.user_fingerprint == fingerprint
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let legacy_matches = legacy_fingerprint
        .map(|fingerprint| {
            profiles
                .iter()
                .filter(|profile| {
                    profile.fingerprint_version
                        == traesync_domain::LEGACY_ACCOUNT_FINGERPRINT_VERSION
                        && profile.user_fingerprint == fingerprint
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    let ambiguous = current_matches.len() > 1
        || legacy_matches.len() > 1
        || (!current_matches.is_empty() && !legacy_matches.is_empty());
    let (profile, credential_state, authorization_state) = if ambiguous {
        (None, None, AccountRegistryAuthorizationState::Conflict)
    } else if let Some(profile) = current_matches.first().copied() {
        let credential_state = credential_states.get(&profile.profile_id).copied();
        let authorization_state = if profile.verification_state
            != AccountVerificationState::Verified
        {
            AccountRegistryAuthorizationState::ReverificationRequired
        } else if current_profile_id == Some(profile.profile_id.as_str()) {
            AccountRegistryAuthorizationState::CurrentVerified
        } else {
            match credential_state {
                Some(CredentialState::Saved) => AccountRegistryAuthorizationState::CredentialSaved,
                Some(CredentialState::Stale) => AccountRegistryAuthorizationState::CredentialStale,
                Some(CredentialState::Invalid) => {
                    AccountRegistryAuthorizationState::CredentialInvalid
                }
                Some(CredentialState::Missing) | None => {
                    AccountRegistryAuthorizationState::ProfileSaved
                }
            }
        };
        (Some(profile), credential_state, authorization_state)
    } else if let Some(profile) = legacy_matches.first().copied() {
        (
            Some(profile),
            None,
            AccountRegistryAuthorizationState::ReverificationRequired,
        )
    } else {
        (None, None, AccountRegistryAuthorizationState::Unbound)
    };

    AccountRegistryEntryWireDto {
        history_account_id: account.user_id.clone(),
        display_label: account_registry_display_label(account),
        project_count: account.project_count,
        session_count: account.session_count,
        profile_id: profile.map(|profile| profile.profile_id.clone()),
        profile_display_name: profile.map(|profile| profile.display_name.clone()),
        profile_verification_state: profile.map(|profile| profile.verification_state),
        credential_state,
        authorization_state,
    }
}

/// 仅撤销仍属于指定代次的读取上下文，避免旧异步读取清掉新授权。
fn invalidate_authorized_read_context_if_current(
    state: &AppState,
    generation: u64,
    expected: &AuthorizationState,
) {
    let invalidated = {
        let mut slot = state.authorization.lock().unwrap();
        let invalidated = slot.revoke_if_matches(generation, expected);
        if invalidated {
            // 与计划写入保持同一锁序：authorization -> pending_sync_plan。
            state.pending_sync_plan.lock().unwrap().take();
        }
        invalidated
    };
    if invalidated {
        request_cancel_inner(state);
    }
}

/// 账号切换确认时立即撤销旧授权、清空旧计划，并请求可取消操作停止。
///
/// 锁顺序固定为 authorization -> pending_sync_plan，避免与扫描授权和计划缓存交错。
fn invalidate_authorization_for_account_switch(state: &AppState) {
    {
        let mut authorization = state.authorization.lock().unwrap();
        authorization.revoke();
        state.pending_sync_plan.lock().unwrap().take();
    }
    // 保留 WaitingForTraeClosed 计划，供外部账号切换完成后重新授权并复核。
    state
        .managed_account_runtime
        .lock()
        .unwrap()
        .authorization_generation = None;
    request_cancel_inner(state);
}

/// 判断错误是否说明当前读取绑定已经不能继续信任。
fn should_invalidate_authorized_read_context(error: &str) -> bool {
    let normalized = error.to_ascii_lowercase();
    normalized.contains("authorization_mismatch")
        || normalized.contains("not_authorized")
        || normalized.contains("account_evidence_changed")
        || normalized.contains("data_location_changed")
        || normalized.contains("data_location_unavailable")
        || normalized.contains("account_evidence_unavailable")
        || error.contains("未授权")
        || error.contains("授权不匹配")
        || error.contains("账号证据")
        || error.contains("数据位置身份")
        || error.contains("APPDATA 未设置")
        || error.contains("APPDATA 必须")
        || error.contains("APPDATA 不可用")
        || error.contains("TRAE 默认根目录")
        || error.contains("TRAE 默认数据库")
}

/// 读取入口出现授权漂移时撤销旧绑定；普通目录库/存储错误不应误撤销用户授权。
fn invalidate_on_authorization_error<T>(
    state: &AppState,
    generation: u64,
    expected: &AuthorizationState,
    error: String,
) -> Result<T, String> {
    if should_invalidate_authorized_read_context(&error) {
        invalidate_authorized_read_context_if_current(state, generation, expected);
    }
    Err(error)
}

/// 在创建恢复区前先完成 fixture 边界校验，创建后再复核真实物理路径。
///
/// 这样即使配置指向不存在的越界路径，也不会先产生目录副作用。
fn prepare_recovery_root(state: &AppState, guard: &FixturePathGuard) -> Result<PathBuf, String> {
    let candidate = recovery_root_path(state)?.to_path_buf();
    let safe_candidate = guard
        .validate_shared_recovery_root(&candidate)
        .map_err(fixture_path_command_error)?;
    std::fs::create_dir_all(&safe_candidate).map_err(|_| "恢复区不可用".to_string())?;
    guard
        .validate_shared_recovery_root(&safe_candidate)
        .map_err(fixture_path_command_error)
}

/// 只读发现已安装产品中的账号身份关系。
///
/// 该命令只解析固定产品根目录中的账号证据，不写 storage、数据库或凭据，
/// 也不做 token 交换、refresh、签到、设备注册或设备重铸。
#[tauri::command]
pub(crate) fn get_product_identity_state() -> Vec<ProductIdentityStateWireDto> {
    let products = [TraeProduct::WorkCn, TraeProduct::TraeCn];
    let evidence: Vec<(TraeProduct, Option<UserId>, String)> = products
        .into_iter()
        .map(|product| {
            let result = WorkCnReadLocation::discover_for(product)
                .ok()
                .map(|location| {
                    let account = AccountEvidenceReader::new().read_account_evidence(
                        location.canonical_root(),
                        std::time::SystemTime::now(),
                    );
                    let status = match account.evidence_state {
                        EvidenceState::Conflict => "conflict",
                        _ if account.user_id.is_some() => "recognized",
                        _ => "unknown",
                    };
                    (account.user_id, status.to_string())
                });
            let (user_id, identity_status) = result.unwrap_or((None, "unknown".to_string()));
            (product, user_id, identity_status)
        })
        .collect();

    let work_user_id = evidence
        .iter()
        .find(|(product, _, status)| *product == TraeProduct::WorkCn && status == "recognized")
        .and_then(|(_, user_id, _)| user_id.as_ref())
        .cloned();

    evidence
        .into_iter()
        .map(|(product, user_id, identity_status)| {
            let relation_to_work = match product {
                TraeProduct::WorkCn => "current_product".to_string(),
                TraeProduct::TraeCn => {
                    match (
                        work_user_id.as_ref(),
                        user_id.as_ref(),
                        identity_status.as_str(),
                    ) {
                        (Some(work), Some(cn), "recognized") if work == cn => {
                            "same_identity".to_string()
                        }
                        (Some(_), Some(_), "recognized") => "different_identity".to_string(),
                        _ => "unknown".to_string(),
                    }
                }
            };
            ProductIdentityStateWireDto {
                product_id: product.product_id().to_string(),
                display_name: product.display_name().to_string(),
                identity_status,
                credential_status: match product {
                    TraeProduct::WorkCn => "managed_by_work".to_string(),
                    TraeProduct::TraeCn => "need_authorization".to_string(),
                },
                relation_to_work,
            }
        })
        .collect()
}

/// 构造 ManagedAccountSwitch 的档案 ID；身份作用域同时绑定数据位置与完整指纹。
///
/// 前端只看到组合摘要，完整指纹仍只用于后端档案匹配，不进入 wire DTO。
fn managed_profile_id(data_location_id: &str, user_fingerprint: &str) -> String {
    let mut digest = Sha256::new();
    digest.update(data_location_id.as_bytes());
    digest.update([0]);
    digest.update(user_fingerprint.as_bytes());
    let encoded = hex::encode(digest.finalize());
    format!("profile-{}", &encoded[..16])
}

fn managed_verification_state(account: &AccountEvidence) -> AccountVerificationState {
    match account.evidence_state {
        EvidenceState::Verified => AccountVerificationState::Verified,
        EvidenceState::SingleSource => AccountVerificationState::SingleSource,
        EvidenceState::Conflict => AccountVerificationState::Conflict,
        EvidenceState::Missing => AccountVerificationState::Unknown,
        EvidenceState::Expired => AccountVerificationState::Expired,
        EvidenceState::FingerprintChanged => AccountVerificationState::FingerprintChanged,
    }
}

/// 将当前账号白名单证据转成账号中心 DTO；此 DTO 不包含认证正文。
fn managed_current_evidence(
    account: &AccountEvidence,
    data_location_id: String,
    observed_at: std::time::SystemTime,
    fingerprint_salt: Option<&[u8]>,
) -> CurrentAccountEvidence {
    let user_fingerprint = account.user_id.as_ref().and_then(|user_id| {
        fingerprint_salt.map(|salt| salted_user_id_fingerprint(salt, user_id.as_str()))
    });
    let profile_id = user_fingerprint
        .as_deref()
        .map(|fingerprint| managed_profile_id(&data_location_id, fingerprint));
    let display_name = user_fingerprint
        .as_deref()
        // P15-6：融合版账号显示名（Work CN 客户端已停服迁移）。
        .map(|fingerprint| format!("{} · {}", TraeProduct::TraeCn.display_name(), &fingerprint[..16]));
    let verification_state = managed_verification_state(account);
    CurrentAccountEvidence {
        profile_id,
        display_name,
        user_fingerprint,
        fingerprint_version: if fingerprint_salt.is_some() && account.user_id.is_some() {
            traesync_domain::ACCOUNT_FINGERPRINT_VERSION
        } else {
            traesync_domain::LEGACY_ACCOUNT_FINGERPRINT_VERSION
        },
        region: Some("cn".to_string()),
        data_location_id: Some(data_location_id),
        verification_state,
        observed_at: Some(observed_at),
        reason: match verification_state {
            AccountVerificationState::Verified => None,
            _ => Some(format!("account_evidence_{verification_state:?}").to_ascii_lowercase()),
        },
    }
}

struct ManagedAccountObservation {
    account: AccountEvidence,
    data_location_id: String,
    data_root: PathBuf,
    storage_root: PathBuf,
    observed_at: std::time::SystemTime,
}

struct ManagedAccountStorePaths {
    profile_path: PathBuf,
    salt_path: PathBuf,
}

/// 解析普通账号档案读取路径。该函数不创建目录、salt、锁文件或 catalog sidecar。
fn managed_account_store_paths_for_read(
    state: &AppState,
) -> Result<Option<ManagedAccountStorePaths>, String> {
    if state.storage_root.is_empty() {
        return Err("account_profile_store_unavailable".to_string());
    }
    let (storage_root, recovery_root) = match state.runtime_mode {
        RuntimeMode::Fixture => {
            let (generation, authorization) = {
                let slot = state.authorization.lock().unwrap();
                if matches!(slot.state, AuthorizationState::NotAuthorized) {
                    return Ok(None);
                }
                (slot.generation, slot.state.clone())
            };
            let (data_root, _, storage_root) =
                resolve_authorized_read_target_for_context(state, generation, &authorization)?;
            let guard = FixturePathGuard::new(&data_root).map_err(fixture_path_command_error)?;
            let recovery_root = guard
                .validate_shared_recovery_root(recovery_root_path(state)?)
                .map_err(fixture_path_command_error)?;
            ensure_authorization_context_current(state, generation, &authorization)?;
            (storage_root, recovery_root)
        }
        RuntimeMode::RealReadPreview => {
            let runtime = state
                .production_catalog
                .as_ref()
                .ok_or_else(|| "account_profile_store_unavailable".to_string())?;
            if !runtime.storage_root.exists() {
                return Ok(None);
            }
            // 只复核既有存储根绑定；不调用可能修复 generation.json 的 catalog verify。
            StorageRootBinding::open_existing(
                &runtime.storage_root,
                None,
                Some(&runtime.catalog_id),
            )
            .map_err(|error| error.to_string())?;
            (runtime.storage_root.clone(), runtime.recovery_root.clone())
        }
    };
    Ok(Some(ManagedAccountStorePaths {
        profile_path: storage_root.join("managed-accounts.json"),
        salt_path: recovery_root.join("managed-account-fingerprint-salt.bin"),
    }))
}

fn managed_account_store_for_read(
    state: &AppState,
) -> Result<Option<JsonManagedAccountProfileStore>, String> {
    let Some(paths) = managed_account_store_paths_for_read(state)? else {
        return Ok(None);
    };
    let fingerprint_salt = load_account_fingerprint_salt(&paths.salt_path)?;
    JsonManagedAccountProfileStore::for_read(paths.profile_path, fingerprint_salt).map(Some)
}

/// 返回固定恢复区的承接意图存储。该函数不创建目录或文件。
fn handoff_intent_store_for_read(
    state: &AppState,
) -> Result<Option<JsonHandoffIntentStore>, String> {
    let root = match state.runtime_mode {
        RuntimeMode::Fixture => {
            let path = recovery_root_path(state)?;
            if !path.exists() {
                return Ok(None);
            }
            path.to_path_buf()
        }
        RuntimeMode::RealReadPreview => state
            .production_catalog
            .as_ref()
            .ok_or_else(|| "handoff_intent_unavailable".to_string())?
            .recovery_root
            .clone(),
    };
    Ok(Some(JsonHandoffIntentStore::new(root)))
}

fn load_handoff_intent_for_state(state: &AppState) -> Result<Option<HandoffIntent>, String> {
    let Some(store) = handoff_intent_store_for_read(state)? else {
        return Ok(None);
    };
    store.load()
}

/// 返回固定恢复区中的独立凭证库。凭证库与目录库使用不同命名空间，
/// 其状态不进入目录库、同步计划或操作 manifest。
fn credential_vault_for_state(state: &AppState) -> Result<CredentialVault, String> {
    let recovery_root = match state.runtime_mode {
        RuntimeMode::Fixture => PathBuf::from(&state.recovery_root),
        RuntimeMode::RealReadPreview => state
            .production_catalog
            .as_ref()
            .ok_or_else(|| "credential_vault_unavailable".to_string())?
            .recovery_root
            .clone(),
    };
    if recovery_root.as_os_str().is_empty() {
        return Err("credential_vault_unavailable".to_string());
    }
    Ok(CredentialVault::new(recovery_root.join("credential-vault")))
}

/// 启动时只恢复凭证切换的最小上下文，不恢复旧同步计划或任何历史正文。
///
/// 扫描 manifest 与承接意图前先取得固定恢复区/数据位置租约；发现冲突时保持
/// pending journal，不自动重放或覆盖 `storage.json`。
pub(crate) fn recover_managed_account_runtime_from_disk(
    recovery_root: &Path,
) -> Result<ManagedAccountRuntimeSlot, String> {
    let mut empty = ManagedAccountRuntimeSlot::default();
    if !recovery_root.is_dir() {
        return Ok(empty);
    }

    let vault = CredentialVault::new(recovery_root.join("credential-vault"));
    let intent_store = JsonHandoffIntentStore::new(recovery_root);
    let mut records = vault
        .pending_recoveries(recovery_root)
        .map_err(credential_error_text)?;
    let mut intent = intent_store.load()?;
    let has_active_intent = intent.as_ref().is_some_and(|value| {
        matches!(
            value.state,
            HandoffIntentState::Prepared
                | HandoffIntentState::Switching
                | HandoffIntentState::TargetVerified
                | HandoffIntentState::PreviewReady
                | HandoffIntentState::ManualRecoveryRequired
        )
    });
    if records.is_empty() && !has_active_intent {
        return Ok(empty);
    }

    let lease_location_id = intent
        .as_ref()
        .map(|value| value.data_location_id.clone())
        .or_else(|| records.first().map(|value| value.data_location_id.clone()))
        .ok_or_else(|| "credential_recovery_binding_missing".to_string())?;
    let lease = OperationLease::acquire_shared(recovery_root, &lease_location_id)
        .map_err(|error| format!("operation_lease_unavailable:{error}"))?;

    // 租约内重新读取，避免启动扫描与另一实例的 manifest 发布交错。
    records = vault
        .pending_recoveries(recovery_root)
        .map_err(credential_error_text)?;
    intent = intent_store.load()?;
    if records.len() > 1 {
        drop(lease);
        return Err("credential_multiple_recoveries".to_string());
    }

    // journal 已经收口时，pending_recoveries 不再返回记录；仍需保留承接意图，
    // 否则重启后账号中心会丢失“目标已复核、等待 fresh 预览”的状态。
    if records.is_empty() {
        if let Some(current_intent) = intent.as_ref() {
            if matches!(
                current_intent.state,
                HandoffIntentState::Prepared
                    | HandoffIntentState::Switching
                    | HandoffIntentState::TargetVerified
                    | HandoffIntentState::PreviewReady
                    | HandoffIntentState::ManualRecoveryRequired
            ) {
                let manual_recovery =
                    current_intent.state == HandoffIntentState::ManualRecoveryRequired;
                let state = match current_intent.state {
                    HandoffIntentState::Prepared => traesync_domain::AccountSwitchState::Preflight,
                    HandoffIntentState::ManualRecoveryRequired => {
                        traesync_domain::AccountSwitchState::ManualRecoveryRequired
                    }
                    _ => traesync_domain::AccountSwitchState::Verifying,
                };
                let reason = if manual_recovery {
                    "credential_manual_recovery_required"
                } else {
                    "credential_recovery_target_verified"
                }
                .to_string();
                empty.runtime.current_account = if manual_recovery {
                    CurrentAccountEvidence {
                        verification_state: AccountVerificationState::ManualRecoveryRequired,
                        reason: Some(reason.clone()),
                        ..CurrentAccountEvidence::default()
                    }
                } else {
                    CurrentAccountEvidence::default()
                };
                empty.runtime.switch_plan = Some(AccountSwitchPlan {
                    plan_id: current_intent.intent_id.clone(),
                    source_profile_id: current_intent.source_profile_id.clone(),
                    target_profile_id: current_intent.target_profile_id.clone(),
                    source_data_location_id: None,
                    target_data_location_id: current_intent.data_location_id.clone(),
                    preflight: AccountSwitchPreflight {
                        source_verified: true,
                        target_known: true,
                        target_verified: current_intent.state != HandoffIntentState::Prepared,
                        target_location_known: true,
                        pending_sync_plan_cleared: true,
                        trae_closed: true,
                        ready: !manual_recovery,
                        reason: Some(reason.clone()),
                    },
                    backup_reference: current_intent.credential_operation_id.clone(),
                    expected_target_fingerprint: String::new(),
                    state,
                    created_at: current_intent.created_at,
                    failure_reason: Some(reason),
                });
            }
        }
        drop(lease);
        return Ok(empty);
    }

    if let (Some(record), Some(current_intent)) = (records.first(), intent.as_mut()) {
        if let Some(manifest_intent_id) = record.intent_id.as_deref() {
            if manifest_intent_id != current_intent.intent_id {
                drop(lease);
                return Err("handoff_intent_conflict".to_string());
            }
        }
        if current_intent.target_profile_id != record.profile_id
            || current_intent.data_location_id != record.data_location_id
        {
            drop(lease);
            return Err("credential_binding_mismatch".to_string());
        }
        if current_intent.state == HandoffIntentState::Prepared {
            // 写入已完成但进程可能在绑定步骤崩溃；在租约内补齐唯一非敏感句柄。
            current_intent.state = if record.state == "manual_recovery_required" {
                HandoffIntentState::ManualRecoveryRequired
            } else {
                HandoffIntentState::Switching
            };
            current_intent.credential_operation_id = Some(record.operation_id.clone());
            current_intent.updated_at = std::time::SystemTime::now();
            intent_store.publish(current_intent)?;
        }
    }

    if let Some(record) = records.first() {
        let active_intent = intent.as_ref();
        let target_profile_id = active_intent
            .map(|value| value.target_profile_id.clone())
            .unwrap_or_else(|| record.profile_id.clone());
        let source_profile_id = active_intent.and_then(|value| value.source_profile_id.clone());
        let target_data_location_id = active_intent
            .map(|value| value.data_location_id.clone())
            .unwrap_or_else(|| record.data_location_id.clone());
        let manual_recovery = record.state == "manual_recovery_required"
            || active_intent
                .is_some_and(|value| value.state == HandoffIntentState::ManualRecoveryRequired);
        let plan_id = active_intent
            .map(|value| value.intent_id.clone())
            .unwrap_or_else(|| format!("credential-recovery-{}", record.operation_id));
        let now = std::time::SystemTime::now();
        let state = if manual_recovery {
            traesync_domain::AccountSwitchState::ManualRecoveryRequired
        } else {
            traesync_domain::AccountSwitchState::Applying
        };
        let reason = if manual_recovery {
            Some("credential_manual_recovery_required".to_string())
        } else {
            Some("credential_recovery_pending_refresh".to_string())
        };
        empty.runtime.current_account = if manual_recovery {
            CurrentAccountEvidence {
                verification_state: AccountVerificationState::ManualRecoveryRequired,
                reason: reason.clone(),
                ..CurrentAccountEvidence::default()
            }
        } else {
            CurrentAccountEvidence::default()
        };
        empty.runtime.switch_plan = Some(AccountSwitchPlan {
            plan_id,
            source_profile_id,
            target_profile_id,
            source_data_location_id: None,
            target_data_location_id,
            preflight: AccountSwitchPreflight {
                source_verified: true,
                target_known: true,
                target_verified: true,
                target_location_known: true,
                pending_sync_plan_cleared: true,
                trae_closed: true,
                // 这是恢复上下文，不是新计划；fresh 账号证据仍由 refresh 再次验证。
                ready: true,
                reason: reason.clone(),
            },
            backup_reference: Some(record.operation_id.clone()),
            expected_target_fingerprint: record.user_fingerprint.clone(),
            state,
            created_at: active_intent.map(|value| value.created_at).unwrap_or(now),
            failure_reason: reason,
        });
    }
    drop(lease);
    Ok(empty)
}

/// 恢复区损坏或并发冲突时，保留一个不可执行的人工恢复计划，
/// 让 UI 明确显示阻断，而不是把错误吞掉后误报为空闲状态。
pub(crate) fn recovery_error_runtime(
    recovery_root: &Path,
    reason: &str,
) -> ManagedAccountRuntimeSlot {
    let intent = JsonHandoffIntentStore::new(recovery_root)
        .load()
        .ok()
        .flatten();
    let target_profile_id = intent
        .as_ref()
        .map(|value| value.target_profile_id.clone())
        .unwrap_or_default();
    let data_location_id = intent
        .as_ref()
        .map(|value| value.data_location_id.clone())
        .unwrap_or_default();
    let source_profile_id = intent
        .as_ref()
        .and_then(|value| value.source_profile_id.clone());
    let plan_id = intent
        .as_ref()
        .map(|value| value.intent_id.clone())
        .unwrap_or_else(|| "credential-recovery-error".to_string());
    let created_at = intent
        .as_ref()
        .map(|value| value.created_at)
        .unwrap_or_else(std::time::SystemTime::now);
    let failure_reason = Some(format!("credential_recovery_failed:{reason}"));
    ManagedAccountRuntimeSlot {
        runtime: ManagedAccountRuntime {
            profiles: Vec::new(),
            current_account: CurrentAccountEvidence {
                verification_state: AccountVerificationState::ManualRecoveryRequired,
                reason: failure_reason.clone(),
                ..CurrentAccountEvidence::default()
            },
            switch_plan: Some(AccountSwitchPlan {
                plan_id,
                source_profile_id,
                target_profile_id,
                source_data_location_id: None,
                target_data_location_id: data_location_id,
                preflight: AccountSwitchPreflight {
                    source_verified: false,
                    target_known: false,
                    target_verified: false,
                    target_location_known: false,
                    pending_sync_plan_cleared: false,
                    trae_closed: false,
                    ready: false,
                    reason: failure_reason.clone(),
                },
                backup_reference: None,
                expected_target_fingerprint: String::new(),
                state: traesync_domain::AccountSwitchState::ManualRecoveryRequired,
                created_at,
                failure_reason,
            }),
        },
        authorization_generation: None,
    }
}

fn credential_status_wire(status: CredentialStatus) -> ManagedCredentialStatusWireDto {
    ManagedCredentialStatusWireDto {
        profile_id: status.profile_id,
        state: status.state,
        data_location_id: status.data_location_id,
        format_version: status.format_version,
    }
}

/// 本机凭证保存/切换的独立 Gate。检查必须发生在任何授权、vault 或目标文件访问之前。
fn ensure_local_credential_switch_enabled(state: &AppState) -> Result<(), String> {
    if state.capabilities.local_credential_switch {
        Ok(())
    } else {
        Err("gate_not_qualified".to_string())
    }
}

fn profile_credential_binding(profile: &AccountProfile) -> CredentialBinding {
    CredentialBinding::new(
        profile.profile_id.clone(),
        profile.data_location_id.clone(),
        profile.user_fingerprint.clone(),
    )
}

fn managed_credential_statuses_inner(
    state: &AppState,
) -> Result<Vec<ManagedCredentialStatusWireDto>, String> {
    let Some(store) = managed_account_store_for_read(state)? else {
        return Ok(Vec::new());
    };
    let mut runtime = ManagedAccountRuntime::default();
    ManagedAccountSwitchService::new(&store)
        .load_profiles(&mut runtime)
        .map_err(|error| error.to_string())?;
    let vault = credential_vault_for_state(state)?;
    Ok(runtime
        .profiles
        .iter()
        .map(|profile| credential_status_wire(vault.status(&profile_credential_binding(profile))))
        .collect())
}

/// 计算一次已授权数据位置的稳定路径与进程观测，不执行任何写入。
fn authorized_storage_context(
    state: &AppState,
    generation: u64,
    authorization: &AuthorizationState,
) -> Result<(PathBuf, String, String), String> {
    let (data_root, db_path, _storage_root) =
        resolve_authorized_read_target_for_context(state, generation, authorization)?;
    let db_relative_path = db_path
        .strip_prefix(&data_root)
        .map_err(|_| "data_location_changed".to_string())?
        .to_string_lossy()
        .into_owned();
    let location = capture_location_identity(
        &PlatformFileIdentityProvider::new(),
        &data_root,
        &db_relative_path,
    )
    .map_err(|error| error.to_string())?;
    let process =
        state
            .process_controller
            .observe(&location.data_location_id, &data_root, &db_relative_path);
    if let Some(error_code) = process.error_code() {
        return Err(error_code.to_string());
    }
    Ok((data_root, location.data_location_id, db_relative_path))
}

fn credential_recovery_root_for_context(
    state: &AppState,
    data_root: &Path,
) -> Result<PathBuf, String> {
    match state.runtime_mode {
        RuntimeMode::Fixture => {
            let guard = FixturePathGuard::new(data_root).map_err(fixture_path_command_error)?;
            guard
                .validate_shared_recovery_root(recovery_root_path(state)?)
                .map_err(fixture_path_command_error)
        }
        RuntimeMode::RealReadPreview => state
            .production_catalog
            .as_ref()
            .ok_or_else(|| "credential_vault_unavailable".to_string())
            .map(|runtime| runtime.recovery_root.clone()),
    }
}

fn update_handoff_intent_state(
    state: &AppState,
    next_state: HandoffIntentState,
    failure_reason: Option<String>,
) -> Result<(), String> {
    let Some(store) = handoff_intent_store_for_read(state)? else {
        return Ok(());
    };
    let Some(mut intent) = store.load()? else {
        return Ok(());
    };
    if intent.state != next_state && !handoff_state_transition_allowed(intent.state, next_state) {
        return Err("handoff_intent_state_transition_invalid".to_string());
    }
    let recovery_root = store
        .path()
        .parent()
        .ok_or_else(|| "handoff_intent_unavailable".to_string())?;
    let lease = OperationLease::acquire_shared(recovery_root, &intent.data_location_id)
        .map_err(|error| format!("operation_lease_unavailable:{error}"))?;
    // 取得租约后再次读取，避免两个实例基于同一旧状态互相覆盖。
    let current = store
        .load()?
        .ok_or_else(|| "handoff_intent_conflict".to_string())?;
    if current.intent_id != intent.intent_id
        || current.updated_at != intent.updated_at
        || current.state != intent.state
    {
        drop(lease);
        return Err("handoff_intent_conflict".to_string());
    }
    intent.state = next_state;
    intent.updated_at = std::time::SystemTime::now();
    intent.failure_reason = failure_reason;
    store.publish(&intent)?;
    drop(lease);
    Ok(())
}

fn handoff_state_transition_allowed(current: HandoffIntentState, next: HandoffIntentState) -> bool {
    matches!(
        (current, next),
        (HandoffIntentState::Prepared, HandoffIntentState::Switching)
            | (HandoffIntentState::Prepared, HandoffIntentState::Expired)
            | (
                HandoffIntentState::Prepared,
                HandoffIntentState::ManualRecoveryRequired
            )
            | (
                HandoffIntentState::Switching,
                HandoffIntentState::TargetVerified
            )
            | (HandoffIntentState::Switching, HandoffIntentState::Expired)
            | (
                HandoffIntentState::Switching,
                HandoffIntentState::ManualRecoveryRequired
            )
            | (
                HandoffIntentState::TargetVerified,
                HandoffIntentState::PreviewReady
            )
            | (
                HandoffIntentState::TargetVerified,
                HandoffIntentState::Expired
            )
            | (
                HandoffIntentState::TargetVerified,
                HandoffIntentState::ManualRecoveryRequired
            )
            | (
                HandoffIntentState::PreviewReady,
                HandoffIntentState::Expired
            )
            | (
                HandoffIntentState::PreviewReady,
                HandoffIntentState::ManualRecoveryRequired
            )
            | (
                HandoffIntentState::ManualRecoveryRequired,
                HandoffIntentState::Expired
            )
    )
}

/// 记录账号切换失败；失败状态必须同时落入内存计划与持久承接意图。
///
/// 这里采用尽力更新策略：原始切换错误仍返回给调用方，状态持久化失败不会
/// 覆盖更有价值的首个错误，但会保留已有恢复现场供下次启动协调。
fn mark_account_switch_failure(state: &AppState, target_profile_id: &str, reason: &str) {
    if let Ok(Some(store)) = managed_account_store_for_read(state) {
        let service = ManagedAccountSwitchService::new(&store);
        let mut runtime_slot = state.managed_account_runtime.lock().unwrap();
        let should_mark = runtime_slot
            .runtime
            .switch_plan
            .as_ref()
            .is_some_and(|plan| plan.target_profile_id == target_profile_id);
        if should_mark {
            if let Some(plan_id) = runtime_slot
                .runtime
                .switch_plan
                .as_ref()
                .map(|plan| plan.plan_id.clone())
            {
                let _ = service.mark_manual_recovery_required(
                    &mut runtime_slot.runtime,
                    &plan_id,
                    reason,
                );
            }
        }
    }
    if load_handoff_intent_for_state(state)
        .ok()
        .flatten()
        .is_some_and(|intent| intent.target_profile_id == target_profile_id)
    {
        let _ = update_handoff_intent_state(
            state,
            HandoffIntentState::ManualRecoveryRequired,
            Some(reason.to_string()),
        );
    }
}

fn create_handoff_intent_inner(
    state: &AppState,
    target_profile_id: &str,
    requested_scope: Option<SyncScope>,
) -> Result<HandoffIntent, String> {
    let (generation, authorization) = capture_authorized_read_context(state)?;
    let store = managed_account_store_for_read(state)?
        .ok_or_else(|| "account_profile_store_unavailable".to_string())?;
    let mut runtime = ManagedAccountRuntime::default();
    ManagedAccountSwitchService::new(&store)
        .load_profiles(&mut runtime)
        .map_err(|error| error.to_string())?;
    let source = {
        let runtime_slot = state.managed_account_runtime.lock().unwrap();
        if runtime_slot.authorization_generation != Some(generation) {
            return Err("source_account_not_verified".to_string());
        }
        runtime_slot.runtime.current_account.clone()
    };
    if source.verification_state != AccountVerificationState::Verified
        || source.data_location_id.is_none()
    {
        return Err("source_account_not_verified".to_string());
    }
    let target = runtime
        .profiles
        .iter()
        .find(|profile| profile.profile_id == target_profile_id)
        .ok_or_else(|| "target_profile_not_found".to_string())?;
    if target.verification_state != AccountVerificationState::Verified {
        return Err("target_account_not_verified".to_string());
    }
    if target.profile_id == source.profile_id.clone().unwrap_or_default() {
        return Err("target_account_is_current".to_string());
    }
    if target.data_location_id != source.data_location_id.clone().unwrap_or_default() {
        return Err("target_data_location_mismatch".to_string());
    }
    let pending = state
        .pending_sync_plan
        .lock()
        .unwrap()
        .clone()
        .ok_or_else(|| "handoff_fresh_plan_required".to_string())?;
    if pending.generation != generation {
        return Err("handoff_fresh_plan_required".to_string());
    }
    let scope = requested_scope.unwrap_or_else(|| pending.plan.scope_snapshot().clone());
    if scope != *pending.plan.scope_snapshot() {
        return Err("handoff_scope_mismatch".to_string());
    }
    let (_data_root, _db_path, storage_root) =
        resolve_authorized_read_target_for_context(state, generation, &authorization)?;
    let catalog_generation =
        resolve_current_catalog_generation_id(&storage_root).map_err(|error| error.to_string())?;
    ensure_authorization_context_current(state, generation, &authorization)?;
    let now = std::time::SystemTime::now();
    let intent = HandoffIntent {
        intent_id: OperationId::new().as_str().to_string(),
        source_profile_id: source.profile_id,
        target_profile_id: target.profile_id.clone(),
        data_location_id: target.data_location_id.clone(),
        scope,
        catalog_id: state
            .production_catalog
            .as_ref()
            .map(|runtime| runtime.catalog_id.clone()),
        catalog_generation: Some(catalog_generation),
        schema_version: Some(pending.plan.schema_fingerprint().to_string()),
        mapping_version: Some(pending.plan.mapping_version().to_string()),
        credential_operation_id: None,
        state: HandoffIntentState::Prepared,
        created_at: now,
        updated_at: now,
        failure_reason: None,
    };
    publish_handoff_intent_for_state(state, &intent)?;
    Ok(intent)
}

/// 发布前置承接意图；恢复区必须已由固定边界检查准备完成。
fn publish_handoff_intent_for_state(
    state: &AppState,
    intent: &HandoffIntent,
) -> Result<(), String> {
    let Some(store) = handoff_intent_store_for_read(state)? else {
        return Err("handoff_intent_unavailable".to_string());
    };
    let recovery_root = store
        .path()
        .parent()
        .ok_or_else(|| "handoff_intent_unavailable".to_string())?;
    let lease = OperationLease::acquire_shared(recovery_root, &intent.data_location_id)
        .map_err(|error| format!("operation_lease_unavailable:{error}"))?;
    let publish_result = store.publish(intent);
    drop(lease);
    publish_result?;
    Ok(())
}

/// 将一次已完成凭证替换绑定到承接意图；只写非敏感 operation id。
fn bind_handoff_credential_operation(state: &AppState, operation_id: &str) -> Result<(), String> {
    let Some(store) = handoff_intent_store_for_read(state)? else {
        return Err("handoff_intent_unavailable".to_string());
    };
    let Some(mut intent) = store.load()? else {
        return Err("handoff_intent_unavailable".to_string());
    };
    let recovery_root = store
        .path()
        .parent()
        .ok_or_else(|| "handoff_intent_unavailable".to_string())?;
    let lease = OperationLease::acquire_shared(recovery_root, &intent.data_location_id)
        .map_err(|error| format!("operation_lease_unavailable:{error}"))?;
    let current = store
        .load()?
        .ok_or_else(|| "handoff_intent_conflict".to_string())?;
    if current.intent_id != intent.intent_id || current.updated_at != intent.updated_at {
        drop(lease);
        return Err("handoff_intent_conflict".to_string());
    }
    intent.credential_operation_id = Some(operation_id.to_string());
    intent.updated_at = std::time::SystemTime::now();
    let result = store.publish(&intent);
    drop(lease);
    result
}

/// 账号刷新先完成位置与账号证据复核，再创建或加载账号摘要 salt。
fn managed_account_store_for_refresh(
    state: &AppState,
    generation: u64,
    authorization: &AuthorizationState,
    observation: &ManagedAccountObservation,
) -> Result<(JsonManagedAccountProfileStore, Option<Vec<u8>>), String> {
    let profile_path = observation.storage_root.join("managed-accounts.json");
    let recovery_root = match state.runtime_mode {
        RuntimeMode::Fixture => FixturePathGuard::new(&observation.data_root)
            .map_err(fixture_path_command_error)?
            .validate_shared_recovery_root(recovery_root_path(state)?)
            .map_err(fixture_path_command_error)?,
        RuntimeMode::RealReadPreview => {
            let runtime = state
                .production_catalog
                .as_ref()
                .ok_or_else(|| "account_profile_store_unavailable".to_string())?;
            if runtime.storage_root != observation.storage_root {
                return Err("authorization_mismatch".to_string());
            }
            runtime.recovery_root.clone()
        }
    };
    let salt_path = recovery_root.join("managed-account-fingerprint-salt.bin");
    let existing_salt = load_account_fingerprint_salt(&salt_path)?;

    if observation.account.evidence_state != EvidenceState::Verified {
        let store = JsonManagedAccountProfileStore::for_read(profile_path, existing_salt.clone())?;
        return Ok((store, existing_salt));
    }

    let fingerprint_salt = match existing_salt {
        Some(salt) => salt,
        None => {
            // v3 或未绑定 v2 档案会在这里失败关闭；只有空库或全 v1 legacy 可首次建 salt。
            let legacy_store = JsonManagedAccountProfileStore::for_read(&profile_path, None)?;
            let mut legacy_runtime = ManagedAccountRuntime::default();
            ManagedAccountSwitchService::new(&legacy_store)
                .load_profiles(&mut legacy_runtime)
                .map_err(|error| error.to_string())?;
            ensure_authorization_context_current(state, generation, authorization)?;
            if state.runtime_mode == RuntimeMode::Fixture {
                let guard = FixturePathGuard::new(&observation.data_root)
                    .map_err(fixture_path_command_error)?;
                let prepared = prepare_recovery_root(state, &guard)?;
                if prepared != recovery_root {
                    return Err("account_fingerprint_salt_unavailable".to_string());
                }
            }
            let salt = load_or_create_account_fingerprint_salt(&salt_path)?;
            ensure_authorization_context_current(state, generation, authorization)?;
            salt
        }
    };
    let store = JsonManagedAccountProfileStore::with_fingerprint_salt(
        profile_path,
        fingerprint_salt.clone(),
    )?;
    Ok((store, Some(fingerprint_salt)))
}

fn detect_managed_current_account(
    state: &AppState,
    generation: u64,
    authorization: &AuthorizationState,
) -> Result<ManagedAccountObservation, String> {
    let now = std::time::SystemTime::now();
    let (data_root, db_path, storage_root) =
        resolve_authorized_read_target_for_context(state, generation, authorization)?;
    let account = state
        .workbench_reader
        .read_account_evidence(&data_root, now);
    if account.evidence_state == EvidenceState::Verified && account.user_id.is_none() {
        return Err("account_evidence_unavailable".to_string());
    }
    let data_location_id = match state.runtime_mode {
        RuntimeMode::Fixture => {
            let db_relative_path = db_path
                .strip_prefix(&data_root)
                .map_err(|_| "data_location_changed".to_string())?
                .to_string_lossy()
                .into_owned();
            let witness = capture_location_witness(
                &PlatformFileIdentityProvider::new(),
                &data_root,
                &db_relative_path,
            )
            .map_err(|error| error.to_string())?;
            witness.data_location_id
        }
        RuntimeMode::RealReadPreview => {
            let (witness, user_fingerprint, auth_fingerprint) = {
                let slot = state.authorization.lock().unwrap();
                if slot.generation != generation || slot.state != *authorization {
                    return Err("authorization_mismatch".to_string());
                }
                (
                    slot.real_location_witness.clone(),
                    slot.real_user_fingerprint.clone(),
                    slot.real_auth_fingerprint.clone(),
                )
            };
            let witness = witness.ok_or_else(|| "authorization_mismatch".to_string())?;
            let user_fingerprint =
                user_fingerprint.ok_or_else(|| "authorization_mismatch".to_string())?;
            let auth_fingerprint =
                auth_fingerprint.ok_or_else(|| "authorization_mismatch".to_string())?;
            let current_user_fingerprint =
                account.user_id.as_ref().map(user_id_binding_fingerprint);
            let current_auth_fingerprint = account
                .auth_fingerprint
                .as_ref()
                .map(|value| value.0.clone());
            if account.evidence_state != EvidenceState::Verified
                || current_user_fingerprint.as_deref() != Some(user_fingerprint.as_str())
                || current_auth_fingerprint.as_deref() != Some(auth_fingerprint.as_str())
            {
                return Err("account_evidence_changed".to_string());
            }
            witness.data_location_id
        }
    };
    ensure_authorization_context_current(state, generation, authorization)?;
    Ok(ManagedAccountObservation {
        account,
        data_location_id,
        data_root,
        storage_root,
        observed_at: now,
    })
}

fn managed_account_view_inner(state: &AppState) -> Result<ManagedAccountsWireView, String> {
    // 测试夹具和热重载路径可能绕过 `run`；首次读取时再做一次幂等恢复扫描。
    let needs_recovery_scan = state
        .managed_account_runtime
        .lock()
        .unwrap()
        .runtime
        .switch_plan
        .is_none();
    if needs_recovery_scan && !state.recovery_root.is_empty() {
        let recovered =
            match recover_managed_account_runtime_from_disk(Path::new(&state.recovery_root)) {
                Ok(recovered) => recovered,
                Err(error) => recovery_error_runtime(Path::new(&state.recovery_root), &error),
            };
        let mut runtime_slot = state.managed_account_runtime.lock().unwrap();
        if runtime_slot.runtime.switch_plan.is_none() {
            *runtime_slot = recovered;
        }
    }
    let mut profiles_runtime = ManagedAccountRuntime::default();
    if let Some(store) = managed_account_store_for_read(state)? {
        ManagedAccountSwitchService::new(&store)
            .load_profiles(&mut profiles_runtime)
            .map_err(|error| error.to_string())?;
    }
    let handoff_intent = match load_handoff_intent_for_state(state) {
        Ok(intent) => intent,
        Err(error) => {
            let recovery_visible = state
                .managed_account_runtime
                .lock()
                .unwrap()
                .runtime
                .switch_plan
                .as_ref()
                .is_some_and(|plan| {
                    plan.state == traesync_domain::AccountSwitchState::ManualRecoveryRequired
                });
            if recovery_visible {
                None
            } else {
                return Err(error);
            }
        }
    };

    // 固定锁序：authorization -> runtime。投影期间不执行文件 I/O。
    let authorization = state.authorization.lock().unwrap();
    let mut runtime_slot = state.managed_account_runtime.lock().unwrap();
    runtime_slot.runtime.profiles = profiles_runtime.profiles;
    let context_current = matches!(authorization.state, AuthorizationState::Authorized { .. })
        && runtime_slot.authorization_generation == Some(authorization.generation);
    Ok(managed_accounts_wire_view(
        &runtime_slot.runtime,
        context_current,
        handoff_intent.as_ref(),
    ))
}

/// 返回独立账号中心状态。该命令不读取认证正文，也不执行账号切换。
#[tauri::command]
pub(crate) fn get_managed_account_state(
    state: tauri::State<AppState>,
) -> Result<ManagedAccountsWireView, String> {
    managed_account_view_inner(&state)
}

/// 返回每个已保存账号的凭证状态；不读取明文登录材料。
#[tauri::command]
pub(crate) fn get_managed_credential_statuses(
    state: tauri::State<AppState>,
) -> Result<Vec<ManagedCredentialStatusWireDto>, String> {
    managed_credential_statuses_inner(&state)
}

/// 保存当前已验证账号的登录命名空间。
///
/// 只在后端确认 TRAE 未运行、数据位置仍是同一见证时读取 `storage.json`；
/// 认证值进入当前 Windows 用户 DPAPI 凭证库，前端只收到状态。
#[tauri::command]
pub(crate) fn capture_current_account_credential(
    state: tauri::State<AppState>,
) -> Result<ManagedCredentialStatusWireDto, String> {
    capture_current_account_credential_inner(&state)
}

fn capture_current_account_credential_inner(
    state: &AppState,
) -> Result<ManagedCredentialStatusWireDto, String> {
    ensure_local_credential_switch_enabled(state)?;
    let (generation, authorization) = capture_authorized_read_context(state)?;
    let (data_root, data_location_id, db_relative_path) =
        authorized_storage_context(state, generation, &authorization)?;
    let observation = detect_managed_current_account(state, generation, &authorization)?;
    if observation.data_location_id != data_location_id
        || observation.account.evidence_state != EvidenceState::Verified
    {
        return Err("account_evidence_changed".to_string());
    }
    let (store, fingerprint_salt) =
        managed_account_store_for_refresh(state, generation, &authorization, &observation)?;
    let evidence = managed_current_evidence(
        &observation.account,
        observation.data_location_id.clone(),
        observation.observed_at,
        fingerprint_salt.as_deref(),
    );
    let profile_id = evidence
        .profile_id
        .clone()
        .ok_or_else(|| "account_evidence_unavailable".to_string())?;
    let user_fingerprint = evidence
        .user_fingerprint
        .clone()
        .ok_or_else(|| "account_evidence_unavailable".to_string())?;
    let binding = CredentialBinding::new(
        profile_id,
        observation.data_location_id.clone(),
        user_fingerprint,
    );
    let storage_path =
        CredentialVault::locate_storage_json(&data_root).map_err(credential_error_text)?;
    let vault = credential_vault_for_state(state)?;
    let recovery_root = credential_recovery_root_for_context(state, &data_root)?;
    let lease = acquire_credential_switch_lease(state, &data_root, &recovery_root)?;
    // 捕获前再次确认 TRAE 仍关闭，避免账号材料读取与外部写入交错。
    let process =
        state
            .process_controller
            .observe(&data_location_id, &data_root, &db_relative_path);
    if let Some(error_code) = process.error_code() {
        drop(lease);
        return Err(error_code.to_string());
    }
    let status = match vault.capture(&binding, &storage_path) {
        Ok(status) => status,
        Err(error) => {
            drop(lease);
            return Err(credential_error_text(error));
        }
    };
    if let Err(error) = ensure_authorization_context_current(state, generation, &authorization) {
        drop(lease);
        return Err(error);
    }
    drop(lease);
    let _ = store;
    Ok(credential_status_wire(status))
}

/// 若目标账号已有 DPAPI 凭证，则执行受控认证命名空间替换；
/// 未保存凭证时返回 `false`，调用方可继续走外部账号管理器等待态。
fn try_apply_saved_credential(
    state: &AppState,
    target_profile_id: &str,
    bind_handoff_intent: bool,
) -> Result<Option<CredentialApplyOutcome>, String> {
    ensure_local_credential_switch_enabled(state)?;
    let (generation, authorization) = capture_authorized_read_context(state)?;
    let (data_root, data_location_id, db_relative_path) =
        authorized_storage_context(state, generation, &authorization)?;
    let store = managed_account_store_for_read(state)?
        .ok_or_else(|| "account_profile_store_unavailable".to_string())?;
    let mut runtime = ManagedAccountRuntime::default();
    ManagedAccountSwitchService::new(&store)
        .load_profiles(&mut runtime)
        .map_err(|error| error.to_string())?;
    let target = runtime
        .profiles
        .iter()
        .find(|profile| profile.profile_id == target_profile_id)
        .ok_or_else(|| "target_profile_not_found".to_string())?;
    if target.data_location_id != data_location_id {
        return Err("target_data_location_mismatch".to_string());
    }
    let binding = profile_credential_binding(target);
    let vault = credential_vault_for_state(state)?;
    let status = vault.status(&binding);
    match status.state {
        CredentialState::Missing => Ok(None),
        CredentialState::Stale => Err("credential_stale".to_string()),
        CredentialState::Invalid => Err("credential_invalid".to_string()),
        CredentialState::Saved => {
            let intent_id = if bind_handoff_intent {
                load_handoff_intent_for_state(state)?.and_then(|intent| {
                    if intent.target_profile_id == target_profile_id
                        && intent.state == HandoffIntentState::Prepared
                    {
                        Some(intent.intent_id)
                    } else {
                        None
                    }
                })
            } else {
                None
            };
            let storage_path =
                CredentialVault::locate_storage_json(&data_root).map_err(credential_error_text)?;
            let recovery_root = credential_recovery_root_for_context(state, &data_root)?;
            let lease = acquire_credential_switch_lease(state, &data_root, &recovery_root)?;
            // 取得共享租约后再次确认进程仍关闭，避免预检与写入之间发生 TOCTOU。
            let process =
                state
                    .process_controller
                    .observe(&data_location_id, &data_root, &db_relative_path);
            if let Some(error_code) = process.error_code() {
                drop(lease);
                return Err(error_code.to_string());
            }
            let outcome = vault
                .apply_with_intent(
                    &binding,
                    &storage_path,
                    &recovery_root,
                    true,
                    intent_id.as_deref(),
                )
                .map_err(credential_error_text)?;
            // 写后再次确认 TRAE 没有在替换窗口启动；无法确认时保留恢复现场并停止。
            let process =
                state
                    .process_controller
                    .observe(&data_location_id, &data_root, &db_relative_path);
            if let Some(error_code) = process.error_code() {
                let _ = vault.mark_manual_recovery_required_with_binding(
                    &recovery_root,
                    &outcome.operation_id,
                    &binding,
                    intent_id.as_deref(),
                );
                drop(lease);
                return Err(error_code.to_string());
            }
            if let Err(error) =
                ensure_authorization_context_current(state, generation, &authorization)
            {
                let _ = vault.mark_manual_recovery_required_with_binding(
                    &recovery_root,
                    &outcome.operation_id,
                    &binding,
                    intent_id.as_deref(),
                );
                drop(lease);
                return Err(error);
            }
            drop(lease);
            Ok(Some(outcome))
        }
    }
}

/// 凭证替换与目录库/同步操作共享固定恢复区和数据位置租约。
fn acquire_credential_switch_lease(
    state: &AppState,
    data_root: &Path,
    recovery_root: &Path,
) -> Result<OperationLease, String> {
    match state.runtime_mode {
        RuntimeMode::Fixture => {
            let guard = FixturePathGuard::new(data_root).map_err(fixture_path_command_error)?;
            guard
                .acquire_shared_path_lease(recovery_root, data_root)
                .map_err(fixture_path_command_error)
        }
        RuntimeMode::RealReadPreview => {
            OperationLease::acquire_shared_for_path(recovery_root, data_root)
                .map_err(|error| format!("operation_lease_unavailable:{error}"))
        }
    }
}

/// 凭证 journal 收口也复用同一数据位置租约，避免与下一次切换/扫描交错。
fn mark_credential_journal(
    state: &AppState,
    data_root: &Path,
    operation_id: &str,
    target_verified: bool,
    binding: &CredentialBinding,
    intent_id: Option<&str>,
) -> Result<(), String> {
    let recovery_root = credential_recovery_root_for_context(state, data_root)?;
    let lease = acquire_credential_switch_lease(state, data_root, &recovery_root)?;
    let vault = credential_vault_for_state(state)?;
    let result = if target_verified {
        vault.mark_target_verified_with_binding(&recovery_root, operation_id, binding, intent_id)
    } else {
        vault.mark_manual_recovery_required_with_binding(
            &recovery_root,
            operation_id,
            binding,
            intent_id,
        )
    };
    drop(lease);
    result.map_err(credential_error_text)
}

/// 凭证已经替换后，若后续编排步骤失败，必须把 journal 收口到人工恢复态。
/// 该辅助只在授权上下文仍有效时尝试写入；无法再次证明位置时保持 fail-closed，
/// 不自动恢复或覆盖当前 storage.json。
fn mark_credential_operation_manual_recovery_for_state(
    state: &AppState,
    target_profile_id: &str,
    operation_id: &str,
) -> Result<(), String> {
    let (generation, authorization) = capture_authorized_read_context(state)?;
    let (data_root, _, _) = authorized_storage_context(state, generation, &authorization)?;
    let store = managed_account_store_for_read(state)?
        .ok_or_else(|| "account_profile_store_unavailable".to_string())?;
    let mut runtime = ManagedAccountRuntime::default();
    ManagedAccountSwitchService::new(&store)
        .load_profiles(&mut runtime)
        .map_err(|error| error.to_string())?;
    let target = runtime
        .profiles
        .iter()
        .find(|profile| profile.profile_id == target_profile_id)
        .ok_or_else(|| "target_profile_not_found".to_string())?;
    let binding = profile_credential_binding(target);
    let intent_id = load_handoff_intent_for_state(state)?.and_then(|intent| {
        (intent.target_profile_id == target_profile_id
            && intent.state != HandoffIntentState::Expired
            && intent.credential_operation_id.as_deref() == Some(operation_id))
        .then_some(intent.intent_id)
    });
    mark_credential_journal(
        state,
        &data_root,
        operation_id,
        false,
        &binding,
        intent_id.as_deref(),
    )
}

/// 把凭证写入后的失败统一映射为人工恢复，避免 UI 把半完成切换显示成普通失败。
fn credential_post_apply_failure(
    state: &AppState,
    target_profile_id: &str,
    operation_id: Option<&str>,
    reason: &str,
) -> String {
    if let Some(operation_id) = operation_id {
        let _ = mark_credential_operation_manual_recovery_for_state(
            state,
            target_profile_id,
            operation_id,
        );
        mark_account_switch_failure(
            state,
            target_profile_id,
            "credential_manual_recovery_required",
        );
        return "credential_manual_recovery_required".to_string();
    }
    mark_account_switch_failure(state, target_profile_id, reason);
    reason.to_string()
}

/// 创建账号切换计划。该命令只清理旧的待执行同步计划并更新内存状态，
/// 不读取凭证、不改写 TRAE 文件，也不执行外部账号切换。
#[tauri::command]
pub(crate) fn prepare_managed_account_switch(
    target_profile_id: String,
    state: tauri::State<AppState>,
) -> Result<ManagedAccountsWireView, String> {
    prepare_managed_account_switch_inner(&state, &target_profile_id)
}

fn prepare_managed_account_switch_inner(
    state: &AppState,
    target_profile_id: &str,
) -> Result<ManagedAccountsWireView, String> {
    let (generation, authorization) = capture_authorized_read_context(state)?;
    let store = managed_account_store_for_read(state)?
        .ok_or_else(|| "account_profile_store_unavailable".to_string())?;

    let (current_account, current_generation) = {
        let runtime_slot = state.managed_account_runtime.lock().unwrap();
        (
            runtime_slot.runtime.current_account.clone(),
            runtime_slot.authorization_generation,
        )
    };
    if current_generation != Some(generation)
        || current_account.verification_state != AccountVerificationState::Verified
    {
        return Err("source_account_not_verified".to_string());
    }

    // 选择新的目标账号即视为用户明确放弃旧计划；不删除快照、备份或失败证据。
    // `take()` 的返回值表示“之前是否存在计划”，不是清理结果；清理动作完成后
    // 预检字段必须为 true，否则一个被正常清掉的旧计划会错误阻断新切换。
    state.pending_sync_plan.lock().unwrap().take();
    let pending_sync_plan_cleared = true;
    let mut candidate = ManagedAccountRuntime {
        profiles: Vec::new(),
        current_account,
        switch_plan: None,
    };
    let service = ManagedAccountSwitchService::new(&store);
    service
        .prepare_switch(
            &mut candidate,
            target_profile_id,
            AccountSwitchPreflight {
                source_verified: true,
                target_known: false,
                target_verified: false,
                target_location_known: false,
                pending_sync_plan_cleared,
                trae_closed: false,
                ready: false,
                reason: None,
            },
            OperationId::new().as_str().to_string(),
            std::time::SystemTime::now(),
        )
        .map_err(|error| error.to_string())?;

    ensure_authorization_context_current(state, generation, &authorization)?;
    let mut runtime_slot = state.managed_account_runtime.lock().unwrap();
    runtime_slot.runtime = candidate;
    runtime_slot.authorization_generation = Some(generation);
    let handoff_intent = load_handoff_intent_for_state(state)?;
    Ok(managed_accounts_wire_view(
        &runtime_slot.runtime,
        true,
        handoff_intent.as_ref(),
    ))
}

/// 进入外部账号切换等待态。
///
/// 目前不直接写 TRAE 登录材料；命令会撤销旧读取授权并保留等待计划，
/// 用户完成外部切换后重新授权/扫描即可由 refresh 命令完成目标复核。
#[tauri::command]
pub(crate) fn switch_account(
    target_profile_id: String,
    state: tauri::State<AppState>,
) -> Result<ManagedAccountsWireView, String> {
    ensure_local_credential_switch_enabled(&state)?;
    switch_account_inner(&state, &target_profile_id)
}

fn switch_account_inner(
    state: &AppState,
    target_profile_id: &str,
) -> Result<ManagedAccountsWireView, String> {
    switch_account_inner_with_intent(state, target_profile_id, true)
}

fn switch_account_inner_with_intent(
    state: &AppState,
    target_profile_id: &str,
    expire_handoff_intent: bool,
) -> Result<ManagedAccountsWireView, String> {
    ensure_local_credential_switch_enabled(state)?;
    let store = managed_account_store_for_read(state)?
        .ok_or_else(|| "account_profile_store_unavailable".to_string())?;
    let service = ManagedAccountSwitchService::new(&store);
    // 所有目标、计划和承接意图检查必须先于凭证文件副作用。
    let plan_id =
        validate_account_switch_request(state, target_profile_id, !expire_handoff_intent)?;
    // 已保存凭证时执行真实认证命名空间替换；缺少凭证则保留外部管理器等待态。
    let credential_outcome =
        match try_apply_saved_credential(state, target_profile_id, !expire_handoff_intent) {
            Ok(outcome) => outcome,
            Err(error) => {
                mark_account_switch_failure(state, target_profile_id, &error);
                return Err(error);
            }
        };
    let credential_operation_id = credential_outcome
        .as_ref()
        .map(|outcome| outcome.operation_id.clone());
    let mut runtime_error = None;
    {
        let mut runtime_slot = state.managed_account_runtime.lock().unwrap();
        let plan_matches = runtime_slot
            .runtime
            .switch_plan
            .as_ref()
            .is_some_and(|plan| {
                plan.target_profile_id == target_profile_id && plan.plan_id == plan_id
            });
        if !plan_matches {
            runtime_error = Some("account_switch_plan_mismatch".to_string());
        } else if let Some(outcome) = credential_outcome.as_ref() {
            // 恢复句柄只留在后端计划中，供后续目标复核完成 journal 收口。
            if let Some(plan) = runtime_slot.runtime.switch_plan.as_mut() {
                plan.backup_reference = Some(outcome.operation_id.clone());
            }
            if let Err(error) =
                service.mark_applying_after_credential(&mut runtime_slot.runtime, &plan_id)
            {
                runtime_error = Some(error.to_string());
            }
        } else {
            if let Err(error) =
                service.mark_waiting_for_trae_closed(&mut runtime_slot.runtime, &plan_id)
            {
                runtime_error = Some(error.to_string());
            }
        }
    }
    if let Some(reason) = runtime_error {
        let public_error = credential_post_apply_failure(
            state,
            target_profile_id,
            credential_operation_id.as_deref(),
            &reason,
        );
        return Err(public_error);
    }
    if let Some(outcome) = credential_outcome.as_ref() {
        if !expire_handoff_intent {
            if let Err(error) = bind_handoff_credential_operation(state, &outcome.operation_id) {
                let public_error = credential_post_apply_failure(
                    state,
                    target_profile_id,
                    Some(outcome.operation_id.as_str()),
                    &error,
                );
                return Err(public_error);
            }
        }
        if expire_handoff_intent {
            if let Err(error) = update_handoff_intent_state(
                state,
                HandoffIntentState::Expired,
                Some("standalone_account_switch".to_string()),
            ) {
                let public_error = credential_post_apply_failure(
                    state,
                    target_profile_id,
                    Some(outcome.operation_id.as_str()),
                    &error,
                );
                return Err(public_error);
            }
        } else if let Err(error) =
            update_handoff_intent_state(state, HandoffIntentState::Switching, None)
        {
            let public_error = credential_post_apply_failure(
                state,
                target_profile_id,
                Some(outcome.operation_id.as_str()),
                &error,
            );
            return Err(public_error);
        }
    } else if expire_handoff_intent {
        update_handoff_intent_state(
            state,
            HandoffIntentState::Expired,
            Some("standalone_account_switch".to_string()),
        )?;
    } else {
        update_handoff_intent_state(state, HandoffIntentState::Switching, None)?;
    }
    // 凭证写入成功后只使用已提交的内存运行态生成返回视图，避免二次文件投影失败
    // 把半完成切换误报为普通错误；必要的恢复标记仍在授权上下文有效时完成。
    let handoff_intent = match load_handoff_intent_for_state(state) {
        Ok(intent) => intent,
        Err(error) => {
            let public_error = credential_post_apply_failure(
                state,
                target_profile_id,
                credential_operation_id.as_deref(),
                &error,
            );
            invalidate_authorization_for_account_switch(state);
            return Err(public_error);
        }
    };
    invalidate_authorization_for_account_switch(state);
    let runtime = state
        .managed_account_runtime
        .lock()
        .unwrap()
        .runtime
        .clone();
    Ok(managed_accounts_wire_view(
        &runtime,
        false,
        handoff_intent.as_ref(),
    ))
}

/// 绑定一次切换的不可变计划和可选承接意图；任何文件写入前完成。
fn validate_account_switch_request(
    state: &AppState,
    target_profile_id: &str,
    require_handoff_intent: bool,
) -> Result<String, String> {
    let plan = {
        let runtime_slot = state.managed_account_runtime.lock().unwrap();
        runtime_slot
            .runtime
            .switch_plan
            .clone()
            .ok_or_else(|| "account_switch_plan_not_found".to_string())?
    };
    if plan.target_profile_id != target_profile_id {
        return Err("account_switch_plan_mismatch".to_string());
    }
    if !plan.preflight.ready || plan.state != traesync_domain::AccountSwitchState::Preflight {
        return Err("account_switch_preflight_blocked".to_string());
    }
    if require_handoff_intent {
        let intent = load_handoff_intent_for_state(state)?
            .ok_or_else(|| "handoff_intent_unavailable".to_string())?;
        if intent.state != HandoffIntentState::Prepared
            || intent.target_profile_id != target_profile_id
            || intent.source_profile_id != plan.source_profile_id
            || intent.data_location_id != plan.target_data_location_id
        {
            return Err("handoff_intent_mismatch".to_string());
        }
    }
    Ok(plan.plan_id)
}

/// 保存最小承接意图，再执行账号切换；切换后必须重新授权并重新生成 fresh 计划。
#[tauri::command]
pub(crate) fn switch_and_handoff(
    target_profile_id: String,
    scope: Option<SyncScope>,
    state: tauri::State<AppState>,
) -> Result<ManagedAccountsWireView, String> {
    ensure_local_credential_switch_enabled(&state)?;
    let existing_intent = load_handoff_intent_for_state(&state)?;
    let can_reuse_existing =
        validate_handoff_reuse(existing_intent.as_ref(), &target_profile_id, scope.as_ref())?;
    if !can_reuse_existing {
        create_handoff_intent_inner(&state, &target_profile_id, scope)?;
    }
    match switch_account_inner_with_intent(&state, &target_profile_id, false) {
        Ok(view) => Ok(view),
        Err(error) => {
            mark_account_switch_failure(&state, &target_profile_id, &error);
            Err(error)
        }
    }
}

/// 判断已有承接意图能否复用；活动中的意图不允许被新请求静默覆盖。
fn validate_handoff_reuse(
    existing: Option<&HandoffIntent>,
    target_profile_id: &str,
    requested_scope: Option<&SyncScope>,
) -> Result<bool, String> {
    let Some(intent) = existing else {
        return Ok(false);
    };
    match intent.state {
        HandoffIntentState::Prepared => {
            if intent.target_profile_id != target_profile_id {
                return Err("handoff_target_mismatch".to_string());
            }
            if requested_scope.is_some_and(|scope| scope != &intent.scope) {
                return Err("handoff_scope_mismatch".to_string());
            }
            Ok(true)
        }
        HandoffIntentState::Switching
        | HandoffIntentState::TargetVerified
        | HandoffIntentState::PreviewReady => Err("handoff_intent_in_progress".to_string()),
        HandoffIntentState::Expired | HandoffIntentState::ManualRecoveryRequired => Ok(false),
    }
}

/// 仅保存当前选择的承接意图，不执行账号切换。
#[tauri::command]
pub(crate) fn prepare_handoff_intent(
    target_profile_id: String,
    scope: Option<SyncScope>,
    state: tauri::State<AppState>,
) -> Result<HandoffIntentWireDto, String> {
    let intent = create_handoff_intent_inner(&state, &target_profile_id, scope)?;
    Ok(handoff_intent_wire(&intent))
}

/// Trae CN 产品账号根目录：与 Work CN 的签到材料、官方 Trae CN 目录均隔离。
/// 这里只保存 Trae Sync 自己的加密凭据、非敏感档案和当前账号指针。
pub(crate) fn trae_cn_material_root(state: &AppState) -> Result<PathBuf, String> {
    if state.storage_root.is_empty() {
        return Err("trae_cn_storage_unavailable".to_string());
    }
    Ok(trae_cn_material_root_from(Path::new(&state.storage_root)))
}

/// 融合版登录代次容器根（P15-4，ADR-0035）：`{storage_root}/fused-login`。
/// 与阶段一迁移探针的落盘位置一致，容器由 `fused_login_credential` 管理。
pub(crate) fn fused_login_root(state: &AppState) -> Result<PathBuf, String> {
    if state.storage_root.is_empty() {
        return Err("trae_cn_storage_unavailable".to_string());
    }
    Ok(Path::new(&state.storage_root).join("fused-login"))
}

/// 融合版产品工具选择指针根（P15-7，ADR-0035）：`{storage_root}/product-state`。
/// 旧 CN 产品池目录随 P15-7 数据删除，工具选择指针（current-account.json）
/// 迁出 `products/trae_cn`，避免指针写入复活已删除目录；与登录代次容器
/// （fused-login）、签到材料根（checkin）三者互不重叠。派生规则单点在
/// infrastructure 的 `product_state_root_from`。
pub(crate) fn product_account_state_root(state: &AppState) -> Result<PathBuf, String> {
    if state.storage_root.is_empty() {
        return Err("trae_cn_storage_unavailable".to_string());
    }
    Ok(product_state_root_from(Path::new(&state.storage_root)))
}

/// 更新单账号的归档标记（U-6 W4 资产库三态筛选：全部/活跃/已归档）。
/// 归档只影响前端视图分组，档案、凭据与本地记录全部保留。
/// 返回账号是否存在；不存在视为错误由前端提示。
#[tauri::command]
pub(crate) fn set_account_archived(
    profile_id: String,
    archived: bool,
    state: tauri::State<AppState>,
) -> Result<(), String> {
    if profile_id.is_empty() || profile_id.len() > 256 {
        return Err("checkin_profile_invalid".to_string());
    }
    let material_root = checkin_material_root(&state)?;
    let store = CheckinCredentialStore::new(&material_root);
    let _profile_lease = store
        .lock_profile(&profile_id)
        .map_err(|_| "checkin_storage_unavailable".to_string())?;
    let updated = AccountRegistry::new(&material_root)
        .set_archived(&profile_id, archived)
        .map_err(|_| "checkin_registry_invalid".to_string())?;
    if !updated {
        return Err("checkin_profile_not_found".to_string());
    }
    Ok(())
}

/// 已登录账号条目（非敏感白名单：不含设备 ID/公钥等绑定材料）。
#[derive(Debug, Clone, Serialize)]
pub(crate) struct CheckinAccountWireDto {
    profile_id: String,
    account_id: String,
    screen_name: String,
    avatar_url: String,
    /// 是否归档（U-6 W4 资产库三态筛选：前端据此分组为全部/活跃/已归档）。
    archived: bool,
    last_verified_at: Option<String>,
}

/// 登录完成回执与 Trae CN 登录回执 DTO 已迁 oauth_login 模块（P12-R3）。

/// Trae CN 独立账号池条目；不把账号 ID、设备绑定或令牌发给前端。
#[derive(Debug, Clone, Serialize)]
pub(crate) struct TraeCnAccountWireDto {
    profile_id: String,
    display_name: String,
    avatar_url: String,
    last_verified_at: Option<String>,
    /// 本地凭据可用性；不代替健康检测的服务端实调结果。
    status: &'static str,
    is_current: bool,
}

/// Trae CN 健康检测逐账号回执；只返回结果和稳定错误码。
#[derive(Debug, Clone, Serialize)]
pub(crate) struct TraeCnHealthEntryDto {
    profile_id: String,
    screen_name: String,
    healthy: bool,
    error_code: Option<String>,
}

// P12-R3：登录 profile_id 生成、OAuth 临时浏览器档案机制（根目录/隔离目录/
// 超龄清理/Chromium 定位/登录页拉起）已迁 oauth_login 模块。

/// 列出已登录账号（非敏感白名单）；注册表尚未创建时返回空表。
/// pub(crate)：oauth_login 登录命令链测试经 crate:: 引用。
pub(crate) fn list_checkin_accounts_inner(
    state: &AppState,
) -> Result<Vec<CheckinAccountWireDto>, String> {
    ensure_login_real_mode(&state)?;
    let material_root = checkin_material_root(state)?;
    let registry = AccountRegistry::new(&material_root);
    let records = registry
        .load()
        .map_err(|_| "checkin_registry_invalid".to_string())?;
    Ok(records
        .into_iter()
        .map(|record| CheckinAccountWireDto {
            last_verified_at: std::time::SystemTime::UNIX_EPOCH
                .checked_add(std::time::Duration::from_secs(
                    record.last_verified_at_unix_seconds,
                ))
                .map(system_time_to_rfc3339),
            profile_id: record.profile_id,
            account_id: record.account_id,
            screen_name: record.screen_name,
            avatar_url: record.avatar_url,
            archived: record.archived,
        })
        .collect())
}

#[tauri::command]
pub(crate) fn list_checkin_accounts(
    state: tauri::State<AppState>,
) -> Result<Vec<CheckinAccountWireDto>, String> {
    list_checkin_accounts_inner(&state)
}

/// 列出融合版 TRAE 账号（P15-4，ADR-0035）：账号主体 = Work 签到档案
/// （16 个迁移账号），登录代次状态徽章 = `.fused` 容器，工具选择 =
/// 产品选择指针（ADR-0034：工具选中与官方身份分开表达，切号后的官方
/// 身份由主库实测/环境档案承载）。只返回展示字段，不发起网络请求。
fn list_trae_cn_accounts_inner(state: &AppState) -> Result<Vec<TraeCnAccountWireDto>, String> {
    ensure_login_real_mode(state)?;
    let material_root = checkin_material_root(state)?;
    let fused_root = fused_login_root(state)?;
    // 工具选择指针存于 product-state 根（P15-7 迁出旧池目录；
    // 并发选择意图语义不变）。
    let state_root = product_account_state_root(state)?;
    let current_profile_id = ProductAccountStateStore::new(&state_root)
        .load()
        .map_err(|_| "trae_cn_account_state_invalid".to_string())?;
    let records = AccountRegistry::new(&material_root)
        .load()
        .map_err(|_| "trae_cn_registry_invalid".to_string())?;
    let store = FusedLoginStore::new(&fused_root);
    let now = unix_now_seconds();
    Ok(records
        .into_iter()
        .filter(|record| !record.archived)
        .map(|record| {
            // 登录代次徽章：容器 access 未过期 = active；已过期 = expired；
            // 无容器（未铸造）= unknown。
            let status = store
                .load(&record.profile_id, &record.account_id)
                .map(|fused| {
                    if fused.access_token_expires_at_unix_seconds > now {
                        "active"
                    } else {
                        "expired"
                    }
                })
                .unwrap_or("unknown");
            TraeCnAccountWireDto {
                profile_id: record.profile_id.clone(),
                display_name: record
                    .display_name
                    .unwrap_or_else(|| record.screen_name.clone()),
                avatar_url: record.avatar_url,
                last_verified_at: std::time::SystemTime::UNIX_EPOCH
                    .checked_add(std::time::Duration::from_secs(
                        record.last_verified_at_unix_seconds,
                    ))
                    .map(system_time_to_rfc3339),
                status,
                is_current: current_profile_id.as_deref() == Some(record.profile_id.as_str()),
            }
        })
        .collect())
}

#[tauri::command]
pub(crate) fn list_trae_cn_accounts(
    state: tauri::State<AppState>,
) -> Result<Vec<TraeCnAccountWireDto>, String> {
    list_trae_cn_accounts_inner(&state)
}

/// 对融合版 TRAE 账号执行只读健康检测（P15-4，ADR-0035）：使用登录代次
/// 容器（`.fused`）的 access token 以 TraeCn 形态查询账号资料并校验身份，
/// 不签到、不读取额度、不打开新的 OAuth，也不触发续期（续期由融合版
/// 客户端自动完成）。
#[tauri::command]
pub(crate) async fn check_trae_cn_account_health(
    profile_ids: Option<Vec<String>>,
    state: tauri::State<'_, AppState>,
    app: tauri::AppHandle,
) -> Result<Vec<TraeCnHealthEntryDto>, String> {
    ensure_login_real_mode(&state)?;
    let selected = validate_product_profile_ids(profile_ids)?;
    // 账号主体 = Work 签到档案（16 账号）；登录代次容器由 inner 从
    // material_root 的父目录（storage_root）派生。
    let material_root = checkin_material_root(&state)?;
    run_blocking("trae_cn_health_join_failed", move || {
        check_trae_cn_account_health_inner(&material_root, &selected, &|event| {
            // 逐账号进度事件（P11-3）：AppHandle 借给 emit 闭包传进深层编排。
            let _ = app.emit(
                "health-check-progress",
                ProductBatchProgressEvent::new("trae_cn", event),
            );
        })
    })
    .await
}

/// 对融合版 TRAE 账号执行只读健康检测（P15-4，ADR-0035）：使用登录代次
/// 容器（`.fused`）的 access token 以 TraeCn 形态查询账号资料并校验身份，
/// 不签到、不读取额度、不触发续期（续期由融合版客户端自动完成）。
fn check_trae_cn_account_health_inner(
    material_root: &Path,
    selected: &[String],
    progress: &dyn Fn(BatchProgressEvent),
) -> Result<Vec<TraeCnHealthEntryDto>, String> {
    // 账号主体 = Work 签到档案；登录代次 = `.fused` 容器（双通道独立）。
    let registry = AccountRegistry::new(material_root);
    let records = registry
        .load()
        .map_err(|_| "trae_cn_registry_invalid".to_string())?;
    let client = trae_http_client();
    let now = unix_now_seconds();
    // 容器根由调用方注入（与 list 命令同源），此处以参数传入避免再依赖 AppState。
    let fused_root = material_root
        .parent()
        .map(|storage_root| storage_root.join("fused-login"))
        .ok_or_else(|| "trae_cn_storage_unavailable".to_string())?;
    let fused_store = FusedLoginStore::new(&fused_root);

    run_serial_batch(
        selected,
        &records,
        progress,
        |profile_id, snapshot, completed, total| {
            let Some(snapshot) = snapshot else {
                // 缺失档案：不触网即失败，进度事件与回执同步带 credential_missing。
                return Ok((
                    TraeCnHealthEntryDto {
                        profile_id: profile_id.to_string(),
                        screen_name: String::new(),
                        healthy: false,
                        error_code: Some("credential_missing".to_string()),
                    },
                    BatchProgressEvent::HealthCheck {
                        profile_id: profile_id.to_string(),
                        screen_name: String::new(),
                        completed,
                        total,
                        healthy: false,
                        error_code: Some("credential_missing".to_string()),
                    },
                ));
            };
            let Some(record) = registry
                .find(profile_id)
                .map_err(|_| "trae_cn_registry_invalid".to_string())?
            else {
                return Ok((
                    TraeCnHealthEntryDto {
                        profile_id: profile_id.to_string(),
                        screen_name: snapshot.screen_name.clone(),
                        healthy: false,
                        error_code: Some("credential_missing".to_string()),
                    },
                    BatchProgressEvent::HealthCheck {
                        profile_id: profile_id.to_string(),
                        screen_name: snapshot.screen_name.clone(),
                        completed,
                        total,
                        healthy: false,
                        error_code: Some("credential_missing".to_string()),
                    },
                ));
            };
            // 登录代次检测：容器读取 → 在线 GetUserInfo（TraeCn 形态）→ 身份匹配。
            // 容器无锁需求：续期由客户端完成，工具对容器只有回存（切号时）写入。
            let result = fused_store
                .load(profile_id, &record.account_id)
                .map_err(|_| "credential_missing".to_string())
                .and_then(|fused| {
                    if fused.access_token_expires_at_unix_seconds <= now {
                        return Err("credential_expired".to_string());
                    }
                    get_user_info_full_for_oauth_client(
                        &client,
                        &fused.access_token,
                        OAuthClient::TraeCn,
                    )
                    .map_err(|error| trae_cn_health_error_code(&error))
                    .and_then(|info| {
                        if info.user_id == record.account_id {
                            Ok(())
                        } else {
                            Err("auth_mismatch".to_string())
                        }
                    })
                });

            match result {
                Ok(()) => {
                    let mut verified = record.clone();
                    verified.last_verified_at_unix_seconds = now;
                    registry
                        .upsert(&verified)
                        .map_err(|_| "trae_cn_registry_invalid".to_string())?;
                    Ok((
                        TraeCnHealthEntryDto {
                            profile_id: profile_id.to_string(),
                            screen_name: record.screen_name.clone(),
                            healthy: true,
                            error_code: None,
                        },
                        BatchProgressEvent::HealthCheck {
                            profile_id: profile_id.to_string(),
                            screen_name: record.screen_name.clone(),
                            completed,
                            total,
                            healthy: true,
                            error_code: None,
                        },
                    ))
                }
                Err(error_code) => Ok((
                    TraeCnHealthEntryDto {
                        profile_id: profile_id.to_string(),
                        screen_name: record.screen_name.clone(),
                        healthy: false,
                        error_code: Some(error_code.clone()),
                    },
                    BatchProgressEvent::HealthCheck {
                        profile_id: profile_id.to_string(),
                        screen_name: record.screen_name.clone(),
                        completed,
                        total,
                        healthy: false,
                        error_code: Some(error_code),
                    },
                )),
            }
        },
    )
}

fn validate_product_profile_ids(profile_ids: Option<Vec<String>>) -> Result<Vec<String>, String> {
    let selected = profile_ids.unwrap_or_default();
    let mut unique = Vec::with_capacity(selected.len());
    for profile_id in selected {
        if profile_id.is_empty() || profile_id.len() > 256 {
            return Err("checkin_profile_invalid".to_string());
        }
        if !unique.contains(&profile_id) {
            unique.push(profile_id);
        }
    }
    if unique.is_empty() {
        return Err("checkin_profile_empty".to_string());
    }
    Ok(unique)
}

/// 验证目标档案和凭据后，登记一个有效的 Trae CN 账号选择意图。
/// 融合版切号目标校验（P15-4，ADR-0035）：档案 = Work 签到注册表；
/// 凭据 = `.fused` 登录代次容器（必须已铸造且 access 未过期）。
/// 指针根（state_root）与档案根（material_root）分离：选择意图与提交
/// 必须落在同一存储（P15-7 迁出旧池目录后为 product-state 根）。
fn begin_trae_cn_account_selection(
    material_root: &Path,
    fused_root: &Path,
    state_root: &Path,
    profile_id: &str,
) -> Result<u64, String> {
    if profile_id.is_empty() || profile_id.len() > 256 {
        return Err("trae_cn_account_not_found".to_string());
    }
    let record = AccountRegistry::new(material_root)
        .find(profile_id)
        .map_err(|_| "trae_cn_registry_invalid".to_string())?
        .ok_or_else(|| "trae_cn_account_not_found".to_string())?;
    FusedLoginStore::new(fused_root)
        .load(profile_id, &record.account_id)
        .map_err(|_| "trae_cn_credential_invalid".to_string())?;

    // 只有目标档案和登录代次均有效，才登记一次账号选择意图。
    ProductAccountStateStore::new(state_root)
        .begin_selection_intent()
        .map_err(|_| "trae_cn_account_state_failed".to_string())
}

/// 切换 Trae CN 当前账号：先验证档案与 DPAPI 凭据绑定，再原子更新当前指针。
/// 不向官方 Trae CN 目录写入 token、blob、设备身份或数据库。
#[tauri::command]
pub(crate) fn switch_trae_cn_account(
    profile_id: String,
    state: tauri::State<AppState>,
) -> Result<Vec<TraeCnAccountWireDto>, String> {
    ensure_login_real_mode(&state)?;
    let material_root = checkin_material_root(&state)?;
    let fused_root = fused_login_root(&state)?;
    let state_root = product_account_state_root(&state)?;
    let account_state = ProductAccountStateStore::new(&state_root);
    // 无效档案/凭据不会盖过正在完成的有效选择意图。
    // 目标校验：档案 = Work 签到注册表，凭据 = 登录代次容器。
    let selection_intent =
        begin_trae_cn_account_selection(&material_root, &fused_root, &state_root, &profile_id)?;
    if !account_state
        .commit_selection(&profile_id, selection_intent)
        .map_err(|_| "trae_cn_account_state_failed".to_string())?
    {
        return Err("trae_cn_account_selection_superseded".to_string());
    }
    list_trae_cn_accounts_inner(&state)
}

#[cfg(test)]
mod trae_cn_account_selection_tests {
    use super::*;

    #[test]
    fn invalid_switch_target_does_not_supersede_pending_oauth_selection() {
        let root = tempfile::tempdir().unwrap();
        let fused_root = tempfile::tempdir().unwrap();
        // P15-7：指针根与档案根分离；意图登记与提交落在同一 product-state 存储。
        let state_root = tempfile::tempdir().unwrap();
        let account_state = ProductAccountStateStore::new(state_root.path());
        let login_intent = account_state.begin_selection_intent().unwrap();

        // P15-4：目标校验 = Work 签到注册表 + 登录代次容器；缺失即拒绝。
        assert_eq!(
            begin_trae_cn_account_selection(
                root.path(),
                fused_root.path(),
                state_root.path(),
                "profile-missing"
            )
            .unwrap_err(),
            "trae_cn_account_not_found"
        );
        assert!(account_state
            .commit_selection("profile-oauth", login_intent)
            .unwrap());
        assert_eq!(
            account_state.load().unwrap(),
            Some("profile-oauth".to_string())
        );
    }
}

/// 单账号积分刷新回执（refresh_checkin_credits 逐账号结果）。
#[derive(Clone, Serialize)]
pub(crate) struct CreditsRefreshEntryDto {
    profile_id: String,
    screen_name: String,
    /// 最新积分；失败时为 None（保留旧缓存值）。
    credits: Option<i64>,
    /// 最新“今日已签”状态；失败时为 None。
    checked_in: Option<bool>,
    /// 真实模型额度剩余（`ide_user_ent_usage`）；失败时为 None（保留旧缓存值）。
    usage_remaining_credits: Option<f64>,
    /// 失败原因码（network_error / business_XXXX / credential_*）；成功为 None。
    error_code: Option<String>,
}

/// 手动刷新登录凭据逐账号回执；不携带 access/refresh 等敏感内容。
#[derive(Clone, Serialize)]
pub(crate) struct CredentialRefreshEntryDto {
    profile_id: String,
    screen_name: String,
    /// 是否已完成同设备凭据换发并安全写回。
    refreshed: bool,
    /// 失败原因码；成功为 None。
    error_code: Option<String>,
}

/// 只读刷新积分：逐账号调 status + ide_user_ent_usage（不 claim、不消耗签到资格），
/// 成功快照分别写回积分/额度缓存。串行执行、账号间无间隔（仅查询，实时完成；
/// 签到 claim 才需要 3-8 秒随机间隔防风控）。单账号失败不影响其余账号；
/// 失败账号保留旧缓存并返回原因码。
#[tauri::command]
pub(crate) async fn refresh_checkin_credits(
    profile_ids: Vec<String>,
    state: tauri::State<'_, AppState>,
    app: tauri::AppHandle,
) -> Result<Vec<CreditsRefreshEntryDto>, String> {
    let mut selected = Vec::new();
    for profile_id in &profile_ids {
        if profile_id.is_empty() || profile_id.len() > 256 {
            return Err("checkin_profile_invalid".to_string());
        }
        if !selected.contains(profile_id) {
            selected.push(profile_id.clone());
        }
    }
    if selected.is_empty() {
        return Err("checkin_profile_empty".to_string());
    }
    let material_root = checkin_material_root(&state)?;
    let runtime_mode = state.runtime_mode;
    run_blocking("checkin_refresh_join_failed", move || {
        refresh_checkin_credits_inner(&material_root, &selected, runtime_mode, &|event| {
            // 逐账号进度事件（P11-3）：AppHandle 借给 emit 闭包传进深层编排。
            let _ = app.emit(
                "credits-refresh-progress",
                ProductBatchProgressEvent::new("work_cn", event),
            );
        })
    })
    .await
}

fn refresh_checkin_credits_inner(
    material_root: &std::path::Path,
    selected: &[String],
    runtime_mode: RuntimeMode,
    progress: &dyn Fn(BatchProgressEvent),
) -> Result<Vec<CreditsRefreshEntryDto>, String> {
    let registry = AccountRegistry::new(material_root);
    let records = registry
        .load()
        .map_err(|_| "checkin_registry_invalid".to_string())?;
    let store = CheckinCredentialStore::new(material_root);
    // 与签到一致的中断写回阻断：存在未收口续期时拒绝刷新，先恢复。
    if store.has_pending_renewal().unwrap_or(false) {
        return Err("manual_recovery_required".to_string());
    }

    let entries = run_serial_batch(
        selected,
        &records,
        progress,
        |profile_id, record, completed, total| {
            let Some(snapshot_record) = record else {
                return Ok((
                    CreditsRefreshEntryDto {
                        profile_id: profile_id.to_string(),
                        screen_name: String::new(),
                        credits: None,
                        checked_in: None,
                        usage_remaining_credits: None,
                        error_code: Some("credential_missing".to_string()),
                    },
                    BatchProgressEvent::CreditsRefresh {
                        profile_id: profile_id.to_string(),
                        screen_name: String::new(),
                        completed,
                        total,
                        ok: false,
                        credits: None,
                        checked_in: None,
                        usage_remaining_credits: None,
                        error_code: Some("credential_missing".to_string()),
                    },
                ));
            };
            let profile_lease = match store.lock_profile(profile_id) {
                Ok(lease) => lease,
                Err(error) => {
                    let error_code = credential_maintenance_error_code(&error).to_string();
                    return Ok((
                        CreditsRefreshEntryDto {
                            profile_id: profile_id.to_string(),
                            screen_name: snapshot_record.screen_name.clone(),
                            credits: None,
                            checked_in: None,
                            usage_remaining_credits: None,
                            error_code: Some(error_code.clone()),
                        },
                        BatchProgressEvent::CreditsRefresh {
                            profile_id: profile_id.to_string(),
                            screen_name: snapshot_record.screen_name.clone(),
                            completed,
                            total,
                            ok: false,
                            credits: None,
                            checked_in: None,
                            usage_remaining_credits: None,
                            error_code: Some(error_code),
                        },
                    ));
                }
            };
            // 等锁期间档案可能已删除或重登；后续查询与写回只用锁内档案。
            let Some(record) = registry
                .find(profile_id)
                .map_err(|_| "checkin_registry_invalid".to_string())?
            else {
                return Ok((
                    CreditsRefreshEntryDto {
                        profile_id: profile_id.to_string(),
                        screen_name: snapshot_record.screen_name.clone(),
                        credits: None,
                        checked_in: None,
                        usage_remaining_credits: None,
                        error_code: Some("credential_missing".to_string()),
                    },
                    BatchProgressEvent::CreditsRefresh {
                        profile_id: profile_id.to_string(),
                        screen_name: snapshot_record.screen_name.clone(),
                        completed,
                        total,
                        ok: false,
                        credits: None,
                        checked_in: None,
                        usage_remaining_credits: None,
                        error_code: Some("credential_missing".to_string()),
                    },
                ));
            };
            let binding = CheckinProfileBinding::from(&record);
            let refresh_result = match runtime_mode {
                RuntimeMode::Fixture => Ok(()),
                RuntimeMode::RealReadPreview => {
                    let renewal = RealCheckinRenewalService::new(&store);
                    let now = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|duration| duration.as_secs())
                        .unwrap_or(0);
                    renewal
                        .renew_if_needed_with_lease(&binding, now, &profile_lease)
                        .map(|_| ())
                }
            };
            let transport: Box<dyn CheckinTransport> = match runtime_mode {
                // for_profiles 接收 &[String]：闭包内 profile_id 已是 &str，补拥有值。
                RuntimeMode::Fixture => Box::new(FixtureCheckinTransport::for_profiles(&[
                    profile_id.to_string(),
                ])),
                RuntimeMode::RealReadPreview => {
                    let mut bindings = std::collections::BTreeMap::new();
                    bindings.insert(record.profile_id.clone(), binding.clone());
                    Box::new(RealCheckinTransport::new(&store, bindings))
                }
            };
            let status = match refresh_result {
                Err(_) => Err("credential_refresh_failed".to_string()),
                Ok(()) => transport
                    .status(profile_id)
                    .map_err(|error| checkin_transport_error_code(&error)),
            };
            match status {
                Ok(snapshot) => {
                    checkin_overview::update_credits_from_snapshots(
                        material_root,
                        &[(profile_id.to_string(), snapshot.clone())],
                    );
                    // U-1 脱敏手机号无感补采：仅真实模式且档案缺失时调用一次
                    // GetUserInfo（只读接口，失败静默，下次刷新再试）。
                    // 健康检测复用本命令路径，存量账号由此补全。
                    if record.masked_mobile.is_empty()
                        && matches!(runtime_mode, RuntimeMode::RealReadPreview)
                    {
                        if let Ok(bundle) = store.load(&binding) {
                            let client = trae_http_client();
                            if let Ok(user_info) = get_user_info(&client, &bundle.access_token) {
                                if !user_info.masked_mobile.is_empty() {
                                    let _ = registry.backfill_masked_mobile(
                                        profile_id,
                                        &user_info.masked_mobile,
                                    );
                                }
                            }
                        }
                    }
                    // status 成功后追加真实额度查询：独立失败处理，额度失败不拖垮积分回执。
                    let usage = transport
                        .entitlement_usage(profile_id)
                        .map_err(|error| checkin_transport_error_code(&error));
                    let usage_remaining_credits = match usage {
                        Ok(usage_snapshot) => {
                            let remaining = usage_snapshot.remaining_credits;
                            checkin_overview::update_usage_cache(
                                material_root,
                                &[(profile_id.to_string(), usage_snapshot)],
                            );
                            Some(remaining)
                        }
                        Err(_) => None,
                    };
                    Ok((
                        CreditsRefreshEntryDto {
                            profile_id: profile_id.to_string(),
                            screen_name: record.screen_name.clone(),
                            credits: snapshot.credits,
                            checked_in: Some(snapshot.checked_in),
                            usage_remaining_credits,
                            error_code: None,
                        },
                        BatchProgressEvent::CreditsRefresh {
                            profile_id: profile_id.to_string(),
                            screen_name: record.screen_name.clone(),
                            completed,
                            total,
                            ok: true,
                            credits: snapshot.credits,
                            checked_in: Some(snapshot.checked_in),
                            usage_remaining_credits,
                            error_code: None,
                        },
                    ))
                }
                Err(error_code) => {
                    checkin_overview::update_refresh_failures(
                        material_root,
                        &[(profile_id.to_string(), error_code.clone())],
                    );
                    Ok((
                        CreditsRefreshEntryDto {
                            profile_id: profile_id.to_string(),
                            screen_name: record.screen_name.clone(),
                            credits: None,
                            checked_in: None,
                            usage_remaining_credits: None,
                            error_code: Some(error_code.clone()),
                        },
                        BatchProgressEvent::CreditsRefresh {
                            profile_id: profile_id.to_string(),
                            screen_name: record.screen_name.clone(),
                            completed,
                            total,
                            ok: false,
                            credits: None,
                            checked_in: None,
                            usage_remaining_credits: None,
                            error_code: Some(error_code),
                        },
                    ))
                }
            }
        },
    )?;
    Ok(entries)
}

/// 手动刷新登录凭据：只走凭据换发链路，不签到、不查询额度、不打开用户 OAuth。
///
/// access 仍有效时由 `renew_now` 使用同设备 AuthCode 换发；access 已过期时
/// 仅允许当前凭据代次的一次 refresh 救援。账号逐个处理，单个失败不会阻塞其余账号。
#[tauri::command]
pub(crate) async fn refresh_checkin_credentials(
    profile_ids: Vec<String>,
    state: tauri::State<'_, AppState>,
    app: tauri::AppHandle,
) -> Result<Vec<CredentialRefreshEntryDto>, String> {
    ensure_checkin_http_enabled(&state)?;
    let mut selected = Vec::new();
    for profile_id in profile_ids {
        if profile_id.is_empty() || profile_id.len() > 256 {
            return Err("checkin_profile_invalid".to_string());
        }
        if !selected.contains(&profile_id) {
            selected.push(profile_id);
        }
    }
    if selected.is_empty() {
        return Err("checkin_profile_empty".to_string());
    }
    let material_root = checkin_material_root(&state)?;
    let execution_lock = Arc::clone(&state.checkin_execution_lock);
    run_blocking("credential_refresh_join_failed", move || {
        // 凭据换发与签到/设备操作共享互斥，避免并发写入同一凭据包。
        let _guard = execution_lock
            .try_lock()
            .map_err(|_| "credential_refresh_busy".to_string())?;
        refresh_checkin_credentials_inner(&material_root, &selected, &|event| {
            // 逐账号进度事件（P11-3）：AppHandle 借给 emit 闭包传进深层编排。
            let _ = app.emit(
                "credential-refresh-progress",
                ProductBatchProgressEvent::new("work_cn", event),
            );
        })
    })
    .await
}

fn refresh_checkin_credentials_inner(
    material_root: &std::path::Path,
    selected: &[String],
    progress: &dyn Fn(BatchProgressEvent),
) -> Result<Vec<CredentialRefreshEntryDto>, String> {
    refresh_product_credentials_inner(material_root, selected, OAuthClient::Solo, progress)
}

fn refresh_product_credentials_inner(
    material_root: &std::path::Path,
    selected: &[String],
    oauth_client: OAuthClient,
    progress: &dyn Fn(BatchProgressEvent),
) -> Result<Vec<CredentialRefreshEntryDto>, String> {
    let store = CheckinCredentialStore::new(material_root);
    // 有未收口写回时整批停止，避免把现场继续推进到更难恢复的状态。
    if store
        .has_pending_renewal()
        .map_err(|_| "manual_recovery_required".to_string())?
    {
        return Err("manual_recovery_required".to_string());
    }

    let now = unix_now_seconds();
    let renewal = RealCheckinRenewalService::new_for_oauth_client(&store, oauth_client);
    refresh_checkin_credentials_inner_with(
        material_root,
        selected,
        now,
        &|binding, timestamp, lease| {
            renewal
                .renew_now_with_lease(binding, timestamp, lease)
                .map(|_| ())
        },
        progress,
    )
}

/// 手动刷新命令的结果收集边界。
///
/// 生产路径传入真实续期服务；测试可以注入确定性闭包，验证选中账号去重、
/// 单账号失败后继续处理、逐账号回执和失败证据写回，而无需触碰 OAuth 网络。
fn refresh_checkin_credentials_inner_with(
    material_root: &std::path::Path,
    selected: &[String],
    now: u64,
    renew: &dyn Fn(
        &CheckinProfileBinding,
        u64,
        &traesync_infrastructure::CheckinProfileOperationLease,
    ) -> Result<(), CheckinCredentialError>,
    progress: &dyn Fn(BatchProgressEvent),
) -> Result<Vec<CredentialRefreshEntryDto>, String> {
    let registry = AccountRegistry::new(material_root);
    let records = registry
        .load()
        .map_err(|_| "checkin_registry_invalid".to_string())?;
    let store = CheckinCredentialStore::new(material_root);
    let maintenance_state = CredentialMaintenanceStateStore::new(material_root);

    let entries = run_serial_batch(
        selected,
        &records,
        progress,
        |profile_id, record, completed, total| {
            let Some(snapshot) = record else {
                return Ok((
                    CredentialRefreshEntryDto {
                        profile_id: profile_id.to_string(),
                        screen_name: String::new(),
                        refreshed: false,
                        error_code: Some("credential_missing".to_string()),
                    },
                    BatchProgressEvent::CredentialRefresh {
                        profile_id: profile_id.to_string(),
                        screen_name: String::new(),
                        completed,
                        total,
                        refreshed: false,
                        error_code: Some("credential_missing".to_string()),
                    },
                ));
            };
            let profile_lease = match store.lock_profile(profile_id) {
                Ok(lease) => lease,
                Err(error) => {
                    let error_code = credential_maintenance_error_code(&error).to_string();
                    return Ok((
                        CredentialRefreshEntryDto {
                            profile_id: profile_id.to_string(),
                            screen_name: snapshot.screen_name.clone(),
                            refreshed: false,
                            error_code: Some(error_code.clone()),
                        },
                        BatchProgressEvent::CredentialRefresh {
                            profile_id: profile_id.to_string(),
                            screen_name: snapshot.screen_name.clone(),
                            completed,
                            total,
                            refreshed: false,
                            error_code: Some(error_code),
                        },
                    ));
                }
            };
            // 等锁期间档案可能已删除或重登，使用锁内最新绑定执行续期。
            let Some(record) = registry
                .find(profile_id)
                .map_err(|_| "checkin_registry_invalid".to_string())?
            else {
                return Ok((
                    CredentialRefreshEntryDto {
                        profile_id: profile_id.to_string(),
                        screen_name: snapshot.screen_name.clone(),
                        refreshed: false,
                        error_code: Some("credential_missing".to_string()),
                    },
                    BatchProgressEvent::CredentialRefresh {
                        profile_id: profile_id.to_string(),
                        screen_name: snapshot.screen_name.clone(),
                        completed,
                        total,
                        refreshed: false,
                        error_code: Some("credential_missing".to_string()),
                    },
                ));
            };
            let binding = CheckinProfileBinding::from(&record);
            match renew(&binding, now, &profile_lease) {
                Ok(_) => {
                    // 手动成功也清除自动维护的旧退避；手动入口不读取/拦截退避，
                    // 只把确认成功的结果反馈给下一次自动维护。
                    let _ = maintenance_state.record_success(profile_id, now);
                    checkin_overview::clear_refresh_failures(
                        material_root,
                        &[profile_id.to_string()],
                    );
                    Ok((
                        CredentialRefreshEntryDto {
                            profile_id: profile_id.to_string(),
                            screen_name: record.screen_name.clone(),
                            refreshed: true,
                            error_code: None,
                        },
                        BatchProgressEvent::CredentialRefresh {
                            profile_id: profile_id.to_string(),
                            screen_name: record.screen_name.clone(),
                            completed,
                            total,
                            refreshed: true,
                            error_code: None,
                        },
                    ))
                }
                Err(error) => {
                    let error_code = credential_maintenance_error_code(&error).to_string();
                    checkin_overview::update_refresh_failures(
                        material_root,
                        &[(profile_id.to_string(), error_code.clone())],
                    );
                    Ok((
                        CredentialRefreshEntryDto {
                            profile_id: profile_id.to_string(),
                            screen_name: record.screen_name.clone(),
                            refreshed: false,
                            error_code: Some(error_code.clone()),
                        },
                        BatchProgressEvent::CredentialRefresh {
                            profile_id: profile_id.to_string(),
                            screen_name: record.screen_name.clone(),
                            completed,
                            total,
                            refreshed: false,
                            error_code: Some(error_code),
                        },
                    ))
                }
            }
        },
    )?;
    Ok(entries)
}

/// P11-3 批量凭据刷新 / 健康检测的逐账号进度事件契约（P12-R2b 起载荷统一为
/// BatchProgressEvent）：载荷非敏感白名单、事件与账号一一对应、
/// completed 递增且失败不中断批次。
#[cfg(test)]
mod progress_event_contract_tests {
    use super::*;

    fn record(profile_id: &str, account_id: &str, screen_name: &str) -> AccountRecord {
        AccountRecord {
            profile_id: profile_id.to_string(),
            account_id: account_id.to_string(),
            screen_name: screen_name.to_string(),
            avatar_url: String::new(),
            device_id: "1234567890123456".to_string(),
            device_public_key: "public-key".to_string(),
            display_name: None,
            masked_mobile: String::new(),
            created_at_unix_seconds: 0,
            last_verified_at_unix_seconds: 0,
            device_created_at_unix_seconds: 0,
            auto_checkin_enabled: true,
            archived: false,
        }
    }

    #[test]
    fn manual_refresh_emits_progress_per_account_and_continues() {
        let root = tempfile::tempdir().unwrap();
        let registry = AccountRegistry::new(root.path());
        registry
            .upsert(&record("profile-fail", "account-fail", "失败账号"))
            .unwrap();
        registry
            .upsert(&record("profile-ok", "account-ok", "成功账号"))
            .unwrap();
        let selected = vec![
            "profile-fail".to_string(),
            "profile-ok".to_string(),
            "profile-missing".to_string(),
        ];
        let events = std::cell::RefCell::new(Vec::new());
        let results = refresh_checkin_credentials_inner_with(
            root.path(),
            &selected,
            1_800_000_000,
            &|binding, _, _lease| {
                if binding.profile_id == "profile-fail" {
                    Err(CheckinCredentialError::CredentialRefreshFailed)
                } else {
                    Ok(())
                }
            },
            &|event| events.borrow_mut().push(event),
        )
        .unwrap();
        let events = events.into_inner();

        // 每个账号完成（含失败与缺失）各推送一次进度，失败不中断批次。
        assert_eq!(events.len(), 3);
        let BatchProgressEvent::CredentialRefresh {
            profile_id,
            screen_name,
            completed,
            total,
            refreshed,
            error_code,
        } = &events[0]
        else {
            panic!("应为凭据刷新链事件");
        };
        assert_eq!(profile_id, "profile-fail");
        assert_eq!(screen_name, "失败账号");
        assert_eq!(*completed, 1);
        assert_eq!(*total, 3);
        assert!(!refreshed);
        assert_eq!(error_code.as_deref(), Some("credential_refresh_failed"));
        let BatchProgressEvent::CredentialRefresh {
            profile_id,
            completed,
            refreshed,
            error_code,
            ..
        } = &events[1]
        else {
            panic!("应为凭据刷新链事件");
        };
        assert_eq!(profile_id, "profile-ok");
        assert_eq!(*completed, 2);
        assert!(*refreshed);
        assert!(error_code.is_none());
        // 缺失档案的进度事件 screen_name 为空（前端回退展示 profile_id）。
        let BatchProgressEvent::CredentialRefresh {
            profile_id,
            screen_name,
            completed,
            error_code,
            ..
        } = &events[2]
        else {
            panic!("应为凭据刷新链事件");
        };
        assert_eq!(profile_id, "profile-missing");
        assert_eq!(screen_name, "");
        assert_eq!(*completed, 3);
        assert_eq!(error_code.as_deref(), Some("credential_missing"));
        // 整批回执与进度事件一一对应，既有失败语义不回归。
        assert_eq!(results.len(), 3);
        assert!(!results[0].refreshed);
        assert!(results[1].refreshed);
    }

    #[cfg(windows)]
    #[test]
    fn trae_cn_health_emits_progress_per_account() {
        let root = tempfile::tempdir().unwrap();
        AccountRegistry::new(root.path())
            .upsert(&record("profile-h", "account-h", "健康号"))
            .unwrap();
        let events = std::cell::RefCell::new(Vec::new());
        let results =
            check_trae_cn_account_health_inner(root.path(), &["profile-h".to_string()], &|event| {
                events.borrow_mut().push(event)
            })
            .unwrap();
        let events = events.into_inner();
        // P15-4 融合版语义：登录代次容器不存在（未铸造）→ credential_missing：
        // 不触网即失败，进度事件与整批回执同步如实带错误码。
        assert_eq!(events.len(), 1);
        let BatchProgressEvent::HealthCheck {
            profile_id,
            screen_name,
            completed,
            total,
            healthy,
            error_code,
        } = &events[0]
        else {
            panic!("应为健康检测链事件");
        };
        assert_eq!(profile_id, "profile-h");
        assert_eq!(screen_name, "健康号");
        assert_eq!(*completed, 1);
        assert_eq!(*total, 1);
        assert!(!healthy);
        assert_eq!(error_code.as_deref(), Some("credential_missing"));
        assert_eq!(results.len(), 1);
        assert!(!results[0].healthy);
    }

    #[test]
    fn credits_refresh_emits_progress_per_account_and_continues() {
        let root = tempfile::tempdir().unwrap();
        AccountRegistry::new(root.path())
            .upsert(&record("profile-ok", "account-ok", "成功账号"))
            .unwrap();
        AccountRegistry::new(root.path())
            .upsert(&record("profile-later", "account-later", "后续账号"))
            .unwrap();
        let selected = vec![
            "profile-ok".to_string(),
            "profile-missing".to_string(),
            "profile-later".to_string(),
        ];
        let events = std::cell::RefCell::new(Vec::new());
        let results =
            refresh_checkin_credits_inner(root.path(), &selected, RuntimeMode::Fixture, &|event| {
                events.borrow_mut().push(event)
            })
            .unwrap();
        let events = events.into_inner();

        // 每个账号完成（含失败与缺失）各推送一次进度，失败不中断批次。
        assert_eq!(events.len(), 3);
        // 成功账号：fixture 快照（credits=0、未签）；额度失败不拖垮积分回执。
        let BatchProgressEvent::CreditsRefresh {
            profile_id,
            screen_name,
            completed,
            total,
            ok,
            credits,
            checked_in,
            error_code,
            ..
        } = &events[0]
        else {
            panic!("应为积分刷新链事件");
        };
        assert_eq!(profile_id, "profile-ok");
        assert_eq!(screen_name, "成功账号");
        assert_eq!(*completed, 1);
        assert_eq!(*total, 3);
        assert!(*ok);
        assert_eq!(credits, &Some(0));
        assert_eq!(checked_in, &Some(false));
        assert!(error_code.is_none());
        // 缺失档案：事件如实带 credential_missing，批次继续。
        let BatchProgressEvent::CreditsRefresh {
            profile_id,
            completed,
            ok,
            error_code,
            ..
        } = &events[1]
        else {
            panic!("应为积分刷新链事件");
        };
        assert_eq!(profile_id, "profile-missing");
        assert_eq!(*completed, 2);
        assert!(!ok);
        assert_eq!(error_code.as_deref(), Some("credential_missing"));
        let BatchProgressEvent::CreditsRefresh {
            profile_id,
            completed,
            ok,
            ..
        } = &events[2]
        else {
            panic!("应为积分刷新链事件");
        };
        assert_eq!(profile_id, "profile-later");
        assert_eq!(*completed, 3);
        assert!(*ok);
        // 整批回执与进度事件一一对应。
        assert_eq!(results.len(), 3);
        assert!(results[0].error_code.is_none());
        assert_eq!(results[1].error_code.as_deref(), Some("credential_missing"));
        assert!(results[2].error_code.is_none());
    }
}

#[cfg(test)]
mod credential_refresh_contract_tests {
    use super::*;

    fn record(profile_id: &str, account_id: &str, screen_name: &str) -> AccountRecord {
        AccountRecord {
            profile_id: profile_id.to_string(),
            account_id: account_id.to_string(),
            screen_name: screen_name.to_string(),
            avatar_url: String::new(),
            device_id: "1234567890123456".to_string(),
            device_public_key: "public-key".to_string(),
            display_name: None,
            masked_mobile: String::new(),
            created_at_unix_seconds: 0,
            last_verified_at_unix_seconds: 0,
            device_created_at_unix_seconds: 0,
            auto_checkin_enabled: true,
            archived: false,
        }
    }

    #[test]
    fn manual_refresh_returns_per_account_results_and_continues_after_failure() {
        let root = tempfile::tempdir().unwrap();
        let registry = AccountRegistry::new(root.path());
        registry
            .upsert(&record("profile-fail", "account-fail", "失败账号"))
            .unwrap();
        registry
            .upsert(&record("profile-ok", "account-ok", "成功账号"))
            .unwrap();
        let calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let selected = vec![
            "profile-fail".to_string(),
            "profile-ok".to_string(),
            "profile-missing".to_string(),
        ];

        let results = refresh_checkin_credentials_inner_with(
            root.path(),
            &selected,
            1_800_000_000,
            &|binding, _, _lease| {
                calls.lock().unwrap().push(binding.profile_id.clone());
                if binding.profile_id == "profile-fail" {
                    Err(CheckinCredentialError::CredentialRefreshFailed)
                } else {
                    Ok(())
                }
            },
            // 既有失败语义回归：进度事件由 progress_event_contract_tests 覆盖。
            &|_| {},
        )
        .unwrap();

        // 一个账号失败不能中断后续账号；缺失账号也必须有明确回执。
        assert_eq!(
            *calls.lock().unwrap(),
            vec!["profile-fail".to_string(), "profile-ok".to_string()]
        );
        assert_eq!(results.len(), 3);
        assert!(!results[0].refreshed);
        assert_eq!(
            results[0].error_code.as_deref(),
            Some("credential_refresh_failed")
        );
        assert!(results[1].refreshed);
        assert_eq!(results[1].screen_name, "成功账号");
        assert!(!results[2].refreshed);
        assert_eq!(results[2].error_code.as_deref(), Some("credential_missing"));
    }
}

/// 删除账号：注册表档案 + DPAPI 凭据包 + 积分缓存条目三件套清理。
/// 幂等性由前端二次确认保障；档案不存在返回错误（不静默成功）。
#[tauri::command]
pub(crate) fn remove_checkin_account(
    profile_id: String,
    state: tauri::State<AppState>,
) -> Result<(), String> {
    if profile_id.is_empty() || profile_id.len() > 256 {
        return Err("checkin_profile_invalid".to_string());
    }
    let material_root = checkin_material_root(&state)?;
    let store = CheckinCredentialStore::new(&material_root);
    let profile_lease = store
        .lock_profile(&profile_id)
        .map_err(|_| "checkin_storage_unavailable".to_string())?;
    let registry = AccountRegistry::new(&material_root);
    let removed = registry
        .remove(&profile_id)
        .map_err(|_| "checkin_registry_invalid".to_string())?;
    if !removed {
        return Err("checkin_profile_invalid".to_string());
    }
    store
        .remove_with_lease(&profile_lease, &profile_id)
        .map_err(|_| "checkin_storage_unavailable".to_string())?;
    checkin_overview::remove_credits_cache_entry(&material_root, &profile_id);
    Ok(())
}

/// 手动重铸账号签到设备：走网络完整流程（GetPCAuthCode → ExchangeToken →
/// 凭据写回），不执行签到。用于自动重铸失败后的手动兜底
/// （适用场景：账号设备连续多日 9074/9095，用户主动换新）。
/// 成功返回新设备 ID，前端刷新总览即可看到新设备尾号。
#[tauri::command]
pub(crate) fn reset_checkin_device(
    profile_id: String,
    state: tauri::State<AppState>,
) -> Result<String, String> {
    if profile_id.is_empty() || profile_id.len() > 256 {
        return Err("checkin_profile_invalid".to_string());
    }
    let material_root = checkin_material_root(&state)?;
    let service = DeviceRemintService::new(&material_root);
    service
        .remint(&profile_id)
        .map(|device_id| device_id)
        .map_err(|error: RemintError| match error {
            RemintError::ProfileNotFound => "checkin_profile_invalid".to_string(),
            other => format!("remint_failed: {other}"),
        })
}

/// 设置/清除账号本地备注名（空串或 None = 清除，回退服务端名）。
/// U-1 数据层：解决服务端 ScreenName（如「用户A」）辨识度差的问题。
#[tauri::command]
pub(crate) fn set_account_display_name(
    profile_id: String,
    display_name: Option<String>,
    state: tauri::State<AppState>,
) -> Result<(), String> {
    if profile_id.is_empty() || profile_id.len() > 256 {
        return Err("checkin_profile_invalid".to_string());
    }
    // trim 后归一化：空别名 = 清除；超长拒绝（注册表 validate 同口径）。
    let alias = display_name.as_deref().map(str::trim);
    if let Some(name) = alias {
        if name.len() > 256 {
            return Err("display_name_invalid".to_string());
        }
    }
    let material_root = checkin_material_root(&state)?;
    let store = CheckinCredentialStore::new(&material_root);
    let _profile_lease = store
        .lock_profile(&profile_id)
        .map_err(|_| "checkin_storage_unavailable".to_string())?;
    let registry = AccountRegistry::new(&material_root);
    registry
        .set_display_name(&profile_id, alias)
        .map_err(|_| "checkin_registry_invalid".to_string())?;
    Ok(())
}

/// G11 手机号补录：写入/清除账号完整手机号（存凭据包，与令牌同级 DPAPI 加密）。
/// 三层校验的前两层在本命令：①大陆手机号格式（11 位，1 开头第二位 3-9）
/// ②与服务端脱敏号首尾比对（脱敏号前缀+后缀必须完全匹配，防串号录错账号）；
/// 第三层查重提示由前端基于总览数据判断后向用户确认。
/// mobile 为 None/空白 = 清除补录（展示回退脱敏号）。
#[tauri::command]
pub(crate) fn set_account_mobile(
    profile_id: String,
    mobile: Option<String>,
    state: tauri::State<AppState>,
) -> Result<(), String> {
    if profile_id.is_empty() || profile_id.len() > 256 {
        return Err("checkin_profile_invalid".to_string());
    }
    let trimmed = mobile.as_deref().map(str::trim).unwrap_or("");
    let material_root = checkin_material_root(&state)?;
    let store = CheckinCredentialStore::new(&material_root);
    let profile_lease = store
        .lock_profile(&profile_id)
        .map_err(|_| "checkin_storage_unavailable".to_string())?;
    let registry = AccountRegistry::new(&material_root);
    let record = registry
        .load()
        .map_err(|_| "checkin_registry_invalid".to_string())?
        .into_iter()
        .find(|record| record.profile_id == profile_id)
        .ok_or_else(|| "checkin_profile_invalid".to_string())?;

    // 清除路径：空输入 = 回退脱敏号展示（输入错了的后悔药）。
    if trimmed.is_empty() {
        let binding = CheckinProfileBinding::from(&record);
        store
            .set_mobile_full_with_lease(&profile_lease, &binding, None)
            .map_err(|_| "mobile_save_failed".to_string())?;
        return Ok(());
    }

    // 校验一：大陆手机号格式（1[3-9] 开头共 11 位数字）。
    if !is_mainland_mobile(trimmed) {
        return Err("mobile_format_invalid".to_string());
    }
    // 校验二：与服务端脱敏号首尾比对（如 "138****0000" → 前 3 后 4）。
    // 脱敏号尚未采集（旧账号未刷新额度）时无基准，跳过比对只走格式校验。
    if let Some((prefix, suffix)) = masked_mobile_parts(&record.masked_mobile) {
        let mismatch = !trimmed.starts_with(prefix.as_str()) || !trimmed.ends_with(suffix.as_str());
        if mismatch {
            return Err("mobile_masked_mismatch".to_string());
        }
    }

    let binding = CheckinProfileBinding::from(&record);
    store
        .set_mobile_full_with_lease(&profile_lease, &binding, Some(trimmed))
        .map_err(|_| "mobile_save_failed".to_string())?;
    Ok(())
}

/// 大陆手机号格式：1 开头、第二位 3-9、共 11 位数字。
fn is_mainland_mobile(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() == 11
        && bytes[0] == b'1'
        && (b'3'..=b'9').contains(&bytes[1])
        && bytes.iter().all(|byte| byte.is_ascii_digit())
}

/// 解析脱敏手机号的首尾明文段（"138****0000" → ("138", "0000")）。
/// 非预期形态（空串、无星号、首尾非数字）返回 None，调用方跳过比对。
fn masked_mobile_parts(masked: &str) -> Option<(String, String)> {
    let star = masked.find('*')?;
    let (prefix, rest) = masked.split_at(star);
    let suffix = rest.trim_start_matches('*');
    if prefix.is_empty() || suffix.is_empty() {
        return None;
    }
    if !prefix.bytes().all(|byte| byte.is_ascii_digit())
        || !suffix.bytes().all(|byte| byte.is_ascii_digit())
    {
        return None;
    }
    Some((prefix.to_string(), suffix.to_string()))
}

// ============================================================================
// P8-5 G18：主库自检四级判定。判定先保持纯函数，真实文件读取只负责组装输入，
// 这样“健康 / 可自愈 / 需人工”不会被某一条读取路径悄悄改变。
// ============================================================================
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MasterSelfCheckLevel {
    Healthy,
    SelfHealable,
    NeedsManual,
}

impl MasterSelfCheckLevel {
    fn as_str(self) -> &'static str {
        match self {
            Self::Healthy => "healthy",
            Self::SelfHealable => "self_healable",
            Self::NeedsManual => "needs_manual",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MasterSelfCheckStatus {
    Passed,
    Attention,
    Failed,
    Blocked,
}

impl MasterSelfCheckStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Passed => "passed",
            Self::Attention => "attention",
            Self::Failed => "failed",
            Self::Blocked => "blocked",
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct MasterSelfCheckInput {
    storage_exists: bool,
    storage_readable: bool,
    blob_present: bool,
    blob_decryptable: bool,
    account_registered: bool,
    cache_consistent: bool,
    instance_process_state: trae_instance_module::ProductProcessState,
    donor_available: bool,
    ledger_valid: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct MasterSelfCheckAssessment {
    level: MasterSelfCheckLevel,
    read_status: MasterSelfCheckStatus,
    consistency_status: MasterSelfCheckStatus,
    switchability_status: MasterSelfCheckStatus,
    deep_status: MasterSelfCheckStatus,
}

fn assess_master_self_check(input: MasterSelfCheckInput) -> MasterSelfCheckAssessment {
    let read_status = if !input.storage_exists {
        // 尚未登录不是故障，仍需让用户知道主库目前没有登录数据。
        MasterSelfCheckStatus::Attention
    } else if !input.storage_readable || !input.blob_decryptable || !input.account_registered {
        MasterSelfCheckStatus::Failed
    } else if !input.blob_present {
        MasterSelfCheckStatus::Attention
    } else {
        MasterSelfCheckStatus::Passed
    };
    let consistency_status = if input.cache_consistent {
        MasterSelfCheckStatus::Passed
    } else if input.blob_decryptable && input.account_registered {
        MasterSelfCheckStatus::Attention
    } else {
        MasterSelfCheckStatus::Blocked
    };
    let switchability_status =
        if input.instance_process_state != trae_instance_module::ProductProcessState::NotRunning {
            MasterSelfCheckStatus::Blocked
        } else if input.donor_available {
            MasterSelfCheckStatus::Passed
        } else {
            MasterSelfCheckStatus::Attention
        };
    let deep_status = if input.ledger_valid {
        MasterSelfCheckStatus::Passed
    } else {
        MasterSelfCheckStatus::Failed
    };

    let statuses = [
        read_status,
        consistency_status,
        switchability_status,
        deep_status,
    ];
    let level = if statuses.contains(&MasterSelfCheckStatus::Failed) {
        MasterSelfCheckLevel::NeedsManual
    } else if statuses
        .iter()
        .any(|status| *status != MasterSelfCheckStatus::Passed)
    {
        MasterSelfCheckLevel::SelfHealable
    } else {
        MasterSelfCheckLevel::Healthy
    };

    MasterSelfCheckAssessment {
        level,
        read_status,
        consistency_status,
        switchability_status,
        deep_status,
    }
}

#[derive(Clone, Serialize)]
struct MasterSelfCheckItemDto {
    key: &'static str,
    status: &'static str,
    summary: String,
}

#[derive(Clone, Serialize)]
pub(crate) struct MasterSelfCheckDto {
    level: &'static str,
    checks: Vec<MasterSelfCheckItemDto>,
    current_account_name: Option<String>,
    observed_account_name: Option<String>,
    can_repair: bool,
}

/// 读取主库自检报告（G18）。文件读取失败只形成报告，不在自检阶段改写用户数据。
fn get_master_self_check_inner(
    material_root: &Path,
    storage_root: &Path,
) -> Result<MasterSelfCheckDto, String> {
    let registry = EnvironmentRegistry::new(storage_root);
    let record = registry
        .ensure_master()
        .map_err(|_| "environment_registry_invalid".to_string())?;
    let accounts = AccountRegistry::new(material_root)
        .load()
        .map_err(|_| "checkin_registry_invalid".to_string())?;
    let master_dir = master_data_dir().map_err(|_| "environment_registry_invalid".to_string())?;
    let storage_path = master_dir
        .join("User")
        .join("globalStorage")
        .join("storage.json");
    let storage_exists = storage_path.is_file();
    let (storage_readable, blob_present) = if !storage_exists {
        (true, false)
    } else {
        match std::fs::read_to_string(&storage_path)
            .ok()
            .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok())
        {
            Some(value) => (true, value.get("iCubeAuthInfo://usertag").is_some()),
            None => (false, false),
        }
    };
    let observed_user_id = if blob_present {
        archive_login_user_id(&master_dir)
    } else {
        None
    };
    let observed_profile_id = observed_user_id.as_deref().and_then(|user_id| {
        accounts
            .iter()
            .find(|account| account.account_id == user_id)
            .map(|account| account.profile_id.clone())
    });
    let cache_consistent = match (
        record.current_profile_id.as_deref(),
        observed_profile_id.as_deref(),
    ) {
        (None, None) => true,
        (Some(cached), Some(observed)) => cached == observed,
        _ => false,
    };
    // P15-6：主库自检的实例状态按融合版产品判定（与 master_data_dir 同源；
    // WorkCn 规则会把 Trae CN 客户端判为不可归属，自检恒报状态未知）。
    let process_state =
        trae_instance_module::current_product_process_state(TraeProduct::TraeCn, &master_dir);
    let donor_profile_id = observed_profile_id
        .as_deref()
        .or(record.current_profile_id.as_deref());
    // P15-6：融合版切号的真实依赖是 `.fused` 登录容器（P15-1 铸造，16/16
    // 齐全）；Work 时代的 trae-instances 存档目录只覆盖少量旧实例，按它
    // 判定会让多数账号误报「没有可用的登录存档」（用户实测 2026-10-14）。
    let fused_root = storage_root.join("fused-login");
    let donor_available = donor_profile_id
        .map(|profile_id| FusedLoginStore::new(&fused_root).exists(profile_id))
        .unwrap_or(true);
    let ledger_valid = RelayLedger::new(storage_root.join("environments"))
        .load()
        .is_ok();
    let assessment = assess_master_self_check(MasterSelfCheckInput {
        storage_exists,
        storage_readable,
        blob_present,
        blob_decryptable: !blob_present || observed_user_id.is_some(),
        account_registered: !blob_present || observed_profile_id.is_some(),
        cache_consistent,
        instance_process_state: process_state,
        donor_available,
        ledger_valid,
    });
    let current_account_name = record.current_profile_id.as_deref().and_then(|profile_id| {
        accounts
            .iter()
            .find(|account| account.profile_id == profile_id)
            .map(|account| {
                account
                    .display_name
                    .clone()
                    .unwrap_or_else(|| account.screen_name.clone())
            })
    });
    let observed_account_name = observed_profile_id.as_deref().and_then(|profile_id| {
        accounts
            .iter()
            .find(|account| account.profile_id == profile_id)
            .map(|account| {
                account
                    .display_name
                    .clone()
                    .unwrap_or_else(|| account.screen_name.clone())
            })
    });
    let read_summary = if !storage_exists {
        "主库尚未产生登录数据"
    } else if !storage_readable {
        "登录数据文件无法读取"
    } else if !blob_present {
        "主库当前未登录账号"
    } else if observed_user_id.is_none() {
        "登录数据已损坏，需重新登录"
    } else if observed_profile_id.is_none() {
        "当前登录账号尚未登记"
    } else {
        "登录数据可读取，账号可反查"
    };
    let consistency_summary = if cache_consistent {
        "工具记录与实际登录一致"
    } else if observed_profile_id.is_some() {
        "实际登录账号与工具记录不一致"
    } else {
        "暂时无法确认主库当前账号"
    };
    let switchability_summary = match process_state {
        trae_instance_module::ProductProcessState::Running => "请先关闭主库 TRAE 窗口",
        trae_instance_module::ProductProcessState::NotRunning if donor_available => {
            "主库已关闭，可进行切换"
        }
        trae_instance_module::ProductProcessState::NotRunning => "当前账号没有可用的登录存档",
        trae_instance_module::ProductProcessState::Unknown => "无法确认主库运行状态，暂不能切换",
    };
    let deep_summary = if ledger_valid {
        "接力记录可读取，展开后可进行深度核对"
    } else {
        "接力记录不可读取，需要人工保留数据后处理"
    };
    Ok(MasterSelfCheckDto {
        level: assessment.level.as_str(),
        checks: vec![
            MasterSelfCheckItemDto {
                key: "read",
                status: assessment.read_status.as_str(),
                summary: read_summary.to_string(),
            },
            MasterSelfCheckItemDto {
                key: "consistency",
                status: assessment.consistency_status.as_str(),
                summary: consistency_summary.to_string(),
            },
            MasterSelfCheckItemDto {
                key: "switchability",
                status: assessment.switchability_status.as_str(),
                summary: switchability_summary.to_string(),
            },
            MasterSelfCheckItemDto {
                key: "deep",
                status: assessment.deep_status.as_str(),
                summary: deep_summary.to_string(),
            },
        ],
        current_account_name,
        observed_account_name,
        can_repair: observed_profile_id.is_some() && !cache_consistent,
    })
}

/// 读取主库自检报告；fixture 模式不触碰真实目录。
#[tauri::command]
pub(crate) async fn get_master_self_check(
    state: tauri::State<'_, AppState>,
) -> Result<MasterSelfCheckDto, String> {
    ensure_real_preview(&state)?;
    let material_root = checkin_material_root(&state)?;
    let storage_root = PathBuf::from(&state.storage_root);
    run_blocking("master_self_check_join_failed", move || {
        get_master_self_check_inner(&material_root, &storage_root)
    })
    .await
}

/// 把自检观测到的账号写回环境缓存；只修正工具记录，不修改 TRAE 登录数据。
#[tauri::command]
pub(crate) async fn repair_master_current_account(
    state: tauri::State<'_, AppState>,
) -> Result<bool, String> {
    ensure_real_preview(&state)?;
    let material_root = checkin_material_root(&state)?;
    let storage_root = PathBuf::from(&state.storage_root);
    run_blocking("master_self_check_join_failed", move || {
        let registry = EnvironmentRegistry::new(&storage_root);
        let _ = registry
            .ensure_master()
            .map_err(|_| "environment_registry_invalid".to_string())?;
        let accounts = AccountRegistry::new(&material_root)
            .load()
            .map_err(|_| "checkin_registry_invalid".to_string())?;
        let master_dir =
            master_data_dir().map_err(|_| "environment_registry_invalid".to_string())?;
        let observed = observed_profile_id(&accounts, &master_dir)
            .ok_or_else(|| "master_observed_account_unavailable".to_string())?;
        registry
            .set_current_profile(MASTER_ENV_ID, &observed)
            .map_err(|_| "environment_registry_invalid".to_string())
    })
    .await
}

/// 强制把主库的工具缓存指向指定账号。该兜底不修复 TRAE 登录数据，且必须在
/// 主库停止后由前端完成一次明确确认，避免把运行中的主库状态继续写乱。
#[tauri::command]
pub(crate) async fn force_reset_master_current_account(
    profile_id: String,
    state: tauri::State<'_, AppState>,
) -> Result<bool, String> {
    ensure_real_preview(&state)?;
    let material_root = checkin_material_root(&state)?;
    let storage_root = PathBuf::from(&state.storage_root);
    run_blocking("master_self_check_join_failed", move || {
        let _ = trae_instance_account_id(&material_root, &profile_id)?;
        let master_dir =
            master_data_dir().map_err(|_| "environment_registry_invalid".to_string())?;
        let processes = list_trae_processes()
            .map_err(|_| "master_self_check_process_unavailable".to_string())?;
        // P15-6：重置守卫按融合版产品判定运行态（与 master_data_dir 同源）。
        let default_data_dir = trae_instance_module::default_product_data_dir(TraeProduct::TraeCn);
        force_reset_master_current_account_inner(
            &storage_root,
            &master_dir,
            default_data_dir.as_deref(),
            &processes,
            &profile_id,
        )
    })
    .await
}

fn force_reset_master_current_account_inner(
    storage_root: &Path,
    master_dir: &Path,
    default_data_dir: Option<&Path>,
    processes: &[trae_instance_module::TraeProcessInfo],
    profile_id: &str,
) -> Result<bool, String> {
    let process_state = trae_instance_module::product_process_state(
        processes,
        TraeProduct::TraeCn,
        master_dir,
        default_data_dir,
    );
    crate::process_guard::ensure_stopped(process_state, "master_self_check_instance_running")?;
    EnvironmentRegistry::new(storage_root)
        .set_current_profile(MASTER_ENV_ID, profile_id)
        .map_err(|_| "environment_registry_invalid".to_string())
}

#[cfg(test)]
mod force_reset_master_process_tests {
    use super::*;

    #[test]
    fn unknown_process_identity_does_not_change_master_registry() {
        let storage = tempfile::tempdir().unwrap();
        let registry = EnvironmentRegistry::new(storage.path());
        registry.ensure_master().unwrap();
        registry
            .set_current_profile(MASTER_ENV_ID, "profile-old")
            .unwrap();
        let master_dir = storage.path().join("master-data");
        let unknown = [trae_instance_module::TraeProcessInfo {
            pid: 4242,
            // 产品身份已确认但命令行缺失时，危险重置仍必须拒绝。
            exe_path: Some(r"E:\TRAE SOLO CN.exe".to_string()),
            command_line: None,
        }];

        let error = force_reset_master_current_account_inner(
            storage.path(),
            &master_dir,
            Some(&master_dir),
            &unknown,
            "profile-new",
        )
        .unwrap_err();

        assert_eq!(error, "process_state_unknown");
        assert_eq!(
            EnvironmentRegistry::new(storage.path())
                .find(MASTER_ENV_ID)
                .unwrap()
                .unwrap()
                .current_profile_id
                .as_deref(),
            Some("profile-old")
        );
    }
}

// ===== P5-1 主库切号五步事务（环境模型 Q1.1，编排层）=====

/// 主库切号进度事件（`master-switch-progress`；账号页切号弹层 P5-2 消费）。
///
/// 阶段与 Q1.1 五步一一对应（Q10 修正后的顺序）：
/// closing → backing_up → switching_login → handing_over → restarting → done。
/// P7-3：handing_over 阶段附 progress 明细（映射/执行/校验逐项上报，
/// 字段可选，向后兼容无明细的旧事件）。
#[derive(Clone, Serialize)]
struct MasterSwitchProgressEvent {
    profile_id: String,
    stage: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    progress: Option<MasterSwitchProgressDetail>,
}

/// handing_over 阶段的细粒度进度（P7-3；label 为项目名等用户可读文本）。
#[derive(Clone, Serialize)]
struct MasterSwitchProgressDetail {
    /// 交接内部阶段：mapping / executing / verifying。
    phase: String,
    current: usize,
    total: usize,
    label: String,
}

/// P7-2 切号失败回滚结果事件（`master-switch-rolled-back`）：
/// rolled_back = true → 前端失败文案附「已自动还原，可安全重试」；
/// false → 前端指向第 2 步备份链（错误码 master_switch_rollback_failed）。
#[derive(Clone, Serialize)]
struct MasterSwitchRolledBackEvent {
    profile_id: String,
    rolled_back: bool,
}

/// 主库切号完成回执（包含当前账号档案与后续启动结果）。
#[derive(Clone, Serialize)]
pub(crate) struct MasterAccountSwitchDto {
    profile_id: String,
    /// 切换后主库登录账号的 TRAE user_id（= 账号注册表 account_id）。
    to_user_id: String,
    /// 切换前主库实际登录账号（凭据互换时主库 blob 内的 userId）；
    /// 供前端展示「A → B」交接轨迹。
    from_user_id: Option<String>,
    /// 归属随行的 project 行数（Q3 全量随行）。
    transferred_projects: usize,
    /// 自动清理的目标账号空镜像行数（E5 坑位）。
    removed_mirror_rows: usize,
    /// 本次换腿（交接）的会话数。
    switched_sessions: usize,
    /// 主库数据备份文件路径（三件套 .switch-bak-* 副本；人工恢复定位用）。
    backup_path: String,
    /// 接力台账写入结果：false = 交接已完成但台账追加失败
    /// （只影响历史页轨迹展示，不影响切号结果）。
    relay_ledger_written: bool,
    /// 云端插件预同步回执（fail-soft：aborted/failed 只影响插件市场
    /// 显示，不影响切号结果；目标账号可手动重装）。
    plugin_sync: PluginCloudSyncOutcome,
    /// 环境档案是否已记住本次切换的账号。
    current_profile_saved: bool,
    /// 主库重启结果：failed 表示账号与记录已切换，但 TRAE 未自动启动。
    relaunch_outcome: String,
}

/// 主库切号（P5-1，Q1.1 全自动五步事务）：
/// 关实例 → 三件套备份 → 登录凭据互换 → 记录交接 → 重启。
///
/// - `force`：Q1.2 生成中切换——检测到主库 DB 活跃（回复生成中）时，
///   非 force 返回 `master_switch_busy` 交前端弹窗「等它完成还是强制切换」；
///   force=true 中断回复按失败轮次保留（备份先行，不丢弃证据）。
/// - 插件对账（第 4.5 步）按 ADR-0026 静默执行：差异直达目标账号云端；
///   含移除的差异由前端预检弹一次确认后才走到这里（纯新增不确认）。
/// - 前置条件：主库已启动并登录过一次（Q2）；目标账号实例已登录过
///   （供体登录凭据来源）。
#[tauri::command]
pub(crate) async fn switch_master_account(
    profile_id: String,
    force: bool,
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
) -> Result<MasterAccountSwitchDto, String> {
    ensure_real_preview(&state)?;
    if state.source_raw_key.is_empty() {
        return Err("source_key_unavailable".to_string());
    }
    let recovery_root = state
        .production_catalog
        .as_ref()
        .map(|runtime| runtime.recovery_root.clone())
        .unwrap_or(PathBuf::from(recovery_root_path(&state)?));
    let material_root = checkin_material_root(&state)?;
    let storage_root = PathBuf::from(&state.storage_root);
    let raw_key = state.source_raw_key.clone();
    // 切号尝试日志需要（storage_root 被下方闭包 move 走，先克隆）。
    let journal_storage_root = storage_root.clone();
    let journal_profile_id = profile_id.clone();
    // 阻塞执行：关实例（最长 5 秒收口）、WAL 采样、文件复制与
    // 数据库事务均为阻塞 IO/长等待。
    let inner = run_blocking("master_switch_join_failed", move || {
        switch_master_account_inner(
            &app,
            &material_root,
            &storage_root,
            &recovery_root,
            &raw_key,
            &profile_id,
            force,
        )
    })
    .await;
    // 切号尝试日志：成功/失败各记一行（fail-soft，不影响切号结果）。
    let outcome_code = match &inner {
        Ok(_) => "ok",
        Err(code) => code.as_str(),
    };
    journal_switch_attempt(&journal_storage_root, &journal_profile_id, outcome_code);
    // P15-11：切号改写主库归属成功后作废体检预聚合缓存（WAL checkpoint
    // 前指纹不变，显式失效保证零陈旧窗口）。
    if inner.is_ok() {
        traesync_infrastructure::master_checkup::invalidate_master_checkup_cache();
    }
    inner
}

/// 切号尝试日志（2026-09-01 稳健性补强）：每次切号一行 JSONL 追加到
/// `data/master-switch-journal.jsonl`（时间 / 目标 / 结果错误码）。
/// 此前应用无任何持久化日志，切号连续失败只能靠备份链与主库日志考古；
/// 有此日志后失败原因可直接定位。fail-soft：写失败静默跳过。
fn journal_switch_attempt(storage_root: &Path, profile_id: &str, outcome: &str) {
    use std::io::Write;
    let at_unix_seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0);
    let entry = serde_json::json!({
        "at_unix_seconds": at_unix_seconds,
        "profile_id": profile_id,
        "outcome": outcome,
    });
    let path = storage_root.join("master-switch-journal.jsonl");
    let Ok(_lock) = FileOperationLock::acquire_sibling_resource(&path) else {
        return;
    };
    let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    else {
        return;
    };
    let _ = writeln!(file, "{entry}");
}

/// P7-1 切号第 3 步双路径编排（ADR-0024 决策 4：凭据包构造优先、
/// 存档移植降级）。
///
/// E2 首选：凭据包（DPAPI 解密）+ GetUserInfo 实调（只读）构造登录态。
/// 身份校验（服务端 user_id == 凭据包 account_id）前置——不通过绝不
/// 写库（E2 纪律），降级走 E1。
///
/// 仅凭据包/在线校验等能够证明尚未写目标文件的失败允许降级 E1。
/// 写后验证失败或写入状态不确定时立即失败，由外层按原始字节回滚。
///
/// SameAccount 处置（2026-09-01 死锁修复）：主库已登录目标账号时
/// **不再报错中止**，而是跳过登录互换（文件不动）、继续执行第 4 步
/// 记录交接——否则记录归属永远停在旧账号名下，TRAE 全部历史不可见。
pub(crate) fn switch_login_identity_dual_path(
    material_root: &Path,
    record: &traesync_infrastructure::account_registry::AccountRecord,
    donor_dir: &Path,
    master_dir: &Path,
) -> Result<AuthIdentitySwitch, AuthSwitchError> {
    let e2_switch = || {
        // E2 首选：凭据包 + GetUserInfo 实调构造。
        let store = CheckinCredentialStore::new(material_root);
        let binding = CheckinProfileBinding::from(record);
        store
            .load(&binding)
            .map_err(|_| AuthSwitchError::DonorAuthUnavailable)
            .and_then(|bundle| {
                if bundle.account_id.as_str() != record.account_id.as_str() {
                    return Err(AuthSwitchError::DonorAuthUnavailable);
                }
                get_user_info_full(&trae_http_client(), &bundle.access_token)
                    .map(|info| (bundle, info))
                    .map_err(|_| AuthSwitchError::DonorAuthUnavailable)
            })
            .and_then(|(bundle, info)| {
                if info.user_id.as_str() != record.account_id.as_str() {
                    return Err(AuthSwitchError::DonorAuthUnavailable);
                }
                let input = ConstructAuthInput {
                    account_id: &bundle.account_id,
                    access_token: &bundle.access_token,
                    refresh_token: &bundle.refresh_token,
                    access_token_expires_at_unix_seconds: bundle
                        .access_token_expires_at_unix_seconds,
                    refresh_token_expires_at_unix_seconds: bundle
                        .refresh_token_expires_at_unix_seconds,
                    user_info: &info,
                };
                construct_auth_identity(&input, master_dir)
            })
    };
    let e1_switch =
        |error| switch_identity_via_e1_fallback(&record.account_id, donor_dir, master_dir, error);
    switch_login_identity_dual_path_with_ops(record, e2_switch, e1_switch)
}

/// 共享 E2/E1 编排；注入点只替换两条路径执行，回退白名单仍由此处强制。
fn switch_login_identity_dual_path_with_ops(
    record: &traesync_infrastructure::account_registry::AccountRecord,
    e2_switch: impl FnOnce() -> Result<AuthIdentitySwitch, AuthSwitchError>,
    e1_switch: impl FnOnce(AuthSwitchError) -> Result<AuthIdentitySwitch, AuthSwitchError>,
) -> Result<AuthIdentitySwitch, AuthSwitchError> {
    if record.account_id.is_empty() {
        return Err(AuthSwitchError::DonorAuthUnavailable);
    }

    match e2_switch() {
        Ok(identity) if identity.to_user_id == record.account_id => Ok(identity),
        // 返回身份偏离注册目标时，不再写入另一份登录材料。
        Ok(_) => Err(AuthSwitchError::VerifyFailed),
        // 主库已是目标账号，且 E2 已验证凭据绑定及服务端身份，可跳过互换。
        Err(AuthSwitchError::SameAccount) => Ok(same_account_identity(record)),
        // 只有能证明 E2 尚未写入目标文件的错误才允许执行 E1。
        Err(error) if e2_error_allows_e1_fallback(&error) => match e1_switch(error) {
            Ok(identity) if identity.to_user_id == record.account_id => Ok(identity),
            Ok(_) => Err(AuthSwitchError::VerifyFailed),
            Err(AuthSwitchError::SameAccount) => Ok(same_account_identity(record)),
            Err(error) => Err(error),
        },
        Err(error) => Err(error),
    }
}

/// 融合版切号的登录身份构造（P15-5，ADR-0035）：`.fused` 登录代次容器
/// + 在线 GetUserInfo（TraeCn 形态请求头，与融合版客户端一致）构造主库
/// 登录态。不使用 Work 时代的存档移植（E1）——融合版切号的唯一凭据来源
/// 是登录代次容器；容器缺失或凭据过期时切号失败（用户先经阶段一同款
/// 铸造路径重铸，消耗 1 设备位）。
pub(crate) fn switch_login_identity_fused(
    fused_root: &Path,
    record: &traesync_infrastructure::account_registry::AccountRecord,
    master_dir: &Path,
) -> Result<AuthIdentitySwitch, AuthSwitchError> {
    if record.account_id.is_empty() {
        return Err(AuthSwitchError::DonorAuthUnavailable);
    }
    let store = FusedLoginStore::new(fused_root);
    let fused = store
        .load(&record.profile_id, &record.account_id)
        .map_err(|_| AuthSwitchError::DonorAuthUnavailable)?;
    // 在线复核 + 补齐构造所需资料（头像/邮箱/区域等容器未存字段）。
    let info = get_user_info_full_for_oauth_client(
        &trae_http_client(),
        &fused.access_token,
        OAuthClient::TraeCn,
    )
    .map_err(|_| AuthSwitchError::DonorAuthUnavailable)?;
    if info.user_id.as_str() != record.account_id.as_str() {
        return Err(AuthSwitchError::DonorAuthUnavailable);
    }
    construct_auth_identity(
        &ConstructAuthInput {
            account_id: &fused.account_id,
            access_token: &fused.access_token,
            refresh_token: &fused.refresh_token,
            access_token_expires_at_unix_seconds: fused.access_token_expires_at_unix_seconds,
            refresh_token_expires_at_unix_seconds: fused.refresh_token_expires_at_unix_seconds,
            user_info: &info,
        },
        master_dir,
    )
}

/// 切出回存（P15-5 第 0 步，ADR-0035）：把主库当前登录 blob 的最新续期态
/// 写回该账号的 `.fused` 容器——融合版客户端运行时会自动续期并轮换
/// refresh，不回存则该账号下次切入时容器凭据已作废。
///
/// 身份纪律：blob 身份未注册（不在签到档案）或与容器账号错位时**不覆盖
/// 任何容器**——宁可不更新，不可写错。回存失败不阻断切号（fail-soft，
/// 只影响旧账号下次切入的凭据新旧）。返回 Some(profile_id) 表示已回存。
fn save_back_master_container(
    master_dir: &Path,
    fused_root: &Path,
    material_root: &Path,
) -> Option<String> {
    let material = master_login_material(master_dir)?;
    let records = AccountRegistry::new(material_root).load().ok()?;
    let record = records
        .iter()
        .find(|record| record.account_id == material.user_id)?;
    let store = FusedLoginStore::new(fused_root);
    // 容器不存在（该账号从未铸造登录代次）→ 不凭空造容器。
    if !store.exists(&record.profile_id) {
        return None;
    }
    let mut fused = store.load(&record.profile_id, &record.account_id).ok()?;
    fused.access_token = material.access_token;
    fused.refresh_token = material.refresh_token;
    fused.access_token_expires_at_unix_seconds = material.access_token_expires_at_unix_seconds;
    fused.refresh_token_expires_at_unix_seconds = material.refresh_token_expires_at_unix_seconds;
    fused.last_updated_at_unix_seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(fused.last_updated_at_unix_seconds);
    match store.save(&fused) {
        Ok(()) => Some(record.profile_id.clone()),
        Err(error) => {
            eprintln!(
                "[master-switch] 切出回存失败（不阻断，账号 {:?}）：{error}",
                record.profile_id
            );
            None
        }
    }
}

/// 仅让可证明未改写目标文件的 E2 失败尝试 E1。
fn e2_error_allows_e1_fallback(error: &AuthSwitchError) -> bool {
    matches!(
        error,
        AuthSwitchError::DonorAuthUnavailable
            | AuthSwitchError::DonorAuthInvalid
            | AuthSwitchError::TargetAuthUnavailable
            | AuthSwitchError::TargetAuthInvalid
            | AuthSwitchError::AmbiguousBlobValue
            | AuthSwitchError::Io
    )
}

fn switch_identity_via_e1_fallback(
    expected_user_id: &str,
    donor_dir: &Path,
    target_dir: &Path,
    e2_error: AuthSwitchError,
) -> Result<AuthIdentitySwitch, AuthSwitchError> {
    if !e2_error_allows_e1_fallback(&e2_error) {
        return Err(e2_error);
    }
    let identity = switch_auth_identity_for_account(donor_dir, target_dir, expected_user_id)?;
    if identity.to_user_id != expected_user_id {
        return Err(AuthSwitchError::VerifyFailed);
    }
    Ok(identity)
}

/// 主库已是目标账号时的恒等交换（登录互换跳过，文件不动）：
/// from = to = 目标账号。台账 from 优先取交接实测的 previous_owner，
/// 不受此恒等值影响；DTO/插件同步据此走「无差异」路径，语义自洽。
fn same_account_identity(
    record: &traesync_infrastructure::account_registry::AccountRecord,
) -> AuthIdentitySwitch {
    let account_id = record.account_id.clone();
    AuthIdentitySwitch {
        from_user_id: account_id.clone(),
        to_user_id: account_id.clone(),
        to_account: account_id,
    }
}

/// 五步编排主体（阻塞上下文；每步失败即中断并返回对应错误码）。
fn switch_master_account_inner(
    app: &tauri::AppHandle,
    material_root: &Path,
    storage_root: &Path,
    recovery_root: &Path,
    raw_key: &str,
    profile_id: &str,
    force: bool,
) -> Result<MasterAccountSwitchDto, String> {
    let master_dir = master_data_dir().map_err(|_| "environment_registry_invalid".to_string())?;
    // 融合版切号（P15-5，ADR-0035）：主库目录已是融合版数据根，进程
    // 开关必须匹配同一产品的可执行文件，身份切换改用登录代次容器。
    let close_master = |dir: &Path| close_instance(TraeProduct::TraeCn, dir).map(|_| ());
    let launch_master = |dir: &Path, title: &str| {
        launch_instance_common(TraeProduct::TraeCn, dir, title, None)
            .map(|relaunch| relaunch.outcome.to_string())
            .map_err(|error| error.to_string())
    };
    switch_master_account_at(
        app,
        material_root,
        storage_root,
        recovery_root,
        raw_key,
        profile_id,
        &master_dir,
        force,
        &close_master,
        &launch_master,
        &|| {},
    )
}

/// 主库切号编排；目录与进程操作可由合成调用者注入，租约及全部业务副作用仍由此处持有。
pub(crate) fn switch_master_account_at<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    material_root: &Path,
    storage_root: &Path,
    recovery_root: &Path,
    raw_key: &str,
    profile_id: &str,
    master_dir: &Path,
    force: bool,
    close_master: &dyn Fn(&Path) -> Result<(), trae_instance_module::TraeInstanceCloseError>,
    launch_master: &dyn Fn(&Path, &str) -> Result<String, String>,
    before_return: &dyn Fn(),
) -> Result<MasterAccountSwitchDto, String> {
    // 融合版身份切换（P15-5，ADR-0035）：`.fused` 登录代次容器构造。
    // donor_dir 参数保留（离线测试注入签名不变），融合版路径不消费存档。
    let fused_root = storage_root.join("fused-login");
    let identity_switch = |record: &AccountRecord, _donor_dir: &Path, target_dir: &Path| {
        switch_login_identity_fused(&fused_root, record, target_dir)
    };
    // 生产插件对账闭包：真实云端清单吸收 + 差异应用（同原第 4.5 步语义）。
    let records_for_plugins = AccountRegistry::new(material_root).load().unwrap_or_default();
    let production_plugin_sync =
        |from_user_id: &str,
         target_record: &AccountRecord,
         _records: &[AccountRecord]|
         -> PluginCloudSyncOutcome {
            sync_plugins_for_switch(
                material_root,
                storage_root,
                &records_for_plugins,
                from_user_id,
                target_record,
            )
        };
    switch_master_account_at_with_identity_switch(
        app,
        material_root,
        storage_root,
        recovery_root,
        raw_key,
        profile_id,
        master_dir,
        force,
        close_master,
        launch_master,
        before_return,
        &identity_switch,
        &production_plugin_sync,
    )
}

/// 主库切号真实编排；身份回调仅供离线故障注入，默认入口接生产融合版实现。
/// 插件对账回调同理：生产入口传真实云端对账，真机探针（feature=probe）
/// 传 no-op——隔离演练不得触碰任何账号的云端插件清单。
#[allow(clippy::too_many_arguments)]
pub(crate) fn switch_master_account_at_with_identity_switch<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    material_root: &Path,
    storage_root: &Path,
    recovery_root: &Path,
    raw_key: &str,
    profile_id: &str,
    master_dir: &Path,
    force: bool,
    close_master: &dyn Fn(&Path) -> Result<(), trae_instance_module::TraeInstanceCloseError>,
    launch_master: &dyn Fn(&Path, &str) -> Result<String, String>,
    before_return: &dyn Fn(),
    identity_switch: &dyn Fn(
        &AccountRecord,
        &Path,
        &Path,
    ) -> Result<AuthIdentitySwitch, AuthSwitchError>,
    plugin_sync: &dyn Fn(
        &str,
        &traesync_infrastructure::account_registry::AccountRecord,
        &[traesync_infrastructure::account_registry::AccountRecord],
    ) -> PluginCloudSyncOutcome,
) -> Result<MasterAccountSwitchDto, String> {
    let _operation_lease = OperationLease::acquire_shared_for_path(recovery_root, master_dir)
        .map_err(|error| format!("operation_lease_unavailable:{error}"))?;

    // 前置：目标账号在注册表（account_id 即 TRAE user_id，交接与台账都要用）。
    let records = AccountRegistry::new(material_root)
        .load()
        .map_err(|_| "checkin_registry_invalid".to_string())?;
    let record = records
        .iter()
        .find(|record| record.profile_id == profile_id)
        .ok_or_else(|| "trae_profile_invalid".to_string())?;
    let target_user_id = record.account_id.clone();
    let window_title = record
        .display_name
        .clone()
        .unwrap_or_else(|| record.screen_name.clone());

    // 主库 = 官方目录（2026-08-31 修订）：五步事务直接作用于官方数据目录。
    let db_path = master_database_path(master_dir);
    let donor_dir =
        instance_data_dir(storage_root, profile_id).map_err(|e| e.code().to_string())?;
    let emit = |stage: &'static str| {
        let _ = app.emit(
            "master-switch-progress",
            MasterSwitchProgressEvent {
                profile_id: profile_id.to_string(),
                stage,
                progress: None,
            },
        );
    };

    // Q1.2 生成中检测：双采样主库 WAL（间隔 800ms），活跃且未强制 → 报忙。
    // 检测在关实例之前（此刻实例可能还在写库，采样才有意义）。
    if db_path.is_file()
        && master_db_activity_detected(&db_path, std::time::Duration::from_millis(800))
        && !force
    {
        return Err("master_switch_busy".to_string());
    }

    // 第 1 步：关闭主库实例（幂等；优雅 3s + 强制兜底，数据无损已验证。
    // 主库 = 官方目录：用户官方启动的实例同样要能关掉）。
    emit("closing");
    close_master(master_dir).map_err(|error| {
        crate::process_guard::close_failure_code(error, "master_switch_close_failed")
    })?;

    // 第 0.5 步（P15-5，ADR-0035）：切出回存——实例已关、blob 稳定，
    // 把上一个账号的最新续期态写回其 `.fused` 容器（fail-soft：身份
    // 校验失败或写失败都不阻断切号，只影响旧账号下次切入的凭据新旧）。
    let fused_root = storage_root.join("fused-login");
    if let Some(saved_profile) =
        save_back_master_container(master_dir, &fused_root, material_root)
    {
        eprintln!("[master-switch] 切出回存完成：{saved_profile:?}");
    }

    // 第 2 步：三件套备份（ADR-0018 铁律：破坏性批量操作前先备份；
    // create_new 保证永不覆盖既有备份链）。
    emit("backing_up");
    // P11-4①：写入当前活动备份目录（backup_dir 覆盖或 db 同目录）。
    let backup_dir = resolve_master_backup_dir(storage_root, &db_path);
    let backup_path =
        backup_master_trio_to(&db_path, Some(&backup_dir)).map_err(|error| match error {
            MasterHandoverError::DbUnavailable => "master_switch_db_missing".to_string(),
            _ => "master_switch_backup_failed".to_string(),
        })?;
    // P5-9：备份生成后按保留策略清理旧备份（静默容错，不影响切号流程）。
    prune_backups_if_enabled(&db_path, storage_root);

    // P7-2 回滚材料：第 3 步改写前的主库登录态原始字节。读不到（文件
    // 缺失/无登录键）时第 3 步必然报 TargetAuthUnavailable 而不会写库，
    // 无需回滚（主库天然处于原状）。
    let master_storage_path = master_dir
        .join("User")
        .join("globalStorage")
        .join("storage.json");
    let master_login_raw_before = std::fs::read_to_string(&master_storage_path).ok();
    let fail_with_rollback = |code: &str| {
        finish_switch_failure_with_rollback(
            app,
            profile_id,
            &master_storage_path,
            master_login_raw_before.as_deref(),
            code,
        )
    };

    // 第 3 步：登录凭据互换（P7-1 双路径，ADR-0024 决策 4）：
    // E2 首选——凭据包 + GetUserInfo 实调构造登录态（新账号零存档依赖）；
    // 能证明尚未写入的失败才允许 E1 存档移植。写后验证失败或写入状态不确定
    // 不再执行第二种写法，交由外层从本次操作前的原始字节回滚。
    // P7-2：写库后失败（VerifyFailed 等）由 fail_with_rollback 还原原始字节。
    emit("switching_login");
    let identity = match identity_switch(record, &donor_dir, master_dir) {
        Ok(identity) => identity,
        Err(error) => {
            let code = match error {
                AuthSwitchError::DonorAuthUnavailable => "switch_donor_login_missing",
                AuthSwitchError::TargetAuthUnavailable => "master_login_missing",
                AuthSwitchError::SameAccount => "switch_same_account",
                AuthSwitchError::VerifyFailed => "switch_verify_failed",
                _ => "switch_auth_failed",
            };
            return Err(fail_with_rollback(code));
        }
    };

    // 第 4 步：记录交接（归属随行 + 活跃会话原地换腿，单事务；
    // 非空 UNIQUE 冲突报人工决策，禁止静默覆盖）。
    // 已知提交前失败才回滚登录态；提交结果未知或提交后校验失败必须保留
    // 实际状态，直接返回专用错误码，禁止只回滚认证文件。
    // P7-3：交接进度逐项上报（映射/执行/校验，label 用项目名）。
    emit("handing_over");
    let handover = match handover_master_records_with_progress(
        &db_path,
        raw_key,
        &target_user_id,
        &|progress: HandoverProgress| {
            let _ = app.emit(
                "master-switch-progress",
                MasterSwitchProgressEvent {
                    profile_id: profile_id.to_string(),
                    stage: "handing_over",
                    progress: Some(MasterSwitchProgressDetail {
                        phase: progress.phase.to_string(),
                        current: progress.current,
                        total: progress.total,
                        label: progress.label,
                    }),
                },
            );
        },
    ) {
        Ok(handover) => handover,
        Err(error) => {
            let code = match &error {
                MasterHandoverError::CommittedNeedsAttention => {
                    "master_switch_committed_needs_attention"
                }
                MasterHandoverError::CommitOutcomeUnknown => "master_switch_commit_outcome_unknown",
                MasterHandoverError::DbUnavailable => "master_switch_db_missing",
                MasterHandoverError::TargetConflict(_) => "master_switch_conflict",
                _ => "master_switch_db_open_failed",
            };
            return Err(finalize_handover_failure(&error, code, fail_with_rollback));
        }
    };

    // 环境档案写回当前账号（App 重启后环境页与主库窗口标题据此恢复）。
    // P7-2：改 fail-soft——此刻登录态与记录归属已一致切到新账号，回滚
    // 反而制造新杂交态；档案写失败只影响下次启动的显示（档案有默认回退）。
    let current_profile_saved = match EnvironmentRegistry::new(storage_root)
        .set_current_profile(MASTER_ENV_ID, profile_id)
    {
        Ok(_) => true,
        Err(error) => {
            eprintln!("[master-switch] 环境档案写回失败（不阻断）：{error:?}");
            false
        }
    };

    // 接力台账追加（交接成功后）：失败不阻断切号——台账只影响历史页
    // 轨迹展示，缺失可从备份核对（relay_ledger 模块契约）。
    let relay_ledger_written = append_relay_ledger(
        storage_root,
        &handover,
        &identity.from_user_id,
        &target_user_id,
        profile_id,
    );

    // 第 4.5 步：云端插件对账（ADR-0026 决策 3 静默应用，fail-soft）：
    // 吸收切换前账号云端现状进环境清单，再把差异静默应用到目标账号
    // ——缺的装上、多的卸掉，重启后 TRAE 调和本地安装时无事发生。
    // 含移除的差异已在切号弹层预检确认（单次）；任何失败都不阻断切号
    // ——插件可手动重装，切号本身必须完成。回调由入口注入：生产 =
    // 真实云端对账；真机探针 = no-op（隔离演练不动云端）。
    emit("syncing_plugins");
    let plugin_sync = plugin_sync(&identity.from_user_id, record, &records);

    // 第 5 步：重启主库（新登录身份 + 新窗口标题；官方目录带参启动，
    // 与用户官方快捷方式启动等价——同一 data_dir）。
    emit("restarting");
    let relaunch_outcome = match launch_master(master_dir, &window_title) {
        Ok(outcome) => outcome,
        Err(error) => {
            eprintln!("[master-switch] 主库重启失败（切换已完成）：{error}");
            "failed".to_string()
        }
    };
    emit("done");
    before_return();

    Ok(MasterAccountSwitchDto {
        profile_id: profile_id.to_string(),
        to_user_id: identity.to_user_id,
        from_user_id: Some(identity.from_user_id),
        transferred_projects: handover.transferred_projects,
        removed_mirror_rows: handover.removed_mirror_rows,
        switched_sessions: handover.switched_sessions.len(),
        backup_path: backup_path.display().to_string(),
        relay_ledger_written,
        plugin_sync,
        current_profile_saved,
        relaunch_outcome,
    })
}

/// P7-2 回滚核心：主库登录态原始字节整体写回 + 读回比对。
/// 字节一致 ⇒ 加密材料与内容均未变（字节级验证强于解密验证）。
pub(crate) fn rollback_master_login_bytes(master_storage_path: &Path, original: &str) -> bool {
    std::fs::write(master_storage_path, original)
        .and_then(|_| std::fs::read_to_string(master_storage_path))
        .map(|written_back| written_back == original)
        .unwrap_or(false)
}

/// 只有明确未提交的 handover 失败才能回滚认证字节；提交状态不明或已提交时保留当前身份。
pub(crate) fn finalize_handover_failure(
    error: &MasterHandoverError,
    code: &'static str,
    rollback: impl FnOnce(&str) -> String,
) -> String {
    if matches!(
        error,
        MasterHandoverError::CommittedNeedsAttention | MasterHandoverError::CommitOutcomeUnknown
    ) {
        code.to_string()
    } else {
        rollback(code)
    }
}

/// P7-2 切号失败收尾：还原第 3 步改写前的主库登录态原始字节，
/// 掐死「登录身份 = 新账号、记录归属 = 旧账号」的杂交态（LY 案例根因）。
///
/// 三种结局：
/// 1. 无回滚材料（第 3 步未写库，读不到原始字节）→ 主库天然原状，
///    直接返回原错误码，不发事件；
/// 2. 写回成功 → 发 rolled_back=true 事件，返回原错误码，
///    前端文案附「已自动还原，可安全重试」；
/// 3. 写回失败 → 发 rolled_back=false 事件，返回
///    master_switch_rollback_failed（前端指向第 2 步备份链）。
fn finish_switch_failure_with_rollback<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    profile_id: &str,
    master_storage_path: &Path,
    original_raw: Option<&str>,
    code: &str,
) -> String {
    // 结局 1：第 3 步从未写库（storage.json 缺失或读取失败），
    // 主库天然处于切换前状态。
    let Some(original) = original_raw else {
        return code.to_string();
    };

    let rolled_back = rollback_master_login_bytes(master_storage_path, original);

    let _ = app.emit(
        "master-switch-rolled-back",
        MasterSwitchRolledBackEvent {
            profile_id: profile_id.to_string(),
            rolled_back,
        },
    );

    if rolled_back {
        code.to_string()
    } else {
        "master_switch_rollback_failed".to_string()
    }
}

/// 切号编排的云端插件对账入口（ADR-0026 决策 3 静默应用，fail-soft）：
/// 源 = 切换前主库登录账号（凭据互换观察到的 from_user_id），
/// 目标 = 切换目标账号。含移除的差异已由前端预检弹一次确认，这里
/// 始终执行完整对账（吸收后应用）。源账号不在注册表或任一凭据包
/// 读取失败（含 token 失效场景由列表拉取 401 兜底）→ aborted 回执，
/// 不报错。
pub(crate) fn sync_plugins_for_switch(
    material_root: &Path,
    storage_root: &Path,
    records: &[traesync_infrastructure::account_registry::AccountRecord],
    from_user_id: &str,
    target_record: &traesync_infrastructure::account_registry::AccountRecord,
) -> PluginCloudSyncOutcome {
    let aborted = || PluginCloudSyncOutcome {
        source_count: 0,
        installed: 0,
        removed: 0,
        failed: 0,
        skipped: 0,
        aborted: true,
        absorbed: Vec::new(),
    };
    // 源账号：主库切换前实际登录的账号（可能是未注册进 App 的账号）
    let Some(source_record) = records.iter().find(|r| r.account_id == from_user_id) else {
        return aborted();
    };
    let store = CheckinCredentialStore::new(material_root);
    let load_token = |record: &traesync_infrastructure::account_registry::AccountRecord| {
        store
            .load(&CheckinProfileBinding::from(record))
            .map(|bundle| bundle.access_token)
    };
    let (Ok(source_token), Ok(target_token)) =
        (load_token(source_record), load_token(target_record))
    else {
        return aborted();
    };
    let outcome = sync_account_cloud_plugins(&source_token, &target_token);
    // 吸收集落盘环境清单（ADR-0023「吸收」：切换前账号云端现状即基线）。
    // 写失败 fail-soft：清单滞后可在插件 tab 手动吸收补齐，不影响切号回执。
    if !outcome.aborted {
        let entries = manifest_entries_from_cloud(&outcome.absorbed);
        let _ = PluginManifest::new(storage_root).save(&entries);
    }
    outcome
}

/// 切号插件差异预检 DTO（ADR-0026 决策 3：remove_names 非空时前端弹
/// 一次移除确认；纯新增差异静默应用，不弹确认）。
#[derive(Clone, Serialize)]
pub(crate) struct MasterSwitchPluginPreviewDto {
    /// 源账号云端市场插件数（吸收后的清单基数）。
    source_count: usize,
    /// 目标账号云端市场插件数。
    target_count: usize,
    /// 待安装到目标账号的插件名（纯新增，静默应用不确认）。
    install_names: Vec<String>,
    /// 待从目标账号移除的插件名（破坏性差异单列；非空才弹确认）。
    remove_names: Vec<String>,
    /// 预检未完成（环境档案/注册表/凭据/列表任一不可用）：
    /// 前端据此静默直过切号，不弹确认。
    aborted: bool,
}

/// 切号插件差异预检（P5-8b-3）：源 = 主库登录态实测当前账号（与插件 tab 同源），
/// 目标 = 切换目标账号；对账计划与执行侧共用 `reconcile_plan`，保证
/// 「确认的移除清单 = 实际应用的移除」。预检永不返回 Err（fail-soft）。
#[tauri::command]
pub(crate) async fn preview_master_switch_plugins(
    profile_id: String,
    state: tauri::State<'_, AppState>,
) -> Result<MasterSwitchPluginPreviewDto, String> {
    ensure_real_preview(&state)?;
    let material_root = checkin_material_root(&state)?;
    let storage_root = PathBuf::from(&state.storage_root);
    run_blocking("plugin_preview_join_failed", move || {
        Ok(preview_master_switch_plugins_inner(
            &material_root,
            &storage_root,
            &profile_id,
        ))
    })
    .await
}

/// 双方云端列表 → 预检 DTO（纯函数，tests 复用）：吸收集口径与执行侧
/// 一致（非 builtin 且有市场 ID）；remove_names 非空 = 需要移除确认。
fn switch_plugin_preview_from_lists(
    source_items: &[CloudPluginItem],
    target_items: &[CloudPluginItem],
) -> MasterSwitchPluginPreviewDto {
    let source_market: Vec<CloudPluginItem> = source_items
        .iter()
        .filter(|item| !item.builtin && item.marketplace_plugin_id.is_some())
        .cloned()
        .collect();
    let (install, remove) = reconcile_plan(&source_market, target_items);
    let display_of = |item: &CloudPluginItem| {
        let fallback = item
            .marketplace_plugin_id
            .clone()
            .unwrap_or_else(|| item.record_id.clone());
        if !item.display_name.is_empty() {
            item.display_name.clone()
        } else if !item.name.is_empty() {
            item.name.clone()
        } else {
            fallback
        }
    };
    MasterSwitchPluginPreviewDto {
        source_count: source_market.len(),
        target_count: target_items
            .iter()
            .filter(|item| !item.builtin && item.marketplace_plugin_id.is_some())
            .count(),
        install_names: install.iter().map(display_of).collect(),
        remove_names: remove.iter().map(display_of).collect(),
        aborted: false,
    }
}

/// 预检主体（阻塞上下文）：拉取双方云端列表 → 对账计划 → 差异名集合。
fn preview_master_switch_plugins_inner(
    material_root: &Path,
    storage_root: &Path,
    profile_id: &str,
) -> MasterSwitchPluginPreviewDto {
    let aborted = MasterSwitchPluginPreviewDto {
        source_count: 0,
        target_count: 0,
        install_names: Vec::new(),
        remove_names: Vec::new(),
        aborted: true,
    };
    // 源账号必须来自主库登录态实测；环境档案只作为展示缓存。
    let Ok(master_dir) = master_data_dir() else {
        return aborted;
    };
    let Ok(Some(source)) = resolve_actual_master_account(material_root, storage_root, &master_dir)
    else {
        return aborted;
    };
    let records = match AccountRegistry::new(material_root).load() {
        Ok(records) => records,
        Err(_) => return aborted,
    };
    let Some(target) = records.iter().find(|r| r.profile_id == profile_id) else {
        return aborted;
    };
    // 同一账号无从谈差异（switch_same_account 由执行侧兜底，这里直过）。
    if source.profile_id == target.profile_id {
        return aborted;
    }
    let store = CheckinCredentialStore::new(material_root);
    let load_token = |record: &traesync_infrastructure::account_registry::AccountRecord| {
        store
            .load(&CheckinProfileBinding::from(record))
            .map(|bundle| bundle.access_token)
    };
    let (Ok(source_token), Ok(target_token)) = (load_token(&source), load_token(target)) else {
        return aborted;
    };
    let (Ok(source_items), Ok(target_items)) = (
        fetch_installed_plugins(&source_token),
        fetch_installed_plugins(&target_token),
    ) else {
        return aborted;
    };
    switch_plugin_preview_from_lists(&source_items, &target_items)
}

/// 交接成功后的接力台账追加（Q3/Q5）：无会话交接视为无需记录（true）；
/// 追加失败返回 false 交 DTO 透出，不阻断切号主流程。
fn append_relay_ledger(
    storage_root: &Path,
    handover: &MasterHandover,
    fallback_from_user_id: &str,
    to_user_id: &str,
    to_profile_id: &str,
) -> bool {
    if handover.switched_sessions.is_empty() {
        return true;
    }
    // 交接前账号优先取实际归属（Q3 不变量下 = 切换前登录账号）；
    // 无归属记录时回退凭据互换观察到的主库旧登录身份。
    let from_user_id = handover
        .previous_owner_user_id
        .clone()
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| fallback_from_user_id.to_string());
    let switched_at_unix_seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0);
    let entries: Vec<RelayLedgerEntry> = handover
        .switched_sessions
        .iter()
        .map(|session| RelayLedgerEntry {
            session_id: session.session_id.clone(),
            from_session_id: Some(session.previous_session_id.clone()),
            project_id: session.project_id.clone(),
            from_user_id: from_user_id.clone(),
            to_user_id: to_user_id.to_string(),
            to_profile_id: to_profile_id.to_string(),
            message_count_at_switch: session.message_count,
            switched_at_unix_seconds,
        })
        .collect();
    RelayLedger::new(storage_root.join("environments"))
        .append(&entries)
        .is_ok()
}

/// U-6 W4 单账号本地数据占用（字节）：实例目录 + 原生账号目录两部分。
/// 目录缺失记 0（未启动过实例 / 原生客户端未登录过该账号）。
#[derive(Clone, Serialize)]
pub(crate) struct AccountStorageFootprintDto {
    profile_id: String,
    /// 实例数据目录（`{storage_root}/trae-instances/{profile_id}`）递归大小。
    instance_dir_bytes: u64,
    /// 原生账号目录（`{APPDATA}/TRAE SOLO CN_{account_id}`）递归大小；
    /// 档案缺失或 account_id 非纯数字时记 0（无目录可定位）。
    native_dir_bytes: u64,
}

/// 批量查询账号本地数据占用（U-6 W4 归档筛选展示与彻底删除确认弹窗）。
/// 只读统计，不发起网络请求；供前端展示「归档账号占用多少空间」。
#[tauri::command]
pub(crate) async fn get_account_storage_footprint(
    profile_ids: Vec<String>,
    state: tauri::State<'_, AppState>,
) -> Result<Vec<AccountStorageFootprintDto>, String> {
    ensure_real_preview(&state)?;
    for profile_id in &profile_ids {
        if profile_id.is_empty() || profile_id.len() > 256 {
            return Err("checkin_profile_invalid".to_string());
        }
    }
    let material_root = checkin_material_root(&state)?;
    let storage_root = PathBuf::from(&state.storage_root);
    // spawn_blocking：递归目录大小是阻塞 IO（实例目录可达 GB 级）。
    run_blocking("footprint_join_failed", move || {
        // 原生目录依赖 APPDATA（与 seed_login_state 同一解析方式）。
        let appdata = std::env::var_os("APPDATA")
            .map(PathBuf::from)
            .unwrap_or_default();
        get_account_storage_footprint_inner(&material_root, &storage_root, &appdata, &profile_ids)
    })
    .await
}

fn get_account_storage_footprint_inner(
    material_root: &Path,
    storage_root: &Path,
    appdata: &Path,
    profile_ids: &[String],
) -> Result<Vec<AccountStorageFootprintDto>, String> {
    let records = AccountRegistry::new(material_root)
        .load()
        .map_err(|_| "checkin_registry_invalid".to_string())?;
    Ok(profile_ids
        .iter()
        .map(|profile_id| {
            // 实例目录：profile_id 非白名单（路径穿越防御）时按 0 处理，
            // 与 get_trae_instance_states 的宽容口径一致。
            let instance_dir_bytes = instance_data_dir(storage_root, profile_id)
                .map(|dir| dir_size_recursive(&dir))
                .unwrap_or(0);
            // 原生目录：需账号档案的 account_id 定位；档案缺失记 0。
            let native_dir_bytes = records
                .iter()
                .find(|record| &record.profile_id == profile_id)
                .and_then(|record| native_account_dir(appdata, &record.account_id))
                .map(|dir| dir_size_recursive(&dir))
                .unwrap_or(0);
            AccountStorageFootprintDto {
                profile_id: profile_id.clone(),
                instance_dir_bytes,
                native_dir_bytes,
            }
        })
        .collect())
}

/// 彻底删除账号本地数据记录（U-6 W4 资产库三态筛选，用户显式发起的
/// 破坏性命令）：实例数据目录 + 原生账号目录 + 会话索引缓存三处。
///
/// 守卫（后端只做守卫，不做自动触发；确认与二次确认由前端引导）：
/// - 账号不存在拒绝（无档案即无 account_id 可定位原生目录）；
/// - 实例运行中一律拒绝（复用进程观测逻辑，防删除运行中数据损坏库文件）；
/// - 执行前备份主库三件套（ADR-0018 备份铁律，P11-4③）；备份失败拒绝执行；
/// - 归档与否不做强制（由前端引导）。
///
/// 注意：本命令不删注册表档案与凭据包——那是 remove_checkin_account
/// （删账号默认只删身份保留记录）的职责，两者是独立动作。
#[tauri::command]
pub(crate) async fn purge_account_records(
    profile_id: String,
    state: tauri::State<'_, AppState>,
) -> Result<(), String> {
    ensure_real_preview(&state)?;
    if profile_id.is_empty() || profile_id.len() > 256 {
        return Err("checkin_profile_invalid".to_string());
    }
    let material_root = checkin_material_root(&state)?;
    let storage_root = PathBuf::from(&state.storage_root);
    let recovery_root = PathBuf::from(&state.recovery_root);
    // spawn_blocking：进程查询（PowerShell）与递归删除都是阻塞 IO。
    run_blocking("purge_join_failed", move || {
        let master_dir =
            master_data_dir().map_err(|_| "environment_registry_invalid".to_string())?;
        let _master_lease = OperationLease::acquire_shared_for_path(&recovery_root, &master_dir)
            .map_err(|error| format!("operation_lease_unavailable:{error}"))?;
        // 破坏性命令 fail-closed：进程查询失败即拒绝（无法证明实例未运行），
        // 与 launch_master_library 同口径（非 get_trae_instance_states 的宽容降级）。
        let appdata = std::env::var_os("APPDATA")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .ok_or_else(|| "storage_root_unavailable".to_string())?;
        let processes = list_trae_processes().map_err(|e| e.code().to_string())?;
        // P11-4③：主库三件套备份定位（与切号/收编同源；解析失败 fail-closed 拒绝）。
        let master_db_path = master_database_path(&master_dir);
        purge_account_records_inner(
            &material_root,
            &storage_root,
            &appdata,
            &master_db_path,
            &profile_id,
            &processes,
        )
    })
    .await
}

fn purge_account_records_inner(
    material_root: &Path,
    storage_root: &Path,
    appdata: &Path,
    master_db_path: &Path,
    profile_id: &str,
    processes: &[trae_instance_module::TraeProcessInfo],
) -> Result<(), String> {
    if !appdata.is_absolute() {
        return Err("storage_root_unavailable".to_string());
    }
    // 守卫 1：账号必须存在（同时取 account_id 定位原生目录）。
    let record = AccountRegistry::new(material_root)
        .find(profile_id)
        .map_err(|_| "checkin_registry_invalid".to_string())?
        .ok_or_else(|| "checkin_profile_not_found".to_string())?;
    let instance_dir =
        instance_data_dir(storage_root, profile_id).map_err(|e| e.code().to_string())?;
    let native_dir = native_account_dir(appdata, &record.account_id);
    // 两个待删除目录都要确认未被客户端使用；只检查托管实例会漏掉原生账号目录。
    let default_data_dir = appdata.join(TraeProduct::WorkCn.data_root_name());
    let instance_process_state = trae_instance_module::product_process_state(
        processes,
        TraeProduct::WorkCn,
        &instance_dir,
        Some(&default_data_dir),
    );
    crate::process_guard::ensure_stopped(instance_process_state, "purge_instance_running")?;
    if let Some(native_dir) = native_dir.as_deref() {
        let native_process_state = trae_instance_module::product_process_state(
            processes,
            TraeProduct::WorkCn,
            native_dir,
            Some(&default_data_dir),
        );
        crate::process_guard::ensure_stopped(native_process_state, "purge_instance_running")?;
    }
    // 备份铁律（ADR-0018 / P11-4③）：purge 前备份主库三件套（与切号/收编/
    // 环境登录同家族纪律）；主库 db 不存在（TRAE 从未启动）无需备份。
    // 备份失败一律拒绝执行——无备份不动数据。
    if master_db_path.is_file() {
        // P11-4①：写入当前活动备份目录（backup_dir 覆盖或 db 同目录）。
        let backup_dir = resolve_master_backup_dir(storage_root, master_db_path);
        backup_master_trio_to(master_db_path, Some(&backup_dir))
            .map_err(|_| "purge_backup_failed".to_string())?;
        // P5-9：备份生成后按保留策略清理旧备份（静默容错，与家族一致）。
        prune_backups_if_enabled(master_db_path, storage_root);
    }
    // 删除 1：实例数据目录（缺失视为成功——幂等）。
    remove_dir_all_if_exists(&instance_dir)?;
    // 删除 2：原生账号目录（account_id 非纯数字时无目录可删，跳过）。
    if let Some(native) = native_dir {
        remove_dir_all_if_exists(&native)?;
    }
    // 删除 3：会话索引缓存（`{storage_root}/session-index/{profile_id}.json`）。
    SessionIndexCacheStore::new(storage_root)
        .remove(profile_id)
        .map_err(|_| "purge_io_failed".to_string())?;
    Ok(())
}

/// 删除目录（存在时）；缺失视为成功（幂等），其他 IO 失败报错。
fn remove_dir_all_if_exists(dir: &Path) -> Result<(), String> {
    match std::fs::remove_dir_all(dir) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err("purge_io_failed".to_string()),
    }
}

/// 重新读取当前 TRAE 账号证据，并保存非敏感档案。
#[tauri::command]
pub(crate) fn refresh_managed_current_account(
    state: tauri::State<AppState>,
) -> Result<ManagedAccountsWireView, String> {
    refresh_managed_current_account_inner(&state)
}

fn refresh_managed_current_account_inner(
    state: &AppState,
) -> Result<ManagedAccountsWireView, String> {
    let (generation, authorization) = capture_authorized_read_context(state)?;
    let observation = match detect_managed_current_account(state, generation, &authorization) {
        Ok(observation) => observation,
        Err(error) => {
            return invalidate_on_authorization_error(state, generation, &authorization, error)
        }
    };
    let (store, fingerprint_salt) =
        match managed_account_store_for_refresh(state, generation, &authorization, &observation) {
            Ok(prepared) => prepared,
            Err(error) => {
                return invalidate_on_authorization_error(state, generation, &authorization, error)
            }
        };
    let evidence = managed_current_evidence(
        &observation.account,
        observation.data_location_id.clone(),
        observation.observed_at,
        fingerprint_salt.as_deref(),
    );
    let observed_data_root = observation.data_root.clone();
    let service = ManagedAccountSwitchService::new(&store);
    // 所有 I/O 先落到局部候选；失败时共享 runtime 完全不变。
    let mut candidate = ManagedAccountRuntime::default();
    service
        .refresh_current(&mut candidate, evidence)
        .map_err(|error| error.to_string())?;

    // 固定锁序：authorization -> runtime。只有仍属当前授权代次的候选可提交。
    let authorization_guard = state.authorization.lock().unwrap();
    if authorization_guard.generation != generation || authorization_guard.state != authorization {
        return Err("authorization_mismatch".to_string());
    }
    let mut handoff_transition = None;
    let mut runtime_slot = state.managed_account_runtime.lock().unwrap();
    // 外部切换前的计划跨越授权代次保留；重新授权后用新证据完成复核。
    candidate.switch_plan = runtime_slot.runtime.switch_plan.clone();
    if let Some(plan) = candidate.switch_plan.clone() {
        if matches!(
            plan.state,
            traesync_domain::AccountSwitchState::WaitingForTraeClosed
                | traesync_domain::AccountSwitchState::Applying
                | traesync_domain::AccountSwitchState::Verifying
        ) {
            let credential_context = plan
                .backup_reference
                .as_deref()
                .map(|operation_id| {
                    let target = candidate
                        .profiles
                        .iter()
                        .find(|profile| profile.profile_id == plan.target_profile_id)
                        .ok_or_else(|| "target_profile_not_found".to_string())?;
                    let binding = profile_credential_binding(target);
                    let intent_id = load_handoff_intent_for_state(state)?.and_then(|intent| {
                        (intent.target_profile_id == plan.target_profile_id
                            && intent.state != HandoffIntentState::Expired
                            && (intent.credential_operation_id.as_deref() == Some(operation_id)
                                || intent.credential_operation_id.is_none()))
                        .then_some(intent.intent_id)
                    });
                    Ok::<_, String>((binding, intent_id))
                })
                .transpose()?;
            let observed = candidate.current_account.clone();
            match service.complete_switch(
                &mut candidate,
                &plan.plan_id,
                &observed,
                std::time::SystemTime::now(),
            ) {
                Ok(()) => {
                    if let (Some(operation_id), Some((binding, intent_id))) = (
                        plan.backup_reference.as_deref(),
                        credential_context.as_ref(),
                    ) {
                        if let Err(error) = mark_credential_journal(
                            state,
                            &observed_data_root,
                            operation_id,
                            true,
                            binding,
                            intent_id.as_deref(),
                        ) {
                            service
                                .mark_manual_recovery_required(
                                    &mut candidate,
                                    &plan.plan_id,
                                    &error,
                                )
                                .map_err(|mark_error| mark_error.to_string())?;
                            handoff_transition =
                                Some((HandoffIntentState::ManualRecoveryRequired, Some(error)));
                        } else {
                            handoff_transition = Some((HandoffIntentState::TargetVerified, None));
                        }
                    } else {
                        handoff_transition = Some((HandoffIntentState::TargetVerified, None));
                    }
                }
                Err(traesync_application::ManagedAccountSwitchError::TargetEvidenceMismatch) => {
                    if let (Some(operation_id), Some((binding, intent_id))) = (
                        plan.backup_reference.as_deref(),
                        credential_context.as_ref(),
                    ) {
                        let _ = mark_credential_journal(
                            state,
                            &observed_data_root,
                            operation_id,
                            false,
                            binding,
                            intent_id.as_deref(),
                        );
                    }
                    service
                        .mark_manual_recovery_required(
                            &mut candidate,
                            &plan.plan_id,
                            "account_switch_target_evidence_mismatch",
                        )
                        .map_err(|error| error.to_string())?;
                    handoff_transition = Some((
                        HandoffIntentState::ManualRecoveryRequired,
                        Some("account_switch_target_evidence_mismatch".to_string()),
                    ));
                }
                Err(error) => {
                    let reason = error.to_string();
                    if let (Some(operation_id), Some((binding, intent_id))) = (
                        plan.backup_reference.as_deref(),
                        credential_context.as_ref(),
                    ) {
                        let _ = mark_credential_journal(
                            state,
                            &observed_data_root,
                            operation_id,
                            false,
                            binding,
                            intent_id.as_deref(),
                        );
                    }
                    service
                        .mark_manual_recovery_required(&mut candidate, &plan.plan_id, &reason)
                        .map_err(|mark_error| mark_error.to_string())?;
                    handoff_transition =
                        Some((HandoffIntentState::ManualRecoveryRequired, Some(reason)));
                }
            }
        }
    }
    if runtime_slot.authorization_generation == Some(generation)
        && runtime_slot.runtime.current_account.observed_at > candidate.current_account.observed_at
    {
        candidate.current_account = runtime_slot.runtime.current_account.clone();
        candidate.switch_plan = runtime_slot.runtime.switch_plan.clone();
    }
    runtime_slot.runtime = candidate;
    runtime_slot.authorization_generation = Some(generation);
    drop(runtime_slot);
    drop(authorization_guard);
    if let Some((next_state, failure_reason)) = handoff_transition {
        update_handoff_intent_state(state, next_state, failure_reason)?;
    }
    let handoff_intent = load_handoff_intent_for_state(state)?;
    let runtime_slot = state.managed_account_runtime.lock().unwrap();
    Ok(managed_accounts_wire_view(
        &runtime_slot.runtime,
        true,
        handoff_intent.as_ref(),
    ))
}

fn current_verified_profile_id(state: &AppState) -> Option<String> {
    // 固定锁序：authorization -> runtime；仅当前授权代次可标记为“当前已验证”。
    let authorization = state.authorization.lock().unwrap();
    let runtime = state.managed_account_runtime.lock().unwrap();
    let context_current = matches!(authorization.state, AuthorizationState::Authorized { .. })
        && runtime.authorization_generation == Some(authorization.generation);
    if context_current
        && runtime.runtime.current_account.verification_state == AccountVerificationState::Verified
    {
        runtime.runtime.current_account.profile_id.clone()
    } else {
        None
    }
}

/// 历史浏览最小存根。
///
/// P6-2 退役会话索引与计划族后，scan/browse/search/assign 命令已移除，但
/// `account_registry_inner` 仍然需要一份“历史发现的账号集合”作为前端卡片
/// 显示的账号基线（账号发现基于主库已登录账号证据的即时读取）。
///
/// 模式约定：
/// * `RuntimeMode::RealReadPreview` —— 必须已建立生产目录库运行材料；
///   否则直接 fail-closed（保持之前真实 catalog 路径的拒斥语义）。
/// * 其他模式 —— 若授权有效且能读取主库，则读主库账号证据；否则返回空。
fn browse_history_inner(state: &AppState) -> Result<BrowseResult, String> {
    use traesync_domain::{EvidenceState, HistoryBrowseSummary};
    // 1) RealReadPreview：必须有生产目录库运行材料（与测试断言一致）。
    if matches!(state.runtime_mode, RuntimeMode::RealReadPreview)
        && state.production_catalog.is_none()
    {
        return Err("生产目录库运行材料不可用".to_string());
    }
    // 2) 从主库位置读取账号证据，若有就合成一个账号节点；否则返回空集合。
    let mut accounts: Vec<BrowseAccountNode> = Vec::new();
    if matches!(state.runtime_mode, RuntimeMode::RealReadPreview) {
        if let Ok(master_dir) = master_data_dir() {
            let account = state
                .workbench_reader
                .read_account_evidence(&master_dir, std::time::SystemTime::now());
            if account.evidence_state == EvidenceState::Verified {
                if let Some(uid) = account.user_id.as_ref() {
                    accounts.push(BrowseAccountNode {
                        user_id: uid.as_str().to_string(),
                        // P15-6：融合版来源标注。
                        display_label: format!(
                            "{} · {}",
                            TraeProduct::TraeCn.display_name(),
                            user_id_display_fingerprint(uid)
                        ),
                        project_count: 0,
                        session_count: 0,
                    });
                }
            }
        }
    }
    let visible_account_count = accounts.len() as u64;
    Ok(BrowseResult {
        accounts,
        projects: Vec::new(),
        sessions: Vec::new(),
        summary: HistoryBrowseSummary {
            visible_account_count,
            visible_project_count: 0,
            visible_session_count: 0,
            soft_deleted_project_count: 0,
            soft_deleted_session_count: 0,
            soft_deleted_message_count: 0,
        },
    })
}

fn account_registry_inner(state: &AppState) -> Result<AccountRegistryWireView, String> {
    let browse = browse_history_inner(state)?;
    let store = managed_account_store_for_read(state)?;
    let mut profile_runtime = ManagedAccountRuntime::default();
    if let Some(store) = store.as_ref() {
        ManagedAccountSwitchService::new(store)
            .load_profiles(&mut profile_runtime)
            .map_err(|error| error.to_string())?;
    }

    let mut credential_states = std::collections::HashMap::new();
    if !profile_runtime.profiles.is_empty() {
        let vault = credential_vault_for_state(state)?;
        for profile in &profile_runtime.profiles {
            credential_states.insert(
                profile.profile_id.clone(),
                vault.status(&profile_credential_binding(profile)).state,
            );
        }
    }
    let current_profile_id = current_verified_profile_id(state);
    let accounts = browse
        .accounts
        .iter()
        .map(|account| {
            let salted_fingerprint = store
                .as_ref()
                .and_then(|store| store.fingerprint_user_id(&account.user_id).ok());
            let legacy_fingerprint = UserId::from_verified(&account.user_id)
                .ok()
                .map(|user_id| user_id_binding_fingerprint(&user_id));
            build_account_registry_entry(
                account,
                &profile_runtime.profiles,
                current_profile_id.as_deref(),
                &credential_states,
                salted_fingerprint.as_deref(),
                legacy_fingerprint.as_deref(),
            )
        })
        .collect();
    Ok(AccountRegistryWireView { accounts })
}

/// 返回历史发现账号与本机授权档案的只读注册表；不启动 TRAE，不发起签到请求。
#[tauri::command]
pub(crate) fn get_account_registry(
    state: tauri::State<AppState>,
) -> Result<AccountRegistryWireView, String> {
    account_registry_inner(&state)
}

#[cfg(test)]
mod master_switch_rollback_tests {
    use super::*;

    #[test]
    fn rollback_restores_original_bytes_after_step3_write() {
        // P7-2 主场景：第 3 步已改写主库登录态后失败 → 原始字节还原。
        let root = tempfile::tempdir().unwrap();
        let storage_path = root.path().join("storage.json");
        let original = r#"{"icube-dc":{"machineId":"m"},"marscode":{"auth_blob":"AAABBB"}}"#;
        std::fs::write(&storage_path, original).unwrap();
        // 模拟第 3 步改写（杂交态：登录身份已是新账号）。
        std::fs::write(
            &storage_path,
            r#"{"icube-dc":{"machineId":"m"},"marscode":{"auth_blob":"XXXYYY"}}"#,
        )
        .unwrap();

        assert!(rollback_master_login_bytes(&storage_path, original));
        assert_eq!(std::fs::read_to_string(&storage_path).unwrap(), original);
    }

    #[test]
    fn rollback_fails_when_storage_path_unwritable() {
        // 写回失败（路径不存在）：返回 false（外层据此转备份链错误码）。
        let missing = Path::new("Z:/trae-sync-test-missing-rollback/storage.json");
        assert!(!rollback_master_login_bytes(missing, "{}"));
    }

    #[test]
    fn committed_or_unknown_handover_error_skips_auth_rollback() {
        for error in [
            MasterHandoverError::CommittedNeedsAttention,
            MasterHandoverError::CommitOutcomeUnknown,
        ] {
            assert_eq!(
                finalize_handover_failure(&error, "stage_error", |_| {
                    panic!("提交后状态不能触发认证回滚")
                }),
                "stage_error"
            );
        }
    }

    #[test]
    fn known_precommit_handover_error_runs_auth_rollback() {
        assert_eq!(
            finalize_handover_failure(
                &MasterHandoverError::DbOpenFailed,
                "db_open_failed",
                |code| { format!("rolled_back:{code}") }
            ),
            "rolled_back:db_open_failed"
        );
    }
}

#[cfg(test)]
mod set_account_mobile_tests {
    use super::*;

    /// 大陆手机号格式：1[3-9] 开头共 11 位数字。
    #[test]
    fn mainland_mobile_format() {
        assert!(is_mainland_mobile("13812345678"));
        assert!(is_mainland_mobile("19999999999"));
        // 长度不对 / 非数字 / 号段外。
        assert!(!is_mainland_mobile("1381234567"));
        assert!(!is_mainland_mobile("138123456789"));
        assert!(!is_mainland_mobile("1381234567a"));
        assert!(!is_mainland_mobile("12812345678")); // 第二位 2 不在 3-9
        assert!(!is_mainland_mobile("23812345678")); // 非 1 开头
    }

    /// 脱敏号首尾段解析：标准形态返回 (前缀, 后缀)，非预期形态返回 None。
    #[test]
    fn masked_mobile_parts_parsing() {
        assert_eq!(
            masked_mobile_parts("138****0000"),
            Some(("138".to_string(), "0000".to_string()))
        );
        assert_eq!(masked_mobile_parts(""), None);
        assert_eq!(masked_mobile_parts("13800001111"), None); // 无星号
        assert_eq!(masked_mobile_parts("****0000"), None); // 前缀空
        assert_eq!(masked_mobile_parts("138****"), None); // 后缀空
    }

    /// 凭据仓库读写往返：set_mobile_full 后 load 读出同值；None 清除。
    /// （命令层校验在纯函数测试覆盖；这里只验证存储层落盘闭环。）
    #[test]
    fn store_mobile_full_roundtrip() {
        let root = tempfile::tempdir().unwrap();
        let store = CheckinCredentialStore::new(root.path());
        let (private_pem, public_pem) = traesync_infrastructure::generate_device_keypair().unwrap();
        let bundle = traesync_infrastructure::CheckinCredentialBundle {
            profile_id: "profile-mobile".to_string(),
            account_id: "700100".to_string(),
            device_id: "1234567890123456".to_string(),
            machine_id: "machine-mobile".to_string(),
            device_public_key: public_pem,
            device_private_key: private_pem,
            access_token: "token".to_string(),
            refresh_token: "refresh".to_string(),
            client_id: "client".to_string(),
            access_token_expires_at_unix_seconds: u64::MAX,
            refresh_token_expires_at_unix_seconds: u64::MAX,
            mobile_full: None,
        };
        store.save(&bundle).unwrap();
        let binding = CheckinProfileBinding::new(
            bundle.profile_id.clone(),
            bundle.account_id.clone(),
            bundle.device_id.clone(),
            bundle.device_public_key.clone(),
        );
        store
            .set_mobile_full(&binding, Some("13812345678"))
            .unwrap();
        assert_eq!(
            store.load(&binding).unwrap().mobile_full.as_deref(),
            Some("13812345678")
        );
        // 清除后回到 None（展示回退脱敏号）。
        store.set_mobile_full(&binding, None).unwrap();
        assert_eq!(store.load(&binding).unwrap().mobile_full, None);
    }
}

// ============================================================================
// P5-1 主库切号：接力台账追加（append_relay_ledger）——编排层唯一可离线
// 单测的纯逻辑；关实例/凭据互换/交接事务由 infrastructure 层各自测试覆盖，
// 真机五步闭环由 P5-2 UI 接线后的真机验收覆盖。
// ============================================================================

#[cfg(test)]
mod master_switch_tests {
    use super::*;
    // 仅测试消费的构造类型（lib 已不再直接使用，避免非测试构建的 unused 警告）。
    use std::io::{BufRead, BufReader, Write};
    use std::process::{Child, ChildStdin, Command, ExitStatus, Stdio};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::mpsc;
    use traesync_infrastructure::operation_lease::FileOperationLock;
    use traesync_infrastructure::HandoverSession;

    fn poll_child_until_exit(
        child: &mut Child,
        timeout: std::time::Duration,
    ) -> Option<ExitStatus> {
        let deadline = std::time::Instant::now() + timeout;
        loop {
            match child.try_wait() {
                Ok(Some(status)) => return Some(status),
                Ok(None) if std::time::Instant::now() < deadline => {
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
                Ok(None) | Err(_) => return None,
            }
        }
    }

    fn kill_and_reap_child(child: &mut Child) {
        let _ = child.kill();
        let _ = child.wait();
    }

    fn release_and_reap_lease_holder(
        child: &mut Child,
        mut stdin: ChildStdin,
    ) -> Option<ExitStatus> {
        let _ = stdin.write_all(b"\n");
        drop(stdin);
        let status = poll_child_until_exit(child, std::time::Duration::from_secs(5));
        if status.is_none() {
            kill_and_reap_child(child);
        }
        status
    }

    struct ConcurrentWriter {
        started: std::sync::mpsc::Receiver<()>,
        finished: std::sync::mpsc::Receiver<()>,
        thread: std::thread::JoinHandle<()>,
    }

    fn spawn_concurrent_writer(operation: impl FnOnce() + Send + 'static) -> ConcurrentWriter {
        let (started_tx, started) = std::sync::mpsc::channel();
        let (finished_tx, finished) = std::sync::mpsc::channel();
        let thread = std::thread::spawn(move || {
            started_tx.send(()).unwrap();
            operation();
            finished_tx.send(()).unwrap();
        });
        ConcurrentWriter {
            started,
            finished,
            thread,
        }
    }

    fn assert_writers_wait_for_held_locks(writers: &[ConcurrentWriter]) {
        for writer in writers {
            writer
                .started
                .recv_timeout(std::time::Duration::from_secs(5))
                .expect("写线程应在有界时间内启动");
        }
        for writer in writers {
            match writer
                .finished
                .recv_timeout(std::time::Duration::from_millis(250))
            {
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                Ok(()) => panic!("写线程越过了仍被持有的 sidecar 锁"),
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                    panic!("写线程在发送完成信号前异常退出")
                }
            }
        }
    }

    fn join_writers(writers: Vec<ConcurrentWriter>) {
        for writer in writers {
            writer
                .finished
                .recv_timeout(std::time::Duration::from_secs(5))
                .expect("释放 sidecar 锁后写线程应完成");
            writer.thread.join().unwrap();
        }
    }

    /// 子进程持有合成主库租约，供切号真实 caller 的跨进程屏障测试使用。
    #[test]
    fn child_process_holds_master_switch_lease_for_caller_test() {
        let (Ok(recovery_root), Ok(resource_path)) = (
            std::env::var("TRAE_SYNC_MASTER_SWITCH_CALLER_LOCK_ROOT"),
            std::env::var("TRAE_SYNC_MASTER_SWITCH_CALLER_LOCK_RESOURCE"),
        ) else {
            return;
        };
        let _lease = OperationLease::acquire_shared_for_path(
            std::path::Path::new(&recovery_root),
            std::path::Path::new(&resource_path),
        )
        .unwrap();
        let mut stdout = std::io::stdout().lock();
        writeln!(stdout, "MASTER_SWITCH_CALLER_LOCK_READY").unwrap();
        stdout.flush().unwrap();
        let mut line = String::new();
        let _ = std::io::stdin().lock().read_line(&mut line);
    }

    /// 子进程执行真实切号 caller，供父进程对其完成时间设上限。
    #[test]
    fn child_runs_master_switch_caller_for_cross_process_test() {
        if std::env::var("TRAE_SYNC_MASTER_SWITCH_CALLER_RUN")
            .ok()
            .as_deref()
            != Some("1")
        {
            return;
        }
        let recovery_root = std::path::PathBuf::from(
            std::env::var("TRAE_SYNC_MASTER_SWITCH_CALLER_RUN_RECOVERY_ROOT").unwrap(),
        );
        let master_dir = std::path::PathBuf::from(
            std::env::var("TRAE_SYNC_MASTER_SWITCH_CALLER_RUN_MASTER_DIR").unwrap(),
        );
        let material_root = std::path::PathBuf::from(
            std::env::var("TRAE_SYNC_MASTER_SWITCH_CALLER_RUN_MATERIAL_ROOT").unwrap(),
        );
        let storage_root = std::path::PathBuf::from(
            std::env::var("TRAE_SYNC_MASTER_SWITCH_CALLER_RUN_STORAGE_ROOT").unwrap(),
        );
        let app = tauri::test::mock_builder()
            .build(tauri::generate_context!())
            .unwrap();
        let app_handle = app.handle().clone();
        let close_calls = AtomicUsize::new(0);
        let launch_calls = AtomicUsize::new(0);
        let before_return_calls = AtomicUsize::new(0);
        let result = switch_master_account_at(
            &app_handle,
            &material_root,
            &storage_root,
            &recovery_root,
            "unused-test-key",
            "missing-profile",
            &master_dir,
            true,
            &|_| {
                close_calls.fetch_add(1, Ordering::SeqCst);
                Ok(())
            },
            &|_, _| {
                launch_calls.fetch_add(1, Ordering::SeqCst);
                Ok("test-launched".to_string())
            },
            &|| {
                before_return_calls.fetch_add(1, Ordering::SeqCst);
            },
        );

        let error = match result {
            Err(error) => error,
            Ok(_) => panic!("其他进程持有主库租约时切号不应成功"),
        };
        let expected = format!(
            "operation_lease_unavailable:{}",
            traesync_infrastructure::operation_lease::OperationLeaseError::DataLocationBusy
        );
        assert_eq!(error, expected);
        assert_eq!(close_calls.load(Ordering::SeqCst), 0);
        assert_eq!(launch_calls.load(Ordering::SeqCst), 0);
        assert_eq!(before_return_calls.load(Ordering::SeqCst), 0);
        assert_eq!(std::fs::read_dir(storage_root).unwrap().count(), 0);
    }

    fn sample_handover(previous_owner: Option<&str>) -> MasterHandover {
        // 逐会话交接前归属与整体 previous_owner 同源（编排层台账暂不消费
        // 该字段，此处仅保持结构完整；None 场景用空串表达"归属未知"）。
        let previous_user_id = previous_owner.unwrap_or("").to_string();
        MasterHandover {
            previous_owner_user_id: previous_owner.map(str::to_string),
            transferred_projects: 2,
            removed_mirror_rows: 1,
            switched_sessions: vec![
                HandoverSession {
                    session_id: "6c-new-a".to_string(),
                    previous_session_id: "old-a".to_string(),
                    project_id: "p1".to_string(),
                    previous_user_id: previous_user_id.clone(),
                    message_count: 12,
                },
                HandoverSession {
                    session_id: "6c-new-b".to_string(),
                    previous_session_id: "old-b".to_string(),
                    project_id: "p2".to_string(),
                    previous_user_id,
                    message_count: 5,
                },
            ],
        }
    }

    fn single_session_handover(session_id: &str, previous_user_id: &str) -> MasterHandover {
        MasterHandover {
            previous_owner_user_id: Some(previous_user_id.to_string()),
            transferred_projects: 1,
            removed_mirror_rows: 0,
            switched_sessions: vec![HandoverSession {
                session_id: session_id.to_string(),
                previous_session_id: format!("{session_id}-previous"),
                project_id: "p-test".to_string(),
                previous_user_id: previous_user_id.to_string(),
                message_count: 3,
            }],
        }
    }

    #[test]
    fn relay_ledger_append_without_sessions_needs_no_file() {
        let root = tempfile::tempdir().unwrap();
        let handover = MasterHandover {
            previous_owner_user_id: None,
            transferred_projects: 0,
            removed_mirror_rows: 0,
            switched_sessions: Vec::new(),
        };
        // 无会话交接 = 无需记录（true），且不创建台账文件。
        assert!(append_relay_ledger(
            &root.path(),
            &handover,
            "111",
            "222",
            "checkin-b"
        ));
        assert!(!root
            .path()
            .join("environments")
            .join("relay-ledger.json")
            .exists());
    }

    #[test]
    fn relay_ledger_append_persists_entries_with_actual_owner() {
        let root = tempfile::tempdir().unwrap();
        let handover = sample_handover(Some("111"));
        assert!(append_relay_ledger(
            &root.path(),
            &handover,
            "999",
            "222",
            "checkin-b"
        ));

        let entries = RelayLedger::new(root.path().join("environments"))
            .load()
            .unwrap();
        assert_eq!(entries.len(), 2);
        // from 取实际归属（111），而非凭据互换回退值（999）。
        assert_eq!(entries[0].from_user_id, "111");
        assert_eq!(entries[0].to_user_id, "222");
        assert_eq!(entries[0].to_profile_id, "checkin-b");
        assert_eq!(entries[0].session_id, "6c-new-a");
        assert_eq!(entries[0].from_session_id.as_deref(), Some("old-a"));
        assert_eq!(entries[0].message_count_at_switch, 12);
        assert!(entries[0].switched_at_unix_seconds > 0);
    }

    #[test]
    fn relay_ledger_append_falls_back_to_observed_login_when_owner_unknown() {
        let root = tempfile::tempdir().unwrap();
        // 无归属记录（previous_owner=None）但有会话：回退凭据互换观察到的旧登录。
        let handover = sample_handover(None);
        assert!(append_relay_ledger(
            &root.path(),
            &handover,
            "999",
            "222",
            "checkin-b"
        ));
        let entries = RelayLedger::new(root.path().join("environments"))
            .load()
            .unwrap();
        assert_eq!(entries[0].from_user_id, "999");
    }

    #[test]
    fn relay_ledger_append_failure_returns_false_without_panic() {
        let root = tempfile::tempdir().unwrap();
        // 损坏台账文件：追加报错（证据不静默重建），编排层只降级为 false。
        let environments = root.path().join("environments");
        std::fs::create_dir_all(&environments).unwrap();
        std::fs::write(environments.join("relay-ledger.json"), "not json").unwrap();
        let handover = sample_handover(Some("111"));
        assert!(!append_relay_ledger(
            &root.path(),
            &handover,
            "999",
            "222",
            "checkin-b"
        ));
    }

    /// 同时持有两种真实 sidecar 锁，确保档案写者与台账写者确实等待后再竞争。
    #[test]
    fn concurrent_master_switch_registry_and_ledger_writes_preserve_both_resources() {
        let root = tempfile::tempdir().unwrap();
        let storage_root = root.path().to_path_buf();
        let registry = EnvironmentRegistry::new(&storage_root);
        registry.ensure_master().unwrap();
        registry
            .set_current_profile(MASTER_ENV_ID, "checkin-old")
            .unwrap();
        let environment = registry.create("开发环境").unwrap();
        let env_id = environment.env_id.clone();
        registry
            .set_current_profile(&env_id, "checkin-secondary")
            .unwrap();

        let environments_dir = storage_root.join("environments");
        assert!(append_relay_ledger(
            &storage_root,
            &single_session_handover("existing-session", "100"),
            "100",
            "200",
            "checkin-existing"
        ));

        let registry_path = storage_root.join("environments.json");
        let ledger_path = environments_dir.join("relay-ledger.json");
        let registry_lock = FileOperationLock::acquire_sibling_resource(&registry_path).unwrap();
        let ledger_lock = FileOperationLock::acquire_sibling_resource(&ledger_path).unwrap();

        let switch_root = storage_root.clone();
        let switch = spawn_concurrent_writer(move || {
            EnvironmentRegistry::new(&switch_root)
                .set_current_profile(MASTER_ENV_ID, "checkin-target")
                .unwrap();
        });

        let rename_root = storage_root.clone();
        let rename = spawn_concurrent_writer(move || {
            EnvironmentRegistry::new(rename_root)
                .rename(&env_id, "并发重命名")
                .unwrap();
        });

        let switch_ledger_root = storage_root.clone();
        let switch_ledger = spawn_concurrent_writer(move || {
            assert!(append_relay_ledger(
                &switch_ledger_root,
                &single_session_handover("switch-session", "111"),
                "999",
                "222",
                "checkin-target"
            ));
        });

        let other_ledger_root = storage_root.clone();
        let other_ledger = spawn_concurrent_writer(move || {
            assert!(append_relay_ledger(
                &other_ledger_root,
                &single_session_handover("other-session", "333"),
                "999",
                "444",
                "checkin-other"
            ));
        });

        let writers = vec![switch, rename, switch_ledger, other_ledger];
        assert_writers_wait_for_held_locks(&writers);
        drop(ledger_lock);
        drop(registry_lock);
        join_writers(writers);

        let environments = registry.load_all().unwrap();
        assert_eq!(environments.len(), 2);
        assert_eq!(
            environments
                .iter()
                .find(|record| record.env_id == MASTER_ENV_ID)
                .and_then(|record| record.current_profile_id.as_deref()),
            Some("checkin-target")
        );
        assert!(environments
            .iter()
            .any(|record| record.env_id == environment.env_id
                && record.name == "并发重命名"
                && record.current_profile_id.as_deref() == Some("checkin-secondary")));

        let ledger = RelayLedger::new(environments_dir).load().unwrap();
        assert_eq!(ledger.len(), 3);
        for (session_id, from_user_id, to_user_id, profile_id) in [
            ("existing-session", "100", "200", "checkin-existing"),
            ("switch-session", "111", "222", "checkin-target"),
            ("other-session", "333", "444", "checkin-other"),
        ] {
            let matching: Vec<_> = ledger
                .iter()
                .filter(|entry| entry.session_id == session_id)
                .collect();
            assert_eq!(matching.len(), 1, "台账中的会话应恰好出现一次");
            assert_eq!(matching[0].from_user_id, from_user_id);
            assert_eq!(matching[0].to_user_id, to_user_id);
            assert_eq!(matching[0].to_profile_id, profile_id);
        }
    }

    /// 跨进程主库租约必须在账号读取、关实例和其他切号副作用之前生效。
    #[test]
    fn master_switch_caller_rejects_cross_process_lease_before_side_effects() {
        let recovery_root = tempfile::tempdir().unwrap();
        let master_dir = tempfile::tempdir().unwrap();
        let material_root = tempfile::tempdir().unwrap();
        let storage_root = tempfile::tempdir().unwrap();
        let mut child = Command::new(std::env::current_exe().unwrap())
            .env("TRAE_SYNC_MASTER_SWITCH_CALLER_LOCK_ROOT", recovery_root.path())
            .env(
                "TRAE_SYNC_MASTER_SWITCH_CALLER_LOCK_RESOURCE",
                master_dir.path(),
            )
            .args([
                "--exact",
                "credential_handover::master_switch_tests::child_process_holds_master_switch_lease_for_caller_test",
                "--nocapture",
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let child_stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        let (ready_tx, ready_rx) = mpsc::channel();
        std::thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            let mut line = String::new();
            let mut ready_sent = false;
            loop {
                line.clear();
                match reader.read_line(&mut line) {
                    Ok(0) | Err(_) => {
                        if !ready_sent {
                            let _ = ready_tx.send(false);
                        }
                        return;
                    }
                    Ok(_) if !ready_sent && line.trim() == "MASTER_SWITCH_CALLER_LOCK_READY" => {
                        let _ = ready_tx.send(true);
                        ready_sent = true;
                    }
                    Ok(_) => {}
                }
            }
        });
        if ready_rx.recv_timeout(std::time::Duration::from_secs(10)) != Ok(true) {
            let _ = release_and_reap_lease_holder(&mut child, child_stdin);
            panic!("持锁子进程未在 10 秒内报告就绪");
        }
        let caller = Command::new(std::env::current_exe().unwrap())
            .env("TRAE_SYNC_MASTER_SWITCH_CALLER_RUN", "1")
            .env(
                "TRAE_SYNC_MASTER_SWITCH_CALLER_RUN_RECOVERY_ROOT",
                recovery_root.path(),
            )
            .env(
                "TRAE_SYNC_MASTER_SWITCH_CALLER_RUN_MASTER_DIR",
                master_dir.path(),
            )
            .env(
                "TRAE_SYNC_MASTER_SWITCH_CALLER_RUN_MATERIAL_ROOT",
                material_root.path(),
            )
            .env(
                "TRAE_SYNC_MASTER_SWITCH_CALLER_RUN_STORAGE_ROOT",
                storage_root.path(),
            )
            .args([
                "--exact",
                "credential_handover::master_switch_tests::child_runs_master_switch_caller_for_cross_process_test",
                "--nocapture",
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .spawn();
        let mut caller = match caller {
            Ok(child) => child,
            Err(error) => {
                let _ = release_and_reap_lease_holder(&mut child, child_stdin);
                panic!("无法启动切号 caller 测试子进程：{error}");
            }
        };

        let caller_status = poll_child_until_exit(&mut caller, std::time::Duration::from_secs(10));
        if caller_status.is_none() {
            let _ = release_and_reap_lease_holder(&mut child, child_stdin);
            let _ = poll_child_until_exit(&mut caller, std::time::Duration::from_secs(5));
            if caller.try_wait().ok().flatten().is_none() {
                kill_and_reap_child(&mut caller);
            }
            panic!("切号 caller 未在持锁期间 10 秒内返回；已释放并回收测试子进程");
        }

        let holder_status = release_and_reap_lease_holder(&mut child, child_stdin);
        assert!(holder_status.is_some_and(|status| status.success()));
        assert!(caller_status.unwrap().success());
    }
}

// ============================================================================
// P8-5 G18：主库自检四级判定。
// ============================================================================
#[cfg(test)]
mod master_self_check_tests {
    use super::*;

    fn input() -> MasterSelfCheckInput {
        MasterSelfCheckInput {
            storage_exists: true,
            storage_readable: true,
            blob_present: true,
            blob_decryptable: true,
            account_registered: true,
            cache_consistent: true,
            instance_process_state: trae_instance_module::ProductProcessState::NotRunning,
            donor_available: true,
            ledger_valid: true,
        }
    }

    #[test]
    fn healthy_when_four_checks_pass() {
        let assessment = assess_master_self_check(input());

        assert_eq!(assessment.level, MasterSelfCheckLevel::Healthy);
        assert_eq!(assessment.read_status, MasterSelfCheckStatus::Passed);
        assert_eq!(assessment.consistency_status, MasterSelfCheckStatus::Passed);
        assert_eq!(
            assessment.switchability_status,
            MasterSelfCheckStatus::Passed
        );
        assert_eq!(assessment.deep_status, MasterSelfCheckStatus::Passed);
    }

    #[test]
    fn stale_registry_is_self_healable() {
        let assessment = assess_master_self_check(MasterSelfCheckInput {
            cache_consistent: false,
            ..input()
        });

        assert_eq!(assessment.level, MasterSelfCheckLevel::SelfHealable);
        assert_eq!(
            assessment.consistency_status,
            MasterSelfCheckStatus::Attention
        );
    }

    #[test]
    fn corrupt_blob_requires_manual_login() {
        let assessment = assess_master_self_check(MasterSelfCheckInput {
            blob_decryptable: false,
            ..input()
        });

        assert_eq!(assessment.level, MasterSelfCheckLevel::NeedsManual);
        assert_eq!(assessment.read_status, MasterSelfCheckStatus::Failed);
    }

    #[test]
    fn unreadable_storage_requires_manual_attention() {
        let assessment = assess_master_self_check(MasterSelfCheckInput {
            storage_readable: false,
            ..input()
        });

        assert_eq!(assessment.level, MasterSelfCheckLevel::NeedsManual);
        assert_eq!(assessment.read_status, MasterSelfCheckStatus::Failed);
    }

    #[test]
    fn running_instance_blocks_switchability() {
        let assessment = assess_master_self_check(MasterSelfCheckInput {
            instance_process_state: trae_instance_module::ProductProcessState::Running,
            ..input()
        });

        assert_eq!(assessment.level, MasterSelfCheckLevel::SelfHealable);
        assert_eq!(
            assessment.switchability_status,
            MasterSelfCheckStatus::Blocked
        );
    }

    #[test]
    fn unknown_instance_process_state_blocks_switchability() {
        let assessment = assess_master_self_check(MasterSelfCheckInput {
            instance_process_state: trae_instance_module::ProductProcessState::Unknown,
            ..input()
        });

        assert_eq!(
            assessment.switchability_status,
            MasterSelfCheckStatus::Blocked
        );
    }

    #[test]
    fn invalid_ledger_requires_manual_attention() {
        let assessment = assess_master_self_check(MasterSelfCheckInput {
            ledger_valid: false,
            ..input()
        });

        assert_eq!(assessment.level, MasterSelfCheckLevel::NeedsManual);
        assert_eq!(assessment.deep_status, MasterSelfCheckStatus::Failed);
    }
}

#[cfg(test)]
mod real_checkin_tests {
    use super::*;
    // P12-R3：AppState 构造夹具与注册表档案夹具为跨域共享，提至 test_support。
    use crate::test_support::{
        make_real_mode_state, registered_account, registered_account_with_id,
    };

    /// U-1 真实环境验收：refresh 路径对脱敏手机号缺失的存量账号无感补采。
    /// 只读（status + usage + GetUserInfo，不 claim）；生产存储 6 账号串行。
    #[test]
    #[ignore = "U-1 真实生产存储验收：refresh + GetUserInfo 补采（只读，不 claim）"]
    fn real_refresh_backfills_masked_mobile() {
        let material_root = std::env::var_os("LOCALAPPDATA")
            .map(|base| {
                std::path::PathBuf::from(base)
                    .join("Trae Sync")
                    .join("data")
                    .join("checkin")
            })
            .expect("LOCALAPPDATA 未设置");
        let registry = AccountRegistry::new(&material_root);
        let records = registry.load().expect("生产注册表读取失败");
        assert!(!records.is_empty(), "生产注册表为空");
        let ids: Vec<String> = records.iter().map(|r| r.profile_id.clone()).collect();
        let entries = refresh_checkin_credits_inner(
            &material_root,
            &ids,
            RuntimeMode::RealReadPreview,
            // 真实验收脚本：不需要进度推送。
            &|_| {},
        )
        .expect("真实 refresh 失败");
        for entry in &entries {
            println!(
                "== refresh {} -> credits={:?} usage={:?} err={:?}",
                entry.profile_id, entry.credits, entry.usage_remaining_credits, entry.error_code
            );
        }
        let after = registry.load().expect("补采后注册表读取失败");
        for record in &after {
            println!(
                "== {}（{}）：masked_mobile={}",
                record.screen_name,
                record.profile_id,
                if record.masked_mobile.is_empty() {
                    "<空>"
                } else {
                    &record.masked_mobile
                }
            );
        }
        let missing: Vec<&str> = after
            .iter()
            .filter(|r| r.masked_mobile.is_empty())
            .map(|r| r.screen_name.as_str())
            .collect();
        assert!(
            missing.is_empty(),
            "补采后仍有账号缺手机号（服务端未返回？）：{missing:?}"
        );
    }

    // P12-R3：login_commands_reject_fixture_mode / begin_login_rejects_when_storage_root_unavailable
    // / begin_login_produces_url_and_consumable_session 三例随登录命令链迁至
    // oauth_login::login_command_tests。

    #[test]
    fn list_checkin_accounts_returns_non_sensitive_whitelist() {
        let root = tempfile::tempdir().unwrap();
        registered_account(root.path(), "profile-a");
        let state = make_real_mode_state(root.path());
        let accounts = list_checkin_accounts_inner(&state).unwrap();
        assert_eq!(accounts.len(), 1);
        assert_eq!(accounts[0].profile_id, "profile-a");
        assert_eq!(accounts[0].account_id, "account-profile-a");
        assert_eq!(accounts[0].screen_name, "测试账号");
        // wire 白名单：不含设备 ID/设备公钥等绑定材料。
        let value = serde_json::to_value(&accounts[0]).unwrap();
        let keys: std::collections::BTreeSet<String> =
            value.as_object().unwrap().keys().cloned().collect();
        assert_eq!(
            keys,
            [
                "account_id",
                "archived",
                "avatar_url",
                "last_verified_at",
                "profile_id",
                "screen_name"
            ]
            .into_iter()
            .map(str::to_string)
            .collect()
        );
        let serialized = serde_json::to_string(&value).unwrap();
        for forbidden in [
            "device_id",
            "device_public_key",
            "refresh_token",
            "access_token",
        ] {
            assert!(!serialized.contains(forbidden), "泄漏字段: {forbidden}");
        }
    }

    // ====================================================================
    // U-6 W4：资产库三态筛选后端原语（归档开关 / 占用统计 / 彻底删除记录）
    // ====================================================================

    /// U-6 W4：占用统计——实例目录 + 原生目录递归求和；目录缺失记 0。
    #[test]
    fn storage_footprint_sums_dirs_and_missing_is_zero() {
        let root = tempfile::tempdir().unwrap();
        // profile-a：纯数字 account_id 可定位原生目录；profile-b：非数字
        // account_id 且从未启动实例（两种「记 0」口径都覆盖）。
        registered_account_with_id(root.path(), "profile-a", "1234567890");
        registered_account(root.path(), "profile-b");

        let instance_dir = root.path().join("trae-instances").join("profile-a");
        std::fs::create_dir_all(instance_dir.join("User")).unwrap();
        std::fs::write(instance_dir.join("machineid"), vec![0u8; 10]).unwrap();
        std::fs::write(
            instance_dir.join("User").join("storage.json"),
            vec![0u8; 26],
        )
        .unwrap();
        let appdata = tempfile::tempdir().unwrap();
        let native = appdata.path().join("TRAE SOLO CN_1234567890");
        std::fs::create_dir_all(&native).unwrap();
        std::fs::write(native.join("machineid"), vec![0u8; 7]).unwrap();

        let entries = get_account_storage_footprint_inner(
            &root.path().join("checkin"),
            root.path(),
            appdata.path(),
            &["profile-a".to_string(), "profile-b".to_string()],
        )
        .unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].profile_id, "profile-a");
        assert_eq!(entries[0].instance_dir_bytes, 36);
        assert_eq!(entries[0].native_dir_bytes, 7);
        // 目录缺失（实例未启动）与 account_id 非纯数字（原生目录定位不到）均记 0。
        assert_eq!(entries[1].profile_id, "profile-b");
        assert_eq!(entries[1].instance_dir_bytes, 0);
        assert_eq!(entries[1].native_dir_bytes, 0);
    }

    /// U-6 W4：purge 守卫——实例运行中一律拒绝，目录原样保留。
    /// 进程观测逻辑与 get_trae_instance_states 一致（--user-data-dir 匹配）。
    #[test]
    fn purge_rejects_when_instance_running() {
        let root = tempfile::tempdir().unwrap();
        registered_account_with_id(root.path(), "profile-a", "1234567890");
        let instance_dir = root.path().join("trae-instances").join("profile-a");
        std::fs::create_dir_all(&instance_dir).unwrap();
        std::fs::write(instance_dir.join("storage.json"), b"{}").unwrap();
        // 模拟运行中的实例进程：命令行指向该实例目录（引号形态与真实
        // PowerShell 输出一致，进程归属 matcher 会归一化路径大小写和引号）。
        let processes = vec![trae_instance_module::TraeProcessInfo {
            pid: 4242,
            exe_path: Some(r"C:\TRAE.exe".to_string()),
            command_line: Some(format!(
                r#""C:\TRAE.exe" --user-data-dir={}"#,
                instance_dir.display()
            )),
        }];
        let appdata = tempfile::tempdir().unwrap();
        let error = purge_account_records_inner(
            &root.path().join("checkin"),
            root.path(),
            appdata.path(),
            &root.path().join("master").join("database.db"),
            "profile-a",
            &processes,
        )
        .unwrap_err();
        assert_eq!(error, "purge_instance_running");
        // 守卫生效：实例目录原样保留。
        assert!(instance_dir.join("storage.json").is_file());
    }

    /// 原生账号目录也可能被 TRAE 直接作为 user-data-dir 使用，删除前同样要确认停机。
    #[test]
    fn purge_rejects_when_native_account_directory_running_before_backup_or_deletion() {
        let root = tempfile::tempdir().unwrap();
        registered_account_with_id(root.path(), "profile-a", "1234567890");
        let instance_dir = root.path().join("trae-instances").join("profile-a");
        std::fs::create_dir_all(&instance_dir).unwrap();
        std::fs::write(instance_dir.join("storage.json"), b"keep-instance").unwrap();
        let master_dir = root.path().join("master");
        std::fs::create_dir_all(&master_dir).unwrap();
        let master_db = master_dir.join("database.db");
        std::fs::write(&master_db, b"master-db-bytes").unwrap();
        let appdata = tempfile::tempdir().unwrap();
        let native_dir = appdata.path().join("TRAE SOLO CN_1234567890");
        std::fs::create_dir_all(native_dir.join("User").join("globalStorage")).unwrap();
        std::fs::write(
            native_dir
                .join("User")
                .join("globalStorage")
                .join("storage.json"),
            b"keep-native",
        )
        .unwrap();
        let processes = [trae_instance_module::TraeProcessInfo {
            pid: 4243,
            exe_path: Some(r"C:\TRAE SOLO CN\TRAE SOLO CN.exe".to_string()),
            command_line: Some(format!(
                r#""C:\TRAE SOLO CN\TRAE SOLO CN.exe" --user-data-dir="{}""#,
                native_dir.display()
            )),
        }];

        let error = purge_account_records_inner(
            &root.path().join("checkin"),
            root.path(),
            appdata.path(),
            &master_db,
            "profile-a",
            &processes,
        )
        .unwrap_err();

        assert_eq!(error, "purge_instance_running");
        assert_eq!(
            std::fs::read(instance_dir.join("storage.json")).unwrap(),
            b"keep-instance"
        );
        assert_eq!(
            std::fs::read(
                native_dir
                    .join("User")
                    .join("globalStorage")
                    .join("storage.json")
            )
            .unwrap(),
            b"keep-native"
        );
        assert!(!std::fs::read_dir(&master_dir)
            .unwrap()
            .filter_map(Result::ok)
            .any(|entry| entry.file_name().to_string_lossy().contains(".switch-bak-")));
    }

    /// P14-2：身份不完整时拒绝 purge，且不得先备份或删除任何现场。
    #[test]
    fn purge_rejects_unknown_process_before_backup_or_deletion() {
        let root = tempfile::tempdir().unwrap();
        registered_account_with_id(root.path(), "profile-a", "1234567890");
        let instance_dir = root.path().join("trae-instances").join("profile-a");
        std::fs::create_dir_all(&instance_dir).unwrap();
        std::fs::write(instance_dir.join("storage.json"), b"keep").unwrap();
        let master_dir = root.path().join("master");
        std::fs::create_dir_all(&master_dir).unwrap();
        let master_db = master_dir.join("database.db");
        std::fs::write(&master_db, b"master-db-bytes").unwrap();
        let appdata = tempfile::tempdir().unwrap();
        let processes = [trae_instance_module::TraeProcessInfo {
            pid: 4244,
            exe_path: None,
            command_line: None,
        }];

        let error = purge_account_records_inner(
            &root.path().join("checkin"),
            root.path(),
            appdata.path(),
            &master_db,
            "profile-a",
            &processes,
        )
        .unwrap_err();

        assert_eq!(error, "process_state_unknown");
        assert_eq!(
            std::fs::read(instance_dir.join("storage.json")).unwrap(),
            b"keep"
        );
        assert!(!std::fs::read_dir(&master_dir)
            .unwrap()
            .filter_map(Result::ok)
            .any(|entry| entry.file_name().to_string_lossy().contains(".switch-bak-")));
    }

    /// APPDATA 缺失或相对路径不能退化成工作目录下的删除目标。
    #[test]
    fn purge_rejects_relative_appdata_before_backup_or_deletion() {
        let root = tempfile::tempdir().unwrap();
        registered_account_with_id(root.path(), "profile-a", "1234567890");
        let instance_dir = root.path().join("trae-instances").join("profile-a");
        std::fs::create_dir_all(&instance_dir).unwrap();
        std::fs::write(instance_dir.join("storage.json"), b"keep-instance").unwrap();
        let master_dir = root.path().join("master");
        std::fs::create_dir_all(&master_dir).unwrap();
        let master_db = master_dir.join("database.db");
        std::fs::write(&master_db, b"master-db-bytes").unwrap();

        let error = purge_account_records_inner(
            &root.path().join("checkin"),
            root.path(),
            Path::new("relative-appdata"),
            &master_db,
            "profile-a",
            &[],
        )
        .unwrap_err();

        assert_eq!(error, "storage_root_unavailable");
        assert!(instance_dir.join("storage.json").is_file());
        assert!(!std::fs::read_dir(&master_dir)
            .unwrap()
            .filter_map(Result::ok)
            .any(|entry| entry.file_name().to_string_lossy().contains(".switch-bak-")));
    }

    /// U-6 W4：purge 守卫——账号不存在拒绝（无档案即无 account_id 可定位原生目录）。
    #[test]
    fn purge_rejects_unknown_account() {
        let root = tempfile::tempdir().unwrap();
        let appdata = tempfile::tempdir().unwrap();
        let error = purge_account_records_inner(
            &root.path().join("checkin"),
            root.path(),
            appdata.path(),
            &root.path().join("master").join("database.db"),
            "profile-404",
            &[],
        )
        .unwrap_err();
        assert_eq!(error, "checkin_profile_not_found");
    }

    /// U-6 W4：purge 删除实例目录 + 原生账号目录 + 会话索引缓存三处；
    /// 档案保留（purge 不删身份），且幂等可重复执行。
    #[test]
    fn purge_removes_dirs_and_cache_and_is_idempotent() {
        let root = tempfile::tempdir().unwrap();
        registered_account_with_id(root.path(), "profile-a", "1234567890");
        // 三处现场：实例目录、原生目录、会话索引缓存。
        let instance_dir = root.path().join("trae-instances").join("profile-a");
        std::fs::create_dir_all(instance_dir.join("User").join("globalStorage")).unwrap();
        std::fs::write(instance_dir.join("machineid"), b"machine").unwrap();
        let appdata = tempfile::tempdir().unwrap();
        let native = appdata.path().join("TRAE SOLO CN_1234567890");
        std::fs::create_dir_all(native.join("User").join("globalStorage")).unwrap();
        std::fs::write(native.join("machineid"), b"machine").unwrap();
        let cache_store = SessionIndexCacheStore::new(root.path());
        cache_store
            .save(
                "profile-a",
                &traesync_infrastructure::account_session_index::CachedSessionIndex {
                    // 与 CACHE_FORMAT_VERSION（当前为 1）一致的有效缓存。
                    format_version: 1,
                    sessions: vec![],
                    last_max_updated_at: None,
                },
            )
            .unwrap();
        let cache_file = root.path().join("session-index").join("profile-a.json");
        assert!(cache_file.is_file());

        purge_account_records_inner(
            &root.path().join("checkin"),
            root.path(),
            appdata.path(),
            &root.path().join("master").join("database.db"),
            "profile-a",
            &[],
        )
        .unwrap();

        assert!(!instance_dir.exists());
        assert!(!native.exists());
        assert!(!cache_file.exists());
        // 档案保留：purge 只删数据记录，不删身份（那是 remove_checkin_account 的职责）。
        assert!(AccountRegistry::new(root.path().join("checkin"))
            .find("profile-a")
            .unwrap()
            .is_some());
        // 幂等：目录与缓存已缺失时重复执行仍成功。
        purge_account_records_inner(
            &root.path().join("checkin"),
            root.path(),
            appdata.path(),
            &root.path().join("master").join("database.db"),
            "profile-a",
            &[],
        )
        .unwrap();
    }

    /// P11-4③：purge 备份先行——主库 db 存在时先留三件套备份（create_new
    /// 永不覆盖既有链），备份成功后再执行删除。
    #[test]
    fn purge_backs_up_master_trio_before_deletion() {
        let root = tempfile::tempdir().unwrap();
        registered_account_with_id(root.path(), "profile-a", "1234567890");
        let instance_dir = root.path().join("trae-instances").join("profile-a");
        std::fs::create_dir_all(&instance_dir).unwrap();
        std::fs::write(instance_dir.join("storage.json"), b"{}").unwrap();
        // 主库 db + wal 现场（备份只做文件复制，字节内容任意）。
        let master_dir = root.path().join("master");
        std::fs::create_dir_all(&master_dir).unwrap();
        let master_db = master_dir.join("database.db");
        std::fs::write(&master_db, b"master-db-bytes").unwrap();
        std::fs::write(master_dir.join("database.db-wal"), b"wal-bytes").unwrap();
        let appdata = tempfile::tempdir().unwrap();

        purge_account_records_inner(
            &root.path().join("checkin"),
            root.path(),
            appdata.path(),
            &master_db,
            "profile-a",
            &[],
        )
        .unwrap();

        // 备份生成：主文件内容与 db 一致，且带 wal 附属件。
        let backup_main = std::fs::read_dir(&master_dir)
            .unwrap()
            .filter_map(|entry| entry.ok())
            .find(|entry| {
                let name = entry.file_name().to_string_lossy().into_owned();
                name.starts_with("database.db.switch-bak-")
                    && !name.ends_with("-wal")
                    && !name.ends_with("-shm")
            })
            .expect("备份主文件应存在");
        assert_eq!(
            std::fs::read(backup_main.path()).unwrap(),
            b"master-db-bytes"
        );
        let backup_name = backup_main.file_name().to_string_lossy().into_owned();
        assert!(master_dir.join(format!("{backup_name}-wal")).is_file());
        // 备份先行不阻断删除主流程：三处现场照常清理。
        assert!(!instance_dir.exists());
    }

    /// P11-4③：备份失败一律拒绝执行（无备份不动数据），删除零发生。
    /// 预占未来数秒的同名备份位使 create_new 撞车，确定性触发备份失败。
    #[test]
    fn purge_rejects_when_master_backup_fails() {
        let root = tempfile::tempdir().unwrap();
        registered_account_with_id(root.path(), "profile-a", "1234567890");
        let instance_dir = root.path().join("trae-instances").join("profile-a");
        std::fs::create_dir_all(&instance_dir).unwrap();
        std::fs::write(instance_dir.join("storage.json"), b"{}").unwrap();
        let master_dir = root.path().join("master");
        std::fs::create_dir_all(&master_dir).unwrap();
        let master_db = master_dir.join("database.db");
        std::fs::write(&master_db, b"master-db-bytes").unwrap();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        for stamp in now - 2..=now + 5 {
            std::fs::write(
                master_dir.join(format!("database.db.switch-bak-{stamp}")),
                b"occupied",
            )
            .unwrap();
        }
        let appdata = tempfile::tempdir().unwrap();

        let error = purge_account_records_inner(
            &root.path().join("checkin"),
            root.path(),
            appdata.path(),
            &master_db,
            "profile-a",
            &[],
        )
        .unwrap_err();

        assert_eq!(error, "purge_backup_failed");
        // 守卫生效：实例目录原样保留（未发生任何删除）。
        assert!(instance_dir.join("storage.json").is_file());
    }
}

// ============================================================================
// G23 插件同步策略·切号预检分支（ADR-0026 决策 3）：被测纯函数
// switch_plugin_preview_from_lists 随域迁入本模块（P12-R3 自 lib.rs
// plugin_sync_policy_tests 随域纯移动）；传播分支留守 lib.rs。
// ============================================================================

#[cfg(test)]
mod switch_plugin_preview_tests {
    use super::*;

    /// 构造云端已装条目（预检测试夹具；与 lib.rs 传播测试夹具同源）。
    fn cloud_item(record_id: &str, market_id: Option<&str>, name: &str) -> CloudPluginItem {
        let mut item = CloudPluginItem {
            record_id: record_id.to_string(),
            marketplace_plugin_id: market_id.map(|id| id.to_string()),
            name: name.to_string(),
            display_name: name.to_string(),
            version: "1.0.0".to_string(),
            registry: "trae-remote-official".to_string(),
            builtin: false,
        };
        item.builtin = record_id.starts_with("builtin:");
        item
    }

    #[test]
    fn preview_pure_install_difference_is_silent_apply() {
        // ADR-0026 决策 3 纯新增分支：源多目标少 → install_names 非空、
        // remove_names 为空（前端不弹确认，静默应用）。
        let source = vec![
            cloud_item("r1", Some("uuid-a"), "插件A"),
            cloud_item("r2", Some("uuid-b"), "插件B"),
        ];
        let target = vec![cloud_item("r9", Some("uuid-a"), "插件A")];
        let preview = switch_plugin_preview_from_lists(&source, &target);
        assert!(!preview.aborted);
        assert_eq!(preview.source_count, 2);
        assert_eq!(preview.target_count, 1);
        assert_eq!(preview.install_names, vec!["插件B".to_string()]);
        // 无移除 → 无需确认。
        assert!(preview.remove_names.is_empty());
    }

    #[test]
    fn preview_with_removal_lists_names_for_confirmation() {
        // ADR-0026 决策 3 含移除分支：目标有源无 → remove_names 非空
        // （前端据此弹一次移除确认，列明插件名）。
        let source = vec![cloud_item("r1", Some("uuid-a"), "插件A")];
        let target = vec![
            cloud_item("r9", Some("uuid-a"), "插件A"),
            cloud_item("r8", Some("uuid-d"), "插件D"),
        ];
        let preview = switch_plugin_preview_from_lists(&source, &target);
        assert_eq!(preview.remove_names, vec!["插件D".to_string()]);
        assert!(preview.install_names.is_empty());
    }
}

#[cfg(test)]
mod p14_3_identity_tests {
    use super::{
        e2_error_allows_e1_fallback, switch_identity_via_e1_fallback,
        switch_login_identity_dual_path_with_ops, switch_master_account_at_with_identity_switch,
        PluginCloudSyncOutcome,
    };
    use std::path::Path;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use traesync_infrastructure::{
        archive_login_user_id, construct_auth_identity, environment_registry::secondary_data_dir,
        environment_registry::EnvironmentRegistry, environment_registry::MASTER_ENV_ID,
        master_database_path, AccountRecord, AccountRegistry, AuthIdentitySwitch, AuthSwitchError,
        ConstructAuthInput, UserInfoFull,
    };

    fn account_record(profile_id: &str, account_id: &str) -> AccountRecord {
        AccountRecord {
            profile_id: profile_id.to_string(),
            account_id: account_id.to_string(),
            screen_name: format!("用户{account_id}"),
            avatar_url: String::new(),
            device_id: format!("device-{profile_id}"),
            device_public_key: format!("pub-{profile_id}"),
            display_name: None,
            masked_mobile: String::new(),
            created_at_unix_seconds: 1,
            last_verified_at_unix_seconds: 1,
            device_created_at_unix_seconds: 0,
            auto_checkin_enabled: false,
            archived: false,
        }
    }

    fn identity_switch(from_user_id: &str, to_user_id: &str) -> AuthIdentitySwitch {
        AuthIdentitySwitch {
            from_user_id: from_user_id.to_string(),
            to_user_id: to_user_id.to_string(),
            to_account: to_user_id.to_string(),
        }
    }

    fn seed_identity(path: &Path, user_id: &str) {
        let info = UserInfoFull {
            user_id: user_id.to_string(),
            screen_name: format!("用户{user_id}"),
            ..Default::default()
        };
        let input = ConstructAuthInput {
            account_id: user_id,
            access_token: "test-token",
            refresh_token: "test-refresh",
            access_token_expires_at_unix_seconds: 4_000_000_000,
            refresh_token_expires_at_unix_seconds: 4_100_000_000,
            user_info: &info,
        };
        construct_auth_identity(&input, path).unwrap();
    }

    fn seed_logged_identity(path: &Path, user_id: &str) {
        seed_identity(path, user_id);
        let storage_path = path.join("User/globalStorage/storage.json");
        let mut value: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&storage_path).unwrap()).unwrap();
        value.as_object_mut().unwrap().insert(
            "iCubeAuthInfo://usertag".to_string(),
            serde_json::Value::String("fixture-usertag".to_string()),
        );
        std::fs::write(storage_path, serde_json::to_vec(&value).unwrap()).unwrap();
    }

    fn storage(path: &Path) -> String {
        std::fs::read_to_string(path.join("User/globalStorage/storage.json")).unwrap()
    }

    fn write_identity_then_error(
        target: &Path,
        user_id: &str,
        error: AuthSwitchError,
    ) -> Result<super::AuthIdentitySwitch, AuthSwitchError> {
        seed_identity(target, user_id);
        Err(error)
    }

    #[test]
    fn e2_write_failures_never_fall_back_to_archive_copy() {
        let root = tempfile::tempdir().unwrap();
        let donor = root.path().join("donor");
        let target = root.path().join("target");
        seed_identity(&donor, "account-b");
        seed_identity(&target, "account-a");
        let target_before = storage(&target);

        for error in [
            AuthSwitchError::VerifyFailed,
            AuthSwitchError::WriteOutcomeUnknown,
        ] {
            assert!(!e2_error_allows_e1_fallback(&error));
            assert_eq!(
                switch_identity_via_e1_fallback("account-b", &donor, &target, error),
                Err(error)
            );
            assert_eq!(storage(&target), target_before);
        }
    }

    #[test]
    fn e1_copy_rejects_donor_that_does_not_match_registered_account() {
        let root = tempfile::tempdir().unwrap();
        let donor = root.path().join("donor");
        let target = root.path().join("target");
        seed_identity(&donor, "account-c");
        seed_identity(&target, "account-a");
        let target_before = storage(&target);

        assert_eq!(
            switch_identity_via_e1_fallback(
                "account-b",
                &donor,
                &target,
                AuthSwitchError::DonorAuthUnavailable,
            ),
            Err(AuthSwitchError::DonorAuthUnavailable)
        );
        assert_eq!(storage(&target), target_before);
    }

    #[test]
    fn e1_copy_preserves_offline_fallback_for_matching_account() {
        let root = tempfile::tempdir().unwrap();
        let donor = root.path().join("donor");
        let target = root.path().join("target");
        seed_identity(&donor, "account-b");
        seed_identity(&target, "account-a");

        let switched = switch_identity_via_e1_fallback(
            "account-b",
            &donor,
            &target,
            AuthSwitchError::DonorAuthUnavailable,
        )
        .unwrap();
        assert_eq!(switched.to_user_id, "account-b");
    }

    /// P14-3：E2 身份与注册目标一致时成功直返，不执行 E1。
    #[test]
    fn e2_success_for_registered_identity_does_not_call_e1() {
        let record = account_record("profile-b", "account-b");
        let e1_calls = AtomicUsize::new(0);

        let result = switch_login_identity_dual_path_with_ops(
            &record,
            || Ok(identity_switch("account-a", "account-b")),
            |_| {
                e1_calls.fetch_add(1, Ordering::SeqCst);
                Err(AuthSwitchError::DonorAuthUnavailable)
            },
        )
        .unwrap();

        assert_eq!(result, identity_switch("account-a", "account-b"));
        assert_eq!(e1_calls.load(Ordering::SeqCst), 0);
    }

    /// P14-3：E2 返回与注册目标不一致的 userId 时失败关闭，不能改走 E1。
    #[test]
    fn e2_success_with_wrong_identity_is_rejected_without_e1() {
        let record = account_record("profile-b", "account-b");
        let e1_calls = AtomicUsize::new(0);

        let result = switch_login_identity_dual_path_with_ops(
            &record,
            || Ok(identity_switch("account-a", "account-c")),
            |_| {
                e1_calls.fetch_add(1, Ordering::SeqCst);
                Err(AuthSwitchError::DonorAuthUnavailable)
            },
        );

        assert_eq!(result, Err(AuthSwitchError::VerifyFailed));
        assert_eq!(e1_calls.load(Ordering::SeqCst), 0);
    }

    /// P14-3：目标已经是所选账号时返回恒等交换，不读取另一个登录供体。
    #[test]
    fn e2_same_account_returns_noop_identity_without_e1() {
        let record = account_record("profile-b", "account-b");
        let e1_calls = AtomicUsize::new(0);

        let result = switch_login_identity_dual_path_with_ops(
            &record,
            || Err(AuthSwitchError::SameAccount),
            |_| {
                e1_calls.fetch_add(1, Ordering::SeqCst);
                Err(AuthSwitchError::DonorAuthUnavailable)
            },
        )
        .unwrap();

        assert_eq!(result, identity_switch("account-b", "account-b"));
        assert_eq!(e1_calls.load(Ordering::SeqCst), 0);
    }

    /// P14-3：E1 发现切换期间目标已变成同账号时也收敛为恒等交换。
    #[test]
    fn e1_same_account_race_returns_noop_identity() {
        let record = account_record("profile-b", "account-b");

        let result = switch_login_identity_dual_path_with_ops(
            &record,
            || Err(AuthSwitchError::DonorAuthUnavailable),
            |_| Err(AuthSwitchError::SameAccount),
        )
        .unwrap();

        assert_eq!(result, identity_switch("account-b", "account-b"));
    }

    /// P14-3：注册目标缺失时在调用 E2/E1 前拒绝身份材料操作。
    #[test]
    fn empty_registered_identity_is_rejected_before_e2_or_e1() {
        let mut record = account_record("profile-empty", "account-b");
        record.account_id.clear();
        let e2_calls = AtomicUsize::new(0);
        let e1_calls = AtomicUsize::new(0);

        let result = switch_login_identity_dual_path_with_ops(
            &record,
            || {
                e2_calls.fetch_add(1, Ordering::SeqCst);
                Ok(identity_switch("account-a", "account-b"))
            },
            |_| {
                e1_calls.fetch_add(1, Ordering::SeqCst);
                Ok(identity_switch("account-a", "account-b"))
            },
        );

        assert_eq!(result, Err(AuthSwitchError::DonorAuthUnavailable));
        assert_eq!(e2_calls.load(Ordering::SeqCst), 0);
        assert_eq!(e1_calls.load(Ordering::SeqCst), 0);
    }

    /// E2 写入 B 后校验失败，编排必须拒绝执行会消费 C 的 E1。
    #[test]
    fn e2_post_write_verify_failure_does_not_consume_mismatched_e1_donor() {
        let root = tempfile::tempdir().unwrap();
        let donor = root.path().join("donor");
        let target = root.path().join("target");
        let record = account_record("profile-b", "account-b");
        seed_identity(&donor, "account-c");
        seed_identity(&target, "account-a");
        let e1_calls = AtomicUsize::new(0);

        let result = switch_login_identity_dual_path_with_ops(
            &record,
            || write_identity_then_error(&target, "account-b", AuthSwitchError::VerifyFailed),
            |error| {
                e1_calls.fetch_add(1, Ordering::SeqCst);
                switch_identity_via_e1_fallback(&record.account_id, &donor, &target, error)
            },
        );

        assert_eq!(result, Err(AuthSwitchError::VerifyFailed));
        assert_eq!(e1_calls.load(Ordering::SeqCst), 0);
        assert_eq!(archive_login_user_id(&target).as_deref(), Some("account-b"));
        assert_ne!(archive_login_user_id(&target).as_deref(), Some("account-c"));
    }

    /// 写入结果未知时，即使文件看起来已变更，也不得再执行 E1。
    #[test]
    fn e2_unknown_write_outcome_does_not_consume_mismatched_e1_donor() {
        let root = tempfile::tempdir().unwrap();
        let donor = root.path().join("donor");
        let target = root.path().join("target");
        let record = account_record("profile-b", "account-b");
        seed_identity(&donor, "account-c");
        seed_identity(&target, "account-a");
        let e1_calls = AtomicUsize::new(0);

        let result = switch_login_identity_dual_path_with_ops(
            &record,
            || {
                write_identity_then_error(
                    &target,
                    "account-b",
                    AuthSwitchError::WriteOutcomeUnknown,
                )
            },
            |error| {
                e1_calls.fetch_add(1, Ordering::SeqCst);
                switch_identity_via_e1_fallback(&record.account_id, &donor, &target, error)
            },
        );

        assert_eq!(result, Err(AuthSwitchError::WriteOutcomeUnknown));
        assert_eq!(e1_calls.load(Ordering::SeqCst), 0);
        assert_eq!(archive_login_user_id(&target).as_deref(), Some("account-b"));
        assert_ne!(archive_login_user_id(&target).as_deref(), Some("account-c"));
    }

    /// 环境登录真实编排在 E2 写后失败时回滚认证，不写档案或会话库归属。
    #[test]
    fn environment_login_caller_rolls_back_post_write_identity_failure() {
        let root = tempfile::tempdir().unwrap();
        let material_root = root.path().join("materials");
        let storage_root = root.path().join("storage");
        let recovery_root = root.path().join("recovery");
        std::fs::create_dir_all(&material_root).unwrap();
        std::fs::create_dir_all(&storage_root).unwrap();
        std::fs::create_dir_all(&recovery_root).unwrap();

        let target_record = account_record("profile-b", "account-b");
        AccountRegistry::new(&material_root)
            .upsert(&target_record)
            .unwrap();
        let donor_dir = traesync_infrastructure::trae_instance::instance_data_dir(
            &storage_root,
            &target_record.profile_id,
        )
        .unwrap();
        seed_identity(&donor_dir, "account-c");

        let registry = EnvironmentRegistry::new(&storage_root);
        let environment = registry.create("身份回滚测试").unwrap();
        registry
            .set_current_profile(&environment.env_id, "profile-a")
            .unwrap();
        let environment_dir = secondary_data_dir(&storage_root, &environment.env_id);
        seed_logged_identity(&environment_dir, "account-a");
        let database_path = master_database_path(&environment_dir);
        std::fs::create_dir_all(database_path.parent().unwrap()).unwrap();
        std::fs::write(&database_path, b"library-before").unwrap();
        let storage_before = storage(&environment_dir);
        let database_before = std::fs::read(&database_path).unwrap();
        let e1_calls = AtomicUsize::new(0);

        let switch_identity = |record: &AccountRecord, donor: &Path, target: &Path| {
            switch_login_identity_dual_path_with_ops(
                record,
                || write_identity_then_error(target, "account-b", AuthSwitchError::VerifyFailed),
                |error| {
                    e1_calls.fetch_add(1, Ordering::SeqCst);
                    switch_identity_via_e1_fallback(&record.account_id, donor, target, error)
                },
            )
            .map(|_| ())
        };
        let result = crate::environment::login_environment_inner_with_ops(
            &material_root,
            &storage_root,
            &recovery_root,
            "unused-test-key",
            &environment.env_id,
            &target_record.profile_id,
            &|_| Ok(()),
            &switch_identity,
            &|| {},
        );

        let error = match result {
            Err(error) => error,
            Ok(_) => panic!("E2 写后校验失败不应完成环境登录"),
        };
        assert_eq!(error, "environment_login_failed");
        assert_eq!(e1_calls.load(Ordering::SeqCst), 0);
        assert_eq!(storage(&environment_dir), storage_before);
        assert_eq!(
            archive_login_user_id(&environment_dir).as_deref(),
            Some("account-a")
        );
        assert_eq!(std::fs::read(&database_path).unwrap(), database_before);
        assert_eq!(
            registry
                .find(&environment.env_id)
                .unwrap()
                .unwrap()
                .current_profile_id
                .as_deref(),
            Some("profile-a")
        );
    }

    /// 主库切号真实编排在 E2 写入状态未知时还原认证，保留档案和数据库状态。
    #[test]
    fn master_switch_caller_rolls_back_post_write_identity_failure() {
        let root = tempfile::tempdir().unwrap();
        let material_root = root.path().join("materials");
        let storage_root = root.path().join("storage");
        let recovery_root = root.path().join("recovery");
        let master_dir = root.path().join("master");
        std::fs::create_dir_all(&material_root).unwrap();
        std::fs::create_dir_all(&storage_root).unwrap();
        std::fs::create_dir_all(&recovery_root).unwrap();
        std::fs::create_dir_all(&master_dir).unwrap();

        let target_record = account_record("profile-b", "account-b");
        AccountRegistry::new(&material_root)
            .upsert(&target_record)
            .unwrap();
        let donor_dir = traesync_infrastructure::trae_instance::instance_data_dir(
            &storage_root,
            &target_record.profile_id,
        )
        .unwrap();
        seed_identity(&donor_dir, "account-c");
        seed_logged_identity(&master_dir, "account-a");
        let registry = EnvironmentRegistry::new(&storage_root);
        registry.ensure_master().unwrap();
        registry
            .set_current_profile(MASTER_ENV_ID, "profile-a")
            .unwrap();
        let database_path = master_database_path(&master_dir);
        std::fs::create_dir_all(database_path.parent().unwrap()).unwrap();
        std::fs::write(&database_path, b"master-library-before").unwrap();
        let storage_before = storage(&master_dir);
        let database_before = std::fs::read(&database_path).unwrap();
        let e1_calls = AtomicUsize::new(0);
        let app = tauri::test::mock_builder()
            .build(tauri::generate_context!())
            .unwrap();
        let app_handle = app.handle().clone();

        let identity_switch = |record: &AccountRecord, donor: &Path, target: &Path| {
            switch_login_identity_dual_path_with_ops(
                record,
                || {
                    write_identity_then_error(
                        target,
                        "account-b",
                        AuthSwitchError::WriteOutcomeUnknown,
                    )
                },
                |error| {
                    e1_calls.fetch_add(1, Ordering::SeqCst);
                    switch_identity_via_e1_fallback(&record.account_id, donor, target, error)
                },
            )
        };
        let plugin_sync =
            |_from_user_id: &str, _target: &AccountRecord, _records: &[AccountRecord]| {
                // E2 故障注入在身份写入步即失败，对账不会触达；回执取 aborted 形态。
                PluginCloudSyncOutcome {
                    source_count: 0,
                    installed: 0,
                    removed: 0,
                    failed: 0,
                    skipped: 0,
                    aborted: true,
                    absorbed: Vec::new(),
                }
            };
        let result = switch_master_account_at_with_identity_switch(
            &app_handle,
            &material_root,
            &storage_root,
            &recovery_root,
            "unused-test-key",
            &target_record.profile_id,
            &master_dir,
            true,
            &|_| Ok(()),
            &|_, _| Ok("test-launched".to_string()),
            &|| {},
            &identity_switch,
            &plugin_sync,
        );

        let error = match result {
            Err(error) => error,
            Ok(_) => panic!("E2 写入状态未知不应完成主库切号"),
        };
        assert_eq!(error, "switch_auth_failed");
        assert_eq!(e1_calls.load(Ordering::SeqCst), 0);
        assert_eq!(storage(&master_dir), storage_before);
        assert_eq!(
            archive_login_user_id(&master_dir).as_deref(),
            Some("account-a")
        );
        assert_eq!(std::fs::read(&database_path).unwrap(), database_before);
        assert_eq!(
            registry
                .find(MASTER_ENV_ID)
                .unwrap()
                .unwrap()
                .current_profile_id
                .as_deref(),
            Some("profile-a")
        );
    }
}

#[cfg(test)]
mod p14_2_process_observation_tests {
    use super::{capture_current_account_credential_inner, try_apply_saved_credential};
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use traesync_application::{
        ProcessControllerPort, ProcessMatchEvidence, ProcessObservation, ProcessObservationStatus,
    };
    use traesync_domain::AuthorizationState;

    struct UnknownProcessController {
        calls: AtomicUsize,
        evidence_code: &'static str,
    }

    impl ProcessControllerPort for UnknownProcessController {
        fn observe(
            &self,
            data_location_id: &str,
            _data_root: &Path,
            _db_relative_path: &str,
        ) -> ProcessObservation {
            self.calls.fetch_add(1, Ordering::SeqCst);
            ProcessObservation {
                status: ProcessObservationStatus::Unknown,
                data_location_id: data_location_id.to_string(),
                candidates: Vec::new(),
                evidence: vec![ProcessMatchEvidence {
                    code: self.evidence_code.to_string(),
                    value: "fixture 查询失败".to_string(),
                }],
            }
        }
    }

    fn authorized_fixture_state(
        evidence_code: &'static str,
    ) -> (
        tempfile::TempDir,
        crate::AppState,
        Arc<UnknownProcessController>,
    ) {
        let local_appdata =
            std::env::var_os("LOCALAPPDATA").expect("测试需要 Windows LOCALAPPDATA 环境变量");
        let trusted_test_root = PathBuf::from(local_appdata).join("Trae Sync").join("tests");
        let isolated_root = tempfile::Builder::new()
            .prefix("p14-2-process-observation-")
            .tempdir_in(&trusted_test_root)
            .expect("在项目可信测试目录创建隔离 fixture");
        let fixture_root = isolated_root.path().join("fixture");
        let storage_root = fixture_root.join("storage");
        let database_relative_path = "User/globalStorage/state.vscdb";
        std::fs::create_dir_all(fixture_root.join("User/globalStorage")).unwrap();
        std::fs::create_dir_all(&storage_root).unwrap();
        std::fs::write(
            fixture_root.join(database_relative_path),
            b"fixture database",
        )
        .unwrap();

        let mut state = crate::test_support::make_fixture_mode_state(&storage_root);
        state.authorization.lock().unwrap().state = AuthorizationState::Authorized {
            canonical_fixture_root: fixture_root.to_string_lossy().into_owned(),
            db_relative_path: database_relative_path.to_string(),
        };
        let process_controller = Arc::new(UnknownProcessController {
            calls: AtomicUsize::new(0),
            evidence_code,
        });
        state.process_controller = process_controller.clone();
        (isolated_root, state, process_controller)
    }

    fn fixture_snapshot(root: &Path) -> Vec<(String, bool, Vec<u8>)> {
        fn visit(root: &Path, current: &Path, entries: &mut Vec<(String, bool, Vec<u8>)>) {
            for item in std::fs::read_dir(current).unwrap() {
                let item = item.unwrap();
                let path = item.path();
                let metadata = item.metadata().unwrap();
                let relative = path
                    .strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .into_owned();
                if metadata.is_dir() {
                    entries.push((relative, true, Vec::new()));
                    visit(root, &path, entries);
                } else {
                    entries.push((relative, false, std::fs::read(path).unwrap()));
                }
            }
        }

        let mut entries = Vec::new();
        visit(root, root, &mut entries);
        entries.sort_by(|left, right| left.0.cmp(&right.0));
        entries
    }

    fn assert_unknown_observation_does_not_write(
        run: impl FnOnce(&crate::AppState) -> Result<(), String>,
        evidence_code: &'static str,
    ) {
        let (isolated_root, state, process_controller) = authorized_fixture_state(evidence_code);
        let fixture_root = isolated_root.path().join("fixture");
        let before = fixture_snapshot(&fixture_root);

        assert_eq!(run(&state).unwrap_err(), "process_state_unknown");
        assert_eq!(process_controller.calls.load(Ordering::SeqCst), 1);
        assert_eq!(fixture_snapshot(&fixture_root), before);
    }

    /// P14-2：进程枚举状态未知时，凭据捕获必须在任何文件写入前返回。
    #[test]
    fn credential_capture_stops_on_unknown_process_without_file_writes() {
        for evidence_code in ["process_enumeration_failed", "target_database_query_failed"] {
            assert_unknown_observation_does_not_write(
                |state| capture_current_account_credential_inner(state).map(|_| ()),
                evidence_code,
            );
        }
    }

    /// P14-2：进程枚举状态未知时，凭据应用必须在任何文件写入前返回。
    #[test]
    fn saved_credential_apply_stops_on_unknown_process_without_file_writes() {
        for evidence_code in ["process_enumeration_failed", "target_database_query_failed"] {
            assert_unknown_observation_does_not_write(
                |state| try_apply_saved_credential(state, "profile-target", true).map(|_| ()),
                evidence_code,
            );
        }
    }
}

/// 真机切号探针（P15-5/P15-8 验收驱动，ADR-0035；`--features probe` 门控）。
///
/// 与生产命令同一编排：`switch_master_account_at` 的融合版身份路径
/// （`.fused` 容器 + 在线 GetUserInfo 复核）+ 真实关/启融合版客户端闭包；
/// 唯一注入点是事件发射走 mock AppHandle（无监听者，emit 静默）——
/// 业务编排、进程开关、数据库交接、网络复核全部真实。供
/// examples/fused_switch.rs 做 16 账号顺序切号与逐账号验收。
/// 生产构建不编译本模块（feature 未启用时零代码、零可见性变化）。
#[cfg(feature = "probe")]
pub mod real_switch_probe {
    // 子模块复用父模块的全部导入（close_instance/TraeProduct/编排入口等）。
    use super::*;
    use serde_json::{json, Value};

    /// 生产同源根目录推导（组合根 lib.rs / production_catalog 同规则：
    /// storage_root = `Trae Sync\data`、recovery_root = `Trae Sync\recovery`、
    /// material_root = `data\checkin`；均为固定派生，无落盘状态可复制）。
    fn production_roots() -> Result<(PathBuf, PathBuf, PathBuf), String> {
        let app_root = std::env::var_os("LOCALAPPDATA")
            .map(|base| PathBuf::from(base).join("Trae Sync"))
            .ok_or_else(|| "localappdata_unavailable".to_string())?;
        let storage_root = app_root.join("data");
        let recovery_root = app_root.join("recovery");
        let material_root = storage_root.join("checkin");
        if !material_root.is_dir() {
            return Err("checkin_registry_missing".to_string());
        }
        Ok((storage_root, recovery_root, material_root))
    }

    /// 账号清单 + 当前选择指针（验收驱动的枚举入口；不触碰任何凭据材料）。
    pub fn list_accounts() -> Result<Value, String> {
        let (storage_root, _recovery_root, material_root) = production_roots()?;
        let records = AccountRegistry::new(&material_root)
            .load()
            .map_err(|_| "checkin_registry_invalid".to_string())?;
        let current = ProductAccountStateStore::new(product_state_root_from(&storage_root))
            .load()
            .map_err(|_| "product_state_invalid".to_string())?;
        Ok(json!({
            "current_profile_id": current,
            "accounts": records
                .iter()
                .map(|record| {
                    json!({
                        "profile_id": record.profile_id,
                        "account_id": record.account_id,
                        "display_name": record
                            .display_name
                            .clone()
                            .unwrap_or_else(|| record.screen_name.clone()),
                    })
                })
                .collect::<Vec<_>>(),
        }))
    }

    /// 主库当前登录 blob 的账号身份（切号后离线验收证据）。dir 可传隔离
    /// 演练目录（读其 blob，不触碰真实主库）。
    pub fn blob_user_id_of(dir: &Path) -> Option<String> {
        master_login_material(dir).map(|material| material.user_id)
    }

    /// 工具存储根（隔离扫批的快照定位用）。
    pub fn storage_root() -> PathBuf {
        production_roots()
            .map(|(storage_root, _, _)| storage_root)
            .unwrap_or_default()
    }

    /// 主库当前登录身份对应的注册账号（隔离扫批的缺省收尾账号）。
    pub fn master_blob_profile_id() -> Option<String> {
        let master_dir = master_data_dir().ok()?;
        let user_id = master_login_material(&master_dir)?.user_id;
        let (_, _, material_root) = production_roots().ok()?;
        AccountRegistry::new(&material_root)
            .load()
            .ok()?
            .into_iter()
            .find(|record| record.account_id == user_id)
            .map(|record| record.profile_id)
    }

    /// 关闭指定目录的实例（close/launch 按 --user-data-dir 精确匹配，
    /// 只影响该目录自身实例，不触碰默认目录的主客户端）。
    pub fn close_instance_at(dir: &Path) -> Result<String, String> {
        close_instance(TraeProduct::TraeCn, dir)
            .map(|outcome| format!("{outcome:?}"))
            .map_err(|error| format!("{error:?}"))
    }

    /// 播种隔离演练目录：从真实主库只读复制 storage.json 与数据库三件套
    /// （副本落在隔离目录同构相对路径下；主库零写入、零加锁）。副本使
    /// 隔离实例首启即为「已初始化」状态，交接事务有真实 schema 可演练。
    pub fn seed_isolated_from_master(dir: &Path) -> Result<Vec<PathBuf>, String> {
        let master_dir =
            master_data_dir().map_err(|_| "environment_registry_invalid".to_string())?;
        let master_storage = master_dir.join("User").join("globalStorage").join("storage.json");
        if !master_storage.is_file() {
            return Err("master_storage_json_missing".to_string());
        }
        let master_db = master_database_path(&master_dir);
        if !master_db.is_file() {
            return Err("master_db_missing".to_string());
        }
        let mut copied = Vec::new();
        // storage.json（含 iCube 机器级密钥，供 construct_auth_identity 合并）。
        let isolated_storage = dir.join("User").join("globalStorage").join("storage.json");
        std::fs::create_dir_all(isolated_storage.parent().ok_or("bad_path")?)
            .map_err(|_| "seed_create_dir_failed".to_string())?;
        std::fs::copy(&master_storage, &isolated_storage)
            .map_err(|_| "seed_copy_storage_failed".to_string())?;
        copied.push(isolated_storage);
        // 数据库三件套（db / -wal / -shm；后两者存在才复制）。
        // 落位保持与 master_database_path 同构相对路径（ModularData\ai-agent\），
        // 编排按 master_database_path(master_dir) 定位隔离库，根目录直放会报
        // master_switch_db_missing（2026-10-13 首轮单账号演练实测）。
        std::fs::create_dir_all(dir).map_err(|_| "seed_create_dir_failed".to_string())?;
        let db_relative = master_db
            .strip_prefix(&master_dir)
            .map_err(|_| "bad_db_path".to_string())?;
        let isolated_db = dir.join(db_relative);
        std::fs::create_dir_all(isolated_db.parent().ok_or("bad_path")?)
            .map_err(|_| "seed_create_dir_failed".to_string())?;
        let db_name = master_db
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or("bad_db_name")?
            .to_string();
        std::fs::copy(&master_db, &isolated_db).map_err(|_| "seed_copy_db_failed".to_string())?;
        for suffix in ["-wal", "-shm"] {
            let sidecar = master_db.with_file_name(format!("{db_name}{suffix}"));
            if sidecar.is_file() {
                let target = isolated_db.with_file_name(format!("{db_name}{suffix}"));
                std::fs::copy(&sidecar, &target)
                    .map_err(|_| "seed_copy_sidecar_failed".to_string())?;
                copied.push(target);
            }
        }
        copied.push(isolated_db);
        Ok(copied)
    }

    /// 单账号真机切号（生产五步事务全量）：关实例 → 切出回存（第 0.5 步）
    /// → 三件套备份 → 容器 + 在线复核构造登录态 → 交接事务 → 重启。
    /// master_dir 可传隔离演练目录（五步事务全部作用其中，真实主库零写入；
    /// 插件对账注入 no-op——隔离演练不得触碰任何账号的云端插件清单）。
    /// 主库写入活动中（busy）时退避重试，两轮后仍忙才允许调用方指定的
    /// force 生效。返回 DTO 与切后 blob 身份的合并 JSON。
    pub fn switch_one_at(
        master_dir: &Path,
        profile_id: &str,
        force: bool,
    ) -> Result<Value, String> {
        let (storage_root, recovery_root, material_root) = production_roots()?;
        // 与 switch_master_account_inner 相同的真实进程闭包（TraeCn 产品；
        // close/launch 按 --user-data-dir 精确匹配目录，不误伤其他实例）。
        let close_master = |dir: &Path| close_instance(TraeProduct::TraeCn, dir).map(|_| ());
        let launch_master = |dir: &Path, title: &str| {
            launch_instance_common(TraeProduct::TraeCn, dir, title, None)
                .map(|relaunch| relaunch.outcome.to_string())
                .map_err(|error| error.to_string())
        };
        // 融合版身份路径（与生产 switch_master_account_at 同源）。
        let fused_root = storage_root.join("fused-login");
        let identity_switch = |record: &AccountRecord, _donor_dir: &Path, target_dir: &Path| {
            switch_login_identity_fused(&fused_root, record, target_dir)
        };
        // 隔离演练：插件对账 no-op（云端零操作）。
        let plugin_sync_noop = |_from_user_id: &str,
                                _target: &AccountRecord,
                                _records: &[AccountRecord]|
         -> PluginCloudSyncOutcome {
            PluginCloudSyncOutcome {
                source_count: 0,
                installed: 0,
                removed: 0,
                failed: 0,
                skipped: 0,
                aborted: true,
                absorbed: Vec::new(),
            }
        };
        // mock AppHandle 仅承接 emit（无监听者）；App 生命周期覆盖整个切号。
        let app = tauri::test::mock_builder()
            .build(tauri::generate_context!())
            .map_err(|error| format!("probe_app_build_failed:{error}"))?;
        let handle = app.handle().clone();
        let before_return = || {};
        let mut last_error = String::new();
        for attempt in 0..3u8 {
            let attempt_force = force || attempt >= 2;
            match switch_master_account_at_with_identity_switch(
                &handle,
                &material_root,
                &storage_root,
                &recovery_root,
                crate::WORK_CN_SOURCE_RAW_KEY,
                profile_id,
                master_dir,
                attempt_force,
                &close_master,
                &launch_master,
                &before_return,
                &identity_switch,
                &plugin_sync_noop,
            ) {
                Ok(dto) => {
                    return Ok(json!({
                        "switch": dto,
                        "blob_user_id_after": blob_user_id_of(master_dir),
                        "attempts": attempt + 1,
                    }));
                }
                Err(error) if error == "master_switch_busy" && attempt < 2 => {
                    // WAL 双采样检测到主库仍在写入：退避后重试（上一轮实例
                    // 收尾写入通常数秒内结束）。
                    eprintln!("[probe] master_switch_busy，3 秒后重试（第 {} 轮）", attempt + 2);
                    last_error = error;
                    std::thread::sleep(std::time::Duration::from_secs(3));
                }
                Err(error) => return Err(error),
            }
        }
        Err(last_error)
    }
}
