import { useCallback, useEffect, useRef, useState } from "react";
import { Activity, RefreshCw, ShieldCheck, UserPlus } from "lucide-react";
import type { ReactNode } from "react";
import type {
  WorkBuddySessionHealthDto,
  WorkBuddyCheckinAccountStatusDto,
  WorkBuddyCheckinParticipationDto,
  WorkBuddyLoginSummaryDto,
  WorkBuddyDeepHealthDto,
} from "../types/product";
import {
  beginWorkBuddyLogin,
  cancelWorkBuddyLogin,
  captureWorkBuddyAccount,
  checkWorkBuddyLoginHealth,
  claimWorkBuddyCheckinBatch,
  claimWorkBuddyCheckinProfile,
  completeWorkBuddyLogin,
  fetchWorkBuddyAutoCheckinStatus,
  fetchWorkBuddyCheckinOverview,
  listWorkBuddyAccounts,
  listWorkBuddyCheckinAccounts,
  loadWorkBuddySessionHealth,
  refreshWorkBuddyAccount,
  switchWorkBuddyAccount,
  type WorkBuddyAutoCheckinStatusDto,
} from "../platform/workbuddyAdapter";
import {
  CheckinSlotBadge,
  WorkBuddyPoolSlotBadge,
  deriveCheckinSlotState,
  deriveWorkBuddyPoolLoginState,
} from "./StatusBadges";
import { ConfirmDialog } from "./shared/ConfirmDialog";
import { AccountPageSkeleton } from "./shared/account/AccountPageSkeleton";
import { WorkBuddyAccountDetail } from "./WorkBuddyAccountDetail";
import { ACCOUNT_SORT_OPTIONS, type AccountSort } from "./shared/account/AccountListControls";
import { SharedAccountEntry, type AccountView, type SharedAccountRecord } from "./shared/account/AccountRow";
import { accountPreferenceKey, readStoredPreference, writeStoredPreference } from "./shared/account/preferences";
import { CheckinPageSkeleton } from "./shared/checkin/CheckinPageSkeleton";
import { CheckinActionsRow } from "./shared/checkin/CheckinActionsRow";
import { CheckinFlowRow } from "./shared/checkin/CheckinFlowRow";
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

/** B4 登录错误码 → 人话（未知码回落 safeUiError 兜底）。 */
function workbuddyLoginErrorText(reason: unknown): string {
  if (typeof reason === "string") {
    switch (reason) {
      case "workbuddy_login_already_in_progress": return "WorkBuddy：已有登录正在进行（同一时间仅允许一个），请稍候再试。";
      case "workbuddy_login_poll_timeout": return "WorkBuddy：等待授权超时（5 分钟），请重新发起登录。";
      case "workbuddy_login_auth_url_untrusted": return "WorkBuddy：授权地址校验未通过，已中止（请检查网络环境）。";
      case "workbuddy_real_mode_required": return "WorkBuddy：当前为演示模式，登录不可用。";
      default: break;
    }
  }
  return `WorkBuddy：登录未完成——${safeUiErrorMessage(reason, "请重新发起登录。")}`;
}

/** 信息行：label + value 的两列展示（卡内统一行结构；WB 域各页共用）。 */
export function InfoRow({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div className="workbuddy-row">
      <span className="workbuddy-row__label">{label}</span>
      <span className="workbuddy-row__value">{children}</span>
    </div>
  );
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
  // P19.1 详情实装：逐账号签到/积分数据面（overview 批查，fail-soft 落空数组）。
  // （P18-B3 的单账号 workbuddy_checkin_status 读取已退役：账号卡行积分摘要
  //  与详情页统一走 get_workbuddy_checkin_overview 逐账号数据面。）
  const [checkinOverview, setCheckinOverview] = useState<readonly WorkBuddyCheckinAccountStatusDto[]>([]);
  // P18-B3：深度健康检测（文件面 + 实时面；按钮单发，结果常驻健康卡）。
  const [checkingHealth, setCheckingHealth] = useState(false);
  // 捕获：单次确认弹窗 → 执行 → 刷新健康与账号池。
  const [captureDialogOpen, setCaptureDialogOpen] = useState(false);
  const [capturing, setCapturing] = useState(false);
  // 凭据续期（与 Trae「刷新登录凭据」同语义）：access 充裕跳过不轮换。
  const [renewing, setRenewing] = useState(false);
  // P17-T6/A4 切号：单次确认弹窗（目标 profile_id）→ 执行 → 刷新健康与池。
  const [switchTarget, setSwitchTarget] = useState<string | null>(null);
  const [switching, setSwitching] = useState(false);
  // P19-B4 添加账号（OAuth 浏览器登录编排）：attempt 非空 = 等待授权中
  //（与 TRAE 添加账号同形态：无弹窗等待，按钮态翻转 + 通知收尾）。
  const [loginAttemptId, setLoginAttemptId] = useState<number | null>(null);
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
      // 逐账号积分/签到面（P19.1）：账号卡行积分摘要与详情页共用，fail-soft。
      fetchWorkBuddyCheckinOverview()
        .then((rows) => setCheckinOverview(rows))
        .catch(() => setCheckinOverview([]));
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
      if (result.live_status === "verified") {
        notify.success(`WorkBuddy：健康检测通过——${liveHealthLabel(result.live_status)}`);
      } else if (result.live_status === "probe_failed") {
        notify.error(`WorkBuddy：健康检测发现异常——${liveHealthLabel(result.live_status)}`);
      } else {
        notify.info(`WorkBuddy：${liveHealthLabel(result.live_status)}`);
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

  // P17-T6/A4 切号执行：单次确认 → switch 命令（切号成功即自动过户，无感切换）→ 刷新。
  const runSwitch = useCallback(async () => {
    if (!switchTarget) return;
    setSwitching(true);
    try {
      const saved = await switchWorkBuddyAccount(switchTarget);
      notify.success(`WorkBuddy：已切换登录到「${saved.screen_name || "所选账号"}」`);
      setSwitchTarget(null);
      await refresh(false);
    } catch (reason: unknown) {
      notify.error(
        `WorkBuddy：切换未完成——${safeUiErrorMessage(reason, "客户端登录态未被改动，请稍后重试。")}`,
      );
    } finally {
      setSwitching(false);
    }
  }, [refresh, switchTarget]);

  // P19-B4 添加账号：begin（自动拉起浏览器）→ complete（阻塞等待授权）→
  // 成功入池刷新。acceptance 语义与 TRAE 一致：failed 不回滚、如实警告、
  // 不自动重试也不发起第二次授权。
  const runAddAccount = useCallback(async () => {
    if (loginAttemptId !== null) return;
    try {
      const begin = await beginWorkBuddyLogin();
      setLoginAttemptId(begin.attempt_id);
      const receipt = await completeWorkBuddyLogin(begin.attempt_id);
      notify.success(`WorkBuddy：账号「${receipt.screen_name}」登录成功，已保存到账号池。`);
      await refresh(false);
      if (receipt.acceptance === "failed") {
        notify.warning(
          `账号已保存，但可持续性验收未通过（${receipt.acceptance_error_code ?? "未知原因"}）。`,
          "可稍后重新发起登录获取新凭据。",
        );
      }
    } catch (reason: unknown) {
      // 取消不是错误：以中性提示收尾（complete 会以 workbuddy_login_cancelled 返回）。
      if (typeof reason === "string" && reason === "workbuddy_login_cancelled") {
        notify.info("登录已取消；若浏览器未完成授权，可重新发起登录。");
      } else {
        notify.error(workbuddyLoginErrorText(reason));
      }
    } finally {
      setLoginAttemptId(null);
    }
  }, [loginAttemptId, refresh]);

  // 取消等待：cancel 令阻塞中的 complete 立即以 workbuddy_login_cancelled 返回。
  const handleCancelLogin = useCallback(() => {
    if (loginAttemptId === null) return;
    void cancelWorkBuddyLogin(loginAttemptId).catch(() => undefined);
  }, [loginAttemptId]);

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
  const renderPoolRow = (record: SharedAccountRecord, ops: { onOpen(): void }) => {
    const account = pool.find((item) => item.profile_id === record.id);
    if (!account) return null;
    const poolState = deriveWorkBuddyPoolLoginState({
      access_expires_at_unix_seconds: account.access_token_expires_at_unix_seconds,
      refresh_expires_at_unix_seconds: account.refresh_token_expires_at_unix_seconds,
      now_unix_seconds: Math.floor(Date.now() / 1000),
    });
    // creditsColumn 能力开关（IA 草案 §1.2）：逐账号积分摘要（overview 数据面，
    // P19.1 由「仅当前登录账号」扩展为全部池行）；读取失败/无数据 = 未读取。
    const overviewRow = checkinOverview.find((row) => row.profile_id === account.profile_id) ?? null;
    const creditsLabel = overviewRow?.error
      ? "状态未知"
      : overviewRow?.total_credits != null
        ? `${overviewRow.total_credits}`
        : "未读取";
    const creditsMetric = uiProduct.capabilities.creditsColumn ? (
      <div className="account-item__credits" title="WorkBuddy 账号的积分余额（签到数据面，逐账号）">
        <strong data-testid={`workbuddy-credits-${account.account_id}`}>{creditsLabel}</strong>
        <span className="account-item__credits-label">积分余额</span>
      </div>
    ) : null;
    const cardCredits = uiProduct.capabilities.creditsColumn ? (
      <div className="account-card__credits" title="WorkBuddy 账号的积分余额（签到数据面，逐账号）">
        <span className="account-card__credits-value" data-testid={`workbuddy-credits-${account.account_id}`}>
          {creditsLabel}
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
        onOpen={ops.onOpen}
        onAction={() => setSwitchTarget(account.profile_id)}
        actionTitle="让 WorkBuddy 客户端切换登录到此账号（切号成功后历史会话归属自动随行，无感切换）"
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
            onClick={() => setCaptureDialogOpen(true)}
            disabled={captureDisabled || capturing || loading || loginAttemptId !== null}
            data-testid="workbuddy-capture-button"
            title={captureDisabled ? "WorkBuddy 客户端当前未登录，请先在客户端中登录" : "把客户端当前登录的账号保存到账号池"}
          >
            <ShieldCheck size={15} aria-hidden="true" />
            保存当前登录账号
          </button>
          {uiProduct.capabilities.addAccount === "oauth-browser" && (
            <button
              type="button"
              className="btn btn--primary"
              onClick={() => void runAddAccount()}
              disabled={loginAttemptId !== null || capturing || loading}
              data-testid="workbuddy-add-account-button"
              title="打开浏览器完成 WorkBuddy 授权登录，账号自动保存到账号池"
            >
              <UserPlus size={15} aria-hidden="true" />
              {loginAttemptId !== null ? "等待浏览器登录完成…" : "添加账号"}
            </button>
          )}
          {loginAttemptId !== null && (
            <button
              type="button"
              className="btn btn--quiet"
              onClick={handleCancelLogin}
              data-testid="workbuddy-login-cancel"
            >
              取消等待
            </button>
          )}
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
      statusRegion={null}
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
      renderRow={renderPoolRow}
      emptyState={{
        className: "workbuddy-page__empty",
        description: "还没有保存 WorkBuddy 账号。点击右上角「添加账号」通过浏览器登录，或在客户端登录后点「保存当前登录账号」。",
      }}
      // P19.1 详情实装：账号详情视图（凭据/积分/参与开关 + 诚实缺席标注）。
      detailView={(profileId, ops) => {
        const account = pool.find((item) => item.profile_id === profileId);
        if (!account) return null;
        return (
          <WorkBuddyAccountDetail
            account={account}
            isCurrentLogin={health?.uid === account.account_id}
            onDataChanged={async () => {
              await refresh(false);
            }}
            onBack={ops.onBack}
          />
        );
      }}
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
                "历史会话记录在本机并按账号隔离；切号成功后历史会话归属自动随行，无需手动转移。",
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
/**
 * P19-B5 后端契约消费（be-engineer edbd20b/9609f9a）：
 * - get_workbuddy_checkin_overview：逐账号状态批查（后端 fail-soft，error 落条目）
 * - workbuddy_checkin_claim_profile / workbuddy_checkin_claim_batch：单签/批量
 *   （幂等 AlreadyCheckedIn → credit=0 verified=true；批量不可中断，无取消）
 * - opt-out 后端化：auto_checkin_store touched 权威，enabled = 有效参与
 *   （未显式设置默认 true，八域对齐 #3）——B2 的前端 touched 集与归一化退役。
 */

/** 近 N 天日期串（YYYY-MM-DD 本地时区；升序，旧→今）。 */
function lastDayKeys(n: number): string[] {
  const out: string[] = [];
  for (let i = n - 1; i >= 0; i -= 1) {
    const day = new Date();
    day.setDate(day.getDate() - i);
    out.push(`${day.getFullYear()}-${String(day.getMonth() + 1).padStart(2, "0")}-${String(day.getDate()).padStart(2, "0")}`);
  }
  return out;
}

/** 签到历史条（IA 草案 §1.3 签到日历的近 N 天形态）：每账号一行近 14 天点阵。 */
function WbCheckinHistoryStrip({ rows }: {
  rows: readonly WorkBuddyCheckinAccountStatusDto[];
}) {
  const days = lastDayKeys(14);
  return (
    <div className="workbuddy-card" data-testid="workbuddy-checkin-history">
      <div className="workbuddy-card__title">签到历史（近 14 天）</div>
      {rows.map((row) => {
        const checked = new Set(row.checkin_dates);
        return (
          <div className="account-list__row" key={row.profile_id} data-testid={`workbuddy-history-${row.profile_id}`}>
            <div className="account-item__main">
              <span className="account-card__avatar" aria-hidden="true">
                {(row.screen_name || "W").trim().charAt(0).toUpperCase()}
              </span>
              <div className="account-item__id">
                <div className="account-item__name-row">
                  <strong className="account-item__name">{row.screen_name || "WorkBuddy 账号"}</strong>
                </div>
              </div>
            </div>
            <div className="account-item__side">
              <span
                className="account-item__meta"
                role="img"
                aria-label={`近 14 天签到 ${days.filter((day) => checked.has(day)).length} 天（实心=已签）`}
              >
                {row.error
                  ? "状态未知"
                  : days.map((day) => (checked.has(day) ? "●" : "○")).join("")}
              </span>
            </div>
          </div>
        );
      })}
    </div>
  );
}

export function WorkBuddyCheckinPage({ active }: WorkBuddyCheckinPageProps) {
  // 逐账号签到状态（后端聚合批查，fail-soft 落条目 error）。
  const [rows, setRows] = useState<readonly WorkBuddyCheckinAccountStatusDto[]>([]);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  // 批量选择（默认 = 有效参与集，IA 草案 §1.3；用户可增删，仅作用本次批次）。
  const [selection, setSelection] = useState<readonly string[]>([]);
  const [busy, setBusy] = useState(false);
  // 逐行 claim 结果（outcome 与 CheckinResultDto 同口径，进 OutcomeLabels 通用档）。
  const [rowResults, setRowResults] = useState<Record<string, { outcome: string; detail_code: string | null }>>({});
  // 参与集已加载标记：默认选择用参与集；未加载（命令失败）回退全选。
  const participationLoadedRef = useRef(false);
  // P17-A5 每日自动签到：设置（status 命令）+ 参与集行（list 命令，有效参与）。
  const [autoStatus, setAutoStatus] = useState<WorkBuddyAutoCheckinStatusDto | null>(null);
  const [participation, setParticipation] = useState<readonly WorkBuddyCheckinParticipationDto[]>([]);

  const refresh = useCallback(async (initial: boolean) => {
    if (initial) setLoading(true);
    try {
      const next = await fetchWorkBuddyCheckinOverview(null);
      setRows(next);
      setLoadError(null);
    } catch (reason: unknown) {
      setLoadError(safeUiErrorMessage(reason, "签到状态暂时不可读取，请稍后重试。"));
    } finally {
      setLoading(false);
    }
  }, []);

  const refreshAuto = useCallback(async () => {
    const [statusResult, participationResult] = await Promise.allSettled([
      fetchWorkBuddyAutoCheckinStatus(),
      listWorkBuddyCheckinAccounts(),
    ]);
    setAutoStatus(statusResult.status === "fulfilled" ? statusResult.value : null);
    if (participationResult.status === "fulfilled") {
      participationLoadedRef.current = true;
      setParticipation(participationResult.value);
    } else {
      setParticipation([]);
    }
  }, []);

  useEffect(() => {
    if (!active) return;
    void refresh(true);
    void refreshAuto();
  }, [active, refresh, refreshAuto]);

  // 批量选择默认 = 有效参与集（list 命令）；行集变化时剔除失效项并保留既有
  // 选择；参与集未加载（命令失败）回退全选，加载但为空（全部显式关闭）保持空选。
  useEffect(() => {
    setSelection((current) => {
      const filtered = current.filter((id) => rows.some((row) => row.profile_id === id));
      if (filtered.length > 0) return filtered;
      if (participationLoadedRef.current) {
        return participation.filter((account) => account.enabled).map((account) => account.profile_id);
      }
      return rows.map((row) => row.profile_id);
    });
  }, [rows, participation]);

  const toggle = useCallback((profileId: string) => {
    setSelection((current) => current.includes(profileId)
      ? current.filter((id) => id !== profileId)
      : [...current, profileId]);
  }, []);

  // 行内单签（矩阵 #56）：幂等领取 → 行内结果 + 该行状态回读。
  const handleClaimOne = useCallback(async (profileId: string) => {
    if (busy) return;
    setBusy(true);
    try {
      const claim = await claimWorkBuddyCheckinProfile(profileId);
      setRowResults((current) => ({
        ...current,
        [profileId]: { outcome: claim.credit > 0 ? "claimed" : "already_checked_in", detail_code: null },
      }));
      const refreshed = await fetchWorkBuddyCheckinOverview(profileId).catch(() => null);
      const row = refreshed?.find((item) => item.profile_id === profileId);
      if (row) setRows((current) => current.map((item) => (item.profile_id === profileId ? row : item)));
    } catch (reason: unknown) {
      setRowResults((current) => ({ ...current, [profileId]: { outcome: "runtime_error", detail_code: null } }));
      notify.error(`WorkBuddy：签到未完成——${safeUiErrorMessage(reason, "请稍后重试。")}`);
    } finally {
      setBusy(false);
    }
  }, [busy]);

  // 批量签到（矩阵 #51）：后端顺序执行逐条回执（fail-soft），前端不提供取消。
  const handleRunIds = useCallback(async (ids: readonly string[]) => {
    if (busy || ids.length === 0) return;
    setBusy(true);
    setRowResults({});
    try {
      const items = await claimWorkBuddyCheckinBatch(ids);
      setRowResults(Object.fromEntries(
        items.map((item) => [item.profile_id, { outcome: item.outcome, detail_code: item.detail_code }]),
      ));
      const failed = items.filter((item) => item.outcome !== "claimed" && item.outcome !== "already_checked_in").length;
      if (failed === 0) notify.success("WorkBuddy：签到批次完成（已签账号幂等跳过，结果见各行）。");
      else notify.warning(`WorkBuddy：部分签到未完成（${failed} 个失败），详见各行结果与通知。`);
      await refresh(false);
    } catch (reason: unknown) {
      notify.error(`WorkBuddy：签到未完成——${safeUiErrorMessage(reason, "请稍后重试。")}`);
    } finally {
      setBusy(false);
    }
  }, [busy, refresh]);

  // P19-B5：WB 签到页 = 逐账号行批量形态（IA 草案 §1.3）。
  // 页面编排由 CheckinPageSkeleton 承担；调度设置卡走 content 后置插槽。
  const uiProduct = productUi("workbuddy");
  const pendingIds = rows
    .filter((row) => row.error === null && row.today_checked_in === false)
    .map((row) => row.profile_id);

  return (
    <CheckinPageSkeleton
      product={uiProduct}
      active={active}
      loading={loading}
      headerActions={!loadError ? (
        <div className="page-header__actions">
          <button
            type="button"
            className="btn"
            onClick={() => void refresh(false)}
            disabled={loading || busy}
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
      entryCount={rows.length}
      selectedCount={selection.length}
      actions={(
        <CheckinActionsRow
          variant="batch"
          enabled={rows.length > 0}
          busy={busy}
          poolCount={rows.length}
          pendingCount={pendingIds.length}
          selectionCount={selection.length}
          onRunAll={() => void handleRunIds(rows.map((row) => row.profile_id))}
          onRunPending={() => void handleRunIds(pendingIds)}
          onRunSelected={() => void handleRunIds(selection)}
        />
      )}
      rows={rows.map((row) => (
        <CheckinFlowRow
          key={row.profile_id}
          productId="workbuddy"
          profileId={row.profile_id}
          displayName={row.screen_name || "WorkBuddy 账号"}
          badge={row.error
            ? <span className="slot-badge slot-badge--idle" title={row.error}>状态未知</span>
            : <CheckinSlotBadge state={deriveCheckinSlotState({ checked_in: row.today_checked_in === true })} />}
          meta={row.error || row.total_credits === null
            ? ""
            : `额度 ${row.total_credits} 积分 · 连签 ${row.streak_days ?? 0} 天`}
          selected={selection.includes(row.profile_id)}
          onToggle={() => toggle(row.profile_id)}
          disabled={busy}
          phase="idle"
          result={rowResults[row.profile_id] ?? null}
          waitSecs={null}
          onInlineCheckin={() => void handleClaimOne(row.profile_id)}
          inlineEnabled={row.error === null && row.today_checked_in === false}
        />
      ))}
      listVariant="flow"
      footer={rows.length > 0 ? <WbCheckinHistoryStrip rows={rows} /> : undefined}
      content={null}
    />
  );
}
