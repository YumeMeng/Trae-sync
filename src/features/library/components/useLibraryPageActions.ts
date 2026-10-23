import { useCallback, useRef, useState } from "react";
import type { LibraryView } from "../config";
import type { LibraryDeleteDialogView } from "./LibraryDeleteDialogs";
import {
  useLibraryDeleteActions, type DeleteTargetView, type LibraryDeleteBusy, type WbDeleteSelection,
} from "./useLibraryDeleteActions";
import type { LibrarySelection } from "./useLibrarySelection";

/**
 * 库页删除接线（P4e-8b-1）：把批量栏的删除动作接到 useLibraryDeleteActions 的三条链，并持有确认弹层单。
 * 旧实现对照 LibrarySessionsPanel.tsx L1046-1067（TRAE 打开/确认真删）、WorkBuddyLibraryPage WB126-160（软删/彻底删）。
 * 三条链以动作 id 判别（batch-delete / batch-soft-delete / batch-purge），不做产品判断：目标集合由装配表给的
 * targets 函数产出（消息数只有原始 DTO 有，视图行不带），归属锚 userId 由页面状态给（TRAE 链不消费，WB 链为过滤值）。
 * 纪律：弹层只回传同一张单；成功 → 通知页面 + 清选择（刷新由 hook 承担，失败只追加警告）；
 * 失败 → 关弹层、保留选择、不刷新（hook 已 notify.error，数据未被改动）。
 */
export interface LibraryPageActionsInput {
  /** 当前视图（批量栏与树行可删集合的唯一来源）；null = 未登记产品，全部动作不接。 */
  readonly view: LibraryView | null;
  readonly selection: LibrarySelection;
  readonly libraryId: string;
  /** WB 删除归属锚（账号过滤值）；TRAE 链不消费。 */
  readonly userId: string;
  /**
   * 选择 → 删除目标（带消息数）：由装配表按产品原始 DTO 产出（视图行不带消息数）。
   * 两个选择集合都要传：TRAE 勾项目时其下已归档会话也在删除范围内（旧 L850-860），只按会话 id 取会漏。
   */
  readonly targets: (sessionIds: readonly string[], projectIds: readonly string[]) => readonly DeleteTargetView[];
  readonly refresh: (options?: { readonly force?: boolean }) => Promise<{ readonly ok: boolean; readonly error?: string }>;
  /** WB 写后重跑体检（旁路）；缺省不跑。 */
  readonly refreshCheckup?: () => Promise<unknown>;
  /** 删除成功后通知页面（关闭被删会话的查看器）。 */
  readonly onDeleted: (ids: readonly string[]) => void;
}
export interface LibraryPageActionsResult {
  readonly dialog: LibraryDeleteDialogView | null;
  readonly busy: LibraryDeleteBusy;
  /** 批量删除动作出口：命中三条删除链之一返回 true（页面据此不再透出该动作）。 */
  readonly handleAction: (actionId: string) => boolean;
  readonly cancel: () => void;
  readonly confirm: () => void;
}
const TRAE_DELETE = "batch-delete", WB_SOFT = "batch-soft-delete", WB_PURGE = "batch-purge";
const NO_TARGETS: readonly DeleteTargetView[] = []; // 未登记产品：无删除目标，动作一律不弹空单

export function useLibraryPageActions(input: LibraryPageActionsInput): LibraryPageActionsResult {
  const latest = useRef(input); latest.current = input; // 入参经 ref 取最新值：动作身份稳定
  const [dialog, setDialog] = useState<LibraryDeleteDialogView | null>(null);
  const del = useLibraryDeleteActions({ refresh: input.refresh, ...(input.refreshCheckup === undefined ? {} : { refreshCheckup: input.refreshCheckup }) });
  const targetsOf = useCallback((): readonly DeleteTargetView[] => {
    const current = latest.current;
    if (current.view === null) return NO_TARGETS;
    return current.targets(current.selection.selectedSessionIds, current.selection.selectedProjectIds);
  }, []);
  const wbSelectionOf = useCallback((): WbDeleteSelection => ({ userId: latest.current.userId, targets: targetsOf() }), [targetsOf]);
  const handleAction = useCallback((actionId: string): boolean => {
    if (actionId === TRAE_DELETE) {
      const preview = del.previewDelete(targetsOf());
      if (preview !== null) setDialog({ kind: "trae-delete", preview });
      return true;
    }
    // 软删与彻底删是两张独立订单（决策 7）：分别取预览，不共用、不互相兜底。
    if (actionId === WB_SOFT) {
      const order = del.previewSoftDelete(wbSelectionOf());
      if (order !== null) setDialog({ kind: "wb-soft", order }); // 未过滤或无目标：不弹空单
      return true;
    }
    if (actionId === WB_PURGE) {
      const order = del.previewPurge(wbSelectionOf());
      if (order !== null) setDialog({ kind: "wb-purge", order });
      return true;
    }
    return false;
  }, [del, targetsOf, wbSelectionOf]);
  const cancel = useCallback(() => setDialog(null), []);
  const confirm = useCallback(() => {
    const current = dialog;
    if (current === null) return;
    const ids = current.kind === "trae-delete" ? current.preview.sessions.map((item) => item.id) : current.order.sessionIds;
    void (async () => {
      // 执行期弹层保持打开（按钮转忙碌句、两侧禁用），完成即关。
      const outcome = current.kind === "trae-delete" ? await del.deleteTraeSessions(current.preview, latest.current.libraryId)
        : current.kind === "wb-soft" ? await del.runSoftDelete(current.order) : await del.runPurge(current.order);
      setDialog(null);
      if (!outcome.ok) return; // 失败：保留选择（hook 已 notify.error，数据未被改动）
      latest.current.onDeleted(ids);
      latest.current.selection.exit(); // 成功清选择（TRAE 同时退出选择模式；WB 模式恒 false）
    })();
  }, [del, dialog]);
  return { dialog, busy: del.busy, handleAction, cancel, confirm };
}
