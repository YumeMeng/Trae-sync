import { Fragment, type ReactNode } from "react";
import { RefreshCw } from "lucide-react";
import { SharedAccountEntry, type AccountView } from "../../../components/shared/account/AccountRow";
import { Badge } from "../../../ui/Badge";
import { actionBusyLabelOf, type AccountActionKind, type AccountActionView, type AccountRowView } from "../config";

/** 账号行列表（P4c-6a，纯展示）：视图行 → SharedAccountEntry 的适配层。行骨架、点击进详情、动作按钮
 * 复用共享行组件（不重写行组件），本文件只把 AccountRowView 的显示名/副标题/徽章/指标/动作翻译进槽位；
 * testid = row.testId 与 action.id（来自 builder，本文件不拼）。 */
export type AccountsRowView = AccountRowView & {
  readonly testId: string; // 行锚点（*RowTestIds 的 row，逐字沿用旧 testid）
  /** 「使用中」锚点 id（*RowTestIds 的 selected）：只承载旧 testid，视觉由 builder 的 accent 徽章表达。 */
  readonly selectedTestId?: string;
};
/** 动作样式档：primary=主按钮 / ghost=描边 / danger=红色描边（与页头同一套）。 */
const KIND_CLASS: Record<AccountActionKind, string> = { primary: "btn btn--primary", ghost: "btn", danger: "btn btn--danger" };

/** 徽章槽：四态 tone 与 Badge 同名直接透传（正常态一律中性，不提供成功绿）。 */
const badgeSlot = (row: AccountsRowView): ReactNode => row.badges.map((badge, index) => (
  <Badge key={`${badge.text}-${index}`} tone={badge.tone}>{badge.text}</Badge>
));
/** 「使用中」锚点文案：取 builder 最后一个 accent 徽章的词（「使用中」/「工具内已选」），与视觉同源。 */
const currentLabel = (row: AccountsRowView): string =>
  [...row.badges].reverse().find((badge) => badge.tone === "accent")?.text ?? "使用中";
/** 「使用中」不可见锚点：视觉零变化（hidden 不参与布局），只承载旧 e2e/单测依赖的 testid。 */
const currentAnchor = (row: AccountsRowView): ReactNode => row.selectedTestId === undefined ? null
  : <span hidden data-testid={row.selectedTestId}>{currentLabel(row)}</span>;
/** 副标题槽：空串 = 本产品不渲染该行（诚实缺席，不占位不臆测）。 */
const metaSlot = (row: AccountsRowView, variant: AccountView): ReactNode => row.subtitle === ""
  ? null
  : <p className={variant === "list" ? "account-item__meta" : "account-card__meta"}>{row.subtitle}</p>;
/** 指标格：复用既有侧栏/卡片积分块类名（warn 用既有琥珀标记类，不新增 CSS）；空指标不占位。 */
const metricSlot = (row: AccountsRowView, variant: AccountView): ReactNode => row.metrics.length === 0 ? null : (
  <div className={variant === "list" ? "account-item__credits" : "account-card__credits"}>
    {row.metrics.map((metric) => (
      <Fragment key={metric.label}>
        {variant === "list"
          ? <strong className={metric.tone === "warn" ? "account-item__credits-failed" : undefined}>{metric.value}</strong>
          : <span className={metric.tone === "warn" ? "account-card__credits-failed" : "account-card__credits-value"}>{metric.value}</span>}
        <span className="account-item__credits-label">{metric.label}</span>
      </Fragment>
    ))}
  </div>
);

/** 共享行组件的动作槽只收一个动作且类名固定 primary，故「首个且 primary」交给它，其余动作（含非
 * primary 的首个）在本适配层按 kind 渲染——不丢动作、也不改共享组件类名契约（两 builder 当前均 primary）。 */
const sideActions = (
  row: AccountsRowView,
  actions: readonly AccountActionView[],
  onRowAction: AccountsRowsProps["onRowAction"],
  busy: { readonly actionId: string | null; readonly rowId: string | null },
): ReactNode => actions.map((action) => {
  // 本行该动作执行中：转圈 + 换忙碌文案（旧 actionBusyLabel 口径），文案覆盖由 config 表给。
  const running = busy.rowId === row.id && busy.actionId === action.id;
  const busyLabel = running ? actionBusyLabelOf(action.id) : null;
  return (
    <button
      key={action.id}
      className={KIND_CLASS[action.kind]}
      type="button"
      disabled={action.disabled !== undefined || running}
      title={action.disabled?.reason}
      aria-busy={running || undefined}
      data-testid={action.id}
      onClick={(event) => {
        event.stopPropagation();
        onRowAction(row.id, action.id);
      }}
    >
      {running && <RefreshCw size={14} className="icon-spin" aria-hidden="true" />}
      {busyLabel ?? action.label}
    </button>
  );
});

export interface AccountsRowsProps {
  rows: readonly AccountsRowView[];
  /** 列表形态：list=横向宽行（默认）/ card=卡片网格。 */
  view?: AccountView;
  /** 列表容器 testid（如旧 workbuddy-pool-list）；缺省不设。 */
  listTestId?: string;
  /** 正在执行的行内动作（id + 行 id）：该按钮转圈并换忙碌文案，其余只按 builder 的 disabled 禁用。 */
  busyActionId?: string | null;
  busyRowId?: string | null;
  onRowAction: (rowId: string, actionId: string) => void;
  onOpenDetail: (rowId: string) => void;
}

export function AccountsRows({ rows, view = "list", listTestId, busyActionId = null, busyRowId = null, onRowAction, onOpenDetail }: AccountsRowsProps) {
  return (
    <ul className={view === "list" ? "account-list" : "account-card-grid"} data-testid={listTestId}>
      {rows.map((row) => {
        // 执行中的动作不进共享组件的动作槽（该槽只换文案、不带转圈）：改由本适配层渲染（转圈 + 忙碌文案）。
        const slotAction = row.actions[0]?.kind === "primary"
          && !(busyRowId === row.id && busyActionId === row.actions[0].id) ? row.actions[0] : null;
        const side = slotAction === null ? row.actions : row.actions.slice(1);
        // 右栏/卡片侧区：指标格 + 未进动作槽的动作；皆空即不渲染。
        const aside = (v: AccountView): ReactNode => <>{metricSlot(row, v)}{sideActions(row, side, onRowAction, { actionId: busyActionId, rowId: busyRowId })}</>;
        return (
          <SharedAccountEntry
            key={row.id}
            account={{
              id: row.id, displayName: row.displayName, lastVerifiedAt: null,
              // 「使用中」已由 builder 编成 accent 徽章：共享行不再渲染可见 chip，锚点走隐藏元素。
              isSelected: false,
            }}
            variant={view}
            testId={row.testId}
            actionTestId={slotAction?.id ?? ""}
            statusContent={<>{badgeSlot(row)}{currentAnchor(row)}</>}
            listMeta={metaSlot(row, "list")}
            cardMeta={metaSlot(row, "card")}
            listMetric={aside("list")}
            cardMetric={aside("card")}
            actionBusy={slotAction?.disabled !== undefined}
            hasAction={slotAction !== null}
            onOpen={() => onOpenDetail(row.id)}
            onAction={() => {
              if (slotAction !== null) onRowAction(row.id, slotAction.id);
            }}
            // 文案与悬浮说明全来自 builder：执行中换忙碌文案（config 表），无禁用原因即无提示
            actionLabel={slotAction?.label ?? ""}
            actionBusyLabel={slotAction?.label ?? ""}
            actionTitle={slotAction?.disabled?.reason ?? ""}
          />
        );
      })}
    </ul>
  );
}