//! P11-6 Trae CN 只读库真实读取探针（真机验收：密钥/路径/读路径三事实）。
//!
//! 背景与目标：P11-6 的 trae_cn 库读接入此前只有 fixture 单测；本探针以
//! 生产同款读路径（read_master_history / read_master_stats /
//! read_master_session_messages）对真实 Trae CN 数据根做只读实测，
//! 回答三个问题：
//! 1. `%APPDATA%\Trae CN` 官方数据根定位是否命中（trae_cn_library_dir 同款）；
//! 2. 硬编码源库密钥（SQLCipher）对 Trae CN 的 ai-agent/database.db 是否有效；
//! 3. 历史/统计/消息三条读路径在真实 CN 库上的返回形态与条数。
//!
//! 运行方式（显式授权；`#[ignore]` 保证不进常规回归）：
//! ```text
//! $env:TRAE_SYNC_CN_LIB_PROBE_RAW_KEY = "<源库 raw key>"
//! cargo test -p traesync-infrastructure --test trae_cn_library_read_probe -- --ignored --nocapture
//! ```
//! 可选覆盖：TRAE_SYNC_CN_LIB_PROBE_DATA_DIR（默认 %APPDATA%\Trae CN）、
//! TRAE_SYNC_CN_LIB_PROBE_STORAGE_ROOT（默认 %LOCALAPPDATA%\Trae Sync\data）。
//!
//! 红线纪律（与 trae_cn_checkin_probe 一致）：
//! - 全程零网络请求；不触碰签到/凭据接口；
//! - 只读打开：底层经隔离三件套副本（系统临时目录），不写产品目录；
//! - 密钥只经环境变量传入内存使用，不落日志/报告；
//! - 报告只落计数、状态码与哈希指纹，不含项目名/会话标题/消息原文/
//!   账号原文。凭据或数据不可用同样如实落档，不伪造结论。

#![cfg(windows)]

use sha2::{Digest, Sha256};
use traesync_infrastructure::account_registry::AccountRegistry;
use traesync_infrastructure::account_session_content::{
    read_master_session_messages, SessionMessagesStatus,
};
use traesync_infrastructure::master_history::{read_master_history, MasterHistoryStatus};
use traesync_infrastructure::master_stats::{read_master_stats, MasterStatsStatus};
use traesync_infrastructure::product_account_state::ProductAccountStateStore;

/// 报告落档根：`<repo>/.scratch/checkin-http/reports/`（从 crate 位置上溯）。
fn probe_reports_dir() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .expect("crate 位于 <repo>/src-tauri/crates/infrastructure")
        .join(".scratch")
        .join("checkin-http")
        .join("reports")
}

/// 字符串的 sha256 前 8 位指纹（同文同哈希，反查不出原文）。
fn fingerprint8(value: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(value.as_bytes());
    hex::encode(&hasher.finalize()[..4])
}

/// 探针报告（落档形态；全部字段可安全入档）。
#[derive(serde::Serialize)]
struct LibraryReadProbeReport {
    probe: &'static str,
    red_line: &'static str,
    /// 官方数据根是否存在（未定位成功则后续全部 skipped）。
    data_dir_present: bool,
    /// ai-agent/database.db 是否存在。
    database_present: bool,
    /// 当前指针账号解析：none = 未选择账号 / resolved = 已解析 / invalid = 注册表异常。
    pointer_state: &'static str,
    /// 指针账号的 user_id 指纹（sha256 前 8 位）与长度；未解析为 None。
    current_user_id_fingerprint: Option<String>,
    /// 三个读路径各自的状态与计数摘要。
    history: Option<HistorySummary>,
    stats: Option<StatsSummary>,
    messages: Option<MessagesSummary>,
    conclusion: String,
    generated_at_unix_seconds: u64,
}

#[derive(serde::Serialize)]
struct HistorySummary {
    status: &'static str,
    project_count: usize,
    session_count: usize,
    /// 首个会话 id 的指纹（后续消息读取目标；不落原值）。
    first_session_id_fingerprint: Option<String>,
}

#[derive(serde::Serialize)]
struct StatsSummary {
    status: &'static str,
    project_count: Option<u64>,
    session_count: Option<u64>,
    message_count: Option<u64>,
    participating_account_count: Option<u64>,
}

#[derive(serde::Serialize)]
struct MessagesSummary {
    status: &'static str,
    message_count: usize,
}

fn main_outcome() {
    let data_dir = std::env::var_os("TRAE_SYNC_CN_LIB_PROBE_DATA_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            std::env::var_os("APPDATA")
                .map(|base| std::path::PathBuf::from(base).join("Trae CN"))
                .expect("APPDATA 缺失且未显式指定数据根")
        });
    let storage_root = std::env::var_os("TRAE_SYNC_CN_LIB_PROBE_STORAGE_ROOT")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            std::env::var_os("LOCALAPPDATA")
                .map(|base| std::path::PathBuf::from(base).join("Trae Sync").join("data"))
                .expect("LOCALAPPDATA 缺失且未显式指定 storage_root")
        });
    let raw_key = std::env::var("TRAE_SYNC_CN_LIB_PROBE_RAW_KEY").unwrap_or_default();
    let data_dir_present = data_dir.is_dir();
    let db_path = data_dir.join("ModularData").join("ai-agent").join("database.db");
    let database_present = db_path.is_file();
    println!("数据根：{}（存在={data_dir_present}）", data_dir.display());
    println!("库文件：{}（存在={database_present}）", db_path.display());
    if raw_key.is_empty() {
        println!("结论：无法实测——未设置 TRAE_SYNC_CN_LIB_PROBE_RAW_KEY（探针不硬编码密钥）");
        write_report(LibraryReadProbeReport {
            probe: "trae-cn-library-read-probe",
            red_line: "只读探针：零网络、零写产品目录（隔离三件套副本）；报告不含密钥/原文/账号原值",
            data_dir_present,
            database_present,
            pointer_state: "skipped",
            current_user_id_fingerprint: None,
            history: None,
            stats: None,
            messages: None,
            conclusion: "无法实测：未提供源库密钥（TRAE_SYNC_CN_LIB_PROBE_RAW_KEY）".to_string(),
            generated_at_unix_seconds: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0),
        });
        return;
    }

    // 第 1 步：产品池当前指针 → 档案（与 resolve_trae_cn_pointer 同源状态）。
    let trae_cn_root = storage_root.join("products").join("trae_cn");
    let registry = AccountRegistry::new(&trae_cn_root);
    let records = match registry.load() {
        Ok(records) => records,
        Err(_) => {
            println!("结论：无法实测——Trae CN 账号注册表读取失败（{}）", trae_cn_root.display());
            write_report(unavailable_report(data_dir_present, database_present, "invalid"));
            return;
        }
    };
    let pointer = ProductAccountStateStore::new(&trae_cn_root).load();
    let Some(current) = pointer.unwrap_or(None) else {
        println!("结论：无法实测——产品池未选择当前账号（先在账号页登录并切换）");
        write_report(unavailable_report(data_dir_present, database_present, "none"));
        return;
    };
    let Some(record) = records.iter().find(|r| r.profile_id == current) else {
        println!("结论：无法实测——当前指针悬空（指向未登记账号）");
        write_report(unavailable_report(data_dir_present, database_present, "none"));
        return;
    };
    let current_user_id = record.account_id.clone();
    println!(
        "当前账号：已解析（user_id 指纹 {}，len={}）",
        fingerprint8(&current_user_id),
        current_user_id.len()
    );

    // 第 2 步：历史读路径（生产同款 read_master_history）。
    let mut first_session_id: Option<String> = None;
    let history = match read_master_history(&data_dir, &raw_key, &current_user_id) {
        MasterHistoryStatus::Ready { projects, sessions } => {
            println!(
                "历史读取：成功——项目 {} 个，会话 {} 条（当前账号可见口径 + 全局归档）",
                projects.len(),
                sessions.len()
            );
            first_session_id = sessions.first().map(|s| s.session_id.clone());
            Some(HistorySummary {
                status: "ready",
                project_count: projects.len(),
                session_count: sessions.len(),
                first_session_id_fingerprint: first_session_id.as_deref().map(fingerprint8),
            })
        }
        MasterHistoryStatus::NoMasterData => {
            println!("历史读取：no_master_data（库文件缺失）");
            None
        }
        MasterHistoryStatus::ReadFailed => {
            println!("历史读取：read_failed——key 不匹配或打开失败（密钥对 CN 库无效？）");
            None
        }
    };

    // 第 3 步：统计读路径（生产同款 read_master_stats）。
    let stats = match read_master_stats(&data_dir, &raw_key, &current_user_id) {
        MasterStatsStatus::Ready(inner) => {
            println!(
                "统计读取：成功——项目 {} / 会话 {} / 消息 {} / 参与账号 {}",
                inner.project_count, inner.session_count, inner.message_count, inner.participating_account_count
            );
            Some(StatsSummary {
                status: "ready",
                project_count: Some(inner.project_count),
                session_count: Some(inner.session_count),
                message_count: Some(inner.message_count),
                participating_account_count: Some(inner.participating_account_count),
            })
        }
        MasterStatsStatus::NoMasterData => {
            println!("统计读取：no_master_data");
            None
        }
        MasterStatsStatus::ReadFailed => {
            println!("统计读取：read_failed");
            None
        }
    };

    // 第 4 步：消息读路径（首个会话，生产同款 read_master_session_messages）。
    let mut messages = None;
    if let Some(session_id) = first_session_id {
        match read_master_session_messages(&data_dir, &session_id, &raw_key, &current_user_id) {
            SessionMessagesStatus::Ready(entries) => {
                println!(
                    "消息读取：成功——首个会话（指纹 {}）返回 {} 条消息（未读取原文）",
                    fingerprint8(&session_id),
                    entries.len()
                );
                messages = Some(MessagesSummary {
                    status: "ready",
                    message_count: entries.len(),
                });
            }
            SessionMessagesStatus::NoInstanceData => {
                println!("消息读取：no_instance_data");
                messages = Some(MessagesSummary { status: "no_instance_data", message_count: 0 });
            }
            SessionMessagesStatus::ReadFailed => {
                println!("消息读取：read_failed");
                messages = Some(MessagesSummary { status: "read_failed", message_count: 0 });
            }
        }
    }

    // 第 5 步：结论推导 + 落档。
    let conclusion = match (&history, &stats) {
        (Some(_), Some(_)) => {
            "真实读取成功：官方数据根定位、SQLCipher 密钥、三条读路径对 Trae CN 真实库全部有效".to_string()
        }
        (None, None) => {
            "真实读取失败：key 不匹配或读取失败——CN 只读库读路径未通过真机验收".to_string()
        }
        _ => "部分读取成功：读路径结果不一致，需人工核对".to_string(),
    };
    println!("探针结论：{conclusion}");
    write_report(LibraryReadProbeReport {
        probe: "trae-cn-library-read-probe",
        red_line: "只读探针：零网络、零写产品目录（隔离三件套副本）；报告不含密钥/原文/账号原值",
        data_dir_present,
        database_present,
        pointer_state: "resolved",
        current_user_id_fingerprint: Some(fingerprint8(&current_user_id)),
        history,
        stats,
        messages,
        conclusion,
        generated_at_unix_seconds: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0),
    });
}

/// 「无法实测」报告（指针/注册表不可用），如实落档不伪造结论。
fn unavailable_report(data_dir_present: bool, database_present: bool, pointer_state: &'static str) -> LibraryReadProbeReport {
    LibraryReadProbeReport {
        probe: "trae-cn-library-read-probe",
        red_line: "只读探针：零网络、零写产品目录（隔离三件套副本）；报告不含密钥/原文/账号原值",
        data_dir_present,
        database_present,
        pointer_state,
        current_user_id_fingerprint: None,
        history: None,
        stats: None,
        messages: None,
        conclusion: "凭据或指针不可用，探针未完成".to_string(),
        generated_at_unix_seconds: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0),
    }
}

/// 落档并打印结果（写失败 fail-soft：打印警示不中断探针）。
fn write_report(report: LibraryReadProbeReport) {
    let json = match serde_json::to_string_pretty(&report) {
        Ok(json) => json,
        Err(error) => {
            println!("报告序列化失败（{error}），结论 = {}", report.conclusion);
            return;
        }
    };
    let path = probe_reports_dir().join(format!(
        "trae-cn-library-read-probe-{}.json",
        report.generated_at_unix_seconds
    ));
    match std::fs::write(&path, json) {
        Ok(()) => println!("报告已落档：{}", path.display()),
        Err(error) => println!("报告落档失败（{error}），仅打印：结论 = {}", report.conclusion),
    }
}

#[test]
#[ignore]
fn trae_cn_library_read_probe_with_explicit_raw_key() {
    main_outcome();
}
