import { ArrowDownAZ } from "lucide-react";
import type { AccountsFiltersView } from "../config";

/**
 * 账号页控件行（P4c-6c-1 从页面抽出，纯展示）：过滤档 + 排序档。
 * 选项、文案、计数与 testid 键全部来自 builder 的 filters 视图模型
 * （testid = account-filter-<key> / account-sort），本文件不拼文案、不判断产品。
 * 抽出动机：AccountsPage 需保持 ≤300 行预算，控件行是内聚且无状态的一块。
 */
export interface AccountsListControlsProps {
  readonly filters: AccountsFiltersView;
  /** 受控排序值：遗留档位不在本产品选项内时已由页面回退首档。 */
  readonly sortValue: string;
  readonly onFilterChange: (key: string) => void;
  readonly onSortChange: (key: string) => void;
}

export function AccountsListControls({ filters, sortValue, onFilterChange, onSortChange }: AccountsListControlsProps) {
  return (
    <div className="account-center__list-controls">
      {filters.options.length > 0 && (
        <div className="seg-control" role="group" aria-label="账号过滤">
          {filters.options.map((option) => (
            <button
              key={option.key}
              type="button"
              className={`seg-control__item${filters.active === option.key ? " seg-control__item--active" : ""}`}
              aria-pressed={filters.active === option.key}
              data-testid={`account-filter-${option.key}`}
              onClick={() => onFilterChange(option.key)}
            >
              {option.label}
              {option.count !== null && option.count > 0 && <span className="seg-control__count">{option.count}</span>}
            </button>
          ))}
        </div>
      )}
      {/* 排序档：只渲染 builder 给本产品的选项（WB 无「签到状态」）。 */}
      <label className="account-center__sort-field">
        <ArrowDownAZ size={14} aria-hidden="true" />
        <span className="sr-only">排序方式</span>
        <select value={sortValue} data-testid="account-sort" onChange={(event) => onSortChange(event.target.value)}>
          {filters.sortOptions.map((option) => (
            <option key={option.key} value={option.key}>{option.label}</option>
          ))}
        </select>
      </label>
    </div>
  );
}
