import type { ReactNode } from "react";
import { Archive, CheckSquare, ChevronDown, ChevronRight, FolderClosed, Square } from "lucide-react";
import { EmptyState } from "../../../components/shared/EmptyState";
import { Badge, type BadgeTone } from "../../../ui/Badge";
import type { LibraryTone, ProjectNodeView, SessionRowView } from "../config";

/**
 * 库页左栏树（P4e-7a，纯展示）：ProjectNodeView / SessionRowView 递归渲染——具名项目、未关联末位组、
 * 其他账号组、WB cwd 组、归档模式层；展开/选中/快捷归档全由视图字段决定，交互只回调
 *（onActivateNode / onActivateSession / onToggleSession / onQuickArchive / onAction）。
 * 文案与 testid 取自 builder 视图或既有锚点约定（L=LibrarySessionsPanel、WB=WorkBuddyLibraryPage）。
 */
const TREE_TEST_ID = "library-tree"; // L1561 既有容器锚（视图模型未建模容器锚）
const ARCHIVE_EMPTY_TEST_ID = "library-archive-empty"; // L1662 归档视图空态锚（同上）
/** 徽章色档：LibraryTone → Badge 四态（ok=正常态一律中性，UI-DIRECTION §4）。 */
const BADGE_TONE: Record<LibraryTone, BadgeTone> = { neutral: "neutral", ok: "neutral", warn: "warn", danger: "danger", accent: "accent" };

export interface LibrarySessionRowProps {
  readonly row: SessionRowView;
  /** 锚点形态：row = TRAE 行容器（library-session-*）；checkbox = WB 复选输入（workbuddy-library-select-*）。 */
  readonly anchor: "row" | "checkbox";
  readonly onActivate: (row: SessionRowView) => void;
  readonly onToggle: (row: SessionRowView) => void;
  readonly onQuickArchive: (row: SessionRowView) => void;
}

/** 单条会话行（L1944-2001 / WB312-355）：标题 + 徽章 + 时间 + 快捷归档；不可选行（cloud）无按钮角色。 */
export function LibrarySessionRow({ row, anchor, onActivate, onToggle, onQuickArchive }: LibrarySessionRowProps) {
  const activate = () => onActivate(row), archive = row.quickArchive;
  return (
    <div className={`lib-session${row.selected ? " lib-session--checked" : ""}`} role={row.selectable ? "button" : undefined}
      tabIndex={row.selectable ? 0 : undefined} aria-pressed={anchor === "row" && row.selectable ? row.selected : undefined}
      onClick={activate}
      onKeyDown={(event) => {
        if (!row.selectable || (event.key !== "Enter" && event.key !== " ")) return;
        event.preventDefault();
        activate();
      }}
      data-testid={anchor === "row" ? row.testId : undefined}>
      {anchor === "checkbox" ? (
        <span className="lib-check" onClick={(event) => event.stopPropagation()}>
          <input type="checkbox" checked={row.selected} disabled={!row.selectable}
            onChange={() => onToggle(row)} data-testid={row.testId} />
        </span>
      ) : row.selectable ? (
        <span className="lib-check" aria-hidden="true">{row.selected ? <CheckSquare size={15} /> : <Square size={15} />}</span>
      ) : null}
      <span className="lib-session__title">{row.title}</span>
      {row.badges.map((badge, index) => (
        <Badge key={`${badge.text}-${index}`} tone={BADGE_TONE[badge.tone]} hint={badge.title ?? undefined}>{badge.text}</Badge>
      ))}
      <span className="lib-session__time">{row.meta}</span>
      {archive !== null && (
        <button className="lib-session__quick" type="button" title={archive.hint ?? undefined} aria-label={archive.label}
          disabled={archive.disabledReason !== null} data-testid={archive.testId}
          onClick={(event) => { event.stopPropagation(); onQuickArchive(row); }}>
          <Archive size={13} aria-hidden="true" />
        </button>
      )}
    </div>
  );
}

export interface LibraryProjectNodeProps {
  readonly node: ProjectNodeView;
  readonly onActivateNode: (node: ProjectNodeView) => void;
  readonly onActivateSession: (row: SessionRowView) => void;
  readonly onToggleSession: (row: SessionRowView) => void;
  readonly onQuickArchive: (row: SessionRowView) => void;
  readonly onAction: (actionId: string) => void;
}

/** 单个项目节点：模式层与其他账号组容器递归下钻；其余形态 = 行（可点/静态）+ 会话子级。 */
export function LibraryProjectNode(props: LibraryProjectNodeProps) {
  const { node } = props;
  const children = node.children.map((child) => <LibraryProjectNode key={child.id} {...props} node={child} />);
  const rows = node.sessions.map((row) => (
    <LibrarySessionRow key={row.id} row={row} anchor={node.kind === "cwd" ? "checkbox" : "row"}
      onActivate={props.onActivateSession} onToggle={props.onToggleSession} onQuickArchive={props.onQuickArchive} />
  ));
  // 模式层（L1566-1567）无旧容器锚 → 不写 testid；其他账号组容器锚取视图（L1362）。
  if (node.kind === "mode") {
    return <div className="lib-arch-mode"><div className="lib-arch-mode__head">{node.title}</div>{children}</div>;
  }
  if (node.kind === "foreign" && children.length > 0) {
    return <div className="lib-branch" data-testid={node.testId}><div className="lib-foreign__head">{node.title}</div>{children}</div>;
  }
  // 可点行 = 具名项目 / 未关联末位组（L1237）；cwd 组头与其他账号行是静态行（WB301 / L1365）。
  const interactive = node.kind === "named" || node.kind === "unlinked";
  return (
    <div className="lib-branch" data-testid={node.kind === "cwd" ? node.testId : undefined}>
      <div className={`lib-project${node.selected ? " lib-project--checked" : ""}${node.kind === "foreign" ? " lib-project--foreign" : ""}`}
        role={interactive ? "button" : undefined} tabIndex={interactive ? 0 : undefined}
        aria-expanded={interactive ? node.expanded : undefined} aria-pressed={interactive && node.selectable ? node.selected : undefined}
        title={node.titleAttr ?? undefined} onClick={interactive ? () => props.onActivateNode(node) : undefined}
        onKeyDown={(event) => {
          if (!interactive || (event.key !== "Enter" && event.key !== " ")) return;
          event.preventDefault();
          props.onActivateNode(node);
        }}
        data-testid={node.kind === "cwd" ? undefined : node.testId}>
        {interactive && (
          <span className="lib-project__chevron" aria-hidden="true">
            {node.expanded ? <ChevronDown size={14} strokeWidth={1.8} /> : <ChevronRight size={14} strokeWidth={1.8} />}
          </span>
        )}
        {node.selectable && (
          <span className="lib-check" aria-hidden="true">{node.selected ? <CheckSquare size={15} /> : <Square size={15} />}</span>
        )}
        <span className="lib-project__icon" aria-hidden="true"><FolderClosed size={15} strokeWidth={1.8} /></span>
        <span className="lib-project__name">{node.title}</span>
        {node.meta !== "" && <span className={node.kind === "cwd" ? "lib-session__time" : "lib-project__count"}>{node.meta}</span>}
        {node.actions.map((action) => (
          <button key={action.id} className="lib-project__merge" type="button" disabled={action.disabledReason !== null}
            title={action.disabledReason ?? action.hint ?? undefined} data-testid={action.testId}
            onClick={() => props.onAction(action.id)}>{action.label}</button>
        ))}
      </div>
      {(node.kind === "cwd" || node.expanded) && rows.length > 0 && <div className="lib-sessions">{rows}</div>}
    </div>
  );
}

export interface LibraryTreeProps {
  /** 左栏树（view.tree：次序与分组由 builder 决定，本层不排序、不分组）。 */
  readonly tree: readonly ProjectNodeView[];
  /** 空态一句（view.treeEmpty；空串 = 不渲染空态）。 */
  readonly treeEmpty: string;
  /**
   * 归档视图标志（页面按 view.libraryId + 归档入口状态注入）：归档视图树为空时没有模式层节点，
   * 仅凭 tree 无法判定，故由调用方显式给；缺省按 tree 是否含模式层节点推断。
   */
  readonly archiveView?: boolean;
  /**
   * 底部固定区 / 批量栏锚点槽：页面注入「已归档」入口、选择/取消入口与批量栏
   *（锚点 library-archive-entry / library-select-mode / library-action-bar 归注入方持有，本层不发明）。
   */
  readonly footer?: ReactNode;
  /**
   * 树容器锚（缺省 library-tree）：WB 旧页容器锚是 workbuddy-library-groups，由调用方按产品传既有锚，
   * 本组件不判断产品（两种锚都是旧页既有约定，不是新发明）。
   */
  readonly containerTestId?: string;
  readonly onActivateNode: (node: ProjectNodeView) => void;
  readonly onActivateSession: (row: SessionRowView) => void;
  readonly onToggleSession: (row: SessionRowView) => void;
  readonly onQuickArchive: (row: SessionRowView) => void;
  readonly onAction: (actionId: string) => void;
}

/** 树容器：归档视图切灰显样式（lib-tree--archive）与归档空态锚；标志优先取调用方给，缺省按模式层节点推断。 */
export function LibraryTree({ tree, treeEmpty, archiveView, footer, containerTestId = TREE_TEST_ID, ...handlers }: LibraryTreeProps) {
  const archive = archiveView ?? tree.some((node) => node.kind === "mode");
  return (
    <>
      <div className={`lib-tree${archive ? " lib-tree--archive" : ""}`} data-testid={containerTestId}>
        {tree.map((node) => <LibraryProjectNode key={node.id} node={node} {...handlers} />)}
        {tree.length === 0 && treeEmpty !== "" && (
          <EmptyState className="lib-tree__empty" description={treeEmpty}
            testId={archive ? ARCHIVE_EMPTY_TEST_ID : undefined} />
        )}
      </div>
      {footer}
    </>
  );
}
