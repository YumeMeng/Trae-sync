//! P17-A3 真机验收执行器：对真实主库执行一次滚动单份备份（与切号路径
//! 同一原语 `backup_master_trio_rolling`）。
//!
//! 验收目标（spec 10.2）：
//! 1. 触发一次与切号路径完全相同的自动备份原语；
//! 2. 备份目录内只剩最新一份（旧链连同凭据子目录彻底删除，不走回收站）；
//! 3. 报告 E 盘备份目录水位变化（3.2GB → 单份 ≈1.7GB）。
//!
//! 安全门：WAL 活动检测（与切号 Q1.2 同口径）——主库正被客户端写入时
//! fail-closed 拒绝，零副作用退出。
//!
//! 用法：`rolling_backup_master_p17 [backup_dir]`
//!   默认 backup_dir = `E:\trae-sync\Trae Sync\backups`（安装版活动备份目录）

use std::path::PathBuf;
use std::process::ExitCode;

use traesync_infrastructure::master_handover::{
    backup_master_trio_rolling, list_master_backups_in, master_database_path,
};
use traesync_infrastructure::environment_registry::master_data_dir;

fn default_backup_dir() -> PathBuf {
    PathBuf::from(r"E:\trae-sync\Trae Sync\backups")
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    let target = PathBuf::from(args.get(1).cloned().unwrap_or_default()).abs_or(default_backup_dir());

    let master_dir = match master_data_dir() {
        Ok(dir) => dir,
        Err(_) => {
            eprintln!("中止：主库数据目录解析失败（环境注册表不可用）");
            return ExitCode::FAILURE;
        }
    };
    let db_path = master_database_path(&master_dir);
    println!("主库：{}", db_path.display());
    println!("备份目标：{}", target.display());
    if !db_path.is_file() {
        eprintln!("中止：主库 db 不存在（TRAE 从未启动，无需备份）");
        return ExitCode::FAILURE;
    }

    // 滚动前现场：链条目与目录水位。
    let db_name = db_path.file_name().unwrap().to_string_lossy().into_owned();
    let before = list_master_backups_in(&target, &db_name);
    println!("---- 滚动前 ----");
    for entry in &before {
        println!(
            "  stamp={} bytes={} has_wal={} has_credentials={}",
            entry.stamp_unix_seconds, entry.total_bytes, entry.has_wal, entry.has_credentials
        );
    }
    println!("  目录水位：{}", format_bytes(dir_size(&target)));

    // Q1.2 生成中检测：WAL 活跃 = 客户端正在写库，fail-closed 拒绝。
    if traesync_infrastructure::master_handover::master_db_activity_detected(
        &db_path,
        std::time::Duration::from_millis(800),
    ) {
        eprintln!("中止：主库 WAL 活动中（客户端可能正在写库）——请关闭 TRAE 后重试");
        return ExitCode::FAILURE;
    }

    let started = std::time::Instant::now();
    let backup = match backup_master_trio_rolling(&db_path, &target) {
        Ok(path) => path,
        Err(error) => {
            eprintln!("中止：滚动备份失败（{error:?}）");
            return ExitCode::FAILURE;
        }
    };
    println!(
        "滚动备份完成（{:.1}s）：{}",
        started.elapsed().as_secs_f64(),
        backup.display()
    );

    // 滚动后现场：应只剩一条链目。
    let after = list_master_backups_in(&target, &db_name);
    println!("---- 滚动后 ----");
    for entry in &after {
        println!(
            "  stamp={} bytes={} has_wal={} has_credentials={}",
            entry.stamp_unix_seconds, entry.total_bytes, entry.has_wal, entry.has_credentials
        );
    }
    println!("  链条目数：{}（应为 1 = 滚动单份）", after.len());
    println!("  目录水位：{}", format_bytes(dir_size(&target)));
    if after.len() != 1 {
        eprintln!("验收失败：滚动后链内仍有 {} 条目", after.len());
        return ExitCode::FAILURE;
    }
    println!("验收通过：同一时刻备份目录内只有一份备份。");
    ExitCode::SUCCESS
}

/// 目录磁盘字节合计（递归；不可读项按 0 计）。
fn dir_size(dir: &std::path::Path) -> u64 {
    let mut total = 0;
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    for entry in entries.flatten() {
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        if metadata.is_dir() {
            total += dir_size(&entry.path());
        } else if metadata.is_file() {
            total += metadata.len();
        }
    }
    total
}

fn format_bytes(bytes: u64) -> String {
    if bytes >= 1024 * 1024 * 1024 {
        format!("{:.2} GB", bytes as f64 / (1024.0 * 1024.0 * 1024.0))
    } else if bytes >= 1024 * 1024 {
        format!("{:.2} MB", bytes as f64 / (1024.0 * 1024.0))
    } else {
        format!("{:.1} KB", bytes as f64 / 1024.0)
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
