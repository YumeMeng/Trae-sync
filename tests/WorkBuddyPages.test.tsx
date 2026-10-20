import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { installFakeInvoke, mockRoute, mockCoreInvoke, resetRoutes } from "./helpers/fakeInvoke";

vi.mock("@tauri-apps/api/core", async () => {
  const fake = await import("./helpers/fakeInvoke");
  return { invoke: fake.mockCoreInvoke };
});

vi.mock("../src/platform/notify", () => ({
  notify: {
    success: vi.fn(),
    error: vi.fn(),
    warning: vi.fn(),
    info: vi.fn(),
    summary: vi.fn(),
  },
}));

import { WorkBuddyOverviewPage } from "../src/pages/WorkBuddyOverviewPage";
import type { AppPage } from "../src/components/NavigationRail";

/** P19-B3 库统计回执（serde 命名与 workbuddy_library.rs 同构）。 */
const libraryStats = {
  account_count: 3,
  session_count: 42,
  soft_deleted_session_count: 2,
  project_count: 7,
  projects_dir_bytes: 1024,
  last_active_unix_seconds: 1760000000,
};

function installOverviewRoutes(options?: { failStats?: boolean }) {
  mockRoute("list_workbuddy_checkin_accounts", () => [
    { profile_id: "wb-uid-1", screen_name: "甲账号", enabled: true },
    { profile_id: "wb-uid-2", screen_name: "乙账号", enabled: true },
  ]);
  mockRoute("get_workbuddy_checkin_overview", () => [
    {
      profile_id: "wb-uid-1", screen_name: "甲账号",
      today_checked_in: true, streak_days: 6, daily_credit: 100, total_credits: 700,
      checkin_dates: ["2026-10-20"], error: null,
    },
    {
      profile_id: "wb-uid-2", screen_name: "乙账号",
      today_checked_in: false, streak_days: 1, daily_credit: 100, total_credits: 500,
      checkin_dates: [], error: null,
    },
  ]);
  mockRoute("get_workbuddy_library_stats", () =>
    options?.failStats ? Promise.reject(new Error("no_library")) : libraryStats);
}

beforeEach(() => {
  resetRoutes();
  installFakeInvoke();
  window.localStorage.clear();
});

afterEach(() => {
  window.localStorage.clear();
});

describe("WorkBuddyOverviewPage（P19-B6 总览工作台模板）", () => {
  it("渲染：签到与账号统计卡（账号/今日已签/积分合计）+ 会话统计卡 + 主操作", async () => {
    installOverviewRoutes();
    const pages: AppPage[] = [];
    render(<WorkBuddyOverviewPage active={true} onNavigate={(page) => pages.push(page)} />);

    // 签到与账号卡：账号 2、今日已签 1/2、积分合计 1200。
    expect(await screen.findByTestId("wb-overview-checkin")).toBeInTheDocument();
    expect(screen.getByTestId("wb-overview-checkin").textContent).toContain("签到与账号");
    expect(screen.getByTestId("wb-overview-checkin-cta")).toHaveTextContent("去签到（余 1）");
    // 会话统计卡：库内会话/项目/账号 + 最近活跃。
    expect(screen.getByTestId("wb-overview-stats-library").textContent).toContain("42");
    expect(screen.getByTestId("wb-overview-stats-library").textContent).toContain("库内会话");
    // 主操作：查看对话库。
    fireEvent.click(screen.getByTestId("wb-overview-library-cta"));
    expect(pages).toContainEqual("master-library");
    // 管理账号入口在册。
    fireEvent.click(screen.getByTestId("wb-overview-accounts-cta"));
    expect(pages).toContainEqual("accounts");
  });

  it("降级：库统计读取失败 → 诚实提示行替代统计卡；签到数据失败 → 签到卡隐藏", async () => {
    installOverviewRoutes({ failStats: true });
    mockRoute("list_workbuddy_checkin_accounts", () => Promise.reject(new Error("down")));
    mockRoute("get_workbuddy_checkin_overview", () => Promise.reject(new Error("down")));
    render(<WorkBuddyOverviewPage active={true} onNavigate={() => undefined} />);

    await waitFor(() => {
      expect(screen.getByTestId("wb-overview-library-hint")).toBeInTheDocument();
    });
    expect(screen.queryByTestId("wb-overview-checkin")).not.toBeInTheDocument();
    expect(screen.queryByTestId("wb-overview-stats-library")).not.toBeInTheDocument();
    // 主操作仍在（对话库入口是恒定导航动作）。
    expect(screen.getByTestId("wb-overview-library-cta")).toBeInTheDocument();
  });
});

// ===== P19-B6④ 设置页 WB 变体（备份诚实说明卡） =====

import { SettingsPanel } from "../src/components/SettingsPanel";

describe("SettingsPanel WB 变体（P19-B6 备份区诚实呈现）", () => {
  it("WB 产品：备份策略说明卡在册，TRAE 备份管理面不渲染且零 IPC", async () => {
    render(<SettingsPanel active={true} productId="workbuddy" />);

    expect(screen.getByTestId("workbuddy-backup-note")).toHaveTextContent("备份策略");
    expect(screen.getByTestId("workbuddy-backup-note").textContent).toContain("自动滚动单份");
    expect(screen.getByText("WorkBuddy 的自动签到设置在「签到」页")).toBeInTheDocument();
    // TRAE 备份管理面不渲染；数据面命令零调用。
    expect(screen.queryByTestId("master-backup-section")).not.toBeInTheDocument();
    expect(mockCoreInvoke).not.toHaveBeenCalled();
  });

  it("TRAE 产品：备份管理面照常（现有行为不变）", async () => {
    render(<SettingsPanel active={true} productId="trae_cn" />);
    // WB 变体不生效：TRAE 备份分区在数据到达后渲染（此处只断言 WB 卡不存在）。
    expect(screen.queryByTestId("workbuddy-backup-note")).not.toBeInTheDocument();
  });
});
