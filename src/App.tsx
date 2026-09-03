import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import type { WorkspaceStateDto } from "./types/workspace";
import { TitleBar } from "./components/TitleBar";
import { HistoryWorkbench } from "./components/HistoryWorkbench";
import { SettingsPanel } from "./components/SettingsPanel";
import { AccountCenter } from "./components/AccountCenter";
import { CheckinPage } from "./components/CheckinPage";
import { EnvironmentPage } from "./components/EnvironmentPage";
import { OverviewPage } from "./pages/OverviewPage";
import { MasterLibraryDetail } from "./pages/MasterLibraryDetail";
import { NavigationRail, type AppPage } from "./components/NavigationRail";
import { safeUiErrorMessage } from "./utils/safeUiError";

// stagger 重放守卫（T9，2026-08-27）：这些列表的进场动画只播首批。
// playedStaggerLists 按类名记录已播过的列表（模块级 = 页面会话）：
// 切页/组件重建不重播；刷新随模块重载而重置，恢复"刷新即重放"合同语义。
const STAGGER_LISTS = ["account-list", "account-card-grid", "checkin-flow"];
const playedStaggerLists = new Set<string>();

// 应用根组件：五页信息架构（总览/历史/账号/签到/设置），
// 历史读取入口收敛在历史页，账号档案收敛在账号页，签到收敛在签到页。
export default function App() {
  const [state, setState] = useState<WorkspaceStateDto | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [activePage, setActivePage] = useState<AppPage>("overview");
  const [accountRefreshState, setAccountRefreshState] = useState<"idle" | "loading" | "error">(
    "idle",
  );
  const [accountRefreshError, setAccountRefreshError] = useState<string | null>(null);
  const [workspaceRefreshPending, setWorkspaceRefreshPending] = useState(false);
  const mainRef = useRef<HTMLElement>(null);
  const navigationMountedRef = useRef(false);
  // 工作台刷新按请求代次收口，避免撤销授权后的旧响应覆盖新状态。
  const workspaceRefreshGenerationRef = useRef(0);
  const mountedRef = useRef(true);

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

  const refreshCurrentAccount = useCallback(async () => {
    setAccountRefreshState("loading");
    setAccountRefreshError(null);
    try {
      // 账号检测命令只刷新非敏感证据，不执行外部账号切换。
      await invoke("refresh_managed_current_account");
      await refreshWorkspaceState();
      setAccountRefreshState("idle");
    } catch (e) {
      setAccountRefreshState("error");
      setAccountRefreshError(
        safeUiErrorMessage(e, "当前账号暂时无法重新检测，请启动并关闭 TRAE 后重试。"),
      );
      // 账号刷新失败也要重新读取权威工作区，避免授权已撤销时保留旧账号标题。
      await refreshWorkspaceState();
    }
  }, [refreshWorkspaceState]);

  useEffect(() => {
    mountedRef.current = true;
    // 通过统一刷新入口拉取 Tauri 状态；前端不直接访问文件/数据库/认证。
    void refreshWorkspaceState();
    return () => {
      mountedRef.current = false;
      workspaceRefreshGenerationRef.current += 1;
    };
  }, [refreshWorkspaceState]);

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
        currentAccount={state.current_account}
        capabilities={state.capabilities}
      />
      <div className="app-body">
        <NavigationRail activePage={activePage} onPageChange={setActivePage} />
        <main id="main-content" className="app-main" ref={mainRef}>
          <div className="app-page" hidden={activePage !== "overview"}>
            <OverviewPage
              state={state}
              active={activePage === "overview"}
              onNavigate={setActivePage}
              accountRefreshState={accountRefreshState}
              accountRefreshError={accountRefreshError}
              onRedetectAccount={refreshCurrentAccount}
            />
          </div>
          <div className="app-page" hidden={activePage !== "history"}>
            <HistoryWorkbench
              active={activePage === "history"}
              onNavigate={setActivePage}
            />
          </div>
          <div className="app-page" hidden={activePage !== "accounts"}>
            <AccountCenter active={activePage === "accounts"} />
          </div>
          <div className="app-page" hidden={activePage !== "checkin"}>
            <CheckinPage active={activePage === "checkin"} onNavigate={setActivePage} />
          </div>
          <div className="app-page" hidden={activePage !== "environment"}>
            <EnvironmentPage
              active={activePage === "environment"}
              onOpenMasterDetail={() => setActivePage("master-library")}
            />
          </div>
          <div className="app-page" hidden={activePage !== "master-library"}>
            <MasterLibraryDetail active={activePage === "master-library"} onNavigate={setActivePage} />
          </div>
          <div className="app-page" hidden={activePage !== "settings"}>
            <SettingsPanel active={activePage === "settings"} />
          </div>
        </main>
      </div>
    </div>
  );
}
