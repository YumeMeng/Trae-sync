//! 融合版/Work 库 schema 对比探针（只读）。
//!
//! 背景（2026-10-13 融合版迁移研究）：工具的库操作（读取/归档/归并/切号
//! 交接）建立在 Work 库表结构上，需要只读核对融合版库 schema 是否同构，
//! 判断这些能力能否直接复用。
//!
//! 用法: unified_schema_probe <data_dir>...
//! 输出: 每个目录的表名 -> 列名清单（JSON），无数据正文。

use std::path::PathBuf;

use rusqlite::Connection;
use serde_json::json;
use traesync_infrastructure::master_handover::master_database_path;

/// 项目已验证的 SQLCipher raw key（TECHNICAL_BASELINE 固化）。
const RAW_KEY: &str = "3605f6691095a993f03d5009c918352ef5be31ae31e8f000212b81ff058da773";

fn main() -> std::process::ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        eprintln!("用法: unified_schema_probe <data_dir>...");
        return std::process::ExitCode::FAILURE;
    }
    let mut results = Vec::new();
    for dir in &args {
        let db_path = master_database_path(&PathBuf::from(dir));
        if !db_path.is_file() {
            results.push(json!({ "dir": dir, "status": "no_database" }));
            continue;
        }
        // 只读打开（与 read_master_checkup 同链路：raw key + readonly）。
        let conn = match Connection::open_with_flags(
            &db_path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        ) {
            Ok(conn) => conn,
            Err(_) => {
                results.push(json!({ "dir": dir, "status": "open_failed" }));
                continue;
            }
        };
        if conn
            .pragma_update(None, "key", format!("x'{RAW_KEY}'"))
            .is_err()
        {
            results.push(json!({ "dir": dir, "status": "key_failed" }));
            continue;
        }
        // 表清单。
        let Ok(mut stmt) = conn.prepare(
            "SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%' ORDER BY name",
        ) else {
            results.push(json!({ "dir": dir, "status": "query_failed" }));
            continue;
        };
        let tables: Vec<String> = stmt
            .query_map([], |row| row.get::<_, String>(0))
            .map(|rows| rows.filter_map(Result::ok).collect())
            .unwrap_or_default();
        // 每表列清单（PRAGMA table_info）。
        let mut schema = serde_json::Map::new();
        for table in &tables {
            if let Ok(mut col_stmt) =
                conn.prepare(&format!("PRAGMA table_info({table})"))
            {
                let cols: Vec<String> = col_stmt
                    .query_map([], |row| row.get::<_, String>(1))
                    .map(|rows| rows.filter_map(Result::ok).collect())
                    .unwrap_or_default();
                schema.insert(table.clone(), json!(cols));
            }
        }
        results.push(json!({ "dir": dir, "status": "ready", "tables": schema }));
    }
    println!("{}", json!({ "tool": "unified-schema-probe", "results": results }));
    std::process::ExitCode::SUCCESS
}
