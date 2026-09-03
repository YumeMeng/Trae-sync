import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { MasterSwitchDialog } from "../src/components/MasterSwitchDialog";
import type {
  MasterAccountSwitchDto,
  MasterSwitchPluginPreviewDto,
} from "../src/types/account_switch";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
// 事件插件 mock：捕获 listen 回调，供阶段推进测试手动触发（模式同 CheckinPage.test）。
const { mockListen } = vi.hoisted(() => ({ mockListen: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({ listen: mockListen }));

const mockInvoke = vi.mocked(invoke);

const target = { profile_id: "profile-b", display_name: "账号乙" };

/** 预检返回（默认 aborted：静默直过切号，不弹确认）。 */
function preview(overrides: Partial<MasterSwitchPluginPreviewDto> = {}): MasterSwitchPluginPreviewDto {
  return {
    source_count: 0,
    target_count: 0,
    install_names: [],
    remove_names: [],
    aborted: true,
    ...overrides,
  };
}

function receipt(): MasterAccountSwitchDto {
  return {
    profile_id: "profile-b",
    to_user_id: "4050081351",
    from_user_id: "4050081350",
    transferred_projects: 4,
    removed_mirror_rows: 1,
    switched_sessions: 7,
    backup_path: "C:\\bak",
    relay_ledger_written: true,
    plugin_sync: {
      source_count: 5,
      installed: 0,
      removed: 0,
      failed: 0,
      skipped: 0,
      aborted: false,
      declined: false,
    },
    relaunch_outcome: "launched",
  };
}

function renderDialog(overrides: { onFinished?: () => Promise<void> } = {}) {
  const onFinished = overrides.onFinished ?? vi.fn(async () => undefined);
  const onClose = vi.fn();
  render(
    <MasterSwitchDialog
      target={target}
      onFinished={onFinished}
      onClose={onClose}
    />,
  );
  return { onFinished, onClose };
}

describe("MasterSwitchDialog（P5-2 切号弹层 + P5-8b-3 插件差异确认）", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    mockListen.mockResolvedValue(() => undefined);
    // 默认路由：预检 aborted（静默直过）→ 切换挂起（模拟进行中）。
    mockInvoke.mockImplementation(async (command: string) => {
      if (command === "preview_master_switch_plugins") return preview();
      return new Promise(() => undefined);
    });
  });

  it("预检无差异时静默直过切号（applyPlugins=true，force=false）并展示五步与目标账号", async () => {
    renderDialog();
    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith("preview_master_switch_plugins", { profileId: "profile-b" });
    });
    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith("switch_master_account", {
        profileId: "profile-b",
        force: false,
        applyPlugins: true,
      });
    });
    expect(screen.getByTestId("master-switch-dialog")).toBeInTheDocument();
    expect(screen.getByTestId("master-switch-target")).toHaveTextContent("账号乙");
    // 五步文案齐全（Q1.1 顺序）
    expect(screen.getByText("关闭 TRAE 主库实例")).toBeInTheDocument();
    expect(screen.getByText("备份主库数据")).toBeInTheDocument();
    expect(screen.getByText("写入目标账号登录态")).toBeInTheDocument();
    expect(screen.getByText("转移对话记录到新账号")).toBeInTheDocument();
    expect(screen.getByText("重启 TRAE")).toBeInTheDocument();
  });

  it("预检失败不阻断切号（fail-soft：直接发起切换）", async () => {
    mockInvoke.mockImplementation(async (command: string) => {
      if (command === "preview_master_switch_plugins") throw new Error("network");
      return new Promise(() => undefined);
    });
    renderDialog();
    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith("switch_master_account", {
        profileId: "profile-b",
        force: false,
        applyPlugins: true,
      });
    });
  });

  it("进度事件推进步骤；回执到达展示完成态与随行统计", async () => {
    let resolveSwitch: (value: MasterAccountSwitchDto) => void = () => undefined;
    mockInvoke.mockImplementation(async (command: string) => {
      if (command === "preview_master_switch_plugins") return preview();
      return new Promise<MasterAccountSwitchDto>((resolve) => {
        resolveSwitch = resolve;
      });
    });
    const handlers = new Map<string, (event: { payload: unknown }) => void>();
    mockListen.mockImplementation(async (event: string, cb: (event: { payload: unknown }) => void) => {
      handlers.set(event, cb);
      return () => undefined;
    });
    const { onFinished } = renderDialog();

    // 预检直过后切换发起（phase 进入 running）再推进事件，避免事件早到被丢弃。
    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith("switch_master_account", {
        profileId: "profile-b",
        force: false,
        applyPlugins: true,
      });
    });

    // backing_up 事件 → 第 2 步进行中
    handlers.get("master-switch-progress")?.({ payload: { profile_id: "profile-b", stage: "backing_up" } });
    await waitFor(() => {
      expect(screen.getByText("备份主库数据").closest(".switch-step")).toHaveClass("switch-step--run");
    });

    // 回执到达 → 完成态：勾选行 + 随行统计 + 完成按钮
    resolveSwitch(receipt());
    await waitFor(() => {
      expect(screen.getByText(/已切换到 账号乙/)).toBeInTheDocument();
    });
    expect(screen.getByText(/保留项目 4 个/)).toBeInTheDocument();
    expect(screen.getByText(/转移会话 7 个/)).toBeInTheDocument();
    expect(screen.getByTestId("master-switch-close")).toBeInTheDocument();
    expect(onFinished).toHaveBeenCalledTimes(1);
  });

  it("master_switch_busy 走「等待完成 / 强制切换」分支，强制后带 force=true 重发", async () => {
    // args 用 unknown 收窄（invoke 签名的 InvokeArgs 含 number[]，不能只声明对象形态）
    mockInvoke.mockImplementation(async (command: string, args?: unknown) => {
      if (command === "preview_master_switch_plugins") return preview();
      const force = (args as { force?: boolean } | undefined)?.force === true;
      if (!force) throw "master_switch_busy";
      return receipt();
    });
    renderDialog();

    // busy 分支提示
    expect(await screen.findByText("TRAE 正在生成回复")).toBeInTheDocument();
    expect(screen.getByTestId("master-switch-wait")).toBeInTheDocument();

    // 强制切换 → force=true 重发（保留预检确认的 applyPlugins）→ 完成态
    fireEvent.click(screen.getByTestId("master-switch-force"));
    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith("switch_master_account", {
        profileId: "profile-b",
        force: true,
        applyPlugins: true,
      });
    });
    await waitFor(() => {
      expect(screen.getByText(/已切换到 账号乙/)).toBeInTheDocument();
    });
  });

  it("已知错误码展示独立文案（如 switch_donor_login_missing）", async () => {
    mockInvoke.mockImplementation(async (command: string) => {
      if (command === "preview_master_switch_plugins") return preview();
      throw "switch_donor_login_missing";
    });
    renderDialog();
    expect(await screen.findByText(/目标账号的登录凭据不可用/)).toBeInTheDocument();
  });

  it("P7-2：失败前收到回滚事件 → 失败文案附「已自动还原，可安全重试」", async () => {
    // 模拟后端时序：rolled-back 事件先到，invoke 拒绝后到。
    let rejectSwitch: (reason: unknown) => void = () => undefined;
    mockInvoke.mockImplementation(async (command: string) => {
      if (command === "preview_master_switch_plugins") return preview();
      return new Promise((_resolve, reject) => {
        rejectSwitch = reject;
      });
    });
    const handlers = new Map<string, (event: { payload: unknown }) => void>();
    mockListen.mockImplementation(async (event: string, cb: (event: { payload: unknown }) => void) => {
      handlers.set(event, cb);
      return () => undefined;
    });
    renderDialog();

    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith("switch_master_account", {
        profileId: "profile-b",
        force: false,
        applyPlugins: true,
      });
    });
    // 事件先到（rolled_back=true），错误后到。
    handlers.get("master-switch-rolled-back")?.({
      payload: { profile_id: "profile-b", rolled_back: true },
    });
    rejectSwitch("master_switch_conflict");
    expect(await screen.findByText(/已自动还原到切换前的登录状态，可安全重试/)).toBeInTheDocument();
    expect(screen.getByText(/目标账号名下存在同名项目/)).toBeInTheDocument();
  });

  it("P7-3：handing_over 事件带细粒度进度 → 展示项目名与序号", async () => {
    mockInvoke.mockImplementation(async (command: string) => {
      if (command === "preview_master_switch_plugins") return preview();
      return new Promise(() => undefined);
    });
    const handlers = new Map<string, (event: { payload: unknown }) => void>();
    mockListen.mockImplementation(async (event: string, cb: (event: { payload: unknown }) => void) => {
      handlers.set(event, cb);
      return () => undefined;
    });
    renderDialog();

    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith("switch_master_account", {
        profileId: "profile-b",
        force: false,
        applyPlugins: true,
      });
    });
    handlers.get("master-switch-progress")?.({
      payload: {
        profile_id: "profile-b",
        stage: "handing_over",
        progress: { phase: "mapping", current: 1, total: 2, label: "官网改版" },
      },
    });
    await waitFor(() => {
      expect(screen.getByTestId("master-switch-handover-progress")).toHaveTextContent(
        "正在整理项目“官网改版”的会话（1/2）",
      );
    });
    // 进入下一阶段（restarting）→ 细目进度消失。
    handlers.get("master-switch-progress")?.({
      payload: { profile_id: "profile-b", stage: "restarting" },
    });
    await waitFor(() => {
      expect(screen.queryByTestId("master-switch-handover-progress")).not.toBeInTheDocument();
    });
  });

  it("target 为 null 不渲染", () => {
    const { container } = render(
      <MasterSwitchDialog target={null} onFinished={vi.fn()} onClose={vi.fn()} />,
    );
    expect(container.firstChild).toBeNull();
    expect(mockInvoke).not.toHaveBeenCalled();
  });

  describe("插件差异确认（+N/-M）", () => {
    function diffPreview(): MasterSwitchPluginPreviewDto {
      return preview({
        aborted: false,
        source_count: 3,
        target_count: 2,
        install_names: ["飞书协作", "视频生成"],
        remove_names: ["浏览器控制"],
      });
    }

    it("预检出差异弹确认：列出 +N/-M 与插件名，确认后带 applyPlugins=true 切换", async () => {
      mockInvoke.mockImplementation(async (command: string) => {
        if (command === "preview_master_switch_plugins") return diffPreview();
        return receipt();
      });
      renderDialog();

      // 确认分支：差异说明 + 插件名清单（+ 安装 / − 移除）
      expect(await screen.findByTestId("master-switch-plugin-diff")).toBeInTheDocument();
      expect(screen.getByText(/安装 2 个、移除 1 个/)).toBeInTheDocument();
      expect(screen.getByText("+ 飞书协作")).toBeInTheDocument();
      expect(screen.getByText("− 浏览器控制")).toBeInTheDocument();

      // 确认同步 → applyPlugins=true 发起切换 → 完成态
      fireEvent.click(screen.getByTestId("master-switch-apply-plugins"));
      await waitFor(() => {
        expect(mockInvoke).toHaveBeenCalledWith("switch_master_account", {
          profileId: "profile-b",
          force: false,
          applyPlugins: true,
        });
      });
      await waitFor(() => {
        expect(screen.getByText(/已切换到 账号乙/)).toBeInTheDocument();
      });
    });

    it("选择保留目标账号插件：带 applyPlugins=false 切换，回执透出 declined 文案", async () => {
      const declinedReceipt = {
        ...receipt(),
        plugin_sync: { ...receipt().plugin_sync, declined: true },
      };
      mockInvoke.mockImplementation(async (command: string) => {
        if (command === "preview_master_switch_plugins") return diffPreview();
        return declinedReceipt;
      });
      renderDialog();

      expect(await screen.findByTestId("master-switch-plugin-diff")).toBeInTheDocument();
      fireEvent.click(screen.getByTestId("master-switch-keep-plugins"));
      await waitFor(() => {
        expect(mockInvoke).toHaveBeenCalledWith("switch_master_account", {
          profileId: "profile-b",
          force: false,
          applyPlugins: false,
        });
      });
      await waitFor(() => {
        expect(screen.getByText(/已按目标账号的插件现状切换/)).toBeInTheDocument();
      });
    });
  });
});
