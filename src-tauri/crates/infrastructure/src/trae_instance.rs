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
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use crate::product_adapter::TraeProduct;

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
    native_account_dir_for(TraeProduct::WorkCn, appdata, account_id)
}

/// 按产品形态定位原生账号目录；未确认的产品规则安全返回 None。
pub fn native_account_dir_for(
    product: TraeProduct,
    appdata: &Path,
    account_id: &str,
) -> Option<PathBuf> {
    if account_id.is_empty() || !account_id.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let prefix = product.native_account_dir_prefix()?;
    Some(appdata.join(format!("{prefix}_{account_id}")))
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

/// 进程与目标产品目录的归属判定。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessMatch {
    /// 产品和目录都能确认匹配。
    Match,
    /// 进程可确认属于别的产品或目录。
    NoMatch,
    /// 缺少身份材料，不能证明进程与目标无关。
    Unknown,
}

/// 多进程快照相对某个产品目录的状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProductProcessState {
    Running,
    NotRunning,
    Unknown,
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

// ===== P15-11 进程查询 TTL 缓存（2026-10-15）=====

/// 进程查询缓存条目：采样时刻 + 快照。
struct ProcessCacheEntry {
    at: Instant,
    processes: Vec<TraeProcessInfo>,
}

/// 进程查询缓存状态机（与真实查询解耦，单测注入 `Instant`）。
#[derive(Default)]
struct ProcessCacheState {
    entry: Option<ProcessCacheEntry>,
}

impl ProcessCacheState {
    /// 环境页轮询间隔 5s，3s TTL 下轮询几乎全命中；对用户手动开/关客户端
    /// 的最坏感知延迟 3s（工具自身干预后由显式失效覆盖，不走 TTL）。
    const TTL: Duration = Duration::from_secs(3);

    fn get_fresh(&self, now: Instant) -> Option<Vec<TraeProcessInfo>> {
        let entry = self.entry.as_ref()?;
        (now.duration_since(entry.at) <= Self::TTL).then(|| entry.processes.clone())
    }

    fn store(&mut self, now: Instant, processes: Vec<TraeProcessInfo>) {
        self.entry = Some(ProcessCacheEntry { at: now, processes });
    }

    fn invalidate(&mut self) {
        self.entry = None;
    }
}

static PROCESS_CACHE: OnceLock<Mutex<ProcessCacheState>> = OnceLock::new();

fn process_cache() -> &'static Mutex<ProcessCacheState> {
    PROCESS_CACHE.get_or_init(|| Mutex::new(ProcessCacheState::default()))
}

/// 工具自身对进程的干预（taskkill / 启动实例）之后调用：清空缓存，保证
/// 关闭复查与启动后的状态查询读到真实最新快照（不受 TTL 影响）。
pub fn invalidate_process_cache() {
    if let Ok(mut state) = process_cache().lock() {
        state.invalidate();
    }
}

/// 查询当前全部 TRAE IDE 进程（不含本应用自身）。
/// 首次调用冷启动 PowerShell 约 0.5~1 秒；调用方应放在阻塞线程。
/// P15-11：TTL 3s 内复用同一快照（环境页每 5s 双命令轮询 1.4s/次 → 0）；
/// 工具自身干预（taskkill/launch）后经 [`invalidate_process_cache`] 立即失效。
#[cfg(windows)]
pub fn list_trae_processes() -> Result<Vec<TraeProcessInfo>, TraeInstanceError> {
    let now = Instant::now();
    if let Ok(state) = process_cache().lock() {
        if let Some(cached) = state.get_fresh(now) {
            return Ok(cached);
        }
    }
    let processes = query_trae_processes()?;
    if let Ok(mut state) = process_cache().lock() {
        state.store(Instant::now(), processes.clone());
    }
    Ok(processes)
}

/// 真实查询（PowerShell Win32_Process；无缓存路径，供缓存壳与复查链使用）。
#[cfg(windows)]
fn query_trae_processes() -> Result<Vec<TraeProcessInfo>, TraeInstanceError> {
    use std::os::windows::process::CommandExt;
    // CREATE_NO_WINDOW：避免每次查询闪出 PowerShell 控制台窗口。
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let output = std::process::Command::new("powershell")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            PROCESS_QUERY_SCRIPT,
        ])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map_err(|_| TraeInstanceError::ProcessQueryFailed)?;
    if !output.status.success() {
        return Err(TraeInstanceError::ProcessQueryFailed);
    }
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    parse_process_json(&stdout)
}

/// 解析 `ConvertTo-Json` 输出的进程数组（纯函数，可单测）。
/// 容忍单对象退化形态；坏快照或缺 PID 整体失败，身份字段缺失则保留为未知。
pub fn parse_process_json(raw: &str) -> Result<Vec<TraeProcessInfo>, TraeInstanceError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(TraeInstanceError::ProcessQueryFailed);
    }
    let value = serde_json::from_str::<serde_json::Value>(trimmed)
        .map_err(|_| TraeInstanceError::ProcessQueryFailed)?;
    let rows: Vec<&serde_json::Value> = match &value {
        serde_json::Value::Array(items) => items.iter().collect(),
        object @ serde_json::Value::Object(_) => vec![object],
        _ => return Err(TraeInstanceError::ProcessQueryFailed),
    };
    rows.iter()
        .map(|row| {
            let pid = row
                .get("ProcessId")
                .and_then(serde_json::Value::as_u64)
                .and_then(|pid| u32::try_from(pid).ok())
                .filter(|pid| *pid != 0)
                .ok_or(TraeInstanceError::ProcessQueryFailed)?;
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
            Ok(TraeProcessInfo {
                pid,
                exe_path,
                command_line,
            })
        })
        .collect()
}

/// 当前产品默认数据根；APPDATA 缺失或不是绝对路径时返回 None，由调用者按未知处理。
pub fn default_product_data_dir(product: TraeProduct) -> Option<PathBuf> {
    default_product_data_dir_from_appdata(product, std::env::var_os("APPDATA").map(PathBuf::from))
}

fn default_product_data_dir_from_appdata(
    product: TraeProduct,
    appdata: Option<PathBuf>,
) -> Option<PathBuf> {
    let appdata = appdata.filter(|path| path.is_absolute())?;
    Some(appdata.join(product.data_root_name()))
}

/// 判断单个 TRAE 进程是否属于指定产品和数据目录。
///
/// 官方默认目录的无参主进程只在 exe 产品匹配且目录确为该产品默认根时成立；
/// 无参数子进程、缺失身份或产品/目录矛盾均返回 Unknown，不能授权危险写入。
pub fn match_product_process(
    process: &TraeProcessInfo,
    product: TraeProduct,
    data_dir: &Path,
    default_data_dir: Option<&Path>,
) -> ProcessMatch {
    let Some(exe_path) = process.exe_path.as_deref() else {
        return ProcessMatch::Unknown;
    };
    let Some(command_line) = process.command_line.as_deref() else {
        return ProcessMatch::Unknown;
    };
    if command_line.trim().is_empty() {
        return ProcessMatch::Unknown;
    }
    let Some(exe_product) = known_executable_product(Path::new(exe_path)) else {
        return ProcessMatch::Unknown;
    };

    match command_line_data_dir_arg(command_line) {
        UserDataDirArg::Value(value) => {
            if windows_path_eq(Path::new(&value), data_dir) {
                return if exe_product == product {
                    ProcessMatch::Match
                } else {
                    ProcessMatch::Unknown
                };
            }
            return ProcessMatch::NoMatch;
        }
        UserDataDirArg::Invalid => return ProcessMatch::Unknown,
        UserDataDirArg::Absent => {}
    }

    if exe_product != product {
        return ProcessMatch::NoMatch;
    }

    let is_default_target =
        default_data_dir.is_some_and(|default_dir| windows_path_eq(default_dir, data_dir));
    if command_line.to_ascii_lowercase().contains("--type=") {
        // 子进程通常继承主进程的数据目录，但部分启动形态不会在自身命令行重复参数。
        // 因而不能仅凭“不是默认目录”证明它与目标实例无关。
        return ProcessMatch::Unknown;
    }
    if is_default_target {
        ProcessMatch::Match
    } else if default_data_dir.is_none() {
        ProcessMatch::Unknown
    } else {
        ProcessMatch::NoMatch
    }
}

/// 汇总进程快照；明确运行优先，其次未知，只有全量可归属才报告未运行。
pub fn product_process_state(
    processes: &[TraeProcessInfo],
    product: TraeProduct,
    data_dir: &Path,
    default_data_dir: Option<&Path>,
) -> ProductProcessState {
    let mut unknown = false;
    for process in processes {
        match match_product_process(process, product, data_dir, default_data_dir) {
            ProcessMatch::Match => return ProductProcessState::Running,
            ProcessMatch::NoMatch => {}
            ProcessMatch::Unknown => unknown = true,
        }
    }
    if unknown {
        ProductProcessState::Unknown
    } else {
        ProductProcessState::NotRunning
    }
}

/// 将进程查询结果转换为状态；枚举失败时保持 Unknown。
fn product_process_state_from_query(
    processes: Result<Vec<TraeProcessInfo>, TraeInstanceError>,
    product: TraeProduct,
    data_dir: &Path,
    default_data_dir: Option<&Path>,
) -> ProductProcessState {
    match processes {
        Ok(processes) => product_process_state(&processes, product, data_dir, default_data_dir),
        Err(_) => ProductProcessState::Unknown,
    }
}

/// 取得可安全聚焦或关闭的目标进程；任何无法归属的 TRAE 候选都会拒绝操作。
pub fn matched_product_process_ids(
    processes: &[TraeProcessInfo],
    product: TraeProduct,
    data_dir: &Path,
    default_data_dir: Option<&Path>,
) -> Result<Vec<u32>, ProductProcessState> {
    let mut matching_pids = Vec::new();
    let mut unknown = false;
    for process in processes {
        match match_product_process(process, product, data_dir, default_data_dir) {
            ProcessMatch::Match => matching_pids.push(process.pid),
            ProcessMatch::NoMatch => {}
            ProcessMatch::Unknown => unknown = true,
        }
    }
    if unknown {
        Err(ProductProcessState::Unknown)
    } else {
        Ok(matching_pids)
    }
}

/// 查询当前产品与数据目录的进程状态；枚举失败或平台不支持时保留未知。
#[cfg(windows)]
pub fn current_product_process_state(product: TraeProduct, data_dir: &Path) -> ProductProcessState {
    let default_data_dir = default_product_data_dir(product);
    product_process_state_from_query(
        list_trae_processes(),
        product,
        data_dir,
        default_data_dir.as_deref(),
    )
}

/// 非 Windows 环境不能确认 TRAE 桌面进程状态，危险写入必须停止。
#[cfg(not(windows))]
pub fn current_product_process_state(
    _product: TraeProduct,
    _data_dir: &Path,
) -> ProductProcessState {
    ProductProcessState::Unknown
}

fn known_executable_product(path: &Path) -> Option<TraeProduct> {
    [TraeProduct::WorkCn, TraeProduct::TraeCn]
        .into_iter()
        .find(|product| product.matches_executable(path))
}

/// `--user-data-dir` 的解析结果。
///
/// 只把真正的命令行参数当作目录参数；目录字符串出现在其他参数中不算。
#[derive(Debug, Clone, PartialEq, Eq)]
enum UserDataDirArg {
    Absent,
    Value(String),
    Invalid,
}

/// 按 Windows 命令行规则拆分参数。
///
/// 这里只处理进程查询需要的引号、空白和反斜杠转义，不依赖 Windows API，
/// 使纯函数边界可以在同一模块中直接测试。
fn split_windows_command_line(command_line: &str) -> Option<Vec<String>> {
    let chars: Vec<char> = command_line.chars().collect();
    let mut args = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;
    let mut token_started = false;
    let mut index = 0;

    while index < chars.len() {
        match chars[index] {
            '\\' => {
                let start = index;
                while index < chars.len() && chars[index] == '\\' {
                    index += 1;
                }
                let slash_count = index - start;
                if index < chars.len() && chars[index] == '"' {
                    current.extend(std::iter::repeat_n('\\', slash_count / 2));
                    if slash_count % 2 == 1 {
                        current.push('"');
                        token_started = true;
                        index += 1;
                    } else {
                        in_quotes = !in_quotes;
                        token_started = true;
                        index += 1;
                    }
                } else {
                    current.extend(std::iter::repeat_n('\\', slash_count));
                    token_started = true;
                }
            }
            '"' => {
                in_quotes = !in_quotes;
                token_started = true;
                index += 1;
            }
            character if character.is_ascii_whitespace() && !in_quotes => {
                if token_started {
                    args.push(std::mem::take(&mut current));
                    token_started = false;
                }
                index += 1;
            }
            character => {
                current.push(character);
                token_started = true;
                index += 1;
            }
        }
    }

    if in_quotes {
        return None;
    }
    if token_started {
        args.push(current);
    }
    Some(args)
}

/// 解析唯一的 `--user-data-dir` 实际值。
///
/// 参数缺值、空值、重复出现或引号未闭合都返回 `Invalid`，调用方必须按
/// `Unknown` 处理，不能退化成默认目录匹配。
fn command_line_data_dir_arg(command_line: &str) -> UserDataDirArg {
    let Some(args) = split_windows_command_line(command_line) else {
        return UserDataDirArg::Invalid;
    };
    let mut value = None;
    let mut index = 0;
    while index < args.len() {
        let argument = &args[index];
        let lowered = argument.to_ascii_lowercase();
        let inline = lowered.strip_prefix("--user-data-dir=");
        if lowered == "--user-data-dir" {
            if value.is_some() {
                return UserDataDirArg::Invalid;
            }
            let Some(next) = args.get(index + 1) else {
                return UserDataDirArg::Invalid;
            };
            if next.is_empty() || next.starts_with('-') {
                return UserDataDirArg::Invalid;
            }
            value = Some(next.clone());
            index += 2;
            continue;
        }
        if let Some(inline) = inline {
            if value.is_some() || inline.is_empty() {
                return UserDataDirArg::Invalid;
            }
            value = Some(argument["--user-data-dir=".len()..].to_string());
        }
        index += 1;
    }
    value.map_or(UserDataDirArg::Absent, UserDataDirArg::Value)
}

fn windows_path_eq(left: &Path, right: &Path) -> bool {
    let normalize = |path: &Path| {
        path.to_string_lossy()
            .replace('/', "\\")
            .trim_end_matches('\\')
            .to_ascii_lowercase()
    };
    normalize(left) == normalize(right)
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

/// 注册表记录中提取目标产品 exe 路径；DisplayIcon 优先，InstallLocation 下
/// 扫描目标产品主程序文件名兜底。
fn exe_from_install_record(product: TraeProduct, record: &TraeInstallRecord) -> Option<PathBuf> {
    if let Some(icon) = &record.exe_path {
        let path = PathBuf::from(icon);
        if path.is_file() && product.matches_executable(&path) {
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
                    .filter(|path| path.is_file() && product.matches_executable(path))
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
    discover_trae_executable_for(TraeProduct::WorkCn, processes)
}

/// 发现指定 TRAE 产品的可执行文件路径。
///
/// 产品适配入口只返回目标产品的主程序，避免 Work CN 与 Trae CN 同时运行时
/// 错选另一个产品；默认入口保留 Work CN 行为。
pub fn discover_trae_executable_for(
    product: TraeProduct,
    processes: &[TraeProcessInfo],
) -> Result<PathBuf, TraeInstanceError> {
    // 1. 运行中进程的真实路径。
    for process in processes {
        if let Some(exe) = &process.exe_path {
            let path = PathBuf::from(exe);
            if path.is_file() && product.matches_executable(&path) {
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
                // 目标产品优先，其余记录保持稳定顺序，避免跨产品误选。
                let mut ordered: Vec<&TraeInstallRecord> = records.iter().collect();
                ordered.sort_by_key(|record| !product.matches_install_name(&record.display_name));
                for record in ordered {
                    if let Some(exe) = exe_from_install_record(product, record) {
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
                            .is_some_and(|name| name.to_ascii_lowercase().starts_with("trae"))
                })
                .collect();
            roots.sort();
            for root in roots {
                if let Ok(files) = std::fs::read_dir(&root) {
                    let mut candidates: Vec<PathBuf> = files
                        .flatten()
                        .map(|entry| entry.path())
                        .filter(|path| path.is_file() && product.matches_executable(path))
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
            &mut context as *mut FocusContext as windows_sys::Win32::Foundation::LPARAM,
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
    let renderer_log = logs_dir.join(latest).join("window1").join("renderer.log");
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
/// LY/import 均含此键，未登录实例账号M不含），键存在时再用最近启动日志
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
fn copy_dir_best_effort(source: &Path, destination: &Path) -> Result<(), TraeInstanceError> {
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

/// 实例关闭错误；身份无法确认与确认关闭失败需要区别处理。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TraeInstanceCloseError {
    /// 进程枚举失败或候选进程身份不完整，不能证明目标实例已安全关闭。
    ProcessStateUnknown,
    /// 目标进程身份明确，但关闭后仍无法确认其退出。
    CloseFailed,
}

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
    let success = command
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false);
    // P15-11：对进程的干预立即失效查询缓存——关闭编排的复查链（3s 内的
    // 多次 query_processes）必须读到真实状态，不能吃干预前的旧快照。
    invalidate_process_cache();
    success
}

/// 关闭运行于指定产品与 data_dir 的 TRAE 实例。
///
/// 策略（2026-08-23 E2E 实测：WM_CLOSE 12 秒不退出，TRAE 疑似驻留托盘）：
/// 1. 对主进程 taskkill /T（WM_CLOSE 优雅关闭）；
/// 2. 等待 3 秒复查，仍存活则 taskkill /T /F 强制结束全部进程；
/// 3. 强制后再复查，仍存在视为失败。
/// 只影响该 data_dir 的进程树，其他实例与用户手动打开的 TRAE 不受波及。
#[cfg(windows)]
pub fn close_instance(
    product: TraeProduct,
    instance_dir: &Path,
) -> Result<CloseOutcome, TraeInstanceCloseError> {
    close_instance_impl(product, instance_dir)
}

/// 关闭实现：产品与目录 matcher 决定哪些进程属于该实例。
#[cfg(windows)]
fn close_instance_impl(
    product: TraeProduct,
    instance_dir: &Path,
) -> Result<CloseOutcome, TraeInstanceCloseError> {
    let default_data_dir = default_product_data_dir(product);
    close_instance_with(
        product,
        instance_dir,
        default_data_dir.as_deref(),
        || list_trae_processes().map_err(|_| TraeInstanceCloseError::ProcessStateUnknown),
        run_taskkill,
        std::thread::sleep,
    )
}

/// 注入进程观察、结束动作和等待；离线测试可验证目标进程集合而不触碰真实 TRAE。
#[cfg(windows)]
fn close_instance_with(
    product: TraeProduct,
    instance_dir: &Path,
    default_data_dir: Option<&Path>,
    mut query_processes: impl FnMut() -> Result<Vec<TraeProcessInfo>, TraeInstanceCloseError>,
    mut taskkill: impl FnMut(u32, bool) -> bool,
    mut wait: impl FnMut(std::time::Duration),
) -> Result<CloseOutcome, TraeInstanceCloseError> {
    let running_pids = |processes: &[TraeProcessInfo]| {
        matched_product_process_ids(
            processes,
            product,
            instance_dir,
            default_data_dir.as_deref(),
        )
        .map_err(|_| TraeInstanceCloseError::ProcessStateUnknown)
    };
    let processes = query_processes()?;
    let pids = running_pids(&processes)?;
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
        taskkill(*pid, false);
    }
    wait(std::time::Duration::from_secs(GRACEFUL_CLOSE_WAIT_SECONDS));
    let processes = query_processes()?;
    let remaining = running_pids(&processes)?;
    if remaining.is_empty() {
        return Ok(CloseOutcome::Closed);
    }
    for pid in &remaining {
        taskkill(*pid, true);
    }
    wait(std::time::Duration::from_secs(FORCE_CLOSE_WAIT_SECONDS));
    // 强制结束后的退出确认：终止落地存在滞后，逐次复查而非单次判定，
    // 避免把已成功的关闭误报为失败（详见 CLOSE_VERIFY_ATTEMPTS 注释）。
    for attempt in 0..CLOSE_VERIFY_ATTEMPTS {
        let processes = query_processes()?;
        if running_pids(&processes)?.is_empty() {
            return Ok(CloseOutcome::ForceClosed);
        }
        if attempt + 1 < CLOSE_VERIFY_ATTEMPTS {
            wait(std::time::Duration::from_secs(
                CLOSE_VERIFY_RETRY_DELAY_SECONDS,
            ));
        }
    }
    Err(TraeInstanceCloseError::CloseFailed)
}

#[cfg(all(test, windows))]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};

    use tempfile::tempdir;

    use crate::product_adapter::TraeProduct;

    use super::{
        close_instance_with, dir_size_recursive, discover_trae_executable_for, instance_data_dir,
        instance_login_state, is_session_dir_name, match_product_process, native_account_dir,
        native_account_dir_for, parse_process_json, product_process_state,
        seed_login_state_from_donor, seed_login_state_with_appdata, write_window_title,
        CloseOutcome, InstanceLoginState, ProcessMatch, ProductProcessState, SeedOutcome,
        TraeInstanceCloseError, TraeInstanceError, TraeProcessInfo,
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
            donor
                .join("User")
                .join("globalStorage")
                .join("storage.json"),
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
        assert!(env
            .join("User")
            .join("globalStorage")
            .join("storage.json")
            .is_file());
        assert!(env
            .join("User")
            .join("globalStorage")
            .join("state.vscdb")
            .is_file());
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
            fs::read_to_string(env.join("User").join("globalStorage").join("storage.json"))
                .unwrap(),
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
            donor
                .join("User")
                .join("globalStorage")
                .join("storage.json"),
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
        assert!(!env
            .join("User")
            .join("globalStorage")
            .join("storage.json")
            .exists());
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
        // CN 的原生账号目录规则尚未实测，适配层必须安全停用而不是猜路径。
        assert!(native_account_dir_for(TraeProduct::TraeCn, appdata, "1234567890").is_none());
    }

    #[test]
    fn executable_discovery_keeps_products_separate() {
        let root = tempdir().unwrap();
        let work = root.path().join("TRAE SOLO CN.exe");
        let cn = root.path().join("Trae CN.exe");
        fs::write(&work, b"work").unwrap();
        fs::write(&cn, b"cn").unwrap();
        let processes = vec![
            TraeProcessInfo {
                pid: 1,
                exe_path: Some(work.to_string_lossy().into_owned()),
                command_line: None,
            },
            TraeProcessInfo {
                pid: 2,
                exe_path: Some(cn.to_string_lossy().into_owned()),
                command_line: None,
            },
        ];
        assert_eq!(
            discover_trae_executable_for(TraeProduct::WorkCn, &processes).unwrap(),
            work
        );
        assert_eq!(
            discover_trae_executable_for(TraeProduct::TraeCn, &processes).unwrap(),
            cn
        );
    }

    #[test]
    fn dir_size_sums_files_recursively_and_missing_is_zero() {
        // U-6 W4 占用统计口径：递归求和 + 目录缺失记 0。
        let root = tempdir().unwrap();
        let dir = root.path().join("inst");
        fs::create_dir_all(dir.join("User").join("globalStorage")).unwrap();
        fs::write(dir.join("machineid"), vec![0u8; 10]).unwrap();
        fs::write(
            dir.join("User").join("globalStorage").join("storage.json"),
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
        let parsed = parse_process_json(array).unwrap();
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].pid, 123);
        assert_eq!(parsed[0].exe_path.as_deref(), Some("E:\\TRAE.exe"));

        // 单对象（ConvertTo-Json 对单元素数组的退化形态）也要兼容。
        let single = r#"{"ProcessId":7,"ExecutablePath":null,"CommandLine":null}"#;
        let parsed = parse_process_json(single).unwrap();
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].pid, 7);
        assert_eq!(parsed[0].exe_path, None);
        assert_eq!(parsed[0].command_line, None);

        assert!(parse_process_json("[]").unwrap().is_empty());
        assert!(parse_process_json("").is_err());
        assert!(parse_process_json("not json").is_err());
        assert!(parse_process_json(r#"[{"ExecutablePath":"E:\\TRAE.exe"}]"#).is_err());
    }

    #[test]
    fn product_process_match_requires_executable_and_data_directory() {
        let appdata = std::path::Path::new(r"C:\Users\u\AppData\Roaming");
        let work_root = appdata.join(TraeProduct::WorkCn.data_root_name());
        let cn_root = appdata.join(TraeProduct::TraeCn.data_root_name());
        let secondary = std::path::Path::new(r"D:\Trae Sync\environments\env-a");
        let process = |pid, exe: &str, command_line: &str| TraeProcessInfo {
            pid,
            exe_path: Some(exe.to_string()),
            command_line: Some(command_line.to_string()),
        };

        let work_bare = process(1, r"E:\TRAE SOLO CN.exe", r#""E:\TRAE SOLO CN.exe""#);
        let work_old_bare = process(2, r"E:\trae.exe", r#""E:\trae.exe""#);
        let cn_bare = process(3, r"E:\Trae CN.exe", r#""E:\Trae CN.exe""#);
        let work_child = process(
            4,
            r"E:\TRAE SOLO CN.exe",
            r#""E:\TRAE SOLO CN.exe" --type=renderer"#,
        );
        let work_secondary = process(
            5,
            r"E:\TRAE SOLO CN.exe",
            r#""E:\TRAE SOLO CN.exe" --user-data-dir="D:\Trae Sync\environments\env-a" --type=renderer"#,
        );

        assert_eq!(
            match_product_process(
                &work_bare,
                TraeProduct::WorkCn,
                &work_root,
                Some(&work_root)
            ),
            ProcessMatch::Match
        );
        assert_eq!(
            match_product_process(
                &work_old_bare,
                TraeProduct::WorkCn,
                &work_root,
                Some(&work_root)
            ),
            ProcessMatch::Match
        );
        assert_eq!(
            match_product_process(&cn_bare, TraeProduct::WorkCn, &work_root, Some(&work_root)),
            ProcessMatch::NoMatch
        );
        assert_eq!(
            match_product_process(&cn_bare, TraeProduct::TraeCn, &cn_root, Some(&cn_root)),
            ProcessMatch::Match
        );
        assert_eq!(
            match_product_process(
                &work_child,
                TraeProduct::WorkCn,
                &work_root,
                Some(&work_root)
            ),
            ProcessMatch::Unknown
        );
        assert_eq!(
            match_product_process(
                &work_secondary,
                TraeProduct::WorkCn,
                secondary,
                Some(&work_root)
            ),
            ProcessMatch::Match
        );
        assert_eq!(
            product_process_state(
                &[work_bare.clone(), cn_bare.clone()],
                TraeProduct::WorkCn,
                &work_root,
                Some(&work_root)
            ),
            ProductProcessState::Running
        );
        assert_eq!(
            product_process_state(
                &[work_bare.clone(), cn_bare.clone()],
                TraeProduct::TraeCn,
                &cn_root,
                Some(&cn_root)
            ),
            ProductProcessState::Running
        );
        let missing_executable = TraeProcessInfo {
            pid: 6,
            exe_path: None,
            command_line: Some(r#""E:\TRAE SOLO CN.exe" --user-data-dir="D:\other""#.to_string()),
        };
        let missing_command_line = TraeProcessInfo {
            pid: 7,
            exe_path: Some(r"E:\TRAE SOLO CN.exe".to_string()),
            command_line: None,
        };
        for process in [missing_executable, missing_command_line] {
            assert_eq!(
                match_product_process(&process, TraeProduct::WorkCn, &work_root, Some(&work_root)),
                ProcessMatch::Unknown
            );
            assert_eq!(
                product_process_state(
                    std::slice::from_ref(&process),
                    TraeProduct::WorkCn,
                    &work_root,
                    Some(&work_root)
                ),
                ProductProcessState::Unknown
            );
        }
    }

    #[test]
    fn process_query_failure_preserves_unknown_state() {
        let target = std::path::Path::new(r"C:\Users\u\AppData\Roaming\TRAE SOLO CN");
        assert_eq!(
            super::product_process_state_from_query(
                Err(TraeInstanceError::ProcessQueryFailed),
                TraeProduct::WorkCn,
                target,
                Some(target),
            ),
            ProductProcessState::Unknown
        );
    }

    #[test]
    fn invalid_appdata_keeps_bare_official_process_state_unknown() {
        let target = std::path::Path::new(r"C:\Users\u\AppData\Roaming\TRAE SOLO CN");
        let process = TraeProcessInfo {
            pid: 11,
            exe_path: Some(r"E:\TRAE SOLO CN.exe".to_string()),
            command_line: Some(r#""E:\TRAE SOLO CN.exe""#.to_string()),
        };

        // APPDATA 为空或相对时，裸启动进程无法归属到默认目录，必须保留未知。
        for appdata in [
            None,
            Some(PathBuf::new()),
            Some(PathBuf::from(r"relative\appdata")),
        ] {
            let default_data_dir =
                super::default_product_data_dir_from_appdata(TraeProduct::WorkCn, appdata);
            assert!(default_data_dir.is_none());
            assert_eq!(
                super::product_process_state(
                    std::slice::from_ref(&process),
                    TraeProduct::WorkCn,
                    target,
                    default_data_dir.as_deref(),
                ),
                ProductProcessState::Unknown
            );
        }
    }

    #[test]
    fn matched_process_ids_refuse_unknown_identity_even_with_a_match() {
        let appdata = std::path::Path::new(r"C:\Users\u\AppData\Roaming");
        let work_root = appdata.join(TraeProduct::WorkCn.data_root_name());
        let secondary = std::path::Path::new(r"D:\Trae Sync\environments\env-a");
        let work_main = TraeProcessInfo {
            pid: 1,
            exe_path: Some(r"E:\TRAE SOLO CN.exe".to_string()),
            command_line: Some(r#""E:\TRAE SOLO CN.exe""#.to_string()),
        };
        let work_child_without_directory = TraeProcessInfo {
            pid: 2,
            exe_path: Some(r"E:\TRAE SOLO CN.exe".to_string()),
            command_line: Some(r#""E:\TRAE SOLO CN.exe" --type=renderer"#.to_string()),
        };
        let other_product = TraeProcessInfo {
            pid: 3,
            exe_path: Some(r"E:\Trae CN.exe".to_string()),
            command_line: Some(r#""E:\Trae CN.exe""#.to_string()),
        };

        assert_eq!(
            super::matched_product_process_ids(
                &[work_main.clone(), other_product],
                TraeProduct::WorkCn,
                &work_root,
                Some(&work_root),
            ),
            Ok(vec![1])
        );
        assert_eq!(
            super::matched_product_process_ids(
                &[work_main, work_child_without_directory],
                TraeProduct::WorkCn,
                secondary,
                Some(&work_root),
            ),
            Err(ProductProcessState::Unknown)
        );
    }

    #[test]
    fn close_instance_only_targets_work_processes() {
        let work_dir = std::path::Path::new(r"D:\Trae Sync\master");
        let default_work_dir = std::path::Path::new(r"C:\Users\u\AppData\Roaming\TRAE SOLO CN");
        let work = TraeProcessInfo {
            pid: 21,
            exe_path: Some(r"E:\TRAE SOLO CN.exe".to_string()),
            command_line: Some(
                r#""E:\TRAE SOLO CN.exe" --user-data-dir="D:\Trae Sync\master""#.to_string(),
            ),
        };
        let cn = TraeProcessInfo {
            pid: 31,
            exe_path: Some(r"E:\Trae CN\Trae CN.exe".to_string()),
            command_line: Some(
                r#""E:\Trae CN\Trae CN.exe" --user-data-dir="D:\Trae CN\data""#.to_string(),
            ),
        };
        let mut snapshots =
            std::collections::VecDeque::from([Ok(vec![work, cn.clone()]), Ok(vec![cn])]);
        let mut taskkill_calls = Vec::new();
        let mut waits = Vec::new();

        let outcome = close_instance_with(
            TraeProduct::WorkCn,
            work_dir,
            Some(default_work_dir),
            || snapshots.pop_front().expect("测试快照应足够"),
            |pid, force| {
                taskkill_calls.push((pid, force));
                true
            },
            |duration| waits.push(duration),
        )
        .unwrap();

        assert_eq!(outcome, CloseOutcome::Closed);
        assert_eq!(taskkill_calls, vec![(21, false)]);
        assert_eq!(waits, vec![std::time::Duration::from_secs(3)]);
    }

    #[test]
    fn close_instance_only_targets_trae_cn_processes() {
        let cn_dir = std::path::Path::new(r"D:\Trae CN\data");
        let work = TraeProcessInfo {
            pid: 21,
            exe_path: Some(r"E:\TRAE SOLO CN.exe".to_string()),
            command_line: Some(
                r#""E:\TRAE SOLO CN.exe" --user-data-dir="D:\Trae Sync\master""#.to_string(),
            ),
        };
        let cn = TraeProcessInfo {
            pid: 31,
            exe_path: Some(r"E:\Trae CN\Trae CN.exe".to_string()),
            command_line: Some(
                r#""E:\Trae CN\Trae CN.exe" --user-data-dir="D:\Trae CN\data""#.to_string(),
            ),
        };
        let mut snapshots =
            std::collections::VecDeque::from([Ok(vec![work.clone(), cn]), Ok(vec![work])]);
        let mut taskkill_calls = Vec::new();
        let mut waits = Vec::new();

        // CN 为目标时只结束 CN 进程，Work 实例必须保留。
        let outcome = close_instance_with(
            TraeProduct::TraeCn,
            cn_dir,
            None,
            || snapshots.pop_front().expect("测试快照应足够"),
            |pid, force| {
                taskkill_calls.push((pid, force));
                true
            },
            |duration| waits.push(duration),
        )
        .unwrap();

        assert_eq!(outcome, CloseOutcome::Closed);
        assert_eq!(taskkill_calls, vec![(31, false)]);
        assert_eq!(waits, vec![std::time::Duration::from_secs(3)]);
    }

    #[test]
    fn close_instance_unknown_identity_has_no_side_effects() {
        let work_dir = std::path::Path::new(r"D:\Trae Sync\master");
        let default_work_dir = std::path::Path::new(r"C:\Users\u\AppData\Roaming\TRAE SOLO CN");
        let unknown = TraeProcessInfo {
            pid: 42,
            exe_path: Some(r"E:\TRAE SOLO CN.exe".to_string()),
            command_line: None,
        };
        let mut taskkill_calls = Vec::new();
        let mut waits = Vec::new();

        let error = close_instance_with(
            TraeProduct::WorkCn,
            work_dir,
            Some(default_work_dir),
            || Ok(vec![unknown.clone()]),
            |pid, force| {
                taskkill_calls.push((pid, force));
                true
            },
            |duration| waits.push(duration),
        )
        .unwrap_err();

        assert_eq!(error, TraeInstanceCloseError::ProcessStateUnknown);
        assert!(taskkill_calls.is_empty());
        assert!(waits.is_empty());
    }

    #[test]
    fn close_instance_query_failure_has_no_side_effects() {
        let work_dir = std::path::Path::new(r"D:\Trae Sync\master");
        let default_work_dir = std::path::Path::new(r"C:\Users\u\AppData\Roaming\TRAE SOLO CN");
        let mut taskkill_calls = Vec::new();
        let mut waits = Vec::new();

        let error = close_instance_with(
            TraeProduct::WorkCn,
            work_dir,
            Some(default_work_dir),
            || Err(TraeInstanceCloseError::ProcessStateUnknown),
            |pid, force| {
                taskkill_calls.push((pid, force));
                true
            },
            |duration| waits.push(duration),
        )
        .unwrap_err();

        assert_eq!(error, TraeInstanceCloseError::ProcessStateUnknown);
        assert!(taskkill_calls.is_empty());
        assert!(waits.is_empty());
    }

    #[test]
    fn close_instance_recheck_failure_does_not_force_kill() {
        let work_dir = std::path::Path::new(r"D:\Trae Sync\master");
        let default_work_dir = std::path::Path::new(r"C:\Users\u\AppData\Roaming\TRAE SOLO CN");
        let work = TraeProcessInfo {
            pid: 21,
            exe_path: Some(r"E:\TRAE SOLO CN.exe".to_string()),
            command_line: Some(
                r#""E:\TRAE SOLO CN.exe" --user-data-dir="D:\Trae Sync\master""#.to_string(),
            ),
        };
        let mut snapshots = std::collections::VecDeque::from([
            Ok(vec![work]),
            Err(TraeInstanceCloseError::ProcessStateUnknown),
        ]);
        let mut taskkill_calls = Vec::new();
        let mut waits = Vec::new();

        let error = close_instance_with(
            TraeProduct::WorkCn,
            work_dir,
            Some(default_work_dir),
            || snapshots.pop_front().expect("测试快照应足够"),
            |pid, force| {
                taskkill_calls.push((pid, force));
                true
            },
            |duration| waits.push(duration),
        )
        .unwrap_err();

        assert_eq!(error, TraeInstanceCloseError::ProcessStateUnknown);
        assert_eq!(taskkill_calls, vec![(21, false)]);
        assert_eq!(waits, vec![std::time::Duration::from_secs(3)]);
    }

    #[test]
    fn close_instance_force_closes_only_remaining_target_process() {
        let work_dir = std::path::Path::new(r"D:\Trae Sync\master");
        let default_work_dir = std::path::Path::new(r"C:\Users\u\AppData\Roaming\TRAE SOLO CN");
        let work = TraeProcessInfo {
            pid: 21,
            exe_path: Some(r"E:\TRAE SOLO CN.exe".to_string()),
            command_line: Some(
                r#""E:\TRAE SOLO CN.exe" --user-data-dir="D:\Trae Sync\master""#.to_string(),
            ),
        };
        let mut snapshots = std::collections::VecDeque::from([
            Ok(vec![work.clone()]),
            Ok(vec![work]),
            Ok(Vec::new()),
        ]);
        let mut taskkill_calls = Vec::new();
        let mut waits = Vec::new();

        let outcome = close_instance_with(
            TraeProduct::WorkCn,
            work_dir,
            Some(default_work_dir),
            || snapshots.pop_front().expect("测试快照应足够"),
            |pid, force| {
                taskkill_calls.push((pid, force));
                true
            },
            |duration| waits.push(duration),
        )
        .unwrap();

        assert_eq!(outcome, CloseOutcome::ForceClosed);
        assert_eq!(taskkill_calls, vec![(21, false), (21, true)]);
        assert_eq!(
            waits,
            vec![
                std::time::Duration::from_secs(3),
                std::time::Duration::from_secs(2)
            ]
        );
    }

    #[test]
    fn command_line_matching_ignores_quotes_and_case() {
        let dir = std::path::Path::new(r"D:\Work\Trae Sync\data\trae-instances\checkin-a");
        let default_dir = std::path::Path::new(r"C:\Users\u\AppData\Roaming\TRAE SOLO CN");
        let process = |command_line: &str| TraeProcessInfo {
            pid: 8,
            exe_path: Some(r"E:\TRAE SOLO CN.exe".to_string()),
            command_line: Some(command_line.to_string()),
        };
        let quoted = r#""E:\x\TRAE.exe" --user-data-dir="D:\Work\Trae Sync\data\trae-instances\checkin-a" --type=gpu"#;
        let differently_cased =
            r#""E:\x\TRAE.exe" --user-data-dir="d:\work\trae sync\data\trae-instances\CHECKIN-A""#;
        assert_eq!(
            match_product_process(
                &process(quoted),
                TraeProduct::WorkCn,
                dir,
                Some(default_dir)
            ),
            ProcessMatch::Match
        );
        assert_eq!(
            match_product_process(
                &process(differently_cased),
                TraeProduct::WorkCn,
                dir,
                Some(default_dir)
            ),
            ProcessMatch::Match
        );

        // 前缀目录不误判：checkin-ab 不是 checkin-a 的运行实例。
        let other =
            r#""E:\x\TRAE.exe" --user-data-dir=D:\Work\Trae Sync\data\trae-instances\checkin-ab"#;
        assert_eq!(
            match_product_process(&process(other), TraeProduct::WorkCn, dir, Some(default_dir)),
            ProcessMatch::NoMatch
        );
        // 无关命令行。
        assert_eq!(
            match_product_process(
                &process(r#""E:\x\TRAE.exe""#),
                TraeProduct::WorkCn,
                dir,
                Some(default_dir)
            ),
            ProcessMatch::NoMatch
        );
    }

    #[test]
    fn command_line_matching_requires_one_complete_actual_argument() {
        let dir = std::path::Path::new(r"D:\Work\Trae Sync\data\trae-instances\checkin-a");
        let default_dir = std::path::Path::new(r"C:\Users\u\AppData\Roaming\TRAE SOLO CN");
        let match_line = |command_line: &str| {
            let process = TraeProcessInfo {
                pid: 9,
                exe_path: Some(r"E:\TRAE SOLO CN.exe".to_string()),
                command_line: Some(command_line.to_string()),
            };
            match_product_process(&process, TraeProduct::WorkCn, dir, Some(default_dir))
        };
        assert_eq!(
            match_line(
                r#""E:\x\TRAE.exe" --user-data-dir "D:\Work\Trae Sync\data\trae-instances\checkin-a""#
            ),
            ProcessMatch::Match
        );
        assert_eq!(
            match_line(r#""E:\x\TRAE.exe" --user-data-dir="""#),
            ProcessMatch::Unknown
        );
        assert_eq!(
            match_line(r#""E:\x\TRAE.exe" --user-data-dir"#),
            ProcessMatch::Unknown
        );
        assert_eq!(
            match_line(
                r#""E:\x\TRAE.exe" --user-data-dir=D:\Work\Trae Sync\data\trae-instances\checkin-a --user-data-dir=D:\other"#
            ),
            ProcessMatch::Unknown
        );
        // 目录字符串仅出现在其他参数时，不得当成 user-data-dir。
        assert_eq!(
            match_line(
                r#""E:\x\TRAE.exe" --other=D:\Work\Trae Sync\data\trae-instances\checkin-a"#
            ),
            ProcessMatch::NoMatch
        );
        assert_eq!(
            match_line(
                r#""E:\x\TRAE.exe" --user-data-dir=D:\Work\Trae Sync\data\trae-instances\checkin-a\child"#
            ),
            ProcessMatch::NoMatch
        );
    }

    #[test]
    fn malformed_data_dir_does_not_fall_back_to_default_match() {
        let root = std::path::Path::new(r"C:\Users\u\AppData\Roaming\TRAE SOLO CN");
        let malformed = |command_line: &str| TraeProcessInfo {
            pid: 9,
            exe_path: Some(r"E:\TRAE SOLO CN.exe".to_string()),
            command_line: Some(command_line.to_string()),
        };
        for process in [
            malformed(r#""E:\TRAE SOLO CN.exe" --user-data-dir="#),
            malformed(r#""E:\TRAE SOLO CN.exe" --user-data-dir=""#),
            malformed(r#""E:\TRAE SOLO CN.exe" --user-data-dir=C:\one --user-data-dir=C:\two"#),
            // 未闭合引号表示目录参数解析不完整，不能退回默认目录匹配。
            malformed(
                r#""E:\TRAE SOLO CN.exe" --user-data-dir="C:\Users\u\AppData\Roaming\TRAE SOLO CN"#,
            ),
            TraeProcessInfo {
                pid: 10,
                exe_path: Some(r"E:\TRAE SOLO CN.exe".to_string()),
                command_line: Some("   ".to_string()),
            },
        ] {
            assert_eq!(
                match_product_process(&process, TraeProduct::WorkCn, root, Some(root)),
                ProcessMatch::Unknown
            );
        }
    }

    #[test]
    fn product_matching_covers_official_bare_launch() {
        let official = std::path::Path::new(r"C:\Users\u\AppData\Roaming\TRAE SOLO CN");
        let process = |command_line: &str| TraeProcessInfo {
            pid: 10,
            exe_path: Some(r"E:\TRAE SOLO CN.exe".to_string()),
            command_line: Some(command_line.to_string()),
        };
        // 官方快捷方式无参启动的主进程：默认 data_dir 即官方目录。
        let bare = r#""E:\软件\TRAE\TRAE SOLO CN\TRAE SOLO CN.exe""#;
        assert_eq!(
            match_product_process(
                &process(bare),
                TraeProduct::WorkCn,
                official,
                Some(official)
            ),
            ProcessMatch::Match
        );
        // 带参形态（App 启动或官方实例子进程）。
        let quoted = r#""E:\x\TRAE.exe" --type=gpu-process --user-data-dir="C:\Users\u\AppData\Roaming\TRAE SOLO CN""#;
        assert_eq!(
            match_product_process(
                &process(quoted),
                TraeProduct::WorkCn,
                official,
                Some(official)
            ),
            ProcessMatch::Match
        );

        // 账号实例（显式指向别的目录）不误判为主库。
        let account =
            r#""E:\x\TRAE.exe" --user-data-dir=D:\Work\Trae Sync\data\trae-instances\checkin-a"#;
        assert_eq!(
            match_product_process(
                &process(account),
                TraeProduct::WorkCn,
                official,
                Some(official)
            ),
            ProcessMatch::NoMatch
        );
        let incomplete = r#""E:\x\TRAE.exe" --user-data-dir="#;
        assert_eq!(
            match_product_process(
                &process(incomplete),
                TraeProduct::WorkCn,
                official,
                Some(official)
            ),
            ProcessMatch::Unknown
        );
        // 无目录参数的子进程无法证明属于其他实例，保留未知。
        let child = r#""E:\x\TRAE.exe" --type=utility --utility-sub-type=monitor"#;
        assert_eq!(
            match_product_process(
                &process(child),
                TraeProduct::WorkCn,
                official,
                Some(official)
            ),
            ProcessMatch::Unknown
        );
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
        assert!(instance_dir
            .join("User")
            .join("globalStorage")
            .join("storage.json")
            .is_file());
        assert!(instance_dir
            .join("User")
            .join("globalStorage")
            .join("state.vscdb")
            .is_file());
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

/// P15-11：进程查询缓存状态机（TTL 命中 / 过期 / 显式失效；注入 Instant 离线验证）。
#[cfg(test)]
mod process_cache_tests {
    use super::*;
    use std::time::Duration;

    fn sample(pid: u32) -> TraeProcessInfo {
        TraeProcessInfo {
            pid,
            exe_path: Some(format!("C:\\fake\\Trae {pid}.exe")),
            command_line: None,
        }
    }

    #[test]
    fn cache_returns_fresh_entry_within_ttl() {
        let mut state = ProcessCacheState::default();
        let t0 = Instant::now();
        state.store(t0, vec![sample(11)]);
        let fresh = state
            .get_fresh(t0 + Duration::from_secs(2))
            .expect("TTL 内应命中");
        assert_eq!(fresh.len(), 1);
        assert_eq!(fresh[0].pid, 11);
    }

    #[test]
    fn cache_expires_after_ttl() {
        let mut state = ProcessCacheState::default();
        let t0 = Instant::now();
        state.store(t0, vec![sample(11)]);
        assert!(state.get_fresh(t0 + ProcessCacheState::TTL).is_some());
        assert!(state
            .get_fresh(t0 + ProcessCacheState::TTL + Duration::from_millis(1))
            .is_none());
    }

    #[test]
    fn invalidate_clears_entry_immediately() {
        let mut state = ProcessCacheState::default();
        let t0 = Instant::now();
        state.store(t0, vec![sample(11)]);
        state.invalidate();
        assert!(state.get_fresh(t0).is_none());
        // 失效后重新 store 恢复命中。
        state.store(t0 + Duration::from_secs(1), vec![sample(12)]);
        let fresh = state.get_fresh(t0 + Duration::from_secs(2)).expect("重新缓存");
        assert_eq!(fresh[0].pid, 12);
    }

    #[test]
    fn empty_state_misses() {
        let state = ProcessCacheState::default();
        assert!(state.get_fresh(Instant::now()).is_none());
    }
}
