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

import { notify } from "../src/platform/notify";
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
  // P19.2④ 档案字段（list_workbuddy_accounts 全量返回）。
  display_name: "",
  archived: false,
  masked_phone: "",
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

/** P19-B5 后端契约（be-engineer 9609f9a）：overview 条目构造器。 */
const overviewRow = (
  profileId: string,
  screenName: string,
  over: Partial<Record<string, unknown>> = {},
) => ({
  profile_id: profileId,
  screen_name: screenName,
  today_checked_in: false,
  streak_days: 6,
  daily_credit: 100,
  total_credits: 700,
  credits_balance: 420,
  credits_total: 840,
  balance_error: null,
  masked_phone: "133****1304",
  checkin_dates: ["2026-10-20", "2026-10-19"],
  error: null,
  ...over,
});

function installWorkbuddyRoutes(options?: {
  pool?: typeof defaultPool;
  accounts?: readonly { profile_id: string; screen_name: string; enabled: boolean }[];
  /** get_workbuddy_checkin_overview 回执（缺省两账号 ok 行）。 */
  overview?: readonly Record<string, unknown>[];
}) {
  mockRoute("get_workbuddy_session_health", () => health);
  mockRoute("list_workbuddy_accounts", () => options?.pool ?? defaultPool);
  mockRoute("workbuddy_checkin_status", () => checkinStatus);
  mockRoute("get_workbuddy_checkin_overview", () =>
    options?.overview ?? [
      overviewRow("wb-uid-1", "甲账号"),
      overviewRow("wb-uid-2", "乙账号"),
    ]);
  mockRoute("workbuddy_checkin_claim_profile", () => ({ credit: 10, streak_days: 7, verified: true }));
  mockRoute("workbuddy_checkin_claim_batch", (args: { profileIds: string[] }) =>
    args.profileIds.map((profileId) => ({
      profile_id: profileId,
      screen_name: profileId === "wb-uid-1" ? "甲账号" : "乙账号",
      outcome: "claimed",
      credit: 10,
      detail_code: null,
    })));
  mockRoute("list_workbuddy_checkin_accounts", () =>
    options?.accounts ?? [
      { profile_id: "wb-uid-1", screen_name: "甲账号", enabled: true },
      { profile_id: "wb-uid-2", screen_name: "乙账号", enabled: false },
    ]);
  mockRoute("get_workbuddy_auto_checkin_status", () =>
    autoStatus(options?.accounts ?? [
      { profile_id: "wb-uid-1", screen_name: "甲账号", enabled: true },
      { profile_id: "wb-uid-2", screen_name: "乙账号", enabled: false },
    ]));
  mockRoute("set_workbuddy_account_auto_checkin", () => undefined);
  mockRoute("set_workbuddy_auto_checkin_settings", () => undefined);
  mockRoute("preview_workbuddy_switch", () => ({
    current_label: "甲账号",
    target_label: "乙账号",
    pending_local_rows: 12,
    pending_project_count: 3,
    current_account_rows: 40,
    cloud_residual_rows: 0,
  }));
  mockRoute("switch_workbuddy_account", () => ({
    switched: true,
    target_label: "乙账号",
    previous_label: "甲账号",
    transfer_executed: true,
    updated_rows: 12,
    verified_sessions: 12,
    missing_body_files: 0,
    cloud_residual_rows: 0,
    transfer_error: null,
    restarted: true,
    post_status: "active",
  }));
  mockRoute("begin_workbuddy_login", () => ({ auth_url: "https://auth.example/xyz", attempt_id: 7 }));
  mockRoute("complete_workbuddy_login", () => ({
    profile_id: "wb-uid-9", account_id: "uid-9", screen_name: "新账号", acceptance: "passed",
  }));
  mockRoute("cancel_workbuddy_login", () => null);
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
  it("骨架渲染：页头产品徽章 + 池清单行走 SharedAccountEntry 规格 + 动作在册", async () => {
    installWorkbuddyRoutes();
    render(<WorkBuddyPanel active={true} />);

    expect(await screen.findByTestId("workbuddy-pool-list")).toBeInTheDocument();
    expect(screen.getByTestId("product-badge-workbuddy")).toBeInTheDocument();
    // 池清单行对齐账号页模板（account-list__row + 切换按钮 + 池清单锚点保留）。
    const list = screen.getByTestId("workbuddy-pool-list");
    expect(list.className).toContain("account-list");
    expect(list.querySelectorAll(".account-list__row")).toHaveLength(2);
    // 当前客户端登录账号行显示「使用中」chip（与 AccountCenter 同语义），非当前行才是切换按钮。
    expect(screen.getByTestId("workbuddy-pool-current-wb-uid-1")).toHaveTextContent("使用中");
    expect(screen.getByTestId("workbuddy-switch-wb-uid-2")).toHaveTextContent("切换到此账号");
    // 页头动作（刷新/续期/健康检测/捕获/添加账号）在册（P19.1：过户按钮退役）。
    for (const testId of ["workbuddy-refresh", "workbuddy-renew-credentials", "workbuddy-deep-health-button", "workbuddy-capture-button", "workbuddy-add-account-button"]) {
      expect(screen.getByTestId(testId)).toBeInTheDocument();
    }
  });

  it("能力开关生效：creditsColumn 逐账号余额块（P19.2③ credits_balance 主展示）+ 「使用中」标记", async () => {
    installWorkbuddyRoutes();
    // wb-uid-2 带脱敏手机号（验证卡行直显）。
    mockRoute("list_workbuddy_accounts", () => [
      poolEntry("wb-uid-1", "uid-1", "甲账号"),
      { ...poolEntry("wb-uid-2", "uid-2", "乙账号"), masked_phone: "133****1304" },
    ]);
    render(<WorkBuddyPanel active={true} />);

    await screen.findByTestId("workbuddy-pool-list");
    // P19.2③：主展示 = credits_balance（billing 真实口径）。
    expect(screen.getByTestId("workbuddy-credits-uid-1")).toHaveTextContent("420");
    expect(screen.getByTestId("workbuddy-credits-uid-2")).toHaveTextContent("420");
    // 次要展示：本期签到累计（total_credits 活动域）进 meta 行。
    expect(screen.getByTestId("workbuddy-pool-list").textContent).toContain("本期签到累计 700");
    // P19.2④：脱敏手机号卡行直显（服务端预脱敏形态，有值才显示）。
    expect(screen.getByTestId("workbuddy-pool-list").textContent).toContain("133****1304");
    // 使用中 chip 仍只标当前客户端登录账号。
    expect(screen.getByTestId("workbuddy-pool-current-wb-uid-1")).toHaveTextContent("使用中");
    expect(screen.queryByTestId("workbuddy-pool-current-wb-uid-2")).not.toBeInTheDocument();
  });

  it("详情实装：行点击进详情，凭据/积分/参与开关在册，档案管理实装", async () => {
    installWorkbuddyRoutes();
    // wb-uid-2 带脱敏手机号（验证「有值才显示手机号行」）。
    mockRoute("list_workbuddy_accounts", () => [
      poolEntry("wb-uid-1", "uid-1", "甲账号"),
      { ...poolEntry("wb-uid-2", "uid-2", "乙账号"), masked_phone: "133****1304" },
    ]);
    render(<WorkBuddyPanel active={true} />);

    await screen.findByTestId("workbuddy-pool-list");
    fireEvent.click(screen.getByTestId("workbuddy-pool-card-wb-uid-2"));
    // 详情视图在册（registry detailView: full）：基础信息 + 凭据健康度 + 积分。
    expect(await screen.findByTestId("workbuddy-account-detail")).toBeInTheDocument();
    expect(screen.getByTestId("workbuddy-detail-status")).toHaveTextContent("正常");
    expect(screen.getByTestId("workbuddy-detail-current")).toHaveTextContent("不是当前账号");
    // P19.2③：积分余额主展示 = credits_balance；本期签到累计 = total_credits。
    expect(screen.getByTestId("workbuddy-detail-credits")).toHaveTextContent("420");
    expect(screen.getByTestId("workbuddy-detail-accumulated")).toHaveTextContent("700");
    expect(screen.getByTestId("workbuddy-detail-streak")).toHaveTextContent("6");
    // P19.2④：脱敏手机号只读行在册（池记录已采集）。
    expect(screen.getByTestId("workbuddy-detail-phone")).toHaveTextContent("133****1304");
    // 凭据到期时间轴（池 DTO 数据源）。
    expect(screen.getByText("访问凭据（access）").nextElementSibling).toHaveTextContent("剩");
    // 参与开关 = 后端有效参与（wb-uid-2 显式关闭 → 未勾选）。
    expect((screen.getByTestId("workbuddy-detail-participation-toggle").querySelector("input") as HTMLInputElement).checked).toBe(false);
    // P19.2④：档案管理实装（备注名/归档/删除），缺席标注退役。
    expect(screen.getByTestId("workbuddy-detail-alias-input")).toBeInTheDocument();
    expect(screen.getByTestId("workbuddy-detail-archived-toggle")).toBeInTheDocument();
    expect(screen.getByTestId("workbuddy-detail-purge")).toBeInTheDocument();
    // 返回列表。
    fireEvent.click(screen.getByTestId("workbuddy-account-detail-back"));
    expect(await screen.findByTestId("workbuddy-pool-list")).toBeInTheDocument();
  });

  it("展示名优先级（P19.2④）：备注名 > 捕获昵称 > 兜底标签", async () => {
    mockRoute("get_workbuddy_session_health", () => health);
    mockRoute("list_workbuddy_accounts", () => [
      { ...poolEntry("wb-a", "uid-a", "A账号"), display_name: "备注优先" },
      { ...poolEntry("wb-b", "uid-b", ""), display_name: "" },
      { ...poolEntry("wb-c", "uid-c", "C账号"), display_name: "" },
    ]);
    mockRoute("get_workbuddy_checkin_overview", () => []);
    render(<WorkBuddyPanel active={true} />);

    await screen.findByTestId("workbuddy-pool-list");
    const names = () =>
      screen.getAllByTestId(/^workbuddy-pool-card-/)
        .map((row) => row.querySelector("strong")?.textContent);
    expect(names()).toEqual(["备注优先", "WorkBuddy 账号", "C账号"]);
    // 手机号未采集 → 卡行整段隐藏（不显示占位符）。
    expect(screen.getByTestId("workbuddy-pool-list").textContent).not.toContain("****");
  });

  it("档案管理（P19.2④）：备注名编辑写 set_workbuddy_account_display_name（accountId 口径）", async () => {
    installWorkbuddyRoutes();
    mockRoute("set_workbuddy_account_display_name", () => ({
      profile_id: "wb-uid-2", account_id: "uid-2", screen_name: "乙账号",
      domain: "workbuddy.example", access_token_expires_at_unix_seconds: 1799999999,
      refresh_token_expires_at_unix_seconds: 1799999999, minted_at_unix_seconds: 1000,
      last_updated_at_unix_seconds: 2000, display_name: "我的小号", archived: false, masked_phone: "",
    }));
    render(<WorkBuddyPanel active={true} />);

    await screen.findByTestId("workbuddy-pool-list");
    fireEvent.click(screen.getByTestId("workbuddy-pool-card-wb-uid-2"));
    const input = await screen.findByTestId("workbuddy-detail-alias-input") as HTMLInputElement;
    fireEvent.change(input, { target: { value: "我的小号" } });
    fireEvent.click(screen.getByTestId("workbuddy-detail-alias-save"));
    await waitFor(() => {
      expect(mockCoreInvoke.mock.calls).toContainEqual(
        ["set_workbuddy_account_display_name", { accountId: "uid-2", displayName: "我的小号" }],
      );
    });
    expect(vi.mocked(notify.success)).toHaveBeenCalledWith("备注名已保存。");
  });

  it("档案管理（P19.2④）：归档开关写 set_workbuddy_account_archived 并提示不退出参与集", async () => {
    installWorkbuddyRoutes();
    mockRoute("set_workbuddy_account_archived", () => ({
      profile_id: "wb-uid-2", account_id: "uid-2", screen_name: "乙账号",
      domain: "workbuddy.example", access_token_expires_at_unix_seconds: 1799999999,
      refresh_token_expires_at_unix_seconds: 1799999999, minted_at_unix_seconds: 1000,
      last_updated_at_unix_seconds: 2000, display_name: "", archived: true, masked_phone: "",
    }));
    render(<WorkBuddyPanel active={true} />);

    await screen.findByTestId("workbuddy-pool-list");
    fireEvent.click(screen.getByTestId("workbuddy-pool-card-wb-uid-2"));
    const toggle = await screen.findByTestId("workbuddy-detail-archived-toggle");
    // 提示文案写明：归档不自动退出签到参与集。
    expect(toggle.textContent).toContain("不会自动退出签到参与集");
    // 手机号未采集 → 整行隐藏（不显示占位符）。
    expect(screen.queryByTestId("workbuddy-detail-phone")).not.toBeInTheDocument();
    fireEvent.click(toggle.querySelector("input") as HTMLInputElement);
    await waitFor(() => {
      expect(mockCoreInvoke.mock.calls).toContainEqual(
        ["set_workbuddy_account_archived", { accountId: "uid-2", archived: true }],
      );
    });
    expect(vi.mocked(notify.success)).toHaveBeenCalledWith("账号已归档。");
  });

  it("档案管理（P19.2④）：删除走单次确认，边界文案写明不碰客户端登录态", async () => {
    installWorkbuddyRoutes();
    mockRoute("purge_workbuddy_accounts", () => ({
      confirmed: true,
      purged_accounts: 1,
      missing_accounts: [],
      profile_cleanup_failed: [],
      remaining_accounts: 1,
    }));
    render(<WorkBuddyPanel active={true} />);

    await screen.findByTestId("workbuddy-pool-list");
    fireEvent.click(screen.getByTestId("workbuddy-pool-card-wb-uid-2"));
    fireEvent.click(await screen.findByTestId("workbuddy-detail-purge"));
    const dialog = await screen.findByTestId("workbuddy-detail-purge-dialog");
    // 边界文案：不触碰客户端 .info / 不登出；删除后不可切号/直连签到。
    expect(dialog.textContent).toContain("不会改动 WorkBuddy 客户端的登录状态");
    expect(dialog.textContent).toContain("不可再切换登录或直连签到");
    fireEvent.click(screen.getByTestId("workbuddy-detail-purge-dialog-confirm"));
    await waitFor(() => {
      expect(mockCoreInvoke.mock.calls).toContainEqual(
        ["purge_workbuddy_accounts", { accountIds: ["uid-2"], confirmed: true }],
      );
    });
    await waitFor(() => {
      expect(notify.success).toHaveBeenCalledWith(
        expect.stringContaining("已删除"),
      );
    });
  });

  it("详情参与开关：翻转写 set_workbuddy_account_auto_checkin 并回读有效参与", async () => {
    installWorkbuddyRoutes();
    render(<WorkBuddyPanel active={true} />);

    await screen.findByTestId("workbuddy-pool-list");
    fireEvent.click(screen.getByTestId("workbuddy-pool-card-wb-uid-2"));
    const toggle = await screen.findByTestId("workbuddy-detail-participation-toggle");
    fireEvent.click(toggle.querySelector("input") as HTMLInputElement);
    await waitFor(() => {
      const calls = mockCoreInvoke.mock.calls.filter(([cmd]) => cmd === "set_workbuddy_account_auto_checkin");
      expect(calls).toContainEqual(["set_workbuddy_account_auto_checkin", { profileId: "wb-uid-2", enabled: true }]);
    });
    // 回读后开关翻转（mock 回执不变，仍断言写入调用与提示）。
    expect(notify.success).toHaveBeenCalledWith("WorkBuddy：已加入每日自动签到。");
  });

  it("余额 fail-soft（P19.2③）：status ok 但余额查询失败 → 卡行「--」不报错，累计照常", async () => {
    installWorkbuddyRoutes({ overview: [
      overviewRow("wb-uid-1", "甲账号", { credits_balance: null, credits_total: null }),
      overviewRow("wb-uid-2", "乙账号", { credits_balance: null, credits_total: null }),
    ] });
    render(<WorkBuddyPanel active={true} />);

    await screen.findByTestId("workbuddy-pool-list");
    // be 5dc3b72 契约：余额查询失败（error 仍空）→ 显示「--」，不要报错弹窗。
    expect(screen.getByTestId("workbuddy-credits-uid-1")).toHaveTextContent("--");
    // 本期签到累计不受余额查询失败影响。
    expect(screen.getByTestId("workbuddy-pool-list").textContent).toContain("本期签到累计 700");
  });

  it("余额诊断（P19.2⑤）：balance_error 映射人话进卡行悬浮与详情页「--」处", async () => {
    installWorkbuddyRoutes({ overview: [
      overviewRow("wb-uid-1", "甲账号", { credits_balance: null, credits_total: null, balance_error: "workbuddy_balance_auth_mismatch" }),
      overviewRow("wb-uid-2", "乙账号", { credits_balance: null, credits_total: null, balance_error: "workbuddy_balance_unreachable" }),
    ] });
    render(<WorkBuddyPanel active={true} />);

    await screen.findByTestId("workbuddy-pool-list");
    // 卡行仍「--」，悬浮提示 = safeUiError 人话（码不进界面）。
    expect(screen.getByTestId("workbuddy-credits-uid-1")).toHaveTextContent("--");
    expect(screen.getByTestId("workbuddy-credits-uid-1").closest("div")?.getAttribute("title"))
      .toContain("登录凭据已失效");

    // 详情页在「--」处展示原因。
    fireEvent.click(screen.getByTestId("workbuddy-pool-card-wb-uid-2"));
    expect(await screen.findByTestId("workbuddy-account-detail")).toBeInTheDocument();
    expect(screen.getByTestId("workbuddy-detail-credits")).toHaveTextContent("--（网络异常，请稍后刷新。）");
    // 稳定码不进界面。
    expect(screen.getByTestId("workbuddy-detail-credits").textContent).not.toContain("workbuddy_balance");
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

describe("WorkBuddyCheckinPage（P19-B5 签到页逐账号行 + 后端契约消费）", () => {
  it("骨架渲染：逐账号行（复选+昵称+额度/连签 meta）+ 批量按钮组 + 自动签到状态行", async () => {
    installWorkbuddyRoutes();
    render(<WorkBuddyCheckinPage active={true} />);

    // 批量区块承载逐账号行（checkin-flow stagger 类名冻结）。
    await screen.findByTestId("checkin-run-all");
    expect(screen.getByTestId("checkin-run-all")).toHaveTextContent("一键全签（2）");
    const flow = document.querySelector("ul.checkin-flow");
    expect(flow).not.toBeNull();
    expect(flow!.querySelectorAll(".checkin-flow__row")).toHaveLength(2);
    // 行内 meta：额度列 + 连签（IA 草案 §1.3 签到账号行字段）。
    expect(flow!.textContent).toContain("额度 700 积分 · 连签 6 天");
    // P19.2：签到历史点阵条删除（用户拍板「显示签到的账号就行」）。
    expect(screen.queryByTestId("workbuddy-checkin-history")).not.toBeInTheDocument();
    expect(screen.queryByTestId("workbuddy-history-wb-uid-1")).not.toBeInTheDocument();
    expect(screen.getByTestId("workbuddy-auto-checkin-meta")).toHaveTextContent("每日 09:30 触发");
    expect(screen.getByTestId("product-badge-workbuddy")).toBeInTheDocument();
  });

  it("行内单签：签到按钮触发 claim_profile 并落行内结果", async () => {
    installWorkbuddyRoutes();
    render(<WorkBuddyCheckinPage active={true} />);

    const inline = await screen.findByTestId("checkin-inline-wb-uid-1");
    expect(inline).toBeEnabled();
    fireEvent.click(inline);
    await waitFor(() => {
      expect(mockCoreInvoke.mock.calls).toContainEqual(
        ["workbuddy_checkin_claim_profile", { profileId: "wb-uid-1" }],
      );
    });
    // 行内结果徽章（WB 通用档文案：签到成功）。
    await screen.findByText("签到成功");
  });

  it("一键全签：后端批量逐条回执（workbuddy_checkin_claim_batch）", async () => {
    installWorkbuddyRoutes();
    render(<WorkBuddyCheckinPage active={true} />);

    fireEvent.click(await screen.findByTestId("checkin-run-all"));
    await waitFor(() => {
      expect(mockCoreInvoke.mock.calls).toContainEqual(
        ["workbuddy_checkin_claim_batch", { profileIds: ["wb-uid-1", "wb-uid-2"] }],
      );
    });
    // 逐行结果落位（claimed → 签到成功，两行都有）。
    expect((await screen.findAllByText("签到成功"))).toHaveLength(2);
  });

  it("一键补签：只对今日未签账号执行", async () => {
    installWorkbuddyRoutes({ overview: [
      overviewRow("wb-uid-1", "甲账号", { today_checked_in: true }),
      overviewRow("wb-uid-2", "乙账号"),
    ] });
    render(<WorkBuddyCheckinPage active={true} />);

    const pendingButton = await screen.findByTestId("checkin-run-pending");
    expect(pendingButton).toHaveTextContent("一键补签（1）");
    fireEvent.click(pendingButton);
    await waitFor(() => {
      expect(mockCoreInvoke.mock.calls).toContainEqual(
        ["workbuddy_checkin_claim_batch", { profileIds: ["wb-uid-2"] }],
      );
    });
  });

  it("行级 fail-soft：单账号 error 落条目标注「状态未知」，不阻塞他行", async () => {
    installWorkbuddyRoutes({ overview: [
      overviewRow("wb-uid-1", "甲账号", {
        error: "workbuddy_checkin_auth_mismatch",
        today_checked_in: null, streak_days: null, daily_credit: null, total_credits: null,
        credits_balance: null, credits_total: null, masked_phone: null, checkin_dates: [],
      }),
      overviewRow("wb-uid-2", "乙账号"),
    ] });
    render(<WorkBuddyCheckinPage active={true} />);

    await screen.findByTestId("checkin-run-all");
    // 行徽章 fail-soft 标注（历史条已随 P19.2 删除，仅行内一处）。
    expect(screen.getAllByText("状态未知")).toHaveLength(1);
    // 他行正常渲染额度 meta；失败行不可单签。
    expect(document.querySelector("ul.checkin-flow")!.textContent).toContain("额度 700 积分");
    expect(screen.getByTestId("checkin-inline-wb-uid-1")).toBeDisabled();
  });

  it("批量不可中断：不渲染取消按钮（后端整体批次语义，诚实 UI）", async () => {
    installWorkbuddyRoutes();
    render(<WorkBuddyCheckinPage active={true} />);

    await screen.findByTestId("checkin-run-all");
    expect(screen.queryByRole("button", { name: "取消未开始任务" })).not.toBeInTheDocument();
  });

  it("参与集（后端有效参与）：默认选择 = 参与集且不触发归一写操作", async () => {
    installWorkbuddyRoutes({ accounts: [
      { profile_id: "wb-uid-1", screen_name: "甲账号", enabled: true },
      { profile_id: "wb-uid-2", screen_name: "乙账号", enabled: false },
    ] });
    render(<WorkBuddyCheckinPage active={true} />);

    await screen.findByTestId("checkin-run-all");
    // 批量选择默认选中参与集（IA §1.3：wb-uid-2 被显式关闭 → 不在默认选择内）。
    expect(screen.getByText("已选 1/2")).toBeInTheDocument();
    // 无前端归一化写操作（参与开关 UI 已随设置卡归位设置页，P19.1）。
    const calls = mockCoreInvoke.mock.calls.filter(([cmd]) => cmd === "set_workbuddy_account_auto_checkin");
    expect(calls).toHaveLength(0);
  });

  it("今日已签到：行内单签禁用 + 签到槽徽章翻转", async () => {
    installWorkbuddyRoutes({ overview: [
      overviewRow("wb-uid-1", "甲账号", { today_checked_in: true }),
      overviewRow("wb-uid-2", "乙账号"),
    ] });
    render(<WorkBuddyCheckinPage active={true} />);

    const inline = await screen.findByTestId("checkin-inline-wb-uid-1");
    expect(inline).toBeDisabled();
    expect(screen.getByTestId("checkin-inline-wb-uid-2")).toBeEnabled();
  });
});


describe("WorkBuddyPanel 添加账号（P19-B4 登录编排消费）", () => {
  beforeEach(() => {
    resetRoutes();
    installFakeInvoke();
    window.localStorage.clear();
    installWorkbuddyRoutes();
  });

  it("页头主操作：添加账号按钮在册（oauth-browser 能力开关），capture 并存降为普通按钮", async () => {
    render(<WorkBuddyPanel active={true} />);

    const add = await screen.findByTestId("workbuddy-add-account-button");
    expect(add).toHaveTextContent("添加账号");
    expect(add.className).toContain("btn--primary");
    expect(screen.getByTestId("workbuddy-capture-button")).toBeInTheDocument();
    expect(screen.getByTestId("workbuddy-capture-button").className).not.toContain("btn--primary");
  });

  it("登录成功流：begin→complete→入池刷新；按钮翻转为等待态", async () => {
    // complete 受控：等待态可被断言（真实命令为 1s×5min 阻塞轮询）。
    let resolveComplete: ((receipt: unknown) => void) = () => undefined;
    mockRoute("complete_workbuddy_login", () => new Promise((resolve) => {
      resolveComplete = resolve;
    }));
    render(<WorkBuddyPanel active={true} />);

    const add = await screen.findByTestId("workbuddy-add-account-button");
    fireEvent.click(add);
    expect(await screen.findByTestId("workbuddy-add-account-button")).toHaveTextContent("等待浏览器登录完成…");
    expect(screen.getByTestId("workbuddy-login-cancel")).toBeInTheDocument();
    await waitFor(() => {
      expect(mockCoreInvoke.mock.calls.some(([cmd]) => cmd === "begin_workbuddy_login")).toBe(true);
      expect(mockCoreInvoke.mock.calls).toContainEqual(
        ["complete_workbuddy_login", { attemptId: 7 }],
      );
    });
    resolveComplete({
      profile_id: "wb-uid-9", account_id: "uid-9", screen_name: "新账号", acceptance: "passed",
    });
    await waitFor(() => {
      expect(vi.mocked(notify.success)).toHaveBeenCalledWith(
        expect.stringContaining("新账号"),
      );
    });
    // 成功后按钮复位 + 池刷新（list 二次调用）。
    await waitFor(() => {
      expect(screen.getByTestId("workbuddy-add-account-button")).toHaveTextContent("添加账号");
    });
    const listCalls = mockCoreInvoke.mock.calls.filter(([cmd]) => cmd === "list_workbuddy_accounts");
    expect(listCalls.length).toBeGreaterThanOrEqual(2);
  });

  it("acceptance failed：账号已保存但如实警告，不自动重试", async () => {
    mockRoute("complete_workbuddy_login", () => ({
      profile_id: "wb-uid-9", account_id: "uid-9", screen_name: "新账号",
      acceptance: "failed", acceptance_error_code: "device_refresh_rejected",
    }));
    render(<WorkBuddyPanel active={true} />);

    fireEvent.click(await screen.findByTestId("workbuddy-add-account-button"));
    await waitFor(() => {
      expect(vi.mocked(notify.warning)).toHaveBeenCalledWith(
        expect.stringContaining("device_refresh_rejected"),
        expect.any(String),
      );
    });
  });

  it("取消等待：cancel 令 complete 以 cancelled 返回，中性提示收尾", async () => {
    let rejectComplete: ((reason: unknown) => void) | null = null;
    mockRoute("complete_workbuddy_login", () => new Promise((_resolve, reject) => {
      rejectComplete = reject;
    }));
    mockRoute("cancel_workbuddy_login", () => {
      rejectComplete?.("workbuddy_login_cancelled");
      return null;
    });
    render(<WorkBuddyPanel active={true} />);

    fireEvent.click(await screen.findByTestId("workbuddy-add-account-button"));
    fireEvent.click(await screen.findByTestId("workbuddy-login-cancel"));
    await waitFor(() => {
      expect(mockCoreInvoke.mock.calls).toContainEqual(
        ["cancel_workbuddy_login", { attemptId: 7 }],
      );
    });
    await waitFor(() => {
      expect(vi.mocked(notify.info)).toHaveBeenCalledWith(
        expect.stringContaining("登录已取消"),
      );
    });
  });

  it("重复发起保护：already_in_progress 错误码映射人话", async () => {
    mockRoute("begin_workbuddy_login", () => Promise.reject("workbuddy_login_already_in_progress"));
    render(<WorkBuddyPanel active={true} />);

    fireEvent.click(await screen.findByTestId("workbuddy-add-account-button"));
    await waitFor(() => {
      expect(vi.mocked(notify.error)).toHaveBeenCalledWith(
        expect.stringContaining("已有登录正在进行"),
      );
    });
  });
});

describe("WorkBuddyPanel 切号两段式（P19.1② preview + confirmed switch 契约消费）", () => {
  beforeEach(() => {
    resetRoutes();
    installFakeInvoke();
    window.localStorage.clear();
    installWorkbuddyRoutes();
  });

  it("选账号 → preview 影响面弹窗（待过户数在册）→ 确认后 switch(targetAccountId, confirmed=true)", async () => {
    render(<WorkBuddyPanel active={true} />);

    // 选账号动作先调 preview（只读零副作用），弹窗带影响面数字。
    fireEvent.click(await screen.findByTestId("workbuddy-switch-wb-uid-2"));
    const dialog = await screen.findByTestId("workbuddy-switch-dialog");
    expect(dialog.textContent).toContain("「甲账号」");
    expect(dialog.textContent).toContain("「乙账号」");
    expect(dialog.textContent).toContain("12 条本地会话（3 个项目）");
    // preview 以 account_id 调用（be 契约：targetAccountId = 池行 account_id）。
    expect(mockCoreInvoke.mock.calls).toContainEqual(
      ["preview_workbuddy_switch", { targetAccountId: "uid-2" }],
    );

    fireEvent.click(screen.getByTestId("workbuddy-switch-dialog-confirm"));
    await waitFor(() => {
      expect(mockCoreInvoke.mock.calls).toContainEqual(
        ["switch_workbuddy_account", { targetAccountId: "uid-2", confirmed: true }],
      );
    });
    await waitFor(() => {
      expect(vi.mocked(notify.success)).toHaveBeenCalledWith(
        expect.stringContaining("已切换登录到「乙账号」"),
        undefined,
      );
    });
    expect(vi.mocked(notify.success)).toHaveBeenCalledWith(
      expect.stringContaining("12 条本地会话已随行"),
      undefined,
    );
  });

  it("部分成功：switched=true 且 transfer_error 非空 → 成功通知 + 过户失败警告（切号不回滚）", async () => {
    mockRoute("switch_workbuddy_account", () => ({
      switched: true,
      target_label: "乙账号",
      previous_label: "甲账号",
      transfer_executed: null,
      updated_rows: null,
      verified_sessions: null,
      missing_body_files: null,
      cloud_residual_rows: 2,
      transfer_error: "workbuddy_transfer_db_locked",
      restarted: true,
      post_status: "active",
    }));
    render(<WorkBuddyPanel active={true} />);

    fireEvent.click(await screen.findByTestId("workbuddy-switch-wb-uid-2"));
    await screen.findByTestId("workbuddy-switch-dialog");
    fireEvent.click(screen.getByTestId("workbuddy-switch-dialog-confirm"));
    await waitFor(() => {
      expect(vi.mocked(notify.success)).toHaveBeenCalledWith(
        expect.stringContaining("已切换登录到「乙账号」"),
        undefined,
      );
    });
    expect(vi.mocked(notify.warning)).toHaveBeenCalledWith(
      expect.stringContaining("历史会话过户失败"),
      expect.stringContaining("切号本身不受影响"),
    );
    // cloud 红线：>0 时成功通知明示不随行。
    expect(vi.mocked(notify.success)).toHaveBeenCalledWith(
      expect.stringContaining("2 条云端会话不随行"),
      undefined,
    );
  });

  it("preview 失败不开弹窗：如实报错（客户端登录面不可读时 switch 也必然失败）", async () => {
    mockRoute("preview_workbuddy_switch", () => Promise.reject("workbuddy_data_root_unavailable"));
    render(<WorkBuddyPanel active={true} />);

    fireEvent.click(await screen.findByTestId("workbuddy-switch-wb-uid-2"));
    await waitFor(() => {
      expect(vi.mocked(notify.error)).toHaveBeenCalledWith(
        expect.stringContaining("切号预览未完成"),
      );
    });
    expect(screen.queryByTestId("workbuddy-switch-dialog")).not.toBeInTheDocument();
    // 未发起 switch（confirmed 入口未触达）。
    expect(mockCoreInvoke.mock.calls.some(([cmd]) => cmd === "switch_workbuddy_account")).toBe(false);
  });
});
