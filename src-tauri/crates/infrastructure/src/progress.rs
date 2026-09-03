//! T11 长任务进度与事件节流。
//!
//! 未知总量阶段只报告已完成字节和阶段，不生成虚假百分比或 ETA。事件按时间和内容
//! 去重，避免慢盘操作被高频进度更新拖慢。

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use traesync_domain::OperationCancellation;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProgressPhase {
    Preparing,
    Copying,
    Hashing,
    Writing,
    Verifying,
    Recovering,
    Completed,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProgressSnapshot {
    pub operation_id: String,
    pub phase: ProgressPhase,
    pub completed_bytes: u64,
    pub total_bytes: Option<u64>,
    pub percent_basis_points: Option<u16>,
    pub cancellable: bool,
}

impl ProgressSnapshot {
    pub fn new(
        operation_id: impl Into<String>,
        phase: ProgressPhase,
        completed_bytes: u64,
        total_bytes: Option<u64>,
        cancellable: bool,
    ) -> Self {
        let percent_basis_points = total_bytes.filter(|total| *total > 0).map(|total| {
            ((u128::from(completed_bytes.min(total)) * 10_000) / u128::from(total)).min(10_000)
                as u16
        });
        Self {
            operation_id: operation_id.into(),
            phase,
            completed_bytes,
            total_bytes,
            percent_basis_points,
            cancellable,
        }
    }
}

/// Tauri 组合根提供的低频进度回调；执行器不依赖 UI 或应用状态类型。
pub type ProgressReporter = Arc<dyn Fn(ProgressSnapshot) + Send + Sync>;

enum ProgressSink {
    Channel(Sender<ProgressSnapshot>),
    Callback(ProgressReporter),
}

pub struct ThrottledProgressEmitter {
    sink: ProgressSink,
    last_emit: Option<Instant>,
    last_snapshot: Option<ProgressSnapshot>,
    interval: Duration,
}

impl ThrottledProgressEmitter {
    pub fn channel(interval: Duration) -> (Self, Receiver<ProgressSnapshot>) {
        let (sender, receiver) = mpsc::channel();
        (
            Self {
                sink: ProgressSink::Channel(sender),
                last_emit: None,
                last_snapshot: None,
                interval,
            },
            receiver,
        )
    }

    /// 创建回调型发射器，供组合根把节流后的事件写入应用状态或 UI 事件总线。
    pub fn callback(interval: Duration, reporter: ProgressReporter) -> Self {
        Self {
            sink: ProgressSink::Callback(reporter),
            last_emit: None,
            last_snapshot: None,
            interval,
        }
    }

    pub fn emit(&mut self, snapshot: ProgressSnapshot) -> bool {
        let now = Instant::now();
        let changed = self.last_snapshot.as_ref() != Some(&snapshot);
        let interval_elapsed = self
            .last_emit
            .map_or(true, |last| now.duration_since(last) >= self.interval);
        let terminal = matches!(
            snapshot.phase,
            ProgressPhase::Completed | ProgressPhase::Failed
        );
        if !changed || (!interval_elapsed && !terminal) {
            return false;
        }
        match &self.sink {
            ProgressSink::Channel(sender) => {
                if sender.send(snapshot.clone()).is_err() {
                    return false;
                }
            }
            ProgressSink::Callback(reporter) => reporter(snapshot.clone()),
        }
        self.last_emit = Some(now);
        self.last_snapshot = Some(snapshot);
        true
    }
}

/// fixture 测量使用的固定分块大小，与生产备份/哈希路径的 1 MiB 分块保持一致。
pub const FIXTURE_IO_CHUNK_BYTES: usize = 1024 * 1024;

/// 为 T11 fixture 提供可重复的慢 I/O 和事件节流参数。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FixtureIoConfig {
    /// 每个成功分块写入后的等待时间，用于模拟慢盘。
    pub delay_per_chunk: Duration,
    /// 进度事件的最小发送间隔。
    pub event_interval: Duration,
}

impl FixtureIoConfig {
    pub const fn new(delay_per_chunk: Duration, event_interval: Duration) -> Self {
        Self {
            delay_per_chunk,
            event_interval,
        }
    }
}

impl Default for FixtureIoConfig {
    fn default() -> Self {
        Self::new(Duration::ZERO, Duration::from_millis(50))
    }
}

/// fixture 流式 I/O 的可观测结果，不保存源文件正文或完整数据库内容。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FixtureIoOutcome {
    Completed,
    /// 可取消阶段收到取消请求；调用方传入的 writer 只代表 staging，不代表目标写入。
    CancelledBeforeWrite,
}

/// T11 fixture 流式 I/O 的耗时、事件、缓冲和 RSS 测量结果。
#[derive(Debug, Clone)]
pub struct FixtureIoMeasurement {
    pub outcome: FixtureIoOutcome,
    pub bytes_read: u64,
    pub bytes_written: u64,
    pub chunks: u64,
    pub elapsed: Duration,
    pub peak_buffer_bytes: usize,
    pub rss_before_bytes: Option<u64>,
    pub rss_peak_bytes: Option<u64>,
    pub rss_after_bytes: Option<u64>,
    pub progress_events: Vec<ProgressSnapshot>,
    pub event_intervals: Vec<Duration>,
}

/// 对已打开的流执行固定分块的 fixture I/O 测量。
///
/// 该 seam 只接收 `Read`/`Write`，不接收路径，不创建 Tauri 写入口，也不会把完整输入
/// 读入内存。`cancellable = true` 时取消只作用于目标写入前的 staging 阶段；进入写入
/// 阶段后应传入 `false`，即使取消信号已置位也必须完成当前数据保护步骤。
pub fn measure_fixture_chunked_io<R, W>(
    mut reader: R,
    mut writer: W,
    operation_id: impl Into<String>,
    phase: ProgressPhase,
    total_bytes: Option<u64>,
    cancellation: &OperationCancellation,
    cancellable: bool,
    config: FixtureIoConfig,
) -> io::Result<FixtureIoMeasurement>
where
    R: Read,
    W: Write,
{
    let started = Instant::now();
    let operation_id = operation_id.into();
    let rss_before = current_process_rss_bytes();
    let mut rss_peak = rss_before;
    let events = Arc::new(Mutex::new(Vec::<(Instant, ProgressSnapshot)>::new()));
    let events_for_reporter = Arc::clone(&events);
    let reporter: ProgressReporter = Arc::new(move |snapshot| {
        events_for_reporter
            .lock()
            .expect("fixture 进度事件锁不应中毒")
            .push((Instant::now(), snapshot));
    });
    let mut emitter = ThrottledProgressEmitter::callback(config.event_interval, reporter);
    let _ = emitter.emit(ProgressSnapshot::new(
        operation_id.clone(),
        phase,
        0,
        total_bytes,
        cancellable,
    ));

    let mut buffer = vec![0_u8; FIXTURE_IO_CHUNK_BYTES];
    observe_peak_rss(&mut rss_peak);
    let mut bytes_read = 0_u64;
    let mut bytes_written = 0_u64;
    let mut chunks = 0_u64;
    let mut outcome = FixtureIoOutcome::Completed;

    loop {
        if cancellable && cancellation.is_requested() {
            outcome = FixtureIoOutcome::CancelledBeforeWrite;
            break;
        }

        let count = reader.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        bytes_read = bytes_read.saturating_add(count as u64);

        // 读完一个分块后再次检查，确保取消不会开始新的目标写入。
        if cancellable && cancellation.is_requested() {
            outcome = FixtureIoOutcome::CancelledBeforeWrite;
            break;
        }

        writer.write_all(&buffer[..count])?;
        bytes_written = bytes_written.saturating_add(count as u64);
        chunks = chunks.saturating_add(1);
        let _ = emitter.emit(ProgressSnapshot::new(
            operation_id.clone(),
            phase,
            bytes_written,
            total_bytes,
            cancellable,
        ));
        observe_peak_rss(&mut rss_peak);
        if !config.delay_per_chunk.is_zero() {
            std::thread::sleep(config.delay_per_chunk);
        }
    }

    if outcome == FixtureIoOutcome::Completed {
        writer.flush()?;
    }
    drop(buffer);
    let rss_after = current_process_rss_bytes();
    observe_peak_value(&mut rss_peak, rss_after);

    // 释放 emitter 后，事件回调不再持有共享状态，可安全提取小型事件轨迹。
    drop(emitter);
    let event_records = Arc::try_unwrap(events)
        .expect("fixture 进度事件回调仍持有共享状态")
        .into_inner()
        .expect("fixture 进度事件锁不应中毒");
    let mut event_intervals = Vec::with_capacity(event_records.len().saturating_sub(1));
    for pair in event_records.windows(2) {
        event_intervals.push(pair[1].0.duration_since(pair[0].0));
    }
    let progress_events = event_records
        .into_iter()
        .map(|(_, snapshot)| snapshot)
        .collect();

    Ok(FixtureIoMeasurement {
        outcome,
        bytes_read,
        bytes_written,
        chunks,
        elapsed: started.elapsed(),
        peak_buffer_bytes: FIXTURE_IO_CHUNK_BYTES,
        rss_before_bytes: rss_before,
        rss_peak_bytes: rss_peak,
        rss_after_bytes: rss_after,
        progress_events,
        event_intervals,
    })
}

fn observe_peak_rss(peak: &mut Option<u64>) {
    observe_peak_value(peak, current_process_rss_bytes());
}

fn observe_peak_value(peak: &mut Option<u64>, sample: Option<u64>) {
    if let Some(sample) = sample {
        *peak = Some(peak.map_or(sample, |current| current.max(sample)));
    }
}

#[cfg(windows)]
#[repr(C)]
struct ProcessMemoryCounters {
    cb: u32,
    page_fault_count: u32,
    peak_working_set_size: usize,
    working_set_size: usize,
    quota_peak_paged_pool_usage: usize,
    quota_paged_pool_usage: usize,
    quota_peak_non_paged_pool_usage: usize,
    quota_non_paged_pool_usage: usize,
    pagefile_usage: usize,
    peak_pagefile_usage: usize,
}

#[cfg(windows)]
#[link(name = "psapi")]
extern "system" {
    fn GetProcessMemoryInfo(
        process: windows_sys::Win32::Foundation::HANDLE,
        counters: *mut ProcessMemoryCounters,
        size: u32,
    ) -> windows_sys::Win32::Foundation::BOOL;
}

#[cfg(windows)]
fn current_process_rss_bytes() -> Option<u64> {
    let mut counters = ProcessMemoryCounters {
        cb: std::mem::size_of::<ProcessMemoryCounters>() as u32,
        page_fault_count: 0,
        peak_working_set_size: 0,
        working_set_size: 0,
        quota_peak_paged_pool_usage: 0,
        quota_paged_pool_usage: 0,
        quota_peak_non_paged_pool_usage: 0,
        quota_non_paged_pool_usage: 0,
        pagefile_usage: 0,
        peak_pagefile_usage: 0,
    };
    let result = unsafe {
        GetProcessMemoryInfo(
            windows_sys::Win32::System::Threading::GetCurrentProcess(),
            &mut counters,
            counters.cb,
        )
    };
    (result != 0).then_some(counters.working_set_size as u64)
}

#[cfg(target_os = "linux")]
fn current_process_rss_bytes() -> Option<u64> {
    let resident_pages = std::fs::read_to_string("/proc/self/statm")
        .ok()?
        .split_whitespace()
        .nth(1)?
        .parse::<u64>()?;
    Some(resident_pages.saturating_mul(4096))
}

#[cfg(all(not(windows), not(target_os = "linux")))]
fn current_process_rss_bytes() -> Option<u64> {
    None
}

/// 将最新进度按 operation_id 保存在固定恢复区，供前端事件丢失后重查。
/// 该 sidecar 不属于操作 manifest，只保存阶段、字节计数和取消边界。
pub fn persist_progress_snapshot(
    recovery_root: &Path,
    snapshot: &ProgressSnapshot,
) -> Result<(), ()> {
    let destination = progress_path(recovery_root, &snapshot.operation_id).ok_or(())?;
    let directory = destination.parent().ok_or(())?;
    reject_directory_chain(directory)?;
    fs::create_dir_all(directory).map_err(|_| ())?;
    // 创建目录后再次检查，避免父目录在创建期间被替换为文件或符号链接。
    reject_directory_chain(directory)?;

    let temporary = directory.join(format!(
        ".{}.tmp-{}-{}",
        snapshot.operation_id,
        now_nanos(),
        std::process::id()
    ));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)
        .map_err(|_| ())?;
    let write_result = (|| {
        serde_json::to_writer(&mut file, snapshot).map_err(|_| ())?;
        file.write_all(b"\n").map_err(|_| ())?;
        file.sync_all().map_err(|_| ())
    })();
    drop(file);
    if write_result.is_err() {
        let _ = fs::remove_file(&temporary);
        return Err(());
    }
    if publish_progress_file(&temporary, &destination).is_err() {
        let _ = fs::remove_file(&temporary);
        return Err(());
    }
    sync_directory(directory)
}

/// 按 operation_id 读取最新进度；缺失表示应用尚未持久化该操作进度。
pub fn read_progress_snapshot(
    recovery_root: &Path,
    operation_id: &str,
) -> Result<Option<ProgressSnapshot>, ()> {
    let Some(path) = progress_path(recovery_root, operation_id) else {
        return Err(());
    };
    let directory = path.parent().ok_or(())?;
    reject_directory_chain(directory)?;
    match fs::symlink_metadata(&path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => Err(()),
        Ok(_) => {
            let snapshot: ProgressSnapshot =
                serde_json::from_reader(File::open(path).map_err(|_| ())?).map_err(|_| ())?;
            if snapshot.operation_id != operation_id {
                return Err(());
            }
            Ok(Some(snapshot))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(_) => Err(()),
    }
}

fn progress_path(recovery_root: &Path, operation_id: &str) -> Option<PathBuf> {
    let suffix = operation_id.strip_prefix("op-")?;
    if suffix.is_empty()
        || suffix
            .chars()
            .any(|character| !character.is_ascii_digit() && character != '-')
    {
        return None;
    }
    Some(
        recovery_root
            .join("progress")
            .join(format!("{operation_id}.json")),
    )
}

fn reject_directory_chain(path: &Path) -> Result<(), ()> {
    // 从目标目录向上检查所有已存在父目录；缺失目录允许由调用方创建，
    // 但任何已存在的文件或符号链接都必须 fail closed。
    let mut current = Some(path);
    while let Some(candidate) = current {
        match fs::symlink_metadata(candidate) {
            Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
                return Err(())
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return Err(()),
        }
        let Some(parent) = candidate.parent() else {
            break;
        };
        if parent == candidate {
            break;
        }
        current = Some(parent);
    }
    Ok(())
}

fn publish_progress_file(temporary: &Path, destination: &Path) -> Result<(), ()> {
    if let Ok(metadata) = fs::symlink_metadata(destination) {
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(());
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::Storage::FileSystem::{
            MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
        };
        let from: Vec<u16> = temporary
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        let to: Vec<u16> = destination
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        let result = unsafe {
            MoveFileExW(
                from.as_ptr(),
                to.as_ptr(),
                MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
            )
        };
        if result == 0 {
            return Err(());
        }
        Ok(())
    }
    #[cfg(not(windows))]
    {
        fs::rename(temporary, destination).map_err(|_| ())
    }
}

fn sync_directory(path: &Path) -> Result<(), ()> {
    #[cfg(unix)]
    {
        File::open(path)
            .map_err(|_| ())?
            .sync_all()
            .map_err(|_| ())?;
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
    Ok(())
}

fn now_nanos() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_total_never_reports_percent() {
        let snapshot = ProgressSnapshot::new("op-1", ProgressPhase::Verifying, 512, None, false);
        assert_eq!(snapshot.percent_basis_points, None);
        assert_eq!(snapshot.total_bytes, None);
    }

    #[test]
    fn known_total_reports_byte_based_percent() {
        let snapshot = ProgressSnapshot::new("op-1", ProgressPhase::Copying, 25, Some(100), true);
        assert_eq!(snapshot.percent_basis_points, Some(2_500));
    }

    #[test]
    fn emitter_deduplicates_and_flushes_terminal_state() {
        let (mut emitter, receiver) = ThrottledProgressEmitter::channel(Duration::from_secs(60));
        let snapshot = ProgressSnapshot::new("op-1", ProgressPhase::Copying, 1, Some(10), true);
        assert!(emitter.emit(snapshot.clone()));
        assert!(!emitter.emit(snapshot));
        let terminal = ProgressSnapshot::new("op-1", ProgressPhase::Completed, 10, Some(10), false);
        assert!(emitter.emit(terminal));
        // emitter 仍持有发送端，使用非阻塞迭代避免测试等待通道关闭。
        assert_eq!(receiver.try_iter().count(), 2);
    }

    #[test]
    fn callback_emitter_deduplicates_and_flushes_terminal_state() {
        let received = Arc::new(std::sync::Mutex::new(Vec::new()));
        let received_for_reporter = Arc::clone(&received);
        let reporter: ProgressReporter = Arc::new(move |snapshot| {
            received_for_reporter.lock().unwrap().push(snapshot);
        });
        let mut emitter = ThrottledProgressEmitter::callback(Duration::from_secs(60), reporter);
        let snapshot = ProgressSnapshot::new("op-1", ProgressPhase::Copying, 1, Some(10), true);

        assert!(emitter.emit(snapshot.clone()));
        assert!(!emitter.emit(snapshot));
        assert!(emitter.emit(ProgressSnapshot::new(
            "op-1",
            ProgressPhase::Completed,
            10,
            Some(10),
            false,
        )));
        assert_eq!(received.lock().unwrap().len(), 2);
    }

    #[test]
    fn fixture_chunked_io_keeps_fixed_buffer_and_tracks_known_total() {
        let input: Vec<u8> = (0..(FIXTURE_IO_CHUNK_BYTES * 2 + 17))
            .map(|index| (index % 251) as u8)
            .collect();
        let cancellation = OperationCancellation::new();
        let mut output = Vec::new();

        let measurement = measure_fixture_chunked_io(
            std::io::Cursor::new(input.clone()),
            &mut output,
            "op-progress-known-total",
            ProgressPhase::Copying,
            Some(input.len() as u64),
            &cancellation,
            true,
            FixtureIoConfig::new(Duration::ZERO, Duration::ZERO),
        )
        .unwrap();

        assert_eq!(measurement.outcome, FixtureIoOutcome::Completed);
        assert_eq!(measurement.bytes_read, input.len() as u64);
        assert_eq!(measurement.bytes_written, input.len() as u64);
        assert_eq!(measurement.chunks, 3);
        assert_eq!(measurement.peak_buffer_bytes, FIXTURE_IO_CHUNK_BYTES);
        assert_eq!(output, input);
        assert!(measurement.progress_events.len() >= 2);
        assert!(measurement
            .progress_events
            .iter()
            .all(|event| event.percent_basis_points.is_some()));
        assert_eq!(
            measurement
                .progress_events
                .last()
                .map(|event| event.completed_bytes),
            Some(input.len() as u64)
        );
    }

    #[test]
    fn fixture_chunked_io_unknown_total_never_reports_percent() {
        let input = vec![0x5a; FIXTURE_IO_CHUNK_BYTES + 3];
        let cancellation = OperationCancellation::new();
        let mut output = Vec::new();

        let measurement = measure_fixture_chunked_io(
            std::io::Cursor::new(input.clone()),
            &mut output,
            "op-progress-unknown-total",
            ProgressPhase::Verifying,
            None,
            &cancellation,
            false,
            FixtureIoConfig::new(Duration::ZERO, Duration::ZERO),
        )
        .unwrap();

        assert_eq!(measurement.outcome, FixtureIoOutcome::Completed);
        assert_eq!(measurement.bytes_written, input.len() as u64);
        assert!(measurement
            .progress_events
            .iter()
            .all(|event| event.percent_basis_points.is_none()));
        assert_eq!(output, input);
    }

    #[test]
    fn fixture_chunked_io_cancellation_before_write_leaves_staging_empty() {
        let cancellation = OperationCancellation::new();
        cancellation.request();
        let mut output = Vec::new();

        let measurement = measure_fixture_chunked_io(
            std::io::Cursor::new(vec![1_u8; FIXTURE_IO_CHUNK_BYTES]),
            &mut output,
            "op-progress-cancelled",
            ProgressPhase::Copying,
            Some(FIXTURE_IO_CHUNK_BYTES as u64),
            &cancellation,
            true,
            FixtureIoConfig::default(),
        )
        .unwrap();

        assert_eq!(measurement.outcome, FixtureIoOutcome::CancelledBeforeWrite);
        assert_eq!(measurement.bytes_read, 0);
        assert_eq!(measurement.bytes_written, 0);
        assert!(output.is_empty());
    }

    #[test]
    fn progress_sidecar_round_trips_by_operation_id_without_overgrowing() {
        let root = tempfile::tempdir().unwrap();
        let first = ProgressSnapshot::new("op-100-1", ProgressPhase::Copying, 10, Some(100), true);
        let second =
            ProgressSnapshot::new("op-100-1", ProgressPhase::Hashing, 100, Some(100), true);

        persist_progress_snapshot(root.path(), &first).unwrap();
        persist_progress_snapshot(root.path(), &second).unwrap();

        assert_eq!(
            read_progress_snapshot(root.path(), "op-100-100").unwrap(),
            None
        );
        assert_eq!(
            read_progress_snapshot(root.path(), "op-100-1").unwrap(),
            Some(second)
        );
        assert_eq!(
            fs::read_dir(root.path().join("progress")).unwrap().count(),
            1,
            "同一 operation_id 只保留最新快照"
        );
    }

    #[test]
    fn progress_sidecar_rejects_path_injection() {
        let root = tempfile::tempdir().unwrap();
        assert!(persist_progress_snapshot(
            root.path(),
            &ProgressSnapshot::new("op-../outside", ProgressPhase::Failed, 0, None, false)
        )
        .is_err());
        assert!(read_progress_snapshot(root.path(), "../outside").is_err());
    }

    #[test]
    fn progress_sidecar_rejects_symlinked_parent_chain() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let recovery_link = root.path().join("recovery-link");
        #[cfg(windows)]
        let link_result = std::os::windows::fs::symlink_dir(outside.path(), &recovery_link);
        #[cfg(unix)]
        let link_result = std::os::unix::fs::symlink(outside.path(), &recovery_link);
        #[cfg(not(any(windows, unix)))]
        return;
        if link_result.is_err() {
            return;
        }

        let snapshot = ProgressSnapshot::new("op-1-1", ProgressPhase::Copying, 1, None, true);
        assert!(persist_progress_snapshot(&recovery_link, &snapshot).is_err());
        assert!(!outside.path().join("progress").exists());
    }

    #[test]
    fn progress_sidecar_rejects_snapshot_for_different_operation() {
        let root = tempfile::tempdir().unwrap();
        let progress_dir = root.path().join("progress");
        fs::create_dir_all(&progress_dir).unwrap();
        let path = progress_dir.join("op-1-1.json");
        let other = ProgressSnapshot::new("op-2-2", ProgressPhase::Copying, 1, None, true);
        fs::write(path, serde_json::to_vec(&other).unwrap()).unwrap();

        assert!(read_progress_snapshot(root.path(), "op-1-1").is_err());
    }
}
