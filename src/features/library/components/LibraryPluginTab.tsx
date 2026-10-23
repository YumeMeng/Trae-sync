import type { ReactNode } from "react";
import { Package, Puzzle, Search, Store, Trash2 } from "lucide-react";
import { ConfirmDialog } from "../../../components/shared/ConfirmDialog";
import { EmptyState } from "../../../components/shared/EmptyState";
import { Badge, type BadgeTone } from "../../../ui/Badge";
import { LIBRARY_TEXTS, type LibraryTone, type PluginRowView, type PluginSegmentId, type PluginTabView } from "../config";
import { LibraryActionButton } from "./LibraryShell";

/**
 * 插件 tab（P4e-7c，纯展示）：PluginTabView → 旧 PluginWorkbench 的表格化骨架（P=src/components/PluginWorkbench.tsx）。
 * 段切换 / 搜索 / 分类 chips / 行列表 / 空态 / 卸载确认全部由视图字段驱动；文案与 testid 取自视图或 LIBRARY_TEXTS。
 * 视图未建模的三处旧约定（分类 chips 容器锚、段/列表/空态锚、市场已安装徽章锚）按旧页命名规则拼接，不发明新锚。
 * 本文件零 useState / useEffect / invoke，交互只回调（安装/卸载的编排归 usePluginActions + 页面弹层）。
 */
/** 旧页容器锚（P385）与列表/空态锚（P408 / P421 / P426）的命名规则。 */
const CHIPS_TEST_ID = "plugin-category-chips";
const listTestId = (segment: PluginSegmentId): string => `plugin-${segment}-list`;
const emptyTestId = (segment: PluginSegmentId): string => `plugin-${segment}-empty`;
/** 市场行「已安装」徽章锚（P280）：旧页按 plugin_id 派生；行锚后缀优先，缺省回落 row.id。 */
const MARKET_ROW_PREFIX = "plugin-market-row-";
const installedBadgeTestId = (row: PluginRowView): string =>
  `plugin-market-installed-${row.testId.startsWith(MARKET_ROW_PREFIX) ? row.testId.slice(MARKET_ROW_PREFIX.length) : row.id}`;
/** 徽章色档：LibraryTone → Badge 四态（ok=正常态一律中性，UI-DIRECTION §4）。 */
const BADGE_TONE: Record<LibraryTone, BadgeTone> = { neutral: "neutral", ok: "neutral", warn: "warn", danger: "danger", accent: "accent" };
/** 分段图标（P344 / P354）：按段 id 选旧页同款图标，不改文案。 */
const SEGMENT_ICON: Record<PluginSegmentId, ReactNode> = {
  installed: <Package size={15} aria-hidden="true" />, market: <Store size={15} aria-hidden="true" />,
};

/** 单行（已装/市场共用，P41-80）：图标 | 名称+元信息 | 分类 | 徽章 + 动作。 */
function PluginRow({ row, segment, busy, onRowAction }: {
  row: PluginRowView; segment: PluginSegmentId; busy: boolean; onRowAction: (row: PluginRowView) => void;
}) {
  const action = row.action, installed = segment === "installed";
  return (
    <div className="plugin-row" data-testid={row.testId} title={row.hoverTitle}>
      <span className="plugin-row__icon" aria-hidden="true"><Puzzle size={16} /></span>
      <div className="plugin-row__main">
        <span className="plugin-row__name">{row.title}</span>
        {row.meta !== null && <span className="plugin-row__meta">{row.meta}</span>}
      </div>
      <span className="plugin-row__category">{row.category ?? ""}</span>
      <div className="plugin-row__side">
        {row.badge !== null && (
          <Badge tone={BADGE_TONE[row.badge.tone]} hint={row.badge.title ?? undefined}
            testId={installed ? undefined : installedBadgeTestId(row)}>{row.badge.text}</Badge>
        )}
        {action !== null && (
          // 已装段 = 移除（描边安静档 + 垃圾桶，P245-255）；市场段 = 安装（P287-295）。
          <button className={installed ? "btn btn--quiet" : "btn"} type="button" onClick={() => onRowAction(row)}
            disabled={busy || action.disabledReason !== null} title={action.disabledReason ?? action.hint ?? undefined}
            data-testid={action.testId}>
            {installed && <Trash2 size={14} aria-hidden="true" />}{action.label}
          </button>
        )}
      </div>
    </div>
  );
}

export interface LibraryPluginTabProps {
  /** 插件 tab 视图（builder 产出；null 由页面决定不渲染，本层不收 null）。 */
  readonly view: PluginTabView;
  /** 有安装/卸载在执行：全部行动作禁用（旧 P98 busyKey !== null 口径）。 */
  readonly busy?: boolean;
  readonly onSegmentChange: (segment: PluginSegmentId) => void;
  readonly onSearch: (value: string) => void;
  readonly onCategoryChange: (category: string | null) => void;
  /** 工具行动作出口（plugin-refresh）。 */
  readonly onAction: (actionId: string) => void;
  /** 行动作出口（安装=直接执行 / 卸载=打开确认，由页面按 action 判定）。 */
  readonly onRowAction: (row: PluginRowView) => void;
  readonly onUninstallCancel: () => void;
  readonly onUninstallConfirm: () => void;
}

/** 插件工作台：工具行（段 + 搜索 + 刷新）→ 分类 chips → 行列表/空态 → 卸载确认。 */
export function LibraryPluginTab(props: LibraryPluginTabProps) {
  const { view, busy = false, onSegmentChange, onSearch, onCategoryChange, onAction, onRowAction, onUninstallCancel, onUninstallConfirm } = props;
  const search = view.search;
  return (
    <section className="plugin-workbench" role="region" aria-label={view.ariaLabel} data-testid={view.testId}>
      <div className="plugin-workbench__bar">
        <div className="plugin-segment" role="tablist" aria-label={LIBRARY_TEXTS.pluginSegmentsAria}>
          {view.segments.map((segment) => (
            <button key={segment.id} type="button" role="tab" aria-selected={segment.active}
              className={`plugin-segment__btn${segment.active ? " plugin-segment__btn--active" : ""}`}
              onClick={() => onSegmentChange(segment.id)} data-testid={segment.testId}>
              {SEGMENT_ICON[segment.id]}{segment.label}
            </button>
          ))}
        </div>
        {search !== null && (
          <div className="plugin-search">
            <Search size={14} aria-hidden="true" />
            <input type="search" value={search.value} onChange={(event) => onSearch(event.target.value)}
              placeholder={search.placeholder} aria-label={LIBRARY_TEXTS.pluginSearch} data-testid={search.testId} />
          </div>
        )}
        <LibraryActionButton action={view.refresh} onClick={() => onAction(view.refresh.id)} />
      </div>

      {/* 分类 chips（P384-406）：单分类无可筛即整条不渲染（旧页 categories.length > 1 守卫）。 */}
      {view.categories.length > 1 && (
        <div className="plugin-chips" role="group" aria-label={LIBRARY_TEXTS.pluginChipsAria} data-testid={CHIPS_TEST_ID}>
          <button type="button" className={`plugin-chip${view.activeCategory === null ? " plugin-chip--active" : ""}`}
            onClick={() => onCategoryChange(null)} data-testid="plugin-chip-all">{LIBRARY_TEXTS.pluginChipAll}</button>
          {view.categories.map((name) => (
            <button key={name} type="button" className={`plugin-chip${view.activeCategory === name ? " plugin-chip--active" : ""}`}
              onClick={() => onCategoryChange(name)} data-testid={`plugin-chip-${name}`}>{name}</button>
          ))}
        </div>
      )}

      <div className="plugin-list" data-testid={listTestId(view.activeSegment)}>
        {view.errorNote !== null && <p className="workbench__error" role="alert">{view.errorNote}</p>}
        {view.rows.map((row) => (
          <PluginRow key={row.id} row={row} segment={view.activeSegment} busy={busy} onRowAction={onRowAction} />
        ))}
        {/* 空态 / 加载句同源：视图模型无独立 loading 字段，由 builder 决定 emptyText（本层不发明文案）。 */}
        {view.emptyText !== null && (
          <EmptyState className="plugin-empty" testId={emptyTestId(view.activeSegment)} description={view.emptyText} />
        )}
      </div>

      {/* 卸载确认（P447-460，ADR-0026 决策 2：单次确认 + 全账号传播；标题/正文行/锚点由 builder 给）。 */}
      {view.uninstall !== null && (
        <ConfirmDialog title={view.uninstall.title} lines={view.uninstall.lines} confirmLabel={view.uninstall.confirmLabel}
          confirmIcon={<Trash2 size={15} aria-hidden="true" />} danger onCancel={onUninstallCancel}
          onConfirm={onUninstallConfirm} testId={view.uninstall.testId} />
      )}
    </section>
  );
}
