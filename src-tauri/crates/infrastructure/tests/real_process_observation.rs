#![cfg(windows)]

//! T08 真实隔离 TRAE 进程观测验收。
//!
//! 测试只启动隔离 Profile，验证真实进程身份和目标数据库占用；不登录、不发送消息。

use std::env;
use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::ptr::null_mut;
use std::thread;
use std::time::{Duration, Instant};

use serde_json::json;
use traesync_infrastructure::{capture_location_identity, WorkCnProcessController};
use traesync_ports::{ProcessControllerPort, ProcessObservationStatus};
use windows_sys::Win32::Foundation::{CloseHandle, GENERIC_READ, HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Storage::FileSystem::{CreateFileW, FILE_ATTRIBUTE_NORMAL, OPEN_EXISTING};

fn required_env(name: &str) -> String {
    env::var(name).unwrap_or_else(|_| panic!("缺少真实进程验收环境变量：{name}"))
}

fn observe_until_status(
    controller: &WorkCnProcessController,
    data_location_id: &str,
    data_root: &Path,
    db_relative: &str,
    expected: ProcessObservationStatus,
) -> traesync_ports::ProcessObservation {
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut last = controller.observe(data_location_id, data_root, db_relative);
    while last.status != expected && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(250));
        last = controller.observe(data_location_id, data_root, db_relative);
    }
    last
}

struct ChildGuard {
    child: Option<Child>,
}

impl ChildGuard {
    fn spawn(executable: &Path, user_data_dir: &Path, port: &str) -> Self {
        let app_data = required_env("TRAE_SYNC_REAL_PROCESS_APPDATA");
        let local_app_data = required_env("TRAE_SYNC_REAL_PROCESS_LOCALAPPDATA");
        let user_profile = required_env("TRAE_SYNC_REAL_PROCESS_USERPROFILE");
        let child = Command::new(executable)
            .args([
                format!("--user-data-dir={}", user_data_dir.display()),
                "--disable-gpu".to_string(),
                format!("--remote-debugging-port={port}"),
            ])
            .env("APPDATA", app_data)
            .env("LOCALAPPDATA", local_app_data)
            .env("USERPROFILE", user_profile)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap_or_else(|error| panic!("启动隔离 TRAE 失败：{error}"));
        Self { child: Some(child) }
    }

    fn id(&self) -> u32 {
        self.child.as_ref().expect("隔离 TRAE 子进程已被取走").id()
    }
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

struct FileShareLock {
    handle: HANDLE,
}

impl FileShareLock {
    fn acquire(path: &Path) -> Self {
        let wide: Vec<u16> = OsStr::new(path.as_os_str())
            .encode_wide()
            .chain(Some(0))
            .collect();
        let handle = unsafe {
            CreateFileW(
                wide.as_ptr(),
                GENERIC_READ,
                0,
                null_mut(),
                OPEN_EXISTING,
                FILE_ATTRIBUTE_NORMAL,
                null_mut(),
            )
        };
        assert_ne!(
            handle,
            INVALID_HANDLE_VALUE,
            "无法为隔离数据库建立独占文件占用：{}",
            path.display()
        );
        Self { handle }
    }
}

impl Drop for FileShareLock {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.handle);
        }
    }
}

#[test]
#[ignore = "仅在用户授权的真实隔离副本上显式运行"]
fn real_isolated_process_observation_acceptance() {
    let executable = PathBuf::from(required_env("TRAE_SYNC_REAL_PROCESS_EXE"));
    let user_data_dir = PathBuf::from(required_env("TRAE_SYNC_REAL_PROCESS_USER_DATA_DIR"));
    let data_root = PathBuf::from(required_env("TRAE_SYNC_REAL_PROCESS_DATA_ROOT"));
    let db_relative = required_env("TRAE_SYNC_REAL_PROCESS_DB_RELATIVE");
    let port = required_env("TRAE_SYNC_REAL_PROCESS_PORT");
    let data_location_id = capture_location_identity(
        &traesync_infrastructure::PlatformFileIdentityProvider::new(),
        &data_root,
        &db_relative,
    )
    .unwrap_or_else(|error| panic!("隔离数据位置身份捕获失败：{error}"))
    .data_location_id;

    assert!(
        executable.is_file(),
        "TRAE 可执行文件不存在：{}",
        executable.display()
    );
    assert!(
        user_data_dir.is_dir(),
        "隔离 user-data-dir 不存在：{}",
        user_data_dir.display()
    );
    assert!(
        data_root.join(&db_relative).is_file(),
        "隔离目标数据库不存在：{}",
        data_root.join(&db_relative).display()
    );

    let child = ChildGuard::spawn(&executable, &user_data_dir, &port);
    let child_pid = child.id();
    let controller =
        WorkCnProcessController::with_executable_names(["TRAE SOLO CN.exe", "trae.exe"]);
    let observation = observe_until_status(
        &controller,
        &data_location_id,
        &data_root,
        &db_relative,
        ProcessObservationStatus::Running,
    );
    let child_seen = observation
        .candidates
        .iter()
        .any(|candidate| candidate.pid == child_pid);

    println!(
        "REAL_PROCESS_OBSERVATION={}",
        json!({
            "data_location_id": observation.data_location_id,
            "status": format!("{:?}", observation.status),
            "child_pid": child_pid,
            "child_seen": child_seen,
            "candidate_count": observation.candidates.len(),
            "candidate_identities": observation.candidates.iter().map(|candidate| {
                json!({
                    "pid": candidate.pid,
                    "executable_path": candidate.executable_path,
                    "executable_identity": candidate.executable_identity,
                })
            }).collect::<Vec<_>>(),
            "evidence": observation
                .evidence
                .iter()
                .map(|item| json!({"code": item.code, "value": item.value}))
                .collect::<Vec<_>>(),
        })
    );

    assert!(
        child_seen,
        "进程观测未识别本轮隔离 TRAE 主进程：{child_pid}"
    );
    assert_eq!(
        observation.status,
        ProcessObservationStatus::Running,
        "目标数据库未被唯一隔离 TRAE 进程占用：{:?}",
        observation.status
    );
}

#[test]
#[ignore = "仅在用户授权的真实隔离副本上显式运行"]
fn real_wrong_target_process_observation_acceptance() {
    let executable = PathBuf::from(required_env("TRAE_SYNC_REAL_PROCESS_EXE"));
    let user_data_dir = PathBuf::from(required_env("TRAE_SYNC_REAL_PROCESS_USER_DATA_DIR"));
    let data_root = PathBuf::from(required_env("TRAE_SYNC_REAL_PROCESS_DATA_ROOT"));
    let db_relative = required_env("TRAE_SYNC_REAL_PROCESS_DB_RELATIVE");
    let port = required_env("TRAE_SYNC_REAL_PROCESS_PORT");
    let database_path = data_root.join(&db_relative);

    assert!(
        executable.is_file(),
        "TRAE 可执行文件不存在：{}",
        executable.display()
    );
    assert!(
        user_data_dir.is_dir(),
        "隔离 user-data-dir 不存在：{}",
        user_data_dir.display()
    );
    assert!(
        database_path.is_file(),
        "隔离目标数据库不存在：{}",
        database_path.display()
    );

    let data_location_id = capture_location_identity(
        &traesync_infrastructure::PlatformFileIdentityProvider::new(),
        &data_root,
        &db_relative,
    )
    .unwrap_or_else(|error| panic!("隔离数据位置身份捕获失败：{error}"))
    .data_location_id;
    // 先捕获文件身份，再由当前测试进程独占目标文件，模拟错误进程持有数据库。
    let _wrong_target_lock = FileShareLock::acquire(&database_path);
    let child = ChildGuard::spawn(&executable, &user_data_dir, &port);
    let child_pid = child.id();
    let controller =
        WorkCnProcessController::with_executable_names(["TRAE SOLO CN.exe", "trae.exe"]);
    let observation = observe_until_status(
        &controller,
        &data_location_id,
        &data_root,
        &db_relative,
        ProcessObservationStatus::WrongTarget,
    );
    let child_seen = observation
        .candidates
        .iter()
        .any(|candidate| candidate.pid == child_pid);

    println!(
        "REAL_WRONG_TARGET_OBSERVATION={}",
        json!({
            "data_location_id": observation.data_location_id,
            "status": format!("{:?}", observation.status),
            "child_pid": child_pid,
            "child_seen": child_seen,
            "candidate_count": observation.candidates.len(),
            "candidate_identities": observation.candidates.iter().map(|candidate| {
                json!({
                    "pid": candidate.pid,
                    "executable_path": candidate.executable_path,
                    "executable_identity": candidate.executable_identity,
                })
            }).collect::<Vec<_>>(),
            "evidence": observation
                .evidence
                .iter()
                .map(|item| json!({"code": item.code, "value": item.value}))
                .collect::<Vec<_>>(),
        })
    );

    assert!(
        child_seen,
        "进程观测未识别本轮隔离 TRAE 主进程：{child_pid}"
    );
    assert_eq!(
        observation.status,
        ProcessObservationStatus::WrongTarget,
        "目标数据库被非 TRAE 进程占用时未识别为错误目标：{:?}",
        observation.status
    );
}
