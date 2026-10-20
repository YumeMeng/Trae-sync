import { Fragment, useEffect, useState } from "react";
import type { ReactNode } from "react";
import { UserRound } from "lucide-react";
import type { ProductUiContext } from "../../../platform/productUiRegistry";
import { EmptyState } from "../EmptyState";
import { ProductBadge } from "../ProductBadge";
import { AccountListControls, type AccountListControlsProps } from "./AccountListControls";
import type { SharedAccountRecord } from "./AccountRow";

/**
 * 账号页编排骨架（P19-B1 步骤③，ADR-0036 铁律一/二）：
 * 页头 + 状态区插槽 + 「我的账号」区块（控件行 + [账号行]×N + 空态）+ 详情视图切换 + 弹层插槽。
 * 信息架构模板（IA 草案 §1.2）的产品无关实现：产品差异只经 ProductUiContext
 * 能力开关与插槽进入，禁止 productId 整页分支。
 *
 * 骨架内部保留三处历史重复的实现细节（一次做对）：
 * - active/loading 门控渲染（加载中也保留页头标题，键盘导航焦点一致）；
 * - data-page-title 焦点管理（进入页面后聚焦页标题）；
 * - 选中态（selectedProfileId）与产品切换清态。
 * 契约冻结：stagger 白名单类名（account-list / account-card-grid）、
 * 全部 data-testid、文案与视觉与 AccountCenter 迁移前逐像素一致。
 */
export interface AccountPageSkeletonProps {
  /** 产品 UI 上下文（能力开关 + 展示名 + 徽章）。 */
  product: ProductUiContext;
  /** 页面可见时才渲染内容（后台页保持挂载但不读数据）。 */
  active: boolean;
  /** 数据读取中：渲染门控态（页头 + 读取中空态）。 */
  loading: boolean;
  /** region 无障碍标签（如「TRAE 账号」）。 */
  ariaLabel: string;
  /** 列表条目（产品层已完成排序/过滤与 DTO→SharedAccountRecord 映射）。 */
  entries: readonly SharedAccountRecord[];
  /** 行插槽：产品层决定徽章组合与 meta；ops.onOpen 由骨架注入（进入详情视图）。 */
  renderRow(entry: SharedAccountRecord, ops: { onOpen(): void }): ReactNode;
  /** 页头动作区插槽（产品特有按钮：刷新/健康检测/添加账号等）。 */
  headerActions?: ReactNode;
  /** 页头与主体之间的状态区插槽（登录等待、批量进度等）。 */
  statusRegion?: ReactNode;
  /** 「我的账号」区块计数（section-caption）。 */
  entryCount: number;
  /** 是否渲染过滤/排序/视图切换控件行（无条目时隐藏，与迁移前一致）。 */
  showControls: boolean;
  /** AccountListControls 受控数据（视图/排序/过滤能力开关）。 */
  controls: AccountListControlsProps;
  /** 列表前置插槽（如适配器产品的官方身份块）。 */
  listPrelude?: ReactNode;
  /** 空态（产品层按数据形态给文案与动作；className 保留既有测试选择器）。 */
  emptyState: { description: string; action?: ReactNode; className?: string };
  /** 详情视图插槽：按选中 id 返回详情节点；null = 列表态（如条目已被删除）。
   *  ops.onBack 由骨架注入（返回列表 = 清除选中态）。 */
  detailView(profileId: string, ops: { onBack(): void }): ReactNode | null;
  /** 产品弹层插槽（切号/手机补录/二次确认），列表与详情视图都渲染。 */
  dialogs?: ReactNode;
  /** 选中态清理键（产品身份变化即清空详情选择）。产品层按自身产品指针
   *  语义传值（如 `adapter?.productId ?? "-"`，与迁移前 currentProductId 同源）。 */
  selectionResetKey: string;
  /** 读取失败态（标题 + 说明 + 重试动作）：渲染于页头之下，替代主体。 */
  errorState?: { title: string; description: string; action?: ReactNode };
  /** 行列表容器 testid（如 WB 池清单 workbuddy-pool-list；缺省无）。 */
  listTestId?: string;
}

export function AccountPageSkeleton({
  product,
  active,
  loading,
  ariaLabel,
  entries,
  renderRow,
  headerActions,
  statusRegion,
  entryCount,
  showControls,
  controls,
  listPrelude,
  emptyState,
  detailView,
  dialogs,
  selectionResetKey,
  errorState,
  listTestId,
}: AccountPageSkeletonProps) {
  // 选中态由骨架管理：非空时进入独立详情视图（selectionResetKey 变化即清态，
  // 避免两个产品 profile_id 相同时展示旧产品详情）。
  const [selectedProfileId, setSelectedProfileId] = useState("");
  useEffect(() => {
    setSelectedProfileId("");
  }, [selectionResetKey]);

  // 进入页面后聚焦页标题（键盘导航与切页焦点一致；加载中不抢焦点）。
  useEffect(() => {
    if (!active || loading) return;
    document.querySelector<HTMLElement>('[data-page-title="accounts"]')?.focus({ preventScroll: true });
  }, [active, loading]);

  // 门控渲染：加载中也保留页面标题，保证键盘导航与切页焦点一致。
  if (!active || loading) {
    return (
      <section className="account-center" role="region" aria-label={ariaLabel}>
        <header className="page-header">
          <div className="page-header__copy">
            <h1 data-page-title="accounts" tabIndex={-1}>账号</h1>
            <ProductBadge productId={product.productId} />
          </div>
        </header>
        <EmptyState role="status" className="account-center__empty" description="正在读取账号信息…" />
      </section>
    );
  }

  // 独立详情视图：产品层查得到该账号时展示（删除后数据刷新即回列表）。
  const detail = selectedProfileId
    ? detailView(selectedProfileId, { onBack: () => setSelectedProfileId("") })
    : null;
  if (detail) {
    return (
      <>
        {detail}
        {dialogs}
      </>
    );
  }

  // 读取失败态：页头 + 诚实说明卡（重试动作由产品层注入）。
  if (errorState) {
    return (
      <section className="account-center" role="region" aria-label={ariaLabel}>
        <header className="page-header">
          <div className="page-header__copy">
            <h1 data-page-title="accounts" tabIndex={-1}>账号</h1>
            <ProductBadge productId={product.productId} />
          </div>
          <div className="page-header__actions">{headerActions}</div>
        </header>
        <EmptyState
          role="alert"
          className="account-center__empty"
          title={errorState.title}
          description={errorState.description}
          action={errorState.action}
        />
        {dialogs}
      </section>
    );
  }

  return (
    <section className="account-center" role="region" aria-label={ariaLabel}>
      <header className="page-header">
        <div className="page-header__copy">
          <h1 data-page-title="accounts" tabIndex={-1}>账号</h1>
          <ProductBadge productId={product.productId} />
        </div>
        <div className="page-header__actions">{headerActions}</div>
      </header>

      {statusRegion}

      {/* 我的账号：页面主体。双视图（列表宽行默认 / 卡片网格切换）。 */}
      <section className="account-center__profiles" aria-labelledby="account-mine-heading">
        <div className="account-center__section-heading">
          <h3 id="account-mine-heading"><UserRound size={16} aria-hidden="true" />我的账号</h3>
          {showControls && (
            <div className="account-center__list-controls">
              <AccountListControls {...controls} />
            </div>
          )}
          <span className="section-caption">{entryCount} 个</span>
        </div>
        {listPrelude}
        {entries.length > 0 ? (
          // stagger 白名单类名（App.tsx 依赖 account-list / account-card-grid）。
          <ul className={controls.view === "list" ? "account-list" : "account-card-grid"} data-testid={listTestId}>
            {entries.map((entry) => (
              <Fragment key={entry.id}>
                {renderRow(entry, { onOpen: () => setSelectedProfileId(entry.id) })}
              </Fragment>
            ))}
          </ul>
        ) : (
          <EmptyState
            className={emptyState.className}
            description={emptyState.description}
            action={emptyState.action}
          />
        )}
      </section>

      {dialogs}
    </section>
  );
}
