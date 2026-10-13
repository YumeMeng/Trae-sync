//! 将进程观测结果统一转换为写入前拒绝条件。

use std::path::Path;

use traesync_infrastructure::trae_instance::{
    current_product_process_state, ProductProcessState, TraeInstanceCloseError,
};
use traesync_infrastructure::TraeProduct;

/// 只有明确确认目标未运行时才允许进入后续写入阶段。
pub(crate) fn ensure_stopped(
    state: ProductProcessState,
    running_error: &'static str,
) -> Result<(), String> {
    match state {
        ProductProcessState::NotRunning => Ok(()),
        ProductProcessState::Running => Err(running_error.to_string()),
        ProductProcessState::Unknown => Err("process_state_unknown".to_string()),
    }
}

/// 按产品和数据目录查询进程状态，并在运行或状态未知时拒绝危险操作。
pub(crate) fn ensure_product_stopped(
    product: TraeProduct,
    data_dir: &Path,
    running_error: &'static str,
) -> Result<(), String> {
    ensure_stopped(
        current_product_process_state(product, data_dir),
        running_error,
    )
}

/// 保留“进程状态未知”的原因码，其它关闭错误沿用操作原有错误码。
pub(crate) fn close_failure_code(
    error: TraeInstanceCloseError,
    operation_failure: &'static str,
) -> String {
    match error {
        TraeInstanceCloseError::ProcessStateUnknown => "process_state_unknown".to_string(),
        TraeInstanceCloseError::CloseFailed => operation_failure.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_or_running_process_state_blocks_before_write() {
        assert_eq!(
            ensure_stopped(ProductProcessState::Running, "target_running").unwrap_err(),
            "target_running"
        );
        assert_eq!(
            ensure_stopped(ProductProcessState::Unknown, "target_running").unwrap_err(),
            "process_state_unknown"
        );
        assert!(ensure_stopped(ProductProcessState::NotRunning, "target_running").is_ok());
    }

    #[test]
    fn close_error_keeps_unknown_distinct_from_close_failure() {
        assert_eq!(
            close_failure_code(
                TraeInstanceCloseError::ProcessStateUnknown,
                "environment_close_failed"
            ),
            "process_state_unknown"
        );
        assert_eq!(
            close_failure_code(
                TraeInstanceCloseError::CloseFailed,
                "environment_close_failed"
            ),
            "environment_close_failed"
        );
    }
}
