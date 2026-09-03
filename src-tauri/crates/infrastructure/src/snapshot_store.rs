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

use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};
use traesync_domain::{
    ProcessRunningState, ScanFailureReason, ScanOutcome, ScanRequest, SnapshotFileEntry,
    SnapshotFileKind, SnapshotFingerprint, SnapshotId, SourceSnapshotMeta,
};
use traesync_ports::{FileIdentityProvider, SnapshotStore};

// R2：复用同 crate 的封闭证明——避免在 snapshot_store 内重复实现较弱校验
use crate::data_location::derive_data_location_id;
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
        capture_snapshot_inner(self.file_identity.as_ref(), request, &|| true, &|| true)
    }

    fn capture_snapshot_with_validation(
        &self,
        request: &ScanRequest,
        is_authorized: &dyn Fn() -> bool,
    ) -> ScanOutcome {
        capture_snapshot_inner(self.file_identity.as_ref(), request, is_authorized, &|| {
            true
        })
    }

    fn capture_snapshot_with_context_validation(
        &self,
        request: &ScanRequest,
        is_authorized: &dyn Fn() -> bool,
        validate_context: &dyn Fn() -> bool,
    ) -> ScanOutcome {
        capture_snapshot_inner(
            self.file_identity.as_ref(),
            request,
            is_authorized,
            validate_context,
        )
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
    is_authorized: &dyn Fn() -> bool,
    validate_context: &dyn Fn() -> bool,
) -> ScanOutcome {
    // 组合根已经先做过授权检查；这里再保留一道基础设施边界，
    // 防止直接调用快照存储时绕过授权状态。
    if !is_authorized() || !validate_context() {
        return ScanOutcome::Failed {
            reason: ScanFailureReason::NotAuthorized,
        };
    }

    // R1 修订（U-6 W3，依据 `.scratch/history-u6/w0-report.md` 实测 0/105 撕裂）：
    // 捕获本身就是复制 db+wal+shm 三件套快照，TRAE 运行中持续写入时
    // 读取一致性已实证安全，进程 Running 不再是捕获拒绝条件。

    let fixture_root = Path::new(&request.canonical_fixture_root);
    let storage_root = Path::new(&request.storage_root);

    // 2. R2：defense-in-depth——在任何文件访问前再次验证 db_relative_path 封闭在 fixture_root 内
    //    组合根已用 FixturePathGuard 验证过；此处的二次校验防止 commands/application 层
    //    绕过组合根直接调用 infrastructure 时仍能拒绝逃逸路径
    let canonical_db =
        match validate_db_relative_path_inside(fixture_root, &request.db_relative_path) {
            Ok(path) => path,
            Err(_) => {
                return ScanOutcome::Failed {
                    reason: ScanFailureReason::DatabaseMissing,
                };
            }
        };
    let canonical_root = match fixture_root.canonicalize() {
        Ok(path) => path,
        Err(_) => {
            return ScanOutcome::Failed {
                reason: ScanFailureReason::DatabaseMissing,
            };
        }
    };
    let normalized_relative = match canonical_db.strip_prefix(&canonical_root) {
        Ok(path) => path.to_string_lossy().replace('\\', "/"),
        Err(_) => {
            return ScanOutcome::Failed {
                reason: ScanFailureReason::DatabaseMissing,
            };
        }
    };
    // 3. 验证 database.db 存在
    let db_path = fixture_root.join(&request.db_relative_path);
    if !db_path.is_file() {
        return ScanOutcome::Failed {
            reason: ScanFailureReason::DatabaseMissing,
        };
    }

    // 4. 读取捕获前文件集
    let pre_capture = match read_file_set_with_validation(
        fixture_root,
        &request.db_relative_path,
        file_identity,
        is_authorized,
    ) {
        Some(entries) => entries,
        None => {
            if !is_authorized() {
                return ScanOutcome::Failed {
                    reason: ScanFailureReason::NotAuthorized,
                };
            }
            return ScanOutcome::Failed {
                reason: ScanFailureReason::SourceSetDrift,
            };
        }
    };
    let Some(data_location_id) = derive_data_location_id(
        &canonical_root,
        &normalized_relative,
        file_identity.read_file_identity(&canonical_root).as_ref(),
        pre_capture
            .first()
            .and_then(|entry| entry.file_identity.as_ref()),
    ) else {
        return ScanOutcome::Failed {
            reason: ScanFailureReason::SourceSetDrift,
        };
    };

    if !is_authorized() {
        return ScanOutcome::Failed {
            reason: ScanFailureReason::NotAuthorized,
        };
    }

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
        if let Some(parent) = dst.parent() {
            if let Err(_) = std::fs::create_dir_all(parent) {
                preserve_staging_failure(storage_root, &staging_snapshot_dir);
                return ScanOutcome::Failed {
                    reason: ScanFailureReason::StorageRootUnavailable,
                };
            }
        }
        match copy_file_with_validation(&src, &dst, is_authorized) {
            Ok(true) => {}
            Ok(false) => {
                preserve_staging_failure(storage_root, &staging_snapshot_dir);
                return ScanOutcome::Failed {
                    reason: ScanFailureReason::NotAuthorized,
                };
            }
            Err(_) => {
                preserve_staging_failure(storage_root, &staging_snapshot_dir);
                return ScanOutcome::Failed {
                    reason: ScanFailureReason::StorageRootUnavailable,
                };
            }
        }
    }

    // 7. R3：复制后重读源文件集（fixture_root）——检测捕获期间源漂移。
    //    规格第 15 节：捕获前后存在性、大小或身份变化时废弃本次快照。
    //    旧实现读取 staging 副本，无法发现源在复制期间被修改的情况。
    let post_source = match read_file_set_with_validation(
        fixture_root,
        &request.db_relative_path,
        file_identity,
        is_authorized,
    ) {
        Some(entries) => entries,
        None => {
            preserve_staging_failure(storage_root, &staging_snapshot_dir);
            if !is_authorized() {
                return ScanOutcome::Failed {
                    reason: ScanFailureReason::NotAuthorized,
                };
            }
            return ScanOutcome::Failed {
                reason: ScanFailureReason::SourceSetDrift,
            };
        }
    };

    // 8. R3：源稳定性比较——pre_capture vs post_source
    //    比较 presence、size、file_identity（规格要求的"存在性、大小或身份"）
    if !file_sets_source_stable(&pre_capture, &post_source) {
        preserve_staging_failure(storage_root, &staging_snapshot_dir);
        return ScanOutcome::Failed {
            reason: ScanFailureReason::SourceSetDrift,
        };
    }

    if !is_authorized() {
        preserve_staging_failure(storage_root, &staging_snapshot_dir);
        return ScanOutcome::Failed {
            reason: ScanFailureReason::NotAuthorized,
        };
    }

    // 9. R3：独立验证 staging 内容完整性——pre_capture vs staging 副本
    //    比较 size + SHA-256，确保复制过程未损坏（文件身份在 staging 中不同，不比较）
    let staging_post = match read_file_set_with_validation(
        &staging_snapshot_dir,
        &request.db_relative_path,
        file_identity,
        is_authorized,
    ) {
        Some(entries) => entries,
        None => {
            preserve_staging_failure(storage_root, &staging_snapshot_dir);
            if !is_authorized() {
                return ScanOutcome::Failed {
                    reason: ScanFailureReason::NotAuthorized,
                };
            }
            return ScanOutcome::Failed {
                reason: ScanFailureReason::SourceSetDrift,
            };
        }
    };
    if !file_sets_content_equal(&pre_capture, &staging_post) {
        preserve_staging_failure(storage_root, &staging_snapshot_dir);
        return ScanOutcome::Failed {
            reason: ScanFailureReason::SourceSetDrift,
        };
    }

    if !is_authorized() {
        preserve_staging_failure(storage_root, &staging_snapshot_dir);
        return ScanOutcome::Failed {
            reason: ScanFailureReason::NotAuthorized,
        };
    }

    // 10. 计算数据指纹（来自经过完整性验证的 staging 副本）
    let fingerprint = compute_fingerprint(&staging_post);

    // 11. 查询已有快照是否同指纹
    let snapshots_dir = storage_root.join("snapshots");
    if let Some((existing_id, existing_meta)) = find_snapshot_by_fingerprint(
        &snapshots_dir,
        &fingerprint,
        &data_location_id,
        request,
        &staging_post,
    ) {
        if !is_authorized() {
            preserve_staging_failure(storage_root, &staging_snapshot_dir);
            return ScanOutcome::Failed {
                reason: ScanFailureReason::NotAuthorized,
            };
        }
        // 去重：删除 staging，返回 Deduplicated
        let _ = std::fs::remove_dir_all(&staging_snapshot_dir);
        return ScanOutcome::Deduplicated {
            existing_snapshot_id: existing_id,
            fingerprint,
            snapshot_meta: existing_meta,
        };
    }

    // 12. 构造快照元数据
    let meta = SourceSnapshotMeta {
        snapshot_id: snapshot_id.clone(),
        platform_id: "work_cn".to_string(),
        data_location_id: data_location_id.clone(),
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
            preserve_staging_failure(storage_root, &staging_snapshot_dir);
            return ScanOutcome::Failed {
                reason: ScanFailureReason::CatalogTransactionFailed,
            };
        }
    };
    if std::fs::write(&snapshot_json_path, json).is_err() {
        preserve_staging_failure(storage_root, &staging_snapshot_dir);
        return ScanOutcome::Failed {
            reason: ScanFailureReason::CatalogTransactionFailed,
        };
    }

    // 发布目录前复核完整上下文；分块复制与哈希阶段只检查廉价授权状态。
    if !validate_context() || !is_authorized() {
        preserve_staging_failure(storage_root, &staging_snapshot_dir);
        return ScanOutcome::Failed {
            reason: ScanFailureReason::NotAuthorized,
        };
    }

    // 14. 原子发布：rename staging → snapshots/<snapshot_id>/
    let _ = std::fs::create_dir_all(&snapshots_dir);
    let publish_dir = snapshots_dir.join(snapshot_id.as_str());
    if publish_dir.exists() {
        // 快照 ID 冲突（极小概率）——不覆盖已发布快照
        preserve_staging_failure(storage_root, &staging_snapshot_dir);
        return ScanOutcome::Failed {
            reason: ScanFailureReason::CatalogTransactionFailed,
        };
    }
    if std::fs::rename(&staging_snapshot_dir, &publish_dir).is_err() {
        // rename 可能跨卷失败，回退到复制+删除
        // 先复制到 snapshots 内的临时目录，再原子改名到最终目录；失败时保留原 staging。
        let publish_staging = snapshots_dir.join(format!(
            ".{}.publish-{}",
            snapshot_id.as_str(),
            std::process::id()
        ));
        if publish_staging.exists()
            || !matches!(
                copy_dir_recursive(&staging_snapshot_dir, &publish_staging, is_authorized),
                Ok(true)
            )
        {
            preserve_staging_failure(storage_root, &publish_staging);
            preserve_staging_failure(storage_root, &staging_snapshot_dir);
            if !is_authorized() {
                return ScanOutcome::Failed {
                    reason: ScanFailureReason::NotAuthorized,
                };
            }
            return ScanOutcome::Failed {
                reason: ScanFailureReason::StorageRootUnavailable,
            };
        }
        if std::fs::rename(&publish_staging, &publish_dir).is_err() {
            preserve_staging_failure(storage_root, &publish_staging);
            preserve_staging_failure(storage_root, &staging_snapshot_dir);
            return ScanOutcome::Failed {
                reason: ScanFailureReason::StorageRootUnavailable,
            };
        }
        let _ = std::fs::remove_dir_all(&staging_snapshot_dir);
    }

    // 发布动作完成后再次确认授权和完整上下文；失效时把刚发布的目录移入失败现场，
    // 避免正式快照暴露半次扫描结果。
    if !is_authorized() || !validate_context() {
        preserve_staging_failure(storage_root, &publish_dir);
        return ScanOutcome::Failed {
            reason: ScanFailureReason::NotAuthorized,
        };
    }

    ScanOutcome::Success {
        snapshot_id,
        snapshot_meta: meta,
        catalog_updated: false,
    }
}

/// 失败现场转入明确目录；移动失败时保留原 staging，不静默删除证据。
fn preserve_staging_failure(storage_root: &Path, staging: &Path) {
    if !staging.exists() {
        return;
    }
    let failure_root = storage_root.join("failure-staging");
    match std::fs::symlink_metadata(&failure_root) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => return,
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            if std::fs::create_dir(&failure_root).is_err() {
                return;
            }
        }
        Err(_) => return,
    }
    let name = staging
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("staging");
    let destination = failure_root.join(format!("{name}-{}", std::process::id()));
    if std::fs::rename(staging, destination).is_err() {
        // 原 staging 仍是失败证据，留在原处供诊断与人工处理。
    }
}

/// 读取文件集：DB/WAL/SHM 的捕获信息。
///
/// 返回三个文件的 SnapshotFileEntry（按 DB, WAL, SHM 顺序）。
/// DB 必须存在；WAL/SHM 按实际存在性。
/// 读取文件集的可中止版本；SHA-256 分块读取期间持续复核授权。
fn read_file_set_with_validation(
    root: &Path,
    db_relative_path: &str,
    file_identity: &dyn FileIdentityProvider,
    is_authorized: &dyn Fn() -> bool,
) -> Option<Vec<SnapshotFileEntry>> {
    if !is_authorized() {
        return None;
    }
    let db_path = root.join(db_relative_path);
    let wal_path = root.join(format!("{}-wal", db_relative_path));
    let shm_path = root.join(format!("{}-shm", db_relative_path));

    let db_entry = read_file_entry_with_validation(
        &db_path,
        db_relative_path,
        SnapshotFileKind::Db,
        file_identity,
        is_authorized,
    )?;
    // DB 必须存在
    if !db_entry.present {
        return None;
    }

    // WAL/SHM 读取失败时传播 None（文件不存在时 read_file_entry 返回 present=false 的 Some）
    let wal_entry = read_file_entry_with_validation(
        &wal_path,
        &format!("{}-wal", db_relative_path),
        SnapshotFileKind::Wal,
        file_identity,
        is_authorized,
    )?;
    let shm_entry = read_file_entry_with_validation(
        &shm_path,
        &format!("{}-shm", db_relative_path),
        SnapshotFileKind::Shm,
        file_identity,
        is_authorized,
    )?;

    Some(vec![db_entry, wal_entry, shm_entry])
}

/// 读取单个文件条目。文件不存在时返回 present=false 的条目。
fn read_file_entry_with_validation(
    path: &Path,
    relative_path: &str,
    kind: SnapshotFileKind,
    file_identity: &dyn FileIdentityProvider,
    is_authorized: &dyn Fn() -> bool,
) -> Option<SnapshotFileEntry> {
    if !is_authorized() {
        return None;
    }
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
    let sha256 = sha256_file_with_validation(path, is_authorized)?;
    if !is_authorized() {
        return None;
    }
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
///
/// R11：identity 的 Option 状态或具体值变化都判定为 drift。
/// - `Some(A) -> None`：身份读取变得不确定，保守失败
/// - `None -> Some(A)`：身份读取变得不确定，保守失败
/// - `Some(A) -> Some(B)` 且 A != B：身份变更
/// 仅当前后 identity 完全相等（同为 None 或同为 Some 且值相等）时才视为稳定。
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
            // R11：file_identity 的 Option 状态或具体值变化 -> 漂移
            // 保守失败：identity 读取不确定时不得继续发布快照
            if pe.file_identity != ae.file_identity {
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

/// 私有持久化 DTO：只在验证快照目录身份后重建公开 SourceSnapshotMeta。
#[derive(serde::Deserialize)]
struct StoredSourceSnapshotMeta {
    snapshot_id: String,
    platform_id: String,
    data_location_id: String,
    product_version: String,
    schema_fingerprint: String,
    mapping_version: String,
    account_evidence_ref: Option<String>,
    captured_at: std::time::SystemTime,
    files: Vec<SnapshotFileEntry>,
    fingerprint: SnapshotFingerprint,
}

/// 在 snapshots/ 目录中查找匹配指纹的快照。
fn find_snapshot_by_fingerprint(
    snapshots_dir: &Path,
    fingerprint: &SnapshotFingerprint,
    data_location_id: &str,
    request: &ScanRequest,
    current_files: &[SnapshotFileEntry],
) -> Option<(SnapshotId, SourceSnapshotMeta)> {
    let canonical_snapshots_dir = snapshots_dir.canonicalize().ok()?;
    let entries = std::fs::read_dir(snapshots_dir).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(canonical_snapshot_dir) = path.canonicalize() else {
            continue;
        };
        let Ok(relative_snapshot_dir) =
            canonical_snapshot_dir.strip_prefix(&canonical_snapshots_dir)
        else {
            continue;
        };
        if !canonical_snapshot_dir.is_dir() || relative_snapshot_dir.components().count() != 1 {
            continue;
        }
        let snapshot_json = canonical_snapshot_dir.join("snapshot.json");
        let Ok(canonical_snapshot_json) = snapshot_json.canonicalize() else {
            continue;
        };
        if canonical_snapshot_json
            .strip_prefix(&canonical_snapshot_dir)
            .is_err()
            || !canonical_snapshot_json.is_file()
        {
            continue;
        }
        if let Ok(content) = std::fs::read_to_string(&canonical_snapshot_json) {
            if let Ok(stored) = serde_json::from_str::<StoredSourceSnapshotMeta>(&content) {
                let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                    continue;
                };
                if stored.snapshot_id != name
                    || &stored.fingerprint != fingerprint
                    || stored.data_location_id != data_location_id
                    || stored.platform_id != "work_cn"
                    || stored.product_version != request.product_version
                    || stored.schema_fingerprint != request.schema_fingerprint
                    || stored.mapping_version != request.mapping_version
                    || stored.account_evidence_ref != request.account_evidence_ref
                    || !file_sets_content_equal(&stored.files, current_files)
                    || compute_fingerprint(&stored.files) != stored.fingerprint
                    || !validate_stored_snapshot_files(&canonical_snapshot_dir, &stored.files)
                {
                    continue;
                }
                let snapshot_id = SnapshotId::from_db_str(name);
                return Some((
                    snapshot_id.clone(),
                    SourceSnapshotMeta {
                        snapshot_id,
                        platform_id: stored.platform_id,
                        data_location_id: stored.data_location_id,
                        product_version: stored.product_version,
                        schema_fingerprint: stored.schema_fingerprint,
                        mapping_version: stored.mapping_version,
                        account_evidence_ref: stored.account_evidence_ref,
                        captured_at: stored.captured_at,
                        files: stored.files,
                        fingerprint: stored.fingerprint,
                    },
                ));
            }
        }
    }
    None
}

/// 重新核对既有快照文件，防止篡改后的快照凭旧 snapshot.json 被去重复用。
fn validate_stored_snapshot_files(snapshot_dir: &Path, files: &[SnapshotFileEntry]) -> bool {
    if files.len() != 3 {
        return false;
    }

    let mut db_count = 0;
    let mut wal_count = 0;
    let mut shm_count = 0;
    let mut relative_paths = std::collections::HashSet::new();

    for entry in files {
        match entry.kind {
            SnapshotFileKind::Db => db_count += 1,
            SnapshotFileKind::Wal => wal_count += 1,
            SnapshotFileKind::Shm => shm_count += 1,
        }

        let relative_path = Path::new(&entry.relative_path);
        if relative_path.as_os_str().is_empty()
            || relative_path.is_absolute()
            || !relative_path
                .components()
                .all(|component| matches!(component, std::path::Component::Normal(_)))
            || !relative_paths.insert(entry.relative_path.as_str())
        {
            return false;
        }

        let candidate = snapshot_dir.join(relative_path);
        if !entry.present {
            if std::fs::symlink_metadata(&candidate).is_ok() {
                return false;
            }
            continue;
        }

        let Ok(canonical_candidate) = candidate.canonicalize() else {
            return false;
        };
        if canonical_candidate.strip_prefix(snapshot_dir).is_err() || !canonical_candidate.is_file()
        {
            return false;
        }
        let Ok(metadata) = canonical_candidate.metadata() else {
            return false;
        };
        if metadata.len() != entry.size
            || sha256_file(&canonical_candidate).as_deref() != Some(entry.sha256.as_str())
        {
            return false;
        }
    }

    db_count == 1 && wal_count == 1 && shm_count == 1
}

/// 递归复制目录（rename 跨卷失败时的回退），并在文件复制期间复核授权。
fn copy_dir_recursive(
    src: &Path,
    dst: &Path,
    is_authorized: &dyn Fn() -> bool,
) -> std::io::Result<bool> {
    if !is_authorized() {
        return Ok(false);
    }
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        if !is_authorized() {
            return Ok(false);
        }
        let entry = entry?;
        let path = entry.path();
        let dest_path = dst.join(entry.file_name());
        if path.is_dir() {
            if !copy_dir_recursive(&path, &dest_path, is_authorized)? {
                return Ok(false);
            }
        } else {
            if !copy_file_with_validation(&path, &dest_path, is_authorized)? {
                return Ok(false);
            }
        }
    }
    Ok(is_authorized())
}

/// 分块复制快照文件，避免系统级整文件复制无法响应授权撤销。
fn copy_file_with_validation(
    source: &Path,
    destination: &Path,
    is_authorized: &dyn Fn() -> bool,
) -> std::io::Result<bool> {
    if !is_authorized() {
        return Ok(false);
    }
    let mut source_file = File::open(source)?;
    let mut destination_file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination)?;
    let mut buffer = vec![0_u8; 1024 * 1024];
    loop {
        if !is_authorized() {
            return Ok(false);
        }
        let count = source_file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        destination_file.write_all(&buffer[..count])?;
        if !is_authorized() {
            return Ok(false);
        }
    }
    destination_file.sync_all()?;
    Ok(is_authorized())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::SystemTime;
    use traesync_domain::{FileIdentity, ProcessRunningState};

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

    fn make_scan_request_for_path(
        fixture_root: &Path,
        storage_root: &Path,
        db_relative_path: &str,
    ) -> ScanRequest {
        ScanRequest {
            canonical_fixture_root: fixture_root.to_string_lossy().to_string(),
            db_relative_path: db_relative_path.to_string(),
            process_state: ProcessRunningState::NotRunning,
            now: SystemTime::UNIX_EPOCH,
            schema_fingerprint: "schema-fp-test".to_string(),
            mapping_version: "work_cn_v1".to_string(),
            product_version: "1.107.1".to_string(),
            account_evidence_ref: Some("evidence-ref-1".to_string()),
            storage_root: storage_root.to_string_lossy().to_string(),
        }
    }

    fn make_scan_request(fixture_root: &Path, storage_root: &Path) -> ScanRequest {
        make_scan_request_for_path(fixture_root, storage_root, "database.db")
    }

    #[test]
    fn chunked_snapshot_copy_stops_when_authorization_expires() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("source.bin");
        let destination = root.path().join("destination.bin");
        let payload = vec![0x2a_u8; (3 * 1024 * 1024) + 17];
        std::fs::write(&source, &payload).unwrap();
        let checks = AtomicUsize::new(0);

        let copied = copy_file_with_validation(&source, &destination, &|| {
            checks.fetch_add(1, Ordering::SeqCst) < 4
        })
        .unwrap();

        assert!(!copied, "授权撤销后分块复制必须中止");
        assert!(checks.load(Ordering::SeqCst) >= 4);
        assert!(destination.is_file());
        assert!(std::fs::metadata(&destination).unwrap().len() < payload.len() as u64);
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
                assert!(snapshot_meta.data_location_id.starts_with("loc-"));
                assert_ne!(
                    snapshot_meta.data_location_id,
                    fixture.path().to_string_lossy().as_ref()
                );
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
    fn authorization_expiring_before_publish_preserves_failure_without_publishing() {
        let fixture = tempfile::tempdir().unwrap();
        let storage = tempfile::tempdir().unwrap();
        std::fs::write(fixture.path().join("database.db"), b"db-content").unwrap();

        let checks = Arc::new(AtomicUsize::new(0));
        let store = FilesystemSnapshotStore::new(Box::new(MockFileIdentityProvider));
        let request = make_scan_request(fixture.path(), storage.path());
        let outcome = store.capture_snapshot_with_validation(&request, &|| {
            let check = checks.fetch_add(1, Ordering::SeqCst) + 1;
            // 第 14 次检查位于 staging 文件复制完成后、正式发布之前，模拟授权失效。
            check < 14
        });

        assert!(matches!(
            outcome,
            ScanOutcome::Failed {
                reason: ScanFailureReason::NotAuthorized
            }
        ));

        let snapshots = storage.path().join("snapshots");
        assert!(
            !snapshots.exists()
                || std::fs::read_dir(&snapshots)
                    .map(|mut entries| entries.next().is_none())
                    .unwrap_or(true),
            "发布前授权失效不得新增已发布快照"
        );

        let failure_root = storage.path().join("failure-staging");
        let failure_count = std::fs::read_dir(&failure_root)
            .map(|entries| entries.count())
            .unwrap_or(0);
        assert_eq!(failure_count, 1, "失败现场必须保留到 failure-staging");
        assert_eq!(checks.load(Ordering::SeqCst), 14);
    }

    #[test]
    fn authorization_expiring_after_publish_preserves_failure_without_publishing() {
        let fixture = tempfile::tempdir().unwrap();
        let storage = tempfile::tempdir().unwrap();
        std::fs::write(fixture.path().join("database.db"), b"db-content").unwrap();

        let store = FilesystemSnapshotStore::new(Box::new(MockFileIdentityProvider));
        let request = make_scan_request(fixture.path(), storage.path());
        let outcome = store.capture_snapshot_with_validation(&request, &|| {
            // 只有发布目录出现后才撤销授权，避免把实现细节的检查次数写死在测试里。
            let published = storage.path().join("snapshots");
            let has_published_snapshot = published.is_dir()
                && std::fs::read_dir(published)
                    .map(|mut entries| entries.next().is_some())
                    .unwrap_or(false);
            !has_published_snapshot
        });

        assert_eq!(
            outcome,
            ScanOutcome::Failed {
                reason: ScanFailureReason::NotAuthorized,
            }
        );

        let snapshots = storage.path().join("snapshots");
        assert!(
            !snapshots.exists()
                || std::fs::read_dir(&snapshots)
                    .map(|mut entries| entries.next().is_none())
                    .unwrap_or(true),
            "发布后授权失效不得保留正式快照"
        );
        let failure_root = storage.path().join("failure-staging");
        let failure_count = std::fs::read_dir(&failure_root)
            .map(|entries| entries.count())
            .unwrap_or(0);
        assert_eq!(failure_count, 1, "发布后的失败现场必须保留");
    }

    #[test]
    fn capture_succeeds_when_process_running() {
        // R1 修订（W0 实测 0/105 撕裂）：运行中捕获照常执行三件套快照复制。
        let fixture = tempfile::tempdir().unwrap();
        let storage = tempfile::tempdir().unwrap();
        std::fs::write(fixture.path().join("database.db"), b"db-content").unwrap();

        let store = FilesystemSnapshotStore::new(Box::new(MockFileIdentityProvider));
        let mut request = make_scan_request(fixture.path(), storage.path());
        request.process_state = ProcessRunningState::Running;
        let outcome = store.capture_snapshot(&request);

        assert!(
            !matches!(outcome, ScanOutcome::Failed { .. }),
            "运行中捕获不应失败，实际 {:?}",
            outcome
        );
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
    fn tampered_existing_snapshot_is_not_reused() {
        let fixture = tempfile::tempdir().unwrap();
        let storage = tempfile::tempdir().unwrap();
        std::fs::write(fixture.path().join("database.db"), b"same-content").unwrap();

        let store = FilesystemSnapshotStore::new(Box::new(MockFileIdentityProvider));
        let request = make_scan_request(fixture.path(), storage.path());
        let first_snapshot_id = match store.capture_snapshot(&request) {
            ScanOutcome::Success { snapshot_id, .. } => snapshot_id,
            other => panic!("首次捕获应成功，实际 {:?}", other),
        };
        let stored_db = storage
            .path()
            .join("snapshots")
            .join(first_snapshot_id.as_str())
            .join("database.db");
        std::fs::write(&stored_db, b"tampered-content").unwrap();

        match store.capture_snapshot(&request) {
            ScanOutcome::Success { snapshot_id, .. } => {
                assert_ne!(snapshot_id, first_snapshot_id, "被篡改快照不得去重复用");
            }
            other => panic!("被篡改快照应触发新捕获，实际 {:?}", other),
        }
        assert!(stored_db.is_file(), "旧快照和篡改证据不得自动删除");
    }

    #[test]
    fn changed_account_evidence_does_not_reuse_old_snapshot_metadata() {
        let fixture = tempfile::tempdir().unwrap();
        let storage = tempfile::tempdir().unwrap();
        std::fs::write(fixture.path().join("database.db"), b"same-content").unwrap();

        let store = FilesystemSnapshotStore::new(Box::new(MockFileIdentityProvider));
        let first_request = make_scan_request(fixture.path(), storage.path());
        let first_snapshot_id = match store.capture_snapshot(&first_request) {
            ScanOutcome::Success { snapshot_id, .. } => snapshot_id,
            other => panic!("首次捕获应成功，实际 {:?}", other),
        };

        let mut second_request = first_request.clone();
        second_request.account_evidence_ref = Some("different-account".to_string());
        match store.capture_snapshot(&second_request) {
            ScanOutcome::Success { snapshot_meta, .. } => {
                assert_ne!(snapshot_meta.snapshot_id, first_snapshot_id);
                assert_eq!(
                    snapshot_meta.account_evidence_ref.as_deref(),
                    Some("different-account")
                );
            }
            other => panic!("账号上下文变化应发布新快照，实际 {:?}", other),
        }
    }

    #[test]
    fn same_content_in_different_data_locations_is_not_deduplicated() {
        let first_fixture = tempfile::tempdir().unwrap();
        let second_fixture = tempfile::tempdir().unwrap();
        let storage = tempfile::tempdir().unwrap();
        std::fs::write(first_fixture.path().join("database.db"), b"same-content").unwrap();
        std::fs::write(second_fixture.path().join("database.db"), b"same-content").unwrap();

        let store = FilesystemSnapshotStore::new(Box::new(MockFileIdentityProvider));
        let first = match store
            .capture_snapshot(&make_scan_request(first_fixture.path(), storage.path()))
        {
            ScanOutcome::Success { snapshot_meta, .. } => snapshot_meta,
            other => panic!("第一个数据位置应成功，实际 {:?}", other),
        };
        let second = match store
            .capture_snapshot(&make_scan_request(second_fixture.path(), storage.path()))
        {
            ScanOutcome::Success { snapshot_meta, .. } => snapshot_meta,
            other => panic!("第二个数据位置不应复用快照，实际 {:?}", other),
        };

        assert_ne!(first.snapshot_id, second.snapshot_id);
        assert_ne!(first.data_location_id, second.data_location_id);
    }

    #[test]
    fn nested_database_path_is_copied_into_snapshot() {
        let fixture = tempfile::tempdir().unwrap();
        let storage = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(fixture.path().join("nested")).unwrap();
        std::fs::write(fixture.path().join("nested/database.db"), b"nested-db").unwrap();

        let store = FilesystemSnapshotStore::new(Box::new(MockFileIdentityProvider));
        let request =
            make_scan_request_for_path(fixture.path(), storage.path(), "nested/database.db");
        let snapshot_id = match store.capture_snapshot(&request) {
            ScanOutcome::Success { snapshot_meta, .. } => snapshot_meta.snapshot_id,
            other => panic!("嵌套数据库路径应成功，实际 {:?}", other),
        };

        assert!(storage
            .path()
            .join("snapshots")
            .join(snapshot_id.as_str())
            .join("nested/database.db")
            .is_file());
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
    use std::sync::Arc;

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

    /// 辅助：在第二次读取源 DB 文件身份时删除 WAL，使 post_source 读到 WAL 不存在。
    /// 根目录身份也会读取一次文件身份，因此不能依赖全局调用序号。
    struct DriftWalDisappearProvider {
        fixture_db_path: std::path::PathBuf,
        fixture_wal_path: std::path::PathBuf,
        db_read_count: AtomicUsize,
    }

    impl FileIdentityProvider for DriftWalDisappearProvider {
        fn read_file_identity(&self, path: &Path) -> Option<FileIdentity> {
            if path == self.fixture_db_path {
                let n = self.db_read_count.fetch_add(1, Ordering::SeqCst);
                // 第 2 次读取源 DB = post_source 阶段，此时复制已完成。
                if n == 1 {
                    let _ = std::fs::remove_file(&self.fixture_wal_path);
                }
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

    /// R11 辅助：identity 在 pre_capture 与 post_source 之间发生 Some/None 状态变化。
    /// - `SomeToNone`：第 0 次返回 Some，第 1 次返回 None（模拟身份读取变得不确定）
    /// - `NoneToSome`：第 0 次返回 None，第 1 次返回 Some
    /// 调用顺序：read_file_set 对 DB/WAL/SHM 各调用一次 read_file_identity；
    /// 仅 DB 存在时第 0 次为 pre_capture DB，第 1 次为 post_source DB。
    enum IdentityDriftMode {
        SomeToNone,
        NoneToSome,
    }

    struct CountingIdentityProvider {
        call_count: Arc<AtomicUsize>,
        mode: IdentityDriftMode,
    }

    impl FileIdentityProvider for CountingIdentityProvider {
        fn read_file_identity(&self, _path: &Path) -> Option<FileIdentity> {
            let n = self.call_count.fetch_add(1, Ordering::SeqCst);
            let sample = FileIdentity {
                volume_serial: 1,
                file_index_high: 0,
                file_index_low: 1,
            };
            match self.mode {
                IdentityDriftMode::SomeToNone => {
                    if n == 0 {
                        Some(sample)
                    } else {
                        None
                    }
                }
                IdentityDriftMode::NoneToSome => {
                    if n == 0 {
                        None
                    } else {
                        Some(sample)
                    }
                }
            }
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
            fixture_db_path: fixture.path().join("database.db"),
            fixture_wal_path: fixture.path().join("database.db-wal"),
            db_read_count: AtomicUsize::new(0),
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
        // identity 同为 None 时视为稳定（前后都读取不到，状态一致）
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

    // ============== R11 反例：identity Some/None 状态变化判定为 drift ==============

    #[test]
    fn r11_some_to_none_identity_judged_drift() {
        // Some(A) -> None：身份读取变得不确定，必须判定为 drift
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
            file_identity: None,
        }];
        assert!(
            !file_sets_source_stable(&pre, &post),
            "Some(A) -> None 必须判定为 drift"
        );
    }

    #[test]
    fn r11_none_to_some_identity_judged_drift() {
        // None -> Some(A)：身份读取变得不确定，必须判定为 drift
        let pre = vec![SnapshotFileEntry {
            kind: SnapshotFileKind::Db,
            relative_path: "database.db".to_string(),
            present: true,
            size: 100,
            sha256: "abc".to_string(),
            file_identity: None,
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
                file_index_low: 1,
            }),
        }];
        assert!(
            !file_sets_source_stable(&pre, &post),
            "None -> Some(A) 必须判定为 drift"
        );
    }

    #[test]
    fn r11_capture_returns_source_set_drift_on_identity_some_to_none() {
        // capture 级别反例：复制后 identity 由 Some 变 None，capture 返回 SourceSetDrift，
        // 且 staging 已清理、snapshots 中没有本次快照。
        let fixture = tempfile::tempdir().expect("create fixture tempdir");
        let storage = tempfile::tempdir().expect("create storage tempdir");
        let db_path = fixture.path().join("database.db");
        std::fs::write(&db_path, b"initial content").unwrap();

        // 计数器控制 file_identity 读取行为：pre_capture 后变 None
        let call_count = Arc::new(AtomicUsize::new(0));
        let provider = CountingIdentityProvider {
            call_count: call_count.clone(),
            // 第 1 次（pre_capture DB）返回 Some，第 2 次（post_source DB）返回 None
            // 第 3+ 次（staging 验证）无关——drift 已判定
            mode: IdentityDriftMode::SomeToNone,
        };
        let store = FilesystemSnapshotStore::new(Box::new(provider));
        let request = make_scan_request(fixture.path(), storage.path());
        let outcome = store.capture_snapshot(&request);

        match outcome {
            ScanOutcome::Failed {
                reason: ScanFailureReason::SourceSetDrift,
            } => {
                // staging 已清理
                let staging = storage.path().join("staging");
                let staging_clean = !staging.exists()
                    || std::fs::read_dir(&staging)
                        .map(|mut d| d.next().is_none())
                        .unwrap_or(true);
                assert!(staging_clean, "drift 后 staging 必须被清理");
                // snapshots 中没有本次快照
                let snapshots_dir = storage.path().join("snapshots");
                if snapshots_dir.exists() {
                    let count = std::fs::read_dir(&snapshots_dir)
                        .map(|d| d.count())
                        .unwrap_or(0);
                    assert_eq!(count, 0, "drift 后不得发布任何快照");
                }
            }
            other => panic!("期望 SourceSetDrift，实际 {:?}", other),
        }
    }

    #[test]
    fn r11_capture_returns_source_set_drift_on_identity_none_to_some() {
        // capture 级别反例：复制后 identity 由 None 变 Some，capture 返回 SourceSetDrift
        let fixture = tempfile::tempdir().expect("create fixture tempdir");
        let storage = tempfile::tempdir().expect("create storage tempdir");
        let db_path = fixture.path().join("database.db");
        std::fs::write(&db_path, b"initial content").unwrap();

        let call_count = Arc::new(AtomicUsize::new(0));
        let provider = CountingIdentityProvider {
            call_count: call_count.clone(),
            mode: IdentityDriftMode::NoneToSome,
        };
        let store = FilesystemSnapshotStore::new(Box::new(provider));
        let request = make_scan_request(fixture.path(), storage.path());
        let outcome = store.capture_snapshot(&request);

        match outcome {
            ScanOutcome::Failed {
                reason: ScanFailureReason::SourceSetDrift,
            } => {
                let staging = storage.path().join("staging");
                let staging_clean = !staging.exists()
                    || std::fs::read_dir(&staging)
                        .map(|mut d| d.next().is_none())
                        .unwrap_or(true);
                assert!(staging_clean, "drift 后 staging 必须被清理");
            }
            other => panic!("期望 SourceSetDrift，实际 {:?}", other),
        }
    }
}
/// 计算受路径防护验证后的文件 SHA-256；文件不存在或读取失败时返回 None。
pub fn sha256_file(path: &Path) -> Option<String> {
    sha256_file_with_validation(path, &|| true)
}

/// 分块计算 SHA-256，并在每个分块前后复核授权。
fn sha256_file_with_validation(path: &Path, is_authorized: &dyn Fn() -> bool) -> Option<String> {
    if !is_authorized() {
        return None;
    }
    let mut file = File::open(path).ok()?;
    let mut hasher = Sha256::new();
    // 快照前后校验会读取大数据库，缓冲区必须放在堆上。
    let mut buffer = vec![0_u8; 1024 * 1024];
    loop {
        if !is_authorized() {
            return None;
        }
        let count = file.read(&mut buffer).ok()?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
        if !is_authorized() {
            return None;
        }
    }
    if !is_authorized() {
        return None;
    }
    Some(hex::encode(hasher.finalize()))
}
