import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { WorkspaceStateDto } from "./types/workspace";
import { TitleBar } from "./components/TitleBar";
import { SettingsPanel } from "./components/SettingsPanel";
import { NavigationRail, type AppPage } from "./components/NavigationRail";
import type { LibraryRef } from "./components/LibrarySessionsPanel";
// 页面槽位接缝（P2a）：六区块的两种实现登记在 sectionSlots，App 只做解析。
import { SECTION_SLOTS, resolveSection } from "./app/transition/sectionSlots";
import {
  detailLibraryRefFor,
  loadCurrentProduct,
  loadProductEntries,
  setCurrentProduct,
} from "./platform/productRegistry";
import { listEnvironments } from "./platform/environment";
import { CURRENT_PRODUCT_ID } from "./types/product";
import type { CurrentProductDto, ProductEntryDto } from "./types/product";
import { notify, Toaster } from "./platform/notify";
import { safeUiErrorMessage } from "./utils/safeUiError";

// stagger 重放守卫（T9，2026-08-27）：这些列表的进场动画只播首批。
// playedStaggerLists 按类名记录已播过的列表（模块级 = 页面会话）：
// 切页/组件重建不重播；刷新随模块重载而重置，恢复"刷新即重放"合同语义。
const STAGGER_LISTS = ["account-list", "account-card-grid", "checkin-flow"];
const playedStaggerLists = new Set<string>();

// 副环境「查看详情」入口的存在性标记（P4d-8b-1）：新环境页只用这个回调**是否存在**
// 决定是否产出 env-detail-<id> 动作，真实跳转走 onOpenEnvDetail。不能复用
// handleOpenLibraryDetail——它收 LibraryRef（新页签名是零参），且被调用会写入空引用。
const ENV_DETAIL_AVAILABLE = () => undefined;

/** 自动签到批次完成事件（auto-checkin-finished）载荷（与后端 DTO 同口径）。 */
interface AutoCheckinFinishedEvent {
  total: number;
  completed: number;
  failed: number;
  skipped: number;
}

// 应用根组件：P17-T5 并列骨架——产品上下文（标题栏切换器）驱动各页面槽位。
// P2a 接缝：六区块各自的两种实现登记在 app/transition/sectionSlots.tsx，
// 本文件只按当前产品解析一次（resolveSection），不再内联产品三目；
// 哪个页面渲染哪套实现是槽位表的事实，壳层不再关心。
export default function App() {
  const [state, setState] = useState<WorkspaceStateDto | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [activePage, setActivePage] = useState<AppPage>("overview");
  // 产品上下文（P17-T5 并列骨架）：标题栏切换器 + 页面分支渲染的数据源。
  // 注册表读取失败不阻塞渲染（products 留空 → 切换器不渲染）；当前产品读取
  // 失败回退缺省 trae_cn（与后端指针缺失语义一致）。
  const [products, setProducts] = useState<readonly ProductEntryDto[]>([]);
  const [currentProduct, setCurrentProductState] = useState<CurrentProductDto | null>(null);
  const [workspaceRefreshPending, setWorkspaceRefreshPending] = useState(false);
  const mainRef = useRef<HTMLElement>(null);
  const navigationMountedRef = useRef(false);
  // 工作台刷新按请求代次收口，避免撤销授权后的旧响应覆盖新状态。
  const workspaceRefreshGenerationRef = useRef(0);
  const mountedRef = useRef(true);
  // P11-5 收官：库详情页当前展示的库实例（null = 主库；副环境 = 环境页
  //「查看详情」入口注入 env_id 与环境名）。与 activePage "master-library"
  // 同一页面槽位承载主库/副库两种详情（导航模式与主库详情一致）。
  const [libraryDetail, setLibraryDetail] = useState<LibraryRef | null>(null);

  // 主库详情页的库实例（ADR-0032 决策 5）：环境页入口注入优先，缺省主库。
  const detailLibrary = detailLibraryRefFor(libraryDetail);

  // 页面槽位解析（P2a 接缝）：产品 → 区块实现的唯一选择点。
  // 各槽位组件以统一形状接收属性（异构属性见 sectionSlots 的 any-exception 说明）。
  const productId = currentProduct?.product_id ?? CURRENT_PRODUCT_ID;
  const OverviewSection = resolveSection(SECTION_SLOTS.overview, productId);
  const AccountsSection = resolveSection(SECTION_SLOTS.accounts, productId);
  const CheckinSection = resolveSection(SECTION_SLOTS.checkin, productId);
  const EnvironmentSection = resolveSection(SECTION_SLOTS.environment, productId);
  const LibrarySection = resolveSection(SECTION_SLOTS["master-library"], productId);

  const handlePageChange = useCallback((page: AppPage) => {
    // 离开库详情页即重置库实例引用，避免下次进入主库详情残留副库上下文。
    if (page !== "master-library") setLibraryDetail(null);
    setActivePage(page);
  }, []);

  /** 主库详情入口（主库卡「详情」）：清空副库引用后进入。 */
  const handleOpenMasterDetail = useCallback(() => {
    setLibraryDetail(null);
    handlePageChange("master-library");
  }, [handlePageChange]);

  /** 副库详情入口（副环境卡「查看详情」）：携带环境引用进入同一详情页。 */
  const handleOpenLibraryDetail = useCallback((library: LibraryRef) => {
    setLibraryDetail(library);
    handlePageChange("master-library");
  }, [handlePageChange]);

  /**
   * 副环境详情接缝（P4d-8b-1）：环境页只回传 envId，库详情页要 LibraryRef
   * （id + 显示名）。签名转换就地在这里做——先带 id 进入详情页（导航不等 IPC），
   * 再按 env_id 从环境列表回查显示名补上；回查失败保持「副库」兜底，不改报错误
   * （详情页标题是次要信息，读不到名字不该阻塞浏览）。
   */
  const handleOpenEnvDetail = useCallback((envId: string) => {
    handleOpenLibraryDetail({ id: envId });
    void listEnvironments({ includeSize: false })
      .then((list) => {
        const name = list.find((item) => item.env_id === envId)?.name.trim();
        if (!name) return;
        // 只在仍停在该副环境详情时补名（用户已切走则不复活旧引用）。
        setLibraryDetail((current) =>
          current?.id === envId && current.displayName !== name ? { id: envId, displayName: name } : current,
        );
      })
      .catch(() => undefined);
  }, [handleOpenLibraryDetail]);

  /**
   * 库页内跳转接缝（P4e-8c）：新库页只回传 libraryId（签名登记在 P4e-8a），而 App 的
   * 详情入口收 LibraryRef（{ id, displayName }）。转换就地在这里做——主库清空副库引用，
   * 其余沿用副环境那条回查路径补显示名（读不到即回落「副库」，不阻塞浏览）。
   */
  const handleOpenLibraryDetailFromId = useCallback((libraryId: string) => {
    if (libraryId === "master") handleOpenMasterDetail();
    else handleOpenEnvDetail(libraryId);
  }, [handleOpenEnvDetail, handleOpenMasterDetail]);

  /** 库页其余动作出口（P4e-8c）：空态引导「去环境页」落到环境页；其余动作暂无接线。 */
  const handleLibraryAction = useCallback((actionId: string) => {
    if (actionId === "empty-open-environment") handlePageChange("environment");
  }, [handlePageChange]);

  const refreshWorkspaceState = useCallback(async () => {
    const generation = (workspaceRefreshGenerationRef.current += 1);
    setWorkspaceRefreshPending(true);
    try {
      const nextState = await invoke<WorkspaceStateDto>("get_workspace_state");
      if (!mountedRef.current || generation !== workspaceRefreshGenerationRef.current) return;
      setState(nextState);
      setError(null);
    } catch (e) {
      if (!mountedRef.current || generation !== workspaceRefreshGenerationRef.current) return;
      setError(safeUiErrorMessage(e, "工作台状态暂时不可读取，请重新打开应用。"));
    } finally {
      if (mountedRef.current && generation === workspaceRefreshGenerationRef.current) {
        setWorkspaceRefreshPending(false);
      }
    }
  }, []);

  useEffect(() => {
    mountedRef.current = true;
    // 通过统一刷新入口拉取 Tauri 状态；前端不直接访问文件/数据库/认证。
    void refreshWorkspaceState();
    // 产品上下文并行加载：注册表失败静默（无切换器），指针失败回退缺省产品。
    void loadProductEntries()
      .then(setProducts)
      .catch(() => undefined);
    void loadCurrentProduct()
      .then(setCurrentProductState)
      .catch(() =>
        setCurrentProductState({ product_id: CURRENT_PRODUCT_ID, display_name: "TRAE" }),
      );
    return () => {
      mountedRef.current = false;
      workspaceRefreshGenerationRef.current += 1;
    };
  }, [refreshWorkspaceState]);

  // 切换产品：写后端指针 → 重读回显（含展示名）；失败提示且状态不变
  // （select 受控值不动）。各页面槽位按当前产品分支，切换即重挂载重拉数据。
  const handleSwitchProduct = useCallback(async (productId: string) => {
    try {
      await setCurrentProduct(productId);
      setCurrentProductState(await loadCurrentProduct());
    } catch (reason: unknown) {
      notify.error(safeUiErrorMessage(reason, "产品切换未完成，请稍后重试。"));
    }
  }, []);

  useEffect(() => {
    // 自动签到跨页完成摘要（P11-1 切片②）：此前只有签到页监听该事件，
    // 用户离开签到页就看不到批次结果。App 级监听保证任意页面都能收到。
    // finished 载荷只有计数（无逐账号名单），失败/跳过计数收进标题，
    // notify.summary 的"成功 N 项"承载完成数；逐账号明细仍由签到页展示。
    let unlisten: (() => void) | null = null;
    let cancelled = false;
    void listen<AutoCheckinFinishedEvent>("auto-checkin-finished", (event) => {
      const { completed, failed, skipped } = event.payload;
      const suffix = failed > 0
        ? `（失败 ${failed}${skipped > 0 ? `，跳过 ${skipped}` : ""}）`
        : skipped > 0 ? `（跳过 ${skipped}）` : "";
      notify.summary(`TRAE：今日自动签到完成${suffix}`, [], completed);
    }).then((fn) => {
      if (cancelled) fn();
      else unlisten = fn;
    }).catch(() => undefined);
    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, []);

  useEffect(() => {
    // 导航切换后把焦点送到当前页面标题，避免键盘用户停留在旧页面的导航按钮上。
    if (!navigationMountedRef.current) {
      navigationMountedRef.current = true;
      return;
    }
    const heading = mainRef.current?.querySelector<HTMLElement>(
      `[data-page-title="${activePage}"]`,
    );
    heading?.focus({ preventScroll: true });
  }, [activePage]);

  useEffect(() => {
    // stagger 重放修复（T9，2026-08-27）：页面用 [hidden] 切换，display:none 重显时
    // CSS animation 会重播；React 组件重建还会换掉整个列表容器（标记随之丢失）。
    // 两条压制路径：a) 同一容器重显——播完后打 data-entered，CSS 压制；
    // b) 容器重建——模块级 playedStaggerLists 记住已播类型，MutationObserver
    // 给新容器补标记（微任务时机，渲染前生效，不会闪一帧动画）。
    // 700ms = 最大 stagger 延迟 360ms + 时长 220ms 的余量，保证整批播完再标记。
    const STAGGER_CONTAINER = ".account-list, .account-card-grid, .checkin-flow";
    const STAGGER_ITEM = ".account-list > li, .account-card-grid > li, .checkin-flow > li";
    const staggerKeyOf = (el: Element) =>
      STAGGER_LISTS.find((cls) => el.classList.contains(cls));
    const onAnimationEnd = (event: AnimationEvent) => {
      const target = event.target;
      if (!(target instanceof Element) || !target.matches(STAGGER_ITEM)) return;
      const container = target.parentElement;
      if (!container || container.dataset.enteredScheduled) return;
      container.dataset.enteredScheduled = "1";
      const key = staggerKeyOf(container);
      if (key) playedStaggerLists.add(key);
      window.setTimeout(() => container.setAttribute("data-entered", "true"), 700);
    };
    // 已播类型的重建容器：直接标记压制，不重播。
    const suppressRebuilt = (el: Element) => {
      const key = staggerKeyOf(el);
      if (key && playedStaggerLists.has(key) && !el.hasAttribute("data-entered")) {
        el.setAttribute("data-entered", "true");
      }
    };
    document.querySelectorAll(STAGGER_CONTAINER).forEach(suppressRebuilt);
    const observer = new MutationObserver((records) => {
      for (const record of records) {
        for (const node of record.addedNodes) {
          if (!(node instanceof Element)) continue;
          if (node.matches(STAGGER_CONTAINER)) suppressRebuilt(node);
          else node.querySelectorAll(STAGGER_CONTAINER).forEach(suppressRebuilt);
        }
      }
    });
    observer.observe(document.body, { childList: true, subtree: true });
    document.addEventListener("animationend", onAnimationEnd);
    return () => {
      document.removeEventListener("animationend", onAnimationEnd);
      observer.disconnect();
    };
  }, []);

  if (error) {
    return (
      <div className="app-error" role="alert" aria-atomic="true">
        加载工作台状态失败：{error}
        <button
          type="button"
          className="btn btn--primary"
          onClick={refreshWorkspaceState}
          disabled={workspaceRefreshPending}
        >
          {workspaceRefreshPending ? "重新读取中…" : "重新读取工作台状态"}
        </button>
      </div>
    );
  }

  if (!state) {
    return (
      <div className="app-loading" role="status" aria-live="polite" aria-atomic="true">
        正在加载 Trae Sync 工作台…
      </div>
    );
  }

  return (
    <div className="app-shell">
      <a className="skip-link" href="#main-content">跳到主要内容</a>
      <TitleBar
        platform={state.platform}
        products={products}
        currentProduct={currentProduct}
        onSwitchProduct={(productId) => void handleSwitchProduct(productId)}
      />
      <div className="app-body">
        <NavigationRail activePage={activePage} onPageChange={handlePageChange} />
        <main id="main-content" className="app-main" ref={mainRef}>
          {/* 页面槽位由 sectionSlots 解析：不同组件类型切换即卸载重挂载，
              数据新鲜度天然由重挂载重拉保证，无需额外刷新信号。
              P2a 接缝：产品判断只发生在 resolveSection 一处，此处零三目。 */}
          {state && (
            <div className="app-page" hidden={activePage !== "overview"}>
              <OverviewSection
                state={state}
                active={activePage === "overview"}
                onNavigate={handlePageChange}
              />
            </div>
          )}
          <div className="app-page" hidden={activePage !== "accounts"}>
            <AccountsSection active={activePage === "accounts"} />
          </div>
          <div className="app-page" hidden={activePage !== "checkin"}>
            <CheckinSection
              active={activePage === "checkin"}
              onNavigate={handlePageChange}
            />
          </div>
          <div className="app-page" hidden={activePage !== "environment"}>
            {/* P4d-8b-1：环境页两产品共用一份实现，productId 由槽位包装绑定。
                详情入口两路——主库走 onOpenMasterDetail；副环境「查看详情」由
                onOpenLibraryDetail 决定是否出现（存在性），跳转经 onOpenEnvDetail
                把 envId 转成 App 的 LibraryRef（见 handleOpenEnvDetail）。 */}
            <EnvironmentSection
              active={activePage === "environment"}
              onOpenMasterDetail={handleOpenMasterDetail}
              onOpenLibraryDetail={ENV_DETAIL_AVAILABLE}
              onOpenEnvDetail={handleOpenEnvDetail}
            />
          </div>
          <div className="app-page" hidden={activePage !== "master-library"}>
            {/* P4e-8c：库页两产品共用一份实现，productId 由槽位包装绑定。库实例（id + 显示名）
                取 App 现有详情态（detailLibrary，缺省主库；副环境入口注入 env_id 与环境名）。
                返回 → 环境页；空态引导（empty-open-environment）同样落到环境页；其余动作原样透出。
                onOpenLibraryDetail 的签名转换就地做：新页回传 libraryId，App 的入口收 LibraryRef。 */}
            <LibrarySection
              active={activePage === "master-library"}
              libraryId={detailLibrary.id}
              libraryDisplayName={detailLibrary.displayName}
              onBack={() => handlePageChange("environment")}
              onOpenLibraryDetail={handleOpenLibraryDetailFromId}
              onAction={handleLibraryAction}
            />
          </div>
          <div className="app-page" hidden={activePage !== "settings"}>
            <SettingsPanel active={activePage === "settings"} productId={productId} />
          </div>
        </main>
      </div>
      {/* 全局通知层（P11-1 切片①）：唯一渲染点。发通知须经 platform/notify。
          顶部下移 48px（标题栏 38px + 10px 间隙），toast 不遮挡标题栏拖拽区；
          悬停暂停为 sonner 2.x 内建默认（pauseOnHover prop 已随 2.0 移除）；
          不抢焦点（sonner 默认 aria-live polite）。 */}
      <Toaster
        position="top-center"
        visibleToasts={4}
        closeButton
        offset={{ top: 48, bottom: 48 }}
      />
      {/* 窗口边缘拖动热区：四周 5px 透明条（配合标题栏）让整圈边缘都能拖动/双击最大化窗口。
          顶部无需单独热区（标题栏整行已是拖拽区）；元素本身命中才触发 drag-region，
          z=5 低于弹层，模态遮罩打开时热区自然被覆盖。 */}
      <div className="window-drag-edge window-drag-edge--left" data-tauri-drag-region="" aria-hidden="true" />
      <div className="window-drag-edge window-drag-edge--right" data-tauri-drag-region="" aria-hidden="true" />
      <div className="window-drag-edge window-drag-edge--bottom" data-tauri-drag-region="" aria-hidden="true" />
    </div>
  );
}
