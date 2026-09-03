//! T06 固定恢复区的追加式操作 manifest；仅保存状态与非敏感证据摘要。

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use traesync_domain::{OperationId, OperationState, TargetFileEvidence};

/// 操作摘要只暴露恢复区中可安全展示的状态，不包含路径、密钥或数据库内容。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OperationSummary {
    pub operation_id: String,
    pub data_location_id: String,
    pub state: OperationState,
    pub sequence: u64,
    pub has_verified_target_file_evidence: bool,
}

/// 未完成操作协调的白名单状态摘要；不携带 operation_id、路径或证据正文。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OperationReconciliationStatus {
    NoUnfinishedOperations,
    Reconciled,
    ManualRecoveryRequired,
    OtherDataLocationPending,
}

/// 按当前数据位置协调 manifest 后返回的数量摘要。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct OperationReconciliationSummary {
    pub inspected_count: u64,
    pub reconciled_count: u64,
    pub not_applied_count: u64,
    pub completed_count: u64,
    pub manual_recovery_required_count: u64,
    pub unrelated_data_location_count: u64,
    pub status: OperationReconciliationStatus,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OperationManifestError {
    StorageRootUnavailable,
    DirectoryUnreadable,
    InvalidManifest,
    ReconciliationFailed,
}

impl std::fmt::Display for OperationManifestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let message = match self {
            Self::StorageRootUnavailable => "恢复区不可用",
            Self::DirectoryUnreadable => "操作记录目录不可读",
            Self::InvalidManifest => "操作记录损坏",
            Self::ReconciliationFailed => "操作记录协调失败",
        };
        f.write_str(message)
    }
}

impl std::error::Error for OperationManifestError {}

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
pub struct OperationManifestJournal {
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
        let latest = self.latest_record();
        let previous_state = latest.as_ref().map(|record| record.state);
        if !is_valid_transition(previous_state, state) {
            return Err(());
        }
        let sequence = latest
            .as_ref()
            .map(|record| record.sequence + 1)
            .unwrap_or(0);
        let verified_target_file_evidence = verified_target_file_evidence
            .or_else(|| {
                latest
                    .as_ref()
                    .and_then(|record| record.verified_target_file_evidence.clone())
            })
            .or_else(|| self.verified_target_file_evidence.clone());
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
    pub fn operation_id(&self) -> &str {
        &self.operation_id
    }

    /// 返回绑定的数据位置；恢复调用方必须拒绝为其他位置捕获失败现场。
    pub fn data_location_id(&self) -> &str {
        &self.data_location_id
    }

    /// 返回已通过新连接验证的目标三件套指纹；缺失时不能自动完成。
    pub fn verified_target_file_evidence(&self) -> Option<&TargetFileEvidence> {
        self.verified_target_file_evidence.as_ref()
    }

    /// 读取最后一个完整状态；临时或损坏文件不会被误认为已完成状态。
    ///
    /// 恢复协调需要根据中断阶段选择幂等的失败现场动作，因此生产恢复路径
    /// 也必须读取这个状态，而不能只在单元测试中观察它。
    pub(crate) fn latest_state(&self) -> Option<OperationState> {
        self.latest_record().map(|record| record.state)
    }

    fn latest_record(&self) -> Option<PersistedOperationState> {
        read_latest_record(&self.directory)
    }
}

/// 只协调当前数据位置；其他位置的未完成操作保持原样，避免当前授权越权收口。
///
/// 所有 manifest 先完成只读预检，随后才执行状态追加。损坏记录、目录读取失败和
/// 状态发布失败统一返回错误；调用方应把它们视为协调未完成，不得继续目标写入。
pub fn reconcile_unfinished_manifests_for_location_with_recovery_handlers<V, F>(
    storage_root: &Path,
    data_location_id: &str,
    mut verify_catalog: V,
    mut capture_failure: F,
) -> Result<OperationReconciliationSummary, OperationManifestError>
where
    V: FnMut(&OperationManifestJournal) -> bool,
    F: FnMut(&OperationManifestJournal) -> bool,
{
    if data_location_id.is_empty() {
        return Err(OperationManifestError::InvalidManifest);
    }

    // 先读完并校验全部记录，避免后面的损坏目录导致前面记录已被部分收口。
    let journals = read_reconciliation_journals(storage_root)?;
    let mut summary = OperationReconciliationSummary {
        inspected_count: journals.len() as u64,
        reconciled_count: 0,
        not_applied_count: 0,
        completed_count: 0,
        manual_recovery_required_count: 0,
        unrelated_data_location_count: 0,
        status: OperationReconciliationStatus::NoUnfinishedOperations,
    };

    for (journal, state) in journals {
        if state.is_terminal() {
            continue;
        }
        if journal.data_location_id() != data_location_id {
            summary.unrelated_data_location_count += 1;
            continue;
        }

        let terminal = match state {
            OperationState::Planned
            | OperationState::BackingUp
            | OperationState::BackupVerified => OperationState::NotApplied,
            OperationState::CatalogReconciling if verify_catalog(&journal) => {
                OperationState::Completed
            }
            // 写后状态无法证明时，只能保留失败现场并进入人工恢复；绝不重放事务。
            _ => {
                if !capture_failure(&journal) {
                    return Err(OperationManifestError::ReconciliationFailed);
                }
                OperationState::ManualRecoveryRequired
            }
        };

        journal
            .transition(terminal)
            .map_err(|_| OperationManifestError::ReconciliationFailed)?;
        summary.reconciled_count += 1;
        match terminal {
            OperationState::NotApplied => summary.not_applied_count += 1,
            OperationState::Completed => summary.completed_count += 1,
            OperationState::ManualRecoveryRequired => summary.manual_recovery_required_count += 1,
            _ => return Err(OperationManifestError::ReconciliationFailed),
        }
    }

    summary.status = if summary.manual_recovery_required_count > 0 {
        OperationReconciliationStatus::ManualRecoveryRequired
    } else if summary.reconciled_count > 0 {
        OperationReconciliationStatus::Reconciled
    } else if summary.unrelated_data_location_count > 0 {
        OperationReconciliationStatus::OtherDataLocationPending
    } else {
        OperationReconciliationStatus::NoUnfinishedOperations
    };
    Ok(summary)
}

fn read_reconciliation_journals(
    storage_root: &Path,
) -> Result<Vec<(OperationManifestJournal, OperationState)>, OperationManifestError> {
    let operations_dir = storage_root.join("operations");
    if !operations_dir.exists() {
        return Ok(Vec::new());
    }
    let entries =
        fs::read_dir(&operations_dir).map_err(|_| OperationManifestError::DirectoryUnreadable)?;
    let mut journals = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|_| OperationManifestError::DirectoryUnreadable)?;
        let file_type = entry
            .file_type()
            .map_err(|_| OperationManifestError::DirectoryUnreadable)?;
        if !file_type.is_dir() {
            return Err(OperationManifestError::InvalidManifest);
        }
        let record =
            read_latest_record(&entry.path()).ok_or(OperationManifestError::InvalidManifest)?;
        if !record_matches_directory(&entry.path(), &record) {
            return Err(OperationManifestError::InvalidManifest);
        }
        let state = record.state;
        journals.push((
            OperationManifestJournal {
                directory: entry.path(),
                operation_id: record.operation_id,
                data_location_id: record.data_location_id,
                target_file_evidence: record.target_file_evidence,
                verified_target_file_evidence: record.verified_target_file_evidence,
            },
            state,
        ));
    }
    Ok(journals)
}

/// 捕获未知写后状态的 fixture 失败现场；已有现场只做完整性复核，不覆盖。
pub fn capture_fixture_failure_scene(
    storage_root: &Path,
    operation_id: &str,
    target_db_path: &Path,
) -> bool {
    if operation_id.is_empty()
        || !operation_id
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '-')
    {
        return false;
    }
    let failure_raw_dir = storage_root
        .join("backups")
        .join(operation_id)
        .join("failure")
        .join("raw");
    if failure_raw_dir.is_dir() {
        return verify_fixture_failure_scene(&failure_raw_dir);
    }
    if failure_raw_dir.exists()
        || fs::symlink_metadata(target_db_path)
            .map(|metadata| metadata.file_type().is_symlink() || !metadata.is_file())
            .unwrap_or(true)
    {
        return false;
    }
    let Some(parent) = failure_raw_dir.parent() else {
        return false;
    };
    if fs::create_dir_all(parent).is_err() || fs::create_dir(&failure_raw_dir).is_err() {
        return false;
    }

    let files = [
        (target_db_path.to_path_buf(), "database.db", true),
        (
            sidecar_path(target_db_path, "-wal"),
            "database.db-wal",
            false,
        ),
        (
            sidecar_path(target_db_path, "-shm"),
            "database.db-shm",
            false,
        ),
    ];
    let mut hashes = Vec::new();
    for (source, name, required) in files {
        let metadata = match fs::symlink_metadata(&source) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound && !required => continue,
            Err(_) => return false,
        };
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return false;
        }
        let destination = failure_raw_dir.join(name);
        if copy_file_and_hash(&source, &destination)
            .map(|hash| hashes.push(format!("{hash}  {name}")))
            .is_none()
        {
            return false;
        }
    }
    if hashes.is_empty()
        || fs::write(failure_raw_dir.join("hashes.sha256"), hashes.join("\n")).is_err()
    {
        return false;
    }
    verify_fixture_failure_scene(&failure_raw_dir)
}

fn sidecar_path(path: &Path, suffix: &str) -> PathBuf {
    let mut value = path.as_os_str().to_os_string();
    value.push(suffix);
    PathBuf::from(value)
}

fn copy_file_and_hash(source: &Path, destination: &Path) -> Option<String> {
    let mut input = File::open(source).ok()?;
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination)
        .ok()?;
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = input.read(&mut buffer).ok()?;
        if read == 0 {
            break;
        }
        output.write_all(&buffer[..read]).ok()?;
        digest.update(&buffer[..read]);
    }
    output.sync_all().ok()?;
    Some(hex::encode(digest.finalize()))
}

fn verify_fixture_failure_scene(directory: &Path) -> bool {
    let manifest = match fs::read_to_string(directory.join("hashes.sha256")) {
        Ok(manifest) => manifest,
        Err(_) => return false,
    };
    let mut seen = Vec::new();
    for line in manifest.lines().filter(|line| !line.is_empty()) {
        let Some((expected, name)) = line.split_once("  ") else {
            return false;
        };
        if !matches!(name, "database.db" | "database.db-wal" | "database.db-shm")
            || seen.iter().any(|existing| existing == &name)
        {
            return false;
        }
        let path = directory.join(name);
        if !path.is_file() || sha256_file(&path).as_deref() != Some(expected) {
            return false;
        }
        seen.push(name);
    }
    if !seen.contains(&"database.db") {
        return false;
    }
    fs::read_dir(directory)
        .ok()
        .map(|mut entries| {
            entries.all(|entry| {
                let Ok(entry) = entry else {
                    return false;
                };
                matches!(
                    entry.file_name().to_str(),
                    Some("database.db")
                        | Some("database.db-wal")
                        | Some("database.db-shm")
                        | Some("hashes.sha256")
                )
            })
        })
        .unwrap_or(false)
}

fn sha256_file(path: &Path) -> Option<String> {
    let mut input = File::open(path).ok()?;
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = input.read(&mut buffer).ok()?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    Some(hex::encode(digest.finalize()))
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
        if !record_matches_directory(&entry.path(), &record) {
            return false;
        }
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
            OperationState::CatalogReconciling if verify_catalog(&journal) => {
                OperationState::Completed
            }
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
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return false,
        // 恢复区存在但不可读时必须阻止新写入，不能把未知状态当成安全状态。
        Err(_) => return true,
    };
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(_) => return true,
        };
        let is_directory = match entry.file_type() {
            Ok(kind) => kind.is_dir(),
            Err(_) => return true,
        };
        if !is_directory {
            continue;
        }
        let record = match read_latest_record(&entry.path()) {
            Some(record) if record_matches_directory(&entry.path(), &record) => record,
            Some(_) | None => return true,
        };
        if record.state == OperationState::ManualRecoveryRequired {
            return true;
        }
    }
    false
}

/// 读取恢复区中的最新操作状态；损坏记录直接失败关闭，不跳过并伪造完整列表。
pub fn list_operation_summaries(
    storage_root: &Path,
) -> Result<Vec<OperationSummary>, OperationManifestError> {
    if !storage_root.exists() || !storage_root.is_dir() {
        return Err(OperationManifestError::StorageRootUnavailable);
    }
    let operations_dir = storage_root.join("operations");
    if !operations_dir.exists() {
        return Ok(Vec::new());
    }
    let entries =
        fs::read_dir(&operations_dir).map_err(|_| OperationManifestError::DirectoryUnreadable)?;
    let mut summaries = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|_| OperationManifestError::DirectoryUnreadable)?;
        if !entry
            .file_type()
            .map_err(|_| OperationManifestError::DirectoryUnreadable)?
            .is_dir()
        {
            continue;
        }
        let record =
            read_latest_record(&entry.path()).ok_or(OperationManifestError::InvalidManifest)?;
        if !record_matches_directory(&entry.path(), &record) {
            return Err(OperationManifestError::InvalidManifest);
        }
        summaries.push(OperationSummary {
            operation_id: record.operation_id,
            data_location_id: record.data_location_id,
            state: record.state,
            sequence: record.sequence,
            has_verified_target_file_evidence: record.verified_target_file_evidence.is_some(),
        });
    }
    summaries.sort_by(|left, right| {
        right
            .sequence
            .cmp(&left.sequence)
            .then_with(|| left.operation_id.cmp(&right.operation_id))
    });
    Ok(summaries)
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
    let result = (|| {
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
        fs::rename(&temporary, &destination).map_err(|_| ())
    })();
    if result.is_err() {
        // 发布失败时临时状态不是恢复证据，必须立即回收，避免重复失败堆积。
        let _ = fs::remove_file(&temporary);
    }
    result
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
    if records.iter().any(|record| {
        record.operation_id != latest.operation_id
            || record.data_location_id != latest.data_location_id
            || record.target_file_evidence != latest.target_file_evidence
    }) {
        return None;
    }
    Some(latest)
}

/// 只允许状态机定义的前驱转换，避免跳过备份、验证或失败留痕阶段。
fn is_valid_transition(previous: Option<OperationState>, next: OperationState) -> bool {
    match previous {
        None => next == OperationState::Planned,
        Some(OperationState::Planned) => matches!(
            next,
            OperationState::BackingUp | OperationState::NotApplied | OperationState::FailedSafe
        ),
        Some(OperationState::BackingUp) => matches!(
            next,
            OperationState::BackupVerified
                | OperationState::NotApplied
                | OperationState::FailedSafe
        ),
        Some(OperationState::BackupVerified) => matches!(
            next,
            OperationState::TargetWriting
                | OperationState::CancelledBeforeWrite
                | OperationState::NotApplied
                | OperationState::FailedSafe
        ),
        Some(OperationState::TargetWriting) => matches!(
            next,
            OperationState::TargetCommittedUnverified
                | OperationState::NotApplied
                | OperationState::FailurePreserving
                | OperationState::ManualRecoveryRequired
        ),
        Some(OperationState::TargetCommittedUnverified) => matches!(
            next,
            OperationState::TargetVerifying
                | OperationState::FailurePreserving
                | OperationState::ManualRecoveryRequired
        ),
        Some(OperationState::TargetVerifying) => matches!(
            next,
            OperationState::CatalogReconciling
                | OperationState::VerificationInconclusive
                | OperationState::FailurePreserving
                | OperationState::ManualRecoveryRequired
        ),
        Some(OperationState::CatalogReconciling) => matches!(
            next,
            OperationState::Completed
                | OperationState::FailurePreserving
                | OperationState::ManualRecoveryRequired
        ),
        Some(OperationState::VerificationInconclusive) => matches!(
            next,
            OperationState::FailurePreserving | OperationState::ManualRecoveryRequired
        ),
        Some(OperationState::FailurePreserving) => matches!(
            next,
            OperationState::FailureSnapshotVerified | OperationState::ManualRecoveryRequired
        ),
        Some(OperationState::FailureSnapshotVerified) => matches!(
            next,
            OperationState::RestoreStaging | OperationState::ManualRecoveryRequired
        ),
        Some(OperationState::RestoreStaging) => matches!(
            next,
            OperationState::RestoreStaged | OperationState::ManualRecoveryRequired
        ),
        Some(OperationState::RestoreStaged) => matches!(
            next,
            OperationState::RestoreReplacing | OperationState::ManualRecoveryRequired
        ),
        Some(OperationState::RestoreReplacing) => matches!(
            next,
            OperationState::RestoredVerifying | OperationState::ManualRecoveryRequired
        ),
        Some(OperationState::RestoredVerifying) => matches!(
            next,
            OperationState::RestoredVerified | OperationState::ManualRecoveryRequired
        ),
        Some(
            OperationState::Completed
            | OperationState::CancelledBeforeWrite
            | OperationState::FailedSafe
            | OperationState::NotApplied
            | OperationState::RestoredVerified
            | OperationState::ManualRecoveryRequired,
        ) => false,
    }
}

fn record_matches_directory(directory: &Path, record: &PersistedOperationState) -> bool {
    directory
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name == record.operation_id)
}

#[cfg(test)]
mod tests {
    use std::process::Command;

    use tempfile::tempdir;
    use traesync_domain::{OperationId, OperationState, TargetFileEvidence};

    use super::{
        has_manual_recovery_required, list_operation_summaries, read_latest_record,
        reconcile_unfinished_manifests_for_location_with_recovery_handlers,
        reconcile_unfinished_manifests_with_failure_capture,
        reconcile_unfinished_manifests_with_recovery_handlers, OperationManifestJournal,
        OperationReconciliationStatus,
    };

    fn evidence() -> TargetFileEvidence {
        TargetFileEvidence {
            db_fingerprint: "fixture-db".to_string(),
            wal_fingerprint: None,
            shm_fingerprint: None,
        }
    }

    fn transition_to_state(journal: &OperationManifestJournal, target: OperationState) {
        let path = match target {
            OperationState::Planned => Vec::new(),
            OperationState::BackingUp => vec![OperationState::BackingUp],
            OperationState::BackupVerified => {
                vec![OperationState::BackingUp, OperationState::BackupVerified]
            }
            OperationState::TargetWriting => vec![
                OperationState::BackingUp,
                OperationState::BackupVerified,
                OperationState::TargetWriting,
            ],
            OperationState::TargetCommittedUnverified => vec![
                OperationState::BackingUp,
                OperationState::BackupVerified,
                OperationState::TargetWriting,
                OperationState::TargetCommittedUnverified,
            ],
            OperationState::TargetVerifying => vec![
                OperationState::BackingUp,
                OperationState::BackupVerified,
                OperationState::TargetWriting,
                OperationState::TargetCommittedUnverified,
                OperationState::TargetVerifying,
            ],
            OperationState::CatalogReconciling => vec![
                OperationState::BackingUp,
                OperationState::BackupVerified,
                OperationState::TargetWriting,
                OperationState::TargetCommittedUnverified,
                OperationState::TargetVerifying,
                OperationState::CatalogReconciling,
            ],
            OperationState::VerificationInconclusive => vec![
                OperationState::BackingUp,
                OperationState::BackupVerified,
                OperationState::TargetWriting,
                OperationState::TargetCommittedUnverified,
                OperationState::TargetVerifying,
                OperationState::VerificationInconclusive,
            ],
            OperationState::FailurePreserving => vec![
                OperationState::BackingUp,
                OperationState::BackupVerified,
                OperationState::TargetWriting,
                OperationState::TargetCommittedUnverified,
                OperationState::TargetVerifying,
                OperationState::FailurePreserving,
            ],
            OperationState::FailureSnapshotVerified => vec![
                OperationState::BackingUp,
                OperationState::BackupVerified,
                OperationState::TargetWriting,
                OperationState::TargetCommittedUnverified,
                OperationState::TargetVerifying,
                OperationState::FailurePreserving,
                OperationState::FailureSnapshotVerified,
            ],
            OperationState::RestoreStaging => vec![
                OperationState::BackingUp,
                OperationState::BackupVerified,
                OperationState::TargetWriting,
                OperationState::TargetCommittedUnverified,
                OperationState::TargetVerifying,
                OperationState::FailurePreserving,
                OperationState::FailureSnapshotVerified,
                OperationState::RestoreStaging,
            ],
            OperationState::RestoreStaged => vec![
                OperationState::BackingUp,
                OperationState::BackupVerified,
                OperationState::TargetWriting,
                OperationState::TargetCommittedUnverified,
                OperationState::TargetVerifying,
                OperationState::FailurePreserving,
                OperationState::FailureSnapshotVerified,
                OperationState::RestoreStaging,
                OperationState::RestoreStaged,
            ],
            OperationState::RestoreReplacing => vec![
                OperationState::BackingUp,
                OperationState::BackupVerified,
                OperationState::TargetWriting,
                OperationState::TargetCommittedUnverified,
                OperationState::TargetVerifying,
                OperationState::FailurePreserving,
                OperationState::FailureSnapshotVerified,
                OperationState::RestoreStaging,
                OperationState::RestoreStaged,
                OperationState::RestoreReplacing,
            ],
            OperationState::RestoredVerifying => vec![
                OperationState::BackingUp,
                OperationState::BackupVerified,
                OperationState::TargetWriting,
                OperationState::TargetCommittedUnverified,
                OperationState::TargetVerifying,
                OperationState::FailurePreserving,
                OperationState::FailureSnapshotVerified,
                OperationState::RestoreStaging,
                OperationState::RestoreStaged,
                OperationState::RestoreReplacing,
                OperationState::RestoredVerifying,
            ],
            OperationState::Completed
            | OperationState::CancelledBeforeWrite
            | OperationState::FailedSafe
            | OperationState::NotApplied
            | OperationState::RestoredVerified
            | OperationState::ManualRecoveryRequired => unreachable!(),
        };

        for state in path {
            if state == OperationState::CatalogReconciling {
                journal.transition_catalog_reconciling(&evidence()).unwrap();
            } else {
                journal.transition(state).unwrap();
            }
        }
    }

    #[test]
    fn transition_rejects_skipped_and_repeated_states() {
        let storage = tempdir().unwrap();
        let journal = OperationManifestJournal::create(
            storage.path(),
            &OperationId::new(),
            "fixture-location",
            &evidence(),
        )
        .unwrap();

        assert!(journal.transition(OperationState::TargetWriting).is_err());
        assert_eq!(journal.latest_state(), Some(OperationState::Planned));
        journal.transition(OperationState::BackingUp).unwrap();
        assert!(journal.transition(OperationState::Completed).is_err());
        assert_eq!(journal.latest_state(), Some(OperationState::BackingUp));
    }

    #[test]
    fn list_operation_summaries_reads_latest_state_without_exposing_paths() {
        let storage = tempdir().unwrap();
        let journal = OperationManifestJournal::create(
            storage.path(),
            &OperationId::new(),
            "fixture-location",
            &evidence(),
        )
        .unwrap();
        journal.transition(OperationState::BackingUp).unwrap();
        journal.transition(OperationState::BackupVerified).unwrap();

        let summaries = list_operation_summaries(storage.path()).unwrap();
        assert_eq!(summaries.len(), 1);
        assert_eq!(summaries[0].data_location_id, "fixture-location");
        assert_eq!(summaries[0].state, OperationState::BackupVerified);
        assert!(!summaries[0]
            .operation_id
            .contains(storage.path().to_string_lossy().as_ref()));
    }

    #[test]
    fn list_rejects_operation_directory_name_mismatch() {
        let storage = tempdir().unwrap();
        let operation_id = OperationId::new();
        OperationManifestJournal::create(
            storage.path(),
            &operation_id,
            "fixture-location",
            &evidence(),
        )
        .unwrap();
        let original = storage
            .path()
            .join("operations")
            .join(operation_id.as_str());
        let tampered = storage.path().join("operations").join("renamed-operation");
        std::fs::rename(&original, &tampered).unwrap();

        assert_eq!(
            list_operation_summaries(storage.path()),
            Err(super::OperationManifestError::InvalidManifest)
        );
    }

    #[test]
    fn missing_operations_directory_is_not_manual_recovery() {
        let storage = tempdir().unwrap();

        assert!(!has_manual_recovery_required(storage.path()));
    }

    #[test]
    fn malformed_operation_directory_blocks_new_writes() {
        let storage = tempdir().unwrap();
        std::fs::create_dir_all(storage.path().join("operations").join("broken")).unwrap();

        assert!(has_manual_recovery_required(storage.path()));
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
        journal.transition(OperationState::BackingUp).unwrap();
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
    fn scoped_reconciliation_skips_other_data_location() {
        let storage = tempdir().unwrap();
        let local = OperationManifestJournal::create(
            storage.path(),
            &OperationId::new(),
            "fixture-local",
            &evidence(),
        )
        .unwrap();
        local.transition(OperationState::BackingUp).unwrap();
        let foreign = OperationManifestJournal::create(
            storage.path(),
            &OperationId::new(),
            "fixture-foreign",
            &evidence(),
        )
        .unwrap();
        foreign.transition(OperationState::BackingUp).unwrap();

        let summary = reconcile_unfinished_manifests_for_location_with_recovery_handlers(
            storage.path(),
            "fixture-local",
            |_| false,
            |_| panic!("写前状态不得捕获失败现场"),
        )
        .unwrap();

        assert_eq!(summary.inspected_count, 2);
        assert_eq!(summary.reconciled_count, 1);
        assert_eq!(summary.not_applied_count, 1);
        assert_eq!(summary.unrelated_data_location_count, 1);
        assert_eq!(summary.status, OperationReconciliationStatus::Reconciled);
        assert_eq!(local.latest_state(), Some(OperationState::NotApplied));
        assert_eq!(foreign.latest_state(), Some(OperationState::BackingUp));
    }

    #[test]
    fn scoped_reconciliation_preflights_all_manifests_before_writing() {
        let storage = tempdir().unwrap();
        let local = OperationManifestJournal::create(
            storage.path(),
            &OperationId::new(),
            "fixture-local",
            &evidence(),
        )
        .unwrap();
        local.transition(OperationState::BackingUp).unwrap();
        std::fs::create_dir_all(storage.path().join("operations").join("broken")).unwrap();

        assert_eq!(
            reconcile_unfinished_manifests_for_location_with_recovery_handlers(
                storage.path(),
                "fixture-local",
                |_| false,
                |_| panic!("损坏记录不得进入失败现场捕获"),
            ),
            Err(super::OperationManifestError::InvalidManifest)
        );
        assert_eq!(local.latest_state(), Some(OperationState::BackingUp));
    }

    #[test]
    fn scoped_reconciliation_is_idempotent() {
        let storage = tempdir().unwrap();
        let journal = OperationManifestJournal::create(
            storage.path(),
            &OperationId::new(),
            "fixture-local",
            &evidence(),
        )
        .unwrap();
        journal.transition(OperationState::BackingUp).unwrap();

        let first = reconcile_unfinished_manifests_for_location_with_recovery_handlers(
            storage.path(),
            "fixture-local",
            |_| false,
            |_| panic!("写前状态不得捕获失败现场"),
        )
        .unwrap();
        let record_count = std::fs::read_dir(journal.directory.as_path())
            .unwrap()
            .count();
        let second = reconcile_unfinished_manifests_for_location_with_recovery_handlers(
            storage.path(),
            "fixture-local",
            |_| panic!("终态不得再次复验"),
            |_| panic!("终态不得再次捕获"),
        )
        .unwrap();

        assert_eq!(first.reconciled_count, 1);
        assert_eq!(second.reconciled_count, 0);
        assert_eq!(
            second.status,
            OperationReconciliationStatus::NoUnfinishedOperations
        );
        assert_eq!(
            std::fs::read_dir(journal.directory.as_path())
                .unwrap()
                .count(),
            record_count
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
            transition_to_state(&journal, nonterminal);

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
            transition_to_state(&journal, nonterminal);
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
            assert_eq!(
                std::fs::read(before_raw.join("database.db")).unwrap(),
                b"original-db"
            );
            assert_eq!(
                std::fs::read(before_raw.join("database.db-wal")).unwrap(),
                b"original-wal"
            );
            assert_eq!(
                std::fs::read(before_raw.join("database.db-shm")).unwrap(),
                b"original-shm"
            );
            assert_eq!(
                std::fs::read(before_logical.join("database.db")).unwrap(),
                b"logical-backup"
            );
            assert_eq!(
                std::fs::read(failure_raw.join("database.db")).unwrap(),
                b"failure-scene"
            );

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
            transition_to_state(&journal, nonterminal);
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
        journal.transition(OperationState::BackingUp).unwrap();
        journal.transition(OperationState::BackupVerified).unwrap();
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
        transition_to_state(&journal, OperationState::TargetVerifying);
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
            transition_to_state(&journal, OperationState::TargetVerifying);
            journal.transition_catalog_reconciling(&evidence()).unwrap();
        } else {
            transition_to_state(&journal, state);
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
            ("failure_preserving", OperationState::ManualRecoveryRequired),
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
            let operation_id = operation_dir
                .file_name()
                .unwrap()
                .to_string_lossy()
                .to_string();
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
            assert_eq!(
                std::fs::read(before_raw.join("database.db")).unwrap(),
                b"original-db"
            );
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
                |_| panic!("终态重启不得再次复验或收口"),
                |_| panic!("终态重启不得再次捕获现场或重放事务")
            ));
            assert_eq!(
                std::fs::read_dir(&operation_dir).unwrap().count(),
                records_after_first_restart,
                "终态重启不得再次追加状态或重放事务"
            );
        }
    }

    #[test]
    fn failed_manifest_publication_removes_temporary_file() {
        let root = tempdir().unwrap();
        let destination = root.path().join("00000000000000000000.json");
        std::fs::create_dir(&destination).unwrap();
        let record = super::PersistedOperationState {
            sequence: 0,
            operation_id: "op-test".to_string(),
            data_location_id: "data-test".to_string(),
            target_file_evidence: evidence(),
            verified_target_file_evidence: None,
            state: OperationState::Planned,
        };

        assert!(super::publish_record(root.path(), &record).is_err());
        assert!(!root.path().read_dir().unwrap().any(|entry| {
            entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".00000000000000000000.tmp-")
        }));
    }
}
