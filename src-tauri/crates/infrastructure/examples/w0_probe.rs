//! W0 探针：TRAE 运行中「三件套快照副本 + 只读解密」读取一致性验证。
//!
//! 用法：`w0_probe <db_path> [rounds] [interval_secs]`（默认 30 轮 × 2 秒）。
//! raw key 从环境变量 `W0_RAW_KEY` 注入（运行时由命令提供，不硬编码进源码）。
//!
//! 铁律：只读原库——三件套快照复制到系统临时目录，由 Drop 自动清理；
//! 撕裂样本（复制/解密/查询失败）只记录错误并继续下一轮，不中断探针。
//! stdout 逐行输出每轮 JSON，结尾附一行汇总 JSON。

use std::path::PathBuf;
use std::process::ExitCode;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use traesync_infrastructure::sqlcipher::open_with_key_readonly_staged;

/// 单文件观测快照：mtime（变化检测用）+ size（JSON 输出用）；缺失记 None。
#[derive(Clone)]
struct FileStat {
    mtime: Option<SystemTime>,
    size: Option<u64>,
}

/// 读取单文件 mtime+size；文件不存在/不可读统一记 None。
fn stat_file(path: &PathBuf) -> FileStat {
    match std::fs::metadata(path) {
        Ok(meta) => FileStat {
            mtime: meta.modified().ok(),
            size: Some(meta.len()),
        },
        Err(_) => FileStat {
            mtime: None,
            size: None,
        },
    }
}

/// 变化判定：size 或 mtime 任一不同即视为变化（含文件出现/消失）。
fn stat_changed(a: &FileStat, b: &FileStat) -> bool {
    a.size != b.size || a.mtime != b.mtime
}

/// 就近秩分位：入参须为升序样本，取 ceil(p*n)-1 下标；空样本返回 None。
fn percentile(sorted: &[f64], p: f64) -> Option<f64> {
    if sorted.is_empty() {
        return None;
    }
    let idx = ((p * sorted.len() as f64).ceil() as usize)
        .saturating_sub(1)
        .min(sorted.len() - 1);
    sorted.get(idx).copied()
}

fn main() -> ExitCode {
    // 参数解析：<db_path> [rounds] [interval_secs]
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        eprintln!("用法: w0_probe <db_path> [rounds] [interval_secs]");
        return ExitCode::FAILURE;
    }
    let db_path = PathBuf::from(&args[1]);
    // 轮数与间隔：默认 30 轮 × 2 秒
    let rounds: usize = args.get(2).and_then(|v| v.parse().ok()).unwrap_or(30);
    let interval_secs: f64 = args.get(3).and_then(|v| v.parse().ok()).unwrap_or(2.0);
    // raw key 只从环境变量读取；源码不落值，避免密钥进入仓库
    let raw_key = match std::env::var("W0_RAW_KEY") {
        Ok(key) => key,
        Err(_) => {
            eprintln!("缺少环境变量 W0_RAW_KEY");
            return ExitCode::FAILURE;
        }
    };
    if !db_path.exists() {
        eprintln!("数据库不存在: {}", db_path.display());
        return ExitCode::FAILURE;
    }

    let wal_path = PathBuf::from(format!("{}-wal", db_path.display()));
    let shm_path = PathBuf::from(format!("{}-shm", db_path.display()));
    // 启动配置走 stderr，保证 stdout 纯 JSONL
    eprintln!(
        "W0 探针启动: db={} rounds={} interval={}s",
        db_path.display(),
        rounds,
        interval_secs
    );

    let interval = Duration::from_secs_f64(interval_secs);
    let mut prev_stats: Option<[FileStat; 3]> = None;
    let mut torn_count = 0usize;
    let mut changed_count = 0usize;
    let mut copy_ms_samples: Vec<f64> = Vec::new();
    let mut read_ms_samples: Vec<f64> = Vec::new();

    for round in 1..=rounds {
        let round_started = Instant::now();
        let ts_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;

        // 1. 记录 db/wal/shm 三件套 mtime+size（变化检测灵敏度数据）
        let stats = [
            stat_file(&db_path),
            stat_file(&wal_path),
            stat_file(&shm_path),
        ];
        let changed = match &prev_stats {
            Some(prev) => stats
                .iter()
                .zip(prev.iter())
                .any(|(cur, old)| stat_changed(cur, old)),
            // 首轮无基线可比，记 false
            None => false,
        };
        prev_stats = Some(stats.clone());
        if changed {
            changed_count += 1;
        }

        // 2+3. 快照复制（计时）→ 只读打开 → 查询（计时）；撕裂样本只记录不中断
        let mut copy_ms = 0.0f64;
        let mut read_ms = 0.0f64;
        let mut sessions: Option<i64> = None;
        let mut max_updated_at: Option<i64> = None;
        let mut torn = false;
        let mut err: Value = Value::Null;

        match open_with_key_readonly_staged(&db_path, &raw_key) {
            Ok((conn, copy_us, open_us)) => {
                copy_ms = copy_us as f64 / 1000.0;
                copy_ms_samples.push(copy_ms);
                // 查询：chat_session 的 COUNT(*) 与 MAX(updated_at)。
                // 解密失败（key 不匹配/副本撕裂）会在此暴露为查询错误 = 撕裂样本。
                let query_started = Instant::now();
                let query_result = conn.query_row(
                    "SELECT COUNT(*), MAX(updated_at) FROM chat_session",
                    [],
                    |row| {
                        let count: i64 = row.get(0)?;
                        let max_ts: Option<i64> = row.get(1)?;
                        Ok((count, max_ts))
                    },
                );
                let query_us = query_started.elapsed().as_micros() as f64;
                // read_ms = 只读打开/设 key + 查询（解密首读在此发生）
                read_ms = (open_us as f64 + query_us) / 1000.0;
                read_ms_samples.push(read_ms);
                match query_result {
                    Ok((count, max_ts)) => {
                        sessions = Some(count);
                        max_updated_at = max_ts;
                    }
                    Err(e) => {
                        torn = true;
                        err = json!(e.to_string());
                    }
                }
            }
            Err(e) => {
                // 复制或打开阶段失败同样计为撕裂样本（copy_ms 无样本，记 0）
                torn = true;
                err = json!(e.to_string());
            }
        }
        if torn {
            torn_count += 1;
        }

        // 4. 每轮一行 JSON
        println!(
            "{}",
            json!({
                "round": round,
                "ts": ts_ms,
                "db_size": stats[0].size,
                "wal_size": stats[1].size,
                "shm_size": stats[2].size,
                "changed": changed,
                "copy_ms": copy_ms,
                "read_ms": read_ms,
                "sessions": sessions,
                "max_updated_at": max_updated_at,
                "torn": torn,
                "err": err,
            })
        );

        // 轮间 pacing：以轮起点对齐；复制耗时超过间隔则背靠背继续
        let elapsed = round_started.elapsed();
        if elapsed < interval {
            std::thread::sleep(interval - elapsed);
        }
    }

    // 汇总：总轮数 / 撕裂次数 / 复制耗时分位 / 读取耗时分位 / 变化轮占比
    let mut copy_sorted = copy_ms_samples.clone();
    copy_sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let mut read_sorted = read_ms_samples.clone();
    read_sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    println!(
        "{}",
        json!({
            "summary": true,
            "rounds": rounds,
            "torn": torn_count,
            "changed_ratio": changed_count as f64 / rounds as f64,
            "copy_ms_p50": percentile(&copy_sorted, 0.50),
            "copy_ms_p95": percentile(&copy_sorted, 0.95),
            "copy_ms_max": copy_sorted.last().copied(),
            "read_ms_p50": percentile(&read_sorted, 0.50),
            "read_ms_p95": percentile(&read_sorted, 0.95),
            "read_ms_max": read_sorted.last().copied(),
        })
    );

    ExitCode::SUCCESS
}
