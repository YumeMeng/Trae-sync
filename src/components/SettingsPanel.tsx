// 设置页入口（P5-4b）：直渲染合并页 features/settings/SettingsPage，零槽位间接层。
//
// 与 App.tsx 内联 settings 区块同形：同一个 SettingsPage、同一份属性形状。
// 产品差异全部在页面内的声明式能力表与 builder 里，本文件零产品分支。
// 保留本组件名与 productId 缺省值只为兼容既有调用面（App 传当前产品；
// 测试按产品渲染），不改变任何一方语义。
import { SettingsPage } from "../features/settings/SettingsPage";

interface SettingsPanelProps {
  /** 页面可见时才读取设置，避免后台 IPC。 */
  active: boolean;
  /** 当前产品（缺省 trae_cn，与产品指针缺失时的回退一致）。 */
  productId?: string;
}

// 设置页：直接渲染合并页（TRAE / WorkBuddy 共用一份实现）。
export function SettingsPanel({ active, productId = "trae_cn" }: SettingsPanelProps) {
  return <SettingsPage active={active} productId={productId} />;
}
