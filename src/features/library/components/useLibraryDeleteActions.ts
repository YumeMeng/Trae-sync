import { useCallback, useRef, useState } from "react";
import { applyMasterArchiveChanges, deleteMasterSessions } from "../../../platform/libraryDelete";
import { deleteWorkBuddySessions, purgeWorkBuddySessions, saveArchiveDraft } from "../../../platform/library";
import { notify } from "../../../platform/notify";
import { LIBRARY_TEXTS, fillLibraryText, type DeletePreviewView } from "../config";
import { masterArchiveCommitFailure, safeUiErrorMessage } from "../../../utils/safeUiError";

/**
 * 库页删除 / 归档动作 hook（P4e-6）：TRAE 批量删除 + 归档应用；WB 软删 + 彻底删两条独立链。
 * 命令一律经 platform（零 invoke）；确认层归调用方弹层——本 hook 只产出「确认单」并在执行时消费它，
 * 计数与标题全来自视图，执行侧不重算（防确认层与执行层漂移）。纪律：成功后 refresh({force:true})，
 * 刷新失败只追加警告（已提交的操作不得误报失败）；失败 notify.error 且不清数据、不刷新、不退选择。
 * WB 软删与彻底删是两个入口、两张订单、两个忙碌位：不共用确认、不互相兜底（决策 7）；彻底删必须先取
 * 预览单，备份前置由后端承担，失败即整单拒绝。
 */

/** 视图给出的删除目标（与 DeletePreviewView.sessions 同形；WB 行无消息数，messageCount 填 0）。 */
export interface DeleteTargetView { readonly id: string; readonly title: string; readonly messageCount: number }
export interface LibraryRefreshOutcome { readonly ok: boolean; readonly error?: string } // 读取 hook 的刷新回执（Trae/Wb 均满足）
export type LibraryForceRefresh = (options?: { readonly force?: boolean }) => Promise<LibraryRefreshOutcome>;
export interface LibraryDeleteBusy { readonly deleting: boolean; readonly archiving: boolean; readonly softDeleting: boolean; readonly purging: boolean } // 两条删除链各自持位
export interface DeleteRunOutcome { readonly ok: boolean; readonly count: number } // count 取后端回执，不用视图计数冒充
export interface WbDeleteSelection { readonly userId: string; readonly targets: readonly DeleteTargetView[] }
export interface WbSoftDeleteOrder { readonly kind: "soft"; readonly sessionIds: readonly string[]; readonly userId: string; readonly preview: DeletePreviewView } // 只能由 previewSoftDelete 产出
export interface WbPurgeOrder { readonly kind: "purge"; readonly sessionIds: readonly string[]; readonly userId: string; readonly preview: DeletePreviewView } // 只能由 previewPurge 产出；备份前置
export interface TraeArchiveOrder { readonly libraryId: string; readonly archiveSessionIds: readonly string[]; readonly restoreSessionIds: readonly string[] } // 草稿正本在后端
export type ArchiveRecoveryKind = "relaunch" | "refresh" | "commit-unknown" | "committed-attention";
export interface ArchiveRunOutcome { readonly ok: boolean; readonly committed: boolean; readonly notice: string | null; readonly recovery: ArchiveRecoveryKind | null } // committed = 库已提交（页面据此清草稿）
export interface LibraryDeleteActionsInput {
  readonly refresh: LibraryForceRefresh; // 写后强制重读；刷新失败不翻转成功回执
  readonly refreshCheckup?: () => Promise<unknown>; // WB 写后重跑体检（旧 WB135/154 同口径）；缺省不跑
}
export interface LibraryDeleteActionsResult {
  readonly busy: LibraryDeleteBusy;
  readonly previewDelete: (targets: readonly DeleteTargetView[]) => DeletePreviewView | null;
  readonly deleteTraeSessions: (preview: DeletePreviewView | null, libraryId: string) => Promise<DeleteRunOutcome>;
  readonly applyArchive: (order: TraeArchiveOrder) => Promise<ArchiveRunOutcome>;
  readonly previewSoftDelete: (selection: WbDeleteSelection) => WbSoftDeleteOrder | null;
  readonly runSoftDelete: (order: WbSoftDeleteOrder | null) => Promise<DeleteRunOutcome>;
  readonly previewPurge: (selection: WbDeleteSelection) => WbPurgeOrder | null;
  readonly runPurge: (order: WbPurgeOrder | null) => Promise<DeleteRunOutcome>;
}
const IDLE: LibraryDeleteBusy = { deleting: false, archiving: false, softDeleting: false, purging: false };
const NO_RUN: DeleteRunOutcome = { ok: false, count: 0 };
const NO_ARCHIVE: ArchiveRunOutcome = { ok: false, committed: false, notice: null, recovery: null };
/** TRAE 真删确认单（L2278-2294 口径）：规模与文案全由视图目标集合算出，空选即无单。 */
export function buildTraeDeletePreview(targets: readonly DeleteTargetView[]): DeletePreviewView | null {
  if (targets.length === 0) return null;
  const messages = targets.reduce((sum, item) => sum + item.messageCount, 0);
  return { testId: "delete-confirm", title: fillLibraryText(LIBRARY_TEXTS.deleteTitle, { count: targets.length }),
    confirmLabel: LIBRARY_TEXTS.deleteConfirm, busyLabel: LIBRARY_TEXTS.deleteBusy, danger: true,
    lines: [fillLibraryText(LIBRARY_TEXTS.deleteBody, { messages })], sessions: targets,
    sessionCount: targets.length, messageCount: messages, backupRequired: true, backupNote: null };
}
function archiveNoticeOf(receipt: { archived_sessions: number; restored_sessions: number; relaunch_outcome: string }): string {
  const counts = { archived: receipt.archived_sessions, restored: receipt.restored_sessions };
  if (receipt.relaunch_outcome === "failed") return fillLibraryText(LIBRARY_TEXTS.applyRelaunchFailed, counts);
  return receipt.relaunch_outcome === "not_requested" ? fillLibraryText(LIBRARY_TEXTS.applyCnDone, counts) : fillLibraryText(LIBRARY_TEXTS.applyDone, counts);
}
const restartedOf = (tail: { readonly restarted: boolean }): string => (tail.restarted ? LIBRARY_TEXTS.wbRestarted : "");
const readyToDelete = (selection: WbDeleteSelection): boolean => selection.userId !== "" && selection.targets.length > 0; // 归属锚 + 非空选

export function useLibraryDeleteActions(input: LibraryDeleteActionsInput): LibraryDeleteActionsResult {
  const latest = useRef(input); latest.current = input; // 入参经 ref 取最新值：动作身份稳定
  const busyRef = useRef<LibraryDeleteBusy>({ ...IDLE }); // 防连点唯一真源（同一 tick 内重复点击也拦得住）
  const [busy, setBusy] = useState<LibraryDeleteBusy>(IDLE);
  const mark = useCallback((patch: Partial<LibraryDeleteBusy>) => {
    busyRef.current = { ...busyRef.current, ...patch }; setBusy(busyRef.current);
  }, []);
  /** 成功后的强制重读：失败只返回 false，由调用方追加一句警告。 */
  const reload = useCallback(async (): Promise<boolean> => {
    try { return (await latest.current.refresh({ force: true })).ok; } catch { return false; }
  }, []);
  const reloadCheckup = useCallback(async () => {
    if (!latest.current.refreshCheckup) return;
    try { await latest.current.refreshCheckup(); } catch { /* 体检是旁路：失败不打断回执 */ }
  }, []);

  // ===== TRAE 批量删除（真删 + 备份前置；一次确认）=====
  const deleteTraeSessions = useCallback(async (preview: DeletePreviewView | null, libraryId: string): Promise<DeleteRunOutcome> => {
    if (preview === null || busyRef.current.deleting || busyRef.current.archiving) return NO_RUN;
    const sessionIds = preview.sessions.map((item) => item.id);
    mark({ deleting: true });
    try {
      const receipt = await deleteMasterSessions({ sessionIds, libraryId });
      notify.success(fillLibraryText(LIBRARY_TEXTS.deleteReceipt, { count: receipt.deleted_sessions }));
      if (!(await reload())) notify.warning(LIBRARY_TEXTS.refreshFailedAfterAction);
      return { ok: true, count: receipt.deleted_sessions };
    } catch (reason: unknown) {
      notify.error(safeUiErrorMessage(reason, LIBRARY_TEXTS.actionFailure)); // 失败不清数据、不刷新、不退选择
      return NO_RUN;
    } finally { mark({ deleting: false }); }
  }, [mark, reload]);

  // ===== TRAE 归档：草稿保存 → 单事务提交 → 按库重启（可选）=====
  const applyArchive = useCallback(async (order: TraeArchiveOrder): Promise<ArchiveRunOutcome> => {
    if (busyRef.current.archiving || busyRef.current.deleting) return NO_ARCHIVE;
    if (order.archiveSessionIds.length === 0 && order.restoreSessionIds.length === 0) return NO_ARCHIVE;
    mark({ archiving: true });
    try {
      // ① 草稿保存（正本在后端）：保存失败即整单拒绝，草稿与主库都不动。
      await saveArchiveDraft({ libraryId: order.libraryId, archiveSessionIds: order.archiveSessionIds, restoreSessionIds: order.restoreSessionIds });
      // ② 单事务提交：关实例 → 写 → 按库重启由后端承担（relaunch_outcome 如实上报）。
      const receipt = await applyMasterArchiveChanges({ archiveSessionIds: order.archiveSessionIds, restoreSessionIds: order.restoreSessionIds, libraryId: order.libraryId });
      const notice = archiveNoticeOf(receipt), refreshed = await reload();
      const recovery: ArchiveRecoveryKind | null = receipt.relaunch_outcome === "failed" ? "relaunch" : refreshed ? null : "refresh";
      if (recovery === null) notify.success(notice); // 需就地恢复时由页面渲染恢复条，不重复通知
      return { ok: true, committed: true, notice, recovery };
    } catch (reason: unknown) {
      const failure = masterArchiveCommitFailure(reason);
      const recovery: ArchiveRecoveryKind | null = failure === "unknown" ? "commit-unknown" : failure === "committed" ? "committed-attention" : null;
      notify.error(safeUiErrorMessage(reason, LIBRARY_TEXTS.draftKept)); // 草稿仍保留，主库不动
      return { ok: false, committed: false, notice: null, recovery };
    } finally { mark({ archiving: false }); }
  }, [mark, reload]);

  // ===== WB 软删：独立入口一（可逆；只写 deleted_at，cloud/他人账号行由后端跳过）=====
  const previewSoftDelete = useCallback((selection: WbDeleteSelection): WbSoftDeleteOrder | null => {
    if (!readyToDelete(selection)) return null;
    const count = selection.targets.length;
    return { kind: "soft", sessionIds: selection.targets.map((item) => item.id), userId: selection.userId,
      preview: { testId: LIBRARY_TEXTS.wbSoftConfirmTestId, title: LIBRARY_TEXTS.wbSoftConfirmTitle,
        confirmLabel: LIBRARY_TEXTS.wbSoftConfirm, busyLabel: LIBRARY_TEXTS.wbSoftConfirmBusy, danger: false,
        lines: [fillLibraryText(LIBRARY_TEXTS.wbSoftConfirmLine1, { count }), LIBRARY_TEXTS.wbSoftConfirmLine2],
        sessions: selection.targets, sessionCount: count, messageCount: 0, backupRequired: false, backupNote: null } };
  }, []);
  const runSoftDelete = useCallback(async (order: WbSoftDeleteOrder | null): Promise<DeleteRunOutcome> => {
    if (order === null || busyRef.current.softDeleting || busyRef.current.purging) return NO_RUN;
    mark({ softDeleting: true });
    try {
      const receipt = await deleteWorkBuddySessions(order.sessionIds, order.userId);
      notify.success(fillLibraryText(LIBRARY_TEXTS.wbSoftDeleteReceipt, { count: receipt.deleted_sessions, restarted: restartedOf(receipt.tail) }));
      if (!(await reload())) notify.warning(LIBRARY_TEXTS.refreshFailedAfterAction);
      await reloadCheckup();
      return { ok: true, count: receipt.deleted_sessions };
    } catch (reason: unknown) {
      notify.error(safeUiErrorMessage(reason, LIBRARY_TEXTS.wbFailureSuffix)); // 数据未被改动
      return NO_RUN;
    } finally { mark({ softDeleting: false }); }
  }, [mark, reload, reloadCheckup]);

  // ===== WB 彻底删：独立入口二（必须先预览；备份前置，失败整单拒绝，不许降级成软删）=====
  const previewPurge = useCallback((selection: WbDeleteSelection): WbPurgeOrder | null => {
    if (!readyToDelete(selection)) return null;
    const count = selection.targets.length;
    return { kind: "purge", sessionIds: selection.targets.map((item) => item.id), userId: selection.userId,
      preview: { testId: LIBRARY_TEXTS.wbPurgeConfirmTestId, title: LIBRARY_TEXTS.wbPurgeConfirmTitle,
        confirmLabel: LIBRARY_TEXTS.wbPurgeConfirm, busyLabel: LIBRARY_TEXTS.wbPurgeConfirmBusy, danger: true,
        lines: [fillLibraryText(LIBRARY_TEXTS.wbPurgeConfirmLine1, { count }), LIBRARY_TEXTS.wbPurgeConfirmLine2, LIBRARY_TEXTS.wbPurgeConfirmLine3],
        sessions: selection.targets, sessionCount: count, messageCount: 0, backupRequired: true, backupNote: LIBRARY_TEXTS.wbPurgeConfirmLine2 } };
  }, []);
  const runPurge = useCallback(async (order: WbPurgeOrder | null): Promise<DeleteRunOutcome> => {
    if (order === null || busyRef.current.purging || busyRef.current.softDeleting) return NO_RUN;
    mark({ purging: true });
    try {
      const receipt = await purgeWorkBuddySessions(order.sessionIds, order.userId);
      notify.success(fillLibraryText(LIBRARY_TEXTS.wbPurgeReceipt,
        { count: receipt.purged_sessions, bodies: receipt.removed_body_files, backup: receipt.backup_path, restarted: restartedOf(receipt.tail) }));
      if (!(await reload())) notify.warning(LIBRARY_TEXTS.refreshFailedAfterAction);
      await reloadCheckup();
      return { ok: true, count: receipt.purged_sessions };
    } catch (reason: unknown) {
      notify.error(safeUiErrorMessage(reason, LIBRARY_TEXTS.wbFailureSuffix)); // 备份前置失败 = 整单拒绝，库原样
      return NO_RUN;
    } finally { mark({ purging: false }); }
  }, [mark, reload, reloadCheckup]);

  return { busy, previewDelete: buildTraeDeletePreview, deleteTraeSessions, applyArchive,
    previewSoftDelete, runSoftDelete, previewPurge, runPurge };
}
