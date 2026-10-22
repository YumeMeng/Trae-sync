import { useCallback, useRef, useState } from "react";
import { notify, type NotifyIssue } from "../../../platform/notify";
import type { CredentialRefreshEntryDto } from "../../../types/account_switch";
import { safeUiErrorMessage } from "../../../utils/safeUiError";

/**
 * 批量刷新登录凭据的二次确认编排（P4c-6c-1 接线补齐）。
 *
 * 契约逐字沿用旧 AccountCenterTrae（L680-729）：入口只开弹层，确认后才执行；
 * 执行体逐个向服务端换发并写回本机登录信息，结果走 notify.summary 汇总
 * （成功数 + 异常逐条「名称：原因」），失败走 notify.error。命令由调用方注入，
 * 本 hook 不 import platform/accounts，也不判断产品。
 */

/** 刷新目标（确认后要换发的账号）：id + 展示名（摘要里的名称优先用备注名）。 */
export interface CredentialRefreshTarget {
  readonly profileId: string;
  readonly displayName: string;
}

export interface CredentialRefreshInput {
  /** 换发命令；undefined = 本产品/本面不提供该入口（fail-closed，不执行也不开弹层）。 */
  readonly refresh?: (profileIds: readonly string[]) => Promise<readonly CredentialRefreshEntryDto[]>;
  /** 目标解析（确认时才取，保证用最新快照）；空数组即无事可做。 */
  readonly targets: () => readonly CredentialRefreshTarget[];
  /** 成功后重读本机缓存（不联网）。 */
  readonly onRefreshed?: () => Promise<void>;
}

export interface CredentialRefreshResult {
  /** 二次确认弹层是否打开。 */
  readonly open: boolean;
  /** 换发进行中（弹层两侧按钮禁用、页头按钮转圈）。 */
  readonly busy: boolean;
  /** 打开确认弹层；无命令或无目标时静默忽略。 */
  readonly request: () => void;
  readonly cancel: () => void;
  /** 确认后执行；返回是否真的跑了一次。 */
  readonly confirm: () => Promise<boolean>;
}

export function useCredentialRefresh(input: CredentialRefreshInput): CredentialRefreshResult {
  // 入参经 ref 取最新值：request/confirm 保持稳定身份，页面重建不换引用。
  const inputRef = useRef(input);
  inputRef.current = input;
  // 忙碌位以 ref 为准：同一 tick 内重复确认也拦得住；state 只驱动渲染。
  const busyRef = useRef(false);
  const [open, setOpen] = useState(false);
  const [busy, setBusy] = useState(false);

  const request = useCallback(() => {
    if (inputRef.current.refresh === undefined || busyRef.current) return;
    if (inputRef.current.targets().length === 0) return;
    setOpen(true);
  }, []);
  const cancel = useCallback(() => setOpen(false), []);

  const confirm = useCallback(async () => {
    setOpen(false);
    const { refresh, targets, onRefreshed } = inputRef.current;
    if (refresh === undefined || busyRef.current) return false;
    const rows = targets();
    if (rows.length === 0) return false;
    busyRef.current = true;
    setBusy(true);
    try {
      const results = await refresh(rows.map((row) => row.profileId));
      const nameOf = new Map(rows.map((row) => [row.profileId, row.displayName]));
      const issues: NotifyIssue[] = results.filter((item) => !item.refreshed).map((item) => ({
        displayName: (nameOf.get(item.profile_id) ?? item.screen_name) || "该账号",
        reason: safeUiErrorMessage(item.error_code, "登录凭据刷新未完成，请稍后重试。"),
      }));
      notify.summary("凭据刷新完成", issues, results.filter((item) => item.refreshed).length);
      await onRefreshed?.();
      return true;
    } catch (reason: unknown) {
      notify.error(safeUiErrorMessage(reason, "凭据刷新未完成，请稍后重试。"));
      return false;
    } finally {
      busyRef.current = false;
      setBusy(false);
    }
  }, []);

  return { open, busy, request, cancel, confirm };
}
