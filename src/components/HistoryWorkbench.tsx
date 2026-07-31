import type { HistorySummaryDto, CapabilityFlagsDto } from "../types/workspace";

interface HistoryWorkbenchProps {
  history: HistorySummaryDto;
  capabilities: CapabilityFlagsDto;
  honestStatus: string;
}

// 历史库工作台：方案 D 的主界面。
// T01 阶段历史库为空，所有真实能力按钮禁用，显示诚实状态。
export function HistoryWorkbench({
  history,
  capabilities,
  honestStatus,
}: HistoryWorkbenchProps) {
  return (
    <section className="workbench" role="region" aria-label="历史库">
      <div className="workbench__header">
        <h2>历史库</h2>
        <div className="workbench__summary">
          <span>账号 {history.account_count}</span>
          <span>项目 {history.project_count}</span>
          <span>对话 {history.session_count}</span>
        </div>
        <button
          type="button"
          className="btn btn--primary"
          disabled={!capabilities.scan_enabled}
          aria-disabled={!capabilities.scan_enabled}
        >
          查找新历史
        </button>
      </div>

      <div className="workbench__body">
        <div className="workbench__tree" role="tree" aria-label="账号与项目树">
          <p className="workbench__empty">尚未收录任何历史。{honestStatus}</p>
        </div>

        <div className="workbench__plan" role="region" aria-label="同步计划">
          <h3>同步计划</h3>
          <dl className="plan-summary">
            <dt>已选择</dt>
            <dd>0</dd>
            <dt>本次可同步</dt>
            <dd>0</dd>
            <dt>已在当前账号</dt>
            <dd>0</dd>
            <dt>需要处理</dt>
            <dd>0</dd>
          </dl>
          <button
            type="button"
            className="btn btn--primary"
            disabled={!capabilities.sync_enabled}
            aria-disabled={!capabilities.sync_enabled}
          >
            检查并安全同步
          </button>
        </div>
      </div>

      <p className="workbench__honest-status" role="status">
        {honestStatus}
      </p>
    </section>
  );
}
