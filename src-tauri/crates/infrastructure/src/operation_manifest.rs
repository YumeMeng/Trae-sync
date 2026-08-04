//! T06 固定恢复区的追加式操作 manifest；仅保存状态与非敏感证据摘要。

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use traesync_domain::{OperationId, OperationState, TargetFileEvidence};

/// 单条不可变状态记录；不写入路径、密钥、账号正文或数据库内容。
#[derive(Debug, Clone, Serialize, Deserialize)]
struct PersistedOperationState {
    sequence: u64,
    operation_id: String,
    data_location_id: String,
    target_file_evidence: TargetFileEvidence,
    #[serde(default)]
    verified_target_file_evidence: Option<TargetFileEvidence>,
    state: OperationState,
}

/// 固定恢复区内单次操作的状态 journal。
///
/// 每次转换发布一个新文件，不就地覆盖旧状态，从而避免 Windows 上替换已有文件时
/// 出现 manifest 缺失窗口。恢复时只读取编号最大的完整 JSON 文件。
pub(crate) struct OperationManifestJournal {
    directory: PathBuf,
    operation_id: String,
    data_location_id: String,
    target_file_evidence: TargetFileEvidence,
    verified_target_file_evidence: Option<TargetFileEvidence>,
}

impl OperationManifestJournal {
    /// 创建新操作 journal，并先持久化 `planned` 意图，之后才允许任何副作用。
    pub(crate) fn create(
        storage_root: &Path,
        operation_id: &OperationId,
        data_location_id: &str,
        target_file_evidence: &TargetFileEvidence,
    ) -> Option<Self> {
        let directory = storage_root.join("operations").join(operation_id.as_str());
        fs::create_dir_all(directory.parent()?).ok()?;
        fs::create_dir(&directory).ok()?;

        let journal = Self {
            directory,
            operation_id: operation_id.as_str().to_string(),
            data_location_id: data_location_id.to_string(),
            target_file_evidence: target_file_evidence.clone(),
            verified_target_file_evidence: None,
        };
        journal.transition(OperationState::Planned).ok()?;
        Some(journal)
    }

    /// 将下一状态作为全新文件原子发布；失败时保留上一条已发布状态。
    pub(crate) fn transition(&self, state: OperationState) -> Result<(), ()> {
        self.transition_with_verified_target_file_evidence(
            state,
            self.verified_target_file_evidence.clone(),
        )
    }

    /// 在新连接验证后持久化目标三件套指纹；重启只能在指纹仍匹配时收口完成。
    pub(crate) fn transition_catalog_reconciling(
        &self,
        verified_target_file_evidence: &TargetFileEvidence,
    ) -> Result<(), ()> {
        self.transition_with_verified_target_file_evidence(
            OperationState::CatalogReconciling,
            Some(verified_target_file_evidence.clone()),
        )
    }

    fn transition_with_verified_target_file_evidence(
        &self,
        state: OperationState,
        verified_target_file_evidence: Option<TargetFileEvidence>,
    ) -> Result<(), ()> {
        let sequence = self
            .latest_record()
            .map(|record| record.sequence + 1)
            .unwrap_or(0);
        let record = PersistedOperationState {
            sequence,
            operation_id: self.operation_id.clone(),
            data_location_id: self.data_location_id.clone(),
            target_file_evidence: self.target_file_evidence.clone(),
            verified_target_file_evidence,
            state,
        };
        publish_record(&self.directory, &record)
    }

    /// 返回既有 journal 的操作 ID，仅供恢复区定位已保留的证据目录。
    pub(crate) fn operation_id(&self) -> &str {
        &self.operation_id
    }

    /// 返回绑定的数据位置；恢复调用方必须拒绝为其他位置捕获失败现场。
    pub(crate) fn data_location_id(&self) -> &str {
        &self.data_location_id
    }

    /// 返回已通过新连接验证的目标三件套指纹；缺失时不能自动完成。
    pub(crate) fn verified_target_file_evidence(&self) -> Option<&TargetFileEvidence> {
        self.verified_target_file_evidence.as_ref()
    }

    /// 读取最后一个完整状态；临时或损坏文件不会被误认为已完成状态。
    #[cfg(test)]
    pub(crate) fn latest_state(&self) -> Option<OperationState> {
        self.latest_record().map(|record| record.state)
    }

    fn latest_record(&self) -> Option<PersistedOperationState> {
        read_latest_record(&self.directory)
    }
}

/// 协调未知写后状态前由调用方复验完成凭据或捕获失败现场。
pub(crate) fn reconcile_unfinished_manifests_with_recovery_handlers<V, F>(
    storage_root: &Path,
    mut verify_catalog: V,
    mut capture_failure: F,
) -> bool
where
    V: FnMut(&OperationManifestJournal) -> bool,
    F: FnMut(&OperationManifestJournal) -> bool,
{
    let operations_dir = storage_root.join("operations");
    if !operations_dir.exists() {
        return true;
    }

    let entries = match fs::read_dir(&operations_dir) {
        Ok(entries) => entries,
        Err(_) => return false,
    };
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(_) => return false,
        };
        let file_type = match entry.file_type() {
            Ok(file_type) => file_type,
            Err(_) => return false,
        };
        if !file_type.is_dir() {
            return false;
        }
        let record = match read_latest_record(&entry.path()) {
            Some(record) => record,
            None => return false,
        };
        if record.state.is_terminal() {
            continue;
        }

        let journal = OperationManifestJournal {
            directory: entry.path(),
            operation_id: record.operation_id,
            data_location_id: record.data_location_id,
            target_file_evidence: record.target_file_evidence,
            verified_target_file_evidence: record.verified_target_file_evidence,
        };
        let terminal = match record.state {
            OperationState::Planned
            | OperationState::BackingUp
            | OperationState::BackupVerified => OperationState::NotApplied,
            OperationState::CatalogReconciling if verify_catalog(&journal) => OperationState::Completed,
            // 写后状态无法独立复核，或完成凭据已漂移时，必须先冻结失败现场。
            _ => {
                // 失败现场未保存时保留原非终态，供下次启动再次捕获；绝不伪造收口。
                if !capture_failure(&journal) {
                    return false;
                }
                OperationState::ManualRecoveryRequired
            }
        };
        if journal.transition(terminal).is_err() {
            return false;
        }
    }

    true
}

/// 仅供 manifest 单元测试验证：失败现场捕获器不能将目录误判为已完成。
#[cfg(test)]
pub(crate) fn reconcile_unfinished_manifests_with_failure_capture<F>(
    storage_root: &Path,
    capture_failure: F,
) -> bool
where
    F: FnMut(&OperationManifestJournal) -> bool,
{
    reconcile_unfinished_manifests_with_recovery_handlers(storage_root, |_| false, capture_failure)
}

/// 任一旧操作需要人工恢复时，禁止在同一恢复区开始新的目标副作用。
pub(crate) fn has_manual_recovery_required(storage_root: &Path) -> bool {
    let operations_dir = storage_root.join("operations");
    let entries = match fs::read_dir(&operations_dir) {
        Ok(entries) => entries,
        Err(_) => return false,
    };
    entries.filter_map(Result::ok).any(|entry| {
        entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false)
            && read_latest_record(&entry.path())
                .map(|record| record.state == OperationState::ManualRecoveryRequired)
                .unwrap_or(true)
    })
}

/// 将未发布临时文件写入并同步后改名为新状态文件；目标文件从不预先存在。
fn publish_record(directory: &Path, record: &PersistedOperationState) -> Result<(), ()> {
    let file_name = format!("{:020}.json", record.sequence);
    let destination = directory.join(file_name);
    let temporary = directory.join(format!(
        ".{:020}.tmp-{}",
        record.sequence,
        std::process::id()
    ));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)
        .map_err(|_| ())?;
    serde_json::to_writer(&mut file, record).map_err(|_| ())?;
    file.write_all(b"\n").map_err(|_| ())?;
    file.sync_all().map_err(|_| ())?;
    drop(file);

    // Windows 不允许把目录作为普通文件句柄同步；文件已同步后再同卷改名，
    // 崩溃时恢复逻辑仍只接受上一条完整发布记录。
    fs::rename(&temporary, &destination).map_err(|_| ())?;
    Ok(())
}

/// 仅接受连续命名的完整 JSON 状态文件；临时文件、空目录或损坏内容均失败关闭。
fn read_latest_record(directory: &Path) -> Option<PersistedOperationState> {
    let mut records = Vec::new();
    for entry in fs::read_dir(directory).ok()? {
        let entry = entry.ok()?;
        if !entry.file_type().ok()?.is_file() {
            return None;
        }
        let name = entry.file_name();
        let name = name.to_str()?;
        if name.starts_with('.') && name.contains(".tmp-") {
            continue;
        }
        if !name.ends_with(".json") {
            return None;
        }
        let record: PersistedOperationState =
            serde_json::from_reader(File::open(entry.path()).ok()?).ok()?;
        if name != format!("{:020}.json", record.sequence) {
            return None;
        }
        records.push(record);
    }
    records.sort_by_key(|record| record.sequence);
    let latest = records.last()?.clone();
    if records
        .iter()
        .enumerate()
        .any(|(index, record)| record.sequence != index as u64)
    {
        return None;
    }
    Some(latest)
}

#[cfg(test)]
mod tests {
    use std::process::Command;

    use tempfile::tempdir;
    use traesync_domain::{OperationId, OperationState, TargetFileEvidence};

    use super::{
        read_latest_record, reconcile_unfinished_manifests_with_failure_capture,
        reconcile_unfinished_manifests_with_recovery_handlers, OperationManifestJournal,
    };

    fn evidence() -> TargetFileEvidence {
        TargetFileEvidence {
            db_fingerprint: "fixture-db".to_string(),
            wal_fingerprint: None,
            shm_fingerprint: None,
        }
    }

    #[test]
    fn reconciliation_marks_prewrite_intent_not_applied() {
        let storage = tempdir().unwrap();
        let journal = OperationManifestJournal::create(
            storage.path(),
            &OperationId::new(),
            "fixture-location",
            &evidence(),
        )
        .unwrap();
        journal.transition(OperationState::BackingUp).unwrap();

        assert!(reconcile_unfinished_manifests_with_failure_capture(
            storage.path(),
            |_| panic!("写前状态不得请求失败现场捕获")
        ));
        assert_eq!(journal.latest_state(), Some(OperationState::NotApplied));
    }

    #[test]
    fn reconciliation_requires_manual_recovery_after_write_intent() {
        let storage = tempdir().unwrap();
        let journal = OperationManifestJournal::create(
            storage.path(),
            &OperationId::new(),
            "fixture-location",
            &evidence(),
        )
        .unwrap();
        journal.transition(OperationState::BackupVerified).unwrap();
        journal.transition(OperationState::TargetWriting).unwrap();

        assert!(reconcile_unfinished_manifests_with_failure_capture(
            storage.path(),
            |_| true
        ));
        assert_eq!(
            journal.latest_state(),
            Some(OperationState::ManualRecoveryRequired),
            "写入意图已持久化但结果未知时不得自动覆盖目标库"
        );
    }

    #[test]
    fn reconciliation_assigns_one_terminal_outcome_to_every_nonterminal_state() {
        let cases = [
            (OperationState::Planned, OperationState::NotApplied),
            (OperationState::BackingUp, OperationState::NotApplied),
            (OperationState::BackupVerified, OperationState::NotApplied),
            (
                OperationState::TargetWriting,
                OperationState::ManualRecoveryRequired,
            ),
            (
                OperationState::TargetCommittedUnverified,
                OperationState::ManualRecoveryRequired,
            ),
            (
                OperationState::TargetVerifying,
                OperationState::ManualRecoveryRequired,
            ),
            (
                OperationState::CatalogReconciling,
                OperationState::ManualRecoveryRequired,
            ),
            (
                OperationState::VerificationInconclusive,
                OperationState::ManualRecoveryRequired,
            ),
            (
                OperationState::FailurePreserving,
                OperationState::ManualRecoveryRequired,
            ),
            (
                OperationState::FailureSnapshotVerified,
                OperationState::ManualRecoveryRequired,
            ),
            (
                OperationState::RestoreStaging,
                OperationState::ManualRecoveryRequired,
            ),
            (
                OperationState::RestoreStaged,
                OperationState::ManualRecoveryRequired,
            ),
            (
                OperationState::RestoreReplacing,
                OperationState::ManualRecoveryRequired,
            ),
            (
                OperationState::RestoredVerifying,
                OperationState::ManualRecoveryRequired,
            ),
        ];

        for (nonterminal, expected_terminal) in cases {
            let storage = tempdir().unwrap();
            let journal = OperationManifestJournal::create(
                storage.path(),
                &OperationId::new(),
                "fixture-location",
                &evidence(),
            )
            .unwrap();
            if nonterminal != OperationState::Planned {
                journal.transition(nonterminal).unwrap();
            }

            assert!(reconcile_unfinished_manifests_with_failure_capture(
                storage.path(),
                |_| true
            ));
            assert_eq!(journal.latest_state(), Some(expected_terminal));
            assert!(reconcile_unfinished_manifests_with_failure_capture(
                storage.path(),
                |_| true
            ));
            assert_eq!(
                journal.latest_state(),
                Some(expected_terminal),
                "终态重启协调不得重放或覆盖既有结果"
            );
        }
    }

    #[test]
    fn reconciliation_preserves_evidence_and_appends_one_terminal_record_per_interruption() {
        let cases = [
            OperationState::Planned,
            OperationState::BackingUp,
            OperationState::BackupVerified,
            OperationState::TargetWriting,
            OperationState::TargetCommittedUnverified,
            OperationState::TargetVerifying,
            OperationState::CatalogReconciling,
            OperationState::VerificationInconclusive,
            OperationState::FailurePreserving,
            OperationState::FailureSnapshotVerified,
            OperationState::RestoreStaging,
            OperationState::RestoreStaged,
            OperationState::RestoreReplacing,
            OperationState::RestoredVerifying,
        ];

        for nonterminal in cases {
            let storage = tempdir().unwrap();
            let operation_id = OperationId::new();
            let journal = OperationManifestJournal::create(
                storage.path(),
                &operation_id,
                "fixture-location",
                &evidence(),
            )
            .unwrap();
            if nonterminal != OperationState::Planned {
                journal.transition(nonterminal).unwrap();
            }
            // 固定三类证据，协调只能追加 manifest，绝不能清理任何现场。
            let backup_root = storage.path().join("backups").join(operation_id.as_str());
            let before_raw = backup_root.join("before").join("raw");
            let before_logical = backup_root.join("before").join("logical");
            let failure_raw = backup_root.join("failure").join("raw");
            std::fs::create_dir_all(&before_raw).unwrap();
            std::fs::create_dir_all(&before_logical).unwrap();
            std::fs::create_dir_all(&failure_raw).unwrap();
            std::fs::write(before_raw.join("database.db"), b"original-db").unwrap();
            std::fs::write(before_raw.join("database.db-wal"), b"original-wal").unwrap();
            std::fs::write(before_raw.join("database.db-shm"), b"original-shm").unwrap();
            std::fs::write(before_logical.join("database.db"), b"logical-backup").unwrap();
            std::fs::write(failure_raw.join("database.db"), b"failure-scene").unwrap();
            let record_count_before = std::fs::read_dir(&journal.directory).unwrap().count();

            assert!(reconcile_unfinished_manifests_with_failure_capture(
                storage.path(),
                |_| true
            ));
            assert_eq!(
                std::fs::read_dir(&journal.directory).unwrap().count(),
                record_count_before + 1,
                "每次中断协调只能追加一个终态记录"
            );
            assert_eq!(std::fs::read(before_raw.join("database.db")).unwrap(), b"original-db");
            assert_eq!(std::fs::read(before_raw.join("database.db-wal")).unwrap(), b"original-wal");
            assert_eq!(std::fs::read(before_raw.join("database.db-shm")).unwrap(), b"original-shm");
            assert_eq!(std::fs::read(before_logical.join("database.db")).unwrap(), b"logical-backup");
            assert_eq!(std::fs::read(failure_raw.join("database.db")).unwrap(), b"failure-scene");

            assert!(reconcile_unfinished_manifests_with_failure_capture(
                storage.path(),
                |_| true
            ));
            assert_eq!(
                std::fs::read_dir(&journal.directory).unwrap().count(),
                record_count_before + 1,
                "终态重启不得再次追加记录或重放事务"
            );
        }
    }

    #[test]
    fn reconciliation_captures_failure_scene_for_every_unknown_postwrite_state() {
        let cases = [
            OperationState::TargetWriting,
            OperationState::TargetCommittedUnverified,
            OperationState::TargetVerifying,
            OperationState::VerificationInconclusive,
            OperationState::FailurePreserving,
            OperationState::FailureSnapshotVerified,
            OperationState::RestoreStaging,
            OperationState::RestoreStaged,
            OperationState::RestoreReplacing,
            OperationState::RestoredVerifying,
        ];

        for nonterminal in cases {
            let storage = tempdir().unwrap();
            let journal = OperationManifestJournal::create(
                storage.path(),
                &OperationId::new(),
                "fixture-location",
                &evidence(),
            )
            .unwrap();
            journal.transition(nonterminal).unwrap();
            let captures = std::cell::Cell::new(0_u8);

            assert!(reconcile_unfinished_manifests_with_failure_capture(
                storage.path(),
                |_journal| {
                    captures.set(captures.get() + 1);
                    true
                }
            ));
            assert_eq!(captures.get(), 1, "未知写后状态必须先捕获失败现场");
            assert_eq!(
                journal.latest_state(),
                Some(OperationState::ManualRecoveryRequired)
            );
        }
    }

    #[test]
    fn reconciliation_keeps_postwrite_manifest_nonterminal_when_failure_capture_fails() {
        let storage = tempdir().unwrap();
        let journal = OperationManifestJournal::create(
            storage.path(),
            &OperationId::new(),
            "fixture-location",
            &evidence(),
        )
        .unwrap();
        journal.transition(OperationState::TargetWriting).unwrap();

        assert!(
            !reconcile_unfinished_manifests_with_failure_capture(storage.path(), |_| false),
            "失败现场未捕获时不得把未知写后状态伪装成已收口终态"
        );
        assert_eq!(journal.latest_state(), Some(OperationState::TargetWriting));
    }

    #[test]
    fn reconciliation_completes_catalog_only_with_reverified_commit_evidence() {
        let storage = tempdir().unwrap();
        let journal = OperationManifestJournal::create(
            storage.path(),
            &OperationId::new(),
            "fixture-location",
            &evidence(),
        )
        .unwrap();
        let verified_target = TargetFileEvidence {
            db_fingerprint: "verified-db".to_string(),
            wal_fingerprint: Some("verified-wal".to_string()),
            shm_fingerprint: Some("verified-shm".to_string()),
        };
        journal
            .transition_catalog_reconciling(&verified_target)
            .unwrap();

        assert!(reconcile_unfinished_manifests_with_recovery_handlers(
            storage.path(),
            |persisted| persisted.verified_target_file_evidence() == Some(&verified_target),
            |_| panic!("可复验的目录收口不得捕获失败现场")
        ));
        assert_eq!(journal.latest_state(), Some(OperationState::Completed));
    }

    /// 子进程在指定状态落盘后直接终止，用于覆盖真实进程中断而非仅内存模拟。
    #[test]
    #[ignore]
    fn crash_child_after_persisting_requested_nonterminal_state() {
        let storage = std::env::var_os("TRAE_SYNC_CRASH_STORAGE")
            .map(std::path::PathBuf::from)
            .expect("父测试必须传入隔离恢复目录");
        let state = match std::env::var("TRAE_SYNC_CRASH_STATE").as_deref() {
            Ok("planned") => OperationState::Planned,
            Ok("backing_up") => OperationState::BackingUp,
            Ok("backup_verified") => OperationState::BackupVerified,
            Ok("target_writing") => OperationState::TargetWriting,
            Ok("target_committed_unverified") => OperationState::TargetCommittedUnverified,
            Ok("target_verifying") => OperationState::TargetVerifying,
            Ok("catalog_reconciling") => OperationState::CatalogReconciling,
            Ok("verification_inconclusive") => OperationState::VerificationInconclusive,
            Ok("failure_preserving") => OperationState::FailurePreserving,
            Ok("failure_snapshot_verified") => OperationState::FailureSnapshotVerified,
            Ok("restore_staging") => OperationState::RestoreStaging,
            Ok("restore_staged") => OperationState::RestoreStaged,
            Ok("restore_replacing") => OperationState::RestoreReplacing,
            Ok("restored_verifying") => OperationState::RestoredVerifying,
            other => panic!("未知测试崩溃状态: {other:?}"),
        };
        let journal = OperationManifestJournal::create(
            &storage,
            &OperationId::new(),
            "fixture-location",
            &evidence(),
        )
        .unwrap();
        let backup_root = storage.join("backups").join(journal.operation_id());
        let before_raw = backup_root.join("before").join("raw");
        let before_logical = backup_root.join("before").join("logical");
        std::fs::create_dir_all(&before_raw).unwrap();
        std::fs::create_dir_all(&before_logical).unwrap();
        std::fs::write(before_raw.join("database.db"), b"original-db").unwrap();
        std::fs::write(before_raw.join("database.db-wal"), b"original-wal").unwrap();
        std::fs::write(before_raw.join("database.db-shm"), b"original-shm").unwrap();
        std::fs::write(before_logical.join("database.db"), b"logical-backup").unwrap();
        if state == OperationState::CatalogReconciling {
            journal.transition_catalog_reconciling(&evidence()).unwrap();
        } else if state != OperationState::Planned {
            journal.transition(state).unwrap();
        }
        std::process::exit(86);
    }

    #[test]
    fn reconciliation_survives_actual_child_termination_for_every_nonterminal_state() {
        let cases = [
            ("planned", OperationState::NotApplied),
            ("backing_up", OperationState::NotApplied),
            ("backup_verified", OperationState::NotApplied),
            ("target_writing", OperationState::ManualRecoveryRequired),
            (
                "target_committed_unverified",
                OperationState::ManualRecoveryRequired,
            ),
            ("target_verifying", OperationState::ManualRecoveryRequired),
            ("catalog_reconciling", OperationState::Completed),
            (
                "verification_inconclusive",
                OperationState::ManualRecoveryRequired,
            ),
            (
                "failure_preserving",
                OperationState::ManualRecoveryRequired,
            ),
            (
                "failure_snapshot_verified",
                OperationState::ManualRecoveryRequired,
            ),
            ("restore_staging", OperationState::ManualRecoveryRequired),
            ("restore_staged", OperationState::ManualRecoveryRequired),
            ("restore_replacing", OperationState::ManualRecoveryRequired),
            ("restored_verifying", OperationState::ManualRecoveryRequired),
        ];

        for (state_name, expected_terminal) in cases {
            let storage = tempdir().unwrap();
            let target_scene = storage.path().join("target-after-crash.db");
            std::fs::write(&target_scene, b"target-after-crash").unwrap();
            let status = Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "operation_manifest::tests::crash_child_after_persisting_requested_nonterminal_state",
                    "--ignored",
                    "--nocapture",
                ])
                .env("TRAE_SYNC_CRASH_STORAGE", storage.path())
                .env("TRAE_SYNC_CRASH_STATE", state_name)
                .status()
                .unwrap();
            assert_eq!(status.code(), Some(86), "子进程必须在状态落盘后终止");

            let operation_dir = std::fs::read_dir(storage.path().join("operations"))
                .unwrap()
                .next()
                .unwrap()
                .unwrap()
                .path();
            let operation_id = operation_dir.file_name().unwrap().to_string_lossy().to_string();
            assert!(reconcile_unfinished_manifests_with_recovery_handlers(
                storage.path(),
                |journal| {
                    state_name == "catalog_reconciling"
                        && journal.verified_target_file_evidence() == Some(&evidence())
                },
                |journal| {
                    let failure_raw = storage
                        .path()
                        .join("backups")
                        .join(journal.operation_id())
                        .join("failure")
                        .join("raw");
                    std::fs::create_dir_all(&failure_raw).is_ok()
                        && std::fs::copy(&target_scene, failure_raw.join("database.db")).is_ok()
                }
            ));
            let records_after_first_restart = std::fs::read_dir(&operation_dir).unwrap().count();
            let latest = read_latest_record(&operation_dir).unwrap();
            assert_eq!(latest.state, expected_terminal);
            let before_raw = storage
                .path()
                .join("backups")
                .join(&operation_id)
                .join("before")
                .join("raw");
            let before_logical = storage
                .path()
                .join("backups")
                .join(&operation_id)
                .join("before")
                .join("logical");
            assert_eq!(std::fs::read(before_raw.join("database.db")).unwrap(), b"original-db");
            assert_eq!(
                std::fs::read(before_logical.join("database.db")).unwrap(),
                b"logical-backup"
            );
            if expected_terminal == OperationState::ManualRecoveryRequired {
                assert_eq!(
                    std::fs::read(
                        storage
                            .path()
                            .join("backups")
                            .join(&operation_id)
                            .join("failure")
                            .join("raw")
                            .join("database.db")
                    )
                    .unwrap(),
                    b"target-after-crash"
                );
            }
            assert!(reconcile_unfinished_manifests_with_recovery_handlers(
                storage.path(),
                |_| panic!("终态重启不得再次复验或收口") ,
                |_| panic!("终态重启不得再次捕获现场或重放事务")
            ));
            assert_eq!(
                std::fs::read_dir(&operation_dir).unwrap().count(),
                records_after_first_restart,
                "终态重启不得再次追加状态或重放事务"
            );
        }
    }
}
