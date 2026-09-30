// ============================================================================
// Trae CN 项目归并 e2e（P13-2，ADR-0033 选择性写：只读库唯一开放写）
// ============================================================================
//
// 覆盖：
// - 入口条件：其他账号项目清单为空时「其他账号的项目」组不渲染；
//   有清单时树末位成组，行内「归并到当前账号」入口可见
// - 确认流：入口 → 规模预览弹层（逐账号规模，主信息 = 账号名）→
//   单次确认 → 归并命令 → 全局通知回执 → 入口消失（清单刷新）
// - 守卫镜像：mock bridge 对非 trae_cn libraryId 拒绝（与后端同构）
//
// 全程走 mock 命令边界，绝不启动 Tauri 或访问真实 TRAE 数据。
// 宿主 = Trae CN 数据根详情页（总览页「浏览 Trae CN 数据」入口）。

import { test, expect, type Locator, type Page } from "@playwright/test";
import { installMockBridge, type MockScenario } from "./mock-bridge";

// CN 详情页 region（LibrarySessionsPanel 的 embedded 宿主）。
function detail(page: Page): Locator {
  return page.getByRole("region", { name: "Trae CN 数据根详情" });
}

// 全局通知 toast（sonner）：回执类提示的统一断言目标。
function notifyToast(page: Page, text: string | RegExp): Locator {
  return page.locator("[data-sonner-toast]").filter({ hasText: text });
}

// 安装 mock bridge → 切到 Trae CN → 总览页「浏览 Trae CN 数据」进详情页。
async function setup(page: Page, scenario: MockScenario = {}) {
  await installMockBridge(page, { traeCnAccounts: true, ...scenario });
  await page.setViewportSize({ width: 1280, height: 800 });
  await page.goto("/");
  await page.getByRole("combobox", { name: "当前产品" }).selectOption("trae_cn");
  await page.getByTestId("navigation-overview").click();
  await page.getByTestId("overview-library-cta").click();
  await expect(detail(page)).toBeVisible();
}

test.describe("Trae CN 项目归并（P13-2）", () => {
  test("无其他账号项目时归并入口不渲染（只读浏览形态不变）", async ({ page }) => {
    await setup(page);
    const panel = detail(page);

    // 读路径正常：对话列表 tab 有项目树。
    await expect(panel.getByTestId("library-project-p-t1")).toContainText("Trae 项目");
    // 默认场景无其他账号项目 → 分组与入口不存在。
    await expect(panel.getByTestId("library-foreign-group")).toHaveCount(0);
    // 只读形态下批量选择入口依旧隐藏（P11-6 断言随迁确认）。
    await expect(panel.getByTestId("library-select-mode")).toHaveCount(0);
  });

  test("入口 → 规模预览弹层 → 单次确认 → 回执通知 → 入口消失", async ({ page }) => {
    await setup(page, { traeCnMergeEntry: true });
    const panel = detail(page);

    // 树末位「其他账号的项目」组：行 + 归并入口可见。
    const group = panel.getByTestId("library-foreign-group");
    await expect(group).toContainText("其他账号的项目");
    const row = panel.getByTestId("library-foreign-project-p-cn-foreign");
    await expect(row).toContainText("共享项目（来源账号）");
    await expect(row).toContainText("2");
    const entry = panel.getByTestId("library-merge-entry-p-cn-foreign");
    await expect(entry).toBeVisible();

    // 点击入口：规模预览弹层（主信息 = 账号名 + 数量，user_id 收悬浮）。
    await entry.click();
    const dialog = page.getByTestId("trae-cn-merge-confirm");
    await expect(dialog).toBeVisible();
    await expect(dialog).toContainText("归并「共享项目（来源账号）」到当前账号");
    await expect(dialog).toContainText("其他来源账号 · 2 个会话 · 9 条消息");
    await expect(dialog).toContainText("归并给 Trae CN 测试账号");
    await expect(dialog).toContainText("归并前会自动备份");
    // 技术标识不进主视野：user_id 只出现在悬浮提示。
    await expect(dialog.getByTestId("trae-cn-merge-source-u-cn-source")).toHaveAttribute(
      "title",
      "u-cn-source",
    );

    // 单次确认：归并命令 + 回执通知（会话数 + 消息数）。
    await dialog.getByTestId("trae-cn-merge-confirm-confirm").click();
    await expect(notifyToast(page, "已把 2 个会话归并到当前账号（共 9 条消息）。")).toBeVisible();
    // 归并后清单刷新：入口整组消失（可变 mock 状态已移除该行）。
    await expect(panel.getByTestId("library-foreign-group")).toHaveCount(0);
  });
});
