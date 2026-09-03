import { test, expect, type Page } from "@playwright/test";
import { installMockBridge } from "./mock-bridge";

// 布局诊断（P5-3 历史页主库视图）：主要工作区矩形不能真实重叠，
// 紧凑桌面左栏项目列表滚动到底部时末项完整可见。

const viewports = [
  { name: "1024x600", width: 1024, height: 600 },
  { name: "1100x700", width: 1100, height: 700 },
  { name: "1199x760", width: 1199, height: 760 },
  { name: "1200x760", width: 1200, height: 760 },
  { name: "1280x800", width: 1280, height: 800 },
  { name: "1366x768", width: 1366, height: 768 },
  { name: "1440x900", width: 1440, height: 900 },
  { name: "1920x1080", width: 1920, height: 1080 },
] as const;

async function prepareHistory(page: Page) {
  // 应用首页是总览页；布局诊断针对历史页两栏结构，需要先切换导航。
  await page.getByTestId("navigation-history").click();
  await expect(page.getByTestId("history-session-list")).toBeVisible();
}

test("布局诊断：主要工作区矩形不能真实重叠", async ({ page }) => {
  await installMockBridge(page);

  for (const viewport of viewports) {
    await page.setViewportSize({ width: viewport.width, height: viewport.height });
    await page.goto("/");
    await prepareHistory(page);

    const snapshot = await page.evaluate(() => {
      // 诊断前复位内部滚动根，避免按钮聚焦后的自动滚动污染边界测量。
      const main = document.querySelector<HTMLElement>(".app-main");
      const projList = document.querySelector<HTMLElement>(".proj-list");
      if (main) {
        main.scrollTop = 0;
        main.scrollLeft = 0;
      }
      if (projList) projList.scrollTop = 0;

      const selectors = {
        titleBar: ".title-bar",
        navigation: ".navigation-rail",
        main: ".app-main",
        historyPage: ".history-page",
        projPanel: ".proj-panel",
        sessPanel: ".sess-panel",
      } as const;
      const rects = Object.fromEntries(
        Object.entries(selectors).map(([name, selector]) => {
          const element = document.querySelector<HTMLElement>(selector);
          const rect = element?.getBoundingClientRect();
          return [
            name,
            rect
              ? {
                  left: Math.round(rect.left),
                  top: Math.round(rect.top),
                  right: Math.round(rect.right),
                  bottom: Math.round(rect.bottom),
                  width: Math.round(rect.width),
                  height: Math.round(rect.height),
                }
              : null,
          ];
        }),
      );
      const overlap = (a: keyof typeof rects, b: keyof typeof rects) => {
        const first = rects[a];
        const second = rects[b];
        if (!first || !second) return false;
        return (
          first.left < second.right &&
          first.right > second.left &&
          first.top < second.bottom &&
          first.bottom > second.top
        );
      };
      // 项目行必须完整落在左栏（两栏各自内部滚动，行不允许溢出面板）。
      const projPanel = document.querySelector<HTMLElement>(".proj-panel");
      const projRect = projPanel?.getBoundingClientRect();
      const projItems = projPanel
        ? Array.from(projPanel.querySelectorAll<HTMLElement>(".proj-item")).map((item) => {
            const rect = item.getBoundingClientRect();
            return {
              top: Math.round(rect.top),
              bottom: Math.round(rect.bottom),
              fullyInsidePanel: Boolean(
                projRect && rect.top >= projRect.top - 1 && rect.bottom <= projRect.bottom + 1,
              ),
            };
          })
        : [];
      const titleBar = document.querySelector<HTMLElement>(".title-bar");
      const titleBarRect = titleBar?.getBoundingClientRect();

      return {
        rects,
        overlaps: {
          titleMain: overlap("titleBar", "main"),
          navMain: overlap("navigation", "main"),
          projSess: overlap("projPanel", "sessPanel"),
        },
        projList: {
          overflowY: projList ? getComputedStyle(projList).overflowY : null,
          clientHeight: projList?.clientHeight ?? 0,
          scrollHeight: projList?.scrollHeight ?? 0,
          items: projItems,
        },
        titleBar: {
          height: Math.round(titleBarRect?.height ?? 0),
          scrollWidth: titleBar?.scrollWidth ?? 0,
          clientWidth: titleBar?.clientWidth ?? 0,
          right: Math.round(titleBarRect?.right ?? 0),
          viewportWidth: window.innerWidth,
        },
        scroll: {
          body: document.body.scrollTop,
          document: document.documentElement.scrollTop,
          main: main?.scrollTop ?? 0,
        },
      };
    });

    console.log(`[layout ${viewport.name}] ${JSON.stringify(snapshot)}`);
    expect(snapshot.overlaps.titleMain).toBe(false);
    expect(snapshot.overlaps.navMain).toBe(false);
    expect(snapshot.overlaps.projSess).toBe(false);
    expect(snapshot.projList.items.length).toBeGreaterThan(0);
    expect(snapshot.projList.items.every((item) => item.fullyInsidePanel)).toBe(true);
    expect(snapshot.scroll.body).toBe(0);
    expect(snapshot.scroll.document).toBe(0);
    if (viewport.width >= 1200) {
      // 宽桌面下标题栏必须单行容纳品牌、当前账号与状态徽章，不产生内部横向滚动。
      expect(snapshot.titleBar.height).toBeLessThanOrEqual(88);
      expect(snapshot.titleBar.scrollWidth).toBeLessThanOrEqual(snapshot.titleBar.clientWidth + 1);
      expect(snapshot.titleBar.right).toBeLessThanOrEqual(snapshot.titleBar.viewportWidth + 1);
    }
  }
});

test("紧凑桌面项目列表滚动到底部时末项完整可见", async ({ page }) => {
  await installMockBridge(page);
  await page.setViewportSize({ width: 1024, height: 600 });
  await page.goto("/");
  await prepareHistory(page);

  const metrics = await page.evaluate(() => {
    const list = document.querySelector<HTMLElement>(".proj-list");
    if (!list) return null;

    // 用长列表模拟真实主库中的大量项目，验证内部滚动不会裁掉末项。
    for (let index = 0; index < 24; index += 1) {
      const item = document.createElement("button");
      item.type = "button";
      item.className = "proj-item";
      item.innerHTML = `<span class="proj-item__main"><span class="proj-item__name">末项验证项目 ${index + 1}</span></span>`;
      list.appendChild(item);
    }

    list.scrollTop = list.scrollHeight;
    const listRect = list.getBoundingClientRect();
    const items = Array.from(list.querySelectorAll<HTMLElement>(".proj-item"));
    const last = items.at(-1)?.getBoundingClientRect();
    return {
      overflowY: getComputedStyle(list).overflowY,
      clientHeight: list.clientHeight,
      scrollHeight: list.scrollHeight,
      lastInside: Boolean(last && last.bottom <= listRect.bottom + 1 && last.top >= listRect.top - 1),
    };
  });

  expect(metrics).not.toBeNull();
  expect(["auto", "scroll"]).toContain(metrics!.overflowY);
  expect(metrics!.scrollHeight).toBeGreaterThan(metrics!.clientHeight);
  expect(metrics!.lastInside).toBe(true);
});
