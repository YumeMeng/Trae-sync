//! P17-A5：WorkBuddy 自动签到调度（与 Trae 调度器同策略的并列实现）。
//!
//! 策略对齐（spec 10.3：与 Trae 账号同策略）：
//! - **三级触发判定**复用 Trae 侧 infrastructure 纯函数（常规触发/中断
//!   恢复/顺延重试），设置与台账各用一份独立的
//!   [`traesync_infrastructure::auto_checkin_store::AutoCheckinStore`]，
//!   存储在 `workbuddy-schedule` 根（产品并列纪律：调度状态独立成根）；
//! - **断连兜底**：逐账号 claim 前先续期该账号（`open_session_for_profile`
//!   内建：access 充裕跳过不轮换，临近过期刷新 + 原子写回新对）；
//! - **失败重试/日志与 Trae 同策略**：真实失败当日不自动重试（决策 5 与
//!   ADR-0019 第 8 条口径），只对「可顺延重试」的跳过做 30 分钟顺延——
//!   WorkBuddy 批次无 execution-lock 撞锁场景（claim 是纯 HTTP，无
//!   profile 租约），因此 skipped 恒为 0，顺延重试分支天然静默；
//! - **错峰**：首发批次 0~15 分钟随机错峰（`build_staggered_plan`），
//!   补跑/重试顺序直跑；
//! - **诊断日志**：与 Trae 同口径——进度事件（workbuddy-auto-checkin-
//!   progress/finished）+ 失败 detail_code 携带稳定错误码；令牌零外显。
//!
//! 账号参与集：WorkBuddy 池记录无 per-account 开关字段，参与自动签到的
//! 账号集合存调度根 `workbuddy-auto-checkin-accounts.json`（tmp+rename
//! 原子写）；参与集与池容器求交——已从池清理的账号自动失效。

use std::path::{Path, PathBuf};

use chrono::Timelike;
use serde::Serialize;
use tauri::Emitter;

use traesync_domain::CheckinOutcome;
use traesync_infrastructure::workbuddy_checkin::{
    open_session_for_profile, WorkBuddyCheckinOutcome, WorkBuddyPoolError,
};
use traesync_infrastructure::{
    AutoCheckinBatchState, AutoCheckinLedger, AutoCheckinSettings, AutoCheckinStore,
};

use crate::checkin_schedule::AutoCheckinLedgerWireDto;

/// 调度 tick 间隔（与 Trae 调度器同频 30 秒）。
const AUTO_CHECKIN_TICK_SECONDS: u64 = 30;

/// 参与集文件名（调度根内）。
const PARTICIPATION_FILE: &str = "workbuddy-auto-checkin-accounts.json";
/// 参与集文件上限（账号数天然有限；防御异常膨胀）。
const MAX_PARTICIPATION_BYTES: u64 = 64 * 1024;

/// 账号参与集存储（调度根内单 JSON；损坏时报错不静默重建——配置文件是
/// 用户决策的证据，与 Trae 设置同纪律）。
struct WorkBuddyParticipationStore {
    path: PathBuf,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct ParticipationFile {
    format_version: u32,
    #[serde(default)]
    enabled_profiles: Vec<String>,
}

impl WorkBuddyParticipationStore {
    fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            path: root.into().join(PARTICIPATION_FILE),
        }
    }

    /// 读取参与集；文件不存在返回空集（首次使用）。
    fn load(&self) -> Result<Vec<String>, ()> {
        let content = match std::fs::read_to_string(&self.path) {
            Ok(content) => content,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Vec::new());
            }
            Err(_) => return Err(()),
        };
        let file: ParticipationFile =
            serde_json::from_str(&content).map_err(|_| ())?;
        if file.format_version != 1 {
            return Err(());
        }
        Ok(file.enabled_profiles)
    }

    /// 原子保存（tmp + rename；失败保留原文件）。
    fn save(&self, enabled_profiles: &[String]) -> Result<(), ()> {
        let file = ParticipationFile {
            format_version: 1,
            enabled_profiles: enabled_profiles.to_vec(),
        };
        let content = serde_json::to_string_pretty(&file).map_err(|_| ())?;
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent).map_err(|_| ())?;
        }
        let mut tmp_name = self
            .path
            .file_name()
            .map(|name| name.to_os_string())
            .unwrap_or_default();
        tmp_name.push(".tmp");
        let temporary = self.path.with_file_name(tmp_name);
        std::fs::write(&temporary, content).map_err(|_| ())?;
        std::fs::rename(&temporary, &self.path).map_err(|_| ())
    }
}

/// 调度根下的设置/台账存储（与 Trae 同一 store 类型，独立实例独立文件）。
fn schedule_store(schedule_root: &Path) -> AutoCheckinStore {
    AutoCheckinStore::new(schedule_root)
}

/// WorkBuddy 自动签到状态回执（设置页/签到页数据源）。
#[derive(Debug, Clone, Serialize)]
pub(crate) struct WorkBuddyAutoCheckinStatusDto {
    pub enabled: bool,
    pub daily_time_hhmm: String,
    pub ledger: Option<AutoCheckinLedgerWireDto>,
    /// 池内账号与其参与状态（默认不参与；UI 逐账号开关）。
    pub accounts: Vec<WorkBuddyAutoAccountDto>,
}

/// 参与自动签到的账号条目。
#[derive(Debug, Clone, Serialize)]
pub(crate) struct WorkBuddyAutoAccountDto {
    pub profile_id: String,
    pub screen_name: String,
    pub enabled: bool,
}

/// WorkBuddy 自动签到状态（签到页数据源；真实模式门禁）。
#[tauri::command]
pub(crate) fn get_workbuddy_auto_checkin_status(
    state: tauri::State<'_, crate::AppState>,
) -> Result<WorkBuddyAutoCheckinStatusDto, String> {
    crate::ensure_login_real_mode_workbuddy(&state)?;
    let (schedule_root, login_root) = workbuddy_roots(&state)?;
    Ok(workbuddy_auto_checkin_status(&schedule_root, &login_root))
}

/// 保存 WorkBuddy 自动签到设置（总开关 + 触发时间）。
#[tauri::command]
pub(crate) fn set_workbuddy_auto_checkin_settings(
    enabled: bool,
    daily_time_hhmm: String,
    state: tauri::State<'_, crate::AppState>,
) -> Result<(), String> {
    crate::ensure_login_real_mode_workbuddy(&state)?;
    let (schedule_root, _login_root) = workbuddy_roots(&state)?;
    save_workbuddy_auto_checkin_settings(&schedule_root, enabled, &daily_time_hhmm)
}

/// 设置单个账号的自动签到参与状态。
#[tauri::command]
pub(crate) fn set_workbuddy_account_auto_checkin(
    profile_id: String,
    enabled: bool,
    state: tauri::State<'_, crate::AppState>,
) -> Result<(), String> {
    crate::ensure_login_real_mode_workbuddy(&state)?;
    let (schedule_root, login_root) = workbuddy_roots(&state)?;
    set_workbuddy_account_participation(&schedule_root, &login_root, &profile_id, enabled)
}

/// 调度根与池根（storage_root 为空拒绝——调度数据必须有落位）。
fn workbuddy_roots(
    state: &crate::AppState,
) -> Result<(PathBuf, PathBuf), String> {
    if state.storage_root.is_empty() {
        return Err("workbuddy_storage_unavailable".to_string());
    }
    let root = Path::new(&state.storage_root);
    Ok((
        traesync_infrastructure::storage_roots::workbuddy_schedule_root_from(root),
        traesync_infrastructure::storage_roots::workbuddy_login_root_from(root),
    ))
}

/// 读取状态（fail-soft：调度数据读失败按默认值返回，不阻塞 UI）。
pub(crate) fn workbuddy_auto_checkin_status(
    schedule_root: &Path,
    login_root: &Path,
) -> WorkBuddyAutoCheckinStatusDto {
    let store = schedule_store(schedule_root);
    let (settings, ledger) = store.load().unwrap_or_default();
    let participation = WorkBuddyParticipationStore::new(schedule_root)
        .load()
        .unwrap_or_default();
    let accounts = pool_summaries(login_root)
        .into_iter()
        .map(|(profile_id, screen_name)| WorkBuddyAutoAccountDto {
            enabled: participation.iter().any(|id| id == &profile_id),
            profile_id,
            screen_name,
        })
        .collect();
    WorkBuddyAutoCheckinStatusDto {
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
        accounts,
    }
}

/// 保存总开关与触发时间（沿用 Trae 校验：HH:MM 格式 fail-closed）。
pub(crate) fn save_workbuddy_auto_checkin_settings(
    schedule_root: &Path,
    enabled: bool,
    daily_time_hhmm: &str,
) -> Result<(), String> {
    if !traesync_infrastructure::auto_checkin_store::is_valid_hhmm(daily_time_hhmm) {
        return Err("workbuddy_auto_checkin_invalid_time".to_string());
    }
    let store = schedule_store(schedule_root);
    let (mut settings, ledger) = store.load().map_err(|_| "workbuddy_schedule_invalid".to_string())?;
    settings.enabled = enabled;
    settings.daily_time_hhmm = daily_time_hhmm.to_string();
    store
        .save_settings(&settings)
        .map_err(|_| "workbuddy_schedule_save_failed".to_string())?;
    // 台账不随设置变化（当日执行情况是事实记录）。
    let _ = ledger;
    Ok(())
}

/// 设置单个账号的参与状态（账号必须仍在池内——防残留 id 复活）。
pub(crate) fn set_workbuddy_account_participation(
    schedule_root: &Path,
    login_root: &Path,
    profile_id: &str,
    enabled: bool,
) -> Result<(), String> {
    let store = WorkBuddyParticipationStore::new(schedule_root);
    let mut participation = store.load().map_err(|_| "workbuddy_schedule_invalid".to_string())?;
    let in_pool = pool_summaries(login_root)
        .iter()
        .any(|(id, _)| id == profile_id);
    if !in_pool {
        return Err("workbuddy_auto_checkin_account_missing".to_string());
    }
    if enabled {
        if !participation.iter().any(|id| id == profile_id) {
            participation.push(profile_id.to_string());
        }
    } else {
        participation.retain(|id| id != profile_id);
    }
    store
        .save(&participation)
        .map_err(|_| "workbuddy_schedule_save_failed".to_string())
}

/// 池内账号清单（profile_id, screen_name）；池读取失败按空处理（fail-soft）。
fn pool_summaries(login_root: &Path) -> Vec<(String, String)> {
    use traesync_infrastructure::workbuddy_login_credential::WorkBuddyLoginStore;
    WorkBuddyLoginStore::new(login_root.to_path_buf())
        .list()
        .unwrap_or_default()
        .into_iter()
        .map(|summary| (summary.profile_id, summary.screen_name))
        .collect()
}

/// 筛选参与自动签到的账号（参与集 ∩ 池内现存账号；池清理自动失效）。
fn workbuddy_eligible_ids(schedule_root: &Path, login_root: &Path) -> Vec<String> {
    let participation = WorkBuddyParticipationStore::new(schedule_root)
        .load()
        .unwrap_or_default();
    pool_summaries(login_root)
        .into_iter()
        .filter(|(profile_id, _)| participation.iter().any(|id| id == profile_id))
        .map(|(profile_id, _)| profile_id)
        .collect()
}

/// 调度线程入口（仅真实模式启动；由组合根 setup 钩子调用）。30 秒 tick，
/// 与 Trae 调度器同频；WB 无凭据维护链（续期内嵌在逐账号 claim 前）。
pub(crate) fn spawn_workbuddy_auto_checkin_scheduler(
    app: tauri::AppHandle,
    storage_root: String,
) {
    std::thread::spawn(move || {
        if storage_root.is_empty() {
            return;
        }
        let schedule_root = traesync_infrastructure::storage_roots::workbuddy_schedule_root_from(
            Path::new(&storage_root),
        );
        let login_root = traesync_infrastructure::storage_roots::workbuddy_login_root_from(
            Path::new(&storage_root),
        );
        loop {
            std::thread::sleep(std::time::Duration::from_secs(AUTO_CHECKIN_TICK_SECONDS));
            run_workbuddy_tick(&app, &schedule_root, &login_root);
        }
    });
}

/// 单次调度 tick：三级触发判定与 Trae 同构（复用 infrastructure 纯函数），
/// 台账/设置独立于 Trae（产品并列纪律）。
fn run_workbuddy_tick(app: &tauri::AppHandle, schedule_root: &Path, login_root: &Path) {
    let store = schedule_store(schedule_root);
    let Ok(Some(_batch_lease)) = store.try_acquire_batch_execution_lease() else {
        return;
    };
    let Ok((settings, ledger)) = store.load() else {
        return;
    };
    if !settings.enabled {
        return;
    }
    let now = chrono::Local::now();
    let today = now.format("%Y-%m-%d").to_string();
    let now_minutes = now.hour() * 60 + now.minute();

    // 1) 常规触发：今日未发起且已到时间点 → 错峰首发。
    if traesync_infrastructure::should_trigger_auto_checkin(
        &settings,
        ledger.as_ref(),
        &today,
        now_minutes,
    ) {
        let eligible_ids = workbuddy_eligible_ids(schedule_root, login_root);
        let mut ledger = fresh_workbuddy_ledger(today.clone(), eligible_ids.len());
        let _ = store.save_ledger(&ledger);
        match traesync_infrastructure::build_staggered_plan(eligible_ids) {
            Ok(plan) => {
                run_workbuddy_batch(
                    app,
                    login_root,
                    &store,
                    &mut ledger,
                    plan,
                    &production_claimer(login_root.to_path_buf()),
                );
            }
            Err(_) => return,
        }
        return;
    }

    // 2) 中断恢复：今日台账 Running = 上次进程中断遗留 → 顺序补跑。
    if traesync_infrastructure::should_recover_interrupted_checkin(ledger.as_ref(), &today) {
        let eligible_ids = workbuddy_eligible_ids(schedule_root, login_root);
        let mut ledger = fresh_workbuddy_ledger(today.clone(), eligible_ids.len());
        let _ = store.save_ledger(&ledger);
        let plan = eligible_ids.into_iter().map(|id| (id, 0u64)).collect();
        run_workbuddy_batch(app, login_root, &store, &mut ledger, plan, &production_claimer(login_root.to_path_buf()));
        return;
    }

    // 3) 顺延重试：WorkBuddy 批次无撞锁跳过场景（skipped 恒 0），
    //    该分支按 Trae 同构保留——skipped 非空时同样顺延（未来若引入
    //    可重试跳过语义，此处零改动生效）。
    let now_unix = now.timestamp().max(0) as u64;
    if traesync_infrastructure::should_retry_skipped_checkin(ledger.as_ref(), &today, now_unix) {
        let Some(saved) = ledger.as_ref() else {
            return;
        };
        let mut ledger = saved.clone();
        let plan = ledger
            .skipped_ids
            .iter()
            .map(|id| (id.clone(), 0u64))
            .collect();
        run_workbuddy_batch(app, login_root, &store, &mut ledger, plan, &production_claimer(login_root.to_path_buf()));
    }
}

/// 今日新台账（与 Trae fresh_auto_checkin_ledger 同形）。
fn fresh_workbuddy_ledger(today: String, total: usize) -> AutoCheckinLedger {
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

/// 生产 claim 闭包：逐账号「先续期（断连兜底）→ status → claim → 复核」。
fn production_claimer(
    login_root: PathBuf,
) -> impl Fn(&str) -> Result<(CheckinOutcome, Option<String>), String> {
    move |profile_id| claim_workbuddy_profile(&login_root, profile_id)
}

/// 逐账号自动签到（P17-A5 核心）：`open_session_for_profile` 内建断连
/// 兜底（access 充裕跳过/临近过期刷新写回），随后 status → claim →
/// status 复核翻转（与手动 claim 命令同语义）。返回 (结果, detail_code)。
pub(crate) fn claim_workbuddy_profile(
    login_root: &Path,
    profile_id: &str,
) -> Result<(CheckinOutcome, Option<String>), String> {
    let now = traesync_infrastructure::workbuddy_credential::unix_ms_now() / 1000;
    let (session, _renewal) = open_session_for_profile(login_root, profile_id, now)
        .map_err(|error| map_pool_error(&error))?;
    // 先查后领（写操作纪律：已签不重复发请求）。
    let before = match session.status() {
        Ok(WorkBuddyCheckinOutcome::Status(status)) => status,
        Ok(WorkBuddyCheckinOutcome::AuthMismatch) => {
            return Ok((CheckinOutcome::RuntimeError, Some("workbuddy_checkin_auth_mismatch".to_string())))
        }
        Ok(other) => {
            return Ok((CheckinOutcome::RuntimeError, Some(format!("workbuddy_checkin_unexpected_response:{other:?}"))))
        }
        Err(error) => return Ok((CheckinOutcome::RuntimeError, Some(error.code().to_string()))),
    };
    if before.today_checked_in {
        return Ok((CheckinOutcome::AlreadyCheckedIn, None));
    }
    let claim = match session.claim() {
        Ok(WorkBuddyCheckinOutcome::Claimed(claim)) => claim,
        Ok(WorkBuddyCheckinOutcome::AlreadyCheckedIn) => {
            // 并发窗口（查询后他端已签）：幂等空操作。
            return Ok((CheckinOutcome::AlreadyCheckedIn, None));
        }
        Ok(WorkBuddyCheckinOutcome::AuthMismatch) => {
            return Ok((CheckinOutcome::RuntimeError, Some("workbuddy_checkin_auth_mismatch".to_string())))
        }
        Ok(other) => {
            return Ok((CheckinOutcome::RuntimeError, Some(format!("workbuddy_checkin_rejected:{other:?}"))))
        }
        Err(error) => return Ok((CheckinOutcome::RuntimeError, Some(error.code().to_string()))),
    };
    // 复核翻转才算真实入账（与手动 claim 同纪律）。
    let verified = matches!(
        session.status(),
        Ok(WorkBuddyCheckinOutcome::Status(status)) if status.today_checked_in
    );
    if !verified {
        return Ok((CheckinOutcome::RuntimeError, Some("workbuddy_checkin_unverified".to_string())));
    }
    let _ = claim;
    Ok((CheckinOutcome::Claimed, None))
}

/// 池错误 → 稳定 detail 码（续期拒绝引导重新捕获，与命令层口径一致）。
fn map_pool_error(error: &WorkBuddyPoolError) -> String {
    match error {
        WorkBuddyPoolError::PoolEmpty | WorkBuddyPoolError::RecordMissing => {
            "workbuddy_auto_checkin_account_missing".to_string()
        }
        WorkBuddyPoolError::RefreshRejected => "workbuddy_checkin_refresh_rejected".to_string(),
        WorkBuddyPoolError::RequestFailed | WorkBuddyPoolError::ResponseInvalid => {
            "workbuddy_checkin_request_failed".to_string()
        }
        WorkBuddyPoolError::StoreIo => "workbuddy_pool_invalid".to_string(),
    }
}

/// 执行一个 WorkBuddy 自动签到批次：逐账号 claim（claim 闭包注入使批次
/// 可离线单测），实时写台账并推送进度；结束置 Finished 并记录结束时刻。
/// plan 为 (profile_id, 相对延迟秒) 升序（首发错峰；补跑/重试全 0）。
fn run_workbuddy_batch<R: tauri::Runtime, C>(
    app: &tauri::AppHandle<R>,
    login_root: &Path,
    store: &AutoCheckinStore,
    ledger: &mut AutoCheckinLedger,
    plan: Vec<(String, u64)>,
    claim: &C,
) where
    C: Fn(&str) -> Result<(CheckinOutcome, Option<String>), String>,
{
    let name_by_profile: std::collections::BTreeMap<String, String> =
        pool_summaries(login_root).into_iter().collect();
    let mut previous_delay = 0u64;
    let mut handled = 0usize;
    for (profile_id, delay) in plan {
        std::thread::sleep(std::time::Duration::from_secs(delay - previous_delay));
        previous_delay = delay;
        handled += 1;
        let (outcome, detail_code) = match claim(&profile_id) {
            Ok(pair) => pair,
            Err(code) => (CheckinOutcome::RuntimeError, Some(code)),
        };
        // 成功口径与 Trae 一致：发放/已签/无资格 = 处理完毕无需干预。
        let is_ok = matches!(
            outcome,
            CheckinOutcome::Claimed
                | CheckinOutcome::AlreadyCheckedIn
                | CheckinOutcome::NotEligible
        );
        if is_ok {
            ledger.completed += 1;
        } else {
            ledger.failed += 1;
        }
        let _ = store.save_ledger(ledger);
        let _ = app.emit(
            "workbuddy-auto-checkin-progress",
            WorkBuddyProgressEvent {
                profile_id: profile_id.clone(),
                screen_name: name_by_profile.get(&profile_id).cloned().unwrap_or_default(),
                outcome: outcome.clone(),
                detail_code: detail_code.clone(),
                completed: handled,
                total: ledger.total,
            },
        );
        let _ = detail_code;
    }
    ledger.state = AutoCheckinBatchState::Finished;
    ledger.last_batch_finished_at = Some(chrono::Utc::now().timestamp().max(0) as u64);
    let _ = store.save_ledger(ledger);
    let _ = app.emit(
        "workbuddy-auto-checkin-finished",
        WorkBuddyFinishedEvent {
            total: ledger.total,
            completed: ledger.completed,
            failed: ledger.failed,
            skipped: ledger.skipped,
        },
    );
}

/// 单账号进度事件（workbuddy-auto-checkin-progress）。
#[derive(Clone, Serialize)]
struct WorkBuddyProgressEvent {
    profile_id: String,
    screen_name: String,
    outcome: CheckinOutcome,
    detail_code: Option<String>,
    completed: usize,
    total: usize,
}

/// 批次完成事件（workbuddy-auto-checkin-finished；前端横幅）。
#[derive(Clone, Serialize)]
struct WorkBuddyFinishedEvent {
    total: usize,
    completed: usize,
    failed: usize,
    skipped: usize,
}

#[cfg(test)]
mod workbuddy_schedule_tests {
    use super::*;
    use tempfile::tempdir;

    /// 参与集存储：roundtrip / 幂等 / 损坏报错不重建。
    #[test]
    fn participation_store_roundtrip_and_validates() {
        let root = tempdir().unwrap();
        let store = WorkBuddyParticipationStore::new(root.path());
        assert_eq!(store.load().unwrap(), Vec::<String>::new(), "缺省空集");
        store.save(&["wb-a".to_string(), "wb-b".to_string()]).unwrap();
        assert_eq!(store.load().unwrap(), vec!["wb-a", "wb-b"]);
        // 覆盖保存（幂等语义）。
        store.save(&["wb-a".to_string()]).unwrap();
        assert_eq!(store.load().unwrap(), vec!["wb-a"]);
        // 损坏报错（不静默重建——用户决策证据纪律）。
        std::fs::write(root.path().join(PARTICIPATION_FILE), "not json").unwrap();
        assert!(store.load().is_err());
    }

    /// 参与状态设置：入池校验（池外账号拒绝）+ 开关幂等。
    #[test]
    fn set_account_participation_validates_pool_membership() {
        let root = tempdir().unwrap();
        let schedule_root = root.path().join("schedule");
        let login_root = root.path().join("login");
        // 空池：任何账号都拒绝。
        assert_eq!(
            set_workbuddy_account_participation(&schedule_root, &login_root, "wb-x", true),
            Err("workbuddy_auto_checkin_account_missing".to_string())
        );
        // 建池：一个账号。
        use traesync_infrastructure::workbuddy_login_credential::{
            WorkBuddyLoginRecord, WorkBuddyLoginStore,
        };
        std::fs::create_dir_all(&login_root).unwrap();
        let store = WorkBuddyLoginStore::new(&login_root);
        store
            .save(&WorkBuddyLoginRecord {
                profile_id: "wb-uid-1".to_string(),
                account_id: "uid-1".to_string(),
                screen_name: "账号一".to_string(),
                masked_mobile: String::new(),
                access_token: "at".to_string(),
                refresh_token: "rt".to_string(),
                access_token_expires_at_unix_seconds: 1_800_000_000,
                refresh_token_expires_at_unix_seconds: 1_800_000_000,
                domain: "d".to_string(),
                session_state: "s".to_string(),
                device_id: "dev".to_string(),
                minted_at_unix_seconds: 1,
                last_updated_at_unix_seconds: 1,
            })
            .unwrap();
        set_workbuddy_account_participation(&schedule_root, &login_root, "wb-uid-1", true).unwrap();
        set_workbuddy_account_participation(&schedule_root, &login_root, "wb-uid-1", true).unwrap();
        let status = workbuddy_auto_checkin_status(&schedule_root, &login_root);
        assert_eq!(status.accounts.len(), 1);
        assert!(status.accounts[0].enabled);
        assert!(status.ledger.is_none(), "未执行过批次时台账为空");
        // 关闭参与。
        set_workbuddy_account_participation(&schedule_root, &login_root, "wb-uid-1", false).unwrap();
        assert!(!workbuddy_auto_checkin_status(&schedule_root, &login_root).accounts[0].enabled);
    }

    /// 参与集 ∩ 池：从池清理的账号自动失效（不再进入 eligible）。
    #[test]
    fn eligible_ids_intersect_participation_and_pool() {
        let root = tempdir().unwrap();
        let schedule_root = root.path().join("schedule");
        let login_root = root.path().join("login");
        std::fs::create_dir_all(&login_root).unwrap();
        WorkBuddyParticipationStore::new(&schedule_root)
            .save(&["wb-in".to_string(), "wb-gone".to_string()])
            .unwrap();
        use traesync_infrastructure::workbuddy_login_credential::{
            WorkBuddyLoginRecord, WorkBuddyLoginStore,
        };
        let store = WorkBuddyLoginStore::new(&login_root);
        for uid in ["in"] {
            store
                .save(&WorkBuddyLoginRecord {
                    profile_id: format!("wb-{uid}"),
                    account_id: uid.to_string(),
                    screen_name: uid.to_string(),
                    masked_mobile: String::new(),
                    access_token: "at".to_string(),
                    refresh_token: "rt".to_string(),
                    access_token_expires_at_unix_seconds: 1,
                    refresh_token_expires_at_unix_seconds: 1,
                    domain: "d".to_string(),
                    session_state: "s".to_string(),
                    device_id: "dev".to_string(),
                    minted_at_unix_seconds: 1,
                    last_updated_at_unix_seconds: 1,
                })
                .unwrap();
        }
        assert_eq!(
            workbuddy_eligible_ids(&schedule_root, &login_root),
            vec!["wb-in"],
            "池外参与 id 自动失效"
        );
    }

    /// 批次执行（注入 claimer）：成功/已签计 completed，失败计 failed；
    /// 台账 Finished + 结束时刻；进度与完成事件发出。
    #[test]
    fn batch_records_outcomes_and_finishes_ledger() {
        let root = tempdir().unwrap();
        let login_root = root.path().join("login");
        let store = schedule_store(root.path());
        let mut ledger = fresh_workbuddy_ledger("2026-10-19".to_string(), 3);
        let outcomes: std::collections::HashMap<String, Result<(CheckinOutcome, Option<String>), String>> =
            [
                ("wb-ok", Ok((CheckinOutcome::Claimed, None))),
                ("wb-already", Ok((CheckinOutcome::AlreadyCheckedIn, None))),
                ("wb-fail", Err("workbuddy_checkin_refresh_rejected".to_string())),
            ]
            .into_iter()
            .map(|(id, outcome)| (id.to_string(), outcome))
            .collect();
        let claim = |profile_id: &str| outcomes[profile_id].clone();
        let app = tauri::test::mock_builder().build(tauri::generate_context!()).unwrap();
        let handle = app.handle().clone();
        run_workbuddy_batch(
            &handle,
            &login_root,
            &store,
            &mut ledger,
            vec![
                ("wb-ok".to_string(), 0),
                ("wb-already".to_string(), 0),
                ("wb-fail".to_string(), 0),
            ],
            &claim,
        );
        assert_eq!(ledger.state, AutoCheckinBatchState::Finished);
        assert_eq!(ledger.completed, 2);
        assert_eq!(ledger.failed, 1);
        assert!(ledger.last_batch_finished_at.is_some());
        // 台账落盘一致。
        let (_, saved) = store.load().unwrap();
        assert_eq!(saved, Some(ledger.clone()));
    }

    /// claim 语义映射：池账号缺失 → Err 携带稳定 detail 码（批次层统一
    /// 映射为 RuntimeError 计入 failed）。
    #[test]
    fn claim_workbuddy_profile_maps_semantics() {
        let root = tempdir().unwrap();
        let error = claim_workbuddy_profile(&root.path().join("nope"), "wb-x").unwrap_err();
        assert_eq!(error, "workbuddy_auto_checkin_account_missing");
    }
}
