import { CheckCircle2, TriangleAlert } from "lucide-react";
import type { AccountProfileDto } from "../../../types/account_switch";
import { ENV_TEXTS, fillEnvText } from "../config";

/**
 * 环境登录账号弹层（P4d-8a-2c，纯展示）：逐字搬旧页 EnvironmentLoginDialog（L1270-1435）。
 * 骨架与 testid 原样（env-login-dialog / env-login-account-list / env-login-account-<profile_id> /
 * env-login-receipt / env-login-cancel / env-login-confirm / env-login-done）；文案全部取
 * ENV_TEXTS；本文件零命令、零产品判断，读取与登录由 useEnvironmentLogin 注入。
 * 未复用 AccountLoginDialog：那是 OAuth 新增账号（浏览器授权）弹层，与本弹层语义不同（选已有账号登录到环境）。
 */

/** 登录回执（旧页 L1284-1290 的本地形状）：账号名与三项计数 + 是否记住当前账号。 */
export interface LoginReceiptView {
  readonly accountName: string;
  readonly seeded: boolean;
  readonly transferredProjects: number;
  readonly switchedSessions: number;
  readonly currentProfileSaved: boolean;
}

export interface EnvironmentLoginDialogProps {
  /** 目标环境显示名（标题用；绝不显示 env_id）。 */
  readonly envName: string;
  /** 已保存账号：null = 读取中。 */
  readonly accounts: readonly AccountProfileDto[] | null;
  readonly selected: string | null;
  readonly busy: boolean;
  /** 内联失败原因（读取账号或登录失败）。 */
  readonly error: string | null;
  /** 非 null = 已登录完成态（回执展示）。 */
  readonly receipt: LoginReceiptView | null;
  readonly onSelect: (profileId: string) => void;
  readonly onCancel: () => void;
  readonly onConfirm: () => void;
  readonly onDone: () => void;
}

export function EnvironmentLoginDialog(props: EnvironmentLoginDialogProps) {
  const { envName, accounts, selected, busy, error, receipt, onSelect, onCancel, onConfirm, onDone } = props;
  return (
    <div className="switch-veil" role="presentation" onClick={(event) => {
      if (event.target === event.currentTarget && !busy) onCancel();
    }}>
      <div className="switch-dialog" role="dialog" aria-modal="true" aria-label={ENV_TEXTS.loginDialogAria}
        data-testid="env-login-dialog">
        <div className="switch-dialog__title">{fillEnvText(ENV_TEXTS.loginDialogTitle, { name: envName })}</div>
        {receipt !== null ? (
          <>
            <div className="switch-dialog__done" role="status" data-testid="env-login-receipt">
              <CheckCircle2 size={18} aria-hidden="true" />
              <span>
                {fillEnvText(ENV_TEXTS.loginReceiptPrefix, { name: receipt.accountName })}
                {receipt.seeded ? ENV_TEXTS.loginReceiptSeeded : fillEnvText(ENV_TEXTS.loginReceiptSwitched, {
                  projects: receipt.transferredProjects, sessions: receipt.switchedSessions })}
              </span>
            </div>
            {!receipt.currentProfileSaved && (
              <div className="switch-dialog__notice switch-dialog__notice--danger" role="alert">
                <TriangleAlert size={18} aria-hidden="true" />
                <p>{ENV_TEXTS.loginNotRemembered}</p>
              </div>
            )}
            <div className="switch-dialog__actions">
              <button className="btn btn--primary" type="button" onClick={onDone} data-testid="env-login-done">
                {ENV_TEXTS.loginDone}
              </button>
            </div>
          </>
        ) : (
          <>
            <p className="switch-dialog__prepare">{ENV_TEXTS.loginPrepare}</p>
            {accounts === null ? (
              <p className="switch-dialog__prepare">{ENV_TEXTS.loginLoading}</p>
            ) : accounts.length === 0 ? (
              <div className="switch-dialog__notice">
                <TriangleAlert size={18} aria-hidden="true" />
                <p>{ENV_TEXTS.loginNoAccounts}</p>
              </div>
            ) : (
              <ul className="env-login-list" data-testid="env-login-account-list">
                {accounts.map((account) => {
                  const isSelected = selected === account.profile_id;
                  return (
                    <li key={account.profile_id}>
                      <button type="button" className={"env-login-account" + (isSelected ? " env-login-account--selected" : "")}
                        onClick={() => onSelect(account.profile_id)} role="radio" aria-checked={isSelected}
                        data-testid={"env-login-account-" + account.profile_id}>
                        <span className="env-card__current-avatar" aria-hidden="true">{account.display_name.slice(0, 1)}</span>
                        <span className="env-login-account__name">{account.display_name}</span>
                      </button>
                    </li>
                  );
                })}
              </ul>
            )}
            {error !== null && (
              <div className="switch-dialog__notice switch-dialog__notice--danger" role="alert">
                <TriangleAlert size={18} aria-hidden="true" />
                <p>{error}</p>
              </div>
            )}
            <div className="switch-dialog__actions">
              <button className="btn" type="button" onClick={onCancel} disabled={busy} data-testid="env-login-cancel">
                {ENV_TEXTS.cancel}
              </button>
              <button className="btn btn--primary" type="button" onClick={onConfirm}
                disabled={busy || selected === null} data-testid="env-login-confirm">
                {busy ? ENV_TEXTS.loginBusy : ENV_TEXTS.loginConfirm}
              </button>
            </div>
          </>
        )}
      </div>
    </div>
  );
}
