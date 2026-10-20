import { CalendarCheck } from "lucide-react";

/**
 * 签到批量操作行（P19-B2④ 从 CheckinPage L392-435 抽取为共享控件）：
 * - batch（真实模式四动作）：一键全签 / 一键补签 / 签到所选（勾选子集非空
 *   且非全量时出现）/ 取消未开始任务；
 * - select（fixture 演示模式）：全部签到（所选语义）/ 取消未开始任务。
 * U-3 数据口径内聚在本控件：全签=全部账号（已签幂等跳过）；补签=今日未签；
 * 所选=勾选子集。testid 契约冻结（checkin-actions / checkin-run-all /
 * checkin-run-pending / checkin-run-selected）。
 */
export interface CheckinActionsRowProps {
  variant: "batch" | "select";
  /** 签到能力可用（不可用时执行按钮禁用）。 */
  enabled: boolean;
  busy: boolean;
  poolCount: number;
  /** 今日未签数（batch 模式补签按钮计数与禁用）。 */
  pendingCount: number;
  /** 已勾选数（select 模式主按钮计数与禁用；batch 模式「签到所选」出现条件）。 */
  selectionCount: number;
  /** batch：对全部账号执行；select：对所选执行。 */
  onRunAll(): void;
  /** batch：只对今日未签执行。 */
  onRunPending(): void;
  /** batch：对勾选子集执行（「签到所选」按钮）。 */
  onRunSelected(): void;
  /** 取消未开始任务（可选：后端整体批次不可中断的产品不传即不渲染取消按钮）。 */
  onCancel?(): void;
}

export function CheckinActionsRow({
  variant,
  enabled,
  busy,
  poolCount,
  pendingCount,
  selectionCount,
  onRunAll,
  onRunPending,
  onRunSelected,
  onCancel,
}: CheckinActionsRowProps) {
  if (variant === "batch") {
    // U-3 四动作直达（真实模式）：全签/补签常驻，勾选子集出现「签到所选」。
    const showSelectedRun = selectionCount > 0 && selectionCount < poolCount;
    return (
      <div className="checkin-page__action-row" data-testid="checkin-actions">
        <button
          className="btn btn--primary"
          type="button"
          onClick={onRunAll}
          disabled={!enabled || busy}
          data-testid="checkin-run-all"
          title="对全部账号执行签到（已签账号自动跳过，不重复领取）"
        >
          <CalendarCheck size={15} aria-hidden="true" />{busy ? "签到执行中…" : `一键全签（${poolCount}）`}
        </button>
        <button
          className="btn"
          type="button"
          onClick={onRunPending}
          disabled={!enabled || busy || pendingCount === 0}
          data-testid="checkin-run-pending"
          title="只对今日未签账号执行签到（少发请求）"
        >
          <CalendarCheck size={15} aria-hidden="true" />一键补签（{pendingCount}）
        </button>
        {showSelectedRun && (
          <button
            className="btn"
            type="button"
            onClick={onRunSelected}
            disabled={!enabled || busy}
            data-testid="checkin-run-selected"
          >
            <CalendarCheck size={15} aria-hidden="true" />签到所选（{selectionCount}）
          </button>
        )}
        {onCancel && (
          <button className="btn btn--quiet" type="button" onClick={onCancel} disabled={!busy}>取消未开始任务</button>
        )}
      </div>
    );
  }
  // fixture 演示模式沿用「全部签到（所选）」语义。
  return (
    <div className="checkin-page__action-row">
      <button className="btn btn--primary" type="button" onClick={onRunAll} disabled={!enabled || selectionCount === 0 || busy}>
        <CalendarCheck size={15} aria-hidden="true" />{busy ? "签到执行中…" : `全部签到（${selectionCount}）`}
      </button>
      {onCancel && (
        <button className="btn btn--quiet" type="button" onClick={onCancel} disabled={!busy}>取消未开始任务</button>
      )}
    </div>
  );
}
