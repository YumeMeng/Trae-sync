/**
 * 消息中继腿 / 右栏查看器纯派生（P4e-3b）：旧 LibrarySessionsPanel.tsx（下称 L）的
 * 台账链回（L626-650）→ 接力腿（L653-713）→ 账号色板（L2433-2443）→ 消息归属（L2449-2459）
 * → 时间文案（L2489-2520）→ 消息行（L2166-2216）→ 查看器 pane（L2008-2160）。
 * 零 React / 零 JSX / 零 invoke / 零 Date.now：时钟一律入参 now（Date），腿端点文案用同一 now。
 * 次序 = 台账数组次序 + 链回 unshift（时间正序）；不排序、不自算滞留 / 空项目 / 孤儿。
 * testid 不在此层产出（旧锚点 library-message-<id> L2180/L2190 由装配层给）。
 */
import type { SessionMessageDto } from "../../types/account_switch";
import type { MasterSessionEntryDto, RelayLedgerEntryDto } from "../../types/history";
import { LIBRARY_TEXTS, fillLibraryText, type MessageView, type RelayLegView, type ViewerPaneView } from "./config";
/** 消息流分页大小（L146）：后端按时间倒序取页后反转为升序。 */
export const CHAT_WINDOW_LIMIT = 2000;
/**
 * 腿区间（L659-685 的中间形态）：端点保留 unix 秒，供消息归属判定（L2449-2459）。
 * config 的 RelayLegView 只有已格式化的 spanLabel、无法反推区间；不往其上自造字段。
 */
export interface RelayLegSpan {
  readonly userId: string; readonly accountName: string | null; readonly fromUnixSeconds: number | null;
  readonly toUnixSeconds: number | null; readonly messages: number;
}
/** 查看器读取态（L133-140 去掉 session 的纯数据形态）。 */
export interface ViewerPaneSource {
  readonly status: "loading" | "ready" | "error"; readonly messages: readonly SessionMessageDto[]; readonly hasMoreBefore: boolean;
  readonly messageOffset: number; readonly loadingOlder: boolean;
}
/** 接力腿入参：腿由台账链 + 会话累计消息数派生（消息数组只影响归属，见 buildMessageViews）。 */
export interface RelayLegInput {
  readonly session: MasterSessionEntryDto; readonly ledger: readonly RelayLedgerEntryDto[]; readonly currentUserId: string | null;
  /** 时钟（L2500/L2489 旧实现读 new Date() / Date.now()）；注入以保证纯函数。 */
  readonly now: Date;
}
/** 查看器 pane 入参：读取态 + tab 由页面状态给，腿与消息由本层派生。 */
export interface ViewerPaneInput extends RelayLegInput {
  readonly source: ViewerPaneSource; readonly activeTab: "messages" | "relay";
}
/** 台账按 session_id 索引（L626-630）：同 id 后者覆盖前者，与旧 Map.set 逐字一致。 */
function ledgerBySessionIdOf(ledger: readonly RelayLedgerEntryDto[]): ReadonlyMap<string, RelayLedgerEntryDto> {
  const map = new Map<string, RelayLedgerEntryDto>();
  for (const entry of ledger) map.set(entry.session_id, entry);
  return map;
}
/** 沿 from_session_id 逐跳链回（L633-650）：每跳 unshift → 时间正序；成环最多回溯台账条数 + 1 跳。 */
export function relayChainsOf(ledger: readonly RelayLedgerEntryDto[]): ReadonlyMap<string, readonly RelayLedgerEntryDto[]> {
  const bySessionId = ledgerBySessionIdOf(ledger); // L626-630
  const chains = new Map<string, readonly RelayLedgerEntryDto[]>();
  for (const target of ledger) { // L635
    const hops: RelayLedgerEntryDto[] = [];
    let cursor: RelayLedgerEntryDto | undefined = target; // L637
    let guard = ledger.length + 1; // L639：异常数据成环时最多回溯台账条数 + 1 跳，避免死循环
    while (cursor && guard > 0) { // L640
      hops.unshift(cursor); // L641
      const previousId: string | null = cursor.from_session_id; // L643-644：显式标注窄化推断环
      cursor = previousId ? bySessionId.get(previousId) : undefined; // L644
      guard -= 1; // L645
    }
    chains.set(target.session_id, hops); // L647
  }
  return chains; // L649-650
}
/** 台账反查账号名（L2472-2478）：先 from 后 to，首个命中即返回。 */
function accountNameOfLedger(ledger: readonly RelayLedgerEntryDto[], userId: string): string | null {
  for (const entry of ledger) {
    if (entry.from_user_id === userId) return entry.from_account_name;
    if (entry.to_user_id === userId) return entry.to_account_name;
  }
  return null;
}
/** 账号色板索引（L2433-2443）：按腿顺序首次出现排，蓝 → 绿 → 琥珀（% 3）循环。 */
export function accountColorIndicesOf(legs: readonly { readonly userId: string }[]): ReadonlyMap<string, number> {
  const indices = new Map<string, number>();
  let next = 0;
  for (const leg of legs) {
    if (!indices.has(leg.userId)) { indices.set(leg.userId, next % 3); next += 1; }
  }
  return indices;
}
/** 会话时间（L2500-2513 逐行）：今天/昨天 HH:mm，同月跨日 M月D日，跨年补年份。 */
export function formatSessionTime(unixSeconds: number | null, now: Date): string {
  if (!unixSeconds) return LIBRARY_TEXTS.timeUnknown; // L2501
  const date = new Date(unixSeconds * 1000); // L2502（由显式秒数构造，不读时钟）
  const pad = (value: number) => String(value).padStart(2, "0"); // L2504
  const hhmm = `${pad(date.getHours())}:${pad(date.getMinutes())}`; // L2505
  const startOfDay = (value: Date) => new Date(value.getFullYear(), value.getMonth(), value.getDate()).getTime(); // L2506
  const dayDiff = Math.round((startOfDay(now) - startOfDay(date)) / 86_400_000); // L2507
  if (dayDiff === 0) return fillLibraryText(LIBRARY_TEXTS.todayAt, { time: hhmm }); // L2508
  if (dayDiff === 1) return fillLibraryText(LIBRARY_TEXTS.yesterdayAt, { time: hhmm }); // L2509
  const sameYear = date.getFullYear() === now.getFullYear(); // L2510
  // L2511：月日文案 config 未登记模板键，逐字沿用旧格式（不新造文案）。
  const monthDay = `${date.getMonth() + 1}月${date.getDate()}日`;
  return sameYear ? `${monthDay} ${hhmm}` : `${date.getFullYear()}年${monthDay} ${hhmm}`; // L2512
}
/** 树内会话相对时间（L2489-2497）：刚刚 / N 分钟前 / N 小时前 / N 天前，更早回落会话时间。 */
export function formatRelativeTime(unixSeconds: number | null, now: Date): string {
  if (!unixSeconds) return LIBRARY_TEXTS.timeUnknown; // L2490
  const diffSeconds = now.getTime() / 1000 - unixSeconds; // L2491（旧 Date.now()）
  if (diffSeconds < 60) return LIBRARY_TEXTS.justNow; // L2492
  if (diffSeconds < 3600) return fillLibraryText(LIBRARY_TEXTS.minutesAgo, { count: Math.floor(diffSeconds / 60) }); // L2493
  if (diffSeconds < 86_400) return fillLibraryText(LIBRARY_TEXTS.hoursAgo, { count: Math.floor(diffSeconds / 3600) }); // L2494
  if (diffSeconds < 7 * 86_400) return fillLibraryText(LIBRARY_TEXTS.daysAgo, { count: Math.floor(diffSeconds / 86_400) }); // L2495
  return formatSessionTime(unixSeconds, now); // L2496
}
/** 腿端点文案（L2516-2520）：最早 → 至今；两端有值即两端时刻。 */
function legSpanLabelOf(span: RelayLegSpan, now: Date): string {
  const from = span.fromUnixSeconds === null ? LIBRARY_TEXTS.spanEarliest : formatSessionTime(span.fromUnixSeconds, now); // L2517
  const to = span.toUnixSeconds === null ? LIBRARY_TEXTS.spanOngoing : formatSessionTime(span.toUnixSeconds, now); // L2518
  return `${from} → ${to}`; // L2519
}
/** 台账链 → 腿区间（L657-686）：首腿最早、中间取相邻差值、当前腿「至今」并按会话总数补齐。 */
export function buildRelayLegSpans(input: RelayLegInput): readonly RelayLegSpan[] {
  const hops = relayChainsOf(input.ledger).get(input.session.session_id) ?? []; // L655-656
  if (hops.length === 0) {
    // L695-711：无台账 → 当前账号单一腿；账号名先查台账，查不到用「当前账号」。
    const name = input.currentUserId
      ? (accountNameOfLedger(input.ledger, input.currentUserId) ?? LIBRARY_TEXTS.infoCurrentAccount)
      : LIBRARY_TEXTS.infoCurrentAccount;
    return [{ userId: input.currentUserId ?? "", accountName: name,
      fromUnixSeconds: null, toUnixSeconds: null, messages: input.session.message_count }]; // L702-710
  }
  const spans: RelayLegSpan[] = [];
  // L658-665：首腿 = 首跳之前的全部消息，归属交接前账号。
  spans.push({ userId: hops[0].from_user_id, accountName: hops[0].from_account_name,
    fromUnixSeconds: null, toUnixSeconds: hops[0].switched_at_unix_seconds, messages: hops[0].message_count_at_switch });
  for (let index = 1; index < hops.length; index += 1) { // L666-676
    const hop = hops[index], previous = hops[index - 1];
    spans.push({ userId: previous.to_user_id, accountName: previous.to_account_name,
      fromUnixSeconds: previous.switched_at_unix_seconds, toUnixSeconds: hop.switched_at_unix_seconds,
      messages: Math.max(hop.message_count_at_switch - previous.message_count_at_switch, 0) }); // L674
  }
  const last = hops[hops.length - 1]; // L677
  // L678-685：当前腿 = 最后一跳交接至今（进行中），消息数 = 会话总数 − 交接时累计（负数归零）。
  spans.push({ userId: last.to_user_id, accountName: last.to_account_name,
    fromUnixSeconds: last.switched_at_unix_seconds, toUnixSeconds: null,
    messages: Math.max(input.session.message_count - last.message_count_at_switch, 0) });
  return spans; // L686
}
/** 腿区间 → RelayLegView（L2230-2252 / L2027）：current=最后一段；colorIndex 同色板；null 名回退「已移除账号」。 */
export function buildRelayLegs(input: RelayLegInput): readonly RelayLegView[] {
  const spans = buildRelayLegSpans(input);
  const colorIndices = accountColorIndicesOf(spans); // L2027 / L2433-2443
  return spans.map((span, index) => ({
    userId: span.userId,
    accountName: span.accountName ?? LIBRARY_TEXTS.removedAccount, // L2178 / L2246
    spanLabel: legSpanLabelOf(span, input.now),
    messages: span.messages,
    current: index === spans.length - 1, // L2232：当前腿 = 最后一段
    colorIndex: colorIndices.get(span.userId) ?? 0, // L2182 / L2240
  }));
}
/** 接力腿 meta 文案（L2250）：时间段 · 新增消息数。 */
export function legMetaText(leg: RelayLegView): string {
  return fillLibraryText(LIBRARY_TEXTS.legMeta, { span: leg.spanLabel, messages: leg.messages });
}
/** 用户消息归属腿（L2449-2459）：created_at 落在 [from, to) 归该腿；无时间戳归首腿，区间外归末腿。 */
export function ownerLegOf(message: SessionMessageDto, spans: readonly RelayLegSpan[]): RelayLegSpan | null {
  if (spans.length === 0) return null; // 旧实现隐含非空前提（L2451）；本层显式兜底
  const timestamp = message.created_at_unix_seconds; // L2450
  if (!timestamp || timestamp <= 0) return spans[0]; // L2451
  for (const span of spans) { // L2452-2456
    const afterFrom = span.fromUnixSeconds === null || timestamp >= span.fromUnixSeconds; // L2453
    const beforeTo = span.toUnixSeconds === null || timestamp < span.toUnixSeconds; // L2454
    if (afterFrom && beforeTo) return span; // L2455
  }
  return spans[spans.length - 1]; // L2457-2458：区间外兜底归最后一腿
}
/** 消息行（L2166-2216）：user 按归属腿着色，assistant 中性，task_trace 收成轨迹摘要行。 */
export function buildMessageViews(messages: readonly SessionMessageDto[], spans: readonly RelayLegSpan[]): readonly MessageView[] {
  const colorIndices = accountColorIndicesOf(spans); // L2027
  return messages.map((message) => {
    const isUser = message.role === "user"; // L2175
    const trace = message.content.kind === "task_trace"; // L2200-2204：轨迹摘要形态
    const span = isUser ? ownerLegOf(message, spans) : null; // L2177-2178
    return {
      id: message.message_id,
      testId: "", // 旧锚点 library-message-<id>（L2180/L2190）归装配层；本层不产出 testid
      role: isUser ? "user" : trace ? "trace" : "assistant", // L2175 / L2189-2196
      text: trace ? fillLibraryText(LIBRARY_TEXTS.traceSteps, { steps: message.content.step_count }) : message.content.text, // L2201-2206
      accountName: span ? (span.accountName ?? LIBRARY_TEXTS.removedAccount) : null, // L2178-2183
      colorIndex: span ? (colorIndices.get(span.userId) ?? 0) : 0, // L2182：助手消息中性色
      stepCount: trace ? message.content.step_count : null, // L2206
    };
  });
}
/** 查看器 pane（L2008-2160）：status 四态由读取态 + 条数派生（ready 且 0 条 = empty，L2129）；emptyText 取 L2130。 */
export function buildViewerPane(input: ViewerPaneInput): ViewerPaneView {
  const spans = buildRelayLegSpans(input);
  const empty = input.source.status === "ready" && input.source.messages.length === 0; // L2129
  return {
    ariaLabel: LIBRARY_TEXTS.regionViewer, // L1706：<section aria-label="对话查看器">
    tabsAriaLabel: LIBRARY_TEXTS.regionViewerTabs, // L2057：role=tablist aria-label="对话内容"
    activeTab: input.activeTab, // L2061 / L2071
    status: input.source.status === "loading" ? "loading" : input.source.status === "error" ? "error" : empty ? "empty" : "ready", // L2123-2131
    messages: buildMessageViews(input.source.messages, spans),
    legs: buildRelayLegs(input),
    hasMoreBefore: input.source.hasMoreBefore, // L2149
    messageOffset: input.source.messageOffset, // L2152：已加载最近 N 条
    loadingOlder: input.source.loadingOlder, // L2144
    emptyText: LIBRARY_TEXTS.messagesEmpty, // L2130
  };
}