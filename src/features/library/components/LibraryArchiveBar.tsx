import { Archive, ArrowLeft, RefreshCw } from "lucide-react";
import { ConfirmDialog } from "../../../components/shared/ConfirmDialog";
import type { ArchiveChromeView, LibraryView } from "../config";
import { LibraryActionButton } from "./LibraryShell";

/**
 * 库页归档动作区（P4e-8b-2，纯展示）：左栏「已归档 N」入口 / 归档视图「返回对话列表」/ 恢复条（带重试）/
 * 草稿存储告警 / Work 库「关闭客户端」应用前确认。旧实现对照 LibrarySessionsPanel.tsx（下称 L）
 * L1506-1515 返回、L1530-1548 恢复条、L1549-1557 草稿告警、L1668-1683 归档入口、L1884-1901 应用前确认。
 * 文案、锚点、按钮词一律取自视图（builder 给），本层不拼句、不判断产品、不发命令，只回调 onAction(actionId)。
 */

/** 左栏「已归档 N」入口（L1672-1683）：仅在正常视图且确有归档会话时由视图给出。 */
export function LibraryArchiveEntry({ entry, onAction }: {
  readonly entry: NonNullable<ArchiveChromeView["entry"]>;
  readonly onAction: (actionId: string) => void;
}) {
  return (
    <button className="lib-archive-entry" type="button" onClick={() => onAction(entry.action.id)}
      title={entry.action.hint ?? undefined} data-testid={entry.action.testId}>
      <Archive size={14} aria-hidden="true" />
      <span>{entry.action.label}</span>
      <span className="lib-archive-entry__count">{entry.count}</span>
    </button>
  );
}

/** 归档视图「返回对话列表」（L1506-1515）：锚 library-archive-back 逐字沿用。 */
export function LibraryArchiveBack({ back, onAction }: {
  readonly back: NonNullable<ArchiveChromeView["back"]>;
  readonly onAction: (actionId: string) => void;
}) {
  return (
    <div className="lib-back">
      <button className="btn btn--quiet" type="button" onClick={() => onAction(back.id)} data-testid={back.testId}>
        <ArrowLeft size={15} aria-hidden="true" />{back.label}
      </button>
    </div>
  );
}

/** 恢复条（L1530-1548）：一句原因 + 重试按钮；执行中按钮转忙碌句并禁用。 */
export function LibraryArchiveNotice({ recovery, retryLabel, busy, onRetry }: {
  readonly recovery: NonNullable<ArchiveChromeView["recovery"]>;
  readonly retryLabel: string;
  readonly busy: boolean;
  readonly onRetry: () => void;
}) {
  return (
    <div className="lib-notice lib-notice--action" data-testid="action-notice" role="status">
      <span className="lib-notice__message">{recovery.message}</span>
      <button className="btn btn--quiet" type="button" onClick={onRetry} disabled={busy}
        data-testid="archive-recovery-retry">
        {busy ? <RefreshCw size={15} className="icon-spin" aria-hidden="true" /> : null}
        {busy ? "处理中…" : retryLabel}
      </button>
    </div>
  );
}

/** 草稿存储告警（L1549-1557）：后端与本地存储双失败才出现，原文取自草稿 hook。 */
export function LibraryDraftStorageError({ message }: { readonly message: string }) {
  return <div className="lib-notice lib-notice--warning" data-testid="archive-draft-storage-error" role="alert">{message}</div>;
}

/** Work 库应用前确认（L1884-1901）：关闭客户端 → 应用 → 重启；CN 只读库视图不给该单。 */
export function LibraryArchiveConfirm({ confirm, busy, onCancel, onConfirm }: {
  readonly confirm: NonNullable<ArchiveChromeView["confirm"]>;
  readonly busy: boolean;
  readonly onCancel: () => void;
  readonly onConfirm: () => void;
}) {
  return (
    <ConfirmDialog title={confirm.title} lines={confirm.lines} confirmLabel={confirm.confirmLabel}
      confirmIcon={<Archive size={15} aria-hidden="true" />} busy={busy}
      onCancel={onCancel} onConfirm={onConfirm} testId={confirm.testId} />
  );
}

/** 归档草稿条（L1851-1882）：锚 archive-draft-bar；放弃 / 应用两动作取自视图，应用受写门禁与恢复态约束。 */
export function LibraryDraftBar({ bar, busy, onAction }: {
  readonly bar: NonNullable<LibraryView["draftBar"]>;
  readonly busy: boolean;
  readonly onAction: (actionId: string) => void;
}) {
  return (
    <div className="lib-actionbar lib-actionbar--draft" role="toolbar" aria-label={bar.ariaLabel} data-testid={bar.testId}>
      <span className="lib-actionbar__count">{bar.summary}</span>
      <div className="lib-actionbar__actions">
        <LibraryActionButton action={bar.discard} busy={busy} onClick={() => onAction(bar.discard.id)} />
        <LibraryActionButton action={bar.apply} busy={busy} onClick={() => onAction(bar.apply.id)} />
      </div>
    </div>
  );
}
