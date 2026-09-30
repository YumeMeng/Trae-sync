import { beforeEach, describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import { MasterLibraryDetail } from "../src/pages/MasterLibraryDetail";
import type { AppPage } from "../src/components/NavigationRail";
import type { EnvironmentStateDto } from "../src/types/environment";
import type { MasterHistoryDto } from "../src/types/history";

// mock Tauri invoke——前端测试不依赖真实后端
vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(),
}));

// P11-1 切片②：读取失败原因已迁到 notify；断言 notify.error 调用。
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

const NOW = Math.floor(Date.now() / 1000);

function envState(overrides: Partial<EnvironmentStateDto> = {}): EnvironmentStateDto {
  return {
    env_id: "master",
    current_profile_id: "profile-a",
    current_account_name: "账号甲",
    data_dir: "C:\\TraeSync\\data\\environments\\master",
    running: true,
    login_state: "logged_in",
    created_at_unix_seconds: 1750000000,
    ...overrides,
  };
}

function historyDto(overrides: Partial<MasterHistoryDto> = {}): MasterHistoryDto {
  return {
    status: "ready",
    current_user_id: "user-a",
    projects: [{ project_id: "p1", name: "项目一", absolute_path: null }],
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
    ],
    fingerprint: {
      db: { mtime_secs: 1700000000, mtime_nanos: 0, size: 1000 },
      wal: null,
      shm: null,
    },
    ...overrides,
  };
}

/** 详情页完整数据 mock（环境状态 + 统计 ready + 备份链 + 主库记录）。
 *  stats/backupChain 显式传 null 表示「读取失败」，不能用 ?? 回退（null 会被吞）。 */
function setupDetail(overrides: {
  stats?: Record<string, unknown> | null;
  backupChain?: Record<string, unknown> | null;
  history?: MasterHistoryDto;
} = {}) {
  const defaultStats = {
    status: "ready",
    current_user_id: "user-a",
    project_count: 4,
    session_count: 16,
    message_count: 128,
    participating_account_count: 3,
    last_active_unix_seconds: NOW - 3600,
    size_bytes: 712 * 1024 * 1024,
  };
  const defaultBackupChain = {
    backups: [
      { stamp_unix_seconds: NOW - 7200, total_bytes: 3 * 1024 * 1024, has_wal: false },
      { stamp_unix_seconds: NOW - 86400, total_bytes: 2 * 1024 * 1024, has_wal: true },
    ],
    backup_dir: "C:\\TraeSync\\data\\environments\\master\\ModularData\\ai-agent",
    keep_policy: 5,
  };
  const stats = "stats" in overrides ? overrides.stats : defaultStats;
  const backupChain = "backupChain" in overrides ? overrides.backupChain : defaultBackupChain;
  mockInvoke.mockImplementation(async (command: string) => {
    if (command === "get_environment_state") return envState();
    if (command === "get_master_library_stats") {
      if (stats === null) throw new Error("trae_real_mode_required");
      return stats;
    }
    if (command === "get_master_backup_chain") {
      if (backupChain === null) throw new Error("trae_real_mode_required");
      return backupChain;
    }
    if (command === "get_master_history") return overrides.history ?? historyDto();
    if (command === "get_relay_ledger") return [];
    if (command === "get_master_verification") {
      // G20 库信息 tab 的备份对比区块（backupOnly：ledger 不渲染但数据随回执返回）。
      return {
        ledger: { status: "ready", checked_count: 0, relayed_away_count: 0, issues: [] },
        backup: {
          status: "ready",
          backup_stamp: NOW - 7200,
          backup_stamps: [NOW - 7200, NOW - 86400],
          common_count: 15,
          added_count: 1,
          relayed_away_count: 0,
          missing: [],
        },
      };
    }
    if (command === "get_plugin_tab_state") {
      return {
        installed: [
          {
            record_id: "rec-1",
            marketplace_plugin_id: "uuid-p1",
            name: "plugin-one",
            display_name: "插件一",
            version: "1.0.0",
            registry: "trae-remote-official",
            builtin: false,
            in_manifest: true,
          },
        ],
        manifest: [
          {
            marketplace_plugin_id: "uuid-p1",
            name: "plugin-one",
            display_name: "插件一",
            version: "1.0.0",
            registry: "trae-remote-official",
            installed_in_cloud: true,
          },
        ],
      };
    }
    return undefined;
  });
}

describe("MasterLibraryDetail（P5-8a-2 主库详情页 shell）", () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it("库信息 tab：账号/大小/路径/最近活跃/最近备份 + 计数 + 备份对比（G20 三 tab）", async () => {
    setupDetail();
    render(<MasterLibraryDetail active onNavigate={() => undefined} />);

    // G20：基础信息收进库信息 tab，顶部不再有大段信息。
    fireEvent.click(screen.getByTestId("master-tab-info"));
    const info = await screen.findByTestId("master-detail-info");
    expect(info).toBeInTheDocument();
    expect(screen.getByTestId("master-info-account")).toHaveTextContent("账号甲");
    // 712MiB 整值 → formatBytes 的整数分支「712 MB」。
    expect(screen.getByTestId("master-info-size")).toHaveTextContent("712 MB");
    expect(screen.getByTestId("master-info-path")).toHaveTextContent("environments\\master");
    expect(screen.getByTestId("master-info-backup")).not.toHaveTextContent("还没有备份");

    const counts = screen.getByTestId("master-detail-counts");
    expect(counts).toHaveTextContent("4");
    expect(counts).toHaveTextContent("16");
    expect(counts).toHaveTextContent("128");
    // 备份对比区块随 tab 激活（原数据校验 tab 的备份区块，G20 移入）。
    expect(await screen.findByTestId("verification-backup-block")).toBeInTheDocument();
    // 接力台账核对已移出库信息 tab（P8-5 主库自检承载）。
    expect(screen.queryByTestId("verification-ledger-block")).not.toBeInTheDocument();
  });

  it("默认对话列表 tab：复用库对话面板（无页级标题，左栏搜索可用）", async () => {
    setupDetail();
    render(<MasterLibraryDetail active onNavigate={() => undefined} />);

    // 嵌入态不渲染页级标题；左栏项目树可展开到会话子级。
    fireEvent.click(await screen.findByTestId("library-project-p1"));
    await screen.findByTestId("library-session-s1");
    expect(screen.queryByRole("heading", { level: 1, name: "历史" })).not.toBeInTheDocument();
    expect(screen.getByTestId("library-search-input")).toBeInTheDocument();
  });

  it("切换插件 tab 显示插件工作台，切回对话列表恢复两栏", async () => {
    setupDetail();
    render(<MasterLibraryDetail active onNavigate={() => undefined} />);
    fireEvent.click(await screen.findByTestId("library-project-p1"));
    await screen.findByTestId("library-session-s1");

    fireEvent.click(screen.getByTestId("master-tab-plugins"));
    // 插件 tab 激活后才发起状态读取（active 联动，不预取）。
    await screen.findByTestId("plugin-workbench");
    expect(screen.getByTestId("plugin-row-rec-1")).toBeVisible();
    // 切走后对话列表隐藏（hidden 属性），不再占据 tab 面板。
    expect(screen.queryByTestId("library-session-s1")).not.toBeVisible();

    fireEvent.click(screen.getByTestId("master-tab-sessions"));
    await waitFor(() => {
      expect(screen.getByTestId("library-session-s1")).toBeVisible();
    });
    expect(screen.queryByTestId("plugin-workbench")).not.toBeVisible();
  });

  it("统计读取失败时计数区隐藏，基础信息仍可展示", async () => {
    setupDetail({ stats: null });
    render(<MasterLibraryDetail active onNavigate={() => undefined} />);

    fireEvent.click(screen.getByTestId("master-tab-info"));
    await screen.findByTestId("master-detail-info");
    expect(screen.queryByTestId("master-detail-counts")).not.toBeInTheDocument();
    // 环境状态不受统计失败影响。
    expect(screen.getByTestId("master-info-account")).toHaveTextContent("账号甲");
  });

  it("无备份链时最近备份显示还没有备份", async () => {
    setupDetail({ backupChain: { backups: [], backup_dir: "C:\\bak", keep_policy: 5 } });
    render(<MasterLibraryDetail active onNavigate={() => undefined} />);
    fireEvent.click(screen.getByTestId("master-tab-info"));
    await screen.findByTestId("master-detail-info");
    expect(screen.getByTestId("master-info-backup")).toHaveTextContent("还没有备份");
  });

  it("G20 顶部无大段基础信息：默认对话列表 tab 直达两栏，信息区只在库信息 tab", async () => {
    setupDetail();
    render(<MasterLibraryDetail active onNavigate={() => undefined} />);

    // 默认 tab 是对话列表：信息网格随库信息 tab 渲染但不可见（hidden 属性）。
    fireEvent.click(await screen.findByTestId("library-project-p1"));
    await screen.findByTestId("library-session-s1");
    expect(screen.getByTestId("master-detail-info")).not.toBeVisible();
    expect(screen.getByTestId("master-info-account")).not.toBeVisible();
  });

  it("返回按钮回调 onNavigate(environment)", async () => {
    setupDetail();
    const visited: AppPage[] = [];
    render(<MasterLibraryDetail active onNavigate={(page) => visited.push(page)} />);
    fireEvent.click(await screen.findByTestId("library-project-p1"));
    await screen.findByTestId("library-session-s1");

    fireEvent.click(screen.getByTestId("master-detail-back"));
    expect(visited).toEqual(["environment"]);
  });

  it("active=false 不发起任何读取", () => {
    render(<MasterLibraryDetail active={false} onNavigate={() => undefined} />);
    expect(mockInvoke).not.toHaveBeenCalled();
  });

  it("环境状态读取失败显示错误提示", async () => {
    mockInvoke.mockImplementation(async (command: string) => {
      if (command === "get_environment_state") throw new Error("environment_registry_invalid");
      return undefined;
    });
    render(<MasterLibraryDetail active onNavigate={() => undefined} />);
    // 读取失败原因走 notify.error（P11-1 切片②），不再渲染页内错误条。
    await waitFor(() => {
      expect(notifyMock.error).toHaveBeenCalled();
    });
  });

  it("P11-5 收官：副库详情页——标题用环境名，统计/备份链/历史按 libraryId 解析", async () => {
    const secondaryListItem = {
      env_id: "env-x1",
      name: "测试环境",
      is_master: false,
      current_profile_id: "profile-b",
      current_account_name: "账号乙",
      data_dir: "D:\\store\\environments\\env-x1",
      running: false,
      login_state: "logged_in",
      created_at_unix_seconds: 1750000100,
      size_bytes: 2097152,
    };
    mockInvoke.mockImplementation(async (command: string, args?: unknown) => {
      const libraryId = (args as { libraryId?: string } | undefined)?.libraryId;
      if (command === "list_environments") return [secondaryListItem];
      if (command === "get_master_library_stats") {
        // 副库必须显式带 libraryId（缺省主库兼容口径不被误用）。
        if (libraryId !== "env-x1") throw new Error("missing libraryId");
        return {
          status: "ready",
          current_user_id: "user-b",
          project_count: 2,
          session_count: 5,
          message_count: 40,
          participating_account_count: 1,
          last_active_unix_seconds: NOW - 60,
          size_bytes: 1024 * 1024,
        };
      }
      if (command === "get_master_backup_chain") {
        if (libraryId !== "env-x1") throw new Error("missing libraryId");
        return { backups: [], backup_dir: "D:\\store\\environments\\env-x1\\ModularData\\ai-agent", keep_policy: 5 };
      }
      if (command === "get_master_history") {
        if (libraryId !== "env-x1") throw new Error("missing libraryId");
        return historyDto();
      }
      if (command === "get_relay_ledger") return [];
      if (command === "get_master_verification") {
        if (libraryId !== "env-x1") throw new Error("missing libraryId");
        return {
          ledger: { status: "ready", checked_count: 0, relayed_away_count: 0, issues: [] },
          backup: { status: "no_backups", backup_stamp: null, backup_stamps: [], common_count: 0, relayed_away_count: 0, added_count: 0, missing: [] },
        };
      }
      return undefined;
    });
    render(
      <MasterLibraryDetail
        active
        onNavigate={() => undefined}
        library={{ id: "env-x1", displayName: "测试环境" }}
      />,
    );

    // 标题/面包屑 = 环境名（不再是「主库」）。
    expect(screen.getByRole("heading", { level: 1 })).toHaveTextContent("测试环境");
    // 副环境档案：当前账号与数据目录来自 list_environments 行。
    fireEvent.click(screen.getByTestId("master-tab-info"));
    const info = await screen.findByTestId("master-detail-info");
    expect(screen.getByTestId("master-info-account")).toHaveTextContent("账号乙");
    expect(screen.getByTestId("master-info-path")).toHaveTextContent("environments\\env-x1");
    expect(screen.getByTestId("master-info-size")).toHaveTextContent("1.0 MB");
    // 统计口径与主库一致（计数区同款结构）。
    await waitFor(() => {
      expect(screen.getByTestId("master-detail-counts")).toHaveTextContent("2");
    });
    expect(info).toBeInTheDocument();
  });

  it("P11-5 收官：副库插件 tab 显示说明（副环境不参与插件管理），不挂工作台", async () => {
    const secondaryListItem = {
      env_id: "env-x1",
      name: "测试环境",
      is_master: false,
      current_profile_id: null,
      current_account_name: null,
      data_dir: "D:\\store\\environments\\env-x1",
      running: false,
      login_state: "uninitialized",
      created_at_unix_seconds: 1750000100,
      size_bytes: 0,
    };
    mockInvoke.mockImplementation(async (command: string) => {
      if (command === "list_environments") return [secondaryListItem];
      if (command === "get_master_library_stats") {
        return { status: "no_master_data", current_user_id: null, project_count: 0, session_count: 0, message_count: 0, participating_account_count: 0, last_active_unix_seconds: null, size_bytes: 0 };
      }
      if (command === "get_master_backup_chain") {
        return { backups: [], backup_dir: "D:\\store", keep_policy: 5 };
      }
      if (command === "get_master_history") return historyDto({ status: "no_master_data", current_user_id: null, projects: [], sessions: [] });
      return undefined;
    });
    render(
      <MasterLibraryDetail
        active
        onNavigate={() => undefined}
        library={{ id: "env-x1", displayName: "测试环境" }}
      />,
    );
    fireEvent.click(await screen.findByTestId("master-tab-plugins"));
    // 诚实 UI：显示说明而非插件工作台。
    expect(screen.getByTestId("master-plugins-secondary-note")).toHaveTextContent("副环境不参与插件管理");
    expect(screen.queryByTestId("plugin-workbench")).not.toBeInTheDocument();
  });

  // ==========================================================================
  // P11-6（ADR-0032 决策 5）：Trae CN 只读数据根详情。
  // 后端契约：trae_cn 库读放行（历史/消息/统计），写命令报 library_read_only，
  // 体检 not_applicable——前端以只读浏览形态呈现，写控件隐藏（不是禁用）。
  // ==========================================================================

  /** trae_cn 只读库的完整数据 mock（读命令带 libraryId=trae_cn；无环境档案查询）。 */
  function setupTraeCnDetail() {
    mockInvoke.mockImplementation(async (command: string, args?: unknown) => {
      const libraryId = (args as { libraryId?: string } | undefined)?.libraryId;
      if (command === "list_trae_cn_foreign_projects") {
        if (libraryId !== "trae_cn") throw new Error("missing libraryId");
        return [
          {
            project_id: "p-f1",
            name: "其他账号目录",
            absolute_path: "D:\\work\\shared",
            session_count: 2,
            message_count: 9,
          },
        ];
      }
      if (command === "get_master_library_stats") {
        if (libraryId !== "trae_cn") throw new Error("missing libraryId");
        return {
          status: "ready",
          current_user_id: "u-t1",
          project_count: 1,
          session_count: 2,
          message_count: 5,
          participating_account_count: 1,
          last_active_unix_seconds: NOW - 60,
          size_bytes: 1024 * 1024,
        };
      }
      if (command === "get_master_backup_chain") {
        if (libraryId !== "trae_cn") throw new Error("missing libraryId");
        return { backups: [], backup_dir: "C:\\trae-cn\\bak", keep_policy: 5 };
      }
      if (command === "get_master_history") {
        if (libraryId !== "trae_cn") throw new Error("missing libraryId");
        return historyDto({
          current_user_id: "u-t1",
          projects: [{ project_id: "p-t1", name: "Trae 项目", absolute_path: null }],
          sessions: [
            {
              session_id: "s-t1",
              project_id: "p-t1",
              title: "Trae 会话",
              message_count: 5,
              updated_at_unix_seconds: NOW - 60,
              deleted: false,
              hidden_status: null,
              work_mode: null,
            },
          ],
        });
      }
      if (command === "get_relay_ledger") return [];
      if (command === "get_master_verification") {
        if (libraryId !== "trae_cn") throw new Error("missing libraryId");
        // 只读库自然降级：无台账无备份（面板只渲染备份对比区块）。
        return {
          ledger: { status: "no_ledger", checked_count: 0, relayed_away_count: 0, issues: [] },
          backup: { status: "no_backups", backup_stamp: null, backup_stamps: [], common_count: 0, relayed_away_count: 0, added_count: 0, missing: [] },
        };
      }
      return undefined;
    });
  }

  it("P11-6：trae_cn 只读详情——标题取注册表口径，写控件隐藏，历史列表挂载", async () => {
    setupTraeCnDetail();
    render(<MasterLibraryDetail active onNavigate={() => undefined} library={{ id: "trae_cn" }} />);

    expect(screen.getByRole("heading", { level: 1 })).toHaveTextContent("Trae CN 数据根");
    // 只读边界：批量操作入口（选择按钮）与悬浮快捷归档隐藏。
    expect(screen.queryByTestId("library-select-mode")).not.toBeInTheDocument();
    expect(screen.queryByTestId("library-quick-archive-s-t1")).not.toBeInTheDocument();
    // 历史列表正常挂载（读路径放行）。
    fireEvent.click(await screen.findByTestId("library-project-p-t1"));
    await screen.findByTestId("library-session-s-t1");
    // 只读库不做环境档案查询（工具不管理 Trae CN 登录态）。
    expect(mockInvoke).not.toHaveBeenCalledWith("get_environment_state", expect.anything());
    expect(mockInvoke).not.toHaveBeenCalledWith("list_environments", expect.anything());
  });

  it("P11-6：trae_cn 库信息 tab——统计卡正常，验证面板自然降级，账号显示为未跟踪", async () => {
    setupTraeCnDetail();
    render(<MasterLibraryDetail active onNavigate={() => undefined} library={{ id: "trae_cn" }} />);

    fireEvent.click(screen.getByTestId("master-tab-info"));
    await screen.findByTestId("master-detail-info");
    // 统计卡正常渲染（stats 已放行）。
    await waitFor(() => {
      expect(screen.getByTestId("master-detail-counts")).toHaveTextContent("2");
    });
    // 工具不管理 Trae CN 登录态：当前账号不冒充「未登录」，显示中性占位。
    expect(screen.getByTestId("master-info-account")).toHaveTextContent("—");
    // 验证面板自然降级（备份对比区块在场，无备份为诚实空态）。
    expect(await screen.findByTestId("verification-backup-block")).toBeInTheDocument();
    expect(screen.getByTestId("verification-backup-badge")).toHaveTextContent("暂无备份");
  });

  it("P11-6：trae_cn 插件 tab 显示只读浏览说明，不挂插件工作台", async () => {
    setupTraeCnDetail();
    render(<MasterLibraryDetail active onNavigate={() => undefined} library={{ id: "trae_cn" }} />);
    fireEvent.click(await screen.findByTestId("master-tab-plugins"));
    expect(screen.getByTestId("master-plugins-readonly-note")).toHaveTextContent("Trae CN 为只读浏览");
    expect(screen.queryByTestId("plugin-workbench")).not.toBeInTheDocument();
  });

  it("P13-2：trae_cn 归并入口——其他账号项目组随读路径下发，主库不发归并清单读取", async () => {
    setupTraeCnDetail();
    render(<MasterLibraryDetail active onNavigate={() => undefined} library={{ id: "trae_cn" }} />);

    // 其他账号的可归并项目树末位成组（归并入口数据源按库解析）。
    const group = await screen.findByTestId("library-foreign-group");
    expect(group).toHaveTextContent("其他账号的项目");
    expect(screen.getByTestId("library-foreign-project-p-f1")).toHaveTextContent("其他账号目录");
    expect(screen.getByTestId("library-merge-entry-p-f1")).toHaveTextContent("归并到当前账号");
    expect(mockInvoke).toHaveBeenCalledWith("list_trae_cn_foreign_projects", {
      libraryId: "trae_cn",
    });

    // 主库详情不发归并清单读取（归并只对 trae_cn 开放）；清调用历史后重挂。
    setupDetail();
    mockInvoke.mockClear();
    render(<MasterLibraryDetail active onNavigate={() => undefined} />);
    await screen.findByTestId("library-project-p1");
    expect(mockInvoke).not.toHaveBeenCalledWith(
      "list_trae_cn_foreign_projects",
      expect.anything(),
    );
  });
});
