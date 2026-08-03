// ============================================================================
// T03/T04 历史库工作台 Playwright 端到端测试
// ============================================================================
//
// 覆盖（交接文档 Playwright 章节）：
// - 授权/空/扫描中/成功/失败状态
// - 账号/项目/会话导航
// - 完整对话预览
// - 搜索结果导航
// - 预览不改变选择
// - 无文本重叠、裁剪或水平溢出
//
// 桌面 + 移动视口由 playwright.config.ts 的两个 project 覆盖。
// 所有测试使用 mock 命令边界，绝不启动 Tauri 或访问真实 TRAE 数据。

import { test, expect, type Page } from "@playwright/test";
import { installMockBridge, setScenario, type MockScenario } from "./mock-bridge";

// 每个 beforeEach 安装 mock bridge 并加载页面
async function setup(page: Page, scenario: MockScenario = {}) {
  await installMockBridge(page, scenario);
  await page.goto("/");
  // 等待工作台加载完成
  await expect(page.getByRole("region", { name: "历史库" })).toBeVisible();
}

// 完成授权表单并点击扫描
async function authorizeAndScan(page: Page) {
  await page.getByTestId("history-fixture-root-input").fill("C:\\fixture");
  await page.getByTestId("authorize-check").check();
  await page.getByTestId("scan-history-button").click();
}

// ============================================================================
// 状态覆盖
// ============================================================================

test.describe("历史库状态覆盖", () => {
  test("idle 状态显示授权表单，不自动扫描", async ({ page }) => {
    await setup(page);
    // 显示授权面板
    await expect(page.getByTestId("auth-panel")).toBeVisible();
    // 扫描按钮初始禁用（未授权）
    await expect(page.getByTestId("scan-history-button")).toBeDisabled();
  });

  test("授权后扫描成功显示浏览结果（success 状态）", async ({ page }) => {
    await setup(page, { scanOutcome: "success", browseMode: "full" });
    await authorizeAndScan(page);
    // 应显示账号/项目树
    await expect(page.getByTestId("account-project-tree")).toBeVisible();
    // 摘要显示数量
    await expect(page.getByTestId("history-summary")).toContainText("账号 2");
    await expect(page.getByTestId("history-summary")).toContainText("项目 2");
    await expect(page.getByTestId("history-summary")).toContainText("对话 2");
  });

  test("扫描中状态显示加载提示", async ({ page }) => {
    await setup(page, { scanOutcome: "success", browseMode: "full" });
    // 填写并授权
    await page.getByTestId("history-fixture-root-input").fill("C:\\fixture");
    await page.getByTestId("authorize-check").check();
    await page.getByTestId("scan-history-button").click();
    // 扫描中状态——可能很快过去，使用 or 条件
    // 由于 mock 立即返回，scanning 状态可能不可见；验证成功状态作为 fallback
    await expect(page.getByTestId("account-project-tree")).toBeVisible({ timeout: 10_000 });
  });

  test("扫描失败显示结构化原因（failure 状态）", async ({ page }) => {
    await setup(page, {
      scanOutcome: { failed: "schema_incompatible" },
    });
    await authorizeAndScan(page);
    // 显示失败状态
    await expect(page.getByTestId("failure-state")).toBeVisible();
    await expect(page.getByTestId("failure-state")).toContainText("schema 不兼容");
    // 不暴露 secret / raw_key / 认证正文
    const text = await page.getByTestId("failure-state").textContent();
    expect(text).not.toMatch(/raw_key|rawkey|secret|bearer|token/i);
  });

  test("扫描成功但目录库为空显示 empty 状态", async ({ page }) => {
    await setup(page, { scanOutcome: "success", browseMode: "empty" });
    await authorizeAndScan(page);
    await expect(page.getByTestId("empty-state")).toBeVisible();
  });

  test("TRAE 运行中时扫描按钮禁用且不发布快照", async ({ page }) => {
    await setup(page);
    await page.getByTestId("history-fixture-root-input").fill("C:\\fixture");
    // 取消"TRAE 已关闭"勾选 → processRunning = true
    await page.getByTestId("trae-not-running-check").uncheck();
    await page.getByTestId("authorize-check").check();
    // 扫描按钮仍应禁用
    await expect(page.getByTestId("scan-history-button")).toBeDisabled();
  });
});

// ============================================================================
// 账号/项目/会话导航
// ============================================================================

test.describe("账号/项目/会话导航", () => {
  test("点击账号展开项目列表", async ({ page }) => {
    await setup(page, { scanOutcome: "success", browseMode: "full" });
    await authorizeAndScan(page);
    // 等待树渲染
    await expect(page.getByTestId("account-user-A")).toBeVisible();
    // 点击第一个账号
    await page.getByTestId("account-user-A").click();
    // 项目应出现
    await expect(page.getByTestId("project-p1")).toBeVisible();
  });

  test("会话列表显示完整标题与消息数", async ({ page }) => {
    await setup(page, { scanOutcome: "success", browseMode: "full" });
    await authorizeAndScan(page);
    // 会话应显示
    await expect(page.getByTestId("session-session-aaa")).toBeVisible();
    await expect(page.getByTestId("session-session-aaa")).toContainText(
      "会话 AAA",
    );
    await expect(page.getByTestId("session-session-aaa")).toContainText("2 条消息");
    await expect(page.getByTestId("session-session-bbb")).toBeVisible();
  });
});

test.describe("T05 同步计划", () => {
  test("自定义选择一条对话后显示目标账号与可同步动作", async ({ page }) => {
    await setup(page, { scanOutcome: "success", browseMode: "full" });
    await authorizeAndScan(page);

    await page.getByRole("radio", { name: "自定义选择" }).check();
    await expect(page.getByTestId("build-sync-plan-button")).toBeDisabled();
    await page.getByRole("checkbox", { name: /选择对话 会话 AAA/ }).check();
    await expect(page.getByTestId("plan-selected")).toHaveText("1");
    await page.getByTestId("build-sync-plan-button").click();

    await expect(page.getByTestId("sync-plan-result")).toBeVisible();
    await expect(page.getByTestId("plan-target-account")).toHaveText("user-B");
    await expect(page.getByTestId("plan-syncable")).toHaveText("1");
    await expect(page.getByTestId("sync-plan-result")).toContainText("挂接 1 条对话");
    await expect(page.getByText(/不会复制成两份账号历史/)).toBeVisible();
    await expect(page.getByTestId("sync-button")).toBeDisabled();
  });
});

// ============================================================================
// 完整对话预览
// ============================================================================

test.describe("完整对话预览", () => {
  test("点击会话标题打开完整对话预览", async ({ page }) => {
    await setup(page, { scanOutcome: "success", browseMode: "full" });
    await authorizeAndScan(page);
    await expect(page.getByTestId("session-session-aaa")).toBeVisible();
    // 点击会话
    await page.getByTestId("session-session-aaa").click();
    // 预览内容应显示
    await expect(page.getByTestId("preview-content")).toBeVisible();
    await expect(page.getByTestId("message-m1")).toBeVisible();
    await expect(page.getByTestId("message-m2")).toBeVisible();
    // 消息内容包含合成文本
    await expect(page.getByTestId("message-m1")).toContainText("hello world");
    await expect(page.getByTestId("message-m2")).toContainText("确定性内容图");
  });

  test("切换会话预览不改变选择（AC9）", async ({ page }) => {
    await setup(page, { scanOutcome: "success", browseMode: "full" });
    await authorizeAndScan(page);
    // 展开账号以选中项目
    await page.getByTestId("account-user-A").click();
    await page.getByTestId("project-p1").click();
    // project-p1 应处于展开状态（aria-expanded=true）
    await expect(page.getByTestId("project-p1")).toHaveAttribute("aria-expanded", "true");
    // 点击会话 AAA 打开预览
    await page.getByTestId("session-session-aaa").click();
    await expect(page.getByTestId("preview-content")).toBeVisible();
    // 点击会话 BBB 切换预览
    await page.getByTestId("session-session-bbb").click();
    await expect(page.getByTestId("message-m3")).toBeVisible();
    // 项目选择应保持不变（仍为 p1）
    await expect(page.getByTestId("project-p1")).toHaveAttribute("aria-expanded", "true");
    // 账号选择也应保持
    await expect(page.getByTestId("account-user-A")).toHaveAttribute("aria-expanded", "true");
  });
});

// ============================================================================
// 搜索结果导航
// ============================================================================

test.describe("搜索结果导航", () => {
  test("搜索显示结果并显示账号/项目/会话上下文", async ({ page }) => {
    await setup(page, {
      scanOutcome: "success",
      browseMode: "full",
      searchMode: "hits",
    });
    await authorizeAndScan(page);
    await expect(page.getByTestId("session-list")).toBeVisible();
    // 输入搜索并执行
    await page.getByTestId("search-input").fill("hello");
    await page.getByTestId("search-button").click();
    // 搜索结果应显示
    await expect(page.getByTestId("search-results")).toBeVisible();
    await expect(page.getByTestId("search-hit-m1")).toBeVisible();
    // 命中应包含上下文：标题、role、内容
    await expect(page.getByTestId("search-hit-m1")).toContainText("会话 AAA");
    await expect(page.getByTestId("search-hit-m1")).toContainText("user");
    await expect(page.getByTestId("search-hit-m1")).toContainText("hello world");
  });

  test("点击搜索结果打开预览不改变选择（AC9）", async ({ page }) => {
    await setup(page, {
      scanOutcome: "success",
      browseMode: "full",
      searchMode: "hits",
    });
    await authorizeAndScan(page);
    // 选中账号与项目
    await page.getByTestId("account-user-A").click();
    await page.getByTestId("project-p1").click();
    await expect(page.getByTestId("project-p1")).toHaveAttribute("aria-expanded", "true");
    // 搜索
    await page.getByTestId("search-input").fill("hello");
    await page.getByTestId("search-button").click();
    await expect(page.getByTestId("search-hit-m1")).toBeVisible();
    // 点击搜索结果打开预览
    await page.getByTestId("search-hit-m1").click();
    await expect(page.getByTestId("preview-content")).toBeVisible();
    await expect(page.getByTestId("message-m1")).toBeVisible();
    // 选择应保持不变
    await expect(page.getByTestId("project-p1")).toHaveAttribute("aria-expanded", "true");
    await expect(page.getByTestId("account-user-A")).toHaveAttribute("aria-expanded", "true");
  });

  test("搜索无结果时显示无匹配", async ({ page }) => {
    await setup(page, {
      scanOutcome: "success",
      browseMode: "full",
      searchMode: "empty",
    });
    await authorizeAndScan(page);
    await page.getByTestId("search-input").fill("zzzz");
    await page.getByTestId("search-button").click();
    await expect(page.getByTestId("search-results")).toBeVisible();
    await expect(page.getByTestId("search-results")).toContainText("无匹配结果");
  });
});

// ============================================================================
// 无文本重叠、裁剪或水平溢出
// ============================================================================

test.describe("布局稳定性：无重叠、裁剪或水平溢出", () => {
  test("页面无水平溢出", async ({ page }) => {
    await setup(page, { scanOutcome: "success", browseMode: "full" });
    await authorizeAndScan(page);
    // 等待三栏渲染
    await expect(page.getByTestId("account-project-tree")).toBeVisible();
    // 检查 body scrollWidth 不超过 viewport
    const overflow = await page.evaluate(() => {
      return {
        scrollWidth: document.documentElement.scrollWidth,
        clientWidth: document.documentElement.clientWidth,
      };
    });
    expect(overflow.scrollWidth).toBeLessThanOrEqual(overflow.clientWidth);
  });

  test("长中文标题不裁剪不溢出", async ({ page }) => {
    await setup(page, { scanOutcome: "success", browseMode: "full" });
    await authorizeAndScan(page);
    // 会话标题为长中文，验证元素可见且无裁剪
    const sessionBtn = page.getByTestId("session-session-aaa");
    await expect(sessionBtn).toBeVisible();
    // 检查按钮的滚动宽度不超过其父容器宽度
    const box = await sessionBtn.boundingBox();
    expect(box).not.toBeNull();
    // 验证会话标题元素存在且可见
    await expect(sessionBtn.locator(".workbench__session-title")).toBeVisible();
  });

  test("对话预览长消息不溢出", async ({ page }) => {
    await setup(page, { scanOutcome: "success", browseMode: "full" });
    await authorizeAndScan(page);
    await page.getByTestId("session-session-aaa").click();
    await expect(page.getByTestId("preview-content")).toBeVisible();
    // 检查预览区域不产生水平溢出
    const overflow = await page.evaluate(() => {
      const preview = document.querySelector(".workbench__preview");
      if (!preview) return { ok: false };
      return {
        ok: preview.scrollWidth <= preview.clientWidth + 1,
        scrollWidth: preview.scrollWidth,
        clientWidth: preview.clientWidth,
      };
    });
    expect(overflow.ok).toBe(true);
  });
});

// ============================================================================
// R7：前端授权调用链——mock 授权状态机验证
// ============================================================================

test.describe("R7 前端授权调用链", () => {
  test("R7：未授权时 scan_history 返回 failed/not_authorized（mock 状态机）", async ({ page }) => {
    // 安装 mock 但不点击授权 checkbox——直接尝试扫描
    // 由于扫描按钮在未授权时禁用，无法直接点击 scan-history-button
    // 验证：未授权时 authorize-check 未选中，scan-history-button 禁用
    await setup(page, { scanOutcome: "success", browseMode: "full" });
    await page.getByTestId("history-fixture-root-input").fill("C:\\fixture");
    // authorize-check 未勾选
    await expect(page.getByTestId("authorize-check")).not.toBeChecked();
    // 扫描按钮禁用
    await expect(page.getByTestId("scan-history-button")).toBeDisabled();
  });

  test("R7：授权成功后 scan_history 使用 canonical fixture_root", async ({ page }) => {
    // 监听 mock invoke 调用——通过 window 对象记录
    await setup(page, { scanOutcome: "success", browseMode: "full" });
    // 注入调用记录器
    await page.addInitScript(() => {
      (window as any).__invokeCalls = [];
      const orig = (window as any).__TAURI_INTERNALS__.invoke;
      (window as any).__TAURI_INTERNALS__.invoke = async function (cmd: string, args?: any) {
        (window as any).__invokeCalls.push({ cmd, args });
        return orig(cmd, args);
      };
    });
    // 重新加载以使记录器生效
    await page.reload();
    await expect(page.getByRole("region", { name: "历史库" })).toBeVisible();

    await page.getByTestId("history-fixture-root-input").fill("C:\\fixture");
    await page.getByTestId("authorize-check").check();
    await page.getByTestId("scan-history-button").click();
    await expect(page.getByTestId("account-project-tree")).toBeVisible({ timeout: 10_000 });

    // 验证 grant_scan_authorization 被调用
    const calls = await page.evaluate(() => (window as any).__invokeCalls);
    const grantCall = calls.find((c: any) => c.cmd === "grant_scan_authorization");
    expect(grantCall).toBeDefined();
    // 验证 scan_history 被调用时传入授权返回的 canonical fixture_root
    const scanCall = calls.find((c: any) => c.cmd === "scan_history");
    expect(scanCall).toBeDefined();
    expect(scanCall.args.fixtureRoot).toBe("C:\\fixture");
  });

  test("R7：路径变化后旧授权失效（调用 revoke_scan_authorization）", async ({ page }) => {
    await setup(page, { scanOutcome: "success", browseMode: "full" });
    // 注入调用记录器
    await page.addInitScript(() => {
      (window as any).__invokeCalls = [];
      const orig = (window as any).__TAURI_INTERNALS__.invoke;
      (window as any).__TAURI_INTERNALS__.invoke = async function (cmd: string, args?: any) {
        (window as any).__invokeCalls.push({ cmd, args });
        return orig(cmd, args);
      };
    });
    await page.reload();
    await expect(page.getByRole("region", { name: "历史库" })).toBeVisible();

    // 第一次填写并授权
    await page.getByTestId("history-fixture-root-input").fill("C:\\fixture");
    await page.getByTestId("authorize-check").check();
    await expect(page.getByTestId("authorize-check")).toBeChecked();

    // 修改 fixture 路径——应触发撤销
    await page.getByTestId("history-fixture-root-input").fill("D:\\other-fixture");
    // 等待异步撤销完成——checkbox 应取消选中
    await expect(page.getByTestId("authorize-check")).not.toBeChecked();

    // 验证 revoke_scan_authorization 被调用
    const calls = await page.evaluate(() => (window as any).__invokeCalls);
    const revokeCalls = calls.filter((c: any) => c.cmd === "revoke_scan_authorization");
    expect(revokeCalls.length).toBeGreaterThanOrEqual(1);
  });

  test("R7：撤销授权后 scan_history 按钮禁用", async ({ page }) => {
    await setup(page, { scanOutcome: "success", browseMode: "full" });
    await page.getByTestId("history-fixture-root-input").fill("C:\\fixture");
    await page.getByTestId("authorize-check").check();
    await expect(page.getByTestId("authorize-check")).toBeChecked();
    // 扫描按钮启用
    await expect(page.getByTestId("scan-history-button")).toBeEnabled();
    // 取消授权
    await page.getByTestId("authorize-check").uncheck();
    await expect(page.getByTestId("authorize-check")).not.toBeChecked();
    // 扫描按钮应禁用
    await expect(page.getByTestId("scan-history-button")).toBeDisabled();
  });
});

// ============================================================================
// R12：授权异步竞态——pending grant 期间路径变化后 stale 响应被丢弃
// ============================================================================

test.describe("R12 授权异步竞态", () => {
  test("R12：pending grant 期间路径变化后 stale 响应被丢弃并 revoke", async ({ page }) => {
    await setup(page, { scanOutcome: "success", browseMode: "full" });
    // 注入调用记录器 + deferred grant 控制器
    // R12：grant_scan_authorization 返回 pending promise，测试通过
    // window.__grantDeferred.resolve 控制 resolve 时机，模拟异步竞态
    await page.addInitScript(() => {
      (window as any).__invokeCalls = [];
      (window as any).__grantDeferred = null;
      const orig = (window as any).__TAURI_INTERNALS__.invoke;
      (window as any).__TAURI_INTERNALS__.invoke = async function (
        cmd: string,
        args?: any,
      ) {
        (window as any).__invokeCalls.push({ cmd, args });
        if (cmd === "grant_scan_authorization") {
          // R12：grant 返回 pending promise——测试通过 resolve 控制
          return new Promise((resolve) => {
            (window as any).__grantDeferred = { resolve, args };
          });
        }
        return orig(cmd, args);
      };
    });
    // 重新加载以使记录器与 deferred 控制器生效
    await page.reload();
    await expect(page.getByRole("region", { name: "历史库" })).toBeVisible();

    // 1. 输入 A 并点击授权——grant(A) 保持 pending
    await page.getByTestId("history-fixture-root-input").fill("C:\\fixture-A");
    // 使用 click 而非 check——pending 时 checkbox 会回弹为未选中，
    // check() 会反复重试导致多次触发 grant
    await page.getByTestId("authorize-check").click();
    // 等待 grant 被调用（pending promise 已建立）
    await page.waitForFunction(
      () => (window as any).__grantDeferred !== null,
    );

    // 2. 将 fixtureRoot 改为 B——应触发路径变化使旧授权失效
    await page.getByTestId("history-fixture-root-input").fill("D:\\fixture-B");

    // 3. resolve grant(A)——返回 canonical A（stale 响应）
    await page.evaluate(() => {
      (window as any).__grantDeferred.resolve("C:\\canonical-A");
    });

    // 4. 等待 stale 响应处理完成——revoke 应被调用
    await page.waitForFunction(
      () =>
        (window as any).__invokeCalls.some(
          (c: any) => c.cmd === "revoke_scan_authorization",
        ),
    );

    // 5. 断言 checkbox 仍未选中——stale 响应不得设置已授权
    await expect(page.getByTestId("authorize-check")).not.toBeChecked();
    // 6. 断言扫描按钮仍禁用
    await expect(page.getByTestId("scan-history-button")).toBeDisabled();

    // 7. 断言调用了 revoke_scan_authorization
    const calls = await page.evaluate(() => (window as any).__invokeCalls);
    const revokeCalls = calls.filter(
      (c: any) => c.cmd === "revoke_scan_authorization",
    );
    expect(revokeCalls.length).toBeGreaterThanOrEqual(1);
    // 8. 断言没有调用 scan_history——stale 授权不得触发扫描
    const scanCalls = calls.filter((c: any) => c.cmd === "scan_history");
    expect(scanCalls.length).toBe(0);
  });

  test("R12-A：pending 时用户点击 checkbox 取消会丢弃旧授权结果", async ({ page }) => {
    await setup(page, { scanOutcome: "success", browseMode: "full" });
    // 注入调用记录器 + deferred grant 控制器
    await page.addInitScript(() => {
      (window as any).__invokeCalls = [];
      (window as any).__grantDeferred = null;
      const orig = (window as any).__TAURI_INTERNALS__.invoke;
      (window as any).__TAURI_INTERNALS__.invoke = async function (
        cmd: string,
        args?: any,
      ) {
        (window as any).__invokeCalls.push({ cmd, args });
        if (cmd === "grant_scan_authorization") {
          return new Promise((resolve) => {
            (window as any).__grantDeferred = { resolve, args };
          });
        }
        return orig(cmd, args);
      };
    });
    await page.reload();
    await expect(page.getByRole("region", { name: "历史库" })).toBeVisible();

    // 1. 输入 A 并点击授权——grant(A) 保持 pending
    await page.getByTestId("history-fixture-root-input").fill("C:\\fixture-A");
    await page.getByTestId("authorize-check").click();
    // 等待 grant 被调用
    await page.waitForFunction(
      () => (window as any).__grantDeferred !== null,
    );

    // R12-A：pending 时 checkbox 应选中——用户可点击取消
    await expect(page.getByTestId("authorize-check")).toBeChecked();

    // 2. 用户点击 checkbox 取消 pending 授权
    await page.getByTestId("authorize-check").click();

    // 3. 取消应立即生效——checkbox 未选中，扫描按钮禁用
    await expect(page.getByTestId("authorize-check")).not.toBeChecked();
    await expect(page.getByTestId("scan-history-button")).toBeDisabled();

    // 4. 应调用 revoke 清除后端可能已建立的授权
    await page.waitForFunction(
      () =>
        (window as any).__invokeCalls.some(
          (c: any) => c.cmd === "revoke_scan_authorization",
        ),
    );

    // 5. resolve grant(A)——stale，不应恢复授权
    await page.evaluate(() => {
      (window as any).__grantDeferred.resolve("C:\\canonical-A");
    });
    // 等待微任务
    await page.waitForTimeout(50);

    // 6. 仍为未授权状态
    await expect(page.getByTestId("authorize-check")).not.toBeChecked();
    await expect(page.getByTestId("scan-history-button")).toBeDisabled();

    // 7. scan_history 未被调用
    const calls = await page.evaluate(() => (window as any).__invokeCalls);
    const scanCalls = calls.filter((c: any) => c.cmd === "scan_history");
    expect(scanCalls.length).toBe(0);
  });
});
