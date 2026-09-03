//! 备份保留策略：定次自动裁剪（P5-9，2026-09-02 grill 定案）。
//!
//! 每次新备份生成后，删除超出保留数的旧备份（`.switch-bak-{时间戳}`
//! 主文件 + wal/shm 附属件连带删）。保留数默认 5、可调（1-50）、可关闭；
//! 关闭时完全不自动删除（回退为永不自动删）。裁剪失败静默跳过
//! （下次备份生成时再试），不阻断备份与切号主流程。
//!
//! 文件布局：`{storage_root}/backup-retention.json`；损坏时报错不静默重建
//! （与台账/档案同纪律——配置文件是用户决策的证据）。

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::atomic_publish::publish_replacing;
use crate::master_handover::list_master_backups;

const STORE_FILE: &str = "backup-retention.json";
const MAX_STORE_BYTES: u64 = 64 * 1024;
/// 保留数下限（至少留 1 份，防止把备份链清空）。
pub const MIN_KEEP: u32 = 1;
/// 保留数上限（防误输入超大值；grill 定案范围 1-50）。
pub const MAX_KEEP: u32 = 50;

/// 备份保留设置存储错误；不携带敏感内容。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackupRetentionError {
    /// 文件损坏、格式不符或保留数超范围。
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
}

impl Default for BackupRetentionSettings {
    fn default() -> Self {
        // grill 定案默认值：开 + 保留最近 5 份。
        Self {
            enabled: true,
            keep: 5,
        }
    }
}

/// 设置合法性：保留数在 [MIN_KEEP, MAX_KEEP] 范围内。
pub fn is_valid_keep(keep: u32) -> bool {
    (MIN_KEEP..=MAX_KEEP).contains(&keep)
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
        if store.format_version != 1 || !is_valid_keep(store.settings.keep) {
            return Err(BackupRetentionError::Invalid);
        }
        Ok(store.settings)
    }

    /// 保存设置（原子写：临时文件 + 原子替换，失败保留原文件）。
    ///
    /// 现有文件损坏时拒绝保存——不覆盖重建以保留用户决策证据
    /// （与台账/档案同纪律）；文件不存在视为首次使用，正常写入。
    pub fn save(&self, settings: &BackupRetentionSettings) -> Result<(), BackupRetentionError> {
        if !is_valid_keep(settings.keep) {
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

/// 裁剪备份链：保留最新 `keep` 份，更旧的删除（附属件连带删）。
///
/// 返回删除的备份数。删除失败静默跳过（该份下次再试，不影响其余）；
/// `keep` 为 0 时不删任何东西（防御：调用方语义上应先确认启用）。
/// 只按 `list_master_backups` 的枚举结果删——命名异常的散件不属备份链，
/// 永不触碰（铁律：失败证据不清）。
pub fn prune_master_backups(db_path: &Path, keep: u32) -> usize {
    if keep == 0 {
        return 0;
    }
    let chain = list_master_backups(db_path);
    // chain 已按时间戳倒序（最新在前）；跳过前 keep 份，其余逐份删除。
    let Some(dir) = db_path.parent() else {
        return 0;
    };
    let Some(db_name) = db_path.file_name().and_then(|name| name.to_str()) else {
        return 0;
    };
    let mut removed = 0;
    for entry in chain.iter().skip(keep as usize) {
        let base = format!("{db_name}.switch-bak-{}", entry.stamp_unix_seconds);
        let mut removed_this = true;
        for name in [&base, &format!("{base}-wal"), &format!("{base}-shm")] {
            // 删除失败不中断整体裁剪；该份下次备份生成时再试。
            if fs::remove_file(dir.join(name)).is_err() {
                // 仅当文件不存在视为已删；其他失败标记该份未完全清理。
                if dir.join(name).exists() {
                    removed_this = false;
                }
            }
        }
        if removed_this {
            removed += 1;
        }
    }
    removed
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn settings(enabled: bool, keep: u32) -> BackupRetentionSettings {
        BackupRetentionSettings { enabled, keep }
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

    fn db_path(dir: &Path) -> PathBuf {
        dir.join("database.db")
    }

    #[test]
    fn load_missing_file_returns_default() {
        let root = tempdir().unwrap();
        let store = BackupRetentionStore::new(root.path());
        assert_eq!(
            store.load().unwrap(),
            BackupRetentionSettings {
                enabled: true,
                keep: 5
            }
        );
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
        let removed = prune_master_backups(&db_path(dir.path()), 2);
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
        assert_eq!(prune_master_backups(&db_path(dir.path()), 0), 0);
        // 调用方语义：keep=0（禁用）时不删任何备份。
        assert!(dir.path().join("database.db.switch-bak-900").is_file());
    }

    #[test]
    fn prune_within_capacity_removes_nothing() {
        let dir = tempdir().unwrap();
        write_backup(dir.path(), "database.db", 1000, false);
        write_backup(dir.path(), "database.db", 900, false);
        assert_eq!(prune_master_backups(&db_path(dir.path()), 5), 0);
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
        assert_eq!(prune_master_backups(&db_path(dir.path()), 1), 0);
        assert!(dir
            .path()
            .join("database.db.switch-bak-abc")
            .is_file());
        assert!(dir.path().join("unrelated.txt").is_file());
    }
}
