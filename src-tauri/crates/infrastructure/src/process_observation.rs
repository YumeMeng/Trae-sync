//! T08 TRAE 进程观测实现。
//!
//! Windows 实现只在“唯一 TRAE 候选 + 目标数据库占用”同时成立时报告运行中；
//! 其他情况全部保守失败，避免把错误进程当成目标进程。

use std::path::Path;

#[cfg(windows)]
use std::collections::{HashMap, HashSet};

#[cfg(windows)]
use std::path::PathBuf;

use traesync_ports::{
    ProcessControllerPort, ProcessIdentity, ProcessMatchEvidence, ProcessObservation,
    ProcessObservationStatus,
};

// Work CN 安装包实际使用带空格的产品名；保留 trae.exe 兼容旧版安装包。
const TRAE_EXECUTABLE_NAMES: &[&str] = &["trae.exe", "trae solo cn.exe"];

#[derive(Debug, Clone)]
pub struct WorkCnProcessController {
    executable_names: Vec<String>,
}

impl Default for WorkCnProcessController {
    fn default() -> Self {
        Self {
            executable_names: TRAE_EXECUTABLE_NAMES
                .iter()
                .map(|name| name.to_ascii_lowercase())
                .collect(),
        }
    }
}

impl WorkCnProcessController {
    pub fn with_executable_names<I, S>(names: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self {
            executable_names: names
                .into_iter()
                .map(Into::into)
                .map(|name| name.to_ascii_lowercase())
                .collect(),
        }
    }

    fn observation(
        &self,
        data_location_id: &str,
        status: ProcessObservationStatus,
        candidates: Vec<ProcessIdentity>,
        evidence: Vec<ProcessMatchEvidence>,
    ) -> ProcessObservation {
        ProcessObservation {
            status,
            data_location_id: data_location_id.to_string(),
            candidates,
            evidence,
        }
    }
}

impl ProcessControllerPort for WorkCnProcessController {
    fn observe(
        &self,
        data_location_id: &str,
        data_root: &Path,
        db_relative_path: &str,
    ) -> ProcessObservation {
        #[cfg(windows)]
        {
            return observe_windows(self, data_location_id, data_root, db_relative_path);
        }

        #[cfg(not(windows))]
        {
            let _ = (data_root, db_relative_path);
            self.observation(
                data_location_id,
                ProcessObservationStatus::Unknown,
                Vec::new(),
                vec![ProcessMatchEvidence {
                    code: "platform_not_supported".to_string(),
                    value: "windows_only".to_string(),
                }],
            )
        }
    }
}

/// fixture 和组合根单元测试使用固定观测，不访问操作系统进程表。
#[derive(Debug, Clone)]
pub struct FixedProcessController {
    status: ProcessObservationStatus,
    candidates: Vec<ProcessIdentity>,
    evidence: Vec<ProcessMatchEvidence>,
}

impl FixedProcessController {
    pub fn new(status: ProcessObservationStatus) -> Self {
        Self {
            status,
            candidates: Vec::new(),
            evidence: vec![ProcessMatchEvidence {
                code: "fixture_observation".to_string(),
                value: "fixed".to_string(),
            }],
        }
    }

    pub fn not_running() -> Self {
        Self::new(ProcessObservationStatus::NotRunning)
    }
}

impl ProcessControllerPort for FixedProcessController {
    fn observe(
        &self,
        data_location_id: &str,
        _data_root: &Path,
        _db_relative_path: &str,
    ) -> ProcessObservation {
        ProcessObservation {
            status: self.status,
            data_location_id: data_location_id.to_string(),
            candidates: self.candidates.clone(),
            evidence: self.evidence.clone(),
        }
    }
}

#[cfg(windows)]
fn observe_windows(
    controller: &WorkCnProcessController,
    data_location_id: &str,
    data_root: &Path,
    db_relative_path: &str,
) -> ProcessObservation {
    let process_table = match enumerate_process_table() {
        Ok(process_table) => process_table,
        Err(code) => {
            return controller.observation(
                data_location_id,
                ProcessObservationStatus::Unknown,
                Vec::new(),
                vec![ProcessMatchEvidence {
                    code: "process_enumeration_failed".to_string(),
                    value: code,
                }],
            )
        }
    };
    let candidates = match enumerate_trae_processes(&process_table, &controller.executable_names) {
        Ok(candidates) => candidates,
        Err(code) => {
            return controller.observation(
                data_location_id,
                ProcessObservationStatus::Unknown,
                Vec::new(),
                vec![ProcessMatchEvidence {
                    code: "process_enumeration_failed".to_string(),
                    value: code,
                }],
            )
        }
    };

    let database_path = data_root.join(db_relative_path);
    let target_resources = target_resource_paths(&database_path);
    let database_lock = database_open_denied(&database_path);
    let mut evidence = vec![ProcessMatchEvidence {
        code: "target_data_location_id".to_string(),
        value: data_location_id.to_string(),
    }];

    evidence.push(ProcessMatchEvidence {
        code: "target_resource_count".to_string(),
        value: target_resources.len().to_string(),
    });
    for resource in &target_resources {
        if let Some(name) = resource.file_name() {
            evidence.push(ProcessMatchEvidence {
                code: "target_resource".to_string(),
                value: name.to_string_lossy().into_owned(),
            });
        }
    }

    // 仅凭“数据库被锁”和“存在 trae.exe”不能证明二者属于同一目标位置。
    // Restart Manager 会按目标文件返回实际占用进程；无法取得这份证据时保持未知。
    let affected_processes = match restart_manager_processes(&target_resources, &process_table) {
        Ok(processes) => processes,
        Err(code) => {
            evidence.push(ProcessMatchEvidence {
                code: "target_file_process_query_failed".to_string(),
                value: code,
            });
            if candidates.is_empty() && database_lock == Some(false) {
                return controller.observation(
                    data_location_id,
                    ProcessObservationStatus::NotRunning,
                    candidates
                        .into_iter()
                        .map(|candidate| candidate.identity)
                        .collect(),
                    evidence,
                );
            }
            return controller.observation(
                data_location_id,
                ProcessObservationStatus::Unknown,
                candidates
                    .into_iter()
                    .map(|candidate| candidate.identity)
                    .collect(),
                evidence,
            );
        }
    };
    // 扫描复制阶段本进程会短暂持有源数据库句柄；该占用不是 TRAE 运行状态，
    // 必须从 Restart Manager 结果中排除，否则只读扫描会被自身误判为未知占用。
    let affected_processes = exclude_process_holders(affected_processes, std::process::id());
    evidence.push(ProcessMatchEvidence {
        code: "target_file_holder_count".to_string(),
        value: affected_processes.len().to_string(),
    });
    evidence.push(ProcessMatchEvidence {
        code: "trae_candidate_count".to_string(),
        value: candidates.len().to_string(),
    });
    for process in &affected_processes {
        evidence.push(ProcessMatchEvidence {
            code: "target_file_holder_pid".to_string(),
            value: process.pid().to_string(),
        });
        evidence.push(ProcessMatchEvidence {
            code: "target_file_holder_process_identity".to_string(),
            value: process
                .identity
                .executable_path
                .as_deref()
                .unwrap_or("unknown")
                .to_string(),
        });
    }

    let status = classify_target_processes(
        &candidates,
        &affected_processes,
        database_lock,
        &process_table,
    );
    evidence.push(ProcessMatchEvidence {
        code: "target_process_match".to_string(),
        value: match status {
            ProcessObservationStatus::Running => "unique_trae_holder",
            ProcessObservationStatus::Ambiguous => "ambiguous",
            ProcessObservationStatus::WrongTarget => "non_trae_holder",
            ProcessObservationStatus::NotRunning => "none",
            ProcessObservationStatus::Unknown => "unknown",
        }
        .to_string(),
    });
    controller.observation(
        data_location_id,
        status,
        candidates
            .into_iter()
            .map(|candidate| candidate.identity)
            .collect(),
        evidence,
    )
}

#[cfg(windows)]
#[derive(Debug, Clone, PartialEq, Eq)]
struct RestartManagerProcess {
    pid: u32,
    creation_time_unix_ms: Option<u64>,
    identity: ProcessIdentity,
}

#[cfg(windows)]
impl RestartManagerProcess {
    fn pid(&self) -> u32 {
        self.pid
    }
}

#[cfg(windows)]
#[derive(Debug, Clone, PartialEq, Eq)]
struct ProcessTableEntry {
    pid: u32,
    parent_pid: u32,
    executable_name: String,
}

#[cfg(windows)]
#[derive(Debug, Clone, PartialEq, Eq)]
struct TraeProcessCandidate {
    identity: ProcessIdentity,
}

#[cfg(windows)]
fn classify_target_processes(
    candidates: &[TraeProcessCandidate],
    affected_processes: &[RestartManagerProcess],
    database_lock: Option<bool>,
    process_table: &[ProcessTableEntry],
) -> ProcessObservationStatus {
    if affected_processes.is_empty() {
        return match (candidates.is_empty(), database_lock) {
            (true, Some(false)) => ProcessObservationStatus::NotRunning,
            _ => ProcessObservationStatus::Unknown,
        };
    }

    let mut matching_candidates = HashSet::new();
    let mut has_wrong_holder = false;
    let mut has_unknown_holder = false;

    for affected in affected_processes {
        // Restart Manager 的时间戳必须与当前 PID 重新读取的身份一致，防止 PID 复用。
        if affected.identity.pid != affected.pid
            || affected.identity.creation_time_unix_ms != affected.creation_time_unix_ms
            || affected.creation_time_unix_ms.is_none()
        {
            has_unknown_holder = true;
            continue;
        }

        let Some(holder_path) = affected.identity.executable_path.as_deref() else {
            has_unknown_holder = true;
            continue;
        };

        let mut holder_matches = Vec::new();
        for (index, candidate) in candidates.iter().enumerate() {
            if candidate.identity.creation_time_unix_ms.is_none() {
                has_unknown_holder = true;
                continue;
            }
            let Some(candidate_path) = candidate.identity.executable_path.as_deref() else {
                has_unknown_holder = true;
                continue;
            };
            if !candidate_path.eq_ignore_ascii_case(holder_path) {
                continue;
            }

            match is_same_or_descendant(affected.pid, candidate.identity.pid, process_table) {
                Some(true) => holder_matches.push(index),
                Some(false) => {}
                None => has_unknown_holder = true,
            }
        }

        if holder_matches.is_empty() {
            if candidates
                .iter()
                .any(|candidate| candidate.identity.executable_path.is_some())
            {
                has_wrong_holder = true;
            } else {
                has_unknown_holder = true;
            }
        } else {
            matching_candidates.extend(holder_matches);
        }
    }

    if has_unknown_holder {
        ProcessObservationStatus::Unknown
    } else if has_wrong_holder {
        ProcessObservationStatus::WrongTarget
    } else if matching_candidates.is_empty() {
        ProcessObservationStatus::WrongTarget
    } else if candidates.len() != 1 || matching_candidates.len() != 1 {
        // 多个独立 TRAE 根进程即使只有一个当前占用目标文件，也不能猜测归属。
        ProcessObservationStatus::Ambiguous
    } else {
        ProcessObservationStatus::Running
    }
}

#[cfg(windows)]
fn is_same_or_descendant(
    process_pid: u32,
    root_pid: u32,
    process_table: &[ProcessTableEntry],
) -> Option<bool> {
    if process_pid == root_pid {
        return Some(true);
    }

    let by_pid: HashMap<u32, &ProcessTableEntry> = process_table
        .iter()
        .map(|entry| (entry.pid, entry))
        .collect();
    let mut visited = HashSet::new();
    let mut current_pid = process_pid;

    loop {
        if !visited.insert(current_pid) {
            return None;
        }
        let entry = by_pid.get(&current_pid)?;
        let parent_pid = entry.parent_pid;
        if parent_pid == root_pid {
            return Some(true);
        }
        if parent_pid == 0 || parent_pid == current_pid {
            return Some(false);
        }
        current_pid = parent_pid;
    }
}

#[cfg(windows)]
fn restart_manager_processes(
    paths: &[PathBuf],
    process_table: &[ProcessTableEntry],
) -> Result<Vec<RestartManagerProcess>, String> {
    use std::mem::zeroed;
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Foundation::{ERROR_MORE_DATA, ERROR_SUCCESS};
    use windows_sys::Win32::System::RestartManager::{
        RmEndSession, RmGetList, RmRegisterResources, RmStartSession, CCH_RM_SESSION_KEY,
        RM_PROCESS_INFO,
    };

    let mut session = 0_u32;
    let mut session_key = [0_u16; CCH_RM_SESSION_KEY as usize + 1];
    let status = unsafe { RmStartSession(&mut session, 0, session_key.as_mut_ptr()) };
    if status != ERROR_SUCCESS {
        return Err(format!("rm_start_session_{status}"));
    }

    let result = (|| {
        let wide_paths: Vec<Vec<u16>> = paths
            .iter()
            .map(|path| path.as_os_str().encode_wide().chain(Some(0)).collect())
            .collect();
        let resources: Vec<*const u16> = wide_paths.iter().map(|path| path.as_ptr()).collect();
        let status = unsafe {
            RmRegisterResources(
                session,
                resources.len() as u32,
                resources.as_ptr(),
                0,
                std::ptr::null(),
                0,
                std::ptr::null(),
            )
        };
        if status != ERROR_SUCCESS {
            return Err(format!("rm_register_resources_{status}"));
        }

        let mut needed = 0_u32;
        let mut count = 0_u32;
        let mut reboot_reasons = 0_u32;
        let status = unsafe {
            RmGetList(
                session,
                &mut needed,
                &mut count,
                std::ptr::null_mut(),
                &mut reboot_reasons,
            )
        };
        if status != ERROR_SUCCESS && status != ERROR_MORE_DATA {
            return Err(format!("rm_get_list_{status}"));
        }
        if needed == 0 {
            return Ok(Vec::new());
        }

        let mut processes = vec![unsafe { zeroed::<RM_PROCESS_INFO>() }; needed as usize];
        count = needed;
        let status = unsafe {
            RmGetList(
                session,
                &mut needed,
                &mut count,
                processes.as_mut_ptr(),
                &mut reboot_reasons,
            )
        };
        if status != ERROR_SUCCESS {
            return Err(format!("rm_get_list_{status}"));
        }

        Ok(deduplicate_restart_manager_processes(
            processes
                .into_iter()
                .take(count as usize)
                .map(|process| {
                    let pid = process.Process.dwProcessId;
                    let table_entry = process_table.iter().find(|entry| entry.pid == pid);
                    let executable_name = table_entry
                        .map(|entry| entry.executable_name.as_str())
                        .unwrap_or("");
                    RestartManagerProcess {
                        pid,
                        creation_time_unix_ms: filetime_to_unix_ms(
                            &process.Process.ProcessStartTime,
                        ),
                        identity: read_process_identity(pid, executable_name),
                    }
                })
                .collect(),
        ))
    })();

    let _ = unsafe { RmEndSession(session) };
    result
}

#[cfg(windows)]
fn target_resource_paths(database_path: &Path) -> Vec<PathBuf> {
    let mut paths = vec![database_path.to_path_buf()];
    for suffix in ["-wal", "-shm"] {
        let mut sidecar = database_path.as_os_str().to_os_string();
        sidecar.push(suffix);
        let sidecar = PathBuf::from(sidecar);
        if sidecar.is_file() {
            paths.push(sidecar);
        }
    }
    paths
}

#[cfg(windows)]
fn deduplicate_restart_manager_processes(
    processes: Vec<RestartManagerProcess>,
) -> Vec<RestartManagerProcess> {
    let mut unique = Vec::with_capacity(processes.len());
    for process in processes {
        if !unique.iter().any(|existing: &RestartManagerProcess| {
            existing.pid == process.pid
                && existing.creation_time_unix_ms == process.creation_time_unix_ms
        }) {
            unique.push(process);
        }
    }
    unique
}

#[cfg(windows)]
fn exclude_process_holders(
    processes: Vec<RestartManagerProcess>,
    excluded_pid: u32,
) -> Vec<RestartManagerProcess> {
    processes
        .into_iter()
        .filter(|process| process.pid != excluded_pid)
        .collect()
}

#[cfg(windows)]
fn enumerate_process_table() -> Result<Vec<ProcessTableEntry>, String> {
    use std::mem::{size_of, zeroed};
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
        TH32CS_SNAPPROCESS,
    };

    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    if snapshot == INVALID_HANDLE_VALUE {
        return Err("snapshot_unavailable".to_string());
    }

    let result = (|| {
        let mut entry: PROCESSENTRY32W = unsafe { zeroed() };
        entry.dwSize = size_of::<PROCESSENTRY32W>() as u32;
        let mut process_table = Vec::new();
        let mut has_entry = unsafe { Process32FirstW(snapshot, &mut entry) } != 0;
        while has_entry {
            process_table.push(ProcessTableEntry {
                pid: entry.th32ProcessID,
                parent_pid: entry.th32ParentProcessID,
                executable_name: utf16_to_string(&entry.szExeFile),
            });
            has_entry = unsafe { Process32NextW(snapshot, &mut entry) } != 0;
        }
        Ok(process_table)
    })();

    unsafe {
        CloseHandle(snapshot);
    }
    result
}

#[cfg(windows)]
fn enumerate_trae_processes(
    process_table: &[ProcessTableEntry],
    executable_names: &[String],
) -> Result<Vec<TraeProcessCandidate>, String> {
    // Electron 的渲染器和服务子进程沿用同一个 exe 名称；只保留没有同名父进程的根进程，
    // 同时保留两个独立根进程，避免把合法双实例误判成单实例。
    let matching_pids: HashSet<u32> = process_table
        .iter()
        .filter(|entry| {
            executable_names
                .iter()
                .any(|expected| expected == &entry.executable_name.to_ascii_lowercase())
        })
        .map(|entry| entry.pid)
        .collect();

    Ok(process_table
        .iter()
        .filter(|entry| {
            matching_pids.contains(&entry.pid)
                && is_root_trae_candidate(entry.parent_pid, &matching_pids)
        })
        .map(|entry| TraeProcessCandidate {
            identity: read_process_identity(entry.pid, &entry.executable_name),
        })
        .collect())
}

#[cfg(windows)]
fn is_root_trae_candidate(parent_pid: u32, matching_pids: &HashSet<u32>) -> bool {
    !matching_pids.contains(&parent_pid)
}

#[cfg(windows)]
fn process_executable_identity(path: Option<&str>, executable_name: &str) -> String {
    path.map(str::to_ascii_lowercase)
        .unwrap_or_else(|| executable_name.to_ascii_lowercase())
}

#[cfg(windows)]
fn read_process_identity(pid: u32, executable_name: &str) -> ProcessIdentity {
    use std::mem::zeroed;
    use windows_sys::Win32::Foundation::{CloseHandle, FILETIME};
    use windows_sys::Win32::System::Threading::{
        GetProcessTimes, OpenProcess, QueryFullProcessImageNameW, PROCESS_QUERY_LIMITED_INFORMATION,
    };

    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if handle.is_null() {
        return ProcessIdentity {
            pid,
            creation_time_unix_ms: None,
            executable_path: None,
            executable_identity: Some(process_executable_identity(None, executable_name)),
        };
    }

    let mut creation: FILETIME = unsafe { zeroed() };
    let mut exit: FILETIME = unsafe { zeroed() };
    let mut kernel: FILETIME = unsafe { zeroed() };
    let mut user: FILETIME = unsafe { zeroed() };
    let creation_time_unix_ms = if unsafe {
        GetProcessTimes(handle, &mut creation, &mut exit, &mut kernel, &mut user)
    } != 0
    {
        filetime_to_unix_ms(&creation)
    } else {
        None
    };

    let mut buffer = vec![0u16; 32_768];
    let mut length = buffer.len() as u32;
    let executable_path = if unsafe {
        QueryFullProcessImageNameW(handle, 0, buffer.as_mut_ptr(), &mut length)
    } != 0
    {
        Some(String::from_utf16_lossy(&buffer[..length as usize]))
    } else {
        None
    };

    unsafe {
        CloseHandle(handle);
    }

    let executable_identity =
        process_executable_identity(executable_path.as_deref(), executable_name);
    ProcessIdentity {
        pid,
        creation_time_unix_ms,
        executable_path,
        executable_identity: Some(executable_identity),
    }
}

#[cfg(windows)]
fn database_open_denied(path: &Path) -> Option<bool> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Foundation::{
        CloseHandle, GetLastError, ERROR_LOCK_VIOLATION, ERROR_SHARING_VIOLATION, GENERIC_READ,
        INVALID_HANDLE_VALUE,
    };
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
        OPEN_EXISTING,
    };

    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let handle = unsafe {
        CreateFileW(
            wide.as_ptr(),
            GENERIC_READ,
            // 只读探针必须允许 SQLite/Chromium 的正常共享打开；独占占用仍由
            // Restart Manager 返回的实际文件持有者进行归属判断。
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            std::ptr::null(),
            OPEN_EXISTING,
            FILE_ATTRIBUTE_NORMAL,
            std::ptr::null_mut(),
        )
    };
    if handle != INVALID_HANDLE_VALUE {
        unsafe {
            CloseHandle(handle);
        }
        return Some(false);
    }

    let error = unsafe { GetLastError() };
    if error == ERROR_SHARING_VIOLATION || error == ERROR_LOCK_VIOLATION {
        Some(true)
    } else {
        None
    }
}

#[cfg(windows)]
fn filetime_to_unix_ms(filetime: &windows_sys::Win32::Foundation::FILETIME) -> Option<u64> {
    let ticks = ((filetime.dwHighDateTime as u64) << 32) | filetime.dwLowDateTime as u64;
    const WINDOWS_TO_UNIX_EPOCH_TICKS: u64 = 116_444_736_000_000_000;
    ticks
        .checked_sub(WINDOWS_TO_UNIX_EPOCH_TICKS)
        .map(|value| value / 10_000)
}

#[cfg(windows)]
fn utf16_to_string(value: &[u16]) -> String {
    let end = value
        .iter()
        .position(|character| *character == 0)
        .unwrap_or(value.len());
    String::from_utf16_lossy(&value[..end])
}

#[cfg(test)]
mod tests {
    use super::*;
    use traesync_ports::ProcessObservationStatus;

    #[test]
    fn fixed_controller_binds_observation_to_requested_location() {
        let controller = FixedProcessController::not_running();
        let observation = controller.observe("loc-test", Path::new("C:\\fixture"), "database.db");
        assert_eq!(observation.status, ProcessObservationStatus::NotRunning);
        assert_eq!(observation.data_location_id, "loc-test");
        assert!(observation.is_safe_to_scan());
    }

    #[test]
    fn non_windows_controller_fails_closed() {
        #[cfg(not(windows))]
        {
            let observation = WorkCnProcessController::default().observe(
                "loc-test",
                Path::new("C:\\fixture"),
                "database.db",
            );
            assert_eq!(observation.status, ProcessObservationStatus::Unknown);
            assert!(!observation.is_safe_to_scan());
        }
    }

    #[cfg(windows)]
    #[test]
    fn target_process_match_requires_unique_candidate_and_file_holder_identity() {
        let candidate = test_candidate(101, 7, 1);
        let matching_holder = test_holder(101, 7, 1);
        let wrong_holder = test_holder(202, 8, 1);
        let unrelated_table = vec![
            test_process(1, 0),
            test_process(101, 1),
            test_process(202, 1),
        ];

        assert_eq!(
            classify_target_processes(
                std::slice::from_ref(&candidate),
                std::slice::from_ref(&matching_holder),
                Some(true),
                &[],
            ),
            ProcessObservationStatus::Running
        );
        assert_eq!(
            classify_target_processes(
                std::slice::from_ref(&candidate),
                std::slice::from_ref(&wrong_holder),
                Some(true),
                &unrelated_table,
            ),
            ProcessObservationStatus::WrongTarget
        );
        assert_eq!(
            classify_target_processes(
                &[candidate.clone(), test_candidate(202, 8, 1)],
                std::slice::from_ref(&matching_holder),
                Some(true),
                &[
                    test_process(1, 0),
                    test_process(101, 1),
                    test_process(202, 1),
                ],
            ),
            ProcessObservationStatus::Ambiguous
        );
        assert_eq!(
            classify_target_processes(
                std::slice::from_ref(&candidate),
                &[matching_holder, test_holder(303, 9, 101)],
                Some(true),
                &[
                    test_process(1, 0),
                    test_process(101, 1),
                    test_process(303, 101),
                ],
            ),
            ProcessObservationStatus::Running
        );
    }

    #[cfg(windows)]
    #[test]
    fn target_process_match_requires_creation_time_on_both_sides() {
        let candidate = test_candidate(101, 7, 1);
        let holder = test_holder(101, 7, 1);

        assert_eq!(
            classify_target_processes(
                std::slice::from_ref(&candidate),
                &[RestartManagerProcess {
                    pid: 101,
                    creation_time_unix_ms: None,
                    identity: ProcessIdentity {
                        pid: 101,
                        creation_time_unix_ms: None,
                        executable_path: Some(TEST_TRAE_PATH.to_string()),
                        executable_identity: Some(TEST_TRAE_IDENTITY.to_string()),
                    },
                }],
                Some(true),
                &[],
            ),
            ProcessObservationStatus::Unknown
        );
        assert_eq!(
            classify_target_processes(
                &[TraeProcessCandidate {
                    identity: ProcessIdentity {
                        creation_time_unix_ms: None,
                        ..candidate.identity
                    },
                }],
                std::slice::from_ref(&holder),
                Some(true),
                &[],
            ),
            ProcessObservationStatus::Unknown
        );
    }

    #[cfg(windows)]
    #[test]
    fn legal_electron_child_holder_belongs_to_unique_trae_root() {
        let candidate = test_candidate(101, 7, 1);
        let holder = test_holder(303, 9, 101);
        let table = vec![
            test_process(1, 0),
            test_process(101, 1),
            test_process(303, 101),
        ];

        assert_eq!(
            classify_target_processes(
                std::slice::from_ref(&candidate),
                std::slice::from_ref(&holder),
                Some(true),
                &table,
            ),
            ProcessObservationStatus::Running
        );
    }

    #[cfg(windows)]
    #[test]
    fn unrelated_same_executable_holder_is_wrong_target() {
        let candidate = test_candidate(101, 7, 1);
        let holder = test_holder(303, 9, 202);
        let table = vec![
            test_process(1, 0),
            test_process(101, 1),
            test_process(202, 1),
            test_process(303, 202),
        ];

        assert_eq!(
            classify_target_processes(
                std::slice::from_ref(&candidate),
                std::slice::from_ref(&holder),
                Some(true),
                &table,
            ),
            ProcessObservationStatus::WrongTarget
        );
    }

    #[cfg(windows)]
    #[test]
    fn no_holder_is_not_running_only_when_database_is_readable() {
        assert_eq!(
            classify_target_processes(&[], &[], Some(false), &[]),
            ProcessObservationStatus::NotRunning
        );
        assert_eq!(
            classify_target_processes(&[], &[], Some(true), &[]),
            ProcessObservationStatus::Unknown
        );
        assert_eq!(
            classify_target_processes(
                &[TraeProcessCandidate {
                    identity: ProcessIdentity {
                        pid: 101,
                        creation_time_unix_ms: None,
                        executable_path: None,
                        executable_identity: Some("trae.exe".to_string()),
                    },
                }],
                &[],
                Some(false),
                &[],
            ),
            ProcessObservationStatus::Unknown
        );
        assert_eq!(
            classify_target_processes(
                &[test_candidate(101, 7, 1), test_candidate(202, 8, 1),],
                &[],
                Some(false),
                &[],
            ),
            ProcessObservationStatus::Unknown
        );
    }

    #[cfg(windows)]
    #[test]
    fn current_process_holder_is_ignored_for_read_only_scan() {
        let current_process = test_holder(101, 7, 1);
        let remaining = exclude_process_holders(vec![current_process], 101);

        assert!(remaining.is_empty());
        assert_eq!(
            classify_target_processes(&[], &remaining, Some(false), &[]),
            ProcessObservationStatus::NotRunning
        );
    }

    #[cfg(windows)]
    #[test]
    fn external_holder_remains_a_boundary_failure_after_current_process_filter() {
        let current_process = test_holder(101, 7, 1);
        let external_process = test_holder(202, 8, 1);
        let remaining =
            exclude_process_holders(vec![current_process, external_process.clone()], 101);

        assert_eq!(remaining, vec![external_process]);
        assert_eq!(
            classify_target_processes(&[], &remaining, Some(true), &[]),
            ProcessObservationStatus::Unknown
        );
    }

    #[cfg(windows)]
    #[test]
    fn executable_identity_prefers_full_path_and_falls_back_to_name() {
        assert_eq!(
            process_executable_identity(Some("C:\\TRAE\\Trae.exe"), "trae.exe"),
            "c:\\trae\\trae.exe"
        );
        assert_eq!(process_executable_identity(None, "Trae.EXE"), "trae.exe");
    }

    #[cfg(windows)]
    #[test]
    fn default_controller_accepts_work_cn_solo_executable_name() {
        assert!(TRAE_EXECUTABLE_NAMES.contains(&"trae solo cn.exe"));
    }

    #[cfg(windows)]
    #[test]
    fn child_electron_process_is_not_a_second_trae_instance() {
        let matching_pids = HashSet::from([100_u32, 200_u32]);
        assert!(is_root_trae_candidate(1, &matching_pids));
        assert!(!is_root_trae_candidate(100, &matching_pids));
        assert!(!is_root_trae_candidate(200, &matching_pids));
    }

    #[cfg(windows)]
    #[test]
    fn target_resource_paths_include_existing_sqlite_sidecars() {
        let directory = tempfile::tempdir().unwrap();
        let database = directory.path().join("database.db");
        std::fs::write(&database, b"db").unwrap();

        assert_eq!(target_resource_paths(&database).len(), 1);

        std::fs::write(directory.path().join("database.db-wal"), b"wal").unwrap();
        std::fs::write(directory.path().join("database.db-shm"), b"shm").unwrap();
        let names = target_resource_paths(&database)
            .into_iter()
            .map(|path| path.file_name().unwrap().to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert_eq!(
            names,
            vec!["database.db", "database.db-wal", "database.db-shm"]
        );
    }

    #[cfg(windows)]
    #[test]
    fn shared_database_open_is_not_treated_as_exclusive_lock() {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::Foundation::{CloseHandle, GENERIC_READ, INVALID_HANDLE_VALUE};
        use windows_sys::Win32::Storage::FileSystem::{
            CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_DELETE, FILE_SHARE_READ,
            FILE_SHARE_WRITE, OPEN_EXISTING,
        };

        let directory = tempfile::tempdir().unwrap();
        let database = directory.path().join("database.db");
        std::fs::write(&database, b"shared database").unwrap();
        let wide: Vec<u16> = database.as_os_str().encode_wide().chain(Some(0)).collect();
        let shared_handle = unsafe {
            CreateFileW(
                wide.as_ptr(),
                GENERIC_READ,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                std::ptr::null(),
                OPEN_EXISTING,
                FILE_ATTRIBUTE_NORMAL,
                std::ptr::null_mut(),
            )
        };
        assert_ne!(shared_handle, INVALID_HANDLE_VALUE);

        // SQLite/Chromium 常规共享打开不等于独占写入；探针应仍报告文件可读。
        assert_eq!(database_open_denied(&database), Some(false));

        unsafe {
            CloseHandle(shared_handle);
        }
    }

    #[cfg(windows)]
    #[test]
    fn restart_manager_processes_are_deduplicated_by_pid_and_creation_time() {
        let process = RestartManagerProcess {
            pid: 101,
            creation_time_unix_ms: Some(7),
            identity: test_identity(101, 7),
        };
        let unique = deduplicate_restart_manager_processes(vec![process.clone(), process.clone()]);
        assert_eq!(unique, vec![process]);
    }

    #[cfg(windows)]
    const TEST_TRAE_PATH: &str = "C:\\TRAE\\trae.exe";

    #[cfg(windows)]
    const TEST_TRAE_IDENTITY: &str = "c:\\trae\\trae.exe";

    #[cfg(windows)]
    fn test_identity(pid: u32, creation_time_unix_ms: u64) -> ProcessIdentity {
        ProcessIdentity {
            pid,
            creation_time_unix_ms: Some(creation_time_unix_ms),
            executable_path: Some(TEST_TRAE_PATH.to_string()),
            executable_identity: Some(TEST_TRAE_IDENTITY.to_string()),
        }
    }

    #[cfg(windows)]
    fn test_candidate(
        pid: u32,
        creation_time_unix_ms: u64,
        _parent_pid: u32,
    ) -> TraeProcessCandidate {
        TraeProcessCandidate {
            identity: test_identity(pid, creation_time_unix_ms),
        }
    }

    #[cfg(windows)]
    fn test_holder(
        pid: u32,
        creation_time_unix_ms: u64,
        _parent_pid: u32,
    ) -> RestartManagerProcess {
        RestartManagerProcess {
            pid,
            creation_time_unix_ms: Some(creation_time_unix_ms),
            identity: test_identity(pid, creation_time_unix_ms),
        }
    }

    #[cfg(windows)]
    fn test_process(pid: u32, parent_pid: u32) -> ProcessTableEntry {
        ProcessTableEntry {
            pid,
            parent_pid,
            executable_name: "trae.exe".to_string(),
        }
    }
}
