import { notify } from "../../../platform/notify";
import type { AccountLoginPhase } from "./useAccountLogin";

/**
 * 添加账号 / OAuth 登录弹层（P4c-6b-2c）：承接 useAccountLogin 的返回，只渲染、不发命令。
 *
 * 形态对齐旧 AccountCenterTrae（L1094-1138）：浏览器方式选择（隔离 / 本机，能力表
 * loginBrowser 决定是否出现）、等待期间可取消、收尾阶段（settling）按钮文案与禁用同旧实现
 * ——「等待浏览器登录完成…」+ 取消登录仍可点。登录地址可复制：后端 begin 已同时拉起浏览器，
 * 地址仅作兜底展示（旧 WB L357 同一说法）。
 *
 * testid：account-cancel-login / login-browser-mode 逐字沿用旧实现；其余为本票新增
 * （旧实现无弹层，等待态挂在页头 status 区），清单见 P4c-6b-2c 报告。
 */
export interface AccountLoginDialogProps {
  /** 能力表 loginBrowser === "isolated-or-system"：两模式可选；false 即只走本机浏览器。 */
  readonly browserModes: boolean;
  readonly useSystemBrowser: boolean;
  readonly phase: AccountLoginPhase;
  /** begin 回执里的登录页地址；null = 尚未发起（此时给「打开浏览器」）。 */
  readonly loginUrl: string | null;
  /** 等待态文案（旧 L1122-1125 逐字，由页面按浏览器模式给）。 */
  readonly waitingHint: string;
  readonly onBrowserModeChange: (useSystemBrowser: boolean) => void;
  readonly onOpenBrowser: () => void;
  readonly onComplete: () => void;
  readonly onCancel: () => void;
}

export function AccountLoginDialog(props: AccountLoginDialogProps) {
  const { browserModes, useSystemBrowser, phase, loginUrl, waitingHint } = props;
  const begun = loginUrl !== null;
  // 收尾阶段（取消太晚 / 已点「我已完成登录」）：旧实现按钮保持「等待浏览器登录完成…」
  // 并禁用，取消入口仍在（旧 L1132 只按 attempt 是否为空禁用）。
  const settling = phase === "settling";
  const beginning = phase === "beginning";

  // 复制登录地址：剪贴板不可用（jsdom / 权限被拒）时如实警告，不谎报已复制。
  const copyUrl = () => {
    if (loginUrl === null) return;
    void navigator.clipboard?.writeText(loginUrl).then(
      () => notify.success("登录地址已复制，可在任意浏览器中打开继续登录。"),
      () => notify.warning("复制失败，请手动选中地址复制。"),
    );
  };

  return (
    <div
      className="switch-veil"
      role="presentation"
      onClick={(event) => { if (event.target === event.currentTarget && !settling) props.onCancel(); }}
    >
      <div className="switch-dialog" role="dialog" aria-modal="true" aria-label="添加账号" data-testid="account-login-dialog">
        <div className="switch-dialog__title">添加账号</div>
        <div className="switch-dialog__body">
          <p className="account-center__meta">{begun ? waitingHint : "选择登录浏览器后打开授权页，登录成功后账号会自动保存到本机。"}</p>
          {begun ? (
            <>
              <code className="account-center__meta" data-testid="account-login-url">{loginUrl}</code>
              <button className="btn btn--quiet" type="button" onClick={copyUrl} data-testid="account-login-copy-url">复制登录地址</button>
            </>
          ) : browserModes ? (
            <select
              aria-label="登录浏览器方式"
              className="page-header__select"
              value={useSystemBrowser ? "system" : "isolated"}
              disabled={beginning}
              onChange={(event) => props.onBrowserModeChange(event.target.value === "system")}
              data-testid="login-browser-mode"
            >
              <option value="isolated">隔离浏览器</option>
              <option value="system">本机浏览器</option>
            </select>
          ) : null}
        </div>
        <div className="switch-dialog__actions">
          <button className="btn btn--quiet" type="button" onClick={props.onCancel} data-testid="account-cancel-login">取消登录</button>
          {begun ? (
            <button
              className="btn btn--primary"
              type="button"
              onClick={props.onComplete}
              disabled={settling}
              data-testid="account-login-complete"
            >
              {settling ? "等待浏览器登录完成…" : "我已完成登录"}
            </button>
          ) : (
            <button
              className="btn btn--primary"
              type="button"
              onClick={props.onOpenBrowser}
              disabled={beginning}
              data-testid="account-login-open-browser"
            >
              {beginning ? "正在打开浏览器…" : "打开浏览器"}
            </button>
          )}
        </div>
      </div>
    </div>
  );
}
