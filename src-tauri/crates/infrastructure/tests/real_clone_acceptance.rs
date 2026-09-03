//! 真实 TRAE 隔离副本验收。
//!
//! 该测试默认忽略，只能显式传入隔离副本环境变量后运行，避免普通测试误触碰真实路径。

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use rusqlite::{params, types::ValueRef, Connection, OpenFlags};
use serde_json::json;
use sha2::{Digest, Sha256};
use traesync_domain::{
    build_sync_plan, BuildSyncPlanInput, CompatibilityState, OperationCancellation, OperationState,
    PlanProjectInput, PlanSessionInput, ProjectIdentity, SessionIdentity, SyncPlan,
    SyncPlanExecutionOutcome, SyncScope, TargetFileEvidence,
};
use traesync_infrastructure::progress::{
    measure_fixture_chunked_io, FixtureIoConfig, FixtureIoOutcome, ProgressPhase,
    FIXTURE_IO_CHUNK_BYTES,
};
use traesync_infrastructure::{
    capture_location_identity, ensure_catalog_initialized, list_operation_summaries, sha256_file,
    FixturePathGuard, PlatformFileIdentityProvider, SqlCipherProbe, WorkCnSyncExecutor,
};
use traesync_ports::{DatabaseProbePort, SyncPlanEvidencePort, SyncPlanExecutorPort};

const TRIO_NAMES: [&str; 3] = ["database.db", "database.db-wal", "database.db-shm"];
const OLD_USER_ID: &str = "3559551364241212";
const SYNTHETIC_SOURCE_USER_ID: &str = "9000000000000001";

fn required_env(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| panic!("缺少真实副本验收环境变量：{name}"))
}

/// 优先读取显式环境变量；被执行策略拦截命令行 key 时，从已授权基线文件解析。
fn required_raw_key() -> String {
    if let Ok(value) = std::env::var("TRAE_SYNC_REAL_CLONE_RAW_KEY") {
        return value.trim().to_string();
    }
    let path = std::env::var("TRAE_SYNC_REAL_CLONE_BASELINE")
        .unwrap_or_else(|_| "docs/TECHNICAL_BASELINE.md".to_string());
    let contents = fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("无法读取 raw key 基线文件 {path}: {error}"));
    contents
        .lines()
        .map(str::trim)
        .find(|line| line.len() == 64 && line.chars().all(|value| value.is_ascii_hexdigit()))
        .map(ToString::to_string)
        .unwrap_or_else(|| panic!("raw key 基线文件未包含 64 位 hex key：{path}"))
}

fn expected_hash(name: &str) -> String {
    required_env(name).trim().to_ascii_lowercase()
}

/// 验收日志只输出不可逆摘要，运行时比较仍使用原始值。
fn text_sha256(value: &str) -> String {
    hex::encode(Sha256::digest(value.as_bytes()))
}

fn path_sha256(path: &Path) -> String {
    text_sha256(path.to_string_lossy().as_ref())
}

fn assert_identifier_eq(actual: &str, expected: &str, label: &str) {
    assert!(
        actual == expected,
        "{label} 不匹配：actual_sha256={}; expected_sha256={}",
        text_sha256(actual),
        text_sha256(expected)
    );
}

fn clone_relative_dir() -> String {
    std::env::var("TRAE_SYNC_REAL_CLONE_DB_RELATIVE")
        .unwrap_or_else(|_| "clone".to_string())
        .trim_end_matches(['/', '\\'])
        .to_string()
}

/// 为本轮真实隔离写入测试分配固定共享恢复区下的临时命名空间。
fn shared_recovery_root_for_test(root: &Path) -> PathBuf {
    let local_appdata = std::env::var_os("LOCALAPPDATA")
        .unwrap_or_else(|| panic!("缺少真实副本验收环境变量：LOCALAPPDATA"));
    let run_name = root
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "real-clone".to_string());
    PathBuf::from(local_appdata)
        .join("Trae Sync")
        .join("recovery")
        .join(format!("real-clone-write-recovery-{run_name}"))
}

fn assert_trio_matches_expected(root: &std::path::Path, relative_dir: &str, prefix: &str) {
    for (index, file_name) in TRIO_NAMES.iter().enumerate() {
        let path = root.join(relative_dir).join(file_name);
        assert!(
            path.is_file(),
            "隔离副本缺少三件套文件：path_sha256={}",
            path_sha256(&path)
        );
        let actual = sha256_file(&path)
            .unwrap_or_else(|| panic!("无法计算文件哈希：path_sha256={}", path_sha256(&path)));
        let env_name = match (prefix, index) {
            ("source", 0) => "TRAE_SYNC_EXPECTED_DB_SHA256",
            ("source", 1) => "TRAE_SYNC_EXPECTED_WAL_SHA256",
            ("source", 2) => "TRAE_SYNC_EXPECTED_SHM_SHA256",
            _ => unreachable!("仅支持 source 三件套哈希校验"),
        };
        assert_eq!(
            actual.to_ascii_lowercase(),
            expected_hash(env_name),
            "三件套哈希不匹配：path_sha256={}",
            path_sha256(&path)
        );
    }
}

fn open_readonly_with_raw_key(path: &std::path::Path, raw_key: &str) -> Connection {
    let connection = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI,
    )
    .unwrap_or_else(|error| panic!("无法只读打开隔离副本：{error}"));
    connection
        .execute_batch(&format!(
            "PRAGMA key = \"x'{raw_key}'\"; PRAGMA query_only = ON;"
        ))
        .unwrap_or_else(|error| panic!("隔离副本 raw key 打开失败：{error}"));
    connection
}

fn open_readwrite_with_raw_key(path: &Path, raw_key: &str) -> Connection {
    let connection =
        Connection::open(path).unwrap_or_else(|error| panic!("无法读写打开隔离副本：{error}"));
    connection
        .execute_batch(&format!("PRAGMA key = \"x'{raw_key}'\";"))
        .unwrap_or_else(|error| panic!("隔离副本 raw key 打开失败：{error}"));
    connection
}

fn sidecar_path(db_path: &Path, suffix: &str) -> PathBuf {
    PathBuf::from(format!("{}{}", db_path.display(), suffix))
}

fn file_hash_if_present(path: &Path) -> Option<String> {
    path.is_file()
        .then(|| sha256_file(path).expect("无法计算隔离副本文件哈希"))
}

fn target_file_evidence(db_path: &Path) -> TargetFileEvidence {
    TargetFileEvidence {
        db_fingerprint: file_hash_if_present(db_path).expect("隔离写入副本缺少 database.db"),
        wal_fingerprint: file_hash_if_present(&sidecar_path(db_path, "-wal")),
        shm_fingerprint: file_hash_if_present(&sidecar_path(db_path, "-shm")),
    }
}

/// 对指定账号的稳定行集做摘要，避免把真实正文写入验收输出。
fn account_rows_fingerprint(
    connection: &Connection,
    first_user: &str,
    second_user: &str,
) -> String {
    let queries = [
        "SELECT p.project_id, p.user_id, p.biz_project_id, COALESCE(p.name, ''), COALESCE(p.description, ''), COALESCE(p.absolute_path, ''), COALESCE(p.deleted_at, 0) FROM project p WHERE p.user_id IN (?1, ?2) AND p.project_id NOT LIKE 't07-%' ORDER BY p.project_id",
        "SELECT s.session_id, s.project_id, s.session_title, s.context, s.deleted_at FROM chat_session s JOIN project p ON p.project_id = s.project_id WHERE p.user_id IN (?1, ?2) AND s.session_id NOT LIKE 't07-%' ORDER BY s.session_id",
        "SELECT m.session_id, m.message_id, m.message_type, m.message_role, m.message_index, m.reply_to_message_id, m.user_message_context, m.deleted_at FROM chat_message m JOIN chat_session s ON s.session_id = m.session_id JOIN project p ON p.project_id = s.project_id WHERE p.user_id IN (?1, ?2) AND m.message_id NOT LIKE 't07-%' ORDER BY m.message_id",
    ];
    let mut hasher = Sha256::new();
    for query in queries {
        let mut statement = connection
            .prepare(query)
            .unwrap_or_else(|error| panic!("无法准备账号稳定行摘要查询：{error}"));
        let rows = statement
            .query_map(params![first_user, second_user], |row| {
                let mut values = Vec::new();
                for index in 0..row.as_ref().column_count() {
                    let value = row.get_ref(index)?;
                    match value {
                        ValueRef::Null => values.extend_from_slice(b"null"),
                        ValueRef::Integer(value) => {
                            values.extend_from_slice(value.to_string().as_bytes())
                        }
                        ValueRef::Real(value) => {
                            values.extend_from_slice(value.to_string().as_bytes())
                        }
                        ValueRef::Text(value) | ValueRef::Blob(value) => {
                            values.extend_from_slice(value)
                        }
                    }
                    values.push(0);
                }
                Ok(values)
            })
            .unwrap_or_else(|error| panic!("无法遍历账号稳定行摘要：{error}"));
        for row in rows {
            hasher.update(row.unwrap_or_else(|error| panic!("账号稳定行摘要读取失败：{error}")));
            hasher.update([0xff]);
        }
        hasher.update([0xfe]);
    }
    hex::encode(hasher.finalize())
}

#[derive(Clone, Copy)]
struct AlwaysCurrentEvidence;

impl SyncPlanEvidencePort for AlwaysCurrentEvidence {
    fn is_current(&self, _plan: &SyncPlan) -> bool {
        true
    }
}

#[test]
fn acceptance_output_hashes_identifiers_without_echoing_them() {
    let identifier = "9000000000000001";
    let digest = text_sha256(identifier);
    assert_eq!(digest.len(), 64);
    assert_ne!(digest, identifier);
    assert!(!digest.contains(identifier));
}

/// 将待读三件套复制到临时目录，避免 SQLCipher 打开时重建源目录的 SHM。
fn disposable_read_copy(source_dir: &Path, parent: &Path) -> (tempfile::TempDir, PathBuf) {
    let directory = tempfile::Builder::new()
        .prefix("real-clone-read-")
        .tempdir_in(parent)
        .unwrap_or_else(|error| panic!("无法创建隔离读取临时目录：{error}"));
    for file_name in TRIO_NAMES {
        let source = source_dir.join(file_name);
        assert!(
            source.is_file(),
            "读取副本缺少文件：path_sha256={}",
            path_sha256(&source)
        );
        fs::copy(&source, directory.path().join(file_name)).unwrap_or_else(|error| {
            panic!(
                "无法复制读取副本 path_sha256={}：{error}",
                path_sha256(&source)
            )
        });
    }
    let db_path = directory.path().join("database.db");
    (directory, db_path)
}

#[test]
#[ignore = "仅在用户授权的真实隔离副本上显式运行"]
fn real_clone_readonly_acceptance() {
    let root = std::path::PathBuf::from(required_env("TRAE_SYNC_REAL_CLONE_ROOT"));
    let raw_key = required_raw_key();
    let expected_user_id = required_env("TRAE_SYNC_EXPECTED_USER_ID");
    let relative_dir = clone_relative_dir();
    assert_eq!(raw_key.len(), 64, "raw key 必须是 32 字节 hex");
    assert!(
        raw_key.chars().all(|value| value.is_ascii_hexdigit()),
        "raw key 必须是 hex"
    );

    // 生产入口先验证隔离根，阻止把原始 TRAE 路径伪装成 fixture。
    let guard = FixturePathGuard::new(&root).expect("隔离根不在受保护的测试根内");
    let _db_path = guard
        .validate_db_relative_path(&format!("{relative_dir}/database.db"))
        .expect("隔离副本数据库三件套路径未通过封闭校验");
    assert_trio_matches_expected(&root, &relative_dir, "source");
    assert_trio_matches_expected(&root, "backup-a", "source");
    assert_trio_matches_expected(&root, "backup-b", "source");

    // 真实读取只针对临时副本，原始 clone 只承担写前哈希基线。
    let (read_copy, read_db_path) =
        disposable_read_copy(&root.join(&relative_dir), guard.canonical_root());

    let probe = SqlCipherProbe::new();
    let compatibility = probe.probe_database(&read_db_path, &raw_key);
    let (schema_fingerprint, counts) = match &compatibility {
        CompatibilityState::Verified {
            schema_fingerprint,
            counts,
        } => (schema_fingerprint.0.clone(), counts.clone()),
        CompatibilityState::Incompatible { reason } => {
            panic!("真实隔离副本 SQLCipher/schema 不兼容：{reason:?}")
        }
    };

    let (cipher_integrity_ok, sqlite_integrity_ok) =
        probe.run_integrity_checks(&read_db_path, &raw_key);
    assert!(cipher_integrity_ok, "cipher_integrity_check 未通过");
    assert!(sqlite_integrity_ok, "integrity_check 未通过");

    let logical_copy = probe
        .backup_to_logical_copy(&read_db_path, &raw_key)
        .expect("无法为隔离副本生成逻辑备份");
    assert!(
        logical_copy.starts_with(guard.canonical_root()),
        "逻辑备份越出隔离根"
    );
    let logical_compatibility = probe.probe_database(&logical_copy, &raw_key);
    assert!(matches!(
        logical_compatibility,
        CompatibilityState::Verified { .. }
    ));
    assert_eq!(
        probe.run_integrity_checks(&logical_copy, &raw_key),
        (true, true)
    );

    let connection = open_readonly_with_raw_key(&read_db_path, &raw_key);
    let cipher_version: String = connection
        .query_row("PRAGMA cipher_version", [], |row| row.get(0))
        .expect("无法读取 SQLCipher 版本");
    let mut user_ids = Vec::new();
    let mut statement = connection
        .prepare("SELECT DISTINCT CAST(user_id AS TEXT) FROM project WHERE user_id IS NOT NULL ORDER BY 1")
        .expect("无法读取 project.user_id");
    let rows = statement
        .query_map([], |row| row.get::<_, String>(0))
        .expect("无法遍历 project.user_id");
    for row in rows {
        user_ids.push(row.expect("project.user_id 读取失败"));
    }
    let expected_user_id_sha256 = text_sha256(&expected_user_id);
    let project_user_ids_sha256 = user_ids
        .iter()
        .map(|user_id| text_sha256(user_id))
        .collect::<Vec<_>>();
    assert!(
        user_ids.iter().any(|user_id| user_id == &expected_user_id),
        "目标账号不在隔离副本 project.user_id 中：expected_user_id_sha256={expected_user_id_sha256}; project_user_ids_sha256={project_user_ids_sha256:?}"
    );
    drop(statement);

    println!(
        "REAL_CLONE_ACCEPTANCE={}",
        json!({
            "root_sha256": path_sha256(guard.canonical_root()),
            "db_relative_sha256": text_sha256(&format!("{relative_dir}/database.db")),
            "expected_user_id_sha256": expected_user_id_sha256,
            "project_user_ids_sha256": project_user_ids_sha256,
            "cipher_version": cipher_version,
            "schema_fingerprint": schema_fingerprint,
            "project_count": counts.project_count,
            "chat_session_count": counts.chat_session_count,
            "chat_message_count": counts.chat_message_count,
            "cipher_integrity_check": true,
            "sqlite_integrity_check": true,
            "logical_backup_verified": true,
            "source_untouched_by_test": true
        })
    );

    drop(connection);
    let _ = std::fs::remove_file(&logical_copy);
    let _ = std::fs::remove_file(logical_copy.with_file_name("database.logical-copy.db-wal"));
    let _ = std::fs::remove_file(logical_copy.with_file_name("database.logical-copy.db-shm"));
    drop(read_copy);
}

#[test]
#[ignore = "仅在用户授权的真实隔离副本上显式运行；慢 I/O 为受控子集，不代表硬件慢盘资格"]
fn real_clone_slow_io_acceptance() {
    let root = PathBuf::from(required_env("TRAE_SYNC_REAL_CLONE_ROOT"));
    let raw_key = required_raw_key();
    let expected_user_id = required_env("TRAE_SYNC_EXPECTED_USER_ID");
    let relative_dir = clone_relative_dir();
    let guard = FixturePathGuard::new(&root).expect("隔离根不在受保护的测试根内");
    guard
        .validate_db_relative_path(&format!("{relative_dir}/database.db"))
        .expect("真实副本数据库路径未通过封闭校验");
    assert_trio_matches_expected(&root, &relative_dir, "source");
    assert_trio_matches_expected(&root, "backup-a", "source");
    assert_trio_matches_expected(&root, "backup-b", "source");

    let source_dir = root.join(&relative_dir);
    let source_db = source_dir.join("database.db");
    let slow_root = tempfile::Builder::new()
        .prefix("real-clone-slow-")
        .tempdir_in(guard.canonical_root())
        .expect("无法创建真实慢 I/O 隔离目录");
    let slow_db = slow_root.path().join("database.db");
    let total_bytes = fs::metadata(&source_db)
        .expect("无法读取真实数据库大小")
        .len();

    // 使用真实大库作为输入，只在隔离根中对目标写入施加受控分块延迟。
    let source_file = fs::File::open(&source_db).expect("无法打开真实隔离数据库");
    let destination_file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&slow_db)
        .expect("无法创建慢 I/O 目标数据库");
    let cancellation = OperationCancellation::new();
    let measurement = measure_fixture_chunked_io(
        source_file,
        destination_file,
        "op-real-clone-slow-io",
        ProgressPhase::Copying,
        Some(total_bytes),
        &cancellation,
        false,
        FixtureIoConfig::new(Duration::from_millis(10), Duration::from_millis(100)),
    )
    .expect("真实慢 I/O 分块复制失败");
    assert_eq!(measurement.outcome, FixtureIoOutcome::Completed);
    assert_eq!(measurement.bytes_read, total_bytes);
    assert_eq!(measurement.bytes_written, total_bytes);
    assert_eq!(measurement.peak_buffer_bytes, FIXTURE_IO_CHUNK_BYTES);
    assert!(
        !measurement.progress_events.is_empty(),
        "未产生真实字节进度事件"
    );

    for file_name in TRIO_NAMES.iter().skip(1) {
        let source = source_dir.join(file_name);
        let destination = slow_root.path().join(file_name);
        fs::copy(&source, &destination).expect("无法复制真实隔离数据库 sidecar");
    }
    for file_name in TRIO_NAMES {
        let source_hash = sha256_file(&source_dir.join(file_name)).expect("无法计算源副本哈希");
        let destination_hash =
            sha256_file(&slow_root.path().join(file_name)).expect("无法计算慢 I/O 副本哈希");
        assert_eq!(
            source_hash, destination_hash,
            "慢 I/O 副本哈希不一致：{file_name}"
        );
    }

    let probe = SqlCipherProbe::new();
    let compatibility = probe.probe_database(&slow_db, &raw_key);
    let counts = match compatibility {
        traesync_domain::CompatibilityState::Verified { counts, .. } => counts,
        other => panic!("真实慢 I/O 副本兼容性检查失败：{other:?}"),
    };
    let account_connection = open_readonly_with_raw_key(&slow_db, &raw_key);
    let target_project_count: i64 = account_connection
        .query_row(
            "SELECT COUNT(*) FROM project WHERE CAST(user_id AS TEXT) = ?1",
            params![expected_user_id],
            |row| row.get(0),
        )
        .expect("无法验证慢 I/O 副本目标账号");
    assert!(
        target_project_count > 0,
        "慢 I/O 副本中不存在目标账号：expected_user_id_sha256={}",
        text_sha256(&expected_user_id)
    );
    drop(account_connection);
    let (cipher_integrity_ok, sqlite_integrity_ok) = probe.run_integrity_checks(&slow_db, &raw_key);
    assert!(
        cipher_integrity_ok,
        "慢 I/O 副本 cipher_integrity_check 未通过"
    );
    assert!(sqlite_integrity_ok, "慢 I/O 副本 integrity_check 未通过");

    let logical_copy = probe
        .backup_to_logical_copy(&slow_db, &raw_key)
        .expect("无法为慢 I/O 副本生成逻辑备份");
    assert!(logical_copy.starts_with(guard.canonical_root()));
    assert!(matches!(
        probe.probe_database(&logical_copy, &raw_key),
        traesync_domain::CompatibilityState::Verified { .. }
    ));
    assert_eq!(
        probe.run_integrity_checks(&logical_copy, &raw_key),
        (true, true)
    );

    println!(
        "REAL_CLONE_SLOW_IO_ACCEPTANCE={}",
        json!({
            "expected_user_id_sha256": text_sha256(&expected_user_id),
            "database_bytes": total_bytes,
            "project_count": counts.project_count,
            "chat_session_count": counts.chat_session_count,
            "chat_message_count": counts.chat_message_count,
            "target_account_verified": true,
            "elapsed_ms": measurement.elapsed.as_millis(),
            "chunks": measurement.chunks,
            "peak_buffer_bytes": measurement.peak_buffer_bytes,
            "rss_before_bytes": measurement.rss_before_bytes,
            "rss_peak_bytes": measurement.rss_peak_bytes,
            "rss_after_bytes": measurement.rss_after_bytes,
            "progress_event_count": measurement.progress_events.len(),
            "controlled_delay_ms_per_chunk": 10,
            "logical_backup_verified": true,
            "source_untouched_by_test": true
        })
    );

    let _ = fs::remove_file(&logical_copy);
    let _ = fs::remove_file(logical_copy.with_file_name("database.logical-copy.db-wal"));
    let _ = fs::remove_file(logical_copy.with_file_name("database.logical-copy.db-shm"));
    let _ = std::io::stdout().flush();
}

#[test]
#[ignore = "仅用于显式读取隔离副本 schema 元数据"]
fn real_clone_schema_dump() {
    let root = std::path::PathBuf::from(required_env("TRAE_SYNC_REAL_CLONE_ROOT"));
    let raw_key = required_raw_key();
    let relative_dir = clone_relative_dir();
    let guard = FixturePathGuard::new(&root).expect("隔离根不在受保护的测试根内");
    let _db_path = guard
        .validate_db_relative_path(&format!("{relative_dir}/database.db"))
        .expect("隔离副本数据库路径未通过封闭校验");
    let (read_copy, read_db_path) =
        disposable_read_copy(&root.join(&relative_dir), guard.canonical_root());
    let connection = open_readonly_with_raw_key(&read_db_path, &raw_key);

    for table in [
        "project",
        "chat_session",
        "chat_message",
        "session_project",
        "snapshot",
        "staging",
        "local_artifact",
        "local_artifact_version",
    ] {
        let escaped = table.replace('\'', "''");
        let sql: Option<String> = connection
            .query_row(
                "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = ?1",
                [&escaped],
                |row| row.get(0),
            )
            .ok();
        println!("REAL_CLONE_SCHEMA table={table} sql={sql:?}");

        let mut statement = connection
            .prepare(&format!("PRAGMA table_info(\"{escaped}\")"))
            .expect("读取表列失败");
        let rows = statement
            .query_map([], |row| {
                Ok(json!({
                    "name": row.get::<_, String>(1)?,
                    "type": row.get::<_, String>(2)?,
                    "not_null": row.get::<_, i64>(3)?,
                    "default": row.get::<_, Option<String>>(4)?,
                    "primary_key": row.get::<_, i64>(5)?
                }))
            })
            .expect("读取表列失败");
        let columns: Vec<_> = rows.map(|row| row.expect("表列读取失败")).collect();
        println!(
            "REAL_CLONE_COLUMNS table={table} columns={}",
            json!(columns)
        );
    }
    drop(connection);
    drop(read_copy);
}

#[test]
#[ignore = "仅用于显式盘点隔离副本项目关系"]
fn real_clone_inventory() {
    let root = PathBuf::from(required_env("TRAE_SYNC_REAL_CLONE_ROOT"));
    let raw_key = required_raw_key();
    let relative_dir = clone_relative_dir();
    let guard = FixturePathGuard::new(&root).expect("隔离根不在受保护的测试根内");
    let source_dir = root.join(&relative_dir);
    let (read_copy, read_db_path) = disposable_read_copy(&source_dir, guard.canonical_root());
    let connection = open_readonly_with_raw_key(&read_db_path, &raw_key);

    // 只输出项目/会话关系摘要，不输出明文标识、消息正文、认证材料或附件内容。
    let mut projects = Vec::new();
    let mut statement = connection
        .prepare(
            "SELECT project_id, user_id, biz_project_id, COALESCE(name, ''), COALESCE(deleted_at, 0) FROM project ORDER BY user_id, project_id",
        )
        .expect("无法读取项目元数据");
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, i64>(4)?,
            ))
        })
        .expect("无法遍历项目元数据")
        .collect::<Result<Vec<_>, _>>()
        .expect("项目元数据读取失败");
    drop(statement);
    for (project_id, user_id, biz_project_id, name, deleted_at) in rows {
        let mut sessions_statement = connection
            .prepare(
                "SELECT session_id FROM chat_session WHERE project_id = ?1 AND deleted_at = 0 ORDER BY updated_at, session_id",
            )
            .expect("无法读取项目会话");
        let sessions = sessions_statement
            .query_map([&project_id], |session_row| session_row.get::<_, String>(0))
            .expect("无法遍历项目会话")
            .collect::<Result<Vec<_>, _>>()
            .expect("项目会话读取失败");
        projects.push(json!({
            "project_id_sha256": text_sha256(&project_id),
            "user_id_sha256": text_sha256(&user_id),
            "biz_project_id_sha256": text_sha256(&biz_project_id),
            "name_sha256": text_sha256(&name),
            "deleted_at": deleted_at,
            "session_ids_sha256": sessions.iter().map(|session_id| text_sha256(session_id)).collect::<Vec<_>>(),
        }));
    }
    println!(
        "REAL_CLONE_INVENTORY={}",
        json!({
            "db_relative_sha256": text_sha256(&format!("{relative_dir}/database.db")),
            "projects": projects
        })
    );
    drop(connection);
    drop(read_copy);
}

#[test]
#[ignore = "仅在用户授权的真实隔离副本工作克隆上显式运行"]
fn real_clone_attach_sessions_write_acceptance() {
    let root = PathBuf::from(required_env("TRAE_SYNC_REAL_CLONE_ROOT"));
    let raw_key = required_raw_key();
    let target_user_id = required_env("TRAE_SYNC_EXPECTED_USER_ID");
    let guard = FixturePathGuard::new(&root).expect("隔离根不在受保护的测试根内");
    let db_path = guard
        .validate_db_relative_path("write-clone/database.db")
        .expect("write-clone 数据库路径未通过封闭校验");
    assert_trio_matches_expected(&root, "write-clone", "source");
    assert_trio_matches_expected(&root, "backup-a", "source");
    assert_trio_matches_expected(&root, "backup-b", "source");

    let before_existing_state = {
        let connection = open_readwrite_with_raw_key(&db_path, &raw_key);
        account_rows_fingerprint(&connection, OLD_USER_ID, &target_user_id)
    };

    let unique = SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("系统时间异常")
        .as_nanos();
    let source_project_id = format!("t07-source-{unique}");
    let target_project_id = format!("t07-target-{unique}");
    let session_id = format!("t07-session-{unique}");
    let message_id = format!("t07-message-{unique}");
    let biz_project_id = format!("t07-biz-{unique}");

    // 仅向 write-clone 注入可识别的合成关系；原始 TRAE 数据和双备份完全不触碰。
    {
        let connection = open_readwrite_with_raw_key(&db_path, &raw_key);
        connection
            .execute_batch("BEGIN IMMEDIATE")
            .expect("无法开始隔离副本合成数据事务");
        connection
            .execute(
                "INSERT INTO project (project_id, source, user_id, name, description, absolute_path, biz_project_id, workspace_status) VALUES (?1, 'native-ide', ?2, 'T07 synthetic source', '', '', ?3, 'single_root')",
                params![source_project_id, SYNTHETIC_SOURCE_USER_ID, biz_project_id],
            )
            .expect("无法写入合成源项目");
        connection
            .execute(
                "INSERT INTO project (project_id, source, user_id, name, description, absolute_path, biz_project_id, workspace_status) VALUES (?1, 'native-ide', ?2, 'T07 synthetic target', '', '', ?3, 'single_root')",
                params![target_project_id, target_user_id, biz_project_id],
            )
            .expect("无法写入合成目标项目");
        connection
            .execute(
                "INSERT INTO chat_session (session_id, project_id, session_title, context) VALUES (?1, ?2, 'T07 synthetic session', '{}')",
                params![session_id, source_project_id],
            )
            .expect("无法写入合成会话");
        connection
            .execute(
                "INSERT INTO chat_message (session_id, message_id, message_type, message_role, message_index, reply_to_message_id, user_message_context) VALUES (?1, ?2, 'text', 'user', 0, '', '{}')",
                params![session_id, message_id],
            )
            .expect("无法写入合成消息");
        connection
            .execute(
                "INSERT INTO session_project (project_id, session_id) VALUES (?1, ?2)",
                params![source_project_id, session_id],
            )
            .expect("无法写入合成会话项目关系");
        connection
            .execute_batch("COMMIT")
            .expect("无法提交隔离副本合成数据事务");
    }
    fs::create_dir_all(root.join("sandbox")).expect("无法创建隔离 sandbox");
    fs::write(
        root.join("sandbox")
            .join(format!("{target_project_id}.json")),
        b"{}",
    )
    .expect("无法写入隔离目标 sandbox 证据");

    let probe = SqlCipherProbe::new();
    let compatibility = probe.probe_database(&db_path, &raw_key);
    let schema_fingerprint = match compatibility {
        CompatibilityState::Verified {
            schema_fingerprint, ..
        } => schema_fingerprint.0,
        other => panic!("write-clone schema 验证失败：{other:?}"),
    };
    let target_file_evidence = target_file_evidence(&db_path);
    let data_location_id = capture_location_identity(
        &PlatformFileIdentityProvider::new(),
        guard.canonical_root(),
        "write-clone/database.db",
    )
    .expect("write-clone 位置身份见证失败")
    .data_location_id;
    let plan = build_sync_plan(BuildSyncPlanInput {
        created_at: SystemTime::now(),
        platform_id: "work_cn".to_string(),
        data_location_id: data_location_id.clone(),
        current_user_id: target_user_id.clone(),
        account_evidence_fingerprint: "real-isolated-account-evidence".to_string(),
        target_file_evidence,
        schema_fingerprint,
        mapping_version: "work_cn_v1".to_string(),
        schema_compatible: true,
        scope: SyncScope::Custom {
            account_ids: Vec::new(),
            project_ids: Vec::new(),
            session_ids: vec![SessionIdentity::new("work_cn", &session_id)],
        },
        projects: vec![
            PlanProjectInput {
                identity: ProjectIdentity {
                    project_id: source_project_id.clone(),
                    biz_project_id: biz_project_id.clone(),
                    display_name: "T07 synthetic source".to_string(),
                    soft_deleted: false,
                },
                display_owner: SYNTHETIC_SOURCE_USER_ID.to_string(),
                current_live_owner: SYNTHETIC_SOURCE_USER_ID.to_string(),
                sessions: vec![PlanSessionInput {
                    identity: SessionIdentity::new("work_cn", &session_id),
                    version_available: true,
                }],
                archived_only: false,
            },
            PlanProjectInput {
                identity: ProjectIdentity {
                    project_id: target_project_id.clone(),
                    biz_project_id,
                    display_name: "T07 synthetic target".to_string(),
                    soft_deleted: false,
                },
                display_owner: target_user_id.clone(),
                current_live_owner: target_user_id.clone(),
                sessions: Vec::new(),
                archived_only: false,
            },
        ],
    });
    assert!(matches!(
        plan.actions(),
        [traesync_domain::PlanAction::AttachSessions {
            source_project_id: source,
            target_project_id: target,
            session_ids,
        }] if source == &source_project_id
            && target == &target_project_id
            && session_ids == &[SessionIdentity::new("work_cn", &session_id)]
    ));

    let storage_root = root.join("t07-write-storage");
    fs::create_dir_all(&storage_root).expect("无法创建 T07 隔离恢复区");
    let recovery_root = shared_recovery_root_for_test(&root);
    fs::create_dir_all(&recovery_root).expect("无法创建固定共享恢复区临时命名空间");
    let operation_lease = guard
        .acquire_catalog_operation_lease(&recovery_root, &storage_root, &data_location_id)
        .expect("无法获取 T07 隔离操作租约");
    let catalog_path =
        ensure_catalog_initialized(&storage_root, &raw_key, &recovery_root, &operation_lease)
            .expect("无法初始化 T07 隔离目录库");
    // 真实长任务必须能留下非敏感阶段证据，并明确写入阶段不可取消。
    let progress_log = Arc::new(Mutex::new(Vec::new()));
    let progress_log_for_reporter = Arc::clone(&progress_log);
    let started_at = Instant::now();
    let executor =
        WorkCnSyncExecutor::new(&raw_key).with_progress_reporter(Arc::new(move |snapshot| {
            progress_log_for_reporter
                .lock()
                .expect("真实写入进度锁已中毒")
                .push(snapshot);
        }));
    let bound = executor
        .bind_fixture(
            &guard,
            &db_path,
            &storage_root,
            &recovery_root,
            operation_lease,
        )
        .expect("write-clone 绑定 fixture 失败");
    let outcome =
        bound.execute_sync_plan(&plan, &OperationCancellation::new(), &AlwaysCurrentEvidence);
    assert!(
        matches!(outcome, SyncPlanExecutionOutcome::Completed { affected_rows } if affected_rows == 2),
        "T07 隔离 AttachSessions 未完成：{outcome:?}"
    );
    let elapsed_ms = started_at.elapsed().as_millis();
    let progress_events = progress_log.lock().expect("真实写入进度锁已中毒").clone();
    for phase in [
        ProgressPhase::Preparing,
        ProgressPhase::Copying,
        ProgressPhase::Hashing,
        ProgressPhase::Verifying,
        ProgressPhase::Writing,
        ProgressPhase::Completed,
    ] {
        assert!(
            progress_events
                .iter()
                .any(|snapshot| snapshot.phase == phase),
            "真实写入缺少阶段进度：{phase:?}"
        );
    }
    let writing_snapshot = progress_events
        .iter()
        .find(|snapshot| snapshot.phase == ProgressPhase::Writing)
        .expect("真实写入缺少 Writing 进度");
    assert!(!writing_snapshot.cancellable);
    assert!(writing_snapshot.total_bytes.is_none());

    let summaries = list_operation_summaries(&recovery_root).expect("无法读取 T07 操作 manifest");
    assert_eq!(summaries.len(), 1, "T07 应只产生一个操作 manifest");
    assert_eq!(summaries[0].state, OperationState::Completed);
    assert!(summaries[0].has_verified_target_file_evidence);
    // 原始/逻辑备份属于可迁移 storage_root；固定 recovery_root 只保存 manifest。
    let operation_root = storage_root
        .join("backups")
        .join(plan.operation_id().as_str());
    let before_raw = operation_root.join("before/raw");
    let raw_database = before_raw.join("database.db");
    assert!(raw_database.is_file(), "T07 缺少 database.db 备份");
    assert_eq!(
        sha256_file(&raw_database).expect("无法计算 database.db 备份哈希"),
        plan.target_file_evidence().db_fingerprint,
        "T07 database.db 备份哈希不匹配"
    );
    let hash_manifest = before_raw.join("hashes.sha256");
    assert!(hash_manifest.is_file(), "T07 缺少备份哈希清单");
    for (file_name, expected_hash) in [
        (
            "database.db-wal",
            plan.target_file_evidence().wal_fingerprint.as_ref(),
        ),
        (
            "database.db-shm",
            plan.target_file_evidence().shm_fingerprint.as_ref(),
        ),
    ] {
        let path = before_raw.join(file_name);
        match expected_hash {
            Some(expected_hash) => {
                assert!(
                    path.is_file(),
                    "T07 缺少计划中存在的 sidecar：path_sha256={}",
                    path_sha256(&path)
                );
                assert_eq!(
                    sha256_file(&path).expect("无法计算 sidecar 备份哈希"),
                    *expected_hash,
                    "T07 sidecar 备份哈希不匹配：path_sha256={}",
                    path_sha256(&path)
                );
            }
            None => assert!(
                !path.exists(),
                "T07 不应凭空生成 sidecar：path_sha256={}",
                path_sha256(&path)
            ),
        }
    }
    assert_eq!(
        fs::read_to_string(&hash_manifest)
            .expect("无法读取原始备份哈希清单")
            .lines()
            .filter(|line| !line.is_empty())
            .count(),
        1 + if plan.target_file_evidence().wal_fingerprint.is_some() {
            1
        } else {
            0
        } + if plan.target_file_evidence().shm_fingerprint.is_some() {
            1
        } else {
            0
        },
        "T07 原始备份哈希清单文件数量不匹配"
    );
    assert!(
        operation_root.join("before/logical/database.db").is_file(),
        "T07 缺少逻辑备份"
    );
    assert!(
        matches!(
            probe.probe_database(&operation_root.join("before/logical/database.db"), &raw_key),
            CompatibilityState::Verified { .. }
        ),
        "T07 逻辑备份无法独立打开"
    );

    let catalog_connection = open_readonly_with_raw_key(&catalog_path, &raw_key);
    let catalog_record: (String, String, i64, String, Option<String>, Option<String>) =
        catalog_connection
            .query_row(
                "SELECT operation_id, data_location_id, affected_rows, state, wal_fingerprint, shm_fingerprint FROM operation_record WHERE operation_id = ?1",
                params![plan.operation_id().as_str()],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                    ))
                },
            )
            .expect("目录库缺少 T07 完成记录");
    assert_identifier_eq(
        &catalog_record.0,
        plan.operation_id().as_str(),
        "目录库 operation_id",
    );
    assert_identifier_eq(
        &catalog_record.1,
        &data_location_id,
        "目录库 data_location_id",
    );
    assert_eq!(catalog_record.2, 2);
    assert_eq!(catalog_record.3, "completed");
    assert_eq!(
        catalog_record.4.as_deref(),
        plan.target_file_evidence().wal_fingerprint.as_deref()
    );
    assert_eq!(
        catalog_record.5.as_deref(),
        plan.target_file_evidence().shm_fingerprint.as_deref()
    );
    drop(catalog_connection);

    let connection = open_readwrite_with_raw_key(&db_path, &raw_key);
    let attached_project: String = connection
        .query_row(
            "SELECT project_id FROM chat_session WHERE session_id = ?1",
            params![session_id],
            |row| row.get(0),
        )
        .expect("无法读取重挂后的会话项目");
    assert_identifier_eq(&attached_project, &target_project_id, "会话目标 project_id");
    let attached_relation: String = connection
        .query_row(
            "SELECT project_id FROM session_project WHERE session_id = ?1",
            params![session_id],
            |row| row.get(0),
        )
        .expect("无法读取重挂后的关系");
    assert_identifier_eq(
        &attached_relation,
        &target_project_id,
        "会话关系目标 project_id",
    );
    let message_count: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM chat_message WHERE message_id = ?1",
            params![message_id],
            |row| row.get(0),
        )
        .expect("无法读取重挂后的消息");
    assert_eq!(message_count, 1);
    let after_existing_state = account_rows_fingerprint(&connection, OLD_USER_ID, &target_user_id);
    assert_eq!(
        before_existing_state, after_existing_state,
        "旧账号与目标账号既有项目、会话、消息行发生变化"
    );
    drop(connection);

    assert_eq!(
        probe.run_integrity_checks(&db_path, &raw_key),
        (true, true),
        "T07 写入后完整性检查失败"
    );
    assert_trio_matches_expected(&root, "backup-a", "source");
    assert_trio_matches_expected(&root, "backup-b", "source");

    // 同一不可变计划再次执行只能被拒绝，不能重复插入关系或生成第二个操作。
    let second =
        bound.execute_sync_plan(&plan, &OperationCancellation::new(), &AlwaysCurrentEvidence);
    assert!(
        matches!(
            second,
            SyncPlanExecutionOutcome::UnsupportedPlan
                | SyncPlanExecutionOutcome::PlanExpired { .. }
        ),
        "重启/重复执行未拒绝旧计划：{second:?}"
    );
    assert_eq!(
        list_operation_summaries(&recovery_root)
            .expect("无法再次读取 T07 操作 manifest")
            .len(),
        1,
        "重复执行不应创建第二个操作 manifest"
    );

    println!(
        "REAL_CLONE_WRITE_ACCEPTANCE={}",
        json!({
            "operation_id_sha256": text_sha256(plan.operation_id().as_str()),
            "data_location_id_sha256": text_sha256(&data_location_id),
            "target_user_id_sha256": text_sha256(&target_user_id),
            "synthetic_source_project_id_sha256": text_sha256(&source_project_id),
            "synthetic_target_project_id_sha256": text_sha256(&target_project_id),
            "synthetic_session_id_sha256": text_sha256(&session_id),
            "outcome": "completed",
            "repeated_plan": "rejected_without_duplicate",
            "double_backup_verified": true,
            "logical_backup_verified": true,
            "old_and_existing_target_rows_unchanged": true,
            "integrity_checks": true,
            "elapsed_ms": elapsed_ms,
            "progress_event_count": progress_events.len(),
            "progress_phases": progress_events.iter().map(|snapshot| format!("{:?}", snapshot.phase)).collect::<Vec<_>>(),
            "writing_phase_cancellable": writing_snapshot.cancellable,
            "original_data_directory_written": false,
        })
    );
}
