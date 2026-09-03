//! T08 进程观测端口：组合根只依赖结果，不依赖 Windows API。

use std::path::Path;

/// 进程观测结论。未知、歧义和错误目标均不得进入有副作用阶段。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessObservationStatus {
    NotRunning,
    Running,
    WrongTarget,
    Ambiguous,
    Unknown,
}

/// 可审计的进程身份摘要，不包含命令行、账号或认证正文。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessIdentity {
    pub pid: u32,
    pub creation_time_unix_ms: Option<u64>,
    pub executable_path: Option<String>,
    pub executable_identity: Option<String>,
}

/// 进程与目标数据位置之间的非敏感匹配证据。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessMatchEvidence {
    pub code: String,
    pub value: String,
}

/// 后端拥有的进程观测结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessObservation {
    pub status: ProcessObservationStatus,
    pub data_location_id: String,
    pub candidates: Vec<ProcessIdentity>,
    pub evidence: Vec<ProcessMatchEvidence>,
}

impl ProcessObservation {
    /// 观测结论可信（NotRunning/Running）即视为可扫描。
    ///
    /// R1 修订（U-6 W3，依据 `.scratch/history-u6/w0-report.md`：三件套快照
    /// 副本在 TRAE 运行中持续写入时 0/105 撕裂）：运行中读取走快照复制路径
    /// 已实证安全，Running 不再是扫描拒绝理由；观测不确定态（WrongTarget/
    /// Ambiguous/Unknown）仍失败关闭——观测本身不可信时不能进入读取。
    pub fn is_safe_to_scan(&self) -> bool {
        matches!(
            self.status,
            ProcessObservationStatus::NotRunning | ProcessObservationStatus::Running
        )
    }

    /// 返回稳定错误码，供组合根映射为用户可见错误。
    ///
    /// 注意：`Running` 仍映射为 `process_running`——写/计划等非快照读取链路
    /// （apply_sync_plan / build_sync_plan）继续以该错误码拒绝运行中操作；
    /// 快照读取链路应使用 `is_safe_to_scan()`（R1 修订后 Running 视为安全）。
    pub fn error_code(&self) -> Option<&'static str> {
        match self.status {
            ProcessObservationStatus::NotRunning => None,
            ProcessObservationStatus::Running => Some("process_running"),
            ProcessObservationStatus::WrongTarget => Some("wrong_target_process"),
            ProcessObservationStatus::Ambiguous => Some("process_target_ambiguous"),
            ProcessObservationStatus::Unknown => Some("process_state_unknown"),
        }
    }
}

/// 进程控制边界目前只开放观测，不开放关闭、强杀或启动。
pub trait ProcessControllerPort: Send + Sync {
    fn observe(
        &self,
        data_location_id: &str,
        data_root: &Path,
        db_relative_path: &str,
    ) -> ProcessObservation;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn not_running_and_running_observations_are_safe() {
        // R1 修订（W0 实测 0/105 撕裂）：Running 走快照路径安全，不再拒绝。
        let base = ProcessObservation {
            status: ProcessObservationStatus::NotRunning,
            data_location_id: "loc-one".to_string(),
            candidates: Vec::new(),
            evidence: Vec::new(),
        };
        assert!(base.is_safe_to_scan());
        assert_eq!(base.error_code(), None);

        let running = ProcessObservation {
            status: ProcessObservationStatus::Running,
            ..base.clone()
        };
        // 读取安全（快照路径），但写/计划链路仍以 error_code 拒绝。
        assert!(running.is_safe_to_scan());
        assert_eq!(running.error_code(), Some("process_running"));

        // 观测不确定态仍失败关闭。
        for status in [
            ProcessObservationStatus::WrongTarget,
            ProcessObservationStatus::Ambiguous,
            ProcessObservationStatus::Unknown,
        ] {
            let observation = ProcessObservation {
                status,
                ..base.clone()
            };
            assert!(!observation.is_safe_to_scan());
            assert!(observation.error_code().is_some());
        }
    }
}
