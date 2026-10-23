// 账号详情 builder（P4c-7a）：快照 + 行视图 → DetailViewModel 的纯映射（零 React / 零 JSX / 零 invoke）。
// 逐项对照三份旧详情组件：字段顺序、文案、data-testid（= 字段 id / 动作 id）逐字保留；旧实现无 testid 的行按同前缀派生
// （清单与行号见 .scratch/cmdr/P4c-7a-report.md）。产品差异只走文件末尾登记表，能力裁剪只读 accountCapabilitiesFor。
import { deriveLoginSlotState, deriveWorkBuddyPoolLoginState, type LoginSlotState } from "../../components/StatusBadges";
import type { AccountView } from "../../platform/accountAdapter";
import type { CheckinOverviewEntryDto, TraeInstanceStateDto } from "../../types/account_switch";
import type { WorkBuddyAccountEntryDto, WorkBuddyCheckinAccountStatusDto } from "../../types/product";
import { displayMobile, effectiveDisplayName, workbuddyDisplayName } from "../../utils/accountDisplay";
import { safeUiErrorMessage } from "../../utils/safeUiError";
import { ACCOUNT_TEXTS, accountCapabilitiesFor, type AccountActionView, type AccountCapabilities, type AccountDetailFieldView, type AccountRowView, type DetailActionView, type DetailFieldView, type DetailSectionView, type DetailViewModel } from "./config";
/** TRAE 数据面：checkin=真实总览（旧 AccountDetail）/ pool=适配器池（旧 AdapterAccountDetail）。 */
export type TraeDetailSnapshot =
  | { readonly face: "checkin"; readonly entry: CheckinOverviewEntryDto; readonly loginState?: TraeInstanceStateDto; readonly currentAccountStatus: "current" | "other" | "unknown"; readonly nowUnixSeconds: number; readonly busy?: readonly string[] }
  | { readonly face: "pool"; readonly account: AccountView; readonly toolSelectionBusy: boolean };
/** WB 数据面：池条目 + 逐账号签到面（null = 未读到）+ 参与集状态（null = 不可读取，与旧实现同态）。 */
export interface WorkBuddyDetailSnapshot { readonly account: WorkBuddyAccountEntryDto; readonly overview: WorkBuddyCheckinAccountStatusDto | null;
  readonly participating: boolean | null; readonly isCurrentLogin: boolean; readonly nowUnixSeconds: number; readonly busy?: readonly string[] }
/** 产品快照：判别字段 productId 与载荷字段同名同值（与 registry.ts 的 AccountsSnapshot 同形）。 */
export type DetailSnapshot = { readonly productId: "trae_cn"; readonly trae: TraeDetailSnapshot } | { readonly productId: "workbuddy"; readonly workbuddy: WorkBuddyDetailSnapshot };
/** builder 入参：数据面一律取 snapshot；row 只提供页头徽章（文案零新增）。 */
export interface DetailViewInput { readonly productId: string; readonly row: AccountRowView; readonly snapshot: DetailSnapshot }
/** 字段行 = [id(=旧 data-testid), label, value, tone?, hint?]；动作行 = [id, label, kind, disabledReason?]。 */
type Field = AccountDetailFieldView & DetailFieldView; // 同时满足既有分区模型与 DetailFieldView（tone/hint 供分区组件取用）
type Row = readonly [string, string, string, ("warn" | "danger")?, string?]; // tone 只有 warn/danger（不提供成功绿）
type ActRow = readonly [string, string, AccountActionView["kind"], string?]; // kind 与页头/行内动作同一套
const F = (rows: readonly Row[]): Field[] => rows.map(([id, label, value, tone, hint]) => ({ id, label, value, tone, hint }));
const A = (rows: readonly ActRow[]): (AccountActionView & DetailActionView)[] => rows.map(([id, label, kind, reason]) => ({ id, label, kind, disabledReason: reason, disabled: reason === undefined ? undefined : { reason } }));
/** 行内编辑字段装配（P4c-6b-2e-2）：展示值 + 编辑描述（草稿/占位/长度由 builder 给，渲染层零文案）。 */
const editField = (id: string, label: string, value: string, kind: "text" | "tel", maxLength: number, draft: string, placeholder: string): Field =>
  ({ id, label, value, edit: { kind, maxLength, draft, placeholder } });
const sec = (id: DetailSectionView["id"], title: string, fields: readonly Field[], actions: readonly AccountActionView[] = [], hint: string | null = null): DetailSectionView => ({ id, title, fields, actions, hint }); // 六区装配：字段与动作声明式给出
const maybe = (on: boolean, section: DetailSectionView): DetailSectionView[] => (on ? [section] : []); // 能力关闭即整区不出现（缺省不占位）
// 文案口径：逐字抄旧实现的三个格式化函数（TRAE 详情 L549-576 / WB 详情 L421-443 / 适配器详情 L140-148）。
const fmtDate = (at: Date): string => new Intl.DateTimeFormat("zh-CN", { year: "numeric", month: "2-digit", day: "2-digit", hour: "2-digit", minute: "2-digit", hour12: false }).format(at);
const fmtDateTime = (v: string | null): string => (!v || Number.isNaN(new Date(v).getTime()) ? "未知" : fmtDate(new Date(v))); // 空/不可读一律「未知」
const fmtUnixTime = (unixSeconds: number): string => (unixSeconds ? new Date(unixSeconds * 1000).toLocaleString(undefined, { year: "numeric", month: "2-digit", day: "2-digit", hour: "2-digit", minute: "2-digit" }) : "未知");
const fmtCredits = (value: number): string => (Number.isInteger(Math.round(value * 10) / 10) ? String(Math.round(value * 10) / 10) : (Math.round(value * 10) / 10).toFixed(1)); // 保留 1 位小数，整数不带小数点
const traeExpiry = (expiresAt: number | null, refresh: boolean, now: number): string => expiresAt === null ? "未能读取" : expiresAt <= now ? "已过期" : `${refresh ? "刷新" : "访问"}令牌剩 ${Math.ceil((expiresAt - now) / 86400)} 天（至 ${fmtDate(new Date(expiresAt * 1000))}）`;
const wbExpiry = (expiresAt: number, now: number): string => (!expiresAt ? "有效期未知（档案未采集）" : expiresAt <= now ? "已过期" : `剩 ${Math.ceil((expiresAt - now) / 86400)} 天（至 ${fmtDate(new Date(expiresAt * 1000))}）`);
const adapterTime = (value: string | null): string => (!value ? "尚未验证" : Number.isNaN(new Date(value).getTime()) ? "时间不可用" : fmtDate(new Date(value)));
const SLOT_WORD: Record<LoginSlotState, string> = { ok: "正常", expired: "已过期", relogin: "需重登", pending: "待登录", signed_out: "未登录" }; // 逐字抄 StatusBadges 的词
const WB_SLOT_WORD: Record<LoginSlotState, string> = { ...SLOT_WORD, pending: "待确认" }; // WB 池槽的 pending 词不同
/** 自动签到参与开关的两态文案（TRAE 详情与 WB 参与行同词，旧实现逐字）。 */
const TOGGLE_WORDS = { on: "参与每日自动签到", off: "不参与每日自动签到" } as const;
/** WB 归档开关两态文案（旧 WorkBuddyAccountDetail L347 逐字）。 */
const ARCHIVE_WORDS = { on: "已归档", off: "在册" } as const;
const WB_CREDITS = "workbuddy-detail-credits"; // WB 余额四态（逐字）：未获取 / 查询失败 / 余额查询失败（含原因）/ 真实数值（0 照实）
const wbBalance = (o: WorkBuddyCheckinAccountStatusDto | null): Row => (o === null ? [WB_CREDITS, "积分余额", "未获取"]
  : o.error !== null ? [WB_CREDITS, "积分余额", "查询失败（签到状态读取失败）", "warn"]
  : o.credits_balance !== null ? [WB_CREDITS, "积分余额", String(o.credits_balance)]
  : [WB_CREDITS, "积分余额", o.balance_error === null ? "余额查询失败" : `余额查询失败（${safeUiErrorMessage(o.balance_error, "请稍后刷新。")}）`, "warn"]);
const wbMetric = (o: WorkBuddyCheckinAccountStatusDto | null, id: string, label: string, pick: (row: WorkBuddyCheckinAccountStatusDto) => string | null): Row => // 整行失败=查询失败，未读到=未获取
  (o === null ? [id, label, "未获取"] : o.error !== null ? [id, label, "查询失败", "warn"] : [id, label, pick(o) ?? "未获取"]);
const wbParticipation = (on: boolean | null): Field => (on === null
  ? { id: "workbuddy-detail-participation-unknown", label: "自动签到参与", value: "参与状态暂时不可读取，请返回后重试。", tone: "warn" }
  : { id: "workbuddy-detail-participation-toggle", label: "自动签到参与", value: on ? TOGGLE_WORDS.on : TOGGLE_WORDS.off,
      toggle: { on, onLabel: TOGGLE_WORDS.on, offLabel: TOGGLE_WORDS.off } }); // 可读取才给开关
/** TRAE 真实总览详情（旧 AccountDetail L246-545）：六区 basics→credential→checkin→actions→danger→tech。 */
function traeCheckinDetail(caps: AccountCapabilities, snapshot: Extract<TraeDetailSnapshot, { face: "checkin" }>, row: AccountRowView): DetailViewModel {
  const { entry, loginState, currentAccountStatus: current, nowUnixSeconds: now } = snapshot;
  const checkedIn = entry.checked_in === true, failed = entry.refresh_error_code !== null;
  // 额度与刷新状态（旧 L368-386）：未查询给动作提示；失败态旧实现用琥珀 + 悬浮说明。
  const usage = entry.usage_remaining_credits == null ? "未查询（点击下方“刷新额度”）" : `${fmtCredits(entry.usage_remaining_credits)}${entry.usage_cached_at ? `（${fmtDateTime(entry.usage_cached_at)} 更新）` : ""}`;
  const refresh = failed ? `失败：${safeUiErrorMessage(entry.refresh_error_code ?? "", "原因未知，请稍后重试。")}` : entry.usage_cached_at ? `正常（${fmtDateTime(entry.usage_cached_at)}）` : "尚未刷新";
  // 基础信息（旧 L257-402，顺序逐字保留）：备注名/登录状态/当前账号/最近验证/手机号/额度/刷新状态/今日签到/添加时间/设备尾号。
  // 行内编辑字段（P4c-6b-2e-2）：草稿/占位/长度逐字沿用旧实现（AccountDetail L265-288 / L334-350）。
  const basics: readonly Field[] = [
    editField("account-detail-alias", "备注名", entry.display_name ?? entry.screen_name, "text", 64, entry.display_name ?? "", entry.screen_name),
    ...F([
      ["account-detail-login-status", "登录状态", loginState === undefined ? "状态待验证" : SLOT_WORD[deriveLoginSlotState({ login_state: loginState.login_state, relogin_only: entry.credential_legacy })]],
      ["account-detail-current-account", "当前账号", current === "current" ? "使用中" : current === "other" ? "不是当前账号" : "暂未确认"],
      ["account-detail-verified", "最近验证", fmtDateTime(entry.last_verified_at)],
    ]),
    // G11 补录（WB 无此能力）：草稿 = 已补录全号、占位 = 脱敏号、上限 11（旧 L334-350）。
    ...(caps.mobileBackfill ? [editField("account-detail-mobile", "手机号", displayMobile(entry) || "未采集", "tel", 11, entry.mobile_full ?? "", entry.masked_mobile || "未采集")] : []),
    ...(caps.mobileBackfill && entry.masked_mobile ? F([["account-detail-mobile-hint", "服务端脱敏号", `${entry.masked_mobile}（输入需与其首尾号段一致）`]]) : []),
    ...F([
      ["account-detail-usage", "模型积分（真实额度）", usage],
      ...(failed ? [["account-detail-refresh-status", "最近额度刷新", refresh, "warn", "下次刷新成功后此提示自动消失"] as const] : [["account-detail-refresh-status", "最近额度刷新", refresh] as const]),
      ["account-detail-checked-in", "今日签到", entry.checked_in === true ? "已签到" : entry.checked_in === false ? "未签到" : "未知（刷新后显示）"],
      ["account-detail-created-at", "添加时间", fmtDateTime(entry.created_at)],
      ["account-detail-device-tail", "签到设备", entry.device_tail ? `…${entry.device_tail}` : "未知"],
    ]),
  ];
  return {
    header: { title: effectiveDisplayName(entry), badges: row.badges },
    sections: [
      // 行内编辑的保存按钮与字段同区（旧 L280-359）。
      sec("basics", "基础信息", basics, A([["account-detail-alias-save", "保存", "ghost"], ...(caps.mobileBackfill ? [["account-detail-mobile-save", "保存", "ghost"] as const] : [])])),
      // 登录健康度（旧 L406-425）：旧通道账号用提示替代「可自动续期」的误导文案。
      sec("credential", "登录健康度", F([
        ["account-detail-access-expiry", "访问令牌（access）", traeExpiry(entry.access_token_expires_at_unix_seconds, false, now)], ["account-detail-refresh-expiry", "刷新令牌（refresh）", traeExpiry(entry.refresh_token_expires_at_unix_seconds, true, now)],
        ...(entry.credential_legacy ? [["account-detail-legacy-hint", "凭据通道", "该账号仍在使用旧版登录通道；可先尝试“刷新登录凭据”，若当前凭据已无法换发，再重新登录。", "warn"] as const] : []),
      ]), [], entry.credential_legacy ? null : "应用会按需维护登录凭据，也可手动刷新；不会重复打开用户 OAuth 登录。"),
      // 自动签到（旧 L428-449）：只影响自动批次，手动签到不受限。
      // 开关字段（P4c-6b-2e-2）：on 与两态文案都由 builder 给，页面乐观翻转只覆盖 on。
      ...maybe(caps.autoCheckinToggle, sec("checkin", "自动签到", [{
        id: "auto-checkin-account-toggle", label: "参与状态",
        value: entry.auto_checkin_enabled ? TOGGLE_WORDS.on : TOGGLE_WORDS.off,
        toggle: { on: entry.auto_checkin_enabled, onLabel: TOGGLE_WORDS.on, offLabel: TOGGLE_WORDS.off },
      }], [], "到点后与其他账号随机错峰执行；关闭后仍可手动签到。")),
      // 操作（旧 L452-486）：按钮顺序即旧实现顺序，能力裁剪走能力表。
      sec("actions", "操作", [], A([
        ...(caps.credentialRefresh === "batch-with-confirm" ? [["account-detail-refresh-credentials", "刷新登录凭据", "primary"] as const] : []),
        ...(caps.creditsColumn ? [["account-detail-refresh-credits", "刷新额度", "ghost"] as const] : []),
        ...(caps.inlineCheckin ? [["account-detail-checkin", checkedIn ? "今日已签到" : "立即签到", "primary", checkedIn ? "今日已签到（缓存确认），无需重复领取" : undefined] as const] : []),
        ["account-detail-relogin", "重新登录", "ghost"], ...(caps.resetDevice ? [["account-detail-reset-device", "重置签到设备", "ghost"] as const] : []),
      ])),
      sec("danger", ACCOUNT_TEXTS.dangerHeading, [], A([["account-detail-remove", "删除账号", "danger"]]), ACCOUNT_TEXTS.dangerHintTrae), // 旧 L500-510
      // 技术细节（旧 L513-529，默认折叠）：排障用标识。
      sec("tech", "技术细节", F([["account-detail-tech-profile", "账号档案标识", entry.profile_id], ["account-detail-tech-account", "服务端账号标识", entry.account_id], ["account-detail-tech-device", "设备 ID", entry.device_id ?? "未知"]])),
    ],
    busy: snapshot.busy,
  };
}
/** TRAE 适配器池详情（旧 AdapterAccountDetail L54-135）：基础信息 + 可选工具账号选择 + 技术细节。 */
function traePoolDetail(caps: AccountCapabilities, snapshot: Extract<TraeDetailSnapshot, { face: "pool" }>, row: AccountRowView): DetailViewModel {
  const { account, toolSelectionBusy } = snapshot;
  const status = account.status === "active" ? "登录有效" : account.status === "expired" ? "登录已过期" : "状态待验证";
  return {
    header: { title: account.displayName || "未命名账号", badges: row.badges },
    sections: [
      // 基础信息（旧 L66-94）：字段全部来自适配器快照，顺序逐字保留。
      sec("basics", "基础信息", F([["adapter-account-detail-status", "登录状态", status], ["adapter-account-detail-current", "工具内选择", account.isToolSelected ? "工具内已选" : "工具内未选"], ["adapter-account-detail-verified", "最近验证", adapterTime(account.lastVerifiedAt)]])),
      // 操作区只在「有工具账号选择能力且目标未选中」时出现（旧 L97-114）：缺省即整区不出现。
      ...maybe(caps.toolSelection && !account.isToolSelected, sec("actions", "操作", [], A([["adapter-account-detail-tool-select", "设置工具当前账号", "primary", toolSelectionBusy ? "正在设置工具账号，请稍候" : undefined]]))),
      sec("tech", "技术细节", F([["adapter-account-detail-tech-id", "账号标识", account.id]])), // 旧 L126-134
    ],
  };
}
/** WB 详情（旧 WorkBuddyAccountDetail L166-419）：六区（签到与积分含自动签到参与开关）。 */
function workbuddyDetail(caps: AccountCapabilities, snapshot: WorkBuddyDetailSnapshot, row: AccountRowView): DetailViewModel {
  const { account, overview, participating, isCurrentLogin, nowUnixSeconds: now } = snapshot;
  // 旧 L159-163：池凭据双过期判定（TRAE 走 deriveLoginSlotState，WB 走池双过期）。
  const slot = deriveWorkBuddyPoolLoginState({ access_expires_at_unix_seconds: account.access_token_expires_at_unix_seconds, refresh_expires_at_unix_seconds: account.refresh_token_expires_at_unix_seconds, now_unix_seconds: now });
  return {
    header: { title: workbuddyDisplayName(account), badges: row.badges },
    sections: [
      // 基础信息（旧 L178-212）：脱敏手机号有值才显示。
      sec("basics", "基础信息", F([["workbuddy-detail-status", "登录状态", WB_SLOT_WORD[slot]], ["workbuddy-detail-current", "当前账号", isCurrentLogin ? "使用中" : "不是当前账号"],
        ...(account.masked_phone ? [["workbuddy-detail-phone", "手机号", account.masked_phone] as const] : []), ["workbuddy-detail-updated", "最近更新", fmtUnixTime(account.last_updated_at_unix_seconds)]])),
      // 凭据健康度（旧 L216-229）。
      sec("credential", "凭据健康度", F([["workbuddy-detail-access-expiry", "访问凭据（access）", wbExpiry(account.access_token_expires_at_unix_seconds, now)], ["workbuddy-detail-refresh-expiry", "续期凭据（refresh）", wbExpiry(account.refresh_token_expires_at_unix_seconds, now)]]),
        [], "应用会在切号与凭据充裕判断下自动续期；也可在账号页用「刷新登录凭据」手动续期。"),
      // 签到与积分（旧 L232-300）：余额主展示 + 累计/连签/今日 + 参与开关。
      sec("checkin", "签到与积分", [
        ...F([
          wbBalance(overview), wbMetric(overview, "workbuddy-detail-accumulated", "本期签到累计", (r) => (r.total_credits === null ? null : String(r.total_credits))),
          wbMetric(overview, "workbuddy-detail-streak", "连签天数", (r) => (r.streak_days === null ? null : String(r.streak_days))), wbMetric(overview, "workbuddy-detail-today", "今日签到", (r) => (r.today_checked_in === true ? "已签到" : r.today_checked_in === false ? "未签到" : null)),
        ]),
        ...(caps.autoCheckinToggle ? [wbParticipation(participating)] : []), // 参与开关（不可读取时 builder 给中性说明行）
      ], [], "签到动作在「签到」页；每日自动签到的总开关与触发时间在「设置」页。"),
      // 档案管理（旧 L304-354）：备注名 + 归档（归档只影响列表标注）。
      sec("actions", "档案管理", [
        // 备注名行内编辑（旧 L308-334：draft = 档案原值、占位 = 捕获昵称、上限 100）。
        editField("workbuddy-detail-alias", "备注名", account.display_name || account.screen_name, "text", 100, account.display_name, account.screen_name),
        // 归档开关（旧 L336-352）：只影响列表标注，不自动退出签到参与集。
        ...(caps.archiveTag ? [{ id: "workbuddy-detail-archived-toggle", label: "归档",
          value: account.archived ? ARCHIVE_WORDS.on : ARCHIVE_WORDS.off,
          toggle: { on: account.archived, onLabel: ARCHIVE_WORDS.on, offLabel: ARCHIVE_WORDS.off } }] : []),
      ], A([["workbuddy-detail-alias-save", "保存", "ghost"]])),
      sec("danger", ACCOUNT_TEXTS.dangerHeading, [], A([["workbuddy-detail-purge", "删除账号", "danger"]]), ACCOUNT_TEXTS.dangerHintWorkbuddy), // 旧 L357-375
      // 技术细节（旧 L378-394，默认折叠）。
      sec("tech", "技术细节", F([["workbuddy-detail-tech-profile", "账号档案标识", account.profile_id], ["workbuddy-detail-tech-account", "服务端账号标识", account.account_id], ["workbuddy-detail-tech-domain", "登录域", account.domain]])),
    ],
    busy: snapshot.busy,
  };
}
const payloadOf = <T,>(snapshot: DetailSnapshot, key: string): T => (snapshot as unknown as Record<string, unknown>)[key] as T; // 按载荷键取快照里的产品数据（键一一对应，与 registry.ts 同法）
/** 查表分发：键 = productId（与 registry.ts 的 BUILDERS 同形，新增产品只在此加一行）。 */
const BUILDERS: Record<string, (input: DetailViewInput) => DetailViewModel> = {
  trae_cn: (input) => { // 能力表在入口处取一次；数据面按 face 分流
    const caps = accountCapabilitiesFor(input.productId), s = payloadOf<TraeDetailSnapshot>(input.snapshot, "trae");
    return s.face === "pool" ? traePoolDetail(caps, s, input.row) : traeCheckinDetail(caps, s, input.row);
  },
  workbuddy: (input) => workbuddyDetail(accountCapabilitiesFor(input.productId), payloadOf<WorkBuddyDetailSnapshot>(input.snapshot, "workbuddy"), input.row),
};
/** 单账号「刷新登录凭据」动作 id（P4c-7c-1 页面接线用）：两产品同语义，逐字沿用旧 testid。 */
export const DETAIL_CREDENTIAL_REFRESH_ACTIONS: readonly string[] = [
  "account-detail-refresh-credentials", // 旧 AccountDetail L455
  "workbuddy-detail-refresh-credentials", // WB 详情的前瞻 id（当前 builder 未产出，页面按 id 直连）
];
/** 快照 + 行 → 详情视图模型（纯函数）；未登记产品 fail-closed：只有行标题与徽章，无分区无动作。 */
export function buildDetailView(input: DetailViewInput): DetailViewModel {
  return BUILDERS[input.productId]?.(input) ?? { header: { title: input.row.displayName, badges: input.row.badges }, sections: [] };
}