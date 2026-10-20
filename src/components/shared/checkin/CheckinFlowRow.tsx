import type { ReactNode } from "react";
import { CalendarCheck, Loader2 } from "lucide-react";
import { InterWaitCountdown } from "./InterWaitCountdown";
import { flowResultTone, outcomeLabelFor } from "./OutcomeLabels";

/**
 * U-3 签到流单列行（P19-B2④ 从 CheckinPage L526 槽位化）：
 * 勾选框 + 名字 + 签到槽位徽章 + meta + 行内单签按钮；执行期间随 phase
 * 演进（queued 排队 / running 执行中高亮 / done 结果），批次结束后 result
 * 直接落在行内——选择/执行/结果零重复。
 * 产品差异只经 displayName/badge/meta 插槽与 productId 文案表进入；
 * testid 契约冻结（checkin-inline-* / checkin-inter-wait）。
 */
export interface CheckinFlowRowProps {
  productId: string;
  profileId: string;
  displayName: string;
  /** G10 签到槽徽章（产品层由 deriveCheckinSlotState 求态）。 */
  badge: ReactNode;
  /** 行内元信息（额度/最近验证等，产品层组装）。 */
  meta: ReactNode;
  selected: boolean;
  onToggle: () => void;
  disabled: boolean;
  phase: "idle" | "queued" | "running" | "inter_wait" | "done";
  /** 本行结果（outcome/detail_code 与 CheckinResultDto 同口径）。 */
  result: { outcome: string; detail_code: string | null } | null;
  waitSecs: number | null;
  onInlineCheckin: () => void;
  /** 行内单签按钮的可用性（仅未签账号可点）。 */
  inlineEnabled: boolean;
}

export function CheckinFlowRow({
  productId,
  profileId,
  displayName,
  badge,
  meta,
  selected,
  onToggle,
  disabled,
  phase,
  result,
  waitSecs,
  onInlineCheckin,
  inlineEnabled,
}: CheckinFlowRowProps) {
  const outcome = result?.outcome ?? null;
  const detailCode = result?.detail_code ?? null;
  const resultTone = flowResultTone(outcome);
  return (
    <li className={`checkin-flow__row${phase === "running" ? " checkin-flow__row--active" : ""}`}>
      <label className="checkin-flow__select">
        <input type="checkbox" checked={selected} onChange={onToggle} disabled={disabled} />
        <span className="checkin-flow__id">
          <span className="checkin-flow__name-row">
            <strong>{displayName}</strong>
            {badge}
          </span>
          <span className="checkin-flow__meta">{meta}</span>
        </span>
      </label>
      <span className="checkin-flow__side">
        {phase === "queued" && <span className="checkin-flow__phase">排队中</span>}
        {phase === "running" && (
          <span className="checkin-flow__phase checkin-flow__phase--active">
            <Loader2 size={13} className="checkin-flow__spinner" aria-hidden="true" />签到中…
          </span>
        )}
        {phase === "inter_wait" && waitSecs !== null && (
          <span className="checkin-flow__phase checkin-flow__phase--active" data-testid="checkin-inter-wait">
            <InterWaitCountdown seconds={waitSecs} />
          </span>
        )}
        {(phase === "done" || phase === "idle") && outcome && (
          <span className={`checkin-flow__result checkin-flow__result--${resultTone}`}>
            {outcomeLabelFor(productId, outcome, detailCode)}
          </span>
        )}
        {/* 行内单账号签到：仅未签账号可点；执行期间禁用。 */}
        <button
          className="btn checkin-flow__inline-btn"
          type="button"
          onClick={onInlineCheckin}
          disabled={disabled || !inlineEnabled}
          title={inlineEnabled ? "仅签到该账号" : "今日已签到"}
          data-testid={`checkin-inline-${profileId}`}
        >
          <CalendarCheck size={13} aria-hidden="true" />签到
        </button>
      </span>
    </li>
  );
}
