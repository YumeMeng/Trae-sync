import type { ReactNode } from "react";
import { Database, Layers, LogIn, MonitorPlay, Pencil, RefreshCw, Square, Trash2 } from "lucide-react";
import { InstanceSlotBadge } from "../../../components/StatusBadges";
import type { TraeInstanceLoginState } from "../../../types/account_switch";
import { Badge, type BadgeTone } from "../../../ui/Badge";
import type { EnvActionKind, EnvActionView, EnvCardView, EnvProcessState } from "../config";


/**
 * 环境实例卡（P4d-7a，纯展示）：EnvCardView → 旧页卡片结构（主库卡 L434-697 / 副环境卡 L1001-1178 同构）。
 * 名称/徽章/当前账号/体积/路径/按钮文案与动作 testid 全部来自 builder 视图模型；本文件不自创文案、
 * 不判断 productId、不读数据、不发命令（动作只回调 onAction(cardId, actionId)）。 */

/** 动作样式档：与页头/账号页同一套（primary=主按钮 / ghost=描边 / danger=红色描边）。 */
const KIND_CLASS: Record<EnvActionKind, string> = { primary: "btn btn--primary", ghost: "btn", danger: "btn btn--danger" };
/** 卡片图标：主库=数据库、辅助环境=层叠（旧页 Database/Layers 22px 1.8）。 */
const CARD_ICON: Record<EnvCardView["icon"], ReactNode> = {
  database: <Database size={22} strokeWidth={1.8} aria-hidden="true" />,
  layers: <Layers size={22} strokeWidth={1.8} aria-hidden="true" />,
};
/** 只读状态卡缺省色档：运行=强调、未运行=中性、未知=琥珀。 */
const STATE_TONE: Record<EnvProcessState, BadgeTone> = { running: "accent", not_running: "neutral", unknown: "warn" };
/** 动作图标：按动作 id 的动词段取旧页同款图标（本层只选图，不改文案）。 */
const ICONS: readonly (readonly [string, ReactNode])[] = [
  ["refresh", <RefreshCw size={15} aria-hidden="true" />], ["-detail", <Database size={15} aria-hidden="true" />],
  ["-close", <Square size={15} aria-hidden="true" />], ["-launch", <MonitorPlay size={15} aria-hidden="true" />],
  ["-login", <LogIn size={15} aria-hidden="true" />], ["-rename", <Pencil size={15} aria-hidden="true" />],
  ["-delete", <Trash2 size={15} aria-hidden="true" />],
];
const actionIcon = (id: string): ReactNode => ICONS.find(([key]) => id.includes(key))?.[1] ?? null;
/** 有启动动作 = 环境卡（两槽位徽章语言）；无 = 只读状态卡（状态词徽章）。 */
const hasLaunch = (card: EnvCardView): boolean => card.actions.some((action) => action.id.endsWith("-launch"));
/** 骨架静态锚（旧页 L442/L454/L653/L1101 逐字）：视图模型只给容器锚与动作锚，内部锚按卡片形状派生。 */
const isEnvCard = (card: EnvCardView): boolean => card.id === "master" || card.testId.startsWith("env-card-");
const currentTestIdOf = (card: EnvCardView): string | undefined => (card.id === "master" ? "env-current-name" : undefined);
const statusTestIdOf = (card: EnvCardView): string | undefined => (card.id === "master" ? "env-master-state" : undefined);
const pathTestIdOf = (card: EnvCardView): string | undefined =>
  (card.id === "master" ? "env-data-dir" : isEnvCard(card) ? `env-path-${card.id}` : undefined);
/** 页面按产品注入的内部锚与色档（缺省走 env-* 派生；视图模型无色档字段，只读卡的色由页面给）。 */
export interface EnvCardAnchors {
  readonly statusTestId?: string; readonly pathTestId?: string; readonly statusTone?: BadgeTone;
}
/** 状态槽：环境卡用两槽位徽章；只读状态卡用状态词徽章；无状态词的卡（WB 登录域健康卡）不占位。 */
function StatusSlot({ card, statusTestId, statusTone, loginState }: {
  card: EnvCardView; statusTestId?: string; statusTone?: BadgeTone; loginState?: TraeInstanceLoginState;
}) {
  if (hasLaunch(card)) {
    return (
      <div className="env-card__status" data-testid={statusTestId ?? statusTestIdOf(card)}>
        <InstanceSlotBadge running={card.process.state === "running"} processState={card.process.state} loginState={loginState} />
      </div>
    );
  }
  if (card.process.launchLabel === "") return null;
  return (
    <div className="env-card__status">
      <Badge tone={statusTone ?? STATE_TONE[card.process.state]} hint={card.process.launchHint}
        testId={statusTestId ?? statusTestIdOf(card)}>{card.process.launchLabel}</Badge>
    </div>
  );
}
/** 单枚动作按钮（页头/卡片共用）：disabledReason 非空即禁用并给悬浮说明；busy 由页面注入。 */
export function EnvActionButton({ action, busy, onClick }: {
  action: EnvActionView; busy: boolean; onClick: () => void;
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

export interface EnvCardProps {
  /** 卡片视图（主库卡与副环境卡同构）；hint = 引导句（主库登录两态，缺省/空串不渲染）。 */
  card: EnvCardView; hint?: string;
  /** 执行中的动作 id（页面按忙碌位注入；缺省=无动作执行中）。 */
  busy?: readonly string[];
  /** 内部锚覆盖（WB 运行态徽章）。 */
  anchors?: EnvCardAnchors;
  /** 实例登录子态（视图模型不含，由页面透传；决定「运行中 · 已登录/待登录」）。 */
  loginState?: TraeInstanceLoginState;
  /** 卡片内区块槽（体检 / 捕获，由第二组组件填充）。 */
  children?: ReactNode;
  onAction: (cardId: string, actionId: string) => void;
}

/** 单张实例卡：状态徽章 + 名称/徽章 + 当前账号块 + 引导句 + 区块槽 + 路径/动作脚。 */
export function EnvCard({ card, hint, busy = [], anchors, loginState, children, onAction }: EnvCardProps) {
  return (
    <div className="env-card" data-testid={card.testId}>
      <div className="env-card__head">
        <span className="env-card__icon" aria-hidden="true">{CARD_ICON[card.icon]}</span>
        <div className="env-card__title">
          <div className="env-card__name-line">
            <span className="env-card__name">{card.name}</span>
            {card.badge !== "" && <span className="env-card__badge">{card.badge}</span>}
          </div>
          <StatusSlot card={card} statusTestId={anchors?.statusTestId} statusTone={anchors?.statusTone} loginState={loginState} />
        </div>
        {card.currentAccountName !== null && (
          <div className="env-card__current">
            <span className="env-card__current-avatar" aria-hidden="true">{card.currentAccountName.slice(0, 1)}</span>
            <div>
              <div className="env-card__current-name" data-testid={currentTestIdOf(card)}>{card.currentAccountName}</div>
              {card.currentAccountMeta !== "" && <div className="env-card__current-meta">{card.currentAccountMeta}</div>}
            </div>
          </div>
        )}
      </div>
      {hint !== undefined && hint !== "" && <p className="env-card__hint">{hint}</p>}
      {children}
      <div className="env-card__foot">
        {/* 路径完整值只进悬浮提示，主视野只显示标签/体积（界面表达纪律）。 */}
        <span className="env-card__path" title={card.pathTitle !== "" ? card.pathTitle : undefined}
          data-testid={anchors?.pathTestId ?? pathTestIdOf(card)}>{card.pathLabel}</span>
        <div className="env-card__foot-actions">
          {card.actions.map((action) => (
            <EnvActionButton key={action.id} action={action} busy={busy.includes(action.id)}
              onClick={() => onAction(card.id, action.id)} />
          ))}
        </div>
      </div>
    </div>
  );
}

export interface EnvCardListProps {
  /** 卡列表（辅助环境区：逐张同构渲染，外层沿用旧页 .env-list 栅格）。 */
  cards: readonly EnvCardView[];
  busy?: readonly string[];
  /** 逐卡区块槽（副环境体检）与逐卡实例登录子态（页面按 env_id 取）。 */
  cardExtra?: (card: EnvCardView) => ReactNode;
  loginStateOf?: (card: EnvCardView) => TraeInstanceLoginState | undefined;
  onAction: (cardId: string, actionId: string) => void;
}

/** 卡列表：只负责分组与稳定 key（env_id），不参与任何数据判断。 */
export function EnvCardList({ cards, busy, cardExtra, loginStateOf, onAction }: EnvCardListProps) {
  return (
    <div className="env-list">
      {cards.map((card) => (
        <EnvCard key={card.id} card={card} busy={busy} loginState={loginStateOf?.(card)} onAction={onAction}>
          {cardExtra?.(card)}
        </EnvCard>
      ))}
    </div>
  );
}
