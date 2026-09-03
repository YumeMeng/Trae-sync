import { beforeEach, describe, expect, it, vi } from "vitest";
import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import { HistoryWorkbench } from "../src/components/HistoryWorkbench";
import type {
  MasterHistoryDto,
  RelayLedgerEntryDto,
} from "../src/types/history";

// mock Tauri invoke——前端测试不依赖真实后端
vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(),
}));

const mockInvoke = vi.mocked(invoke);

const NOW = Date.now() / 1000;
const DAY = 24 * 3600;

function fingerprint(tag: number) {
  return {
    db: { mtime_secs: 1700000000 + tag, mtime_nanos: 0, size: 1000 + tag },
    wal: null,
    shm: null,
  };
}

function historyDto(overrides: Partial<MasterHistoryDto> = {}): MasterHistoryDto {
  return {
    status: "ready",
    current_user_id: "u-b",
    projects: [
      { project_id: "p1", name: "项目一", absolute_path: "d:\\work\\project-one" },
      { project_id: "p2", name: "项目二", absolute_path: null },
    ],
    sessions: [
      {
        session_id: "s1",
        project_id: "p1",
        title: "会话一",
        message_count: 10,
        updated_at_unix_seconds: NOW - 60,
        deleted: false,
        hidden_status: null,
        work_mode: "code",
      },
      {
        session_id: "s2",
        project_id: "p1",
        title: "会话二",
        message_count: 3,
        updated_at_unix_seconds: NOW - 40 * DAY,
        deleted: false,
        hidden_status: null,
        work_mode: "code",
      },
      {
        // P5-8a：voice_discussion 借用为归档 → 不入主列表，进归档视图。
        session_id: "s3",
        project_id: "p2",
        title: "旧归档会话",
        message_count: 1,
        updated_at_unix_seconds: NOW - 40 * DAY,
        deleted: false,
        hidden_status: "voice_discussion",
        work_mode: "work",
      },
    ],
    fingerprint: fingerprint(1),
    ...overrides,
  };
}

/** 两跳台账：s0(u-x→u-a) → s1(u-a→u-b)；s1 链回出 3 条腿。 */
function ledgerEntries(): RelayLedgerEntryDto[] {
  return [
    {
      session_id: "s0",
      from_session_id: null,
      project_id: "p1",
      from_user_id: "u-x",
      from_account_name: "小谢",
      to_user_id: "u-a",
      to_account_name: "账号A",
      message_count_at_switch: 4,
      switched_at_unix_seconds: Math.floor(NOW - 2 * 3600),
    },
    {
      session_id: "s1",
      from_session_id: "s0",
      project_id: "p1",
      from_user_id: "u-a",
      from_account_name: "账号A",
      to_user_id: "u-b",
      to_account_name: "账号B",
      message_count_at_switch: 7,
      switched_at_unix_seconds: Math.floor(NOW - 3600),
    },
  ];
}

function sessionMessages(sessionId: string) {
  return {
    session_id: sessionId,
    status: "ready",
    messages: [
      {
        message_id: "m1",
        role: "user",
        message_type: "general",
        created_at_unix_seconds: Math.floor(NOW - 120),
        content: { kind: "text", text: "帮我看下这个报错", step_count: 0, thoughts: [] },
      },
      {
        message_id: "m2",
        role: "assistant",
        message_type: "task",
        created_at_unix_seconds: Math.floor(NOW - 60),
        content: { kind: "task_trace", text: "", step_count: 2, thoughts: ["先定位", "再修复"] },
      },
    ],
  };
}

function setupHistory(history: MasterHistoryDto) {
  mockInvoke.mockImplementation(async (command: string, args?: unknown) => {
    if (command === "get_master_history") return history;
    if (command === "get_relay_ledger") return ledgerEntries();
    if (command === "get_master_session_messages") {
      return sessionMessages((args as { sessionId: string }).sessionId);
    }
    return undefined;
  });
}

describe("HistoryWorkbench（P5-3 历史页主库视图）", () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it("ready 状态渲染两栏：左栏项目 + 右栏会话 + 项目统计", async () => {
    setupHistory(historyDto());
    render(<HistoryWorkbench active />);

    // 左栏项目（含"全部项目"入口）；归档会话（s3）不计入项目统计
    expect(await screen.findByTestId("history-project-all")).toBeInTheDocument();
    expect(screen.getByTestId("history-project-p1")).toHaveTextContent("项目一");
    expect(screen.getByTestId("history-project-p1")).toHaveTextContent("2 会话");
    expect(screen.getByTestId("history-project-p2")).toHaveTextContent("0 会话");

    // 右栏全部会话（默认未选项目 = 全部）；归档会话不入主列表，入口带计数
    const list = screen.getByTestId("history-session-list");
    expect(list).toHaveTextContent("会话一");
    expect(list).toHaveTextContent("会话二");
    expect(list).not.toHaveTextContent("旧归档会话");
    expect(screen.getByTestId("history-archive-entry")).toHaveTextContent("已归档 1");

    // 全部项目视图按项目分组：组头 = 项目名 + 会话数
    expect(screen.getByTestId("history-session-group-p1")).toHaveTextContent("项目一");
    expect(screen.getByTestId("history-session-group-p1")).toHaveTextContent("2 个会话");

    // 项目文件夹路径收进悬浮提示，不占主视野
    expect(screen.getByTestId("history-project-p1")).toHaveAttribute(
      "title",
      "d:\\work\\project-one",
    );
  });

  it("P5-8a-2 embedded 模式：无页级标题，工具行（搜索/筛选/刷新）保留", async () => {
    setupHistory(historyDto());
    render(<HistoryWorkbench active embedded />);

    await screen.findByTestId("history-session-s1");
    // 页级标题让位给宿主页（主库详情页），但两栏与工具行完整可用。
    expect(screen.queryByRole("heading", { level: 1, name: "历史" })).not.toBeInTheDocument();
    expect(screen.getByTestId("history-search-input")).toBeInTheDocument();
    expect(screen.getByTestId("history-time-week")).toBeInTheDocument();
    expect(screen.getByTestId("history-refresh")).toBeInTheDocument();
    expect(screen.getByTestId("history-project-p1")).toHaveTextContent("项目一");
  });

  it("接力徽章：沿 from_session_id 链回完整轨迹（3 账号 + 悬停明细）", async () => {
    setupHistory(historyDto());
    render(<HistoryWorkbench active />);
    await screen.findByTestId("history-session-s1");

    // s1 行上有接力徽章；无台账的会话（s2/s3）没有
    expect(screen.getByTestId("history-session-s1")).toHaveTextContent("接力记录 · 3 个账号使用过");
    expect(screen.queryByTestId("history-relay-chain")).toBeTruthy();

    // 悬停浮层内容：三段腿（首腿小谢、中段账号A、当前腿账号B 标记当前账号）
    const pop = screen.getByTestId("history-session-s1");
    expect(pop).toHaveTextContent("小谢");
    expect(pop).toHaveTextContent("账号A");
    expect(pop).toHaveTextContent("账号B（当前账号）");
  });

  it("点击左栏项目：右栏只显示该项目会话", async () => {
    setupHistory(historyDto());
    render(<HistoryWorkbench active />);
    await screen.findByTestId("history-session-list");

    fireEvent.click(screen.getByTestId("history-project-p1"));
    const list = screen.getByTestId("history-session-list");
    expect(list).toHaveTextContent("会话一");
    expect(list).toHaveTextContent("会话二");
    expect(list).not.toHaveTextContent("旧归档会话");
  });

  it("搜索与时间范围筛选会话", async () => {
    setupHistory(historyDto());
    render(<HistoryWorkbench active />);
    await screen.findByTestId("history-session-list");

    // 搜索标题
    fireEvent.change(screen.getByTestId("history-search-input"), { target: { value: "会话一" } });
    let list = screen.getByTestId("history-session-list");
    expect(list).toHaveTextContent("会话一");
    expect(list).not.toHaveTextContent("会话二");

    // 清空搜索，切"今天"：只保留最近更新的 s1
    fireEvent.change(screen.getByTestId("history-search-input"), { target: { value: "" } });
    fireEvent.click(screen.getByTestId("history-time-today"));
    list = screen.getByTestId("history-session-list");
    expect(list).toHaveTextContent("会话一");
    expect(list).not.toHaveTextContent("会话二");
    expect(list).not.toHaveTextContent("旧归档会话");
  });

  it("点击会话打开预览弹层：接力时间线 + 消息流，Esc 关闭", async () => {
    setupHistory(historyDto());
    render(<HistoryWorkbench active />);
    await screen.findByTestId("history-session-s1");

    fireEvent.click(screen.getByTestId("history-session-s1"));
    const preview = await screen.findByTestId("history-preview");

    // 接力时间线：三步 + 当前账号徽章
    expect(preview).toHaveTextContent("接力记录（谁在什么时候用过这条会话）");
    expect(preview).toHaveTextContent("当前账号");

    // 消息流：文本消息 + 任务轨迹（含 thought 摘要）
    await waitFor(() => {
      expect(preview).toHaveTextContent("帮我看下这个报错");
    });
    expect(preview).toHaveTextContent("任务轨迹 · 2 步");
    expect(preview).toHaveTextContent("先定位");
    expect(mockInvoke).toHaveBeenCalledWith("get_master_session_messages", { sessionId: "s1" });

    // Esc 关闭
    fireEvent.keyDown(window, { key: "Escape" });
    await waitFor(() => {
      expect(screen.queryByTestId("history-preview")).not.toBeInTheDocument();
    });
  });

  it("no_master_data：主库从未启动时显示引导并跳转环境页", async () => {
    setupHistory(historyDto({ status: "no_master_data", projects: [], sessions: [] }));
    const onNavigate = vi.fn();
    render(<HistoryWorkbench active onNavigate={onNavigate} />);

    fireEvent.click(await screen.findByTestId("history-guide-launch"));
    expect(onNavigate).toHaveBeenCalledWith("environment");
    // 引导态不拉台账
    expect(mockInvoke).not.toHaveBeenCalledWith("get_relay_ledger");
  });

  it("no_current_account / read_failed 状态提示", async () => {
    setupHistory(historyDto({ status: "no_current_account", projects: [], sessions: [] }));
    const { unmount } = render(<HistoryWorkbench active />);
    expect(await screen.findByText("主库尚未登记登录账号。")).toBeInTheDocument();
    unmount();

    setupHistory(historyDto({ status: "read_failed", projects: [], sessions: [] }));
    render(<HistoryWorkbench active />);
    expect(await screen.findByText("主库记录暂时无法读取，请稍后重试。")).toBeInTheDocument();
  });

  it("读取失败显示错误与重试入口；active=false 不发起读取", async () => {
    mockInvoke.mockRejectedValue(new Error("boom"));
    render(<HistoryWorkbench active />);
    expect(await screen.findByTestId("history-error")).toHaveTextContent("主库记录暂时不可读取");

    mockInvoke.mockClear();
    mockInvoke.mockResolvedValue(undefined);
    render(<HistoryWorkbench active={false} />);
    expect(mockInvoke).not.toHaveBeenCalled();
  });

  it("轮询带 previous 指纹；unchanged 不刷新台账", async () => {
    vi.useFakeTimers();
    try {
      const dto = historyDto();
      const unchanged: MasterHistoryDto = {
        status: "unchanged",
        current_user_id: null,
        projects: [],
        sessions: [],
        fingerprint: dto.fingerprint,
      };
      let calls = 0;
      mockInvoke.mockImplementation(async (command: string) => {
        if (command === "get_master_history") {
          calls += 1;
          return calls === 1 ? dto : unchanged;
        }
        if (command === "get_relay_ledger") return ledgerEntries();
        return undefined;
      });

      render(<HistoryWorkbench active />);
      // 初次加载完成（fake timers 下用 act 冲洗微任务，findBy 会卡在假定时器上）
      await act(async () => {
        await vi.advanceTimersByTimeAsync(0);
      });
      expect(screen.getByTestId("history-session-s1")).toBeInTheDocument();
      expect(mockInvoke).toHaveBeenCalledWith("get_master_history", { previous: null });

      // 5 秒轮询：带上一轮指纹；返回 unchanged 时不重拉台账、列表维持
      await act(async () => {
        await vi.advanceTimersByTimeAsync(5000);
      });
      expect(mockInvoke).toHaveBeenCalledWith("get_master_history", { previous: dto.fingerprint });
      expect(mockInvoke.mock.calls.filter(([c]) => c === "get_relay_ledger")).toHaveLength(1);
      expect(screen.getByTestId("history-session-s1")).toBeInTheDocument();
    } finally {
      vi.useRealTimers();
    }
  });
});

// ============================================================================
// P5-8a 会话归档：选择模式批量栏 + 归档视图 + 真实删除确认
// ============================================================================

/** 可变会话副本（归档/恢复/删除直接改状态，与真机两栏联动行为同构）。 */
type MutableSession = {
  session_id: string;
  project_id: string;
  title: string;
  message_count: number;
  updated_at_unix_seconds: number | null;
  deleted: boolean;
  hidden_status: string | null;
  work_mode: string | null;
};

/** 可变历史 mock：批量命令直接改写状态，get_master_history 返回新引用触发刷新。 */
function setupMutableHistory() {
  const dto = historyDto();
  const sessions = dto.sessions.map((session) => ({ ...session })) as MutableSession[];
  mockInvoke.mockImplementation(async (command: string, args?: unknown) => {
    if (command === "get_master_history") return { ...dto, sessions: [...sessions] };
    if (command === "get_relay_ledger") return ledgerEntries();
    if (command === "get_master_session_messages") {
      return sessionMessages((args as { sessionId: string }).sessionId);
    }
    if (command === "archive_master_sessions") {
      const ids = new Set((args as { sessionIds: string[] }).sessionIds);
      for (const session of sessions) {
        if (ids.has(session.session_id)) session.hidden_status = "voice_discussion";
      }
      return { affected: ids.size };
    }
    if (command === "restore_master_sessions") {
      const ids = new Set((args as { sessionIds: string[] }).sessionIds);
      let affected = 0;
      for (const session of sessions) {
        if (ids.has(session.session_id) && session.hidden_status === "voice_discussion") {
          session.hidden_status = null;
          affected += 1;
        }
      }
      return { affected };
    }
    if (command === "delete_master_sessions") {
      const ids = new Set((args as { sessionIds: string[] }).sessionIds);
      let messages = 0;
      for (let index = sessions.length - 1; index >= 0; index -= 1) {
        const session = sessions[index]!;
        if (ids.has(session.session_id)) {
          messages += session.message_count;
          sessions.splice(index, 1);
        }
      }
      return {
        deleted_sessions: ids.size,
        deleted_messages: messages,
        removed_projects: 0,
        backup_path: "backup-path",
      };
    }
    if (command === "merge_master_projects") {
      // P5-8c：源分组会话改挂目标分组（mock 与后端事务行为同构）。
      const { sourceProjectIds, targetProjectId } = args as {
        sourceProjectIds: string[];
        targetProjectId: string;
      };
      let moved = 0;
      for (const session of sessions) {
        if (sourceProjectIds.includes(session.project_id)) {
          session.project_id = targetProjectId;
          moved += 1;
        }
      }
      return {
        moved_sessions: moved,
        removed_projects: sourceProjectIds.length,
        backup_path: "backup-path",
      };
    }
    return undefined;
  });
  return sessions;
}

describe("HistoryWorkbench（P5-8a 会话归档与批量操作）", () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it("选择模式 + 批量归档：归档后主列表消失、入口计数增加、退出选择态", async () => {
    setupMutableHistory();
    render(<HistoryWorkbench active />);
    await screen.findByTestId("history-session-s1");

    // 进入选择模式 → 浮出批量栏 → 勾选两个会话
    fireEvent.click(screen.getByTestId("history-select-mode"));
    expect(screen.getByTestId("session-batch-bar")).toBeInTheDocument();
    fireEvent.click(screen.getByTestId("history-session-s1"));
    fireEvent.click(screen.getByTestId("history-session-s2"));
    expect(screen.getByTestId("batch-count")).toHaveTextContent("已选 2 / 2 项");

    // 归档所选（voice_discussion 写入 + 强制刷新）
    fireEvent.click(screen.getByTestId("batch-archive"));
    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith("archive_master_sessions", {
        sessionIds: ["s1", "s2"],
      });
    });

    // 刷新后：主列表无归档会话、入口计数 3（s3 原有 + 新归档 2）、退出选择态
    await waitFor(() => {
      expect(screen.getByTestId("history-archive-entry")).toHaveTextContent("已归档 3");
    });
    expect(screen.queryByTestId("session-batch-bar")).not.toBeInTheDocument();
    expect(screen.queryByTestId("history-session-s1")).not.toBeInTheDocument();
    expect(screen.queryByTestId("history-session-s2")).not.toBeInTheDocument();
  });

  it("Esc 退出选择模式并恢复会话预览行为", async () => {
    setupMutableHistory();
    render(<HistoryWorkbench active />);
    await screen.findByTestId("history-session-s1");

    fireEvent.click(screen.getByTestId("history-select-mode"));
    fireEvent.click(screen.getByTestId("history-session-s1"));
    fireEvent.keyDown(window, { key: "Escape" });

    await waitFor(() => {
      expect(screen.queryByTestId("session-batch-bar")).not.toBeInTheDocument();
    });
    // 退出后点击会话恢复打开预览（选择态点击是勾选，非预览）
    fireEvent.click(screen.getByTestId("history-session-s1"));
    expect(await screen.findByTestId("history-preview")).toBeInTheDocument();
  });

  it("归档视图：模式→分组层级展示 + 恢复所选归位主列表", async () => {
    setupMutableHistory();
    render(<HistoryWorkbench active />);
    await screen.findByTestId("history-session-s1");

    // 进入归档视图：Work 模式 → 项目二分组 → 会话行
    fireEvent.click(screen.getByTestId("history-archive-entry"));
    const archiveList = await screen.findByTestId("history-archive-list");
    expect(archiveList).toHaveTextContent("Work 模式");
    expect(archiveList).toHaveTextContent("项目二");
    expect(screen.getByTestId("archive-session-s3")).toHaveTextContent("旧归档会话");

    // 选择 → 恢复所选
    fireEvent.click(screen.getByTestId("archive-select-mode"));
    fireEvent.click(screen.getByTestId("archive-session-s3"));
    fireEvent.click(screen.getByTestId("batch-restore"));
    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith("restore_master_sessions", {
        sessionIds: ["s3"],
      });
    });

    // 恢复后归档视图清空；返回主列表可见 s3 归位
    await waitFor(() => {
      expect(screen.getByTestId("archive-empty")).toBeInTheDocument();
    });
    fireEvent.click(screen.getByTestId("history-archive-back"));
    expect(await screen.findByTestId("history-session-s3")).toBeInTheDocument();
  });

  it("删除所选：二次确认列明规模，取消不删除、确认后调用真实删除", async () => {
    setupMutableHistory();
    render(<HistoryWorkbench active />);
    await screen.findByTestId("history-session-s1");

    fireEvent.click(screen.getByTestId("history-archive-entry"));
    await screen.findByTestId("archive-session-s3");

    // 选择 → 删除所选 → 确认弹窗（列明会话数与消息量）
    fireEvent.click(screen.getByTestId("archive-select-mode"));
    fireEvent.click(screen.getByTestId("archive-session-s3"));
    fireEvent.click(screen.getByTestId("batch-delete"));

    const dialog = await screen.findByTestId("delete-confirm");
    expect(dialog).toHaveTextContent("删除 1 个会话");
    expect(dialog).toHaveTextContent("共 1 条消息");

    // 取消路径：不触发删除命令
    fireEvent.click(screen.getByTestId("delete-confirm-cancel"));
    expect(mockInvoke).not.toHaveBeenCalledWith(
      "delete_master_sessions",
      expect.anything(),
    );

    // 再次发起并确认 → 真实删除 + 归档视图清空
    fireEvent.click(screen.getByTestId("batch-delete"));
    fireEvent.click(await screen.findByTestId("delete-confirm-ok"));
    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith("delete_master_sessions", {
        sessionIds: ["s3"],
      });
    });
    await waitFor(() => {
      expect(screen.getByTestId("archive-empty")).toBeInTheDocument();
    });
  });
});

// ============================================================================
// P5-8c 分组合并：左栏选择态 + 确认弹层（选保留分组）+ 合并回执
// ============================================================================

describe("HistoryWorkbench（P5-8c 分组合并）", () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it("合并流程：选择分组 → 弹层选保留目标 → 调用后端 → 回执与列表刷新", async () => {
    setupMutableHistory();
    render(<HistoryWorkbench active />);
    await screen.findByTestId("history-session-s1");

    // 进入左栏分组选择态：批量栏浮出，浏览态头部与"全部项目"入口让位
    fireEvent.click(screen.getByTestId("project-merge-entry"));
    expect(screen.getByTestId("project-batch-bar")).toBeInTheDocument();
    expect(screen.queryByTestId("history-project-all")).not.toBeInTheDocument();
    expect(screen.queryByTestId("project-merge-entry")).not.toBeInTheDocument();

    // 勾选两个分组（选择态点击 = 勾选而非筛选）
    fireEvent.click(screen.getByTestId("history-project-p1"));
    fireEvent.click(screen.getByTestId("history-project-p2"));
    expect(screen.getByTestId("project-batch-count")).toHaveTextContent("已选 2 / 2");

    // 打开确认弹层：列明分组数与会话量（默认保留第一个分组 → 移动 p2 的 1 个归档会话）
    fireEvent.click(screen.getByTestId("batch-merge"));
    const dialog = await screen.findByTestId("merge-confirm");
    expect(dialog).toHaveTextContent("合并 2 个分组");
    expect(dialog).toHaveTextContent("1 个会话将移入保留的分组（含已归档）");
    expect(dialog).toHaveTextContent("项目一");
    expect(dialog).toHaveTextContent("项目二");
    expect(dialog).toHaveTextContent("合并前会自动创建主库数据备份");

    // 改选保留分组为「项目二」：源 = p1（2 个会话移入）
    fireEvent.click(screen.getByTestId("merge-target-p2"));
    expect(dialog).toHaveTextContent("2 个会话将移入保留的分组（含已归档）");

    // 确认合并 → 调用后端（备份 + 事务改挂）
    fireEvent.click(screen.getByTestId("merge-confirm-ok"));
    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith("merge_master_projects", {
        sourceProjectIds: ["p1"],
        targetProjectId: "p2",
      });
    });

    // 刷新后：会话全部归入项目二、左栏退出选择态、回执以数量表述
    await waitFor(() => {
      expect(screen.queryByTestId("project-batch-bar")).not.toBeInTheDocument();
    });
    expect(screen.getByTestId("history-project-p2")).toHaveTextContent("2 会话");
    const notice = screen.getByTestId("merge-notice");
    expect(notice).toHaveTextContent("已把 2 个会话并入「项目二」，清理 1 个空分组。");

    // 会话列表组头也随合并更新（p2 组含 s1/s2）
    expect(screen.getByTestId("history-session-group-p2")).toHaveTextContent("2 个会话");
  });

  it("少于 2 个分组时合并按钮禁用；Esc 关闭弹层并保留选择态", async () => {
    setupMutableHistory();
    render(<HistoryWorkbench active />);
    await screen.findByTestId("history-session-s1");

    fireEvent.click(screen.getByTestId("project-merge-entry"));
    // 只勾选 1 个分组：合并按钮禁用
    fireEvent.click(screen.getByTestId("history-project-p1"));
    expect(screen.getByTestId("batch-merge")).toBeDisabled();

    // 勾选第二个后可发起；Esc 关闭弹层但保持选择态（勾选不丢）
    fireEvent.click(screen.getByTestId("history-project-p2"));
    fireEvent.click(screen.getByTestId("batch-merge"));
    expect(await screen.findByTestId("merge-confirm")).toBeInTheDocument();
    fireEvent.keyDown(window, { key: "Escape" });

    await waitFor(() => {
      expect(screen.queryByTestId("merge-confirm")).not.toBeInTheDocument();
    });
    expect(screen.getByTestId("project-batch-bar")).toBeInTheDocument();
    expect(screen.getByTestId("project-batch-count")).toHaveTextContent("已选 2 / 2");

    // 再次 Esc：退出分组选择态，浏览态恢复
    fireEvent.keyDown(window, { key: "Escape" });
    await waitFor(() => {
      expect(screen.queryByTestId("project-batch-bar")).not.toBeInTheDocument();
    });
    expect(screen.getByTestId("history-project-all")).toBeInTheDocument();
    // 未调用合并命令（未确认）
    expect(mockInvoke).not.toHaveBeenCalledWith("merge_master_projects", expect.anything());
  });

  it("合并错误走安全文案：不吞失败、列表维持", async () => {
    const sessions = setupMutableHistory();
    mockInvoke.mockImplementation(async (command: string) => {
      if (command === "merge_master_projects") {
        // 模拟主库运行中拒绝合并（真实错误码走 safeUiError 映射）。
        throw new Error("master_merge_running");
      }
      // 其余命令复用可变 mock 的行为
      if (command === "get_master_history") {
        return historyDto({
          sessions: sessions.map((session) => ({ ...session })),
        });
      }
      if (command === "get_relay_ledger") return ledgerEntries();
      return undefined;
    });
    render(<HistoryWorkbench active />);
    await screen.findByTestId("history-session-s1");

    fireEvent.click(screen.getByTestId("project-merge-entry"));
    fireEvent.click(screen.getByTestId("history-project-p1"));
    fireEvent.click(screen.getByTestId("history-project-p2"));
    fireEvent.click(screen.getByTestId("batch-merge"));
    fireEvent.click(await screen.findByTestId("merge-confirm-ok"));

    // 错误以自然语言提示（master_merge_running 的映射文案）
    await waitFor(() => {
      expect(screen.getByTestId("batch-error")).toHaveTextContent("主库正在运行");
    });
    // 列表维持原状（会话未被移动）
    expect(screen.getByTestId("history-project-p1")).toHaveTextContent("2 会话");
  });
});
