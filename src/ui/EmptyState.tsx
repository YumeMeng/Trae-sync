import type { ReactNode } from "react";

/**
 * 空态原子（UI-DIRECTION §5）：图标 + 一句结论（≤20 字）+ 可选动作。
 * 只承载结构；样式由使用方按域传入（页面既有空态行类名可直接复用）。
 */
export interface EmptyStateProps {
  /** 一句结论。 */
  title: string;
  /** 顶部图标（装饰性，读屏跳过）。 */
  icon?: ReactNode;
  /** 可选动作区（按钮等）。 */
  action?: ReactNode;
  className?: string;
  testId?: string;
}

export function EmptyState({
  title,
  icon,
  action,
  className = "ui-empty",
  testId,
}: EmptyStateProps) {
  return (
    <div className={className} data-testid={testId}>
      {icon && (
        <span className="ui-empty__icon" aria-hidden="true">
          {icon}
        </span>
      )}
      <span className="ui-empty__title">{title}</span>
      {action}
    </div>
  );
}
