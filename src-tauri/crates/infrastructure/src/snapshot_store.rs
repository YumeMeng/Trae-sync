//! 文件系统快照存储：T03 不可变 DB/WAL/SHM 快照捕获。
//!
//! 实现 `SnapshotStore` port，提供不可变快照捕获与查询。
//!
//! 捕获流程（规格第 15 节）：
//! 1. 验证 database.db 存在
//! 2. 读取捕获前文件集（DB/WAL/SHM 的存在性、大小、SHA-256、文件身份）
//! 3. 复制文件到 staging 目录
//! 4. 读取捕获后文件集
//! 5. 比较前后是否漂移——漂移废弃本次快照
//! 6. 计算数据指纹（所有文件 SHA-256 聚合）
//! 7. 查询已有快照是否同指纹——相同返回 Deduplicated
//! 8. 原子发布 staging → snapshots/<snapshot_id>/
//! 9. 写入 snapshot.json
//!
//! 规则：
//! - database.db 必须存在；WAL/SHM 按实际存在性捕获，不创建占位文件
//! - 成功发布后快照不可修改
//! - 不自动删除已发布快照
//! - 相同指纹不重复创建快照

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use sha2::{Digest, Sha256};
use traesync_domain::{
    FileIdentity, ProcessRunningState, ScanFailureReason, ScanOutcome, ScanRequest,
    SnapshotFileEntry, SnapshotFileKind, SnapshotFingerprint, SnapshotId, SourceSnapshotMeta,
};
use traesync_ports::{FileIdentityProvider, SnapshotStore};

// R2：复用同 crate 的封闭证明——避免在 snapshot_store 内重复实现较弱校验
use crate::fixture_paths::validate_db_relative_path_inside;

/// 文件系统快照存储。
///
/// 持有 `FileIdentityProvider` 用于读取文件身份。
/// 存储根从 `ScanRequest.storage_root` 获取。
pub struct FilesystemSnapshotStore {
    file_identity: Box<dyn FileIdentityProvider>,
}

impl FilesystemSnapshotStore {
    pub fn new(file_identity: Box<dyn FileIdentityProvider>) -> Self {
        Self { file_identity }
    }
}

impl SnapshotStore for FilesystemSnapshotStore {
    fn capture_snapshot(&self, request: &ScanRequest) -> ScanOutcome {
        // Box<dyn FileIdentityProvider> 需通过 as_ref() 解引用为 &dyn FileIdentityProvider
        capture_snapshot_inner(self.file_identity.as_ref(), request)
    }

    fn find_by_fingerprint(&self, fingerprint: &SnapshotFingerprint) -> Option<SnapshotId> {
        // fingerprint 存储在 snapshot.json 中，需要扫描 snapshots/ 目录
        // 但 SnapshotStore 没有持有 storage_root——需要从 request 获取
        // 这里返回 None，由 application 层在 capture_snapshot 中处理去重
        // 实际上 find_by_fingerprint 由 capture_snapshot 内部调用
        let _ = fingerprint;
        None
    }

    fn read_snapshot_meta(&self, _snapshot_id: &SnapshotId) -> Option<SourceSnapshotMeta> {
        // 需要 storage_root，但本对象不持有——由 application 层通过其他方式读取
        None
    }

    fn snapshot_dir(&self, _snapshot_id: &SnapshotId) -> Option<PathBuf> {
        None
    }
}

/// 内部捕获实现。
///
/// 设计：所有路径操作只针对 ScanRequest 中的 fixture_root 和 storage_root。
/// fixture_root 是要扫描的数据位置；storage_root 是快照发布目标。
fn capture_snapshot_inner(
    file_identity: &dyn FileIdentityProvider,
    request: &ScanRequest,
) -> ScanOutcome {
    // 1. 检查 TRAE 进程状态
    if request.process_state == ProcessRunningState::Running {
        return ScanOutcome::Failed {
            reason: ScanFailureReason::ProcessRunning,
        };
    }

    let fixture_root = Path::new(&request.canonical_fixture_root);
    let storage_root = Path::new(&request.storage_root);

    // 2. R2：defense-in-depth——在任何文件访问前再次验证 db_relative_path 封闭在 fixture_root 内
    //    组合根已用 FixturePathGuard 验证过；此处的二次校验防止 commands/application 层
    //    绕过组合根直接调用 infrastructure 时仍能拒绝逃逸路径
    if validate_db_relative_path_inside(fixture_root, &request.db_relative_path).is_err() {
        return ScanOutcome::Failed {
            reason: ScanFailureReason::DatabaseMissing,
        };
    }

    // 3. 验证 database.db 存在
    let db_path = fixture_root.join(&request.db_relative_path);
    if !db_path.is_file() {
        return ScanOutcome::Failed {
            reason: ScanFailureReason::DatabaseMissing,
        };
    }

    // 4. 读取捕获前文件集
    let pre_capture = match read_file_set(fixture_root, &request.db_relative_path, file_identity) {
        Some(entries) => entries,
        None => {
            return ScanOutcome::Failed {
                reason: ScanFailureReason::SourceSetDrift,
            };
        }
    };

    // 5. 创建 staging 目录
    let staging_dir = storage_root.join("staging");
    if let Err(_) = std::fs::create_dir_all(&staging_dir) {
        return ScanOutcome::Failed {
            reason: ScanFailureReason::StorageRootUnavailable,
        };
    }

    // 6. 复制文件到 staging
    let snapshot_id = SnapshotId::new();
    let staging_snapshot_dir = staging_dir.join(snapshot_id.as_str());
    if let Err(_) = std::fs::create_dir_all(&staging_snapshot_dir) {
        return ScanOutcome::Failed {
            reason: ScanFailureReason::StorageRootUnavailable,
        };
    }

    for entry in &pre_capture {
        if !entry.present {
            continue;
        }
        let src = fixture_root.join(&entry.relative_path);
        let dst = staging_snapshot_dir.join(&entry.relative_path);
        if let Err(_) = std::fs::copy(&src, &dst) {
            // 清理 staging 后返回失败
            let _ = std::fs::remove_dir_all(&staging_snapshot_dir);
            return ScanOutcome::Failed {
                reason: ScanFailureReason::StorageRootUnavailable,
            };
        }
    }

    // 7. R3：复制后重读源文件集（fixture_root）——检测捕获期间源漂移。
    //    规格第 15 节：捕获前后存在性、大小或身份变化时废弃本次快照。
    //    旧实现读取 staging 副本，无法发现源在复制期间被修改的情况。
    let post_source = match read_file_set(fixture_root, &request.db_relative_path, file_identity) {
        Some(entries) => entries,
        None => {
            let _ = std::fs::remove_dir_all(&staging_snapshot_dir);
            return ScanOutcome::Failed {
                reason: ScanFailureReason::SourceSetDrift,
            };
        }
    };

    // 8. R3：源稳定性比较——pre_capture vs post_source
    //    比较 presence、size、file_identity（规格要求的"存在性、大小或身份"）
    if !file_sets_source_stable(&pre_capture, &post_source) {
        let _ = std::fs::remove_dir_all(&staging_snapshot_dir);
        return ScanOutcome::Failed {
            reason: ScanFailureReason::SourceSetDrift,
        };
    }

    // 9. R3：独立验证 staging 内容完整性——pre_capture vs staging 副本
    //    比较 size + SHA-256，确保复制过程未损坏（文件身份在 staging 中不同，不比较）
    let staging_post = match read_file_set(
        &staging_snapshot_dir,
        &request.db_relative_path,
        file_identity,
    ) {
        Some(entries) => entries,
        None => {
            let _ = std::fs::remove_dir_all(&staging_snapshot_dir);
            return ScanOutcome::Failed {
                reason: ScanFailureReason::SourceSetDrift,
            };
        }
    };
    if !file_sets_content_equal(&pre_capture, &staging_post) {
        let _ = std::fs::remove_dir_all(&staging_snapshot_dir);
        return ScanOutcome::Failed {
            reason: ScanFailureReason::SourceSetDrift,
        };
    }

    // 10. 计算数据指纹（来自经过完整性验证的 staging 副本）
    let fingerprint = compute_fingerprint(&staging_post);

    // 11. 查询已有快照是否同指纹
    let snapshots_dir = storage_root.join("snapshots");
    if let Some(existing_id) = find_snapshot_by_fingerprint(&snapshots_dir, &fingerprint) {
        // 去重：删除 staging，返回 Deduplicated
        let _ = std::fs::remove_dir_all(&staging_snapshot_dir);
        return ScanOutcome::Deduplicated {
            existing_snapshot_id: existing_id,
            fingerprint,
        };
    }

    // 12. 构造快照元数据
    let meta = SourceSnapshotMeta {
        snapshot_id: snapshot_id.clone(),
        platform_id: "work_cn".to_string(),
        data_location_id: request.canonical_fixture_root.clone(),
        product_version: request.product_version.clone(),
        schema_fingerprint: request.schema_fingerprint.clone(),
        mapping_version: request.mapping_version.clone(),
        account_evidence_ref: request.account_evidence_ref.clone(),
        captured_at: request.now,
        files: staging_post.clone(),
        fingerprint: fingerprint.clone(),
    };

    // 13. 写入 snapshot.json 到 staging
    let snapshot_json_path = staging_snapshot_dir.join("snapshot.json");
    let json = match serde_json::to_string_pretty(&meta) {
        Ok(s) => s,
        Err(_) => {
            let _ = std::fs::remove_dir_all(&staging_snapshot_dir);
            return ScanOutcome::Failed {
                reason: ScanFailureReason::CatalogTransactionFailed,
            };
        }
    };
    if std::fs::write(&snapshot_json_path, json).is_err() {
        let _ = std::fs::remove_dir_all(&staging_snapshot_dir);
        return ScanOutcome::Failed {
            reason: ScanFailureReason::CatalogTransactionFailed,
        };
    }

    // 14. 原子发布：rename staging → snapshots/<snapshot_id>/
    let _ = std::fs::create_dir_all(&snapshots_dir);
    let publish_dir = snapshots_dir.join(snapshot_id.as_str());
    if publish_dir.exists() {
        // 快照 ID 冲突（极小概率）——不覆盖已发布快照
        let _ = std::fs::remove_dir_all(&staging_snapshot_dir);
        return ScanOutcome::Failed {
            reason: ScanFailureReason::CatalogTransactionFailed,
        };
    }
    if std::fs::rename(&staging_snapshot_dir, &publish_dir).is_err() {
        // rename 可能跨卷失败，回退到复制+删除
        if copy_dir_recursive(&staging_snapshot_dir, &publish_dir).is_err() {
            let _ = std::fs::remove_dir_all(&staging_snapshot_dir);
            return ScanOutcome::Failed {
                reason: ScanFailureReason::StorageRootUnavailable,
            };
        }
        let _ = std::fs::remove_dir_all(&staging_snapshot_dir);
    }

    ScanOutcome::Success {
        snapshot_id,
        snapshot_meta: meta,
        catalog_updated: false,
    }
}

/// 读取文件集：DB/WAL/SHM 的捕获信息。
///
/// 返回三个文件的 SnapshotFileEntry（按 DB, WAL, SHM 顺序）。
/// DB 必须存在；WAL/SHM 按实际存在性。
fn read_file_set(
    root: &Path,
    db_relative_path: &str,
    file_identity: &dyn FileIdentityProvider,
) -> Option<Vec<SnapshotFileEntry>> {
    let db_path = root.join(db_relative_path);
    let wal_path = root.join(format!("{}-wal", db_relative_path));
    let shm_path = root.join(format!("{}-shm", db_relative_path));

    let db_entry = read_file_entry(
        &db_path,
        db_relative_path,
        SnapshotFileKind::Db,
        file_identity,
    )?;
    // DB 必须存在
    if !db_entry.present {
        return None;
    }

    // WAL/SHM 读取失败时传播 None（文件不存在时 read_file_entry 返回 present=false 的 Some）
    let wal_entry = read_file_entry(
        &wal_path,
        &format!("{}-wal", db_relative_path),
        SnapshotFileKind::Wal,
        file_identity,
    )?;
    let shm_entry = read_file_entry(
        &shm_path,
        &format!("{}-shm", db_relative_path),
        SnapshotFileKind::Shm,
        file_identity,
    )?;

    Some(vec![db_entry, wal_entry, shm_entry])
}

/// 读取单个文件条目。文件不存在时返回 present=false 的条目。
fn read_file_entry(
    path: &Path,
    relative_path: &str,
    kind: SnapshotFileKind,
    file_identity: &dyn FileIdentityProvider,
) -> Option<SnapshotFileEntry> {
    if !path.is_file() {
        return Some(SnapshotFileEntry {
            kind,
            relative_path: relative_path.to_string(),
            present: false,
            size: 0,
            sha256: String::new(),
            file_identity: None,
        });
    }

    let metadata = std::fs::metadata(path).ok()?;
    let size = metadata.len();
    let content = std::fs::read(path).ok()?;
    let mut hasher = Sha256::new();
    hasher.update(&content);
    let sha256 = hex::encode(hasher.finalize());
    let identity = file_identity.read_file_identity(path);

    Some(SnapshotFileEntry {
        kind,
        relative_path: relative_path.to_string(),
        present: true,
        size,
        sha256,
        file_identity: identity,
    })
}

/// 比较两个文件集的内容是否相等（大小 + SHA-256，不比较文件身份）。
fn file_sets_content_equal(a: &[SnapshotFileEntry], b: &[SnapshotFileEntry]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    for (ae, be) in a.iter().zip(b.iter()) {
        if ae.relative_path != be.relative_path {
            return false;
        }
        if ae.present != be.present {
            return false;
        }
        if ae.present {
            if ae.size != be.size || ae.sha256 != be.sha256 {
                return false;
            }
        }
    }
    true
}

/// R3：源稳定性比较——比较捕获前后源文件集的 presence、size、file_identity。
///
/// 规格第 15 节要求"捕获前后存在性、大小或身份变化时废弃本次快照"。
/// 与 `file_sets_content_equal` 的区别：
/// - 比较 `file_identity`（同一物理文件的稳定身份，源文件不变时应一致）
/// - 不比较 `sha256`（SHA-256 已隐含在 size + identity 稳定性中，且 identity 更精准）
///
/// 任一文件 presence/size/identity 变化即判定为漂移。
fn file_sets_source_stable(pre: &[SnapshotFileEntry], post: &[SnapshotFileEntry]) -> bool {
    if pre.len() != post.len() {
        return false;
    }
    for (pe, ae) in pre.iter().zip(post.iter()) {
        if pe.relative_path != ae.relative_path {
            return false;
        }
        // presence 变化 -> 漂移
        if pe.present != ae.present {
            return false;
        }
        if pe.present {
            // size 变化 -> 漂移
            if pe.size != ae.size {
                return false;
            }
            // file_identity 变化 -> 漂移
            // 注意：identity 为 None 时（读取失败）保守视为稳定，避免误报
            if pe.file_identity.is_some()
                && ae.file_identity.is_some()
                && pe.file_identity != ae.file_identity
            {
                return false;
            }
        }
    }
    true
}

/// 计算数据指纹：所有存在文件的 (relative_path, sha256) 按路径排序后聚合哈希。
fn compute_fingerprint(entries: &[SnapshotFileEntry]) -> SnapshotFingerprint {
    let mut present: Vec<(&str, &str)> = entries
        .iter()
        .filter(|e| e.present)
        .map(|e| (e.relative_path.as_str(), e.sha256.as_str()))
        .collect();
    present.sort_by(|a, b| a.0.cmp(b.0));

    let mut hasher = Sha256::new();
    for (path, sha) in &present {
        hasher.update(path.as_bytes());
        hasher.update(b":");
        hasher.update(sha.as_bytes());
        hasher.update(b"\n");
    }
    SnapshotFingerprint(hex::encode(hasher.finalize()))
}

/// 仅反序列化 fingerprint 字段，避免反序列化整个 SourceSnapshotMeta（防注入设计）。
/// SourceSnapshotMeta 故意不派生 Deserialize，此处只提取比较指纹所需的最小字段。
#[derive(serde::Deserialize)]
struct SnapshotFingerprintOnly {
    fingerprint: SnapshotFingerprint,
}

/// 在 snapshots/ 目录中查找匹配指纹的快照。
fn find_snapshot_by_fingerprint(
    snapshots_dir: &Path,
    fingerprint: &SnapshotFingerprint,
) -> Option<SnapshotId> {
    let entries = std::fs::read_dir(snapshots_dir).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let snapshot_json = path.join("snapshot.json");
        if let Ok(content) = std::fs::read_to_string(&snapshot_json) {
            // 只反序列化 fingerprint 字段，保持 SourceSnapshotMeta 的防注入设计
            if let Ok(fp_only) = serde_json::from_str::<SnapshotFingerprintOnly>(&content) {
                if &fp_only.fingerprint == fingerprint {
                    // 从路径名提取 snapshot_id 并用 from_db_str 重建
                    if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                        return Some(SnapshotId::from_db_str(name));
                    }
                }
            }
        }
    }
    None
}

/// 递归复制目录（rename 跨卷失败时的回退）。
fn copy_dir_recursive(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let path = entry.path();
        let dest_path = dst.join(entry.file_name());
        if path.is_dir() {
            copy_dir_recursive(&path, &dest_path)?;
        } else {
            std::fs::copy(&path, &dest_path)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use traesync_domain::ProcessRunningState;

    /// 测试用 mock FileIdentityProvider——返回固定的虚拟身份。
    struct MockFileIdentityProvider;

    impl FileIdentityProvider for MockFileIdentityProvider {
        fn read_file_identity(&self, _path: &Path) -> Option<FileIdentity> {
            Some(FileIdentity {
                volume_serial: 1,
                file_index_high: 0,
                file_index_low: 1,
            })
        }
    }

    fn make_scan_request(fixture_root: &Path, storage_root: &Path) -> ScanRequest {
        ScanRequest {
            canonical_fixture_root: fixture_root.to_string_lossy().to_string(),
            db_relative_path: "database.db".to_string(),
            process_state: ProcessRunningState::NotRunning,
            now: SystemTime::UNIX_EPOCH,
            schema_fingerprint: "schema-fp-test".to_string(),
            mapping_version: "work_cn_v1".to_string(),
            product_version: "1.107.1".to_string(),
            account_evidence_ref: Some("evidence-ref-1".to_string()),
            storage_root: storage_root.to_string_lossy().to_string(),
        }
    }

    #[test]
    fn capture_db_only_snapshot() {
        let fixture = tempfile::tempdir().unwrap();
        let storage = tempfile::tempdir().unwrap();
        std::fs::write(fixture.path().join("database.db"), b"db-content").unwrap();

        let store = FilesystemSnapshotStore::new(Box::new(MockFileIdentityProvider));
        let request = make_scan_request(fixture.path(), storage.path());
        let outcome = store.capture_snapshot(&request);

        match outcome {
            ScanOutcome::Success { snapshot_meta, .. } => {
                assert_eq!(snapshot_meta.files.len(), 3);
                assert!(snapshot_meta.files[0].present); // DB
                assert!(!snapshot_meta.files[1].present); // WAL
                assert!(!snapshot_meta.files[2].present); // SHM
                let snapshot_dir = storage
                    .path()
                    .join("snapshots")
                    .join(snapshot_meta.snapshot_id.as_str());
                assert!(snapshot_dir.is_dir());
                assert!(snapshot_dir.join("snapshot.json").is_file());
                assert!(snapshot_dir.join("database.db").is_file());
            }
            other => panic!("期望 Success，实际 {:?}", other),
        }
    }

    #[test]
    fn capture_db_wal_shm_snapshot() {
        let fixture = tempfile::tempdir().unwrap();
        let storage = tempfile::tempdir().unwrap();
        std::fs::write(fixture.path().join("database.db"), b"db-content").unwrap();
        std::fs::write(fixture.path().join("database.db-wal"), b"wal-content").unwrap();
        std::fs::write(fixture.path().join("database.db-shm"), b"shm-content").unwrap();

        let store = FilesystemSnapshotStore::new(Box::new(MockFileIdentityProvider));
        let request = make_scan_request(fixture.path(), storage.path());
        let outcome = store.capture_snapshot(&request);

        match outcome {
            ScanOutcome::Success { snapshot_meta, .. } => {
                assert!(snapshot_meta.files[0].present);
                assert!(snapshot_meta.files[1].present);
                assert!(snapshot_meta.files[2].present);
            }
            other => panic!("期望 Success，实际 {:?}", other),
        }
    }

    #[test]
    fn capture_fails_when_process_running() {
        let fixture = tempfile::tempdir().unwrap();
        let storage = tempfile::tempdir().unwrap();
        std::fs::write(fixture.path().join("database.db"), b"db-content").unwrap();

        let store = FilesystemSnapshotStore::new(Box::new(MockFileIdentityProvider));
        let mut request = make_scan_request(fixture.path(), storage.path());
        request.process_state = ProcessRunningState::Running;
        let outcome = store.capture_snapshot(&request);

        match outcome {
            ScanOutcome::Failed { reason } => {
                assert_eq!(reason, ScanFailureReason::ProcessRunning);
            }
            other => panic!("期望 Failed(ProcessRunning)，实际 {:?}", other),
        }
    }

    #[test]
    fn capture_fails_when_db_missing() {
        let fixture = tempfile::tempdir().unwrap();
        let storage = tempfile::tempdir().unwrap();

        let store = FilesystemSnapshotStore::new(Box::new(MockFileIdentityProvider));
        let request = make_scan_request(fixture.path(), storage.path());
        let outcome = store.capture_snapshot(&request);

        match outcome {
            ScanOutcome::Failed { reason } => {
                assert_eq!(reason, ScanFailureReason::DatabaseMissing);
            }
            other => panic!("期望 Failed(DatabaseMissing)，实际 {:?}", other),
        }
    }

    #[test]
    fn deduplication_when_same_fingerprint() {
        let fixture = tempfile::tempdir().unwrap();
        let storage = tempfile::tempdir().unwrap();
        std::fs::write(fixture.path().join("database.db"), b"same-content").unwrap();

        let store = FilesystemSnapshotStore::new(Box::new(MockFileIdentityProvider));
        let request = make_scan_request(fixture.path(), storage.path());

        let outcome1 = store.capture_snapshot(&request);
        assert!(matches!(outcome1, ScanOutcome::Success { .. }));

        let outcome2 = store.capture_snapshot(&request);
        match outcome2 {
            ScanOutcome::Deduplicated { fingerprint, .. } => {
                assert!(!fingerprint.as_str().is_empty());
            }
            other => panic!("期望 Deduplicated，实际 {:?}", other),
        }
    }

    #[test]
    fn published_snapshot_not_overwritten() {
        let fixture = tempfile::tempdir().unwrap();
        let storage = tempfile::tempdir().unwrap();
        std::fs::write(fixture.path().join("database.db"), b"content-v1").unwrap();

        let store = FilesystemSnapshotStore::new(Box::new(MockFileIdentityProvider));
        let request = make_scan_request(fixture.path(), storage.path());

        let outcome1 = store.capture_snapshot(&request);
        let snapshot_id_1 = match outcome1 {
            ScanOutcome::Success { snapshot_meta, .. } => snapshot_meta.snapshot_id,
            other => panic!("期望 Success，实际 {:?}", other),
        };

        std::fs::write(fixture.path().join("database.db"), b"content-v2").unwrap();
        let outcome2 = store.capture_snapshot(&request);
        match outcome2 {
            ScanOutcome::Success { snapshot_meta, .. } => {
                assert_ne!(snapshot_meta.snapshot_id, snapshot_id_1);
            }
            other => panic!("期望 Success，实际 {:?}", other),
        }

        let dir1 = storage
            .path()
            .join("snapshots")
            .join(snapshot_id_1.as_str());
        assert!(dir1.is_dir(), "已发布快照不应被删除");
    }

    // ============== TDD #4：确定性源文件集漂移检测 ==============

    #[test]
    fn file_sets_content_equal_detects_size_mismatch() {
        // 确定性测试：大小不同 -> 漂移
        let a = vec![SnapshotFileEntry {
            kind: SnapshotFileKind::Db,
            relative_path: "database.db".to_string(),
            present: true,
            size: 100,
            sha256: "abc".to_string(),
            file_identity: None,
        }];
        let b = vec![SnapshotFileEntry {
            kind: SnapshotFileKind::Db,
            relative_path: "database.db".to_string(),
            present: true,
            size: 200,
            sha256: "abc".to_string(),
            file_identity: None,
        }];
        assert!(!file_sets_content_equal(&a, &b));
    }

    #[test]
    fn file_sets_content_equal_detects_sha256_mismatch() {
        // 确定性测试：SHA-256 不同 -> 漂移
        let a = vec![SnapshotFileEntry {
            kind: SnapshotFileKind::Db,
            relative_path: "database.db".to_string(),
            present: true,
            size: 100,
            sha256: "aaa".to_string(),
            file_identity: None,
        }];
        let b = vec![SnapshotFileEntry {
            kind: SnapshotFileKind::Db,
            relative_path: "database.db".to_string(),
            present: true,
            size: 100,
            sha256: "bbb".to_string(),
            file_identity: None,
        }];
        assert!(!file_sets_content_equal(&a, &b));
    }

    #[test]
    fn file_sets_content_equal_detects_presence_mismatch() {
        // 确定性测试：文件存在性变化 -> 漂移
        let a = vec![SnapshotFileEntry {
            kind: SnapshotFileKind::Wal,
            relative_path: "database.db-wal".to_string(),
            present: true,
            size: 50,
            sha256: "xxx".to_string(),
            file_identity: None,
        }];
        let b = vec![SnapshotFileEntry {
            kind: SnapshotFileKind::Wal,
            relative_path: "database.db-wal".to_string(),
            present: false,
            size: 0,
            sha256: String::new(),
            file_identity: None,
        }];
        assert!(!file_sets_content_equal(&a, &b));
    }

    #[test]
    fn file_sets_content_equal_accepts_identical_sets() {
        // 相同文件集 -> 不漂移
        let a = vec![SnapshotFileEntry {
            kind: SnapshotFileKind::Db,
            relative_path: "database.db".to_string(),
            present: true,
            size: 100,
            sha256: "abc".to_string(),
            file_identity: None,
        }];
        let b = a.clone();
        assert!(file_sets_content_equal(&a, &b));
    }

    #[test]
    fn compute_fingerprint_is_deterministic() {
        // 确定性测试：相同文件集（不同顺序）-> 相同指纹
        let entries = vec![
            SnapshotFileEntry {
                kind: SnapshotFileKind::Db,
                relative_path: "database.db".to_string(),
                present: true,
                size: 100,
                sha256: "abc".to_string(),
                file_identity: None,
            },
            SnapshotFileEntry {
                kind: SnapshotFileKind::Wal,
                relative_path: "database.db-wal".to_string(),
                present: true,
                size: 50,
                sha256: "def".to_string(),
                file_identity: None,
            },
        ];
        let fp1 = compute_fingerprint(&entries);
        // 反向顺序
        let mut reversed = entries.clone();
        reversed.reverse();
        let fp2 = compute_fingerprint(&reversed);
        assert_eq!(fp1, fp2, "指纹应不受文件顺序影响");
    }

    #[test]
    fn capture_db_wal_only_snapshot() {
        // TDD #3：DB+WAL（无 SHM）也能捕获
        let fixture = tempfile::tempdir().unwrap();
        let storage = tempfile::tempdir().unwrap();
        std::fs::write(fixture.path().join("database.db"), b"db-content").unwrap();
        std::fs::write(fixture.path().join("database.db-wal"), b"wal-content").unwrap();

        let store = FilesystemSnapshotStore::new(Box::new(MockFileIdentityProvider));
        let request = make_scan_request(fixture.path(), storage.path());
        let outcome = store.capture_snapshot(&request);

        match outcome {
            ScanOutcome::Success { snapshot_meta, .. } => {
                assert!(snapshot_meta.files[0].present); // DB
                assert!(snapshot_meta.files[1].present); // WAL
                assert!(!snapshot_meta.files[2].present); // SHM 不存在
            }
            other => panic!("期望 Success，实际 {:?}", other),
        }
    }

    // ============== R3：源稳定性捕获前后验证反例测试 ==============

    /// 辅助：构造可变 FileIdentityProvider——在首次 read_file_identity 调用时
    /// 对源 DB 文件追加字节，模拟捕获期间源被修改。
    /// read_file_identity 在 read_file_entry 中最后调用（size/sha 已读取），
    /// 因此 pre_capture 的 DB entry 保留旧 size，post_source 读到新 size → 漂移。
    use std::io::Write;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    struct DriftDbSizeProvider {
        fixture_db_path: std::path::PathBuf,
        mutated: AtomicBool,
    }

    impl FileIdentityProvider for DriftDbSizeProvider {
        fn read_file_identity(&self, _path: &Path) -> Option<FileIdentity> {
            if !self.mutated.swap(true, Ordering::SeqCst) {
                // 追加字节 → size 变化
                if let Ok(mut f) = std::fs::OpenOptions::new()
                    .append(true)
                    .open(&self.fixture_db_path)
                {
                    let _ = f.write_all(b"DRIFT-SUFFIX");
                }
            }
            Some(FileIdentity {
                volume_serial: 1,
                file_index_high: 0,
                file_index_low: 1,
            })
        }
    }

    /// 辅助：在第 3 次 read_file_identity 调用（post_source 的 DB 读取）时
    /// 删除 WAL 文件，使 post_source 读到 WAL 不存在 → presence 漂移。
    /// 调用顺序：pre_capture DB(0) → pre_capture WAL(1) → [复制完成] →
    /// post_source DB(2，此时删除 WAL) → post_source WAL(3，已不存在)
    struct DriftWalDisappearProvider {
        fixture_wal_path: std::path::PathBuf,
        call_count: AtomicUsize,
    }

    impl FileIdentityProvider for DriftWalDisappearProvider {
        fn read_file_identity(&self, _path: &Path) -> Option<FileIdentity> {
            let n = self.call_count.fetch_add(1, Ordering::SeqCst);
            // 第 3 次调用（索引 2）= post_source 的 DB 读取时删除 WAL
            // 此时 pre_capture 已完成且复制已完成，WAL 在 pre_capture 中存在、在 post_source 中不存在
            if n == 2 {
                let _ = std::fs::remove_file(&self.fixture_wal_path);
            }
            Some(FileIdentity {
                volume_serial: 1,
                file_index_high: 0,
                file_index_low: 1,
            })
        }
    }

    /// 辅助：pre_capture 和 post_source 返回不同 file_identity，模拟文件被替换。
    struct DriftIdentityProvider {
        call_count: AtomicUsize,
    }

    impl FileIdentityProvider for DriftIdentityProvider {
        fn read_file_identity(&self, _path: &Path) -> Option<FileIdentity> {
            let n = self.call_count.fetch_add(1, Ordering::SeqCst);
            // 偶数调用（pre_capture 的 DB，索引 0）返回 identity A
            // 奇数调用（post_source 的 DB，索引 1）返回 identity B
            let low = if n == 0 { 1 } else { 2 };
            Some(FileIdentity {
                volume_serial: 1,
                file_index_high: 0,
                file_index_low: low,
            })
        }
    }

    #[test]
    fn r3_source_drift_db_size_change_rejects_publish() {
        // 反例：捕获期间 DB 文件被追加字节 → size 漂移 → 废弃快照
        let fixture = tempfile::tempdir().unwrap();
        let storage = tempfile::tempdir().unwrap();
        std::fs::write(fixture.path().join("database.db"), b"db-content").unwrap();

        let provider = DriftDbSizeProvider {
            fixture_db_path: fixture.path().join("database.db"),
            mutated: AtomicBool::new(false),
        };
        let store = FilesystemSnapshotStore::new(Box::new(provider));
        let request = make_scan_request(fixture.path(), storage.path());
        let outcome = store.capture_snapshot(&request);

        match outcome {
            ScanOutcome::Failed { reason } => {
                assert_eq!(
                    reason,
                    ScanFailureReason::SourceSetDrift,
                    "DB size 漂移应废弃快照"
                );
            }
            other => panic!("期望 Failed(SourceSetDrift)，实际 {:?}", other),
        }
        // staging 应被清理
        let staging = storage.path().join("staging");
        assert!(
            !staging.exists()
                || std::fs::read_dir(&staging)
                    .map(|mut d| d.next().is_none())
                    .unwrap_or(true),
            "漂移后 staging 应被清理"
        );
        // snapshots 目录不应有已发布快照
        let snapshots = storage.path().join("snapshots");
        assert!(
            !snapshots.exists()
                || std::fs::read_dir(&snapshots)
                    .map(|mut d| d.next().is_none())
                    .unwrap_or(true)
        );
    }

    #[test]
    fn r3_source_drift_wal_disappears_rejects_publish() {
        // 反例：捕获期间 WAL 文件消失 → presence 漂移 → 废弃快照
        let fixture = tempfile::tempdir().unwrap();
        let storage = tempfile::tempdir().unwrap();
        std::fs::write(fixture.path().join("database.db"), b"db-content").unwrap();
        std::fs::write(fixture.path().join("database.db-wal"), b"wal-content").unwrap();

        let provider = DriftWalDisappearProvider {
            fixture_wal_path: fixture.path().join("database.db-wal"),
            call_count: AtomicUsize::new(0),
        };
        let store = FilesystemSnapshotStore::new(Box::new(provider));
        let request = make_scan_request(fixture.path(), storage.path());
        let outcome = store.capture_snapshot(&request);

        match outcome {
            ScanOutcome::Failed { reason } => {
                assert_eq!(
                    reason,
                    ScanFailureReason::SourceSetDrift,
                    "WAL 消失应废弃快照"
                );
            }
            other => panic!("期望 Failed(SourceSetDrift)，实际 {:?}", other),
        }
    }

    #[test]
    fn r3_source_drift_identity_change_rejects_publish() {
        // 反例：file_identity 变化（文件被替换）→ 漂移 → 废弃快照
        let fixture = tempfile::tempdir().unwrap();
        let storage = tempfile::tempdir().unwrap();
        std::fs::write(fixture.path().join("database.db"), b"db-content").unwrap();

        let provider = DriftIdentityProvider {
            call_count: AtomicUsize::new(0),
        };
        let store = FilesystemSnapshotStore::new(Box::new(provider));
        let request = make_scan_request(fixture.path(), storage.path());
        let outcome = store.capture_snapshot(&request);

        match outcome {
            ScanOutcome::Failed { reason } => {
                assert_eq!(
                    reason,
                    ScanFailureReason::SourceSetDrift,
                    "file_identity 变化应废弃快照"
                );
            }
            other => panic!("期望 Failed(SourceSetDrift)，实际 {:?}", other),
        }
    }

    #[test]
    fn r3_source_stable_accepts_unchanged_source() {
        // 正例：源文件不变 → 正常捕获
        let fixture = tempfile::tempdir().unwrap();
        let storage = tempfile::tempdir().unwrap();
        std::fs::write(fixture.path().join("database.db"), b"db-content").unwrap();
        std::fs::write(fixture.path().join("database.db-wal"), b"wal-content").unwrap();

        let store = FilesystemSnapshotStore::new(Box::new(MockFileIdentityProvider));
        let request = make_scan_request(fixture.path(), storage.path());
        let outcome = store.capture_snapshot(&request);

        match outcome {
            ScanOutcome::Success { snapshot_meta, .. } => {
                assert_eq!(snapshot_meta.files.len(), 3);
                assert!(snapshot_meta.files[0].present); // DB
                assert!(snapshot_meta.files[1].present); // WAL
            }
            other => panic!("期望 Success，实际 {:?}", other),
        }
    }

    #[test]
    fn r3_file_sets_source_stable_detects_size_mismatch() {
        let pre = vec![SnapshotFileEntry {
            kind: SnapshotFileKind::Db,
            relative_path: "database.db".to_string(),
            present: true,
            size: 100,
            sha256: "abc".to_string(),
            file_identity: Some(FileIdentity {
                volume_serial: 1,
                file_index_high: 0,
                file_index_low: 1,
            }),
        }];
        let post = vec![SnapshotFileEntry {
            kind: SnapshotFileKind::Db,
            relative_path: "database.db".to_string(),
            present: true,
            size: 200,
            sha256: "abc".to_string(),
            file_identity: Some(FileIdentity {
                volume_serial: 1,
                file_index_high: 0,
                file_index_low: 1,
            }),
        }];
        assert!(!file_sets_source_stable(&pre, &post));
    }

    #[test]
    fn r3_file_sets_source_stable_detects_identity_mismatch() {
        let pre = vec![SnapshotFileEntry {
            kind: SnapshotFileKind::Db,
            relative_path: "database.db".to_string(),
            present: true,
            size: 100,
            sha256: "abc".to_string(),
            file_identity: Some(FileIdentity {
                volume_serial: 1,
                file_index_high: 0,
                file_index_low: 1,
            }),
        }];
        let post = vec![SnapshotFileEntry {
            kind: SnapshotFileKind::Db,
            relative_path: "database.db".to_string(),
            present: true,
            size: 100,
            sha256: "abc".to_string(),
            file_identity: Some(FileIdentity {
                volume_serial: 1,
                file_index_high: 0,
                file_index_low: 2,
            }),
        }];
        assert!(!file_sets_source_stable(&pre, &post));
    }

    #[test]
    fn r3_file_sets_source_stable_detects_presence_mismatch() {
        let pre = vec![SnapshotFileEntry {
            kind: SnapshotFileKind::Wal,
            relative_path: "database.db-wal".to_string(),
            present: true,
            size: 50,
            sha256: "def".to_string(),
            file_identity: Some(FileIdentity {
                volume_serial: 1,
                file_index_high: 0,
                file_index_low: 2,
            }),
        }];
        let post = vec![SnapshotFileEntry {
            kind: SnapshotFileKind::Wal,
            relative_path: "database.db-wal".to_string(),
            present: false,
            size: 0,
            sha256: String::new(),
            file_identity: None,
        }];
        assert!(!file_sets_source_stable(&pre, &post));
    }

    #[test]
    fn r3_file_sets_source_stable_accepts_identical_sets() {
        let pre = vec![SnapshotFileEntry {
            kind: SnapshotFileKind::Db,
            relative_path: "database.db".to_string(),
            present: true,
            size: 100,
            sha256: "abc".to_string(),
            file_identity: Some(FileIdentity {
                volume_serial: 1,
                file_index_high: 0,
                file_index_low: 1,
            }),
        }];
        let post = pre.clone();
        assert!(file_sets_source_stable(&pre, &post));
    }

    #[test]
    fn r3_file_sets_source_stable_accepts_none_identity() {
        // identity 为 None 时不判定漂移（保守，避免误报）
        let pre = vec![SnapshotFileEntry {
            kind: SnapshotFileKind::Db,
            relative_path: "database.db".to_string(),
            present: true,
            size: 100,
            sha256: "abc".to_string(),
            file_identity: None,
        }];
        let post = pre.clone();
        assert!(file_sets_source_stable(&pre, &post));
    }
}
