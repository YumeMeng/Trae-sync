import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { WorkspaceStateDto } from "./types/workspace";
import { TitleBar } from "./components/TitleBar";
import { SettingsPanel } from "./components/SettingsPanel";
import { AccountWorkbench } from "./components/AccountWorkbench";
import { CheckinPage } from "./components/CheckinPage";
import { EnvironmentPage } from "./components/EnvironmentPage";
import { OverviewPage } from "./pages/OverviewPage";
import { WorkBuddyOverviewPage } from "./pages/WorkBuddyOverviewPage";
import { MasterLibraryDetail } from "./pages/MasterLibraryDetail";
import { NavigationRail, type AppPage } from "./components/NavigationRail";
import { WorkBuddyCheckinPage, WorkBuddyPanel } from "./components/WorkBuddyPanel";
import { CapabilitySection } from "./components/CapabilitySection";
import type { LibraryRef } from "./components/LibrarySessionsPanel";
import {
  detailLibraryRefFor,
  loadCurrentProduct,
  loadProductEntries,
  setCurrentProduct,
} from "./platform/productRegistry";
import { CURRENT_PRODUCT_ID, WORKBUDDY_PRODUCT_ID } from "./types/product";
import type { CurrentProductDto, ProductEntryDto } from "./types/product";
import type { SectionKey } from "./platform/productRegistry";
import { notify, Toaster } from "./platform/notify";
import { safeUiErrorMessage } from "./utils/safeUiError";

// stagger 重放守卫（T9，2026-08-27）：这些列表的进场动画只播首批。
// playedStaggerLists 按类名记录已播过的列表（模块级 = 页面会话）：
// 切页/组件重建不重播；刷新随模块重载而重置，恢复"刷新即重放"合同语义。
const STAGGER_LISTS = ["account-list", "account-card-grid", "checkin-flow"];
const playedStaggerLists = new Set<string>();

/**
 * 未适配区块的诚实占位页（P17-T5 并列骨架一期）：页头保持与常规页一致的
 * region/标题/焦点管理结构，正文渲染 pending 说明卡（CapabilitySection）。
 */
function ProductPendingPage({ section, title, note }: { section: SectionKey; title: string; note: string }) {
  return (
    <section className="workbuddy-page" role="region" aria-label={title}>
      <header className="page-header">
        <div className="page-header__copy">
          <h1 data-page-title={section} tabIndex={-1}>{title}</h1>
        </div>
      </header>
      <CapabilitySection
        state="pending"
        product={WORKBUDDY_PRODUCT_ID}
        title={`${title}能力适配中`}
        note={note}
      />
    </section>
  );
}

/** 自动签到批次完成事件（auto-checkin-finished）载荷（与后端 DTO 同口径）。 */
interface AutoCheckinFinishedEvent {
  total: number;
  completed: number;
  failed: number;
  skipped: number;
}

// 应用根组件：P17-T5 并列骨架——产品上下文（标题栏切换器）驱动各页面槽位
// 的分支渲染：TRAE 产品走完整工作区五页；WorkBuddy 产品当前只有账号与
// 签到两个 ready 区块（WorkBuddyPanel / WorkBuddyCheckinPage），其余区块
// 渲染 pending 说明卡（诚实占位）。settings 是应用级设置，两产品共用。
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

  // 当前产品是否 WorkBuddy：决定各页面槽位的分支渲染（并列骨架一期）。
  const isWorkBuddy = currentProduct?.product_id === WORKBUDDY_PRODUCT_ID;

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
          {/* 页面槽位按当前产品分支渲染：不同组件类型切换即卸载重挂载，
              数据新鲜度天然由重挂载重拉保证，无需额外刷新信号。 */}
          {state && (
            <div className="app-page" hidden={activePage !== "overview"}>
              {isWorkBuddy ? (
                <WorkBuddyOverviewPage
                  active={activePage === "overview"}
                  onNavigate={handlePageChange}
                />
              ) : (
                <OverviewPage
                  state={state}
                  active={activePage === "overview"}
                  onNavigate={handlePageChange}
                />
              )}
            </div>
          )}
          <div className="app-page" hidden={activePage !== "accounts"}>
            {isWorkBuddy ? (
              <WorkBuddyPanel active={activePage === "accounts"} />
            ) : (
              <AccountWorkbench active={activePage === "accounts"} />
            )}
          </div>
          <div className="app-page" hidden={activePage !== "checkin"}>
            {isWorkBuddy ? (
              <WorkBuddyCheckinPage active={activePage === "checkin"} />
            ) : (
              <CheckinPage
                active={activePage === "checkin"}
                onNavigate={handlePageChange}
              />
            )}
          </div>
          <div className="app-page" hidden={activePage !== "environment"}>
            {isWorkBuddy ? (
              <ProductPendingPage
                section="environment"
                title="环境"
                note="WorkBuddy 的环境管理将随客户端适配扩展开放。"
              />
            ) : (
              <EnvironmentPage
                active={activePage === "environment"}
                onOpenMasterDetail={handleOpenMasterDetail}
                onOpenLibraryDetail={handleOpenLibraryDetail}
              />
            )}
          </div>
          <div className="app-page" hidden={activePage !== "master-library"}>
            {isWorkBuddy ? (
              <ProductPendingPage
                section="master-library"
                title="主库详情"
                note="WorkBuddy 的对话库浏览将随对话库适配开放。"
              />
            ) : (
              <MasterLibraryDetail
                active={activePage === "master-library"}
                onNavigate={handlePageChange}
                library={detailLibrary}
              />
            )}
          </div>
          <div className="app-page" hidden={activePage !== "settings"}>
            <SettingsPanel active={activePage === "settings"} />
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
