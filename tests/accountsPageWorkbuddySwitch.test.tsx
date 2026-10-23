import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { installFakeInvoke, mockCoreInvoke, mockRoute, resetRoutes } from "./helpers/fakeInvoke";

/**
 * WB 行内切号两段式（P4c-9b-2 补覆盖）：旧 tests/WorkBuddyPanel.test.tsx 的 3 例
 * （preview 弹窗 → confirmed switch / 部分成功 / preview 失败不开弹窗）随旧页删除后，
 * 合并页这条路径此前**零渲染层断言**。切号会写客户端登录态，是本票删除范围内
 * 风险最高的未覆盖行为，故按旧断言逐条搬到合并页（文案与命令契约逐字不变）。
 */

vi.mock("@tauri-apps/api/core", async () => {
  const fake = await import("./helpers/fakeInvoke");
  return { invoke: fake.mockCoreInvoke };
});
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn().mockResolvedValue(() => undefined) }));
vi.mock("../src/platform/notify", () => ({
  notify: { success: vi.fn(), error: vi.fn(), warning: vi.fn(), info: vi.fn(), summary: vi.fn() },
}));

import { notify } from "../src/platform/notify";
import { AccountsPage } from "../src/features/accounts/AccountsPage";

const NOW = Math.floor(Date.now() / 1000);
const DAY = 86400;

/** WB 池条目：甲账号为当前登录（health.uid 命中），乙账号为切换目标。 */
const pool = [
  {
    profile_id: "wb-uid-1", account_id: "uid-1", screen_name: "甲账号", domain: "workbuddy.example",
    access_token_expires_at_unix_seconds: NOW + 5 * DAY, refresh_token_expires_at_unix_seconds: NOW + 50 * DAY,
    minted_at_unix_seconds: NOW - DAY, last_updated_at_unix_seconds: NOW - 3600,
    display_name: "", archived: false, masked_phone: "",
  },
  {
    profile_id: "wb-uid-2", account_id: "uid-2", screen_name: "乙账号", domain: "workbuddy.example",
    access_token_expires_at_unix_seconds: NOW + 5 * DAY, refresh_token_expires_at_unix_seconds: NOW + 50 * DAY,
    minted_at_unix_seconds: NOW - DAY, last_updated_at_unix_seconds: NOW - 3600,
    display_name: "", archived: false, masked_phone: "",
  },
];

function installBaseRoutes(): void {
  mockRoute("get_workbuddy_session_health", () => ({
    status: "active", uid: "uid-1", uin: "10001", domain: "workbuddy.example", captured_in_pool: true,
    access_expires_at_unix_seconds: NOW + 10 * DAY, refresh_expires_at_unix_seconds: NOW + 100 * DAY,
  }));
  mockRoute("list_workbuddy_accounts", () => pool);
  mockRoute("get_workbuddy_checkin_overview", () => []);
  mockRoute("list_workbuddy_checkin_accounts", () => []);
  mockRoute("preview_workbuddy_switch", () => ({
    current_label: "甲账号", target_label: "乙账号", pending_local_rows: 12, pending_project_count: 3,
    current_account_rows: 40, cloud_residual_rows: 0,
  }));
  mockRoute("switch_workbuddy_account", () => ({
    switched: true, target_label: "乙账号", previous_label: "甲账号", transfer_executed: true,
    updated_rows: 12, verified_sessions: 12, missing_body_files: 0, cloud_residual_rows: 0,
    transfer_error: null, restarted: true, post_status: "active",
  }));
}

const callsOf = (cmd: string): number => mockCoreInvoke.mock.calls.filter((call) => call[0] === cmd).length;

beforeEach(() => {
  resetRoutes();
  installFakeInvoke();
  vi.clearAllMocks();
});

describe("WB 行内切号两段式（P4c-9b-2：旧 WorkBuddyPanel 3 例迁到合并页）", () => {
  // 旧 L692-722：选账号 → preview（只读零副作用）→ 影响面弹窗 → 确认才写客户端登录态。
  it("选账号 → preview 影响面弹窗 → 确认后 switch(accountId, confirmed=true)", async () => {
    installBaseRoutes();
    render(<AccountsPage active={true} productId="workbuddy" />);

    fireEvent.click(await screen.findByTestId("workbuddy-switch-wb-uid-2"));
    const dialog = await screen.findByTestId("workbuddy-switch-dialog");
    expect(dialog.textContent).toContain("「甲账号」");
    expect(dialog.textContent).toContain("「乙账号」");
    expect(dialog.textContent).toContain("12 条本地会话（3 个项目）");
    // preview 以 account_id 调用（be 契约：targetAccountId = 池行 account_id）。
    expect(mockCoreInvoke.mock.calls).toContainEqual(
      ["preview_workbuddy_switch", { targetAccountId: "uid-2" }],
    );
    // 单次确认纪律：弹窗未确认前不得写客户端登录态。
    expect(callsOf("switch_workbuddy_account")).toBe(0);

    fireEvent.click(screen.getByTestId("workbuddy-switch-dialog-confirm"));
    await waitFor(() => {
      expect(mockCoreInvoke.mock.calls).toContainEqual(
        ["switch_workbuddy_account", { targetAccountId: "uid-2", confirmed: true }],
      );
    });
    await waitFor(() => {
      expect(vi.mocked(notify.success)).toHaveBeenCalledWith(
        expect.stringContaining("已切换登录到「乙账号」"), undefined,
      );
    });
    expect(vi.mocked(notify.success)).toHaveBeenCalledWith(
      expect.stringContaining("12 条本地会话已随行"), undefined,
    );
    // 成功后弹层关闭（切号单槽清空）。
    await waitFor(() => expect(screen.queryByTestId("workbuddy-switch-dialog")).not.toBeInTheDocument());
  });

  // 旧 L724-758：switched=true 且 transfer_error 非空 → 成功通知 + 过户失败警告（切号不回滚）。
  it("部分成功：过户失败只警告，切号不回滚；云端残留明示不随行", async () => {
    installBaseRoutes();
    mockRoute("switch_workbuddy_account", () => ({
      switched: true, target_label: "乙账号", previous_label: "甲账号", transfer_executed: null,
      updated_rows: null, verified_sessions: null, missing_body_files: null, cloud_residual_rows: 2,
      transfer_error: "workbuddy_transfer_db_locked", restarted: true, post_status: "active",
    }));
    render(<AccountsPage active={true} productId="workbuddy" />);

    fireEvent.click(await screen.findByTestId("workbuddy-switch-wb-uid-2"));
    await screen.findByTestId("workbuddy-switch-dialog");
    fireEvent.click(screen.getByTestId("workbuddy-switch-dialog-confirm"));

    await waitFor(() => {
      expect(vi.mocked(notify.success)).toHaveBeenCalledWith(
        expect.stringContaining("已切换登录到「乙账号」"), undefined,
      );
    });
    expect(vi.mocked(notify.warning)).toHaveBeenCalledWith(
      expect.stringContaining("历史会话过户失败"), expect.stringContaining("切号本身不受影响"),
    );
    // cloud 红线：>0 时成功通知明示不随行。
    expect(vi.mocked(notify.success)).toHaveBeenCalledWith(
      expect.stringContaining("2 条云端会话不随行"), undefined,
    );
    // 不谎报失败：切号成功路径不得走 error。
    expect(notify.error).not.toHaveBeenCalled();
  });

  // 旧 L760-773：preview 失败不开弹窗（客户端登录面不可读时 switch 也必然失败）。
  it("preview 失败不开弹窗：如实报错，且不发起 switch", async () => {
    installBaseRoutes();
    mockRoute("preview_workbuddy_switch", () => Promise.reject("workbuddy_data_root_unavailable"));
    render(<AccountsPage active={true} productId="workbuddy" />);

    fireEvent.click(await screen.findByTestId("workbuddy-switch-wb-uid-2"));
    await waitFor(() => {
      expect(vi.mocked(notify.error)).toHaveBeenCalledWith(
        expect.stringContaining("切号预览未完成"),
      );
    });
    expect(screen.queryByTestId("workbuddy-switch-dialog")).not.toBeInTheDocument();
    // 未发起 switch（confirmed 入口未触达）。
    expect(callsOf("switch_workbuddy_account")).toBe(0);
  });
});
