import { Fragment, type ReactNode } from "react";
import { RefreshCw } from "lucide-react";
import { Badge } from "../../../ui/Badge";
import type { AccountActionKind, AccountsView } from "../config";

/**
 * 账号页页头（P4c-6a，纯展示）：标题 + 产品徽标槽 + 状态徽章 + 动作按钮组。
 * 文案、禁用原因、testid 全部来自 builder 的 header 视图模型（testid = action.id，
 * 逐字沿用旧实现）：本文件不拼 testid、不自创文案、不判断 productId、不读数据。
 */
const KIND_CLASS: Record<AccountActionKind, string> = {
  primary: "btn btn--primary", // 主行动（如「添加账号」）
  ghost: "btn", // 页头次要按钮：与旧页头同款描边按钮
  danger: "btn btn--danger", // 破坏性描边按钮
};

export interface AccountsHeaderProps {
  view: AccountsView["header"];
  /** 产品徽标槽（页面注入 <ProductBadge productId=… />）。 */
  productBadge?: ReactNode;
  /** 登录方式选择槽（隔离/本机浏览器；能力开关与选项文案归页面）。 */
  loginMode?: ReactNode;
  /** 进行中的动作 id：builder 已算好 disabled.reason，此处只加转圈图标，不改文案。 */
  busy?: readonly string[];
  onAction: (id: string) => void;
}

export function AccountsHeader({
  view,
  productBadge,
  loginMode,
  busy = [],
  onAction,
}: AccountsHeaderProps) {
  // 登录方式选择器落在首个主按钮之前（旧布局：与「添加账号」同一组）；无主按钮则垫底。
  const primaryAt = view.actions.findIndex((action) => action.kind === "primary");
  return (
    <header className="page-header">
      <div className="page-header__copy">
        <h1 data-page-title="accounts" tabIndex={-1}>{view.title}</h1>
        {productBadge}
      </div>
      <div className="page-header__actions">
        {/* 状态徽章与按钮同一行（旧页头顺序：徽章在前、按钮在后）。 */}
        {view.badges.map((badge, index) => (
          <Badge key={`${badge.text}-${index}`} tone={badge.tone}>{badge.text}</Badge>
        ))}
        {view.actions.map((action, index) => {
          const running = busy.includes(action.id);
          return (
            <Fragment key={action.id}>
              {index === primaryAt && loginMode}
              <button
                className={KIND_CLASS[action.kind]}
                type="button"
                onClick={() => onAction(action.id)}
                disabled={action.disabled !== undefined}
                title={action.disabled?.reason}
                aria-busy={running || undefined}
                data-testid={action.id}
              >
                {running && <RefreshCw size={15} className="icon-spin" aria-hidden="true" />}
                {action.label}
              </button>
            </Fragment>
          );
        })}
        {primaryAt < 0 && loginMode}
      </div>
    </header>
  );
}
