import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { AccountCenter } from "../src/components/AccountCenter";
import type {
  AccountProfileDto,
  ManagedAccountsViewDto,
  CheckinOverviewEntryDto,
  CheckinResultDto,
} from "../src/types/account_switch";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

const mockInvoke = vi.mocked(invoke);
const checkedAt = "2025-07-01T00:00:00.000Z";

const currentProfile: AccountProfileDto = {
  profile_id: "profile-current",
  display_name: "工作账号 A",
  region: "cn",
  data_location_id: "location-current",
  last_verified_at: checkedAt,
  verification_state: "verified",
};

const targetProfile: AccountProfileDto = {
  profile_id: "profile-target",
  display_name: "工作账号 B",
  region: "cn",
  data_location_id: "location-target",
  last_verified_at: checkedAt,
  verification_state: "verified",
};

function state(overrides: Partial<ManagedAccountsViewDto> = {}): ManagedAccountsViewDto {
  return {
    saved_accounts: [currentProfile, targetProfile],
    current_account: {
      profile_id: currentProfile.profile_id,
      display_name: currentProfile.display_name,
      region: "cn",
      data_location_id: currentProfile.data_location_id,
      verification_state: "verified",
      observed_at: checkedAt,
      reason: null,
    },
    recent_verification: {
      verification_state: "verified",
      checked_at: checkedAt,
      reason: null,
    },
    switch_state: null,
    handoff_intent: null,
    history_is_separate: true,
    ...overrides,
  };
}

// 真实签到能力（真实 transport）：驱动账号总览加载与「添加账号」入口。
const realCapability = {
  enabled: true,
  transport: "real",
  real_http_enabled: true,
  message: "真实签到已启用：仅对已通过登录的账号直连 TRAE；未登录账号会提示先登录。",
};

// fixture 模式能力：总览不加载，页面回退到已保存账号行式列表。
const fixtureCapability = {
  enabled: false,
  transport: "fixture",
  real_http_enabled: false,
  message: "fixture",
};

interface EntryOverrides extends Partial<CheckinOverviewEntryDto> {}

function overviewEntry(profileId: string, screenName: string, overrides: EntryOverrides = {}): CheckinOverviewEntryDto {
  const now = Date.now() / 1000;
  return {
    profile_id: profileId,
    screen_name: screenName,
    account_id: `account-${profileId}`,
    created_at: "2026-08-01T00:00:00.000Z",
    last_verified_at: "2026-08-22T00:00:00.000Z",
    credits: 1240,
    credits_cached_at: "2026-08-22T01:00:00.000Z",
    usage_remaining_credits: null,
    usage_cached_at: null,
    checked_in: true,
    access_token_expires_at_unix_seconds: Math.floor(now + 10 * 86400),
    refresh_token_expires_at_unix_seconds: Math.floor(now + 170 * 86400),
    device_tail: "8502",
    device_id: "249085123408502",
    display_name: null,
    masked_mobile: "138****0000",
    auto_checkin_enabled: true,
    refresh_error_code: null,
    credential_legacy: false,
    ...overrides,
  };
}

function checkinResult(profileId: string, outcome: string, overrides: Partial<CheckinResultDto> = {}): CheckinResultDto {
  return {
    profile_id: profileId,
    outcome: outcome as CheckinResultDto["outcome"],
    state: "done",
    claim_attempted: true,
    before: { enabled: true, checked_in: false, credits: 1240, business_code: null },
    after: { enabled: true, checked_in: true, credits: 1250, business_code: null },
    detail_code: null,
    started_at: "2026-08-23T00:00:00.000Z",
    finished_at: "2026-08-23T00:00:05.000Z",
    ...overrides,
  };
}

/** 真实模式默认 mock：一个已登录账号的完整总览。 */
function mockRealMode(entries: CheckinOverviewEntryDto[], stateView = state()) {
  mockInvoke.mockImplementation(async (command) => {
    if (command === "get_managed_account_state") return stateView;
    if (command === "get_checkin_capability") return realCapability;
    if (command === "get_checkin_overview") return entries;
    // 登录凭据健康度默认全部未初始化（徽章显示「未登录」）。
    if (command === "get_trae_instance_states") {
      return entries.map((entry) => ({
        profile_id: entry.profile_id,
        login_state: "uninitialized",
        archive_available: false,
      }));
    }
    throw new Error(`unexpected command: ${String(command)}`);
  });
}

describe("AccountCenter", () => {
  beforeEach(() => {
    // U-2 视图/排序偏好存 localStorage：测试间必须清零，防止偏好泄漏串扰断言。
    window.localStorage.clear();
    mockInvoke.mockReset();
    mockInvoke.mockImplementation(async (command) => {
      if (command === "get_managed_account_state") return state();
      throw new Error(`unexpected command: ${String(command)}`);
    });
  });

  afterEach(() => vi.clearAllMocks());

  it("fixture 模式回退到已保存账号列表，不加载总览也不出现添加入口", async () => {
    mockInvoke.mockImplementation(async (command) => {
      if (command === "get_managed_account_state") return state();
      if (command === "get_checkin_capability") return fixtureCapability;
      throw new Error(`unexpected command: ${String(command)}`);
    });

    render(<AccountCenter active={true} />);
    expect(await screen.findByText("工作账号 A")).toBeInTheDocument();
    expect(screen.getByText("工作账号 B")).toBeInTheDocument();
    expect(mockInvoke).not.toHaveBeenCalledWith("get_checkin_overview");
    expect(screen.queryByTestId("account-add-primary")).not.toBeInTheDocument();
  });

  it("真实模式条目展示要素：名称、模型额度、签到槽位徽章、令牌 meta（U-2 新徽章语言）", async () => {
    mockRealMode([overviewEntry("profile-login-1", "登录账号甲", {
      usage_remaining_credits: 1240,
      usage_cached_at: "2026-08-22T01:00:00.000Z",
    })]);

    render(<AccountCenter active={true} />);
    const card = await screen.findByTestId("account-card-profile-login-1");
    // 可断言文本：名称、模型积分额度、签到槽（已签）、令牌 meta 文字。
    expect(card).toHaveTextContent("登录账号甲");
    expect(card).toHaveTextContent("模型积分");
    expect(card).toHaveTextContent("1240");
    expect(screen.getByText("已签")).toBeInTheDocument();
    expect(screen.getByText("令牌 10 天")).toBeInTheDocument();
  });

  it("积分未查询时条目显示占位文案，未签显示空心槽位徽章", async () => {
    mockRealMode([
      overviewEntry("profile-login-1", "登录账号甲", {
        credits: null,
        credits_cached_at: null,
        checked_in: false,
      }),
    ]);

    render(<AccountCenter active={true} />);
    expect(await screen.findByText("未查询")).toBeInTheDocument();
    expect(screen.getByText("未签")).toBeInTheDocument();
    expect(screen.getByTestId("account-card-profile-login-1")).not.toHaveTextContent(/积分 \d/);
  });

  it("token 健康度文字化：过期为琥珀、临近 7 天内为琥珀（异常才亮色）", async () => {
    const now = Date.now() / 1000;
    mockRealMode([
      overviewEntry("profile-a", "过期账号", { access_token_expires_at_unix_seconds: Math.floor(now - 100) }),
      overviewEntry("profile-b", "临近账号", { access_token_expires_at_unix_seconds: Math.floor(now + 2 * 86400) }),
    ]);

    render(<AccountCenter active={true} />);
    expect(await screen.findByText("登录已过期")).toBeInTheDocument();
    expect(screen.getByText("令牌 2 天")).toBeInTheDocument();
    // 临近账号的令牌文字转琥珀（meta-warn 类）。
    const warnMeta = screen.getByText("令牌 2 天");
    expect(warnMeta).toHaveClass("account-item__meta-warn");
  });

  it("点击卡片进入详情视图，展示基础信息与折叠技术细节，返回回到列表", async () => {
    mockRealMode([overviewEntry("profile-login-1", "登录账号甲", {
      usage_remaining_credits: 1240,
      usage_cached_at: "2026-08-22T01:00:00.000Z",
    })]);

    render(<AccountCenter active={true} />);
    fireEvent.click(await screen.findByTestId("account-card-profile-login-1"));

    // 详情区基础信息：模型额度、今日签到、设备尾号。
    expect(await screen.findByTestId("account-detail-usage")).toHaveTextContent(/1240/);
    expect(screen.getByTestId("account-detail-checked-in")).toHaveTextContent("已签到");
    expect(screen.getByText(/…8502/)).toBeInTheDocument();
    // 技术细节默认折叠（jsdom 仍渲染折叠内容，断言 details 未展开）。
    const tech = screen.getByText("技术细节").closest("details");
    expect(tech).not.toBeNull();
    expect(tech).not.toHaveAttribute("open");

    fireEvent.click(screen.getByTestId("account-detail-back"));
    expect(await screen.findByTestId("account-card-profile-login-1")).toBeInTheDocument();
  });

  it("详情页刷新额度调用单账号只读命令并回写提示", async () => {
    const entries = [overviewEntry("profile-login-1", "登录账号甲", { usage_remaining_credits: null, usage_cached_at: null })];
    let refreshed = false;
    mockInvoke.mockImplementation(async (command) => {
      if (command === "get_managed_account_state") return state();
      if (command === "get_checkin_capability") return realCapability;
      if (command === "get_checkin_overview") {
        return refreshed
          ? [overviewEntry("profile-login-1", "登录账号甲")]
          : entries;
      }
      if (command === "refresh_checkin_credits") {
        refreshed = true;
        return [{
          profile_id: "profile-login-1",
          screen_name: "登录账号甲",
          credits: 1250,
          checked_in: true,
          usage_remaining_credits: 236.5,
          error_code: null,
        }];
      }
      throw new Error(`unexpected command: ${String(command)}`);
    });

    render(<AccountCenter active={true} />);
    fireEvent.click(await screen.findByTestId("account-card-profile-login-1"));
    fireEvent.click(await screen.findByRole("button", { name: /刷新额度/ }));

    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith("refresh_checkin_credits", { profileIds: ["profile-login-1"] });
    });
    expect(await screen.findByText(/额度已更新：模型积分 236\.5（今日已签）/)).toBeInTheDocument();
  });

  it("详情页立即签到执行单账号批次，成功后显示奖励已发放", async () => {
    const entries = [overviewEntry("profile-login-1", "登录账号甲", { checked_in: false })];
    let checkedIn = false;
    mockInvoke.mockImplementation(async (command) => {
      if (command === "get_managed_account_state") return state();
      if (command === "get_checkin_capability") return realCapability;
      if (command === "get_checkin_overview") {
        return checkedIn
          ? [overviewEntry("profile-login-1", "登录账号甲")]
          : entries;
      }
      if (command === "run_checkin") {
        checkedIn = true;
        return { total: 1, completed: 1, failed: 0, cancelled: 0, results: [checkinResult("profile-login-1", "claimed")] };
      }
      throw new Error(`unexpected command: ${String(command)}`);
    });

    render(<AccountCenter active={true} />);
    fireEvent.click(await screen.findByTestId("account-card-profile-login-1"));
    fireEvent.click(await screen.findByTestId("account-detail-checkin"));

    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith("run_checkin", { profileIds: ["profile-login-1"] });
    });
    expect(await screen.findByText(/签到成功，奖励已发放/)).toBeInTheDocument();
  });

  it("缓存确认今日已签时详情页签到按钮禁用，不重复发起", async () => {
    mockRealMode([overviewEntry("profile-login-1", "登录账号甲", { checked_in: true })]);

    render(<AccountCenter active={true} />);
    fireEvent.click(await screen.findByTestId("account-card-profile-login-1"));

    const button = await screen.findByTestId("account-detail-checkin");
    expect(button).toBeDisabled();
    expect(button).toHaveTextContent("今日已签到");
    expect(mockInvoke).not.toHaveBeenCalledWith("run_checkin", expect.anything());
  });

  it("详情页自动签到开关：退出参与后调用单账号命令并更新提示", async () => {
    // 初始参与自动签到；切换后总览回填关闭状态。
    let autoEnabled = true;
    mockInvoke.mockImplementation(async (command) => {
      if (command === "get_managed_account_state") return state();
      if (command === "get_checkin_capability") return realCapability;
      if (command === "get_checkin_overview") {
        return [overviewEntry("profile-login-1", "登录账号甲", { auto_checkin_enabled: autoEnabled })];
      }
      if (command === "set_account_auto_checkin") {
        autoEnabled = false;
        return undefined;
      }
      throw new Error(`unexpected command: ${String(command)}`);
    });

    render(<AccountCenter active={true} />);
    fireEvent.click(await screen.findByTestId("account-card-profile-login-1"));

    const toggle = await screen.findByTestId("auto-checkin-account-toggle");
    expect(toggle).toHaveTextContent("参与每日自动签到");
    fireEvent.click(toggle.querySelector("input")!);

    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith("set_account_auto_checkin", {
        profileId: "profile-login-1",
        enabled: false,
      });
    });
    expect(await screen.findByText("已退出每日自动签到。")).toBeInTheDocument();
    expect(await screen.findByText("不参与每日自动签到")).toBeInTheDocument();
  });

  it("详情页删除账号需二次确认，确认后删除并回到列表", async () => {
    let removed = false;
    mockInvoke.mockImplementation(async (command) => {
      if (command === "get_managed_account_state") return state();
      if (command === "get_checkin_capability") return realCapability;
      if (command === "get_checkin_overview") return removed ? [] : [overviewEntry("profile-login-1", "登录账号甲")];
      if (command === "remove_checkin_account") {
        removed = true;
        return undefined;
      }
      throw new Error(`unexpected command: ${String(command)}`);
    });
    const confirmSpy = vi.spyOn(window, "confirm").mockReturnValue(true);

    render(<AccountCenter active={true} />);
    fireEvent.click(await screen.findByTestId("account-card-profile-login-1"));
    fireEvent.click(await screen.findByRole("button", { name: /删除账号/ }));

    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith("remove_checkin_account", { profileId: "profile-login-1" });
    });
    // 删除后回到列表，空状态出现。
    expect(await screen.findByText(/还没有账号/)).toBeInTheDocument();
    confirmSpy.mockRestore();
  });

  it("删除账号取消确认时不发起删除命令", async () => {
    mockRealMode([overviewEntry("profile-login-1", "登录账号甲")]);
    const confirmSpy = vi.spyOn(window, "confirm").mockReturnValue(false);

    render(<AccountCenter active={true} />);
    fireEvent.click(await screen.findByTestId("account-card-profile-login-1"));
    fireEvent.click(await screen.findByRole("button", { name: /删除账号/ }));

    expect(mockInvoke).not.toHaveBeenCalledWith("remove_checkin_account", expect.anything());
    expect(await screen.findByTestId("account-detail-usage")).toBeInTheDocument();
    confirmSpy.mockRestore();
  });

  it("详情页重铸签到设备：确认后调用命令并提示下次使用新设备", async () => {
    const entries = [overviewEntry("profile-login-1", "登录账号甲")];
    mockInvoke.mockImplementation(async (command) => {
      if (command === "get_managed_account_state") return state();
      if (command === "get_checkin_capability") return realCapability;
      if (command === "get_checkin_overview") return entries;
      if (command === "get_trae_instance_states") {
        return entries.map((entry) => ({
          profile_id: entry.profile_id,
          login_state: "uninitialized",
          archive_available: false,
        }));
      }
      if (command === "reset_checkin_device") return "1234567890123456";
      throw new Error(`unexpected command: ${String(command)}`);
    });
    const confirmSpy = vi.spyOn(window, "confirm").mockReturnValue(true);

    render(<AccountCenter active={true} />);
    fireEvent.click(await screen.findByTestId("account-card-profile-login-1"));
    fireEvent.click(await screen.findByTestId("account-detail-reset-device"));

    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith("reset_checkin_device", { profileId: "profile-login-1" });
    });
    expect(await screen.findByText(/签到设备已重置/)).toBeInTheDocument();
    confirmSpy.mockRestore();
  });

  it("详情页重置签到设备：取消确认时不改绑", async () => {
    mockRealMode([overviewEntry("profile-login-1", "登录账号甲")]);
    const confirmSpy = vi.spyOn(window, "confirm").mockReturnValue(false);

    render(<AccountCenter active={true} />);
    fireEvent.click(await screen.findByTestId("account-card-profile-login-1"));
    fireEvent.click(await screen.findByTestId("account-detail-reset-device"));

    expect(mockInvoke).not.toHaveBeenCalledWith("reset_checkin_device", expect.anything());
    confirmSpy.mockRestore();
  });

  it("列表页批量刷新额度对所有账号发起只读查询", async () => {
    mockRealMode([
      overviewEntry("profile-login-1", "登录账号甲"),
      overviewEntry("profile-login-2", "登录账号乙"),
    ]);
    mockInvoke.mockImplementation(async (command) => {
      if (command === "get_managed_account_state") return state();
      if (command === "get_checkin_capability") return realCapability;
      if (command === "get_checkin_overview") {
        return [overviewEntry("profile-login-1", "登录账号甲"), overviewEntry("profile-login-2", "登录账号乙")];
      }
      if (command === "refresh_checkin_credits") {
        return [
          { profile_id: "profile-login-1", screen_name: "登录账号甲", credits: 1250, checked_in: true, error_code: null },
          { profile_id: "profile-login-2", screen_name: "登录账号乙", credits: 980, checked_in: true, error_code: null },
        ];
      }
      throw new Error(`unexpected command: ${String(command)}`);
    });

    render(<AccountCenter active={true} />);
    fireEvent.click(await screen.findByTestId("account-refresh-credits"));

    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith("refresh_checkin_credits", {
        profileIds: ["profile-login-1", "profile-login-2"],
      });
    });
    expect(await screen.findByText(/全部 2 个账号额度已更新/)).toBeInTheDocument();
  });

  it("真实模式可通过浏览器登录添加账号并刷新卡片", async () => {
    const before = [overviewEntry("profile-login-1", "登录账号甲")];
    const after = [...before, overviewEntry("profile-login-2", "登录账号乙")];
    let loginStarted = false;
    mockInvoke.mockImplementation(async (command) => {
      if (command === "get_managed_account_state") return state();
      if (command === "get_checkin_capability") return realCapability;
      if (command === "get_checkin_overview") return loginStarted ? after : before;
      if (command === "begin_checkin_login") {
        loginStarted = true;
        return { login_url: "https://www.trae.cn/authorization?challenge=abc" };
      }
      if (command === "complete_checkin_login") {
        return {
          profile_id: "profile-login-2",
          account_id: "account-profile-login-2",
          screen_name: "登录账号乙",
          avatar_url: "",
        };
      }
      throw new Error(`unexpected command: ${String(command)}`);
    });

    render(<AccountCenter active={true} />);
    await screen.findByText("登录账号甲");
    fireEvent.click(screen.getByTestId("account-add-primary"));

    // 默认隔离浏览器：不携带系统登录态，添加账号与本机会话互不影响。
    await waitFor(() =>
      expect(mockInvoke).toHaveBeenCalledWith("begin_checkin_login", { useSystemBrowser: false })
    );
    await waitFor(() => expect(mockInvoke).toHaveBeenCalledWith("complete_checkin_login"));
    expect(await screen.findByText(/账号“登录账号乙”登录成功/)).toBeInTheDocument();
    // 登录后总览刷新，新账号进入「我的账号」卡片。
    expect(await screen.findByTestId("account-card-profile-login-2")).toBeInTheDocument();
  });

  it("选择本机浏览器登录时向登录命令传递系统浏览器标记", async () => {
    // 覆盖登录命令：begin 返回登录页地址即可，本测试只断言浏览器模式传参。
    mockInvoke.mockImplementation(async (command) => {
      if (command === "get_managed_account_state") return state();
      if (command === "get_checkin_capability") return realCapability;
      if (command === "get_checkin_overview") return [];
      if (command === "begin_checkin_login") {
        return { login_url: "https://www.trae.cn/authorization?challenge=abc" };
      }
      if (command === "complete_checkin_login") {
        return {
          profile_id: "profile-login-1",
          account_id: "account-profile-login-1",
          screen_name: "本机登录账号",
          avatar_url: "",
        };
      }
      throw new Error(`unexpected command: ${String(command)}`);
    });

    render(<AccountCenter active={true} />);
    await screen.findByText("还没有账号。通过浏览器登录添加第一个账号，登录成功后即可签到。");
    // 切换登录方式为“本机浏览器”（复用系统已登录会话）后发起登录。
    fireEvent.change(screen.getByTestId("login-browser-mode"), { target: { value: "system" } });
    fireEvent.click(screen.getByTestId("account-add-primary"));

    await waitFor(() =>
      expect(mockInvoke).toHaveBeenCalledWith("begin_checkin_login", { useSystemBrowser: true })
    );
  });

  it("登录存档槽位（P6-2 四态）：待登录亮琥珀；登录有效中性；未保存显示未登录", async () => {
    const entryA = overviewEntry("profile-login-state-a", "存档待登录账号");
    const entryB = overviewEntry("profile-login-state-b", "存档有效账号");
    const entryC = overviewEntry("profile-login-state-c", "存档未初始化账号");
    mockInvoke.mockImplementation(async (command) => {
      if (command === "get_managed_account_state") return state();
      if (command === "get_checkin_capability") return realCapability;
      if (command === "get_checkin_overview") return [entryA, entryB, entryC];
      if (command === "get_trae_instance_states") {
        return [
          { profile_id: entryA.profile_id, login_state: "logged_out", archive_available: false },
          { profile_id: entryB.profile_id, login_state: "logged_in", archive_available: true },
          { profile_id: entryC.profile_id, login_state: "uninitialized", archive_available: true },
        ];
      }
      throw new Error(`unexpected command: ${String(command)}`);
    });

    render(<AccountCenter active={true} />);
    await screen.findByText("存档待登录账号");
    // logged_out：琥珀警示（存档在但登录键缺失，重新登录一次即可恢复）。
    expect(screen.getByText("待登录")).toBeInTheDocument();
    // logged_in：「登录有效」（中性灰，凭据存档可直接用于切换账号）。
    const cardB = screen.getByTestId("account-card-profile-login-state-b");
    expect(cardB).toHaveTextContent("登录有效");
    // P7-5 存档可用（切换备用方式）收进悬浮提示，不占主视野。
    expect(screen.getByText("登录有效")).toHaveAttribute(
      "title",
      expect.stringContaining("历史登录存档"),
    );
    // 未初始化：「未登录」（尚未保存过登录凭据，真正无信息量才用这个词）。
    const cardC = screen.getByTestId("account-card-profile-login-state-c");
    expect(cardC).toHaveTextContent("未登录");
    // 未登录但保留存档：提示存档仍可用于切换账号（备用方式不丢）。
    expect(screen.getByText("未登录")).toHaveAttribute(
      "title",
      expect.stringContaining("仍可用于切换账号"),
    );
  });

  it("登录存档槽位：失效态（登录键在但会话过期）亮琥珀加强警示", async () => {
    const entry = overviewEntry("profile-login-stale-1", "失效账号");
    mockInvoke.mockImplementation(async (command) => {
      if (command === "get_managed_account_state") return state();
      if (command === "get_checkin_capability") return realCapability;
      if (command === "get_checkin_overview") return [entry];
      if (command === "get_trae_instance_states") {
        // 模拟 LY 案例：登录键仍在但最近启动日志含服务端拒绝证据。
        return [{ profile_id: entry.profile_id, login_state: "stale", archive_available: true }];
      }
      throw new Error(`unexpected command: ${String(command)}`);
    });

    render(<AccountCenter active={true} />);
    expect(await screen.findByText("登录失效")).toBeInTheDocument();
    const badge = screen.getByText("登录失效");
    expect(badge).toHaveClass("slot-badge--warn");
  });

  it("旧通道凭据账号：实调通过也按登录失效展示（本地判定，页面加载即生效）", async () => {
    const entry = overviewEntry("profile-legacy-1", "旧通道账号", { credential_legacy: true });
    mockInvoke.mockImplementation(async (command) => {
      if (command === "get_managed_account_state") return state();
      if (command === "get_checkin_capability") return realCapability;
      if (command === "get_checkin_overview") return [entry];
      if (command === "get_trae_instance_states") {
        // token 尚未过期：实调通过——这正是修复前“已失效仍显示登录有效”的失真场景。
        return [{ profile_id: entry.profile_id, login_state: "logged_in", archive_available: false }];
      }
      throw new Error(`unexpected command: ${String(command)}`);
    });

    render(<AccountCenter active={true} />);
    expect(await screen.findByText("登录失效")).toBeInTheDocument();
    // 旧通道不会自动恢复：悬浮提示直接指向重新登录，而非“等待自动恢复”。
    expect(screen.getByText("登录失效")).toHaveAttribute(
      "title",
      expect.stringContaining("重新登录一次即可更新凭据"),
    );
  });

  it("续期被拒的持久化失败标记：徽章融合为登录失效，卡片显示刷新失败", async () => {
    const entry = overviewEntry("profile-renew-dead-1", "续期死亡账号", {
      refresh_error_code: "credential_refresh_failed",
    });
    mockInvoke.mockImplementation(async (command) => {
      if (command === "get_managed_account_state") return state();
      if (command === "get_checkin_capability") return realCapability;
      if (command === "get_checkin_overview") return [entry];
      if (command === "get_trae_instance_states") {
        return [{ profile_id: entry.profile_id, login_state: "logged_in", archive_available: false }];
      }
      throw new Error(`unexpected command: ${String(command)}`);
    });

    render(<AccountCenter active={true} />);
    expect(await screen.findByText("登录失效")).toBeInTheDocument();
    expect(screen.getByText("刷新失败")).toBeInTheDocument();
  });

  it("批量刷新额度：失败账号在汇总消息中列名，成功后卡片标记消失", async () => {
    const entryA = overviewEntry("profile-refresh-a", "刷新成功账号");
    const entryB = overviewEntry("profile-refresh-b", "刷新失败账号");
    let refreshed = false;
    mockInvoke.mockImplementation(async (command) => {
      if (command === "get_managed_account_state") return state();
      if (command === "get_checkin_capability") return realCapability;
      if (command === "get_checkin_overview") {
        // 刷新后总览带回持久化失败标记（后端已写入额度缓存）。
        return refreshed
          ? [entryA, overviewEntry("profile-refresh-b", "刷新失败账号", { refresh_error_code: "credential_refresh_failed" })]
          : [entryA, entryB];
      }
      if (command === "refresh_checkin_credits") {
        refreshed = true;
        return [
          { profile_id: entryA.profile_id, screen_name: entryA.screen_name, credits: 200, checked_in: true, usage_remaining_credits: 100, error_code: null },
          { profile_id: entryB.profile_id, screen_name: entryB.screen_name, credits: null, checked_in: null, usage_remaining_credits: null, error_code: "credential_refresh_failed" },
        ];
      }
      if (command === "get_trae_instance_states") {
        return [entryA, entryB].map((entry) => ({
          profile_id: entry.profile_id,
          login_state: "logged_in",
          archive_available: false,
        }));
      }
      throw new Error(`unexpected command: ${String(command)}`);
    });

    render(<AccountCenter active={true} />);
    await screen.findByText("刷新成功账号");
    fireEvent.click(screen.getByTestId("account-refresh-credits"));

    // 汇总消息直接列出失败账号名（不再只说“详见各账号详情”）。
    expect(await screen.findByText(/1 个成功，1 个失败（刷新失败账号）/)).toBeInTheDocument();
    // 总览带回持久化标记：卡片显示“刷新失败”，徽章融合为登录失效。
    expect(await screen.findByText("刷新失败")).toBeInTheDocument();
    expect(await screen.findByText("登录失效")).toBeInTheDocument();
  });

  it("详情页展示最近额度刷新失败原因与旧通道续期提示", async () => {
    const entry = overviewEntry("profile-detail-fail-1", "失败详情账号", {
      refresh_error_code: "credential_refresh_failed",
      credential_legacy: true,
    });
    mockInvoke.mockImplementation(async (command) => {
      if (command === "get_managed_account_state") return state();
      if (command === "get_checkin_capability") return realCapability;
      if (command === "get_checkin_overview") return [entry];
      if (command === "get_trae_instance_states") {
        return [{ profile_id: entry.profile_id, login_state: "logged_in", archive_available: false }];
      }
      throw new Error(`unexpected command: ${String(command)}`);
    });

    render(<AccountCenter active={true} />);
    fireEvent.click(await screen.findByTestId("account-card-profile-detail-fail-1"));

    // 基础信息：最近刷新失败 + 原因文案（错误码映射，不露底层正文）。
    expect(await screen.findByTestId("account-detail-refresh-status")).toHaveTextContent(
      "失败：登录凭据已过期或不可用，无法自动续期；请重新登录该账号以更新凭据。",
    );
    // 登录健康度：旧通道提示替代“可自动续期”的误导文案。
    expect(await screen.findByTestId("account-detail-legacy-hint")).toHaveTextContent("旧版登录通道");
  });

  it("健康检测按钮：本地检测 + 网络探测后徽章刷新并给出汇总", async () => {
    const entryA = overviewEntry("profile-health-a", "健康账号甲");
    const entryB = overviewEntry("profile-health-b", "失效账号乙");
    // 初始读取：两账号存档均登录有效（旧数据），健康检测后乙翻转为 stale。
    let statesLogin: Record<string, string> = {
      [entryA.profile_id]: "logged_in",
      [entryB.profile_id]: "logged_in",
    };
    mockInvoke.mockImplementation(async (command) => {
      if (command === "get_managed_account_state") return state();
      if (command === "get_checkin_capability") return realCapability;
      if (command === "get_checkin_overview") return [entryA, entryB];
      if (command === "get_trae_instance_states") {
        return [entryA, entryB].map((entry) => ({
          profile_id: entry.profile_id,
          login_state: statesLogin[entry.profile_id],
          archive_available: false,
        }));
      }
      if (command === "refresh_checkin_credits") {
        // 网络探测：甲正常，乙签到会话异常。
        return [
          { profile_id: entryA.profile_id, screen_name: entryA.screen_name, credits: 200, checked_in: true, usage_remaining_credits: 100, error_code: null },
          { profile_id: entryB.profile_id, screen_name: entryB.screen_name, credits: null, checked_in: null, usage_remaining_credits: null, error_code: "credential_expired" },
        ];
      }
      throw new Error(`unexpected command: ${String(command)}`);
    });

    render(<AccountCenter active={true} />);
    await screen.findByText("健康账号甲");
    // 初始：两个账号都显示登录有效。
    await waitFor(() => expect(screen.getAllByText("登录有效").length).toBe(2));

    // 模拟后端深度检测即将发现的新证据（乙的最近启动日志含拒绝记录）。
    statesLogin[entryB.profile_id] = "stale";
    fireEvent.click(screen.getByTestId("account-health-check"));

    // 检测后：乙的徽章立即翻转为失效（本地深度检测即时刷新）。
    expect(await screen.findByText("登录失效")).toBeInTheDocument();
    // 汇总消息包含登录存档段与签到会话段。
    expect(await screen.findByText(/健康检测完成。登录存档：1 登录有效、1 登录失效；签到会话：1 正常、1 异常（失效账号乙）/)).toBeInTheDocument();
    await waitFor(() => expect(mockInvoke).toHaveBeenCalledWith("refresh_checkin_credits", { profileIds: [entryA.profile_id, entryB.profile_id] }));
  });

  it("真实模式未登录任何账号时显示空状态引导且无签到入口", async () => {
    mockRealMode([]);

    render(<AccountCenter active={true} />);
    await screen.findByText("还没有账号。通过浏览器登录添加第一个账号，登录成功后即可签到。");
    expect(screen.getByTestId("account-add-primary")).toBeEnabled();
    expect(screen.queryByTestId("account-refresh-credits")).not.toBeInTheDocument();
  });

  it("登录失败显示稳定错误提示，不出现底层错误正文", async () => {
    mockInvoke.mockImplementation(async (command) => {
      if (command === "get_managed_account_state") return state();
      if (command === "get_checkin_capability") return realCapability;
      if (command === "get_checkin_overview") return [];
      if (command === "begin_checkin_login") return { login_url: "https://www.trae.cn/authorization?challenge=abc" };
      // 后端登录失败以稳定错误码返回；UI 只映射为固定文案。
      if (command === "complete_checkin_login") throw "login_callback_invalid";
      throw new Error(`unexpected command: ${String(command)}`);
    });

    render(<AccountCenter active={true} />);
    // 无登录账号时页头与空状态各有一个“添加账号”，用 testid 定位页头主按钮
    fireEvent.click(await screen.findByTestId("account-add-primary"));

    await waitFor(() => {
      expect(screen.getByRole("alert")).toHaveTextContent("浏览器登录回调无效，请重新发起登录。");
    });
    // 登录失败后按钮恢复可用，可重新发起
    expect(screen.getByTestId("account-add-primary")).toBeEnabled();
  });

  it("P7-4：登录等待期间可取消，取消后按钮解锁且不显示为错误", async () => {
    // complete 挂起模拟等待浏览器回调；cancel 命令置位后 complete 以
    // login_cancelled 收尾（后端一个轮询周期内完成，此处同步触发）。
    let rejectLogin: ((reason: string) => void) | null = null;
    mockInvoke.mockImplementation(async (command) => {
      if (command === "get_managed_account_state") return state();
      if (command === "get_checkin_capability") return realCapability;
      if (command === "get_checkin_overview") return [];
      if (command === "begin_checkin_login") return { login_url: "https://www.trae.cn/authorization?challenge=abc" };
      if (command === "complete_checkin_login") {
        return new Promise<never>((_resolve, reject) => {
          rejectLogin = reject;
        });
      }
      if (command === "cancel_checkin_login") {
        rejectLogin?.("login_cancelled");
        return null;
      }
      throw new Error(`unexpected command: ${String(command)}`);
    });

    render(<AccountCenter active={true} />);
    fireEvent.click(await screen.findByTestId("account-add-primary"));

    // 等待期间出现取消入口，按钮文案保持等待态。
    expect(await screen.findByTestId("account-cancel-login")).toBeInTheDocument();
    expect(screen.getByTestId("account-add-primary")).toHaveTextContent("等待浏览器登录完成…");

    fireEvent.click(screen.getByTestId("account-cancel-login"));
    await waitFor(() => expect(mockInvoke).toHaveBeenCalledWith("cancel_checkin_login"));
    // 取消走中性提示（role=status），不进错误通道（role=alert）。
    expect(await screen.findByText(/登录已取消/)).toBeInTheDocument();
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
    // 按钮解锁，可重新发起登录。
    await waitFor(() => expect(screen.getByTestId("account-add-primary")).toBeEnabled());
    expect(screen.getByTestId("account-add-primary")).toHaveTextContent("添加账号");
  });

  it("账号总览读取失败时错误可见，不静默伪装成空账号", async () => {
    mockInvoke.mockImplementation(async (command) => {
      if (command === "get_managed_account_state") return state();
      if (command === "get_checkin_capability") return realCapability;
      if (command === "get_checkin_overview") throw "checkin_registry_invalid";
      throw new Error(`unexpected command: ${String(command)}`);
    });

    render(<AccountCenter active={true} />);
    await waitFor(() => {
      expect(screen.getByRole("alert")).toHaveTextContent("账号注册表不可读取，请重启应用后重试；若持续出现请保留数据目录后反馈。");
    });
  });

  it("U-2 双视图：默认列表宽行，分段控件切换到卡片并记忆偏好", async () => {
    mockRealMode([
      overviewEntry("profile-view-1", "视图账号甲"),
      overviewEntry("profile-view-2", "视图账号乙"),
    ]);

    render(<AccountCenter active={true} />);
    await screen.findByText("视图账号甲");
    // 默认列表视图：行容器 + 列表分段项激活。
    expect(document.querySelector(".account-list__row")).not.toBeNull();
    expect(screen.getByTestId("account-view-list")).toHaveClass("seg-control__item--active");

    // 切到卡片视图：容器类切换 + 偏好写入 localStorage。
    fireEvent.click(screen.getByTestId("account-view-card"));
    await waitFor(() => expect(document.querySelector(".account-list__row")).toBeNull());
    expect(document.querySelector(".account-card--clickable")).not.toBeNull();
    expect(screen.getByTestId("account-view-card")).toHaveClass("seg-control__item--active");
    expect(window.localStorage.getItem("accounts.view")).toBe("card");

    // 切回列表：偏好更新。
    fireEvent.click(screen.getByTestId("account-view-list"));
    await waitFor(() => expect(document.querySelector(".account-list__row")).not.toBeNull());
    expect(window.localStorage.getItem("accounts.view")).toBe("list");
  });

  it("U-2 排序：按签到状态（未签在前）与名称排序生效并记忆偏好", async () => {
    mockRealMode([
      overviewEntry("profile-sort-1", "乙账号", { checked_in: true }),
      overviewEntry("profile-sort-2", "甲账号", { checked_in: false }),
    ]);

    render(<AccountCenter active={true} />);
    await screen.findByText("乙账号");
    // 默认添加序：乙（先注册）在前。
    const list = document.querySelector("ul.account-list");
    expect(list).not.toBeNull();
    expect(list?.children[0]).toHaveTextContent("乙账号");

    // 切签到状态排序：未签的甲升到首位。
    fireEvent.change(screen.getByTestId("account-sort"), { target: { value: "checkin" } });
    await waitFor(() => expect(list?.children[0]).toHaveTextContent("甲账号"));
    expect(window.localStorage.getItem("accounts.sort")).toBe("checkin");

    // 切名称排序：甲（拼音序）在前，与签到无关。
    fireEvent.change(screen.getByTestId("account-sort"), { target: { value: "name" } });
    await waitFor(() => expect(list?.children[0]).toHaveTextContent("甲账号"));
    expect(window.localStorage.getItem("accounts.sort")).toBe("name");

    // 回添加序：恢复注册表顺序。
    fireEvent.change(screen.getByTestId("account-sort"), { target: { value: "added" } });
    await waitFor(() => expect(list?.children[0]).toHaveTextContent("乙账号"));
  });
});
