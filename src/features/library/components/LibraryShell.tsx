import type { ReactNode } from "react";
import { Archive, ArrowLeft, Merge, RefreshCw, Trash2, Undo2 } from "lucide-react";
import { ProductBadge } from "../../../components/shared/ProductBadge";
import type { LibraryActionKind, LibraryActionView, LibraryTabId, LibraryView } from "../config";

/**
 * 库页壳（P4e-7a，纯展示）：region + 页头（返回 / 标题 / 只读徽章 / 产品徽章 / 动作槽）+ tab 条 + 内容槽。
 * 标题、region 名、data-page-title、tab 文案与全部 testid 取自 builder 的 LibraryView
 *（旧 D138-207 / WB183-201）；本文件不自创文案、不判断产品、不读数据、不发命令
 *（动作只回调 onAction(actionId)，tab 只回调 onTabChange(tabId)）。
 */

/** 动作样式档：与页头/账号页同一套（primary=主按钮 / ghost=描边 / danger=红色描边）。 */
const KIND_CLASS: Record<LibraryActionKind, string> = { primary: "btn btn--primary", ghost: "btn", danger: "btn btn--danger" };
/** 动作图标：按动作 id 的动词段取旧页同款图标（本层只选图，不改文案）。 */
const ICONS: readonly (readonly [string, ReactNode])[] = [
  ["-refresh", <RefreshCw size={15} aria-hidden="true" />], ["-back", <ArrowLeft size={15} aria-hidden="true" />],
  ["-restore", <Undo2 size={15} aria-hidden="true" />], ["-archive", <Archive size={15} aria-hidden="true" />],
  ["-merge", <Merge size={15} aria-hidden="true" />], ["-delete", <Trash2 size={15} aria-hidden="true" />],
];
const actionIcon = (id: string): ReactNode => ICONS.find(([key]) => id.endsWith(key))?.[1] ?? null;

/** 单枚动作按钮（页头 / tab 外动作 / 批量栏共用）：disabledReason 非空即禁用并给悬浮说明。 */
export function LibraryActionButton({ action, busy = false, onClick }: {
  action: LibraryActionView; busy?: boolean; onClick: () => void;
}) {
  return (
    <button className={KIND_CLASS[action.kind]} type="button" onClick={onClick}
      disabled={busy || action.disabledReason !== null}
      title={action.disabledReason ?? action.hint ?? undefined}
      aria-busy={busy || undefined} data-testid={action.testId}>
      {busy ? <RefreshCw size={15} className="icon-spin" aria-hidden="true" /> : actionIcon(action.id)}
      {busy ? action.busyLabel ?? action.label : action.label}
    </button>
  );
}

export interface LibraryShellProps {
  /** 页面唯一渲染输入（builder 产出）。 */
  readonly view: LibraryView;
  /**
   * tab 条读屏名称（视图模型未建模；页面按库形态注入 LIBRARY_TEXTS.detailTabsAriaMaster /
   * detailTabsAriaSecondary；缺省不写该属性，不猜「主库/副库」）。
   */
  readonly tabsAriaLabel?: string;
  /**
   * 页面容器类名（缺省 = 详情页骨架 master-detail；WB 由页面注入其域内类名 workbuddy-page）。
   * 本组件不判断产品，只按调用方给的类名渲染。
   */
  readonly className?: string;
  /** 页头额外动作槽（刷新之外，由页面注入）。 */
  readonly headerExtra?: ReactNode;
  /** 执行中的动作 id（页面按忙碌位注入；缺省=无动作执行中）。 */
  readonly busy?: readonly string[];
  /** 页头动作出口（返回 / 刷新）。 */
  readonly onAction: (actionId: string) => void;
  /** tab 切换出口（sessions / plugins / info）。 */
  readonly onTabChange: (tabId: LibraryTabId) => void;
  /** 内容槽：tab 面板（会话列表 / 插件 / 库信息由装配层填充）。 */
  readonly children: ReactNode;
  /**
   * 浮层槽（归档草稿条 / 删除与归并确认层）：渲染在 region **内部**（旧页弹层与草稿条
   * 同样落在 region 子树里，e2e 按 region 作用域取锚点）。定位仍由各自 CSS 的 fixed 承担。
   */
  readonly overlays?: ReactNode;
}

/** 页面外框：region（aria-label 取 header.regionLabel）+ 页头 + tab 条 + 内容槽。 */
export function LibraryShell({ view, tabsAriaLabel, className = "master-detail", headerExtra, busy = [], onAction, onTabChange, children, overlays }: LibraryShellProps) {
  const header = view.header, back = header.back, refresh = header.refresh;
  return (
    <section className={className} role="region" aria-label={header.regionLabel}>
      <header className="page-header">
        <div className="page-header__copy">
          <div className="master-detail__title-row">
            {back !== null && (
              <LibraryActionButton action={back} busy={busy.includes(back.id)} onClick={() => onAction(back.id)} />
            )}
            {/* h1 的 data-page-title 取视图字段（= 能力表 pageRegion "master-library"）：App 按它移焦。 */}
            <h1 data-page-title={header.pageTitleAttr} tabIndex={-1}>{header.title}</h1>
            {header.readonlyBadge !== null && <span className="env-card__badge">{header.readonlyBadge}</span>}
            {header.badgeProductId !== null && <ProductBadge productId={header.badgeProductId} />}
          </div>
        </div>
        {(refresh !== null || headerExtra !== undefined) && (
          <div className="page-header__actions">
            {refresh !== null && (
              <LibraryActionButton action={refresh} busy={busy.includes(refresh.id)} onClick={() => onAction(refresh.id)} />
            )}
            {headerExtra}
          </div>
        )}
      </header>
      {/* tab 条：tab 文案与锚点（master-tab-*）全部来自视图；无 tab 的产品（WB）整条不渲染。 */}
      {view.tabs.length > 0 && (
        <div className="master-detail__tabs" role="tablist" aria-label={tabsAriaLabel}>
          {view.tabs.map((tab) => (
            <button key={tab.id} type="button" role="tab" aria-selected={tab.active}
              className={`master-tab${tab.active ? " master-tab--active" : ""}`}
              onClick={() => onTabChange(tab.id)} data-testid={tab.testId}>{tab.label}</button>
          ))}
        </div>
      )}
      {/* 有 tab 的库（详情页三 tab）内容包一层 tab 面板容器撑满剩余高度；无 tab 的库直接落正文。 */}
      {view.tabs.length > 0 ? <div className="master-detail__tabpanels">{children}</div> : children}
      {/* 浮层槽：草稿条与确认层（fixed 定位，不占正文流）。 */}
      {overlays}
    </section>
  );
}
