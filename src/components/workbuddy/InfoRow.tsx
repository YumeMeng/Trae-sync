import type { ReactNode } from "react";

/**
 * 信息行（P4c-9b-2 从 WorkBuddyPanel 迁出）：label + value 的两列展示。
 * WorkBuddy 域各页共用（环境页 16 处引用）；样式类 `workbuddy-row` 原样沿用，
 * 迁出只为让 WorkBuddyPanel 旧账号页可被删除——行为与呈现零变更。
 */
export function InfoRow({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div className="workbuddy-row">
      <span className="workbuddy-row__label">{label}</span>
      <span className="workbuddy-row__value">{children}</span>
    </div>
  );
}
