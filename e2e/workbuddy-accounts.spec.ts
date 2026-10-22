// ============================================================================
// P2b WorkBuddy 冒烟：账号页
// ============================================================================
//
// 护栏目的：同 workbuddy-overview.spec.ts——WB 账号页此前零 e2e 覆盖。
// 断言点：列表容器渲染 + 存在账号行则进入详情再返回（用既有 testid/文案）。
// mock 方案与产品切换写法沿用 mock-bridge.ts 与标题栏原生 select。

import { test, expect, type Page } from "@playwright/test";
import { installMockBridge } from "./mock-bridge";

async function switchToWorkBuddy(page: Page) {
  const switcher = page.getByTestId("product-switcher");
  await expect(switcher).toBeVisible();
  await switcher.selectOption("workbuddy");
  await expect(page.getByRole("heading", { level: 1, name: "总览" })).toBeVisible();
}

test.describe("P2b WorkBuddy 账号页冒烟", () => {
  test("切到 WorkBuddy 后账号页渲染列表容器，账号行可进详情再返回", async ({ page }) => {
    const consoleErrors: string[] = [];
    page.on("console", (message) => {
      if (message.type() === "error") consoleErrors.push(message.text());
    });
    page.on("pageerror", (error) => consoleErrors.push(String(error)));

    await installMockBridge(page, { workbuddy: true });
    await page.goto("/");
    await switchToWorkBuddy(page);

    // 账号页：region 精确匹配「账号」（页内含同名子 region）。
    await page.getByTestId("navigation-accounts").click();
    const accountRegion = page.getByRole("region", { name: "账号", exact: true });
    await expect(accountRegion).toBeVisible();

    // 列表容器（AccountPageSkeleton 的 listTestId）+ 两个 mock 账号行。
    const poolList = page.getByTestId("workbuddy-pool-list");
    await expect(poolList).toBeVisible();
    const firstRow = page.getByTestId("workbuddy-pool-card-wb-profile-1");
    await expect(firstRow).toContainText("WB 账号甲");
    await expect(page.getByTestId("workbuddy-pool-card-wb-profile-2")).toContainText("备用号");

    // 存在账号行 → 点进详情（既有 testid workbuddy-account-detail）→ 返回列表。
    await firstRow.click();
    const detail = page.getByTestId("workbuddy-account-detail");
    await expect(detail).toBeVisible();
    // 详情页仍属账号 region 语义（role=region + aria-label="账号详情"）。
    await expect(page.getByRole("region", { name: "账号详情" })).toBeVisible();
    await expect(detail).toContainText("基础信息");
    await expect(detail).toContainText("签到与积分");

    await page.getByTestId("workbuddy-account-detail-back").click();
    await expect(page.getByTestId("workbuddy-pool-list")).toBeVisible();
    await expect(page.getByTestId("workbuddy-account-detail")).toHaveCount(0);

    expect(consoleErrors, "WorkBuddy 账号页出现 console error：" + consoleErrors.join(" | ")).toHaveLength(0);
  });
});
