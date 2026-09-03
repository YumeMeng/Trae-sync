//! T11 SQLCipher 长任务 fixture：只使用受控合成目录库，不访问真实 TRAE。

use std::fs;
use std::path::Path;

use rusqlite::{params, Connection};
use tempfile::TempDir;
use traesync_infrastructure::sha256_file;
use traesync_infrastructure::sqlcipher::SqlCipherProbe;
use traesync_ports::DatabaseProbePort;

const CATALOG_KEY: &str = "deadbeefcafebabe1234567890abcdefdeadbeefcafebabe1234567890abcdef";
const PAYLOAD_BYTES: i64 = (8 * 1024 * 1024) + 17;

fn create_synthetic_catalog(path: &Path) {
    let connection = Connection::open(path).expect("创建合成 SQLCipher 目录库失败");
    connection
        .execute_batch(&format!(
            "PRAGMA key = \"x'{CATALOG_KEY}'\";
             PRAGMA journal_mode = DELETE;
             CREATE TABLE history_blob (id INTEGER PRIMARY KEY, payload BLOB NOT NULL);"
        ))
        .expect("初始化合成 SQLCipher 目录库失败");

    // 使用 SQLite 的 zeroblob 在磁盘中构造多页记录，避免测试进程一次性分配完整正文。
    connection
        .execute(
            "INSERT INTO history_blob(id, payload) VALUES (1, zeroblob(?1))",
            params![PAYLOAD_BYTES],
        )
        .expect("写入合成大记录失败");
}

fn read_payload_bytes(path: &Path) -> i64 {
    let connection = Connection::open(path).expect("打开逻辑副本失败");
    connection
        .execute_batch(&format!("PRAGMA key = \"x'{CATALOG_KEY}'\";"))
        .expect("设置逻辑副本 SQLCipher 密钥失败");
    connection
        .query_row(
            "SELECT length(payload) FROM history_blob WHERE id = 1",
            [],
            |row| row.get(0),
        )
        .expect("读取逻辑副本合成记录失败")
}

#[test]
fn t11_default_sqlcipher_exports_synthetic_large_catalog_without_mutating_source() {
    let root = TempDir::new().expect("创建 T11 SQLCipher fixture 根失败");
    let root_path = root.path().to_path_buf();
    let source = root.path().join("synthetic.db");
    create_synthetic_catalog(&source);
    let source_hash = sha256_file(&source).expect("计算源目录库哈希失败");
    let source_bytes = fs::metadata(&source).expect("读取源目录库大小失败").len();

    let probe = SqlCipherProbe::new();
    let logical_copy = probe
        .backup_to_logical_copy(&source, CATALOG_KEY)
        .expect("默认 SQLCipher 应导出合成逻辑副本");

    assert!(
        logical_copy.starts_with(root.path()),
        "逻辑副本不能越出 fixture 根"
    );
    assert_eq!(
        sha256_file(&source).expect("复核源目录库哈希失败"),
        source_hash,
        "SQLCipher 导出不得改写源目录库"
    );
    assert_eq!(read_payload_bytes(&logical_copy), PAYLOAD_BYTES);
    assert!(
        fs::metadata(&logical_copy)
            .expect("读取逻辑副本大小失败")
            .len()
            >= source_bytes / 2,
        "逻辑副本不应丢失多页合成记录"
    );

    // 导出后的新只读连接必须通过 SQLCipher 和 SQLite 两层完整性检查。
    assert_eq!(
        probe.run_integrity_checks(&logical_copy, CATALOG_KEY),
        (true, true)
    );

    drop(root);
    assert!(
        !root_path.exists(),
        "T11 SQLCipher fixture 临时目录必须清理"
    );
}
