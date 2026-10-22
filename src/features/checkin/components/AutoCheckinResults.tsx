import { outcomeLabelFor } from "../../../components/shared/checkin/OutcomeLabels";

/**
 * 自动签到结果清单（P4b-3）：auto-checkin-progress 事件逐账号回填，
 * 与手动批次的行内结果分离——自动批次由后端调度发起，不在本页勾选列表里。
 */
export interface AutoResultRow {
  readonly profileId: string;
  readonly displayName: string;
  readonly outcome: string;
  readonly detailCode: string | null;
}

export function AutoCheckinResults({
  rows,
  productId,
}: {
  rows: readonly AutoResultRow[];
  productId: string;
}) {
  if (rows.length === 0) return null;
  return (
    <ul
      className="account-center__checkin-results"
      aria-label="自动签到结果"
      data-testid="auto-checkin-results"
    >
      {rows.map((row) => (
        <li key={row.profileId}>
          <strong>{row.displayName}</strong>
          <span>{outcomeLabelFor(productId, row.outcome, row.detailCode)}</span>
        </li>
      ))}
    </ul>
  );
}
