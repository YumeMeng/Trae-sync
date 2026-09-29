//! OAuth 登录命令链（M4 begin/complete 两段式 + P6-3 临时档案清理 + P7-4 取消）。
//!
//! P12-R3 自 lib.rs 纯移动：登录/取消/完成命令与编排、wire DTO、浏览器拉起与
//! 临时档案机制、内嵌测试。命令体、错误码与 wire 形态零变化；
//! 与 infrastructure 的 checkin_login 模块边界保持现状（本模块只做命令壳与编排）。

use std::path::{Path, PathBuf};

use serde::Serialize;
use sha2::{Digest, Sha256};
use traesync_infrastructure::{
    begin_login_for, complete_login, trae_http_client, AccountRegistry, CheckinCredentialStore,
    LoginProduct, LoginSession, ProductAccountStateStore, CALLBACK_TIMEOUT_SECONDS,
};

use crate::{
    checkin_material_root, ensure_login_real_mode, run_blocking, trae_cn_material_root, AppState,
};
use crate::error_mapping::login_command_error;

/// 登录开始结果；login_url 同时返回给前端展示（浏览器唤起失败时用户可手动复制）。
/// pub(crate)：tauri 命令宏展开的包装函数签名需要暴露返回类型。
#[derive(Debug, Clone, Serialize)]
pub(crate) struct CheckinLoginBeginWireDto {
    login_url: String,
}

/// 登录完成回执（非敏感白名单）。
/// pub(crate)：tauri 命令宏展开的包装函数签名需要暴露返回类型。
#[derive(Debug, Clone, Serialize)]
pub(crate) struct CheckinLoginReceiptWireDto {
    profile_id: String,
    account_id: String,
    screen_name: String,
    avatar_url: String,
    /// 首次登录可持续性验收结果（ADR-0027）：passed / failed。
    /// failed 表示凭据已入库、账号已注册，但同设备换发验收未通过；
    /// 前端如实展示，不自动重试、不发起第二次授权。
    acceptance: &'static str,
    /// 验收失败的稳定错误码；通过时省略。
    #[serde(skip_serializing_if = "Option::is_none")]
    acceptance_error_code: Option<&'static str>,
}

/// Trae CN 登录完成回执；只返回产品账号页需要的非敏感展示字段。
/// pub(crate)：tauri 命令宏展开的包装函数签名需要暴露返回类型。
#[derive(Debug, Clone, Serialize)]
pub(crate) struct TraeCnLoginReceiptWireDto {
    profile_id: String,
    screen_name: String,
    avatar_url: String,
    /// 首次登录可持续性验收结果（ADR-0027）：与 Work 回执同一套语义。
    acceptance: &'static str,
    /// 验收失败的稳定错误码；通过时省略。
    #[serde(skip_serializing_if = "Option::is_none")]
    acceptance_error_code: Option<&'static str>,
}

/// 生成新登录会话的 profile_id（时间 + 进程指纹哈希；同一账号重复登录
/// 在入库时复用已有档案的 profile_id，这里只需保证新账号间不冲突）。
fn generate_login_profile_id() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    let mut hasher = Sha256::new();
    hasher.update(nanos.to_le_bytes());
    hasher.update(std::process::id().to_le_bytes());
    format!("checkin-{}", &hex::encode(hasher.finalize())[..12])
}

/// OAuth 临时浏览器档案保留时长（秒）：目录名即创建时间戳，超龄即废弃。
/// 必须大于登录回调等待上限（CALLBACK_TIMEOUT_SECONDS = 300），
/// 保证进行中的登录不会被超龄清理误删；超龄档案自动重置，
/// 用户需重新发起登录（重新授权）。
const OAUTH_PROFILE_MAX_AGE_SECONDS: u64 = 600;

/// OAuth 临时浏览器档案根目录（时间戳子目录的父目录）。
#[cfg(windows)]
fn oauth_browser_profile_root() -> std::path::PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("Trae Sync")
        .join("data")
        .join("oauth-browser-profiles")
}

/// 隔离浏览器 profile 目录：每次登录使用独立时间戳子目录，
/// 授权页 cookie 与本机默认浏览器环境完全隔离（添加新账号无需退出已有账号）。
#[cfg(windows)]
fn oauth_browser_profile_dir() -> std::path::PathBuf {
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0);
    oauth_browser_profile_root().join(timestamp.to_string())
}

/// 清理超龄的 OAuth 临时浏览器档案（P6-3 核心逻辑，供包装与测试复用）：
/// 目录名即创建时间戳，超过保留时长即删除；非时间戳命名的条目不动
/// （不猜来历）。删除失败静默（浏览器占用），由调用方的下个周期重试。
fn cleanup_expired_oauth_profiles_in_root(root: &Path, now_secs: u64) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return; // 根目录不存在 = 从未登录过，无事可做。
    };
    for entry in entries.flatten() {
        let created_at = entry
            .file_name()
            .to_str()
            .and_then(|name| name.parse::<u64>().ok());
        if let Some(created) = created_at {
            if now_secs.saturating_sub(created) >= OAUTH_PROFILE_MAX_AGE_SECONDS {
                let _ = std::fs::remove_dir_all(entry.path());
            }
        }
    }
}

/// 清理超龄的 OAuth 临时浏览器档案（P6-3 包装：定位真实根目录与当前时间）。
/// 浏览器进程仍占用时 Windows 删除失败，静默等待下个周期重试。
#[cfg(windows)]
pub(crate) fn cleanup_expired_oauth_profiles() {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0);
    cleanup_expired_oauth_profiles_in_root(&oauth_browser_profile_root(), now);
}

/// 定位可用的 Chromium 系浏览器（Chrome 优先，Edge 兜底）；找不到返回 None。
#[cfg(windows)]
fn locate_chromium_browser() -> Option<std::path::PathBuf> {
    // LOCALAPPDATA 下的用户级 Chrome 安装路径需动态拼接。
    let user_level_chrome = std::env::var_os("LOCALAPPDATA").map(|base| {
        std::path::PathBuf::from(base)
            .join(r"Google\Chrome\Application\chrome.exe")
            .to_string_lossy()
            .into_owned()
    });
    let mut candidates: Vec<String> = [
        r"C:\Program Files\Google\Chrome\Application\chrome.exe",
        r"C:\Program Files (x86)\Google\Chrome\Application\chrome.exe",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    if let Some(path) = user_level_chrome {
        candidates.push(path);
    }
    candidates.extend(
        [
            r"C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe",
            r"C:\Program Files\Microsoft\Edge\Application\msedge.exe",
        ]
        .iter()
        .map(|s| s.to_string()),
    );
    candidates
        .iter()
        .map(std::path::PathBuf::from)
        .find(|path| path.is_file())
}

/// 打开 OAuth 登录页（默认隔离模式）：
/// - 隔离模式（默认）：独立 user-data-dir 的 Chrome/Edge 实例，与本机登录态完全
///   隔离，适合添加系统浏览器未登录的其他账号（无需退出已有账号）。
/// - 本机模式：直接用系统浏览器默认 profile 打开，复用已登录会话，
///   授权页自动识别当前账号（一键确认即可）。
/// 浏览器不可用时回退系统默认浏览器（rundll32 FileProtocolHandler）。
///
/// 隔离模式启动成功时返回当次临时档案目录（Some），供登录结束后清理；
/// 同时返回浏览器子进程句柄（仅隔离模式：该进程即浏览器主进程，退出 =
/// 用户关闭了登录窗口）。本机模式与回退路径无隔离档案，返回 (None, None)。
#[cfg(windows)]
pub(crate) fn open_login_url_in_browser(
    url: &str,
    use_system_browser: bool,
) -> (Option<std::path::PathBuf>, Option<std::process::Child>) {
    if let Some(browser) = locate_chromium_browser() {
        if use_system_browser {
            // 不带 --user-data-dir：使用系统默认 profile 的登录态。
            // 注意：Chrome 已在运行时新进程会委托给既有实例后立即退出，
            // 进程退出不代表用户放弃登录，因此本模式不做进程退出检测。
            let launched = std::process::Command::new(&browser).arg(url).spawn();
            if launched.is_ok() {
                return (None, None);
            }
        } else {
            let profile_dir = oauth_browser_profile_dir();
            // 目录创建失败不阻塞登录：浏览器会按需自建或落到默认行为。
            let _ = std::fs::create_dir_all(&profile_dir);
            let launched = std::process::Command::new(&browser)
                .arg(format!("--user-data-dir={}", profile_dir.display()))
                .arg(url)
                .spawn();
            if let Ok(child) = launched {
                return (Some(profile_dir), Some(child));
            }
        }
    }
    let _ = std::process::Command::new("rundll32")
        .args(["url.dll,FileProtocolHandler", url])
        .spawn();
    (None, None)
}

/// 开始 OAuth 登录（内部逻辑，供命令与测试复用）：
/// 生成虚拟设备与 PKCE，返回登录 URL 与待保存会话。
fn begin_checkin_login_inner(state: &AppState) -> Result<(String, LoginSession), String> {
    ensure_login_real_mode(state)?;
    // 存储根可用性预检：登录入库需要凭据存储位置。
    checkin_material_root(state)?;
    let profile_id = generate_login_profile_id();
    let handoff = begin_login_for(&profile_id, LoginProduct::WorkCn)
        .map_err(|_| "login_begin_failed".to_string())?;
    Ok((handoff.login_url, handoff.session))
}

/// 开始 Trae CN 独立 OAuth 登录；浏览器会话可复用，但认证参数和落盘账号池
/// 与 Work CN 完全不同。
fn begin_trae_cn_login_inner(state: &AppState) -> Result<(String, LoginSession), String> {
    ensure_login_real_mode(state)?;
    trae_cn_material_root(state)?;
    let profile_id = generate_login_profile_id();
    let handoff = begin_login_for(&profile_id, LoginProduct::TraeCn)
        .map_err(|_| "trae_cn_login_begin_failed".to_string())?;
    Ok((handoff.login_url, handoff.session))
}

/// 开始 OAuth 登录：生成虚拟设备与 PKCE，按选择打开浏览器并返回登录 URL。
/// `use_system_browser`：true 用本机浏览器默认 profile（复用已登录会话）；
/// 缺省/False 用隔离实例（默认，与本机登录态互不影响）。
/// 回调监听 socket 随会话存活在后端；同一时间只保留最后一个会话。
#[tauri::command]
pub(crate) fn begin_checkin_login(
    use_system_browser: Option<bool>,
    state: tauri::State<AppState>,
) -> Result<CheckinLoginBeginWireDto, String> {
    let (login_url, session) = begin_checkin_login_inner(&state)?;
    // 新登录会话开始：清除上一会话可能遗留的取消标记（P7-4）。
    state
        .login_cancel
        .store(false, std::sync::atomic::Ordering::Relaxed);
    *state.pending_login.lock().unwrap() = Some(session);
    // 隔离模式启动成功才记录当次临时档案（本机/回退模式无隔离档案）；
    // 重复 begin 覆盖旧记录，被覆盖目录由周期清理按超龄回收。
    #[cfg(windows)]
    {
        let (profile, browser) =
            open_login_url_in_browser(&login_url, use_system_browser.unwrap_or(false));
        *state.active_oauth_profile.lock().unwrap() = profile;
        *state.active_login_browser.lock().unwrap() = browser;
    }
    #[cfg(not(windows))]
    {
        // 非 Windows 平台不由后端拉起浏览器（前端用返回的 URL 自行打开）。
        *state.active_login_browser.lock().unwrap() = None;
    }
    Ok(CheckinLoginBeginWireDto { login_url })
}

/// 开始 Trae CN OAuth 登录。默认使用本机浏览器 profile，方便复用网页端已有的
/// TRAE 登录会话；这只复用网页授权，不复制 Work 的本地 token/blob。
#[tauri::command]
pub(crate) fn begin_trae_cn_login(
    use_system_browser: Option<bool>,
    state: tauri::State<AppState>,
) -> Result<CheckinLoginBeginWireDto, String> {
    let (login_url, session) = begin_trae_cn_login_inner(&state)?;
    state
        .login_cancel
        .store(false, std::sync::atomic::Ordering::Relaxed);
    *state.pending_login.lock().unwrap() = Some(session);
    #[cfg(windows)]
    {
        let (profile, browser) =
            open_login_url_in_browser(&login_url, use_system_browser.unwrap_or(true));
        *state.active_oauth_profile.lock().unwrap() = profile;
        *state.active_login_browser.lock().unwrap() = browser;
    }
    #[cfg(not(windows))]
    {
        let _ = use_system_browser;
        *state.active_login_browser.lock().unwrap() = None;
    }
    Ok(CheckinLoginBeginWireDto { login_url })
}

/// 取出进行中的登录会话（一次性消费）；无会话时立即失败。
#[cfg(test)]
fn take_pending_login(state: &AppState) -> Result<LoginSession, String> {
    take_pending_login_for(state, LoginProduct::WorkCn)
}

/// 只允许与命令产品一致的会话完成，防止 Work/CN 两个完成命令交叉消费。
fn take_pending_login_for(
    state: &AppState,
    product: LoginProduct,
) -> Result<LoginSession, String> {
    let mut pending = state.pending_login.lock().unwrap();
    let Some(session) = pending.as_ref() else {
        return Err("login_not_started".to_string());
    };
    if session.product != product {
        return Err("login_product_mismatch".to_string());
    }
    pending
        .take()
        .ok_or("login_not_started".to_string())
}

/// 等待浏览器回调并完成登录（阻塞直到回调、失败、被取消或超时）。
/// 会话一次性消费；任何失败都不产生入库副作用。
/// 登录流程终结（无论成败）后清理当次隔离浏览器档案。
async fn complete_product_login_inner(
    state: &AppState,
    product: LoginProduct,
    material_root: PathBuf,
) -> Result<CheckinLoginReceiptWireDto, String> {
    // 先取走会话（一次性）：没有进行中的登录立即失败，不占用阻塞线程。
    let session = take_pending_login_for(state, product)?;
    // 会话已消费即登录流程终结：取出当次隔离档案目录，结束后删除。
    let finished_profile = state.active_oauth_profile.lock().unwrap().take();
    // P7-4 中止判定的共享状态：取消标记 + 隔离浏览器子进程（克隆 Arc 进
    // 阻塞线程；子进程退出 = 用户关闭了登录窗口，等待即时收尾）。
    let cancel_flag = state.login_cancel.clone();
    let browser_child = state.active_login_browser.clone();
    let result = run_blocking("login_join_failed", move || {
        let store = CheckinCredentialStore::new(&material_root);
        let registry = AccountRegistry::new(&material_root);
        let client = trae_http_client();
        let should_abort = || {
            if cancel_flag.load(std::sync::atomic::Ordering::Relaxed) {
                return true;
            }
            // 锁中毒不视为中止（fail-open：宁可多等，不误杀进行中的登录）。
            if let Ok(mut guard) = browser_child.lock() {
                if let Some(child) = guard.as_mut() {
                    if matches!(child.try_wait(), Ok(Some(_))) {
                        return true;
                    }
                }
            }
            false
        };
        let receipt = complete_login(
            session,
            &store,
            &registry,
            &client,
            std::time::Duration::from_secs(CALLBACK_TIMEOUT_SECONDS),
            &should_abort,
        )
        .map_err(|error| login_command_error(&error))?;
        Ok(CheckinLoginReceiptWireDto {
            profile_id: receipt.profile_id,
            account_id: receipt.account_id,
            screen_name: receipt.screen_name,
            avatar_url: receipt.avatar_url,
            // 验收结果如实透出：失败时登录仍成立（凭据与账号保留），前端按
            // failed + 稳定错误码提示；不在此层自动重试或二次授权。
            acceptance: receipt.acceptance.status(),
            acceptance_error_code: receipt.acceptance.error_code,
        })
    })
    .await;
    // 登录流程终结：释放浏览器子进程句柄。drop 不结束进程（正常路径浏览器
    // 由回调页 script 自行关闭；取消路径的结束进程由 cancel 命令负责）。
    let _ = state.active_login_browser.lock().unwrap().take();
    // 删除放阻塞线程池（浏览器 profile 含大量小文件，不宜占 async 线程）；
    // 浏览器仍占用时删除失败静默，由周期清理在目录超龄后兜底回收。
    if let Some(profile_dir) = finished_profile {
        tauri::async_runtime::spawn_blocking(move || {
            let _ = std::fs::remove_dir_all(profile_dir);
        });
    }
    result
}

#[tauri::command]
pub(crate) async fn complete_checkin_login(
    state: tauri::State<'_, AppState>,
) -> Result<CheckinLoginReceiptWireDto, String> {
    let material_root = checkin_material_root(&state)?;
    complete_product_login_inner(&state, LoginProduct::WorkCn, material_root).await
}

/// 完成 Trae CN OAuth 回调并写入 Trae CN 独立账号池。
#[tauri::command]
pub(crate) async fn complete_trae_cn_login(
    state: tauri::State<'_, AppState>,
) -> Result<TraeCnLoginReceiptWireDto, String> {
    let material_root = trae_cn_material_root(&state)?;
    let receipt = complete_product_login_inner(&state, LoginProduct::TraeCn, material_root.clone())
        .await?;
    // 登录成功后把新账号设为当前 CN 账号；该指针仍属于 Trae Sync 自己的
    // CN 账号池，不会改写官方 Trae CN 原生存储。
    ProductAccountStateStore::new(&material_root)
        .save(&receipt.profile_id)
        .map_err(|_| "trae_cn_account_state_failed".to_string())?;
    Ok(TraeCnLoginReceiptWireDto {
        profile_id: receipt.profile_id,
        screen_name: receipt.screen_name,
        avatar_url: receipt.avatar_url,
        // Trae CN 复用同一 complete_login 编排，验收语义与 Work 一致；
        // CN 真实续期效果仍以 ADR-0030 的独立实测验收为准。
        acceptance: receipt.acceptance,
        acceptance_error_code: receipt.acceptance_error_code,
    })
}

/// 取消进行中的 OAuth 登录（P7-4）：
/// - 置位取消标记 → complete 的等待循环在一个轮询周期内以 login_cancelled 收尾；
/// - 结束隔离浏览器子进程（登录窗口随之中页关闭，不留后台浏览器）；
/// - 兜底清理：complete 尚未被调用时（前端异常路径），顺带丢弃挂起会话
///   与隔离档案目录，避免按钮锁死后资源滞留。
fn cancel_login_inner(state: &AppState) -> Result<(), String> {
    state
        .login_cancel
        .store(true, std::sync::atomic::Ordering::Relaxed);
    if let Some(mut child) = state.active_login_browser.lock().unwrap().take() {
        // kill + wait 回收：避免 Windows 上留下僵尸进程句柄。
        let _ = child.kill();
        let _ = child.wait();
    }
    // 兜底：会话仍挂起说明 complete 未在等待（异常路径），就地清理。
    if state.pending_login.lock().unwrap().take().is_some() {
        if let Some(profile_dir) = state.active_oauth_profile.lock().unwrap().take() {
            tauri::async_runtime::spawn_blocking(move || {
                let _ = std::fs::remove_dir_all(profile_dir);
            });
        }
    }
    Ok(())
}

#[tauri::command]
pub(crate) fn cancel_trae_cn_login(state: tauri::State<AppState>) -> Result<(), String> {
    cancel_login_inner(&state)
}

// ============================================================================
// 内嵌测试（P12-R3 随域纯移动，数量不减）
// ============================================================================

#[cfg(test)]
mod login_receipt_wire_tests {
    use super::*;
    use traesync_infrastructure::LoginAcceptance;

    fn work_receipt(acceptance: LoginAcceptance) -> CheckinLoginReceiptWireDto {
        CheckinLoginReceiptWireDto {
            profile_id: "profile-1".to_string(),
            account_id: "700001".to_string(),
            screen_name: "账号一".to_string(),
            avatar_url: String::new(),
            acceptance: acceptance.status(),
            acceptance_error_code: acceptance.error_code,
        }
    }

    #[test]
    fn acceptance_passed_receipt_omits_error_code() {
        let json = serde_json::to_value(&work_receipt(LoginAcceptance {
            passed: true,
            error_code: None,
        }))
        .unwrap();
        assert_eq!(json["acceptance"], "passed");
        assert!(json.get("acceptance_error_code").is_none());
    }

    #[test]
    fn acceptance_failed_receipt_carries_stable_error_code_only() {
        let json = serde_json::to_value(&work_receipt(LoginAcceptance {
            passed: false,
            error_code: Some("credential_refresh_failed"),
        }))
        .unwrap();
        assert_eq!(json["acceptance"], "failed");
        assert_eq!(json["acceptance_error_code"], "credential_refresh_failed");
        // 非敏感白名单：token / 私钥正文不得进入登录回执。
        assert!(json.get("access_token").is_none());
        assert!(json.get("refresh_token").is_none());
        assert!(json.get("device_private_key").is_none());
    }

    #[test]
    fn trae_cn_receipt_carries_same_acceptance_semantics() {
        let dto = TraeCnLoginReceiptWireDto {
            profile_id: "profile-cn".to_string(),
            screen_name: "CN 账号".to_string(),
            avatar_url: String::new(),
            acceptance: LoginAcceptance {
                passed: false,
                error_code: Some("credential_refresh_failed"),
            }
            .status(),
            acceptance_error_code: Some("credential_refresh_failed"),
        };
        let json = serde_json::to_value(&dto).unwrap();
        assert_eq!(json["acceptance"], "failed");
        assert_eq!(json["acceptance_error_code"], "credential_refresh_failed");
        assert!(json.get("access_token").is_none());
    }
}

// P6-3：OAuth 临时浏览器档案超龄清理——目录名即创建时间戳，
// 超龄删除、未超龄保留（保护进行中的登录）、非时间戳条目不动。
#[cfg(test)]
mod oauth_profile_cleanup_tests {
    use super::*;

    #[test]
    fn removes_expired_dirs_and_keeps_recent_and_non_timestamp() {
        let root = tempfile::tempdir().unwrap();
        let now = 10_000_u64;
        // 超龄目录（now - created = 1000 >= 600）：删除，含内容一并清掉。
        let expired = root.path().join("9000");
        std::fs::create_dir_all(&expired).unwrap();
        std::fs::write(expired.join("Cookies"), b"x").unwrap();
        // 进行中登录目录（now - created = 100 < 600）：保留。
        std::fs::create_dir_all(root.path().join("9900")).unwrap();
        // 非时间戳命名：不猜来历，不动。
        std::fs::create_dir_all(root.path().join("not-a-timestamp")).unwrap();

        cleanup_expired_oauth_profiles_in_root(root.path(), now);

        assert!(!expired.exists());
        assert!(root.path().join("9900").exists());
        assert!(root.path().join("not-a-timestamp").exists());
    }

    #[test]
    fn missing_root_is_noop() {
        // 根目录不存在（从未登录过）不报错、不创建。
        let missing = Path::new("Z:/trae-sync-test-missing-oauth-root");
        cleanup_expired_oauth_profiles_in_root(missing, 1_000_000);
        assert!(!missing.exists());
    }

    #[test]
    fn clock_skew_keeps_future_dated_dirs() {
        // 目录时间戳晚于当前时间（时钟回拨场景）：saturating 到 0，不删。
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("999999")).unwrap();

        cleanup_expired_oauth_profiles_in_root(root.path(), 1_000);

        assert!(root.path().join("999999").exists());
    }
}

/// 登录命令链门禁与会话消费测试（P12-R3 自 real_checkin_tests 随域纯移动）；
/// AppState 构造夹具为跨域共享，提至 test_support。
#[cfg(test)]
mod login_command_tests {
    use super::*;
    use crate::list_checkin_accounts_inner;
    use crate::test_support::make_real_mode_state;
    use crate::RuntimeMode;

    #[test]
    fn login_commands_reject_fixture_mode() {
        // fixture 模式下登录命令失败关闭：登录入库只属于真实模式。
        let mut state = make_real_mode_state(Path::new("/nonexistent"));
        state.runtime_mode = RuntimeMode::Fixture;
        assert_eq!(
            begin_checkin_login_inner(&state).map(|_| ()).unwrap_err(),
            "login_real_mode_required"
        );
        assert_eq!(
            list_checkin_accounts_inner(&state).unwrap_err(),
            "login_real_mode_required"
        );
    }

    #[test]
    fn begin_login_rejects_when_storage_root_unavailable() {
        let state = make_real_mode_state(Path::new(""));
        assert_eq!(
            begin_checkin_login_inner(&state).map(|_| ()).unwrap_err(),
            "checkin_storage_unavailable"
        );
    }

    #[test]
    fn begin_login_produces_url_and_consumable_session() {
        let root = tempfile::tempdir().unwrap();
        let state = make_real_mode_state(root.path());
        let (login_url, session) = begin_checkin_login_inner(&state).unwrap();
        // URL 指向 TRAE 授权页并携带 PKCE challenge 与本地回调声明。
        assert!(login_url.starts_with("https://www.trae.cn/authorization?"));
        assert!(login_url.contains("code_challenge="));
        assert!(login_url.contains("auth_callback_url=http%3A%2F%2F127.0.0.1%3A"));
        assert!(!session.profile_id.is_empty());

        // 会话一次性消费：第一次取走成功，第二次失败。
        *state.pending_login.lock().unwrap() = Some(session);
        assert!(take_pending_login(&state).is_ok());
        assert_eq!(
            take_pending_login(&state).map(|_| ()).unwrap_err(),
            "login_not_started"
        );
    }
}
