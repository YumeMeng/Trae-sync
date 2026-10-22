/**
 * TRAE 账号页 builder（P4c-3）：快照 → AccountsView 的纯映射（零 React / 零 JSX / 零命令）。
 * 时钟与偏好档位由快照注入；徽章一律经 StatusBadges 的 derive* 求态；能力只读 accountCapabilitiesFor("trae_cn")。
 */
import { deriveCheckinSlotState, deriveLoginSlotState } from "../../components/StatusBadges";
import type { AccountSnapshot } from "../../platform/accountAdapter";
import type { AccountProfileDto, CheckinCapabilityDto, CheckinOverviewEntryDto, TraeInstanceStateDto } from "../../types/account_switch";
import type { TraeCnAccountDto } from "../../types/product";
import { displayMobile, effectiveDisplayName } from "../../utils/accountDisplay";
import { ACCOUNT_TEXTS, accountCapabilitiesFor, type AccountActionView, type AccountBadgeView, type AccountMetricView, type AccountRowView, type AccountsEmptyView, type AccountsFiltersView, type AccountsOptionView, type AccountsView, type AccountTone } from "./config";

/** 行数据面（旧 L931-933 优先级）：checkin=真实总览 / pool=工具账号池 / saved=fixture 回退。 */
export type TraeRowFace = "checkin" | "pool" | "saved";
/** 忙碌态：builder 只用来算 disabled；按钮忙碌文案覆盖由页面负责。 */
export interface TraeAccountsBusy {
  readonly login: boolean; readonly credits: boolean; readonly credentialRefresh: boolean; readonly health: boolean;
  readonly localRefresh: boolean; readonly switching: boolean; readonly toolSelection: boolean;
}

/** 聚合页面需要的所有已取回数据：builder 只消费，不发起任何读取。 */
export interface TraeAccountsSnapshot {
  readonly capability: CheckinCapabilityDto | null; // get_checkin_capability；null = 未读到（真实模式按关闭）
  readonly overview: readonly CheckinOverviewEntryDto[]; // get_checkin_overview（真实模式行数据）
  readonly loginStates: Readonly<Record<string, TraeInstanceStateDto>>; // get_trae_instance_states 按 profile_id
  readonly observedProfileId: string | null; // get_environment_state.observed_profile_id（null = 未确认）
  readonly traePool: readonly TraeCnAccountDto[] | null; // list_trae_cn_accounts；null = 未启用
  readonly poolStatusLabel: string | null; // adapter.loadSnapshot().statusLabel（池面凭据状态）；null = 不显示
  readonly savedAccounts: readonly AccountProfileDto[]; // get_managed_account_state.saved_accounts（fixture 面）
  readonly savedCurrentProfileId: string | null; // 同上 current_account.profile_id（fixture 面「使用中」）
  readonly filter: "all" | "attention"; // G14 会话内过滤档
  readonly sort: "added" | "name" | "checkin"; // U-2 排序档（偏好持久化在页面）
  readonly busy: TraeAccountsBusy;
  readonly nowUnixSeconds: number; // 纯函数不读时钟：判定时刻由调用方注入
}
/** 行 testid 锚点：逐字抄旧实现（AccountCard L1372-1374 / AdapterAccountCard L1244-1246 / saved L984-986）。 */
export function traeRowTestIds(face: TraeRowFace, id: string): { readonly row: string; readonly action: string; readonly selected: string; readonly usage: string } {
  if (face === "pool") return { row: `account-adapter-card-${id}`, action: "account-adapter-tool-select", selected: `account-adapter-current-${id}`, usage: "" };
  if (face === "saved") return { row: `account-saved-card-${id}`, action: `account-saved-switch-${id}`, selected: `account-saved-current-${id}`, usage: "" };
  return { row: `account-card-${id}`, action: `account-switch-${id}`, selected: `account-current-${id}`, usage: `account-usage-${id}` };
}
/** 适配器面 → 池面两字段（仅测试注入路径）：池内账号与状态文案同源；未注入即恒 null（不猜数据）。 */
export function traePoolFields(adapter: AccountSnapshot | undefined): Pick<TraeAccountsSnapshot, "traePool" | "poolStatusLabel"> {
  if (adapter === undefined) return { traePool: null, poolStatusLabel: null };
  return {
    poolStatusLabel: adapter.statusLabel,
    traePool: adapter.accounts.map((account): TraeCnAccountDto => ({
      profile_id: account.id, display_name: account.displayName, avatar_url: "",
      last_verified_at: account.lastVerifiedAt, status: account.status, is_current: account.isToolSelected,
    })),
  };
}

/** 数据面判定（旧 L959-1016）：适配器面 > 真实模式 > fixture 回退。 */
export function traeRowFace(snapshot: TraeAccountsSnapshot): TraeRowFace {
  return snapshot.traePool !== null ? "pool" : snapshot.capability?.real_http_enabled === true ? "checkin" : "saved";
}
// 槽位枚举态 → 词与色档（词逐字抄 StatusBadges：绿=确认正常取 neutral，琥珀=warn，红=danger）。
const SLOTS: Record<string, readonly [string, AccountTone]> = {
  checked_in: ["已签", "neutral"], unchecked: ["未签", "neutral"], failed: ["签到失败", "danger"], retry_pending: ["待重试", "warn"], not_eligible: ["不可领取", "neutral"],
  not_refreshed: ["未刷新", "neutral"], ok: ["正常", "neutral"], expired: ["已过期", "warn"], relogin: ["需重登", "danger"], pending: ["待登录", "warn"], signed_out: ["未登录", "neutral"],
};
const slotBadge = (state: string): AccountBadgeView => ({ text: SLOTS[state][0], tone: SLOTS[state][1] });
const CURRENT: AccountBadgeView = { text: "使用中", tone: "accent" };
/** 工具账号选择 ≠ 官方切号：文案逐字抄旧 AdapterAccountCard L1254-1258 / AdapterAccountDetail L107。 */
export const TRAE_TOOL_SELECTION_COPY = { label: "设为工具当前账号", selectedLabel: "工具内已选", busyLabel: "设置中…",
  hint: "仅更新 Trae Sync 中的账号选择，不会切换官方客户端登录账号。" } as const;
const TOOL_SELECTED: AccountBadgeView = { text: TRAE_TOOL_SELECTION_COPY.selectedLabel, tone: "accent" };
const PENDING: AccountBadgeView = { text: "状态待验证", tone: "neutral" };
// 排序档键与文案逐字抄 shared/account/AccountListControls（本文件不 import UI 组件）。
const SORT_KEYS = ["added", "name", "checkin"] as const;
const SORT_LABELS = { added: "添加顺序", name: "名称", checkin: "签到状态" } as const;
// 已确认只能重新登录恢复的错误码（旧 L1397-1409）。
const RELOGIN_CODES = new Set(["credential_refresh_failed", "binding_mismatch", "auth_mismatch", "credential_missing", "credential_unavailable", "credential_invalid", "manual_recovery_required"]);
const reloginOnly = (entry: CheckinOverviewEntryDto): boolean => entry.credential_legacy || RELOGIN_CODES.has(entry.refresh_error_code ?? "");
const accessExpired = (entry: CheckinOverviewEntryDto, now: number): boolean => entry.access_token_expires_at_unix_seconds != null && entry.access_token_expires_at_unix_seconds <= now;
/** 徽章生效登录态 = 实调结果 × 两层本地修正（旧 L1431-1438）：旧通道 / access 过期按 stale。 */
const effectiveLogin = (entry: CheckinOverviewEntryDto, state: TraeInstanceStateDto | undefined, now: number) =>
  (reloginOnly(entry) || accessExpired(entry, now) ? "stale" : state?.login_state);
/** 登录槽枚举态（旧 L424-427 / L1292 同一调用口径）。 */
const loginSlot = (entry: CheckinOverviewEntryDto, snapshot: TraeAccountsSnapshot) =>
  deriveLoginSlotState({ login_state: effectiveLogin(entry, snapshot.loginStates[entry.profile_id], snapshot.nowUnixSeconds), relogin_only: reloginOnly(entry) });
/** G14 需处理（旧 L420-430）：凭据明确失效/过期，或登录槽红/琥珀，或签到槽红。 */
const needsAttention = (entry: CheckinOverviewEntryDto, snapshot: TraeAccountsSnapshot): boolean => {
  if (reloginOnly(entry) || accessExpired(entry, snapshot.nowUnixSeconds)) return true;
  const slot = loginSlot(entry, snapshot);
  return slot === "relogin" || slot === "expired" || slot === "pending" || deriveCheckinSlotState(entry, new Date(snapshot.nowUnixSeconds * 1000)) === "failed";
};
const byName = (a: string, b: string): number => a.localeCompare(b, "zh-Hans-CN");
/** 时间展示（旧 L1508-1513）：未验证 / 不可读都有中性回退文案。 */
const fmtDateTime = (value: string | null): string => {
  if (!value) return "尚未验证";
  const date = new Date(value);
  return Number.isNaN(date.getTime()) ? "时间不可用" : new Intl.DateTimeFormat("zh-CN", { year: "numeric", month: "2-digit", day: "2-digit", hour: "2-digit", minute: "2-digit", hour12: false }).format(date);
};
/** 额度数值（旧 L1503-1506）：保留 1 位小数，整数不带小数点。 */
const fmtCredits = (value: number): string => {
  const rounded = Math.round(value * 10) / 10;
  return Number.isInteger(rounded) ? String(rounded) : rounded.toFixed(1);
};
/** 列表侧栏凭据剩余时间（旧 L1516-1522）：不足一天仍显示 1 天。 */
const tokenRemaining = (expiresAt: number | null, now: number): string => expiresAt == null ? "登录凭据有效期未知" : expiresAt <= now ? "登录凭据已过期" : `登录凭据剩余 ${Math.ceil((expiresAt - now) / 86400)} 天`;
/** 真实模式行（旧 AccountCard L1263-1388）：签到槽在前、登录槽在后、使用中垫底；额度指标；切号动作。 */
function checkinRow(entry: CheckinOverviewEntryDto, snapshot: TraeAccountsSnapshot): AccountRowView {
  const now = snapshot.nowUnixSeconds, current = snapshot.observedProfileId === entry.profile_id;
  const credits: AccountMetricView = { label: "模型积分", value: entry.usage_remaining_credits != null ? fmtCredits(entry.usage_remaining_credits) : "未查询", tone: "neutral" };
  return {
    id: entry.profile_id, displayName: effectiveDisplayName(entry), archived: false,
    // 行「使用中」= 本次实测到的官方登录身份；页面据此给不可见锚点，不重复渲染徽章。
    current,
    subtitle: [`最近验证：${fmtDateTime(entry.last_verified_at)}`, displayMobile(entry), tokenRemaining(entry.access_token_expires_at_unix_seconds, now)].filter(Boolean).join(" · "),
    badges: [slotBadge(deriveCheckinSlotState(entry, new Date(now * 1000))), slotBadge(loginSlot(entry, snapshot)), ...(current ? [CURRENT] : [])],
    metrics: entry.refresh_error_code != null ? [credits, { label: "上次刷新", value: "刷新失败", tone: "warn" as const }] : [credits],
    // 当前账号不给切号按钮（旧实现渲染「使用中」chip 替代动作）。
    actions: current ? [] : [{ id: `account-switch-${entry.profile_id}`, label: "切换到此账号", kind: "primary", disabled: snapshot.busy.switching ? { reason: "切换正在进行中" } : undefined }],
  };
}
/** 工具账号池行（旧 AdapterAccountCard L1210-1261）：单枚凭据徽章 + 工具账号选择（≠ 官方切号）。 */
function poolRow(account: TraeCnAccountDto, snapshot: TraeAccountsSnapshot): AccountRowView {
  const badge: AccountBadgeView = account.status === "active" ? { text: "登录有效", tone: "neutral" }
    : account.status === "expired" ? { text: "登录已过期", tone: "danger" } : PENDING;
  return {
    id: account.profile_id, displayName: account.display_name, archived: false, metrics: [],
    // 工具账号选择 ≠ 官方切号：current 来自池内自有选择指针。
    current: account.is_current,
    subtitle: `最近验证：${fmtDateTime(account.last_verified_at)}`,
    badges: account.is_current ? [badge, TOOL_SELECTED] : [badge],
    actions: accountCapabilitiesFor("trae_cn").toolSelection && !account.is_current
      ? [{ id: "account-adapter-tool-select", label: TRAE_TOOL_SELECTION_COPY.label, kind: "primary", disabled: snapshot.busy.toolSelection ? { reason: "正在设置工具账号，请稍候" } : undefined }]
      : [],
  };
}
/** fixture 回退行（旧 L978-993）：只有「状态待验证」占位，无动作。 */
function savedRow(account: AccountProfileDto, snapshot: TraeAccountsSnapshot): AccountRowView {
  return {
    id: account.profile_id, displayName: account.display_name, archived: false, metrics: [], actions: [],
    current: snapshot.savedCurrentProfileId === account.profile_id,
    subtitle: `最近验证：${fmtDateTime(account.last_verified_at)}${account.region ? ` · ${account.region.toUpperCase()}` : ""}`,
    badges: snapshot.savedCurrentProfileId === account.profile_id ? [PENDING, CURRENT] : [PENDING],
  };
}
/** 页头（旧 L1054-1114）：本机加密徽章 + 池面凭据状态；刷新 / 健康检测 / 刷新额度 / 刷新凭据 / 添加账号（id 即旧 testid）。 */
function headerFor(snapshot: TraeAccountsSnapshot, face: TraeRowFace, real: boolean, hasEntries: boolean): AccountsView["header"] {
  const { busy } = snapshot;
  const badges: AccountBadgeView[] = [{ text: "登录信息仅本机加密保存", tone: "neutral" }, ...(face === "pool" && snapshot.poolStatusLabel ? [{ text: snapshot.poolStatusLabel, tone: "neutral" as const }] : [])];
  const wait = busy.health || busy.credits || busy.credentialRefresh || busy.login ? { reason: "批量操作正在进行中" } : undefined;
  const actions: AccountActionView[] = hasEntries ? [{ id: "account-refresh-local", label: ACCOUNT_TEXTS.refreshLocal, kind: "ghost", disabled: busy.localRefresh || busy.credentialRefresh ? { reason: "刷新正在进行中" } : undefined }] : [];
  if (hasEntries && (real || face === "pool")) actions.push({ id: "account-health-check", label: ACCOUNT_TEXTS.healthCheck, kind: "ghost", disabled: wait });
  if (hasEntries && real) actions.push({ id: "account-refresh-credits", label: ACCOUNT_TEXTS.refreshCredits, kind: "ghost", disabled: wait }, { id: "account-refresh-credentials", label: ACCOUNT_TEXTS.refreshCredentials, kind: "primary", disabled: wait });
  if (snapshot.capability?.transport !== "fixture") actions.push({ id: "account-add-primary", label: busy.login ? ACCOUNT_TEXTS.addAccountBusy : ACCOUNT_TEXTS.addAccount, kind: "primary", disabled: wait });
  return { title: ACCOUNT_TEXTS.pageTitle, badges, actions };
}
/** 控件行（旧 L1169-1178）：过滤档只在本产品有能力时出现；排序档按 checkinSort 裁剪。 */
function filtersFor(snapshot: TraeAccountsSnapshot, real: boolean, attentionCount: number): AccountsFiltersView | null {
  const caps = accountCapabilitiesFor("trae_cn");
  // 过滤行与「签到状态」档都只在真实总览面出现（旧 L1173-1177：非真实面 filter=undefined 且裁掉 checkin 档）。
  const keys = caps.checkinSort && real ? SORT_KEYS : SORT_KEYS.filter((key) => key !== "checkin");
  const sortOptions = keys.map((key): AccountsOptionView => ({ key, label: SORT_LABELS[key], count: null }));
  const options: AccountsOptionView[] = caps.attentionFilter && real
    ? [{ key: "all", label: "全部", count: null }, { key: "attention", label: "需处理", count: attentionCount }] : [];
  return { options, active: snapshot.filter, sortOptions, sort: snapshot.sort };
}
/** 空态（旧 L1019-1042 逐字）：testId 沿用旧实现的空态类名（旧实现无 data-testid）。 */
function emptyFor(face: TraeRowFace, rowCount: number, totalCount: number, filter: string): AccountsEmptyView {
  if (face === "saved" || (totalCount > 0 && !(rowCount === 0 && filter === "attention"))) return { description: ACCOUNT_TEXTS.emptyTrae, actionLabel: null, testId: "account-center__empty" };
  return totalCount === 0
    ? { description: face === "checkin" ? ACCOUNT_TEXTS.emptyTraeCheckin : ACCOUNT_TEXTS.emptyTrae, actionLabel: ACCOUNT_TEXTS.addAccount, testId: "account-center__empty-actions" }
    : { description: ACCOUNT_TEXTS.emptyFiltered, actionLabel: null, testId: "account-center__empty" };
}
/** 快照 → 账号页视图模型（纯函数：同一快照恒得同一视图）。 */
export function buildTraeAccountsView(snapshot: TraeAccountsSnapshot): AccountsView {
  const face = traeRowFace(snapshot), real = face === "checkin", pool = snapshot.traePool ?? [];
  const total = real ? snapshot.overview.length : face === "pool" ? pool.length : snapshot.savedAccounts.length;
  const attention = real ? snapshot.overview.filter((entry) => needsAttention(entry, snapshot)).length : 0;
  // 排序：真实签到档未签在前（旧 L404-415）；池面/回退面只有名称档重排（旧 L925-930）。
  const sorted = snapshot.sort === "added" ? snapshot.overview : [...snapshot.overview].sort((a, b) => {
    const nameOf = (x: CheckinOverviewEntryDto, y: CheckinOverviewEntryDto) => byName(effectiveDisplayName(x), effectiveDisplayName(y));
    return snapshot.sort === "name" ? nameOf(a, b) : (Number(a.checked_in !== true) - Number(b.checked_in !== true)) || nameOf(a, b);
  });
  const visible = real && snapshot.filter === "attention" ? sorted.filter((entry) => needsAttention(entry, snapshot)) : sorted;
  const byNameOnly = <T,>(list: readonly T[], nameOf: (item: T) => string): readonly T[] => (snapshot.sort === "name" ? [...list].sort((a, b) => byName(nameOf(a), nameOf(b))) : list);
  const rows: readonly AccountRowView[] = real
    ? visible.map((entry) => checkinRow(entry, snapshot))
    : face === "pool" ? byNameOnly(pool, (account) => account.display_name).map((account) => poolRow(account, snapshot))
    : byNameOnly(snapshot.savedAccounts, (account) => account.display_name).map((account) => savedRow(account, snapshot));
  return {
    header: headerFor(snapshot, face, real, total > 0),
    // 旧页面无摘要指标行：计数走骨架 section-caption（旧 L1167），诚实留 null。
    stats: null,
    filters: filtersFor(snapshot, real, attention),
    rows,
    // 计数口径 = 账号总数（旧骨架 entryCount）：真实面=总览条数 / 池面=池内账号数 / 回退面=存档账号数。
    totalCount: total,
    // 批量动作全在页头（旧布局），列表层无独立动作行。
    listActions: [],
    empty: emptyFor(face, rows.length, total, snapshot.filter),
    // 详情分区（旧 AccountDetail 六区）归 P4c-7，本 builder 不臆造。
    detail: [],
  };
}
