import type { ReactNode } from "react";
import { ArrowLeftRight } from "lucide-react";

// U-2 账号页双视图：list=横向宽行（默认，多账号扫描对比最快）；card=卡片网格。
export type AccountView = "list" | "card";

/** 账号条目共享的最小展示数据；产品 DTO 只能在适配层映射到这里。 */
export interface SharedAccountRecord {
  readonly id: string;
  readonly displayName: string;
  readonly lastVerifiedAt: string | null;
  readonly isSelected: boolean;
}

export interface SharedAccountEntryProps {
  account: SharedAccountRecord;
  variant: AccountView;
  testId: string;
  actionTestId: string;
  selectedTestId?: string;
  statusContent?: ReactNode;
  listMeta: ReactNode;
  cardMeta: ReactNode;
  listMetric?: ReactNode;
  cardMetric?: ReactNode;
  actionBusy: boolean;
  hasAction?: boolean;
  onOpen?: () => void;
  onAction?: () => void;
  actionTitle?: string;
  selectedLabel?: string;
  selectedTitle?: string;
  actionLabel?: string;
  actionBusyLabel?: string;
}

export function avatarLetter(name: string | null | undefined): string {
  const text = (name ?? "").trim();
  if (text.length === 0) return "?";
  return text.charAt(0).toUpperCase();
}

/**
 * 账号条目（U-2 双视图）：variant=list 横向宽行 / variant=card 卡片网格。
 * 两槽位徽章系统（StatusBadges）：槽位1 签到三态 + 槽位2 登录存档健康度；
 * 登录凭据剩余时间与手机号降级为 meta 文字（正常态安静，异常才亮色）。
 * 点击条目进入详情视图；切换按钮不冒泡。
 * 注：签到活动 credits 为静态池值（实证恒 200），不展示以免误导。
 */
/**
 * 完整账号事务与适配器注入路径共用的账号条目骨架。
 * 产品差异只能通过状态槽、指标槽和动作能力进入，不能再复制整张卡片。
 */
export function SharedAccountEntry({
  account,
  variant,
  testId,
  actionTestId,
  selectedTestId,
  statusContent,
  listMeta,
  cardMeta,
  listMetric,
  cardMetric,
  actionBusy,
  hasAction = true,
  onOpen,
  onAction,
  actionTitle = `切换当前产品到 ${account.displayName}`,
  selectedLabel = "使用中",
  selectedTitle = "当前产品正在使用的账号",
  actionLabel = "切换到此账号",
  actionBusyLabel = "切换中…",
}: SharedAccountEntryProps) {
  const interactive = onOpen !== undefined;
  const displayName = account.displayName || "未命名账号";
  const rowClassName = variant === "list"
    ? `account-list__row account-item${interactive ? " account-list__row--interactive" : ""}`
    : `account-card account-item${interactive ? " account-card--clickable" : ""}`;
  const selectedIndicator = account.isSelected ? (
    <span
      className="account-item__current-chip"
      data-testid={selectedTestId}
      title={selectedTitle}
    >
      {selectedLabel}
    </span>
  ) : null;
  const actionContent = !hasAction ? selectedIndicator : account.isSelected ? selectedIndicator : (
    <button
      className={variant === "card" ? "btn btn--primary account-card__switch" : "btn btn--primary"}
      type="button"
      disabled={actionBusy}
      onClick={(event) => {
        event.stopPropagation();
        onAction?.();
      }}
      data-testid={actionTestId}
      title={actionTitle}
    >
      <ArrowLeftRight size={14} aria-hidden="true" />
      {actionBusy ? actionBusyLabel : actionLabel}
    </button>
  );

  return (
    <li>
      <div
        className={rowClassName}
        role={interactive ? "button" : undefined}
        tabIndex={interactive ? 0 : undefined}
        onClick={interactive ? onOpen : undefined}
        onKeyDown={interactive ? (event) => {
          // 仅条目本体自身按键进入详情；操作按钮的按键事件不冒泡触发。
          if (event.target === event.currentTarget && (event.key === "Enter" || event.key === " ")) {
            event.preventDefault();
            onOpen?.();
          }
        } : undefined}
        data-testid={testId}
        title={interactive ? `查看 ${displayName} 的详情` : undefined}
      >
        {variant === "list" ? (
          <>
            <div className="account-item__main">
              <span className="account-card__avatar" aria-hidden="true">{avatarLetter(displayName)}</span>
              <div className="account-item__id">
                <div className="account-item__name-row">
                  <strong className="account-item__name" title={displayName}>{displayName}</strong>
                  {statusContent}
                </div>
                <div className="account-meta-group">{listMeta}</div>
              </div>
            </div>
            <div className="account-item__side">
              {listMetric}
              {actionContent}
            </div>
          </>
        ) : (
          <>
            <div className="account-card__head">
              <span className="account-card__avatar" aria-hidden="true">{avatarLetter(displayName)}</span>
              <strong className="account-card__name" title={displayName}>{displayName}</strong>
              {selectedIndicator}
            </div>
            {statusContent && <div className="account-card__slots">{statusContent}</div>}
            {cardMetric}
            <div className="account-meta-group">{cardMeta}</div>
            {!account.isSelected && actionContent}
          </>
        )}
      </div>
    </li>
  );
}
