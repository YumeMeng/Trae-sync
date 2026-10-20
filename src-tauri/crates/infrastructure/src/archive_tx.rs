//! P19-B3 归档事务纪律状态机（master_archive 参数化提取，Trae 与 WB 共用）。
//!
//! 纪律（p19-architecture-draft §4.2，自 master_archive.rs L236-358 模式提取）：
//!
//! ```text
//! 预检(库存在/可读——调用方负责) → 归属重校验(写前在库层重新确认归属，
//! body 内完成) → unchecked_transaction → 计数/写入 → commit
//! （失败 = CommitOutcomeUnknown，不得误报已回滚）→ 提交后读回校验
//! （不符 = CommittedNeedsAttention）
//! ```
//!
//! 提取形态：body 与 verify 的错误类型由调用方选定（`E`），提交失败的
//! 错误值由调用方以 `commit_error` 闭包构造——TRAE `MasterArchiveError`
//! 与 WB `WorkBuddyLibraryError` 各自的 `CommitOutcomeUnknown` 变体
//! 无需建立继承关系即可共用同一状态机。

use rusqlite::Connection;

/// 事务纪律状态机主体。
///
/// - `open_error`：`unchecked_transaction` 开启失败时构造错误（事务尚未
///   开始，语义归"写入失败"而非"提交结果未知"）。
/// - `body`：事务体内操作（含写前归属重校验）；失败整体回滚并原样返回
///   调用方错误（库保持操作前状态，由 SQLite 事务原子性保证）。
/// - `commit_error`：`tx.commit()` 失败时构造错误——提交错误意味着无法
///   证明事务最终持久状态，必须用专用变体（不得误报"已回滚"）。
/// - `verify_after_commit`：提交后读回校验；接收 body 的返回值（预期计数
///   等由事务内算出并带回）。校验失败 = 已提交需处理，调用方不得把它
///   显示为未提交并鼓励重试。
pub fn run_archive_transaction<T, E>(
    conn: &Connection,
    open_error: impl FnOnce() -> E,
    body: impl FnOnce(&rusqlite::Transaction) -> Result<T, E>,
    commit_error: impl FnOnce() -> E,
    verify_after_commit: impl FnOnce(&Connection, &T) -> Result<(), E>,
) -> Result<T, E> {
    let tx = conn.unchecked_transaction().map_err(|_| open_error())?;
    match body(&tx) {
        Ok(value) => {
            tx.commit().map_err(|_| commit_error())?;
            verify_after_commit(conn, &value)?;
            Ok(value)
        }
        Err(error) => {
            // 回滚失败也只能放弃：库由 SQLite 事务原子性保证（连接关闭时回滚）。
            let _ = tx.rollback();
            Err(error)
        }
    }
}

/// 构造 `?, ?, ...` 占位符串（批量 IN 子句用；master_archive 与 WB 库共用）。
pub(crate) fn placeholders(count: usize) -> String {
    vec!["?"; count].join(", ")
}

/// 构造 `?N, ?N+1, ...` 编号占位符串（从 `start` 起；SQL 中 IN 子句出现在
/// 显式编号参数之后时必须用编号形态——SQLite 把无编号 `?` 顺延到最大
/// 已用编号之后，混用会错位。master_archive 的 IN-在前场景无此问题，
/// WB 库的 `SET x = ?N WHERE id IN (…)` 场景必须编号）。
pub(crate) fn numbered_placeholders(count: usize, start: usize) -> String {
    (0..count)
        .map(|offset| format!("?{}", start + offset))
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 测试错误：与真实调用方同构的最小三变体。
    #[derive(Debug, Clone, PartialEq, Eq)]
    enum TestError {
        WriteFailed,
        CommitOutcomeUnknown,
        CommittedNeedsAttention,
    }

    /// 事务体失败：整体回滚，原样返回 body 错误。
    #[test]
    fn body_error_rolls_back_and_returns_body_error() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE t (v INTEGER);").unwrap();
        let result: Result<usize, TestError> = run_archive_transaction(
            &conn,
            || TestError::WriteFailed,
            |tx| {
                tx.execute("INSERT INTO t VALUES (1)", []).map_err(|_| TestError::WriteFailed)?;
                Err(TestError::WriteFailed)
            },
            || TestError::CommitOutcomeUnknown,
            |_, _| Ok(()),
        );
        assert_eq!(result, Err(TestError::WriteFailed));
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM t", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 0, "body 失败必须回滚，库保持操作前状态");
    }

    /// 提交失败：CommitOutcomeUnknown（用延迟外键制造真实 COMMIT 错误），
    /// 且事务释放后写入没有残留。
    #[test]
    fn commit_error_reports_unknown_and_rolls_back() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "PRAGMA foreign_keys = ON;
             CREATE TABLE parent (id INTEGER PRIMARY KEY);
             CREATE TABLE child (
                parent_id INTEGER REFERENCES parent(id) DEFERRABLE INITIALLY DEFERRED
             );",
        )
        .unwrap();
        let result: Result<usize, TestError> = run_archive_transaction(
            &conn,
            || TestError::WriteFailed,
            |tx| {
                tx.execute("INSERT INTO child VALUES (1)", [])
                    .map_err(|_| TestError::WriteFailed)
            },
            || TestError::CommitOutcomeUnknown,
            |_, _| Ok(()),
        );
        assert_eq!(result, Err(TestError::CommitOutcomeUnknown));
        let child_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM child", [], |row| row.get(0))
            .unwrap();
        assert_eq!(child_count, 0);
    }

    /// 提交成功但读回校验不符：CommittedNeedsAttention（已提交需处理）。
    #[test]
    fn verify_mismatch_after_commit_reports_attention() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE t (v INTEGER);").unwrap();
        let result: Result<usize, TestError> = run_archive_transaction(
            &conn,
            || TestError::WriteFailed,
            |tx| {
                tx.execute("INSERT INTO t VALUES (1)", [])
                    .map_err(|_| TestError::WriteFailed)
            },
            || TestError::CommitOutcomeUnknown,
            |conn, written| {
                // 读回口径故意取"预期 2 行"（实际只写了 1 行）——模拟
                // 真实场景 verify_archive_after_commit 归属行数与预期不符。
                let count: i64 = conn
                    .query_row("SELECT COUNT(*) FROM t", [], |row| row.get(0))
                    .map_err(|_| TestError::CommittedNeedsAttention)?;
                if count as usize == *written + 1 {
                    Ok(())
                } else {
                    Err(TestError::CommittedNeedsAttention)
                }
            },
        );
        assert_eq!(result, Err(TestError::CommittedNeedsAttention));
        // 数据确实已提交（不得误报为已回滚）。
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM t", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 1);
    }

    /// 快乐路径：body 返回值带入 verify，校验通过后原样返回。
    #[test]
    fn happy_path_returns_body_value_after_verified_commit() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE t (v INTEGER);").unwrap();
        let result: Result<usize, TestError> = run_archive_transaction(
            &conn,
            || TestError::WriteFailed,
            |tx| {
                tx.execute("INSERT INTO t VALUES (1)", [])
                    .map_err(|_| TestError::WriteFailed)
            },
            || TestError::CommitOutcomeUnknown,
            |conn, written| {
                let count: i64 = conn
                    .query_row("SELECT COUNT(*) FROM t", [], |row| row.get(0))
                    .map_err(|_| TestError::CommittedNeedsAttention)?;
                if count as usize == *written {
                    Ok(())
                } else {
                    Err(TestError::CommittedNeedsAttention)
                }
            },
        );
        assert_eq!(result, Ok(1));
    }

    /// 占位符构造（批量 IN 子句共用件）。
    #[test]
    fn placeholders_builds_comma_separated_marks() {
        assert_eq!(placeholders(1), "?");
        assert_eq!(placeholders(3), "?, ?, ?");
        assert_eq!(placeholders(0), "");
        // 编号形态（IN 子句在显式编号参数之后的场景）。
        assert_eq!(numbered_placeholders(3, 1), "?1, ?2, ?3");
        assert_eq!(numbered_placeholders(2, 5), "?5, ?6");
        assert_eq!(numbered_placeholders(0, 1), "");
    }
}
