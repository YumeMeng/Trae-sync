import { beforeEach, describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import type { WorkspaceStateDto } from "../src/types/workspace";
import App from "../src/App";

// App 测试只模拟 Tauri command，确保测试不会访问真实 TRAE 数据。
vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(),
}));

const mockInvoke = vi.mocked(invoke);

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
      throw new Error(`未模拟的命令: ${command}`);
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
});
