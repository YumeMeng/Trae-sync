/**
 * 两槽位徽章系统（设计契约 docs/DESIGN_TOKENS.md；G10 统一状态机，2026-09-03 grill 确立）。
 *
 * 全局唯一状态语言：所有页面的账号状态徽章统一经过本模块，
 * 保证一处学会处处适用（账号卡片/列表行、签到页、总览、详情同义同色）。
 *
 * 四色语义（G10 全局统一）：绿 = 确认正常；灰 = 中性无需动作；
 * 琥珀 = 需关注 / 可自动解决；红 = 确定性失败，必须人工处理。
 * 槽位永不消失（无数据态显示灰空心占位），布局零跳动。
 *
 * 判定与渲染分离：derive* 纯函数负责「输入 → 枚举态」，
 * 徽章组件只接收枚举态负责「词 × 色」，两页（账号页/签到页）共用同一判定。
 */
import type { TraeInstanceLoginState } from "../types/account_switch";
import type { EnvironmentProcessState } from "../types/environment";

// ===== G10 签到槽状态机（六态） =====

/** 签到槽六态（G10 决策：消灭「状态未知」一词）。 */
export type CheckinSlotState =
  | "checked_in" // 已签（绿）：服务端确认
  | "unchecked" // 未签（灰）：今日无尝试
  | "failed" // 签到失败（红）：业务性失败
  | "retry_pending" // 待重试（琥珀）：传输性失败
  | "not_eligible" // 不可领取（灰）：服务端业务拒绝
  | "not_refreshed"; // 未刷新（灰空心）：今日无服务端数据

/** 签到槽判定输入（G10c 接入时由调用方从 CheckinOverviewEntryDto 提取）。 */
export interface CheckinSlotInput {
  /** 服务端确认的今日签到状态；null/undefined = 今日无服务端数据。 */
  readonly checked_in: boolean | null | undefined;
  /** 今日最近一次尝试的结果码（见 CheckinOverviewEntryDto.last_attempt_outcome）。 */
  readonly last_attempt_outcome?: string | null;
  /** 最近一次尝试发生时间（RFC3339）。 */
  readonly last_attempt_date?: string | null;
}

/**
 * 签到槽判定（纯函数）：
 * 优先级 = 已签（服务端最终态） > 今日尝试结果 > 未刷新（无数据） > 未签。
 * 今日尝试按本地日期判定，跨日不残留（签到状态 CST 午夜重置）。
 */
export function deriveCheckinSlotState(
  input: CheckinSlotInput,
  now: Date = new Date(),
): CheckinSlotState {
  // 已签是服务端确认的最终态：即使存在早先的失败尝试残留也以它为准。
  if (input.checked_in === true) return "checked_in";
  const outcome = input.last_attempt_outcome ?? null;
  if (outcome && isSameLocalDay(input.last_attempt_date, now)) {
    if (outcome === "not_eligible") return "not_eligible";
    if (outcome.startsWith("business:")) return "failed";
    // "ok" = 签到尝试成功（checked_in 未确认的秒级竞态窗口）：按已签处理，
    // 不落入下方琥珀兜底（写缓存与写尝试两次落盘跨 00:00 时可能出现）。
    if (outcome === "ok") return "checked_in";
    // transport: 前缀与其他未归类结果码都按需关注处理（琥珀）——
    // 失败尝试即使 checked_in 未确认（null）也不丢失，避免误显示「未刷新」。
    return "retry_pending";
  }
  if (input.checked_in == null) return "not_refreshed";
  return "unchecked";
}

/**
 * 尝试日期是否与 now 同一本地日。兼容两种形态：
 * - 后端 DTO 输出的本地日期串（YYYY-MM-DD）：直接字符串比对，时区无关。
 * - 测试/历史路径传入的 RFC3339 时间戳：Date 解析后按本地年月日比对。
 * 无法解析按否处理（退回“今日无尝试”）。
 */
function isSameLocalDay(dateText: string | null | undefined, now: Date): boolean {
  if (!dateText) return false;
  if (/^\d{4}-\d{2}-\d{2}$/.test(dateText)) {
    return dateText === localDateString(now);
  }
  const at = new Date(dateText);
  if (Number.isNaN(at.getTime())) return false;
  return (
    at.getFullYear() === now.getFullYear() &&
    at.getMonth() === now.getMonth() &&
    at.getDate() === now.getDate()
  );
}

/** Date 的本地日期串（YYYY-MM-DD），与后端 local_today_string 同一口径。 */
function localDateString(at: Date): string {
  const month = String(at.getMonth() + 1).padStart(2, "0");
  const day = String(at.getDate()).padStart(2, "0");
  return `${at.getFullYear()}-${month}-${day}`;
}

/** 签到槽各态渲染配置：词 × 色 × 点形 × 悬浮提示。 */
const CHECKIN_SLOT_VARIANTS: Record<
  CheckinSlotState,
  { word: string; className: string; dotClassName: string; title: string }
> = {
  checked_in: {
    word: "已签",
    className: "slot-badge--ok",
    dotClassName: "",
    title: "该账号今日已完成签到",
  },
  unchecked: {
    word: "未签",
    className: "slot-badge--idle",
    dotClassName: "slot-badge__dot--hollow",
    title: "该账号今日尚未签到",
  },
  failed: {
    word: "签到失败",
    className: "slot-badge--danger",
    dotClassName: "",
    title: "今日签到被服务端拒绝；可稍后重试，持续失败请查看该账号的签到记录",
  },
  retry_pending: {
    word: "待重试",
    className: "slot-badge--warn",
    dotClassName: "slot-badge__dot--warn",
    title: "今日签到未完成（网络或服务暂不可用）；稍后重试即可",
  },
  not_eligible: {
    word: "不可领取",
    className: "slot-badge--idle",
    dotClassName: "slot-badge__dot--muted",
    title: "该账号当前不可领取签到积分（服务端业务规则拒绝）",
  },
  not_refreshed: {
    word: "未刷新",
    className: "slot-badge--idle slot-badge--unloaded",
    dotClassName: "slot-badge__dot--hollow",
    title: "今日签到状态尚未获取；刷新额度后显示",
  },
};

/**
 * 槽位 1 · 签到状态（G10 六态）：调用方先用 deriveCheckinSlotState 求枚举态。
 */
export function CheckinSlotBadge({ state }: { state: CheckinSlotState }) {
  const variant = CHECKIN_SLOT_VARIANTS[state];
  return (
    <span className={`slot-badge ${variant.className}`} title={variant.title}>
      <i className={`slot-badge__dot ${variant.dotClassName}`} aria-hidden="true" />
      {variant.word}
    </span>
  );
}

// ===== G10 登录槽状态机（五态） =====

/** 登录槽五态（G10 决策：现有 login_state 基础上拆分 stale）。 */
export type LoginSlotState =
  | "ok" // 正常（绿）：凭据实调通过
  | "expired" // 已过期（琥珀）：stale 可自动恢复
  | "relogin" // 需重登（红）：stale 不可自动恢复
  | "pending" // 待登录（琥珀）：凭据不可用
  | "signed_out"; // 未登录（灰）：从未保存凭据

/** 登录槽判定输入。 */
export interface LoginSlotInput {
  readonly login_state?: TraeInstanceLoginState | null;
  /** 失效不会自动恢复（旧通道凭据 / 续期被服务端拒绝）：提示改为重新登录。 */
  readonly relogin_only?: boolean;
}

/** 登录槽判定（纯函数）。 */
export function deriveLoginSlotState(input: LoginSlotInput): LoginSlotState {
  switch (input.login_state) {
    case "logged_in":
      return "ok";
    case "logged_out":
      return "pending";
    case "stale":
      return input.relogin_only ? "relogin" : "expired";
    default:
      // undefined / null / uninitialized：从未保存可验证的登录凭据。
      return "signed_out";
  }
}

/**
 * 槽位 2 · 登录凭据健康度（G10 五态）：调用方先用 deriveLoginSlotState 求枚举态。
 * archiveAvailable（登录存档存在，即切换的备用方式可用）按界面纪律
 * 收进悬浮提示，不占主视野。
 */
export function LoginArchiveSlotBadge({
  state,
  archiveAvailable = false,
}: {
  state: LoginSlotState;
  archiveAvailable?: boolean;
}) {
  const effective = state;
  // 存档次要信息（悬浮提示附注）：有存档 = 切换账号时存在备用方式。
  const archiveNote = archiveAvailable ? "；另保留有历史登录存档（切换账号的备用方式）" : "";
  switch (effective) {
    case "ok":
      // 正常：绿（确认正常——徽章说正常则切换账号直接可用）。
      return (
        <span
          className="slot-badge slot-badge--ok"
          title={`登录凭据验证可用，切换账号时直接可用${archiveNote}`}
        >
          <i className="slot-badge__dot" aria-hidden="true" />正常
        </span>
      );
    case "expired":
      // 已过期：琥珀（等待自动恢复——下次签到后续期写回）。
      return (
        <span
          className="slot-badge slot-badge--warn"
          title={`该账号的 TRAE 登录态已过期；登录态等待自动恢复（下次签到后续期写回）${archiveNote}`}
        >
          <i className="slot-badge__dot slot-badge__dot--warn" aria-hidden="true" />已过期
        </span>
      );
    case "relogin":
      // 需重登：红（不可自动恢复——旧通道凭据/续期被拒，只能重新登录）。
      return (
        <span
          className="slot-badge slot-badge--danger"
          title={`该账号的登录凭据已失效且无法自动恢复；重新登录一次即可更新凭据（对话记录不受影响）${archiveNote}`}
        >
          <i className="slot-badge__dot" aria-hidden="true" />需重登
        </span>
      );
    case "pending":
      // 待登录：琥珀（凭据不可用，重新登录一次即可保存）。
      return (
        <span
          className="slot-badge slot-badge--warn"
          title="该账号的登录凭据不可用；重新登录一次即可保存，供切换账号时使用"
        >
          <i className="slot-badge__dot slot-badge__dot--warn" aria-hidden="true" />待登录
        </span>
      );
    default:
      // 未登录：灰（从未保存凭据，无信息量占位）。
      return (
        <span
          className="slot-badge slot-badge--idle"
          title={
            archiveAvailable
              ? "该账号尚未保存可验证的登录凭据；但保留有历史登录存档，仍可用于切换账号"
              : "该账号尚未保存 TRAE 登录凭据"
          }
        >
          <i className="slot-badge__dot slot-badge__dot--hollow" aria-hidden="true" />未登录
        </span>
      );
  }
}

/**
 * 槽位 2 · 实例状态（复合态，环境页主库实例专用），停止态也透出登录子态（与健康检测汇总词表一致）：
 * - 停止：未启动（从未初始化）/ 登录有效 / 待登录（琥珀）/ 登录失效（琥珀加强）
 * - 运行：运行中 / 运行中·已登录 / 运行中·待登录（琥珀）/ 运行中·登录失效（琥珀加强）
 * 哲学不变："异常才亮色"——登录有效是正常态用中性色，待登录/失效需用户动手才亮琥珀；
 * 登录失效（stale）是持续性异常，停止态也亮琥珀加强。
 * loginState 为 undefined 表示从未初始化（无信息量），显示中性的「未启动」。
 */
export function InstanceSlotBadge({
  running,
  processState,
  loginState,
}: {
  running: boolean;
  processState?: EnvironmentProcessState;
  loginState?: TraeInstanceLoginState;
}) {  const isRunning = processState ? processState === "running" : running;
  if (processState === "unknown") {
    return (
      <span
        className="slot-badge slot-badge--warn"
        title="无法确认 TRAE 是否正在运行；为保护数据，实例操作暂不可用。"
      >
        <i className="slot-badge__dot slot-badge__dot--warn" aria-hidden="true" />状态未知
      </span>
    );
  }
  if (!isRunning) {
    if (loginState === "stale") {
      // 停止 + 登录失效：持续性异常，琥珀加强（U-7 保活后无需手动重登，等待自动恢复）。
      return (
        <span
          className="slot-badge slot-badge--warn slot-badge--warn-strong"
          title="该账号的 TRAE 登录态已失效（上次启动时服务端验证被拒）；登录态等待自动恢复（下次签到后续期写回，实例重启生效）"
        >
          <i className="slot-badge__dot slot-badge__dot--warn" aria-hidden="true" />登录失效
        </span>
      );
    }
    if (loginState === "logged_out") {
      // 停止 + 待登录：主库尚未注入登录态，提示走 App 切号/登录完成注入（琥珀）。
      return (
        <span
          className="slot-badge slot-badge--warn"
          title="主库尚未注入登录态；在账号页切换到该账号（或完成登录）即可注入，无需在 TRAE 窗口内手动登录"
        >
          <i className="slot-badge__dot slot-badge__dot--warn" aria-hidden="true" />待登录
        </span>
      );
    }
    if (loginState === "logged_in") {
      // 停止 + 登录有效：正常态，中性灰（登录凭据保留，下次启动免登录）。
      return (
        <span
          className="slot-badge slot-badge--idle"
          title="实例未运行；登录态保留，下次启动免登录"
        >
          <i className="slot-badge__dot slot-badge__dot--hollow" aria-hidden="true" />登录有效
        </span>
      );
    }
    return (
      <span className="slot-badge slot-badge--idle" title="主库实例尚未启动过">
        <i className="slot-badge__dot slot-badge__dot--hollow" aria-hidden="true" />未启动
      </span>
    );
  }
  if (loginState === "logged_out") {
    // 异常态：琥珀（需要用户在 TRAE 窗口内登录一次）。
    return (
      <span
        className="slot-badge slot-badge--warn"
        title="主库实例正在运行，但尚未登录；可在账号页切换到目标账号注入登录态，或在 TRAE 窗口内直接登录一次"
      >
        <i className="slot-badge__dot slot-badge__dot--warn" aria-hidden="true" />运行中 · 待登录
      </span>
    );
  }
  if (loginState === "stale") {
    // 异常加强：琥珀全饱和边框（登录键仍在但服务端已拒绝，U-7 保活将续期写回自动恢复）。
    return (
      <span
        className="slot-badge slot-badge--warn slot-badge--warn-strong"
        title="实例正在运行，但 TRAE 登录态已失效（服务端验证被拒）；登录态等待自动恢复（下次签到后续期写回，实例重启生效）"
      >
        <i className="slot-badge__dot slot-badge__dot--warn" aria-hidden="true" />运行中 · 登录失效
      </span>
    );
  }
  const loggedIn = loginState === "logged_in";
  return (
    <span
      className="slot-badge slot-badge--run"
      title={loggedIn ? "该账号的 TRAE 实例正在运行，登录态有效" : "该账号的 TRAE 实例正在运行"}
    >
      <i className="slot-badge__dot" aria-hidden="true" />
      {loggedIn ? "运行中 · 已登录" : "运行中"}
    </span>
  );
}

// ===== P18-B3 WorkBuddy 登录槽（G10 四色语义映射） =====

/** WorkBuddy 客户端登录状态（与 WorkBuddySessionHealthDto.status 同源）。 */
export type WorkBuddyHealthStatus =
  | "active"
  | "access_expired"
  | "fully_expired"
  | "logged_out"
  | "file_missing"
  | "unknown";

/**
 * WorkBuddy 客户端登录态 → G10 登录槽枚举：
 * active=正常（绿）；access_expired=已过期（琥珀，daemon 可自动刷新）；
 * fully_expired=需重登（红，双过期不可自动恢复）；logged_out=待登录（琥珀，
 * 已在客户端登出）；file_missing/unknown=未登录（灰，无信息量占位）。
 */
export function deriveWorkBuddyLoginSlotState(
  status: WorkBuddyHealthStatus,
): LoginSlotState {
  switch (status) {
    case "active":
      return "ok";
    case "access_expired":
      return "expired";
    case "fully_expired":
      return "relogin";
    case "logged_out":
      return "pending";
    default:
      return "signed_out";
  }
}

/** WorkBuddy 客户端登录徽章：复用 slot-badge 视觉规格，文案 WB 化。 */
export function WorkBuddyLoginSlotBadge({ status }: { status: WorkBuddyHealthStatus }) {
  const state = deriveWorkBuddyLoginSlotState(status);
  switch (state) {
    case "ok":
      return (
        <span className="slot-badge slot-badge--ok" title="WorkBuddy 客户端当前已登录且登录凭据有效">
          <i className="slot-badge__dot" aria-hidden="true" />已登录
        </span>
      );
    case "expired":
      return (
        <span
          className="slot-badge slot-badge--warn"
          title="登录凭据已过期；WorkBuddy 运行中会自动刷新，无需手动处理"
        >
          <i className="slot-badge__dot slot-badge__dot--warn" aria-hidden="true" />登录已过期
        </span>
      );
    case "relogin":
      return (
        <span
          className="slot-badge slot-badge--danger"
          title="登录凭据已完全失效；请在 WorkBuddy 客户端重新登录一次（对话记录不受影响）"
        >
          <i className="slot-badge__dot" aria-hidden="true" />登录已失效
        </span>
      );
    case "pending":
      return (
        <span
          className="slot-badge slot-badge--warn"
          title="WorkBuddy 客户端当前处于登出状态；在客户端登录后即可保存/切换"
        >
          <i className="slot-badge__dot slot-badge__dot--warn" aria-hidden="true" />已在客户端登出
        </span>
      );
    default:
      return (
        <span className="slot-badge slot-badge--idle" title="未检测到 WorkBuddy 登录记录">
          <i className="slot-badge__dot slot-badge__dot--hollow" aria-hidden="true" />未登录
        </span>
      );
  }
}

/**
 * WorkBuddy 账号池条目登录徽章判定（纯函数）：按池内凭据双过期时刻推导
 * G10 登录槽——refresh 过期=需重登（不可自动恢复）；仅 access 过期=已过期
 * （可续期）；双未知=待关注（琥珀）；其余=正常。
 */
export function deriveWorkBuddyPoolLoginState(input: {
  access_expires_at_unix_seconds: number;
  refresh_expires_at_unix_seconds: number;
  now_unix_seconds: number;
}): LoginSlotState {
  const { access_expires_at_unix_seconds, refresh_expires_at_unix_seconds, now_unix_seconds } = input;
  const refreshKnown = refresh_expires_at_unix_seconds > 0;
  const accessKnown = access_expires_at_unix_seconds > 0;
  if (refreshKnown && refresh_expires_at_unix_seconds <= now_unix_seconds) return "relogin";
  if (accessKnown && access_expires_at_unix_seconds <= now_unix_seconds) return "expired";
  if (!accessKnown && !refreshKnown) return "pending";
  return "ok";
}

/** WorkBuddy 账号池条目登录徽章（词与 Trae 登录槽一致，悬浮提示 WB 化）。 */
export function WorkBuddyPoolSlotBadge({ state }: { state: LoginSlotState }) {
  switch (state) {
    case "ok":
      return (
        <span className="slot-badge slot-badge--ok" title="该账号的登录凭据验证可用，切换账号时直接可用">
          <i className="slot-badge__dot" aria-hidden="true" />正常
        </span>
      );
    case "expired":
      return (
        <span
          className="slot-badge slot-badge--warn"
          title="该账号的登录凭据已过期；可用「刷新登录凭据」续期后继续使用"
        >
          <i className="slot-badge__dot slot-badge__dot--warn" aria-hidden="true" />已过期
        </span>
      );
    case "relogin":
      return (
        <span
          className="slot-badge slot-badge--danger"
          title="该账号的登录凭据已完全失效且无法自动续期；需在 WorkBuddy 客户端重新登录后重新保存"
        >
          <i className="slot-badge__dot" aria-hidden="true" />需重登
        </span>
      );
    case "pending":
      return (
        <span
          className="slot-badge slot-badge--warn"
          title="该账号的凭据有效期未知；建议用「刷新登录凭据」确认状态"
        >
          <i className="slot-badge__dot slot-badge__dot--warn" aria-hidden="true" />待确认
        </span>
      );
    default:
      return (
        <span className="slot-badge slot-badge--idle" title="该账号尚未保存登录凭据">
          <i className="slot-badge__dot slot-badge__dot--hollow" aria-hidden="true" />未登录
        </span>
      );
  }
}
