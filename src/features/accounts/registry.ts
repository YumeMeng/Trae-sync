/**
 * 账号页 builder 登记表（P4c-4）：产品 id → builder 的查表分发，零 if/else 产品链。
 * 未登记产品 fail-closed 到空视图（不猜能力、不套用任一产品的行语义）。
 */
import { ACCOUNT_TEXTS, type AccountsView } from "./config";
import { buildTraeAccountsView, type TraeAccountsSnapshot } from "./traeAccounts";
import { buildWorkBuddyAccountsView, type WorkBuddyAccountsSnapshot } from "./workbuddyAccounts";

/** 页面读取层产出的产品快照（判别字段 productId 与载荷字段同名同值）。 */
export type AccountsSnapshot =
  | { productId: "trae_cn"; trae: TraeAccountsSnapshot }
  | { productId: "workbuddy"; workbuddy: WorkBuddyAccountsSnapshot };

/** 按载荷键取快照里的产品数据（表内键与载荷键一一对应，故直接断言）。 */
const payloadOf = <T,>(snapshot: AccountsSnapshot, key: string): T =>
  (snapshot as unknown as Record<string, unknown>)[key] as T;

/** 查表分发：键 = productId，值 = 该产品的 builder（新增产品只在此加一行）。 */
const BUILDERS: Record<string, (snapshot: AccountsSnapshot) => AccountsView> = {
  trae_cn: (snapshot) => buildTraeAccountsView(payloadOf<TraeAccountsSnapshot>(snapshot, "trae")),
  workbuddy: (snapshot) => buildWorkBuddyAccountsView(payloadOf<WorkBuddyAccountsSnapshot>(snapshot, "workbuddy")),
};

/** fail-closed 空视图：无行、无动作、无过滤，空态文案不冒充任一产品。 */
const EMPTY_VIEW: AccountsView = {
  header: { title: ACCOUNT_TEXTS.pageTitle, badges: [], actions: [] },
  stats: null, filters: null, rows: [], listActions: [], detail: [],
  empty: { description: "账号信息暂不可用，请稍后重试。", actionLabel: null, testId: "account-center__empty" },
};

/** 快照 → 视图模型：未知产品一律空视图（不抛错，页面按空态渲染）。 */
export function accountsViewFor(snapshot: AccountsSnapshot): AccountsView {
  return BUILDERS[snapshot.productId]?.(snapshot) ?? EMPTY_VIEW;
}
