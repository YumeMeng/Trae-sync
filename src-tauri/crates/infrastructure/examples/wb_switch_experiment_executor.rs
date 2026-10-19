//! P18-B2 切号实验真机执行器（**示例——默认零副作用，真机执行双重门禁**）。
//!
//! 纪律（2026-10-19 用户决定）：功能开发阶段不碰真机；本示例是唯一真机
//! 执行入口，且必须满足**全部**条件才会改写任何文件：
//! 1. 命令行显式传入 `--i-accept-machine-run`（第一重确认）；
//! 2. 环境变量 `TRAE_SYNC_WB_EXPERIMENT_UNLOCK=2026-10-unified-acceptance`
//!    （第二重确认——统一验收时由主理人当面设值）。
//! 缺任一条件：只打印实验计划（目标账号/形态/文件路径/步骤）后退出，
//! 零写入、零进程操作。
//!
//! 用法（统一验收时）：
//! ```text
//! # 试运行（只出计划）：
//! cargo run --release --example wb_switch_experiment_executor -- \
//!   --login-root <工具 storage>/checkin/products-workbuddy \
//!   --profile wb-<uid> --mode plain
//! # 真机执行（两重门禁齐备后）：
//! TRAE_SYNC_WB_EXPERIMENT_UNLOCK=2026-10-unified-acceptance \
//!   cargo run --release --example wb_switch_experiment_executor -- \
//!   --login-root <工具 storage>/checkin/products-workbuddy \
//!   --profile wb-<uid> --mode envelope --i-accept-machine-run
//! ```
//!
//! 风险知情（spec 8.4 修订记录）：冷启动接受度未知——失败可重登（重捕
//! 链已有）；改写前自动滚动单备份（`{info}.experiment-bak`）。

use traesync_infrastructure::workbuddy_credential::DEFAULT_AUTH_ID;
use traesync_infrastructure::workbuddy_login_credential::WorkBuddyLoginStore;
use traesync_infrastructure::workbuddy_switch_experiment::{
    fetch_derived_key, run_switch_experiment, ClientProcessState, ExperimentDeps,
    TargetTokens, WriteMode,
};
use traesync_infrastructure::workbuddy_transfer::{
    close_client_real, count_client_processes_real, observe_client_real, start_client_real,
};

const UNLOCK_ENV: &str = "TRAE_SYNC_WB_EXPERIMENT_UNLOCK";
const UNLOCK_VALUE: &str = "2026-10-unified-acceptance";
const MACHINE_RUN_FLAG: &str = "--i-accept-machine-run";

struct Args {
    login_root: Option<String>,
    profile_id: Option<String>,
    mode: WriteMode,
    machine_run: bool,
}

fn parse_args() -> Args {
    let mut args = Args {
        login_root: None,
        profile_id: None,
        mode: WriteMode::Plain,
        machine_run: false,
    };
    let mut iter = std::env::args().skip(1);
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--login-root" => args.login_root = iter.next(),
            "--profile" => args.profile_id = iter.next(),
            "--mode" => {
                args.mode = match iter.next().as_deref() {
                    Some("envelope") => WriteMode::ToolEnvelope,
                    _ => WriteMode::Plain,
                }
            }
            MACHINE_RUN_FLAG => args.machine_run = true,
            other => {
                eprintln!("未知参数：{other}");
                std::process::exit(2);
            }
        }
    }
    args
}

fn print_plan(args: &Args, executing: bool) {
    println!("=== WorkBuddy 切号实验计划 ===");
    println!("形态          : {}", match args.mode { WriteMode::Plain => "明文（5.5.x 旧形态）", WriteMode::ToolEnvelope => "工具加密信封（AES-256-GCM）" });
    println!("账号池根      : {}", args.login_root.as_deref().unwrap_or("<未提供>"));
    println!("目标 profile  : {}", args.profile_id.as_deref().unwrap_or("<未提供>"));
    println!("步骤          : 完全退出客户端 → 滚动单备份 .info → 改写 token 字段 → 冷启动 → 文件面健康复核");
    if executing {
        println!("门禁          : 双重确认齐备，将真实执行");
    } else {
        println!("门禁          : 缺 {MACHINE_RUN_FLAG} 或 {UNLOCK_ENV}——只出计划，零写入");
    }
}

fn main() {
    let args = parse_args();
    let executing = args.machine_run
        && std::env::var(UNLOCK_ENV).as_deref() == Ok(UNLOCK_VALUE);
    print_plan(&args, executing);
    if !executing {
        return;
    }
    let Some(login_root) = args.login_root.as_ref() else {
        eprintln!("缺少 --login-root");
        std::process::exit(2);
    };
    let Some(profile_id) = args.profile_id.as_ref() else {
        eprintln!("缺少 --profile");
        std::process::exit(2);
    };
    // 目标 token：账号池记录（load 返回的明文只存在于本进程内存）。
    let store = WorkBuddyLoginStore::new(login_root);
    let summaries = store.list().expect("账号池清单不可读");
    let summary = summaries
        .iter()
        .find(|summary| &summary.profile_id == profile_id)
        .unwrap_or_else(|| panic!("账号池中不存在 {profile_id}"));
    let record = store
        .load(&summary.profile_id, &summary.account_id)
        .expect("账号池记录不可读");
    let target = TargetTokens {
        access_token: record.access_token.clone(),
        refresh_token: record.refresh_token.clone(),
        access_expires_at_ms: record.access_token_expires_at_unix_seconds.saturating_mul(1000),
        refresh_expires_at_ms: record.refresh_token_expires_at_unix_seconds.saturating_mul(1000),
        session_state: record.session_state.clone(),
    };
    // 信封形态先取钥（借客户端运行时；派生密钥只出派生值）。
    let key = match args.mode {
        WriteMode::ToolEnvelope => {
            let exe = traesync_infrastructure::workbuddy_transfer::discover_install()
                .expect("未发现 WorkBuddy 安装位置")
                .exe;
            Some(fetch_derived_key(&exe).expect("取钥失败"))
        }
        WriteMode::Plain => None,
    };
    // .info 存储根（凭据层事实：LOCALAPPDATA\CodeBuddyExtension\Data\Public\auth）。
    let auth_dir = traesync_infrastructure::workbuddy_credential::auth_dir()
        .expect("LOCALAPPDATA 不可用");
    let deps = ExperimentDeps {
        observe: &observe_client_real,
        close: &close_client_real,
        start: Some(&|| {
            let exe = traesync_infrastructure::workbuddy_transfer::discover_install()
                .map(|found| found.exe)
                .expect("未发现 WorkBuddy 安装位置");
            start_client_real(&exe)
        }),
        restart_settle: std::time::Duration::from_secs(8),
        now_ms: &traesync_infrastructure::workbuddy_credential::unix_ms_now,
    };
    let _ = count_client_processes_real; // 重启确认由健康复核 + 用户人工确认承载。
    let receipt = run_switch_experiment(
        &auth_dir,
        DEFAULT_AUTH_ID,
        &target,
        args.mode,
        key.as_ref(),
        &deps,
    )
    .expect("实验执行失败（备份在 .experiment-bak，可人工回滚）");
    println!("=== 实验回执 ===");
    println!("形态          : {:?}", receipt.mode);
    println!("备份          : {}", receipt.backup_path.display());
    println!("改写          : {}", receipt.wrote_info);
    println!("冷启动        : {}", receipt.cold_started);
    println!("复核登录态    : {}", receipt.post_status.as_deref().unwrap_or("不可得"));
    println!("复核 uid      : {}", receipt.post_uid.as_deref().unwrap_or("不可得"));
    println!("后续          : 请在 WorkBuddy 客户端内人工确认登录身份与历史会话可见性");
    let _ = ClientProcessState::NotRunning; // 类型归属说明（进程三态复用 B1）。
}
