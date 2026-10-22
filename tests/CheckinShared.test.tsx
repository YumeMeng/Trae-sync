import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { CheckinBatchBar } from "../src/features/checkin/components/CheckinBatchBar";
import { CheckinFlowRow } from "../src/components/shared/checkin/CheckinFlowRow";
import { CheckinSelectRow } from "../src/components/shared/checkin/CheckinRow";
import { outcomeLabelFor, flowResultTone, selectResultTone } from "../src/components/shared/checkin/OutcomeLabels";
import { CheckinPage } from "../src/features/checkin/CheckinPage";
import { traeCheckinView } from "../src/features/checkin/traeCheckin";
import { workbuddyCheckinView } from "../src/features/checkin/workbuddyCheckin";
import type { CheckinViewInput } from "../src/features/checkin/config";
import type { CheckinSnapshot } from "../src/platform/checkin";
import { CURRENT_PRODUCT_ID, WORKBUDDY_PRODUCT_ID } from "../src/types/product";
import { installFakeInvoke, mockRoute, resetRoutes } from "./helpers/fakeInvoke";

// P4b-4：本文件原为 CheckinSkeleton.test.tsx（旧 CheckinPageSkeleton /
// CheckinActionsRow 的编排断言）。两个旧共享件随旧页删除后成为孤儿被移除，
// 用例逐条迁到替代件：CheckinActionsRow → CheckinBatchBar（动作由视图模型
// 声明），CheckinPageSkeleton → features/checkin/CheckinPage 本体。

vi.mock("@tauri-apps/api/core", async () => {
  const fake = await import("./helpers/fakeInvoke");
  return { invoke: fake.mockCoreInvoke };
});

vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn().mockResolvedValue(() => undefined) }));

vi.mock("../src/platform/notify", () => ({
  notify: {
    success: vi.fn(),
    error: vi.fn(),
    info: vi.fn(),
    warning: vi.fn(),
    summary: vi.fn(),
  },
}));

const noop = () => undefined;

const viewInput: CheckinViewInput = { selection: [], busy: false };

/** TRAE 真实模式快照（三条账号行；能力开启）。 */
function traeSnapshot(count: number): CheckinSnapshot {
  return {
    kind: "trae",
    trae: {
      capability: { enabled: true, transport: "real", real_http_enabled: true, message: "" },
      overview: Array.from({ length: count }, (_, index) => ({
        profile_id: `p${index + 1}`,
        screen_name: `账号${index + 1}`,
        account_id: `a${index + 1}`,
        created_at: "2026-08-01T00:00:00.000Z",
        last_verified_at: "2026-08-22T00:00:00.000Z",
        credits: 100,
        credits_cached_at: "2026-08-22T01:00:00.000Z",
        usage_remaining_credits: 100,
        usage_cached_at: "2026-08-22T01:00:00.000Z",
        checked_in: index === 0,
        access_token_expires_at_unix_seconds: 0,
        refresh_token_expires_at_unix_seconds: 0,
        device_tail: "0000",
        device_id: null,
        display_name: null,
        masked_mobile: "138****0000",
        mobile_full: null,
        auto_checkin_enabled: true,
        refresh_error_code: null,
        credential_legacy: false,
        last_attempt_outcome: null,
        last_attempt_date: null,
      })),
      fixturePool: null,
      auto: null,
      error: null,
    },
  };
}

beforeEach(() => {
  resetRoutes();
  installFakeInvoke();
});

afterEach(() => vi.clearAllMocks());

describe("CheckinBatchBar（P4b-4 替代 CheckinActionsRow：动作由视图模型声明）", () => {
  it("batch 形态：四动作标签来自 TRAE 视图模型，点击按 key 派发，取消仅执行中可用", () => {
    const view = traeCheckinView(traeSnapshot(4), { selection: ["p1", "p2"], busy: false }, CURRENT_PRODUCT_ID);
    const onAction = vi.fn();
    const { rerender } = render(
      <CheckinBatchBar actions={view.actions} progress={{ completed: 0, total: 0 }} busy={false} onAction={onAction} />,
    );
    expect(screen.getByTestId("checkin-run-all")).toHaveTextContent("一键全签（4）");
    expect(screen.getByTestId("checkin-run-pending")).toHaveTextContent("一键补签（3）");
    expect(screen.getByTestId("checkin-run-selected")).toHaveTextContent("签到所选（2）");
    // 空闲态：取消按钮不可用（没有可取消的任务）。
    expect(screen.getByTestId("checkin-cancel")).toBeDisabled();

    fireEvent.click(screen.getByTestId("checkin-run-all"));
    fireEvent.click(screen.getByTestId("checkin-run-selected"));
    expect(onAction.mock.calls.map(([key]) => key)).toEqual(["run-all", "run-selected"]);

    // 全量勾选（4/4）：「签到所选」不出现（全签已覆盖）。
    const allSelected = traeCheckinView(traeSnapshot(4), { selection: ["p1", "p2", "p3", "p4"], busy: false }, CURRENT_PRODUCT_ID);
    rerender(
      <CheckinBatchBar actions={allSelected.actions} progress={{ completed: 0, total: 0 }} busy={false} onAction={onAction} />,
    );
    expect(screen.queryByTestId("checkin-run-selected")).not.toBeInTheDocument();
    // 执行中：执行类按钮全禁用、取消可用（P4b-2 起「执行中」由总进度条
    // 与禁用态表达，主按钮文案不再翻转——与旧 CheckinActionsRow 的差异点）。
    const busyView = traeCheckinView(traeSnapshot(4), { selection: [], busy: true }, CURRENT_PRODUCT_ID);
    rerender(
      <CheckinBatchBar actions={busyView.actions} progress={{ completed: 1, total: 4 }} busy={true} onAction={onAction} />,
    );
    expect(screen.getByTestId("checkin-run-all")).toBeDisabled();
    expect(screen.getByTestId("checkin-run-pending")).toBeDisabled();
    expect(screen.getByTestId("checkin-cancel")).toBeEnabled();
    expect(screen.getByTestId("checkin-progress")).toHaveTextContent("1/4");
  });

  it("select 形态（演示模式）：全部签到（所选语义），未选时禁用", () => {
    const fixtureSnapshot: CheckinSnapshot = {
      kind: "trae",
      trae: {
        capability: { enabled: true, transport: "fixture", real_http_enabled: false, message: "" },
        overview: null,
        fixturePool: [
          { profile_id: "f1", display_name: "演示甲" },
          { profile_id: "f2", display_name: "演示乙" },
          { profile_id: "f3", display_name: "演示丙" },
        ],
        auto: null,
        error: null,
      },
    };
    const empty = traeCheckinView(fixtureSnapshot, { selection: [], busy: false }, CURRENT_PRODUCT_ID);
    const { rerender } = render(
      <CheckinBatchBar actions={empty.actions} progress={{ completed: 0, total: 0 }} busy={false} onAction={noop} />,
    );
    expect(screen.getByTestId("checkin-run-all")).toHaveTextContent("全部签到（0）");
    expect(screen.getByTestId("checkin-run-all")).toBeDisabled();

    const picked = traeCheckinView(fixtureSnapshot, { selection: ["f1", "f2", "f3"], busy: false }, CURRENT_PRODUCT_ID);
    rerender(
      <CheckinBatchBar actions={picked.actions} progress={{ completed: 0, total: 0 }} busy={false} onAction={noop} />,
    );
    expect(screen.getByTestId("checkin-run-all")).toHaveTextContent("全部签到（3）");
    expect(screen.getByTestId("checkin-run-all")).toBeEnabled();
  });

  it("进度条：执行中渲染 X/Y，空闲不渲染", () => {
    const view = traeCheckinView(traeSnapshot(3), { selection: [], busy: true }, CURRENT_PRODUCT_ID);
    const { rerender } = render(
      <CheckinBatchBar actions={view.actions} progress={{ completed: 1, total: 3 }} busy={true} onAction={noop} />,
    );
    expect(screen.getByTestId("checkin-progress")).toHaveTextContent("1/3");
    rerender(<CheckinBatchBar actions={view.actions} progress={{ completed: 1, total: 3 }} busy={false} onAction={noop} />);
    expect(screen.queryByTestId("checkin-progress")).not.toBeInTheDocument();
  });

  it("WorkBuddy 形态：无取消按钮（后端整批不可中断），补签计数只算未签行", () => {
    const snapshot: CheckinSnapshot = {
      kind: "workbuddy",
      workbuddy: {
        rows: [
          { profile_id: "w1", screen_name: "甲", today_checked_in: true, streak_days: 3, daily_credit: 10, total_credits: 700, credits_balance: 1, credits_total: 2, balance_error: null, masked_phone: "", checkin_dates: [], error: null },
          { profile_id: "w2", screen_name: "乙", today_checked_in: false, streak_days: 3, daily_credit: 10, total_credits: 700, credits_balance: 1, credits_total: 2, balance_error: null, masked_phone: "", checkin_dates: [], error: null },
        ],
        participation: null,
        auto: null,
        error: null,
      },
    };
    const view = workbuddyCheckinView(snapshot, viewInput, WORKBUDDY_PRODUCT_ID);
    render(<CheckinBatchBar actions={view.actions} progress={{ completed: 0, total: 0 }} busy={false} onAction={noop} />);
    expect(screen.getByTestId("checkin-run-all")).toHaveTextContent("一键全签（2）");
    expect(screen.getByTestId("checkin-run-pending")).toHaveTextContent("一键补签（1）");
    expect(screen.queryByTestId("checkin-cancel")).not.toBeInTheDocument();
  });
});

describe("OutcomeLabels（P19-B2④ per-product 结果文案表）", () => {
  it("TRAE 表：业务码人话 + 未知业务码原样透出（ADR-0019）", () => {
    expect(outcomeLabelFor("trae_cn", "failed", "business_9095")).toBe("该设备今日名额已用（9095），明日再试");
    expect(outcomeLabelFor("trae_cn", "failed", "business_9999")).toBe("服务端拒绝（9999）");
    expect(outcomeLabelFor("work_cn", "claimed")).toBe("签到成功");
  });

  it("未注册产品回退通用档 + 语气换算助手", () => {
    expect(outcomeLabelFor("unknown_product", "claimed")).toBe("签到成功");
    expect(outcomeLabelFor("unknown_product", "network_error")).toBe("网络错误");
    expect(flowResultTone("already_checked_in")).toBe("ok");
    expect(flowResultTone("claimed")).toBe("ok");
    expect(flowResultTone("network_error")).toBe("warn");
    expect(selectResultTone("already_checked_in")).toBe("neutral");
    expect(selectResultTone("claimed")).toBe("safe");
    expect(selectResultTone(null)).toBe("warning");
  });
});

describe("CheckinFlowRow / CheckinSelectRow（P19-B2④ 槽位化签到行）", () => {
  it("演进式行：running 高亮签到中 + idle 结果文案 + 已签账号行内单签禁用", () => {
    const { rerender } = render(
      <CheckinFlowRow
        productId="trae_cn"
        profileId="p1"
        displayName="账号甲"
        badge={<span>徽章</span>}
        meta="额度 12.5"
        selected={false}
        onToggle={noop}
        disabled={false}
        phase="running"
        result={null}
        waitSecs={null}
        onInlineCheckin={noop}
        inlineEnabled={true}
      />,
    );
    expect(screen.getByText("签到中…")).toBeInTheDocument();
    expect(screen.getByTestId("checkin-inline-p1")).toBeEnabled();

    rerender(
      <CheckinFlowRow
        productId="trae_cn"
        profileId="p1"
        displayName="账号甲"
        badge={<span>徽章</span>}
        meta="额度 12.5"
        selected={true}
        onToggle={noop}
        disabled={false}
        phase="idle"
        result={{ outcome: "already_checked_in", detail_code: null }}
        waitSecs={null}
        onInlineCheckin={noop}
        inlineEnabled={false}
      />,
    );
    expect(screen.getByText("今日已签到")).toBeInTheDocument();
    expect(screen.getByTestId("checkin-inline-p1")).toBeDisabled();
  });

  it("选择行：复选与结果徽章语气联动", () => {
    render(
      <CheckinSelectRow
        name="账号乙"
        meta=""
        badge={null}
        selected={true}
        onToggle={noop}
        disabled={false}
        resultLabel="签到成功"
        resultTone="safe"
      />,
    );
    expect(screen.getByRole("checkbox")).toBeChecked();
    expect(screen.getByText("签到成功").className).toContain("status-badge--safe");
  });
});

describe("签到页（P4b-4 替代 CheckinPageSkeleton 的编排断言）", () => {
  it("门控渲染：首次读取完成前只保留页头标题与产品徽章，不渲染主体", async () => {
    mockRoute("get_checkin_capability", () => ({
      enabled: true,
      transport: "real",
      real_http_enabled: true,
      message: "",
    }));
    // 总览读取受控：保持挂起，页面停在首次读取的门控态。
    mockRoute("get_checkin_overview", () => new Promise(() => undefined));
    mockRoute("get_auto_checkin_settings", () => ({ enabled: false, daily_time_hhmm: "10:00", ledger: null }));

    render(<CheckinPage productId={CURRENT_PRODUCT_ID} active={true} onNavigate={noop} />);

    expect(screen.getByRole("heading", { level: 1, name: "签到" })).toBeInTheDocument();
    expect(screen.getByTestId("product-badge-trae_cn")).toBeInTheDocument();
    expect(screen.getByText("正在读取签到信息…")).toBeInTheDocument();
    // 主体（选择区块 / 进度条 / 行列表）不出。
    expect(screen.queryByTestId("checkin-progress")).not.toBeInTheDocument();
    expect(document.querySelector("ul.checkin-flow")).toBeNull();
  });

  it("批量区块：已选计数 caption + 演进式列表容器（checkin-flow 类名冻结）", async () => {
    mockRoute("get_checkin_capability", () => ({
      enabled: true,
      transport: "real",
      real_http_enabled: true,
      message: "",
    }));
    mockRoute("get_checkin_overview", () => traeSnapshot(3).kind === "trae"
      ? (traeSnapshot(3) as Extract<CheckinSnapshot, { kind: "trae" }>).trae.overview
      : []);
    mockRoute("get_auto_checkin_settings", () => ({ enabled: false, daily_time_hhmm: "10:00", ledger: null }));

    render(<CheckinPage productId={CURRENT_PRODUCT_ID} active={true} onNavigate={noop} />);

    // 默认全选 3/3：取消勾选一个 → 已选 2/3。
    const boxes = await screen.findAllByRole("checkbox");
    expect(boxes).toHaveLength(3);
    fireEvent.click(boxes[0]);
    expect(screen.getByText("已选 2/3")).toBeInTheDocument();
    expect(document.querySelector("ul.checkin-flow")).not.toBeNull();
    expect(document.querySelectorAll(".checkin-flow__row")).toHaveLength(3);
  });

  it("WorkBuddy 形态：产品徽章随槽位注入值切换，逐账号行同构渲染", async () => {
    mockRoute("get_workbuddy_checkin_overview", () => [
      { profile_id: "w1", screen_name: "甲账号", today_checked_in: true, streak_days: 6, daily_credit: 100, total_credits: 700, credits_balance: 420, credits_total: 840, balance_error: null, masked_phone: "", checkin_dates: [], error: null },
    ]);
    mockRoute("list_workbuddy_checkin_accounts", () => [{ profile_id: "w1", screen_name: "甲账号", enabled: true }]);
    mockRoute("get_workbuddy_auto_checkin_status", () => ({ enabled: true, daily_time_hhmm: "09:30", ledger: null, accounts: [] }));

    render(<CheckinPage productId={WORKBUDDY_PRODUCT_ID} active={true} onNavigate={noop} />);

    expect(await screen.findByTestId("product-badge-workbuddy")).toBeInTheDocument();
    expect(screen.getByTestId("workbuddy-auto-checkin-meta")).toHaveTextContent("每日 09:30 触发");
    expect(document.querySelectorAll(".checkin-flow__row")).toHaveLength(1);
    // 不渲染 TRAE 专属状态行（演示模式 / 存储未就绪）。
    expect(screen.queryByTestId("checkin-demo-banner")).not.toBeInTheDocument();
  });

  it("空态：无条目时渲染一句结论 + 去账号页动作", async () => {
    const onNavigate = vi.fn();
    mockRoute("get_checkin_capability", () => ({
      enabled: true,
      transport: "real",
      real_http_enabled: true,
      message: "",
    }));
    mockRoute("get_checkin_overview", () => []);
    mockRoute("get_auto_checkin_settings", () => ({ enabled: false, daily_time_hhmm: "10:00", ledger: null }));

    render(<CheckinPage productId={CURRENT_PRODUCT_ID} active={true} onNavigate={onNavigate} />);

    expect(await screen.findByTestId("checkin-empty")).toBeInTheDocument();
    expect(screen.getByText("还没有可签到的账号。")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "去账号页添加" }));
    expect(onNavigate).toHaveBeenCalledWith("accounts");
    await waitFor(() => expect(document.querySelector("ul.checkin-flow")).toBeNull());
  });
});
