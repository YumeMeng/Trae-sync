import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { SettingsPanel } from "../src/components/SettingsPanel";
import type { MasterBackupEntryDto } from "../src/types/masterLibrary";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

// P11-1 切片②：操作结果类提示已迁到 notify 适配层；断言 notify 调用而非页内文本。
vi.mock("../src/platform/notify", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../src/platform/notify")>();
  return {
    ...actual,
    notify: {
      success: vi.fn(),
      error: vi.fn(),
      info: vi.fn(),
      warning: vi.fn(),
      summary: vi.fn(),
    },
  };
});

import { notify } from "../src/platform/notify";

const mockInvoke = vi.mocked(invoke);
const notifyMock = vi.mocked(notify);

describe("设置面板", () => {
  beforeEach(() => {
    mockInvoke.mockReset();
  });

  afterEach(() => vi.clearAllMocks());

  it("G25：安全默认值死区已删除，不再出现占位开关与开发声明", () => {
    render(<SettingsPanel active={false} />);

    expect(screen.queryByText("自动查找新历史")).not.toBeInTheDocument();
    expect(screen.queryByText("固定安全值")).not.toBeInTheDocument();
    expect(screen.queryByTestId("settings-readonly-notice")).not.toBeInTheDocument();
    expect(screen.queryByText(/尚未接入的选项/)).not.toBeInTheDocument();
  });

  it("不把存储根或其他技术参数伪装成日常设置", () => {
    render(<SettingsPanel active={false} />);

    expect(screen.queryByText("存储根")).not.toBeInTheDocument();
    expect(screen.queryByTestId("storage-root-state")).not.toBeInTheDocument();
    expect(screen.getByRole("region", { name: "设置" })).toBeInTheDocument();
    expect(screen.queryByText("读取完成后重新打开 TRAE")).not.toBeInTheDocument();
  });

  it("G3：备份路径收进 details，创建入口可见", async () => {
    mockInvoke.mockImplementation(async (command) => {
      if (command === "get_master_backup_chain") {
        return { backups: [], backup_dir: "D:\\bak", keep_policy: 5 };
      }
      throw new Error(`unexpected command: ${String(command)}`);
    });

    render(<SettingsPanel active={true} />);

    expect(await screen.findByTestId("master-backup-location-details")).toBeInTheDocument();
    expect(screen.getByTestId("master-backup-create")).toBeInTheDocument();
  });

  it("自动签到区块展示开关、触发时间与今日台账", async () => {
    mockInvoke.mockImplementation(async (command) => {
      if (command === "get_auto_checkin_settings") {
        return {
          enabled: true,
          daily_time_hhmm: "10:30",
          ledger: { date: "2026-08-23", running: false, total: 5, completed: 4, failed: 1, skipped: 0 },
        };
      }
      throw new Error(`unexpected command: ${String(command)}`);
    });

    render(<SettingsPanel active={true} />);

    expect(await screen.findByText("已开启")).toBeInTheDocument();
    // 总开关为 toggle（checkbox）形态，开启态勾选。
    const toggle = screen.getByTestId("auto-checkin-enabled-toggle");
    expect(toggle.querySelector("input")).toBeChecked();
    expect(screen.getByTestId("auto-checkin-time-input")).toHaveValue("10:30");
    // 错峰与补偿说明收进悬浮提示，不再占界面正文。
    expect(screen.getByTestId("auto-checkin-enabled-toggle")).toHaveAttribute(
      "title",
      expect.stringContaining("错峰"),
    );
    // 台账含失败计数。
    expect(screen.getByTestId("auto-checkin-ledger")).toHaveTextContent("今日已完成（成功 4/5，失败 1）");
  });

  it("关闭自动签到后提交设置命令并回填关闭状态", async () => {
    let enabled = true;
    mockInvoke.mockImplementation(async (command) => {
      if (command === "get_auto_checkin_settings") {
        return { enabled, daily_time_hhmm: "10:00", ledger: null };
      }
      if (command === "set_auto_checkin_settings") {
        enabled = false;
        return undefined;
      }
      throw new Error(`unexpected command: ${String(command)}`);
    });

    render(<SettingsPanel active={true} />);
    await screen.findByTestId("auto-checkin-settings");

    const toggle = screen.getByTestId("auto-checkin-enabled-toggle");
    fireEvent.click(toggle.querySelector("input")!);

    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith("set_auto_checkin_settings", {
        enabled: false,
        dailyTimeHhmm: "10:00",
      });
    });
    await waitFor(() => {
      expect(notifyMock.success).toHaveBeenCalledWith("自动签到已关闭。");
    });
    // 关闭后时间控件禁用。
    expect(screen.getByTestId("auto-checkin-time-input")).toBeDisabled();
  });

  it("P5-4：备份分区展示备份链（份数/条目/恢复指引），读取失败时分区不渲染", async () => {
    const backupChain = {
      backups: [
        { stamp_unix_seconds: 1756000000, total_bytes: 2 * 1024 * 1024, has_wal: true },
        { stamp_unix_seconds: 1755900000, total_bytes: 512 * 1024, has_wal: false },
      ],
      backup_dir: "C:\\TraeSync\\data\\environments\\master\\ModularData\\ai-agent",
      keep_policy: 5,
    };
    let failChain = false;
    mockInvoke.mockImplementation(async (command) => {
      if (command === "get_master_backup_chain") {
        if (failChain) throw new Error("trae_real_mode_required");
        return backupChain;
      }
      throw new Error(`unexpected command: ${String(command)}`);
    });

    const { rerender } = render(<SettingsPanel active={true} />);
    const section = await screen.findByTestId("master-backup-section");
    // 徽章显示现有份数。
    expect(screen.getByText("现有 2 份")).toBeInTheDocument();
    // 备份条目含附属件标记与大小。
    expect(screen.getAllByTestId("master-backup-entry")).toHaveLength(2);
    expect(section).toHaveTextContent("含附属件");
    expect(section).toHaveTextContent("完整快照");
    expect(section).toHaveTextContent("2.0 MiB");
    // 恢复指引给出备份目录与人工步骤（P5-9：自动清理由保留设置决定，未开启时不自动删）。
    expect(screen.getByTestId("master-backup-restore-hint")).toHaveTextContent(/不会被自动删除/);
    expect(screen.getByTestId("master-backup-restore-hint")).toHaveTextContent(/关闭 TRAE/);

    // 读取失败（fixture 模式等）：分区整体消失（可插拔降级，不报错）。
    failChain = true;
    rerender(<SettingsPanel active={false} />);
    rerender(<SettingsPanel active={true} />);
    await waitFor(() => {
      expect(screen.queryByTestId("master-backup-section")).not.toBeInTheDocument();
    });
  });

  it("P5-4：点击立即备份提交命令并重读备份链", async () => {
    const backups: MasterBackupEntryDto[] = [];
    mockInvoke.mockImplementation(async (command) => {
      if (command === "get_master_backup_chain") {
        // 每次返回新对象（后端语义）：mock 持有同一引用会让 React 状态不变、徽章不刷新。
        return { backups: [...backups], backup_dir: "D:\\bak", keep_policy: 5 };
      }
      if (command === "create_master_backup") {
        backups.push({ stamp_unix_seconds: 1756000000, total_bytes: 1024 * 1024, has_wal: false, has_credentials: false });
        return "D:\\bak\\.switch-bak-1756000000";
      }
      throw new Error(`unexpected command: ${String(command)}`);
    });

    render(<SettingsPanel active={true} />);
    // 空链：显示引导文案而非空列表。
    expect(await screen.findByText(/尚无备份/)).toBeInTheDocument();

    fireEvent.click(screen.getByTestId("master-backup-create"));
    await waitFor(() => {
      // P11-4②：凭据勾选默认关闭，随命令提交（后端按 false 不建凭据子目录）。
      expect(mockInvoke).toHaveBeenCalledWith("create_master_backup", { includeCredentials: false });
    });
    await waitFor(() => {
      expect(notifyMock.success).toHaveBeenCalledWith("主库数据备份已创建，原数据未受影响。");
    });
    // 备份后备份链重读：徽章更新为 1 份。
    expect(await screen.findByText("现有 1 份")).toBeInTheDocument();
  });

  it("P11-4②：勾选包含账号凭据后提交 includeCredentials 并提示含凭据", async () => {
    mockInvoke.mockImplementation(async (command) => {
      if (command === "get_master_backup_chain") {
        return {
          backups: [{ stamp_unix_seconds: 1756000000, total_bytes: 1024, has_wal: false, has_credentials: true }],
          backup_dir: "D:\\bak",
          keep_policy: 5,
        };
      }
      if (command === "create_master_backup") {
        return "D:\\bak\\.switch-bak-1756000000";
      }
      throw new Error(`unexpected command: ${String(command)}`);
    });

    render(<SettingsPanel active={true} />);
    await screen.findByTestId("master-backup-section");

    fireEvent.click(screen.getByTestId("backup-include-credentials-toggle").querySelector("input")!);
    fireEvent.click(screen.getByTestId("master-backup-create"));
    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith("create_master_backup", { includeCredentials: true });
    });
    await waitFor(() => {
      expect(notifyMock.success).toHaveBeenCalledWith("主库数据备份已创建（含账号凭据子目录），原数据未受影响。");
    });
  });

  it("P11-4①：更改备份位置默认先迁移再切换并重读备份链", async () => {
    const calls: string[] = [];
    mockInvoke.mockImplementation(async (command) => {
      calls.push(command);
      if (command === "get_master_backup_chain") {
        return { backups: [], backup_dir: "D:\\bak", keep_policy: 5 };
      }
      if (command === "migrate_master_backups") {
        return { migrated_files: 3, failures: [] };
      }
      if (command === "set_backup_dir") return undefined;
      throw new Error(`unexpected command: ${String(command)}`);
    });

    render(<SettingsPanel active={true} />);
    await screen.findByTestId("master-backup-section");

    fireEvent.click(screen.getByTestId("backup-location-change"));
    fireEvent.change(screen.getByTestId("backup-location-input"), {
      target: { value: "E:\\TraeBackups" },
    });
    // 迁移勾选默认勾选。
    expect(screen.getByTestId("backup-location-migrate-toggle").querySelector("input")).toBeChecked();
    fireEvent.click(screen.getByTestId("backup-location-dialog-confirm"));

    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith("migrate_master_backups", { targetDir: "E:\\TraeBackups" });
      expect(mockInvoke).toHaveBeenCalledWith("set_backup_dir", { backupDir: "E:\\TraeBackups" });
    });
    // 顺序约束：迁移在切换之前（此刻活动目录仍是旧位置才能枚举到待迁备份）。
    expect(calls.indexOf("migrate_master_backups")).toBeLessThan(calls.indexOf("set_backup_dir"));
    await waitFor(() => {
      expect(notifyMock.success).toHaveBeenCalledWith(
        "备份位置已更新，3 个文件已复制到新位置（原位置备份保留，可自行清理）。",
      );
    });
  });

  it("P11-4①：取消迁移时只切换位置；迁移部分失败中止切换并返回清单", async () => {
    let failMigration = false;
    mockInvoke.mockImplementation(async (command) => {
      if (command === "get_master_backup_chain") {
        return { backups: [], backup_dir: "D:\\bak", keep_policy: 5 };
      }
      if (command === "migrate_master_backups") {
        if (failMigration) {
          return { migrated_files: 1, failures: [{ file_name: "database.db.switch-bak-1000", reason: "目标已存在同名文件且内容不一致，未覆盖" }] };
        }
        return { migrated_files: 2, failures: [] };
      }
      if (command === "set_backup_dir") return undefined;
      throw new Error(`unexpected command: ${String(command)}`);
    });

    render(<SettingsPanel active={true} />);
    await screen.findByTestId("master-backup-section");

    // 取消迁移：不调 migrate，直接 set。
    fireEvent.click(screen.getByTestId("backup-location-change"));
    fireEvent.change(screen.getByTestId("backup-location-input"), {
      target: { value: "E:\\TraeBackups" },
    });
    fireEvent.click(screen.getByTestId("backup-location-migrate-toggle").querySelector("input")!);
    fireEvent.click(screen.getByTestId("backup-location-dialog-confirm"));
    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith("set_backup_dir", { backupDir: "E:\\TraeBackups" });
    });
    expect(mockInvoke).not.toHaveBeenCalledWith("migrate_master_backups", expect.anything());
    await waitFor(() => {
      expect(notifyMock.success).toHaveBeenCalledWith("备份位置已更新，之后的备份将写入新目录。");
    });

    // 部分失败：中止切换（不调 set），notify.summary 带失败清单。
    // 重新打开弹层时迁移勾选被重置为默认勾选，直接确认即可走迁移路径。
    failMigration = true;
    fireEvent.click(screen.getByTestId("backup-location-change"));
    fireEvent.change(screen.getByTestId("backup-location-input"), {
      target: { value: "F:\\TraeBackups" },
    });
    fireEvent.click(screen.getByTestId("backup-location-dialog-confirm"));
    await waitFor(() => {
      expect(notifyMock.summary).toHaveBeenCalledWith(
        "部分备份未能复制到新位置，备份位置未变更",
        [expect.objectContaining({ displayName: "database.db.switch-bak-1000" })],
        1,
      );
    });
    expect(mockInvoke).not.toHaveBeenCalledWith("set_backup_dir", { backupDir: "F:\\TraeBackups" });
  });

  it("P5-4：主库运行中创建备份被拒并给出稳定提示", async () => {
    mockInvoke.mockImplementation(async (command) => {
      if (command === "get_master_backup_chain") {
        return { backups: [], backup_dir: "D:\\bak", keep_policy: 5 };
      }
      if (command === "create_master_backup") {
        throw new Error("master_backup_running");
      }
      throw new Error(`unexpected command: ${String(command)}`);
    });

    render(<SettingsPanel active={true} />);
    await screen.findByTestId("master-backup-section");

    fireEvent.click(screen.getByTestId("master-backup-create"));
    await waitFor(() => {
      expect(notifyMock.error).toHaveBeenCalledWith(expect.stringMatching(/主库正在运行/));
    });
  });

  it("P5-9：备份分区展示保留设置并随开关保存", async () => {
    let retention = { enabled: true, keep: 5 };
    mockInvoke.mockImplementation(async (command) => {
      if (command === "get_master_backup_chain") {
        return { backups: [], backup_dir: "D:\\bak", keep_policy: 5 };
      }
      if (command === "get_backup_retention") return { ...retention };
      if (command === "set_backup_retention") {
        retention = { enabled: false, keep: retention.keep };
        return undefined;
      }
      throw new Error(`unexpected command: ${String(command)}`);
    });

    render(<SettingsPanel active={true} />);
    const settings = await screen.findByTestId("backup-retention-settings");
    // 开启态：hint 说明超出保留份数自动删除。
    expect(screen.getByTestId("backup-retention-hint")).toHaveTextContent(/自动删除/);
    expect(screen.getByTestId("backup-retention-keep-input")).toHaveValue(5);

    fireEvent.change(screen.getByTestId("backup-retention-enabled-select"), { target: { value: "off" } });
    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith("set_backup_retention", { enabled: false, keep: 5 });
    });
    await waitFor(() => {
      expect(notifyMock.success).toHaveBeenCalledWith("备份自动清理已关闭，旧备份不会再被自动删除。");
    });
    // 关闭后保留份数输入禁用。
    expect(screen.getByTestId("backup-retention-keep-input")).toBeDisabled();
    expect(settings).toBeInTheDocument();
  });

  it("P5-9：修改保留份数立即保存，越界值不提交", async () => {
    let retention = { enabled: true, keep: 5 };
    const calls: Array<{ enabled: boolean; keep: number }> = [];
    mockInvoke.mockImplementation(async (command, args) => {
      if (command === "get_master_backup_chain") {
        return { backups: [], backup_dir: "D:\\bak", keep_policy: 5 };
      }
      if (command === "get_backup_retention") return { ...retention };
      if (command === "set_backup_retention") {
        const payload = args as { enabled: boolean; keep: number };
        calls.push(payload);
        retention = { ...payload };
        return undefined;
      }
      throw new Error(`unexpected command: ${String(command)}`);
    });

    render(<SettingsPanel active={true} />);
    await screen.findByTestId("backup-retention-settings");

    fireEvent.change(screen.getByTestId("backup-retention-keep-input"), { target: { value: "12" } });
    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith("set_backup_retention", { enabled: true, keep: 12 });
    });
    await waitFor(() => {
      expect(notifyMock.success).toHaveBeenCalledWith(
        "备份自动清理已开启，超出 12 份的旧备份会在新备份生成后自动删除。",
      );
    });

    // 越界值（51）不提交，避免后端拒绝后设置与界面不一致。
    const before = calls.length;
    fireEvent.change(screen.getByTestId("backup-retention-keep-input"), { target: { value: "51" } });
    expect(calls).toHaveLength(before);
  });
});
