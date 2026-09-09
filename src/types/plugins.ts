/**
 * P5-8b 插件 tab 类型（get_plugin_tab_state / browse_plugin_market /
 * install_plugin / uninstall_plugin_everywhere 契约，ADR-0026 实时同步）。
 * 字段与 src-tauri/src/lib.rs 的 InstalledPluginDto / ManifestPluginDto /
 * PluginTabStateDto / PluginUninstallEverywhereDto 和 infrastructure 的
 * MarketPluginItem（serde 命名）一致。
 */

/** 云端已装条目（含清单归属标记，供「切号保留 / 仅此账号」标签）。 */
export interface InstalledPluginDto {
  /** 不透明记录 ID（卸载键；实测形如 `9.C3RY_-ZGFYN-`）。 */
  readonly record_id: string;
  /** 市场 UUID；null = 用户自装或客户端内置，无法跨账号同步。 */
  readonly marketplace_plugin_id: string | null;
  readonly name: string;
  readonly display_name: string;
  readonly version: string;
  readonly registry: string;
  /** builtin 条目 = 客户端内置，云端无记录，不可卸载/同步。 */
  readonly builtin: boolean;
  /** 条目（按市场 UUID）是否已在环境清单内。 */
  readonly in_manifest: boolean;
}

/** 环境清单条目（含云端在装标记）。 */
export interface ManifestPluginDto {
  readonly marketplace_plugin_id: string;
  readonly name: string;
  readonly display_name: string;
  readonly version: string;
  readonly registry: string;
  /** 清单条目在当前账号云端是否已装。 */
  readonly installed_in_cloud: boolean;
}

/** 插件 tab 状态（get_plugin_tab_state 返回）。 */
export interface PluginTabStateDto {
  readonly installed: readonly InstalledPluginDto[];
  readonly manifest: readonly ManifestPluginDto[];
  /** 工具已知账号数（含当前；卸载确认「将同时从 N 个账号移除」的 N）。 */
  readonly known_account_count: number;
}

/** 插件市场目录条目（browse_plugin_market 返回；单页只读）。 */
export interface MarketPluginItem {
  /** 市场 UUID（安装键）。 */
  readonly plugin_id: string;
  readonly name: string;
  readonly display_name: string;
  readonly description: string;
  readonly registry: string;
  /** 条目全部分类键（含 "Featured" = 推荐）。 */
  readonly categories: readonly string[];
  /** 主分组分类键（首个非 Featured 分类；无分类为空串）。 */
  readonly category_key: string;
  /** 主分组展示名（分类目录中文优先；无分类为空串，前端归「其他」）。 */
  readonly category_name: string;
}

/** uninstall_plugin_everywhere 逐账号回执（ADR-0026 决策 2）。 */
export interface PluginUninstallAccountDto {
  /** 账号展示名（备注名优先；内部 ID 不进主视野）。 */
  readonly display_name: string;
  /** true = 已从该账号云端移除。 */
  readonly removed: boolean;
  /** true = 移除失败（fail-soft：只记失败不中断其他账号）。 */
  readonly failed: boolean;
  /** 失败原因码（成功为 null；技术细节收悬浮提示）。 */
  readonly error_code: string | null;
}

/** uninstall_plugin_everywhere 返回：逐账号成败回执。 */
export interface PluginUninstallEverywhereDto {
  readonly accounts: readonly PluginUninstallAccountDto[];
}
