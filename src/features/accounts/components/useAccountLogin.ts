import { useCallback, useRef, useState } from "react";
import { notify } from "../../../platform/notify";
import { safeUiErrorMessage } from "../../../utils/safeUiError";

/**
 * 账号 OAuth 登录 hook（P4c-5b）：单槽 attempt、浏览器模式透传、三类收尾语义。命令由调用方
 * 注入（不 import platform/accounts 或 workbuddyAdapter）。收尾文案逐字沿用旧 AccountCenterTrae：
 * login_cancelled 中性 L490-491、login_cancel_too_late 进入收尾 L586-587、login_already_finished
 * / login_not_started 已结束或收尾中 L588-593；acceptance=failed 只警告不重试 L455-462。
 */
export interface AccountLoginReceipt {
  readonly profile_id: string;
  readonly screen_name: string;
  readonly acceptance?: "passed" | "failed";
  readonly acceptance_error_code?: string;
  readonly mobile_full?: string | null; // 缺省/空 = 需补录（页面按总览判定，旧 L480-487）
}

export interface AccountLoginCommands {
  begin(useSystemBrowser: boolean): Promise<{ readonly login_url: string; readonly attempt_id: number }>;
  complete(attemptId: number): Promise<AccountLoginReceipt>;
  cancel(attemptId: number): Promise<void>;
  /** 补录提示钩子；是否真的弹层由页面按「未补录」判定。 */
  onMobileBackfill?(profileId: string): void;
}

/** idle=无登录；beginning=已发起未拿到槽；waiting=等待回调；settling=取消太晚，等真实结果。 */
export type AccountLoginPhase = "idle" | "beginning" | "waiting" | "settling";

export interface AccountLoginResult {
  /** 单槽 attempt 号；null = 当前没有进行中的登录。 */
  readonly attempt: number | null;
  readonly phase: AccountLoginPhase;
  readonly error: string | null; // 失败人话；取消/收尾语义走中性提示（旧 L488-494）
  /** 缺省参数沿用 setBrowserMode 选定的模式（true=本机浏览器，语义透传）。 */
  readonly startLogin: (useSystemBrowser?: boolean) => Promise<AccountLoginReceipt | null>;
  readonly cancelLogin: () => Promise<void>;
  readonly setBrowserMode: (useSystemBrowser: boolean) => void;
}

/** 验收失败原因码 → 括号内短句（旧 L81-95 同一词表，不外露原文）。 */
const ACCEPTANCE_REASON: ReadonlyArray<readonly [RegExp, string]> = [
  [/credential_refresh_failed/i, "换发新登录凭据未通过"],
  [/credential_missing|credential_unavailable|credential_invalid/i, "登录凭据保存后不可用"],
  [/binding_mismatch|auth_mismatch/i, "登录凭据与账号绑定不一致"],
  [/manual_recovery_required/i, "登录凭据需要人工恢复"],
];

function acceptanceReasonText(errorCode: string | undefined): string {
  for (const [pattern, message] of ACCEPTANCE_REASON) if (pattern.test(errorCode ?? "")) return message;
  return errorCode ? safeUiErrorMessage(errorCode, "原因未知").replace(/。$/, "") : "原因未知";
}

export function useAccountLogin(commands: AccountLoginCommands): AccountLoginResult {
  // 命令经 ref 取最新值：startLogin / cancelLogin 保持稳定身份，页面重建入参对象不换引用。
  const commandsRef = useRef(commands);
  commandsRef.current = commands;
  // 单槽 attempt 与忙碌位以 ref 为准（同一 tick 内重复点击也拦得住），state 只驱动渲染。
  const attemptRef = useRef<number | null>(null);
  const busyRef = useRef(false);
  const [attempt, setAttempt] = useState<number | null>(null);
  const [phase, setPhase] = useState<AccountLoginPhase>("idle");
  const [error, setError] = useState<string | null>(null);
  const [systemBrowser, setSystemBrowser] = useState(false);
  const setSlot = useCallback((value: number | null) => { attemptRef.current = value; setAttempt(value); }, []);

  // 并发保护逐字保留旧 L447：登录中重复发起直接忽略。单槽纪律：若仍有残留 attempt（上一轮
  // 取消未落地），先 cancel 再 begin，绝不并行两个 attempt。
  const startLogin = useCallback(async (useSystemBrowser?: boolean) => {
    if (busyRef.current) return null;
    busyRef.current = true;
    setError(null);
    try {
      if (attemptRef.current !== null) {
        await commandsRef.current.cancel(attemptRef.current).catch(() => undefined);
        setSlot(null);
      }
      setPhase("beginning");
      const begin = await commandsRef.current.begin(useSystemBrowser ?? systemBrowser);
      setSlot(begin.attempt_id);
      setPhase("waiting");
      const receipt = await commandsRef.current.complete(begin.attempt_id);
      notify.success(`账号“${receipt.screen_name}”登录成功。`);
      // 首登验收 failed = 账号已保存但同设备换发未通过：如实警告，不自动重试、
      // 不发起第二次授权（旧 L455-462 / L470-477）。
      if (receipt.acceptance === "failed") {
        const why = acceptanceReasonText(receipt.acceptance_error_code);
        notify.warning(`账号“${receipt.screen_name}”已保存，但可持续性验收未通过（${why}）。`,
          "可稍后在账号详情手动刷新凭据或重新登录。");
      }
      if (!receipt.mobile_full) commandsRef.current.onMobileBackfill?.(receipt.profile_id);
      return receipt;
    } catch (reason: unknown) {
      // 取消不是错误：中性提示收尾（浏览器被关闭同样走此路径，旧 L489-491）。
      if (typeof reason === "string" && reason === "login_cancelled") {
        notify.info("登录已取消；若浏览器已关闭或未完成授权，可重新发起登录。");
        return null;
      }
      const text = safeUiErrorMessage(reason, "登录未完成，请重新发起登录。");
      setError(text); notify.error(text);
      return null;
    } finally {
      setSlot(null); setPhase("idle"); busyRef.current = false; // 单槽随本次登录收尾清空
    }
  }, [setSlot, systemBrowser]);

  // 取消请求失败的三类收尾语义（旧 L579-598）：成功取消不打扰（complete 以 login_cancelled
  // 收尾）；其余三类各自提示，不改 attempt 槽（真实结果仍由 complete 落地）。
  const cancelLogin = useCallback(async () => {
    const current = attemptRef.current;
    if (current === null) return;
    try {
      await commandsRef.current.cancel(current);
    } catch (reason: unknown) {
      const code = typeof reason === "string" ? reason : "";
      if (code === "login_cancel_too_late") { setPhase("settling"); notify.info("登录已进入收尾阶段，正在等待真实结果。"); }
      else if (code === "login_already_finished" || code === "login_not_started") notify.info("登录已结束或正在收尾，请等待页面结果更新。");
      else notify.warning("取消登录未成功，登录等待将继续；完成或超时后会自动收尾。");
    }
  }, []);

  const setBrowserMode = useCallback((useSystemBrowser: boolean) => setSystemBrowser(useSystemBrowser), []);

  return { attempt, phase, error, startLogin, cancelLogin, setBrowserMode };
}
