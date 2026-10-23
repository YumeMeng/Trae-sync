import { useState } from "react";
import { Merge } from "lucide-react";
import { ConfirmDialog } from "../../../components/shared/ConfirmDialog";
import { LIBRARY_TEXTS, fillLibraryText } from "../config";
import type { CnMergePreview, CwdMergeOrder, MergeBusy, ProjectMergePreview } from "./useMergeActions";

/**
 * 库页归并确认弹层组（P4e-7d，纯展示）：TRAE 项目归并（选保留目标）/ Trae CN 跨账号归并预览 / WB cwd 归并三层。
 * 本层只渲染并回传订单：不执行命令、不刷新、不发通知（执行归 useMergeActions）；标题/正文/按钮词/锚点取自
 * 预览单与订单，本层不判断产品、不重算后端口径。三条链三个忙碌位，本层只取同链那一位。
 * 旧实现对照：L2307-2362（MergeConfirmDialog）/ L2369-2423（TraeCnMergeConfirmDialog）/ WB521-537（cwd 归并）。
 */

const IDLE_BUSY: MergeBusy = { project: false, traeCn: false, cwd: false };
const UNNAMED_PROJECT = "未命名项目"; // L2321/L2352 逐字：目标名空即回退
/** 候选项目会话数后缀（L2354 逐字，config 无该键，弹层专用短句）。 */
const candidateMeta = (count: number): string => `${count} 个会话`;

/** 项目归并：候选单选（默认保留第一个，L2321）+ 正文 moved 随保留目标重算（L2323-2326）。 */
export function ProjectMergeDialog({ preview, busy, onCancel, onConfirm }: {
  readonly preview: ProjectMergePreview;
  readonly busy?: boolean;
  readonly onCancel: () => void;
  /** 回传保留目标 id（订单由 chooseProjectMergeTarget 产出，本层不下单）。 */
  readonly onConfirm: (targetProjectId: string) => void;
}) {
  const [chosen, setChosen] = useState(preview.candidates[0]?.projectId ?? "");
  // 预览单换了候选集（重新打开且候选不同）时不沿用失效目标：回退到第一个候选。
  const targetId = preview.candidates.some((item) => item.projectId === chosen) ? chosen : preview.candidates[0]?.projectId ?? "";
  const moved = preview.candidates.filter((item) => item.projectId !== targetId)
    .reduce((sum, item) => sum + item.sessionCount, 0);
  return (
    <ConfirmDialog title={preview.title} confirmLabel={preview.confirmLabel} busyLabel={preview.busyLabel} busy={busy}
      confirmIcon={<Merge size={15} aria-hidden="true" />} confirmDisabled={targetId === ""}
      onCancel={onCancel} onConfirm={() => onConfirm(targetId)} testId={preview.testId}>
      <p className="confirm-dialog__text">{fillLibraryText(LIBRARY_TEXTS.mergeBody, { moved })}</p>
      <ul className="confirm-dialog__list confirm-dialog__list--choice">
        {preview.candidates.map((project) => (
          <li key={project.projectId}>
            <label className="merge-choice">
              <input type="radio" name="merge-target" checked={project.projectId === targetId}
                onChange={() => setChosen(project.projectId)} data-testid={`merge-target-${project.projectId}`} />
              <span className="merge-choice__name">{project.name.trim() || UNNAMED_PROJECT}</span>
              <span className="merge-choice__meta">{candidateMeta(project.sessionCount)}</span>
            </label>
          </li>
        ))}
      </ul>
    </ConfirmDialog>
  );
}

/** Trae CN 跨账号归并：规模预览逐账号行（主信息=账号名，user_id 只进悬浮）+ 目标提示（L2388-2419）。 */
export function CnMergeDialog({ preview, busy, busyLabel, onCancel, onConfirm }: {
  readonly preview: CnMergePreview;
  readonly busy?: boolean;
  /** 进度阶段文案（hook 的 mergeStage；缺省回落订单的「归并中…」，执行首帧不空窗）。 */
  readonly busyLabel?: string | null;
  readonly onCancel: () => void;
  readonly onConfirm: () => void;
}) {
  return (
    <ConfirmDialog title={preview.title} confirmLabel={preview.confirmLabel} busyLabel={busyLabel ?? preview.busyLabel}
      busy={busy} confirmIcon={<Merge size={15} aria-hidden="true" />} onCancel={onCancel} onConfirm={onConfirm}
      testId={preview.testId}>
      <p className="confirm-dialog__text">{preview.body}</p>
      <ul className="confirm-dialog__list">
        {preview.rows.map((row) => (
          <li key={row.userId} title={row.userId} data-testid={`trae-cn-merge-source-${row.userId}`}>
            {row.name}<span>{row.meta}</span>
          </li>
        ))}
        {preview.targetNote !== null && <li data-testid="trae-cn-merge-target-note">{preview.targetNote}</li>}
      </ul>
    </ConfirmDialog>
  );
}

/** WB cwd 归并：三行正文（canonical 吸收 variants + 不删会话 + 关→写→重启）全由订单给（WB521-537）。 */
export function CwdMergeDialog({ order, busy, onCancel, onConfirm }: {
  readonly order: CwdMergeOrder;
  readonly busy?: boolean;
  readonly onCancel: () => void;
  readonly onConfirm: () => void;
}) {
  return (
    <ConfirmDialog title={order.title} lines={order.lines} confirmLabel={order.confirmLabel} busyLabel={order.busyLabel}
      busy={busy} confirmIcon={<Merge size={15} aria-hidden="true" />} onCancel={onCancel} onConfirm={onConfirm}
      testId={order.testId} />
  );
}

/** 待确认的归并单（三层判别）。 */
export type LibraryMergeDialogView =
  | { readonly kind: "project"; readonly preview: ProjectMergePreview }
  | { readonly kind: "trae-cn"; readonly preview: CnMergePreview }
  | { readonly kind: "cwd"; readonly order: CwdMergeOrder };

/** 确认回传：项目归并多带保留目标 id（其余两层订单已完整）。 */
export type LibraryMergeConfirm =
  | { readonly kind: "project"; readonly preview: ProjectMergePreview; readonly targetProjectId: string }
  | { readonly kind: "trae-cn"; readonly preview: CnMergePreview }
  | { readonly kind: "cwd"; readonly order: CwdMergeOrder };

export interface LibraryMergeDialogsProps {
  readonly dialog: LibraryMergeDialogView | null;
  /** 三条链三个忙碌位：只取同链那一位。 */
  readonly busy?: MergeBusy;
  /** CN 进度阶段文案（hook 的 mergeStage）；缺省回落订单忙碌句。 */
  readonly mergeStage?: string | null;
  readonly onCancel: () => void;
  readonly onConfirm: (confirm: LibraryMergeConfirm) => void;
}

/** 三层归并确认的统一出口：按 kind 分派到对应弹层，确认时回传同形订单。 */
export function LibraryMergeDialogs({ dialog, busy = IDLE_BUSY, mergeStage = null, onCancel, onConfirm }: LibraryMergeDialogsProps) {
  if (dialog === null) return null;
  if (dialog.kind === "project") {
    return <ProjectMergeDialog preview={dialog.preview} busy={busy.project} onCancel={onCancel}
      onConfirm={(targetProjectId) => onConfirm({ kind: "project", preview: dialog.preview, targetProjectId })} />;
  }
  if (dialog.kind === "trae-cn") {
    return <CnMergeDialog preview={dialog.preview} busy={busy.traeCn} busyLabel={mergeStage} onCancel={onCancel}
      onConfirm={() => onConfirm({ kind: "trae-cn", preview: dialog.preview })} />;
  }
  return <CwdMergeDialog order={dialog.order} busy={busy.cwd} onCancel={onCancel}
    onConfirm={() => onConfirm({ kind: "cwd", order: dialog.order })} />;
}
