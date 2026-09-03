/**
 * U-2/U-3 视觉契约 DOM 断言（一次性工具）。
 * 验证 DESIGN_TOKENS 关键点在真实渲染中的落地：玻璃面板、两槽位徽章、
 * 四动作、危险分区、meta 文字化、历史页玻璃基底（U-4）。与 visual-shots 共用 mock 边界。
 */
import { chromium } from "@playwright/test";

const BASE = "http://127.0.0.1:4173";
const results = [];
const check = (name, ok, extra = "") =>
  results.push(`${ok ? "PASS" : "FAIL"}  ${name}${extra ? `  (${extra})` : ""}`);

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
    { profile_id: "p5", screen_name: "17513301392", account_id: "a-p5", created_at: "2026-08-05T00:00:00Z", last_verified_at: "2026-08-26T00:00:00Z",
      credits: 200, credits_cached_at: "2026-08-26T01:00:00Z", usage_remaining_credits: 400, usage_cached_at: "2026-08-26T01:00:00Z",
      checked_in: false, access_token_expires_at_unix_seconds: NOW_LOCAL + 5 * DAY_LOCAL, refresh_token_expires_at_unix_seconds: NOW_LOCAL + 175 * DAY_LOCAL,
      device_tail: "4738", device_id: "2611766200000005", display_name: null, masked_mobile: "175******92", auto_checkin_enabled: false },
  ];
  const instanceStates = [
    { profile_id: "p1", running: true, login_state: "logged_in" },
    { profile_id: "p2", running: true, login_state: "logged_out" },
    { profile_id: "p5", running: false, login_state: "stale" },
  ];
  switch (command) {
    case "get_workspace_state": return {
      platform: { platform_id: "work_cn", display_name: "TRAE Work CN", adapter_implemented: false },
      data_location: { selected: true, display_name: "TRAE Work CN", unavailable_reason: null },
      current_account: { detected: false, user_fingerprint: null, unavailable_reason: "not_detected" },
      history: { account_count: 2, project_count: 6, session_count: 74 },
      capabilities: { scan_enabled: true, sync_enabled: false, backup_enabled: false, restore_enabled: false },
      honest_status: "ok",
    };
    case "get_checkin_capability": return { enabled: true, transport: "real", real_http_enabled: true, message: "ok" };
    case "get_checkin_overview": return overview;
    case "get_trae_instance_states": return instanceStates;
    case "get_auto_checkin_settings": return { enabled: true, daily_time_hhmm: "10:00", ledger: null };
    case "get_managed_account_state": return { saved_accounts: [], switch_state: "idle" };
    // P5-3 历史页主库视图：ready 两栏数据 + 接力台账 + 消息预览。
    case "get_master_history": return {
      status: "ready",
      current_user_id: "u-b",
      projects: [{ project_id: "p1", name: "项目阿尔法" }],
      sessions: [{
        session_id: "s1", project_id: "p1", title: "会话一", message_count: 8,
        updated_at_unix_seconds: NOW_LOCAL - 60, deleted: false,
      }],
      fingerprint: { db: { mtime_secs: NOW_LOCAL, mtime_nanos: 0, size: 1000 }, wal: null, shm: null },
    };
    case "get_relay_ledger": return [{
      session_id: "s1", from_session_id: null, project_id: "p1",
      from_user_id: "u-a", from_account_name: "账号A",
      to_user_id: "u-b", to_account_name: "账号B",
      message_count_at_switch: 4, switched_at_unix_seconds: NOW_LOCAL - 3600,
    }];
    case "get_master_session_messages": return {
      session_id: "s1", status: "ready",
      messages: [{ message_id: "m1", role: "user", message_type: "general", created_at_unix_seconds: NOW_LOCAL - 60, content: { kind: "text", text: "预览消息", step_count: 0, thoughts: [] } }],
    };
    default: return null;
  }
}

const browser = await chromium.launch();
const page = await browser.newPage({ viewport: { width: 1280, height: 800 } });
await page.addInitScript(`
  window.__TAURI_INTERNALS__ = {
    invoke: (cmd, args) => window.__mockInvoke(cmd, args),
    metadata: { currentWindow: { label: "main" }, currentWebview: { label: "main" } },
  };
`);
await page.addInitScript((impl) => { window.__mockInvoke = eval(`(${impl})`); }, mockInvoke.toString());
await page.goto(BASE, { waitUntil: "networkidle" });
await page.waitForTimeout(800);

// —— 总览页契约 ——
const overviewCheckin = await page.locator('[data-testid="overview-checkin"]').count();
check("总览签到摘要区块（真实模式）", overviewCheckin === 1);
if (overviewCheckin === 1) {
  const statsText = await page.locator('[data-testid="overview-checkin"]').innerText();
  check("四统计卡含今日签到 X/Y", statsText.includes("今日签到") && statsText.includes("1/3"), statsText.replace(/\n/g, "|").slice(0, 80));
}

// —— 全局玻璃契约（T10 加固：blur 与背景透明度双断言，杜绝"有 blur 无透明"假绿灯） ——
// alpha 解析：rgba(...) 取 alpha 值；rgb(...) 或解析失败视为 1（不透明 = 玻璃失效）。
const alphaOf = (bg) => {
  if (!bg.startsWith("rgba")) return 1;
  return parseFloat(/,\s*([\d.]+)\)$/.exec(bg)?.[1] ?? "1");
};
const bars = await page.evaluate(() => {
  const read = (sel) => {
    const el = document.querySelector(sel);
    const s = el ? getComputedStyle(el) : null;
    return s ? { bg: s.backgroundColor, blur: s.backdropFilter } : { bg: "", blur: "" };
  };
  return { titlebar: read(".title-bar"), rail: read(".navigation-rail") };
});
check(
  "标题栏玻璃 blur + 背景透明",
  bars.titlebar.blur.includes("blur") && alphaOf(bars.titlebar.bg) < 1,
  `${bars.titlebar.blur} / ${bars.titlebar.bg}`,
);
check(
  "导航栏玻璃 blur + 背景透明",
  bars.rail.blur.includes("blur") && alphaOf(bars.rail.bg) < 1,
  `${bars.rail.blur} / ${bars.rail.bg}`,
);
// 玻璃面板最小覆盖（T3 全面板玻璃化）：统计可见的 backdrop-filter 元素。
const glassPanels = await page.evaluate(() => {
  let n = 0;
  for (const el of document.querySelectorAll("body *")) {
    if (!(el.offsetWidth > 0 && el.offsetHeight > 0)) continue;
    const s = getComputedStyle(el);
    if (s.backdropFilter !== "none" && s.backdropFilter !== "") n++;
  }
  return n;
});
check("玻璃面板覆盖（总览页可见 ≥ 6 处）", glassPanels >= 6, `${glassPanels} 处`);
// 雾斑 computed 实测（T2）：对角贯穿大尺寸、靛蓝约 20% 径向渐变、36s alternate 漂移。
const mist = await page.evaluate(() => {
  const s = getComputedStyle(document.querySelector(".app-shell"), "::before");
  return {
    width: parseFloat(s.width),
    image: s.backgroundImage,
    filter: s.filter,
    name: s.animationName,
    duration: s.animationDuration,
    iteration: s.animationIterationCount,
    direction: s.animationDirection,
  };
});
check("雾斑大尺寸贯穿画布（≥1100px）", mist.width >= 1100, `${mist.width}px`);
check(
  "雾斑径向渐变（靛蓝 20%）",
  mist.image.includes("radial-gradient") && mist.image.includes("rgba(99, 102, 241, 0.2)"),
  mist.image.slice(0, 60),
);
check("雾斑柔化 filter（blur）", mist.filter.includes("blur"), mist.filter);
check(
  "雾斑漂移动画（mist-drift 36s infinite alternate）",
  mist.name === "mist-drift" && mist.duration === "36s" && mist.iteration === "infinite" && mist.direction === "alternate",
  `${mist.name} ${mist.duration} ${mist.direction}`,
);

// —— 账号页契约 ——
await page.click('[data-testid="navigation-accounts"]');
await page.waitForTimeout(700);
check("默认列表视图（宽行）", (await page.locator(".account-list__row").count()) === 3);
check("分段控件存在", (await page.locator(".seg-control").count()) === 1);
check("排序控件存在", (await page.locator('[data-testid="account-sort"]').count()) === 1);

// 两槽位徽章：p2 运行中·待登录（琥珀）；p5 登录失效（琥珀加强，停止态透出登录子态）。
const p2badges = await page.locator('[data-testid="account-card-p2"] .slot-badge').allInnerTexts();
check("p2 琥珀「运行中 · 待登录」", p2badges.some((t) => t.includes("运行中 · 待登录")), p2badges.join("/"));
const p2warn = await page.locator('[data-testid="account-card-p2"] .slot-badge--warn').count();
check("p2 琥珀类生效", p2warn >= 1);
const p5badges = await page.locator('[data-testid="account-card-p5"] .slot-badge').allInnerTexts();
check("p5 琥珀加强「登录失效」", p5badges.some((t) => t.includes("登录失效")), p5badges.join("/"));
// meta 文字化：手机号 + 令牌天数 + 设备尾号。
const p1meta = await page.locator('[data-testid="account-card-p1"] .account-item__meta').innerText();
check("p1 meta 三段（手机号/令牌/设备）", p1meta.includes("156******19") && p1meta.includes("令牌") && p1meta.includes("9012"), p1meta);
// 令牌临期（p5=5天）meta 琥珀。
const p5metaWarn = await page.locator('[data-testid="account-card-p5"] .account-item__meta-warn').count();
check("p5 令牌临期 meta 琥珀", p5metaWarn === 1);

// —— 卡片视图切换 ——
await page.click('[data-testid="account-view-card"]');
await page.waitForTimeout(400);
check("卡片视图生效", (await page.locator(".account-card--clickable").count()) === 3);
check("视图偏好持久化", (await page.evaluate(() => localStorage.getItem("accounts.view"))) === "card");
await page.click('[data-testid="account-view-list"]');
await page.waitForTimeout(300);

// —— 详情页危险分区 ——
await page.click('[data-testid="account-card-p1"]');
await page.waitForTimeout(500);
const dangerSection = await page.locator(".account-detail__section--danger").count();
check("详情危险操作分区", dangerSection === 1);
const dangerOutsideActions = await page.evaluate(() => {
  const danger = document.querySelector(".account-detail__section--danger");
  const actions = document.querySelector(".account-detail__actions");
  return danger && actions && !actions.contains(danger.querySelector("button"));
});
check("删除按钮不在常规操作区", dangerOutsideActions === true);

// —— 签到页四动作 ——
await page.click('[data-testid="navigation-checkin"]');
await page.waitForTimeout(600);
check("一键全签按钮", (await page.locator('[data-testid="checkin-run-all"]').count()) === 1);
const pendingLabel = await page.locator('[data-testid="checkin-run-pending"]').innerText();
check("一键补签计数（2 未签）", pendingLabel.includes("2"), pendingLabel);
check("签到所选隐藏（默认全选）", (await page.locator('[data-testid="checkin-run-selected"]').count()) === 0);
check("行内单签按钮（未签可点）", (await page.locator('[data-testid="checkin-inline-p2"]:not([disabled])').count()) === 1);
check("行内单签禁用（已签）", (await page.locator('[data-testid="checkin-inline-p1"][disabled]').count()) === 1);
check("单列表演进式列表", (await page.locator(".checkin-flow__row").count()) === 3);

// —— 历史页契约（P5-3 主库视图：两栏玻璃面板 + 接力徽章） ——
await page.click('[data-testid="navigation-history"]');
await page.locator('[data-testid="history-session-list"]').waitFor({ state: "visible", timeout: 10_000 });
check("历史页导航可达（主库会话列表渲染）", true);
const historyPanels = await page.evaluate(() => {
  const read = (sel) => {
    const el = document.querySelector(sel);
    const s = el ? getComputedStyle(el) : null;
    return s ? { bg: s.backgroundColor, blur: s.backdropFilter, radius: s.borderRadius } : { bg: "", blur: "", radius: "" };
  };
  let glass = 0;
  for (const el of document.querySelectorAll("body *")) {
    if (!(el.offsetWidth > 0 && el.offsetHeight > 0)) continue;
    const s = getComputedStyle(el);
    if (s.backdropFilter !== "none" && s.backdropFilter !== "") glass++;
  }
  return { proj: read(".proj-panel"), sess: read(".sess-panel"), glass };
});
check(
  "历史页左栏（项目）玻璃 blur + 背景透明",
  historyPanels.proj.blur.includes("blur") && alphaOf(historyPanels.proj.bg) < 1,
  `${historyPanels.proj.blur} / ${historyPanels.proj.bg}`,
);
check(
  "历史页右栏（会话）玻璃 blur + 背景透明",
  historyPanels.sess.blur.includes("blur") && alphaOf(historyPanels.sess.bg) < 1,
  `${historyPanels.sess.blur} / ${historyPanels.sess.bg}`,
);
check("历史页玻璃面板覆盖（可见 ≥ 4 处）", historyPanels.glass >= 4, `${historyPanels.glass} 处`);
check("历史页两栏圆角对齐", historyPanels.proj.radius === historyPanels.sess.radius, historyPanels.proj.radius);
// 接力徽章：台账条目对应会话行渲染头像链。
check("接力徽章头像链可见", (await page.locator('[data-testid="history-relay-chain"]').count()) >= 1);

await browser.close();
const failed = results.filter((r) => r.startsWith("FAIL"));
console.log(results.join("\n"));
console.log(`\n${results.length - failed.length}/${results.length} passed`);
if (failed.length > 0) process.exit(1);
