import { useCallback, useRef, useState } from "react";
import { useArchiveDraft } from "../../../hooks/useArchiveDraft";
import { launchMasterLibrary } from "../../../platform/library";
import { notify } from "../../../platform/notify";
import { safeUiErrorMessage } from "../../../utils/safeUiError";
import { LIBRARY_TEXTS, fillLibraryText, type ArchiveRecoveryKind, type ArchiveRecoveryView } from "../config";
import type { SessionTreeDraft } from "../sessionTree";
import type { ArchiveRunOutcome, LibraryForceRefresh, TraeArchiveOrder } from "./useLibraryDeleteActions";
import type { LibrarySelection } from "./useLibrarySelection";

/**
 * 库页归档编排（P4e-8b-2）：草稿暂存 → 应用（保存草稿 → 单事务 → 可选重启）→ recovery 四态就地恢复。
 * 旧实现对照 LibrarySessionsPanel.tsx（下称 L）：L245-257 草稿与恢复态、L875-922 草稿暂存、L924-986 应用、
 * L988-1028 恢复条重试、L1030-1044 放弃设置、L1105-1115 视图切换。执行归 useLibraryDeleteActions.applyArchive
 * （页面把同一个实例的出口注入），本 hook 只持草稿 / 视图 / 折叠 / 恢复态与动作 id 判别，不判产品。
 * 纪律：成功 → 清草稿 + 退选择 + 由 applyArchive 发成功回执；需就地恢复 → 只落恢复条（不重复通知）；
 * 失败 → 草稿保留、主库不动、恢复条按 commit-unknown / committed-attention 两态给核对指引。
 */
/** 可见会话中命中选择（项目级勾选展开为其下会话）：由装配表按原始 DTO 算（视图行不带 project_id）。 */
export interface VisibleSelectionInput {
  readonly draft: SessionTreeDraft;
  readonly archiveView: boolean;
  readonly sessionIds: readonly string[];
  readonly projectIds: readonly string[];
}
export interface ArchiveActionsInput {
  readonly active: boolean;
  readonly libraryId: string;
  /**
   * 草稿机制是否参与（首帧即可判定的静态条件：能力表声明有归档 + 库写能力非 none）。
   * 旧 L254 用「操作就绪 + 写能力」双门禁当 readOnly，但那要等读取面就绪；这里只按静态能力开草稿读写，
   * 真正下单前仍由 canArchive 现取（操作未就绪即拒绝），不会把未就绪库的草稿提交上去。
   */
  readonly supported: boolean;
  /** 归档写门禁（动作时现取：操作就绪 + 写能力）。 */
  readonly canArchive: () => boolean;
  /** 应用前是否需要「关闭客户端」确认（Work 库 true / 只读 CN 库 false，由视图给）。 */
  readonly confirmRequired: () => boolean;
  readonly refresh: LibraryForceRefresh;
  /** 与删除链同一个实例的归档出口（保存草稿 → 单事务 → 重启）。 */
  readonly applyArchive: (order: TraeArchiveOrder) => Promise<ArchiveRunOutcome>;
  /** 归档执行中（同一实例的忙碌位）。 */
  readonly archiving: boolean;
  readonly selection: LibrarySelection;
  readonly visibleSelection: (input: VisibleSelectionInput) => readonly string[];
  readonly viewerSessionId: string | null;
  readonly closeSession: () => void;
}
/** 页面需要的归档状态（原样进 TraeLibraryPageFields）。 */
export interface ArchivePageFields {
  readonly draft: SessionTreeDraft;
  readonly view: boolean;
  readonly collapsedKeys: readonly string[];
  readonly recovery: ArchiveRecoveryView | null;
  readonly draftError: string | null;
}
export interface ArchiveActionsResult {
  readonly page: ArchivePageFields;
  readonly busy: boolean;
  /** Work 库应用前的「关闭客户端」确认单是否开着（只读 CN 库不会开）。 */
  readonly confirmOpen: boolean;
  readonly handleAction: (actionId: string) => boolean;
  /** 恢复条重试（L988-1028）：实例未启动先重试启动，再刷新列表；提交待核对两态只刷新并给核对指引。 */
  readonly retry: () => void;
  /** 行内快捷归档（L904-913：只进草稿，同时关掉被移出列表的查看器）。 */
  readonly quickArchive: (sessionId: string) => void;
  /** 归档视图项目折叠（键 = 「模式\u0000项目」，L1571-1572；parentId = 模式层节点 id）。 */
  readonly toggleCollapsed: (key: string) => void;
  /** 归档视图展开（选择模式勾选折叠项目时立即展开，让勾选结果可见，L1579-1585）。 */
  readonly expand: (key: string) => void;
  readonly cancel: () => void;
  readonly confirm: () => void;
}
/** 后端回执 / 失败分类 → 恢复条（L951-980）：relaunch / refresh 带回执句，commit-* 用核对指引。 */
const recoveryOf = (kind: ArchiveRecoveryKind | null, notice: string | null): ArchiveRecoveryView | null => {
  if (kind === null) return null;
  if (kind === "relaunch") return { kind, message: notice ?? LIBRARY_TEXTS.applyRelaunchFailed };
  if (kind === "refresh") return { kind, message: (notice ?? "") + LIBRARY_TEXTS.applyRefreshFailedSuffix };
  return { kind, message: kind === "commit-unknown" ? LIBRARY_TEXTS.commitUnknown : LIBRARY_TEXTS.committedAttention };
};
export function useArchiveActions(input: ArchiveActionsInput): ArchiveActionsResult {
  const latest = useRef(input); latest.current = input; // 入参经 ref 取最新值：动作身份稳定
  // 只读库 / 无归档产品不参与草稿机制（P11-6）：静态能力关即不读、不写、不迁移。
  const store = useArchiveDraft(input.libraryId, { active: input.active, readOnly: !input.supported });
  const [view, setView] = useState(false);
  const [collapsed, setCollapsed] = useState<readonly string[]>([]);
  const [recovery, setRecovery] = useState<ArchiveRecoveryView | null>(null);
  const [confirmOpen, setConfirmOpen] = useState(false);
  const draftRef = useRef(store.draft); draftRef.current = store.draft; // 执行期读最新草稿（不重算）
  const recoveryRef = useRef(recovery); recoveryRef.current = recovery;

  /** 草稿暂存（L875-901）：两集合互斥，切换目标即从另一集合摘除；暂存是中间态提醒，走中性通知。 */
  const stage = useCallback((mode: "archive" | "restore", sessionIds: readonly string[]) => {
    if (sessionIds.length === 0) return;
    store.setDraft((current) => {
      const archive = new Set(current.archiveSessionIds), restore = new Set(current.restoreSessionIds);
      for (const id of sessionIds) {
        const remove = mode === "archive" ? restore : archive;
        if (remove.delete(id)) continue; // 已在反向集合：本次点击 = 撤回暂存
        (mode === "archive" ? archive : restore).add(id);
      }
      return { archiveSessionIds: [...archive], restoreSessionIds: [...restore] };
    });
    setRecovery(null);
    notify.info(fillLibraryText(mode === "archive" ? LIBRARY_TEXTS.draftQueuedArchive : LIBRARY_TEXTS.draftQueuedRestore, { count: sessionIds.length }));
  }, [store]);

  /** 应用整批设置（L924-986）：Work 库先弹关闭客户端确认；成功清草稿 + 退选择；异常落恢复条。 */
  const apply = useCallback(async (confirmed: boolean) => {
    const current = latest.current, draft = draftRef.current;
    const blocked = recoveryRef.current?.kind === "commit-unknown" || recoveryRef.current?.kind === "committed-attention";
    if (!current.canArchive() || current.archiving || blocked) return; // 写门禁未就绪 / 另一链在跑 / 待核对
    if (draft.archiveSessionIds.length === 0 && draft.restoreSessionIds.length === 0) return; // 空草稿不发起提交
    if (!current.supported) return; // 无归档产品 / 只读库：不发起提交（视图也不会给应用按钮）
    if (!confirmed && current.confirmRequired()) { setConfirmOpen(true); return; } // Work 库先确认关闭客户端
    setConfirmOpen(false); setRecovery(null);
    const outcome = await current.applyArchive({ libraryId: current.libraryId, archiveSessionIds: draft.archiveSessionIds, restoreSessionIds: draft.restoreSessionIds });
    if (!outcome.ok) { setRecovery(recoveryOf(outcome.recovery, outcome.notice)); return; } // 失败：草稿保留、主库不动
    store.reset(); current.selection.exit(); // 已提交：草稿与选择一并收尾
    if (outcome.recovery !== null) setRecovery(recoveryOf(outcome.recovery, outcome.notice)); // 成功回执由 applyArchive 发
  }, [store]);

  /** 恢复条重试（L988-1028）：实例未启动先重试启动，再刷新列表；提交待核对两态只刷新并给核对指引。 */
  const retry = useCallback(async () => {
    const current = latest.current, active = recoveryRef.current;
    if (active === null || current.archiving) return;
    let failureStage: ArchiveRecoveryKind = active.kind;
    try {
      if (active.kind === "relaunch") await launchMasterLibrary();
      const refreshed = await current.refresh({ force: true });
      if (!refreshed.ok) {
        failureStage = "refresh";
        const keep = active.kind === "commit-unknown" || active.kind === "committed-attention" ? active.kind : "refresh";
        setRecovery({ kind: keep, message: LIBRARY_TEXTS.recoveryRetryRefreshFailed });
        notify.error(LIBRARY_TEXTS.recoveryRefreshFailed);
        return;
      }
      setRecovery(null);
      if (active.kind === "committed-attention") { store.reset(); current.selection.exit(); notify.info(LIBRARY_TEXTS.recoveryRetryCommittedOk); }
      else if (active.kind === "commit-unknown") notify.info(LIBRARY_TEXTS.recoveryRetryUnknownOk);
      else notify.success(active.kind === "relaunch" ? LIBRARY_TEXTS.recoveryRetryRelaunchOk : LIBRARY_TEXTS.recoveryRetryRefreshOk);
    } catch (reason: unknown) {
      notify.error(safeUiErrorMessage(reason, failureStage === "relaunch" ? LIBRARY_TEXTS.recoveryRelaunchFailed : LIBRARY_TEXTS.recoveryRefreshFailed));
    }
  }, [store]);

  /** 放弃草稿（L1030-1044）：不触碰主库；已提交 / 结果未知两态给核对指引而非「主库没有变化」。 */
  const discard = useCallback(() => {
    if (latest.current.archiving) return;
    const kind = recoveryRef.current?.kind;
    store.reset(); setRecovery(null);
    notify.info(kind === "committed-attention" ? LIBRARY_TEXTS.draftClearedSubmitted
      : kind === "commit-unknown" ? LIBRARY_TEXTS.draftClearedCheck : LIBRARY_TEXTS.draftDiscarded);
  }, [store]);

  const toggleCollapsed = useCallback((key: string) => {
    setCollapsed((current) => current.includes(key) ? current.filter((item) => item !== key) : [...current, key]);
  }, []);

  const expand = useCallback((key: string) => {
    setCollapsed((current) => current.includes(key) ? current.filter((item) => item !== key) : current);
  }, []);

  const quickArchive = useCallback((sessionId: string) => {
    stage("archive", [sessionId]);
    if (latest.current.viewerSessionId === sessionId) latest.current.closeSession(); // 被移出正常列表即关右栏（L909）
  }, [stage]);

  const handleAction = useCallback((actionId: string): boolean => {
    const current = latest.current, draft = draftRef.current;
    if (actionId === "batch-archive") {
      stage("archive", current.visibleSelection({ draft, archiveView: false, sessionIds: current.selection.selectedSessionIds, projectIds: current.selection.selectedProjectIds }));
      current.selection.exit(); return true; // L904-911：归档只进草稿，随后退选择
    }
    if (actionId === "batch-restore") {
      stage("restore", current.visibleSelection({ draft, archiveView: true, sessionIds: current.selection.selectedSessionIds, projectIds: current.selection.selectedProjectIds }));
      current.selection.exit(); return true; // L916-922
    }
    if (actionId === "draft-apply") { void apply(false); return true; }
    if (actionId === "draft-discard") { discard(); return true; }
    if (actionId === "archive-entry") { setView(true); current.selection.exit(); return true; } // L1107-1110
    if (actionId === "archive-back") { setView(false); current.selection.exit(); return true; } // L1112-1115
    return false;
  }, [apply, discard, stage]);

  return {
    page: { draft: store.draft, view, collapsedKeys: collapsed, recovery, draftError: store.storageError },
    busy: input.archiving, confirmOpen, handleAction, retry: () => void retry(), quickArchive, toggleCollapsed, expand,
    cancel: () => setConfirmOpen(false),
    confirm: () => { setConfirmOpen(false); void apply(true); },
  };
}
