import { invoke } from "@tauri-apps/api/core";
import type {
  MarketPluginItem,
  PluginTabStateDto,
  PluginUninstallEverywhereDto,
} from "../types/plugins";

/**
 * 环境域插件命令的类型化封装（P4d-1 拆分件）：插件 tab（库详情 plugins tab
 * 与后续环境页入口共用）只引用本文件。只做 invoke 转接——不吞错误、不做
 * 跨账号编排、不改后端契约；单次确认（卸载传播）与失败文案由调用方负责。
 */

/** 读取插件 tab 状态（云端已装 + 环境清单 + 双向归属标记；清单缺失时后端自动导入）。 */
export function readPluginTabState(): Promise<PluginTabStateDto> {
  return invoke<PluginTabStateDto>("get_plugin_tab_state");
}

/** 浏览插件市场目录（只读单页；分类关联是增强信息，失败由调用方降级）。 */
export function browsePluginMarket(): Promise<MarketPluginItem[]> {
  return invoke<MarketPluginItem[]>("browse_plugin_market");
}

/** 安装市场插件到当前账号云端（零确认；入参为市场条目字段，pluginId = 市场 UUID）。 */
export function installPlugin(
  {
    pluginId,
    name,
    displayName,
    registry,
  }: {
    readonly pluginId: string;
    readonly name: string;
    readonly displayName: string;
    readonly registry: string;
  },
): Promise<void> {
  return invoke<void>("install_plugin", { pluginId, name, displayName, registry });
}

/** 卸载插件并传播到全部已知账号（后端 fail-soft 逐账号尽力）；返回逐账号成败回执。 */
export function uninstallPluginEverywhere(
  { recordId }: { readonly recordId: string },
): Promise<PluginUninstallEverywhereDto> {
  return invoke<PluginUninstallEverywhereDto>("uninstall_plugin_everywhere", { recordId });
}
