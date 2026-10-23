import { TriangleAlert } from "lucide-react";
import { safeUiErrorMessage } from "../../../utils/safeUiError";
import { ENV_TEXTS } from "../config";

/**
 * 名称输入弹层（P4d-8a-2b，创建/重命名共用）：逐字搬旧页 EnvironmentNameDialog（L1190-1267）。
 * 骨架与 testid 原样：env-name-dialog / env-name-input / env-name-cancel / env-name-confirm；
 * 文案全部取 ENV_TEXTS（含 L1236 占位符、L1251/L1261 按钮）；本文件零命令、零产品判断，
 * 提交与刷新由调用方注入 onSubmit（成功才关层，失败内联提示，旧 L1204-1217 同法）。
 */

/** 弹层状态（旧 NameDialogState L1182-1188 去掉 env：重命名目标由调用方持有）。 */
export interface NameDialogValue {
  readonly name: string;
  readonly error: string | null;
  readonly busy: boolean;
}

export interface EnvironmentNameDialogProps {
  /** 弹层标题（新建环境 / 重命名环境）。 */
  readonly title: string;
  /** 确认按钮文案（创建 / 保存）。 */
  readonly confirmLabel: string;
  readonly value: NameDialogValue;
  readonly onChange: (next: NameDialogValue) => void;
  readonly onClose: () => void;
  /** 提交动作（成功才关闭弹层；失败抛错由本组件转为内联提示）。 */
  readonly onSubmit: (name: string) => Promise<void>;
}

export function EnvironmentNameDialog({ title, confirmLabel, value, onChange, onClose, onSubmit }: EnvironmentNameDialogProps) {
  // 提交（旧 L1207-1217 逐字）：空名/执行中直接返回；失败映射为人话留在弹层内（可改名重试）。
  const submit = async () => {
    const name = value.name.trim();
    if (!name || value.busy) return;
    onChange({ ...value, busy: true, error: null });
    try {
      await onSubmit(name);
      onClose();
    } catch (reason: unknown) {
      onChange({ ...value, busy: false, error: safeUiErrorMessage(reason, ENV_TEXTS.nameFailure) });
    }
  };

  return (
    <div className="switch-veil" role="presentation" onClick={(event) => {
      if (event.target === event.currentTarget && !value.busy) onClose();
    }}>
      <div className="switch-dialog" role="dialog" aria-modal="true" aria-label={title} data-testid="env-name-dialog">
        <div className="switch-dialog__title">{title}</div>
        <input
          className="env-name-input"
          type="text"
          value={value.name}
          maxLength={64}
          placeholder={ENV_TEXTS.namePlaceholder}
          autoFocus
          onChange={(event) => onChange({ ...value, name: event.target.value })}
          onKeyDown={(event) => {
            if (event.key === "Enter") void submit();
          }}
          data-testid="env-name-input"
        />
        {value.error && (
          <div className="switch-dialog__notice switch-dialog__notice--danger" role="alert">
            <TriangleAlert size={18} aria-hidden="true" />
            <p>{value.error}</p>
          </div>
        )}
        <div className="switch-dialog__actions">
          <button className="btn" type="button" onClick={onClose} disabled={value.busy} data-testid="env-name-cancel">
            {ENV_TEXTS.cancel}
          </button>
          <button
            className="btn btn--primary"
            type="button"
            onClick={() => void submit()}
            disabled={value.busy || value.name.trim().length === 0}
            data-testid="env-name-confirm"
          >
            {value.busy ? ENV_TEXTS.nameBusy : confirmLabel}
          </button>
        </div>
      </div>
    </div>
  );
}
