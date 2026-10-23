/**
 * TRAE 库视图 builder（P4e-4a）：快照 → LibraryView 的纯映射（零 React / 零 JSX / 零 invoke / 零命令）。
 * 分组/搜索/归档树只读 sessionTree（P4e-3a）；消息与接力腿只读 relayLegs（P4e-3b）；能力只读 libraryCapabilitiesFor("trae_cn")。
 * testid 逐字抄旧实现：L=LibrarySessionsPanel、D=MasterLibraryDetail。时钟由快照注入 now：不读 Date.now、不发起读取、不排序。
 * K-08：轨迹行 thoughts 取自快照消息的 content.thoughts（经 TraeMessageView 携带；config 的 MessageView 未建模该字段，本文件不改 config）。
 */
import type { MasterHistoryDto, MasterSessionEntryDto, RelayLedgerEntryDto, TraeCnForeignProjectDto } from "../../types/history";
import type { MasterBackupChainDto, MasterLibraryStatsDto, MasterVerificationDto } from "../../types/masterLibrary";
import type { ProductOperationSnapshotDto } from "../../types/product";
import { LIBRARY_TEXTS, LIBRARY_VIEW_TEXTS, fillLibraryText, libraryCapabilitiesFor, type ArchiveChromeView, type ArchiveRecoveryView, type LibraryActionKind, type LibraryActionView, type LibraryInfoItemView, type LibraryInfoView, type LibraryStatView, type LibraryTabId, type LibraryView, type MessageView, type ProjectNodeView, type ViewerPaneView } from "./config";
import { archivedSessionsOf, buildArchiveSessionTree, buildSessionTree, countTreeSessions, liveSessionsOf, type SessionTreeDraft } from "./sessionTree";
import { buildViewerPane, formatRelativeTime, type ViewerPaneSource } from "./relayLegs";
/** 只读库实例 id（与 platform/productRegistry 的 TRAE_CN_LIBRARY_ID 同值；本文件不引 UI 侧依赖）；下行为写操作 id 字面量。 */
const CN_LIBRARY_ID = "trae_cn";
const ARCHIVE_OP = "library.archive", WORK_MERGE_OP = "library.work_project_merge", DELETE_OP = "library.delete", CN_MERGE_OP = "library.trae_cn_project_merge";
/** 查看器读取态（旧 ViewerState L133-140）；K-08：MessageView 无 thoughts，本文件以只读扩展承载旧 L2207-2213 轨迹展开列表。 */
export interface TraeViewerSource extends ViewerPaneSource { readonly session: MasterSessionEntryDto }
export interface TraeMessageView extends MessageView { readonly thoughts: readonly string[] }
export interface TraeViewerPaneView extends ViewerPaneView { readonly messages: readonly TraeMessageView[] } // 额外携带 K-08 thoughts
export interface TraeLibraryView extends LibraryView { readonly viewer: TraeViewerPaneView | null } // 仍是 LibraryView（可直接交给页面）
/** 聚合页面需要的所有已取回数据：builder 只消费，不发起任何读取。 */
export interface TraeLibrarySnapshot {
  readonly libraryId: string; // master / env-* / trae_cn（写能力与页头标题的既有派生键）
  readonly libraryTitle: string; readonly readOnly: boolean; // D57-59 库名 / isReadOnlyLibrary 只读库整页隐藏写控件
  readonly activeTab: LibraryTabId; // D66 三 tab 当前档
  readonly history: MasterHistoryDto | null; // get_master_history（status 四态）
  readonly ledger: readonly RelayLedgerEntryDto[]; // get_relay_ledger（接力腿数据源）
  readonly operations: ProductOperationSnapshotDto | null; // get_product_operation_states（null = 未读到）
  readonly stats: MasterLibraryStatsDto | null; // get_master_library_stats（库信息计数条）
  readonly verification: MasterVerificationDto | null; // get_master_verification（备份对比，P4e-8 消费）
  readonly backupChain: MasterBackupChainDto | null; // get_master_backup_chain（最近备份）
  readonly currentAccountName: string | null; readonly dataDir: string | null; // 环境档案 / 数据目录（只进悬浮提示）
  readonly foreignProjects: readonly TraeCnForeignProjectDto[]; // list_trae_cn_foreign_projects（其他账号组）
  readonly archiveDraft: SessionTreeDraft; // 归档草稿两集合（正本在后端）
  readonly searchText: string; // 搜索框原始值（sessionTree 内 trim）
  readonly expandedIds: readonly string[]; readonly selectedProjectIds: readonly string[]; readonly selectedSessionIds: readonly string[];
  readonly selectMode: boolean; readonly archiveView: boolean; // G22 选择模式 / 归档视图（模式 → 项目 → 会话）
  readonly archiveCollapsedKeys: readonly string[]; // 「模式\u0000项目」折叠键
  readonly viewer: TraeViewerSource | null; // null = 右栏空态引导
  readonly viewerTab: "messages" | "relay"; readonly busy: boolean; // 查看器 tab / 批量忙碌态（只用于算 disabled）
  readonly loadError: string | null; // 读取失败原因（页面兜底句已在页面侧映射）
  readonly archiveRecovery: ArchiveRecoveryView | null; readonly draftStorageError: string | null; // L1532 恢复提示条 / L1552 草稿存储告警
  readonly now: Date; // 纯函数不读时钟
}
/** 动作行（页头/批量栏/草稿条共用）：testId 逐字沿用旧锚点，hint 即旧 title。 */
const action = (id: string, label: string, kind: LibraryActionKind, testId: string, hint: string | null = null, disabledReason: string | null = null, busyLabel: string | null = null): LibraryActionView =>
  ({ id, label, kind, testId, hint, disabledReason, busyLabel });
/** 写能力派生（L76-82 原样）：只读库=归档/恢复；master/env-* 全开；其余纯只读；只读标志只能降级。 */
export function traeWriteCapabilityOf(libraryId: string, readOnly: boolean): "full" | "archive-only" | "none" {
  const derived = libraryId === "master" || libraryId.startsWith("env-") ? "full" : libraryId === CN_LIBRARY_ID ? "archive-only" : "none";
  return readOnly && derived === "full" ? "none" : derived;
}
/** 操作就绪表（L171-188）：库 id + 产品协议值双匹配才认快照；错库一律按未就绪（fail-closed）。 */
export function traeOperationStatesOf(libraryId: string, operations: ProductOperationSnapshotDto | null): { readonly matched: boolean; readonly states: ReadonlyMap<string, string>; readonly reasons: ReadonlyMap<string, string> } {
  const productId = libraryId === CN_LIBRARY_ID ? CN_LIBRARY_ID : "work_cn";
  const matched = operations !== null && operations.product_id === productId && operations.library_id === libraryId;
  const states = new Map<string, string>(), reasons = new Map<string, string>();
  for (const op of operations && matched ? operations.operations : []) { states.set(op.operation_id, op.state); reasons.set(op.operation_id, op.reason); }
  return { matched, states, reasons };
}
/** 写门禁四态（页面与 builder 共用，避免两处各算一遍）：归档 / 项目归并 / 删除 / CN 跨账号归并。 */
export interface TraeWriteGates { readonly canArchive: boolean; readonly canWorkMerge: boolean; readonly canDelete: boolean; readonly canCnMerge: boolean }
export function traeWriteGatesOf(libraryId: string, readOnly: boolean, operations: ProductOperationSnapshotDto | null): TraeWriteGates {
  const base = traeWriteCapabilityOf(libraryId, readOnly), full = base === "full", states = traeOperationStatesOf(libraryId, operations).states;
  return { canArchive: base !== "none" && states.get(ARCHIVE_OP) === "ready", // L185
    canWorkMerge: full && states.get(WORK_MERGE_OP) === "ready", // L186
    canDelete: full && states.get(DELETE_OP) === "ready", // L187
    canCnMerge: states.get(CN_MERGE_OP) === "ready" };
}
/** 写门禁提示（L195-219）：未匹配 = 产品未知句；有 pending 项 = 「A、B暂不可用。原因」。 */
function gateNoticeOf(ops: { readonly matched: boolean; readonly states: ReadonlyMap<string, string>; readonly reasons: ReadonlyMap<string, string> },
  entries: readonly { readonly id: string; readonly label: string }[], unknownText: string): string | null {
  if (!ops.matched) return unknownText;
  const pending = entries.filter((entry) => ops.states.get(entry.id) === "pending");
  return pending.length === 0 ? null : fillLibraryText(LIBRARY_TEXTS.gatePending,
    { operations: pending.map((entry) => entry.label).join("、"), reason: ops.reasons.get(pending[0].id) ?? "" });
}
const CN_PENDING = [{ id: ARCHIVE_OP, label: LIBRARY_TEXTS.gateArchiveLabel }, { id: CN_MERGE_OP, label: LIBRARY_TEXTS.gateMergeLabel }];
const WORK_PENDING = [{ id: ARCHIVE_OP, label: LIBRARY_TEXTS.gateArchiveLabel }, { id: WORK_MERGE_OP, label: LIBRARY_TEXTS.gateMergeLabel }, { id: DELETE_OP, label: LIBRARY_TEXTS.gateDeleteLabel }];
/** 页头（D138-171）：返回 + 只读徽章 + 刷新；region 与 data-page-title 逐字沿用旧契约。 */
function headerOf(snapshot: TraeLibrarySnapshot): LibraryView["header"] {
  const caps = libraryCapabilitiesFor("trae_cn"), isMaster = snapshot.libraryId === "master";
  return {
    title: snapshot.libraryTitle,
    regionLabel: isMaster ? LIBRARY_TEXTS.detailRegionMaster : fillLibraryText(LIBRARY_TEXTS.detailRegionSuffix, { name: snapshot.libraryTitle }),
    pageTitleAttr: caps.pageRegion, // D155：data-page-title="master-library"
    readonlyBadge: snapshot.readOnly ? LIBRARY_TEXTS.readonlyBadge : null, // D157
    badgeProductId: null, // 旧详情页无产品徽章（ProductBadge 归 WB 页）
    back: action("library-back", LIBRARY_TEXTS.detailBack, "ghost", "master-detail-back"), // D151
    refresh: action("library-refresh", LIBRARY_TEXTS.detailRefresh, "ghost", "master-detail-refresh",
      fillLibraryText(LIBRARY_TEXTS.detailRefreshHint, { name: snapshot.libraryTitle }), // D166 旧 title
      snapshot.busy ? LIBRARY_TEXTS.busyShort : null),
  };
}
/** 三 tab（D176-207）：锚点与文案逐字；能力表声明 TRAE 三 tab 全在（pluginTab / libraryInfoTab 均非 absent）。 */
const TABS = [{ id: "sessions", label: LIBRARY_TEXTS.detailTabSessions, testId: "master-tab-sessions" },
  { id: "plugins", label: LIBRARY_TEXTS.detailTabPlugins, testId: "master-tab-plugins" },
  { id: "info", label: LIBRARY_TEXTS.detailTabInfo, testId: "master-tab-info" }] as const;
const tabsOf = (active: LibraryTabId): LibraryView["tabs"] => TABS.map((tab) => ({ ...tab, active: tab.id === active }));
/** 左栏树（sessionTree 只读调用）：正常视图 = 具名项目 + 未关联末位 + 其他账号组；归档视图 = 模式层。 */
function treeOf(history: MasterHistoryDto, snapshot: TraeLibrarySnapshot, canArchive: boolean, canCnMerge: boolean): readonly ProjectNodeView[] {
  const relativeTime = (unixSeconds: number | null) => formatRelativeTime(unixSeconds, snapshot.now);
  const base = { projects: history.projects, sessions: history.sessions, archiveDraft: snapshot.archiveDraft,
    selectedProjectIds: snapshot.selectedProjectIds, selectedSessionIds: snapshot.selectedSessionIds,
    selectMode: snapshot.selectMode, relativeTime };
  if (snapshot.archiveView) return buildArchiveSessionTree({ ...base, archiveCollapsedKeys: snapshot.archiveCollapsedKeys });
  return buildSessionTree({ ...base, searchText: snapshot.searchText, expandedIds: snapshot.expandedIds, canArchive,
    foreignProjects: snapshot.foreignProjects, foreignMergeReady: canCnMerge });
}
/** 右栏查看器（relayLegs.buildViewerPane）：消息补旧锚点 library-message-<id>（L2180/L2190）与 K-08 thoughts。 */
function viewerOf(snapshot: TraeLibrarySnapshot, history: MasterHistoryDto): TraeViewerPaneView | null {
  const source = snapshot.viewer;
  if (source === null) return null;
  const pane = buildViewerPane({ session: source.session, ledger: snapshot.ledger, currentUserId: history.current_user_id,
    now: snapshot.now, source, activeTab: snapshot.viewerTab });
  const thoughts = new Map<string, readonly string[]>();
  for (const message of source.messages) thoughts.set(message.message_id, message.content.thoughts);
  return { ...pane, messages: pane.messages.map((message) => ({ ...message, testId: `library-message-${message.id}`, thoughts: thoughts.get(message.id) ?? [] })) };
}
/** 批量栏（L1770-1848）：选择模式下浮出；归档视图给恢复/删除，正常视图给归档/合并/删除（CN 无删除/合并）。 */
function batchBarOf(snapshot: TraeLibrarySnapshot, canArchive: boolean, canWorkMerge: boolean, canDelete: boolean, selected: number, mergeTargets: number): LibraryView["batchBar"] {
  if (!snapshot.selectMode) return null;
  const busy = snapshot.busy, blocked = busy ? LIBRARY_TEXTS.busyShort : selected === 0 ? "未选择会话" : null;
  const mergeBlocked = busy ? LIBRARY_TEXTS.busyShort : mergeTargets < 2 ? LIBRARY_TEXTS.mergeDisabled : null; // L1826-1828
  const list: (LibraryActionView | null)[] = snapshot.archiveView // L1780-1805：归档视图只给恢复/删除
    ? [canArchive ? action("batch-restore", LIBRARY_TEXTS.restore, "primary", "batch-restore", null, blocked) : null, // L1788
      canDelete ? action("batch-delete", LIBRARY_TEXTS.deleteAction, "danger", "batch-delete", null, blocked) : null] // L1800
    : [canArchive ? action("batch-archive", LIBRARY_TEXTS.archiveAction, "primary", "batch-archive", null, blocked) : null, // L1814
      canWorkMerge ? action("batch-merge", LIBRARY_TEXTS.mergeAction, "ghost", "batch-merge", null, mergeBlocked) : null, // L1829
      canDelete ? action("batch-delete", LIBRARY_TEXTS.deleteAction, "danger", "batch-delete", null, blocked) : null]; // L1840
  return { testId: "library-action-bar", ariaLabel: LIBRARY_TEXTS.regionBatch,
    summary: fillLibraryText(LIBRARY_TEXTS.selectionSummary, { sessions: snapshot.selectedSessionIds.length, projects: snapshot.selectedProjectIds.length }),
    actions: list.filter((item): item is LibraryActionView => item !== null) };
}
/** 归档草稿条（L1851-1882）：草稿有变更才浮出；放弃/应用两动作，应用受写门禁与待核对恢复态双重约束。 */
function draftBarOf(snapshot: TraeLibrarySnapshot, canArchive: boolean): LibraryView["draftBar"] {
  const draft = snapshot.archiveDraft, busy = snapshot.busy, recovery = snapshot.archiveRecovery?.kind ?? null;
  if (draft.archiveSessionIds.length === 0 && draft.restoreSessionIds.length === 0) return null;
  // 提交结果待核对两态不得再次应用（L1875 / L926：先刷新核对再决定）。
  const blocked = recovery === "commit-unknown" || recovery === "committed-attention";
  const applyBlocked = busy ? LIBRARY_TEXTS.busyShort : !canArchive ? LIBRARY_TEXTS.gateUnknown : blocked ? LIBRARY_TEXTS.draftApplyBlocked : null;
  return { testId: "archive-draft-bar", ariaLabel: LIBRARY_TEXTS.regionDraft,
    summary: fillLibraryText(LIBRARY_TEXTS.draftSummary, { archive: draft.archiveSessionIds.length, restore: draft.restoreSessionIds.length }),
    discard: action("draft-discard", LIBRARY_TEXTS.draftDiscard, "ghost", "archive-draft-discard", null, busy ? LIBRARY_TEXTS.busyShort : null), // L1867
    apply: action("draft-apply", LIBRARY_TEXTS.draftApply, "primary", "archive-draft-apply", null, applyBlocked, LIBRARY_TEXTS.draftApplying) }; // L1876-1879
}
/** 归档动作区（L1506-1557 / L1668-1683 / L1884-1901）：入口、返回、恢复条、草稿告警、Work 应用前确认。
 * 只读库（Trae CN）无「关闭客户端」确认（旧 L760 守卫）；入口与返回互斥（归档视图不显示入口）。 */
function archiveChromeOf(snapshot: TraeLibrarySnapshot, canArchive: boolean, archivedCount: number): ArchiveChromeView {
  const recovery = snapshot.archiveRecovery;
  return {
    view: snapshot.archiveView, canArchive,
    entry: snapshot.archiveView || archivedCount === 0 ? null : { count: archivedCount,
      action: action("archive-entry", LIBRARY_TEXTS.archivedEntry, "ghost", "library-archive-entry") }, // L1672-1683
    back: snapshot.archiveView ? action("archive-back", LIBRARY_TEXTS.backToTree, "ghost", "library-archive-back") : null, // L1507-1515
    recovery,
    retryLabel: recovery === null ? null : recovery.kind === "relaunch" ? LIBRARY_TEXTS.retryLaunch : LIBRARY_TEXTS.refreshHistory, // L1541-1545
    draftError: snapshot.draftStorageError, // L1549-1557
    confirm: snapshot.readOnly ? null : { title: LIBRARY_TEXTS.workArchiveTitle, confirmLabel: LIBRARY_TEXTS.workArchiveConfirm,
      lines: [LIBRARY_TEXTS.workArchiveLine1, LIBRARY_TEXTS.workArchiveLine2, LIBRARY_TEXTS.workArchiveLine3], testId: "work-archive-confirm" }, // L1884-1901
  };
}
/** 库路径展示（D321-324）：只取末两段，完整路径在悬浮提示（界面表达纪律）。 */
const dataDirTail = (path: string): string => { const segments = path.split(/[\\/]/).filter(Boolean); return segments.length > 1 ? segments.slice(-2).join("\\") : path; };
/** 字节数人性化（D327-337 原样）：KB/MB/GB/TB，≥100 取整。 */
const formatBytes = (value: number): string => {
  const units = ["KB", "MB", "GB", "TB"];
  let size = value, unit = -1;
  if (value < 1024) return `${value} B`;
  do { size /= 1024; unit += 1; } while (size >= 1024 && unit < units.length - 1);
  return `${size >= 100 ? Math.round(size) : size.toFixed(1)} ${units[unit]}`;
};
/** 绝对时间（D340-347）：最近活跃/最近备份要能定位到具体时刻（与列表相对时间不同）。 */
function formatDateTime(unixSeconds: number): string {
  const date = new Date(unixSeconds * 1000), now = new Date(), pad = (value: number) => String(value).padStart(2, "0");
  const hm = `${pad(date.getHours())}:${pad(date.getMinutes())}`, monthDay = `${date.getMonth() + 1}月${date.getDate()}日`;
  return date.getFullYear() === now.getFullYear() ? `${monthDay} ${hm}` : `${date.getFullYear()}年${monthDay} ${hm}`;
}
/** 库信息 tab（D248-306）：五项 + 计数条；不可读一律给旧页「—」占位（null 会变成不渲染，与旧页不一致）。 */
function infoOf(snapshot: TraeLibrarySnapshot): LibraryInfoView {
  const isMaster = snapshot.libraryId === "master", stats = snapshot.stats, ready = stats !== null && stats.status === "ready";
  const latest = snapshot.backupChain?.backups[0] ?? null, dataDir = snapshot.dataDir?.trim() || null, dash = LIBRARY_TEXTS.infoDash;
  const item = (id: string, testId: string, label: string, value: string | null, titleAttr: string | null = null): LibraryInfoItemView => ({ id, testId, label, value, titleAttr });
  const count = (id: string, label: string, value: number, hint: string): LibraryStatView => ({ id, label, value: String(value), hint });
  return {
    testId: "master-detail-info",
    items: [
      // 只读数据根：工具不管理其登录态，中性占位不冒充「未登录」（D252-254）。
      item("account", "master-info-account", LIBRARY_TEXTS.infoCurrentAccount, snapshot.readOnly ? dash : snapshot.currentAccountName ?? LIBRARY_TEXTS.infoNotLoggedIn),
      item("size", "master-info-size", isMaster ? LIBRARY_TEXTS.infoSizeMaster : LIBRARY_TEXTS.infoSizeSecondary, stats !== null && stats.size_bytes > 0 ? formatBytes(stats.size_bytes) : dash), // D259-260
      item("active", "master-info-active", LIBRARY_TEXTS.infoLastActive, ready && stats.last_active_unix_seconds !== null ? formatDateTime(stats.last_active_unix_seconds) : dash), // D265-268
      item("backup", "master-info-backup", LIBRARY_TEXTS.infoLatestBackup, latest !== null ? formatDateTime(latest.stamp_unix_seconds) : LIBRARY_TEXTS.infoNoBackup), // D273-274
      item("path", "master-info-path", isMaster ? LIBRARY_TEXTS.infoPathMaster : LIBRARY_TEXTS.infoPathSecondary, dataDir !== null ? dataDirTail(dataDir) : dash, dataDir), // D279-287
    ],
    // 计数条（D294-305）：三项 + 旧页悬浮说明；统计未就绪即不渲染（旧页 statsReady 守卫）。
    counts: ready ? [count("projects", LIBRARY_TEXTS.infoProjectCount, stats.project_count, "当前账号可见的项目数"),
      count("sessions", LIBRARY_TEXTS.infoSessionCount, stats.session_count, "当前账号可见的会话数"),
      count("messages", LIBRARY_TEXTS.infoMessageCount, stats.message_count, "当前账号会话的消息总数")] : [],
    backup: null, // 备份对比区块（V180-264）归 P4e-8 的 TraeInfoTab；快照已留 verification / backupChain，本票不臆造
  };
}
/** 快照 → 库视图（纯函数：同一快照恒得同一视图）。 */
export function buildTraeLibraryView(snapshot: TraeLibrarySnapshot): TraeLibraryView {
  const caps = libraryCapabilitiesFor("trae_cn"), history = snapshot.history !== null && snapshot.history.status === "ready" ? snapshot.history : null;
  const ops = traeOperationStatesOf(snapshot.libraryId, snapshot.operations), full = traeWriteCapabilityOf(snapshot.libraryId, snapshot.readOnly) === "full";
  const { canArchive, canWorkMerge, canDelete, canCnMerge } = traeWriteGatesOf(snapshot.libraryId, snapshot.readOnly, snapshot.operations); // L185-188
  // 选择入口可见性（L230-232）：归档视图看 归档/删除，正常视图看 归档/合并/删除；全不可写即无入口。
  const canSelect = snapshot.archiveView ? canArchive || canDelete : canArchive || canWorkMerge || canDelete; // L188
  const draft = snapshot.archiveDraft, keyword = snapshot.searchText.trim(), tree = history !== null ? treeOf(history, snapshot, canArchive, canCnMerge) : [];
  // 选择摘要口径（L838-848）：当前视图的可见会话（含已归档、不含已删除），与搜索无关。
  const archivedAll = history === null ? [] : archivedSessionsOf(history.sessions, draft); // 入口计数（L1672：不分视图）
  const visible = history === null ? [] : snapshot.archiveView ? archivedAll : liveSessionsOf(history.sessions, draft);
  const selected = visible.filter((session) => snapshot.selectedSessionIds.includes(session.session_id) || snapshot.selectedProjectIds.includes(session.project_id)).length;
  const mergeTargets = snapshot.selectedProjectIds.filter((id) => history !== null && history.projects.some((project) => project.project_id === id && project.name.trim() !== "")).length;
  const lib = snapshot.libraryTitle, failure = fillLibraryText(LIBRARY_TEXTS.loadFailure, { lib }), status = snapshot.history?.status ?? null;
  const error = snapshot.loadError !== null || status === "read_failed" ? { title: failure, description: snapshot.loadError ?? "", retryLabel: LIBRARY_TEXTS.retry } : null; // L1396-1413
  // 空态（L1425-1481）：无库数据 / 未登记账号两档；非 full 能力库不出环境页引导与动作（P13-3）。
  const empty = error !== null || (status !== "no_master_data" && status !== "no_current_account") ? null
    : status === "no_master_data"
      ? { title: fillLibraryText(LIBRARY_TEXTS.emptyNoDataTitle, { lib }), hint: full ? fillLibraryText(LIBRARY_TEXTS.emptyNoDataHint, { lib }) : "", actionLabel: full ? LIBRARY_TEXTS.emptyNoDataAction : null }
      : { title: fillLibraryText(LIBRARY_TEXTS.emptyNoAccountTitle, { lib }), hint: full ? fillLibraryText(LIBRARY_TEXTS.emptyNoAccountHint, { lib }) : "", actionLabel: full ? LIBRARY_TEXTS.emptyNoAccountAction : null };
  // 空态文案（L1653-1663）：归档视图看归档集合，搜索态看命中数，浏览态看正常会话数。
  const treeEmpty = history === null ? LIBRARY_TEXTS.emptyNoSessions
    : snapshot.archiveView ? (visible.length === 0 ? LIBRARY_TEXTS.emptyArchived : "")
    : keyword !== "" ? (countTreeSessions(tree) === 0 ? LIBRARY_TEXTS.emptyNoMatch : "")
    : (visible.length === 0 ? LIBRARY_TEXTS.emptyNoSessions : "");
  // 页内提示（L1750-1764）：只读库给 CN 待就绪句；其余库给归档原因与 full 库的待就绪句。
  const notices: string[] = [];
  if (snapshot.readOnly) { const pending = gateNoticeOf(ops, CN_PENDING, LIBRARY_TEXTS.gateUnknownCn); if (pending !== null) notices.push(`${pending} 当前仍可浏览对话。`); }
  else {
    const reason = ops.states.get(ARCHIVE_OP) === "ready" ? ops.reasons.get(ARCHIVE_OP) ?? "" : ""; // L189-191
    if (reason !== "") notices.push(reason);
    if (full) { const pending = gateNoticeOf(ops, WORK_PENDING, LIBRARY_TEXTS.gateUnknown); if (pending !== null) notices.push(pending); } // L1760-1764
  }
  return {
    productId: "trae_cn", libraryId: snapshot.libraryId, header: headerOf(snapshot), tabs: tabsOf(snapshot.activeTab),
    loadingNote: snapshot.history === null && snapshot.loadError === null ? LIBRARY_TEXTS.loading : null, // L1415-1423
    error, empty,
    stats: null, filter: null, includeDeleted: null, // 计数条在 info.counts（旧 D294-305）；§2-9 A 无账号过滤；§2-6 无软删开关
    search: history !== null && caps.projectSearch && !snapshot.archiveView
      ? { testId: "library-search-input", placeholder: LIBRARY_TEXTS.searchPlaceholder, ariaLabel: LIBRARY_TEXTS.searchAria, value: snapshot.searchText } : null, // L1520-1527
    tree, treeEmpty, viewer: history !== null ? viewerOf(snapshot, history) : null,
    batchBar: batchBarOf(snapshot, canArchive, canWorkMerge, canDelete, selected, mergeTargets), draftBar: draftBarOf(snapshot, canArchive),
    archive: archiveChromeOf(snapshot, canArchive, archivedAll.length),
    selectionEntry: canSelect ? { label: LIBRARY_TEXTS.select, cancelLabel: LIBRARY_TEXTS.cancel } : null, // L1684-1701（锚由展示层持有）
    checkup: null, plugins: null, // §2-11 A 库页无体检（G18 在环境页）；§2-12 插件 tab 由 P4e-8 的 usePluginWorkbench 产出
    info: infoOf(snapshot), notices, texts: LIBRARY_VIEW_TEXTS.trae_cn,
  };
}