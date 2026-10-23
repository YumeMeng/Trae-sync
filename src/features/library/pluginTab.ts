/**
 * 插件 tab 视图 builder（P4e-8b-3）：原始 DTO（get_plugin_tab_state / browse_plugin_market）+ 本地段/搜索/分类/卸载单
 * → PluginTabView（工作台或说明卡）。旧实现逐条对照 src/components/PluginWorkbench.tsx（下称 P）与
 * src/pages/MasterLibraryDetail.tsx（下称 D）。
 * 纯映射：零 React / 零 JSX / 零 invoke / 零命令 / 零 platform 依赖；testid 逐字沿用旧锚（P230/P251/P270/P280/P292/P332…）。
 * 说明卡形态（D232-243）：只读库与副环境不参与插件管理——由库形态（libraryId / readOnly）决定，不由调用方猜。
 */
import type { InstalledPluginDto, MarketPluginItem, PluginTabStateDto } from "../../types/plugins";
import {
  LIBRARY_TEXTS, fillLibraryText, libraryCapabilitiesFor,
  type LibraryActionKind, type LibraryActionView, type LibraryBadgeView,
  type PluginRowView, type PluginSegmentId, type PluginTabView,
} from "./config";
import type { PluginUninstallOrder } from "./components/usePluginActions";

/** 插件 tab 的读取面 + 本地交互态（页面 hook 产出；builder 只消费）。 */
export interface PluginTabData {
  readonly segment: PluginSegmentId;
  readonly searchText: string;
  /** 分类筛选（null = 全部）；段切换时由 hook 重置（P316-321）。 */
  readonly category: string | null;
  readonly state: PluginTabStateDto | null;
  readonly market: readonly MarketPluginItem[] | null;
  readonly marketError: string | null;
  readonly loading: boolean;
  /** 卸载单（previewUninstall 产出；null = 无确认层）。 */
  readonly uninstall: PluginUninstallOrder | null;
}
/** 库形态：只有主库且非只读库才有插件工作台（D229-243）。 */
export interface PluginScope { readonly libraryId: string; readonly readOnly: boolean }
const MASTER_LIBRARY_ID = "master";
/** 插件工作台门禁（页面据此决定是否发起读取，不写产品分支）：能力表声明有工作台 + 库实例是主库。
 * 只读数据根与副环境库都到不了主库，故门禁一关即整块不读（旧 D229-243 三种形态）。 */
export function pluginTabEnabled(productId: string, libraryId: string): boolean {
  return libraryCapabilitiesFor(productId).pluginTab === "workbench" && libraryId === MASTER_LIBRARY_ID;
}
/** 未关联市场分类（自装/已下架/无分类）的归类展示名（P27）。 */
const UNCATEGORIZED = LIBRARY_TEXTS.pluginUncategorized;
/** 条目的分类展示名（空串归「其他」；P30-33）。 */
const categoryLabel = (name: string | undefined): string => (name?.trim() ? name.trim() : UNCATEGORIZED);
const action = (id: string, label: string, kind: LibraryActionKind, testId: string, hint: string | null = null,
  disabledReason: string | null = null, busyLabel: string | null = null): LibraryActionView => ({ id, label, kind, testId, hint, disabledReason, busyLabel });
const badge = (text: string, tone: LibraryBadgeView["tone"]): LibraryBadgeView => ({ text, tone, title: null });
const hoverTitleOf = (item: { readonly name: string; readonly registry: string }): string => item.registry
  ? fillLibraryText(LIBRARY_TEXTS.pluginHoverTitle, { name: item.name, registry: item.registry }) : fillLibraryText(LIBRARY_TEXTS.pluginHoverTitlePlain, { name: item.name });
/** 说明卡（D232-243）：只读数据根 / 副环境各一句，不渲染段、搜索、列表与确认层。 */
const noteOf = (scope: PluginScope): PluginTabView => ({
  kind: "note", ariaLabel: LIBRARY_TEXTS.pluginRegion, text: scope.readOnly ? LIBRARY_TEXTS.pluginsReadonlyNote : LIBRARY_TEXTS.pluginsSecondaryNote,
  testId: scope.readOnly ? "master-plugins-readonly-note" : "master-plugins-secondary-note",
});
/** 已装行（P228-256）：归属徽章三档 + 移除动作（内置禁用）。 */
const installedRowOf = (item: InstalledPluginDto, category: string | null): PluginRowView => ({
  id: item.record_id, testId: `plugin-row-${item.record_id}`, title: item.display_name || item.name,
  meta: item.version ? fillLibraryText(LIBRARY_TEXTS.pluginVersion, { version: item.version }) : null, category,
  badge: item.builtin ? badge(LIBRARY_TEXTS.pluginBadgeBuiltin, "neutral")
    : item.in_manifest ? badge(LIBRARY_TEXTS.pluginBadgeBound, "accent") : badge(LIBRARY_TEXTS.pluginBadgeDrift, "neutral"),
  hoverTitle: hoverTitleOf(item),
  action: action(`uninstall-${item.record_id}`, LIBRARY_TEXTS.pluginUninstall, "ghost", `plugin-uninstall-${item.record_id}`,
    LIBRARY_TEXTS.pluginUninstallHint, item.builtin ? LIBRARY_TEXTS.pluginBuiltinHint : null), // P244-255
});
/** 市场行（P266-298）：已装条目给徽章不给安装按钮（已安装徽章锚由展示层按行锚派生，P280）。 */
const marketRowOf = (item: MarketPluginItem, installedKeys: ReadonlySet<string>): PluginRowView => {
  const installed = installedKeys.has(item.plugin_id) || installedKeys.has(item.name);
  return {
    id: item.plugin_id, testId: `plugin-market-row-${item.plugin_id}`, title: item.display_name || item.name,
    meta: item.description || null, category: categoryLabel(item.category_name), hoverTitle: hoverTitleOf(item),
    badge: installed ? badge(LIBRARY_TEXTS.pluginBadgeInstalled, "accent") : null,
    action: installed ? null : action(`install-${item.plugin_id}`, LIBRARY_TEXTS.pluginInstall, "ghost", `plugin-install-${item.plugin_id}`),
  };
};
/** 分类 chips（P303-314）：当前段实际出现的分类，顺序保持数据顺序。 */
function categoriesOf(segment: PluginSegmentId, installed: readonly InstalledPluginDto[], market: readonly MarketPluginItem[] | null,
  installedCategory: (item: InstalledPluginDto) => string | null): readonly string[] {
  const seen: string[] = [];
  const push = (name: string) => { if (!seen.includes(name)) seen.push(name); };
  if (segment === "installed") for (const item of installed) push(installedCategory(item) ?? UNCATEGORIZED);
  else for (const item of market ?? []) push(categoryLabel(item.category_name));
  return seen;
}
/** 空态 / 加载句（P323-325、P409-431）：加载句无锚，空态句带旧锚。 */
function emptyOf(data: PluginTabData, rowCount: number): { readonly text: string; readonly testId: string | null } {
  if (data.segment === "installed") {
    if (data.loading && data.state === null) return { text: LIBRARY_TEXTS.pluginLoadingInstalled, testId: null };
    return !data.loading && data.state !== null && rowCount === 0 ? { text: LIBRARY_TEXTS.pluginEmptyInstalled, testId: "plugin-installed-empty" } : { text: "", testId: null };
  }
  if (data.market === null) return { text: data.marketError ?? LIBRARY_TEXTS.pluginLoadingMarket, testId: null };
  return rowCount === 0 ? { text: LIBRARY_TEXTS.pluginEmptyMarket, testId: "plugin-market-empty" } : { text: "", testId: null };
}
/** 插件 tab 视图：说明卡（非主库 / 只读库）或工作台（段 + 搜索 + 分类 + 行 + 空态 + 卸载单）。 */
export function buildPluginTabView(data: PluginTabData, scope: PluginScope): PluginTabView {
  if (scope.libraryId !== MASTER_LIBRARY_ID || scope.readOnly) return noteOf(scope);
  const installed = data.state?.installed ?? [], market = data.market, keyword = data.searchText.trim().toLowerCase();
  const marketById = new Map((market ?? []).map((item) => [item.plugin_id, item] as const));
  // 已装条目的分类关联（P205-212）：无市场关联即 null（分类列留空），不猜「其他」。
  const installedCategory = (item: InstalledPluginDto): string | null => {
    const entry = item.marketplace_plugin_id === null ? undefined : marketById.get(item.marketplace_plugin_id);
    return entry === undefined ? null : categoryLabel(entry.category_name);
  };
  const installedKeys = new Set(installed.flatMap((item) => [item.marketplace_plugin_id, item.name])
    .filter((key): key is string => key !== null)); // P215-219
  const rows = data.segment === "installed"
    ? installed.filter((item) => data.category === null || installedCategory(item) === data.category)
      .map((item) => installedRowOf(item, installedCategory(item)))
    : (market ?? []).filter((item) => data.category === null || categoryLabel(item.category_name) === data.category)
      .filter((item) => keyword === "" || [item.display_name, item.name, item.description].some((text) => text.toLowerCase().includes(keyword)))
      .map((item) => marketRowOf(item, installedKeys)); // P258-298
  const empty = emptyOf(data, rows.length), categories = categoriesOf(data.segment, installed, market, installedCategory);
  return {
    kind: "workbench", testId: "plugin-workbench", ariaLabel: LIBRARY_TEXTS.pluginRegion, // P328-333
    refresh: action("plugin-refresh", LIBRARY_TEXTS.pluginRefresh, "ghost", "plugin-refresh", LIBRARY_TEXTS.pluginRefreshHint), // P370-381
    segments: ([["installed", LIBRARY_TEXTS.pluginInstalled], ["market", LIBRARY_TEXTS.pluginMarket]] as const).map(([id, label]) => ({
      id, label, testId: `plugin-segment-${id}`, active: data.segment === id })), // P336-355
    activeSegment: data.segment,
    search: data.segment === "market" // P357-369：只有市场段有搜索
      ? { testId: "plugin-market-search", placeholder: LIBRARY_TEXTS.pluginSearch, value: data.searchText } : null,
    categories, activeCategory: data.category, rows,
    emptyText: empty.text === "" ? null : empty.text, emptyTestId: empty.testId,
    errorNote: data.segment === "market" && market !== null ? data.marketError : null, // P415-417
    uninstall: data.uninstall === null ? null : { title: LIBRARY_TEXTS.pluginUninstallTitle, // P447-460
      confirmLabel: LIBRARY_TEXTS.pluginUninstallConfirm, lines: data.uninstall.lines, testId: LIBRARY_TEXTS.pluginUninstallTestId },
  };
}
