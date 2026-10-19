//! 产品操作状态快照：页面展示与普通命令门禁共用同一份操作事实。

use serde::Serialize;
use std::path::Path;
use traesync_infrastructure::trae_instance::ProductProcessState;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum OperationReadiness {
    Ready,
    Pending,
    Absent,
}

#[derive(Debug, Clone, Copy, Serialize)]
pub(crate) struct ProductOperationStateDto {
    pub(crate) operation_id: &'static str,
    pub(crate) state: OperationReadiness,
    pub(crate) reason: &'static str,
    pub(crate) evidence_ref: Option<&'static str>,
    #[serde(skip)]
    gate_error: Option<&'static str>,
}

type ProcessStateObserver<'a> =
    dyn Fn(&str, &str, &Path) -> Result<ProductProcessState, String> + 'a;

#[derive(Debug, Clone, Serialize)]
pub(crate) struct ProductOperationSnapshotDto {
    pub(crate) product_id: String,
    pub(crate) library_id: Option<String>,
    pub(crate) operations: Vec<ProductOperationStateDto>,
}

const WORK_OPERATIONS: &[ProductOperationStateDto] = &[
    operation(
        "account.health_check",
        OperationReadiness::Ready,
        "Work 账号健康检测已完成现有验收。",
        Some("tickets.new.md#P2-4"),
    ),
    operation(
        "account.tool_selection",
        OperationReadiness::Ready,
        "Work 账号选择已完成现有验收。",
        Some("tickets.new.md#P5-2"),
    ),
    operation(
        "account.switch",
        OperationReadiness::Ready,
        "Work 账号切换保持现有已验证行为。",
        Some("tickets.new.md#P5-1"),
    ),
    operation(
        "checkin.status",
        OperationReadiness::Ready,
        "Work 签到状态读取已完成现有验收。",
        Some("tickets.new.md#P1-3"),
    ),
    // P15-10：checkin.claim 惰性注册表条目已清理——签到 claim 由 run_checkin
    // 命令直接承载（SOLO 通道），无门禁消费方，条目只剩惰性元数据。
    operation(
        "library.read",
        OperationReadiness::Ready,
        "Work 库浏览保持现有已验证行为。",
        Some("tickets.new.md#P5-3"),
    ),
    operation(
        "library.archive",
        OperationReadiness::Ready,
        "Work 归档与恢复已于 2026-10-16 完成真机验收（归档+恢复往返，主库终态核对通过）。",
        Some("tickets.new.md#P15-8"),
    ),
    operation(
        "library.delete",
        OperationReadiness::Ready,
        "Work 会话删除已于 2026-10-16 完成真机验收（删除+空壳清理，备份链留存）。",
        Some("tickets.new.md#P15-8"),
    ),
    operation(
        "library.work_project_merge",
        OperationReadiness::Ready,
        "Work 项目归并已于 2026-10-16 完成真机验收（同名项目合并生效，客户端已确认）。",
        Some("tickets.new.md#P15-8"),
    ),
];

const TRAE_CN_OPERATIONS: &[ProductOperationStateDto] = &[
    operation(
        "account.health_check",
        OperationReadiness::Ready,
        "Trae CN 账号健康检测已完成现有验收。",
        // P17-A7：证据锚点持久化到 tickets（原 .scratch 探针报告被清理）。
        Some("tickets.new.md#P2-4"),
    ),
    operation(
        "account.tool_selection",
        OperationReadiness::Ready,
        "工具内账号选择已实现；这不代表官方客户端已切换。",
        Some("tests/AccountAdapter.test.tsx#switch_trae_cn_account"),
    ),
    operation(
        "account.native_activation",
        OperationReadiness::Pending,
        "官方客户端真实切换尚待隔离验收。",
        Some("tickets.new.md#P14-8b"),
    ),
    operation(
        "checkin.status",
        OperationReadiness::Ready,
        "签到状态查询已通过只读探针验证。",
        Some("tickets.new.md#P1-3"), // P17-A7：探针证据持久化（原 .scratch 报告被清理）
    ),
    // P15-10：checkin.claim 惰性条目清理（原 Pending「真实领取尚待验收」；
    // 融合版身份下的真实 claim 验收由签到域真机关口承载，注册表不再登记）。
    operation(
        "checkin.schedule",
        OperationReadiness::Absent,
        "Trae CN 当前不提供自动签到。",
        Some("docs/adr/0030-trae-cn-credential-maintenance-boundary.md"),
    ),
    operation(
        "library.read",
        OperationReadiness::Ready,
        "Trae CN 数据根只读浏览已实现。",
        Some("tickets.new.md#P5-3"), // P17-A7：探针证据持久化
    ),
    operation(
        "library.archive",
        OperationReadiness::Pending,
        "Trae CN 归档尚待真实验收。",
        Some("tickets.new.md#P14-9"),
    ),
    operation(
        "library.trae_cn_project_merge",
        OperationReadiness::Pending,
        "Trae CN 项目归并尚待真实验收。",
        Some("tickets.new.md#P14-9"),
    ),
];

const fn operation(
    operation_id: &'static str,
    state: OperationReadiness,
    reason: &'static str,
    evidence_ref: Option<&'static str>,
) -> ProductOperationStateDto {
    ProductOperationStateDto {
        operation_id,
        state,
        reason,
        evidence_ref,
        gate_error: None,
    }
}

fn operations_for(product_id: &str) -> Result<&'static [ProductOperationStateDto], String> {
    match product_id {
        "work_cn" => Ok(WORK_OPERATIONS),
        "trae_cn" => Ok(TRAE_CN_OPERATIONS),
        _ => Err("product_not_found".to_string()),
    }
}

pub(crate) fn snapshot(
    product_id: &str,
    library_id: Option<String>,
    storage_root: &Path,
    real_environment: bool,
) -> Result<ProductOperationSnapshotDto, String> {
    snapshot_with_process_observer(
        product_id,
        library_id,
        storage_root,
        real_environment,
        &crate::library_archive::observe_library_process_state,
    )
}

/// 使用注入的进程观察器生成快照，测试不枚举本机进程。
fn snapshot_with_process_observer(
    product_id: &str,
    library_id: Option<String>,
    storage_root: &Path,
    real_environment: bool,
    observe_process: &ProcessStateObserver<'_>,
) -> Result<ProductOperationSnapshotDto, String> {
    if let Some(library_id) = library_id.as_deref() {
        crate::library_archive::validate_product_library_id(product_id, library_id, storage_root)?;
    }
    let operation_definitions = operations_for(product_id)?;
    let process_state = if real_environment
        && library_id.is_some()
        && operation_definitions
            .iter()
            .any(|operation| requires_process_observation(operation.operation_id))
    {
        Some(
            observe_process(product_id, library_id.as_deref().unwrap(), storage_root)
                .unwrap_or(ProductProcessState::Unknown),
        )
    } else {
        None
    };
    let operations = operation_definitions
        .iter()
        .map(|operation| {
            let library_missing =
                operation.operation_id.starts_with("library.") && library_id.is_none();
            let unavailable_runtime =
                !real_environment && operation.state == OperationReadiness::Ready;
            let process_gate = process_state.and_then(|state| {
                requires_process_observation(operation.operation_id).then(|| {
                    (
                        state,
                        process_gate_error(product_id, operation.operation_id, state),
                    )
                })
            });
            if let Some((state, error)) = process_gate {
                let blocked = error.is_some();
                ProductOperationStateDto {
                    operation_id: operation.operation_id,
                    state: if blocked {
                        OperationReadiness::Pending
                    } else {
                        operation.state
                    },
                    reason: match (state, error) {
                        (ProductProcessState::Running, Some(_)) => {
                            "请先关闭对应 TRAE 客户端，再执行此操作。"
                        }
                        (ProductProcessState::Unknown, Some(_)) => {
                            "无法确认对应 TRAE 客户端是否运行，暂不能执行此操作。"
                        }
                        (ProductProcessState::Running, None)
                            if operation.state == OperationReadiness::Ready =>
                        {
                            "归档前会先关闭 Work 客户端，再进行备份与写入。"
                        }
                        (ProductProcessState::Running, None) => operation.reason,
                        _ => operation.reason,
                    },
                    evidence_ref: operation.evidence_ref,
                    gate_error: error,
                }
            } else if library_missing || unavailable_runtime {
                ProductOperationStateDto {
                    operation_id: operation.operation_id,
                    state: OperationReadiness::Pending,
                    reason: if library_missing {
                        "选择数据后才能确认当前库的操作状态。"
                    } else {
                        "当前运行模式不支持真实 TRAE 操作。"
                    },
                    evidence_ref: operation.evidence_ref,
                    gate_error: None,
                }
            } else {
                *operation
            }
        })
        .collect();
    Ok(ProductOperationSnapshotDto {
        product_id: product_id.to_string(),
        library_id,
        operations,
    })
}

fn requires_process_observation(operation_id: &str) -> bool {
    matches!(
        operation_id,
        "library.archive"
            | "library.delete"
            | "library.work_project_merge"
            | "library.trae_cn_project_merge"
    )
}

/// Work 归档会自行关闭客户端；CN 归档与两类归并要求进入操作前已停机。
fn process_gate_error(
    product_id: &str,
    operation_id: &str,
    process_state: ProductProcessState,
) -> Option<&'static str> {
    let requires_stopped = match operation_id {
        "library.archive" => product_id == "trae_cn",
        "library.delete" | "library.work_project_merge" | "library.trae_cn_project_merge" => true,
        _ => false,
    };
    match process_state {
        ProductProcessState::Unknown => Some("process_state_unknown"),
        ProductProcessState::Running if requires_stopped => Some(match product_id {
            "trae_cn" if operation_id == "library.archive" => "trae_cn_archive_client_running",
            "trae_cn" => "trae_cn_merge_client_running",
            _ if operation_id == "library.delete" => "master_delete_running",
            _ => "master_merge_running",
        }),
        ProductProcessState::Running | ProductProcessState::NotRunning => None,
    }
}

/// 执行命令在任何副作用前按相同的产品、库与运行环境规则复查门禁。
pub(crate) fn ensure_ready(
    product_id: &str,
    operation_id: &str,
    library_id: Option<String>,
    storage_root: &Path,
    real_environment: bool,
) -> Result<(), String> {
    ensure_ready_with_process_observer(
        product_id,
        operation_id,
        library_id,
        storage_root,
        real_environment,
        &crate::library_archive::observe_library_process_state,
    )
}

fn ensure_ready_with_process_observer(
    product_id: &str,
    operation_id: &str,
    library_id: Option<String>,
    storage_root: &Path,
    real_environment: bool,
    observe_process: &ProcessStateObserver<'_>,
) -> Result<(), String> {
    let snapshot = snapshot_with_process_observer(
        product_id,
        library_id,
        storage_root,
        real_environment,
        observe_process,
    )?;
    let state = snapshot
        .operations
        .iter()
        .find(|state| state.operation_id == operation_id)
        .ok_or_else(|| "product_operation_unknown".to_string())?;
    match state.state {
        OperationReadiness::Ready => Ok(()),
        OperationReadiness::Pending => Err(state
            .gate_error
            .unwrap_or("product_operation_pending")
            .to_string()),
        OperationReadiness::Absent => Err("product_operation_absent".to_string()),
    }
}

/// 兼容库命令的缺省主库语义，并从真实库上下文推导所属产品。
pub(crate) fn ensure_library_ready(
    operation_id: &str,
    library_id: Option<String>,
    storage_root: &Path,
    real_environment: bool,
) -> Result<(), String> {
    ensure_library_ready_with_process_observer(
        operation_id,
        library_id,
        storage_root,
        real_environment,
        &crate::library_archive::observe_library_process_state,
    )
}

fn ensure_library_ready_with_process_observer(
    operation_id: &str,
    library_id: Option<String>,
    storage_root: &Path,
    real_environment: bool,
    observe_process: &ProcessStateObserver<'_>,
) -> Result<(), String> {
    let library_id = library_id.or_else(|| Some("master".to_string()));
    let product_id =
        crate::library_archive::product_id_for_library(library_id.as_deref(), storage_root)?;
    ensure_ready_with_process_observer(
        product_id,
        operation_id,
        library_id,
        storage_root,
        real_environment,
        observe_process,
    )
}

#[tauri::command]
pub(crate) fn get_product_operation_states(
    product_id: String,
    library_id: Option<String>,
    state: tauri::State<'_, crate::AppState>,
) -> Result<ProductOperationSnapshotDto, String> {
    snapshot(
        &product_id,
        library_id,
        Path::new(&state.storage_root),
        state.runtime_mode == crate::RuntimeMode::RealReadPreview,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use tauri::Manager;

    #[test]
    fn unknown_product_and_pending_or_absent_operations_fail_closed() {
        let root = tempfile::tempdir().unwrap();
        assert_eq!(
            snapshot("unknown", None, root.path(), true).unwrap_err(),
            "product_not_found"
        );
        // P15-10：checkin.claim 惰性条目清理后，未登记的操作一律 fail-closed。
        assert_eq!(
            ensure_ready("trae_cn", "checkin.claim", None, root.path(), true).unwrap_err(),
            "product_operation_unknown"
        );
        assert_eq!(
            ensure_ready("trae_cn", "checkin.schedule", None, root.path(), true).unwrap_err(),
            "product_operation_absent"
        );
        assert_eq!(
            ensure_ready("trae_cn", "unknown", None, root.path(), true).unwrap_err(),
            "product_operation_unknown"
        );
    }

    #[test]
    fn snapshot_requires_a_library_and_rejects_cross_product_library() {
        let root = tempfile::tempdir().unwrap();
        let missing = snapshot("work_cn", None, root.path(), true).unwrap();
        for operation in missing
            .operations
            .iter()
            .filter(|operation| operation.operation_id.starts_with("library."))
        {
            assert_eq!(operation.state, OperationReadiness::Pending);
            assert_eq!(operation.reason, "选择数据后才能确认当前库的操作状态。");
        }
        assert_eq!(
            snapshot("trae_cn", Some("master".to_string()), root.path(), true).unwrap_err(),
            "product_library_mismatch"
        );
    }

    #[test]
    fn library_mutation_gate_uses_the_resolved_products_operation_state() {
        let root = tempfile::tempdir().unwrap();
        let observe = |_: &str, _: &str, _: &Path| Ok(ProductProcessState::NotRunning);
        let cn_snapshot = snapshot_with_process_observer(
            "trae_cn",
            Some("trae_cn".to_string()),
            root.path(),
            true,
            &observe,
        )
        .unwrap();
        assert_eq!(
            cn_snapshot
                .operations
                .iter()
                .find(|operation| operation.operation_id == "library.trae_cn_project_merge")
                .unwrap()
                .state,
            OperationReadiness::Pending
        );
        assert!(!cn_snapshot
            .operations
            .iter()
            .any(|operation| operation.operation_id == "library.work_project_merge"));
        assert_eq!(
            ensure_library_ready_with_process_observer(
                "library.trae_cn_project_merge",
                Some("trae_cn".to_string()),
                root.path(),
                true,
                &observe,
            )
            .unwrap_err(),
            "product_operation_pending"
        );
        assert_eq!(
            ensure_library_ready_with_process_observer(
                "library.archive",
                Some("trae_cn".to_string()),
                root.path(),
                true,
                &observe,
            )
            .unwrap_err(),
            "product_operation_pending"
        );
        // P15-8：Work 三项完成真机验收转 Ready——NotRunning 下闸门放行
        //（进程规则仍由 snapshot 运行时降级承载，见后续两个进程规则测试）；
        // Pending 拒绝语义由上方 trae_cn 侧条目继续覆盖。
        assert!(ensure_library_ready_with_process_observer(
            "library.archive",
            Some("master".to_string()),
            root.path(),
            true,
            &observe,
        )
        .is_ok());
        assert!(ensure_library_ready_with_process_observer(
            "library.work_project_merge",
            Some("master".to_string()),
            root.path(),
            true,
            &observe,
        )
        .is_ok());
        assert!(ensure_library_ready_with_process_observer(
            "library.work_project_merge",
            None,
            root.path(),
            true,
            &observe,
        )
        .is_ok());
    }

    #[test]
    fn work_merge_readiness_and_command_gate_share_target_process_state() {
        let root = tempfile::tempdir().unwrap();
        // P15-8：验收后静态 Ready——NotRunning 放行；Running/Unknown 仍由
        // snapshot 运行时降级为 Pending（进程闸门规则不变）。
        let cases = [
            (
                ProductProcessState::NotRunning,
                OperationReadiness::Ready,
                None,
            ),
            (
                ProductProcessState::Running,
                OperationReadiness::Pending,
                Some("master_merge_running"),
            ),
            (
                ProductProcessState::Unknown,
                OperationReadiness::Pending,
                Some("process_state_unknown"),
            ),
        ];

        for (process_state, expected_state, expected_gate) in cases {
            let observe = move |product_id: &str, library_id: &str, _: &Path| {
                assert_eq!(product_id, "work_cn");
                assert_eq!(library_id, "master");
                Ok(process_state)
            };
            let snapshot = snapshot_with_process_observer(
                "work_cn",
                Some("master".to_string()),
                root.path(),
                true,
                &observe,
            )
            .unwrap();
            let merge = snapshot
                .operations
                .iter()
                .find(|operation| operation.operation_id == "library.work_project_merge")
                .unwrap();
            assert_eq!(merge.state, expected_state);
            let result = ensure_library_ready_with_process_observer(
                "library.work_project_merge",
                Some("master".to_string()),
                root.path(),
                true,
                &observe,
            );
            assert_eq!(result.err().as_deref(), expected_gate);
        }
    }

    #[test]
    fn work_archive_and_delete_readiness_follow_distinct_process_rules() {
        let root = tempfile::tempdir().unwrap();
        let cases = [
            (ProductProcessState::NotRunning, None, None),
            (
                ProductProcessState::Running,
                None,
                Some("master_delete_running"),
            ),
            (
                ProductProcessState::Unknown,
                Some("process_state_unknown"),
                Some("process_state_unknown"),
            ),
        ];

        for (process_state, expected_archive_error, expected_delete_error) in cases {
            let observe = move |product_id: &str, library_id: &str, _: &Path| {
                assert_eq!(product_id, "work_cn");
                assert_eq!(library_id, "master");
                Ok(process_state)
            };
            let snapshot = snapshot_with_process_observer(
                "work_cn",
                Some("master".to_string()),
                root.path(),
                true,
                &observe,
            )
            .unwrap();
            let operation = |id| {
                snapshot
                    .operations
                    .iter()
                    .find(|item| item.operation_id == id)
                    .unwrap()
            };

            let archive = operation("library.archive");
            let delete = operation("library.delete");
            // P15-8：静态 Ready 后，snapshot 仅在进程闸门阻断（error=Some）
            // 时降级 Pending；未阻断透传静态值。archive 的运行中守卫在提交
            // 编排内（ensure_cn_client_stopped），注册表层 Running 不阻断。
            match process_state {
                ProductProcessState::NotRunning => {
                    assert_eq!(archive.state, OperationReadiness::Ready);
                    assert_eq!(archive.gate_error, None);
                    assert_eq!(delete.state, OperationReadiness::Ready);
                    assert_eq!(delete.gate_error, None);
                }
                ProductProcessState::Running => {
                    assert_eq!(archive.state, OperationReadiness::Ready);
                    assert_eq!(archive.gate_error, None);
                    assert_eq!(delete.state, OperationReadiness::Pending);
                    assert_eq!(
                        delete.gate_error.as_deref(),
                        Some("master_delete_running")
                    );
                }
                ProductProcessState::Unknown => {
                    assert_eq!(archive.state, OperationReadiness::Pending);
                    assert_eq!(
                        archive.gate_error.as_deref(),
                        Some("process_state_unknown")
                    );
                    assert_eq!(delete.state, OperationReadiness::Pending);
                    assert_eq!(
                        delete.gate_error.as_deref(),
                        Some("process_state_unknown")
                    );
                }
            }
        }

        assert!(!TRAE_CN_OPERATIONS
            .iter()
            .any(|item| item.operation_id == "library.delete"));
    }

    #[test]
    fn trae_cn_archive_process_snapshot_and_gate_match() {
        let root = tempfile::tempdir().unwrap();
        let cases = [
            (ProductProcessState::NotRunning, "product_operation_pending"),
            (
                ProductProcessState::Running,
                "trae_cn_archive_client_running",
            ),
            (ProductProcessState::Unknown, "process_state_unknown"),
        ];

        for (process_state, expected_error) in cases {
            let observe = move |product_id: &str, library_id: &str, _: &Path| {
                assert_eq!(product_id, "trae_cn");
                assert_eq!(library_id, "trae_cn");
                Ok(process_state)
            };
            let snapshot = snapshot_with_process_observer(
                "trae_cn",
                Some("trae_cn".to_string()),
                root.path(),
                true,
                &observe,
            )
            .unwrap();
            let archive = snapshot
                .operations
                .iter()
                .find(|operation| operation.operation_id == "library.archive")
                .unwrap();
            assert_eq!(archive.state, OperationReadiness::Pending);
            assert_eq!(
                ensure_ready_with_process_observer(
                    "trae_cn",
                    "library.archive",
                    Some("trae_cn".to_string()),
                    root.path(),
                    true,
                    &observe,
                )
                .unwrap_err(),
                expected_error
            );
        }
    }

    #[test]
    fn trae_cn_merge_stays_pending_and_gate_tracks_process_state() {
        let root = tempfile::tempdir().unwrap();
        let cases = [
            (ProductProcessState::NotRunning, "product_operation_pending"),
            (ProductProcessState::Running, "trae_cn_merge_client_running"),
            (ProductProcessState::Unknown, "process_state_unknown"),
        ];

        for (process_state, expected_error) in cases {
            let observe = move |product_id: &str, library_id: &str, _: &Path| {
                assert_eq!(product_id, "trae_cn");
                assert_eq!(library_id, "trae_cn");
                Ok(process_state)
            };
            let snapshot = snapshot_with_process_observer(
                "trae_cn",
                Some("trae_cn".to_string()),
                root.path(),
                true,
                &observe,
            )
            .unwrap();
            let merge = snapshot
                .operations
                .iter()
                .find(|operation| operation.operation_id == "library.trae_cn_project_merge")
                .unwrap();
            assert_eq!(merge.state, OperationReadiness::Pending);
            assert_eq!(
                ensure_ready_with_process_observer(
                    "trae_cn",
                    "library.trae_cn_project_merge",
                    Some("trae_cn".to_string()),
                    root.path(),
                    true,
                    &observe,
                )
                .unwrap_err(),
                expected_error
            );
        }
    }

    #[test]
    fn pending_archive_apply_and_delete_stop_before_real_mode_work() {
        let root = tempfile::tempdir().unwrap();
        let state = crate::test_support::make_fixture_mode_state(root.path());
        let app = tauri::test::mock_builder()
            .manage(state)
            .build(tauri::generate_context!())
            .unwrap();

        // P15-8：主库 archive/delete 静态 Ready 后，fixture 模式下写入口仍被
        // 挡在真实模式工作之外——fixture state 的 storage_root 无生产目录库，
        // snapshot 运行时降级为 Pending（product_operation_pending）承载拦截；
        // trae_cn 侧 archive 维持注册表 Pending 闸门。
        let apply_error =
            tauri::async_runtime::block_on(crate::library_archive::apply_master_archive_changes(
                vec!["session-1".to_string()],
                Vec::new(),
                Some("trae_cn".to_string()),
                app.state::<crate::AppState>(),
            ))
            .err()
            .unwrap();
        assert_eq!(apply_error, "product_operation_pending");

        let delete_error =
            tauri::async_runtime::block_on(crate::library_archive::delete_master_sessions(
                vec!["session-1".to_string()],
                Some("master".to_string()),
                app.state::<crate::AppState>(),
            ))
            .err()
            .unwrap();
        assert_eq!(delete_error, "product_operation_pending");

        assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 0);
    }

    #[test]
    fn ready_operations_point_to_operation_specific_acceptance_evidence() {
        const EXPECTED_EVIDENCE: &[(&str, &str, &str)] = &[
            ("work_cn", "account.health_check", "tickets.new.md#P2-4"),
            ("work_cn", "account.tool_selection", "tickets.new.md#P5-2"),
            ("work_cn", "account.switch", "tickets.new.md#P5-1"),
            ("work_cn", "checkin.status", "tickets.new.md#P1-3"),
            ("work_cn", "library.read", "tickets.new.md#P5-3"),
            ("work_cn", "library.archive", "tickets.new.md#P15-8"),
            ("work_cn", "library.delete", "tickets.new.md#P15-8"),
            (
                "work_cn",
                "library.work_project_merge",
                "tickets.new.md#P15-8",
            ),
            (
                "trae_cn",
                "account.health_check",
                // P17-A7：原 .scratch 探针报告被空间治理清理，证据锚点改持久化
                // 到 tickets（P2-4 含"健康检测"+"Rust 单测 11 项"双标记，与
                // work_cn 侧同源语义）。
                "tickets.new.md#P2-4",
            ),
            (
                "trae_cn",
                "account.tool_selection",
                "tests/AccountAdapter.test.tsx#switch_trae_cn_account",
            ),
            (
                "trae_cn",
                "checkin.status",
                "tickets.new.md#P1-3",
            ),
            (
                "trae_cn",
                "library.read",
                "tickets.new.md#P5-3",
            ),
        ];
        let repository_root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
        let ready_operations: Vec<_> = WORK_OPERATIONS
            .iter()
            .filter(|operation| operation.state == OperationReadiness::Ready)
            .map(|operation| ("work_cn", operation))
            .chain(
                TRAE_CN_OPERATIONS
                    .iter()
                    .filter(|operation| operation.state == OperationReadiness::Ready)
                    .map(|operation| ("trae_cn", operation)),
            )
            .collect();
        assert_eq!(ready_operations.len(), EXPECTED_EVIDENCE.len());

        for (product_id, operation) in ready_operations {
            let evidence_ref = operation
                .evidence_ref
                .expect("ready operation needs evidence");
            assert_eq!(
                EXPECTED_EVIDENCE
                    .iter()
                    .find(|(expected_product, operation_id, _)| {
                        *expected_product == product_id && *operation_id == operation.operation_id
                    })
                    .map(|(_, _, evidence)| *evidence),
                Some(evidence_ref),
                "ready evidence changed for {}.{}",
                product_id,
                operation.operation_id
            );
            let (file, fragment) = evidence_ref
                .split_once('#')
                .expect("operation evidence needs an operation-specific anchor");
            let evidence_path = repository_root.join(file);
            let evidence = std::fs::read_to_string(&evidence_path)
                .unwrap_or_else(|error| panic!("cannot read {evidence_ref}: {error}"));

            if file == "tickets.new.md" {
                let section = ticket_evidence_section(&evidence, fragment)
                    .unwrap_or_else(|| panic!("missing ticket evidence section: {evidence_ref}"));
                for marker in ticket_evidence_markers(operation.operation_id) {
                    assert!(
                        section.contains(marker),
                        "ticket evidence for {} lacks {marker}: {evidence_ref}",
                        operation.operation_id
                    );
                }
            } else if file.ends_with(".json") {
                verify_probe_evidence(operation.operation_id, fragment, &evidence);
            } else if operation.operation_id == "account.tool_selection" {
                // P15-10：测试标题随单产品塌缩更名，证据锚点同步（文件与
                // switch_trae_cn_account 用例保持同一证据链）。
                assert!(evidence.contains("适配器路径使用同一账号工作台"));
                assert!(evidence.contains("switch_trae_cn_account"));
            } else {
                panic!(
                    "unsupported evidence type for {}: {evidence_ref}",
                    operation.operation_id
                );
            }
        }
    }

    #[test]
    fn work_library_mutations_ready_after_real_machine_acceptance() {
        // P15-8（2026-10-16）：三项真机验收通过（探针 p15_acceptance 在真实
        // 主库执行归并/删除/归档+恢复，终态核对通过，客户端结果用户确认），
        // 注册表由 Pending 转 Ready，证据锚点指向验收票据小节。
        let expected = [
            (
                "library.archive",
                "Work 归档与恢复已于 2026-10-16 完成真机验收（归档+恢复往返，主库终态核对通过）。",
            ),
            (
                "library.delete",
                "Work 会话删除已于 2026-10-16 完成真机验收（删除+空壳清理，备份链留存）。",
            ),
            (
                "library.work_project_merge",
                "Work 项目归并已于 2026-10-16 完成真机验收（同名项目合并生效，客户端已确认）。",
            ),
        ];

        for (operation_id, expected_reason) in expected {
            let operation = WORK_OPERATIONS
                .iter()
                .find(|operation| operation.operation_id == operation_id)
                .unwrap();
            assert_eq!(operation.state, OperationReadiness::Ready);
            assert_eq!(operation.reason, expected_reason);
            assert_eq!(
                operation.evidence_ref,
                Some("tickets.new.md#P15-8")
            );
        }
    }

    fn ticket_evidence_section<'a>(contents: &'a str, anchor: &str) -> Option<String> {
        let heading_prefix = format!("### {anchor} ");
        let mut in_section = false;
        let mut lines = Vec::new();
        for line in contents.lines() {
            if !in_section {
                if line.starts_with(&heading_prefix) {
                    in_section = true;
                    lines.push(line);
                }
            } else if line.starts_with("### ") {
                break;
            } else {
                lines.push(line);
            }
        }
        in_section.then(|| lines.join("\n"))
    }

    fn ticket_evidence_markers(operation_id: &str) -> &'static [&'static str] {
        match operation_id {
            "account.health_check" => &["健康检测", "Rust 单测 11 项"],
            "account.tool_selection" => &["AccountCenter.tsx", "当前账号"],
            "account.switch" => &["五步事务", "真机端到端验收"],
            "checkin.status" => &["status API", "积分"],
            "library.read" => &["get_master_history", "真机验收"],
            // P15-8：三项写操作的验收证据标记（tickets.new.md#P15-8 小节须
            // 包含对应关键词，验收回执数字与备份路径随小节落档）。
            "library.archive" => &["归档", "恢复", "真机验收", "archived_sessions"],
            "library.delete" => &["删除", "空壳", "真机验收", "deleted_sessions"],
            "library.work_project_merge" => &["归并", "moved_sessions", "真机验收"],
            _ => panic!("ticket evidence is not mapped for {operation_id}"),
        }
    }

    fn verify_probe_evidence(operation_id: &str, probe: &str, contents: &str) {
        let report: serde_json::Value = serde_json::from_str(contents).unwrap();
        assert_eq!(report["probe"], probe, "wrong probe for {operation_id}");
        match operation_id {
            "account.health_check" => {
                let accounts = report["accounts"].as_array().unwrap();
                assert!(!accounts.is_empty());
                assert!(accounts.iter().all(|account| {
                    account["healthy"] == true && account["auth_matches_registry"] == true
                }));
            }
            "checkin.status" => {
                let findings = report["findings"].as_array().unwrap();
                assert!(findings.iter().any(|finding| {
                    finding["endpoint"]
                        .as_str()
                        .is_some_and(|endpoint| endpoint.contains("checkin_credits/status"))
                        && finding["outcome"] == "ok"
                        && finding["http_status"] == 200
                        && finding["business_code"] == 0
                }));
            }
            "library.read" => {
                assert_eq!(report["history"]["status"], "ready");
                assert_eq!(report["stats"]["status"], "ready");
                assert_eq!(report["messages"]["status"], "ready");
            }
            _ => panic!("probe evidence is not mapped for {operation_id}"),
        }
    }

    #[test]
    fn snapshot_downgrades_ready_operations_outside_real_runtime() {
        let root = tempfile::tempdir().unwrap();
        let snapshot = snapshot("trae_cn", None, root.path(), false).unwrap();
        assert_eq!(
            snapshot
                .operations
                .iter()
                .find(|state| state.operation_id == "checkin.status")
                .unwrap()
                .state,
            OperationReadiness::Pending
        );
        assert_eq!(
            snapshot
                .operations
                .iter()
                .find(|state| state.operation_id == "checkin.schedule")
                .unwrap()
                .state,
            OperationReadiness::Absent
        );
    }
}
