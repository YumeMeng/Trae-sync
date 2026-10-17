//! P17-T4b 真机备份执行器（P17 立项授权：需要 WorkBuddy 退出操作的任务）。
//!
//! 执行序列：
//! 1. 探活 WorkBuddy 本地探针 API（运行中则打印状态）；
//! 2. `close_workbuddy()` 优雅关闭（用户已在 P17 立项时授权）；关闭失败
//!    中止（fail-closed，零副作用）；
//! 3. `backup_workbuddy_data_to` 备份数据目录（暂存压缩 → manifest →
//!    发布；任一成员失败整体失败，零副作用）；
//! 4. 回读校验：恢复到隔离校验目录，逐成员 sha256 比对 manifest + 与
//!    当前源文件比对（证明备份捕获了源现状），校验后清理校验目录；
//! 5. 打印体积与压缩比报告。完成后 WorkBuddy 可由用户重开。
//!
//! 用法：`workbuddy_backup_p17 [source_dir] [target_dir]`
//!   默认 source = `%USERPROFILE%\.workbuddy\app`
//!   默认 target = `E:\trae-sync\Trae Sync\backups\workbuddy`（软件本体旁
//!   backups/ 的 workbuddy 子目录，与 P16 备份治理位置同口径）。

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Instant;

use traesync_infrastructure::workbuddy_backup::{
    backup_workbuddy_data_to, list_workbuddy_backups, restore_workbuddy_data_from,
};
use traesync_infrastructure::workbuddy_instance::{close_workbuddy, probe_local_api, CloseOutcome};

fn default_source() -> PathBuf {
    let profile = std::env::var("USERPROFILE").unwrap_or_default();
    Path::new(&profile).join(".workbuddy").join("app")
}

fn default_target() -> PathBuf {
    PathBuf::from(r"E:\trae-sync\Trae Sync\backups\workbuddy")
}

/// 回读校验用的隔离目录（.scratch 下；校验后由本程序自身清理）。
fn verify_dir() -> PathBuf {
    PathBuf::from(r"D:\work\Trae-sync\.scratch\p17-backup-verify")
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    let source = PathBuf::from(args.get(1).cloned().unwrap_or_default()).abs_or(default_source());
    let target = PathBuf::from(args.get(2).cloned().unwrap_or_default()).abs_or(default_target());

    println!("源数据根：{}", source.display());
    println!("备份目标：{}", target.display());

    // 第 1 步：探活（状态可见性；不作为门禁——关闭编排自行判定进程态）。
    let http = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(2))
        .build()
        .ok();
    if let Some(http) = &http {
        match probe_local_api(http) {
            Ok(info) => println!(
                "探针：WorkBuddy 运行中（v{} @ {}）",
                info.version, info.app
            ),
            Err(error) => println!("探针：不可达（{}）——可能未运行", error.code()),
        }
    }

    // 第 2 步：关闭（用户已在 P17 立项时授权 WorkBuddy 退出操作）。
    let started = Instant::now();
    match close_workbuddy() {
        Ok(outcome) => match outcome {
            CloseOutcome::NotRunning => println!("关闭编排：WorkBuddy 未在运行，直接继续"),
            CloseOutcome::Closed => println!("关闭编排：已优雅关闭"),
            CloseOutcome::ForceClosed => println!("关闭编排：优雅窗口后仍有存活进程，已强制结束"),
        },
        Err(error) => {
            eprintln!("中止：关闭编排失败（{}）——零副作用退出", error.code());
            return ExitCode::FAILURE;
        }
    }
    println!("关闭耗时 {:.1}s", started.elapsed().as_secs_f64());

    // 第 3 步：备份（fail-closed：任一成员失败 → 暂存清理 + 零副作用）。
    let started = Instant::now();
    let outcome = match backup_workbuddy_data_to(&source, &target) {
        Ok(outcome) => outcome,
        Err(error) => {
            eprintln!("中止：备份失败（{}）——零副作用退出", error.code());
            return ExitCode::FAILURE;
        }
    };
    println!("备份耗时 {:.1}s", started.elapsed().as_secs_f64());
    println!("备份目录：{}", outcome.backup_dir.display());

    // 第 4 步：回读校验——恢复到隔离目录并逐成员与当前源比对。
    let backup_dir = outcome.backup_dir.clone();
    let verify = verify_dir();
    let _ = std::fs::remove_dir_all(&verify);
    let started = Instant::now();
    if let Err(error) = restore_workbuddy_data_from(&backup_dir, &verify) {
        eprintln!("中止：回读恢复失败（{:?}）", error);
        return ExitCode::FAILURE;
    }
    println!(
        "恢复校验：{} 文件 / {} 字节解压重写成功（{:.1}s）",
        outcome.file_count,
        outcome.source_bytes,
        started.elapsed().as_secs_f64()
    );
    // 与当前源逐成员 sha256 比对（备份后源未再变动的前提下应全一致）。
    let mut mismatch = 0_usize;
    // 链条目报告（list_workbuddy_backups：manifest 声明值 + 磁盘字节）。
    for entry in list_workbuddy_backups(&target) {
        println!(
            "链条目 stamp={}：file_count={:?} source_bytes={:?} compressed_disk={}",
            entry.stamp_unix_seconds, entry.file_count, entry.source_bytes, entry.compressed_bytes
        );
    }
    for relative in walk_member_paths(&backup_dir) {
        let restored = verify.join(&relative);
        let origin = source.join(&relative);
        match (
            traesync_infrastructure::snapshot_store::sha256_file(&restored),
            traesync_infrastructure::snapshot_store::sha256_file(&origin),
        ) {
            (Some(a), Some(b)) if a == b => {}
            _ => {
                mismatch += 1;
                eprintln!("  比对不一致/缺源：{relative}");
            }
        }
    }
    // 清理校验目录（程序内自身临时产物）。
    let _ = std::fs::remove_dir_all(&verify);
    if mismatch > 0 {
        eprintln!("中止：{mismatch} 个成员与当前源不一致（源可能在备份后被改动）");
        return ExitCode::FAILURE;
    }
    println!("源比对：{} 文件全部一致", outcome.file_count);

    // 第 5 步：报告。
    let ratio = if outcome.source_bytes > 0 {
        outcome.compressed_bytes as f64 / outcome.source_bytes as f64
    } else {
        0.0
    };
    println!("---- P17-T4b 备份报告 ----");
    println!("  备份成员：{} 文件", outcome.file_count);
    println!("  源字节：{}", format_bytes(outcome.source_bytes));
    println!(
        "  压缩后：{}（压缩比 {ratio:.3}，节省 {:.1}%）",
        format_bytes(outcome.compressed_bytes),
        (1.0 - ratio) * 100.0
    );
    println!("WorkBuddy 现在可以重新打开。");
    ExitCode::SUCCESS
}

/// 收集备份目录内全部成员相对路径（含 manifest 自身——排除）。
fn walk_member_paths(backup_dir: &Path) -> Vec<String> {
    fn walk(dir: &Path, relative: &str, plan: &mut Vec<String>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let child = if relative.is_empty() {
                name.clone()
            } else {
                format!("{relative}/{name}")
            };
            if entry.path().is_dir() {
                walk(&entry.path(), &child, plan);
            } else if name != "manifest.json" {
                // 成员以 `.zst` 存储，比对对象是去后缀后的相对路径。
                if let Some(stripped) = child.strip_suffix(".zst") {
                    plan.push(stripped.to_string());
                }
            }
        }
    }
    let mut plan = Vec::new();
    walk(backup_dir, "", &mut plan);
    plan.sort();
    plan
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
