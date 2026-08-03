import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import type {
  BrowseResultDto,
  BrowseSessionNodeDto,
  ConversationPreviewDto,
  ProcessRunningState,
  ScanOutcomeDto,
  SearchHitDto,
  SessionIdentityDto,
  SyncPlanDto,
  SyncScopeDto,
} from "../types/history";

// ============================================================================
// T03 历史库工作台：方案 D 三栏布局 + 授权扫描 + 浏览/搜索/预览
// ============================================================================
//
// 状态机：
// - idle：未授权，显示授权表单
// - scanning：扫描中，禁用交互
// - success：扫描成功，显示浏览结果
// - failure：扫描失败，显示结构化原因
// - empty：扫描成功但目录库为空
//
// 关键约束（T03/T04 AC）：
// - 初始不自动扫描（AC1）
// - 未授权/运行中不发布快照（AC2）
// - 点击对话标题只打开预览，不改变同步选择（AC9）
// - 搜索结果显示账号/项目/会话上下文，打开预览不改选择（AC9）
// - 不暴露同步/写入/备份/恢复控件（P1 范围外）

type HistoryPhase = "idle" | "scanning" | "success" | "failure" | "empty";

interface HistoryWorkbenchProps {
  /** T01 工作台能力开关——P1 阶段 scan_enabled 由历史库自身授权控制 */
  capabilities: { scan_enabled: boolean; sync_enabled: boolean };
  honestStatus: string;
}

export function HistoryWorkbench({
  capabilities,
  honestStatus,
}: HistoryWorkbenchProps) {
  // 授权与扫描状态
  const [phase, setPhase] = useState<HistoryPhase>("idle");
  const [fixtureRoot, setFixtureRoot] = useState("");
  const [dbRelativePath, setDbRelativePath] = useState("database.db");
  const [processRunning, setProcessRunning] = useState(false);
  const [scanError, setScanError] = useState<string | null>(null);

  // 浏览结果
  const [browseResult, setBrowseResult] = useState<BrowseResultDto | null>(null);
  const [browseError, setBrowseError] = useState<string | null>(null);

  // 选中账号/项目（用于筛选树展开）
  const [selectedAccount, setSelectedAccount] = useState<string | null>(null);
  const [selectedProject, setSelectedProject] = useState<string | null>(null);

  // 预览（点击标题只打开预览，不改选择——AC9）
  const [previewSession, setPreviewSession] = useState<SessionIdentityDto | null>(null);
  const [previewData, setPreviewData] = useState<ConversationPreviewDto | null>(null);
  const [previewLoading, setPreviewLoading] = useState(false);

  // 搜索
  const [searchQuery, setSearchQuery] = useState("");
  const [searchResults, setSearchResults] = useState<readonly SearchHitDto[] | null>(null);
  const [searchError, setSearchError] = useState<string | null>(null);

  // T05：同步范围与只读计划预览。执行写入仍由后续任务包提供。
  const [scopeMode, setScopeMode] = useState<"all" | "custom">("all");
  const [selectedAccounts, setSelectedAccounts] = useState<readonly string[]>([]);
  const [selectedProjects, setSelectedProjects] = useState<readonly string[]>([]);
  const [selectedSessions, setSelectedSessions] = useState<readonly SessionIdentityDto[]>([]);
  const [syncPlan, setSyncPlan] = useState<SyncPlanDto | null>(null);
  const [planLoading, setPlanLoading] = useState(false);
  const [planError, setPlanError] = useState<string | null>(null);
  const planGenerationRef = useRef(0);

  // R7：已授权的 canonical fixture_root——由后端 grant_scan_authorization 返回。
  // 前端不再持有"自封"的授权；只有后端成功授权后此值才非空。
  // 取消授权或路径变化时此值清空，旧授权在后端被撤销。
  const [authorizedFixtureRoot, setAuthorizedFixtureRoot] = useState<string | null>(null);

  // R12：授权 pending 标记——true 时表示有 grant 请求未返回。
  // pending 期间禁止重复发起同一授权；扫描按钮保持禁用；不显示为已授权。
  const [authorizationPending, setAuthorizationPending] = useState(false);

  // R12：单调递增的授权请求代次——每次路径变化、取消授权、卸载都递增。
  // grant 返回时只有代次匹配且输入快照匹配才接受结果；否则立即 revoke 并丢弃。
  const authGenerationRef = useRef(0);
  // R12：当前 pending 请求的输入快照——用于验证返回时输入未变化
  const pendingAuthSnapshotRef = useRef<{
    fixtureRoot: string;
    dbRelativePath: string;
  } | null>(null);

  // R12-B：串行保存 revoke。新的 grant 必须等待此前撤销完成，
  // 避免旧 revoke 晚完成后清除新授权。
  const authorizationMutationRef = useRef<Promise<void>>(Promise.resolve());

  // R12-C：mounted guard——组件卸载后所有异步回调（grant 返回、stale revoke、
  // revoke 失败）均不得调用 React setter，否则触发"对已卸载组件 setState"警告。
  const mountedRef = useRef(true);

  // 扫描授权前置条件：fixture_root 与 db_relative_path 非空
  // storage_root 由后端 env 控制，前端不可注入，未配置时后端返回错误
  const canAuthorize =
    fixtureRoot.trim().length > 0 &&
    dbRelativePath.trim().length > 0;

  // R7：authorized 派生自后端授权状态——只有 authorizedFixtureRoot 非空时为 true
  // R12：pending 时不视为已授权
  const authorized = authorizedFixtureRoot !== null && !authorizationPending;

  // R12-A：checkbox 在 pending 时也选中——用户可点击取消 pending 请求。
  // pending 与已授权通过 label 文字区分，避免用户混淆。
  const authorizeChecked = authorized || authorizationPending;

  // R12：使当前 pending 请求失效的内部辅助——递增 generation 并清空快照。
  // 调用后，任何未返回的 grant 响应都会被视为 stale。
  const invalidatePendingAuth = useCallback(() => {
    authGenerationRef.current += 1;
    pendingAuthSnapshotRef.current = null;
    setAuthorizationPending(false);
    planGenerationRef.current += 1;
    setSyncPlan(null);
    setPlanLoading(false);
  }, []);

  // R12-B：撤销后端授权的辅助——调用 revoke 并处理失败状态。
  // revoke 失败时本地仍保持未授权，并显示结构化失败状态；不静默恢复旧授权。
  // R12-C：revoke 失败的 setScanError 通过 mountedRef guard 保护。
  const revokeBackendAuth = useCallback((reason: string, reportFailure = true) => {
    const revoke = authorizationMutationRef.current
      .catch(() => undefined)
      .then(() => invoke<void>("revoke_scan_authorization"))
      .catch((e) => {
        // R12-C：卸载清理失败不更新状态；交互触发的失败仍显示诊断。
        if (reportFailure && mountedRef.current) {
          setScanError(`${reason}: revoke 失败 ${String(e)}`);
        }
      });
    authorizationMutationRef.current = revoke;
    return revoke;
  }, []);

  // R7：用户勾选授权时调用后端 grant_scan_authorization，建立后端授权状态机
  // R12：绑定 generation 与输入快照，await 返回后校验代次与输入一致性
  // R12-A：pending 时 checkbox 选中，用户可点击取消（checked=false 进入 else 分支）
  // R12-B：stale grant 返回时只在当前无有效授权时才 revoke，避免误伤新授权
  // R12-C：所有 await 后的 setter 通过 mountedRef guard 保护
  const handleAuthorizeToggle = useCallback(
    async (checked: boolean) => {
      if (checked) {
        if (!canAuthorize) {
          // 前置条件不满足——拒绝建立授权（按钮应已禁用，此处防御）
          return;
        }
        if (authorizationPending) {
          // R12：pending 期间禁止重复发起同一授权
          return;
        }
        // R12：发起新授权请求——递增 generation 并捕获输入快照
        const generation = (authGenerationRef.current += 1);
        const snapshot = { fixtureRoot, dbRelativePath };
        pendingAuthSnapshotRef.current = snapshot;
        setAuthorizationPending(true);
        setScanError(null);
        try {
          // R12-B：等待此前路径变化/取消产生的 revoke 完成，再建立新授权。
          await authorizationMutationRef.current;
          if (
            !mountedRef.current ||
            generation !== authGenerationRef.current ||
            pendingAuthSnapshotRef.current !== snapshot
          ) {
            return;
          }
          const canonical = await invoke<string>("grant_scan_authorization", {
            fixtureRoot,
            dbRelativePath,
          });
          // R12-C：卸载后不更新任何状态
          if (!mountedRef.current) return;
          // R12：返回后校验——代次、输入快照、组件状态均须匹配
          if (
            generation !== authGenerationRef.current ||
            pendingAuthSnapshotRef.current !== snapshot
          ) {
            // R12-B：后端服务端代次保证 stale grant 无法提交；此前撤销也已串行完成。
            return;
          }
          // R12：再次校验当前输入与发起时一致
          if (
            fixtureRoot !== snapshot.fixtureRoot ||
            dbRelativePath !== snapshot.dbRelativePath
          ) {
            // 输入已变化——不应发生（generation 匹配意味着未变化），防御性 revoke
            revokeBackendAuth("grant 返回时输入已变化");
            return;
          }
          // 接受最新授权结果
          setAuthorizedFixtureRoot(canonical);
          setAuthorizationPending(false);
          pendingAuthSnapshotRef.current = null;
          setScanError(null);
        } catch (e) {
          // R12-C：授权失败——仅在组件仍挂载且仍是当前 generation 时更新状态
          if (mountedRef.current && generation === authGenerationRef.current) {
            setAuthorizedFixtureRoot(null);
            setAuthorizationPending(false);
            pendingAuthSnapshotRef.current = null;
            setScanError(String(e));
            setPhase("failure");
          }
        }
      } else {
        // R12-A：取消授权（包括 pending 期间取消）——先使 pending 失效，
        // 再撤销后端授权，清空本地状态。pending 时后端可能已建立授权，必须 revoke。
        // revoke 失败时由 revokeBackendAuth 设置结构化失败状态，本地保持未授权
        invalidatePendingAuth();
        setAuthorizedFixtureRoot(null);
        revokeBackendAuth("用户取消授权");
      }
    },
    [
      canAuthorize,
      fixtureRoot,
      dbRelativePath,
      authorizationPending,
      invalidatePendingAuth,
      revokeBackendAuth,
    ],
  );

  // R7：路径变化时撤销旧授权——旧授权不得继续有效
  // R12：pending 请求也必须失效——递增 generation 使 stale response 被丢弃
  const handleFixtureRootChange = useCallback(
    (value: string) => {
      setFixtureRoot(value);
      // R12：无论 pending 还是已授权，路径变化都使当前授权上下文失效
      if (authorizationPending || authorizedFixtureRoot !== null) {
        invalidatePendingAuth();
        setAuthorizedFixtureRoot(null);
        revokeBackendAuth("fixtureRoot 变化");
      }
    },
    [authorizationPending, authorizedFixtureRoot, invalidatePendingAuth, revokeBackendAuth],
  );

  const handleDbRelativePathChange = useCallback(
    (value: string) => {
      setDbRelativePath(value);
      // R12：无论 pending 还是已授权，路径变化都使当前授权上下文失效
      if (authorizationPending || authorizedFixtureRoot !== null) {
        invalidatePendingAuth();
        setAuthorizedFixtureRoot(null);
        revokeBackendAuth("dbRelativePath 变化");
      }
    },
    [authorizationPending, authorizedFixtureRoot, invalidatePendingAuth, revokeBackendAuth],
  );

  // R12-C：组件卸载时标记 mounted=false，使所有异步回调不再更新 React 状态。
  // 同时递增 generation 使 pending grant 返回时被识别为 stale，并 revoke 后端授权。
  useEffect(() => {
    // React StrictMode 开发模式会执行 setup-cleanup-setup；每次 setup 必须恢复挂载标记。
    mountedRef.current = true;
    return () => {
      mountedRef.current = false;
      authGenerationRef.current += 1;
      pendingAuthSnapshotRef.current = null;
      // 卸载清理加入同一串行队列；失败不得触发 React setter。
      void revokeBackendAuth("组件卸载", false);
    };
  }, [revokeBackendAuth]);

  // 执行扫描：显式用户动作，不自动触发
  const handleScan = useCallback(async () => {
    if (!authorized) {
      setScanError("未授权扫描");
      setPhase("failure");
      return;
    }
    if (processRunning) {
      setScanError("TRAE 正在运行，无法扫描");
      setPhase("failure");
      return;
    }
    setPhase("scanning");
    setScanError(null);
    planGenerationRef.current += 1;
    setSyncPlan(null);
    setPlanLoading(false);
    try {
      const processState: ProcessRunningState = processRunning ? "running" : "not_running";
      // R7：使用后端授权返回的 canonical fixture_root，确保与授权范围一致
      const outcome = await invoke<ScanOutcomeDto>("scan_history", {
        fixtureRoot: authorizedFixtureRoot,
        dbRelativePath,
        processState,
      });
      if (outcome.kind === "success") {
        // 扫描成功后刷新浏览
        try {
          const result = await invoke<BrowseResultDto>("browse_history");
          setSyncPlan(null);
          setBrowseResult(result);
          setBrowseError(null);
          const totalVisible =
            result.summary.visible_account_count +
            result.summary.visible_project_count +
            result.summary.visible_session_count;
          setPhase(totalVisible === 0 ? "empty" : "success");
        } catch (e) {
          setBrowseError(String(e));
          setPhase("success");
        }
      } else if (outcome.kind === "deduplicated") {
        // 去重也刷新浏览
        try {
          const result = await invoke<BrowseResultDto>("browse_history");
          setSyncPlan(null);
          setBrowseResult(result);
          setBrowseError(null);
          const totalVisible =
            result.summary.visible_account_count +
            result.summary.visible_project_count +
            result.summary.visible_session_count;
          setPhase(totalVisible === 0 ? "empty" : "success");
        } catch (e) {
          setBrowseError(String(e));
          setPhase("success");
        }
      } else {
        // failed：结构化原因，不携带 secret
        setScanError(renderScanFailure(outcome.reason));
        setPhase("failure");
      }
    } catch (e) {
      setScanError(String(e));
      setPhase("failure");
    }
  }, [authorized, processRunning, authorizedFixtureRoot, dbRelativePath]);

  // 点击会话标题：只打开预览，不改选择（AC9）
  const handleSessionClick = useCallback(async (session: BrowseSessionNodeDto) => {
    setPreviewSession(session.session_identity);
    setPreviewLoading(true);
    setPreviewData(null);
    try {
      const preview = await invoke<ConversationPreviewDto | null>("read_conversation", {
        session: session.session_identity,
      });
      setPreviewData(preview);
    } catch (e) {
      setPreviewData(null);
    } finally {
      setPreviewLoading(false);
    }
  }, []);

  // 搜索
  const handleSearch = useCallback(async () => {
    if (searchQuery.trim().length === 0) {
      setSearchError("搜索查询不能为空");
      return;
    }
    setSearchError(null);
    try {
      const hits = await invoke<readonly SearchHitDto[]>("search_history", {
        query: searchQuery,
      });
      setSearchResults(hits);
    } catch (e) {
      setSearchError(String(e));
      setSearchResults(null);
    }
  }, [searchQuery]);

  // 从搜索结果打开预览（不改选择——AC9）
  const handleSearchHitClick = useCallback(
    async (hit: SearchHitDto) => {
      setPreviewSession(hit.session_identity);
      setPreviewLoading(true);
      setPreviewData(null);
      try {
        const preview = await invoke<ConversationPreviewDto | null>("read_conversation", {
          session: hit.session_identity,
        });
        setPreviewData(preview);
      } catch {
        setPreviewData(null);
      } finally {
        setPreviewLoading(false);
      }
    },
    [],
  );

  // 按选中账号筛选项目
  const filteredProjects = useMemo(() => {
    if (!browseResult) return [];
    if (!selectedAccount) return browseResult.projects;
    // display_owner 与账号 user_id 一致时归属该账号
    return browseResult.projects.filter((p) => p.display_owner === selectedAccount);
  }, [browseResult, selectedAccount]);

  // 按选中项目筛选会话
  const filteredSessions = useMemo(() => {
    if (!browseResult) return [];
    if (!selectedProject) return browseResult.sessions;
    // 会话归属项目通过 project_id 关联
    return browseResult.sessions.filter(
      (s) => s.project_id === selectedProject,
    );
  }, [browseResult, selectedProject]);

  const summary = browseResult?.summary;

  const sessionKey = useCallback(
    (session: SessionIdentityDto) =>
      `${session.product_history_namespace}:${session.original_session_id}`,
    [],
  );

  const toggleStringSelection = useCallback(
    (value: string, selected: readonly string[], setSelected: (next: readonly string[]) => void) => {
      setSelected(
        selected.includes(value)
          ? selected.filter((item) => item !== value)
          : [...selected, value],
      );
      planGenerationRef.current += 1;
      setSyncPlan(null);
      setPlanLoading(false);
    },
    [],
  );

  const toggleSessionSelection = useCallback(
    (session: SessionIdentityDto) => {
      const key = sessionKey(session);
      setSelectedSessions((current) =>
        current.some((item) => sessionKey(item) === key)
          ? current.filter((item) => sessionKey(item) !== key)
          : [...current, session],
      );
      planGenerationRef.current += 1;
      setSyncPlan(null);
      setPlanLoading(false);
    },
    [sessionKey],
  );

  const selectedSessionKeys = useMemo(() => {
    if (!browseResult) return new Set<string>();
    if (scopeMode === "all") {
      return new Set(browseResult.sessions.map((session) => sessionKey(session.session_identity)));
    }
    const selectedProjectIds = new Set(selectedProjects);
    for (const project of browseResult.projects) {
      if (selectedAccounts.includes(project.display_owner)) {
        selectedProjectIds.add(project.project_id);
      }
    }
    const keys = new Set(selectedSessions.map(sessionKey));
    for (const session of browseResult.sessions) {
      if (selectedProjectIds.has(session.project_id)) {
        keys.add(sessionKey(session.session_identity));
      }
    }
    return keys;
  }, [
    browseResult,
    scopeMode,
    selectedAccounts,
    selectedProjects,
    selectedSessions,
    sessionKey,
  ]);

  const handleBuildPlan = useCallback(async () => {
    if (!authorized || !browseResult) return;
    const scope: SyncScopeDto =
      scopeMode === "all"
        ? { kind: "all_history" }
        : {
            kind: "custom",
            account_ids: selectedAccounts,
            project_ids: selectedProjects,
            session_ids: selectedSessions,
          };
    setPlanLoading(true);
    setPlanError(null);
    const generation = (planGenerationRef.current += 1);
    try {
      const plan = await invoke<SyncPlanDto>("build_sync_plan", { scope });
      if (mountedRef.current && generation === planGenerationRef.current) {
        setSyncPlan(plan);
      }
    } catch (error) {
      if (mountedRef.current && generation === planGenerationRef.current) {
        setSyncPlan(null);
        setPlanError(String(error));
      }
    } finally {
      if (mountedRef.current && generation === planGenerationRef.current) {
        setPlanLoading(false);
      }
    }
  }, [
    authorized,
    browseResult,
    scopeMode,
    selectedAccounts,
    selectedProjects,
    selectedSessions,
  ]);

  const syncableCount = useMemo(() => {
    if (!syncPlan || !browseResult) return 0;
    return syncPlan.actions.reduce((count, action) => {
      if (action.kind === "attach_sessions") return count + action.session_ids.length;
      return (
        count +
        browseResult.sessions.filter((session) => session.project_id === action.project_id).length
      );
    }, 0);
  }, [syncPlan, browseResult]);

  const alreadyCurrentCount = useMemo(
    () => countExcludedSessions(syncPlan, browseResult, (reason) => reason === "already_current"),
    [syncPlan, browseResult],
  );

  const needsProcessingCount = useMemo(
    () => countExcludedSessions(syncPlan, browseResult, (reason) => reason !== "already_current"),
    [syncPlan, browseResult],
  );

  return (
    <section className="workbench" role="region" aria-label="历史库">
      {/* 头部：摘要 + 查找新历史 */}
      <div className="workbench__header">
        <h2>历史库</h2>
        <div className="workbench__summary" data-testid="history-summary">
          <span>账号 {summary?.visible_account_count ?? 0}</span>
          <span>项目 {summary?.visible_project_count ?? 0}</span>
          <span>对话 {summary?.visible_session_count ?? 0}</span>
        </div>
        <button
          type="button"
          className="btn btn--primary"
          onClick={handleScan}
          disabled={phase === "scanning" || !authorized || processRunning}
          aria-disabled={phase === "scanning" || !authorized || processRunning}
          data-testid="scan-history-button"
        >
          {phase === "scanning" ? "扫描中…" : "查找新历史"}
        </button>
      </div>

      {/* 授权表单：未授权时显示 */}
      {phase === "idle" && (
        <div className="workbench__auth" data-testid="auth-panel">
          <h3>扫描授权</h3>
          <p className="workbench__hint">
            P1 fixture 模式：扫描需要明确授权并确认 TRAE 未运行。
          </p>
          <div className="workbench__form">
            <label className="workbench__field">
              fixture 路径
              <input
                type="text"
                value={fixtureRoot}
                onChange={(e) => handleFixtureRootChange(e.target.value)}
                placeholder="例如：%LOCALAPPDATA%\Trae Sync\tests\fixture-xxx"
                data-testid="history-fixture-root-input"
              />
            </label>
            <label className="workbench__field">
              数据库相对路径
              <input
                type="text"
                value={dbRelativePath}
                onChange={(e) => handleDbRelativePathChange(e.target.value)}
                data-testid="history-db-path-input"
              />
            </label>
            <label className="workbench__check">
              <input
                type="checkbox"
                checked={!processRunning}
                onChange={(e) => setProcessRunning(!e.target.checked)}
                data-testid="trae-not-running-check"
              />
              确认 TRAE 已关闭
            </label>
            <label className="workbench__check">
              <input
                type="checkbox"
                checked={authorizeChecked}
                onChange={(e) => handleAuthorizeToggle(e.target.checked)}
                disabled={!canAuthorize}
                data-testid="authorize-check"
              />
              {authorizationPending ? "授权中…（点击取消）" : "授权扫描指定 fixture 路径"}
            </label>
          </div>
        </div>
      )}

      {/* 扫描中状态 */}
      {phase === "scanning" && (
        <div className="workbench__scanning" data-testid="scanning-state" role="status">
          正在扫描并捕获不可变快照…
        </div>
      )}

      {/* 扫描失败状态 */}
      {phase === "failure" && (
        <div className="workbench__failure" role="alert" data-testid="failure-state">
          <p>扫描失败：{scanError ?? "未知原因"}</p>
          <button
            type="button"
            className="btn"
            onClick={() => setPhase("idle")}
            data-testid="retry-scan-button"
          >
            重新授权
          </button>
        </div>
      )}

      {/* 扫描成功但空 */}
      {phase === "empty" && (
        <div className="workbench__empty" data-testid="empty-state">
          <p>扫描完成，但目录库中暂无可见历史。{honestStatus}</p>
          <button type="button" className="btn" onClick={() => setPhase("idle")}>
            重新扫描
          </button>
        </div>
      )}

      {/* 扫描成功且有数据：三栏布局 */}
      {(phase === "success" || phase === "empty") && browseResult && (
        <>
        {browseError && (
          <p className="workbench__error" role="alert">
            浏览失败：{browseError}
          </p>
        )}
        <div className="workbench__body">
          {/* 左栏：账号/项目树 */}
          <div
            className="workbench__tree"
            role="tree"
            aria-label="账号与项目树"
            data-testid="account-project-tree"
          >
            <h3>账号与项目</h3>
            {browseResult.accounts.length === 0 ? (
              <p className="workbench__empty">无可见账号</p>
            ) : (
              <ul role="group">
                {browseResult.accounts.map((acc) => (
                  <li key={acc.user_id} role="treeitem">
                    <div className="workbench__select-row">
                      {scopeMode === "custom" && (
                        <input
                          type="checkbox"
                          checked={selectedAccounts.includes(acc.user_id)}
                          onChange={() =>
                            toggleStringSelection(
                              acc.user_id,
                              selectedAccounts,
                              setSelectedAccounts,
                            )
                          }
                          aria-label={`选择账号 ${acc.display_label}`}
                        />
                      )}
                      <button
                        type="button"
                        className="workbench__node"
                        onClick={() => {
                          setSelectedAccount(
                            selectedAccount === acc.user_id ? null : acc.user_id,
                          );
                          setSelectedProject(null);
                        }}
                        aria-expanded={selectedAccount === acc.user_id}
                        data-testid={`account-${acc.user_id}`}
                      >
                        {acc.display_label}（项目 {acc.project_count}，对话{" "}
                        {acc.session_count}）
                      </button>
                    </div>
                  </li>
                ))}
              </ul>
            )}
            {filteredProjects.length > 0 && (
              <ul role="group" className="workbench__subtree">
                {filteredProjects.map((proj) => (
                  <li key={proj.project_id} role="treeitem">
                    <div className="workbench__select-row workbench__select-row--child">
                      {scopeMode === "custom" && (
                        <input
                          type="checkbox"
                          checked={selectedProjects.includes(proj.project_id)}
                          onChange={() =>
                            toggleStringSelection(
                              proj.project_id,
                              selectedProjects,
                              setSelectedProjects,
                            )
                          }
                          aria-label={`选择项目 ${proj.display_name}`}
                        />
                      )}
                      <button
                        type="button"
                        className="workbench__node workbench__node--child"
                        onClick={() => {
                          setSelectedProject(
                            selectedProject === proj.project_id ? null : proj.project_id,
                          );
                        }}
                        aria-expanded={selectedProject === proj.project_id}
                        data-testid={`project-${proj.project_id}`}
                      >
                        {proj.display_name}（归属 {proj.display_owner}，对话{" "}
                        {proj.session_count}）
                      </button>
                    </div>
                  </li>
                ))}
              </ul>
            )}
          </div>

          {/* 中栏：会话列表 + 搜索 */}
          <div
            className="workbench__sessions"
            role="region"
            aria-label="对话列表"
            data-testid="session-list"
          >
            <h3>对话</h3>
            <div className="workbench__search">
              <input
                type="text"
                value={searchQuery}
                onChange={(e) => setSearchQuery(e.target.value)}
                placeholder="搜索消息内容…"
                data-testid="search-input"
              />
              <button
                type="button"
                className="btn"
                onClick={handleSearch}
                disabled={searchQuery.trim().length === 0}
                data-testid="search-button"
              >
                搜索
              </button>
            </div>
            {searchError && (
              <p className="workbench__error" role="alert">
                {searchError}
              </p>
            )}

            {/* 搜索结果 */}
            {searchResults !== null && (
              <div className="workbench__search-results" data-testid="search-results">
                <h4>搜索结果（{searchResults.length}）</h4>
                {searchResults.length === 0 ? (
                  <p>无匹配结果</p>
                ) : (
                  <ul>
                    {searchResults.map((hit) => (
                      <li key={`${hit.message_id}-${hit.session_identity.original_session_id}`}>
                        <button
                          type="button"
                          className="workbench__search-hit"
                          onClick={() => handleSearchHitClick(hit)}
                          data-testid={`search-hit-${hit.message_id}`}
                        >
                          <span className="workbench__hit-title">{hit.title}</span>
                          <span className="workbench__hit-context">
                            [{hit.role}] {hit.content_excerpt}
                          </span>
                        </button>
                      </li>
                    ))}
                  </ul>
                )}
              </div>
            )}

            {/* 会话列表（点击只预览，不改选择——AC9） */}
            {filteredSessions.length === 0 ? (
              <p className="workbench__empty">无可见对话</p>
            ) : (
              <ul className="workbench__session-list">
                {filteredSessions.map((s) => {
                  const isPreviewing =
                    previewSession?.original_session_id ===
                    s.session_identity.original_session_id;
                  return (
                    <li key={s.session_identity.original_session_id}>
                      <div className="workbench__select-row">
                        {scopeMode === "custom" && (
                          <input
                            type="checkbox"
                            checked={selectedSessions.some(
                              (item) => sessionKey(item) === sessionKey(s.session_identity),
                            )}
                            onChange={() => toggleSessionSelection(s.session_identity)}
                            aria-label={`选择对话 ${s.title}`}
                          />
                        )}
                        <button
                          type="button"
                          className={
                            "workbench__session" +
                            (isPreviewing ? " workbench__session--active" : "")
                          }
                          onClick={() => handleSessionClick(s)}
                          data-testid={`session-${s.session_identity.original_session_id}`}
                        >
                          <span className="workbench__session-title">{s.title}</span>
                          <span className="workbench__session-meta">
                            {s.message_count} 条消息
                          </span>
                        </button>
                      </div>
                    </li>
                  );
                })}
              </ul>
            )}
          </div>

          {/* 右栏：对话预览 */}
          <div
            className="workbench__preview"
            role="region"
            aria-label="对话内容预览"
            data-testid="conversation-preview"
          >
            <h3>对话内容</h3>
            {previewLoading && <p data-testid="preview-loading">加载中…</p>}
            {!previewLoading && !previewSession && (
              <p className="workbench__empty">点击对话标题查看内容（不会改变选择）</p>
            )}
            {!previewLoading && previewSession && previewData && (
              <div data-testid="preview-content">
                <h4>{previewData.title}</h4>
                <p className="workbench__preview-meta">
                  共 {previewData.total_message_count} 条消息
                </p>
                <ul className="workbench__messages">
                  {previewData.messages.map((m) => (
                    <li
                      key={m.message_id}
                      className="workbench__message"
                      data-testid={`message-${m.message_id}`}
                    >
                      <span className="workbench__message-role">{m.role}</span>
                      <span className="workbench__message-content">
                        {m.content_excerpt}
                      </span>
                    </li>
                  ))}
                </ul>
              </div>
            )}
            {!previewLoading && previewSession && !previewData && (
              <p className="workbench__empty">无法加载对话内容</p>
            )}
          </div>
        </div>
        </>
      )}

      {/* T05：同步范围与不可变计划预览，不执行写入。 */}
      <div className="workbench__plan" role="region" aria-label="同步计划">
        <h3>同步计划</h3>
        <div className="workbench__scope" role="group" aria-label="同步范围">
          <label>
            <input
              type="radio"
              name="sync-scope"
              checked={scopeMode === "all"}
              onChange={() => {
                setScopeMode("all");
                planGenerationRef.current += 1;
                setSyncPlan(null);
                setPlanLoading(false);
              }}
            />
            全部历史
          </label>
          <label>
            <input
              type="radio"
              name="sync-scope"
              checked={scopeMode === "custom"}
              onChange={() => {
                setScopeMode("custom");
                planGenerationRef.current += 1;
                setSyncPlan(null);
                setPlanLoading(false);
              }}
            />
            自定义选择
          </label>
        </div>
        <dl className="plan-summary">
          <dt>已选择</dt>
          <dd data-testid="plan-selected">{selectedSessionKeys.size}</dd>
          <dt>本次可同步</dt>
          <dd data-testid="plan-syncable">{syncableCount}</dd>
          <dt>已在当前账号</dt>
          <dd data-testid="plan-already-current">{alreadyCurrentCount}</dd>
          <dt>需要处理</dt>
          <dd data-testid="plan-needs-processing">{needsProcessingCount}</dd>
          <dt>目标账号</dt>
          <dd data-testid="plan-target-account">{syncPlan?.current_user_id ?? "生成后确认"}</dd>
        </dl>
        <p className="workbench__hint">
          计划会把项目归属调整到当前账号，或把选中对话挂到可靠的目标项目；不会复制成两份账号历史。
        </p>
        {planError && <p className="workbench__error" role="alert">{planError}</p>}
        {syncPlan && (
          <div className="workbench__plan-result" data-testid="sync-plan-result">
            <p>动作 {syncPlan.actions.length}，排除 {syncPlan.exclusions.length}</p>
            <ul>
              {syncPlan.actions.map((action, index) => (
                <li key={`${action.kind}-${index}`}>{renderPlanAction(action)}</li>
              ))}
              {syncPlan.exclusions.map((item, index) => (
                <li key={`${item.project_id}-${index}`}>
                  排除 {item.project_id}：{renderPlanExclusion(item.reason)}
                </li>
              ))}
            </ul>
          </div>
        )}
        <button
          type="button"
          className="btn btn--primary"
          onClick={handleBuildPlan}
          disabled={
            planLoading ||
            !authorized ||
            !browseResult ||
            (scopeMode === "custom" && selectedSessionKeys.size === 0)
          }
          data-testid="build-sync-plan-button"
        >
          {planLoading ? "生成中…" : "生成同步计划"}
        </button>
        <button
          type="button"
          className="btn"
          disabled={!capabilities.sync_enabled || !syncPlan}
          aria-disabled={!capabilities.sync_enabled || !syncPlan}
          data-testid="sync-button"
        >
          检查并安全同步（后续任务包）
        </button>
      </div>

      <p className="workbench__honest-status" role="status">
        {honestStatus}
      </p>
    </section>
  );
}

function renderPlanAction(action: SyncPlanDto["actions"][number]): string {
  if (action.kind === "follow_project") {
    return `跟随整个项目 ${action.project_id} 到账号 ${action.to_user_id}`;
  }
  return `挂接 ${action.session_ids.length} 条对话到项目 ${action.target_project_id}`;
}

function renderPlanExclusion(reason: string): string {
  const labels: Record<string, string> = {
    already_current: "已在当前账号",
    project_identity_conflict: "项目身份冲突",
    project_identity_unknown: "项目身份证据不足",
    archived_only: "仅有归档内容",
    deleted_project: "项目已删除",
    schema_incompatible: "数据库结构不兼容",
    session_version_unavailable: "对话版本不可用",
    partial_project_requires_target: "目标无同项目，请选择整个项目",
  };
  return labels[reason] ?? reason;
}

function countExcludedSessions(
  plan: SyncPlanDto | null,
  browseResult: BrowseResultDto | null,
  include: (reason: string) => boolean,
): number {
  if (!plan || !browseResult) return 0;
  const sessionKeys = new Set<string>();
  for (const exclusion of plan.exclusions) {
    if (!include(exclusion.reason)) continue;
    if (exclusion.session_id) {
      sessionKeys.add(
        `${exclusion.session_id.product_history_namespace}:${exclusion.session_id.original_session_id}`,
      );
      continue;
    }
    for (const session of browseResult.sessions) {
      if (session.project_id === exclusion.project_id) {
        sessionKeys.add(
          `${session.session_identity.product_history_namespace}:${session.session_identity.original_session_id}`,
        );
      }
    }
  }
  return sessionKeys.size;
}

// 渲染扫描失败原因：结构化，不携带 secret
function renderScanFailure(reason: string): string {
  const reasonMap: Record<string, string> = {
    not_authorized: "未授权扫描",
    process_running: "TRAE 正在运行",
    database_missing: "数据库文件不存在",
    schema_incompatible: "schema 不兼容",
    storage_root_unavailable: "存储根不可用",
    source_set_drift: "捕获前后源文件集漂移",
    catalog_transaction_failed: "目录库事务失败",
    catalog_key_missing: "目录库密钥未配置",
  };
  return reasonMap[reason] ?? reason;
}
