import { useEffect, type ReactNode } from "react";

/**
 * 通用单次确认弹层：破坏性/大范围操作前的二次确认（ADR-0018 铁律——单次确认后执行）。
 * 视觉骨架与切号/环境页弹层同源（switch-dialog 家族），全工具确认形态统一入口。
 * 正文两种形态：lines（逐行文字说明）或 children（自定义正文：清单、单选列表、结构化通知）。
 */
export function ConfirmDialog({
  title,
  lines,
  children,
  confirmLabel = "确认",
  confirmIcon,
  busyLabel = "处理中…",
  busy = false,
  confirmDisabled = false,
  danger = false,
  onCancel,
  onConfirm,
  testId,
}: {
  /** 弹层标题（操作名，如「删除账号」「刷新登录凭据」）。 */
  title: string;
  /** 正文行：每行一条影响说明；首行主要后果，后续行边界或兜底信息。 */
  lines?: readonly string[];
  /** 自定义正文（清单/单选列表等结构化内容）；与 lines 二选一，优先于 lines。 */
  children?: ReactNode;
  /** 确认按钮文案（默认「确认」，建议写具体动作如「删除」）。 */
  confirmLabel?: string;
  /** 确认按钮图标（危险操作常用 Trash2，与文案并排）。 */
  confirmIcon?: ReactNode;
  /** 执行中按钮文案。 */
  busyLabel?: string;
  /** 确认后执行中：禁用两侧按钮，防重复提交。 */
  busy?: boolean;
  /** 确认按钮额外禁用条件（如前置数据未加载完成），与 busy 取或。 */
  confirmDisabled?: boolean;
  /** 破坏性操作：确认按钮用危险色。 */
  danger?: boolean;
  onCancel: () => void;
  onConfirm: () => void;
  /** 弹层容器 testid；取消/确认按钮自动追加 -cancel / -confirm。 */
  testId: string;
}) {
  // ESC 关闭与页面级键盘习惯一致；执行中不关，防止中断进行中的操作。
  useEffect(() => {
    if (busy) return;
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") onCancel();
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [busy, onCancel]);

  return (
    <div
      className="switch-veil"
      role="presentation"
      onClick={(event) => {
        // 点击遮罩关闭（与切号弹层同交互）；执行中不允许。
        if (event.target === event.currentTarget && !busy) onCancel();
      }}
    >
      <div
        className="switch-dialog"
        role="dialog"
        aria-modal="true"
        aria-label={title}
        data-testid={testId}
      >
        <div className="switch-dialog__title">{title}</div>
        {children ?? (
          lines && lines.length > 0 ? (
            <ul className="switch-dialog__body">
              {lines.map((line, index) => (
                <li key={index}>{line}</li>
              ))}
            </ul>
          ) : null
        )}
        <div className="switch-dialog__actions">
          <button
            className="btn btn--quiet"
            type="button"
            onClick={onCancel}
            disabled={busy}
            data-testid={`${testId}-cancel`}
          >
            取消
          </button>
          <button
            className={danger ? "btn btn--danger" : "btn btn--primary"}
            type="button"
            onClick={onConfirm}
            disabled={busy || confirmDisabled}
            data-testid={`${testId}-confirm`}
          >
            {confirmIcon}
            {busy ? busyLabel : confirmLabel}
          </button>
        </div>
      </div>
    </div>
  );
}
