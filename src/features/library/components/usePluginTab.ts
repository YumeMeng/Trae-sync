import { useCallback, useEffect, useLayoutEffect, useRef, useState } from "react";
import { browsePluginMarket, readPluginTabState } from "../../../platform/libraryPlugins";
import { notify } from "../../../platform/notify";
import { safeUiErrorMessage } from "../../../utils/safeUiError";
import type { MarketPluginItem, PluginTabStateDto } from "../../../types/plugins";
import { LIBRARY_TEXTS, type PluginRowView, type PluginSegmentId } from "../config";
import { usePluginActions, type PluginUninstallOrder } from "./usePluginActions";
import type { PluginTabData } from "../pluginTab";

/**
 * 插件 tab 读取与动作接线（P4e-8b-3）：已装状态 + 市场目录两路读取、段/搜索/分类本地态、安装（零确认）与
 * 全账号卸载（preview → 确认层 → runUninstall）。旧实现逐条对照 src/components/PluginWorkbench.tsx（下称 P）：
 * P101-139 两路读取与 active 门禁、P141-195 安装与卸载、P316-321 段切换重置筛选、P370-381 刷新两路。
 * 命令经 platform/libraryPlugins（本文件零 invoke）；读写编排、回执与失败通知归 usePluginActions，本文件只持本地态与动作 id 判别。
 * 门禁：enabled（能力表 workbench + 主库 + 非只读库）与 active（插件 tab 当前可见）双开才发 IPC——旧页「tab 激活才读取」口径。
 */
export interface PluginTabInput {
  /** 静态门禁：该库有插件工作台（能力表 + 库形态），由 pluginTab.pluginTabEnabled 判定。 */
  readonly enabled: boolean;
  /** 插件 tab 当前可见（页面 active 且 tab=plugins）：不可见不发起请求。 */
  readonly active: boolean;
}
export interface PluginTabResult {
  readonly data: PluginTabData;
  readonly busy: boolean;
  /** 工具行动作出口（plugin-refresh）：命中返回 true。 */
  readonly handleAction: (actionId: string) => boolean;
  readonly chooseSegment: (segment: PluginSegmentId) => void;
  readonly search: (value: string) => void;
  readonly chooseCategory: (category: string | null) => void;
  /** 行内动作：安装直接执行（零确认）；卸载只产单并开确认层。 */
  readonly rowAction: (row: PluginRowView) => void;
  readonly cancelUninstall: () => void;
  readonly confirmUninstall: () => void;
}
const INSTALL_PREFIX = "install-", UNINSTALL_PREFIX = "uninstall-";

export function usePluginTab(input: PluginTabInput): PluginTabResult {
  const { enabled, active } = input;
  const [segment, setSegment] = useState<PluginSegmentId>("installed");
  const [searchText, setSearchText] = useState("");
  const [category, setCategory] = useState<string | null>(null);
  const [state, setState] = useState<PluginTabStateDto | null>(null);
  const [market, setMarket] = useState<readonly MarketPluginItem[] | null>(null);
  const [marketError, setMarketError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const [uninstall, setUninstall] = useState<PluginUninstallOrder | null>(null);
  // 最新可见性 / 已取回数据经 ref 取：动作身份稳定，且执行期不读旧闭包。
  const latest = useRef({ enabled, active, state, market }); latest.current = { enabled, active, state, market };
  const stateTicket = useRef(0), marketTicket = useRef(0);
  const loadState = useCallback(async (): Promise<void> => {
    if (!latest.current.enabled || !latest.current.active) return;
    const ticket = ++stateTicket.current; setLoading(true);
    try {
      const next = await readPluginTabState();
      if (ticket === stateTicket.current) setState(next);
    } catch (reason: unknown) {
      // 已装状态读取失败：通知一次，保留上次清单（旧 P106-110 口径，不清已取回数据）。
      if (ticket === stateTicket.current) notify.error(safeUiErrorMessage(reason, LIBRARY_TEXTS.pluginStateFailure));
    } finally { if (ticket === stateTicket.current) setLoading(false); }
  }, []);
  const loadMarket = useCallback(async (): Promise<void> => {
    if (!latest.current.enabled || !latest.current.active) return;
    const ticket = ++marketTicket.current;
    try {
      const items = await browsePluginMarket();
      if (ticket === marketTicket.current) { setMarket(items); setMarketError(null); }
    } catch (reason: unknown) {
      // 市场目录是增强信息：失败不阻断已装列表（旧 P115-126 fail-soft）。
      if (ticket === marketTicket.current) setMarketError(safeUiErrorMessage(reason, LIBRARY_TEXTS.pluginMarketFailure));
    }
  }, []);
  const actions = usePluginActions({ refresh: loadState, knownAccountCount: state?.known_account_count ?? 1 });
  // 门禁一变即清态（换库 / 换产品后不残留他库清单）；可见性变化作废在途读取。
  useLayoutEffect(() => {
    stateTicket.current += 1; marketTicket.current += 1;
    setState(null); setMarket(null); setMarketError(null); setUninstall(null); setLoading(false);
  }, [enabled]);
  useEffect(() => {
    if (!enabled || !active) return;
    void loadState(); void loadMarket();
    return () => { stateTicket.current += 1; marketTicket.current += 1; }; // 卸载 / 切走：在途读取作废
  }, [enabled, active, loadState, loadMarket]);
  // 段切换重置筛选（P316-321）：两段分类口径不同，旧筛选易造成空列表困惑。
  const chooseSegment = useCallback((next: PluginSegmentId) => { setSegment(next); setCategory(null); setSearchText(""); }, []);
  const rowAction = useCallback((row: PluginRowView) => {
    if (row.action === null) return;
    const id = row.action.id;
    if (id.startsWith(INSTALL_PREFIX)) { // 安装：零确认，即时改云端并重读清单（ADR-0026 决策 1）
      const plugin = (latest.current.market ?? []).find((item) => item.plugin_id === id.slice(INSTALL_PREFIX.length));
      if (plugin !== undefined) void actions.install(plugin);
      return;
    }
    if (!id.startsWith(UNINSTALL_PREFIX)) return;
    const recordId = id.slice(UNINSTALL_PREFIX.length);
    const target = (latest.current.state?.installed ?? []).find((item) => item.record_id === recordId) ?? null;
    const order = actions.previewUninstall(target); // 内置条目 / 未找到即拒单（不弹空层）
    if (order !== null) setUninstall(order);
  }, [actions]);
  const cancelUninstall = useCallback(() => setUninstall(null), []);
  const confirmUninstall = useCallback(() => {
    const order = uninstall;
    setUninstall(null); // 旧 P169 同口径：确认即关层，执行期由忙碌位禁用全部行按钮
    void actions.runUninstall(order);
  }, [actions, uninstall]);
  const handleAction = useCallback((actionId: string): boolean => {
    if (actionId !== "plugin-refresh") return false;
    void loadState(); void loadMarket(); // 刷新两路（旧 P373-376）
    return true;
  }, [loadState, loadMarket]);
  const data: PluginTabData = { segment, searchText, category, state, market, marketError, loading, uninstall };
  return { data, busy: actions.busyKey !== null, handleAction, chooseSegment, search: setSearchText,
    chooseCategory: setCategory, rowAction, cancelUninstall, confirmUninstall };
}
