use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::Path;
use std::time::Duration;

use tempfile::TempDir;
use traesync_domain::OperationCancellation;
use traesync_infrastructure::progress::{
    measure_fixture_chunked_io, persist_progress_snapshot, read_progress_snapshot, FixtureIoConfig,
    FixtureIoOutcome, ProgressPhase, ProgressSnapshot, FIXTURE_IO_CHUNK_BYTES,
};

const FIXTURE_BYTES: usize = (16 * 1024 * 1024) + 17;

fn write_synthetic_fixture(path: &Path, length: usize) {
    let mut file = File::create(path).expect("创建合成 fixture 失败");
    let mut block = [0_u8; 64 * 1024];
    for (index, byte) in block.iter_mut().enumerate() {
        *byte = (index as u8).wrapping_mul(31).wrapping_add(7);
    }

    let mut remaining = length;
    while remaining > 0 {
        let count = remaining.min(block.len());
        file.write_all(&block[..count])
            .expect("写入合成 fixture 失败");
        remaining -= count;
    }
    file.sync_all().expect("刷盘合成 fixture 失败");
}

fn streaming_signature(path: &Path) -> (u64, u64) {
    let mut file = File::open(path).expect("打开 fixture 失败");
    let mut buffer = [0_u8; 64 * 1024];
    let mut length = 0_u64;
    let mut checksum = 0_u64;
    loop {
        let count = file.read(&mut buffer).expect("读取 fixture 失败");
        if count == 0 {
            break;
        }
        length += count as u64;
        for byte in &buffer[..count] {
            checksum = checksum.wrapping_add(u64::from(*byte));
        }
    }
    (length, checksum)
}

#[test]
fn t11_slow_fixture_reports_truthful_throttled_progress_and_bounded_memory() {
    let root = TempDir::new().expect("创建 T11 fixture 根失败");
    let root_path = root.path().to_path_buf();
    let source = root.path().join("source.db");
    let destination = root.path().join("staging.db");
    write_synthetic_fixture(&source, FIXTURE_BYTES);
    let total_bytes = fs::metadata(&source)
        .expect("读取 fixture 元数据失败")
        .len();
    let cancellation = OperationCancellation::new();
    let config = FixtureIoConfig::new(Duration::from_millis(5), Duration::from_millis(12));

    let measurement = measure_fixture_chunked_io(
        File::open(&source).expect("打开源 fixture 失败"),
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&destination)
            .expect("创建 staging fixture 失败"),
        "op-1101",
        ProgressPhase::Copying,
        Some(total_bytes),
        &cancellation,
        true,
        config,
    )
    .expect("fixture 流式 I/O 测量失败");

    assert_eq!(measurement.outcome, FixtureIoOutcome::Completed);
    assert_eq!(measurement.bytes_read, total_bytes);
    assert_eq!(measurement.bytes_written, total_bytes);
    assert_eq!(measurement.chunks, 17);
    assert_eq!(measurement.peak_buffer_bytes, FIXTURE_IO_CHUNK_BYTES);
    assert!(measurement.elapsed >= Duration::from_millis(50));
    assert_eq!(
        streaming_signature(&source),
        streaming_signature(&destination)
    );

    assert!(measurement.progress_events.len() >= 2);
    assert!(measurement.progress_events.len() < measurement.chunks as usize);
    assert!(measurement
        .event_intervals
        .iter()
        .all(|interval| *interval >= Duration::from_millis(8)));
    let mut previous_bytes = 0_u64;
    for event in &measurement.progress_events {
        assert_eq!(event.operation_id, "op-1101");
        assert_eq!(event.phase, ProgressPhase::Copying);
        assert_eq!(event.total_bytes, Some(total_bytes));
        assert!(event.cancellable);
        assert!(event.completed_bytes >= previous_bytes);
        assert!(event.completed_bytes <= total_bytes);
        assert!(event.percent_basis_points.is_some());
        previous_bytes = event.completed_bytes;
    }

    #[cfg(windows)]
    {
        let rss_before = measurement
            .rss_before_bytes
            .expect("Windows fixture 应能读取进程 RSS");
        let rss_peak = measurement
            .rss_peak_bytes
            .expect("Windows fixture 应能读取峰值 RSS");
        assert!(rss_peak >= rss_before);
        assert!(rss_peak.saturating_sub(rss_before) < (32 * 1024 * 1024) as u64);
    }

    drop(root);
    assert!(!root_path.exists(), "T11 fixture 临时目录必须清理");
}

#[test]
fn t11_cancellation_boundary_preserves_target_before_write_and_ignores_request_during_write() {
    let root = TempDir::new().expect("创建 T11 取消 fixture 根失败");
    let root_path = root.path().to_path_buf();
    let source = root.path().join("source.db");
    let target = root.path().join("target.db");
    let committed = root.path().join("committed.db");
    write_synthetic_fixture(&source, (2 * 1024 * 1024) + 3);
    fs::write(&target, b"target-before-write").expect("创建目标哨兵失败");
    let target_before = fs::read(&target).expect("读取目标哨兵失败");
    let total_bytes = fs::metadata(&source)
        .expect("读取源 fixture 元数据失败")
        .len();

    let cancellation = OperationCancellation::new();
    cancellation.request();
    let cancelled = measure_fixture_chunked_io(
        File::open(&source).expect("打开取消测试源失败"),
        OpenOptions::new()
            .write(true)
            .open(&target)
            .expect("打开取消测试目标失败"),
        "op-1102",
        ProgressPhase::Copying,
        Some(total_bytes),
        &cancellation,
        true,
        FixtureIoConfig::default(),
    )
    .expect("执行写前取消测量失败");

    assert_eq!(cancelled.outcome, FixtureIoOutcome::CancelledBeforeWrite);
    assert_eq!(cancelled.bytes_written, 0);
    assert_eq!(
        fs::read(&target).expect("读取取消后目标失败"),
        target_before
    );
    assert!(cancelled
        .progress_events
        .iter()
        .all(|event| event.cancellable));

    let writing = measure_fixture_chunked_io(
        File::open(&source).expect("打开写入阶段源失败"),
        File::create(&committed).expect("创建写入阶段目标失败"),
        "op-1102",
        ProgressPhase::Writing,
        None,
        &cancellation,
        false,
        FixtureIoConfig::default(),
    )
    .expect("执行不可取消写入测量失败");

    assert_eq!(writing.outcome, FixtureIoOutcome::Completed);
    assert_eq!(writing.bytes_written, total_bytes);
    assert_eq!(
        streaming_signature(&source),
        streaming_signature(&committed)
    );
    assert!(writing
        .progress_events
        .iter()
        .all(|event| !event.cancellable && event.total_bytes.is_none()));
    assert!(writing
        .progress_events
        .iter()
        .all(|event| event.percent_basis_points.is_none()));

    drop(root);
    assert!(!root_path.exists(), "T11 取消 fixture 临时目录必须清理");
}

#[test]
fn t11_persisted_progress_recovers_by_operation_id_after_event_loss() {
    let root = TempDir::new().expect("创建 T11 进度恢复 fixture 根失败");
    let operation_id = "op-1103";

    // 事件总线不可用时，sidecar 仍按 operation_id 保留最新的非敏感进度。
    persist_progress_snapshot(
        root.path(),
        &ProgressSnapshot::new(operation_id, ProgressPhase::Preparing, 0, None, true),
    )
    .expect("持久化准备阶段进度失败");
    persist_progress_snapshot(
        root.path(),
        &ProgressSnapshot::new(
            operation_id,
            ProgressPhase::Copying,
            FIXTURE_IO_CHUNK_BYTES as u64,
            Some((2 * FIXTURE_IO_CHUNK_BYTES) as u64),
            true,
        ),
    )
    .expect("持久化复制阶段进度失败");

    let recovered = read_progress_snapshot(root.path(), operation_id)
        .expect("按 operation_id 读取进度失败")
        .expect("事件丢失后应能读取持久化进度");
    assert_eq!(recovered.operation_id, operation_id);
    assert_eq!(recovered.phase, ProgressPhase::Copying);
    assert_eq!(recovered.completed_bytes, FIXTURE_IO_CHUNK_BYTES as u64);
    assert_eq!(
        recovered.total_bytes,
        Some((2 * FIXTURE_IO_CHUNK_BYTES) as u64)
    );
    assert_eq!(recovered.percent_basis_points, Some(5_000));

    // 不存在的 operation_id 只能返回空，不得把另一条操作的进度串给前端。
    assert_eq!(
        read_progress_snapshot(root.path(), "op-9999").expect("读取缺失进度失败"),
        None
    );

    let progress_dir = root.path().join("progress");
    let entries: Vec<_> = fs::read_dir(progress_dir)
        .expect("读取进度目录失败")
        .map(|entry| entry.expect("读取进度目录项失败").file_name())
        .collect();
    assert_eq!(entries, vec![std::ffi::OsString::from("op-1103.json")]);
}
