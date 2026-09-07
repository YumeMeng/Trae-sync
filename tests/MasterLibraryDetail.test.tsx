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

const mockInvoke = vi.mocked(invoke);

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

  it("默认对话列表 tab：复用历史页两栏（无页级标题，保留搜索工具行）", async () => {
    setupDetail();
    render(<MasterLibraryDetail active onNavigate={() => undefined} />);

    // 嵌入态不渲染历史页页级标题，但工具行（搜索）保留。
    await screen.findByTestId("history-session-s1");
    expect(screen.queryByRole("heading", { level: 1, name: "历史" })).not.toBeInTheDocument();
    expect(screen.getByTestId("history-search-input")).toBeInTheDocument();
  });

  it("切换插件 tab 显示插件工作台，切回对话列表恢复两栏", async () => {
    setupDetail();
    render(<MasterLibraryDetail active onNavigate={() => undefined} />);
    await screen.findByTestId("history-session-s1");

    fireEvent.click(screen.getByTestId("master-tab-plugins"));
    // 插件 tab 激活后才发起状态读取（active 联动，不预取）。
    await screen.findByTestId("plugin-workbench");
    expect(screen.getByTestId("plugin-row-rec-1")).toBeVisible();
    // 切走后对话列表隐藏（hidden 属性），不再占据 tab 面板。
    expect(screen.queryByTestId("history-session-s1")).not.toBeVisible();

    fireEvent.click(screen.getByTestId("master-tab-sessions"));
    await waitFor(() => {
      expect(screen.getByTestId("history-session-s1")).toBeVisible();
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
    await screen.findByTestId("history-session-s1");
    expect(screen.getByTestId("master-detail-info")).not.toBeVisible();
    expect(screen.getByTestId("master-info-account")).not.toBeVisible();
  });

  it("返回按钮回调 onNavigate(environment)", async () => {
    setupDetail();
    const visited: AppPage[] = [];
    render(<MasterLibraryDetail active onNavigate={(page) => visited.push(page)} />);
    await screen.findByTestId("history-session-s1");

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
    const alert = await screen.findByRole("alert");
    expect(alert).toBeInTheDocument();
  });
});
