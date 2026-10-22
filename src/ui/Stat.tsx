import type { ReactNode } from "react";

/**
 * 指标原子（UI-DIRECTION §3）：数值在上、标签在下，数值优先。
 * 无业务概念、无 invoke、无产品概念；样式由使用方按域传入（本组件不自带 CSS）。
 */

/** 使用方提供的样式钩子：根 / 数值 / 标签 / 短注四处，用于复用既有页面样式。 */
export interface StatClasses {
  readonly root: string;
  readonly value: string;
  readonly label: string;
  readonly note?: string;
}

export interface StatProps {
  /** 数值（已格式化；"—" 表示未获取，不写 0）。 */
  value: ReactNode;
  /** 字段标签（≤6 字）。 */
  label: string;
  /** 数值下方短注（≤12 字；如「1/2 账号已获取」）。 */
  note?: string;
  /** 口径说明：悬浮提示，不占主视野。 */
  hint?: string;
  /** warn = 该指标当前读不到（需要处理，非错误）。 */
  tone?: "warn";
  testId?: string;
  classes?: StatClasses;
}

const DEFAULT_CLASSES: StatClasses = {
  root: "ui-stat",
  value: "ui-stat__value",
  label: "ui-stat__label",
  note: "ui-stat__note",
};

export function Stat({
  value,
  label,
  note,
  hint,
  tone,
  testId,
  classes = DEFAULT_CLASSES,
}: StatProps) {
  return (
    <div
      className={tone === "warn" ? `${classes.root} ${classes.root}--warn` : classes.root}
      title={hint}
      data-testid={testId}
    >
      <span className={classes.value}>{value}</span>
      <span className={classes.label}>{label}</span>
      {note !== undefined && <span className={classes.note ?? classes.label}>{note}</span>}
    </div>
  );
}
