import type { ReactNode } from "react";
import { selectResultTone } from "./OutcomeLabels";

/**
 * 签到选择行（P19-B2④ 从 CheckinPage L599 抽取为共享控件）：
 * 复选框 + 账号名 + meta + 徽章槽 + 最近一次签到结果徽章。
 * 产品差异只经 name/meta/badge/resultLabel 进入；行结构与类名全产品一致。
 */
export interface CheckinSelectRowProps {
  name: string;
  meta: ReactNode;
  badge: ReactNode;
  selected: boolean;
  onToggle: () => void;
  disabled: boolean;
  /** 最近一次结果文案（产品层经 outcomeLabelFor 求得；null = 无结果）。 */
  resultLabel: string | null;
  /** 结果语气（产品层经 selectResultTone 求得）。 */
  resultTone: "safe" | "neutral" | "warning";
}

export function CheckinSelectRow({
  name,
  meta,
  badge,
  selected,
  onToggle,
  disabled,
  resultLabel,
  resultTone,
}: CheckinSelectRowProps) {
  return (
    <li className={`account-center__profile${selected ? " account-center__profile--selected" : ""}`}>
      <label className="account-center__profile-select">
        <input type="checkbox" checked={selected} onChange={onToggle} disabled={disabled} />
        <span className="account-center__profile-copy">
          <strong>{name}</strong>
          {meta ? <span>{meta}</span> : null}
        </span>
      </label>
      <span className="account-center__profile-status">
        {badge}
        {resultLabel && (
          <span className={`status-badge status-badge--${resultTone}`}>{resultLabel}</span>
        )}
      </span>
    </li>
  );
}

/** 便捷换算：outcome → 选择行语气（供产品层组装 resultLabel 时使用）。 */
export { selectResultTone };
