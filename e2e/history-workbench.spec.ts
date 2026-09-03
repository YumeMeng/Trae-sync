// ============================================================================
// P5-3 历史页主库视图 Playwright 端到端测试
// ============================================================================
//
// 覆盖：
// - 两栏结构：左栏项目（统计 + 参与账号头像）/ 右栏会话（deleted 过滤）
// - 项目联动筛选、时间范围筛选、标题搜索
// - 接力轨迹徽章 + 预览弹层（轨迹时间线 + 消息流 + Esc 关闭）
// - 主库引导态（no_master_data / no_current_account）与读取失败态
// - 无水平溢出
//
// Windows 桌面视口由 playwright.config.ts 覆盖；移动端不属于当前产品目标。
// 所有测试使用 mock 命令边界，绝不启动 Tauri 或访问真实 TRAE 数据。

import { test, expect, type Page } from "@playwright/test";
import { installMockBridge, type MockScenario } from "./mock-bridge";

// 每个 beforeEach 安装 mock bridge 并加载页面；默认场景主库 ready。
async function setup(page: Page, scenario: MockScenario = {}) {
  await installMockBridge(page, scenario);
  await page.goto("/");
  // 应用首页是总览页；历史页测试需要先切换导航。
  await page.getByTestId("navigation-history").click();
  await expect(page.getByRole("region", { name: "历史" })).toBeVisible();
}

// ============================================================================
// 两栏结构与筛选
// ============================================================================

test.describe("P5-3 历史页主库视图", () => {
  test("ready 状态渲染两栏：左栏项目 + 右栏会话 + 项目统计", async ({ page }) => {
    await setup(page);

    // 左栏：全部项目入口 + 两个项目（统计由会话实时聚合；归档会话不计入）。
    await expect(page.getByTestId("history-project-all")).toBeVisible();
    const p1 = page.getByTestId("history-project-p1");
    await expect(p1).toContainText("项目阿尔法");
    await expect(p1).toContainText("2 会话");
    await expect(page.getByTestId("history-project-p2")).toContainText("项目贝塔");
    await expect(page.getByTestId("history-project-p2")).toContainText("0 会话");

    // 右栏：两个正常会话；deleted=true 的 s4 与已归档的 s3 不出现，入口带计数。
    const list = page.getByTestId("history-session-list");
    await expect(list).toContainText("会话一 关于构建稳定的历史库");
    await expect(list).toContainText("会话二 软删除与版本图");
    await expect(list).not.toContainText("旧归档会话");
    await expect(list).not.toContainText("已删除会话");
    await expect(page.getByTestId("history-archive-entry")).toContainText("已归档 1");
  });

  test("点击左栏项目，右栏只显示该项目会话", async ({ page }) => {
    await setup(page);
    await page.getByTestId("history-project-p2").click();

    // p2 唯一会话已归档 → 项目视图空态。
    const list = page.getByTestId("history-session-list");
    await expect(list).not.toContainText("会话一");
    await expect(list).not.toContainText("旧归档会话");

    // 回到全部项目恢复完整列表。
    await page.getByTestId("history-project-all").click();
    await expect(list).toContainText("会话一");
    await expect(list).toContainText("会话二");
  });

  test("时间范围筛选：近 7 天隐藏 40 天前的旧会话", async ({ page }) => {
    await setup(page);
    await page.getByTestId("history-time-week").click();

    const list = page.getByTestId("history-session-list");
    await expect(list).toContainText("会话一");
    await expect(list).not.toContainText("旧归档会话");

    // 近 30 天同样隐藏 40 天前会话；全部恢复。
    await page.getByTestId("history-time-month").click();
    await expect(list).not.toContainText("旧归档会话");
    await page.getByTestId("history-time-all").click();
    await expect(list).toContainText("会话一");
    await expect(list).toContainText("会话二");
  });

  test("标题搜索过滤会话且保留无结果的空态文案", async ({ page }) => {
    await setup(page);
    const input = page.getByTestId("history-search-input");
    await input.fill("软删除");
    const list = page.getByTestId("history-session-list");
    await expect(list).toContainText("会话二");
    await expect(list).not.toContainText("会话一");

    await input.fill("不存在的关键词");
    await expect(page.getByTestId("history-empty-sessions")).toBeVisible();
    await expect(page.getByTestId("history-empty-sessions")).toContainText(
      "没有符合条件的会话",
    );
  });
});

// ============================================================================
// 接力轨迹与预览弹层
// ============================================================================

test.describe("接力轨迹与预览", () => {
  test("接力徽章显示头像链，预览弹层展示完整轨迹时间线与消息", async ({ page }) => {
    await setup(page);

    // s1 三跳接力：徽章头像链可见（最多 3 个头像 + 溢出计数）。
    const session = page.getByTestId("history-session-s1");
    await expect(session).toBeVisible();
    await expect(session.locator('[data-testid="history-relay-chain"]')).toBeVisible();

    // 打开预览：轨迹时间线 + 消息流（文本 + 任务轨迹两种形态）。
    await session.click();
    const preview = page.getByTestId("history-preview");
    await expect(preview).toBeVisible();
    await expect(preview.locator(".preview__section").first()).toContainText("接力记录");
    // 三跳链 → 4 条腿（首腿 + 中间两腿 + 当前腿）。
    await expect(preview.locator(".leg-step")).toHaveCount(4);
    // 当前账号徽章落在 user-B 持有的两段腿上（A→B 接收腿 + 最终当前腿）。
    await expect(preview.locator(".leg-step__badge")).toHaveCount(2);

    await expect(page.getByTestId("preview-content")).toContainText("如何跨账号接力这条会话？");
    await expect(page.getByTestId("preview-content")).toContainText("通过主库交接把记录转移给接收账号。");
    await expect(page.getByTestId("preview-content")).toContainText("任务轨迹 · 2 步");
    await expect(page.getByTestId("preview-content")).toContainText("检索主库会话索引");

    // 关闭按钮可用。
    await page.getByTestId("history-preview-close").click();
    await expect(preview).toHaveCount(0);
  });

  test("无台账记录的会话不显示接力徽章", async ({ page }) => {
    await setup(page);
    await expect(
      page.getByTestId("history-session-s2").locator('[data-testid="history-relay-chain"]'),
    ).toHaveCount(0);
  });

  test("Esc 关闭预览弹层", async ({ page }) => {
    await setup(page);
    await page.getByTestId("history-session-s1").click();
    await expect(page.getByTestId("history-preview")).toBeVisible();
    await page.keyboard.press("Escape");
    await expect(page.getByTestId("history-preview")).toHaveCount(0);
  });
});

// ============================================================================
// 引导态与错误态
// ============================================================================

test.describe("主库引导态与错误态", () => {
  test("no_master_data 显示去环境页引导", async ({ page }) => {
    await setup(page, { masterHistory: "no_master_data" });
    const guide = page.getByTestId("history-guide");
    await expect(guide).toBeVisible();
    await expect(guide).toContainText("主库还没有对话数据");
    await expect(page.getByTestId("history-guide-launch")).toBeVisible();
  });

  test("no_current_account 显示登录引导", async ({ page }) => {
    await setup(page, { masterHistory: "no_current_account" });
    await expect(page.getByTestId("history-guide")).toContainText("主库尚未登记登录账号");
  });

  test("read_failed 显示错误与重试入口", async ({ page }) => {
    await setup(page, { masterHistory: "read_failed" });
    const error = page.getByTestId("history-error");
    await expect(error).toBeVisible();
    await expect(error).toContainText("主库记录暂时无法读取");
    await expect(error.getByRole("button", { name: /重试/ })).toBeVisible();
  });
});

// ============================================================================
// 布局稳定性与页面切换保持
// ============================================================================

test.describe("布局与状态保持", () => {
  test("页面无水平溢出", async ({ page }) => {
    await setup(page);
    await expect(page.getByTestId("history-session-s1")).toBeVisible();
    const overflow = await page.evaluate(() => ({
      scrollWidth: document.documentElement.scrollWidth,
      clientWidth: document.documentElement.clientWidth,
    }));
    expect(overflow.scrollWidth).toBeLessThanOrEqual(overflow.clientWidth);
  });

  test("切换到总览页再回到历史页不丢失浏览状态", async ({ page }) => {
    await setup(page);
    await page.getByTestId("history-project-p1").click();
    await expect(page.getByTestId("history-session-list")).toContainText("会话一");

    await page.getByTestId("navigation-overview").click();
    await expect(page.getByRole("region", { name: "总览" })).toBeVisible();
    await page.getByTestId("navigation-history").click();

    // 组件保持挂载：项目筛选与列表维持原状。
    await expect(page.getByTestId("history-session-list")).toContainText("会话一");
    await expect(page.getByTestId("history-session-list")).not.toContainText("已删除会话");
  });
});

// ============================================================================
// P5-8a 会话归档：选择模式批量栏 + 归档视图 + 真实删除确认
// ============================================================================

test.describe("P5-8a 会话归档与批量操作", () => {
  test("选择模式批量归档：归档后主列表联动、入口计数增加", async ({ page }) => {
    await setup(page);

    // 进入选择模式：浮出批量栏，会话行点击变为勾选。
    await page.getByTestId("history-select-mode").click();
    await expect(page.getByTestId("session-batch-bar")).toBeVisible();
    await page.getByTestId("history-session-s1").click();
    await page.getByTestId("history-session-s2").click();
    await expect(page.getByTestId("batch-count")).toContainText("已选 2 / 2 项");

    // 归档所选 → 刷新后主列表仅剩空态，入口计数 3（s3 原有 + 新归档 2）。
    await page.getByTestId("batch-archive").click();
    await expect(page.getByTestId("history-archive-entry")).toContainText("已归档 3");
    await expect(page.getByTestId("session-batch-bar")).toHaveCount(0);
    await expect(page.getByTestId("history-session-s1")).toHaveCount(0);
  });

  test("归档视图按模式→分组层级展示，恢复所选后归位主列表", async ({ page }) => {
    await setup(page);

    await page.getByTestId("history-archive-entry").click();
    const archiveList = page.getByTestId("history-archive-list");
    await expect(archiveList).toContainText("Work 模式");
    await expect(archiveList).toContainText("项目贝塔");
    await expect(page.getByTestId("archive-session-s3")).toContainText("旧归档会话");

    // 选择 → 恢复 → 归档视图清空，返回主列表可见归位。
    await page.getByTestId("archive-select-mode").click();
    await page.getByTestId("archive-session-s3").click();
    await page.getByTestId("batch-restore").click();
    await expect(page.getByTestId("archive-empty")).toBeVisible();

    await page.getByTestId("history-archive-back").click();
    await expect(page.getByTestId("history-session-s3")).toBeVisible();
  });

  test("删除所选：二次确认列明规模，取消不删除、确认后执行", async ({ page }) => {
    await setup(page);

    await page.getByTestId("history-archive-entry").click();
    await page.getByTestId("archive-session-s3").waitFor();

    await page.getByTestId("archive-select-mode").click();
    await page.getByTestId("archive-session-s3").click();
    await page.getByTestId("batch-delete").click();

    // 确认弹窗列明会话数与消息量（s3 = 1 个会话 3 条消息）。
    const dialog = page.getByTestId("delete-confirm");
    await expect(dialog).toContainText("删除 1 个会话");
    await expect(dialog).toContainText("共 3 条消息");

    // 取消：不执行删除。
    await page.getByTestId("delete-confirm-cancel").click();
    await expect(page.getByTestId("archive-session-s3")).toBeVisible();

    // 确认：真实删除 → 归档视图清空。
    await page.getByTestId("batch-delete").click();
    await page.getByTestId("delete-confirm-ok").click();
    await expect(page.getByTestId("archive-empty")).toBeVisible();
  });
});

// ============================================================================
// 账号页（沿用：与历史页共享 mock 边界）
// ============================================================================

test.describe("账号档案", () => {
  test("账号页展示已保存账号并保留本机切换边界", async ({ page }) => {
    await installMockBridge(page);
    await page.goto("/");
    await page.getByTestId("navigation-accounts").click();

    // 账号页内还有子 region，主 region 必须精确匹配。
    const accountCenter = page.getByRole("region", { name: "账号", exact: true });
    await expect(accountCenter).toBeVisible();
    await expect(accountCenter).toContainText("工作账号 A");
    await expect(accountCenter).toContainText("工作账号 B");
    // 旧外部切换入口（保存当前登录/切换账号）已随环境模型移除。
    await expect(accountCenter.getByRole("button", { name: "保存当前登录" })).toHaveCount(0);
    await expect(accountCenter.getByRole("button", { name: "切换账号" })).toHaveCount(0);
  });
});
