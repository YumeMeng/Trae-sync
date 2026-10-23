/**
 * 库页装配表（P4e-8b-1，从 LibraryPage 抽出）：productId → 快照合成 / 读侧出口 / 删除目标派生 / 产品壳差异。
 * 键 = productId 查表分发（与 environment 的 ASSEMBLERS 同法），页面内零产品判断、零 invoke、零命令名。
 * ledger / foreignProjects / accounts 在读取 hook 里放宽为 null（未读到），builder 快照要求非空数组：此处按空集合喂入。
 * 删除目标按产品原始 DTO 反查（视图行不带消息数）：TRAE 取会话 message_count（L850-860：勾项目时其下会话含已归档一起纳入），
 * WB 行无消息数按 0（旧 WB486-500 同口径），cloud 行不进写集合（写红线不碰云端）。
 */
import type { WorkBuddyCwdGroupDto } from "../../../types/workbuddyLibrary";
import { LIBRARY_TEXTS } from "../config";
import type { LibrarySnapshot } from "../registry";
import { formatSessionTime } from "../relayLegs";
import { projectNameOf, unlinkedProjectIdsOf, type SessionTreeDraft } from "../sessionTree";
import type { SessionViewerHeading } from "./SessionViewer";
import type { LibrarySelectionResolvers } from "./useLibrarySelection";
import type { TraeLibraryReadResult } from "./useTraeLibrary";
import type { WbLibraryReadResult } from "./useWbLibrary";
import type { DeleteTargetView } from "./useLibraryDeleteActions";

/** 8b-1 不持归档草稿（草稿与归档应用归 8b-2）：恒空集合，树按「无草稿」口径派生。 */
export const EMPTY_DRAFT: SessionTreeDraft = { archiveSessionIds: [], restoreSessionIds: [] };
/** 装配结果：合成快照 + 查看器标题区 + 读侧出口 + 删除目标派生 + 产品壳差异（容器类名 / 树容器锚）。 */
export interface Assembled {
  readonly snapshot: LibrarySnapshot;
  readonly heading: SessionViewerHeading | null;
  readonly openSession: (sessionId: string) => void;
  readonly closeSession: () => void;
  readonly loadOlder: () => void;
  readonly refresh: (options?: { readonly force?: boolean }) => Promise<{ readonly ok: boolean; readonly error?: string }>;
  readonly targets: (sessionIds: readonly string[], projectIds: readonly string[]) => readonly DeleteTargetView[];
  /** 当前查看器会话 id（删除成功后据此关闭右栏；null = 无查看器）。 */
  readonly viewerSessionId: string | null;
  /** WB 写后重跑体检（旁路）；缺省不跑（旧 WB154）。 */
  readonly refreshCheckup?: () => Promise<unknown>;
  /** 选择态解析器（项目 id / 会话归属）：视图行不带 project_id，只能由装配层按原始 DTO 反查。 */
  readonly selection: LibrarySelectionResolvers;
  readonly shellClass?: string;
  readonly treeTestId?: string;
}
/** 装配入参：两条读取 hook 的结果（未匹配产品的 hook 自门禁，恒空且零 IPC）+ 注入时钟 + 选择态。 */
export interface AssembleInput {
  readonly trae: TraeLibraryReadResult; readonly wb: WbLibraryReadResult; readonly now: Date;
  readonly selectMode: boolean; readonly selectedSessionIds: readonly string[]; readonly selectedProjectIds: readonly string[];
}

export const ASSEMBLERS: Record<string, (input: AssembleInput) => Assembled> = {
  trae_cn: ({ trae, now, selectMode, selectedSessionIds, selectedProjectIds }) => {
    const data = trae.data, history = data.history !== null && data.history.status === "ready" ? data.history : null;
    const session = data.viewer?.session ?? null;
    return {
      snapshot: { productId: "trae_cn", trae: { ...data, ledger: data.ledger ?? [], foreignProjects: data.foreignProjects ?? [],
        archiveDraft: EMPTY_DRAFT, selectedProjectIds, selectedSessionIds, selectMode, archiveView: false,
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
      refresh: (options) => trae.refresh(options),
      targets: (sessionIds, projectIds) => (history?.sessions ?? [])
        .filter((row) => !row.deleted && (sessionIds.includes(row.session_id) || projectIds.includes(row.project_id)))
        .map((row) => ({ id: row.session_id, title: row.title?.trim() || LIBRARY_TEXTS.unnamedSession, messageCount: row.message_count })),
      viewerSessionId: session?.session_id ?? null,
      // 可选项目 = 具名项目（节点 id 即 project_id）与「未关联文件夹」组（背后是全部空名项目，L618-621）；
      // 其他账号组、模式层、cwd 组都不是可勾选项目（旧页同口径）。
      selection: {
        projectIdsOfNode: (node) => node.kind === "named" ? [node.id]
          : node.kind === "unlinked" ? unlinkedProjectIdsOf(history?.projects ?? []) : [],
        projectIdOfSession: (sessionId) => (history?.sessions ?? []).find((row) => row.session_id === sessionId)?.project_id ?? null,
      },
    };
  },
  workbuddy: ({ wb, now, selectedSessionIds }) => {
    const data = wb.data, groups: readonly WorkBuddyCwdGroupDto[] = data.groups ?? [], openId = data.openSessionId;
    const group = openId === null ? undefined : groups.find((item) => item.sessions.some((row) => row.session_id === openId));
    const row = group === undefined || openId === null ? undefined : group.sessions.find((item) => item.session_id === openId);
    return {
      snapshot: { productId: "workbuddy", workbuddy: { ...data, accounts: data.accounts ?? [], selectedIds: selectedSessionIds,
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
      targets: (sessionIds) => groups.flatMap((item) => item.sessions)
        .filter((item) => item.transport !== "cloud" && sessionIds.includes(item.session_id))
        .map((item) => ({ id: item.session_id, title: item.title?.trim() || LIBRARY_TEXTS.unnamedSession, messageCount: 0 })),
      viewerSessionId: openId,
      refreshCheckup: () => wb.runCheckup(),
      // WB 逐会话复选：cwd 组头不可勾选、无项目级联动（§2-3 B），故项目解析器恒空。
      selection: { projectIdsOfNode: () => [], projectIdOfSession: () => null },
      shellClass: "workbuddy-page",
      treeTestId: "workbuddy-library-groups",
    };
  },
};
