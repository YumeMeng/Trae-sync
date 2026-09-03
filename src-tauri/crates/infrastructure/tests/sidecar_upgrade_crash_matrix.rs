//! T13 旁路升级 fixture：覆盖版本矩阵、空间 guard 和 durable stage 中断。

use std::fs;
use std::path::Path;

use rusqlite::Connection;
use tempfile::{tempdir, tempdir_in, TempDir};
use traesync_infrastructure::recovery_package::upgrade_catalog_sidecar_with_lease;
use traesync_infrastructure::{sha256_file, FixturePathGuard, OperationLease};

const CATALOG_KEY: &str = "deadbeefcafebabe1234567890abcdefdeadbeefcafebabe1234567890abcdef";

fn isolated_recovery_root() -> (TempDir, std::path::PathBuf) {
    let local_appdata = std::env::var_os("LOCALAPPDATA").expect("LOCALAPPDATA 必须存在");
    let recovery_namespace = std::path::PathBuf::from(local_appdata)
        .join("Trae Sync")
        .join("recovery");
    fs::create_dir_all(&recovery_namespace).expect("创建共享恢复区命名空间");
    let directory = tempdir_in(&recovery_namespace).expect("创建隔离恢复区");
    let path = directory.path().to_path_buf();
    (directory, path)
}

fn acquire_lease(recovery_root: &Path) -> OperationLease {
    let local_appdata = std::env::var_os("LOCALAPPDATA").expect("LOCALAPPDATA 必须存在");
    let test_namespace = std::path::PathBuf::from(local_appdata)
        .join("Trae Sync")
        .join("tests");
    fs::create_dir_all(&test_namespace).expect("创建 fixture 测试根");
    let fixture_root = tempdir_in(&test_namespace).expect("创建租约验证 fixture");
    let guard = FixturePathGuard::new(fixture_root.path()).expect("创建路径守卫");
    guard
        .acquire_operation_lease(recovery_root, "catalog-upgrade")
        .expect("取得已验证共享恢复区租约")
}

fn create_catalog(path: &Path, schema_version: u32) {
    let connection = Connection::open(path).expect("create synthetic catalog");
    connection
        .execute_batch(&format!(
            "PRAGMA key = \"x'{CATALOG_KEY}'\";
             CREATE TABLE catalog_meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
             CREATE TABLE history_item (id INTEGER PRIMARY KEY, value TEXT NOT NULL);
             INSERT INTO history_item(id, value) VALUES (1, 'fixture'), (2, 'fixture-2');
             INSERT INTO catalog_meta(key, value) VALUES ('schema_version', '{schema_version}');
             PRAGMA user_version = {schema_version};"
        ))
        .expect("initialize synthetic catalog");
}

fn reservation_files(root: &Path) -> Vec<String> {
    fs::read_dir(root)
        .expect("read reservation parent")
        .filter_map(Result::ok)
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.starts_with(".space-reservation-"))
        .collect()
}

fn write_durable_stage(recovery_root: &Path, stage: &str) {
    let operation = recovery_root
        .join("migration-manifests")
        .join("fixture-operation");
    fs::create_dir_all(&operation).expect("create durable manifest fixture");
    let record = serde_json::json!({
        "format_version": 1,
        "operation_id": "fixture-operation",
        "kind": "catalog_upgrade",
        "sequence": 0,
        "stage": stage,
        "source_id": "fixture-source",
        "source_bytes": 1,
        "source_manifest_hash": "fixture-hash",
        "staging_id": "fixture-staging",
        "destination_id": null
    });
    fs::write(
        operation.join("00000000000000000000.json"),
        serde_json::to_vec(&record).expect("serialize durable manifest fixture"),
    )
    .expect("write durable manifest fixture");
}

fn latest_stage(recovery_root: &Path) -> String {
    let operation = recovery_root
        .join("migration-manifests")
        .join("fixture-operation");
    fs::read_dir(operation)
        .expect("read durable manifest records")
        .filter_map(Result::ok)
        .filter_map(|entry| serde_json::from_reader(fs::File::open(entry.path()).ok()?).ok())
        .max_by_key(|record: &serde_json::Value| record["sequence"].as_u64().unwrap_or(0))
        .and_then(|record| record["stage"].as_str().map(str::to_string))
        .expect("durable manifest has latest stage")
}

#[test]
fn sidecar_upgrade_publishes_version_matrix_and_releases_space_reservation() {
    let root = tempdir().expect("create fixture tempdir");
    let source = root.path().join("old.db");
    let destination = root.path().join("catalog");
    let (_recovery_dir, recovery) = isolated_recovery_root();
    create_catalog(&source, 1);
    let source_before = fs::read(&source).expect("snapshot old catalog");
    let source_hash = sha256_file(&source).expect("hash old catalog");

    let lease = acquire_lease(&recovery);
    let result =
        upgrade_catalog_sidecar_with_lease(&source, &destination, &lease, CATALOG_KEY, 1, 2)
            .expect("upgrade synthetic catalog");

    assert_eq!(fs::read(&source).expect("read old catalog"), source_before);
    assert_eq!(
        sha256_file(&source).expect("rehash old catalog"),
        source_hash
    );
    assert_eq!(result.schema_version, 2);
    assert_eq!(
        serde_json::from_reader::<_, serde_json::Value>(
            fs::File::open(destination.join("current.json")).expect("open current pointer")
        )
        .expect("parse current pointer")["generation_id"]
            .as_str(),
        Some(result.generation_id.as_str())
    );

    let metadata_path = destination
        .join("generations")
        .join(&result.generation_id)
        .join("generation.json");
    let metadata: serde_json::Value =
        serde_json::from_reader(fs::File::open(metadata_path).expect("open generation metadata"))
            .expect("parse generation metadata");
    assert_eq!(metadata["metadata_version"], 1);
    assert_eq!(metadata["package_format_version"], 1);
    assert_eq!(metadata["catalog_schema_version"], 2);
    assert_eq!(metadata["mapping_version"], "catalog-v1");
    assert_eq!(metadata["key_wrapper_version"], 1);
    assert!(
        reservation_files(root.path()).is_empty(),
        "upgrade must release its pre-write space reservation"
    );
}

#[test]
fn durable_non_terminal_stage_is_frozen_before_new_generation() {
    for stage in ["planned", "staging", "staged_verified", "publishing"] {
        let root = tempdir().expect("create fixture tempdir");
        let source = root.path().join("old.db");
        let destination = root.path().join("catalog");
        let (_recovery_dir, recovery) = isolated_recovery_root();
        create_catalog(&source, 1);
        let source_before = fs::read(&source).expect("snapshot old catalog");
        write_durable_stage(&recovery, stage);
        let lease = acquire_lease(&recovery);

        let result =
            upgrade_catalog_sidecar_with_lease(&source, &destination, &lease, CATALOG_KEY, 1, 2);

        assert!(matches!(
            result,
            Err(traesync_infrastructure::recovery_package::RecoveryPackageError::MigrationRecoveryRequired)
        ));
        assert!(
            !destination.exists(),
            "{stage} must not create a generation"
        );
        assert_eq!(fs::read(&source).expect("read old catalog"), source_before);
        assert_eq!(latest_stage(&recovery), "manual_recovery_required");
        assert!(reservation_files(root.path()).is_empty());
    }
}
