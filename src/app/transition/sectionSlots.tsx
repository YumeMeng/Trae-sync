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
import {
  LibraryPage,
  type LibraryPageProps,
} from "../../features/library/LibraryPage";
import {
  AccountsPage,
  type AccountsPageProps,
} from "../../features/accounts/AccountsPage";
import {
  CheckinPage,
  type CheckinPageProps,
} from "../../features/checkin/CheckinPage";
import {
  EnvironmentPage,
  type EnvironmentPageProps,
} from "../../features/environment/EnvironmentPage";
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

// P4c-6c-1：accounts 的 TRAE 槽位切到合并页（首次真接线）。与 overview / checkin
// 同一形态：接缝内绑定产品协议值，页面内零产品判断。App 只传 active，故这里
// 不额外映射跳转/动作回调——onOpenDetail / onHeaderAction / onRowAction 缺省即
// 不执行（详情页与批量动作分别归 P4c-7 / 6b-2，本票不顺手做）。
const TraeAccountsSlot = (props: AccountsPageProps) => (
  <AccountsPage {...props} productId={CURRENT_PRODUCT_ID} />
);
// P4c-9b-2：accounts 的 WorkBuddy 槽位也切到合并页（账号页收官）。与上者同构：
// 接缝内绑定 WB 协议值，页面内零产品判断；旧 WorkBuddyPanel 随之删除。
const WorkBuddyAccountsSlot = (props: AccountsPageProps) => (
  <AccountsPage {...props} productId={WORKBUDDY_PRODUCT_ID} />
);

// P4d-8b-1：environment 的 TRAE 槽位切到合并页（与 accounts 同形）。接缝内绑定产品
// 协议值，页面内零产品判断。App 侧回调形状与新页不同：App 的库详情入口收
// LibraryRef（{ id, displayName }），新页只回传 envId——转换在 App 里做（见 App.tsx
// handleOpenEnvDetail），此处不反向改写页面签名。
const TraeEnvironmentSlot = (props: EnvironmentPageProps) => (
  <EnvironmentPage {...props} productId={CURRENT_PRODUCT_ID} />
);
// P4d-8b-1：environment 的 WorkBuddy 槽位同样指向合并页（WB 能力差异全在
// features/environment 的声明式装配表与 builder 里）。旧 components/WorkBuddyEnvironmentPage
// 保留到下一票删除，此刻不再被槽位引用。
const WorkBuddyEnvironmentSlot = (props: EnvironmentPageProps) => (
  <EnvironmentPage {...props} productId={WORKBUDDY_PRODUCT_ID} />
);

// P4e-8c：library 的 TRAE 槽位切到合并页（与 accounts / environment 同形）。接缝内绑定
// 产品协议值，页面内零产品判断；库实例（libraryId / libraryDisplayName）与返回、跳转
// 回调由 App 注入（App 现有 libraryDetail 态即库实例来源）。旧 pages/MasterLibraryDetail
// 与 pages/WorkBuddyLibraryPage 本票不删，此刻不再被槽位引用。
const TraeLibrarySlot = (props: LibraryPageProps) => (
  <LibraryPage {...props} productId={CURRENT_PRODUCT_ID} />
);
// P4e-8c：library 的 WorkBuddy 槽位同样指向合并页（WB 无库实例概念：页面缺省 libraryId
// 即主库，builder 只按产品查装配表）。旧 pages/WorkBuddyLibraryPage 保留到下一票删除。
const WorkBuddyLibrarySlot = (props: LibraryPageProps) => (
  <LibraryPage {...props} productId={WORKBUDDY_PRODUCT_ID} />
);

/**
 * 六区块 × 两产品的实现登记表（settings 也登记：其 WB 变体曾是整页早返回）。
 * 只登记、不判断：产品选择逻辑全部在 resolveSection 一处。
 */
export const SECTION_SLOTS: Record<SectionKey, SectionSlot> = {
  // P4a：两产品指向同一份总览实现（迁移完成的正常形态）。
  overview: { trae: TraeOverviewSlot, workbuddy: WorkBuddyOverviewSlot },
  // P4c-6c-1 / P4c-9b-2：两产品都指向同一份账号页实现（迁移完成的正常形态）。
  accounts: { trae: TraeAccountsSlot, workbuddy: WorkBuddyAccountsSlot },
  // P4b-3：两产品指向同一份签到实现（迁移完成的正常形态）。
  checkin: { trae: TraeCheckinSlot, workbuddy: WorkBuddyCheckinSlot },
  // P4d-8b-1：两产品都指向同一份环境页实现（迁移完成的正常形态）。
  environment: { trae: TraeEnvironmentSlot, workbuddy: WorkBuddyEnvironmentSlot },
  // P4e-8c：两产品都指向同一份库页实现（迁移完成的正常形态）。
  "master-library": { trae: TraeLibrarySlot, workbuddy: WorkBuddyLibrarySlot },
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