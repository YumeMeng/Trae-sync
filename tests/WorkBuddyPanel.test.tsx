import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { installFakeInvoke, mockRoute, mockCoreInvoke, resetRoutes } from "./helpers/fakeInvoke";

vi.mock("@tauri-apps/api/core", async () => {
  const fake = await import("./helpers/fakeInvoke");
  return { invoke: fake.mockCoreInvoke };
});

// 通知走全局适配层：测试只关心 UI 状态，通知静默。
vi.mock("../src/platform/notify", () => ({
  notify: {
    success: vi.fn(),
    error: vi.fn(),
    warning: vi.fn(),
    info: vi.fn(),
    summary: vi.fn(),
  },
}));

import { WorkBuddyCheckinPage, WorkBuddyPanel } from "../src/components/WorkBuddyPanel";

// WorkBuddy 域命令回执（脱敏形态，与真实 DTO 同构）。
const health = {
  status: "active",
  uid: "uid-1",
  uin: "uid-1",
  domain: "workbuddy.example",
  access_expires_at_unix_seconds: 1799999999,
  refresh_expires_at_unix_seconds: 1799999999,
  captured_in_pool: true,
};

const poolEntry = (profileId: string, accountId: string, screenName: string) => ({
  profile_id: profileId,
  account_id: accountId,
  screen_name: screenName,
  domain: "workbuddy.example",
  access_token_expires_at_unix_seconds: 1799999999,
  refresh_token_expires_at_unix_seconds: 1799999999,
  minted_at_unix_seconds: 1000,
  last_updated_at_unix_seconds: 2000,
});

const checkinStatus = {
  today_checked_in: false,
  streak_days: 6,
  daily_credit: 100,
  total_credits: 700,
};

const autoStatus = (accounts: readonly { profile_id: string; screen_name: string; enabled: boolean }[]) => ({
  enabled: true,
  daily_time_hhmm: "09:30",
  ledger: null,
  accounts,
});

const defaultPool = [
  poolEntry("wb-uid-1", "uid-1", "甲账号"),
  poolEntry("wb-uid-2", "uid-2", "乙账号"),
];

function installWorkbuddyRoutes(options?: {
  pool?: typeof defaultPool;
  accounts?: readonly { profile_id: string; screen_name: string; enabled: boolean }[];
}) {
  mockRoute("get_workbuddy_session_health", () => health);
  mockRoute("list_workbuddy_accounts", () => options?.pool ?? defaultPool);
  mockRoute("workbuddy_checkin_status", () => checkinStatus);
  mockRoute("get_workbuddy_auto_checkin_status", () =>
    autoStatus(options?.accounts ?? [
      { profile_id: "wb-uid-1", screen_name: "甲账号", enabled: true },
      { profile_id: "wb-uid-2", screen_name: "乙账号", enabled: false },
    ]));
  mockRoute("set_workbuddy_account_auto_checkin", () => undefined);
  mockRoute("set_workbuddy_auto_checkin_settings", () => undefined);
}

beforeEach(() => {
  resetRoutes();
  installFakeInvoke();
  window.localStorage.clear();
});

afterEach(() => {
  window.localStorage.clear();
});

describe("WorkBuddyPanel（P19-B2⑤ 账号页骨架消费）", () => {
  it("骨架渲染：页头产品徽章 + 健康卡 + 池清单行走 SharedAccountEntry 规格 + 五动作在册", async () => {
    installWorkbuddyRoutes();
    render(<WorkBuddyPanel active={true} />);

    expect(await screen.findByTestId("workbuddy-health-card")).toBeInTheDocument();
    expect(screen.getByTestId("product-badge-workbuddy")).toBeInTheDocument();
    // 池清单行对齐账号页模板（account-list__row + 切换按钮 + 池清单锚点保留）。
    const list = screen.getByTestId("workbuddy-pool-list");
    expect(list.className).toContain("account-list");
    expect(list.querySelectorAll(".account-list__row")).toHaveLength(2);
    // 当前客户端登录账号行显示「使用中」chip（与 AccountCenter 同语义），非当前行才是切换按钮。
    expect(screen.getByTestId("workbuddy-pool-current-wb-uid-1")).toHaveTextContent("使用中");
    expect(screen.getByTestId("workbuddy-switch-wb-uid-2")).toHaveTextContent("切换到此账号");
    // 五个页头动作（刷新/续期/健康检测/过户/捕获）在册。
    for (const testId of ["workbuddy-refresh", "workbuddy-renew-credentials", "workbuddy-deep-health-button", "workbuddy-transfer-button", "workbuddy-capture-button"]) {
      expect(screen.getByTestId(testId)).toBeInTheDocument();
    }
  });

  it("能力开关生效：creditsColumn 只给当前客户端登录账号行积分块 + 「使用中」标记", async () => {
    installWorkbuddyRoutes();
    render(<WorkBuddyPanel active={true} />);

    await screen.findByTestId("workbuddy-health-card");
    // uid-1 = 当前客户端登录账号（健康卡 uid 同源）：积分块 + 使用中 chip。
    expect(screen.getByTestId("workbuddy-credits-uid-1")).toHaveTextContent("700");
    expect(screen.getByTestId("workbuddy-pool-current-wb-uid-1")).toHaveTextContent("使用中");
    expect(screen.queryByTestId("workbuddy-credits-uid-2")).not.toBeInTheDocument();
    expect(screen.queryByTestId("workbuddy-pool-current-wb-uid-2")).not.toBeInTheDocument();
  });

  it("U-2 双视图：卡片视图切换生效并按产品分 key 记忆偏好", async () => {
    installWorkbuddyRoutes();
    render(<WorkBuddyPanel active={true} />);

    await screen.findByTestId("workbuddy-pool-list");
    expect(document.querySelector(".account-card-grid")).not.toBeInTheDocument();
    fireEvent.click(screen.getByTestId("account-view-card"));
    expect(document.querySelector(".account-card-grid")).toBeInTheDocument();
    expect(window.localStorage.getItem("accounts.workbuddy.view")).toBe("card");
  });

  it("U-2 排序：名称排序生效（added 保持池序）", async () => {
    // added 序：B 在前；name 序：A 在前。
    mockRoute("get_workbuddy_session_health", () => health);
    mockRoute("list_workbuddy_accounts", () => [
      poolEntry("wb-b", "uid-b", "B账号"),
      poolEntry("wb-a", "uid-a", "A账号"),
    ]);
    mockRoute("workbuddy_checkin_status", () => checkinStatus);
    mockRoute("get_workbuddy_auto_checkin_status", () => autoStatus([]));
    render(<WorkBuddyPanel active={true} />);

    await screen.findByTestId("workbuddy-pool-list");
    const rowNames = () =>
      screen.getAllByTestId(/^workbuddy-pool-card-/)
        .map((row) => row.querySelector("strong")?.textContent);
    expect(rowNames()).toEqual(["B账号", "A账号"]);
    fireEvent.change(screen.getByTestId("account-sort"), { target: { value: "name" } });
    expect(rowNames()).toEqual(["A账号", "B账号"]);
    expect(window.localStorage.getItem("accounts.workbuddy.sort")).toBe("name");
  });

  it("读取失败态：骨架错误说明卡 + 重新读取动作在册", async () => {
    mockRoute("get_workbuddy_session_health", () => Promise.reject(new Error("boom")));
    mockRoute("list_workbuddy_accounts", () => Promise.reject(new Error("boom")));
    render(<WorkBuddyPanel active={true} />);

    expect(await screen.findByRole("alert")).toHaveTextContent("无法读取 WorkBuddy 登录状态");
    expect(screen.getByRole("button", { name: "重新读取" })).toBeInTheDocument();
  });

  it("空池：诚实空态文案在册，池清单不渲染", async () => {
    installWorkbuddyRoutes({ pool: [] });
    render(<WorkBuddyPanel active={true} />);

    expect(await screen.findByText(/还没有保存 WorkBuddy 账号/)).toBeInTheDocument();
    expect(screen.queryByTestId("workbuddy-pool-list")).not.toBeInTheDocument();
  });
});

describe("WorkBuddyCheckinPage（P19-B2⑤ 签到页骨架消费 + 参与集默认全参与）", () => {
  it("骨架渲染：单账号卡 + 自动签到状态行 + 立即签到可用", async () => {
    installWorkbuddyRoutes();
    render(<WorkBuddyCheckinPage active={true} />);

    expect(await screen.findByTestId("workbuddy-checkin-card")).toBeInTheDocument();
    expect(screen.getByTestId("workbuddy-checkin-button")).toHaveTextContent("立即签到");
    expect(screen.getByTestId("workbuddy-auto-checkin-meta")).toHaveTextContent("每日 09:30 触发");
    expect(screen.getByTestId("product-badge-workbuddy")).toBeInTheDocument();
  });

  it("参与集默认全参与：从未显式设置的账号池加载后自动批量置 true 并记录 touched", async () => {
    installWorkbuddyRoutes();
    render(<WorkBuddyCheckinPage active={true} />);

    await screen.findByTestId("workbuddy-checkin-card");
    // wb-uid-2 enabled=false 且从未显式设置：归一为参与（一次置 true）。
    await waitFor(() => {
      const calls = mockCoreInvoke.mock.calls.filter(([cmd]) => cmd === "set_workbuddy_account_auto_checkin");
      expect(calls).toContainEqual(["set_workbuddy_account_auto_checkin", { profileId: "wb-uid-2", enabled: true }]);
    });
    const touched = JSON.parse(window.localStorage.getItem("auto-checkin.workbuddy.touched") ?? "[]");
    expect(touched).toContain("wb-uid-2");
    // 归一只跑一轮：不会把已参与的 wb-uid-1 再写一遍。
    const calls = mockCoreInvoke.mock.calls.filter(([cmd]) => cmd === "set_workbuddy_account_auto_checkin");
    expect(calls.filter(([, args]) => (args as { profileId: string }).profileId === "wb-uid-1")).toHaveLength(0);
  });

  it("显式关闭尊重：touched 账号保持关闭，不被归一改写", async () => {
    window.localStorage.setItem("auto-checkin.workbuddy.touched", JSON.stringify(["wb-uid-2"]));
    installWorkbuddyRoutes();
    render(<WorkBuddyCheckinPage active={true} />);

    await screen.findByTestId("workbuddy-checkin-card");
    const checkbox = await screen.findByTestId("workbuddy-auto-checkin-account-wb-uid-2") as HTMLInputElement;
    expect(checkbox.checked).toBe(false);
    const calls = mockCoreInvoke.mock.calls.filter(([cmd]) => cmd === "set_workbuddy_account_auto_checkin");
    expect(calls).toHaveLength(0);
  });

  it("全部参与入口：一键把未参与账号批量置 true", async () => {
    // 两账号都已显式关闭（touched）：归一不动它们，按钮保持可用（未参与 2）。
    window.localStorage.setItem("auto-checkin.workbuddy.touched", JSON.stringify(["wb-uid-1", "wb-uid-2"]));
    installWorkbuddyRoutes({ accounts: [
      { profile_id: "wb-uid-1", screen_name: "甲账号", enabled: false },
      { profile_id: "wb-uid-2", screen_name: "乙账号", enabled: false },
    ] });
    render(<WorkBuddyCheckinPage active={true} />);

    const button = await screen.findByTestId("workbuddy-auto-checkin-participate-all");
    expect(button).toHaveTextContent("全部参与（2）");
    fireEvent.click(button);
    await waitFor(() => {
      const calls = mockCoreInvoke.mock.calls.filter(([cmd]) => cmd === "set_workbuddy_account_auto_checkin");
      expect(calls).toHaveLength(2);
      expect(calls).toContainEqual(["set_workbuddy_account_auto_checkin", { profileId: "wb-uid-1", enabled: true }]);
      expect(calls).toContainEqual(["set_workbuddy_account_auto_checkin", { profileId: "wb-uid-2", enabled: true }]);
    });
  });

  it("今日已签到：按钮翻转禁用态", async () => {
    mockRoute("get_workbuddy_session_health", () => health);
    mockRoute("workbuddy_checkin_status", () => ({ ...checkinStatus, today_checked_in: true }));
    mockRoute("get_workbuddy_auto_checkin_status", () => autoStatus([]));
    render(<WorkBuddyCheckinPage active={true} />);

    const button = await screen.findByTestId("workbuddy-checkin-button");
    expect(button).toHaveTextContent("今日已签到");
    expect(button).toBeDisabled();
  });
});
