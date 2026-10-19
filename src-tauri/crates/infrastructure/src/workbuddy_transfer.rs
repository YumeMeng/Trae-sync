//! P18-B1：WorkBuddy 会话过户产品化（wb_transfer.py 三轮真机验证流程 → Rust）。
//!
//! 需求与方案唯一依据：`%TEMP%\handoff-workbuddy-session-transfer-20261017.md`
//! （三轮真实过户验证，非推测设计）。核心事实：
//! - WorkBuddy 会话 = 本地 SQLite 索引（`workbuddy.db` sessions 表按 user_id
//!   隔离）+ 本地 jsonl 正文（全局共享、不带归属）——过户只需 UPDATE
//!   sessions.user_id，正文零触碰；
//! - `transport='cloud'` 走 edge-sync 云同步、通道按 uid 隔离——**红线：禁止
//!   修改 cloud 会话归属**（SQL 必须带 `AND transport='local'`）；
//! - 软删除行（`deleted_at IS NOT NULL`）UI 不显示，过户时**一并转移**
//!   （不带 deleted_at 过滤），否则残留旧账号名下；
//! - 切号是热切换（daemon 不重启），写库前必须让客户端**完全退出**；
//!   WAL 模式强杀不损坏库。
//!
//! 进程安全复用 P14-2 边界：进程状态 Unknown 时拒绝一切写副作用。
//! 编排函数（[`execute_transfer`]）的进程观察/关闭/重启全部注入，
//! 离线单测同 `workbuddy_instance::close_workbuddy_with` 模式。
//!
//! 真机纪律：本模块提供 dry_run（只出计划不动库）；真实过户属破坏性
//! 批量操作，由组合根命令在 UI 单次确认后调用（不在单测中真实执行）。

use std::path::{Path, PathBuf};
use std::time::Duration;

// ===== 发现层：数据根与安装路径 =====

/// WorkBuddy 数据根目录名（数据根 ≠ Electron userData：userData 是
/// `~/.workbuddy/app`，数据根是 `~/.workbuddy`，勿混淆——10-17 文档 §9）。
pub const DATA_ROOT_DIR_NAME: &str = ".workbuddy";
/// 会话索引库（SQLite，WAL 模式，无加密——直读/直写安全）。
pub const DB_FILE_NAME: &str = "workbuddy.db";
/// 会话正文目录（`projects/<转义cwd>/<sessionId>.jsonl`，全局共享）。
pub const PROJECTS_DIR_NAME: &str = "projects";
/// 客户端日志目录（`logs/<日期>/*.log`，账号方向识别权威来源）。
pub const LOGS_DIR_NAME: &str = "logs";

/// 数据根（`~/.workbuddy`）。`WORKBUDDY_DATA_ROOT` 环境变量可覆盖
/// （测试/非标环境），缺省按 `%USERPROFILE%\.workbuddy`。
pub fn default_data_root() -> Option<PathBuf> {
    if let Ok(override_root) = std::env::var("WORKBUDDY_DATA_ROOT") {
        if !override_root.trim().is_empty() {
            return Some(PathBuf::from(override_root));
        }
    }
    std::env::var_os("USERPROFILE")
        .map(|home| PathBuf::from(home).join(DATA_ROOT_DIR_NAME))
}

/// 会话索引库绝对路径。
pub fn db_path(data_root: &Path) -> PathBuf {
    data_root.join(DB_FILE_NAME)
}

/// 会话正文根绝对路径。
pub fn projects_path(data_root: &Path) -> PathBuf {
    data_root.join(PROJECTS_DIR_NAME)
}

/// 客户端日志根绝对路径。
pub fn logs_path(data_root: &Path) -> PathBuf {
    data_root.join(LOGS_DIR_NAME)
}

/// 安装位置来源（探测优先级：环境覆盖 > 进程命令行 > 常见路径 > 注册表）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallSource {
    /// `WORKBUDDY_CLIENT_EXE` 环境覆盖（测试/非标安装）。
    EnvOverride,
    /// 运行中进程命令行首段（客户端运行中最准——即实际运行路径）。
    ProcessCommandLine,
    /// 常见安装路径候选（本机实测 + Electron 惯例）。
    CommonPath,
    /// 注册表 Uninstall 键（安装位置非固定盘的兜底）。
    Registry,
}

/// 安装位置探测结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallDiscovery {
    pub exe: PathBuf,
    pub source: InstallSource,
}

/// WorkBuddy 安装路径三级探测。
///
/// 注意区分：进程命令行里的 `--user-data-dir="~/.workbuddy/app"` 是 Electron
/// userData，**不是**安装路径也不是数据根；安装路径取命令行首段 exe。
pub fn discover_install() -> Option<InstallDiscovery> {
    // 0) 环境覆盖（沿用 workbuddy_commands::workbuddy_client_exe 的既有约定）。
    if let Ok(override_path) = std::env::var("WORKBUDDY_CLIENT_EXE") {
        let candidate = PathBuf::from(override_path.trim());
        if candidate.is_file() {
            return Some(InstallDiscovery { exe: candidate, source: InstallSource::EnvOverride });
        }
    }
    // 1) 进程命令行首段（客户端在跑时最准）。
    if let Ok(processes) = crate::workbuddy_instance::list_workbuddy_processes() {
        for process in processes {
            if let Some(line) = process.command_line.as_deref() {
                if let Some(exe) = exe_path_from_command_line(line) {
                    if exe.is_file() {
                        return Some(InstallDiscovery {
                            exe,
                            source: InstallSource::ProcessCommandLine,
                        });
                    }
                }
            }
        }
    }
    // 2) 常见路径候选（本机实测优先 + Electron 惯例目录）。
    for candidate in common_install_candidates() {
        if candidate.is_file() {
            return Some(InstallDiscovery { exe: candidate, source: InstallSource::CommonPath });
        }
    }
    // 3) 注册表 Uninstall 键兜底（WorkBuddy 安装位置非固定盘）。
    let exe = registry_install_exe()?;
    if exe.is_file() {
        return Some(InstallDiscovery { exe, source: InstallSource::Registry });
    }
    None
}

/// 常见安装路径候选（有文件存在性之外的前提不成立时由调用方过滤）。
fn common_install_candidates() -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    // 本机实测安装位置（P17-T2 探针；WorkBuddy 安装位置非固定盘，此为当前事实）。
    candidates.push(PathBuf::from(r"E:\软件\workbuddy\WorkBuddy.exe"));
    if let Ok(local) = std::env::var("LOCALAPPDATA") {
        candidates.push(
            Path::new(&local)
                .join("Programs")
                .join("workbuddy")
                .join("WorkBuddy.exe"),
        );
        candidates.push(
            Path::new(&local)
                .join("Programs")
                .join("WorkBuddy")
                .join("WorkBuddy.exe"),
        );
    }
    candidates
}

/// 从进程命令行提取 exe 路径（首段：带引号形态取首尾引号之间，裸形态取首个空白前）。
pub fn exe_path_from_command_line(line: &str) -> Option<PathBuf> {
    let trimmed = line.trim_start();
    if let Some(rest) = trimmed.strip_prefix('"') {
        let end = rest.find('"')?;
        let raw = &rest[..end];
        if raw.is_empty() { None } else { Some(PathBuf::from(raw)) }
    } else {
        let raw = trimmed.split_whitespace().next()?;
        if raw.is_empty() { None } else { Some(PathBuf::from(raw)) }
    }
}

/// 注册表 Uninstall 键兜底探测（HKCU/HKLM 逐子键查 DisplayIcon/InstallLocation
/// 中含 workbuddy 的条目）。仅在 windows 下可用；探测失败静默返回 None。
#[cfg(windows)]
fn registry_install_exe() -> Option<PathBuf> {
    use windows_sys::Win32::Foundation::{ERROR_NO_MORE_ITEMS, ERROR_SUCCESS};
    use windows_sys::Win32::System::Registry::{
        RegCloseKey, RegEnumKeyExW, RegOpenKeyExW, RegQueryValueExW, HKEY,
        HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ, REG_EXPAND_SZ, REG_SZ,
    };

    const UNINSTALL_SUBKEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Uninstall";
    const MAX_SUBKEYS: u32 = 512;

    fn to_wide(value: &str) -> Vec<u16> {
        value.encode_utf16().chain(Some(0)).collect()
    }

    /// 读 REG_SZ / REG_EXPAND_SZ 字符串值（键不存在或类型不符返回 None）。
    unsafe fn read_string(handle: HKEY, value_name: &[u16]) -> Option<String> {
        let mut buffer = [0u16; 1024];
        let mut size = (buffer.len() * 2) as u32;
        let mut kind = 0u32;
        let status = unsafe {
            RegQueryValueExW(
                handle,
                value_name.as_ptr(),
                std::ptr::null(),
                &mut kind,
                buffer.as_mut_ptr().cast::<u8>(),
                &mut size,
            )
        };
        if status != ERROR_SUCCESS || (kind != REG_SZ && kind != REG_EXPAND_SZ) {
            return None;
        }
        let len = buffer.iter().position(|&c| c == 0).unwrap_or(buffer.len());
        Some(String::from_utf16_lossy(&buffer[..len]))
    }

    /// 从 DisplayIcon 值提取 exe 路径（惯例形态 `"<path>,0"`——剥逗号后的图标序号）。
    fn exe_from_display_icon(value: &str) -> Option<PathBuf> {
        let raw = value.split(',').next()?.trim().trim_matches('"');
        if raw.is_empty() { None } else { Some(PathBuf::from(raw)) }
    }

    for root in [HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE] {
        let uninstall_wide = to_wide(UNINSTALL_SUBKEY);
        let mut uninstall_handle: HKEY = std::ptr::null_mut();
        let status = unsafe {
            RegOpenKeyExW(root, uninstall_wide.as_ptr(), 0, KEY_READ, &mut uninstall_handle)
        };
        if status != ERROR_SUCCESS {
            continue;
        }
        let found = (|| -> Option<PathBuf> {
            for index in 0..MAX_SUBKEYS {
                let mut name_buffer = [0u16; 256];
                let mut name_len = name_buffer.len() as u32;
                let status = unsafe {
                    RegEnumKeyExW(
                        uninstall_handle,
                        index,
                        name_buffer.as_mut_ptr(),
                        &mut name_len,
                        // lpReserved / lpClass / lpcchClass / lpftLastWriteTime 均不需要。
                        std::ptr::null_mut(),
                        std::ptr::null_mut(),
                        std::ptr::null_mut(),
                        std::ptr::null_mut(),
                    )
                };
                if status == ERROR_NO_MORE_ITEMS {
                    break;
                }
                if status != ERROR_SUCCESS {
                    continue;
                }
                let subkey = String::from_utf16_lossy(&name_buffer[..name_len as usize]);
                let subkey_wide = to_wide(&format!(r"{UNINSTALL_SUBKEY}\{subkey}"));
                let mut subkey_handle: HKEY = std::ptr::null_mut();
                let status = unsafe {
                    RegOpenKeyExW(root, subkey_wide.as_ptr(), 0, KEY_READ, &mut subkey_handle)
                };
                if status != ERROR_SUCCESS {
                    continue;
                }
                let found = unsafe {
                    read_string(subkey_handle, &to_wide("DisplayIcon"))
                        .filter(|value| value.to_ascii_lowercase().contains("workbuddy"))
                        .as_deref()
                        .and_then(exe_from_display_icon)
                }
                .or_else(|| unsafe {
                    read_string(subkey_handle, &to_wide("InstallLocation"))
                        .filter(|value| value.to_ascii_lowercase().contains("workbuddy"))
                        .map(|location| PathBuf::from(location.trim().trim_matches('"')).join("WorkBuddy.exe"))
                });
                unsafe { let _ = RegCloseKey(subkey_handle); }
                if found.is_some() {
                    return found;
                }
            }
            None
        })();
        unsafe { let _ = RegCloseKey(uninstall_handle); }
        if found.is_some() {
            return found;
        }
    }
    None
}

/// 非 windows 平台无注册表探测（fail-closed 返回 None，由上层候选链兜底）。
#[cfg(not(windows))]
fn registry_install_exe() -> Option<PathBuf> {
    None
}

// ===== 进程安全（P14-2 边界：Unknown 拒绝一切写副作用） =====

/// 客户端进程三态（与 ProcessObservationStatus 同纪律的 WorkBuddy 简化形态）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClientProcessState {
    NotRunning,
    Running,
    Unknown,
}

/// 真实进程观察（映像名枚举；枚举失败 = Unknown，绝不猜）。
pub fn observe_client_real() -> ClientProcessState {
    match crate::workbuddy_instance::list_workbuddy_processes() {
        Ok(list) if list.is_empty() => ClientProcessState::NotRunning,
        Ok(_) => ClientProcessState::Running,
        Err(_) => ClientProcessState::Unknown,
    }
}

/// 真实关闭编排（workbuddy_instance：优雅 → /F 强杀；WAL 模式强杀安全）。
/// 返回是否编排完成（退出确认由调用方关闭后再次 observe）。
pub fn close_client_real() -> bool {
    crate::workbuddy_instance::close_workbuddy().is_ok()
}

/// 真实重启（启动 exe；CREATE_NO_WINDOW 避免闪控制台）。
pub fn start_client_real(exe: &Path) -> bool {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    std::process::Command::new(exe)
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .is_ok()
}

/// 真实进程计数（重启后 ≥3 确认用）。
pub fn count_client_processes_real() -> Option<usize> {
    crate::workbuddy_instance::list_workbuddy_processes()
        .ok()
        .map(|list| list.len())
}

// ===== 错误与回执类型 =====

/// 过户域错误（组合根只暴露稳定错误码，不透传内部细节）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransferError {
    /// 索引库文件不存在。
    DbMissing,
    /// 索引库打开失败（损坏/被独占）。
    DbUnavailable,
    /// sessions 表结构不符（缺必需列——大版本升级 schema 漂移防御）。
    SchemaInvalid,
    /// 日志与回退都无法识别当前登录账号。
    DirectionUnresolved,
    /// 没有上一个账号（单账号在用），无需转移。
    NoPreviousAccount,
    /// 上一账号与当前账号相同（防御，正常路径不可达）。
    SameAccount,
    /// 客户端进程状态未知——拒绝一切写副作用（P14-2 边界）。
    ProcessStateUnknown,
    /// 关闭客户端失败（强杀后仍有进程存活）。
    CloseFailed,
    /// 过户 SQL 执行失败（事务已回滚）。
    SqlFailed,
    /// WAL checkpoint 失败（事务已提交，落盘时序问题）。
    CheckpointFailed,
    /// 正文抽验读取失败。
    VerifyFailed,
}

impl TransferError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::DbMissing => "wb_transfer_db_missing",
            Self::DbUnavailable => "wb_transfer_db_unavailable",
            Self::SchemaInvalid => "wb_transfer_schema_invalid",
            Self::DirectionUnresolved => "wb_transfer_direction_unresolved",
            Self::NoPreviousAccount => "wb_transfer_no_previous_account",
            Self::SameAccount => "wb_transfer_same_account",
            Self::ProcessStateUnknown => "wb_transfer_process_state_unknown",
            Self::CloseFailed => "wb_transfer_close_failed",
            Self::SqlFailed => "wb_transfer_sql_failed",
            Self::CheckpointFailed => "wb_transfer_checkpoint_failed",
            Self::VerifyFailed => "wb_transfer_verify_failed",
        }
    }
}

/// 账号方向识别来源。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DirectionSource {
    /// 日志 `userId changed` 事件（权威来源）。
    LogEvents,
    /// 日志缺失回退 sessions 表最近活跃（10-17 文档 §3.1 回退路径）。
    SessionsFallback,
}

/// 账号分布行（读取层统计；不含任何会话内容）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountDistribution {
    pub user_id: String,
    pub session_count: u64,
    pub last_activity_unix: Option<i64>,
}

/// 账号方向（当前账号 + 上一个账号）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransferDirection {
    pub current_uid: String,
    pub previous_uid: Option<String>,
    pub source: DirectionSource,
}

/// 过户计划（dry_run 输出；零副作用）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransferPlan {
    pub current_uid: String,
    pub previous_uid: String,
    /// 方向识别来源（build_transfer_plan 预填日志口径；execute_transfer
    /// 编排时覆盖为实际识别结果）。
    pub source: DirectionSource,
    /// 上一账号名下待过户的 local 行数（含软删除行）。
    pub pending_local_rows: u64,
    /// 当前账号现有会话数（全部 transport）。
    pub current_account_rows: u64,
    /// 上一账号名下 cloud 会话数（红线：不碰；>0 时 UI 明示）。
    pub cloud_residual_rows: u64,
    /// 其余更早账号的 local 行数（本票按「上一账号→当前账号」蓝本，不扩面）。
    pub other_account_rows: u64,
}

/// 过户执行回执（10-17 文档 §3.4：更新行数 / cloud 残留数 / 缺失数）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransferReceipt {
    pub updated_rows: u64,
    pub cloud_residual_rows: u64,
    pub verified_sessions: u64,
    pub missing_body_files: u64,
}

/// 完整执行回执（过户 + 重启确认；uid 随回执上浮供组合根出昵称标签，
/// UI 纪律由组合根保证——uid 不进前端 DTO 主视野）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecutionReceipt {
    pub transfer: TransferReceipt,
    pub current_uid: String,
    pub previous_uid: String,
    pub source: DirectionSource,
    pub restarted: bool,
    pub process_count_after_restart: Option<usize>,
}

/// 编排结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransferOutcome {
    /// dry_run：只出计划（零副作用）。
    Plan(Box<TransferPlan>),
    /// 真实执行完成。
    Executed(Box<ExecutionReceipt>),
    /// 无需转移（无上一账号 / 待过户为 0——跳过关应用）。
    NothingToTransfer { current_uid: String, current_sessions: u64 },
}

// ===== 读取层（只读连接；WAL 只读安全） =====

/// 打开只读连接（库不存在显式报错，区别于打开失败）。
fn open_read_only(db: &Path) -> Result<rusqlite::Connection, TransferError> {
    if !db.is_file() {
        return Err(TransferError::DbMissing);
    }
    rusqlite::Connection::open_with_flags(
        db,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .map_err(|_| TransferError::DbUnavailable)
}

/// sessions 表结构校验（大版本升级 schema 漂移防御——10-17 文档 §9）。
/// 必需列：本模块 SQL 依赖的最小集合。
pub fn validate_schema(db: &Path) -> Result<(), TransferError> {
    const REQUIRED_COLUMNS: &[&str] = &[
        "id", "cwd", "user_id", "transport", "deleted_at", "created_at", "last_activity_at",
    ];
    let conn = open_read_only(db)?;
    let mut stmt = conn
        .prepare("PRAGMA table_info(sessions)")
        .map_err(|_| TransferError::SchemaInvalid)?;
    let mut columns: Vec<String> = Vec::new();
    let rows = stmt
        .query_map([], |row| row.get::<_, String>(1))
        .map_err(|_| TransferError::SchemaInvalid)?;
    for row in rows {
        columns.push(row.map_err(|_| TransferError::SchemaInvalid)?);
    }
    let missing = REQUIRED_COLUMNS
        .iter()
        .any(|required| !columns.iter().any(|column| column == required));
    if missing {
        return Err(TransferError::SchemaInvalid);
    }
    Ok(())
}

/// 账号分布/会话归属统计（按最近活跃降序；空 user_id 与非 uuid 形态
/// （旧数据迁移兼容口子）一并返回供统计，但不参与方向回退判定）。
pub fn read_account_distribution(
    db: &Path,
) -> Result<Vec<AccountDistribution>, TransferError> {
    let conn = open_read_only(db)?;
    let mut stmt = conn
        .prepare(
            "SELECT user_id, COUNT(*), MAX(COALESCE(last_activity_at, created_at)) \
             FROM sessions GROUP BY user_id ORDER BY 3 DESC",
        )
        .map_err(|_| TransferError::SqlFailed)?;
    let rows = stmt
        .query_map([], |row| {
            Ok(AccountDistribution {
                user_id: row.get::<_, String>(0)?,
                session_count: row.get::<_, i64>(1)?.max(0) as u64,
                last_activity_unix: row.get(2).ok(),
            })
        })
        .map_err(|_| TransferError::SqlFailed)?;
    let mut result = Vec::new();
    for row in rows {
        result.push(row.map_err(|_| TransferError::SqlFailed)?);
    }
    Ok(result)
}

// ===== 账号方向识别（日志权威 + sessions 回退） =====

/// 日志中账号切换事件的标记子串（10-17 文档 §3.1 权威锚点）。
pub const UID_CHANGED_MARKER: &str = "userId changed:";

/// 单条账号切换事件（登出 uid / 登录 uid，均可能缺省——首次登录无登出）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UidEvent {
    /// 日志时间戳原文（`YYYY/MM/DD HH:MM:SS.mmm`——字典序即时间序）。
    pub timestamp: String,
    pub out_uid: Option<String>,
    pub in_uid: Option<String>,
}

/// uid 合法性：36 位 8-4-4-4-12 十六进制分组（与 wb_transfer.py
/// `[0-9a-f-]{36}` fullmatch 同口径收紧为分组校验）。
pub fn is_uuid_like(value: &str) -> bool {
    fn is_hex(b: u8) -> bool {
        matches!(b, b'0'..=b'9' | b'a'..=b'f' | b'A'..=b'F')
    }
    let bytes = value.as_bytes();
    if bytes.len() != 36 {
        return false;
    }
    for (index, &byte) in bytes.iter().enumerate() {
        let expected_dash = matches!(index, 8 | 13 | 18 | 23);
        if expected_dash {
            if byte != b'-' {
                return false;
            }
        } else if !is_hex(byte) {
            return false;
        }
    }
    true
}

/// 单行解析（无 regex 依赖：标记定位 + 箭头分割 + 首词截取；时间戳取
/// 行首方括号内原文，缺失该形态的行跳过）。
fn parse_uid_event_line(line: &str) -> Option<UidEvent> {
    let marker_pos = line.find(UID_CHANGED_MARKER)?;
    let open = line.find('[')?;
    let close = line.find(']')?;
    if open >= close {
        return None;
    }
    let timestamp = line[open + 1..close].trim().to_string();
    let remainder = line[marker_pos + UID_CHANGED_MARKER.len()..].trim();
    let (out_raw, in_raw) = match remainder.split_once("->") {
        Some((left, right)) => (left.trim(), right.trim()),
        None => (remainder, ""),
    };
    let out_uid = normalize_uid_token(out_raw);
    let in_uid = normalize_uid_token(in_raw);
    if out_uid.is_none() && in_uid.is_none() {
        return None;
    }
    Some(UidEvent { timestamp, out_uid, in_uid })
}

/// 取候选 token（首词）并做 uuid 形态校验；空串/异形一律 None。
fn normalize_uid_token(raw: &str) -> Option<String> {
    let token = raw.split_whitespace().next().unwrap_or("");
    if is_uuid_like(token) {
        Some(token.to_ascii_lowercase())
    } else {
        None
    }
}

/// 扫描 `logs/<日期>/*.log` 全部切换事件，按时间戳排序。
pub fn parse_uid_events(log_root: &Path) -> Vec<UidEvent> {
    let mut events = Vec::new();
    let Ok(day_dirs) = std::fs::read_dir(log_root) else {
        return events;
    };
    for day in day_dirs.flatten() {
        let day_path = day.path();
        if !day_path.is_dir() {
            continue;
        }
        let Ok(files) = std::fs::read_dir(&day_path) else {
            continue;
        };
        for file in files.flatten() {
            let path = file.path();
            if path.extension().and_then(|ext| ext.to_str()) != Some("log") {
                continue;
            }
            let Ok(content) = std::fs::read_to_string(&path) else {
                continue;
            };
            for line in content.lines() {
                if let Some(event) = parse_uid_event_line(line) {
                    events.push(event);
                }
            }
        }
    }
    events.sort_by(|left, right| left.timestamp.cmp(&right.timestamp));
    events
}

/// 识别账号方向（10-17 文档 §3.1 蓝本逐条对齐）：
/// - 当前账号 = 日志中最后一次成功登录（`-> <uid>`）的 uid；
/// - 上一个账号 = 最近一次「登出某 uid 且该 uid ≠ 当时当前账号」的 uid；
///   （保留蓝本语义：按事件序以"当时的当前账号"判重）
/// - 日志无事件时回退：sessions 表按 MAX(COALESCE(last_activity_at,
///   created_at)) 最新者为当前，次新异于当前者为上一（仅接受 uuid 形态，
///   排除 `''` 旧数据兼容口子）。
pub fn resolve_account_direction(
    db: &Path,
    log_root: &Path,
) -> Result<TransferDirection, TransferError> {
    let events = parse_uid_events(log_root);
    let mut current: Option<String> = None;
    let mut previous: Option<String> = None;
    for event in &events {
        if let Some(in_uid) = &event.in_uid {
            current = Some(in_uid.clone());
        }
        if let Some(out_uid) = &event.out_uid {
            if Some(out_uid) != current.as_ref() {
                previous = Some(out_uid.clone());
            }
        }
    }
    if let Some(current_uid) = current {
        let previous_uid = match previous {
            Some(candidate) if candidate != current_uid => Some(candidate),
            _ => fallback_previous_uid(db, &current_uid)?,
        };
        return Ok(TransferDirection {
            current_uid,
            previous_uid,
            source: DirectionSource::LogEvents,
        });
    }
    // 回退路径：sessions 表最近活跃排序（空 user_id / 非 uuid 形态排除）。
    let distribution = read_account_distribution(db)?;
    let candidates: Vec<String> = distribution
        .into_iter()
        .filter(|entry| is_uuid_like(&entry.user_id))
        .map(|entry| entry.user_id)
        .collect();
    let Some(current_uid) = candidates.first().cloned() else {
        return Err(TransferError::DirectionUnresolved);
    };
    let previous_uid = candidates.into_iter().nth(1);
    Ok(TransferDirection {
        current_uid,
        previous_uid,
        source: DirectionSource::SessionsFallback,
    })
}

/// 回退上一账号：sessions 表中异于当前账号、最近活跃的 uuid 形态 uid。
fn fallback_previous_uid(
    db: &Path,
    current: &str,
) -> Result<Option<String>, TransferError> {
    let distribution = read_account_distribution(db)?;
    Ok(distribution
        .into_iter()
        .filter(|entry| entry.user_id != current && is_uuid_like(&entry.user_id))
        .map(|entry| entry.user_id)
        .next())
}

// ===== 过户计划（dry_run，零副作用） =====

/// 构建过户计划（只读；不触碰任何数据）。同账号防御在plan层兜底。
pub fn build_transfer_plan(
    db: &Path,
    current: &str,
    previous: &str,
) -> Result<TransferPlan, TransferError> {
    if current == previous {
        return Err(TransferError::SameAccount);
    }
    let conn = open_read_only(db)?;
    let count = |sql: &str| -> Result<u64, TransferError> {
        conn.query_row(sql, [], |row| {
            Ok(row.get::<_, i64>(0)?.max(0) as u64)
        })
        .map_err(|_| TransferError::SqlFailed)
    };
    Ok(TransferPlan {
        current_uid: current.to_string(),
        previous_uid: previous.to_string(),
        pending_local_rows: count(&format!(
            "SELECT COUNT(*) FROM sessions \
             WHERE user_id = '{}' AND transport = 'local'",
            previous
        ))?,
        current_account_rows: count(&format!(
            "SELECT COUNT(*) FROM sessions WHERE user_id = '{}'",
            current
        ))?,
        cloud_residual_rows: count(&format!(
            "SELECT COUNT(*) FROM sessions \
             WHERE user_id = '{}' AND transport = 'cloud'",
            previous
        ))?,
        other_account_rows: count(&format!(
            "SELECT COUNT(*) FROM sessions \
             WHERE user_id NOT IN ('{}', '{}') AND transport = 'local'",
            current, previous
        ))?,
        // 来源预填日志口径；execute_transfer 编排时以实际识别结果覆盖。
        source: DirectionSource::LogEvents,
    })
}

// ===== 正文抽验（cwd 转义规则） =====

/// cwd → projects 目录转义（实测规则，以真机 projects 目录名为准：
/// `D:/work/Trae-sync → d-work-Trae-sync`、`C:/Users/.../WorkBuddy/... →
/// c-Users-...-WorkBuddy-...`）：非字母数字转 `-`、连续 `-` 压缩、**仅首字符
/// （盘符）小写，其余大小写保留**。注意：wb_transfer.py 的整串 `.lower()`
/// 是被 NTFS 大小写不敏感掩盖的偏差（2026-10-19 对真机目录名 ls 取证修正）；
/// 字母数字按 Unicode 口径（与 wb_transfer.py `isalnum()` 一致——中文 cwd
/// 段保留）。
pub fn escape_cwd_dir(cwd: &str) -> String {
    let mut escaped = String::with_capacity(cwd.len());
    for (index, character) in cwd.chars().enumerate() {
        if character.is_alphanumeric() {
            // 仅首字符（盘符）小写；其余保持原大小写（真机目录名取证）。
            if index == 0 {
                escaped.extend(character.to_lowercase());
            } else {
                escaped.push(character);
            }
        } else {
            escaped.push('-');
        }
    }
    let mut collapsed = String::with_capacity(escaped.len());
    let mut previous_dash = false;
    for character in escaped.chars() {
        if character == '-' {
            if !previous_dash {
                collapsed.push(character);
            }
            previous_dash = true;
        } else {
            collapsed.push(character);
            previous_dash = false;
        }
    }
    collapsed
}

/// 抽验当前账号全部会话的正文 jsonl 存在性（10-17 文档 §3.4）。
/// 返回 (会话总数, 缺失正文数)。
pub fn verify_body_files(
    db: &Path,
    projects_root: &Path,
    uid: &str,
) -> Result<(u64, u64), TransferError> {
    let conn = open_read_only(db)?;
    let mut stmt = conn
        .prepare("SELECT id, cwd FROM sessions WHERE user_id = ?1")
        .map_err(|_| TransferError::VerifyFailed)?;
    let rows = stmt
        .query_map([uid], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(|_| TransferError::VerifyFailed)?;
    let mut total: u64 = 0;
    let mut missing: u64 = 0;
    for row in rows {
        let (session_id, cwd) = row.map_err(|_| TransferError::VerifyFailed)?;
        total += 1;
        let body = projects_root
            .join(escape_cwd_dir(&cwd))
            .join(format!("{session_id}.jsonl"));
        if !body.is_file() {
            missing += 1;
        }
    }
    Ok((total, missing))
}

// ===== 过户事务（写路径；调用方必须已确认客户端完全退出） =====

/// 事务内过户（10-17 文档 §3.3 蓝本）：
/// BEGIN IMMEDIATE → UPDATE sessions SET user_id=当前 WHERE user_id=上一
/// AND transport='local'（含 deleted_at IS NOT NULL 行——软删除一并过户）
/// → cloud 残留防御 COUNT 检查（不改，仅回执报告）→ COMMIT
/// → PRAGMA wal_checkpoint(TRUNCATE) → 正文抽验。
pub fn transfer_sessions(
    db: &Path,
    projects_root: &Path,
    current: &str,
    previous: &str,
) -> Result<TransferReceipt, TransferError> {
    if current == previous {
        return Err(TransferError::SameAccount);
    }
    validate_schema(db)?;
    let mut conn = rusqlite::Connection::open(db)
        .map_err(|_| TransferError::DbUnavailable)?;
    conn.busy_timeout(Duration::from_secs(10))
        .map_err(|_| TransferError::DbUnavailable)?;
    let transaction = conn
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(|_| TransferError::SqlFailed)?;
    // 红线：UPDATE 只带 transport='local'；cloud 会话一行不碰。
    // 不带 deleted_at 过滤：软删除行一并过户（否则残留旧账号名下）。
    let updated_rows = transaction
        .execute(
            "UPDATE sessions SET user_id = ?1 WHERE user_id = ?2 AND transport = 'local'",
            rusqlite::params![current, previous],
        )
        .map_err(|_| TransferError::SqlFailed)? as u64;
    // cloud 残留防御：上一账号名下的 cloud 行应恒为 0（>0 说明该账号有
    // 云同步会话——属异常上报项，本次仍不触碰）。
    let cloud_residual_rows = transaction
        .query_row(
            "SELECT COUNT(*) FROM sessions \
             WHERE user_id = ?1 AND transport = 'cloud'",
            rusqlite::params![previous],
            |row| Ok(row.get::<_, i64>(0)?.max(0) as u64),
        )
        .map_err(|_| TransferError::SqlFailed)?;
    transaction.commit().map_err(|_| TransferError::SqlFailed)?;
    // wal_checkpoint(TRUNCATE) 落盘（防数据滞留 -wal）。已提交后失败不回滚，
    // 属落盘时序问题——按蓝本 best-effort，不吞掉整个回执。
    let checkpoint = conn.query_row(
        "PRAGMA wal_checkpoint(TRUNCATE)",
        [],
        |row| Ok(row.get::<_, i64>(0).unwrap_or(-1)),
    );
    if checkpoint.is_err() {
        return Err(TransferError::CheckpointFailed);
    }
    // 正文抽验（重开只读连接，验证可见性）。
    let (verified_sessions, missing_body_files) =
        verify_body_files(db, projects_root, current)?;
    Ok(TransferReceipt {
        updated_rows,
        cloud_residual_rows,
        verified_sessions,
        missing_body_files,
    })
}

// ===== 编排（进程依赖注入；离线单测同 workbuddy_instance 模式） =====

/// 编排的客户端进程依赖（观察/关闭/重启/计数全部注入）。
pub struct TransferClientDeps<'a> {
    /// 进程观察（Unknown 必须如实报告——P14-2 边界）。
    pub observe: &'a dyn Fn() -> ClientProcessState,
    /// 关闭编排（优雅→强杀；返回编排是否完成，退出确认由 observe 复核）。
    pub close: &'a dyn Fn() -> bool,
    /// 重启动作（None = 跳过重启，回执 restarted=false）。
    pub start: Option<&'a dyn Fn() -> bool>,
    /// 重启后进程计数（与 start 成对；≥3 为客户端起全的正常形态）。
    pub count_processes: Option<&'a dyn Fn() -> Option<usize>>,
    /// 重启后等待进程树铺开的静置时长（真实模式 2-3s；单测传 ZERO）。
    pub restart_settle: Duration,
}

/// 完整过户编排：方向识别 → 计划 →（dry_run 返回）→ 完全退出前置校验
/// → 事务过户 → 正文抽验 → 重启 + 进程数确认。
pub fn execute_transfer(
    data_root: &Path,
    deps: &TransferClientDeps<'_>,
    dry_run: bool,
) -> Result<TransferOutcome, TransferError> {
    let db = db_path(data_root);
    let projects = projects_path(data_root);
    let logs = logs_path(data_root);
    // schema 校验前置（fail-fast：schema 漂移时连计划都不出）。
    validate_schema(&db)?;
    let direction = resolve_account_direction(&db, &logs)?;
    let Some(previous_uid) = direction.previous_uid.clone() else {
        let current_sessions = read_account_distribution(&db)?
            .into_iter()
            .find(|entry| entry.user_id == direction.current_uid)
            .map(|entry| entry.session_count)
            .unwrap_or(0);
        return Ok(TransferOutcome::NothingToTransfer {
            current_uid: direction.current_uid,
            current_sessions,
        });
    };
    let mut plan = build_transfer_plan(&db, &direction.current_uid, &previous_uid)?;
    // 计划与执行回执携带实际识别来源（build_transfer_plan 预填值的覆盖点）。
    plan.source = direction.source;
    if dry_run {
        return Ok(TransferOutcome::Plan(Box::new(plan)));
    }
    // 待过户为 0：跳过关应用（与 wb_transfer.py 「无需关应用」分支一致）。
    if plan.pending_local_rows == 0 {
        return Ok(TransferOutcome::NothingToTransfer {
            current_uid: plan.current_uid,
            current_sessions: plan.current_account_rows,
        });
    }
    // 完全退出前置校验（P14-2 边界：Unknown 拒绝一切写副作用；Running 先关，
    // 关后复核仍非 NotRunning 一律拒绝——含 Unknown，不猜）。
    match (deps.observe)() {
        ClientProcessState::NotRunning => {}
        ClientProcessState::Running => {
            if !(deps.close)() {
                return Err(TransferError::CloseFailed);
            }
            if (deps.observe)() != ClientProcessState::NotRunning {
                return Err(TransferError::CloseFailed);
            }
        }
        ClientProcessState::Unknown => return Err(TransferError::ProcessStateUnknown),
    }
    let receipt = transfer_sessions(&db, &projects, &plan.current_uid, &previous_uid)?;
    // 重启 + 进程数确认（best-effort：重启失败/计数不可得如实回执，不失败）。
    let mut restarted = false;
    let mut process_count_after_restart = None;
    if let (Some(start), Some(count_processes)) = (deps.start, deps.count_processes) {
        if start() {
            restarted = true;
            if !deps.restart_settle.is_zero() {
                std::thread::sleep(deps.restart_settle);
            }
            process_count_after_restart = count_processes();
        }
    }
    Ok(TransferOutcome::Executed(Box::new(ExecutionReceipt {
        transfer: receipt,
        current_uid: plan.current_uid,
        previous_uid,
        source: direction.source,
        restarted,
        process_count_after_restart,
    })))
}

// ===== 单测（fixture：tempdir 内现造最小 sessions 库，schema 对齐真机） =====

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    const UID_A: &str = "11e1dfbf-d66d-42d1-9d1b-d6017334f0b1";
    const UID_B: &str = "89b16d6b-0549-4e1e-8ced-918b5c8152ae";
    const UID_C: &str = "0a0b0c0d-0000-4000-8000-0000000000cc";

    /// 现造最小 fixture 库（列集合为真机 sessions 表相关子集——
    /// 模块 SQL 只依赖这些列，与 .scratch/workbuddy-survey/db-copy 实测对齐）。
    fn fixture_db(dir: &Path) -> PathBuf {
        let db = db_path(dir);
        let conn = Connection::open(&db).unwrap();
        conn.execute_batch(
            "CREATE TABLE sessions (
                id TEXT PRIMARY KEY,
                cwd TEXT,
                user_id TEXT,
                title TEXT,
                created_at INTEGER,
                updated_at INTEGER,
                last_activity_at INTEGER,
                deleted_at INTEGER,
                transport TEXT
            );",
        )
        .unwrap();
        db
    }

    fn insert_session(
        conn: &Connection,
        id: &str,
        user_id: &str,
        transport: &str,
        deleted_at: Option<i64>,
        cwd: &str,
        last_activity: i64,
    ) {
        conn.execute(
            "INSERT INTO sessions (id, cwd, user_id, title, created_at, updated_at, \
             last_activity_at, deleted_at, transport) \
             VALUES (?1, ?2, ?3, ?4, 1000, 1000, ?5, ?6, ?7)",
            rusqlite::params![id, cwd, user_id, format!("title-{id}"), last_activity, deleted_at, transport],
        )
        .unwrap();
    }

    fn all_rows(db: &Path) -> Vec<(String, String, String, Option<i64>)> {
        let conn = Connection::open_with_flags(db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .unwrap();
        let mut stmt = conn
            .prepare("SELECT id, user_id, transport, deleted_at FROM sessions ORDER BY id")
            .unwrap();
        stmt.query_map([], |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
            ))
        })
        .unwrap()
        .map(Result::unwrap)
        .collect()
    }

    fn write_log(log_root: &Path, day: &str, file: &str, lines: &[&str]) {
        let dir = log_root.join(day);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(file), lines.join("\n")).unwrap();
    }

    // ---- cwd 转义规则 ----

    #[test]
    fn escape_cwd_dir_matches_verified_rule() {
        // 真机取证（2026-10-19 ls projects 目录）：大小写保留。
        assert_eq!(escape_cwd_dir("D:/work/Trae-sync"), "d-work-Trae-sync");
        assert_eq!(
            escape_cwd_dir("C:/Users/12732/WorkBuddy/2026-10-14-13-38-49"),
            "c-Users-12732-WorkBuddy-2026-10-14-13-38-49"
        );
        // 非字母数字转 -、连续 - 压缩。
        assert_eq!(escape_cwd_dir("a:::b"), "a-b");
        // Unicode 字母数字保留（与 wb_transfer.py isalnum() 口径一致）。
        assert_eq!(escape_cwd_dir(r"E:\软件\workbuddy"), "e-软件-workbuddy");
    }

    // ---- uid 形态与日志解析 ----

    #[test]
    fn is_uuid_like_validates_grouped_hex() {
        assert!(is_uuid_like(UID_A));
        assert!(!is_uuid_like("not-a-uuid"));
        assert!(!is_uuid_like(""));
        assert!(!is_uuid_like("zzzzzzzz-zzzz-zzzz-zzzz-zzzzzzzzzzzz"));
        // 分组位错置（35 位/37 位/错位横线）一律拒绝。
        assert!(!is_uuid_like(&UID_A[..35]));
        let mut misplaced = UID_A.as_bytes().to_vec();
        misplaced[7] = b'-';
        assert!(!is_uuid_like(std::str::from_utf8(&misplaced).unwrap()));
    }

    #[test]
    fn parse_uid_events_sorts_and_skips_noise() {
        let temp = tempfile::tempdir().unwrap();
        let logs = logs_path(temp.path());
        write_log(&logs, "2026-10-17", "main.log", &[
            &format!("[2026/10/17 09:00:00.100] [INFO] AuthenticationManager] userId changed: {UID_A} -> {UID_B}"),
            &format!("[2026/10/17 08:00:00.100] [INFO] AuthenticationManager] userId changed: {UID_B} -> {UID_A}"),
            "噪声行没有标记",
            "[2026/10/17 10:00:00.100] [INFO] AuthenticationManager] userId changed:  -> not-a-uuid",
        ]);
        write_log(&logs, "2026-10-16", "main.log", &[
            &format!("[2026/10/16 20:00:00.000] [INFO] AuthenticationManager] userId changed: -> {UID_B}"),
        ]);
        let events = parse_uid_events(&logs);
        assert_eq!(events.len(), 3);
        // 字典序 = 时间序（跨日期目录排序）。
        assert!(events[0].timestamp.starts_with("2026/10/16"));
        assert!(events[1].timestamp.starts_with("2026/10/17 08"));
        assert!(events[2].timestamp.starts_with("2026/10/17 09"));
        // 首次登录：out 缺省；两侧均非法 uid 的事件整体丢弃。
        assert_eq!(events[0].out_uid, None);
        assert_eq!(events[0].in_uid.as_deref(), Some(UID_B));
        assert_eq!(events[2].out_uid.as_deref(), Some(UID_A));
        assert_eq!(events[2].in_uid.as_deref(), Some(UID_B));
    }

    // ---- 账号方向识别（日志路径 + 回退路径） ----

    #[test]
    fn direction_from_log_events_two_account_round_trip() {
        let temp = tempfile::tempdir().unwrap();
        let db = fixture_db(temp.path());
        let logs = logs_path(temp.path());
        write_log(&logs, "2026-10-17", "main.log", &[
            &format!("[2026/10/17 09:00:00.000] [INFO] AuthenticationManager] userId changed: {UID_A} -> {UID_B}"),
            &format!("[2026/10/17 10:00:00.000] [INFO] AuthenticationManager] userId changed: {UID_B} -> {UID_A}"),
        ]);
        let direction = resolve_account_direction(&db, &logs).unwrap();
        assert_eq!(direction.current_uid, UID_A);
        assert_eq!(direction.previous_uid.as_deref(), Some(UID_B));
        assert_eq!(direction.source, DirectionSource::LogEvents);
    }

    #[test]
    fn direction_falls_back_to_sessions_when_logs_missing() {
        let temp = tempfile::tempdir().unwrap();
        let db = fixture_db(temp.path());
        {
            let conn = Connection::open(&db).unwrap();
            insert_session(&conn, "s-old", UID_A, "local", None, "D:/w", 100);
            insert_session(&conn, "s-new", UID_B, "local", None, "D:/w", 900);
            insert_session(&conn, "s-legacy", "", "local", None, "D:/w", 950);
        }
        let direction =
            resolve_account_direction(&db, &logs_path(temp.path())).unwrap();
        // 回退只接受 uuid 形态：'' 旧数据口子与最近活跃排序均被正确处理。
        assert_eq!(direction.source, DirectionSource::SessionsFallback);
        assert_eq!(direction.current_uid, UID_B);
        assert_eq!(direction.previous_uid.as_deref(), Some(UID_A));
    }

    #[test]
    fn direction_unresolved_when_no_usable_signal() {
        let temp = tempfile::tempdir().unwrap();
        let db = fixture_db(temp.path());
        {
            let conn = Connection::open(&db).unwrap();
            insert_session(&conn, "s-legacy", "", "local", None, "D:/w", 100);
        }
        assert_eq!(
            resolve_account_direction(&db, &logs_path(temp.path())).unwrap_err(),
            TransferError::DirectionUnresolved
        );
    }

    // ---- schema 校验 ----

    #[test]
    fn schema_validation_rejects_drifted_tables() {
        let temp = tempfile::tempdir().unwrap();
        let db = db_path(temp.path());
        let conn = Connection::open(&db).unwrap();
        // 缺 transport 列（模拟大版本升级 schema 漂移）。
        conn.execute_batch(
            "CREATE TABLE sessions (id TEXT, cwd TEXT, user_id TEXT, deleted_at INTEGER, \
             created_at INTEGER, last_activity_at INTEGER);",
        )
        .unwrap();
        assert_eq!(validate_schema(&db).unwrap_err(), TransferError::SchemaInvalid);
    }

    #[test]
    fn missing_db_file_is_explicit_error() {
        let temp = tempfile::tempdir().unwrap();
        assert_eq!(
            validate_schema(&db_path(temp.path())).unwrap_err(),
            TransferError::DbMissing
        );
    }

    // ---- 过户 SQL 正确性（含 deleted_at 行 / cloud 红线） ----

    #[test]
    fn transfer_moves_local_rows_including_deleted_and_never_touches_cloud() {
        let temp = tempfile::tempdir().unwrap();
        let db = fixture_db(temp.path());
        let projects = projects_path(temp.path());
        {
            let conn = Connection::open(&db).unwrap();
            // 上一账号 A：local 3（1 条软删除）+ cloud 2（红线不碰）。
            insert_session(&conn, "a-local-1", UID_A, "local", None, "D:/w", 10);
            insert_session(&conn, "a-local-2", UID_A, "local", None, "D:/w", 11);
            insert_session(&conn, "a-del", UID_A, "local", Some(1), "D:/w", 12);
            insert_session(&conn, "a-cloud-1", UID_A, "cloud", None, "D:/w", 13);
            insert_session(&conn, "a-cloud-2", UID_A, "cloud", Some(1), "D:/w", 14);
            // 当前账号 B：local 1。
            insert_session(&conn, "b-local-1", UID_B, "local", None, "D:/w", 20);
            // 更早账号 C：local 1（本票范围外，不动）。
            insert_session(&conn, "c-local-1", UID_C, "local", None, "D:/w", 5);
        }
        let receipt = transfer_sessions(&db, &projects, UID_B, UID_A).unwrap();
        assert_eq!(receipt.updated_rows, 3, "软删除行一并过户");
        assert_eq!(receipt.cloud_residual_rows, 2, "cloud 残留如实报告");
        let rows = all_rows(&db);
        let owner = |id: &str| -> String {
            rows.iter()
                .find(|(row_id, ..)| row_id == id)
                .map(|(_, user, ..)| user.clone())
                .unwrap()
        };
        assert_eq!(owner("a-local-1"), UID_B);
        assert_eq!(owner("a-local-2"), UID_B);
        assert_eq!(owner("a-del"), UID_B, "软删除行同样过户");
        assert_eq!(owner("b-local-1"), UID_B);
        assert_eq!(owner("a-cloud-1"), UID_A, "红线：cloud 行不碰");
        assert_eq!(owner("a-cloud-2"), UID_A, "红线：cloud 行不碰");
        assert_eq!(owner("c-local-1"), UID_C, "更早账号不扩面");
    }

    #[test]
    fn plan_is_read_only_with_zero_side_effects() {
        let temp = tempfile::tempdir().unwrap();
        let db = fixture_db(temp.path());
        {
            let conn = Connection::open(&db).unwrap();
            insert_session(&conn, "a-local-1", UID_A, "local", None, "D:/w", 10);
            insert_session(&conn, "a-cloud-1", UID_A, "cloud", None, "D:/w", 13);
            insert_session(&conn, "b-local-1", UID_B, "local", None, "D:/w", 20);
        }
        let before = all_rows(&db);
        let plan = build_transfer_plan(&db, UID_B, UID_A).unwrap();
        assert_eq!(plan.pending_local_rows, 1);
        assert_eq!(plan.current_account_rows, 1);
        assert_eq!(plan.cloud_residual_rows, 1);
        assert_eq!(plan.other_account_rows, 0);
        assert_eq!(all_rows(&db), before, "计划构建零副作用");
    }

    #[test]
    fn plan_rejects_same_account() {
        let temp = tempfile::tempdir().unwrap();
        let db = fixture_db(temp.path());
        assert_eq!(
            build_transfer_plan(&db, UID_A, UID_A).unwrap_err(),
            TransferError::SameAccount
        );
    }

    // ---- 正文抽验 ----

    #[test]
    fn verify_body_files_counts_missing_with_cwd_escape() {
        let temp = tempfile::tempdir().unwrap();
        let db = fixture_db(temp.path());
        let projects = projects_path(temp.path());
        {
            let conn = Connection::open(&db).unwrap();
            insert_session(&conn, "sid-exists", UID_B, "local", None, "D:/work/Trae-sync", 10);
            insert_session(&conn, "sid-missing", UID_B, "local", None, "D:/work/Trae-sync", 11);
            insert_session(&conn, "sid-other", UID_A, "local", None, "D:/work", 12);
        }
        // 正文按转义规则落位：projects/d-work-Trae-sync/sid-exists.jsonl。
        let dir = projects.join("d-work-Trae-sync");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("sid-exists.jsonl"), b"{}").unwrap();
        let (total, missing) = verify_body_files(&db, &projects, UID_B).unwrap();
        assert_eq!(total, 2);
        assert_eq!(missing, 1);
    }

    // ---- 编排（进程依赖注入） ----

    #[test]
    fn execute_transfer_unknown_process_refuses_write() {
        let temp = tempfile::tempdir().unwrap();
        let db = fixture_db(temp.path());
        {
            let conn = Connection::open(&db).unwrap();
            insert_session(&conn, "a-local-1", UID_A, "local", None, "D:/w", 10);
            insert_session(&conn, "b-local-1", UID_B, "local", None, "D:/w", 20);
        }
        let before = all_rows(&db);
        let observe = || ClientProcessState::Unknown;
        let close = || true;
        let deps = TransferClientDeps {
            observe: &observe,
            close: &close,
            start: None,
            count_processes: None,
            restart_settle: Duration::ZERO,
        };
        // Unknown：真实执行拒绝一切写副作用。
        assert_eq!(
            execute_transfer(temp.path(), &deps, false).unwrap_err(),
            TransferError::ProcessStateUnknown
        );
        assert_eq!(all_rows(&db), before, "Unknown 时零写副作用");
        // 同状态 dry_run 正常出计划（只读）。
        match execute_transfer(temp.path(), &deps, true).unwrap() {
            TransferOutcome::Plan(plan) => {
                assert_eq!(plan.pending_local_rows, 1);
                assert_eq!(plan.current_uid, UID_B, "日志缺失回退最近活跃");
            }
            other => panic!("expected plan, got {other:?}"),
        }
    }

    #[test]
    fn execute_transfer_full_happy_path_with_restart() {
        let temp = tempfile::tempdir().unwrap();
        let db = fixture_db(temp.path());
        {
            let conn = Connection::open(&db).unwrap();
            insert_session(&conn, "a-local-1", UID_A, "local", None, "D:/w", 10);
            insert_session(&conn, "a-local-2", UID_A, "local", Some(1), "D:/w", 11);
            insert_session(&conn, "a-cloud-1", UID_A, "cloud", None, "D:/w", 12);
            insert_session(&conn, "b-local-1", UID_B, "local", None, "D:/w", 20);
        }
        let observe = || ClientProcessState::NotRunning;
        let close = || true;
        let started = std::cell::Cell::new(false);
        let start = || {
            started.set(true);
            true
        };
        let count = || Some(4);
        let deps = TransferClientDeps {
            observe: &observe,
            close: &close,
            start: Some(&start),
            count_processes: Some(&count),
            restart_settle: Duration::ZERO,
        };
        match execute_transfer(temp.path(), &deps, false).unwrap() {
            TransferOutcome::Executed(receipt) => {
                assert_eq!(receipt.transfer.updated_rows, 2);
                assert_eq!(receipt.transfer.cloud_residual_rows, 1);
                assert!(receipt.restarted);
                assert_eq!(receipt.process_count_after_restart, Some(4));
            }
            other => panic!("expected executed, got {other:?}"),
        }
        assert!(started.get());
        assert_eq!(
            all_rows(&db)
                .into_iter()
                .filter(|(_, user, transport, _)| user == UID_A && transport == "local")
                .count(),
            0,
            "过户后 A 名下 local 行清零"
        );
    }

    #[test]
    fn execute_transfer_running_client_is_closed_then_rechecked() {
        let temp = tempfile::tempdir().unwrap();
        let db = fixture_db(temp.path());
        {
            let conn = Connection::open(&db).unwrap();
            insert_session(&conn, "a-local-1", UID_A, "local", None, "D:/w", 10);
            insert_session(&conn, "b-local-1", UID_B, "local", None, "D:/w", 20);
        }
        // 观察序列：首次 Running → 关闭后 NotRunning（RefCell 保证闭包是 Fn）。
        let sequence = std::cell::RefCell::new(vec![
            ClientProcessState::Running,
            ClientProcessState::NotRunning,
        ]);
        let observe = || {
            let mut seq = sequence.borrow_mut();
            if seq.len() > 1 {
                seq.remove(0)
            } else {
                seq[0]
            }
        };
        let close_calls = std::cell::Cell::new(0u32);
        let close = || {
            close_calls.set(close_calls.get() + 1);
            true
        };
        let deps = TransferClientDeps {
            observe: &observe,
            close: &close,
            start: None,
            count_processes: None,
            restart_settle: Duration::ZERO,
        };
        match execute_transfer(temp.path(), &deps, false).unwrap() {
            TransferOutcome::Executed(receipt) => {
                assert_eq!(receipt.transfer.updated_rows, 1);
                assert!(!receipt.restarted, "未注入重启动作时如实报告");
            }
            other => panic!("expected executed, got {other:?}"),
        }
        assert_eq!(close_calls.get(), 1, "Running 先关后写");
    }

    #[test]
    fn execute_transfer_close_failure_refuses_write() {
        let temp = tempfile::tempdir().unwrap();
        let db = fixture_db(temp.path());
        {
            let conn = Connection::open(&db).unwrap();
            insert_session(&conn, "a-local-1", UID_A, "local", None, "D:/w", 10);
            insert_session(&conn, "b-local-1", UID_B, "local", None, "D:/w", 20);
        }
        let before = all_rows(&db);
        // 观察序列：Running →（强杀后）仍 Running → 拒绝写。
        let sequence = std::cell::RefCell::new(vec![
            ClientProcessState::Running,
            ClientProcessState::Running,
        ]);
        let observe = || {
            let mut seq = sequence.borrow_mut();
            if seq.len() > 1 {
                seq.remove(0)
            } else {
                seq[0]
            }
        };
        let close = || true;
        let deps = TransferClientDeps {
            observe: &observe,
            close: &close,
            start: None,
            count_processes: None,
            restart_settle: Duration::ZERO,
        };
        assert_eq!(
            execute_transfer(temp.path(), &deps, false).unwrap_err(),
            TransferError::CloseFailed
        );
        assert_eq!(all_rows(&db), before, "关闭失败拒绝写");
    }

    #[test]
    fn execute_transfer_zero_pending_skips_close() {
        let temp = tempfile::tempdir().unwrap();
        let db = fixture_db(temp.path());
        {
            let conn = Connection::open(&db).unwrap();
            insert_session(&conn, "b-local-1", UID_B, "local", None, "D:/w", 20);
            insert_session(&conn, "a-cloud-1", UID_A, "cloud", None, "D:/w", 12);
        }
        let observe = || ClientProcessState::NotRunning;
        let close_called = std::cell::Cell::new(false);
        let close = || {
            close_called.set(true);
            true
        };
        let deps = TransferClientDeps {
            observe: &observe,
            close: &close,
            start: None,
            count_processes: None,
            restart_settle: Duration::ZERO,
        };
        match execute_transfer(temp.path(), &deps, false).unwrap() {
            TransferOutcome::NothingToTransfer { current_uid, current_sessions } => {
                assert_eq!(current_uid, UID_B);
                assert_eq!(current_sessions, 1);
            }
            other => panic!("expected nothing-to-transfer, got {other:?}"),
        }
        assert!(!close_called.get(), "待过户为 0 不关应用（wb_transfer.py 分支）");
    }

    #[test]
    fn execute_transfer_without_previous_account_is_noop() {
        let temp = tempfile::tempdir().unwrap();
        let db = fixture_db(temp.path());
        {
            let conn = Connection::open(&db).unwrap();
            insert_session(&conn, "b-local-1", UID_B, "local", None, "D:/w", 20);
        }
        let observe = || ClientProcessState::NotRunning;
        let close = || true;
        let deps = TransferClientDeps {
            observe: &observe,
            close: &close,
            start: None,
            count_processes: None,
            restart_settle: Duration::ZERO,
        };
        match execute_transfer(temp.path(), &deps, false).unwrap() {
            TransferOutcome::NothingToTransfer { current_uid, current_sessions } => {
                assert_eq!(current_uid, UID_B);
                assert_eq!(current_sessions, 1);
            }
            other => panic!("expected nothing-to-transfer, got {other:?}"),
        }
    }

    // ---- 安装路径探测（纯函数部分） ----

    #[test]
    fn exe_path_from_command_line_handles_quoted_and_bare_forms() {
        assert_eq!(
            exe_path_from_command_line(
                r#""E:\软件\workbuddy\WorkBuddy.exe" --user-data-dir="C:\Users\me\.workbuddy\app""#
            )
            .unwrap(),
            PathBuf::from(r"E:\软件\workbuddy\WorkBuddy.exe")
        );
        assert_eq!(
            exe_path_from_command_line(r#"C:\WorkBuddy\WorkBuddy.exe --flag"#).unwrap(),
            PathBuf::from(r"C:\WorkBuddy\WorkBuddy.exe")
        );
        // 含空格路径在 Windows 进程命令行惯例下必带引号；裸形态按首个空白
        // 截断（与 workbuddy_instance::user_data_dir 同口径）。
        assert_eq!(
            exe_path_from_command_line(r#""C:\Program Files\WorkBuddy\WorkBuddy.exe" --flag"#)
                .unwrap(),
            PathBuf::from(r"C:\Program Files\WorkBuddy\WorkBuddy.exe")
        );
        assert_eq!(exe_path_from_command_line(""), None);
        // user-data-dir 不是安装路径——首段提取天然不会误取它。
        assert_ne!(
            exe_path_from_command_line(
                r#""E:\软件\workbuddy\WorkBuddy.exe" --user-data-dir="C:\Users\me\.workbuddy\app""#
            )
            .unwrap(),
            PathBuf::from(r"C:\Users\me\.workbuddy\app")
        );
    }
}
