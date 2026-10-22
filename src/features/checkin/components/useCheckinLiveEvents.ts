import { useEffect, useRef } from "react";
import { listen } from "@tauri-apps/api/event";

/**
 * 签到事件监听（P4b-3）：四路后端事件集中订阅一次，页面只持有状态。
 *
 * - checkin-progress：手动批次逐账号结果（串行批次实时上屏）
 * - checkin-phase：账号间错峰等待（inter_wait，剩余秒数由前端本地递减）
 * - auto-checkin-progress：自动批次逐账号结果 + 今日台账实时回填
 * - auto-checkin-finished：自动批次完成（台账翻完成态 + 重读快照）
 * - workbuddy-auto-checkin-progress / workbuddy-auto-checkin-finished：
 *   WorkBuddy 后端的同名事件（载荷与 TRAE 同构），共用同一回调——
 *   两个产品的事件名都监听，页面内不做产品判断。
 *
 * 回调经 ref 取最新值，订阅只在 active 变化时重建——避免每次渲染重新注册。
 */

/** 逐账号进度事件载荷（checkin-progress / auto-checkin-progress 同构）。 */
export interface CheckinProgressPayload {
  readonly profile_id: string;
  readonly screen_name?: string;
  readonly outcome: string;
  readonly detail_code: string | null;
  readonly completed: number;
  readonly total: number;
}

/** 阶段事件载荷（checkin-phase）：phase = inter_wait 时携带剩余秒数。 */
export interface CheckinPhasePayload {
  readonly profile_id: string;
  readonly phase: string;
  readonly remaining_secs: number;
}

/** 自动批次完成事件载荷（auto-checkin-finished）。 */
export interface AutoCheckinFinishedPayload {
  readonly total: number;
  readonly completed: number;
  readonly failed: number;
  readonly skipped: number;
}

export interface CheckinLiveHandlers {
  onProgress: (payload: CheckinProgressPayload) => void;
  onPhase: (payload: CheckinPhasePayload) => void;
  onAutoProgress: (payload: CheckinProgressPayload) => void;
  onAutoFinished: (payload: AutoCheckinFinishedPayload) => void;
}

export function useCheckinLiveEvents(active: boolean, handlers: CheckinLiveHandlers): void {
  const latest = useRef(handlers);
  latest.current = handlers;

  useEffect(() => {
    if (!active) return;
    let cancelled = false;
    const unlisteners: Array<() => void> = [];
    const subscribe = <T,>(event: string, deliver: (payload: T) => void) => {
      void listen<T>(event, (incoming) => deliver(incoming.payload))
        .then((fn) => {
          if (cancelled) fn();
          else unlisteners.push(fn);
        })
        .catch(() => undefined);
    };
    subscribe<CheckinProgressPayload>("checkin-progress", (payload) => latest.current.onProgress(payload));
    subscribe<CheckinPhasePayload>("checkin-phase", (payload) => latest.current.onPhase(payload));
    subscribe<CheckinProgressPayload>("auto-checkin-progress", (payload) => latest.current.onAutoProgress(payload));
    subscribe<AutoCheckinFinishedPayload>("auto-checkin-finished", (payload) => latest.current.onAutoFinished(payload));
    // WorkBuddy 后端事件名（workbuddy_schedule.rs 的 emit 口径），同一回调。
    subscribe<CheckinProgressPayload>("workbuddy-auto-checkin-progress", (payload) => latest.current.onAutoProgress(payload));
    subscribe<AutoCheckinFinishedPayload>("workbuddy-auto-checkin-finished", (payload) => latest.current.onAutoFinished(payload));
    return () => {
      cancelled = true;
      unlisteners.forEach((fn) => fn());
    };
  }, [active]);
}
