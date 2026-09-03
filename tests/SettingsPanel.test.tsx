import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { SettingsPanel } from "../src/components/SettingsPanel";
import type { KeyStatusDto } from "../src/types/account_switch";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

const mockInvoke = vi.mocked(invoke);

const keyStatus: KeyStatusDto = {
  source_key_configured: true,
  source_key_version: "v2026.08",
  source_key_pending_version: null,
  source_key_activation_pending: false,
  catalog_key_configured: true,
  catalog_key_generation: 3,
  probe_state: "verified",
};

describe("设置面板", () => {
  beforeEach(() => {
    mockInvoke.mockReset();
  });

  afterEach(() => vi.clearAllMocks());

  it("只显示已生效的固定安全值，不伪装成可编辑表单", () => {
    render(<SettingsPanel active={false} />);

    expect(screen.queryByRole("checkbox")).not.toBeInTheDocument();
    expect(screen.getByText("关闭")).toBeInTheDocument();
    expect(screen.getByTestId("settings-readonly-notice")).toHaveTextContent(
      /不会把未接入的选项显示为可编辑配置/,
    );
  });

  it("不把存储根或其他技术参数伪装成日常设置", () => {
    render(<SettingsPanel active={false} />);

    expect(screen.queryByText("存储根")).not.toBeInTheDocument();
    expect(screen.queryByTestId("storage-root-state")).not.toBeInTheDocument();
    expect(screen.getByRole("region", { name: "设置" })).toHaveTextContent(
      "自动查找新历史",
    );
    expect(screen.queryByText("读取完成后重新打开 TRAE")).not.toBeInTheDocument();
  });

  it("页面激活时读取密钥状态，只显示版本与代次，不回显正文", async () => {
    mockInvoke.mockImplementation(async (command) => {
      if (command === "get_key_status") return keyStatus;
      throw new Error(`unexpected command: ${String(command)}`);
    });

    render(<SettingsPanel active={true} />);

    expect(await screen.findByText("探测通过")).toBeInTheDocument();
    expect(screen.getByText(/来源密钥：v2026.08/)).toBeInTheDocument();
    expect(screen.getByText(/历史库密钥：代次 3/)).toBeInTheDocument();
    // 密钥正文永不回显：候选输入框是密码框，界面不出现密钥内容。
    expect(screen.getByTestId("source-key-candidate-input")).toHaveAttribute("type", "password");
  });

  it("可发起只读密钥探测并显示结论", async () => {
    mockInvoke.mockImplementation(async (command) => {
      if (command === "get_key_status") return keyStatus;
      if (command === "probe_source_key") {
        return { ...keyStatus, probe_state: "rejected" as const };
      }
      throw new Error(`unexpected command: ${String(command)}`);
    });

    render(<SettingsPanel active={true} />);
    await screen.findByText("探测通过");

    fireEvent.click(screen.getByRole("button", { name: "重新探测密钥" }));
    await waitFor(() => expect(mockInvoke).toHaveBeenCalledWith("probe_source_key"));
    expect(await screen.findByText("当前密钥未通过只读探测，已保持阻断。")).toBeInTheDocument();
    expect(screen.getByText("探测拒绝")).toBeInTheDocument();
  });

  it("登记候选密钥走只读验证，成功后清空输入并提示下次启动激活", async () => {
    mockInvoke.mockImplementation(async (command) => {
      if (command === "get_key_status") return keyStatus;
      if (command === "register_source_key_candidate") {
        return {
          ...keyStatus,
          probe_state: "verified_pending" as const,
          source_key_pending_version: "v2026.09-candidate",
          source_key_activation_pending: true,
        };
      }
      throw new Error(`unexpected command: ${String(command)}`);
    });

    render(<SettingsPanel active={true} />);
    await screen.findByText("探测通过");

    const input = screen.getByTestId("source-key-candidate-input");
    expect(screen.getByRole("button", { name: "登记候选密钥" })).toBeDisabled();
    fireEvent.change(input, { target: { value: "candidate-key-material" } });
    fireEvent.click(screen.getByRole("button", { name: "登记候选密钥" }));

    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith("register_source_key_candidate", {
        candidateKey: "candidate-key-material",
        productVersion: "TRAE Work CN",
      });
    });
    expect(await screen.findByText(/候选 source key 已通过只读验证并登记/)).toBeInTheDocument();
    expect(screen.getByTestId("source-key-pending-notice")).toHaveTextContent(/下一次启动激活/);
    // 登记成功后输入框清空，密钥材料不留在界面。
    expect(input).toHaveValue("");
  });

  it("自动签到区块展示开关、触发时间与今日台账", async () => {
    mockInvoke.mockImplementation(async (command) => {
      if (command === "get_key_status") return keyStatus;
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
    expect(screen.getByTestId("auto-checkin-enabled-select")).toHaveValue("on");
    expect(screen.getByTestId("auto-checkin-time-input")).toHaveValue("10:30");
    // 台账含失败计数。
    expect(screen.getByTestId("auto-checkin-ledger")).toHaveTextContent("今日已完成（成功 4/5，失败 1）");
  });

  it("关闭自动签到后提交设置命令并回填关闭状态", async () => {
    let enabled = true;
    mockInvoke.mockImplementation(async (command) => {
      if (command === "get_key_status") return keyStatus;
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

    fireEvent.change(screen.getByTestId("auto-checkin-enabled-select"), { target: { value: "off" } });

    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith("set_auto_checkin_settings", {
        enabled: false,
        dailyTimeHhmm: "10:00",
      });
    });
    expect(await screen.findByText("自动签到已关闭。")).toBeInTheDocument();
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
      if (command === "get_key_status") return keyStatus;
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
    const backups: Array<{ stamp_unix_seconds: number; total_bytes: number; has_wal: boolean }> = [];
    mockInvoke.mockImplementation(async (command) => {
      if (command === "get_key_status") return keyStatus;
      if (command === "get_master_backup_chain") {
        // 每次返回新对象（后端语义）：mock 持有同一引用会让 React 状态不变、徽章不刷新。
        return { backups: [...backups], backup_dir: "D:\\bak", keep_policy: 5 };
      }
      if (command === "create_master_backup") {
        backups.push({ stamp_unix_seconds: 1756000000, total_bytes: 1024 * 1024, has_wal: false });
        return "D:\\bak\\.switch-bak-1756000000";
      }
      throw new Error(`unexpected command: ${String(command)}`);
    });

    render(<SettingsPanel active={true} />);
    // 空链：显示引导文案而非空列表。
    expect(await screen.findByText(/尚无备份/)).toBeInTheDocument();

    fireEvent.click(screen.getByTestId("master-backup-create"));
    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith("create_master_backup");
    });
    expect(await screen.findByText("主库数据备份已创建，原数据未受影响。")).toBeInTheDocument();
    // 备份后备份链重读：徽章更新为 1 份。
    expect(await screen.findByText("现有 1 份")).toBeInTheDocument();
  });

  it("P5-4：主库运行中创建备份被拒并给出稳定提示", async () => {
    mockInvoke.mockImplementation(async (command) => {
      if (command === "get_key_status") return keyStatus;
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
    expect(await screen.findByRole("alert")).toHaveTextContent(/主库正在运行/);
  });

  it("P5-9：备份分区展示保留设置并随开关保存", async () => {
    let retention = { enabled: true, keep: 5 };
    mockInvoke.mockImplementation(async (command) => {
      if (command === "get_key_status") return keyStatus;
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
    expect(await screen.findByText(/备份自动清理已关闭/)).toBeInTheDocument();
    // 关闭后保留份数输入禁用。
    expect(screen.getByTestId("backup-retention-keep-input")).toBeDisabled();
    expect(settings).toBeInTheDocument();
  });

  it("P5-9：修改保留份数立即保存，越界值不提交", async () => {
    let retention = { enabled: true, keep: 5 };
    const calls: Array<{ enabled: boolean; keep: number }> = [];
    mockInvoke.mockImplementation(async (command, args) => {
      if (command === "get_key_status") return keyStatus;
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
    expect(await screen.findByText(/保留 12 份/)).toBeInTheDocument();

    // 越界值（51）不提交，避免后端拒绝后设置与界面不一致。
    const before = calls.length;
    fireEvent.change(screen.getByTestId("backup-retention-keep-input"), { target: { value: "51" } });
    expect(calls).toHaveLength(before);
  });
});
