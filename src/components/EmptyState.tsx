import type { ReactNode } from "react";

/**
 * 统一空态组件（P11-6 任务 4）：图标 + 标题 + 说明 + 可选动作的轻量组件，
 * 收敛各页自持的 __empty 实现结构与样式。文案仍由各页提供（本组件不发明文案）。
 */
export interface EmptyStateProps {
  /** 顶部图标（lucide 元素；装饰性，读屏跳过）。 */
  icon?: ReactNode;
  /** 主说明行（比 description 强调一档）。 */
  title?: ReactNode;
  /** 说明文本或补充行。 */
  description?: ReactNode;
  /** 可选动作区（按钮等）。 */
  action?: ReactNode;
  /** 玻璃卡形态（区块级说明卡用）；缺省为轻量空态行。 */
  panel?: boolean;
  /** 页面原空态类名钩子：保留既有测试/e2e 选择器，不再承担样式职责。 */
  className?: string;
  /** 无障碍角色透传（如 loading 态的 status）。 */
  role?: string;
  /** 测试锚点透传。 */
  testId?: string;
}

export function EmptyState({
  icon,
  title,
  description,
  action,
  panel = false,
  className,
  role,
  testId,
}: EmptyStateProps) {
  const classes = ["empty-state", panel ? "empty-state--panel" : "", className ?? ""]
    .filter(Boolean)
    .join(" ");
  return (
    <div className={classes} role={role} data-testid={testId}>
      {icon && (
        <span className="empty-state__icon" aria-hidden="true">
          {icon}
        </span>
      )}
      {title && <p className="empty-state__title">{title}</p>}
      {description && <p className="empty-state__desc">{description}</p>}
      {action && <div className="empty-state__action">{action}</div>}
    </div>
  );
}
