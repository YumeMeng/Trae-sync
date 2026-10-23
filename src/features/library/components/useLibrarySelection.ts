import { useCallback, useRef, useState } from "react";
import type { ProjectNodeView } from "../config";

/**
 * 库页选择态（P4e-8b-1）：G22 统一选择（TRAE 项目↔会话联动）与 WB 逐会话复选共用同一套勾选集合。
 * 旧实现对照 LibrarySessionsPanel.tsx（L）：L240-243 两个集合、L745-755 进入/退出清空、L764-817 项目与会话切换。
 * 本 hook 只持状态与切换；「一个树节点背后有哪些真实项目 id」「一条会话属于哪个项目」由调用方注入解析器
 * （视图行不带 project_id，未关联组背后是多个空名项目 id，不能凭 node.id 猜）。本层不判产品、不重算删除集合。
 * 两个集合放在同一份 state 里：项目切换要同时改两者，分开 setState 会让「全选判定」读到上一轮快照。
 */
export interface LibrarySelectionResolvers {
  /** 树节点背后的真实项目 id（未关联组 = 其下全部空名项目；模式层 / cwd 组 = 空）。 */
  readonly projectIdsOfNode: (node: ProjectNodeView) => readonly string[];
  /** 会话所属项目 id（取消会话勾选时同步取消项目勾选，L807-815）；无归属给 null。 */
  readonly projectIdOfSession: (sessionId: string) => string | null;
}
export interface LibrarySelection {
  readonly selectMode: boolean;
  readonly selectedSessionIds: readonly string[];
  readonly selectedProjectIds: readonly string[];
  /** 进入选择模式并清空勾选（L745）。 */
  readonly enter: () => void;
  /** 退出选择模式并清空勾选（L751；写成功后也用它收尾）。 */
  readonly exit: () => void;
  /** 只清勾选、保留模式（切换账号过滤 / 软删开关时用，旧 WB257-286）。 */
  readonly reset: () => void;
  readonly toggleSession: (sessionId: string) => void;
  /** 项目行切换：勾选其下全部会话（含子级；归档模式层经 children 承载项目，L1565-1640）。 */
  readonly toggleProject: (node: ProjectNodeView) => void;
}
interface Checked { readonly sessions: readonly string[]; readonly projects: readonly string[] }
const EMPTY: Checked = { sessions: [], projects: [] };
/** 节点下的全部可见会话 id（本节点行在前，再递归子级）。 */
const sessionIdsUnder = (node: ProjectNodeView): readonly string[] =>
  [...node.sessions.map((row) => row.id), ...node.children.flatMap(sessionIdsUnder)];
/** 集合增减（保持既有次序，不重排）。 */
const toggleIn = (list: readonly string[], id: string): readonly string[] =>
  list.includes(id) ? list.filter((item) => item !== id) : [...list, id];
/** 批量增删（allSelected 时删、否则加）。 */
const applyTo = (list: readonly string[], ids: readonly string[], allSelected: boolean): readonly string[] => {
  const next = new Set(list);
  for (const id of ids) { if (allSelected) next.delete(id); else next.add(id); }
  return [...next];
};

/**
 * @param resolvers 解析器取用口（每次调用现取）：页面在装配完成后才拿到真实快照，用 getter 可避免
 * 「选择态 hook 必须早于数据 hook」的循环依赖；解析器内容一变，下一次点击立即生效。
 */
export function useLibrarySelection(resolvers: () => LibrarySelectionResolvers): LibrarySelection {
  const latest = useRef(resolvers); latest.current = resolvers; // 渲染期写入（同 useLibraryDeleteActions 口径）
  const [selectMode, setSelectMode] = useState(false);
  const [checked, setChecked] = useState<Checked>(EMPTY);
  const enter = useCallback(() => { setSelectMode(true); setChecked(EMPTY); }, []);
  const exit = useCallback(() => { setSelectMode(false); setChecked(EMPTY); }, []);
  const reset = useCallback(() => setChecked(EMPTY), []);
  // 会话切换（L797-817）：切换会话本身；该会话所属项目若正被勾选，同步取消项目勾选（避免「项目全选」假象）。
  const toggleSession = useCallback((sessionId: string) => {
    const projectId = latest.current().projectIdOfSession(sessionId);
    setChecked((current) => ({
      sessions: toggleIn(current.sessions, sessionId),
      projects: projectId === null ? current.projects : current.projects.filter((id) => id !== projectId),
    }));
  }, []);
  // 项目切换（L764-794）：全选判定按真实项目 id（未关联组 = 多个项目一起勾选），勾选同时覆盖其下全部会话。
  const toggleProject = useCallback((node: ProjectNodeView) => {
    const projectIds = latest.current().projectIdsOfNode(node), sessionIds = sessionIdsUnder(node);
    if (projectIds.length === 0) return; // 模式层 / cwd 组：不是可选项目
    setChecked((current) => {
      const allSelected = projectIds.every((id) => current.projects.includes(id));
      return { sessions: applyTo(current.sessions, sessionIds, allSelected), projects: applyTo(current.projects, projectIds, allSelected) };
    });
  }, []);
  return { selectMode, selectedSessionIds: checked.sessions, selectedProjectIds: checked.projects,
    enter, exit, reset, toggleSession, toggleProject };
}
