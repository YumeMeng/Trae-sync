import { useCallback, useRef, useState, type ReactNode } from "react";
import { ConfirmDialog } from "../../../components/shared/ConfirmDialog";
import { notify } from "../../../platform/notify";
import { safeUiErrorMessage } from "../../../utils/safeUiError";

/**
 * 池级页头动作接线（P4c-9b-1 / K-04）：WB 页头四个动作（刷新登录凭据 / 健康检测 /
 * 保存当前登录账号 / 添加账号）在旧 WorkBuddyPanel L172-342 是实装的，这里把命令与文案
 * 经接缝注入后重放同一编排；本文件零产品判断、零 invoke(。
 *
 * 呈现纪律（K-04 第 4 条）：这四个命令都只返回池级 / 客户端级单条回执，没有逐账号
 * healthy / error_code，故一律按旧实现的呈现方式汇报——成功 / 中性 / 失败三档 notify，
 * **绝不拆成逐账号摘要**（notify.summary 属逐账号动作，本文件不用）。
 * 刷新时机逐字沿用旧实现：续期 / 保存 / 添加账号成功后重读本机缓存；健康检测只读不写、
 * 不重读（旧实现结果常驻健康卡，合并页暂无该卡位，只走通知）。
 */

/** 续期回执（旧 L324-342）：renewed=false 即凭据仍充裕、本次未轮换。 */
export interface PoolRenewReceipt { readonly renewed: boolean; readonly access_expires_at_unix_seconds: number }
/** 深度健康回执（旧 L172-191）：live_status 决定成功 / 异常 / 中性三档。 */
export interface PoolHealthReceipt { readonly live_status: string }
/** 捕获回执（旧 L198-213）：成功只取昵称做回执文案。 */
export interface PoolCaptureReceipt { readonly screen_name: string }
/** 登录回执（旧 L288-312）：acceptance=failed 只警告、不回滚、不重试。 */
export interface PoolLoginReceipt {
  readonly screen_name: string;
  readonly acceptance?: "passed" | "failed";
  readonly acceptance_error_code?: string;
}
/** 失败文案：前缀 + 人话原因（safeUiError 词表命中即用，未命中用 fallback）。 */
export interface PoolFailureCopy { readonly prefix: string; readonly fallback: string }
/** 捕获确认弹层（旧 L574-589 的 ConfirmDialog 属性，testid 逐字沿用）。 */
export interface PoolConfirmCopy {
  readonly title: string;
  readonly lines: readonly string[];
  readonly confirmLabel: string;
  readonly busyLabel: string;
  readonly testId: string;
}

/** 单动作登记：动作 id + 命令 + 旧文案（缺省即该动作不注册，动作 id 原样透出）。 */
export interface PagePoolActions {
  readonly renew?: {
    readonly actionId: string;
    readonly run: () => Promise<PoolRenewReceipt>;
    readonly copy: (receipt: PoolRenewReceipt) => { readonly tone: "success" | "info"; readonly text: string };
    readonly failure: PoolFailureCopy;
  };
  readonly health?: {
    readonly actionId: string;
    readonly run: () => Promise<PoolHealthReceipt>;
    readonly copy: (receipt: PoolHealthReceipt) => { readonly tone: "success" | "error" | "info"; readonly text: string };
    readonly failure: PoolFailureCopy;
  };
  readonly capture?: {
    readonly actionId: string;
    readonly run: () => Promise<PoolCaptureReceipt>;
    readonly confirm: PoolConfirmCopy;
    readonly successCopy: (receipt: PoolCaptureReceipt) => string;
    readonly failure: PoolFailureCopy;
  };
  readonly login?: {
    readonly actionId: string;
    /** 等待授权中的「取消等待」动作 id（旧 L517-526 的 workbuddy-login-cancel）。 */
    readonly cancelActionId: string;
    readonly commands: {
      readonly begin: () => Promise<{ readonly attempt_id: number }>;
      readonly complete: (attemptId: number) => Promise<PoolLoginReceipt>;
      readonly cancel: (attemptId: number) => Promise<unknown>;
    };
    readonly successCopy: (receipt: PoolLoginReceipt) => string;
    /** acceptance=failed 的警告正文与补充说明（旧 L296-301 的 title + description）。 */
    readonly acceptanceWarning: (code: string | undefined) => string;
    readonly acceptanceHint: string;
    /** 取消的稳定码（旧 L304 逐字比较 workbuddy_login_cancelled）：命中即中性提示。 */
    readonly cancelCode: string;
    readonly cancelCopy: string;
    /** 其余失败的人话（旧 L79-90 的 WB 登录错误码表 + safeUiError 兜底）。 */
    readonly failureCopy: (reason: unknown) => string;
  };
}

export interface PoolHeaderActionsInput {
  /** 本产品的池级动作登记；null = 未登记（所有页头动作原样透出）。 */
  readonly wiring: PagePoolActions | null;
  /** 成功后重读本机缓存（旧实现每个动作后 refresh(false) 同法）。 */
  readonly onSettled: () => Promise<void>;
}

export interface PoolHeaderActionsResult {
  /** 四路独立忙碌位（builder 据此禁用按钮，页面据此转圈）。 */
  readonly busy: {
    readonly renewing: boolean;
    readonly health: boolean;
    readonly capturing: boolean;
    readonly login: boolean;
  };
  /** 正在跑的页头动作 id（页面据此转圈；未跑即空数组）。 */
  readonly busyActionIds: readonly string[];
  /** 页头动作分发：true = 已由本 hook 处理，调用方不再透出。 */
  readonly handle: (actionId: string) => boolean;
  /** 弹层槽：捕获确认层（未打开即 null）。 */
  readonly dialogs: ReactNode;
}

const IDLE = { renewing: false, health: false, capturing: false, login: false } as const;

export function usePoolHeaderActions(input: PoolHeaderActionsInput): PoolHeaderActionsResult {
  // 入参经 ref 取最新值：handle / 异步收尾保持稳定身份，页面重建不换引用。
  const inputRef = useRef(input);
  inputRef.current = input;
  // 忙碌位以 ref 为准（同一 tick 内重复点击也拦得住），state 只驱动渲染。
  const busyRef = useRef<Record<keyof typeof IDLE, boolean>>({ ...IDLE });
  const [busy, setBusy] = useState<Record<keyof typeof IDLE, boolean>>({ ...IDLE });
  const mark = useCallback((kind: keyof typeof IDLE, value: boolean) => {
    busyRef.current[kind] = value;
    setBusy((previous) => (previous[kind] === value ? previous : { ...previous, [kind]: value }));
  }, []);
  // 捕获确认层：打开即挂载（旧 L496 只开弹层、不动忙碌位）。
  const [captureOpen, setCaptureOpen] = useState(false);
  // 登录单槽 attempt（旧 L134 loginAttemptId）：非空才允许「取消等待」。
  const attemptRef = useRef<number | null>(null);

  // 刷新登录凭据（旧 L324-342）：无弹层；renewed 决定成功还是「仍充裕」中性回执。
  const runRenew = useCallback(async () => {
    const seam = inputRef.current.wiring?.renew;
    if (seam === undefined || busyRef.current.renewing) return;
    mark("renewing", true);
    try {
      const copy = seam.copy(await seam.run());
      if (copy.tone === "success") notify.success(copy.text);
      else notify.info(copy.text);
      await inputRef.current.onSettled();
    } catch (reason: unknown) {
      notify.error(`${seam.failure.prefix}${safeUiErrorMessage(reason, seam.failure.fallback)}`);
    } finally {
      mark("renewing", false);
    }
  }, [mark]);

  // 深度健康检测（旧 L172-191）：只读零写；单条回执按 live_status 三档，不重读数据。
  const runHealth = useCallback(async () => {
    const seam = inputRef.current.wiring?.health;
    if (seam === undefined || busyRef.current.health) return;
    mark("health", true);
    try {
      const copy = seam.copy(await seam.run());
      if (copy.tone === "success") notify.success(copy.text);
      else if (copy.tone === "error") notify.error(copy.text);
      else notify.info(copy.text);
    } catch (reason: unknown) {
      notify.error(`${seam.failure.prefix}${safeUiErrorMessage(reason, seam.failure.fallback)}`);
    } finally {
      mark("health", false);
    }
  }, [mark]);

  // 保存当前登录账号（旧 L198-213）：确认后执行；成功才关层，失败保留弹层可重试。
  const runCapture = useCallback(async () => {
    const seam = inputRef.current.wiring?.capture;
    if (seam === undefined || busyRef.current.capturing) return;
    mark("capturing", true);
    try {
      notify.success(seam.successCopy(await seam.run()));
      setCaptureOpen(false);
      await inputRef.current.onSettled();
    } catch (reason: unknown) {
      notify.error(`${seam.failure.prefix}${safeUiErrorMessage(reason, seam.failure.fallback)}`);
    } finally {
      mark("capturing", false);
    }
  }, [mark]);

  // 添加账号（旧 L288-312）：begin 自动拉起浏览器 → complete 阻塞等授权 → 成功后重读缓存；
  // acceptance=failed 只警告不重试；取消走中性提示（稳定码比较与旧实现逐字一致）。
  const runLogin = useCallback(async () => {
    const seam = inputRef.current.wiring?.login;
    if (seam === undefined || busyRef.current.login) return;
    mark("login", true);
    try {
      const begin = await seam.commands.begin();
      attemptRef.current = begin.attempt_id;
      const receipt = await seam.commands.complete(begin.attempt_id);
      notify.success(seam.successCopy(receipt));
      await inputRef.current.onSettled();
      if (receipt.acceptance === "failed") {
        notify.warning(seam.acceptanceWarning(receipt.acceptance_error_code), seam.acceptanceHint);
      }
    } catch (reason: unknown) {
      if (typeof reason === "string" && reason === seam.cancelCode) notify.info(seam.cancelCopy);
      else notify.error(seam.failureCopy(reason));
    } finally {
      attemptRef.current = null;
      mark("login", false);
    }
  }, [mark]);

  // 动作分发：命中接缝即在本页执行，其余一律交回调用方透出（未登记产品 fail-closed）。
  const handle = useCallback((actionId: string): boolean => {
    const wiring = inputRef.current.wiring;
    if (wiring === null) return false;
    if (wiring.renew !== undefined && actionId === wiring.renew.actionId) { void runRenew(); return true; }
    if (wiring.health !== undefined && actionId === wiring.health.actionId) { void runHealth(); return true; }
    if (wiring.capture !== undefined && actionId === wiring.capture.actionId) { setCaptureOpen(true); return true; }
    if (wiring.login !== undefined && actionId === wiring.login.actionId) { void runLogin(); return true; }
    if (wiring.login !== undefined && actionId === wiring.login.cancelActionId) {
      // 取消等待（旧 L315-318）：令阻塞中的 complete 立即以取消码返回；无 attempt 即无事可做。
      const attempt = attemptRef.current;
      if (attempt !== null) void wiring.login.commands.cancel(attempt).catch(() => undefined);
      return true;
    }
    return false;
  }, [runRenew, runHealth, runLogin]);

  // 捕获确认弹层：文案与 testid 全部来自接缝（旧 L574-589 逐字）。
  const capture = inputRef.current.wiring?.capture;
  const dialogs = captureOpen && capture !== undefined ? (
    <ConfirmDialog
      title={capture.confirm.title}
      lines={capture.confirm.lines}
      confirmLabel={capture.confirm.confirmLabel}
      busyLabel={capture.confirm.busyLabel}
      busy={busy.capturing}
      onCancel={() => { if (!busy.capturing) setCaptureOpen(false); }}
      onConfirm={() => void runCapture()}
      testId={capture.confirm.testId}
    />
  ) : null;

  // 忙碌动作 id：与 busy 同源，页面只把它并进页头忙碌列表（不自行映射动作 id）。
  const wiring = inputRef.current.wiring;
  const busyActionIds = [
    ...(busy.renewing && wiring?.renew !== undefined ? [wiring.renew.actionId] : []),
    ...(busy.health && wiring?.health !== undefined ? [wiring.health.actionId] : []),
    ...(busy.capturing && wiring?.capture !== undefined ? [wiring.capture.actionId] : []),
    ...(busy.login && wiring?.login !== undefined ? [wiring.login.actionId] : []),
  ];
  return { busy, busyActionIds, handle, dialogs };
}
