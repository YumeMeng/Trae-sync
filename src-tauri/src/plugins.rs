//! 插件域（P5-8b 插件 tab：ADR-0023 环境插件清单 + ADR-0026 实时同步）。
//!
//! P12-R3 自 lib.rs 纯移动：tab 状态/市场浏览/安装/全账号卸载传播四命令，
//! 清单构造（manifest_entries_from_cloud）、当前账号凭据解析
//! （plugin_tab_account_token）、跨账号传播器与 wire DTO 及
//! plugin_sync_policy_tests 两例一并随域迁移；命令体、错误码与 wire 形态
//! 零变化。`manifest_entries_from_cloud` 另供凭据域切号插件预检复用
//! （credential_handover 经 `crate::plugins::` 引用）；门禁/阻塞包装/
//! 主库当前账号解析留守组合根，经 `crate::` 引用（同 crate 内 mod 互引）。

use std::path::{Path, PathBuf};

use serde::Serialize;

use traesync_infrastructure::environment_registry::master_data_dir;
use traesync_infrastructure::{
    fetch_installed_plugins, fetch_market_plugins, find_uninstall_target, install_market_plugin,
    uninstall_cloud_plugin, AccountRegistry, CheckinCredentialStore, CheckinProfileBinding,
    CloudPluginItem, MarketPluginItem, PluginManifest, PluginManifestEntry,
};

use crate::{
    checkin_material_root, ensure_real_preview, resolve_actual_master_account, run_blocking,
    AppState,
};

// ===== P5-8b 插件 tab（ADR-0023 环境插件清单）：状态/市场/装/卸/吸收 =====

/// 云端已装条目 DTO（含清单归属标记，供前端差异提示）。
#[derive(Clone, Serialize)]
struct InstalledPluginDto {
    /// 不透明记录 ID（卸载键）。
    record_id: String,
    marketplace_plugin_id: Option<String>,
    name: String,
    display_name: String,
    version: String,
    registry: String,
    /// builtin 条目 = 客户端内置，不可卸载/同步。
    builtin: bool,
    /// 条目（按市场 UUID）是否已在环境清单内。
    in_manifest: bool,
}

/// 清单条目 DTO（含云端在装标记，供「清单待应用/已移除」提示）。
#[derive(Clone, Serialize)]
struct ManifestPluginDto {
    marketplace_plugin_id: String,
    name: String,
    display_name: String,
    version: String,
    registry: String,
    /// 清单条目在当前账号云端是否已装。
    installed_in_cloud: bool,
}

/// 插件 tab 状态（get_plugin_tab_state 返回）。
/// pub(crate)：tauri 命令宏展开的包装函数签名需要暴露返回类型。
#[derive(Clone, Serialize)]
pub(crate) struct PluginTabStateDto {
    installed: Vec<InstalledPluginDto>,
    manifest: Vec<ManifestPluginDto>,
    /// 工具已知账号数（含当前账号；ADR-0026 决策 2 卸载确认文案
    /// 「将同时从 N 个账号移除」的 N）。
    known_account_count: usize,
}

/// 插件 tab 当前账号凭据解析：主库登录态实测 user_id → 账号注册表
/// → 凭据包 access_token。任一环节缺失返回稳定错误码（前端据此降级文案）。
fn plugin_tab_account_token(material_root: &Path, storage_root: &Path) -> Result<String, String> {
    let master_dir = master_data_dir().map_err(|_| "environment_registry_invalid".to_string())?;
    let account = resolve_actual_master_account(material_root, storage_root, &master_dir)?
        .ok_or_else(|| "plugin_tab_no_account".to_string())?;
    CheckinCredentialStore::new(material_root)
        .load(&CheckinProfileBinding::from(&account))
        .map(|bundle| bundle.access_token)
        .map_err(|_| "plugin_tab_credential_unavailable".to_string())
}

/// 从云端已装列表构造清单条目（首次导入/吸收共用，ADR-0023 决策 1/2）：
/// 只收非 builtin 且有 marketplace_plugin_id 的市场插件；空字段给回退值
/// （清单 validate 拒绝空串）。
/// pub(crate)：凭据域切号预检（credential_handover）复用同一构造。
pub(crate) fn manifest_entries_from_cloud(items: &[CloudPluginItem]) -> Vec<PluginManifestEntry> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0);
    items
        .iter()
        .filter(|item| !item.builtin)
        .filter_map(|item| {
            let id = item.marketplace_plugin_id.clone()?;
            Some(PluginManifestEntry {
                name: if item.name.is_empty() {
                    id.clone()
                } else {
                    item.name.clone()
                },
                display_name: if item.display_name.is_empty() {
                    item.name.clone()
                } else {
                    item.display_name.clone()
                },
                version: if item.version.is_empty() {
                    "0.0.0".into()
                } else {
                    item.version.clone()
                },
                registry: if item.registry.is_empty() {
                    "unknown".into()
                } else {
                    item.registry.clone()
                },
                marketplace_plugin_id: id,
                added_unix_seconds: now,
            })
        })
        .collect()
}

/// 读取插件 tab 状态：云端已装 + 环境清单 + 双向归属标记。
/// 清单文件不存在 = 首次启用，从当前账号云端导入后落盘（ADR-0023 决策 1）。
#[tauri::command]
pub(crate) async fn get_plugin_tab_state(
    state: tauri::State<'_, AppState>,
) -> Result<PluginTabStateDto, String> {
    ensure_real_preview(&state)?;
    let material_root = checkin_material_root(&state)?;
    let storage_root = PathBuf::from(&state.storage_root);
    run_blocking("plugin_tab_state_join_failed", move || {
        let token = plugin_tab_account_token(&material_root, &storage_root)?;
        let installed = fetch_installed_plugins(&token)
            .map_err(|_| "plugin_tab_cloud_unavailable".to_string())?;
        let manifest = PluginManifest::new(&storage_root);
        let entries = match manifest
            .load()
            .map_err(|_| "plugin_manifest_invalid".to_string())?
        {
            Some(entries) => entries,
            None => {
                // 首次启用：当前账号云端现状即基线。
                let imported = manifest_entries_from_cloud(&installed);
                manifest
                    .save(&imported)
                    .map_err(|_| "plugin_manifest_write_failed".to_string())?;
                imported
            }
        };
        let manifest_ids: std::collections::HashSet<&str> = entries
            .iter()
            .map(|entry| entry.marketplace_plugin_id.as_str())
            .collect();
        let cloud_ids: std::collections::HashSet<&str> = installed
            .iter()
            .filter_map(|item| item.marketplace_plugin_id.as_deref())
            .collect();
        let installed_dto = installed
            .iter()
            .map(|item| InstalledPluginDto {
                record_id: item.record_id.clone(),
                marketplace_plugin_id: item.marketplace_plugin_id.clone(),
                name: item.name.clone(),
                display_name: item.display_name.clone(),
                version: item.version.clone(),
                registry: item.registry.clone(),
                builtin: item.builtin,
                in_manifest: item
                    .marketplace_plugin_id
                    .as_deref()
                    .is_some_and(|id| manifest_ids.contains(id)),
            })
            .collect();
        let manifest_dto = entries
            .iter()
            .map(|entry| ManifestPluginDto {
                marketplace_plugin_id: entry.marketplace_plugin_id.clone(),
                name: entry.name.clone(),
                display_name: entry.display_name.clone(),
                version: entry.version.clone(),
                registry: entry.registry.clone(),
                installed_in_cloud: cloud_ids.contains(entry.marketplace_plugin_id.as_str()),
            })
            .collect();
        // 已知账号数：账号注册表全量（含归档账号——凭据仍在，云端可能
        // 装着该插件；卸载传播同样遍历全量，口径一致）。
        let known_account_count = AccountRegistry::new(&material_root)
            .load()
            .map(|records| records.len())
            .unwrap_or(0);
        Ok(PluginTabStateDto {
            installed: installed_dto,
            manifest: manifest_dto,
            known_account_count,
        })
    })
    .await
}

/// 浏览插件市场目录（GET /extensions/api/-/plugin/list，只读）。
#[tauri::command]
pub(crate) async fn browse_plugin_market(
    state: tauri::State<'_, AppState>,
) -> Result<Vec<MarketPluginItem>, String> {
    ensure_real_preview(&state)?;
    let material_root = checkin_material_root(&state)?;
    let storage_root = PathBuf::from(&state.storage_root);
    run_blocking("plugin_market_join_failed", move || {
        let token = plugin_tab_account_token(&material_root, &storage_root)?;
        fetch_market_plugins(&token).map_err(|_| "plugin_market_unavailable".to_string())
    })
    .await
}

/// 安装市场插件到当前账号云端（ADR-0023 决策 3：即时改云端 + 更新清单）。
/// 入参为市场条目字段（plugin_id = 市场 UUID）。
#[tauri::command]
pub(crate) async fn install_plugin(
    state: tauri::State<'_, AppState>,
    plugin_id: String,
    name: String,
    display_name: String,
    registry: String,
) -> Result<(), String> {
    ensure_real_preview(&state)?;
    let material_root = checkin_material_root(&state)?;
    let storage_root = PathBuf::from(&state.storage_root);
    run_blocking("plugin_install_join_failed", move || {
        let token = plugin_tab_account_token(&material_root, &storage_root)?;
        let plugin = MarketPluginItem {
            plugin_id,
            name,
            display_name,
            description: String::new(),
            registry,
            // 分类仅用于市场目录展示；安装流程不消费，构造占位值。
            categories: Vec::new(),
            category_key: String::new(),
            category_name: String::new(),
        };
        let installed_ok = install_market_plugin(&token, &plugin)
            .map_err(|_| "plugin_install_failed".to_string())?;
        if !installed_ok {
            return Err("plugin_install_failed".to_string());
        }
        // 清单更新：优先取安装后的云端权威数据（版本等字段准确）；
        // 列表滞后时回退市场条目字段（版本未知记 0.0.0）。
        let mut entry = PluginManifestEntry {
            name: if plugin.name.is_empty() {
                plugin.plugin_id.clone()
            } else {
                plugin.name.clone()
            },
            display_name: if plugin.display_name.is_empty() {
                plugin.name.clone()
            } else {
                plugin.display_name.clone()
            },
            version: "0.0.0".to_string(),
            registry: if plugin.registry.is_empty() {
                "unknown".to_string()
            } else {
                plugin.registry.clone()
            },
            marketplace_plugin_id: plugin.plugin_id.clone(),
            added_unix_seconds: 0,
        };
        if let Ok(items) = fetch_installed_plugins(&token) {
            if let Some(item) = items
                .iter()
                .find(|item| item.marketplace_plugin_id.as_deref() == Some(&plugin.plugin_id))
            {
                entry.version = item.version.clone();
                entry.registry = item.registry.clone();
            }
        }
        entry.added_unix_seconds = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_secs())
            .unwrap_or(0);
        let manifest = PluginManifest::new(&storage_root);
        let mut entries = manifest
            .load()
            .map_err(|_| "plugin_manifest_invalid".to_string())?
            .unwrap_or_default();
        entries.retain(|existing| existing.marketplace_plugin_id != entry.marketplace_plugin_id);
        entries.push(entry);
        manifest
            .save(&entries)
            .map_err(|_| "plugin_manifest_write_failed".to_string())
    })
    .await
}

/// 卸载全账号传播的逐账号回执（ADR-0026 决策 2）。
#[derive(Clone, Serialize)]
struct PluginUninstallAccountDto {
    /// 账号展示名（备注名优先，回退服务端昵称；不含内部 ID）。
    display_name: String,
    /// true = 已从云端移除；false = 无需移除（该账号没装）。
    removed: bool,
    /// 移除失败 true（fail-soft：只记失败不中断其他账号）。
    failed: bool,
    /// 失败原因码（成功时 None；技术细节收悬浮提示，不进主视野）。
    error_code: Option<String>,
}

/// uninstall_plugin_everywhere 返回：逐账号成败回执。
/// pub(crate)：tauri 命令宏展开的包装函数签名需要暴露返回类型。
#[derive(Clone, Serialize)]
pub(crate) struct PluginUninstallEverywhereDto {
    accounts: Vec<PluginUninstallAccountDto>,
}

/// 卸载插件并传播到全部已知账号（ADR-0026 决策 2，前端已单次确认）：
/// 当前账号云端卸载（失败即整单报错，不传播）→ 清单移除 → 其他已知
/// 账号云端逐个卸载（fail-soft，逐账号尽力；builtin 跳过）。
/// 自装条目（无市场 UUID）只处理当前账号——没有跨账号的可靠匹配键。
#[tauri::command]
pub(crate) async fn uninstall_plugin_everywhere(
    state: tauri::State<'_, AppState>,
    record_id: String,
) -> Result<PluginUninstallEverywhereDto, String> {
    ensure_real_preview(&state)?;
    let material_root = checkin_material_root(&state)?;
    let storage_root = PathBuf::from(&state.storage_root);
    run_blocking("plugin_uninstall_join_failed", move || {
        let master_dir =
            master_data_dir().map_err(|_| "environment_registry_invalid".to_string())?;
        let current_account =
            resolve_actual_master_account(&material_root, &storage_root, &master_dir)?
                .ok_or_else(|| "plugin_tab_no_account".to_string())?;
        let token = plugin_tab_account_token(&material_root, &storage_root)?;
        // 先查条目：校验非 builtin，并取市场 UUID 供清单移除与传播匹配。
        let items = fetch_installed_plugins(&token)
            .map_err(|_| "plugin_tab_cloud_unavailable".to_string())?;
        let item = items
            .iter()
            .find(|item| item.record_id == record_id)
            .ok_or_else(|| "plugin_not_found".to_string())?;
        if item.builtin {
            return Err("plugin_builtin_uninstallable".to_string());
        }
        // 当前账号云端卸载：操作主体，失败整单报错（与安装对称的即时语义）。
        let removed_ok = uninstall_cloud_plugin(&token, &record_id)
            .map_err(|_| "plugin_uninstall_failed".to_string())?;
        if !removed_ok {
            return Err("plugin_uninstall_failed".to_string());
        }
        // 当前账号回执展示名：注册表账号名（备注名优先）；读不到时
        // 用通用兜底（回执是账号维度结果，不塞插件名）。
        let current_name = account_display_name(&current_account);
        let mut accounts = vec![PluginUninstallAccountDto {
            display_name: current_name,
            removed: true,
            failed: false,
            error_code: None,
        }];
        // 清单移除该插件（无市场 UUID 的自装条目本就不在清单内）。
        if let Some(market_id) = &item.marketplace_plugin_id {
            let manifest = PluginManifest::new(&storage_root);
            if let Some(mut entries) = manifest
                .load()
                .map_err(|_| "plugin_manifest_invalid".to_string())?
            {
                entries.retain(|entry| &entry.marketplace_plugin_id != market_id);
                manifest
                    .save(&entries)
                    .map_err(|_| "plugin_manifest_write_failed".to_string())?;
            }
            // 传播到其他已知账号：逐账号尽力，失败不中断（fail-soft）。
            let records = AccountRegistry::new(&material_root)
                .load()
                .map_err(|_| "checkin_registry_invalid".to_string())?;
            let others = propagate_uninstall_to_accounts(
                &records,
                Some(current_account.profile_id.as_str()),
                market_id,
                &material_root,
            );
            accounts.extend(others);
        }
        Ok(PluginUninstallEverywhereDto { accounts })
    })
    .await
}

/// 账号展示名：备注名优先，回退服务端昵称（界面纪律：内部 ID 不进主视野）。
fn account_display_name(
    record: &traesync_infrastructure::account_registry::AccountRecord,
) -> String {
    record
        .display_name
        .clone()
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| record.screen_name.clone())
}

/// 移除传播执行器（ADR-0026 决策 2，依赖注入版供 tests 验证 fail-soft）：
/// 逐账号 load_token → fetch 列表 → 按市场 UUID 定位（builtin 跳过）→
/// uninstall。任一账号失败只记回执不中断后续；未装不算失败。
/// current_profile_id 对应账号已单独处理，跳过。
fn propagate_uninstall_with(
    records: &[traesync_infrastructure::account_registry::AccountRecord],
    current_profile_id: Option<&str>,
    marketplace_plugin_id: &str,
    mut load_token: impl FnMut(
        &traesync_infrastructure::account_registry::AccountRecord,
    ) -> Option<String>,
    mut fetch: impl FnMut(&str) -> Option<Vec<CloudPluginItem>>,
    mut uninstall: impl FnMut(&str, &str) -> bool,
) -> Vec<PluginUninstallAccountDto> {
    let mut results = Vec::new();
    for record in records {
        if Some(record.profile_id.as_str()) == current_profile_id {
            continue;
        }
        let display_name = account_display_name(record);
        let mut entry = PluginUninstallAccountDto {
            display_name,
            removed: false,
            failed: false,
            error_code: None,
        };
        match load_token(record).filter(|token| !token.is_empty()) {
            None => {
                entry.failed = true;
                entry.error_code = Some("plugin_propagate_credential_unavailable".to_string());
            }
            Some(token) => match fetch(&token) {
                None => {
                    entry.failed = true;
                    entry.error_code = Some("plugin_propagate_cloud_unavailable".to_string());
                }
                Some(items) => match find_uninstall_target(&items, marketplace_plugin_id) {
                    // 该账号没装：无需移除（不是失败）。
                    None => {}
                    Some(other_record_id) => {
                        if uninstall(&token, &other_record_id) {
                            entry.removed = true;
                        } else {
                            entry.failed = true;
                            entry.error_code =
                                Some("plugin_propagate_uninstall_failed".to_string());
                        }
                    }
                },
            },
        }
        results.push(entry);
    }
    results
}

/// 移除传播到其他账号（生产实现）：凭据包取 token → 云端列表 → 云端卸载。
fn propagate_uninstall_to_accounts(
    records: &[traesync_infrastructure::account_registry::AccountRecord],
    current_profile_id: Option<&str>,
    marketplace_plugin_id: &str,
    material_root: &Path,
) -> Vec<PluginUninstallAccountDto> {
    let store = CheckinCredentialStore::new(material_root);
    propagate_uninstall_with(
        records,
        current_profile_id,
        marketplace_plugin_id,
        |record| {
            store
                .load(&CheckinProfileBinding::from(record))
                .ok()
                .map(|bundle| bundle.access_token)
        },
        |token| fetch_installed_plugins(token).ok(),
        |token, record_id| uninstall_cloud_plugin(token, record_id).unwrap_or(false),
    )
}

// ============================================================================

/// G23 插件同步策略（ADR-0026）：移除全账号传播（逐账号 fail-soft、builtin
/// 跳过）。切号预检分支（switch_plugin_preview_from_lists）随凭据域迁
/// credential_handover::switch_plugin_preview_tests（P12-R3）。
#[cfg(test)]
mod plugin_sync_policy_tests {
    use super::*;

    /// 构造云端已装条目（传播测试共用）。
    fn cloud_item(record_id: &str, market_id: Option<&str>, name: &str) -> CloudPluginItem {
        let mut item = CloudPluginItem {
            record_id: record_id.to_string(),
            marketplace_plugin_id: market_id.map(|id| id.to_string()),
            name: name.to_string(),
            display_name: name.to_string(),
            version: "1.0.0".to_string(),
            registry: "trae-remote-official".to_string(),
            builtin: false,
        };
        item.builtin = record_id.starts_with("builtin:");
        item
    }

    /// 构造注册表档案（传播测试共用；profile_id 决定当前账号判定）。
    fn account(profile_id: &str, name: &str) -> traesync_infrastructure::AccountRecord {
        traesync_infrastructure::AccountRecord {
            profile_id: profile_id.to_string(),
            account_id: format!("uid-{profile_id}"),
            screen_name: name.to_string(),
            avatar_url: String::new(),
            device_id: "1234567890123456".to_string(),
            device_public_key: "pub".to_string(),
            display_name: None,
            masked_mobile: String::new(),
            created_at_unix_seconds: 0,
            last_verified_at_unix_seconds: 0,
            device_created_at_unix_seconds: 0,
            auto_checkin_enabled: true,
            archived: false,
        }
    }

    #[test]
    fn propagate_uninstall_is_fail_soft_per_account() {
        // 逐账号尽力：甲凭据不可用、乙成功、丙云端卸载失败、丁未装——
        // 任一失败不中断后续账号，全部账号都有回执。
        let records = vec![
            account("profile-a", "账号甲"),
            account("profile-b", "账号乙"),
            account("profile-c", "账号丙"),
            account("profile-d", "账号丁"),
        ];
        let results = propagate_uninstall_with(
            &records,
            None, // 无当前账号（全部按其他账号处理）
            "uuid-x",
            |record| match record.profile_id.as_str() {
                "profile-a" => None, // 凭据不可用
                _ => Some(format!("token-{}", record.profile_id)),
            },
            |token| {
                if token == "token-profile-b" || token == "token-profile-c" {
                    Some(vec![cloud_item("r-b1", Some("uuid-x"), "目标插件")])
                } else {
                    Some(vec![]) // 丁没装
                }
            },
            |token, _record_id| token != "token-profile-c", // 丙的卸载失败
        );
        assert_eq!(results.len(), 4);
        // 甲：凭据不可用 → 失败但继续。
        assert!(results[0].failed);
        assert!(!results[0].removed);
        assert_eq!(
            results[0].error_code.as_deref(),
            Some("plugin_propagate_credential_unavailable")
        );
        assert_eq!(results[0].display_name, "账号甲");
        // 乙：成功移除。
        assert!(results[1].removed && !results[1].failed);
        // 丙：卸载失败（fail-soft 记失败，不影响丁）。
        assert!(results[2].failed && !results[2].removed);
        // 丁：未装 → 无需移除，不算失败。
        assert!(!results[3].removed && !results[3].failed);
        assert_eq!(results[3].error_code, None);
    }

    #[test]
    fn propagate_uninstall_skips_current_account_and_builtin() {
        // 当前账号已单独处理（云端卸载 + 清单移除），传播跳过；
        // 其他账号列表中的 builtin 条目即使带上同 UUID 也不可卸载。
        let records = vec![
            account("profile-current", "当前账号"),
            account("profile-other", "账号乙"),
        ];
        let mut builtin_with_same_id =
            cloud_item("builtin:trae-remote-official:x", Some("uuid-x"), "内置");
        builtin_with_same_id.builtin = true;
        let results = propagate_uninstall_with(
            &records,
            Some("profile-current"),
            "uuid-x",
            |_| Some("token".to_string()),
            |_| Some(vec![builtin_with_same_id.clone()]),
            |_, _| panic!("builtin 条目不可触发卸载"),
        );
        // 只剩账号乙（当前账号被跳过）；builtin 无可卸载目标 → 未装语义。
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].display_name, "账号乙");
        assert!(!results[0].removed && !results[0].failed);
    }
}
