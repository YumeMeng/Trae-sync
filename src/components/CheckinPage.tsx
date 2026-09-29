import { useCallback, useEffect, useMemo, useState } from "react";
import type { ReactNode } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { CalendarCheck, Loader2, RefreshCw, Timer, UserPlus } from "lucide-react";
import type {
  AutoCheckinStatusDto,
  CheckinBatchSummaryDto,
  CheckinCapabilityDto,
  CheckinOverviewEntryDto,
  CheckinResultDto,
  ManagedAccountsViewDto,
} from "../types/account_switch";
import type {
  TraeCnAccountDto,
  TraeCnCheckinResultDto,
  TraeCnCheckinStatusDto,
} from "../types/product";
import type { AppPage } from "./NavigationRail";
import { CapabilitySection } from "./CapabilitySection";
import { EmptyState } from "./EmptyState";
import { sectionStateOf, type ProductId } from "../platform/productRegistry";
import { safeUiErrorMessage } from "../utils/safeUiError";
import { effectiveDisplayName } from "../utils/accountDisplay";
// 统一通知适配层（P11-1 切片②）：操作结果类提示一律经 notify 发出。
import { notify } from "../platform/notify";
import { CheckinSlotBadge, deriveCheckinSlotState } from "./StatusBadges";

/** 后端逐账号进度事件（checkin-progress）载荷；outcome 与 CheckinResultDto 同口径。 */
interface CheckinProgressEvent {
  profile_id: string;
  screen_name: string;
  outcome: CheckinResultDto["outcome"];
  detail_code: string | null;
  completed: number;
  total: number;
}

/** 后端阶段事件（checkin-phase）载荷：账号间错峰等待开始时推送一次。 */
interface CheckinPhaseEvent {
  profile_id: string;
  screen_name: string;
  phase: "inter_wait";
  remaining_secs: number;
}

/** 自动签到批次完成事件（auto-checkin-finished）载荷。 */
interface AutoCheckinFinishedEvent {
  total: number;
  completed: number;
  failed: number;
  skipped: number;
}

interface CheckinPageProps {
  /** 页面可见时才发起读取，避免后台 IPC。 */
  active: boolean;
  /** 空状态引导跳转账号页添加账号。 */
  onNavigate: (page: AppPage) => void;
  /** 当前产品（缺省 work_cn）：区块三态取自产品注册表。 */
  product?: ProductId;
}

/**
 * 签到页：专注「对账号的每日签到操作」。
 * 账号档案管理在「账号」页；这里只做签到选择、批量执行与逐账号结果。
 * 执行语义遵循 grill D8/ADR-0019：串行逐账号 status→claim→status，
 * 账号间随机间隔 3-8 秒（后端保障），本页只做期望管理提示。
 * 区块能力三态（P11-6）：非 ready 产品渲染诚实说明卡（探针已验证端点、
 * 实装待真实验收 → pending），不发起任何读取与监听。
 */
export function CheckinPage({ active, onNavigate, product = "work_cn" }: CheckinPageProps) {
  // 产品事实源：区块三态来自注册表，组件内不做 productId 判断。
  const sectionState = sectionStateOf(product, "checkin");
  const [capability, setCapability] = useState<CheckinCapabilityDto | null>(null);
  const [overview, setOverview] = useState<readonly CheckinOverviewEntryDto[]>([]);
  // fixture 模式（CI 演示）没有登录账号，沿用本地已保存账号作为签到池。
  const [savedPool, setSavedPool] = useState<readonly { profile_id: string; display_name: string }[]>([]);
  const [selection, setSelection] = useState<readonly string[]>([]);
  const [summary, setSummary] = useState<CheckinBatchSummaryDto | null>(null);
  // 批量执行期间的逐账号实时结果（来自 checkin-progress 事件）；批次结束由 summary 接管。
  const [liveResults, setLiveResults] = useState<readonly CheckinProgressEvent[]>([]);
  const [liveProgress, setLiveProgress] = useState<{ completed: number; total: number } | null>(null);
  // U-3 单列表演进式：本次批次的目标顺序（进度推演“当前执行到哪一行”）。
  const [runIds, setRunIds] = useState<readonly string[]>([]);
  // 账号间错峰等待倒计时（checkin-phase/inter_wait）：下一个账号 → 剩余秒数。
  // 批量执行期间 3~8 秒错峰间隔显示"N 秒后开始"，不再是无变化的"签到中…"。
  const [interWaits, setInterWaits] = useState<Record<string, number>>({});
  const [busy, setBusy] = useState(false);
  // 自动签到：设置与今日台账状态行 + 错峰执行实时结果（完成摘要由 App 级 notify.summary 承载）。
  const [autoStatus, setAutoStatus] = useState<AutoCheckinStatusDto | null>(null);
  const [autoResults, setAutoResults] = useState<readonly CheckinProgressEvent[]>([]);

  const realMode = capability?.real_http_enabled === true;

  useEffect(() => {
    if (!active || sectionState !== "ready") return;
    let cancelled = false;
    void invoke<CheckinCapabilityDto>("get_checkin_capability")
      .then((next) => { if (!cancelled) setCapability(next); })
      .catch(() => {
        if (!cancelled) {
          setCapability({
            enabled: false,
            transport: "disabled",
            real_http_enabled: false,
            message: "签到能力当前不可读取。",
          });
        }
      });
    return () => { cancelled = true; };
  }, [active, sectionState]);

  // 签到池数据：真实模式读取账号总览（含缓存积分/今日状态）；fixture 读取已保存账号。
  useEffect(() => {
    if (!active || !capability) return;
    let cancelled = false;
    if (capability.real_http_enabled) {
      void invoke<CheckinOverviewEntryDto[]>("get_checkin_overview")
        .then((entries) => { if (!cancelled) setOverview(entries); })
        .catch((reason: unknown) => {
          // 读取失败必须可见：静默置空会把真实故障伪装成“还没有账号”。
          if (!cancelled) {
            setOverview([]);
            notify.error(safeUiErrorMessage(reason, "账号总览读取失败。"));
          }
        });
    } else if (capability.transport === "fixture") {
      void invoke<ManagedAccountsViewDto>("get_managed_account_state")
        .then((view) => {
          if (!cancelled) {
            setSavedPool(view.saved_accounts.map((account) => ({
              profile_id: account.profile_id,
              display_name: account.display_name,
            })));
          }
        })
        .catch(() => undefined);
    }
    return () => { cancelled = true; };
  }, [active, capability]);

  const poolIds = useMemo(
    () => realMode
      ? overview.map((entry) => entry.profile_id)
      : savedPool.map((account) => account.profile_id),
    [realMode, overview, savedPool],
  );

  // 选择默认全选；池变化时剔除失效项并保留既有选择。
  useEffect(() => {
    setSelection((current) => {
      const filtered = current.filter((id) => poolIds.includes(id));
      return filtered.length > 0 ? filtered : poolIds;
    });
  }, [poolIds]);

  const displayName = useCallback((profileId: string): string => {
    if (!realMode) {
      return savedPool.find((account) => account.profile_id === profileId)?.display_name ?? profileId;
    }
    const entry = overview.find((item) => item.profile_id === profileId);
    return entry ? effectiveDisplayName(entry) : profileId;
  }, [realMode, overview, savedPool]);

  // 监听后端逐账号进度事件：串行批次（3-8 秒/账号）期间实时上屏，不再黑盒等待。
  useEffect(() => {
    if (!active || sectionState !== "ready") return;
    let unlisten: (() => void) | null = null;
    let cancelled = false;
    void listen<CheckinProgressEvent>("checkin-progress", (event) => {
      const payload = event.payload;
      setLiveProgress({ completed: payload.completed, total: payload.total });
      setLiveResults((current) => [
        ...current.filter((item) => item.profile_id !== payload.profile_id),
        payload,
      ]);
    }).then((fn) => {
      if (cancelled) fn();
      else unlisten = fn;
    }).catch(() => undefined);
    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, [active, sectionState]);

  // 监听阶段事件（checkin-phase）：账号间错峰等待，行内显示"N 秒后开始"。
  useEffect(() => {
    if (!active || sectionState !== "ready") return;
    let unlisten: (() => void) | null = null;
    let cancelled = false;
    void listen<CheckinPhaseEvent>("checkin-phase", (event) => {
      const payload = event.payload;
      if (payload.phase === "inter_wait") {
        setInterWaits((current) => ({ ...current, [payload.profile_id]: payload.remaining_secs }));
      }
    }).then((fn) => {
      if (cancelled) fn();
      else unlisten = fn;
    }).catch(() => undefined);
    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, [active, sectionState]);

  // 自动签到：读取今日台账 + 监听错峰执行进度与完成事件（与手动通道分离）。
  useEffect(() => {
    if (!active || sectionState !== "ready") return;
    let cancelled = false;
    const unlisteners: Array<() => void> = [];
    const addUnlisten = (promise: Promise<() => void>) => {
      void promise.then((fn) => {
        if (cancelled) fn();
        else unlisteners.push(fn);
      }).catch(() => undefined);
    };
    void invoke<AutoCheckinStatusDto>("get_auto_checkin_settings")
      .then((next) => { if (!cancelled) setAutoStatus(next); })
      .catch(() => undefined);
    addUnlisten(listen<CheckinProgressEvent>("auto-checkin-progress", (event) => {
      const payload = event.payload;
      setAutoResults((current) => [
        ...current.filter((item) => item.profile_id !== payload.profile_id),
        payload,
      ]);
      // 台账实时回填：进度事件携带今日批次 completed/total。
      setAutoStatus((current) => current && current.ledger
        ? { ...current, ledger: { ...current.ledger, completed: payload.completed, running: true } }
        : current);
    }));
    // 完成事件只做本页数据回填（台账翻完成态 + 刷新总览）；跨页完成摘要
    // 由 App 级监听经 notify.summary 发布（P11-1 切片②），不再渲染页内横幅。
    addUnlisten(listen<AutoCheckinFinishedEvent>("auto-checkin-finished", (event) => {
      const payload = event.payload;
      setAutoStatus((current) => current
        ? { ...current, ledger: { date: current.ledger?.date ?? "", running: false, total: payload.total, completed: payload.completed, failed: payload.failed, skipped: payload.skipped } }
        : current);
      // 自动批次会消耗签到资格并写回缓存：刷新总览同步“今日已签/积分”。
      if (realMode) {
        void invoke<CheckinOverviewEntryDto[]>("get_checkin_overview")
          .then((entries) => { if (!cancelled) setOverview(entries); })
          .catch(() => undefined);
      }
    }));
    return () => {
      cancelled = true;
      unlisteners.forEach((fn) => fn());
    };
  }, [active, sectionState, realMode]);

  // U-3 通用执行入口：一键全签 / 一键补签 / 签到所选 / 行内单签 共用同一批次通道。
  // ids 顺序即执行顺序（后端串行），同时存入 runIds 供进度条推演当前行。
  const handleRunIds = useCallback(async (ids: readonly string[]) => {
    if (!capability?.enabled || ids.length === 0 || busy) return;
    setBusy(true);
    setSummary(null);
    setLiveResults([]);
    setLiveProgress(null);
    setInterWaits({});
    setRunIds(ids);
    try {
      const next = await invoke<CheckinBatchSummaryDto>("run_checkin", { profileIds: [...ids] });
      setSummary(next);
      // 批次结果走全局通知：全成功按成功档，部分失败按警告档（异常原因仍在逐行结果里）。
      if (next.failed === 0) notify.success("签到流程已完成。");
      else notify.warning("部分签到未完成，请查看逐账号结果。");
      // 签到会更新积分缓存（后端写回）；刷新总览让“今日已签/积分”同步。
      if (realMode) {
        const entries = await invoke<CheckinOverviewEntryDto[]>("get_checkin_overview").catch(() => null);
        if (entries) setOverview(entries);
      }
    } catch (reason: unknown) {
      notify.error(safeUiErrorMessage(reason, "签到未完成，请稍后重试。"));
    } finally {
      setBusy(false);
      setRunIds([]);
      setInterWaits({});
    }
  }, [busy, capability?.enabled, realMode]);

  // fixture 模式沿用「全部签到（所选）」语义。
  const handleRun = useCallback(() => void handleRunIds(selection), [handleRunIds, selection]);

  const handleCancel = useCallback(async () => {
    if (!busy) return;
    await invoke("cancel_checkin").catch(() => undefined);
  }, [busy]);

  // G15 纯本地刷新：重读账号总览（本机缓存，零网络请求）。语义分工：
  // 刷新 = 重读缓存；签到/补签 = 真实网络操作。与账号页刷新按钮行为一致。
  const [localRefreshBusy, setLocalRefreshBusy] = useState(false);
  const handleLocalRefresh = useCallback(() => {
    if (!realMode || localRefreshBusy) return;
    setLocalRefreshBusy(true);
    void invoke<CheckinOverviewEntryDto[]>("get_checkin_overview")
      .then((entries) => setOverview(entries))
      .catch(() => undefined) // 读取失败保持旧数据（初始加载失败已有可见口径）。
      .finally(() => setLocalRefreshBusy(false));
  }, [realMode, localRefreshBusy]);

  const toggle = useCallback((profileId: string) => {
    setSelection((current) => current.includes(profileId)
      ? current.filter((id) => id !== profileId)
      : [...current, profileId]);
  }, []);

  // U-3 四动作直达的数据口径（真实模式）：
  // 全签 = 全部账号（已签账号幂等跳过，只多一次 status 查询）；
  // 补签 = 今日未签账号（少发请求，降低风控触发面）；
  // 所选 = 勾选子集（非空且非全量时显示按钮）。
  const pendingIds = useMemo(
    () => realMode ? overview.filter((entry) => entry.checked_in !== true).map((entry) => entry.profile_id) : [],
    [realMode, overview],
  );
  const showSelectedRun = realMode && selection.length > 0 && selection.length < poolIds.length;
  const progressTotal = liveProgress?.total ?? runIds.length;
  // 进度条与行阶段推演：liveProgress.completed 是权威游标（后端串行递增）。
  const progressCompleted = liveProgress?.completed ?? 0;
  const cursorId = busy && runIds.length > 0 ? runIds[Math.min(progressCompleted, runIds.length - 1)] : null;

  // 区块能力三态（P11-6，ADR-0032 决策 2/4）：非 ready 产品渲染诚实说明卡。
  // Trae CN 签到：只读探针已验证 status 端点存在，claim 实装未做真实验收 → pending。
  if (sectionState !== "ready") {
    return (
      <section className="checkin-page" role="region" aria-label="签到">
        <header className="page-header">
          <div className="page-header__copy">
            <h1 data-page-title="checkin" tabIndex={-1}>签到</h1>
          </div>
        </header>
        <CapabilitySection
          state={sectionState}
          product={product}
          title="签到能力适配中"
          note="协议端点已验证，实装待验收。"
        />
      </section>
    );
  }

  if (!active) {
    return (
      <section className="checkin-page" role="region" aria-label="签到">
        <header className="page-header">
          <div className="page-header__copy">
            <h1 data-page-title="checkin" tabIndex={-1}>签到</h1>
          </div>
        </header>
        <EmptyState role="status" className="checkin-page__empty" description="正在读取签到信息…" />
      </section>
    );
  }

  // Trae CN 签到（P13-1）：区块 ready 后按 CN 产品账号池渲染独立签到流。
  // 此处分支依据是产品账号池事实（CN 独立注册表/凭据根，单账号手动签到，
  // 无批量无调度），能力三态仍由 productRegistry 单一事实源裁决。
  if (product === "trae_cn") {
    return <TraeCnCheckinSection onNavigate={onNavigate} />;
  }

  return (
    <section className="checkin-page" role="region" aria-label="签到">
      <header className="page-header">
        <div className="page-header__copy">
          <h1 data-page-title="checkin" tabIndex={-1}>签到</h1>
        </div>
        {/* G15 纯本地刷新：重读缓存零联网，与签到/补签的网络操作分工。 */}
        {realMode && poolIds.length > 0 && (
          <div className="page-header__actions">
            <button
              className="btn"
              type="button"
              onClick={handleLocalRefresh}
              disabled={localRefreshBusy}
              data-testid="checkin-refresh-local"
              title="重读本机缓存的账号信息（不联网）"
            >
              <RefreshCw size={15} className={localRefreshBusy ? "icon-spin" : undefined} aria-hidden="true" />刷新
            </button>
          </div>
        )}
      </header>

      {autoStatus && (
        <p className="checkin-page__meta" role="status" data-testid="auto-checkin-status">
          自动签到：{autoStatus.enabled ? `每日 ${autoStatus.daily_time_hhmm} 触发` : "已关闭"}
          {autoStatus.enabled ? ` · ${autoLedgerLabel(autoStatus)}` : ""}
        </p>
      )}
      {/* 常驻提示只保留两类：fixture 演示模式标识（一行）与异常态（能力读取失败或存储根
          不可用，统一显示存储未就绪并附带原因）。正常态不展示 capability.message——
          其中是开发注释型自述，用户不可据此行动。capability 数据照常读取，仅不再常驻展示。 */}
      {capability?.transport === "fixture" && (
        <p className="checkin-page__meta" role="status" data-testid="checkin-demo-banner">演示模式</p>
      )}
      {capability && capability.transport !== "fixture" && !capability.enabled && (
        <p className="workbench__error" role="alert" data-testid="checkin-unavailable">
          签到功能不可用：存储未就绪
        </p>
      )}

      {poolIds.length > 0 ? (
        <section className="checkin-page__selection" aria-labelledby="checkin-selection-heading">
          <div className="account-center__section-heading">
            <h3 id="checkin-selection-heading"><CalendarCheck size={16} aria-hidden="true" />签到账号</h3>
            <span className="section-caption">已选 {selection.length}/{poolIds.length}</span>
          </div>

          {/* U-3 四动作直达（真实模式）：全签/补签常驻，勾选子集出现「签到所选」。 */}
          {realMode ? (
            <div className="checkin-page__action-row" data-testid="checkin-actions">
              <button
                className="btn btn--primary"
                type="button"
                onClick={() => void handleRunIds(poolIds)}
                disabled={!capability?.enabled || busy}
                data-testid="checkin-run-all"
                title="对全部账号执行签到（已签账号自动跳过，不重复领取）"
              >
                <CalendarCheck size={15} aria-hidden="true" />{busy ? "签到执行中…" : `一键全签（${poolIds.length}）`}
              </button>
              <button
                className="btn"
                type="button"
                onClick={() => void handleRunIds(pendingIds)}
                disabled={!capability?.enabled || busy || pendingIds.length === 0}
                data-testid="checkin-run-pending"
                title="只对今日未签账号执行签到（少发请求）"
              >
                <CalendarCheck size={15} aria-hidden="true" />一键补签（{pendingIds.length}）
              </button>
              {showSelectedRun && (
                <button
                  className="btn"
                  type="button"
                  onClick={() => void handleRunIds(selection)}
                  disabled={!capability?.enabled || busy}
                  data-testid="checkin-run-selected"
                >
                  <CalendarCheck size={15} aria-hidden="true" />签到所选（{selection.length}）
                </button>
              )}
              <button className="btn btn--quiet" type="button" onClick={() => void handleCancel()} disabled={!busy}>取消未开始任务</button>
            </div>
          ) : (
            <div className="checkin-page__action-row">
              <button className="btn btn--primary" type="button" onClick={() => void handleRun()} disabled={!capability?.enabled || selection.length === 0 || busy}>
                <CalendarCheck size={15} aria-hidden="true" />{busy ? "签到执行中…" : `全部签到（${selection.length}）`}
              </button>
              <button className="btn btn--quiet" type="button" onClick={() => void handleCancel()} disabled={!busy}>取消未开始任务</button>
            </div>
          )}

          {/* U-3 总进度条：串行批次期间 X/Y + 已完成比例。 */}
          {busy && progressTotal > 0 && (
            <div className="checkin-progress" role="status" data-testid="checkin-progress">
              <div className="checkin-progress__track">
                <div
                  className="checkin-progress__fill"
                  style={{ width: `${Math.round((progressCompleted / progressTotal) * 100)}%` }}
                />
              </div>
              <span className="checkin-progress__label">
                {progressCompleted}/{progressTotal} · 每账号走 状态 → 领取 → 复核，账号间自动间隔 3-8 秒
              </span>
            </div>
          )}

          {/* U-3 单列表演进式（真实模式）：同一行随流程变状态（排队/执行中/结果）。 */}
          {realMode ? (
            <ul className="checkin-flow" aria-label="签到账号与结果">
              {overview.map((entry) => {
                const live = liveResults.find((item) => item.profile_id === entry.profile_id);
                const finalResult = summary?.results.find((result) => result.profile_id === entry.profile_id) ?? null;
                const isCursor = busy && entry.profile_id === cursorId;
                const doneInRun = busy && live !== undefined;
                // 当前行尚未开始且处于账号间错峰等待：显示"N 秒后开始"。
                const waitSecs = interWaits[entry.profile_id] ?? null;
                const inWait = isCursor && !doneInRun && waitSecs !== null;
                return (
                  <CheckinFlowRow
                    key={entry.profile_id}
                    entry={entry}
                    selected={selection.includes(entry.profile_id)}
                    onToggle={() => toggle(entry.profile_id)}
                    disabled={busy}
                    phase={busy ? (doneInRun ? "done" : isCursor ? (inWait ? "inter_wait" : "running") : "queued") : "idle"}
                    result={busy ? (live ?? null) : finalResult}
                    waitSecs={inWait ? waitSecs : null}
                    onInlineCheckin={() => void handleRunIds([entry.profile_id])}
                  />
                );
              })}
            </ul>
          ) : (
            <ul className="account-center__profile-list">
              {savedPool.map((account) => (
                <CheckinRow
                  key={account.profile_id}
                  profileId={account.profile_id}
                  name={account.display_name}
                  meta=""
                  badge={null}
                  selected={selection.includes(account.profile_id)}
                  onToggle={() => toggle(account.profile_id)}
                  disabled={busy}
                  result={summary?.results.find((result) => result.profile_id === account.profile_id) ?? null}
                />
              ))}
            </ul>
          )}

          {autoResults.length > 0 && (
            <ul className="account-center__checkin-results" aria-label="自动签到结果" data-testid="auto-checkin-results">
              {autoResults.map((item) => (
                <li key={item.profile_id}>
                  <strong>{item.screen_name || displayName(item.profile_id)}</strong>
                  <span>{checkinOutcomeLabel(item.outcome, item.detail_code)}</span>
                </li>
              ))}
            </ul>
          )}
        </section>
      ) : (
        <EmptyState
          className="checkin-page__empty-actions"
          description="还没有可签到的账号。先在「账号」页通过浏览器登录添加账号。"
          action={
            <button className="btn btn--primary" type="button" onClick={() => onNavigate("accounts")}>
              <UserPlus size={15} aria-hidden="true" />去账号页添加
            </button>
          }
        />
      )}
    </section>
  );
}

/**
 * Trae CN 签到区块（P13-1）：CN 账号池逐账号展示签到状态并提供手动签到。
 * 与 Work 流的差异是产品事实：CN 为独立账号池（list_trae_cn_accounts）、
 * 单账号手动 claim（一次一授权，仅用户点击触发）、无批量/调度/自动签到；
 * 状态查询直连直出（本轮不落缓存）。结果文案与 Work 共用同一套映射，
 * 业务码按红线透传（business_{code} → 服务端拒绝（code））。
 */
function TraeCnCheckinSection({ onNavigate }: { onNavigate: (page: AppPage) => void }) {
  const [accounts, setAccounts] = useState<readonly TraeCnAccountDto[]>([]);
  const [statusByProfile, setStatusByProfile] = useState<Record<string, TraeCnCheckinStatusDto | null>>({});
  const [errorByProfile, setErrorByProfile] = useState<Record<string, string>>({});
  const [resultByProfile, setResultByProfile] = useState<Record<string, TraeCnCheckinResultDto>>({});
  const [loadFailed, setLoadFailed] = useState(false);
  const [busyProfile, setBusyProfile] = useState<string | null>(null);
  const [refreshing, setRefreshing] = useState(false);

  // 单账号状态查询：只读直连；行内失败可见（不静默伪装成"未查询"）。
  const refreshStatus = useCallback(async (profileId: string) => {
    try {
      const status = await invoke<TraeCnCheckinStatusDto>("get_trae_cn_checkin_status", { profileId });
      setStatusByProfile((current) => ({ ...current, [profileId]: status }));
      setErrorByProfile((current) => {
        if (!(profileId in current)) return current;
        const next = { ...current };
        delete next[profileId];
        return next;
      });
    } catch (reason: unknown) {
      setStatusByProfile((current) => ({ ...current, [profileId]: null }));
      setErrorByProfile((current) => ({
        ...current,
        [profileId]: traeCnCheckinErrorMessage(reason, "签到状态查询失败，请稍后重试。"),
      }));
    }
  }, []);

  const loadAccounts = useCallback(() => {
    setRefreshing(true);
    void invoke<TraeCnAccountDto[]>("list_trae_cn_accounts")
      .then(async (rows) => {
        setAccounts(rows);
        setLoadFailed(false);
        await Promise.all(rows.map((row) => refreshStatus(row.profile_id)));
      })
      .catch(() => setLoadFailed(true))
      .finally(() => setRefreshing(false));
  }, [refreshStatus]);

  useEffect(() => {
    loadAccounts();
  }, [loadAccounts]);

  // 手动签到：单账号 claim；成功/失败都经 notify 透出并刷新该行状态。
  const handleClaim = useCallback(async (profileId: string) => {
    if (busyProfile) return;
    setBusyProfile(profileId);
    try {
      const result = await invoke<TraeCnCheckinResultDto>("run_trae_cn_checkin", { profileId });
      setResultByProfile((current) => ({ ...current, [profileId]: result }));
      if (result.outcome === "claimed" || result.outcome === "already_checked_in") {
        notify.success(checkinOutcomeLabel(result.outcome, result.detail_code));
      } else {
        notify.error(checkinOutcomeLabel(result.outcome, result.detail_code));
      }
      await refreshStatus(profileId);
    } catch (reason: unknown) {
      notify.error(traeCnCheckinErrorMessage(reason, "签到未完成，请稍后重试。"));
    } finally {
      setBusyProfile(null);
    }
  }, [busyProfile, refreshStatus]);

  return (
    <section className="checkin-page" role="region" aria-label="签到">
      <header className="page-header">
        <div className="page-header__copy">
          <h1 data-page-title="checkin" tabIndex={-1}>签到</h1>
        </div>
        {accounts.length > 0 && (
          <div className="page-header__actions">
            <button
              className="btn"
              type="button"
              onClick={loadAccounts}
              disabled={refreshing}
              data-testid="checkin-cn-refresh"
              title="重新查询各账号的签到状态（联网）"
            >
              <RefreshCw size={15} className={refreshing ? "icon-spin" : undefined} aria-hidden="true" />刷新状态
            </button>
          </div>
        )}
      </header>

      {loadFailed ? (
        <EmptyState
          className="checkin-page__empty"
          description="Trae CN 账号列表暂时不可读取，请稍后重试。"
        />
      ) : accounts.length === 0 ? (
        <EmptyState
          className="checkin-page__empty-actions"
          description="还没有 Trae CN 账号。先在「账号」页完成登录添加。"
          action={
            <button className="btn btn--primary" type="button" onClick={() => onNavigate("accounts")}>
              <UserPlus size={15} aria-hidden="true" />去账号页添加
            </button>
          }
        />
      ) : (
        <section className="checkin-page__selection" aria-labelledby="checkin-selection-heading">
          <div className="account-center__section-heading">
            <h3 id="checkin-selection-heading"><CalendarCheck size={16} aria-hidden="true" />签到账号</h3>
            <span className="section-caption">{accounts.length} 个账号</span>
          </div>
          <ul className="checkin-flow" aria-label="Trae CN 签到账号与结果">
            {accounts.map((account) => (
              <TraeCnCheckinRow
                key={account.profile_id}
                account={account}
                status={statusByProfile[account.profile_id] ?? null}
                error={errorByProfile[account.profile_id] ?? null}
                result={resultByProfile[account.profile_id] ?? null}
                busy={busyProfile !== null}
                spinning={busyProfile === account.profile_id}
                onClaim={() => void handleClaim(account.profile_id)}
              />
            ))}
          </ul>
        </section>
      )}
    </section>
  );
}

/** Trae CN 签到账号行：名字 + 今日状态徽章 + 额外积分 + 行内签到按钮与结果。 */
function TraeCnCheckinRow({
  account,
  status,
  error,
  result,
  busy,
  spinning,
  onClaim,
}: {
  account: TraeCnAccountDto;
  status: TraeCnCheckinStatusDto | null;
  error: string | null;
  result: TraeCnCheckinResultDto | null;
  busy: boolean;
  spinning: boolean;
  onClaim: () => void;
}) {
  const checkedIn = status?.checked_in === true;
  const claimable = status?.enable === true && status.checked_in === false;
  const outcome = result?.outcome ?? null;
  // 结果语气：claimed/already 安静（中性）；其余琥珀（需要关注）。
  const resultTone = outcome === "claimed" || outcome === "already_checked_in" ? "ok" : "warn";
  const metaParts: string[] = [];
  // CN 特有字段（语义待 P13-4 验收）：非空非零时如实展示。
  if (status?.extra_credits != null && status.extra_credits > 0) {
    metaParts.push(`额外积分 ${status.extra_credits}`);
  }
  if (account.last_verified_at) metaParts.push(`最近验证 ${formatShortDate(account.last_verified_at)}`);
  return (
    <li className="checkin-flow__row">
      <span className="checkin-flow__id">
        <span className="checkin-flow__name-row">
          <strong>{account.display_name}</strong>
          {status && <CheckinSlotBadge state={checkedIn ? "checked_in" : "unchecked"} />}
        </span>
        <span className="checkin-flow__meta">{error ?? metaParts.join(" · ")}</span>
      </span>
      <span className="checkin-flow__side">
        {spinning && (
          <span className="checkin-flow__phase checkin-flow__phase--active">
            <Loader2 size={13} className="checkin-flow__spinner" aria-hidden="true" />签到中…
          </span>
        )}
        {!spinning && result && (
          <span className={`checkin-flow__result checkin-flow__result--${resultTone}`}>
            {checkinOutcomeLabel(result.outcome, result.detail_code)}
          </span>
        )}
        <button
          className="btn checkin-flow__inline-btn"
          type="button"
          onClick={onClaim}
          disabled={busy || !claimable}
          title={claimable ? "领取今日签到积分" : checkedIn ? "今日已签到" : "当前不可领取"}
          data-testid={`checkin-cn-claim-${account.profile_id}`}
        >
          <CalendarCheck size={13} aria-hidden="true" />签到
        </button>
      </span>
    </li>
  );
}

/**
 * CN 签到错误 → 界面文案：业务码先提取数值原样展示（红线：不压平），
 * 其余稳定原因码走 safeUiError 词表（对齐 environment_delete_failed_residue
 * 的"先特判、再词表"先例）。
 */
function traeCnCheckinErrorMessage(error: unknown, fallback: string): string {
  const text = typeof error === "string"
    ? error
    : error instanceof Error
      ? error.message
      : "";
  const business = /trae_cn_checkin_business_(\d+)/.exec(text);
  if (business) return `服务端拒绝（${business[1]}）`;
  return safeUiErrorMessage(error, fallback);
}

/**
 * U-3 签到流单列行：勾选框 + 名字 + 签到槽位徽章 + meta + 行内单签按钮；
 * 执行期间随 phase 演进（queued 排队 / running 执行中高亮 / done 结果），
 * 批次结束后 result 直接落在行内——选择/执行/结果零重复。
 */
function CheckinFlowRow({
  entry,
  selected,
  onToggle,
  disabled,
  phase,
  result,
  waitSecs,
  onInlineCheckin,
}: {
  entry: CheckinOverviewEntryDto;
  selected: boolean;
  onToggle: () => void;
  disabled: boolean;
  phase: "idle" | "queued" | "running" | "inter_wait" | "done";
  result: CheckinResultDto | CheckinProgressEvent | null;
  waitSecs: number | null;
  onInlineCheckin: () => void;
}) {
  const unchecked = entry.checked_in !== true;
  // G10 统一状态机：签到槽徽章由 derive* 纯函数求枚举态（今日尝试失败显示
  // “签到失败/待重试”，与账号页同义同色）。
  const checkinState = deriveCheckinSlotState(entry);
  const outcome = result?.outcome ?? null;
  const detailCode = result?.detail_code ?? null;
  // 结果语气：claimed/already 安静（中性）；其余琥珀（需要关注）。
  const resultTone = outcome === "claimed" || outcome === "already_checked_in" ? "ok" : "warn";
  return (
    <li className={`checkin-flow__row${phase === "running" ? " checkin-flow__row--active" : ""}`}>
      <label className="checkin-flow__select">
        <input type="checkbox" checked={selected} onChange={onToggle} disabled={disabled} />
        <span className="checkin-flow__id">
          <span className="checkin-flow__name-row">
            <strong>{effectiveDisplayName(entry)}</strong>
            <CheckinSlotBadge state={checkinState} />
          </span>
          <span className="checkin-flow__meta">{entryMeta(entry)}</span>
        </span>
      </label>
      <span className="checkin-flow__side">
        {phase === "queued" && <span className="checkin-flow__phase">排队中</span>}
        {phase === "running" && (
          <span className="checkin-flow__phase checkin-flow__phase--active">
            <Loader2 size={13} className="checkin-flow__spinner" aria-hidden="true" />签到中…
          </span>
        )}
        {phase === "inter_wait" && waitSecs !== null && (
          <span className="checkin-flow__phase checkin-flow__phase--active" data-testid="checkin-inter-wait">
            <InterWaitCountdown seconds={waitSecs} />
          </span>
        )}
        {(phase === "done" || phase === "idle") && outcome && (
          <span className={`checkin-flow__result checkin-flow__result--${resultTone}`}>
            {checkinOutcomeLabel(outcome, detailCode)}
          </span>
        )}
        {/* 行内单账号签到：仅未签账号可点；执行期间禁用。 */}
        <button
          className="btn checkin-flow__inline-btn"
          type="button"
          onClick={onInlineCheckin}
          disabled={disabled || !unchecked}
          title={unchecked ? "仅签到该账号" : "今日已签到"}
          data-testid={`checkin-inline-${entry.profile_id}`}
        >
          <CalendarCheck size={13} aria-hidden="true" />签到
        </button>
      </span>
    </li>
  );
}

/** 签到选择行：复选框 + 账号名 + 今日状态 + 缓存积分 + 最近一次签到结果徽章。 */
function CheckinRow({ name, meta, badge, selected, onToggle, disabled, result }: {
  profileId: string;
  name: string;
  meta: string;
  badge: ReactNode;
  selected: boolean;
  onToggle: () => void;
  disabled: boolean;
  result: CheckinResultDto | null;
}) {
  const resultTone = result?.outcome === "claimed" ? "safe"
    : result?.outcome === "already_checked_in" ? "neutral" : "warning";
  return (
    <li className={`account-center__profile${selected ? " account-center__profile--selected" : ""}`}>
      <label className="account-center__profile-select">
        <input type="checkbox" checked={selected} onChange={onToggle} disabled={disabled} />
        <span className="account-center__profile-copy">
          <strong>{name}</strong>
          {meta && <span>{meta}</span>}
        </span>
      </label>
      <span className="account-center__profile-status">
        {badge}
        {result && (
          <span className={`status-badge status-badge--${resultTone}`}>
            {checkinOutcomeLabel(result.outcome, result.detail_code)}
          </span>
        )}
      </span>
    </li>
  );
}

/**
 * 本地秒级倒计时（后端只在阶段开始时推送一次总时长，前端自行递减）。
 */
function useCountdownSeconds(initial: number) {
  const [remaining, setRemaining] = useState(initial);
  useEffect(() => {
    setRemaining(initial);
  }, [initial]);
  useEffect(() => {
    if (remaining <= 0) return;
    const timer = window.setTimeout(() => setRemaining((value) => Math.max(0, value - 1)), 1000);
    return () => window.clearTimeout(timer);
  }, [remaining]);
  return remaining;
}

/**
 * 账号间错峰等待倒计时：批量执行时下一账号前的 3~8 秒随机间隔
 * 显示"N 秒后开始"；归零后显示"开始中…"（由 running 阶段接管）。
 */
function InterWaitCountdown({ seconds }: { seconds: number }) {
  const remaining = useCountdownSeconds(seconds);
  if (remaining <= 0) {
    return (
      <>
        <Loader2 size={13} className="checkin-flow__spinner" aria-hidden="true" />开始中…
      </>
    );
  }
  return (
    <>
      <Timer size={13} aria-hidden="true" />{remaining} 秒后开始
    </>
  );
}

/** 行内元信息：模型额度（含时间标注）+ 最近验证。
 * 注：签到活动 credits 为静态池值（实证恒 200），不展示。 */
function entryMeta(entry: CheckinOverviewEntryDto): string {
  const parts: string[] = [];
  if (entry.usage_remaining_credits !== null) {
    parts.push(`额度 ${formatCreditsValue(entry.usage_remaining_credits)}${entry.usage_cached_at ? `（${formatShortDate(entry.usage_cached_at)} 缓存）` : ""}`);
  }
  if (entry.last_verified_at) parts.push(`最近验证 ${formatShortDate(entry.last_verified_at)}`);
  return parts.join(" · ");
}

/** 今日自动批次台账文案；与设置页同一口径。 */
function autoLedgerLabel(status: AutoCheckinStatusDto): string {
  const ledger = status.ledger;
  if (!ledger) return "今日尚未发起";
  const parts = [`成功 ${ledger.completed}/${ledger.total}`];
  if (ledger.failed > 0) parts.push(`失败 ${ledger.failed}`);
  if (ledger.skipped > 0) parts.push(`跳过 ${ledger.skipped}`);
  return ledger.running ? `今日执行中（${parts.join("，")}）` : `今日已完成（${parts.join("，")}）`;
}

// —— 以下工具函数与账号页共用同一口径；签到结果文案保持 ADR-0019 业务码透出规则。 ——
function checkinOutcomeLabel(outcome: string, detailCode: string | null = null): string {
  switch (detailCode) {
    case "account_not_logged_in": return "未登录，请先添加账号";
    case "recovery_required": return "存在中断的续期现场，需先恢复再签到";
    case "credential_missing": return "登录材料缺失，请重新登录";
    case "credential_unavailable": return "登录存储不可用，请重新登录";
    case "credential_invalid": return "登录材料无效，请重新登录";
    case "device_too_new": return "设备刚创建，请等待约 5 分钟后重试";
    case "business_9074": return "设备信息未获服务端信任（9074），可在账号详情重置签到设备";
    case "business_9095": return "该设备今日名额已用（9095），明日再试";
    case "remint_failed": return "重置设备失败，请在账号详情手动重试";
    case "business_20324": return "登录已失效（20324），请重新登录";
    default: break;
  }
  // 未知业务码原样透出数值，保证排障不丢信息（ADR-0019：业务码不得压平）。
  if (detailCode && detailCode.startsWith("business_")) return `服务端拒绝（${detailCode.slice("business_".length)}）`;
  switch (outcome) { case "claimed": return "签到成功"; case "already_checked_in": return "今日已签到"; case "not_eligible": return "当前不可领取"; case "verification_failed": return "结果待复核"; case "auth_mismatch": return "授权不匹配"; case "credential_refresh_failed": return "登录续期失败，请重新登录"; case "network_error": return "网络错误"; case "profile_busy": return "账号忙"; default: return "签到失败"; }
}
function formatShortDate(value: string): string {
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return "时间不可用";
  return new Intl.DateTimeFormat("zh-CN", { month: "2-digit", day: "2-digit", hour: "2-digit", minute: "2-digit", hour12: false }).format(date);
}
// 额度数值格式化：保留 1 位小数（与账号页同口径）。
function formatCreditsValue(value: number): string {
  const rounded = Math.round(value * 10) / 10;
  return Number.isInteger(rounded) ? String(rounded) : rounded.toFixed(1);
}
