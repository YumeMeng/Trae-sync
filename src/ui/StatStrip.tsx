import type { ReactNode } from "react";

/**
 * 摘要条原子（UI-DIRECTION §5）：首屏 3–5 个指标的横向容器。
 * 只负责分组语义与栅格钩子；指标本身由 Stat 渲染。
 */
export interface StatStripProps {
  children: ReactNode;
  /** 页面既有栅格样式钩子。 */
  className?: string;
  testId?: string;
  /** 读屏上下文名称。 */
  label?: string;
}

export function StatStrip({
  children,
  className = "ui-stat-strip",
  testId,
  label,
}: StatStripProps) {
  return (
    <div className={className} data-testid={testId} role="group" aria-label={label}>
      {children}
    </div>
  );
}
