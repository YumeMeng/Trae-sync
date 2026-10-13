//! 真机切号验收驱动（P15-5/P15-8，ADR-0035；需 `--features probe`）。
//!
//! 隔离演练模式（推荐，主客户端零风险）：`--isolated-sweep <dir>` 把五步
//! 事务全部指向独立数据目录——播种（复制主库 storage.json 与数据库三件套
//! 到隔离目录）→ 快照（环境注册表/切号日志）→ 顺序切完全部注册账号（每轮
//! 关/启的都是隔离实例）→ 恢复快照 → 关闭并删除隔离目录（--keep 保留）。
//! 主客户端目录零写入、云端插件零操作（对账 no-op）。
//!
//! 用法：
//!   cargo run --example fused_switch --features probe -- --list
//!   cargo run --example fused_switch --features probe -- --isolated-sweep <dir> [--end <profile_id>] [--force] [--settle-secs 12] [--keep]
//!   cargo run --example fused_switch --features probe -- --one-at <dir> <profile_id> [--force] [--keep]
//!   cargo run --example fused_switch --features probe -- --cleanup <dir>
//!   cargo run --example fused_switch --features probe -- --close-at <dir>

use std::path::{Path, PathBuf};

#[cfg(feature = "probe")]
use trae_sync_lib::credential_handover::real_switch_probe;

#[cfg(feature = "probe")]
fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mode = args.first().map(String::as_str).unwrap_or("");
    let mut force = false;
    let mut keep = false;
    let mut settle_secs: u64 = 12;
    let mut end: Option<String> = None;
    let mut positional: Vec<String> = Vec::new();
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--force" => force = true,
            "--keep" => keep = true,
            "--end" => {
                i += 1;
                end = args.get(i).cloned();
            }
            "--settle-secs" => {
                i += 1;
                settle_secs = args.get(i).and_then(|v| v.parse().ok()).unwrap_or(12);
            }
            other => positional.push(other.to_string()),
        }
        i += 1;
    }

    let exit_code = match mode {
        "--list" => match real_switch_probe::list_accounts() {
            Ok(info) => {
                println!("{}", serde_json::to_string_pretty(&info).unwrap_or_default());
                0
            }
            Err(error) => {
                eprintln!("list_accounts 失败：{error}");
                1
            }
        },
        "--isolated-sweep" => {
            let Some(dir) = positional.first() else {
                eprintln!("--isolated-sweep 需要 <dir>");
                return eprint_usage_and_exit();
            };
            run_isolated_sweep(Path::new(dir), end, force, settle_secs, keep)
        }
        "--one-at" => {
            let [dir, profile_id] = positional.as_slice() else {
                eprintln!("--one-at 需要 <dir> <profile_id>");
                return eprint_usage_and_exit();
            };
            run_one_at(Path::new(dir), profile_id, force, keep)
        }
        "--cleanup" => {
            let Some(dir) = positional.first() else {
                eprintln!("--cleanup 需要 <dir>");
                return eprint_usage_and_exit();
            };
            cleanup_isolated(Path::new(dir))
        }
        "--close-at" => {
            let Some(dir) = positional.first() else {
                eprintln!("--close-at 需要 <dir>");
                return eprint_usage_and_exit();
            };
            match real_switch_probe::close_instance_at(Path::new(dir)) {
                Ok(outcome) => {
                    println!("关闭隔离实例：{outcome}");
                    0
                }
                Err(error) => {
                    eprintln!("关闭失败：{error}");
                    1
                }
            }
        }
        _ => eprint_usage_and_exit(),
    };
    std::process::exit(exit_code);
}

/// 播种隔离目录：复制真实主库的 storage.json 与数据库三件套（只读复制，
/// 不加锁不改写主库任何字节）。
#[cfg(feature = "probe")]
fn seed_isolated(dir: &Path) -> Result<Vec<PathBuf>, String> {
    use trae_sync_lib::credential_handover::real_switch_probe;
    real_switch_probe::seed_isolated_from_master(dir)
}

/// 隔离扫批：播种 → 快照 → 顺序切号 → 恢复 → 清理。
#[cfg(feature = "probe")]
fn run_isolated_sweep(dir: &Path, end: Option<String>, force: bool, settle_secs: u64, keep: bool) -> i32 {
    use std::time::Duration;

    let info = match real_switch_probe::list_accounts() {
        Ok(info) => info,
        Err(error) => {
            eprintln!("list_accounts 失败：{error}");
            return 1;
        }
    };
    let accounts = info["accounts"].as_array().cloned().unwrap_or_default();
    let profiles: Vec<String> = accounts
        .iter()
        .filter_map(|a| a["profile_id"].as_str().map(str::to_string))
        .collect();
    if profiles.is_empty() {
        eprintln!("注册表为空，无可切账号");
        return 1;
    }
    // 最后停留账号：显式 --end > 主库当前登录身份对应的注册账号 > 注册表最后一个。
    let end_profile = end
        .or_else(|| real_switch_probe::master_blob_profile_id())
        .filter(|current| profiles.contains(current))
        .unwrap_or_else(|| profiles.last().cloned().unwrap());

    // 1. 播种（storage.json + 数据库三件套，全部复制进隔离目录）。
    let seeded = match seed_isolated(dir) {
        Ok(files) => files,
        Err(error) => {
            eprintln!("播种失败：{error}");
            return 1;
        }
    };
    println!(
        "隔离目录已播种：{} 个文件（{}）",
        seeded.len(),
        dir.display()
    );

    // 2. 快照会被编排写到的真实存储根文件（环境注册表 / 切号日志），
    //    扫批结束后字节级还原——编排对真实状态的唯一可见副作用清零。
    let storage_root = real_switch_probe::storage_root();
    let snapshot_targets = [
        storage_root.join("environments.json"),
        storage_root.join("master-switch-journal.jsonl"),
    ];
    let mut snapshots: Vec<(PathBuf, Option<Vec<u8>>)> = Vec::new();
    for path in &snapshot_targets {
        let bytes = std::fs::read(path).ok();
        snapshots.push((path.clone(), bytes));
    }

    // 3. 顺序切号（隔离目录上的完整五步事务）。
    let mut order: Vec<String> = profiles
        .iter()
        .filter(|profile| *profile != &end_profile)
        .cloned()
        .collect();
    order.push(end_profile.clone());
    println!(
        "扫批开始：{} 个账号，最后停留 {end_profile}，settle {settle_secs}s，隔离目录 {}",
        order.len(),
        dir.display()
    );
    let mut succeeded = 0usize;
    let mut failures: Vec<(String, String)> = Vec::new();
    for (index, profile) in order.iter().enumerate() {
        let name = accounts
            .iter()
            .find(|a| a["profile_id"].as_str() == Some(profile.as_str()))
            .and_then(|a| a["display_name"].as_str())
            .unwrap_or("?");
        println!("[{}/{}] 切号 → {name}（{profile}）", index + 1, order.len());
        match real_switch_probe::switch_one_at(dir, profile, force) {
            Ok(report) => {
                let blob = report["blob_user_id_after"]
                    .as_str()
                    .unwrap_or("(blob 读取失败)");
                let attempts = report["attempts"].as_u64().unwrap_or(1);
                let transferred = report["switch"]["transferred_projects"]
                    .as_u64()
                    .unwrap_or(0);
                println!(
                    "  ✓ 完成（{attempts} 轮）；隔离库 blob 身份 = {blob}；交接项目数 = {transferred}"
                );
                succeeded += 1;
            }
            Err(error) => {
                println!("  ✗ 失败：{error}");
                failures.push((profile.clone(), error));
            }
        }
        if index + 1 < order.len() {
            std::thread::sleep(Duration::from_secs(settle_secs));
        }
    }

    // 4. 关闭隔离实例 + 字节级还原快照。
    match real_switch_probe::close_instance_at(dir) {
        Ok(outcome) => println!("隔离实例已关闭：{outcome}"),
        Err(error) => eprintln!("隔离实例关闭失败（不阻断收尾）：{error}"),
    }
    for (path, bytes) in &snapshots {
        match bytes {
            Some(content) => {
                if std::fs::write(path, content).is_ok() {
                    println!("已还原：{}", path.display());
                } else {
                    eprintln!("还原失败：{}", path.display());
                }
            }
            // 扫批前不存在的文件：删除编排新写入的版本。
            None => {
                if path.exists() {
                    let _ = std::fs::remove_file(path);
                    println!("已移除扫批新增文件：{}", path.display());
                }
            }
        }
    }

    // 5. 清理隔离目录（--keep 保留供检查）。
    if keep {
        println!("--keep：隔离目录保留在 {}", dir.display());
    } else {
        match std::fs::remove_dir_all(dir) {
            Ok(_) => println!("隔离目录已删除：{}", dir.display()),
            Err(error) => {
                eprintln!("隔离目录删除失败（可手动删除）：{error}");
                return if failures.is_empty() { 1 } else { 1 };
            }
        }
    }

    println!("扫批结束：{succeeded}/{} 成功", order.len());
    if !failures.is_empty() {
        println!("失败清单：{failures:?}");
    }
    if failures.is_empty() {
        0
    } else {
        1
    }
}

#[cfg(feature = "probe")]
fn run_one_at(dir: &Path, profile_id: &str, force: bool, keep: bool) -> i32 {
    // 单账号隔离演练闭环：播种（目录缺失时）→ 快照真实根文件 → 切号 →
    // 关隔离实例 → 字节级还原 → 清理隔离目录（--keep 保留供检查）。
    // 主客户端目录零写入、云端插件零操作（对账 no-op）。
    if !dir.exists() {
        match seed_isolated(dir) {
            Ok(files) => println!("隔离目录已播种：{} 个文件（{}）", files.len(), dir.display()),
            Err(error) => {
                eprintln!("播种失败：{error}");
                return 1;
            }
        }
    }
    // 编排会把环境注册表/切号日志写进真实存储根，演练后字节级还原。
    let storage_root = real_switch_probe::storage_root();
    let snapshot_targets = [
        storage_root.join("environments.json"),
        storage_root.join("master-switch-journal.jsonl"),
    ];
    let mut snapshots: Vec<(PathBuf, Option<Vec<u8>>)> = Vec::new();
    for path in &snapshot_targets {
        snapshots.push((path.clone(), std::fs::read(path).ok()));
    }

    println!("隔离切号 → {profile_id}（force={force}，目录 {}）", dir.display());
    let result = real_switch_probe::switch_one_at(dir, profile_id, force);

    // 收尾：无论成败都关隔离实例 + 还原快照（真实状态可见副作用清零）。
    match real_switch_probe::close_instance_at(dir) {
        Ok(outcome) => println!("隔离实例已关闭：{outcome}"),
        Err(error) => eprintln!("隔离实例关闭失败（继续收尾）：{error}"),
    }
    for (path, bytes) in &snapshots {
        match bytes {
            Some(content) => {
                if std::fs::write(path, content).is_ok() {
                    println!("已还原：{}", path.display());
                } else {
                    eprintln!("还原失败：{}", path.display());
                }
            }
            None => {
                if path.exists() {
                    let _ = std::fs::remove_file(path);
                    println!("已移除演练新增文件：{}", path.display());
                }
            }
        }
    }
    if keep {
        println!("--keep：隔离目录保留在 {}", dir.display());
    } else if dir.exists() {
        match std::fs::remove_dir_all(dir) {
            Ok(_) => println!("隔离目录已删除：{}", dir.display()),
            Err(error) => eprintln!("隔离目录删除失败（可手动删除）：{error}"),
        }
    }

    match result {
        Ok(report) => {
            println!("✓ 切号完成：{}", serde_json::to_string_pretty(&report).unwrap_or_default());
            0
        }
        Err(error) => {
            println!("✗ 切号失败：{error}");
            1
        }
    }
}

#[cfg(feature = "probe")]
fn cleanup_isolated(dir: &Path) -> i32 {
    match real_switch_probe::close_instance_at(dir) {
        Ok(outcome) => println!("隔离实例关闭：{outcome}"),
        Err(error) => eprintln!("关闭失败（继续删除）：{error}"),
    }
    match std::fs::remove_dir_all(dir) {
        Ok(_) => {
            println!("隔离目录已删除：{}", dir.display());
            0
        }
        Err(error) => {
            eprintln!("删除失败：{error}");
            1
        }
    }
}

#[cfg(feature = "probe")]
fn eprint_usage_and_exit() -> ! {
    eprintln!(
        "用法：fused_switch --list | --isolated-sweep <dir> [--end <profile_id>] [--force] [--settle-secs 12] [--keep] | --one-at <dir> <profile_id> [--force] [--keep] | --cleanup <dir> | --close-at <dir>"
    );
    std::process::exit(2);
}

#[cfg(not(feature = "probe"))]
fn main() {
    eprintln!("本探针需以 --features probe 构建：cargo run --example fused_switch --features probe");
    std::process::exit(2);
}
