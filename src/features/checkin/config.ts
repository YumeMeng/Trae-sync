import type { CheckinSlotState } from "../../components/StatusBadges";
import type { CheckinSnapshot } from "../../platform/checkin";
import { CURRENT_PRODUCT_ID } from "../../types/product";
import { traeCheckinView } from "./traeCheckin";
import { workbuddyCheckinView } from "./workbuddyCheckin";

/**
 * 签到页的产品差异（P4b-2）：**全部**收敛为 features/checkin 下的声明式配置。
 *
 * 页面（CheckinPage.tsx）只渲染本文件导出的视图模型，不做任何产品判断——
 * 每产品「显示哪些状态行 / 哪些列 / 哪些按钮 / 状态文案怎么说」都是登记表里
 * 的一行。新增产品 = 加一条登记 + 一个 builder，页面一行不改。
 */

// ===== 视图模型 =====

/** 行内状态徽章：slot = 复用 G10 签到槽六态；text = 行级故障的诚实文案。 */
export type CheckinRowBadge =
  | { readonly kind: "slot"; readonly state: CheckinSlotState }
  | { readonly kind: "text"; readonly word: string; readonly hint: string };

/** 单个账号行：两产品数据面归一后的展示输入。 */
export interface CheckinRowView {
  readonly profileId: string;
  readonly displayName: string;
  /** null = 该产品此形态不显示徽章列。 */
  readonly badge: CheckinRowBadge | null;
  /** 行内元信息（额度/连签等）；空串 = 不显示。 */
  readonly meta: string;
  /** 今日是否已签（null = 状态未知）。 */
  readonly checkedIn: boolean | null;
  /** 行级错误（fail-soft 产品）：非空 = 该行状态不可信、不可单签。 */
  readonly error: string | null;
}

/** 列开关：哪些列显示（产品差异只体现在「有没有这个数据」）。 */
export interface CheckinColumns {
  readonly selection: boolean;
  readonly badge: boolean;
  readonly meta: boolean;
  readonly inlineAction: boolean;
}

/** 动作键：页面按此映射到执行通道（不是产品判断）。 */
export type CheckinActionKey = "run-all" | "run-pending" | "run-selected" | "cancel";

export interface CheckinActionView {
  readonly key: CheckinActionKey;
  readonly label: string;
  readonly tone: "primary" | "secondary" | "quiet";
  readonly enabled: boolean;
  readonly testId: string;
}

/** 状态行：一句话说清当前状态（长说明改状态，不堆段落）。 */
export interface CheckinStatusView {
  readonly testId: string;
  readonly text: string;
  /** warn = 需要处理（琥珀/红），info = 中性说明。 */
  readonly tone: "info" | "warn";
}

/** 通知文案（操作结果一律经 notify 发出；前缀取产品展示名）。 */
export interface CheckinTexts {
  readonly notificationPrefix: string;
  readonly runSuccess: string;
  readonly runPartial: string;
  readonly runFailure: string;
}

/** 今日自动批次台账（两产品字段同构）。 */
export interface CheckinLedgerView {
  readonly running: boolean;
  readonly total: number;
  readonly completed: number;
  readonly failed: number;
  readonly skipped: number;
}

/** 自动签到状态（两产品 DTO 的共同子集）。 */
export interface CheckinAutoStatusView {
  readonly enabled: boolean;
  readonly daily_time_hhmm: string;
  readonly ledger: CheckinLedgerView | null;
}

/** 签到页视图模型：页面唯一渲染输入。 */
export interface CheckinView {
  readonly productId: string;
  /** 页面标题（≤8 字）。 */
  readonly title: string;
  readonly statuses: readonly CheckinStatusView[];
  /** 读取失败态（失败才展开：一句结论 + 重试动作）。 */
  readonly error: {
    readonly description: string;
    readonly retryLabel: string;
    readonly testId: string;
  } | null;
  readonly rows: readonly CheckinRowView[];
  readonly columns: CheckinColumns;
  readonly listVariant: "flow" | "plain";
  readonly actions: readonly CheckinActionView[];
  /** 默认勾选集（真实=全量；参与制产品=有效参与集）。 */
  readonly defaultSelection: readonly string[];
  /** 今日未签集（补签按钮的数据口径）。 */
  readonly pendingIds: readonly string[];
  readonly empty: {
    readonly description: string;
    readonly actionLabel: string;
    readonly actionPage: "accounts";
    readonly testId: string;
  } | null;
  readonly refresh: {
    readonly label: string;
    readonly enabled: boolean;
    readonly hint: string;
    readonly testId: string;
  } | null;
  readonly texts: CheckinTexts;
}

/** 空视图：数据未到位时的中性形态（标题保留，主体不出）。 */
export const EMPTY_VIEW: CheckinView = {
  productId: CURRENT_PRODUCT_ID,
  title: "签到",
  statuses: [],
  error: null,
  rows: [],
  columns: { selection: true, badge: false, meta: false, inlineAction: false },
  listVariant: "flow",
  actions: [],
  defaultSelection: [],
  pendingIds: [],
  empty: null,
  refresh: null,
  texts: {
    notificationPrefix: "TRAE",
    runSuccess: "签到流程已完成。",
    runPartial: "部分签到未完成，请查看逐账号结果。",
    runFailure: "签到未完成，请稍后重试。",
  },
};

/** 页面渲染输入：勾选集与执行中由页面持有，视图由此求得（纯映射）。 */
export interface CheckinViewInput {
  readonly selection: readonly string[];
  readonly busy: boolean;
}

// ===== 状态文案映射（两产品同一套词，差异只在测试锚点） =====

/** 今日自动批次台账文案；与设置页同一口径。 */
function autoLedgerLabel(ledger: CheckinLedgerView | null): string {
  if (ledger === null) return "今日尚未发起";
  const parts = [`成功 ${ledger.completed}/${ledger.total}`];
  if (ledger.failed > 0) parts.push(`失败 ${ledger.failed}`);
  if (ledger.skipped > 0) parts.push(`跳过 ${ledger.skipped}`);
  return ledger.running ? `今日执行中（${parts.join("，")}）` : `今日已完成（${parts.join("，")}）`;
}

/** 自动签到状态行（关闭时只说关闭；开启时附今日台账）。 */
export function autoStatusLine(
  auto: CheckinAutoStatusView | null,
  testId: string,
): CheckinStatusView | null {
  if (auto === null) return null;
  const head = auto.enabled ? `每日 ${auto.daily_time_hhmm} 触发` : "已关闭";
  const tail = auto.enabled ? ` · ${autoLedgerLabel(auto.ledger)}` : "";
  return { testId, text: `自动签到：${head}${tail}`, tone: "info" };
}

/** 短日期（月-日 时:分）；无法解析给「时间不可用」，不给假时间。 */
export function formatShortDate(value: string): string {
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return "时间不可用";
  return new Intl.DateTimeFormat("zh-CN", {
    month: "2-digit",
    day: "2-digit",
    hour: "2-digit",
    minute: "2-digit",
    hour12: false,
  }).format(date);
}

/** 额度数值格式化：保留 1 位小数（与账号页同口径）。 */
export function formatCreditsValue(value: number): string {
  const rounded = Math.round(value * 10) / 10;
  return Number.isInteger(rounded) ? String(rounded) : rounded.toFixed(1);
}

// ===== 产品登记表（声明式：键是产品协议值，值是视图 builder） =====

type ViewBuilder = (
  snapshot: CheckinSnapshot,
  input: CheckinViewInput,
  productId: string,
) => CheckinView;

const VIEW_BUILDERS: Record<string, ViewBuilder> = {
  [CURRENT_PRODUCT_ID]: traeCheckinView,
  workbuddy: workbuddyCheckinView,
};

const DEFAULT_BUILDER: ViewBuilder = VIEW_BUILDERS[CURRENT_PRODUCT_ID];

/** 按产品解析签到视图模型（页面唯一的产品差异入口；快照未到位返回空视图）。 */
export function checkinViewFor(
  productId: string,
  snapshot: CheckinSnapshot | null,
  input: CheckinViewInput,
): CheckinView {
  if (snapshot === null) return { ...EMPTY_VIEW, productId };
  // 产品标识由登记表注入：builder 只描述差异，不自带第二份产品事实。
  return (VIEW_BUILDERS[productId] ?? DEFAULT_BUILDER)(snapshot, input, productId);
}
