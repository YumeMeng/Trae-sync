// ============================================================================
// P2b WorkBuddy 冒烟：总览页
// ============================================================================
//
// 护栏目的：WB 页面的 e2e 覆盖此前为零（39 条全是 TRAE 侧），P4 迁移 WB 页面
// 时没有任何回归网。本 spec 只做冒烟级断言——统计卡出现、不是「适配中」占位、
// 无 console error——不复制 WB 页面内部实现细节。
//
// mock 方案沿用 e2e/mock-bridge.ts 的既有边界（绝不启动 Tauri / 不碰真实数据）；
// 产品切换走标题栏原生 select（data-testid=product-switcher），与真实 UI 路径一致。
// 选择器只用现有 testid / 文案，不改 src/ 下任何产品代码（含 data-testid）。

import { test, expect, type Page } from "@playwright/test";
import { installMockBridge } from "./mock-bridge";

// 产品切换：标题栏原生 select 写入后端指针 → App 重读 → 槽位重挂载。
async function switchToWorkBuddy(page: Page) {
  const switcher = page.getByTestId("product-switcher");
  await expect(switcher).toBeVisible();
  await switcher.selectOption("workbuddy");
  // 切换完成的可见证据：总览问候语改为 WorkBuddy 工作台。
  await expect(page.getByRole("heading", { level: 1, name: /WorkBuddy 工作台$/ })).toBeVisible();
}

// console error 采集：Playwright 的 console 事件按页收集，断言前统一取出。
function collectConsoleErrors(page: Page): string[] {
  const errors: string[] = [];
  page.on("console", (message) => {
    if (message.type() === "error") errors.push(message.text());
  });
  page.on("pageerror", (error) => errors.push(String(error)));
  return errors;
}

test.describe("P2b WorkBuddy 总览冒烟", () => {
  test("切到 WorkBuddy 后总览统计卡出现、无适配中占位、无 console error", async ({ page }) => {
    const consoleErrors = collectConsoleErrors(page);
    await installMockBridge(page, { workbuddy: true });
    await page.goto("/");

    await switchToWorkBuddy(page);

    // 总览 region 与 WB 统计卡（签到与账号卡 + 对话库统计卡）。
    const overview = page.getByRole("region", { name: "总览" });
    await expect(overview).toBeVisible();

    const checkinCard = page.getByTestId("wb-overview-checkin");
    await expect(checkinCard).toBeVisible();
    // 统计卡承载真实计数（mock：2 账号 / 1 已签 / 余额 2000）。
    await expect(checkinCard).toContainText("账号");
    await expect(checkinCard).toContainText("今日签到");
    await expect(checkinCard).toContainText("1/2");

    // 对话库统计卡（mock：16 会话 / 4 项目 / 2 账号）。
    const libraryCard = page.getByTestId("wb-overview-stats-library");
    await expect(libraryCard).toBeVisible();
    await expect(libraryCard).toContainText("库内会话");
    await expect(libraryCard).toContainText("16");

    // 断言不是占位文案：WB 总览是 ready 态，不能出现「适配中」说明卡。
    await expect(page.getByTestId("capability-pending")).toHaveCount(0);
    await expect(page.getByTestId("capability-absent")).toHaveCount(0);
    await expect(page.getByText("该能力适配中")).toHaveCount(0);

    // 每屏一个主操作：查看对话库入口可用。
    await expect(page.getByTestId("wb-overview-library-cta")).toBeVisible();

    // 无 console error（页面级采集，覆盖切换与首屏渲染全过程）。
    expect(consoleErrors, "WorkBuddy 总览出现 console error：" + consoleErrors.join(" | ")).toHaveLength(0);
  });
});
