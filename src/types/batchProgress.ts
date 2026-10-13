import type { ProductId } from "../platform/productRegistry";

/**
 * 批量操作逐账号进度事件统一契约（P12-R2 Q2 定案）。
 *
 * 后端将把 credential-refresh-progress / health-check-progress /
 * credits-refresh-progress 三个事件（事件名保持不变）的载荷统一为
 * BatchProgressEvent：serde `tag = "kind"` 区分链路，公共字段 +
 * 各链特有字段（serde default，向后兼容）。
 *
 * 每条载荷都带产品来源；前端缺少来源或来源与当前账号页不符时丢弃。
 * kind 仍兼容 R2b 之前的载荷；存在时按事件名校验，不匹配则丢弃。
 */

/** 批量进度事件链类型（serde tag="kind" 的取值）。 */
export type BatchProgressKind = "credential_refresh" | "health_check" | "credits_refresh";

/** 批量操作逐账号进度事件载荷（三链统一，非敏感白名单）。 */
export interface BatchProgressEvent {
  /** 进度所属产品；账号页只接受当前产品的事件。 */
  product_id: ProductId;
  /** 事件链类型；R2b 落地前的过渡期载荷可能缺省（前端按事件名降级推断）。 */
  kind?: BatchProgressKind;
  profile_id: string;
  screen_name: string;
  /** 已处理账号数（含失败，1 起）。 */
  completed: number;
  total: number;
  error_code: string | null;
  // —— 各链特有字段（serde default，向后兼容）——
  /** credential_refresh 链：该账号凭据是否换发成功。 */
  refreshed?: boolean;
  /** health_check 链：该账号是否健康。 */
  healthy?: boolean;
  /** credits_refresh 链：该账号查询是否成功。 */
  ok?: boolean;
  /** credits_refresh 链：查询到的积分（无缓存/失败为 null）。 */
  credits?: number | null;
  /** credits_refresh 链：今日是否已签到（未知为 null）。 */
  checked_in?: boolean | null;
  /** credits_refresh 链：真实可用模型积分。 */
  usage_remaining_credits?: number | null;
}
