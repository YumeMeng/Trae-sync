//! 目录库 sidecar 的公开契约回归。
//!
//! 这里只经由基础设施公开 API 验证 fail-closed 行为，避免测试依赖实现私有细节。

use std::fs;
use std::path::{Path, PathBuf};

use rusqlite::Connection;
use tempfile::{tempdir, tempdir_in, TempDir};
use traesync_infrastructure::{
    ensure_catalog_initialized, initialize_catalog_identity, reconcile_current_catalog_sidecar,
    sha256_file, upgrade_catalog_sidecar_with_lease, verify_catalog_identity, CatalogPathError,
    FixturePathError, FixturePathGuard, OperationLease, RecoveryPackageError,
};

const CATALOG_KEY: &str = "deadbeefcafebabe1234567890abcdefdeadbeefcafebabe1234567890abcdef";
const WRITE_PROTOCOL_UPGRADE_CODE: &str = "catalog_write_protocol_upgrade_required";

/// 公开入口面对不可信目录库时的错误码约束。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExpectedCatalogFailure {
    /// 本测试只验证失败关闭与零副作用，不限制具体错误分类。
    Any,
    /// 旧 WAL 写协议只能提示旁路升级，不能泛化为恢复或初始化失败。
    WriteProtocolUpgradeRequired,
    /// 未完成事务 journal 不是可升级协议，不能误报为 WAL 升级。
    NotWriteProtocolUpgradeRequired,
}

/// 验证公开错误码保持稳定；调用方据此决定显示升级说明还是通用诊断。
fn assert_catalog_failure<T>(
    result: Result<T, CatalogPathError>,
    expected: ExpectedCatalogFailure,
    message: &str,
) {
    let error = match result {
        Ok(_) => panic!("{message}"),
        Err(error) => error,
    };
    match expected {
        ExpectedCatalogFailure::Any => {}
        ExpectedCatalogFailure::WriteProtocolUpgradeRequired => assert_eq!(
            error.code(),
            WRITE_PROTOCOL_UPGRADE_CODE,
            "{message} 必须返回稳定写协议升级错误码"
        ),
        ExpectedCatalogFailure::NotWriteProtocolUpgradeRequired => assert_ne!(
            error.code(),
            WRITE_PROTOCOL_UPGRADE_CODE,
            "{message} 不能把未完成 journal 误报为可升级 WAL 协议"
        ),
    }
}

/// 记录 SQLite 主库旁的三个可能持久化 sidecar。
fn sqlite_sidecar_artifacts(catalog_path: &Path) -> Vec<(PathBuf, Option<Vec<u8>>)> {
    let file_name = catalog_path
        .file_name()
        .and_then(|name| name.to_str())
        .expect("目录库文件名有效");
    ["-wal", "-shm", "-journal"]
        .into_iter()
        .map(|suffix| catalog_path.with_file_name(format!("{file_name}{suffix}")))
        .map(|path| {
            let bytes = match fs::read(&path) {
                Ok(bytes) => Some(bytes),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
                Err(error) => panic!("读取 SQLite sidecar 失败 {}: {error}", path.display()),
            };
            (path, bytes)
        })
        .collect()
}

/// 记录公开目录库布局中本轮 fail-closed 路径不应改变的所有持久文件。
///
/// `None` 同样是断言的一部分：只读失败路径不得遗留新的 SQLite sidecar。
fn catalog_public_artifacts(catalog_path: &Path) -> Vec<(PathBuf, Option<Vec<u8>>)> {
    let generation_dir = catalog_path.parent().expect("目录库位于代次目录");
    let catalog_root = generation_dir
        .parent()
        .and_then(Path::parent)
        .expect("代次目录位于 catalog 根下");
    let mut paths = vec![
        catalog_path.to_path_buf(),
        generation_dir.join("generation.json"),
        catalog_root.join("current.json"),
    ];
    paths.extend(
        sqlite_sidecar_artifacts(catalog_path)
            .into_iter()
            .map(|(path, _)| path),
    );
    paths
        .into_iter()
        .map(|path| {
            let bytes = match fs::read(&path) {
                Ok(bytes) => Some(bytes),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
                Err(error) => panic!("读取公开目录库工件失败 {}: {error}", path.display()),
            };
            (path, bytes)
        })
        .collect()
}

/// 同一损坏现场必须被四个公开目录库入口一致拒绝，且不能留下持久修改。
fn assert_public_reopen_paths_fail_without_persistent_mutation(
    storage_root: &Path,
    catalog_path: &Path,
    catalog_id: &str,
    operation_lease: &OperationLease,
    expected_failure: ExpectedCatalogFailure,
) {
    let before = catalog_public_artifacts(catalog_path);
    let recovery_root = operation_lease.recovery_root();

    assert_catalog_failure(
        ensure_catalog_initialized(storage_root, CATALOG_KEY, recovery_root, operation_lease),
        expected_failure,
        "启动重开不得接受不可信目录库",
    );
    assert_eq!(
        catalog_public_artifacts(catalog_path),
        before,
        "启动重开拒绝不可信目录库后不得改写持久工件"
    );

    assert_catalog_failure(
        reconcile_current_catalog_sidecar(
            storage_root,
            CATALOG_KEY,
            recovery_root,
            operation_lease,
        ),
        expected_failure,
        "sidecar 协调不得接受不可信目录库",
    );
    assert_eq!(
        catalog_public_artifacts(catalog_path),
        before,
        "sidecar 协调拒绝不可信目录库后不得遗留持久副作用"
    );

    assert_catalog_failure(
        initialize_catalog_identity(
            catalog_path,
            CATALOG_KEY,
            catalog_id,
            1,
            recovery_root,
            operation_lease,
        ),
        expected_failure,
        "身份初始化不得接受不可信目录库",
    );
    assert_eq!(
        catalog_public_artifacts(catalog_path),
        before,
        "身份初始化拒绝不可信目录库后不得遗留持久副作用"
    );

    assert_catalog_failure(
        verify_catalog_identity(
            catalog_path,
            CATALOG_KEY,
            catalog_id,
            1,
            recovery_root,
            operation_lease,
        ),
        expected_failure,
        "身份复核不得接受不可信目录库",
    );
    assert_eq!(
        catalog_public_artifacts(catalog_path),
        before,
        "身份复核拒绝不可信目录库后不得遗留持久副作用"
    );
}

/// 每个目录库公开写入口都必须复用同一已持有共享租约。
///
/// 临时目录位于 `%LOCALAPPDATA%\\Trae Sync\\tests`，恢复区位于固定 recovery
/// 命名空间；两者都是 fixture，仅用于锁和目录库回归，不读取真实 TRAE 数据。
struct CatalogFixture {
    // 字段顺序确保租约先释放，随后才清理临时目录，避免 Windows 锁句柄阻碍删除。
    lease: OperationLease,
    fixture_root: PathBuf,
    storage_root: PathBuf,
    recovery_root: PathBuf,
    _recovery_directory: TempDir,
    _fixture_directory: TempDir,
}

/// 构造受 `FixturePathGuard` 保护的目录库和共享恢复区租约。
fn catalog_fixture() -> CatalogFixture {
    let local_appdata = std::env::var_os("LOCALAPPDATA").expect("LOCALAPPDATA 必须存在");
    let namespace = PathBuf::from(local_appdata).join("Trae Sync").join("tests");
    fs::create_dir_all(&namespace).expect("创建目录库 fixture 测试根");
    let fixture_directory = tempdir_in(namespace).expect("创建目录库 fixture");
    let storage_root = fixture_directory.path().join("storage-root");
    fs::create_dir(&storage_root).expect("创建目录库存储根");
    let guard = FixturePathGuard::new(fixture_directory.path()).expect("创建路径守卫");
    let (recovery_directory, recovery_root) = isolated_recovery_root();
    let lease = guard
        .acquire_catalog_operation_lease(&recovery_root, &storage_root, "catalog-sidecar")
        .expect("取得目录库共享租约");

    CatalogFixture {
        lease,
        fixture_root: fixture_directory.path().to_path_buf(),
        storage_root,
        recovery_root,
        _recovery_directory: recovery_directory,
        _fixture_directory: fixture_directory,
    }
}

/// 第二写租约必须在目录库锁层被拒绝，测试不能要求 `OperationLease` 实现 `Debug`。
fn assert_second_lease_is_rejected(result: Result<OperationLease, FixturePathError>) {
    let error = match result {
        Ok(_) => panic!("已有目录库租约时不得取得第二个写租约"),
        Err(error) => error,
    };
    assert!(matches!(
        error,
        FixturePathError::OperationLeaseUnavailable { ref source } if source == "目录库锁忙"
    ));
}

/// 构造具备身份字段的公开目录库 fixture。
fn initialized_catalog() -> (CatalogFixture, PathBuf) {
    let fixture = catalog_fixture();
    let catalog_path = ensure_catalog_initialized(
        &fixture.storage_root,
        CATALOG_KEY,
        &fixture.recovery_root,
        &fixture.lease,
    )
    .expect("初始化 fixture 目录库");
    initialize_catalog_identity(
        &catalog_path,
        CATALOG_KEY,
        "fixture-catalog",
        1,
        &fixture.recovery_root,
        &fixture.lease,
    )
    .expect("写入 fixture 目录库身份");
    (fixture, catalog_path)
}

/// 将已初始化目录库持久切换为 WAL，并删除瞬态 sidecar。
///
/// SQLite 把 journal mode 保存于数据库头；所以仅检查 `-wal/-shm` 文件不足以确认
/// 目录库仍遵守 DELETE/FULL 写入协议。
fn persist_wal_without_sidecars(catalog_path: &Path) {
    let connection = Connection::open(catalog_path).expect("打开 fixture 目录库");
    connection
        .execute_batch(&format!(
            "PRAGMA key = \"x'{CATALOG_KEY}'\"; PRAGMA journal_mode = WAL;"
        ))
        .expect("将 fixture 目录库切换为 WAL");
    let journal_mode: String = connection
        .query_row("PRAGMA journal_mode", [], |row| row.get(0))
        .expect("读取 fixture journal mode");
    assert_eq!(journal_mode.to_ascii_lowercase(), "wal");
    drop(connection);

    let file_name = catalog_path
        .file_name()
        .and_then(|name| name.to_str())
        .expect("目录库文件名有效");
    for suffix in ["-wal", "-shm", "-journal"] {
        let sidecar = catalog_path.with_file_name(format!("{file_name}{suffix}"));
        if sidecar.exists() {
            fs::remove_file(&sidecar).expect("清理 fixture SQLite sidecar");
        }
        assert!(
            !sidecar.exists(),
            "fixture 必须不留 {suffix} 文件，才能覆盖持久 WAL 但外观干净的场景"
        );
    }
}

/// 模拟旧版本已在 WAL 模式下完整发布 sidecar 的情况。
///
/// 若只依赖哈希失配触发拒绝，更新为一致的哈希后会错误放行，因此必须继续检查
/// 数据库头持久化的 journal mode。
fn make_sidecar_match_wal_catalog(catalog_path: &Path) {
    let metadata_path = catalog_path
        .parent()
        .expect("目录库位于代次目录")
        .join("generation.json");
    let mut metadata: serde_json::Value =
        serde_json::from_reader(fs::File::open(&metadata_path).expect("读取 generation sidecar"))
            .expect("解析 generation sidecar");
    metadata["bytes"] = serde_json::json!(fs::metadata(catalog_path)
        .expect("读取 WAL fixture 字节数")
        .len());
    metadata["catalog_sha256"] =
        serde_json::json!(sha256_file(catalog_path).expect("计算 WAL fixture 哈希"));
    fs::write(
        metadata_path,
        serde_json::to_vec_pretty(&metadata).expect("序列化一致 sidecar"),
    )
    .expect("发布一致 sidecar");
}

/// 旁路升级使用固定共享恢复区租约；测试目录在该根下自动清理，不触及用户历史。
fn isolated_recovery_root() -> (TempDir, PathBuf) {
    let local_appdata = std::env::var_os("LOCALAPPDATA").expect("LOCALAPPDATA 必须存在");
    let namespace = PathBuf::from(local_appdata)
        .join("Trae Sync")
        .join("recovery");
    fs::create_dir_all(&namespace).expect("创建隔离恢复区命名空间");
    let directory = tempdir_in(namespace).expect("创建隔离恢复区");
    let path = directory.path().to_path_buf();
    (directory, path)
}

/// 构造最小已版本化目录库，供旁路升级入口验证其读取前置条件。
fn create_upgrade_source_catalog(path: &Path) {
    let connection = Connection::open(path).expect("创建升级源目录库");
    connection
        .execute_batch(&format!(
            "PRAGMA key = \"x'{CATALOG_KEY}'\";
             PRAGMA journal_mode = DELETE;
             CREATE TABLE catalog_meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
             CREATE TABLE history_item (id INTEGER PRIMARY KEY, value TEXT NOT NULL);
             INSERT INTO history_item(id, value) VALUES (1, 'fixture');
             INSERT INTO catalog_meta(key, value) VALUES ('schema_version', '1');
             INSERT INTO catalog_meta(key, value) VALUES ('content_revision', '1');
             PRAGMA user_version = 1;"
        ))
        .expect("初始化升级源目录库");
}

/// 取得只服务于 fixture 的共享租约，保持对真实恢复区和真实 TRAE 数据零访问。
fn acquire_fixture_lease(
    recovery_root: &Path,
) -> (TempDir, traesync_infrastructure::OperationLease) {
    let local_appdata = std::env::var_os("LOCALAPPDATA").expect("LOCALAPPDATA 必须存在");
    let namespace = PathBuf::from(local_appdata).join("Trae Sync").join("tests");
    fs::create_dir_all(&namespace).expect("创建 fixture 测试根命名空间");
    let fixture_root = tempdir_in(namespace).expect("创建 fixture 根");
    let guard = FixturePathGuard::new(fixture_root.path()).expect("创建路径守卫");
    let lease = guard
        .acquire_operation_lease(recovery_root, "catalog-upgrade")
        .expect("取得 fixture 共享租约");
    (fixture_root, lease)
}

#[test]
fn persistent_wal_without_filesystem_sidecars_fails_closed_on_all_public_reopen_paths() {
    let (fixture, catalog_path) = initialized_catalog();
    persist_wal_without_sidecars(&catalog_path);
    make_sidecar_match_wal_catalog(&catalog_path);
    assert_public_reopen_paths_fail_without_persistent_mutation(
        &fixture.storage_root,
        &catalog_path,
        "fixture-catalog",
        &fixture.lease,
        ExpectedCatalogFailure::WriteProtocolUpgradeRequired,
    );
}

#[test]
fn sqlite_sidecars_fail_closed_without_persistent_mutation_on_all_public_reopen_paths() {
    for suffix in ["-wal", "-shm", "-journal"] {
        let (fixture, catalog_path) = initialized_catalog();
        let file_name = catalog_path
            .file_name()
            .and_then(|name| name.to_str())
            .expect("目录库文件名有效");
        fs::write(
            catalog_path.with_file_name(format!("{file_name}{suffix}")),
            b"active-sidecar-fixture",
        )
        .expect("写入活动 SQLite sidecar fixture");

        let expected_failure = match suffix {
            "-wal" | "-shm" => ExpectedCatalogFailure::WriteProtocolUpgradeRequired,
            "-journal" => ExpectedCatalogFailure::NotWriteProtocolUpgradeRequired,
            _ => unreachable!("测试仅覆盖 SQLite 标准 sidecar"),
        };
        assert_public_reopen_paths_fail_without_persistent_mutation(
            &fixture.storage_root,
            &catalog_path,
            "fixture-catalog",
            &fixture.lease,
            expected_failure,
        );
    }
}

#[test]
fn missing_generation_sidecar_fails_closed_without_persistent_mutation() {
    let (fixture, catalog_path) = initialized_catalog();
    fs::remove_file(
        catalog_path
            .parent()
            .expect("目录库位于代次目录")
            .join("generation.json"),
    )
    .expect("删除 generation sidecar fixture");

    assert_public_reopen_paths_fail_without_persistent_mutation(
        &fixture.storage_root,
        &catalog_path,
        "fixture-catalog",
        &fixture.lease,
        ExpectedCatalogFailure::Any,
    );
}

#[test]
fn unknown_generation_field_fails_closed_without_persistent_mutation() {
    let (fixture, catalog_path) = initialized_catalog();
    let metadata_path = catalog_path
        .parent()
        .expect("目录库位于代次目录")
        .join("generation.json");
    let mut metadata: serde_json::Value =
        serde_json::from_reader(fs::File::open(&metadata_path).expect("读取 generation sidecar"))
            .expect("解析 generation sidecar");
    metadata["unknown_field"] = serde_json::json!(true);
    fs::write(
        metadata_path,
        serde_json::to_vec_pretty(&metadata).expect("序列化未知字段 fixture"),
    )
    .expect("写入未知字段 fixture");

    assert_public_reopen_paths_fail_without_persistent_mutation(
        &fixture.storage_root,
        &catalog_path,
        "fixture-catalog",
        &fixture.lease,
        ExpectedCatalogFailure::Any,
    );
}

#[test]
fn same_revision_drift_fails_closed_without_persistent_mutation() {
    let (fixture, catalog_path) = initialized_catalog();
    let metadata_path = catalog_path
        .parent()
        .expect("目录库位于代次目录")
        .join("generation.json");
    let mut metadata: serde_json::Value =
        serde_json::from_reader(fs::File::open(&metadata_path).expect("读取 generation sidecar"))
            .expect("解析 generation sidecar");
    metadata["catalog_sha256"] = serde_json::json!("same-revision-drift");
    fs::write(
        metadata_path,
        serde_json::to_vec_pretty(&metadata).expect("序列化同 revision 漂移 fixture"),
    )
    .expect("写入同 revision 漂移 fixture");

    assert_public_reopen_paths_fail_without_persistent_mutation(
        &fixture.storage_root,
        &catalog_path,
        "fixture-catalog",
        &fixture.lease,
        ExpectedCatalogFailure::Any,
    );
}

#[test]
fn four_public_catalog_entrypoints_accept_one_held_lease() {
    let fixture = catalog_fixture();
    let catalog_path = ensure_catalog_initialized(
        &fixture.storage_root,
        CATALOG_KEY,
        &fixture.recovery_root,
        &fixture.lease,
    )
    .expect("初始化目录库必须接受已持有租约");
    initialize_catalog_identity(
        &catalog_path,
        CATALOG_KEY,
        "fixture-catalog",
        1,
        &fixture.recovery_root,
        &fixture.lease,
    )
    .expect("身份初始化必须接受同一租约");
    verify_catalog_identity(
        &catalog_path,
        CATALOG_KEY,
        "fixture-catalog",
        1,
        &fixture.recovery_root,
        &fixture.lease,
    )
    .expect("身份复核必须接受同一租约");
    assert_eq!(
        reconcile_current_catalog_sidecar(
            &fixture.storage_root,
            CATALOG_KEY,
            &fixture.recovery_root,
            &fixture.lease,
        )
        .expect("sidecar 协调必须接受同一租约"),
        catalog_path
    );
}

#[test]
fn second_catalog_lease_is_rejected_without_catalog_or_sidecar_mutation() {
    let fixture = catalog_fixture();
    assert!(
        !fixture.storage_root.join("catalog").exists(),
        "尚未初始化时 fixture 不得已有目录库工件"
    );

    let second_guard = FixturePathGuard::new(&fixture.fixture_root).expect("创建第二路径守卫");
    assert_second_lease_is_rejected(
        second_guard.acquire_operation_lease(&fixture.recovery_root, "catalog-sidecar-second"),
    );
    assert!(
        !fixture.storage_root.join("catalog").exists(),
        "第二租约抢占失败不得创建目录库代次或 SQLite sidecar"
    );
}

#[test]
fn second_catalog_lease_is_rejected_without_initialized_catalog_mutation() {
    let (fixture, catalog_path) = initialized_catalog();
    let before = catalog_public_artifacts(&catalog_path);

    let second_guard = FixturePathGuard::new(&fixture.fixture_root).expect("创建第二路径守卫");
    assert_second_lease_is_rejected(
        second_guard.acquire_operation_lease(&fixture.recovery_root, "catalog-sidecar-second"),
    );
    assert_eq!(
        catalog_public_artifacts(&catalog_path),
        before,
        "第二租约抢占失败不得创建目录库代次、改写 current.json 或遗留 SQLite sidecar"
    );
}

#[test]
fn unbound_catalog_lease_is_rejected_before_creating_catalog_artifacts() {
    let fixture = catalog_fixture();
    // 释放 fixture 初始租约后，重新取得普通（未绑定 storage root）租约。
    drop(fixture.lease);
    let guard = FixturePathGuard::new(&fixture.fixture_root).expect("创建路径守卫");
    let unbound = guard
        .acquire_operation_lease(&fixture.recovery_root, "catalog-unbound")
        .expect("普通租约本身可以取得，但不应被目录库入口接受");

    let result = ensure_catalog_initialized(
        &fixture.storage_root,
        CATALOG_KEY,
        unbound.recovery_root(),
        &unbound,
    );
    assert_eq!(
        result,
        Err(CatalogPathError::LeaseContextMismatch),
        "未绑定 storage root 的普通租约必须在目录库入口前被拒绝"
    );
    assert!(
        !fixture.storage_root.join("catalog").exists(),
        "租约上下文拒绝不得创建目录库或 current 指针"
    );
}

#[test]
fn catalog_lease_bound_to_other_storage_root_is_rejected_without_side_effects() {
    let fixture = catalog_fixture();
    let other_storage_root = fixture.fixture_root.join("other-storage-root");
    fs::create_dir(&other_storage_root).expect("创建第二存储根");

    let result = ensure_catalog_initialized(
        &other_storage_root,
        CATALOG_KEY,
        &fixture.recovery_root,
        &fixture.lease,
    );
    assert_eq!(
        result,
        Err(CatalogPathError::LeaseContextMismatch),
        "绑定到另一 storage root 的租约必须被拒绝"
    );
    assert!(
        !other_storage_root.join("catalog").exists(),
        "错误 storage root 拒绝不得创建目录库"
    );
}

#[test]
fn catalog_entrypoints_reject_mismatched_recovery_root_without_side_effects() {
    let (fixture, catalog_path) = initialized_catalog();
    let (_wrong_recovery_directory, wrong_recovery_root) = isolated_recovery_root();
    let before = catalog_public_artifacts(&catalog_path);

    assert_eq!(
        ensure_catalog_initialized(
            &fixture.storage_root,
            CATALOG_KEY,
            &wrong_recovery_root,
            &fixture.lease,
        ),
        Err(CatalogPathError::LeaseContextMismatch),
        "启动重开必须拒绝不同恢复区的租约"
    );
    assert_eq!(catalog_public_artifacts(&catalog_path), before);

    assert_eq!(
        reconcile_current_catalog_sidecar(
            &fixture.storage_root,
            CATALOG_KEY,
            &wrong_recovery_root,
            &fixture.lease,
        ),
        Err(CatalogPathError::LeaseContextMismatch),
        "sidecar 协调必须拒绝不同恢复区的租约"
    );
    assert_eq!(catalog_public_artifacts(&catalog_path), before);

    assert_eq!(
        initialize_catalog_identity(
            &catalog_path,
            CATALOG_KEY,
            "fixture-catalog",
            1,
            &wrong_recovery_root,
            &fixture.lease,
        ),
        Err(CatalogPathError::LeaseContextMismatch),
        "身份初始化必须拒绝不同恢复区的租约"
    );
    assert_eq!(catalog_public_artifacts(&catalog_path), before);

    assert_eq!(
        verify_catalog_identity(
            &catalog_path,
            CATALOG_KEY,
            "fixture-catalog",
            1,
            &wrong_recovery_root,
            &fixture.lease,
        ),
        Err(CatalogPathError::LeaseContextMismatch),
        "身份复核必须拒绝不同恢复区的租约"
    );
    assert_eq!(catalog_public_artifacts(&catalog_path), before);
}

#[test]
fn sidecar_upgrade_rejects_persistent_wal_before_creating_staging_or_manifest() {
    let root = tempdir().expect("创建升级 fixture 根");
    let source = root.path().join("old.db");
    let destination = root.path().join("catalog");
    create_upgrade_source_catalog(&source);
    persist_wal_without_sidecars(&source);
    let source_before = fs::read(&source).expect("记录升级源目录库");
    let source_sidecars_before = sqlite_sidecar_artifacts(&source);
    let (_recovery_directory, recovery_root) = isolated_recovery_root();
    let (_fixture_directory, lease) = acquire_fixture_lease(&recovery_root);
    let migration_manifests = recovery_root.join("migration-manifests");
    assert!(
        !migration_manifests.exists(),
        "预检前 fixture 恢复区不得已有迁移 manifest"
    );

    let result =
        upgrade_catalog_sidecar_with_lease(&source, &destination, &lease, CATALOG_KEY, 1, 2);

    assert!(matches!(
        result,
        Err(RecoveryPackageError::VerificationFailed)
    ));
    assert!(
        !destination.exists(),
        "持久 WAL 必须在创建 staging、指针或迁移 manifest 前被拒绝"
    );
    assert_eq!(
        fs::read(&source).expect("读取升级源目录库"),
        source_before,
        "拒绝路径不得改写升级源目录库"
    );
    assert_eq!(
        sqlite_sidecar_artifacts(&source),
        source_sidecars_before,
        "WAL 源库只读预检不得遗留 SQLite sidecar"
    );
    assert!(
        !migration_manifests.exists(),
        "持久 WAL 预检失败不得创建迁移 manifest"
    );
}
