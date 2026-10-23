import { TriangleAlert } from "lucide-react";
import { ConfirmDialog } from "../../../components/shared/ConfirmDialog";
import type { EnvironmentDeletePreviewDto } from "../../../types/environment";
import { ENV_TEXTS, fillEnvText } from "../config";
import { formatBytes } from "../traeEnvironment";

/**
 * 删除环境确认弹层（P4d-8a-2b）：逐字搬旧页 EnvironmentDeleteDialog（L1437-1530）。
 * 骨架复用共享 ConfirmDialog（同一组件，旧实现即用它），testid 原样 env-delete-dialog
 * （取消/确认后缀由 ConfirmDialog 追加）；规模句与降级提示全取 ENV_TEXTS。
 * 本文件零命令、零产品判断：预览读取与删除执行由调用方注入（busy/error 由 hook 持有）。
 */

export interface EnvironmentDeleteDialogProps {
  /** 目标环境显示名（标题用）。 */
  readonly envName: string;
  /** 规模预览：null = 仍在读取（确认按钮禁用，旧 L1499 confirmDisabled）。 */
  readonly preview: EnvironmentDeletePreviewDto | null;
  /** 预览读取失败的人话原因（旧 loadError）。 */
  readonly loadError: string | null;
  readonly busy: boolean;
  /** 删除失败的内联提示（旧 error；弹层保留可重试）。 */
  readonly error: string | null;
  readonly onCancel: () => void;
  readonly onConfirm: () => void;
}

/** 规模句（旧 L1481-1490 逐字四支）：有记录列数量与体积、无记录轻描淡写、读不到如实告知。 */
function scaleTextOf(preview: EnvironmentDeletePreviewDto): string {
  const size = formatBytes(preview.size_bytes);
  if (preview.status === "no_data") return fillEnvText(ENV_TEXTS.deleteScaleNoData, { size });
  if (preview.status === "read_failed") return fillEnvText(ENV_TEXTS.deleteScaleReadFailed, { size });
  if (preview.session_count === 0) return fillEnvText(ENV_TEXTS.deleteScaleEmpty, { size });
  return fillEnvText(ENV_TEXTS.deleteScaleReady, { projects: preview.project_count, sessions: preview.session_count, size });
}

export function EnvironmentDeleteDialog({ envName, preview, loadError, busy, error, onCancel, onConfirm }: EnvironmentDeleteDialogProps) {
  const scaleText = preview === null ? null : scaleTextOf(preview);
  return (
    <ConfirmDialog
      title={fillEnvText(ENV_TEXTS.deleteDialogTitle, { name: envName })}
      confirmLabel={ENV_TEXTS.deleteConfirm}
      busyLabel={ENV_TEXTS.deleteBusy}
      busy={busy}
      danger
      confirmDisabled={preview === null}
      onCancel={onCancel}
      onConfirm={onConfirm}
      testId="env-delete-dialog"
    >
      {/* 规模预览：有记录列明数量与体积；无记录轻描淡写；读不到时如实告知（旧 L1505-1520）。 */}
      {scaleText !== null ? (
        <div className="switch-dialog__notice switch-dialog__notice--danger" role="alert">
          <TriangleAlert size={18} aria-hidden="true" />
          <div>
            <strong>{ENV_TEXTS.deleteIrreversible}</strong>
            <p>{scaleText}</p>
          </div>
        </div>
      ) : loadError !== null ? (
        <div className="switch-dialog__notice switch-dialog__notice--danger" role="alert">
          <TriangleAlert size={18} aria-hidden="true" />
          <p>{loadError}</p>
        </div>
      ) : (
        <p className="switch-dialog__prepare">{ENV_TEXTS.deleteLoading}</p>
      )}
      {error !== null && (
        <div className="switch-dialog__notice switch-dialog__notice--danger" role="alert">
          <TriangleAlert size={18} aria-hidden="true" />
          <p>{error}</p>
        </div>
      )}
    </ConfirmDialog>
  );
}
