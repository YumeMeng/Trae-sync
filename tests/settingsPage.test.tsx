import { fireEvent, render, screen, waitFor } from "@testing-library/react";
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
import { notify } from "../src/platform/notify";

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

/**
 * P4f-7 收编：原 tests/WorkBuddyPages.test.tsx L318-376 的四例（旧组件 `render(<WorkBuddyBackupSection />)`）
 * 逐例迁到新页——经 SettingsPage(productId=workbuddy) 渲染，锚点与文案与旧实现逐字一致
 * （旧 WB L139/L148/L156/L167/L190/L223/L242/L260，新 BackupSection 按同一锚点渲染）。
 * 四例守的都是**用户可见行为**（链如实呈现、三个破坏性动作的单次确认与命令载荷），无一例只测内部细节，
 * 故全部保留、零删除；差异只有渲染入口（旧组件 → 合并页）与「恢复/删除成功后链重读」由 hook 承担。
 */
const wbBackupChain = {
  backup_dir: "C:\\data\\workbuddy-backups",
  backups: [
    { stamp_unix_seconds: 1760001200, file_count: 12, source_bytes: 2048, compressed_bytes: 1024 },
    { stamp_unix_seconds: 1759000000, file_count: null, source_bytes: null, compressed_bytes: 512 },
  ],
};

/** WB 备份四件套 + 签到两路（设置页可见即读三路，未注册的路会 fail-fast）。 */
function installWbBackupRoutes() {
  mockRoute("get_workbuddy_auto_checkin_status", () => ({ enabled: false, daily_time_hhmm: "09:30", ledger: null }));
  mockRoute("list_workbuddy_checkin_accounts", () => []);
  mockRoute("list_workbuddy_backups", () => wbBackupChain);
  mockRoute("create_workbuddy_backup", () => ({
    backup_dir: "C:\\data\\workbuddy-backups", stamp_unix_seconds: 1760002000,
    file_count: 13, source_bytes: 2100, rolled_over: 1,
  }));
  mockRoute("restore_workbuddy_backup", (args: { stamp: number }) => ({
    restored_files: 12, restored_bytes: 2048,
    safety_backup_stamp: 1760001500, restored_stamp: args.stamp,
  }));
  mockRoute("delete_workbuddy_backups", (args: { stamps: number[] }) => ({
    deleted_stamps: args.stamps, missing_stamps: [], remaining: 1,
    backup_dir: "C:\\data\\workbuddy-backups",
  }));
}

describe("WB 备份链管理面（P19-B8 四例收编到合并页，P4f-7）", () => {
  beforeEach(() => {
    resetRoutes();
    installFakeInvoke();
    installWbBackupRoutes();
    vi.mocked(notify.success).mockClear();
    vi.mocked(notify.error).mockClear();
  });

  it("渲染：链列表（最新在前）+ 手动备份/删除/恢复入口 + 半成品条目如实标注", async () => {
    render(<SettingsPage active={true} productId="workbuddy" />);

    const chain = await screen.findByTestId("workbuddy-backup-chain");
    expect(chain.textContent).toContain("12 个文件");
    // 半成品条目（manifest 不可读）如实标注。
    expect(chain.textContent).toContain("清单缺失");
    expect(screen.getByTestId("workbuddy-backup-create")).toBeEnabled();
    expect(screen.getByTestId("workbuddy-backup-delete")).toBeDisabled();
    // 恢复入口逐条在册（旧 WB 每行一个「恢复」按钮）。
    expect(screen.getByTestId("workbuddy-backup-restore-1760001200")).toBeInTheDocument();
    expect(screen.getByTestId("workbuddy-backup-restore-1759000000")).toBeInTheDocument();
  });

  it("手动备份：确认弹窗带客户端重启说明，回执滚动清理如实提示", async () => {
    render(<SettingsPage active={true} productId="workbuddy" />);

    fireEvent.click(await screen.findByTestId("workbuddy-backup-create"));
    const dialog = await screen.findByTestId("workbuddy-backup-create-confirm");
    expect(dialog).toHaveTextContent("自动重启");
    fireEvent.click(screen.getByTestId("workbuddy-backup-create-confirm-confirm"));
    await waitFor(() => {
      expect(mockCoreInvoke.mock.calls.some(([cmd]) => cmd === "create_workbuddy_backup")).toBe(true);
    });
    // 回执：rolled_over>0 时如实提示滚动清理份数（旧 WB L58-60）。
    await waitFor(() => {
      expect(vi.mocked(notify.success)).toHaveBeenCalledWith(expect.stringContaining("滚动清理 1 份旧备份"));
    });
  });

  it("恢复：确认弹窗展示保链回退说明；恢复调用带链内 stamp", async () => {
    render(<SettingsPage active={true} productId="workbuddy" />);

    fireEvent.click(await screen.findByTestId("workbuddy-backup-restore-1760001200"));
    const dialog = await screen.findByTestId("workbuddy-backup-restore-confirm");
    expect(dialog).toHaveTextContent("自动把当前状态备份一份");
    fireEvent.click(screen.getByTestId("workbuddy-backup-restore-confirm-confirm"));
    await waitFor(() => {
      expect(mockCoreInvoke.mock.calls).toContainEqual(["restore_workbuddy_backup", { stamp: 1760001200 }]);
    });
    // safety_backup_stamp 回退点呈现（be 契约：UI 务必展示该字段）。
    await waitFor(() => {
      expect(vi.mocked(notify.success)).toHaveBeenCalledWith(expect.stringContaining("恢复前状态已存为"));
    });
  });

  it("删除：多选 + 单次确认调用 delete（confirmed=true 由适配器承载）", async () => {
    render(<SettingsPage active={true} productId="workbuddy" />);

    fireEvent.click(await screen.findByTestId("workbuddy-backup-select-1760001200"));
    fireEvent.click(screen.getByTestId("workbuddy-backup-delete"));
    fireEvent.click(await screen.findByTestId("workbuddy-backup-delete-confirm-confirm"));
    await waitFor(() => {
      expect(mockCoreInvoke.mock.calls).toContainEqual(
        ["delete_workbuddy_backups", { stamps: [1760001200], confirmed: true }],
      );
    });
  });
});