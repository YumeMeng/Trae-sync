import { fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { AdapterAccountDetail } from "../src/components/AdapterAccountDetail";
import type { AccountView } from "../src/platform/accountAdapter";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

const mockInvoke = vi.mocked(invoke);

/** 适配器账号视图：默认非当前账号（当前账号用例单独覆盖）。 */
function account(overrides: Partial<AccountView> = {}): AccountView {
  return {
    id: "cn-a",
    displayName: "CN 主账号",
    status: "active",
    lastVerifiedAt: "2026-09-01T08:00:00.000Z",
    isCurrent: false,
    ...overrides,
  };
}

function renderDetail(overrides: {
  account?: AccountView;
  canSwitch?: boolean;
  onSignedOut?: () => void;
} = {}) {
  const onBack = vi.fn();
  const onSwitch = vi.fn();
  const onSignedOut = overrides.onSignedOut ?? vi.fn();
  render(
    <AdapterAccountDetail
      account={overrides.account ?? account()}
      productId="trae_cn"
      productName="Trae CN"
      canSwitch={overrides.canSwitch ?? true}
      switchBusy={false}
      onSwitch={onSwitch}
      onSignedOut={onSignedOut}
      onBack={onBack}
    />,
  );
  return { onBack, onSwitch, onSignedOut };
}

describe("AdapterAccountDetail", () => {
  beforeEach(() => {
    mockInvoke.mockReset();
  });

  afterEach(() => vi.clearAllMocks());

  it("基础信息渲染：显示名标题、状态徽章、当前使用标记、最近验证", () => {
    renderDetail();
    expect(screen.getByRole("heading", { name: "CN 主账号" })).toBeInTheDocument();
    expect(screen.getByTestId("adapter-account-detail-status")).toHaveTextContent("登录有效");
    expect(screen.getByTestId("adapter-account-detail-current")).toHaveTextContent("否");
    // 展示名作为归属标注传入设备面板。
    expect(screen.getByTestId("adapter-account-detail-verified")).toHaveTextContent("2026/09/01");
  });

  it("设备模块挂载：默认折叠未触网，展开后按详情账号拉取列表", async () => {
    mockInvoke.mockResolvedValue({
      profile_id: "cn-a",
      account_label: "CN 主账号",
      used_count: 1,
      max_count: 10,
      devices: [],
    });
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

  it("切换操作区：仅非当前账号且具备 switch 能力时出现", () => {
    const { onSwitch } = renderDetail();
    expect(screen.getByTestId("adapter-account-detail-switch")).toBeInTheDocument();

    fireEvent.click(screen.getByTestId("adapter-account-detail-switch"));
    expect(onSwitch).toHaveBeenCalledTimes(1);
  });

  it("当前账号不渲染切换按钮，但保留「使用中」标记", () => {
    renderDetail({ account: account({ isCurrent: true }) });
    expect(screen.queryByTestId("adapter-account-detail-switch")).not.toBeInTheDocument();
    expect(screen.getByTestId("adapter-account-detail-current")).toHaveTextContent("使用中");
  });

  it("适配器无 switch 能力时不渲染切换操作区", () => {
    renderDetail({ canSwitch: false });
    expect(screen.queryByTestId("adapter-account-detail-switch")).not.toBeInTheDocument();
  });

  it("状态徽章三态：过期账号显示「登录已过期」", () => {
    renderDetail({ account: account({ status: "expired" }) });
    expect(screen.getByTestId("adapter-account-detail-status")).toHaveTextContent("登录已过期");
  });
});
