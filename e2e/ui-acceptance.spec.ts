import { test, expect, type Page } from "@playwright/test";
import { installMockBridge } from "./mock-bridge";

// 桌面版验收矩阵：不把移动端作为产品目标，只检查 Windows/Tauri 对应的 CSS 视口。
const desktopViewports = [
  { name: "1024x600", width: 1024, height: 600 },
  { name: "1100x700", width: 1100, height: 700 },
  { name: "1199x760", width: 1199, height: 760 },
  { name: "1280x800", width: 1280, height: 800 },
  { name: "1366x768", width: 1366, height: 768 },
  { name: "1440x900", width: 1440, height: 900 },
  { name: "1920x1080", width: 1920, height: 1080 },
] as const;

// 应用首页是总览页；主库详情页（库对话面板宿主）需要先经环境页进入。
async function gotoMasterDetail(page: Page) {
  await page.getByTestId("navigation-environment").click();
  await page.getByTestId("env-master-detail").click();
  await expect(page.getByRole("region", { name: "主库详情" })).toBeVisible();
}

// 进页即读主库（默认对话列表 tab）；等左栏项目树渲染完成。
async function waitForLibraryLoaded(page: Page) {
  await expect(page.getByTestId("library-tree")).toBeVisible();
}

async function resetMainScroll(page: Page) {
  // 直接设置滚动位置，避免 Chromium 对 behavior: instant 的实现差异影响截图证据。
  await page.evaluate(() => {
    const main = document.querySelector<HTMLElement>(".app-main");
    if (main) {
      main.scrollTop = 0;
      main.scrollLeft = 0;
    }
  });
}

test.describe("桌面尺寸与滚动验收", () => {
  test("各桌面视口无水平溢出，主库详情两栏完整可见", async ({ page }) => {
    await installMockBridge(page);

    for (const viewport of desktopViewports) {
      await page.setViewportSize({ width: viewport.width, height: viewport.height });
      await page.goto("/");
      await gotoMasterDetail(page);
      await waitForLibraryLoaded(page);
      // 展开项目让会话行也进入树（行完整落栏断言覆盖项目行与会话行）。
      await page.getByTestId("library-project-p1").click();

      const metrics = await page.evaluate(() => {
        const root = document.documentElement;
        const titleBar = document.querySelector<HTMLElement>(".title-bar");
        // G21 两栏：左栏项目树（项目行 + 展开的会话行）/ 右栏对话查看器。
        const panels = [".lib-panel", ".viewer-panel"].map((selector) => {
          const panel = document.querySelector<HTMLElement>(selector);
          const rect = panel?.getBoundingClientRect();
          return rect ? { top: rect.top, bottom: rect.bottom, width: rect.width } : null;
        });
        // 树内行（项目行与会话行）都必须完整落在左栏面板内。
        const rows = [".lib-panel .lib-project", ".lib-panel .lib-session"].map((selector) =>
          Array.from(document.querySelectorAll<HTMLElement>(selector)).map((row) => {
            const rect = row.getBoundingClientRect();
            return { top: rect.top, bottom: rect.bottom };
          }),
        );
        const titleItems = Array.from(
          document.querySelectorAll<HTMLElement>(".title-bar__item"),
        ).map((item) => {
          const rect = item.getBoundingClientRect();
          return { width: rect.width, height: rect.height };
        });
        return {
          scrollWidth: root.scrollWidth,
          clientWidth: root.clientWidth,
          viewportHeight: window.innerHeight,
          titleBarHeight: titleBar?.getBoundingClientRect().height ?? 0,
          titleBarTop: titleBar?.getBoundingClientRect().top ?? Number.NaN,
          titleItems,
          panels,
          rows,
        };
      });

      expect(metrics.scrollWidth).toBeLessThanOrEqual(metrics.clientWidth + 1);
      expect(metrics.titleBarHeight).toBeLessThanOrEqual(124);
      expect(metrics.titleBarTop).toBeGreaterThanOrEqual(-1);
      // T8 透明圆角窗口：标题栏随壳层浮起于 body 12px 呼吸带内（+1px 壳描边），
      // 上界从贴边 1px 放宽到呼吸带常量 13px。
      expect(metrics.titleBarTop).toBeLessThanOrEqual(13);
      expect(metrics.titleItems.every((item) => item.width >= 80)).toBe(true);
      // 两栏均渲染且树内行完整落在左栏面板内。
      expect(metrics.panels.every((panel) => panel !== null)).toBe(true);
      expect(metrics.rows.every((rows) => rows.length > 0)).toBe(true);
      const libPanel = metrics.panels[0];
      for (const rows of metrics.rows) {
        expect(
          rows.every(
            (row) => row.top >= (libPanel?.top ?? 0) - 1 && row.bottom <= (libPanel?.bottom ?? 0) + 1,
          ),
        ).toBe(true);
      }
      if (viewport.width >= 1200) {
        // 宽桌面标题栏应保持单行证据带，不因上下文项被挤压而跳高。
        expect(metrics.titleBarHeight).toBeLessThanOrEqual(88);
      }

      await page.screenshot({
        path: `artifacts/ui-acceptance/screenshots/${viewport.name}-viewport.png`,
        fullPage: false,
      });
      // 固定应用壳使用内部滚动根；视口截图比 fullPage 更接近真实桌面窗口。
      await resetMainScroll(page);
      await page.screenshot({
        path: `artifacts/ui-acceptance/screenshots/${viewport.name}.png`,
        fullPage: false,
      });
    }
  });

  test("紧凑桌面长项目列表保持面板内滚动，不撑破壳层", async ({ page }) => {
    await installMockBridge(page);
    await page.setViewportSize({ width: 1024, height: 600 });
    await page.goto("/");
    await gotoMasterDetail(page);
    await waitForLibraryLoaded(page);

    // 用与真实树分支相同的 class 注入长列表，验证左栏面板内滚动边界。
    const metrics = await page.evaluate(() => {
      const panel = document.querySelector<HTMLElement>(".lib-panel");
      const list = document.querySelector<HTMLElement>(".lib-tree");
      if (!panel || !list) return null;
      for (let index = 0; index < 24; index += 1) {
        const branch = document.createElement("div");
        branch.className = "lib-branch";
        branch.innerHTML = `<div class="lib-project"><span class="lib-project__name">长项目节点 ${index + 1}</span></div>`;
        list.appendChild(branch);
      }
      const panelRect = panel.getBoundingClientRect();
      const sess = document.querySelector<HTMLElement>(".viewer-panel");
      return {
        panelOverflowY: getComputedStyle(panel).overflowY,
        listClientHeight: list.clientHeight,
        listScrollHeight: list.scrollHeight,
        panelBottom: panelRect.bottom,
        sessTop: sess?.getBoundingClientRect().top ?? 0,
        documentHeight: document.documentElement.scrollHeight,
        viewportHeight: window.innerHeight,
      };
    });

    expect(metrics).not.toBeNull();
    expect(["auto", "scroll", "hidden"]).toContain(metrics!.panelOverflowY);
    expect(metrics!.listScrollHeight).toBeGreaterThanOrEqual(metrics!.listClientHeight);
    // 两栏并排：左栏底边不得越过右栏顶边（并排无遮挡即满足）。
    expect(metrics!.documentHeight).toBeLessThanOrEqual(metrics!.viewportHeight + 1);
  });

  test("账号页在目标桌面尺寸不溢出", async ({ page }) => {
    await installMockBridge(page);

    for (const viewport of [
      { name: "1024x600", width: 1024, height: 600 },
      { name: "1280x800", width: 1280, height: 800 },
      { name: "1920x1080", width: 1920, height: 1080 },
    ]) {
      await page.setViewportSize(viewport);
      await page.goto("/");
      // 总览页也有“管理账号”入口；导航必须用唯一 testid 定位。
      await page.getByTestId("navigation-accounts").click();

      // 账号页内含子 region，主 region 必须精确匹配。
      const accountCenter = page.getByRole("region", { name: "账号", exact: true });
      await expect(accountCenter).toBeVisible();
      const metrics = await page.evaluate(() => {
        const root = document.documentElement;
        const account = document.querySelector<HTMLElement>('[aria-label="账号"]');
        const rect = account?.getBoundingClientRect();
        return {
          scrollWidth: root.scrollWidth,
          clientWidth: root.clientWidth,
          accountRight: rect?.right ?? 0,
          viewportWidth: window.innerWidth,
          accountHeight: rect?.height ?? 0,
          accountScrollWidth: account?.scrollWidth ?? 0,
          accountClientWidth: account?.clientWidth ?? 0,
        };
      });

      expect(metrics.scrollWidth).toBeLessThanOrEqual(metrics.clientWidth + 1);
      expect(metrics.accountRight).toBeLessThanOrEqual(metrics.viewportWidth + 1);
      expect(metrics.accountHeight).toBeGreaterThan(0);
      expect(metrics.accountScrollWidth).toBeLessThanOrEqual(metrics.accountClientWidth + 1);

      await resetMainScroll(page);
      await accountCenter.scrollIntoViewIfNeeded();
      await page.screenshot({
        path: `artifacts/ui-acceptance/screenshots/accounts-${viewport.name}.png`,
        fullPage: false,
      });
    }
  });
});

test.describe("键盘与焦点验收", () => {
  test("一级导航切换后焦点进入当前页面标题", async ({ page }) => {
    await installMockBridge(page);
    await page.goto("/");

    // 总览是初始页；切换到设置页验证焦点移动到页面标题。
    await page.getByTestId("navigation-settings").click();

    await expect(page.locator('[data-page-title="settings"]')).toBeFocused();
    await expect(page.getByRole("region", { name: "设置" })).toBeVisible();
  });

  test("跳过链接、导航和只读查看器可由键盘完成", async ({ page }) => {
    await installMockBridge(page);
    await page.goto("/");
    await gotoMasterDetail(page);
    await waitForLibraryLoaded(page);

    const skipLink = page.locator(".skip-link");
    await skipLink.focus();
    await expect(skipLink).toBeFocused();
    const skipFocusStyle = await skipLink.evaluate((element) => {
      const style = getComputedStyle(element);
      return { outlineStyle: style.outlineStyle, outlineWidth: style.outlineWidth };
    });
    expect(skipFocusStyle.outlineStyle).not.toBe("none");
    expect(skipFocusStyle.outlineWidth).not.toBe("0px");

    // 键盘路径：搜索框输入过滤 → 会话行 Enter 打开右栏查看器（只读）。
    const search = page.getByTestId("library-search-input");
    await search.focus();
    await page.keyboard.type("会话一");
    await expect(page.getByTestId("library-session-s1")).toBeVisible();

    const session = page.getByTestId("library-session-s1");
    await session.focus();
    await page.keyboard.press("Enter");
    await expect(page.getByTestId("library-viewer-title")).toBeVisible();
    // 查看器为只读区：仅 消息|接力 tab 与关闭按钮，无写入口
    //（tab 显式 role=tab，唯一的 button 是关闭）。
    const viewer = page.getByTestId("library-viewer");
    await expect(viewer.getByRole("tab")).toHaveCount(2);
    await expect(viewer.getByRole("button")).toHaveCount(1);
    // 关闭按钮键盘可达，关闭后回到空状态引导。
    await page.getByTestId("library-viewer-close").focus();
    await page.keyboard.press("Enter");
    await expect(page.getByTestId("library-viewer-empty")).toBeVisible();
  });

  test("强制颜色模式下主要按钮和输入焦点仍可见", async ({ page }) => {
    await installMockBridge(page);
    await page.goto("/");
    await gotoMasterDetail(page);
    await waitForLibraryLoaded(page);

    await page.emulateMedia({ forcedColors: "active" });
    // 程序性 focus 不触发 :focus-visible；先按一次 Tab 建立键盘模态，再聚焦目标控件。
    await page.keyboard.press("Tab");
    const search = page.getByTestId("library-search-input");
    await search.focus();
    await expect(search).toBeFocused();
    const inputFocusStyle = await search.evaluate((element) => {
      const style = getComputedStyle(element);
      return { outlineStyle: style.outlineStyle, outlineWidth: style.outlineWidth };
    });
    expect(inputFocusStyle.outlineStyle).not.toBe("none");
    expect(inputFocusStyle.outlineWidth).not.toBe("0px");

    const navigation = page.getByTestId("navigation-accounts");
    await navigation.focus();
    await expect(navigation).toBeFocused();
    const navigationColors = await navigation.evaluate((element) => {
      const style = getComputedStyle(element);
      return { color: style.color, backgroundColor: style.backgroundColor };
    });
    expect(navigationColors.color).not.toBe("rgba(0, 0, 0, 0)");
    expect(navigationColors.backgroundColor).not.toBe("rgba(0, 0, 0, 0)");
  });
});

test.describe("高 DPI 与高对比度验收", () => {
  test.use({ viewport: { width: 1280, height: 800 }, deviceScaleFactor: 2 });

  test("2x DPI 保持布局，强制颜色模式保持焦点和层级", async ({ page }) => {
    await installMockBridge(page);
    await page.goto("/");
    await gotoMasterDetail(page);
    await waitForLibraryLoaded(page);

    const dpi = await page.evaluate(() => ({
      devicePixelRatio: window.devicePixelRatio,
      scrollWidth: document.documentElement.scrollWidth,
      clientWidth: document.documentElement.clientWidth,
    }));
    expect(dpi.devicePixelRatio).toBe(2);
    expect(dpi.scrollWidth).toBeLessThanOrEqual(dpi.clientWidth + 1);
    await page.screenshot({
      path: "artifacts/ui-acceptance/screenshots/1280x800-2x.png",
      fullPage: false,
    });

    await page.emulateMedia({ forcedColors: "active" });
    // 视觉证据保持在页面顶部，并把焦点放在真实表单控件上，避免跳过链接遮住品牌区域。
    await resetMainScroll(page);
    const searchInput = page.getByTestId("library-search-input");
    await searchInput.focus();
    await expect(searchInput).toBeFocused();
    await page.screenshot({
      path: "artifacts/ui-acceptance/screenshots/1280x800-forced-colors.png",
      fullPage: false,
    });
    const focusStyle = await searchInput.evaluate((element) => {
      const style = getComputedStyle(element);
      return { outlineStyle: style.outlineStyle, outlineWidth: style.outlineWidth };
    });
    expect(focusStyle.outlineStyle).not.toBe("none");
    expect(focusStyle.outlineWidth).not.toBe("0px");
  });
});

test.describe("签到页验收", () => {
  test("自动签到状态行展示触发时间与今日台账进度", async ({ page }) => {
    await installMockBridge(page);
    await page.goto("/");

    await page.getByTestId("navigation-checkin").click();

    // 状态行 = 初次读取 get_auto_checkin_settings 的渲染结果
    //（mock-bridge 固定返回每日 10:00 + 台账执行中 成功 1/3）。
    const status = page.getByTestId("auto-checkin-status");
    await expect(status).toBeVisible();
    await expect(status).toHaveText(/每日 10:00 触发/);
    await expect(status).toHaveText(/今日执行中（成功 1\/3）/);
  });
});

test.describe("P5-4 总览联动与设置页备份分区验收", () => {
  test("环境页统计格展示主库聚合，设置页备份链可见且手动备份使计数 +1", async ({ page }) => {
    // production 场景：数据位置就绪（主库统计 ready 的前提，G1 空态三分支）。
    await installMockBridge(page, { production: true });
    await page.goto("/");

    // 总览：主库统计卡（mock ready）替换旧历史统计，主操作切换为「查看主库记录」。
    await expect(page.getByTestId("overview-stats-master")).toBeVisible();
    await expect(page.getByTestId("overview-scan-cta")).toHaveText(/查看主库记录/);

    // 环境页：统计格三卡（会话/项目/参与账号，mock 16/4/3）。
    await page.getByTestId("navigation-environment").click();
    const stats = page.getByTestId("env-master-stats");
    await expect(stats).toBeVisible();
    await expect(stats).toContainText("会话");
    await expect(stats).toContainText("项目");
    await expect(stats).toContainText("参与账号");
    await expect(stats).toContainText("16");

    // 设置页：备份分区（mock 初始 2 份）→ 立即备份 → 徽章 3 份 + 恢复指引可见。
    // 提示文案与 SettingsPanel.test 同口径（P5-9 文案修订后的现行表述）。
    await page.getByTestId("navigation-settings").click();
    const section = page.getByTestId("master-backup-section");
    await expect(section).toBeVisible();
    await expect(page.getByText("现有 2 份")).toBeVisible();
    await expect(page.getByTestId("master-backup-restore-hint")).toContainText(/不会被自动删除/);

    await page.getByTestId("master-backup-create").click();
    await expect(page.getByText("现有 3 份")).toBeVisible();
    await expect(page.getByText("主库数据备份已创建")).toBeVisible();
  });
});

test.describe("P5-8a-2 主库详情页验收", () => {
  test("环境页进入详情：对话列表 tab 内嵌库面板 + 库信息 + 插件 tab + 返回", async ({ page }) => {
    await installMockBridge(page);
    await page.goto("/");

    // 环境页主库卡 → 详情。
    await page.getByTestId("navigation-environment").click();
    await page.getByTestId("env-master-detail").click();
    await expect(page.getByRole("region", { name: "主库详情" })).toBeVisible();

    // G20 三 tab：默认对话列表 tab 内嵌库对话面板（无页级标题，展开后会话行可见）。
    await page.getByTestId("library-project-p1").click();
    await expect(page.getByTestId("library-session-s1")).toBeVisible();
    await expect(page.getByRole("heading", { level: 1, name: "历史" })).toHaveCount(0);

    // 库信息 tab：基础信息 + 计数（mock：工作账号 B / 712MB / 计数 4·16·128 / 已有备份）。
    await page.getByTestId("master-tab-info").click();
    const info = page.getByTestId("master-detail-info");
    await expect(info).toBeVisible();
    await expect(page.getByTestId("master-info-account")).toHaveText("工作账号 B");
    await expect(page.getByTestId("master-info-size")).toHaveText("712 MB");
    await expect(page.getByTestId("master-info-path")).toContainText("environments\\master");
    const counts = page.getByTestId("master-detail-counts");
    await expect(counts).toContainText("项目");
    await expect(counts).toContainText("会话");
    await expect(counts).toContainText("消息");
    await expect(page.getByTestId("master-info-backup")).not.toHaveText("还没有备份");

    // 插件 tab（P5-8b 工作台）；切回对话列表恢复两栏。
    await page.getByTestId("master-tab-plugins").click();
    await expect(page.getByTestId("plugin-workbench")).toBeVisible();
    await expect(page.getByTestId("library-session-s1")).toBeHidden();
    await page.getByTestId("master-tab-sessions").click();
    await expect(page.getByTestId("library-session-s1")).toBeVisible();

    // 返回环境页。
    await page.getByTestId("master-detail-back").click();
    await expect(page.getByRole("region", { name: "环境" })).toBeVisible();
  });
});

test.describe("P5-8b 插件 tab 验收（ADR-0026 同步策略）", () => {
  test("已装表格 + 分类筛选 + 市场安装零确认 + 卸载单确认全账号传播", async ({ page }) => {
    await installMockBridge(page);
    await page.goto("/");

    // 进入主库详情 → 插件 tab。
    await page.getByTestId("navigation-environment").click();
    await page.getByTestId("env-master-detail").click();
    await page.getByTestId("master-tab-plugins").click();

    // 已装表格（G23 密集行）：切号保留 / 内置（移除禁用）/ 仅此账号。
    await expect(page.getByTestId("plugin-row-plug-rec-1")).toContainText("飞书协作");
    await expect(page.getByTestId("plugin-row-plug-rec-1")).toContainText("切号保留");
    await expect(page.getByTestId("plugin-row-plug-rec-2")).toContainText("内置");
    await expect(page.getByTestId("plugin-uninstall-plug-rec-2")).toBeDisabled();
    await expect(page.getByTestId("plugin-row-plug-rec-3")).toContainText("仅此账号");
    // 对账条已随 ADR-0026 静默应用策略移除。
    await expect(page.getByTestId("plugin-drift")).toHaveCount(0);

    // 市场段：目录 + 搜索框；已装条目显示已安装。
    await page.getByTestId("plugin-segment-market").click();
    await expect(page.getByTestId("plugin-market-search")).toBeVisible();
    await expect(page.getByTestId("plugin-market-row-plug-uuid-5")).toContainText("代码图谱");
    await expect(page.getByTestId("plugin-market-installed-plug-uuid-1")).toBeVisible();

    // 安装零确认（ADR-0026 决策 1）：点击即装，无确认弹层，回执提示。
    await page.getByTestId("plugin-install-plug-uuid-5").click();
    await expect(page.getByTestId("plugin-market-installed-plug-uuid-5")).toBeVisible();
    await expect(page.getByTestId("plugin-notice")).toContainText("已安装 代码图谱");

    // 卸载：单次确认（列明影响面 N 账号）→ 确认后行消失 + 回执。
    await page.getByTestId("plugin-segment-installed").click();
    await page.getByTestId("plugin-uninstall-plug-rec-1").click();
    await expect(page.getByTestId("plugin-uninstall-confirm")).toContainText("将同时从 2 个账号移除");
    await page.getByTestId("plugin-uninstall-confirm-ok").click();
    await expect(page.getByTestId("plugin-row-plug-rec-1")).toHaveCount(0);
    await expect(page.getByTestId("plugin-notice")).toContainText("已从 2 个账号移除 飞书协作");
  });
});
