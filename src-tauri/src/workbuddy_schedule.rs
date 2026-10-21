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
//! 账号参与集（P19-B5 opt-out 持久化）：调度根
//! `workbuddy-auto-checkin-accounts.json`（tmp+rename 原子写）存显式设置
//! 记录（enabled/touched 双集合）——**默认全参与**（八域对齐 #3）：未
//! 显式设置过的账号视为参与；显式关闭永远尊重。参与集与池容器求交——
//! 已从池清理的账号自动失效。

use std::path::{Path, PathBuf};

use chrono::Timelike;
use serde::Serialize;
use tauri::Emitter;

use traesync_domain::CheckinOutcome;
use traesync_infrastructure::workbuddy_checkin::{
    open_session_for_profile, open_session_for_profile_forced, WorkBuddyCheckinOutcome,
    WorkBuddyPoolError,
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
///
/// P19-B5 持久 opt-out 语义（架构草案 §1.2E / 八域对齐 #3「默认全参与」）：
/// - `enabled_profiles`：显式开启参与的账号；
/// - `touched_profiles`：**曾被用户显式设置过的账号**（开启或关闭都算
///   touched）——默认参与语义只作用于「从未设置过」的账号；
/// - **有效参与 = 未 touched → true（默认参与）**；touched → enabled。
///   显式关闭永远尊重（默认全参与的归一化不会复活已关闭账号）；
/// - v1 迁移：v1 仅有 enabled_profiles（不在列 = 默认不参与）。迁移映射
///   `touched = enabled_profiles`（保留显式开启意图），未列入账号转为
///   「未 touched」→ 按新产品决策默认参与（一次性语义切换，用户拍板）。
struct WorkBuddyParticipationStore {
    path: PathBuf,
}

/// 参与集状态（enabled / touched 双集合；有效参与 = 未 touched || enabled）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct ParticipationState {
    enabled_profiles: Vec<String>,
    touched_profiles: Vec<String>,
}

impl ParticipationState {
    /// 有效参与判定（默认全参与语义的唯一口径）。
    fn effective_enabled(&self, profile_id: &str) -> bool {
        !self.touched_profiles.iter().any(|id| id == profile_id)
            || self.enabled_profiles.iter().any(|id| id == profile_id)
    }
}

#[derive(serde::Serialize, serde::Deserialize)]
struct ParticipationFile {
    format_version: u32,
    #[serde(default)]
    enabled_profiles: Vec<String>,
    #[serde(default)]
    touched_profiles: Vec<String>,
}

impl WorkBuddyParticipationStore {
    fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            path: root.into().join(PARTICIPATION_FILE),
        }
    }

    /// 读取参与集（v1 自动迁移到 v2 语义）；文件不存在返回缺省空集。
    /// 迁移映射：v1 `enabled_profiles` → v2 `touched = enabled`
    /// （显式开启意图保留）；v1 未列入账号 → 未 touched → 默认参与
    /// （八域对齐 #3 一次性语义切换）。损坏/版本不符返回 Err（调用方
    /// 按各自纪律处理——设置页报错，调度 fail-soft 跳过）。
    fn load_state(&self) -> Result<ParticipationState, ()> {
        let content = match std::fs::read_to_string(&self.path) {
            Ok(content) => content,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(ParticipationState::default());
            }
            Err(_) => return Err(()),
        };
        let file: ParticipationFile = serde_json::from_str(&content).map_err(|_| ())?;
        match file.format_version {
            1 => Ok(ParticipationState {
                // v1 迁移：显式开启 = touched；未列入 → 默认参与。
                enabled_profiles: file.enabled_profiles.clone(),
                touched_profiles: file.enabled_profiles,
            }),
            2 => Ok(ParticipationState {
                enabled_profiles: file.enabled_profiles,
                touched_profiles: file.touched_profiles,
            }),
            _ => Err(()),
        }
    }

    /// 兼容旧读取口（显式开启集合；B5 起调用方应改用
    /// [`Self::load_state`] + `ParticipationState::effective_enabled`）。
    fn load(&self) -> Result<Vec<String>, ()> {
        self.load_state().map(|state| state.enabled_profiles)
    }

    /// 保存参与集（tmp + rename 原子写；v2 格式，失败保留原文件）。
    fn save_state(&self, state: &ParticipationState) -> Result<(), ()> {
        let file = ParticipationFile {
            format_version: 2,
            enabled_profiles: state.enabled_profiles.clone(),
            touched_profiles: state.touched_profiles.clone(),
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

    /// v1 兼容保存口（既有调用方/旧测试；内部转 v2：enabled 即 touched，
    /// 保持「显式设置过」语义）。
    fn save(&self, enabled_profiles: &[String]) -> Result<(), ()> {
        self.save_state(&ParticipationState {
            enabled_profiles: enabled_profiles.to_vec(),
            touched_profiles: enabled_profiles.to_vec(),
        })
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
    /// 池内账号与其参与状态（P19-B5：有效参与——未 touched 默认参与）。
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
///
/// P19-B5：`accounts[].enabled` 语义升级为**有效参与**（未 touched →
/// 默认参与 true；touched → 显式值）。参与集文件损坏时按缺省态处理
/// （全部视为未 touched → 默认参与——产品默认语义即「全参与」，claim
/// 幂等风险可控；文件修复后显式设置恢复权威）。
pub(crate) fn workbuddy_auto_checkin_status(
    schedule_root: &Path,
    login_root: &Path,
) -> WorkBuddyAutoCheckinStatusDto {
    let store = schedule_store(schedule_root);
    let (settings, ledger) = store.load().unwrap_or_default();
    let participation = WorkBuddyParticipationStore::new(schedule_root)
        .load_state()
        .unwrap_or_default();
    let accounts = pool_summaries(login_root)
        .into_iter()
        .map(|(profile_id, screen_name)| WorkBuddyAutoAccountDto {
            enabled: participation.effective_enabled(&profile_id),
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
///
/// P19-B5 opt-out 语义：任何显式设置都写入 touched（开启/关闭均算），
/// 此后默认全参与的归一化不再作用于该账号；关闭 = touched + 移出
/// enabled（显式关闭永远尊重）。
pub(crate) fn set_workbuddy_account_participation(
    schedule_root: &Path,
    login_root: &Path,
    profile_id: &str,
    enabled: bool,
) -> Result<(), String> {
    let store = WorkBuddyParticipationStore::new(schedule_root);
    let mut state = store.load_state().map_err(|_| "workbuddy_schedule_invalid".to_string())?;
    let in_pool = pool_summaries(login_root)
        .iter()
        .any(|(id, _)| id == profile_id);
    if !in_pool {
        return Err("workbuddy_auto_checkin_account_missing".to_string());
    }
    // 显式设置 = touched（无论开/关——默认参与语义自此对该账号失效）。
    if !state.touched_profiles.iter().any(|id| id == profile_id) {
        state.touched_profiles.push(profile_id.to_string());
    }
    if enabled {
        if !state.enabled_profiles.iter().any(|id| id == profile_id) {
            state.enabled_profiles.push(profile_id.to_string());
        }
    } else {
        state.enabled_profiles.retain(|id| id != profile_id);
    }
    store
        .save_state(&state)
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

// ===== P19-B5 逐账号签到命令面（矩阵 #43/#46/#48/#51/#56 WB 适配器） =====

/// 逐账号签到状态 DTO（fail-soft：某账号失败不阻塞他账号——error 单独
/// 外显，状态字段在 error 非空时为缺省值）。
#[derive(Debug, Clone, Serialize)]
pub(crate) struct WorkBuddyCheckinAccountStatusDto {
    pub profile_id: String,
    pub screen_name: String,
    /// 查询成功时为 Some；失败（error 非空）为 None。
    pub today_checked_in: Option<bool>,
    pub streak_days: Option<u32>,
    pub daily_credit: Option<u32>,
    /// **本期签到累计**获得积分（活动域，只增不减——不是余额；余额看
    /// credits_balance。P19.2③ 语义改名对齐客户端真实口径）。
    pub total_credits: Option<u32>,
    /// 积分余额（billing 资源域 Σ CycleRemainCapacity，随对话消耗减少——
    /// 客户端「积分余额」真实口径；查询失败/未采集为 None，fail-soft）。
    pub credits_balance: Option<u64>,
    /// 账户资源包总额（billing 资源域 Σ CycleTotalCapacity）。
    pub credits_total: Option<u64>,
    /// 已签日期列表（YYYY-MM-DD；签到日历数据源，矩阵 #57）。
    pub checkin_dates: Vec<String>,
    /// P19.2② 脱敏手机号（服务端预脱敏形态如 `133****1304`，直显；
    /// None = 池记录未采集——重捕获/OAuth 登录后补齐）。
    pub masked_phone: Option<String>,
    /// 失败时的稳定错误码（None = 查询成功）。
    pub error: Option<String>,
}

/// 单账号 status 查询（生产闭包：先续期该账号〔断连兜底〕再 status；
/// status 成功后顺带查资源包余额〔best-effort fail-soft：余额失败只落
/// None，不失败整行〕）。P19.1 401 兜底：AuthMismatch 时强制刷新换新对
/// 后重试一次。
/// 返回 (today_checked_in, streak, daily, 本期签到累计, dates, 余额)。
pub(crate) fn query_profile_status(
    login_root: &Path,
    profile_id: &str,
) -> Result<
    (
        bool,
        u32,
        u32,
        u32,
        Vec<String>,
        Option<traesync_infrastructure::workbuddy_checkin::WorkBuddyResourceBalance>,
    ),
    String,
> {
    use traesync_infrastructure::workbuddy_checkin::WorkBuddyCheckinSession;
    let now = traesync_infrastructure::workbuddy_credential::unix_ms_now() / 1000;
    let read_status =
        |session: &WorkBuddyCheckinSession| {
            match session.status() {
                Ok(WorkBuddyCheckinOutcome::Status(status)) => {
                    // P19.2③ 余额对齐（best-effort：查询失败不失败整行）。
                    let balance = session.resource_balance().ok();
                    Ok((
                        status.today_checked_in,
                        status.streak_days,
                        status.daily_credit,
                        status.total_credits,
                        status.checkin_dates,
                        balance,
                    ))
                }
                Ok(WorkBuddyCheckinOutcome::AuthMismatch) => {
                    Err("workbuddy_checkin_auth_mismatch".to_string())
                }
                Ok(other) => Err(format!("workbuddy_checkin_unexpected_response:{other:?}")),
                Err(error) => Err(error.code().to_string()),
            }
        };
    let (session, _renewal) = open_session_for_profile(login_root, profile_id, now)
        .map_err(|error| map_pool_error(&error))?;
    match read_status(&session) {
        Err(code) if code == "workbuddy_checkin_auth_mismatch" => {
            // P19.1：强制刷新（忽略阈值）换新对后重试一次。
            let (session, _renewal) =
                open_session_for_profile_forced(login_root, profile_id, now)
                    .map_err(|error| map_pool_error(&error))?;
            read_status(&session)
        }
        other => other,
    }
}

/// 聚合编排内核（注入 status 闭包，离线单测 fail-soft 语义）：
/// 逐账号查询，单账号任何失败只落到该账号的 error 字段。
fn workbuddy_checkin_overview_inner(
    targets: Vec<(String, String)>,
    status_fn: &dyn Fn(
        &str,
    ) -> Result<
        (
            bool,
            u32,
            u32,
            u32,
            Vec<String>,
            Option<traesync_infrastructure::workbuddy_checkin::WorkBuddyResourceBalance>,
        ),
        String,
    >,
) -> Vec<WorkBuddyCheckinAccountStatusDto> {
    targets
        .into_iter()
        .map(|(profile_id, screen_name)| match status_fn(&profile_id) {
            Ok((
                today_checked_in,
                streak_days,
                daily_credit,
                total_credits,
                checkin_dates,
                balance,
            )) => {
                WorkBuddyCheckinAccountStatusDto {
                    profile_id,
                    screen_name,
                    today_checked_in: Some(today_checked_in),
                    streak_days: Some(streak_days),
                    daily_credit: Some(daily_credit),
                    total_credits: Some(total_credits),
                    credits_balance: balance.map(|value| value.credits_balance),
                    credits_total: balance.map(|value| value.credits_total),
                    checkin_dates,
                    masked_phone: None,
                    error: None,
                }
            }
            Err(code) => WorkBuddyCheckinAccountStatusDto {
                profile_id,
                screen_name,
                today_checked_in: None,
                streak_days: None,
                daily_credit: None,
                total_credits: None,
                credits_balance: None,
                credits_total: None,
                checkin_dates: Vec::new(),
                masked_phone: None,
                error: Some(code),
            },
        })
        .collect()
}

/// WB 签到聚合视图（矩阵 #43 get_checkin_overview WB 适配器 + #48
/// refresh_checkin_credits 单账号模式）：逐账号 status 汇总（total_credits/
/// streak/daily_credit/checkin_dates）。profile_id=Some → 单账号（不在池内
/// 报稳定错误码）；None → 全池批查（fail-soft）。
#[tauri::command]
pub(crate) async fn get_workbuddy_checkin_overview(
    profile_id: Option<String>,
    state: tauri::State<'_, crate::AppState>,
) -> Result<Vec<WorkBuddyCheckinAccountStatusDto>, String> {
    crate::ensure_login_real_mode_workbuddy(&state)?;
    let (_schedule_root, login_root) = workbuddy_roots(&state)?;
    crate::run_blocking("workbuddy_checkin_overview_join_failed", move || {
        let summaries = pool_summaries(&login_root);
        let targets = match profile_id.as_deref() {
            Some(id) => {
                let found = summaries
                    .iter()
                    .find(|(pid, _)| pid == id)
                    .ok_or("workbuddy_auto_checkin_account_missing".to_string())?;
                vec![(found.0.clone(), found.1.clone())]
            }
            None => summaries,
        };
        let login_root_for_query = login_root.clone();
        let status_fn =
            |pid: &str| query_profile_status(&login_root_for_query, pid);
        let mut rows = workbuddy_checkin_overview_inner(targets, &status_fn);
        // P19.2② 脱敏手机号（池记录采集面；容器读取失败 = None，fail-soft
        // 不阻塞状态行）。
        use traesync_infrastructure::workbuddy_login_credential::WorkBuddyLoginStore;
        let store = WorkBuddyLoginStore::new(login_root);
        for row in rows.iter_mut() {
            row.masked_phone = store
                .load(&row.profile_id, account_id_of(&row.profile_id))
                .ok()
                .map(|record| record.masked_mobile)
                .filter(|phone| !phone.trim().is_empty());
        }
        Ok(rows)
    })
    .await
}

/// profile_id（`wb-{uid}` 惯例）→ account_id（uid）——容器读取绑定键。
fn account_id_of(profile_id: &str) -> &str {
    profile_id.strip_prefix("wb-").unwrap_or(profile_id)
}

/// WB 参与集（矩阵 #46 list_checkin_accounts WB 版）：池内账号逐条带
/// **有效参与**标志（未 touched 默认参与；显式关闭永远 false）——fe 过滤
/// enabled 构造参与集，后端不替前端裁剪（保留全部行供开关渲染）。
#[tauri::command]
pub(crate) fn list_workbuddy_checkin_accounts(
    state: tauri::State<'_, crate::AppState>,
) -> Result<Vec<WorkBuddyAutoAccountDto>, String> {
    crate::ensure_login_real_mode_workbuddy(&state)?;
    let (schedule_root, login_root) = workbuddy_roots(&state)?;
    let participation = WorkBuddyParticipationStore::new(&schedule_root)
        .load_state()
        .unwrap_or_default();
    Ok(pool_summaries(&login_root)
        .into_iter()
        .map(|(profile_id, screen_name)| WorkBuddyAutoAccountDto {
            enabled: participation.effective_enabled(&profile_id),
            profile_id,
            screen_name,
        })
        .collect())
}

/// WB 单账号手动签到（矩阵 #56 per-account 形态；**单次确认入口**——
/// UI 逐账号行独立签到按钮；与当前账号版 workbuddy_checkin_claim 同纪律：
/// 先查后领 + 复核翻转，credit 外显）。
#[tauri::command]
pub(crate) async fn workbuddy_checkin_claim_profile(
    profile_id: String,
    state: tauri::State<'_, crate::AppState>,
) -> Result<crate::workbuddy_commands::WorkBuddyCheckinClaimDto, String> {
    crate::ensure_login_real_mode_workbuddy(&state)?;
    let (_schedule_root, login_root) = workbuddy_roots(&state)?;
    crate::run_blocking("workbuddy_checkin_claim_profile_join_failed", move || {
        let receipt = claim_workbuddy_profile_detailed(&login_root, &profile_id)?;
        // WB 直连链只产出 Claimed/AlreadyCheckedIn/RuntimeError 三态；
        // 其余领域变体（AuthMismatch/ProfileBusy 等来自 TRAE 虚拟设备链）
        // 按防御性兜底归 Err（detail 优先，缺省给稳定码）。
        match receipt.outcome {
            CheckinOutcome::Claimed => Ok(crate::workbuddy_commands::WorkBuddyCheckinClaimDto {
                credit: receipt.credit,
                streak_days: claim_streak_after(&login_root, &profile_id),
                verified: true,
            }),
            // 已签 = 幂等空操作，复核口径 verified=true（与当前账号版一致）。
            CheckinOutcome::AlreadyCheckedIn => {
                Ok(crate::workbuddy_commands::WorkBuddyCheckinClaimDto {
                    credit: 0,
                    streak_days: claim_streak_after(&login_root, &profile_id),
                    verified: true,
                })
            }
            CheckinOutcome::NotEligible => Err(
                receipt
                    .detail_code
                    .unwrap_or_else(|| "workbuddy_checkin_not_eligible".to_string()),
            ),
            _ => Err(
                receipt
                    .detail_code
                    .unwrap_or_else(|| "workbuddy_checkin_request_failed".to_string()),
            ),
        }
    })
    .await
}

/// 领取后的连签天数（best-effort 回读：status 失败按 0，不阻塞回执）。
fn claim_streak_after(login_root: &Path, profile_id: &str) -> u32 {
    query_profile_status(login_root, profile_id)
        .map(|(_, streak, _, _, _, _)| streak)
        .unwrap_or(0)
}

/// 批量签到逐账号条目（fail-soft：单账号失败只落本条）。
#[derive(Debug, Clone, Serialize)]
pub(crate) struct WorkBuddyCheckinBatchItemDto {
    pub profile_id: String,
    pub screen_name: String,
    /// Claimed / AlreadyCheckedIn / NotEligible / RuntimeError（领域枚举
    /// 线格式，与自动签到进度事件同口径）。
    pub outcome: CheckinOutcome,
    /// 本次入账积分（仅 Claimed 时 > 0）。
    pub credit: u32,
    pub detail_code: Option<String>,
}

/// 批量编排内核（注入 claim 闭包，离线单测 fail-soft 语义）：顺序执行，
/// 单账号任何失败只落到本条 outcome=RuntimeError + detail_code。
fn workbuddy_checkin_claim_batch_inner(
    targets: Vec<(String, String)>,
    claim: &dyn Fn(&str) -> Result<WorkBuddyProfileClaimReceipt, String>,
) -> Vec<WorkBuddyCheckinBatchItemDto> {
    targets
        .into_iter()
        .map(|(profile_id, screen_name)| {
            let (outcome, detail_code, credit) = match claim(&profile_id) {
                Ok(receipt) => (receipt.outcome, receipt.detail_code, receipt.credit),
                // 池账号缺失等 Err：统一 RuntimeError（与自动批次同口径）。
                Err(code) => (CheckinOutcome::RuntimeError, Some(code), 0),
            };
            WorkBuddyCheckinBatchItemDto {
                profile_id,
                screen_name,
                outcome,
                credit,
                detail_code,
            }
        })
        .collect()
}

/// WB 批量签到（矩阵 #51 run_checkin WB 适配器；**单次确认入口**：UI
/// 「一键签到(N)」——参与集默认全选）。逐账号顺序执行（claim 纯 HTTP
/// 幂等，无调度错峰需求）；fail-soft：某账号失败不阻塞他账号，逐条回执。
#[tauri::command]
pub(crate) async fn workbuddy_checkin_claim_batch(
    profile_ids: Vec<String>,
    state: tauri::State<'_, crate::AppState>,
) -> Result<Vec<WorkBuddyCheckinBatchItemDto>, String> {
    crate::ensure_login_real_mode_workbuddy(&state)?;
    let (_schedule_root, login_root) = workbuddy_roots(&state)?;
    crate::run_blocking("workbuddy_checkin_claim_batch_join_failed", move || {
        let summaries = pool_summaries(&login_root);
        // 参与集裁剪在后端兜底：入参去重并限定池内账号（池外 id 落条目
        // 报错——fail-soft，不整体拒绝）。
        let mut seen = std::collections::HashSet::new();
        let targets: Vec<(String, String)> = profile_ids
            .into_iter()
            .filter(|id| seen.insert(id.clone()))
            .map(|id| {
                let screen_name = summaries
                    .iter()
                    .find(|(pid, _)| pid == &id)
                    .map(|(_, name)| name.clone())
                    .unwrap_or_default();
                (id, screen_name)
            })
            .collect();
        let login_root_for_claim = login_root.clone();
        let claim =
            |pid: &str| claim_workbuddy_profile_detailed(&login_root_for_claim, pid);
        Ok(workbuddy_checkin_claim_batch_inner(targets, &claim))
    })
    .await
}

/// 筛选参与自动签到的账号（**有效参与** ∩ 池内现存账号：未 touched 的
/// 池账号默认参与〔八域对齐 #3〕；显式关闭永远排除；池清理自动失效）。
fn workbuddy_eligible_ids(schedule_root: &Path, login_root: &Path) -> Vec<String> {
    let participation = WorkBuddyParticipationStore::new(schedule_root)
        .load_state()
        .unwrap_or_default();
    pool_summaries(login_root)
        .into_iter()
        .filter(|(profile_id, _)| participation.effective_enabled(profile_id))
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

/// 逐账号签到回执（B5：手动单签命令需要 credit 外显——调度口径仍走
/// [`claim_workbuddy_profile`] 的 (outcome, detail) 二元组）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WorkBuddyProfileClaimReceipt {
    pub outcome: CheckinOutcome,
    pub detail_code: Option<String>,
    /// 本次入账积分（仅 outcome=Claimed 时有效；其余为 0）。
    pub credit: u32,
}

/// 逐账号签到核心（B5 拆分自 claim_workbuddy_profile）：`open_session_for_profile`
/// 内建断连兜底（access 充裕跳过/临近过期刷新写回），随后 status → claim →
/// status 复核翻转。复核不过 = RuntimeError（credit 不外显）。
/// P19.1 401 兜底：AuthMismatch 时强制刷新换新对后重试一次。
pub(crate) fn claim_workbuddy_profile_detailed(
    login_root: &Path,
    profile_id: &str,
) -> Result<WorkBuddyProfileClaimReceipt, String> {
    let now = traesync_infrastructure::workbuddy_credential::unix_ms_now() / 1000;
    let (session, _renewal) = open_session_for_profile(login_root, profile_id, now)
        .map_err(|error| map_pool_error(&error))?;
    let receipt = claim_with_session(&session);
    if receipt.detail_code.as_deref() == Some("workbuddy_checkin_auth_mismatch") {
        // P19.1：强制刷新（忽略阈值）换新对后重试一次（轮换副作用：旧对
        // 即作废；重试仍失败如实报因——引导重新捕获/OAuth）。
        let (session, _renewal) = open_session_for_profile_forced(login_root, profile_id, now)
            .map_err(|error| map_pool_error(&error))?;
        return Ok(claim_with_session(&session));
    }
    Ok(receipt)
}

/// 单会话签到动作序列（先查后领 + 复核翻转；P19.1 拆出供 401 重试复用）。
fn claim_with_session(
    session: &traesync_infrastructure::workbuddy_checkin::WorkBuddyCheckinSession,
) -> WorkBuddyProfileClaimReceipt {
    // 先查后领（写操作纪律：已签不重复发请求）。
    let before = match session.status() {
        Ok(WorkBuddyCheckinOutcome::Status(status)) => status,
        Ok(WorkBuddyCheckinOutcome::AuthMismatch) => {
            return WorkBuddyProfileClaimReceipt {
                outcome: CheckinOutcome::RuntimeError,
                detail_code: Some("workbuddy_checkin_auth_mismatch".to_string()),
                credit: 0,
            }
        }
        Ok(other) => {
            return WorkBuddyProfileClaimReceipt {
                outcome: CheckinOutcome::RuntimeError,
                detail_code: Some(format!("workbuddy_checkin_unexpected_response:{other:?}")),
                credit: 0,
            }
        }
        Err(error) => {
            return WorkBuddyProfileClaimReceipt {
                outcome: CheckinOutcome::RuntimeError,
                detail_code: Some(error.code().to_string()),
                credit: 0,
            }
        }
    };
    if before.today_checked_in {
        return WorkBuddyProfileClaimReceipt {
            outcome: CheckinOutcome::AlreadyCheckedIn,
            detail_code: None,
            credit: 0,
        };
    }
    let claim = match session.claim() {
        Ok(WorkBuddyCheckinOutcome::Claimed(claim)) => claim,
        Ok(WorkBuddyCheckinOutcome::AlreadyCheckedIn) => {
            // 并发窗口（查询后他端已签）：幂等空操作。
            return WorkBuddyProfileClaimReceipt {
                outcome: CheckinOutcome::AlreadyCheckedIn,
                detail_code: None,
                credit: 0,
            };
        }
        Ok(WorkBuddyCheckinOutcome::AuthMismatch) => {
            return WorkBuddyProfileClaimReceipt {
                outcome: CheckinOutcome::RuntimeError,
                detail_code: Some("workbuddy_checkin_auth_mismatch".to_string()),
                credit: 0,
            }
        }
        Ok(other) => {
            return WorkBuddyProfileClaimReceipt {
                outcome: CheckinOutcome::RuntimeError,
                detail_code: Some(format!("workbuddy_checkin_rejected:{other:?}")),
                credit: 0,
            }
        }
        Err(error) => {
            return WorkBuddyProfileClaimReceipt {
                outcome: CheckinOutcome::RuntimeError,
                detail_code: Some(error.code().to_string()),
                credit: 0,
            }
        }
    };
    // 复核翻转才算真实入账（与手动 claim 同纪律）。
    let verified = matches!(
        session.status(),
        Ok(WorkBuddyCheckinOutcome::Status(status)) if status.today_checked_in
    );
    if !verified {
        return WorkBuddyProfileClaimReceipt {
            outcome: CheckinOutcome::RuntimeError,
            detail_code: Some("workbuddy_checkin_unverified".to_string()),
            credit: 0,
        };
    }
    WorkBuddyProfileClaimReceipt {
        outcome: CheckinOutcome::Claimed,
        detail_code: None,
        credit: claim.credit,
    }
}

/// 逐账号自动签到（P17-A5 核心，调度口径薄封装）。
pub(crate) fn claim_workbuddy_profile(
    login_root: &Path,
    profile_id: &str,
) -> Result<(CheckinOutcome, Option<String>), String> {
    claim_workbuddy_profile_detailed(login_root, profile_id)
        .map(|receipt| (receipt.outcome, receipt.detail_code))
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
// ============================================================================
// P19-B5 后端单测（逐账号命令面 + opt-out 持久化）
// ============================================================================

#[cfg(test)]
mod workbuddy_b5_tests {
    use super::*;
    use tempfile::tempdir;
    use traesync_infrastructure::workbuddy_login_credential::{
        WorkBuddyLoginRecord, WorkBuddyLoginStore,
    };

    /// 建池夹具：两个池内账号（wb-uid-1/账号一、wb-uid-2/账号二）。
    fn make_pool(login_root: &Path) {
        std::fs::create_dir_all(login_root).unwrap();
        let store = WorkBuddyLoginStore::new(login_root);
        for (uid, name) in [("uid-1", "账号一"), ("uid-2", "账号二")] {
            store
                .save(&WorkBuddyLoginRecord {
                    profile_id: format!("wb-{uid}"),
                    account_id: uid.to_string(),
                    screen_name: name.to_string(),
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
        }
    }

    /// 有效参与语义：未 touched 默认参与；显式关闭永远 false；显式开启 true。
    #[test]
    fn effective_participation_default_on_and_opt_out_respected() {
        let root = tempdir().unwrap();
        let schedule_root = root.path().join("schedule");
        let login_root = root.path().join("login");
        make_pool(&login_root);

        // 未 touched：两账号默认参与（八域对齐 #3）。
        let status = workbuddy_auto_checkin_status(&schedule_root, &login_root);
        assert!(status.accounts.iter().all(|account| account.enabled));
        assert_eq!(workbuddy_eligible_ids(&schedule_root, &login_root).len(), 2);

        // 显式关闭 wb-uid-1：touched + 移出 enabled → effective false，
        // 且默认参与语义不再作用于它（opt-out 永久尊重）。
        set_workbuddy_account_participation(&schedule_root, &login_root, "wb-uid-1", false)
            .unwrap();
        let status = workbuddy_auto_checkin_status(&schedule_root, &login_root);
        let first = status.accounts.iter().find(|a| a.profile_id == "wb-uid-1").unwrap();
        let second = status.accounts.iter().find(|a| a.profile_id == "wb-uid-2").unwrap();
        assert!(!first.enabled, "显式关闭永远尊重");
        assert!(second.enabled, "未 touched 账号不受他账号设置影响");
        assert_eq!(
            workbuddy_eligible_ids(&schedule_root, &login_root),
            vec!["wb-uid-2".to_string()],
            "调度 eligible 同口径：显式关闭排除、默认参与保留"
        );

        // 再显式开启：touched + enabled → true。
        set_workbuddy_account_participation(&schedule_root, &login_root, "wb-uid-1", true)
            .unwrap();
        assert!(workbuddy_auto_checkin_status(&schedule_root, &login_root)
            .accounts
            .iter()
            .all(|account| account.enabled));
    }

    /// v1 → v2 迁移：v1 enabled_profiles = touched（显式开启意图保留）；
    /// v1 未列入账号转为「未 touched」→ 默认参与（一次性语义切换）。
    #[test]
    fn participation_v1_migrates_to_touched_semantics() {
        let root = tempdir().unwrap();
        let schedule_root = root.path().join("schedule");
        let login_root = root.path().join("login");
        make_pool(&login_root);
        // 手写 v1 文件：仅 wb-uid-1 显式开启。
        std::fs::create_dir_all(&schedule_root).unwrap();
        let v1 = r#"{"format_version":1,"enabled_profiles":["wb-uid-1"]}"#;
        std::fs::write(schedule_root.join(PARTICIPATION_FILE), v1).unwrap();
        let state = WorkBuddyParticipationStore::new(&schedule_root)
            .load_state()
            .unwrap();
        assert!(state.touched_profiles.iter().any(|id| id == "wb-uid-1"));
        assert!(state.effective_enabled("wb-uid-1"));
        // v1 未列入的 wb-uid-2 → 未 touched → 新语义默认参与。
        assert!(state.effective_enabled("wb-uid-2"));
        // 重新读取稳定（迁移语义幂等）。
        assert!(WorkBuddyParticipationStore::new(&schedule_root)
            .load_state()
            .is_ok());
    }

    /// overview 编排内核 fail-soft：账号一成功、账号二失败——失败只落
    /// 本条 error 字段，不阻塞他账号。余额字段随 status 成功透传
    /// （P19.2③：credits_balance/credits_total）。
    #[test]
    fn overview_is_fail_soft_per_account() {
        let targets = vec![
            ("wb-uid-1".to_string(), "账号一".to_string()),
            ("wb-uid-2".to_string(), "账号二".to_string()),
        ];
        let balance = Some(traesync_infrastructure::workbuddy_checkin::WorkBuddyResourceBalance {
            credits_balance: 1600,
            credits_total: 2300,
        });
        let results: std::collections::HashMap<
            String,
            Result<
                (
                    bool,
                    u32,
                    u32,
                    u32,
                    Vec<String>,
                    Option<traesync_infrastructure::workbuddy_checkin::WorkBuddyResourceBalance>,
                ),
                String,
            >,
        > = [
            (
                "wb-uid-1",
                Ok((true, 6, 100, 600, vec!["2026-10-20".to_string()], balance)),
            ),
            (
                "wb-uid-2",
                Err("workbuddy_checkin_refresh_rejected".to_string()),
            ),
        ]
        .into_iter()
        .map(|(id, outcome)| (id.to_string(), outcome))
        .collect();
        let status_fn = |pid: &str| results[pid].clone();
        let overview = workbuddy_checkin_overview_inner(targets, &status_fn);
        assert_eq!(overview.len(), 2, "失败账号不缩短批查结果");
        let first = &overview[0];
        assert!(first.error.is_none());
        assert_eq!(first.today_checked_in, Some(true));
        assert_eq!(first.total_credits, Some(600), "本期签到累计（活动域）");
        assert_eq!(first.credits_balance, Some(1600), "余额（资源域）");
        assert_eq!(first.credits_total, Some(2300));
        assert_eq!(first.checkin_dates, vec!["2026-10-20".to_string()]);
        let second = &overview[1];
        assert_eq!(
            second.error.as_deref(),
            Some("workbuddy_checkin_refresh_rejected")
        );
        assert_eq!(second.today_checked_in, None, "失败账号状态字段为 None");
        assert_eq!(second.credits_balance, None);
        assert!(second.checkin_dates.is_empty());
    }

    /// claim 单账号命令编排：池内缺失账号 → 稳定错误码（复用调度口径）。
    #[test]
    fn claim_profile_missing_pool_account_maps_error() {
        let root = tempdir().unwrap();
        let error = claim_workbuddy_profile_detailed(&root.path().join("nope"), "wb-x")
            .unwrap_err();
        assert_eq!(error, "workbuddy_auto_checkin_account_missing");
    }

    /// batch 编排内核 fail-soft：成功/已签/失败逐条落；池外 id 的 Err
    /// 兜底（claim 闭包注入；命令层去重，内核不假设唯一）。
    #[test]
    fn claim_batch_is_fail_soft_per_account() {
        let targets = vec![
            ("wb-uid-1".to_string(), "账号一".to_string()),
            ("wb-uid-1".to_string(), "账号一".to_string()),
            ("wb-uid-2".to_string(), "账号二".to_string()),
            ("wb-gone".to_string(), String::new()),
        ];
        let claims: std::collections::HashMap<
            String,
            Result<WorkBuddyProfileClaimReceipt, String>,
        > = [
            (
                "wb-uid-1",
                Ok(WorkBuddyProfileClaimReceipt {
                    outcome: CheckinOutcome::Claimed,
                    detail_code: None,
                    credit: 100,
                }),
            ),
            (
                "wb-uid-2",
                Ok(WorkBuddyProfileClaimReceipt {
                    outcome: CheckinOutcome::AlreadyCheckedIn,
                    detail_code: None,
                    credit: 0,
                }),
            ),
            (
                "wb-gone",
                Err("workbuddy_auto_checkin_account_missing".to_string()),
            ),
        ]
        .into_iter()
        .map(|(id, outcome)| (id.to_string(), outcome))
        .collect();
        let claim = |pid: &str| claims[pid].clone();
        let items = workbuddy_checkin_claim_batch_inner(targets, &claim);
        assert_eq!(items.len(), 4, "逐条回执：失败/重复不缩短结果");
        assert_eq!(items[0].outcome, CheckinOutcome::Claimed);
        assert_eq!(items[0].credit, 100);
        // 重复条目原样透传（去重是命令层职责，内核不假设唯一）。
        assert_eq!(items[1].outcome, CheckinOutcome::Claimed);
        assert_eq!(items[2].outcome, CheckinOutcome::AlreadyCheckedIn);
        assert_eq!(items[2].credit, 0);
        assert_eq!(items[3].outcome, CheckinOutcome::RuntimeError);
        assert_eq!(
            items[3].detail_code.as_deref(),
            Some("workbuddy_auto_checkin_account_missing")
        );
    }

    /// 参与集命令内核：list 语义 = 全池行 + 有效参与标志（显式关闭 false、
    /// 未 touched true）。
    #[test]
    fn list_accounts_reports_effective_participation() {
        let root = tempdir().unwrap();
        let schedule_root = root.path().join("schedule");
        let login_root = root.path().join("login");
        make_pool(&login_root);
        set_workbuddy_account_participation(&schedule_root, &login_root, "wb-uid-1", false)
            .unwrap();
        let participation = WorkBuddyParticipationStore::new(&schedule_root)
            .load_state()
            .unwrap_or_default();
        let rows: Vec<(String, bool)> = pool_summaries(&login_root)
            .into_iter()
            .map(|(profile_id, _)| {
                (profile_id.clone(), participation.effective_enabled(&profile_id))
            })
            .collect();
        assert_eq!(
            rows,
            vec![
                ("wb-uid-1".to_string(), false),
                ("wb-uid-2".to_string(), true)
            ]
        );
    }
}
