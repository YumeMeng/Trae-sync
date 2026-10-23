import { ListChecks, X } from "lucide-react";
import type { LibraryView } from "../config";
import { LibraryActionButton } from "./LibraryShell";

/**
 * 库页底部选择入口 + 批量栏（P4e-8b-1，纯展示）：左栏底部「选择/取消」（锚 library-select-mode）与批量操作栏
 * （锚取自视图 batchBar.testId：TRAE library-action-bar / WB workbuddy-library-actions）。
 * 旧实现对照 LibrarySessionsPanel.tsx（L）L1671-1702 底部固定区、L1769-1849 G22 浮动栏；WorkBuddyLibraryPage WB393-418 页内动作行。
 * 文案、锚点、禁用理由、动作次序全部取自视图（本层不拼句、不判断产品、不重算计数）；动作只回调 onAction(actionId)。
 * 形态差异（浮动栏 / 页内动作行）由调用方给的 selectMode 决定：G22 选择模式浮出浮动栏，WB 逐会话复选的页内动作行常驻。
 */
export interface LibrarySelectButtonProps {
  readonly selectMode: boolean;
  readonly label: string; // 未选择态文案（视图给「选择」）
  readonly cancelLabel: string; // 选择态文案（视图给「取消」）
  readonly onToggle: () => void;
}
/** 选择入口按钮本体（不带容器）：底部固定区里与「已归档」入口同排时由调用方包一层 lib-foot。 */
export function LibrarySelectButton({ selectMode, label, cancelLabel, onToggle }: LibrarySelectButtonProps) {
  return (
    <button className="btn btn--quiet lib-select-btn" type="button" onClick={onToggle} data-testid="library-select-mode">
      {selectMode ? <X size={15} aria-hidden="true" /> : <ListChecks size={15} aria-hidden="true" />}
      {selectMode ? cancelLabel : label}
    </button>
  );
}

export interface LibraryBatchBarProps {
  /** 视图批量栏（null = 不渲染；TRAE 仅选择模式、WB 常驻）。 */
  readonly bar: LibraryView["batchBar"];
  /** 选择模式：true 用 G22 浮动栏，false 用页内动作行（旧页两形态）。 */
  readonly selectMode: boolean;
  readonly onAction: (actionId: string) => void;
}
/** 批量栏：汇总（有 summary 才渲染 batch-count 锚）+ 动作按钮；动作全部经 LibraryActionButton 渲染禁用理由。 */
export function LibraryBatchBar({ bar, selectMode, onAction }: LibraryBatchBarProps) {
  if (bar === null) return null;
  return (
    <div className={selectMode ? "lib-actionbar" : "checkin-page__action-row"} role="toolbar"
      aria-label={bar.ariaLabel === "" ? undefined : bar.ariaLabel} data-testid={bar.testId}>
      {bar.summary !== "" && <span className="lib-actionbar__count" data-testid="batch-count">{bar.summary}</span>}
      <div className={selectMode ? "lib-actionbar__actions" : undefined}>
        {bar.actions.map((action) => (
          <LibraryActionButton key={action.id} action={action} onClick={() => onAction(action.id)} />
        ))}
      </div>
    </div>
  );
}
