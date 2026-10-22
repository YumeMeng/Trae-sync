import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { installFakeInvoke, mockCoreInvoke, mockRoute, resetRoutes } from "./helpers/fakeInvoke";
import type { CheckinOverviewEntryDto } from "../src/types/account_switch";
import type { WorkBuddyAccountEntryDto } from "../src/types/product";

// 共享 fakeInvoke 路由器（迁移模式见 tests/helpers/fakeInvoke.ts 头注释）。
vi.mock("@tauri-apps/api/core", async () => {
  const fake = await import("./helpers/fakeInvoke");
  return { invoke: fake.mockCoreInvoke };
});
// 批量进度事件订阅：jsdom 无 Tauri IPC，listen 给空退订函数即可。
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn().mockResolvedValue(() => undefined) }));
// 通知走 notify 适配层：断言 notify 调用，不断言页内文本。
vi.mock("../src/platform/notify", () => ({
  notify: { success: vi.fn(), error: vi.fn(), warning: vi.fn(), info: vi.fn(), summary: vi.fn() },
}));

import { notify } from "../src/platform/notify";
import { AccountsPage } from "../src/features/accounts/AccountsPage";

/** 判定时刻基准：真实总览面的令牌过期与登录槽求态都按 Unix 秒。 */
const NOW = Math.floor(Date.now() / 1000);
const DAY = 86400;
/** 真实签到能力（transport 非 fixture → 页头给「添加账号」主行动）。 */
const REAL_CAPABILITY = { enabled: true, transport: "real_http", real_http_enabled: true, message: "真实签到已启用。" };

/** 真实总览条目构造器：默认「已签 + 登录正常 + 令牌未过期」的健康账号。 */
function traeEntry(profileId: string, name: string, overrides: Partial<CheckinOverviewEntryDto> = {}): CheckinOverviewEntryDto {
  return {
    profile_id: profileId, screen_name: name, account_id: `acct-${profileId}`,
    created_at: "2026-08-01T00:00:00.000Z", last_verified_at: "2026-08-22T00:00:00.000Z",
    credits: 200, credits_cached_at: "2026-08-22T01:00:00.000Z", usage_remaining_credits: 1600,
    usage_cached_at: "2026-08-22T01:00:00.000Z", checked_in: true, device_tail: "8502",
    access_token_expires_at_unix_seconds: NOW + 10 * DAY, refresh_token_expires_at_unix_seconds: NOW + 170 * DAY,
    device_id: "249085123408502", display_name: null, masked_mobile: "138****0000", mobile_full: null,
    auto_checkin_enabled: true, refresh_error_code: null, credential_legacy: false,
    last_attempt_outcome: null, last_attempt_date: null, ...overrides,
  };
}

/** 需处理行：旧通道凭据（credential_legacy）→ 登录槽「需重登」、签到槽「未签」。 */
const attentionEntry = () => traeEntry("p1", "Beta 账号", { credential_legacy: true, checked_in: false });
/** 健康行：登录正常 + 已签，不进「需处理」档。 */
const healthyEntry = () => traeEntry("p2", "Alpha 账号");
/** 存档面（fixture 回退）空表：真实面行数据只来自总览。 */
const MANAGED_EMPTY = {
  saved_accounts: [], switch_state: null, handoff_intent: null, history_is_separate: true,
  current_account: { profile_id: null, display_name: null, region: null, data_location_id: null, verification_state: "unknown", observed_at: null, reason: null },
  recent_verification: { verification_state: "unknown", checked_at: null, reason: null },
};

/** TRAE 读取路由：总览两行 + 实测当前账号 p1 + 逐账号登录态（p1 旧通道 stale、p2 正常）。 */
function installTraeRoutes(options: { overview?: readonly CheckinOverviewEntryDto[]; observed?: string | null } = {}): void {
  mockRoute("get_checkin_capability", () => REAL_CAPABILITY);
  mockRoute("get_checkin_overview", () => options.overview ?? [attentionEntry(), healthyEntry()]);
  mockRoute("get_environment_state", () => ({
    env_id: "master", current_profile_id: null, current_account_name: null, running: false,
    observed_profile_id: options.observed ?? "p1", data_dir: "C:\\data\\master",
    login_state: "logged_in", created_at_unix_seconds: 1750000000 }));
  mockRoute("get_managed_account_state", () => MANAGED_EMPTY);
  mockRoute("get_trae_instance_states", (args: { profileIds: readonly string[] }) => args.profileIds.map((profileId) => ({
    profile_id: profileId, login_state: profileId === "p1" ? "stale" : "logged_in", archive_available: true,
  })));
}

/** WB 池条目构造器：默认凭据有效、备注名空、未归档。 */
function wbAccount(profileId: string, name: string, overrides: Partial<WorkBuddyAccountEntryDto> = {}): WorkBuddyAccountEntryDto {
  return {
    profile_id: profileId, account_id: `acct-${profileId}`, screen_name: name, domain: "workbuddy.example",
    access_token_expires_at_unix_seconds: NOW + 5 * DAY, refresh_token_expires_at_unix_seconds: NOW + 50 * DAY,
    minted_at_unix_seconds: NOW - DAY, last_updated_at_unix_seconds: NOW - 3600, display_name: "",
    archived: false, masked_phone: "", ...overrides,
  };
}

/** WB 读取路由：会话健康（当前登录 = wb1）+ 池两行 + 逐账号余额（真实值 / 整行失败）。 */
function installWorkbuddyRoutes(): void {
  mockRoute("get_workbuddy_session_health", () => ({
    status: "active", uid: "acct-wb1", uin: "10001", domain: "workbuddy.example", captured_in_pool: true,
    access_expires_at_unix_seconds: NOW + 10 * DAY, refresh_expires_at_unix_seconds: NOW + 100 * DAY,
  }));
  mockRoute("list_workbuddy_accounts", () => [wbAccount("wb1", "WB 账号甲"), wbAccount("wb2", "备用号")]);
  mockRoute("get_workbuddy_checkin_overview", () => [
    { profile_id: "wb1", screen_name: "WB 账号甲", today_checked_in: true, streak_days: 3, daily_credit: 200,
      total_credits: 600, credits_balance: 1234, credits_total: 2000, balance_error: null,
      checkin_dates: ["2026-10-22"], masked_phone: "133****1304", error: null },
    { profile_id: "wb2", screen_name: "备用号", today_checked_in: null, streak_days: null, daily_credit: null,
      total_credits: null, credits_balance: null, credits_total: null, balance_error: null,
      checkin_dates: [], masked_phone: null, error: "workbuddy_checkin_runtime_error" }]);
}

/** 列表行锚点序列（DOM 顺序即渲染顺序）：行 testid 全部由 builder + 接缝产出。 */
const rowIds = (): (string | null)[] =>
  screen.queryAllByTestId(/^account-card-/).map((element) => element.getAttribute("data-testid"));

beforeEach(() => {
  resetRoutes();
  installFakeInvoke();
  // 通知替身跨用例清零（mockClear 不清实现，installFakeInvoke 的分发实现不受影响）。
  vi.clearAllMocks();
});

describe("账号页页面级集成（P4c-8b：渲染 + 交互）", () => {
  // 真实总览面：两行按总览顺序渲染，行锚点与「使用中」隐藏锚点逐字沿用旧 testid。
  it("TRAE 真实面：渲染行、行锚点与当前账号锚点，计数取账号总数", async () => {
    installTraeRoutes();
    render(<AccountsPage active={true} productId="trae_cn" />);
    await screen.findByTestId("account-card-p1");
    expect(rowIds()).toEqual(["account-card-p1", "account-card-p2"]);
    // 当前账号 = 实测 observed_profile_id：隐藏锚点承载旧 testid（视觉由 accent 徽章表达）。
    expect(screen.getByTestId("account-current-p1")).toBeInTheDocument();
    expect(screen.queryByTestId("account-current-p2")).not.toBeInTheDocument();
    expect(screen.getByText("2 个")).toBeInTheDocument();
    // 行文案与徽章：需处理行「需重登」、健康行「正常」。
    expect(screen.getByTestId("account-card-p1").textContent).toContain("需重登");
    expect(screen.getByTestId("account-card-p2").textContent).toContain("正常");
  });

  // 过滤档：点「需处理」后只剩需处理行，计数仍是账号总数（不随可见行缩水）。
  it("TRAE 过滤档：切到需处理只剩需处理行，计数不变", async () => {
    installTraeRoutes();
    render(<AccountsPage active={true} productId="trae_cn" />);
    await screen.findByTestId("account-card-p1");
    fireEvent.click(screen.getByTestId("account-filter-attention"));
    await waitFor(() => expect(rowIds()).toEqual(["account-card-p1"]));
    expect(screen.getByText("2 个")).toBeInTheDocument();
    // 切回「全部」即恢复两行（过滤档是会话内状态，不丢数据）。
    fireEvent.click(screen.getByTestId("account-filter-all"));
    await waitFor(() => expect(rowIds()).toEqual(["account-card-p1", "account-card-p2"]));
  });

  // 排序档：切「名称」按展示名重排（added = 总览原序），行集合不变。
  it("TRAE 排序档：切名称改变行序", async () => {
    installTraeRoutes();
    render(<AccountsPage active={true} productId="trae_cn" />);
    await screen.findByTestId("account-card-p1");
    expect(rowIds()).toEqual(["account-card-p1", "account-card-p2"]);
    fireEvent.change(screen.getByTestId("account-sort"), { target: { value: "name" } });
    await waitFor(() => expect(rowIds()).toEqual(["account-card-p2", "account-card-p1"]));
  });

  // 首次读取失败：只给错误卡（带原因），绝不用空账号列表冒充读取失败。
  it("读取失败：显示错误卡，不渲染空账号列表", async () => {
    installTraeRoutes();
    mockRoute("get_checkin_overview", () => Promise.reject(new Error("boom")));
    render(<AccountsPage active={true} productId="trae_cn" />);
    const card = await screen.findByTestId("account-center__error");
    expect(card.textContent).toContain("账号信息读取失败");
    expect(card.textContent).toContain("账号信息暂时不可读取");
    expect(rowIds()).toEqual([]);
    expect(screen.queryByTestId("account-center__empty")).not.toBeInTheDocument();
  });

  // 错误卡「重新读取」= 纯本地刷新：再次发同一批读取命令，而不是静默重试。
  it("错误卡重试：点重新读取会再次发命令", async () => {
    installTraeRoutes();
    mockRoute("get_checkin_overview", () => Promise.reject(new Error("boom")));
    render(<AccountsPage active={true} productId="trae_cn" />);
    await screen.findByTestId("account-center__error");
    const overviewCalls = () => mockCoreInvoke.mock.calls.filter(([cmd]) => cmd === "get_checkin_overview").length;
    expect(overviewCalls()).toBe(1);
    fireEvent.click(screen.getByRole("button", { name: "重新读取" }));
    await waitFor(() => expect(overviewCalls()).toBe(2));
  });

  // 已有数据时刷新失败：保留旧数据继续渲染 + notify.error 说明，不清空列表、不退回错误卡。
  it("刷新失败：保留旧数据并触发 notify.error", async () => {
    installTraeRoutes();
    render(<AccountsPage active={true} productId="trae_cn" />);
    await screen.findByTestId("account-card-p1");

    mockRoute("get_checkin_overview", () => Promise.reject(new Error("boom")));
    fireEvent.click(screen.getByTestId("account-refresh-local"));
    await waitFor(() => expect(vi.mocked(notify.error)).toHaveBeenCalledWith(
      expect.any(String), expect.stringContaining("正在显示上次读取到的账号信息"),
    ));
    expect(screen.queryByTestId("account-center__error")).not.toBeInTheDocument();
    expect(rowIds()).toEqual(["account-card-p1", "account-card-p2"]);
  });

  // 行点击进详情：详情页 region 与返回锚点（account-detail-back）在册，行 id 透出观察者。
  it("行点击进详情：account-detail-back 存在", async () => {
    installTraeRoutes();
    const opened: string[] = [];
    render(<AccountsPage active={true} productId="trae_cn" onOpenDetail={(rowId) => opened.push(rowId)} />);
    fireEvent.click(await screen.findByTestId("account-card-p2"));

    expect(await screen.findByTestId("account-detail-back")).toBeInTheDocument();
    expect(screen.getByRole("region", { name: "账号详情" })).toBeInTheDocument();
    expect(opened).toEqual(["p2"]);
  });

  // 详情返回列表：返回后详情页消失、两行原样恢复。
  it("详情返回：回到账号列表", async () => {
    installTraeRoutes();
    render(<AccountsPage active={true} productId="trae_cn" />);
    fireEvent.click(await screen.findByTestId("account-card-p2"));
    fireEvent.click(await screen.findByTestId("account-detail-back"));

    await waitFor(() => expect(screen.queryByTestId("account-detail-back")).not.toBeInTheDocument());
    expect(rowIds()).toEqual(["account-card-p1", "account-card-p2"]);
  });

  // WB 池面：行锚点 workbuddy-pool-card-*、列表容器沿用 workbuddy-pool-list、「使用中」隐藏锚点。
  it("WB 池面：渲染 workbuddy-pool-card-* 与列表容器", async () => {
    installWorkbuddyRoutes();
    render(<AccountsPage active={true} productId="workbuddy" />);

    await screen.findByTestId("workbuddy-pool-card-wb1");
    expect(screen.getByTestId("workbuddy-pool-list")).toBeInTheDocument();
    expect(screen.getByTestId("workbuddy-pool-card-wb2")).toBeInTheDocument();
    // 当前客户端登录账号（health.uid 与池 account_id 同源）给隐藏锚点。
    expect(screen.getByTestId("workbuddy-pool-current-wb1")).toBeInTheDocument();
  });

  // WB 无过滤档（attentionFilter=false）：不给过滤按钮；排序档只剩「添加顺序 / 名称」。
  it("WB 池面：无过滤档，排序档仅添加顺序与名称", async () => {
    installWorkbuddyRoutes();
    render(<AccountsPage active={true} productId="workbuddy" />);
    await screen.findByTestId("workbuddy-pool-card-wb1");

    expect(screen.queryByTestId("account-filter-all")).not.toBeInTheDocument();
    expect(screen.queryByTestId("account-filter-attention")).not.toBeInTheDocument();
    const sort = screen.getByTestId("account-sort") as HTMLSelectElement;
    expect(Array.from(sort.options).map((option) => option.value)).toEqual(["added", "name"]);
  });

  // WB 余额四态取两态：真实数值照实显示、整行读取失败标「查询失败」（不静默成 0）。
  it("WB 池面：余额真实数值与查询失败两态", async () => {
    installWorkbuddyRoutes();
    render(<AccountsPage active={true} productId="workbuddy" />);

    const paid = await screen.findByTestId("workbuddy-pool-card-wb1");
    expect(paid.textContent).toContain("积分余额");
    expect(paid.textContent).toContain("1234");
    const failed = screen.getByTestId("workbuddy-pool-card-wb2");
    expect(failed.textContent).toContain("查询失败");
    expect(failed.textContent).not.toMatch(/积分余额\s*0/);
  });

  // 空态：零账号给空态文案 + 主行动；主行动透出页头「添加账号」动作 id（不就地发明动作）。
  it("空态：零账号显示空态与主行动", async () => {
    installTraeRoutes({ overview: [] });
    const actions: string[] = [];
    render(<AccountsPage active={true} productId="trae_cn" onHeaderAction={(actionId) => actions.push(actionId)} />);

    const empty = await screen.findByTestId("account-center__empty-actions");
    expect(empty.textContent).toContain("还没有账号");
    expect(screen.getByText("0 个")).toBeInTheDocument();
    expect(rowIds()).toEqual([]);
    fireEvent.click(within(empty).getByRole("button"));
    expect(actions).toEqual(["account-add-primary"]);
  });

  // WB 空态现状：空态卡在册但 builder 的 actionLabel=null，故不给「添加账号」按钮
  //（主行动只在页头 workbuddy-add-account-button）；此处断言现状，不为通过而改 src/。
  it("WB 空态：空态卡在册但卡内无主行动（现状）", async () => {
    installWorkbuddyRoutes();
    mockRoute("list_workbuddy_accounts", () => []);
    render(<AccountsPage active={true} productId="workbuddy" />);

    const empty = await screen.findByTestId("workbuddy-page__empty");
    expect(empty.textContent).toContain("还没有保存 WorkBuddy 账号");
    expect(within(empty).queryByRole("button")).not.toBeInTheDocument();
    // 主行动改由页头承载（不受空态影响）。
    expect(screen.getByTestId("workbuddy-add-account-button")).toBeInTheDocument();
  });

  // 页面不可见不发 IPC：后台页保持挂载但不读取，只渲染读取中外壳。
  it("页面不可见：不发读取命令", async () => {
    installTraeRoutes();
    render(<AccountsPage active={false} productId="trae_cn" />);

    expect(await screen.findByText("正在读取账号信息…")).toBeInTheDocument();
    expect(mockCoreInvoke).not.toHaveBeenCalled();
  });
});
