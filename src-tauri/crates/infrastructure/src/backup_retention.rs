//! 备份保留策略：定次自动裁剪（P5-9，2026-09-02 grill 定案）。
//!
//! 每次新备份生成后，删除超出保留数的旧备份（`.switch-bak-{时间戳}`
//! 主文件 + wal/shm 附属件 + 同 stamp 凭据子目录连带删；R0-P3）。保留数
//! 默认 5、可调（1-50）、可关闭；关闭时完全不自动删除（回退为永不自动
//! 删）。裁剪失败静默跳过（下次备份生成时再试），不阻断备份与切号主流程。
//!
//! 文件布局：`{storage_root}/backup-retention.json`；损坏时报错不静默重建
//! （与台账/档案同纪律——配置文件是用户决策的证据）。

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::atomic_publish::publish_replacing;
use crate::master_handover::{list_master_backups_in, MasterBackupEntry};

const STORE_FILE: &str = "backup-retention.json";
const MAX_STORE_BYTES: u64 = 64 * 1024;
/// 保留数下限（至少留 1 份，防止把备份链清空）。
pub const MIN_KEEP: u32 = 1;
/// 保留数上限（防误输入超大值；grill 定案范围 1-50）。
pub const MAX_KEEP: u32 = 50;
/// 备份目录路径长度上限（UTF-8 字节；防误输入超大路径）。
pub const MAX_BACKUP_DIR_BYTES: usize = 1024;

/// 备份保留设置存储错误；不携带敏感内容。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackupRetentionError {
    /// 文件损坏、格式不符或保留数/备份位置超范围。
    Invalid,
    /// 读写失败。
    Io,
}

/// 备份保留设置（用户可改）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackupRetentionSettings {
    /// 自动清理开关；false = 永不自动删除（回退原「永不自动删」行为）。
    pub enabled: bool,
    /// 保留最近 N 份（含刚生成的最新备份）。
    pub keep: u32,
    /// 备份位置覆盖（P11-4①）：Some = 绝对路径目录，所有新备份写到该目录；
    /// None/缺省 = 主库 db 同目录（原行为）。serde default 兼容旧配置文件。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backup_dir: Option<String>,
}

impl Default for BackupRetentionSettings {
    fn default() -> Self {
        // grill 定案默认值：开 + 保留最近 5 份 + 主库同目录。
        Self {
            enabled: true,
            keep: 5,
            backup_dir: None,
        }
    }
}

/// 设置合法性：保留数在 [MIN_KEEP, MAX_KEEP] 范围内。
pub fn is_valid_keep(keep: u32) -> bool {
    (MIN_KEEP..=MAX_KEEP).contains(&keep)
}

/// 备份位置合法性（P11-4①）：None 合法（默认位置）；Some 要求非空、
/// 绝对路径、长度上限内。可创建性由设置命令层校验（涉及磁盘 IO），
/// 这里只做纯格式校验（load/save 的 fail-closed 门）。
pub fn is_valid_backup_dir(backup_dir: Option<&str>) -> bool {
    match backup_dir {
        None => true,
        Some(dir) => {
            !dir.is_empty()
                && dir.len() <= MAX_BACKUP_DIR_BYTES
                && Path::new(dir).is_absolute()
        }
    }
}

#[derive(Serialize, Deserialize)]
struct StoreFile {
    format_version: u32,
    settings: BackupRetentionSettings,
}

/// 备份保留设置存储（storage_root 下单 JSON）。
pub struct BackupRetentionStore {
    path: PathBuf,
}

impl BackupRetentionStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            path: root.into().join(STORE_FILE),
        }
    }

    /// 读取设置；文件不存在时返回默认设置（首次使用）。
    pub fn load(&self) -> Result<BackupRetentionSettings, BackupRetentionError> {
        let metadata = match fs::symlink_metadata(&self.path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(BackupRetentionSettings::default());
            }
            Err(_) => return Err(BackupRetentionError::Io),
        };
        if !metadata.is_file() || metadata.len() > MAX_STORE_BYTES {
            return Err(BackupRetentionError::Invalid);
        }
        let content = fs::read_to_string(&self.path).map_err(|_| BackupRetentionError::Io)?;
        let store: StoreFile =
            serde_json::from_str(&content).map_err(|_| BackupRetentionError::Invalid)?;
        // fail-closed：保留数或备份位置非法都拒绝（配置文件是用户决策的证据，
        // 不静默采用越界值）。
        if store.format_version != 1
            || !is_valid_keep(store.settings.keep)
            || !is_valid_backup_dir(store.settings.backup_dir.as_deref())
        {
            return Err(BackupRetentionError::Invalid);
        }
        Ok(store.settings)
    }

    /// 保存设置（原子写：临时文件 + 原子替换，失败保留原文件）。
    ///
    /// 现有文件损坏时拒绝保存——不覆盖重建以保留用户决策证据
    /// （与台账/档案同纪律）；文件不存在视为首次使用，正常写入。
    pub fn save(&self, settings: &BackupRetentionSettings) -> Result<(), BackupRetentionError> {
        if !is_valid_keep(settings.keep) || !is_valid_backup_dir(settings.backup_dir.as_deref()) {
            return Err(BackupRetentionError::Invalid);
        }
        self.load().map_err(|_| BackupRetentionError::Invalid)?;
        let store = StoreFile {
            format_version: 1,
            settings: settings.clone(),
        };
        let content =
            serde_json::to_string_pretty(&store).map_err(|_| BackupRetentionError::Invalid)?;
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent).map_err(|_| BackupRetentionError::Io)?;
        }
        let temporary = self.path.with_extension("json.tmp");
        fs::write(&temporary, content.as_bytes()).map_err(|_| BackupRetentionError::Io)?;
        publish_replacing(&temporary, &self.path).map_err(|_| BackupRetentionError::Io)
    }
}

/// 读取备份位置覆盖（P11-4①，fail-soft）：配置读取失败或未配置返回 None
/// （回退主库 db 同目录行为），不阻断备份/切号主流程；配置合法时返回目录。
pub fn load_backup_dir_override(storage_root: &Path) -> Option<PathBuf> {
    BackupRetentionStore::new(storage_root)
        .load()
        .ok()
        .and_then(|settings| settings.backup_dir)
        .map(PathBuf::from)
}

/// 裁剪备份链：保留最新 `keep` 份，更旧的删除（附属件连带删）。
///
/// P11-4①：只处理 `backup_dir` 指定目录内的链——位置变更后旧目录的备份
/// 不再被自动清理（铁律：不自动删快照，旧位置由用户手动处置）。
/// R0-P3：删除某份主备份时，同 stamp 的凭据子目录（`credentials-{stamp}`）
/// 同进同退连带删除（存在才删，失败静默跳过）；keep 计数完全以主备份链
/// 为准，凭据子目录不参与计数；孤儿凭据子目录（对应主备份已不在）永不
/// 触碰（铁律：不自动删证据，由恢复指引提示用户手动清理）。
/// 返回删除的备份数。删除失败静默跳过（该份下次再试，不影响其余）；
/// `keep` 为 0 时不删任何东西（防御：调用方语义上应先确认启用）。
/// 只按 `list_master_backups_in` 的枚举结果删——命名异常的散件不属备份链，
/// 永不触碰（铁律：失败证据不清）。
pub fn prune_master_backups(backup_dir: &Path, db_name: &str, keep: u32) -> usize {
    if keep == 0 {
        return 0;
    }
    let chain = list_master_backups_in(backup_dir, db_name);
    // chain 已按时间戳倒序（最新在前）；跳过前 keep 份，其余逐份删除。
    let mut removed = 0;
    for entry in chain.iter().skip(keep as usize) {
        let stamp = entry.stamp_unix_seconds;
        let base = format!("{db_name}.switch-bak-{stamp}");
        let mut removed_this = true;
        for name in [&base, &format!("{base}-wal"), &format!("{base}-shm")] {
            // 删除失败不中断整体裁剪；该份下次备份生成时再试。
            if fs::remove_file(backup_dir.join(name)).is_err() {
                // 仅当文件不存在视为已删；其他失败标记该份未完全清理。
                if backup_dir.join(name).exists() {
                    removed_this = false;
                }
            }
        }
        // R0-P3：同 stamp 凭据子目录随主备份一并删除（存在才删，失败
        // 静默跳过——与主备份删除同口径，不影响主备份计数）。
        let credentials_dir = backup_dir.join(format!("credentials-{stamp}"));
        if credentials_dir.exists() {
            let _ = fs::remove_dir_all(&credentials_dir);
        }
        if removed_this {
            removed += 1;
        }
    }
    removed
}

/// 迁移回执：成功落位文件数 + 逐文件失败清单（不回滚，已复制的保留）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackupMigrationOutcome {
    /// 本次确认在新位置的文件数（含目标已存在且内容一致的幂等项）。
    pub migrated_files: usize,
    /// 失败清单（用户可读原因，不含绝对路径——file_name 为备份目录内相对名）。
    pub failures: Vec<BackupMigrationFailure>,
}

/// 单个文件的迁移失败记录。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackupMigrationFailure {
    /// 备份目录内相对名（凭据子目录带 `credentials-{stamp}/` 前缀）。
    pub file_name: String,
    /// 用户可读失败原因。
    pub reason: String,
}

/// 备份迁移（P11-4①）：把 source_dir 下全部备份链产物（三件套 + 凭据
/// 子目录）**复制**到 target_dir，逐文件 sha256 校验；原位保留不删
/// （铁律：不自动删快照）。
///
/// 幂等与安全边界：目标同名文件已存在且内容一致 → 计入成功（重复迁移
/// 安全）；不一致 → 记失败（永不覆盖既有文件）。复制后校验不一致时清掉
/// 刚复制的残件并记失败。命名异常散件不属备份链，永不触碰。
pub fn migrate_master_backups(
    source_dir: &Path,
    target_dir: &Path,
    db_name: &str,
) -> Result<BackupMigrationOutcome, BackupRetentionError> {
    std::fs::create_dir_all(target_dir).map_err(|_| BackupRetentionError::Io)?;
    // 收集待迁清单：(源绝对路径, 备份目录内相对显示名)。
    let mut plan: Vec<(PathBuf, String)> = Vec::new();
    for entry in list_master_backups_in(source_dir, db_name) {
        let base = format!("{db_name}.switch-bak-{}", entry.stamp_unix_seconds);
        for name in [&base, &format!("{base}-wal"), &format!("{base}-shm")] {
            let path = source_dir.join(name);
            if path.is_file() {
                plan.push((path, name.to_string()));
            }
        }
        // 凭据子目录（存在时）整体按相对结构迁入。
        let cred_relative = format!("credentials-{}", entry.stamp_unix_seconds);
        collect_dir_files(&source_dir.join(&cred_relative), &cred_relative, &mut plan)?;
    }
    let mut outcome = BackupMigrationOutcome {
        migrated_files: 0,
        failures: Vec::new(),
    };
    for (source, relative) in &plan {
        let target = target_dir.join(relative);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).map_err(|_| BackupRetentionError::Io)?;
        }
        if target.exists() {
            // 目标已有同名文件：内容一致视为已迁移（幂等）；不一致记失败
            // （永不覆盖既有文件——与备份链 create_new 同纪律）。
            match (fs::read(source), fs::read(&target)) {
                (Ok(origin), Ok(existing)) if origin == existing => outcome.migrated_files += 1,
                _ => outcome.failures.push(BackupMigrationFailure {
                    file_name: relative.clone(),
                    reason: "目标已存在同名文件且内容不一致，未覆盖".to_string(),
                }),
            }
            continue;
        }
        // 复制 + sha256 逐文件校验（fail-soft：失败保留已复制项继续）。
        match std::fs::copy(source, &target) {
            Ok(_) => {
                let same = match (
                    crate::snapshot_store::sha256_file(source),
                    crate::snapshot_store::sha256_file(&target),
                ) {
                    (Some(origin_hash), Some(target_hash)) => origin_hash == target_hash,
                    _ => false,
                };
                if same {
                    outcome.migrated_files += 1;
                } else {
                    // 校验不一致：清掉刚复制的残件（非用户数据），记失败。
                    let _ = fs::remove_file(&target);
                    outcome.failures.push(BackupMigrationFailure {
                        file_name: relative.clone(),
                        reason: "复制后内容校验不一致，目标未保留该文件".to_string(),
                    });
                }
            }
            Err(_) => outcome.failures.push(BackupMigrationFailure {
                file_name: relative.clone(),
                reason: "复制失败（磁盘或权限问题），目标未产生该文件".to_string(),
            }),
        }
    }
    Ok(outcome)
}

/// 递归收集目录内的常规文件到迁移清单（relative 为备份目录内相对显示名）。
/// 目录不存在视为无凭据材料（直接返回）；符号链接等非常规对象拒绝迁移
/// （fail-closed：不猜测内容）。
fn collect_dir_files(
    dir: &Path,
    relative: &str,
    plan: &mut Vec<(PathBuf, String)>,
) -> Result<(), BackupRetentionError> {
    let metadata = match fs::symlink_metadata(dir) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(_) => return Err(BackupRetentionError::Io),
    };
    if !metadata.is_dir() {
        return Err(BackupRetentionError::Io);
    }
    let entries = fs::read_dir(dir).map_err(|_| BackupRetentionError::Io)?;
    for entry in entries {
        let entry = entry.map_err(|_| BackupRetentionError::Io)?;
        let child_meta = entry.metadata().map_err(|_| BackupRetentionError::Io)?;
        let child_relative = format!("{relative}/{}", entry.file_name().to_string_lossy());
        if child_meta.is_dir() {
            collect_dir_files(&entry.path(), &child_relative, plan)?;
        } else if child_meta.is_file() {
            plan.push((entry.path(), child_relative));
        } else {
            return Err(BackupRetentionError::Io);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn settings(enabled: bool, keep: u32) -> BackupRetentionSettings {
        BackupRetentionSettings {
            enabled,
            keep,
            backup_dir: None,
        }
    }

    fn write_backup(dir: &Path, db_name: &str, stamp: u64, with_wal: bool) {
        std::fs::write(
            dir.join(format!("{db_name}.switch-bak-{stamp}")),
            [1u8; 100],
        )
        .unwrap();
        if with_wal {
            std::fs::write(
                dir.join(format!("{db_name}.switch-bak-{stamp}-wal")),
                [1u8; 20],
            )
            .unwrap();
            std::fs::write(
                dir.join(format!("{db_name}.switch-bak-{stamp}-shm")),
                [1u8; 5],
            )
            .unwrap();
        }
    }

    #[test]
    fn load_missing_file_returns_default() {
        let root = tempdir().unwrap();
        let store = BackupRetentionStore::new(root.path());
        assert_eq!(store.load().unwrap(), settings(true, 5));
    }

    #[test]
    fn save_and_reload_roundtrip() {
        let root = tempdir().unwrap();
        let store = BackupRetentionStore::new(root.path());
        store.save(&settings(false, 12)).unwrap();
        assert_eq!(store.load().unwrap(), settings(false, 12));
        // 跨实例持久。
        assert_eq!(
            BackupRetentionStore::new(root.path()).load().unwrap(),
            settings(false, 12)
        );
    }

    #[test]
    fn invalid_keep_rejected() {
        let root = tempdir().unwrap();
        let store = BackupRetentionStore::new(root.path());
        assert_eq!(
            store.save(&settings(true, 0)),
            Err(BackupRetentionError::Invalid)
        );
        assert_eq!(
            store.save(&settings(true, MAX_KEEP + 1)),
            Err(BackupRetentionError::Invalid)
        );
        // 越界配置文件加载拒绝（不静默采用）。
        std::fs::write(
            root.path().join(STORE_FILE),
            serde_json::json!({"format_version": 1, "settings": {"enabled": true, "keep": 99}})
                .to_string(),
        )
        .unwrap();
        assert_eq!(
            store.load().unwrap_err(),
            BackupRetentionError::Invalid
        );
    }

    #[test]
    fn corrupt_file_rejected_not_rebuilt() {
        let root = tempdir().unwrap();
        let store = BackupRetentionStore::new(root.path());
        std::fs::write(root.path().join(STORE_FILE), "not json").unwrap();
        assert_eq!(store.load().unwrap_err(), BackupRetentionError::Invalid);
        // 保存被拒而非覆盖重建（证据保留纪律）。
        assert_eq!(
            store.save(&settings(true, 5)),
            Err(BackupRetentionError::Invalid)
        );
        assert_eq!(
            std::fs::read_to_string(root.path().join(STORE_FILE)).unwrap(),
            "not json"
        );
    }

    #[test]
    fn prune_keeps_newest_and_removes_older_with_sides() {
        let dir = tempdir().unwrap();
        // 3 份备份：1000（带 wal/shm）、900、800。
        write_backup(dir.path(), "database.db", 1000, true);
        write_backup(dir.path(), "database.db", 900, false);
        write_backup(dir.path(), "database.db", 800, false);
        let removed = prune_master_backups(dir.path(), "database.db", 2);
        assert_eq!(removed, 1);
        // 最新两份在位（1000 含附属件），最旧的 800 连主文件一起删除。
        assert!(dir
            .path()
            .join("database.db.switch-bak-1000")
            .is_file());
        assert!(dir
            .path()
            .join("database.db.switch-bak-1000-wal")
            .is_file());
        assert!(dir
            .path()
            .join("database.db.switch-bak-1000-shm")
            .is_file());
        assert!(dir.path().join("database.db.switch-bak-900").is_file());
        assert!(!dir
            .path()
            .join("database.db.switch-bak-800")
            .is_file());
    }

    #[test]
    fn prune_keep_zero_is_noop() {
        let dir = tempdir().unwrap();
        write_backup(dir.path(), "database.db", 1000, false);
        write_backup(dir.path(), "database.db", 900, false);
        assert_eq!(prune_master_backups(dir.path(), "database.db", 0), 0);
        // 调用方语义：keep=0（禁用）时不删任何备份。
        assert!(dir.path().join("database.db.switch-bak-900").is_file());
    }

    #[test]
    fn prune_within_capacity_removes_nothing() {
        let dir = tempdir().unwrap();
        write_backup(dir.path(), "database.db", 1000, false);
        write_backup(dir.path(), "database.db", 900, false);
        assert_eq!(prune_master_backups(dir.path(), "database.db", 5), 0);
        assert!(dir.path().join("database.db.switch-bak-1000").is_file());
        assert!(dir.path().join("database.db.switch-bak-900").is_file());
    }

    #[test]
    fn prune_ignores_misnamed_files() {
        let dir = tempdir().unwrap();
        write_backup(dir.path(), "database.db", 1000, false);
        // 命名异常散件：非备份链成员，裁剪永不触碰（失败证据不清）。
        std::fs::write(dir.path().join("database.db.switch-bak-abc"), [1u8; 7]).unwrap();
        std::fs::write(dir.path().join("unrelated.txt"), [1u8; 3]).unwrap();
        assert_eq!(prune_master_backups(dir.path(), "database.db", 1), 0);
        assert!(dir
            .path()
            .join("database.db.switch-bak-abc")
            .is_file());
        assert!(dir.path().join("unrelated.txt").is_file());
    }

    // ===== R0-P3 凭据备份子目录纳入保留裁剪（同 stamp 同进同退）=====

    /// 测试辅助：在备份目录写一个 credentials-{stamp} 凭据子目录（含一个文件）。
    fn write_credentials_dir(dir: &Path, stamp: u64) {
        let cred = dir.join(format!("credentials-{stamp}"));
        std::fs::create_dir_all(&cred).unwrap();
        std::fs::write(cred.join("accounts.json"), b"{}").unwrap();
    }

    /// 裁剪主备份时，同 stamp 的凭据子目录连带删除；保留份的凭据子目录不动。
    #[test]
    fn prune_removes_credentials_dir_with_same_stamp() {
        let dir = tempdir().unwrap();
        // 3 份主备份，各自带同 stamp 凭据子目录。
        write_backup(dir.path(), "database.db", 1000, false);
        write_backup(dir.path(), "database.db", 900, false);
        write_backup(dir.path(), "database.db", 800, false);
        write_credentials_dir(dir.path(), 1000);
        write_credentials_dir(dir.path(), 900);
        write_credentials_dir(dir.path(), 800);
        let removed = prune_master_backups(dir.path(), "database.db", 2);
        assert_eq!(removed, 1);
        // 最旧的 800 主备份连带同 stamp 凭据子目录一起删除。
        assert!(!dir.path().join("database.db.switch-bak-800").is_file());
        assert!(!dir.path().join("credentials-800").exists());
        // 保留份（1000/900）的凭据子目录原样保留。
        assert!(dir.path().join("credentials-1000").is_dir());
        assert!(dir.path().join("credentials-900").is_dir());
    }

    /// keep 计数完全以主备份链为准：凭据子目录不参与链计数（不顶替保留份数）。
    #[test]
    fn prune_keep_count_ignores_credentials_dirs() {
        let dir = tempdir().unwrap();
        // 2 份主备份各带凭据子目录，另有 1 个无主备份的凭据目录。
        write_backup(dir.path(), "database.db", 1000, false);
        write_backup(dir.path(), "database.db", 900, false);
        write_credentials_dir(dir.path(), 1000);
        write_credentials_dir(dir.path(), 900);
        write_credentials_dir(dir.path(), 700);
        // 链只数主备份：2 份主备份 keep=2 → 一份不删。
        assert_eq!(prune_master_backups(dir.path(), "database.db", 2), 0);
        assert!(dir.path().join("database.db.switch-bak-1000").is_file());
        assert!(dir.path().join("database.db.switch-bak-900").is_file());
        assert!(dir.path().join("credentials-700").is_dir());
    }

    /// 孤儿凭据子目录（对应主备份已被手动删除）不自动清理
    /// （铁律：不自动删证据），仅在恢复指引提示用户手动处置。
    #[test]
    fn prune_leaves_orphan_credentials_dir() {
        let dir = tempdir().unwrap();
        write_backup(dir.path(), "database.db", 1000, false);
        write_backup(dir.path(), "database.db", 900, false);
        // 孤儿：credentials-800 存在但 800 主备份已不在（枚举不会看到它）。
        write_credentials_dir(dir.path(), 800);
        let removed = prune_master_backups(dir.path(), "database.db", 1);
        assert_eq!(removed, 1);
        // 900 被裁剪但没有凭据子目录可删；孤儿的 800 凭据目录保留。
        assert!(!dir.path().join("database.db.switch-bak-900").is_file());
        assert!(!dir.path().join("credentials-900").exists());
        assert!(dir.path().join("credentials-800").is_dir());
    }

    // ===== P11-4① 备份位置可选：配置兼容 / override / 跨目录裁剪隔离 / 迁移 =====

    /// 旧格式配置文件（无 backup_dir 字段）兼容：读取按 None（默认位置）。
    #[test]
    fn legacy_config_without_backup_dir_loads_as_none() {
        let root = tempdir().unwrap();
        std::fs::write(
            root.path().join(STORE_FILE),
            serde_json::json!({
                "format_version": 1,
                "settings": {"enabled": true, "keep": 5}
            })
            .to_string(),
        )
        .unwrap();
        let store = BackupRetentionStore::new(root.path());
        assert_eq!(
            store.load().unwrap(),
            BackupRetentionSettings {
                enabled: true,
                keep: 5,
                backup_dir: None
            }
        );
    }

    /// backup_dir 字段写读回环；非法值（相对路径）load/save 双向拒绝
    /// （fail-closed，不静默采用）。
    #[test]
    fn backup_dir_roundtrip_and_invalid_rejected() {
        let root = tempdir().unwrap();
        let store = BackupRetentionStore::new(root.path());
        let custom = root.path().join("custom-backups");
        store
            .save(&BackupRetentionSettings {
                enabled: true,
                keep: 3,
                backup_dir: Some(custom.display().to_string()),
            })
            .unwrap();
        assert_eq!(
            BackupRetentionStore::new(root.path())
                .load()
                .unwrap()
                .backup_dir
                .as_deref(),
            Some(custom.display().to_string().as_str())
        );
        // 相对路径：save 拒绝；文件内出现同样 load 拒绝。
        assert_eq!(
            store.save(&BackupRetentionSettings {
                enabled: true,
                keep: 3,
                backup_dir: Some("relative/dir".to_string()),
            }),
            Err(BackupRetentionError::Invalid)
        );
        std::fs::write(
            root.path().join(STORE_FILE),
            serde_json::json!({
                "format_version": 1,
                "settings": {"enabled": true, "keep": 5, "backup_dir": "relative/dir"}
            })
            .to_string(),
        )
        .unwrap();
        assert_eq!(
            BackupRetentionStore::new(root.path()).load().unwrap_err(),
            BackupRetentionError::Invalid
        );
    }

    /// override 读取（fail-soft）：未配置 None；配置 Some；损坏回退 None。
    #[test]
    fn load_backup_dir_override_fallbacks() {
        let root = tempdir().unwrap();
        // 未配置（文件不存在）：None = 主库同目录行为。
        assert_eq!(load_backup_dir_override(root.path()), None);
        // 配置了 override：返回目录。
        let custom = root.path().join("custom");
        BackupRetentionStore::new(root.path())
            .save(&BackupRetentionSettings {
                enabled: true,
                keep: 5,
                backup_dir: Some(custom.display().to_string()),
            })
            .unwrap();
        assert_eq!(
            load_backup_dir_override(root.path()),
            Some(custom).map(PathBuf::from)
        );
        // 配置损坏：fail-soft 回退 None（不阻断备份/切号主流程）。
        std::fs::write(root.path().join(STORE_FILE), "not json").unwrap();
        assert_eq!(load_backup_dir_override(root.path()), None);
    }

    /// 裁剪只动当前活动目录的链；位置变更前的旧目录链完整保留
    /// （铁律：工具不自动删用户快照，旧位置由用户手动处置）。
    #[test]
    fn prune_only_touches_current_dir_chain() {
        let root = tempdir().unwrap();
        let current = root.path().join("current");
        let legacy = root.path().join("legacy");
        std::fs::create_dir_all(&current).unwrap();
        std::fs::create_dir_all(&legacy).unwrap();
        write_backup(&current, "database.db", 1000, false);
        write_backup(&current, "database.db", 900, false);
        write_backup(&current, "database.db", 800, false);
        write_backup(&legacy, "database.db", 500, false);
        write_backup(&legacy, "database.db", 400, false);
        let removed = prune_master_backups(&current, "database.db", 2);
        assert_eq!(removed, 1);
        // 当前目录：最旧的 800 被清理。
        assert!(!current.join("database.db.switch-bak-800").is_file());
        assert!(current.join("database.db.switch-bak-1000").is_file());
        // 旧目录链不碰。
        assert!(legacy.join("database.db.switch-bak-500").is_file());
        assert!(legacy.join("database.db.switch-bak-400").is_file());
    }

    /// 迁移：链三件套 + 凭据子目录复制到目标并逐文件校验；命名异常散件
    /// 与无关文件不迁；原位保留不删（铁律）。
    #[test]
    fn migrate_copies_chain_and_credentials_verified() {
        let root = tempdir().unwrap();
        let source = root.path().join("source");
        let target = root.path().join("target");
        std::fs::create_dir_all(&source).unwrap();
        write_backup(&source, "database.db", 1000, true);
        write_backup(&source, "database.db", 900, false);
        // 凭据子目录（含子层结构）。
        let cred = source.join("credentials-1000");
        std::fs::create_dir_all(cred.join("profiles")).unwrap();
        std::fs::write(cred.join("accounts.json"), b"{}").unwrap();
        std::fs::write(cred.join("profiles").join("a.checkin"), b"TRVCKN01-cipher").unwrap();
        // 命名异常散件 + 无关文件：不属备份链，永不触碰。
        std::fs::write(source.join("database.db.switch-bak-abc"), b"x").unwrap();
        std::fs::write(source.join("unrelated.txt"), b"y").unwrap();

        let outcome = migrate_master_backups(&source, &target, "database.db").unwrap();
        // 1000 三件套 3 文件 + 900 主文件 1 + 凭据 2 = 6。
        assert_eq!(outcome.migrated_files, 6);
        assert!(outcome.failures.is_empty());
        // 目标逐文件字节一致。
        assert_eq!(
            std::fs::read(target.join("database.db.switch-bak-1000")).unwrap(),
            [1u8; 100]
        );
        assert_eq!(
            std::fs::read(target.join("database.db.switch-bak-1000-wal")).unwrap(),
            [1u8; 20]
        );
        assert_eq!(
            std::fs::read(target.join("database.db.switch-bak-900")).unwrap(),
            [1u8; 100]
        );
        assert_eq!(
            std::fs::read(target.join("credentials-1000").join("profiles").join("a.checkin"))
                .unwrap(),
            b"TRVCKN01-cipher"
        );
        // 散件与无关文件不迁移。
        assert!(!target.join("database.db.switch-bak-abc").exists());
        assert!(!target.join("unrelated.txt").exists());
        // 原位保留不删（铁律：不自动删快照）。
        assert!(source.join("database.db.switch-bak-1000").is_file());
        assert!(source.join("database.db.switch-bak-900").is_file());
        assert!(cred.join("profiles").join("a.checkin").is_file());
    }

    /// 迁移冲突：目标同名文件内容不一致 → 记失败、不覆盖、源保留。
    #[test]
    fn migrate_conflict_reports_failure_without_overwrite() {
        let root = tempdir().unwrap();
        let source = root.path().join("source");
        let target = root.path().join("target");
        std::fs::create_dir_all(&source).unwrap();
        std::fs::create_dir_all(&target).unwrap();
        write_backup(&source, "database.db", 1000, false);
        // 目标已有同名文件（内容不同）：永不覆盖。
        std::fs::write(target.join("database.db.switch-bak-1000"), [9u8; 100]).unwrap();

        let outcome = migrate_master_backups(&source, &target, "database.db").unwrap();
        assert_eq!(outcome.migrated_files, 0);
        assert_eq!(outcome.failures.len(), 1);
        assert_eq!(
            outcome.failures[0].file_name,
            "database.db.switch-bak-1000"
        );
        assert!(!outcome.failures[0].reason.is_empty());
        // 目标未被覆盖、源原位保留。
        assert_eq!(
            std::fs::read(target.join("database.db.switch-bak-1000")).unwrap(),
            [9u8; 100]
        );
        assert!(source.join("database.db.switch-bak-1000").is_file());
    }

    /// 迁移幂等：重复执行时目标已存在且内容一致 → 全部计入成功、无失败
    /// （半失败后的重试安全）。
    #[test]
    fn migrate_rerun_is_idempotent_when_contents_match() {
        let root = tempdir().unwrap();
        let source = root.path().join("source");
        let target = root.path().join("target");
        std::fs::create_dir_all(&source).unwrap();
        write_backup(&source, "database.db", 1000, false);
        let first = migrate_master_backups(&source, &target, "database.db").unwrap();
        assert_eq!(first.migrated_files, 1);
        assert!(first.failures.is_empty());
        let second = migrate_master_backups(&source, &target, "database.db").unwrap();
        assert_eq!(second.migrated_files, 1);
        assert!(second.failures.is_empty());
    }
}
