/**
 * 两槽位徽章系统（设计契约 docs/DESIGN_TOKENS.md，2026-08-26 grill 确立）。
 *
 * 全局唯一状态语言：所有页面的账号状态徽章统一经过本模块，
 * 保证一处学会处处适用（账号卡片/列表行、签到页、总览、详情同义同色）。
 *
 * 哲学「异常才亮色」：正常状态全部中性安静（中性底 + 点）；
 * 琥珀 = 需要用户动手（待登录/令牌临期）；全画面暖色仅此一处时视觉独占。
 * 槽位永不消失（未知态显示灰占位），布局零跳动。
 */
import type { TraeInstanceLoginState } from "../types/account_switch";

/** 槽位 1 · 签到状态（三态固定）：已签 / 未签 / 状态未知。 */
export function CheckinSlotBadge({ checkedIn }: { checkedIn: boolean | null }) {
  if (checkedIn === true) {
    // 正常态：中性底 + 靛蓝点（55% 透明度，安静确认）。
    return (
      <span className="slot-badge slot-badge--done" title="该账号今日已完成签到">
        <i className="slot-badge__dot" aria-hidden="true" />已签
      </span>
    );
  }
  if (checkedIn === false) {
    return (
      <span className="slot-badge slot-badge--todo" title="该账号今日尚未签到">
        <i className="slot-badge__dot slot-badge__dot--hollow" aria-hidden="true" />未签
      </span>
    );
  }
  // 未知态占位：保持槽位稳定（缓存刷新后即翻转）。
  return (
    <span className="slot-badge slot-badge--unknown" title="今日签到状态未知（刷新额度后显示）">
      <i className="slot-badge__dot slot-badge__dot--muted" aria-hidden="true" />状态未知
    </span>
  );
}

/**
 * 槽位 2 · 登录凭据健康度（P7-5 凭据包实调判定，账号卡片专用）：
 * - 登录有效（中性）/ 待登录（琥珀）/ 登录失效（琥珀加强）/ 未登录（中性占位）
 * 主信息为凭据包 token 实调 GetUserInfo 的结果（徽章说有效则切号必通），
 * 断网时降级为登录存档本地证据；archiveAvailable（登录存档存在，即切换的
 * 备用方式可用）按界面纪律收进悬浮提示，不占主视野。
 * loginState 为 undefined 表示从未初始化（无信息量），显示中性的「未登录」。
 */
export function LoginArchiveSlotBadge({
  loginState,
  archiveAvailable = false,
  reloginOnly = false,
}: {
  loginState?: TraeInstanceLoginState;
  archiveAvailable?: boolean;
  /** 失效原因不会自动恢复（旧通道凭据/续期被拒）：提示改为重新登录。 */
  reloginOnly?: boolean;
}) {
  // 存档次要信息（悬浮提示附注）：有存档 = 切换账号时存在备用方式。
  const archiveNote = archiveAvailable ? "；另保留有历史登录存档（切换账号的备用方式）" : "";
  if (loginState === "stale") {
    // 登录失效：持续性异常，琥珀加强。两类恢复路径：
    // 常规失效等待自动恢复（下次签到后续期写回）；不会自动恢复的
    // （旧通道凭据/续期被服务端拒绝）只能重新登录，提示直接指向该动作。
    const title = reloginOnly
      ? `该账号的登录凭据已失效且无法自动恢复；重新登录一次即可更新凭据（对话记录不受影响）${archiveNote}`
      : `该账号的 TRAE 登录态已失效；登录态等待自动恢复（下次签到后续期写回）${archiveNote}`;
    return (
      <span
        className="slot-badge slot-badge--warn slot-badge--warn-strong"
        title={title}
      >
        <i className="slot-badge__dot slot-badge__dot--warn" aria-hidden="true" />登录失效
      </span>
    );
  }
  if (loginState === "logged_out") {
    // 待登录：凭据不可用，需要重新登录，琥珀。
    return (
      <span
        className="slot-badge slot-badge--warn"
        title="该账号的登录凭据不可用；重新登录一次即可保存，供切换账号时使用"
      >
        <i className="slot-badge__dot slot-badge__dot--warn" aria-hidden="true" />待登录
      </span>
    );
  }
  if (loginState === "logged_in") {
    // 登录有效：正常态，中性灰（凭据实调通过，切换账号时直接可用）。
    return (
      <span
        className="slot-badge slot-badge--idle"
        title={`登录凭据验证可用，切换账号时直接可用${archiveNote}`}
      >
        <i className="slot-badge__dot slot-badge__dot--hollow" aria-hidden="true" />登录有效
      </span>
    );
  }
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
  loginState,
}: {
  running: boolean;
  loginState?: TraeInstanceLoginState;
}) {
  if (!running) {
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

/**
 * 令牌健康文字化（降级为 meta，不再是徽章）：
 * 返回「令牌 N 天」；null 表示凭据不可读；<=7 天整段文字转琥珀（异常才亮色）。
 */
export function tokenHealthText(accessExpiresAt: number | null): {
  text: string;
  warn: boolean;
  title: string;
} {
  if (accessExpiresAt === null) {
    return { text: "令牌未知", warn: false, title: "未能读取本机凭据包" };
  }
  const remainingDays = Math.ceil((accessExpiresAt - Date.now() / 1000) / 86400);
  if (remainingDays <= 0) {
    return { text: "登录已过期", warn: true, title: "访问令牌已过期，请重新登录" };
  }
  return {
    text: `令牌 ${remainingDays} 天`,
    warn: remainingDays <= 7,
    title: remainingDays <= 7 ? "访问令牌临近过期，将自动续期；若失败请重新登录" : "访问令牌剩余天数（到期前自动续期）",
  };
}
