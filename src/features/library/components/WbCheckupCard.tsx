import { LIBRARY_TEXTS, fillLibraryText, type CheckupView } from "../config";

/**
 * WB 库体检卡（P4e-8b-1，从 LibraryPage 抽出）：五项 + 归并候选入口 + 立即体检。
 * WB 无库信息 tab，故本卡落正文（旧 WorkBuddyLibraryPage WB421-481）；文案、锚点、禁用理由全部取自视图。
 * 本层只渲染并回调 onAction，不取数、不判断产品。
 */
export function WbCheckupCard({ view, onAction }: { readonly view: CheckupView; readonly onAction: (actionId: string) => void }) {
  return (
    <div className="workbuddy-card" data-testid={view.testId}>
      <div className="workbuddy-card__title">{view.title}</div>
      {view.items.length > 0 ? (
        <ul className="account-center__checkin-results">
          {view.items.map((item) => (
            <li key={item.id} title={item.hint ?? undefined}><strong>{item.label}</strong><span>{item.value}</span></li>
          ))}
        </ul>
      ) : <p className="account-item__meta">{view.idleNote}</p>}
      <div className="workbuddy-card__actions">
        {view.candidates.map((group) => (
          <button key={group.canonical} className="btn" type="button" onClick={() => onAction(`merge-${group.canonical}`)}
            data-testid={`workbuddy-library-merge-${group.variants.length + 1}`}>
            {fillLibraryText(LIBRARY_TEXTS.wbMergeEntry, { cwd: group.canonical })}
          </button>
        ))}
        <button className="btn" type="button" onClick={() => onAction(view.action.id)}
          disabled={view.action.disabledReason !== null} data-testid={view.action.testId}>
          {view.action.label}
        </button>
      </div>
    </div>
  );
}
