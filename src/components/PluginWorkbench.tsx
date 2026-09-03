import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Package, RefreshCw, Store, Trash2 } from "lucide-react";
import type {
  InstalledPluginDto,
  MarketPluginItem,
  PluginTabStateDto,
} from "../types/plugins";
import { safeUiErrorMessage } from "../utils/safeUiError";

// ============================================================================
// P5-8b 插件 tab 工作台（ADR-0023 环境插件清单）：已装清单 + 对账 +
// 市场浏览 + 装/卸。工具为主管理插件环境：即时改当前账号云端并同步环境
// 清单；检测到 TRAE 内手动装/卸产生的差异时提示吸收（以账号现状为准）。
// ============================================================================

/** 插件 tab 内部分段：已装（默认）/ 市场（懒加载，首次进入才拉目录）。 */
type PluginSegment = "installed" | "market";

interface PluginWorkbenchProps {
  /** tab 可见时才读取；不可见不发起请求（与详情页其他分区同口径）。 */
  active: boolean;
}

export function PluginWorkbench({ active }: PluginWorkbenchProps) {
  const [state, setState] = useState<PluginTabStateDto | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const [segment, setSegment] = useState<PluginSegment>("installed");
  // 市场目录懒加载：首次切到市场段才请求（用户触发式，不后台预取）。
  const [market, setMarket] = useState<MarketPluginItem[] | null>(null);
  const [marketLoading, setMarketLoading] = useState(false);
  const [marketError, setMarketError] = useState<string | null>(null);
  // 卸载二次确认（与主库会话删除同级纪律：列明对象，单次确认）。
  const [pendingUninstall, setPendingUninstall] = useState<InstalledPluginDto | null>(null);
  // 装/卸/吸收进行中的条目键，防止重复点击。
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

  useEffect(() => {
    mountedRef.current = true;
    return () => {
      mountedRef.current = false;
    };
  }, []);

  useEffect(() => {
    if (!active) return;
    void loadState();
  }, [active, loadState]);

  const loadMarket = useCallback(async () => {
    if (marketLoading) return;
    setMarketLoading(true);
    try {
      const items = await invoke<MarketPluginItem[]>("browse_plugin_market");
      if (!mountedRef.current) return;
      setMarket(items);
      setMarketError(null);
    } catch (reason: unknown) {
      if (!mountedRef.current) return;
      setMarketError(safeUiErrorMessage(reason, "插件市场暂时不可读取，请稍后重试。"));
    } finally {
      if (mountedRef.current) setMarketLoading(false);
    }
  }, [marketLoading]);

  const enterMarket = useCallback(() => {
    setSegment("market");
    // 懒加载：目录只在首次进入市场段时拉取，之后沿用缓存（刷新按钮可重拉）。
    if (market === null) void loadMarket();
  }, [market, loadMarket]);

  /** 安装市场插件：即时改云端 + 更新清单（ADR-0023 决策 3），完成后刷新状态。 */
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

  /** 确认卸载：云端移除 + 清单同步移除，完成后刷新状态。 */
  const confirmUninstall = useCallback(async () => {
    if (!pendingUninstall) return;
    const target = pendingUninstall;
    setPendingUninstall(null);
    setBusyKey(target.record_id);
    setNotice(null);
    try {
      await invoke("uninstall_plugin", { recordId: target.record_id });
      if (!mountedRef.current) return;
      setNotice(`已移除 ${target.display_name || target.name}。`);
      await loadState();
    } catch (reason: unknown) {
      if (!mountedRef.current) return;
      setError(safeUiErrorMessage(reason, "移除未完成，请稍后重试。"));
    } finally {
      if (mountedRef.current) setBusyKey(null);
    }
  }, [pendingUninstall, loadState]);

  /** 吸收云端变化进清单（ADR-0023 决策 2）：以当前账号现状为准重写基线。 */
  const absorb = useCallback(async () => {
    setBusyKey("__absorb__");
    setNotice(null);
    try {
      await invoke("absorb_plugin_manifest");
      if (!mountedRef.current) return;
      setNotice("已按当前账号更新主库插件清单。");
      await loadState();
    } catch (reason: unknown) {
      if (!mountedRef.current) return;
      setError(safeUiErrorMessage(reason, "清单更新未完成，请稍后重试。"));
    } finally {
      if (mountedRef.current) setBusyKey(null);
    }
  }, [loadState]);

  // 对账差异（ADR-0023 决策 2）：TRAE 内手动装/卸与清单基线的偏移。
  const cloudOnly = (state?.installed ?? []).filter(
    (item) => !item.builtin && item.marketplace_plugin_id !== null && !item.in_manifest,
  );
  const manifestMissing = (state?.manifest ?? []).filter((entry) => !entry.installed_in_cloud);
  const hasDrift = cloudOnly.length > 0 || manifestMissing.length > 0;

  // 市场条目安装态：按市场 UUID（其次名称）与云端已装匹配。
  const installedKeys = new Set(
    (state?.installed ?? [])
      .flatMap((item) => [item.marketplace_plugin_id, item.name])
      .filter((key): key is string => key !== null),
  );

  /**
   * 市场目录分组：后端已按分类排序，相邻同分类合并成组；
   * 无分类信息的条目归入「其他」（与 TRAE 插件市场按作用分类对齐）。
   */
  const marketGroups = useMemo(() => {
    if (!market) return [];
    const groups: { name: string; items: MarketPluginItem[] }[] = [];
    for (const plugin of market) {
      const name = plugin.category_name?.trim() || "其他";
      const last = groups[groups.length - 1];
      if (last && last.name === name) last.items.push(plugin);
      else groups.push({ name, items: [plugin] });
    }
    return groups;
  }, [market]);

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
            onClick={() => setSegment("installed")}
            data-testid="plugin-segment-installed"
          >
            <Package size={15} aria-hidden="true" />已装插件
          </button>
          <button
            type="button"
            role="tab"
            aria-selected={segment === "market"}
            className={`plugin-segment__btn ${segment === "market" ? "plugin-segment__btn--active" : ""}`}
            onClick={enterMarket}
            data-testid="plugin-segment-market"
          >
            <Store size={15} aria-hidden="true" />插件市场
          </button>
        </div>
        <button
          className="btn btn--quiet"
          type="button"
          onClick={() => {
            setNotice(null);
            void loadState();
            if (segment === "market") void loadMarket();
          }}
          title="重新读取插件状态"
          data-testid="plugin-refresh"
        >
          <RefreshCw size={15} aria-hidden="true" />刷新
        </button>
      </div>

      {error && (
        <p className="workbench__error" role="alert" data-testid="plugin-error">
          {error}
        </p>
      )}
      {notice && (
        <p className="plugin-workbench__notice" data-testid="plugin-notice">{notice}</p>
      )}

      {/* 对账提示（有差异才出现；无差异静默——减少打扰） */}
      {state && hasDrift && segment === "installed" && (
        <div className="plugin-drift" data-testid="plugin-drift">
          <div className="plugin-drift__copy">
            <b>主库插件清单与当前账号不一致</b>
            <span>
              {cloudOnly.length > 0 && `在 TRAE 内新装 ${cloudOnly.length} 项`}
              {cloudOnly.length > 0 && manifestMissing.length > 0 && " · "}
              {manifestMissing.length > 0 && `在 TRAE 内移除 ${manifestMissing.length} 项`}
            </span>
          </div>
          <button
            className="btn"
            type="button"
            onClick={() => void absorb()}
            disabled={busyKey !== null}
            title="以当前账号云端现状为准，更新主库插件清单"
            data-testid="plugin-drift-absorb"
          >
            以当前账号为准更新
          </button>
        </div>
      )}

      {segment === "installed" && (
        <div className="plugin-list" data-testid="plugin-installed-list">
          {loading && state === null && <p className="plugin-empty">正在读取已装插件…</p>}
          {!loading && state !== null && state.installed.length === 0 && (
            <p className="plugin-empty" data-testid="plugin-installed-empty">
              当前账号还没有已装插件；可到插件市场浏览安装。
            </p>
          )}
          {state?.installed.map((item) => (
            <div
              key={item.record_id}
              className="plugin-row"
              data-testid={`plugin-row-${item.record_id}`}
              title={
                item.registry
                  ? `插件标识：${item.name} · 来源：${item.registry}`
                  : `插件标识：${item.name}`
              }
            >
              <div className="plugin-row__main">
                <span className="plugin-row__name">{item.display_name || item.name}</span>
                {item.version && (
                  <span className="plugin-row__meta">版本 {item.version}</span>
                )}
              </div>
              <div className="plugin-row__side">
                {item.builtin && <span className="plugin-badge">内置</span>}
                {!item.builtin && item.in_manifest && <span className="plugin-badge plugin-badge--bound">随主库</span>}
                {!item.builtin && !item.in_manifest && (
                  <span className="plugin-badge plugin-badge--drift">未入清单</span>
                )}
                <button
                  className="btn btn--quiet"
                  type="button"
                  onClick={() => setPendingUninstall(item)}
                  disabled={item.builtin || busyKey !== null}
                  title={item.builtin ? "内置插件，无需移除" : "从当前账号移除该插件"}
                  data-testid={`plugin-uninstall-${item.record_id}`}
                >
                  <Trash2 size={14} aria-hidden="true" />移除
                </button>
              </div>
            </div>
          ))}
        </div>
      )}

      {segment === "market" && (
        <div className="plugin-list" data-testid="plugin-market-list">
          {marketLoading && market === null && (
            <p className="plugin-empty">正在读取插件市场…</p>
          )}
          {marketError && <p className="workbench__error" role="alert">{marketError}</p>}
          {!marketLoading && market !== null && market.length === 0 && (
            <p className="plugin-empty" data-testid="plugin-market-empty">
              插件市场目录暂时为空，请稍后刷新。
            </p>
          )}
          {marketGroups.map((group) => (
            <div
              className="plugin-market-group"
              key={group.name}
              data-testid={`plugin-market-group-${group.name}`}
            >
              <div className="plugin-market-group__head">
                <span>{group.name}</span>
                <span>{group.items.length} 项</span>
              </div>
              {group.items.map((plugin) => {
                const installed = installedKeys.has(plugin.plugin_id) || installedKeys.has(plugin.name);
                return (
                  <div
                    key={plugin.plugin_id}
                    className="plugin-row"
                    data-testid={`plugin-market-row-${plugin.plugin_id}`}
                    title={`插件标识：${plugin.name}${plugin.registry ? ` · 来源：${plugin.registry}` : ""}`}
                  >
                    <div className="plugin-row__main">
                      <span className="plugin-row__name">{plugin.display_name || plugin.name}</span>
                      {plugin.description && (
                        <span className="plugin-row__desc">{plugin.description}</span>
                      )}
                    </div>
                    <div className="plugin-row__side">
                      {installed ? (
                        <span className="plugin-badge plugin-badge--bound" data-testid={`plugin-market-installed-${plugin.plugin_id}`}>
                          已安装
                        </span>
                      ) : (
                        <button
                          className="btn"
                          type="button"
                          onClick={() => void install(plugin)}
                          disabled={busyKey !== null}
                          data-testid={`plugin-install-${plugin.plugin_id}`}
                        >
                          安装
                        </button>
                      )}
                    </div>
                  </div>
                );
              })}
            </div>
          ))}
        </div>
      )}

      {/* 卸载二次确认：与主库删除同级纪律（列明对象 + 单次确认） */}
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
                将从当前账号移除该插件，并同步移出主库插件清单；插件本体文件不受影响，可随时重新安装。
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
