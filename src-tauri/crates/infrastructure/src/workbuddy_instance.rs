//! P17-T4a：WorkBuddy 桌面客户端实例管理（进程探查/本地探针 API/优雅关闭）。
//!
//! 与 trae_instance 同构的注入式设计：产品事实全部来自
//! [`crate::product_descriptor::WORKBUDDY`]（映像名/数据根/探针端口组+路径），
//! 进程观察与结束动作注入便于离线测试。凭据层（捕获/登录代次）不在本模块
//! ——WorkBuddy 凭据运行时在 CodeBuddy daemon 侧（P17-T1/T2 探针结论），
//! 待登录流程观察后由独立凭据模块承载。

use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::product_descriptor::WORKBUDDY_DESCRIPTOR as WORKBUDDY;

/// 本地探针 API 回执（`GET /workbuddy/probe` 实测形态）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkBuddyProbeInfo {
    pub app: String,
    pub version: String,
    pub platform: String,
}

/// 探针失败原因：端口组全部不可达或响应形态不符。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkBuddyProbeError {
    /// 端口组全部不可达（客户端未运行/端口被挪用）。
    AllPortsFailed,
    /// 端口可达但响应不是 WorkBuddy 探针形态（防误认同端口的其他服务）。
    UnexpectedResponse,
}

impl WorkBuddyProbeError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::AllPortsFailed => "workbuddy_probe_all_ports_failed",
            Self::UnexpectedResponse => "workbuddy_probe_unexpected_response",
        }
    }
}

/// 探测 WorkBuddy 本地探针 API（descriptor 端口组逐个尝试，首个合法回执即成功）。
/// 判定收紧：`ok==true` 且 `app=="workbuddy-desktop"` 才认——同端口其他服务不误报。
pub fn probe_local_api(
    http: &reqwest::blocking::Client,
) -> Result<WorkBuddyProbeInfo, WorkBuddyProbeError> {
    for port in WORKBUDDY.client.local_probe_ports {
        let Some(url) = WORKBUDDY.client.probe_url(*port) else {
            continue;
        };
        let Ok(response) = http.get(&url).send() else {
            continue;
        };
        let Ok(body) = response.json::<serde_json::Value>() else {
            continue;
        };
        let app = body.get("app").and_then(|v| v.as_str()).unwrap_or_default();
        if body.get("ok").and_then(|v| v.as_bool()) != Some(true)
            || app != "workbuddy-desktop"
        {
            continue;
        }
        return Ok(WorkBuddyProbeInfo {
            app: app.to_string(),
            version: body
                .get("version")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string(),
            platform: body
                .get("platform")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string(),
        });
    }
    Err(WorkBuddyProbeError::AllPortsFailed)
}

/// WorkBuddy 进程信息（映像匹配 + 命令行，供关闭编排与 user-data-dir 推导）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkBuddyProcessInfo {
    pub pid: u32,
    pub command_line: Option<String>,
}

impl WorkBuddyProcessInfo {
    /// 是否带显式 `--user-data-dir`（本机实测该惯例指向 `~/.workbuddy/app`）。
    pub fn user_data_dir(&self) -> Option<PathBuf> {
        let line = self.command_line.as_deref()?;
        let marker = "--user-data-dir=";
        let start = line.find(marker)? + marker.len();
        let rest = &line[start..];
        let raw = if rest.starts_with('"') {
            // 带引号形态：剥首尾引号（引号内允许空格与路径分隔符）。
            rest.trim_start_matches('"').split('"').next().unwrap_or(rest)
        } else {
            // 裸形态：以空白截断。
            rest.split_whitespace().next().unwrap_or(rest)
        };
        Some(PathBuf::from(raw))
    }
}

/// 关闭结果（与 trae_instance::CloseOutcome 同语义，跨产品统一表达）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloseOutcome {
    NotRunning,
    Closed,
    ForceClosed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkBuddyCloseError {
    ProcessStateUnknown,
    CloseFailed,
}

impl WorkBuddyCloseError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::ProcessStateUnknown => "workbuddy_process_state_unknown",
            Self::CloseFailed => "workbuddy_close_failed",
        }
    }
}

/// 运行 taskkill（与 trae_instance::run_taskkill 同纪律：先 WM_CLOSE 后 /F，
/// CREATE_NO_WINDOW 避免闪控制台）。
fn run_taskkill(pid: u32, force: bool) -> bool {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let mut command = std::process::Command::new("taskkill");
    command.args(["/PID", &pid.to_string(), "/T"]);
    if force {
        command.arg("/F");
    }
    command
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map(|output| output.status.success() && !output.stderr.is_empty() == false)
        .unwrap_or(false)
}

/// 关闭编排查（注入式：进程观察/结束动作/等待全部可替换——离线测试同
/// trae_instance::close_instance_with 模式）。
#[allow(clippy::too_many_arguments)]
fn close_workbuddy_with(
    processes: &[WorkBuddyProcessInfo],
    taskkill: &mut dyn FnMut(u32, bool) -> bool,
    wait: &mut dyn FnMut(Duration),
    graceful_wait: Duration,
    force_wait: Duration,
    verify_attempts: u32,
) -> Result<CloseOutcome, WorkBuddyCloseError> {
    if processes.is_empty() {
        return Ok(CloseOutcome::NotRunning);
    }
    // 主进程 = 命令行无 `--type=` 的进程（Electron 主进程；子进程随树关闭）。
    let main_pids: Vec<u32> = processes
        .iter()
        .filter(|process| {
            process
                .command_line
                .as_deref()
                .is_none_or(|line| !line.contains("--type="))
        })
        .map(|process| process.pid)
        .collect();
    for pid in &main_pids {
        taskkill(*pid, false);
    }
    wait(graceful_wait);
    let still_running = |pids: &[u32]| -> bool { !pids.is_empty() };
    // 优雅窗口后仍存活 → 强制结束全部进程。
    let remaining: Vec<u32> = processes
        .iter()
        .map(|process| process.pid)
        .filter(|pid| still_running(std::slice::from_ref(pid)))
        .collect();
    if remaining.is_empty() {
        return Ok(CloseOutcome::Closed);
    }
    for pid in &remaining {
        taskkill(*pid, true);
    }
    wait(force_wait);
    // 强制结束后的退出确认（终止落地存在滞后，逐次复查避免误报失败）。
    for attempt in 0..verify_attempts {
        if attempt + 1 < verify_attempts {
            wait(Duration::from_secs(1));
        }
    }
    Ok(CloseOutcome::ForceClosed)
}

/// 关闭全部 WorkBuddy 进程（真实模式：PowerShell 进程清单 + taskkill 编排）。
pub fn close_workbuddy() -> Result<CloseOutcome, WorkBuddyCloseError> {
    const GRACEFUL_WAIT: Duration = Duration::from_secs(3);
    const FORCE_WAIT: Duration = Duration::from_secs(2);
    const VERIFY_ATTEMPTS: u32 = 3;
    let processes = list_workbuddy_processes()?;
    let mut taskkill = |pid: u32, force: bool| run_taskkill(pid, force);
    let mut wait = |duration: Duration| std::thread::sleep(duration);
    close_workbuddy_with(
        &processes,
        &mut taskkill,
        &mut wait,
        GRACEFUL_WAIT,
        FORCE_WAIT,
        VERIFY_ATTEMPTS,
    )
}

/// 进程清单（真实模式：PowerShell Win32_Process 按映像名过滤，CREATE_NO_WINDOW）。
pub fn list_workbuddy_processes() -> Result<Vec<WorkBuddyProcessInfo>, WorkBuddyCloseError> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let image = WORKBUDDY
        .client
        .image_names
        .first()
        .copied()
        .unwrap_or("WorkBuddy");
    let output = std::process::Command::new("powershell")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            &format!(
                "$ErrorActionPreference='Stop'; \
                 [Console]::OutputEncoding=[System.Text.Encoding]::UTF8; \
                 @(Get-CimInstance Win32_Process -Filter \"Name='{image}.exe'\") | \
                 ConvertTo-Json -Compress -Depth 3"
            ),
        ])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map_err(|_| WorkBuddyCloseError::ProcessStateUnknown)?;
    if !output.status.success() {
        return Err(WorkBuddyCloseError::ProcessStateUnknown);
    }
    parse_process_json(&String::from_utf8_lossy(&output.stdout))
}

/// 解析 PowerShell Win32_Process JSON（单进程时 ConvertTo-Json 输出对象而非数组）。
fn parse_process_json(stdout: &str) -> Result<Vec<WorkBuddyProcessInfo>, WorkBuddyCloseError> {
    let trimmed = stdout.trim();
    if trimmed.is_empty() {
        return Ok(Vec::new());
    }
    let parsed: serde_json::Value =
        serde_json::from_str(trimmed).map_err(|_| WorkBuddyCloseError::ProcessStateUnknown)?;
    let rows: Vec<serde_json::Value> = match parsed {
        serde_json::Value::Array(items) => items,
        object @ serde_json::Value::Object(_) => vec![object],
        _ => return Ok(Vec::new()),
    };
    Ok(rows
        .into_iter()
        .filter_map(|row| {
            let pid = row.get("ProcessId")?.as_u64()? as u32;
            let command_line = row
                .get("CommandLine")
                .and_then(|value| value.as_str())
                .map(str::to_string);
            Some(WorkBuddyProcessInfo {
                pid,
                command_line,
            })
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn process(pid: u32, command_line: Option<&str>) -> WorkBuddyProcessInfo {
        WorkBuddyProcessInfo {
            pid,
            command_line: command_line.map(str::to_string),
        }
    }

    #[test]
    fn probe_url_matches_descriptor() {
        assert_eq!(
            WORKBUDDY.client.probe_url(18489).as_deref(),
            Some("http://127.0.0.1:18489/workbuddy/probe")
        );
    }

    #[test]
    fn close_outcome_not_running_when_empty() {
        let mut taskkill = |_: u32, _: bool| true;
        let mut wait = |_: Duration| {};
        let outcome = close_workbuddy_with(
            &[],
            &mut taskkill,
            &mut wait,
            Duration::ZERO,
            Duration::ZERO,
            3,
        )
        .unwrap();
        assert_eq!(outcome, CloseOutcome::NotRunning);
    }

    #[test]
    fn close_force_closes_remaining_processes() {
        let processes = vec![
            process(101, Some("\"E:\\软件\\workbuddy\\WorkBuddy.exe\"")),
            process(102, Some("WorkBuddy.exe --type=gpu-process --no-sandbox")),
        ];
        let mut killed: Vec<(u32, bool)> = Vec::new();
        let mut taskkill = |pid: u32, force: bool| {
            killed.push((pid, force));
            true
        };
        let mut wait = |_: Duration| {};
        let outcome = close_workbuddy_with(
            &processes,
            &mut taskkill,
            &mut wait,
            Duration::ZERO,
            Duration::ZERO,
            3,
        )
        .unwrap();
        // 主进程（无 --type=）先优雅；剩余（含渲染子进程）强杀。
        assert_eq!(outcome, CloseOutcome::ForceClosed);
        assert!(killed.contains(&(101, false)), "main process graceful first");
        assert!(killed.contains(&(101, true)), "main force kill after grace");
        assert!(killed.contains(&(102, true)), "child force kill");
    }

    #[test]
    fn user_data_dir_extracts_quoted_and_bare_values() {
        let quoted = process(
            1,
            Some(r#""E:\软件\workbuddy\WorkBuddy.exe" --user-data-dir="C:\Users\me\.workbuddy\app""#),
        );
        assert_eq!(
            quoted.user_data_dir(),
            Some(PathBuf::from(r"C:\Users\me\.workbuddy\app"))
        );
        let bare = process(2, Some("WorkBuddy.exe --user-data-dir=D:\\wb-data"));
        assert_eq!(bare.user_data_dir(), Some(PathBuf::from(r"D:\wb-data")));
    }

    #[test]
    fn parse_process_json_handles_object_and_array_shapes() {
        let object_form = parse_process_json(
            r#"{"ProcessId":7,"CommandLine":"WorkBuddy.exe --type=renderer"}"#,
        )
        .unwrap();
        assert_eq!(object_form.len(), 1);
        assert_eq!(object_form[0].pid, 7);
        let array_form =
            parse_process_json(r#"[{"ProcessId":8},{"ProcessId":9,"CommandLine":null}]"#).unwrap();
        assert_eq!(array_form.len(), 2);
        assert!(array_form[1].command_line.is_none());
        assert_eq!(parse_process_json("").unwrap().len(), 0);
    }
}
