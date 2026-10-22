// TRANSITION: 仅过渡用，P4 页面迁移完成后删除（与 app/transition/sectionSlots.tsx 同批）。
//
// WorkBuddy 设置页变体：由原 SettingsPanel 第 260-287 行的整页早返回拆出
// （`if (productId === "workbuddy") return (...)`）。拆出的 JSX 逐字未改，
// 只是从"函数体内的条件返回"变成"独立组件"——产品选择改由槽位解析完成
// （app/transition/sectionSlots.tsx），本组件内部零 productId 判断。

import { WorkBuddyAutoCheckinSettings } from "../WorkBuddyAutoCheckinSettings";
import { WorkBuddyBackupSection } from "../WorkBuddyBackupSection";

/**
 * WB 设置变体（IA §1.6 备份区诚实呈现；P19.1③ 每日自动签到设置卡归位设置页，
 * 对齐 TRAE 真机排布——签到页只留签到动作区）；WB 备份 = 彻底删除前置自动
 * 滚动单份（无独立链命令 → 不渲染不可用按钮）。
 *
 * 页面级 aria 契约与原实现一致：role=region + aria-label="设置"，
 * h1 带 data-page-title="settings"（App 导航后按它移焦）。
 */
export function WorkBuddySettingsVariant() {
  return (
    <section className="settings-panel" role="region" aria-label="设置">
      <header className="page-header">
        <div className="page-header__copy">
          <span className="page-header__eyebrow">偏好与安全</span>
          <h1 data-page-title="settings" tabIndex={-1}>设置</h1>
        </div>
      </header>

      <div className="settings-panel__section-heading">
        <div>
          <span className="workbench__eyebrow">应用行为</span>
          <h3>自动签到</h3>
        </div>
      </div>
      <WorkBuddyAutoCheckinSettings />

      <div className="settings-panel__section-heading">
        <div>
          <span className="workbench__eyebrow">数据安全</span>
          <h3>对话库备份</h3>
        </div>
      </div>
      <WorkBuddyBackupSection />
    </section>
  );
}
