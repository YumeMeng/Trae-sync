import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
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
    mobile_full: null,
    auto_checkin_enabled: true,
    refresh_error_code: null,
    credential_legacy: false,
    last_attempt_outcome: null,
    last_attempt_date: null,
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
    // 可断言文本：名称、模型积分额度、签到槽（已签）、G13 meta 精简后的手机号。
    expect(card).toHaveTextContent("登录账号甲");
    expect(card).toHaveTextContent("模型积分");
    expect(card).toHaveTextContent("1240");
    expect(screen.getByText("已签")).toBeInTheDocument();
    expect(screen.getByText("138****0000", { selector: ".account-item__meta" })).toBeInTheDocument();
    // G13：令牌天数不再出现在列表 meta（收进详情页）。
    expect(card).not.toHaveTextContent(/令牌 \d+ 天/);
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

  it("token 健康度不再占列表 meta：令牌天数与过期警示收进登录槽徽章与详情页", async () => {
    const now = Date.now() / 1000;
    mockRealMode([
      overviewEntry("profile-a", "过期账号", { access_token_expires_at_unix_seconds: Math.floor(now - 100) }),
    ]);

    render(<AccountCenter active={true} />);
    const card = await screen.findByTestId("account-card-profile-a");
    // G13 移除 meta 令牌文字；G10 后过期警示由登录槽徽章承载（此处凭据未初始化 → 未登录灰）。
    expect(card).not.toHaveTextContent(/令牌 \d+ 天/);
    expect(card).not.toHaveTextContent("登录已过期");
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
    // G9 结果卡：全部正常时只显示结论行（正常账号不占空间）。
    const card = await screen.findByTestId("operation-result-card");
    expect(card).toHaveTextContent("额度刷新完成：2 个账号正常。");
    expect(screen.queryByText("登录账号甲", { selector: ".result-card__issue-name" })).not.toBeInTheDocument();
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
    // G11 登录成功会弹补录层：收尾关闭，避免影响后续断言。
    const skip = await screen.findByTestId("mobile-backfill-skip");
    fireEvent.click(skip);
    await waitFor(() =>
      expect(screen.queryByTestId("mobile-backfill-dialog")).not.toBeInTheDocument()
    );
  });

  it("G11 登录成功后弹手机号补录层：展示脱敏号供比对，跳过不发起保存", async () => {
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
    await screen.findByTestId("mobile-backfill-dialog");

    // 弹层内容：标题 + 账号名 + 服务端脱敏号（肉眼比对基准）。
    const dialog = screen.getByTestId("mobile-backfill-dialog");
    expect(dialog).toHaveTextContent("补全手机号");
    expect(dialog).toHaveTextContent("登录账号乙");
    expect(dialog).toHaveTextContent("服务端记录：138****0000");

    // 跳过：关闭弹层且不发起保存。
    fireEvent.click(screen.getByTestId("mobile-backfill-skip"));
    await waitFor(() =>
      expect(screen.queryByTestId("mobile-backfill-dialog")).not.toBeInTheDocument()
    );
    expect(mockInvoke).not.toHaveBeenCalledWith("set_account_mobile", expect.anything());
  });

  it("G11 补录弹层空输入按 Enter 不触发保存（防误清除已补录手机号）", async () => {
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
    await screen.findByTestId("mobile-backfill-dialog");

    // 空输入按 Enter：与保存按钮 disabled 同口径，弹层保持、不发起保存
    // （saveMobileBackfill 空串 = set_account_mobile(null) 清除补录，属误触发）。
    fireEvent.keyDown(screen.getByTestId("mobile-backfill-input"), { key: "Enter" });
    await waitFor(() =>
      expect(mockInvoke).not.toHaveBeenCalledWith("set_account_mobile", expect.anything())
    );
    expect(screen.getByTestId("mobile-backfill-dialog")).toBeInTheDocument();
  });

  it("G11 已补录手机号的账号重复登录不再弹补录层", async () => {
    const before = [overviewEntry("profile-login-1", "登录账号甲")];
    const after = [
      ...before,
      overviewEntry("profile-login-2", "登录账号乙", { mobile_full: "13812340000" }),
    ];
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
    await waitFor(() => expect(mockInvoke).toHaveBeenCalledWith("complete_checkin_login"));
    // 已补录账号重复登录：不再打扰，不弹补录层。
    await waitFor(() =>
      expect(screen.queryByTestId("mobile-backfill-dialog")).not.toBeInTheDocument()
    );
  });

  it("G11 登录后补录保存：调用 set_account_mobile 并重读总览后关闭弹层", async () => {
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
      if (command === "set_account_mobile") return null;
      throw new Error(`unexpected command: ${String(command)}`);
    });

    render(<AccountCenter active={true} />);
    await screen.findByText("登录账号甲");
    fireEvent.click(screen.getByTestId("account-add-primary"));
    await screen.findByTestId("mobile-backfill-dialog");

    // 输入完整手机号并保存：非数字输入被过滤（输入框只收数字）。
    const input = screen.getByTestId("mobile-backfill-input");
    fireEvent.change(input, { target: { value: "1381234abc0000" } });
    expect(input).toHaveValue("13812340000");
    fireEvent.click(screen.getByTestId("mobile-backfill-save"));

    await waitFor(() =>
      expect(mockInvoke).toHaveBeenCalledWith("set_account_mobile", {
        profileId: "profile-login-2",
        mobile: "13812340000",
      })
    );
    // 保存成功后弹层关闭并提示。
    await waitFor(() =>
      expect(screen.queryByTestId("mobile-backfill-dialog")).not.toBeInTheDocument()
    );
    expect(await screen.findByText(/手机号已补全/)).toBeInTheDocument();
  });

  it("G11 补录校验失败：脱敏号不匹配时弹层就地展示映射文案，不关闭", async () => {
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
      if (command === "set_account_mobile") throw "mobile_masked_mismatch";
      throw new Error(`unexpected command: ${String(command)}`);
    });

    render(<AccountCenter active={true} />);
    await screen.findByText("登录账号甲");
    fireEvent.click(screen.getByTestId("account-add-primary"));
    await screen.findByTestId("mobile-backfill-dialog");

    fireEvent.change(screen.getByTestId("mobile-backfill-input"), { target: { value: "13999990000" } });
    fireEvent.click(screen.getByTestId("mobile-backfill-save"));

    // 后端拒绝（首尾号段与脱敏号不一致）：弹层保留，映射后的文案就地展示。
    expect(await screen.findByTestId("mobile-backfill-error")).toHaveTextContent(
      "手机号与该账号的服务端记录不一致（首尾号段不匹配），请核对后重新输入。"
    );
    expect(screen.getByTestId("mobile-backfill-dialog")).toBeInTheDocument();
  });

  it("G11 详情页手机号行内补录：保存调用命令，卡片/详情显示补录后的全号", async () => {
    const saved = overviewEntry("profile-m-1", "手机号账号", { mobile_full: "13812340000" });
    const initial = { ...saved, mobile_full: null };
    let backfilled = false;
    mockInvoke.mockImplementation(async (command) => {
      if (command === "get_managed_account_state") return state();
      if (command === "get_checkin_capability") return realCapability;
      if (command === "get_checkin_overview") return backfilled ? [saved] : [initial];
      if (command === "set_account_mobile") {
        backfilled = true;
        return null;
      }
      throw new Error(`unexpected command: ${String(command)}`);
    });

    render(<AccountCenter active={true} />);
    // 未补录时：列表 meta 显示脱敏号。
    await waitFor(() =>
      expect(screen.getByText("138****0000", { selector: ".account-item__meta" })).toBeInTheDocument()
    );

    // 进入详情：输入框占位为脱敏号，下方有比对提示。
    fireEvent.click(screen.getByTestId("account-card-profile-m-1"));
    const input = await screen.findByTestId("account-detail-mobile-input");
    expect(input).toHaveAttribute("placeholder", "138****0000");
    expect(screen.getByText(/服务端脱敏号：138\*\*\*\*0000/)).toBeInTheDocument();

    // 输入全号保存：调用命令并重读总览。
    fireEvent.change(input, { target: { value: "13812340000" } });
    fireEvent.click(screen.getByTestId("account-detail-mobile-save"));
    await waitFor(() =>
      expect(mockInvoke).toHaveBeenCalledWith("set_account_mobile", {
        profileId: "profile-m-1",
        mobile: "13812340000",
      })
    );
    expect(await screen.findByText("手机号已保存。")).toBeInTheDocument();

    // 返回列表：补录后的全号替代脱敏号展示。
    fireEvent.click(screen.getByTestId("account-detail-back"));
    await waitFor(() =>
      expect(screen.getByText("13812340000", { selector: ".account-item__meta" })).toBeInTheDocument()
    );
  });

  it("G11 查重提示：输入已用于其他账号的手机号需确认，取消则不保存", async () => {
    mockRealMode([
      overviewEntry("profile-m-1", "账号一"),
      overviewEntry("profile-m-2", "账号二", { mobile_full: "13912340000", masked_mobile: "139****0000" }),
    ]);
    const confirmSpy = vi.spyOn(window, "confirm").mockReturnValue(false);

    render(<AccountCenter active={true} />);
    await screen.findByTestId("account-card-profile-m-1");
    fireEvent.click(screen.getByTestId("account-card-profile-m-1"));

    const input = await screen.findByTestId("account-detail-mobile-input");
    fireEvent.change(input, { target: { value: "13912340000" } });
    fireEvent.click(screen.getByTestId("account-detail-mobile-save"));

    // 第三层查重：提示已用于账号二；用户取消则不发起保存。
    await waitFor(() =>
      expect(confirmSpy).toHaveBeenCalledWith(expect.stringContaining("账号二"))
    );
    expect(mockInvoke).not.toHaveBeenCalledWith("set_account_mobile", expect.anything());
    confirmSpy.mockRestore();
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

  it("登录存档槽位（G10 五态）：待登录亮琥珀；正常亮绿；未保存显示未登录", async () => {
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
    // logged_in：「正常」（G10 绿——凭据实调通过，切换账号直接可用）。
    const cardB = screen.getByTestId("account-card-profile-login-state-b");
    expect(cardB).toHaveTextContent("正常");
    // P7-5 存档可用（切换备用方式）收进悬浮提示，不占主视野。
    expect(screen.getByText("正常")).toHaveAttribute(
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

  it("登录存档槽位：失效态（登录键在但会话过期）显示已过期，亮琥珀等待自动恢复", async () => {
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
    expect(await screen.findByText("已过期")).toBeInTheDocument();
    const badge = screen.getByText("已过期");
    expect(badge).toHaveClass("slot-badge--warn");
  });

  it("旧通道凭据账号：实调通过也按需重登展示（本地判定，页面加载即生效）", async () => {
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
    expect(await screen.findByText("需重登")).toBeInTheDocument();
    // 旧通道不会自动恢复：悬浮提示直接指向重新登录，而非“等待自动恢复”。
    expect(screen.getByText("需重登")).toHaveAttribute(
      "title",
      expect.stringContaining("重新登录一次即可更新凭据"),
    );
  });

  it("续期被拒的持久化失败标记：徽章融合为需重登，卡片显示刷新失败", async () => {
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
    expect(await screen.findByText("需重登")).toBeInTheDocument();
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

    // G9 结果卡：首行结论 + 异常账号列名与一句人话原因（正常账号不占空间）。
    expect(await screen.findByText("额度刷新完成：1 个账号正常，1 个需要处理。")).toBeInTheDocument();
    expect(screen.getByText("刷新失败账号", { selector: ".result-card__issue-name" })).toBeInTheDocument();
    expect(
      screen.getByText("登录凭据已过期或不可用，无法自动续期；请重新登录该账号以更新凭据。"),
    ).toBeInTheDocument();
    expect(screen.queryByText("刷新成功账号", { selector: ".result-card__issue-name" })).not.toBeInTheDocument();
    // 总览带回持久化标记：卡片显示“刷新失败”，徽章融合为需重登。
    expect(await screen.findByText("刷新失败")).toBeInTheDocument();
    expect(await screen.findByText("需重登")).toBeInTheDocument();
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
    // 初始：两个账号都显示正常（G10 绿）。
    await waitFor(() => expect(screen.getAllByText("正常").length).toBe(2));

    // 模拟后端深度检测即将发现的新证据（乙的最近启动日志含拒绝记录）。
    statesLogin[entryB.profile_id] = "stale";
    fireEvent.click(screen.getByTestId("account-health-check"));

    // 检测后：乙的徽章立即翻转为已过期（本地深度检测即时刷新）。
    expect(await screen.findByText("已过期")).toBeInTheDocument();
    // G9 结果卡：首行结论 + 乙列为需处理（登录原因优先于会话探测异常）。
    expect(await screen.findByText("健康检测完成：1 个账号正常，1 个需要处理。")).toBeInTheDocument();
    expect(screen.getByText("失效账号乙", { selector: ".result-card__issue-name" })).toBeInTheDocument();
    expect(screen.getByText("登录态已过期，等待自动恢复")).toBeInTheDocument();
    await waitFor(() => expect(mockInvoke).toHaveBeenCalledWith("refresh_checkin_credits", { profileIds: [entryA.profile_id, entryB.profile_id] }));
  });

  it("健康检测：未登录（从未保存凭据）不计为需处理（灰=中性，与 G14 口径一致）", async () => {
    const entry = overviewEntry("profile-silent", "未登录账号");
    mockInvoke.mockImplementation(async (command) => {
      if (command === "get_managed_account_state") return state();
      if (command === "get_checkin_capability") return realCapability;
      if (command === "get_checkin_overview") return [entry];
      if (command === "get_trae_instance_states") {
        return [{ profile_id: entry.profile_id, login_state: "uninitialized", archive_available: false }];
      }
      if (command === "refresh_checkin_credits") {
        // 探测本身成功：该账号无任何需处理项。
        return [{ profile_id: entry.profile_id, screen_name: entry.screen_name, credits: 200, checked_in: true, usage_remaining_credits: 100, error_code: null }];
      }
      throw new Error(`unexpected command: ${String(command)}`);
    });

    render(<AccountCenter active={true} />);
    await screen.findByText("未登录账号");
    fireEvent.click(screen.getByTestId("account-health-check"));

    // uninitialized = 未登录（灰·中性）：不算需处理，结果卡只报正常。
    expect(await screen.findByText("健康检测完成：1 个账号正常。")).toBeInTheDocument();
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

  it("G14 需处理过滤：登录槽红或签到槽红计入角标，切换后只显示异常账号", async () => {
    mockRealMode([
      overviewEntry("profile-ok", "正常账号", { checked_in: true }),
      // 旧通道凭据 → 登录槽红（需重登）。
      overviewEntry("profile-dead", "凭据失效账号", { credential_legacy: true }),
      // 今日业务性签到失败 → 签到槽红。
      overviewEntry("profile-failed", "签到失败账号", {
        checked_in: null,
        last_attempt_outcome: "business:not_eligible",
        last_attempt_date: new Date().toISOString(),
      }),
      // 未签（灰）与未刷新（灰空心）不算异常。
      overviewEntry("profile-unchecked", "未签账号", { checked_in: false }),
    ]);

    render(<AccountCenter active={true} />);
    await screen.findByText("正常账号");
    // 默认「全部」：四个账号全部可见。
    expect(screen.getByTestId("account-card-profile-ok")).toBeInTheDocument();
    expect(screen.getByTestId("account-card-profile-unchecked")).toBeInTheDocument();

    // 「需处理」角标 = 2（凭据失效 + 签到失败）。
    const attentionBtn = screen.getByTestId("account-filter-attention");
    expect(attentionBtn).toHaveTextContent("需处理");
    expect(attentionBtn).toHaveTextContent("2");

    // 切到「需处理」：只剩异常账号，正常与未签账号隐藏。
    fireEvent.click(attentionBtn);
    await waitFor(() => expect(screen.queryByTestId("account-card-profile-ok")).not.toBeInTheDocument());
    expect(screen.queryByTestId("account-card-profile-unchecked")).not.toBeInTheDocument();
    expect(screen.getByTestId("account-card-profile-dead")).toBeInTheDocument();
    expect(screen.getByTestId("account-card-profile-failed")).toBeInTheDocument();

    // 切回「全部」：全量恢复。
    fireEvent.click(screen.getByTestId("account-filter-all"));
    await waitFor(() => expect(screen.getByTestId("account-card-profile-ok")).toBeInTheDocument());
    expect(screen.getByTestId("account-card-profile-unchecked")).toBeInTheDocument();
  });

  it("G14 需处理过滤：登录槽琥珀（存档过期）也计入；过滤后无异常时给空态提示", async () => {
    const entry = overviewEntry("profile-stale-1", "存档过期账号", { checked_in: true });
    mockInvoke.mockImplementation(async (command) => {
      if (command === "get_managed_account_state") return state();
      if (command === "get_checkin_capability") return realCapability;
      if (command === "get_checkin_overview") return [entry];
      if (command === "get_trae_instance_states") {
        return [{ profile_id: entry.profile_id, login_state: "stale", archive_available: true }];
      }
      throw new Error(`unexpected command: ${String(command)}`);
    });

    render(<AccountCenter active={true} />);
    await screen.findByText("已过期");
    // 「需处理」角标 = 1。
    expect(screen.getByTestId("account-filter-attention")).toHaveTextContent("1");

    fireEvent.click(screen.getByTestId("account-filter-attention"));
    // 琥珀账号仍显示在需处理视图。
    expect(await screen.findByTestId("account-card-profile-stale-1")).toBeInTheDocument();
  });

  it("G14 需处理过滤：全部正常时无角标，切过去显示空态提示", async () => {
    mockRealMode([
      overviewEntry("profile-ok-1", "正常账号甲", { checked_in: true }),
      overviewEntry("profile-ok-2", "正常账号乙", { checked_in: true }),
    ]);

    render(<AccountCenter active={true} />);
    await screen.findByText("正常账号甲");
    // 正常时「需处理」无数字角标。
    expect(screen.getByTestId("account-filter-attention")).not.toHaveTextContent(/\d/);

    // 切过去：空列表 + 一句人话空态（不是「还没有账号」误导）。
    fireEvent.click(screen.getByTestId("account-filter-attention"));
    await waitFor(() => expect(screen.queryByTestId("account-card-profile-ok-1")).not.toBeInTheDocument());
    expect(screen.getByText("没有需要处理的账号。")).toBeInTheDocument();
  });

  it("G15 纯本地刷新：重读总览与环境档案，不发网络探测命令", async () => {
    mockRealMode([overviewEntry("profile-r1", "刷新账号甲")]);

    render(<AccountCenter active={true} />);
    await screen.findByTestId("account-card-profile-r1");
    const callsOf = (command: string) => mockInvoke.mock.calls.filter(([name]) => name === command).length;
    const overviewBefore = callsOf("get_checkin_overview");
    const environmentBefore = callsOf("get_environment_state");
    // 含网络实调的两类命令的初始调用次数（健康检测/额度刷新才会触发）。
    const statesBefore = callsOf("get_trae_instance_states");
    const creditsBefore = callsOf("refresh_checkin_credits");

    fireEvent.click(screen.getByTestId("account-refresh-local"));
    // 总览与环境档案（均为本地读取）被重读。
    await waitFor(() => expect(callsOf("get_checkin_overview")).toBeGreaterThan(overviewBefore));
    await waitFor(() => expect(callsOf("get_environment_state")).toBeGreaterThan(environmentBefore));
    // 等待一个宏任务回合，确认没有异步尾巴发出网络命令。
    await act(async () => { await new Promise((resolve) => setTimeout(resolve, 20)); });
    expect(callsOf("get_trae_instance_states")).toBe(statesBefore);
    expect(callsOf("refresh_checkin_credits")).toBe(creditsBefore);
  });
});
