import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import { AccountWorkbench } from "../src/components/AccountWorkbench";
import { accountAdapterFor, createTraeCnAccountAdapter } from "../src/platform/accountAdapter";
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
    mockRoute("begin_trae_cn_login", () => ({ login_url: "https://example.test/login" }));
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
    mockRoute("refresh_trae_cn_credentials", () => [
      { profile_id: "cn-main", screen_name: "CN 主账号", refreshed: true, error_code: null },
      { profile_id: "cn-backup", screen_name: "CN 备用账号", refreshed: false, error_code: "credential_refresh_failed" },
    ]);
  });

  it("通过产品 ID 路由到 CN 适配器，并保留 Work 的渐进迁移入口", () => {
    expect(accountAdapterFor("trae_cn")?.productId).toBe("trae_cn");
    // Work 仍由 AccountCenter 保留成熟事务；迁移完成前不伪造一个空适配器。
    expect(accountAdapterFor("work_cn")).toBeNull();
  });

  it("把 CN DTO 映射为统一快照并只声明真实账号能力", async () => {
    const snapshot = await createTraeCnAccountAdapter().loadSnapshot();

    expect(snapshot.accounts).toEqual([
      expect.objectContaining({ id: "cn-main", displayName: "CN 主账号", isCurrent: true }),
      expect.objectContaining({ id: "cn-backup", displayName: "CN 备用账号", isCurrent: false }),
    ]);
    expect(snapshot.currentAccountId).toBe("cn-main");
    expect(snapshot.capabilities).toEqual([
      "login",
      "relogin",
      "switch",
      "health",
      "refreshCredential",
      "remoteDevices",
    ]);
    expect(snapshot.identityMessage).toContain("相同的 TRAE 身份");
  });

  it("把 CN 登录流程保持在适配器内", async () => {
    const adapter = createTraeCnAccountAdapter();

    await expect(adapter.login()).resolves.toEqual({
      displayName: "CN 新账号",
      acceptance: "passed",
      acceptanceErrorCode: null,
    });
    await adapter.cancelLogin();

    expect(mockInvoke).toHaveBeenCalledWith("begin_trae_cn_login", { useSystemBrowser: true });
    expect(mockInvoke).toHaveBeenCalledWith("complete_trae_cn_login");
    expect(mockInvoke).toHaveBeenCalledWith("cancel_trae_cn_login");
  });

  it("Trae CN 使用同一账号工作台，并隐藏 Work 专有操作", async () => {
    render(<AccountWorkbench active adapter={createTraeCnAccountAdapter()} />);

    expect(await screen.findByRole("region", { name: "Trae CN 账号" })).toBeInTheDocument();
    expect(screen.getByText("CN 主账号")).toBeInTheDocument();
    expect(screen.getByText("CN 备用账号")).toBeInTheDocument();
    expect(screen.getByTestId("account-health-check")).toBeInTheDocument();
    expect(screen.queryByTestId("account-refresh-credits")).not.toBeInTheDocument();
    expect(screen.getByTestId("account-refresh-credentials")).toBeInTheDocument();

    const card = screen.getByTestId("account-adapter-card-cn-backup");
    expect(card.querySelector(".account-item__main")).not.toBeNull();
    expect(card.querySelector(".account-item__side")).not.toBeNull();
    expect(card.querySelector(".slot-badge")).not.toBeNull();
    expect(card).toHaveTextContent("切换到此账号");
    // CN 没有详情能力时保持静态条目，不能让悬停样式制造假的可点击暗示。
    expect(card).not.toHaveClass("account-card--clickable");

    fireEvent.click(screen.getByTestId("account-health-check"));
    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith("check_trae_cn_account_health", {
        profileIds: ["cn-main", "cn-backup"],
      });
    });

    fireEvent.click(screen.getByTestId("account-refresh-credentials"));
    fireEvent.click(screen.getByTestId("account-refresh-credentials-confirm-confirm"));
    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith("refresh_trae_cn_credentials", {
        profileIds: ["cn-main", "cn-backup"],
      });
    });

    fireEvent.click(screen.getByTestId("account-adapter-switch"));
    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith("switch_trae_cn_account", { profileId: "cn-backup" });
    });
  });
});
