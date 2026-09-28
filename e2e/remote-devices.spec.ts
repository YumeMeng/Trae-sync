import { test, expect } from "@playwright/test";
import { installMockBridge } from "./mock-bridge";

// ============================================================================
// 登录设备面板 e2e（ADR-0031，P10-5 切片 3 / 结构调整）
// ============================================================================
//
// 全程走 mock-bridge（不触网、不启动 Tauri）。P10-5 起登录设备绑定在
// 账号详情页：入口路径为 Trae CN 账号列表 → 点击账号卡片进入详情 →
// 展开登录设备。fixture：
// - traeCnAccounts 场景让 Trae CN 账号页有一个可进入详情的账号
//   （与 list_remote_devices 的归属标注同源）；
// - list_remote_devices 返回两行设备（一本机一非本机）；
// - clear_remote_device 对本机目标抛 remote_device_local_device_targeted
//   （后端拒绝防线模拟），对其他设备从 fixture 删除（下次列表少一行）。
//
// 列表结构（结构调整后）：汇总行 → 本机区块（置顶强调）→
// 「其他设备 (N)」默认折叠分组；退出操作需先展开分组。

test.describe("登录设备面板（ADR-0031）", () => {
  test("账号详情页内登录设备：本机区块置顶无退出按钮；其他设备默认折叠，展开后退出需确认且成功后列表刷新", async ({ page }) => {
    await installMockBridge(page, { traeCnAccounts: true });
    await page.setViewportSize({ width: 1280, height: 800 });
    await page.goto("/");

    // 切到 Trae CN 产品进入账号页；列表页不应出现独立设备区块。
    await page.getByRole("combobox", { name: "当前产品" }).selectOption("trae_cn");
    const card = page.getByTestId("account-adapter-card-trae-cn-profile");
    await expect(card).toBeVisible();
    await expect(page.getByTestId("remote-devices-panel")).toHaveCount(0);

    // 点击账号卡片进入详情：设备模块出现但默认折叠，未发设备命令。
    await card.click();
    const panel = page.getByTestId("remote-devices-panel");
    await expect(page.getByTestId("adapter-account-detail")).toBeVisible();
    await expect(panel).toBeVisible();
    await page.getByTestId("remote-devices-toggle").click();

    // 汇总行：已用/上限。
    const summary = page.getByTestId("remote-devices-summary");
    await expect(summary).toHaveText(/已用 2 \/ 上限 10 台/);

    // 本机区块：置顶可见，本机行带「本机」徽章，无退出按钮（UI 第一道防线）。
    const localBlock = page.getByTestId("remote-devices-local");
    await expect(localBlock).toBeVisible();
    const localRow = page.getByTestId("remote-device-row-dev-mock-local");
    await expect(localRow).toBeVisible();
    await expect(localRow.locator(".remote-devices__badge")).toHaveText("本机");
    await expect(page.getByTestId("remote-device-signout-dev-mock-local")).toHaveCount(0);

    // 其他设备分组默认折叠：折叠头显示数量，本机区块之外无任何退出按钮可见。
    const othersToggle = page.getByTestId("remote-devices-others-toggle");
    await expect(othersToggle).toHaveText(/其他设备 \(1\)/);
    await expect(othersToggle).toHaveAttribute("aria-expanded", "false");
    await expect(page.getByTestId("remote-device-row-dev-mock-mobile")).toHaveCount(0);
    await expect(page.getByTestId("remote-device-signout-dev-mock-mobile")).toHaveCount(0);

    // 展开「其他设备 (1)」：非本机行可见，点击后出现单次确认弹窗
    // （文案列明重新登录影响）。
    await othersToggle.click();
    await expect(othersToggle).toHaveAttribute("aria-expanded", "true");
    await expect(page.getByTestId("remote-device-row-dev-mock-mobile")).toBeVisible();
    await page.getByTestId("remote-device-signout-dev-mock-mobile").click();
    const dialog = page.getByTestId("remote-device-signout-confirm");
    await expect(dialog).toBeVisible();
    await expect(dialog).toContainText("退出这台设备？");
    await expect(dialog).toContainText("重新登录");

    // 确认退出：mock 从 fixture 删除该行，重拉后汇总少一台、其他分组数量减一。
    await page.getByTestId("remote-device-signout-confirm-confirm").click();
    await expect(summary).toHaveText(/已用 1 \/ 上限 10 台/);
    await expect(othersToggle).toHaveText(/其他设备 \(0\)/);
    await expect(page.getByTestId("remote-device-row-dev-mock-mobile")).toHaveCount(0);
    await expect(panel).toContainText("已退出该设备，列表已刷新。");
    // 本机行仍在区块中。
    await expect(page.getByTestId("remote-device-row-dev-mock-local")).toBeVisible();
  });
});
