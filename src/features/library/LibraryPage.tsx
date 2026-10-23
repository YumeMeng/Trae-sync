/**
 * 库页装配（P4e-8a 读取与渲染 + P4e-8b-1 动作接线）。
 * 数据面 useTraeLibrary / useWbLibrary 经 libraryAssemblers 的 ASSEMBLERS 查表（键 = productId，页面零产品判断、
 * 零 invoke、零命令名）；视图模型 registry.libraryViewFor；展示组件 LibraryShell / LibraryTree / SessionViewer /
 * LibraryInfoTab / LibraryBatchBar / LibraryDeleteDialogs / WbCheckupCard。
 * 本文件不发明文案与 testid：标题、tab、锚点、空态句一律取自 LibraryView。动作接线（选择态、批量栏、删除三链）
 * 抽到 useLibrarySelection / useLibraryPageActions；页面只做装配与分发。
 */
import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { Search } from "lucide-react";
import { EmptyState } from "../../components/shared/EmptyState";
import { LIBRARY_TEXTS, type LibraryTabId, type LibraryView, type ProjectNodeView, type SessionRowView } from "./config";
import { libraryViewFor, type LibrarySnapshot } from "./registry";
import { useTraeLibrary, type TraeLibraryPageFields } from "./components/useTraeLibrary";
import { useWbLibrary, type WbLibraryPageFields } from "./components/useWbLibrary";
import { ASSEMBLERS, EMPTY_DRAFT } from "./components/libraryAssemblers";
import { LibraryShell } from "./components/LibraryShell";
import { LibraryTree } from "./components/LibraryTree";
import { SessionViewer, type SessionViewerHeading } from "./components/SessionViewer";
import { LibraryInfoTab } from "./components/LibraryInfoTab";
import { LibraryBatchBar, LibrarySelectToggle } from "./components/LibraryBatchBar";
import { LibraryListControls } from "./components/LibraryListControls";
import { LibraryStatsCard } from "./components/LibraryStatsCard";
import { LibraryDeleteDialogs } from "./components/LibraryDeleteDialogs";
import { WbCheckupCard } from "./components/WbCheckupCard";
import { useLibrarySelection } from "./components/useLibrarySelection";
import { useLibraryPageActions } from "./components/useLibraryPageActions";
import type { DeleteTargetView } from "./components/useLibraryDeleteActions";
import type { LibrarySelectionResolvers } from "./components/useLibrarySelection";
import type { Assembled } from "./components/libraryAssemblers";

/** 删除确认层锚点契约（旧页 L2285）：TRAE 层经 testId 传此值，逐字沿用旧锚（WB 两层各取订单 testId）。 */
export const LIBRARY_DELETE_CONFIRM_TEST_ID = "delete-confirm";
/** 无会话时的标题区占位（SessionViewer 在 view=null 时整块不渲染标题区，本值只满足签名）。 */
const EMPTY_HEADING: SessionViewerHeading = { title: "", projectName: "", messageCount: 0, lastActiveLabel: "" };
/** 本票不接线的出口（归档/归并/插件归后续票）：不发明假回执。 */
const NO_ACTION = () => undefined;
const NO_BASE = (_stamp: number | null) => undefined;
/** 未登记产品的兜底：无删除目标、刷新不发起读取（页面已 fail-closed 渲染）。 */
const NO_TARGETS = (_sessionIds: readonly string[], _projectIds: readonly string[]): readonly DeleteTargetView[] => [];
const NO_REFRESH = async (): Promise<{ readonly ok: boolean }> => ({ ok: true });
/** 未登记产品的选择解析兜底：无项目、无归属（选择态仍可用但不会勾出目标）。 */
const EMPTY_SELECTION_RESOLVERS: LibrarySelectionResolvers = { projectIdsOfNode: () => [], projectIdOfSession: () => null };

export interface LibraryPageProps {
  /** 页面可见才读取（后台页保持挂载但不发 IPC）。 */
  readonly active: boolean;
  /** 当前产品协议值：只用于查装配表，页面内不做任何产品判断。 */
  readonly productId: string;
  /** 库实例 id（缺省主库；副环境为 env_id；只读数据根为 trae_cn）。 */
  readonly libraryId?: string;
  /** 副环境库显示名（缺省由 builder 回退「副库」）。 */
  readonly libraryDisplayName?: string;
  /** 返回环境页（页头「返回」）；缺省时该动作原样透出到 onAction。 */
  readonly onBack?: () => void;
  /** 库内跳转到另一库详情：本票只登记签名（8b-2 接线），不调用。 */
  readonly onOpenLibraryDetail?: (libraryId: string) => void;
  /** 其余动作（草稿条 / 插件 / 归并 / 空态引导）：原样透出，本票不实现。 */
  readonly onAction?: (actionId: string) => void;
}

export function LibraryPage(props: LibraryPageProps) {
  const { active, productId, libraryId = "master", libraryDisplayName, onBack, onAction } = props;
  const now = new Date();
  const [activeTab, setActiveTab] = useState<LibraryTabId>("sessions");
  const [searchText, setSearchText] = useState("");
  const [expandedIds, setExpandedIds] = useState<readonly string[]>([]);
  const [viewerTab, setViewerTab] = useState<"messages" | "relay">("messages");
  const [uidFilter, setUidFilter] = useState("");
  const [includeDeleted, setIncludeDeleted] = useState(false);
  const [pending, setPending] = useState(false); // 刷新在跑位（转圈 + 防连点）
  // 选择态（选择模式 / 勾选 / 清空）：解析器经 getter 现取（装配完成后才有真实快照，避免循环依赖）。
  const assembledRef = useRef<Assembled | null>(null);
  const selection = useLibrarySelection(() => assembledRef.current?.selection ?? EMPTY_SELECTION_RESOLVERS);
  const page: TraeLibraryPageFields & WbLibraryPageFields = {
    activeTab, searchText, expandedIds, viewerTab, uidFilter, includeDeleted,
    archiveDraft: EMPTY_DRAFT, selectedProjectIds: selection.selectedProjectIds, selectedSessionIds: selection.selectedSessionIds,
    selectMode: selection.selectMode, archiveView: false, archiveCollapsedKeys: [], busy: false, archiveRecovery: null, draftStorageError: null,
    selectedIds: selection.selectedSessionIds, writeBusy: false, mergeBusy: false,
  };
  const trae = useTraeLibrary(page, { productId, libraryId, libraryDisplayName, active, now });
  const wb = useWbLibrary(page, { productId, active, now });
  const assemble = ASSEMBLERS[productId];
  const assembled = assemble === undefined ? null : assemble({ trae, wb, now, selectMode: selection.selectMode,
    selectedSessionIds: selection.selectedSessionIds, selectedProjectIds: selection.selectedProjectIds });
  assembledRef.current = assembled; // 选择态解析器在点击时现取（渲染期写入，不触发重渲染）
  const view: LibraryView | null = assembled === null ? null : libraryViewFor(assembled.snapshot);
  // 查看器滚动锚定（旧 L2110-2121）：首次滚到底；「加载更早」拼接后按高度差保持阅读位置。
  const flowRef = useRef<HTMLDivElement>(null);
  const keepScroll = useRef<{ top: number; height: number } | null>(null);
  const messageCount = view?.viewer?.messages.length ?? 0, messageOffset = view?.viewer?.messageOffset ?? 0;
  useLayoutEffect(() => {
    const flow = flowRef.current;
    if (flow === null) return;
    const previous = keepScroll.current;
    if (previous === null) { flow.scrollTop = flow.scrollHeight; return; }
    flow.scrollTop = previous.top + Math.max(0, flow.scrollHeight - previous.height);
    keepScroll.current = null;
  }, [messageCount, messageOffset]);
  // 删除三链（TRAE 真删 / WB 软删 / WB 彻底删）：弹层单与忙碌位由 hook 持有，动作 id 命中即被页面吞掉。
  const actions = useLibraryPageActions({
    view, selection, libraryId, userId: uidFilter,
    targets: assembled === null ? NO_TARGETS : assembled.targets,
    refresh: assembled === null ? NO_REFRESH : assembled.refresh,
    refreshCheckup: assembled?.refreshCheckup,
    onDeleted: (ids) => {
      const open = assembled?.viewerSessionId ?? null;
      if (open !== null && ids.includes(open)) assembled?.closeSession(); // 旧 L1062：删掉正在看的会话即关右栏
    },
  });
  // Esc 退出选择模式（旧 L1187-1197 口径）：确认弹层开着时让弹层先关，不越级退模式。
  useEffect(() => {
    if (!selection.selectMode) return;
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape" && actions.dialog === null) selection.exit();
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [selection, actions.dialog]);
  // 刷新是读取不是动作：页面就地执行（旧详情页 load()），失败不清已取回数据、不翻错误卡。
  const refresh = async () => {
    if (pending || assembled === null) return;
    setPending(true);
    try { await assembled.refresh({ force: true }); } finally { setPending(false); }
  };
  const busyIds = pending && view?.header.refresh !== null && view !== null ? [view.header.refresh.id] : [];
  const handleAction = (actionId: string) => {
    if (actions.handleAction(actionId)) return; // 删除三链：命中即执行（弹层 / 直接动作）
    if (actionId === view?.header.refresh?.id) { void refresh(); return; }
    if (actionId === view?.header.back?.id && onBack !== undefined) { onBack(); return; }
    onAction?.(actionId); // 其余动作一律透出：草稿条 / 插件 / 归并 / 空态引导归后续票
  };
  // 项目行：选择模式 = 勾选其下全部会话并立即展开（旧 L1223-1230：让勾选结果可见），浏览态 = 展开/收起。
  const toggleNode = (node: ProjectNodeView) => {
    if (selection.selectMode) {
      selection.toggleProject(node);
      setExpandedIds((current) => (current.includes(node.id) ? current : [...current, node.id]));
      return;
    }
    setExpandedIds((current) => (current.includes(node.id) ? current.filter((id) => id !== node.id) : [...current, node.id]));
  };
  // 点行（标题区）：TRAE 选择模式 = 勾选（L1962 三态判定），浏览态 = 打开查看器。
  const activateSession = (row: SessionRowView) => {
    if (selection.selectMode) selection.toggleSession(row.id);
    else assembled?.openSession(row.id);
  };
  const toggleSession = (row: SessionRowView) => selection.toggleSession(row.id); // 复选输入（WB 行锚）：常显
  const quickArchive = (row: SessionRowView) => { if (row.quickArchive !== null) onAction?.(row.quickArchive.id); };
  const loadOlder = () => {
    const flow = flowRef.current;
    if (flow !== null) keepScroll.current = { top: flow.scrollTop, height: flow.scrollHeight };
    assembled?.loadOlder();
  };

  // 未登记产品 fail-closed：不猜数据面，按 registry 空视图给一句失败说明（不套用任一产品的行语义）。
  if (assembled === null || view === null) {
    const fallback = libraryViewFor({ productId } as unknown as LibrarySnapshot);
    return (
      <LibraryShell view={fallback} onAction={NO_ACTION} onTabChange={NO_ACTION}>
        <EmptyState panel role="alert" description={fallback.empty?.title} />
      </LibraryShell>
    );
  }
  // 会话面板：控制条（账号过滤 / 软删开关）+ 左栏搜索 + 树 + 底部选择入口 / 右栏查看器。
  const sessions = (
    <>
      <LibraryListControls filter={view.filter} includeDeleted={view.includeDeleted}
        onFilterChange={(value) => { setUidFilter(value); selection.reset(); }} // 切换过滤即清选择（旧 WB257-286）
        onIncludeDeletedChange={(checked) => { setIncludeDeleted(checked); selection.reset(); }} />
      <div className="history-body">
        <aside className="lib-panel" aria-label={LIBRARY_TEXTS.regionSessions}>
          {view.search !== null && (
            <div className="lib-search">
              <Search size={14} aria-hidden="true" />
              <input type="text" placeholder={view.search.placeholder} value={searchText}
                onChange={(event) => setSearchText(event.target.value)} aria-label={view.search.ariaLabel}
                data-testid={view.search.testId} />
            </div>
          )}
          <LibraryTree tree={view.tree} treeEmpty={view.treeEmpty} archiveView={false}
            containerTestId={assembled.treeTestId} onActivateNode={toggleNode}
            onActivateSession={activateSession} onToggleSession={toggleSession}
            onQuickArchive={quickArchive} onAction={handleAction}
            footer={view.selectionEntry === null ? null : (
              <LibrarySelectToggle selectMode={selection.selectMode} label={view.selectionEntry.label}
                cancelLabel={view.selectionEntry.cancelLabel}
                onToggle={selection.selectMode ? selection.exit : selection.enter} />
            )} />
        </aside>
        <SessionViewer view={view.viewer} heading={assembled.heading ?? EMPTY_HEADING} flowRef={flowRef}
          onTabChange={setViewerTab} onClose={assembled.closeSession} onLoadOlder={loadOlder} />
      </div>
      {/* 批量栏：TRAE 选择模式浮出浮动栏（G22，fixed 不占流）；WB 页内动作行常驻（逐会话复选，§2-3 B，
          位置同旧页 WB393：树之后、体检卡之前）。 */}
      <LibraryBatchBar bar={view.batchBar} selectMode={selection.selectMode} onAction={handleAction} />
    </>
  );
  const blocked = view.loadingNote !== null || view.error !== null || view.empty !== null;

  return (
    <>
      <LibraryShell view={view} className={assembled.shellClass} busy={busyIds} onAction={handleAction}
        onTabChange={setActiveTab}>
        {view.notices.map((notice, index) => <p className="checkin-page__meta" role="status" key={index}>{notice}</p>)}
        {view.stats !== null && <LibraryStatsCard view={view.stats} />}
        {view.loadingNote !== null && <EmptyState role="status" description={view.loadingNote} />}
        {view.error !== null && (
          <EmptyState panel role="alert" title={view.error.title} description={view.error.description}
            action={<button className="btn" type="button" onClick={() => void refresh()}>{view.error.retryLabel}</button>} />
        )}
        {view.empty !== null && (
          <EmptyState panel title={view.empty.title} description={view.empty.hint === "" ? undefined : view.empty.hint} />
        )}
        {/* tab 面板按 view.tabs 切换（WB 无 tab：正文直接落树 + 查看器 + 体检卡）。插件 tab 面板留后续票。 */}
        {!blocked && (view.tabs.length > 0 ? (
          <>
            <div className="master-detail__tabpanel" hidden={activeTab !== "sessions"}>{sessions}</div>
            <div className="master-detail__tabpanel" hidden={activeTab !== "plugins"} />
            <div className="master-detail__tabpanel" hidden={activeTab !== "info"}>
              {view.info !== null && (
                <LibraryInfoTab view={view.info} checkup={view.checkup} busy={busyIds} onAction={handleAction} onBackupBase={NO_BASE} />
              )}
            </div>
          </>
        ) : (
          <>
            {sessions}
            {view.checkup !== null && <WbCheckupCard view={view.checkup} onAction={handleAction} />}
          </>
        ))}
      </LibraryShell>
      {/* 删除确认层：TRAE 层锚逐字沿用旧页 delete-confirm；WB 两层各取订单 testId（决策 7 不共用，
          故覆盖值只给 TRAE 层——否则会把软删/彻底删的锚一起改写）。 */}
      <LibraryDeleteDialogs dialog={actions.dialog} busy={actions.busy}
        testId={actions.dialog?.kind === "trae-delete" ? LIBRARY_DELETE_CONFIRM_TEST_ID : undefined}
        onCancel={actions.cancel} onConfirm={actions.confirm} />
    </>
  );
}
