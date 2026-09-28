import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import { ProductAccountPage } from "../src/components/ProductAccountPage";
import { productDefinitionOf } from "../src/platform/productRegistry";
import type { WorkspaceStateDto } from "../src/types/workspace";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(),
}));

const mockInvoke = vi.mocked(invoke);
const product = productDefinitionOf("trae_cn");
const state: WorkspaceStateDto = {
  platform: {
    platform_id: "trae_cn",
    display_name: "Trae CN",
    adapter_implemented: false,
  },
  data_location: {
    selected: false,
    display_name: null,
    unavailable_reason: "product_authorization_required",
  },
  current_account: {
    detected: false,
    user_fingerprint: null,
    unavailable_reason: "need_authorization",
  },
  history: {
    account_count: 0,
    project_count: 0,
    session_count: 0,
  },
  capabilities: {
    scan_enabled: false,
    sync_enabled: false,
    backup_enabled: false,
    restore_enabled: false,
  },
  honest_status: "当前产品只开放账号适配；跨产品凭据恢复仍需授权。",
};

describe("Trae CN 产品账号页", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    mockInvoke.mockImplementation(async (command: string) => {
      if (command === "get_product_identity_state") {
        return [{
          product_id: "trae_cn",
          display_name: "Trae CN",
          identity_status: "unknown",
          credential_status: "need_authorization",
          relation_to_work: "unknown",
        }];
      }
      if (command === "list_trae_cn_accounts") {
        return [
          {
            profile_id: "cn-a",
            display_name: "CN 主账号",
            avatar_url: "",
            last_verified_at: "2026-09-22T10:00:00Z",
            status: "active",
            is_current: true,
          },
          {
            profile_id: "cn-b",
            display_name: "CN 备用账号",
            avatar_url: "",
            last_verified_at: "2026-09-21T10:00:00Z",
            status: "active",
            is_current: false,
          },
        ];
      }
      if (command === "switch_trae_cn_account") return [];
      throw new Error(`未模拟的命令: ${command}`);
    });
  });

  it("显示独立账号池并调用 CN 切换命令", async () => {
    render(<ProductAccountPage product={product} state={state} onSwitchProduct={() => undefined} />);

    expect(await screen.findByText("CN 主账号")).toBeInTheDocument();
    expect(screen.getByTestId("product-credential-status")).toHaveTextContent("已保存 2 个账号");
    fireEvent.click(screen.getByTestId("trae-cn-switch-account"));

    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith("switch_trae_cn_account", { profileId: "cn-b" });
    });
    expect(await screen.findByRole("status")).toHaveTextContent("当前账号已切换");
  });
});
