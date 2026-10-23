import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import { AccountsPage } from "../src/features/accounts/AccountsPage";
import { accountAdapterFor, createTraeCnAccountAdapter } from "../src/platform/accountAdapter";
import { CURRENT_PRODUCT_ID } from "../src/types/product";
import { installFakeInvoke, mockRoute, resetRoutes } from "./helpers/fakeInvoke";

// P13-5：共享 fakeInvoke 路由器（迁移模式见 tests/helpers/fakeInvoke.ts 头注释）。
vi.mock("@tauri-apps/api/core", async () => {
  const fake = await import("./helpers/fakeInvoke");
  return { invoke: fake.mockCoreInvoke };
});
// P4c-9a：合并页订阅批量进度事件；jsdom 无 Tauri IPC，listen 给空退订函数即可。
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn().mockResolvedValue(() => undefined) }));

const mockInvoke = vi.mocked(invoke);

/** 合并页 TRAE 读取四命令桩（池面测试只需它们不抛错；行数据全部来自 adapterPool）。 */
function installTraeReadRoutes(): void {
  mockRoute("get_checkin_capability", () => ({ enabled: true, transport: "fixture", real_http_enabled: false, message: "" }));
  mockRoute("get_checkin_overview", () => []);
  mockRoute("get_environment_state", () => ({
    env_id: "master", current_profile_id: null, current_account_name: null, running: false,
    observed_profile_id: null, data_dir: "C:\\data\\master",
    login_state: "logged_out", created_at_unix_seconds: 1750000000 }));
  mockRoute("get_managed_account_state", () => ({
    saved_accounts: [], switch_state: null, handoff_intent: null, history_is_separate: true,
    current_account: { profile_id: null, display_name: null, region: null, data_location_id: null, verification_state: "unknown", observed_at: null, reason: null },
    recent_verification: { verification_state: "unknown", checked_at: null, reason: null } }));
}

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
    // P4c-9a：旧 AccountCenterTrae 已删除，适配器面改由合并页的 adapterPool 注入口承载
    // （K-02：适配器池只在测试注入路径存在）。合并页的 TRAE 读取四命令同样需要桩。
    installTraeReadRoutes();
    const adapter = createTraeCnAccountAdapter();
    const pool = await adapter.loadSnapshot();
    render(<AccountsPage active={true} productId="trae_cn" adapterPool={pool} />);

    // 合并页 region 文案统一「账号」（旧适配器面曾写「TRAE 账号」，差异记 K-03）。
    // 账号行异步到位：必须等行本身，不能拿外壳出现当数据就绪（P4c-QA-1 纪律）。
    expect(await screen.findByTestId("account-adapter-card-cn-backup")).toBeInTheDocument();
    expect(screen.getByRole("region", { name: /^账号$/ })).toBeInTheDocument();
    expect(screen.getByTestId("account-adapter-card-cn-main")).toBeInTheDocument();
    // 池面页头：本地刷新 + 健康检测在册；Work 专有的额度/凭据批量入口不出现。
    expect(screen.getByTestId("account-health-check")).toBeInTheDocument();
    expect(screen.queryByTestId("account-refresh-credits")).not.toBeInTheDocument();
    // P15-7（ADR-0035 决策 7）：登录代次由融合版客户端自动续期，
    // 凭据刷新是 Work 签到档案专有操作，融合版账号页不出现该入口。
    expect(screen.queryByTestId("account-refresh-credentials")).not.toBeInTheDocument();

    const card = screen.getByTestId("account-adapter-card-cn-backup");
    expect(card.querySelector(".account-item__main")).not.toBeNull();
    expect(card.querySelector(".account-item__side")).not.toBeNull();
    // 徽章文案由 builder 给（旧 .slot-badge 类 → 统一 Badge 族，差异记 K-03）：
    // active = 登录有效；unknown = 状态待验证（不臆测成有效）。
    expect(card).toHaveTextContent("状态待验证");
    expect(card).toHaveTextContent("设为工具当前账号");
    expect(screen.getByTestId("account-adapter-card-cn-main")).toHaveTextContent("登录有效");

    fireEvent.click(screen.getByTestId("account-adapter-tool-select"));
    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith("switch_trae_cn_account", { profileId: "cn-backup" });
    });
  });
});
