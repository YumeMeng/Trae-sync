/**
 * WorkBuddy 库视图 builder（P4e-4b）：快照 → LibraryView 的纯映射（零 React / 零 JSX / 零 invoke / 零命令 / 零 platform 依赖）。
 * 能力只读 libraryCapabilitiesFor("workbuddy")；WB 独有四项：①软删/彻底删分离（两个确认层归 P4e-9 的 WbDialogs，本层只给触发按钮 + 禁用理由）
 * ②账号过滤档（删除归属校验锚：selected=null 即写按钮禁用 + 一句说明）③库体检五项 ④cwd 归并候选入口。cwd 分组与时间只读 relayLegs。
 * testid 逐字抄旧实现 WB = src/pages/WorkBuddyLibraryPage.tsx（541 行）；容器锚点（workbuddy-library-groups / -browser / -messages /
 * -actions 行内的 aria 与 hover 文案）归装配层，本层不发明容器锚点。
 */
import type { WorkBuddyLoginSummaryDto } from "../../types/product";
import type { WorkBuddyCwdGroupDto, WorkBuddyLibraryCheckupDto, WorkBuddyLibraryStatsDto, WorkBuddySessionMessagesDto, WorkBuddySessionRowDto } from "../../types/workbuddyLibrary";
import {
  LIBRARY_TEXTS, LIBRARY_VIEW_TEXTS, fillLibraryText, libraryCapabilitiesFor,
  type AccountFilterView, type CheckupView, type LibraryActionKind, type LibraryActionView, type LibraryBadgeView,
  type LibraryStatView, type LibraryStatsView, type LibraryView, type MessageView, type ProjectNodeView, type SessionRowView, type ViewerPaneView,
} from "./config";
import { formatSessionTime } from "./relayLegs";
/** 聚合页面已取回的 WB 数据：builder 只消费，不发起任何读取（7 条库域命令 + 账号池清单）。 */
export interface WorkBuddyLibrarySnapshot {
  readonly stats: WorkBuddyLibraryStatsDto | null; // get_workbuddy_library_stats（五项统计卡；null = 未读到）
  readonly groups: readonly WorkBuddyCwdGroupDto[] | null; // get_workbuddy_library_history（null = 未读到）
  readonly accounts: readonly WorkBuddyLoginSummaryDto[]; // list_workbuddy_accounts（账号过滤选项）
  readonly uidFilter: string; // 账号过滤当前值（"" = 全部账号；删除归属校验锚，WB117-123）
  readonly includeDeleted: boolean; // 显示软删除会话（WB273-287）
  readonly selectedIds: readonly string[]; // 逐会话复选（无项目级联动）
  readonly loading: boolean; readonly writeBusy: boolean; readonly mergeBusy: boolean; // 读取/写/归并忙碌态（只用于算 disabled）
  readonly loadError: string | null; // 读取失败原因（WB73：兜底句已由页面侧 safeUiError 映射）
  readonly checkup: WorkBuddyLibraryCheckupDto | null; readonly checking: boolean; // 体检报告 / 体检中
  readonly openSessionId: string | null; // 行内展开的会话（null = 无展开）
  readonly messages: WorkBuddySessionMessagesDto | null; readonly messagesLoading: boolean; // jsonl 正文读取态
  readonly now: Date; // 纯函数不读时钟
}
/** 消息行扩展：旧 WB371 的标签是 `role ?? kind`（config 的 MessageView.role 只有三值，装不下 kind），只读扩展携带，不改 config。 */
export interface WbMessageView extends MessageView { readonly label: string }
/** 查看器扩展：行内时间线标题含 skipped_lines / body_missing 如实提示（WB363-366），config 的 ViewerPaneView 无该字段。 */
export interface WbViewerPaneView extends ViewerPaneView { readonly messages: readonly WbMessageView[]; readonly title: string }
/** 体检扩展：归并候选的逐组入口按钮（锚点 workbuddy-library-merge-<N>，WB456）归本层产出。 */
export interface WbCheckupView extends CheckupView { readonly mergeEntries: readonly LibraryActionView[] }
/** 仍是 LibraryView（可直接交给页面），只收紧 viewer / checkup 两处元素类型。 */
export interface WorkBuddyLibraryView extends LibraryView { readonly viewer: WbViewerPaneView | null; readonly checkup: WbCheckupView | null }
/** 动作行：testId 逐字沿用旧锚点，hint 即旧 title（旧页 class 映射：btn→ghost、btn--danger→danger、btn--primary→primary）。 */
const action = (id: string, label: string, kind: LibraryActionKind, testId: string, hint: string | null = null, disabledReason: string | null = null, busyLabel: string | null = null): LibraryActionView =>
  ({ id, label, kind, testId, hint, disabledReason, busyLabel });
const badge = (text: string, tone: LibraryBadgeView["tone"], title: string | null): LibraryBadgeView => ({ text, tone, title });
/** 时间文案：旧 WB179-180 的 toLocaleDateString 只到日且随宿主 locale 漂移，改用 relayLegs.formatSessionTime（注入 now）；null 保留旧页「—」。 */
const timeOf = (unixSeconds: number | null, now: Date): string => (unixSeconds ? formatSessionTime(unixSeconds, now) : LIBRARY_TEXTS.infoDash);
/** 可写会话（WB119-124）：cloud 行不进写集合（写红线不碰云端会话）。 */
const selectableIdsOf = (snapshot: WorkBuddyLibrarySnapshot): readonly string[] => {
  const cloud = new Set((snapshot.groups ?? []).flatMap((group) => group.sessions.filter((row) => row.transport === "cloud").map((row) => row.session_id)));
  return snapshot.selectedIds.filter((id) => !cloud.has(id));
};
/** 页头（WB183-201）：标题「对话库」+ 产品徽章 + 刷新；无返回按钮、无只读徽章（只读边界落在 cloud 行）。 */
function headerOf(busy: boolean): LibraryView["header"] {
  const caps = libraryCapabilitiesFor("workbuddy");
  return {
    title: LIBRARY_TEXTS.wbRegion, regionLabel: LIBRARY_TEXTS.wbRegion, pageTitleAttr: caps.pageRegion, // WB183/186：aria 与 data-page-title
    readonlyBadge: null, badgeProductId: "workbuddy", back: null, // WB187 ProductBadge；旧页无返回与只读徽章
    refresh: action("library-refresh", LIBRARY_TEXTS.wbRefresh, "ghost", "workbuddy-library-refresh",
      LIBRARY_TEXTS.wbRefreshHint, busy ? LIBRARY_TEXTS.busyShort : null), // WB190-199
  };
}
/** 统计卡（WB220-247）：五项逐项口径；未读到即整卡不渲染（旧页 `{stats && ...}` 守卫）。 */
function statsOf(stats: WorkBuddyLibraryStatsDto | null): LibraryStatsView | null {
  if (stats === null) return null;
  const item = (id: string, label: string, value: string | number, hint: string | null = null): LibraryStatView => ({ id, label, value: String(value), hint });
  return {
    testId: "workbuddy-library-stats", title: LIBRARY_TEXTS.wbStatsTitle, unreadableNote: null,
    items: [
      item("sessions", LIBRARY_TEXTS.wbStatSessions, stats.session_count), // WB225-227
      item("projects", LIBRARY_TEXTS.wbStatProjects, stats.project_count), // WB229-231
      item("accounts", LIBRARY_TEXTS.wbStatAccounts, stats.account_count), // WB233-235
      item("softDeleted", LIBRARY_TEXTS.wbStatSoftDeleted, stats.soft_deleted_session_count, LIBRARY_TEXTS.wbSoftDeletedHint), // WB236-239
      item("bytes", LIBRARY_TEXTS.wbStatBytes, stats.projects_dir_bytes > 0 ? `${Math.round(stats.projects_dir_bytes / 1024)} KB` : LIBRARY_TEXTS.infoDash), // WB240-245
    ],
  };
}
/** 账号过滤档（WB254-272）：选项值 = account_id（命令参数口径）；未过滤即 requiredNote（WB415-417 同源一句）。 */
function filterOf(snapshot: WorkBuddyLibrarySnapshot): AccountFilterView {
  const selected = snapshot.uidFilter === "" ? null : snapshot.uidFilter;
  return {
    testId: "workbuddy-library-uid-filter", label: LIBRARY_TEXTS.wbAccountFilter, allLabel: LIBRARY_TEXTS.wbAllAccounts,
    options: snapshot.accounts.map((account) => ({ id: account.account_id, label: account.screen_name || LIBRARY_TEXTS.wbAccountFallback })), // WB266-269
    selected, requiredNote: selected === null ? LIBRARY_TEXTS.wbFilterRequiredNote : null,
  };
}
/** 会话行（WB312-355）：testId 取旧页唯一逐行锚点（复选）；cloud 行不可选、给「云端」徽章；软删行给 warn 徽章。 */
function sessionRowOf(session: WorkBuddySessionRowDto, selected: boolean, now: Date): SessionRowView {
  const cloud = session.transport === "cloud"; // WB119-124 / WB316-324
  const badges: LibraryBadgeView[] = [];
  if (cloud) badges.push(badge(LIBRARY_TEXTS.wbCloudBadge, "neutral", LIBRARY_TEXTS.wbCloudBadgeTitle)); // WB340-342
  if (session.deleted) badges.push(badge(LIBRARY_TEXTS.wbSoftDeletedBadge, "warn", null)); // WB343-345
  return {
    id: session.session_id, testId: `workbuddy-library-select-${session.session_id}`, // WB333
    title: session.title?.trim() || LIBRARY_TEXTS.unnamedSession, // WB339
    meta: fillLibraryText(LIBRARY_TEXTS.wbSessionMeta, { time: timeOf(session.last_active_unix_seconds, now) }), // WB348
    badges, selectable: !cloud, selected: selected && !cloud, archived: false, quickArchive: null, // 软删不是归档：WB 无归档（§2-7 A）
  };
}
/** 左栏（WB290-391）：cwd 转义目录逐组，组头 cwd 全路径 + 会话平铺；组不可勾选（无项目级联动，§2-3 B）。 */
function treeOf(snapshot: WorkBuddyLibrarySnapshot): readonly ProjectNodeView[] {
  const selected = new Set(snapshot.selectedIds), now = snapshot.now;
  return (snapshot.groups ?? []).map((group) => ({
    id: group.escaped_dir, testId: `workbuddy-library-group-${group.escaped_dir}`, kind: "cwd", // WB311
    title: group.cwd, titleAttr: group.cwd, // WB301：组头显示 cwd，title 同值（全路径）
    meta: fillLibraryText(LIBRARY_TEXTS.wbGroupMeta, { sessions: group.session_count, time: timeOf(group.last_active_unix_seconds, now) }), // WB305
    expanded: true, selected: false, selectable: false, actions: [], // 组头无动作
    sessions: group.sessions.map((session) => sessionRowOf(session, selected.has(session.session_id), now)), children: [],
  }));
}
/** 批量写行（WB393-418）：软删与彻底删是两个按钮、两次确认（决策 7，确认层不在本层）；未过滤即两按钮禁用 + 说明。 */
function batchBarOf(snapshot: WorkBuddyLibrarySnapshot): LibraryView["batchBar"] {
  if (!libraryCapabilitiesFor("workbuddy").softDeleteSeparate) return null; // 能力表：不分离软删/彻底删的产品不给本行
  const selectable = selectableIdsOf(snapshot), filtered = snapshot.uidFilter !== "";
  const blocked = !filtered ? LIBRARY_TEXTS.wbFilterFirstHint : snapshot.writeBusy ? LIBRARY_TEXTS.busyShort : selectable.length === 0 ? "未选择会话" : null;
  return {
    testId: "workbuddy-library-actions", ariaLabel: "", // 旧页该行无 aria-label（不发明）；未过滤说明归 filter.requiredNote（WB415）
    summary: "", // 旧页无汇总文案：计数写在按钮标签里（WB403/413）
    actions: [
      action("batch-soft-delete", fillLibraryText(LIBRARY_TEXTS.wbSoftDelete, { count: selectable.length }), "ghost", "workbuddy-library-soft-delete",
        filtered ? LIBRARY_TEXTS.wbSoftDeleteHint : LIBRARY_TEXTS.wbFilterFirstHint, blocked), // WB395-404
      action("batch-purge", fillLibraryText(LIBRARY_TEXTS.wbPurge, { count: selectable.length }), "danger", "workbuddy-library-purge",
        LIBRARY_TEXTS.wbPurgeHint, blocked), // WB405-414
    ],
  };
}
/** 右栏行内时间线（WB356-381）：jsonl 只读；无接力腿、无分页、无写控件；skipped_lines / body_missing 如实进标题。 */
function viewerOf(snapshot: WorkBuddyLibrarySnapshot): WbViewerPaneView | null {
  if (snapshot.openSessionId === null) return null;
  const messages = snapshot.messages, loading = snapshot.messagesLoading;
  const rows: WbMessageView[] = (messages?.messages ?? []).map((message) => ({
    id: String(message.index), testId: "", // WB370 key={index}：行内时间线无逐条锚点（不发明）
    role: message.role === "user" ? "user" : "assistant", label: message.role ?? message.kind, // WB371 标签原文
    text: message.text ?? LIBRARY_TEXTS.wbNoText, accountName: null, colorIndex: 0, stepCount: null, // WB372；WB 无接力与轨迹
  }));
  const missing = messages?.body_missing === true, skipped = messages?.skipped_lines ?? 0;
  return {
    ariaLabel: "", tabsAriaLabel: "", activeTab: "messages", // 旧行内块无 aria-label、无消息/接力 tab（不发明）
    status: loading ? "loading" : messages === null || rows.length === 0 ? "empty" : "ready", // WB360-378：读取中 / 暂无内容 / 列表
    messages: rows, legs: [], hasMoreBefore: false, messageOffset: 0, loadingOlder: false, // 无接力腿、无更早分页
    emptyText: LIBRARY_TEXTS.wbNoContent,
    title: fillLibraryText(LIBRARY_TEXTS.wbMessagesTitle, { count: rows.length,
      bodyMissing: missing ? LIBRARY_TEXTS.wbBodyMissing : "", skipped: skipped > 0 ? fillLibraryText(LIBRARY_TEXTS.wbSkippedLines, { lines: skipped }) : "" }), // WB363-366
  };
}
/** 库体检（WB421-481）：五项 + 归并候选入口 + 立即体检；报告未读到只给 idleNote（旧页 `{checkup ? ... : idleNote}`）。 */
function checkupOf(snapshot: WorkBuddyLibrarySnapshot): WbCheckupView | null {
  if (libraryCapabilitiesFor("workbuddy").checkupItems === 0) return null; // 能力表：无体检项即整卡不出现
  const report = snapshot.checkup, busy = snapshot.checking;
  const item = (id: string, label: string, value: string) => ({ id, label, value, hint: null });
  const groups = report?.cwd_merge_candidates ?? [];
  return {
    testId: "workbuddy-library-checkup", title: LIBRARY_TEXTS.wbCheckupTitle, // WB421-422
    items: report === null ? [] : [
      item("orphan", LIBRARY_TEXTS.wbCheckupOrphan, fillLibraryText(LIBRARY_TEXTS.wbCheckupOrphanValue, { count: report.orphan_body_files.length })), // WB426-429
      item("missing", LIBRARY_TEXTS.wbCheckupMissing, fillLibraryText(LIBRARY_TEXTS.wbCheckupMissingValue,
        { count: report.missing_body_count, sample: report.missing_body_samples.length > 0 ? `，如 ${report.missing_body_samples[0]}` : "" })), // WB430-433
      item("softDeleted", LIBRARY_TEXTS.wbCheckupSoftDeleted, fillLibraryText(LIBRARY_TEXTS.wbCheckupSoftDeletedValue, { count: report.soft_deleted_session_count })), // WB434-437
      item("foreign", LIBRARY_TEXTS.wbCheckupForeign, fillLibraryText(LIBRARY_TEXTS.wbCheckupForeignValue, { count: report.foreign_dirs.length })), // WB438-441
      item("candidates", LIBRARY_TEXTS.wbCheckupCandidates, fillLibraryText(LIBRARY_TEXTS.wbCheckupCandidatesValue, { count: groups.length })), // WB442-445
    ],
    candidates: groups.map((group) => ({ canonical: group[0], variants: group.slice(1) })), // canonical 在前（能力表口径）
    mergeEntries: groups.map((group) => action(`merge-${group[0]}`, fillLibraryText(LIBRARY_TEXTS.wbMergeEntry, { cwd: group[0] }), "ghost",
      `workbuddy-library-merge-${group.length}`, // WB456：锚点取组长度（canonical + variants）
      fillLibraryText(LIBRARY_TEXTS.wbMergeHint, { count: group.length - 1, cwd: group[0] }), // WB457
      snapshot.mergeBusy ? LIBRARY_TEXTS.wbMergeConfirmBusy : null)),
    action: action("checkup-run", LIBRARY_TEXTS.wbCheckupAction, "ghost", "workbuddy-library-checkup-button", null, busy ? LIBRARY_TEXTS.wbCheckupBusy : null), // WB470-479
    idleNote: busy ? LIBRARY_TEXTS.wbCheckupBusy : LIBRARY_TEXTS.wbCheckupIdle, // WB467
  };
}
/** 快照 → 库视图（纯函数：同一快照恒得同一视图）。 */
export function buildWorkBuddyLibraryView(snapshot: WorkBuddyLibrarySnapshot): WorkBuddyLibraryView {
  const busy = snapshot.loading || snapshot.writeBusy, groups = snapshot.groups;
  const emptyTree = groups !== null && groups.length === 0; // WB290/389：组列表为空才给空态句
  return {
    productId: "workbuddy", libraryId: "workbuddy", // WB 无库实例概念：与 productId 同值，仅作页面唯一键
    header: headerOf(busy), tabs: [], // WB 无 tab 条（插件/库信息 tab 均 absent，§2-12 / §2-13 A）
    loadingNote: snapshot.loading ? LIBRARY_TEXTS.wbLoading : null, // WB203-204
    error: snapshot.loadError !== null
      ? { title: LIBRARY_TEXTS.wbErrorTitle, description: snapshot.loadError, retryLabel: LIBRARY_TEXTS.wbReload } : null, // WB205-216
    empty: null, // 无整页空态：无会话时正文给 treeEmpty 一句（WB389），不叠两块空态
    stats: statsOf(snapshot.stats), filter: filterOf(snapshot),
    includeDeleted: { testId: "workbuddy-library-include-deleted", label: LIBRARY_TEXTS.wbIncludeDeleted, hint: LIBRARY_TEXTS.wbIncludeDeletedHint, checked: snapshot.includeDeleted }, // WB273-287
    search: null, // §2-2 A：WB 无标题搜索
    tree: treeOf(snapshot), treeEmpty: emptyTree ? LIBRARY_TEXTS.wbEmptyGroups : "",
    viewer: viewerOf(snapshot), batchBar: batchBarOf(snapshot),
    draftBar: null, // §2-7 A：WB 无归档（软删是删除语义，不是归档）
    checkup: checkupOf(snapshot), plugins: null, info: null, // §2-12 / §2-13 A：WB 无插件 tab、无库信息 tab（统计卡承担）
    notices: [], texts: LIBRARY_VIEW_TEXTS.workbuddy, // WB 回执前缀「WorkBuddy：」
  };
}
