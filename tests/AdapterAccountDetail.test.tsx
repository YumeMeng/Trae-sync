import { fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { AdapterAccountDetail } from "../src/components/AdapterAccountDetail";
import type { AccountView } from "../src/platform/accountAdapter";
import { installFakeInvoke, mockRoute, resetRoutes } from "./helpers/fakeInvoke";

// P12-R1c 示范：共享 fakeInvoke 路由器。工厂必须内联 async + 动态 import
//（Vitest 工厂不能引用文件内 import 绑定，时序会错），invoke 替身指向共享
// mockCoreInvoke 单例；未注册命令 fail-fast 抛错（详见 tests/helpers/fakeInvoke.ts 头注释）。
vi.mock("@tauri-apps/api/core", async () => {
  const fake = await import("./helpers/fakeInvoke");
  return { invoke: fake.mockCoreInvoke };
});

const mockInvoke = vi.mocked(invoke);

/** 适配器账号视图：默认未在工具内选中（已选账号用例单独覆盖）。 */
function account(overrides: Partial<AccountView> = {}): AccountView {
  return {
    id: "cn-a",
    displayName: "CN 主账号",
    status: "active",
    lastVerifiedAt: "2026-09-01T08:00:00.000Z",
    isToolSelected: false,
    ...overrides,
  };
}

function renderDetail(overrides: {
  account?: AccountView;
  canSelectToolAccount?: boolean;
  onSignedOut?: () => void;
} = {}) {
  const onBack = vi.fn();
  const onSelectToolAccount = vi.fn();
  const onSignedOut = overrides.onSignedOut ?? vi.fn();
  render(
    <AdapterAccountDetail
      account={overrides.account ?? account()}
      productId="trae_cn"
      productName="Trae CN"
      canSelectToolAccount={overrides.canSelectToolAccount ?? true}
      toolSelectionBusy={false}
      onSelectToolAccount={onSelectToolAccount}
      onSignedOut={onSignedOut}
      onBack={onBack}
    />,
  );
  return { onBack, onSelectToolAccount, onSignedOut };
}

describe("AdapterAccountDetail", () => {
  beforeEach(() => {
    // 等价于原 mockInvoke.mockReset()：清调用历史 + 重装按路由表分发的实现。
    resetRoutes();
    installFakeInvoke();
  });

  afterEach(() => vi.clearAllMocks());

  it("基础信息渲染：显示名标题、状态徽章、工具选择状态、最近验证", () => {
    renderDetail();
    expect(screen.getByRole("heading", { name: "CN 主账号" })).toBeInTheDocument();
    expect(screen.getByTestId("adapter-account-detail-status")).toHaveTextContent("登录有效");
    expect(screen.getByTestId("adapter-account-detail-current")).toHaveTextContent("工具内未选");
    // 展示名作为归属标注传入设备面板。
    expect(screen.getByTestId("adapter-account-detail-verified")).toHaveTextContent("2026/09/01");
    const commonFields = [
      screen.getByTestId("adapter-account-detail-status"),
      screen.getByTestId("adapter-account-detail-current"),
      screen.getByTestId("adapter-account-detail-verified"),
    ];
    for (let index = 0; index < commonFields.length - 1; index += 1) {
      expect(
        commonFields[index].compareDocumentPosition(commonFields[index + 1]) & Node.DOCUMENT_POSITION_FOLLOWING,
      ).not.toBe(0);
    }
  });

  it("设备模块挂载：默认折叠未触网，展开后按详情账号拉取列表", async () => {
    // 原 mockInvoke.mockResolvedValue({...}) 的路由器等价写法：按命令注册响应。
    mockRoute("list_remote_devices", () => ({
      profile_id: "cn-a",
      account_label: "CN 主账号",
      used_count: 1,
      max_count: 10,
      devices: [],
    }));
    renderDetail();

    expect(screen.getByTestId("remote-devices-panel")).toBeInTheDocument();
    expect(mockInvoke).not.toHaveBeenCalled();

    fireEvent.click(screen.getByTestId("remote-devices-toggle"));
    expect(await screen.findByTestId("remote-devices-summary")).toHaveTextContent("已用 1 / 上限 10 台");
    expect(mockInvoke).toHaveBeenCalledWith("list_remote_devices", {
      productId: "trae_cn",
      profileId: "cn-a",
    });
  });

  it("返回按钮：点击后回调 onBack", () => {
    const { onBack } = renderDetail();
    fireEvent.click(screen.getByTestId("adapter-account-detail-back"));
    expect(onBack).toHaveBeenCalledTimes(1);
  });

  it("工具账号选择区：仅未选账号且具备能力时出现", () => {
    const { onSelectToolAccount } = renderDetail();
    const button = screen.getByTestId("adapter-account-detail-tool-select");
    expect(button).toHaveTextContent("设置工具当前账号");
    expect(button).toHaveAttribute("title", expect.stringContaining("不会改变官方 TRAE 客户端登录状态"));

    fireEvent.click(button);
    expect(onSelectToolAccount).toHaveBeenCalledTimes(1);

    const actionSection = button.closest("section");
    const deviceSection = screen.getByTestId("remote-devices-panel");
    const technicalDetails = screen.getByText("技术细节").closest("details");
    const orderedSections = [actionSection, deviceSection, technicalDetails];
    expect(orderedSections.every((section) => section !== null)).toBe(true);
    for (let index = 0; index < orderedSections.length - 1; index += 1) {
      const current = orderedSections[index];
      const next = orderedSections[index + 1];
      expect(current!.compareDocumentPosition(next!) & Node.DOCUMENT_POSITION_FOLLOWING).not.toBe(0);
    }
  });

  it("工具内已选账号不渲染选择按钮，但保留选择标记", () => {
    renderDetail({ account: account({ isToolSelected: true }) });
    expect(screen.queryByTestId("adapter-account-detail-tool-select")).not.toBeInTheDocument();
    expect(screen.getByTestId("adapter-account-detail-current")).toHaveTextContent("工具内已选");
  });

  it("适配器无工具账号选择能力时不渲染操作区", () => {
    renderDetail({ canSelectToolAccount: false });
    expect(screen.queryByTestId("adapter-account-detail-tool-select")).not.toBeInTheDocument();
  });

  it("状态徽章三态：过期账号显示「登录已过期」", () => {
    renderDetail({ account: account({ status: "expired" }) });
    expect(screen.getByTestId("adapter-account-detail-status")).toHaveTextContent("登录已过期");
  });
});
