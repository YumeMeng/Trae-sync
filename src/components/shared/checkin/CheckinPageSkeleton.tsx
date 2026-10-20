import type { ReactNode } from "react";
import { CalendarCheck } from "lucide-react";
import type { ProductUiContext } from "../../../platform/productUiRegistry";
import { EmptyState } from "../EmptyState";
import { ProductBadge } from "../ProductBadge";

/**
 * 签到页编排骨架（P19-B2④，ADR-0036 铁律一/二；架构草案 §1.2D）：
 * 门控渲染 + 页头（标题/产品徽章/动作区）+ 状态区插槽 + 签到账号区块
 * （选择计数 + 批量操作行 + 总进度条 + 行列表 + 自动签到结果）或
 * 单账号内容插槽 + 空态。
 * 信息架构模板（IA 草案 §1.3）的产品无关实现：产品差异只经 ProductUiContext
 * 能力开关与插槽进入，禁止 productId 整页分支。
 * 契约冻结：stagger 白名单类名（checkin-flow）、全部 data-testid、文案。
 */
export interface CheckinPageSkeletonProps {
  product: ProductUiContext;
  /** 页面可见时才渲染主体（后台页保持页头，不发数据读取）。 */
  active: boolean;
  /** 数据读取中：渲染门控态。 */
  loading: boolean;
  /** region 无障碍标签（缺省「签到」）。 */
  ariaLabel?: string;
  /** 页头动作区插槽（刷新等）。 */
  headerActions?: ReactNode;
  /** 页头与主体之间的状态区插槽（自动签到状态行/演示横幅/不可用提示）。 */
  statusRegion?: ReactNode;
  /** 无账号空态（缺省 = 不渲染主体）。 */
  emptyState?: { description: string; action?: ReactNode; className?: string };
  /** —— 签到账号区块（batch-rows 形态；entries > 0 时渲染）—— */
  /** 池总数（caption「已选 X/Y」的 Y；0 = 不渲染区块，走 emptyState/content）。 */
  entryCount?: number;
  /** 已选数（caption 的 X；undefined = caption 只显示计数场景由产品自理）。 */
  selectedCount?: number;
  /** 区块标题（缺省「签到账号」）。 */
  selectionHeading?: string;
  /** 批量操作行插槽（CheckinActionsRow 或产品自定义）。 */
  actions?: ReactNode;
  /** 总进度条（串行批次 X/Y + 说明文案）。 */
  progress?: { completed: number; total: number } | null;
  /** 行节点列表（行组件来自 shared/checkin；key 由产品层设置）。 */
  rows?: readonly ReactNode[];
  /** 列表容器形态：flow=演进式单列（checkin-flow）；plain=简单选择列表。 */
  listVariant?: "flow" | "plain";
  /** 区块尾部插槽（自动签到结果清单等）。 */
  footer?: ReactNode;
  /** —— 单账号内容插槽（single 形态，如 WB 单账号卡）—— */
  content?: ReactNode;
}

export function CheckinPageSkeleton({
  product,
  active,
  loading,
  ariaLabel = "签到",
  headerActions,
  statusRegion,
  emptyState,
  entryCount = 0,
  selectedCount,
  selectionHeading = "签到账号",
  actions,
  progress,
  rows,
  listVariant = "flow",
  footer,
  content,
}: CheckinPageSkeletonProps) {
  // 门控渲染：加载中也保留页面标题，保证键盘导航与切页焦点一致（动作区不渲染）。
  if (!active || loading) {
    return (
      <section className="checkin-page" role="region" aria-label={ariaLabel}>
        <header className="page-header">
          <div className="page-header__copy">
            <h1 data-page-title="checkin" tabIndex={-1}>签到</h1>
            <ProductBadge productId={product.productId} />
          </div>
        </header>
        <EmptyState role="status" className="checkin-page__empty" description="正在读取签到信息…" />
      </section>
    );
  }

  const hasEntries = entryCount > 0;

  return (
    <section className="checkin-page" role="region" aria-label={ariaLabel}>
      <header className="page-header">
        <div className="page-header__copy">
          <h1 data-page-title="checkin" tabIndex={-1}>签到</h1>
          <ProductBadge productId={product.productId} />
        </div>
        {headerActions}
      </header>

      {statusRegion}

      {/* 单账号内容形态（single）：产品层自带卡片；提供 content 即不走批量区块。 */}
      {content}

      {content === undefined && hasEntries && rows !== undefined && (
        <section className="checkin-page__selection" aria-labelledby="checkin-selection-heading">
          <div className="account-center__section-heading">
            <h3 id="checkin-selection-heading"><CalendarCheck size={16} aria-hidden="true" />{selectionHeading}</h3>
            <span className="section-caption">
              {selectedCount !== undefined ? `已选 ${selectedCount}/${entryCount}` : `${entryCount} 个`}
            </span>
          </div>

          {actions}

          {/* U-3 总进度条：串行批次期间 X/Y + 已完成比例。 */}
          {progress && progress.total > 0 && (
            <div className="checkin-progress" role="status" data-testid="checkin-progress">
              <div className="checkin-progress__track">
                <div
                  className="checkin-progress__fill"
                  style={{ width: `${Math.round((progress.completed / progress.total) * 100)}%` }}
                />
              </div>
              <span className="checkin-progress__label">
                {progress.completed}/{progress.total} · 每账号走 状态 → 领取 → 复核，账号间自动间隔 3-8 秒
              </span>
            </div>
          )}

          {/* stagger 白名单类名（App.tsx 依赖 checkin-flow）。 */}
          {listVariant === "flow" ? (
            <ul className="checkin-flow" aria-label="签到账号与结果">{rows}</ul>
          ) : (
            <ul className="account-center__profile-list">{rows}</ul>
          )}

          {footer}
        </section>
      )}

      {content === undefined && !hasEntries && emptyState && (
        <EmptyState
          className={emptyState.className}
          description={emptyState.description}
          action={emptyState.action}
        />
      )}
    </section>
  );
}
