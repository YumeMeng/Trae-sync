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
    known_account_count: 2,
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

/** 默认路由：插件状态 + 市场目录（G23 起已装段分类关联也用市场数据）。 */
function mockRoutes(handlers: Record<string, (args?: unknown) => unknown> = {}) {
  mockInvoke.mockImplementation(async (command: string, args?: unknown) => {
    if (command in handlers) return handlers[command](args);
    if (command === "get_plugin_tab_state") return tabState();
    if (command === "browse_plugin_market") return MARKET;
    return undefined;
  });
}

describe("PluginWorkbench（G23 插件 tab 表格化 + 实时同步）", () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it("已装列表表格渲染：切号保留标签 + 内置条目移除禁用 + 分类关联", async () => {
    mockRoutes();
    render(<PluginWorkbench active />);

    await screen.findByTestId("plugin-row-rec-1");
    // G4 术语：清单内条目标「切号保留」。
    expect(screen.getByTestId("plugin-row-rec-1")).toHaveTextContent("插件一");
    expect(screen.getByTestId("plugin-row-rec-1")).toHaveTextContent("切号保留");
    // 分类列：按市场 UUID 关联市场目录分类名。
    expect(screen.getByTestId("plugin-row-rec-1")).toHaveTextContent("效率工具");
    // 内置条目：标签 + 移除禁用（云端无记录，卸载必须跳过）。
    expect(screen.getByTestId("plugin-row-rec-2")).toHaveTextContent("内置");
    expect(screen.getByTestId("plugin-uninstall-rec-2")).toBeDisabled();
    expect(screen.getByTestId("plugin-uninstall-rec-1")).toBeEnabled();
  });

  it("清单外条目标「仅此账号」（G4 术语，替代旧「未入清单」）", async () => {
    mockRoutes({
      get_plugin_tab_state: () =>
        tabState({
          installed: [
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
        }),
    });
    render(<PluginWorkbench active />);

    const row = await screen.findByTestId("plugin-row-rec-3");
    expect(row).toHaveTextContent("仅此账号");
  });

  it("对账条已移除：存在差异也不显示 plugin-drift（ADR-0026 静默应用）", async () => {
    mockRoutes({
      get_plugin_tab_state: () =>
        tabState({
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
        }),
    });
    render(<PluginWorkbench active />);

    await screen.findByTestId("plugin-row-rec-3");
    expect(screen.queryByTestId("plugin-drift")).not.toBeInTheDocument();
    expect(screen.queryByTestId("plugin-drift-absorb")).not.toBeInTheDocument();
  });

  it("分类筛选 chips：复用市场分类，点击后列表过滤", async () => {
    mockRoutes();
    render(<PluginWorkbench active />);

    await screen.findByTestId("plugin-row-rec-1");
    // 已装段 chips：已装条目关联分类（效率工具）与未关联（其他）。
    const chips = await screen.findByTestId("plugin-category-chips");
    expect(chips).toHaveTextContent("效率工具");
    expect(chips).toHaveTextContent("其他");

    // 点「效率工具」：只剩关联该分类的行（rec-1）。
    fireEvent.click(screen.getByTestId("plugin-chip-效率工具"));
    expect(screen.getByTestId("plugin-row-rec-1")).toBeInTheDocument();
    expect(screen.queryByTestId("plugin-row-rec-2")).not.toBeInTheDocument();

    // 点「全部」：恢复全部行。
    fireEvent.click(screen.getByTestId("plugin-chip-all"));
    expect(screen.getByTestId("plugin-row-rec-2")).toBeInTheDocument();
  });

  it("市场段表格化：搜索框按名称/描述过滤 + 分类 chips", async () => {
    mockRoutes();
    render(<PluginWorkbench active />);

    await screen.findByTestId("plugin-row-rec-1");
    fireEvent.click(screen.getByTestId("plugin-segment-market"));

    await screen.findByTestId("plugin-market-row-uuid-p2");
    expect(screen.getByTestId("plugin-market-row-uuid-p1")).toBeInTheDocument();
    expect(screen.getByTestId("plugin-market-row-uuid-p3")).toBeInTheDocument();

    // 搜索「二」：只剩插件二。
    fireEvent.change(screen.getByTestId("plugin-market-search"), {
      target: { value: "二" },
    });
    expect(screen.getByTestId("plugin-market-row-uuid-p2")).toBeInTheDocument();
    expect(screen.queryByTestId("plugin-market-row-uuid-p1")).not.toBeInTheDocument();

    // 清空搜索后按分类筛选：效率工具只剩插件一/二（已装显示「已安装」）。
    fireEvent.change(screen.getByTestId("plugin-market-search"), {
      target: { value: "" },
    });
    fireEvent.click(screen.getByTestId("plugin-chip-效率工具"));
    expect(screen.getByTestId("plugin-market-row-uuid-p1")).toBeInTheDocument();
    expect(screen.getByTestId("plugin-market-row-uuid-p2")).toBeInTheDocument();
    expect(screen.queryByTestId("plugin-market-row-uuid-p3")).not.toBeInTheDocument();
  });

  it("安装零确认：点击安装直接调 install_plugin（无确认弹层）", async () => {
    let installedIds = new Set(["uuid-p1"]);
    mockRoutes({
      get_plugin_tab_state: () => {
        const base = tabState();
        return installedIds.has("uuid-p2")
          ? {
              ...base,
              installed: [
                ...base.installed,
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
              ],
            }
          : base;
      },
      install_plugin: (args) => {
        expect(args).toMatchObject({ pluginId: "uuid-p2", name: "plugin-two" });
        installedIds = new Set([...installedIds, "uuid-p2"]);
        return undefined;
      },
    });
    render(<PluginWorkbench active />);

    await screen.findByTestId("plugin-row-rec-1");
    fireEvent.click(screen.getByTestId("plugin-segment-market"));
    fireEvent.click(await screen.findByTestId("plugin-install-uuid-p2"));

    // 直接安装（零确认），完成后条目变为已安装。
    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith("install_plugin", expect.objectContaining({
        pluginId: "uuid-p2",
      }));
    });
    await waitFor(() => {
      expect(screen.getByTestId("plugin-market-installed-uuid-p2")).toBeVisible();
    });
  });

  it("卸载确认文案列明影响面：N 个账号 + 切号不再带走；确认后调 uninstall_plugin_everywhere", async () => {
    let state = tabState();
    mockRoutes({
      get_plugin_tab_state: () => state,
      uninstall_plugin_everywhere: () => {
        state = {
          ...state,
          installed: state.installed.filter((item) => item.record_id !== "rec-1"),
          manifest: [],
        };
        return {
          accounts: [
            { display_name: "账号甲", removed: true, failed: false, error_code: null },
            { display_name: "账号乙", removed: true, failed: false, error_code: null },
          ],
        };
      },
    });
    render(<PluginWorkbench active />);

    await screen.findByTestId("plugin-uninstall-rec-1");
    fireEvent.click(screen.getByTestId("plugin-uninstall-rec-1"));
    // 确认弹窗：列明「将同时从 N 个账号移除」与切号语义（N=known_account_count）。
    const dialog = screen.getByTestId("plugin-uninstall-confirm");
    expect(dialog).toHaveTextContent("将同时从 2 个账号移除");
    expect(dialog).toHaveTextContent("移除后切换账号不再带走该插件");
    // 未确认不触发卸载。
    expect(mockInvoke).not.toHaveBeenCalledWith(
      "uninstall_plugin_everywhere",
      expect.anything(),
    );

    fireEvent.click(screen.getByTestId("plugin-uninstall-confirm-confirm"));
    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith("uninstall_plugin_everywhere", {
        recordId: "rec-1",
      });
    });
    // 卸载后行消失 + 回执列明账号数。
    await waitFor(() => {
      expect(screen.queryByTestId("plugin-row-rec-1")).not.toBeInTheDocument();
    });
    expect(screen.getByTestId("plugin-notice")).toHaveTextContent("已从 2 个账号移除 插件一");
  });

  it("卸载回执透出部分失败（fail-soft：列明失败账号）", async () => {
    mockRoutes({
      uninstall_plugin_everywhere: () => ({
        accounts: [
          { display_name: "账号甲", removed: true, failed: false, error_code: null },
          { display_name: "账号乙", removed: false, failed: true, error_code: "plugin_propagate_uninstall_failed" },
        ],
      }),
    });
    render(<PluginWorkbench active />);

    await screen.findByTestId("plugin-uninstall-rec-1");
    fireEvent.click(screen.getByTestId("plugin-uninstall-rec-1"));
    fireEvent.click(screen.getByTestId("plugin-uninstall-confirm-confirm"));

    await waitFor(() => {
      expect(screen.getByTestId("plugin-notice")).toHaveTextContent(
        "已从 1 个账号移除 插件一；账号乙 移除失败",
      );
    });
  });

  it("卸载确认弹窗可取消，不触发命令", async () => {
    mockRoutes();
    render(<PluginWorkbench active />);

    await screen.findByTestId("plugin-uninstall-rec-1");
    fireEvent.click(screen.getByTestId("plugin-uninstall-rec-1"));
    fireEvent.click(screen.getByTestId("plugin-uninstall-confirm").querySelector("button")!);
    expect(screen.queryByTestId("plugin-uninstall-confirm")).not.toBeInTheDocument();
    expect(mockInvoke).not.toHaveBeenCalledWith(
      "uninstall_plugin_everywhere",
      expect.anything(),
    );
  });

  it("读取失败显示稳定错误文案（plugin_tab_no_account）", async () => {
    mockInvoke.mockImplementation(async (command: string) => {
      if (command === "get_plugin_tab_state") throw new Error("plugin_tab_no_account");
      if (command === "browse_plugin_market") return MARKET;
      return undefined;
    });
    render(<PluginWorkbench active />);

    const alert = await screen.findByTestId("plugin-error");
    expect(alert).toHaveTextContent("主库当前没有已登录账号");
  });

  it("市场目录读取失败不阻断已装列表（分类列留空，fail-soft）", async () => {
    mockInvoke.mockImplementation(async (command: string) => {
      if (command === "get_plugin_tab_state") return tabState();
      if (command === "browse_plugin_market") throw new Error("plugin_market_unavailable");
      return undefined;
    });
    render(<PluginWorkbench active />);

    const row = await screen.findByTestId("plugin-row-rec-1");
    expect(row).toHaveTextContent("插件一");
    // 无市场数据：无分类 chips（单一条目无分类可筛）。
    expect(screen.queryByTestId("plugin-category-chips")).not.toBeInTheDocument();
  });

  it("active=false 不发起任何读取", () => {
    render(<PluginWorkbench active={false} />);
    expect(mockInvoke).not.toHaveBeenCalled();
  });

  it("已装为空显示空态引导", async () => {
    mockRoutes({
      get_plugin_tab_state: () => ({
        installed: [],
        manifest: [],
        known_account_count: 1,
      }),
    });
    render(<PluginWorkbench active />);

    await screen.findByTestId("plugin-installed-empty");
    expect(screen.getByTestId("plugin-installed-empty")).toHaveTextContent("还没有已装插件");
  });
});
