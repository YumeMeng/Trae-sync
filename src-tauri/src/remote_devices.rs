//! 远程设备管理域（ADR-0031 账号级远程设备管理，P10-5 切片 2/3）。
//!
//! P12-R3 自 lib.rs 纯移动：设备列表只读 + 远程退出命令及 wire DTO，
//! 命令体、错误码与 wire 形态零变化。产品账号路由
//! `resolve_remote_device_account` 按定案留在组合根（lib.rs），
//! 本模块经 `crate::` 引用（同 crate 内 mod 互引）。

use std::path::Path;

use serde::Serialize;
use traesync_application::RemoteDeviceManager;
use traesync_infrastructure::{CheckinProfileBinding, RealRemoteDeviceManager};

use crate::{
    checkin_material_root, ensure_login_real_mode, resolve_remote_device_account, run_blocking,
    trae_cn_material_root, AppState,
};
#[cfg(windows)]
use crate::open_login_url_in_browser;

/// 远程设备行 DTO：与 ports::RemoteDeviceEntry 对齐的 wire 形态（snake_case）。
/// device_id 仅用于前端退出定位与内部流转，UI 不渲染。
#[derive(Serialize)]
struct RemoteDeviceEntryDto {
    device_id: String,
    device_type: Option<String>,
    device_name: Option<String>,
    bound_products: Vec<String>,
    /// 服务端原样保留（字符串/毫秒时间戳双形态），格式化交给 UI 层。
    last_active_at: Option<serde_json::Value>,
    is_local: bool,
}

/// 设备列表快照 DTO：账号归属标注（展示名）+ 全量设备行 + 已用/上限摘要。
/// pub(crate)：tauri 命令宏展开的包装函数签名需要暴露返回类型。
#[derive(Serialize)]
pub(crate) struct RemoteDeviceSnapshotDto {
    profile_id: String,
    /// 账号展示名（本地备注名优先，其次服务端昵称；均不可用回退中性文案「当前账号」）。
    account_label: String,
    devices: Vec<RemoteDeviceEntryDto>,
    used_count: usize,
    max_count: Option<u32>,
}

/// 查看账号的远程设备占用列表（只读，ADR-0031 决策 5：按账号全量展示）。
/// Work CN 与 Trae CN 共用同一命令；凭据按产品各自的材料根解析。
#[tauri::command]
pub(crate) async fn list_remote_devices(
    product_id: String,
    profile_id: Option<String>,
    state: tauri::State<'_, AppState>,
) -> Result<RemoteDeviceSnapshotDto, String> {
    ensure_login_real_mode(&state)?;
    let material_root = match product_id.as_str() {
        "work_cn" => checkin_material_root(&state)?,
        "trae_cn" => trae_cn_material_root(&state)?,
        _ => return Err("unknown_product".to_string()),
    };
    let storage_root = state.storage_root.clone();
    run_blocking("remote_devices_join_failed", move || {
        list_remote_devices_inner(&material_root, &storage_root, &product_id, profile_id)
    })
    .await
}

/// 命令主体（阻塞线程执行）：解析目标账号 → 构造产品对应的远程设备管理器
/// → 直连服务端拉取列表。协议错误透传稳定原因码（RemoteDeviceError Display）。
fn list_remote_devices_inner(
    material_root: &Path,
    storage_root: &str,
    product_id: &str,
    profile_id: Option<String>,
) -> Result<RemoteDeviceSnapshotDto, String> {
    let (record, oauth_client) = resolve_remote_device_account(
        material_root,
        storage_root,
        product_id,
        profile_id.as_deref(),
    )?;

    // 归属标注：本地备注名优先，其次服务端昵称；两者都为空回退中性文案
    //（界面表达纪律：profile_id 等内部标识不进主视野）。
    let account_label = record
        .display_name
        .clone()
        .unwrap_or_else(|| record.screen_name.clone());
    let account_label = if account_label.trim().is_empty() {
        "当前账号".to_string()
    } else {
        account_label
    };

    let binding = CheckinProfileBinding::from(&record);
    let manager = RealRemoteDeviceManager::new(material_root, binding, oauth_client);
    let snapshot = manager
        .list_devices(&record.profile_id)
        .map_err(|error| error.to_string())?;
    Ok(RemoteDeviceSnapshotDto {
        profile_id: record.profile_id.clone(),
        account_label,
        devices: snapshot
            .devices
            .into_iter()
            .map(|device| RemoteDeviceEntryDto {
                device_id: device.device_id,
                device_type: device.device_type,
                device_name: device.device_name,
                bound_products: device.bound_products,
                last_active_at: device.last_active_at,
                is_local: device.is_local,
            })
            .collect(),
        used_count: snapshot.used_count,
        max_count: snapshot.max_count,
    })
}

/// 远程退出目标设备并清除其刷新令牌（ADR-0031 决策 3/4：逐台单次确认由前端
/// 保证，目标是本机设备时协议层 clear_refresh_token 直接拒绝，为第二道防线）。
/// 纯服务端操作：不写任何本地状态文件，成功即返回。
#[tauri::command]
pub(crate) async fn clear_remote_device(
    product_id: String,
    profile_id: Option<String>,
    device_id: String,
    state: tauri::State<'_, AppState>,
) -> Result<(), String> {
    ensure_login_real_mode(&state)?;
    // 参数防御：空目标无法定位设备，直接给稳定原因码。
    if device_id.is_empty() {
        return Err("remote_device_empty_target".to_string());
    }
    let material_root = match product_id.as_str() {
        "work_cn" => checkin_material_root(&state)?,
        "trae_cn" => trae_cn_material_root(&state)?,
        _ => return Err("unknown_product".to_string()),
    };
    let storage_root = state.storage_root.clone();
    run_blocking("remote_device_join_failed", move || {
        clear_remote_device_inner(
            &material_root,
            &storage_root,
            &product_id,
            profile_id.as_deref(),
            &device_id,
        )
    })
    .await
}

/// 命令主体（阻塞线程执行）：与 list_remote_devices 同一账号路由
/// → 构造产品对应的远程设备管理器 → 直连服务端清除目标设备凭据。
/// 协议错误透传稳定原因码（RemoteDeviceError Display）。
fn clear_remote_device_inner(
    material_root: &Path,
    storage_root: &str,
    product_id: &str,
    profile_id: Option<&str>,
    device_id: &str,
) -> Result<(), String> {
    let (record, oauth_client) =
        resolve_remote_device_account(material_root, storage_root, product_id, profile_id)?;
    let binding = CheckinProfileBinding::from(&record);
    let manager = RealRemoteDeviceManager::new(material_root, binding, oauth_client);
    manager
        .clear_refresh_token(&record.profile_id, device_id)
        .map_err(|error| error.to_string())
}

/// 官方设备管理文档页（研究文档 .scratch/research-trae-device-management-20260923.md
/// 记录的唯一官方入口；官方授权页无公开稳定 URL，以文档页兜底）。
#[cfg(windows)]
const DEVICE_MANAGEMENT_DOCS_URL: &str = "https://docs.trae.cn/ide_device-limit";

/// 打开官方设备管理页（ADR-0031 决策 2：原生列表失败时的兜底入口）。
/// 复用 OAuth 登录同一浏览器拉起机制（系统浏览器 + rundll32 回退）；
/// 固定 URL 不接受前端传参，避免被诱导打开任意地址。失败静默（无跟进动作）。
#[tauri::command]
pub(crate) fn open_device_management_docs() {
    // 非 Windows 平台与登录命令同策略：不由后端拉起浏览器。
    #[cfg(windows)]
    let _ = open_login_url_in_browser(DEVICE_MANAGEMENT_DOCS_URL, true);
}
