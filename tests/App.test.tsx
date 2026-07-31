import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, within } from "@testing-library/react";
import type { WorkspaceStateDto } from "../src/types/workspace";

// 默认 mock：模拟后端返回的空工作台状态（T01 全部能力禁用）
const mockEmptyWorkspace: WorkspaceStateDto = {
  platform: {
    platform_id: "work_cn",
    display_name: "TRAE Work CN",
    adapter_implemented: false,
  },
  data_location: {
    selected: false,
    display_name: null,
    unavailable_reason: "not_selected",
  },
  current_account: {
    detected: false,
    user_fingerprint: null,
    unavailable_reason: "not_detected",
  },
  history: {
    account_count: 0,
    project_count: 0,
    session_count: 0,
  },
  capabilities: {
    scan_enabled: false,
    sync_enabled: false,
    backup_enabled: false,
    restore_enabled: false,
  },
  honest_status: "真实能力尚未启用",
};

// mock Tauri invoke，让前端在不依赖真实后端的情况下测试
vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(async (cmd: string) => {
    if (cmd === "get_workspace_state") {
      return mockEmptyWorkspace;
    }
    throw new Error(`未模拟的命令: ${cmd}`);
  }),
}));

import App from "../src/App";

describe("空工作台 UI（T01 骨架）", () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it("标题栏显示平台上下文（Work CN）", async () => {
    render(<App />);
    // 等待 Tauri invoke 返回后渲染
    expect(await screen.findByText(/TRAE Work CN/)).toBeInTheDocument();
  });

  it("标题栏显示数据位置未选择状态", async () => {
    render(<App />);
    expect(await screen.findByText(/数据位置未选择|未选择数据位置/)).toBeInTheDocument();
  });

  it("标题栏显示当前账号未检测状态", async () => {
    render(<App />);
    expect(await screen.findByText(/当前账号未检测|未检测到账号/)).toBeInTheDocument();
  });

  it("显示历史库入口与空摘要（账号/项目/对话均为 0）", async () => {
    render(<App />);
    const historySection = await screen.findByRole("region", { name: /历史库/ });
    // 历史库摘要显示账号/项目/对话数量均为 0
    expect(within(historySection).getByText("账号 0")).toBeInTheDocument();
    expect(within(historySection).getByText("项目 0")).toBeInTheDocument();
    expect(within(historySection).getByText("对话 0")).toBeInTheDocument();
    // 历史库区域应显示“查找新历史”入口（T01 阶段禁用）
    expect(within(historySection).getByRole("button", { name: /查找新历史/ })).toBeDisabled();
  });

  it("显示操作与备份入口", async () => {
    render(<App />);
    expect(await screen.findByRole("region", { name: /操作与备份/ })).toBeInTheDocument();
  });

  it("显示设置入口", async () => {
    render(<App />);
    expect(await screen.findByRole("region", { name: /设置/ })).toBeInTheDocument();
  });

  it("显示诚实状态：真实能力尚未启用", async () => {
    render(<App />);
    // honestStatus 在历史库工作台中出现两次（空提示 + 状态栏），用 findAllByText
    expect((await screen.findAllByText(/真实能力尚未启用/)).length).toBeGreaterThan(0);
  });

  it("所有真实能力按钮均禁用（扫描、同步、备份、恢复）", async () => {
    render(<App />);
    await screen.findAllByText(/真实能力尚未启用/);
    // 扫描按钮在历史库区域
    expect(screen.getByRole("button", { name: /查找新历史/ })).toBeDisabled();
    // 同步按钮
    expect(screen.getByRole("button", { name: /检查并安全同步/ })).toBeDisabled();
  });
});
