/**
 * U-2/U-3 视觉验收截图脚本（一次性工具，非测试）。
 *
 * 用 mock invoke 边界（同 e2e/mock-bridge 约束：不启动 Tauri、不访问真实数据）
 * 驱动构建产物，对账号页（列表/卡片）、签到页、总览页、历史页截图，
 * 供与 DESIGN_TOKENS 样张（方案 02 亮白通用玻璃）对照验收。
 *
 * 运行前置：pnpm build && pnpm preview --port 4173
 * 运行方式：node e2e/visual-shots.mjs
 */
import { chromium } from "@playwright/test";
import { mkdirSync } from "node:fs";

const BASE = "http://127.0.0.1:4173";
const OUT = "artifacts/ui-shots";
mkdirSync(OUT, { recursive: true });

// 合成账号总览构造器（仅注释参考；mock 数据已内联进 mockInvoke 保证自包含）。
// 覆盖两槽位徽章全部形态：已签/未签 × 运行中·已登录/运行中·待登录(琥珀)/
// 登录有效/未启动/待登录(琥珀)/登录失效(琥珀加强)/令牌临期(5天,meta 琥珀)。
function buildOverview() { return []; }
void buildOverview;

async function mockInvoke(command) {
  const NOW_LOCAL = Math.floor(Date.now() / 1000);
  const DAY_LOCAL = 86400;
  const overview = [
    { profile_id: "p1", screen_name: "梦梦", account_id: "a-p1", created_at: "2026-08-01T00:00:00Z", last_verified_at: "2026-08-26T00:00:00Z",
      credits: 200, credits_cached_at: "2026-08-26T01:00:00Z", usage_remaining_credits: 1600, usage_cached_at: "2026-08-26T01:00:00Z",
      checked_in: true, access_token_expires_at_unix_seconds: NOW_LOCAL + 13 * DAY_LOCAL, refresh_token_expires_at_unix_seconds: NOW_LOCAL + 179 * DAY_LOCAL,
      device_tail: "9012", device_id: "3569646294624771", display_name: null, masked_mobile: "156******19", auto_checkin_enabled: true },
    { profile_id: "p2", screen_name: "LY", account_id: "a-p2", created_at: "2026-08-02T00:00:00Z", last_verified_at: "2026-08-26T00:00:00Z",
      credits: 200, credits_cached_at: "2026-08-26T01:00:00Z", usage_remaining_credits: 1400, usage_cached_at: "2026-08-26T01:00:00Z",
      checked_in: false, access_token_expires_at_unix_seconds: NOW_LOCAL + 8 * DAY_LOCAL, refresh_token_expires_at_unix_seconds: NOW_LOCAL + 170 * DAY_LOCAL,
      device_tail: "7701", device_id: "2229135200000002", display_name: null, masked_mobile: "158******27", auto_checkin_enabled: true },
    { profile_id: "p3", screen_name: "用户92431183708", account_id: "a-p3", created_at: "2026-08-03T00:00:00Z", last_verified_at: "2026-08-26T00:00:00Z",
      credits: 200, credits_cached_at: "2026-08-26T01:00:00Z", usage_remaining_credits: 1720.9, usage_cached_at: "2026-08-26T01:00:00Z",
      checked_in: true, access_token_expires_at_unix_seconds: NOW_LOCAL + 13 * DAY_LOCAL, refresh_token_expires_at_unix_seconds: NOW_LOCAL + 179 * DAY_LOCAL,
      device_tail: "6509", device_id: "1928709300000003", display_name: "主力号", masked_mobile: "133******04", auto_checkin_enabled: true },
    { profile_id: "p4", screen_name: "用户4050081350", account_id: "a-p4", created_at: "2026-08-04T00:00:00Z", last_verified_at: "2026-08-26T00:00:00Z",
      credits: 200, credits_cached_at: "2026-08-26T01:00:00Z", usage_remaining_credits: 623.9, usage_cached_at: "2026-08-26T01:00:00Z",
      checked_in: false, access_token_expires_at_unix_seconds: NOW_LOCAL + 13 * DAY_LOCAL, refresh_token_expires_at_unix_seconds: NOW_LOCAL + 179 * DAY_LOCAL,
      device_tail: "0030", device_id: "2379904400000004", display_name: null, masked_mobile: "156******86", auto_checkin_enabled: true },
    { profile_id: "p5", screen_name: "17513301392", account_id: "a-p5", created_at: "2026-08-05T00:00:00Z", last_verified_at: "2026-08-26T00:00:00Z",
      credits: 200, credits_cached_at: "2026-08-26T01:00:00Z", usage_remaining_credits: 400, usage_cached_at: "2026-08-26T01:00:00Z",
      checked_in: false, access_token_expires_at_unix_seconds: NOW_LOCAL + 5 * DAY_LOCAL, refresh_token_expires_at_unix_seconds: NOW_LOCAL + 175 * DAY_LOCAL,
      device_tail: "4738", device_id: "2611766200000005", display_name: null, masked_mobile: "175******92", auto_checkin_enabled: false },
  ];
  const instanceStates = [
    { profile_id: "p1", running: true, login_state: "logged_in" },
    { profile_id: "p2", running: true, login_state: "logged_out" },   // 琥珀：运行中·待登录
    { profile_id: "p3", running: false, login_state: "logged_in" },   // 停止态：登录有效
    { profile_id: "p4", running: false, login_state: "uninitialized" },
    { profile_id: "p5", running: false, login_state: "stale" },       // 琥珀加强：登录失效
  ];
  switch (command) {
    case "get_workspace_state": return {
      platform: { platform_id: "work_cn", display_name: "TRAE Work CN", adapter_implemented: false },
      data_location: { selected: true, display_name: "TRAE Work CN", unavailable_reason: null },
      current_account: { detected: false, user_fingerprint: null, unavailable_reason: "not_detected" },
      history: { account_count: 2, project_count: 6, session_count: 74 },
      capabilities: { scan_enabled: true, sync_enabled: false, backup_enabled: false, restore_enabled: false },
      honest_status: "真实能力已启用",
    };
    case "get_checkin_capability":
      return { enabled: true, transport: "real", real_http_enabled: true, message: "真实签到已启用：仅对已通过登录的账号直连 TRAE。" };
    case "get_checkin_overview": return overview;
    case "get_trae_instance_states": return instanceStates;
    case "get_auto_checkin_settings":
      return { enabled: true, daily_time_hhmm: "10:00", ledger: { date: "2026-08-26", running: false, total: 5, completed: 4, failed: 0, skipped: 1 } };
    case "get_managed_account_state": return { saved_accounts: [], switch_state: "idle" };
    case "list_operations": return [];
    default: return null;
  }
}

const browser = await chromium.launch();
const page = await browser.newPage({ viewport: { width: 1280, height: 800 } });
page.on("pageerror", (err) => console.log("[pageerror]", String(err).slice(0, 600)));
page.on("console", (msg) => {
  if (msg.type() === "error") console.log("[console.error]", msg.text().slice(0, 400));
});
await page.addInitScript(`
  window.__TAURI_INTERNALS__ = {
    invoke: (cmd, args) => window.__mockInvoke(cmd, args),
    metadata: { currentWindow: { label: "main" }, currentWebview: { label: "main" } },
  };
  window.__mockInvokeMap = [];
`);
// 注入 mock 实现（每次导航前重新注入，addInitScript 在 document 创建早期执行）。
// 注意：eval 包裹括号使其成为表达式并返回函数本体（直接 eval 函数声明返回 undefined）。
await page.addInitScript((impl) => {
  window.__mockInvoke = eval(`(${impl})`);
}, mockInvoke.toString());

await page.goto(BASE, { waitUntil: "networkidle" });

// —— 总览页（默认页）——
await page.waitForTimeout(600);
console.log("[body@overview]", await page.evaluate(() => document.body.innerText.slice(0, 400)));
await page.screenshot({ path: `${OUT}/01-overview.png`, fullPage: false });

// —— 账号页：列表视图（默认）——
await page.click('[data-testid="navigation-accounts"]');
await page.waitForTimeout(700);
await page.screenshot({ path: `${OUT}/02-accounts-list.png` });

// —— 账号页：卡片视图 ——
await page.click('[data-testid="account-view-card"]');
await page.waitForTimeout(500);
await page.screenshot({ path: `${OUT}/03-accounts-cards.png` });
// 切回列表（保持默认偏好纯净）。
await page.click('[data-testid="account-view-list"]');
await page.waitForTimeout(300);

// —— 账号详情（含备注名编辑 + 危险分区）——
await page.click('[data-testid="account-card-p3"]');
await page.waitForTimeout(500);
await page.screenshot({ path: `${OUT}/04-account-detail.png`, fullPage: true });

// —— 签到页（四动作 + 单列表演进式）——
await page.click('[data-testid="navigation-checkin"]');
await page.waitForTimeout(600);
await page.screenshot({ path: `${OUT}/05-checkin.png` });

// —— 历史页（U-4 B1 玻璃基底验收）：初始授权态即可，目标是玻璃/主题可见非内容渲染 ——
await page.click('[data-testid="navigation-history"]');
await page.waitForTimeout(800);
await page.screenshot({ path: `${OUT}/06-history.png`, fullPage: false });

await browser.close();
console.log(`screenshots -> ${OUT}`);
