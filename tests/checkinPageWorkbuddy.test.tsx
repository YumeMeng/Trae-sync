import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { installFakeInvoke, mockRoute, mockCoreInvoke, resetRoutes } from "./helpers/fakeInvoke";

/**
 * 签到页 WorkBuddy 形态（P4c-9b-2 从 tests/WorkBuddyPanel.test.tsx 迁出保留）。
 *
 * 迁出动机：原文件测的是旧账号页 WorkBuddyPanel（已随槽位切换删除），但其中 8 例
 * 断言的是**仍在线上**的 features/checkin/CheckinPage（WB 形态）。它们与
 * tests/CheckinPage.test.tsx / tests/CheckinShared.test.tsx 不重复（那两份覆盖 TRAE
 * 形态与 WB 事件名/批量条），故按「活代码的覆盖不随旧组件一起删」保留于此，
 * 断言与 fixture 逐字未改。
 */

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

import { CheckinPage } from "../src/features/checkin/CheckinPage";
import { WORKBUDDY_PRODUCT_ID } from "../src/types/product";

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

const autoStatus = (accounts: readonly { profile_id: string; screen_name: string; enabled: boolean }[]) => ({
  enabled: true,
  daily_time_hhmm: "09:30",
  ledger: null,
  accounts,
});

/** 签到页 WB 形态所需的路由（逐字沿用原 installWorkbuddyRoutes 的签到子集）。 */
function installWorkbuddyRoutes(options?: {
  accounts?: readonly { profile_id: string; screen_name: string; enabled: boolean }[];
  /** get_workbuddy_checkin_overview 回执（缺省两账号 ok 行）。 */
  overview?: readonly Record<string, unknown>[];
}) {
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
}

beforeEach(() => {
  resetRoutes();
  installFakeInvoke();
  window.localStorage.clear();
});

afterEach(() => {
  window.localStorage.clear();
});

/** 渲染签到页的 WorkBuddy 形态（旧 WorkBuddyCheckinPage 已删，P4b-4）。 */
function renderWbCheckin() {
  return render(
    <CheckinPage productId={WORKBUDDY_PRODUCT_ID} active={true} onNavigate={() => undefined} />,
  );
}

describe("签到页 WorkBuddy 形态（P19-B5 逐账号行 + 后端契约消费；P4b-4 迁到 features/checkin）", () => {
  it("骨架渲染：逐账号行（复选+昵称+额度/连签 meta）+ 批量按钮组 + 自动签到状态行", async () => {
    installWorkbuddyRoutes();
    renderWbCheckin();

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

  it("行内单签：签到按钮只对该账号发起领取并落行内结果", async () => {
    installWorkbuddyRoutes();
    renderWbCheckin();

    const inline = await screen.findByTestId("checkin-inline-wb-uid-1");
    expect(inline).toBeEnabled();
    fireEvent.click(inline);
    // P4b-2 合并页统一走批次通道（platform/checkin 的 runWorkBuddyCheckin）：
    // 行内单签 = 只含该账号的一批，后端契约仍是 fail-soft 逐条回执。
    await waitFor(() => {
      expect(mockCoreInvoke.mock.calls).toContainEqual(
        ["workbuddy_checkin_claim_batch", { profileIds: ["wb-uid-1"] }],
      );
    });
    // 未勾选其他账号：不得连带请求乙账号。
    const batchCalls = mockCoreInvoke.mock.calls.filter(([cmd]) => cmd === "workbuddy_checkin_claim_batch");
    expect(batchCalls).toHaveLength(1);
    // 行内结果徽章（WB 通用档文案：签到成功）。
    await screen.findByText("签到成功");
  });

  it("一键全签：后端批量逐条回执（workbuddy_checkin_claim_batch）", async () => {
    installWorkbuddyRoutes();
    renderWbCheckin();

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
    renderWbCheckin();

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
    renderWbCheckin();

    await screen.findByTestId("checkin-run-all");
    // 行徽章 fail-soft 标注（历史条已随 P19.2 删除，仅行内一处）。
    expect(screen.getAllByText("状态未知")).toHaveLength(1);
    // 他行正常渲染额度 meta；失败行不可单签。
    expect(document.querySelector("ul.checkin-flow")!.textContent).toContain("额度 700 积分");
    expect(screen.getByTestId("checkin-inline-wb-uid-1")).toBeDisabled();
  });

  it("批量不可中断：不渲染取消按钮（后端整体批次语义，诚实 UI）", async () => {
    installWorkbuddyRoutes();
    renderWbCheckin();

    await screen.findByTestId("checkin-run-all");
    expect(screen.queryByRole("button", { name: "取消未开始任务" })).not.toBeInTheDocument();
  });

  it("参与集（后端有效参与）：默认选择 = 参与集且不触发归一写操作", async () => {
    installWorkbuddyRoutes({ accounts: [
      { profile_id: "wb-uid-1", screen_name: "甲账号", enabled: true },
      { profile_id: "wb-uid-2", screen_name: "乙账号", enabled: false },
    ] });
    renderWbCheckin();

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
    renderWbCheckin();

    const inline = await screen.findByTestId("checkin-inline-wb-uid-1");
    expect(inline).toBeDisabled();
    expect(screen.getByTestId("checkin-inline-wb-uid-2")).toBeEnabled();
  });
});
