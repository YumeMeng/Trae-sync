//! 插件云端预同步（切号编排第 4.5 步，2026-08-31；2026-09-01 按 ADR-0023
//! 改造为「吸收后应用」对账）：以源账号（切换前账号）云端现状为环境清单，
//! 把清单应用到目标账号——缺的装上、多的卸掉。
//!
//! 背景：切号重启后 TRAE 按目标账号云端插件列表调和本地安装（云端没有的
//! 会被卸载）。对账在重启前把差异消除，重启后 TRAE 调和即无事发生。
//! 移除属破坏性操作：由切号弹层预检（preview_master_switch_plugins，
//! lib.rs）以 +N/-M 形式一次确认后执行；「多装少卸」的纯增量场景同样
//! 走确认（差异即用户可见的变化）。
//!
//! 协议事实（.scratch/history-u6/w0-probe/plugin_sync_probe.rs 实测闭环，
//! 2026-08-31 真机 4/4 成功 + 复核确认；证据同 docs/TECHNICAL_BASELINE.md）：
//! 1. GET  {REMOTE_API_BASE}/api/remote/v1/plugins?page_size=200
//!    —— 账号云端已装列表（鉴权 Cloud-IDE-JWT）。
//! 2. GET  {MARKET_API_BASE}/extensions/api/-/plugin/detail?plugin_id=&registry=
//!    —— 市场插件详情（download_url/checksum/file_size/manifest_json 等）。
//! 3. POST {REMOTE_API_BASE}/api/remote/v1/plugins
//!    —— 安装到账号云端；请求体与 solo-lite 前端 m() 构造器字段对齐，
//!    并附 bN() 产品参数（product_name=Lite/os=Windows）。
//! 4. DELETE {REMOTE_API_BASE}/api/remote/v1/plugins/:record_id
//!    —— 从账号云端移除（api_gap_probe 实测 code 0 + 往返无损）。
//!
//! 失败语义（fail-soft）：插件同步不阻断切号主流程——同步失败只意味着
//! 目标账号需手动重装插件（可恢复、非破坏性），切号本身必须照常完成。

use std::collections::HashSet;
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// solo-lite 渲染端实录的 remote API 域名（/api/remote/v1 家族）。
const REMOTE_API_BASE: &str = "https://trae-api-cn.mchost.guru";
/// 市场插件详情域名（network.log 实录，boot config marketApi）。
const MARKET_API_BASE: &str = "https://api.trae.com.cn";
/// 插件列表分页大小：单页覆盖个人账号全部已装插件（实测最大 5）。
const PAGE_SIZE: usize = 200;
/// 逐项安装间隔：与签到同族的串行节奏，避免触发服务端限流。
const INSTALL_INTERVAL: Duration = Duration::from_secs(2);

/// 一次对账的结果回执（fail-soft：aborted=true 表示整体未执行）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct PluginCloudSyncOutcome {
    /// 源账号云端市场插件数（吸收进环境清单的数量）。
    pub source_count: usize,
    /// 成功安装到目标账号云端的插件数。
    pub installed: usize,
    /// 成功从目标账号云端移除的插件数（清单外多余插件）。
    pub removed: usize,
    /// 装卸失败数（详情缺失/网络/业务码非 0）。
    pub failed: usize,
    /// 跳过数（源账号自装无市场 ID，无法跨账号同步）。
    pub skipped: usize,
    /// 整体未执行（列表拉取失败/凭据不可用/源账号不在注册表）。
    pub aborted: bool,
    /// 用户在预检中选择「以目标账号现状为准」：未执行对账，清单保持原状。
    pub declined: bool,
    /// 吸收集（源账号云端市场插件定型视图）；aborted 时为空。
    /// 供 lib.rs 落盘环境清单（ADR-0023「吸收」），不进前端 DTO。
    #[serde(skip)]
    pub absorbed: Vec<CloudPluginItem>,
}

/// 切号插件对账（ADR-0023「吸收后应用」，fail-soft，不返回 Err）：
/// 吸收集 = 源账号云端市场插件（随回执带回，调用方落盘清单）；
/// 应用 = 缺的装上 + 多的卸掉。移除属破坏性操作，由切号弹层预检
/// 以 +N/-M 形式一次确认后才会走到这里。
///
/// 供切号编排调用：源 = 切换前登录账号，目标 = 切换目标账号。
/// 任一账号列表拉取失败即中止（token 失效属可预期情况，交上层透出）。
pub fn sync_account_cloud_plugins(
    source_token: &str,
    target_token: &str,
) -> PluginCloudSyncOutcome {
    let client = crate::checkin_http::trae_http_client();
    let mut outcome = PluginCloudSyncOutcome {
        source_count: 0,
        installed: 0,
        removed: 0,
        failed: 0,
        skipped: 0,
        aborted: false,
        declined: false,
        absorbed: Vec::new(),
    };

    let source_items = match fetch_installed(&client, source_token) {
        Ok(items) => items,
        Err(_) => {
            outcome.aborted = true;
            return outcome;
        }
    };
    let target_items = match fetch_installed(&client, target_token) {
        Ok(items) => items,
        Err(_) => {
            outcome.aborted = true;
            return outcome;
        }
    };
    // 吸收集 = 源账号市场插件（builtin 与自装不入清单宇宙，ADR-0023）。
    let typed_source: Vec<CloudPluginItem> = source_items
        .iter()
        .filter_map(CloudPluginItem::from_raw)
        .collect();
    let absorbed: Vec<CloudPluginItem> = typed_source
        .iter()
        .filter(|item| !item.builtin && item.marketplace_plugin_id.is_some())
        .cloned()
        .collect();
    outcome.source_count = absorbed.len();
    outcome.skipped = typed_source
        .iter()
        .filter(|item| !item.builtin && item.marketplace_plugin_id.is_none())
        .count();
    outcome.absorbed = absorbed;

    let typed_target: Vec<CloudPluginItem> = target_items
        .iter()
        .filter_map(CloudPluginItem::from_raw)
        .collect();
    let (to_install, to_remove) = reconcile_plan(&outcome.absorbed, &typed_target);

    // 先装后卸：安装是切号主诉求（防目标账号插件市场被清空），卸载次之。
    for item in &to_install {
        let Some(id) = item.marketplace_plugin_id.as_deref() else {
            continue;
        };
        let raw = cloud_item_install_source(item);
        match install_one(&client, source_token, target_token, &raw, id, &item.registry) {
            Ok(true) => outcome.installed += 1,
            Ok(false) => outcome.failed += 1,
            Err(_) => outcome.failed += 1,
        }
        std::thread::sleep(INSTALL_INTERVAL);
    }
    for item in &to_remove {
        match uninstall_cloud_plugin(target_token, &item.record_id) {
            Ok(true) => outcome.removed += 1,
            Ok(false) => outcome.failed += 1,
            Err(_) => outcome.failed += 1,
        }
        std::thread::sleep(INSTALL_INTERVAL);
    }
    outcome
}

/// 对账计划（纯函数，预检与执行共用）：吸收后清单 = 源账号市场插件；
/// install = 源有目标无；remove = 目标市场插件有而源无
/// （目标自装无市场 ID 与 builtin 条目不在同步宇宙，不动）。
pub fn reconcile_plan(
    source: &[CloudPluginItem],
    target: &[CloudPluginItem],
) -> (Vec<CloudPluginItem>, Vec<CloudPluginItem>) {
    let source_ids: HashSet<&str> = source
        .iter()
        .filter_map(|item| item.marketplace_plugin_id.as_deref())
        .collect();
    let target_ids: HashSet<&str> = target
        .iter()
        .filter_map(|item| item.marketplace_plugin_id.as_deref())
        .collect();
    let install = source
        .iter()
        .filter(|item| {
            item.marketplace_plugin_id
                .as_deref()
                .is_some_and(|id| !target_ids.contains(id))
        })
        .cloned()
        .collect();
    let remove = target
        .iter()
        .filter(|item| {
            !item.builtin
                && item
                    .marketplace_plugin_id
                    .as_deref()
                    .is_some_and(|id| !source_ids.contains(id))
        })
        .cloned()
        .collect();
    (install, remove)
}

/// 已装条目 → 安装体回退字段（install_one 的 item 参数形态）。
/// install_one 从 item 取 name/version/registry 等回退字段，从 detail
/// 取权威值；这里由定型条目还原成原始对象形态。
fn cloud_item_install_source(item: &CloudPluginItem) -> serde_json::Value {
    serde_json::json!({
        "marketplace_plugin_id": item.marketplace_plugin_id,
        "name": item.name,
        "display_name": item.display_name,
        "version": item.version,
        "registry": item.registry,
    })
}

/// 拉取账号云端已装插件列表（GET /api/remote/v1/plugins）。
fn fetch_installed(
    client: &reqwest::blocking::Client,
    token: &str,
) -> Result<Vec<serde_json::Value>, String> {
    let url = format!("{REMOTE_API_BASE}/api/remote/v1/plugins?page_size={PAGE_SIZE}");
    let response = client
        .get(&url)
        .header("Authorization", format!("Cloud-IDE-JWT {token}"))
        .header("Accept", "application/json")
        .header("X-Trae-Client-Type", "lite")
        .send()
        .map_err(|e| e.to_string())?;
    let status = response.status();
    let text = response.text().map_err(|e| e.to_string())?;
    if !status.is_success() {
        return Err(format!("HTTP {status}"));
    }
    let json: serde_json::Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    Ok(json
        .pointer("/data/items")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default())
}

/// 安装单个插件：取市场详情 → 构造请求体 → POST 到目标账号云端。
/// 返回 Ok(true)=成功，Ok(false)=业务失败（详情缺失/HTTP/业务码非 0）。
fn install_one(
    client: &reqwest::blocking::Client,
    source_token: &str,
    target_token: &str,
    item: &serde_json::Value,
    plugin_id: &str,
    registry: &str,
) -> Result<bool, String> {
    // 详情（安装体必填 uri=download_url）：拉取失败或无 data 视为插件不可装
    let detail = fetch_detail(client, source_token, plugin_id, registry)?;
    let Some(detail) = detail else {
        return Ok(false);
    };

    let body = build_install_body(item, &detail);
    let url = format!("{REMOTE_API_BASE}/api/remote/v1/plugins");
    let response = client
        .post(&url)
        .header("Authorization", format!("Cloud-IDE-JWT {target_token}"))
        .header("Accept", "application/json")
        .header("Content-Type", "application/json")
        .header("X-Trae-Client-Type", "lite")
        .json(&body)
        .send()
        .map_err(|e| e.to_string())?;
    let status = response.status();
    let text = response.text().map_err(|e| e.to_string())?;
    // 成功判定：HTTP 2xx 且业务码 code=0（与 SoloLiteApiObservability 实录一致）
    Ok(status.is_success() && text.contains("\"code\":0"))
}

/// 拉取市场插件详情（GET /extensions/api/-/plugin/detail）。
/// Ok(None) 表示响应无 data 字段（插件可能已下架）。
fn fetch_detail(
    client: &reqwest::blocking::Client,
    token: &str,
    plugin_id: &str,
    registry: &str,
) -> Result<Option<serde_json::Value>, String> {
    let url = format!("{MARKET_API_BASE}/extensions/api/-/plugin/detail");
    let response = client
        .get(url)
        .query(&[("plugin_id", plugin_id), ("registry", registry)])
        .header("Authorization", format!("Cloud-IDE-JWT {token}"))
        .header("Accept", "application/json")
        .header("X-Trae-Client-Type", "lite")
        .send()
        .map_err(|e| e.to_string())?;
    let status = response.status();
    let text = response.text().map_err(|e| e.to_string())?;
    if !status.is_success() {
        return Err(format!("HTTP {status}"));
    }
    let json: serde_json::Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    Ok(json.get("data").cloned())
}

/// 构造安装请求体：与 solo-lite 前端 m() 构造器字段逐一对齐，
/// 并附 bN() 产品参数（product_name/chat_mode/os；client_version 缺省省略）。
/// detail 为详情响应 data 原始对象（snake_case 直取，无需 camelCase 转换）。
fn build_install_body(item: &serde_json::Value, detail: &serde_json::Value) -> serde_json::Value {
    let plugin = detail.get("plugin").cloned().unwrap_or_default();
    let item_name = item_field(item, "name").unwrap_or_default();
    let item_version = item_field(item, "version").unwrap_or_else(|| "1.0.0".into());
    // 空串字段（connector_json 等在 raw 响应为 ""）按前端 void 0 语义省略
    let non_empty = |key: &str| {
        detail
            .get(key)
            .and_then(|f| f.as_str())
            .filter(|v| !v.is_empty())
            .map(|v| v.to_string())
    };
    let str_of = |v: &serde_json::Value, key: &str| {
        v.get(key).and_then(|f| f.as_str()).map(|s| s.to_string())
    };

    serde_json::json!({
        "marketplace_plugin_id": item_field(item, "marketplace_plugin_id"),
        "version": str_of(&plugin, "version").unwrap_or(item_version),
        "name": str_of(&plugin, "name").unwrap_or(item_name.clone()),
        "display_name": str_of(&plugin, "display_name")
            .or_else(|| item_field(item, "display_name"))
            .unwrap_or_else(|| item_name.clone()),
        "description": str_of(&plugin, "description").unwrap_or_default(),
        "uri": str_of(detail, "download_url").unwrap_or_default(),
        "file_size": detail.get("file_size").and_then(|f| f.as_i64()),
        "checksum": str_of(detail, "checksum"),
        "connector_json": non_empty("connector_json"),
        "mcp_servers_json": non_empty("mcp_servers_json"),
        "skills_json": non_empty("skills_json"),
        "manifest_json": non_empty("manifest_json"),
        "icon_url": str_of(detail, "icon_url")
            .or_else(|| str_of(&plugin, "icon_url"))
            .or_else(|| item_field(item, "icon_url")),
        "registry": str_of(&plugin, "registry")
            .or_else(|| item_field(item, "registry")),
        "origin_plugin_name": str_of(&plugin, "origin_plugin_name"),
        // bN() 产品参数包装（47144 模块：lite + windows）
        "product_name": "lite",
        "chat_mode": "code",
        "os": "windows",
    })
}

/// 取对象顶层字符串字段（空串视为缺失，与前端 String(x||"") 语义一致）。
fn item_field(item: &serde_json::Value, key: &str) -> Option<String> {
    item.get(key)
        .and_then(|f| f.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
}

// ============================================================================
// P5-8b 插件 tab 云端 API（ADR-0023）：已装列表定型视图、市场目录浏览、
// 云端卸载、按市场条目安装。协议事实同上（api_gap_probe 实测 2026-08-31）。
// ============================================================================

/// 云端已装条目（插件 tab 展示与对账用；原始 JSON 的定型视图）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CloudPluginItem {
    /// 不透明记录 ID（卸载键；实测形如 `9.C3RY_-ZGFYN-`）。
    pub record_id: String,
    /// 市场 UUID（同步/安装键）；None = 用户自装或内置，无法跨账号同步。
    pub marketplace_plugin_id: Option<String>,
    pub name: String,
    pub display_name: String,
    pub version: String,
    pub registry: String,
    /// `builtin:` 前缀条目 = 客户端内置，云端无记录，卸载/同步必须跳过。
    pub builtin: bool,
}

impl CloudPluginItem {
    /// 从云端列表原始条目定型；record_id 缺失视为脏数据（None）。
    pub fn from_raw(item: &serde_json::Value) -> Option<Self> {
        let record_id = item_field(item, "plugin_id")?;
        Some(Self {
            builtin: record_id.starts_with("builtin:"),
            record_id,
            marketplace_plugin_id: item_field(item, "marketplace_plugin_id"),
            name: item_field(item, "name").unwrap_or_default(),
            display_name: item_field(item, "display_name").unwrap_or_default(),
            version: item_field(item, "version").unwrap_or_default(),
            registry: item_field(item, "registry").unwrap_or_default(),
        })
    }
}

/// 拉取账号云端已装插件（定型视图；任一条目脏数据跳过不阻塞整表）。
pub fn fetch_installed_plugins(token: &str) -> Result<Vec<CloudPluginItem>, String> {
    let client = crate::checkin_http::trae_http_client();
    let raw = fetch_installed(&client, token)?;
    Ok(raw.iter().filter_map(CloudPluginItem::from_raw).collect())
}

/// 市场目录条目（GET /extensions/api/-/plugin/list，响应键 `data.plugins`）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MarketPluginItem {
    /// 市场 UUID（安装键，与 detail 端点的 plugin_id 同空间）。
    pub plugin_id: String,
    pub name: String,
    pub display_name: String,
    pub description: String,
    /// 实测记录未列 registry 字段：透传原始值（有则供安装用，无则空串）。
    pub registry: String,
    /// 条目全部分类键（原始 category_key 数组；含 "Featured" = 推荐）。
    pub categories: Vec<String>,
    /// 主分组分类键（首个非 Featured 分类；仅 Featured 时为 "Featured"；无分类为空串）。
    pub category_key: String,
    /// 主分组展示名（分类目录 i18n 中文优先；未知键回退键本身；无分类为空串）。
    pub category_name: String,
}

/// 市场分类目录条目（GET /extensions/api/-/plugin/categories，响应 data.categories）。
#[derive(Clone, Debug)]
struct MarketCategory {
    category_key: String,
    /// 展示名（i18n 的 zh / zh-cn 优先，回退 display_name）。
    display_name: String,
    /// 分组排序权重（服务端语义，降序展示）。
    weight: i64,
}

/// 解析分类目录响应（tests 复用；响应形状见 marketpeek 探针实录 2026-09-01）。
fn market_categories_from_json(json: &serde_json::Value) -> Vec<MarketCategory> {
    let entries = json
        .pointer("/data/categories")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    entries
        .iter()
        .filter_map(|entry| {
            // status 字段存在且非 ACTIVE 的分类不展示（与服务端语义一致）。
            if let Some(status) = entry.get("status").and_then(|v| v.as_str()) {
                if status != "ACTIVE" {
                    return None;
                }
            }
            let category_key = item_field(entry, "category_key")?;
            // 中文优先：i18n.zh → i18n.zh-cn → display_name。
            let display_name = ["zh", "zh-cn"]
                .iter()
                .find_map(|lang| {
                    entry
                        .pointer(&format!("/i18n/{lang}"))
                        .and_then(|v| v.as_str())
                        .filter(|s| !s.is_empty())
                })
                .map(|s| s.to_string())
                .or_else(|| item_field(entry, "display_name"))
                .unwrap_or_else(|| category_key.clone());
            Some(MarketCategory {
                category_key,
                display_name,
                weight: entry.get("weight").and_then(|v| v.as_i64()).unwrap_or(-1),
            })
        })
        .collect()
}

/// 拉取市场分类目录（只读；调用方失败可降级为空目录继续浏览）。
fn fetch_market_categories(
    client: &reqwest::blocking::Client,
    token: &str,
) -> Result<Vec<MarketCategory>, String> {
    let url = format!("{MARKET_API_BASE}/extensions/api/-/plugin/categories");
    let response = client
        .get(&url)
        .header("Authorization", format!("Cloud-IDE-JWT {token}"))
        .header("Accept", "application/json")
        .header("X-Trae-Client-Type", "lite")
        .send()
        .map_err(|e| e.to_string())?;
    let status = response.status();
    let text = response.text().map_err(|e| e.to_string())?;
    if !status.is_success() {
        return Err(format!("HTTP {status}"));
    }
    let json: serde_json::Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    Ok(market_categories_from_json(&json))
}

/// 单条市场条目解析 + 主分组归类（tests 复用）。
/// 主分组 = 首个非 "Featured" 分类（Featured 是推荐标记而非作用分类）；
/// 仅 Featured 或无分类时归 Featured / 空键（前端占位「其他」）。
fn market_item_from_raw(
    item: &serde_json::Value,
    categories: &[MarketCategory],
) -> Option<MarketPluginItem> {
    let plugin_id = item_field(item, "plugin_id")?;
    let raw_categories: Vec<String> = item
        .get("categories")
        .and_then(|v| v.as_array())
        .map(|list| {
            list.iter()
                .filter_map(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string())
                .collect()
        })
        .unwrap_or_default();
    let category_key = raw_categories
        .iter()
        .find(|key| key.as_str() != "Featured")
        .or_else(|| raw_categories.first())
        .cloned()
        .unwrap_or_default();
    let category_name = categories
        .iter()
        .find(|c| c.category_key == category_key)
        .map(|c| c.display_name.clone())
        // 未知分类键：回退键本身（比丢弃分组信息更可恢复）。
        .unwrap_or_else(|| category_key.clone());
    Some(MarketPluginItem {
        plugin_id,
        name: item_field(item, "name").unwrap_or_default(),
        display_name: item_field(item, "display_name").unwrap_or_default(),
        description: item_field(item, "description").unwrap_or_default(),
        registry: item_field(item, "registry").unwrap_or_default(),
        categories: raw_categories,
        category_key,
        category_name,
    })
}

/// 浏览插件市场目录（page_token 游标翻页，拉全量）。
///
/// 分页协议（2026-09-02 marketpage 探针实测）：响应 `data` 含
/// `total`/`next_page_token`；翻页参数名为 `page_token`（取上页
/// `next_page_token` 值），`page`/`page_num`/`cursor`/`offset` 均无效。
/// 此前单页 page_size=50 只拿到首页 45 条（全量 155），导致工具内
/// 市场条目远少于 TRAE Work 实际。
///
/// 条目按主分组分类权重降序排列（同组保持服务端原始顺序），前端按
/// category_name 相邻分组渲染；分类目录拉取失败降级为无分组（fail-soft，
/// 市场浏览本身不受影响）。
pub fn fetch_market_plugins(token: &str) -> Result<Vec<MarketPluginItem>, String> {
    let client = crate::checkin_http::trae_http_client();
    let url = format!("{MARKET_API_BASE}/extensions/api/-/plugin/list");
    // 翻页安全上限：防服务端异常时游标循环不终止（155 条 / 50 每页 ≈ 4 页）。
    const MAX_PAGES: usize = 20;
    let mut all_plugins: Vec<serde_json::Value> = Vec::new();
    let mut seen_ids: HashSet<String> = HashSet::new();
    let mut page_token: Option<String> = None;
    for _ in 0..MAX_PAGES {
        let mut query: Vec<(&str, &str)> = vec![("page_size", "50")];
        if let Some(token_value) = page_token.as_deref() {
            query.push(("page_token", token_value));
        }
        let response = client
            .get(&url)
            .query(&query)
            .header("Authorization", format!("Cloud-IDE-JWT {token}"))
            .header("Accept", "application/json")
            .header("X-Trae-Client-Type", "lite")
            .send()
            .map_err(|e| e.to_string())?;
        let status = response.status();
        let text = response.text().map_err(|e| e.to_string())?;
        if !status.is_success() {
            return Err(format!("HTTP {status}"));
        }
        let json: serde_json::Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
        let plugins = json
            .pointer("/data/plugins")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        // 按 plugin_id 去重后累积（防御服务端游标回退导致的重复页）。
        for plugin in plugins {
            if let Some(id) = plugin.get("plugin_id").and_then(|v| v.as_str()) {
                if !seen_ids.insert(id.to_string()) {
                    continue;
                }
            }
            all_plugins.push(plugin);
        }
        // 无 next_page_token 即到最后一页。
        page_token = json
            .pointer("/data/next_page_token")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string());
        if page_token.is_none() {
            break;
        }
    }
    // 分类目录是分组展示的增强信息：失败不阻断市场浏览（条目归「其他」）。
    let categories = fetch_market_categories(&client, token).unwrap_or_default();
    let mut items: Vec<MarketPluginItem> = all_plugins
        .iter()
        .filter_map(|item| market_item_from_raw(item, &categories))
        .collect();
    items.sort_by_key(|item| {
        let weight = categories
            .iter()
            .find(|c| c.category_key == item.category_key)
            .map(|c| c.weight)
            .unwrap_or(-1);
        -weight
    });
    Ok(items)
}

/// 云端卸载（DELETE /api/remote/v1/plugins/:record_id，api_gap_probe 实测）。
/// record_id 必须用列表项的不透明记录 ID（市场 UUID 是 404）；builtin 条目
/// 由调用方跳过（云端无记录，恒 404）。
pub fn uninstall_cloud_plugin(token: &str, record_id: &str) -> Result<bool, String> {
    let client = crate::checkin_http::trae_http_client();
    // 探针实录：bN 遥测字段经传输层作为 query 参数附加；冒号需 URL 编码。
    let encoded = record_id.replace(':', "%3A");
    let url = format!("{REMOTE_API_BASE}/api/remote/v1/plugins/{encoded}");
    let response = client
        .delete(&url)
        .query(&[
            ("product_name", "lite"),
            ("chat_mode", "code"),
            ("os", "windows"),
        ])
        .header("Authorization", format!("Cloud-IDE-JWT {token}"))
        .header("Accept", "application/json")
        .header("X-Trae-Client-Type", "lite")
        .send()
        .map_err(|e| e.to_string())?;
    let status = response.status();
    let text = response.text().map_err(|e| e.to_string())?;
    Ok(status.is_success() && text.contains("\"code\":0"))
}

/// 按市场条目安装到当前账号云端（install_one：详情 → 构造安装体 → POST；
/// 详情缺失/HTTP/业务码非 0 → Ok(false)）。
pub fn install_market_plugin(token: &str, plugin: &MarketPluginItem) -> Result<bool, String> {
    let client = crate::checkin_http::trae_http_client();
    // 市场条目 → 已装条目形态（install_one 安装体的回退字段来源）。
    let item = serde_json::json!({
        "marketplace_plugin_id": plugin.plugin_id,
        "name": plugin.name,
        "display_name": plugin.display_name,
        "registry": plugin.registry,
    });
    install_one(&client, token, token, &item, &plugin.plugin_id, &plugin.registry)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// 详情响应 data 样例（renderer.log "getPluginDetail raw result" 原样转录，
    /// manifest_json 等长文本截短不影响断言语义）。
    fn sample_detail() -> serde_json::Value {
        json!({
            "plugin": {
                "plugin_id": "42ef9a5d-9586-4445-93e8-37155ac24caa",
                "name": "trae-remote-official:seedream",
                "origin_plugin_name": "seedream",
                "display_name": "seedream",
                "description": "AI image generation plugin powered by Seedream.",
                "version": "1.0.1",
                "registry": "trae-remote-official",
                "icon_url": "https://example/icon.svg"
            },
            "changelog": "",
            "download_url": "https://example/package.zip",
            "manifest_json": "{\"name\":\"seedream\"}",
            "connector_json": "",
            "mcp_servers_json": "",
            "skills_json": "",
            "icon_url": "https://example/icon.svg",
            "checksum": "28b43e3166eb1e3ea236ce5b8766b1d6bb28dec6508401858702c44442a9d01a",
            "file_size": 962
        })
    }

    fn sample_item() -> serde_json::Value {
        json!({
            "plugin_id": "111",
            "marketplace_plugin_id": "42ef9a5d-9586-4445-93e8-37155ac24caa",
            "name": "trae-remote-official:seedream",
            "display_name": "seedream",
            "version": "1.0.1",
            "registry": "trae-remote-official",
            "enabled": true
        })
    }

    #[test]
    fn install_body_matches_frontend_constructor() {
        let body = build_install_body(&sample_item(), &sample_detail());
        // 与 m() 构造器逐字段对齐（288.e82102fe.mjs @4512333）
        assert_eq!(body["marketplace_plugin_id"], "42ef9a5d-9586-4445-93e8-37155ac24caa");
        assert_eq!(body["version"], "1.0.1");
        assert_eq!(body["name"], "trae-remote-official:seedream");
        assert_eq!(body["uri"], "https://example/package.zip");
        assert_eq!(body["file_size"], 962);
        assert_eq!(body["origin_plugin_name"], "seedream");
        // 空串字段按 void 0 语义省略
        assert!(body["connector_json"].is_null());
        assert!(body["manifest_json"].is_string());
        // bN() 产品参数
        assert_eq!(body["product_name"], "lite");
        assert_eq!(body["os"], "windows");
    }

    #[test]
    fn install_body_falls_back_to_item_fields() {
        // 详情缺 plugin 子对象时回退已装列表条目字段（与 m() 的 e.* 回退一致）
        let detail = json!({"download_url": "https://example/pkg.zip", "checksum": "abc", "file_size": 1});
        let body = build_install_body(&sample_item(), &detail);
        assert_eq!(body["name"], "trae-remote-official:seedream");
        assert_eq!(body["version"], "1.0.1");
        assert_eq!(body["registry"], "trae-remote-official");
        assert_eq!(body["uri"], "https://example/pkg.zip");
    }

    #[test]
    fn item_field_treats_empty_as_missing() {
        let item = json!({"marketplace_plugin_id": "", "name": "x"});
        assert!(item_field(&item, "marketplace_plugin_id").is_none());
        assert_eq!(item_field(&item, "name").as_deref(), Some("x"));
    }

    #[test]
    fn cloud_item_from_raw_marks_builtin_and_market_id() {
        // 市场插件：有不透明记录 ID + 市场 UUID。
        let market = CloudPluginItem::from_raw(&sample_item()).unwrap();
        assert_eq!(market.record_id, "111");
        assert_eq!(
            market.marketplace_plugin_id.as_deref(),
            Some("42ef9a5d-9586-4445-93e8-37155ac24caa")
        );
        assert!(!market.builtin);
        // builtin 条目：plugin_id 前缀 builtin:，无市场 UUID。
        let builtin = CloudPluginItem::from_raw(&json!({
            "plugin_id": "builtin:trae-remote-official:lark",
            "name": "trae-remote-official:lark",
            "display_name": "lark"
        }))
        .unwrap();
        assert!(builtin.builtin);
        assert_eq!(builtin.marketplace_plugin_id, None);
        // 无 record_id = 脏数据，跳过。
        assert!(CloudPluginItem::from_raw(&json!({"name": "x"})).is_none());
    }

    /// 分类目录样例（marketpeek 探针实录 2026-09-01，截取 3 条）。
    fn sample_categories() -> serde_json::Value {
        json!({
            "data": {"categories": [
                {"category_key": "Featured", "display_name": "Featured",
                 "i18n": {"en": "Featured", "ja": "おすすめ", "zh": "推荐"},
                 "weight": 1000, "status": "ACTIVE"},
                {"category_key": "development_tools", "display_name": "Developer Tools",
                 "i18n": {"en": "Developer Tools", "ja": "開発ツール", "zh": "开发工具"},
                 "weight": 600, "status": "ACTIVE"},
                {"category_key": "content_creation", "display_name": "content creation",
                 "i18n": {"zh-cn": "内容创作"}, "weight": 700, "status": "INACTIVE"}
            ]}
        })
    }

    #[test]
    fn market_categories_parse_zh_name_and_skip_inactive() {
        let categories = market_categories_from_json(&sample_categories());
        // INACTIVE 分类被过滤；zh 优先于 display_name。
        assert_eq!(categories.len(), 2);
        assert_eq!(categories[0].display_name, "推荐");
        assert_eq!(categories[1].display_name, "开发工具");
        assert_eq!(categories[1].weight, 600);
    }

    #[test]
    fn market_item_primary_category_skips_featured() {
        let categories = market_categories_from_json(&sample_categories());
        // Featured + 真实分类并存：主分组取非 Featured 分类，Featured 保留在全集。
        let item = market_item_from_raw(
            &json!({
                "plugin_id": "d2f9993a", "display_name": "高质量 PPT",
                "categories": ["Featured", "content_creation"]
            }),
            &categories,
        )
        .unwrap();
        assert_eq!(item.category_key, "content_creation");
        // 未知分类键（目录缺失或 INACTIVE）：回退键本身。
        assert_eq!(item.category_name, "content_creation");
        assert!(item.categories.contains(&"Featured".to_string()));
        // 已知分类：解析为中文展示名。
        let github = market_item_from_raw(
            &json!({
                "plugin_id": "b679bee3", "display_name": "GitHub",
                "categories": ["development_tools"]
            }),
            &categories,
        )
        .unwrap();
        assert_eq!(github.category_key, "development_tools");
        assert_eq!(github.category_name, "开发工具");
        // 无分类条目：主分组为空（前端归「其他」）。
        let bare = market_item_from_raw(
            &json!({"plugin_id": "x", "display_name": "y"}),
            &categories,
        )
        .unwrap();
        assert_eq!(bare.category_key, "");
        assert_eq!(bare.category_name, "");
    }

    #[test]
    fn market_item_featured_only_uses_featured_group() {
        let categories = market_categories_from_json(&sample_categories());
        let item = market_item_from_raw(
            &json!({"plugin_id": "x", "categories": ["Featured"]}),
            &categories,
        )
        .unwrap();
        // 仅 Featured：归推荐分组（避免无分组可挂）。
        assert_eq!(item.category_key, "Featured");
        assert_eq!(item.category_name, "推荐");
    }

    /// 构造云端已装条目（对账计划测试用）。
    fn installed_item(record_id: &str, market_id: Option<&str>) -> CloudPluginItem {
        CloudPluginItem {
            record_id: record_id.to_string(),
            marketplace_plugin_id: market_id.map(|id| id.to_string()),
            name: format!("plugin:{record_id}"),
            display_name: record_id.to_string(),
            version: "1.0.0".to_string(),
            registry: "trae-remote-official".to_string(),
            builtin: record_id.starts_with("builtin:"),
        }
    }

    #[test]
    fn reconcile_plan_install_and_remove_symmetric_difference() {
        // 源有 A/B/C，目标有 B/C/D → 装 A、卸 D，交集 B/C 不动。
        let source = vec![
            installed_item("r1", Some("uuid-a")),
            installed_item("r2", Some("uuid-b")),
            installed_item("r3", Some("uuid-c")),
        ];
        let target = vec![
            installed_item("r9", Some("uuid-b")),
            installed_item("r8", Some("uuid-c")),
            installed_item("r7", Some("uuid-d")),
        ];
        let (install, remove) = reconcile_plan(&source, &target);
        assert_eq!(install.len(), 1);
        assert_eq!(install[0].marketplace_plugin_id.as_deref(), Some("uuid-a"));
        assert_eq!(remove.len(), 1);
        assert_eq!(remove[0].marketplace_plugin_id.as_deref(), Some("uuid-d"));
        // 完全一致 → 无差异（切号弹层静默直过的前提）。
        let (none, none2) = reconcile_plan(&source, &source);
        assert!(none.is_empty() && none2.is_empty());
    }

    #[test]
    fn reconcile_plan_excludes_builtin_and_self_installed_from_universe() {
        let source = vec![
            // 自装（无市场 ID）：不进安装集。
            installed_item("r-self", None),
            // builtin：不在同步宇宙。
            installed_item("builtin:trae-remote-official:lark", None),
            installed_item("r1", Some("uuid-a")),
        ];
        let target = vec![
            // 目标自装：不被移除（清单宇宙外，动它属意外破坏）。
            installed_item("r-target-self", None),
            // 目标 builtin：不可卸载（云端无记录）。
            installed_item("builtin:trae-remote-official:seed", None),
            installed_item("r7", Some("uuid-d")),
        ];
        let (install, remove) = reconcile_plan(&source, &target);
        assert_eq!(install.len(), 1);
        assert_eq!(install[0].marketplace_plugin_id.as_deref(), Some("uuid-a"));
        assert_eq!(remove.len(), 1);
        assert_eq!(remove[0].marketplace_plugin_id.as_deref(), Some("uuid-d"));
    }

    #[test]
    fn install_source_carries_market_fields_for_install_body() {
        // 定型条目还原的安装源字段齐备（install_one 构造安装体的回退来源）。
        let raw = cloud_item_install_source(&installed_item("r1", Some("uuid-a")));
        assert_eq!(raw["marketplace_plugin_id"], "uuid-a");
        assert_eq!(raw["name"], "plugin:r1");
        assert_eq!(raw["version"], "1.0.0");
        assert_eq!(raw["registry"], "trae-remote-official");
    }
}
