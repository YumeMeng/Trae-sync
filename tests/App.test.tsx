import { beforeEach, describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import type { WorkspaceStateDto } from "../src/types/workspace";
import App from "../src/App";

// App 测试只模拟 Tauri command，确保测试不会访问真实 TRAE 数据。
vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(),
}));

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
    display_name: "TRAE Work CN",
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
    mockInvoke.mockImplementation(async (command: string) => {
      if (command === "get_workspace_state") return workspace;
      if (command === "get_environment_state") {
        return {
          env_id: "master",
          current_profile_id: "profile-a",
          current_account_name: "主库账号",
          data_dir: "C:\\TRAE\\master",
          running: false,
          login_state: "logged_in",
          created_at_unix_seconds: 1,
        };
      }
      if (command === "get_checkin_capability") {
        return {
          enabled: false,
          transport: "disabled",
          real_http_enabled: false,
          message: "签到能力当前不可读取。",
        };
      }
      if (command === "get_product_identity_state") {
        return [
          {
            product_id: "work_cn",
            display_name: "TRAE Work CN",
            identity_status: "recognized",
            credential_status: "managed_by_work",
            relation_to_work: "current_product",
          },
          {
            product_id: "trae_cn",
            display_name: "Trae CN",
            identity_status: "recognized",
            credential_status: "need_authorization",
            relation_to_work: "different_identity",
          },
        ];
      }
      if (command === "list_trae_cn_accounts") return [];
      throw new Error(`未模拟的命令: ${command}`);
    });
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
      expect(notifyMock.summary).toHaveBeenCalledWith("今日自动签到完成（失败 1，跳过 1）", [], 2);
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
    mockInvoke.mockImplementation(async (command: string) => {
      if (command === "get_workspace_state") return unauthorizedWorkspace;
      if (command === "get_environment_state") {
        return {
          env_id: "master",
          current_profile_id: null,
          current_account_name: null,
          data_dir: "C:\\TRAE\\master",
          running: false,
          login_state: "logged_out",
          created_at_unix_seconds: 1,
        };
      }
      throw new Error(`未模拟的命令: ${command}`);
    });

    render(<App />);

    expect(await screen.findByTestId("current-account-name")).toHaveTextContent("未登录");
    // 内部发布术语不进入用户界面（honest_status 已不再直接渲染）。
    expect(screen.queryByText(/RealReadPreview|真实只读 Preview/)).not.toBeInTheDocument();
  });

  it("初始化状态读取失败时提供可重试的工作台恢复动作", async () => {
    let shouldFail = true;
    mockInvoke.mockImplementation(async (command: string) => {
      if (command === "get_workspace_state") {
        if (shouldFail) {
          throw new Error("IPC 暂时不可用");
        }
        return workspace;
      }
      throw new Error(`未模拟的命令: ${command}`);
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
    mockInvoke.mockImplementation(async (command: string) => {
      if (command === "get_workspace_state") return workspace;
      if (command === "get_checkin_capability") {
        return {
          enabled: false,
          transport: "disabled",
          real_http_enabled: false,
          message: "签到能力当前不可读取。",
        };
      }
      if (command === "get_master_library_stats") {
        return {
          status: "ready",
          current_user_id: "u-1",
          project_count: 4,
          session_count: 16,
          message_count: 128,
          participating_account_count: 3,
          last_active_unix_seconds: null,
          size_bytes: 0,
        };
      }
      throw new Error(`未模拟的命令: ${command}`);
    });

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
    mockInvoke.mockImplementation(async (command: string) => {
      if (command === "get_workspace_state") return noLocationWorkspace;
      if (command === "get_checkin_capability") {
        return {
          enabled: false,
          transport: "disabled",
          real_http_enabled: false,
          message: "签到能力当前不可读取。",
        };
      }
      throw new Error(`未模拟的命令: ${command}`);
    });

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

  it("P11-6（ADR-0032）：导航五页全产品开放，Trae CN 各页按区块三态呈现", async () => {
    // 全量 mock：ready 页激活所需命令；pending/absent 页不发起读取。
    mockInvoke.mockImplementation(async (command: string, args?: unknown) => {
      const libraryId = (args as { libraryId?: string } | undefined)?.libraryId;
      if (command === "get_workspace_state") return workspace;
      if (command === "get_environment_state") {
        return {
          env_id: "master",
          current_profile_id: "profile-a",
          current_account_name: "主库账号",
          data_dir: "C:\\TRAE\\master",
          running: false,
          login_state: "logged_in",
          created_at_unix_seconds: 1,
        };
      }
      if (command === "get_checkin_capability") {
        return { enabled: false, transport: "disabled", real_http_enabled: false, message: "" };
      }
      if (command === "get_product_identity_state") {
        return [
          {
            product_id: "work_cn",
            display_name: "TRAE Work CN",
            identity_status: "recognized",
            credential_status: "managed_by_work",
            relation_to_work: "current_product",
          },
          {
            product_id: "trae_cn",
            display_name: "Trae CN",
            identity_status: "recognized",
            credential_status: "need_authorization",
            relation_to_work: "different_identity",
          },
        ];
      }
      if (command === "list_trae_cn_accounts") return [];
      if (command === "get_master_library_stats") {
        if (libraryId === "trae_cn") {
          return {
            status: "ready",
            current_user_id: "u-t1",
            project_count: 1,
            session_count: 2,
            message_count: 5,
            participating_account_count: 1,
            last_active_unix_seconds: null,
            size_bytes: 0,
          };
        }
        throw new Error("未模拟的命令: get_master_library_stats");
      }
      if (command === "get_master_backup_chain") {
        if (libraryId === "trae_cn") {
          return { backups: [], backup_dir: "C:\\trae-cn", keep_policy: 5 };
        }
        return { backups: [], backup_dir: "C:\\bak", keep_policy: 5 };
      }
      if (command === "get_master_history") {
        if (libraryId === "trae_cn") {
          return {
            status: "ready",
            current_user_id: "u-t1",
            projects: [{ project_id: "p-t1", name: "Trae 项目", absolute_path: null }],
            sessions: [
              {
                session_id: "s-t1",
                project_id: "p-t1",
                title: "Trae 会话",
                message_count: 5,
                updated_at_unix_seconds: 1750000000,
                deleted: false,
                hidden_status: null,
                work_mode: null,
              },
            ],
            fingerprint: { db: { mtime_secs: 1, mtime_nanos: 0, size: 1 }, wal: null, shm: null },
          };
        }
        throw new Error("未模拟的命令: get_master_history");
      }
      if (command === "get_relay_ledger") return [];
      if (command === "get_master_session_messages") {
        return { session_id: "s-t1", status: "ready", messages: [] };
      }
      if (command === "get_backup_retention") return { enabled: true, keep: 5 };
      if (command === "get_auto_checkin_settings") {
        return { enabled: false, daily_time_hhmm: "09:00", ledger: null };
      }
      throw new Error(`未模拟的命令: ${command}`);
    });

    render(<App />);
    await screen.findByTestId("current-account-context");

    fireEvent.change(screen.getByRole("combobox", { name: "当前产品" }), {
      target: { value: "trae_cn" },
    });

    // 导航五页全产品渲染（master-library 仍不生成导航项，从浏览入口进入）。
    for (const page of ["overview", "accounts", "checkin", "environment", "settings"]) {
      expect(screen.getByTestId(`navigation-${page}`)).toBeInTheDocument();
    }

    // accounts = ready：Trae CN 账号工作台，不复用 Work CN 账号页。
    expect(await screen.findByRole("region", { name: "Trae CN 账号" })).toBeVisible();
    expect(screen.getByTestId("product-credential-status")).toHaveTextContent("需要授权");
    expect(screen.getByText("已发现账号材料")).toBeInTheDocument();
    expect(screen.queryByRole("region", { name: "账号" })).not.toBeInTheDocument();

    // overview = pending：问候语随产品变化，诚实说明卡 + 数据根浏览入口。
    fireEvent.click(screen.getByTestId("navigation-overview"));
    const overviewRegion = await screen.findByRole("region", { name: "总览" });
    // 签到页也有 pending 卡（页面槽位常驻 DOM），在总览 region 内定位。
    expect(within(overviewRegion).getByTestId("capability-pending")).toBeVisible();
    expect(
      within(overviewRegion).getByRole("heading", { level: 1, name: /Trae CN 工作台$/ }),
    ).toBeInTheDocument();
    // pending 形态同一槽位重渲染：Work CN 证据带与统计卡不再出现在总览内。
    expect(within(overviewRegion).queryByTestId("current-account-context")).not.toBeInTheDocument();
    expect(within(overviewRegion).queryByTestId("overview-stats-master")).not.toBeInTheDocument();

    // checkin = ready（P13-1 CN 签到实装）：App 级 mock 未配置 CN 账号 →
    // 空状态引导（操作界面按账号池存在性出现），不再渲染 pending 说明卡。
    fireEvent.click(screen.getByTestId("navigation-checkin"));
    const checkinRegion = await screen.findByRole("region", { name: "签到" });
    expect(within(checkinRegion).getByText(/还没有 Trae CN 账号/)).toBeInTheDocument();
    expect(within(checkinRegion).queryByTestId("capability-pending")).not.toBeInTheDocument();

    // environment = absent：环境体系是 Work CN 产品能力，整页说明卡。
    fireEvent.click(screen.getByTestId("navigation-environment"));
    const envRegion = await screen.findByRole("region", { name: "环境" });
    expect(within(envRegion).getByTestId("capability-absent")).toBeVisible();

    // settings = ready：工具级设置两产品通用。
    fireEvent.click(screen.getByTestId("navigation-settings"));
    expect(await screen.findByRole("region", { name: "设置" })).toBeVisible();

    // master-library = ready：Trae CN 数据根详情（总览浏览入口进入）。
    fireEvent.click(screen.getByTestId("navigation-overview"));
    fireEvent.click(await screen.findByTestId("overview-library-cta"));
    const detailRegion = await screen.findByRole("region", { name: "Trae CN 数据根详情" });
    expect(within(detailRegion).getByRole("heading", { level: 1 })).toHaveTextContent(
      "Trae CN 数据根",
    );
    // P13-3 archive-only：选择入口与快捷归档按库能力开放（原 P11-6 断言反转），
    // 历史列表正常挂载。
    expect(screen.getByTestId("library-select-mode")).toBeInTheDocument();
    fireEvent.click(screen.getByTestId("library-project-p-t1"));
    expect(await screen.findByTestId("library-session-s-t1")).toBeVisible();
    expect(screen.getByTestId("library-quick-archive-s-t1")).toBeInTheDocument();

    // 切回 Work CN：签到页恢复 ready 形态（pending 卡消失）。
    fireEvent.change(screen.getByRole("combobox", { name: "当前产品" }), {
      target: { value: "work_cn" },
    });
    fireEvent.click(screen.getByTestId("navigation-checkin"));
    const workCheckin = await screen.findByRole("region", { name: "签到" });
    expect(within(workCheckin).queryByTestId("capability-pending")).not.toBeInTheDocument();
  });
});
