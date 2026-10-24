import { beforeEach, describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { installFakeInvoke, mockCoreInvoke, mockRoute, resetRoutes } from "./helpers/fakeInvoke";
// P4e-9a：旧四份库页测试迁到合并页 LibraryPage（锚点与文案逐字沿用旧实现）。
vi.mock("@tauri-apps/api/core", async () => {
  const fake = await import("./helpers/fakeInvoke");
  return { invoke: fake.mockCoreInvoke };
});
const { mockListen } = vi.hoisted(() => ({ mockListen: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({ listen: mockListen }));
vi.mock("../src/platform/notify", () => ({ notify: { success: vi.fn(), error: vi.fn(), info: vi.fn(), warning: vi.fn(), summary: vi.fn() } }));
import { notify } from "../src/platform/notify";
import { LibraryPage } from "../src/features/library/LibraryPage";

const NOW = Math.floor(Date.now() / 1000), notifyMock = vi.mocked(notify);
const FP = { db: { mtime_secs: 1700000000, mtime_nanos: 0, size: 1000 }, wal: null, shm: null };
const row = (id: string, projectId: string, title: string, count: number, age: number, hidden: string | null = null, mode = "code") =>
  ({ session_id: id, project_id: projectId, title, message_count: count, updated_at_unix_seconds: NOW - age, deleted: false, hidden_status: hidden, work_mode: mode });
const HISTORY = { status: "ready", current_user_id: "u-b", fingerprint: FP,
  projects: [{ project_id: "p1", name: "项目一", absolute_path: "d:\\work\\project-one" }, { project_id: "p2", name: "项目二", absolute_path: null }, { project_id: "p9", name: "", absolute_path: null }],
  sessions: [row("s1", "p1", "会话一", 10, 3600), row("s2", "p1", "会话二", 3, 40 * 86400), row("s3", "p2", "旧归档会话", 1, 40 * 86400, "voice_discussion", "work"), row("s4", "p2", "会话四", 5, 30), row("s5", "p2", "会话五", 2, 7200), row("s9", "p9", "未关联会话", 2, 600)] };
const msg = (id: string, role: string, type: string, text: string, steps = 0, thoughts: string[] = []) =>
  ({ message_id: id, role, message_type: type, created_at_unix_seconds: NOW - 60, content: { kind: steps > 0 ? "task_trace" : "text", text, step_count: steps, thoughts } });
const OPS = ["library.archive", "library.delete", "library.work_project_merge", "library.trae_cn_project_merge"].map((id) => ({ operation_id: id, state: "ready", reason: "已验收。", evidence_ref: null }));
const PLUGIN_STATE = { known_account_count: 2, manifest: [], installed: [
  { record_id: "rec-1", marketplace_plugin_id: "uuid-p1", name: "plugin-one", display_name: "插件一", version: "1.0.0", registry: "trae-remote-official", builtin: false, in_manifest: true },
  { record_id: "rec-2", marketplace_plugin_id: null, name: "builtin-lark", display_name: "飞书", version: "0.0.1", registry: "builtin", builtin: true, in_manifest: false }] };
const MARKET = [{ plugin_id: "uuid-p1", name: "plugin-one", display_name: "插件一", description: "已装插件", registry: "trae-remote-official", categories: [], category_key: "efficiency", category_name: "效率工具" },
  { plugin_id: "uuid-p2", name: "plugin-two", display_name: "插件二", description: "合成市场描述二", registry: "trae-remote-official", categories: [], category_key: "efficiency", category_name: "效率工具" }];
function traeRoutes(history: () => unknown = () => HISTORY) {
  mockRoute("get_master_history", history);
  mockRoute("get_relay_ledger", () => []);
  mockRoute("get_product_operation_states", (a: any) => ({ product_id: a.productId, library_id: a.libraryId, operations: OPS }));
  mockRoute("get_master_library_stats", () => ({ status: "ready", current_user_id: "u-b", project_count: 4, session_count: 16, message_count: 128, participating_account_count: 3, last_active_unix_seconds: NOW - 3600, size_bytes: 712 * 1024 * 1024 }));
  mockRoute("get_master_backup_chain", () => ({ backups: [], backup_dir: "C:\\bak", keep_policy: 5 }));
  mockRoute("get_environment_state", () => ({ env_id: "master", current_profile_id: "pa", current_account_name: "账号甲", observed_profile_id: "pa", data_dir: "C:\\data\\master", running: false, login_state: "logged_in", created_at_unix_seconds: 1750000000 }));
  mockRoute("get_archive_draft", () => ({ archive_session_ids: [], restore_session_ids: [] }));
  mockRoute("set_archive_draft", () => true);
  mockRoute("get_master_session_messages", (a: any) => (a.sessionId === "s1"
    ? { session_id: "s1", status: "ready", has_more: false, messages: [msg("m1", "user", "general", "帮我看下这个报错"), msg("m2", "assistant", "task", "", 2, ["先定位", "再修复"])] }
    : { session_id: a.sessionId, status: "ready", has_more: false, messages: [msg("m1", "user", "general", "普通会话消息")] }));
  mockRoute("get_master_verification", () => ({ ledger: { status: "ready", checked_count: 0, relayed_away_count: 0, issues: [] },
    backup: { status: "ready", backup_stamp: NOW - 7200, backup_stamps: [NOW - 7200], common_count: 15, added_count: 1, relayed_away_count: 0, missing: [] } }));
  mockRoute("get_plugin_tab_state", () => PLUGIN_STATE);
  mockRoute("browse_plugin_market", () => MARKET);
  mockRoute("install_plugin", () => undefined);
  mockRoute("uninstall_plugin_everywhere", () => ({ accounts: [{ display_name: "账号甲", removed: true, failed: false, error_code: null }] }));
  mockRoute("list_trae_cn_foreign_projects", () => [{ project_id: "p-f1", name: "其他账号目录", absolute_path: "D:\\work\\shared", session_count: 2, message_count: 9 }]);
  mockRoute("delete_master_sessions", () => ({ deleted_sessions: 2, deleted_messages: 13, removed_projects: 0, backup_path: "C:\\bak" }));
  mockRoute("apply_master_archive_changes", () => ({ archived_sessions: 1, restored_sessions: 0, relaunch_outcome: "focused" }));
  mockRoute("merge_master_projects", () => ({ moved_sessions: 2, removed_projects: 1, backup_path: "C:\\bak" }));
}
const WB_GROUPS = [{ cwd: "D:\\work\\proj-a", escaped_dir: "d--work-proj-a", session_count: 2, last_active_unix_seconds: NOW - 60, sessions: [
  { session_id: "s-1", title: "会话一", transport: "local", deleted: false, last_active_unix_seconds: NOW - 60 },
  { session_id: "s-2", title: "会话二", transport: "cloud", deleted: false, last_active_unix_seconds: NOW - 120 }] }];
function wbRoutes(groups: () => unknown = () => WB_GROUPS) {
  mockRoute("get_workbuddy_library_stats", () => ({ account_count: 1, session_count: 2, soft_deleted_session_count: 0, project_count: 1, projects_dir_bytes: 2048, last_active_unix_seconds: NOW - 60 }));
  mockRoute("get_workbuddy_library_history", groups);
  mockRoute("list_workbuddy_accounts", () => [{ profile_id: "wb-1", account_id: "uid-1", screen_name: "甲账号", domain: "x", access_token_expires_at_unix_seconds: 1, refresh_token_expires_at_unix_seconds: 1, minted_at_unix_seconds: 1, last_updated_at_unix_seconds: 1 }]);
  mockRoute("get_workbuddy_session_messages", () => ({ session_id: "s-1", skipped_lines: 0, body_missing: false,
    messages: [{ index: 0, kind: "message", role: "user", timestamp_unix_ms: 1760000000000, text: "你好" },
      { index: 1, kind: "message", role: "assistant", timestamp_unix_ms: 1760000001000, text: "你好，有什么可以帮你？" }] }));
  mockRoute("get_workbuddy_library_checkup", () => ({ orphan_body_files: ["a.jsonl"], missing_body_count: 1, missing_body_samples: ["s-9"], soft_deleted_session_count: 0, foreign_dirs: [], cwd_merge_candidates: [["d--work-proj-a", "d--work-proj-a-v2"]] }));
  mockRoute("delete_workbuddy_sessions", () => ({ deleted_sessions: 1, skipped_ids: 0, tail: { restarted: true, process_count_after_restart: 1 } }));
  mockRoute("purge_workbuddy_sessions", () => ({ purged_sessions: 1, removed_body_files: 1, missing_body_files: 0, failed_body_files: [], backup_path: "C:\\backup\\wb", tail: { restarted: true, process_count_after_restart: 1 } }));
  mockRoute("merge_workbuddy_cwd_projects", () => ({ moved_sessions: 2, moved_body_files: 2, missing_body_files: 0, conflict_body_files: 0, tail: { restarted: true, process_count_after_restart: 1 } }));
}
const argsOf = (cmd: string): any => mockCoreInvoke.mock.calls.filter((c) => c[0] === cmd).at(-1)?.[1];
const called = (cmd: string): boolean => mockCoreInvoke.mock.calls.some((c) => c[0] === cmd);
const renderTrae = (libraryId = "master", onAction?: (id: string) => void) =>
  render(<LibraryPage active productId="trae_cn" libraryId={libraryId} {...(onAction ? { onAction } : {})} />);
const renderWb = () => render(<LibraryPage active productId="workbuddy" />);
const expand = async (projectId: string) => fireEvent.click(await screen.findByTestId(`library-project-${projectId}`));
const filterTo = async (uid: string) => {
  fireEvent.change(screen.getByTestId("workbuddy-library-uid-filter"), { target: { value: uid } });
  await waitFor(() => expect(argsOf("get_workbuddy_library_history")).toEqual({ userId: uid, includeDeleted: false }));
};
beforeEach(() => {
  vi.clearAllMocks(); resetRoutes(); installFakeInvoke();
  mockListen.mockResolvedValue(() => undefined); window.localStorage.clear();
});
describe("库页（TRAE）：三 tab / 树 / 查看器 / 选择与删除", () => {
  it("三 tab 与库信息 / 插件 tab：五项信息 + 计数条 + 备份对比，插件清单按 tab 激活才读", async () => {
    traeRoutes(); renderTrae();
    expect(await screen.findByTestId("library-project-p1")).toBeInTheDocument(); expect(screen.getByTestId("master-tab-sessions")).toHaveAttribute("aria-selected", "true");
    fireEvent.click(screen.getByTestId("master-tab-info"));
    expect(await screen.findByTestId("master-detail-info")).toBeInTheDocument(); expect(screen.getByTestId("master-info-account")).toHaveTextContent("账号甲");
    expect(screen.getByTestId("master-info-size")).toHaveTextContent("712 MB"); expect(screen.getByTestId("master-detail-counts")).toHaveTextContent("16");
    expect(await screen.findByTestId("verification-backup-block")).toBeInTheDocument(); expect(called("get_plugin_tab_state")).toBe(false);
    fireEvent.click(screen.getByTestId("master-tab-plugins"));
    expect(await screen.findByTestId("plugin-workbench")).toBeInTheDocument(); expect(called("get_plugin_tab_state")).toBe(true);
  });
  it("树结构：项目行计数 + 子级按需展开 + 未关联文件夹末位 + 路径只进悬浮提示", async () => {
    traeRoutes(); renderTrae();
    await waitFor(() => expect(argsOf("get_master_history")).toEqual({ previous: null, previousCurrentUserId: null, libraryId: "master" }));
    expect(await screen.findByTestId("library-project-p1")).toHaveTextContent("项目一"); expect(screen.getByTestId("library-project-p1")).toHaveTextContent("2");
    expect(screen.getByTestId("library-project-p1")).toHaveAttribute("title", "d:\\work\\project-one"); expect(screen.queryByTestId("library-session-s1")).not.toBeInTheDocument();
    await expand("p1");
    expect(screen.getByTestId("library-session-s1")).toHaveTextContent("会话一");
    fireEvent.click(screen.getByTestId("library-project-p1"));
    expect(screen.queryByTestId("library-session-s1")).not.toBeInTheDocument();
    fireEvent.click(screen.getByTestId("library-project-__unlinked__"));
    expect(screen.getByTestId("library-session-s9")).toHaveTextContent("未关联会话"); expect(screen.getByTestId("library-viewer-empty")).toHaveTextContent("从左侧选择一个会话查看对话");
  });
  it("搜索过滤树内会话：命中项目自动展开，无命中项目与未关联组隐藏，清空恢复", async () => {
    traeRoutes(); renderTrae();
    await screen.findByTestId("library-project-p1");
    fireEvent.change(screen.getByTestId("library-search-input"), { target: { value: "会话二" } });
    expect(screen.getByTestId("library-session-s2")).toBeInTheDocument(); expect(screen.queryByTestId("library-session-s1")).not.toBeInTheDocument();
    expect(screen.queryByTestId("library-project-p2")).not.toBeInTheDocument(); expect(screen.queryByTestId("library-project-__unlinked__")).not.toBeInTheDocument();
    fireEvent.change(screen.getByTestId("library-search-input"), { target: { value: "" } });
    expect(screen.getByTestId("library-project-p2")).toBeInTheDocument();
  });
  it("点会话出查看器：标题/项目/消息数 + 消息流（含 thoughts 轨迹展开）+ 关闭回空态", async () => {
    traeRoutes(); renderTrae();
    await expand("p1");
    fireEvent.click(screen.getByTestId("library-session-s1"));
    const viewer = await screen.findByTestId("library-viewer");
    expect(screen.getByTestId("library-viewer-title")).toHaveTextContent("会话一"); expect(viewer).toHaveTextContent("项目一"); expect(viewer).toHaveTextContent("10 条消息");
    await waitFor(() => expect(screen.getByTestId("library-viewer-messages")).toHaveTextContent("帮我看下这个报错"));
    expect(argsOf("get_master_session_messages")).toEqual({ sessionId: "s1", libraryId: "master" });
    const trace = screen.getByTestId("library-message-m2");
    expect(trace).toHaveTextContent("任务轨迹 · 2 步"); expect(trace).toHaveTextContent("先定位"); expect(trace).toHaveTextContent("再修复");
    fireEvent.click(screen.getByTestId("library-viewer-close"));
    expect(screen.getByTestId("library-viewer-empty")).toBeInTheDocument();
  });
  it("选择模式与批量栏：勾项目连带会话 + 摘要计数 + 只读提示 + Esc 退出恢复查看器", async () => {
    traeRoutes(); renderTrae();
    await expand("p1");
    fireEvent.click(screen.getByTestId("library-select-mode"));
    fireEvent.click(screen.getByTestId("library-project-p1"));
    expect(screen.getByTestId("library-project-p1")).toHaveClass("lib-project--checked"); expect(screen.getByTestId("library-session-s1")).toHaveClass("lib-session--checked");
    expect(screen.getByTestId("batch-count")).toHaveTextContent("已选 2 个会话 · 1 个项目"); expect(screen.getByTestId("batch-archive")).not.toBeDisabled();
    expect(screen.getByTestId("library-select-hint")).toHaveTextContent("选择模式：预览只读");
    fireEvent.keyDown(window, { key: "Escape" });
    await waitFor(() => expect(screen.queryByTestId("library-action-bar")).not.toBeInTheDocument());
    fireEvent.click(screen.getByTestId("library-session-s1"));
    expect(await screen.findByTestId("library-viewer")).toBeInTheDocument();
  });
  it("删除确认（旧锚 delete-confirm）：列明规模，取消零命令、确认后真删并清选择", async () => {
    traeRoutes(); renderTrae();
    await screen.findByTestId("library-project-p1");
    fireEvent.click(screen.getByTestId("library-select-mode"));
    fireEvent.click(screen.getByTestId("library-project-p1"));
    fireEvent.click(screen.getByTestId("batch-delete"));
    const dialog = await screen.findByTestId("delete-confirm");
    expect(dialog).toHaveTextContent("删除 2 个会话"); expect(dialog).toHaveTextContent("共 13 条消息"); expect(dialog).toHaveTextContent("删除前会自动备份");
    fireEvent.click(screen.getByTestId("delete-confirm-cancel"));
    expect(called("delete_master_sessions")).toBe(false);
    fireEvent.click(screen.getByTestId("batch-delete"));
    fireEvent.click(await screen.findByTestId("delete-confirm-confirm"));
    await waitFor(() => expect(argsOf("delete_master_sessions")).toEqual({ sessionIds: ["s1", "s2"], libraryId: "master" }));
    await waitFor(() => expect(notifyMock.success).toHaveBeenCalledWith("已删除 2 个会话。"));
    await waitFor(() => expect(screen.queryByTestId("library-action-bar")).not.toBeInTheDocument());
  });
  it("归档入口与归档视图：已归档 N 进模式层（Work 模式 → 项目 → 会话），返回恢复主树", async () => {
    traeRoutes(); renderTrae();
    expect(await screen.findByTestId("library-archive-entry")).toHaveTextContent("已归档");
    fireEvent.click(screen.getByTestId("library-archive-entry"));
    const tree = await screen.findByTestId("library-tree");
    expect(screen.getByTestId("library-archive-back")).toBeInTheDocument(); expect(tree).toHaveTextContent("Work 模式");
    expect(screen.getByTestId("library-session-s3")).toHaveTextContent("旧归档会话");
    fireEvent.click(screen.getByTestId("library-archive-back"));
    await expand("p2"); expect(screen.getByTestId("library-session-s4")).toBeInTheDocument();
  });
  it("快捷归档只产草稿（不进选择模式），可放弃且不写主库", async () => {
    traeRoutes(); renderTrae();
    await expand("p1");
    fireEvent.click(screen.getByTestId("library-quick-archive-s2"));
    expect(screen.getByTestId("archive-draft-bar")).toHaveTextContent("归档 1 个"); expect(called("apply_master_archive_changes")).toBe(false);
    expect(screen.queryByTestId("library-action-bar")).not.toBeInTheDocument();
    fireEvent.click(screen.getByTestId("archive-draft-discard"));
    await waitFor(() => expect(screen.queryByTestId("archive-draft-bar")).not.toBeInTheDocument());
    expect(notifyMock.info).toHaveBeenCalledWith("已放弃未提交的归档设置，主库没有变化。");
  });
  it("草稿应用：Work 库先出关闭客户端确认（work-archive-confirm），确认后一次性提交", async () => {
    traeRoutes(); renderTrae();
    await expand("p1");
    fireEvent.click(screen.getByTestId("library-quick-archive-s2"));
    fireEvent.click(screen.getByTestId("archive-draft-apply"));
    expect(await screen.findByTestId("work-archive-confirm")).toHaveTextContent("应用 Work 归档设置"); expect(called("apply_master_archive_changes")).toBe(false);
    fireEvent.click(screen.getByTestId("work-archive-confirm-confirm"));
    await waitFor(() => expect(argsOf("apply_master_archive_changes")).toEqual({ archiveSessionIds: ["s2"], restoreSessionIds: [], libraryId: "master" }));
  });
  it("归并入口：勾两个具名项目 → 合并确认（默认保留第一个）→ 调用项目归并命令", async () => {
    traeRoutes(); renderTrae();
    await screen.findByTestId("library-project-p1");
    fireEvent.click(screen.getByTestId("library-select-mode"));
    fireEvent.click(screen.getByTestId("library-project-p1"));
    fireEvent.click(screen.getByTestId("library-project-p2"));
    expect(screen.getByTestId("batch-count")).toHaveTextContent("已选 4 个会话 · 2 个项目");
    fireEvent.click(screen.getByTestId("batch-merge"));
    expect(await screen.findByTestId("merge-confirm")).toHaveTextContent("合并 2 个项目"); expect(screen.getByTestId("merge-target-p1")).toBeChecked();
    fireEvent.click(screen.getByTestId("merge-confirm-confirm"));
    await waitFor(() => expect(argsOf("merge_master_projects")).toEqual({ sourceProjectIds: ["p2"], targetProjectId: "p1", libraryId: "master" }));
  });
  it("插件 tab 门禁：只读库给说明卡，零插件命令（工作台不挂载，浏览不受影响）", async () => {
    traeRoutes(); renderTrae("trae_cn");
    expect(await screen.findByTestId("library-project-p1")).toBeInTheDocument(); expect(screen.getByTestId("library-select-mode")).toBeInTheDocument();
    fireEvent.click(screen.getByTestId("master-tab-plugins"));
    expect(screen.getByTestId("master-plugins-readonly-note")).toHaveTextContent("Trae CN 为只读浏览"); expect(screen.queryByTestId("plugin-workbench")).not.toBeInTheDocument();
    expect(called("get_plugin_tab_state")).toBe(false); expect(called("browse_plugin_market")).toBe(false);
  });
  it("插件工作台：已装行徽章（切号保留/内置）+ 内置移除禁用 + 卸载单次确认走全账号传播", async () => {
    traeRoutes(); renderTrae();
    await screen.findByTestId("library-project-p1");
    fireEvent.click(screen.getByTestId("master-tab-plugins"));
    expect(await screen.findByTestId("plugin-row-rec-1")).toHaveTextContent("切号保留"); expect(screen.getByTestId("plugin-row-rec-2")).toHaveTextContent("内置");
    expect(screen.getByTestId("plugin-uninstall-rec-2")).toBeDisabled();
    fireEvent.click(screen.getByTestId("plugin-uninstall-rec-1"));
    const dialog = await screen.findByTestId("plugin-uninstall-confirm");
    expect(dialog).toHaveTextContent("将同时从 2 个账号移除");
    fireEvent.click(screen.getByTestId("plugin-uninstall-confirm-confirm"));
    await waitFor(() => expect(argsOf("uninstall_plugin_everywhere")).toEqual({ recordId: "rec-1" }));
  });
  it("插件市场段：切段后搜索框出现并按名称过滤；安装零确认直接调 install_plugin", async () => {
    traeRoutes(); renderTrae();
    await screen.findByTestId("library-project-p1");
    fireEvent.click(screen.getByTestId("master-tab-plugins"));
    fireEvent.click(await screen.findByTestId("plugin-segment-market"));
    const search = await screen.findByTestId("plugin-market-search");
    fireEvent.change(search, { target: { value: "插件二" } });
    expect(screen.queryByTestId("plugin-market-row-uuid-p1")).not.toBeInTheDocument();
    fireEvent.click(screen.getByTestId("plugin-install-uuid-p2"));
    await waitFor(() => expect(argsOf("install_plugin")).toEqual({ pluginId: "uuid-p2", name: "plugin-two", displayName: "插件二", registry: "trae-remote-official" }));
    expect(notifyMock.success).toHaveBeenCalledWith("已安装 插件二。");
  });
  it("读取失败错误卡（旧锚 history-error）", async () => {
    traeRoutes(() => { throw new Error("boom"); }); renderTrae();
    expect(await screen.findByTestId("history-error")).toHaveTextContent("库记录暂时不可读取");
  });
  it("空态：no_master_data 给 history-guide 与启动引导，引导动作原样透出", async () => {
    traeRoutes(() => ({ ...HISTORY, status: "no_master_data", current_user_id: null, projects: [], sessions: [] }));
    const onAction = vi.fn();
    renderTrae("master", onAction);
    expect(await screen.findByTestId("history-guide")).toHaveTextContent("主库还没有对话数据。");
    fireEvent.click(screen.getByTestId("history-guide-launch"));
    expect(onAction).toHaveBeenCalledWith("empty-open-environment");
  });
});
describe("库页（WorkBuddy）：分组树 / 统计 / 过滤 / 两条删除链 / 体检与归并", () => {
  it("分组树 + 统计卡五项 + 体检卡五项（含归并候选入口与立即体检）", async () => {
    wbRoutes(); renderWb();
    expect(await screen.findByTestId("workbuddy-library-stats")).toHaveTextContent("软删除"); expect(screen.getByTestId("workbuddy-library-groups")).toBeInTheDocument();
    expect(screen.getByText("D:\\work\\proj-a")).toBeInTheDocument();
    const checkup = await screen.findByTestId("workbuddy-library-checkup");
    for (const label of ["孤儿正文", "缺失正文", "软删除规模", "外部目录", "归并候选"]) expect(checkup).toHaveTextContent(label);
    expect(checkup).toHaveTextContent("1 份（正文有、索引无）"); expect(screen.getByTestId("workbuddy-library-merge-2")).toBeInTheDocument();
    expect(screen.getByTestId("workbuddy-library-checkup-button")).toBeInTheDocument();
  });
  it("账号过滤：切换后按 account_id 重读分组；未过滤时删除按钮禁用", async () => {
    wbRoutes(); renderWb();
    await screen.findByTestId("workbuddy-library-groups");
    expect(screen.getByTestId("workbuddy-library-soft-delete")).toBeDisabled(); expect(screen.getByTestId("workbuddy-library-purge")).toBeDisabled();
    expect(screen.getByTestId("workbuddy-library-soft-delete")).toHaveAttribute("title", "先按账号过滤，再执行删除（归属校验需要账号范围）");
    await filterTo("uid-1");
    expect(screen.getByTestId("workbuddy-library-soft-delete")).toHaveTextContent("软删除（0）");
  });
  it("软删除：独立确认层 workbuddy-library-soft-delete-confirm → delete_workbuddy_sessions", async () => {
    wbRoutes(); renderWb();
    await screen.findByTestId("workbuddy-library-groups");
    await filterTo("uid-1");
    fireEvent.click(screen.getByTestId("workbuddy-library-select-s-1"));
    fireEvent.click(screen.getByTestId("workbuddy-library-soft-delete"));
    expect(await screen.findByTestId("workbuddy-library-soft-delete-confirm")).toHaveTextContent("可恢复");
    fireEvent.click(screen.getByTestId("workbuddy-library-soft-delete-confirm-confirm"));
    await waitFor(() => expect(argsOf("delete_workbuddy_sessions")).toEqual({ sessionIds: ["s-1"], userId: "uid-1" }));
    expect(called("purge_workbuddy_sessions")).toBe(false);
  });
  it("彻底删除：另一层独立确认（决策 7）并列明滚动备份前置 → purge_workbuddy_sessions", async () => {
    wbRoutes(); renderWb();
    await screen.findByTestId("workbuddy-library-groups");
    await filterTo("uid-1");
    fireEvent.click(screen.getByTestId("workbuddy-library-select-s-1"));
    fireEvent.click(screen.getByTestId("workbuddy-library-purge"));
    const dialog = await screen.findByTestId("workbuddy-library-purge-confirm");
    expect(dialog).toHaveTextContent("滚动备份"); expect(dialog).toHaveTextContent("备份失败则整单拒绝"); expect(screen.queryByTestId("workbuddy-library-soft-delete-confirm")).not.toBeInTheDocument();
    fireEvent.click(screen.getByTestId("workbuddy-library-purge-confirm-confirm"));
    await waitFor(() => expect(argsOf("purge_workbuddy_sessions")).toEqual({ sessionIds: ["s-1"], userId: "uid-1" }));
    expect(called("delete_workbuddy_sessions")).toBe(false);
  });
  it("cwd 归并入口：候选组按钮 → 确认层（canonical 吸收变体）→ merge_workbuddy_cwd_projects", async () => {
    wbRoutes(); renderWb();
    fireEvent.click(await screen.findByTestId("workbuddy-library-merge-2"));
    expect(await screen.findByTestId("workbuddy-library-merge-confirm")).toHaveTextContent("d--work-proj-a-v2");
    fireEvent.click(screen.getByTestId("workbuddy-library-merge-confirm-confirm"));
    await waitFor(() => expect(argsOf("merge_workbuddy_cwd_projects")).toEqual({ canonicalCwd: "d--work-proj-a", variantCwds: ["d--work-proj-a-v2"] }));
  });
  it("会话消息：点本地行出行内时间线；cloud 行只读不可选", async () => {
    wbRoutes(); renderWb();
    await screen.findByTestId("workbuddy-library-groups");
    expect(screen.getByTestId("workbuddy-library-select-s-2")).toBeDisabled(); expect(screen.getByText("云端")).toBeInTheDocument();
    fireEvent.click(screen.getByText("会话一"));
    expect(await screen.findByTestId("library-viewer")).toHaveTextContent("你好，有什么可以帮你？");
    await waitFor(() => expect(argsOf("get_workbuddy_session_messages")).toEqual({ sessionId: "s-1", userId: null }));
  });
  it("读取失败错误卡（旧页无锚，按文案断言）", async () => {
    wbRoutes(() => { throw new Error("boom"); }); renderWb();
    expect(await screen.findByText("无法读取对话库")).toBeInTheDocument(); expect(screen.getByText("对话库暂不可读取，请稍后重试。")).toBeInTheDocument();
  });
  it("空态：分组为空给一句「库中无会话」，统计与体检卡仍在册", async () => {
    wbRoutes(() => []); renderWb();
    expect(await screen.findByTestId("workbuddy-library-groups")).toBeInTheDocument(); expect(screen.getByText("库中无会话（或当前过滤无匹配）。")).toBeInTheDocument();
    expect(screen.getByTestId("workbuddy-library-stats")).toBeInTheDocument(); expect(screen.getByTestId("workbuddy-library-checkup")).toBeInTheDocument();
  });
});