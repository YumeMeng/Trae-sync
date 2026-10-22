import { useState } from "react";
import { Trash2 } from "lucide-react";
import { ConfirmDialog } from "../../../components/shared/ConfirmDialog";
import type { DetailActionView } from "../config";

/**
 * 危险分区动作（P4c-7b 特例 2）：危险样式按钮 + 点击先二次确认（复用 shared/ConfirmDialog），
 * 确认后才走统一动作出口 onAction(id)——本文件不执行任何命令、不读数据、不判断产品。
 * 弹层 testid 直接用 view 给的动作 id（不拼字符串）；取消/确认后缀由 ConfirmDialog 自己追加。
 */
export interface DetailDangerActionProps {
  readonly action: DetailActionView;
  /** 影响范围说明（分区 hint，逐字来自 builder 的危险区文案）。 */
  readonly hint: string | null;
  readonly busy: boolean;
  readonly onAction: (id: string) => void;
}

export function DetailDangerAction({ action, hint, busy, onAction }: DetailDangerActionProps) {
  // 弹层开合是纯本地 UI 状态（唯一 hook）：不承载数据，也不跨分区共享。
  const [open, setOpen] = useState(false);
  const disabled = action.disabledReason !== undefined || busy;
  return (
    <>
      <button
        className="btn btn--danger"
        type="button"
        onClick={() => setOpen(true)}
        disabled={disabled}
        title={action.disabledReason}
        data-testid={action.id}
      >
        <Trash2 size={15} aria-hidden="true" />
        {action.label}
      </button>
      {open && (
        <ConfirmDialog
          title={action.label}
          lines={hint === null ? undefined : [hint]}
          confirmLabel={action.label}
          busy={busy}
          danger
          onCancel={() => setOpen(false)}
          onConfirm={() => {
            setOpen(false);
            onAction(action.id);
          }}
          testId={action.id}
        />
      )}
    </>
  );
}
