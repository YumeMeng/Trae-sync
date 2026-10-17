//! P17-T4b：WorkBuddy 数据目录备份治理（zstd 压缩 + 清单 JSON + 恢复 + 保留裁剪）。
//!
//! 与 Trae 侧（`backup_retention` / `master_handover`）同纪律的差异适配：
//! - WorkBuddy 数据是**未加密文件**（IndexedDB/LocalStorage leveldb + JSON），
//!   zstd 压缩真有效（对比 SQLCipher 密文约 1:1 的诚实修正，见 P16-T1）；
//! - 备份对象是**目录树**而非三件套：沿用 P16-T1「每成员独立标准 zstd 帧、
//!   无自定义容器」决策——按文件逐个压缩为 `{相对路径}.zst`，目录内相对
//!   结构镜像保存，`manifest.json` 记录全部成员（相对路径/原字节/sha256）；
//! - leveldb 文件在客户端运行时可能被锁/写入中：备份编排的前置是
//!   `workbuddy_instance::close_workbuddy()`（编排层职责，本模块只做纯
//!   文件操作）；任一成员压缩失败 → 删除暂存目录整体失败（fail-closed，
//!   不产出不完整备份，零副作用退出）。
//!
//! 备份范围（黑名单制，登录后新增目录默认纳入）：
//! - 目录名精确命中黑名单（Electron/Chromium 可再生缓存惯例名）→ 排除；
//! - `lockfile`（运行锁文件，运行时被独占锁定）→ 排除；
//! - 其余全部纳入：Local Storage、IndexedDB、Session Storage、Network
//!   （Trust Tokens/未来 Cookies）、Partitions 各 webview 子区数据、
//!   playbook-covers（用户内容资源）、data/、memory/、根部 JSON。
//!
//! 布局：`{target_dir}/workbuddy-data-{stamp}/manifest.json` +
//! `{相对路径}.zst`。恢复读 manifest 逐成员解压并 sha256 校验；恢复路径
//! 对压缩（`.zst`）与未压缩（原始文件）双形态成员兼容。

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::master_handover::{compress_file_create_new, decompress_backup_member};

/// 备份目录前缀（`workbuddy-data-{stamp}`）。
const BACKUP_DIR_PREFIX: &str = "workbuddy-data-";
/// 清单文件名（备份目录根部）。
const MANIFEST_NAME: &str = "manifest.json";
/// manifest 读取上限（成员清单按 200 字节/条估算，数千成员仍在限内）。
const MAX_MANIFEST_BYTES: u64 = 16 * 1024 * 1024;

/// 可再生缓存目录名黑名单（任意层级精确匹配）。实测来源：P17-T4b 隔离
/// 副本盘点——顶层 `cache`/`CodeCache`/`Crashpad` + `session` 下 Chromium
/// 缓存族（Cache/Code Cache/GPUCache/Dawn*/VideoDecodeStats/Shared
/// Dictionary/shared_proto_db/Service Worker）。排除理由：可再生缓存不属
/// 数据安全网，且图片缓存等压缩率低、体积占比高。
const EXCLUDED_DIR_NAMES: &[&str] = &[
    "cache",
    "Cache",
    "CodeCache",
    "Code Cache",
    "Crashpad",
    "GPUCache",
    "DawnGraphiteCache",
    "DawnWebGPUCache",
    "VideoDecodeStats",
    "Shared Dictionary",
    "shared_proto_db",
    "Service Worker",
];

/// 排除的文件名：`lockfile`（运行锁，复制无意义且运行时被独占锁定）。
const EXCLUDED_FILE_NAMES: &[&str] = &["lockfile"];

/// 备份范围判定（相对路径，`/` 分隔；目录与文件通用）：任一段命中目录
/// 黑名单 → 排除（目录调用即剪枝整棵子树）；末段命中文件黑名单 → 排除。
fn is_backup_member(relative: &str) -> bool {
    let segments: Vec<&str> = relative.split('/').collect();
    if segments
        .iter()
        .any(|segment| EXCLUDED_DIR_NAMES.contains(segment))
    {
        return false;
    }
    if let Some((file, _)) = segments.split_last() {
        if EXCLUDED_FILE_NAMES.contains(file) {
            return false;
        }
    }
    true
}

/// 清单成员：备份目录内相对路径 + 原始字节 + sha256（hex）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkBuddyBackupMember {
    /// 相对路径（`/` 分隔，无 `.zst` 后缀）。
    pub path: String,
    /// 原始字节数。
    pub bytes: u64,
    /// 原始内容 sha256（hex，与 snapshot_store::sha256_file 同口径）。
    pub sha256: String,
}

/// 备份清单（备份目录根部 manifest.json）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkBuddyBackupManifest {
    pub format_version: u32,
    pub product: String,
    pub stamp_unix_seconds: u64,
    pub file_count: usize,
    /// 纳入备份的源文件原始字节合计。
    pub source_bytes: u64,
    /// 备份成员压缩字节合计（不含 manifest 自身）。
    pub compressed_bytes: u64,
    pub files: Vec<WorkBuddyBackupMember>,
}

/// 备份回执。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkBuddyBackupOutcome {
    /// 发布后的备份目录（`workbuddy-data-{stamp}`）。
    pub backup_dir: PathBuf,
    /// 纳入备份的文件数。
    pub file_count: usize,
    /// 源文件原始字节合计。
    pub source_bytes: u64,
    /// 压缩成员字节合计（含 manifest 自身）。
    pub compressed_bytes: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkBuddyBackupError {
    /// 源目录不存在（数据根未安装/路径错）。
    SourceUnavailable,
    /// 同 stamp 目标已存在（永不覆盖既有备份）。
    TargetExists,
    /// 文件枚举/压缩/发布失败（暂存已清理，零副作用）。
    Io,
}

impl WorkBuddyBackupError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::SourceUnavailable => "workbuddy_backup_source_unavailable",
            Self::TargetExists => "workbuddy_backup_target_exists",
            Self::Io => "workbuddy_backup_io",
        }
    }
}

/// 秒级 UNIX 时间戳（与 master_handover 同口径）。
fn unix_seconds_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

/// 递归收集备份计划（相对路径 `/` 分隔，排序保证 manifest 稳定）。
/// 符号链接等非常规对象拒绝（fail-closed，与 backup_retention 同纪律）。
fn collect_backup_plan(root: &Path) -> Result<Vec<String>, WorkBuddyBackupError> {
    fn walk(dir: &Path, relative: &str, plan: &mut Vec<String>) -> Result<(), WorkBuddyBackupError> {
        let entries = fs::read_dir(dir).map_err(|_| WorkBuddyBackupError::Io)?;
        for entry in entries {
            let entry = entry.map_err(|_| WorkBuddyBackupError::Io)?;
            // DirEntry::metadata 不跟随符号链接（防逃逸判定与
            // backup_retention::collect_dir_files 同语义）。
            let metadata = entry.metadata().map_err(|_| WorkBuddyBackupError::Io)?;
            let name = entry.file_name();
            let name = name.to_string_lossy();
            let child_relative = if relative.is_empty() {
                name.to_string()
            } else {
                format!("{relative}/{name}")
            };
            if metadata.is_dir() {
                if EXCLUDED_DIR_NAMES.contains(&name.as_ref()) {
                    continue;
                }
                walk(&entry.path(), &child_relative, plan)?;
            } else if metadata.is_file() {
                // 文件级判定走统一规则（is_backup_member：黑名单文件名排除）。
                if !is_backup_member(&child_relative) {
                    continue;
                }
                plan.push(child_relative);
            } else {
                // 符号链接等非常规对象：不猜测内容，fail-closed。
                return Err(WorkBuddyBackupError::Io);
            }
        }
        Ok(())
    }
    let mut plan = Vec::new();
    walk(root, "", &mut plan)?;
    plan.sort();
    Ok(plan)
}

/// 备份 WorkBuddy 数据目录到 target_dir（新建 `workbuddy-data-{stamp}`）。
///
/// 流程：收集计划 → 暂存目录逐文件压缩（create_new）→ 写 manifest →
/// 同卷改名发布。任一步失败清理暂存返回 Err（零副作用）；最终名已存在
/// → 拒绝（永不覆盖既有备份）。前置编排（关闭 WorkBuddy）由调用方负责。
pub fn backup_workbuddy_data_to(
    source_root: &Path,
    target_dir: &Path,
) -> Result<WorkBuddyBackupOutcome, WorkBuddyBackupError> {
    backup_workbuddy_data_stamped(source_root, target_dir, unix_seconds_now())
}

/// 显式时间戳版本（编排层可统一发号；测试确定性构造冲突现场）。
/// 语义与 [`backup_workbuddy_data_to`] 完全一致。
pub fn backup_workbuddy_data_stamped(
    source_root: &Path,
    target_dir: &Path,
    stamp: u64,
) -> Result<WorkBuddyBackupOutcome, WorkBuddyBackupError> {
    if !source_root.is_dir() {
        return Err(WorkBuddyBackupError::SourceUnavailable);
    }
    std::fs::create_dir_all(target_dir).map_err(|_| WorkBuddyBackupError::Io)?;
    let final_dir = target_dir.join(format!("{BACKUP_DIR_PREFIX}{stamp}"));
    if final_dir.exists() {
        return Err(WorkBuddyBackupError::TargetExists);
    }
    let plan = collect_backup_plan(source_root)?;

    // 暂存压缩：任一成员失败 → 清暂存、零副作用退出（不产出不完整备份）。
    let staging = target_dir.join(format!(".wb-baking-{stamp}"));
    let cleanup = |staging: &Path| {
        let _ = fs::remove_dir_all(staging);
    };
    let mut members = Vec::with_capacity(plan.len());
    let mut source_bytes = 0_u64;
    let mut compressed_bytes = 0_u64;
    for relative in &plan {
        // Path::join 接受 `/` 分隔的相对串（Windows API 亦兼容 `/`）。
        let source = source_root.join(relative);
        let target = staging.join(format!("{relative}.zst"));
        let Some(parent) = target.parent() else {
            cleanup(&staging);
            return Err(WorkBuddyBackupError::Io);
        };
        if fs::create_dir_all(parent).is_err()
            || compress_file_create_new(&source, &target).is_err()
        {
            cleanup(&staging);
            return Err(WorkBuddyBackupError::Io);
        }
        let bytes = fs::metadata(&source).map(|meta| meta.len()).unwrap_or(0);
        let compressed = fs::metadata(&target).map(|meta| meta.len()).unwrap_or(0);
        let sha256 = crate::snapshot_store::sha256_file(&source).ok_or_else(|| {
            cleanup(&staging);
            WorkBuddyBackupError::Io
        })?;
        source_bytes += bytes;
        compressed_bytes += compressed;
        members.push(WorkBuddyBackupMember {
            path: relative.clone(),
            bytes,
            sha256,
        });
    }

    let manifest = WorkBuddyBackupManifest {
        format_version: 1,
        product: crate::product_descriptor::WORKBUDDY_DESCRIPTOR.product_id.to_string(),
        stamp_unix_seconds: stamp,
        file_count: members.len(),
        source_bytes,
        compressed_bytes,
        files: members,
    };
    let manifest_path = staging.join(MANIFEST_NAME);
    let manifest_json = serde_json::to_string_pretty(&manifest)
        .map_err(|_| {
            cleanup(&staging);
            WorkBuddyBackupError::Io
        })
        .and_then(|json| fs::write(&manifest_path, json).map_err(|_| WorkBuddyBackupError::Io));
    manifest_json?;
    compressed_bytes += fs::metadata(&manifest_path)
        .map(|meta| meta.len())
        .map_err(|_| {
            cleanup(&staging);
            WorkBuddyBackupError::Io
        })?;

    // 发布：暂存 → 最终名（同卷改名）。
    if fs::rename(&staging, &final_dir).is_err() {
        cleanup(&staging);
        return Err(WorkBuddyBackupError::Io);
    }
    Ok(WorkBuddyBackupOutcome {
        backup_dir: final_dir,
        file_count: manifest.file_count,
        source_bytes: manifest.source_bytes,
        compressed_bytes,
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkBuddyRestoreError {
    /// manifest.json 缺失。
    ManifestMissing,
    /// manifest 损坏/超限/格式版本不符。
    ManifestInvalid,
    /// 清单成员路径不安全（绝对路径/`..`/反斜杠——防清单被篡改写入任意位置）。
    UnsafePath,
    /// 清单成员在备份目录无对应文件。
    MemberMissing(String),
    /// 成员 sha256 校验不符（半写目标已清理）。
    VerifyFailed(String),
    /// 其他 IO 失败。
    Io,
}

impl WorkBuddyRestoreError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::ManifestMissing => "workbuddy_restore_manifest_missing",
            Self::ManifestInvalid => "workbuddy_restore_manifest_invalid",
            Self::UnsafePath => "workbuddy_restore_unsafe_path",
            Self::MemberMissing(_) => "workbuddy_restore_member_missing",
            Self::VerifyFailed(_) => "workbuddy_restore_verify_failed",
            Self::Io => "workbuddy_restore_io",
        }
    }
}

/// 恢复回执。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkBuddyRestoreOutcome {
    pub restored_files: usize,
    /// 恢复内容字节合计（manifest 声明口径）。
    pub restored_bytes: u64,
}

/// 清单成员路径安全校验：相对路径、`/` 分隔、全部段为普通段。
/// 用 `Path::components` 统一拒绝绝对路径/盘符前缀/根段/`.`/`..`
/// （Windows 上 `C:/x` 解析出 Prefix 组件——防清单被篡改写入任意位置）。
fn member_path_is_safe(relative: &str) -> bool {
    use std::path::Component;
    !relative.is_empty()
        && !relative.contains('\\')
        && Path::new(relative)
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
}

/// 从备份目录恢复数据树到 target_root（逐成员解压 + sha256 校验）。
///
/// 双格式兼容：成员优先取 `{path}.zst`（压缩形态），缺席时回退原始文件
/// `{path}`（手工维护/未来未压缩成员）。目标既有文件被覆盖（恢复语义 =
/// 目标内容替换为备份内容）；重复恢复结果一致（幂等）。任一成员校验
/// 失败 → 删除该半写目标并中止（不产出部分校验通过的状态混存）。
pub fn restore_workbuddy_data_from(
    backup_dir: &Path,
    target_root: &Path,
) -> Result<WorkBuddyRestoreOutcome, WorkBuddyRestoreError> {
    let manifest = read_manifest(backup_dir)?;
    let mut restored_files = 0_usize;
    let mut restored_bytes = 0_u64;
    for member in &manifest.files {
        if !member_path_is_safe(&member.path) {
            return Err(WorkBuddyRestoreError::UnsafePath);
        }
        let compressed = backup_dir.join(format!("{}.zst", member.path));
        let raw = backup_dir.join(&member.path);
        let target = target_root.join(&member.path);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).map_err(|_| WorkBuddyRestoreError::Io)?;
        }
        if compressed.is_file() {
            // 解压失败（坏帧/中断）：清半写目标，fail-closed 中止。
            if decompress_backup_member(&compressed, &target).is_err() {
                let _ = fs::remove_file(&target);
                return Err(WorkBuddyRestoreError::Io);
            }
        } else if raw.is_file() {
            fs::copy(&raw, &target).map_err(|_| WorkBuddyRestoreError::Io)?;
        } else {
            return Err(WorkBuddyRestoreError::MemberMissing(member.path.clone()));
        }
        // sha256 校验：不符删半写目标（不留在目标位置的静默坏内容）。
        let matches = crate::snapshot_store::sha256_file(&target)
            .map(|hash| hash == member.sha256)
            .unwrap_or(false);
        if !matches {
            let _ = fs::remove_file(&target);
            return Err(WorkBuddyRestoreError::VerifyFailed(member.path.clone()));
        }
        restored_files += 1;
        restored_bytes += member.bytes;
    }
    Ok(WorkBuddyRestoreOutcome {
        restored_files,
        restored_bytes,
    })
}

/// 读取并校验 manifest（损坏/超限/版本不符一律拒绝，不静默重建）。
fn read_manifest(backup_dir: &Path) -> Result<WorkBuddyBackupManifest, WorkBuddyRestoreError> {
    let path = backup_dir.join(MANIFEST_NAME);
    let metadata = fs::metadata(&path).map_err(|_| WorkBuddyRestoreError::ManifestMissing)?;
    if !metadata.is_file() || metadata.len() > MAX_MANIFEST_BYTES {
        return Err(WorkBuddyRestoreError::ManifestInvalid);
    }
    let content =
        fs::read_to_string(&path).map_err(|_| WorkBuddyRestoreError::ManifestInvalid)?;
    let manifest: WorkBuddyBackupManifest =
        serde_json::from_str(&content).map_err(|_| WorkBuddyRestoreError::ManifestInvalid)?;
    if manifest.format_version != 1 || manifest.product != "workbuddy" {
        return Err(WorkBuddyRestoreError::ManifestInvalid);
    }
    Ok(manifest)
}

/// 备份链条目（设置页/报告数据源，只读）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkBuddyBackupEntry {
    pub stamp_unix_seconds: u64,
    /// manifest 声明的文件数（manifest 不可读时为 None）。
    pub file_count: Option<usize>,
    /// manifest 声明的源字节量。
    pub source_bytes: Option<u64>,
    /// 备份目录实际磁盘字节（含 manifest，遍历统计）。
    pub compressed_bytes: u64,
}

/// 枚举备份链（stamp 升序）。命名不符的散目录不属备份链，永不触碰
/// （铁律：不自动删证据）。
pub fn list_workbuddy_backups(target_dir: &Path) -> Vec<WorkBuddyBackupEntry> {
    list_workbuddy_backup_stamps(target_dir)
        .into_iter()
        .map(|stamp| {
            let dir = target_dir.join(format!("{BACKUP_DIR_PREFIX}{stamp}"));
            let manifest = read_manifest(&dir).ok();
            WorkBuddyBackupEntry {
                stamp_unix_seconds: stamp,
                file_count: manifest.as_ref().map(|manifest| manifest.file_count),
                source_bytes: manifest.as_ref().map(|manifest| manifest.source_bytes),
                compressed_bytes: dir_size(&dir),
            }
        })
        .collect()
}

/// 枚举 `workbuddy-data-{纯数字}` 目录的 stamp（升序）。
fn list_workbuddy_backup_stamps(target_dir: &Path) -> Vec<u64> {
    let Ok(entries) = fs::read_dir(target_dir) else {
        return Vec::new();
    };
    let mut stamps = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some(stamp) = name.strip_prefix(BACKUP_DIR_PREFIX) else {
            continue;
        };
        if stamp.is_empty() || !stamp.bytes().all(|byte| byte.is_ascii_digit()) {
            continue;
        }
        let Ok(stamp) = stamp.parse::<u64>() else {
            continue;
        };
        if entry.path().is_dir() {
            stamps.push(stamp);
        }
    }
    stamps.sort_unstable();
    stamps
}

/// 目录磁盘字节合计（递归；不可读项按 0 计——统计口径，fail-soft）。
fn dir_size(dir: &Path) -> u64 {
    let mut total = 0;
    let Ok(entries) = fs::read_dir(dir) else {
        return 0;
    };
    for entry in entries.flatten() {
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        if metadata.is_dir() {
            total += dir_size(&entry.path());
        } else if metadata.is_file() {
            total += metadata.len();
        }
    }
    total
}

/// 保留裁剪：保留最新 `keep` 份，更旧的整目录删除。keep=0 时不删任何
/// 东西（禁用语义，与 Trae 侧 prune_master_backups 同口径）；删除失败
/// 静默跳过（该份下次再试）；命名不符的散目录永不触碰。返回删除份数。
pub fn prune_workbuddy_backups(target_dir: &Path, keep: u32) -> usize {
    if keep == 0 {
        return 0;
    }
    let mut stamps = list_workbuddy_backup_stamps(target_dir);
    stamps.sort_unstable();
    let mut removed = 0;
    for stamp in stamps.iter().rev().skip(keep as usize) {
        let dir = target_dir.join(format!("{BACKUP_DIR_PREFIX}{stamp}"));
        if fs::remove_dir_all(&dir).is_ok() {
            removed += 1;
        }
    }
    removed
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    /// 构造模拟数据根：登录态关键目录 + 黑名单缓存目录 + 根部杂项。
    fn make_source(dir: &Path) {
        let local_storage = dir.join("session").join("Local Storage").join("leveldb");
        fs::create_dir_all(&local_storage).unwrap();
        fs::write(local_storage.join("000003.log"), b"log-bytes").unwrap();
        fs::write(local_storage.join("000005.ldb"), vec![7_u8; 4096]).unwrap();
        let indexed_db = dir
            .join("session")
            .join("IndexedDB")
            .join("file__0.indexeddb.leveldb");
        fs::create_dir_all(&indexed_db).unwrap();
        fs::write(indexed_db.join("000001.log"), b"idb-bytes").unwrap();
        let network = dir.join("session").join("Network");
        fs::create_dir_all(&network).unwrap();
        fs::write(network.join("Trust Tokens"), b"trust-tokens").unwrap();
        // 黑名单目录（顶层 + 嵌套）与运行锁文件：不应进备份。
        fs::create_dir_all(dir.join("CodeCache")).unwrap();
        fs::write(dir.join("CodeCache").join("blob"), b"cache").unwrap();
        let gpu = dir.join("session").join("GPUCache");
        fs::create_dir_all(&gpu).unwrap();
        fs::write(gpu.join("data_0"), b"gpu").unwrap();
        fs::write(dir.join("lockfile"), b"").unwrap();
        // 根部 JSON 与用户内容目录：应进备份。
        fs::write(dir.join("app-config.json"), b"{\"version\":1}").unwrap();
        let memory = dir.join("memory");
        fs::create_dir_all(&memory).unwrap();
        fs::write(memory.join("polling-lease-1.json"), b"lease").unwrap();
        let covers = dir.join("playbook-covers");
        fs::create_dir_all(&covers).unwrap();
        fs::write(covers.join("a.png"), vec![9_u8; 2048]).unwrap();
    }

    /// 备份范围规则：黑名单目录/锁文件排除，登录态目录与根部 JSON 纳入。
    #[test]
    fn backup_scope_excludes_cache_dirs_and_lockfile() {
        assert!(!is_backup_member("CodeCache/blob"));
        assert!(!is_backup_member("session/GPUCache/data_0"));
        assert!(!is_backup_member("session/Code Cache/js/x.js"));
        assert!(!is_backup_member("session/Partitions/p1/Cache/f"));
        assert!(!is_backup_member("session/Service Worker/CacheStorage/x"));
        assert!(!is_backup_member("lockfile"));
        // 登录态与用户内容纳入。
        assert!(is_backup_member("session/Local Storage/leveldb/000003.log"));
        assert!(is_backup_member("session/IndexedDB/file__0.indexeddb.leveldb/000001.log"));
        assert!(is_backup_member("session/Network/Trust Tokens"));
        assert!(is_backup_member("session/Partitions/p1/Local Storage/leveldb/000003.log"));
        assert!(is_backup_member("playbook-covers/a.png"));
        assert!(is_backup_member("app-config.json"));
        assert!(is_backup_member("memory/polling-lease-1.json"));
    }

    /// 压缩写出：manifest 字段/成员齐全，黑名单对象未进备份，成员为
    /// zstd 压缩形态（magic 帧头 0x28 B5 2F FD）。
    #[test]
    fn backup_writes_zstd_members_and_manifest() {
        let root = tempdir().unwrap();
        let source = root.path().join("app");
        make_source(&source);
        let target = root.path().join("backups");

        let outcome = backup_workbuddy_data_to(&source, &target).unwrap();
        assert_eq!(outcome.file_count, 7);
        // 源字节：9(log-bytes)+4096+9(idb-bytes)+12+13+5(lease)+2048。
        assert_eq!(outcome.source_bytes, 9 + 4096 + 9 + 12 + 13 + 5 + 2048);
        // 备份目录内：7 成员 + manifest；黑名单对象不存在。
        let backup = &outcome.backup_dir;
        assert!(backup.join("manifest.json").is_file());
        assert!(backup.join("app-config.json.zst").is_file());
        assert!(backup
            .join("session/Local Storage/leveldb/000003.log.zst")
            .is_file());
        assert!(backup
            .join("session/IndexedDB/file__0.indexeddb.leveldb/000001.log.zst")
            .is_file());
        assert!(backup.join("session/Network/Trust Tokens.zst").is_file());
        assert!(backup.join("playbook-covers/a.png.zst").is_file());
        assert!(backup.join("memory/polling-lease-1.json.zst").is_file());
        assert!(!backup.join("CodeCache").exists());
        assert!(!backup.join("session/GPUCache").exists());
        assert!(!backup.join("lockfile.zst").exists());
        // zstd 帧头（标准帧 magic）。
        let head = fs::read(backup.join("app-config.json.zst")).unwrap();
        assert_eq!(&head[..4], &[0x28, 0xB5, 0x2F, 0xFD]);
        // manifest 统计与回执一致；压缩产物小于源（可压缩样本）。
        let manifest = read_manifest(backup).unwrap();
        assert_eq!(manifest.file_count, 7);
        assert_eq!(manifest.source_bytes, outcome.source_bytes);
        assert_eq!(
            outcome.compressed_bytes,
            manifest.compressed_bytes + fs::metadata(backup.join("manifest.json")).unwrap().len()
        );
        assert!(outcome.compressed_bytes < outcome.source_bytes);
    }

    /// 同 stamp 目标已存在 → 拒绝且不覆盖（永不覆盖既有备份纪律）。
    /// 用显式时间戳版本确定性构造冲突现场（不依赖时钟推进）。
    #[test]
    fn backup_refuses_existing_target() {
        let root = tempdir().unwrap();
        let source = root.path().join("app");
        make_source(&source);
        let target = root.path().join("backups");
        let first = backup_workbuddy_data_stamped(&source, &target, 1000).unwrap();
        assert_eq!(
            first.backup_dir,
            target.join("workbuddy-data-1000"),
            "备份目录按 stamp 命名"
        );
        // 同 stamp 重跑：TargetExists，占位内容未被破坏。
        assert_eq!(
            backup_workbuddy_data_stamped(&source, &target, 1000).unwrap_err(),
            WorkBuddyBackupError::TargetExists
        );
        assert!(first.backup_dir.join("manifest.json").is_file());
        // 不同 stamp 正常出第二份（链内并存）。
        assert!(backup_workbuddy_data_stamped(&source, &target, 2000).is_ok());
        assert_eq!(list_workbuddy_backup_stamps(&target), vec![1000, 2000]);
    }

    /// 恢复往返：恢复内容与源逐文件字节一致；重复恢复幂等。
    #[test]
    fn restore_roundtrip_byte_identical_and_idempotent() {
        let root = tempdir().unwrap();
        let source = root.path().join("app");
        make_source(&source);
        let target = root.path().join("backups");
        backup_workbuddy_data_to(&source, &target).unwrap();

        let restored = root.path().join("restored");
        let backup_dir = target.join(format!(
            "workbuddy-data-{}",
            list_workbuddy_backup_stamps(&target)[0]
        ));
        let outcome = restore_workbuddy_data_from(&backup_dir, &restored).unwrap();
        assert_eq!(outcome.restored_files, 7);
        // 逐文件字节一致（源计划内文件）。
        for relative in collect_backup_plan(&source).unwrap() {
            assert_eq!(
                fs::read(source.join(&relative)).unwrap(),
                fs::read(restored.join(&relative)).unwrap(),
                "member {relative} 字节不一致"
            );
        }
        // 黑名单对象不在恢复结果（备份范围即恢复范围）。
        assert!(!restored.join("CodeCache").exists());
        // 幂等：重复恢复结果一致。
        let second = restore_workbuddy_data_from(&backup_dir, &restored).unwrap();
        assert_eq!(second, outcome);
    }

    /// 双格式兼容：未压缩原始成员（无 .zst）也能按 manifest 恢复。
    #[test]
    fn restore_accepts_uncompressed_member() {
        let root = tempdir().unwrap();
        let backup = root.path().join("workbuddy-data-1000");
        fs::create_dir_all(&backup).unwrap();
        fs::write(backup.join("app-config.json"), b"{\"raw\":true}").unwrap();
        fs::write(
            backup.join(MANIFEST_NAME),
            serde_json::json!({
                "format_version": 1,
                "product": "workbuddy",
                "stamp_unix_seconds": 1000,
                "file_count": 1,
                "source_bytes": 13,
                "compressed_bytes": 13,
                "files": [{
                    "path": "app-config.json",
                    "bytes": 13,
                    "sha256": crate::snapshot_store::sha256_file(&*{
                        let tmp = root.path().join("raw.json");
                        fs::write(&tmp, b"{\"raw\":true}").unwrap();
                        tmp
                    }).unwrap()
                }]
            })
            .to_string(),
        )
        .unwrap();
        let restored = root.path().join("restored");
        let outcome = restore_workbuddy_data_from(&backup, &restored).unwrap();
        assert_eq!(outcome.restored_files, 1);
        assert_eq!(
            fs::read(restored.join("app-config.json")).unwrap(),
            b"{\"raw\":true}"
        );
    }

    /// 校验失败：成员内容被篡改 → VerifyFailed 且半写目标被清理。
    #[test]
    fn restore_fails_on_corrupt_member() {
        let root = tempdir().unwrap();
        let source = root.path().join("app");
        make_source(&source);
        let target = root.path().join("backups");
        backup_workbuddy_data_to(&source, &target).unwrap();
        let backup_dir = target.join(format!(
            "workbuddy-data-{}",
            list_workbuddy_backup_stamps(&target)[0]
        ));
        // 篡改成员为合法 zstd 帧但内容不同（坏帧走 Io 分支，另见
        // restore_corrupt_frame_cleans_partial_target）。
        let tampered = zstd::stream::encode_all(std::io::Cursor::new(b"tampered"), 3).unwrap();
        fs::write(backup_dir.join("app-config.json.zst"), tampered).unwrap();
        let restored = root.path().join("restored");
        let error = restore_workbuddy_data_from(&backup_dir, &restored).unwrap_err();
        match error {
            WorkBuddyRestoreError::VerifyFailed(path) => assert_eq!(path, "app-config.json"),
            other => panic!("期望 VerifyFailed，实际 {other:?}"),
        }
        // 半写目标已清理。
        assert!(!restored.join("app-config.json").exists());
    }

    /// 坏帧成员（非法 zstd 数据）→ Io 分支且半写目标被清理（fail-closed）。
    #[test]
    fn restore_corrupt_frame_cleans_partial_target() {
        let root = tempdir().unwrap();
        let source = root.path().join("app");
        make_source(&source);
        let target = root.path().join("backups");
        backup_workbuddy_data_to(&source, &target).unwrap();
        let backup_dir = target.join(format!(
            "workbuddy-data-{}",
            list_workbuddy_backup_stamps(&target)[0]
        ));
        fs::write(backup_dir.join("app-config.json.zst"), b"corrupt").unwrap();
        let restored = root.path().join("restored");
        assert_eq!(
            restore_workbuddy_data_from(&backup_dir, &restored).unwrap_err(),
            WorkBuddyRestoreError::Io
        );
        // 解压失败的半写目标已清理。
        assert!(!restored.join("app-config.json").exists());
    }

    /// 恢复错误：manifest 缺失；成员缺失；不安全路径拒绝。
    #[test]
    fn restore_error_branches() {
        let root = tempdir().unwrap();
        // manifest 缺失。
        assert_eq!(
            restore_workbuddy_data_from(&root.path(), &root.path().join("out"))
                .unwrap_err(),
            WorkBuddyRestoreError::ManifestMissing
        );
        // manifest 损坏。
        let backup = root.path().join("workbuddy-data-1");
        fs::create_dir_all(&backup).unwrap();
        fs::write(backup.join(MANIFEST_NAME), "not json").unwrap();
        assert_eq!(
            restore_workbuddy_data_from(&backup, &root.path().join("out")).unwrap_err(),
            WorkBuddyRestoreError::ManifestInvalid
        );
        // 成员缺失（manifest 声明但无对应文件）。
        fs::write(
            backup.join(MANIFEST_NAME),
            serde_json::json!({
                "format_version": 1,
                "product": "workbuddy",
                "stamp_unix_seconds": 1,
                "file_count": 1,
                "source_bytes": 1,
                "compressed_bytes": 1,
                "files": [{"path": "a.json", "bytes": 1, "sha256": "00"}]
            })
            .to_string(),
        )
        .unwrap();
        assert_eq!(
            restore_workbuddy_data_from(&backup, &root.path().join("out")).unwrap_err(),
            WorkBuddyRestoreError::MemberMissing("a.json".to_string())
        );
        // 不安全路径（.. 段）。
        fs::write(
            backup.join(MANIFEST_NAME),
            serde_json::json!({
                "format_version": 1,
                "product": "workbuddy",
                "stamp_unix_seconds": 1,
                "file_count": 1,
                "source_bytes": 1,
                "compressed_bytes": 1,
                "files": [{"path": "../escape.json", "bytes": 1, "sha256": "00"}]
            })
            .to_string(),
        )
        .unwrap();
        assert_eq!(
            restore_workbuddy_data_from(&backup, &root.path().join("out")).unwrap_err(),
            WorkBuddyRestoreError::UnsafePath
        );
        // 绝对路径与反斜杠同样拒绝（纯校验函数）。
        assert!(!member_path_is_safe("C:/x"));
        assert!(!member_path_is_safe("a\\b"));
    }

    /// 链枚举与裁剪：升序枚举、保留最新 keep 份、旧份整目录删；散目录
    /// 与 keep=0 不触碰。
    #[test]
    fn prune_keeps_newest_and_ignores_non_chain_dirs() {
        let target = tempdir().unwrap();
        for stamp in [800_u64, 900, 1000] {
            let dir = target.path().join(format!("workbuddy-data-{stamp}"));
            fs::create_dir_all(dir.join("session")).unwrap();
            fs::write(dir.join("manifest.json"), b"{}").unwrap();
            fs::write(dir.join("session").join("a.json"), b"{}").unwrap();
        }
        // 非链命名：带后缀 / 非数字 / 散文件 / 暂存目录——永不触碰。
        fs::create_dir_all(target.path().join("workbuddy-data-700-extra")).unwrap();
        fs::create_dir_all(target.path().join("workbuddy-data-abc")).unwrap();
        fs::create_dir_all(target.path().join(".wb-baking-600")).unwrap();
        fs::write(target.path().join("unrelated.txt"), b"x").unwrap();

        assert_eq!(list_workbuddy_backup_stamps(target.path()), vec![800, 900, 1000]);
        assert_eq!(prune_workbuddy_backups(target.path(), 0), 0);
        assert_eq!(prune_workbuddy_backups(target.path(), 2), 1);
        // 最新两份保留，最旧 800 整目录删除。
        assert!(target.path().join("workbuddy-data-1000").is_dir());
        assert!(target.path().join("workbuddy-data-900").is_dir());
        assert!(!target.path().join("workbuddy-data-800").exists());
        // 非链对象全部原样。
        assert!(target.path().join("workbuddy-data-700-extra").is_dir());
        assert!(target.path().join("workbuddy-data-abc").is_dir());
        assert!(target.path().join(".wb-baking-600").is_dir());
        assert!(target.path().join("unrelated.txt").is_file());
    }

    /// 链条目统计：manifest 可读时给声明值，磁盘字节为实际遍历合计。
    #[test]
    fn list_entries_reports_manifest_and_disk_bytes() {
        let root = tempdir().unwrap();
        let source = root.path().join("app");
        make_source(&source);
        let target = root.path().join("backups");
        let outcome = backup_workbuddy_data_to(&source, &target).unwrap();
        let entries = list_workbuddy_backups(&target);
        assert_eq!(entries.len(), 1);
        let entry = &entries[0];
        assert_eq!(entry.file_count, Some(7));
        assert_eq!(entry.source_bytes, Some(outcome.source_bytes));
        // 磁盘字节 ≥ 压缩成员合计（还含 manifest）。
        assert!(entry.compressed_bytes >= outcome.compressed_bytes);
    }

    /// 源目录不存在 → SourceUnavailable（不创建任何目标内容）。
    #[test]
    fn backup_rejects_missing_source() {
        let root = tempdir().unwrap();
        let error = backup_workbuddy_data_to(&root.path().join("nope"), &root.path().join("b"))
            .unwrap_err();
        assert_eq!(error, WorkBuddyBackupError::SourceUnavailable);
        assert!(!root.path().join("b").exists());
    }
}
