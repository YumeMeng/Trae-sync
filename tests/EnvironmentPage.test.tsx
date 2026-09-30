import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { EnvironmentPage } from "../src/components/EnvironmentPage";
import type { EnvironmentListItemDto, EnvironmentStateDto } from "../src/types/environment";
import { installFakeInvoke, mockRoute, resetRoutes } from "./helpers/fakeInvoke";

// P13-5：共享 fakeInvoke 路由器（迁移模式见 tests/helpers/fakeInvoke.ts 头注释）。
vi.mock("@tauri-apps/api/core", async () => {
  const fake = await import("./helpers/fakeInvoke");
  return { invoke: fake.mockCoreInvoke };
});

// 事件插件 mock：捕获 listen 回调，供收编进度测试手动触发（模式同 CheckinPage.test）。
const { mockListen } = vi.hoisted(() => ({ mockListen: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({ listen: mockListen }));

// P11-1 切片②：操作结果类提示已迁到 notify 适配层；断言 notify 调用而非页内文本。
vi.mock("../src/platform/notify", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../src/platform/notify")>();
  return {
    ...actual,
    notify: {
      success: vi.fn(),
      error: vi.fn(),
      info: vi.fn(),
      warning: vi.fn(),
      summary: vi.fn(),
    },
  };
});

import { notify } from "../src/platform/notify";

const mockInvoke = vi.mocked(invoke);
const notifyMock = vi.mocked(notify);

function envState(overrides: Partial<EnvironmentStateDto> = {}): EnvironmentStateDto {
  return {
    env_id: "master",
    current_profile_id: "profile-a",
    current_account_name: "账号甲",
    data_dir: "C:\\TraeSync\\data\\environments\\master",
    running: true,
    login_state: "logged_in",
    created_at_unix_seconds: 1750000000,
    ...overrides,
  };
}

/** P6-4 环境列表项 fixture（默认主库置顶项）。 */
function envListItem(overrides: Partial<EnvironmentListItemDto> = {}): EnvironmentListItemDto {
  return {
    env_id: "master",
    name: "主库",
    is_master: true,
    current_profile_id: "profile-a",
    current_account_name: "账号甲",
    data_dir: "C:\\TraeSync\\data\\environments\\master",
    running: true,
    login_state: "logged_in",
    created_at_unix_seconds: 1750000000,
    size_bytes: 1048576,
    ...overrides,
  };
}

describe("EnvironmentPage（P5-2 环境页）", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    // 收编进度监听默认立即注册成功（unlisten 为空函数）。
    mockListen.mockResolvedValue(() => undefined);
    // 等价于原 mockInvoke.mockImplementation(...)：清历史 + 重装路由分发 + 默认路由。
    resetRoutes();
    installFakeInvoke();
    mockRoute("get_environment_state", () => envState());
    mockRoute("launch_master_library", () => ({ outcome: "launched", login_state: "logged_in" }));
    mockRoute("list_environments", () => [envListItem()]);
  });

  it("渲染主库卡：当前账号 + 运行态 + data_dir + 无辅助环境时空态提示", async () => {
    render(<EnvironmentPage active />);
    // 主库卡与当前账号
    expect(await screen.findByTestId("env-master-card")).toBeInTheDocument();
    expect(screen.getByTestId("env-current-name")).toHaveTextContent("账号甲");
    expect(screen.getByText("主库")).toBeInTheDocument();
    expect(screen.getByText("默认环境")).toBeInTheDocument();
    // data_dir 足迹：完整路径收进悬浮提示，主视野只显示目录标签
    expect(screen.getByTestId("env-data-dir")).toHaveAttribute(
      "title",
      "主库数据目录：C:\\TraeSync\\data\\environments\\master",
    );
    expect(screen.getByTestId("env-data-dir")).toHaveTextContent("数据目录");
    // P6-4：无辅助环境时空态提示 + 创建入口
    expect(screen.getByTestId("env-placeholder")).toHaveTextContent("还没有辅助环境");
    expect(screen.getByTestId("env-create-entry")).toBeInTheDocument();
    // 运行中 → 按钮为聚焦语义
    expect(screen.getByTestId("env-master-launch")).toHaveTextContent("聚焦主库");
  });

  it("未登录账号时提示首次登录引导，且不显示当前账号块", async () => {
    mockRoute("get_environment_state", () =>
      envState({ current_profile_id: null, current_account_name: null, running: false, login_state: "uninitialized" }));
    render(<EnvironmentPage active />);
    expect(await screen.findByText(/首次启动后请在 TRAE 窗口内登录一次/)).toBeInTheDocument();
    expect(screen.queryByTestId("env-current-name")).not.toBeInTheDocument();
    expect(screen.getByTestId("env-master-launch")).toHaveTextContent("启动主库");
  });

  it("点击启动主库：invoke launch_master_library 并刷新状态", async () => {
    render(<EnvironmentPage active />);
    await screen.findByTestId("env-master-card");
    fireEvent.click(screen.getByTestId("env-master-launch"));
    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith("launch_master_library");
    });
    // 启动后重读环境状态
    await waitFor(() => {
      expect(mockInvoke.mock.calls.filter(([command]) => command === "get_environment_state").length).toBeGreaterThanOrEqual(2);
    });
  });

  it("active=false 不发起任何读取", () => {
    render(<EnvironmentPage active={false} />);
    expect(mockInvoke).not.toHaveBeenCalled();
  });

  it("P5-8a-2：详情按钮回调 onOpenMasterDetail（进入主库详情页）", async () => {
    const onOpen = vi.fn();
    render(<EnvironmentPage active onOpenMasterDetail={onOpen} />);
    await screen.findByTestId("env-master-card");
    fireEvent.click(screen.getByTestId("env-master-detail"));
    expect(onOpen).toHaveBeenCalledTimes(1);
  });

  it("G5：环境页主库卡不再渲染统计数字，统计职责留在详情页", async () => {
    mockRoute("get_environment_state", () => envState());
    mockRoute("get_master_library_stats", () => ({
      status: "ready",
      current_user_id: "user-a",
      project_count: 4,
      session_count: 7,
      participating_account_count: 3,
      last_active_unix_seconds: null,
    }));
    render(<EnvironmentPage active />);
    await screen.findByTestId("env-master-card");
    expect(screen.queryByTestId("env-master-stats")).not.toBeInTheDocument();
    expect(mockInvoke).not.toHaveBeenCalledWith("get_master_library_stats");
  });

  it("P11-5：主库体检整合区块自动渲染登录健康四项，可纠正缓存", async () => {
    mockRoute("get_environment_state", () => envState());
    mockRoute("get_master_self_check", () => ({
      level: "self_healable",
      checks: [
        { key: "read", status: "passed", summary: "登录数据可读取" },
        { key: "consistency", status: "attention", summary: "检测到实际登录账号与工具记录不一致" },
        { key: "switchability", status: "passed", summary: "主库已关闭，可进行切换" },
        { key: "deep", status: "passed", summary: "接力记录可读取" },
      ],
      current_account_name: "账号甲",
      observed_account_name: "账号乙",
      can_repair: true,
    }));
    mockRoute("repair_master_current_account", () => true);
    render(<EnvironmentPage active />);

    // 自检报告随体检自动读取（原独立「自检」按钮已移除）。
    const report = await screen.findByTestId("env-checkup");
    expect(report).toHaveTextContent("主库体检");
    expect(report).toHaveTextContent("登录健康：可自愈");
    expect(report).toHaveTextContent("读校验");
    expect(report).toHaveTextContent("一致性");
    expect(report).toHaveTextContent("可切换");
    expect(report).toHaveTextContent("深度检查");
    // 纠正入口仍可用：纠正后整组重读（含自检报告）。
    fireEvent.click(screen.getByTestId("env-self-check-repair"));
    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith("repair_master_current_account");
    });
    await waitFor(() => {
      expect(
        mockInvoke.mock.calls.filter(([command]) => command === "get_master_self_check").length,
      ).toBeGreaterThanOrEqual(2);
    });
  });

  it("P11-5：独立「自检」入口已移除，报告读取失败时登录健康分组降级", async () => {
    mockRoute("get_environment_state", () => envState());
    mockRoute("get_master_checkup", () => ({
      status: "ready",
      current_account_name: "账号甲",
      accounts: [],
      orphan_project_count: 0,
      orphan_session_count: 0,
    }));
    // 自检读取失败（reject）→ 降级为 null，不阻塞库内容分组。
    mockRoute("get_master_self_check", () => {
      throw new Error("trae_real_mode_required");
    });
    render(<EnvironmentPage active />);
    await screen.findByTestId("env-checkup");
    expect(screen.queryByTestId("env-master-self-check")).not.toBeInTheDocument();
    expect(screen.getByTestId("env-checkup-login")).toHaveTextContent("登录健康暂时无法读取。");
    // 库内容分组不受登录健康失败影响。
    expect(screen.getByTestId("env-checkup-content")).toHaveTextContent(
      "暂未发现滞留记录，账号记录分布正常。",
    );
  });

  it("P5-4：主库统计读取失败时统计格整体不渲染（可插拔降级）", async () => {
    // beforeEach 默认 mock 未实现 get_master_library_stats → undefined / reject 之外的
    // 降级路径：显式 reject 覆盖为失败，统计格应静默消失而非报错。
    mockRoute("get_environment_state", () => envState());
    mockRoute("get_master_library_stats", () => {
      throw new Error("trae_real_mode_required");
    });
    render(<EnvironmentPage active />);
    await screen.findByTestId("env-master-card");
    expect(screen.queryByTestId("env-master-stats")).not.toBeInTheDocument();
    // 环境状态本体不受统计失败影响。
    expect(screen.getByTestId("env-current-name")).toHaveTextContent("账号甲");
  });

  // ===== P5-5 主库体检 + 一键收编 =====

  /** 体检报告 fixture：当前账号 + 两个滞留账号（一个未登记）+ 孤儿行。 */
  function checkupReport() {
    return {
      status: "ready",
      current_account_name: "账号甲",
      accounts: [
        { user_id: "user-a", account_name: "账号甲", registered: true, current: true, project_count: 3, session_count: 5, empty_project_count: 0 },
        { user_id: "user-b", account_name: "账号乙", registered: true, current: false, project_count: 2, session_count: 4, empty_project_count: 0 },
        { user_id: "user-c", account_name: null, registered: false, current: false, project_count: 1, session_count: 1, empty_project_count: 0 },
      ],
      orphan_project_count: 2,
      orphan_session_count: 0,
    };
  }

  it("P11-5：体检库内容分组——滞留账号诊断行 + 收编入口（数量表述 + 账号名主信息）", async () => {
    mockRoute("get_environment_state", () => envState());
    mockRoute("get_master_checkup", () => checkupReport());
    render(<EnvironmentPage active />);
    const checkup = await screen.findByTestId("env-checkup");
    // 规模主信息：2 个滞留账号、合计 5 个会话（4 + 1）。
    expect(checkup).toHaveTextContent("有 2 个账号的 5 个会话在主库中");
    // 账号行：账号名 + 会话/项目数量；未登记账号用自然表述（user_id 不进主视野）。
    expect(checkup).toHaveTextContent("账号乙");
    expect(checkup).toHaveTextContent("4 个会话 · 2 个项目");
    expect(checkup).toHaveTextContent("未登记账号");
    expect(checkup).not.toHaveTextContent("user-b");
    // 孤儿行只报告不收编。
    expect(checkup).toHaveTextContent("另有 2 条无归属记录不会被归入");
    // 收编入口存在。
    expect(screen.getByTestId("env-incorporate-entry")).toBeInTheDocument();
  });

  it("P11-5：库内容无滞留时显示正常态提示，读取失败且无自检报告时区块静默", async () => {
    // 场景 1：分布里只有当前账号（无滞留）→ 库内容分组显示正常态。
    mockRoute("get_environment_state", () => envState());
    mockRoute("get_master_checkup", () => ({
      ...checkupReport(),
      accounts: checkupReport().accounts.filter((a) => a.current),
    }));
    const { unmount } = render(<EnvironmentPage active />);
    await screen.findByTestId("env-checkup-content-ok");
    expect(screen.getByTestId("env-checkup-content-ok")).toHaveTextContent(
      "暂未发现滞留记录，账号记录分布正常。",
    );
    unmount();

    // 场景 2：体检读取失败（reject）且无自检报告 → 区块整体静默，环境页不受影响。
    mockRoute("get_environment_state", () => envState());
    mockRoute("get_master_checkup", () => {
      throw new Error("trae_real_mode_required");
    });
    render(<EnvironmentPage active />);
    expect(await screen.findByTestId("env-master-card")).toBeInTheDocument();
    expect(screen.queryByTestId("env-checkup")).not.toBeInTheDocument();
    expect(screen.getByTestId("env-current-name")).toHaveTextContent("账号甲");
  });

  it("P8-7：归档会话不进入收编，空项目行单独报告", async () => {
    const report = {
      ...checkupReport(),
      accounts: [
        checkupReport().accounts[0],
        { user_id: "user-b", account_name: "归档账号", registered: true, current: false, project_count: 0, session_count: 0, empty_project_count: 0 },
        { user_id: "user-c", account_name: "空项目账号", registered: true, current: false, project_count: 0, session_count: 0, empty_project_count: 1 },
      ],
    };
    mockRoute("get_environment_state", () => envState());
    mockRoute("get_master_checkup", () => report);

    render(<EnvironmentPage active />);
    const checkup = await screen.findByTestId("env-checkup");
    expect(checkup).toHaveTextContent("另有 1 个空项目记录未进入归入范围");
    expect(screen.getByTestId("env-empty-shell-list")).toHaveTextContent("空项目账号");
    expect(screen.queryByTestId("env-incorporate-entry")).not.toBeInTheDocument();
  });

  it("P5-5：空项目显示清理入口，确认后清理并刷新体检", async () => {
    let cleanupCalls = 0;
    let checkupCalls = 0;
    // 带空项目的体检报告（P8-7 同款 fixture，空项目数 2 便于验证数量文案）。
    const reportWithEmpty = () => ({
      ...checkupReport(),
      accounts: [
        checkupReport().accounts[0],
        { user_id: "user-c", account_name: "空项目账号", registered: true, current: false, project_count: 0, session_count: 0, empty_project_count: 2 },
      ],
    });
    mockRoute("get_environment_state", () => envState());
    mockRoute("get_master_checkup", () => {
      checkupCalls += 1;
      // 清理完成后刷新：空项目清零（体检区块自然消失）。
      return cleanupCalls > 0
        ? { ...checkupReport(), accounts: checkupReport().accounts.filter((a) => a.current) }
        : reportWithEmpty();
    });
    mockRoute("cleanup_master_empty_projects", () => {
      cleanupCalls += 1;
      return { deleted_projects: 2, backup_path: "C:\\bak\\switch-bak-456", relaunch_outcome: "launched" };
    });
    render(<EnvironmentPage active />);

    // 体检区块出现空项目行与清理入口。
    await screen.findByTestId("env-checkup");
    fireEvent.click(screen.getByTestId("env-cleanup-empty-projects-entry"));

    // 确认弹层：列明删除数量与备份提示（ADR-0018 单次确认，自然语言表述）。
    const dialog = await screen.findByTestId("env-cleanup-empty-projects-dialog");
    expect(dialog).toHaveTextContent("将删除 2 个空项目记录，删除前会自动备份。");
    expect(dialog).toHaveTextContent("空项目记录不含任何对话，删除不影响对话历史。");
    expect(dialog).not.toHaveTextContent("孤儿");
    expect(dialog).not.toHaveTextContent("软删");

    // 取消可关闭（无副作用）。
    fireEvent.click(screen.getByTestId("env-cleanup-empty-projects-dialog-cancel"));
    expect(screen.queryByTestId("env-cleanup-empty-projects-dialog")).not.toBeInTheDocument();
    expect(mockInvoke).not.toHaveBeenCalledWith("cleanup_master_empty_projects");

    // 重新进入并确认：invoke 清理命令。
    fireEvent.click(screen.getByTestId("env-cleanup-empty-projects-entry"));
    fireEvent.click(await screen.findByTestId("env-cleanup-empty-projects-dialog-confirm"));
    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith("cleanup_master_empty_projects");
    });

    // 成功后弹层关闭、结果消息走 notify（数量表述）并刷新体检。
    await waitFor(() => {
      expect(notifyMock.success).toHaveBeenCalledWith("已删除 2 个空项目记录（已自动备份）。");
    });
    expect(screen.queryByTestId("env-cleanup-empty-projects-dialog")).not.toBeInTheDocument();
    await waitFor(() => {
      expect(checkupCalls).toBeGreaterThanOrEqual(2);
    });
    // P11-5：整合区块常驻，清理后库内容分组翻为正常态提示。
    await waitFor(() => {
      expect(screen.getByTestId("env-checkup-content-ok")).toBeInTheDocument();
    });
  });

  it("P5-5：清理失败走错误码映射，弹层关闭后体检区块保留", async () => {
    const reportWithEmpty = () => ({
      ...checkupReport(),
      accounts: [
        checkupReport().accounts[0],
        { user_id: "user-c", account_name: "空项目账号", registered: true, current: false, project_count: 0, session_count: 0, empty_project_count: 1 },
      ],
    });
    mockRoute("get_environment_state", () => envState());
    mockRoute("get_master_checkup", () => reportWithEmpty());
    mockRoute("cleanup_master_empty_projects", () => {
      throw new Error("cleanup_master_busy");
    });
    render(<EnvironmentPage active />);
    await screen.findByTestId("env-checkup");
    fireEvent.click(screen.getByTestId("env-cleanup-empty-projects-entry"));
    fireEvent.click(await screen.findByTestId("env-cleanup-empty-projects-dialog-confirm"));
    // 失败提示：错误码映射后的用户文案（无底层码泄露），走 notify.error。
    await waitFor(() => {
      expect(notifyMock.error).toHaveBeenCalledWith("主库正在生成回复，清理需等它完成；稍后重试即可。");
    });
    // 弹层已关闭，体检区块保留（数据未变，可重试）。
    expect(screen.queryByTestId("env-cleanup-empty-projects-dialog")).not.toBeInTheDocument();
    expect(screen.getByTestId("env-checkup")).toBeInTheDocument();
  });

  it("P5-5：仅当前账号名下存在空项目时，体检区块与清理入口仍出现", async () => {
    // 无滞留账号（其余账号 0 会话 0 项目），空项目全部挂在当前账号名下。
    const reportCurrentOnlyEmpty = () => ({
      ...checkupReport(),
      accounts: [
        { user_id: "user-a", account_name: "账号甲", registered: true, current: true, project_count: 3, session_count: 5, empty_project_count: 1 },
        { user_id: "user-b", account_name: "账号乙", registered: true, current: false, project_count: 0, session_count: 0, empty_project_count: 0 },
      ],
    });
    mockRoute("get_environment_state", () => envState());
    mockRoute("get_master_checkup", () => reportCurrentOnlyEmpty());
    render(<EnvironmentPage active />);

    // 体检区块出现，提示行按全库口径计数；诊断列表只列非当前账号（此处无）。
    await screen.findByTestId("env-checkup");
    expect(screen.getByTestId("env-checkup")).toHaveTextContent("另有 1 个空项目记录未进入归入范围");
    expect(screen.queryByTestId("env-empty-shell-list")).not.toBeInTheDocument();
    // 清理入口出现（删除范围为全库空项目）；无滞留账号则无归入入口。
    expect(screen.getByTestId("env-cleanup-empty-projects-entry")).toBeInTheDocument();
    expect(screen.queryByTestId("env-incorporate-entry")).not.toBeInTheDocument();
  });

  it("P5-5：重启失败回执按已清理提示，不误导为清理未完成", async () => {
    const reportWithEmpty = () => ({
      ...checkupReport(),
      accounts: [
        checkupReport().accounts[0],
        { user_id: "user-c", account_name: "空项目账号", registered: true, current: false, project_count: 0, session_count: 0, empty_project_count: 1 },
      ],
    });
    mockRoute("get_environment_state", () => envState());
    mockRoute("get_master_checkup", () => reportWithEmpty());
    mockRoute("cleanup_master_empty_projects", () => {
      // 删除已提交但重启失败：relaunch_outcome=failed 的成功回执。
      return { deleted_projects: 1, backup_path: "C:\\bak\\switch-bak-789", relaunch_outcome: "failed" };
    });
    render(<EnvironmentPage active />);
    await screen.findByTestId("env-checkup");
    fireEvent.click(screen.getByTestId("env-cleanup-empty-projects-entry"));
    fireEvent.click(await screen.findByTestId("env-cleanup-empty-projects-dialog-confirm"));

    // 成功回执 + 重启失败：据实提示已删除与补救入口，不出现"清理未完成"。
    await waitFor(() => {
      expect(notifyMock.success).toHaveBeenCalledWith(
        "已删除 1 个空项目记录（已自动备份）；主库 TRAE 未能自动启动，可从环境页重新打开。",
      );
    });
    expect(notifyMock.error).not.toHaveBeenCalled();
  });

  it("P5-5：一键收编全流程（确认规模 → 进度事件 → 完成回执 → 刷新体检）", async () => {
    let resolveIncorporate: (value: unknown) => void = () => undefined;
    let checkupCalls = 0;
    mockRoute("get_environment_state", () => envState());
    mockRoute("get_master_checkup", () => {
      // 收编完成后刷新：第二次起返回无滞留报告（区块自然消失）。
      checkupCalls += 1;
      return checkupCalls <= 1
        ? checkupReport()
        : { ...checkupReport(), accounts: checkupReport().accounts.filter((a) => a.current), orphan_project_count: 0 };
    });
    mockRoute("incorporate_master_records", () => {
      // 挂起回执：先推进进度事件再 resolve，验证运行中步骤状态。
      return new Promise((resolve) => { resolveIncorporate = resolve; });
    });
    render(<EnvironmentPage active />);
    fireEvent.click(await screen.findByTestId("env-incorporate-entry"));

    // 确认弹层：规模 + 账号列表 + 无归属提示（ADR-0018 单次确认）。
    const dialog = await screen.findByTestId("incorporate-dialog");
    expect(dialog).toHaveTextContent("将归入 2 个账号的记录");
    expect(dialog).toHaveTextContent("共 5 个会话、3 个项目将归入账号甲");
    expect(screen.getByTestId("incorporate-account-list")).toHaveTextContent("账号乙");
    expect(screen.getByTestId("incorporate-account-list")).toHaveTextContent("未登记账号");
    expect(dialog).toHaveTextContent("另有 2 条无归属记录不会被归入");

    // 取消可关闭（无副作用）。
    fireEvent.click(screen.getByTestId("incorporate-cancel"));
    expect(screen.queryByTestId("incorporate-dialog")).not.toBeInTheDocument();

    // 重新进入并确认：注册进度监听 → invoke 收编（主库不带 libraryId）。
    fireEvent.click(screen.getByTestId("env-incorporate-entry"));
    fireEvent.click(await screen.findByTestId("incorporate-confirm"));
    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith("incorporate_master_records", undefined);
    });
    await waitFor(() => {
      expect(mockListen).toHaveBeenCalledWith("master-incorporate-progress", expect.any(Function));
    });

    // 推进进度事件到「归入记录」阶段：对应步骤高亮为 run。
    // P11-5 收官：进度载荷带 env_id，弹层只推进自己目标的事件。
    const progressHandler = mockListen.mock.calls.find(
      (call) => call[0] === "master-incorporate-progress",
    )?.[1] as (event: { payload: { stage: string; env_id: string } }) => void;
    await act(async () => {
      progressHandler({ payload: { stage: "incorporating", env_id: "master" } });
    });
    expect(screen.getByTestId("incorporate-dialog")).toHaveTextContent("归入记录");

    // 回执 resolve → 完成态：数量表述 + 100% 进度。
    await act(async () => {
      resolveIncorporate({
        merged_accounts: 2,
        transferred_projects: 3,
        removed_mirror_rows: 1,
        switched_sessions: 5,
        backup_path: "C:\\bak\\switch-bak-123",
        relay_ledger_written: true,
        relaunch_outcome: "launched",
      });
    });
    await waitFor(() => {
      expect(screen.getByTestId("incorporate-dialog")).toHaveTextContent(
        "已归入 2 个账号：保留项目 3 个 · 转移会话 5 个",
      );
    });

    // 完成后刷新体检（get_master_checkup 至少 2 次）→ 滞留诊断清零。
    await waitFor(() => {
      expect(mockInvoke.mock.calls.filter(([command]) => command === "get_master_checkup").length).toBeGreaterThanOrEqual(2);
    });
    fireEvent.click(screen.getByTestId("incorporate-done"));
    expect(screen.queryByTestId("incorporate-dialog")).not.toBeInTheDocument();
    // P11-5：整合区块常驻，收编后库内容分组翻为正常态提示。
    await waitFor(() => {
      expect(screen.getByTestId("env-checkup-content-ok")).toBeInTheDocument();
    });
  });

  it("P5-5：收编失败显示错误文案（错误码映射），可关闭弹层", async () => {
    mockRoute("get_environment_state", () => envState());
    mockRoute("get_master_checkup", () => checkupReport());
    mockRoute("incorporate_master_records", () => {
      throw new Error("incorporate_nothing_to_do");
    });
    render(<EnvironmentPage active />);
    fireEvent.click(await screen.findByTestId("env-incorporate-entry"));
    fireEvent.click(await screen.findByTestId("incorporate-confirm"));
    // 失败提示：错误码映射后的用户文案（无底层码泄露）。
    const dialog = await waitFor(() => {
      const node = screen.getByTestId("incorporate-dialog");
      expect(node).toHaveTextContent("主库内没有待归入的其他账号记录，无需操作。");
      return node;
    });
    expect(dialog).not.toHaveTextContent("incorporate_nothing_to_do");
    fireEvent.click(screen.getByTestId("incorporate-failed-close"));
    expect(screen.queryByTestId("incorporate-dialog")).not.toBeInTheDocument();
    // 失败后体检区块仍在（数据未变，可重试）。
    expect(screen.getByTestId("env-checkup")).toBeInTheDocument();
  });

  // ===== P6-4 辅助环境管理 V2 =====

  /** 辅助环境列表项 fixture（含当前账号与体积）。 */
  function secondaryEnv(overrides: Partial<EnvironmentListItemDto> = {}): EnvironmentListItemDto {
    return envListItem({
      env_id: "env-x1",
      name: "测试环境",
      is_master: false,
      current_profile_id: "profile-b",
      current_account_name: "账号乙",
      data_dir: "D:\\store\\environments\\env-x1",
      running: false,
      login_state: "logged_in",
      created_at_unix_seconds: 1750000100,
      size_bytes: 2097152,
      ...overrides,
    });
  }

  /** 账号选择弹层用的账号视图 fixture（组件只读 saved_accounts）。 */
  function accountView() {
    return {
      saved_accounts: [
        {
          profile_id: "profile-a",
          display_name: "账号甲",
          region: null,
          data_location_id: "loc",
          last_verified_at: null,
          verification_state: "verified",
        },
        {
          profile_id: "profile-b",
          display_name: "账号乙",
          region: null,
          data_location_id: "loc",
          last_verified_at: null,
          verification_state: "verified",
        },
      ],
    };
  }

  it("P6-4：辅助环境卡渲染（名称/当前账号/体积/四项操作）", async () => {
    mockRoute("get_environment_state", () => envState());
    mockRoute("list_environments", () => [envListItem(), secondaryEnv()]);
    render(<EnvironmentPage active />);
    const card = await screen.findByTestId("env-card-env-x1");
    expect(card).toHaveTextContent("测试环境");
    expect(card).toHaveTextContent("辅助环境");
    expect(card).toHaveTextContent("账号乙");
    // 体积展示（formatBytes 对 <100 的值保留一位小数）与目录路径悬浮提示（完整路径不进主视野）。
    expect(card).toHaveTextContent("2.0 MB");
    expect(screen.getByTestId("env-path-env-x1")).toHaveAttribute(
      "title",
      "数据目录：D:\\store\\environments\\env-x1",
    );
    // 四项操作齐备；未运行 → 启动语义。
    expect(screen.getByTestId("env-login-env-x1")).toBeInTheDocument();
    expect(screen.getByTestId("env-rename-env-x1")).toBeInTheDocument();
    expect(screen.getByTestId("env-delete-env-x1")).toBeInTheDocument();
    expect(screen.getByTestId("env-launch-env-x1")).toHaveTextContent("启动");
    // 空态提示不渲染（已有辅助环境）。
    expect(screen.queryByTestId("env-placeholder")).not.toBeInTheDocument();
  });

  it("P11-5 收官：副环境体检区块有差异才显示，归入按钮按库发起收编", async () => {
    const secondaryCheckup = {
      status: "ready",
      current_account_name: "账号乙",
      accounts: [
        { user_id: "user-b", account_name: "账号乙", registered: true, current: true, project_count: 1, session_count: 2, empty_project_count: 0 },
        { user_id: "user-c", account_name: "未登记账号", registered: false, current: false, project_count: 2, session_count: 3, empty_project_count: 1 },
      ],
      orphan_project_count: 1,
      orphan_session_count: 0,
    };
    // 带副环境 libraryId 的体检 → 副环境报告；缺省 → 主库报告。
    mockRoute("get_environment_state", () => envState());
    mockRoute("list_environments", () => [envListItem(), secondaryEnv()]);
    mockRoute("get_master_checkup", (args) => {
      return (args as { libraryId?: string } | undefined)?.libraryId === "env-x1"
        ? secondaryCheckup
        : checkupReport();
    });
    mockRoute("incorporate_master_records", () => ({
      merged_accounts: 1,
      transferred_projects: 2,
      removed_mirror_rows: 0,
      switched_sessions: 3,
      backup_path: "C:\\bak\\switch-bak-42",
      relay_ledger_written: true,
      relaunch_outcome: "launched",
    }));
    const onOpenLibraryDetail = vi.fn();
    render(<EnvironmentPage active onOpenLibraryDetail={onOpenLibraryDetail} />);
    // 体检区块（有差异）：登录健康只显示注册表账号（不出自检四项）。
    const checkup = await screen.findByTestId("env-checkup-env-x1");
    expect(checkup).toHaveTextContent("测试环境体检");
    expect(screen.getByTestId("env-checkup-login-env-x1")).toHaveTextContent("当前登录：账号乙");
    expect(checkup).toHaveTextContent("有 1 个账号的 3 个会话在此环境中");
    // 归入按钮在副环境卡上。
    fireEvent.click(screen.getByTestId(`env-incorporate-env-x1`));
    const dialog = await screen.findByTestId("incorporate-dialog");
    expect(dialog).toHaveTextContent("「测试环境」会自动关闭并重启");
    // 确认后按库发起收编（libraryId = env_id）。
    fireEvent.click(screen.getByTestId("incorporate-confirm"));
    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith("incorporate_master_records", { libraryId: "env-x1" });
    });
    // 查看详情入口携带环境引用。
    fireEvent.click(screen.getByTestId("env-detail-env-x1"));
    expect(onOpenLibraryDetail).toHaveBeenCalledWith({ id: "env-x1", displayName: "测试环境" });
  });

  it("P11-5 收官：副环境体检无差异时不显示体检区块", async () => {
    const cleanSecondaryCheckup = {
      status: "ready",
      current_account_name: "账号乙",
      accounts: [
        { user_id: "user-b", account_name: "账号乙", registered: true, current: true, project_count: 1, session_count: 2, empty_project_count: 0 },
      ],
      orphan_project_count: 0,
      orphan_session_count: 0,
    };
    mockRoute("get_environment_state", () => envState());
    mockRoute("list_environments", () => [envListItem(), secondaryEnv()]);
    mockRoute("get_master_checkup", (args) => {
      return (args as { libraryId?: string } | undefined)?.libraryId === "env-x1"
        ? cleanSecondaryCheckup
        : checkupReport();
    });
    render(<EnvironmentPage active />);
    await screen.findByTestId("env-card-env-x1");
    // 无差异：副环境卡无体检区块（主库体检区块照常显示）。
    expect(screen.queryByTestId("env-checkup-env-x1")).not.toBeInTheDocument();
    expect(screen.getByTestId("env-checkup")).toBeInTheDocument();
  });

  it("P6-4：创建环境全流程（输入名称 → 确认 → invoke create_environment）", async () => {
    const created: string[] = [];
    mockRoute("get_environment_state", () => envState());
    mockRoute("list_environments", () => {
      // 创建后列表含新环境（第二次读取起，env_id 与创建回执一致）。
      return created.length > 0
        ? [envListItem(), secondaryEnv({ env_id: "env-x2", name: created[0] })]
        : [envListItem()];
    });
    mockRoute("create_environment", (args) => {
      const payload = args as Record<string, unknown>;
      created.push(String(payload.name));
      return { env_id: "env-x2", name: String(payload.name), created_at_unix_seconds: 1750000200 };
    });
    render(<EnvironmentPage active />);
    fireEvent.click(await screen.findByTestId("env-create-entry"));
    fireEvent.change(screen.getByTestId("env-name-input"), { target: { value: "新环境" } });
    fireEvent.click(screen.getByTestId("env-name-confirm"));
    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith("create_environment", { name: "新环境" });
    });
    // 成功后弹层关闭，列表刷新出现新环境。
    await waitFor(() => {
      expect(screen.queryByTestId("env-name-dialog")).not.toBeInTheDocument();
    });
    expect(await screen.findByTestId("env-card-env-x2")).toBeInTheDocument();
  });

  it("P6-4：创建失败（重名）在弹层内显示映射文案，不泄露错误码", async () => {
    mockRoute("get_environment_state", () => envState());
    mockRoute("list_environments", () => [envListItem()]);
    mockRoute("create_environment", () => {
      throw new Error("environment_name_taken");
    });
    render(<EnvironmentPage active />);
    fireEvent.click(await screen.findByTestId("env-create-entry"));
    fireEvent.change(screen.getByTestId("env-name-input"), { target: { value: "主库" } });
    fireEvent.click(screen.getByTestId("env-name-confirm"));
    const dialog = await waitFor(() => {
      const node = screen.getByTestId("env-name-dialog");
      expect(node).toHaveTextContent("已有同名环境，请换一个名称。");
      return node;
    });
    expect(dialog).not.toHaveTextContent("environment_name_taken");
    // 失败后弹层保留（可改名重试）。
    expect(screen.getByTestId("env-name-confirm")).toBeInTheDocument();
  });

  it("P6-4：重命名环境（预填原名 → 保存 → invoke rename_environment）", async () => {
    mockRoute("get_environment_state", () => envState());
    mockRoute("list_environments", () => [envListItem(), secondaryEnv()]);
    mockRoute("rename_environment", () => ({
      env_id: "env-x1",
      name: "新名字",
      created_at_unix_seconds: 1750000100,
    }));
    render(<EnvironmentPage active />);
    fireEvent.click(await screen.findByTestId("env-rename-env-x1"));
    // 弹层预填原名。
    expect(screen.getByTestId("env-name-input")).toHaveValue("测试环境");
    fireEvent.change(screen.getByTestId("env-name-input"), { target: { value: "新名字" } });
    fireEvent.click(screen.getByTestId("env-name-confirm"));
    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith("rename_environment", { envId: "env-x1", name: "新名字" });
    });
    await waitFor(() => {
      expect(screen.queryByTestId("env-name-dialog")).not.toBeInTheDocument();
    });
  });

  it("P6-4：环境登录账号全流程（选账号 → 登录 → 回执 → 刷新列表）", async () => {
    const logins: { envId: string; profileId: string }[] = [];
    let loginCalls = 0;
    mockRoute("get_environment_state", () => envState());
    mockRoute("list_environments", () => {
      // 登录后当前账号翻为账号甲（第二次读取起）。
      return loginCalls > 0
        ? [envListItem(), secondaryEnv({ current_profile_id: "profile-a", current_account_name: "账号甲" })]
        : [envListItem(), secondaryEnv()];
    });
    mockRoute("get_managed_account_state", () => accountView());
    mockRoute("login_environment", (args) => {
      const payload = args as Record<string, unknown>;
      loginCalls += 1;
      logins.push({ envId: String(payload.envId), profileId: String(payload.profileId) });
      // 空环境首登回执（播种形态）。
      return {
        env_id: String(payload.envId),
        profile_id: String(payload.profileId),
        seeded: true,
        transferred_projects: 0,
        switched_sessions: 0,
        removed_mirror_rows: 0,
      };
    });
    render(<EnvironmentPage active />);
    fireEvent.click(await screen.findByTestId("env-login-env-x1"));
    // 账号列表渲染，默认未选中。
    const list = await screen.findByTestId("env-login-account-list");
    expect(list).toHaveTextContent("账号甲");
    expect(list).toHaveTextContent("账号乙");
    expect(screen.getByTestId("env-login-confirm")).toBeDisabled();
    // 选中账号甲 → 确认登录。
    fireEvent.click(screen.getByTestId("env-login-account-profile-a"));
    fireEvent.click(screen.getByTestId("env-login-confirm"));
    await waitFor(() => {
      expect(logins).toEqual([{ envId: "env-x1", profileId: "profile-a" }]);
    });
    // 首登回执（数量表述，无内部标识）。
    const receipt = await screen.findByTestId("env-login-receipt");
    expect(receipt).toHaveTextContent("已登录账号 账号甲");
    expect(receipt).toHaveTextContent("环境首次登录完成");
    // 完成关闭弹层并刷新列表（当前账号翻为账号甲）。
    fireEvent.click(screen.getByTestId("env-login-done"));
    await waitFor(() => {
      expect(screen.queryByTestId("env-login-dialog")).not.toBeInTheDocument();
    });
    await waitFor(() => {
      expect(screen.getByTestId("env-card-env-x1")).toHaveTextContent("账号甲");
    });
  });

  it("P6-4：环境登录失败显示映射文案（错误码不泄露）", async () => {
    mockRoute("get_environment_state", () => envState());
    mockRoute("list_environments", () => [envListItem(), secondaryEnv()]);
    mockRoute("get_managed_account_state", () => accountView());
    mockRoute("login_environment", () => {
      throw new Error("environment_login_busy");
    });
    render(<EnvironmentPage active />);
    fireEvent.click(await screen.findByTestId("env-login-env-x1"));
    fireEvent.click(await screen.findByTestId("env-login-account-profile-a"));
    fireEvent.click(screen.getByTestId("env-login-confirm"));
    const dialog = await waitFor(() => {
      const node = screen.getByTestId("env-login-dialog");
      expect(node).toHaveTextContent("该环境正在生成回复，请等它完成后再切换账号。");
      return node;
    });
    expect(dialog).not.toHaveTextContent("environment_login_busy");
  });

  it("P6-4：删除环境确认弹层列明规模，确认后删除并刷新列表", async () => {
    const deletes: string[] = [];
    let deleteCalls = 0;
    mockRoute("get_environment_state", () => envState());
    mockRoute("list_environments", () => (deleteCalls > 0 ? [envListItem()] : [envListItem(), secondaryEnv()]));
    mockRoute("get_environment_delete_preview", () => ({
      status: "ready",
      project_count: 3,
      session_count: 5,
      size_bytes: 2097152,
    }));
    mockRoute("delete_environment", (args) => {
      const payload = args as Record<string, unknown>;
      deleteCalls += 1;
      deletes.push(String(payload.envId));
      return true;
    });
    render(<EnvironmentPage active />);
    fireEvent.click(await screen.findByTestId("env-delete-env-x1"));
    // 确认弹层：列明规模（ADR-0018 单次确认）。
    const dialog = await screen.findByTestId("env-delete-dialog");
    expect(dialog).toHaveTextContent("删除后无法恢复");
    expect(dialog).toHaveTextContent("3 个项目、5 个会话");
    expect(dialog).toHaveTextContent("2.0 MB");
    // 取消可关闭（无副作用）。
    fireEvent.click(screen.getByTestId("env-delete-dialog-cancel"));
    expect(screen.queryByTestId("env-delete-dialog")).not.toBeInTheDocument();
    // 重新进入并确认删除。
    fireEvent.click(screen.getByTestId("env-delete-env-x1"));
    fireEvent.click(await screen.findByTestId("env-delete-dialog-confirm"));
    await waitFor(() => {
      expect(deletes).toEqual(["env-x1"]);
    });
    // 删除后列表刷新，卡片消失。
    await waitFor(() => {
      expect(screen.queryByTestId("env-card-env-x1")).not.toBeInTheDocument();
    });
  });

  it("P6-4：删除运行中环境被拒（错误码映射后显示可执行提示）", async () => {
    mockRoute("get_environment_state", () => envState());
    mockRoute("list_environments", () => [envListItem(), secondaryEnv()]);
    mockRoute("get_environment_delete_preview", () => ({
      status: "ready",
      project_count: 0,
      session_count: 0,
      size_bytes: 1024,
    }));
    mockRoute("delete_environment", () => {
      throw new Error("environment_delete_running");
    });
    render(<EnvironmentPage active />);
    fireEvent.click(await screen.findByTestId("env-delete-env-x1"));
    fireEvent.click(await screen.findByTestId("env-delete-dialog-confirm"));
    const dialog = await waitFor(() => {
      const node = screen.getByTestId("env-delete-dialog");
      expect(node).toHaveTextContent("该环境正在运行，请先关闭它的窗口再删除。");
      return node;
    });
    expect(dialog).not.toHaveTextContent("environment_delete_running");
  });

  // ===== P11-5 关闭环境实例 =====

  it("P11-5：主库运行中显示关闭按钮，确认弹层后调用关闭命令并刷新状态", async () => {
    let closeCalls = 0;
    mockRoute("get_environment_state", () => {
      // 关闭成功后重读：运行态翻转为停止（5 秒轮询之外的主动校准）。
      return envState({ running: closeCalls > 0 ? false : true });
    });
    mockRoute("close_environment_instance", () => {
      closeCalls += 1;
      return { outcome: "closed" };
    });
    render(<EnvironmentPage active />);
    // 运行中 → 关闭按钮出现（与启动/聚焦同区）。
    fireEvent.click(await screen.findByTestId("env-master-close"));
    // 确认弹层：说明将关闭该环境的 TRAE 实例。
    const dialog = await screen.findByTestId("env-close-dialog");
    expect(dialog).toHaveTextContent("将关闭「主库」正在运行的 TRAE 实例。");
    fireEvent.click(screen.getByTestId("env-close-dialog-cancel"));
    expect(screen.queryByTestId("env-close-dialog")).not.toBeInTheDocument();
    expect(mockInvoke).not.toHaveBeenCalledWith("close_environment_instance", expect.anything());

    // 确认 → 关闭命令 + 按 outcome 反馈 + 刷新。
    fireEvent.click(await screen.findByTestId("env-master-close"));
    fireEvent.click(await screen.findByTestId("env-close-dialog-confirm"));
    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith("close_environment_instance", { envId: "master" });
    });
    await waitFor(() => {
      expect(notifyMock.success).toHaveBeenCalledWith("已关闭「主库」的 TRAE 实例。");
    });
    // 关闭后运行态刷新为停止：按钮随运行态隐藏。
    await waitFor(() => {
      expect(screen.queryByTestId("env-master-close")).not.toBeInTheDocument();
    });
  });

  it("P11-5：副环境关闭按钮仅运行中显示，强制结束与未运行回执各自反馈", async () => {
    // 场景 1：未运行 → 关闭按钮不出现。
    mockRoute("get_environment_state", () => envState());
    mockRoute("list_environments", () => [envListItem(), secondaryEnv({ running: false })]);
    const first = render(<EnvironmentPage active />);
    await screen.findByTestId("env-card-env-x1");
    expect(screen.queryByTestId("env-close-env-x1")).not.toBeInTheDocument();
    first.unmount();

    // 场景 2：运行中 → 确认后强制结束回执（force_closed）反馈。
    mockRoute("get_environment_state", () => envState());
    mockRoute("list_environments", () => [envListItem(), secondaryEnv({ running: true })]);
    mockRoute("close_environment_instance", () => ({ outcome: "force_closed" }));
    render(<EnvironmentPage active />);
    fireEvent.click(await screen.findByTestId("env-close-env-x1"));
    const dialog = await screen.findByTestId("env-close-dialog");
    expect(dialog).toHaveTextContent("将关闭「测试环境」正在运行的 TRAE 实例。");
    fireEvent.click(screen.getByTestId("env-close-dialog-confirm"));
    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith("close_environment_instance", { envId: "env-x1" });
    });
    await waitFor(() => {
      expect(notifyMock.success).toHaveBeenCalledWith("「测试环境」未能自动退出，已强制结束进程。");
    });
  });

  it("P11-5：关闭时环境已自行退出按幂等提示，不报错", async () => {
    mockRoute("get_environment_state", () => envState());
    mockRoute("list_environments", () => [envListItem(), secondaryEnv({ running: true })]);
    mockRoute("close_environment_instance", () => ({ outcome: "not_running" }));
    render(<EnvironmentPage active />);
    fireEvent.click(await screen.findByTestId("env-close-env-x1"));
    fireEvent.click(await screen.findByTestId("env-close-dialog-confirm"));
    await waitFor(() => {
      expect(notifyMock.info).toHaveBeenCalledWith("「测试环境」当前没有运行中的实例。");
    });
    expect(notifyMock.error).not.toHaveBeenCalled();
  });

  it("P11-5：关闭失败走错误码映射（notify.error）", async () => {
    mockRoute("get_environment_state", () => envState());
    mockRoute("list_environments", () => [envListItem(), secondaryEnv({ running: true })]);
    mockRoute("close_environment_instance", () => {
      throw new Error("environment_close_failed");
    });
    render(<EnvironmentPage active />);
    fireEvent.click(await screen.findByTestId("env-close-env-x1"));
    fireEvent.click(await screen.findByTestId("env-close-dialog-confirm"));
    await waitFor(() => {
      expect(notifyMock.error).toHaveBeenCalledWith(
        "TRAE 实例未能关闭，请稍后重试，或在 TRAE 窗口内直接退出。",
      );
    });
  });
});
