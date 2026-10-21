//! P17-T5 并列骨架：WorkBuddy 产品域命令 + 产品注册表/切换命令。
//!
//! 命令分组：
//! - 产品注册表与切换（`list_products`/`get_current_product`/
//!   `set_current_product`）：descriptor 驱动 + product-state 指针；
//! - WorkBuddy 会话健康（只读快照 + 登录态判定 + 容器在池标记）；
//! - WorkBuddy 账号池（workbuddy-login 容器脱敏清单）；
//! - WorkBuddy 直连签到（status/claim，幂等纪律在 infrastructure 层）；
//! - WorkBuddy 捕获铸造（**单次确认入口**：UI 弹窗确认后调用，命令即执行
//!   编排——写工具自有 workbuddy-login 容器，不触碰客户端数据）。
//!
//! 真实模式门禁：WorkBuddy 域命令仅 RealReadPreview 可用（fixture 是
//! Trae 测试专用形态，WorkBuddy 无 fixture 基建——错误码
//! `workbuddy_real_mode_required`）。

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Serialize;
use tauri::State;

use traesync_infrastructure::product_context::CurrentProductStore;
use traesync_infrastructure::product_descriptor::{descriptor_by_product_id, ALL_PRODUCTS};
use traesync_infrastructure::storage_roots::{
    product_state_root_from, workbuddy_login_root_from,
};
use traesync_infrastructure::workbuddy_capture::capture_workbuddy_login_from_machine;
use traesync_infrastructure::workbuddy_checkin::{
    open_session_from_machine, open_session_from_pool, open_session_from_pool_forced,
    renew_pool_credentials, verify_capture_renewal, WorkBuddyCheckinOutcome,
    WorkBuddyCheckinSession, WorkBuddyPoolError, WorkBuddyRenewalOutcome,
};
use traesync_infrastructure::workbuddy_credential::{
    current_session_status, load_session_snapshot, unix_ms_now, WorkBuddySessionStatus,
    WorkBuddySessionSnapshot, DEFAULT_AUTH_ID,
};
use traesync_infrastructure::workbuddy_login_credential::WorkBuddyLoginStore;
use traesync_infrastructure::workbuddy_wbipc::{
    parse_endpoint, verify_client_session_via_rpc, PipeTransport, WbipcTransport,
};

use crate::{ensure_login_real_mode, run_blocking, AppState};

/// 产品条目（产品管理页/切换器数据源；来自 descriptor 注册表）。
#[derive(Debug, Serialize)]
pub struct ProductEntryDto {
    pub product_id: &'static str,
    pub display_name: &'static str,
}

/// 当前产品（指针 + descriptor 展示名；未初始化缺省 trae_cn）。
#[derive(Debug, Serialize)]
pub struct CurrentProductDto {
    pub product_id: String,
    pub display_name: String,
}

/// WorkBuddy 会话健康（文件面，只读；不包含令牌材料）。
#[derive(Debug, Serialize)]
pub struct WorkBuddySessionHealthDto {
    /// Active / AccessExpired / FullyExpired / LoggedOut / FileMissing / Unknown。
    pub status: &'static str,
    pub uid: String,
    pub uin: String,
    pub domain: String,
    /// access 过期（Unix 秒；0 = 未知）。
    pub access_expires_at_unix_seconds: u64,
    /// refresh 过期（Unix 秒；0 = 未知）。
    pub refresh_expires_at_unix_seconds: u64,
    /// 该账号是否已捕获入池（workbuddy-login 容器存在）。
    pub captured_in_pool: bool,
}

/// 签到状态回执（脱敏；不含令牌）。
#[derive(Debug, Serialize)]
pub struct WorkBuddyCheckinStatusDto {
    pub today_checked_in: bool,
    pub streak_days: u32,
    pub daily_credit: u32,
    /// **本期签到累计**获得积分（活动域，只增不减——不是余额；P19.2③
    /// 语义改名。余额真实口径走 overview 的 credits_balance）。
    pub total_credits: u32,
}

/// 凭据续期回执（P17-T4c⑤；用户可感知语义与 Trae 刷新对齐）。
#[derive(Debug, Serialize)]
pub struct WorkBuddyRefreshStatusDto {
    /// 是否发生了实际刷新（false = access 仍充裕，未轮换）。
    pub renewed: bool,
    pub access_expires_at_unix_seconds: u64,
    pub refresh_expires_at_unix_seconds: u64,
}

/// 签到领取回执。
#[derive(Debug, Serialize)]
pub struct WorkBuddyCheckinClaimDto {
    pub credit: u32,
    pub streak_days: u32,
    /// 复核确认：领取后 status 查询的 today_checked_in（翻转 = 真实入账）。
    pub verified: bool,
}

/// 产品注册表（静态声明；新产品适配 = descriptor 追加一行）。
#[tauri::command]
pub(crate) fn list_products() -> Vec<ProductEntryDto> {
    ALL_PRODUCTS
        .iter()
        .map(|descriptor| ProductEntryDto {
            product_id: descriptor.product_id,
            display_name: descriptor.display_name,
        })
        .collect()
}

/// 当前产品指针（未初始化缺省 trae_cn——既有单产品用户兼容语义）。
#[tauri::command]
pub(crate) fn get_current_product(state: State<AppState>) -> Result<CurrentProductDto, String> {
    let product_id = current_product_id(&state)?;
    let display_name = descriptor_by_product_id(&product_id)
        .map(|descriptor| descriptor.display_name)
        .unwrap_or("未知产品");
    Ok(CurrentProductDto {
        product_id,
        display_name: display_name.to_string(),
    })
}

/// 切换当前产品（注册表校验 fail-closed；只写指针，不动任何凭据/账号数据）。
#[tauri::command]
pub(crate) fn set_current_product(
    product_id: String,
    state: State<AppState>,
) -> Result<(), String> {
    current_product_store(&state)?
        .set(&product_id)
        .map_err(|error| error.to_string())
}

/// WorkBuddy 会话健康（文件面只读；真实模式门禁）。
#[tauri::command]
pub(crate) fn get_workbuddy_session_health(
    state: State<AppState>,
) -> Result<WorkBuddySessionHealthDto, String> {
    ensure_login_real_mode_workbuddy(&state)?;
    let snapshot = load_session_snapshot(DEFAULT_AUTH_ID)
        .map_err(|error| error.code().to_string())?;
    let status = current_session_status(DEFAULT_AUTH_ID, unix_ms_now());
    Ok(session_health_dto(&snapshot, status, &state))
}

/// WorkBuddy 账号池（workbuddy-login 容器脱敏清单 + 档案字段；真实模式门禁）。
/// P19.2①：清单带档案旁表字段（display_name/archived）——**全量返回，
/// 归档过滤由前端执行**（后端不替前端裁剪，与参与集同哲学）。
#[tauri::command]
pub(crate) fn list_workbuddy_accounts(
    state: State<AppState>,
) -> Result<Vec<WorkBuddyAccountEntryDto>, String> {
    ensure_login_real_mode_workbuddy(&state)?;
    let login_root = workbuddy_login_root(&state)?;
    let store = WorkBuddyLoginStore::new(login_root.clone());
    let summaries = store
        .list()
        .map_err(|_| "workbuddy_pool_invalid".to_string())?;
    let profiles =
        traesync_infrastructure::workbuddy_account_profile::WorkBuddyAccountProfileStore::new(
            login_root,
        );
    Ok(summaries
        .iter()
        .map(|summary| workbuddy_account_entry(summary, &profiles))
        .collect())
}

/// WorkBuddy 账号条目（池摘要 + 档案旁表字段；不含任何令牌材料）。
#[derive(Debug, Serialize)]
pub struct WorkBuddyAccountEntryDto {
    // ---- WorkBuddyLoginSummary 平铺（既有消费面不破坏） ----
    pub profile_id: String,
    pub account_id: String,
    pub screen_name: String,
    pub domain: String,
    pub access_token_expires_at_unix_seconds: u64,
    pub refresh_token_expires_at_unix_seconds: u64,
    pub minted_at_unix_seconds: u64,
    pub last_updated_at_unix_seconds: u64,
    // ---- P19.2① 档案（pool 旁表） ----
    /// 用户备注名（空串 = 未设置；展示优先级：备注名 > 捕获昵称 > 兜底）。
    pub display_name: String,
    /// 归档标记（fe 据此过滤列表；默认 false = 在册）。
    pub archived: bool,
    /// P19.2② 脱敏手机号（服务端预脱敏形态如 `133****1304`；空串 = 未采集
    /// ——重捕获/OAuth 登录后补齐）。
    pub masked_phone: String,
}

/// 摘要 + 档案 → 条目 DTO（档案读取失败按缺省条目降级——展示面不阻塞）。
fn workbuddy_account_entry(
    summary: &traesync_infrastructure::workbuddy_login_credential::WorkBuddyLoginSummary,
    profiles: &traesync_infrastructure::workbuddy_account_profile::WorkBuddyAccountProfileStore,
) -> WorkBuddyAccountEntryDto {
    let profile = profiles.get(&summary.account_id).unwrap_or_default();
    WorkBuddyAccountEntryDto {
        profile_id: summary.profile_id.clone(),
        account_id: summary.account_id.clone(),
        screen_name: summary.screen_name.clone(),
        domain: summary.domain.clone(),
        access_token_expires_at_unix_seconds: summary.access_token_expires_at_unix_seconds,
        refresh_token_expires_at_unix_seconds: summary.refresh_token_expires_at_unix_seconds,
        minted_at_unix_seconds: summary.minted_at_unix_seconds,
        last_updated_at_unix_seconds: summary.last_updated_at_unix_seconds,
        display_name: profile.display_name,
        archived: profile.archived,
        masked_phone: summary.masked_mobile.clone(),
    }
}

/// 账号在池校验（清单面核对，防路径形态 account_id 逃逸）。
fn pool_contains(
    store: &WorkBuddyLoginStore,
    account_id: &str,
) -> Result<bool, String> {
    let profile_id =
        traesync_infrastructure::workbuddy_login_credential::WorkBuddyLoginRecord::profile_id_for(
            account_id,
        );
    Ok(store
        .list()
        .map_err(|_| "workbuddy_pool_invalid".to_string())?
        .iter()
        .any(|summary| summary.profile_id == profile_id))
}

/// 设置账号备注名（**档案旁表写入**——.info 无此字段〔nickname 为加密
/// 信封且语义不符〕，落工具侧账号档案存储，capture/switch 只动代次容器
/// 天然不破坏）。account_id 必须在池内。
#[tauri::command]
pub(crate) async fn set_workbuddy_account_display_name(
    account_id: String,
    display_name: String,
    state: State<'_, AppState>,
) -> Result<WorkBuddyAccountEntryDto, String> {
    ensure_login_real_mode_workbuddy(&state)?;
    let login_root = workbuddy_login_root(&state)?;
    run_blocking("workbuddy_account_profile_join_failed", move || {
        let store = WorkBuddyLoginStore::new(login_root.clone());
        if !pool_contains(&store, &account_id)? {
            return Err("workbuddy_pool_record_missing".to_string());
        }
        let profiles =
            traesync_infrastructure::workbuddy_account_profile::WorkBuddyAccountProfileStore::new(
                &login_root,
            );
        profiles
            .set_display_name(&account_id, &display_name, unix_ms_now() / 1000)
            .map_err(|error| error.code().to_string())?;
        account_entry_of(&login_root, &account_id)
    })
    .await
}

/// 设置账号归档标记（档案旁表；归档账号列表过滤由前端执行）。account_id
/// 必须在池内。
#[tauri::command]
pub(crate) async fn set_workbuddy_account_archived(
    account_id: String,
    archived: bool,
    state: State<'_, AppState>,
) -> Result<WorkBuddyAccountEntryDto, String> {
    ensure_login_real_mode_workbuddy(&state)?;
    let login_root = workbuddy_login_root(&state)?;
    run_blocking("workbuddy_account_profile_join_failed", move || {
        let store = WorkBuddyLoginStore::new(login_root.clone());
        if !pool_contains(&store, &account_id)? {
            return Err("workbuddy_pool_record_missing".to_string());
        }
        let profiles =
            traesync_infrastructure::workbuddy_account_profile::WorkBuddyAccountProfileStore::new(
                &login_root,
            );
        profiles
            .set_archived(&account_id, archived, unix_ms_now() / 1000)
            .map_err(|error| error.code().to_string())?;
        account_entry_of(&login_root, &account_id)
    })
    .await
}

/// 池摘要 + 档案 → 条目 DTO（账号定位失败 = 池记录缺失）。
fn account_entry_of(login_root: &Path, account_id: &str) -> Result<WorkBuddyAccountEntryDto, String> {
    let store = WorkBuddyLoginStore::new(login_root.to_path_buf());
    let profile_id =
        traesync_infrastructure::workbuddy_login_credential::WorkBuddyLoginRecord::profile_id_for(
            account_id,
        );
    let summary = store
        .list()
        .map_err(|_| "workbuddy_pool_invalid".to_string())?
        .into_iter()
        .find(|summary| summary.profile_id == profile_id)
        .ok_or_else(|| "workbuddy_pool_record_missing".to_string())?;
    let profiles =
        traesync_infrastructure::workbuddy_account_profile::WorkBuddyAccountProfileStore::new(
            login_root,
        );
    Ok(workbuddy_account_entry(&summary, &profiles))
}

/// 账号彻底删除回执（P19.2①；脱敏——只报计数与缺失清单）。
#[derive(Debug, Serialize)]
pub struct WorkBuddyAccountsPurgeDto {
    pub confirmed: bool,
    /// 实际删除的池记录数（含其档案旁表条目）。
    pub purged_accounts: usize,
    /// 请求中不在池内的 account_id（未删除，如实回执）。
    pub missing_accounts: Vec<String>,
    /// 档案旁表清理失败的 account_id（池记录已删；残留无害，幂等可重试）。
    pub profile_cleanup_failed: Vec<String>,
    /// 删除后池内剩余账号数。
    pub remaining_accounts: usize,
}

/// 账号彻底删除（**单次确认入口**；命名对齐库域 purge_workbuddy_sessions）。
///
/// 边界（写清）：删除范围 = 工具侧池记录（`{root}/wb-{uid}.wbsess` 代次
/// 容器）+ 档案旁表条目（备注名/归档）；**不触碰客户端 `.info` 会话文件、
/// 不做客户端登出**——客户端登录态独立存续，需清除登录态由用户在客户端
/// 操作。删除后该账号不可再被切号/直连签到选中（池为空时签到回退机器
/// 解密路径）。
#[tauri::command]
pub(crate) async fn purge_workbuddy_accounts(
    account_ids: Vec<String>,
    confirmed: bool,
    state: State<'_, AppState>,
) -> Result<WorkBuddyAccountsPurgeDto, String> {
    ensure_login_real_mode_workbuddy(&state)?;
    if !confirmed {
        return Err("workbuddy_purge_confirmation_required".to_string());
    }
    let login_root = workbuddy_login_root(&state)?;
    run_blocking("workbuddy_account_purge_join_failed", move || {
        // 去重保序（重复条目只删一次，回执不重复计数）。
        let mut seen = std::collections::HashSet::new();
        let ids: Vec<String> = account_ids
            .into_iter()
            .filter(|id| seen.insert(id.clone()))
            .collect();
        let store = WorkBuddyLoginStore::new(login_root.clone());
        let summaries = store
            .list()
            .map_err(|_| "workbuddy_pool_invalid".to_string())?;
        let mut purged = Vec::new();
        let mut missing_accounts = Vec::new();
        for account_id in &ids {
            let profile_id =
                traesync_infrastructure::workbuddy_login_credential::WorkBuddyLoginRecord::profile_id_for(account_id);
            if summaries.iter().any(|summary| summary.profile_id == profile_id) {
                store
                    .remove(&profile_id)
                    .map_err(|_| "workbuddy_pool_store_io".to_string())?;
                purged.push(account_id.clone());
            } else {
                missing_accounts.push(account_id.clone());
            }
        }
        // 档案旁表清理（失败不回滚池删除——池记录已删、残留无害，幂等可重试）。
        let profiles =
            traesync_infrastructure::workbuddy_account_profile::WorkBuddyAccountProfileStore::new(
                &login_root,
            );
        let profile_cleanup_failed = if profiles.remove(&purged).is_ok() {
            Vec::new()
        } else {
            purged.clone()
        };
        let remaining_accounts = store
            .list()
            .map_err(|_| "workbuddy_pool_invalid".to_string())?
            .len();
        Ok(WorkBuddyAccountsPurgeDto {
            confirmed: true,
            purged_accounts: purged.len(),
            missing_accounts,
            profile_cleanup_failed,
            remaining_accounts,
        })
    })
    .await
}

/// WorkBuddy 续期验证探针回执（P19.1④；回答用户核心质疑「保存当前

/// WorkBuddy 签到状态查询（只读；解密借客户端运行时，客户端可关可开）。
/// P19.1 401 兜底：池路径 AuthMismatch 时强制刷新换新对重试一次
/// （机器解密路径无池凭据，不重试）。
#[tauri::command]
pub(crate) async fn workbuddy_checkin_status(state: State<'_, AppState>) -> Result<WorkBuddyCheckinStatusDto, String> {
    ensure_login_real_mode_workbuddy(&state)?;
    let login_root = workbuddy_login_root(&state).ok();
    run_blocking("workbuddy_checkin_join_failed", move || {
        let (session, from_pool) =
            open_checkin_session_any(login_root.as_deref(), &workbuddy_client_exe()?)?;
        let read_status = |session: &WorkBuddyCheckinSession| -> Result<WorkBuddyCheckinStatusDto, String> {
            match session.status() {
                Ok(WorkBuddyCheckinOutcome::Status(status)) => Ok(WorkBuddyCheckinStatusDto {
                    today_checked_in: status.today_checked_in,
                    streak_days: status.streak_days,
                    daily_credit: status.daily_credit,
                    total_credits: status.total_credits,
                }),
                Ok(WorkBuddyCheckinOutcome::AuthMismatch) => {
                    Err("workbuddy_checkin_auth_mismatch".to_string())
                }
                Ok(other) => Err(format!("workbuddy_checkin_unexpected_response:{other:?}")),
                Err(error) => Err(error.code().to_string()),
            }
        };
        match read_status(&session) {
            Err(code) if code == "workbuddy_checkin_auth_mismatch" && from_pool => {
                // P19.1：强制刷新（忽略阈值）换新对后重试一次。
                let (session, _renewal) =
                    open_session_from_pool_forced(login_root.as_deref().expect("from_pool 必有根"), unix_ms_now() / 1000)
                        .map_err(|error| error.code().to_string())?;
                read_status(&session)
            }
            other => other,
        }
    })
    .await
}

/// WorkBuddy 签到领取（幂等；编排 = 查询未签才领 + 复核翻转确认）。
/// 已签时直接返回 verified=true（today_checked_in 复核口径）。
/// P19.1 401 兜底：池路径 AuthMismatch 时强制刷新换新对重试一次。
#[tauri::command]
pub(crate) async fn workbuddy_checkin_claim(state: State<'_, AppState>) -> Result<WorkBuddyCheckinClaimDto, String> {
    ensure_login_real_mode_workbuddy(&state)?;
    let login_root = workbuddy_login_root(&state).ok();
    run_blocking("workbuddy_checkin_join_failed", move || {
        let (session, from_pool) =
            open_checkin_session_any(login_root.as_deref(), &workbuddy_client_exe()?)?;
        let claim_with_session = |session: &WorkBuddyCheckinSession| -> Result<WorkBuddyCheckinClaimDto, String> {
            // 先查后领（写操作纪律：已签不重复发请求）。
            let before = match session.status() {
                Ok(WorkBuddyCheckinOutcome::Status(status)) => status,
                Ok(WorkBuddyCheckinOutcome::AuthMismatch) => {
                    return Err("workbuddy_checkin_auth_mismatch".to_string())
                }
                Ok(other) => return Err(format!("workbuddy_checkin_unexpected_response:{other:?}")),
                Err(error) => return Err(error.code().to_string()),
            };
            if before.today_checked_in {
                return Ok(WorkBuddyCheckinClaimDto {
                    credit: 0,
                    streak_days: before.streak_days,
                    verified: true,
                });
            }
            let claim = match session.claim() {
                Ok(WorkBuddyCheckinOutcome::Claimed(claim)) => claim,
                Ok(WorkBuddyCheckinOutcome::AlreadyCheckedIn) => {
                    // 并发窗口（查询后他端已签）：幂等空操作，复核口径确认。
                    return Ok(WorkBuddyCheckinClaimDto {
                        credit: 0,
                        streak_days: before.streak_days,
                        verified: true,
                    });
                }
                Ok(WorkBuddyCheckinOutcome::AuthMismatch) => {
                    return Err("workbuddy_checkin_auth_mismatch".to_string())
                }
                Ok(other) => return Err(format!("workbuddy_checkin_rejected:{other:?}")),
                Err(error) => return Err(error.code().to_string()),
            };
            // 复核：today_checked_in 翻转才算真实入账。
            let verified = match session.status() {
                Ok(WorkBuddyCheckinOutcome::Status(status)) => status.today_checked_in,
                _ => false,
            };
            if !verified {
                return Err("workbuddy_checkin_unverified".to_string());
            }
            Ok(WorkBuddyCheckinClaimDto {
                credit: claim.credit,
                streak_days: claim.streak_days,
                verified: true,
            })
        };
        match claim_with_session(&session) {
            Err(code) if code == "workbuddy_checkin_auth_mismatch" && from_pool => {
                // P19.1：强制刷新（忽略阈值）换新对后重试一次。
                let (session, _renewal) =
                    open_session_from_pool_forced(login_root.as_deref().expect("from_pool 必有根"), unix_ms_now() / 1000)
                        .map_err(|error| error.code().to_string())?;
                claim_with_session(&session)
            }
            other => other,
        }
    })
    .await
}

/// 签到会话装配（P17-T4c⑤ 池优先）：池内凭据优先（access 充裕零轮换、
/// 临近过期自动续期写回），池为空或不可用时回退机器解密（.info 现解密，
/// 覆盖未捕获账号）。池错误仅在机器回退同样失败时优先暴露——refresh
/// 拒绝（池 refresh 已轮换作废）引导重新捕获，比机器解密错误更有引导性。
/// 返回 (会话, 是否池路径)——P19.1 401 兜底只对池路径重试（机器路径
/// 无池凭据可刷）。
fn open_checkin_session_any(
    login_root: Option<&Path>,
    client_exe: &Path,
) -> Result<(WorkBuddyCheckinSession, bool), String> {
    let now = unix_ms_now() / 1000;
    let pool_error = if let Some(root) = login_root {
        match open_session_from_pool(root, now) {
            Ok((session, _renewal)) => return Ok((session, true)),
            Err(error) => Some(error),
        }
    } else {
        None
    };
    let session = open_session_from_machine(client_exe, DEFAULT_AUTH_ID).map_err(|machine_error| {
        match pool_error {
            Some(error @ (WorkBuddyPoolError::RefreshRejected | WorkBuddyPoolError::RecordMissing)) => {
                error.code().to_string()
            }
            _ => machine_error,
        }
    })?;
    Ok((session, false))
}

/// WorkBuddy 凭据手动续期（对齐 Trae「刷新登录凭据」入口语义）：
/// access 充裕时跳过（不轮换——refreshToken 一次性，最低轮换频率纪律），
/// 临近过期/已过期用池内 refreshToken 刷新并原子写回新对。
#[tauri::command]
pub(crate) async fn refresh_workbuddy_account(
    state: State<'_, AppState>,
) -> Result<WorkBuddyRefreshStatusDto, String> {
    ensure_login_real_mode_workbuddy(&state)?;
    let login_root = workbuddy_login_root(&state)?;
    run_blocking("workbuddy_refresh_join_failed", move || {
        let now = unix_ms_now() / 1000;
        let outcome = renew_pool_credentials(&login_root, now)
            .map_err(|error| error.code().to_string())?;
        match outcome {
            WorkBuddyRenewalOutcome::Skipped { .. } => {
                // 跳过时回读容器现值，给前端完整的有效期展示。
                use traesync_infrastructure::workbuddy_login_credential::WorkBuddyLoginStore;
                let store = WorkBuddyLoginStore::new(login_root);
                let summaries = store.list().map_err(|_| "workbuddy_pool_invalid".to_string())?;
                let summary = summaries.first().ok_or("workbuddy_pool_empty".to_string())?;
                let record = store
                    .load(&summary.profile_id, &summary.account_id)
                    .map_err(|_| "workbuddy_pool_record_missing".to_string())?;
                Ok(WorkBuddyRefreshStatusDto {
                    renewed: false,
                    access_expires_at_unix_seconds: record.access_token_expires_at_unix_seconds,
                    refresh_expires_at_unix_seconds: record.refresh_token_expires_at_unix_seconds,
                })
            }
            WorkBuddyRenewalOutcome::Renewed {
                access_expires_at_unix_seconds,
                refresh_expires_at_unix_seconds,
            } => Ok(WorkBuddyRefreshStatusDto {
                renewed: true,
                access_expires_at_unix_seconds,
                refresh_expires_at_unix_seconds,
            }),
        }
    })
    .await
}

/// WorkBuddy 账号捕获铸造（**单次确认入口**：UI 弹窗确认后调用）。
/// 编排 = 会话 Active 校验 → 品牌身份 → 解密 → uid 交叉校验 → 容器落盘
/// （写工具自有 workbuddy-login 根；生产会话文件只读）。幂等可重跑：
/// 同账号再次捕获 = 整代次原子替换为最新。
#[tauri::command]
pub(crate) async fn capture_workbuddy_account(
    state: State<'_, AppState>,
) -> Result<traesync_infrastructure::workbuddy_login_credential::WorkBuddyLoginSummary, String> {
    ensure_login_real_mode_workbuddy(&state)?;
    let login_root = workbuddy_login_root(&state)?;
    run_blocking("workbuddy_capture_join_failed", move || {
        let client_exe = workbuddy_client_exe()?;
        let store = WorkBuddyLoginStore::new(login_root);
        let record = capture_workbuddy_login_from_machine(&client_exe, &store, unix_ms_now())
            .map_err(|error| format!("{}:{:?}", error.code(), error))?;
        Ok(record.summary())
    })
    .await
}

/// WorkBuddy 续期验证探针回执（P19.1④；回答用户核心质疑「保存当前
/// 登录账号真的能持续更新吗」）。
#[derive(Debug, Serialize)]
pub struct WorkBuddyRenewalVerifyDto {
    /// 当前登录账号 uid（快照明文面）。
    pub uid: String,
    /// .info 快照时间戳（Unix 秒；0 = 未知）。
    pub info_last_refresh_unix_seconds: u64,
    pub info_access_expires_unix_seconds: u64,
    pub info_refresh_expires_unix_seconds: u64,
    /// .info 文件修改时刻（Unix 秒；0 = 不可得）。
    pub info_modified_unix_seconds: u64,
    /// 续期链路验证结果：true = 实调刷新端点成功且新对已原子写回池容器。
    pub renewed: bool,
    /// 刷新后（或现有）有效期（Unix 秒）。
    pub access_expires_at_unix_seconds: u64,
    pub refresh_expires_at_unix_seconds: u64,
}

/// WorkBuddy 续期验证探针（**非破坏**：只读 .info 与池容器；写仅限池
/// 容器自身的刷新轮换原子写回，生产会话文件零触碰）。
///
/// 编排 = 读凭据快照时间戳 → 按当前登录账号定位池记录 → 实调
/// `POST /v2/plugin/auth/token/refresh`（强制刷新，忽略阈值）验证续期
/// 链路 → 回执成功与否/新过期时间。信封检测分支：当前登录账号 token
/// 为 `$wbEncrypted` 加密信封且无明文池凭据时诚实报
/// `workbuddy_verify_envelope_encrypted`（登录态已加密，请用 OAuth 添加
/// 账号）；无池记录（非信封）报 `workbuddy_verify_credential_mismatch`；
/// 刷新被拒报 `workbuddy_verify_refresh_rejected`。
#[tauri::command]
pub(crate) async fn verify_workbuddy_capture_renewal(
    state: State<'_, AppState>,
) -> Result<WorkBuddyRenewalVerifyDto, String> {
    ensure_login_real_mode_workbuddy(&state)?;
    let login_root = workbuddy_login_root(&state)?;
    run_blocking("workbuddy_verify_join_failed", move || {
        use traesync_infrastructure::workbuddy_checkin::force_renew_pool_profile;
        let store = WorkBuddyLoginStore::new(login_root.clone());
        let load_snapshot = || {
            traesync_infrastructure::workbuddy_credential::load_session_snapshot(DEFAULT_AUTH_ID)
        };
        let info_modified_unix = || -> Option<u64> {
            let path = traesync_infrastructure::workbuddy_credential::session_file(DEFAULT_AUTH_ID)?;
            let modified = std::fs::metadata(&path).ok()?.modified().ok()?;
            modified
                .duration_since(std::time::UNIX_EPOCH)
                .ok()
                .map(|duration| duration.as_secs())
        };
        let now = unix_ms_now() / 1000;
        let renew = |profile_id: &str| force_renew_pool_profile(&login_root, profile_id, now);
        let report = verify_capture_renewal(&store, &load_snapshot, &info_modified_unix, &renew)
            .map_err(|error| error.code())?;
        Ok(WorkBuddyRenewalVerifyDto {
            uid: report.uid,
            info_last_refresh_unix_seconds: report.info_last_refresh_unix_seconds,
            info_access_expires_unix_seconds: report.info_access_expires_unix_seconds,
            info_refresh_expires_unix_seconds: report.info_refresh_expires_unix_seconds,
            info_modified_unix_seconds: report.info_modified_unix_seconds,
            renewed: report.renewed,
            access_expires_at_unix_seconds: report.access_expires_at_unix_seconds,
            refresh_expires_at_unix_seconds: report.refresh_expires_at_unix_seconds,
        })
    })
    .await
}

/// WorkBuddy 切号回执（P19.1②：切号 + 自动过户编排；脱敏——账号一律
/// 展示名标签，uid 不进主视野）。
#[derive(Debug, Serialize)]
pub struct WorkBuddySwitchDto {
    /// 切号是否成功（写凭据 + 验证通过；失败时本命令整体 Err）。
    pub switched: bool,
    /// 切换到的账号展示名。
    pub target_label: String,
    /// 切号前账号展示名（过户方向起点；防御同账号时 None）。
    pub previous_label: Option<String>,
    /// 过户结果：true = 执行了事务过户；false = 待过户为 0（无需关库）；
    /// None = 过户未执行（前置失败——报因看 transfer_error）。
    pub transfer_executed: Option<bool>,
    pub updated_rows: Option<u64>,
    pub verified_sessions: Option<u64>,
    pub missing_body_files: Option<u64>,
    /// 切号前账号名下 cloud 会话数（红线：不碰；>0 UI 明示）。
    pub cloud_residual_rows: Option<u64>,
    /// 过户失败报因（稳定错误码；None = 成功或无需过户）。
    /// **过户失败不回滚切号**（会话归属可后补）。
    pub transfer_error: Option<String>,
    /// 过户后是否发起了冷启动（重启客户端读新凭据与新库态）。
    pub restarted: bool,
    /// 冷启动后文件面登录态（active/access_expired/...；best-effort
    /// 强验证，失败不否定切号）。
    pub post_status: Option<String>,
}

/// 切号影响面预览回执（对齐 Trae preview_master_switch 形态；只读零副作用，
/// db 缺失/读取失败降级 None——预览不阻塞切号入口）。
#[derive(Debug, Serialize)]
pub struct WorkBuddySwitchPreviewDto {
    /// 当前登录账号展示名（预览基准）。
    pub current_label: String,
    /// 目标账号展示名（None = 仅账号清单，未选定目标）。
    pub target_label: Option<String>,
    /// 切到目标后将被转移的本地会话数（含软删行）。
    pub pending_local_rows: Option<u64>,
    /// 待过户会话涉及的项目数（cwd 维度 distinct）。
    pub pending_project_count: Option<u64>,
    /// 目标账号现有会话数。
    pub current_account_rows: Option<u64>,
    /// 当前账号名下 cloud 会话数（红线：不碰；>0 UI 明示）。
    pub cloud_residual_rows: Option<u64>,
}

/// WorkBuddy 切号（**单次确认入口**；P19.1② 自动过户编排生产装配）。
///
/// 编排 = 读切号前 .info uid → 写目标凭据（ToolEnvelope AES-256-GCM 信封，
/// 取自账号池；前置进程纪律 observe/close）→ 验证 → 显式方向自动过户
/// （含软删行；cloud 红线不碰）→ 冷启动 → 强验证。过户失败不回滚切号
/// （失败如实报因在回执 transfer_error）。
#[tauri::command]
pub(crate) async fn switch_workbuddy_account(
    target_account_id: String,
    confirmed: bool,
    state: State<'_, AppState>,
) -> Result<WorkBuddySwitchDto, String> {
    ensure_login_real_mode_workbuddy(&state)?;
    if !confirmed {
        return Err("workbuddy_switch_confirmation_required".to_string());
    }
    let login_root = workbuddy_login_root(&state)?;
    run_blocking("workbuddy_switch_join_failed", move || {
        switch_workbuddy_account_blocking(&login_root, &target_account_id)
    })
    .await
}

/// 切号编排生产装配（阻塞闭包体；注入式内核见
/// `infrastructure::workbuddy_switch_flow::switch_with_auto_transfer`）。
fn switch_workbuddy_account_blocking(
    login_root: &Path,
    target_account_id: &str,
) -> Result<WorkBuddySwitchDto, String> {
    use traesync_infrastructure::workbuddy_credential as credential;
    use traesync_infrastructure::workbuddy_switch_experiment as experiment;
    use traesync_infrastructure::workbuddy_switch_flow::{
        switch_with_auto_transfer, SwitchFlowDeps, SwitchFlowError, TransferFlowResult,
    };
    use traesync_infrastructure::workbuddy_transfer as transfer;

    // 0) 目标账号必须在池（切号凭据来源；uid = pool account_id）。
    let store = WorkBuddyLoginStore::new(login_root.to_path_buf());
    let target_profile = traesync_infrastructure::workbuddy_login_credential::WorkBuddyLoginRecord::profile_id_for(target_account_id);
    let target_record = store
        .load(&target_profile, target_account_id)
        .map_err(|_| SwitchFlowError::TargetNotInPool.code().to_string())?;

    // 数据根（会话过户 db/projects）；安装 exe 探测（冷启动用，缺省跳过）。
    let data_root = transfer::default_data_root()
        .ok_or_else(|| "workbuddy_data_root_unavailable".to_string())?;
    let client_exe = transfer::discover_install().map(|found| found.exe);

    // 1) 读切号前 .info uid（快照解析；错误映射进实验域——.info 缺失/损坏）。
    let read_current_uid = || -> Result<String, SwitchFlowError> {
        let snapshot = credential::load_session_snapshot(DEFAULT_AUTH_ID).map_err(|error| {
            SwitchFlowError::Experiment(match error {
                credential::WorkBuddySessionError::FileMissing => {
                    experiment::ExperimentError::InfoMissing
                }
                _ => experiment::ExperimentError::InfoMalformed,
            })
        })?;
        Ok(snapshot.uid)
    };

    // 2) 切号实验（写 .info 目标凭据；**不含冷启动**——延后到过户后统一执行）。
    let run_experiment = || -> Result<experiment::ExperimentReceipt, experiment::ExperimentError> {
        let auth_dir = credential::auth_dir().ok_or(experiment::ExperimentError::InfoMissing)?;
        let exe = client_exe.as_ref().ok_or_else(|| {
            experiment::ExperimentError::KeyFetchFailed(
                "workbuddy_decrypt_client_exe_missing".to_string(),
            )
        })?;
        let key = experiment::fetch_derived_key(exe)?;
        let target = experiment::TargetTokens {
            access_token: target_record.access_token.clone(),
            refresh_token: target_record.refresh_token.clone(),
            access_expires_at_ms: target_record.access_token_expires_at_unix_seconds * 1000,
            refresh_expires_at_ms: target_record.refresh_token_expires_at_unix_seconds * 1000,
            session_state: target_record.session_state.clone(),
        };
        let observe = || transfer::observe_client_real();
        let close = || transfer::close_client_real();
        let deps = experiment::ExperimentDeps {
            observe: &observe,
            close: &close,
            start: None,
            restart_settle: Duration::ZERO,
            now_ms: &credential::unix_ms_now,
        };
        experiment::run_switch_experiment(
            &auth_dir,
            DEFAULT_AUTH_ID,
            &target,
            experiment::WriteMode::ToolEnvelope,
            Some(&key),
            &deps,
        )
    };

    // 3) 切号验证（.info 当前账号 == 目标）。
    let verify_switch = || -> Result<String, SwitchFlowError> {
        credential::load_session_snapshot(DEFAULT_AUTH_ID)
            .map(|snapshot| snapshot.uid)
            .map_err(|_| SwitchFlowError::Experiment(experiment::ExperimentError::InfoMalformed))
    };

    // 4) 自动过户（显式方向 current=目标、previous=切号前；零行转 Nothing）。
    let run_transfer = |current: &str, previous: &str| {
        let db = transfer::db_path(&data_root);
        let plan = transfer::build_transfer_plan(&db, current, previous)?;
        if plan.pending_local_rows == 0 {
            return Ok(TransferFlowResult::Nothing {
                current_sessions: plan.current_account_rows,
            });
        }
        let receipt = transfer::transfer_sessions(
            &db,
            &transfer::projects_path(&data_root),
            current,
            previous,
        )?;
        Ok(TransferFlowResult::Executed(receipt))
    };

    // 5) 过户前置进程纪律 + 冷启动 + 强验证（真实进程面）。
    let observe = || transfer::observe_client_real();
    let close = || transfer::close_client_real();
    let start = || {
        client_exe
            .as_ref()
            .map(|exe| transfer::start_client_real(exe))
            .unwrap_or(false)
    };
    let post_verify = || -> Option<String> {
        let snapshot = credential::load_session_snapshot(DEFAULT_AUTH_ID).ok()?;
        let marker = credential::logout_marker_file(DEFAULT_AUTH_ID)?
            .exists();
        let status = credential::session_status_with(
            Some(&snapshot),
            marker,
            credential::unix_ms_now(),
        );
        Some(session_status_str(status).to_string())
    };
    let deps = SwitchFlowDeps {
        read_current_uid: &read_current_uid,
        run_experiment: &run_experiment,
        verify_switch: &verify_switch,
        run_transfer: &run_transfer,
        observe: &observe,
        close: &close,
        start: Some(&start),
        post_verify: Some(&post_verify),
    };
    let outcome = switch_with_auto_transfer(target_account_id, &deps)
        .map_err(|error| error.code().to_string())?;

    // 6) 回执组装（脱敏：uid → 展示名标签）。
    Ok(WorkBuddySwitchDto {
        switched: outcome.switched,
        target_label: workbuddy_account_label(
            &outcome.target_uid,
            Some(login_root),
            "目标账号",
        ),
        previous_label: outcome.previous_uid.as_deref().map(|uid| {
            workbuddy_account_label(uid, Some(login_root), "当前登录的账号")
        }),
        transfer_executed: outcome.transfer.as_ref().map(|result| match result {
            TransferFlowResult::Executed(_) => true,
            TransferFlowResult::Nothing { .. } => false,
        }),
        updated_rows: match outcome.transfer.as_ref() {
            Some(TransferFlowResult::Executed(receipt)) => Some(receipt.updated_rows),
            _ => None,
        },
        verified_sessions: match outcome.transfer.as_ref() {
            Some(TransferFlowResult::Executed(receipt)) => Some(receipt.verified_sessions),
            _ => None,
        },
        missing_body_files: match outcome.transfer.as_ref() {
            Some(TransferFlowResult::Executed(receipt)) => Some(receipt.missing_body_files),
            _ => None,
        },
        cloud_residual_rows: match outcome.transfer.as_ref() {
            Some(TransferFlowResult::Executed(receipt)) => Some(receipt.cloud_residual_rows),
            _ => None,
        },
        transfer_error: outcome.transfer_error,
        restarted: outcome.restarted,
        post_status: outcome.post_status,
    })
}

/// 切号影响面预览（只读零副作用；对齐 Trae preview_master_switch 形态）。
/// 方向口径与执行一致：previous = 当前 .info uid、current = 目标账号。
#[tauri::command]
pub(crate) fn preview_workbuddy_switch(
    target_account_id: Option<String>,
    state: State<'_, AppState>,
) -> Result<WorkBuddySwitchPreviewDto, String> {
    ensure_login_real_mode_workbuddy(&state)?;
    let login_root = workbuddy_login_root(&state)?;
    let target = target_account_id.as_deref();
    // previous = 切号前账号（当前 .info uid）。
    let snapshot = load_session_snapshot(DEFAULT_AUTH_ID)
        .map_err(|error| error.code().to_string())?;
    let previous = snapshot.uid;
    if let Some(target) = target {
        if target == previous {
            return Err(
                traesync_infrastructure::workbuddy_switch_flow::SwitchFlowError::SameAccount
                    .code()
                    .to_string(),
            );
        }
    }
    // 影响面统计（db 缺失/读取失败降级 None——预览不阻塞切号入口）。
    let influence = || -> Option<(Option<u64>, Option<u64>, Option<u64>, Option<u64>)> {
        use traesync_infrastructure::workbuddy_transfer::{build_transfer_plan, db_path, default_data_root};
        let db = db_path(&default_data_root()?);
        let pending_project_count =
            traesync_infrastructure::workbuddy_switch_flow::count_pending_projects(&db, &previous);
        let Some(target) = target else {
            return Some((None, pending_project_count, None, None));
        };
        let plan = build_transfer_plan(&db, target, &previous).ok()?;
        Some((
            Some(plan.pending_local_rows),
            pending_project_count,
            Some(plan.current_account_rows),
            Some(plan.cloud_residual_rows),
        ))
    };
    let (pending_local_rows, pending_project_count, current_account_rows, cloud_residual_rows) =
        influence().unwrap_or((None, None, None, None));
    Ok(WorkBuddySwitchPreviewDto {
        current_label: workbuddy_account_label(&previous, Some(&login_root), "当前登录的账号"),
        target_label: target
            .map(|uid| workbuddy_account_label(uid, Some(&login_root), "目标账号")),
        pending_local_rows,
        pending_project_count,
        current_account_rows,
        cloud_residual_rows,
    })
}

// ===== P18-B1 会话过户（wb_transfer.py 产品化；单次确认入口） =====

/// WorkBuddy 会话过户 DTO（dry_run 计划 + 执行回执合一；界面表达纪律：
/// 账号一律用昵称标签，uid 原文不进 UI）。
#[derive(Debug, Serialize)]
pub struct WorkBuddyTransferDto {
    pub dry_run: bool,
    /// 无需转移（无上一账号或待过户为 0）——UI 直接提示，不出确认弹窗。
    pub nothing_to_transfer: bool,
    /// 当前登录账号展示名（昵称优先，缺省自然语言标签）。
    pub current_label: String,
    /// 上一个登录账号展示名（None = 无上一账号）。
    pub previous_label: Option<String>,
    /// 上一账号名下待过户的本地会话数（含软删除行）。
    pub pending_local_rows: u64,
    /// 当前账号现有会话数。
    pub current_account_rows: u64,
    /// 上一账号名下云端会话数（红线：不碰；>0 时 UI 明示）。
    pub cloud_residual_rows: u64,
    /// 其余更早账号的本地会话数（不在本次范围）。
    pub other_account_rows: u64,
    /// 方向识别来源（"log" = 日志权威；"sessions_fallback" = 最近活跃回退）。
    pub direction_source: &'static str,
    // ---- 执行回执（dry_run=false 时填充） ----
    pub updated_rows: Option<u64>,
    pub verified_sessions: Option<u64>,
    pub missing_body_files: Option<u64>,
    pub restarted: Option<bool>,
    pub process_count_after_restart: Option<usize>,
}

/// 方向来源 → DTO 字符串（前端契约值稳定）。
fn direction_source_str(
    source: traesync_infrastructure::workbuddy_transfer::DirectionSource,
) -> &'static str {
    match source {
        traesync_infrastructure::workbuddy_transfer::DirectionSource::LogEvents => "log",
        traesync_infrastructure::workbuddy_transfer::DirectionSource::SessionsFallback => {
            "sessions_fallback"
        }
    }
}

/// uid → 展示名：**备注名优先**（P19.2① 档案旁表）> 账号池捕获昵称
/// （profile_id 惯例 wb-{uid}）> 兜底自然语言标签。uid 只在容器查询内部
/// 使用，不落到返回值。
fn workbuddy_account_label(
    uid: &str,
    login_root: Option<&Path>,
    fallback: &str,
) -> String {
    let Some(root) = login_root else {
        return fallback.to_string();
    };
    // 备注名（档案旁表；读取失败按未设置降级）。
    let display_name =
        traesync_infrastructure::workbuddy_account_profile::WorkBuddyAccountProfileStore::new(root)
            .get(uid)
            .ok()
            .map(|profile| profile.display_name)
            .filter(|name| !name.trim().is_empty());
    if let Some(name) = display_name {
        return name;
    }
    // 池捕获昵称。
    let from_pool = WorkBuddyLoginStore::new(root.to_path_buf())
        .list()
        .ok()
        .and_then(|summaries| {
            let profile_id =
                traesync_infrastructure::workbuddy_login_credential::WorkBuddyLoginRecord::profile_id_for(uid);
            summaries
                .into_iter()
                .find(|summary| summary.profile_id == profile_id)
                .map(|summary| summary.screen_name)
                .filter(|name| !name.trim().is_empty())
        });
    from_pool.unwrap_or_else(|| fallback.to_string())
}

/// WorkBuddy 会话过户（**单次确认入口**：UI 弹窗确认后调用）。
///
/// dry_run=true 只出转移计划（零副作用，关闭客户端之前）；false 执行完整
/// 编排（方向识别 → 计划 → 完全退出前置校验〔进程 Unknown 拒绝一切写
/// 副作用〕→ 事务过户（仅 transport='local'，云会话红线不碰）→
/// wal_checkpoint 落盘 → 正文抽验 → 重启 + 进程数确认）。
#[tauri::command]
pub(crate) async fn transfer_workbuddy_sessions(
    dry_run: bool,
    state: State<'_, AppState>,
) -> Result<WorkBuddyTransferDto, String> {
    ensure_login_real_mode_workbuddy(&state)?;
    let login_root = workbuddy_login_root(&state).ok();
    // 重启入口：三级探测安装路径；探测失败时跳过重启（回执如实报告），
    // 不阻塞过户本体——用户可手动重启客户端。
    let install_exe =
        traesync_infrastructure::workbuddy_transfer::discover_install().map(|found| found.exe);
    run_blocking("workbuddy_transfer_join_failed", move || {
        let data_root = traesync_infrastructure::workbuddy_transfer::default_data_root()
            .ok_or("workbuddy_data_root_unavailable".to_string())?;
        let observe = || traesync_infrastructure::workbuddy_transfer::observe_client_real();
        let close = || traesync_infrastructure::workbuddy_transfer::close_client_real();
        let start_exe = install_exe.clone();
        let start = move || {
            start_exe
                .as_deref()
                .map(traesync_infrastructure::workbuddy_transfer::start_client_real)
                .unwrap_or(false)
        };
        let count_processes =
            || traesync_infrastructure::workbuddy_transfer::count_client_processes_real();
        let deps = traesync_infrastructure::workbuddy_transfer::TransferClientDeps {
            observe: &observe,
            close: &close,
            start: Some(&start),
            count_processes: Some(&count_processes),
            restart_settle: Duration::from_secs(3),
        };
        let outcome = traesync_infrastructure::workbuddy_transfer::execute_transfer(
            &data_root, &deps, dry_run,
        )
        .map_err(|error| error.code().to_string())?;
        Ok(match outcome {
            traesync_infrastructure::workbuddy_transfer::TransferOutcome::Plan(plan) => {
                WorkBuddyTransferDto {
                    dry_run: true,
                    nothing_to_transfer: false,
                    current_label: workbuddy_account_label(
                        &plan.current_uid,
                        login_root.as_deref(),
                        "当前登录的账号",
                    ),
                    previous_label: Some(workbuddy_account_label(
                        &plan.previous_uid,
                        login_root.as_deref(),
                        "上一个登录的账号",
                    )),
                    pending_local_rows: plan.pending_local_rows,
                    current_account_rows: plan.current_account_rows,
                    cloud_residual_rows: plan.cloud_residual_rows,
                    other_account_rows: plan.other_account_rows,
                    direction_source: direction_source_str(plan.source),
                    updated_rows: None,
                    verified_sessions: None,
                    missing_body_files: None,
                    restarted: None,
                    process_count_after_restart: None,
                }
            }
            traesync_infrastructure::workbuddy_transfer::TransferOutcome::Executed(receipt) => {
                WorkBuddyTransferDto {
                    dry_run: false,
                    nothing_to_transfer: false,
                    current_label: workbuddy_account_label(
                        &receipt.current_uid,
                        login_root.as_deref(),
                        "当前登录的账号",
                    ),
                    previous_label: Some(workbuddy_account_label(
                        &receipt.previous_uid,
                        login_root.as_deref(),
                        "上一个登录的账号",
                    )),
                    pending_local_rows: 0,
                    current_account_rows: receipt.transfer.verified_sessions,
                    cloud_residual_rows: receipt.transfer.cloud_residual_rows,
                    other_account_rows: 0,
                    direction_source: direction_source_str(receipt.source),
                    updated_rows: Some(receipt.transfer.updated_rows),
                    verified_sessions: Some(receipt.transfer.verified_sessions),
                    missing_body_files: Some(receipt.transfer.missing_body_files),
                    restarted: Some(receipt.restarted),
                    process_count_after_restart: receipt.process_count_after_restart,
                }
            }
            traesync_infrastructure::workbuddy_transfer::TransferOutcome::NothingToTransfer {
                ..
            } => WorkBuddyTransferDto {
                dry_run,
                nothing_to_transfer: true,
                current_label: String::new(),
                previous_label: None,
                pending_local_rows: 0,
                current_account_rows: 0,
                cloud_residual_rows: 0,
                other_account_rows: 0,
                direction_source: "log",
                updated_rows: None,
                verified_sessions: None,
                missing_body_files: None,
                restarted: None,
                process_count_after_restart: None,
            },
        })
    })
    .await
}

/// WorkBuddy 深度健康检测回执（P18-B3；文件面 + 实时面双口径）。
#[derive(Debug, Serialize)]
pub struct WorkBuddyDeepHealthDto {
    /// 文件面登录态（与 session_health_dto.status 同口径）。
    pub status: &'static str,
    /// 实时凭据验证：`verified`（客户端运行中且 wbipc 只读回读通过）/
    /// `client_offline`（客户端未运行，仅完成本地检测）/
    /// `probe_failed`（客户端运行中但探针回读异常）。
    pub live_status: &'static str,
}

/// WorkBuddy 深度健康检测（P18-B3「健康检测按钮」后端；只读零写副作用）。
/// 文件面：.info 快照 + 登出标记判定（不依赖客户端运行）；实时面：客户端
/// 运行中时经 wbipc 管道做一次只读会话验证（同切号命令的验证步骤，但
/// 不做任何凭据操作）。
#[tauri::command]
pub(crate) async fn check_workbuddy_login_health(
    state: State<'_, AppState>,
) -> Result<WorkBuddyDeepHealthDto, String> {
    ensure_login_real_mode_workbuddy(&state)?;
    run_blocking("workbuddy_health_join_failed", move || {
        let status = current_session_status(DEFAULT_AUTH_ID, unix_ms_now());
        // 实时面（best-effort）：探针端口组不可达 = 客户端未运行（正常态，
        // 不是错误）；可达但回读失败 = 探针异常（如实报告）。
        let live_status = if traesync_infrastructure::workbuddy_instance::probe_local_api_default().is_err() {
            "client_offline"
        } else {
            let verified = (|| -> Result<(), String> {
                let endpoint_path =
                    traesync_infrastructure::workbuddy_wbipc::default_endpoint_path()
                        .ok_or("workbuddy_health_endpoint_missing".to_string())?;
                let endpoint_raw = std::fs::read_to_string(&endpoint_path)
                    .map_err(|_| "workbuddy_health_endpoint_missing".to_string())?;
                let endpoint =
                    parse_endpoint(&endpoint_raw).map_err(|error| error.code())?;
                verify_client_session_via_rpc(&endpoint, &|| {
                    traesync_infrastructure::workbuddy_wbipc::PipeTransport::connect(
                        &endpoint.pipe_path,
                    )
                    .map(|transport| Box::new(transport) as Box<dyn WbipcTransport>)
                })
                .map_err(|error| error.code())?;
                Ok(())
            })();
            match verified {
                Ok(()) => "verified",
                Err(_) => "probe_failed",
            }
        };
        Ok(WorkBuddyDeepHealthDto {
            status: session_status_str(status),
            live_status,
        })
    })
    .await
}

// ===== 内部 helper =====

/// WorkBuddy 域真实模式门禁（独立错误码；fixture 模式无 WorkBuddy 基建）。
/// pub(crate)：P17-A5 调度命令（workbuddy_schedule）共用同一门禁。
pub(crate) fn ensure_login_real_mode_workbuddy(state: &AppState) -> Result<(), String> {
    if state.runtime_mode != crate::RuntimeMode::RealReadPreview {
        return Err("workbuddy_real_mode_required".to_string());
    }
    Ok(())
}

/// 当前产品指针 store（product-state 根；storage_root 缺失时仍可读缺省——
/// 指针文件不存在即返回缺省值，不依赖 storage 目录存在）。
fn current_product_store(state: &AppState) -> Result<CurrentProductStore, String> {
    if state.storage_root.is_empty() {
        return Err("product_context_unavailable".to_string());
    }
    Ok(CurrentProductStore::new(product_state_root_from(Path::new(
        &state.storage_root,
    ))))
}

/// 当前产品 id（指针缺失回退缺省 trae_cn；storage_root 缺失也回退缺省——
/// 产品上下文是 UI 状态，未初始化环境不应阻塞渲染）。
fn current_product_id(state: &AppState) -> Result<String, String> {
    if state.storage_root.is_empty() {
        return Ok(traesync_infrastructure::product_context::DEFAULT_PRODUCT_ID.to_string());
    }
    let store = current_product_store(state)?;
    store.load().map_err(|_| "product_context_invalid".to_string())
}

/// WorkBuddy 登录容器根。
/// pub(crate)：P19-B3 库域命令（workbuddy_library_commands）共用。
pub(crate) fn workbuddy_login_root(state: &AppState) -> Result<PathBuf, String> {
    if state.storage_root.is_empty() {
        return Err("workbuddy_storage_unavailable".to_string());
    }
    Ok(workbuddy_login_root_from(Path::new(&state.storage_root)))
}

/// 会话健康 DTO 组装（含容器在池标记）。
fn session_health_dto(
    snapshot: &WorkBuddySessionSnapshot,
    status: WorkBuddySessionStatus,
    state: &AppState,
) -> WorkBuddySessionHealthDto {
    let captured_in_pool = workbuddy_login_root(state)
        .ok()
        .map(|root| {
            WorkBuddyLoginStore::new(root)
                .exists(&WorkBuddyLoginRecordProfileId::for_uid(&snapshot.uid))
        })
        .unwrap_or(false);
    WorkBuddySessionHealthDto {
        status: session_status_str(status),
        uid: snapshot.uid.clone(),
        uin: snapshot.uin.clone(),
        domain: snapshot.auth.domain.clone(),
        access_expires_at_unix_seconds: snapshot.auth.expires_at.unwrap_or(0) / 1000,
        refresh_expires_at_unix_seconds: snapshot.auth.refresh_expires_at.unwrap_or(0) / 1000,
        captured_in_pool,
    }
}

fn session_status_str(status: WorkBuddySessionStatus) -> &'static str {
    match status {
        WorkBuddySessionStatus::Active => "active",
        WorkBuddySessionStatus::AccessExpired => "access_expired",
        WorkBuddySessionStatus::FullyExpired => "fully_expired",
        WorkBuddySessionStatus::LoggedOut => "logged_out",
        WorkBuddySessionStatus::FileMissing => "file_missing",
        WorkBuddySessionStatus::Unknown => "unknown",
    }
}

/// profile_id 惯例薄封装（容器在池判定用；惯例本体在容器模块）。
struct WorkBuddyLoginRecordProfileId;

impl WorkBuddyLoginRecordProfileId {
    fn for_uid(uid: &str) -> String {
        traesync_infrastructure::workbuddy_login_credential::WorkBuddyLoginRecord::profile_id_for(
            uid,
        )
    }
}

/// 签到凭据组装与 HTTP 客户端已下沉 infrastructure 层
/// （workbuddy_checkin::open_session_from_machine——组合根零 HTTP 依赖）。

/// WorkBuddy 客户端 exe 探测（常见安装路径候选；TODO：迁移到 descriptor
/// 安装位置发现 + 注册表探测，与 Trae 侧 trae_instance 的入口发现对齐）。
fn workbuddy_client_exe() -> Result<PathBuf, String> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    // 命令行显式覆盖（测试/非标安装）。
    if let Ok(override_path) = std::env::var("WORKBUDDY_CLIENT_EXE") {
        candidates.push(PathBuf::from(override_path));
    }
    // 本机实测安装位置（P17-T2 探针）。
    candidates.push(PathBuf::from(r"E:\软件\workbuddy\WorkBuddy.exe"));
    if let Ok(local) = std::env::var("LOCALAPPDATA") {
        candidates.push(
            Path::new(&local)
                .join("Programs")
                .join("workbuddy")
                .join("WorkBuddy.exe"),
        );
    }
    candidates
        .into_iter()
        .find(|candidate| candidate.is_file())
        .ok_or_else(|| "workbuddy_client_exe_not_found".to_string())
}
