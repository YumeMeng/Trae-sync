import { useCallback, useMemo, useRef } from "react";
import type { ActionWiring } from "../actionCommands";
import type { AccountActionBusy } from "./useAccountActions";

/**
 * 页头动作的总分发与忙碌位（P4c-9b-1 从 AccountsPage 抽出）：本地刷新 / 批量凭据 /
 * 批量动作 / 池级四动作 / 登录，各自命中即在本页执行，其余原样透出给观察者。
 * 抽出动机：AccountsPage 需保持在 UI-CHARTER 的 300 行预算内；本 hook 零状态、零命令、
 * 零产品判断——产品差异全在注入的 handle 里，页面只把结果接进 AccountsHeader。
 */

/** 页头忙碌位来源：四路各自独立表达，互不冒充（未登记的动作 id 为空串，直接过滤）。 */
export interface HeaderBusySources {
  /** 本地刷新在跑时的动作 id（null = 没在跑）。 */
  readonly pendingId: string | null;
  readonly credentialActionId?: string;
  readonly credentialBusy: boolean;
  readonly batchBusy: AccountActionBusy;
  readonly batchActionIds: ActionWiring["actionIds"];
  /** 池级动作正在跑的 id（WB 的续期 / 健康 / 捕获 / 登录）。 */
  readonly poolBusyIds: readonly string[];
}

/** 组装页头忙碌 id 列表（builder 已算好 disabled，页面只负责转圈）。 */
export function headerBusyIds(sources: HeaderBusySources): readonly string[] {
  return [
    ...(sources.pendingId === null ? [] : [sources.pendingId]),
    ...(sources.credentialBusy && sources.credentialActionId !== undefined ? [sources.credentialActionId] : []),
    ...(sources.batchBusy.credits ? [sources.batchActionIds.credits ?? ""] : []),
    ...(sources.batchBusy.health ? [sources.batchActionIds.health ?? ""] : []),
    ...sources.poolBusyIds,
  ].filter((id) => id !== "");
}

export interface AccountsHeaderWiringInput {
  readonly refreshLocalActionId: string;
  readonly localRefreshBusy: boolean;
  readonly onRefreshLocal: () => void;
  readonly credentialActionId?: string;
  readonly credentialBusy: boolean;
  readonly requestCredential: () => void;
  readonly batch: { readonly handle: (actionId: string) => boolean; readonly busy: AccountActionBusy; readonly actionIds: ActionWiring["actionIds"] };
  readonly pool: { readonly handle: (actionId: string) => boolean; readonly busyActionIds: readonly string[] };
  readonly loginHandle: (actionId: string) => boolean;
  readonly onHeaderAction?: (actionId: string) => void;
}

export interface AccountsHeaderWiring {
  readonly busy: readonly string[];
  readonly handle: (actionId: string) => void;
}

export function useAccountsHeaderWiring(input: AccountsHeaderWiringInput): AccountsHeaderWiring {
  // 入参经 ref 取最新值：handle 保持稳定身份，页面重建快照不换引用。
  const latest = useRef(input);
  latest.current = input;
  const busy = useMemo(
    () => headerBusyIds({
      pendingId: input.localRefreshBusy ? input.refreshLocalActionId : null,
      credentialActionId: input.credentialActionId,
      credentialBusy: input.credentialBusy,
      batchBusy: input.batch.busy,
      batchActionIds: input.batch.actionIds,
      poolBusyIds: input.pool.busyActionIds,
    }),
    [input.batch.busy, input.batch.actionIds, input.credentialActionId, input.credentialBusy,
      input.localRefreshBusy, input.pool.busyActionIds, input.refreshLocalActionId],
  );
  // 分发顺序：本地刷新 → 批量凭据 → 批量动作 → 池级四动作 → 登录；都不认即透出。
  const handle = useCallback((actionId: string) => {
    if (actionId === "") return;
    const current = latest.current;
    if (actionId === current.refreshLocalActionId) { current.onRefreshLocal(); return; }
    if (actionId === current.credentialActionId) { current.requestCredential(); return; }
    if (current.batch.handle(actionId)) return;
    if (current.pool.handle(actionId)) return;
    current.loginHandle(actionId);
    // 透出契约不丢：动作 id 仍回给外部观察者（空态主行动与页头按钮同一 id）。
    current.onHeaderAction?.(actionId);
  }, []);
  return { busy, handle };
}
