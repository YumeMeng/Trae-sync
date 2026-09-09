//! 签到账号总览（UI 展示聚合层，tickets P1-3 完整档）。
//!
//! 职责：把账号注册表档案、积分缓存、凭据包令牌到期时间聚合成
//! 非敏感展示 DTO；并在 run_checkin 完成后更新积分缓存。
//! 约定（P1-3）：积分随签到结果刷新；离线/未签到时显示上次缓存值并标注时间，
//! 不为此消耗 status API 的风控预算（每账号每日 <= 3 次）。

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;
use traesync_domain::{
    CheckinOutcome, CheckinResult, CheckinStatusSnapshot, EntitlementUsageSnapshot,
};
use traesync_infrastructure::account_registry::AccountRegistry;
use traesync_infrastructure::checkin_credential::{CheckinCredentialStore, CheckinProfileBinding};
use traesync_infrastructure::checkin_http::TRAE_SOLO_CLIENT_ID;

/// 积分缓存文件名（位于 `<storage_root>/checkin` 下）。
const CREDITS_CACHE_FILE: &str = "credits-cache.json";
/// 缓存文件体积上限：远超 1KB/账号即视为损坏，读取按空缓存处理。
const MAX_CACHE_BYTES: u64 = 64 * 1024;

/// 单账号展示条目（非敏感白名单：不含公钥/令牌）。
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CheckinOverviewEntryDto {
    pub profile_id: String,
    pub screen_name: String,
    /// 服务端用户 ID（JWT 载荷，非敏感数字 ID）；详情页技术细节区展示。
    pub account_id: String,
    pub created_at: Option<String>,
    pub last_verified_at: Option<String>,
    /// 上次签到后缓存的积分；None = 本机从未签到过。
    pub credits: Option<i64>,
    /// 积分缓存时间（RFC3339）；与 credits 同生共死。
    pub credits_cached_at: Option<String>,
    /// 真实模型额度剩余（`ide_user_ent_usage` 汇总，非签到活动积分）。
    pub usage_remaining_credits: Option<f64>,
    /// 额度缓存时间（RFC3339）；与 usage_remaining_credits 同生共死。
    pub usage_cached_at: Option<String>,
    /// 上次签到后缓存的“今日已签”状态；仅当缓存时刻落在本地今日时上报，
    /// 隔日缓存返回 None（签到是当日语义，跨日不滚动沿用）。
    pub checked_in: Option<bool>,
    /// 今日最后一次签到尝试的结果码（"ok" = 签到成功；"business:9074" /
    /// "transport:network_error" 等 = 今日尝试失败，供“签到失败/待重试”
    /// 徽章判定）；None = 今日从未尝试（隔日尝试结果不滚动沿用，
    /// 与 checked_in 同一日界口径）。
    pub last_attempt_outcome: Option<String>,
    /// 今日最后一次签到尝试的本地日期（YYYY-MM-DD；与 outcome 同一日界
    /// 过滤输出）。前端据此判定“今日尝试，跨日不残留”；None = 今日无尝试。
    pub last_attempt_date: Option<String>,
    /// access token 到期时刻（Unix 秒）；凭据包缺失/不可读时为 None。
    pub access_token_expires_at_unix_seconds: Option<u64>,
    /// refresh token 到期时刻（Unix 秒）。
    pub refresh_token_expires_at_unix_seconds: Option<u64>,
    /// 设备 ID 尾 4 位（卡片徽章口径，不暴露全文）。
    pub device_tail: Option<String>,
    /// 完整设备 ID：仅详情页“技术细节”折叠区展示（本机排障用）。
    pub device_id: Option<String>,
    /// 本地备注名（用户自定义别名）；None = 使用服务端 screen_name。
    /// 前端展示名口径：display_name ?? screen_name。
    pub display_name: Option<String>,
    /// 脱敏手机号（登录/补采写入）；空串 = 未采集。
    pub masked_mobile: String,
    /// 完整手机号（G11 手工补录，凭据包 DPAPI 加密存储）；None = 未补录
    /// （展示回退脱敏号）。展示口径：mobile_full ?? masked_mobile。
    pub mobile_full: Option<String>,
    /// 该账号是否参与自动签到（详情页复选框数据源；grill 2026-08-23 决策 2）。
    pub auto_checkin_enabled: bool,
    /// 最近一次额度刷新失败原因码（成功刷新后清除）；卡片持续显示“刷新失败”标记。
    pub refresh_error_code: Option<String>,
    /// 凭据包属于已退役的旧 Work 通道（client_id 非 SOLO）：登录态直接按
    /// “登录失效”处理（页面加载即本地判定，零网络），唯一恢复方式是重新登录。
    pub credential_legacy: bool,
}

/// 缓存文件结构（磁盘格式 v1）。
#[derive(Debug, Clone, Serialize, Deserialize)]
struct CreditsCacheFile {
    format_version: u32,
    #[serde(default)]
    entries: BTreeMap<String, CreditsCacheEntry>,
}

/// 单条积分缓存。
#[derive(Debug, Clone, Serialize, Deserialize)]
struct CreditsCacheEntry {
    credits: Option<i64>,
    checked_in: Option<bool>,
    cached_at_unix_seconds: u64,
    /// 真实模型额度剩余（serde default 兼容 v1 旧缓存文件）。
    #[serde(default)]
    usage_remaining_credits: Option<f64>,
    /// 额度缓存时刻；None 表示该条目尚无额度数据。
    #[serde(default)]
    usage_cached_at_unix_seconds: Option<u64>,
    /// 最近一次额度刷新失败原因码；成功刷新/签到后清除（serde default 兼容旧文件）。
    #[serde(default)]
    refresh_error_code: Option<String>,
    /// 今日最后一次签到尝试的日期（本地时区 YYYY-MM-DD）；None = 从未尝试。
    /// 隔日不滚动沿用：build_overview 只在日期为本地今日时输出尝试结果。
    #[serde(default)]
    last_attempt_date: Option<String>,
    /// 今日最后一次签到尝试的结果码（"ok" / "business:9074" /
    /// "transport:network_error"）。铁律：失败证据只被新的尝试覆盖，
    /// 不因只读查询/成功刷新而清除。
    #[serde(default)]
    last_attempt_outcome: Option<String>,
}

impl Default for CreditsCacheFile {
    fn default() -> Self {
        Self {
            format_version: 1,
            entries: BTreeMap::new(),
        }
    }
}

fn cache_path(material_root: &Path) -> std::path::PathBuf {
    material_root.join(CREDITS_CACHE_FILE)
}

/// 读取积分缓存；文件不存在返回空表，损坏或超限时按空表处理（展示缓存，
/// 不删除损坏文件——保留失败证据，由下次写回覆盖修复）。
fn read_credits_cache(material_root: &Path) -> BTreeMap<String, CreditsCacheEntry> {
    let path = cache_path(material_root);
    let Ok(metadata) = std::fs::symlink_metadata(&path) else {
        return BTreeMap::new();
    };
    if !metadata.is_file() || metadata.len() > MAX_CACHE_BYTES {
        return BTreeMap::new();
    }
    let Ok(content) = std::fs::read_to_string(&path) else {
        return BTreeMap::new();
    };
    serde_json::from_str::<CreditsCacheFile>(&content)
        .map(|file| file.entries)
        .unwrap_or_default()
}

/// 用签到结果更新积分缓存（原子写：先写临时文件再改名，避免半截 JSON）。
/// 仅记录带有 after 快照的结果（未登录/拦截等失败结果没有可缓存值）。
/// 签到结果不携带额度数据：保留该条目已有的 usage 字段。
pub fn update_credits_cache(material_root: &Path, results: &[CheckinResult]) {
    if results.is_empty() {
        return;
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0);
    let mut cache = CreditsCacheFile {
        entries: read_credits_cache(material_root),
        ..CreditsCacheFile::default()
    };
    for result in results {
        let Some(after) = &result.after else {
            continue;
        };
        let previous = cache.entries.remove(&result.profile_id);
        cache.entries.insert(
            result.profile_id.clone(),
            CreditsCacheEntry {
                credits: after.credits,
                checked_in: Some(after.checked_in),
                cached_at_unix_seconds: now,
                usage_remaining_credits: previous
                    .as_ref()
                    .and_then(|entry| entry.usage_remaining_credits),
                usage_cached_at_unix_seconds: previous
                    .as_ref()
                    .and_then(|entry| entry.usage_cached_at_unix_seconds),
                // 签到成功 = 凭据链路健康，清除历史失败标记。
                refresh_error_code: None,
                // 尝试证据由 update_last_attempt 随后写入；这里保留旧值防丢。
                last_attempt_date: previous
                    .as_ref()
                    .and_then(|entry| entry.last_attempt_date.clone()),
                last_attempt_outcome: previous
                    .as_ref()
                    .and_then(|entry| entry.last_attempt_outcome.clone()),
            },
        );
    }
    if std::fs::create_dir_all(material_root).is_err() {
        return;
    }
    let Ok(payload) = serde_json::to_string_pretty(&cache) else {
        return;
    };
    let destination = cache_path(material_root);
    let temp = destination.with_extension("json.tmp");
    // 写缓存失败不影响签到结果返回：这里只做展示增强。
    if std::fs::write(&temp, payload).is_ok() {
        let _ = std::fs::rename(&temp, &destination);
    }
}

/// 用只读 status 快照更新积分缓存（refresh_checkin_credits 用）。
/// 仅写入成功取得快照的账号；失败账号保留旧缓存值。
/// status 不携带额度数据：保留该条目已有的 usage 字段。
pub fn update_credits_from_snapshots(
    material_root: &Path,
    snapshots: &[(String, CheckinStatusSnapshot)],
) {
    if snapshots.is_empty() {
        return;
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0);
    let mut cache = CreditsCacheFile {
        entries: read_credits_cache(material_root),
        ..CreditsCacheFile::default()
    };
    for (profile_id, snapshot) in snapshots {
        let previous = cache.entries.remove(profile_id);
        cache.entries.insert(
            profile_id.clone(),
            CreditsCacheEntry {
                credits: snapshot.credits,
                checked_in: Some(snapshot.checked_in),
                cached_at_unix_seconds: now,
                usage_remaining_credits: previous
                    .as_ref()
                    .and_then(|entry| entry.usage_remaining_credits),
                usage_cached_at_unix_seconds: previous
                    .as_ref()
                    .and_then(|entry| entry.usage_cached_at_unix_seconds),
                // 刷新成功：清除失败标记（持久化标记只在持续失败期间展示）。
                refresh_error_code: None,
                // 铁律：只读查询不得清除尝试证据（last_attempt_* 只被新尝试覆盖）。
                last_attempt_date: previous
                    .as_ref()
                    .and_then(|entry| entry.last_attempt_date.clone()),
                last_attempt_outcome: previous
                    .as_ref()
                    .and_then(|entry| entry.last_attempt_outcome.clone()),
            },
        );
    }
    write_credits_cache(material_root, &cache);
}

/// 记录额度刷新失败账号的原因码（refresh_checkin_credits 用）：
/// 与成功快照集合互斥，保留条目其余字段；成功刷新/签到时清除。
pub fn update_refresh_failures(material_root: &Path, failures: &[(String, String)]) {
    if failures.is_empty() {
        return;
    }
    let mut cache = CreditsCacheFile {
        entries: read_credits_cache(material_root),
        ..CreditsCacheFile::default()
    };
    for (profile_id, error_code) in failures {
        let previous = cache
            .entries
            .remove(profile_id)
            .unwrap_or(CreditsCacheEntry {
                credits: None,
                checked_in: None,
                cached_at_unix_seconds: 0,
                usage_remaining_credits: None,
                usage_cached_at_unix_seconds: None,
                refresh_error_code: None,
                last_attempt_date: None,
                last_attempt_outcome: None,
            });
        cache.entries.insert(
            profile_id.clone(),
            CreditsCacheEntry {
                refresh_error_code: Some(error_code.clone()),
                ..previous
            },
        );
    }
    write_credits_cache(material_root, &cache);
}

/// 清除已成功完成凭据维护账号的展示失败标记；不触碰积分、额度和签到证据。
pub fn clear_refresh_failures(material_root: &Path, profile_ids: &[String]) {
    if profile_ids.is_empty() {
        return;
    }
    let mut cache = CreditsCacheFile {
        entries: read_credits_cache(material_root),
        ..CreditsCacheFile::default()
    };
    let mut changed = false;
    for profile_id in profile_ids {
        if let Some(entry) = cache.entries.get_mut(profile_id) {
            changed |= entry.refresh_error_code.take().is_some();
        }
    }
    if changed {
        write_credits_cache(material_root, &cache);
    }
}

/// 用 `ide_user_ent_usage` 快照更新额度缓存（refresh_checkin_credits 用）。
/// 仅写入成功取得快照的账号；保留其余字段不动。
pub fn update_usage_cache(material_root: &Path, usages: &[(String, EntitlementUsageSnapshot)]) {
    if usages.is_empty() {
        return;
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0);
    let mut cache = CreditsCacheFile {
        entries: read_credits_cache(material_root),
        ..CreditsCacheFile::default()
    };
    for (profile_id, usage) in usages {
        // 条目可能尚不存在（从未签到）：用 Default 填充积分侧字段。
        let previous = cache
            .entries
            .remove(profile_id)
            .unwrap_or(CreditsCacheEntry {
                credits: None,
                checked_in: None,
                cached_at_unix_seconds: 0,
                usage_remaining_credits: None,
                usage_cached_at_unix_seconds: None,
                refresh_error_code: None,
                last_attempt_date: None,
                last_attempt_outcome: None,
            });
        cache.entries.insert(
            profile_id.clone(),
            CreditsCacheEntry {
                usage_remaining_credits: Some(usage.remaining_credits),
                usage_cached_at_unix_seconds: Some(now),
                ..previous
            },
        );
    }
    write_credits_cache(material_root, &cache);
}

/// 移除单账号积分缓存条目（账号删除流程）；文件整体重写，原子替换。
pub fn remove_credits_cache_entry(material_root: &Path, profile_id: &str) {
    let mut cache = CreditsCacheFile {
        entries: read_credits_cache(material_root),
        ..CreditsCacheFile::default()
    };
    if cache.entries.remove(profile_id).is_none() {
        return;
    }
    write_credits_cache(material_root, &cache);
}

/// 签到结果 -> 今日尝试结果码（写入 last_attempt_outcome，G10 状态机数据地基）。
/// 映射规则：
/// - 签到成功（claim 成功或 status 探测发现已签）→ "ok"
/// - 业务码失败（HTTP 200 但 code 非 0，如 9074/9095）→ "business:{业务码}"
///   （detail_code 形如 "business_9074"，取数值部分；9074 上下文标注
///   device_too_new 语义仍是业务拒绝，原样保留）
/// - 网络/凭据失败 → "transport:{映射码}"（复用 checkin_transport_error_code
///   码表：network_error / auth_mismatch / credential_refresh_failed 等）
pub fn checkin_last_attempt_outcome(result: &CheckinResult) -> String {
    match result.outcome {
        CheckinOutcome::Claimed | CheckinOutcome::AlreadyCheckedIn => "ok".to_string(),
        CheckinOutcome::NotEligible => match result.detail_code.as_deref() {
            Some(code) if code.starts_with("business_") => {
                format!("business:{}", &code["business_".len()..])
            }
            Some(code) => format!("business:{code}"),
            // 服务端判定不可领取但无业务码（如签到活动未开启）。
            None => "not_eligible".to_string(),
        },
        // 网络/凭据等传输失败（含 VerificationFailed 结果待复核）：
        // 透传 detail_code 中的现有映射码。待复核归入传输类按“待重试”呈现，
        // 不当作确定性失败。
        _ => format!(
            "transport:{}",
            result.detail_code.as_deref().unwrap_or("unknown")
        ),
    }
}

/// 记录账号今日最后一次签到尝试的结果（G10a：失败证据不丢）。
/// 无论成败都写：last_attempt_* 只被新的尝试覆盖，不被只读查询清除。
/// 条目可能尚不存在（从未签到就失败的账号）：其余字段按空值创建。
pub fn update_last_attempt(material_root: &Path, profile_id: &str, outcome: &str) {
    let mut cache = CreditsCacheFile {
        entries: read_credits_cache(material_root),
        ..CreditsCacheFile::default()
    };
    let previous = cache
        .entries
        .remove(profile_id)
        .unwrap_or(CreditsCacheEntry {
            credits: None,
            checked_in: None,
            cached_at_unix_seconds: 0,
            usage_remaining_credits: None,
            usage_cached_at_unix_seconds: None,
            refresh_error_code: None,
            last_attempt_date: None,
            last_attempt_outcome: None,
        });
    cache.entries.insert(
        profile_id.to_string(),
        CreditsCacheEntry {
            last_attempt_date: Some(local_today_string()),
            last_attempt_outcome: Some(outcome.to_string()),
            ..previous
        },
    );
    write_credits_cache(material_root, &cache);
}

fn write_credits_cache(material_root: &Path, cache: &CreditsCacheFile) {
    if std::fs::create_dir_all(material_root).is_err() {
        return;
    }
    let Ok(payload) = serde_json::to_string_pretty(cache) else {
        return;
    };
    let destination = cache_path(material_root);
    let temp = destination.with_extension("json.tmp");
    // 写缓存失败不影响调用方主流程：缓存只做展示增强。
    if std::fs::write(&temp, payload).is_ok() {
        let _ = std::fs::rename(&temp, &destination);
    }
}

/// 聚合账号总览：注册表档案 + 积分缓存 + 凭据包到期时间。
/// 注册表损坏按 checkin_registry_invalid 报错（与签到一致，fail-visible）；
/// 单账号凭据读取失败只置空令牌信息，不拖垮整页展示。
pub fn build_overview(material_root: &Path) -> Result<Vec<CheckinOverviewEntryDto>, String> {
    let registry = AccountRegistry::new(material_root);
    let records = registry
        .load()
        .map_err(|_| "checkin_registry_invalid".to_string())?;
    let cache = read_credits_cache(material_root);
    let store = CheckinCredentialStore::new(material_root);
    // 今日日期只算一次：last_attempt_* 的日界判定与 checked_in 同一自然日口径。
    let today = local_today_string();

    let entries = records
        .into_iter()
        .map(|record| {
            // 凭据包到期时间与通道判定：解密读取只取时间戳/client_id，
            // 不向 DTO 泄露任何令牌材料。
            let bundle = CheckinProfileBinding::new(
                record.profile_id.clone(),
                record.account_id.clone(),
                record.device_id.clone(),
                record.device_public_key.clone(),
            );
            let (access_expires, refresh_expires, credential_legacy, mobile_full) = store
                .load(&bundle)
                .map(|bundle| {
                    (
                        Some(bundle.access_token_expires_at_unix_seconds),
                        Some(bundle.refresh_token_expires_at_unix_seconds),
                        // 旧 Work 通道（client_id 非 SOLO）已于 2026-09-02 退役：
                        // 本地零网络判定，登录态直接按“登录失效”处理。
                        bundle.client_id != TRAE_SOLO_CLIENT_ID,
                        // G11 补录的完整手机号（凭据包密文内读出；未补录为 None）。
                        bundle.mobile_full,
                    )
                })
                .unwrap_or((None, None, false, None));
            let cached = cache.get(&record.profile_id);
            // 尾 4 位：与签到结果“兜底设备 …XXXX”同一展示口径。
            let device_tail = if record.device_id.len() >= 4 {
                Some(record.device_id[record.device_id.len() - 4..].to_string())
            } else {
                None
            };
            CheckinOverviewEntryDto {
                profile_id: record.profile_id,
                screen_name: record.screen_name,
                account_id: record.account_id,
                created_at: unix_seconds_to_rfc3339(record.created_at_unix_seconds),
                last_verified_at: unix_seconds_to_rfc3339(record.last_verified_at_unix_seconds),
                credits: cached.and_then(|entry| entry.credits),
                credits_cached_at: cached
                    .map(|entry| unix_seconds_to_rfc3339(entry.cached_at_unix_seconds))
                    .flatten(),
                usage_remaining_credits: cached.and_then(|entry| entry.usage_remaining_credits),
                usage_cached_at: cached
                    .and_then(|entry| entry.usage_cached_at_unix_seconds)
                    .and_then(unix_seconds_to_rfc3339),
                // “已签”只在缓存来自本地今日时上报：隔日缓存不再滚动沿用，
                // 否则失效账号会日复一日显示“签到成功”（2026-09-03 修复）。
                checked_in: cached
                    .filter(|entry| cached_on_local_today(entry.cached_at_unix_seconds))
                    .and_then(|entry| entry.checked_in),
                // 今日尝试结果同样以日界过滤：昨日失败到了今天就是“未尝试”，
                // 失败徽章只对今日的尝试生效（G10a）。
                // date 与 outcome 同源输出：前端“今日尝试”判定需要两者成对出现。
                last_attempt_outcome: cached
                    .filter(|entry| entry.last_attempt_date.as_deref() == Some(today.as_str()))
                    .and_then(|entry| entry.last_attempt_outcome.clone()),
                last_attempt_date: cached
                    .filter(|entry| entry.last_attempt_date.as_deref() == Some(today.as_str()))
                    .and_then(|entry| entry.last_attempt_date.clone()),
                access_token_expires_at_unix_seconds: access_expires,
                refresh_token_expires_at_unix_seconds: refresh_expires,
                device_tail,
                device_id: Some(record.device_id),
                display_name: record.display_name,
                masked_mobile: record.masked_mobile,
                mobile_full,
                auto_checkin_enabled: record.auto_checkin_enabled,
                refresh_error_code: cached.and_then(|entry| entry.refresh_error_code.clone()),
                credential_legacy,
            }
        })
        .collect();
    Ok(entries)
}

/// 判断缓存时刻是否落在本地今日（按自然日判定，与签到服务端的日配额口径一致）。
fn cached_on_local_today(cached_at_unix_seconds: u64) -> bool {
    use chrono::TimeZone as _;
    let Some(cached) = chrono::Local
        .timestamp_opt(cached_at_unix_seconds as i64, 0)
        .single()
    else {
        return false;
    };
    cached.date_naive() == chrono::Local::now().date_naive()
}

/// 本地时区今日日期（YYYY-MM-DD）：与 checked_in 的自然日判定同一口径，
/// 供 last_attempt_date 写入与日界过滤使用。
fn local_today_string() -> String {
    chrono::Local::now().date_naive().format("%Y-%m-%d").to_string()
}

/// Unix 秒 -> RFC3339；超界时间（注册表防御性允许 0）映射为 None 展示“尚未验证”。
fn unix_seconds_to_rfc3339(seconds: u64) -> Option<String> {
    let time =
        std::time::SystemTime::UNIX_EPOCH.checked_add(std::time::Duration::from_secs(seconds))?;
    // 与 lib.rs 的 system_time_to_rfc3339 保持同一格式（UTC 毫秒）。
    let datetime = chrono::DateTime::<chrono::Utc>::from(time);
    Some(datetime.to_rfc3339_opts(chrono::SecondsFormat::Millis, true))
}

#[cfg(test)]
mod tests {
    use super::*;
    use traesync_domain::CheckinOutcome;
    use traesync_domain::CheckinStatusSnapshot;
    use traesync_domain::CheckinTaskState;

    fn temp_root(tag: &str) -> std::path::PathBuf {
        let root =
            std::env::temp_dir().join(format!("traesync-overview-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    fn result_with_after(profile_id: &str, credits: i64, checked_in: bool) -> CheckinResult {
        CheckinResult {
            profile_id: profile_id.to_string(),
            outcome: if checked_in {
                CheckinOutcome::Claimed
            } else {
                CheckinOutcome::AlreadyCheckedIn
            },
            state: CheckinTaskState::Completed,
            claim_attempted: true,
            before: None,
            after: Some(CheckinStatusSnapshot {
                enabled: true,
                checked_in,
                credits: Some(credits),
                business_code: None,
            }),
            detail_code: None,
            started_at: std::time::SystemTime::UNIX_EPOCH,
            finished_at: std::time::SystemTime::UNIX_EPOCH,
        }
    }

    #[test]
    fn credits_cache_roundtrip_keeps_existing_entries() {
        let root = temp_root("roundtrip");
        update_credits_cache(&root, &[result_with_after("p1", 120, true)]);
        update_credits_cache(&root, &[result_with_after("p2", 30, false)]);
        // 第二次写回不应丢失 p1 的缓存。
        let cache = read_credits_cache(&root);
        assert_eq!(cache.get("p1").unwrap().credits, Some(120));
        assert_eq!(cache.get("p2").unwrap().credits, Some(30));
        assert_eq!(cache.get("p2").unwrap().checked_in, Some(false));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn refresh_failure_marker_persists_until_success() {
        let root = temp_root("refresh-failure");
        // 失败标记写入：新账号（无既有条目）与已有缓存账号都保留其余字段。
        update_credits_cache(&root, &[result_with_after("p1", 120, true)]);
        update_refresh_failures(
            &root,
            &[
                ("p1".to_string(), "credential_refresh_failed".to_string()),
                ("p2".to_string(), "network_error".to_string()),
            ],
        );
        let cache = read_credits_cache(&root);
        assert_eq!(
            cache.get("p1").unwrap().refresh_error_code.as_deref(),
            Some("credential_refresh_failed")
        );
        // p1 的积分缓存值不被失败标记覆盖。
        assert_eq!(cache.get("p1").unwrap().credits, Some(120));
        assert_eq!(
            cache.get("p2").unwrap().refresh_error_code.as_deref(),
            Some("network_error")
        );
        // p1 刷新成功后标记清除，p2 标记保留（成功集合与失败集合互斥）。
        update_credits_cache(&root, &[result_with_after("p1", 130, true)]);
        let cache = read_credits_cache(&root);
        assert_eq!(cache.get("p1").unwrap().refresh_error_code, None);
        assert!(cache.get("p2").unwrap().refresh_error_code.is_some());

        // 后台维护成功也能独立清除标记，不需要额外触发一次 status 网络请求。
        update_refresh_failures(&root, &[("p1".to_string(), "network_error".to_string())]);
        clear_refresh_failures(&root, &["p1".to_string()]);
        assert_eq!(
            read_credits_cache(&root)
                .get("p1")
                .unwrap()
                .refresh_error_code,
            None
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn corrupted_cache_reads_as_empty_without_delete() {
        let root = temp_root("corrupt");
        std::fs::write(cache_path(&root), "{ not json").unwrap();
        assert!(read_credits_cache(&root).is_empty());
        // 铁律：损坏文件是失败证据，读取路径不得删除它。
        assert!(cache_path(&root).exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn build_overview_without_registry_returns_empty_list() {
        let root = temp_root("empty");
        // 无 accounts.json = 从未登录，注册表按空表处理而非报错。
        assert!(build_overview(&root).unwrap().is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 注册一个最小档案：字段只需通过 validate（非空、不超长），
    /// 凭据包不存在时 build_overview 对令牌字段取 None，不影响 checked_in 断言。
    fn register_minimal_account(root: &Path, profile_id: &str) {
        AccountRegistry::new(root)
            .upsert(&traesync_infrastructure::account_registry::AccountRecord {
                profile_id: profile_id.to_string(),
                account_id: "aid".to_string(),
                screen_name: "name".to_string(),
                avatar_url: String::new(),
                device_id: "device".to_string(),
                device_public_key: "pk".to_string(),
                display_name: None,
                masked_mobile: String::new(),
                created_at_unix_seconds: 1,
                last_verified_at_unix_seconds: 1,
                device_created_at_unix_seconds: 0,
                auto_checkin_enabled: true,
                archived: false,
            })
            .unwrap();
    }

    /// 直接落一份指定时刻的缓存（绕开 update_credits_cache 的“now”取值）。
    fn write_cache_at(root: &Path, profile_id: &str, checked_in: bool, cached_at: u64) {
        let mut cache = CreditsCacheFile::default();
        cache.entries.insert(
            profile_id.to_string(),
            CreditsCacheEntry {
                credits: Some(120),
                checked_in: Some(checked_in),
                cached_at_unix_seconds: cached_at,
                usage_remaining_credits: None,
                usage_cached_at_unix_seconds: None,
                refresh_error_code: None,
                last_attempt_date: None,
                last_attempt_outcome: None,
            },
        );
        write_credits_cache(root, &cache);
    }

    #[test]
    fn checked_in_from_previous_day_is_not_reported_as_today() {
        let root = temp_root("stale-day");
        register_minimal_account(&root, "p1");
        // 昨天（本地日界前 26 小时必落在前一自然日）签到成功留下的缓存。
        let yesterday = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs()
            - 26 * 3600;
        write_cache_at(&root, "p1", true, yesterday);
        let entry = build_overview(&root)
            .unwrap()
            .into_iter()
            .find(|entry| entry.profile_id == "p1")
            .unwrap();
        // 隔日“已签”不得当作今日状态：否则失效账号永远显示签到成功。
        assert_eq!(entry.checked_in, None);
        // 积分是余额语义，不随日界失效，保留展示。
        assert_eq!(entry.credits, Some(120));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn checked_in_from_today_is_reported() {
        let root = temp_root("today-day");
        register_minimal_account(&root, "p1");
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        write_cache_at(&root, "p1", true, now);
        let entry = build_overview(&root)
            .unwrap()
            .into_iter()
            .find(|entry| entry.profile_id == "p1")
            .unwrap();
        assert_eq!(entry.checked_in, Some(true));
        let _ = std::fs::remove_dir_all(&root);
    }

    // ===== G10a：今日签到尝试结果（last_attempt_*）=====
    // 账号页/签到页要区分"今日签到失败"与"从未尝试"（只有 checked_in 布尔
    // 时失败账号显示"未签"，无法区分），失败证据必须落缓存。

    /// 构造一个失败结果（无 after 快照）：映射规则测试用。
    fn result_with_failure(
        profile_id: &str,
        outcome: CheckinOutcome,
        detail_code: Option<&str>,
    ) -> CheckinResult {
        CheckinResult {
            profile_id: profile_id.to_string(),
            outcome,
            state: CheckinTaskState::Completed,
            claim_attempted: true,
            before: None,
            after: None,
            detail_code: detail_code.map(str::to_string),
            started_at: std::time::SystemTime::UNIX_EPOCH,
            finished_at: std::time::SystemTime::UNIX_EPOCH,
        }
    }

    #[test]
    fn last_attempt_outcome_mapping_follows_ticket_rules() {
        // 签到成功（claim 成功或 status 探测发现已签）→ "ok"。
        assert_eq!(
            checkin_last_attempt_outcome(&result_with_after("p1", 120, true)),
            "ok"
        );
        assert_eq!(
            checkin_last_attempt_outcome(&result_with_after("p2", 30, false)),
            "ok"
        );
        // 业务码失败（HTTP 200 但 code 非 0）→ "business:{业务码}"。
        assert_eq!(
            checkin_last_attempt_outcome(&result_with_failure(
                "p3",
                CheckinOutcome::NotEligible,
                Some("business_9074")
            )),
            "business:9074"
        );
        // 9074 上下文标注（device_too_new）语义仍是业务拒绝，原样保留。
        assert_eq!(
            checkin_last_attempt_outcome(&result_with_failure(
                "p4",
                CheckinOutcome::NotEligible,
                Some("device_too_new")
            )),
            "business:device_too_new"
        );
        // 网络/凭据失败 → "transport:{映射码}"（复用现有错误码常量）。
        assert_eq!(
            checkin_last_attempt_outcome(&result_with_failure(
                "p5",
                CheckinOutcome::NetworkError,
                Some("network_error")
            )),
            "transport:network_error"
        );
        assert_eq!(
            checkin_last_attempt_outcome(&result_with_failure(
                "p6",
                CheckinOutcome::CredentialRefreshFailed,
                Some("credential_refresh_failed")
            )),
            "transport:credential_refresh_failed"
        );
    }

    #[test]
    fn update_last_attempt_writes_outcome_and_keeps_other_fields() {
        let root = temp_root("last-attempt");
        update_credits_cache(&root, &[result_with_after("p1", 120, true)]);
        update_last_attempt(&root, "p1", "business:9074");
        let cache = read_credits_cache(&root);
        let entry = cache.get("p1").unwrap();
        assert_eq!(
            entry.last_attempt_outcome.as_deref(),
            Some("business:9074")
        );
        assert_eq!(
            entry.last_attempt_date.as_deref(),
            Some(local_today_string().as_str())
        );
        // 其他字段不丢：积分/已签状态仍来自签到写回。
        assert_eq!(entry.credits, Some(120));
        assert_eq!(entry.checked_in, Some(true));
        // 从未签到过的账号也能单独记录尝试结果（其余字段按空值创建）。
        update_last_attempt(&root, "p2", "transport:network_error");
        let cache = read_credits_cache(&root);
        let entry = cache.get("p2").unwrap();
        assert_eq!(
            entry.last_attempt_outcome.as_deref(),
            Some("transport:network_error")
        );
        assert_eq!(entry.credits, None);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn status_refresh_does_not_clear_last_attempt() {
        let root = temp_root("attempt-keep");
        update_credits_cache(&root, &[result_with_after("p1", 120, true)]);
        update_last_attempt(&root, "p1", "transport:network_error");
        // 铁律：只读 status 刷新不得清除尝试证据（last_attempt_* 只被新尝试覆盖）。
        update_credits_from_snapshots(
            &root,
            &[(
                "p1".to_string(),
                CheckinStatusSnapshot {
                    enabled: true,
                    checked_in: true,
                    credits: Some(125),
                    business_code: None,
                },
            )],
        );
        let cache = read_credits_cache(&root);
        let entry = cache.get("p1").unwrap();
        assert_eq!(entry.credits, Some(125));
        assert_eq!(
            entry.last_attempt_outcome.as_deref(),
            Some("transport:network_error")
        );
        assert_eq!(
            entry.last_attempt_date.as_deref(),
            Some(local_today_string().as_str())
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 直接落一份带指定日期尝试结果的缓存（绕开 update_last_attempt 的"now"取值）。
    fn write_cache_with_attempt(root: &Path, profile_id: &str, date: &str, outcome: &str) {
        let mut cache = CreditsCacheFile::default();
        cache.entries.insert(
            profile_id.to_string(),
            CreditsCacheEntry {
                credits: Some(120),
                checked_in: Some(false),
                cached_at_unix_seconds: 0,
                usage_remaining_credits: None,
                usage_cached_at_unix_seconds: None,
                refresh_error_code: None,
                last_attempt_date: Some(date.to_string()),
                last_attempt_outcome: Some(outcome.to_string()),
            },
        );
        write_credits_cache(root, &cache);
    }

    #[test]
    fn last_attempt_from_previous_day_is_not_reported() {
        let root = temp_root("attempt-stale-day");
        register_minimal_account(&root, "p1");
        let yesterday = (chrono::Local::now().date_naive() - chrono::Duration::days(1))
            .format("%Y-%m-%d")
            .to_string();
        write_cache_with_attempt(&root, "p1", &yesterday, "business:9074");
        let entry = build_overview(&root)
            .unwrap()
            .into_iter()
            .find(|entry| entry.profile_id == "p1")
            .unwrap();
        // 昨日的尝试结果不得当作今日状态：今日是否已尝试以日界为准。
        assert_eq!(entry.last_attempt_outcome, None);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn last_attempt_from_today_is_reported() {
        let root = temp_root("attempt-today");
        register_minimal_account(&root, "p1");
        write_cache_with_attempt(
            &root,
            "p1",
            &local_today_string(),
            "transport:network_error",
        );
        let entry = build_overview(&root)
            .unwrap()
            .into_iter()
            .find(|entry| entry.profile_id == "p1")
            .unwrap();
        assert_eq!(
            entry.last_attempt_outcome.as_deref(),
            Some("transport:network_error")
        );
        // date 与 outcome 成对输出：前端“今日尝试”判定依赖两者同现（G10a IPC 契约）。
        assert_eq!(entry.last_attempt_date.as_deref(), Some(local_today_string().as_str()));
        let _ = std::fs::remove_dir_all(&root);
    }
}
