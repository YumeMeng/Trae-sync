import { Layers, RefreshCw } from "lucide-react";
import { EmptyState } from "../../../components/shared/EmptyState";
import type { LibraryView } from "../config";

/**
 * 库页首读三态卡（P4e-8c 从 LibraryPage 抽出，纯展示）：读取失败卡 / 整页空态卡。
 * 旧实现对照 LibrarySessionsPanel.tsx（下称 L）L1396-1413 错误卡、L1425-1481 两档引导卡。
 * 锚点、标题、说明与动作词全部取自视图（testId 逐字沿用旧锚 history-error / history-guide；
 * 启动入口锚 history-guide-launch 只在视图给出时渲染）。本层不判断产品、不发起读取、不拼文案。
 */

/** 读取失败卡（L1396-1413 / L1482-1500）：一句失败原因 + 重试按钮。 */
export function LibraryErrorCard({ error, onRetry }: {
  readonly error: NonNullable<LibraryView["error"]>;
  readonly onRetry: () => void;
}) {
  return (
    <EmptyState panel className="history-empty" testId={error.testId ?? undefined} role="alert"
      title={error.title} description={error.description === "" ? undefined : error.description}
      action={
        <button className="btn" type="button" onClick={onRetry}>
          <RefreshCw size={15} aria-hidden="true" />{error.retryLabel}
        </button>
      } />
  );
}

/** 整页空态卡（L1425-1481）：无库数据 / 未登记账号两档，同一锚；引导动作按视图给不给。 */
export function LibraryEmptyCard({ empty, onAction }: {
  readonly empty: NonNullable<LibraryView["empty"]>;
  readonly onAction: (actionId: string) => void;
}) {
  return (
    <EmptyState panel className="history-empty" testId={empty.testId === "" ? undefined : empty.testId}
      icon={<Layers size={26} strokeWidth={1.6} />} title={empty.title}
      description={empty.hint === "" ? undefined : empty.hint}
      action={empty.actionLabel === null ? undefined : (
        <button className="btn btn--primary" type="button" onClick={() => onAction(empty.actionId)}
          data-testid={empty.actionTestId ?? undefined}>
          {empty.actionLabel}
        </button>
      )} />
  );
}
