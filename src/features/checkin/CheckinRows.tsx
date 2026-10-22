import { CheckinSlotBadge } from "../../components/StatusBadges";
import { CheckinFlowRow } from "../../components/shared/checkin/CheckinFlowRow";
import { CheckinSelectRow } from "../../components/shared/checkin/CheckinRow";
import { outcomeLabelFor, selectResultTone } from "../../components/shared/checkin/OutcomeLabels";
import { Badge } from "../../ui/Badge";
import type { CheckinRowBadge, CheckinView } from "./config";

/**
 * 签到行列表（P4b-2）：两种列表形态共用同一份行映射——
 * flow = 演进式单列（排队/执行中/结果随批次变），plain = 简单选择列表（演示模式）。
 * 行数据全部来自视图模型；本文件不做产品判断，也不读数据。
 */

/** 行内状态徽章：slot = G10 签到槽六态；text = 行级故障的诚实文案。 */
function badgeNode(badge: CheckinRowBadge) {
  return badge.kind === "slot"
    ? <CheckinSlotBadge state={badge.state} />
    : <Badge tone="neutral" hint={badge.hint}>{badge.word}</Badge>;
}

export interface CheckinRowsProps {
  view: CheckinView;
  selection: readonly string[];
  busy: boolean;
  /** 当前执行到的行（进度游标；null = 不在执行）。 */
  cursorId: string | null;
  results: Record<string, { outcome: string; detail_code: string | null }>;
  onToggle: (profileId: string) => void;
  onInlineRun: (profileId: string) => void;
}

export function CheckinRows({
  view,
  selection,
  busy,
  cursorId,
  results,
  onToggle,
  onInlineRun,
}: CheckinRowsProps) {
  return (
    <ul
      className={view.listVariant === "flow" ? "checkin-flow" : "account-center__profile-list"}
      aria-label="签到账号与结果"
    >
      {view.rows.map((row) => {
        const result = results[row.profileId] ?? null;
        const selected = selection.includes(row.profileId);
        const badge = row.badge === null ? null : badgeNode(row.badge);
        if (view.listVariant !== "flow") {
          return (
            <CheckinSelectRow
              key={row.profileId}
              name={row.displayName}
              meta={row.meta}
              badge={badge}
              selected={selected}
              onToggle={() => onToggle(row.profileId)}
              disabled={busy}
              resultLabel={result === null
                ? null
                : outcomeLabelFor(view.productId, result.outcome, result.detail_code)}
              resultTone={selectResultTone(result?.outcome ?? null)}
            />
          );
        }
        // 演进式阶段：空闲 / 已出结果 / 当前执行行 / 排队。
        const phase = !busy
          ? "idle"
          : result !== null
            ? "done"
            : row.profileId === cursorId
              ? "running"
              : "queued";
        return (
          <CheckinFlowRow
            key={row.profileId}
            productId={view.productId}
            profileId={row.profileId}
            displayName={row.displayName}
            badge={badge}
            meta={view.columns.meta ? row.meta : ""}
            selected={selected}
            onToggle={() => onToggle(row.profileId)}
            disabled={busy}
            phase={phase}
            result={result}
            waitSecs={null}
            onInlineCheckin={() => onInlineRun(row.profileId)}
            inlineEnabled={
              view.columns.inlineAction && row.error === null && row.checkedIn !== true
            }
          />
        );
      })}
    </ul>
  );
}
