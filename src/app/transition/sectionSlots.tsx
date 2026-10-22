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
import { CURRENT_PRODUCT_ID, WORKBUDDY_PRODUCT_ID } from "../../types/product";
import {
  OverviewPage,
  type OverviewPageProps,
} from "../../features/overview/OverviewPage";
import { MasterLibraryDetail } from "../../pages/MasterLibraryDetail";
import { WorkBuddyLibraryPage } from "../../pages/WorkBuddyLibraryPage";
import { AccountWorkbench } from "../../components/AccountWorkbench";
import {
  CheckinPage,
  type CheckinPageProps,
} from "../../features/checkin/CheckinPage";
import { EnvironmentPage } from "../../components/EnvironmentPage";
import { WorkBuddyPanel } from "../../components/WorkBuddyPanel";
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

// P4a：overview 已合并为一份实现。两个槽位在接缝内各自绑定产品协议值后指向
// 同一组件——页面内零产品判断（差异全在 features/overview 的声明式配置），
// 而「切产品即重挂载重拉数据」的既有契约保持不变（两个包装组件类型不同）。
// 产品绑定只能发生在接缝里，这是 E-01 登记的唯一允许位置。
const TraeOverviewSlot = (props: OverviewPageProps) => (
  <OverviewPage {...props} productId={CURRENT_PRODUCT_ID} />
);
const WorkBuddyOverviewSlot = (props: OverviewPageProps) => (
  <OverviewPage {...props} productId={WORKBUDDY_PRODUCT_ID} />
);

// P4b-3：checkin 也已合并为一份实现。与 overview 同一形态——两个槽位在接缝内
// 绑定产品协议值后指向同一组件，页面内零产品判断（差异全在 features/checkin
// 的声明式配置）。两个包装组件类型不同，切产品仍会重挂载重拉数据。
const TraeCheckinSlot = (props: CheckinPageProps) => (
  <CheckinPage {...props} productId={CURRENT_PRODUCT_ID} />
);
const WorkBuddyCheckinSlot = (props: CheckinPageProps) => (
  <CheckinPage {...props} productId={WORKBUDDY_PRODUCT_ID} />
);

/**
 * 六区块 × 两产品的实现登记表（settings 也登记：其 WB 变体曾是整页早返回）。
 * 只登记、不判断：产品选择逻辑全部在 resolveSection 一处。
 */
export const SECTION_SLOTS: Record<SectionKey, SectionSlot> = {
  // P4a：两产品指向同一份总览实现（迁移完成的正常形态）。
  overview: { trae: TraeOverviewSlot, workbuddy: WorkBuddyOverviewSlot },
  accounts: { trae: AccountWorkbench, workbuddy: WorkBuddyPanel },
  // P4b-3：两产品指向同一份签到实现（迁移完成的正常形态）。
  checkin: { trae: TraeCheckinSlot, workbuddy: WorkBuddyCheckinSlot },
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