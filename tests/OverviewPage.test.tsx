import { beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import { OverviewPage } from "../src/pages/OverviewPage";
import type { WorkspaceStateDto } from "../src/types/workspace";

// ============================================================================
// OverviewPage（P11-6）：
// 任务 1 —— 问候语取产品注册表 displayName 动态生成，随产品切换变化；
// 任务 2 —— Trae CN 总览为 pending 形态：问候 + 诚实说明卡 + 浏览入口，
// 不发起 Work CN 数据读取（跨产品数据隔离）。
// ============================================================================

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(),
}));

const mockInvoke = vi.mocked(invoke);

function workspaceState(overrides: Partial<WorkspaceStateDto> = {}): WorkspaceStateDto {
  return {
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
    history: { account_count: 1, project_count: 2, session_count: 3 },
    capabilities: {
      scan_enabled: true,
      sync_enabled: false,
      backup_enabled: false,
      restore_enabled: false,
    },
    honest_status: "RealReadPreview",
    ...overrides,
  };
}

/** ready 形态的最小数据 mock（签到能力未启用 + 主库统计 ready）。 */
function setupReadyMocks() {
  mockInvoke.mockImplementation(async (command: string) => {
    if (command === "get_checkin_capability") {
      return { enabled: false, transport: "disabled", real_http_enabled: false, message: "" };
    }
    if (command === "get_environment_state") {
      return {
        env_id: "master",
        current_profile_id: "profile-a",
        current_account_name: "主库账号",
        data_dir: "C:\\TRAE\\master",
        running: false,
        login_state: "logged_in",
        created_at_unix_seconds: 1,
      };
    }
    if (command === "get_master_library_stats") {
      return {
        status: "ready",
        current_user_id: "u-1",
        project_count: 4,
        session_count: 16,
        message_count: 128,
        participating_account_count: 3,
        last_active_unix_seconds: null,
        size_bytes: 0,
      };
    }
    throw new Error(`未模拟的命令: ${command}`);
  });
}

describe("OverviewPage 问候语动态化（任务 1）", () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it("Work CN：问候语带注册表 displayName「TRAE Work CN 工作台」", () => {
    setupReadyMocks();
    render(<OverviewPage state={workspaceState()} active product="work_cn" onNavigate={() => undefined} />);
    const heading = screen.getByRole("heading", { level: 1 });
    // 时段词（早上好/下午好等）随真实时间变化，断言产品名后缀稳定存在。
    expect(heading).toHaveTextContent(/，这里是你的 TRAE Work CN 工作台$/);
  });

  it("Trae CN：问候语随产品切换变为「Trae CN 工作台」", () => {
    render(<OverviewPage state={workspaceState()} active product="trae_cn" onNavigate={() => undefined} />);
    const heading = screen.getByRole("heading", { level: 1 });
    expect(heading).toHaveTextContent(/，这里是你的 Trae CN 工作台$/);
    expect(heading).not.toHaveTextContent("TRAE Work CN");
  });
});

describe("OverviewPage Trae CN pending 形态（任务 2）", () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it("渲染诚实说明卡与浏览入口，不发起 Work CN 数据读取", () => {
    render(<OverviewPage state={workspaceState()} active product="trae_cn" onNavigate={() => undefined} />);

    expect(screen.getByTestId("capability-pending")).toBeInTheDocument();
    // 浏览入口进入主库详情页（Trae CN 只读数据根）。
    expect(screen.getByTestId("overview-library-cta")).toBeInTheDocument();
    expect(screen.getByTestId("overview-accounts-cta")).toBeInTheDocument();
    // 跨产品数据隔离：pending 形态不读取 Work CN 环境与统计。
    expect(mockInvoke).not.toHaveBeenCalled();
  });

  it("不出 Work CN 证据带、统计卡与目录缺失文案", () => {
    render(<OverviewPage state={workspaceState()} active product="trae_cn" onNavigate={() => undefined} />);
    expect(screen.queryByTestId("current-account-context")).not.toBeInTheDocument();
    expect(screen.queryByTestId("overview-stats-master")).not.toBeInTheDocument();
    expect(screen.queryByText(/未找到 TRAE 数据目录/)).not.toBeInTheDocument();
  });

  it("Work CN ready 形态不受影响：无 pending 卡，统计卡正常", async () => {
    setupReadyMocks();
    render(<OverviewPage state={workspaceState()} active product="work_cn" onNavigate={() => undefined} />);
    expect(screen.queryByTestId("capability-pending")).not.toBeInTheDocument();
    expect(await screen.findByTestId("overview-stats-master")).toBeInTheDocument();
  });
});
