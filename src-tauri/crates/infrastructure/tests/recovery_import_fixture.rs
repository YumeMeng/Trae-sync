//! T13 恢复导入 fixture：只使用合成 SQLCipher 目录库，不访问真实 TRAE。

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use rusqlite::Connection;
use tempfile::tempdir;
use traesync_infrastructure::recovery_package::{
    export_recovery_package, import_recovery_package_verified, RecoveryPackageImporter,
    RecoveryPayload,
};
use traesync_ports::{RecoveryImportError, RecoveryImportRequest, RecoveryPackageImportPort};

const CATALOG_KEY: &str = "deadbeefcafebabe1234567890abcdefdeadbeefcafebabe1234567890abcdef";

fn create_catalog(path: &Path, catalog_id: &str, key_generation: u32) {
    let connection = Connection::open(path).expect("create synthetic catalog");
    connection
        .execute_batch(&format!(
            "PRAGMA key = \"x'{CATALOG_KEY}'\";
             CREATE TABLE catalog_meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
             CREATE TABLE history_item (id INTEGER PRIMARY KEY, value TEXT NOT NULL);
             INSERT INTO history_item(id, value) VALUES (1, 'fixture');
             INSERT INTO catalog_meta(key, value) VALUES
                 ('catalog_id', '{catalog_id}'),
                 ('key_generation', '{key_generation}'),
                 ('schema_version', '1');
             PRAGMA user_version = 1;"
        ))
        .expect("initialize synthetic catalog");
}

fn payload(catalog_id: &str, key_generation: u32) -> RecoveryPayload {
    RecoveryPayload {
        catalog_id: catalog_id.to_string(),
        key_generation,
        catalog_key_hex: CATALOG_KEY.to_string(),
    }
}

fn request<'a>(
    package_path: &'a Path,
    catalog_path: &'a Path,
    passphrase: &'a str,
    expected_catalog_id: &'a str,
) -> RecoveryImportRequest<'a> {
    RecoveryImportRequest {
        package_path,
        catalog_path,
        passphrase,
        expected_catalog_id,
        expected_key_generation: 7,
        expected_schema_version: 1,
    }
}

fn snapshot_files(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fs::read_dir(root)
        .expect("read fixture root")
        .map(|entry| {
            let path = entry.expect("read fixture entry").path();
            (path.clone(), fs::read(path).expect("read fixture file"))
        })
        .collect()
}

#[test]
fn verified_import_calls_wrapper_after_readonly_catalog_validation() {
    let root = tempdir().expect("create fixture tempdir");
    let catalog = root.path().join("catalog.db");
    let package = root.path().join("catalog.traesync-recovery");
    create_catalog(&catalog, "catalog-a", 7);
    export_recovery_package(&package, "correct-password", &payload("catalog-a", 7))
        .expect("export synthetic recovery package");
    let before = snapshot_files(root.path());

    let verified =
        import_recovery_package_verified(&package, "correct-password", &catalog, "catalog-a", 7, 1)
            .expect("direct verified import should succeed");
    assert_eq!(verified.metadata.metadata_version, 1);
    assert_eq!(verified.metadata.package_format_version, 1);
    assert_eq!(verified.metadata.kdf_version, 1);
    assert_eq!(verified.metadata.cipher_version, "aes-256-gcm-v1");
    assert_eq!(verified.metadata.mapping_version, "catalog-v1");
    assert_eq!(verified.metadata.key_wrapper_version, 1);

    let importer = RecoveryPackageImporter::new();
    let material = importer
        .import_and_verify(&request(
            &package,
            &catalog,
            "correct-password",
            "catalog-a",
        ))
        .expect("verified import should succeed");

    assert_eq!(material.catalog_id(), "catalog-a");
    assert_eq!(material.key_generation(), 7);
    assert_eq!(material.schema_version(), 1);
    assert_eq!(material.key_wrapper_version(), 1);
    assert_eq!(material.catalog_key_hex(), CATALOG_KEY);
    assert_eq!(snapshot_files(root.path()), before);
}

#[test]
fn wrong_password_corruption_and_identity_mismatch_have_zero_side_effects() {
    let cases = [
        (
            "wrong-password",
            "catalog-a",
            "catalog-a",
            false,
            RecoveryImportError::WrongPassphrase,
        ),
        (
            "correct-password",
            "catalog-a",
            "catalog-a",
            true,
            RecoveryImportError::WrongPassphrase,
        ),
        (
            "correct-password",
            "catalog-other",
            "catalog-a",
            false,
            RecoveryImportError::IdentityMismatch,
        ),
    ];

    for (passphrase, catalog_id, expected_id, corrupt, expected_error) in cases {
        let root = tempdir().expect("create fixture tempdir");
        let catalog = root.path().join("catalog.db");
        let package = root.path().join("catalog.traesync-recovery");
        create_catalog(&catalog, catalog_id, 7);
        export_recovery_package(&package, "correct-password", &payload("catalog-a", 7))
            .expect("export synthetic recovery package");
        if corrupt {
            let mut bytes = fs::read(&package).expect("read package for corruption fixture");
            let last = bytes.len() - 1;
            bytes[last] ^= 0x01;
            fs::write(&package, bytes).expect("write corruption fixture");
        }
        let before = snapshot_files(root.path());

        let importer = RecoveryPackageImporter::new();
        let result =
            importer.import_and_verify(&request(&package, &catalog, passphrase, expected_id));

        assert_eq!(result, Err(expected_error));
        assert_eq!(snapshot_files(root.path()), before);
    }
}

#[test]
fn catalog_identity_mismatch_after_readonly_open_never_calls_wrapper() {
    let root = tempdir().expect("create fixture tempdir");
    let catalog = root.path().join("catalog.db");
    let package = root.path().join("catalog.traesync-recovery");
    create_catalog(&catalog, "catalog-other", 7);
    export_recovery_package(&package, "correct-password", &payload("catalog-a", 7))
        .expect("export synthetic recovery package");
    let before = snapshot_files(root.path());

    let importer = RecoveryPackageImporter::new();
    let result = importer.import_and_verify(&request(
        &package,
        &catalog,
        "correct-password",
        "catalog-a",
    ));

    assert_eq!(result, Err(RecoveryImportError::IdentityMismatch));
    assert_eq!(snapshot_files(root.path()), before);
}

#[test]
fn oversized_recovery_package_is_rejected_before_reading_and_without_side_effects() {
    let root = tempdir().expect("create fixture tempdir");
    let catalog = root.path().join("catalog.db");
    let package = root.path().join("oversized.traesync-recovery");
    create_catalog(&catalog, "catalog-a", 7);

    // 使用稀疏文件模拟异常大包；导入器应先检查文件长度，不把正文读入内存。
    let file = fs::File::create(&package).expect("create oversized package fixture");
    file.set_len(1024 * 1024 + 1)
        .expect("size oversized package fixture");
    let before = snapshot_files(root.path());

    let importer = RecoveryPackageImporter::new();
    let result = importer.import_and_verify(&request(
        &package,
        &catalog,
        "correct-password",
        "catalog-a",
    ));

    assert_eq!(result, Err(RecoveryImportError::InvalidPackage));
    assert_eq!(snapshot_files(root.path()), before);
}
