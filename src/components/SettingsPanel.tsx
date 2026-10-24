// TRANSITION: 仅过渡用，P4 页面迁移完成后删除（与 app/transition/sectionSlots.tsx 同批）。
//
// 设置页槽位分发器：本文件只做一件事——按槽位解析取实现。
// 原拆出的两个变体（settings/TraeSettingsVariant.tsx、settings/WorkBuddySettingsVariant.tsx）
// 已于 P4f-6 删除，此刻两产品槽位同指 features/settings/SettingsPage；
// 产品判断收敛到 app/transition/sectionSlots.tsx 的 resolveSection，此处零分支。
//
// 保留 productId 入参是兼容既有调用面（App 传当前产品；测试按产品渲染），
// 不改变任何一方的语义。
import type { ComponentType } from "react";
import { SECTION_SLOTS, resolveSection } from "../app/transition/sectionSlots";

interface SettingsPanelProps {
  /** 页面可见时才读取设置，避免后台 IPC。 */
  active: boolean;
  /** 当前产品（缺省 trae_cn，与产品指针缺失时的回退一致）。 */
  productId?: string;
}

/** 设置槽位的统一属性形状（P4f-5：合并页接 active + productId；旧两变体只接 active）。 */
interface SettingsVariantProps {
  active: boolean;
  productId: string;
}

// 设置页：按当前产品从槽位取变体渲染（TRAE 管理面 / WorkBuddy 管理面）。
export function SettingsPanel({ active, productId = "trae_cn" }: SettingsPanelProps) {
  const SettingsSection = resolveSection(
    SECTION_SLOTS.settings,
    productId,
  ) as ComponentType<SettingsVariantProps>;
  return <SettingsSection active={active} productId={productId} />;
}
