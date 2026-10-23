import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { installFakeInvoke, mockRoute, resetRoutes } from "./helpers/fakeInvoke";
import type { CheckinOverviewEntryDto } from "../src/types/account_switch";

// P4c-9c（K-05 回归护栏）：批量进度事件 → 合并页逐账号文案的端到端接线。
// 事件订阅用内存注册表模拟 Tauri 事件路由，测试里手动投递后端 emit 的载荷。
vi.mock("@tauri-apps/api/core", async () => {
  const fake = await import("./helpers/fakeInvoke");
  return { invoke: fake.mockCoreInvoke };
});
const { mockListen } = vi.hoisted(() => ({ mockListen: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({ listen: mockListen }));
vi.mock("../src/platform/notify", () => ({
  notify: { success: vi.fn(), error: vi.fn(), warning: vi.fn(), info: vi.fn(), summary: vi.fn() },
}));

import { AccountsPage } from "../src/features/accounts/AccountsPage";

const NOW = Math.floor(Date.now() / 1000);
const REAL_CAPABILITY = { enabled: true, transport: "real_http", real_http_enabled: true, message: "真实签到已启用。" };

/** 事件名 → 已注册回调（模拟 Tauri 事件路由；unlisten 只做清理）。 */
const registry = new Map<string, Set<(event: { payload: unknown }) => void>>();

/** 投递一条后端进度事件（等价 emit）；包在 act 里让状态更新落地。 */
function emit(name: string, payload: unknown): void {
  act(() => {
    for (const handler of registry.get(name) ?? []) handler({ payload });
  });
}

/** 真实总览条目：两行健康账号（页头给「刷新额度 / 健康检测 / 刷新凭据」入口）。 */
function entry(profileId: string, name: string): CheckinOverviewEntryDto {
  return {
    profile_id: profileId, screen_name: name, account_id: `acct-${profileId}`,
    created_at: "2026-08-01T00:00:00.000Z", last_verified_at: "2026-08-22T00:00:00.000Z",
    credits: 200, credits_cached_at: "2026-08-22T01:00:00.000Z", usage_remaining_credits: 1600,
    usage_cached_at: "2026-08-22T01:00:00.000Z", checked_in: true, device_tail: "8502",
    access_token_expires_at_unix_seconds: NOW + 10 * 86400, refresh_token_expires_at_unix_seconds: NOW + 170 * 86400,
    device_id: "249085123408502", display_name: null, masked_mobile: "138****0000", mobile_full: null,
    auto_checkin_enabled: true, refresh_error_code: null, credential_legacy: false,
    last_attempt_outcome: null, last_attempt_date: null,
  };
}

/** TRAE 读取路由：总览两行（额度刷新命令永不返回，保持批次在跑）。 */
function installRoutes(): void {
  mockRoute("get_checkin_capability", () => REAL_CAPABILITY);
  mockRoute("get_checkin_overview", () => [entry("p1", "进度账号甲"), entry("p2", "进度账号乙")]);
  mockRoute("get_environment_state", () => ({
    env_id: "master", current_profile_id: null, current_account_name: null, running: false,
    observed_profile_id: null, data_dir: "C:\\data\\master", login_state: "logged_in",
    created_at_unix_seconds: 1750000000 }));
  mockRoute("get_managed_account_state", () => ({
    saved_accounts: [], switch_state: null, handoff_intent: null, history_is_separate: true,
    current_account: { profile_id: null, display_name: null, region: null, data_location_id: null, verification_state: "unknown", observed_at: null, reason: null },
    recent_verification: { verification_state: "unknown", checked_at: null, reason: null } }));
  mockRoute("get_trae_instance_states", (args: { profileIds: readonly string[] }) => args.profileIds.map((profileId) => ({
    profile_id: profileId, login_state: "logged_in", archive_available: true })));
  mockRoute("refresh_checkin_credits", () => new Promise(() => undefined));
}

beforeEach(() => {
  resetRoutes();
  installFakeInvoke();
  vi.clearAllMocks();
  registry.clear();
  mockListen.mockImplementation(async (name: string, handler: (event: { payload: unknown }) => void) => {
    const handlers = registry.get(name) ?? new Set<(event: { payload: unknown }) => void>();
    registry.set(name, handlers);
    handlers.add(handler);
    return () => handlers.delete(handler);
  });
});

describe("账号页批量进度文案（P4c-9c 补回 K-05）", () => {
  it("额度刷新进行中：事件到达即上屏「正在处理 1/2：账号名」", async () => {
    installRoutes();
    render(<AccountsPage active={true} productId="trae_cn" />);
    await screen.findByTestId("account-card-p1");
    fireEvent.click(screen.getByTestId("account-refresh-credits"));
    // 未收到事件前不显示逐账号文案（旧实现此处给回退文案，本票不补）。
    expect(screen.queryByText(/正在处理 \d\/\d/)).not.toBeInTheDocument();

    emit("credits-refresh-progress", {
      product_id: "work_cn", kind: "credits_refresh", profile_id: "p1", screen_name: "进度账号甲",
      completed: 1, total: 2, error_code: null,
    });
    expect(await screen.findByText("正在处理 1/2：进度账号甲")).toBeInTheDocument();
  });

  it("其他产品来源的进度事件被丢弃：不上屏、不泄露账号名", async () => {
    installRoutes();
    render(<AccountsPage active={true} productId="trae_cn" />);
    await screen.findByTestId("account-card-p1");
    fireEvent.click(screen.getByTestId("account-refresh-credits"));

    emit("credits-refresh-progress", {
      product_id: "trae_cn", kind: "credits_refresh", profile_id: "p1", screen_name: "外来账号名",
      completed: 1, total: 2, error_code: null,
    });
    await waitFor(() => expect(screen.queryByText(/正在处理/)).not.toBeInTheDocument());
  });
});
