import { useCallback, useState } from "react";
import { cancelCheckin, runCheckin } from "../../../platform/checkin";
import { notify } from "../../../platform/notify";
import { safeUiErrorMessage } from "../../../utils/safeUiError";
import type { CheckinPhasePayload, CheckinProgressPayload } from "./useCheckinLiveEvents";

/**
 * 签到批次执行状态（P4b-3）：勾选/执行/进度游标/错峰倒计时集中在这里，
 * 页面只负责把事件喂进来并渲染视图模型。
 *
 * 进度游标：后端串行批次逐账号推送 completed（权威游标），无事件时回落
 * 已完成行数——两者取大，保证行阶段推进不倒退。
 */

/** 行结果（本批次逐行 outcome；执行期间由进度事件实时写入）。 */
export type RowResult = { readonly outcome: string; readonly detail_code: string | null };

/** 通知文案（来自视图模型的 texts，产品差异不落在这里）。 */
export interface CheckinRunTexts {
  readonly notificationPrefix: string;
  readonly runSuccess: string;
  readonly runPartial: string;
  readonly runFailure: string;
}

export interface CheckinRunOptions {
  productId: string;
  texts: CheckinRunTexts;
  /** 批次结束后重读快照（签到写回今日状态与积分缓存）。 */
  reload: () => Promise<void>;
}

export interface CheckinRun {
  readonly busy: boolean;
  readonly results: Record<string, RowResult>;
  /** 当前执行到的行（进度游标；null = 不在执行）。 */
  readonly cursorId: string | null;
  readonly progress: { readonly completed: number; readonly total: number };
  /** 账号间错峰剩余秒数（profileId → 秒；批次结束清空）。 */
  readonly waits: Record<string, number>;
  readonly run: (ids: readonly string[]) => Promise<void>;
  readonly cancel: () => Promise<void>;
  readonly onProgress: (payload: CheckinProgressPayload) => void;
  readonly onPhase: (payload: CheckinPhasePayload) => void;
}

export function useCheckinRun({ productId, texts, reload }: CheckinRunOptions): CheckinRun {
  const [busy, setBusy] = useState(false);
  const [results, setResults] = useState<Record<string, RowResult>>({});
  // 本批次执行顺序（进度游标依据）与实时完成数。
  const [runIds, setRunIds] = useState<readonly string[]>([]);
  const [liveCompleted, setLiveCompleted] = useState(0);
  const [waits, setWaits] = useState<Record<string, number>>({});

  // 通用执行入口：全签 / 补签 / 所选 / 行内单签共用同一批次通道。
  // ids 顺序即执行顺序（后端串行），同时作为进度游标的依据。
  const run = useCallback(async (ids: readonly string[]) => {
    if (busy || ids.length === 0) return;
    setBusy(true);
    setResults({});
    setLiveCompleted(0);
    setWaits({});
    setRunIds(ids);
    try {
      const outcome = await runCheckin(productId, ids);
      setResults(Object.fromEntries(
        outcome.results.map((item) => [
          item.profile_id,
          { outcome: item.outcome, detail_code: item.detail_code },
        ]),
      ));
      const prefix = texts.notificationPrefix;
      if (outcome.failed === 0) notify.success(`${prefix}：${texts.runSuccess}`);
      else notify.warning(`${prefix}：${texts.runPartial}`);
      // 签到会写回今日状态与积分缓存：重读快照让行内状态同步。
      await reload();
    } catch (reason: unknown) {
      notify.error(`${texts.notificationPrefix}：${safeUiErrorMessage(reason, texts.runFailure)}`);
    } finally {
      setBusy(false);
      setRunIds([]);
      setWaits({});
    }
  }, [busy, productId, reload, texts]);

  const cancel = useCallback(async () => {
    await cancelCheckin(productId);
  }, [productId]);

  // 逐账号进度：结果落行 + 权威游标推进。
  const onProgress = useCallback((payload: CheckinProgressPayload) => {
    setLiveCompleted(payload.completed);
    setResults((current) => ({
      ...current,
      [payload.profile_id]: { outcome: payload.outcome, detail_code: payload.detail_code },
    }));
  }, []);

  // 阶段事件：仅账号间错峰等待需要行内倒计时（冷却阶段已随决议 6 退役）。
  const onPhase = useCallback((payload: CheckinPhasePayload) => {
    if (payload.phase !== "inter_wait") return;
    setWaits((current) => ({ ...current, [payload.profile_id]: payload.remaining_secs }));
  }, []);

  const total = runIds.length;
  const completed = Math.min(Math.max(liveCompleted, Object.keys(results).length), total);
  const cursorId = busy && total > 0 ? runIds[Math.min(completed, total - 1)] : null;

  return {
    busy,
    results,
    cursorId,
    progress: { completed, total },
    waits,
    run,
    cancel,
    onProgress,
    onPhase,
  };
}
