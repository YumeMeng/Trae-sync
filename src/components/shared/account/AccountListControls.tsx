import { ArrowDownAZ, LayoutGrid, List } from "lucide-react";
import type { AccountView } from "./AccountRow";

// U-2 排序：added=添加序（注册表顺序）；name=名称；checkin=签到状态（未签在前）。
export type AccountSort = "added" | "name" | "checkin";
// G14 账号过滤：all=全部（默认）；attention=需处理（只显示异常账号）。会话内状态，不持久化。
export type AccountFilter = "all" | "attention";

/** 全部排序档（能力开关联动裁剪：产品不含 checkin 口径时去掉「签到状态」）。 */
export const ACCOUNT_SORT_OPTIONS: readonly AccountSort[] = ["added", "name", "checkin"];

const SORT_LABELS: Record<AccountSort, string> = {
  added: "添加顺序",
  name: "名称",
  checkin: "签到状态",
};

/**
 * 账号页页头控件组（P19-B1② 从 AccountCenter L1108-1173 抽取）：
 * 过滤分段控件（能力开关）+ 排序下拉 + 视图切换分段控件。
 * 产品差异只通过 sortOptions / filter 是否传入进入，布局与交互全产品一致；
 * testid 契约冻结（account-filter-* / account-sort / account-view-*）。
 */
export interface AccountListControlsProps {
  view: AccountView;
  onViewChange(next: AccountView): void;
  sort: AccountSort;
  /** 能力开关：本产品支持的排序档（WB 无签到口径时不出 "checkin"）。 */
  sortOptions: readonly AccountSort[];
  onSortChange(next: AccountSort): void;
  /** 有 attention 过滤能力才渲染（G14 需处理过滤的产品化入口）。 */
  filter?: {
    value: AccountFilter;
    attentionCount: number;
    onChange(next: AccountFilter): void;
  };
}

export function AccountListControls({
  view,
  onViewChange,
  sort,
  sortOptions,
  onSortChange,
  filter,
}: AccountListControlsProps) {
  // 存储偏好可能残留本产品已关闭的档位：回退第一档，select 不出现无效值。
  const sortValue = sortOptions.includes(sort) ? sort : sortOptions[0];
  return (
    <>
      {filter && (
        <div className="seg-control" role="group" aria-label="账号过滤">
          <button
            type="button"
            className={`seg-control__item${filter.value === "all" ? " seg-control__item--active" : ""}`}
            aria-pressed={filter.value === "all"}
            onClick={() => filter?.onChange("all")}
            data-testid="account-filter-all"
            title="显示全部账号"
          >
            全部
          </button>
          <button
            type="button"
            className={`seg-control__item${filter.value === "attention" ? " seg-control__item--active" : ""}`}
            aria-pressed={filter.value === "attention"}
            onClick={() => filter?.onChange("attention")}
            data-testid="account-filter-attention"
            title="只显示需要处理的账号（需重登、已过期、待登录或签到失败）"
          >
            需处理
            {filter.attentionCount > 0 && <span className="seg-control__count">{filter.attentionCount}</span>}
          </button>
        </div>
      )}
      {/* 排序：添加序 / 名称 / 签到状态（未签在前，补签场景优先）。 */}
      <label className="account-center__sort-field">
        <ArrowDownAZ size={14} aria-hidden="true" />
        <span className="sr-only">排序方式</span>
        <select
          value={sortValue}
          onChange={(event) => onSortChange(event.target.value as AccountSort)}
          data-testid="account-sort"
        >
          {sortOptions.map((option) => (
            <option key={option} value={option}>{SORT_LABELS[option]}</option>
          ))}
        </select>
      </label>
      {/* 视图切换分段控件：列表 / 卡片（记忆 + 平滑过渡）。 */}
      <div className="seg-control" role="group" aria-label="视图切换">
        <button
          type="button"
          className={`seg-control__item${view === "list" ? " seg-control__item--active" : ""}`}
          aria-pressed={view === "list"}
          onClick={() => onViewChange("list")}
          data-testid="account-view-list"
          title="列表视图：横向宽行，扫描对比最快"
        >
          <List size={14} aria-hidden="true" />列表
        </button>
        <button
          type="button"
          className={`seg-control__item${view === "card" ? " seg-control__item--active" : ""}`}
          aria-pressed={view === "card"}
          onClick={() => onViewChange("card")}
          data-testid="account-view-card"
          title="卡片视图：网格布局，信息分层展示"
        >
          <LayoutGrid size={14} aria-hidden="true" />卡片
        </button>
      </div>
    </>
  );
}
