import type { CheckinActionView } from "../config";

/**
 * 签到批量操作区（P4b-3）：动作按钮行 + 总进度条。
 *
 * 动作与文案全部来自视图模型（config 的 actions[]），本组件只做渲染与
 * 回调派发（按 key 交给页面的统一批次通道），不做产品判断。
 */
export interface CheckinBatchBarProps {
  actions: readonly CheckinActionView[];
  /** 执行中的进度（busy=false 时不渲染进度条）。 */
  progress: { readonly completed: number; readonly total: number };
  busy: boolean;
  onAction: (key: string) => void;
}

export function CheckinBatchBar({ actions, progress, busy, onAction }: CheckinBatchBarProps) {
  return (
    <>
      <div className="checkin-page__action-row" data-testid="checkin-actions">
        {actions.map((action) => (
          <button
            key={action.key}
            className={`btn${action.tone === "primary" ? " btn--primary" : action.tone === "quiet" ? " btn--quiet" : ""}`}
            type="button"
            onClick={() => onAction(action.key)}
            disabled={!action.enabled}
            data-testid={action.testId}
          >
            {action.label}
          </button>
        ))}
      </div>

      {busy && progress.total > 0 && (
        <div className="checkin-progress" role="status" data-testid="checkin-progress">
          <div className="checkin-progress__track">
            <div
              className="checkin-progress__fill"
              style={{ width: `${Math.round((progress.completed / progress.total) * 100)}%` }}
            />
          </div>
          <span className="checkin-progress__label">
            {progress.completed}/{progress.total}
          </span>
        </div>
      )}
    </>
  );
}
