#![cfg(windows)]

//! T13 真实 Windows DPAPI 当前用户 Profile 与恢复包隔离集成测试。

use std::fs;
use std::path::Path;

use rusqlite::Connection;
use tempfile::tempdir;
use traesync_infrastructure::recovery_package::{
    export_recovery_package, RecoveryPackageImporter, RecoveryPayload,
};
use traesync_infrastructure::DpapiKeyWrapper;
use traesync_ports::{
    KeyWrapperPort, KeyWrapperRequest, RecoveryImportRequest, RecoveryPackageImportPort,
};

const CATALOG_KEY: &str = "deadbeefcafebabe1234567890abcdefdeadbeefcafebabe1234567890abcdef";

fn create_catalog(path: &Path) {
    let connection = Connection::open(path).expect("创建隔离目录库");
    connection
        .execute_batch(&format!(
            "PRAGMA key = \"x'{CATALOG_KEY}'\";
             CREATE TABLE catalog_meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
             CREATE TABLE history_item (id INTEGER PRIMARY KEY, value TEXT NOT NULL);
             INSERT INTO history_item(id, value) VALUES (1, 'isolated');
             INSERT INTO catalog_meta(key, value) VALUES
                 ('catalog_id', 'catalog-real-dpapi'),
                 ('key_generation', '7'),
                 ('schema_version', '1');
             PRAGMA user_version = 1;"
        ))
        .expect("初始化隔离目录库");
}

#[test]
fn verified_recovery_material_rewraps_with_current_profile_dpapi() {
    let root = tempdir().expect("创建隔离 Profile 根");
    let catalog = root.path().join("catalog.db");
    let package = root.path().join("catalog.traesync-recovery");
    let wrapper_path = root.path().join("catalog-key.dpapi");
    create_catalog(&catalog);

    export_recovery_package(
        &package,
        "correct-password",
        &RecoveryPayload {
            catalog_id: "catalog-real-dpapi".to_string(),
            key_generation: 7,
            catalog_key_hex: CATALOG_KEY.to_string(),
        },
    )
    .expect("生成隔离恢复包");

    let importer = RecoveryPackageImporter::new();
    let material = importer
        .import_and_verify(&RecoveryImportRequest {
            package_path: &package,
            catalog_path: &catalog,
            passphrase: "correct-password",
            expected_catalog_id: "catalog-real-dpapi",
            expected_key_generation: 7,
            expected_schema_version: 1,
        })
        .expect("恢复包和目录库只读验证通过");

    // 包装文件在隔离根内，但 DPAPI 使用当前 Windows 用户 Profile 的真实密钥保护。
    let wrapper = DpapiKeyWrapper::new(&wrapper_path);
    let request = KeyWrapperRequest::new(
        material.catalog_id(),
        material.key_generation(),
        material.catalog_key_hex(),
    );
    let receipt = wrapper
        .wrap_catalog_key(&request)
        .expect("当前用户 Profile DPAPI 包装通过");
    assert_eq!(receipt.wrapper_version, material.key_wrapper_version());
    assert_eq!(
        wrapper
            .unwrap_catalog_key("catalog-real-dpapi", 7)
            .expect("当前用户 Profile DPAPI 解包通过"),
        CATALOG_KEY
    );

    let wrapper_bytes = fs::read(wrapper.path()).expect("读取 DPAPI 包装文件");
    assert!(!wrapper_bytes
        .windows(CATALOG_KEY.len())
        .any(|window| window == CATALOG_KEY.as_bytes()));

    // 模拟 DPAPI 包装丢失：恢复包重新验证后才能重新生成包装文件。
    fs::remove_file(wrapper.path()).expect("删除隔离包装文件");
    assert!(!wrapper.path().exists());
    let recovered = importer
        .import_and_verify(&RecoveryImportRequest {
            package_path: &package,
            catalog_path: &catalog,
            passphrase: "correct-password",
            expected_catalog_id: "catalog-real-dpapi",
            expected_key_generation: 7,
            expected_schema_version: 1,
        })
        .expect("重新验证恢复包");
    let recovered_request = KeyWrapperRequest::new(
        recovered.catalog_id(),
        recovered.key_generation(),
        recovered.catalog_key_hex(),
    );
    wrapper
        .wrap_catalog_key(&recovered_request)
        .expect("重新生成 DPAPI 包装文件");
    assert_eq!(
        wrapper
            .unwrap_catalog_key("catalog-real-dpapi", 7)
            .expect("重新生成的包装文件可解包"),
        CATALOG_KEY
    );

    let before_wrong_identity = fs::read(wrapper.path()).expect("保存包装文件基线");
    assert!(wrapper.unwrap_catalog_key("catalog-other", 7).is_err());
    assert_eq!(
        fs::read(wrapper.path()).expect("读取错误身份后的包装文件"),
        before_wrong_identity
    );
}
