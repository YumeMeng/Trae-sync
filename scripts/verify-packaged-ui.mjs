import { chromium } from "@playwright/test";
import { mkdir, writeFile, stat } from "node:fs/promises";
import path from "node:path";

// 安装包使用 WebView2 合成表面；CDP 截图和 DOM 断言比 GDI CopyFromScreen 可靠。
const args = new Map();
for (let index = 2; index < process.argv.length; index += 1) {
  const value = process.argv[index];
  if (!value.startsWith("--")) continue;
  const [key, inline] = value.slice(2).split("=", 2);
  args.set(key, inline ?? process.argv[++index]);
}

const port = Number(args.get("cdp-port") ?? 9358);
const evidenceRoot = path.resolve(String(args.get("evidence-root") ?? ".scratch/packaged-ui"));
await mkdir(evidenceRoot, { recursive: true });

const browser = await chromium.connectOverCDP(`http://127.0.0.1:${port}`);
try {
  const pages = browser.contexts().flatMap((context) => context.pages());
  const page = pages.find((candidate) => candidate.url().startsWith("http://tauri.localhost")) ?? pages[0];
  if (!page) throw new Error("WebView2 没有可检查的页面");

  await page.waitForLoadState("domcontentloaded");
  await page.locator("[data-testid=title-bar-mode]").waitFor({ state: "visible", timeout: 15_000 });

  const screenshotPath = path.join(evidenceRoot, "packaged-window-cdp.png");
  await page.screenshot({ path: screenshotPath, fullPage: false });
  const screenshotSize = (await stat(screenshotPath)).size;

  const probe = await page.evaluate(() => {
    const rectOf = (selector) => {
      const element = document.querySelector(selector);
      if (!element) return null;
      const rect = element.getBoundingClientRect();
      return {
        left: Math.round(rect.left),
        top: Math.round(rect.top),
        right: Math.round(rect.right),
        bottom: Math.round(rect.bottom),
        width: Math.round(rect.width),
        height: Math.round(rect.height),
      };
    };
    const rects = {
      titleBar: rectOf(".title-bar"),
      navigation: rectOf(".navigation-rail"),
      main: rectOf(".app-main"),
      workbench: rectOf(".workbench"),
      titleContext: rectOf(".title-bar__context"),
    };
    const overlaps = (first, second) => {
      if (!first || !second) return false;
      return (
        first.left < second.right &&
        first.right > second.left &&
        first.top < second.bottom &&
        first.bottom > second.top
      );
    };
    const visible = (selector) =>
      [...document.querySelectorAll(selector)].filter((element) => {
        const rect = element.getBoundingClientRect();
        const style = getComputedStyle(element);
        return style.display !== "none" && style.visibility !== "hidden" && rect.width > 0 && rect.height > 0;
      }).length;
    const resourceNames = performance.getEntriesByType("resource").map((entry) => entry.name);
    const bodyText = document.body.innerText;
    const authorize = document.querySelector("[data-testid=authorize-check]");
    const scanButtons = [...document.querySelectorAll("[data-testid=scan-history-button]")];
    const syncButtons = [...document.querySelectorAll("[data-testid=sync-button]")];
    return {
      href: location.href,
      readyState: document.readyState,
      bodyText,
      rootChildren: document.getElementById("root")?.children.length ?? 0,
      rects,
      overlaps: {
        titleMain: overlaps(rects.titleBar, rects.main),
        navigationMain: overlaps(rects.navigation, rects.main),
        contextMain: overlaps(rects.titleContext, rects.main),
      },
      viewport: { width: innerWidth, height: innerHeight, devicePixelRatio },
      document: {
        scrollWidth: document.documentElement.scrollWidth,
        clientWidth: document.documentElement.clientWidth,
        scrollHeight: document.documentElement.scrollHeight,
        clientHeight: document.documentElement.clientHeight,
      },
      initialAuthorization: {
        modeText: document.querySelector("[data-testid=title-bar-mode]")?.textContent?.trim() ?? "",
        locationText: document.querySelector("[data-testid=data-location-context]")?.textContent?.trim() ?? "",
        authorizeControlPresent: Boolean(authorize),
        authorized: authorize instanceof HTMLInputElement ? authorize.checked : null,
        scanButtonCount: scanButtons.length,
        scanButtonDisabled: scanButtons.length === 0 || scanButtons.every((button) => button.disabled),
        syncButtonCount: syncButtons.length,
      },
      visibleNamedElements: [...document.querySelectorAll("button, input, [role]")]
        .filter((element) => {
          const rect = element.getBoundingClientRect();
          return rect.width > 0 && rect.height > 0;
        })
        .map((element) => element.getAttribute("aria-label") || element.textContent?.trim() || element.tagName)
        .filter(Boolean)
        .slice(0, 80),
      resourceNames,
      forbiddenResourceHits: resourceNames.filter((name) => /file:\/\/|database\.db|\.db-wal|\.db-shm/i.test(name)),
    };
  });

  const failures = [];
  if (probe.href !== "http://tauri.localhost/") failures.push(`unexpected page URL: ${probe.href}`);
  if (probe.readyState !== "complete") failures.push(`document not complete: ${probe.readyState}`);
  if (probe.rootChildren < 1 || probe.bodyText.length < 80) failures.push("首屏 DOM 或可见文本为空");
  if (probe.overlaps.titleMain || probe.overlaps.navigationMain || probe.overlaps.contextMain) {
    failures.push("应用壳层矩形发生重叠");
  }
  if (probe.document.scrollWidth > probe.document.clientWidth + 1) failures.push("首屏出现水平溢出");
  if (!probe.initialAuthorization.modeText.includes("读取未授权")) failures.push("初始授权状态不是读取未授权");
  if (probe.initialAuthorization.authorized !== false) failures.push("隔离 Profile 初始授权控件未保持未勾选");
  if (!probe.initialAuthorization.scanButtonDisabled) failures.push("未授权时扫描入口没有保持禁用/不可见");
  if (probe.initialAuthorization.syncButtonCount !== 0) failures.push("未授权首屏出现同步入口");
  if (probe.forbiddenResourceHits.length > 0) failures.push(`首屏资源疑似读取真实数据库: ${probe.forbiddenResourceHits.join(", ")}`);
  if (screenshotSize < 20_000) failures.push(`CDP 截图过小，疑似空白: ${screenshotSize} bytes`);

  const report = {
    verifier: "verify-packaged-ui-cdp",
    cdp_port: port,
    screenshot: screenshotPath,
    screenshot_bytes: screenshotSize,
    visual_probe: "WebView2 CDP screenshot + DOM/layout assertions",
    probe,
    failures,
    verdict: failures.length === 0 ? "PASS" : "FAIL",
  };
  await writeFile(path.join(evidenceRoot, "packaged-ui-cdp-report.json"), JSON.stringify(report, null, 2), "utf8");
  console.log(JSON.stringify(report, null, 2));
  if (failures.length > 0) process.exitCode = 1;
} finally {
  await browser.close();
}
