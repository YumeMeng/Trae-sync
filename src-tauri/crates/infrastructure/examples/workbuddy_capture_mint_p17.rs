//! P17 真机捕获执行器：调用完整捕获编排，把 WorkBuddy 登录凭据铸造入池
//! （WBSESSN1 明文容器落盘）。用户已单次确认（2026-10-19 会话内 AskUserQuestion）。

use std::path::PathBuf;

use traesync_infrastructure::workbuddy_capture::capture_workbuddy_login_from_machine;
use traesync_infrastructure::storage_roots::workbuddy_login_root_from;
use traesync_infrastructure::workbuddy_login_credential::WorkBuddyLoginStore;

fn main() {
    let client_exe = PathBuf::from(r"E:\软件\workbuddy\WorkBuddy.exe");
    let local = std::env::var("LOCALAPPDATA").expect("LOCALAPPDATA unavailable");
    let root = workbuddy_login_root_from(&PathBuf::from(local).join("Trae Sync").join("data"));
    let store = WorkBuddyLoginStore::new(&root);

    println!("开始真机捕获（单次确认已由用户会话内给出）……");
    match capture_workbuddy_login_from_machine(&client_exe, &store, now_unix_seconds()) {
        Ok(record) => {
            let summary = record.summary();
            println!("---- 捕获成功 ----");
            println!("profile_id：{}", summary.profile_id);
            println!("account_id：{}", summary.account_id);
            println!("screen_name：{}", summary.screen_name);
            println!("domain：{}", summary.domain);
            println!("access 过期时刻：{}（unix 秒）", summary.access_token_expires_at_unix_seconds);
            println!("容器落盘：{}", root.display());
        }
        Err(error) => {
            eprintln!("捕获失败：{}（{error:?}）", error.code());
            std::process::exit(1);
        }
    }
}

fn now_unix_seconds() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock before epoch")
        .as_secs()
}
