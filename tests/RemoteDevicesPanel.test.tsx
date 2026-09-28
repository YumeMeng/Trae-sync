import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { RemoteDevicesPanel } from "../src/components/RemoteDevicesPanel";
import type { RemoteDeviceEntry, RemoteDeviceSnapshot } from "../src/types/product";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

const mockInvoke = vi.mocked(invoke);

/** 构造设备行；未指定的字段给合理默认值。 */
function entry(deviceId: string, overrides: Partial<RemoteDeviceEntry> = {}): RemoteDeviceEntry {
  return {
    device_id: deviceId,
    device_type: "IDE_PC",
    device_name: `设备-${deviceId}`,
    bound_products: ["Trae CN"],
    last_active_at: Date.now() - 60_000,
    is_local: false,
    ...overrides,
  };
}

/** 面板成功快照：一本机一远程（顺序故意乱序，验证排序）。 */
function snapshot(overrides: Partial<RemoteDeviceSnapshot> = {}): RemoteDeviceSnapshot {
  return {
    profile_id: "profile-cn-1",
    account_label: "CN 主账号",
    devices: [
      entry("dev-remote-1", {
        device_type: "MOBILE",
        device_name: "出差的手机",
        bound_products: ["TRAE 移动端"],
        last_active_at: Date.now() - 3 * 3_600_000,
      }),
      entry("dev-local", { device_name: "本机电脑", is_local: true }),
    ],
    used_count: 2,
    max_count: 10,
    ...overrides,
  };
}

/** 展开面板（懒加载：展开才拉取列表）。 */
async function expandPanel() {
  fireEvent.click(screen.getByTestId("remote-devices-toggle"));
  return screen.findByTestId("remote-devices-summary");
}

/** 展开「其他设备」折叠分组（默认折叠，非本机行收纳其中）。 */
function expandOthers() {
  fireEvent.click(screen.getByTestId("remote-devices-others-toggle"));
}

describe("RemoteDevicesPanel", () => {
  beforeEach(() => {
    mockInvoke.mockReset();
  });

  afterEach(() => {
    vi.clearAllMocks();
  });

  it("加载成功：归属标注、汇总行、设备行（产品标签 + 本机徽章）齐全", async () => {
    mockInvoke.mockResolvedValue(snapshot());
    render(
      <RemoteDevicesPanel
        productId="trae_cn"
        productName="Trae CN"
        profileId="profile-cn-1"
        accountLabel=""
      />,
    );
    await expandPanel();

    expect(mockInvoke).toHaveBeenCalledWith("list_remote_devices", {
      productId: "trae_cn",
      profileId: "profile-cn-1",
    });
    // 归属标注：后端 account_label + 产品名（明确是账号级列表）。
    expect(screen.getByText("账号「CN 主账号」 · Trae CN")).toBeInTheDocument();
    expect(screen.getByTestId("remote-devices-summary")).toHaveTextContent("已用 2 / 上限 10 台");
    const localRow = screen.getByTestId("remote-device-row-dev-local");
    // 本机行渲染在本机强调区块内（置顶区块，与其他设备分组区分）。
    expect(screen.getByTestId("remote-devices-local")).toContainElement(localRow);
    expect(localRow).toHaveTextContent("本机电脑");
    expect(localRow).toHaveTextContent("桌面设备");
    expect(localRow).toHaveTextContent("Trae CN");
    expect(localRow).toHaveTextContent("本机");
  });

  it("本机行无退出按钮（ADR-0031 决策 3 UI 防线）；其他设备默认折叠，展开后可见且有退出按钮", async () => {
    mockInvoke.mockResolvedValue(snapshot());
    render(
      <RemoteDevicesPanel
        productId="trae_cn"
        productName="Trae CN"
        profileId="profile-cn-1"
        accountLabel=""
      />,
    );
    await expandPanel();

    // 默认折叠：非本机行及其退出按钮不可见；本机行始终无退出按钮。
    expect(screen.queryByTestId("remote-device-row-dev-remote-1")).not.toBeInTheDocument();
    expect(screen.queryByTestId("remote-device-signout-dev-remote-1")).not.toBeInTheDocument();
    expect(screen.queryByTestId("remote-device-signout-dev-local")).not.toBeInTheDocument();

    // 折叠头：显示数量，aria-expanded=false。
    const toggle = screen.getByTestId("remote-devices-others-toggle");
    expect(toggle).toHaveTextContent("其他设备 (1)");
    expect(toggle).toHaveAttribute("aria-expanded", "false");

    // 展开后：非本机行可见且有退出按钮，aria-expanded 翻转。
    expandOthers();
    expect(toggle).toHaveAttribute("aria-expanded", "true");
    expect(screen.getByTestId("remote-device-row-dev-remote-1")).toBeInTheDocument();
    expect(screen.getByTestId("remote-device-signout-dev-remote-1")).toBeInTheDocument();
    expect(screen.queryByTestId("remote-device-signout-dev-local")).not.toBeInTheDocument();
  });

  it("列表非空但无本机行时，本机区块显示未识别提示（中性，不报错）", async () => {
    mockInvoke.mockResolvedValue(
      snapshot({
        devices: [
          entry("dev-remote-1", {
            device_type: "MOBILE",
            device_name: "出差的手机",
            bound_products: ["TRAE 移动端"],
            last_active_at: Date.now() - 3 * 3_600_000,
          }),
        ],
        used_count: 1,
      }),
    );
    render(
      <RemoteDevicesPanel
        productId="trae_cn"
        productName="Trae CN"
        profileId="profile-cn-1"
        accountLabel=""
      />,
    );
    await expandPanel();

    // 无 is_local 行：显示中性空态提示；其他设备仍收纳在折叠分组中。
    expect(screen.getByTestId("remote-devices-local-empty")).toHaveTextContent(
      "未识别到本机对应的设备",
    );
    expect(screen.queryByTestId("remote-device-row-dev-local")).not.toBeInTheDocument();
    expect(screen.getByTestId("remote-devices-others-toggle")).toHaveTextContent(
      "其他设备 (1)",
    );
  });

  it("max_count 未知时汇总只显示已用台数", async () => {
    mockInvoke.mockResolvedValue(snapshot({ max_count: null }));
    render(
      <RemoteDevicesPanel
        productId="trae_cn"
        productName="Trae CN"
        profileId="profile-cn-1"
        accountLabel=""
      />,
    );
    await expandPanel();

    expect(screen.getByTestId("remote-devices-summary")).toHaveTextContent("已用 2 台");
    expect(screen.getByTestId("remote-devices-summary")).not.toHaveTextContent("上限");
  });

  it("排序：最近活跃倒序，缺失或不可解析的时刻沉底（其他设备分组内生效）", async () => {
    mockInvoke.mockResolvedValue(
      snapshot({
        devices: [
          entry("dev-null", { last_active_at: null }),
          entry("dev-bad", { last_active_at: "not-a-date" }),
          entry("dev-old", { last_active_at: "2020-01-01 10:00:00" }),
          entry("dev-new", { last_active_at: Date.now() - 60_000 }),
        ],
      }),
    );
    render(
      <RemoteDevicesPanel
        productId="trae_cn"
        productName="Trae CN"
        profileId="profile-cn-1"
        accountLabel=""
      />,
    );
    await expandPanel();
    // 全部为非本机行：先展开「其他设备」分组再校验组内顺序。
    expandOthers();

    const rows = screen.getAllByTestId(/^remote-device-row-/);
    expect(rows.map((row) => row.getAttribute("data-testid"))).toEqual([
      "remote-device-row-dev-new",
      "remote-device-row-dev-old",
      // 两个沉底行保持原相对顺序（稳定排序）。
      "remote-device-row-dev-null",
      "remote-device-row-dev-bad",
    ]);
    // 未知设备类型不透传内部枚举。
    expect(screen.getByTestId("remote-device-row-dev-new")).toHaveTextContent("桌面设备");
  });

  it("未知设备类型与缺失名称的降级展示：其他设备 / 未命名设备", async () => {
    mockInvoke.mockResolvedValue(
      snapshot({
        devices: [
          entry("dev-odd", { device_type: "SOMETHING_ELSE", device_name: null, is_local: true }),
        ],
        used_count: 1,
      }),
    );
    render(
      <RemoteDevicesPanel
        productId="trae_cn"
        productName="Trae CN"
        profileId="profile-cn-1"
        accountLabel="本地备注名"
      />,
    );
    await expandPanel();

    const row = screen.getByTestId("remote-device-row-dev-odd");
    expect(row).toHaveTextContent("未命名设备");
    expect(row).toHaveTextContent("其他设备");
    // 父层传入的显示名优先于后端 account_label。
    expect(screen.getByText("账号「本地备注名」 · Trae CN")).toBeInTheDocument();
  });

  it("加载失败：映射为可理解文案并支持重试", async () => {
    mockInvoke.mockRejectedValue("remote_device_network");
    render(
      <RemoteDevicesPanel
        productId="trae_cn"
        productName="Trae CN"
        profileId="profile-cn-1"
        accountLabel=""
      />,
    );
    fireEvent.click(screen.getByTestId("remote-devices-toggle"));

    expect(await screen.findByTestId("remote-devices-error")).toHaveTextContent(
      "网络或服务暂时不可用，请稍后重试。",
    );
    // 重试成功后恢复列表。
    mockInvoke.mockResolvedValue(snapshot());
    fireEvent.click(screen.getByTestId("remote-devices-retry"));
    expect(await screen.findByTestId("remote-devices-summary")).toHaveTextContent("已用 2 / 上限 10 台");
  });

  it("失败态提供官方设备管理页兜底入口（ADR-0031 决策 2）", async () => {
    mockInvoke.mockRejectedValue("remote_device_network");
    render(
      <RemoteDevicesPanel
        productId="trae_cn"
        productName="Trae CN"
        profileId="profile-cn-1"
        accountLabel=""
      />,
    );
    fireEvent.click(screen.getByTestId("remote-devices-toggle"));

    const docsButton = await screen.findByTestId("remote-devices-official-docs");
    expect(docsButton).toHaveTextContent("打开官方设备管理页");
    // 点击触发后端打开命令；说明文案告知实际管理入口。
    fireEvent.click(docsButton);
    await waitFor(() =>
      expect(mockInvoke).toHaveBeenCalledWith("open_device_management_docs"),
    );
    expect(screen.getByText("可在 TRAE 官方客户端或网页登录页管理设备。")).toBeInTheDocument();
  });

  it("切换账号：面板重置为折叠并清空快照，再次展开按新账号重新拉取", async () => {
    mockInvoke.mockResolvedValue(snapshot());
    const view = render(
      <RemoteDevicesPanel
        productId="trae_cn"
        productName="Trae CN"
        profileId="profile-cn-1"
        accountLabel=""
      />,
    );
    await expandPanel();
    expect(screen.getByTestId("remote-devices-summary")).toBeInTheDocument();

    // 切换归属账号：面板重置为折叠，旧快照不再显示。
    view.rerender(
      <RemoteDevicesPanel
        productId="trae_cn"
        productName="Trae CN"
        profileId="profile-cn-2"
        accountLabel=""
      />,
    );
    expect(screen.queryByTestId("remote-devices-summary")).not.toBeInTheDocument();
    expect(screen.getByTestId("remote-devices-toggle")).toHaveTextContent("查看设备");

    // 再次展开：以新 profileId 重新拉取，而不是复用旧账号快照。
    mockInvoke.mockResolvedValue(snapshot({ account_label: "CN 备用账号" }));
    await expandPanel();
    expect(mockInvoke).toHaveBeenLastCalledWith("list_remote_devices", {
      productId: "trae_cn",
      profileId: "profile-cn-2",
    });
    expect(screen.getByText("账号「CN 备用账号」 · Trae CN")).toBeInTheDocument();
  });

  it("切换账号后飞行中的旧账号响应被丢弃，不滞留旧快照", async () => {
    let resolveStale!: (value: RemoteDeviceSnapshot) => void;
    mockInvoke.mockImplementationOnce(
      () =>
        new Promise<RemoteDeviceSnapshot>((resolve) => {
          resolveStale = resolve;
        }),
    );
    const view = render(
      <RemoteDevicesPanel
        productId="trae_cn"
        productName="Trae CN"
        profileId="profile-cn-1"
        accountLabel=""
      />,
    );
    fireEvent.click(screen.getByTestId("remote-devices-toggle"));

    // 请求飞行中切换账号：面板重置；随后旧账号响应才到达。
    view.rerender(
      <RemoteDevicesPanel
        productId="trae_cn"
        productName="Trae CN"
        profileId="profile-cn-2"
        accountLabel=""
      />,
    );
    await act(async () => {
      resolveStale(snapshot());
    });

    // 过期结果不写入：再次展开以新账号重新拉取，看不到旧账号数据。
    mockInvoke.mockResolvedValue(snapshot({ account_label: "CN 备用账号" }));
    await expandPanel();
    expect(mockInvoke).toHaveBeenLastCalledWith("list_remote_devices", {
      productId: "trae_cn",
      profileId: "profile-cn-2",
    });
    expect(screen.getByText("账号「CN 备用账号」 · Trae CN")).toBeInTheDocument();
  });

  it("确认弹窗：文案含影响说明；取消不调用退出", async () => {
    const signOut = vi.fn().mockResolvedValue(undefined);
    mockInvoke.mockResolvedValue(snapshot());
    render(
      <RemoteDevicesPanel
        productId="trae_cn"
        productName="Trae CN"
        profileId="profile-cn-1"
        accountLabel=""
        onSignOut={signOut}
      />,
    );
    await expandPanel();
    // 退出按钮在「其他设备」折叠分组内：先展开再操作。
    expandOthers();

    fireEvent.click(screen.getByTestId("remote-device-signout-dev-remote-1"));
    const dialog = screen.getByTestId("remote-device-signout-confirm");
    expect(dialog).toHaveTextContent("退出这台设备？");
    expect(dialog).toHaveTextContent("设备：出差的手机（移动设备）");
    expect(dialog).toHaveTextContent("绑定产品：TRAE 移动端");
    expect(dialog).toHaveTextContent("退出后，这台设备上登录的全部 TRAE 产品都需要重新登录。");

    // 取消：不调用退出动作，弹层关闭。
    fireEvent.click(screen.getByTestId("remote-device-signout-confirm-cancel"));
    await waitFor(() =>
      expect(screen.queryByTestId("remote-device-signout-confirm")).not.toBeInTheDocument(),
    );
    expect(signOut).not.toHaveBeenCalled();
  });

  it("确认退出：执行中 busy 态；成功后重拉设备列表", async () => {
    let resolveSignOut!: () => void;
    const signOut = vi.fn(
      () => new Promise<void>((resolve) => {
        resolveSignOut = resolve;
      }),
    );
    mockInvoke.mockResolvedValue(snapshot());
    render(
      <RemoteDevicesPanel
        productId="trae_cn"
        productName="Trae CN"
        profileId="profile-cn-1"
        accountLabel=""
        onSignOut={signOut}
      />,
    );
    await expandPanel();
    // 退出按钮在「其他设备」折叠分组内：先展开再操作。
    expandOthers();

    fireEvent.click(screen.getByTestId("remote-device-signout-dev-remote-1"));
    fireEvent.click(screen.getByTestId("remote-device-signout-confirm-confirm"));
    // busy：确认按钮转执行中文案并禁用，防重复提交。
    expect(signOut).toHaveBeenCalledWith("dev-remote-1");
    const confirmButton = screen.getByTestId("remote-device-signout-confirm-confirm");
    expect(confirmButton).toHaveTextContent("正在退出…");
    expect(confirmButton).toBeDisabled();

    await act(async () => {
      resolveSignOut();
    });
    // 成功后弹层关闭 + 列表重拉（initial + 刷新 = 两次 list_remote_devices）。
    await waitFor(() =>
      expect(screen.queryByTestId("remote-device-signout-confirm")).not.toBeInTheDocument(),
    );
    await waitFor(() => {
      const listCalls = mockInvoke.mock.calls.filter((call) => call[0] === "list_remote_devices");
      expect(listCalls.length).toBe(2);
    });
    expect(screen.getByText("已退出该设备，列表已刷新。")).toBeInTheDocument();
  });

  it("默认退出实现调用 clear_remote_device 命令（切片 3 接入后端）", async () => {
    mockInvoke.mockResolvedValue(snapshot());
    render(
      <RemoteDevicesPanel
        productId="trae_cn"
        productName="Trae CN"
        profileId="profile-cn-1"
        accountLabel=""
      />,
    );
    await expandPanel();
    // 退出按钮在「其他设备」折叠分组内：先展开再操作。
    expandOthers();

    fireEvent.click(screen.getByTestId("remote-device-signout-dev-remote-1"));
    fireEvent.click(screen.getByTestId("remote-device-signout-confirm-confirm"));
    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith("clear_remote_device", {
        productId: "trae_cn",
        profileId: "profile-cn-1",
        deviceId: "dev-remote-1",
      });
    });
  });

  it("退出成功且列表重拉完成后 onSignedOut 触发一次（健康检查联动挂点）", async () => {
    const onSignedOut = vi.fn();
    // 默认 onSignOut 链路：clear_remote_device 成功返回后重拉列表。
    mockInvoke.mockImplementation(async (cmd: string) => {
      if (cmd === "list_remote_devices") return snapshot();
      return null; // clear_remote_device：服务端操作，成功无返回值
    });
    render(
      <RemoteDevicesPanel
        productId="trae_cn"
        productName="Trae CN"
        profileId="profile-cn-1"
        accountLabel=""
        onSignedOut={onSignedOut}
      />,
    );
    await expandPanel();
    // 退出按钮在「其他设备」折叠分组内：先展开再操作。
    expandOthers();

    fireEvent.click(screen.getByTestId("remote-device-signout-dev-remote-1"));
    fireEvent.click(screen.getByTestId("remote-device-signout-confirm-confirm"));
    // 触发时机在列表重拉完成之后：notice 出现即代表整个成功链路走完。
    await waitFor(() =>
      expect(screen.getByText("已退出该设备，列表已刷新。")).toBeInTheDocument(),
    );
    expect(onSignedOut).toHaveBeenCalledTimes(1);
    // ADR-0031 决策 6：联动携带面板归属账号，父层只检测该账号。
    expect(onSignedOut).toHaveBeenCalledWith("profile-cn-1");
  });

  it("退出失败：弹层关闭后在面板展示映射文案，可重试", async () => {
    const signOut = vi.fn().mockRejectedValue("remote_device_local_device_targeted");
    mockInvoke.mockResolvedValue(snapshot());
    render(
      <RemoteDevicesPanel
        productId="trae_cn"
        productName="Trae CN"
        profileId="profile-cn-1"
        accountLabel=""
        onSignOut={signOut}
      />,
    );
    await expandPanel();
    // 退出按钮在「其他设备」折叠分组内：先展开再操作。
    expandOthers();

    fireEvent.click(screen.getByTestId("remote-device-signout-dev-remote-1"));
    fireEvent.click(screen.getByTestId("remote-device-signout-confirm-confirm"));
    expect(await screen.findByRole("alert")).toHaveTextContent(
      "不能通过工具退出正在使用的本机设备。",
    );
  });
});
