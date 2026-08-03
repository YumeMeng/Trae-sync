import { useCallback, useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import type {
  BrowseResultDto,
  BrowseSessionNodeDto,
  ConversationPreviewDto,
  ProcessRunningState,
  ScanOutcomeDto,
  SearchHitDto,
  SessionIdentityDto,
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

  // R7：已授权的 canonical fixture_root——由后端 grant_scan_authorization 返回。
  // 前端不再持有"自封"的授权；只有后端成功授权后此值才非空。
  // 取消授权或路径变化时此值清空，旧授权在后端被撤销。
  const [authorizedFixtureRoot, setAuthorizedFixtureRoot] = useState<string | null>(null);

  // 扫描授权前置条件：fixture_root 与 db_relative_path 非空
  // storage_root 由后端 env 控制，前端不可注入，未配置时后端返回错误
  const canAuthorize =
    fixtureRoot.trim().length > 0 &&
    dbRelativePath.trim().length > 0;

  // R7：authorized 派生自后端授权状态——只有 authorizedFixtureRoot 非空时为 true
  const authorized = authorizedFixtureRoot !== null;

  // R7：用户勾选授权时调用后端 grant_scan_authorization，建立后端授权状态机
  // 取消勾选时调用 revoke_scan_authorization 撤销后端授权
  const handleAuthorizeToggle = useCallback(
    async (checked: boolean) => {
      if (checked) {
        if (!canAuthorize) {
          // 前置条件不满足——拒绝建立授权（按钮应已禁用，此处防御）
          return;
        }
        try {
          // 调用后端建立授权——传入当前 fixtureRoot 与 dbRelativePath
          // 后端会 canonicalize 路径并存储 AuthorizationState::Authorized
          const canonical = await invoke<string>("grant_scan_authorization", {
            fixtureRoot,
            dbRelativePath,
          });
          // 只有后端成功后才进入已授权状态
          setAuthorizedFixtureRoot(canonical);
          setScanError(null);
        } catch (e) {
          // 授权失败：保持未授权并显示结构化错误
          setAuthorizedFixtureRoot(null);
          setScanError(String(e));
          setPhase("failure");
        }
      } else {
        // 取消授权：撤销后端授权，清空本地状态
        try {
          await invoke<void>("revoke_scan_authorization");
        } catch {
          // 撤销失败不阻塞——本地状态仍清空，旧授权在后端可能残留但已无本地凭证
        }
        setAuthorizedFixtureRoot(null);
      }
    },
    [canAuthorize, fixtureRoot, dbRelativePath],
  );

  // R7：路径变化时撤销旧授权——旧授权不得继续有效
  // 当 fixtureRoot 或 dbRelativePath 改变时，已授权状态自动失效
  const handleFixtureRootChange = useCallback(
    (value: string) => {
      setFixtureRoot(value);
      if (authorizedFixtureRoot !== null) {
        // 路径变化导致授权范围变化——撤销后端授权
        setAuthorizedFixtureRoot(null);
        invoke<void>("revoke_scan_authorization").catch(() => {
          // 撤销失败不阻塞本地状态更新
        });
      }
    },
    [authorizedFixtureRoot],
  );

  const handleDbRelativePathChange = useCallback(
    (value: string) => {
      setDbRelativePath(value);
      if (authorizedFixtureRoot !== null) {
        // 路径变化导致授权范围变化——撤销后端授权
        setAuthorizedFixtureRoot(null);
        invoke<void>("revoke_scan_authorization").catch(() => {
          // 撤销失败不阻塞本地状态更新
        });
      }
    },
    [authorizedFixtureRoot],
  );

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
                checked={authorized}
                onChange={(e) => handleAuthorizeToggle(e.target.checked)}
                disabled={!canAuthorize}
                data-testid="authorize-check"
              />
              授权扫描指定 fixture 路径
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
                  </li>
                ))}
              </ul>
            )}
            {filteredProjects.length > 0 && (
              <ul role="group" className="workbench__subtree">
                {filteredProjects.map((proj) => (
                  <li key={proj.project_id} role="treeitem">
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

      {/* 同步计划区域：P1 阶段只展示禁用状态 */}
      <div className="workbench__plan" role="region" aria-label="同步计划">
        <h3>同步计划</h3>
        <dl className="plan-summary">
          <dt>已选择</dt>
          <dd data-testid="plan-selected">0</dd>
          <dt>本次可同步</dt>
          <dd data-testid="plan-syncable">0</dd>
          <dt>已在当前账号</dt>
          <dd data-testid="plan-already-current">0</dd>
          <dt>需要处理</dt>
          <dd data-testid="plan-needs-processing">0</dd>
        </dl>
        <button
          type="button"
          className="btn btn--primary"
          disabled={!capabilities.sync_enabled}
          aria-disabled={!capabilities.sync_enabled}
          data-testid="sync-button"
        >
          检查并安全同步
        </button>
      </div>

      <p className="workbench__honest-status" role="status">
        {honestStatus}
      </p>
    </section>
  );
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
