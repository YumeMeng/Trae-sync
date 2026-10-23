/**
 * 库页装配（P4e-8a 读取与渲染 + 8b-1 删除三链 + 8b-2 归档与归并接线 + 8b-3 插件 tab 与库信息 tab）。
 * 数据面 useTraeLibrary / useWbLibrary 经 libraryAssemblers 的 ASSEMBLERS 查表（键 = productId，页面零产品判断、
 * 零 invoke、零命令名）；视图模型 registry.libraryViewFor；展示组件 LibraryShell / LibrarySessionsBody / LibraryTree /
 * SessionViewer / LibraryInfoTab / LibraryBatchBar / LibraryArchiveBar / WbCheckupCard / 两组确认弹层。
 * 本文件不发明文案与 testid：标题、tab、锚点、空态句一律取自 LibraryView。动作接线（选择态、删除三链、归档编排、
 * 三条归并链、插件安装/卸载、备份对比校验、WB 体检）抽到 useLibrarySelection / useLibraryPageActions /
 * useArchiveActions / useLibraryMergeActions / usePluginTab / useLibraryVerification；
 * 页面只做装配与分发（动作 id 判别顺序：归档 → 删除 → 归并 → 插件 → 校验 → 体检 → 刷新/返回 → 透出）。
 */
import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { LIBRARY_TEXTS, fillLibraryText, libraryCapabilitiesFor,
  type LibraryTabId, type LibraryView, type ProjectNodeView, type SessionRowView } from "./config";
import { libraryViewFor, type LibrarySnapshot } from "./registry";
import { useTraeLibrary, type TraeLibraryPageFields } from "./components/useTraeLibrary";
import { useWbLibrary, type WbLibraryPageFields } from "./components/useWbLibrary";
import { ASSEMBLERS, type Assembled } from "./components/libraryAssemblers";
import { EmptyState } from "../../components/shared/EmptyState";
import { LibraryShell } from "./components/LibraryShell";
import { LibrarySessionsBody } from "./components/LibrarySessionsBody";
import { LibraryInfoTab } from "./components/LibraryInfoTab";
import { LibraryPluginTab } from "./components/LibraryPluginTab";
import { usePluginTab } from "./components/usePluginTab";
import { useLibraryVerification } from "./components/useLibraryVerification";
import { pluginTabEnabled } from "./pluginTab";
import { notify } from "../../platform/notify";
import { LibraryDeleteDialogs } from "./components/LibraryDeleteDialogs";
import { LibraryMergeDialogs, type LibraryMergeConfirm } from "./components/LibraryMergeDialogs";
import { LibraryArchiveConfirm, LibraryDraftBar } from "./components/LibraryArchiveBar";
import { WbCheckupCard } from "./components/WbCheckupCard";
import { useLibrarySelection, type LibrarySelectionResolvers } from "./components/useLibrarySelection";
import { useLibraryDeleteActions, type DeleteTargetView, type TraeArchiveOrder } from "./components/useLibraryDeleteActions";
import { useLibraryPageActions } from "./components/useLibraryPageActions";
import { useArchiveActions } from "./components/useArchiveActions";
import { useLibraryMergeActions } from "./components/useLibraryMergeActions";

/** 删除确认层锚点契约（旧页 L2285）：TRAE 层经 testId 传此值，逐字沿用旧锚（WB 两层各取订单 testId）。 */
export const LIBRARY_DELETE_CONFIRM_TEST_ID = "delete-confirm";
/** 未登记产品的兜底出口：不发明假回执。 */
const NO_ACTION = () => undefined;
/** 未登记产品的兜底：无删除目标、刷新不发起读取、无归档/归并候选（页面已 fail-closed 渲染）。 */
const NO_TARGETS = (_sessionIds: readonly string[], _projectIds: readonly string[]): readonly DeleteTargetView[] => [];
const NO_REFRESH = async (): Promise<{ readonly ok: boolean }> => ({ ok: true });
const NO_VISIBLE = (): readonly string[] => [];
const NO_MERGE_CANDIDATES = () => [];
const NO_FOREIGN = () => null;
const NO_CWD = () => null;
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
  /** 库内跳转到另一库详情：本票只登记签名，不调用。 */
  readonly onOpenLibraryDetail?: (libraryId: string) => void;
  /** 其余动作（插件 / 空态引导）：原样透出，本票不实现。 */
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
  // 删除/归档同一个动作实例：两条链共用一个忙碌位（归档与删除互斥，后端也互斥），页面只持一份。
  // 先于读取 hook 调用：归档 hook 的入参全是惰性 getter（装配后现取），不构成循环依赖。
  const del = useLibraryDeleteActions({
    refresh: async () => (assembledRef.current ?? { refresh: NO_REFRESH }).refresh({ force: true }),
    refreshCheckup: async () => { await assembledRef.current?.refreshCheckup?.(); }, // WB 写后重跑体检（旧 WB154）
  });
  const archive = useArchiveActions({
    active, libraryId,
    // 静态能力：能力表声明该产品有归档（草稿机制只在此时读写；WB 无归档即全程不碰存储）。
    supported: libraryCapabilitiesFor(productId).archive === "draft-apply",
    canArchive: () => assembledRef.current?.archiveEnabled ?? false,
    confirmRequired: () => assembledRef.current?.archiveConfirmRequired ?? false,
    refresh: (options) => (assembledRef.current ?? { refresh: NO_REFRESH }).refresh(options),
    applyArchive: (order: TraeArchiveOrder) => del.applyArchive(order),
    archiving: del.busy.archiving, selection,
    visibleSelection: (input) => assembledRef.current?.visibleSelection(input.draft, input.archiveView, input.sessionIds, input.projectIds) ?? NO_VISIBLE(),
    viewerSessionId: assembledRef.current?.viewerSessionId ?? null,
    closeSession: () => assembledRef.current?.closeSession(),
  });
  // 插件 tab 与库信息 tab 的读取：按「页面可见 + 该 tab 当前激活」双门禁（旧 P135-139 / V69-73 同口径），
  // 切走即停读；安装/卸载成功重读插件清单，基准切换与重新校验重读备份对比（各自 hook 内闭环）。
  const plugins = usePluginTab({ enabled: pluginTabEnabled(productId, libraryId), active: active && activeTab === "plugins" });
  const verify = useLibraryVerification({ libraryId, active: active && activeTab === "info" });
  // 页面状态（原样进两条读取 hook 的快照）：归档页状态在本渲染即取真值（不经过 ref，无跨渲染滞后）。
  const page: TraeLibraryPageFields & WbLibraryPageFields = {
    activeTab, searchText, expandedIds, viewerTab, uidFilter, includeDeleted,
    archiveDraft: archive.page.draft, selectedProjectIds: selection.selectedProjectIds, selectedSessionIds: selection.selectedSessionIds,
    selectMode: selection.selectMode, archiveView: archive.page.view, archiveCollapsedKeys: archive.page.collapsedKeys,
    busy: false, archiveRecovery: archive.page.recovery, draftStorageError: archive.page.draftError,
    selectedIds: selection.selectedSessionIds, writeBusy: false, mergeBusy: false,
    verificationState: verify.state, pluginTab: plugins.data,
  };
  const trae = useTraeLibrary(page, { productId, libraryId, libraryDisplayName, active, now });
  const wb = useWbLibrary(page, { productId, active, now });
  const assemble = ASSEMBLERS[productId];
  const merge = useLibraryMergeActions({
    libraryId,
    refresh: (options) => (assembledRef.current ?? { refresh: NO_REFRESH }).refresh(options),
    refreshCheckup: async () => { await assembledRef.current?.refreshCheckup?.(); }, // WB cwd 归并后重跑体检（旧 WB171）
    mergeCandidates: (projectIds) => assembledRef.current?.mergeCandidates(projectIds) ?? NO_MERGE_CANDIDATES(),
    foreignProject: (projectId) => assembledRef.current?.foreignProject(projectId) ?? NO_FOREIGN(),
    cwdCandidate: (canonical) => assembledRef.current?.cwdCandidate(canonical) ?? NO_CWD(),
    selection,
  });
  // 页面忙碌位：归档 / 删除 / 归并任一在跑（同一动作实例 + 两条独立链）。
  const busyAll = del.busy.archiving || del.busy.deleting || del.busy.softDeleting || del.busy.purging
    || merge.busy.project || merge.busy.traeCn || merge.busy.cwd;
  const assembled = assemble === undefined ? null : assemble({ trae, wb, now, selectMode: selection.selectMode,
    selectedSessionIds: selection.selectedSessionIds, selectedProjectIds: selection.selectedProjectIds,
    archive: archive.page, busy: busyAll, wbBusy: { writeBusy: del.busy.softDeleting || del.busy.purging, mergeBusy: merge.busy.cwd } });
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
  // 删除三链（TRAE 真删 / WB 软删 / WB 彻底删）：弹层单与忙碌位由实例持有，动作 id 命中即被页面吞掉。
  const actions = useLibraryPageActions({
    view, selection, libraryId, userId: uidFilter, del,
    targets: assembled === null ? NO_TARGETS : assembled.targets,
    onDeleted: (ids) => {
      const open = assembled?.viewerSessionId ?? null;
      if (open !== null && ids.includes(open)) assembled?.closeSession(); // 旧 L1062：删掉正在看的会话即关右栏
    },
  });
  const dialogOpen = actions.dialog !== null || merge.dialog !== null || archive.confirmOpen;
  // Esc 退出选择模式（旧 L1187-1197 口径）：确认弹层开着时让弹层先关，不越级退模式。
  useEffect(() => {
    if (!selection.selectMode) return;
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape" && !dialogOpen) selection.exit();
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [selection, dialogOpen]);
  // 刷新是读取不是动作：页面就地执行（旧详情页 load()），失败不清已取回数据、不翻错误卡。
  const refresh = async () => {
    if (pending || assembled === null) return;
    setPending(true);
    try { await assembled.refresh({ force: true }); } finally { setPending(false); }
  };
  const busyIds = pending && view?.header.refresh !== null && view !== null ? [view.header.refresh.id] : [];
  const handleAction = (actionId: string) => {
    if (archive.handleAction(actionId)) return; // 归档草稿 / 应用 / 入口 / 返回：命中即执行
    if (actions.handleAction(actionId)) return; // 删除三链：命中即执行（弹层 / 直接动作）
    if (merge.handleAction(actionId)) return; // 归并三链：命中即执行（预览 / 弹层）
    if (plugins.handleAction(actionId)) return; // 插件刷新（plugin-refresh）：重读两路
    if (verify.handleAction(actionId)) return; // 备份对比重新校验（verification-refresh）
    if (actionId === "checkup-run") { void runCheckup(); return; } // WB 立即体检（旧 WB470-479）
    if (actionId === view?.header.refresh?.id) { void refresh(); return; }
    if (actionId === view?.header.back?.id && onBack !== undefined) { onBack(); return; }
    onAction?.(actionId); // 其余动作一律透出：空态引导等
  };
  // WB 体检刷新（旧 WB81-90）：失败只提示，不清已有报告（hook 内保留上次报告）。
  const runCheckup = async () => {
    const outcome = await wb.runCheckup();
    if (!outcome.ok) notify.error(LIBRARY_TEXTS.wbNotifyPrefix + fillLibraryText(LIBRARY_TEXTS.wbCheckupFailure, { reason: outcome.error }));
  };
  // 插件安装/卸载与备份对比的忙碌位（插件行按钮与「重新校验」据此转忙碌句并禁用）。
  const pluginBusy = plugins.busy;
  const infoBusy = verify.state.loading && view?.info?.backup != null ? [...busyIds, view.info.backup.refresh.id] : busyIds;
  // 项目行：选择模式 = 勾选其下全部会话并立即展开（旧 L1223-1230：让勾选结果可见）；浏览态 = 展开/收起。
  const toggleNode = (node: ProjectNodeView) => {
    if (selection.selectMode) {
      selection.toggleProject(node);
      if (node.collapseKey !== undefined) archive.expand(node.collapseKey); // 归档视图：勾选即展开（L1579-1585）
      else setExpandedIds((current) => (current.includes(node.id) ? current : [...current, node.id]));
      return;
    }
    // 归档视图折叠键随节点给（「模式\u0000项目」），折叠态由归档 hook 持；正常视图按项目 id 展开。
    if (node.collapseKey !== undefined) { archive.toggleCollapsed(node.collapseKey); return; }
    setExpandedIds((current) => (current.includes(node.id) ? current.filter((id) => id !== node.id) : [...current, node.id]));
  };
  // 点行（标题区）：选择模式 = 勾选（L1962 三态判定），浏览态 = 打开查看器。
  const activateSession = (row: SessionRowView) => {
    if (selection.selectMode) selection.toggleSession(row.id);
    else assembled?.openSession(row.id);
  };
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
  const sessions = (
    <LibrarySessionsBody view={view} heading={assembled.heading} treeTestId={assembled.treeTestId} selection={selection}
      searchText={searchText} onSearch={setSearchText} onFilterChange={setUidFilter} onIncludeDeletedChange={setIncludeDeleted}
      flowRef={flowRef} onTabChange={setViewerTab} onCloseViewer={assembled.closeSession} onLoadOlder={loadOlder}
      onAction={handleAction} onActivateNode={toggleNode} onActivateSession={activateSession}
      onToggleSession={(row) => selection.toggleSession(row.id)}
      onQuickArchive={(row) => { if (row.quickArchive !== null) archive.quickArchive(row.id); }}
      onRetryRecovery={archive.retry} archiveBusy={busyAll} />
  );
  const blocked = view.loadingNote !== null || view.error !== null || view.empty !== null;

  return (
    <>
      <LibraryShell view={view} className={assembled.shellClass} busy={busyIds} onAction={handleAction}
        onTabChange={setActiveTab}>
        {view.notices.map((notice, index) => <p className="checkin-page__meta" role="status" key={index}>{notice}</p>)}
        {view.loadingNote !== null && <EmptyState role="status" description={view.loadingNote} />}
        {view.error !== null && (
          <EmptyState panel role="alert" title={view.error.title} description={view.error.description}
            action={<button className="btn" type="button" onClick={() => void refresh()}>{view.error.retryLabel}</button>} />
        )}
        {view.empty !== null && (
          <EmptyState panel title={view.empty.title} description={view.empty.hint === "" ? undefined : view.empty.hint} />
        )}
        {/* tab 面板按 view.tabs 切换（WB 无 tab：正文直接落树 + 查看器 + 体检卡）；
            插件 tab 面板按视图形态给工作台或说明卡（只读库 / 副环境），库信息 tab 面板给信息卡 + 备份对比。 */}
        {!blocked && (view.tabs.length > 0 ? (
          <>
            <div className="master-detail__tabpanel" hidden={activeTab !== "sessions"}>{sessions}</div>
            <div className="master-detail__tabpanel" hidden={activeTab !== "plugins"}>
              {view.plugins !== null && (
                <LibraryPluginTab view={view.plugins} busy={pluginBusy} onSegmentChange={plugins.chooseSegment}
                  onSearch={plugins.search} onCategoryChange={plugins.chooseCategory} onAction={handleAction}
                  onRowAction={plugins.rowAction} onUninstallCancel={plugins.cancelUninstall} onUninstallConfirm={plugins.confirmUninstall} />
              )}
            </div>
            <div className="master-detail__tabpanel" hidden={activeTab !== "info"}>
              {view.info !== null && (
                <LibraryInfoTab view={view.info} checkup={view.checkup} busy={infoBusy}
                  onAction={handleAction} onBackupBase={verify.chooseBase} />
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
      {/* 归档草稿条（L1851-1882）：与批量栏同时存在时上下错开（lib-actionbar--draft）。 */}
      {!blocked && view.draftBar !== null && <LibraryDraftBar bar={view.draftBar} busy={archive.busy} onAction={handleAction} />}
      {/* Work 库应用前确认（L1884-1901）：CN 只读库视图不给该单，故只在视图给出时渲染。 */}
      {view.archive?.confirm != null && archive.confirmOpen && (
        <LibraryArchiveConfirm confirm={view.archive.confirm} busy={archive.busy} onCancel={archive.cancel} onConfirm={archive.confirm} />
      )}
      {/* 删除确认层：TRAE 层锚逐字沿用旧页 delete-confirm；WB 两层各取订单 testId（决策 7 不共用）。 */}
      <LibraryDeleteDialogs dialog={actions.dialog} busy={actions.busy}
        testId={actions.dialog?.kind === "trae-delete" ? LIBRARY_DELETE_CONFIRM_TEST_ID : undefined}
        onCancel={actions.cancel} onConfirm={() => actions.confirm()} />
      {/* 归并确认层：项目 / CN 跨账号 / WB cwd 三层，忙碌位只取同链那一位。 */}
      <LibraryMergeDialogs dialog={merge.dialog} busy={merge.busy} mergeStage={merge.mergeStage}
        onCancel={merge.cancel} onConfirm={(confirm: LibraryMergeConfirm) => merge.confirm(confirm)} />
    </>
  );
}
