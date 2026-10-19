//! 直连签到 + 自动签到调度域（P12-R3 自 lib.rs 纯移动）。
//!
//! 覆盖：签到命令壳（能力/执行/取消/总览）、真实批量签到编排、
//! 自动签到调度线程（30 秒 tick + 三级触发判定编排）、自动签到设置命令，
//! 以及三条批量链共用的进度事件契约 BatchProgressEvent 与串行运行器
//! run_serial_batch。凭据刷新/健康检测/积分刷新三条链的命令本轮留在
//! lib.rs（凭据域语义，credential_handover 梯队再定），经
//! `use crate::checkin_schedule::{BatchProgressEvent, run_serial_batch}` 引用。
//! 命令体、错误码与 wire 形态零变化；与 infrastructure 的 auto_checkin_store /
//! checkin_http 边界保持现状（本模块只做命令壳与编排）。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};

use chrono::{Timelike, Utc};
use serde::Serialize;
use tauri::Emitter;

use traesync_application::{BatchCheckinRunner, CheckinService, CheckinTransport};
use traesync_infrastructure::{
    AccountRecord, AccountRegistry, AutoCheckinBatchState, AutoCheckinLedger, AutoCheckinSettings,
    AutoCheckinStore, CheckinCredentialError, CheckinCredentialStore, CheckinProfileBinding,
    CheckinProfileOperationLease, CredentialMaintenanceStateStore, FixtureCheckinTransport,
    RealCheckinRenewalService, RealCheckinTransport,
};

use crate::{
    checkin_overview, error_mapping::credential_maintenance_error_code, run_blocking, AppState,
    RuntimeMode,
};
// P12-R3：system_time_to_rfc3339 随凭据域迁 credential_handover。
use crate::credential_handover::system_time_to_rfc3339;

/// 签到能力白名单 DTO；真实 HTTP 未启用时明确显示，不伪装成线上签到。
/// pub(crate)：tauri 命令宏展开的包装函数签名需要暴露返回类型。
#[derive(Debug, Clone, Serialize)]
pub(crate) struct CheckinCapabilityWireDto {
    enabled: bool,
    transport: String,
    real_http_enabled: bool,
    message: String,
}

#[derive(Debug, Clone, Serialize)]
struct CheckinStatusWireDto {
    enabled: bool,
    checked_in: bool,
    credits: Option<i64>,
    business_code: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
struct CheckinResultWireDto {
    profile_id: String,
    outcome: traesync_domain::CheckinOutcome,
    state: traesync_domain::CheckinTaskState,
    claim_attempted: bool,
    before: Option<CheckinStatusWireDto>,
    after: Option<CheckinStatusWireDto>,
    detail_code: Option<String>,
    started_at: String,
    finished_at: String,
}

/// pub(crate)：tauri 命令宏展开的包装函数签名需要暴露返回类型。
#[derive(Debug, Clone, Serialize)]
pub(crate) struct CheckinBatchSummaryWireDto {
    total: usize,
    completed: usize,
    failed: usize,
    cancelled: usize,
    results: Vec<CheckinResultWireDto>,
}

/// 签到逐账号进度事件（checkin-progress）：串行批次中每完成一个账号推送一次，
/// 让前端在 3-8 秒/账号的防风控间隔期间仍有实时反馈，而不是黑盒等待。
#[derive(Debug, Clone, Serialize)]
struct CheckinProgressEvent {
    profile_id: String,
    screen_name: String,
    outcome: traesync_domain::CheckinOutcome,
    detail_code: Option<String>,
    /// 已完成账号数（含失败，1 起）。
    completed: usize,
    /// 批次账号总数。
    total: usize,
}

/// 签到阶段事件（checkin-phase）：批量执行进入需等待的阶段时推送一次，
/// 前端据 remaining_secs 本地倒计时——冷却与账号间错峰不再是
/// 无反馈的"签到中…"盲区。
#[derive(Debug, Clone, Serialize)]
struct CheckinPhaseEvent {
    profile_id: String,
    screen_name: String,
    /// "cooldown"（设备冷却中）/ "inter_wait"（账号间错峰等待，下一个账号）。
    phase: String,
    /// 该阶段剩余秒数（推送时刻的全额时长，前端自行递减）。
    remaining_secs: u64,
}

fn checkin_status_wire(status: &traesync_domain::CheckinStatusSnapshot) -> CheckinStatusWireDto {
    CheckinStatusWireDto {
        enabled: status.enabled,
        checked_in: status.checked_in,
        credits: status.credits,
        business_code: status.business_code,
    }
}

fn checkin_result_wire(result: traesync_domain::CheckinResult) -> CheckinResultWireDto {
    CheckinResultWireDto {
        profile_id: result.profile_id,
        outcome: result.outcome,
        state: result.state,
        claim_attempted: result.claim_attempted,
        before: result.before.as_ref().map(checkin_status_wire),
        after: result.after.as_ref().map(checkin_status_wire),
        detail_code: result.detail_code,
        started_at: system_time_to_rfc3339(result.started_at),
        finished_at: system_time_to_rfc3339(result.finished_at),
    }
}

fn checkin_capability_inner(state: &AppState) -> CheckinCapabilityWireDto {
    match state.runtime_mode {
        RuntimeMode::Fixture => CheckinCapabilityWireDto {
            enabled: true,
            transport: "fixture".to_string(),
            real_http_enabled: false,
            message: "当前为 fixture transport，仅用于验收签到流程；不会访问远程服务。".to_string(),
        },
        // 真实模式：签到能力由已登录账号（注册表 + 凭据包）驱动，未登录账号单独提示。
        RuntimeMode::RealReadPreview => {
            if state.storage_root.is_empty() {
                CheckinCapabilityWireDto {
                    enabled: false,
                    transport: "disabled".to_string(),
                    real_http_enabled: false,
                    message: "存储根不可用，真实签到未启用。".to_string(),
                }
            } else {
                CheckinCapabilityWireDto {
                    enabled: true,
                    transport: "real".to_string(),
                    real_http_enabled: true,
                    message:
                        "真实签到已启用：仅对已通过登录的账号直连 TRAE；未登录账号会提示先登录。"
                            .to_string(),
                }
            }
        }
    }
}

/// 返回签到能力状态。此命令无网络和文件副作用。
#[tauri::command]
pub(crate) fn get_checkin_capability(state: tauri::State<AppState>) -> CheckinCapabilityWireDto {
    checkin_capability_inner(&state)
}

/// 执行批量签到：fixture 模式走本地演示 transport；真实模式只对已登录账号直连。
#[tauri::command]
pub(crate) async fn run_checkin(
    profile_ids: Vec<String>,
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
) -> Result<CheckinBatchSummaryWireDto, String> {
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
    // 新批次从干净取消状态开始；取消只影响当前批次尚未开始的条目。
    state.checkin_cancellation.store(false, Ordering::Release);
    let cancellation = std::sync::Arc::clone(&state.checkin_cancellation);
    // 手动批次与自动签到批次互斥（v6：try-lock 快速失败——执行中重复触发
    // 立即报 checkin_already_running，不再静默排队造成"一直签到中"假死感；
    // 自动批次侧本来就以 try_lock 跳过，语义对等）。match 两分支闭包各自持克隆。
    let execution_lock_fixture = std::sync::Arc::clone(&state.checkin_execution_lock);
    let execution_lock_real = std::sync::Arc::clone(&state.checkin_execution_lock);
    // 阻塞执行（含 3-8 秒/账号的防风控间隔）全部放 spawn_blocking，
    // async 命令立即返回挂起，窗口与前端事件循环不受影响。
    let summary = match state.runtime_mode {
        RuntimeMode::Fixture => {
            let ids = selected.clone();
            let app_handle = app.clone();
            run_blocking(
                "checkin_join_failed",
                move || -> Result<traesync_domain::CheckinBatchSummary, String> {
                    let _guard = execution_lock_fixture
                        .try_lock()
                        .map_err(|_| "checkin_already_running".to_string())?;
                    let transport = FixtureCheckinTransport::for_profiles(&ids);
                    let service = CheckinService::new(&transport);
                    let summary = service.run_batch(&ids, &cancellation);
                    // fixture 无间隔瞬间完成：结果统一补发进度事件，保持前端体验一致。
                    for (index, result) in summary.results.iter().enumerate() {
                        emit_checkin_progress(
                            &app_handle,
                            result,
                            &std::collections::BTreeMap::new(),
                            index + 1,
                            summary.total,
                        );
                    }
                    Ok(summary)
                },
            )
            .await?
        }
        RuntimeMode::RealReadPreview => {
            let material_root = checkin_material_root(&state)?;
            let ids = selected.clone();
            let app_handle = app.clone();
            run_blocking("checkin_join_failed", move || {
                let _guard = execution_lock_real
                    .try_lock()
                    .map_err(|_| "checkin_already_running".to_string())?;
                run_real_checkin(material_root, ids, cancellation, Some(app_handle), false)
            })
            .await?
        }
    };
    Ok(CheckinBatchSummaryWireDto {
        total: summary.total,
        completed: summary.completed,
        failed: summary.failed,
        cancelled: summary.cancelled,
        results: summary
            .results
            .into_iter()
            .map(checkin_result_wire)
            .collect(),
    })
}

/// 发送单账号签到进度事件：screen_name 查不到时为空串，前端回退显示 profile_id。
fn emit_checkin_progress(
    app: &tauri::AppHandle,
    result: &traesync_domain::CheckinResult,
    name_by_profile: &std::collections::BTreeMap<String, String>,
    completed: usize,
    total: usize,
) {
    let _ = app.emit(
        "checkin-progress",
        CheckinProgressEvent {
            profile_id: result.profile_id.clone(),
            screen_name: name_by_profile
                .get(&result.profile_id)
                .cloned()
                .unwrap_or_default(),
            outcome: result.outcome.clone(),
            detail_code: result.detail_code.clone(),
            completed,
            total,
        },
    );
}

/// 签到材料根目录：`<storage_root>/checkin`（DPAPI 凭据密文 + 非敏感账号注册表）。
/// OAuth 登录入库与真实签到必须使用同一路径约定，避免档案与凭据分离。
/// pub(crate)：oauth_login 模块与留守组合根命令共用同一路径约定。
pub(crate) fn checkin_material_root(state: &AppState) -> Result<PathBuf, String> {
    if state.storage_root.is_empty() {
        return Err("checkin_storage_unavailable".to_string());
    }
    // P16-T2：路径拼接收口到 storage_roots 唯一来源。
    Ok(traesync_infrastructure::storage_roots::checkin_material_root_from(
        Path::new(&state.storage_root),
    ))
}

/// 自动签到调度器循环间隔（秒）：兼顾触发及时性与存储读取频率。
const AUTO_CHECKIN_TICK_SECONDS: u64 = 30;

/// 凭据维护检查间隔（秒）：在 access token 过期前留出足够的重试窗口，
/// 同时避免在每个 30 秒签到 tick 上重复访问 OAuth 接口。
const CREDENTIAL_MAINTENANCE_INTERVAL_SECONDS: u64 = 60 * 60;

/// 独立维护所有已登记账号的凭据。
///
/// 维护不依赖自动签到开关或当天签到时间；只要管理工具在运行，就会按
/// `RealCheckinRenewalService` 的阈值检查凭据。整个维护批次复用签到编排的
/// 互斥锁，避免维护、签到、手动重置设备并发写入同一凭据包。
fn maintain_all_credentials(material_root: &Path, execution_lock: &Arc<Mutex<()>>) -> bool {
    // 维护批次整体持锁，避免同一进程内签到/设备操作在账号之间插入写回。
    let Ok(_guard) = execution_lock.try_lock() else {
        // 锁忙时不消费本次维护时间点，下一 tick 继续尝试，避免与签到
        // 恰好重叠后整整延迟一个维护周期。
        return false;
    };
    let registry = AccountRegistry::new(material_root);
    let records = registry.load().unwrap_or_default();
    let store = CheckinCredentialStore::new(material_root);
    let maintenance_state = CredentialMaintenanceStateStore::new(material_root);
    let renewal = RealCheckinRenewalService::new(&store);
    let now = Utc::now().timestamp().max(0) as u64;
    for snapshot in records {
        let profile_id = snapshot.profile_id;
        // 账号级租约覆盖退避判定、续期与结果记录，避免多进程按旧状态重复触网。
        let profile_lease = match store.lock_profile(&profile_id) {
            Ok(lease) => lease,
            Err(_) => continue,
        };
        // 排队等锁期间档案可能已删除、重登或修改设备，后续操作只使用锁内快照。
        let record = match registry.find(&profile_id) {
            Ok(Some(record)) => record,
            Ok(None) => continue,
            Err(_) => continue,
        };
        let binding = CheckinProfileBinding::from(&record);
        // 退避状态落盘后，App 重启不会把失败账号重新当成可立即尝试，
        // 从而避免启动/定时器反复消耗服务端请求。
        let allowed = match maintenance_state.is_attempt_allowed(&profile_id, now) {
            Ok(allowed) => allowed,
            Err(_) => {
                // 状态文件损坏或不可读时按 fail-closed 处理：不发网络请求，
                // 同时留下可在账号列表中观察到的非敏感原因码。
                checkin_overview::update_refresh_failures(
                    material_root,
                    &[(
                        profile_id,
                        "credential_maintenance_state_unavailable".to_string(),
                    )],
                );
                continue;
            }
        };
        if !allowed {
            continue;
        }
        match renewal.renew_if_needed_with_lease(&binding, now, &profile_lease) {
            Ok(Some(_)) => {
                if maintenance_state
                    .record_success(&binding.profile_id, now)
                    .is_ok()
                {
                    checkin_overview::clear_refresh_failures(
                        material_root,
                        std::slice::from_ref(&profile_id),
                    );
                } else {
                    checkin_overview::update_refresh_failures(
                        material_root,
                        &[(
                            profile_id,
                            "credential_maintenance_state_unavailable".to_string(),
                        )],
                    );
                }
            }
            Ok(None) => {}
            Err(error) => {
                let error_code = credential_maintenance_error_code(&error).to_string();
                checkin_overview::update_refresh_failures(
                    material_root,
                    &[(profile_id.clone(), error_code.clone())],
                );
                if maintenance_state
                    .record_failure(&profile_id, now, &error_code)
                    .is_err()
                {
                    checkin_overview::update_refresh_failures(
                        material_root,
                        &[(
                            profile_id,
                            "credential_maintenance_state_unavailable".to_string(),
                        )],
                    );
                }
            }
        }
    }
    true
}

/// 自动签到调度器（grill 2026-08-23 决策 1B：每日时间点 + 打开补偿混合）。
///
/// 后台线程每 tick 检查触发条件（设置开启 + 今日未发起 + 已到/已过今日时间点）；
/// 通过后读取注册表筛选 `auto_checkin_enabled` 账号，生成 0~15 分钟错峰计划，
/// 逐账号短持锁执行（手动批次执行中 try_lock 失败即跳过，决策 5 互斥规则）。
/// 台账发起即写：错峰期间 App 退出后台账停 Running，下次 tick 由中断恢复
/// 补跑收尾（P11-2②）；撞锁跳过账号当日每约 30 分钟顺延重试（P11-2①）。
/// 仅真实模式启动（fixture 演示无自动签到语义）。
/// pub(crate)：由组合根 run() 的 setup 钩子启动。
pub(crate) fn spawn_auto_checkin_scheduler(
    app: tauri::AppHandle,
    storage_root: String,
    execution_lock: Arc<Mutex<()>>,
) {
    std::thread::spawn(move || {
        // storage_root 为空（生产目录初始化失败）时不启动任何自动行为。
        if storage_root.is_empty() {
            return;
        }
        // P16-T2：路径拼接收口到 storage_roots 唯一来源。
        let material_root = traesync_infrastructure::storage_roots::checkin_material_root_from(
            Path::new(&storage_root),
        );
        let mut last_credential_maintenance: Option<std::time::Instant> = None;
        loop {
            std::thread::sleep(std::time::Duration::from_secs(AUTO_CHECKIN_TICK_SECONDS));
            let maintenance_due = last_credential_maintenance
                .map(|last| {
                    last.elapsed()
                        >= std::time::Duration::from_secs(CREDENTIAL_MAINTENANCE_INTERVAL_SECONDS)
                })
                .unwrap_or(true);
            if maintenance_due && maintain_all_credentials(&material_root, &execution_lock) {
                last_credential_maintenance = Some(std::time::Instant::now());
            }
            run_auto_checkin_tick(&app, &material_root, &execution_lock);
        }
    });
}

/// 单次调度 tick：判定 -> 执行对应批次。任何存储错误静默等待下个 tick
/// （fail-closed：损坏的设置/台账文件不触发自动签到）。
///
/// 判定优先级（互斥，逐级落空）：
/// 1. 常规触发：今日未发起且已到时间点 → 全量错峰首发批次；
/// 2. 中断恢复：今日台账 Running（批次在本线程同步执行，tick 时观察到的
///    Running 必为上次进程中断遗留）→ 全量补跑（before 探测防重复 claim）；
/// 3. 顺延重试：今日台账已完成且仍有撞锁跳过账号，距上次批次结束满
///    30 分钟 → 只重试 skipped 账号（P11-2①）。
fn run_auto_checkin_tick(
    app: &tauri::AppHandle,
    material_root: &Path,
    execution_lock: &Arc<Mutex<()>>,
) {
    let store = AutoCheckinStore::new(material_root);
    // 一个共享产品根同一时间只允许一个进程推进批次台账；租约不锁设置文件。
    let Ok(Some(_batch_lease)) = store.try_acquire_batch_execution_lease() else {
        return;
    };
    let Ok((settings, ledger)) = store.load() else {
        return;
    };
    // 总开关关闭时调度器完全静默（含中断补跑与顺延重试）。
    if !settings.enabled {
        return;
    }
    let now = chrono::Local::now();
    let today = now.format("%Y-%m-%d").to_string();
    let now_minutes = now.hour() * 60 + now.minute();

    // 1) 常规触发：今日未发起（台账非今日）且已到/已过今日触发时间点。
    if traesync_infrastructure::should_trigger_auto_checkin(
        &settings,
        ledger.as_ref(),
        &today,
        now_minutes,
    ) {
        let eligible_ids = auto_checkin_eligible_ids(material_root);
        let mut ledger = fresh_auto_checkin_ledger(today.clone(), eligible_ids.len());
        let _ = store.save_ledger(&ledger);
        match traesync_infrastructure::build_staggered_plan(eligible_ids) {
            Ok(plan) => {
                run_auto_checkin_batch(
                    app,
                    material_root,
                    execution_lock,
                    &store,
                    &mut ledger,
                    plan,
                );
            }
            // 随机源异常极罕见；台账已记 Running，下个 tick 走中断恢复补跑。
            Err(_) => return,
        }
        return;
    }

    // 2) 中断恢复：今日台账 Running = 上次进程中断遗留，重写台账全量补跑。
    if traesync_infrastructure::should_recover_interrupted_checkin(ledger.as_ref(), &today) {
        let eligible_ids = auto_checkin_eligible_ids(material_root);
        let mut ledger = fresh_auto_checkin_ledger(today.clone(), eligible_ids.len());
        let _ = store.save_ledger(&ledger);
        // 补跑不错峰：顺序直跑；before 探测让已签账号直接 AlreadyCheckedIn。
        let plan = eligible_ids.into_iter().map(|id| (id, 0u64)).collect();
        run_auto_checkin_batch(
            app,
            material_root,
            execution_lock,
            &store,
            &mut ledger,
            plan,
        );
        return;
    }

    // 3) 顺延重试：已完成批次仍有撞锁账号且距上次批次结束满 30 分钟。
    let now_unix = now.timestamp().max(0) as u64;
    if traesync_infrastructure::should_retry_skipped_checkin(ledger.as_ref(), &today, now_unix) {
        let Some(saved) = ledger.as_ref() else {
            return;
        };
        let mut ledger = saved.clone();
        // 只重试撞锁跳过的账号（不错峰）；再撞锁则台账结束时刻随批次刷新，
        // 自然继续顺延到无 skipped 或当日结束。
        let plan = ledger
            .skipped_ids
            .iter()
            .map(|id| (id.clone(), 0u64))
            .collect();
        run_auto_checkin_batch(
            app,
            material_root,
            execution_lock,
            &store,
            &mut ledger,
            plan,
        );
    }
}

/// 筛选参与自动签到的账号 id（注册表读取失败按无账号处理）。
fn auto_checkin_eligible_ids(material_root: &Path) -> Vec<String> {
    AccountRegistry::new(material_root)
        .load()
        .unwrap_or_default()
        .into_iter()
        .filter(|record| record.auto_checkin_enabled)
        .map(|record| record.profile_id)
        .collect()
}

/// 今日新批次台账（首发与中断补跑共用：计数从零起，描述本次批次）。
fn fresh_auto_checkin_ledger(today: String, total: usize) -> AutoCheckinLedger {
    AutoCheckinLedger {
        date: today,
        state: AutoCheckinBatchState::Running,
        total,
        completed: 0,
        failed: 0,
        skipped: 0,
        skipped_ids: Vec::new(),
        last_batch_finished_at: None,
    }
}

/// 记录跳过总数，并只把仍需重试的撞锁账号留在 retry 集合。
fn record_auto_checkin_skip(ledger: &mut AutoCheckinLedger, profile_id: &str, retryable: bool) {
    let existing = ledger.skipped_ids.iter().position(|id| id == profile_id);
    if retryable {
        if existing.is_none() {
            ledger.skipped += 1;
            ledger.skipped_ids.push(profile_id.to_string());
        }
    } else if let Some(position) = existing {
        ledger.skipped_ids.remove(position);
    } else {
        ledger.skipped += 1;
    }
}

/// 手动批次占锁时复核账号开关，并在 profile lease 内更新可重试跳过状态。
fn record_busy_auto_checkin_skip(
    material_root: &Path,
    store: &AutoCheckinStore,
    ledger: &mut AutoCheckinLedger,
    profile_id: &str,
) {
    let credential_store = CheckinCredentialStore::new(material_root);
    let profile_lease = credential_store.lock_profile(profile_id).ok();
    // 状态读取失败时保留顺延候选；实际签到仍会重新校验并失败关闭。
    let retryable = profile_lease
        .as_ref()
        .map(|_| {
            AccountRegistry::new(material_root)
                .find(profile_id)
                .ok()
                .flatten()
                .map(|record| record.auto_checkin_enabled)
                .unwrap_or(true)
        })
        .unwrap_or(true);
    record_auto_checkin_skip(ledger, profile_id, retryable);
    let _ = store.save_ledger(ledger);
}

/// 执行一个自动签到批次：逐账号短持锁 + 单账号签到路径，实时写台账并
/// 推送进度；结束时置 Finished 并记录结束时刻（顺延重试的间隔基准）。
/// plan 为 (profile_id, 相对延迟秒) 升序序列：首发批次错峰 0~15 分钟，
/// 补跑/重试批次全部为 0（顺序直跑）。
fn run_auto_checkin_batch<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    material_root: &Path,
    execution_lock: &Arc<Mutex<()>>,
    store: &AutoCheckinStore,
    ledger: &mut AutoCheckinLedger,
    plan: Vec<(String, u64)>,
) {
    // 进度事件的名称查找表（与 run_real_checkin 相同约定）。
    let name_by_profile: std::collections::BTreeMap<String, String> =
        AccountRegistry::new(material_root)
            .load()
            .unwrap_or_default()
            .into_iter()
            .map(|record| (record.profile_id, record.screen_name))
            .collect();
    let mut previous_delay = 0u64;
    let mut handled = 0usize;
    for (profile_id, delay) in plan {
        // 增量睡眠到该账号目标时刻（plan 按延迟升序；补跑/重试全为 0）。
        std::thread::sleep(std::time::Duration::from_secs(delay - previous_delay));
        previous_delay = delay;
        handled += 1;
        // 决策 5 互斥：手动批次执行中跳过该账号，记入当日顺延重试集合
        // （重试批次再撞锁则保持原状，等下个 30 分钟周期）。
        let Ok(_guard) = execution_lock.try_lock() else {
            // 不持 execution mutex；profile lease 与设置写入共用同一把账号锁。
            record_busy_auto_checkin_skip(material_root, store, ledger, &profile_id);
            emit_auto_checkin_progress(
                app,
                &profile_id,
                &name_by_profile,
                &traesync_domain::CheckinOutcome::ProfileBusy,
                None,
                handled,
                ledger.total,
            );
            continue;
        };
        // 自动批次使用独立取消标记：不受手动批次取消状态影响。
        let cancellation = Arc::new(AtomicBool::new(false));
        let checkin = run_real_checkin(
            material_root.to_path_buf(),
            vec![profile_id.clone()],
            cancellation,
            None,
            true,
        );
        if checkin
            .as_ref()
            .is_ok_and(|summary| summary.cancelled == 1 && summary.results.is_empty())
        {
            // 用户在错峰等待期间关闭该账号时，记为本批跳过且不进入顺延重试。
            record_auto_checkin_skip(ledger, &profile_id, false);
            let _ = store.save_ledger(ledger);
            continue;
        }
        let outcome = match checkin {
            Ok(summary) => summary
                .results
                .first()
                .map(|result| (result.outcome.clone(), result.detail_code.clone()))
                .unwrap_or((traesync_domain::CheckinOutcome::RuntimeError, None)),
            Err(_) => (traesync_domain::CheckinOutcome::RuntimeError, None),
        };
        // 成功口径：发放/已签/无资格都算"处理完毕无需干预"；其余计失败待人工关注。
        let is_ok = matches!(
            outcome.0,
            traesync_domain::CheckinOutcome::Claimed
                | traesync_domain::CheckinOutcome::AlreadyCheckedIn
                | traesync_domain::CheckinOutcome::NotEligible
        );
        if is_ok {
            ledger.completed += 1;
        } else {
            ledger.failed += 1;
        }
        // 重试批次中已有结论的账号移出顺延重试集合：签成转 completed、
        // 真实失败转 failed，均不再反复重试（skipped 计数同步回落）。
        if let Some(position) = ledger.skipped_ids.iter().position(|id| id == &profile_id) {
            ledger.skipped_ids.remove(position);
            ledger.skipped = ledger.skipped.saturating_sub(1);
        }
        let _ = store.save_ledger(ledger);
        emit_auto_checkin_progress(
            app,
            &profile_id,
            &name_by_profile,
            &outcome.0,
            outcome.1.as_deref(),
            handled,
            ledger.total,
        );
    }
    ledger.state = AutoCheckinBatchState::Finished;
    ledger.last_batch_finished_at = Some(chrono::Utc::now().timestamp().max(0) as u64);
    let _ = store.save_ledger(ledger);
    // 完成事件（前端横幅，决策 5：应用内通知）。
    let _ = app.emit(
        "auto-checkin-finished",
        AutoCheckinFinishedEvent {
            total: ledger.total,
            completed: ledger.completed,
            failed: ledger.failed,
            skipped: ledger.skipped,
        },
    );
}

/// 自动签到单账号进度事件（auto-checkin-progress）：与手动通道分离，
/// completed/total 语义是"今日自动批次"而非单次调用。
fn emit_auto_checkin_progress<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    profile_id: &str,
    name_by_profile: &std::collections::BTreeMap<String, String>,
    outcome: &traesync_domain::CheckinOutcome,
    detail_code: Option<&str>,
    completed: usize,
    total: usize,
) {
    let _ = app.emit(
        "auto-checkin-progress",
        CheckinProgressEvent {
            profile_id: profile_id.to_string(),
            screen_name: name_by_profile.get(profile_id).cloned().unwrap_or_default(),
            outcome: outcome.clone(),
            detail_code: detail_code.map(str::to_string),
            completed,
            total,
        },
    );
}

#[cfg(test)]
mod auto_checkin_schedule_tests {
    use super::*;
    use crate::test_support::registered_account;

    #[test]
    fn busy_manual_batch_skips_disabled_accounts_without_retrying() {
        let app = tauri::test::mock_builder()
            .build(tauri::generate_context!())
            .unwrap();
        let root = tempfile::tempdir().unwrap();
        registered_account(root.path(), "profile-a");
        let material_root = root.path().join("checkin");
        let registry = AccountRegistry::new(&material_root);
        let execution_lock = Arc::new(Mutex::new(()));
        let _manual_batch = execution_lock.lock().unwrap();
        let store = AutoCheckinStore::new(&material_root);
        let profile_id = "profile-a".to_string();
        let today = chrono::Local::now().format("%Y-%m-%d").to_string();

        registry
            .set_auto_checkin_enabled(&profile_id, false)
            .unwrap();
        let mut first_ledger = fresh_auto_checkin_ledger(today.clone(), 1);
        assert!(execution_lock.try_lock().is_err());
        run_auto_checkin_batch(
            app.handle(),
            &material_root,
            &execution_lock,
            &store,
            &mut first_ledger,
            vec![(profile_id.clone(), 0)],
        );
        assert_eq!(first_ledger.skipped, 1);
        assert!(first_ledger.skipped_ids.is_empty());
        first_ledger.state = AutoCheckinBatchState::Finished;
        first_ledger.last_batch_finished_at = Some(0);
        assert!(!traesync_infrastructure::should_retry_skipped_checkin(
            Some(&first_ledger),
            &today,
            u64::MAX,
        ));

        registry
            .set_auto_checkin_enabled(&profile_id, true)
            .unwrap();
        let mut retry_ledger = fresh_auto_checkin_ledger(today.clone(), 1);
        run_auto_checkin_batch(
            app.handle(),
            &material_root,
            &execution_lock,
            &store,
            &mut retry_ledger,
            vec![(profile_id.clone(), 0)],
        );
        assert_eq!(retry_ledger.skipped, 1);
        assert_eq!(retry_ledger.skipped_ids, [profile_id.clone()]);
        retry_ledger.state = AutoCheckinBatchState::Finished;
        retry_ledger.last_batch_finished_at = Some(0);
        assert!(traesync_infrastructure::should_retry_skipped_checkin(
            Some(&retry_ledger),
            &today,
            u64::MAX,
        ));

        registry
            .set_auto_checkin_enabled(&profile_id, false)
            .unwrap();
        run_auto_checkin_batch(
            app.handle(),
            &material_root,
            &execution_lock,
            &store,
            &mut retry_ledger,
            vec![(profile_id, 0)],
        );
        assert_eq!(retry_ledger.skipped, 1);
        assert!(retry_ledger.skipped_ids.is_empty());
        assert!(!traesync_infrastructure::should_retry_skipped_checkin(
            Some(&retry_ledger),
            &today,
            u64::MAX,
        ));
    }
}

/// 自动签到批次完成事件（auto-checkin-finished）：前端横幅汇总。
#[derive(Debug, Clone, Serialize)]
struct AutoCheckinFinishedEvent {
    total: usize,
    completed: usize,
    failed: usize,
    skipped: usize,
}

/// 查询自动签到设置与今日台账（设置页 + 签到页状态行共用）。
#[tauri::command]
pub(crate) fn get_auto_checkin_settings(
    state: tauri::State<'_, AppState>,
) -> Result<AutoCheckinStatusWireDto, String> {
    let material_root = checkin_material_root(&state)?;
    let (settings, ledger) = AutoCheckinStore::new(&material_root)
        .load()
        .map_err(|_| "auto_checkin_store_invalid".to_string())?;
    Ok(AutoCheckinStatusWireDto {
        enabled: settings.enabled,
        daily_time_hhmm: settings.daily_time_hhmm,
        ledger: ledger.map(|ledger| AutoCheckinLedgerWireDto {
            date: ledger.date,
            running: ledger.state == AutoCheckinBatchState::Running,
            total: ledger.total,
            completed: ledger.completed,
            failed: ledger.failed,
            skipped: ledger.skipped,
        }),
    })
}

/// 更新自动签到设置；时间格式非法返回错误（前端用 time 控件一般不会触发）。
#[tauri::command]
pub(crate) fn set_auto_checkin_settings(
    enabled: bool,
    daily_time_hhmm: String,
    state: tauri::State<'_, AppState>,
) -> Result<(), String> {
    if !traesync_infrastructure::is_valid_hhmm(&daily_time_hhmm) {
        return Err("auto_checkin_time_invalid".to_string());
    }
    let material_root = checkin_material_root(&state)?;
    AutoCheckinStore::new(&material_root)
        .save_settings(&AutoCheckinSettings {
            enabled,
            daily_time_hhmm,
        })
        .map_err(|_| "auto_checkin_store_invalid".to_string())
}

/// 更新单账号的自动签到参与开关（账号详情页复选框）。
/// 返回账号是否存在；不存在视为错误由前端提示。
#[tauri::command]
pub(crate) fn set_account_auto_checkin(
    profile_id: String,
    enabled: bool,
    state: tauri::State<'_, AppState>,
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
        .set_auto_checkin_enabled(&profile_id, enabled)
        .map_err(|_| "checkin_registry_invalid".to_string())?;
    if !updated {
        return Err("checkin_profile_not_found".to_string());
    }
    Ok(())
}

/// 自动签到设置 + 今日台账查询 DTO。
/// pub(crate)：tauri 命令宏展开的包装函数签名需要暴露返回类型。
#[derive(Debug, Clone, Serialize)]
pub(crate) struct AutoCheckinStatusWireDto {
    enabled: bool,
    daily_time_hhmm: String,
    ledger: Option<AutoCheckinLedgerWireDto>,
}

/// 今日台账 DTO（running=true 表示错峰执行中或中断未完成）。
/// pub(crate)：P17-A5 WorkBuddy 调度状态回执复用同形（workbuddy_schedule）。
#[derive(Debug, Clone, Serialize)]
pub(crate) struct AutoCheckinLedgerWireDto {
    pub(crate) date: String,
    pub(crate) running: bool,
    pub(crate) total: usize,
    pub(crate) completed: usize,
    pub(crate) failed: usize,
    pub(crate) skipped: usize,
}

/// 真实批量签到：注册表取档案 -> 阈值续期（refresh 模式）-> status/claim/status 直连。
///
/// 失败收口规则（fail-closed，ADR-0019 第 8 条、ADR-0027）：
/// - 未登录（注册表无档案）或续期失败的账号合成失败结果，不发起签到；
/// - 存在中断写回现场（RecoveryRequired）时所有账号拒绝，先恢复再签到；
/// - 单账号失败不影响其余账号执行。
fn run_real_checkin(
    material_root: PathBuf,
    selected: Vec<String>,
    cancellation: std::sync::Arc<std::sync::atomic::AtomicBool>,
    app: Option<tauri::AppHandle>,
    automatic: bool,
) -> Result<traesync_domain::CheckinBatchSummary, String> {
    let registry = AccountRegistry::new(&material_root);
    let records = registry
        .load()
        .map_err(|_| "checkin_registry_invalid".to_string())?;
    // 进度事件的名称查找表；查不到（如 profile 已不在注册表）由前端回退显示 profile_id。
    let name_by_profile: std::collections::BTreeMap<String, String> = records
        .iter()
        .map(|record| (record.profile_id.clone(), record.screen_name.clone()))
        .collect();
    let total = selected.len();
    let completed_seen = std::cell::Cell::new(0usize);
    // 设备铸造时刻查找表：9074 报错时区分「设备太新」与「设备被拒」。
    let device_created_by_profile: std::collections::BTreeMap<String, u64> = records
        .iter()
        .map(|record| {
            (
                record.profile_id.clone(),
                record.device_created_at_unix_seconds,
            )
        })
        .collect();
    let expected_accounts: BTreeMap<String, String> = records
        .iter()
        .map(|record| (record.profile_id.clone(), record.account_id.clone()))
        .collect();
    let active_profile = std::cell::RefCell::new(None::<(String, String)>);
    // 阶段事件需要独立持有名称与 AppHandle，避免和签到进度回调共享所有权。
    let phase_app = app.clone();
    let phase_names = name_by_profile.clone();
    // 串行循环内单线程调用：Cell 计数即可，无需原子类型。
    let progress_device_created = device_created_by_profile.clone();
    let progress_names = &name_by_profile;
    let progress_expected_accounts = &expected_accounts;
    let progress_active_profile = &active_profile;
    let emit_progress = |result: &traesync_domain::CheckinResult| {
        // 进度事件同样带 9074 上下文标注，与最终结果列表语义一致。
        let mut annotated = result.clone();
        annotate_device_too_new(
            &mut annotated,
            progress_device_created.get(&result.profile_id).copied(),
        );
        // 回调在 transport 释放前执行，因此此账号租约仍保护结果与额度缓存写回。
        if let Some((profile_id, account_id)) = progress_active_profile.borrow_mut().take() {
            if profile_id == result.profile_id
                && progress_expected_accounts.get(&profile_id) == Some(&account_id)
                && AccountRegistry::new(&material_root)
                    .find(&profile_id)
                    .ok()
                    .flatten()
                    .is_some_and(|current| current.account_id == account_id)
            {
                checkin_overview::update_credits_cache(
                    &material_root,
                    std::slice::from_ref(&annotated),
                );
                let outcome = checkin_overview::checkin_last_attempt_outcome(&annotated);
                checkin_overview::update_last_attempt(&material_root, &profile_id, &outcome);
            }
        }
        if let Some(app) = app.as_ref() {
            completed_seen.set(completed_seen.get() + 1);
            emit_checkin_progress(app, &annotated, progress_names, completed_seen.get(), total);
        }
    };

    let mut bindings = BTreeMap::new();
    // None 表示注册表中无档案（未登录）；Some 为续期阶段的具体错误。
    let mut blocked: Vec<(String, Option<CheckinCredentialError>)> = Vec::new();
    let mut auto_disabled = std::collections::BTreeSet::new();
    let mut retained_leases = BTreeMap::<String, CheckinProfileOperationLease>::new();
    for profile_id in &selected {
        let Some(record) = records.iter().find(|r| &r.profile_id == profile_id) else {
            blocked.push((profile_id.clone(), None));
            continue;
        };
        bindings.insert(profile_id.clone(), CheckinProfileBinding::from(record));
    }

    let store = CheckinCredentialStore::new(&material_root);
    // 需要续期的账号先走真实续期（设备签名 + 身份校验 + 安全写回）。
    let renewal = RealCheckinRenewalService::new(&store);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0);
    let profile_ids = bindings.keys().cloned().collect::<Vec<_>>();
    for profile_id in profile_ids {
        let lease = match store.lock_profile(&profile_id) {
            Ok(lease) => lease,
            Err(error) => {
                blocked.push((profile_id.clone(), Some(error)));
                bindings.remove(&profile_id);
                continue;
            }
        };
        let record = match registry.find(&profile_id) {
            Ok(Some(record)) => record,
            Ok(None) => {
                blocked.push((profile_id.clone(), Some(CheckinCredentialError::Missing)));
                bindings.remove(&profile_id);
                continue;
            }
            Err(_) => {
                blocked.push((profile_id.clone(), Some(CheckinCredentialError::Invalid)));
                bindings.remove(&profile_id);
                continue;
            }
        };
        if expected_accounts.get(&profile_id) != Some(&record.account_id) {
            blocked.push((profile_id.clone(), Some(CheckinCredentialError::Invalid)));
            bindings.remove(&profile_id);
            continue;
        }
        if automatic && !record.auto_checkin_enabled {
            auto_disabled.insert(profile_id.clone());
            bindings.remove(&profile_id);
            continue;
        }
        let binding = CheckinProfileBinding::from(&record);
        if let Err(error) = renewal.renew_if_needed_with_lease(&binding, now, &lease) {
            let synthetic = synthetic_real_checkin_failure(&profile_id, Some(&error));
            let outcome = checkin_overview::checkin_last_attempt_outcome(&synthetic);
            checkin_overview::update_last_attempt(&material_root, &profile_id, &outcome);
            blocked.push((profile_id.clone(), Some(error)));
            bindings.remove(&profile_id);
        } else {
            bindings.insert(profile_id.clone(), binding);
            if automatic {
                retained_leases.insert(profile_id, lease);
            }
        }
    }

    let batch_ids: Vec<String> = selected
        .iter()
        .filter(|profile_id| bindings.contains_key(*profile_id))
        .cloned()
        .collect();

    // 每个账号单独持有 profile 租约直到 status→claim→复核完成；工厂按
    // profile_id 读取最新绑定，避免锁外快照被重登录/重铸覆盖。
    let factory_root = material_root.clone();
    let factory_store = store.clone();
    let factory_active_profile = &active_profile;
    let factory_expected_accounts = &expected_accounts;
    let factory_retained_leases = std::cell::RefCell::new(retained_leases);
    let factory = move |profile_id: &str| -> Box<dyn CheckinTransport> {
        let fallback = || {
            Box::new(RealCheckinTransport::new(&factory_store, BTreeMap::new()))
                as Box<dyn CheckinTransport>
        };
        *factory_active_profile.borrow_mut() = None;
        let lease = match factory_retained_leases
            .borrow_mut()
            .remove(profile_id)
            .map(Ok)
            .unwrap_or_else(|| factory_store.lock_profile(profile_id))
        {
            Ok(lease) => lease,
            Err(_) => return fallback(),
        };
        let record = match AccountRegistry::new(&factory_root).find(profile_id) {
            Ok(Some(record)) => record,
            _ => return fallback(),
        };
        if factory_expected_accounts.get(profile_id) != Some(&record.account_id)
            || (automatic && !record.auto_checkin_enabled)
        {
            return fallback();
        }
        match RealCheckinTransport::new_with_profile_lease(
            &factory_store,
            CheckinProfileBinding::from(&record),
            lease,
        ) {
            Ok(transport) => {
                *factory_active_profile.borrow_mut() =
                    Some((profile_id.to_string(), record.account_id));
                Box::new(transport)
            }
            Err(_) => fallback(),
        }
    };
    // 账号间 3~8 秒随机间隔：模拟多真人错峰操作，降低批量连发风控特征。
    // 阶段事件（checkin-phase）：账号间等待开始时推送一次，前端本地倒计时。
    let inter_app = phase_app;
    let inter_names = phase_names;
    // 账号间等待（inter_wait）：载荷为下一个账号——错峰期它显示"N 秒后开始"。
    let emit_inter_wait = move |profile_id: &str, remaining_secs: u64| {
        let Some(app) = inter_app.as_ref() else {
            return;
        };
        let _ = app.emit(
            "checkin-phase",
            CheckinPhaseEvent {
                profile_id: profile_id.to_string(),
                screen_name: inter_names.get(profile_id).cloned().unwrap_or_default(),
                phase: "inter_wait".to_string(),
                remaining_secs,
            },
        );
    };
    let runner = BatchCheckinRunner::new(&factory)
        .with_inter_account_delay((3000, 8000))
        .with_progress_callback(&emit_progress)
        .with_inter_wait_callback(&emit_inter_wait);
    let mut summary = runner.run(&batch_ids, &cancellation);

    // 按用户选择顺序合并被拦截账号的合成失败结果；取消的条目保持“无结果”语义。
    let blocked_by_profile: BTreeMap<String, Option<CheckinCredentialError>> =
        blocked.into_iter().collect();
    let mut results_by_profile: BTreeMap<String, traesync_domain::CheckinResult> = summary
        .results
        .drain(..)
        .map(|result| (result.profile_id.clone(), result))
        .collect();
    let mut results = Vec::new();
    let mut cancelled = 0usize;
    for profile_id in &selected {
        if let Some(mut result) = results_by_profile.remove(profile_id) {
            // 最终结果列表同样带 9074 上下文标注（与进度事件语义一致）。
            annotate_device_too_new(
                &mut result,
                device_created_by_profile.get(profile_id).copied(),
            );
            results.push(result);
        } else if batch_ids.contains(profile_id) {
            // run_batch 因取消跳过的条目：计入取消，不合成失败结果。
            cancelled += 1;
        } else if auto_disabled.contains(profile_id) {
            cancelled += 1;
        } else {
            let synthetic = synthetic_real_checkin_failure(
                profile_id,
                blocked_by_profile.get(profile_id).and_then(Option::as_ref),
            );
            // 被拦截账号（未登录/续期失败）同样推送进度，前端计数才能收口到 total。
            emit_progress(&synthetic);
            results.push(synthetic);
        }
    }
    let completed = results
        .iter()
        .filter(|result| {
            matches!(
                result.outcome,
                traesync_domain::CheckinOutcome::Claimed
                    | traesync_domain::CheckinOutcome::AlreadyCheckedIn
            )
        })
        .count();
    let summary = traesync_domain::CheckinBatchSummary {
        total: selected.len(),
        completed,
        failed: results.len() - completed,
        cancelled,
        results,
    };
    Ok(summary)
}

/// 9074 上下文标注（ADR-0019 v6）：设备铸造后 5 分钟内被 9074 拒绝时，
/// detail_code 改为 `device_too_new`——2026-09-02 实测（用户B）：
/// 新设备 12 秒首签被拒、192 秒后同设备重试成功，属频率风控而非设备被拉黑，
/// 正确引导是"稍等几分钟再试"而非"重置设备"。设备创建时刻未知（存量
/// 档案值为 0）按旧设备处理，保持 business_9074 引导重置。
fn annotate_device_too_new(
    result: &mut traesync_domain::CheckinResult,
    device_created_at: Option<u64>,
) {
    if result.detail_code.as_deref() != Some("business_9074") {
        return;
    }
    const DEVICE_TRUST_WINDOW_SECS: u64 = 300;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0);
    let is_new_device = device_created_at
        .is_some_and(|created| now.saturating_sub(created) < DEVICE_TRUST_WINDOW_SECS);
    if is_new_device {
        result.detail_code = Some("device_too_new".to_string());
    }
}

/// 为签到前即被拦截的账号合成失败结果（未登录/续期失败/需要恢复）。
fn synthetic_real_checkin_failure(
    profile_id: &str,
    error: Option<&CheckinCredentialError>,
) -> traesync_domain::CheckinResult {
    let (outcome, detail_code) = match error {
        // 注册表无档案：用户尚未完成 OAuth 登录。
        None => (
            traesync_domain::CheckinOutcome::RuntimeError,
            "account_not_logged_in",
        ),
        Some(CheckinCredentialError::BindingMismatch) => (
            traesync_domain::CheckinOutcome::AuthMismatch,
            "binding_mismatch",
        ),
        Some(CheckinCredentialError::CredentialRefreshFailed) => (
            traesync_domain::CheckinOutcome::CredentialRefreshFailed,
            "credential_refresh_failed",
        ),
        Some(CheckinCredentialError::AuthMismatch) => (
            traesync_domain::CheckinOutcome::AuthMismatch,
            "auth_mismatch",
        ),
        Some(CheckinCredentialError::RecoveryRequired) => (
            traesync_domain::CheckinOutcome::RuntimeError,
            "recovery_required",
        ),
        Some(CheckinCredentialError::Missing) => (
            traesync_domain::CheckinOutcome::RuntimeError,
            "credential_missing",
        ),
        Some(CheckinCredentialError::Unavailable) => (
            traesync_domain::CheckinOutcome::RuntimeError,
            "credential_unavailable",
        ),
        Some(CheckinCredentialError::Invalid) => (
            traesync_domain::CheckinOutcome::RuntimeError,
            "credential_invalid",
        ),
    };
    let now = std::time::SystemTime::now();
    traesync_domain::CheckinResult {
        profile_id: profile_id.to_string(),
        outcome,
        state: traesync_domain::CheckinTaskState::Completed,
        claim_attempted: false,
        before: None,
        after: None,
        detail_code: Some(detail_code.to_string()),
        started_at: now,
        finished_at: now,
    }
}

/// 请求停止尚未开始的签到任务；当前 transport 不强行中断已发出的任务。
#[tauri::command]
pub(crate) fn cancel_checkin(state: tauri::State<AppState>) -> bool {
    state.checkin_cancellation.store(true, Ordering::Release);
    true
}

/// 账号总览（只读聚合）：档案 + 积分缓存 + 令牌到期 + 设备尾号。
/// 供账号页富信息卡片展示；不发起任何网络请求（P1-3：离线显示缓存值）。
#[tauri::command]
pub(crate) fn get_checkin_overview(
    state: tauri::State<AppState>,
) -> Result<Vec<checkin_overview::CheckinOverviewEntryDto>, String> {
    let material_root = checkin_material_root(&state)?;
    checkin_overview::build_overview(&material_root)
}

/// 批量操作逐账号进度事件统一契约（P12-R2b）：三条批量链（凭据刷新
/// `credential-refresh-progress`、健康检测 `health-check-progress`、
/// 积分/额度刷新 `credits-refresh-progress`）共用单一判别枚举，事件名保持
/// 不变；serde `tag = "kind"` 区分链路，kind 字面值与前端
/// `src/types/batchProgress.ts` 的 BatchProgressKind 逐一相等。
/// 载荷为非敏感白名单，不含 token 或任何凭据材料。
/// pub(crate)：凭据刷新/健康检测/积分刷新链留守 lib.rs，经转发导入引用。
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum BatchProgressEvent {
    /// 凭据刷新链：批量手动刷新每完成一个账号推送一次。
    CredentialRefresh {
        profile_id: String,
        screen_name: String,
        /// 已处理账号数（含失败，1 起）。
        completed: usize,
        /// 批次账号总数。
        total: usize,
        /// 该账号本次换发是否成功。
        refreshed: bool,
        /// 失败原因码；成功为 None（序列化保留 null）。
        error_code: Option<String>,
    },
    /// 健康检测链：批量健康检测每完成一个账号推送一次。
    HealthCheck {
        profile_id: String,
        screen_name: String,
        /// 已检测账号数（含失败，1 起）。
        completed: usize,
        /// 批次账号总数。
        total: usize,
        /// 该账号健康检测是否通过。
        healthy: bool,
        /// 失败原因码；成功为 None（序列化保留 null）。
        error_code: Option<String>,
    },
    /// 积分/额度刷新链：只读刷新每完成一个账号推送一次，结果字段与
    /// `CreditsRefreshEntryDto` 同源，均为非敏感展示值。
    CreditsRefresh {
        profile_id: String,
        screen_name: String,
        /// 已刷新账号数（含失败，1 起）。
        completed: usize,
        /// 批次账号总数。
        total: usize,
        /// 该账号本次刷新是否成功。
        ok: bool,
        /// 最新积分；失败或未取到为 None。
        credits: Option<i64>,
        /// 最新「今日已签」状态；失败为 None。
        checked_in: Option<bool>,
        /// 额度剩余；失败或未取到为 None。
        usage_remaining_credits: Option<f64>,
        /// 失败原因码；成功为 None（序列化保留 null）。
        error_code: Option<String>,
    },
}

/// 在原进度载荷中加入产品来源；`flatten` 保持 `kind` 与进度字段处于同一层。
#[derive(Debug, Clone, Serialize)]
pub(crate) struct ProductBatchProgressEvent<'a> {
    product_id: &'a str,
    #[serde(flatten)]
    event: BatchProgressEvent,
}

impl<'a> ProductBatchProgressEvent<'a> {
    pub(crate) fn new(product_id: &'a str, event: BatchProgressEvent) -> Self {
        Self { product_id, event }
    }
}

/// 泛型批量运行器（P12-R2b）：三条批量链共享的「串行循环 + 逐账号进度事件
/// 发射」骨架。内建语义（CONTEXT.md 既有约束，行为不变量）：
/// - 串行逐账号执行，账号间不并发；
/// - 单账号失败不阻断批次（失败由链闭包编码进回执与事件的 error_code）；
/// - completed（含失败，1 起）逐账号递增，total 为批次账号总数；
/// - 档案缺失/失败账号也各发一次进度事件。
///
/// 档案查找由运行器统一执行；链闭包接收（profile_id、档案（缺失为 None）、
/// completed、total），返回（链内回执、该账号进度事件）。闭包返回 Err 表示
/// 整批中止的存储故障（如凭据包写回失败），直接向上传播且该账号不发事件，
/// 与既有 `?` 提前返回语义一致。
pub(crate) fn run_serial_batch<E, F>(
    selected: &[String],
    records: &[AccountRecord],
    mut on_progress: impl FnMut(BatchProgressEvent),
    mut step: F,
) -> Result<Vec<E>, String>
where
    F: FnMut(&str, Option<&AccountRecord>, usize, usize) -> Result<(E, BatchProgressEvent), String>,
{
    let total = selected.len();
    let mut entries = Vec::with_capacity(selected.len());
    for (index, profile_id) in selected.iter().enumerate() {
        let completed = index + 1;
        let record = records
            .iter()
            .find(|record| &record.profile_id == profile_id);
        let (entry, event) = step(profile_id, record, completed, total)?;
        on_progress(event);
        entries.push(entry);
    }
    Ok(entries)
}

/// P12-R2b 统一事件枚举与泛型批量运行器的契约测试：
/// kind 字面值与前端 `src/types/batchProgress.ts` 的 BatchProgressKind
/// 三个字符串精确相等（Q2 同批对齐）；运行器内建串行、失败不阻断、
/// completed（含失败，1 起）递增、空批次不发射语义。
#[cfg(test)]
mod batch_runner_tests {
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

    /// kind 字面值必须与前端契约逐一相等；不匹配的 kind 会被前端静默丢弃。
    #[test]
    fn batch_progress_event_kind_literals_match_frontend_contract() {
        let refresh = BatchProgressEvent::CredentialRefresh {
            profile_id: "p".to_string(),
            screen_name: String::new(),
            completed: 1,
            total: 1,
            refreshed: true,
            error_code: None,
        };
        let health = BatchProgressEvent::HealthCheck {
            profile_id: "p".to_string(),
            screen_name: String::new(),
            completed: 1,
            total: 1,
            healthy: true,
            error_code: None,
        };
        let credits = BatchProgressEvent::CreditsRefresh {
            profile_id: "p".to_string(),
            screen_name: String::new(),
            completed: 1,
            total: 1,
            ok: true,
            credits: None,
            checked_in: None,
            usage_remaining_credits: None,
            error_code: None,
        };
        assert_eq!(
            serde_json::to_value(&refresh).unwrap()["kind"],
            "credential_refresh"
        );
        assert_eq!(
            serde_json::to_value(&health).unwrap()["kind"],
            "health_check"
        );
        assert_eq!(
            serde_json::to_value(&credits).unwrap()["kind"],
            "credits_refresh"
        );
    }

    /// 公共字段齐全；error_code 为 None 时序列化保留 null（前端类型 string | null，
    /// 不得被 skip）；载荷维持非敏感白名单。
    #[test]
    fn batch_progress_event_common_fields_and_null_error_code() {
        let credits = BatchProgressEvent::CreditsRefresh {
            profile_id: "profile-1".to_string(),
            screen_name: "账号一".to_string(),
            completed: 2,
            total: 5,
            ok: false,
            credits: None,
            checked_in: None,
            usage_remaining_credits: None,
            error_code: None,
        };
        let json = serde_json::to_value(&credits).unwrap();
        assert_eq!(json["profile_id"], "profile-1");
        assert_eq!(json["screen_name"], "账号一");
        assert_eq!(json["completed"], 2);
        assert_eq!(json["total"], 5);
        assert!(json.get("error_code").is_some());
        assert!(json["error_code"].is_null());
        // 非敏感白名单：不含任何凭据材料字段。
        assert!(json.get("access_token").is_none());
        assert!(json.get("refresh_token").is_none());
        assert!(json.get("device_private_key").is_none());
    }

    /// 运行器：串行顺序处理；缺失档案不阻断后续账号；completed 含缺失
    /// 1 起递增、total 为批次总数；每账号各发一次事件；回执保持输入顺序。
    #[test]
    fn batch_runner_runs_serially_and_continues_after_failure() {
        let records = vec![
            record("profile-a", "account-a", "账号甲"),
            record("profile-b", "account-b", "账号乙"),
        ];
        let selected = vec![
            "profile-a".to_string(),
            "profile-missing".to_string(),
            "profile-b".to_string(),
        ];
        let mut calls: Vec<String> = Vec::new();
        let mut events: Vec<BatchProgressEvent> = Vec::new();
        let entries: Vec<String> = run_serial_batch(
            &selected,
            &records,
            |event| events.push(event),
            |profile_id, record, completed, total| {
                calls.push(profile_id.to_string());
                let (healthy, error_code) = match record {
                    Some(_) => (true, None),
                    None => (false, Some("credential_missing".to_string())),
                };
                Ok((
                    profile_id.to_string(),
                    BatchProgressEvent::HealthCheck {
                        profile_id: profile_id.to_string(),
                        screen_name: record.map(|r| r.screen_name.clone()).unwrap_or_default(),
                        completed,
                        total,
                        healthy,
                        error_code,
                    },
                ))
            },
        )
        .unwrap();

        // 串行顺序与输入一致；缺失档案也执行且不阻断后续账号。
        assert_eq!(
            calls,
            vec![
                "profile-a".to_string(),
                "profile-missing".to_string(),
                "profile-b".to_string()
            ]
        );
        // 回执顺序保持输入顺序。
        assert_eq!(
            entries,
            vec![
                "profile-a".to_string(),
                "profile-missing".to_string(),
                "profile-b".to_string()
            ]
        );
        // 每账号各发一次事件，completed 含缺失 1 起递增。
        assert_eq!(events.len(), 3);
        for (index, event) in events.iter().enumerate() {
            let BatchProgressEvent::HealthCheck {
                profile_id,
                completed,
                total,
                ..
            } = event
            else {
                panic!("应为健康检测链事件");
            };
            assert_eq!(*completed, index + 1);
            assert_eq!(*total, 3);
            assert_eq!(profile_id, &selected[index]);
        }
        // 缺失账号事件如实带 credential_missing。
        let BatchProgressEvent::HealthCheck { error_code, .. } = &events[1] else {
            panic!("应为健康检测链事件");
        };
        assert_eq!(error_code.as_deref(), Some("credential_missing"));
    }

    /// 运行器：空批次不执行任何账号、不发射任何事件，返回空回执。
    #[test]
    fn batch_runner_empty_selection_emits_no_events() {
        let mut events: Vec<BatchProgressEvent> = Vec::new();
        let entries: Vec<String> = run_serial_batch(
            &[] as &[String],
            &[] as &[AccountRecord],
            |event| events.push(event),
            |_profile_id, _record, _completed, _total| {
                panic!("空批次不应执行任何账号闭包");
            },
        )
        .unwrap();
        assert!(entries.is_empty());
        assert!(events.is_empty());
    }
}

#[cfg(test)]
mod progress_event_contract_tests {
    use super::*;

    #[test]
    fn progress_events_serialize_non_sensitive_payload_shape() {
        let refresh = ProductBatchProgressEvent::new(
            "work_cn",
            BatchProgressEvent::CredentialRefresh {
                profile_id: "profile-1".to_string(),
                screen_name: "账号一".to_string(),
                completed: 1,
                total: 2,
                refreshed: true,
                error_code: None,
            },
        );
        let health = ProductBatchProgressEvent::new(
            "trae_cn",
            BatchProgressEvent::HealthCheck {
                profile_id: "profile-2".to_string(),
                screen_name: "账号二".to_string(),
                completed: 2,
                total: 2,
                healthy: false,
                error_code: Some("credential_invalid".to_string()),
            },
        );
        let refresh_json = serde_json::to_value(&refresh).unwrap();
        assert_eq!(refresh_json["product_id"], "work_cn");
        assert_eq!(refresh_json["kind"], "credential_refresh");
        assert_eq!(refresh_json["profile_id"], "profile-1");
        assert_eq!(refresh_json["screen_name"], "账号一");
        assert_eq!(refresh_json["completed"], 1);
        assert_eq!(refresh_json["total"], 2);
        assert_eq!(refresh_json["refreshed"], true);
        assert!(refresh_json["error_code"].is_null());
        let health_json = serde_json::to_value(&health).unwrap();
        assert_eq!(health_json["product_id"], "trae_cn");
        assert_eq!(health_json["kind"], "health_check");
        assert_eq!(health_json["profile_id"], "profile-2");
        assert_eq!(health_json["completed"], 2);
        assert_eq!(health_json["total"], 2);
        assert_eq!(health_json["healthy"], false);
        assert_eq!(health_json["error_code"], "credential_invalid");
        // 两种载荷都不含任何凭据材料字段。
        for payload in [&refresh_json, &health_json] {
            assert!(payload.get("access_token").is_none());
            assert!(payload.get("refresh_token").is_none());
            assert!(payload.get("device_private_key").is_none());
        }
    }

    #[test]
    fn credits_refresh_progress_serializes_non_sensitive_payload_shape() {
        let event = ProductBatchProgressEvent::new(
            "work_cn",
            BatchProgressEvent::CreditsRefresh {
                profile_id: "profile-1".to_string(),
                screen_name: "账号一".to_string(),
                completed: 1,
                total: 2,
                ok: true,
                credits: Some(200),
                checked_in: Some(false),
                usage_remaining_credits: Some(80.5),
                error_code: None,
            },
        );
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["product_id"], "work_cn");
        assert_eq!(json["kind"], "credits_refresh");
        assert_eq!(json["profile_id"], "profile-1");
        assert_eq!(json["screen_name"], "账号一");
        assert_eq!(json["completed"], 1);
        assert_eq!(json["total"], 2);
        assert_eq!(json["ok"], true);
        assert_eq!(json["credits"], 200);
        assert_eq!(json["checked_in"], false);
        assert_eq!(json["usage_remaining_credits"], 80.5);
        assert!(json["error_code"].is_null());
        // 非敏感白名单：不含任何凭据材料字段。
        assert!(json.get("access_token").is_none());
        assert!(json.get("refresh_token").is_none());
        assert!(json.get("device_private_key").is_none());
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
    fn automatic_maintenance_persists_failure_backoff_before_next_attempt() {
        let root = tempfile::tempdir().unwrap();
        AccountRegistry::new(root.path())
            .upsert(&record(
                "profile-missing",
                "account-missing",
                "缺少凭据账号",
            ))
            .unwrap();
        let execution_lock = std::sync::Arc::new(std::sync::Mutex::new(()));

        // 缺失凭据不会触碰网络，但必须形成可跨重启读取的退避记录。
        assert!(maintain_all_credentials(root.path(), &execution_lock));
        let state_path = root.path().join("credential-maintenance.json");
        let first: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&state_path).unwrap()).unwrap();
        assert_eq!(first["accounts"]["profile-missing"]["attempts_today"], 1);

        // 同一退避窗口内第二次维护不应重新尝试该账号，计数保持不变。
        assert!(maintain_all_credentials(root.path(), &execution_lock));
        let second: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&state_path).unwrap()).unwrap();
        assert_eq!(second["accounts"]["profile-missing"]["attempts_today"], 1);
    }
}

#[cfg(test)]
mod real_checkin_tests {
    use super::*;
    // P12-R3：AppState 构造夹具与注册表档案夹具为跨域共享，提至 test_support。
    use crate::test_support::{make_real_mode_state, registered_account};

    /// 测试包装：由 state 推导 material_root，无进度推送、无取消。
    /// run_real_checkin 签名演进（异步化 + 进度事件参数）后保持既有测试语义。
    fn run_real_checkin_for_test(
        state: &AppState,
        selected: &[String],
    ) -> Result<traesync_domain::CheckinBatchSummary, String> {
        run_real_checkin_for_test_mode(state, selected, false)
    }

    fn run_real_checkin_for_test_mode(
        state: &AppState,
        selected: &[String],
        automatic: bool,
    ) -> Result<traesync_domain::CheckinBatchSummary, String> {
        let material_root = checkin_material_root(state)?;
        run_real_checkin(
            material_root,
            selected.to_vec(),
            Arc::new(AtomicBool::new(false)),
            None,
            automatic,
        )
    }

    #[test]
    fn real_checkin_rejects_when_storage_root_unavailable() {
        // 生产启动失败时 storage_root 为空：真实签到必须失败关闭。
        let state = make_real_mode_state(Path::new(""));
        let error = run_real_checkin_for_test(&state, &["profile-a".to_string()]).unwrap_err();
        assert_eq!(error, "checkin_storage_unavailable");
    }

    #[test]
    fn real_checkin_reports_not_logged_in_without_registry() {
        // 注册表不存在（从未登录）：每个账号按“未登录”失败收口，不发起网络请求。
        let root = tempfile::tempdir().unwrap();
        let state = make_real_mode_state(root.path());
        let selected = vec!["profile-a".to_string(), "profile-b".to_string()];
        let summary = run_real_checkin_for_test(&state, &selected).unwrap();
        assert_eq!(summary.total, 2);
        assert_eq!(summary.completed, 0);
        assert_eq!(summary.failed, 2);
        assert_eq!(summary.cancelled, 0);
        assert_eq!(summary.results.len(), 2);
        for result in &summary.results {
            assert_eq!(result.detail_code.as_deref(), Some("account_not_logged_in"));
            assert!(!result.claim_attempted);
            assert_eq!(result.state, traesync_domain::CheckinTaskState::Completed);
            // 拦截路径不产生任何状态快照。
            assert!(result.before.is_none());
            assert!(result.after.is_none());
        }
    }

    #[test]
    fn real_checkin_reports_missing_credential_for_registered_profile() {
        // 注册表有档案但凭据密文缺失（如被手动删除）：续期阶段失败收口。
        let root = tempfile::tempdir().unwrap();
        registered_account(root.path(), "profile-a");
        let state = make_real_mode_state(root.path());
        let summary = run_real_checkin_for_test(&state, &["profile-a".to_string()]).unwrap();
        assert_eq!(summary.total, 1);
        assert_eq!(summary.failed, 1);
        assert_eq!(summary.results.len(), 1);
        assert_eq!(
            summary.results[0].detail_code.as_deref(),
            Some("credential_missing")
        );
        assert!(!summary.results[0].claim_attempted);
    }

    #[test]
    fn automatic_checkin_skips_account_disabled_after_stagger_snapshot() {
        let root = tempfile::tempdir().unwrap();
        registered_account(root.path(), "profile-a");
        let registry = AccountRegistry::new(root.path().join("checkin"));
        assert!(registry
            .set_auto_checkin_enabled("profile-a", false)
            .unwrap());
        let state = make_real_mode_state(root.path());

        let summary =
            run_real_checkin_for_test_mode(&state, &["profile-a".to_string()], true).unwrap();

        assert_eq!(summary.total, 1);
        assert_eq!(summary.completed, 0);
        assert_eq!(summary.failed, 0);
        assert_eq!(summary.cancelled, 1);
        assert!(summary.results.is_empty());
    }

    #[test]
    fn real_checkin_merges_blocked_and_batch_counts() {
        // 混合选择：一个未登录 + 一个已登录但凭据缺失——均合成失败并保持总数语义。
        let root = tempfile::tempdir().unwrap();
        registered_account(root.path(), "profile-registered");
        let state = make_real_mode_state(root.path());
        let selected = vec![
            "profile-registered".to_string(),
            "profile-unknown".to_string(),
        ];
        let summary = run_real_checkin_for_test(&state, &selected).unwrap();
        assert_eq!(summary.total, 2);
        assert_eq!(summary.failed, 2);
        assert_eq!(summary.completed, 0);
        // 结果按用户选择顺序返回。
        assert_eq!(summary.results[0].profile_id, "profile-registered");
        assert_eq!(
            summary.results[0].detail_code.as_deref(),
            Some("credential_missing")
        );
        assert_eq!(summary.results[1].profile_id, "profile-unknown");
        assert_eq!(
            summary.results[1].detail_code.as_deref(),
            Some("account_not_logged_in")
        );
    }

    #[test]
    fn real_checkin_rejects_invalid_registry() {
        // 注册表损坏（非 JSON）：整体失败关闭，不猜测任何账号状态。
        let root = tempfile::tempdir().unwrap();
        let checkin_root = root.path().join("checkin");
        std::fs::create_dir_all(&checkin_root).unwrap();
        std::fs::write(checkin_root.join("accounts.json"), b"not json").unwrap();
        let state = make_real_mode_state(root.path());
        let error = run_real_checkin_for_test(&state, &["profile-a".to_string()]).unwrap_err();
        assert_eq!(error, "checkin_registry_invalid");
    }

    #[test]
    fn real_checkin_capability_reports_real_transport_when_ready() {
        // 真实模式且存储根可用：上报真实 transport；存储根缺失时保持禁用。
        let root = tempfile::tempdir().unwrap();
        let state = make_real_mode_state(root.path());
        let capability = checkin_capability_inner(&state);
        assert!(capability.enabled);
        assert_eq!(capability.transport, "real");
        assert!(capability.real_http_enabled);

        let unavailable = make_real_mode_state(Path::new(""));
        let disabled = checkin_capability_inner(&unavailable);
        assert!(!disabled.enabled);
        assert!(!disabled.real_http_enabled);
    }

    #[test]
    fn fixture_capability_unchanged() {
        // fixture 模式能力上报保持原语义，现有 UI/E2E 不受真实分支影响。
        let mut state = make_real_mode_state(Path::new("/nonexistent"));
        state.runtime_mode = RuntimeMode::Fixture;
        let capability = checkin_capability_inner(&state);
        assert!(capability.enabled);
        assert_eq!(capability.transport, "fixture");
        assert!(!capability.real_http_enabled);
    }

    /// v6：9074 上下文标注——设备铸造后 5 分钟内被拒报 device_too_new，
    /// 旧设备/未知时刻保持 business_9074，其他业务码不受影响。
    #[test]
    fn annotate_device_too_new_distinguishes_fresh_and_old_device() {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();

        fn result_with(detail: &str) -> traesync_domain::CheckinResult {
            traesync_domain::CheckinResult {
                profile_id: "p".to_string(),
                outcome: traesync_domain::CheckinOutcome::NotEligible,
                state: traesync_domain::CheckinTaskState::Completed,
                claim_attempted: true,
                before: None,
                after: None,
                detail_code: Some(detail.to_string()),
                started_at: std::time::SystemTime::now(),
                finished_at: std::time::SystemTime::now(),
            }
        }

        // 新设备（60 秒前铸造）：改报 device_too_new，引导"稍后重试"。
        let mut fresh = result_with("business_9074");
        annotate_device_too_new(&mut fresh, Some(now - 60));
        assert_eq!(fresh.detail_code.as_deref(), Some("device_too_new"));

        // 旧设备（1 小时前铸造）：保持 business_9074，引导"重置设备"。
        let mut old = result_with("business_9074");
        annotate_device_too_new(&mut old, Some(now - 3600));
        assert_eq!(old.detail_code.as_deref(), Some("business_9074"));

        // 存量档案（时刻未知）：按旧设备处理。
        let mut unknown = result_with("business_9074");
        annotate_device_too_new(&mut unknown, None);
        assert_eq!(unknown.detail_code.as_deref(), Some("business_9074"));

        // 其他业务码不受影响。
        let mut other = result_with("business_9095");
        annotate_device_too_new(&mut other, Some(now - 10));
        assert_eq!(other.detail_code.as_deref(), Some("business_9095"));
    }
}
