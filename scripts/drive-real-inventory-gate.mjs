import { chromium } from "@playwright/test";
import { mkdir, writeFile } from "node:fs/promises";
import path from "node:path";

// 只驱动库存扫描按钮和摘要，不保存会话正文、账号原始 ID 或认证材料。
const args = new Map();
for (let index = 2; index < process.argv.length; index += 1) {
  const value = process.argv[index];
  if (!value.startsWith("--")) continue;
  const [key, inline] = value.slice(2).split("=", 2);
  args.set(key, inline ?? process.argv[++index]);
}

const port = Number(args.get("cdp-port") ?? 9391);
const evidenceRoot = path.resolve(String(args.get("evidence-root") ?? ".scratch/real-inventory-gate"));
const reportPath = path.join(evidenceRoot, "ui-inventory-report.json");
await mkdir(evidenceRoot, { recursive: true });

function summaryFromText(text) {
  const read = (label) => Number(text.match(new RegExp(`${label}\\s+(\\d+)`))?.[1] ?? -1);
  return {
    accounts: read("账号"),
    projects: read("项目"),
    sessions: read("对话"),
  };
}

async function runOnce(page, run) {
  const button = page.getByTestId("scan-inventory-button");
  await button.waitFor({ state: "visible", timeout: 15_000 });
  await button.click();
  await page.getByTestId("scanning-state").waitFor({ state: "visible", timeout: 5_000 }).catch(() => {});

  const failure = page.getByTestId("failure-state");
  const success = page.getByTestId("history-workspace");
  const empty = page.getByTestId("empty-state");
  const deadline = Date.now() + 120_000;
  while (Date.now() < deadline) {
    if (await failure.isVisible().catch(() => false)) {
      const message = (await failure.innerText()).replace(/\s+/g, " ").trim();
      throw new Error(`库存扫描第 ${run} 次失败: ${message}`);
    }
    if (await success.isVisible().catch(() => false) || await empty.isVisible().catch(() => false)) {
      const summaryText = (await page.getByTestId("history-summary").innerText()).replace(/\s+/g, " ").trim();
      const summary = summaryFromText(summaryText);
      if (summary.accounts < 0 || summary.projects < 0 || summary.sessions < 0) {
        throw new Error(`库存扫描第 ${run} 次摘要不可解析`);
      }
      return { run, summary };
    }
    await page.waitForTimeout(250);
  }
  throw new Error(`库存扫描第 ${run} 次超过 120 秒未收口`);
}

const browser = await chromium.connectOverCDP(`http://127.0.0.1:${port}`);
try {
  const pages = browser.contexts().flatMap((context) => context.pages());
  const page = pages.find((candidate) => candidate.url().startsWith("http://tauri.localhost")) ?? pages[0];
  if (!page) throw new Error("WebView2 没有可检查页面");
  await page.waitForLoadState("domcontentloaded");
  const first = await runOnce(page, 1);
  const second = await runOnce(page, 2);
  const stable = JSON.stringify(first.summary) === JSON.stringify(second.summary);
  const report = {
    verifier: "drive-real-inventory-gate",
    cdp_port: port,
    runs: [first, second],
    repeated_scan_stable: stable,
    source_content: "not_captured_by_driver",
    session_content: "not_captured",
    authentication_material: "not_captured",
    verdict: stable ? "PASS" : "FAIL",
  };
  await writeFile(reportPath, JSON.stringify(report, null, 2), "utf8");
  console.log(JSON.stringify(report, null, 2));
  if (!stable) process.exitCode = 1;
} catch (error) {
  const report = {
    verifier: "drive-real-inventory-gate",
    cdp_port: port,
    verdict: "FAIL",
    error: String(error?.message ?? error),
    source_content: "not_captured_by_driver",
    session_content: "not_captured",
    authentication_material: "not_captured",
  };
  await writeFile(reportPath, JSON.stringify(report, null, 2), "utf8");
  throw error;
} finally {
  await browser.close();
}
