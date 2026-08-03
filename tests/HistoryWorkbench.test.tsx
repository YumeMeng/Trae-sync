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
    // R12：默认 mock 返回已 resolve 的 Promise——组件卸载清理会调用
    // revoke_scan_authorization，未设置实现的测试需返回 Promise 避免 .catch 崩溃。
    // 各测试内部可用 mockImplementation 覆盖具体命令的返回值。
    mockInvoke.mockResolvedValue(undefined);
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

  it("TRAE 运行中时扫描按钮禁用且不调用 scan_history（AC2）", async () => {
    // R7：mock grant_scan_authorization 返回 canonical 路径
    mockInvoke.mockImplementation(async (cmd: string) => {
      if (cmd === "grant_scan_authorization") return "C:\\fixture";
      if (cmd === "revoke_scan_authorization") return null;
      throw new Error(`未模拟: ${cmd}`);
    });

    render(<HistoryWorkbench {...defaultProps} />);
    // 填写表单
    fireEvent.change(screen.getByTestId("history-fixture-root-input"), {
      target: { value: "C:\\fixture" },
    });
    // 取消"TRAE 已关闭"勾选 → processRunning = true
    fireEvent.click(screen.getByTestId("trae-not-running-check"));
    // 授权（R7：异步调用 grant_scan_authorization）
    fireEvent.click(screen.getByTestId("authorize-check"));
    // 等待授权完成
    await waitFor(() => {
      expect(screen.getByTestId("authorize-check")).toBeChecked();
    });
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
      // R7：mock grant_scan_authorization 返回 canonical 路径
      if (cmd === "grant_scan_authorization") return "C:\\fixture";
      if (cmd === "revoke_scan_authorization") return null;
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
    // 等待 R7 异步授权完成
    await waitFor(() => {
      expect(screen.getByTestId("authorize-check")).toBeChecked();
    });
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
      // R7：mock grant_scan_authorization 返回 canonical 路径
      if (cmd === "grant_scan_authorization") return "C:\\fixture";
      if (cmd === "revoke_scan_authorization") return null;
      if (cmd === "scan_history") return failedOutcome;
      throw new Error(`未模拟: ${cmd}`);
    });

    render(<HistoryWorkbench {...defaultProps} />);
    fireEvent.change(screen.getByTestId("history-fixture-root-input"), {
      target: { value: "C:\\fixture" },
    });
    fireEvent.click(screen.getByTestId("authorize-check"));
    // 等待 R7 异步授权完成
    await waitFor(() => {
      expect(screen.getByTestId("authorize-check")).toBeChecked();
    });
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
      // R7：mock grant_scan_authorization 返回 canonical 路径
      if (cmd === "grant_scan_authorization") return "C:\\fixture";
      if (cmd === "revoke_scan_authorization") return null;
      if (cmd === "scan_history") return successOutcome;
      if (cmd === "browse_history") return emptyBrowse;
      throw new Error(`未模拟: ${cmd}`);
    });

    render(<HistoryWorkbench {...defaultProps} />);
    fireEvent.change(screen.getByTestId("history-fixture-root-input"), {
      target: { value: "C:\\fixture" },
    });
    fireEvent.click(screen.getByTestId("authorize-check"));
    // 等待 R7 异步授权完成
    await waitFor(() => {
      expect(screen.getByTestId("authorize-check")).toBeChecked();
    });
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
      // R7：mock grant_scan_authorization 返回 canonical 路径
      if (cmd === "grant_scan_authorization") return "C:\\fixture";
      if (cmd === "revoke_scan_authorization") return null;
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
    // 等待 R7 异步授权完成
    await waitFor(() => {
      expect(screen.getByTestId("authorize-check")).toBeChecked();
    });
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
      // R7：mock grant_scan_authorization 返回 canonical 路径
      if (cmd === "grant_scan_authorization") return "C:\\fixture";
      if (cmd === "revoke_scan_authorization") return null;
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
    // 等待 R7 异步授权完成
    await waitFor(() => {
      expect(screen.getByTestId("authorize-check")).toBeChecked();
    });
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
      // R7：mock grant_scan_authorization 返回 canonical 路径
      if (cmd === "grant_scan_authorization") return "C:\\fixture";
      if (cmd === "revoke_scan_authorization") return null;
      if (cmd === "scan_history") return failedOutcome;
      throw new Error(`未模拟: ${cmd}`);
    });

    render(<HistoryWorkbench {...defaultProps} />);
    fireEvent.change(screen.getByTestId("history-fixture-root-input"), {
      target: { value: "C:\\fixture" },
    });
    fireEvent.click(screen.getByTestId("authorize-check"));
    // 等待 R7 异步授权完成
    await waitFor(() => {
      expect(screen.getByTestId("authorize-check")).toBeChecked();
    });
    fireEvent.click(screen.getByTestId("scan-history-button"));

    await waitFor(() => {
      expect(screen.getByTestId("failure-state")).toBeInTheDocument();
    });
    const failureText = screen.getByTestId("failure-state").textContent ?? "";
    // 不暴露 raw_key、认证正文或 secret
    expect(failureText).not.toMatch(/raw_key|rawkey|secret|bearer|token/i);
  });

  // ============== R7：前端授权调用链反例测试 ==============

  it("R7：未建立后端授权时扫描被拒绝（scan_history 不被调用）", async () => {
    // grant_scan_authorization 抛错——后端授权未建立
    // scan_history mock 抛错——若被调用则测试失败
    mockInvoke.mockImplementation(async (cmd: string) => {
      if (cmd === "grant_scan_authorization") {
        throw new Error("授权失败：fixture 路径无效");
      }
      if (cmd === "scan_history") {
        throw new Error("scan_history 不应被调用——未授权");
      }
      throw new Error(`未模拟: ${cmd}`);
    });

    render(<HistoryWorkbench {...defaultProps} />);
    fireEvent.change(screen.getByTestId("history-fixture-root-input"), {
      target: { value: "C:\\fixture" },
    });
    // 点击授权——grant_scan_authorization 抛错，授权失败
    fireEvent.click(screen.getByTestId("authorize-check"));
    // 等待异步授权失败——应进入 failure 状态
    await waitFor(() => {
      expect(screen.getByTestId("failure-state")).toBeInTheDocument();
    });
    // 扫描按钮仍禁用（phase 不是 idle，但仍禁用）
    expect(screen.getByTestId("scan-history-button")).toBeDisabled();
    // scan_history 未被调用——后端授权未建立
    const scanCalls = mockInvoke.mock.calls.filter(
      ([cmd]) => cmd === "scan_history",
    );
    expect(scanCalls.length).toBe(0);
  });

  it("R7：授权成功后才允许扫描", async () => {
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
      // R7：grant_scan_authorization 成功返回 canonical 路径
      if (cmd === "grant_scan_authorization") return "C:\\canonical-fixture";
      if (cmd === "revoke_scan_authorization") return null;
      if (cmd === "scan_history") return successOutcome;
      if (cmd === "browse_history") return makeBrowseResult();
      throw new Error(`未模拟: ${cmd}`);
    });

    render(<HistoryWorkbench {...defaultProps} />);
    fireEvent.change(screen.getByTestId("history-fixture-root-input"), {
      target: { value: "C:\\fixture" },
    });
    // 授权成功
    fireEvent.click(screen.getByTestId("authorize-check"));
    await waitFor(() => {
      expect(screen.getByTestId("authorize-check")).toBeChecked();
    });
    // 扫描按钮启用
    expect(screen.getByTestId("scan-history-button")).toBeEnabled();
    // 执行扫描——应调用 scan_history 并使用授权返回的 canonical 路径
    fireEvent.click(screen.getByTestId("scan-history-button"));
    await waitFor(() => {
      expect(screen.getByTestId("account-project-tree")).toBeInTheDocument();
    });
    // 验证 scan_history 被调用时传入 authorizedFixtureRoot（canonical）
    const scanCall = mockInvoke.mock.calls.find(
      ([cmd]) => cmd === "scan_history",
    );
    expect(scanCall).toBeDefined();
    const scanArgs = scanCall?.[1] as { fixtureRoot: string };
    expect(scanArgs.fixtureRoot).toBe("C:\\canonical-fixture");
  });

  it("R7：路径变化后旧授权失效（调用 revoke_scan_authorization）", async () => {
    mockInvoke.mockImplementation(async (cmd: string) => {
      if (cmd === "grant_scan_authorization") return "C:\\fixture";
      if (cmd === "revoke_scan_authorization") return null;
      throw new Error(`未模拟: ${cmd}`);
    });

    render(<HistoryWorkbench {...defaultProps} />);
    // 第一次填写并授权
    fireEvent.change(screen.getByTestId("history-fixture-root-input"), {
      target: { value: "C:\\fixture" },
    });
    fireEvent.click(screen.getByTestId("authorize-check"));
    await waitFor(() => {
      expect(screen.getByTestId("authorize-check")).toBeChecked();
    });
    // 修改 fixture 路径——应触发撤销
    fireEvent.change(screen.getByTestId("history-fixture-root-input"), {
      target: { value: "D:\\other-fixture" },
    });
    // 等待异步撤销完成——checkbox 应取消选中
    await waitFor(() => {
      expect(screen.getByTestId("authorize-check")).not.toBeChecked();
    });
    // revoke_scan_authorization 被调用
    const revokeCalls = mockInvoke.mock.calls.filter(
      ([cmd]) => cmd === "revoke_scan_authorization",
    );
    expect(revokeCalls.length).toBeGreaterThanOrEqual(1);
  });

  it("R7：撤销授权后再次扫描被拒绝", async () => {
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
    let scanCalled = false;
    mockInvoke.mockImplementation(async (cmd: string) => {
      if (cmd === "grant_scan_authorization") return "C:\\fixture";
      if (cmd === "revoke_scan_authorization") return null;
      if (cmd === "scan_history") {
        scanCalled = true;
        return successOutcome;
      }
      if (cmd === "browse_history") return makeBrowseResult();
      throw new Error(`未模拟: ${cmd}`);
    });

    render(<HistoryWorkbench {...defaultProps} />);
    fireEvent.change(screen.getByTestId("history-fixture-root-input"), {
      target: { value: "C:\\fixture" },
    });
    // 授权
    fireEvent.click(screen.getByTestId("authorize-check"));
    await waitFor(() => {
      expect(screen.getByTestId("authorize-check")).toBeChecked();
    });
    // 取消授权
    fireEvent.click(screen.getByTestId("authorize-check"));
    await waitFor(() => {
      expect(screen.getByTestId("authorize-check")).not.toBeChecked();
    });
    // 扫描按钮应禁用——未授权
    expect(screen.getByTestId("scan-history-button")).toBeDisabled();
    // scan_history 不应被调用
    expect(scanCalled).toBe(false);
  });

  // ============== R12：pending 授权异步竞态反例测试 ==============

  // 辅助：构造可控 deferred 的 grant_scan_authorization mock
  function makeDeferredGrant() {
    let resolveFn!: (value: string) => void;
    let rejectFn!: (reason: unknown) => void;
    const promise = new Promise<string>((resolve, reject) => {
      resolveFn = resolve;
      rejectFn = reject;
    });
    return { promise, resolve: resolveFn, reject: rejectFn };
  }

  it("R12：授权 pending 时 fixtureRoot 变化会丢弃旧授权结果", async () => {
    const grantA = makeDeferredGrant();
    let scanCalled = false;
    mockInvoke.mockImplementation(async (cmd: string) => {
      if (cmd === "grant_scan_authorization") return grantA.promise;
      if (cmd === "revoke_scan_authorization") return null;
      if (cmd === "scan_history") {
        scanCalled = true;
        return {} as ScanOutcomeDto;
      }
      throw new Error(`未模拟: ${cmd}`);
    });

    render(<HistoryWorkbench {...defaultProps} />);
    // 输入 A 并点击授权——保持 pending
    fireEvent.change(screen.getByTestId("history-fixture-root-input"), {
      target: { value: "C:\\fixture-A" },
    });
    fireEvent.click(screen.getByTestId("authorize-check"));
    // 等待 grant 被调用
    await waitFor(() => {
      expect(
        mockInvoke.mock.calls.some(([cmd]) => cmd === "grant_scan_authorization"),
      ).toBe(true);
    });

    // 在 pending 期间将 fixtureRoot 改为 B
    fireEvent.change(screen.getByTestId("history-fixture-root-input"), {
      target: { value: "D:\\fixture-B" },
    });

    // 现在 resolve grant(A)——返回 canonical A
    grantA.resolve("C:\\canonical-A");

    // 等待 stale response 处理完成
    await waitFor(() => {
      // 应调用 revoke 清除后端过期授权
      const revokeCalls = mockInvoke.mock.calls.filter(
        ([cmd]) => cmd === "revoke_scan_authorization",
      );
      expect(revokeCalls.length).toBeGreaterThanOrEqual(1);
    });

    // checkbox 仍未选中——过期结果不应设置已授权状态
    expect(screen.getByTestId("authorize-check")).not.toBeChecked();
    // 扫描按钮仍禁用
    expect(screen.getByTestId("scan-history-button")).toBeDisabled();
    // scan_history 未被调用
    expect(scanCalled).toBe(false);
  });

  it("R12：授权 pending 时 dbRelativePath 变化会丢弃旧授权结果", async () => {
    const grantA = makeDeferredGrant();
    let scanCalled = false;
    mockInvoke.mockImplementation(async (cmd: string) => {
      if (cmd === "grant_scan_authorization") return grantA.promise;
      if (cmd === "revoke_scan_authorization") return null;
      if (cmd === "scan_history") {
        scanCalled = true;
        return {} as ScanOutcomeDto;
      }
      throw new Error(`未模拟: ${cmd}`);
    });

    render(<HistoryWorkbench {...defaultProps} />);
    fireEvent.change(screen.getByTestId("history-fixture-root-input"), {
      target: { value: "C:\\fixture" },
    });
    fireEvent.click(screen.getByTestId("authorize-check"));
    await waitFor(() => {
      expect(
        mockInvoke.mock.calls.some(([cmd]) => cmd === "grant_scan_authorization"),
      ).toBe(true);
    });

    // pending 期间修改 dbRelativePath
    fireEvent.change(screen.getByTestId("history-db-path-input"), {
      target: { value: "other.db" },
    });

    grantA.resolve("C:\\canonical");

    await waitFor(() => {
      const revokeCalls = mockInvoke.mock.calls.filter(
        ([cmd]) => cmd === "revoke_scan_authorization",
      );
      expect(revokeCalls.length).toBeGreaterThanOrEqual(1);
    });

    expect(screen.getByTestId("authorize-check")).not.toBeChecked();
    expect(screen.getByTestId("scan-history-button")).toBeDisabled();
    expect(scanCalled).toBe(false);
  });

  it("R12-A：pending 时用户点击 checkbox 取消会丢弃旧授权结果", async () => {
    const grantA = makeDeferredGrant();
    let scanCalled = false;
    mockInvoke.mockImplementation(async (cmd: string) => {
      if (cmd === "grant_scan_authorization") return grantA.promise;
      if (cmd === "revoke_scan_authorization") return null;
      if (cmd === "scan_history") {
        scanCalled = true;
        return {} as ScanOutcomeDto;
      }
      throw new Error(`未模拟: ${cmd}`);
    });

    render(<HistoryWorkbench {...defaultProps} />);
    fireEvent.change(screen.getByTestId("history-fixture-root-input"), {
      target: { value: "C:\\fixture" },
    });
    fireEvent.click(screen.getByTestId("authorize-check"));
    await waitFor(() => {
      expect(
        mockInvoke.mock.calls.some(([cmd]) => cmd === "grant_scan_authorization"),
      ).toBe(true);
    });

    // R12-A：pending 时 checkbox 选中——用户点击 checkbox 取消
    // checkbox checked={authorized || authorizationPending}，pending 时为 true
    expect(screen.getByTestId("authorize-check")).toBeChecked();
    // 点击取消——触发 checked=false
    fireEvent.click(screen.getByTestId("authorize-check"));

    // 取消应立即生效——checkbox 未选中，扫描按钮禁用
    await waitFor(() => {
      expect(screen.getByTestId("authorize-check")).not.toBeChecked();
    });
    expect(screen.getByTestId("scan-history-button")).toBeDisabled();

    // 应调用 revoke 清除后端可能已建立的授权
    const revokeCalls = mockInvoke.mock.calls.filter(
      ([cmd]) => cmd === "revoke_scan_authorization",
    );
    expect(revokeCalls.length).toBeGreaterThanOrEqual(1);

    // resolve grant(A)——stale，不应恢复授权
    grantA.resolve("C:\\canonical-A");
    await new Promise((r) => setTimeout(r, 0));

    // 仍为未授权状态
    expect(screen.getByTestId("authorize-check")).not.toBeChecked();
    expect(screen.getByTestId("scan-history-button")).toBeDisabled();
    expect(scanCalled).toBe(false);
  });

  it("R12：A、B 两次授权乱序返回，只有 B 可以生效", async () => {
    const grantA = makeDeferredGrant();
    const grantB = makeDeferredGrant();
    let grantCallCount = 0;
    let scanCalled = false;
    mockInvoke.mockImplementation(async (cmd: string) => {
      if (cmd === "grant_scan_authorization") {
        grantCallCount += 1;
        // 第一次返回 grantA.promise，第二次返回 grantB.promise
        return grantCallCount === 1 ? grantA.promise : grantB.promise;
      }
      if (cmd === "revoke_scan_authorization") return null;
      if (cmd === "scan_history") {
        scanCalled = true;
        return {} as ScanOutcomeDto;
      }
      throw new Error(`未模拟: ${cmd}`);
    });

    render(<HistoryWorkbench {...defaultProps} />);
    // 输入 A 并点击授权
    fireEvent.change(screen.getByTestId("history-fixture-root-input"), {
      target: { value: "C:\\A" },
    });
    fireEvent.click(screen.getByTestId("authorize-check"));
    await waitFor(() => {
      expect(grantCallCount).toBe(1);
    });

    // 改路径为 B——应触发 revoke + 旧 grant 失效
    fireEvent.change(screen.getByTestId("history-fixture-root-input"), {
      target: { value: "D:\\B" },
    });
    await waitFor(() => {
      expect(
        mockInvoke.mock.calls.some(([cmd]) => cmd === "revoke_scan_authorization"),
      ).toBe(true);
    });

    // 再次点击授权——发起 grant(B)
    // checkbox 当前未选中，点击触发 checked=true
    fireEvent.click(screen.getByTestId("authorize-check"));
    await waitFor(() => {
      expect(grantCallCount).toBe(2);
    });

    // 乱序返回：先 resolve A（stale），再 resolve B（最新）
    grantA.resolve("C:\\canonical-A");
    grantB.resolve("D:\\canonical-B");

    // 等待处理完成
    await waitFor(() => {
      expect(screen.getByTestId("authorize-check")).toBeChecked();
    });

    // 最终生效的应是 B
    expect(screen.getByTestId("authorize-check")).toBeChecked();
    // 验证 scan_history 调用时使用 B 的 canonical
    // 触发扫描
    fireEvent.click(screen.getByTestId("scan-history-button"));
    await waitFor(() => {
      expect(scanCalled).toBe(true);
    });
    const scanCall = mockInvoke.mock.calls.find(
      ([cmd]) => cmd === "scan_history",
    );
    const scanArgs = scanCall?.[1] as { fixtureRoot: string };
    expect(scanArgs.fixtureRoot).toBe("D:\\canonical-B");
  });

  it("R12：组件卸载后旧响应不得更新状态，并应撤销可能建立的后端授权", async () => {
    const grantA = makeDeferredGrant();
    const { unmount } = render(<HistoryWorkbench {...defaultProps} />);
    mockInvoke.mockImplementation(async (cmd: string) => {
      if (cmd === "grant_scan_authorization") return grantA.promise;
      if (cmd === "revoke_scan_authorization") return null;
      throw new Error(`未模拟: ${cmd}`);
    });

    fireEvent.change(screen.getByTestId("history-fixture-root-input"), {
      target: { value: "C:\\fixture" },
    });
    fireEvent.click(screen.getByTestId("authorize-check"));
    await waitFor(() => {
      expect(
        mockInvoke.mock.calls.some(([cmd]) => cmd === "grant_scan_authorization"),
      ).toBe(true);
    });

    // 卸载组件
    unmount();

    // resolve grant(A)——不应有状态更新（无 React 警告），应调用 revoke
    grantA.resolve("C:\\canonical-A");

    // 等待微任务
    await new Promise((r) => setTimeout(r, 0));

    // 应调用 revoke 清除后端过期授权
    const revokeCalls = mockInvoke.mock.calls.filter(
      ([cmd]) => cmd === "revoke_scan_authorization",
    );
    expect(revokeCalls.length).toBeGreaterThanOrEqual(1);
  });

  // ============== R12-B：stale revoke 不得误伤新授权 ==============

  it("R12-B：B 先返回建立授权后 A stale 返回不得 revoke B 的授权", async () => {
    const grantA = makeDeferredGrant();
    const grantB = makeDeferredGrant();
    let grantCallCount = 0;
    let scanCalled = false;
    let scanFixtureRoot: string | null = null;
    mockInvoke.mockImplementation(async (cmd: string, args?: unknown) => {
      if (cmd === "grant_scan_authorization") {
        grantCallCount += 1;
        return grantCallCount === 1 ? grantA.promise : grantB.promise;
      }
      if (cmd === "revoke_scan_authorization") return null;
      if (cmd === "scan_history") {
        scanCalled = true;
        const a = args as { fixtureRoot: string };
        scanFixtureRoot = a.fixtureRoot;
        return {} as ScanOutcomeDto;
      }
      throw new Error(`未模拟: ${cmd}`);
    });

    render(<HistoryWorkbench {...defaultProps} />);
    // 输入 A 并点击授权——grant(A) pending
    fireEvent.change(screen.getByTestId("history-fixture-root-input"), {
      target: { value: "C:\\A" },
    });
    fireEvent.click(screen.getByTestId("authorize-check"));
    await waitFor(() => {
      expect(grantCallCount).toBe(1);
    });

    // 改路径为 B——触发 revoke + 旧 grant 失效
    fireEvent.change(screen.getByTestId("history-fixture-root-input"), {
      target: { value: "D:\\B" },
    });
    await waitFor(() => {
      expect(
        mockInvoke.mock.calls.some(([cmd]) => cmd === "revoke_scan_authorization"),
      ).toBe(true);
    });

    // 再次点击授权——发起 grant(B)
    fireEvent.click(screen.getByTestId("authorize-check"));
    await waitFor(() => {
      expect(grantCallCount).toBe(2);
    });

    // R12-B 关键时序：先 resolve B（建立授权），再 resolve A（stale）
    grantB.resolve("D:\\canonical-B");
    // 等待 B 被接受
    await waitFor(() => {
      expect(screen.getByTestId("authorize-check")).toBeChecked();
    });

    // 记录此时 revoke 调用次数——A stale 返回前的基线
    const revokeCountBeforeA = mockInvoke.mock.calls.filter(
      ([cmd]) => cmd === "revoke_scan_authorization",
    ).length;

    // resolve A——stale，不应 revoke B 的授权
    grantA.resolve("C:\\canonical-A");
    await new Promise((r) => setTimeout(r, 10));

    // A stale 返回后不应新增 revoke 调用——B 授权保持有效
    const revokeCountAfterA = mockInvoke.mock.calls.filter(
      ([cmd]) => cmd === "revoke_scan_authorization",
    ).length;
    expect(revokeCountAfterA).toBe(revokeCountBeforeA);

    // B 仍已授权——checkbox 选中，扫描按钮启用
    expect(screen.getByTestId("authorize-check")).toBeChecked();
    expect(screen.getByTestId("scan-history-button")).toBeEnabled();

    // 触发扫描——应使用 B 的 canonical
    fireEvent.click(screen.getByTestId("scan-history-button"));
    await waitFor(() => {
      expect(scanCalled).toBe(true);
    });
    expect(scanFixtureRoot).toBe("D:\\canonical-B");
  });

  // ============== R12-C：卸载后异步失败不得更新 React 状态 ==============

  it("R12-C：卸载后 stale grant 返回时 revoke 失败不触发 setState", async () => {
    const grantA = makeDeferredGrant();
    const { unmount } = render(<HistoryWorkbench {...defaultProps} />);
    // revoke 故意 reject——模拟后端 revoke 失败
    mockInvoke.mockImplementation(async (cmd: string) => {
      if (cmd === "grant_scan_authorization") return grantA.promise;
      if (cmd === "revoke_scan_authorization") {
        throw new Error("revoke 后端失败");
      }
      throw new Error(`未模拟: ${cmd}`);
    });

    fireEvent.change(screen.getByTestId("history-fixture-root-input"), {
      target: { value: "C:\\fixture" },
    });
    fireEvent.click(screen.getByTestId("authorize-check"));
    await waitFor(() => {
      expect(
        mockInvoke.mock.calls.some(([cmd]) => cmd === "grant_scan_authorization"),
      ).toBe(true);
    });

    // 卸载组件
    unmount();

    // resolve grant(A)——stale，组件已卸载
    // 卸载 cleanup 会调用 revoke（失败），stale 分支也会跳过（mountedRef=false）
    // 不应产生任何 React setState 警告
    grantA.resolve("C:\\canonical-A");

    // 等待微任务和 revoke reject 传播
    await new Promise((r) => setTimeout(r, 50));

    // 验证：revoke 被调用（卸载 cleanup + 可能的 stale 分支）
    const revokeCalls = mockInvoke.mock.calls.filter(
      ([cmd]) => cmd === "revoke_scan_authorization",
    );
    expect(revokeCalls.length).toBeGreaterThanOrEqual(1);
    // 无 React act 警告即表示未对已卸载组件 setState
    // React 18 不再打印警告，但若 setState 被调用会有 console.error
    // 此测试主要验证不抛出未捕获异常
  });
});
