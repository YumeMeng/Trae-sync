//! T12/T13 迁移与目录库升级的固定恢复 journal。
//!
//! 大型数据仍留在各自的 staging 目录；固定恢复区只保存阶段、稳定身份和哈希摘要。
//! 发现非终态时只冻结为人工恢复，不猜测副作用是否已经发布，也不自动删除现场。

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum MigrationKind {
    StorageRoot,
    CatalogUpgrade,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum MigrationStage {
    Planned,
    Staging,
    StagedVerified,
    Publishing,
    Completed,
    ManualRecoveryRequired,
}

impl MigrationStage {
    pub(crate) fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::ManualRecoveryRequired)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct MigrationManifest {
    pub(crate) format_version: u32,
    pub(crate) operation_id: String,
    pub(crate) kind: MigrationKind,
    pub(crate) sequence: u64,
    pub(crate) stage: MigrationStage,
    pub(crate) source_id: String,
    pub(crate) source_bytes: u64,
    pub(crate) source_manifest_hash: String,
    pub(crate) staging_id: String,
    pub(crate) destination_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum MigrationManifestError {
    RecoveryRootUnavailable,
    InvalidManifest,
    Io,
}

impl std::fmt::Display for MigrationManifestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let message = match self {
            Self::RecoveryRootUnavailable => "固定恢复区不可用",
            Self::InvalidManifest => "迁移 manifest 损坏",
            Self::Io => "迁移 manifest 读写失败",
        };
        f.write_str(message)
    }
}

impl std::error::Error for MigrationManifestError {}

pub(crate) struct MigrationJournal {
    directory: PathBuf,
    latest: MigrationManifest,
}

impl MigrationJournal {
    pub(crate) fn create(
        recovery_root: &Path,
        kind: MigrationKind,
        source_id: impl Into<String>,
        source_bytes: u64,
        source_manifest_hash: impl Into<String>,
        staging_id: impl Into<String>,
    ) -> Result<Self, MigrationManifestError> {
        ensure_directory_chain(recovery_root)?;
        fs::create_dir_all(recovery_root).map_err(|_| MigrationManifestError::Io)?;
        ensure_directory_chain(recovery_root)?;

        let operation_id = format!("migration-{}-{}", now_nanos(), std::process::id());
        let directory = recovery_root
            .join("migration-manifests")
            .join(&operation_id);
        if fs::symlink_metadata(&directory).is_ok() {
            return Err(MigrationManifestError::Io);
        }
        fs::create_dir_all(&directory).map_err(|_| MigrationManifestError::Io)?;

        let journal = Self {
            directory,
            latest: MigrationManifest {
                format_version: 1,
                operation_id,
                kind,
                sequence: 0,
                stage: MigrationStage::Planned,
                source_id: source_id.into(),
                source_bytes,
                source_manifest_hash: source_manifest_hash.into(),
                staging_id: staging_id.into(),
                destination_id: None,
            },
        };
        journal.publish_current()?;
        Ok(journal)
    }

    pub(crate) fn transition(
        &mut self,
        stage: MigrationStage,
        destination_id: Option<String>,
    ) -> Result<(), MigrationManifestError> {
        if !valid_transition(self.latest.stage, stage) {
            return Err(MigrationManifestError::InvalidManifest);
        }
        self.latest.sequence = self.latest.sequence.saturating_add(1);
        self.latest.stage = stage;
        if destination_id.is_some() {
            self.latest.destination_id = destination_id;
        }
        self.publish_current()
    }

    #[cfg(test)]
    pub(crate) fn latest(&self) -> &MigrationManifest {
        &self.latest
    }

    fn publish_current(&self) -> Result<(), MigrationManifestError> {
        publish_record(&self.directory, &self.latest)
    }
}

/// 冻结指定类型的所有非终态迁移；返回 true 表示发现并冻结了现场。
pub(crate) fn freeze_unfinished(
    recovery_root: &Path,
    kind: MigrationKind,
) -> Result<bool, MigrationManifestError> {
    let root = recovery_root.join("migration-manifests");
    match fs::symlink_metadata(&root) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(_) => return Err(MigrationManifestError::RecoveryRootUnavailable),
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
            return Err(MigrationManifestError::RecoveryRootUnavailable)
        }
        Ok(_) => {}
    }

    let mut found = false;
    for entry in fs::read_dir(&root).map_err(|_| MigrationManifestError::Io)? {
        let entry = entry.map_err(|_| MigrationManifestError::Io)?;
        let metadata = entry.file_type().map_err(|_| MigrationManifestError::Io)?;
        if !metadata.is_dir() {
            return Err(MigrationManifestError::InvalidManifest);
        }
        let Some(latest) = read_latest(&entry.path())? else {
            return Err(MigrationManifestError::InvalidManifest);
        };
        if latest.kind != kind || latest.stage.is_terminal() {
            continue;
        }
        found = true;
        let mut journal = MigrationJournal {
            directory: entry.path(),
            latest,
        };
        journal.transition(MigrationStage::ManualRecoveryRequired, None)?;
    }
    Ok(found)
}

#[cfg(test)]
pub(crate) fn read_latest_for_test(
    recovery_root: &Path,
    operation_id: &str,
) -> Result<Option<MigrationManifest>, MigrationManifestError> {
    read_latest(&recovery_root.join("migration-manifests").join(operation_id))
}

fn read_latest(directory: &Path) -> Result<Option<MigrationManifest>, MigrationManifestError> {
    let mut latest: Option<MigrationManifest> = None;
    for entry in fs::read_dir(directory).map_err(|_| MigrationManifestError::Io)? {
        let entry = entry.map_err(|_| MigrationManifestError::Io)?;
        let metadata = entry.file_type().map_err(|_| MigrationManifestError::Io)?;
        if !metadata.is_file() {
            return Err(MigrationManifestError::InvalidManifest);
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        if !name.ends_with(".json") {
            continue;
        }
        let record: MigrationManifest = serde_json::from_reader(
            File::open(entry.path()).map_err(|_| MigrationManifestError::InvalidManifest)?,
        )
        .map_err(|_| MigrationManifestError::InvalidManifest)?;
        if record.format_version != 1 || record.operation_id.is_empty() {
            return Err(MigrationManifestError::InvalidManifest);
        }
        if latest
            .as_ref()
            .map_or(true, |current| record.sequence > current.sequence)
        {
            latest = Some(record);
        }
    }
    Ok(latest)
}

fn publish_record(
    directory: &Path,
    record: &MigrationManifest,
) -> Result<(), MigrationManifestError> {
    let destination = directory.join(format!("{:020}.json", record.sequence));
    let temporary = directory.join(format!(
        ".{:020}.tmp-{}",
        record.sequence,
        std::process::id()
    ));
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .map_err(|_| MigrationManifestError::Io)?;
        serde_json::to_writer(&mut file, record).map_err(|_| MigrationManifestError::Io)?;
        file.write_all(b"\n")
            .map_err(|_| MigrationManifestError::Io)?;
        file.sync_all().map_err(|_| MigrationManifestError::Io)?;
        drop(file);
        fs::rename(&temporary, destination).map_err(|_| MigrationManifestError::Io)
    })();
    if result.is_err() {
        // 失败的临时 journal 不是可恢复状态，立即删除，避免恢复区持续膨胀。
        let _ = fs::remove_file(&temporary);
    }
    result
}

fn valid_transition(previous: MigrationStage, next: MigrationStage) -> bool {
    matches!(
        (previous, next),
        (MigrationStage::Planned, MigrationStage::Staging)
            | (MigrationStage::Staging, MigrationStage::StagedVerified)
            | (MigrationStage::StagedVerified, MigrationStage::Publishing)
            | (MigrationStage::Publishing, MigrationStage::Completed)
            | (
                MigrationStage::Planned
                    | MigrationStage::Staging
                    | MigrationStage::StagedVerified
                    | MigrationStage::Publishing,
                MigrationStage::ManualRecoveryRequired
            )
    )
}

fn ensure_directory_chain(path: &Path) -> Result<(), MigrationManifestError> {
    let mut current = Some(path);
    while let Some(candidate) = current {
        match fs::symlink_metadata(candidate) {
            Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
                return Err(MigrationManifestError::RecoveryRootUnavailable)
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return Err(MigrationManifestError::RecoveryRootUnavailable),
        }
        let Some(parent) = candidate.parent() else {
            break;
        };
        if parent == candidate {
            break;
        }
        current = Some(parent);
    }
    Ok(())
}

fn now_nanos() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_nanos())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn journal_is_append_only_and_freezes_unfinished_state() {
        let root = tempfile::tempdir().unwrap();
        let mut journal = MigrationJournal::create(
            root.path(),
            MigrationKind::StorageRoot,
            "root-source",
            10,
            "manifest-hash",
            "staging-dir",
        )
        .unwrap();
        journal.transition(MigrationStage::Staging, None).unwrap();
        let operation_id = journal.latest().operation_id.clone();
        assert_eq!(
            read_latest_for_test(root.path(), &operation_id)
                .unwrap()
                .unwrap()
                .stage,
            MigrationStage::Staging
        );

        assert!(freeze_unfinished(root.path(), MigrationKind::StorageRoot).unwrap());
        let latest = read_latest_for_test(root.path(), &operation_id)
            .unwrap()
            .unwrap();
        assert_eq!(latest.stage, MigrationStage::ManualRecoveryRequired);
        assert_eq!(latest.sequence, 2);
        assert!(!freeze_unfinished(root.path(), MigrationKind::StorageRoot).unwrap());
    }

    #[test]
    fn different_migration_kind_does_not_freeze_other_journal() {
        let root = tempfile::tempdir().unwrap();
        let mut journal = MigrationJournal::create(
            root.path(),
            MigrationKind::CatalogUpgrade,
            "catalog-source",
            10,
            "catalog-hash",
            "generation-staging",
        )
        .unwrap();
        journal.transition(MigrationStage::Staging, None).unwrap();
        assert!(!freeze_unfinished(root.path(), MigrationKind::StorageRoot).unwrap());
        assert_eq!(journal.latest().stage, MigrationStage::Staging);
    }

    #[test]
    fn failed_journal_publication_removes_temporary_file() {
        let root = tempfile::tempdir().unwrap();
        let destination = root.path().join("00000000000000000000.json");
        fs::create_dir(&destination).unwrap();
        let record = MigrationManifest {
            format_version: 1,
            operation_id: "migration-test".to_string(),
            kind: MigrationKind::StorageRoot,
            sequence: 0,
            stage: MigrationStage::Planned,
            source_id: "source-test".to_string(),
            source_bytes: 0,
            source_manifest_hash: "hash-test".to_string(),
            staging_id: "staging-test".to_string(),
            destination_id: None,
        };

        assert!(publish_record(root.path(), &record).is_err());
        assert!(!root.path().read_dir().unwrap().any(|entry| {
            entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".00000000000000000000.tmp-")
        }));
    }
}
