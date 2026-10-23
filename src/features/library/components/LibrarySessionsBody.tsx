import type { RefObject } from "react";
import { Search } from "lucide-react";
import { LIBRARY_TEXTS, type LibraryView, type ProjectNodeView, type SessionRowView } from "../config";
import { LibraryTree } from "./LibraryTree";
import { SessionViewer, type SessionViewerHeading } from "./SessionViewer";
import { LibraryBatchBar, LibrarySelectButton } from "./LibraryBatchBar";
import { LibraryListControls } from "./LibraryListControls";
import { LibraryArchiveBack, LibraryArchiveEntry, LibraryArchiveNotice, LibraryDraftStorageError } from "./LibraryArchiveBar";
import type { LibrarySelection } from "./useLibrarySelection";

/**
 * 库页会话面板（P4e-8b-2，从 LibraryPage 抽出，纯展示）：控制条 + 左栏（归档返回/搜索 → 恢复条 → 草稿告警 → 树 →
 * 底部「已归档」入口 + 选择入口）+ 右栏查看器 + 批量栏。
 * 旧实现对照 LibrarySessionsPanel.tsx（下称 L）L1503-1730 两栏骨架、L1668-1702 底部固定区、L1769-1849 批量栏；
 * WorkBuddyLibraryPage WB290-418 同构。文案、锚点、禁用理由全部取自视图，本层不拼句、不判断产品、不发命令。
 */
export interface LibrarySessionsBodyProps {
  readonly view: LibraryView;
  readonly heading: SessionViewerHeading | null;
  readonly treeTestId?: string;
  readonly selection: LibrarySelection;
  /** 搜索框原始值（受控；归档视图视图不给 search，本层也不渲染）。 */
  readonly searchText: string;
  readonly onSearch: (text: string) => void;
  /** 账号过滤 / 软删开关变化（WB 才渲染控制条；切换即清勾选，旧 WB257-286）。 */
  readonly onFilterChange: (value: string) => void;
  readonly onIncludeDeletedChange: (checked: boolean) => void;
  readonly flowRef: RefObject<HTMLDivElement>;
  readonly onTabChange: (tab: "messages" | "relay") => void;
  readonly onCloseViewer: () => void;
  readonly onLoadOlder: () => void;
  readonly onAction: (actionId: string) => void;
  readonly onActivateNode: (node: ProjectNodeView) => void;
  readonly onActivateSession: (row: SessionRowView) => void;
  readonly onToggleSession: (row: SessionRowView) => void;
  readonly onQuickArchive: (row: SessionRowView) => void;
  /** 恢复条重试（归档动作 hook 的 retry）。 */
  readonly onRetryRecovery: () => void;
  /** 归档执行中（恢复条与草稿条按钮转忙碌）。 */
  readonly archiveBusy: boolean;
}

export function LibrarySessionsBody(props: LibrarySessionsBodyProps) {
  const { view, selection, archiveBusy } = props, archive = view.archive;
  const footer = archive === null && view.selectionEntry === null ? null : (
    <div className="lib-foot">
      {archive?.entry != null && <LibraryArchiveEntry entry={archive.entry} onAction={props.onAction} />}
      {view.selectionEntry !== null && (
        <LibrarySelectButton selectMode={selection.selectMode} label={view.selectionEntry.label}
          cancelLabel={view.selectionEntry.cancelLabel} onToggle={selection.selectMode ? selection.exit : selection.enter} />
      )}
    </div>
  );
  return (
    <>
      <LibraryListControls filter={view.filter} includeDeleted={view.includeDeleted}
        onFilterChange={(value) => { props.onFilterChange(value); selection.reset(); }}
        onIncludeDeletedChange={(checked) => { props.onIncludeDeletedChange(checked); selection.reset(); }} />
      <div className="history-body">
        <aside className="lib-panel" aria-label={LIBRARY_TEXTS.regionSessions}>
          {archive?.back != null ? <LibraryArchiveBack back={archive.back} onAction={props.onAction} />
            : view.search !== null && (
              <div className="lib-search">
                <Search size={14} aria-hidden="true" />
                <input type="text" placeholder={view.search.placeholder} value={props.searchText}
                  onChange={(event) => props.onSearch(event.target.value)} aria-label={view.search.ariaLabel}
                  data-testid={view.search.testId} />
              </div>
            )}
          {/* 需就地恢复的异常收尾优先于草稿告警（L1530-1557 次序）。 */}
          {archive?.recovery != null && archive.retryLabel !== null && (
            <LibraryArchiveNotice recovery={archive.recovery} retryLabel={archive.retryLabel} busy={archiveBusy}
              onRetry={props.onRetryRecovery} />
          )}
          {archive?.draftError != null && <LibraryDraftStorageError message={archive.draftError} />}
          <LibraryTree tree={view.tree} treeEmpty={view.treeEmpty} archiveView={archive?.view ?? false}
            containerTestId={props.treeTestId} onActivateNode={props.onActivateNode}
            onActivateSession={props.onActivateSession} onToggleSession={props.onToggleSession}
            onQuickArchive={props.onQuickArchive} onAction={props.onAction} footer={footer} />
        </aside>
        <SessionViewer view={view.viewer} heading={props.heading ?? { title: "", projectName: "", messageCount: 0, lastActiveLabel: "" }}
          flowRef={props.flowRef} onTabChange={props.onTabChange} onClose={props.onCloseViewer} onLoadOlder={props.onLoadOlder} />
      </div>
      {/* 批量栏：TRAE 选择模式浮出浮动栏（fixed 不占流）；WB 页内动作行常驻（树之后、体检卡之前，WB393）。 */}
      <LibraryBatchBar bar={view.batchBar} selectMode={selection.selectMode} onAction={props.onAction} />
    </>
  );
}
