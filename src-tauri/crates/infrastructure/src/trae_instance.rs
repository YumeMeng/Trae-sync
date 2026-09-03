//! P2-2 TRAE 实例管理：安装路径发现、`--user-data-dir` 并行实例的状态检测与窗口聚焦。
//!
//! 依据 ADR-0020（2026-08-23 修订：并行多开）与实验报告
//! `.scratch/p2-2-instance-experiments/REPORT.md`：
//! - TRAE 接受 `--user-data-dir` 参数，对话库（ModularData）完全跟随目录；
//! - 实例锁按目录独立（code.lock 在各自 data_dir 内），不同目录实例可并行运行；
//! - 登录 blob 的加密密钥为机器级，跨目录复制 `User\globalStorage` + `machineid`
//!   即完成登录态迁移（TRAE 原生多账号目录 `TRAE SOLO CN_{account_id}` 可作种子）。
//!
//! 本模块只做无副作用判定与系统交互；启动编排（账号校验、种子决策）在应用层。

use std::path::{Path, PathBuf};

/// 实例管理错误；`code()` 返回前端可映射文案的稳定错误码。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TraeInstanceError {
    /// profile_id 含非法字符（只允许小写字母/数字/连字符，防路径穿越）。
    InvalidProfileId,
    /// account_id 必须为纯数字（拼接原生目录名 `TRAE SOLO CN_{account_id}`）。
    InvalidAccountId,
    /// PowerShell 进程查询失败（不存在的环境或执行超时外的失败）。
    ProcessQueryFailed,
    /// 登录态种子复制失败（种子源存在但复制中途出错）。
    SeedCopyFailed,
}

impl TraeInstanceError {
    /// 返回启动层可安全传递给 UI 的稳定错误码。
    pub const fn code(self) -> &'static str {
        match self {
            Self::InvalidProfileId => "trae_profile_invalid",
            Self::InvalidAccountId => "trae_account_invalid",
            Self::ProcessQueryFailed => "trae_process_query_failed",
            Self::SeedCopyFailed => "trae_seed_copy_failed",
        }
    }
}

impl std::fmt::Display for TraeInstanceError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let message = match self {
            Self::InvalidProfileId => "账号标识格式非法",
            Self::InvalidAccountId => "账号 ID 格式非法",
            Self::ProcessQueryFailed => "TRAE 进程查询失败",
            Self::SeedCopyFailed => "登录态种子复制失败",
        };
        formatter.write_str(message)
    }
}

impl std::error::Error for TraeInstanceError {}

/// profile_id 白名单：小写字母/数字/连字符（与 `checkin-{hex}` 生成格式一致），
/// 长度上限 64。防止把路径分隔符等注入实例目录名。
/// pub(crate)：W3 会话索引缓存以 profile_id 作文件名，复用同一白名单。
pub(crate) fn profile_id_is_safe(profile_id: &str) -> bool {
    !profile_id.is_empty()
        && profile_id.len() <= 64
        && profile_id
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// 账号专属实例目录：`{storage_root}\trae-instances\{profile_id}`。
/// profile_id 不合法时拒绝（不创建目录）。
pub fn instance_data_dir(
    storage_root: &Path,
    profile_id: &str,
) -> Result<PathBuf, TraeInstanceError> {
    if !profile_id_is_safe(profile_id) {
        return Err(TraeInstanceError::InvalidProfileId);
    }
    Ok(storage_root.join("trae-instances").join(profile_id))
}

/// TRAE 原生账号目录：`{APPDATA}\TRAE SOLO CN_{account_id}`（登录态种子源）。
/// account_id 必须为纯数字（拼接目录名，防路径注入）；非法返回 None。
/// U-6 W4 占用统计与彻底删除记录按同一命名规则定位该目录。
pub fn native_account_dir(appdata: &Path, account_id: &str) -> Option<PathBuf> {
    if account_id.is_empty() || !account_id.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    Some(appdata.join(format!("TRAE SOLO CN_{account_id}")))
}

/// 递归统计目录占用字节数（文件大小之和，不跟踪符号链接）。
/// 目录缺失记 0（U-6 W4 占用统计口径：缺失即未产生本地数据）；
/// 单个条目读取失败按 0 跳过——统计仅用于展示，不因个别被锁文件失败。
pub fn dir_size_recursive(path: &Path) -> u64 {
    let entries = match std::fs::read_dir(path) {
        Ok(entries) => entries,
        Err(_) => return 0,
    };
    let mut total = 0u64;
    for entry in entries.flatten() {
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_dir() {
            total += dir_size_recursive(&entry.path());
        } else if file_type.is_file() {
            total += entry.metadata().map(|metadata| metadata.len()).unwrap_or(0);
        }
    }
    total
}

/// 一条 TRAE 进程信息（PowerShell `Win32_Process` 查询结果）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TraeProcessInfo {
    pub pid: u32,
    /// 主程序完整路径；权限不足时可能为空。
    pub exe_path: Option<String>,
    /// 完整命令行；权限不足时可能为空。
    pub command_line: Option<String>,
}

/// PowerShell 查询脚本：列出 TRAE IDE 进程（排除本应用 trae-sync 与
/// 参考项目 manager），UTF-8 输出 JSON 数组。
/// 注意：WQL 不支持 `NOT LIKE`，必须用 `NOT (Name LIKE '...')` 形式
///（2026-08-23 真实环境验证：`NOT LIKE` 会报“无效查询”导致查询整体失败）。
const PROCESS_QUERY_SCRIPT: &str = concat!(
    "$ErrorActionPreference='Stop'; ",
    "[Console]::OutputEncoding=[System.Text.Encoding]::UTF8; ",
    "$rows = @(Get-CimInstance Win32_Process -Filter ",
    "\"Name LIKE '%trae%' AND NOT (Name LIKE '%sync%') AND NOT (Name LIKE '%manager%')\" ",
    "| Select-Object ProcessId, ExecutablePath, CommandLine); ",
    "ConvertTo-Json -InputObject $rows -Compress"
);

/// 查询当前全部 TRAE IDE 进程（不含本应用自身）。
/// 首次调用冷启动 PowerShell 约 0.5~1 秒；调用方应放在阻塞线程。
#[cfg(windows)]
pub fn list_trae_processes() -> Result<Vec<TraeProcessInfo>, TraeInstanceError> {
    use std::os::windows::process::CommandExt;
    // CREATE_NO_WINDOW：避免每次查询闪出 PowerShell 控制台窗口。
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let output = std::process::Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", PROCESS_QUERY_SCRIPT])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map_err(|_| TraeInstanceError::ProcessQueryFailed)?;
    if !output.status.success() {
        return Err(TraeInstanceError::ProcessQueryFailed);
    }
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    Ok(parse_process_json(&stdout))
}

/// 解析 `ConvertTo-Json` 输出的进程数组（纯函数，可单测）。
/// 容忍单对象（非数组）、空输出与字段缺失，尽量多恢复条目。
pub fn parse_process_json(raw: &str) -> Vec<TraeProcessInfo> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Vec::new();
    }
    let Ok(value) = serde_json::from_str::<serde_json::Value>(trimmed) else {
        return Vec::new();
    };
    let rows: Vec<&serde_json::Value> = match &value {
        serde_json::Value::Array(items) => items.iter().collect(),
        object @ serde_json::Value::Object(_) => vec![object],
        _ => return Vec::new(),
    };
    rows.iter()
        .filter_map(|row| {
            let pid = row.get("ProcessId")?.as_u64()? as u32;
            let exe_path = row
                .get("ExecutablePath")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string());
            let command_line = row
                .get("CommandLine")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string());
            if pid == 0 && exe_path.is_none() && command_line.is_none() {
                return None;
            }
            Some(TraeProcessInfo {
                pid,
                exe_path,
                command_line,
            })
        })
        .collect()
}

/// 判断进程命令行是否以 `--user-data-dir=<data_dir>` 运行。
/// 比较前去除全部引号并统一小写：Windows 路径大小写不敏感，
/// 且 PowerShell 展示命令行时引号形态不稳定（`--user-data-dir="path"` 与
/// `--user-data-dir=path` 均为合法 Chromium 参数形态）。
pub fn command_line_uses_data_dir(command_line: &str, data_dir: &Path) -> bool {
    let normalized: String = command_line
        .chars()
        .filter(|c| *c != '"')
        .flat_map(|c| c.to_lowercase())
        .collect();
    let dir = data_dir.to_string_lossy().replace('"', "").to_lowercase();
    // 需匹配到完整目录边界：data_dir 是另一目录的前缀时不误判
    //（如 ...\profile-a 与 ...\profile-ab）。
    let Some(position) = normalized.find(&dir) else {
        return false;
    };
    let prefix_end = position + dir.len();
    let boundary_ok = normalized.len() == prefix_end
        || !normalized.as_bytes()[prefix_end].is_ascii_alphanumeric();
    boundary_ok && normalized.contains("--user-data-dir=")
}

/// 主库实例匹配（2026-08-31 架构修订：主库 = 官方目录）。
///
/// 官方目录实例有两种运行形态：
/// 1. 带参形态：`--user-data-dir=<官方目录>`（App 启动的主进程与全部
///    Chromium 子进程，官方启动形态的子进程同样带参）；
/// 2. 无参形态：用户从官方快捷方式启动的主进程，命令行不带
///    `--user-data-dir`（Chromium 默认 data_dir 即官方目录）。
/// 两种形态都必须识别，否则聚焦/关闭/切号都管不到用户日常手开的实例。
pub fn command_line_matches_master(command_line: &str, data_dir: &Path) -> bool {
    if command_line_uses_data_dir(command_line, data_dir) {
        return true;
    }
    // 无参主进程：非子进程（无 --type=）且未指定其他 data_dir。
    let lowered = command_line.to_lowercase();
    !lowered.contains("--user-data-dir") && !lowered.contains("--type=")
}

/// TRAE 安装信息（注册表 Uninstall 查询结果）。
#[derive(Debug, Clone, PartialEq, Eq)]
struct TraeInstallRecord {
    display_name: String,
    /// DisplayIcon 中的 exe 完整路径（优先）。
    exe_path: Option<String>,
    /// InstallLocation 目录（兜底：目录下扫描 exe）。
    install_location: Option<String>,
}

/// PowerShell 注册表查询脚本：TRAE 相关 Uninstall 记录（排除 manager）。
const REGISTRY_QUERY_SCRIPT: &str = concat!(
    "$ErrorActionPreference='Stop'; ",
    "[Console]::OutputEncoding=[System.Text.Encoding]::UTF8; ",
    "$keys = @('HKLM:\\Software\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\*', ",
    "'HKLM:\\Software\\WOW6432Node\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\*', ",
    "'HKCU:\\Software\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\*'); ",
    "$apps = @(Get-ItemProperty $keys -ErrorAction SilentlyContinue ",
    "| Where-Object { $_.DisplayName -like '*Trae*' -and $_.DisplayName -notlike '*Manager*' } ",
    "| Select-Object DisplayName, InstallLocation, DisplayIcon); ",
    "ConvertTo-Json -InputObject $apps -Compress"
);

/// 解析注册表查询 JSON（纯函数，可单测）。
fn parse_registry_json(raw: &str) -> Vec<TraeInstallRecord> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Vec::new();
    }
    let Ok(value) = serde_json::from_str::<serde_json::Value>(trimmed) else {
        return Vec::new();
    };
    let rows: Vec<&serde_json::Value> = match &value {
        serde_json::Value::Array(items) => items.iter().collect(),
        object @ serde_json::Value::Object(_) => vec![object],
        _ => return Vec::new(),
    };
    rows.iter()
        .filter_map(|row| {
            let display_name = row
                .get("DisplayName")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            if display_name.is_empty() {
                return None;
            }
            let strip_quotes = |s: &str| s.trim().trim_matches('"').to_string();
            let exe_path = row
                .get("DisplayIcon")
                .and_then(|v| v.as_str())
                .map(strip_quotes)
                .filter(|s| !s.is_empty());
            let install_location = row
                .get("InstallLocation")
                .and_then(|v| v.as_str())
                .map(strip_quotes)
                .filter(|s| !s.is_empty());
            Some(TraeInstallRecord {
                display_name,
                exe_path,
                install_location,
            })
        })
        .collect()
}

/// 注册表记录中提取存在的 exe 路径；DisplayIcon 优先，InstallLocation 下
/// 扫描文件名含 "trae" 的 exe 兜底。
fn exe_from_install_record(record: &TraeInstallRecord) -> Option<PathBuf> {
    if let Some(icon) = &record.exe_path {
        let path = PathBuf::from(icon);
        if path.is_file() {
            return Some(path);
        }
    }
    if let Some(location) = &record.install_location {
        let dir = PathBuf::from(location);
        if dir.is_dir() {
            if let Ok(entries) = std::fs::read_dir(&dir) {
                // 单层扫描即可覆盖（exe 直接位于安装根目录）。
                let mut candidates: Vec<PathBuf> = entries
                    .flatten()
                    .map(|entry| entry.path())
                    .filter(|path| {
                        path.extension()
                            .and_then(|ext| ext.to_str())
                            .is_some_and(|ext| ext.eq_ignore_ascii_case("exe"))
                            && path
                                .file_stem()
                                .and_then(|stem| stem.to_str())
                                .is_some_and(|stem| stem.to_lowercase().contains("trae"))
                    })
                    .collect();
                candidates.sort();
                if let Some(first) = candidates.first() {
                    return Some(first.clone());
                }
            }
        }
    }
    None
}

/// 发现 TRAE Work CN 可执行文件路径。
///
/// 优先级：
/// 1. 运行中的 TRAE 进程 `ExecutablePath`（最准确，用户真实安装位置）；
/// 2. 注册表 Uninstall 记录（DisplayName 含 "Work" 优先——本项目只支持 Work CN）；
/// 3. 常见安装路径扫描（`%LOCALAPPDATA%\Programs\Trae*`）。
pub fn discover_trae_executable(
    processes: &[TraeProcessInfo],
) -> Result<PathBuf, TraeInstanceError> {
    // 1. 运行中进程的真实路径。
    for process in processes {
        if let Some(exe) = &process.exe_path {
            let path = PathBuf::from(exe);
            if path.is_file() {
                return Ok(path);
            }
        }
    }
    // 2. 注册表 Uninstall 记录。
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        if let Ok(output) = std::process::Command::new("powershell")
            .args([
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                REGISTRY_QUERY_SCRIPT,
            ])
            .creation_flags(CREATE_NO_WINDOW)
            .output()
        {
            if output.status.success() {
                let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
                let records = parse_registry_json(&stdout);
                // Work CN 优先（本项目定位），其余 Trae 记录按名称排序保持稳定。
                let mut ordered: Vec<&TraeInstallRecord> = records.iter().collect();
                ordered.sort_by_key(|record| {
                    !record.display_name.to_lowercase().contains("work")
                });
                for record in ordered {
                    if let Some(exe) = exe_from_install_record(record) {
                        return Ok(exe);
                    }
                }
            }
        }
    }
    // 3. 常见安装路径兜底。
    if let Some(local_appdata) = std::env::var_os("LOCALAPPDATA") {
        let programs = PathBuf::from(&local_appdata).join("Programs");
        if let Ok(entries) = std::fs::read_dir(&programs) {
            let mut roots: Vec<PathBuf> = entries
                .flatten()
                .map(|entry| entry.path())
                .filter(|path| {
                    path.is_dir()
                        && path
                            .file_name()
                            .and_then(|name| name.to_str())
                            .is_some_and(|name| name.to_lowercase().starts_with("trae"))
                })
                .collect();
            roots.sort();
            for root in roots {
                if let Ok(files) = std::fs::read_dir(&root) {
                    let mut candidates: Vec<PathBuf> = files
                        .flatten()
                        .map(|entry| entry.path())
                        .filter(|path| {
                            path.extension()
                                .and_then(|ext| ext.to_str())
                                .is_some_and(|ext| ext.eq_ignore_ascii_case("exe"))
                                && path
                                    .file_stem()
                                    .and_then(|stem| stem.to_str())
                                    .is_some_and(|stem| stem.to_lowercase().contains("trae"))
                        })
                        .collect();
                    candidates.sort();
                    if let Some(first) = candidates.first() {
                        return Ok(first.clone());
                    }
                }
            }
        }
    }
    Err(TraeInstanceError::ProcessQueryFailed)
}

/// 聚焦给定进程集合的主窗口（还原 + 前置）。
/// 返回是否找到并前置了可见主窗口。
#[cfg(windows)]
pub fn focus_instance_windows(pids: &[u32]) -> bool {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GetWindowTextW, GetWindowThreadProcessId, IsWindowVisible,
        SetForegroundWindow, ShowWindowAsync, SW_RESTORE,
    };

    /// EnumWindows 回调上下文：目标 PID 集合与是否已聚焦。
    struct FocusContext {
        pids: Vec<u32>,
        focused: bool,
    }

    unsafe extern "system" fn enum_callback(
        hwnd: windows_sys::Win32::Foundation::HWND,
        lparam: windows_sys::Win32::Foundation::LPARAM,
    ) -> windows_sys::Win32::Foundation::BOOL {
        let context = &mut *(lparam as *mut FocusContext);
        let mut pid: u32 = 0;
        GetWindowThreadProcessId(hwnd, &mut pid);
        // 条件：属于目标实例进程 + 可见 + 有窗口标题（主窗口特征；
        // TRAE 子进程的隐藏辅助窗口无标题，直接跳过）。
        if context.pids.contains(&pid) && IsWindowVisible(hwnd) != 0 {
            let mut title = [0u16; 128];
            let length = GetWindowTextW(hwnd, title.as_mut_ptr(), title.len() as i32);
            if length > 0 {
                ShowWindowAsync(hwnd, SW_RESTORE);
                SetForegroundWindow(hwnd);
                context.focused = true;
                return 0; // 已找到主窗口，停止枚举。
            }
        }
        1
    }

    let mut context = FocusContext {
        pids: pids.to_vec(),
        focused: false,
    };
    unsafe {
        EnumWindows(
            Some(enum_callback),
            &mut context as *mut FocusContext
                as windows_sys::Win32::Foundation::LPARAM,
        );
    }
    context.focused
}

/// 实例登录态（读实例目录 storage.json + 最近启动日志判定，纯文件检查无副作用）。
/// 序列化为 snake_case 字符串直达前端
/// （uninitialized/logged_in/logged_out/stale）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum InstanceLoginState {
    /// 实例目录无 storage.json：该账号从未启动过实例。
    Uninitialized,
    /// storage.json 含 `iCubeAuthInfo://usertag` 键：TRAE 内已登录。
    LoggedIn,
    /// storage.json 存在但无登录键：启动过但未登录。
    LoggedOut,
    /// 登录键仍在但最近一次启动日志含未登录证据：登录态已失效
    /// （TRAE 会话过期时不清除失效 blob，需在 TRAE 窗口内重新登录）。
    Stale,
}

/// 最近一次启动会话的登录证据（读 TRAE renderer.log 提取，纯文件检查）。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct LoginEvidence {
    /// 正向标记：`User info loaded {"userId":...}`（TRAE 加载到已保存的登录态）。
    positive: bool,
    /// 负向标记：`User not authenticated` / `[ckg] not login`
    /// （TRAE 拿 blob 去服务端验证被拒，2026-08-24 LY 实例实证）。
    negative: bool,
}

/// 从最近启动会话的 renderer.log 提取登录证据。
///
/// 日志目录名形如 `20260824T021507`（每次启动一个，按名字排序即按时间），
/// 渲染进程日志位于 `window1/renderer.log`。TRAE 更新可能调整日志结构，
/// 读取失败一律返回"无证据"（退化为 storage.json 键存在性判定）。
fn latest_session_login_evidence(instance_dir: &Path) -> LoginEvidence {
    let logs_dir = instance_dir.join("logs");
    let Ok(entries) = std::fs::read_dir(&logs_dir) else {
        return LoginEvidence::default();
    };
    // 取字典序最大的合法会话目录（时间戳命名，字典序 = 时间序）。
    let latest = entries
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.file_name())
        .filter_map(|name| name.to_str().map(str::to_owned))
        .filter(|name| is_session_dir_name(name))
        .max();
    let Some(latest) = latest else {
        return LoginEvidence::default();
    };
    let renderer_log = logs_dir
        .join(latest)
        .join("window1")
        .join("renderer.log");
    let Ok(log) = std::fs::read_to_string(&renderer_log) else {
        return LoginEvidence::default();
    };
    LoginEvidence {
        positive: log.contains("User info loaded"),
        negative: log.contains("User not authenticated") || log.contains("[ckg] not login"),
    }
}

/// 判定会话目录名是否为 TRAE 时间戳格式：8 位日期 + 'T' + 6 位时间。
fn is_session_dir_name(name: &str) -> bool {
    let bytes = name.as_bytes();
    bytes.len() == 15
        && bytes[8] == b'T'
        && bytes[..8].iter().all(u8::is_ascii_digit)
        && bytes[9..].iter().all(u8::is_ascii_digit)
}

/// 判定实例登录态：读 `{instance_dir}/User/globalStorage/storage.json`
/// 检查 `iCubeAuthInfo://usertag` 键（2026-08-24 三实例实证：已登录实例
/// LY/import 均含此键，未登录实例梦梦不含），键存在时再用最近启动日志
/// 裁决真伪——blob 是加密黑盒无法本地验证，但 TRAE 启动时会在日志里
/// 留下服务端验证结果（正向 `User info loaded` / 负向 `User not
/// authenticated`，2026-08-24 LY 实例"键在会话死"实证）。
///
/// 文件读取或解析失败按 LoggedOut 处理：storage.json 存在即说明实例启动过，
/// 按"待登录"引导无害（在 TRAE 内再登录一次是幂等操作）。
pub fn instance_login_state(instance_dir: &Path) -> InstanceLoginState {
    let storage = instance_dir
        .join("User")
        .join("globalStorage")
        .join("storage.json");
    if !storage.is_file() {
        return InstanceLoginState::Uninitialized;
    }
    let Ok(raw) = std::fs::read_to_string(&storage) else {
        return InstanceLoginState::LoggedOut;
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return InstanceLoginState::LoggedOut;
    };
    let has_blob = value
        .as_object()
        .is_some_and(|map| map.contains_key("iCubeAuthInfo://usertag"));
    if !has_blob {
        return InstanceLoginState::LoggedOut;
    }
    // 有登录键：日志证据裁决（正向优先——登录成功前的负向标记会被
    // 随后的正向标记覆盖；无证据时乐观判已登录，等下次启动自证）。
    let evidence = latest_session_login_evidence(instance_dir);
    if evidence.positive {
        InstanceLoginState::LoggedIn
    } else if evidence.negative {
        InstanceLoginState::Stale
    } else {
        InstanceLoginState::LoggedIn
    }
}

/// 种子复制结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SeedOutcome {
    /// 已从原生目录复制登录态种子。
    Seeded,
    /// 实例目录已有登录态（storage.json 存在），无需种子。
    NotNeeded,
    /// 未找到原生账号目录（账号从未在 TRAE 客户端登录过），
    /// 首次启动将进入 TRAE 登录页。
    NativeMissing,
}

/// 为全新实例目录播种登录态（实验验证的迁移路径）：
/// 复制原生目录的 `User\globalStorage`（含加密登录 blob 与 state.vscdb）
/// 与 `machineid`。已存在 storage.json 时幂等跳过。
///
/// 单个非关键文件被占用（原生实例运行中）时跳过该文件继续；
/// 关键文件 storage.json 复制失败则报错。
pub fn seed_login_state(
    instance_dir: &Path,
    account_id: &str,
) -> Result<SeedOutcome, TraeInstanceError> {
    let appdata = std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_default();
    seed_login_state_with_appdata(instance_dir, account_id, &appdata)
}

/// `seed_login_state` 的可测试版本：原生目录根由调用方注入。
pub(crate) fn seed_login_state_with_appdata(
    instance_dir: &Path,
    account_id: &str,
    appdata: &Path,
) -> Result<SeedOutcome, TraeInstanceError> {
    // 原生目录命名规则集中在 native_account_dir（account_id 非纯数字被拒绝）。
    let Some(native) = native_account_dir(appdata, account_id) else {
        return Err(TraeInstanceError::InvalidAccountId);
    };
    let target_storage = instance_dir.join("User").join("globalStorage");
    if target_storage.join("storage.json").is_file() {
        return Ok(SeedOutcome::NotNeeded);
    }
    if !native.is_dir() {
        return Ok(SeedOutcome::NativeMissing);
    }
    let native_storage = native.join("User").join("globalStorage");
    if !native_storage.join("storage.json").is_file() {
        return Ok(SeedOutcome::NativeMissing);
    }
    copy_dir_best_effort(&native_storage, &target_storage)?;
    // machineid 与 globalStorage 同级，单独补一份（设备标识随登录态走）。
    let native_machine_id = native.join("machineid");
    if native_machine_id.is_file() {
        let target_machine_id = instance_dir.join("machineid");
        if !target_machine_id.is_file() {
            if std::fs::copy(&native_machine_id, &target_machine_id).is_err() {
                return Err(TraeInstanceError::SeedCopyFailed);
            }
        }
    }
    if !target_storage.join("storage.json").is_file() {
        return Err(TraeInstanceError::SeedCopyFailed);
    }
    Ok(SeedOutcome::Seeded)
}

/// 从 App 登录存档目录为全新环境播种登录态（P6-4 环境登录：空环境首次
/// 登录路径）。
///
/// 空环境无 `storage.json` 密钥材料，E1/E2 凭据互换都无从下手；首登 =
/// 从该账号的登录凭据存档（`{storage_root}\trae-instances\{profile_id}`
/// 三件套所在目录）整体移植 `User\globalStorage`（含加密登录 blob 与
/// state.vscdb）与 `machineid`——移植完成后环境登录态即该账号（P6-4
/// 「失败形态下从该账号登录凭据存档移植」的空环境特例：无互换对象，
/// 移植即登录）。
///
/// 幂等：环境已有 storage.json 时不做任何修改（NotNeeded）。
/// 供体存档缺 storage.json（账号从未产生登录存档）时报 SeedCopyFailed，
/// 由调用方提示重新登录该账号。
pub fn seed_login_state_from_donor(
    instance_dir: &Path,
    donor_dir: &Path,
) -> Result<SeedOutcome, TraeInstanceError> {
    let donor_storage = donor_dir.join("User").join("globalStorage");
    if !donor_storage.join("storage.json").is_file() {
        // 供体无登录存档：无法播种（调用方报「登录凭据不可用」）。
        return Err(TraeInstanceError::SeedCopyFailed);
    }
    let target_storage = instance_dir.join("User").join("globalStorage");
    // 幂等守卫按「目标已有登录 blob」判定，而非 storage.json 文件存在：
    // 环境可能启动过但未登录（LoggedOut，storage.json 只有设备键），
    // 此时没有可互换的登录材料，同样需要播种（供体文件覆盖移植）。
    if matches!(
        instance_login_state(instance_dir),
        InstanceLoginState::LoggedIn | InstanceLoginState::Stale
    ) {
        return Ok(SeedOutcome::NotNeeded);
    }
    copy_dir_best_effort(&donor_storage, &target_storage)?;
    // machineid 与 globalStorage 同级（设备标识随登录态走），缺失才补。
    let donor_machine_id = donor_dir.join("machineid");
    if donor_machine_id.is_file() {
        let target_machine_id = instance_dir.join("machineid");
        if !target_machine_id.is_file() {
            if std::fs::copy(&donor_machine_id, &target_machine_id).is_err() {
                return Err(TraeInstanceError::SeedCopyFailed);
            }
        }
    }
    if !target_storage.join("storage.json").is_file() {
        return Err(TraeInstanceError::SeedCopyFailed);
    }
    Ok(SeedOutcome::Seeded)
}

/// 向实例目录写入窗口标题（A2，2026-08-29 trae-mate 对标裁定采纳）。
///
/// 机制（trae-mate 实测结论）：`--title` CLI 参数对 TRAE 无效；`window.title`
/// 是 VS Code 系标准配置项，TRAE 工作台启动时读取用户 settings.json 渲染
/// 窗口标题。合并写入：文件已存在且可解析时保留其他设置、只改
/// `window.title` 键；不存在或解析失败时以仅含该键的新对象起步。
/// 失败由调用方决定是否忽略（标题是便利功能，不阻断实例启动）。
pub fn write_window_title(instance_dir: &Path, title: &str) -> std::io::Result<()> {
    let user_dir = instance_dir.join("User");
    std::fs::create_dir_all(&user_dir)?;
    let settings_path = user_dir.join("settings.json");
    let mut value = std::fs::read_to_string(&settings_path)
        .ok()
        .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok())
        .filter(serde_json::Value::is_object)
        .unwrap_or_else(|| serde_json::json!({}));
    value["window.title"] = serde_json::Value::String(title.to_string());
    std::fs::write(
        &settings_path,
        serde_json::to_string_pretty(&value).map_err(std::io::Error::other)?,
    )
}

/// 递归复制目录；单个文件失败（如被运行中的原生实例锁定）时跳过继续，
/// 整个目录创建失败才返回错误。
fn copy_dir_best_effort(
    source: &Path,
    destination: &Path,
) -> Result<(), TraeInstanceError> {
    std::fs::create_dir_all(destination).map_err(|_| TraeInstanceError::SeedCopyFailed)?;
    let entries = std::fs::read_dir(source).map_err(|_| TraeInstanceError::SeedCopyFailed)?;
    for entry in entries.flatten() {
        let from = entry.path();
        let to = destination.join(entry.file_name());
        if from.is_dir() {
            copy_dir_best_effort(&from, &to)?;
        } else {
            // 单文件失败跳过（WAL/journal 等可能被锁）；关键文件缺失
            // 由调用方在收尾校验 storage.json 时兜底。
            let _ = std::fs::copy(&from, &to);
        }
    }
    Ok(())
}

/// 关闭实例结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloseOutcome {
    /// 优雅关闭（WM_CLOSE）在等待窗口内完成。
    Closed,
    /// 优雅关闭未完成（TRAE 收到 WM_CLOSE 后驻留托盘），已强制结束。
    /// 实测数据完整性无损（SQLite WAL + 下次启动恢复）。
    ForceClosed,
    /// 没有运行中的实例（幂等：重复关闭不报错）。
    NotRunning,
}

/// 实例关闭失败（强制结束后进程仍存在，如权限不足）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TraeInstanceCloseError;

/// 优雅关闭等待窗口：给 TRAE 处理 WM_CLOSE 的时间。
/// 实测 Electron 收到 WM_CLOSE 常驻留托盘不退出，短窗口即可进入强制兜底。
const GRACEFUL_CLOSE_WAIT_SECONDS: u64 = 3;
/// 强制结束后的收尾等待：进程表更新与句柄释放。
const FORCE_CLOSE_WAIT_SECONDS: u64 = 2;
/// 强制结束后确认实例已退出的最多复查次数（间隔 1 秒）。
/// 2026-08-23 E2E 实测：`taskkill /F` 对 Electron 子进程树的终止落地存在
/// 滞后（进程在最后一个 taskkill 返回后仍可存活 1~3 秒），单次复查会与
/// 进程拆除竞态，把实际已成功的关闭误报为失败。复查吸收该滞后。
const CLOSE_VERIFY_ATTEMPTS: u32 = 3;
const CLOSE_VERIFY_RETRY_DELAY_SECONDS: u64 = 1;

/// 运行 taskkill（CREATE_NO_WINDOW 避免 GUI 应用闪控制台窗口）。
/// 返回是否成功（退出码 0）；stderr 中的部分失败按不成功处理。
#[cfg(windows)]
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
        .map(|output| output.status.success())
        .unwrap_or(false)
}

/// 关闭运行于指定 data_dir 的 TRAE 实例（账号实例口径：带参匹配）。
///
/// 策略（2026-08-23 E2E 实测：WM_CLOSE 12 秒不退出，TRAE 疑似驻留托盘）：
/// 1. 对主进程 taskkill /T（WM_CLOSE 优雅关闭）；
/// 2. 等待 3 秒复查，仍存活则 taskkill /T /F 强制结束全部进程；
/// 3. 强制后再复查，仍存在视为失败。
/// 只影响该 data_dir 的进程树，其他实例与用户手动打开的 TRAE 不受波及。
#[cfg(windows)]
pub fn close_instance(instance_dir: &Path) -> Result<CloseOutcome, TraeInstanceCloseError> {
    close_instance_impl(instance_dir, command_line_uses_data_dir)
}

/// 关闭主库（官方目录）实例：额外覆盖官方无参启动的主进程形态
/// （用户从官方快捷方式手开的实例，切号第 1 步必须能关掉它）。
#[cfg(windows)]
pub fn close_master_instance(instance_dir: &Path) -> Result<CloseOutcome, TraeInstanceCloseError> {
    close_instance_impl(instance_dir, command_line_matches_master)
}

/// 关闭实现：matcher 决定「哪些进程属于该实例」（账号 vs 主库口径）。
#[cfg(windows)]
fn close_instance_impl(
    instance_dir: &Path,
    matcher: fn(&str, &Path) -> bool,
) -> Result<CloseOutcome, TraeInstanceCloseError> {
    let running_pids = |processes: &[TraeProcessInfo]| -> Vec<u32> {
        processes
            .iter()
            .filter(|process| {
                process
                    .command_line
                    .as_deref()
                    .is_some_and(|line| matcher(line, instance_dir))
            })
            .map(|process| process.pid)
            .collect()
    };
    let processes = list_trae_processes().map_err(|_| TraeInstanceCloseError)?;
    let pids = running_pids(&processes);
    if pids.is_empty() {
        return Ok(CloseOutcome::NotRunning);
    }
    // 主进程 = 无 --type= 的进程（Electron 主进程；子进程随树关闭）。
    let main_pids: Vec<u32> = processes
        .iter()
        .filter(|process| {
            pids.contains(&process.pid)
                && process
                    .command_line
                    .as_deref()
                    .is_some_and(|line| !line.contains("--type="))
        })
        .map(|process| process.pid)
        .collect();
    for pid in &main_pids {
        run_taskkill(*pid, false);
    }
    std::thread::sleep(std::time::Duration::from_secs(
        GRACEFUL_CLOSE_WAIT_SECONDS,
    ));
    let processes = list_trae_processes().map_err(|_| TraeInstanceCloseError)?;
    let remaining = running_pids(&processes);
    if remaining.is_empty() {
        return Ok(CloseOutcome::Closed);
    }
    for pid in &remaining {
        run_taskkill(*pid, true);
    }
    std::thread::sleep(std::time::Duration::from_secs(FORCE_CLOSE_WAIT_SECONDS));
    // 强制结束后的退出确认：终止落地存在滞后，逐次复查而非单次判定，
    // 避免把已成功的关闭误报为失败（详见 CLOSE_VERIFY_ATTEMPTS 注释）。
    for attempt in 0..CLOSE_VERIFY_ATTEMPTS {
        let processes = list_trae_processes().map_err(|_| TraeInstanceCloseError)?;
        if running_pids(&processes).is_empty() {
            return Ok(CloseOutcome::ForceClosed);
        }
        if attempt + 1 < CLOSE_VERIFY_ATTEMPTS {
            std::thread::sleep(std::time::Duration::from_secs(
                CLOSE_VERIFY_RETRY_DELAY_SECONDS,
            ));
        }
    }
    Err(TraeInstanceCloseError)
}

#[cfg(all(test, windows))]
mod tests {
    use std::fs;
    use std::path::Path;

    use tempfile::tempdir;

    use super::{
        command_line_matches_master, command_line_uses_data_dir, dir_size_recursive,
        instance_data_dir, instance_login_state,
        is_session_dir_name, native_account_dir,
        parse_process_json, seed_login_state_from_donor, seed_login_state_with_appdata,
        write_window_title, InstanceLoginState, SeedOutcome, TraeInstanceError,
    };

    #[test]
    fn instance_dir_rejects_unsafe_profile_ids() {
        let root = tempdir().unwrap();
        assert!(instance_data_dir(root.path(), "checkin-abc123").is_ok());
        for bad in [
            "",
            "a".repeat(65).as_str(),
            "profile/../escape",
            "PROFILE-UPPER",
            "profile with space",
            "profile\\slash",
        ] {
            assert_eq!(
                instance_data_dir(root.path(), bad).unwrap_err(),
                TraeInstanceError::InvalidProfileId,
                "profile_id 应被拒绝: {bad}"
            );
        }
    }

    #[test]
    fn window_title_merges_into_existing_settings() {
        // A2：首次写入生成仅含 window.title 的 settings.json。
        let root = tempdir().unwrap();
        write_window_title(root.path(), "账号甲").unwrap();
        let raw = fs::read_to_string(root.path().join("User").join("settings.json")).unwrap();
        let value: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(value["window.title"], serde_json::json!("账号甲"));
        assert_eq!(value.as_object().unwrap().len(), 1);

        // 二次写入：保留其他设置键，仅更新 window.title（幂等合并）。
        fs::write(
            root.path().join("User").join("settings.json"),
            r#"{"editor.fontSize": 14, "window.title": "旧标题"}"#,
        )
        .unwrap();
        write_window_title(root.path(), "账号乙").unwrap();
        let raw = fs::read_to_string(root.path().join("User").join("settings.json")).unwrap();
        let value: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(value["window.title"], serde_json::json!("账号乙"));
        assert_eq!(value["editor.fontSize"], serde_json::json!(14));

        // 已有文件但内容非法：退化为仅含新键的对象，不报错。
        fs::write(root.path().join("User").join("settings.json"), "not-json").unwrap();
        write_window_title(root.path(), "账号丙").unwrap();
        let raw = fs::read_to_string(root.path().join("User").join("settings.json")).unwrap();
        let value: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(value["window.title"], serde_json::json!("账号丙"));
        assert_eq!(value.as_object().unwrap().len(), 1);
    }

    #[test]
    fn seed_from_donor_seeds_empty_env_and_is_idempotent() {
        // P6-4 空环境首登：从账号登录存档移植三件套（storage.json +
        // state.vscdb + machineid），移植即登录。
        let root = tempdir().unwrap();
        let donor = root.path().join("trae-instances").join("checkin-abc");
        let env = root.path().join("environments").join("env-1-a");
        fs::create_dir_all(donor.join("User").join("globalStorage")).unwrap();
        fs::write(
            donor.join("User").join("globalStorage").join("storage.json"),
            r#"{"iCubeAuthInfo://usertag":"blob"}"#,
        )
        .unwrap();
        fs::write(
            donor.join("User").join("globalStorage").join("state.vscdb"),
            vec![1u8; 16],
        )
        .unwrap();
        fs::write(donor.join("machineid"), "machine-123").unwrap();

        // 空环境播种：三件套就位。
        assert_eq!(
            seed_login_state_from_donor(&env, &donor).unwrap(),
            SeedOutcome::Seeded
        );
        assert!(env.join("User").join("globalStorage").join("storage.json").is_file());
        assert!(env.join("User").join("globalStorage").join("state.vscdb").is_file());
        assert_eq!(
            fs::read_to_string(env.join("machineid")).unwrap(),
            "machine-123"
        );

        // 幂等：环境已有登录态时不动文件（NotNeeded）。
        fs::write(
            env.join("User").join("globalStorage").join("storage.json"),
            r#"{"iCubeAuthInfo://usertag":"changed"}"#,
        )
        .unwrap();
        assert_eq!(
            seed_login_state_from_donor(&env, &donor).unwrap(),
            SeedOutcome::NotNeeded
        );
        assert_eq!(
            fs::read_to_string(env.join("User").join("globalStorage").join("storage.json")).unwrap(),
            r#"{"iCubeAuthInfo://usertag":"changed"}"#
        );
    }

    #[test]
    fn seed_from_donor_overwrites_logged_out_env() {
        // 环境启动过但未登录（storage.json 只有设备键、无登录 blob）：
        // 没有可互换的登录材料，播种覆盖移植（供体 storage.json 整体覆盖）。
        let root = tempdir().unwrap();
        let donor = root.path().join("trae-instances").join("checkin-abc");
        let env = root.path().join("environments").join("env-2-a");
        fs::create_dir_all(donor.join("User").join("globalStorage")).unwrap();
        fs::write(
            donor.join("User").join("globalStorage").join("storage.json"),
            r#"{"iCubeAuthInfo://usertag":"blob"}"#,
        )
        .unwrap();
        fs::create_dir_all(env.join("User").join("globalStorage")).unwrap();
        fs::write(
            env.join("User").join("globalStorage").join("storage.json"),
            r#"{"icube-dc":"device-key-only"}"#,
        )
        .unwrap();

        assert_eq!(
            seed_login_state_from_donor(&env, &donor).unwrap(),
            SeedOutcome::Seeded
        );
        // 登录键来自供体（覆盖后环境登录态 = 供体账号）。
        let raw = fs::read_to_string(env.join("User").join("globalStorage").join("storage.json"))
            .unwrap();
        assert!(raw.contains("iCubeAuthInfo://usertag"));
    }

    #[test]
    fn seed_from_donor_fails_without_donor_archive() {
        // 供体无登录存档（账号从未产生三件套）：报错交调用方提示重新登录。
        let root = tempdir().unwrap();
        let donor = root.path().join("trae-instances").join("checkin-none");
        let env = root.path().join("environments").join("env-1-b");
        assert_eq!(
            seed_login_state_from_donor(&env, &donor).unwrap_err(),
            TraeInstanceError::SeedCopyFailed
        );
        // 目录未创建（失败不留半成品痕迹）。
        assert!(!env.join("User").join("globalStorage").join("storage.json").exists());
    }

    #[test]
    fn native_account_dir_requires_numeric_account_id() {
        // U-6 W4：占用统计与彻底删除按同一命名规则定位原生目录。
        let appdata = Path::new(r"C:\Users\demo\AppData\Roaming");
        assert_eq!(
            native_account_dir(appdata, "1234567890").unwrap(),
            appdata.join("TRAE SOLO CN_1234567890")
        );
        // 非纯数字 account_id 一律拒绝（路径注入防御）。
        for bad in ["", "abc", "12-34", "../escape", "12 34"] {
            assert!(native_account_dir(appdata, bad).is_none(), "应拒绝: {bad}");
        }
    }

    #[test]
    fn dir_size_sums_files_recursively_and_missing_is_zero() {
        // U-6 W4 占用统计口径：递归求和 + 目录缺失记 0。
        let root = tempdir().unwrap();
        let dir = root.path().join("inst");
        fs::create_dir_all(dir.join("User").join("globalStorage")).unwrap();
        fs::write(dir.join("machineid"), vec![0u8; 10]).unwrap();
        fs::write(
            dir.join("User")
                .join("globalStorage")
                .join("storage.json"),
            vec![0u8; 26],
        )
        .unwrap();
        assert_eq!(dir_size_recursive(&dir), 36);
        // 缺失目录记 0（未启动过实例/原生客户端未登录过该账号）。
        assert_eq!(dir_size_recursive(&root.path().join("missing")), 0);
    }

    #[test]
    fn process_json_parses_array_single_and_empty() {
        let array = r#"[{"ProcessId":123,"ExecutablePath":"E:\\TRAE.exe","CommandLine":"\"E:\\TRAE.exe\" --user-data-dir=D:\\a"}]"#;
        let parsed = parse_process_json(array);
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].pid, 123);
        assert_eq!(parsed[0].exe_path.as_deref(), Some("E:\\TRAE.exe"));

        // 单对象（ConvertTo-Json 对单元素数组的退化形态）也要兼容。
        let single = r#"{"ProcessId":7,"ExecutablePath":null,"CommandLine":null}"#;
        let parsed = parse_process_json(single);
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].pid, 7);
        assert_eq!(parsed[0].exe_path, None);

        assert!(parse_process_json("").is_empty());
        assert!(parse_process_json("not json").is_empty());
    }

    #[test]
    fn command_line_matching_ignores_quotes_and_case() {
        let dir = std::path::Path::new(r"D:\Work\Trae Sync\data\trae-instances\checkin-a");
        let quoted = r#""E:\x\TRAE.exe" --user-data-dir="D:\Work\Trae Sync\data\trae-instances\checkin-a" --type=gpu"#;
        let plain = "--user-data-dir=d:\\work\\trae sync\\data\\trae-instances\\CHECKIN-A";
        assert!(command_line_uses_data_dir(quoted, dir));
        assert!(command_line_uses_data_dir(plain, dir));

        // 前缀目录不误判：checkin-ab 不是 checkin-a 的运行实例。
        let other = r#""E:\x\TRAE.exe" --user-data-dir=D:\Work\Trae Sync\data\trae-instances\checkin-ab"#;
        assert!(!command_line_uses_data_dir(other, dir));
        // 无关命令行。
        assert!(!command_line_uses_data_dir(r#""E:\x\TRAE.exe""#, dir));
    }

    #[test]
    fn master_matching_covers_official_bare_launch() {
        let official = std::path::Path::new(r"C:\Users\u\AppData\Roaming\TRAE SOLO CN");
        // 官方快捷方式无参启动的主进程：默认 data_dir 即官方目录。
        let bare = r#""E:\软件\TRAE\TRAE SOLO CN\TRAE SOLO CN.exe""#;
        assert!(command_line_matches_master(bare, official));
        // 带参形态（App 启动或官方实例子进程）。
        let quoted = r#""E:\x\TRAE.exe" --type=gpu-process --user-data-dir="C:\Users\u\AppData\Roaming\TRAE SOLO CN""#;
        assert!(command_line_matches_master(quoted, official));

        // 账号实例（显式指向别的目录）不误判为主库。
        let account = r#""E:\x\TRAE.exe" --user-data-dir=D:\Work\Trae Sync\data\trae-instances\checkin-a"#;
        assert!(!command_line_matches_master(account, official));
        // 子进程形态（--type=）不带主库目录也不算（无参主进程仅限主进程）。
        let child = r#""E:\x\TRAE.exe" --type=utility --utility-sub-type=monitor"#;
        assert!(!command_line_matches_master(child, official));
    }

    #[test]
    fn login_state_follows_storage_json_usertag_key() {
        let root = tempdir().unwrap();
        let instance_dir = root.path().join("inst");
        fs::create_dir_all(&instance_dir).unwrap();

        // 未初始化：实例目录存在但 TRAE 从未启动写入 storage.json。
        assert_eq!(
            instance_login_state(&instance_dir),
            InstanceLoginState::Uninitialized
        );

        // 启动过但未登录：storage.json 存在但无 usertag 键。
        let storage = instance_dir.join("User").join("globalStorage");
        fs::create_dir_all(&storage).unwrap();
        fs::write(storage.join("storage.json"), r#"{"theme":"dark"}"#).unwrap();
        assert_eq!(
            instance_login_state(&instance_dir),
            InstanceLoginState::LoggedOut
        );

        // 已登录：storage.json 含 usertag 键。
        fs::write(
            storage.join("storage.json"),
            r#"{"iCubeAuthInfo://usertag":"blob","theme":"dark"}"#,
        )
        .unwrap();
        assert_eq!(
            instance_login_state(&instance_dir),
            InstanceLoginState::LoggedIn
        );

        // 文件损坏（解析失败）按待登录处理：引导一次登录无害（幂等）。
        fs::write(storage.join("storage.json"), "not-json").unwrap();
        assert_eq!(
            instance_login_state(&instance_dir),
            InstanceLoginState::LoggedOut
        );
    }

    /// 在实例目录下写一个会话的 renderer.log，返回日志文件路径。
    fn write_session_log(instance_dir: &Path, session: &str, lines: &str) {
        let dir = instance_dir.join("logs").join(session).join("window1");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("renderer.log"), lines).unwrap();
    }

    #[test]
    fn login_state_stale_when_log_shows_auth_rejected() {
        let root = tempdir().unwrap();
        let instance_dir = root.path().join("inst");
        let storage = instance_dir.join("User").join("globalStorage");
        fs::create_dir_all(&storage).unwrap();
        // LY 案例（2026-08-24 实证）：blob 键仍在，但启动日志含服务端拒绝证据。
        fs::write(
            storage.join("storage.json"),
            r#"{"iCubeAuthInfo://usertag":"blob"}"#,
        )
        .unwrap();
        write_session_log(
            &instance_dir,
            "20260824T021507",
            "[StoragePort] User not authenticated, userId is undefined\n[ckg] not login, skip setup",
        );
        assert_eq!(
            instance_login_state(&instance_dir),
            InstanceLoginState::Stale
        );
    }

    #[test]
    fn login_state_positive_marker_wins_over_earlier_rejection() {
        let root = tempdir().unwrap();
        let instance_dir = root.path().join("inst");
        let storage = instance_dir.join("User").join("globalStorage");
        fs::create_dir_all(&storage).unwrap();
        fs::write(
            storage.join("storage.json"),
            r#"{"iCubeAuthInfo://usertag":"blob"}"#,
        )
        .unwrap();
        // 登录流程时序：启动时先报未登录，用户登录后出现正向标记 → 已登录。
        write_session_log(
            &instance_dir,
            "20260824T021507",
            "User not authenticated, userId is undefined\n[RouteService] User info loaded {\"userId\":\"123\"}",
        );
        assert_eq!(
            instance_login_state(&instance_dir),
            InstanceLoginState::LoggedIn
        );
    }

    #[test]
    fn login_state_uses_latest_session_only() {
        let root = tempdir().unwrap();
        let instance_dir = root.path().join("inst");
        let storage = instance_dir.join("User").join("globalStorage");
        fs::create_dir_all(&storage).unwrap();
        fs::write(
            storage.join("storage.json"),
            r#"{"iCubeAuthInfo://usertag":"blob"}"#,
        )
        .unwrap();
        // 旧会话登录成功，最新会话被拒 → 以最新会话为准（Stale）。
        write_session_log(
            &instance_dir,
            "20260823T195626",
            "[RouteService] User info loaded {\"userId\":\"123\"}",
        );
        write_session_log(
            &instance_dir,
            "20260824T021507",
            "[StoragePort] User not authenticated",
        );
        assert_eq!(
            instance_login_state(&instance_dir),
            InstanceLoginState::Stale
        );
    }

    #[test]
    fn login_state_ignores_non_session_dirs() {
        let root = tempdir().unwrap();
        let instance_dir = root.path().join("inst");
        let storage = instance_dir.join("User").join("globalStorage");
        fs::create_dir_all(&storage).unwrap();
        fs::write(
            storage.join("storage.json"),
            r#"{"iCubeAuthInfo://usertag":"blob"}"#,
        )
        .unwrap();
        // 非 TRAE 时间戳命名的日志目录（如 aha_log）不参与判定。
        write_session_log(&instance_dir, "aha_log", "User not authenticated");
        assert_eq!(
            instance_login_state(&instance_dir),
            InstanceLoginState::LoggedIn
        );
        assert!(!is_session_dir_name("aha_log"));
        assert!(is_session_dir_name("20260824T021507"));
    }

    #[test]
    fn seed_copies_login_state_and_is_idempotent() {
        // 模拟 TRAE 原生账号目录结构（appdata 根由测试注入）。
        let appdata = tempdir().unwrap();
        let native = appdata.path().join("TRAE SOLO CN_1234567890");
        let native_storage = native.join("User").join("globalStorage");
        fs::create_dir_all(&native_storage).unwrap();
        fs::write(native_storage.join("storage.json"), b"{\"auth\":1}").unwrap();
        fs::write(native_storage.join("state.vscdb"), b"db-bytes").unwrap();
        fs::write(native.join("machineid"), b"machine-abc").unwrap();

        let instance = tempdir().unwrap();
        let instance_dir = instance.path().join("trae-instances").join("checkin-a");
        fs::create_dir_all(&instance_dir).unwrap();

        // 首次：播种。
        let outcome =
            seed_login_state_with_appdata(&instance_dir, "1234567890", appdata.path()).unwrap();
        assert_eq!(outcome, SeedOutcome::Seeded);
        assert!(instance_dir.join("User").join("globalStorage").join("storage.json").is_file());
        assert!(instance_dir.join("User").join("globalStorage").join("state.vscdb").is_file());
        assert!(instance_dir.join("machineid").is_file());

        // 再次：幂等跳过。
        let outcome =
            seed_login_state_with_appdata(&instance_dir, "1234567890", appdata.path()).unwrap();
        assert_eq!(outcome, SeedOutcome::NotNeeded);

        // account_id 非法（路径注入）被拒绝。
        assert_eq!(
            seed_login_state_with_appdata(&instance_dir, "../escape", appdata.path()).unwrap_err(),
            TraeInstanceError::InvalidAccountId
        );
    }

    #[test]
    fn seed_reports_native_missing_for_unknown_account() {
        let appdata = tempdir().unwrap();
        let instance = tempdir().unwrap();
        let instance_dir = instance.path().join("inst");
        fs::create_dir_all(&instance_dir).unwrap();
        let outcome =
            seed_login_state_with_appdata(&instance_dir, "9876543210987654", appdata.path())
                .unwrap();
        assert_eq!(outcome, SeedOutcome::NativeMissing);
    }
}
