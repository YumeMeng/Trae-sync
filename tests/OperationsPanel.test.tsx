import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { invoke, type InvokeArgs } from "@tauri-apps/api/core";
import {
  MOCK_OPERATION_LIST,
  MOCK_OPERATION_LOCK_STATUS,
  MOCK_OPERATION_PROGRESS,
} from "../e2e/mock-bridge";
import { OperationsPanel } from "../src/components/OperationsPanel";
import type { CapabilityFlagsDto } from "../src/types/workspace";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(),
}));

const { mockListen } = vi.hoisted(() => ({ mockListen: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({
  listen: mockListen,
}));

const mockInvoke = vi.mocked(invoke);
const readOnlyCapabilities: CapabilityFlagsDto = {
  scan_enabled: false,
  sync_enabled: false,
  backup_enabled: false,
  restore_enabled: false,
};
const safeSyncCapabilities: CapabilityFlagsDto = {
  scan_enabled: true,
  sync_enabled: true,
  backup_enabled: true,
  restore_enabled: false,
};

describe("操作与证据面板", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    mockListen.mockRejectedValue(new Error("事件插件不可用"));
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("读取操作列表、忙锁、不可取消进度和恢复结果", async () => {
    mockInvoke.mockImplementation(async (command: string) => {
      if (command === "list_operations") {
        return MOCK_OPERATION_LIST;
      }
      if (command === "get_operation_lock_status") {
        return MOCK_OPERATION_LOCK_STATUS;
      }
      if (command === "get_progress") {
        return MOCK_OPERATION_PROGRESS;
      }
      throw new Error(`未模拟的命令: ${command}`);
    });

    render(<OperationsPanel capabilities={readOnlyCapabilities} />);

    expect(await screen.findByTestId("operations-list")).toHaveTextContent("op-fixture");
    expect(screen.getByTestId("operations-list")).toHaveTextContent("op-fixture-restored");
    expect(screen.getByTestId("operations-list")).toHaveTextContent("恢复已验证");
    expect(screen.getByTestId("operation-lock-status")).toHaveTextContent("有操作进行中");
    expect(screen.getByTestId("operation-progress")).toHaveTextContent("正在写入");
    expect(screen.getByTestId("operation-progress")).toHaveTextContent("本阶段不可取消");
    expect(screen.queryByRole("button", { name: "查看备份" })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "手工恢复" })).not.toBeInTheDocument();
    expect(
      screen.queryByRole("button", { name: "重新核对未完成操作" }),
    ).not.toBeInTheDocument();
    expect(mockInvoke).toHaveBeenCalledWith("list_operations");
    expect(mockInvoke).toHaveBeenCalledWith("get_operation_lock_status");
    expect(mockInvoke).toHaveBeenCalledWith("get_progress", { operationId: "op-fixture" });
  });

  it("隔离副本存在非终态记录时可无参数核对并刷新状态", async () => {
    const activeOperation = { ...MOCK_OPERATION_LIST[0], state: "backing_up" as const };
    const terminalOperation = { ...activeOperation, state: "not_applied" as const, sequence: 3 };
    let listCallCount = 0;

    mockInvoke.mockImplementation(async (command: string) => {
      if (command === "list_operations") {
        listCallCount += 1;
        return listCallCount === 1 ? [activeOperation] : [terminalOperation];
      }
      if (command === "get_operation_lock_status") {
        return {
          ...MOCK_OPERATION_LOCK_STATUS,
          catalog_lock_held: false,
          data_location_lock_held: false,
        };
      }
      if (command === "get_progress") return null;
      if (command === "reconcile_unfinished_operations") {
        return {
          inspected_count: 1,
          reconciled_count: 1,
          not_applied_count: 1,
          completed_count: 0,
          manual_recovery_required_count: 0,
          unrelated_data_location_count: 0,
          status: "reconciled",
        };
      }
      throw new Error(`未模拟的命令: ${command}`);
    });

    render(<OperationsPanel capabilities={safeSyncCapabilities} />);

    const button = await screen.findByRole("button", { name: "重新核对未完成操作" });
    fireEvent.click(button);

    expect(await screen.findByTestId("operation-reconcile-result")).toHaveTextContent(
      "1 条确认未应用",
    );
    expect(mockInvoke).toHaveBeenCalledWith("reconcile_unfinished_operations");
    await waitFor(() => expect(screen.getByTestId("operations-list")).toHaveTextContent("未应用"));
    expect(
      screen.queryByRole("button", { name: "重新核对未完成操作" }),
    ).not.toBeInTheDocument();
  });

  it("协调进入人工恢复时明确警示且不显示成功", async () => {
    mockInvoke.mockImplementation(async (command: string) => {
      if (command === "list_operations") return [MOCK_OPERATION_LIST[0]];
      if (command === "get_operation_lock_status") return MOCK_OPERATION_LOCK_STATUS;
      if (command === "get_progress") return MOCK_OPERATION_PROGRESS;
      if (command === "reconcile_unfinished_operations") {
        return {
          inspected_count: 1,
          reconciled_count: 1,
          not_applied_count: 0,
          completed_count: 0,
          manual_recovery_required_count: 1,
          unrelated_data_location_count: 0,
          status: "manual_recovery_required",
        };
      }
      throw new Error(`未模拟的命令: ${command}`);
    });

    render(<OperationsPanel capabilities={safeSyncCapabilities} />);
    fireEvent.click(await screen.findByRole("button", { name: "重新核对未完成操作" }));

    const result = await screen.findByTestId("operation-reconcile-result");
    expect(result).toHaveAttribute("role", "alert");
    expect(result).toHaveTextContent("1 条需要人工恢复");
    expect(result).toHaveTextContent("请勿启动 TRAE");
    expect(result).not.toHaveTextContent("核对完成");
  });

  it("显示锁状态中的结构化关注原因", async () => {
    mockInvoke.mockImplementation(async (command: string) => {
      if (command === "list_operations") return [];
      if (command === "get_operation_lock_status") {
        return {
          ...MOCK_OPERATION_LOCK_STATUS,
          reason: "空间不足：请迁移或手工管理存储",
        };
      }
      if (command === "get_progress") return null;
      throw new Error(`未模拟的命令: ${command}`);
    });

    render(<OperationsPanel capabilities={readOnlyCapabilities} />);

    expect(await screen.findByTestId("operation-lock-status")).toHaveTextContent(
      "需要关注：空间不足：请迁移或手工管理存储",
    );
  });

  it("任一读取 command 不可用时保持只读并提示状态不可用", async () => {
    mockInvoke.mockRejectedValue(new Error("command unavailable"));

    render(<OperationsPanel capabilities={readOnlyCapabilities} />);

    expect(await screen.findByTestId("operations-unavailable")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "查看备份" })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "手工恢复" })).not.toBeInTheDocument();
  });

  it("即使能力标志误开也不显示没有处理器的备份和恢复按钮", async () => {
    mockInvoke.mockImplementation(async (command: string) => {
      if (command === "list_operations") return [];
      if (command === "get_operation_lock_status") return MOCK_OPERATION_LOCK_STATUS;
      if (command === "get_progress") return null;
      throw new Error(`未模拟的命令: ${command}`);
    });

    render(
      <OperationsPanel
        capabilities={{
          scan_enabled: true,
          sync_enabled: true,
          backup_enabled: true,
          restore_enabled: true,
        }}
      />,
    );

    await screen.findByTestId("operations-empty");
    expect(screen.queryByRole("button", { name: "查看备份" })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "手工恢复" })).not.toBeInTheDocument();
  });

  it("事件到达时更新进度并显示需要关注的结构化错误", async () => {
    const handlers = new Map<string, (event: { payload: Record<string, unknown> }) => void>();
    mockListen.mockImplementation(
      async (event: string, handler: (event: { payload: Record<string, unknown> }) => void) => {
        handlers.set(event, handler);
        return () => undefined;
      },
    );
    mockInvoke.mockImplementation(async (command: string) => {
      if (command === "list_operations") return MOCK_OPERATION_LIST;
      if (command === "get_operation_lock_status") return MOCK_OPERATION_LOCK_STATUS;
      if (command === "get_progress") return MOCK_OPERATION_PROGRESS;
      throw new Error(`未模拟的命令: ${command}`);
    });

    render(<OperationsPanel capabilities={readOnlyCapabilities} />);
    await screen.findByTestId("operations-list");
    await waitFor(() => expect(handlers.has("operation-progress")).toBe(true));

    act(() => {
      handlers.get("operation-progress")?.({
        payload: {
          ...MOCK_OPERATION_PROGRESS,
          phase: "verifying",
          cancellable: false,
        },
      });
      handlers.get("operation-needs-attention")?.({
        payload: {
          operation_id: "op-fixture",
          error: {
            code: "manual_recovery_required",
            message: "当前操作需要人工恢复。",
            recommended_action: "保留失败证据。",
            retryable: false,
          },
        },
      });
    });

    expect(screen.getByTestId("operation-progress")).toHaveTextContent("正在验证");
    expect(screen.getByTestId("operation-attention")).toHaveTextContent("当前操作需要人工恢复");

    act(() => {
      handlers.get("operation-finished")?.({
        payload: {
          operation_id: "op-fixture",
          outcome: { kind: "completed", affected_rows: 1 },
        },
      });
    });
    await waitFor(() => expect(screen.queryByTestId("operation-attention")).toBeNull());
  });

  it("单个事件监听失败时保留其他监听", async () => {
    const handlers = new Map<string, (event: { payload: Record<string, unknown> }) => void>();
    mockListen.mockImplementation(
      async (event: string, handler: (event: { payload: Record<string, unknown> }) => void) => {
        if (event === "operation-needs-attention") throw new Error("单个事件不可用");
        handlers.set(event, handler);
        return () => undefined;
      },
    );
    mockInvoke.mockImplementation(async (command: string) => {
      if (command === "list_operations") return MOCK_OPERATION_LIST;
      if (command === "get_operation_lock_status") return MOCK_OPERATION_LOCK_STATUS;
      if (command === "get_progress") return MOCK_OPERATION_PROGRESS;
      throw new Error(`未模拟的命令: ${command}`);
    });

    render(<OperationsPanel capabilities={readOnlyCapabilities} />);
    await screen.findByTestId("operations-list");
    await waitFor(() => expect(handlers.has("operation-progress")).toBe(true));

    act(() => {
      handlers.get("operation-progress")?.({
        payload: {
          ...MOCK_OPERATION_PROGRESS,
          phase: "verifying",
          cancellable: false,
        },
      });
    });
    expect(screen.getByTestId("operation-progress")).toHaveTextContent("正在验证");
  });

  it("忽略旧操作迟到的关注事件", async () => {
    const handlers = new Map<string, (event: { payload: Record<string, unknown> }) => void>();
    mockListen.mockImplementation(
      async (event: string, handler: (event: { payload: Record<string, unknown> }) => void) => {
        handlers.set(event, handler);
        return () => undefined;
      },
    );
    mockInvoke.mockImplementation(async (command: string) => {
      if (command === "list_operations") return MOCK_OPERATION_LIST;
      if (command === "get_operation_lock_status") return MOCK_OPERATION_LOCK_STATUS;
      if (command === "get_progress") return MOCK_OPERATION_PROGRESS;
      throw new Error(`未模拟的命令: ${command}`);
    });

    render(<OperationsPanel capabilities={readOnlyCapabilities} />);
    await screen.findByTestId("operations-list");
    await waitFor(() => expect(handlers.has("operation-needs-attention")).toBe(true));

    act(() => {
      handlers.get("operation-needs-attention")?.({
        payload: {
          operation_id: "op-old",
          error: {
            code: "manual_recovery_required",
            message: "旧操作提示",
            recommended_action: "忽略旧提示",
            retryable: false,
          },
        },
      });
    });
    // 旧操作提示会触发一次异步重查；等待重查完成，避免测试结束后遗留 React 状态更新。
    await waitFor(() => {
      expect(
        mockInvoke.mock.calls.filter(([command]) => command === "get_progress"),
      ).toHaveLength(2);
    });
    expect(screen.queryByTestId("operation-attention")).toBeNull();

    act(() => {
      handlers.get("operation-needs-attention")?.({
        payload: {
          operation_id: "op-fixture",
          error: {
            code: "manual_recovery_required",
            message: "当前操作提示",
            recommended_action: "保留失败证据",
            retryable: false,
          },
        },
      });
    });
    // 结构化错误只显示稳定安全文案，不把事件正文原样透传到界面。
    expect(screen.getByTestId("operation-attention")).toHaveTextContent("当前操作需要人工恢复");
    expect(screen.getByTestId("operation-attention")).not.toHaveTextContent("当前操作提示");
    expect(screen.getByTestId("operation-attention")).not.toHaveTextContent("保留失败证据");
  });

  it("没有活动操作时忽略迟到的进度事件", async () => {
    const handlers = new Map<string, (event: { payload: Record<string, unknown> }) => void>();
    mockListen.mockImplementation(
      async (event: string, handler: (event: { payload: Record<string, unknown> }) => void) => {
        handlers.set(event, handler);
        return () => undefined;
      },
    );
    mockInvoke.mockImplementation(async (command: string) => {
      if (command === "list_operations") return [];
      if (command === "get_operation_lock_status") return MOCK_OPERATION_LOCK_STATUS;
      if (command === "get_progress") return null;
      throw new Error(`未模拟的命令: ${command}`);
    });

    render(<OperationsPanel capabilities={readOnlyCapabilities} />);
    await screen.findByTestId("operations-empty");
    await waitFor(() => expect(handlers.has("operation-progress")).toBe(true));

    act(() => {
      handlers.get("operation-progress")?.({
        payload: {
          ...MOCK_OPERATION_PROGRESS,
          operation_id: "op-stale",
        },
      });
    });

    expect(screen.queryByTestId("operation-progress")).not.toBeInTheDocument();
  });

  it("按 sequence 选择当前操作，而不依赖后端返回顺序", async () => {
    const unorderedOperations = [...MOCK_OPERATION_LIST].reverse();
    mockInvoke.mockImplementation(async (command: string, args?: InvokeArgs) => {
      if (command === "list_operations") return unorderedOperations;
      if (command === "get_operation_lock_status") return MOCK_OPERATION_LOCK_STATUS;
      if (command === "get_progress") {
        expect(args).toEqual({ operationId: "op-fixture" });
        return MOCK_OPERATION_PROGRESS;
      }
      throw new Error(`未模拟的命令: ${command}`);
    });

    render(<OperationsPanel capabilities={readOnlyCapabilities} />);

    await screen.findByTestId("operations-list");
    expect(mockInvoke).toHaveBeenCalledWith("get_progress", { operationId: "op-fixture" });
  });

  it("优先选择当前锁定数据位置的活动操作", async () => {
    const operations = [
      {
        ...MOCK_OPERATION_LIST[0],
        operation_id: "op-other-location",
        data_location_id: "loc-other",
        sequence: 99,
      },
      {
        ...MOCK_OPERATION_LIST[0],
        operation_id: "op-current-location",
        data_location_id: "loc-fixture",
        sequence: 1,
      },
    ];

    mockInvoke.mockImplementation(async (command: string, args?: InvokeArgs) => {
      if (command === "list_operations") return operations;
      if (command === "get_operation_lock_status") {
        return { ...MOCK_OPERATION_LOCK_STATUS, data_location_id: "loc-fixture" };
      }
      if (command === "get_progress") {
        expect(args).toEqual({ operationId: "op-current-location" });
        return { ...MOCK_OPERATION_PROGRESS, operation_id: "op-current-location" };
      }
      throw new Error(`未模拟的命令: ${command}`);
    });

    render(<OperationsPanel capabilities={readOnlyCapabilities} />);

    await screen.findByTestId("operations-list");
    expect(mockInvoke).toHaveBeenCalledWith("get_progress", {
      operationId: "op-current-location",
    });
  });

  it("手工恢复终态仍读取并展示其保留的进度", async () => {
    const operations = [
      {
        ...MOCK_OPERATION_LIST[0],
        operation_id: "op-manual-recovery",
        state: "manual_recovery_required" as const,
        sequence: 4,
      },
      {
        ...MOCK_OPERATION_LIST[0],
        operation_id: "op-inconclusive",
        state: "verification_inconclusive" as const,
        sequence: 3,
      },
    ];

    mockInvoke.mockImplementation(async (command: string, args?: InvokeArgs) => {
      if (command === "list_operations") return operations;
      if (command === "get_operation_lock_status") {
        return { ...MOCK_OPERATION_LOCK_STATUS, catalog_lock_held: false, data_location_lock_held: false };
      }
      if (command === "get_progress") {
        expect(args).toEqual({ operationId: "op-manual-recovery" });
        return {
          operation_id: "op-manual-recovery",
          phase: "recovering" as const,
          completed_bytes: 768,
          total_bytes: 1024,
          percent_basis_points: 7500,
          cancellable: true,
        };
      }
      throw new Error(`未模拟的命令: ${command}`);
    });

    render(<OperationsPanel capabilities={readOnlyCapabilities} />);

    await waitFor(() => expect(screen.getByTestId("operation-progress")).toBeInTheDocument());
    expect(screen.getByTestId("operation-progress")).toHaveTextContent("正在恢复");
    // 百分比不再渲染为文本，改由进度条 aria 属性承载
    const progressBar = screen.getByRole("progressbar", { name: "操作进度" });
    expect(progressBar).toHaveAttribute("aria-valuenow", "75");
  });

  it("当前操作完成事件会立即触发状态重查", async () => {
    const handlers = new Map<string, (event: { payload: Record<string, unknown> }) => void>();
    const activeOperation = { ...MOCK_OPERATION_LIST[0], state: "target_writing" as const };
    const completedOperation = { ...activeOperation, state: "completed" as const };
    let listCallCount = 0;
    let resolveRefresh: ((value: typeof completedOperation[]) => void) | undefined;
    const refreshResult = new Promise<typeof completedOperation[]>((resolve) => {
      resolveRefresh = resolve;
    });

    mockListen.mockImplementation(
      async (event: string, handler: (event: { payload: Record<string, unknown> }) => void) => {
        handlers.set(event, handler);
        return () => undefined;
      },
    );
    mockInvoke.mockImplementation(async (command: string, args?: InvokeArgs) => {
      if (command === "list_operations") {
        listCallCount += 1;
        return listCallCount === 1 ? [activeOperation] : refreshResult;
      }
      if (command === "get_operation_lock_status") return MOCK_OPERATION_LOCK_STATUS;
      if (command === "get_progress") {
        const operationId =
          args && typeof args === "object" && !Array.isArray(args) && "operationId" in args
            ? args.operationId
            : undefined;
        return operationId === activeOperation.operation_id
          ? MOCK_OPERATION_PROGRESS
          : null;
      }
      throw new Error(`未模拟的命令: ${command}`);
    });

    render(<OperationsPanel capabilities={readOnlyCapabilities} />);
    await screen.findByTestId("operations-list");
    await waitFor(() => expect(handlers.has("operation-finished")).toBe(true));

    act(() => {
      handlers.get("operation-finished")?.({
        payload: {
          operation_id: activeOperation.operation_id,
          outcome: { kind: "completed", affected_rows: 1 },
        },
      });
    });

    await waitFor(() => expect(listCallCount).toBe(2), { timeout: 250 });
    resolveRefresh?.([completedOperation]);
    await waitFor(() => expect(screen.getByTestId("operations-list")).toHaveTextContent("已完成"));
  });

  it("已终态旧操作的关注事件不会污染当前操作", async () => {
    const handlers = new Map<string, (event: { payload: Record<string, unknown> }) => void>();
    mockListen.mockImplementation(
      async (event: string, handler: (event: { payload: Record<string, unknown> }) => void) => {
        handlers.set(event, handler);
        return () => undefined;
      },
    );
    mockInvoke.mockImplementation(async (command: string) => {
      if (command === "list_operations") return MOCK_OPERATION_LIST;
      if (command === "get_operation_lock_status") return MOCK_OPERATION_LOCK_STATUS;
      if (command === "get_progress") return MOCK_OPERATION_PROGRESS;
      throw new Error(`未模拟的命令: ${command}`);
    });

    render(<OperationsPanel capabilities={readOnlyCapabilities} />);
    await screen.findByTestId("operations-list");
    await waitFor(() => expect(handlers.has("operation-needs-attention")).toBe(true));

    act(() => {
      handlers.get("operation-needs-attention")?.({
        payload: {
          operation_id: "op-fixture-restored",
          error: {
            code: "manual_recovery_required",
            message: "旧操作提示",
            recommended_action: "忽略旧提示",
            retryable: false,
          },
        },
      });
    });

    expect(screen.queryByTestId("operation-attention")).toBeNull();
  });

  it("较早的刷新响应不能覆盖终态事件触发的新状态", async () => {
    const handlers = new Map<string, (event: { payload: Record<string, unknown> }) => void>();
    let resolveInitialList: ((value: typeof MOCK_OPERATION_LIST) => void) | undefined;
    const initialList = new Promise<typeof MOCK_OPERATION_LIST>((resolve) => {
      resolveInitialList = resolve;
    });
    const latestOperation = {
      ...MOCK_OPERATION_LIST[0],
      operation_id: "op-latest",
      sequence: 3,
    };
    let listCallCount = 0;

    mockListen.mockImplementation(
      async (event: string, handler: (event: { payload: Record<string, unknown> }) => void) => {
        handlers.set(event, handler);
        return () => undefined;
      },
    );
    mockInvoke.mockImplementation(async (command: string, args?: InvokeArgs) => {
      if (command === "list_operations") {
        listCallCount += 1;
        return listCallCount === 1 ? initialList : [latestOperation];
      }
      if (command === "get_operation_lock_status") return MOCK_OPERATION_LOCK_STATUS;
      if (command === "get_progress") {
        const operationId =
          args && typeof args === "object" && !Array.isArray(args) && "operationId" in args
            ? args.operationId
            : undefined;
        return operationId === "op-latest"
          ? { ...MOCK_OPERATION_PROGRESS, operation_id: "op-latest" }
          : MOCK_OPERATION_PROGRESS;
      }
      throw new Error(`未模拟的命令: ${command}`);
    });

    render(<OperationsPanel capabilities={readOnlyCapabilities} />);
    await waitFor(() => expect(handlers.has("operation-finished")).toBe(true));

    act(() => {
      handlers.get("operation-finished")?.({
        payload: {
          operation_id: "op-latest",
          outcome: { kind: "completed", affected_rows: 1 },
        },
      });
    });

    await waitFor(() => expect(screen.getByTestId("operations-list")).toHaveTextContent("op-latest"));
    resolveInitialList?.(MOCK_OPERATION_LIST);
    await new Promise((resolve) => setTimeout(resolve, 0));

    expect(screen.getByTestId("operations-list")).toHaveTextContent("op-latest");
    expect(
      mockInvoke.mock.calls.filter(([command]) => command === "get_progress"),
    ).toEqual([["get_progress", { operationId: "op-latest" }]]);
  });
  it("未知操作的关注事件会触发重查并保留提示", async () => {
    const handlers = new Map<string, (event: { payload: Record<string, unknown> }) => void>();
    const currentOperations = [MOCK_OPERATION_LIST[0]];
    const newOperation = {
      ...MOCK_OPERATION_LIST[0],
      operation_id: "op-new",
      sequence: 99,
      state: "manual_recovery_required" as const,
    };
    let listCallCount = 0;

    mockListen.mockImplementation(
      async (event: string, handler: (event: { payload: Record<string, unknown> }) => void) => {
        handlers.set(event, handler);
        return () => undefined;
      },
    );
    mockInvoke.mockImplementation(async (command: string, args?: InvokeArgs) => {
      if (command === "list_operations") {
        listCallCount += 1;
        return listCallCount === 1 ? currentOperations : [newOperation];
      }
      if (command === "get_operation_lock_status") return MOCK_OPERATION_LOCK_STATUS;
      if (command === "get_progress") {
        const operationId =
          args && typeof args === "object" && !Array.isArray(args) && "operationId" in args
            ? args.operationId
            : undefined;
        return operationId === "op-new"
          ? {
              ...MOCK_OPERATION_PROGRESS,
              operation_id: "op-new",
              phase: "recovering" as const,
              completed_bytes: 768,
              total_bytes: 1024,
              percent_basis_points: 7500,
              cancellable: true,
            }
          : MOCK_OPERATION_PROGRESS;
      }
      throw new Error(`未模拟命令: ${command}`);
    });

    render(<OperationsPanel capabilities={readOnlyCapabilities} />);
    await screen.findByTestId("operations-list");
    await waitFor(() => expect(handlers.has("operation-needs-attention")).toBe(true));

    act(() => {
      handlers.get("operation-needs-attention")?.({
        payload: {
          operation_id: "op-new",
          error: {
            code: "manual_recovery_required",
            message: "操作需要人工恢复",
            recommended_action: "保留失败证据",
            retryable: false,
          },
        },
      });
    });

    await waitFor(() => {
      expect(screen.getByTestId("operations-list")).toHaveTextContent("op-new");
      expect(screen.getByTestId("operation-attention")).toBeInTheDocument();
    });
    // manual_recovery_required 虽是后端终态，但若保留进度仍应显示，便于人工核对现场。
    expect(mockInvoke).toHaveBeenCalledWith("get_progress", { operationId: "op-new" });
    expect(screen.getByTestId("operation-progress")).toHaveTextContent("正在恢复");
  });

  it("未知操作先完成后收到关注事件时仍保留人工恢复提示", async () => {
    const handlers = new Map<string, (event: { payload: Record<string, unknown> }) => void>();
    let listCallCount = 0;
    mockListen.mockImplementation(
      async (event: string, handler: (event: { payload: Record<string, unknown> }) => void) => {
        handlers.set(event, handler);
        return () => undefined;
      },
    );
    mockInvoke.mockImplementation(async (command: string) => {
      if (command === "list_operations") {
        listCallCount += 1;
        return MOCK_OPERATION_LIST;
      }
      if (command === "get_operation_lock_status") return MOCK_OPERATION_LOCK_STATUS;
      if (command === "get_progress") return MOCK_OPERATION_PROGRESS;
      throw new Error(`未模拟命令: ${command}`);
    });

    render(<OperationsPanel capabilities={readOnlyCapabilities} />);
    await screen.findByTestId("operations-list");
    await waitFor(() => expect(handlers.has("operation-finished")).toBe(true));

    act(() => {
      handlers.get("operation-finished")?.({
        payload: {
          operation_id: "op-not-persisted-yet",
          outcome: { kind: "failed_safe", affected_rows: 0 },
        },
      });
    });
    act(() => {
      handlers.get("operation-needs-attention")?.({
        payload: {
          operation_id: "op-not-persisted-yet",
          error: {
            code: "manual_recovery_required",
            message: "操作需要人工恢复",
            recommended_action: "保留失败证据",
            retryable: false,
          },
        },
      });
    });

    await waitFor(() => expect(listCallCount).toBeGreaterThan(1));
    expect(screen.getByTestId("operation-attention")).toBeInTheDocument();
    expect(screen.getByTestId("operation-attention")).toHaveTextContent("当前操作需要人工恢复");
  });
});
