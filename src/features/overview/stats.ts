import type { OverviewSnapshot } from "../../platform/overview";
import type { SectionKey } from "../../platform/productRegistry";
import type { WorkspaceStateDto } from "../../types/workspace";
import { CURRENT_PRODUCT_ID } from "../../types/product";
import { traeView } from "./traeStats";
import { workbuddyView } from "./workbuddyStats";

/**
 * 总览页的产品差异（P4a）：**全部**收敛为 features/overview 下的声明式配置。
 *
 * 页面（OverviewPage.tsx）只渲染本文件导出的视图模型，不做任何产品判断——
 * 每产品"摘要条显示哪几个指标 / 唯一主行动是什么 / 何时出现待处理提示"
 * 都是登记表里的一行。新增产品 = 加一条登记 + 一个 builder，页面一行不改。
 */

/** 单个指标：数值优先，hint 是口径说明（进悬浮提示，不进主视野）。 */
export interface OverviewStat {
  readonly key: string;
  readonly label: string;
  /** 已格式化的数值文案（"—" = 未获取，禁止写假 0）。 */
  readonly value: string;
  /** 数值下方短注（≤12 字，如「1/2 账号已获取」）。 */
  readonly note?: string;
  readonly hint?: string;
  /** warn = 该指标当前读不到（需要处理，非错误）。 */
  readonly tone?: "warn";
  readonly testId?: string;
}

/** 指标分组：一组 = 一处既有选择器容器（title 有值即带区块标题）。 */
export interface OverviewStatGroup {
  readonly testId: string;
  readonly title?: string;
  /** 摘要条栅格样式钩子（复用既有页面样式：指标多时用自适应列）。 */
  readonly stripClass: "overview-page__stats" | "overview-checkin__stats";
  readonly stats: readonly OverviewStat[];
}

/** 导航动作；primary = 本屏唯一主行动。 */
export interface OverviewAction {
  readonly label: string;
  readonly page: SectionKey;
  readonly testId: string;
}

/** 当前登录证据（一行，不是段落）。 */
export interface OverviewEvidence {
  readonly accountName: string;
  readonly lastActive: string | null;
}

/** 空态图标语义键（页面层映射到具体图标；配置不 import 图标库）。 */
export type EmptyIconKey = "book" | "database";

/** 总览页视图模型：页面唯一渲染输入。 */
export interface OverviewView {
  readonly groups: readonly OverviewStatGroup[];
  readonly evidence: OverviewEvidence | null;
  /** 待处理提示（仅当有事；最多一条）。 */
  readonly pending: OverviewAction | null;
  /** 空态：一句结论（≤25 字）+ 图标语义键 + 选择器锚点。 */
  readonly empty: {
    readonly title: string;
    readonly icon: EmptyIconKey;
    readonly testId: string;
  } | null;
  readonly primary: OverviewAction | null;
  readonly secondary: OverviewAction | null;
  /** 读不到数据时的刷新入口（无则 null）。 */
  readonly refresh: { readonly label: string; readonly testId: string } | null;
}

/** 空视图：数据未到位 / 产品数据面不可读时的中性形态。 */
export const EMPTY_VIEW: OverviewView = {
  groups: [],
  evidence: null,
  pending: null,
  empty: null,
  primary: null,
  secondary: null,
  refresh: null,
};

/** 管理账号入口（两个产品同文案；选择器锚点由各 builder 决定）。 */
export const ACCOUNTS_ACTION: OverviewAction = {
  label: "管理账号",
  page: "accounts",
  testId: "overview-accounts-cta",
};

// ===== 文案工具（两个产品共用同一口径） =====

/** 最近活跃：相对时间短文案（超过 30 天回落本地日期）。 */
export function formatLastActive(unixSeconds: number): string {
  const elapsed = Math.max(0, Date.now() / 1000 - unixSeconds);
  if (elapsed < 60) return "刚刚";
  if (elapsed < 3600) return `${Math.floor(elapsed / 60)} 分钟前`;
  if (elapsed < 86400) return `${Math.floor(elapsed / 3600)} 小时前`;
  if (elapsed < 86400 * 30) return `${Math.floor(elapsed / 86400)} 天前`;
  return new Date(unixSeconds * 1000).toLocaleDateString();
}

/** 积分合计：保留 1 位小数（与账号页同口径）。 */
export function formatCredits(value: number): string {
  const rounded = Math.round(value * 10) / 10;
  return Number.isInteger(rounded) ? String(rounded) : rounded.toFixed(1);
}

// ===== 产品登记表（声明式：键是产品协议值，值是视图 builder） =====

type ViewBuilder = (
  snapshot: OverviewSnapshot,
  state: WorkspaceStateDto | undefined,
) => OverviewView;

const VIEW_BUILDERS: Record<string, ViewBuilder> = {
  [CURRENT_PRODUCT_ID]: traeView,
  workbuddy: (snapshot) => workbuddyView(snapshot),
};

const DEFAULT_BUILDER: ViewBuilder = VIEW_BUILDERS[CURRENT_PRODUCT_ID];

/** 按产品解析总览视图模型（页面唯一的产品差异入口）。 */
export function overviewViewFor(
  productId: string,
  snapshot: OverviewSnapshot,
  state: WorkspaceStateDto | undefined,
): OverviewView {
  return (VIEW_BUILDERS[productId] ?? DEFAULT_BUILDER)(snapshot, state);
}