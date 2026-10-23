import { useCallback, useState } from "react";
import type { AccountLoginDialogProps } from "./AccountLoginDialog";
import { useAccountLogin, type AccountLoginCommands, type AccountLoginReceipt } from "./useAccountLogin";

/**
 * 登录流程的页面侧接线（P4c-6b-2c）：把 useAccountLogin 的分步接口（begin / complete /
 * cancel）收成一个弹层门面——打开意图、浏览器模式、成功后重读本机缓存都在这里，
 * AccountsPage 只查接缝表（PageLogin）并渲染 dialog。
 *
 * 未登记登录接缝的产品（wiring=null）一律 fail-closed：handle 恒返回 false，
 * 弹层永不出现，页头「添加账号」动作原样透出给调用方（旧页面自持入口）。
 */
export interface PageLogin {
  /** 页头「添加账号」动作 id（旧 testid，如 account-add-primary）。 */
  readonly actionId: string;
  readonly commands: AccountLoginCommands;
  /** 能力表 loginBrowser === "isolated-or-system"：两种浏览器模式可选；false 即只走本机浏览器。 */
  readonly browserModes: boolean;
  /** 等待态说明（旧实现 L1122-1125 逐字，按浏览器模式分支）。 */
  readonly waitingHint: (useSystemBrowser: boolean) => string;
}

/** 未登记产品的占位命令：只在误调时如实失败，不伪装成功（页面靠 wiring=null 拦住入口）。 */
const unsupported = (): Promise<never> => Promise.reject("login_unsupported");
const UNAVAILABLE_LOGIN_COMMANDS: AccountLoginCommands = { begin: unsupported, complete: unsupported, cancel: unsupported };

export interface AccountLoginFlow {
  /** 等待浏览器授权中（builder 的 busy.login：页头按钮禁用与「等待浏览器登录完成…」）。 */
  readonly busy: boolean;
  /** 弹层属性；null = 不挂载（未登记接缝，或本轮已收尾）。 */
  readonly dialog: AccountLoginDialogProps | null;
  /** 页头动作分发：true = 已由本流程处理（打开弹层），调用方不再透出。 */
  readonly handle: (actionId: string) => boolean;
}

export function useAccountLoginFlow(wiring: PageLogin | null, onSettled: () => Promise<void>): AccountLoginFlow {
  const { beginLogin, completeLogin, cancelLogin, attempt, phase, loginUrl } = useAccountLogin(
    wiring?.commands ?? UNAVAILABLE_LOGIN_COMMANDS,
  );
  // open = 用户打开意图；仍在进行中的 attempt 也保持挂载（取消太晚进入收尾时弹层不消失）。
  const [open, setOpen] = useState(false);
  const [systemBrowser, setSystemBrowser] = useState(false);
  const mounted = open || attempt !== null || phase !== "idle";

  const handle = useCallback((actionId: string): boolean => {
    if (wiring === null || actionId !== wiring.actionId) return false;
    setOpen(true);
    return true;
  }, [wiring]);

  // 「打开浏览器」= 发起登录（后端 begin 即拉起所选浏览器）；失败由 hook 通知，弹层留在原地可重试。
  const openBrowser = useCallback(() => { void beginLogin(systemBrowser); }, [beginLogin, systemBrowser]);

  // 「我已完成登录」：等回调 → 成功即重读本机缓存（旧 L468 refreshOverview 同法）。
  // 失败/取消的提示语义全在 hook 内，此处只负责收尾关层（取消太晚时 attempt 仍在，
  // 弹层按 attempt 继续挂载并展示 settling，不会因为关层而丢掉真实结果）。
  const complete = useCallback(() => {
    void (async () => {
      const receipt: AccountLoginReceipt | null = await completeLogin();
      setOpen(false);
      if (receipt !== null) await onSettled();
    })();
  }, [completeLogin, onSettled]);

  // 取消：先收起意图；若取消太晚（attempt 仍在收尾），弹层按 attempt 继续挂载并展示 settling。
  const cancel = useCallback(() => {
    setOpen(false);
    void cancelLogin();
  }, [cancelLogin]);

  const dialog = wiring === null || !mounted ? null : {
    browserModes: wiring.browserModes,
    useSystemBrowser: systemBrowser,
    phase,
    loginUrl,
    waitingHint: wiring.waitingHint(systemBrowser),
    onBrowserModeChange: setSystemBrowser,
    onOpenBrowser: openBrowser,
    onComplete: complete,
    onCancel: cancel,
  };

  return { busy: attempt !== null || phase !== "idle", dialog, handle };
}
