import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Package, Puzzle, RefreshCw, Search, Store, Trash2 } from "lucide-react";
import type {
  InstalledPluginDto,
  MarketPluginItem,
  PluginTabStateDto,
  PluginUninstallEverywhereDto,
} from "../types/plugins";
import { safeUiErrorMessage } from "../utils/safeUiError";

// ============================================================================
// G23 插件 tab 工作台（ADR-0023 清单 + ADR-0026 实时同步）：表格化
// 已装/市场 + 分类筛选 chips。策略：安装零确认（即时改云端 + 清单）；
// 卸载单次确认后全账号传播（将同时从 N 个账号移除）；切号对账静默应用
// （含移除才在切号弹层确认），对账条随策略移除。
// ============================================================================

/** 插件 tab 内部分段：已装（默认）/ 市场。 */
type PluginSegment = "installed" | "market";

/** 未关联市场分类（自装/已下架/无分类）条目的归类展示名。 */
const UNCATEGORIZED = "其他";

/** 条目的分类展示名（空串归「其他」；G23 复用市场 category_name）。 */
function categoryLabel(name: string | undefined): string {
  const trimmed = name?.trim();
  return trimmed ? trimmed : UNCATEGORIZED;
}

interface PluginWorkbenchProps {
  /** tab 可见时才读取；不可见不发起请求（与详情页其他分区同口径）。 */
  active: boolean;
}

/** 已装/市场共用行骨架（G23：密集行列表，行高约 40px）。 */
function PluginRow({
  title,
  meta,
  category,
  badge,
  action,
  rowTestId,
  hoverTitle,
}: {
  /** 主名称（展示名优先）。 */
  title: string;
  /** 次级元信息（版本 / 描述截断）。 */
  meta?: string;
  /** 分类名；null = 无市场关联（不渲染该列内容）。 */
  category: string | null;
  /** 归属标签（切号保留 / 仅此账号 / 内置 / 已安装）。 */
  badge?: React.ReactNode;
  /** 右侧操作（移除 / 安装按钮）。 */
  action?: React.ReactNode;
  rowTestId: string;
  /** 行悬浮提示（技术细节：插件标识/来源）。 */
  hoverTitle?: string;
}) {
  return (
    <div className="plugin-row" data-testid={rowTestId} title={hoverTitle}>
      <span className="plugin-row__icon" aria-hidden="true">
        <Puzzle size={16} />
      </span>
      <div className="plugin-row__main">
        <span className="plugin-row__name">{title}</span>
        {meta && <span className="plugin-row__meta">{meta}</span>}
      </div>
      <span className="plugin-row__category">{category ?? ""}</span>
      <div className="plugin-row__side">
        {badge}
        {action}
      </div>
    </div>
  );
}

export function PluginWorkbench({ active }: PluginWorkbenchProps) {
  const [state, setState] = useState<PluginTabStateDto | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const [segment, setSegment] = useState<PluginSegment>("installed");
  // 市场目录：已装段分类关联与市场段浏览共用（G23 顶部分类 chips 数据源），
  // tab 激活即拉取（只读），失败不阻断已装列表（分类列留空）。
  const [market, setMarket] = useState<MarketPluginItem[] | null>(null);
  const [marketError, setMarketError] = useState<string | null>(null);
  // 分类筛选（null = 全部）；segment 切换时重置。
  const [category, setCategory] = useState<string | null>(null);
  // 市场段搜索词（名称/描述包含匹配）。
  const [query, setQuery] = useState("");
  // 卸载确认弹层目标（ADR-0026 决策 2：单次确认 + 全账号传播）。
  const [pendingUninstall, setPendingUninstall] = useState<InstalledPluginDto | null>(null);
  // 装/卸进行中的条目键，防止重复点击。
  const [busyKey, setBusyKey] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const mountedRef = useRef(true);

  const loadState = useCallback(async () => {
    setLoading(true);
    try {
      const next = await invoke<PluginTabStateDto>("get_plugin_tab_state");
      if (!mountedRef.current) return;
      setState(next);
      setError(null);
    } catch (reason: unknown) {
      if (!mountedRef.current) return;
      setError(safeUiErrorMessage(reason, "插件状态暂时不可读取，请稍后重试。"));
    } finally {
      if (mountedRef.current) setLoading(false);
    }
  }, []);

  const loadMarket = useCallback(async () => {
    try {
      const items = await invoke<MarketPluginItem[]>("browse_plugin_market");
      if (!mountedRef.current) return;
      setMarket(items);
      setMarketError(null);
    } catch (reason: unknown) {
      if (!mountedRef.current) return;
      // 分类关联是增强信息：失败不阻断已装列表（fail-soft）。
      setMarketError(safeUiErrorMessage(reason, "插件市场暂时不可读取，请稍后重试。"));
    }
  }, []);

  useEffect(() => {
    mountedRef.current = true;
    return () => {
      mountedRef.current = false;
    };
  }, []);

  useEffect(() => {
    if (!active) return;
    void loadState();
    void loadMarket();
  }, [active, loadState, loadMarket]);

  /** 安装市场插件（ADR-0026 决策 1：零确认，即时改云端 + 更新清单）。 */
  const install = useCallback(async (plugin: MarketPluginItem) => {
    setBusyKey(plugin.plugin_id);
    setNotice(null);
    try {
      await invoke("install_plugin", {
        pluginId: plugin.plugin_id,
        name: plugin.name,
        displayName: plugin.display_name,
        registry: plugin.registry,
      });
      if (!mountedRef.current) return;
      setNotice(`已安装 ${plugin.display_name || plugin.name}。`);
      await loadState();
    } catch (reason: unknown) {
      if (!mountedRef.current) return;
      setError(safeUiErrorMessage(reason, "安装未完成，请稍后重试。"));
    } finally {
      if (mountedRef.current) setBusyKey(null);
    }
  }, [loadState]);

  /**
   * 确认卸载（ADR-0026 决策 2）：当前账号云端卸载 + 清单移除 + 其他已知
   * 账号云端逐个卸载（后端 fail-soft，逐账号尽力）。
   */
  const confirmUninstall = useCallback(async () => {
    if (!pendingUninstall) return;
    const target = pendingUninstall;
    setPendingUninstall(null);
    setBusyKey(target.record_id);
    setNotice(null);
    const pluginTitle = target.display_name || target.name;
    try {
      const receipt = await invoke<PluginUninstallEverywhereDto>(
        "uninstall_plugin_everywhere",
        { recordId: target.record_id },
      );
      if (!mountedRef.current) return;
      const removedCount = receipt.accounts.filter((item) => item.removed).length;
      const failedNames = receipt.accounts
        .filter((item) => item.failed)
        .map((item) => item.display_name);
      setNotice(
        failedNames.length > 0
          ? `已从 ${removedCount} 个账号移除 ${pluginTitle}；${failedNames.join("、")} 移除失败，可稍后重试。`
          : `已从 ${removedCount} 个账号移除 ${pluginTitle}。`,
      );
      await loadState();
    } catch (reason: unknown) {
      if (!mountedRef.current) return;
      setError(safeUiErrorMessage(reason, "移除未完成，请稍后重试。"));
    } finally {
      if (mountedRef.current) setBusyKey(null);
    }
  }, [pendingUninstall, loadState]);

  /** 市场条目索引（已装条目的分类关联用）。 */
  const marketById = useMemo(() => {
    const map = new Map<string, MarketPluginItem>();
    for (const item of market ?? []) map.set(item.plugin_id, item);
    return map;
  }, [market]);

  /** 已装条目 → 市场分类名（无关联返回 null，分类列留空）。 */
  const installedCategory = useCallback(
    (item: InstalledPluginDto): string | null => {
      if (!item.marketplace_plugin_id) return null;
      const entry = marketById.get(item.marketplace_plugin_id);
      return entry ? categoryLabel(entry.category_name) : null;
    },
    [marketById],
  );

  // 市场条目安装态：按市场 UUID（其次名称）与云端已装匹配。
  const installedKeys = new Set(
    (state?.installed ?? [])
      .flatMap((item) => [item.marketplace_plugin_id, item.name])
      .filter((key): key is string => key !== null),
  );

  /** 当前段经分类/搜索过滤后的行数据（已装/市场共用行骨架，切数据源）。 */
  const rows = useMemo(() => {
    if (segment === "installed") {
      const items = state?.installed ?? [];
      const filtered = category === null
        ? items
        : items.filter((item) => installedCategory(item) === category);
      return filtered.map((item) => ({
        key: item.record_id,
        testId: `plugin-row-${item.record_id}`,
        title: item.display_name || item.name,
        meta: item.version ? `版本 ${item.version}` : undefined,
        category: installedCategory(item),
        hoverTitle: item.registry
          ? `插件标识：${item.name} · 来源：${item.registry}`
          : `插件标识：${item.name}`,
        badge: item.builtin ? (
          <span className="plugin-badge">内置</span>
        ) : item.in_manifest ? (
          <span className="plugin-badge plugin-badge--bound">切号保留</span>
        ) : (
          <span className="plugin-badge plugin-badge--drift">仅此账号</span>
        ),
        action: (
          <button
            className="btn btn--quiet"
            type="button"
            onClick={() => setPendingUninstall(item)}
            disabled={item.builtin || busyKey !== null}
            title={item.builtin ? "内置插件，无需移除" : "从所有已登录账号移除该插件"}
            data-testid={`plugin-uninstall-${item.record_id}`}
          >
            <Trash2 size={14} aria-hidden="true" />移除
          </button>
        ),
      }));
    }
    const keyword = query.trim().toLowerCase();
    const items = (market ?? [])
      .filter((plugin) => category === null || categoryLabel(plugin.category_name) === category)
      .filter((plugin) => {
        if (!keyword) return true;
        return [plugin.display_name, plugin.name, plugin.description]
          .some((text) => text.toLowerCase().includes(keyword));
      });
    return items.map((plugin) => {
      const installed =
        installedKeys.has(plugin.plugin_id) || installedKeys.has(plugin.name);
      return {
        key: plugin.plugin_id,
        testId: `plugin-market-row-${plugin.plugin_id}`,
        title: plugin.display_name || plugin.name,
        meta: plugin.description || undefined,
        category: categoryLabel(plugin.category_name),
        hoverTitle: plugin.registry
          ? `插件标识：${plugin.name} · 来源：${plugin.registry}`
          : `插件标识：${plugin.name}`,
        badge: installed ? (
          <span
            className="plugin-badge plugin-badge--bound"
            data-testid={`plugin-market-installed-${plugin.plugin_id}`}
          >
            已安装
          </span>
        ) : undefined,
        action: installed ? undefined : (
          <button
            className="btn"
            type="button"
            onClick={() => void install(plugin)}
            disabled={busyKey !== null}
            data-testid={`plugin-install-${plugin.plugin_id}`}
          >
            安装
          </button>
        ),
      };
    });
    // installedKeys/busyKey 随渲染重建，依赖 state/market/install 即可覆盖。
  }, [segment, state, market, category, query, install, busyKey, installedCategory]);

  /** 分类 chips：当前段实际出现的分类（顺序保持数据顺序）。 */
  const categories = useMemo(() => {
    const seen: string[] = [];
    const push = (name: string) => {
      if (!seen.includes(name)) seen.push(name);
    };
    if (segment === "installed") {
      for (const item of state?.installed ?? []) push(installedCategory(item) ?? UNCATEGORIZED);
    } else {
      for (const plugin of market ?? []) push(categoryLabel(plugin.category_name));
    }
    return seen;
  }, [segment, state, market, installedCategory]);

  const switchSegment = (next: PluginSegment) => {
    setSegment(next);
    // 段切换重置筛选（两段分类口径不同，旧筛选易造成空列表困惑）。
    setCategory(null);
    setQuery("");
  };

  const isEmpty = segment === "installed"
    ? !loading && state !== null && rows.length === 0
    : market !== null && rows.length === 0;

  return (
    <section
      className="plugin-workbench"
      role="region"
      aria-label="主库插件"
      data-testid="plugin-workbench"
    >
      <div className="plugin-workbench__bar">
        <div className="plugin-segment" role="tablist" aria-label="插件视图">
          <button
            type="button"
            role="tab"
            aria-selected={segment === "installed"}
            className={`plugin-segment__btn ${segment === "installed" ? "plugin-segment__btn--active" : ""}`}
            onClick={() => switchSegment("installed")}
            data-testid="plugin-segment-installed"
          >
            <Package size={15} aria-hidden="true" />已装插件
          </button>
          <button
            type="button"
            role="tab"
            aria-selected={segment === "market"}
            className={`plugin-segment__btn ${segment === "market" ? "plugin-segment__btn--active" : ""}`}
            onClick={() => switchSegment("market")}
            data-testid="plugin-segment-market"
          >
            <Store size={15} aria-hidden="true" />插件市场
          </button>
        </div>
        {segment === "market" && (
          <div className="plugin-search">
            <Search size={14} aria-hidden="true" />
            <input
              type="search"
              value={query}
              onChange={(event) => setQuery(event.target.value)}
              placeholder="搜索插件"
              aria-label="搜索插件"
              data-testid="plugin-market-search"
            />
          </div>
        )}
        <button
          className="btn btn--quiet"
          type="button"
          onClick={() => {
            setNotice(null);
            void loadState();
            void loadMarket();
          }}
          title="重新读取插件状态"
          data-testid="plugin-refresh"
        >
          <RefreshCw size={15} aria-hidden="true" />刷新
        </button>
      </div>

      {categories.length > 1 && (
        <div className="plugin-chips" role="group" aria-label="按分类筛选" data-testid="plugin-category-chips">
          <button
            type="button"
            className={`plugin-chip ${category === null ? "plugin-chip--active" : ""}`}
            onClick={() => setCategory(null)}
            data-testid="plugin-chip-all"
          >
            全部
          </button>
          {categories.map((name) => (
            <button
              key={name}
              type="button"
              className={`plugin-chip ${category === name ? "plugin-chip--active" : ""}`}
              onClick={() => setCategory(name)}
              data-testid={`plugin-chip-${name}`}
            >
              {name}
            </button>
          ))}
        </div>
      )}

      {error && (
        <p className="workbench__error" role="alert" data-testid="plugin-error">
          {error}
        </p>
      )}
      {notice && (
        <p className="plugin-workbench__notice" data-testid="plugin-notice">{notice}</p>
      )}

      <div className="plugin-list" data-testid={segment === "installed" ? "plugin-installed-list" : "plugin-market-list"}>
        {loading && state === null && segment === "installed" && (
          <p className="plugin-empty">正在读取已装插件…</p>
        )}
        {segment === "market" && market === null && (
          <p className="plugin-empty">{marketError ?? "正在读取插件市场…"}</p>
        )}
        {segment === "market" && market !== null && marketError && (
          <p className="workbench__error" role="alert">{marketError}</p>
        )}
        {isEmpty && segment === "installed" && (
          <p className="plugin-empty" data-testid="plugin-installed-empty">
            当前账号还没有已装插件；可到插件市场浏览安装。
          </p>
        )}
        {isEmpty && segment === "market" && (
          <p className="plugin-empty" data-testid="plugin-market-empty">
            没有符合条件的插件，可调整分类或搜索词。
          </p>
        )}
        {rows.map((row) => (
          <PluginRow
            key={row.key}
            rowTestId={row.testId}
            title={row.title}
            meta={row.meta}
            category={row.category}
            badge={row.badge}
            action={row.action}
            hoverTitle={row.hoverTitle}
          />
        ))}
      </div>

      {/* 卸载确认（ADR-0026 决策 2：列明影响面，单次确认） */}
      {pendingUninstall && (
        <div
          className="preview-veil preview-veil--open"
          onClick={(event) => {
            if (event.target === event.currentTarget) setPendingUninstall(null);
          }}
          data-testid="plugin-uninstall-confirm"
        >
          <div
            className="preview confirm-dialog"
            role="dialog"
            aria-modal="true"
            aria-label="确认移除插件"
          >
            <div className="preview__head">
              <div className="preview__head-main">
                <div className="preview__title">移除插件</div>
                <div className="preview__meta">
                  <span>{pendingUninstall.display_name || pendingUninstall.name}</span>
                </div>
              </div>
            </div>
            <div className="preview__body">
              <p className="confirm-dialog__text">
                将同时从 {state?.known_account_count ?? 1} 个账号移除；移除后切换账号不再带走该插件。插件本体文件不受影响，可随时重新安装。
              </p>
            </div>
            <div className="confirm-dialog__foot">
              <button className="btn" type="button" onClick={() => setPendingUninstall(null)}>
                取消
              </button>
              <button
                className="btn btn--danger"
                type="button"
                onClick={() => void confirmUninstall()}
                data-testid="plugin-uninstall-confirm-ok"
              >
                <Trash2 size={15} aria-hidden="true" />确认移除
              </button>
            </div>
          </div>
        </div>
      )}
    </section>
  );
}
