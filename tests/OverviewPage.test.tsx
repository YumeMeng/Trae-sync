import { beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import { OverviewPage } from "../src/features/overview/OverviewPage";
import type { CheckinOverviewEntryDto } from "../src/types/account_switch";
import type { WorkspaceStateDto } from "../src/types/workspace";
import { installFakeInvoke, mockRoute, resetRoutes } from "./helpers/fakeInvoke";

// ============================================================================
// OverviewPage（P4a 合并页）：标题统一为「总览」（问候语已删，产品名在标题栏）；
// 总览恒 ready：证据带 + 摘要条（主库统计）正常渲染。
// ============================================================================

// P13-5：共享 fakeInvoke 路由器（迁移模式见 tests/helpers/fakeInvoke.ts 头注释）。
vi.mock("@tauri-apps/api/core", async () => {
  const fake = await import("./helpers/fakeInvoke");
  return { invoke: fake.mockCoreInvoke };
});

function workspaceState(overrides: Partial<WorkspaceStateDto> = {}): WorkspaceStateDto {
  return {
    platform: {
      platform_id: "work_cn",
      display_name: "TRAE",
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
  mockRoute("get_checkin_capability", () => ({
    enabled: false,
    transport: "disabled",
    real_http_enabled: false,
    message: "",
  }));
  mockRoute("get_environment_state", () => ({
    env_id: "master",
    current_profile_id: "profile-a",
    current_account_name: "主库账号",
    observed_profile_id: "profile-a",
    data_dir: "C:\\TRAE\\master",
    running: false,
    login_state: "logged_in",
    created_at_unix_seconds: 1,
  }));
  mockRoute("get_master_library_stats", () => ({
    status: "ready",
    current_user_id: "u-1",
    project_count: 4,
    session_count: 16,
    message_count: 128,
    participating_account_count: 3,
    last_active_unix_seconds: null,
    size_bytes: 0,
  }));
}

describe("OverviewPage 合并页（P4a）", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    // 等价于原 vi.fn() 空实现：清历史 + 重装按路由表分发的实现。
    resetRoutes();
    installFakeInvoke();
  });

  it("标题统一为「总览」：无问候语、不重复产品名", () => {
    setupReadyMocks();
    render(
      <OverviewPage
        state={workspaceState()}
        active
        productId="trae_cn"
        onNavigate={() => undefined}
      />,
    );
    const heading = screen.getByRole("heading", { level: 1 });
    expect(heading).toHaveTextContent(/^总览$/);
    expect(heading).not.toHaveTextContent("工作台");
    expect(heading).not.toHaveTextContent("Work CN");
  });

  it("ready 形态：无 pending/absent 说明卡，证据带与统计卡正常", async () => {
    setupReadyMocks();
    render(
      <OverviewPage
        state={workspaceState()}
        active
        productId="trae_cn"
        onNavigate={() => undefined}
      />,
    );
    expect(screen.queryByTestId("capability-pending")).not.toBeInTheDocument();
    expect(screen.queryByTestId("capability-absent")).not.toBeInTheDocument();
    expect(await screen.findByTestId("current-account-context")).toBeInTheDocument();
    expect(await screen.findByTestId("overview-stats-master")).toBeInTheDocument();
  });

  // R-1：P4a 合并时漏搬的「模型积分合计」（旧页 OverviewPage.tsx:89-95 计算、139-142 展示）。
  // 口径：各账号 usage_remaining_credits 求和（未查询账号不计入）；合计 0 → 占位「—」。
  it("签到摘要恢复「模型积分合计」：两条账号求和，全部未查询时占位「—」", async () => {
    setupReadyMocks();
    mockRoute("get_checkin_capability", () => ({
      enabled: true,
      transport: "real_http",
      real_http_enabled: true,
      message: "真实签到已启用。",
    }));
    mockRoute("get_checkin_overview", () =>
      [overviewEntry("p1", "甲账号", 1600), overviewEntry("p2", "乙账号", 900)],
    );
    const { unmount } = render(
      <OverviewPage
        state={workspaceState()}
        active
        productId="trae_cn"
        onNavigate={() => undefined}
      />,
    );
    const credits = await screen.findByTestId("overview-credits-total");
    expect(credits.textContent).toContain("2500");
    expect(credits.textContent).toContain("模型积分合计");

    // 两条账号均未查询额度（usage_remaining_credits = null）→ 不显示假 0，占位「—」。
    unmount();
    mockRoute("get_checkin_overview", () =>
      [overviewEntry("p1", "甲账号", null), overviewEntry("p2", "乙账号", null)],
    );
    render(
      <OverviewPage
        state={workspaceState()}
        active
        productId="trae_cn"
        onNavigate={() => undefined}
      />,
    );
    const placeholder = await screen.findByTestId("overview-credits-total");
    expect(placeholder.textContent).toContain("—");
    expect(placeholder.textContent).not.toContain("0");
  });
});

/** 真实签到摘要行构造器（总览只读 usage_remaining_credits 与 checked_in 两列口径）。 */
function overviewEntry(
  profileId: string,
  name: string,
  usageRemaining: number | null,
): CheckinOverviewEntryDto {
  return {
    profile_id: profileId,
    screen_name: name,
    account_id: `acct-${profileId}`,
    created_at: null,
    last_verified_at: null,
    credits: null,
    credits_cached_at: null,
    usage_remaining_credits: usageRemaining,
    usage_cached_at: null,
    checked_in: true,
    access_token_expires_at_unix_seconds: null,
    refresh_token_expires_at_unix_seconds: null,
    device_tail: null,
    device_id: null,
    display_name: null,
    masked_mobile: "",
    mobile_full: null,
    auto_checkin_enabled: false,
    refresh_error_code: null,
    credential_legacy: false,
    last_attempt_outcome: null,
    last_attempt_date: null,
  };
}