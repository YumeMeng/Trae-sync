//! 本机 TRAE 登录材料凭证库。
//!
//! 只捕获 `storage.json` 白名单认证键，密文使用当前 Windows 用户 DPAPI
//! 保护。模块不接触活动数据库、缓存、MachineGuid 或遥测状态。

use std::collections::BTreeMap;
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::atomic_publish::publish_replacing;
use crate::key_wrapper::{protect_secret, unprotect_secret};

const CREDENTIAL_FORMAT_VERSION: u32 = 1;
const VAULT_MAGIC: &[u8; 8] = b"TRVCRED1";
const BACKUP_MAGIC: &[u8; 8] = b"TRCBAK01";
const MAX_STORAGE_JSON_BYTES: u64 = 8 * 1024 * 1024;
const MAX_VAULT_BYTES: u64 = 8 * 1024 * 1024;
const MAX_PROFILE_ID_BYTES: usize = 256;
const MAX_BINDING_FIELD_BYTES: usize = 512;

const AUTH_PREFIX: &str = "iCubeAuthInfo://";
const AUTH_DEFAULT: &str = "iCubeAuthInfo://default";
const AUTH_CLOUDIDE: &str = "iCubeAuthInfo://icube.cloudide";
const AUTH_ENTITLEMENT: &str = "iCubeAuthInfo://entitlement";
const AUTH_SERVER: &str = "iCubeAuthInfo://server";
const AUTH_USERTAG: &str = "iCubeAuthInfo://usertag";
const AUTH_DC_PREFIX: &str = "iCubeAuthInfo://icube-dc:";

/// 绑定凭证条目的非敏感账号身份。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CredentialBinding {
    pub profile_id: String,
    pub data_location_id: String,
    pub user_fingerprint: String,
}

impl CredentialBinding {
    pub fn new(
        profile_id: impl Into<String>,
        data_location_id: impl Into<String>,
        user_fingerprint: impl Into<String>,
    ) -> Self {
        Self {
            profile_id: profile_id.into(),
            data_location_id: data_location_id.into(),
            user_fingerprint: user_fingerprint.into(),
        }
    }
}

/// 前端可见凭证状态；不包含认证值、密文或 DPAPI 材料。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CredentialState {
    Missing,
    Saved,
    Stale,
    Invalid,
}

/// 凭证状态 DTO。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CredentialStatus {
    pub state: CredentialState,
    pub profile_id: String,
    pub data_location_id: String,
    pub user_fingerprint: String,
    pub format_version: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CredentialVaultError {
    Unavailable,
    Missing,
    Invalid,
    BindingMismatch,
    TraeNotClosed,
    StorageInvalid,
    StorageTooLarge,
    UnsupportedAuthKey,
    RecoveryRequired,
}

/// 凭证替换完成后返回的非敏感操作句柄。
/// 句柄只用于把恢复 journal 与账号切换计划绑定，不包含路径或认证材料。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CredentialApplyOutcome {
    pub operation_id: String,
    /// 与账号切换承接意图绑定的非敏感 ID；旧调用未提供时为 `None`。
    pub intent_id: Option<String>,
}

impl fmt::Display for CredentialVaultError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::Unavailable => "凭证库不可用",
            Self::Missing => "凭证不存在",
            Self::Invalid => "凭证无效",
            Self::BindingMismatch => "凭证绑定不匹配",
            Self::TraeNotClosed => "TRAE 未确认关闭",
            Self::StorageInvalid => "TRAE 登录存储无效",
            Self::StorageTooLarge => "TRAE 登录存储过大",
            Self::UnsupportedAuthKey => "TRAE 登录键不受支持",
            Self::RecoveryRequired => "凭证切换需要人工恢复",
        };
        formatter.write_str(message)
    }
}

impl std::error::Error for CredentialVaultError {}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CredentialEnvelope {
    format_version: u32,
    profile_id: String,
    data_location_id: String,
    user_fingerprint: String,
    entries: BTreeMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RecoveryManifest {
    format_version: u32,
    operation_id: String,
    operation: String,
    sequence: u64,
    state: String,
    profile_id: String,
    data_location_id: String,
    user_fingerprint: String,
    backup_file: String,
    before_sha256: String,
    target_sha256: String,
    created_at_unix_seconds: u64,
    /// 可选以兼容 0.2.1 早期 journal；新承接流程应提供并校验此字段。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    intent_id: Option<String>,
}

/// 启动恢复扫描返回的非敏感凭证 journal 摘要。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CredentialRecoveryRecord {
    pub operation_id: String,
    pub intent_id: Option<String>,
    pub state: String,
    pub sequence: u64,
    pub profile_id: String,
    pub data_location_id: String,
    pub user_fingerprint: String,
}

/// 当前用户本机凭证库。
#[derive(Debug, Clone)]
pub struct CredentialVault {
    root: PathBuf,
}

impl CredentialVault {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// 在已验证的 TRAE 数据根中定位登录存储文件。
    ///
    /// 生产版本优先使用根目录下的 `storage.json`；fixture 兼容
    /// `globalStorage/storage.json`。只返回已存在且通过重解析点检查的普通文件。
    pub fn locate_storage_json(root: &Path) -> Result<PathBuf, CredentialVaultError> {
        reject_path_chain(root, false)?;
        let candidates = [
            root.join("storage.json"),
            root.join("globalStorage").join("storage.json"),
        ];
        let mut found = None;
        for candidate in candidates {
            if let Ok(metadata) = fs::symlink_metadata(&candidate) {
                if is_reparse_or_symlink(&metadata) || !metadata.is_file() {
                    return Err(CredentialVaultError::StorageInvalid);
                }
                if found.is_some() {
                    return Err(CredentialVaultError::StorageInvalid);
                }
                found = Some(candidate);
            }
        }
        found.ok_or(CredentialVaultError::StorageInvalid)
    }

    /// 查询状态。绑定不匹配时返回 `stale`，不会把密文内容暴露给调用方。
    pub fn status(&self, binding: &CredentialBinding) -> CredentialStatus {
        let base = CredentialStatus {
            state: CredentialState::Missing,
            profile_id: binding.profile_id.clone(),
            data_location_id: binding.data_location_id.clone(),
            user_fingerprint: binding.user_fingerprint.clone(),
            format_version: None,
        };
        let path = match self.credential_path(&binding.profile_id) {
            Ok(path) => path,
            Err(_) => {
                return CredentialStatus {
                    state: CredentialState::Invalid,
                    ..base
                }
            }
        };
        if !self.root.exists() {
            return base;
        }
        if fs::symlink_metadata(&self.root)
            .map(|metadata| is_reparse_or_symlink(&metadata) || !metadata.is_dir())
            .unwrap_or(true)
        {
            return CredentialStatus {
                state: CredentialState::Invalid,
                ..base
            };
        }
        match read_vault_payload(&path).and_then(|payload| decode_envelope(&payload)) {
            Ok(envelope) if envelope.format_version == CREDENTIAL_FORMAT_VERSION => {
                let state = if envelope_matches(&envelope, binding) {
                    CredentialState::Saved
                } else {
                    CredentialState::Stale
                };
                CredentialStatus {
                    state,
                    format_version: Some(envelope.format_version),
                    ..base
                }
            }
            Ok(_) => CredentialStatus {
                state: CredentialState::Invalid,
                ..base
            },
            Err(CredentialVaultError::Missing) => base,
            Err(_) => CredentialStatus {
                state: CredentialState::Invalid,
                ..base
            },
        }
    }

    /// 捕获当前 `storage.json` 的认证白名单并写入 DPAPI 密文。
    pub fn capture(
        &self,
        binding: &CredentialBinding,
        storage_path: &Path,
    ) -> Result<CredentialStatus, CredentialVaultError> {
        let storage = read_storage_json(storage_path)?;
        let entries = extract_auth_entries(&storage.value)?;
        if entries.is_empty() {
            return Err(CredentialVaultError::StorageInvalid);
        }

        let envelope = CredentialEnvelope {
            format_version: CREDENTIAL_FORMAT_VERSION,
            profile_id: binding.profile_id.clone(),
            data_location_id: binding.data_location_id.clone(),
            user_fingerprint: binding.user_fingerprint.clone(),
            entries,
        };
        let plaintext = serde_json::to_vec(&envelope).map_err(|_| CredentialVaultError::Invalid)?;
        let encrypted = protect_secret(&plaintext).map_err(map_key_wrapper_error)?;
        let payload = encode_vault_payload(&encrypted)?;
        let destination = self.credential_path(&binding.profile_id)?;
        write_atomically(&destination, &payload, true)?;
        Ok(self.status(binding))
    }

    /// 确认 TRAE 已关闭后，将目标认证白名单原子写回 `storage.json`。
    ///
    /// 先保存原文件与无敏感 manifest，再发布目标文件。任何发布后校验失败
    /// 都尝试原子恢复原字节；恢复失败时保留现场并返回人工恢复错误。
    pub fn apply(
        &self,
        binding: &CredentialBinding,
        storage_path: &Path,
        recovery_root: &Path,
        trae_closed: bool,
    ) -> Result<CredentialApplyOutcome, CredentialVaultError> {
        self.apply_with_intent(binding, storage_path, recovery_root, trae_closed, None)
    }

    /// 带承接意图绑定的凭证替换入口。
    ///
    /// `intent_id` 只进入恢复 manifest，不包含账号认证材料。旧 `apply` API
    /// 保持可用，供尚未接入承接意图的调用方过渡。
    pub fn apply_with_intent(
        &self,
        binding: &CredentialBinding,
        storage_path: &Path,
        recovery_root: &Path,
        trae_closed: bool,
        intent_id: Option<&str>,
    ) -> Result<CredentialApplyOutcome, CredentialVaultError> {
        if !trae_closed {
            return Err(CredentialVaultError::TraeNotClosed);
        }

        validate_binding(binding)?;
        let intent_id = validate_intent_id(intent_id)?;

        if self.has_pending_recovery(recovery_root)? {
            return Err(CredentialVaultError::RecoveryRequired);
        }

        let original = read_storage_json(storage_path)?;
        let payload = self.load_envelope(binding)?;
        let mut updated = original.value.clone();
        replace_auth_entries(&mut updated, &payload.entries)?;
        let updated_bytes = serde_json::to_vec_pretty(&updated)
            .map_err(|_| CredentialVaultError::StorageInvalid)?;

        let operation_id = format!("credential-{}-{}", std::process::id(), now_unix_nanos());
        let operation_dir = recovery_root
            .join("credential-switches")
            .join(&operation_id);
        prepare_recovery_directory(&operation_dir)?;
        let backup_path = operation_dir.join("storage.json.before");
        let manifest_path = operation_dir.join("manifest.json");
        let before_sha256 = sha256_bytes(&original.bytes);
        let target_sha256 = sha256_bytes(&updated_bytes);
        write_protected_backup(&backup_path, &original.bytes)?;
        let manifest = RecoveryManifest {
            format_version: CREDENTIAL_FORMAT_VERSION,
            operation_id: operation_id.clone(),
            operation: "credential_apply".to_string(),
            sequence: 1,
            state: "prepared".to_string(),
            profile_id: binding.profile_id.clone(),
            data_location_id: binding.data_location_id.clone(),
            user_fingerprint: binding.user_fingerprint.clone(),
            backup_file: format!("credential-switches/{operation_id}/storage.json.before"),
            before_sha256,
            target_sha256: target_sha256.clone(),
            created_at_unix_seconds: now_unix_seconds(),
            intent_id: intent_id.clone(),
        };
        write_manifest(&manifest_path, &manifest)?;

        if let Err(error) = write_atomically(storage_path, &updated_bytes, true) {
            return Err(error);
        }

        let target_written = RecoveryManifest {
            sequence: 2,
            state: "target_written".to_string(),
            ..manifest.clone()
        };
        if let Err(error) = write_manifest(&manifest_path, &target_written) {
            return self.restore_after_failure(
                storage_path,
                &original.bytes,
                &manifest_path,
                &target_written,
                &target_sha256,
                error,
            );
        }

        let verified = read_storage_json(storage_path).and_then(|stored| {
            let current_entries = extract_auth_entries(&stored.value)?;
            if current_entries != payload.entries {
                return Err(CredentialVaultError::StorageInvalid);
            }
            if non_auth_projection(&stored.value) != non_auth_projection(&original.value) {
                return Err(CredentialVaultError::StorageInvalid);
            }
            Ok(())
        });
        if let Err(error) = verified {
            return self.restore_after_failure(
                storage_path,
                &original.bytes,
                &manifest_path,
                &manifest,
                &target_sha256,
                error,
            );
        }

        let applied = RecoveryManifest {
            sequence: 3,
            state: "storage_verified".to_string(),
            ..target_written
        };
        if let Err(error) = write_manifest(&manifest_path, &applied) {
            return self.restore_after_failure(
                storage_path,
                &original.bytes,
                &manifest_path,
                &applied,
                &target_sha256,
                error,
            );
        }
        Ok(CredentialApplyOutcome {
            operation_id,
            intent_id,
        })
    }

    fn restore_after_failure(
        &self,
        storage_path: &Path,
        original: &[u8],
        manifest_path: &Path,
        manifest: &RecoveryManifest,
        target_sha256: &str,
        cause: CredentialVaultError,
    ) -> Result<CredentialApplyOutcome, CredentialVaultError> {
        let recovery_manifest = RecoveryManifest {
            sequence: manifest.sequence.saturating_add(1),
            state: "manual_recovery_required".to_string(),
            ..manifest.clone()
        };
        let manifest_result = write_manifest(manifest_path, &recovery_manifest);
        let restore_result = match read_storage_json(storage_path) {
            Ok(current) if sha256_bytes(&current.bytes) == target_sha256 => {
                write_atomically(storage_path, original, true)
            }
            Ok(current) if sha256_bytes(&current.bytes) == sha256_bytes(original) => Ok(()),
            Ok(_) => Err(CredentialVaultError::RecoveryRequired),
            Err(_) => Err(CredentialVaultError::RecoveryRequired),
        };
        if manifest_result.is_err() || restore_result.is_err() {
            return Err(CredentialVaultError::RecoveryRequired);
        }
        Err(cause)
    }

    /// 判断恢复区是否留有未完成的凭证替换。启动阶段只报告人工恢复，
    /// 不自动重放或覆盖用户当前登录状态。
    pub fn has_pending_recovery(&self, recovery_root: &Path) -> Result<bool, CredentialVaultError> {
        Ok(!self.pending_recoveries(recovery_root)?.is_empty())
    }

    /// 枚举启动协调需要处理的凭证 journal；只返回非敏感绑定摘要。
    pub fn pending_recoveries(
        &self,
        recovery_root: &Path,
    ) -> Result<Vec<CredentialRecoveryRecord>, CredentialVaultError> {
        let mut records = Vec::new();
        collect_pending_manifest(&recovery_root.join("manifest.json"), &mut records)?;

        let journal_root = recovery_root.join("credential-switches");
        let metadata = match fs::symlink_metadata(&journal_root) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(records),
            Err(_) => return Err(CredentialVaultError::RecoveryRequired),
        };
        if is_reparse_or_symlink(&metadata) || !metadata.is_dir() {
            return Err(CredentialVaultError::RecoveryRequired);
        }
        let entries =
            fs::read_dir(&journal_root).map_err(|_| CredentialVaultError::RecoveryRequired)?;
        for entry in entries {
            let entry = entry.map_err(|_| CredentialVaultError::RecoveryRequired)?;
            let metadata = fs::symlink_metadata(entry.path())
                .map_err(|_| CredentialVaultError::RecoveryRequired)?;
            if is_reparse_or_symlink(&metadata) || !metadata.is_dir() {
                return Err(CredentialVaultError::RecoveryRequired);
            }
            collect_pending_manifest(&entry.path().join("manifest.json"), &mut records)?;
        }
        records.sort_by(|left, right| left.operation_id.cmp(&right.operation_id));
        Ok(records)
    }

    /// 将已完成的凭证 journal 标记为目标账号已复核；不触碰 storage.json。
    pub fn mark_target_verified(
        &self,
        recovery_root: &Path,
        operation_id: &str,
    ) -> Result<(), CredentialVaultError> {
        let path = manifest_path(recovery_root, operation_id)?;
        update_manifest_state(&path, operation_id, "target_verified", None, None)
    }

    /// 带账号绑定与承接意图校验的目标复核收口入口。
    pub fn mark_target_verified_with_binding(
        &self,
        recovery_root: &Path,
        operation_id: &str,
        binding: &CredentialBinding,
        intent_id: Option<&str>,
    ) -> Result<(), CredentialVaultError> {
        validate_binding(binding)?;
        let intent_id = validate_intent_id(intent_id)?;
        let path = manifest_path(recovery_root, operation_id)?;
        update_manifest_state(
            &path,
            operation_id,
            "target_verified",
            Some(binding),
            Some(intent_id.as_deref()),
        )
    }

    /// 账号证据不匹配或进程竞态时保留现场，禁止自动恢复。
    pub fn mark_manual_recovery_required(
        &self,
        recovery_root: &Path,
        operation_id: &str,
    ) -> Result<(), CredentialVaultError> {
        let path = manifest_path(recovery_root, operation_id)?;
        update_manifest_state(&path, operation_id, "manual_recovery_required", None, None)
    }

    /// 带账号绑定与承接意图校验的人工恢复收口入口。
    pub fn mark_manual_recovery_required_with_binding(
        &self,
        recovery_root: &Path,
        operation_id: &str,
        binding: &CredentialBinding,
        intent_id: Option<&str>,
    ) -> Result<(), CredentialVaultError> {
        validate_binding(binding)?;
        let intent_id = validate_intent_id(intent_id)?;
        let path = manifest_path(recovery_root, operation_id)?;
        update_manifest_state(
            &path,
            operation_id,
            "manual_recovery_required",
            Some(binding),
            Some(intent_id.as_deref()),
        )
    }

    fn load_envelope(
        &self,
        binding: &CredentialBinding,
    ) -> Result<CredentialEnvelope, CredentialVaultError> {
        let path = self.credential_path(&binding.profile_id)?;
        let payload = read_vault_payload(&path)?;
        let envelope = decode_envelope(&payload)?;
        if envelope.format_version != CREDENTIAL_FORMAT_VERSION {
            return Err(CredentialVaultError::Invalid);
        }
        if !envelope_matches(&envelope, binding) {
            return Err(CredentialVaultError::BindingMismatch);
        }
        Ok(envelope)
    }

    fn credential_path(&self, profile_id: &str) -> Result<PathBuf, CredentialVaultError> {
        if profile_id.is_empty() || profile_id.len() > MAX_PROFILE_ID_BYTES {
            return Err(CredentialVaultError::Invalid);
        }
        let mut hasher = Sha256::new();
        hasher.update(profile_id.as_bytes());
        let name = format!("{}.dpapi", hex::encode(hasher.finalize()));
        Ok(self.root.join(name))
    }
}

#[derive(Debug, Clone)]
struct StorageJson {
    bytes: Vec<u8>,
    value: serde_json::Value,
}

fn read_storage_json(path: &Path) -> Result<StorageJson, CredentialVaultError> {
    let parent = path.parent().ok_or(CredentialVaultError::StorageInvalid)?;
    reject_path_chain(parent, false)?;
    let metadata = fs::symlink_metadata(path).map_err(|_| CredentialVaultError::StorageInvalid)?;
    if is_reparse_or_symlink(&metadata) || !metadata.is_file() {
        return Err(CredentialVaultError::StorageInvalid);
    }
    if metadata.len() > MAX_STORAGE_JSON_BYTES {
        return Err(CredentialVaultError::StorageTooLarge);
    }
    let bytes = read_file_exact(path, metadata.len(), MAX_STORAGE_JSON_BYTES)
        .map_err(|_| CredentialVaultError::StorageInvalid)?;
    let value: serde_json::Value =
        serde_json::from_slice(&bytes).map_err(|_| CredentialVaultError::StorageInvalid)?;
    if !value.is_object() {
        return Err(CredentialVaultError::StorageInvalid);
    }
    Ok(StorageJson { bytes, value })
}

fn extract_auth_entries(
    value: &serde_json::Value,
) -> Result<BTreeMap<String, serde_json::Value>, CredentialVaultError> {
    let object = value
        .as_object()
        .ok_or(CredentialVaultError::StorageInvalid)?;
    let mut entries = BTreeMap::new();
    for (key, value) in object {
        if !key.starts_with(AUTH_PREFIX) {
            continue;
        }
        if !is_allowed_auth_key(key) {
            return Err(CredentialVaultError::UnsupportedAuthKey);
        }
        if value.is_null() {
            return Err(CredentialVaultError::StorageInvalid);
        }
        entries.insert(key.clone(), value.clone());
    }
    Ok(entries)
}

fn replace_auth_entries(
    value: &mut serde_json::Value,
    entries: &BTreeMap<String, serde_json::Value>,
) -> Result<(), CredentialVaultError> {
    let object = value
        .as_object_mut()
        .ok_or(CredentialVaultError::StorageInvalid)?;
    let current_auth_keys = object
        .keys()
        .filter(|key| key.starts_with(AUTH_PREFIX))
        .cloned()
        .collect::<Vec<_>>();
    for key in current_auth_keys {
        if !is_allowed_auth_key(&key) {
            return Err(CredentialVaultError::UnsupportedAuthKey);
        }
        object.remove(&key);
    }
    for (key, entry) in entries {
        object.insert(key.clone(), entry.clone());
    }
    Ok(())
}

fn is_allowed_auth_key(key: &str) -> bool {
    key == AUTH_DEFAULT
        || key == AUTH_CLOUDIDE
        || key == AUTH_ENTITLEMENT
        || key == AUTH_SERVER
        || key == AUTH_USERTAG
        || (key.starts_with(AUTH_DC_PREFIX) && key.len() > AUTH_DC_PREFIX.len())
}

fn non_auth_projection(value: &serde_json::Value) -> serde_json::Value {
    let Some(object) = value.as_object() else {
        return serde_json::Value::Null;
    };
    serde_json::Value::Object(
        object
            .iter()
            .filter(|(key, _)| !key.starts_with(AUTH_PREFIX))
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect(),
    )
}

fn encode_vault_payload(encrypted: &[u8]) -> Result<Vec<u8>, CredentialVaultError> {
    if encrypted.is_empty() || encrypted.len() as u64 > MAX_VAULT_BYTES {
        return Err(CredentialVaultError::Invalid);
    }
    let mut bytes = Vec::with_capacity(VAULT_MAGIC.len() + 8 + encrypted.len());
    bytes.extend_from_slice(VAULT_MAGIC);
    bytes.extend_from_slice(&CREDENTIAL_FORMAT_VERSION.to_le_bytes());
    bytes.extend_from_slice(&(encrypted.len() as u32).to_le_bytes());
    bytes.extend_from_slice(encrypted);
    Ok(bytes)
}

fn decode_envelope(payload: &[u8]) -> Result<CredentialEnvelope, CredentialVaultError> {
    if payload.len() < VAULT_MAGIC.len() + 8 || &payload[..VAULT_MAGIC.len()] != VAULT_MAGIC {
        return Err(CredentialVaultError::Invalid);
    }
    let version_offset = VAULT_MAGIC.len();
    let version = u32::from_le_bytes(
        payload[version_offset..version_offset + 4]
            .try_into()
            .map_err(|_| CredentialVaultError::Invalid)?,
    );
    if version != CREDENTIAL_FORMAT_VERSION {
        return Err(CredentialVaultError::Invalid);
    }
    let length_offset = version_offset + 4;
    let length = u32::from_le_bytes(
        payload[length_offset..length_offset + 4]
            .try_into()
            .map_err(|_| CredentialVaultError::Invalid)?,
    ) as usize;
    let start = length_offset + 4;
    if length == 0 || length as u64 > MAX_VAULT_BYTES || payload.len() != start + length {
        return Err(CredentialVaultError::Invalid);
    }
    let plaintext = unprotect_secret(&payload[start..]).map_err(map_key_wrapper_error)?;
    if plaintext.len() as u64 > MAX_VAULT_BYTES {
        return Err(CredentialVaultError::Invalid);
    }
    serde_json::from_slice(&plaintext).map_err(|_| CredentialVaultError::Invalid)
}

fn read_vault_payload(path: &Path) -> Result<Vec<u8>, CredentialVaultError> {
    let parent = path.parent().ok_or(CredentialVaultError::Invalid)?;
    reject_path_chain(parent, false)?;
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(CredentialVaultError::Missing)
        }
        Err(_) => return Err(CredentialVaultError::Invalid),
    };
    if is_reparse_or_symlink(&metadata) || !metadata.is_file() {
        return Err(CredentialVaultError::Invalid);
    }
    if metadata.len() > MAX_VAULT_BYTES {
        return Err(CredentialVaultError::Invalid);
    }
    read_file_exact(path, metadata.len(), MAX_VAULT_BYTES)
        .map_err(|_| CredentialVaultError::Invalid)
}

fn envelope_matches(envelope: &CredentialEnvelope, binding: &CredentialBinding) -> bool {
    envelope.profile_id == binding.profile_id
        && envelope.data_location_id == binding.data_location_id
        && envelope.user_fingerprint == binding.user_fingerprint
}

fn prepare_recovery_directory(path: &Path) -> Result<(), CredentialVaultError> {
    reject_path_chain(path, true)?;
    fs::create_dir_all(path).map_err(|_| CredentialVaultError::RecoveryRequired)?;
    reject_path_chain(path, false)
}

fn validate_binding(binding: &CredentialBinding) -> Result<(), CredentialVaultError> {
    validate_binding_field(&binding.profile_id)?;
    validate_binding_field(&binding.data_location_id)?;
    validate_binding_field(&binding.user_fingerprint)
}

fn validate_binding_field(value: &str) -> Result<(), CredentialVaultError> {
    if value.is_empty() || value.len() > MAX_BINDING_FIELD_BYTES {
        return Err(CredentialVaultError::BindingMismatch);
    }
    Ok(())
}

fn validate_intent_id(intent_id: Option<&str>) -> Result<Option<String>, CredentialVaultError> {
    let Some(intent_id) = intent_id else {
        return Ok(None);
    };
    if intent_id.is_empty() || intent_id.len() > MAX_BINDING_FIELD_BYTES {
        return Err(CredentialVaultError::BindingMismatch);
    }
    Ok(Some(intent_id.to_string()))
}

fn validate_operation_id(operation_id: &str) -> Result<(), CredentialVaultError> {
    validate_binding_field(operation_id)?;
    if operation_id == "."
        || operation_id == ".."
        || operation_id.contains('/')
        || operation_id.contains('\\')
    {
        return Err(CredentialVaultError::BindingMismatch);
    }
    Ok(())
}

fn manifest_path(
    recovery_root: &Path,
    operation_id: &str,
) -> Result<PathBuf, CredentialVaultError> {
    validate_operation_id(operation_id)?;
    Ok(recovery_root
        .join("credential-switches")
        .join(operation_id)
        .join("manifest.json"))
}

fn write_manifest(path: &Path, manifest: &RecoveryManifest) -> Result<(), CredentialVaultError> {
    validate_manifest_shape(manifest)?;
    let bytes =
        serde_json::to_vec_pretty(manifest).map_err(|_| CredentialVaultError::RecoveryRequired)?;
    write_atomically(path, &bytes, true)
}

/// 保护凭证切换前的 storage.json 快照。文件名保留原有诊断约定，
/// 内容只允许是 DPAPI 密文，避免恢复区出现可直接读取的认证正文。
fn write_protected_backup(path: &Path, bytes: &[u8]) -> Result<(), CredentialVaultError> {
    let encrypted = protect_secret(bytes).map_err(map_key_wrapper_error)?;
    if encrypted.is_empty() || encrypted.len() as u64 > MAX_VAULT_BYTES {
        return Err(CredentialVaultError::Invalid);
    }
    let mut payload = Vec::with_capacity(BACKUP_MAGIC.len() + 8 + encrypted.len());
    payload.extend_from_slice(BACKUP_MAGIC);
    payload.extend_from_slice(&CREDENTIAL_FORMAT_VERSION.to_le_bytes());
    payload.extend_from_slice(&(encrypted.len() as u32).to_le_bytes());
    payload.extend_from_slice(&encrypted);
    write_atomically(path, &payload, false)
}

fn write_atomically(
    destination: &Path,
    bytes: &[u8],
    replace_existing: bool,
) -> Result<(), CredentialVaultError> {
    let parent = destination.parent().ok_or(CredentialVaultError::Invalid)?;
    reject_path_chain(parent, true)?;
    fs::create_dir_all(parent).map_err(|_| CredentialVaultError::Unavailable)?;
    reject_path_chain(parent, false)?;
    if let Ok(metadata) = fs::symlink_metadata(destination) {
        if is_reparse_or_symlink(&metadata) || (!replace_existing && metadata.is_file()) {
            return Err(CredentialVaultError::RecoveryRequired);
        }
        if metadata.is_dir() {
            return Err(CredentialVaultError::RecoveryRequired);
        }
    }
    let temporary = parent.join(format!(
        ".{}.tmp-{}-{}",
        destination
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("credential"),
        now_unix_nanos(),
        std::process::id()
    ));
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .map_err(|_| CredentialVaultError::Unavailable)?;
        file.write_all(bytes)
            .map_err(|_| CredentialVaultError::RecoveryRequired)?;
        file.sync_all()
            .map_err(|_| CredentialVaultError::RecoveryRequired)?;
        drop(file);
        publish_replacing(&temporary, destination)
            .map_err(|_| CredentialVaultError::RecoveryRequired)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

fn read_file_exact(path: &Path, expected_len: u64, max_len: u64) -> std::io::Result<Vec<u8>> {
    let file = File::open(path)?;
    let mut bytes = Vec::with_capacity(expected_len as usize);
    file.take(max_len.saturating_add(1))
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 != expected_len || bytes.len() as u64 > max_len {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "文件长度变化",
        ));
    }
    Ok(bytes)
}

fn sha256_bytes(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn read_manifest_if_present(path: &Path) -> Result<Option<RecoveryManifest>, CredentialVaultError> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(CredentialVaultError::RecoveryRequired),
    };
    if is_reparse_or_symlink(&metadata) || !metadata.is_file() {
        return Err(CredentialVaultError::RecoveryRequired);
    }
    let bytes = read_file_exact(path, metadata.len(), 64 * 1024)
        .map_err(|_| CredentialVaultError::RecoveryRequired)?;
    let manifest: RecoveryManifest =
        serde_json::from_slice(&bytes).map_err(|_| CredentialVaultError::RecoveryRequired)?;
    validate_manifest_shape(&manifest)?;
    Ok(Some(manifest))
}

fn validate_manifest_shape(manifest: &RecoveryManifest) -> Result<(), CredentialVaultError> {
    if manifest.format_version != CREDENTIAL_FORMAT_VERSION
        || manifest.operation != "credential_apply"
        || manifest.sequence == 0
    {
        return Err(CredentialVaultError::RecoveryRequired);
    }
    validate_operation_id(&manifest.operation_id)
        .map_err(|_| CredentialVaultError::RecoveryRequired)?;
    validate_binding_field(&manifest.profile_id)
        .map_err(|_| CredentialVaultError::RecoveryRequired)?;
    validate_binding_field(&manifest.data_location_id)
        .map_err(|_| CredentialVaultError::RecoveryRequired)?;
    validate_binding_field(&manifest.user_fingerprint)
        .map_err(|_| CredentialVaultError::RecoveryRequired)?;
    if manifest.backup_file.is_empty()
        || manifest.before_sha256.is_empty()
        || manifest.target_sha256.is_empty()
    {
        return Err(CredentialVaultError::RecoveryRequired);
    }
    if let Some(intent_id) = manifest.intent_id.as_deref() {
        validate_intent_id(Some(intent_id)).map_err(|_| CredentialVaultError::RecoveryRequired)?;
    }
    if !matches!(
        manifest.state.as_str(),
        "prepared"
            | "target_written"
            | "storage_verified"
            | "verified"
            | "target_verified"
            | "manual_recovery_required"
    ) {
        return Err(CredentialVaultError::RecoveryRequired);
    }
    Ok(())
}

fn collect_pending_manifest(
    path: &Path,
    records: &mut Vec<CredentialRecoveryRecord>,
) -> Result<(), CredentialVaultError> {
    let Some(manifest) = read_manifest_if_present(path)? else {
        return Ok(());
    };
    if manifest_is_pending_state(&manifest.state) {
        records.push(CredentialRecoveryRecord {
            operation_id: manifest.operation_id,
            intent_id: manifest.intent_id,
            state: manifest.state,
            sequence: manifest.sequence,
            profile_id: manifest.profile_id,
            data_location_id: manifest.data_location_id,
            user_fingerprint: manifest.user_fingerprint,
        });
    }
    Ok(())
}

fn manifest_is_pending_state(state: &str) -> bool {
    matches!(
        state,
        "prepared"
            | "target_written"
            | "storage_verified"
            | "verified"
            | "manual_recovery_required"
    )
}

fn update_manifest_state(
    path: &Path,
    operation_id: &str,
    state: &str,
    expected_binding: Option<&CredentialBinding>,
    expected_intent_id: Option<Option<&str>>,
) -> Result<(), CredentialVaultError> {
    let current = read_manifest_if_present(path)?.ok_or(CredentialVaultError::RecoveryRequired)?;
    if current.operation_id != operation_id || current.operation != "credential_apply" {
        return Err(CredentialVaultError::BindingMismatch);
    }
    if let Some(binding) = expected_binding {
        if current.profile_id != binding.profile_id
            || current.data_location_id != binding.data_location_id
            || current.user_fingerprint != binding.user_fingerprint
        {
            return Err(CredentialVaultError::BindingMismatch);
        }
    }
    if let Some(expected_intent_id) = expected_intent_id {
        if current.intent_id.as_deref() != expected_intent_id {
            return Err(CredentialVaultError::BindingMismatch);
        }
    }
    validate_manifest_transition(&current.state, state)?;
    let next = RecoveryManifest {
        sequence: current.sequence.saturating_add(1),
        state: state.to_string(),
        ..current
    };
    write_manifest(path, &next)
}

fn validate_manifest_transition(current: &str, next: &str) -> Result<(), CredentialVaultError> {
    if current == next {
        return Ok(());
    }
    let allowed = matches!(
        (current, next),
        ("prepared", "target_written" | "manual_recovery_required")
            | (
                "target_written",
                "storage_verified" | "verified" | "manual_recovery_required"
            )
            | (
                "storage_verified",
                "target_verified" | "manual_recovery_required"
            )
            | ("verified", "target_verified" | "manual_recovery_required")
    );
    if allowed {
        Ok(())
    } else {
        Err(CredentialVaultError::RecoveryRequired)
    }
}

fn reject_path_chain(path: &Path, allow_missing: bool) -> Result<(), CredentialVaultError> {
    let mut current = Some(path);
    while let Some(candidate) = current {
        match fs::symlink_metadata(candidate) {
            Ok(metadata) if is_reparse_or_symlink(&metadata) || !metadata.is_dir() => {
                return Err(CredentialVaultError::Invalid)
            }
            Ok(_) => {}
            Err(error) if allow_missing && error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Err(CredentialVaultError::Invalid)
            }
            Err(_) => return Err(CredentialVaultError::Invalid),
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

fn is_reparse_or_symlink(metadata: &fs::Metadata) -> bool {
    if metadata.file_type().is_symlink() {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        return metadata.file_attributes() & 0x400 != 0;
    }
    #[cfg(not(windows))]
    {
        false
    }
}

fn map_key_wrapper_error(error: traesync_ports::KeyWrapperError) -> CredentialVaultError {
    match error {
        traesync_ports::KeyWrapperError::Unavailable => CredentialVaultError::Unavailable,
        traesync_ports::KeyWrapperError::Failed => CredentialVaultError::Invalid,
    }
}

fn now_unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

fn now_unix_nanos() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn binding() -> CredentialBinding {
        CredentialBinding::new("profile-a", "location-a", "fingerprint-a")
    }

    fn storage(path: &Path, token: &str) {
        fs::write(
            path,
            serde_json::json!({
                "iCubeAuthInfo://icube.cloudide": token,
                "iCubeAuthInfo://icube-dc:device-a": "device-value",
                "productVersion": "1.107.1",
                "machineid": "machine-stable",
                "telemetry": {"enabled": true}
            })
            .to_string(),
        )
        .unwrap();
    }

    fn write_pending_manifest(path: &Path, state: &str) {
        write_pending_manifest_with_intent(path, state, None);
    }

    fn write_pending_manifest_with_intent(path: &Path, state: &str, intent_id: Option<&str>) {
        // 待处理 manifest 只作为 fixture；不写入任何真实恢复区。
        let operation_dir = path
            .join("credential-switches")
            .join("credential-fixture-operation");
        fs::create_dir_all(&operation_dir).unwrap();
        let mut manifest = serde_json::json!({
            "format_version": CREDENTIAL_FORMAT_VERSION,
            "operation_id": "credential-fixture-operation",
            "operation": "credential_apply",
            "sequence": 1,
            "state": state,
            "profile_id": "profile-a",
            "data_location_id": "location-a",
            "user_fingerprint": "fingerprint-a",
            "backup_file": "credential-switches/credential-fixture-operation/storage.json.before",
            "before_sha256": "fixture-before-sha256",
            "target_sha256": "fixture-target-sha256",
            "created_at_unix_seconds": 1
        });
        if let Some(intent_id) = intent_id {
            manifest["intent_id"] = serde_json::Value::String(intent_id.to_string());
        }
        fs::write(operation_dir.join("manifest.json"), manifest.to_string()).unwrap();
    }

    fn link_file(target: &Path, link: &Path) -> bool {
        #[cfg(windows)]
        {
            std::os::windows::fs::symlink_file(target, link).is_ok()
        }
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(target, link).is_ok()
        }
        #[cfg(not(any(windows, unix)))]
        {
            let _ = (target, link);
            false
        }
    }

    fn link_directory(target: &Path, link: &Path) -> bool {
        #[cfg(windows)]
        {
            std::os::windows::fs::symlink_dir(target, link).is_ok()
        }
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(target, link).is_ok()
        }
        #[cfg(not(any(windows, unix)))]
        {
            let _ = (target, link);
            false
        }
    }

    #[cfg(windows)]
    #[test]
    fn dpapi_roundtrip_has_no_auth_plaintext_in_vault_file() {
        let root = tempdir().unwrap();
        let storage_path = root.path().join("storage.json");
        storage(&storage_path, "unique-auth-material-8c2c");
        let vault = CredentialVault::new(root.path().join("vault"));
        let binding = binding();

        let status = vault.capture(&binding, &storage_path).unwrap();
        assert_eq!(status.state, CredentialState::Saved);
        let bytes = fs::read_dir(vault.root()).unwrap().next().unwrap().unwrap();
        let encrypted = fs::read(bytes.path()).unwrap();
        assert!(!encrypted
            .windows("unique-auth-material-8c2c".len())
            .any(|window| window == b"unique-auth-material-8c2c"));
        assert_eq!(vault.status(&binding).state, CredentialState::Saved);
    }

    #[cfg(windows)]
    #[test]
    fn wrong_binding_rejected_without_storage_change() {
        let root = tempdir().unwrap();
        let source = root.path().join("source-storage.json");
        let target = root.path().join("target-storage.json");
        storage(&source, "source-auth");
        storage(&target, "target-auth");
        let vault = CredentialVault::new(root.path().join("vault"));
        let source_before = fs::read(&source).unwrap();
        let original = fs::read(&target).unwrap();
        vault.capture(&binding(), &source).unwrap();
        let wrong = CredentialBinding::new("profile-a", "other-location", "fingerprint-a");

        assert_eq!(vault.status(&wrong).state, CredentialState::Stale);
        assert_eq!(
            vault.apply(&wrong, &target, &root.path().join("recovery"), true),
            Err(CredentialVaultError::BindingMismatch)
        );
        assert_eq!(fs::read(target).unwrap(), original);
        assert_eq!(fs::read(source).unwrap(), source_before);
        assert!(!root.path().join("recovery").exists());
    }

    #[cfg(windows)]
    #[test]
    fn apply_changes_only_auth_namespace_and_preserves_database() {
        let root = tempdir().unwrap();
        let source = root.path().join("source-storage.json");
        let target = root.path().join("target-storage.json");
        let database = root.path().join("database.db");
        storage(&source, "source-auth");
        storage(&target, "old-auth");
        fs::write(&database, b"database-is-not-touched").unwrap();
        let db_before = Sha256::digest(fs::read(&database).unwrap());
        let before_value: serde_json::Value =
            serde_json::from_slice(&fs::read(&target).unwrap()).unwrap();
        let vault = CredentialVault::new(root.path().join("vault"));
        vault.capture(&binding(), &source).unwrap();
        let outcome = vault
            .apply(&binding(), &target, &root.path().join("recovery"), true)
            .unwrap();

        let after_value: serde_json::Value =
            serde_json::from_slice(&fs::read(&target).unwrap()).unwrap();
        assert_eq!(
            after_value["iCubeAuthInfo://icube.cloudide"],
            before_value["iCubeAuthInfo://icube.cloudide"]
                .as_str()
                .unwrap()
                .replace("old-auth", "source-auth")
        );
        assert_eq!(
            non_auth_projection(&after_value),
            non_auth_projection(&before_value)
        );
        assert_eq!(after_value["machineid"], before_value["machineid"]);
        assert_eq!(
            after_value["productVersion"],
            before_value["productVersion"]
        );
        assert_eq!(Sha256::digest(fs::read(&database).unwrap()), db_before);
        let operation_dir = root
            .path()
            .join("recovery")
            .join("credential-switches")
            .join(&outcome.operation_id);
        let backup = fs::read(operation_dir.join("storage.json.before")).unwrap();
        assert!(backup.starts_with(BACKUP_MAGIC));
        assert!(!backup
            .windows("old-auth".len())
            .any(|window| window == b"old-auth"));
        let manifest: serde_json::Value =
            serde_json::from_slice(&fs::read(operation_dir.join("manifest.json")).unwrap())
                .unwrap();
        assert_eq!(manifest["state"], "storage_verified");
        assert_eq!(manifest["sequence"], 3);
        assert_eq!(manifest["operation_id"], outcome.operation_id);
        assert_eq!(
            manifest["backup_file"],
            format!(
                "credential-switches/{}/storage.json.before",
                outcome.operation_id
            )
        );
        assert!(manifest["before_sha256"].as_str().is_some());
        assert!(manifest["target_sha256"].as_str().is_some());
    }

    #[cfg(windows)]
    #[test]
    fn apply_requires_closed_trae_without_any_side_effect() {
        let root = tempdir().unwrap();
        let source = root.path().join("source-storage.json");
        let target = root.path().join("target-storage.json");
        let recovery = root.path().join("recovery");
        storage(&source, "source-auth");
        storage(&target, "target-auth");
        let before = fs::read(&target).unwrap();
        let vault = CredentialVault::new(root.path().join("vault"));
        vault.capture(&binding(), &source).unwrap();

        assert_eq!(
            vault.apply(&binding(), &target, &recovery, false),
            Err(CredentialVaultError::TraeNotClosed)
        );
        assert_eq!(fs::read(&target).unwrap(), before);
        assert!(!recovery.exists());
    }

    #[cfg(windows)]
    #[test]
    fn corrupted_vault_fails_closed_without_target_write() {
        let root = tempdir().unwrap();
        let source = root.path().join("source-storage.json");
        let target = root.path().join("target-storage.json");
        let recovery = root.path().join("recovery");
        storage(&source, "source-auth");
        storage(&target, "target-auth");
        let before = fs::read(&target).unwrap();
        let vault = CredentialVault::new(root.path().join("vault"));
        vault.capture(&binding(), &source).unwrap();
        let credential_path = vault.credential_path("profile-a").unwrap();
        fs::write(&credential_path, b"corrupted-vault-payload").unwrap();

        assert_eq!(vault.status(&binding()).state, CredentialState::Invalid);
        assert_eq!(
            vault.apply(&binding(), &target, &recovery, true),
            Err(CredentialVaultError::Invalid)
        );
        assert_eq!(fs::read(&target).unwrap(), before);
        assert!(!recovery.exists());
    }

    #[test]
    fn oversized_storage_is_rejected_before_vault_write() {
        let root = tempdir().unwrap();
        let storage_path = root.path().join("storage.json");
        let file = File::create(&storage_path).unwrap();
        file.set_len(MAX_STORAGE_JSON_BYTES + 1).unwrap();
        drop(file);
        let vault = CredentialVault::new(root.path().join("vault"));

        assert_eq!(
            vault.capture(&binding(), &storage_path),
            Err(CredentialVaultError::StorageTooLarge)
        );
        assert!(!vault.root().exists());
    }

    #[test]
    fn malformed_storage_is_rejected_before_vault_write() {
        let root = tempdir().unwrap();
        let storage_path = root.path().join("storage.json");
        fs::write(&storage_path, b"not-json").unwrap();
        let vault = CredentialVault::new(root.path().join("vault"));

        assert_eq!(
            vault.capture(&binding(), &storage_path),
            Err(CredentialVaultError::StorageInvalid)
        );
        assert!(!vault.root().exists());
    }

    #[test]
    fn oversized_vault_is_invalid_without_target_write() {
        let root = tempdir().unwrap();
        let target = root.path().join("target-storage.json");
        let recovery = root.path().join("recovery");
        storage(&target, "target-auth");
        let before = fs::read(&target).unwrap();
        let vault = CredentialVault::new(root.path().join("vault"));
        fs::create_dir_all(vault.root()).unwrap();
        let credential_path = vault.credential_path("profile-a").unwrap();
        fs::write(&credential_path, vec![0_u8; (MAX_VAULT_BYTES + 1) as usize]).unwrap();

        assert_eq!(vault.status(&binding()).state, CredentialState::Invalid);
        assert_eq!(
            vault.apply(&binding(), &target, &recovery, true),
            Err(CredentialVaultError::Invalid)
        );
        assert_eq!(fs::read(&target).unwrap(), before);
        assert!(!recovery.exists());
    }

    #[test]
    fn reparse_storage_path_fails_closed_before_vault_write() {
        let root = tempdir().unwrap();
        let outside = root.path().join("outside-storage.json");
        let linked = root.path().join("linked-storage.json");
        storage(&outside, "outside-auth");
        if !link_file(&outside, &linked) {
            return;
        }
        let vault = CredentialVault::new(root.path().join("vault"));

        assert_eq!(
            vault.capture(&binding(), &linked),
            Err(CredentialVaultError::StorageInvalid)
        );
        assert!(!vault.root().exists());
    }

    #[test]
    fn pending_manifest_blocks_apply_and_preserves_external_storage() {
        let root = tempdir().unwrap();
        let target = root.path().join("target-storage.json");
        let recovery = root.path().join("recovery");
        storage(&target, "initial-auth");
        write_pending_manifest(&recovery, "prepared");
        storage(&target, "external-auth");
        let external = fs::read(&target).unwrap();
        let vault = CredentialVault::new(root.path().join("vault"));

        assert!(vault.has_pending_recovery(&recovery).unwrap());
        assert_eq!(
            vault.apply(&binding(), &target, &recovery, true),
            Err(CredentialVaultError::RecoveryRequired)
        );
        assert_eq!(fs::read(&target).unwrap(), external);
    }

    #[test]
    fn storage_verified_and_legacy_verified_manifests_are_pending_and_enumerated() {
        let root = tempdir().unwrap();
        let vault = CredentialVault::new(root.path().join("vault"));

        write_pending_manifest_with_intent(
            root.path(),
            "storage_verified",
            Some("intent-storage-verified"),
        );
        let records = vault.pending_recoveries(root.path()).unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].state, "storage_verified");
        assert_eq!(
            records[0].intent_id.as_deref(),
            Some("intent-storage-verified")
        );
        assert!(vault.has_pending_recovery(root.path()).unwrap());

        write_pending_manifest(root.path(), "verified");
        let records = vault.pending_recoveries(root.path()).unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].state, "verified");
    }

    #[test]
    fn bound_journal_mark_rejects_wrong_binding_and_enforces_monotonic_state() {
        let root = tempdir().unwrap();
        let recovery = root.path().join("recovery");
        let vault = CredentialVault::new(root.path().join("vault"));
        write_pending_manifest_with_intent(&recovery, "storage_verified", Some("intent-a"));

        let wrong_binding = CredentialBinding::new("profile-a", "other-location", "fingerprint-a");
        assert_eq!(
            vault.mark_target_verified_with_binding(
                &recovery,
                "credential-fixture-operation",
                &wrong_binding,
                Some("intent-a"),
            ),
            Err(CredentialVaultError::BindingMismatch)
        );
        assert_eq!(
            vault.pending_recoveries(&recovery).unwrap()[0].state,
            "storage_verified"
        );

        assert_eq!(
            vault.mark_target_verified_with_binding(
                &recovery,
                "credential-fixture-operation",
                &binding(),
                Some("wrong-intent"),
            ),
            Err(CredentialVaultError::BindingMismatch)
        );
        vault
            .mark_target_verified_with_binding(
                &recovery,
                "credential-fixture-operation",
                &binding(),
                Some("intent-a"),
            )
            .unwrap();
        assert!(!vault.has_pending_recovery(&recovery).unwrap());

        // 终态允许幂等重试，但不得回退为人工恢复态。
        vault
            .mark_target_verified_with_binding(
                &recovery,
                "credential-fixture-operation",
                &binding(),
                Some("intent-a"),
            )
            .unwrap();
        assert_eq!(
            vault.mark_manual_recovery_required_with_binding(
                &recovery,
                "credential-fixture-operation",
                &binding(),
                Some("intent-a"),
            ),
            Err(CredentialVaultError::RecoveryRequired)
        );
    }

    #[test]
    fn bound_journal_mark_rejects_direct_target_verification() {
        let root = tempdir().unwrap();
        let recovery = root.path().join("recovery");
        let vault = CredentialVault::new(root.path().join("vault"));
        write_pending_manifest_with_intent(&recovery, "target_written", Some("intent-a"));

        assert_eq!(
            vault.mark_target_verified_with_binding(
                &recovery,
                "credential-fixture-operation",
                &binding(),
                Some("intent-a"),
            ),
            Err(CredentialVaultError::RecoveryRequired)
        );
        assert_eq!(
            vault.pending_recoveries(&recovery).unwrap()[0].state,
            "target_written"
        );
    }

    #[test]
    fn restore_after_failure_preserves_external_storage_change() {
        let root = tempdir().unwrap();
        let storage_path = root.path().join("storage.json");
        let original_path = root.path().join("original-storage.json");
        let intended_target_path = root.path().join("intended-target-storage.json");
        let manifest_path = root
            .path()
            .join("recovery")
            .join("credential-switches")
            .join("credential-fixture-operation")
            .join("manifest.json");
        storage(&original_path, "original-auth");
        storage(&intended_target_path, "intended-target-auth");
        storage(&storage_path, "external-auth");
        let original = fs::read(&original_path).unwrap();
        let target_sha256 = sha256_bytes(&fs::read(&intended_target_path).unwrap());
        let manifest = RecoveryManifest {
            format_version: CREDENTIAL_FORMAT_VERSION,
            operation_id: "credential-fixture-operation".to_string(),
            operation: "credential_apply".to_string(),
            sequence: 2,
            state: "target_written".to_string(),
            profile_id: "profile-a".to_string(),
            data_location_id: "location-a".to_string(),
            user_fingerprint: "fingerprint-a".to_string(),
            backup_file: "credential-switches/credential-fixture-operation/storage.json.before"
                .to_string(),
            before_sha256: sha256_bytes(&original),
            target_sha256: target_sha256.clone(),
            created_at_unix_seconds: 1,
            intent_id: None,
        };
        fs::create_dir_all(manifest_path.parent().unwrap()).unwrap();
        write_manifest(&manifest_path, &manifest).unwrap();
        let external = fs::read(&storage_path).unwrap();
        let vault = CredentialVault::new(root.path().join("vault"));

        assert_eq!(
            vault.restore_after_failure(
                &storage_path,
                &original,
                &manifest_path,
                &manifest,
                &target_sha256,
                CredentialVaultError::StorageInvalid,
            ),
            Err(CredentialVaultError::RecoveryRequired)
        );
        assert_eq!(fs::read(&storage_path).unwrap(), external);
        let persisted: serde_json::Value =
            serde_json::from_slice(&fs::read(&manifest_path).unwrap()).unwrap();
        assert_eq!(persisted["state"], "manual_recovery_required");
    }

    #[cfg(windows)]
    #[test]
    fn reparse_vault_root_fails_closed_without_target_write() {
        let root = tempdir().unwrap();
        let source = root.path().join("source-storage.json");
        let target = root.path().join("target-storage.json");
        let vault_path = root.path().join("vault");
        let real_vault_path = root.path().join("vault-real");
        let recovery = root.path().join("recovery");
        storage(&source, "source-auth");
        storage(&target, "target-auth");
        let before = fs::read(&target).unwrap();
        let vault = CredentialVault::new(&vault_path);
        vault.capture(&binding(), &source).unwrap();
        fs::rename(&vault_path, &real_vault_path).unwrap();
        if !link_directory(&real_vault_path, &vault_path) {
            return;
        }

        assert_eq!(vault.status(&binding()).state, CredentialState::Invalid);
        assert_eq!(
            vault.apply(&binding(), &target, &recovery, true),
            Err(CredentialVaultError::Invalid)
        );
        assert_eq!(fs::read(&target).unwrap(), before);
        assert!(!recovery.exists());
    }

    #[cfg(windows)]
    #[test]
    fn failed_publish_keeps_original_storage() {
        let root = tempdir().unwrap();
        let source = root.path().join("source-storage.json");
        let target = root.path().join("target-storage.json");
        storage(&source, "source-auth");
        storage(&target, "target-auth");
        let original = fs::read(&target).unwrap();
        let vault = CredentialVault::new(root.path().join("vault"));
        vault.capture(&binding(), &source).unwrap();
        let recovery_file = root.path().join("recovery-file");
        fs::write(&recovery_file, b"not-a-directory").unwrap();

        assert!(vault
            .apply(&binding(), &target, &recovery_file, true)
            .is_err());
        assert_eq!(fs::read(target).unwrap(), original);
    }

    #[test]
    fn unknown_auth_key_rejected() {
        let root = tempdir().unwrap();
        let path = root.path().join("storage.json");
        fs::write(
            &path,
            serde_json::json!({"iCubeAuthInfo://unknown": "value"}).to_string(),
        )
        .unwrap();
        let vault = CredentialVault::new(root.path().join("vault"));
        assert_eq!(
            vault.capture(&binding(), &path),
            Err(CredentialVaultError::UnsupportedAuthKey)
        );
    }

    #[cfg(not(windows))]
    #[test]
    fn non_windows_capture_fails_closed() {
        let root = tempdir().unwrap();
        let path = root.path().join("storage.json");
        storage(&path, "source-auth");
        let vault = CredentialVault::new(root.path().join("vault"));
        assert_eq!(
            vault.capture(&binding(), &path),
            Err(CredentialVaultError::Unavailable)
        );
    }
}
