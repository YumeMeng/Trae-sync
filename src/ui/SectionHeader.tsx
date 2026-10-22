import type { ReactNode } from "react";

/**
 * 标题原子：页面标题（h1）与区块标题（h2）同构。
 * 页面标题需要被导航聚焦时传 focusAnchor（App 按 [data-page-title] 移焦）。
 */
export interface SectionHeaderProps {
  /** 标题文案（页面 ≤8 字 / 区块 ≤10 字）。 */
  title: string;
  level?: 1 | 2;
  /** 焦点锚点（写入 data-page-title；同时使标题可编程聚焦）。 */
  focusAnchor?: string;
  /** 右侧动作（按钮/徽章）；缺省不出。 */
  action?: ReactNode;
  className?: string;
  testId?: string;
}

export function SectionHeader({
  title,
  level = 2,
  focusAnchor,
  action,
  className = "ui-section-header",
  testId,
}: SectionHeaderProps) {
  const Heading = level === 1 ? "h1" : "h2";
  return (
    <div className={className} data-testid={testId}>
      <Heading
        data-page-title={focusAnchor}
        tabIndex={focusAnchor === undefined ? undefined : -1}
      >
        {title}
      </Heading>
      {action}
    </div>
  );
}
