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
        };
        journal.transition(OperationState::Planned).ok()?;
        Some(journal)
    }

    /// 将下一状态作为全新文件原子发布；失败时保留上一条已发布状态。
    pub(crate) fn transition(&self, state: OperationState) -> Result<(), ()> {
        let sequence = self
            .latest_record()
            .map(|record| record.sequence + 1)
            .unwrap_or(0);
        let record = PersistedOperationState {
            sequence,
            operation_id: self.operation_id.clone(),
            data_location_id: self.data_location_id.clone(),
            target_file_evidence: self.target_file_evidence.clone(),
            state,
        };
        publish_record(&self.directory, &record)
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

/// 协调固定恢复区中所有未终态操作；只对可证明未写目标的阶段自动收口。
pub(crate) fn reconcile_unfinished_manifests(storage_root: &Path) -> bool {
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
        };
        let terminal = match record.state {
            OperationState::Planned
            | OperationState::BackingUp
            | OperationState::BackupVerified => OperationState::NotApplied,
            // 进入写入意图后，崩溃点可能位于提交前、提交后或验证中；不猜测并不覆盖。
            _ => OperationState::ManualRecoveryRequired,
        };
        if journal.transition(terminal).is_err() {
            return false;
        }
    }

    true
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
    use tempfile::tempdir;
    use traesync_domain::{OperationId, OperationState, TargetFileEvidence};

    use super::{reconcile_unfinished_manifests, OperationManifestJournal};

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

        assert!(reconcile_unfinished_manifests(storage.path()));
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

        assert!(reconcile_unfinished_manifests(storage.path()));
        assert_eq!(
            journal.latest_state(),
            Some(OperationState::ManualRecoveryRequired),
            "写入意图已持久化但结果未知时不得自动覆盖目标库"
        );
    }
}
