import { createElement, useCallback, useRef, useState } from "react";
import { notify } from "../../../platform/notify";
import { safeUiErrorMessage } from "../../../utils/safeUiError";
import type { AccountLoginDialogProps } from "./AccountLoginDialog";
import { MobileBackfillDialog } from "./MobileBackfillDialog";
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

/** 登录后手机号补录接缝（G11）：resolve 判定缺号账号（null = 不打扰），save 复用 set_account_mobile。 */
export interface LoginMobileBackfill {
  readonly resolve: (profileId: string) => Promise<{ readonly name: string; readonly masked: string } | null>;
  readonly save: (profileId: string, mobile: string) => Promise<void>;
}

export interface AccountLoginFlow {
  /** 等待浏览器授权中（builder 的 busy.login：页头按钮禁用与「等待浏览器登录完成…」）。 */
  readonly busy: boolean;
  /** 弹层属性；null = 不挂载（未登记接缝，或本轮已收尾）。 */
  readonly dialog: AccountLoginDialogProps | null;
  /** 手机号补录弹层（G11）；null = 不挂载（未登记补录接缝，或本轮未触发/已收尾）。 */
  readonly backfill: React.ReactNode;
  /** 页头动作分发：true = 已由本流程处理（打开弹层），调用方不再透出。 */
  readonly handle: (actionId: string) => boolean;
  /**
   * 直接打开登录弹层（P4c-6b-2e-1 详情「重新登录」）：旧 AccountDetail 的 onRelogin =
   * AccountCenterTrae L898 handleLogin(false)，与页头「添加账号」同一条 OAuth 流程，
   * 故复用同一弹层而非另起一套；false = 本产品未登记登录（动作原样透出）。
   */
  readonly open: () => boolean;
}

/** 登录流程入参：接缝 + 成功后重读缓存 + 可选补录接缝（G11）。 */
export interface AccountLoginFlowInput {
  readonly wiring: PageLogin | null;
  readonly onSettled: () => Promise<void>;
  /** 补录接缝；缺省 = 本产品不做补录（WB 能力表 mobileBackfill=false），弹层永不出现。 */
  readonly backfill?: LoginMobileBackfill;
}

export function useAccountLoginFlow(input: AccountLoginFlowInput): AccountLoginFlow {
  const { wiring, onSettled } = input;
  // 入参经 ref 取最新值：complete/cancel 保持稳定身份，补录判定在渲染期读最新数据面。
  const inputRef = useRef(input);
  inputRef.current = input;
  const { beginLogin, completeLogin, cancelLogin, attempt, phase, loginUrl } = useAccountLogin(
    wiring?.commands ?? UNAVAILABLE_LOGIN_COMMANDS,
  );
  // openIntent = 用户打开意图；仍在进行中的 attempt 也保持挂载（取消太晚进入收尾时弹层不消失）。
  const [openIntent, setOpen] = useState(false);
  const [systemBrowser, setSystemBrowser] = useState(false);
  const mounted = openIntent || attempt !== null || phase !== "idle";

  const handle = useCallback((actionId: string): boolean => {
    if (wiring === null || actionId !== wiring.actionId) return false;
    setOpen(true);
    return true;
  }, [wiring]);

  // 详情「重新登录」入口：与页头同一弹层（旧 onRelogin 就是 handleLogin(false)）。
  const openDialog = useCallback((): boolean => {
    if (wiring === null) return false;
    setOpen(true);
    return true;
  }, [wiring]);

  // 「打开浏览器」= 发起登录（后端 begin 即拉起所选浏览器）；失败由 hook 通知，弹层留在原地可重试。
  const openBrowser = useCallback(() => { void beginLogin(systemBrowser); }, [beginLogin, systemBrowser]);

  // 手机号补录弹层（G11）：目标账号 + 展示名 + 脱敏号；null = 不挂载。
  const [prompt, setPrompt] = useState<{ readonly profileId: string; readonly name: string; readonly masked: string } | null>(null);
  const [promptBusy, setPromptBusy] = useState(false);
  const [promptError, setPromptError] = useState<string | null>(null);

  // 「我已完成登录」：等回调 → 成功即重读本机缓存（旧 L468 refreshOverview 同法）→ 缺号引导补录。
  // 失败/取消的提示语义全在 hook 内，此处只负责收尾关层（取消太晚时 attempt 仍在，
  // 弹层按 attempt 继续挂载并展示 settling，不会因为关层而丢掉真实结果）。
  const complete = useCallback(() => {
    void (async () => {
      const receipt: AccountLoginReceipt | null = await completeLogin();
      setOpen(false);
      if (receipt === null) return;
      await onSettled();
      // 补录触发条件（旧 L478-487）：回执没带完整手机号，且重读后的总览确认该账号仍未补录。
      if (receipt.mobile_full) return;
      const seam = inputRef.current.backfill;
      if (seam === undefined) return;
      const target = await seam.resolve(receipt.profile_id);
      if (target === null) return;
      setPromptError(null);
      setPrompt({ profileId: receipt.profile_id, name: target.name, masked: target.masked });
    })();
  }, [completeLogin, onSettled]);

  // 补录保存（旧 L553-574）：成功后关层；失败保留弹层与输入，映射后文案就地展示。
  const saveBackfill = useCallback((value: string) => {
    const current = prompt;
    const seam = inputRef.current.backfill;
    if (current === null || seam === undefined || promptBusy) return;
    setPromptBusy(true);
    setPromptError(null);
    void (async () => {
      try {
        await seam.save(current.profileId, value.trim());
        await inputRef.current.onSettled();
        setPrompt(null);
        notify.success("手机号已补全，账号列表与详情页都会显示完整号码。");
      } catch (reason: unknown) {
        setPromptError(safeUiErrorMessage(reason, "手机号保存失败，请稍后重试。"));
      } finally {
        setPromptBusy(false);
      }
    })();
  }, [prompt, promptBusy]);

  // 跳过（旧 L846-858）：只关层，不写任何数据。
  const skipBackfill = useCallback(() => {
    if (promptBusy) return;
    setPrompt(null);
    setPromptError(null);
  }, [promptBusy]);

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

  // 补录弹层：未登记接缝或未触发即 null（挂载即打开，关闭即卸载）。
  const backfillNode = prompt === null ? null : createElement(MobileBackfillDialog, {
    accountName: prompt.name, masked: prompt.masked, busy: promptBusy, error: promptError,
    onSave: saveBackfill, onSkip: skipBackfill,
  });

  return { busy: attempt !== null || phase !== "idle", dialog, backfill: backfillNode, handle, open: openDialog };
}
