/**
 * WorkBuddy 账号页 builder（P4c-4）：WB 快照 → AccountsView 的纯映射（零 React / 零 JSX / 零命令）。
 * 判定时刻与忙碌态由快照注入；徽章经 deriveWorkBuddyPoolLoginState 求态；能力只读 accountCapabilitiesFor("workbuddy")。
 */
import { deriveWorkBuddyPoolLoginState, type LoginSlotState } from "../../components/StatusBadges";
import type { WorkBuddyAccountEntryDto, WorkBuddyCheckinAccountStatusDto, WorkBuddySessionHealthDto } from "../../types/product";
import { workbuddyDisplayName } from "../../utils/accountDisplay";
import { ACCOUNT_TEXTS, accountCapabilitiesFor, type AccountActionView, type AccountBadgeView, type AccountMetricView, type AccountRowView, type AccountsEmptyView, type AccountsFiltersView, type AccountsOptionView, type AccountsView, type AccountTone } from "./config";

/** 忙碌态：builder 只用来算 disabled；按钮忙碌文案覆盖由页面负责。 */
export interface WorkBuddyAccountsBusy {
  readonly loading: boolean; // 首读
  readonly refreshing: boolean; // 页头「刷新」
  readonly renewing: boolean; // 「刷新登录凭据」
  readonly health: boolean; // 「健康检测」
  readonly capturing: boolean; // 「保存当前登录账号」
  readonly switching: boolean; // 切号执行中
  readonly preview: boolean; // 切号影响面预览中
  readonly login: boolean; // 等待浏览器授权中（loginAttemptId 非空）
}

/** 聚合页面需要的所有已取回数据：builder 只消费，不发起任何读取。 */
export interface WorkBuddyAccountsSnapshot {
  readonly sessionHealth: WorkBuddySessionHealthDto | null; // get_workbuddy_session_health；null = 未读到
  readonly pool: readonly WorkBuddyAccountEntryDto[]; // list_workbuddy_accounts（档案字段全量，归档不预过滤）
  readonly checkinOverview: readonly WorkBuddyCheckinAccountStatusDto[]; // get_workbuddy_checkin_overview（逐账号余额面）
  readonly sort: "added" | "name"; // U-2 排序档（WB 无签到档；偏好持久化在页面）
  readonly busy: WorkBuddyAccountsBusy;
  readonly nowUnixSeconds: number; // 纯函数不读时钟：凭据判定时刻由调用方注入
}

/** 行 testid 锚点：逐字抄旧实现（WorkBuddyPanel L421-423 / L396 / L430）。 */
export function workbuddyRowTestIds(profileId: string, accountId: string): {
  readonly row: string; readonly action: string; readonly selected: string; readonly credits: string; readonly archived: string;
} {
  return {
    row: `workbuddy-pool-card-${profileId}`,
    action: `workbuddy-switch-${profileId}`,
    selected: `workbuddy-pool-current-${profileId}`,
    credits: `workbuddy-credits-${accountId}`, // 旧实现按 account_id 而非 profile_id
    archived: `workbuddy-archived-${profileId}`,
  };
}

const CAPS = accountCapabilitiesFor("workbuddy");
// 凭据槽枚举态 → 词与色档（词逐字抄 WorkBuddyPoolSlotBadge：绿=正常取 neutral，琥珀=warn，红=danger）。
const POOL_SLOTS: Record<LoginSlotState, readonly [string, AccountTone]> = {
  ok: ["正常", "neutral"], expired: ["已过期", "warn"], relogin: ["需重登", "danger"], pending: ["待确认", "warn"], signed_out: ["未登录", "neutral"],
};
/** 归档标记（旧 L426-433）：与凭据槽并列的独立标记，不占槽位。 */
const ARCHIVED: AccountBadgeView = { text: "已归档", tone: "neutral" };
/** 当前客户端登录账号（旧 L356-357：health.uid 与池 account_id 同源）。 */
const CURRENT: AccountBadgeView = { text: "使用中", tone: "accent" };
// 排序档键与文案逐字抄 shared/account/AccountListControls（本文件不 import UI 组件）。
const SORT_KEYS = ["added", "name", "checkin"] as const;
const SORT_LABELS = { added: "添加顺序", name: "名称", checkin: "签到状态" } as const;

/** Unix 秒 → 本地「日期 时:分」；0 = 未知返回 null（旧 L53-62 同口径）。 */
const fmtUnixTime = (unixSeconds: number): string | null => unixSeconds
  ? new Date(unixSeconds * 1000).toLocaleString(undefined, { year: "numeric", month: "2-digit", day: "2-digit", hour: "2-digit", minute: "2-digit" })
  : null;

/** 逐账号余额数据面（旧 L372）。 */
const overviewOf = (snapshot: WorkBuddyAccountsSnapshot, profileId: string): WorkBuddyCheckinAccountStatusDto | null =>
  snapshot.checkinOverview.find((row) => row.profile_id === profileId) ?? null;

/**
 * 余额四态（旧 L372-379 逐字）：未获取 / 查询失败 / 余额查询失败 / 真实数值（0 照实显示）。
 * 旧实现四态同一文字色；此处把两个读取失败态提为琥珀（可重试的读取失败，不属凭据失效，故不用红）。
 */
function balanceMetric(row: WorkBuddyCheckinAccountStatusDto | null): AccountMetricView {
  if (row === null) return { label: "积分余额", value: "未获取", tone: "neutral" };
  if (row.error) return { label: "积分余额", value: "查询失败", tone: "warn" };
  if (row.credits_balance == null) return { label: "积分余额", value: "余额查询失败", tone: "warn" };
  return { label: "积分余额", value: String(row.credits_balance), tone: "neutral" };
}

/** meta 行（旧 L408-416）：有效期 / 更新于 / 本期累计 / 脱敏手机号，空段不进文案。 */
function metaFor(account: WorkBuddyAccountEntryDto, row: WorkBuddyCheckinAccountStatusDto | null): string {
  const accum = row && !row.error && row.total_credits != null ? ` · 本期签到累计 ${row.total_credits}` : "";
  const phone = account.masked_phone ? ` · ${account.masked_phone}` : "";
  return `登录有效期至 ${fmtUnixTime(account.access_token_expires_at_unix_seconds) ?? "未知"} · 更新于 ${fmtUnixTime(account.last_updated_at_unix_seconds) ?? "未知"}${accum}${phone}`;
}

/** 池行（旧 L351-448）：单槽凭据徽章 + 归档标记 + 余额指标 + 切号动作。 */
function poolRow(account: WorkBuddyAccountEntryDto, snapshot: WorkBuddyAccountsSnapshot): AccountRowView {
  const slot = POOL_SLOTS[deriveWorkBuddyPoolLoginState({
    access_expires_at_unix_seconds: account.access_token_expires_at_unix_seconds,
    refresh_expires_at_unix_seconds: account.refresh_token_expires_at_unix_seconds,
    now_unix_seconds: snapshot.nowUnixSeconds,
  })];
  const current = snapshot.sessionHealth !== null && snapshot.sessionHealth.uid === account.account_id;
  const overviewRow = overviewOf(snapshot, account.profile_id);
  return {
    id: account.profile_id, displayName: workbuddyDisplayName(account), archived: account.archived,
    subtitle: metaFor(account, overviewRow),
    badges: [...(CAPS.archiveTag && account.archived ? [ARCHIVED] : []), { text: slot[0], tone: slot[1] }, ...(current ? [CURRENT] : [])],
    metrics: CAPS.creditsColumn ? [balanceMetric(overviewRow)] : [],
    // 当前账号渲染「使用中」chip 替代动作（旧 SharedAccountEntry L89-104 同语义）。
    actions: current ? [] : [{
      id: `workbuddy-switch-${account.profile_id}`, label: "切换到此账号", kind: "primary",
      disabled: snapshot.busy.switching || snapshot.busy.preview ? { reason: "切换正在进行中" } : undefined,
    }],
  };
}

/** 页头（旧 L458-528）：刷新 / 刷新登录凭据 / 健康检测 / 保存当前登录账号 / 添加账号（等待中可取消）。 */
function headerFor(snapshot: WorkBuddyAccountsSnapshot, poolCount: number): AccountsView["header"] {
  const { busy } = snapshot;
  const wait = busy.loading || busy.capturing || busy.login;
  // 客户端未登录时捕获必然失败（旧 L321）：UI 提前禁用并给原因。
  const captureBlocked = snapshot.sessionHealth !== null && snapshot.sessionHealth.status !== "active";
  const actions: AccountActionView[] = [
    { id: "workbuddy-refresh", label: ACCOUNT_TEXTS.refreshLocal, kind: "ghost", disabled: busy.refreshing || busy.loading ? { reason: "正在读取账号信息" } : undefined },
    { id: "workbuddy-renew-credentials", label: "刷新登录凭据", kind: "ghost",
      disabled: busy.renewing || busy.loading || poolCount === 0 ? { reason: poolCount === 0 ? "还没有已保存的账号" : "刷新正在进行中" } : undefined },
    { id: "workbuddy-deep-health-button", label: ACCOUNT_TEXTS.healthCheck, kind: "ghost", disabled: busy.health || busy.loading ? { reason: "健康检测正在进行中" } : undefined },
    { id: "workbuddy-capture-button", label: "保存当前登录账号", kind: "ghost",
      disabled: captureBlocked ? { reason: "WorkBuddy 客户端当前未登录，请先在客户端中登录" } : wait ? { reason: "登录或保存正在进行中" } : undefined },
    { id: "workbuddy-add-account-button", label: busy.login ? ACCOUNT_TEXTS.addAccountBusy : ACCOUNT_TEXTS.addAccount, kind: "primary",
      disabled: busy.login || busy.capturing || busy.loading ? { reason: "登录或保存正在进行中" } : undefined },
  ];
  if (busy.login) actions.push({ id: "workbuddy-login-cancel", label: "取消等待", kind: "ghost" });
  // 旧页面页头无徽章（本机加密说明由 WB 健康卡承担，不臆造）：诚实留空。
  return { title: ACCOUNT_TEXTS.pageTitle, badges: [], actions };
}

/** 控件行（旧 L540-549）：WB 无过滤行（attentionFilter=false），排序档按 checkinSort 裁剪。 */
function filtersFor(snapshot: WorkBuddyAccountsSnapshot): AccountsFiltersView {
  const sortOptions: AccountsOptionView[] = SORT_KEYS
    .filter((key) => key !== "checkin" || CAPS.checkinSort)
    .map((key) => ({ key, label: SORT_LABELS[key], count: null }));
  return { options: CAPS.attentionFilter ? [{ key: "all", label: "全部", count: null }] : [], active: "all", sortOptions, sort: snapshot.sort };
}

/** 空态（旧 L553-556 逐字）：testId 沿用旧实现的空态类名（旧实现无 data-testid）。 */
const EMPTY: AccountsEmptyView = { description: ACCOUNT_TEXTS.emptyWorkbuddy, actionLabel: null, testId: "workbuddy-page__empty" };

/** 快照 → 账号页视图模型（纯函数：同一快照恒得同一视图）。 */
export function buildWorkBuddyAccountsView(snapshot: WorkBuddyAccountsSnapshot): AccountsView {
  // 排序：added=池原序（注册表顺序）；name=展示名本地化比较（旧 L347-350）。
  const sorted = snapshot.sort === "name"
    ? [...snapshot.pool].sort((a, b) => workbuddyDisplayName(a).localeCompare(workbuddyDisplayName(b), "zh-Hans-CN"))
    : snapshot.pool;
  return {
    header: headerFor(snapshot, snapshot.pool.length),
    // 旧页面无摘要指标行：计数走骨架 section-caption（旧 L539），诚实留 null。
    stats: null,
    filters: filtersFor(snapshot),
    rows: sorted.map((account) => poolRow(account, snapshot)),
    // 批量动作全在页头（旧布局），列表层无独立动作行。
    listActions: [],
    empty: EMPTY,
    // 详情分区（旧 WorkBuddyAccountDetail 六区）归 P4c-7，本 builder 不臆造。
    detail: [],
  };
}
