// ============================================================================
// Trae CN 归档/恢复 e2e（P13-3，ADR-0033 选择性写：hidden_status 通道首例 UI 写）
// ============================================================================
//
// 覆盖：
// - 归档全流程：展开项目 → 悬浮快捷归档 → 草稿条 → 应用 → 全局通知回执 →
//   树联动（会话离开项目树）→「已归档」入口出现
// - 恢复全流程：归档视图（模式→项目→会话）→ 选择模式 → 恢复 → 应用 →
//   归档视图清空 → 会话归位项目树
// - archive-only 形态守卫：选择模式只有归档/恢复动作，删除/合并控件
//   整体不渲染（红线：CN 库仍不开放删除会话/合并项目）
//
// 守卫镜像：mock bridge 对 trae_cn 放行 apply_master_archive_changes
// （可变状态改 hidden_status，同 master 先例）；删除/合并命令仍 master-only。
// 全程走 mock 命令边界，绝不启动 Tauri 或访问真实 TRAE 数据。
// 宿主 = Trae CN 数据根详情页（总览页「浏览 Trae CN 数据」入口）。

import { test, expect, type Locator, type Page } from "@playwright/test";
import { installMockBridge } from "./mock-bridge";

// CN 详情页 region（LibrarySessionsPanel 的 embedded 宿主）。
function detail(page: Page): Locator {
  return page.getByRole("region", { name: "TRAE 数据根详情" });
}

// 全局通知 toast（sonner）：回执类提示的统一断言目标。
function notifyToast(page: Page, text: string | RegExp): Locator {
  return page.locator("[data-sonner-toast]").filter({ hasText: text });
}

// 安装 mock bridge → 切到 Trae CN → 总览页「浏览 Trae CN 数据」进详情页。
async function setup(page: Page) {
  await installMockBridge(page, {
    traeCnAccounts: true,
    operationReadiness: { "library.archive": "ready" },
  });
  await page.setViewportSize({ width: 1280, height: 800 });
  await page.goto("/");
  await page.getByRole("combobox", { name: "当前产品" }).selectOption("trae_cn");
  await page.getByTestId("navigation-overview").click();
  await page.getByTestId("overview-library-cta").click();
  await expect(detail(page)).toBeVisible();
}

test.describe("Trae CN 归档/恢复（P13-3）", () => {
  test("归档 → 树联动 → 归档视图恢复（archive-only 全流程往返）", async ({ page }) => {
    await setup(page);
    const panel = detail(page);

    // 初始：项目树「Trae 项目」挂 1 个会话；无归档内容 → 已归档入口不渲染。
    const project = panel.getByTestId("library-project-p-t1");
    await expect(project).toContainText("Trae 项目");
    await expect(panel.getByTestId("library-archive-entry")).toHaveCount(0);

    // 悬浮快捷归档：展开项目 → 悬浮会话行（按钮为悬浮显隐形态）→
    // 单会话生成草稿（不进选择模式）。
    await project.click();
    const sessionRow = panel.getByTestId("library-session-s-cn-1");
    await expect(sessionRow).toContainText("CN 会话一");
    await sessionRow.hover();
    await panel.getByTestId("library-quick-archive-s-cn-1").click();
    await expect(panel.getByTestId("archive-draft-bar")).toContainText("归档 1 个");

    // 应用：全局通知回执 + 树联动（唯一会话离开项目树）+ 已归档入口出现。
    await panel.getByTestId("archive-draft-apply").click();
    await expect(
      notifyToast(page, "已应用归档设置：归档 1 个、恢复 0 个会话。"),
    ).toBeVisible();
    await expect(panel.getByTestId("library-session-s-cn-1")).toHaveCount(0);
    const archiveEntry = panel.getByTestId("library-archive-entry");
    await expect(archiveEntry).toContainText("1");

    // 归档视图：Code 模式 → 项目 → 会话（层级与主库归档视图同构）。
    await archiveEntry.click();
    await expect(panel.getByTestId("library-archive-back")).toBeVisible();
    await expect(panel.getByTestId("library-tree")).toContainText("Code 模式");
    await expect(panel.getByTestId("library-session-s-cn-1")).toContainText("CN 会话一");

    // 选择模式只有恢复动作：删除控件整体不渲染（archive-only 守卫形态）。
    await panel.getByTestId("library-select-mode").click();
    await expect(panel.getByTestId("batch-restore")).toBeVisible();
    await expect(panel.getByTestId("batch-delete")).toHaveCount(0);
    await panel.getByTestId("library-session-s-cn-1").click();
    await panel.getByTestId("batch-restore").click();
    await expect(panel.getByTestId("archive-draft-bar")).toContainText("恢复 1 个");
    await panel.getByTestId("archive-draft-apply").click();
    await expect(
      notifyToast(page, "已应用归档设置：归档 0 个、恢复 1 个会话。"),
    ).toBeVisible();

    // 恢复后归档视图清空；返回主树会话归位项目。
    // 项目在首次展开后保持展开记忆（恢复归位即展开态）：仅在仍折叠时点击。
    await expect(panel.getByTestId("library-archive-empty")).toBeVisible();
    await panel.getByTestId("library-archive-back").click();
    const restoredSession = panel.getByTestId("library-session-s-cn-1");
    if (!(await restoredSession.isVisible())) {
      await panel.getByTestId("library-project-p-t1").click();
    }
    await expect(restoredSession).toContainText("CN 会话一");
  });

  test("选择模式无删除/合并控件（archive-only 形态守卫）", async ({ page }) => {
    await setup(page);
    const panel = detail(page);

    await panel.getByTestId("library-project-p-t1").click();
    // P13-3：选择入口出现（原 P11-6 只读形态断言反转）。
    await panel.getByTestId("library-select-mode").click();
    await expect(panel.getByTestId("library-action-bar")).toBeVisible();
    await expect(panel.getByTestId("batch-archive")).toBeVisible();
    await expect(panel.getByTestId("batch-delete")).toHaveCount(0);
    await expect(panel.getByTestId("batch-merge")).toHaveCount(0);
  });
});
