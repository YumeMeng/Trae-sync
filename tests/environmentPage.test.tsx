import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { EnvironmentPage } from "../src/features/environment/EnvironmentPage";
import { installFakeInvoke, mockRoute, resetRoutes } from "./helpers/fakeInvoke";
/**
 * 环境页测试（P4d-8b-2a）：旧 tests/EnvironmentPage.test.tsx（38 例）+ WorkBuddyPages.test.tsx 的
 * 2 例 WB 环境用例迁到合并页 features/environment/EnvironmentPage（TRAE / WB 共用骨架）；
 * 断言只经 data-testid 与用户可见文案，锚点仍是同一批 env-* / workbuddy-env-*。
 */
vi.mock("@tauri-apps/api/core", async () => {
  const fake = await import("./helpers/fakeInvoke");
  return { invoke: fake.mockCoreInvoke };
});
// 事件插件：收编进度用例要拿到页面注册的回调（useTauriEvents 内层解包 payload，故存包装函数）。
const { mockListen, listenCalls } = vi.hoisted(() => {
  const calls: { name: string; handler: (event: { payload: unknown }) => void }[] = [];
  const mockListen = vi.fn(async (name: string, handler: (event: { payload: unknown }) => void) => {
    calls.push({ name, handler });
    return () => undefined;
  });
  return { mockListen, listenCalls: calls };
});
vi.mock("@tauri-apps/api/event", () => ({ listen: mockListen }));
// 操作结果走 notify 适配层（P11-1 切片②）：断言调用而非页内文本。
vi.mock("../src/platform/notify", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../src/platform/notify")>();
  return { ...actual, notify: { success: vi.fn(), error: vi.fn(), info: vi.fn(), warning: vi.fn(), summary: vi.fn() } };
});
import { notify } from "../src/platform/notify";
const mockInvoke = vi.mocked(invoke), notifyMock = vi.mocked(notify);
// ===== fixtures（与后端 DTO 同形） =====
const state = { env_id: "master", current_profile_id: "profile-a", current_account_name: "账号甲", observed_profile_id: "profile-a", data_dir: "C:\\data\\master", running: true, login_state: "logged_in", created_at_unix_seconds: 1750000000 };
const master = { env_id: "master", name: "主库", is_master: true, current_profile_id: "profile-a", current_account_name: "账号甲", data_dir: "C:\\data\\master", running: true, login_state: "logged_in", created_at_unix_seconds: 1750000000, size_bytes: 1048576 };
const secondary = { env_id: "env-x1", name: "测试环境", is_master: false, current_profile_id: "profile-b", current_account_name: "账号乙", data_dir: "D:\\store\\environments\\env-x1", running: false, login_state: "logged_in", created_at_unix_seconds: 1750000100, size_bytes: 2097152 };
const selfCheck = { level: "self_healable", can_repair: true, current_account_name: "账号甲", observed_account_name: "账号甲", checks: [
  { key: "read", status: "passed", summary: "登录数据可读取" }, { key: "consistency", status: "attention", summary: "检测到实际登录账号与工具记录不一致" },
  { key: "switchability", status: "passed", summary: "主库已关闭，可进行切换" }, { key: "deep", status: "passed", summary: "接力记录可读取" }] };
/** 体检：2 个滞留账号（5 会话 / 3 项目）+ 当前账号名下 1 个空项目 + 2 条无归属。 */
const staleReport = { status: "ready", current_account_name: "账号甲", orphan_project_count: 2, orphan_session_count: 0, accounts: [
  { user_id: "u-a", account_name: "账号甲", registered: true, current: true, project_count: 1, session_count: 1, empty_project_count: 1 },
  { user_id: "u-b", account_name: "账号乙", registered: true, current: false, project_count: 2, session_count: 3, empty_project_count: 0 },
  { user_id: "u-c", account_name: null, registered: false, current: false, project_count: 1, session_count: 2, empty_project_count: 0 }] };
/** 无滞留、无空项目：库内容分组翻为正常态提示。 */
const cleanReport = { ...staleReport, orphan_project_count: 0, accounts: [{ ...staleReport.accounts[0], empty_project_count: 0 }] };
/** 空项目全部挂在非当前账号名下：空壳行单独报告，无滞留 → 无归入入口。 */
const emptyShellReport = { ...staleReport, orphan_project_count: 0, accounts: [cleanReport.accounts[0],
  { user_id: "u-e", account_name: "空项目账号", registered: true, current: false, project_count: 0, session_count: 0, empty_project_count: 2 }] };
const accountView = { saved_accounts: [{ profile_id: "profile-a", display_name: "账号甲", region: null, data_location_id: "loc", last_verified_at: null, verification_state: "verified" }] };
const receipt = { merged_accounts: 2, transferred_projects: 3, removed_mirror_rows: 1, switched_sessions: 5, backup_path: "C:\\bak\\b1", relay_ledger_written: true, relaunch_outcome: "launched" };
const wbSession = { status: "active", uid: "uid-1", uin: "uid-1", domain: "workbuddy.example", access_expires_at_unix_seconds: 1799999999, refresh_expires_at_unix_seconds: 1799999999, captured_in_pool: true };
const wbProbe = { running: true, process_count: 1, app: "WorkBuddy", version: "3.2.0", platform: "win32", install_path: "C:\\apps\\workbuddy", install_source: "registry", data_root: "C:\\Users\\x\\.workbuddy" };
const wbOfflineProbe = { running: false, process_count: null, app: null, version: null, platform: null, install_path: null, install_source: null, data_root: null };
function baseRoutes(report: unknown = staleReport) {
  mockRoute("get_environment_state", () => state); mockRoute("list_environments", () => [master]);
  mockRoute("get_master_checkup", () => report); mockRoute("get_master_self_check", () => selfCheck);
  mockRoute("repair_master_current_account", () => true);
}
beforeEach(() => { vi.clearAllMocks(); listenCalls.length = 0; resetRoutes(); installFakeInvoke(); baseRoutes(); });
async function mountTrae(props: Record<string, unknown> = {}) {
  render(<EnvironmentPage active productId="trae_cn" {...props} />); await screen.findByTestId("env-master-card");
}
async function mountWorkbuddy() {
  render(<EnvironmentPage active productId="workbuddy" />); await screen.findByTestId("workbuddy-env-client-card");
}
/** 整组重读（refreshFull）计数：list_environments 带 includeSize=true 的调用次数。 */
const fullListCalls = () => mockInvoke.mock.calls.filter((call) => call[0] === "list_environments" && (call[1] as { includeSize?: boolean } | undefined)?.includeSize === true).length;
describe("TRAE 环境页：渲染", () => {
  it("主库卡：当前账号 / 默认环境 / 目录悬浮 / 运行态两按钮 / 无统计 / 空态入口 / 详情回调", async () => {
    const onOpenMasterDetail = vi.fn();
    await mountTrae({ onOpenMasterDetail });
    expect(screen.getByTestId("env-current-name")).toHaveTextContent("账号甲"); expect(screen.getByText("默认环境")).toBeInTheDocument();
    expect(screen.getByTestId("env-data-dir")).toHaveAttribute("title", "主库数据目录：C:\\data\\master"); expect(screen.getByTestId("env-data-dir")).toHaveTextContent("数据目录"); expect(screen.getByTestId("env-master-state")).toHaveTextContent("运行中");
    expect(screen.getByTestId("env-master-launch")).toHaveTextContent("聚焦主库"); expect(screen.getByTestId("env-master-close")).toBeInTheDocument();
    expect(screen.getByText(/全部对话记录都在主库内共享/)).toBeInTheDocument();
    // G5：主库卡不渲染统计，也不读统计命令（统计职责留在详情页）。
    expect(screen.queryByTestId("env-master-stats")).not.toBeInTheDocument(); expect(mockInvoke).not.toHaveBeenCalledWith("get_master_library_stats");
    expect(screen.getByTestId("env-placeholder")).toHaveTextContent("还没有辅助环境"); expect(screen.getByTestId("env-create-entry")).toBeInTheDocument();
    fireEvent.click(screen.getByTestId("env-master-detail")); expect(onOpenMasterDetail).toHaveBeenCalledTimes(1);
  });
  it("未登录主库：首次登录引导替代账号块，启动按钮为「启动主库」", async () => {
    mockRoute("get_environment_state", () => ({ ...state, current_profile_id: null, current_account_name: null, running: false, login_state: "uninitialized" }));
    await mountTrae(); expect(screen.getByText(/首次启动后请在 TRAE 窗口内登录一次/)).toBeInTheDocument();
    expect(screen.queryByTestId("env-current-name")).not.toBeInTheDocument(); expect(screen.getByTestId("env-master-launch")).toHaveTextContent("启动主库");
  });
  it("进程状态未知：显示状态未知，启停与副环境登录/删除一律停用", async () => {
    mockRoute("get_environment_state", () => ({ ...state, running: false, process_state: "unknown" }));
    mockRoute("list_environments", () => [master, { ...secondary, process_state: "unknown" }]);
    await mountTrae(); expect(screen.getByTestId("env-master-state")).toHaveTextContent("状态未知"); expect(screen.getByTestId("env-master-launch")).toBeDisabled();
    expect(screen.queryByTestId("env-master-close")).not.toBeInTheDocument();
    for (const id of ["env-login-env-x1", "env-delete-env-x1", "env-launch-env-x1"]) expect(screen.getByTestId(id)).toBeDisabled();
    expect(screen.queryByTestId("env-close-env-x1")).not.toBeInTheDocument();
  });
  it("副环境卡：名称 / 辅助环境 / 当前账号 / 体积 / 路径悬浮 / 五项操作 / 详情回调", async () => {
    const onOpenEnvDetail = vi.fn();
    mockRoute("list_environments", () => [master, secondary]);
    mockRoute("get_master_checkup", (args) => (args as { libraryId?: string } | undefined)?.libraryId === "env-x1" ? cleanReport : staleReport);
    await mountTrae({ onOpenLibraryDetail: () => undefined, onOpenEnvDetail });
    const card = screen.getByTestId("env-card-env-x1");
    expect(card).toHaveTextContent("测试环境"); expect(card).toHaveTextContent("辅助环境");
    expect(card).toHaveTextContent("账号乙"); expect(card).toHaveTextContent("2.0 MB");
    expect(screen.getByTestId("env-path-env-x1")).toHaveAttribute("title", "数据目录：D:\\store\\environments\\env-x1");
    for (const id of ["env-login-env-x1", "env-rename-env-x1", "env-delete-env-x1", "env-launch-env-x1", "env-detail-env-x1"]) expect(screen.getByTestId(id)).toBeInTheDocument();
    expect(screen.getByTestId("env-launch-env-x1")).toHaveTextContent("启动"); expect(screen.queryByTestId("env-placeholder")).not.toBeInTheDocument();
    expect(screen.queryByTestId("env-checkup-env-x1")).not.toBeInTheDocument(); // 无差异不显示体检区块
    fireEvent.click(screen.getByTestId("env-detail-env-x1")); expect(onOpenEnvDetail).toHaveBeenCalledWith("env-x1");
  });
  it("体检卡：登录健康四项 + 库内容滞留/空项目/无归属句 + 归入与清理入口 + 纠正", async () => {
    await mountTrae();
    const checkup = await screen.findByTestId("env-checkup");
    expect(checkup).toHaveTextContent("主库体检"); expect(checkup).toHaveTextContent("登录健康：可自愈");
    for (const name of ["读校验", "一致性", "可切换", "深度检查"]) expect(checkup).toHaveTextContent(name);
    const content = screen.getByTestId("env-checkup-content");
    expect(content).toHaveTextContent("有 2 个账号的 5 个会话在主库中，未随当前账号展示");
    expect(content).toHaveTextContent("另有 1 个空项目记录未进入归入范围"); expect(content).toHaveTextContent("3 个会话 · 2 个项目"); expect(content).toHaveTextContent("未登记账号");
    expect(content).not.toHaveTextContent("u-b"); expect(content).toHaveTextContent("另有 2 条无归属记录不会被归入。");
    // 空项目只挂在当前账号名下：不渲染空壳清单，但清理入口按全库口径仍出现。
    expect(screen.queryByTestId("env-empty-shell-list")).not.toBeInTheDocument();
    expect(screen.getByTestId("env-cleanup-empty-projects-entry")).toBeInTheDocument(); expect(screen.getByTestId("env-incorporate-entry")).toBeInTheDocument();
    fireEvent.click(screen.getByTestId("env-self-check-repair"));
    await waitFor(() => expect(mockInvoke).toHaveBeenCalledWith("repair_master_current_account"));
  });
  it("体检降级：自检失败只降级登录健康分组；两组都不可读时区块整体静默", async () => {
    baseRoutes(cleanReport); mockRoute("get_master_self_check", () => { throw new Error("trae_real_mode_required"); });
    const first = render(<EnvironmentPage active productId="trae_cn" />);
    await screen.findByTestId("env-checkup");
    expect(screen.getByTestId("env-checkup-login")).toHaveTextContent("登录健康暂时无法读取。");
    expect(screen.getByTestId("env-checkup-content-ok")).toHaveTextContent("暂未发现滞留记录，账号记录分布正常。");
    first.unmount();
    mockRoute("get_master_checkup", () => { throw new Error("checkup_read_failed"); });
    await mountTrae();
    expect(screen.queryByTestId("env-checkup")).not.toBeInTheDocument(); expect(screen.getByTestId("env-current-name")).toHaveTextContent("账号甲");
  });
});
describe("TRAE 环境页：启停与生命周期", () => {
  it("启动主库：调用 launch_master_library 并轻刷新运行态", async () => {
    mockRoute("launch_master_library", () => ({ outcome: "launched", login_state: "logged_in" }));
    await mountTrae();
    fireEvent.click(screen.getByTestId("env-master-launch"));
    await waitFor(() => expect(mockInvoke).toHaveBeenCalledWith("launch_master_library"));
    await waitFor(() => expect(mockInvoke.mock.calls.filter((call) => call[0] === "get_environment_state").length).toBeGreaterThanOrEqual(2));
  });
  it("关闭主库：取消零命令，确认后才调用并整组重读", async () => {
    mockRoute("close_environment_instance", () => ({ outcome: "closed" }));
    await mountTrae();
    fireEvent.click(screen.getByTestId("env-master-close"));
    expect(await screen.findByTestId("env-close-dialog")).toHaveTextContent("将关闭「主库」正在运行的 TRAE 实例。");
    fireEvent.click(screen.getByTestId("env-close-dialog-cancel"));
    expect(screen.queryByTestId("env-close-dialog")).not.toBeInTheDocument(); expect(mockInvoke).not.toHaveBeenCalledWith("close_environment_instance", expect.anything());
    fireEvent.click(screen.getByTestId("env-master-close")); fireEvent.click(await screen.findByTestId("env-close-dialog-confirm"));
    await waitFor(() => expect(mockInvoke).toHaveBeenCalledWith("close_environment_instance", { envId: "master" }));
    await waitFor(() => expect(notifyMock.success).toHaveBeenCalledWith("已关闭「主库」的 TRAE 实例。"));
    await waitFor(() => expect(fullListCalls()).toBeGreaterThanOrEqual(2));
  });
  it("创建环境：空名禁用确认，输入后调用 create_environment 并关层", async () => {
    mockRoute("create_environment", (args) => ({ env_id: "env-x2", name: (args as { name: string }).name, created_at_unix_seconds: 1750000200 }));
    await mountTrae();
    fireEvent.click(screen.getByTestId("env-create-entry")); await screen.findByTestId("env-name-dialog");
    expect(screen.getByTestId("env-name-confirm")).toBeDisabled();
    fireEvent.change(screen.getByTestId("env-name-input"), { target: { value: "新环境" } });
    expect(screen.getByTestId("env-name-confirm")).toBeEnabled();
    fireEvent.click(screen.getByTestId("env-name-confirm"));
    await waitFor(() => expect(mockInvoke).toHaveBeenCalledWith("create_environment", { name: "新环境" }));
    await waitFor(() => expect(screen.queryByTestId("env-name-dialog")).not.toBeInTheDocument());
    expect(notifyMock.success).toHaveBeenCalledWith("已创建环境「新环境」。");
  });
  it("重命名环境：预填原名，保存调用 rename_environment", async () => {
    mockRoute("list_environments", () => [master, secondary]);
    mockRoute("rename_environment", () => ({ env_id: "env-x1", name: "新名字", created_at_unix_seconds: 1750000100 }));
    await mountTrae();
    fireEvent.click(screen.getByTestId("env-rename-env-x1")); expect(await screen.findByTestId("env-name-input")).toHaveValue("测试环境");
    fireEvent.change(screen.getByTestId("env-name-input"), { target: { value: "新名字" } });
    fireEvent.click(screen.getByTestId("env-name-confirm"));
    await waitFor(() => expect(mockInvoke).toHaveBeenCalledWith("rename_environment", { envId: "env-x1", name: "新名字" }));
    await waitFor(() => expect(screen.queryByTestId("env-name-dialog")).not.toBeInTheDocument());
  });
  it("删除环境：预览未到禁用确认，规模到达后确认才调用 delete_environment", async () => {
    let resolvePreview: ((value: unknown) => void) | null = null;
    mockRoute("list_environments", () => [master, secondary]);
    mockRoute("get_environment_delete_preview", () => new Promise((resolve) => { resolvePreview = resolve; }));
    mockRoute("delete_environment", () => true);
    await mountTrae();
    fireEvent.click(screen.getByTestId("env-delete-env-x1"));
    const dialog = await screen.findByTestId("env-delete-dialog");
    expect(dialog).toHaveTextContent("正在读取环境数据规模…"); expect(screen.getByTestId("env-delete-dialog-confirm")).toBeDisabled();
    await act(async () => { resolvePreview?.({ status: "ready", project_count: 3, session_count: 5, size_bytes: 2097152 }); });
    expect(dialog).toHaveTextContent("该环境内有 3 个项目、5 个会话（约 2.0 MB），删除后无法恢复。"); expect(screen.getByTestId("env-delete-dialog-confirm")).toBeEnabled();
    fireEvent.click(screen.getByTestId("env-delete-dialog-confirm"));
    await waitFor(() => expect(mockInvoke).toHaveBeenCalledWith("delete_environment", { envId: "env-x1" }));
    await waitFor(() => expect(notifyMock.success).toHaveBeenCalledWith("已删除环境「测试环境」。"));
  });
  it("删除预览失败/删除被拒：预览失败禁用确认零删除；运行中被拒显示映射文案", async () => {
    mockRoute("list_environments", () => [master, secondary]);
    mockRoute("get_environment_delete_preview", () => { throw new Error("environment_preview_unavailable"); });
    await mountTrae();
    fireEvent.click(screen.getByTestId("env-delete-env-x1"));
    const failed = await screen.findByTestId("env-delete-dialog");
    await waitFor(() => expect(failed).toHaveTextContent("环境信息暂时不可读取，请稍后重试。"));
    expect(screen.getByTestId("env-delete-dialog-confirm")).toBeDisabled(); expect(mockInvoke).not.toHaveBeenCalledWith("delete_environment", expect.anything());
    fireEvent.click(screen.getByTestId("env-delete-dialog-cancel"));
    mockRoute("get_environment_delete_preview", () => ({ status: "ready", project_count: 0, session_count: 0, size_bytes: 1024 }));
    mockRoute("delete_environment", () => { throw new Error("environment_delete_running"); });
    fireEvent.click(screen.getByTestId("env-delete-env-x1")); fireEvent.click(await screen.findByTestId("env-delete-dialog-confirm"));
    const rejected = await screen.findByTestId("env-delete-dialog");
    await waitFor(() => expect(rejected).toHaveTextContent("该环境正在运行，请先关闭它的窗口再删除。"));
    expect(rejected).not.toHaveTextContent("environment_delete_running");
  });
});
describe("TRAE 环境页：收编 / 清理 / 登录", () => {
  it("一键收编：确认层规模 → 确认发命令 → 进度阶段文案 → 完成回执并刷新", async () => {
    let resolveIncorporate: ((value: unknown) => void) | null = null;
    mockRoute("incorporate_master_records", () => new Promise((resolve) => { resolveIncorporate = resolve; }));
    await mountTrae();
    fireEvent.click(await screen.findByTestId("env-incorporate-entry"));
    const dialog = await screen.findByTestId("incorporate-dialog");
    expect(dialog).toHaveTextContent("将归入 2 个账号的记录"); expect(dialog).toHaveTextContent("共 5 个会话、3 个项目将归入账号甲；主库会自动关闭并重启");
    expect(dialog).toHaveTextContent("另有 2 条无归属记录不会被归入。"); expect(screen.getByTestId("incorporate-account-list")).toHaveTextContent("未登记账号");
    fireEvent.click(screen.getByTestId("incorporate-confirm"));
    await waitFor(() => expect(mockInvoke).toHaveBeenCalledWith("incorporate_master_records", {}));
    // 进度事件推进到「归入记录」：四阶段文案唯一来源是 builder 的 incorporateStageLabels。
    const progress = listenCalls.find((call) => call.name === "master-incorporate-progress")?.handler;
    await act(async () => { progress?.({ payload: { stage: "incorporating", env_id: "master" } }); });
    expect(dialog).toHaveTextContent("归入记录"); expect(dialog).toHaveTextContent("关闭主库");
    await act(async () => { resolveIncorporate?.(receipt); });
    await waitFor(() => expect(dialog).toHaveTextContent("已归入 2 个账号：保留项目 3 个 · 转移会话 5 个"));
    await waitFor(() => expect(fullListCalls()).toBeGreaterThanOrEqual(2));
    fireEvent.click(screen.getByTestId("incorporate-done")); expect(screen.queryByTestId("incorporate-dialog")).not.toBeInTheDocument();
  });
  it("副环境逐库收编：体检有差异才显示，确认后按 libraryId 发起并报重启失败", async () => {
    mockRoute("list_environments", () => [master, secondary]);
    mockRoute("get_master_checkup", (args) => (args as { libraryId?: string } | undefined)?.libraryId === "env-x1" ? staleReport : cleanReport);
    mockRoute("incorporate_master_records", () => ({ ...receipt, relaunch_outcome: "failed" }));
    await mountTrae();
    const checkup = await screen.findByTestId("env-checkup-env-x1");
    expect(checkup).toHaveTextContent("测试环境体检"); expect(screen.getByTestId("env-checkup-login-env-x1")).toHaveTextContent("当前登录：账号乙");
    fireEvent.click(screen.getByTestId("env-incorporate-env-x1"));
    expect(await screen.findByTestId("incorporate-dialog")).toHaveTextContent("「测试环境」会自动关闭并重启");
    fireEvent.click(screen.getByTestId("incorporate-confirm"));
    await waitFor(() => expect(mockInvoke).toHaveBeenCalledWith("incorporate_master_records", { libraryId: "env-x1" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("归入已完成，但环境未能自动启动");
  });
  it("清理空项目：先预览后确认，成功通知并整组重读（空壳行单独报告）", async () => {
    baseRoutes(emptyShellReport);
    mockRoute("cleanup_master_empty_projects", () => ({ deleted_projects: 2, backup_path: "", relaunch_outcome: "launched" }));
    await mountTrae();
    expect(screen.getByTestId("env-empty-shell-list")).toHaveTextContent("空项目账号"); expect(screen.queryByTestId("env-incorporate-entry")).not.toBeInTheDocument();
    fireEvent.click(screen.getByTestId("env-cleanup-empty-projects-entry"));
    const dialog = await screen.findByTestId("env-cleanup-empty-projects-dialog");
    expect(mockInvoke).not.toHaveBeenCalledWith("cleanup_master_empty_projects", expect.anything());
    expect(dialog).toHaveTextContent("将删除 2 个空项目记录，删除前会自动备份。");
    fireEvent.click(screen.getByTestId("env-cleanup-empty-projects-dialog-confirm"));
    await waitFor(() => expect(mockInvoke).toHaveBeenCalledWith("cleanup_master_empty_projects", {}));
    await waitFor(() => expect(notifyMock.success).toHaveBeenCalledWith("已删除 2 个空项目记录（已自动备份）。"));
    await waitFor(() => expect(fullListCalls()).toBeGreaterThanOrEqual(2));
  });
  // P4d-8b-2b（K-07 项 4 回归）：成功回执与刷新失败并存——回执不丢、卡片不被整页错误卡替代。
  it("收编成功但重读失败：完成回执与弹层保留，追加刷新失败提示，页面不翻错误卡", async () => {
    let mutationDone = false;
    mockRoute("get_environment_state", () => {
      if (mutationDone) throw new Error("environment_refresh_failed");
      return state;
    });
    mockRoute("incorporate_master_records", () => { mutationDone = true; return receipt; });
    await mountTrae();
    fireEvent.click(await screen.findByTestId("env-incorporate-entry"));
    fireEvent.click(await screen.findByTestId("incorporate-confirm"));
    const dialog = await screen.findByTestId("incorporate-dialog");
    // 回执照旧：完成句与「完成」按钮都在（旧页 L1717 的刷新失败只追加提示）。
    await waitFor(() => expect(dialog).toHaveTextContent("已归入 2 个账号：保留项目 3 个 · 转移会话 5 个"));
    expect(dialog).toHaveTextContent("归入已完成，但环境状态刷新失败，请手动刷新。");
    expect(dialog).not.toHaveTextContent("归入未完成");
    expect(screen.getByTestId("incorporate-done")).toBeInTheDocument();
    // 数据保留：主库卡仍在，不被整页错误卡替代。
    expect(screen.getByTestId("env-master-card")).toHaveTextContent("账号甲");
    expect(screen.queryByTestId("environment-load-error")).not.toBeInTheDocument();
    expect(notifyMock.error).not.toHaveBeenCalled();
    fireEvent.click(screen.getByTestId("incorporate-done"));
    expect(screen.queryByTestId("incorporate-dialog")).not.toBeInTheDocument();
  });
  it("清理成功但重读失败：成功回执追加刷新失败后缀，卡片与体检保留", async () => {
    let mutationDone = false;
    baseRoutes(emptyShellReport); // 先装基线路由，再覆盖读取路由（后注册者生效）
    mockRoute("get_environment_state", () => {
      if (mutationDone) throw new Error("environment_refresh_failed");
      return state;
    });
    mockRoute("cleanup_master_empty_projects", () => { mutationDone = true; return { deleted_projects: 2, backup_path: "", relaunch_outcome: "launched" }; });
    await mountTrae();
    fireEvent.click(screen.getByTestId("env-cleanup-empty-projects-entry"));
    fireEvent.click(await screen.findByTestId("env-cleanup-empty-projects-dialog-confirm"));
    // 回执文案逐字旧页：成功句 + 「环境状态刷新失败，请手动刷新。」后缀。
    await waitFor(() => expect(notifyMock.success).toHaveBeenCalledWith(
      "已删除 2 个空项目记录（已自动备份）。环境状态刷新失败，请手动刷新。"));
    expect(notifyMock.error).not.toHaveBeenCalled();
    expect(screen.getByTestId("env-master-card")).toBeInTheDocument();
    expect(screen.queryByTestId("environment-load-error")).not.toBeInTheDocument();
  });
  it("清理预览失败：报错且不开破坏性弹层、零删除", async () => {
    let checkupCalls = 0;
    mockRoute("get_master_checkup", () => { checkupCalls += 1; if (checkupCalls > 1) throw new Error("checkup_read_failed"); return staleReport; });
    await mountTrae(); fireEvent.click(screen.getByTestId("env-cleanup-empty-projects-entry"));
    await waitFor(() => expect(notifyMock.error).toHaveBeenCalledWith("环境操作未能完成，请稍后重试。"));
    expect(screen.queryByTestId("env-cleanup-empty-projects-dialog")).not.toBeInTheDocument(); expect(mockInvoke).not.toHaveBeenCalledWith("cleanup_master_empty_projects", expect.anything());
  });
  it("环境登录：登录成功不刷新，「完成」才关层并整组重读", async () => {
    mockRoute("list_environments", () => [master, secondary]); mockRoute("get_managed_account_state", () => accountView);
    mockRoute("login_environment", () => ({ env_id: "env-x1", profile_id: "profile-a", seeded: false, transferred_projects: 2, switched_sessions: 4, removed_mirror_rows: 0, current_profile_saved: true }));
    await mountTrae();
    fireEvent.click(screen.getByTestId("env-login-env-x1")); await screen.findByTestId("env-login-account-list");
    expect(screen.getByTestId("env-login-confirm")).toBeDisabled();
    fireEvent.click(screen.getByTestId("env-login-account-profile-a")); fireEvent.click(screen.getByTestId("env-login-confirm"));
    await waitFor(() => expect(mockInvoke).toHaveBeenCalledWith("login_environment", { envId: "env-x1", profileId: "profile-a" }));
    expect(await screen.findByTestId("env-login-receipt")).toHaveTextContent("已登录账号 账号甲；随行项目 2 个 · 会话 4 个");
    // 回执态：弹层仍在、尚未重读。
    expect(screen.getByTestId("env-login-dialog")).toBeInTheDocument(); expect(fullListCalls()).toBe(1);
    fireEvent.click(screen.getByTestId("env-login-done"));
    await waitFor(() => expect(screen.queryByTestId("env-login-dialog")).not.toBeInTheDocument());
    await waitFor(() => expect(fullListCalls()).toBeGreaterThanOrEqual(2));
  });
});
describe("环境页：读取态", () => {
  it("读取失败：错误卡如实提示并给重试，不伪装成空环境", async () => {
    mockRoute("get_environment_state", () => { throw new Error("environment_state_unavailable"); });
    render(<EnvironmentPage active productId="trae_cn" />);
    expect(await screen.findByTestId("environment-load-error")).toHaveTextContent("环境状态暂时不可读取，请稍后重试。");
    expect(screen.getByRole("alert")).toHaveTextContent("环境状态暂时不可读取"); expect(screen.queryByTestId("env-master-card")).not.toBeInTheDocument();
  });
  it("首读骨架：读取中只换正文状态行，页头标题不重建", () => {
    mockRoute("get_environment_state", () => new Promise(() => undefined));
    render(<EnvironmentPage active productId="trae_cn" />);
    expect(screen.getByTestId("environment-skeleton")).toHaveTextContent("正在读取环境状态…"); expect(screen.getByRole("heading", { level: 1, name: "环境" })).toBeInTheDocument();
  });
  it("active=false：不发起任何读取", () => {
    render(<EnvironmentPage active={false} productId="trae_cn" />); expect(mockInvoke).not.toHaveBeenCalled();
  });
});
describe("WorkBuddy 环境页（只读状态卡，无启停）", () => {
  it("两张卡：客户端连接（版本/运行态）+ 登录域健康（本地态/深度结果/只读检测）", async () => {
    mockRoute("get_workbuddy_session_health", () => wbSession);
    mockRoute("check_workbuddy_login_health", () => ({ status: "active", live_status: "verified" }));
    mockRoute("get_workbuddy_probe_info", () => wbProbe);
    await mountWorkbuddy();
    const client = screen.getByTestId("workbuddy-env-client-card");
    expect(client).toHaveTextContent("已登录（登录文件有效）"); expect(client).toHaveTextContent("3.2.0");
    expect(screen.getByTestId("workbuddy-env-client-runtime")).toHaveTextContent("运行中（凭据实时回读通过）");
    const health = screen.getByTestId("workbuddy-env-health-card");
    expect(health).toHaveTextContent("登录域健康"); expect(health).toHaveTextContent("正常");
    expect(screen.getByTestId("workbuddy-env-deep-result")).toHaveTextContent("已验证可用"); expect(screen.getByTestId("workbuddy-env-deep-health-button")).toBeEnabled();
    // 只读：无启停、无生命周期；多实例待实验票说明替代列表。
    expect(screen.queryByTestId("env-master-launch")).not.toBeInTheDocument(); expect(screen.queryByTestId("env-master-close")).not.toBeInTheDocument();
    expect(screen.getByTestId("workbuddy-env-multi-pending")).toBeInTheDocument(); expect(screen.getByTestId("env-create-entry")).toBeDisabled();
  });
  it("探针降级：客户端未运行 → 运行态与版本/路径如实显示未知", async () => {
    mockRoute("get_workbuddy_session_health", () => ({ ...wbSession, status: "access_expired", access_expires_at_unix_seconds: 0, refresh_expires_at_unix_seconds: 0, captured_in_pool: false }));
    mockRoute("check_workbuddy_login_health", () => ({ status: "access_expired", live_status: "client_offline" }));
    mockRoute("get_workbuddy_probe_info", () => wbOfflineProbe);
    await mountWorkbuddy();
    const client = screen.getByTestId("workbuddy-env-client-card");
    expect(client).toHaveTextContent("未运行"); expect(client).toHaveTextContent("未知");
    expect(screen.getByTestId("workbuddy-env-deep-result")).toHaveTextContent("客户端未运行，仅本地检测"); expect(screen.getByTestId("workbuddy-env-health-card")).toHaveTextContent("将过期");
  });
});
