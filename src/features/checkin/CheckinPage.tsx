import { useCallback, useEffect, useMemo, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { RefreshCw, UserPlus } from "lucide-react";
import type { AppPage } from "../../components/NavigationRail";
import {
  cancelCheckin,
  loadCheckinSnapshot,
  runCheckin,
  type CheckinSnapshot,
} from "../../platform/checkin";
import { notify } from "../../platform/notify";
import { safeUiErrorMessage } from "../../utils/safeUiError";
import { EmptyState } from "../../ui/EmptyState";
import { SectionHeader } from "../../ui/SectionHeader";
import { CheckinRows } from "./CheckinRows";
import { checkinViewFor, type CheckinView } from "./config";

/** 行结果（本批次逐行 outcome；执行期间由进度事件实时写入）。 */
type RowResult = { readonly outcome: string; readonly detail_code: string | null };

/** 后端逐账号进度事件（checkin-progress）载荷；outcome 与 CheckinResultDto 同口径。 */
type CheckinProgressEvent = RowResult & {
  readonly profile_id: string;
  readonly completed: number;
  readonly total: number;
};

/** 签到页属性：槽位接缝按统一形状调用（productId 由接缝注入）。 */
export interface CheckinPageProps {
  /** 当前产品协议值：只用于查声明式配置，页面内不做任何产品判断。 */
  productId: string;
  /** 页面可见时才发起读取，避免后台 IPC。 */
  active: boolean;
  onNavigate: (page: AppPage) => void;
}

/**
 * 签到页（P4b-2 合并实现）：TRAE 与 WorkBuddy 共用这一份实现。
 *
 * 结构（UI-DIRECTION §5）：标题「签到」+ 自动签到状态行 + 账号签到列表。
 * 产品差异**全部**来自 features/checkin 的声明式配置（config.ts 登记表）：
 * 本文件不做任何产品整页分支判断，也不直接调用 Tauri 命令——
 * 数据一律经 platform/checkin 读取与执行。
 *
 * 长说明改状态：异常只留一句结论 + 重试；读取失败才展开原因。
 */
export function CheckinPage({ productId, active, onNavigate }: CheckinPageProps) {
  const [snapshot, setSnapshot] = useState<CheckinSnapshot | null>(null);
  const [selection, setSelection] = useState<readonly string[]>([]);
  const [busy, setBusy] = useState(false);
  const [refreshing, setRefreshing] = useState(false);
  // 本批次执行顺序（进度游标依据）与逐行实时结果。
  const [runIds, setRunIds] = useState<readonly string[]>([]);
  const [results, setResults] = useState<Record<string, RowResult>>({});
  const [liveCompleted, setLiveCompleted] = useState(0);

  const load = useCallback(async () => {
    setSnapshot(await loadCheckinSnapshot(productId));
  }, [productId]);

  // 产品切换即重拉（同一份实现不重挂载，故以 productId 为依赖）。
  useEffect(() => {
    if (!active) return;
    void load();
  }, [active, load]);

  // 批量执行期间监听逐账号进度：串行批次实时上屏，不黑盒等待。
  useEffect(() => {
    if (!active) return;
    let unlisten: (() => void) | null = null;
    let cancelled = false;
    void listen<CheckinProgressEvent>("checkin-progress", (event) => {
      const payload = event.payload;
      setLiveCompleted(payload.completed);
      setResults((current) => ({
        ...current,
        [payload.profile_id]: { outcome: payload.outcome, detail_code: payload.detail_code },
      }));
    }).then((fn) => {
      if (cancelled) fn();
      else unlisten = fn;
    }).catch(() => undefined);
    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, [active]);

  const view: CheckinView = useMemo(
    () => checkinViewFor(productId, snapshot, { selection, busy }),
    [productId, snapshot, selection, busy],
  );

  // 勾选默认值随池/参与集变化：剔除失效项并保留既有选择（空则回到默认集）。
  const defaultKey = view.defaultSelection.join("|");
  useEffect(() => {
    const defaults = defaultKey === "" ? [] : defaultKey.split("|");
    setSelection((current) => {
      const filtered = current.filter((id) => defaults.includes(id));
      return filtered.length > 0 ? filtered : defaults;
    });
  }, [defaultKey]);

  const handleRefresh = useCallback(() => {
    if (refreshing || busy) return;
    setRefreshing(true);
    void load().finally(() => setRefreshing(false));
  }, [load, refreshing, busy]);

  // 通用执行入口：全签 / 补签 / 所选 / 行内单签共用同一批次通道。
  // ids 顺序即执行顺序（后端串行），同时作为进度游标的依据。
  const handleRunIds = useCallback(async (ids: readonly string[]) => {
    if (busy || ids.length === 0) return;
    setBusy(true);
    setResults({});
    setLiveCompleted(0);
    setRunIds(ids);
    try {
      const outcome = await runCheckin(productId, ids);
      setResults(Object.fromEntries(
        outcome.results.map((item) => [
          item.profile_id,
          { outcome: item.outcome, detail_code: item.detail_code },
        ]),
      ));
      const prefix = view.texts.notificationPrefix;
      if (outcome.failed === 0) notify.success(`${prefix}：${view.texts.runSuccess}`);
      else notify.warning(`${prefix}：${view.texts.runPartial}`);
      // 签到会写回今日状态与积分缓存：重读快照让行内状态同步。
      await load();
    } catch (reason: unknown) {
      notify.error(
        `${view.texts.notificationPrefix}：${safeUiErrorMessage(reason, view.texts.runFailure)}`,
      );
    } finally {
      setBusy(false);
      setRunIds([]);
    }
  }, [busy, load, productId, view.texts]);

  const handleAction = useCallback((key: string) => {
    switch (key) {
      case "run-all":
        void handleRunIds(view.rows.map((row) => row.profileId));
        return;
      case "run-pending":
        void handleRunIds(view.pendingIds);
        return;
      case "run-selected":
        void handleRunIds(selection);
        return;
      case "cancel":
        void cancelCheckin(productId);
        return;
      default:
        return;
    }
  }, [handleRunIds, productId, selection, view.pendingIds, view.rows]);

  const toggle = useCallback((profileId: string) => {
    setSelection((current) => current.includes(profileId)
      ? current.filter((id) => id !== profileId)
      : [...current, profileId]);
  }, []);

  const progressTotal = runIds.length;
  // 进度游标：liveCompleted 是权威游标（后端串行递增），无事件时回落已完成行数。
  const progressCompleted = Math.min(
    Math.max(liveCompleted, Object.keys(results).length),
    progressTotal,
  );
  const cursorId = busy && runIds.length > 0
    ? runIds[Math.min(progressCompleted, runIds.length - 1)]
    : null;

  const headerActions = view.refresh !== null && view.error === null ? (
    <div className="page-header__actions">
      <button
        className="btn"
        type="button"
        onClick={handleRefresh}
        disabled={refreshing || !view.refresh.enabled}
        data-testid={view.refresh.testId}
        title={view.refresh.hint}
      >
        <RefreshCw size={15} className={refreshing ? "icon-spin" : undefined} aria-hidden="true" />
        {view.refresh.label}
      </button>
    </div>
  ) : undefined;

  return (
    <section className="checkin-page" role="region" aria-label={view.title}>
      <SectionHeader
        title={view.title}
        level={1}
        focusAnchor="checkin"
        className="page-header"
        action={headerActions}
      />

      {/* 状态行：自动签到台账 / 演示模式 / 存储未就绪，一行说清。 */}
      {view.statuses.map((status) => (
        <p
          key={status.testId}
          className={status.tone === "warn" ? "workbench__error" : "checkin-page__meta"}
          role={status.tone === "warn" ? "alert" : "status"}
          data-testid={status.testId}
        >
          {status.text}
        </p>
      ))}

      {/* 读取失败才展开：一句原因 + 重试动作。 */}
      {view.error !== null && (
        <EmptyState
          title={view.error.description}
          testId={view.error.testId}
          action={(
            <button
              className="btn btn--primary"
              type="button"
              onClick={() => void load()}
              disabled={refreshing}
            >
              {view.error.retryLabel}
            </button>
          )}
        />
      )}

      {view.error === null && view.rows.length > 0 && (
        <section className="checkin-page__selection" aria-labelledby="checkin-selection-heading">
          <div className="account-center__section-heading">
            <h2 id="checkin-selection-heading">签到账号</h2>
            <span className="section-caption">已选 {selection.length}/{view.rows.length}</span>
          </div>

          <div className="checkin-page__action-row" data-testid="checkin-actions">
            {view.actions.map((action) => (
              <button
                key={action.key}
                className={`btn${action.tone === "primary" ? " btn--primary" : action.tone === "quiet" ? " btn--quiet" : ""}`}
                type="button"
                onClick={() => handleAction(action.key)}
                disabled={!action.enabled}
                data-testid={action.testId}
              >
                {action.label}
              </button>
            ))}
          </div>

          {busy && progressTotal > 0 && (
            <div className="checkin-progress" role="status" data-testid="checkin-progress">
              <div className="checkin-progress__track">
                <div
                  className="checkin-progress__fill"
                  style={{ width: `${Math.round((progressCompleted / progressTotal) * 100)}%` }}
                />
              </div>
              <span className="checkin-progress__label">
                {progressCompleted}/{progressTotal}
              </span>
            </div>
          )}

          <CheckinRows
            view={view}
            selection={selection}
            busy={busy}
            cursorId={cursorId}
            results={results}
            onToggle={toggle}
            onInlineRun={(profileId) => void handleRunIds([profileId])}
          />
        </section>
      )}

      {/* 空态：一句结论 + 一个入口。 */}
      {view.error === null && view.rows.length === 0 && view.empty !== null && (
        <EmptyState
          title={view.empty.description}
          className="checkin-page__empty-actions"
          testId={view.empty.testId}
          action={(
            <button
              className="btn btn--primary"
              type="button"
              onClick={() => onNavigate("accounts")}
            >
              <UserPlus size={15} aria-hidden="true" />
              {view.empty.actionLabel}
            </button>
          )}
        />
      )}
    </section>
  );
}
