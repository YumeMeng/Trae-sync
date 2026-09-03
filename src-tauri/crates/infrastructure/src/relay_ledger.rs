//! 接力台账（P5-1，环境模型 Q3/Q5）。
//!
//! App 自有的会话接力记录：每次主库切号换腿时为每个被交接的会话追加一条
//! 记录（新腿 session_id、前后账号、交接时消息数、时间）。历史页（P5-3）
//! 据此还原每个会话的接力轨迹徽章：相邻两条记录的消息数差 = 该腿期间新增
//! 消息数，首条记录之前的消息归属于 `from_user_id` 对应账号。
//!
//! 台账独立于 TRAE 数据库（`{storage_root}\environments\relay-ledger.json`），
//! 只追加不删除（证据保留铁律）；损坏文件报错不静默重建。

use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::atomic_publish::publish_replacing;

const LEDGER_FILE: &str = "relay-ledger.json";
const MAX_LEDGER_BYTES: u64 = 16 * 1024 * 1024;
/// 台账条目上限：切号次数 × 每次活跃会话数的长期上界（防异常膨胀）。
const MAX_LEDGER_ENTRIES: usize = 200_000;
const MAX_ID_FIELD_BYTES: usize = 256;

/// 台账读写失败（非敏感）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RelayLedgerError {
    /// 文件损坏或格式不符（不静默覆盖，报错交人工处置）。
    Invalid,
    /// 追加条目非法（字段为空/超长、超出条目上限）。
    InvalidEntry,
    /// 读写失败。
    Io,
}

/// 一条接力记录 = 一次切号中一个会话的换腿。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RelayLedgerEntry {
    /// 新腿的本地 session_id（换腿后身份；历史页按会话聚合轨迹）。
    pub session_id: String,
    /// 换腿前的旧 session_id（P5-3 轨迹链回：旧值 → 新值逐跳衔接成完整
    /// 接力链）。P5-1 早期写入的条目无此字段（serde default 兼容旧文件，
    /// 链回在缺失处自然断链，只显示最后一段接力）。
    #[serde(default)]
    pub from_session_id: Option<String>,
    /// 会话所属 project_id（换腿后）。
    pub project_id: String,
    /// 交接前账号的 TRAE user_id（该腿期间的会话主人）。
    pub from_user_id: String,
    /// 接收账号的 TRAE user_id（新腿主人）。
    pub to_user_id: String,
    /// 接收账号的 profile_id（App 账号注册表身份，展示层反查昵称）。
    pub to_profile_id: String,
    /// 交接时会话累计消息数（chat_message 行数；相邻条目差值 = 该腿新增）。
    pub message_count_at_switch: i64,
    pub switched_at_unix_seconds: u64,
}

impl RelayLedgerEntry {
    /// 校验字段：ID 字段非空且不超长；时间戳非零。
    pub fn validate(&self) -> Result<(), RelayLedgerError> {
        if let Some(from_session) = &self.from_session_id {
            if from_session.is_empty() || from_session.len() > MAX_ID_FIELD_BYTES {
                return Err(RelayLedgerError::InvalidEntry);
            }
        }
        for field in [
            &self.session_id,
            &self.project_id,
            &self.from_user_id,
            &self.to_user_id,
            &self.to_profile_id,
        ] {
            if field.is_empty() || field.len() > MAX_ID_FIELD_BYTES {
                return Err(RelayLedgerError::InvalidEntry);
            }
        }
        if self.message_count_at_switch < 0 || self.switched_at_unix_seconds == 0 {
            return Err(RelayLedgerError::InvalidEntry);
        }
        Ok(())
    }
}

#[derive(Serialize, Deserialize)]
struct LedgerFile {
    format_version: u32,
    entries: Vec<RelayLedgerEntry>,
}

/// 接力台账（storage_root 的 environments 目录下单文件 JSON）。
pub struct RelayLedger {
    path: PathBuf,
}

impl RelayLedger {
    /// 台账文件路径：`{root}\relay-ledger.json`（调用方传 environments 目录）。
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            path: root.into().join(LEDGER_FILE),
        }
    }

    /// 读取全部记录；文件不存在返回空（首次切号前无需预创建）。
    pub fn load(&self) -> Result<Vec<RelayLedgerEntry>, RelayLedgerError> {
        let metadata = match fs::symlink_metadata(&self.path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Vec::new());
            }
            Err(_) => return Err(RelayLedgerError::Io),
        };
        if !metadata.is_file() || metadata.len() > MAX_LEDGER_BYTES {
            return Err(RelayLedgerError::Invalid);
        }
        let content = fs::read_to_string(&self.path).map_err(|_| RelayLedgerError::Io)?;
        let ledger: LedgerFile =
            serde_json::from_str(&content).map_err(|_| RelayLedgerError::Invalid)?;
        if ledger.format_version != 1 {
            return Err(RelayLedgerError::Invalid);
        }
        if ledger.entries.len() > MAX_LEDGER_ENTRIES {
            return Err(RelayLedgerError::Invalid);
        }
        for entry in &ledger.entries {
            entry.validate()?;
        }
        Ok(ledger.entries)
    }

    /// 追加记录（读取 → 校验 → 合并 → 原子写回）。
    /// 事务性由调用方保证：换腿事务提交成功后才追加台账；追加失败时报错
    /// 但不影响已完成的数据库交接（台账缺失只影响轨迹展示，可从备份核对）。
    pub fn append(&self, new_entries: &[RelayLedgerEntry]) -> Result<(), RelayLedgerError> {
        if new_entries.is_empty() {
            return Ok(());
        }
        for entry in new_entries {
            entry.validate()?;
        }
        let mut entries = self.load()?;
        if entries.len() + new_entries.len() > MAX_LEDGER_ENTRIES {
            return Err(RelayLedgerError::InvalidEntry);
        }
        entries.extend_from_slice(new_entries);
        self.write(&entries)
    }

    /// 原子写入：失败时保留原文件。
    fn write(&self, entries: &[RelayLedgerEntry]) -> Result<(), RelayLedgerError> {
        let ledger = LedgerFile {
            format_version: 1,
            entries: entries.to_vec(),
        };
        let content =
            serde_json::to_string(&ledger).map_err(|_| RelayLedgerError::Invalid)?;
        if content.len() as u64 > MAX_LEDGER_BYTES {
            return Err(RelayLedgerError::Invalid);
        }
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent).map_err(|_| RelayLedgerError::Io)?;
        }
        let temporary = self.path.with_extension("json.tmp");
        fs::write(&temporary, content).map_err(|_| RelayLedgerError::Io)?;
        publish_replacing(&temporary, &self.path).map_err(|_| RelayLedgerError::Io)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn sample_entry(session_id: &str, to_user: &str, count: i64) -> RelayLedgerEntry {
        RelayLedgerEntry {
            session_id: session_id.to_string(),
            from_session_id: None,
            project_id: "proj-1".to_string(),
            from_user_id: "111".to_string(),
            to_user_id: to_user.to_string(),
            to_profile_id: "checkin-abc".to_string(),
            message_count_at_switch: count,
            switched_at_unix_seconds: 1_800_000_000,
        }
    }

    #[test]
    fn load_missing_file_returns_empty() {
        let root = tempdir().unwrap();
        assert!(RelayLedger::new(root.path()).load().unwrap().is_empty());
    }

    #[test]
    fn append_persists_and_accumulates() {
        let root = tempdir().unwrap();
        let ledger = RelayLedger::new(root.path());
        ledger.append(&[sample_entry("s1", "222", 10)]).unwrap();
        ledger.append(&[sample_entry("s2", "333", 20), sample_entry("s3", "333", 5)])
            .unwrap();
        let entries = ledger.load().unwrap();
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].session_id, "s1");
        assert_eq!(entries[2].to_user_id, "333");
        // 跨实例恢复（App 重启后台账不丢）。
        assert_eq!(RelayLedger::new(root.path()).load().unwrap().len(), 3);
    }

    #[test]
    fn append_empty_is_noop_without_creating_file() {
        let root = tempdir().unwrap();
        let ledger = RelayLedger::new(root.path());
        ledger.append(&[]).unwrap();
        assert!(!root.path().join(LEDGER_FILE).exists());
    }

    #[test]
    fn invalid_entries_rejected() {
        let root = tempdir().unwrap();
        let ledger = RelayLedger::new(root.path());
        let mut entry = sample_entry("s1", "222", 1);
        entry.session_id = String::new();
        assert_eq!(
            ledger.append(&[entry]),
            Err(RelayLedgerError::InvalidEntry)
        );
        let mut zero_time = sample_entry("s1", "222", 1);
        zero_time.switched_at_unix_seconds = 0;
        assert_eq!(
            ledger.append(&[zero_time]),
            Err(RelayLedgerError::InvalidEntry)
        );
        // 未写入任何内容。
        assert!(ledger.load().unwrap().is_empty());
    }

    #[test]
    fn corrupt_file_rejected_not_silently_rebuilt() {
        let root = tempdir().unwrap();
        let ledger = RelayLedger::new(root.path());
        std::fs::write(root.path().join(LEDGER_FILE), "not json").unwrap();
        assert_eq!(ledger.load().unwrap_err(), RelayLedgerError::Invalid);
        // 追加在损坏文件上报错而非覆盖重建（证据保留）。
        assert_eq!(
            ledger.append(&[sample_entry("s1", "222", 1)]),
            Err(RelayLedgerError::Invalid)
        );
        assert_eq!(
            std::fs::read_to_string(root.path().join(LEDGER_FILE)).unwrap(),
            "not json"
        );
    }

    #[test]
    fn from_session_id_roundtrip_and_legacy_file_compatible() {
        // P5-3 轨迹链回：from_session_id 持久化往返。
        let root = tempdir().unwrap();
        let ledger = RelayLedger::new(root.path());
        let mut entry = sample_entry("s2", "222", 10);
        entry.from_session_id = Some("s1".to_string());
        ledger.append(&[entry]).unwrap();
        let loaded = ledger.load().unwrap();
        assert_eq!(loaded[0].from_session_id.as_deref(), Some("s1"));

        // P5-1 早期格式（无 from_session_id 字段）反序列化为 None，不报错。
        let legacy = serde_json::json!({
            "format_version": 1,
            "entries": [{
                "session_id": "s9",
                "project_id": "proj-1",
                "from_user_id": "111",
                "to_user_id": "222",
                "to_profile_id": "checkin-abc",
                "message_count_at_switch": 3,
                "switched_at_unix_seconds": 1_800_000_000
            }]
        });
        std::fs::write(
            root.path().join(LEDGER_FILE),
            serde_json::to_string(&legacy).unwrap(),
        )
        .unwrap();
        let loaded = ledger.load().unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].from_session_id, None);

        // 空串 from_session_id 视为非法（缺失与空串语义区分）。
        let mut bad = sample_entry("s3", "222", 1);
        bad.from_session_id = Some(String::new());
        assert_eq!(
            ledger.append(&[bad]),
            Err(RelayLedgerError::InvalidEntry)
        );
    }
}
