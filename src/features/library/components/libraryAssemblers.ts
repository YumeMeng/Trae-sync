/**
 * 库页装配表（P4e-8b-1 建表 / 8b-2 补归档与归并派生）：productId → 快照合成 / 读侧出口 / 删除目标 / 归档与归并派生 / 产品壳差异。
 * 键 = productId 查表分发（与 environment 的 ASSEMBLERS 同法），页面内零产品判断、零 invoke、零命令名。
 * ledger / foreignProjects / accounts 在读取 hook 里放宽为 null（未读到），builder 快照要求非空数组：此处按空集合喂入。
 * 删除目标按产品原始 DTO 反查（视图行不带消息数）：TRAE 取会话 message_count（L850-860：勾项目时其下会话含已归档一起纳入），
 * WB 行无消息数按 0（旧 WB486-500 同口径），cloud 行不进写集合（写红线不碰云端）。
 * 归档可见选择（8b-2）与归并候选（8b-2）同样按原始 DTO 反查：视图行不带 project_id / 会话数，不能凭视图猜。
 */
import type { MasterSessionEntryDto } from "../../../types/history";
import type { WorkBuddyCwdGroupDto } from "../../../types/workbuddyLibrary";
import { LIBRARY_TEXTS, type ArchiveRecoveryView } from "../config";
import { traeWriteGatesOf } from "../traeLibrary";
import type { LibrarySnapshot } from "../registry";
import { formatSessionTime } from "../relayLegs";
import { archivedSessionsOf, liveSessionsOf, mergeSessionCountsOf, projectNameOf, unlinkedProjectIdsOf, type SessionTreeDraft } from "../sessionTree";
import type { SessionViewerHeading } from "./SessionViewer";
import type { LibrarySelectionResolvers } from "./useLibrarySelection";
import type { MergeProjectView } from "./useMergeActions";
import type { TraeLibraryReadResult } from "./useTraeLibrary";
import type { WbLibraryReadResult } from "./useWbLibrary";
import type { DeleteTargetView } from "./useLibraryDeleteActions";

/** 装配结果：合成快照 + 查看器标题区 + 读侧出口 + 派生函数 + 产品壳差异（容器类名 / 树容器锚）。 */
export interface Assembled {
  readonly snapshot: LibrarySnapshot;
  readonly heading: SessionViewerHeading | null;
  readonly openSession: (sessionId: string) => void;
  readonly closeSession: () => void;
  readonly loadOlder: () => void;
  readonly refresh: (options?: { readonly force?: boolean }) => Promise<{ readonly ok: boolean; readonly error?: string }>;
  readonly targets: (sessionIds: readonly string[], projectIds: readonly string[]) => readonly DeleteTargetView[];
  /** 当前视图可见会话中命中选择的会话 id（归档/恢复暂存口径，L838-848）。 */
  readonly visibleSelection: (draft: SessionTreeDraft, archiveView: boolean, sessionIds: readonly string[], projectIds: readonly string[]) => readonly string[];
  /** 项目归并候选（项目 id + 显示名 + 会话数；含已归档、不含已删除，L1202-1210）。 */
  readonly mergeCandidates: (projectIds: readonly string[]) => readonly MergeProjectView[];
  /** 其他账号项目（CN 归并入口按 id 反查显示名）。 */
  readonly foreignProject: (projectId: string) => { readonly projectId: string; readonly name: string } | null;
  /** WB cwd 归并候选（canonical 在前，WB521-537）。 */
  readonly cwdCandidate: (canonical: string) => readonly string[] | null;
  /** 归档写门禁（操作就绪 + 写能力；WB 恒 false）。 */
  readonly archiveEnabled: boolean;
  /** 归档应用前是否需要「关闭客户端」确认（Work 库 true；只读 CN 库 / WB 无归档 false，旧 L760 守卫）。 */
  readonly archiveConfirmRequired: boolean;
  /** 当前查看器会话 id（删除成功后据此关闭右栏；null = 无查看器）。 */
  readonly viewerSessionId: string | null;
  /** WB 写后重跑体检（旁路）；缺省不跑（旧 WB154）。 */
  readonly refreshCheckup?: () => Promise<unknown>;
  /** 选择态解析器（项目 id / 会话归属）：视图行不带 project_id，只能由装配层按原始 DTO 反查。 */
  readonly selection: LibrarySelectionResolvers;
  readonly shellClass?: string;
  readonly treeTestId?: string;
}
/** 装配入参：两条读取 hook 的结果（未匹配产品的 hook 自门禁，恒空且零 IPC）+ 注入时钟 + 选择态 + 归档状态。 */
export interface AssembleInput {
  readonly trae: TraeLibraryReadResult; readonly wb: WbLibraryReadResult; readonly now: Date;
  readonly selectMode: boolean; readonly selectedSessionIds: readonly string[]; readonly selectedProjectIds: readonly string[];
  /** 归档页状态（草稿 / 归档视图 / 折叠键 / 恢复条 / 草稿告警）：WB 忽略（无归档）。 */
  readonly archive: { readonly draft: SessionTreeDraft; readonly view: boolean; readonly collapsedKeys: readonly string[];
    readonly recovery: ArchiveRecoveryView | null; readonly draftError: string | null };
  /** WB 写/归并忙碌位（动作行禁用理由）。 */
  readonly wbBusy: { readonly writeBusy: boolean; readonly mergeBusy: boolean };
  /** 页面忙碌位（归档 / 删除 / 归并任一在跑）：TRAE 批量栏与草稿条的禁用理由据此算。 */
  readonly busy: boolean;
}
/** 当前视图可见会话（L838-848）：归档视图看归档集合、正常视图看正常集合；与搜索无关。 */
const selectedVisibleOf = (visible: readonly MasterSessionEntryDto[], sessionIds: readonly string[], projectIds: readonly string[]): readonly string[] =>
  visible.filter((session) => sessionIds.includes(session.session_id) || projectIds.includes(session.project_id)).map((session) => session.session_id);
const NO_CWD: readonly string[] | null = null;

export const ASSEMBLERS: Record<string, (input: AssembleInput) => Assembled> = {
  trae_cn: ({ trae, now, selectMode, selectedSessionIds, selectedProjectIds, archive, busy }) => {
    const data = trae.data, history = data.history !== null && data.history.status === "ready" ? data.history : null;
    const session = data.viewer?.session ?? null, sessions = history?.sessions ?? [], projects = history?.projects ?? [];
    const counts = mergeSessionCountsOf(sessions);
    const gates = traeWriteGatesOf(data.libraryId, data.readOnly, data.operations); // 写门禁与 builder 同一份算法
    return {
      snapshot: { productId: "trae_cn", trae: { ...data, ledger: data.ledger ?? [], foreignProjects: data.foreignProjects ?? [],
        archiveDraft: archive.draft, selectedProjectIds, selectedSessionIds, selectMode, archiveView: archive.view,
        archiveCollapsedKeys: archive.collapsedKeys, archiveRecovery: archive.recovery, draftStorageError: archive.draftError,
        busy } }, // 忙碌位由页面统一给（归档 / 删除 / 归并共用），builder 只用于算 disabled
      heading: session === null ? null : {
        title: session.title?.trim() || LIBRARY_TEXTS.unnamedSession,
        projectName: projectNameOf(projects, session.project_id),
        messageCount: session.message_count,
        lastActiveLabel: formatSessionTime(session.updated_at_unix_seconds, now),
      },
      openSession: (sessionId) => {
        const target = sessions.find((row) => row.session_id === sessionId);
        if (target !== undefined) void trae.openSession(target);
      },
      closeSession: trae.closeSession,
      loadOlder: () => void trae.loadOlder(),
      refresh: (options) => trae.refresh(options),
      targets: (sessionIds, projectIds) => sessions
        .filter((row) => !row.deleted && (sessionIds.includes(row.session_id) || projectIds.includes(row.project_id)))
        .map((row) => ({ id: row.session_id, title: row.title?.trim() || LIBRARY_TEXTS.unnamedSession, messageCount: row.message_count })),
      visibleSelection: (draft, archiveView, sessionIds, projectIds) =>
        selectedVisibleOf(archiveView ? archivedSessionsOf(sessions, draft) : liveSessionsOf(sessions, draft), sessionIds, projectIds),
      // 归并候选只含有名称的项目（L1071 / L1826-1828）；会话数含已归档、不含已删除（L1202-1210）。
      mergeCandidates: (projectIds) => projects.filter((project) => projectIds.includes(project.project_id) && project.name.trim() !== "")
        .map((project) => ({ projectId: project.project_id, name: project.name, sessionCount: counts.get(project.project_id) ?? 0 })),
      foreignProject: (projectId) => (data.foreignProjects ?? []).filter((project) => project.project_id === projectId)
        .map((project) => ({ projectId: project.project_id, name: project.name }))[0] ?? null,
      cwdCandidate: () => NO_CWD, // cwd 归并是 WB 专属（能力表 cwdMerge: absent）
      archiveEnabled: gates.canArchive, // L185：操作就绪 + 写能力双门禁
      archiveConfirmRequired: !data.readOnly, // L927：只读库（Trae CN）不弹「关闭客户端」确认
      viewerSessionId: session?.session_id ?? null,
      // 可选项目 = 具名项目（节点 id 即 project_id）与「未关联文件夹」组（背后是全部空名项目，L618-621）；
      // 其他账号组、模式层、cwd 组都不是可勾选项目（旧页同口径）。
      selection: {
        projectIdsOfNode: (node) => node.kind === "named" ? [node.id]
          : node.kind === "unlinked" ? unlinkedProjectIdsOf(projects) : [],
        projectIdOfSession: (sessionId) => sessions.find((row) => row.session_id === sessionId)?.project_id ?? null,
      },
    };
  },
  workbuddy: ({ wb, now, selectedSessionIds, wbBusy }) => {
    const data = wb.data, groups: readonly WorkBuddyCwdGroupDto[] = data.groups ?? [], openId = data.openSessionId;
    const group = openId === null ? undefined : groups.find((item) => item.sessions.some((row) => row.session_id === openId));
    const row = group === undefined || openId === null ? undefined : group.sessions.find((item) => item.session_id === openId);
    return {
      snapshot: { productId: "workbuddy", workbuddy: { ...data, accounts: data.accounts ?? [], selectedIds: selectedSessionIds,
        writeBusy: wbBusy.writeBusy, mergeBusy: wbBusy.mergeBusy } },
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
      targets: (sessionIds) => groups.flatMap((item) => item.sessions)
        .filter((item) => item.transport !== "cloud" && sessionIds.includes(item.session_id))
        .map((item) => ({ id: item.session_id, title: item.title?.trim() || LIBRARY_TEXTS.unnamedSession, messageCount: 0 })),
      // WB 无归档：可见集合即全部组内会话，选择即复选集合（本票不消费）。
      visibleSelection: (_draft, _archiveView, sessionIds) => groups.flatMap((item) => item.sessions)
        .filter((item) => item.transport !== "cloud" && sessionIds.includes(item.session_id)).map((item) => item.session_id),
      mergeCandidates: () => [], // 项目归并是 TRAE 专属（能力表 projectMerge: absent）
      foreignProject: () => null, // 无其他账号项目组
      // 候选组 = checkup.cwd_merge_candidates 里 canonical 相同的一组（canonical 在前，WB521-537）。
      cwdCandidate: (canonical) => (data.checkup?.cwd_merge_candidates ?? []).find((item) => item[0] === canonical) ?? NO_CWD,
      archiveEnabled: false, archiveConfirmRequired: false, // WB 无归档（能力表 archive: absent）
      viewerSessionId: openId,
      refreshCheckup: () => wb.runCheckup(),
      // WB 逐会话复选：cwd 组头不可勾选、无项目级联动（§2-3 B），故项目解析器恒空。
      selection: { projectIdsOfNode: () => [], projectIdOfSession: () => null },
      shellClass: "workbuddy-page",
      treeTestId: "workbuddy-library-groups",
    };
  },
};
