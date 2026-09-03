//! 主库实际登录态探针：直接解密 auth blob 读取当前登录 userId（只读）。
//!
//! 背景（2026-09-03 卡死案例）：官方退出/登录后，环境注册表不随之更新，
//! 工具显示的「当前账号」停留在旧值。本探针验证修复方案的核心检测：
//! `archive_login_user_id`（读 `iCubeAuthInfo://icube.cloudide` blob 明文
//! userId）能否直接得到主库真实登录账号。
//!
//! 用法：`master_login_probe <instance_dir>...`
//! 输出：每个目录一行 JSON（user_id / 解析结果），不含敏感 token。
//!
//! 铁律：只读，零写回。

use std::path::PathBuf;
use std::process::ExitCode;

use serde_json::json;
use traesync_infrastructure::archive_login_user_id;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        eprintln!("用法: master_login_probe <instance_dir>...");
        return ExitCode::FAILURE;
    }
    let mut rows = Vec::new();
    for dir in &args {
        let path = PathBuf::from(dir);
        let user_id = archive_login_user_id(&path);
        rows.push(json!({
            "dir": dir,
            "user_id": user_id,
        }));
    }
    println!("{}", json!({ "tool": "master-login-probe", "results": rows }));
    ExitCode::SUCCESS
}
