import { useCallback, useRef, useState } from "react";
import { installPlugin, uninstallPluginEverywhere } from "../../../platform/libraryPlugins";
import { notify, type NotifyIssue } from "../../../platform/notify";
import type { InstalledPluginDto, MarketPluginItem, PluginUninstallEverywhereDto } from "../../../types/plugins";
import { LIBRARY_TEXTS, fillLibraryText } from "../config";
import { safeUiErrorMessage } from "../../../utils/safeUiError";

/**
 * 插件 tab 动作 hook（P4e-6）：安装（零确认）/ 全账号卸载（单次确认，确认层归调用方弹层）。
 * 命令经 platform/libraryPlugins（本文件零 invoke）；卸载的跨账号传播与 fail-soft 逐账号由后端承担，
 * 本 hook 只产出「卸载单」（确认文案里的 N = known_account_count）并按回执发批量摘要通知。
 * 防连点用 busyKey ref：同一 tick 内重复点击也拦得住，成功/失败都在 finally 收位；成功后由调用方重读。
 */

/** 一次安装 / 卸载的忙碌位：null = 空闲；非空 = 该插件键正在执行（页面据此禁用全部行按钮）。 */
export type PluginBusyKey = string | null;
/** 卸载单：只能由 previewUninstall 产出，执行侧不再重算文案与账号数（防确认层与执行层漂移）。 */
export interface PluginUninstallOrder {
  readonly recordId: string;
  readonly title: string;
  readonly lines: readonly string[];
}
export interface PluginActionsInput {
  /** 写后重读插件 tab（旧 P110-113 口径）；失败只追加警告，不翻转已成功的回执。 */
  readonly refresh: () => Promise<unknown>;
  /** 已知账号数（PluginTabStateDto.known_account_count）：卸载确认「将同时从 N 个账号移除」的 N。 */
  readonly knownAccountCount: number;
}
export interface PluginActionsResult {
  readonly busyKey: PluginBusyKey;
  readonly install: (plugin: MarketPluginItem) => Promise<boolean>;
  /** 生成卸载单（内置条目与空账号数一律拒绝下单）；确认弹层与文案由调用方渲染。 */
  readonly previewUninstall: (plugin: InstalledPluginDto | null) => PluginUninstallOrder | null;
  /** 执行卸载（order=null 即未确认/已取消）：全账号传播，逐账号成败走批量摘要。 */
  readonly runUninstall: (order: PluginUninstallOrder | null) => Promise<boolean>;
}
const titleOf = (plugin: { readonly display_name: string; readonly name: string }): string => plugin.display_name || plugin.name;

export function usePluginActions(input: PluginActionsInput): PluginActionsResult {
  const latest = useRef(input); latest.current = input; // 入参经 ref 取最新值：动作身份稳定
  const busyRef = useRef<PluginBusyKey>(null); // 防连点唯一真源（state 只驱动渲染）
  const [busyKey, setBusyKey] = useState<PluginBusyKey>(null);
  const mark = useCallback((key: PluginBusyKey) => { busyRef.current = key; setBusyKey(key); }, []);
  /** 写后重读：失败只警告（已完成的安装/卸载不得误报失败）。 */
  const reload = useCallback(async () => {
    try { await latest.current.refresh(); } catch { notify.warning(LIBRARY_TEXTS.refreshFailedAfterAction); }
  }, []);

  // 安装（ADR-0026 决策 1）：零确认，即时改当前账号云端并更新清单。
  const install = useCallback(async (plugin: MarketPluginItem): Promise<boolean> => {
    if (busyRef.current !== null) return false;
    mark(plugin.plugin_id);
    try {
      await installPlugin({ pluginId: plugin.plugin_id, name: plugin.name, displayName: plugin.display_name, registry: plugin.registry });
      notify.success(fillLibraryText(LIBRARY_TEXTS.pluginInstalledNotice, { name: titleOf(plugin) }));
      await reload();
      return true;
    } catch (reason: unknown) {
      notify.error(safeUiErrorMessage(reason, LIBRARY_TEXTS.pluginInstallFailure)); // 安装失败不改清单
      return false;
    } finally { mark(null); }
  }, [mark, reload]);

  // 卸载单（ADR-0026 决策 2）：N 取 known_account_count（缺省 1，绝不显示 0 个账号）。
  const previewUninstall = useCallback((plugin: InstalledPluginDto | null): PluginUninstallOrder | null => {
    if (plugin === null || plugin.builtin) return null; // 内置条目云端无记录，不可卸载
    const count = Math.max(latest.current.knownAccountCount, 1);
    return { recordId: plugin.record_id, title: titleOf(plugin),
      lines: [titleOf(plugin), fillLibraryText(LIBRARY_TEXTS.pluginUninstallLine, { count })] };
  }, []);

  // 全账号卸载：后端逐账号 fail-soft；回执走批量摘要（成功数 + 失败账号名），不静默吞失败。
  const runUninstall = useCallback(async (order: PluginUninstallOrder | null): Promise<boolean> => {
    if (order === null || busyRef.current !== null) return false;
    mark(order.recordId);
    try {
      const receipt: PluginUninstallEverywhereDto = await uninstallPluginEverywhere({ recordId: order.recordId });
      const issues: NotifyIssue[] = receipt.accounts.filter((item) => item.failed)
        .map((item) => ({ displayName: item.display_name, reason: LIBRARY_TEXTS.pluginAccountIssue }));
      notify.summary(fillLibraryText(LIBRARY_TEXTS.pluginRemovedNotice, { title: order.title }), issues,
        receipt.accounts.filter((item) => item.removed).length);
      await reload();
      return true;
    } catch (reason: unknown) {
      notify.error(safeUiErrorMessage(reason, LIBRARY_TEXTS.pluginUninstallFailure));
      return false;
    } finally { mark(null); }
  }, [mark, reload]);

  return { busyKey, install, previewUninstall, runUninstall };
}
