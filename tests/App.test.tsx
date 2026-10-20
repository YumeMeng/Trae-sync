import { beforeEach, describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import type { WorkspaceStateDto } from "../src/types/workspace";
import App from "../src/App";
import { installFakeInvoke, mockRoute, resetRoutes } from "./helpers/fakeInvoke";

// App 测试只模拟 Tauri command，确保测试不会访问真实 TRAE 数据。
// P13-5：共享 fakeInvoke 路由器（迁移模式见 tests/helpers/fakeInvoke.ts 头注释）。
vi.mock("@tauri-apps/api/core", async () => {
  const fake = await import("./helpers/fakeInvoke");
  return { invoke: fake.mockCoreInvoke };
});

// 事件插件 mock：捕获 listen 回调，供自动签到跨页摘要测试手动触发。
const { mockListen } = vi.hoisted(() => ({ mockListen: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({ listen: mockListen }));

// P11-1 切片②：App 级自动签到完成摘要经 notify.summary 发布；断言 notify 调用。
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

const workspace: WorkspaceStateDto = {
  platform: {
    platform_id: "work_cn",
    display_name: "TRAE",
    adapter_implemented: true,
  },
  data_location: {
    selected: true,
    display_name: "C:\\TRAE\\ModularData",
    unavailable_reason: null,
  },
  current_account: {
    detected: true,
    user_fingerprint: "acct-1234",
    unavailable_reason: null,
  },
  history: {
    account_count: 0,
    project_count: 0,
    session_count: 0,
  },
  capabilities: {
    scan_enabled: true,
    sync_enabled: false,
    backup_enabled: false,
    restore_enabled: false,
  },
  honest_status: "RealReadPreview",
};

describe("应用工作区入口", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    // 事件监听默认立即注册成功（unlisten 为空函数）。
    mockListen.mockResolvedValue(() => undefined);
    // 等价于原 mockInvoke.mockImplementation(...)：清历史 + 重装路由分发 + 默认路由。
    resetRoutes();
    installFakeInvoke();
    mockRoute("get_workspace_state", () => workspace);
    // 产品上下文（P17-T5 并列骨架）：缺省当前产品 trae_cn + 双产品注册表。
    mockRoute("list_products", () => [
      { product_id: "trae_cn", display_name: "TRAE" },
      { product_id: "workbuddy", display_name: "WorkBuddy" },
    ]);
    mockRoute("get_current_product", () => ({ product_id: "trae_cn", display_name: "TRAE" }));
    mockRoute("get_environment_state", () => ({
      env_id: "master",
      current_profile_id: "profile-a",
      current_account_name: "主库账号",
      observed_profile_id: "profile-a",
      data_dir: "C:\\TRAE\\master",
      running: false,
      login_state: "logged_in",
      created_at_unix_seconds: 1,
    }));
    mockRoute("get_checkin_capability", () => ({
      enabled: false,
      transport: "disabled",
      real_http_enabled: false,
      message: "签到能力当前不可读取。",
    }));
    mockRoute("get_product_identity_state", () => [
      {
        product_id: "work_cn",
        display_name: "TRAE Work CN",
        identity_status: "recognized",
        credential_status: "managed_by_work",
        relation_to_work: "current_product",
      },
      {
        product_id: "trae_cn",
        display_name: "TRAE",
        identity_status: "recognized",
        credential_status: "need_authorization",
        relation_to_work: "different_identity",
      },
    ]);
    mockRoute("list_trae_cn_accounts", () => []);
  });

  it("P11-1：App 级监听自动签到完成事件，跨页发布 notify.summary 摘要", async () => {
    const handlers = new Map<string, (event: { payload: unknown }) => void>();
    mockListen.mockImplementation(async (event: string, cb: (event: { payload: unknown }) => void) => {
      handlers.set(event, cb);
      return () => undefined;
    });
    render(<App />);
    await screen.findByTestId("current-account-context");

    // 等待 App 级监听注册完成，再模拟后端批次完成事件（带失败与跳过计数）。
    await waitFor(() => {
      expect(handlers.get("auto-checkin-finished")).toBeDefined();
    });
    handlers.get("auto-checkin-finished")?.({
      payload: { total: 4, completed: 2, failed: 1, skipped: 1 },
    });

    // 跨页摘要：失败/跳过计数收进标题，成功数经 notify.summary 的 okCount 承载。
    await waitFor(() => {
      expect(notifyMock.summary).toHaveBeenCalledWith("TRAE：今日自动签到完成（失败 1，跳过 1）", [], 2);
    });
  });

  it("G8：独立历史入口已删除，主库对话面板仅由主库详情承载", async () => {
    render(<App />);

    expect(await screen.findByTestId("current-account-context")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "历史" })).not.toBeInTheDocument();
    // 主库详情未激活前不挂载库内对话面板，也不保留旧授权扫描入口。
    expect(screen.queryByTestId("authorize-check")).not.toBeInTheDocument();
    expect(screen.queryByText("Work CN 只读入口")).not.toBeInTheDocument();
    expect(screen.queryByText("同步目标")).not.toBeInTheDocument();
    expect(screen.queryByText("允许副作用")).not.toBeInTheDocument();
    expect(mockInvoke).not.toHaveBeenCalledWith("read_work_cn_state", expect.anything());
  });

  it("G7：总览证据带显示主库真实账号，不再走指纹检测链路", async () => {
    render(<App />);

    await waitFor(() => {
      expect(screen.getByTestId("current-account-context")).toHaveTextContent(
        "当前登录：主库账号",
      );
    });
    expect(screen.queryByTestId("redetect-account-button")).not.toBeInTheDocument();
    expect(screen.getByTestId("current-account-context")).not.toHaveTextContent("acct-1234");
  });

  it("G7：主库没有当前账号时显示未登录，不渲染内部状态术语", async () => {
    const unauthorizedWorkspace: WorkspaceStateDto = {
      ...workspace,
      current_account: {
        ...workspace.current_account,
        detected: false,
        user_fingerprint: null,
        unavailable_reason: "authorization_required",
      },
      capabilities: {
        ...workspace.capabilities,
        scan_enabled: true,
      },
      honest_status: "真实只读 Preview；等待用户授权后发现 TRAE 数据位置；TRAE 写入保持禁用。",
    };
    mockRoute("get_workspace_state", () => unauthorizedWorkspace);
    mockRoute("get_environment_state", () => ({
      env_id: "master",
      current_profile_id: null,
      current_account_name: null,
      observed_profile_id: null,
      data_dir: "C:\\TRAE\\master",
      running: false,
      login_state: "logged_out",
      created_at_unix_seconds: 1,
    }));

    render(<App />);

    expect(await screen.findByTestId("current-account-name")).toHaveTextContent("未登录");
    // 内部发布术语不进入用户界面（honest_status 已不再直接渲染）。
    expect(screen.queryByText(/RealReadPreview|真实只读 Preview/)).not.toBeInTheDocument();
  });

  it("初始化状态读取失败时提供可重试的工作台恢复动作", async () => {
    let shouldFail = true;
    mockRoute("get_workspace_state", () => {
      if (shouldFail) {
        throw new Error("IPC 暂时不可用");
      }
      return workspace;
    });

    render(<App />);

    expect(await screen.findByRole("alert")).toHaveTextContent("加载工作台状态失败");
    const retryButton = screen.getByRole("button", { name: "重新读取工作台状态" });

    shouldFail = false;
    fireEvent.click(retryButton);

    await waitFor(() => {
      expect(screen.getByTestId("current-account-context")).toBeInTheDocument();
    });
    // 只统计工作区状态读取；签到摘要等并行读取不计入。
    expect(
      mockInvoke.mock.calls.filter(([command]) => command === "get_workspace_state"),
    ).toHaveLength(2);
  });

  it("总览页有主库数据时显示统计卡，不同屏出现空态引导", async () => {
    mockRoute("get_workspace_state", () => workspace);
    mockRoute("get_checkin_capability", () => ({
      enabled: false,
      transport: "disabled",
      real_http_enabled: false,
      message: "签到能力当前不可读取。",
    }));
    mockRoute("get_master_library_stats", () => ({
      status: "ready",
      current_user_id: "u-1",
      project_count: 4,
      session_count: 16,
      message_count: 128,
      participating_account_count: 3,
      last_active_unix_seconds: null,
      size_bytes: 0,
    }));

    render(<App />);

    expect(await screen.findByTestId("overview-stats-master")).toBeInTheDocument();
    // 空态与统计互斥（G1）：有数据时不出空态行与环境页引导按钮。
    expect(screen.queryByTestId("overview-empty-hint")).not.toBeInTheDocument();
    expect(screen.queryByTestId("overview-master-cta")).not.toBeInTheDocument();
    expect(screen.getByTestId("overview-scan-cta")).toHaveTextContent("查看主库记录");

    // G8：总览主操作进入主库详情页，不再依赖已删除的独立历史工作区。
    fireEvent.click(screen.getByTestId("overview-scan-cta"));
    expect(await screen.findByRole("region", { name: "主库详情" })).toBeVisible();
  });

  it("总览页目录就绪但无对话时显示中性空态并引导去环境页", async () => {
    // beforeEach 默认 mock 即该场景：目录就绪 + 历史全 0 + 主库统计读取失败回落 null。
    render(<App />);

    expect(await screen.findByTestId("overview-empty-hint")).toHaveTextContent(
      "主库就绪，暂无对话",
    );
    expect(screen.getByTestId("overview-master-cta")).toHaveTextContent("去环境页查看主库");
    // 空态与统计互斥：无数据时不出统计卡与查看记录主操作。
    expect(screen.queryByTestId("overview-stats")).not.toBeInTheDocument();
    expect(screen.queryByTestId("overview-stats-master")).not.toBeInTheDocument();
    expect(screen.queryByTestId("overview-scan-cta")).not.toBeInTheDocument();
  });

  it("总览页目录缺失时只保留一行中性提示，不出统计与主操作", async () => {
    const noLocationWorkspace: WorkspaceStateDto = {
      ...workspace,
      data_location: {
        selected: false,
        display_name: null,
        unavailable_reason: "not_selected",
      },
    };
    mockRoute("get_workspace_state", () => noLocationWorkspace);
    mockRoute("get_checkin_capability", () => ({
      enabled: false,
      transport: "disabled",
      real_http_enabled: false,
      message: "签到能力当前不可读取。",
    }));

    render(<App />);

    expect(
      await screen.findByText("未找到 TRAE 数据目录，请确认 TRAE 已安装。"),
    ).toBeInTheDocument();
    // 异常态不加引导（G1）：不出统计、空态行与主操作按钮。
    expect(screen.queryByTestId("overview-stats")).not.toBeInTheDocument();
    expect(screen.queryByTestId("overview-stats-master")).not.toBeInTheDocument();
    expect(screen.queryByTestId("overview-empty-hint")).not.toBeInTheDocument();
    expect(screen.queryByTestId("overview-scan-cta")).not.toBeInTheDocument();
    expect(screen.queryByTestId("overview-master-cta")).not.toBeInTheDocument();
  });

  it("导航使用总览、账号、签到、环境、设置五个工作区", async () => {
    render(<App />);
    await screen.findByTestId("current-account-context");

    expect(screen.getByRole("button", { name: "总览" })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "历史" })).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "账号" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "签到" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "环境" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "设置" })).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "账号" }));
    expect(await screen.findByRole("region", { name: "账号" })).toBeVisible();
    await waitFor(() => {
      expect(screen.getByRole("heading", { name: "账号", level: 1 })).toHaveFocus();
    });

    // 签到是独立工作区：点击后区域可见且标题获得焦点，账号页隐藏。
    fireEvent.click(screen.getByRole("button", { name: "签到" }));
    expect(await screen.findByRole("region", { name: "签到" })).toBeVisible();
    await waitFor(() => {
      expect(screen.getByRole("heading", { name: "签到", level: 1 })).toHaveFocus();
    });

    // 环境是独立工作区（P5-2）：点击后区域可见且标题获得焦点。
    fireEvent.click(screen.getByRole("button", { name: "环境" }));
    expect(await screen.findByRole("region", { name: "环境" })).toBeVisible();
    await waitFor(() => {
      expect(screen.getByRole("heading", { name: "环境", level: 1 })).toHaveFocus();
    });
  });

  it("P17-T5 并列骨架：产品切换器在册，TRAE 产品五页全 ready", async () => {
    // 全量 mock：当前产品 trae_cn 的五页全部 ready，各自激活所需命令。
    mockRoute("get_workspace_state", () => workspace);
    mockRoute("get_environment_state", () => ({
      env_id: "master",
      current_profile_id: "profile-a",
      current_account_name: "主库账号",
      observed_profile_id: "profile-a",
      data_dir: "C:\\TRAE\\master",
      running: false,
      login_state: "logged_in",
      created_at_unix_seconds: 1,
    }));
    mockRoute("get_checkin_capability", () => ({
      enabled: false,
      transport: "disabled",
      real_http_enabled: false,
      message: "",
    }));
    mockRoute("get_master_library_stats", () => ({
      status: "ready",
      current_user_id: "u-1",
      project_count: 4,
      session_count: 16,
      message_count: 128,
      participating_account_count: 3,
      last_active_unix_seconds: null,
      size_bytes: 0,
    }));
    mockRoute("get_backup_retention", () => ({ enabled: true, keep: 5 }));
    mockRoute("get_auto_checkin_settings", () => ({ enabled: false, daily_time_hhmm: "09:00", ledger: null }));

    render(<App />);
    await screen.findByTestId("current-account-context");

    // 产品切换器在册且选中当前产品（P15-10 的「无切换器」断言已随并列骨架恢复）。
    const switcher = screen.getByRole("combobox", { name: "当前产品" });
    expect(switcher).toHaveValue("trae_cn");
    expect(within(switcher).getByRole("option", { name: "TRAE" })).toBeInTheDocument();
    expect(within(switcher).getByRole("option", { name: "WorkBuddy" })).toBeInTheDocument();

    // 导航五页全部渲染（master-library 仍不生成导航项，从详情入口进入）。
    for (const page of ["overview", "accounts", "checkin", "environment", "settings"]) {
      expect(screen.getByTestId(`navigation-${page}`)).toBeInTheDocument();
    }

    // overview = ready：问候语固定单产品展示名，证据带与统计卡正常。
    const overviewRegion = screen.getByRole("region", { name: "总览" });
    expect(
      within(overviewRegion).getByRole("heading", { level: 1, name: /TRAE 工作台$/ }),
    ).toBeInTheDocument();
    expect(within(overviewRegion).queryByTestId("capability-pending")).not.toBeInTheDocument();
    expect(within(overviewRegion).getByTestId("current-account-context")).toBeVisible();

    // accounts = ready：完整账号事务工作台（适配器路径不再由产品切换触达）。
    fireEvent.click(screen.getByTestId("navigation-accounts"));
    expect(await screen.findByRole("region", { name: "账号" })).toBeVisible();

    // checkin = ready：签到页渲染同一签到工作台，无 pending 说明卡。
    fireEvent.click(screen.getByTestId("navigation-checkin"));
    const checkinRegion = await screen.findByRole("region", { name: "签到" });
    expect(within(checkinRegion).queryByTestId("capability-pending")).not.toBeInTheDocument();

    // environment = ready：环境体系完整渲染（主库卡 + 辅助环境区）。
    fireEvent.click(screen.getByTestId("navigation-environment"));
    const envRegion = await screen.findByRole("region", { name: "环境" });
    expect(within(envRegion).queryByTestId("capability-absent")).not.toBeInTheDocument();
    expect(within(envRegion).getByTestId("env-master-card")).toBeInTheDocument();

    // settings = ready：设置页直接渲染完整管理面（无产品空态）。
    fireEvent.click(screen.getByTestId("navigation-settings"));
    const settingsRegion = await screen.findByRole("region", { name: "设置" });
    expect(await within(settingsRegion).findByTestId("auto-checkin-settings")).toBeInTheDocument();
    expect(within(settingsRegion).queryByTestId("product-settings-empty")).not.toBeInTheDocument();
  });

  it("P17-T5：切到 WorkBuddy 后账号/签到走 WorkBuddy 面板，未适配区块渲染 pending 占位", async () => {
    // 切换指针：写成功后重读回显新产品（组合根 get/set 语义的假实现）。
    let current = { product_id: "trae_cn", display_name: "TRAE" };
    mockRoute("get_current_product", () => current);
    mockRoute("set_current_product", (args: { productId: string }) => {
      current = args.productId === "workbuddy"
        ? { product_id: "workbuddy", display_name: "WorkBuddy" }
        : { product_id: "trae_cn", display_name: "TRAE" };
      return undefined;
    });
    // WorkBuddy 域命令回执（脱敏形态，与真实 DTO 同构）。
    mockRoute("get_workbuddy_session_health", () => ({
      status: "active",
      uid: "2079418751248068608",
      uin: "2079418751248068608",
      domain: "codebuddy.example",
      access_expires_at_unix_seconds: 1799999999,
      refresh_expires_at_unix_seconds: 1799999999,
      captured_in_pool: false,
    }));
    mockRoute("list_workbuddy_accounts", () => []);
    // 账号页健康卡的积分行（workbuddy_checkin_status 任选会话语义）。
    mockRoute("workbuddy_checkin_status", () => ({
      today_checked_in: false,
      streak_days: 6,
      daily_credit: 100,
      total_credits: 700,
    }));
    // P19-B5 签到页逐账号行：per-account 状态查询（池空则不发）。
    // 联调期 cast：命令契约已冻结、后端落地中（be-engineer B5），入册后摘除。
    mockRoute("workbuddy_checkin_status_for" as Parameters<typeof mockRoute>[0], () => ({
      today_checked_in: false,
      streak_days: 6,
      daily_credit: 100,
      total_credits: 700,
      checkin_dates: ["2026-10-20", "2026-10-19"],
    }));

    render(<App />);
    await screen.findByTestId("current-account-context");
    // 切换器随产品上下文异步挂载，先等它出现再操作。
    const switcher = await screen.findByRole("combobox", { name: "当前产品" });

    // 标题栏切到 WorkBuddy：写指针 → 重读回显 → 各槽位切换分支。
    fireEvent.change(switcher, { target: { value: "workbuddy" } });
    await waitFor(() => {
      expect(screen.getByRole("combobox", { name: "当前产品" })).toHaveValue("workbuddy");
    });

    // accounts = ready：WorkBuddy 面板（会话健康卡在册）。
    fireEvent.click(screen.getByTestId("navigation-accounts"));
    expect(await screen.findByTestId("workbuddy-health-card")).toBeInTheDocument();
    // P18-B3 后登录态由 G10 slot-badge 承载（WorkBuddyLoginSlotBadge，无独立 testid）。
    expect(screen.getByTestId("workbuddy-health-card")).toHaveTextContent("已登录");

    // checkin = ready：WorkBuddy 签到页（P19-B5 逐账号行形态，池空时历史条不渲染，
    // 但调度设置卡与页头骨架在册——断言骨架区块承载）。
    fireEvent.click(screen.getByTestId("navigation-checkin"));
    await waitFor(() => {
      expect(within(screen.getByRole("region", { name: "签到" })).getByTestId("product-badge-workbuddy")).toBeInTheDocument();
    });

    // environment = pending：未适配区块渲染诚实说明卡。
    // （overview/master-library 槽位同样持有 hidden 的 pending 卡，
    //   必须收在当前 region 内断言，避免多元素命中。）
    fireEvent.click(screen.getByTestId("navigation-environment"));
    const workbuddyEnvRegion = await screen.findByRole("region", { name: "环境" });
    expect(within(workbuddyEnvRegion).getByTestId("capability-pending")).toBeInTheDocument();

    // 切回 TRAE：账号页恢复完整账号工作台（重挂载重拉数据）。
    fireEvent.change(screen.getByRole("combobox", { name: "当前产品" }), {
      target: { value: "trae_cn" },
    });
    await waitFor(() => {
      expect(screen.getByRole("combobox", { name: "当前产品" })).toHaveValue("trae_cn");
    });
    fireEvent.click(screen.getByTestId("navigation-accounts"));
    expect(await screen.findByRole("region", { name: "账号" })).toBeVisible();
    expect(screen.queryByTestId("workbuddy-health-card")).not.toBeInTheDocument();
  });
});
