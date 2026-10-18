//! P17-A1 真机迁移执行器：WorkBuddy 登录代次容器 DPAPI → 明文 JSON。
//!
//! spec 10.3 拍板：个人工具明文存储，不做防御性措施；存量账号一次性
//! 解密迁移。迁移前自动把旧密文容器备份到 `workbuddy-legacy/` 下的
//! `workbuddy-login/`（E 盘软件本体旁 backups，密文形态不引入明文泄漏
//! 面）；迁移函数幂等，失败文件原位保留可重试。
//!
//! 用法：`migrate_workbuddy_login_plain_p17 [container_root] [legacy_backup_dir]`
//!   默认 container_root = `%LOCALAPPDATA%\Trae Sync\data\workbuddy-login`
//!   默认 legacy_backup_dir = `E:\trae-sync\Trae Sync\backups\workbuddy-legacy\workbuddy-login`

use std::path::PathBuf;
use std::process::ExitCode;

use traesync_infrastructure::storage_roots::workbuddy_login_root_from;
use traesync_infrastructure::workbuddy_login_credential::WorkBuddyLoginStore;

fn default_container_root() -> PathBuf {
    // 工具存储根（ADR-0035）：`%LOCALAPPDATA%\Trae Sync\data`。
    let local = std::env::var("LOCALAPPDATA").unwrap_or_default();
    workbuddy_login_root_from(&PathBuf::from(local).join("Trae Sync").join("data"))
}

fn default_legacy_backup_dir() -> PathBuf {
    PathBuf::from(r"E:\trae-sync\Trae Sync\backups\workbuddy-legacy\workbuddy-login")
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    let root = PathBuf::from(args.get(1).cloned().unwrap_or_default())
        .abs_or(default_container_root());
    let backup = PathBuf::from(args.get(2).cloned().unwrap_or_default())
        .abs_or(default_legacy_backup_dir());

    println!("容器根：{}", root.display());
    println!("旧密文备份：{}", backup.display());
    if !root.is_dir() {
        eprintln!("中止：容器根不存在（无存量可迁移）");
        return ExitCode::FAILURE;
    }

    let store = WorkBuddyLoginStore::new(&root);
    match store.list() {
        Ok(summaries) => println!("迁移前可读容器：{} 个", summaries.len()),
        Err(_) => println!("迁移前清单读取失败（损坏文件将在迁移中记录）"),
    }

    let outcome = match store.migrate_legacy_to_plain(Some(&backup)) {
        Ok(outcome) => outcome,
        Err(_) => {
            eprintln!("中止：迁移扫描失败（容器目录不可读）");
            return ExitCode::FAILURE;
        }
    };
    println!(
        "迁移完成：DPAPI→明文 {} 个；已是明文跳过 {} 个；失败 {} 个",
        outcome.migrated,
        outcome.already_plain,
        outcome.failed.len()
    );
    for profile in &outcome.failed {
        eprintln!("  失败（原密文保留，可重试）：{profile}");
    }

    match store.list() {
        Ok(summaries) => println!("迁移后可读容器：{} 个", summaries.len()),
        Err(_) => {
            eprintln!("中止：迁移后清单读取失败");
            return ExitCode::FAILURE;
        }
    }
    if outcome.failed.is_empty() {
        println!("WorkBuddy 登录凭据明文化迁移全部成功。");
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

trait AbsOr {
    fn abs_or(self, fallback: PathBuf) -> PathBuf;
}

impl AbsOr for PathBuf {
    fn abs_or(self, fallback: PathBuf) -> PathBuf {
        if self.as_os_str().is_empty() {
            fallback
        } else {
            self
        }
    }
}
