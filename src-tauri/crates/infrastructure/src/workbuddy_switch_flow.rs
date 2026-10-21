//! P19.1：WorkBuddy 切号自动过户编排（对齐 Trae 切号语义——用户指摘
//! 「转移历史会话手动按钮与 Trae 逻辑不符，应切号时无感完成」）。
//!
//! 编排（核心注入式，离线单测状态机；生产装配在组合根命令层）：
//!
//! ```text
//! 1. 读切号前 .info uid（= previous；过户方向 previous→target）
//! 2. run_switch_experiment（B1 实验路径：写 .info 目标凭据
//!    〔ToolEnvelope，AES-256-GCM 信封——明文 token 不落盘〕→ 前置进程
//!    纪律 observe/close；冷启动延后到过户后统一执行）
//! 3. 切号验证：.info 当前账号 == 目标（文件面；任务书口径）
//! 4. 自动过户：**显式方向** previous→target（不走 resolve_account_direction
//!    ——sessions fallback 在切号场景可能把 current 判成旧账号【旧账号
//!    会话最近活跃】，方向反了即灾难；切号编排自身持有权威方向）
//!    → 完全退出 → transfer_sessions（含软删行、cloud 红线）→ 冷启动
//! 5. **过户失败不回滚切号**（会话归属可后补，切号本身优先成功）——
//!    失败如实报因在回执 transfer_error
//! ```
//!
//! 轮换副作用提示（transfer 无关，切号写凭据时同步生效）：工具加密
//! 信封写入的 access/refresh 对取自账号池；daemon 持有的旧对在客户端
//! 侧自然失效——客户端如提示重新登录，属切号预期行为。

use std::path::Path;

use crate::workbuddy_switch_experiment::{ExperimentError, ExperimentReceipt};
use crate::workbuddy_transfer::{ClientProcessState, TransferError, TransferReceipt};

/// 过户执行结果（编排内核口径：计划为零转 Nothing，否则执行）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransferFlowResult {
    /// 事务过户完成（含软删行；cloud 残留只报告不触碰）。
    Executed(TransferReceipt),
    /// 待过户为 0——跳过关应用与写库（与 B1「无需关应用」分支一致）。
    Nothing { current_sessions: u64 },
}

/// 切号编排错误（过户失败不在此列——见模块头「不回滚」纪律）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SwitchFlowError {
    /// 目标账号不在账号池（切号无从取凭据）。
    TargetNotInPool,
    /// 目标与切号前账号相同（防御，正常路径不可达——前端不给同项）。
    SameAccount,
    /// 切号实验失败（进程 Unknown / 关闭失败 / .info 缺失或损坏 / 写失败）。
    Experiment(ExperimentError),
    /// 切号验证不符：写凭据后 .info 当前账号 != 目标（防御——正常路径
    /// 快照即写入值；出现说明客户端并发改写，诚实拒绝进过户）。
    SwitchVerifyFailed,
    /// 过户前置进程检查失败（Unknown 拒绝一切写副作用——P14-2 边界）。
    TransferProcessStateUnknown,
    /// 过户前置关闭失败。
    TransferCloseFailed,
}

impl SwitchFlowError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::TargetNotInPool => "workbuddy_switch_target_not_in_pool",
            Self::SameAccount => "workbuddy_switch_same_account",
            Self::Experiment(error) => error.code(),
            Self::SwitchVerifyFailed => "workbuddy_switch_verify_failed",
            Self::TransferProcessStateUnknown => "wb_transfer_process_state_unknown",
            Self::TransferCloseFailed => "wb_transfer_close_failed",
        }
    }
}

/// 切号编排回执（组合根 DTO 的来源；switched=true 时过户结果看
/// transfer / transfer_error / transfer_nothing 三字段）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SwitchFlowOutcome {
    /// 切号是否成功（实验写入 + 验证通过）。
    pub switched: bool,
    /// 目标账号 uid。
    pub target_uid: String,
    /// 切号前账号 uid（过户方向起点；防御同账号时 None）。
    pub previous_uid: Option<String>,
    /// 切号实验回执（.info 备份路径/写回/冷启动标记）。
    pub experiment: Option<ExperimentReceipt>,
    /// 冷启动后的文件面登录态（best-effort 强验证；失败不否定切号）。
    pub post_status: Option<String>,
    /// 过户结果（切换成功且执行到过户时有值）。
    pub transfer: Option<TransferFlowResult>,
    /// 过户失败报因（稳定错误码；None = 过户成功或无需过户）。
    pub transfer_error: Option<String>,
    /// 过户后是否发起了冷启动（重启客户端读新凭据与新库态）。
    pub restarted: bool,
}

/// 编排依赖注入（全部离线可测；生产装配在组合根）。
pub struct SwitchFlowDeps<'a> {
    /// 读切号前 .info uid（快照解析；None/Err 由实现决定语义）。
    pub read_current_uid: &'a dyn Fn() -> Result<String, SwitchFlowError>,
    /// 切号实验（写 .info 目标凭据；**不含冷启动**——延后到过户后统一
    /// 执行，避免「关→写→起→再关→写库→再起」两次重启）。
    pub run_experiment: &'a dyn Fn() -> Result<ExperimentReceipt, ExperimentError>,
    /// 切号验证：读 .info 当前账号（快照 uid）。
    pub verify_switch: &'a dyn Fn() -> Result<String, SwitchFlowError>,
    /// 过户执行（显式方向 current/previous 由内核传入；实现内部
    /// build_transfer_plan → 零行转 Nothing → transfer_sessions）。
    pub run_transfer: &'a dyn Fn(&str, &str) -> Result<TransferFlowResult, TransferError>,
    /// 进程观察（过户前置；Unknown 拒绝写副作用）。
    pub observe: &'a dyn Fn() -> ClientProcessState,
    /// 关闭编排（优雅→强杀；退出确认由 observe 复核）。
    pub close: &'a dyn Fn() -> bool,
    /// 冷启动动作（None = 跳过，回执 restarted=false 如实报告）。
    pub start: Option<&'a dyn Fn() -> bool>,
    /// 冷启动后强验证（best-effort：.info 未被客户端改回/登出；失败仅
    /// 回执 post_status=None，不否定切号）。
    pub post_verify: Option<&'a dyn Fn() -> Option<String>>,
}

/// 切号 + 自动过户编排核心（状态机；生产装配见组合根）。
pub fn switch_with_auto_transfer(
    target_uid: &str,
    deps: &SwitchFlowDeps<'_>,
) -> Result<SwitchFlowOutcome, SwitchFlowError> {
    // 1) 切号前账号（过户方向起点）。
    let previous_uid = (deps.read_current_uid)()?;
    if previous_uid == target_uid {
        return Err(SwitchFlowError::SameAccount);
    }
    // 2) 切号实验（写 .info 目标凭据；前置进程纪律在实验内部）。
    let experiment = (deps.run_experiment)().map_err(SwitchFlowError::Experiment)?;
    // 3) 切号验证（任务书口径：.info 当前账号 == 目标）。
    let verified_uid = (deps.verify_switch)()?;
    if verified_uid != target_uid {
        return Err(SwitchFlowError::SwitchVerifyFailed);
    }
    // 4) 自动过户（显式方向；失败不回滚切号——失败转回执字段）。
    let mut outcome = SwitchFlowOutcome {
        switched: true,
        target_uid: target_uid.to_string(),
        previous_uid: Some(previous_uid.clone()),
        experiment: Some(experiment),
        post_status: None,
        transfer: None,
        transfer_error: None,
        restarted: false,
    };
    // 前置进程检查（Unknown 拒绝写副作用；Running 先关后复核）。
    enum PreCheck {
        Proceed,
        CloseFailed,
        ProcessUnknown,
    }
    let precheck = match (deps.observe)() {
        ClientProcessState::NotRunning => PreCheck::Proceed,
        ClientProcessState::Running => {
            if !(deps.close)() || (deps.observe)() != ClientProcessState::NotRunning {
                PreCheck::CloseFailed
            } else {
                PreCheck::Proceed
            }
        }
        ClientProcessState::Unknown => PreCheck::ProcessUnknown,
    };
    match precheck {
        PreCheck::Proceed => match (deps.run_transfer)(target_uid, &previous_uid) {
            Ok(result) => outcome.transfer = Some(result),
            Err(error) => outcome.transfer_error = Some(error.code().to_string()),
        },
        PreCheck::CloseFailed => {
            outcome.transfer_error =
                Some(SwitchFlowError::TransferCloseFailed.code().to_string());
        }
        PreCheck::ProcessUnknown => {
            outcome.transfer_error =
                Some(SwitchFlowError::TransferProcessStateUnknown.code().to_string());
        }
    }
    // 5) 冷启动（过户后统一一次；客户端起来读新凭据与新库态）。
    if let Some(start) = deps.start {
        outcome.restarted = start();
    }
    // 6) 冷启动后强验证（best-effort：失败不否定切号，如实回执）。
    if outcome.restarted {
        if let Some(post_verify) = deps.post_verify {
            outcome.post_status = post_verify();
        }
    }
    Ok(outcome)
}

/// 过户影响面预览（对齐 Trae preview_master_switch 形态）。
///
/// 方向口径与执行一致：previous = 切号前账号（当前 .info uid）、
/// current = 目标账号——预览「切到目标后有多少会话/项目将被转移」。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SwitchPreview {
    /// 切号前账号 uid（预览基准；label 由组合根映射昵称）。
    pub previous_uid: String,
    /// 目标账号 uid（None = 仅账号清单，未选定目标）。
    pub target_uid: Option<String>,
    /// 待过户 local 会话数（含软删行）。
    pub pending_local_rows: Option<u64>,
    /// 待过户会话涉及的项目数（cwd 维度 distinct）。
    pub pending_project_count: Option<u64>,
    /// 目标账号现有会话数。
    pub current_account_rows: Option<u64>,
    /// 切号前账号名下 cloud 会话数（红线：不碰；>0 UI 明示）。
    pub cloud_residual_rows: Option<u64>,
}

/// 影响面统计（只读；db 缺失/读取失败由调用方降级为 None——预览是
/// 辅助决策面，不阻塞切号入口）。
pub fn count_pending_projects(db: &Path, previous_uid: &str) -> Option<u64> {
    let conn = rusqlite::Connection::open_with_flags(
        db,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .ok()?;
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(DISTINCT cwd) FROM sessions \
             WHERE user_id = ?1 AND transport = 'local'",
            [previous_uid],
            |row| row.get(0),
        )
        .ok()?;
    Some(count.max(0) as u64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    /// 状态机：目标与前账号相同 → SameAccount（防御，不进实验）。
    #[test]
    fn same_account_is_rejected_before_experiment() {
        let experiment_calls = Cell::new(0u32);
        let deps = SwitchFlowDeps {
            read_current_uid: &|| Ok("uid-a".to_string()),
            run_experiment: &|| {
                experiment_calls.set(experiment_calls.get() + 1);
                unreachable!("同账号防御必须先于实验")
            },
            verify_switch: &|| unreachable!(),
            run_transfer: &|_, _| unreachable!(),
            observe: &|| unreachable!(),
            close: &|| unreachable!(),
            start: None,
            post_verify: None,
        };
        assert_eq!(
            switch_with_auto_transfer("uid-a", &deps).unwrap_err(),
            SwitchFlowError::SameAccount
        );
        assert_eq!(experiment_calls.get(), 0);
    }

    /// 切号验证不符：不进过户（防御态——客户端并发改写诚实拒绝）。
    #[test]
    fn verify_mismatch_blocks_transfer() {
        let transfer_calls = Cell::new(0u32);
        let deps = SwitchFlowDeps {
            read_current_uid: &|| Ok("uid-a".to_string()),
            run_experiment: &|| {
                Ok(ExperimentReceipt {
                    mode: crate::workbuddy_switch_experiment::WriteMode::ToolEnvelope,
                    backup_path: std::path::PathBuf::from("bak"),
                    wrote_info: true,
                    cold_started: false,
                    post_status: None,
                    post_uid: Some("uid-b".to_string()),
                })
            },
            verify_switch: &|| Ok("uid-a".to_string()), // 客户端并发改回
            run_transfer: &|_, _| {
                transfer_calls.set(transfer_calls.get() + 1);
                unreachable!("验证不符不得进过户")
            },
            observe: &|| unreachable!(),
            close: &|| unreachable!(),
            start: None,
            post_verify: None,
        };
        assert_eq!(
            switch_with_auto_transfer("uid-b", &deps).unwrap_err(),
            SwitchFlowError::SwitchVerifyFailed
        );
        assert_eq!(transfer_calls.get(), 0);
    }

    /// 快乐路径：实验 → 验证 → 关闭 → 过户 → 冷启动 → 强验证；回执
    /// switched=true + transfer=Executed + restarted=true。
    #[test]
    fn happy_path_switch_then_transfer_then_restart() {
        let close_calls = Cell::new(0u32);
        // 进程状态模拟：Running → close 后 NotRunning（observe 复核口径）。
        let running = Cell::new(true);
        let transferred = Cell::new(Option::<(String, String)>::None);
        let deps = SwitchFlowDeps {
            read_current_uid: &|| Ok("uid-a".to_string()),
            run_experiment: &|| {
                Ok(ExperimentReceipt {
                    mode: crate::workbuddy_switch_experiment::WriteMode::ToolEnvelope,
                    backup_path: std::path::PathBuf::from("bak"),
                    wrote_info: true,
                    cold_started: false,
                    post_status: None,
                    post_uid: Some("uid-b".to_string()),
                })
            },
            verify_switch: &|| Ok("uid-b".to_string()),
            run_transfer: &|current, previous| {
                transferred.set(Some((current.to_string(), previous.to_string())));
                Ok(TransferFlowResult::Executed(TransferReceipt {
                    updated_rows: 3,
                    cloud_residual_rows: 0,
                    verified_sessions: 3,
                    missing_body_files: 0,
                }))
            },
            observe: &|| {
                if running.get() {
                    ClientProcessState::Running
                } else {
                    ClientProcessState::NotRunning
                }
            },
            close: &|| {
                close_calls.set(close_calls.get() + 1);
                running.set(false);
                true
            },
            start: Some(&|| true),
            post_verify: Some(&|| Some("active".to_string())),
        };
        let outcome = switch_with_auto_transfer("uid-b", &deps).unwrap();
        assert!(outcome.switched);
        assert_eq!(outcome.previous_uid.as_deref(), Some("uid-a"));
        // 过户方向：current=target, previous=切号前（显式，非 fallback）。
        assert_eq!(
            transferred.take(),
            Some(("uid-b".to_string(), "uid-a".to_string()))
        );
        // 过户前置关闭被调用（实验后客户端仍在运行 → 过户前再关）。
        assert_eq!(close_calls.get(), 1);
        match outcome.transfer {
            Some(TransferFlowResult::Executed(receipt)) => assert_eq!(receipt.updated_rows, 3),
            other => panic!("期望 Executed，实际 {other:?}"),
        }
        assert!(outcome.restarted);
        assert_eq!(outcome.post_status.as_deref(), Some("active"));
        assert!(outcome.transfer_error.is_none());
    }

    /// 过户失败不回滚切号：transfer Err → switched=true + transfer_error
    /// 如实报因（会话归属可后补，切号优先成功）；冷启动照常执行。
    #[test]
    fn transfer_failure_does_not_rollback_switch() {
        let deps = SwitchFlowDeps {
            read_current_uid: &|| Ok("uid-a".to_string()),
            run_experiment: &|| {
                Ok(ExperimentReceipt {
                    mode: crate::workbuddy_switch_experiment::WriteMode::ToolEnvelope,
                    backup_path: std::path::PathBuf::from("bak"),
                    wrote_info: true,
                    cold_started: false,
                    post_status: None,
                    post_uid: Some("uid-b".to_string()),
                })
            },
            verify_switch: &|| Ok("uid-b".to_string()),
            run_transfer: &|_, _| Err(TransferError::SqlFailed),
            observe: &|| ClientProcessState::NotRunning,
            close: &|| unreachable!("NotRunning 不需要关闭"),
            start: Some(&|| true),
            post_verify: None,
        };
        let outcome = switch_with_auto_transfer("uid-b", &deps).unwrap();
        assert!(outcome.switched, "切号优先成功");
        assert!(outcome.transfer.is_none());
        assert_eq!(
            outcome.transfer_error.as_deref(),
            Some("wb_transfer_sql_failed"),
            "过户失败如实报因"
        );
        assert!(outcome.restarted, "过户失败不阻塞冷启动");
    }

    /// 过户前置进程 Unknown：拒绝写副作用（P14-2 边界），但切号仍成功
    /// （不回滚）；错误转 transfer_error。
    #[test]
    fn transfer_process_unknown_degrades_to_transfer_error() {
        let deps = SwitchFlowDeps {
            read_current_uid: &|| Ok("uid-a".to_string()),
            run_experiment: &|| {
                Ok(ExperimentReceipt {
                    mode: crate::workbuddy_switch_experiment::WriteMode::ToolEnvelope,
                    backup_path: std::path::PathBuf::from("bak"),
                    wrote_info: true,
                    cold_started: false,
                    post_status: None,
                    post_uid: Some("uid-b".to_string()),
                })
            },
            verify_switch: &|| Ok("uid-b".to_string()),
            run_transfer: &|_, _| unreachable!("Unknown 不进过户"),
            observe: &|| ClientProcessState::Unknown,
            close: &|| unreachable!(),
            start: Some(&|| false),
            post_verify: None,
        };
        let outcome = switch_with_auto_transfer("uid-b", &deps).unwrap();
        assert!(outcome.switched);
        assert!(outcome.transfer.is_none());
        assert_eq!(
            outcome.transfer_error.as_deref(),
            Some("wb_transfer_process_state_unknown")
        );
        assert!(!outcome.restarted, "start 失败如实报告");
    }

    /// 待过户为零：Nothing 分支（无 transfer_error，不写库）。
    #[test]
    fn zero_pending_rows_reports_nothing() {
        let deps = SwitchFlowDeps {
            read_current_uid: &|| Ok("uid-a".to_string()),
            run_experiment: &|| {
                Ok(ExperimentReceipt {
                    mode: crate::workbuddy_switch_experiment::WriteMode::ToolEnvelope,
                    backup_path: std::path::PathBuf::from("bak"),
                    wrote_info: true,
                    cold_started: false,
                    post_status: None,
                    post_uid: Some("uid-b".to_string()),
                })
            },
            verify_switch: &|| Ok("uid-b".to_string()),
            run_transfer: &|_, _| {
                Ok(TransferFlowResult::Nothing { current_sessions: 2 })
            },
            observe: &|| ClientProcessState::NotRunning,
            close: &|| unreachable!(),
            start: None,
            post_verify: None,
        };
        let outcome = switch_with_auto_transfer("uid-b", &deps).unwrap();
        assert!(outcome.switched);
        assert_eq!(
            outcome.transfer,
            Some(TransferFlowResult::Nothing { current_sessions: 2 })
        );
        assert!(outcome.transfer_error.is_none());
        assert!(!outcome.restarted, "start 缺省 = 跳过，回执如实");
    }
}
