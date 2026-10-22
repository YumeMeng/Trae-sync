import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { RefreshCw, UserPlus } from "lucide-react";
import type { AppPage } from "../../components/NavigationRail";
import { loadCheckinSnapshot, type CheckinSnapshot } from "../../platform/checkin";
import { EmptyState } from "../../ui/EmptyState";
import { CheckinRows } from "./CheckinRows";
import { AutoCheckinResults, type AutoResultRow } from "./components/AutoCheckinResults";
import { CheckinBatchBar } from "./components/CheckinBatchBar";
import { CheckinPageHeader } from "./components/CheckinPageHeader";
import {
  useCheckinLiveEvents,
  type AutoCheckinFinishedPayload,
  type CheckinProgressPayload,
} from "./components/useCheckinLiveEvents";
import { useCheckinRun } from "./components/useCheckinRun";
import {
  autoStatusOf,
  checkinTextsFor,
  checkinViewFor,
  type CheckinAutoStatusView,
  type CheckinView,
} from "./config";

/** 签到页属性：槽位接缝按统一形状调用（productId 由接缝注入）。 */
export interface CheckinPageProps {
  /** 当前产品协议值：只用于查声明式配置，页面内不做任何产品判断。 */
  productId: string;
  /** 页面可见时才发起读取，避免后台 IPC。 */
  active: boolean;
  onNavigate: (page: AppPage) => void;
}

/** 自动批次逐账号结果（按 profileId 去重覆盖，与手动批次的行内结果分离）。 */
interface AutoResultState {
  readonly screenName: string;
  readonly outcome: string;
  readonly detailCode: string | null;
}

/**
 * 签到页（P4b-2 合并实现，P4b-3 接线）：TRAE 与 WorkBuddy 共用这一份实现。
 *
 * 结构（UI-DIRECTION §5）：标题 + 产品徽章 + 自动签到状态行 + 账号签到列表。
 * 产品差异**全部**来自 features/checkin 的声明式配置（config.ts 登记表）：
 * 本文件不做任何产品整页分支判断，也不直接调用 Tauri 命令——
 * 数据一律经 platform/checkin 读取与执行。
 *
 * P4b-3 补齐的三处能力：
 * - 自动批次实时结果清单 + 今日台账回填（auto-checkin-progress/finished）；
 * - 账号间错峰倒计时（checkin-phase 的 inter_wait，行内「N 秒后开始」）；
 * - 产品徽章与读取中门控（页头由槽位注入 productId）。
 */
export function CheckinPage({ productId, active, onNavigate }: CheckinPageProps) {
  const [snapshot, setSnapshot] = useState<CheckinSnapshot | null>(null);
  const [selection, setSelection] = useState<readonly string[]>([]);
  const [refreshing, setRefreshing] = useState(false);
  // 自动签到状态（undefined = 用快照值；事件回填后由本状态接管台账）。
  const [auto, setAuto] = useState<CheckinAutoStatusView | null | undefined>(undefined);
  const [autoResults, setAutoResults] = useState<Record<string, AutoResultState>>({});

  const texts = checkinTextsFor(productId);

  const load = useCallback(async () => {
    setSnapshot(await loadCheckinSnapshot(productId));
  }, [productId]);

  // 产品切换即重拉（同一份实现不重挂载，故以 productId 为依赖）。
  useEffect(() => {
    if (!active) return;
    void load();
  }, [active, load]);

  // 自动签到状态基线：首次快照到位时同步；事件一旦回填（autoTouched），
  // 后续重读快照不再覆盖——否则完成后重拉会把"已完成"翻回"执行中"。
  const autoTouched = useRef(false);
  useEffect(() => {
    autoTouched.current = false;
    setAuto(undefined);
  }, [productId]);
  useEffect(() => {
    if (snapshot === null || autoTouched.current) return;
    setAuto(autoStatusOf(snapshot));
  }, [snapshot]);

  const run = useCheckinRun({ productId, texts, reload: load });

  // 自动批次进度：逐账号结果上屏 + 台账实时回填（completed 来自事件，running 置真）。
  const onAutoProgress = useCallback((payload: CheckinProgressPayload) => {
    autoTouched.current = true;
    setAutoResults((current) => ({
      ...current,
      [payload.profile_id]: {
        screenName: payload.screen_name ?? "",
        outcome: payload.outcome,
        detailCode: payload.detail_code,
      },
    }));
    setAuto((current) => current?.ledger
      ? { ...current, ledger: { ...current.ledger, completed: payload.completed, running: true } }
      : current);
  }, []);

  // 自动批次完成：台账翻完成态 + 重读快照（签到会写回今日状态与积分缓存）。
  const onAutoFinished = useCallback((payload: AutoCheckinFinishedPayload) => {
    autoTouched.current = true;
    setAuto((current) => current
      ? {
          ...current,
          ledger: {
            running: false,
            total: payload.total,
            completed: payload.completed,
            failed: payload.failed,
            skipped: payload.skipped,
          },
        }
      : current);
    void load();
  }, [load]);

  useCheckinLiveEvents(active, {
    onProgress: run.onProgress,
    onPhase: run.onPhase,
    onAutoProgress,
    onAutoFinished,
  });

  const view: CheckinView = useMemo(
    () => checkinViewFor(productId, snapshot, {
      selection,
      busy: run.busy,
      autoOverride: auto,
    }),
    [productId, snapshot, selection, run.busy, auto],
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
    if (refreshing || run.busy) return;
    setRefreshing(true);
    void load().finally(() => setRefreshing(false));
  }, [load, refreshing, run.busy]);

  const handleAction = useCallback((key: string) => {
    switch (key) {
      case "run-all":
        void run.run(view.rows.map((row) => row.profileId));
        return;
      case "run-pending":
        void run.run(view.pendingIds);
        return;
      case "run-selected":
        void run.run(selection);
        return;
      case "cancel":
        void run.cancel();
        return;
      default:
        return;
    }
  }, [run, selection, view.pendingIds, view.rows]);

  const toggle = useCallback((profileId: string) => {
    setSelection((current) => current.includes(profileId)
      ? current.filter((id) => id !== profileId)
      : [...current, profileId]);
  }, []);

  // 自动批次结果清单：名字优先用事件里的账号名，缺失回落行内展示名。
  const autoRows: readonly AutoResultRow[] = Object.entries(autoResults).map(([profileId, item]) => ({
    profileId,
    displayName: item.screenName
      || view.rows.find((row) => row.profileId === profileId)?.displayName
      || "未知账号",
    outcome: item.outcome,
    detailCode: item.detailCode,
  }));

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

  // 门控：首次读取完成前只保留页头（键盘焦点与切页一致），不发主体。
  const loading = active && snapshot === null;

  return (
    <section className="checkin-page" role="region" aria-label={view.title}>
      <CheckinPageHeader productId={productId} action={loading ? undefined : headerActions} />

      {loading && (
        <EmptyState title="正在读取签到信息…" className="checkin-page__empty" />
      )}

      {/* 状态行：自动签到台账 / 演示模式 / 存储未就绪，一行说清。 */}
      {!loading && view.statuses.map((status) => (
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
      {!loading && view.error !== null && (
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

      {!loading && view.error === null && view.rows.length > 0 && (
        <section className="checkin-page__selection" aria-labelledby="checkin-selection-heading">
          <div className="account-center__section-heading">
            <h2 id="checkin-selection-heading">签到账号</h2>
            <span className="section-caption">已选 {selection.length}/{view.rows.length}</span>
          </div>

          <CheckinBatchBar
            actions={view.actions}
            progress={run.progress}
            busy={run.busy}
            onAction={handleAction}
          />

          <CheckinRows
            view={view}
            selection={selection}
            busy={run.busy}
            cursorId={run.cursorId}
            waits={run.waits}
            results={run.results}
            onToggle={toggle}
            onInlineRun={(profileId) => void run.run([profileId])}
          />

          <AutoCheckinResults rows={autoRows} productId={productId} />
        </section>
      )}

      {/* 空态：一句结论 + 一个入口。 */}
      {!loading && view.error === null && view.rows.length === 0 && view.empty !== null && (
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
