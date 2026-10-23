import type { LibraryStatsView } from "../config";

/**
 * 库页统计卡（P4e-8b-1，从 LibraryPage 抽出，纯展示）：五项数值 + 悬浮说明。
 * 旧实现对照 WorkBuddyLibraryPage WB220-247；TRAE 的 stats 为 null（计数条在库信息 tab），故该卡只在视图给值时渲染。
 * 文案与锚点全部取自视图（本层不拼句、不判断产品、不重算计数）。
 */
export function LibraryStatsCard({ view }: { readonly view: LibraryStatsView }) {
  return (
    <div className="workbuddy-card" data-testid={view.testId}>
      <div className="workbuddy-card__title">{view.title}</div>
      <div className="overview-page__stats">
        {view.items.map((item) => (
          <div className="overview-stat" key={item.id} title={item.hint ?? undefined}>
            <span className="overview-stat__value">{item.value}</span><span className="overview-stat__label">{item.label}</span>
          </div>
        ))}
      </div>
    </div>
  );
}
