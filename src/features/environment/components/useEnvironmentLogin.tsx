import { useCallback, useRef, useState, type ReactNode } from "react";
import { notify } from "../../../platform/notify";
import { safeUiErrorMessage } from "../../../utils/safeUiError";
import type { AccountProfileDto } from "../../../types/account_switch";
import type { EnvironmentLoginReceiptDto } from "../../../types/environment";
import { ENV_TEXTS } from "../config";
import { EnvironmentLoginDialog, type LoginReceiptView } from "./EnvironmentLoginDialog";

/**
 * 环境登录账号弹层 hook（P4d-8a-2c）：选已有账号登录到指定环境（旧页 EnvironmentLoginDialog
 * L1270-1435 + 页面 L846-857 的 onDone 编排）。命令由调用方注入（本文件零 platform 命令、
 * 零 invoke、零产品判断）：listAccounts → get_managed_account_state，login → login_environment。
 *
 * 刷新口径逐字照旧页：登录成功不立即重读（弹层停在回执态），点「完成」才关层 + 整组 load()；
 * 重读失败只警告「账号已切换，但环境状态刷新失败，请手动刷新。」（L853），不改报登录失败。
 */

/** 注入命令：账号列表（只读）+ 登录到环境（破坏性由环境侧单次确认）。 */
export interface EnvironmentLoginCommands {
  /** 已保存账号列表（后端 DTO 的 readonly 数组，hook 内拷贝为可变数组入 state）。 */
  readonly listAccounts: () => Promise<{ readonly saved_accounts: readonly AccountProfileDto[] }>;
  /** 登录到环境：空环境首登=存档移植，否则凭据互换 + 记录交接。 */
  readonly login: (envId: string, profileId: string) => Promise<EnvironmentLoginReceiptDto>;
}

export interface EnvironmentLoginInput {
  readonly commands: EnvironmentLoginCommands;
  /** 登录完成后的整组重读（旧页 load()）；失败已由本 hook 警告，不重抛。 */
  readonly refresh: () => Promise<void>;
}

export interface EnvironmentLoginResult {
  /** 登录弹层节点（无打开项时 null）；挂到页面根节点即可。 */
  readonly dialog: ReactNode;
  /** 打开登录弹层（env-login-<id>）：先开层再读账号列表（旧页 L1292-1308 同序）。 */
  readonly openLogin: (target: { readonly envId: string; readonly envName: string }) => void;
}

/** 登录会话态（旧页 L1278-1290 的五个 useState 合并为一个会话）。 */
interface LoginSession {
  readonly envId: string;
  readonly envName: string;
  readonly accounts: readonly AccountProfileDto[] | null;
  readonly selected: string | null;
  readonly busy: boolean;
  readonly error: string | null;
  readonly receipt: LoginReceiptView | null;
}

export function useEnvironmentLogin(input: EnvironmentLoginInput): EnvironmentLoginResult {
  const latest = useRef(input);
  latest.current = input;
  const [session, setSession] = useState<LoginSession | null>(null);
  // 代次守卫：连开两个环境时晚到的旧账号列表不得落到新弹层（旧页 L1293 的 cancelled 同法）。
  const ticket = useRef(0);

  const openLogin = useCallback((target: { readonly envId: string; readonly envName: string }) => {
    const mine = ++ticket.current;
    setSession({ envId: target.envId, envName: target.envName, accounts: null, selected: null, busy: false, error: null, receipt: null });
    void latest.current.commands.listAccounts()
      .then((view) => {
        if (mine !== ticket.current) return;
        setSession((current) => current === null ? null : { ...current, accounts: [...view.saved_accounts] });
      })
      .catch((reason: unknown) => {
        const message = safeUiErrorMessage(reason, ENV_TEXTS.loginFailure);
        if (mine !== ticket.current) return;
        setSession((current) => current === null ? null : { ...current, accounts: [], error: message });
      });
  }, []);

  const select = useCallback((profileId: string) => {
    setSession((current) => current === null ? null : { ...current, selected: profileId });
  }, []);

  // 关闭：执行中不关（旧页 L1330 只按 busy 拦截）。
  const cancel = useCallback(() => {
    setSession((current) => (current?.busy === true ? current : null));
  }, []);

  // 确认登录（旧页 L1312-1335）：成功转回执态（不关层、不刷新），失败内联提示。
  const confirm = useCallback(() => {
    const current = session;
    if (current === null || current.busy || current.selected === null) return;
    const account = current.accounts?.find((candidate) => candidate.profile_id === current.selected);
    const accountName = account?.display_name ?? ENV_TEXTS.incorporateTargetFallback;
    setSession({ ...current, busy: true, error: null });
    void latest.current.commands.login(current.envId, current.selected)
      .then((result) => {
        setSession((now) => now === null ? null : { ...now, busy: false, receipt: {
          accountName, seeded: result.seeded, transferredProjects: result.transferred_projects,
          switchedSessions: result.switched_sessions, currentProfileSaved: result.current_profile_saved } });
      })
      .catch((reason: unknown) => {
        const message = safeUiErrorMessage(reason, ENV_TEXTS.loginFailure);
        setSession((now) => now === null ? null : { ...now, busy: false, error: message });
      });
  }, [session]);

  // 完成（旧页 L850-856）：先关层再整组重读；重读失败只警告，不覆盖已完成的登录结果。
  const done = useCallback(() => {
    ticket.current += 1; // 作废在途账号列表
    setSession(null);
    void latest.current.refresh().catch(() => { notify.warning(ENV_TEXTS.loginRefreshFailed); });
  }, []);

  const dialog = session === null ? null : (
    <EnvironmentLoginDialog envName={session.envName} accounts={session.accounts} selected={session.selected}
      busy={session.busy} error={session.error} receipt={session.receipt}
      onSelect={select} onCancel={cancel} onConfirm={confirm} onDone={done} />
  );

  return { dialog, openLogin };
}
