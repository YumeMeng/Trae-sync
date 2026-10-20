import { useCallback, useEffect, useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { RefreshCw, UserPlus } from "lucide-react";
import type {
  AutoCheckinStatusDto,
  CheckinBatchSummaryDto,
  CheckinCapabilityDto,
  CheckinOverviewEntryDto,
  CheckinResultDto,
  ManagedAccountsViewDto,
} from "../types/account_switch";
import type { AppPage } from "./NavigationRail";
import { PRODUCT_DISPLAY_NAME, sectionStateOf } from "../platform/productRegistry";
import { safeUiErrorMessage } from "../utils/safeUiError";
import { effectiveDisplayName } from "../utils/accountDisplay";
// 统一通知适配层（P11-1 切片②）：操作结果类提示一律经 notify 发出。
import { notify } from "../platform/notify";
import { CheckinSlotBadge, deriveCheckinSlotState } from "./StatusBadges";
import { productUi } from "../platform/productUiRegistry";
import { CheckinPageSkeleton } from "./shared/checkin/CheckinPageSkeleton";
import { CheckinActionsRow } from "./shared/checkin/CheckinActionsRow";
import { CheckinFlowRow } from "./shared/checkin/CheckinFlowRow";
import { CheckinSelectRow, selectResultTone } from "./shared/checkin/CheckinRow";
import { outcomeLabelFor } from "./shared/checkin/OutcomeLabels";

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
}

// 通知前缀取单产品展示名（P15-10 塌缩：不再按发起操作的产品路由）。
function checkinNotification(message: string): string {
  return `${PRODUCT_DISPLAY_NAME}：${message}`;
}

/**
 * 签到页：专注「对账号的每日签到操作」。
 * 账号档案管理在「账号」页；这里只做签到选择、批量执行与逐账号结果。
 * 执行语义遵循 grill D8/ADR-0019：串行逐账号 status→claim→status，
 * 账号间随机间隔 3-8 秒（后端保障），本页只做期望管理提示。
 * 区块能力三态（P11-6）：非 ready 产品渲染诚实说明卡（探针已验证端点、
 * 实装待真实验收 → pending），不发起任何读取与监听。
 */
export function CheckinPage({ active, onNavigate }: CheckinPageProps) {
  // P15-7（ADR-0035）：签到统一走 SOLO 单通道（旧 CN 独立账号池签到区块
  // 已随产品合并移除）；单一签到数据源 = Work 签到档案（16 迁移账号）。
  return <WorkCheckinPage active={active} onNavigate={onNavigate} />;
}

function WorkCheckinPage({ active, onNavigate }: Pick<CheckinPageProps, "active" | "onNavigate">) {
  const sectionState = sectionStateOf("checkin");
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
            notify.error(checkinNotification(safeUiErrorMessage(reason, "账号总览读取失败。")));
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
      if (next.failed === 0) notify.success(checkinNotification("签到流程已完成。"));
      else notify.warning(checkinNotification("部分签到未完成，请查看逐账号结果。"));
      // 签到会更新积分缓存（后端写回）；刷新总览让“今日已签/积分”同步。
      if (realMode) {
        const entries = await invoke<CheckinOverviewEntryDto[]>("get_checkin_overview").catch(() => null);
        if (entries) setOverview(entries);
      }
    } catch (reason: unknown) {
      notify.error(checkinNotification(safeUiErrorMessage(reason, "签到未完成，请稍后重试。")));
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
  const progressTotal = liveProgress?.total ?? runIds.length;
  // 进度条与行阶段推演：liveProgress.completed 是权威游标（后端串行递增）。
  const progressCompleted = liveProgress?.completed ?? 0;
  const cursorId = busy && runIds.length > 0 ? runIds[Math.min(progressCompleted, runIds.length - 1)] : null;

  // 区块能力三态（P11-6，ADR-0032 决策 2/4）：单产品塌缩后签到区块恒 ready，
  // 非 ready 说明卡随 PRODUCT_SECTION_STATES 塌缩移除（P15-10）。
  // P19-B2④：页面编排（门控/页头/状态区/选择区块/进度条/列表/空态）由
  // CheckinPageSkeleton 承担，本组件退化为 TRAE 数据与插槽提供方。
  const product = productUi("trae_cn");

  return (
    <CheckinPageSkeleton
      product={product}
      active={active}
      loading={false}
      headerActions={realMode && poolIds.length > 0 ? (
        <div className="page-header__actions">
          {/* G15 纯本地刷新：重读缓存零联网，与签到/补签的网络操作分工。 */}
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
      ) : undefined}
      statusRegion={(
        <>
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
        </>
      )}
      entryCount={poolIds.length}
      selectedCount={selection.length}
      actions={realMode ? (
        <CheckinActionsRow
          variant="batch"
          enabled={capability?.enabled ?? false}
          busy={busy}
          poolCount={poolIds.length}
          pendingCount={pendingIds.length}
          selectionCount={selection.length}
          onRunAll={() => void handleRunIds(poolIds)}
          onRunPending={() => void handleRunIds(pendingIds)}
          onRunSelected={() => void handleRunIds(selection)}
          onCancel={() => void handleCancel()}
        />
      ) : (
        <CheckinActionsRow
          variant="select"
          enabled={capability?.enabled ?? false}
          busy={busy}
          poolCount={poolIds.length}
          pendingCount={0}
          selectionCount={selection.length}
          onRunAll={() => void handleRun()}
          onRunPending={() => undefined}
          onRunSelected={() => void handleRun()}
          onCancel={() => void handleCancel()}
        />
      )}
      progress={busy ? { completed: progressCompleted, total: progressTotal } : null}
      rows={realMode
        ? overview.map((entry) => {
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
              productId="trae_cn"
              profileId={entry.profile_id}
              displayName={effectiveDisplayName(entry)}
              badge={<CheckinSlotBadge state={deriveCheckinSlotState(entry)} />}
              meta={entryMeta(entry)}
              selected={selection.includes(entry.profile_id)}
              onToggle={() => toggle(entry.profile_id)}
              disabled={busy}
              phase={busy ? (doneInRun ? "done" : isCursor ? (inWait ? "inter_wait" : "running") : "queued") : "idle"}
              result={busy ? (live ?? null) : finalResult}
              waitSecs={inWait ? waitSecs : null}
              onInlineCheckin={() => void handleRunIds([entry.profile_id])}
              inlineEnabled={entry.checked_in !== true}
            />
          );
        })
        : savedPool.map((account) => {
          const result = summary?.results.find((item) => item.profile_id === account.profile_id) ?? null;
          return (
            <CheckinSelectRow
              key={account.profile_id}
              name={account.display_name}
              meta=""
              badge={null}
              selected={selection.includes(account.profile_id)}
              onToggle={() => toggle(account.profile_id)}
              disabled={busy}
              resultLabel={result ? outcomeLabelFor("trae_cn", result.outcome, result.detail_code) : null}
              resultTone={selectResultTone(result?.outcome ?? null)}
            />
          );
        })}
      listVariant={realMode ? "flow" : "plain"}
      footer={autoResults.length > 0 ? (
        <ul className="account-center__checkin-results" aria-label="自动签到结果" data-testid="auto-checkin-results">
          {autoResults.map((item) => (
            <li key={item.profile_id}>
              <strong>{item.screen_name || displayName(item.profile_id)}</strong>
              <span>{outcomeLabelFor("trae_cn", item.outcome, item.detail_code)}</span>
            </li>
          ))}
        </ul>
      ) : undefined}
      emptyState={{
        className: "checkin-page__empty-actions",
        description: "还没有可签到的账号。先在「账号」页通过浏览器登录添加账号。",
        action: (
          <button className="btn btn--primary" type="button" onClick={() => onNavigate("accounts")}>
            <UserPlus size={15} aria-hidden="true" />去账号页添加
          </button>
        ),
      }}
    />
  );
}

// —— 以下为 TRAE 产品层私有工具（骨架插槽的数据组装面）——
// CheckinFlowRow / CheckinRow / 批量按钮组已抽取到 shared/checkin/（P19-B2④）；
// 结果文案表迁移至 shared/checkin/OutcomeLabels.ts（per-product 映射）。

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

// —— TRAE 业务码透出表已迁移至 shared/checkin/OutcomeLabels.ts（P19-B2④）。 ——
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
