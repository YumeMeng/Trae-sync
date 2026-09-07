import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { CheckinPage } from "../src/components/CheckinPage";
import type {
  CheckinBatchSummaryDto,
  CheckinOverviewEntryDto,
  CheckinResultDto,
  ManagedAccountsViewDto,
} from "../src/types/account_switch";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

// 事件插件 mock：捕获 listen 回调，供横幅测试手动触发。
const { mockListen } = vi.hoisted(() => ({ mockListen: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({ listen: mockListen }));

const mockInvoke = vi.mocked(invoke);

const realCapability = {
  enabled: true,
  transport: "real",
  real_http_enabled: true,
  message: "真实签到已启用：仅对已通过登录的账号直连 TRAE；未登录账号会提示先登录。",
};

const fixtureCapability = {
  enabled: true,
  transport: "fixture",
  real_http_enabled: false,
  message: "演示模式：使用本地已保存账号模拟签到。",
};

function entry(profileId: string, screenName: string, overrides: Partial<CheckinOverviewEntryDto> = {}): CheckinOverviewEntryDto {
  const now = Date.now() / 1000;
  return {
    profile_id: profileId,
    screen_name: screenName,
    account_id: `account-${profileId}`,
    created_at: "2026-08-01T00:00:00.000Z",
    last_verified_at: "2026-08-22T00:00:00.000Z",
    credits: 1240,
    credits_cached_at: "2026-08-22T01:00:00.000Z",
    usage_remaining_credits: 1240,
    usage_cached_at: "2026-08-22T01:00:00.000Z",
    checked_in: true,
    access_token_expires_at_unix_seconds: Math.floor(now + 10 * 86400),
    refresh_token_expires_at_unix_seconds: Math.floor(now + 170 * 86400),
    device_tail: "8502",
    device_id: null,
    display_name: null,
    masked_mobile: "138****0000",
    mobile_full: null,
    auto_checkin_enabled: true,
    refresh_error_code: null,
    credential_legacy: false,
    last_attempt_outcome: null,
    last_attempt_date: null,
    ...overrides,
  };
}

function result(profileId: string, overrides: Partial<CheckinResultDto> = {}): CheckinResultDto {
  return {
    profile_id: profileId,
    outcome: "claimed",
    state: "claimed",
    claim_attempted: true,
    before: { enabled: true, checked_in: false, credits: 1240, business_code: null },
    after: { enabled: true, checked_in: true, credits: 1245, business_code: null },
    detail_code: null,
    started_at: "2026-08-22T02:00:00.000Z",
    finished_at: "2026-08-22T02:00:12.000Z",
    ...overrides,
  };
}

function summary(results: readonly CheckinResultDto[], failed = 0): CheckinBatchSummaryDto {
  return {
    total: results.length,
    completed: results.length - failed,
    failed,
    cancelled: 0,
    results,
  };
}

function fixtureView(): ManagedAccountsViewDto {
  return {
    saved_accounts: [
      {
        profile_id: "profile-fixture-1",
        display_name: "演示账号甲",
        region: "cn",
        data_location_id: "location-1",
        last_verified_at: "2026-08-22T00:00:00.000Z",
        verification_state: "verified",
      },
    ],
    // 契约：current_account/recent_verification 为必填对象（Rust 侧非 Option），
    // fixture 模式用“无当前账号”的证据形态（全部字段为空 + unknown 状态）。
    current_account: {
      profile_id: null,
      display_name: null,
      region: null,
      data_location_id: null,
      verification_state: "unknown",
      observed_at: null,
      reason: null,
    },
    recent_verification: {
      verification_state: "unknown",
      checked_at: null,
      reason: null,
    },
    switch_state: null,
    handoff_intent: null,
    history_is_separate: true,
  };
}

describe("CheckinPage", () => {
  beforeEach(() => {
    mockInvoke.mockReset();
    // 默认返回可用的 unlisten 句柄；需触发事件的测试自行覆写实现。
    mockListen.mockReset();
    mockListen.mockResolvedValue(() => undefined);
  });

  afterEach(() => vi.clearAllMocks());

  it("状态行展示自动签到触发时间与今日台账进度", async () => {
    mockInvoke.mockImplementation(async (command) => {
      if (command === "get_checkin_capability") return realCapability;
      if (command === "get_checkin_overview") return [entry("profile-login-1", "登录账号甲")];
      if (command === "get_auto_checkin_settings") {
        return {
          enabled: true,
          daily_time_hhmm: "10:00",
          ledger: { date: "2026-08-23", running: true, total: 3, completed: 1, failed: 0, skipped: 0 },
        };
      }
      throw new Error(`unexpected command: ${String(command)}`);
    });

    render(<CheckinPage active={true} onNavigate={vi.fn()} />);

    const status = await screen.findByTestId("auto-checkin-status");
    expect(status).toHaveTextContent("每日 10:00 触发");
    expect(status).toHaveTextContent("今日执行中（成功 1/3）");
  });

  it("自动签到关闭时状态行只显示关闭，不带台账", async () => {
    mockInvoke.mockImplementation(async (command) => {
      if (command === "get_checkin_capability") return realCapability;
      if (command === "get_checkin_overview") return [entry("profile-login-1", "登录账号甲")];
      if (command === "get_auto_checkin_settings") {
        return { enabled: false, daily_time_hhmm: "10:00", ledger: null };
      }
      throw new Error(`unexpected command: ${String(command)}`);
    });

    render(<CheckinPage active={true} onNavigate={vi.fn()} />);

    const status = await screen.findByTestId("auto-checkin-status");
    expect(status).toHaveTextContent("自动签到：已关闭");
    expect(status).not.toHaveTextContent("今日");
  });

  it("auto-checkin-finished 事件显示完成横幅并翻完成态", async () => {
    // 横幅由事件驱动：捕获 listen 回调后手动触发（mock 边界不产生真实事件）。
    const handlers = new Map<string, (event: { payload: unknown }) => void>();
    mockListen.mockImplementation(async (event: string, cb: (event: { payload: unknown }) => void) => {
      handlers.set(event, cb);
      return () => undefined;
    });
    mockInvoke.mockImplementation(async (command) => {
      if (command === "get_checkin_capability") return realCapability;
      if (command === "get_checkin_overview") return [entry("profile-login-1", "登录账号甲")];
      if (command === "get_auto_checkin_settings") {
        return {
          enabled: true,
          daily_time_hhmm: "10:00",
          ledger: { date: "2026-08-23", running: true, total: 3, completed: 1, failed: 0, skipped: 0 },
        };
      }
      throw new Error(`unexpected command: ${String(command)}`);
    });

    render(<CheckinPage active={true} onNavigate={vi.fn()} />);

    const status = await screen.findByTestId("auto-checkin-status");
    expect(status).toHaveTextContent("今日执行中（成功 1/3）");

    await act(async () => {
      handlers.get("auto-checkin-finished")?.({ payload: { total: 3, completed: 2, failed: 1, skipped: 0 } });
    });

    // 完成横幅出现，且台账从执行中翻到已完成（running 消失）。
    const banner = screen.getByTestId("auto-checkin-banner");
    expect(banner).toHaveTextContent("今日自动签到完成：成功 2/3，失败 1");
    expect(status).toHaveTextContent("今日已完成（成功 2/3，失败 1）");
    expect(status).not.toHaveTextContent("今日执行中");
  });

  it("真实模式加载总览并展示账号行：今日状态与缓存额度", async () => {
    mockInvoke.mockImplementation(async (command) => {
      if (command === "get_checkin_capability") return realCapability;
      if (command === "get_checkin_overview") {
        return [
          entry("profile-login-1", "登录账号甲"),
          entry("profile-login-2", "登录账号乙", { checked_in: false, credits: null, credits_cached_at: null, usage_remaining_credits: null, usage_cached_at: null }),
        ];
      }
      throw new Error(`unexpected command: ${String(command)}`);
    });

    render(<CheckinPage active={true} onNavigate={vi.fn()} />);

    expect(await screen.findByText("登录账号甲")).toBeInTheDocument();
    // U-3 两槽位徽章语言：已签/未签（签到槽位）。
    expect(screen.getByText("已签")).toBeInTheDocument();
    expect(screen.getByText("未签")).toBeInTheDocument();
    expect(screen.getByText(/额度 1240（\d{2}\/\d{2} \d{2}:\d{2} 缓存）/)).toBeInTheDocument();
    // 行内元信息是拼接串（两个账号各含最近验证）。
    expect(screen.getAllByText(/最近验证/)).toHaveLength(2);
    // 四动作直达：全签计数与池一致；补签计数为未签数（乙未签 -> 1）。
    expect(screen.getByRole("button", { name: /一键全签（2）/ })).toBeEnabled();
    expect(screen.getByRole("button", { name: /一键补签（1）/ })).toBeEnabled();
  });

  it("可切换账号选择并只对选中账号执行签到（签到所选）", async () => {
    const runMock = vi.fn().mockResolvedValue(summary([result("profile-login-1")]));
    mockInvoke.mockImplementation(async (command) => {
      if (command === "get_checkin_capability") return realCapability;
      if (command === "get_checkin_overview") {
        return [entry("profile-login-1", "登录账号甲"), entry("profile-login-2", "登录账号乙")];
      }
      if (command === "run_checkin") return runMock();
      throw new Error(`unexpected command: ${String(command)}`);
    });

    render(<CheckinPage active={true} onNavigate={vi.fn()} />);
    await screen.findByText("登录账号甲");

    // 取消勾选乙：出现「签到所选（1）」按钮（全选时不显示，避免按钮堆叠）。
    fireEvent.click(screen.getByRole("checkbox", { name: /登录账号乙/ }));
    expect(screen.getByRole("button", { name: /签到所选（1）/ })).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: /签到所选/ }));
    await waitFor(() => expect(mockInvoke).toHaveBeenCalledWith("run_checkin", { profileIds: ["profile-login-1"] }));
    expect(runMock).toHaveBeenCalledTimes(1);
  });

  it("一键补签只对今日未签账号发起请求", async () => {
    mockInvoke.mockImplementation(async (command) => {
      if (command === "get_checkin_capability") return realCapability;
      if (command === "get_checkin_overview") {
        // 甲已签、乙未签：补签应只发乙。
        return [
          entry("profile-login-1", "登录账号甲"),
          entry("profile-login-2", "登录账号乙", { checked_in: false }),
        ];
      }
      if (command === "run_checkin") {
        return summary([result("profile-login-2")]);
      }
      throw new Error(`unexpected command: ${String(command)}`);
    });

    render(<CheckinPage active={true} onNavigate={vi.fn()} />);
    await screen.findByText("登录账号甲");

    fireEvent.click(screen.getByRole("button", { name: /一键补签（1）/ }));
    await waitFor(() =>
      expect(mockInvoke).toHaveBeenCalledWith("run_checkin", { profileIds: ["profile-login-2"] })
    );
  });

  it("行内单账号签到只发送该账号（已签账号行内按钮禁用）", async () => {
    mockInvoke.mockImplementation(async (command) => {
      if (command === "get_checkin_capability") return realCapability;
      if (command === "get_checkin_overview") {
        return [
          entry("profile-login-1", "登录账号甲"),
          entry("profile-login-2", "登录账号乙", { checked_in: false }),
        ];
      }
      if (command === "run_checkin") return summary([result("profile-login-2")]);
      throw new Error(`unexpected command: ${String(command)}`);
    });

    render(<CheckinPage active={true} onNavigate={vi.fn()} />);
    await screen.findByText("登录账号甲");

    // 已签账号的行内按钮禁用；未签账号可点。
    expect(screen.getByTestId("checkin-inline-profile-login-1")).toBeDisabled();
    expect(screen.getByTestId("checkin-inline-profile-login-2")).toBeEnabled();

    fireEvent.click(screen.getByTestId("checkin-inline-profile-login-2"));
    await waitFor(() =>
      expect(mockInvoke).toHaveBeenCalledWith("run_checkin", { profileIds: ["profile-login-2"] })
    );
  });

  it("新设备首签过快（device_too_new）时给出等待引导文案", async () => {
    // v6：签到链路不再自动重铸/冷却；设备铸造后 5 分钟内被 9074 拒绝时，
    // 后端将 detail_code 标注为 device_too_new，前端引导"稍后重试"。
    mockInvoke.mockImplementation(async (command) => {
      if (command === "get_checkin_capability") return realCapability;
      if (command === "get_checkin_overview") {
        return [entry("profile-login-1", "登录账号甲", { checked_in: false })];
      }
      if (command === "run_checkin") {
        return summary(
          [
            result("profile-login-1", {
              outcome: "not_eligible",
              state: "blocked",
              claim_attempted: false,
              detail_code: "device_too_new",
            }),
          ],
          1,
        );
      }
      throw new Error(`unexpected command: ${String(command)}`);
    });

    render(<CheckinPage active={true} onNavigate={vi.fn()} />);
    await screen.findByText("登录账号甲");
    fireEvent.click(screen.getByRole("button", { name: /一键全签/ }));

    expect(await screen.findByText("部分签到未完成，请查看逐账号结果。")).toBeInTheDocument();
    expect(screen.getByText("设备刚创建，请等待约 5 分钟后重试")).toBeInTheDocument();
  });

  it("账号间错峰等待期下一账号行内显示倒计时（inter_wait 事件驱动），批次结束清除", async () => {
    // 两个账号批量执行：第一个完成后进入 3~8 秒错峰等待，后端推送
    // checkin-phase(inter_wait)（载荷为下一个账号），该行显示"N 秒后开始"。
    const handlers = new Map<string, (event: { payload: unknown }) => void>();
    mockListen.mockImplementation(async (event: string, cb: (event: { payload: unknown }) => void) => {
      handlers.set(event, cb);
      return () => undefined;
    });
    let resolveRun!: (value: CheckinBatchSummaryDto) => void;
    const runDeferred = new Promise<CheckinBatchSummaryDto>((resolve) => { resolveRun = resolve; });
    mockInvoke.mockImplementation(async (command) => {
      if (command === "get_checkin_capability") return realCapability;
      if (command === "get_checkin_overview") {
        return [
          entry("profile-login-1", "登录账号甲", { checked_in: false }),
          entry("profile-login-2", "登录账号乙", { checked_in: false }),
        ];
      }
      if (command === "run_checkin") return runDeferred;
      throw new Error(`unexpected command: ${String(command)}`);
    });

    render(<CheckinPage active={true} onNavigate={vi.fn()} />);
    await screen.findByText("登录账号甲");
    fireEvent.click(screen.getByRole("button", { name: /一键全签/ }));

    // 第一个账号完成（completed=1）：cursor 推进到第二个账号。
    await act(async () => {
      handlers.get("checkin-progress")?.({
        payload: { profile_id: "profile-login-1", screen_name: "登录账号甲", outcome: "claimed", detail_code: null, completed: 1, total: 2 },
      });
    });
    // 错峰等待开始：下一账号行内显示"N 秒后开始"。
    await act(async () => {
      handlers.get("checkin-phase")?.({
        payload: { profile_id: "profile-login-2", screen_name: "登录账号乙", phase: "inter_wait", remaining_secs: 5 },
      });
    });
    const wait = await screen.findByTestId("checkin-inter-wait");
    expect(wait).toHaveTextContent("5 秒后开始");

    // 批次结束：等待态清除。
    await act(async () => {
      resolveRun(summary([result("profile-login-1"), result("profile-login-2")]));
    });
    expect(await screen.findByText("签到流程已完成。")).toBeInTheDocument();
    expect(screen.queryByTestId("checkin-inter-wait")).not.toBeInTheDocument();
  });

  it("签到完成后展示逐账号结果、积分变化与完成消息，并刷新总览", async () => {
    let overviewReads = 0;
    mockInvoke.mockImplementation(async (command) => {
      if (command === "get_checkin_capability") return realCapability;
      if (command === "get_checkin_overview") {
        overviewReads += 1;
        return [
          entry("profile-login-1", "登录账号甲", { checked_in: false }),
          entry("profile-login-2", "登录账号乙", { checked_in: false }),
        ];
      }
      if (command === "run_checkin") {
        return summary([
          result("profile-login-1", { outcome: "already_checked_in", state: "already_checked_in", claim_attempted: false }),
          result("profile-login-2", { outcome: "claimed" }),
        ]);
      }
      throw new Error(`unexpected command: ${String(command)}`);
    });

    render(<CheckinPage active={true} onNavigate={vi.fn()} />);
    await screen.findByText("登录账号甲");
    fireEvent.click(screen.getByRole("button", { name: /一键全签/ }));

    expect(await screen.findByText("签到流程已完成。")).toBeInTheDocument();
    // U-3 单列表演进式：结果直接落在行内（无独立结果列表）。
    // 已签到账号显示「今日已签到」；新领取账号展示成功。
    const flowList = screen.getByRole("list", { name: "签到账号与结果" });
    expect(within(flowList).getByText(/^今日已签到$/)).toBeInTheDocument();
    expect(within(flowList).getByText(/^签到成功$/)).toBeInTheDocument();
    // 后端写回积分缓存后，前端必须再读一次总览同步「今日已签」。
    await waitFor(() => expect(overviewReads).toBe(2));
  });

  it("部分账号失败时给出未完成消息并透出业务码", async () => {
    mockInvoke.mockImplementation(async (command) => {
      if (command === "get_checkin_capability") return realCapability;
      if (command === "get_checkin_overview") {
        return [entry("profile-login-1", "登录账号甲"), entry("profile-login-2", "登录账号乙")];
      }
      if (command === "run_checkin") {
        return summary(
          [
            result("profile-login-1"),
            result("profile-login-2", {
              outcome: "not_eligible",
              state: "blocked",
              claim_attempted: false,
              detail_code: "business_9074",
            }),
          ],
          1,
        );
      }
      throw new Error(`unexpected command: ${String(command)}`);
    });

    render(<CheckinPage active={true} onNavigate={vi.fn()} />);
    await screen.findByText("登录账号甲");
    fireEvent.click(screen.getByRole("button", { name: /一键全签/ }));

    expect(await screen.findByText("部分签到未完成，请查看逐账号结果。")).toBeInTheDocument();
    // ADR-0019：业务码原样透出，不压平为通用失败（U-3 起结果只在行内出现一次）。
    expect(screen.getAllByText(/设备信息未获服务端信任（9074）/)).toHaveLength(1);
  });

  it("真实模式下总览读取失败必须可见，不静默伪装成空账号", async () => {
    mockInvoke.mockImplementation(async (command) => {
      if (command === "get_checkin_capability") return realCapability;
      if (command === "get_checkin_overview") throw new Error("checkin_storage_unavailable");
      throw new Error(`unexpected command: ${String(command)}`);
    });

    render(<CheckinPage active={true} onNavigate={vi.fn()} />);

    expect(await screen.findByRole("alert")).toHaveTextContent("本机登录存储不可用，请检查磁盘后重试。");
  });

  it("没有可签到账号时显示空状态引导并可跳转账号页", async () => {
    const onNavigate = vi.fn();
    mockInvoke.mockImplementation(async (command) => {
      if (command === "get_checkin_capability") return realCapability;
      if (command === "get_checkin_overview") return [];
      throw new Error(`unexpected command: ${String(command)}`);
    });

    render(<CheckinPage active={true} onNavigate={onNavigate} />);

    expect(await screen.findByText(/还没有可签到的账号/)).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "去账号页添加" }));
    expect(onNavigate).toHaveBeenCalledWith("accounts");
  });

  it("fixture 模式回退到已保存账号池，不读取账号总览", async () => {
    mockInvoke.mockImplementation(async (command) => {
      if (command === "get_checkin_capability") return fixtureCapability;
      if (command === "get_managed_account_state") return fixtureView();
      throw new Error(`unexpected command: ${String(command)}`);
    });

    render(<CheckinPage active={true} onNavigate={vi.fn()} />);

    expect(await screen.findByText("演示账号甲")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /全部签到（1）/ })).toBeEnabled();
    expect(mockInvoke).not.toHaveBeenCalledWith("get_checkin_overview");
    // fixture 模式收敛为一行「演示模式」横幅，不再展示 transport 细节文案。
    expect(screen.getByTestId("checkin-demo-banner")).toHaveTextContent("演示模式");
  });

  it("签到能力未启用时禁用执行按钮", async () => {
    mockInvoke.mockImplementation(async (command) => {
      if (command === "get_checkin_capability") {
        // 池来自 fixture 账号，能力开关关闭：按钮可见但禁用。
        return { enabled: false, transport: "fixture", real_http_enabled: false, message: "签到能力当前未启用。" };
      }
      if (command === "get_managed_account_state") return fixtureView();
      throw new Error(`unexpected command: ${String(command)}`);
    });

    render(<CheckinPage active={true} onNavigate={vi.fn()} />);

    expect(await screen.findByText("演示账号甲")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /全部签到/ })).toBeDisabled();
    expect(mockInvoke).not.toHaveBeenCalledWith("run_checkin", expect.anything());
  });

  it("存储未就绪时显示异常提示，不显示演示模式横幅", async () => {
    mockInvoke.mockImplementation(async (command) => {
      if (command === "get_checkin_capability") {
        // 后端真实形态：RealReadPreview 且存储根为空 → disabled，池请求不发出。
        return { enabled: false, transport: "disabled", real_http_enabled: false, message: "存储根不可用，真实签到未启用。" };
      }
      throw new Error(`unexpected command: ${String(command)}`);
    });

    render(<CheckinPage active={true} onNavigate={vi.fn()} />);

    expect(await screen.findByTestId("checkin-unavailable")).toHaveTextContent("签到功能不可用：存储未就绪");
    expect(screen.queryByTestId("checkin-demo-banner")).not.toBeInTheDocument();
  });

  it("G15 纯本地刷新：重读账号总览，不发签到或额度查询网络命令", async () => {
    mockInvoke.mockImplementation(async (command) => {
      if (command === "get_checkin_capability") return realCapability;
      if (command === "get_checkin_overview") return [entry("profile-login-1", "登录账号甲")];
      if (command === "get_auto_checkin_settings") {
        return { enabled: false, daily_time_hhmm: "10:00", ledger: null };
      }
      throw new Error(`unexpected command: ${String(command)}`);
    });

    render(<CheckinPage active={true} onNavigate={vi.fn()} />);
    await screen.findByText("登录账号甲");
    const callsOf = (command: string) => mockInvoke.mock.calls.filter(([name]) => name === command).length;
    const overviewBefore = callsOf("get_checkin_overview");

    fireEvent.click(screen.getByTestId("checkin-refresh-local"));
    // 总览（本地缓存读取）被重读。
    await waitFor(() => expect(callsOf("get_checkin_overview")).toBeGreaterThan(overviewBefore));
    // 等待一个宏任务回合，确认没有异步尾巴发出网络命令。
    await act(async () => { await new Promise((resolve) => setTimeout(resolve, 20)); });
    expect(mockInvoke).not.toHaveBeenCalledWith("run_checkin", expect.anything());
    expect(mockInvoke).not.toHaveBeenCalledWith("refresh_checkin_credits", expect.anything());
  });
});
