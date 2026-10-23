// ============================================================================
// 消息中继腿 / 查看器纯派生契约（P4e-3c）：守旧 LibrarySessionsPanel.tsx 的腿区间、成环 guard、
// 消息归属、role 映射、pane 四态与时间文案四档。纯函数直喂 DTO + 注入 now：零 mock、零真实时钟。
// ============================================================================
import { describe, expect, it } from "vitest";
import {
  buildMessageViews, buildRelayLegSpans, buildViewerPane, formatRelativeTime, formatSessionTime, relayChainsOf,
  type RelayLegInput, type ViewerPaneInput, type ViewerPaneSource,
} from "../src/features/library/relayLegs";
import type { SessionMessageDto } from "../src/types/account_switch";
import type { MasterSessionEntryDto, RelayLedgerEntryDto } from "../src/types/history";

const NOW = new Date(2026, 9, 24, 12, 0, 0); // 本地时间，避免时区相关的日界断言
const sec = (date: Date) => Math.floor(date.getTime() / 1000);
const T1 = 1_700_000_000, T2 = T1 + 3600;
const session = (overrides: Partial<MasterSessionEntryDto> = {}): MasterSessionEntryDto => ({
  session_id: "s2", project_id: "p1", title: "会话", message_count: 40, updated_at_unix_seconds: null,
  deleted: false, hidden_status: null, work_mode: "work", ...overrides });
const entry = (overrides: Partial<RelayLedgerEntryDto> = {}): RelayLedgerEntryDto => ({
  session_id: "s1", from_session_id: null, project_id: "p1", from_user_id: "uA", from_account_name: "甲",
  to_user_id: "uB", to_account_name: "乙", message_count_at_switch: 0, switched_at_unix_seconds: T1, ...overrides });
const message = (message_id: string, role: string, created_at_unix_seconds: number | null, overrides: Partial<SessionMessageDto["content"]> = {}): SessionMessageDto => ({
  message_id, role, message_type: "chat", created_at_unix_seconds,
  content: { kind: "text", text: message_id, step_count: 0, thoughts: [], ...overrides } });
const legInput = (overrides: Partial<RelayLegInput> = {}): RelayLegInput => ({ session: session(), ledger: [], currentUserId: "uC", now: NOW, ...overrides });
const source = (overrides: Partial<ViewerPaneSource> = {}): ViewerPaneSource => ({
  status: "ready", messages: [], hasMoreBefore: false, messageOffset: 0, loadingOlder: false, ...overrides });
const paneInput = (overrides: Partial<ViewerPaneInput> = {}): ViewerPaneInput => ({ ...legInput(), source: source(), activeTab: "messages", ...overrides });

// 两跳台账 → 三腿：甲(最早→T1, 10 条) → 乙(T1→T2, 15 条) → 丙(T2→至今, 40-25=15 条)
const HOP1 = entry({ session_id: "s1", from_session_id: null, message_count_at_switch: 10, switched_at_unix_seconds: T1 });
const HOP2 = entry({ session_id: "s2", from_session_id: "s1", from_user_id: "uB", from_account_name: "乙",
  to_user_id: "uC", to_account_name: "丙", message_count_at_switch: 25, switched_at_unix_seconds: T2 });
const THREE_LEGS = buildRelayLegSpans(legInput({ ledger: [HOP1, HOP2] }));

describe("buildRelayLegSpans（腿区间）", () => {
  it("两跳台账派三腿：首腿最早、中间取相邻差值、当前腿至今并按会话总数补齐", () => {
    expect(THREE_LEGS).toEqual([
      { userId: "uA", accountName: "甲", fromUnixSeconds: null, toUnixSeconds: T1, messages: 10 },
      { userId: "uB", accountName: "乙", fromUnixSeconds: T1, toUnixSeconds: T2, messages: 15 },
      { userId: "uC", accountName: "丙", fromUnixSeconds: T2, toUnixSeconds: null, messages: 15 },
    ]);
    // 差值/补齐为负一律归零（L674 / L684 Math.max(..., 0)）
    const clamp = buildRelayLegSpans(legInput({ session: session({ message_count: 20 }),
      ledger: [HOP1, entry({ session_id: "s2", from_session_id: "s1", message_count_at_switch: 8, switched_at_unix_seconds: T2 })] }));
    expect(clamp.map((span) => span.messages)).toEqual([10, 0, 12]);
  });

  it("无台账回退：单一腿两端 null、消息数=会话总数；账号名先查台账再回退「当前账号」", () => {
    const known = buildRelayLegSpans(legInput({ ledger: [entry({ session_id: "other", to_user_id: "uC", to_account_name: "丙" })], currentUserId: "uC" }));
    expect(known).toEqual([{ userId: "uC", accountName: "丙", fromUnixSeconds: null, toUnixSeconds: null, messages: 40 }]);
    // 台账里查不到 + 无当前账号：userId 空串、名字取「当前账号」
    expect(buildRelayLegSpans(legInput({ currentUserId: null }))).toEqual([
      { userId: "", accountName: "当前账号", fromUnixSeconds: null, toUnixSeconds: null, messages: 40 }]);
  });

  it("成环 guard：最多回溯台账条数 + 1 跳，不死循环（两跳互指 → 3 跳 / 4 腿）", () => {
    // s1 ← s2 ← s1 互指：链回在 guard 用尽时停在 3 跳（= ledger.length + 1），不无限循环
    const a = entry({ session_id: "s1", from_session_id: "s2", from_user_id: "uA", from_account_name: "甲",
      to_user_id: "uB", to_account_name: "乙", message_count_at_switch: 5, switched_at_unix_seconds: T1 });
    const b = entry({ session_id: "s2", from_session_id: "s1", from_user_id: "uB", from_account_name: "乙",
      to_user_id: "uA", to_account_name: "甲", message_count_at_switch: 9, switched_at_unix_seconds: T2 });
    expect(relayChainsOf([a, b]).get("s2")).toHaveLength(3); // 3 跳上限
    const spans = buildRelayLegSpans(legInput({ ledger: [a, b], currentUserId: "uA" }));
    expect(spans.map((span) => span.userId)).toEqual(["uB", "uA", "uB", "uA"]); // 首腿 uB → 交替 → 当前腿 uA
    expect(spans).toHaveLength(4); // 3 跳 → 4 腿（含当前腿）
    expect(spans.map((span) => span.messages)).toEqual([9, 0, 4, 31]); // 负差值归零，当前腿=40-9
  });
});

describe("buildMessageViews（消息行与归属）", () => {
  it("用户消息归属腿：created_at 落 [from, to) 左闭右开；无时间戳归首腿、区间外归末腿", () => {
    const views = buildMessageViews([
      message("before", "user", T1 - 1), message("atT1", "user", T1), message("atT2", "user", T2),
      message("after", "user", T2 + 10), message("noTime", "user", null),
    ], THREE_LEGS);
    expect(views.map((view) => view.accountName)).toEqual(["甲", "乙", "丙", "丙", "甲"]);
    expect(views.map((view) => view.colorIndex)).toEqual([0, 1, 2, 2, 0]); // 色板按腿序首次出现：甲0 乙1 丙2
  });

  it("role 映射：user 带账号名，非 user 一律助手侧中性，task_trace 收成轨迹摘要行", () => {
    const views = buildMessageViews([
      message("u1", "user", T1), message("a1", "assistant", T1), message("sys", "system", T1),
      message("t1", "assistant", T1, { kind: "task_trace", text: "", step_count: 3 }),
    ], THREE_LEGS);
    expect(views.map((view) => view.role)).toEqual(["user", "assistant", "assistant", "trace"]);
    expect(views.map((view) => view.accountName)).toEqual(["乙", null, null, null]);
    expect(views.map((view) => view.colorIndex)).toEqual([1, 0, 0, 0]); // 助手侧中性色 0，不参与账号着色
    expect(views.map((view) => view.stepCount)).toEqual([null, null, null, 3]);
    expect(views[3].text).toBe("任务轨迹 · 3 步"); // L2206 文案表 traceSteps
    expect(views[0].text).toBe("u1"); // text 形态正文直出
    expect(views.map((view) => view.testId)).toEqual(["", "", "", ""]); // 旧锚点归装配层，本层不产出
  });
});

describe("buildViewerPane（查看器 pane）", () => {
  it("四态：loading / error / ready+0 条=empty / ready；tab 与分页字段直传", () => {
    const loading = buildViewerPane(paneInput({ source: source({ status: "loading", messages: [message("m", "user", T1)] }) }));
    const error = buildViewerPane(paneInput({ source: source({ status: "error" }) }));
    const empty = buildViewerPane(paneInput({ source: source({ status: "ready", messages: [] }) }));
    const ready = buildViewerPane(paneInput({ source: source({ messages: [message("m", "user", T1)], hasMoreBefore: true, messageOffset: 2000, loadingOlder: true }), activeTab: "relay" }));
    expect([loading.status, error.status, empty.status, ready.status]).toEqual(["loading", "error", "empty", "ready"]);
    expect(empty.emptyText).toBe("该会话没有可展示的消息。");
    expect(ready).toMatchObject({ activeTab: "relay", ariaLabel: "对话查看器", tabsAriaLabel: "对话内容",
      hasMoreBefore: true, messageOffset: 2000, loadingOlder: true });
    expect(ready.messages.map((view) => view.id)).toEqual(["m"]);
    expect(ready.legs.map((leg) => leg.current)).toEqual([true]); // 无台账 → 单一腿即当前腿
  });
});

describe("时间文案（注入固定 now）", () => {
  it("formatSessionTime 四档：今天 / 昨天 / 同年月日 / 跨年补年份；null → 时间未知", () => {
    expect(formatSessionTime(sec(new Date(2026, 9, 24, 9, 5)), NOW)).toBe("今天 09:05");
    expect(formatSessionTime(sec(new Date(2026, 9, 23, 8, 7)), NOW)).toBe("昨天 08:07");
    expect(formatSessionTime(sec(new Date(2026, 2, 5, 7, 2)), NOW)).toBe("3月5日 07:02");
    expect(formatSessionTime(sec(new Date(2025, 11, 31, 23, 59)), NOW)).toBe("2025年12月31日 23:59");
    expect(formatSessionTime(null, NOW)).toBe("时间未知");
  });

  it("formatRelativeTime 四档：刚刚 / N 分钟前 / N 小时前 / N 天前；≥7 天回落会话时间", () => {
    expect(formatRelativeTime(sec(NOW) - 30, NOW)).toBe("刚刚");
    expect(formatRelativeTime(sec(NOW) - 125, NOW)).toBe("2 分钟前"); // 向下取整，非四舍五入
    expect(formatRelativeTime(sec(NOW) - 7300, NOW)).toBe("2 小时前");
    expect(formatRelativeTime(sec(NOW) - (3 * 86_400 + 60), NOW)).toBe("3 天前");
    expect(formatRelativeTime(sec(NOW) - 8 * 86_400, NOW)).toBe("10月16日 12:00");
    expect(formatRelativeTime(null, NOW)).toBe("时间未知");
  });
});
