import type { AccountFilterView, LibraryView } from "../config";

/**
 * 库页列表控制条（P4e-8b-1，从 LibraryPage 抽出，纯展示）：账号过滤下拉 + 软删开关。
 * 旧实现对照 WorkBuddyLibraryPage WB253-288；TRAE 两视图均为 null（无账号过滤、无软删），整条不渲染。
 * 文案、锚点、选项全部取自视图；切换后清选择的口径由调用方（页面）承担，本层只回调新值。
 */
export interface LibraryListControlsProps {
  readonly filter: AccountFilterView | null;
  readonly includeDeleted: LibraryView["includeDeleted"];
  readonly onFilterChange: (value: string) => void;
  readonly onIncludeDeletedChange: (checked: boolean) => void;
}
export function LibraryListControls({ filter, includeDeleted, onFilterChange, onIncludeDeletedChange }: LibraryListControlsProps) {
  if (filter === null && includeDeleted === null) return null;
  return (
    <div className="account-center__list-controls">
      {filter !== null && (
        <label className="account-center__target-field">
          <span>{filter.label}</span>
          <select value={filter.selected ?? ""} onChange={(event) => onFilterChange(event.target.value)} data-testid={filter.testId}>
            <option value="">{filter.allLabel}</option>
            {filter.options.map((option) => <option key={option.id} value={option.id}>{option.label}</option>)}
          </select>
        </label>
      )}
      {includeDeleted !== null && (
        <label className="settings-panel__toggle-row">
          <input type="checkbox" checked={includeDeleted.checked} data-testid={includeDeleted.testId}
            onChange={(event) => onIncludeDeletedChange(event.target.checked)} />
          <span className="account-center__profile-copy"><strong>{includeDeleted.label}</strong><span>{includeDeleted.hint}</span></span>
        </label>
      )}
    </div>
  );
}
