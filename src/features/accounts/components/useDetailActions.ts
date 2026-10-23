import { createElement, useCallback, useRef, useState, type ReactNode } from "react";
import { ConfirmDialog } from "../../../components/shared/ConfirmDialog";
import { notify } from "../../../platform/notify";
import { safeUiErrorMessage } from "../../../utils/safeUiError";
import { ACCOUNT_TEXTS } from "../config";
import type {
  DetailActionTarget, DetailConfirmCopy, PageDetailActions,
} from "../detailActionCommands";

/**
 * 详情页动作分发（P4c-6b-2e-1）：刷新额度 / 立即签到 / 重新登录 / 重置设备 / 删除。
 * 命令与文案全部由 detailActionCommands 的接缝注入（本文件零 platform 命令、零产品判断、
 * 零 invoke）。语义逐字沿用旧实现：额度 AccountDetail L117-131、签到 L218-237、重登
 * L239-244（= AccountCenterTrae L898 handleLogin(false)，同一 OAuth 流程）、重置设备
 * L142-144/L203-208、删除 L146-149/L209-215 与 WorkBuddyAccountDetail L131-157。
 *
 * 删除的确认由危险区 DetailDangerAction 承担（文案由本 hook 经 dangerConfirm 注入，逐字旧实现）
 * ——ADR-0018 单次确认纪律，不再叠第二个弹层。重置设备不在危险区，确认弹层由本 hook 产出。
 * 未登记命令的动作一律 fail-closed：handle 返回 false，动作原样透出给外部观察者。
 */

export interface DetailActionsInput {
  /** 本产品接缝；undefined = 未登记（所有详情动作只透出）。 */
  readonly seam: PageDetailActions | undefined;
  readonly rowId: string;
  /** 本产品数据面（动作触发时才取，保证用最新快照解析目标）。 */
  readonly readPayload: () => unknown;
  /** 成功后重读本机缓存（旧实现每个动作后 refreshOverview / refresh 同法）。 */
  readonly onSettled: () => Promise<void>;
  /** 删除成功后关闭详情回列表（旧 L213 onBack 同法）。 */
  readonly onClose: () => void;
  /** 重新登录 = 打开登录弹层（复用 6b-2c 流程）；false = 未登记，动作只透出。 */
  readonly onRelogin: (actionId: string) => boolean;
  /** 登录等待中：给「重新登录」按钮转圈并禁用（旧 L472-473 loginBusy 同法）。 */
  readonly reloginBusy?: boolean;
}

export interface DetailActionsResult {
  /** 正在执行的详情动作 id：只影响被点按钮（旧实现 busy 单值锁）。 */
  readonly busy: readonly string[];
  /** 重置设备的二次确认弹层（挂载即打开）。 */
  readonly dialogs: ReactNode;
  /** 分发：true = 本 hook 已处理；false = 未登记，调用方透出。 */
  readonly handle: (actionId: string) => boolean;
  /** 危险区动作的确认文案（逐字旧实现）；null = 沿用危险区 hint。 */
  readonly dangerConfirm: (actionId: string) => DetailConfirmCopy | null;
}

/** 额度格式化：保留 1 位小数，整数不带小数点（旧 AccountDetail L572-576 同口径）。 */
const formatCredits = (value: number): string => {
  const rounded = Math.round(value * 10) / 10;
  return Number.isInteger(rounded) ? String(rounded) : rounded.toFixed(1);
};

export function useDetailActions(input: DetailActionsInput): DetailActionsResult {
  const inputRef = useRef(input);
  inputRef.current = input;
  // 忙碌位以 ref 为准（同 tick 重复点击也拦得住），state 只驱动渲染。
  const busyRef = useRef(false);
  const [pending, setPending] = useState<string | null>(null);
  const [confirm, setConfirm] = useState<{ readonly actionId: string; readonly copy: DetailConfirmCopy } | null>(null);

  // 统一执行骨架（旧 runAction L86-95）：busy 锁 + 失败 notify.error 且不清数据。
  const run = useCallback(async (actionId: string, task: () => Promise<void>, copy?: DetailConfirmCopy) => {
    if (busyRef.current) return;
    busyRef.current = true;
    setPending(actionId);
    try {
      await task();
    } catch (reason: unknown) {
      notify.error(`${copy?.failurePrefix ?? ""}${safeUiErrorMessage(reason, copy?.failureFallback ?? ACCOUNT_TEXTS.failureFallback)}`);
    } finally {
      busyRef.current = false;
      setPending(null);
    }
  }, []);

  // 目标每次触发都重新解析（数据面可能已刷新）；找不到即 null（不猜目标）。
  const targetOf = useCallback((): DetailActionTarget | null => {
    const { seam, readPayload, rowId } = inputRef.current;
    return seam === undefined ? null : seam.targetOf(readPayload(), rowId);
  }, []);

  // 刷新额度：只读查询（不消耗签到资格）→ 重读缓存 → 旧成功回执（L117-131 逐字）。
  const runCredits = useCallback(async (seam: PageDetailActions, target: DetailActionTarget) => {
    const action = seam.refreshCredits;
    if (action === undefined) return;
    await run(action.id, async () => {
      const result = (await action.run(target.profileId))[0];
      if (result?.error_code) throw new Error(result.error_code);
      await inputRef.current.onSettled();
      const usage = result?.usage_remaining_credits == null ? "未知" : formatCredits(result.usage_remaining_credits);
      notify.success(`额度已更新：模型积分 ${usage}（今日${result?.checked_in ? "已签" : "未签"}）。`);
    });
  }, [run]);

  // 立即签到：单账号批次 → 重读缓存 → 三支回执（旧 L218-237 逐字）。
  const runCheckin = useCallback(async (seam: PageDetailActions, target: DetailActionTarget) => {
    const action = seam.checkin;
    if (action === undefined) return;
    await run(action.id, async () => {
      const first = (await action.run(target.profileId)).results[0];
      await inputRef.current.onSettled();
      if (first?.outcome === "claimed") notify.success(ACCOUNT_TEXTS.checkinClaimed);
      else if (first?.outcome === "already_checked_in") notify.info(ACCOUNT_TEXTS.checkinAlready);
      else if (first?.detail_code) throw new Error(first.detail_code); // 业务码交给 safeUiErrorMessage
      else throw new Error("checkin_failed");
    });
  }, [run]);

  // 删除（旧 L209-215 / WB L131-157）：确认已由 DetailDangerAction 完成，直接执行；
  // 成功后先重读缓存再回列表；WB 回执按回执态报（无回执的 TRAE 不发明成功文案）。
  const runRemove = useCallback(async (seam: PageDetailActions, target: DetailActionTarget) => {
    const action = seam.remove;
    if (action === undefined) return;
    await run(action.id, async () => {
      const receipt = await action.run(target);
      await inputRef.current.onSettled();
      inputRef.current.onClose();
      if (receipt !== null) (receipt.tone === "warning" ? notify.warning : notify.success)(receipt.text);
    }, action.confirm?.(target));
  }, [run]);

  // 分发：命中已登记命令即执行 / 开确认弹层；未登记返回 false（调用方原样透出）。
  const handle = useCallback((actionId: string): boolean => {
    const { seam } = inputRef.current;
    if (actionId === "" || seam === undefined) return false;
    const target = targetOf();
    if (target === null) return false;
    if (seam.relogin === actionId) return inputRef.current.onRelogin(actionId);
    if (seam.refreshCredits?.id === actionId) { void runCredits(seam, target); return true; }
    if (seam.checkin?.id === actionId) { void runCheckin(seam, target); return true; }
    if (seam.remove?.id === actionId) { void runRemove(seam, target); return true; }
    const reset = seam.resetDevice;
    if (reset !== undefined && reset.id === actionId) {
      setConfirm({ actionId, copy: reset.confirm(target) });
      return true;
    }
    return false;
  }, [runCheckin, runCredits, runRemove, targetOf]);

  // 确认文案只在危险区动作上注入（重置设备的确认弹层由本 hook 自己渲染）。
  const dangerConfirm = useCallback((actionId: string): DetailConfirmCopy | null => {
    const { seam } = inputRef.current;
    const target = targetOf();
    if (seam?.remove === undefined || target === null || seam.remove.id !== actionId) return null;
    return seam.remove.confirm?.(target) ?? null;
  }, [targetOf]);

  const cancelConfirm = useCallback(() => {
    if (pending !== null) return; // 执行中不关层（ConfirmDialog busy 契约同法）
    setConfirm(null);
  }, [pending]);

  // 确认后执行（旧 executePendingAction L187-216）：先关弹层再跑命令，失败走 notify.error。
  const runConfirmed = useCallback(() => {
    const request = confirm;
    if (request === null) return;
    const { seam, onSettled } = inputRef.current;
    const target = targetOf();
    const reset = seam?.resetDevice;
    setConfirm(null);
    if (reset === undefined || target === null) return;
    void run(request.actionId, async () => {
      await reset.run(target.profileId);
      await onSettled();
      notify.success(ACCOUNT_TEXTS.deviceReset);
    }, request.copy);
  }, [confirm, run, targetOf]);

  const dialogs = confirm === null ? null : createElement(ConfirmDialog, {
    title: confirm.copy.title, lines: confirm.copy.lines, confirmLabel: confirm.copy.confirmLabel,
    busyLabel: confirm.copy.busyLabel, busy: pending === confirm.actionId, danger: confirm.copy.danger,
    onCancel: cancelConfirm, onConfirm: runConfirmed, testId: confirm.actionId,
  });

  // 忙碌位：正在执行的动作 + 登录等待中的「重新登录」，各自只影响自己的按钮。
  const busy = [pending, input.reloginBusy === true ? input.seam?.relogin ?? null : null].filter((id): id is string => id !== null && id !== "");
  return { busy, dialogs, handle, dangerConfirm };
}
