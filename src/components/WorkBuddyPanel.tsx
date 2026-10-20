import { useCallback, useEffect, useRef, useState } from "react";
import { Activity, ArrowRightLeft, Loader2, RefreshCw, ShieldCheck } from "lucide-react";
import type { ReactNode } from "react";
import type {
  WorkBuddySessionHealthDto,
  WorkBuddyCheckinStatusDto,
  WorkBuddyLoginSummaryDto,
  WorkBuddyTransferDto,
  WorkBuddyDeepHealthDto,
} from "../types/product";
import {
  captureWorkBuddyAccount,
  checkWorkBuddyLoginHealth,
  claimWorkBuddyCheckin,
  fetchWorkBuddyAutoCheckinStatus,
  fetchWorkBuddyCheckinStatus,
  listWorkBuddyAccounts,
  loadWorkBuddySessionHealth,
  planWorkBuddySessionTransfer,
  refreshWorkBuddyAccount,
  setWorkBuddyAccountAutoCheckin,
  setWorkBuddyAutoCheckinSettings,
  switchWorkBuddyAccount,
  transferWorkBuddySessions,
  type WorkBuddyAutoCheckinStatusDto,
} from "../platform/workbuddyAdapter";
import {
  CheckinSlotBadge,
  WorkBuddyLoginSlotBadge,
  WorkBuddyPoolSlotBadge,
  deriveCheckinSlotState,
  deriveWorkBuddyPoolLoginState,
} from "./StatusBadges";
import { ConfirmDialog } from "./shared/ConfirmDialog";
import { AccountPageSkeleton } from "./shared/account/AccountPageSkeleton";
import { ACCOUNT_SORT_OPTIONS, type AccountSort } from "./shared/account/AccountListControls";
import { SharedAccountEntry, type AccountView, type SharedAccountRecord } from "./shared/account/AccountRow";
import { accountPreferenceKey, readStoredPreference, writeStoredPreference } from "./shared/account/preferences";
import { CheckinPageSkeleton } from "./shared/checkin/CheckinPageSkeleton";
import { AutoCheckinSettingsCard } from "./shared/checkin/AutoCheckinSettingsCard";
import { productUi } from "../platform/productUiRegistry";
import { notify } from "../platform/notify";
import { safeUiErrorMessage } from "../utils/safeUiError";

/**
 * WorkBuddy 产品域页面（P17-T5 并列骨架一期）。
 *
 * 两个页面槽位组件：
 * - WorkBuddyPanel：账号页（会话健康 + 捕获入池 + 工具账号池清单）；
 * - WorkBuddyCheckinPage：签到页（直连签到状态 + 领取）。
 *
 * 数据语义全部在后端组合根命令中（workbuddy_commands.rs）；本文件只做
 * 读取时机（active 才发起 IPC）、加载/错误三态与单次确认弹窗编排。
 * 界面表达纪律：不展示 uid/uin/profile_id 等内部标识，主信息只用
 * 状态、名称、时间与动作结果。
 */

/** Unix 秒 → 本地「日期 时:分」；0 = 未知返回 null。 */
function formatUnixTime(unixSeconds: number): string | null {
  if (!unixSeconds) return null;
  return new Date(unixSeconds * 1000).toLocaleString(undefined, {
    year: "numeric",
    month: "2-digit",
    day: "2-digit",
    hour: "2-digit",
    minute: "2-digit",
  });
}

/** 深度健康检测实时面 → 展示文案（P18-B3）。 */
function liveHealthLabel(liveStatus: WorkBuddyDeepHealthDto["live_status"]): string {
  switch (liveStatus) {
    case "verified":
      return "已验证可用（客户端运行中，凭据实时回读通过）";
    case "client_offline":
      return "客户端未运行，已完成本地检测";
    case "probe_failed":
      return "客户端运行中但凭据回读异常，建议重启客户端后重试";
    default:
      return "状态未知";
  }
}

/** 信息行：label + value 的两列展示（卡内统一行结构）。 */
function InfoRow({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div className="workbuddy-row">
      <span className="workbuddy-row__label">{label}</span>
      <span className="workbuddy-row__value">{children}</span>
    </div>
  );
}

/**
 * P18-B1 过户确认弹窗文案（界面表达纪律：昵称+数量表达，不用 uid/内部
 * 编号；方向识别走回退路径时如实告知「按最近活跃识别」）。
 */
function transferDialogLines(plan: WorkBuddyTransferDto): string[] {
  const lines = [
    `将把上一个登录账号（${plan.previous_label ?? "未知"}）名下的 ${plan.pending_local_rows} 条本地会话转移给当前登录账号（${plan.current_label}）。`,
    "转移只改写本机会话索引的归属标记：不删除任何会话，也不触碰云端会话；会话正文文件零改动。",
  ];
  if (plan.cloud_residual_rows > 0) {
    lines.push(
      `检测到上一账号名下有 ${plan.cloud_residual_rows} 条云端会话，本次不会改动它们。`,
    );
  }
  if (plan.other_account_rows > 0) {
    lines.push(
      `另有 ${plan.other_account_rows} 条更早账号的本地会话不在本次转移范围。`,
    );
  }
  if (plan.direction_source === "sessions_fallback") {
    lines.push("账号方向按本机会话记录的最近活跃情况识别（未找到登录日志）。");
  }
  lines.push("执行时需要完全退出 WorkBuddy 客户端，转移完成后会自动重启。");
  return lines;
}

/** P18-B3 对齐 CheckinPage 口径：WorkBuddy 自动签到台账文案。 */
function wbAutoLedgerLabel(status: WorkBuddyAutoCheckinStatusDto): string {
  const ledger = status.ledger;
  if (!ledger) return "今日尚未发起";
  const parts = [`成功 ${ledger.completed}/${ledger.total}`];
  if (ledger.failed > 0) parts.push(`失败 ${ledger.failed}`);
  if (ledger.skipped > 0) parts.push(`跳过 ${ledger.skipped}`);
  return ledger.running ? `今日执行中（${parts.join("，")}）` : `今日已完成（${parts.join("，")}）`;
}

// ============================================================================
// 账号页：WorkBuddy 会话健康 + 捕获入池 + 工具账号池
// ============================================================================

interface WorkBuddyPanelProps {
  /** 页面可见时才读取（hidden 槽位常驻，不 visible 不发 IPC）。 */
  active: boolean;
}

export function WorkBuddyPanel({ active }: WorkBuddyPanelProps) {
  const [health, setHealth] = useState<WorkBuddySessionHealthDto | null>(null);
  const [pool, setPool] = useState<readonly WorkBuddyLoginSummaryDto[]>([]);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const [refreshing, setRefreshing] = useState(false);
  // P18-B3：积分入账号卡（签到状态 best-effort 读取，失败不阻塞健康卡）。
  const [checkinStatus, setCheckinStatus] = useState<WorkBuddyCheckinStatusDto | null>(null);
  // P18-B3：深度健康检测（文件面 + 实时面；按钮单发，结果常驻健康卡）。
  const [deepHealth, setDeepHealth] = useState<WorkBuddyDeepHealthDto | null>(null);
  const [checkingHealth, setCheckingHealth] = useState(false);
  // 捕获：单次确认弹窗 → 执行 → 刷新健康与账号池。
  const [captureDialogOpen, setCaptureDialogOpen] = useState(false);
  const [capturing, setCapturing] = useState(false);
  // 凭据续期（与 Trae「刷新登录凭据」同语义）：access 充裕跳过不轮换。
  const [renewing, setRenewing] = useState(false);
  // P17-T6/A4 切号：单次确认弹窗（目标 profile_id）→ 执行 → 刷新健康与池。
  const [switchTarget, setSwitchTarget] = useState<string | null>(null);
  const [switching, setSwitching] = useState(false);
  // U-2 双视图与排序（P19-B2⑤ 对齐账号页模板；key 按产品分前缀）。
  const [accountView, setAccountView] = useState<AccountView>(() =>
    readStoredPreference(accountPreferenceKey("workbuddy", "view"), "list", ["list", "card"] as const));
  const [accountSort, setAccountSort] = useState<AccountSort>(() =>
    readStoredPreference<AccountSort>(accountPreferenceKey("workbuddy", "sort"), "added", ["added", "name"]));
  const changeAccountView = useCallback((next: AccountView) => {
    setAccountView(next);
    writeStoredPreference(accountPreferenceKey("workbuddy", "view"), next);
  }, []);
  const changeAccountSort = useCallback((next: AccountSort) => {
    setAccountSort(next);
    writeStoredPreference(accountPreferenceKey("workbuddy", "sort"), next);
  }, []);

  const refresh = useCallback(async (initial: boolean) => {
    if (initial) setLoading(true);
    else setRefreshing(true);
    try {
      const [nextHealth, nextPool] = await Promise.all([
        loadWorkBuddySessionHealth(),
        listWorkBuddyAccounts(),
      ]);
      setHealth(nextHealth);
      setPool(nextPool);
      setLoadError(null);
      // 积分数据 best-effort：失败只影响积分行显示（置空），不阻塞健康卡。
      fetchWorkBuddyCheckinStatus()
        .then((status) => setCheckinStatus(status))
        .catch(() => setCheckinStatus(null));
    } catch (reason: unknown) {
      setLoadError(safeUiErrorMessage(reason, "WorkBuddy 登录状态暂时不可读取，请稍后重试。"));
    } finally {
      setLoading(false);
      setRefreshing(false);
    }
  }, []);

  // P18-B3 深度健康检测：文件面 + 实时面（只读零写副作用）；结果常驻健康卡。
  const runDeepHealth = useCallback(async () => {
    setCheckingHealth(true);
    try {
      const result = await checkWorkBuddyLoginHealth();
      setDeepHealth(result);
      if (result.live_status === "verified") {
        notify.success("WorkBuddy：健康检测通过（本地登录态有效，实时凭据验证可用）");
      } else if (result.live_status === "probe_failed") {
        notify.error("WorkBuddy：健康检测发现异常——客户端运行中但凭据回读失败，建议重启客户端。");
      } else {
        notify.info("WorkBuddy：本地健康检测完成（客户端未运行，未做实时验证）。");
      }
    } catch (reason: unknown) {
      notify.error(
        `WorkBuddy：健康检测未完成——${safeUiErrorMessage(reason, "请稍后重试。")}`,
      );
    } finally {
      setCheckingHealth(false);
    }
  }, []);

  useEffect(() => {
    if (!active) return;
    void refresh(true);
  }, [active, refresh]);

  const runCapture = useCallback(async () => {
    setCapturing(true);
    try {
      const saved = await captureWorkBuddyAccount();
      const label = saved.screen_name || "WorkBuddy 账号";
      notify.success(`WorkBuddy：已保存账号「${label}」到账号池`);
      setCaptureDialogOpen(false);
      await refresh(false);
    } catch (reason: unknown) {
      notify.error(
        `WorkBuddy：保存失败——${safeUiErrorMessage(reason, "账号保存未完成，请稍后重试。")}`,
      );
    } finally {
      setCapturing(false);
    }
  }, [refresh]);

  // 客户端未登录时捕获必然失败（后端第一步就是会话 Active 校验），UI 提前禁用。
  const captureDisabled = health !== null && health.status !== "active";

  // 凭据续期：renewed=true 已换新钥匙；false = 仍充裕（不轮换，保护客户端副本）。
  const runRenew = useCallback(async () => {
    setRenewing(true);
    try {
      const status = await refreshWorkBuddyAccount();
      const until = formatUnixTime(status.access_expires_at_unix_seconds) ?? "未知";
      if (status.renewed) {
        notify.success(`WorkBuddy：登录凭据已更新，有效期至 ${until}`);
      } else {
        notify.info(`WorkBuddy：登录凭据仍充裕（有效期至 ${until}），本次未刷新`);
      }
      await refresh(false);
    } catch (reason: unknown) {
      notify.error(
        `WorkBuddy：刷新登录凭据失败——${safeUiErrorMessage(reason, "凭据续期未完成，请稍后重试。")}`,
      );
    } finally {
      setRenewing(false);
    }
  }, [refresh]);

  // P17-T6/A4 切号：wbipc RPC 编排在后端（logout → saveOauthToken →
  // 回读校验）。P18-B1 认知修正：会话 = 本地 SQLite 索引（按账号隔离）+
  // 本地 jsonl 正文（全局共享）；切号只换登录态，历史会话不可见需用
  // 「转移历史会话」过户（改写索引归属，不碰正文与云端会话）。
  const runSwitch = useCallback(async () => {
    if (!switchTarget) return;
    setSwitching(true);
    try {
      const receipt = await switchWorkBuddyAccount(switchTarget);
      const label = receipt.screen_name || "WorkBuddy 账号";
      notify.success(
        receipt.renewed
          ? `WorkBuddy：已切换到「${label}」（凭据已先续期为最新）`
          : `WorkBuddy：已切换到「${label}」`,
      );
      setSwitchTarget(null);
      await refresh(false);
    } catch (reason: unknown) {
      notify.error(
        `WorkBuddy：切换未完成——${safeUiErrorMessage(reason, "切号未完成，客户端可能停在登出状态，请在客户端重新登录或重试。")}`,
      );
    } finally {
      setSwitching(false);
    }
  }, [refresh, switchTarget]);

  // P18-B1 会话过户：先出计划（dry_run，零副作用）→ 单次确认弹窗 → 执行。
  const [transferPlan, setTransferPlan] = useState<WorkBuddyTransferDto | null>(null);
  const [planningTransfer, setPlanningTransfer] = useState(false);
  const [transferring, setTransferring] = useState(false);

  const openTransferPlan = useCallback(async () => {
    setPlanningTransfer(true);
    try {
      const plan = await planWorkBuddySessionTransfer();
      if (plan.nothing_to_transfer) {
        notify.info("WorkBuddy：没有需要转移的历史会话，当前账号已是最新。");
        return;
      }
      setTransferPlan(plan);
    } catch (reason: unknown) {
      notify.error(
        `WorkBuddy：无法生成转移计划——${safeUiErrorMessage(reason, "请稍后重试。")}`,
      );
    } finally {
      setPlanningTransfer(false);
    }
  }, []);

  const runTransfer = useCallback(async () => {
    if (!transferPlan) return;
    setTransferring(true);
    try {
      const receipt = await transferWorkBuddySessions();
      const missing = receipt.missing_body_files ?? 0;
      const restartNote = receipt.restarted ? "，WorkBuddy 已重启" : "（客户端未自动重启，可手动启动）";
      notify.success(
        `WorkBuddy：已转移 ${receipt.updated_rows ?? 0} 条会话${restartNote}` +
          (missing > 0 ? `；正文抽验发现 ${missing} 条缺失，详见转移记录` : "") ,
      );
      setTransferPlan(null);
      await refresh(false);
    } catch (reason: unknown) {
      notify.error(
        `WorkBuddy：转移未完成——${safeUiErrorMessage(reason, "会话转移未完成，数据未被改动，请稍后重试。")}`,
      );
    } finally {
      setTransferring(false);
    }
  }, [refresh, transferPlan]);

  // P19-B2⑤：账号页编排（门控/页头/健康卡位/控件行/池行/空态）由
  // AccountPageSkeleton 承担（ADR-0036 铁律），本组件退化为 WB 数据与插槽提供方。
  const uiProduct = productUi("workbuddy");
  const visiblePool = accountSort === "name"
    ? [...pool].sort((a, b) => (a.screen_name || "").localeCompare(b.screen_name || "", "zh-Hans-CN"))
    : pool;
  const poolEntries = visiblePool.map((account) => ({
    id: account.profile_id,
    displayName: account.screen_name || "WorkBuddy 账号",
    lastVerifiedAt: null,
    // 「使用中」= 当前客户端登录账号（健康卡的 uid 与池账号 account_id 同源）。
    isSelected: health?.uid === account.account_id,
  }));
  const renderPoolRow = (record: SharedAccountRecord) => {
    const account = pool.find((item) => item.profile_id === record.id);
    if (!account) return null;
    const poolState = deriveWorkBuddyPoolLoginState({
      access_expires_at_unix_seconds: account.access_token_expires_at_unix_seconds,
      refresh_expires_at_unix_seconds: account.refresh_token_expires_at_unix_seconds,
      now_unix_seconds: Math.floor(Date.now() / 1000),
    });
    // creditsColumn 能力开关（IA 草案 §1.2）：仅当前客户端登录账号有积分数据面。
    const creditsMetric = uiProduct.capabilities.creditsColumn && record.isSelected && checkinStatus ? (
      <div className="account-item__credits" title="WorkBuddy 当前登录账号的积分余额（签到数据面）">
        <strong data-testid={`workbuddy-credits-${account.account_id}`}>{checkinStatus.total_credits}</strong>
        <span className="account-item__credits-label">积分余额</span>
      </div>
    ) : null;
    const cardCredits = uiProduct.capabilities.creditsColumn && record.isSelected && checkinStatus ? (
      <div className="account-card__credits" title="WorkBuddy 当前登录账号的积分余额（签到数据面）">
        <span className="account-card__credits-value" data-testid={`workbuddy-credits-${account.account_id}`}>
          {checkinStatus.total_credits}
        </span>
        <span className="account-item__credits-label">积分余额</span>
      </div>
    ) : null;
    const expiryMeta = (
      <p className="account-item__meta">
        登录有效期至 {formatUnixTime(account.access_token_expires_at_unix_seconds) ?? "未知"}
        {" · "}
        更新于 {formatUnixTime(account.last_updated_at_unix_seconds) ?? "未知"}
      </p>
    );
    return (
      <SharedAccountEntry
        account={record}
        variant={accountView}
        testId={`workbuddy-pool-card-${account.profile_id}`}
        actionTestId={`workbuddy-switch-${account.profile_id}`}
        selectedTestId={`workbuddy-pool-current-${account.profile_id}`}
        statusContent={<WorkBuddyPoolSlotBadge state={poolState} />}
        listMeta={expiryMeta}
        cardMeta={expiryMeta}
        listMetric={creditsMetric}
        cardMetric={cardCredits}
        actionBusy={switching}
        onAction={() => setSwitchTarget(account.profile_id)}
        actionTitle="让 WorkBuddy 客户端切换登录到此账号（切换后如看不到历史会话，可用「转移历史会话」恢复可见）"
        actionLabel="切换到此账号"
      />
    );
  };

  return (
    <AccountPageSkeleton
      product={uiProduct}
      active={active}
      loading={loading}
      ariaLabel="账号"
      selectionResetKey="workbuddy"
      headerActions={(
        <>
          <button
            type="button"
            className="btn"
            onClick={() => void refresh(false)}
            disabled={refreshing || loading}
            data-testid="workbuddy-refresh"
          >
            <RefreshCw size={15} className={refreshing ? "icon-spin" : undefined} aria-hidden="true" />
            刷新
          </button>
          <button
            type="button"
            className="btn"
            onClick={() => void runRenew()}
            disabled={renewing || loading || pool.length === 0}
            data-testid="workbuddy-renew-credentials"
            title={pool.length === 0 ? "还没有已保存的账号" : "续期账号池中的登录凭据（有效期充裕时自动跳过）"}
          >
            <RefreshCw size={15} className={renewing ? "icon-spin" : undefined} aria-hidden="true" />
            刷新登录凭据
          </button>
          <button
            type="button"
            className="btn"
            onClick={() => void runDeepHealth()}
            disabled={checkingHealth || loading}
            data-testid="workbuddy-deep-health-button"
            title="检测本地登录态与客户端实时凭据（只读，不修改任何数据）"
          >
            <Activity size={15} className={checkingHealth ? "icon-spin" : undefined} aria-hidden="true" />
            健康检测
          </button>
          <button
            type="button"
            className="btn"
            onClick={() => void openTransferPlan()}
            disabled={planningTransfer || transferring || loading}
            data-testid="workbuddy-transfer-button"
            title="把上一个登录账号的本地历史会话转移给当前登录账号（切换账号后看不到历史会话时使用）"
          >
            <ArrowRightLeft size={15} className={transferring ? "icon-spin" : undefined} aria-hidden="true" />
            转移历史会话
          </button>
          <button
            type="button"
            className="btn btn--primary"
            onClick={() => setCaptureDialogOpen(true)}
            disabled={captureDisabled || capturing || loading}
            data-testid="workbuddy-capture-button"
            title={captureDisabled ? "WorkBuddy 客户端当前未登录，请先在客户端中登录" : undefined}
          >
            <ShieldCheck size={15} aria-hidden="true" />
            保存当前登录账号
          </button>
        </>
      )}
      errorState={loadError ? {
        title: "无法读取 WorkBuddy 登录状态",
        description: loadError,
        action: (
          <button type="button" className="btn btn--primary" onClick={() => void refresh(true)}>
            重新读取
          </button>
        ),
      } : undefined}
      statusRegion={health ? (
        <div className="workbuddy-card" data-testid="workbuddy-health-card">
          <div className="workbuddy-card__title">客户端登录状态</div>
          <InfoRow label="登录状态">
            {/* P18-B3：徽章统一走 G10 slot-badge 体系（与账号页/签到页同义同色）。 */}
            <WorkBuddyLoginSlotBadge status={health.status} />
          </InfoRow>
          <InfoRow label="账号池">
            {health.captured_in_pool
              ? "当前登录账号已保存到 Trae Sync 账号池"
              : "尚未保存到 Trae Sync 账号池"}
          </InfoRow>
          <InfoRow label="登录有效期至">
            {formatUnixTime(health.access_expires_at_unix_seconds) ?? "未知"}
          </InfoRow>
          <InfoRow label="续期凭据有效期至">
            {formatUnixTime(health.refresh_expires_at_unix_seconds) ?? "未知"}
          </InfoRow>
          {/* P18-B3：积分入账号卡（签到数据面已有；读取失败显示未知）。 */}
          <InfoRow label="积分余额">
            {checkinStatus ? `${checkinStatus.total_credits} 积分` : "未知"}
          </InfoRow>
          <InfoRow label="已连续签到">
            {checkinStatus ? `${checkinStatus.streak_days} 天` : "未知"}
          </InfoRow>
          {deepHealth && (
            <InfoRow label="健康检测">
              <span
                className={`slot-badge ${deepHealth.live_status === "verified" ? "slot-badge--ok" : deepHealth.live_status === "probe_failed" ? "slot-badge--danger" : "slot-badge--idle"}`}
                data-testid="workbuddy-deep-health-result"
              >
                <i className="slot-badge__dot" aria-hidden="true" />
                {liveHealthLabel(deepHealth.live_status)}
              </span>
            </InfoRow>
          )}
        </div>
      ) : null}
      entryCount={pool.length}
      showControls={pool.length > 0}
      controls={{
        view: accountView,
        onViewChange: changeAccountView,
        sort: accountSort,
        sortOptions: ACCOUNT_SORT_OPTIONS.filter((option) => option !== "checkin"),
        onSortChange: changeAccountSort,
        // attentionFilter=false（IA 草案）：WB 暂无凭据异常过滤口径，不渲染过滤器行。
        filter: undefined,
      }}
      listTestId="workbuddy-pool-list"
      entries={poolEntries}
      renderRow={(record) => renderPoolRow(record)}
      emptyState={{
        className: "workbuddy-page__empty",
        description: "还没有保存 WorkBuddy 账号。在 WorkBuddy 客户端登录后，点击右上角「保存当前登录账号」。",
      }}
      detailView={() => null}
      dialogs={(
        <>
          {captureDialogOpen && (
            <ConfirmDialog
              title="保存当前 WorkBuddy 登录账号"
              lines={[
                "将从本机 WorkBuddy 客户端读取当前登录会话，加密保存到 Trae Sync 的账号池。",
                "只读取登录凭据，不会修改 WorkBuddy 客户端的任何数据。",
                "同一账号重复保存会用最新凭据覆盖旧记录。",
              ]}
              confirmLabel="保存账号"
              busyLabel="保存中…"
              busy={capturing}
              onCancel={() => setCaptureDialogOpen(false)}
              onConfirm={() => void runCapture()}
              testId="workbuddy-capture-dialog"
            />
          )}

          {/* P17-T6/A4 切号单次确认（覆盖性操作：改写客户端登录态）。 */}
          {switchTarget && (
            <ConfirmDialog
              title="切换 WorkBuddy 登录账号"
              lines={[
                "将让 WorkBuddy 客户端退出当前账号并登录到所选账号（写入客户端登录态）。",
                "历史会话记录在本机并按账号隔离；切换后如看不到历史会话，可在本页使用「转移历史会话」恢复可见。",
                "需要 WorkBuddy 客户端处于运行状态；切换完成后客户端即已登录新账号。",
              ]}
              confirmLabel="确认切换"
              busyLabel="切换中…"
              busy={switching}
              onCancel={() => {
                if (!switching) setSwitchTarget(null);
              }}
              onConfirm={() => void runSwitch()}
              testId="workbuddy-switch-dialog"
            />
          )}

          {/* P18-B1 会话过户单次确认（破坏性批量操作纪律；计划由后端 dry_run 生成）。 */}
          {transferPlan && (
            <ConfirmDialog
              title="转移历史会话"
              lines={transferDialogLines(transferPlan)}
              confirmLabel="确认转移"
              busyLabel="转移中…"
              busy={transferring}
              onCancel={() => {
                if (!transferring) setTransferPlan(null);
              }}
              onConfirm={() => void runTransfer()}
              testId="workbuddy-transfer-dialog"
            />
          )}
        </>
      )}
    />
  );
}

// ============================================================================
// 签到页：WorkBuddy 直连签到（status/claim 幂等编排在后端）
// ============================================================================

interface WorkBuddyCheckinPageProps {
  active: boolean;
}

/**
 * 参与集「显式设置过」的本机记录（P19-B2 参与集默认全参与修复，前端侧）：
 * 用户在 UI 显式拨动过参与开关的账号 id。未记录的账号视为「从未显式设置」，
 * UI 默认视为参与，且池加载后自动批量置 true 落到后端（八域对齐方案第 3 条：
 * 默认全参与）；显式关闭（touched 且 false）永远尊重。
 * 注：本机存储丢失会导致一次重新归一（把未显式关闭的账号重新置为参与），
 * 可接受；持久的 opt-out 语义归后端 auto_checkin_store（架构草案 §1.2E）。
 */
const WB_AUTO_CHECKIN_TOUCHED_KEY = "auto-checkin.workbuddy.touched";

function loadTouchedParticipation(): Set<string> {
  try {
    const raw = window.localStorage.getItem(WB_AUTO_CHECKIN_TOUCHED_KEY);
    const parsed: unknown = raw ? JSON.parse(raw) : [];
    return Array.isArray(parsed) ? new Set(parsed.filter((id): id is string => typeof id === "string")) : new Set();
  } catch {
    return new Set();
  }
}

function saveTouchedParticipation(touched: Set<string>): void {
  try {
    window.localStorage.setItem(WB_AUTO_CHECKIN_TOUCHED_KEY, JSON.stringify([...touched]));
  } catch { /* 存储不可用时仅本次会话生效 */ }
}

export function WorkBuddyCheckinPage({ active }: WorkBuddyCheckinPageProps) {
  const [status, setStatus] = useState<WorkBuddyCheckinStatusDto | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const [claiming, setClaiming] = useState(false);
  // P17-A5 每日自动签到：设置 + 账号参与清单（与 Trae 自动签到同策略）。
  const [autoStatus, setAutoStatus] = useState<WorkBuddyAutoCheckinStatusDto | null>(null);
  const [autoBusy, setAutoBusy] = useState(false);
  // 参与集显式设置记录（localStorage；见上方注释）。
  const touchedRef = useRef<Set<string>>(loadTouchedParticipation());
  // 参与集归一守卫：每次 autoStatus 到达最多跑一轮，防止失败循环重试。
  const normalizedRef = useRef(false);

  const touch = useCallback((profileId: string) => {
    touchedRef.current.add(profileId);
    saveTouchedParticipation(touchedRef.current);
  }, []);

  const refresh = useCallback(async (initial: boolean) => {
    if (initial) setLoading(true);
    try {
      const next = await fetchWorkBuddyCheckinStatus();
      setStatus(next);
      setLoadError(null);
    } catch (reason: unknown) {
      setLoadError(safeUiErrorMessage(reason, "签到状态暂时不可读取，请稍后重试。"));
    } finally {
      setLoading(false);
    }
  }, []);

  const refreshAuto = useCallback(async () => {
    try {
      setAutoStatus(await fetchWorkBuddyAutoCheckinStatus());
    } catch {
      setAutoStatus(null); // 调度数据读失败静默（分区可插拔语义）。
    }
  }, []);

  useEffect(() => {
    if (!active) return;
    void refresh(true);
    void refreshAuto();
  }, [active, refresh, refreshAuto]);

  // P19-B2 参与集默认全参与：池加载后，从未显式设置过的账号自动批量置 true
  //（一次归一；显式关闭过的账号不动）。
  useEffect(() => {
    if (!autoStatus || normalizedRef.current) return;
    normalizedRef.current = true;
    const unset = autoStatus.accounts.filter(
      (account) => !touchedRef.current.has(account.profile_id) && !account.enabled,
    );
    if (unset.length === 0) return;
    void (async () => {
      for (const account of unset) {
        await setWorkBuddyAccountAutoCheckin(account.profile_id, true).catch(() => undefined);
        touch(account.profile_id);
      }
      await refreshAuto();
    })();
  }, [autoStatus, refreshAuto, touch]);

  const handleSaveAutoSettings = useCallback(
    (enabled: boolean, dailyTimeHhmm: string) => {
      if (autoBusy) return;
      setAutoBusy(true);
      void setWorkBuddyAutoCheckinSettings(enabled, dailyTimeHhmm)
        .then(refreshAuto)
        .then(() => {
          notify.success(enabled
            ? `WorkBuddy：每日自动签到已开启，每日 ${dailyTimeHhmm} 后自动执行（参与账号逐个错峰）。`
            : "WorkBuddy：每日自动签到已关闭。");
        })
        .catch((reason: unknown) => notify.error(
          `WorkBuddy：自动签到设置保存失败——${safeUiErrorMessage(reason, "请稍后重试。")}`,
        ))
        .finally(() => setAutoBusy(false));
    },
    [autoBusy, refreshAuto],
  );

  const handleToggleAccount = useCallback(
    (profileId: string, enabled: boolean) => {
      if (autoBusy) return;
      touch(profileId); // 显式设置即记录，此后尊重用户选择。
      setAutoBusy(true);
      void setWorkBuddyAccountAutoCheckin(profileId, enabled)
        .then(refreshAuto)
        .catch((reason: unknown) => notify.error(
          `WorkBuddy：参与状态保存失败——${safeUiErrorMessage(reason, "请稍后重试。")}`,
        ))
        .finally(() => setAutoBusy(false));
    },
    [autoBusy, refreshAuto, touch],
  );

  // 批量置 true 入口：把未参与账号全部置为参与（显式记录，防归一回翻）。
  const handleParticipateAll = useCallback(() => {
    if (autoBusy || !autoStatus) return;
    const targets = autoStatus.accounts.filter(
      (account) => !account.enabled
        || (!touchedRef.current.has(account.profile_id) && !account.enabled),
    );
    if (targets.length === 0) return;
    setAutoBusy(true);
    void (async () => {
      for (const account of targets) {
        await setWorkBuddyAccountAutoCheckin(account.profile_id, true).catch(() => undefined);
        touch(account.profile_id);
      }
      await refreshAuto();
      notify.success(`WorkBuddy：已把 ${targets.length} 个账号全部置为参与自动签到。`);
    })().finally(() => setAutoBusy(false));
  }, [autoBusy, autoStatus, refreshAuto, touch]);

  const runClaim = useCallback(async () => {
    setClaiming(true);
    try {
      const claim = await claimWorkBuddyCheckin();
      notify.success(
        `WorkBuddy：签到成功，获得 ${claim.credit} 积分，已连续签到 ${claim.streak_days} 天`,
      );
      await refresh(false);
    } catch (reason: unknown) {
      notify.error(
        `WorkBuddy：签到未完成——${safeUiErrorMessage(reason, "签到未完成，请稍后重试。")}`,
      );
    } finally {
      setClaiming(false);
    }
  }, [refresh]);

  // P19-B2⑤：页面编排（门控/页头/状态行）由 CheckinPageSkeleton 承担；
  // WB 签到形态为 single（单账号卡 + 自动签到设置卡），批量行升级归 B5。
  const uiProduct = productUi("workbuddy");
  // 参与集 UI 生效态：显式设置过的账号按后端值；未设置过的默认参与。
  const participationAccounts = (autoStatus?.accounts ?? []).map((account) => ({
    profileId: account.profile_id,
    displayName: account.screen_name,
    participating: touchedRef.current.has(account.profile_id) ? account.enabled : true,
  }));

  return (
    <CheckinPageSkeleton
      product={uiProduct}
      active={active}
      loading={loading}
      headerActions={status ? (
        <div className="page-header__actions">
          <button
            type="button"
            className="btn"
            onClick={() => void refresh(false)}
            disabled={loading || claiming}
            data-testid="workbuddy-checkin-refresh"
          >
            <RefreshCw size={15} className={loading ? "icon-spin" : undefined} aria-hidden="true" />
            刷新
          </button>
        </div>
      ) : undefined}
      errorState={loadError ? {
        title: "无法读取签到状态",
        description: loadError,
        action: (
          <button type="button" className="btn btn--primary" onClick={() => void refresh(true)}>
            重新读取
          </button>
        ),
      } : undefined}
      statusRegion={autoStatus ? (
        <p className="checkin-page__meta" role="status" data-testid="workbuddy-auto-checkin-meta">
          自动签到：{autoStatus.enabled ? `每日 ${autoStatus.daily_time_hhmm} 触发` : "已关闭"}
          {autoStatus.enabled ? ` · ${wbAutoLedgerLabel(autoStatus)}` : ""}
        </p>
      ) : null}
      content={status ? (
        <>
          <div className="workbuddy-card" data-testid="workbuddy-checkin-card">
            <div className="workbuddy-card__title">每日签到</div>
            <InfoRow label="今日状态">
              {/* P18-B3：签到槽徽章统一走 G10 CheckinSlotBadge（与 Trae 签到页同义同色）。 */}
              <CheckinSlotBadge state={deriveCheckinSlotState({ checked_in: status.today_checked_in })} />
            </InfoRow>
            <InfoRow label="已连续签到">{status.streak_days} 天</InfoRow>
            <InfoRow label="今日可得">+{status.daily_credit} 积分</InfoRow>
            <InfoRow label="积分余额">{status.total_credits} 积分</InfoRow>
            <div className="workbuddy-card__actions">
              <button
                type="button"
                className="btn btn--primary"
                onClick={() => void runClaim()}
                disabled={status.today_checked_in || claiming}
                data-testid="workbuddy-checkin-button"
              >
                {claiming && <Loader2 size={15} className="icon-spin" aria-hidden="true" />}
                {status.today_checked_in ? "今日已签到" : claiming ? "签到中…" : "立即签到"}
              </button>
            </div>
          </div>

          {/* P17-A5 每日自动签到：设置 + 账号参与（共享设置卡，P19-B2⑤ 抽取）。 */}
          {autoStatus && (
            <AutoCheckinSettingsCard
              testIdPrefix="workbuddy"
              enabled={autoStatus.enabled}
              dailyTimeHhmm={autoStatus.daily_time_hhmm}
              busy={autoBusy}
              toggleSummary="开启后到点自动执行（断连自动续期兜底）"
              ledgerNote={autoStatus.ledger
                ? autoStatus.ledger.running
                  ? `今日执行中（完成 ${autoStatus.ledger.completed}/${autoStatus.ledger.total}${autoStatus.ledger.failed ? `，失败 ${autoStatus.ledger.failed}` : ""}）`
                  : `今日已完成（成功 ${autoStatus.ledger.completed}/${autoStatus.ledger.total}${autoStatus.ledger.failed ? `，失败 ${autoStatus.ledger.failed}` : ""}）`
                : null}
              accounts={participationAccounts}
              onSettingsChange={handleSaveAutoSettings}
              onParticipationChange={handleToggleAccount}
              onParticipateAll={handleParticipateAll}
            />
          )}
        </>
      ) : null}
    />
  );
}
