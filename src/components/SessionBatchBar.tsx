import { CheckSquare, Square, X } from "lucide-react";

/**
 * P5-8a Gmail 式批量操作栏（ADR-0022 决策 4）：
 * 浏览态无勾选框；进入选择态后浮出本栏（全选 / 批量动作 / 完成）。
 * 主列表与归档视图共用，动作按钮由调用方以 children 注入
 * （主列表：归档所选；归档视图：恢复所选 + 删除所选）。
 */
export function SessionBatchBar({
  selectedCount,
  totalCount,
  busy,
  allSelected,
  onSelectAll,
  onDone,
  children,
}: {
  /** 当前已勾选会话数。 */
  selectedCount: number;
  /** 当前视图可选会话总数（全选判定）。 */
  totalCount: number;
  /** 操作进行中：按钮禁用防重复提交。 */
  busy: boolean;
  /** 是否已全选（勾选图标切换）。 */
  allSelected: boolean;
  onSelectAll: () => void;
  onDone: () => void;
  children: React.ReactNode;
}) {
  return (
    <div className="batch-bar" data-testid="session-batch-bar" role="toolbar" aria-label="批量操作">
      <button
        className="btn btn--quiet"
        type="button"
        onClick={onSelectAll}
        disabled={busy || totalCount === 0}
        data-testid="batch-select-all"
      >
        {allSelected ? <CheckSquare size={15} aria-hidden="true" /> : <Square size={15} aria-hidden="true" />}
        {allSelected ? "取消全选" : "全选"}
      </button>
      <span className="batch-bar__count" data-testid="batch-count">
        已选 {selectedCount} / {totalCount} 项
      </span>
      <div className="batch-bar__actions">
        {children}
      </div>
      <button
        className="btn btn--quiet"
        type="button"
        onClick={onDone}
        disabled={busy}
        data-testid="batch-done"
      >
        <X size={15} aria-hidden="true" />完成
      </button>
    </div>
  );
}
