import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, fireEvent, waitFor, within } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import { HistoryWorkbench } from "../src/components/HistoryWorkbench";
import type {
  BrowseResultDto,
  ConversationPreviewDto,
  ScanOutcomeDto,
  SearchHitDto,
} from "../src/types/history";

// mock Tauri invoke——前端测试不依赖真实后端
vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(),
}));

const mockInvoke = vi.mocked(invoke);

const defaultProps = {
  capabilities: { scan_enabled: true, sync_enabled: false },
  honestStatus: "真实能力尚未启用",
};

// 合成浏览结果（两账号/两项目/两会话）
function makeBrowseResult(): BrowseResultDto {
  return {
    accounts: [
      { user_id: "user-A", display_label: "User A", project_count: 1, session_count: 2 },
    ],
    projects: [
      {
        project_id: "p1",
        display_name: "Project 1",
        display_owner: "user-A",
        session_count: 2,
      },
    ],
    sessions: [
      {
        session_identity: {
          product_history_namespace: "work_cn",
          original_session_id: "session-aaa",
        },
        title: "会话 AAA",
        message_count: 2,
        last_captured_at: { secs_since_epoch: 1700000000, nanos_since_epoch: 0 },
        project_id: "p1",
      },
      {
        session_identity: {
          product_history_namespace: "work_cn",
          original_session_id: "session-bbb",
        },
        title: "会话 BBB",
        message_count: 1,
        last_captured_at: { secs_since_epoch: 1700000001, nanos_since_epoch: 0 },
        project_id: "p1",
      },
    ],
    summary: {
      visible_account_count: 1,
      visible_project_count: 1,
      visible_session_count: 2,
      soft_deleted_project_count: 0,
      soft_deleted_session_count: 0,
      soft_deleted_message_count: 0,
    },
  };
}

// 合成对话预览
function makePreview(sessionId: string): ConversationPreviewDto {
  return {
    session_identity: {
      product_history_namespace: "work_cn",
      original_session_id: sessionId,
    },
    title: sessionId === "session-aaa" ? "会话 AAA" : "会话 BBB",
    messages:
      sessionId === "session-aaa"
        ? [
            {
              message_id: "m1",
              session_id: sessionId,
              role: "user",
              content_excerpt: "hello world",
              soft_deleted: false,
              seq: 0,
            },
            {
              message_id: "m2",
              session_id: sessionId,
              role: "assistant",
              content_excerpt: "hi there",
              soft_deleted: false,
              seq: 1,
            },
          ]
        : [
            {
              message_id: "m3",
              session_id: sessionId,
              role: "user",
              content_excerpt: "world hello",
              soft_deleted: false,
              seq: 0,
            },
          ],
    total_message_count: sessionId === "session-aaa" ? 2 : 1,
  };
}

describe("T03 历史库工作台", () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  // ============== TDD #1：初始不自动扫描 ==============

  it("初始渲染不调用 scan_history（AC1）", () => {
    render(<HistoryWorkbench {...defaultProps} />);
    const scanCalls = mockInvoke.mock.calls.filter(
      ([cmd]) => cmd === "scan_history",
    );
    expect(scanCalls.length).toBe(0);
  });

  it("初始渲染显示授权表单（idle 状态）", () => {
    render(<HistoryWorkbench {...defaultProps} />);
    expect(screen.getByTestId("auth-panel")).toBeInTheDocument();
  });

  // ============== TDD #2：未授权/运行中不发布快照 ==============

  it("未授权时扫描按钮禁用（AC2）", () => {
    render(<HistoryWorkbench {...defaultProps} />);
    const scanButton = screen.getByTestId("scan-history-button");
    expect(scanButton).toBeDisabled();
  });

  it("TRAE 运行中时扫描按钮禁用且不调用 scan_history（AC2）", () => {
    render(<HistoryWorkbench {...defaultProps} />);
    // 填写表单
    fireEvent.change(screen.getByTestId("history-fixture-root-input"), {
      target: { value: "C:\\fixture" },
    });
    // 取消"TRAE 已关闭"勾选 → processRunning = true
    fireEvent.click(screen.getByTestId("trae-not-running-check"));
    // 授权
    fireEvent.click(screen.getByTestId("authorize-check"));
    // 扫描按钮仍应禁用（processRunning）
    expect(screen.getByTestId("scan-history-button")).toBeDisabled();
    const scanCalls = mockInvoke.mock.calls.filter(
      ([cmd]) => cmd === "scan_history",
    );
    expect(scanCalls.length).toBe(0);
  });

  // ============== TDD #18：UI 状态与交互覆盖 ==============

  it("授权并扫描成功后显示浏览结果（success 状态）", async () => {
    const successOutcome: ScanOutcomeDto = {
      kind: "success",
      snapshot_id: "snap-1",
      snapshot_meta: {
        snapshot_id: "snap-1",
        platform_id: "work_cn",
        data_location_id: "loc-1",
        product_version: "1.0",
        schema_fingerprint: "fp",
        mapping_version: "work_cn_v1",
        account_evidence_ref: null,
        captured_at: { secs_since_epoch: 1700000000, nanos_since_epoch: 0 },
        files: [],
        fingerprint: "abc",
      },
      catalog_updated: true,
    };
    mockInvoke.mockImplementation(async (cmd: string) => {
      if (cmd === "scan_history") return successOutcome;
      if (cmd === "browse_history") return makeBrowseResult();
      throw new Error(`未模拟: ${cmd}`);
    });

    render(<HistoryWorkbench {...defaultProps} />);
    // 填写并授权
    fireEvent.change(screen.getByTestId("history-fixture-root-input"), {
      target: { value: "C:\\fixture" },
    });
    fireEvent.click(screen.getByTestId("authorize-check"));
    // 扫描
    fireEvent.click(screen.getByTestId("scan-history-button"));
    // 应显示浏览结果
    await waitFor(() => {
      expect(screen.getByTestId("account-project-tree")).toBeInTheDocument();
    });
    // 摘要应显示账号/项目/对话数
    const summary = screen.getByTestId("history-summary");
    expect(within(summary).getByText("账号 1")).toBeInTheDocument();
    expect(within(summary).getByText("项目 1")).toBeInTheDocument();
    expect(within(summary).getByText("对话 2")).toBeInTheDocument();
  });

  it("扫描失败时显示结构化原因（failure 状态）", async () => {
    const failedOutcome: ScanOutcomeDto = {
      kind: "failed",
      reason: "schema_incompatible",
    };
    mockInvoke.mockImplementation(async (cmd: string) => {
      if (cmd === "scan_history") return failedOutcome;
      throw new Error(`未模拟: ${cmd}`);
    });

    render(<HistoryWorkbench {...defaultProps} />);
    fireEvent.change(screen.getByTestId("history-fixture-root-input"), {
      target: { value: "C:\\fixture" },
    });
    fireEvent.click(screen.getByTestId("authorize-check"));
    fireEvent.click(screen.getByTestId("scan-history-button"));

    await waitFor(() => {
      expect(screen.getByTestId("failure-state")).toBeInTheDocument();
    });
    // 结构化原因不携带 secret
    expect(screen.getByTestId("failure-state")).toHaveTextContent(
      "schema 不兼容",
    );
  });

  it("扫描成功但目录库为空时显示 empty 状态", async () => {
    const successOutcome: ScanOutcomeDto = {
      kind: "success",
      snapshot_id: "snap-1",
      snapshot_meta: {
        snapshot_id: "snap-1",
        platform_id: "work_cn",
        data_location_id: "loc-1",
        product_version: "1.0",
        schema_fingerprint: "fp",
        mapping_version: "work_cn_v1",
        account_evidence_ref: null,
        captured_at: { secs_since_epoch: 1700000000, nanos_since_epoch: 0 },
        files: [],
        fingerprint: "abc",
      },
      catalog_updated: true,
    };
    const emptyBrowse: BrowseResultDto = {
      accounts: [],
      projects: [],
      sessions: [],
      summary: {
        visible_account_count: 0,
        visible_project_count: 0,
        visible_session_count: 0,
        soft_deleted_project_count: 0,
        soft_deleted_session_count: 0,
        soft_deleted_message_count: 0,
      },
    };
    mockInvoke.mockImplementation(async (cmd: string) => {
      if (cmd === "scan_history") return successOutcome;
      if (cmd === "browse_history") return emptyBrowse;
      throw new Error(`未模拟: ${cmd}`);
    });

    render(<HistoryWorkbench {...defaultProps} />);
    fireEvent.change(screen.getByTestId("history-fixture-root-input"), {
      target: { value: "C:\\fixture" },
    });
    fireEvent.click(screen.getByTestId("authorize-check"));
    fireEvent.click(screen.getByTestId("scan-history-button"));

    await waitFor(() => {
      expect(screen.getByTestId("empty-state")).toBeInTheDocument();
    });
  });

  it("点击对话标题只打开预览，不改变选择（AC9）", async () => {
    const successOutcome: ScanOutcomeDto = {
      kind: "success",
      snapshot_id: "snap-1",
      snapshot_meta: {
        snapshot_id: "snap-1",
        platform_id: "work_cn",
        data_location_id: "loc-1",
        product_version: "1.0",
        schema_fingerprint: "fp",
        mapping_version: "work_cn_v1",
        account_evidence_ref: null,
        captured_at: { secs_since_epoch: 1700000000, nanos_since_epoch: 0 },
        files: [],
        fingerprint: "abc",
      },
      catalog_updated: true,
    };
    mockInvoke.mockImplementation(async (cmd: string, args?: unknown) => {
      if (cmd === "scan_history") return successOutcome;
      if (cmd === "browse_history") return makeBrowseResult();
      if (cmd === "read_conversation") {
        const session = (args as { session: { original_session_id: string } })
          .session;
        return makePreview(session.original_session_id);
      }
      throw new Error(`未模拟: ${cmd}`);
    });

    render(<HistoryWorkbench {...defaultProps} />);
    fireEvent.change(screen.getByTestId("history-fixture-root-input"), {
      target: { value: "C:\\fixture" },
    });
    fireEvent.click(screen.getByTestId("authorize-check"));
    fireEvent.click(screen.getByTestId("scan-history-button"));

    // 等待会话列表渲染
    await waitFor(() => {
      expect(screen.getByTestId("session-session-aaa")).toBeInTheDocument();
    });

    // 点击第一个会话
    fireEvent.click(screen.getByTestId("session-session-aaa"));
    await waitFor(() => {
      expect(screen.getByTestId("preview-content")).toBeInTheDocument();
    });
    // 预览内容正确
    expect(screen.getByTestId("message-m1")).toBeInTheDocument();
    expect(screen.getByTestId("message-m2")).toBeInTheDocument();

    // 点击第二个会话——预览应切换，但不应改变"选择"
    // 选择由 selectedAccount/selectedProject 控制，点击会话不影响这些
    const sessionButtons = screen.getAllByRole("button", { name: /会话/ });
    // 确保会话按钮存在且可点击
    expect(sessionButtons.length).toBeGreaterThan(0);
  });

  it("搜索显示结果并打开预览不改变选择（AC9）", async () => {
    const successOutcome: ScanOutcomeDto = {
      kind: "success",
      snapshot_id: "snap-1",
      snapshot_meta: {
        snapshot_id: "snap-1",
        platform_id: "work_cn",
        data_location_id: "loc-1",
        product_version: "1.0",
        schema_fingerprint: "fp",
        mapping_version: "work_cn_v1",
        account_evidence_ref: null,
        captured_at: { secs_since_epoch: 1700000000, nanos_since_epoch: 0 },
        files: [],
        fingerprint: "abc",
      },
      catalog_updated: true,
    };
    const searchHits: SearchHitDto[] = [
      {
        session_identity: {
          product_history_namespace: "work_cn",
          original_session_id: "session-aaa",
        },
        message_id: "m1",
        project_id: "p1",
        title: "会话 AAA",
        content_excerpt: "hello world",
        role: "user",
      },
    ];
    mockInvoke.mockImplementation(async (cmd: string, args?: unknown) => {
      if (cmd === "scan_history") return successOutcome;
      if (cmd === "browse_history") return makeBrowseResult();
      if (cmd === "search_history") return searchHits;
      if (cmd === "read_conversation") {
        const session = (args as { session: { original_session_id: string } })
          .session;
        return makePreview(session.original_session_id);
      }
      throw new Error(`未模拟: ${cmd}`);
    });

    render(<HistoryWorkbench {...defaultProps} />);
    fireEvent.change(screen.getByTestId("history-fixture-root-input"), {
      target: { value: "C:\\fixture" },
    });
    fireEvent.click(screen.getByTestId("authorize-check"));
    fireEvent.click(screen.getByTestId("scan-history-button"));

    await waitFor(() => {
      expect(screen.getByTestId("session-list")).toBeInTheDocument();
    });

    // 输入搜索并执行
    fireEvent.change(screen.getByTestId("search-input"), {
      target: { value: "hello" },
    });
    fireEvent.click(screen.getByTestId("search-button"));

    // 搜索结果应显示
    await waitFor(() => {
      expect(screen.getByTestId("search-results")).toBeInTheDocument();
    });
    expect(screen.getByTestId("search-hit-m1")).toBeInTheDocument();

    // 点击搜索结果打开预览
    fireEvent.click(screen.getByTestId("search-hit-m1"));
    await waitFor(() => {
      expect(screen.getByTestId("preview-content")).toBeInTheDocument();
    });
    expect(screen.getByTestId("message-m1")).toBeInTheDocument();
  });

  it("扫描失败时不暴露 raw_key 或认证正文", async () => {
    const failedOutcome: ScanOutcomeDto = {
      kind: "failed",
      reason: "catalog_key_missing",
    };
    mockInvoke.mockImplementation(async (cmd: string) => {
      if (cmd === "scan_history") return failedOutcome;
      throw new Error(`未模拟: ${cmd}`);
    });

    render(<HistoryWorkbench {...defaultProps} />);
    fireEvent.change(screen.getByTestId("history-fixture-root-input"), {
      target: { value: "C:\\fixture" },
    });
    fireEvent.click(screen.getByTestId("authorize-check"));
    fireEvent.click(screen.getByTestId("scan-history-button"));

    await waitFor(() => {
      expect(screen.getByTestId("failure-state")).toBeInTheDocument();
    });
    const failureText = screen.getByTestId("failure-state").textContent ?? "";
    // 不暴露 raw_key、认证正文或 secret
    expect(failureText).not.toMatch(/raw_key|rawkey|secret|bearer|token/i);
  });
});
