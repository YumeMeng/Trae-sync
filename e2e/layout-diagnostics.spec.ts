import { test, expect, type Page } from "@playwright/test";
import { installMockBridge } from "./mock-bridge";

// 布局诊断（主库详情页库对话面板，G21 两栏式）：主要工作区矩形不能真实重叠，
// 紧凑桌面左栏项目树滚动到底部时末项完整可见。
// 独立历史页实例常驻 DOM 但停留在加载态（不渲染 .lib-panel），选择器天然唯一。

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

async function prepareMasterDetail(page: Page) {
  // 应用首页是总览页；布局诊断针对主库详情页两栏结构，需经环境页进入。
  await page.getByTestId("navigation-environment").click();
  await page.getByTestId("env-master-detail").click();
  await expect(page.getByRole("region", { name: "主库详情" })).toBeVisible();
  // 展开项目让会话行进入树（行完整落栏诊断覆盖项目行与会话行）。
  await expect(page.getByTestId("library-tree")).toBeVisible();
  await page.getByTestId("library-project-p1").click();
}

test("布局诊断：主要工作区矩形不能真实重叠", async ({ page }) => {
  await installMockBridge(page);

  for (const viewport of viewports) {
    await page.setViewportSize({ width: viewport.width, height: viewport.height });
    await page.goto("/");
    await prepareMasterDetail(page);

    const snapshot = await page.evaluate(() => {
      // 诊断前复位内部滚动根，避免按钮聚焦后的自动滚动污染边界测量。
      const main = document.querySelector<HTMLElement>(".app-main");
      const tree = document.querySelector<HTMLElement>(".lib-tree");
      if (main) {
        main.scrollTop = 0;
        main.scrollLeft = 0;
      }
      if (tree) tree.scrollTop = 0;

      const selectors = {
        titleBar: ".title-bar",
        navigation: ".navigation-rail",
        main: ".app-main",
        masterDetail: ".master-detail",
        libPanel: ".lib-panel",
        viewerPanel: ".viewer-panel",
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
      // 树内行（项目行与会话行）必须完整落在左栏（两栏各自内部滚动，行不允许溢出面板）。
      const libPanel = document.querySelector<HTMLElement>(".lib-panel");
      const libRect = libPanel?.getBoundingClientRect();
      const treeRows = libPanel
        ? Array.from(libPanel.querySelectorAll<HTMLElement>(".lib-project, .lib-session")).map(
            (item) => {
              const rect = item.getBoundingClientRect();
              return {
                top: Math.round(rect.top),
                bottom: Math.round(rect.bottom),
                fullyInsidePanel: Boolean(
                  libRect && rect.top >= libRect.top - 1 && rect.bottom <= libRect.bottom + 1,
                ),
              };
            },
          )
        : [];
      const titleBar = document.querySelector<HTMLElement>(".title-bar");
      const titleBarRect = titleBar?.getBoundingClientRect();

      return {
        rects,
        overlaps: {
          titleMain: overlap("titleBar", "main"),
          navMain: overlap("navigation", "main"),
          libViewer: overlap("libPanel", "viewerPanel"),
        },
        libTree: {
          overflowY: tree ? getComputedStyle(tree).overflowY : null,
          clientHeight: tree?.clientHeight ?? 0,
          scrollHeight: tree?.scrollHeight ?? 0,
          rows: treeRows,
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
    expect(snapshot.overlaps.libViewer).toBe(false);
    expect(snapshot.libTree.rows.length).toBeGreaterThan(0);
    expect(snapshot.libTree.rows.every((row) => row.fullyInsidePanel)).toBe(true);
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

test("紧凑桌面项目树滚动到底部时末项完整可见", async ({ page }) => {
  await installMockBridge(page);
  await page.setViewportSize({ width: 1024, height: 600 });
  await page.goto("/");
  await prepareMasterDetail(page);

  const metrics = await page.evaluate(() => {
    const tree = document.querySelector<HTMLElement>(".lib-tree");
    if (!tree) return null;

    // 用与真实树分支相同的 class 注入长列表，验证内部滚动不会裁掉末项。
    for (let index = 0; index < 24; index += 1) {
      const branch = document.createElement("div");
      branch.className = "lib-branch";
      branch.innerHTML = `<div class="lib-project"><span class="lib-project__name">末项验证项目 ${index + 1}</span></div>`;
      tree.appendChild(branch);
    }

    tree.scrollTop = tree.scrollHeight;
    const treeRect = tree.getBoundingClientRect();
    const rows = Array.from(tree.querySelectorAll<HTMLElement>(".lib-project"));
    const last = rows.at(-1)?.getBoundingClientRect();
    return {
      overflowY: getComputedStyle(tree).overflowY,
      clientHeight: tree.clientHeight,
      scrollHeight: tree.scrollHeight,
      lastInside: Boolean(last && last.bottom <= treeRect.bottom + 1 && last.top >= treeRect.top - 1),
    };
  });

  expect(metrics).not.toBeNull();
  expect(["auto", "scroll"]).toContain(metrics!.overflowY);
  expect(metrics!.scrollHeight).toBeGreaterThan(metrics!.clientHeight);
  expect(metrics!.lastInside).toBe(true);
});
