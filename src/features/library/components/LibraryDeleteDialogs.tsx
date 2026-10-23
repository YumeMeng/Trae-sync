import { Trash2 } from "lucide-react";
import { ConfirmDialog } from "../../../components/shared/ConfirmDialog";
import { LIBRARY_TEXTS, fillLibraryText, type DeletePreviewView } from "../config";
import type { LibraryDeleteBusy, WbPurgeOrder, WbSoftDeleteOrder } from "./useLibraryDeleteActions";

/**
 * 库页删除确认弹层组（P4e-7d，纯展示）：TRAE 批量真删 / WB 软删 / WB 彻底删三层，一张视图或订单驱动一层。
 * 本层只渲染并回传同一张单：不执行命令、不刷新、不发通知（执行归 useLibraryDeleteActions）；标题/正文/按钮词/
 * 危险色/锚点全部取自视图（DeletePreviewView），本层不拼文案、不判断产品、不重算计数。
 * 决策 7：软删与彻底删是两个入口、两层、两个锚点（workbuddy-library-soft-delete-confirm /
 * workbuddy-library-purge-confirm 各自独立），绝不共用；彻底删的三行备份前置句（含「备份失败则整单拒绝」）
 * 由视图 lines 给，本层逐字渲染、不省略、不与软删合并。
 * 旧实现对照：L2266-2300（DeleteConfirmDialog）/ WB486-500（软删）/ WB502-519（彻底删）。
 */

/** 待确认的删除单：三种判别形态（WB 两种 kind 与订单自身判别字段同源；TRAE 侧包一层防混用）。 */
export type LibraryDeleteDialogView =
  | { readonly kind: "trae-delete"; readonly preview: DeletePreviewView }
  | { readonly kind: "wb-soft"; readonly order: WbSoftDeleteOrder }
  | { readonly kind: "wb-purge"; readonly order: WbPurgeOrder };

/** 库页批量删除确认锚（P4e-7d 逐字）：TRAE 层默认用它；旧页锚 delete-confirm 可由 testId prop 显式回落。 */
export const LIBRARY_DELETE_TEST_ID = "library-delete-confirm";

export interface LibraryDeleteDialogsProps {
  /** 当前待确认的删除单；null = 无弹层（整层不渲染）。 */
  readonly dialog: LibraryDeleteDialogView | null;
  /** 动作 hook 的四个忙碌位：本层只取与当前弹层同链的那一位（另一条链在跑不禁用本层）。 */
  readonly busy?: LibraryDeleteBusy;
  /**
   * 锚点覆盖（缺省：TRAE 层 = LIBRARY_DELETE_TEST_ID「library-delete-confirm」；WB 两层 = 视图/订单 testId，
   * 即 config 的 workbuddy-library-soft-delete-confirm / workbuddy-library-purge-confirm，两层永不共用）。
   * 需要逐字沿用旧页 TRAE 锚 delete-confirm 时由装配层显式传入，本层不静默改写视图锚点。
   */
  readonly testId?: string;
  readonly onCancel: () => void;
  /** 确认出口：把同一张单原样回传（执行与刷新归调用方，本层不碰命令）。 */
  readonly onConfirm: (dialog: LibraryDeleteDialogView) => void;
}

const IDLE_BUSY: LibraryDeleteBusy = { deleting: false, archiving: false, softDeleting: false, purging: false };
/** 同链忙碌位：TRAE 真删 / WB 软删 / WB 彻底删各取自己那一位（两链互不兜底）。 */
const busyOf = (dialog: LibraryDeleteDialogView, busy: LibraryDeleteBusy): boolean =>
  dialog.kind === "trae-delete" ? busy.deleting : dialog.kind === "wb-soft" ? busy.softDeleting : busy.purging;

/** TRAE 真删正文：后果句（视图 lines，L2287-2289）+ 会话清单（逐条消息数，L2290-2297）。 */
function TraeDeleteBody({ preview }: { readonly preview: DeletePreviewView }) {
  return (
    <>
      {preview.lines.map((line, index) => (
        <p className="confirm-dialog__text" key={index}>{line}</p>
      ))}
      <ul className="confirm-dialog__list">
        {preview.sessions.map((session) => (
          <li key={session.id} data-testid={`delete-confirm-item-${session.id}`}>
            {session.title.trim() || LIBRARY_TEXTS.unnamedSession}
            <span>{fillLibraryText(LIBRARY_TEXTS.deleteItemSuffix, { count: session.messageCount })}</span>
          </li>
        ))}
      </ul>
    </>
  );
}

/**
 * 三层删除确认：同一 ConfirmDialog 骨架，差异全由视图字段决定——
 * 正文形态（TRAE 带清单，WB 两/三行纯文字）、危险色、按钮词、锚点（testId 逐字取视图）。
 */
export function LibraryDeleteDialogs({ dialog, busy = IDLE_BUSY, testId, onCancel, onConfirm }: LibraryDeleteDialogsProps) {
  if (dialog === null) return null;
  const preview = dialog.kind === "trae-delete" ? dialog.preview : dialog.order.preview;
  // TRAE 层正文 = 后果句 + 会话清单（children）；WB 两层正文 = 视图给的纯文字行（lines）。二者不混用。
  const trae = dialog.kind === "trae-delete";
  const anchor = testId ?? (trae ? LIBRARY_DELETE_TEST_ID : preview.testId);
  return (
    <ConfirmDialog title={preview.title} lines={trae ? undefined : preview.lines}
      children={trae ? <TraeDeleteBody preview={preview} /> : undefined}
      confirmLabel={preview.confirmLabel} busyLabel={preview.busyLabel} danger={preview.danger}
      confirmIcon={preview.danger ? <Trash2 size={15} aria-hidden="true" /> : undefined}
      busy={busyOf(dialog, busy)} onCancel={onCancel} onConfirm={() => onConfirm(dialog)}
      testId={anchor} />
  );
}