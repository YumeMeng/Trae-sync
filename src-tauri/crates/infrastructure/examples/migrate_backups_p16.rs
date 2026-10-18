//! P16-T1 真机迁移执行器（Q12 用户授权：旧未压缩备份 → 软件本体旁压缩档）。
//!
//! 执行前置（由操作者在外部确认，本程序再做双重防御）：
//! 1. Trae CN 客户端已完全退出（tasklist 无 Trae CN 进程；客户端运行中
//!    不得执行——主理人纪律「正在运行的实例不干扰」）；
//! 2. 本程序对每个待迁源成员做独占打开探针，任何句柄占用即中止
//!    （fail-closed，不做部分迁移）。
//!
//! 迁移语义见 `backup_retention::migrate_legacy_backups_to_compressed`：
//! 暂存压缩 → 流式解压比对 → 改名发布 → 删除源副本；幂等可重跑。
//!
//! 用法：
//!   `migrate_backups_p16 [source_dir] [target_dir]`
//!   默认 source = `%APPDATA%\Trae CN\ModularData\ai-agent`
//!   默认 target = `<安装目录>\backups`（显式传入以匹配安装版推导：
//!   `E:\trae-sync\Trae Sync\backups`）。

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use traesync_infrastructure::backup_retention::migrate_legacy_backups_to_compressed;
use traesync_infrastructure::master_handover::{backup_member_file_names, list_master_backups_in};

const DB_NAME: &str = "database.db";

fn default_source() -> PathBuf {
    let appdata = std::env::var("APPDATA").unwrap_or_default();
    Path::new(&appdata)
        .join("Trae CN")
        .join("ModularData")
        .join("ai-agent")
}

fn default_target() -> PathBuf {
    // 与安装版 resolve_master_backup_dir 的 P16-T1 默认推导同口径：
    // 软件本体（安装目录内 exe）同级 backups/。
    let exe = std::env::current_exe().unwrap_or_default();
    exe.parent()
        .map(|dir| dir.join("backups"))
        .unwrap_or_default()
}

/// 独占打开探针：目标文件被任何进程持有句柄时打开失败（share_mode 0）。
fn file_is_locked(path: &Path) -> bool {
    // share_mode 需 Windows 扩展 trait 在作用域内（补缺失导入）。
    use std::os::windows::fs::OpenOptionsExt;
    std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .share_mode(0)
        .open(path)
        .is_err()
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    let source = PathBuf::from(args.get(1).cloned().unwrap_or_default()).abs_or(default_source());
    let target = PathBuf::from(args.get(2).cloned().unwrap_or_default()).abs_or(default_target());

    println!("源目录：{}", source.display());
    println!("目标目录：{}", target.display());

    // 防御一：枚举旧未压缩成员，确认存在且无句柄占用（fail-closed）。
    let chain = list_master_backups_in(&source, DB_NAME);
    if chain.is_empty() {
        println!("源目录无 {DB_NAME}.switch-bak-* 备份链，无需迁移。");
        return ExitCode::SUCCESS;
    }
    let mut locked = Vec::new();
    let mut total_bytes = 0_u64;
    for entry in &chain {
        for name in backup_member_file_names(DB_NAME, entry.stamp_unix_seconds) {
            let path = source.join(&name);
            if !name.ends_with(".zst") && path.is_file() {
                // 只探针旧未压缩成员（迁移对象）；.zst 本就是目标形态。
                let size = std::fs::metadata(&path).map(|meta| meta.len()).unwrap_or(0);
                total_bytes += size;
                if file_is_locked(&path) {
                    locked.push(name);
                }
            }
        }
    }
    if !locked.is_empty() {
        eprintln!("中止：以下源文件被其他进程占用（客户端可能仍在运行）：{locked:?}");
        return ExitCode::FAILURE;
    }
    println!(
        "待迁移：{} 份备份（旧未压缩字节合计 {}）",
        chain.len(),
        format_bytes(total_bytes)
    );

    // 防御二之外无其他副作用；迁移函数自身保证校验全过才删源。
    match migrate_legacy_backups_to_compressed(&source, &target, DB_NAME) {
        Ok(outcome) => {
            println!("迁移完成：");
            println!("  校验通过并删除原副本：{} 份", outcome.migrated_backups);
            println!("  幂等跳过（目标已一致）：{} 份", outcome.idempotent_backups);
            println!(
                "  释放空间（源目录）：{}",
                format_bytes(outcome.freed_bytes)
            );
            if !outcome.failures.is_empty() {
                println!("  失败（原副本保留）：");
                for failure in &outcome.failures {
                    println!("    - {failure}");
                }
                return ExitCode::FAILURE;
            }
            println!("目标目录压缩档可经工具恢复链/标准 zstd 解压读取。");
            ExitCode::SUCCESS
        }
        Err(_) => {
            eprintln!("迁移失败：IO/目录错误（原副本未删除）。");
            ExitCode::FAILURE
        }
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

fn format_bytes(bytes: u64) -> String {
    if bytes >= 1024 * 1024 * 1024 {
        format!("{:.2} GB", bytes as f64 / (1024.0 * 1024.0 * 1024.0))
    } else {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    }
}
