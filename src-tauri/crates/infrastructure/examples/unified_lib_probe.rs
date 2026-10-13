//! 融合版/Work 主库账号分布对比探针（只读）。
//!
//! 背景（2026-10-13 融合版迁移研究）：官方迁移把 Work 库数据流式搬进
//! 融合版目录，需要只读核对迁移后融合版库内的账号分布，与 Work 原库
//! 对比，确认 16 个账号的历史记录归属情况。
//!
//! 用法: unified_lib_probe <data_dir>...
//! 输出: 每个目录一份只读体检报告 JSON（账号分布/孤儿行），无敏感正文。
//!
//! 铁律：只读；user_id 是服务端账号 ID，非认证材料，可显示。

use std::path::PathBuf;

use serde_json::json;
use traesync_infrastructure::master_checkup::{read_master_checkup, MasterCheckupStatus};

/// 项目已验证的 SQLCipher raw key（TECHNICAL_BASELINE 固化）。
const RAW_KEY: &str = "3605f6691095a993f03d5009c918352ef5be31ae31e8f000212b81ff058da773";

fn main() -> std::process::ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        eprintln!("用法: unified_lib_probe <data_dir>...");
        return std::process::ExitCode::FAILURE;
    }
    let mut rows = Vec::new();
    for dir in &args {
        let status = read_master_checkup(&PathBuf::from(dir), RAW_KEY);
        let entry = match status {
            MasterCheckupStatus::Ready(checkup) => json!({
                "dir": dir,
                "status": "ready",
                "accounts": checkup.accounts,
                "orphan_project_count": checkup.orphan_project_count,
                "orphan_session_count": checkup.orphan_session_count,
            }),
            MasterCheckupStatus::NoMasterData => json!({ "dir": dir, "status": "no_master_data" }),
            MasterCheckupStatus::ReadFailed => json!({ "dir": dir, "status": "read_failed" }),
        };
        rows.push(entry);
    }
    println!("{}", json!({ "tool": "unified-lib-probe", "results": rows }));
    std::process::ExitCode::SUCCESS
}
