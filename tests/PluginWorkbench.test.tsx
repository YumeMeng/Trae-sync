import { beforeEach, describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import { PluginWorkbench } from "../src/components/PluginWorkbench";
import type { PluginTabStateDto } from "../src/types/plugins";

// mock Tauri invoke——前端测试不依赖真实后端
vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(),
}));

const mockInvoke = vi.mocked(invoke);

function tabState(overrides: Partial<PluginTabStateDto> = {}): PluginTabStateDto {
  return {
    installed: [
      {
        record_id: "rec-1",
        marketplace_plugin_id: "uuid-p1",
        name: "plugin-one",
        display_name: "插件一",
        version: "1.0.0",
        registry: "trae-remote-official",
        builtin: false,
        in_manifest: true,
      },
      {
        record_id: "rec-2",
        marketplace_plugin_id: null,
        name: "builtin-lark",
        display_name: "飞书",
        version: "0.0.1",
        registry: "builtin",
        builtin: true,
        in_manifest: false,
      },
    ],
    manifest: [
      {
        marketplace_plugin_id: "uuid-p1",
        name: "plugin-one",
        display_name: "插件一",
        version: "1.0.0",
        registry: "trae-remote-official",
        installed_in_cloud: true,
      },
    ],
    ...overrides,
  };
}

const MARKET = [
  {
    plugin_id: "uuid-p1",
    name: "plugin-one",
    display_name: "插件一",
    description: "合成市场描述一",
    registry: "trae-remote-official",
    categories: ["efficiency"],
    category_key: "efficiency",
    category_name: "效率工具",
  },
  {
    plugin_id: "uuid-p2",
    name: "plugin-two",
    display_name: "插件二",
    description: "合成市场描述二",
    registry: "trae-remote-official",
    categories: ["efficiency"],
    category_key: "efficiency",
    category_name: "效率工具",
  },
  {
    plugin_id: "uuid-p3",
    name: "plugin-three",
    display_name: "插件三",
    description: "合成市场描述三",
    registry: "trae-remote-official",
    categories: [],
    category_key: "other",
    category_name: "其他",
  },
];

/** 吸收后的收敛状态：手动装的插件入清单（in_manifest=true）。 */
function absorbedState(): PluginTabStateDto {
  const base = tabState();
  return {
    ...base,
    installed: base.installed.map((item) =>
      item.record_id === "rec-3" ? { ...item, in_manifest: true } : item,
    ),
  };
}

describe("PluginWorkbench（P5-8b 插件 tab）", () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it("已装列表渲染：随主库徽章 + 内置条目移除禁用", async () => {
    mockInvoke.mockImplementation(async (command: string) => {
      if (command === "get_plugin_tab_state") return tabState();
      return undefined;
    });
    render(<PluginWorkbench active />);

    await screen.findByTestId("plugin-row-rec-1");
    expect(screen.getByTestId("plugin-row-rec-1")).toHaveTextContent("插件一");
    expect(screen.getByTestId("plugin-row-rec-1")).toHaveTextContent("1.0.0");
    // 内置条目：徽章 + 移除禁用（云端无记录，卸载必须跳过）。
    expect(screen.getByTestId("plugin-row-rec-2")).toHaveTextContent("内置");
    expect(screen.getByTestId("plugin-uninstall-rec-2")).toBeDisabled();
    expect(screen.getByTestId("plugin-uninstall-rec-1")).toBeEnabled();
  });

  it("无差异不显示对账条（静默通过）", async () => {
    mockInvoke.mockImplementation(async (command: string) => {
      if (command === "get_plugin_tab_state") return tabState();
      return undefined;
    });
    render(<PluginWorkbench active />);

    await screen.findByTestId("plugin-row-rec-1");
    expect(screen.queryByTestId("plugin-drift")).not.toBeInTheDocument();
  });

  it("对账差异：显示手动装/卸数量，吸收后差异收敛", async () => {
    const withDrift = tabState({
      installed: [
        ...tabState().installed,
        {
          record_id: "rec-3",
          marketplace_plugin_id: "uuid-p3",
          name: "plugin-three",
          display_name: "插件三",
          version: "2.0.0",
          registry: "trae-remote-official",
          builtin: false,
          in_manifest: false,
        },
      ],
      manifest: [
        ...tabState().manifest,
        {
          marketplace_plugin_id: "uuid-p4",
          name: "plugin-four",
          display_name: "插件四",
          version: "1.0.0",
          registry: "trae-remote-official",
          installed_in_cloud: false,
        },
      ],
    });
    mockInvoke.mockImplementation(async (command: string) => {
      if (command === "get_plugin_tab_state") return withDrift;
      if (command === "absorb_plugin_manifest") return undefined;
      return undefined;
    });
    render(<PluginWorkbench active />);

    const drift = await screen.findByTestId("plugin-drift");
    expect(drift).toHaveTextContent("新装 1 项");
    expect(drift).toHaveTextContent("移除 1 项");

    fireEvent.click(screen.getByTestId("plugin-drift-absorb"));
    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith("absorb_plugin_manifest");
    });
  });

  it("吸收触发状态重读（absorbed 数据替换旧状态）", async () => {
    const withDrift = tabState({
      installed: [
        ...tabState().installed,
        {
          record_id: "rec-3",
          marketplace_plugin_id: "uuid-p3",
          name: "plugin-three",
          display_name: "插件三",
          version: "2.0.0",
          registry: "trae-remote-official",
          builtin: false,
          in_manifest: false,
        },
      ],
    });
    let state = withDrift;
    mockInvoke.mockImplementation(async (command: string) => {
      if (command === "get_plugin_tab_state") return state;
      if (command === "absorb_plugin_manifest") {
        state = absorbedState();
        return undefined;
      }
      return undefined;
    });
    render(<PluginWorkbench active />);

    await screen.findByTestId("plugin-drift");
    fireEvent.click(screen.getByTestId("plugin-drift-absorb"));
    // 吸收后：差异条消失 + 吸收回执可见。
    await waitFor(() => {
      expect(screen.queryByTestId("plugin-drift")).not.toBeInTheDocument();
    });
    expect(screen.getByTestId("plugin-notice")).toHaveTextContent("已按当前账号更新主库插件清单");
  });

  it("市场懒加载：首次进入市场段才拉目录，已装条目显示已安装", async () => {
    mockInvoke.mockImplementation(async (command: string) => {
      if (command === "get_plugin_tab_state") return tabState();
      if (command === "browse_plugin_market") return MARKET;
      return undefined;
    });
    render(<PluginWorkbench active />);

    await screen.findByTestId("plugin-row-rec-1");
    // 已装段不预取市场。
    expect(mockInvoke).not.toHaveBeenCalledWith("browse_plugin_market");

    fireEvent.click(screen.getByTestId("plugin-segment-market"));
    await screen.findByTestId("plugin-market-row-uuid-p2");
    expect(mockInvoke).toHaveBeenCalledWith("browse_plugin_market");
    // 已装条目（按市场 UUID 匹配）显示「已安装」而非安装按钮。
    expect(screen.getByTestId("plugin-market-installed-uuid-p1")).toBeVisible();
    expect(screen.queryByTestId("plugin-install-uuid-p1")).not.toBeInTheDocument();
    expect(screen.getByTestId("plugin-install-uuid-p2")).toBeEnabled();
  });

  it("市场目录按作用分类分组展示（与 TRAE 插件市场对齐）", async () => {
    mockInvoke.mockImplementation(async (command: string) => {
      if (command === "get_plugin_tab_state") return tabState();
      if (command === "browse_plugin_market") return MARKET;
      return undefined;
    });
    render(<PluginWorkbench active />);

    await screen.findByTestId("plugin-row-rec-1");
    fireEvent.click(screen.getByTestId("plugin-segment-market"));

    // 分组头：分类名 + 条目数；相邻同分类合并为一组。
    const efficiency = await screen.findByTestId("plugin-market-group-效率工具");
    expect(efficiency).toHaveTextContent("2 项");
    expect(efficiency).toHaveTextContent("插件一");
    expect(efficiency).toHaveTextContent("插件二");
    expect(screen.getByTestId("plugin-market-group-其他")).toHaveTextContent("插件三");
  });

  it("安装市场插件：调 install_plugin 后刷新状态并显示回执", async () => {
    let installedIds = new Set(["uuid-p1"]);
    mockInvoke.mockImplementation(async (command: string, args?: unknown) => {
      if (command === "get_plugin_tab_state") {
        const base = tabState();
        return {
          ...base,
          installed: [
            ...base.installed,
            ...(installedIds.has("uuid-p2")
              ? [
                  {
                    record_id: "rec-3",
                    marketplace_plugin_id: "uuid-p2",
                    name: "plugin-two",
                    display_name: "插件二",
                    version: "1.0.0",
                    registry: "trae-remote-official",
                    builtin: false,
                    in_manifest: true,
                  },
                ]
              : []),
          ],
        };
      }
      if (command === "browse_plugin_market") return MARKET;
      if (command === "install_plugin") {
        expect(args).toMatchObject({ pluginId: "uuid-p2", name: "plugin-two" });
        installedIds = new Set([...installedIds, "uuid-p2"]);
        return undefined;
      }
      return undefined;
    });
    render(<PluginWorkbench active />);

    await screen.findByTestId("plugin-row-rec-1");
    fireEvent.click(screen.getByTestId("plugin-segment-market"));
    await screen.findByTestId("plugin-install-uuid-p2");
    fireEvent.click(screen.getByTestId("plugin-install-uuid-p2"));

    // 安装回执 + 状态刷新后该条目变为已安装。
    await waitFor(() => {
      expect(screen.getByTestId("plugin-market-installed-uuid-p2")).toBeVisible();
    });
    expect(screen.getByTestId("plugin-notice")).toHaveTextContent("已安装 插件二");
  });

  it("卸载走二次确认：确认后调 uninstall_plugin 并刷新", async () => {
    let state = tabState();
    mockInvoke.mockImplementation(async (command: string, args?: unknown) => {
      if (command === "get_plugin_tab_state") return state;
      if (command === "uninstall_plugin") {
        expect(args).toMatchObject({ recordId: "rec-1" });
        state = {
          ...state,
          installed: state.installed.filter((item) => item.record_id !== "rec-1"),
          manifest: [],
        };
        return undefined;
      }
      return undefined;
    });
    render(<PluginWorkbench active />);

    await screen.findByTestId("plugin-uninstall-rec-1");
    fireEvent.click(screen.getByTestId("plugin-uninstall-rec-1"));
    // 确认弹窗出现（未确认不触发卸载）。
    expect(screen.getByTestId("plugin-uninstall-confirm")).toBeVisible();
    expect(mockInvoke).not.toHaveBeenCalledWith("uninstall_plugin", expect.anything());

    fireEvent.click(screen.getByTestId("plugin-uninstall-confirm-ok"));
    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith("uninstall_plugin", { recordId: "rec-1" });
    });
    // 卸载后行消失 + 回执可见。
    await waitFor(() => {
      expect(screen.queryByTestId("plugin-row-rec-1")).not.toBeInTheDocument();
    });
    expect(screen.getByTestId("plugin-notice")).toHaveTextContent("已移除 插件一");
  });

  it("卸载确认弹窗可取消，不触发命令", async () => {
    mockInvoke.mockImplementation(async (command: string) => {
      if (command === "get_plugin_tab_state") return tabState();
      return undefined;
    });
    render(<PluginWorkbench active />);

    await screen.findByTestId("plugin-uninstall-rec-1");
    fireEvent.click(screen.getByTestId("plugin-uninstall-rec-1"));
    fireEvent.click(screen.getByTestId("plugin-uninstall-confirm").querySelector("button")!);
    expect(screen.queryByTestId("plugin-uninstall-confirm")).not.toBeInTheDocument();
    expect(mockInvoke).not.toHaveBeenCalledWith("uninstall_plugin", expect.anything());
  });

  it("读取失败显示稳定错误文案（plugin_tab_no_account）", async () => {
    mockInvoke.mockImplementation(async (command: string) => {
      if (command === "get_plugin_tab_state") throw new Error("plugin_tab_no_account");
      return undefined;
    });
    render(<PluginWorkbench active />);

    const alert = await screen.findByTestId("plugin-error");
    expect(alert).toHaveTextContent("主库当前没有已登录账号");
  });

  it("active=false 不发起任何读取", () => {
    render(<PluginWorkbench active={false} />);
    expect(mockInvoke).not.toHaveBeenCalled();
  });

  it("已装为空显示空态引导", async () => {
    mockInvoke.mockImplementation(async (command: string) => {
      if (command === "get_plugin_tab_state") return { installed: [], manifest: [] };
      return undefined;
    });
    render(<PluginWorkbench active />);

    await screen.findByTestId("plugin-installed-empty");
    expect(screen.getByTestId("plugin-installed-empty")).toHaveTextContent("还没有已装插件");
  });
});
