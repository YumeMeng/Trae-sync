import type { ReactNode } from "react";

/**
 * 徽章原子（UI-DIRECTION §4/§8）：四态语义色，文字 ≤6 字。
 * 只表达「需要处理（琥珀）/ 错误（红）/ 可操作（靛蓝）/ 中性」，
 * 不提供「成功绿」——正常态一律中性。
 */
export type BadgeTone = "neutral" | "warn" | "danger" | "accent";

export interface BadgeProps {
  children: ReactNode;
  tone?: BadgeTone;
  /** 悬浮说明（补充口径，不进主视野）。 */
  hint?: string;
  testId?: string;
}

/** 复用既有徽章样式族（workbench.css 的 status-badge 四态），不新增 CSS。 */
const TONE_CLASS: Record<BadgeTone, string> = {
  neutral: "status-badge status-badge--neutral",
  warn: "status-badge status-badge--warning",
  danger: "status-badge status-badge--danger",
  accent: "status-badge status-badge--safe",
};

export function Badge({ children, tone = "neutral", hint, testId }: BadgeProps) {
  return (
    <span className={TONE_CLASS[tone]} title={hint} data-testid={testId}>
      {children}
    </span>
  );
}
