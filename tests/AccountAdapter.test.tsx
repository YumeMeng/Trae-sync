import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import { AccountCenter } from "../src/components/product/trae/AccountCenterTrae";
import { accountAdapterFor, createTraeCnAccountAdapter } from "../src/platform/accountAdapter";
import { CURRENT_PRODUCT_ID } from "../src/types/product";
import { installFakeInvoke, mockRoute, resetRoutes } from "./helpers/fakeInvoke";

// P13-5：共享 fakeInvoke 路由器（迁移模式见 tests/helpers/fakeInvoke.ts 头注释）。
vi.mock("@tauri-apps/api/core", async () => {
  const fake = await import("./helpers/fakeInvoke");
  return { invoke: fake.mockCoreInvoke };
});

const mockInvoke = vi.mocked(invoke);

describe("账号适配器与共享工作台", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    resetRoutes();
    installFakeInvoke();
    mockRoute("get_product_identity_state", () => [{
      product_id: "trae_cn",
      display_name: "Trae CN",
      identity_status: "recognized",
      credential_status: "need_authorization",
      relation_to_work: "same_identity",
    }]);
    mockRoute("list_trae_cn_accounts", () => [
      {
        profile_id: "cn-main",
        display_name: "CN 主账号",
        avatar_url: "",
        last_verified_at: "2026-09-22T10:00:00Z",
        status: "active",
        is_current: true,
      },
      {
        profile_id: "cn-backup",
        display_name: "CN 备用账号",
        avatar_url: "",
        last_verified_at: null,
        status: "unknown",
        is_current: false,
      },
    ]);
    mockRoute("begin_trae_cn_login", () => ({ login_url: "https://example.test/login", attempt_id: 23 }));
    mockRoute("complete_trae_cn_login", () => ({
      profile_id: "cn-new",
      screen_name: "CN 新账号",
      avatar_url: "",
      acceptance: "passed",
    }));
    mockRoute("cancel_trae_cn_login", () => undefined);
    mockRoute("switch_trae_cn_account", () => []);
    mockRoute("check_trae_cn_account_health", () => [
      { profile_id: "cn-main", screen_name: "CN 主账号", healthy: true, error_code: null },
      { profile_id: "cn-backup", screen_name: "CN 备用账号", healthy: false, error_code: "credential_invalid" },
    ]);
  });

  it("按协议值路由到 CN 适配器（单产品塌缩后 Work 仍由 AccountCenter 主路径承载）", () => {
    expect(accountAdapterFor(CURRENT_PRODUCT_ID)?.productId).toBe(CURRENT_PRODUCT_ID);
    // 完整账号事务（官方切号/凭据/额度）由 AccountCenter 主路径保留；非当前
    // 协议值不伪造适配器。
    expect(accountAdapterFor("work_cn")).toBeNull();
  });

  it("把 CN DTO 映射为统一快照并明确工具内账号选择语义", async () => {
    const adapter = createTraeCnAccountAdapter();
    const snapshot = await adapter.loadSnapshot();

    expect(snapshot.productId).toBe(CURRENT_PRODUCT_ID);
    expect(snapshot.accounts).toEqual([
      expect.objectContaining({ id: "cn-main", displayName: "CN 主账号", isToolSelected: true }),
      expect.objectContaining({ id: "cn-backup", displayName: "CN 备用账号", isToolSelected: false }),
    ]);
    expect(adapter.capabilities).toEqual([
      "login",
      "relogin",
      "toolSelection",
      "health",
      "remoteDevices",
    ]);
    expect(snapshot.officialIdentityLabel).toBe("官方客户端身份：已识别");
    // P15-10 单产品塌缩：身份观察只描述本机客户端证据状态，不再表述 Work CN 关系。
    expect(snapshot.officialIdentityMessage).toBe("已从本机客户端登录数据读取到官方身份。");
  });

  it("账号选择只更新 Trae Sync 工具自有指针", async () => {
    const adapter = createTraeCnAccountAdapter();

    await adapter.selectToolAccount("cn-backup");

    expect(mockInvoke).toHaveBeenCalledWith("switch_trae_cn_account", { profileId: "cn-backup" });
  });

  it("把 CN 登录流程保持在适配器内", async () => {
    const adapter = createTraeCnAccountAdapter();

    let attemptId = 0;
    await expect(adapter.login((id) => { attemptId = id; })).resolves.toEqual({
      displayName: "CN 新账号",
      acceptance: "passed",
      acceptanceErrorCode: null,
    });
    await adapter.cancelLogin(attemptId);

    expect(mockInvoke).toHaveBeenCalledWith("begin_trae_cn_login", { useSystemBrowser: true });
    expect(mockInvoke).toHaveBeenCalledWith("complete_trae_cn_login", { attemptId: 23 });
    expect(mockInvoke).toHaveBeenCalledWith("cancel_trae_cn_login", { attemptId: 23 });
  });

  it("适配器路径使用同一账号工作台，并隐藏 Work 专有操作（凭据刷新随融合版续期语义移除）", async () => {
    render(<AccountCenter active adapter={createTraeCnAccountAdapter()} />);

    expect(await screen.findByRole("region", { name: "TRAE 账号" })).toBeInTheDocument();
    expect(screen.getByText("CN 主账号")).toBeInTheDocument();
    expect(screen.getByText("CN 备用账号")).toBeInTheDocument();
    expect(screen.getByTestId("account-health-check")).toBeInTheDocument();
    expect(screen.queryByTestId("account-refresh-credits")).not.toBeInTheDocument();
    // P15-7（ADR-0035 决策 7）：登录代次由融合版客户端自动续期，
    // 凭据刷新是 Work 签到档案专有操作，融合版账号页不出现该入口。
    expect(screen.queryByTestId("account-refresh-credentials")).not.toBeInTheDocument();

    const card = screen.getByTestId("account-adapter-card-cn-backup");
    expect(card.querySelector(".account-item__main")).not.toBeNull();
    expect(card.querySelector(".account-item__side")).not.toBeNull();
    expect(card.querySelector(".slot-badge")).not.toBeNull();
    expect(card).toHaveTextContent("设为工具当前账号");
    // CN 没有详情能力时保持静态条目，不能让悬停样式制造假的可点击暗示。
    expect(card).not.toHaveClass("account-card--clickable");

    fireEvent.click(screen.getByTestId("account-health-check"));
    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith("check_trae_cn_account_health", {
        profileIds: ["cn-main", "cn-backup"],
      });
    });

    fireEvent.click(screen.getByTestId("account-adapter-tool-select"));
    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith("switch_trae_cn_account", { profileId: "cn-backup" });
    });
  });
});
