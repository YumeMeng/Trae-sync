import { useCallback, useLayoutEffect, useRef, useState } from "react";
import { useTauriEvents } from "../../../hooks/useTauriEvents";
import type { BatchProgressEvent, BatchProgressKind } from "../../../types/batchProgress";

/**
 * 账号页批量进度 hook（P4c-5a）：订阅三路逐账号进度事件，产品切换自动清态。
 * 事件名与校验规则逐条照旧实现（AccountCenterTrae L65-78 / L315-333）：载荷来源
 * 产品不符即丢弃；带 kind 时校验，未知或不匹配 kind 一律丢弃（旧载荷缺 kind 时
 * 按事件名降级接受）。本 hook 只管进度，不发命令（动作编排在 P4c-5b）。
 */

/** 单链进度：第 completed/N 个账号名（进度文案所需最小状态）。 */
export interface AccountBatchProgress { readonly completed: number; readonly total: number; readonly name: string }
/** 三链进度表：kind 为键，缺键即该链无进度。 */
export type AccountBatchProgressMap = Partial<Record<BatchProgressKind, AccountBatchProgress>>;

/** 载荷校验：来源产品不符、或 kind 存在且不等于预期（含未知 kind）→ 丢弃返回 null。 */
function toProgress(
  payload: BatchProgressEvent, expected: BatchProgressKind, productId: string,
): AccountBatchProgress | null {
  if (payload.product_id !== productId) return null;
  if (payload.kind !== undefined && payload.kind !== expected) return null;
  return { completed: payload.completed, total: payload.total, name: payload.screen_name };
}

export interface AccountBatchProgressResult {
  /** 三链进度（按 kind 判别）；空对象 = 无进行中批次。 */
  readonly progress: AccountBatchProgressMap;
  /** 清进度：不传 kind 清全部（产品切换 / 批次结束），传 kind 只清该链。 */
  readonly reset: (kind?: BatchProgressKind) => void;
}

/**
 * @param productId 进度所属产品的协议值：TRAE 账号域为 `work_cn`（旧 L54-56 / L115
 *   的链路标识，不是产品切换概念），适配器面取该适配器的 productId。WB 无三路进度
 *   事件（旧 WorkBuddyPanel 未订阅），传入其产品 id 时载荷自然全部丢弃。
 */
export function useAccountBatchProgress(productId: string): AccountBatchProgressResult {
  const [progress, setProgress] = useState<AccountBatchProgressMap>({});
  // 事件回调经 ref 取当前产品：产品切换不重订阅，只让不匹配载荷在落地时被丢弃。
  const productIdRef = useRef(productId);
  productIdRef.current = productId;
  // 按 kind 写入单条进度；同值短路避免多余渲染（旧 L156-165 同法）。
  const setKind = useCallback((kind: BatchProgressKind, next: AccountBatchProgress | null) => {
    setProgress((current) => {
      if ((current[kind] ?? null) === next) return current;
      const copy = { ...current };
      if (next) copy[kind] = next; else delete copy[kind];
      return copy;
    });
  }, []);
  // 产品切换：在浏览器绘制前清空，旧产品进度（含账号名）不得泄露到新首帧（旧 L167-169）。
  useLayoutEffect(() => {
    setProgress((current) => (Object.keys(current).length > 0 ? {} : current));
  }, [productId]);
  // 单链落库：事件名对应预期 kind，未知 kind / 来源不符由 toProgress 丢弃。
  const handle = useCallback((payload: BatchProgressEvent, kind: BatchProgressKind) => {
    const next = toProgress(payload, kind, productIdRef.current);
    if (next) setKind(kind, next);
  }, [setKind]);
  // 挂载期订阅、卸载退订；页面隐藏仍保持挂载，事件照收，重新显示即最新进度。
  useTauriEvents({
    "credential-refresh-progress": (payload: BatchProgressEvent) => handle(payload, "credential_refresh"),
    "health-check-progress": (payload: BatchProgressEvent) => handle(payload, "health_check"),
    "credits-refresh-progress": (payload: BatchProgressEvent) => handle(payload, "credits_refresh"),
  }, []);
  // 清进度：缺省清全部（保留空对象身份，无进度时不触发渲染）。
  const reset = useCallback((kind?: BatchProgressKind) => {
    setProgress((current) => {
      if (kind === undefined) return Object.keys(current).length > 0 ? {} : current;
      if (!(kind in current)) return current;
      const copy = { ...current };
      delete copy[kind];
      return copy;
    });
  }, []);
  return { progress, reset };
}
