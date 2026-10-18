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

use serde::Serialize;
use tauri::State;

use traesync_infrastructure::product_context::CurrentProductStore;
use traesync_infrastructure::product_descriptor::{descriptor_by_product_id, ALL_PRODUCTS};
use traesync_infrastructure::storage_roots::{
    product_state_root_from, workbuddy_login_root_from,
};
use traesync_infrastructure::workbuddy_capture::capture_workbuddy_login_from_machine;
use traesync_infrastructure::workbuddy_checkin::{
    open_session_from_machine, open_session_from_pool, renew_pool_credentials,
    WorkBuddyCheckinOutcome, WorkBuddyCheckinSession, WorkBuddyPoolError,
    WorkBuddyRenewalOutcome,
};
use traesync_infrastructure::workbuddy_credential::{
    current_session_status, load_session_snapshot, unix_ms_now, WorkBuddySessionStatus,
    WorkBuddySessionSnapshot, DEFAULT_AUTH_ID,
};
use traesync_infrastructure::workbuddy_login_credential::WorkBuddyLoginStore;

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

/// WorkBuddy 账号池（workbuddy-login 容器脱敏清单；真实模式门禁）。
#[tauri::command]
pub(crate) fn list_workbuddy_accounts(
    state: State<AppState>,
) -> Result<Vec<traesync_infrastructure::workbuddy_login_credential::WorkBuddyLoginSummary>, String>
{
    ensure_login_real_mode_workbuddy(&state)?;
    let store = WorkBuddyLoginStore::new(workbuddy_login_root(&state)?);
    store.list().map_err(|_| "workbuddy_pool_invalid".to_string())
}

/// WorkBuddy 签到状态查询（只读；解密借客户端运行时，客户端可关可开）。
#[tauri::command]
pub(crate) async fn workbuddy_checkin_status(state: State<'_, AppState>) -> Result<WorkBuddyCheckinStatusDto, String> {
    ensure_login_real_mode_workbuddy(&state)?;
    let login_root = workbuddy_login_root(&state).ok();
    run_blocking("workbuddy_checkin_join_failed", move || {
        let session = open_checkin_session_any(login_root.as_deref(), &workbuddy_client_exe()?)?;
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
    })
    .await
}

/// WorkBuddy 签到领取（幂等；编排 = 查询未签才领 + 复核翻转确认）。
/// 已签时直接返回 verified=true（today_checked_in 复核口径）。
#[tauri::command]
pub(crate) async fn workbuddy_checkin_claim(state: State<'_, AppState>) -> Result<WorkBuddyCheckinClaimDto, String> {
    ensure_login_real_mode_workbuddy(&state)?;
    let login_root = workbuddy_login_root(&state).ok();
    run_blocking("workbuddy_checkin_join_failed", move || {
        let session = open_checkin_session_any(login_root.as_deref(), &workbuddy_client_exe()?)?;
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
    })
    .await
}

/// 签到会话装配（P17-T4c⑤ 池优先）：池内凭据优先（access 充裕零轮换、
/// 临近过期自动续期写回），池为空或不可用时回退机器解密（.info 现解密，
/// 覆盖未捕获账号）。池错误仅在机器回退同样失败时优先暴露——refresh
/// 拒绝（池 refresh 已轮换作废）引导重新捕获，比机器解密错误更有引导性。
fn open_checkin_session_any(
    login_root: Option<&Path>,
    client_exe: &Path,
) -> Result<WorkBuddyCheckinSession, String> {
    let now = unix_ms_now() / 1000;
    let pool_error = if let Some(root) = login_root {
        match open_session_from_pool(root, now) {
            Ok((session, _renewal)) => return Ok(session),
            Err(error) => Some(error),
        }
    } else {
        None
    };
    open_session_from_machine(client_exe, DEFAULT_AUTH_ID).map_err(|machine_error| {
        match pool_error {
            Some(error @ (WorkBuddyPoolError::RefreshRejected | WorkBuddyPoolError::RecordMissing)) => {
                error.code().to_string()
            }
            _ => machine_error,
        }
    })
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

// ===== 内部 helper =====

/// WorkBuddy 域真实模式门禁（独立错误码；fixture 模式无 WorkBuddy 基建）。
fn ensure_login_real_mode_workbuddy(state: &AppState) -> Result<(), String> {
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
fn workbuddy_login_root(state: &AppState) -> Result<PathBuf, String> {
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
