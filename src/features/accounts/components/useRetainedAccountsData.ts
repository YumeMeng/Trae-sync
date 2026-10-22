import { useEffect, useRef } from "react";
import { notify } from "../../../platform/notify";
import { ACCOUNT_TEXTS } from "../config";
import type { AccountsData } from "./useAccountsSnapshot";

/**
 * 「刷新失败保留旧数据」的页面侧保留位（P4c-6c-1 从 AccountsPage 抽出）。
 *
 * 语义（旧 G15，P4c-6b-2a 定稿）：读取 hook 的纪律是「失败即清快照」，这对首次
 * 加载是对的（不能用空账号冒充失败）；但已成功读到过数据时，刷新失败应继续渲染
 * 旧数据 + 一条通知，而不是退回错误卡。
 *
 * 两条防串数据纪律原样保留：
 * 1. 保留位按 scope（产品 + 可见性）隔离，切产品/隐藏即清空；
 * 2. 快照必须含本产品的载荷键才算数（迟到的上一产品快照一律不认）。
 */
export interface RetainedAccountsData {
  /** 本次读取到的本产品快照；null = 加载中 / 已清态 / 载荷键不符。 */
  readonly fresh: AccountsData | null;
  /** 渲染用数据：fresh 优先，缺失时回落保留的旧数据；两者皆无才落错误卡。 */
  readonly shown: AccountsData | null;
}

/**
 * @param data hook 返回的快照（可能含别的产品的迟到数据）
 * @param payloadKey 本产品接缝的载荷键
 * @param scope 保留位作用域（产品 + 可见性）
 * @param error 读取失败原因；null = 无失败
 * @param refreshing 本地刷新进行中（进行中的 error 不提示，收尾后才提示）
 */
export function useRetainedAccountsData(
  data: AccountsData | null, payloadKey: string, scope: string, error: string | null, refreshing: boolean,
): RetainedAccountsData {
  const payloadOf = (value: AccountsData): unknown => (value as unknown as Record<string, unknown>)[payloadKey];
  const fresh: AccountsData | null = data !== null && payloadOf(data) !== undefined ? data : null;
  const kept = useRef<{ scope: string; data: AccountsData | null }>({ scope, data: null });
  if (kept.current.scope !== scope) kept.current = { scope, data: null };
  if (fresh !== null && kept.current.data !== fresh) kept.current = { scope, data: fresh };
  const keptData = kept.current.data;
  // 失败提示：只在刷新收尾后提示（进行中的 error 不提示），且每次点击最多提示一次
  // ——notified 在错误变化时重置，重复失败仍有反馈。
  const notified = useRef<string | null>(null);
  useEffect(() => {
    if (error === null) { notified.current = null; return; }
    if (refreshing || keptData === null || notified.current === error) return;
    notified.current = error;
    notify.error(error, ACCOUNT_TEXTS.refreshFailureKept);
  }, [error, keptData, refreshing]);
  return { fresh, shown: fresh ?? keptData };
}
