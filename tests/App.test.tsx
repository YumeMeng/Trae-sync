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
      if (command === "refresh_managed_current_account") return undefined;
      if (command === "list_operations") return [];
      if (command === "get_operation_lock_status") {
        return {
          data_location_id: "location-1",
          catalog_lock_held: false,
          data_location_lock_held: false,
          write_allowed: false,
          reason: null,
        };
      }
      if (command === "get_progress") return null;
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

  it("历史页走主库直读，不存在授权扫描入口", async () => {
    render(<App />);

    expect(await screen.findByTestId("title-bar-mode")).toBeInTheDocument();
    // P5-3 起历史页直接读主库，授权勾选/扫描入口全部退役。
    expect(screen.queryByTestId("authorize-check")).not.toBeInTheDocument();
    expect(screen.queryByText("Work CN 只读入口")).not.toBeInTheDocument();
    expect(screen.queryByText("同步目标")).not.toBeInTheDocument();
    expect(screen.queryByText("允许副作用")).not.toBeInTheDocument();
    expect(mockInvoke).not.toHaveBeenCalledWith("read_work_cn_state", expect.anything());
  });

  it("标题栏持续显示账号和只读状态，并提供重新检测", async () => {
    render(<App />);

    expect(await screen.findByTestId("title-bar-mode")).toHaveTextContent("只读保护中");
    expect(screen.getByTestId("current-account-context")).toHaveTextContent(
      "当前账号 · acct-1234",
    );

    fireEvent.click(screen.getByTestId("redetect-account-button"));
    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith("refresh_managed_current_account");
    });
    expect(mockInvoke).toHaveBeenCalledWith("get_workspace_state");
  });

  it("账号刷新失败仍重新读取工作区并清除旧标题栏账号", async () => {
    let revoked = false;
    let workspaceReadCount = 0;
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
        scan_enabled: false,
      },
      honest_status: "读取未授权；等待用户授权后发现 TRAE 数据位置；TRAE 写入保持禁用。",
    };

    mockInvoke.mockImplementation(async (command: string) => {
      if (command === "get_workspace_state") {
        workspaceReadCount += 1;
        return revoked ? unauthorizedWorkspace : workspace;
      }
      if (command === "refresh_managed_current_account") {
        revoked = true;
        throw new Error("account_profile_store_busy");
      }
      if (command === "list_operations") return [];
      if (command === "get_operation_lock_status") {
        return {
          data_location_id: "location-1",
          catalog_lock_held: false,
          data_location_lock_held: false,
          write_allowed: false,
          reason: null,
        };
      }
      if (command === "get_progress") return null;
      throw new Error(`未模拟的命令: ${command}`);
    });

    render(<App />);
    await waitFor(() => {
      expect(screen.getByTestId("current-account-context")).toHaveTextContent(
        "当前账号 · acct-1234",
      );
    });

    fireEvent.click(screen.getByTestId("redetect-account-button"));

    // 账号失效会级联撤销历史读取授权，再触发一次权威工作区刷新。
    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith("refresh_managed_current_account");
      expect(screen.getByTestId("current-account-context")).toHaveTextContent(
        "未检测",
      );
      expect(screen.getByTestId("current-account-context")).not.toHaveTextContent("acct-1234");
      expect(screen.getByTestId("title-bar-mode")).toHaveTextContent("读取未授权");
    });
    expect(workspaceReadCount).toBeGreaterThanOrEqual(2);
  });

  it("读取授权未建立时显示读取未授权，不误导为账号重新检测", async () => {
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
      if (command === "list_operations") return [];
      if (command === "get_operation_lock_status") {
        return {
          data_location_id: "location-1",
          catalog_lock_held: false,
          data_location_lock_held: false,
          write_allowed: false,
          reason: null,
        };
      }
      if (command === "get_progress") return null;
      throw new Error(`未模拟的命令: ${command}`);
    });

    render(<App />);

    expect(await screen.findByTestId("title-bar-mode")).toHaveTextContent("读取未授权");
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
      expect(screen.getByTestId("title-bar-mode")).toBeInTheDocument();
    });
    // 只统计工作区状态读取；总览页的活动面板会并行轮询其他只读命令。
    expect(
      mockInvoke.mock.calls.filter(([command]) => command === "get_workspace_state"),
    ).toHaveLength(2);
  });

  it("导航使用总览、历史、账号、签到、环境、设置六个工作区", async () => {
    render(<App />);
    await screen.findByTestId("title-bar-mode");

    expect(screen.getByRole("button", { name: "总览" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "历史" })).toBeInTheDocument();
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
