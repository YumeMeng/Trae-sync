//! P15-8 主库真实写操作验收驱动（归档/恢复、项目归并、删除会话；需 `--features probe`）。
//!
//! 与 UI 命令共享同一编排核心；每个写入口执行前自动三件套备份（ADR-0018）。
//! 用法：
//!   cargo run --example p15_acceptance --features probe -- --list
//!   cargo run --example p15_acceptance --features probe -- --backup
//!   cargo run --example p15_acceptance --features probe -- --archive <会话id,逗号分隔> [--restore <id,id>]
//!   cargo run --example p15_acceptance --features probe -- --merge <源项目id,逗号分隔> --target <目标项目id>
//!   cargo run --example p15_acceptance --features probe -- --delete <会话id,逗号分隔>

#[cfg(feature = "probe")]
fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mode = args.first().map(String::as_str).unwrap_or("");
    let split_ids = |value: Option<&String>| -> Vec<String> {
        value
            .map(|raw| {
                raw.split(',')
                    .map(|item| item.trim().to_string())
                    .filter(|item| !item.is_empty())
                    .collect()
            })
            .unwrap_or_default()
    };
    let flag_value = |flag: &str| -> Option<String> {
        args.iter()
            .position(|item| item == flag)
            .and_then(|index| args.get(index + 1))
            .cloned()
    };
    let result: Result<serde_json::Value, String> = match mode {
        "--list" => trae_sync_lib::library_archive::acceptance_probe::list_snapshot(),
        "--backup" => trae_sync_lib::library_archive::acceptance_probe::backup_now(),
        "--archive" => trae_sync_lib::library_archive::acceptance_probe::archive_apply(
            split_ids(flag_value("--archive").as_ref()),
            split_ids(flag_value("--restore").as_ref()),
        ),
        "--merge" => {
            let Some(target) = flag_value("--target") else {
                eprintln!("缺少 --target <目标项目id>");
                std::process::exit(2);
            };
            trae_sync_lib::library_archive::acceptance_probe::merge_projects(
                split_ids(flag_value("--merge").as_ref()),
                target,
            )
        }
        "--delete" => trae_sync_lib::library_archive::acceptance_probe::delete_sessions(
            split_ids(flag_value("--delete").as_ref()),
        ),
        "--migrate-backups" => {
            let Some(target) = flag_value("--migrate-backups") else {
                eprintln!("缺少 --migrate-backups <目标目录>");
                std::process::exit(2);
            };
            trae_sync_lib::library_archive::acceptance_probe::migrate_backups(
                std::path::Path::new(&target),
            )
        }
        other => {
            eprintln!("未知模式 {other:?}；见文件头用法注释");
            std::process::exit(2);
        }
    };
    match result {
        Ok(value) => {
            println!("{}", serde_json::to_string_pretty(&value).unwrap_or_default());
        }
        Err(error) => {
            eprintln!("失败：{error}");
            std::process::exit(1);
        }
    }
}

#[cfg(not(feature = "probe"))]
fn main() {
    eprintln!("需以 --features probe 编译运行");
    std::process::exit(2);
}
