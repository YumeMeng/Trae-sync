import type { CapabilityFlagsDto } from "../types/workspace";

interface OperationsPanelProps {
  capabilities: CapabilityFlagsDto;
}

// 操作与备份入口：T01 阶段无历史操作，备份/恢复能力禁用。
export function OperationsPanel({ capabilities }: OperationsPanelProps) {
  return (
    <section className="operations-panel" role="region" aria-label="操作与备份">
      <h2>操作与备份</h2>
      <p className="operations-panel__empty">暂无操作记录。</p>
      <div className="operations-panel__actions">
        <button
          type="button"
          className="btn"
          disabled={!capabilities.backup_enabled}
          aria-disabled={!capabilities.backup_enabled}
        >
          查看备份
        </button>
        <button
          type="button"
          className="btn"
          disabled={!capabilities.restore_enabled}
          aria-disabled={!capabilities.restore_enabled}
        >
          手工恢复
        </button>
      </div>
    </section>
  );
}
