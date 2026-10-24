import { fireEvent, render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { installFakeInvoke, mockCoreInvoke, mockRoute, resetRoutes } from "./helpers/fakeInvoke";

vi.mock("@tauri-apps/api/core", async () => {
  const fake = await import("./helpers/fakeInvoke");
  return { invoke: fake.mockCoreInvoke };
});

vi.mock("../src/platform/notify", () => ({
  notify: { success: vi.fn(), error: vi.fn(), warning: vi.fn(), info: vi.fn(), summary: vi.fn() },
}));

import { SettingsPage } from "../src/features/settings/SettingsPage";

/**
 * P4f-5 设置页装配的两条行为断言：
 * ① 读取门禁统一取 TRAE 口径——WB 由「挂载即读」改为「可见才读」（KNOWN-ISSUES K-11）：
 *    active=false 零 IPC，active=true 才读三路。
 * ② 位置变更弹层不再夹带「当前目录」行（旧弹层只在弹层外显示位置，重构不夹带新功能）。
 */
function installWbRoutes() {
  mockRoute("get_workbuddy_auto_checkin_status", () => ({ enabled: true, daily_time_hhmm: "09:30", ledger: null }));
  mockRoute("list_workbuddy_checkin_accounts", () => [
    { profile_id: "wb-1", screen_name: "甲账号", enabled: true },
  ]);
  mockRoute("list_workbuddy_backups", () => ({ backup_dir: "C:\\bak", backups: [] }));
}

describe("设置页装配（P4f-5）", () => {
  beforeEach(() => {
    resetRoutes();
    installFakeInvoke();
  });

  it("WB 读取门禁：页面不可见零 IPC，可见后才读签到与备份链（K-11）", async () => {
    installWbRoutes();
    const { rerender } = render(<SettingsPage active={false} productId="workbuddy" />);

    // 不可见：一条命令都不发（旧 WB 是挂载即读，本票有意改为可见才读）。
    expect(mockCoreInvoke).not.toHaveBeenCalled();
    // 页头契约照旧在册（region + h1[data-page-title]）；备份区只有空壳标题（无份数徽章、无读取中句）。
    expect(screen.getByRole("region", { name: "设置" })).toBeInTheDocument();
    expect(screen.getByTestId("workbuddy-backup-section")).toBeInTheDocument();
    expect(screen.queryByText("正在读取备份链…")).not.toBeInTheDocument();

    rerender(<SettingsPage active={true} productId="workbuddy" />);
    expect(await screen.findByTestId("workbuddy-auto-checkin-settings")).toBeInTheDocument();
    expect(await screen.findByTestId("workbuddy-backup-section")).toBeInTheDocument();
    expect(mockCoreInvoke.mock.calls.map(([cmd]) => cmd)).toEqual(expect.arrayContaining([
      "get_workbuddy_auto_checkin_status", "list_workbuddy_checkin_accounts", "list_workbuddy_backups",
    ]));
  });

  it("位置变更弹层不含「当前目录」行（与旧实现一致，只在弹层外显示位置）", async () => {
    mockRoute("get_master_backup_chain", () => ({
      backups: [], backup_dir: "D:\\bak", keep_policy: 5,
    }));
    mockRoute("get_auto_checkin_settings", () => ({ enabled: false, daily_time_hhmm: "09:00", ledger: null }));
    mockRoute("get_backup_retention", () => ({ enabled: true, keep: 5 }));

    render(<SettingsPage active={true} productId="trae_cn" />);
    fireEvent.click(await screen.findByTestId("backup-location-change"));

    const dialog = await screen.findByTestId("backup-location-dialog");
    // 弹层内只有标题/说明/输入/迁移勾选：不出现「当前备份位置」与目录值。
    expect(dialog).not.toHaveTextContent("当前备份位置");
    expect(dialog).not.toHaveTextContent("D:\\bak");
    expect(screen.getByTestId("backup-location-input")).toBeInTheDocument();
  });
});
