import { beforeEach, describe, expect, it, vi } from "vitest";
import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import { LibrarySessionsPanel } from "../src/components/LibrarySessionsPanel";
import type { MasterHistoryDto, RelayLedgerEntryDto } from "../src/types/history";

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
      // G19：空名 + 无路径项目 → 归并「未关联文件夹」组
      { project_id: "p9", name: "", absolute_path: null },
    ],
    sessions: [
      {
        session_id: "s1",
        project_id: "p1",
        title: "会话一",
        message_count: 10,
        updated_at_unix_seconds: NOW - 3600,
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
        // P5-8a：voice_discussion 借用为归档 → 不入主树，进归档视图。
        session_id: "s3",
        project_id: "p2",
        title: "旧归档会话",
        message_count: 1,
        updated_at_unix_seconds: NOW - 40 * DAY,
        deleted: false,
        hidden_status: "voice_discussion",
        work_mode: "work",
      },
      {
        session_id: "s4",
        project_id: "p2",
        title: "会话四",
        message_count: 5,
        updated_at_unix_seconds: NOW - 30,
        deleted: false,
        hidden_status: null,
        work_mode: "code",
      },
      {
        session_id: "s5",
        project_id: "p2",
        title: "会话五",
        message_count: 2,
        updated_at_unix_seconds: NOW - 7200,
        deleted: false,
        hidden_status: null,
        work_mode: "code",
      },
      {
        session_id: "s9",
        project_id: "p9",
        title: "未关联会话",
        message_count: 2,
        updated_at_unix_seconds: NOW - 600,
        deleted: false,
        hidden_status: null,
        work_mode: "code",
      },
    ],
    fingerprint: fingerprint(1),
    ...overrides,
  };
}

/**
 * 两跳台账：s0(u-x→u-a) → s1(u-a→u-b)；s1 链回出 3 条腿：
 * 小谢 [最早, T1) → 账号A [T1, T2) → 账号B [T2, 至今)。
 */
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
      switched_at_unix_seconds: Math.floor(NOW - 7200),
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

/** s1 消息流：三个时间段的用户消息各一条 + 一条助手消息。 */
function sessionMessages(sessionId: string, offset = 0) {
  if (offset > 0) {
    return {
      session_id: sessionId,
      status: "ready",
      has_more: false,
      messages: [
        {
          message_id: "older-m0",
          role: "user",
          message_type: "general",
          created_at_unix_seconds: Math.floor(NOW - 10_000),
          content: { kind: "text", text: "更早的一条消息", step_count: 0, thoughts: [] },
        },
      ],
    };
  }
  if (sessionId !== "s1") {
    return {
      session_id: sessionId,
      status: "ready",
      has_more: false,
      messages: [
        {
          message_id: "m1",
          role: "user",
          message_type: "general",
          created_at_unix_seconds: Math.floor(NOW - 120),
          content: { kind: "text", text: "普通会话消息", step_count: 0, thoughts: [] },
        },
      ],
    };
  }
  return {
    session_id: sessionId,
    status: "ready",
    has_more: true,
    messages: [
      {
        // 早于首跳交接（NOW-7200）→ 归属首腿小谢（色板 c0 蓝）。
        message_id: "m0",
        role: "user",
        message_type: "general",
        created_at_unix_seconds: Math.floor(NOW - 9000),
        content: { kind: "text", text: "最早的一条提问", step_count: 0, thoughts: [] },
      },
      {
        // 两跳之间（NOW-7200 → NOW-3600）→ 归属中腿账号A（色板 c1 绿）。
        message_id: "m1",
        role: "user",
        message_type: "general",
        created_at_unix_seconds: Math.floor(NOW - 5000),
        content: { kind: "text", text: "帮我看下这个报错", step_count: 0, thoughts: [] },
      },
      {
        message_id: "m2",
        role: "assistant",
        message_type: "task",
        created_at_unix_seconds: Math.floor(NOW - 60),
        content: { kind: "task_trace", text: "", step_count: 2, thoughts: ["先定位", "再修复"] },
      },
      {
        // 最后一跳之后 → 归属当前腿账号B（色板 c2 琥珀）。
        message_id: "m3",
        role: "user",
        message_type: "general",
        created_at_unix_seconds: Math.floor(NOW - 120),
        content: { kind: "text", text: "接手继续处理", step_count: 0, thoughts: [] },
      },
    ],
  };
}

function setupHistory(history: MasterHistoryDto) {
  mockInvoke.mockImplementation(async (command: string, args?: unknown) => {
    if (command === "get_master_history") return history;
    if (command === "get_relay_ledger") return ledgerEntries();
    if (command === "get_master_session_messages") {
      const typedArgs = args as { sessionId: string; offset?: number };
      return sessionMessages(typedArgs.sessionId, typedArgs.offset);
    }
    return undefined;
  });
}

/** 展开项目树分支（会话子级按需渲染）。 */
async function expandProject(projectId: string) {
  fireEvent.click(await screen.findByTestId(`library-project-${projectId}`));
}

describe("LibrarySessionsPanel（G21 项目树与对话查看器）", () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it("树结构：项目行 + 会话子级按需展开 + 未关联文件夹末位 + 库注入参数", async () => {
    setupHistory(historyDto());
    render(<LibrarySessionsPanel active />);

    // 库注入：所有读取都带 libraryId（ADR-0025，缺省主库）
    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith("get_master_history", {
        previous: null,
        libraryId: "master",
      });
    });
    expect(mockInvoke).toHaveBeenCalledWith("get_relay_ledger", { libraryId: "master" });

    // 项目行：名称 + 会话数角标；会话子级默认收起
    expect(await screen.findByTestId("library-project-p1")).toHaveTextContent("项目一");
    expect(screen.getByTestId("library-project-p1")).toHaveTextContent("2");
    expect(screen.getByTestId("library-project-p2")).toHaveTextContent("项目二");
    expect(screen.queryByTestId("library-session-s1")).not.toBeInTheDocument();

    // 点击原地展开：会话行 = 标题 + 相对时间；再点收起
    await expandProject("p1");
    expect(screen.getByTestId("library-session-s1")).toHaveTextContent("会话一");
    expect(screen.getByTestId("library-session-s1")).toHaveTextContent("1 小时前");
    expect(screen.getByTestId("library-session-s2")).toHaveTextContent("会话二");
    fireEvent.click(screen.getByTestId("library-project-p1"));
    expect(screen.queryByTestId("library-session-s1")).not.toBeInTheDocument();

    // 「未关联文件夹」末位合并组：空名项目的会话平铺其下
    const unlinked = screen.getByTestId("library-project-__unlinked__");
    expect(unlinked).toHaveTextContent("未关联文件夹");
    expect(unlinked).toHaveTextContent("1");
    fireEvent.click(unlinked);
    expect(screen.getByTestId("library-session-s9")).toHaveTextContent("未关联会话");

    // 项目文件夹路径收进悬浮提示，不占主视野（界面表达纪律）
    expect(screen.getByTestId("library-project-p1")).toHaveAttribute(
      "title",
      "d:\\work\\project-one",
    );

    // 无选中：右栏空态引导
    expect(screen.getByTestId("library-viewer-empty")).toHaveTextContent("从左侧选择一个会话查看对话");
  });

  it("搜索过滤树内会话：命中项目自动展开，无命中项目隐藏", async () => {
    setupHistory(historyDto());
    render(<LibrarySessionsPanel active />);
    await screen.findByTestId("library-project-p1");

    fireEvent.change(screen.getByTestId("library-search-input"), { target: { value: "会话二" } });
    // 命中自动展开：s2 可见；s1 被过滤；无命中的项目与未关联组隐藏
    expect(screen.getByTestId("library-session-s2")).toBeInTheDocument();
    expect(screen.queryByTestId("library-session-s1")).not.toBeInTheDocument();
    expect(screen.queryByTestId("library-project-p2")).not.toBeInTheDocument();
    expect(screen.queryByTestId("library-project-__unlinked__")).not.toBeInTheDocument();

    // 清空恢复全树
    fireEvent.change(screen.getByTestId("library-search-input"), { target: { value: "" } });
    expect(screen.getByTestId("library-project-p2")).toBeInTheDocument();
  });

  it("会话选中显示查看器：标题/项目/消息数 + 消息流渲染 + 关闭清空", async () => {
    setupHistory(historyDto());
    render(<LibrarySessionsPanel active />);
    await expandProject("p1");

    fireEvent.click(screen.getByTestId("library-session-s1"));
    const viewer = await screen.findByTestId("library-viewer");
    expect(screen.getByTestId("library-viewer-title")).toHaveTextContent("会话一");
    expect(viewer).toHaveTextContent("项目一");
    expect(viewer).toHaveTextContent("10 条消息");

    // 消息流（带库参数读取）：文本 + 任务轨迹两种形态
    await waitFor(() => {
      expect(screen.getByTestId("library-viewer-messages")).toHaveTextContent("帮我看下这个报错");
    });
    expect(mockInvoke).toHaveBeenCalledWith("get_master_session_messages", {
      sessionId: "s1",
      libraryId: "master",
    });
    expect(screen.getByTestId("library-viewer-messages")).toHaveTextContent("任务轨迹 · 2 步");
    expect(screen.getByTestId("library-viewer-messages")).toHaveTextContent("先定位");

    // 消息流滚到顶部自动请求更早一页，并把消息拼到当前窗口前面。
    const flow = screen.getByTestId("library-viewer-messages");
    fireEvent.scroll(flow, { target: { scrollTop: 0 } });
    await waitFor(() => expect(screen.getByTestId("library-message-older-m0")).toBeInTheDocument());
    expect(mockInvoke).toHaveBeenCalledWith("get_master_session_messages", {
      sessionId: "s1",
      libraryId: "master",
      offset: 4,
    });

    // 右上关闭清除选中，回到空态引导
    fireEvent.click(screen.getByTestId("library-viewer-close"));
    expect(screen.getByTestId("library-viewer-empty")).toBeInTheDocument();
  });

  it("消息着色：用户消息按接力腿归属账号（蓝/绿/琥珀），助手消息中性无头像", async () => {
    setupHistory(historyDto());
    render(<LibrarySessionsPanel active />);
    await expandProject("p1");
    fireEvent.click(screen.getByTestId("library-session-s1"));
    await screen.findByTestId("library-viewer-messages");

    // 归属判定：created_at 落在哪腿的 [from, to) 就归该腿账号
    const avatarOf = (messageId: string) =>
      screen.getByTestId(`library-message-${messageId}`).querySelector(".account-avatar");

    expect(screen.getByTestId("library-message-m0")).toHaveTextContent("小谢");
    expect(avatarOf("m0")?.className).toContain("account-avatar--c0");
    expect(screen.getByTestId("library-message-m1")).toHaveTextContent("账号A");
    expect(avatarOf("m1")?.className).toContain("account-avatar--c1");
    expect(screen.getByTestId("library-message-m3")).toHaveTextContent("账号B");
    expect(avatarOf("m3")?.className).toContain("account-avatar--c2");

    // 助手消息：中性机器人图标，无账号头像
    expect(screen.getByTestId("library-message-m2").querySelector(".account-avatar")).toBeNull();
    expect(screen.getByTestId("library-message-m2").querySelector(".chat-msg__bot")).not.toBeNull();
  });

  it("接力 tab：三腿时间线 + 当前腿「当前」徽章", async () => {
    setupHistory(historyDto());
    render(<LibrarySessionsPanel active />);
    await expandProject("p1");
    fireEvent.click(screen.getByTestId("library-session-s1"));
    await screen.findByTestId("library-viewer-messages");

    fireEvent.click(screen.getByTestId("library-tab-relay"));
    const relay = await screen.findByTestId("library-relay-tab");
    // 三腿：小谢 → 账号A → 账号B（当前腿徽章只在最后一段）
    expect(relay.querySelectorAll(".leg-step")).toHaveLength(3);
    expect(relay).toHaveTextContent("小谢");
    expect(relay).toHaveTextContent("账号A");
    expect(relay).toHaveTextContent("账号B");
    expect(relay.querySelectorAll(".leg-step__badge")).toHaveLength(1);
    expect(relay).toHaveTextContent("当前");
    // 当前腿期间新增消息数 = 10 - 7 = 3
    expect(relay).toHaveTextContent("至今");
  });

  it("no_master_data：显示引导并跳转环境页；引导态不拉台账", async () => {
    setupHistory(historyDto({ status: "no_master_data", projects: [], sessions: [] }));
    const onNavigate = vi.fn();
    render(<LibrarySessionsPanel active onNavigate={onNavigate} />);

    fireEvent.click(await screen.findByTestId("history-guide-launch"));
    expect(onNavigate).toHaveBeenCalledWith("environment");
    expect(mockInvoke).not.toHaveBeenCalledWith("get_relay_ledger", expect.anything());
  });

  it("no_current_account / read_failed 状态提示", async () => {
    setupHistory(historyDto({ status: "no_current_account", projects: [], sessions: [] }));
    const { unmount } = render(<LibrarySessionsPanel active />);
    expect(await screen.findByText("主库尚未登记登录账号。")).toBeInTheDocument();
    unmount();

    setupHistory(historyDto({ status: "read_failed", projects: [], sessions: [] }));
    render(<LibrarySessionsPanel active />);
    expect(await screen.findByText("主库记录暂时无法读取，请稍后重试。")).toBeInTheDocument();
  });

  it("读取失败显示错误与重试入口；active=false 不发起读取", async () => {
    mockInvoke.mockRejectedValue(new Error("boom"));
    render(<LibrarySessionsPanel active />);
    expect(await screen.findByTestId("history-error")).toHaveTextContent("库记录暂时不可读取");

    mockInvoke.mockClear();
    mockInvoke.mockResolvedValue(undefined);
    render(<LibrarySessionsPanel active={false} />);
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

      render(<LibrarySessionsPanel active />);
      // 初次加载完成（fake timers 下用 act 冲洗微任务）
      await act(async () => {
        await vi.advanceTimersByTimeAsync(0);
      });
      expect(screen.getByTestId("library-project-p1")).toBeInTheDocument();
      expect(mockInvoke).toHaveBeenCalledWith("get_master_history", {
        previous: null,
        libraryId: "master",
      });

      // 5 秒轮询：带上一轮指纹；返回 unchanged 时不重拉台账、树维持
      await act(async () => {
        await vi.advanceTimersByTimeAsync(5000);
      });
      expect(mockInvoke).toHaveBeenCalledWith("get_master_history", {
        previous: dto.fingerprint,
        libraryId: "master",
      });
      expect(mockInvoke.mock.calls.filter(([command]) => command === "get_relay_ledger")).toHaveLength(1);
      expect(screen.getByTestId("library-project-p1")).toBeInTheDocument();
    } finally {
      vi.useRealTimers();
    }
  });
});

// ============================================================================
// G22 统一选择模式：左栏底部进入，浮出 Gmail 式操作栏
// ============================================================================

/** 可变会话副本（归档/恢复/删除/合并直接改状态，与真机两栏联动行为同构）。 */
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
      // 源项目会话改挂目标项目（mock 与后端事务行为同构）。
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

describe("LibrarySessionsPanel（G22 统一选择模式与批量操作）", () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it("批量归档：勾选会话浮出操作栏 → 归档调用命令 → 退出选择 + 行内回执", async () => {
    setupMutableHistory();
    render(<LibrarySessionsPanel active />);
    await expandProject("p1");

    // 左栏底部「选择」进入统一选择模式
    fireEvent.click(screen.getByTestId("library-select-mode"));
    expect(screen.getByTestId("library-action-bar")).toBeInTheDocument();
    // 右栏顶部提示条：选择模式只读预览
    expect(screen.getByTestId("library-select-hint")).toHaveTextContent("选择模式：预览只读");

    // 勾选两个会话（选择态点击 = 勾选而非查看）
    fireEvent.click(screen.getByTestId("library-session-s1"));
    fireEvent.click(screen.getByTestId("library-session-s2"));
    expect(screen.getByTestId("batch-count")).toHaveTextContent("已选 2 个会话 · 0 个项目");

    // 归档（可逆 → 直接执行不确认）
    fireEvent.click(screen.getByTestId("batch-archive"));
    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith("archive_master_sessions", {
        sessionIds: ["s1", "s2"],
        libraryId: "master",
      });
    });

    // 刷新后：退出选择模式、行内回执、入口计数 3（s3 原有 + 新归档 2）
    await waitFor(() => {
      expect(screen.getByTestId("library-archive-entry")).toHaveTextContent("3");
    });
    expect(screen.queryByTestId("library-action-bar")).not.toBeInTheDocument();
    expect(screen.getByTestId("action-notice")).toHaveTextContent("已归档 2 个会话。");
    expect(screen.queryByTestId("library-session-s1")).not.toBeInTheDocument();
  });

  it("Esc 退出选择模式并恢复会话选中行为", async () => {
    setupMutableHistory();
    render(<LibrarySessionsPanel active />);
    await expandProject("p1");

    fireEvent.click(screen.getByTestId("library-select-mode"));
    fireEvent.click(screen.getByTestId("library-session-s1"));
    fireEvent.keyDown(window, { key: "Escape" });

    await waitFor(() => {
      expect(screen.queryByTestId("library-action-bar")).not.toBeInTheDocument();
    });
    // 退出后点击会话恢复打开查看器（选择态点击是勾选，非查看）
    fireEvent.click(screen.getByTestId("library-session-s1"));
    expect(await screen.findByTestId("library-viewer")).toBeInTheDocument();
  });

  it("悬浮快捷归档：单会话直接归档，不进选择模式", async () => {
    setupMutableHistory();
    render(<LibrarySessionsPanel active />);
    await expandProject("p1");

    fireEvent.click(screen.getByTestId("library-quick-archive-s2"));
    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith("archive_master_sessions", {
        sessionIds: ["s2"],
        libraryId: "master",
      });
    });
    // 不进选择模式；回执 + 树内即时消失
    expect(screen.queryByTestId("library-action-bar")).not.toBeInTheDocument();
    await waitFor(() => {
      expect(screen.getByTestId("action-notice")).toHaveTextContent("已归档 1 个会话。");
    });
    expect(screen.queryByTestId("library-session-s2")).not.toBeInTheDocument();
  });

  it("删除确认列明规模：取消不删除、确认后调用真实删除", async () => {
    setupMutableHistory();
    render(<LibrarySessionsPanel active />);
    await expandProject("p1");

    fireEvent.click(screen.getByTestId("library-select-mode"));
    fireEvent.click(screen.getByTestId("library-session-s1"));
    fireEvent.click(screen.getByTestId("batch-delete"));

    // 确认弹窗列明会话数与消息量（s1 = 10 条消息）
    const dialog = await screen.findByTestId("delete-confirm");
    expect(dialog).toHaveTextContent("删除 1 个会话");
    expect(dialog).toHaveTextContent("共 10 条消息");
    expect(dialog).toHaveTextContent("删除前会自动备份");

    // 取消路径：不触发删除命令
    fireEvent.click(screen.getByTestId("delete-confirm-cancel"));
    expect(mockInvoke).not.toHaveBeenCalledWith("delete_master_sessions", expect.anything());

    // 再次发起并确认 → 真实删除 + 退出选择模式
    fireEvent.click(screen.getByTestId("batch-delete"));
    fireEvent.click(await screen.findByTestId("delete-confirm-ok"));
    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith("delete_master_sessions", {
        sessionIds: ["s1"],
        libraryId: "master",
      });
    });
    await waitFor(() => {
      expect(screen.queryByTestId("library-action-bar")).not.toBeInTheDocument();
    });
    expect(screen.getByTestId("action-notice")).toHaveTextContent("已删除 1 个会话。");
  });

  it("归档视图：模式→项目层级展示 + 恢复所选归位主树", async () => {
    setupMutableHistory();
    render(<LibrarySessionsPanel active />);
    await screen.findByTestId("library-project-p1");

    // 进入归档视图：Work 模式 → 项目二 → 会话行
    fireEvent.click(screen.getByTestId("library-archive-entry"));
    const tree = await screen.findByTestId("library-tree");
    expect(screen.getByTestId("library-archive-back")).toBeInTheDocument();
    expect(tree).toHaveTextContent("Work 模式");
    expect(tree).toHaveTextContent("项目二");
    expect(screen.getByTestId("library-session-s3")).toHaveTextContent("旧归档会话");

    // 选择 → 恢复所选（归档视图操作栏：恢复 + 删除）
    fireEvent.click(screen.getByTestId("library-select-mode"));
    fireEvent.click(screen.getByTestId("library-session-s3"));
    fireEvent.click(screen.getByTestId("batch-restore"));
    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith("restore_master_sessions", {
        sessionIds: ["s3"],
        libraryId: "master",
      });
    });

    // 恢复后归档视图空；返回主树可见 s3 归位项目二
    await waitFor(() => {
      expect(screen.getByTestId("library-archive-empty")).toBeInTheDocument();
    });
    fireEvent.click(screen.getByTestId("library-archive-back"));
    await expandProject("p2");
    expect(screen.getByTestId("library-session-s3")).toBeInTheDocument();
  });

  it("合并流：勾选两个项目 → 弹层选保留目标 → 调用后端 → 回执与树刷新", async () => {
    setupMutableHistory();
    render(<LibrarySessionsPanel active />);
    await screen.findByTestId("library-project-p1");

    // 统一选择模式：项目行勾选（合并源）
    fireEvent.click(screen.getByTestId("library-select-mode"));
    fireEvent.click(screen.getByTestId("library-project-p1"));
    fireEvent.click(screen.getByTestId("library-project-p2"));
    expect(screen.getByTestId("batch-count")).toHaveTextContent("已选 0 个会话 · 2 个项目");

    // 打开确认弹层：默认保留第一个项目（p1）→ 移动 p2 的 3 个会话
    fireEvent.click(screen.getByTestId("batch-merge"));
    const dialog = await screen.findByTestId("merge-confirm");
    expect(dialog).toHaveTextContent("合并 2 个项目");
    expect(dialog).toHaveTextContent("3 个会话将移入保留的项目（含已归档）");
    expect(dialog).toHaveTextContent("项目一");
    expect(dialog).toHaveTextContent("项目二");
    expect(dialog).toHaveTextContent("合并前会自动备份");

    // 改选保留项目为「项目二」：源 = p1（2 个会话移入）
    fireEvent.click(screen.getByTestId("merge-target-p2"));
    expect(dialog).toHaveTextContent("2 个会话将移入保留的项目（含已归档）");

    // 确认合并 → 调用后端（备份 + 事务改挂，带库参数）
    fireEvent.click(screen.getByTestId("merge-confirm-ok"));
    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith("merge_master_projects", {
        sourceProjectIds: ["p1"],
        targetProjectId: "p2",
        libraryId: "master",
      });
    });

    // 刷新后：退出选择模式、回执以数量表述、p1 无会话不再显示、p2 收拢全部
    await waitFor(() => {
      expect(screen.queryByTestId("library-action-bar")).not.toBeInTheDocument();
    });
    expect(screen.getByTestId("action-notice")).toHaveTextContent(
      "已把 2 个会话并入「项目二」，清理 1 个空分组。",
    );
    expect(screen.queryByTestId("library-project-p1")).not.toBeInTheDocument();
    expect(screen.getByTestId("library-project-p2")).toHaveTextContent("4");
  });

  it("合并错误走安全文案：不吞失败、树维持", async () => {
    const sessions = setupMutableHistory();
    mockInvoke.mockImplementation(async (command: string) => {
      if (command === "merge_master_projects") {
        // 模拟主库运行中拒绝合并（真实错误码走 safeUiError 映射）。
        throw new Error("master_merge_running");
      }
      if (command === "get_master_history") {
        return historyDto({ sessions: sessions.map((session) => ({ ...session })) });
      }
      if (command === "get_relay_ledger") return ledgerEntries();
      return undefined;
    });
    render(<LibrarySessionsPanel active />);
    await screen.findByTestId("library-project-p1");

    fireEvent.click(screen.getByTestId("library-select-mode"));
    fireEvent.click(screen.getByTestId("library-project-p1"));
    fireEvent.click(screen.getByTestId("library-project-p2"));
    fireEvent.click(screen.getByTestId("batch-merge"));
    fireEvent.click(await screen.findByTestId("merge-confirm-ok"));

    // 错误以自然语言提示（master_merge_running 的映射文案）
    await waitFor(() => {
      expect(screen.getByTestId("batch-error")).toHaveTextContent("主库正在运行");
    });
    // 树维持原状（会话未被移动）
    expect(screen.getByTestId("library-project-p1")).toHaveTextContent("2");
  });
});
