//! P17-T4b/P17-A3：WorkBuddy 数据目录备份治理（zstd 压缩 + 清单 JSON +
//! 恢复 + 滚动单份）。
//!
//! P17-A3 改造（spec 10.2 定案，2026-10-18）：
//! - **备份范围收窄：黑名单改白名单制**——仅登录凭据 + 配置文件（凭据
//!   明文为用户拍板）；对话/IndexedDB/LocalStorage/缓存/封面全部不备。
//! - **滚动单份**：同一时刻备份目录内只有一份 `workbuddy-data-{stamp}`；
//!   旧份在新份发布成功后彻底删除（`hard_delete_workbuddy_backup` 直删，
//!   **不走回收站**——备份属可再生数据，spec 10.4 删除纪律）；keep=N
//!   链式保留退役。
//!
//! P19-B3 范围扩展（p19-plan 决策 1 用户拍板：**WB 备份范围 = 凭据材料
//! + workbuddy.db + projects 正文目录，全纳入**）。在 P17-A3 白名单之上
//! 新增 `data_root`（`~/.workbuddy`）来源（见 [`WorkBuddyBackupSources`]）：
//! 4. 索引库 `workbuddy.db`（连同 `-wal`/`-shm` 附属件，存在才收）——
//!    命名空间 `db/`；
//! 5. 身份材料 `device-id` / `account-snapshot.json`（P17 8.7 缺口警示
//!    补齐）——命名空间 `identity/`；
//! 6. 会话正文目录 `projects/` 全量（jsonl 正文随库备份）——命名空间
//!    `projects/`。
//! IndexedDB/LocalStorage/缓存/封面等跟号数据仍不在备份范围（P17-A3
//! 白名单制不变）；`data_root` 缺失（客户端从未初始化）时新增命名空间
//! 为空，不视为错误。
//!
//! 与 Trae 侧（`backup_retention` / `master_handover`）同纪律的差异适配：
//! - 备份对象是**多源小文件集**（凭据成员 KB 级 + 正文 jsonl 可达 MB 级），
//!   沿用 P16-T1「每成员独立标准 zstd 帧、无自定义容器」决策——按文件
//!   逐个压缩为 `{命名空间}/{相对名}.zst`，`manifest.json` 记录全部成员
//!   （相对路径/原字节/sha256）；
//! - 备份编排的前置是 `workbuddy_instance::close_workbuddy()`（编排层
//!   职责，本模块只做纯文件操作；P19-B3 起含库文件——活库带 wal 备份
//!   不一致，必须先关客户端）；任一成员压缩失败 → 删除暂存目录
//!   整体失败（fail-closed，不产出不完整备份，零副作用）。
//!
//! 布局：`{target_dir}/workbuddy-data-{stamp}/manifest.json` +
//! `{app-config.json|auth/…|containers/…|db/…|identity/…|projects/…}.zst`。
//! 恢复按命名空间前缀映射回各源位置并 sha256 校验；恢复路径对压缩
//! （`.zst`）与未压缩（原始文件）双形态成员兼容。manifest format_version
//! 保持 1（新增命名空间为加法演进，旧份恢复不受影响）。

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

/// P17-A3 白名单：应用配置成员名（`app_config_root` 根部精确命中）。
const APP_CONFIG_MEMBER: &str = "app-config.json";
/// 恢复命名空间前缀：登录凭据来源容器成员。
const AUTH_NAMESPACE: &str = "auth/";
/// 恢复命名空间前缀：工具侧登录代次容器成员。
const CONTAINERS_NAMESPACE: &str = "containers/";
/// P19-B3 恢复命名空间前缀：索引库成员（`db/workbuddy.db[-wal|-shm]`）。
const DB_NAMESPACE: &str = "db/";
/// P19-B3 恢复命名空间前缀：身份材料成员（`identity/device-id` 等）。
const IDENTITY_NAMESPACE: &str = "identity/";
/// P19-B3 恢复命名空间前缀：会话正文目录成员（`projects/<转义目录>/<id>.jsonl`）。
const PROJECTS_NAMESPACE: &str = "projects/";
/// P19-B3：索引库文件名（与 workbuddy_transfer::DB_FILE_NAME 同名常量——
/// 本模块不受 sqlcipher feature 门控，不跨门控引用）。
const DB_FILE: &str = "workbuddy.db";
/// P19-B3：会话正文目录名（与 workbuddy_transfer::PROJECTS_DIR_NAME 同名）。
const PROJECTS_DIR: &str = "projects";

/// P17-A3 备份源三元组 + P19-B3 数据根来源。
///
/// 纪律：`app_config_root` 必须存在（WorkBuddy 应用数据根，备份的锚点）；
/// `auth_dir` / `login_container_dir` / `data_root` 下的各成员可缺失
/// （从未登录/从未捕获/客户端从未初始化时对应命名空间为空，不视为错误）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkBuddyBackupSources {
    /// WorkBuddy 应用配置根（`~/.workbuddy/app`）——白名单仅
    /// `app-config.json`。
    pub app_config_root: PathBuf,
    /// 登录凭据来源容器目录
    /// （`%LOCALAPPDATA%\CodeBuddyExtension\Data\Public\auth`）——白名单仅
    /// 文件名含 `.info` 的会话/登出标记文件。
    pub auth_dir: PathBuf,
    /// 工具侧登录代次容器目录（`{storage_root}/workbuddy-login`）——
    /// 白名单仅 `wb-*.wbsess`。
    pub login_container_dir: PathBuf,
    /// P19-B3（决策 1）：数据根（`~/.workbuddy`）——索引库 workbuddy.db、
    /// projects 正文目录、device-id / account-snapshot.json 身份材料的来源。
    pub data_root: PathBuf,
}

/// 清单成员：备份目录内相对路径 + 原始字节 + sha256（hex）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkBuddyBackupMember {
    /// 相对路径（`/` 分隔，无 `.zst` 后缀；命名空间前缀见
    /// `AUTH_NAMESPACE` / `CONTAINERS_NAMESPACE`，配置成员在根部）。
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
    /// 本次滚动清理删除的旧备份数（P17-A3；首份备份为 0）。
    pub rolled_over: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkBuddyBackupError {
    /// 应用配置根不存在（数据根未安装/路径错）。
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

/// P17-A3 白名单收集备份计划：(源绝对路径, 备份目录内相对路径)。
///
/// 相对路径按命名空间组织（排序保证 manifest 稳定）；白名单之外的任何
/// 文件一律不收——防御性保证 IndexedDB/LocalStorage/缓存/封面不进
/// 备份（跟号走；P19-B3 起对话库/正文按用户拍板纳入，见
/// `collect_data_root_plan`）。白名单命中但非常规对象（符号链接等）
/// 拒绝（fail-closed，不猜测内容）。
fn collect_scope_plan(
    sources: &WorkBuddyBackupSources,
) -> Result<Vec<(PathBuf, String)>, WorkBuddyBackupError> {
    let mut plan: Vec<(PathBuf, String)> = Vec::new();
    // 1) 应用配置：根部 app-config.json（存在才收；缺失 = 未首次启动，
    //    不视为错误——凭据容器仍是备份主体）。
    let app_config = sources.app_config_root.join(APP_CONFIG_MEMBER);
    if app_config.is_file() {
        plan.push((app_config, APP_CONFIG_MEMBER.to_string()));
    }
    // 2) 登录凭据来源容器：文件名含 `.info`（会话 .info 与登出标记
    //    `.info.{hash}.observation.json`——登出标记属登录状态证据）。
    if sources.auth_dir.is_dir() {
        let entries = fs::read_dir(&sources.auth_dir).map_err(|_| WorkBuddyBackupError::Io)?;
        for entry in entries {
            let entry = entry.map_err(|_| WorkBuddyBackupError::Io)?;
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if !name.contains(".info") {
                continue;
            }
            // DirEntry::metadata 不跟随符号链接（fail-closed，防逃逸）。
            if !entry
                .metadata()
                .map_err(|_| WorkBuddyBackupError::Io)?
                .is_file()
            {
                return Err(WorkBuddyBackupError::Io);
            }
            plan.push((
                entry.path(),
                format!("{AUTH_NAMESPACE}{name}"),
            ));
        }
    }
    // 3) 工具侧登录代次容器：`wb-*.wbsess`（原子写暂存 `.tmp` 不匹配，
    //    天然排除半成品）。
    if sources.login_container_dir.is_dir() {
        let entries =
            fs::read_dir(&sources.login_container_dir).map_err(|_| WorkBuddyBackupError::Io)?;
        for entry in entries {
            let entry = entry.map_err(|_| WorkBuddyBackupError::Io)?;
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if !(name.starts_with("wb-") && name.ends_with(".wbsess")) {
                continue;
            }
            if !entry
                .metadata()
                .map_err(|_| WorkBuddyBackupError::Io)?
                .is_file()
            {
                return Err(WorkBuddyBackupError::Io);
            }
            plan.push((
                entry.path(),
                format!("{CONTAINERS_NAMESPACE}{name}"),
            ));
        }
    }
    // 4) P19-B3（决策 1）：数据根成员（索引库/身份材料/projects 正文）。
    collect_data_root_plan(sources, &mut plan)?;
    plan.sort_by(|a, b| a.1.cmp(&b.1));
    Ok(plan)
}

/// P19-B3（决策 1）：数据根成员收集——索引库 + 身份材料 + projects 正文。
///
/// - `workbuddy.db` 与 `-wal`/`-shm` 附属件（存在才收；恢复时成套还原）；
/// - `device-id` / `account-snapshot.json`（P17 8.7 缺口补齐）；
/// - `projects/` 全量递归（jsonl 正文随库备份；符号链接等非常规对象
///   fail-closed 拒绝——防逃逸纪律与白名单面一致）。
fn collect_data_root_plan(
    sources: &WorkBuddyBackupSources,
    plan: &mut Vec<(PathBuf, String)>,
) -> Result<(), WorkBuddyBackupError> {
    let data_root = &sources.data_root;
    // 1) 索引库三件套。
    for suffix in ["", "-wal", "-shm"] {
        let member = format!("{DB_FILE}{suffix}");
        let path = data_root.join(&member);
        if path.is_file() {
            plan.push((path, format!("{DB_NAMESPACE}{member}")));
        }
    }
    // 2) 身份材料。
    for member in ["device-id", "account-snapshot.json"] {
        let path = data_root.join(member);
        if path.is_file() {
            plan.push((path, format!("{IDENTITY_NAMESPACE}{member}")));
        }
    }
    // 3) projects 正文全量递归。
    let projects_root = data_root.join(PROJECTS_DIR);
    if projects_root.is_dir() {
        collect_projects_files(&projects_root, PROJECTS_NAMESPACE, plan)?;
    }
    Ok(())
}

/// projects 目录递归收集（相对路径挂在 `prefix` 命名空间下）。
fn collect_projects_files(
    dir: &Path,
    prefix: &str,
    plan: &mut Vec<(PathBuf, String)>,
) -> Result<(), WorkBuddyBackupError> {
    let entries = std::fs::read_dir(dir).map_err(|_| WorkBuddyBackupError::Io)?;
    for entry in entries {
        let entry = entry.map_err(|_| WorkBuddyBackupError::Io)?;
        let name = entry.file_name().to_string_lossy().into_owned();
        // DirEntry::metadata 不跟随符号链接（fail-closed，防逃逸）。
        let metadata = entry.metadata().map_err(|_| WorkBuddyBackupError::Io)?;
        if metadata.is_dir() {
            collect_projects_files(&entry.path(), &format!("{prefix}{name}/"), plan)?;
        } else if metadata.is_file() {
            plan.push((entry.path(), format!("{prefix}{name}")));
        } else {
            // 符号链接等非常规对象：拒绝（不猜测内容）。
            return Err(WorkBuddyBackupError::Io);
        }
    }
    Ok(())
}

/// 备份 WorkBuddy 凭据+配置白名单到 target_dir（滚动单份）。
///
/// 流程：收集白名单计划 → 暂存目录逐文件压缩（create_new）→ 写 manifest
/// → 同卷改名发布 → **彻底删除其余旧份**（滚动单份；先写后删的失败安全
/// 顺序——写失败退化为保留旧份，备份系统的失败模式必须是「多留」而非
/// 「丢失」，与 Trae 侧 `backup_master_trio_rolling_stamped` 同决策）。
/// 任一步失败清理暂存返回 Err（零副作用）；最终名已存在 → 拒绝。
/// 前置编排（关闭 WorkBuddy）由调用方负责。
pub fn backup_workbuddy_scope_to(
    sources: &WorkBuddyBackupSources,
    target_dir: &Path,
) -> Result<WorkBuddyBackupOutcome, WorkBuddyBackupError> {
    backup_workbuddy_scope_stamped(sources, target_dir, unix_seconds_now())
}

/// 显式时间戳版本（编排层可统一发号；测试确定性构造现场）。
/// 语义与 [`backup_workbuddy_scope_to`] 完全一致（滚动单份）。
pub fn backup_workbuddy_scope_stamped(
    sources: &WorkBuddyBackupSources,
    target_dir: &Path,
    stamp: u64,
) -> Result<WorkBuddyBackupOutcome, WorkBuddyBackupError> {
    backup_workbuddy_scope_stamped_inner(sources, target_dir, stamp, true)
}

/// P19-B8：非滚动发布变体（**保留既有链条**）——恢复前置「再备份当前态
/// 一次」专用：滚动语义会清掉全部旧份（含即将恢复的目标份），恢复前置
/// 必须保链。其余语义（暂存压缩/manifest/原子发布）与滚动版一致。
pub fn backup_workbuddy_scope_stamped_keep_chain(
    sources: &WorkBuddyBackupSources,
    target_dir: &Path,
    stamp: u64,
) -> Result<WorkBuddyBackupOutcome, WorkBuddyBackupError> {
    backup_workbuddy_scope_stamped_inner(sources, target_dir, stamp, false)
}

/// 发布内核（rollover = 是否发布成功后滚动清旧）。
fn backup_workbuddy_scope_stamped_inner(
    sources: &WorkBuddyBackupSources,
    target_dir: &Path,
    stamp: u64,
    rollover: bool,
) -> Result<WorkBuddyBackupOutcome, WorkBuddyBackupError> {
    if !sources.app_config_root.is_dir() {
        return Err(WorkBuddyBackupError::SourceUnavailable);
    }
    std::fs::create_dir_all(target_dir).map_err(|_| WorkBuddyBackupError::Io)?;
    let final_dir = target_dir.join(format!("{BACKUP_DIR_PREFIX}{stamp}"));
    if final_dir.exists() {
        return Err(WorkBuddyBackupError::TargetExists);
    }
    let plan = collect_scope_plan(sources)?;

    // 暂存压缩：任一成员失败 → 清暂存、零副作用退出（不产出不完整备份）。
    // 空白名单计划（如凭据容器全缺）同样需要暂存目录落 manifest。
    let staging = target_dir.join(format!(".wb-baking-{stamp}"));
    fs::create_dir_all(&staging).map_err(|_| WorkBuddyBackupError::Io)?;
    let cleanup = |staging: &Path| {
        let _ = fs::remove_dir_all(staging);
    };
    let mut members = Vec::with_capacity(plan.len());
    let mut source_bytes = 0_u64;
    let mut compressed_bytes = 0_u64;
    for (source, relative) in &plan {
        // Path::join 接受 `/` 分隔的相对串（Windows API 亦兼容 `/`）。
        let target = staging.join(format!("{relative}.zst"));
        let Some(parent) = target.parent() else {
            cleanup(&staging);
            return Err(WorkBuddyBackupError::Io);
        };
        if fs::create_dir_all(parent).is_err()
            || compress_file_create_new(source, &target).is_err()
        {
            cleanup(&staging);
            return Err(WorkBuddyBackupError::Io);
        }
        let bytes = fs::metadata(source).map(|meta| meta.len()).unwrap_or(0);
        let compressed = fs::metadata(&target).map(|meta| meta.len()).unwrap_or(0);
        let sha256 = crate::snapshot_store::sha256_file(source).ok_or_else(|| {
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

    // P17-A3 滚动单份：发布成功后彻底删除其余旧份（直删不走回收站）。
    // P19-B8：keep_chain 变体（rollover=false）跳过滚动——恢复前置再备份
    // 不能清掉即将恢复的目标份。
    let mut rolled_over = 0_usize;
    if rollover {
        for other in list_workbuddy_backup_stamps(target_dir) {
            if other != stamp && hard_delete_workbuddy_backup(target_dir, other) {
                rolled_over += 1;
            }
        }
    }

    Ok(WorkBuddyBackupOutcome {
        backup_dir: final_dir,
        file_count: manifest.file_count,
        source_bytes: manifest.source_bytes,
        compressed_bytes,
        rolled_over,
    })
}

/// P17-A3 彻底删除原语：整目录 `remove_dir_all` 直删一份 WorkBuddy 备份，
/// **不走回收站**（备份属可再生数据，spec 10.4 删除纪律——safe-delete
/// 回收站 shim 仅用于用户可能误删需找回的场景）。返回是否实际删除。
pub fn hard_delete_workbuddy_backup(target_dir: &Path, stamp: u64) -> bool {
    let dir = target_dir.join(format!("{BACKUP_DIR_PREFIX}{stamp}"));
    dir.is_dir() && fs::remove_dir_all(&dir).is_ok()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkBuddyRestoreError {
    /// manifest.json 缺失。
    ManifestMissing,
    /// manifest 损坏/超限/格式版本不符。
    ManifestInvalid,
    /// 清单成员路径不安全（绝对路径/`..`/反斜杠/未知命名空间——防清单被
    /// 篡改写入任意位置）。
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

/// P17-A3：清单成员相对路径 → 还原目标（按命名空间前缀映射回源位置）。
/// 配置成员在根部；`auth/` 前缀 → 凭据来源容器目录；`containers/` 前缀 →
/// 工具侧代次容器目录；P19-B3：`db/` 与 `identity/` 前缀 → 数据根，
/// `projects/` 前缀 → 数据根正文目录。未知前缀返回 None（调用方按
/// UnsafePath 拒绝）。
fn resolve_scope_target(sources: &WorkBuddyBackupSources, relative: &str) -> Option<PathBuf> {
    if relative == APP_CONFIG_MEMBER {
        return Some(sources.app_config_root.join(APP_CONFIG_MEMBER));
    }
    if let Some(rest) = relative.strip_prefix(AUTH_NAMESPACE) {
        if rest.is_empty() {
            return None;
        }
        return Some(sources.auth_dir.join(rest));
    }
    if let Some(rest) = relative.strip_prefix(CONTAINERS_NAMESPACE) {
        if rest.is_empty() {
            return None;
        }
        return Some(sources.login_container_dir.join(rest));
    }
    // P19-B3：数据根命名空间（db/identity/projects）。
    if let Some(rest) = relative
        .strip_prefix(DB_NAMESPACE)
        .or_else(|| relative.strip_prefix(IDENTITY_NAMESPACE))
    {
        if rest.is_empty() {
            return None;
        }
        return Some(sources.data_root.join(rest));
    }
    if let Some(rest) = relative.strip_prefix(PROJECTS_NAMESPACE) {
        if rest.is_empty() {
            return None;
        }
        return Some(sources.data_root.join(PROJECTS_DIR).join(rest));
    }
    None
}

/// 从备份目录恢复白名单成员到各源位置（逐成员解压 + sha256 校验）。
///
/// 双格式兼容：成员优先取 `{path}.zst`（压缩形态），缺席时回退原始文件
/// `{path}`（手工维护/未来未压缩成员）。目标既有文件被覆盖（恢复语义 =
/// 目标内容替换为备份内容）；重复恢复结果一致（幂等）。任一成员校验
/// 失败 → 删除该半写目标并中止（不产出部分校验通过的状态混存）。
pub fn restore_workbuddy_scope_from(
    backup_dir: &Path,
    sources: &WorkBuddyBackupSources,
) -> Result<WorkBuddyRestoreOutcome, WorkBuddyRestoreError> {
    let manifest = read_manifest(backup_dir)?;
    let mut restored_files = 0_usize;
    let mut restored_bytes = 0_u64;
    for member in &manifest.files {
        if !member_path_is_safe(&member.path) {
            return Err(WorkBuddyRestoreError::UnsafePath);
        }
        // 未知命名空间/空段：fail-closed（清单不可信，不猜测落点）。
        let Some(target) = resolve_scope_target(sources, &member.path) else {
            return Err(WorkBuddyRestoreError::UnsafePath);
        };
        let compressed = backup_dir.join(format!("{}.zst", member.path));
        let raw = backup_dir.join(&member.path);
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

/// 保留裁剪（P17-A3 keep=N 链式保留退役）：**滚动单份**语义——保留最新
/// 1 份，更旧的整目录彻底删除（`hard_delete_workbuddy_backup` 直删，不走
/// 回收站）。`keep` 语义变为开关：0 = 不删任何东西（禁用语义）；≥1 一律
/// 按保留最新 1 份执行。删除失败静默跳过（该份下次再试）；命名不符的
/// 散目录永不触碰。返回删除份数。
pub fn prune_workbuddy_backups(target_dir: &Path, keep: u32) -> usize {
    if keep == 0 {
        return 0;
    }
    let stamps = list_workbuddy_backup_stamps(target_dir);
    // stamps 升序；保留最后一份（最新），其余直删。
    stamps
        .iter()
        .rev()
        .skip(1)
        .filter(|stamp| hard_delete_workbuddy_backup(target_dir, **stamp))
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    /// 构造白名单源：配置 + 登录凭据来源容器 + 工具侧代次容器 +
    /// P19-B3 数据根（索引库/身份材料/projects 正文），
    /// 外加全部应被排除的干扰对象（IndexedDB/缓存/封面/半成品 tmp）。
    fn make_sources(root: &Path) -> WorkBuddyBackupSources {
        let app_root = root.join("app");
        fs::create_dir_all(&app_root).unwrap();
        fs::write(app_root.join("app-config.json"), b"{\"version\":1}").unwrap();
        // 排除面：Chromium 会话树（IndexedDB/LocalStorage/缓存）与封面。
        let session = app_root.join("session").join("IndexedDB");
        fs::create_dir_all(&session).unwrap();
        fs::write(session.join("000001.log"), b"idb-bytes").unwrap();
        let storage = app_root.join("session").join("Local Storage");
        fs::create_dir_all(&storage).unwrap();
        fs::write(storage.join("000003.log"), b"ls-bytes").unwrap();
        fs::create_dir_all(app_root.join("CodeCache")).unwrap();
        fs::write(app_root.join("CodeCache").join("blob"), b"cache").unwrap();
        let covers = app_root.join("playbook-covers");
        fs::create_dir_all(&covers).unwrap();
        fs::write(covers.join("a.png"), vec![9_u8; 2048]).unwrap();
        fs::write(app_root.join("lockfile"), b"").unwrap();

        let auth_dir = root.join("auth");
        fs::create_dir_all(&auth_dir).unwrap();
        fs::write(auth_dir.join("workbuddy-desktop.info"), b"auth-info-bytes").unwrap();
        // 登出标记（含 .info 的证据文件）：白名单内。
        fs::write(
            auth_dir.join("workbuddy-desktop.info.5d63bdd69323e571.observation.json"),
            b"logout-marker",
        )
        .unwrap();
        // 干扰文件：不含 .info → 排除。
        fs::write(auth_dir.join("unrelated.txt"), b"noise").unwrap();

        let containers = root.join("workbuddy-login");
        fs::create_dir_all(&containers).unwrap();
        fs::write(containers.join("wb-uid-1.wbsess"), b"container-bytes").unwrap();
        // 原子写暂存半成品：不匹配 wb-*.wbsess → 排除。
        fs::write(containers.join("wb-uid-1.wbsess.tmp"), b"half-written").unwrap();
        fs::write(containers.join("notes.txt"), b"text").unwrap();

        // P19-B3（决策 1）：数据根成员——索引库 + 身份材料 + projects 正文。
        let data_root = root.join(".workbuddy");
        fs::create_dir_all(&data_root).unwrap();
        fs::write(data_root.join("workbuddy.db"), b"db-bytes").unwrap();
        fs::write(data_root.join("device-id"), b"device-id-bytes").unwrap();
        fs::write(data_root.join("account-snapshot.json"), b"{\"primary\":{}}").unwrap();
        let projects = data_root.join("projects").join("d-work-demo");
        fs::create_dir_all(&projects).unwrap();
        fs::write(projects.join("sess-1.jsonl"), b"jsonl-line-1\njsonl-line-2\n").unwrap();

        WorkBuddyBackupSources {
            app_config_root: app_root,
            auth_dir,
            login_container_dir: containers,
            data_root,
        }
    }

    /// P17-A3 白名单 + P19-B3 数据根范围：配置/.info 族/wbsess/索引库/
    /// 身份材料/projects 正文进备份；IndexedDB/LocalStorage/缓存/封面/
    /// 暂存半成品全部排除。
    #[test]
    fn backup_scope_is_whitelist_of_credentials_and_config() {
        let root = tempdir().unwrap();
        let sources = make_sources(root.path());
        let target = root.path().join("backups");

        let outcome = backup_workbuddy_scope_to(&sources, &target).unwrap();
        assert_eq!(outcome.file_count, 8);
        assert_eq!(outcome.rolled_over, 0, "首份备份无旧份可滚动");
        // 源字节：app-config 13 + .info 15 + 登出标记 13 + 容器 15
        // + db 8 + device-id 16 + snapshot 13 + jsonl 26。
        assert_eq!(
            outcome.source_bytes,
            13 + 15 + 13 + 15 + 8 + 16 + 13 + 26
        );

        let backup = &outcome.backup_dir;
        assert!(backup.join("manifest.json").is_file());
        assert!(backup.join("app-config.json.zst").is_file());
        assert!(backup.join("auth/workbuddy-desktop.info.zst").is_file());
        assert!(backup
            .join("auth/workbuddy-desktop.info.5d63bdd69323e571.observation.json.zst")
            .is_file());
        assert!(backup.join("containers/wb-uid-1.wbsess.zst").is_file());
        // P19-B3 命名空间落位。
        assert!(backup.join("db/workbuddy.db.zst").is_file());
        assert!(backup.join("identity/device-id.zst").is_file());
        assert!(backup.join("identity/account-snapshot.json.zst").is_file());
        assert!(backup.join("projects/d-work-demo/sess-1.jsonl.zst").is_file());
        // 排除面全部不进备份。
        assert!(!backup.join("session").exists());
        assert!(!backup.join("CodeCache").exists());
        assert!(!backup.join("playbook-covers").exists());
        assert!(!backup.join("lockfile.zst").exists());
        assert!(!backup.join("containers/notes.txt.zst").exists());
        assert!(!backup.join("containers/wb-uid-1.wbsess.tmp.zst").exists());
        assert!(!backup.join("auth/unrelated.txt.zst").exists());
        // zstd 帧头（标准帧 magic）。
        let head = fs::read(backup.join("app-config.json.zst")).unwrap();
        assert_eq!(&head[..4], &[0x28, 0xB5, 0x2F, 0xFD]);
        // manifest 统计与回执一致。
        let manifest = read_manifest(backup).unwrap();
        assert_eq!(manifest.file_count, 8);
        assert_eq!(manifest.source_bytes, outcome.source_bytes);
    }

    /// 同 stamp 目标已存在 → 拒绝且不覆盖（永不覆盖既有备份纪律）。
    #[test]
    fn backup_refuses_existing_target() {
        let root = tempdir().unwrap();
        let sources = make_sources(root.path());
        let target = root.path().join("backups");
        let first = backup_workbuddy_scope_stamped(&sources, &target, 1000).unwrap();
        assert_eq!(
            first.backup_dir,
            target.join("workbuddy-data-1000"),
            "备份目录按 stamp 命名"
        );
        // 同 stamp 重跑：TargetExists，占位内容未被破坏。
        assert_eq!(
            backup_workbuddy_scope_stamped(&sources, &target, 1000).unwrap_err(),
            WorkBuddyBackupError::TargetExists
        );
        assert!(first.backup_dir.join("manifest.json").is_file());
    }

    /// P19-B8 keep_chain 变体：发布不滚动——两次备份并存，既有链条
    /// 不被清理（恢复前置再备份的语义基础）。
    #[test]
    fn keep_chain_backup_publishes_without_rollover() {
        let root = tempdir().unwrap();
        let sources = make_sources(root.path());
        let target = root.path().join("backups");
        backup_workbuddy_scope_stamped(&sources, &target, 1000).unwrap();
        let second =
            backup_workbuddy_scope_stamped_keep_chain(&sources, &target, 2000).unwrap();
        assert_eq!(second.rolled_over, 0, "keep_chain 不触发滚动");
        // 两条并存（对照滚动版：rolling_backup_keeps_single_copy 只留一份）。
        assert_eq!(list_workbuddy_backup_stamps(&target), vec![1000, 2000]);
        // keep_chain 目标已存在同样拒绝（永不覆盖既有备份纪律不变）。
        assert_eq!(
            backup_workbuddy_scope_stamped_keep_chain(&sources, &target, 2000).unwrap_err(),
            WorkBuddyBackupError::TargetExists
        );
    }

    /// P17-A3 滚动单份：第二次备份后旧份整目录彻底删除（直删不进回收站
    /// ——目录不存在即证）；同一时刻目录内只有一份备份。
    #[test]
    fn rolling_backup_keeps_single_copy() {
        let root = tempdir().unwrap();
        let sources = make_sources(root.path());
        let target = root.path().join("backups");
        // 旧份遗留现场（模拟 P17-A3 前的链式备份 + 命名散目录）。
        let legacy = target.join("workbuddy-data-900");
        fs::create_dir_all(&legacy).unwrap();
        fs::write(legacy.join("manifest.json"), b"{}").unwrap();
        let stray = target.join("workbuddy-data-abc");
        fs::create_dir_all(&stray).unwrap();
        fs::write(stray.join("x"), b"stray").unwrap();

        let second = backup_workbuddy_scope_stamped(&sources, &target, 2000).unwrap();
        assert_eq!(second.rolled_over, 1, "旧份 900 被滚动删除");
        assert!(!legacy.exists(), "旧份已被彻底删除（不走回收站）");
        // 命名不符的散目录永不触碰（铁律：失败证据不清）。
        assert!(stray.is_dir());
        // 目录内只剩新份一条。
        assert_eq!(list_workbuddy_backup_stamps(&target), vec![2000]);
        // 第三次滚动：同样只留一份。
        backup_workbuddy_scope_stamped(&sources, &target, 3000).unwrap();
        assert_eq!(list_workbuddy_backup_stamps(&target), vec![3000]);
        assert!(!second.backup_dir.exists());
    }

    /// 恢复往返：恢复内容与源逐文件字节一致；重复恢复幂等。
    #[test]
    fn restore_roundtrip_byte_identical_and_idempotent() {
        let root = tempdir().unwrap();
        let sources = make_sources(root.path());
        let target = root.path().join("backups");
        backup_workbuddy_scope_to(&sources, &target).unwrap();

        // 先改源再恢复：证明恢复把目标内容替换为备份内容。
        fs::write(sources.app_config_root.join("app-config.json"), b"drifted").unwrap();
        fs::write(sources.data_root.join("workbuddy.db"), b"drifted-db").unwrap();
        let backup_dir = target.join(format!(
            "workbuddy-data-{}",
            list_workbuddy_backup_stamps(&target)[0]
        ));
        let outcome = restore_workbuddy_scope_from(&backup_dir, &sources).unwrap();
        assert_eq!(outcome.restored_files, 8);
        assert_eq!(
            fs::read(sources.app_config_root.join("app-config.json")).unwrap(),
            b"{\"version\":1}",
            "漂移后的配置被备份内容覆盖"
        );
        assert_eq!(
            fs::read(sources.data_root.join("workbuddy.db")).unwrap(),
            b"db-bytes",
            "漂移后的索引库被备份内容覆盖"
        );
        assert_eq!(
            fs::read(sources.auth_dir.join("workbuddy-desktop.info")).unwrap(),
            b"auth-info-bytes"
        );
        assert_eq!(
            fs::read(sources.login_container_dir.join("wb-uid-1.wbsess")).unwrap(),
            b"container-bytes"
        );
        // P19-B3：identity 与 projects 命名空间还原到数据根。
        assert_eq!(
            fs::read(sources.data_root.join("device-id")).unwrap(),
            b"device-id-bytes"
        );
        assert_eq!(
            fs::read(
                sources
                    .data_root
                    .join("projects")
                    .join("d-work-demo")
                    .join("sess-1.jsonl")
            )
            .unwrap(),
            b"jsonl-line-1\njsonl-line-2\n"
        );
        // 幂等：重复恢复结果一致。
        let second = restore_workbuddy_scope_from(&backup_dir, &sources).unwrap();
        assert_eq!(second, outcome);
    }

    /// P19-B3：索引库 -wal/-shm 附属件存在时一并收录、成套还原
    /// （活库一致性语义：三件套缺一不可——单还原 db 不还原 wal 会丢数据）。
    #[test]
    fn data_root_collects_db_sidecars_and_restores_them_together() {
        let root = tempdir().unwrap();
        let sources = make_sources(root.path());
        fs::write(sources.data_root.join("workbuddy.db-wal"), b"wal-bytes").unwrap();
        fs::write(sources.data_root.join("workbuddy.db-shm"), b"shm-bytes").unwrap();
        let target = root.path().join("backups");
        let outcome = backup_workbuddy_scope_to(&sources, &target).unwrap();
        let backup_dir = &outcome.backup_dir;
        assert!(backup_dir.join("db/workbuddy.db-wal.zst").is_file());
        assert!(backup_dir.join("db/workbuddy.db-shm.zst").is_file());
        // 删除源附属件后还原：成套回来。
        fs::remove_file(sources.data_root.join("workbuddy.db-wal")).unwrap();
        fs::remove_file(sources.data_root.join("workbuddy.db-shm")).unwrap();
        restore_workbuddy_scope_from(backup_dir, &sources).unwrap();
        assert_eq!(fs::read(sources.data_root.join("workbuddy.db-wal")).unwrap(), b"wal-bytes");
        assert_eq!(fs::read(sources.data_root.join("workbuddy.db-shm")).unwrap(), b"shm-bytes");
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
        let sources = WorkBuddyBackupSources {
            app_config_root: root.path().join("app"),
            auth_dir: root.path().join("auth"),
            login_container_dir: root.path().join("containers"),
            data_root: root.path().join(".workbuddy"),
        };
        fs::create_dir_all(&sources.app_config_root).unwrap();
        let outcome = restore_workbuddy_scope_from(&backup, &sources).unwrap();
        assert_eq!(outcome.restored_files, 1);
        assert_eq!(
            fs::read(sources.app_config_root.join("app-config.json")).unwrap(),
            b"{\"raw\":true}"
        );
    }

    /// 校验失败：成员内容被篡改 → VerifyFailed 且半写目标被清理。
    #[test]
    fn restore_fails_on_corrupt_member() {
        let root = tempdir().unwrap();
        let sources = make_sources(root.path());
        let target = root.path().join("backups");
        backup_workbuddy_scope_to(&sources, &target).unwrap();
        let backup_dir = target.join(format!(
            "workbuddy-data-{}",
            list_workbuddy_backup_stamps(&target)[0]
        ));
        // 篡改成员为合法 zstd 帧但内容不同（坏帧走 Io 分支，另见
        // restore_corrupt_frame_cleans_partial_target）。
        let tampered = zstd::stream::encode_all(std::io::Cursor::new(b"tampered"), 3).unwrap();
        fs::write(backup_dir.join("app-config.json.zst"), tampered).unwrap();
        let error = restore_workbuddy_scope_from(&backup_dir, &sources).unwrap_err();
        match error {
            WorkBuddyRestoreError::VerifyFailed(path) => assert_eq!(path, "app-config.json"),
            other => panic!("期望 VerifyFailed，实际 {other:?}"),
        }
        // 半写目标已清理。
        assert!(sources
            .app_config_root
            .join("app-config.json")
            .metadata()
            .is_err());
    }

    /// 坏帧成员（非法 zstd 数据）→ Io 分支且半写目标被清理（fail-closed）。
    #[test]
    fn restore_corrupt_frame_cleans_partial_target() {
        let root = tempdir().unwrap();
        let sources = make_sources(root.path());
        let target = root.path().join("backups");
        backup_workbuddy_scope_to(&sources, &target).unwrap();
        let backup_dir = target.join(format!(
            "workbuddy-data-{}",
            list_workbuddy_backup_stamps(&target)[0]
        ));
        fs::write(backup_dir.join("app-config.json.zst"), b"corrupt").unwrap();
        assert_eq!(
            restore_workbuddy_scope_from(&backup_dir, &sources).unwrap_err(),
            WorkBuddyRestoreError::Io
        );
        // 解压失败的半写目标已清理。
        assert!(sources
            .app_config_root
            .join("app-config.json")
            .metadata()
            .is_err());
    }

    /// 恢复错误：manifest 缺失；成员缺失；不安全路径与未知命名空间拒绝。
    #[test]
    fn restore_error_branches() {
        let root = tempdir().unwrap();
        let sources = WorkBuddyBackupSources {
            app_config_root: root.path().join("app"),
            auth_dir: root.path().join("auth"),
            login_container_dir: root.path().join("containers"),
            data_root: root.path().join(".workbuddy"),
        };
        // manifest 缺失。
        assert_eq!(
            restore_workbuddy_scope_from(&root.path(), &sources).unwrap_err(),
            WorkBuddyRestoreError::ManifestMissing
        );
        // manifest 损坏。
        let backup = root.path().join("workbuddy-data-1");
        fs::create_dir_all(&backup).unwrap();
        fs::write(backup.join(MANIFEST_NAME), "not json").unwrap();
        assert_eq!(
            restore_workbuddy_scope_from(&backup, &sources).unwrap_err(),
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
                "files": [{"path": "auth/a.info", "bytes": 1, "sha256": "00"}]
            })
            .to_string(),
        )
        .unwrap();
        assert_eq!(
            restore_workbuddy_scope_from(&backup, &sources).unwrap_err(),
            WorkBuddyRestoreError::MemberMissing("auth/a.info".to_string())
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
                "files": [{"path": "auth/../escape.info", "bytes": 1, "sha256": "00"}]
            })
            .to_string(),
        )
        .unwrap();
        assert_eq!(
            restore_workbuddy_scope_from(&backup, &sources).unwrap_err(),
            WorkBuddyRestoreError::UnsafePath
        );
        // 未知命名空间（映射不到任何源位置）：同样 fail-closed 拒绝。
        fs::write(
            backup.join(MANIFEST_NAME),
            serde_json::json!({
                "format_version": 1,
                "product": "workbuddy",
                "stamp_unix_seconds": 1,
                "file_count": 1,
                "source_bytes": 1,
                "compressed_bytes": 1,
                "files": [{"path": "unknown/x.json", "bytes": 1, "sha256": "00"}]
            })
            .to_string(),
        )
        .unwrap();
        assert_eq!(
            restore_workbuddy_scope_from(&backup, &sources).unwrap_err(),
            WorkBuddyRestoreError::UnsafePath
        );
        // 绝对路径与反斜杠同样拒绝（纯校验函数）。
        assert!(!member_path_is_safe("C:/x"));
        assert!(!member_path_is_safe("a\\b"));
    }

    /// 链枚举与裁剪（P17-A3 滚动单份）：升序枚举、只留最新 1 份、旧份整
    /// 目录直删；散目录与 keep=0 不触碰。
    #[test]
    fn prune_rolls_to_newest_and_ignores_non_chain_dirs() {
        let target = tempdir().unwrap();
        for stamp in [800_u64, 900, 1000] {
            let dir = target.path().join(format!("workbuddy-data-{stamp}"));
            fs::create_dir_all(dir.join("auth")).unwrap();
            fs::write(dir.join("manifest.json"), b"{}").unwrap();
            fs::write(dir.join("auth").join("a.info"), b"{}").unwrap();
        }
        // 非链命名：带后缀 / 非数字 / 散文件 / 暂存目录——永不触碰。
        fs::create_dir_all(target.path().join("workbuddy-data-700-extra")).unwrap();
        fs::create_dir_all(target.path().join("workbuddy-data-abc")).unwrap();
        fs::create_dir_all(target.path().join(".wb-baking-600")).unwrap();
        fs::write(target.path().join("unrelated.txt"), b"x").unwrap();

        assert_eq!(list_workbuddy_backup_stamps(target.path()), vec![800, 900, 1000]);
        assert_eq!(prune_workbuddy_backups(target.path(), 0), 0);
        // P17-A3：keep 数值不再生效，≥1 一律滚动单份（删 2 份）。
        assert_eq!(prune_workbuddy_backups(target.path(), 2), 2);
        assert!(target.path().join("workbuddy-data-1000").is_dir());
        assert!(!target.path().join("workbuddy-data-900").exists());
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
        let sources = make_sources(root.path());
        let target = root.path().join("backups");
        let outcome = backup_workbuddy_scope_to(&sources, &target).unwrap();
        let entries = list_workbuddy_backups(&target);
        assert_eq!(entries.len(), 1);
        let entry = &entries[0];
        assert_eq!(entry.file_count, Some(8));
        assert_eq!(entry.source_bytes, Some(outcome.source_bytes));
        // 磁盘字节 ≥ 压缩成员合计（还含 manifest）。
        assert!(entry.compressed_bytes >= outcome.compressed_bytes);
    }

    /// 源目录不存在 → SourceUnavailable（不创建任何目标内容）；
    /// 凭据容器目录缺失不报错（从未登录/从未捕获时对应命名空间为空）；
    /// P19-B3：data_root 不存在同样不报错（客户端从未初始化 → 库域
    /// 命名空间为空）。
    #[test]
    fn backup_source_guards() {
        let root = tempdir().unwrap();
        let missing = WorkBuddyBackupSources {
            app_config_root: root.path().join("nope"),
            auth_dir: root.path().join("auth"),
            login_container_dir: root.path().join("containers"),
            data_root: root.path().join(".workbuddy"),
        };
        let error =
            backup_workbuddy_scope_to(&missing, &root.path().join("b")).unwrap_err();
        assert_eq!(error, WorkBuddyBackupError::SourceUnavailable);
        assert!(!root.path().join("b").exists());

        // app 根存在但凭据容器与数据根全缺：0 成员备份成功（空清单合法）。
        let app_root = root.path().join("app");
        fs::create_dir_all(&app_root).unwrap();
        let bare = WorkBuddyBackupSources {
            app_config_root: app_root.clone(),
            auth_dir: root.path().join("auth-missing"),
            login_container_dir: root.path().join("containers-missing"),
            data_root: root.path().join(".workbuddy-missing"),
        };
        let outcome = backup_workbuddy_scope_to(&bare, &root.path().join("b")).unwrap();
        assert_eq!(outcome.file_count, 0);
        assert_eq!(outcome.source_bytes, 0);
    }
}
