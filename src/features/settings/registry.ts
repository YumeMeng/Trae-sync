/**
 * 设置页 builder 登记表（P4f-2）：产品 id → builder 的查表分发，零 if/else 产品链。
 * 未登记产品 fail-closed 到空视图（无签到区、无备份区、不套用任一产品的分区语义）。
 * 新增产品只在 SettingsSnapshot 联合加一支 + 在 BUILDERS 加一行，不写 === 分支。
 */
import { SETTINGS_TEXTS, type SettingsView } from "./config";
import { buildTraeSettingsView, type TraeSettingsSnapshot } from "./traeSettings";

/** 页面读取层产出的产品快照（判别字段 productId 与载荷字段同名同值）。 */
export type SettingsSnapshot = { productId: "trae_cn"; trae: TraeSettingsSnapshot };
/** 按载荷键取快照里的产品数据（表内键与载荷键一一对应，故直接断言）。 */
const payloadOf = <T,>(snapshot: SettingsSnapshot, key: string): T => (snapshot as unknown as Record<string, unknown>)[key] as T;
/** 查表分发：键 = productId，值 = 该产品的 builder（新增产品只在此加一行）。 */
const BUILDERS: Record<string, (snapshot: SettingsSnapshot) => SettingsView> = {
  trae_cn: (snapshot) => buildTraeSettingsView(payloadOf<TraeSettingsSnapshot>(snapshot, "trae")),
};
/** fail-closed 空视图：只有页头契约，无签到/备份分区、无动作；文案不冒充任一产品。 */
const EMPTY_VIEW: SettingsView = {
  productId: "",
  header: { eyebrow: SETTINGS_TEXTS.eyebrow, title: SETTINGS_TEXTS.pageTitle, regionLabel: SETTINGS_TEXTS.regionLabel, pageTitleAttr: "settings" },
  autoCheckin: null, backup: null,
  // 通知前缀留空：未登记产品没有展示名可借，不冒充任一产品（页面级兜底句仍用通用文案）。
  texts: { notificationPrefix: "", fallbackFailure: SETTINGS_TEXTS.fallbackFailure },
};
/** 快照 → 视图模型：未知产品一律空视图（不抛错，页面按空页头渲染）。 */
export function settingsViewFor(snapshot: SettingsSnapshot): SettingsView {
  return BUILDERS[snapshot.productId]?.(snapshot) ?? EMPTY_VIEW;
}
