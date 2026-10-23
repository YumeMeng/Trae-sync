/**
 * 库页装配（P4e-8a：只做读取与渲染；动作与弹层留 P4e-8b）。
 * 数据面 useTraeLibrary / useWbLibrary 经本文件 ASSEMBLERS 查表（键 = productId，页面零 productId 相等判断、
 * 零 invoke、零命令名）；视图模型 registry.libraryViewFor；展示组件 LibraryShell / LibraryTree / SessionViewer / LibraryInfoTab。
 * 本文件不发明文案与 testid：标题、tab、锚点、空态句一律取自 LibraryView；动作只经 onAction 原样透出。
 */
import { useLayoutEffect, useRef, useState } from "react";
import { Search } from "lucide-react";
import { EmptyState } from "../../components/shared/EmptyState";
import type { WorkBuddyCwdGroupDto } from "../../types/workbuddyLibrary";
import {
  LIBRARY_TEXTS, fillLibraryText, type CheckupView, type LibraryTabId, type LibraryView, type ProjectNodeView, type SessionRowView,
} from "./config";
import { libraryViewFor, type LibrarySnapshot } from "./registry";
import { useTraeLibrary, type TraeLibraryPageFields, type TraeLibraryReadResult } from "./components/useTraeLibrary";
import { useWbLibrary, type WbLibraryPageFields, type WbLibraryReadResult } from "./components/useWbLibrary";
import { LibraryShell } from "./components/LibraryShell";
import { LibraryTree } from "./components/LibraryTree";
import { SessionViewer, type SessionViewerHeading } from "./components/SessionViewer";
import { LibraryInfoTab } from "./components/LibraryInfoTab";
import { formatSessionTime } from "./relayLegs";
import { projectNameOf, type SessionTreeDraft } from "./sessionTree";

/** 删除确认层锚点契约（旧页 L2285）：8b 渲染 LibraryDeleteDialogs 时经 testId 传此值，逐字沿用旧锚。 */
export const LIBRARY_DELETE_CONFIRM_TEST_ID = "delete-confirm";
/** 8a 不持归档草稿与勾选（草稿、选择属 8b 动作）：恒空集合，树按「无草稿、非选择模式」口径派生。 */
const EMPTY_DRAFT: SessionTreeDraft = { archiveSessionIds: [], restoreSessionIds: [] };
/** 无会话时的标题区占位（SessionViewer 在 view=null 时整块不渲染标题区，本值只满足签名）。 */
const EMPTY_HEADING: SessionViewerHeading = { title: "", projectName: "", messageCount: 0, lastActiveLabel: "" };
/** 本票不接线的出口（8b 填）：勾选与对比基准切换留空，不发明假回执。 */
const NO_ACTION = () => undefined;
const NO_BASE = (_stamp: number | null) => undefined;

/** WB 库体检卡（旧 WB421-481）：五项 + 归并候选入口 + 立即体检。WB 无库信息 tab，故本卡落正文。 */
function WbCheckupCard({ view, onAction }: { readonly view: CheckupView; readonly onAction: (actionId: string) => void }) {
  return (
    <div className="workbuddy-card" data-testid={view.testId}>
      <div className="workbuddy-card__title">{view.title}</div>
      {view.items.length > 0 ? (
        <ul className="account-center__checkin-results">
          {view.items.map((item) => (
            <li key={item.id} title={item.hint ?? undefined}><strong>{item.label}</strong><span>{item.value}</span></li>
          ))}
        </ul>
      ) : <p className="account-item__meta">{view.idleNote}</p>}
      <div className="workbuddy-card__actions">
        {view.candidates.map((group) => (
          <button key={group.canonical} className="btn" type="button" onClick={() => onAction(`merge-${group.canonical}`)}
            data-testid={`workbuddy-library-merge-${group.variants.length + 1}`}>
            {fillLibraryText(LIBRARY_TEXTS.wbMergeEntry, { cwd: group.canonical })}
          </button>
        ))}
        <button className="btn" type="button" onClick={() => onAction(view.action.id)}
          disabled={view.action.disabledReason !== null} data-testid={view.action.testId}>
          {view.action.label}
        </button>
      </div>
    </div>
  );
}

/** 装配结果：合成快照 + 查看器标题区 + 读侧出口 + 产品壳差异（容器类名 / 树容器锚）。 */
interface Assembled {
  readonly snapshot: LibrarySnapshot;
  readonly heading: SessionViewerHeading | null;
  readonly openSession: (sessionId: string) => void;
  readonly closeSession: () => void;
  readonly loadOlder: () => void;
  readonly refresh: () => Promise<{ readonly ok: boolean }>;
  readonly shellClass?: string;
  readonly treeTestId?: string;
}
/** 装配入参：两条读取 hook 的结果（未匹配产品的 hook 自门禁，恒空且零 IPC）+ 注入时钟。 */
interface AssembleInput { readonly trae: TraeLibraryReadResult; readonly wb: WbLibraryReadResult; readonly now: Date }

/**
 * 装配表（键 = productId，与 environment 的 ASSEMBLERS 同法）：hook 数据 + 页面读侧字段 → builder 快照。
 * ledger / foreignProjects / accounts 在 hook 里放宽为 null（未读到），builder 快照要求非空数组：此处按空集合喂入。
 */
const ASSEMBLERS: Record<string, (input: AssembleInput) => Assembled> = {
  trae_cn: ({ trae, now }) => {
    const data = trae.data, history = data.history !== null && data.history.status === "ready" ? data.history : null;
    const session = data.viewer?.session ?? null;
    return {
      snapshot: { productId: "trae_cn", trae: { ...data, ledger: data.ledger ?? [], foreignProjects: data.foreignProjects ?? [],
        archiveDraft: EMPTY_DRAFT, selectedProjectIds: [], selectedSessionIds: [], selectMode: false, archiveView: false,
        archiveCollapsedKeys: [], archiveRecovery: null, draftStorageError: null } },
      heading: session === null ? null : {
        title: session.title?.trim() || LIBRARY_TEXTS.unnamedSession,
        projectName: projectNameOf(history?.projects ?? [], session.project_id),
        messageCount: session.message_count,
        lastActiveLabel: formatSessionTime(session.updated_at_unix_seconds, now),
      },
      openSession: (sessionId) => {
        const target = history?.sessions.find((row) => row.session_id === sessionId);
        if (target !== undefined) void trae.openSession(target);
      },
      closeSession: trae.closeSession,
      loadOlder: () => void trae.loadOlder(),
      refresh: () => trae.refresh({ force: true }),
    };
  },
  workbuddy: ({ wb, now }) => {
    const data = wb.data, groups: readonly WorkBuddyCwdGroupDto[] = data.groups ?? [], openId = data.openSessionId;
    const group = openId === null ? undefined : groups.find((item) => item.sessions.some((row) => row.session_id === openId));
    const row = group === undefined || openId === null ? undefined : group.sessions.find((item) => item.session_id === openId);
    return {
      snapshot: { productId: "workbuddy", workbuddy: { ...data, accounts: data.accounts ?? [], selectedIds: [],
        writeBusy: false, mergeBusy: false } },
      heading: row === undefined || group === undefined ? null : {
        title: row.title?.trim() || LIBRARY_TEXTS.unnamedSession,
        projectName: group.cwd,
        messageCount: data.messages?.messages.length ?? 0,
        lastActiveLabel: formatSessionTime(row.last_active_unix_seconds, now),
      },
      openSession: (sessionId) => { void wb.openSession(sessionId); },
      closeSession: wb.closeSession,
      loadOlder: () => undefined,
      refresh: () => wb.refresh(),
      shellClass: "workbuddy-page",
      treeTestId: "workbuddy-library-groups",
    };
  },
};

export interface LibraryPageProps {
  /** 页面可见才读取（后台页保持挂载但不发 IPC）。 */
  readonly active: boolean;
  /** 当前产品协议值：只用于查装配表，页面内不做任何产品判断。 */
  readonly productId: string;
  /** 库实例 id（缺省主库；副环境为 env_id，只读数据根为 trae_cn）。 */
  readonly libraryId?: string;
  /** 副环境库显示名（缺省由 builder 回退「副库」）。 */
  readonly libraryDisplayName?: string;
  /** 返回环境页（页头「返回」）；缺省时该动作原样透出到 onAction。 */
  readonly onBack?: () => void;
  /** 库内跳转到另一库详情：本票只登记签名（8b 接线），不调用。 */
  readonly onOpenLibraryDetail?: (libraryId: string) => void;
  /** 其余动作（批量栏 / 草稿条 / 插件 / 归并 / 删除 / 空态引导）：原样透出，本票不实现。 */
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
  const [pending, setPending] = useState(false); // 刷新在跑位（转圈 + 防连点）；读侧六项见下，动作位恒空（归 8b）
  const page: TraeLibraryPageFields & WbLibraryPageFields = {
    activeTab, searchText, expandedIds, viewerTab, uidFilter, includeDeleted,
    archiveDraft: EMPTY_DRAFT, selectedProjectIds: [], selectedSessionIds: [], selectMode: false,
    archiveView: false, archiveCollapsedKeys: [], busy: false, archiveRecovery: null, draftStorageError: null,
    selectedIds: [], writeBusy: false, mergeBusy: false,
  };
  const trae = useTraeLibrary(page, { productId, libraryId, libraryDisplayName, active, now });
  const wb = useWbLibrary(page, { productId, active, now });
  const assemble = ASSEMBLERS[productId];
  const assembled = assemble === undefined ? null : assemble({ trae, wb, now });
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

  // 未登记产品 fail-closed：不猜数据面，按 registry 空视图给一句失败说明（不套用任一产品的行语义）。
  if (assembled === null || view === null) {
    const fallback = libraryViewFor({ productId } as unknown as LibrarySnapshot);
    return (
      <LibraryShell view={fallback} onAction={NO_ACTION} onTabChange={NO_ACTION}>
        <EmptyState panel role="alert" description={fallback.empty?.title} />
      </LibraryShell>
    );
  }

  // 刷新是读取不是动作：页面就地执行（旧详情页 load()），失败不清已取回数据、不翻错误卡。
  const refresh = async () => {
    if (pending) return;
    setPending(true);
    try { await assembled.refresh(); } finally { setPending(false); }
  };
  const busyIds = pending && view.header.refresh !== null ? [view.header.refresh.id] : [];
  const handleAction = (actionId: string) => {
    if (actionId === view.header.refresh?.id) { void refresh(); return; }
    if (actionId === view.header.back?.id && onBack !== undefined) { onBack(); return; }
    onAction?.(actionId); // 其余动作一律透出：批量栏 / 草稿条 / 插件 / 归并 / 删除归 8b
  };
  const toggleNode = (node: ProjectNodeView) => setExpandedIds((current) =>
    (current.includes(node.id) ? current.filter((id) => id !== node.id) : [...current, node.id]));
  const quickArchive = (row: SessionRowView) => { if (row.quickArchive !== null) onAction?.(row.quickArchive.id); };
  const loadOlder = () => {
    const flow = flowRef.current;
    if (flow !== null) keepScroll.current = { top: flow.scrollTop, height: flow.scrollHeight };
    assembled.loadOlder();
  };

  // 统计卡（WB 五项；TRAE 的 stats 为 null 即整卡不渲染——计数条在库信息 tab）。
  const stats = view.stats === null ? null : (
    <div className="workbuddy-card" data-testid={view.stats.testId}>
      <div className="workbuddy-card__title">{view.stats.title}</div>
      <div className="overview-page__stats">
        {view.stats.items.map((item) => (
          <div className="overview-stat" key={item.id} title={item.hint ?? undefined}>
            <span className="overview-stat__value">{item.value}</span><span className="overview-stat__label">{item.label}</span>
          </div>
        ))}
      </div>
    </div>
  );
  // 账号过滤（WB 删除归属锚）+ 软删开关：视图为 null 即不渲染。
  const filter = view.filter === null ? null : (
    <label className="account-center__target-field">
      <span>{view.filter.label}</span>
      <select value={view.filter.selected ?? ""} onChange={(event) => setUidFilter(event.target.value)} data-testid={view.filter.testId}>
        <option value="">{view.filter.allLabel}</option>
        {view.filter.options.map((option) => <option key={option.id} value={option.id}>{option.label}</option>)}
      </select>
    </label>
  );
  const deleted = view.includeDeleted === null ? null : (
    <label className="settings-panel__toggle-row">
      <input type="checkbox" checked={view.includeDeleted.checked} onChange={(event) => setIncludeDeleted(event.target.checked)}
        data-testid={view.includeDeleted.testId} />
      <span className="account-center__profile-copy"><strong>{view.includeDeleted.label}</strong><span>{view.includeDeleted.hint}</span></span>
    </label>
  );
  // 会话面板：左栏搜索 + 树 / 右栏查看器（搜索框按视图有无渲染，WB 无标题搜索）。
  const sessions = (
    <>
      {filter === null && deleted === null ? null : <div className="account-center__list-controls">{filter}{deleted}</div>}
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
            onActivateSession={(row: SessionRowView) => assembled.openSession(row.id)}
            onToggleSession={NO_ACTION} onQuickArchive={quickArchive} onAction={handleAction} />
        </aside>
        <SessionViewer view={view.viewer} heading={assembled.heading ?? EMPTY_HEADING} flowRef={flowRef}
          onTabChange={setViewerTab} onClose={assembled.closeSession} onLoadOlder={loadOlder} />
      </div>
    </>
  );
  const blocked = view.loadingNote !== null || view.error !== null || view.empty !== null;

  return (
    <LibraryShell view={view} className={assembled.shellClass} busy={busyIds} onAction={handleAction}
      onTabChange={setActiveTab}>
      {view.notices.map((notice, index) => <p className="checkin-page__meta" role="status" key={index}>{notice}</p>)}
      {stats}
      {view.loadingNote !== null && <EmptyState role="status" description={view.loadingNote} />}
      {view.error !== null && (
        <EmptyState panel role="alert" title={view.error.title} description={view.error.description}
          action={<button className="btn" type="button" onClick={() => void refresh()}>{view.error.retryLabel}</button>} />
      )}
      {view.empty !== null && (
        <EmptyState panel title={view.empty.title} description={view.empty.hint === "" ? undefined : view.empty.hint} />
      )}
      {/* tab 面板按 view.tabs 切换（WB 无 tab：正文直接落树 + 查看器 + 体检卡）。插件 tab 面板留 P4e-8b。 */}
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
  );
}
