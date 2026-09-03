//! 自动签到设置与每日台账（JSON 文件，ADR-0019 决策 5 / 2026-08-23 grill 定案）。
//!
//! 触发模型 B：每日时间点（App 运行中跨过即触发）+ 打开补偿（迟打开时
//! `now >= 今日时间点` 自然满足触发条件）。调度器抽象保留未来托盘常驻（C）余地。
//!
//! 文件布局：`<checkin_root>/auto_checkin.json`，含设置（enabled / daily_time）
//! 与台账（今日执行状态）。损坏时 fail-closed（不触发自动签到），不删除文件（铁律）。

use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::atomic_publish::publish_replacing;

const STORE_FILE: &str = "auto_checkin.json";
const MAX_STORE_BYTES: u64 = 64 * 1024;

/// 自动签到错误；不携带敏感内容。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AutoCheckinStoreError {
    /// 文件损坏或格式不符。
    Invalid,
    /// 读写失败。
    Io,
}

/// 自动签到设置（用户可改）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AutoCheckinSettings {
    /// 总开关；false 时调度器完全静默。
    pub enabled: bool,
    /// 每日触发时间，格式 "HH:MM"（24 小时制，本地时区）。
    pub daily_time_hhmm: String,
}

impl Default for AutoCheckinSettings {
    fn default() -> Self {
        // 默认 10:00：避开 CST 午夜重置后的抢跑时段与服务端早高峰。
        Self {
            enabled: true,
            daily_time_hhmm: "10:00".to_string(),
        }
    }
}

/// 今日台账：记录当日自动签到执行情况（跨日作废重建）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AutoCheckinLedger {
    /// 台账所属日期（本地时区 "YYYY-MM-DD"）；与今日不符视为未执行。
    pub date: String,
    /// 批次状态：发起即记录（中断后不重触发，未签账号由手动补）。
    pub state: AutoCheckinBatchState,
    pub total: usize,
    pub completed: usize,
    pub failed: usize,
    /// 因手动批次持锁而跳过的账号数。
    pub skipped: usize,
}

/// 台账批次状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AutoCheckinBatchState {
    /// 已发起，错峰执行中（或因 App 关闭而中断）。
    Running,
    /// 全部账号处理完毕。
    Finished,
}

#[derive(Serialize, Deserialize)]
struct StoreFile {
    format_version: u32,
    #[serde(default)]
    settings: AutoCheckinSettings,
    #[serde(default)]
    ledger: Option<AutoCheckinLedger>,
}

/// 自动签到存储：设置 + 台账同文件持久化。
pub struct AutoCheckinStore {
    path: PathBuf,
}

impl AutoCheckinStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            path: root.into().join(STORE_FILE),
        }
    }

    /// 读取（设置, 台账）；文件不存在时返回默认设置（首次使用）。
    pub fn load(
        &self,
    ) -> Result<(AutoCheckinSettings, Option<AutoCheckinLedger>), AutoCheckinStoreError> {
        let metadata = match fs::symlink_metadata(&self.path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok((AutoCheckinSettings::default(), None));
            }
            Err(_) => return Err(AutoCheckinStoreError::Io),
        };
        if !metadata.is_file() || metadata.len() > MAX_STORE_BYTES {
            return Err(AutoCheckinStoreError::Invalid);
        }
        let content = fs::read_to_string(&self.path).map_err(|_| AutoCheckinStoreError::Io)?;
        let store: StoreFile =
            serde_json::from_str(&content).map_err(|_| AutoCheckinStoreError::Invalid)?;
        if store.format_version != 1 {
            return Err(AutoCheckinStoreError::Invalid);
        }
        // 时间格式校验：HH:MM 且范围合法。
        if !is_valid_hhmm(&store.settings.daily_time_hhmm) {
            return Err(AutoCheckinStoreError::Invalid);
        }
        Ok((store.settings, store.ledger))
    }

    /// 保存设置（保留现有台账）。
    pub fn save_settings(
        &self,
        settings: &AutoCheckinSettings,
    ) -> Result<(), AutoCheckinStoreError> {
        if !is_valid_hhmm(&settings.daily_time_hhmm) {
            return Err(AutoCheckinStoreError::Invalid);
        }
        let (_, ledger) = self.load()?;
        self.write(settings, ledger.as_ref())
    }

    /// 写入台账（保留现有设置）。
    pub fn save_ledger(
        &self,
        ledger: &AutoCheckinLedger,
    ) -> Result<(), AutoCheckinStoreError> {
        let (settings, _) = self.load()?;
        self.write(&settings, Some(ledger))
    }

    fn write(
        &self,
        settings: &AutoCheckinSettings,
        ledger: Option<&AutoCheckinLedger>,
    ) -> Result<(), AutoCheckinStoreError> {
        let store = StoreFile {
            format_version: 1,
            settings: settings.clone(),
            ledger: ledger.cloned(),
        };
        let content =
            serde_json::to_string_pretty(&store).map_err(|_| AutoCheckinStoreError::Invalid)?;
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent).map_err(|_| AutoCheckinStoreError::Io)?;
        }
        let temporary = self.path.with_extension("json.tmp");
        fs::write(&temporary, content.as_bytes()).map_err(|_| AutoCheckinStoreError::Io)?;
        publish_replacing(&temporary, &self.path).map_err(|_| AutoCheckinStoreError::Io)
    }
}

/// "HH:MM" 格式与范围校验。
pub fn is_valid_hhmm(value: &str) -> bool {
    let Some((hour, minute)) = value.split_once(':') else {
        return false;
    };
    let Ok(hour) = hour.parse::<u8>() else {
        return false;
    };
    let Ok(minute) = minute.parse::<u8>() else {
        return false;
    };
    hour < 24 && minute < 60 && value.len() == 5
}

/// 错峰窗口上限（秒）：每账号独立随机延迟 0~15 分钟（grill 决策 4）。
const MAX_STAGGER_SECONDS: u64 = 15 * 60;

/// 为今日自动批次生成错峰执行计划（纯函数 + 密码学随机源）。
///
/// 每账号独立均匀采样 0~900 秒延迟，按延迟升序返回；账号顺序的每日随机性
/// 由延迟随机性自然产生（同一账号集合每天排序结果不同）。调用方按升序
/// 增量睡眠到各账号目标时刻后逐个执行。
pub fn build_staggered_plan(
    profile_ids: Vec<String>,
) -> Result<Vec<(String, u64)>, AutoCheckinStoreError> {
    let mut plan = Vec::with_capacity(profile_ids.len());
    for profile_id in profile_ids {
        let mut bytes = [0u8; 4];
        // openssl 密码学随机：均匀性足够（901 取模的模偏差可忽略——
        // 2^32 % 901 的偏差量级 < 0.0000006%，不影响行为模拟目的）。
        openssl::rand::rand_bytes(&mut bytes).map_err(|_| AutoCheckinStoreError::Io)?;
        let value = u32::from_le_bytes(bytes) as u64;
        plan.push((profile_id, value % (MAX_STAGGER_SECONDS + 1)));
    }
    plan.sort_by_key(|(_, delay)| *delay);
    Ok(plan)
}

/// 触发判定（纯函数，可单测）：是否应发起今日自动签到批次。
///
/// 条件：设置开启 且 台账非今日（今日未发起过）且 `now` 已到达今日触发时间。
/// 打开补偿无需额外分支：迟打开时 `now >= 时间点` 自然成立。
pub fn should_trigger_auto_checkin(
    settings: &AutoCheckinSettings,
    ledger: Option<&AutoCheckinLedger>,
    today: &str,
    now_minutes_of_day: u32,
) -> bool {
    if !settings.enabled {
        return false;
    }
    if let Some(ledger) = ledger {
        if ledger.date == today {
            return false;
        }
    }
    let Some((hour, minute)) = settings.daily_time_hhmm.split_once(':') else {
        return false;
    };
    let (Ok(hour), Ok(minute)) = (hour.parse::<u32>(), minute.parse::<u32>()) else {
        return false;
    };
    now_minutes_of_day >= hour * 60 + minute
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn store(root: &Path) -> AutoCheckinStore {
        AutoCheckinStore::new(root)
    }

    #[test]
    fn load_missing_file_returns_defaults() {
        let root = tempfile::tempdir().unwrap();
        let (settings, ledger) = store(root.path()).load().unwrap();
        assert!(settings.enabled);
        assert_eq!(settings.daily_time_hhmm, "10:00");
        assert!(ledger.is_none());
    }

    #[test]
    fn save_and_reload_roundtrip() {
        let root = tempfile::tempdir().unwrap();
        let s = store(root.path());
        s.save_settings(&AutoCheckinSettings {
            enabled: false,
            daily_time_hhmm: "09:30".to_string(),
        })
        .unwrap();
        let (settings, _) = s.load().unwrap();
        assert!(!settings.enabled);
        assert_eq!(settings.daily_time_hhmm, "09:30");

        let ledger = AutoCheckinLedger {
            date: "2026-08-23".to_string(),
            state: AutoCheckinBatchState::Finished,
            total: 5,
            completed: 4,
            failed: 1,
            skipped: 0,
        };
        s.save_ledger(&ledger).unwrap();
        let (settings, loaded) = s.load().unwrap();
        // 台账写入不覆盖设置。
        assert!(!settings.enabled);
        assert_eq!(loaded.unwrap(), ledger);
    }

    #[test]
    fn invalid_time_format_rejected() {
        let root = tempfile::tempdir().unwrap();
        let s = store(root.path());
        assert!(s
            .save_settings(&AutoCheckinSettings {
                enabled: true,
                daily_time_hhmm: "9:00".to_string(), // 缺前导零
            })
            .is_err());
        assert!(s
            .save_settings(&AutoCheckinSettings {
                enabled: true,
                daily_time_hhmm: "24:00".to_string(), // 越界
            })
            .is_err());
    }

    #[test]
    fn corrupted_file_fails_closed() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join(STORE_FILE);
        fs::write(&path, "{ not json").unwrap();
        // 损坏时读取失败（调度器不触发），且不删除文件（铁律）。
        assert!(store(root.path()).load().is_err());
        assert!(path.exists());
    }

    #[test]
    fn trigger_rules() {
        let settings = AutoCheckinSettings::default(); // 10:00
        let today = "2026-08-23";
        let ledger_today = AutoCheckinLedger {
            date: today.to_string(),
            state: AutoCheckinBatchState::Running,
            total: 5,
            completed: 2,
            failed: 0,
            skipped: 0,
        };
        let ledger_yesterday = AutoCheckinLedger {
            date: "2026-08-22".to_string(),
            state: AutoCheckinBatchState::Finished,
            total: 5,
            completed: 5,
            failed: 0,
            skipped: 0,
        };

        // 未到点（09:59）不触发。
        assert!(!should_trigger_auto_checkin(&settings, None, today, 9 * 60 + 59));
        // 到点（10:00）触发。
        assert!(should_trigger_auto_checkin(&settings, None, today, 10 * 60));
        // 补偿语义：下午打开（14:23）触发。
        assert!(should_trigger_auto_checkin(&settings, None, today, 14 * 60 + 23));
        // 今日已发起（任意状态）不重复触发。
        assert!(!should_trigger_auto_checkin(
            &settings,
            Some(&ledger_today),
            today,
            15 * 60
        ));
        // 昨日台账不阻塞今日。
        assert!(should_trigger_auto_checkin(
            &settings,
            Some(&ledger_yesterday),
            today,
            10 * 60
        ));
        // 总开关关闭。
        let off = AutoCheckinSettings {
            enabled: false,
            daily_time_hhmm: "10:00".to_string(),
        };
        assert!(!should_trigger_auto_checkin(&off, None, today, 11 * 60));
    }

    #[test]
    fn staggered_plan_covers_all_accounts_within_window_sorted() {
        let ids = vec![
            "profile-a".to_string(),
            "profile-b".to_string(),
            "profile-c".to_string(),
        ];
        let plan = build_staggered_plan(ids.clone()).unwrap();
        // 全部账号都在计划中，延迟都在 0~15 分钟窗口内。
        assert_eq!(plan.len(), 3);
        let mut covered = Vec::new();
        for (id, delay) in &plan {
            covered.push(id.clone());
            assert!(*delay <= 15 * 60);
        }
        covered.sort();
        assert_eq!(covered, ids);
        // 延迟升序（调用方依赖增量睡眠语义）。
        let delays: Vec<u64> = plan.iter().map(|(_, d)| *d).collect();
        let mut sorted = delays.clone();
        sorted.sort_unstable();
        assert_eq!(delays, sorted);
    }

    #[test]
    fn staggered_plan_empty_input() {
        assert!(build_staggered_plan(Vec::new()).unwrap().is_empty());
    }
}
