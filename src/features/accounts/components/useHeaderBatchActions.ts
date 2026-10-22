import { useCallback, useRef } from "react";
import type { BatchProgressKind } from "../../../types/batchProgress";
import { UNAVAILABLE_ACTION_COMMANDS, type ActionWiring } from "../actionCommands";
import { useAccountActions, type AccountActionBusy } from "./useAccountActions";

/**
 * 页头批量动作接线（P4c-6b-2b）：把 actionCommands 的装配结果交给 useAccountActions，
 * 并把页头动作 id 映射到两条执行链（额度刷新 / 健康检测）。
 *
 * 只有装配里登记了动作 id 的产品才分发（未登记 = 页头无该入口，一律交回调用方透出）；
 * 命令本身的产品差异全在 actionCommands，本 hook 不判断产品、不发命令。
 */

/** 动作 ↔ 进度链（旧实现批次开始前清残留、收尾清本条，避免按钮被陈旧进度永久禁用）。 */
const PROGRESS_KIND: Record<"credits" | "health", BatchProgressKind> = {
  credits: "credits_refresh", health: "health_check",
};

export interface HeaderBatchActionsInput {
  /** 本产品的动作装配；null = 无数据面或未登记产品（占位命令永不执行）。 */
  readonly wiring: ActionWiring | null;
  /** 目标解析（调用时取最新快照：选中行优先，否则全量）。 */
  readonly targets: () => readonly string[];
  /** 动作真正执行后重读本机缓存（旧实现每个动作后 refreshOverview 同法）。 */
  readonly onSettled: () => Promise<void>;
  /** 清某条进度链（旧实现批次前后各清一次）。 */
  readonly resetProgress: (kind: BatchProgressKind) => void;
}

export interface HeaderBatchActions {
  /** 三链忙碌位（页面据此转圈，builder 据此算禁用）。 */
  readonly busy: AccountActionBusy;
  /** 页头动作分发：true = 已由本 hook 处理，调用方不再透出。 */
  readonly handle: (actionId: string) => boolean;
}

export function useHeaderBatchActions(input: HeaderBatchActionsInput): HeaderBatchActions {
  const { busy, runRefreshCredits, runHealthCheck } = useAccountActions(
    input.wiring?.commands ?? UNAVAILABLE_ACTION_COMMANDS,
  );
  // 入参经 ref 取最新值：handle 身份稳定，不因页面重建快照而换引用。
  const latest = useRef(input);
  latest.current = input;
  const creditsId = input.wiring?.actionIds.credits ?? "";
  const healthId = input.wiring?.actionIds.health ?? "";
  const run = useCallback(async (kind: "credits" | "health") => {
    const { targets, onSettled, resetProgress } = latest.current;
    const ids = targets();
    resetProgress(PROGRESS_KIND[kind]); // 清上批残留：新批次的进度由事件重新落库
    const result = kind === "credits" ? await runRefreshCredits(ids) : await runHealthCheck(ids);
    resetProgress(PROGRESS_KIND[kind]); // 收尾同样清：末条事件可能因失败/空目标而缺席
    // 只有真正执行过才重读：忙碌/空目标/失败都已各自通知，不额外空跑一次读取。
    if (result !== null) await onSettled();
  }, [runRefreshCredits, runHealthCheck]);
  const handle = useCallback((actionId: string): boolean => {
    if (actionId === creditsId) { void run("credits"); return true; }
    if (actionId === healthId) { void run("health"); return true; }
    return false;
  }, [creditsId, healthId, run]);
  return { busy, handle };
}
