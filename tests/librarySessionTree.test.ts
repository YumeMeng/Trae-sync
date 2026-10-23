// ============================================================================
// 会话树纯派生契约（P4e-3c）：守旧 LibrarySessionsPanel.tsx（下称 L）的分组 / 搜索 / 归档层口径。
// 纯函数直喂 DTO 快照：零 mock、零渲染、零时钟；断言一律具体值（次序、文案、testid、计数）。
// ============================================================================
import { describe, expect, it } from "vitest";
import {
  FOREIGN_GROUP_ID, UNLINKED_GROUP_ID, buildArchiveSessionTree, buildSessionTree, countTreeSessions,
  flattenTreeSessions, mergeSessionCountsOf, type ArchiveTreeInput, type SessionTreeDraft, type SessionTreeInput,
} from "../src/features/library/sessionTree";
import type { MasterProjectEntryDto, MasterSessionEntryDto, TraeCnForeignProjectDto } from "../src/types/history";

const project = (project_id: string, name: string, absolute_path: string | null = null): MasterProjectEntryDto => ({ project_id, name, absolute_path });
const session = (session_id: string, project_id: string, overrides: Partial<MasterSessionEntryDto> = {}): MasterSessionEntryDto => ({
  session_id, project_id, title: session_id, message_count: 3, updated_at_unix_seconds: 1_700_000_000,
  deleted: false, hidden_status: null, work_mode: "work", ...overrides });
const draft = (archiveSessionIds: readonly string[] = [], restoreSessionIds: readonly string[] = []): SessionTreeDraft => ({ archiveSessionIds, restoreSessionIds });
const treeInput = (overrides: Partial<SessionTreeInput> = {}): SessionTreeInput => ({
  projects: [], sessions: [], archiveDraft: draft(), searchText: "", expandedIds: [], selectedProjectIds: [],
  selectedSessionIds: [], selectMode: false, canArchive: false, relativeTime: () => "刚刚", ...overrides });
const archiveInput = (overrides: Partial<ArchiveTreeInput> = {}): ArchiveTreeInput => ({
  projects: [], sessions: [], archiveDraft: draft(), archiveCollapsedKeys: [], selectedProjectIds: [],
  selectedSessionIds: [], selectMode: false, relativeTime: () => "刚刚", ...overrides });
const foreign = (project_id: string, name: string): TraeCnForeignProjectDto => ({ project_id, name, absolute_path: `/cn/${project_id}`, session_count: 2, message_count: 9 });
const ids = (nodes: readonly { readonly id: string }[]) => nodes.map((node) => node.id);
const rowsOf = (nodes: readonly { readonly sessions: readonly { readonly id: string }[] }[]) => nodes.flatMap((node) => node.sessions.map((row) => row.id));
const archivedRowsOf = (nodes: readonly { readonly children: readonly { readonly sessions: readonly { readonly id: string }[] }[] }[]) =>
  nodes.flatMap((node) => node.children.flatMap((child) => child.sessions.map((row) => row.id)));

describe("buildSessionTree（正常视图树）", () => {
  // 契约：分组次序 = projects 数组次序 → 未关联组末位 → 其他账号组末位；组内保持 sessions 次序（旧实现零 sort）。
  it("分组次序：projects 次序 → 未关联末位 → 其他账号末位，组内保持 sessions 次序", () => {
    const nodes = buildSessionTree(treeInput({
      projects: [project("p1", "甲", "/repo/one"), project("p-empty", "  "), project("p2", "乙")],
      sessions: [session("s2", "p2"), session("s1b", "p1"), session("s1a", "p1"), session("su", "p-empty")],
      foreignProjects: [foreign("f1", "别人项目")],
    }));
    expect(ids(nodes)).toEqual(["p1", "p2", UNLINKED_GROUP_ID, FOREIGN_GROUP_ID]);
    expect(nodes.map((node) => node.kind)).toEqual(["named", "named", "unlinked", "foreign"]);
    expect(rowsOf(nodes)).toEqual(["s1b", "s1a", "s2", "su"]); // 组内次序 = history.sessions 次序
    expect(nodes[0]).toMatchObject({ title: "甲", titleAttr: "/repo/one", meta: "2", testId: "library-project-p1" });
  });

  // 契约：其他账号组头只有文案无计数、子行计数=session_count；归并入口只在写门禁就绪时给。
  it("其他账号组：组头无计数、路径只进悬浮、归并入口受 foreignMergeReady 门禁", () => {
    const foreignProjects = [foreign("f1", "别人项目")];
    const nodes = buildSessionTree(treeInput({ projects: [project("p1", "甲")], sessions: [session("s1", "p1")], foreignProjects }));
    expect(ids(nodes)).toEqual(["p1", FOREIGN_GROUP_ID]); // 组恒在具名项目之后
    expect(nodes[1]).toMatchObject({ meta: "", title: "其他账号的项目", testId: "library-foreign-group", expanded: true, selectable: false });
    expect(nodes[1].children[0]).toMatchObject({ meta: "2", title: "别人项目", titleAttr: "/cn/f1", testId: "library-foreign-project-f1", actions: [] });
    const ready = buildSessionTree(treeInput({ projects: [project("p1", "甲")], sessions: [session("s1", "p1")], foreignProjects, foreignMergeReady: true }));
    expect(ready[1].children[0].actions[0]).toMatchObject({ testId: "library-merge-entry-f1", label: "归并到当前账号", hint: "把这些会话归并到当前账号（共 9 条消息）" });
  });

  // 契约：空名项目会话平铺进「未关联文件夹」（不做二级展示）；具名但 0 匹配会话的项目整体跳过。
  it("空名项目会话平铺进「未关联文件夹」、具名但 0 匹配会话的项目跳过；勾选展开真实项目 id", () => {
    const projects = [project("e1", "  "), project("e2", ""), project("p1", "甲")];
    const sessions = [session("s1", "e1"), session("s2", "e2"), session("s3", "p1")];
    const nodes = buildSessionTree(treeInput({ projects, sessions }));
    expect(ids(nodes)).toEqual(["p1", UNLINKED_GROUP_ID]);
    expect(nodes[1]).toMatchObject({ kind: "unlinked", title: "未关联文件夹", meta: "2", testId: "library-project-__unlinked__" });
    expect(nodes[1].sessions.map((row) => row.id)).toEqual(["s1", "s2"]);
    // 未关联组勾选 = 全部空名真实项目都被选（L1293-1294）
    expect(nodes[1].selected).toBe(false);
    expect(buildSessionTree(treeInput({ projects, sessions, selectedProjectIds: ["e1", "e2"] }))[1].selected).toBe(true);
    expect(buildSessionTree(treeInput({ projects, sessions, selectedProjectIds: ["e1"] }))[1].selected).toBe(false);
  });

  // 契约：搜索 trim 后子串命中即过滤，项目与未关联组自动展开；大小写敏感（旧 title.includes，不小写化）。
  it("搜索：trim 后子串命中即过滤，项目与未关联组自动展开；未命中项目整体消失", () => {
    const projects = [project("p1", "甲"), project("p2", "乙"), project("e1", " ")];
    const sessions = [session("s1", "p1", { title: "登录排查" }), session("s2", "p2", { title: "导出报表" }), session("s3", "e1", { title: "登录排查" })];
    const nodes = buildSessionTree(treeInput({ projects, sessions, searchText: " 登录 " }));
    expect(ids(nodes)).toEqual(["p1", UNLINKED_GROUP_ID]);
    expect(nodes.map((node) => node.expanded)).toEqual([true, true]); // L1220 / L1292 搜索时自动展开
    expect(rowsOf(nodes)).toEqual(["s1", "s3"]);
    expect(buildSessionTree(treeInput({ projects, sessions })).map((node) => node.expanded)).toEqual([false, false, false]);
    expect(buildSessionTree(treeInput({ projects, sessions: [session("s1", "p1", { title: "ABC" })], searchText: "abc" }))).toHaveLength(0);
  });

  // 契约：搜索只作用于正常视图；归档视图不吃 keyword，全部归档会话照旧出现。
  it("搜索只作用正常视图：归档视图不吃 keyword，全部归档会话照旧出现", () => {
    const projects = [project("p1", "甲")];
    const sessions = [session("a1", "p1", { title: "归档一", hidden_status: "voice_discussion" }), session("a2", "p1", { title: "归档二", hidden_status: "voice_discussion" })];
    expect(buildSessionTree(treeInput({ projects, sessions, searchText: "归档" }))).toHaveLength(0);
    expect(archivedRowsOf(buildArchiveSessionTree(archiveInput({ projects, sessions })))).toEqual(["a1", "a2"]);
  });

  // 契约：正常视图排除已删与原生隐藏值；待归档即时隐藏、待恢复优先显示（草稿先于库状态）。
  it("正常视图过滤：排除已删与原生隐藏，待归档即时隐藏、待恢复优先显示", () => {
    const sessions = [
      session("deleted", "p1", { deleted: true }), session("native", "p1", { hidden_status: "scheduled_task" }),
      session("normal", "p1"), session("pendingArchive", "p1"), session("pendingRestore", "p1", { hidden_status: "voice_discussion" }),
    ];
    const nodes = buildSessionTree(treeInput({ projects: [project("p1", "甲")], sessions, archiveDraft: draft(["pendingArchive"], ["pendingRestore"]) }));
    expect(rowsOf(nodes)).toEqual(["normal", "pendingRestore"]);
  });

  // 契约：会话行空标题回退「未命名会话」、时间文案由入参注入、快捷归档 = !selectMode && canArchive。
  it("会话行：空标题回退「未命名会话」、时间文案由入参注入、快捷归档只在浏览态且可归档时给", () => {
    const sessions = [session("s1", "p1", { title: "   ", updated_at_unix_seconds: 42 })];
    const browse = buildSessionTree(treeInput({ projects: [project("p1", "甲")], sessions, canArchive: true, relativeTime: (seconds) => `#${seconds}` }));
    expect(browse[0].sessions[0]).toMatchObject({ id: "s1", title: "未命名会话", meta: "#42", testId: "library-session-s1", archived: false, badges: [] });
    expect(browse[0].sessions[0].quickArchive).toMatchObject({ testId: "library-quick-archive-s1", label: "归档会话：未命名会话", hint: "归档这个会话" });
    expect(buildSessionTree(treeInput({ projects: [project("p1", "甲")], sessions, canArchive: true, selectMode: true }))[0].sessions[0].quickArchive).toBeNull();
    expect(buildSessionTree(treeInput({ projects: [project("p1", "甲")], sessions }))[0].sessions[0].quickArchive).toBeNull();
    expect(buildSessionTree(treeInput({ projects: [project("p1", "甲")], sessions, selectMode: true }))[0].selectable).toBe(true);
  });
});

describe("buildArchiveSessionTree（归档视图树）", () => {
  // 契约：work_mode 空串/空白 → 「未标注」；同模式合并；模式头只有文案（无 testid、无计数）。
  it("模式层：work_mode 空串/空白 → 「未标注」，同模式合并、标签逐字沿用旧档", () => {
    const sessions = [
      session("w1", "p1", { hidden_status: "voice_discussion", work_mode: "work" }), session("n1", "p1", { hidden_status: "voice_discussion", work_mode: null }),
      session("n2", "p1", { hidden_status: "voice_discussion", work_mode: "   " }), session("c1", "p1", { hidden_status: "voice_discussion", work_mode: "code" }),
    ];
    const modes = buildArchiveSessionTree(archiveInput({ projects: [project("p1", "甲")], sessions }));
    expect(ids(modes)).toEqual(["work", "未标注", "code"]);
    expect(modes.map((node) => node.title)).toEqual(["Work 模式", "未标注模式", "Code 模式"]);
    expect(modes[1].children[0].sessions.map((row) => row.id)).toEqual(["n1", "n2"]);
    expect(modes[0]).toMatchObject({ testId: "", meta: "", kind: "mode", expanded: true, selected: false, selectable: false, sessions: [] });
  });

  // 契约：未关联组恒在各模式末位；组内次序 = archivedSessions 次序（不是 projects 次序）；归档行不给快捷归档。
  it("未关联组恒在各模式末位；组内会话次序 = archivedSessions 次序（非 projects 次序）", () => {
    const projects = [project("p1", "甲"), project("e1", " ")];
    const sessions = [
      session("x", "p1", { hidden_status: "voice_discussion" }), session("y", "e1", { hidden_status: "voice_discussion" }),
      session("z", "p1", { hidden_status: "voice_discussion" }), session("w", "e1", { hidden_status: "voice_discussion" }),
    ];
    const [work] = buildArchiveSessionTree(archiveInput({ projects, sessions }));
    expect(work.children.map((child) => child.id)).toEqual(["p1", UNLINKED_GROUP_ID]);
    expect(work.children.map((child) => child.sessions.map((row) => row.id))).toEqual([["x", "z"], ["y", "w"]]);
    expect(work.children[1]).toMatchObject({ title: "未关联文件夹", testId: "library-archive-project-__unlinked__" });
    expect(work.children[0].sessions[0]).toMatchObject({ archived: true, quickArchive: null });
  });

  // 契约：折叠键为「模式\u0000项目」；归档默认展开；未关联组勾选展开为其真实项目集合。
  it("折叠键为「模式\\u0000项目」；未关联组勾选展开为其真实项目集合", () => {
    const projects = [project("p1", "甲"), project("e1", " "), project("e2", "")];
    const sessions = [
      session("x", "p1", { hidden_status: "voice_discussion" }), session("y", "e1", { hidden_status: "voice_discussion" }),
      session("w", "e2", { hidden_status: "voice_discussion" }),
    ];
    const collapsed = buildArchiveSessionTree(archiveInput({ projects, sessions, archiveCollapsedKeys: ["work\u0000p1"] }));
    expect(collapsed[0].children.map((child) => child.expanded)).toEqual([false, true]);
    expect(buildArchiveSessionTree(archiveInput({ projects, sessions, selectedProjectIds: ["e1", "e2"] }))[0].children[1].selected).toBe(true);
    expect(buildArchiveSessionTree(archiveInput({ projects, sessions, selectedProjectIds: ["e1"] }))[0].children[1].selected).toBe(false);
  });
});

describe("计数与展平", () => {
  // 契约：mergeSessionCountsOf 跳过已删、含已归档；flattenTreeSessions 按可视次序递归展平（含 children）。
  it("mergeSessionCountsOf 跳过已删、含已归档；flattenTreeSessions 按可视次序递归展平", () => {
    const sessions = [session("a", "p1"), session("b", "p1", { hidden_status: "voice_discussion" }), session("c", "p1", { deleted: true }), session("d", "p2")];
    const counts = mergeSessionCountsOf(sessions);
    expect([counts.get("p1"), counts.get("p2")]).toEqual([2, 1]); // a + 已归档 b；已删 c 不计
    const live = buildSessionTree(treeInput({ projects: [project("p1", "甲")], sessions: [session("s2", "p1"), session("s1", "p1")] }));
    expect(flattenTreeSessions(live).map((row) => row.id)).toEqual(["s2", "s1"]);
    expect([countTreeSessions(live), countTreeSessions([])]).toEqual([2, 0]);
    const archived = [session("ar1", "p1", { hidden_status: "voice_discussion" }), session("ar2", "p1", { hidden_status: "voice_discussion" })];
    expect(flattenTreeSessions(buildArchiveSessionTree(archiveInput({ projects: [project("p1", "甲")], sessions: archived }))).map((row) => row.id)).toEqual(["ar1", "ar2"]);
  });
});
