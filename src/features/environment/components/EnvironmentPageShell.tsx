import type { ReactNode } from "react";
import { Layers } from "lucide-react";
import { EmptyState } from "../../../components/shared/EmptyState";
import type { TraeInstanceLoginState } from "../../../types/account_switch";
import type { EnvCardView, EnvironmentView } from "../config";
import { EnvActionButton, EnvCard, EnvCardList, type EnvCardAnchors } from "./EnvironmentCards";

/**
 * 环境页壳（P4d-7a，纯展示）：页头（标题 + region 名 + 动作槽）+ 实例卡列表 + 辅助环境区空态。
 * 标题/region/动作文案与 testid 全部来自 builder 的 EnvironmentView（旧页 L414-430 / L700-759、WB L79-97 / L199-205）；
 * 本文件不自创文案、不判断 productId、不读数据、不发命令（动作只回调 onAction(actionId)）。
 */

/**
 * 辅助环境空态锚（旧页 L715 逐字）：视图模型只给区块锚 env-secondary-section，
 * 故此处以常量保留该锚点；P4d-8 若给 EnvSecondarySectionView 补 emptyTestId，可换为视图字段。
 */
const SECONDARY_EMPTY_TEST_ID = "env-placeholder";

export interface EnvironmentPageShellProps {
  /** 页面唯一渲染输入（builder 产出）。 */
  view: EnvironmentView;
  /** 页头产品徽标槽（页面注入 <ProductBadge productId=… />）。 */
  productBadge?: ReactNode;
  /** 页头额外动作槽（刷新之外的入口；刷新按钮由视图 header.refresh 驱动）。 */
  headerExtra?: ReactNode;
  /** 执行中的动作 id（页面按忙碌位注入；缺省=无动作执行中）。 */
  busy?: readonly string[];
  /** 主库卡内区块槽（体检 / 捕获，由第二组组件填充）。 */
  masterExtra?: ReactNode;
  /** 主库卡后置区块槽（如 WB 登录域健康卡）。 */
  afterMaster?: ReactNode;
  /** 主库卡内部锚覆盖（WB 客户端运行状态徽章）。 */
  masterAnchors?: EnvCardAnchors;
  /** 主库实例登录子态（视图模型不含，由页面从状态快照透传）。 */
  masterLoginState?: TraeInstanceLoginState;
  /** 副环境卡内区块槽（逐库体检）。 */
  secondaryCardExtra?: (card: EnvCardView) => ReactNode;
  /** 副环境实例登录子态（页面按 env_id 取）。 */
  secondaryLoginStateOf?: (card: EnvCardView) => TraeInstanceLoginState | undefined;
  onAction: (actionId: string) => void;
}

/** 页头：h1 带 data-page-title 供导航移焦；产品徽标与动作槽由页面注入。 */
function PageHeader({ view, productBadge, headerExtra, busy, onAction }: {
  view: EnvironmentView; productBadge?: ReactNode; headerExtra?: ReactNode;
  busy: readonly string[]; onAction: (actionId: string) => void;
}) {
  const refresh = view.header.refresh;
  return (
    <header className="page-header">
      <div className="page-header__copy">
        <h1 data-page-title={view.header.pageTitleAttr} tabIndex={-1}>{view.header.title}</h1>
        {productBadge}
      </div>
      {(refresh !== null || headerExtra !== undefined) && (
        <div className="page-header__actions">
          {refresh !== null && (
            <EnvActionButton action={refresh} busy={busy.includes(refresh.id)} onClick={() => onAction(refresh.id)} />
          )}
          {headerExtra}
        </div>
      )}
    </header>
  );
}

/** 辅助环境区：标题 + 创建入口 +（pending 说明 / 空态 / 卡列表三选一）。 */
function SecondarySection({ section, busy, secondaryCardExtra, secondaryLoginStateOf, onAction }: {
  section: NonNullable<EnvironmentView["secondary"]>; busy: readonly string[];
  secondaryCardExtra?: (card: EnvCardView) => ReactNode;
  secondaryLoginStateOf?: (card: EnvCardView) => TraeInstanceLoginState | undefined;
  onAction: (actionId: string) => void;
}) {
  const { createAction, cards, pendingNote, emptyTitle, emptyText } = section;
  return (
    <div className="env-section" data-testid={section.testId}>
      <div className="env-section__head">
        <h2 className="env-section__title">{section.title}</h2>
        <EnvActionButton action={createAction} busy={busy.includes(createAction.id)} onClick={() => onAction(createAction.id)} />
      </div>
      {/* pending 档（WB 多实例待实验票）：说明代替列表，创建入口由 disabledReason 置灰。 */}
      {pendingNote !== null && (
        <EmptyState panel title={section.title} description={pendingNote} />
      )}
      {/* 空态只在列表已读且为空时出（builder 用 emptyTitle=null 表达「不渲染」）。 */}
      {emptyTitle !== null && (
        <div className="env-placeholder" data-testid={SECONDARY_EMPTY_TEST_ID}>
          <span className="env-placeholder__icon" aria-hidden="true"><Layers size={20} strokeWidth={1.8} /></span>
          <div>
            <div className="env-placeholder__title">{emptyTitle}</div>
            {emptyText !== "" && <p className="env-placeholder__text">{emptyText}</p>}
          </div>
        </div>
      )}
      {cards.length > 0 && (
        <EnvCardList cards={cards} busy={busy} cardExtra={secondaryCardExtra} loginStateOf={secondaryLoginStateOf}
          onAction={(_cardId: string, actionId: string) => onAction(actionId)} />
      )}
    </div>
  );
}

/** 环境页壳：页头 + 主库卡（含引导句与区块槽）+ 辅助环境区。 */
export function EnvironmentPageShell({
  view, productBadge, headerExtra, busy = [], masterExtra, afterMaster, masterAnchors, masterLoginState,
  secondaryCardExtra, secondaryLoginStateOf, onAction,
}: EnvironmentPageShellProps) {
  return (
    <section className="environment-page" role="region" aria-label={view.header.regionLabel}>
      <PageHeader view={view} productBadge={productBadge} headerExtra={headerExtra} busy={busy} onAction={onAction} />
      <div className="environment-page__body">
        {view.master !== null && (
          <EnvCard card={view.master} hint={view.masterHint} busy={busy} anchors={masterAnchors} loginState={masterLoginState}
            onAction={(_cardId: string, actionId: string) => onAction(actionId)}>
            {masterExtra}
          </EnvCard>
        )}
        {afterMaster}
        {view.secondary !== null && (
          <SecondarySection section={view.secondary} busy={busy} secondaryCardExtra={secondaryCardExtra}
            secondaryLoginStateOf={secondaryLoginStateOf} onAction={onAction} />
        )}
      </div>
    </section>
  );
}
