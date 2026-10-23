/**
 * 环境页 builder 登记表（P4d-3/4）：产品 id → builder 的查表分发，零 if/else 产品链。
 * 未登记产品 fail-closed 到空视图（不猜能力、不套用任一产品的卡片语义）。
 */
import { ENV_TEXTS, type EnvironmentView } from "./config";
import { buildTraeEnvironmentView, type TraeEnvironmentSnapshot } from "./traeEnvironment";
import { buildWorkBuddyEnvironmentView, type WorkBuddyEnvironmentSnapshot } from "./workbuddyEnvironment";
/** 页面读取层产出的产品快照（判别字段 productId 与载荷字段同名同值）。 */
export type EnvironmentSnapshot =
  | { readonly productId: "trae_cn"; readonly trae: TraeEnvironmentSnapshot }
  | { readonly productId: "workbuddy"; readonly workbuddy: WorkBuddyEnvironmentSnapshot };
/** 按载荷键取快照里的产品数据（表内键与载荷键一一对应，故直接断言）。 */
const payloadOf = <T,>(snapshot: EnvironmentSnapshot, key: string): T => (snapshot as unknown as Record<string, unknown>)[key] as T;
/** 查表分发：键 = productId，值 = 该产品的 builder（新增产品只在此加一行）。 */
const BUILDERS: Record<string, (snapshot: EnvironmentSnapshot) => EnvironmentView> = {
  trae_cn: (snapshot) => buildTraeEnvironmentView(payloadOf<TraeEnvironmentSnapshot>(snapshot, "trae")),
  workbuddy: (snapshot) => buildWorkBuddyEnvironmentView(payloadOf<WorkBuddyEnvironmentSnapshot>(snapshot, "workbuddy")),
};
/** fail-closed 空视图：无卡、无体检、无辅助环境，文案不冒充任一产品。 */
const EMPTY_VIEW: EnvironmentView = {
  productId: "", header: { title: ENV_TEXTS.pageTitle, regionLabel: ENV_TEXTS.regionLabel, pageTitleAttr: "environment", badgeProductId: null, refresh: null },
  loadingNote: null, error: { title: ENV_TEXTS.pageTitle, description: ENV_TEXTS.loadFailure, retryLabel: ENV_TEXTS.refresh },
  master: null, masterHint: ENV_TEXTS.masterHintLoggedOut, checkup: null, capture: null, secondary: null,
  stats: null, backups: [], plugins: [],
  texts: { fallbackFailure: ENV_TEXTS.actionFailure, loadFailure: ENV_TEXTS.loadFailure, refreshFailedSuffix: ENV_TEXTS.refreshFailedSuffix },
};
/** 快照 → 视图模型：未知产品一律空视图（不抛错，页面按错误/空态渲染）。 */
export function environmentViewFor(snapshot: EnvironmentSnapshot): EnvironmentView {
  return BUILDERS[snapshot.productId]?.(snapshot) ?? EMPTY_VIEW;
}
