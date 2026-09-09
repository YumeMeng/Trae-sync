// ============================================================================
// 库对话面板 Playwright 端到端测试（G21 两栏式 + G22 统一选择模式，ADR-0025）
// ============================================================================
//
// 覆盖：
// - 左栏项目树：搜索置顶、项目行原地展开会话子级（标题 + 相对时间）、
//   「未关联文件夹」末位分组（G19）、底部「已归档」入口（有归档才显示）
// - 右栏查看器：未选会话空状态引导、标题 + 消息|接力 tab、用户消息右对齐
//   带账号色头像 / 助手左对齐、>=2000 条截断提示
// - 统一选择模式：勾选框、底部浮出操作栏（已选 N 会话·M 项目 + 归档/合并到…/删除）、
//   归档视图（恢复/删除）、删除单次确认弹层（列明规模，ADR-0018）、Esc 退出
// - 主库引导态（no_master_data / no_current_account）与读取失败态
// - 无水平溢出、返回环境页再进入详情浏览状态保持
//
// 宿主 = 主库详情页（环境页 → 主库卡「详情」，默认对话列表 tab 内嵌面板）。
// 独立历史页（navigation-history）P8-6 将删除，不再作为测试宿主。
// App 各页常驻 DOM（[hidden] 切换），独立历史页实例停留在加载态且不渲染
// 库面板内容——但为防跨实例 testid 冲突，所有库面板断言都 scope 在
// 「主库详情」region 内。
// Windows 桌面视口由 playwright.config.ts 覆盖；移动端不属于当前产品目标。
// 所有测试使用 mock 命令边界，绝不启动 Tauri 或访问真实 TRAE 数据。

import { test, expect, type Locator, type Page } from "@playwright/test";
import { installMockBridge, type MockScenario } from "./mock-bridge";

// 主库详情 region（LibrarySessionsPanel 的 embedded 宿主）。
function detail(page: Page): Locator {
  return page.getByRole("region", { name: "主库详情" });
}

// 安装 mock bridge → 环境页 → 主库卡「详情」进入主库详情页（真实 UI 路径）。
// 默认 tab 即对话列表；树是否渲染由各测试自行等待（引导态场景无树）。
async function setup(page: Page, scenario: MockScenario = {}) {
  await installMockBridge(page, scenario);
  await page.goto("/");
  await page.getByTestId("navigation-environment").click();
  await expect(page.getByRole("region", { name: "环境" })).toBeVisible();
  await page.getByTestId("env-master-detail").click();
  await expect(detail(page)).toBeVisible();
}

// ============================================================================
// G21 左栏项目树
// ============================================================================

test.describe("G21 左栏项目树", () => {
  test("ready 状态：具名项目分组 + 未关联文件夹末位 + 右栏空状态引导", async ({ page }) => {
    await setup(page);
    const panel = detail(page);

    // 具名项目（仅含有匹配会话的）：p1 两会话、p2 一会话（归档的 s3 不计数）。
    const p1 = panel.getByTestId("library-project-p1");
    await expect(p1).toContainText("项目阿尔法");
    await expect(p1).toContainText("2");
    await expect(panel.getByTestId("library-project-p2")).toContainText("项目贝塔");

    // 「未关联文件夹」末位合并组（空名项目 p3 的会话归并，G19）。
    const unlinked = panel.getByTestId("library-project-__unlinked__");
    await expect(unlinked).toContainText("未关联文件夹");
    await expect(unlinked).toContainText("1");
    // 末位断言：合并组是树内最后一个项目行。
    await expect(panel.getByTestId("library-tree").locator(".lib-project").last()).toHaveAttribute(
      "data-testid",
      "library-project-__unlinked__",
    );

    // 已删除会话（s4）不入任何视图；底部「已归档」入口带计数（s3 归档中）。
    await expect(panel.getByTestId("library-tree")).not.toContainText("已删除会话");
    const archiveEntry = panel.getByTestId("library-archive-entry");
    await expect(archiveEntry).toContainText("已归档");
    await expect(archiveEntry).toContainText("1");
    // 选择按钮常驻底部固定区。
    await expect(panel.getByTestId("library-select-mode")).toBeVisible();

    // 右栏未选会话：空状态引导。
    await expect(panel.getByTestId("library-viewer-empty")).toContainText(
      "从左侧选择一个会话查看对话",
    );
  });

  test("点击项目行原地展开会话子级（标题 + 相对时间），再点收起", async ({ page }) => {
    await setup(page);
    const panel = detail(page);

    const p1 = panel.getByTestId("library-project-p1");
    await p1.click();
    const s1 = panel.getByTestId("library-session-s1");
    await expect(s1).toContainText("会话一 关于构建稳定的历史库");
    // 相对时间（fixture：s1 = 1 小时前、s2 = 2 天前）。
    await expect(s1.locator(".lib-session__time")).toHaveText("1 小时前");
    await expect(panel.getByTestId("library-session-s2")).toContainText("会话二 软删除与版本图");
    await expect(panel.getByTestId("library-session-s2").locator(".lib-session__time")).toHaveText(
      "2 天前",
    );

    // 再点同一项目行：原地收起。
    await p1.click();
    await expect(s1).toHaveCount(0);
  });

  test("标题搜索过滤树内会话，无结果保留空态文案", async ({ page }) => {
    await setup(page);
    const panel = detail(page);

    const search = panel.getByTestId("library-search-input");
    await search.fill("软删除");
    // 搜索时项目自动展开，仅匹配会话可见。
    await expect(panel.getByTestId("library-session-s2")).toBeVisible();
    await expect(panel.getByTestId("library-session-s1")).toHaveCount(0);

    await search.fill("不存在的关键词");
    await expect(panel.getByTestId("library-tree")).toContainText("没有匹配的会话");
  });
});

// ============================================================================
// G21 右栏查看器与接力轨迹
// ============================================================================

test.describe("右栏查看器与接力轨迹", () => {
  test("点击会话行显示查看器：标题 + 消息|接力 tab + 消息流两种形态", async ({ page }) => {
    await setup(page);
    const panel = detail(page);

    await panel.getByTestId("library-project-p1").click();
    await panel.getByTestId("library-session-s1").click();

    await expect(panel.getByTestId("library-viewer")).toBeVisible();
    await expect(panel.getByTestId("library-viewer-title")).toContainText(
      "会话一 关于构建稳定的历史库",
    );
    await expect(panel.getByTestId("library-tab-messages")).toBeVisible();
    await expect(panel.getByTestId("library-tab-relay")).toBeVisible();

    // 消息 tab 默认：文本 + 任务轨迹两种形态。
    const messages = panel.getByTestId("library-viewer-messages");
    await expect(messages).toContainText("如何跨账号接力这条会话？");
    await expect(messages).toContainText("通过主库交接把记录转移给接收账号。");
    await expect(messages).toContainText("任务轨迹 · 2 步");
    await expect(messages).toContainText("检索主库会话索引");

    // 用户消息右对齐带账号色头像与账号名（m1 时间落在账号 A 持有腿）；
    // 助手消息左对齐中性色（通用机器人图标，不着色）。
    const userMessage = panel.getByTestId("library-message-m1");
    await expect(userMessage).toHaveClass(/chat-msg--user/);
    await expect(userMessage.locator(".account-avatar")).toBeVisible();
    await expect(userMessage.locator(".chat-msg__name")).toHaveText("工作账号 A");
    await expect(panel.getByTestId("library-message-m2")).toHaveClass(/chat-msg--assistant/);
  });

  test("接力 tab 展示完整轨迹时间线（三跳链 → 4 腿，当前腿标记）", async ({ page }) => {
    await setup(page);
    const panel = detail(page);

    await panel.getByTestId("library-project-p1").click();
    await panel.getByTestId("library-session-s1").click();
    await panel.getByTestId("library-tab-relay").click();

    const relay = panel.getByTestId("library-relay-tab");
    await expect(relay).toContainText("这条会话被以下账号先后使用，按时间顺序排列。");
    // 三跳链 → 4 条腿（首腿 + 中间两腿 + 当前腿）。
    await expect(relay.locator(".leg-step")).toHaveCount(4);
    // 「当前」徽章只落在时间线最后一段（进行中腿）。
    await expect(relay.locator(".leg-step__badge")).toHaveCount(1);
    await expect(relay.locator(".leg-step__badge")).toHaveText("当前");
    await expect(relay).toContainText("工作账号 A");
    await expect(relay).toContainText("工作账号 B");
  });

  test("无台账记录的会话：接力 tab 只有当前账号单一腿", async ({ page }) => {
    await setup(page);
    const panel = detail(page);

    await panel.getByTestId("library-project-p1").click();
    await panel.getByTestId("library-session-s2").click();
    await panel.getByTestId("library-tab-relay").click();

    // 主库记录单一归属（ADR-0021）：无台账 → 当前账号单一腿。
    const relay = panel.getByTestId("library-relay-tab");
    await expect(relay.locator(".leg-step")).toHaveCount(1);
    await expect(relay.locator(".leg-step__badge")).toHaveCount(1);
    await expect(relay).toContainText("工作账号 B");
  });

  test("关闭查看器回到空状态引导", async ({ page }) => {
    await setup(page);
    const panel = detail(page);

    await panel.getByTestId("library-project-p1").click();
    await panel.getByTestId("library-session-s1").click();
    await expect(panel.getByTestId("library-viewer")).toBeVisible();
    await panel.getByTestId("library-viewer-close").click();
    await expect(panel.getByTestId("library-viewer-empty")).toBeVisible();
  });

  test("消息达到窗口上限时可继续向上加载更早消息", async ({ page }) => {
    await setup(page, { sessionMessages: "bulk" });
    const panel = detail(page);

    await panel.getByTestId("library-project-p1").click();
    await panel.getByTestId("library-session-s1").click();
    // 首页按 2000 条分页，提示仍有更早消息可读。
    await expect(panel.getByTestId("library-chat-notice")).toContainText(
      "已加载最近 2000 条消息，还可继续查看更早内容。",
    );
    await panel.getByTestId("library-viewer-messages").evaluate((node) => {
      node.scrollTop = 0;
      node.dispatchEvent(new Event("scroll", { bubbles: true }));
    });
    await expect(panel.getByTestId("library-message-bulk-older-0")).toBeVisible();
  });
});

// ============================================================================
// G22 统一选择模式与批量操作
// ============================================================================

test.describe("G22 统一选择模式与批量操作", () => {
  test("勾选会话批量归档：操作栏计数、归档后树联动", async ({ page }) => {
    await setup(page);
    const panel = detail(page);

    // 先展开项目（选择模式下项目行点击是勾选项目，不能再展开）。
    await panel.getByTestId("library-project-p1").click();
    await panel.getByTestId("library-select-mode").click();

    const bar = panel.getByTestId("library-action-bar");
    await expect(bar).toBeVisible();
    await expect(panel.getByTestId("library-select-hint")).toContainText("选择模式：预览只读");

    await panel.getByTestId("library-session-s1").click();
    await panel.getByTestId("library-session-s2").click();
    await expect(panel.getByTestId("batch-count")).toContainText("已选 2 个会话 · 0 个项目");

    // 归档所选 → p1 移空后行消失；未涉及的 p2 仍在；入口计数 3（s3 + 新归档 2）。
    await panel.getByTestId("batch-archive").click();
    await expect(panel.getByTestId("action-notice")).toContainText("已归档 2 个会话。");
    await expect(panel.getByTestId("library-project-p1")).toHaveCount(0);
    await expect(panel.getByTestId("library-session-s1")).toHaveCount(0);
    await expect(panel.getByTestId("library-project-p2")).toBeVisible();
    await expect(panel.getByTestId("library-archive-entry")).toContainText("3");
    // 归档完成自动退出选择模式（操作栏消失）。
    await expect(bar).toHaveCount(0);
  });

  test("浏览态悬浮快捷归档：单会话直接归档不进选择模式", async ({ page }) => {
    await setup(page);
    const panel = detail(page);

    await panel.getByTestId("library-project-p1").click();
    const row = panel.getByTestId("library-session-s1");
    // 快捷按钮 hover 才显示（display:none → inline-flex）。
    await row.hover();
    await panel.getByTestId("library-quick-archive-s1").click();

    await expect(panel.getByTestId("action-notice")).toContainText("已归档 1 个会话。");
    await expect(panel.getByTestId("library-session-s1")).toHaveCount(0);
    await expect(panel.getByTestId("library-archive-entry")).toContainText("2");
  });

  test("Esc 退出选择模式", async ({ page }) => {
    await setup(page);
    const panel = detail(page);

    await panel.getByTestId("library-project-p1").click();
    await panel.getByTestId("library-select-mode").click();
    await expect(panel.getByTestId("library-action-bar")).toBeVisible();
    await page.keyboard.press("Escape");
    await expect(panel.getByTestId("library-action-bar")).toHaveCount(0);
  });

  test("勾选项目合并：确认弹层选保留目标，合并后回执与树联动", async ({ page }) => {
    await setup(page);
    const panel = detail(page);

    await panel.getByTestId("library-select-mode").click();
    // 勾选两个项目作为合并源（选择模式下项目行点击 = 勾选）。
    await panel.getByTestId("library-project-p1").click();
    await panel.getByTestId("library-project-p2").click();
    await expect(panel.getByTestId("batch-count")).toContainText("已选 0 个会话 · 2 个项目");

    await panel.getByTestId("batch-merge").click();
    const dialog = panel.getByTestId("merge-confirm");
    await expect(dialog).toContainText("合并 2 个项目");
    // 弹层列明规模：默认保留 p1 → p2 的 2 个会话（s3 归档 + s5）移入。
    await expect(dialog).toContainText("2 个会话将移入保留的项目（含已归档）");

    // 改选保留 p2 → 移动量变成 p1 的 2 个会话（s1 s2；s4 已删除不计）。
    await panel.getByTestId("merge-target-p2").check();
    await panel.getByTestId("merge-confirm-ok").click();

    // 回执只用数量与动作结果表述（界面表达纪律）。
    await expect(panel.getByTestId("action-notice")).toContainText(
      "已把 2 个会话并入「项目贝塔」，清理 1 个空分组。",
    );
    // 合并后树内 p1 移空被清理；「未关联文件夹」组不受影响。
    await expect(panel.getByTestId("library-project-p1")).toHaveCount(0);
    const p2 = panel.getByTestId("library-project-p2");
    await expect(p2).toBeVisible();
    await expect(p2).toContainText("3");
    await expect(panel.getByTestId("library-project-__unlinked__")).toBeVisible();
  });

  test("归档视图按模式→项目层级展示，恢复所选后归位主列表", async ({ page }) => {
    await setup(page);
    const panel = detail(page);

    await panel.getByTestId("library-archive-entry").click();
    // 归档视图：模式 → 项目 → 会话（s3 = Work 模式 → 项目贝塔）。
    const tree = panel.getByTestId("library-tree");
    await expect(tree).toContainText("Work 模式");
    await expect(panel.getByTestId("library-archive-project-p2")).toContainText("项目贝塔");
    await expect(panel.getByTestId("library-session-s3")).toContainText("旧归档会话");

    // 选择 → 恢复 → 归档视图清空。
    await panel.getByTestId("library-select-mode").click();
    await panel.getByTestId("library-session-s3").click();
    await expect(panel.getByTestId("batch-count")).toContainText("已选 1 个会话 · 0 个项目");
    await panel.getByTestId("batch-restore").click();
    await expect(panel.getByTestId("action-notice")).toContainText("已恢复 1 个会话。");
    await expect(panel.getByTestId("library-archive-empty")).toContainText("没有已归档的会话");

    // 返回主列表：s3 归位 p2 分组。
    await panel.getByTestId("library-archive-back").click();
    await panel.getByTestId("library-project-p2").click();
    await expect(panel.getByTestId("library-session-s3")).toBeVisible();
  });

  test("删除所选：单次确认弹层列明规模，取消不删除、确认后执行", async ({ page }) => {
    await setup(page);
    const panel = detail(page);

    await panel.getByTestId("library-archive-entry").click();
    await panel.getByTestId("library-session-s3").waitFor();
    await panel.getByTestId("library-select-mode").click();
    await panel.getByTestId("library-session-s3").click();
    await panel.getByTestId("batch-delete").click();

    // 确认弹窗列明会话数与消息量（s3 = 1 个会话 3 条消息，ADR-0018 单次确认）。
    const dialog = panel.getByTestId("delete-confirm");
    await expect(dialog).toContainText("删除 1 个会话");
    await expect(dialog).toContainText("共 3 条消息");
    await expect(dialog).toContainText("删除前会自动备份");

    // 取消：不执行删除。
    await panel.getByTestId("delete-confirm-cancel").click();
    await expect(panel.getByTestId("library-session-s3")).toBeVisible();

    // 确认：真实删除 → 归档视图清空。
    await panel.getByTestId("batch-delete").click();
    await panel.getByTestId("delete-confirm-ok").click();
    await expect(panel.getByTestId("library-archive-empty")).toBeVisible();
  });
});

// ============================================================================
// 主库引导态与错误态
// ============================================================================

test.describe("主库引导态与错误态", () => {
  test("no_master_data 显示去环境页引导", async ({ page }) => {
    await setup(page, { masterHistory: "no_master_data" });
    const guide = detail(page).getByTestId("history-guide");
    await expect(guide).toBeVisible();
    await expect(guide).toContainText("主库还没有对话数据");
    await expect(detail(page).getByTestId("history-guide-launch")).toBeVisible();
  });

  test("no_current_account 显示登录引导", async ({ page }) => {
    await setup(page, { masterHistory: "no_current_account" });
    await expect(detail(page).getByTestId("history-guide")).toContainText("主库尚未登记登录账号");
  });

  test("read_failed 显示错误与重试入口", async ({ page }) => {
    await setup(page, { masterHistory: "read_failed" });
    const error = detail(page).getByTestId("history-error");
    await expect(error).toBeVisible();
    await expect(error).toContainText("主库记录暂时无法读取");
    await expect(error.getByRole("button", { name: /重试/ })).toBeVisible();
  });
});

// ============================================================================
// 布局稳定性与页面切换保持
// ============================================================================

test.describe("布局与状态保持", () => {
  test("主库详情页无水平溢出", async ({ page }) => {
    await setup(page);
    const panel = detail(page);
    await panel.getByTestId("library-project-p1").click();
    await expect(panel.getByTestId("library-session-s1")).toBeVisible();
    const overflow = await page.evaluate(() => ({
      scrollWidth: document.documentElement.scrollWidth,
      clientWidth: document.documentElement.clientWidth,
    }));
    expect(overflow.scrollWidth).toBeLessThanOrEqual(overflow.clientWidth);
  });

  test("返回环境页再进入详情，浏览状态保持", async ({ page }) => {
    await setup(page);
    const panel = detail(page);

    // 展开项目并打开会话查看器。
    await panel.getByTestId("library-project-p1").click();
    await panel.getByTestId("library-session-s1").click();
    await expect(panel.getByTestId("library-viewer-title")).toBeVisible();

    // 返回环境页（详情页唯一退路）→ 再进入。
    await panel.getByTestId("master-detail-back").click();
    await expect(page.getByRole("region", { name: "环境" })).toBeVisible();
    await page.getByTestId("env-master-detail").click();
    await expect(detail(page)).toBeVisible();

    // 组件保持挂载：展开状态与查看器维持原状（unchanged 指纹不触发重读）。
    await expect(panel.getByTestId("library-session-s1")).toBeVisible();
    await expect(panel.getByTestId("library-viewer-title")).toContainText("会话一");
  });
});

// ============================================================================
// 账号页（沿用：与库对话面板共享 mock 边界）
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
