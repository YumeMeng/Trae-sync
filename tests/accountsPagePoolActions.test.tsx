import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { installFakeInvoke, mockCoreInvoke, mockRoute, resetRoutes } from "./helpers/fakeInvoke";

// 共享 fakeInvoke 路由器（用法见 tests/helpers/fakeInvoke.ts 头注释）。
vi.mock("@tauri-apps/api/core", async () => {
  const fake = await import("./helpers/fakeInvoke");
  return { invoke: fake.mockCoreInvoke };
});
// 批量进度事件订阅：jsdom 无 Tauri IPC，listen 给空退订函数即可。
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn().mockResolvedValue(() => undefined) }));
vi.mock("../src/platform/notify", () => ({
  notify: { success: vi.fn(), error: vi.fn(), warning: vi.fn(), info: vi.fn(), summary: vi.fn() },
}));

import { notify } from "../src/platform/notify";
import { AccountsPage } from "../src/features/accounts/AccountsPage";

/**
 * WB 页头池级四动作（P4c-9b-1 / K-04）：刷新登录凭据 / 健康检测 / 保存当前登录账号 /
 * 添加账号。旧实现见 WorkBuddyPanel L172-342；本测试只断言接线（命令 + 单条回执呈现 +
 * 刷新时机），文案按旧实现逐字核对。
 */

const NOW = Math.floor(Date.now() / 1000);
const DAY = 86400;

/** WB 池条目（凭据有效、未归档、无备注名）。 */
const wbAccount = (profileId: string, name: string) => ({
  profile_id: profileId, account_id: `acct-${profileId}`, screen_name: name, domain: "workbuddy.example",
  access_token_expires_at_unix_seconds: NOW + 5 * DAY, refresh_token_expires_at_unix_seconds: NOW + 50 * DAY,
  minted_at_unix_seconds: NOW - DAY, last_updated_at_unix_seconds: NOW - 3600, display_name: "",
  archived: false, masked_phone: "",
});

/** 基础读取路由（会话健康 active + 池一行 + 逐账号余额）。 */
function installBaseRoutes(): void {
  mockRoute("get_workbuddy_session_health", () => ({
    status: "active", uid: "acct-wb1", uin: "10001", domain: "workbuddy.example", captured_in_pool: true,
    access_expires_at_unix_seconds: NOW + 10 * DAY, refresh_expires_at_unix_seconds: NOW + 100 * DAY,
  }));
  mockRoute("list_workbuddy_accounts", () => [wbAccount("wb1", "WB 账号甲")]);
  mockRoute("get_workbuddy_checkin_overview", () => [{
    profile_id: "wb1", screen_name: "WB 账号甲", today_checked_in: true, streak_days: 3, daily_credit: 200,
    total_credits: 600, credits_balance: 1234, credits_total: 2000, balance_error: null,
    checkin_dates: ["2026-10-22"], masked_phone: "", error: null,
  }]);
  mockRoute("list_workbuddy_checkin_accounts", () => []);
}

/** 某命令的调用次数（invoke 首参即命令名）。 */
const callsOf = (cmd: string): number => mockCoreInvoke.mock.calls.filter((call) => call[0] === cmd).length;

beforeEach(() => {
  resetRoutes();
  installFakeInvoke();
  vi.mocked(notify.success).mockClear();
  vi.mocked(notify.error).mockClear();
  vi.mocked(notify.info).mockClear();
  vi.mocked(notify.warning).mockClear();
});

describe("WB 页头池级四动作（P4c-9b-1 / K-04）", () => {
  // 刷新登录凭据（旧 L324-342）：无弹层；renewed=true 成功回执带有效期；成功后重读缓存。
  it("刷新登录凭据：命令发出、成功回执带有效期、刷新账号池", async () => {
    installBaseRoutes();
    mockRoute("refresh_workbuddy_account", () => ({ renewed: true, access_expires_at_unix_seconds: NOW + 9 * DAY, refresh_expires_at_unix_seconds: NOW + 90 * DAY }));
    render(<AccountsPage active={true} productId="workbuddy" />);
    const listBefore = callsOf("list_workbuddy_accounts");

    fireEvent.click(await screen.findByTestId("workbuddy-renew-credentials"));
    await waitFor(() => expect(notify.success).toHaveBeenCalled());
    expect(callsOf("refresh_workbuddy_account")).toBe(1);
    expect(vi.mocked(notify.success).mock.calls[0][0]).toContain("登录凭据已更新，有效期至");
    // 刷新时机：成功后重读本机缓存（旧 refresh(false) 同法）。
    await waitFor(() => expect(callsOf("list_workbuddy_accounts")).toBeGreaterThan(listBefore));
  });

  // renewed=false = 凭据仍充裕、本次未轮换：旧实现走中性提示，不能谎报成成功。
  it("刷新登录凭据：仍充裕时走中性提示（不谎报成功）", async () => {
    installBaseRoutes();
    mockRoute("refresh_workbuddy_account", () => ({ renewed: false, access_expires_at_unix_seconds: NOW + 30 * DAY, refresh_expires_at_unix_seconds: NOW + 90 * DAY }));
    render(<AccountsPage active={true} productId="workbuddy" />);

    fireEvent.click(await screen.findByTestId("workbuddy-renew-credentials"));
    await waitFor(() => expect(notify.info).toHaveBeenCalled());
    expect(vi.mocked(notify.info).mock.calls[0][0]).toContain("登录凭据仍充裕");
    expect(notify.success).not.toHaveBeenCalled();
  });

  // 深度健康（旧 L172-191）：池级单条回执，按 live_status 三档呈现，且不拆成逐账号结果。
  it("健康检测：verified 走成功回执、只发一条通知、不重读数据", async () => {
    installBaseRoutes();
    mockRoute("check_workbuddy_login_health", () => ({ status: "active", live_status: "verified" }));
    render(<AccountsPage active={true} productId="workbuddy" />);
    const listBefore = callsOf("list_workbuddy_accounts");

    fireEvent.click(await screen.findByTestId("workbuddy-deep-health-button"));
    await waitFor(() => expect(notify.success).toHaveBeenCalled());
    expect(callsOf("check_workbuddy_login_health")).toBe(1);
    expect(vi.mocked(notify.success).mock.calls[0][0]).toContain("健康检测通过");
    // 单条回执：不走逐账号摘要（notify.summary 属逐账号动作）。
    expect(notify.summary).not.toHaveBeenCalled();
    // 只读零写：健康检测后不重读数据面（旧实现结果常驻健康卡）。
    expect(callsOf("list_workbuddy_accounts")).toBe(listBefore);
  });

  it("健康检测：probe_failed 走失败回执、client_offline 走中性回执", async () => {
    installBaseRoutes();
    mockRoute("check_workbuddy_login_health", () => ({ status: "active", live_status: "probe_failed" }));
    render(<AccountsPage active={true} productId="workbuddy" />);

    fireEvent.click(await screen.findByTestId("workbuddy-deep-health-button"));
    await waitFor(() => expect(notify.error).toHaveBeenCalled());
    expect(vi.mocked(notify.error).mock.calls[0][0]).toContain("健康检测发现异常");
  });

  it("健康检测：client_offline 走中性提示（客户端未运行不算异常）", async () => {
    installBaseRoutes();
    mockRoute("check_workbuddy_login_health", () => ({ status: "active", live_status: "client_offline" }));
    render(<AccountsPage active={true} productId="workbuddy" />);

    fireEvent.click(await screen.findByTestId("workbuddy-deep-health-button"));
    await waitFor(() => expect(notify.info).toHaveBeenCalled());
    expect(vi.mocked(notify.info).mock.calls[0][0]).toContain("客户端未运行");
    expect(notify.error).not.toHaveBeenCalled();
  });

  // 保存当前登录账号（旧 L198-213 + L574-589）：先弹单次确认（testid 逐字），确认才发命令。
  it("保存当前登录账号：先弹确认层（workbuddy-capture-dialog），确认后保存并重读", async () => {
    installBaseRoutes();
    mockRoute("capture_workbuddy_account", () => ({ profile_id: "wb1", account_id: "acct-wb1", screen_name: "WB 账号甲", domain: "workbuddy.example", access_token_expires_at_unix_seconds: NOW + 5 * DAY, refresh_token_expires_at_unix_seconds: NOW + 50 * DAY, minted_at_unix_seconds: NOW, last_updated_at_unix_seconds: NOW }));
    render(<AccountsPage active={true} productId="workbuddy" />);

    fireEvent.click(await screen.findByTestId("workbuddy-capture-button"));
    const dialog = await screen.findByTestId("workbuddy-capture-dialog");
    expect(dialog.textContent).toContain("将从本机 WorkBuddy 客户端读取当前登录会话");
    // 弹层未确认前不发命令（单次确认纪律）。
    expect(callsOf("capture_workbuddy_account")).toBe(0);

    fireEvent.click(screen.getByTestId("workbuddy-capture-dialog-confirm"));
    await waitFor(() => expect(notify.success).toHaveBeenCalled());
    expect(callsOf("capture_workbuddy_account")).toBe(1);
    expect(vi.mocked(notify.success).mock.calls[0][0]).toContain("已保存账号「WB 账号甲」到账号池");
    await waitFor(() => expect(screen.queryByTestId("workbuddy-capture-dialog")).not.toBeInTheDocument());
  });

  // 添加账号（旧 L288-312）：begin（自动拉起浏览器）→ complete（等授权）→ 成功入池刷新。
  it("添加账号：begin + complete 后成功回执并重读", async () => {
    installBaseRoutes();
    mockRoute("begin_workbuddy_login", () => ({ auth_url: "https://workbuddy.example/auth", attempt_id: 7 }));
    mockRoute("complete_workbuddy_login", () => ({ profile_id: "wb2", account_id: "acct-wb2", screen_name: "新账号", acceptance: "passed" }));
    render(<AccountsPage active={true} productId="workbuddy" />);
    const listBefore = callsOf("list_workbuddy_accounts");

    fireEvent.click(await screen.findByTestId("workbuddy-add-account-button"));
    await waitFor(() => expect(notify.success).toHaveBeenCalled());
    expect(callsOf("begin_workbuddy_login")).toBe(1);
    expect(callsOf("complete_workbuddy_login")).toBe(1);
    expect(vi.mocked(notify.success).mock.calls[0][0]).toContain("账号「新账号」登录成功，已保存到账号池");
    await waitFor(() => expect(callsOf("list_workbuddy_accounts")).toBeGreaterThan(listBefore));
  });

  // acceptance=failed 只警告、不回滚、不自动重试（旧 L296-301）。
  it("添加账号：验收未通过只警告，不重试", async () => {
    installBaseRoutes();
    mockRoute("begin_workbuddy_login", () => ({ auth_url: "https://workbuddy.example/auth", attempt_id: 8 }));
    mockRoute("complete_workbuddy_login", () => ({ profile_id: "wb2", account_id: "acct-wb2", screen_name: "新账号", acceptance: "failed", acceptance_error_code: "credential_refresh_failed" }));
    render(<AccountsPage active={true} productId="workbuddy" />);

    fireEvent.click(await screen.findByTestId("workbuddy-add-account-button"));
    await waitFor(() => expect(notify.warning).toHaveBeenCalled());
    expect(vi.mocked(notify.warning).mock.calls[0][0]).toContain("可持续性验收未通过");
    // 不发起第二次授权。
    expect(callsOf("begin_workbuddy_login")).toBe(1);
  });

  // 取消不是错误：complete 以 workbuddy_login_cancelled 返回 → 中性提示（旧 L304-305）。
  it("添加账号：取消走中性提示（不报错）", async () => {
    installBaseRoutes();
    mockRoute("begin_workbuddy_login", () => ({ auth_url: "https://workbuddy.example/auth", attempt_id: 9 }));
    mockRoute("complete_workbuddy_login", () => { throw "workbuddy_login_cancelled"; });
    render(<AccountsPage active={true} productId="workbuddy" />);

    fireEvent.click(await screen.findByTestId("workbuddy-add-account-button"));
    await waitFor(() => expect(notify.info).toHaveBeenCalled());
    expect(vi.mocked(notify.info).mock.calls[0][0]).toContain("登录已取消");
    expect(notify.error).not.toHaveBeenCalled();
  });

  // 取消等待（旧 L315-318）：等待授权中出现 workbuddy-login-cancel，点击即发 cancel，
  // 阻塞中的 complete 以取消码收尾（中性提示），按钮态随收尾复位。
  it("添加账号：等待中可取消等待，取消后按钮复位", async () => {
    installBaseRoutes();
    mockRoute("begin_workbuddy_login", () => ({ auth_url: "https://workbuddy.example/auth", attempt_id: 11 }));
    mockRoute("cancel_workbuddy_login", () => null);
    // complete 挂起：直到测试主动让它以取消码收尾。
    const gateControl: { release: () => void } = { release: () => undefined };
    const gate = new Promise<void>((resolve) => { gateControl.release = resolve; });
    mockRoute("complete_workbuddy_login", async () => { await gate; throw "workbuddy_login_cancelled"; });
    render(<AccountsPage active={true} productId="workbuddy" />);

    fireEvent.click(await screen.findByTestId("workbuddy-add-account-button"));
    // 等待授权中：按钮换忙碌文案，并出现「取消等待」。
    await waitFor(() => expect(screen.getByTestId("workbuddy-add-account-button")).toHaveTextContent("等待浏览器登录完成…"));
    fireEvent.click(screen.getByTestId("workbuddy-login-cancel"));
    await waitFor(() => expect(callsOf("cancel_workbuddy_login")).toBe(1));
    gateControl.release();
    await waitFor(() => expect(notify.info).toHaveBeenCalled());
    // 收尾后按钮复位、取消入口消失（登录单槽清空）。
    await waitFor(() => expect(screen.queryByTestId("workbuddy-login-cancel")).not.toBeInTheDocument());
    expect(screen.getByTestId("workbuddy-add-account-button")).toHaveTextContent("添加账号");
  });

  // 命令失败必须可见：人话原因经 safeUiError 词表（不把原始错误码搬上界面）。
  it("刷新登录凭据失败：错误提示带 WB 前缀与人话原因", async () => {
    installBaseRoutes();
    mockRoute("refresh_workbuddy_account", () => { throw "workbuddy_refresh_rejected"; });
    render(<AccountsPage active={true} productId="workbuddy" />);

    fireEvent.click(await screen.findByTestId("workbuddy-renew-credentials"));
    await waitFor(() => expect(notify.error).toHaveBeenCalled());
    expect(vi.mocked(notify.error).mock.calls[0][0]).toContain("WorkBuddy：刷新登录凭据失败——");
    expect(vi.mocked(notify.error).mock.calls[0][0]).toContain("账号池中的登录凭据已失效");
    expect(vi.mocked(notify.error).mock.calls[0][0]).not.toContain("workbuddy_refresh_rejected");
  });
});
