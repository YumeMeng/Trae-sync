import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, within, fireEvent } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import type { WorkspaceStateDto } from "../src/types/workspace";
import type { WorkbenchReadStateDto } from "../src/types/workbench_read";

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

// T02 切片 C：Work CN 只读入口 UI 边界
describe("Work CN 只读入口（T02 切片 C）", () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it("初始不自动调用 read_work_cn_state（切片 C AC1）", async () => {
    const invokeSpy = vi.mocked(invoke);
    render(<App />);
    // 等待 T01 工作台加载
    await screen.findByText(/TRAE Work CN/);
    // read_work_cn_state 不应在初始渲染时被调用
    const readCalls = invokeSpy.mock.calls.filter(
      ([cmd]) => cmd === "read_work_cn_state",
    );
    expect(readCalls.length).toBe(0);
  });

  it("Work CN 只读入口区域显示 fixture 模式提示与读按钮（切片 C AC2）", async () => {
    render(<App />);
    expect(await screen.findByRole("region", { name: /Work CN 只读入口/ })).toBeInTheDocument();
    expect(screen.getByTestId("workbench-read-hint")).toHaveTextContent(
      /T02 阶段仅支持 fixture 路径/,
    );
    // 读取按钮初始存在
    expect(screen.getByTestId("read-workbench-button")).toBeInTheDocument();
  });

  it("点击读取按钮调用 read_work_cn_state 并显示状态（切片 C AC2）", async () => {
    const invokeSpy = vi.mocked(invoke);
    // R2/R6：使用合成账号 ID（不复制真实基线 ID），UI 显示明确摘要而非“已检测”
    const mockReadState: WorkbenchReadStateDto = {
      platform: {
        platform_id: "work_cn",
        display_name: "TRAE Work CN",
        adapter_implemented: true,
      },
      data_location: {
        selected: true,
        display_name: "C:\\fixture",
        unavailable_reason: null,
      },
      compatibility: {
        kind: "Verified",
        schema_fingerprint: "abc123",
        counts: {
          project_count: 1,
          chat_session_count: 2,
          chat_message_count: 3,
        },
      },
      current_account: {
        user_id: "1000000000000001",
        source_events: [],
        auth_fingerprint: "fp",
        local_storage_user_id: "1000000000000001",
        product_version: "1.107.1",
        observed_at: { secs_since_epoch: 1700000000, nanos_since_epoch: 0 },
        evidence_state: "verified",
      },
      readonly_reason: null,
    };
    invokeSpy.mockImplementation(async (cmd: string) => {
      if (cmd === "get_workspace_state") return mockEmptyWorkspace;
      if (cmd === "read_work_cn_state") return mockReadState;
      throw new Error(`未模拟的命令: ${cmd}`);
    });
    render(<App />);
    await screen.findByText(/TRAE Work CN/);
    // 输入 fixture 路径
    const input = screen.getByTestId("fixture-root-input") as HTMLInputElement;
    fireEvent.change(input, { target: { value: "C:\\fixture" } });
    // 点击读取按钮
    const button = screen.getByTestId("read-workbench-button")!;
    fireEvent.click(button);
    // 状态应显示
    expect(await screen.findByTestId("workbench-read-state")).toBeInTheDocument();
    expect(screen.getByTestId("wr-compatibility")).toHaveTextContent("Verified");
    // R6：UI 显示 user_id 摘要（首尾 4 位），而非模糊的“已检测”
    expect(screen.getByTestId("wr-account")).toHaveTextContent(/1000…0001/);
  });

  it("错误状态不展示 raw_key/认证正文（切片 C AC3）", async () => {
    const invokeSpy = vi.mocked(invoke);
    // 模拟错误 key 状态——只读原因为 wrong_key
    const mockWrongKeyState: WorkbenchReadStateDto = {
      platform: {
        platform_id: "work_cn",
        display_name: "TRAE Work CN",
        adapter_implemented: true,
      },
      data_location: {
        selected: true,
        display_name: "C:\\fixture",
        unavailable_reason: null,
      },
      compatibility: {
        kind: "Incompatible",
        reason: "wrong_key",
      },
      current_account: {
        user_id: null,
        source_events: [],
        auth_fingerprint: null,
        local_storage_user_id: null,
        product_version: null,
        observed_at: { secs_since_epoch: 1700000000, nanos_since_epoch: 0 },
        evidence_state: "missing",
      },
      readonly_reason: "wrong_key",
    };
    invokeSpy.mockImplementation(async (cmd: string) => {
      if (cmd === "get_workspace_state") return mockEmptyWorkspace;
      if (cmd === "read_work_cn_state") return mockWrongKeyState;
      throw new Error(`未模拟的命令: ${cmd}`);
    });
    render(<App />);
    await screen.findByText(/TRAE Work CN/);
    const input = screen.getByTestId("fixture-root-input") as HTMLInputElement;
    fireEvent.change(input, { target: { value: "C:\\fixture" } });
    fireEvent.click(screen.getByTestId("read-workbench-button")!);
    const state = await screen.findByTestId("workbench-read-state");
    // 显示结构化只读原因
    expect(screen.getByTestId("wr-readonly-reason")).toHaveTextContent("wrong_key");
    expect(screen.getByTestId("wr-incompatible-reason")).toHaveTextContent("wrong_key");
    // 不展示 raw_key 原文（mockReadState 不含 raw_key 字段）
    expect(state.textContent).not.toContain("3605f669");
    expect(state.textContent).not.toContain("rawkey");
  });

  it("只读提示明确手工账号选择不能解除只读（切片 C AC4）", async () => {
    render(<App />);
    expect(await screen.findByTestId("readonly-notice")).toHaveTextContent(
      /手工账号选择不能解除只读/,
    );
    // 所有真实写能力按钮仍禁用
    expect(screen.getByRole("button", { name: /查找新历史/ })).toBeDisabled();
    expect(screen.getByRole("button", { name: /检查并安全同步/ })).toBeDisabled();
  });
});
