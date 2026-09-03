//! ManagedAccountSwitch 的非敏感账号档案存储。

use std::collections::HashSet;
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use fs2::FileExt;
use openssl::rand::rand_bytes;
use sha2::{Digest, Sha256};
use traesync_domain::{AccountProfile, ACCOUNT_FINGERPRINT_VERSION};
use traesync_ports::ManagedAccountProfileStorePort;

const PROFILE_STORE_FORMAT_VERSION: u32 = 3;
const LEGACY_PROFILE_STORE_FORMAT_VERSION: u32 = 2;
const PROFILE_SALT_MAGIC: &[u8; 8] = b"TRASALT2";
const PROFILE_SALT_BYTES: usize = 32;
const PROFILE_STORE_MAX_BYTES: u64 = 1024 * 1024;
const TEMPORARY_NAME_BYTES: usize = 16;
const TEMPORARY_NAME_ATTEMPTS: usize = 16;
const PROFILE_STORE_LOCK_SUFFIX: &str = ".lock";
const PROFILE_STORE_LOCK_TIMEOUT: Duration = Duration::from_secs(2);
const PROFILE_STORE_LOCK_RETRY: Duration = Duration::from_millis(10);

#[derive(Debug, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct ProfileStoreEnvelope {
    format_version: u32,
    fingerprint_salt_id: String,
    profiles: Vec<AccountProfile>,
}

#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct LegacyProfileStoreEnvelope {
    format_version: u32,
    profiles: Vec<AccountProfile>,
}

/// 读取或首次创建安装级账号摘要 salt；文件不包含 user_id 或认证材料。
pub fn load_or_create_account_fingerprint_salt(path: &Path) -> Result<Vec<u8>, String> {
    // 目录句柄覆盖整次检查、创建和发布，避免目录被换成重解析点。
    let _directory_guard = protect_parent_directory(
        path,
        true,
        "account_fingerprint_salt_invalid",
        "account_fingerprint_salt_unavailable",
    )?
    .ok_or_else(|| "account_fingerprint_salt_unavailable".to_string())?;
    if let Some(salt) = load_existing_account_fingerprint_salt(path)? {
        return Ok(salt);
    }

    // 多实例可能同时首次创建；硬链接发布只允许一个胜者。
    if let Some(salt) = load_existing_account_fingerprint_salt(path)? {
        return Ok(salt);
    }

    let mut salt = vec![0_u8; PROFILE_SALT_BYTES];
    rand_bytes(&mut salt).map_err(|_| "account_fingerprint_salt_unavailable".to_string())?;
    let mut payload = Vec::with_capacity(PROFILE_SALT_MAGIC.len() + PROFILE_SALT_BYTES);
    payload.extend_from_slice(PROFILE_SALT_MAGIC);
    payload.extend_from_slice(&salt);
    let parent = parent_directory(path, "account_fingerprint_salt_invalid")?;
    let temporary = write_unique_temporary_file(
        parent,
        path,
        &payload,
        "account_fingerprint_salt_unavailable",
    )?;

    let publication = (|| {
        let temporary_bytes = read_existing_file_limited(
            &temporary,
            payload.len() as u64,
            "account_fingerprint_salt_unavailable",
            "account_fingerprint_salt_unavailable",
        )?
        .ok_or_else(|| "account_fingerprint_salt_unavailable".to_string())?;
        if temporary_bytes != payload {
            return Err("account_fingerprint_salt_unavailable".to_string());
        }
        if open_existing_regular_file(
            path,
            "account_fingerprint_salt_invalid",
            "account_fingerprint_salt_unavailable",
        )?
        .is_some()
        {
            return Err("account_fingerprint_salt_unavailable".to_string());
        }
        publish_without_replacing(&temporary, path)
            .map_err(|_| "account_fingerprint_salt_unavailable".to_string())
    })();

    if publication.is_err() {
        remove_owned_temporary_file(&temporary);
    }

    // hard_link 只允许一个创建者成功；所有失败者都重读最终已发布 salt，避免指纹分裂。
    match load_existing_account_fingerprint_salt(path) {
        Ok(Some(published_salt)) => Ok(published_salt),
        Ok(None) => Err("account_fingerprint_salt_unavailable".to_string()),
        Err(error) => Err(error),
    }
}

/// 只读取既有安装级 salt；缺失时返回 None，绝不创建目录或文件。
pub fn load_account_fingerprint_salt(path: &Path) -> Result<Option<Vec<u8>>, String> {
    let Some(_directory_guard) = protect_parent_directory(
        path,
        false,
        "account_fingerprint_salt_invalid",
        "account_fingerprint_salt_unavailable",
    )?
    else {
        return Ok(None);
    };
    load_existing_account_fingerprint_salt(path)
}

/// 使用安装级 salt 派生不可逆账号摘要；不同安装默认不可关联。
pub fn salted_user_id_fingerprint(salt: &[u8], user_id: &str) -> String {
    let mut digest = Sha256::new();
    digest.update(b"trae-sync-account-fingerprint-v2");
    digest.update([0]);
    digest.update(salt);
    digest.update([0]);
    digest.update(user_id.as_bytes());
    format!(
        "v{}-{}",
        ACCOUNT_FINGERPRINT_VERSION,
        hex::encode(digest.finalize())
    )
}

/// 使用 JSON 保存账号元数据；文件中没有认证对象、token 或 cookie。
pub struct JsonManagedAccountProfileStore {
    path: PathBuf,
    fingerprint_salt: Option<Vec<u8>>,
    create_missing: bool,
}

impl JsonManagedAccountProfileStore {
    /// 构造只读档案库；缺少 salt 时仅允许读取 legacy v1 档案。
    pub fn for_read(
        path: impl Into<PathBuf>,
        fingerprint_salt: Option<Vec<u8>>,
    ) -> Result<Self, String> {
        if fingerprint_salt
            .as_ref()
            .is_some_and(|salt| salt.len() != PROFILE_SALT_BYTES)
        {
            return Err("account_fingerprint_salt_invalid".to_string());
        }
        Ok(Self {
            path: path.into(),
            fingerprint_salt,
            create_missing: false,
        })
    }

    /// 生产账号档案必须使用安装级 salt；salt 只驻留后端内存，不进入 JSON/UI。
    pub fn with_fingerprint_salt(path: impl Into<PathBuf>, salt: Vec<u8>) -> Result<Self, String> {
        let mut store = Self::for_read(path, Some(salt))?;
        store.create_missing = true;
        Ok(store)
    }

    pub fn fingerprint_user_id(&self, user_id: &str) -> Result<String, String> {
        let salt = self
            .fingerprint_salt
            .as_deref()
            .ok_or_else(|| "account_fingerprint_salt_unavailable".to_string())?;
        Ok(salted_user_id_fingerprint(salt, user_id))
    }

    fn read_profiles_under_guard(&self) -> Result<Vec<AccountProfile>, String> {
        let Some(bytes) = read_existing_file_limited(
            &self.path,
            PROFILE_STORE_MAX_BYTES,
            "account_profile_store_invalid",
            "account_profile_store_unavailable",
        )?
        else {
            return Ok(Vec::new());
        };
        parse_profiles(&bytes, self.fingerprint_salt.as_deref())
    }
}

impl ManagedAccountProfileStorePort for JsonManagedAccountProfileStore {
    fn load_profiles(&self) -> Result<Vec<AccountProfile>, String> {
        let Some(_directory_guard) = protect_parent_directory(
            &self.path,
            self.create_missing,
            "account_profile_store_path_invalid",
            "account_profile_store_unavailable",
        )?
        else {
            return Ok(Vec::new());
        };
        // 普通读取不得为了加锁创建 `.lock`。若写端尚未创建锁文件，原子发布已保证
        // 读取者只会看到旧版或新版完整档案。
        let _lock = if self.create_missing {
            Some(ProfileStoreLock::acquire_shared(&profile_store_lock_path(
                &self.path,
            ))?)
        } else {
            ProfileStoreLock::acquire_shared_existing(&profile_store_lock_path(&self.path))?
        };
        self.read_profiles_under_guard()
    }

    fn upsert_profile(&self, profile: AccountProfile) -> Result<Vec<AccountProfile>, String> {
        let fingerprint_salt = self
            .fingerprint_salt
            .as_deref()
            .ok_or_else(|| "account_fingerprint_salt_unavailable".to_string())?;
        if !self.create_missing {
            return Err("account_profile_store_read_only".to_string());
        }
        validate_current_profile(&profile)?;
        let _directory_guard = protect_parent_directory(
            &self.path,
            true,
            "account_profile_store_path_invalid",
            "account_profile_store_unavailable",
        )?
        .ok_or_else(|| "account_profile_store_unavailable".to_string())?;
        let _lock = ProfileStoreLock::acquire_exclusive(&profile_store_lock_path(&self.path))?;

        // 读、身份合并、验证、发布和发布后复读均在同一锁与目录句柄内。
        let mut profiles = self.read_profiles_under_guard()?;
        merge_profile(&mut profiles, profile);
        // 合并后再验证完整集合，避免 profile_id 碰撞或重复身份先发布、后报错。
        validate_profiles(&profiles)?;
        let payload = serde_json::to_vec_pretty(&ProfileStoreEnvelope {
            format_version: PROFILE_STORE_FORMAT_VERSION,
            fingerprint_salt_id: fingerprint_salt_id(fingerprint_salt),
            profiles,
        })
        .map_err(|_| "account_profile_store_invalid".to_string())?;
        if payload.len() as u64 > PROFILE_STORE_MAX_BYTES {
            return Err("account_profile_store_invalid".to_string());
        }
        let parent = parent_directory(&self.path, "account_profile_store_path_invalid")?;
        let temporary = write_unique_temporary_file(
            parent,
            &self.path,
            &payload,
            "account_profile_store_unavailable",
        )?;
        let result = (|| {
            let temporary_bytes = read_existing_file_limited(
                &temporary,
                PROFILE_STORE_MAX_BYTES,
                "account_profile_store_unavailable",
                "account_profile_store_unavailable",
            )?
            .ok_or_else(|| "account_profile_store_unavailable".to_string())?;
            if temporary_bytes != payload {
                return Err("account_profile_store_unavailable".to_string());
            }
            crate::atomic_publish::publish_replacing(&temporary, &self.path)
                .map_err(|_| "account_profile_store_unavailable".to_string())?;
            let published = read_existing_file_limited(
                &self.path,
                PROFILE_STORE_MAX_BYTES,
                "account_profile_store_invalid",
                "account_profile_store_unavailable",
            )?
            .ok_or_else(|| "account_profile_store_unavailable".to_string())?;
            if published != payload {
                return Err("account_profile_store_unavailable".to_string());
            }
            parse_profiles(&published, Some(fingerprint_salt))
        })();
        if result.is_err() {
            remove_owned_temporary_file(&temporary);
        }
        result
    }
}

fn merge_profile(profiles: &mut Vec<AccountProfile>, profile: AccountProfile) {
    if let Some(existing) = profiles.iter_mut().find(|existing| {
        existing.data_location_id == profile.data_location_id
            && existing.user_fingerprint == profile.user_fingerprint
            && existing.fingerprint_version == profile.fingerprint_version
    }) {
        *existing = profile;
    } else {
        profiles.push(profile);
    }
}

fn parse_profiles(
    bytes: &[u8],
    fingerprint_salt: Option<&[u8]>,
) -> Result<Vec<AccountProfile>, String> {
    let value: serde_json::Value =
        serde_json::from_slice(bytes).map_err(|_| "account_profile_store_invalid".to_string())?;
    let mut profiles = if value.is_array() {
        let profiles = serde_json::from_value::<Vec<AccountProfile>>(value)
            .map_err(|_| "account_profile_store_invalid".to_string())?;
        validate_legacy_profiles(profiles)?
    } else {
        let format_version = value
            .get("format_version")
            .and_then(serde_json::Value::as_u64)
            .ok_or_else(|| "account_profile_store_invalid".to_string())?;
        match format_version as u32 {
            PROFILE_STORE_FORMAT_VERSION => {
                let envelope = serde_json::from_value::<ProfileStoreEnvelope>(value)
                    .map_err(|_| "account_profile_store_invalid".to_string())?;
                let fingerprint_salt = fingerprint_salt
                    .ok_or_else(|| "account_fingerprint_salt_unavailable".to_string())?;
                if envelope.fingerprint_salt_id != fingerprint_salt_id(fingerprint_salt) {
                    return Err("account_profile_store_salt_mismatch".to_string());
                }
                validate_profiles(&envelope.profiles)?;
                envelope.profiles
            }
            LEGACY_PROFILE_STORE_FORMAT_VERSION => {
                let envelope = serde_json::from_value::<LegacyProfileStoreEnvelope>(value)
                    .map_err(|_| "account_profile_store_invalid".to_string())?;
                if envelope.format_version != LEGACY_PROFILE_STORE_FORMAT_VERSION {
                    return Err("account_profile_store_invalid".to_string());
                }
                validate_legacy_profiles(envelope.profiles)?
            }
            _ => return Err("account_profile_store_invalid".to_string()),
        }
    };
    mark_legacy_profiles_for_reverification(&mut profiles);
    Ok(profiles)
}

fn fingerprint_salt_id(salt: &[u8]) -> String {
    let mut digest = Sha256::new();
    digest.update(b"trae-sync-account-profile-salt-id-v1");
    digest.update([0]);
    digest.update(salt);
    format!("v1-{}", hex::encode(digest.finalize()))
}

fn validate_legacy_profiles(profiles: Vec<AccountProfile>) -> Result<Vec<AccountProfile>, String> {
    validate_profiles(&profiles)?;
    if profiles.iter().any(|profile| {
        profile.fingerprint_version != traesync_domain::LEGACY_ACCOUNT_FINGERPRINT_VERSION
    }) {
        return Err("account_profile_store_salt_binding_required".to_string());
    }
    Ok(profiles)
}

fn validate_current_profile(profile: &AccountProfile) -> Result<(), String> {
    if profile.profile_id.is_empty()
        || profile.data_location_id.is_empty()
        || profile.fingerprint_version != ACCOUNT_FINGERPRINT_VERSION
        || !valid_current_fingerprint(&profile.user_fingerprint)
    {
        return Err("account_profile_store_invalid".to_string());
    }
    Ok(())
}

fn validate_profiles(profiles: &[AccountProfile]) -> Result<(), String> {
    let mut profile_ids = HashSet::new();
    let mut identities = HashSet::new();
    for profile in profiles {
        if profile.profile_id.is_empty()
            || profile.data_location_id.is_empty()
            || profile.user_fingerprint.is_empty()
            || !matches!(
                profile.fingerprint_version,
                traesync_domain::LEGACY_ACCOUNT_FINGERPRINT_VERSION | ACCOUNT_FINGERPRINT_VERSION
            )
            || (profile.fingerprint_version == ACCOUNT_FINGERPRINT_VERSION
                && !valid_current_fingerprint(&profile.user_fingerprint))
            || !profile_ids.insert(profile.profile_id.clone())
            || !identities.insert((
                profile.data_location_id.clone(),
                profile.user_fingerprint.clone(),
                profile.fingerprint_version,
            ))
        {
            return Err("account_profile_store_invalid".to_string());
        }
    }
    Ok(())
}

fn valid_current_fingerprint(value: &str) -> bool {
    let Some(encoded) = value.strip_prefix("v2-") else {
        return false;
    };
    encoded.len() == 64
        && encoded
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn mark_legacy_profiles_for_reverification(profiles: &mut [AccountProfile]) {
    for profile in profiles {
        if profile.fingerprint_version == traesync_domain::LEGACY_ACCOUNT_FINGERPRINT_VERSION
            && profile.verification_state == traesync_domain::AccountVerificationState::Verified
        {
            profile.verification_state =
                traesync_domain::AccountVerificationState::FingerprintChanged;
        }
    }
}

fn parent_directory<'a>(path: &'a Path, invalid: &str) -> Result<&'a Path, String> {
    let parent = path.parent().ok_or_else(|| invalid.to_string())?;
    if parent.as_os_str().is_empty() {
        return Err(invalid.to_string());
    }
    Ok(parent)
}

/// 保存目录链的打开句柄；Windows 上句柄拒绝 delete share，阻止目录被重命名或删除。
struct ProtectedDirectoryChain {
    #[cfg(windows)]
    _handles: Vec<File>,
}

/// 文件打开结果区分缺失、碰撞、重解析点和常规 I/O 错误，避免误把异常当作空档案。
enum SecurePathOpenError {
    Missing,
    AlreadyExists,
    Invalid,
    Unavailable,
}

/// 在账户路径的整个父目录链上建立防重解析守卫。
fn protect_parent_directory(
    path: &Path,
    create_missing: bool,
    invalid: &str,
    unavailable: &str,
) -> Result<Option<ProtectedDirectoryChain>, String> {
    let parent = parent_directory(path, invalid)?;

    #[cfg(windows)]
    {
        if !parent.is_absolute() {
            return Err(invalid.to_string());
        }
        let mut handles = Vec::new();
        let chain = parent.ancestors().collect::<Vec<_>>();
        for directory in chain.into_iter().rev() {
            match open_directory_no_reparse(directory) {
                Ok(handle) => handles.push(handle),
                Err(SecurePathOpenError::Missing) if !create_missing => return Ok(None),
                Err(SecurePathOpenError::Missing) => {
                    match fs::create_dir(directory) {
                        Ok(()) => {}
                        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                        Err(_) => return Err(unavailable.to_string()),
                    }
                    match open_directory_no_reparse(directory) {
                        Ok(handle) => handles.push(handle),
                        Err(SecurePathOpenError::Invalid) => return Err(invalid.to_string()),
                        Err(_) => return Err(unavailable.to_string()),
                    }
                }
                Err(SecurePathOpenError::Invalid) => return Err(invalid.to_string()),
                Err(_) => return Err(unavailable.to_string()),
            }
        }
        Ok(Some(ProtectedDirectoryChain { _handles: handles }))
    }

    #[cfg(not(windows))]
    {
        validate_directory_chain(parent, invalid)?;
        if !parent.exists() {
            if !create_missing {
                return Ok(None);
            }
            fs::create_dir_all(parent).map_err(|_| unavailable.to_string())?;
            validate_directory_chain(parent, invalid)?;
        }
        Ok(Some(ProtectedDirectoryChain {}))
    }
}

#[cfg(windows)]
fn open_directory_no_reparse(path: &Path) -> Result<File, SecurePathOpenError> {
    use windows_sys::Win32::Foundation::GENERIC_READ;
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
    };

    // 允许目录中的正常读写；刻意不共享删除，持续阻止目录被重命名或移除。
    open_windows_file_no_reparse(
        path,
        GENERIC_READ,
        FILE_SHARE_READ | FILE_SHARE_WRITE,
        OPEN_EXISTING,
        true,
    )
}

#[cfg(windows)]
fn open_windows_file_no_reparse(
    path: &Path,
    access: u32,
    share_mode: u32,
    creation_disposition: u32,
    expect_directory: bool,
) -> Result<File, SecurePathOpenError> {
    use std::os::windows::ffi::OsStrExt;
    use std::os::windows::io::{AsRawHandle, FromRawHandle};
    use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION,
        FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_NORMAL, FILE_ATTRIBUTE_REPARSE_POINT,
        FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
    };

    let wide = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let flags = FILE_ATTRIBUTE_NORMAL | FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT;
    let handle = unsafe {
        CreateFileW(
            wide.as_ptr(),
            access,
            share_mode,
            std::ptr::null(),
            creation_disposition,
            flags,
            std::ptr::null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return Err(match std::io::Error::last_os_error().kind() {
            std::io::ErrorKind::NotFound => SecurePathOpenError::Missing,
            std::io::ErrorKind::AlreadyExists => SecurePathOpenError::AlreadyExists,
            _ => SecurePathOpenError::Unavailable,
        });
    }
    // 句柄从这里接管，后续属性校验失败也会自动关闭。
    let file = unsafe { File::from_raw_handle(handle) };
    let mut information = std::mem::MaybeUninit::<BY_HANDLE_FILE_INFORMATION>::zeroed();
    let ok = unsafe { GetFileInformationByHandle(file.as_raw_handle(), information.as_mut_ptr()) };
    if ok == 0 {
        return Err(SecurePathOpenError::Unavailable);
    }
    let attributes = unsafe { information.assume_init().dwFileAttributes };
    let is_directory = attributes & FILE_ATTRIBUTE_DIRECTORY != 0;
    if attributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 || is_directory != expect_directory {
        return Err(SecurePathOpenError::Invalid);
    }
    Ok(file)
}

#[cfg(not(windows))]
fn validate_directory_chain(path: &Path, invalid: &str) -> Result<(), String> {
    let mut current = Some(path);
    while let Some(candidate) = current {
        match fs::symlink_metadata(candidate) {
            Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
                return Err(invalid.to_string())
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return Err(invalid.to_string()),
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

/// 用拒绝重解析点的句柄打开既有普通文件；Windows 不先做可交换的路径元数据检查。
fn open_existing_regular_file(
    path: &Path,
    invalid: &str,
    unavailable: &str,
) -> Result<Option<File>, String> {
    #[cfg(windows)]
    {
        use windows_sys::Win32::Foundation::GENERIC_READ;
        use windows_sys::Win32::Storage::FileSystem::{FILE_SHARE_READ, OPEN_EXISTING};

        return match open_windows_file_no_reparse(
            path,
            GENERIC_READ,
            FILE_SHARE_READ,
            OPEN_EXISTING,
            false,
        ) {
            Ok(file) => Ok(Some(file)),
            Err(SecurePathOpenError::Missing) => Ok(None),
            Err(SecurePathOpenError::Invalid) => Err(invalid.to_string()),
            Err(_) => Err(unavailable.to_string()),
        };
    }

    #[cfg(not(windows))]
    {
        let metadata = match fs::symlink_metadata(path) {
            Ok(metadata) if !metadata.file_type().is_symlink() && metadata.is_file() => metadata,
            Ok(_) => return Err(invalid.to_string()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(_) => return Err(unavailable.to_string()),
        };
        let file = File::open(path).map_err(|_| unavailable.to_string())?;
        if file.metadata().map(|actual| actual.len()).ok() != Some(metadata.len()) {
            return Err(invalid.to_string());
        }
        Ok(Some(file))
    }
}

/// 打开或创建锁文件。锁文件本身也拒绝重解析点，且不授予 delete sharing。
fn open_or_create_regular_file(path: &Path) -> Result<File, SecurePathOpenError> {
    #[cfg(windows)]
    {
        use windows_sys::Win32::Foundation::{GENERIC_READ, GENERIC_WRITE};
        use windows_sys::Win32::Storage::FileSystem::{
            FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_ALWAYS,
        };

        return open_windows_file_no_reparse(
            path,
            GENERIC_READ | GENERIC_WRITE,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            OPEN_ALWAYS,
            false,
        );
    }

    #[cfg(not(windows))]
    {
        use std::fs::OpenOptions;

        match fs::symlink_metadata(path) {
            Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
                return Err(SecurePathOpenError::Invalid)
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return Err(SecurePathOpenError::Unavailable),
        }
        OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .open(path)
            .map_err(|_| SecurePathOpenError::Unavailable)
    }
}

/// 使用 create_new 创建随机临时文件，既有名称一律重试，绝不跟随重解析点。
fn create_new_regular_file(path: &Path) -> Result<File, SecurePathOpenError> {
    #[cfg(windows)]
    {
        use windows_sys::Win32::Foundation::{GENERIC_READ, GENERIC_WRITE};
        use windows_sys::Win32::Storage::FileSystem::{CREATE_NEW, FILE_SHARE_READ};

        return open_windows_file_no_reparse(
            path,
            GENERIC_READ | GENERIC_WRITE,
            FILE_SHARE_READ,
            CREATE_NEW,
            false,
        );
    }

    #[cfg(not(windows))]
    {
        use std::fs::OpenOptions;

        OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(path)
            .map_err(|error| match error.kind() {
                std::io::ErrorKind::AlreadyExists => SecurePathOpenError::AlreadyExists,
                _ => SecurePathOpenError::Unavailable,
            })
    }
}

/// 读取前后均检查同一已验证句柄的长度，限制损坏文件的内存占用。
fn read_existing_file_limited(
    path: &Path,
    maximum_bytes: u64,
    invalid: &str,
    unavailable: &str,
) -> Result<Option<Vec<u8>>, String> {
    let Some(mut file) = open_existing_regular_file(path, invalid, unavailable)? else {
        return Ok(None);
    };
    let before = file.metadata().map_err(|_| unavailable.to_string())?;
    if before.len() > maximum_bytes {
        return Err(invalid.to_string());
    }
    let mut bytes = Vec::with_capacity(before.len() as usize);
    Read::by_ref(&mut file)
        .take(maximum_bytes.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|_| unavailable.to_string())?;
    if bytes.len() as u64 > maximum_bytes {
        return Err(invalid.to_string());
    }
    let after = file.metadata().map_err(|_| unavailable.to_string())?;
    if after.len() != before.len() || after.len() != bytes.len() as u64 {
        return Err(invalid.to_string());
    }
    Ok(Some(bytes))
}

fn load_existing_account_fingerprint_salt(path: &Path) -> Result<Option<Vec<u8>>, String> {
    let Some(bytes) = read_existing_file_limited(
        path,
        (PROFILE_SALT_MAGIC.len() + PROFILE_SALT_BYTES) as u64,
        "account_fingerprint_salt_invalid",
        "account_fingerprint_salt_unavailable",
    )?
    else {
        return Ok(None);
    };
    if bytes.len() != PROFILE_SALT_MAGIC.len() + PROFILE_SALT_BYTES
        || !bytes.starts_with(PROFILE_SALT_MAGIC)
    {
        return Err("account_fingerprint_salt_invalid".to_string());
    }
    Ok(Some(bytes[PROFILE_SALT_MAGIC.len()..].to_vec()))
}

/// 使用随机名称和 create_new 写入同目录临时文件，避免固定路径被链接劫持。
fn write_unique_temporary_file(
    parent: &Path,
    destination: &Path,
    payload: &[u8],
    unavailable: &str,
) -> Result<PathBuf, String> {
    let name = destination
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("managed-account-store");
    for _ in 0..TEMPORARY_NAME_ATTEMPTS {
        let mut random = [0_u8; TEMPORARY_NAME_BYTES];
        rand_bytes(&mut random).map_err(|_| unavailable.to_string())?;
        let temporary = parent.join(format!(".{name}.tmp-{}", hex::encode(random)));
        let file = match create_new_regular_file(&temporary) {
            Ok(file) => file,
            Err(SecurePathOpenError::AlreadyExists) => continue,
            Err(_) => return Err(unavailable.to_string()),
        };
        let result = (|| {
            let mut file = file;
            file.write_all(payload)
                .and_then(|_| file.sync_all())
                .map_err(|_| unavailable.to_string())
        })();
        if let Err(error) = result {
            remove_owned_temporary_file(&temporary);
            return Err(error);
        }
        return Ok(temporary);
    }
    Err(unavailable.to_string())
}

/// 临时文件由本次调用随机创建，发布失败后可安全清理。
fn remove_owned_temporary_file(path: &Path) {
    let _ = fs::remove_file(path);
}

/// Windows 使用不替换且 write-through 的原子移动发布 salt，竞争者随后重读胜者。
#[cfg(windows)]
fn publish_without_replacing(temporary: &Path, destination: &Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{MoveFileExW, MOVEFILE_WRITE_THROUGH};

    let temporary = temporary
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let destination = destination
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let result = unsafe {
        MoveFileExW(
            temporary.as_ptr(),
            destination.as_ptr(),
            MOVEFILE_WRITE_THROUGH,
        )
    };
    if result == 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

/// 非 Windows 测试环境使用同卷硬链接保持“不替换”语义。
#[cfg(not(windows))]
fn publish_without_replacing(temporary: &Path, destination: &Path) -> std::io::Result<()> {
    fs::hard_link(temporary, destination)?;
    fs::remove_file(temporary)
}

fn profile_store_lock_path(path: &Path) -> PathBuf {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("managed-account-store");
    parent.join(format!(".{name}{PROFILE_STORE_LOCK_SUFFIX}"))
}

/// 专用锁覆盖读、合并、发布和验证；锁忙时不尝试写入。
struct ProfileStoreLock {
    file: File,
}

impl ProfileStoreLock {
    fn acquire_shared(path: &Path) -> Result<Self, String> {
        Self::acquire(path, false)
    }

    fn acquire_exclusive(path: &Path) -> Result<Self, String> {
        Self::acquire(path, true)
    }

    fn acquire_shared_existing(path: &Path) -> Result<Option<Self>, String> {
        let Some(file) = open_existing_regular_file(
            path,
            "account_profile_store_path_invalid",
            "account_profile_store_unavailable",
        )?
        else {
            return Ok(None);
        };
        Self::acquire_file(file, false).map(Some)
    }

    fn acquire(path: &Path, exclusive: bool) -> Result<Self, String> {
        let file = match open_or_create_regular_file(path) {
            Ok(file) => file,
            Err(SecurePathOpenError::Invalid) => {
                return Err("account_profile_store_path_invalid".to_string())
            }
            Err(_) => return Err("account_profile_store_unavailable".to_string()),
        };
        Self::acquire_file(file, exclusive)
    }

    fn acquire_file(file: File, exclusive: bool) -> Result<Self, String> {
        let started = Instant::now();
        loop {
            let result = if exclusive {
                FileExt::try_lock_exclusive(&file)
            } else {
                FileExt::try_lock_shared(&file)
            };
            if result.is_ok() {
                return Ok(Self { file });
            }
            if started.elapsed() >= PROFILE_STORE_LOCK_TIMEOUT {
                return Err("account_profile_store_busy".to_string());
            }
            std::thread::sleep(PROFILE_STORE_LOCK_RETRY);
        }
    }
}

impl Drop for ProfileStoreLock {
    fn drop(&mut self) {
        let _ = self.file.unlock();
    }
}

/// 测试辅助：判断档案序列化结果是否意外出现敏感字段名。
pub fn profile_store_payload_is_non_sensitive(path: &Path) -> bool {
    let Ok(Some(_directory_guard)) = protect_parent_directory(
        path,
        false,
        "account_profile_store_invalid",
        "account_profile_store_unavailable",
    ) else {
        return false;
    };
    let Ok(_lock) = ProfileStoreLock::acquire_shared(&profile_store_lock_path(path)) else {
        return false;
    };
    let Ok(Some(bytes)) = read_existing_file_limited(
        path,
        PROFILE_STORE_MAX_BYTES,
        "account_profile_store_invalid",
        "account_profile_store_unavailable",
    ) else {
        return false;
    };
    let Ok(text) = String::from_utf8(bytes) else {
        return false;
    };
    let lower = text.to_ascii_lowercase();
    !["token", "cookie", "jwt", "refresh_token", "authorization"]
        .iter()
        .any(|field| lower.contains(field))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;
    use std::sync::{Arc, Barrier};
    use std::thread;
    use std::time::SystemTime;
    use traesync_domain::{AccountVerificationState, LEGACY_ACCOUNT_FINGERPRINT_VERSION};

    fn profile() -> AccountProfile {
        profile_with_id("one")
    }

    fn profile_with_id(id: &str) -> AccountProfile {
        AccountProfile {
            profile_id: format!("profile-{id}"),
            display_name: format!("账号 {id}"),
            user_fingerprint: salted_user_id_fingerprint(&test_salt(), &format!("user-{id}")),
            fingerprint_version: traesync_domain::ACCOUNT_FINGERPRINT_VERSION,
            region: Some("cn".to_string()),
            data_location_id: format!("location-{id}"),
            last_verified_at: Some(SystemTime::UNIX_EPOCH),
            verification_state: AccountVerificationState::Verified,
        }
    }

    fn test_salt() -> Vec<u8> {
        vec![7_u8; PROFILE_SALT_BYTES]
    }

    fn test_store(path: impl Into<PathBuf>) -> JsonManagedAccountProfileStore {
        JsonManagedAccountProfileStore::with_fingerprint_salt(path, test_salt()).unwrap()
    }

    fn make_directory_link(target: &Path, link: &Path) -> bool {
        #[cfg(windows)]
        {
            std::process::Command::new("cmd")
                .args([
                    "/C",
                    "mklink",
                    "/J",
                    &link.to_string_lossy(),
                    &target.to_string_lossy(),
                ])
                .status()
                .map(|status| status.success())
                .unwrap_or(false)
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

    #[test]
    fn json_store_round_trips_only_non_sensitive_profile_metadata() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("managed-accounts.json");
        let store = test_store(&path);
        store.upsert_profile(profile()).unwrap();
        assert_eq!(store.load_profiles().unwrap(), vec![profile()]);
        assert!(profile_store_payload_is_non_sensitive(&path));
        store.upsert_profile(profile()).unwrap();
        assert_eq!(store.load_profiles().unwrap(), vec![profile()]);
    }

    #[test]
    fn fingerprint_salt_is_stable_across_restarts() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("state").join("account-fingerprint.salt");

        let first = load_or_create_account_fingerprint_salt(&path).unwrap();
        let second = load_or_create_account_fingerprint_salt(&path).unwrap();

        assert_eq!(first, second);
        let bytes = fs::read(path).unwrap();
        assert_eq!(bytes.len(), PROFILE_SALT_MAGIC.len() + PROFILE_SALT_BYTES);
        assert!(bytes.starts_with(PROFILE_SALT_MAGIC));
    }

    #[test]
    fn concurrent_first_salt_creation_converges_on_one_value() {
        let root = tempfile::tempdir().unwrap();
        let path = Arc::new(root.path().join("state").join("account-fingerprint.salt"));
        let barrier = Arc::new(Barrier::new(2));
        let mut workers = Vec::new();
        for _ in 0..2 {
            let path = Arc::clone(&path);
            let barrier = Arc::clone(&barrier);
            workers.push(thread::spawn(move || {
                barrier.wait();
                load_or_create_account_fingerprint_salt(&path)
            }));
        }

        let first = workers.remove(0).join().unwrap().unwrap();
        let second = workers.remove(0).join().unwrap().unwrap();

        assert_eq!(first, second);
        assert_eq!(fs::read(&*path).unwrap()[PROFILE_SALT_MAGIC.len()..], first);
    }

    #[test]
    fn malformed_salt_fails_closed_without_overwrite() {
        let root = tempfile::tempdir().unwrap();
        let cases = [
            ("short", b"short".to_vec()),
            (
                "wrong-magic",
                [b"BADMAGIC".as_slice(), &[7_u8; PROFILE_SALT_BYTES]].concat(),
            ),
            (
                "long",
                [
                    PROFILE_SALT_MAGIC.as_slice(),
                    &[9_u8; PROFILE_SALT_BYTES + 1],
                ]
                .concat(),
            ),
        ];

        for (name, expected) in cases {
            let path = root.path().join(format!("{name}.salt"));
            fs::write(&path, &expected).unwrap();

            assert_eq!(
                load_or_create_account_fingerprint_salt(&path),
                Err("account_fingerprint_salt_invalid".to_string())
            );
            assert_eq!(fs::read(path).unwrap(), expected);
        }
    }

    #[test]
    fn different_salts_make_user_fingerprints_unlinkable() {
        let first = salted_user_id_fingerprint(&[1_u8; PROFILE_SALT_BYTES], "same-user");
        let second = salted_user_id_fingerprint(&[2_u8; PROFILE_SALT_BYTES], "same-user");

        assert_ne!(first, second);
        assert_eq!(
            first,
            salted_user_id_fingerprint(&[1_u8; PROFILE_SALT_BYTES], "same-user")
        );
    }

    #[test]
    fn salt_leaf_reparse_point_is_rejected_without_touching_external_directory() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let path = root.path().join("account-fingerprint.salt");
        assert!(
            make_directory_link(outside.path(), &path),
            "无法建立 junction fixture"
        );

        assert_eq!(
            load_or_create_account_fingerprint_salt(&path),
            Err("account_fingerprint_salt_invalid".to_string())
        );
        assert!(!outside.path().join("account-fingerprint.salt").exists());
    }

    #[test]
    fn salt_parent_link_is_rejected_without_creating_external_file() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let linked_parent = root.path().join("linked");
        assert!(
            make_directory_link(outside.path(), &linked_parent),
            "无法建立目录 symlink/junction fixture"
        );
        let path = linked_parent.join("account-fingerprint.salt");

        assert_eq!(
            load_or_create_account_fingerprint_salt(&path),
            Err("account_fingerprint_salt_invalid".to_string())
        );
        assert!(!outside.path().join("account-fingerprint.salt").exists());
    }

    #[test]
    fn salt_publication_never_replaces_existing_file() {
        let root = tempfile::tempdir().unwrap();
        let destination = root.path().join("account-fingerprint.salt");
        let first = root.path().join("first.tmp");
        let second = root.path().join("second.tmp");
        fs::write(&first, b"first").unwrap();
        fs::write(&second, b"second").unwrap();

        publish_without_replacing(&first, &destination).unwrap();
        assert!(publish_without_replacing(&second, &destination).is_err());
        assert_eq!(fs::read(destination).unwrap(), b"first");
        assert_eq!(fs::read(second).unwrap(), b"second");
    }

    #[test]
    fn profile_store_returns_empty_only_for_missing_file() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("managed-accounts.json");
        let store = test_store(&path);

        assert_eq!(store.load_profiles().unwrap(), Vec::<AccountProfile>::new());
        fs::create_dir(&path).unwrap();
        assert_eq!(
            store.load_profiles(),
            Err("account_profile_store_invalid".to_string())
        );
    }

    #[test]
    fn read_only_missing_profile_does_not_create_parent_or_lock() {
        let root = tempfile::tempdir().unwrap();
        let parent = root.path().join("missing");
        let path = parent.join("managed-accounts.json");
        let store = JsonManagedAccountProfileStore::for_read(&path, Some(test_salt())).unwrap();

        assert_eq!(store.load_profiles().unwrap(), Vec::<AccountProfile>::new());
        assert!(!parent.exists());

        fs::create_dir(&parent).unwrap();
        assert_eq!(store.load_profiles().unwrap(), Vec::<AccountProfile>::new());
        assert!(!profile_store_lock_path(&path).exists());
        assert_eq!(
            store.upsert_profile(profile()),
            Err("account_profile_store_read_only".to_string())
        );
    }

    #[test]
    fn profile_leaf_reparse_point_is_rejected_for_load_and_upsert() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let path = root.path().join("managed-accounts.json");
        assert!(
            make_directory_link(outside.path(), &path),
            "无法建立 junction fixture"
        );
        let store = test_store(&path);

        assert_eq!(
            store.load_profiles(),
            Err("account_profile_store_invalid".to_string())
        );
        assert_eq!(
            store.upsert_profile(profile()),
            Err("account_profile_store_invalid".to_string())
        );
        assert!(!outside.path().join("managed-accounts.json").exists());
    }

    #[test]
    fn profile_parent_link_is_rejected_for_load_and_upsert() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let linked_parent = root.path().join("linked");
        assert!(
            make_directory_link(outside.path(), &linked_parent),
            "无法建立目录 symlink/junction fixture"
        );
        let path = linked_parent.join("managed-accounts.json");
        let store = test_store(&path);

        assert_eq!(
            store.load_profiles(),
            Err("account_profile_store_path_invalid".to_string())
        );
        assert_eq!(
            store.upsert_profile(profile()),
            Err("account_profile_store_path_invalid".to_string())
        );
        assert!(!outside.path().join("managed-accounts.json").exists());
    }

    #[test]
    fn corrupt_and_oversize_profiles_fail_closed_without_overwrite() {
        let root = tempfile::tempdir().unwrap();
        let cases = [
            ("corrupt", b"not-json".to_vec()),
            ("oversize", vec![b' '; PROFILE_STORE_MAX_BYTES as usize + 1]),
        ];

        for (name, expected) in cases {
            let path = root.path().join(format!("{name}.json"));
            fs::write(&path, &expected).unwrap();
            let store = test_store(&path);

            assert_eq!(
                store.load_profiles(),
                Err("account_profile_store_invalid".to_string())
            );
            assert_eq!(
                store.upsert_profile(profile()),
                Err("account_profile_store_invalid".to_string())
            );
            assert_eq!(fs::read(path).unwrap(), expected);
        }
    }

    #[test]
    fn legacy_array_stays_unverified_and_upgrades_only_with_current_profile() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("managed-accounts.json");
        fs::write(
            &path,
            r#"[
  {
    "profile_id": "legacy-profile",
    "display_name": "旧账号",
    "user_fingerprint": "legacy-fingerprint",
    "region": null,
    "data_location_id": "legacy-location",
    "last_verified_at": null,
    "verification_state": "verified"
  }
]"#,
        )
        .unwrap();
        let store = test_store(&path);

        let profiles = store.load_profiles().unwrap();
        assert_eq!(
            profiles[0].fingerprint_version,
            LEGACY_ACCOUNT_FINGERPRINT_VERSION
        );
        assert_eq!(
            profiles[0].verification_state,
            AccountVerificationState::FingerprintChanged
        );
        store.upsert_profile(profile()).unwrap();

        let value: serde_json::Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
        assert_eq!(value["format_version"], PROFILE_STORE_FORMAT_VERSION);
        assert_eq!(
            value["fingerprint_salt_id"],
            fingerprint_salt_id(&test_salt())
        );
        assert_eq!(
            value["profiles"][0]["fingerprint_version"],
            LEGACY_ACCOUNT_FINGERPRINT_VERSION
        );
        assert_eq!(
            value["profiles"][0]["verification_state"],
            "fingerprint_changed"
        );
        assert_eq!(
            value["profiles"][1]["fingerprint_version"],
            ACCOUNT_FINGERPRINT_VERSION
        );
    }

    #[test]
    fn unknown_profile_store_format_is_rejected() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("managed-accounts.json");
        fs::write(&path, r#"{"format_version": 4, "profiles": []}"#).unwrap();
        let store = test_store(&path);

        assert_eq!(
            store.load_profiles(),
            Err("account_profile_store_invalid".to_string())
        );
    }

    #[test]
    fn unknown_envelope_and_profile_fields_fail_closed_without_overwrite() {
        let root = tempfile::tempdir().unwrap();
        let cases = [
            (
                "unknown-envelope",
                r#"{"format_version":2,"profiles":[],"token":"forbidden"}"#,
            ),
            (
                "unknown-profile",
                r#"{
  "format_version": 2,
  "profiles": [{
    "profile_id": "profile-one",
    "display_name": "账号 one",
    "user_fingerprint": "fingerprint-one",
    "fingerprint_version": 2,
    "region": "cn",
    "data_location_id": "location-one",
    "last_verified_at": null,
    "verification_state": "verified",
    "cookie": "forbidden"
  }]
}"#,
            ),
        ];

        for (name, payload) in cases {
            let path = root.path().join(format!("{name}.json"));
            fs::write(&path, payload.as_bytes()).unwrap();
            let before = fs::read(&path).unwrap();
            let store = test_store(&path);

            assert_eq!(
                store.load_profiles(),
                Err("account_profile_store_invalid".to_string())
            );
            assert_eq!(
                store.upsert_profile(profile()),
                Err("account_profile_store_invalid".to_string())
            );
            assert_eq!(fs::read(path).unwrap(), before);
        }
    }

    #[test]
    fn profile_store_rejects_missing_or_different_salt_without_overwrite() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("managed-accounts.json");
        let first = test_store(&path);
        first.upsert_profile(profile()).unwrap();
        let before = fs::read(&path).unwrap();

        let missing = JsonManagedAccountProfileStore::for_read(&path, None).unwrap();
        assert_eq!(
            missing.load_profiles(),
            Err("account_fingerprint_salt_unavailable".to_string())
        );
        assert_eq!(
            missing.upsert_profile(profile()),
            Err("account_fingerprint_salt_unavailable".to_string())
        );

        let different = JsonManagedAccountProfileStore::with_fingerprint_salt(
            &path,
            vec![8_u8; PROFILE_SALT_BYTES],
        )
        .unwrap();
        assert_eq!(
            different.load_profiles(),
            Err("account_profile_store_salt_mismatch".to_string())
        );
        assert_eq!(
            different.upsert_profile(profile()),
            Err("account_profile_store_salt_mismatch".to_string())
        );
        assert_eq!(fs::read(path).unwrap(), before);
    }

    #[test]
    fn unbound_v2_profiles_require_explicit_salt_binding() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("managed-accounts.json");
        let payload = serde_json::to_vec_pretty(&serde_json::json!({
            "format_version": LEGACY_PROFILE_STORE_FORMAT_VERSION,
            "profiles": [profile()],
        }))
        .unwrap();
        fs::write(&path, &payload).unwrap();
        let store = test_store(&path);

        assert_eq!(
            store.load_profiles(),
            Err("account_profile_store_salt_binding_required".to_string())
        );
        assert_eq!(
            store.upsert_profile(profile_with_id("two")),
            Err("account_profile_store_salt_binding_required".to_string())
        );
        assert_eq!(fs::read(path).unwrap(), payload);
    }

    #[test]
    fn legacy_v2_envelope_upgrades_with_current_profile() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("managed-accounts.json");
        let mut legacy = profile_with_id("legacy");
        legacy.user_fingerprint = "legacy-fingerprint".to_string();
        legacy.fingerprint_version = LEGACY_ACCOUNT_FINGERPRINT_VERSION;
        let payload = serde_json::to_vec_pretty(&serde_json::json!({
            "format_version": LEGACY_PROFILE_STORE_FORMAT_VERSION,
            "profiles": [legacy],
        }))
        .unwrap();
        fs::write(&path, payload).unwrap();
        let store = test_store(&path);

        let loaded = store.load_profiles().unwrap();
        assert_eq!(
            loaded[0].verification_state,
            AccountVerificationState::FingerprintChanged
        );
        store.upsert_profile(profile()).unwrap();

        let value: serde_json::Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
        assert_eq!(value["format_version"], PROFILE_STORE_FORMAT_VERSION);
        assert_eq!(
            value["fingerprint_salt_id"],
            fingerprint_salt_id(&test_salt())
        );
        assert_eq!(value["profiles"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn invalid_profile_identity_sets_fail_closed_without_overwrite() {
        let root = tempfile::tempdir().unwrap();
        let base = profile();
        let mut duplicate_id = profile_with_id("duplicate-id");
        duplicate_id.profile_id = base.profile_id.clone();
        let mut duplicate_identity = profile_with_id("duplicate-identity");
        duplicate_identity.data_location_id = base.data_location_id.clone();
        duplicate_identity.user_fingerprint = base.user_fingerprint.clone();
        let mut unknown_version = profile_with_id("unknown-version");
        unknown_version.fingerprint_version = 99;
        let mut bad_fingerprint = profile_with_id("bad-fingerprint");
        bad_fingerprint.user_fingerprint = "v2-not-a-sha256".to_string();
        let cases = [
            vec![base.clone(), duplicate_id],
            vec![base.clone(), duplicate_identity],
            vec![unknown_version],
            vec![bad_fingerprint],
        ];

        for (index, profiles) in cases.into_iter().enumerate() {
            let path = root.path().join(format!("invalid-{index}.json"));
            let payload = serde_json::to_vec_pretty(&ProfileStoreEnvelope {
                format_version: PROFILE_STORE_FORMAT_VERSION,
                fingerprint_salt_id: fingerprint_salt_id(&test_salt()),
                profiles,
            })
            .unwrap();
            fs::write(&path, &payload).unwrap();
            let store = test_store(&path);

            assert_eq!(
                store.load_profiles(),
                Err("account_profile_store_invalid".to_string())
            );
            assert_eq!(
                store.upsert_profile(profile_with_id("new")),
                Err("account_profile_store_invalid".to_string())
            );
            assert_eq!(fs::read(path).unwrap(), payload);
        }
    }

    #[test]
    fn reading_missing_salt_does_not_create_parent() {
        let root = tempfile::tempdir().unwrap();
        let parent = root.path().join("missing");
        let path = parent.join("account-fingerprint.salt");

        assert_eq!(load_account_fingerprint_salt(&path).unwrap(), None);
        assert!(!parent.exists());
    }

    #[test]
    fn profile_lock_directory_is_rejected_without_profile_write() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("managed-accounts.json");
        let lock_path = profile_store_lock_path(&path);
        fs::create_dir(&lock_path).unwrap();
        let store = test_store(&path);

        assert_eq!(
            store.load_profiles(),
            Err("account_profile_store_path_invalid".to_string())
        );
        assert_eq!(
            store.upsert_profile(profile()),
            Err("account_profile_store_path_invalid".to_string())
        );
        assert!(!path.exists());
    }

    #[cfg(windows)]
    #[test]
    fn profile_lock_reparse_point_is_rejected_without_external_write() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let path = root.path().join("managed-accounts.json");
        let lock_path = profile_store_lock_path(&path);
        assert!(make_directory_link(outside.path(), &lock_path));
        let store = test_store(&path);

        assert_eq!(
            store.load_profiles(),
            Err("account_profile_store_path_invalid".to_string())
        );
        assert_eq!(
            store.upsert_profile(profile()),
            Err("account_profile_store_path_invalid".to_string())
        );
        assert!(!path.exists());
        assert!(fs::read_dir(outside.path()).unwrap().next().is_none());
    }

    #[test]
    fn child_process_upserts_profile() {
        let Ok(path) = std::env::var("TRAE_SYNC_PROFILE_CHILD_PATH") else {
            return;
        };
        let id = std::env::var("TRAE_SYNC_PROFILE_CHILD_ID").unwrap();
        let expected = profile_with_id(&id);
        let profiles = test_store(path).upsert_profile(expected.clone()).unwrap();
        assert!(profiles.contains(&expected));
    }

    #[test]
    fn concurrent_process_upserts_preserve_every_profile() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("managed-accounts.json");
        let mut children = ["first", "second"]
            .into_iter()
            .map(|id| {
                Command::new(std::env::current_exe().unwrap())
                    .env("TRAE_SYNC_PROFILE_CHILD_PATH", &path)
                    .env("TRAE_SYNC_PROFILE_CHILD_ID", id)
                    .args([
                        "--exact",
                        "account_switch::tests::child_process_upserts_profile",
                        "--nocapture",
                    ])
                    .spawn()
                    .unwrap()
            })
            .collect::<Vec<_>>();

        for child in &mut children {
            assert!(child.wait().unwrap().success());
        }

        let profiles = test_store(path).load_profiles().unwrap();
        assert_eq!(profiles.len(), 2);
        assert!(profiles.contains(&profile_with_id("first")));
        assert!(profiles.contains(&profile_with_id("second")));
    }

    #[test]
    fn child_process_observes_profile_store_busy_without_overwrite() {
        let Ok(path) = std::env::var("TRAE_SYNC_PROFILE_BUSY_CHILD_PATH") else {
            return;
        };
        let before = fs::read(&path).unwrap();
        let result = test_store(&path).upsert_profile(profile_with_id("blocked"));
        assert_eq!(result, Err("account_profile_store_busy".to_string()));
        assert_eq!(fs::read(path).unwrap(), before);
    }

    #[test]
    fn cross_process_lock_timeout_is_explicit_and_preserves_bytes() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("managed-accounts.json");
        let store = test_store(&path);
        store.upsert_profile(profile()).unwrap();
        let before = fs::read(&path).unwrap();
        let _directory_guard = protect_parent_directory(
            &path,
            true,
            "account_profile_store_path_invalid",
            "account_profile_store_unavailable",
        )
        .unwrap()
        .unwrap();
        let _lock = ProfileStoreLock::acquire_exclusive(&profile_store_lock_path(&path)).unwrap();

        let status = Command::new(std::env::current_exe().unwrap())
            .env("TRAE_SYNC_PROFILE_BUSY_CHILD_PATH", &path)
            .args([
                "--exact",
                "account_switch::tests::child_process_observes_profile_store_busy_without_overwrite",
                "--nocapture",
            ])
            .status()
            .unwrap();

        assert!(status.success());
        assert_eq!(fs::read(path).unwrap(), before);
    }

    #[cfg(windows)]
    #[test]
    fn protected_directory_chain_blocks_parent_rename_until_release() {
        let root = tempfile::tempdir().unwrap();
        let parent = root.path().join("store");
        fs::create_dir(&parent).unwrap();
        let path = parent.join("managed-accounts.json");
        let moved = root.path().join("moved-store");

        let guard = protect_parent_directory(
            &path,
            false,
            "account_profile_store_path_invalid",
            "account_profile_store_unavailable",
        )
        .unwrap()
        .unwrap();
        assert!(fs::rename(&parent, &moved).is_err());

        drop(guard);
        fs::rename(parent, moved).unwrap();
    }

    #[test]
    fn old_fixed_temporary_hard_link_is_not_used() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("managed-accounts.json");
        let old_temporary = path.with_extension("json.tmp");
        let outside = root.path().join("outside.json");
        fs::write(&outside, b"outside").unwrap();
        // 旧实现写固定临时名会截断这个同一文件实体；新实现不会引用该名称。
        fs::hard_link(&outside, &old_temporary).unwrap();
        let store = test_store(&path);

        store.upsert_profile(profile()).unwrap();

        assert_eq!(fs::read(outside).unwrap(), b"outside");
        assert_eq!(fs::read(old_temporary).unwrap(), b"outside");
        assert_eq!(store.load_profiles().unwrap(), vec![profile()]);
    }
}
