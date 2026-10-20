import { useCallback, useEffect, useMemo, useState } from "react";
import { Activity, Database, RefreshCw, Trash2 } from "lucide-react";
import type {
  WorkBuddyCwdGroupDto,
  WorkBuddyLibraryCheckupDto,
  WorkBuddyLibraryStatsDto,
  WorkBuddySessionMessagesDto,
} from "../types/workbuddyLibrary";
import type { WorkBuddyLoginSummaryDto } from "../types/product";
import {
  deleteWorkBuddySessions,
  fetchWorkBuddyLibraryCheckup,
  fetchWorkBuddyLibraryHistory,
  fetchWorkBuddyLibraryStats,
  fetchWorkBuddySessionMessages,
  listWorkBuddyAccounts,
  mergeWorkBuddyCwdProjects,
  purgeWorkBuddySessions,
} from "../platform/workbuddyAdapter";
import { ConfirmDialog } from "../components/shared/ConfirmDialog";
import { EmptyState } from "../components/shared/EmptyState";
import { ProductBadge } from "../components/shared/ProductBadge";
import { notify } from "../platform/notify";
import { safeUiErrorMessage } from "../utils/safeUiError";

/**
 * WB 对话库页（P19-B6，架构草案 §4.5 WbLibraryDetail；IA §1.4 库页模板）：
 * 统计卡 + 会话浏览（uid 过滤 + cwd 分组 + 软删开关）+ 会话消息查看（jsonl
 * 解析时间线）+ 软删除/彻底删除（单次确认，备份前置）+ 体检卡（五项）+
 * cwd 归并入口（单次确认）。
 * 写红线：cloud 行只读；删除仅在账号过滤激活时可用（user_id 归属重校验锚）；
 * 工具不碰云端会话。
 */
export function WorkBuddyLibraryPage({ active }: { active: boolean }) {
  const [stats, setStats] = useState<WorkBuddyLibraryStatsDto | null>(null);
  const [groups, setGroups] = useState<readonly WorkBuddyCwdGroupDto[] | null>(null);
  const [accounts, setAccounts] = useState<readonly WorkBuddyLoginSummaryDto[]>([]);
  const [uidFilter, setUidFilter] = useState<string>("");
  const [includeDeleted, setIncludeDeleted] = useState(false);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);

  // 会话消息详情（单会话展开视图）。
  const [openSessionId, setOpenSessionId] = useState<string | null>(null);
  const [messages, setMessages] = useState<WorkBuddySessionMessagesDto | null>(null);
  const [messagesLoading, setMessagesLoading] = useState(false);

  // 多选与写操作（单次确认；仅账号过滤激活时可用）。
  const [selectedIds, setSelectedIds] = useState<readonly string[]>([]);
  const [writeBusy, setWriteBusy] = useState(false);
  const [softConfirmOpen, setSoftConfirmOpen] = useState(false);
  const [purgeConfirmOpen, setPurgeConfirmOpen] = useState(false);

  // 体检（五项）与归并确认。
  const [checkup, setCheckup] = useState<WorkBuddyLibraryCheckupDto | null>(null);
  const [checking, setChecking] = useState(false);
  const [mergeCandidate, setMergeCandidate] = useState<readonly string[] | null>(null);
  const [mergeBusy, setMergeBusy] = useState(false);

  const loadLibrary = useCallback(async (initial: boolean) => {
    if (initial) setLoading(true);
    try {
      const [statsResult, historyResult, accountsResult] = await Promise.allSettled([
        fetchWorkBuddyLibraryStats(),
        fetchWorkBuddyLibraryHistory(uidFilter === "" ? null : uidFilter, includeDeleted),
        listWorkBuddyAccounts(),
      ]);
      if (statsResult.status === "fulfilled") setStats(statsResult.value);
      if (historyResult.status === "fulfilled") {
        setGroups(historyResult.value);
        setLoadError(null);
      } else {
        setLoadError(safeUiErrorMessage(historyResult.reason, "对话库暂不可读取，请稍后重试。"));
      }
      if (accountsResult.status === "fulfilled") setAccounts(accountsResult.value);
    } finally {
      if (initial) setLoading(false);
    }
  }, [uidFilter, includeDeleted]);

  const loadCheckup = useCallback(async () => {
    setChecking(true);
    try {
      setCheckup(await fetchWorkBuddyLibraryCheckup());
    } catch (reason: unknown) {
      notify.error(`WorkBuddy：库体检未完成——${safeUiErrorMessage(reason, "请稍后重试。")}`);
    } finally {
      setChecking(false);
    }
  }, []);

  useEffect(() => {
    if (!active) return;
    void loadLibrary(true);
    void loadCheckup();
  }, [active, loadLibrary, loadCheckup]);

  const openMessages = useCallback(async (sessionId: string) => {
    setOpenSessionId(sessionId);
    setMessages(null);
    setMessagesLoading(true);
    try {
      setMessages(await fetchWorkBuddySessionMessages(sessionId, uidFilter === "" ? null : uidFilter));
    } catch (reason: unknown) {
      notify.error(`WorkBuddy：消息读取未完成——${safeUiErrorMessage(reason, "请稍后重试。")}`);
      setOpenSessionId(null);
    } finally {
      setMessagesLoading(false);
    }
  }, [uidFilter]);

  const toggleSelect = useCallback((sessionId: string) => {
    setSelectedIds((current) => current.includes(sessionId)
      ? current.filter((id) => id !== sessionId)
      : [...current, sessionId]);
  }, []);

  // cloud 行只读（写红线：工具不碰云端会话）。
  const selectableIds = useMemo(() => {
    const cloud = new Set(
      (groups ?? []).flatMap((group) => group.sessions.filter((row) => row.transport === "cloud").map((row) => row.session_id)),
    );
    return selectedIds.filter((id) => !cloud.has(id));
  }, [groups, selectedIds]);

  const runSoftDelete = useCallback(async () => {
    if (uidFilter === "" || selectableIds.length === 0 || writeBusy) return;
    setSoftConfirmOpen(false);
    setWriteBusy(true);
    try {
      const receipt = await deleteWorkBuddySessions(selectableIds, uidFilter);
      notify.success(`WorkBuddy：已软删除 ${receipt.deleted_sessions} 条会话（可逆）${receipt.tail.restarted ? "，已重启客户端" : ""}。`);
      setSelectedIds([]);
      await loadLibrary(false);
      await loadCheckup();
    } catch (reason: unknown) {
      notify.error(`WorkBuddy：软删除未完成——${safeUiErrorMessage(reason, "数据未被改动，请稍后重试。")}`);
    } finally {
      setWriteBusy(false);
    }
  }, [loadCheckup, loadLibrary, selectableIds, uidFilter, writeBusy]);

  const runPurge = useCallback(async () => {
    if (uidFilter === "" || selectableIds.length === 0 || writeBusy) return;
    setPurgeConfirmOpen(false);
    setWriteBusy(true);
    try {
      const receipt = await purgeWorkBuddySessions(selectableIds, uidFilter);
      notify.success(
        `WorkBuddy：已彻底删除 ${receipt.purged_sessions} 条会话（删除正文 ${receipt.removed_body_files} 份，前置备份 ${receipt.backup_path}）${receipt.tail.restarted ? "，已重启客户端" : ""}。`,
      );
      setSelectedIds([]);
      await loadLibrary(false);
      await loadCheckup();
    } catch (reason: unknown) {
      notify.error(`WorkBuddy：彻底删除未完成——${safeUiErrorMessage(reason, "数据未被改动，请稍后重试。")}`);
    } finally {
      setWriteBusy(false);
    }
  }, [loadCheckup, loadLibrary, selectableIds, uidFilter, writeBusy]);

  const runMerge = useCallback(async (candidate: readonly string[]) => {
    setMergeCandidate(null);
    setMergeBusy(true);
    try {
      const receipt = await mergeWorkBuddyCwdProjects(candidate[0], candidate.slice(1));
      notify.success(
        `WorkBuddy：已归并 ${receipt.moved_sessions} 条会话到 ${candidate[0]}（正文迁移 ${receipt.moved_body_files} 份）${receipt.tail.restarted ? "，已重启客户端" : ""}。`,
      );
      await loadLibrary(false);
      await loadCheckup();
    } catch (reason: unknown) {
      notify.error(`WorkBuddy：归并未完成——${safeUiErrorMessage(reason, "数据未被改动，请稍后重试。")}`);
    } finally {
      setMergeBusy(false);
    }
  }, [loadCheckup, loadLibrary]);

  const formatTime = (unixSeconds: number | null): string =>
    unixSeconds ? new Date(unixSeconds * 1000).toLocaleDateString() : "—";

  return (
    <section className="workbuddy-page" role="region" aria-label="对话库">
      <header className="page-header">
        <div className="page-header__copy">
          <h1 data-page-title="master-library" tabIndex={-1}>对话库</h1>
          <ProductBadge productId="workbuddy" />
        </div>
        <div className="page-header__actions">
          <button
            type="button"
            className="btn"
            onClick={() => void loadLibrary(false)}
            disabled={loading || writeBusy}
            data-testid="workbuddy-library-refresh"
            title="重读会话索引与统计（只读）"
          >
            <RefreshCw size={15} className={loading ? "icon-spin" : undefined} aria-hidden="true" />刷新
          </button>
        </div>
      </header>

      {loading ? (
        <EmptyState role="status" className="workbuddy-page__empty" description="正在读取对话库…" />
      ) : loadError ? (
        <EmptyState
          role="alert"
          className="workbuddy-page__empty"
          title="无法读取对话库"
          description={loadError}
          action={
            <button type="button" className="btn btn--primary" onClick={() => void loadLibrary(true)}>
              重新读取
            </button>
          }
        />
      ) : (
        <>
          {/* 统计卡（IA §1.4 库内统计）。 */}
          {stats && (
            <div className="workbuddy-card" data-testid="workbuddy-library-stats">
              <div className="workbuddy-card__title">库内统计</div>
              <div className="overview-page__stats">
                <div className="overview-stat">
                  <span className="overview-stat__value">{stats.session_count}</span>
                  <span className="overview-stat__label">会话</span>
                </div>
                <div className="overview-stat">
                  <span className="overview-stat__value">{stats.project_count}</span>
                  <span className="overview-stat__label">项目（cwd）</span>
                </div>
                <div className="overview-stat">
                  <span className="overview-stat__value">{stats.account_count}</span>
                  <span className="overview-stat__label">账号</span>
                </div>
                <div className="overview-stat" title="软删除会话数（可恢复）">
                  <span className="overview-stat__value">{stats.soft_deleted_session_count}</span>
                  <span className="overview-stat__label">软删除</span>
                </div>
                <div className="overview-stat">
                  <span className="overview-stat__value overview-stat__value--time">
                    {stats.projects_dir_bytes > 0 ? `${Math.round(stats.projects_dir_bytes / 1024)} KB` : "—"}
                  </span>
                  <span className="overview-stat__label">正文字节</span>
                </div>
              </div>
            </div>
          )}

          {/* 会话浏览（uid 过滤 + cwd 分组 + 软删开关）。 */}
          <div className="workbuddy-card" data-testid="workbuddy-library-browser">
            <div className="workbuddy-card__title">会话浏览</div>
            <div className="account-center__list-controls">
              <label className="account-center__target-field">
                <span>账号过滤</span>
                <select
                  value={uidFilter}
                  onChange={(event) => {
                    setUidFilter(event.target.value);
                    setSelectedIds([]);
                    setOpenSessionId(null);
                  }}
                  data-testid="workbuddy-library-uid-filter"
                >
                  <option value="">全部账号</option>
                  {accounts.map((account) => (
                    <option key={account.profile_id} value={account.account_id}>
                      {account.screen_name || "WorkBuddy 账号"}
                    </option>
                  ))}
                </select>
              </label>
              <label className="settings-panel__toggle-row">
                <input
                  type="checkbox"
                  checked={includeDeleted}
                  onChange={(event) => {
                    setIncludeDeleted(event.target.checked);
                    setSelectedIds([]);
                  }}
                  data-testid="workbuddy-library-include-deleted"
                />
                <span className="account-center__profile-copy">
                  <strong>显示软删除会话</strong>
                  <span>软删行可在此视图彻底删除</span>
                </span>
              </label>
            </div>

            {groups !== null && groups.length > 0 ? (
              <ul className="account-list" data-testid="workbuddy-library-groups">
                {groups.map((group) => (
                  <li key={group.escaped_dir}>
                    <div className="account-list__row">
                      <div className="account-item__main">
                        <span className="account-card__avatar" aria-hidden="true">
                          <Database size={14} aria-hidden="true" />
                        </span>
                        <div className="account-item__id">
                          <div className="account-item__name-row">
                            <strong className="account-item__name" title={group.cwd}>{group.cwd}</strong>
                          </div>
                          <div className="account-meta-group">
                            <p className="account-item__meta">
                              {group.session_count} 条会话 · 最近活跃 {formatTime(group.last_active_unix_seconds)}
                            </p>
                          </div>
                        </div>
                      </div>
                    </div>
                    <ul className="account-list" data-testid={`workbuddy-library-group-${group.escaped_dir}`}>
                      {group.sessions.map((session) => (
                        <li key={session.session_id}>
                          <div
                            className={`account-list__row${openSessionId === session.session_id ? " account-list__row--interactive" : ""}`}
                            role={session.transport === "cloud" ? undefined : "button"}
                            tabIndex={session.transport === "cloud" ? undefined : 0}
                            onClick={session.transport === "cloud" ? undefined : () => void openMessages(session.session_id)}
                            onKeyDown={session.transport === "cloud" ? undefined : (event) => {
                              if (event.target === event.currentTarget && event.key === "Enter") {
                                void openMessages(session.session_id);
                              }
                            }}
                            title={session.transport === "cloud" ? "云端会话（只读展示，工具不碰云端）" : "查看会话消息"}
                          >
                            <div className="account-item__main">
                              <label className="account-center__profile-select" onClick={(event) => event.stopPropagation()}>
                                <input
                                  type="checkbox"
                                  checked={selectedIds.includes(session.session_id)}
                                  disabled={session.transport === "cloud"}
                                  onChange={() => toggleSelect(session.session_id)}
                                  data-testid={`workbuddy-library-select-${session.session_id}`}
                                  title={session.transport === "cloud" ? "云端会话不可删除" : "加入选择"}
                                />
                              </label>
                              <div className="account-item__id">
                                <div className="account-item__name-row">
                                  <strong className="account-item__name">{session.title || "未命名会话"}</strong>
                                  {session.transport === "cloud" && (
                                    <span className="slot-badge slot-badge--idle" title="云同步会话：工具只读展示，写操作红线不碰">云端</span>
                                  )}
                                  {session.deleted && (
                                    <span className="slot-badge slot-badge--warn">已软删除</span>
                                  )}
                                </div>
                                <div className="account-meta-group">
                                  <p className="account-item__meta">最近活跃 {formatTime(session.last_active_unix_seconds)}</p>
                                </div>
                              </div>
                            </div>
                            <div className="account-item__side">
                              {openSessionId === session.session_id && <span className="account-item__meta">查看中</span>}
                            </div>
                          </div>
                          {/* 会话消息时间线（jsonl 正文解析）。 */}
                          {openSessionId === session.session_id && (
                            <div className="workbuddy-card" data-testid="workbuddy-library-messages">
                              {messagesLoading ? (
                                <p className="account-item__meta">正在读取正文…</p>
                              ) : messages ? (
                                <>
                                  <div className="workbuddy-card__title">
                                    会话消息（{messages.messages.length} 条
                                    {messages.body_missing ? "；正文缺失" : ""}
                                    {messages.skipped_lines > 0 ? `；跳过 ${messages.skipped_lines} 行未解析` : ""}）
                                  </div>
                                  <ul className="account-center__checkin-results">
                                    {messages.messages.map((message) => (
                                      <li key={message.index}>
                                        <strong>{message.role ?? message.kind}</strong>
                                        <span>{message.text ?? "（无文本内容）"}</span>
                                      </li>
                                    ))}
                                  </ul>
                                </>
                              ) : (
                                <p className="account-item__meta">暂无内容。</p>
                              )}
                            </div>
                          )}
                        </li>
                      ))}
                    </ul>
                  </li>
                ))}
              </ul>
            ) : (
              <EmptyState className="workbuddy-page__empty" description="库中无会话（或当前过滤无匹配）。" />
            )}
          </div>

          {/* 批量写操作行（单次确认；仅账号过滤激活时可用——user_id 归属重校验锚）。 */}
          <div className="checkin-page__action-row" data-testid="workbuddy-library-actions">
            <button
              type="button"
              className="btn"
              onClick={() => setSoftConfirmOpen(true)}
              disabled={uidFilter === "" || selectableIds.length === 0 || writeBusy}
              data-testid="workbuddy-library-soft-delete"
              title={uidFilter === "" ? "先按账号过滤，再执行删除（归属校验需要账号范围）" : "软删除所选会话（可逆）"}
            >
              <Trash2 size={15} aria-hidden="true" />软删除（{selectableIds.length}）
            </button>
            <button
              type="button"
              className="btn btn--danger"
              onClick={() => setPurgeConfirmOpen(true)}
              disabled={uidFilter === "" || selectableIds.length === 0 || writeBusy}
              data-testid="workbuddy-library-purge"
              title="彻底删除所选会话（含正文；备份前置，不可逆）"
            >
              <Trash2 size={15} aria-hidden="true" />彻底删除（{selectableIds.length}）
            </button>
            {uidFilter === "" && (
              <span className="account-item__meta">删除操作需先按账号过滤（归属校验锚）。</span>
            )}
          </div>

          {/* 体检卡（五项 + cwd 归并入口）。 */}
          <div className="workbuddy-card" data-testid="workbuddy-library-checkup">
            <div className="workbuddy-card__title">库体检</div>
            {checkup ? (
              <>
                <ul className="account-center__checkin-results">
                  <li>
                    <strong>孤儿正文</strong>
                    <span>{checkup.orphan_body_files.length} 份（正文有、索引无）</span>
                  </li>
                  <li>
                    <strong>缺失正文</strong>
                    <span>{checkup.missing_body_count} 条（索引有、正文无{checkup.missing_body_samples.length > 0 ? `，如 ${checkup.missing_body_samples[0]}` : ""}）</span>
                  </li>
                  <li>
                    <strong>软删除规模</strong>
                    <span>{checkup.soft_deleted_session_count} 条</span>
                  </li>
                  <li>
                    <strong>外部目录</strong>
                    <span>{checkup.foreign_dirs.length} 个（cwd 不在已知项目集）</span>
                  </li>
                  <li>
                    <strong>归并候选</strong>
                    <span>{checkup.cwd_merge_candidates.length} 组（同一 cwd 多转义形态）</span>
                  </li>
                </ul>
                {checkup.cwd_merge_candidates.length > 0 && (
                  <div className="workbuddy-card__actions">
                    {checkup.cwd_merge_candidates.map((candidate) => (
                      <button
                        key={candidate[0]}
                        type="button"
                        className="btn"
                        onClick={() => setMergeCandidate(candidate)}
                        disabled={mergeBusy}
                        data-testid={`workbuddy-library-merge-${candidate.length}`}
                        title={`把 ${candidate.length - 1} 个变体目录归并到 ${candidate[0]}`}
                      >
                        <Activity size={14} aria-hidden="true" />
                        归并：{candidate[0]}
                      </button>
                    ))}
                  </div>
                )}
              </>
            ) : (
              <p className="account-item__meta">{checking ? "体检中…" : "尚未体检。"}</p>
            )}
            <div className="workbuddy-card__actions">
              <button
                type="button"
                className="btn"
                onClick={() => void loadCheckup()}
                disabled={checking}
                data-testid="workbuddy-library-checkup-button"
              >
                <Activity size={15} className={checking ? "icon-spin" : undefined} aria-hidden="true" />
                立即体检（只读）
              </button>
            </div>
          </div>
        </>
      )}

      {/* 软删除单次确认（可逆；写前关停客户端由后端进程纪律承担）。 */}
      {softConfirmOpen && (
        <ConfirmDialog
          title="软删除会话"
          lines={[
            `将把所选 ${selectableIds.length} 条会话标记为已删除（可恢复）。`,
            "云端会话不会受影响；执行时如客户端在运行会先关闭再重启。",
          ]}
          confirmLabel="软删除"
          busyLabel="删除中…"
          busy={writeBusy}
          onCancel={() => setSoftConfirmOpen(false)}
          onConfirm={() => void runSoftDelete()}
          testId="workbuddy-library-soft-delete-confirm"
        />
      )}

      {/* 彻底删除单次确认（决策 7：与软删除分开确认；备份前置不可跳过）。 */}
      {purgeConfirmOpen && (
        <ConfirmDialog
          title="彻底删除会话"
          lines={[
            `将把所选 ${selectableIds.length} 条会话连同正文文件一并删除，不可恢复。`,
            "执行前会自动创建滚动备份（含 workbuddy.db 与正文目录），备份失败则整单拒绝。",
            "执行时如客户端在运行会先关闭再重启。",
          ]}
          confirmLabel="彻底删除"
          busyLabel="删除中…"
          busy={writeBusy}
          danger
          onCancel={() => setPurgeConfirmOpen(false)}
          onConfirm={() => void runPurge()}
          testId="workbuddy-library-purge-confirm"
        />
      )}

      {/* cwd 归并单次确认。 */}
      {mergeCandidate && (
        <ConfirmDialog
          title="归并项目目录"
          lines={[
            `将把以下 ${mergeCandidate.length - 1} 个变体目录的会话归并到 ${mergeCandidate[0]}（同一 cwd 的历史转义形态收敛）：${mergeCandidate.slice(1).join("、")}。`,
            "归并只改写本机索引归属与正文目录位置，不删除任何会话。",
            "执行时如客户端在运行会先关闭再重启。",
          ]}
          confirmLabel="确认归并"
          busyLabel="归并中…"
          busy={mergeBusy}
          onCancel={() => setMergeCandidate(null)}
          onConfirm={() => void runMerge(mergeCandidate)}
          testId="workbuddy-library-merge-confirm"
        />
      )}
    </section>
  );
}
