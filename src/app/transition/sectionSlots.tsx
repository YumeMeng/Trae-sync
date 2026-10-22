// TRANSITION: 仅过渡用，P4 页面迁移完成后删除
//
// 页面槽位接缝（P2a，UI-CHARTER §4.1 的过渡例外，见 docs/architecture/exceptions.md）：
// 重构目标态是「一个壳 + 一份页面 + 能力裁剪」，但 P4 六页迁移尚未完成，
// 六区块此刻仍各有 trae / workbuddy 两套实现。本文件把这六处「整页选哪个
// 组件」的判断**收敛到唯一一处**：
//
//   App.tsx 与 SettingsPanel 只问 resolveSection(SECTION_SLOTS.x, productId)，
//   不再自己写 isWorkBuddy / productId === "workbuddy"。
//
// 这是 strangler 接缝，不是页面重写：登记的组件内部实现一行未动，
// 产品差异仍留在各自页面里；P4 每迁完一页就把该页 trae 实现换成合并页，
// 六页全部迁完时本文件整份删除（届时 app 层不再有任何产品整页分支）。
import type { ComponentType } from "react";
import type { SectionKey } from "../../platform/productRegistry";
import { WORKBUDDY_PRODUCT_ID } from "../../types/product";
import { OverviewPage } from "../../pages/OverviewPage";
import { WorkBuddyOverviewPage } from "../../pages/WorkBuddyOverviewPage";
import { MasterLibraryDetail } from "../../pages/MasterLibraryDetail";
import { WorkBuddyLibraryPage } from "../../pages/WorkBuddyLibraryPage";
import { AccountWorkbench } from "../../components/AccountWorkbench";
import { CheckinPage } from "../../components/CheckinPage";
import { EnvironmentPage } from "../../components/EnvironmentPage";
import { WorkBuddyPanel, WorkBuddyCheckinPage } from "../../components/WorkBuddyPanel";
import { WorkBuddyEnvironmentPage } from "../../components/WorkBuddyEnvironmentPage";
import { TraeSettingsVariant } from "../../components/settings/TraeSettingsVariant";
import { WorkBuddySettingsVariant } from "../../components/settings/WorkBuddySettingsVariant";

// any-exception: 槽位接缝需以统一 props 形状承载六页异构属性（overview 要 state、
// 库页要 library、环境页要两个回调），接缝本身不该反向改写页面 props；
// 已登记 docs/architecture/exceptions.md E-01，P4 页面迁移完成后随本文件删除。
/**
 * 单个区块的两种实现登记项：组件按统一形状调用，具体属性由各槽位自行传入。
 */
export interface SectionSlot {
  trae: ComponentType<any>;
  workbuddy: ComponentType<any>;
}

/**
 * 六区块 × 两产品的实现登记表（settings 也登记：其 WB 变体曾是整页早返回）。
 * 只登记、不判断：产品选择逻辑全部在 resolveSection 一处。
 */
export const SECTION_SLOTS: Record<SectionKey, SectionSlot> = {
  overview: { trae: OverviewPage, workbuddy: WorkBuddyOverviewPage },
  accounts: { trae: AccountWorkbench, workbuddy: WorkBuddyPanel },
  checkin: { trae: CheckinPage, workbuddy: WorkBuddyCheckinPage },
  environment: { trae: EnvironmentPage, workbuddy: WorkBuddyEnvironmentPage },
  "master-library": { trae: MasterLibraryDetail, workbuddy: WorkBuddyLibraryPage },
  settings: { trae: TraeSettingsVariant, workbuddy: WorkBuddySettingsVariant },
};

/**
 * 槽位解析：唯一的产品整页选择点。
 *
 * 只认 workbuddy 协议值，其余（含指针缺失回退的 trae_cn）一律走 trae 实现——
 * 与接缝引入前的行为完全一致：此前各处写作 isWorkBuddy ? B : A，
 * isWorkBuddy = product_id === "workbuddy"，失败/缺省都落 A。
 */
export function resolveSection(slot: SectionSlot, productId: string): ComponentType<any> {
  return productId === WORKBUDDY_PRODUCT_ID ? slot.workbuddy : slot.trae;
}
