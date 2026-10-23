/**
 * 会话树纯派生（P4e-3a）：旧 LibrarySessionsPanel.tsx（下称 L）的 live/archived 过滤、标题搜索、
 * 项目分组、未关联末位合并组、归档模式层、其他账号项目组 → ProjectNodeView[]。
 * 零 React / 零 JSX / 零 invoke / 零 Date.now：草稿、查询串、展开与勾选集合、时间文案一律入参
 * （时间格式化器由调用方注入 L2489 formatRelativeTime 口径；正式落位归 relayLegs，本文件不读时钟）。
 * 旧实现全文件无 sort 调用（grep 0 命中）：次序 = projects 数组次序 → 未关联组末位 → 其他账号组末位，
 * 组内会话 = history.sessions 次序；本文件不排序，逐行沿用该稳定次序。
 * 旧公式一律照搬，本文件不自算滞留 / 空项目 / 孤儿（会话树本就不涉及这三项）。
 */
import type { MasterProjectEntryDto, MasterSessionEntryDto, TraeCnForeignProjectDto } from "../../types/history";
import { LIBRARY_TEXTS, fillLibraryText, type LibraryActionView, type ProjectNodeKind, type ProjectNodeView, type SessionRowView } from "./config";

/** 「未关联文件夹」合并组哨兵 id（L143，G19：空名项目归并一组，会话平铺）。 */
export const UNLINKED_GROUP_ID = "__unlinked__";
/** 「其他账号的项目」分组容器 id（L1362 的 key="__foreign__"）。 */
export const FOREIGN_GROUP_ID = "__foreign__";

/** 归档草稿的两集合（与 hooks/useArchiveDraft 的 ArchiveDraft 同形；本文件只读不持有状态）。 */
export interface SessionTreeDraft { readonly archiveSessionIds: readonly string[]; readonly restoreSessionIds: readonly string[] }
/** 正常视图树入参：项目/会话为 history 原样，其余为页面状态（查询、展开、勾选、写能力）。 */
export interface SessionTreeInput {
  readonly projects: readonly MasterProjectEntryDto[]; readonly sessions: readonly MasterSessionEntryDto[];
  readonly archiveDraft: SessionTreeDraft;
  /** 搜索框原始值（本文件内 trim，等价 L509 的 keyword 派生）。 */
  readonly searchText: string;
  readonly expandedIds: readonly string[]; readonly selectedProjectIds: readonly string[]; readonly selectedSessionIds: readonly string[];
  /** 选择模式（L1221/L1293：项目行勾选只在选择模式出现）。 */
  readonly selectMode: boolean;
  /** 浏览态快捷归档可用（L1278：showQuickArchive = !selectMode && canArchive）。 */
  readonly canArchive: boolean;
  /** 会话行时间文案（L1983）；注入以保证纯函数不读时钟。 */
  readonly relativeTime: (unixSeconds: number | null) => string;
  /** 其他账号项目行（L1361-1393；不传即不渲染该组）。 */
  readonly foreignProjects?: readonly TraeCnForeignProjectDto[];
  /** CN 归并写门禁（L1379 mergeOperationReady）；false 时项目行不给归并动作。 */
  readonly foreignMergeReady?: boolean;
}
/** 归档视图树入参：折叠集合按「模式\u0000项目」键（L1571-1572），其余与正常视图同源。 */
export interface ArchiveTreeInput {
  readonly projects: readonly MasterProjectEntryDto[]; readonly sessions: readonly MasterSessionEntryDto[];
  readonly archiveDraft: SessionTreeDraft; readonly archiveCollapsedKeys: readonly string[];
  readonly selectedProjectIds: readonly string[]; readonly selectedSessionIds: readonly string[];
  readonly selectMode: boolean;
  readonly relativeTime: (unixSeconds: number | null) => string;
}

/** 正常视图会话（L512-522 逐行）：排除已删除；待恢复优先显示；待归档即时隐藏；原生隐藏值一律不进正常视图。 */
export function liveSessionsOf(sessions: readonly MasterSessionEntryDto[], draft: SessionTreeDraft): readonly MasterSessionEntryDto[] {
  const archiveIds = new Set(draft.archiveSessionIds), restoreIds = new Set(draft.restoreSessionIds);
  return sessions.filter((session) => !session.deleted
    && (restoreIds.has(session.session_id) || (session.hidden_status === null && !archiveIds.has(session.session_id))));
}
/** 归档会话（L525-535 逐行）：含待提交归档，不含待提交恢复；原生 voice_discussion 借位归档。 */
export function archivedSessionsOf(sessions: readonly MasterSessionEntryDto[], draft: SessionTreeDraft): readonly MasterSessionEntryDto[] {
  const archiveIds = new Set(draft.archiveSessionIds), restoreIds = new Set(draft.restoreSessionIds);
  return sessions.filter((session) => !session.deleted && !restoreIds.has(session.session_id)
    && (archiveIds.has(session.session_id) || session.hidden_status === "voice_discussion"));
}
/** 标题匹配（L539）：大小写敏感的子串包含，旧口径即 title.includes(keyword)。 */
export function matchesSessionTitle(session: MasterSessionEntryDto, keyword: string): boolean {
  return session.title.includes(keyword);
}
/** 搜索过滤（L539）：keyword 为空串即原样返回（同一数组引用语义由调用方决定）。 */
export function filterSessionsByTitle(sessions: readonly MasterSessionEntryDto[], keyword: string): readonly MasterSessionEntryDto[] {
  return keyword ? sessions.filter((session) => matchesSessionTitle(session, keyword)) : sessions;
}
/** 具名项目 id 集（L543-546）：name.trim() 非空即具名；空名项目归「未关联文件夹」。 */
export function namedProjectIdsOf(projects: readonly MasterProjectEntryDto[]): ReadonlySet<string> {
  return new Set(projects.filter((project) => project.name.trim()).map((project) => project.project_id));
}
/** 未关联组背后的真实项目 id（L618-621）：选择时展开为真实项目集合。 */
export function unlinkedProjectIdsOf(projects: readonly MasterProjectEntryDto[]): readonly string[] {
  return projects.filter((project) => !project.name.trim()).map((project) => project.project_id);
}
/** 项目显示名（L609-615）：空名项目统一占位「未关联文件夹」。 */
export function projectNameOf(projects: readonly MasterProjectEntryDto[], projectId: string): string {
  const found = projects.find((project) => project.project_id === projectId);
  return found?.name?.trim() || LIBRARY_TEXTS.unlinkedGroup;
}
/** 归档模式层显示词（L2481-2486 原样）：work/code/未标注 三档固定词，其余原样回显。 */
export function modeLabelOf(mode: string): string {
  return mode === "work" ? LIBRARY_TEXTS.modeWork : mode === "code" ? LIBRARY_TEXTS.modeCode
    : mode === LIBRARY_TEXTS.modeUnlabeledRaw ? LIBRARY_TEXTS.modeUnlabeledMode : mode;
}
/** 会话按 project_id 分桶（L553-558）：桶内保持 history.sessions 原次序。 */
export function groupSessionsByProject(sessions: readonly MasterSessionEntryDto[]): Map<string, MasterSessionEntryDto[]> {
  const byProject = new Map<string, MasterSessionEntryDto[]>();
  for (const session of sessions) {
    const list = byProject.get(session.project_id) ?? [];
    list.push(session);
    byProject.set(session.project_id, list);
  }
  return byProject;
}
/** 项目会话计数（L1202-1210，合并弹层规模同源）：跳过已删除，含已归档，按 project_id 累加。 */
export function mergeSessionCountsOf(sessions: readonly MasterSessionEntryDto[]): Map<string, number> {
  const counts = new Map<string, number>();
  for (const session of sessions) {
    if (session.deleted) continue;
    counts.set(session.project_id, (counts.get(session.project_id) ?? 0) + 1);
  }
  return counts;
}
/** 展平树内全部会话行（可视次序：本节点会话行在前，再递归 children——模式层靠 children 承载项目）。 */
export function flattenTreeSessions(nodes: readonly ProjectNodeView[]): readonly SessionRowView[] {
  return nodes.flatMap((node) => [...node.sessions, ...flattenTreeSessions(node.children)]);
}
/** 树内会话行总数（空态判定与批量摘要共用，L1653-1665 / L1777）。 */
export function countTreeSessions(nodes: readonly ProjectNodeView[]): number {
  return flattenTreeSessions(nodes).length;
}

/** 快捷归档动作（L1984-1999）：aria-label 进 label，旧 title 进 hint，testId 逐字沿用。 */
const quickArchiveAction = (session: MasterSessionEntryDto): LibraryActionView => {
  const title = session.title?.trim() || LIBRARY_TEXTS.unnamedSession;
  return { id: "quick-archive", label: fillLibraryText(LIBRARY_TEXTS.quickArchiveAria, { title }), kind: "ghost",
    testId: `library-quick-archive-${session.session_id}`, hint: LIBRARY_TEXTS.quickArchiveHint, disabledReason: null, busyLabel: null };
};
/** 会话行（L1944-2001）：title 空值回退「未命名会话」，meta 为时间文案，badges 在 TRAE 树恒空。 */
const sessionRowOf = (session: MasterSessionEntryDto, selected: boolean, archived: boolean, quickArchive: LibraryActionView | null,
  relativeTime: (unixSeconds: number | null) => string): SessionRowView => ({
  id: session.session_id, testId: `library-session-${session.session_id}`, title: session.title?.trim() || LIBRARY_TEXTS.unnamedSession,
  meta: relativeTime(session.updated_at_unix_seconds), badges: [], selectable: true, selected, archived, quickArchive,
});
/** 正常视图项目节点（L1236-1269 / L1307-1339）：计数文本即旧 lib-project__count，路径只进悬浮提示。 */
const projectNodeOf = (base: { readonly id: string; readonly kind: ProjectNodeKind; readonly testId: string; readonly title: string;
  readonly titleAttr: string | null; readonly expanded: boolean; readonly selected: boolean; readonly selectable: boolean;
  readonly actions?: readonly LibraryActionView[]; readonly sessions: readonly SessionRowView[] }): ProjectNodeView => ({
  id: base.id, testId: base.testId, kind: base.kind, title: base.title, titleAttr: base.titleAttr,
  meta: String(base.sessions.length), expanded: base.expanded, selected: base.selected, selectable: base.selectable,
  actions: base.actions ?? [], sessions: base.sessions, children: [],
});

/**
 * 正常视图树（L1644-1652 的渲染次序）：具名项目（仅含匹配会话，按 projects 次序）→「未关联文件夹」末位
 * →「其他账号的项目」末位。搜索命中时项目与未关联组自动展开（L1220 / L1292）。
 */
export function buildSessionTree(input: SessionTreeInput): readonly ProjectNodeView[] {
  const keyword = input.searchText.trim(); // L509
  const treeSessions = filterSessionsByTitle(liveSessionsOf(input.sessions, input.archiveDraft), keyword); // L512-541
  const byProject = groupSessionsByProject(treeSessions); // L553-558
  const selectedProjects = new Set(input.selectedProjectIds), selectedSessions = new Set(input.selectedSessionIds);
  const expandedIds = new Set(input.expandedIds);
  const searching = keyword !== ""; // L1220 / L1292：搜索时自动展开
  const row = (session: MasterSessionEntryDto, archived: boolean, quickArchive: LibraryActionView | null) =>
    sessionRowOf(session, selectedSessions.has(session.session_id), archived, quickArchive, input.relativeTime);
  const quickArchiveEnabled = !input.selectMode && input.canArchive; // L1278：showQuickArchive = !selectMode && canArchive
  const nodes: ProjectNodeView[] = [];
  const unlinked: MasterSessionEntryDto[] = [];
  for (const project of input.projects) { // L562-574：按 projects 次序，无去重（与旧实现一致）
    const sessions = byProject.get(project.project_id) ?? [];
    if (!project.name.trim()) { unlinked.push(...sessions); continue; } // L564-565：空名项目会话并入末位组
    if (sessions.length === 0) continue; // L566：仅含有匹配会话的具名项目
    nodes.push(projectNodeOf({ id: project.project_id, kind: "named", testId: `library-project-${project.project_id}`,
      title: project.name, titleAttr: project.absolute_path?.trim() || null, // L1243：路径只进悬浮提示
      expanded: searching || expandedIds.has(project.project_id), selected: selectedProjects.has(project.project_id),
      selectable: input.selectMode, sessions: sessions.map((session) => row(session, false, quickArchiveEnabled ? quickArchiveAction(session) : null)) }));
  }
  if (unlinked.length > 0) { // L1646-1648：组内非空才渲染
    const unlinkedIds = unlinkedProjectIdsOf(input.projects); // L618-621
    nodes.push(projectNodeOf({ id: UNLINKED_GROUP_ID, kind: "unlinked", testId: `library-project-${UNLINKED_GROUP_ID}`,
      title: LIBRARY_TEXTS.unlinkedGroup, titleAttr: null, expanded: searching || expandedIds.has(UNLINKED_GROUP_ID),
      selected: unlinkedIds.length > 0 && unlinkedIds.every((id) => selectedProjects.has(id)), // L1293-1294
      selectable: input.selectMode, sessions: unlinked.map((session) => row(session, false, quickArchiveEnabled ? quickArchiveAction(session) : null)) }));
  }
  const foreign = input.foreignProjects ?? [];
  if (foreign.length > 0) { // L1361-1393 / L1649-1651：只读组，行内归并入口由写门禁给
    nodes.push({ id: FOREIGN_GROUP_ID, testId: "library-foreign-group", kind: "foreign", title: LIBRARY_TEXTS.foreignGroup,
      titleAttr: null, meta: "", expanded: true, selected: false, selectable: false, sessions: [], // 旧组头无计数（L1363）
      actions: [], children: foreign.map((project) => ({ id: project.project_id, testId: `library-foreign-project-${project.project_id}`,
        kind: "foreign", title: project.name?.trim() || LIBRARY_TEXTS.unlinkedGroup, // L1376
        titleAttr: project.absolute_path?.trim() || null, // L1369
        meta: String(project.session_count), expanded: false, selected: false, selectable: false, sessions: [], children: [],
        actions: input.foreignMergeReady ? [{ id: "foreign-merge", label: LIBRARY_TEXTS.foreignMerge, kind: "ghost",
          testId: `library-merge-entry-${project.project_id}`, hint: `把这些会话归并到当前账号（共 ${project.message_count} 条消息）`,
          disabledReason: null, busyLabel: null }] : [] })) }); // L1384：旧 title 原文
  }
  return nodes;
}

/**
 * 归档视图树（L582-606 / L1565-1640）：模式（work/code/未标注）→ 项目 → 会话；未关联组恒在各模式末位。
 * 会话次序 = archivedSessions 次序（与正常视图按 projects 分桶不同，逐行照旧）。
 */
export function buildArchiveSessionTree(input: ArchiveTreeInput): readonly ProjectNodeView[] {
  const archived = archivedSessionsOf(input.sessions, input.archiveDraft); // L525-535
  const namedIds = namedProjectIdsOf(input.projects); // L543-546
  const unlinkedIds = unlinkedProjectIdsOf(input.projects); // L618-621
  const selectedProjects = new Set(input.selectedProjectIds), selectedSessions = new Set(input.selectedSessionIds);
  const collapsed = new Set(input.archiveCollapsedKeys);
  const modes = new Map<string, Map<string, MasterSessionEntryDto[]>>(); // L583
  for (const session of archived) { // L584-596
    const mode = session.work_mode?.trim() || LIBRARY_TEXTS.modeUnlabeledRaw; // L585
    const groups = modes.get(mode) ?? new Map<string, MasterSessionEntryDto[]>();
    const groupId = namedIds.has(session.project_id) ? session.project_id : UNLINKED_GROUP_ID; // L589-591
    const list = groups.get(groupId) ?? [];
    list.push(session);
    groups.set(groupId, list);
    modes.set(mode, groups);
  }
  for (const groups of modes.values()) { // L598-604：未关联组始终移到各模式末位
    const unlinked = groups.get(UNLINKED_GROUP_ID);
    if (unlinked) { groups.delete(UNLINKED_GROUP_ID); groups.set(UNLINKED_GROUP_ID, unlinked); }
  }
  return [...modes.entries()].map(([mode, groups]) => ({
    id: mode, testId: "", kind: "mode" as const, title: modeLabelOf(mode), titleAttr: null, // L1566-1567：模式头只出文案，旧容器无锚点（testId 留空，不发明）
    meta: "", expanded: true, selected: false, selectable: false, actions: [], sessions: [], // 旧模式头只有文案（L1567）
    children: [...groups.entries()].map(([projectId, sessions]) => {
      const selectableIds = projectId === UNLINKED_GROUP_ID ? unlinkedIds : [projectId]; // L1569-1570
      return projectNodeOf({ id: projectId, kind: projectId === UNLINKED_GROUP_ID ? "unlinked" : "named",
        testId: `library-archive-project-${projectId}`, title: projectNameOf(input.projects, projectId), // L1619
        titleAttr: null, expanded: !collapsed.has(`${mode}\u0000${projectId}`), // L1571-1572
        selected: selectableIds.length > 0 && selectableIds.every((id) => selectedProjects.has(id)), // L1573-1575
        selectable: input.selectMode,
        sessions: sessions.map((session) => sessionRowOf(session, selectedSessions.has(session.session_id), true, null, input.relativeTime)) });
    }),
  }));
}
