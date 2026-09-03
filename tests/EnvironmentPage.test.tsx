import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { EnvironmentPage } from "../src/components/EnvironmentPage";
import type { EnvironmentListItemDto, EnvironmentStateDto } from "../src/types/environment";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

// 事件插件 mock：捕获 listen 回调，供收编进度测试手动触发（模式同 CheckinPage.test）。
const { mockListen } = vi.hoisted(() => ({ mockListen: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({ listen: mockListen }));

const mockInvoke = vi.mocked(invoke);

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
    mockInvoke.mockImplementation(async (command: string) => {
      if (command === "get_environment_state") return envState();
      if (command === "launch_master_library") return { outcome: "launched", login_state: "logged_in" };
      if (command === "list_environments") return [envListItem()];
      return undefined;
    });
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
    mockInvoke.mockImplementation(async (command: string) => {
      if (command === "get_environment_state") {
        return envState({ current_profile_id: null, current_account_name: null, running: false, login_state: "uninitialized" });
      }
      return undefined;
    });
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

  it("P5-4：主库统计可读时渲染统计格（会话/项目/参与账号）", async () => {
    mockInvoke.mockImplementation(async (command: string) => {
      if (command === "get_environment_state") return envState();
      if (command === "get_master_library_stats") {
        return {
          status: "ready",
          current_user_id: "user-a",
          project_count: 4,
          session_count: 7,
          participating_account_count: 3,
          last_active_unix_seconds: null,
        };
      }
      return undefined;
    });
    render(<EnvironmentPage active />);
    const stats = await screen.findByTestId("env-master-stats");
    expect(stats).toHaveTextContent("7");
    expect(stats).toHaveTextContent("会话");
    expect(stats).toHaveTextContent("项目");
    expect(stats).toHaveTextContent("参与账号");
  });

  it("P5-4：主库统计读取失败时统计格整体不渲染（可插拔降级）", async () => {
    // beforeEach 默认 mock 未实现 get_master_library_stats → undefined / reject 之外的
    // 降级路径：显式 reject 覆盖为失败，统计格应静默消失而非报错。
    mockInvoke.mockImplementation(async (command: string) => {
      if (command === "get_environment_state") return envState();
      if (command === "get_master_library_stats") throw new Error("trae_real_mode_required");
      return undefined;
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
        { user_id: "user-a", account_name: "账号甲", registered: true, current: true, project_count: 3, session_count: 5 },
        { user_id: "user-b", account_name: "账号乙", registered: true, current: false, project_count: 2, session_count: 4 },
        { user_id: "user-c", account_name: null, registered: false, current: false, project_count: 1, session_count: 1 },
      ],
      orphan_project_count: 2,
      orphan_session_count: 0,
    };
  }

  it("P5-5：体检发现滞留账号时渲染体检区块（数量表述 + 账号名主信息）", async () => {
    mockInvoke.mockImplementation(async (command: string) => {
      if (command === "get_environment_state") return envState();
      if (command === "get_master_checkup") return checkupReport();
      return undefined;
    });
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

  it("P5-5：体检无滞留或读取失败时区块静默不渲染", async () => {
    // 场景 1：分布里只有当前账号（无滞留）。
    mockInvoke.mockImplementation(async (command: string) => {
      if (command === "get_environment_state") return envState();
      if (command === "get_master_checkup") {
        return { ...checkupReport(), accounts: checkupReport().accounts.filter((a) => a.current) };
      }
      return undefined;
    });
    const { unmount } = render(<EnvironmentPage active />);
    await screen.findByTestId("env-master-card");
    expect(screen.queryByTestId("env-checkup")).not.toBeInTheDocument();
    unmount();

    // 场景 2：体检读取失败（reject）→ 静默降级，环境页本体不受影响。
    mockInvoke.mockImplementation(async (command: string) => {
      if (command === "get_environment_state") return envState();
      if (command === "get_master_checkup") throw new Error("trae_real_mode_required");
      return undefined;
    });
    render(<EnvironmentPage active />);
    expect(await screen.findByTestId("env-master-card")).toBeInTheDocument();
    expect(screen.queryByTestId("env-checkup")).not.toBeInTheDocument();
    expect(screen.getByTestId("env-current-name")).toHaveTextContent("账号甲");
  });

  it("P5-5：一键收编全流程（确认规模 → 进度事件 → 完成回执 → 刷新体检）", async () => {
    let resolveIncorporate: (value: unknown) => void = () => undefined;
    let checkupCalls = 0;
    mockInvoke.mockImplementation(async (command: string) => {
      if (command === "get_environment_state") return envState();
      if (command === "get_master_checkup") {
        // 收编完成后刷新：第二次起返回无滞留报告（区块自然消失）。
        checkupCalls += 1;
        return checkupCalls <= 1
          ? checkupReport()
          : { ...checkupReport(), accounts: checkupReport().accounts.filter((a) => a.current), orphan_project_count: 0 };
      }
      if (command === "incorporate_master_records") {
        // 挂起回执：先推进进度事件再 resolve，验证运行中步骤状态。
        return new Promise((resolve) => { resolveIncorporate = resolve; });
      }
      return undefined;
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

    // 重新进入并确认：注册进度监听 → invoke 收编。
    fireEvent.click(screen.getByTestId("env-incorporate-entry"));
    fireEvent.click(await screen.findByTestId("incorporate-confirm"));
    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith("incorporate_master_records");
    });
    await waitFor(() => {
      expect(mockListen).toHaveBeenCalledWith("master-incorporate-progress", expect.any(Function));
    });

    // 推进进度事件到「归入记录」阶段：对应步骤高亮为 run。
    const progressHandler = mockListen.mock.calls.find(
      (call) => call[0] === "master-incorporate-progress",
    )?.[1] as (event: { payload: { stage: string } }) => void;
    await act(async () => {
      progressHandler({ payload: { stage: "incorporating" } });
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

    // 完成后刷新体检（get_master_checkup 至少 2 次）→ 体检区块消失。
    await waitFor(() => {
      expect(mockInvoke.mock.calls.filter(([command]) => command === "get_master_checkup").length).toBeGreaterThanOrEqual(2);
    });
    fireEvent.click(screen.getByTestId("incorporate-done"));
    expect(screen.queryByTestId("incorporate-dialog")).not.toBeInTheDocument();
    await waitFor(() => {
      expect(screen.queryByTestId("env-checkup")).not.toBeInTheDocument();
    });
  });

  it("P5-5：收编失败显示错误文案（错误码映射），可关闭弹层", async () => {
    mockInvoke.mockImplementation(async (command: string) => {
      if (command === "get_environment_state") return envState();
      if (command === "get_master_checkup") return checkupReport();
      if (command === "incorporate_master_records") throw new Error("incorporate_nothing_to_do");
      return undefined;
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
    mockInvoke.mockImplementation(async (command: string) => {
      if (command === "get_environment_state") return envState();
      if (command === "list_environments") return [envListItem(), secondaryEnv()];
      return undefined;
    });
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

  it("P6-4：创建环境全流程（输入名称 → 确认 → invoke create_environment）", async () => {
    const created: string[] = [];
    mockInvoke.mockImplementation(async (command: string, args?: unknown) => {
      if (command === "get_environment_state") return envState();
      if (command === "list_environments") {
        // 创建后列表含新环境（第二次读取起，env_id 与创建回执一致）。
        return created.length > 0
          ? [envListItem(), secondaryEnv({ env_id: "env-x2", name: created[0] })]
          : [envListItem()];
      }
      if (command === "create_environment" && args) {
        const payload = args as Record<string, unknown>;
        created.push(String(payload.name));
        return { env_id: "env-x2", name: String(payload.name), created_at_unix_seconds: 1750000200 };
      }
      return undefined;
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
    mockInvoke.mockImplementation(async (command: string) => {
      if (command === "get_environment_state") return envState();
      if (command === "list_environments") return [envListItem()];
      if (command === "create_environment") throw new Error("environment_name_taken");
      return undefined;
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
    mockInvoke.mockImplementation(async (command: string) => {
      if (command === "get_environment_state") return envState();
      if (command === "list_environments") return [envListItem(), secondaryEnv()];
      if (command === "rename_environment") {
        return { env_id: "env-x1", name: "新名字", created_at_unix_seconds: 1750000100 };
      }
      return undefined;
    });
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
    mockInvoke.mockImplementation(async (command: string, args?: unknown) => {
      if (command === "get_environment_state") return envState();
      if (command === "list_environments") {
        // 登录后当前账号翻为账号甲（第二次读取起）。
        return loginCalls > 0
          ? [envListItem(), secondaryEnv({ current_profile_id: "profile-a", current_account_name: "账号甲" })]
          : [envListItem(), secondaryEnv()];
      }
      if (command === "get_managed_account_state") return accountView();
      if (command === "login_environment" && args) {
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
      }
      return undefined;
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
    mockInvoke.mockImplementation(async (command: string) => {
      if (command === "get_environment_state") return envState();
      if (command === "list_environments") return [envListItem(), secondaryEnv()];
      if (command === "get_managed_account_state") return accountView();
      if (command === "login_environment") throw new Error("environment_login_busy");
      return undefined;
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
    mockInvoke.mockImplementation(async (command: string, args?: unknown) => {
      if (command === "get_environment_state") return envState();
      if (command === "list_environments") {
        return deleteCalls > 0 ? [envListItem()] : [envListItem(), secondaryEnv()];
      }
      if (command === "get_environment_delete_preview") {
        return { status: "ready", project_count: 3, session_count: 5, size_bytes: 2097152 };
      }
      if (command === "delete_environment" && args) {
        const payload = args as Record<string, unknown>;
        deleteCalls += 1;
        deletes.push(String(payload.envId));
        return true;
      }
      return undefined;
    });
    render(<EnvironmentPage active />);
    fireEvent.click(await screen.findByTestId("env-delete-env-x1"));
    // 确认弹层：列明规模（ADR-0018 单次确认）。
    const dialog = await screen.findByTestId("env-delete-dialog");
    expect(dialog).toHaveTextContent("删除后无法恢复");
    expect(dialog).toHaveTextContent("3 个项目、5 个会话");
    expect(dialog).toHaveTextContent("2.0 MB");
    // 取消可关闭（无副作用）。
    fireEvent.click(screen.getByTestId("env-delete-cancel"));
    expect(screen.queryByTestId("env-delete-dialog")).not.toBeInTheDocument();
    // 重新进入并确认删除。
    fireEvent.click(screen.getByTestId("env-delete-env-x1"));
    fireEvent.click(await screen.findByTestId("env-delete-confirm"));
    await waitFor(() => {
      expect(deletes).toEqual(["env-x1"]);
    });
    // 删除后列表刷新，卡片消失。
    await waitFor(() => {
      expect(screen.queryByTestId("env-card-env-x1")).not.toBeInTheDocument();
    });
  });

  it("P6-4：删除运行中环境被拒（错误码映射后显示可执行提示）", async () => {
    mockInvoke.mockImplementation(async (command: string) => {
      if (command === "get_environment_state") return envState();
      if (command === "list_environments") return [envListItem(), secondaryEnv()];
      if (command === "get_environment_delete_preview") {
        return { status: "ready", project_count: 0, session_count: 0, size_bytes: 1024 };
      }
      if (command === "delete_environment") throw new Error("environment_delete_running");
      return undefined;
    });
    render(<EnvironmentPage active />);
    fireEvent.click(await screen.findByTestId("env-delete-env-x1"));
    fireEvent.click(await screen.findByTestId("env-delete-confirm"));
    const dialog = await waitFor(() => {
      const node = screen.getByTestId("env-delete-dialog");
      expect(node).toHaveTextContent("该环境正在运行，请先关闭它的窗口再删除。");
      return node;
    });
    expect(dialog).not.toHaveTextContent("environment_delete_running");
  });
});
