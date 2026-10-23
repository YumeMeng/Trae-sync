import { useCallback, useRef, useState } from "react";
import { useMergeActions, type MergeProjectView } from "./useMergeActions";
import type { LibraryMergeConfirm, LibraryMergeDialogView } from "./LibraryMergeDialogs";
import type { LibraryForceRefresh } from "./useLibraryDeleteActions";
import type { LibrarySelection } from "./useLibrarySelection";

/**
 * 库页归并接线（P4e-8b-2）：把三条归并链（TRAE 项目 / Trae CN 跨账号 / WB cwd）接到动作 id 上，并持有确认层单。
 * 旧实现对照 LibrarySessionsPanel.tsx（下称 L）L1069-1103 项目归并、L1119-1184 CN 归并、L1187-1197 Esc 让弹层先关；
 * WorkBuddyLibraryPage WB162-177 / WB521-537 cwd 归并。执行、回执、刷新与失败通知归 useMergeActions
 *（本层不重算计数、不拼文案、不判产品）；本层只按动作 id 判别 → 产出弹层单 → 确认时下单 → 成功后清选择。
 * 纪律：成功 refresh({force:true}) 由 useMergeActions 承担；失败 notify.error 且不清数据、不退选择（本层不关单以外的事）。
 */
export interface MergePageInput {
  readonly libraryId: string;
  readonly refresh: LibraryForceRefresh;
  readonly refreshCheckup?: () => Promise<unknown>;
  /** 项目归并候选（装配表按原始 DTO 算会话数：含已归档、不含已删除，L1202-1210）。 */
  readonly mergeCandidates: (projectIds: readonly string[]) => readonly MergeProjectView[];
  /** 其他账号项目（CN 归并入口的动作 id 带项目 id，名称由装配表反查）。 */
  readonly foreignProject: (projectId: string) => { readonly projectId: string; readonly name: string } | null;
  /** WB cwd 归并候选（视图 checkup.candidates 按 canonical 反查，组内 canonical 在前）。 */
  readonly cwdCandidate: (canonical: string) => readonly string[] | null;
  readonly selection: LibrarySelection;
}
export interface MergePageResult {
  readonly dialog: LibraryMergeDialogView | null;
  readonly busy: { readonly project: boolean; readonly traeCn: boolean; readonly cwd: boolean };
  readonly mergeStage: string | null;
  /** 归并动作出口：命中任一归并链返回 true（页面据此不再透出该动作）。 */
  readonly handleAction: (actionId: string) => boolean;
  readonly cancel: () => void;
  readonly confirm: (confirm: LibraryMergeConfirm) => void;
}
const MERGE_ACTION = "batch-merge", FOREIGN_PREFIX = "foreign-merge-", CWD_PREFIX = "merge-";

export function useLibraryMergeActions(input: MergePageInput): MergePageResult {
  const latest = useRef(input); latest.current = input; // 入参经 ref 取最新值：动作身份稳定
  const merge = useMergeActions({ refresh: input.refresh, ...(input.refreshCheckup === undefined ? {} : { refreshCheckup: input.refreshCheckup }) });
  const [dialog, setDialog] = useState<LibraryMergeDialogView | null>(null);
  const handleAction = useCallback((actionId: string): boolean => {
    const current = latest.current;
    if (actionId === MERGE_ACTION) { // L1069-1075：至少两个有名称项目才有合并意义
      const preview = merge.previewProjectMerge(current.mergeCandidates(current.selection.selectedProjectIds));
      if (preview !== null) setDialog({ kind: "project", preview });
      return true;
    }
    if (actionId.startsWith(FOREIGN_PREFIX)) { // L1119-1133：先取规模预览，失败即不弹层
      const project = current.foreignProject(actionId.slice(FOREIGN_PREFIX.length));
      if (project !== null) {
        void (async () => {
          const preview = await merge.openCnMergePreview(project, current.libraryId);
          if (preview !== null) setDialog({ kind: "trae-cn", preview });
        })();
      }
      return true;
    }
    if (actionId.startsWith(CWD_PREFIX)) { // WB456-457：候选组 → 确认层
      const order = merge.previewCwdMerge(current.cwdCandidate(actionId.slice(CWD_PREFIX.length)));
      if (order !== null) setDialog({ kind: "cwd", order });
      return true;
    }
    return false;
  }, [merge]);
  // 确认即下单（弹层在执行期保持打开：同链忙碌位驱动按钮文案与禁用，完成即关）。
  const confirm = useCallback((confirmed: LibraryMergeConfirm) => {
    const current = latest.current;
    void (async () => {
      const outcome = confirmed.kind === "project"
        ? await merge.runProjectMerge(merge.chooseProjectMergeTarget(confirmed.preview, confirmed.targetProjectId, current.libraryId))
        : confirmed.kind === "trae-cn"
          ? await merge.runCnMerge({ projectId: confirmed.preview.projectId, projectName: confirmed.preview.projectName, libraryId: current.libraryId })
          : await merge.runCwdMerge(confirmed.order);
      setDialog(null);
      if (!outcome.ok) return; // 失败：保留选择（useMergeActions 已 notify.error，数据未被改动）
      current.selection.exit(); // 成功清选择并退出选择模式（L1098 / WB 复选清空）
    })();
  }, [merge]);
  return { dialog, busy: merge.busy, mergeStage: merge.mergeStage, handleAction, cancel: () => setDialog(null), confirm };
}
