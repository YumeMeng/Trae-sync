import { createElement, useCallback, useRef, useState, type ReactNode } from "react";
import { ConfirmDialog } from "../../../components/shared/ConfirmDialog";
import { notify } from "../../../platform/notify";
import { safeUiErrorMessage } from "../../../utils/safeUiError";
import { ACCOUNT_TEXTS } from "../config";

/**
 * G11 手机号查重确认层（P4c-6b-2e-3）：第三层查重命中后弹统一确认层，确认后用同一命令强行写入。
 *
 * 判定口径逐字沿用旧 AccountCenterTrae L505-521：命中 = 账号总览内**其他账号**的 mobile_full
 * 与输入完全相同（后端 set_account_mobile 只做格式与脱敏号首尾比对，**没有查重错误码**——
 * 查重本来就是客户端第三层，故按数据面判定而非按错误码判定）。占用者展示名走备注名优先。
 *
 * 弹层形态与 testid（account-mobile-duplicate-confirm 及 -cancel / -confirm）逐字沿用旧 L798-812；
 * 本文件零 platform 命令、零 invoke：写入出口只有 request.write（由调用方注入，列表补录与详情行内共用）。
 */
export interface DuplicateMobileConfirmRequest {
  /** 占用该手机号的账号展示名（旧 L516 effectiveDisplayName(owner)）。 */
  readonly ownerName: string;
  /** 确认后的写入（同一命令强行写入；旧 confirmDuplicateMobile L535 同法）。 */
  readonly write: () => Promise<void>;
  /** 写入成功后的重读（旧 refreshOverview）。 */
  readonly onSettled: () => Promise<void>;
  /** 成功回执（旧 L538 逐字）。 */
  readonly successCopy: string;
  /** 失败兜底文案；缺省 = 通用「操作未完成，请稍后重试。」 */
  readonly failureFallback?: string;
}

export interface DuplicateMobileConfirm {
  /** 弹层并等用户决定：true = 已强行写入；false = 用户取消（本次未保存）。 */
  readonly request: (request: DuplicateMobileConfirmRequest) => Promise<boolean>;
  /** 弹层节点（null = 未命中）；调用方挂到详情区/列表区的弹层槽。 */
  readonly dialogs: ReactNode;
}

/** 待确认请求 + 结果回调（promise 在用户决定前保持 pending，调用方据此保持编辑态）。 */
interface PendingDuplicate extends DuplicateMobileConfirmRequest {
  readonly resolve: (saved: boolean) => void;
}

export function useMobileDuplicateConfirm(): DuplicateMobileConfirm {
  const [pending, setPending] = useState<PendingDuplicate | null>(null);
  const [busy, setBusy] = useState(false);
  // 执行中与当前请求以 ref 为准：同 tick 重复点击也拦得住（与 useDetailActions 同法）。
  const busyRef = useRef(false);
  const pendingRef = useRef<PendingDuplicate | null>(null);
  pendingRef.current = pending;

  const request = useCallback((next: DuplicateMobileConfirmRequest): Promise<boolean> =>
    new Promise<boolean>((resolve) => { setPending({ ...next, resolve }); }), []);

  const cancel = useCallback(() => {
    const current = pendingRef.current;
    if (current === null || busyRef.current) return; // 执行中不关层（ConfirmDialog busy 契约）
    setPending(null);
    current.resolve(false);
  }, []);

  const confirm = useCallback(() => {
    const current = pendingRef.current;
    if (current === null || busyRef.current) return;
    busyRef.current = true;
    setBusy(true);
    void (async () => {
      try {
        await current.write();
        await current.onSettled();
        setPending(null);
        notify.success(current.successCopy);
        current.resolve(true);
      } catch (reason: unknown) {
        // 失败：弹层保留（可重试或取消），映射后的人话就地提示。
        notify.error(safeUiErrorMessage(reason, current.failureFallback ?? ACCOUNT_TEXTS.failureFallback));
      } finally {
        busyRef.current = false;
        setBusy(false);
      }
    })();
  }, []);

  const dialogs = pending === null ? null : createElement(ConfirmDialog, {
    title: "手机号已被其他账号使用",
    lines: [
      `该手机号已用于账号“${pending.ownerName}”。`,
      "仍要保存到当前账号吗？两个账号将显示同一手机号。",
    ],
    confirmLabel: "仍要保存", busyLabel: "保存中…", busy,
    onCancel: cancel, onConfirm: confirm, testId: "account-mobile-duplicate-confirm",
  });
  return { request, dialogs };
}
