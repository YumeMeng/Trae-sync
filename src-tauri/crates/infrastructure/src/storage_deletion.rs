//! T12 独立手工删除计划。
//!
//! 删除前固定对象 ID、文件哈希、保护状态和根身份；确认前不产生删除副作用，
//! 漂移或越界时整单失败。删除后保留不可变墓碑，说明原对象已不可恢复。

use std::collections::BTreeSet;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use traesync_domain::FileIdentity;
use traesync_ports::FileIdentityProvider;

use crate::file_identity::PlatformFileIdentityProvider;
use crate::operation_lease::OperationLease;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeletionCandidate {
    pub object_id: String,
    pub relative_path: String,
    pub sha256: String,
    /// 计划输出的服务端保护结论；构建计划时不信任调用方传入值。
    pub protected: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StorageDeletionPlan {
    pub storage_root_id: String,
    pub object_ids: Vec<String>,
    pub candidates: Vec<DeletionCandidate>,
    pub confirmation_token: String,
    /// 服务端捕获的文件身份；不接受前端伪造，避免同路径替换文件绕过计划。
    #[serde(default)]
    pub file_identities: std::collections::BTreeMap<String, FileIdentity>,
    /// 计划生成时发现的活动 manifest 对象 ID；解除保护不能绕过这些引用。
    #[serde(default)]
    pub live_reference_object_ids: BTreeSet<String>,
    /// 计划生成时发现的活动 manifest 相对路径；低层执行 API 也必须保留路径保护。
    #[serde(default)]
    pub live_reference_relative_paths: BTreeSet<String>,
    /// 固定恢复区引用集合的指纹；执行前重新读取，防止计划生成后引用变化。
    #[serde(default)]
    pub live_reference_fingerprint: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeletionTombstone {
    pub object_id: String,
    pub relative_path: String,
    pub sha256: String,
    pub file_identity: FileIdentity,
    pub operation_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeletionError {
    OutsideRoot,
    ProtectedObject,
    ActiveManifestReference,
    LiveReferenceUnavailable,
    PlanExpired,
    ConfirmationRequired,
    ConfirmationTokenMismatch,
    UnprotectRequired,
    DeleteFailed,
    TombstoneFailed,
    OperationAlreadyExists,
}

impl std::fmt::Display for DeletionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let message = match self {
            Self::OutsideRoot => "删除对象不在存储根内",
            Self::ProtectedObject => "删除计划包含受保护对象",
            Self::ActiveManifestReference => "删除对象仍被非终态恢复记录引用",
            Self::LiveReferenceUnavailable => "活动引用恢复记录不可验证",
            Self::PlanExpired => "删除计划证据已漂移",
            Self::ConfirmationRequired => "删除需要二次确认",
            Self::ConfirmationTokenMismatch => "删除确认令牌不匹配",
            Self::UnprotectRequired => "受保护对象必须先显式解除保护",
            Self::DeleteFailed => "删除对象失败",
            Self::TombstoneFailed => "删除墓碑保存失败",
            Self::OperationAlreadyExists => "删除操作编号已存在",
        };
        f.write_str(message)
    }
}

impl std::error::Error for DeletionError {}

pub fn build_deletion_plan(
    storage_root: &Path,
    storage_root_id: &str,
    candidates: Vec<DeletionCandidate>,
) -> Result<StorageDeletionPlan, DeletionError> {
    build_deletion_plan_with_index(
        storage_root,
        storage_root_id,
        candidates,
        &LiveReferenceIndex::empty(),
    )
}

/// 生产删除计划必须从 lease 绑定的固定恢复区读取活动引用。
pub fn build_deletion_plan_with_lease(
    storage_root: &Path,
    storage_root_id: &str,
    candidates: Vec<DeletionCandidate>,
    lease: &OperationLease,
) -> Result<StorageDeletionPlan, DeletionError> {
    let live_references = LiveReferenceIndex::from_recovery_root(lease.recovery_root())?;
    build_deletion_plan_with_index(storage_root, storage_root_id, candidates, &live_references)
}

#[cfg(test)]
fn build_deletion_plan_with_recovery_root(
    storage_root: &Path,
    storage_root_id: &str,
    candidates: Vec<DeletionCandidate>,
    recovery_root: &Path,
) -> Result<StorageDeletionPlan, DeletionError> {
    let lease = OperationLease::acquire(recovery_root, "storage-deletion")
        .map_err(|_| DeletionError::LiveReferenceUnavailable)?;
    build_deletion_plan_with_lease(storage_root, storage_root_id, candidates, &lease)
}

/// 固定恢复区活动引用的服务端索引。
///
/// 该索引只读取最新 manifest，不读取对话正文。解析失败时 fail closed，避免把未知
/// 的进行中操作误当成没有引用；执行计划前还会重新计算指纹。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveReferenceIndex {
    object_ids: BTreeSet<String>,
    relative_paths: BTreeSet<String>,
    fingerprint: String,
}

impl LiveReferenceIndex {
    pub fn empty() -> Self {
        Self::from_sets(BTreeSet::new(), BTreeSet::new())
    }

    pub fn from_recovery_root(recovery_root: &Path) -> Result<Self, DeletionError> {
        reject_symlink_directory_chain(recovery_root)?;
        let mut object_ids = BTreeSet::new();
        let mut relative_paths = BTreeSet::new();
        scan_manifest_family(
            recovery_root,
            "operations",
            &mut object_ids,
            &mut relative_paths,
        )?;
        scan_manifest_family(
            recovery_root,
            "migration-manifests",
            &mut object_ids,
            &mut relative_paths,
        )?;
        Ok(Self::from_sets(object_ids, relative_paths))
    }

    pub fn fingerprint(&self) -> &str {
        &self.fingerprint
    }

    fn from_sets(object_ids: BTreeSet<String>, relative_paths: BTreeSet<String>) -> Self {
        let mut digest = Sha256::new();
        digest.update(b"trae-sync-live-references-v1\0");
        for value in &object_ids {
            digest.update(b"object\0");
            digest.update(value.as_bytes());
            digest.update([0]);
        }
        for value in &relative_paths {
            digest.update(b"path\0");
            digest.update(value.as_bytes());
            digest.update([0]);
        }
        Self {
            object_ids,
            relative_paths,
            fingerprint: hex::encode(digest.finalize()),
        }
    }

    fn protects(&self, candidate: &DeletionCandidate) -> bool {
        self.object_ids.contains(&candidate.object_id)
            || normalize_relative_path(&candidate.relative_path)
                .is_some_and(|path| self.relative_paths.contains(&path))
    }
}

fn build_deletion_plan_with_index(
    storage_root: &Path,
    storage_root_id: &str,
    candidates: Vec<DeletionCandidate>,
    live_references: &LiveReferenceIndex,
) -> Result<StorageDeletionPlan, DeletionError> {
    // 删除边界必须绑定实体存储根，不能通过 junction 或 symlink 把根解析到授权范围外。
    reject_symlink_directory_chain(storage_root).map_err(|_| DeletionError::OutsideRoot)?;
    let mut seen_ids = BTreeSet::new();
    let mut seen_paths = BTreeSet::new();
    let mut normalized_candidates = Vec::with_capacity(candidates.len());
    let file_identity_provider = PlatformFileIdentityProvider::new();
    let mut file_identities = std::collections::BTreeMap::new();
    for candidate in candidates {
        let path = safe_path(storage_root, &candidate.relative_path)?;
        let identity_before = file_identity_provider
            .read_file_identity(&path)
            .ok_or(DeletionError::PlanExpired)?;
        let hash = hash_file(&path)?;
        let identity_after = file_identity_provider
            .read_file_identity(&path)
            .ok_or(DeletionError::PlanExpired)?;
        if !seen_ids.insert(candidate.object_id.clone())
            || !seen_paths.insert(path.clone())
            || !path.is_file()
            || hash != candidate.sha256
            || identity_before != identity_after
        {
            return Err(DeletionError::PlanExpired);
        }
        let live_referenced = live_references.protects(&candidate);
        file_identities.insert(candidate.object_id.clone(), identity_after);
        normalized_candidates.push(DeletionCandidate {
            protected: backend_protected_path(&candidate.relative_path) || live_referenced,
            ..candidate
        });
    }
    let mut digest = Sha256::new();
    digest.update(storage_root_id.as_bytes());
    for candidate in &normalized_candidates {
        digest.update(candidate.object_id.as_bytes());
        digest.update(candidate.relative_path.as_bytes());
        digest.update(candidate.sha256.as_bytes());
        digest.update([u8::from(candidate.protected)]);
        if let Some(identity) = file_identities.get(&candidate.object_id) {
            digest.update(identity.volume_serial.to_le_bytes());
            digest.update(identity.file_index_high.to_le_bytes());
            digest.update(identity.file_index_low.to_le_bytes());
        }
    }
    let live_reference_object_ids = normalized_candidates
        .iter()
        .filter(|candidate| live_references.object_ids.contains(&candidate.object_id))
        .map(|candidate| candidate.object_id.clone())
        .collect();
    let live_reference_relative_paths = normalized_candidates
        .iter()
        .filter_map(|candidate| normalize_relative_path(&candidate.relative_path))
        .filter(|path| live_references.relative_paths.contains(path))
        .collect();

    Ok(StorageDeletionPlan {
        storage_root_id: storage_root_id.to_string(),
        object_ids: normalized_candidates
            .iter()
            .map(|candidate| candidate.object_id.clone())
            .collect(),
        candidates: normalized_candidates,
        confirmation_token: hex::encode(digest.finalize()),
        file_identities,
        live_reference_object_ids,
        live_reference_relative_paths,
        live_reference_fingerprint: live_references.fingerprint().to_string(),
    })
}

/// 推导存储根中不能被普通手工删除的权威或恢复对象。
///
/// 前端传入的 `protected` 只用于兼容旧 DTO，绝不参与该判断；未知对象仍需通过
/// 计划哈希、二次确认和逐对象审计才能删除。
fn backend_protected_path(relative_path: &str) -> bool {
    let normalized = relative_path
        .replace('\\', "/")
        .split('/')
        .filter(|component| !component.is_empty() && *component != ".")
        .collect::<Vec<_>>()
        .join("/")
        .to_ascii_lowercase();
    normalized == "storage-root.json"
        || normalized == "catalog.db"
        || normalized == "current.json"
        || normalized == "catalog"
        || normalized.starts_with("catalog/")
        || normalized.starts_with("operations/")
        || normalized.starts_with("progress/")
        || normalized.starts_with("deletion-journal/")
        || normalized.starts_with("deletion-tombstones/")
        || normalized.starts_with("backups/")
        || normalized.starts_with("snapshots/")
}

pub fn apply_deletion_plan(
    storage_root: &Path,
    current_storage_root_id: &str,
    plan: &StorageDeletionPlan,
    operation_id: &str,
    confirmation_token: &str,
    confirmed: bool,
    unprotected_object_ids: &BTreeSet<String>,
) -> Result<Vec<DeletionTombstone>, DeletionError> {
    let live_references = LiveReferenceIndex::from_sets(
        plan.live_reference_object_ids.clone(),
        plan.live_reference_relative_paths.clone(),
    );
    apply_deletion_plan_with_index(
        storage_root,
        current_storage_root_id,
        plan,
        operation_id,
        confirmation_token,
        confirmed,
        unprotected_object_ids,
        &live_references,
        false,
    )
}

/// 生产删除执行在逐对象副作用前重新读取 lease 绑定的固定恢复区。
pub fn apply_deletion_plan_with_lease(
    storage_root: &Path,
    current_storage_root_id: &str,
    plan: &StorageDeletionPlan,
    operation_id: &str,
    confirmation_token: &str,
    confirmed: bool,
    unprotected_object_ids: &BTreeSet<String>,
    lease: &OperationLease,
) -> Result<Vec<DeletionTombstone>, DeletionError> {
    let live_references = LiveReferenceIndex::from_recovery_root(lease.recovery_root())?;
    apply_deletion_plan_with_index(
        storage_root,
        current_storage_root_id,
        plan,
        operation_id,
        confirmation_token,
        confirmed,
        unprotected_object_ids,
        &live_references,
        true,
    )
}

#[cfg(test)]
fn apply_deletion_plan_with_recovery_root(
    storage_root: &Path,
    current_storage_root_id: &str,
    plan: &StorageDeletionPlan,
    operation_id: &str,
    confirmation_token: &str,
    confirmed: bool,
    unprotected_object_ids: &BTreeSet<String>,
    recovery_root: &Path,
) -> Result<Vec<DeletionTombstone>, DeletionError> {
    let lease = OperationLease::acquire(recovery_root, "storage-deletion")
        .map_err(|_| DeletionError::LiveReferenceUnavailable)?;
    apply_deletion_plan_with_lease(
        storage_root,
        current_storage_root_id,
        plan,
        operation_id,
        confirmation_token,
        confirmed,
        unprotected_object_ids,
        &lease,
    )
}

fn apply_deletion_plan_with_index(
    storage_root: &Path,
    current_storage_root_id: &str,
    plan: &StorageDeletionPlan,
    operation_id: &str,
    confirmation_token: &str,
    confirmed: bool,
    unprotected_object_ids: &BTreeSet<String>,
    live_references: &LiveReferenceIndex,
    check_live_reference_fingerprint: bool,
) -> Result<Vec<DeletionTombstone>, DeletionError> {
    // 执行阶段再次检查根目录，防止计划生成后根路径被替换成链接别名。
    reject_symlink_directory_chain(storage_root).map_err(|_| DeletionError::OutsideRoot)?;
    if !confirmed {
        return Err(DeletionError::ConfirmationRequired);
    }
    if plan.confirmation_token != confirmation_token {
        return Err(DeletionError::ConfirmationTokenMismatch);
    }
    if plan.storage_root_id != current_storage_root_id {
        return Err(DeletionError::PlanExpired);
    }
    if check_live_reference_fingerprint
        && plan.live_reference_fingerprint != live_references.fingerprint()
    {
        return Err(DeletionError::PlanExpired);
    }
    for candidate in &plan.candidates {
        let path = safe_path(storage_root, &candidate.relative_path)?;
        if live_references.protects(candidate) {
            return Err(DeletionError::ActiveManifestReference);
        }
        if candidate.protected && !unprotected_object_ids.contains(&candidate.object_id) {
            return Err(DeletionError::UnprotectRequired);
        }
        verify_candidate(&path, candidate, plan)?;
    }

    let tombstones: Vec<_> = plan
        .candidates
        .iter()
        .map(|candidate| {
            Ok(DeletionTombstone {
                object_id: candidate.object_id.clone(),
                relative_path: candidate.relative_path.clone(),
                sha256: candidate.sha256.clone(),
                file_identity: plan
                    .file_identities
                    .get(&candidate.object_id)
                    .ok_or(DeletionError::PlanExpired)?
                    .clone(),
                operation_id: operation_id.to_string(),
            })
        })
        .collect::<Result<Vec<_>, DeletionError>>()?;
    let mut journal = create_journal(storage_root, operation_id, &tombstones)?;
    let mut deleted_tombstones = Vec::new();
    for candidate in &plan.candidates {
        let path = safe_path(storage_root, &candidate.relative_path)?;
        if verify_candidate(&path, candidate, plan).is_err() {
            let _ = append_journal_event(
                &mut journal,
                &JournalEvent::Failed {
                    object_id: candidate.object_id.clone(),
                    reason: "plan_expired_before_delete",
                },
            );
            return Err(DeletionError::PlanExpired);
        }
        let expected_identity = plan
            .file_identities
            .get(&candidate.object_id)
            .ok_or(DeletionError::PlanExpired)?;
        if let Err(error) = delete_verified_file(&path, expected_identity, &candidate.sha256) {
            let reason = match error {
                DeletionError::PlanExpired => "plan_expired_during_delete",
                _ => "delete_failed",
            };
            let _ = append_journal_event(
                &mut journal,
                &JournalEvent::Failed {
                    object_id: candidate.object_id.clone(),
                    reason,
                },
            );
            return Err(error);
        }
        let tombstone = tombstones
            .iter()
            .find(|value| value.object_id == candidate.object_id)
            .expect("计划候选与墓碑数量必须一致");
        if append_tombstone(storage_root, tombstone).is_err() {
            // 文件已删除但审计写入失败：保留操作日志，明确进入人工恢复，而不是伪装成未发生。
            let _ = append_journal_event(
                &mut journal,
                &JournalEvent::Failed {
                    object_id: candidate.object_id.clone(),
                    reason: "tombstone_failed_after_delete",
                },
            );
            return Err(DeletionError::TombstoneFailed);
        }
        append_journal_event(
            &mut journal,
            &JournalEvent::Deleted {
                object_id: candidate.object_id.clone(),
            },
        )?;
        deleted_tombstones.push(tombstone.clone());
    }
    append_journal_event(&mut journal, &JournalEvent::Completed)?;
    Ok(deleted_tombstones)
}

fn scan_manifest_family(
    recovery_root: &Path,
    family: &str,
    object_ids: &mut BTreeSet<String>,
    relative_paths: &mut BTreeSet<String>,
) -> Result<(), DeletionError> {
    let family_root = recovery_root.join(family);
    match fs::symlink_metadata(&family_root) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(_) => return Err(DeletionError::LiveReferenceUnavailable),
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
            return Err(DeletionError::LiveReferenceUnavailable)
        }
        Ok(_) => {}
    }

    for entry in fs::read_dir(&family_root).map_err(|_| DeletionError::LiveReferenceUnavailable)? {
        let entry = entry.map_err(|_| DeletionError::LiveReferenceUnavailable)?;
        let metadata = entry
            .file_type()
            .map_err(|_| DeletionError::LiveReferenceUnavailable)?;
        if metadata.is_symlink() || !metadata.is_dir() {
            return Err(DeletionError::LiveReferenceUnavailable);
        }
        let latest = latest_manifest(&entry.path())?;
        let Some(document) = latest else {
            return Err(DeletionError::LiveReferenceUnavailable);
        };
        let state = document
            .get("state")
            .and_then(Value::as_str)
            .or_else(|| document.get("stage").and_then(Value::as_str))
            .ok_or(DeletionError::LiveReferenceUnavailable)?;
        if is_terminal_manifest_state(state) {
            continue;
        }
        collect_manifest_references(&document, object_ids, relative_paths);
        if family == "migration-manifests" {
            add_migration_reference_suffixes(&document, object_ids, relative_paths);
        }
    }
    Ok(())
}

/// 固定恢复区及其父目录不得通过符号链接指向未授权位置。
fn reject_symlink_directory_chain(path: &Path) -> Result<(), DeletionError> {
    let mut current = Some(path);
    while let Some(candidate) = current {
        match fs::symlink_metadata(candidate) {
            Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
                return Err(DeletionError::LiveReferenceUnavailable)
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return Err(DeletionError::LiveReferenceUnavailable),
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

fn latest_manifest(directory: &Path) -> Result<Option<Value>, DeletionError> {
    let mut latest: Option<(u64, Value)> = None;
    for entry in fs::read_dir(directory).map_err(|_| DeletionError::LiveReferenceUnavailable)? {
        let entry = entry.map_err(|_| DeletionError::LiveReferenceUnavailable)?;
        let metadata = entry
            .file_type()
            .map_err(|_| DeletionError::LiveReferenceUnavailable)?;
        if metadata.is_symlink() || !metadata.is_file() {
            return Err(DeletionError::LiveReferenceUnavailable);
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        if !name.ends_with(".json") {
            continue;
        }
        let document: Value = serde_json::from_reader(
            File::open(entry.path()).map_err(|_| DeletionError::LiveReferenceUnavailable)?,
        )
        .map_err(|_| DeletionError::LiveReferenceUnavailable)?;
        let sequence = document
            .get("sequence")
            .and_then(Value::as_u64)
            .ok_or(DeletionError::LiveReferenceUnavailable)?;
        if latest
            .as_ref()
            .map_or(true, |(current, _)| sequence > *current)
        {
            latest = Some((sequence, document));
        }
    }
    Ok(latest.map(|(_, document)| document))
}

fn is_terminal_manifest_state(state: &str) -> bool {
    matches!(
        state,
        "completed"
            | "cancelled_before_write"
            | "failed_safe"
            | "not_applied"
            | "restored_verified"
            | "manual_recovery_required"
    )
}

fn collect_manifest_references(
    value: &Value,
    object_ids: &mut BTreeSet<String>,
    relative_paths: &mut BTreeSet<String>,
) {
    match value {
        Value::Object(map) => {
            for (key, child) in map {
                let key = key.to_ascii_lowercase();
                if is_object_reference_key(&key) {
                    collect_string_values(child, object_ids, None);
                } else if is_path_reference_key(&key) {
                    collect_string_values(child, relative_paths, Some(normalize_relative_path));
                }
                collect_manifest_references(child, object_ids, relative_paths);
            }
        }
        Value::Array(values) => {
            for child in values {
                collect_manifest_references(child, object_ids, relative_paths);
            }
        }
        _ => {}
    }
}

fn collect_string_values(
    value: &Value,
    destination: &mut BTreeSet<String>,
    normalize: Option<fn(&str) -> Option<String>>,
) {
    match value {
        Value::String(value) => {
            let normalized =
                normalize.map_or_else(|| Some(value.clone()), |normalize| normalize(value));
            if let Some(value) = normalized.filter(|value| !value.is_empty()) {
                destination.insert(value);
            }
        }
        Value::Array(values) => {
            for value in values {
                collect_string_values(value, destination, normalize);
            }
        }
        _ => {}
    }
}

fn is_object_reference_key(key: &str) -> bool {
    matches!(
        key,
        "object_id"
            | "object_ids"
            | "storage_object_id"
            | "storage_object_ids"
            | "referenced_object_id"
            | "referenced_object_ids"
    )
}

fn is_path_reference_key(key: &str) -> bool {
    matches!(
        key,
        "relative_path"
            | "relative_paths"
            | "storage_relative_path"
            | "storage_relative_paths"
            | "referenced_relative_path"
            | "referenced_relative_paths"
    )
}

fn add_migration_reference_suffixes(
    document: &Value,
    object_ids: &mut BTreeSet<String>,
    relative_paths: &mut BTreeSet<String>,
) {
    for (field, prefixes) in [
        (
            "staging_id",
            &["staging/", "catalog/generations/.staging-"][..],
        ),
        ("destination_id", &["catalog/generations/"][..]),
    ] {
        let Some(value) = document.get(field).and_then(Value::as_str) else {
            continue;
        };
        if value.is_empty() {
            continue;
        }
        object_ids.insert(value.to_string());
        for prefix in prefixes {
            relative_paths.insert(format!("{prefix}{value}").to_ascii_lowercase());
        }
    }
}

fn normalize_relative_path(relative_path: &str) -> Option<String> {
    let mut components = Vec::new();
    for component in relative_path.replace('\\', "/").split('/') {
        if component.is_empty() || component == "." {
            continue;
        }
        if component == ".." {
            return None;
        }
        components.push(component.to_ascii_lowercase());
    }
    (!components.is_empty()).then(|| components.join("/"))
}

fn safe_path(root: &Path, relative: &str) -> Result<PathBuf, DeletionError> {
    let root = root
        .canonicalize()
        .map_err(|_| DeletionError::OutsideRoot)?;
    let relative_path = Path::new(relative);
    if relative_path.as_os_str().is_empty()
        || relative_path.is_absolute()
        || relative_path.components().any(|component| {
            matches!(
                component,
                Component::Prefix(_) | Component::RootDir | Component::ParentDir
            )
        })
    {
        return Err(DeletionError::OutsideRoot);
    }

    // 逐级检查 symlink，避免 canonicalize 后把根外对象误当成根内文件。
    let mut path = root.clone();
    for component in relative_path.components() {
        if let Component::Normal(name) = component {
            path.push(name);
            let metadata = fs::symlink_metadata(&path).map_err(|_| DeletionError::OutsideRoot)?;
            if metadata.file_type().is_symlink() {
                return Err(DeletionError::OutsideRoot);
            }
        }
    }
    let canonical = path
        .canonicalize()
        .map_err(|_| DeletionError::OutsideRoot)?;
    if !canonical.starts_with(&root) || canonical == root {
        return Err(DeletionError::OutsideRoot);
    }
    Ok(canonical)
}

fn verify_candidate(
    path: &Path,
    candidate: &DeletionCandidate,
    plan: &StorageDeletionPlan,
) -> Result<(), DeletionError> {
    if !path.is_file() {
        return Err(DeletionError::PlanExpired);
    }
    let provider = PlatformFileIdentityProvider::new();
    let identity_before = provider
        .read_file_identity(path)
        .ok_or(DeletionError::PlanExpired)?;
    let expected_identity = plan
        .file_identities
        .get(&candidate.object_id)
        .ok_or(DeletionError::PlanExpired)?;
    if &identity_before != expected_identity {
        return Err(DeletionError::PlanExpired);
    }
    if hash_file(path)? != candidate.sha256 {
        return Err(DeletionError::PlanExpired);
    }
    let identity_after = provider
        .read_file_identity(path)
        .ok_or(DeletionError::PlanExpired)?;
    if &identity_after != expected_identity {
        return Err(DeletionError::PlanExpired);
    }
    Ok(())
}

#[derive(Debug, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
enum JournalEvent {
    Prepared {
        candidates: Vec<DeletionTombstone>,
    },
    Deleted {
        object_id: String,
    },
    Failed {
        object_id: String,
        reason: &'static str,
    },
    Completed,
}

fn create_journal(
    root: &Path,
    operation_id: &str,
    tombstones: &[DeletionTombstone],
) -> Result<File, DeletionError> {
    if operation_id.is_empty()
        || operation_id
            .chars()
            .any(|character| matches!(character, '\\' | '/' | ':'))
        || operation_id == "."
        || operation_id == ".."
    {
        return Err(DeletionError::OutsideRoot);
    }
    let dir = audit_directory(root, "deletion-journal")?;
    let path = dir.join(format!("{operation_id}.jsonl"));
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(path)
        .map_err(|error| {
            if error.kind() == std::io::ErrorKind::AlreadyExists {
                DeletionError::OperationAlreadyExists
            } else {
                DeletionError::TombstoneFailed
            }
        })?;
    append_journal_event(
        &mut file,
        &JournalEvent::Prepared {
            candidates: tombstones.to_vec(),
        },
    )?;
    Ok(file)
}

fn append_journal_event(file: &mut File, event: &JournalEvent) -> Result<(), DeletionError> {
    serde_json::to_writer(&mut *file, event).map_err(|_| DeletionError::TombstoneFailed)?;
    file.write_all(b"\n")
        .map_err(|_| DeletionError::TombstoneFailed)?;
    file.sync_all().map_err(|_| DeletionError::TombstoneFailed)
}

fn audit_directory(root: &Path, name: &str) -> Result<PathBuf, DeletionError> {
    let dir = root.join(name);
    match fs::symlink_metadata(&dir) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
            Err(DeletionError::TombstoneFailed)
        }
        Ok(_) => Ok(dir),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir(&dir).map_err(|_| DeletionError::TombstoneFailed)?;
            Ok(dir)
        }
        Err(_) => Err(DeletionError::TombstoneFailed),
    }
}

fn append_tombstone(root: &Path, tombstone: &DeletionTombstone) -> Result<(), DeletionError> {
    let dir = audit_directory(root, "deletion-tombstones")?;
    let path = dir.join(format!("{}.jsonl", tombstone.operation_id));
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|_| DeletionError::TombstoneFailed)?;
    serde_json::to_writer(&mut file, tombstone).map_err(|_| DeletionError::TombstoneFailed)?;
    file.write_all(b"\n")
        .map_err(|_| DeletionError::TombstoneFailed)?;
    file.sync_all().map_err(|_| DeletionError::TombstoneFailed)
}

fn hash_file(path: &Path) -> Result<String, DeletionError> {
    let mut file = File::open(path).map_err(|_| DeletionError::PlanExpired)?;
    hash_reader(&mut file)
}

fn hash_reader(reader: &mut File) -> Result<String, DeletionError> {
    reader
        .seek(SeekFrom::Start(0))
        .map_err(|_| DeletionError::PlanExpired)?;
    let mut digest = Sha256::new();
    // 删除前校验可能读取大文件，使用堆缓冲区保持线程栈占用稳定。
    let mut buffer = vec![0_u8; 1024 * 1024];
    loop {
        let count = reader
            .read(&mut buffer)
            .map_err(|_| DeletionError::PlanExpired)?;
        if count == 0 {
            break;
        }
        digest.update(&buffer[..count]);
    }
    Ok(hex::encode(digest.finalize()))
}

/// 在 Windows 上用已验证文件句柄标记删除，避免校验后按路径删除另一文件。
#[cfg(windows)]
fn delete_verified_file(
    path: &Path,
    expected_identity: &FileIdentity,
    expected_sha256: &str,
) -> Result<(), DeletionError> {
    use std::os::windows::ffi::OsStrExt;
    use std::os::windows::io::{AsRawHandle, FromRawHandle};
    use windows_sys::Win32::Foundation::{GENERIC_READ, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, FileDispositionInfo, SetFileInformationByHandle, DELETE,
        FILE_ATTRIBUTE_NORMAL, FILE_DISPOSITION_INFO, FILE_FLAG_OPEN_REPARSE_POINT,
        FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
    };

    let wide: Vec<u16> = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let handle = unsafe {
        CreateFileW(
            wide.as_ptr(),
            GENERIC_READ | DELETE,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            std::ptr::null(),
            OPEN_EXISTING,
            FILE_ATTRIBUTE_NORMAL | FILE_FLAG_OPEN_REPARSE_POINT,
            std::ptr::null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return Err(DeletionError::PlanExpired);
    }
    // 句柄从这里接管，所有返回路径都会自动关闭它。
    let mut file = unsafe { File::from_raw_handle(handle) };
    let actual_identity =
        read_file_identity_from_handle(file.as_raw_handle()).ok_or(DeletionError::PlanExpired)?;
    if &actual_identity != expected_identity || hash_reader(&mut file)? != expected_sha256 {
        return Err(DeletionError::PlanExpired);
    }
    let disposition = FILE_DISPOSITION_INFO { DeleteFile: 1 };
    let ok = unsafe {
        SetFileInformationByHandle(
            file.as_raw_handle(),
            FileDispositionInfo,
            (&disposition as *const FILE_DISPOSITION_INFO).cast(),
            std::mem::size_of::<FILE_DISPOSITION_INFO>() as u32,
        )
    };
    if ok == 0 {
        return Err(DeletionError::DeleteFailed);
    }
    Ok(())
}

#[cfg(windows)]
fn read_file_identity_from_handle(handle: std::os::windows::io::RawHandle) -> Option<FileIdentity> {
    use windows_sys::Win32::Storage::FileSystem::GetFileInformationByHandle;

    #[repr(C)]
    #[derive(Default)]
    struct ByHandleFileInformation {
        _file_attributes: u32,
        _creation_time_low: u32,
        _creation_time_high: u32,
        _last_access_time_low: u32,
        _last_access_time_high: u32,
        _last_write_time_low: u32,
        _last_write_time_high: u32,
        volume_serial: u32,
        _file_size_high: u32,
        _file_size_low: u32,
        _number_of_links: u32,
        file_index_high: u32,
        file_index_low: u32,
    }

    let mut info = ByHandleFileInformation::default();
    let ok = unsafe {
        GetFileInformationByHandle(handle, (&mut info as *mut ByHandleFileInformation).cast())
    };
    if ok == 0 {
        return None;
    }
    Some(FileIdentity {
        volume_serial: info.volume_serial as u64,
        file_index_high: info.file_index_high as u64,
        file_index_low: info.file_index_low as u64,
    })
}

#[cfg(not(windows))]
fn delete_verified_file(
    path: &Path,
    expected_identity: &FileIdentity,
    expected_sha256: &str,
) -> Result<(), DeletionError> {
    // 非 Windows 仅用于跨平台 fixture；路径已规范化，且删除前再次验证身份和哈希。
    let provider = PlatformFileIdentityProvider::new();
    let identity_before = provider
        .read_file_identity(path)
        .ok_or(DeletionError::PlanExpired)?;
    let mut file = File::open(path).map_err(|_| DeletionError::PlanExpired)?;
    let hash = hash_reader(&mut file)?;
    let identity_after = provider
        .read_file_identity(path)
        .ok_or(DeletionError::PlanExpired)?;
    if &identity_before != expected_identity
        || &identity_after != expected_identity
        || hash != expected_sha256
    {
        return Err(DeletionError::PlanExpired);
    }
    fs::remove_file(path).map_err(|_| DeletionError::DeleteFailed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidate(path: &Path, id: &str, protected: bool) -> DeletionCandidate {
        DeletionCandidate {
            object_id: id.to_string(),
            relative_path: path.file_name().unwrap().to_string_lossy().into_owned(),
            sha256: hash_file(path).unwrap(),
            protected,
        }
    }

    #[test]
    fn confirmation_and_unprotect_are_required_before_delete() {
        let root = tempfile::tempdir().unwrap();
        let snapshots = root.path().join("snapshots");
        fs::create_dir_all(&snapshots).unwrap();
        let file = snapshots.join("snapshot.db");
        fs::write(&file, b"snapshot").unwrap();
        let plan = build_deletion_plan(
            root.path(),
            "root-a",
            vec![DeletionCandidate {
                object_id: "snap-a".to_string(),
                relative_path: "snapshots/snapshot.db".to_string(),
                sha256: hash_file(&file).unwrap(),
                protected: false,
            }],
        )
        .unwrap();
        assert!(plan.candidates[0].protected);
        let empty = BTreeSet::new();
        assert!(matches!(
            apply_deletion_plan(
                root.path(),
                "root-a",
                &plan,
                "op-a",
                &plan.confirmation_token,
                false,
                &empty,
            ),
            Err(DeletionError::ConfirmationRequired)
        ));
        assert!(file.exists());
        assert!(matches!(
            apply_deletion_plan(
                root.path(),
                "root-a",
                &plan,
                "op-a",
                &plan.confirmation_token,
                true,
                &empty,
            ),
            Err(DeletionError::UnprotectRequired)
        ));
        assert!(file.exists());
    }

    #[test]
    fn client_protected_flag_cannot_unprotect_root_metadata() {
        let root = tempfile::tempdir().unwrap();
        let binding = root.path().join("storage-root.json");
        fs::write(&binding, b"binding").unwrap();
        let plan = build_deletion_plan(
            root.path(),
            "root-a",
            vec![DeletionCandidate {
                object_id: "binding".to_string(),
                relative_path: "storage-root.json".to_string(),
                sha256: hash_file(&binding).unwrap(),
                protected: false,
            }],
        )
        .unwrap();

        assert!(plan.candidates[0].protected);
    }

    #[test]
    fn drift_causes_zero_deletion() {
        let root = tempfile::tempdir().unwrap();
        let first = root.path().join("first.db");
        let second = root.path().join("second.db");
        fs::write(&first, b"first").unwrap();
        fs::write(&second, b"second").unwrap();
        let plan = build_deletion_plan(
            root.path(),
            "root-a",
            vec![
                candidate(&first, "one", false),
                candidate(&second, "two", false),
            ],
        )
        .unwrap();
        fs::write(&first, b"changed").unwrap();
        let result = apply_deletion_plan(
            root.path(),
            "root-a",
            &plan,
            "op-a",
            &plan.confirmation_token,
            true,
            &BTreeSet::new(),
        );
        assert!(matches!(result, Err(DeletionError::PlanExpired)));
        assert!(first.exists());
        assert!(second.exists());
    }

    #[test]
    fn plan_rejects_client_supplied_hash_that_does_not_match_file() {
        let root = tempfile::tempdir().unwrap();
        let file = root.path().join("snapshot.db");
        fs::write(&file, b"snapshot").unwrap();
        let result = build_deletion_plan(
            root.path(),
            "root-a",
            vec![DeletionCandidate {
                object_id: "snap-a".to_string(),
                relative_path: "snapshot.db".to_string(),
                sha256: "forged".to_string(),
                protected: false,
            }],
        );
        assert!(matches!(result, Err(DeletionError::PlanExpired)));
        assert!(file.exists());
    }

    #[test]
    fn mismatched_confirmation_token_causes_zero_deletion() {
        let root = tempfile::tempdir().unwrap();
        let file = root.path().join("snapshot.db");
        fs::write(&file, b"snapshot").unwrap();
        let plan = build_deletion_plan(
            root.path(),
            "root-a",
            vec![candidate(&file, "snap-a", false)],
        )
        .unwrap();
        let result = apply_deletion_plan(
            root.path(),
            "root-a",
            &plan,
            "op-a",
            "wrong-token",
            true,
            &BTreeSet::new(),
        );
        assert!(matches!(
            result,
            Err(DeletionError::ConfirmationTokenMismatch)
        ));
        assert!(file.exists());
    }

    #[test]
    fn duplicate_normalized_paths_expire_plan() {
        let root = tempfile::tempdir().unwrap();
        let file = root.path().join("snapshot.db");
        fs::write(&file, b"snapshot").unwrap();
        let hash = hash_file(&file).unwrap();
        let result = build_deletion_plan(
            root.path(),
            "root-a",
            vec![
                candidate(&file, "snap-a", false),
                DeletionCandidate {
                    object_id: "snap-b".to_string(),
                    relative_path: Path::new(".")
                        .join("snapshot.db")
                        .to_string_lossy()
                        .into_owned(),
                    sha256: hash,
                    protected: false,
                },
            ],
        );
        assert!(matches!(result, Err(DeletionError::PlanExpired)));
        assert!(file.exists());
    }

    #[test]
    fn symlink_outside_root_is_rejected() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let target = outside.path().join("outside.db");
        fs::write(&target, b"outside").unwrap();
        let link = root.path().join("link.db");

        #[cfg(windows)]
        let link_result = std::os::windows::fs::symlink_file(&target, &link);
        #[cfg(unix)]
        let link_result = std::os::unix::fs::symlink(&target, &link);
        #[cfg(not(any(windows, unix)))]
        return;
        if link_result.is_err() {
            return;
        }

        let result = build_deletion_plan(
            root.path(),
            "root-a",
            vec![DeletionCandidate {
                object_id: "link-a".to_string(),
                relative_path: "link.db".to_string(),
                sha256: hash_file(&target).unwrap(),
                protected: false,
            }],
        );
        assert!(matches!(result, Err(DeletionError::OutsideRoot)));
        assert!(target.exists());
    }

    #[test]
    fn symlinked_storage_root_is_rejected_before_plan_creation() {
        let outside = tempfile::tempdir().unwrap();
        let parent = tempfile::tempdir().unwrap();
        let link = parent.path().join("storage-link");
        let file = outside.path().join("snapshot.db");
        fs::write(&file, b"outside").unwrap();

        #[cfg(windows)]
        let link_created = {
            let script_name = "create-storage-junction.cmd";
            let script_path = parent.path().join(script_name);
            let script = format!(
                "@echo off\r\nmklink /J \"{}\" \"{}\"\r\n",
                link.display(),
                outside.path().display()
            );
            fs::write(&script_path, script).is_ok()
                && std::process::Command::new("cmd.exe")
                    .current_dir(parent.path())
                    .args(["/d", "/s", "/c", script_name])
                    .status()
                    .map(|status| status.success())
                    .unwrap_or(false)
        };
        #[cfg(unix)]
        let link_created = std::os::unix::fs::symlink(outside.path(), &link).is_ok();
        #[cfg(not(any(windows, unix)))]
        let link_created = false;

        assert!(
            link_created,
            "BLOCKED: cannot create symlink/junction for storage-root boundary test"
        );
        let result = build_deletion_plan(
            &link,
            "root-a",
            vec![DeletionCandidate {
                object_id: "outside-object".to_string(),
                relative_path: "snapshot.db".to_string(),
                sha256: hash_file(&file).unwrap(),
                protected: false,
            }],
        );
        assert!(matches!(result, Err(DeletionError::OutsideRoot)));
        assert!(file.exists());
    }

    #[test]
    fn tombstone_failure_after_delete_is_recorded_as_manual_recovery() {
        let root = tempfile::tempdir().unwrap();
        let file = root.path().join("snapshot.db");
        fs::write(&file, b"snapshot").unwrap();
        // 用同名文件阻止墓碑目录创建，模拟删除后审计存储不可用。
        fs::write(root.path().join("deletion-tombstones"), b"blocked").unwrap();
        let plan = build_deletion_plan(
            root.path(),
            "root-a",
            vec![candidate(&file, "snap-a", false)],
        )
        .unwrap();

        let result = apply_deletion_plan(
            root.path(),
            "root-a",
            &plan,
            "op-a",
            &plan.confirmation_token,
            true,
            &BTreeSet::new(),
        );
        assert!(matches!(result, Err(DeletionError::TombstoneFailed)));
        assert!(!file.exists());
        let journal = fs::read_to_string(root.path().join("deletion-journal/op-a.jsonl")).unwrap();
        assert!(journal.contains("tombstone_failed_after_delete"));
    }

    #[test]
    fn successful_delete_writes_tombstone_and_completed_journal() {
        let root = tempfile::tempdir().unwrap();
        let file = root.path().join("snapshot.db");
        fs::write(&file, b"snapshot").unwrap();
        let plan = build_deletion_plan(
            root.path(),
            "root-a",
            vec![candidate(&file, "snap-a", false)],
        )
        .unwrap();

        let tombstones = apply_deletion_plan(
            root.path(),
            "root-a",
            &plan,
            "op-a",
            &plan.confirmation_token,
            true,
            &BTreeSet::new(),
        )
        .unwrap();
        assert_eq!(tombstones.len(), 1);
        assert!(!file.exists());
        assert!(root.path().join("deletion-tombstones/op-a.jsonl").is_file());
        let journal = fs::read_to_string(root.path().join("deletion-journal/op-a.jsonl")).unwrap();
        assert!(journal.contains("completed"));
    }

    #[test]
    fn same_hash_replacement_expires_plan_by_file_identity() {
        let root = tempfile::tempdir().unwrap();
        let file = root.path().join("snapshot.db");
        let replacement = root.path().join("replacement.db");
        fs::write(&file, b"snapshot").unwrap();
        let plan = build_deletion_plan(
            root.path(),
            "root-a",
            vec![candidate(&file, "snap-a", false)],
        )
        .unwrap();
        fs::write(&replacement, b"snapshot").unwrap();
        fs::remove_file(&file).unwrap();
        fs::rename(&replacement, &file).unwrap();

        let result = apply_deletion_plan(
            root.path(),
            "root-a",
            &plan,
            "op-1-1",
            &plan.confirmation_token,
            true,
            &BTreeSet::new(),
        );
        assert!(matches!(result, Err(DeletionError::PlanExpired)));
        assert!(file.exists());
    }

    #[test]
    fn interrupted_delete_cannot_resume_same_operation_id() {
        let root = tempfile::tempdir().unwrap();
        let first = root.path().join("first.db");
        let second = root.path().join("second.db");
        fs::write(&first, b"first").unwrap();
        fs::write(&second, b"second").unwrap();
        fs::write(root.path().join("deletion-tombstones"), b"blocked").unwrap();
        let plan = build_deletion_plan(
            root.path(),
            "root-a",
            vec![
                candidate(&first, "one", false),
                candidate(&second, "two", false),
            ],
        )
        .unwrap();

        let first_result = apply_deletion_plan(
            root.path(),
            "root-a",
            &plan,
            "op-2-2",
            &plan.confirmation_token,
            true,
            &BTreeSet::new(),
        );
        assert!(matches!(first_result, Err(DeletionError::TombstoneFailed)));
        assert!(!first.exists());
        assert!(second.exists());

        fs::remove_file(root.path().join("deletion-tombstones")).unwrap();
        let retry_plan = build_deletion_plan(
            root.path(),
            "root-a",
            vec![candidate(&second, "two", false)],
        )
        .unwrap();
        let retry = apply_deletion_plan(
            root.path(),
            "root-a",
            &retry_plan,
            "op-2-2",
            &retry_plan.confirmation_token,
            true,
            &BTreeSet::new(),
        );
        assert!(matches!(retry, Err(DeletionError::OperationAlreadyExists)));
        assert!(second.exists(), "失败操作不能未经重新规划继续删除剩余对象");
    }

    fn write_active_manifest(recovery_root: &Path, family: &str, id: &str, document: Value) {
        let directory = recovery_root.join(family).join(id);
        fs::create_dir_all(&directory).unwrap();
        fs::write(
            directory.join("00000000000000000000.json"),
            serde_json::to_vec(&document).unwrap(),
        )
        .unwrap();
    }

    #[test]
    fn active_operation_and_migration_references_cannot_be_unprotected() {
        let root = tempfile::tempdir().unwrap();
        let recovery = tempfile::tempdir().unwrap();
        let operation_file = root.path().join("operation-cache.db");
        let migration_file = root.path().join("migration-cache.db");
        fs::write(&operation_file, b"operation").unwrap();
        fs::write(&migration_file, b"migration").unwrap();

        write_active_manifest(
            recovery.path(),
            "operations",
            "op-live",
            serde_json::json!({
                "sequence": 0,
                "state": "target_writing",
                "references": [{
                    "object_id": "operation-object",
                    "relative_path": "operation-cache.db"
                }]
            }),
        );
        write_active_manifest(
            recovery.path(),
            "migration-manifests",
            "migration-live",
            serde_json::json!({
                "sequence": 0,
                "stage": "staging",
                "referenced_object_ids": ["migration-object"],
                "referenced_relative_paths": ["migration-cache.db"]
            }),
        );

        let plan = build_deletion_plan_with_recovery_root(
            root.path(),
            "root-a",
            vec![
                candidate(&operation_file, "operation-object", false),
                candidate(&migration_file, "migration-object", false),
            ],
            recovery.path(),
        )
        .unwrap();
        assert!(plan.candidates.iter().all(|candidate| candidate.protected));

        let unprotected = ["operation-object", "migration-object"]
            .into_iter()
            .map(str::to_string)
            .collect();
        let result = apply_deletion_plan_with_recovery_root(
            root.path(),
            "root-a",
            &plan,
            "op-live-delete",
            &plan.confirmation_token,
            true,
            &unprotected,
            recovery.path(),
        );
        assert!(matches!(
            result,
            Err(DeletionError::ActiveManifestReference)
        ));
        assert!(operation_file.exists());
        assert!(migration_file.exists());
    }

    #[test]
    fn low_level_delete_api_preserves_live_path_reference_protection() {
        let root = tempfile::tempdir().unwrap();
        let recovery = tempfile::tempdir().unwrap();
        let file = root.path().join("path-only.db");
        fs::write(&file, b"path-only").unwrap();
        write_active_manifest(
            recovery.path(),
            "operations",
            "op-path-only",
            serde_json::json!({
                "sequence": 0,
                "state": "target_writing",
                "referenced_object_ids": ["different-object"],
                "referenced_relative_paths": ["path-only.db"]
            }),
        );

        let plan = build_deletion_plan_with_recovery_root(
            root.path(),
            "root-a",
            vec![candidate(&file, "candidate-object", false)],
            recovery.path(),
        )
        .unwrap();
        assert!(plan.candidates[0].protected);
        let unprotected = BTreeSet::from(["candidate-object".to_string()]);

        let result = apply_deletion_plan(
            root.path(),
            "root-a",
            &plan,
            "op-path-only-delete",
            &plan.confirmation_token,
            true,
            &unprotected,
        );
        assert!(matches!(
            result,
            Err(DeletionError::ActiveManifestReference)
        ));
        assert!(file.exists());
    }

    #[test]
    fn recovery_root_symlink_chain_fails_closed() {
        let outside = tempfile::tempdir().unwrap();
        let parent = tempfile::tempdir().unwrap();
        let link = parent.path().join("recovery-link");

        #[cfg(windows)]
        let link_created = {
            // 先写入临时脚本，再从临时目录执行，避免 Rust 与 cmd.exe 共同解析路径引号。
            let script_name = "create-recovery-junction.cmd";
            let script_path = parent.path().join(script_name);
            let script = format!(
                "@echo off\r\nmklink /J \"{}\" \"{}\"\r\n",
                link.display(),
                outside.path().display()
            );
            fs::write(&script_path, script).is_ok()
                && std::process::Command::new("cmd.exe")
                    .current_dir(parent.path())
                    .args(["/d", "/s", "/c", script_name])
                    .status()
                    .map(|status| status.success())
                    .unwrap_or(false)
        };
        #[cfg(unix)]
        let link_created = std::os::unix::fs::symlink(outside.path(), &link).is_ok();
        #[cfg(not(any(windows, unix)))]
        let link_created = false;

        assert!(
            link_created,
            "BLOCKED: cannot create symlink/junction for recovery-root boundary test"
        );
        assert!(matches!(
            LiveReferenceIndex::from_recovery_root(&link),
            Err(DeletionError::LiveReferenceUnavailable)
        ));
    }

    #[test]
    fn active_reference_drift_expires_deletion_plan_before_delete() {
        let root = tempfile::tempdir().unwrap();
        let recovery = tempfile::tempdir().unwrap();
        let file = root.path().join("referenced.db");
        fs::write(&file, b"referenced").unwrap();
        write_active_manifest(
            recovery.path(),
            "operations",
            "op-drift",
            serde_json::json!({
                "sequence": 0,
                "state": "target_writing",
                "referenced_object_ids": ["referenced-object"]
            }),
        );

        let plan = build_deletion_plan_with_recovery_root(
            root.path(),
            "root-a",
            vec![candidate(&file, "referenced-object", false)],
            recovery.path(),
        )
        .unwrap();
        fs::write(
            recovery
                .path()
                .join("operations/op-drift/00000000000000000000.json"),
            br#"{"sequence":0,"state":"completed","referenced_object_ids":["referenced-object"]}"#,
        )
        .unwrap();

        let result = apply_deletion_plan_with_recovery_root(
            root.path(),
            "root-a",
            &plan,
            "op-drift-delete",
            &plan.confirmation_token,
            true,
            &BTreeSet::from(["referenced-object".to_string()]),
            recovery.path(),
        );
        assert!(matches!(result, Err(DeletionError::PlanExpired)));
        assert!(file.exists());
    }

    #[test]
    fn malformed_live_manifest_fails_closed_before_plan_creation() {
        let root = tempfile::tempdir().unwrap();
        let recovery = tempfile::tempdir().unwrap();
        let file = root.path().join("candidate.db");
        fs::write(&file, b"candidate").unwrap();
        let directory = recovery.path().join("operations").join("broken");
        fs::create_dir_all(&directory).unwrap();
        fs::write(directory.join("00000000000000000000.json"), b"not-json").unwrap();

        let result = build_deletion_plan_with_recovery_root(
            root.path(),
            "root-a",
            vec![candidate(&file, "candidate", false)],
            recovery.path(),
        );
        assert!(matches!(
            result,
            Err(DeletionError::LiveReferenceUnavailable)
        ));
        assert!(file.exists());
    }
}
