import {
  beginWorkBuddyLogin, cancelWorkBuddyLogin, captureWorkBuddyAccount, checkWorkBuddyLoginHealth,
  completeWorkBuddyLogin, refreshWorkBuddyAccount,
} from "../../platform/workbuddyAdapter";
import type { WorkBuddyDeepHealthDto } from "../../types/product";
import { safeUiErrorMessage } from "../../utils/safeUiError";
import { ACCOUNT_TEXTS } from "./config";
import type { PagePoolActions } from "./components/usePoolHeaderActions";

/**
 * 池级页头动作的命令装配（P4c-9b-1 / K-04）：WB 页头的「刷新登录凭据 / 健康检测 /
 * 保存当前登录账号 / 添加账号」四条接线。命令、文案、确认弹层、刷新时机逐字沿用旧
 * WorkBuddyPanel（L172-191 / L198-213 / L288-342 / L574-589）。
 *
 * 呈现纪律：这四个命令都只有池级 / 客户端级**单条回执**（无逐账号 healthy/error_code），
 * 故一律按旧实现的 notify.success/info/error 三档汇报，**不拆成逐账号摘要**。
 * WB 页头无「刷新额度 / 批量凭据」入口，故本表只登记池级四项。
 */

/** Unix 秒 → 本地「日期 时:分」；0 = 未知返回「未知」（旧 L53-62 + L328 同口径）。 */
const untilText = (unixSeconds: number): string => (unixSeconds
  ? new Date(unixSeconds * 1000).toLocaleString(undefined, {
    year: "numeric", month: "2-digit", day: "2-digit", hour: "2-digit", minute: "2-digit" })
  : "未知");

/** 深度健康实时面 → 展示文案（旧 L64-76 liveHealthLabel 逐字）。 */
function liveHealthLabel(liveStatus: WorkBuddyDeepHealthDto["live_status"]): string {
  switch (liveStatus) {
    case "verified":
      return "已验证可用（客户端运行中，凭据实时回读通过）";
    case "client_offline":
      return "客户端未运行，已完成本地检测";
    case "probe_failed":
      return "客户端运行中但凭据回读异常，建议重启客户端后重试";
    default:
      return "状态未知";
  }
}

/** 登录错误码 → 人话（旧 L78-90 workbuddyLoginErrorText 逐字，未知码回落 safeUiError 兜底）。 */
function loginErrorText(reason: unknown): string {
  if (typeof reason === "string") {
    switch (reason) {
      case "workbuddy_login_already_in_progress": return `${ACCOUNT_TEXTS.notifyPrefixWorkbuddy}已有登录正在进行（同一时间仅允许一个），请稍候再试。`;
      case "workbuddy_login_poll_timeout": return `${ACCOUNT_TEXTS.notifyPrefixWorkbuddy}等待授权超时（5 分钟），请重新发起登录。`;
      case "workbuddy_login_auth_url_untrusted": return `${ACCOUNT_TEXTS.notifyPrefixWorkbuddy}授权地址校验未通过，已中止（请检查网络环境）。`;
      case "workbuddy_real_mode_required": return `${ACCOUNT_TEXTS.notifyPrefixWorkbuddy}当前为演示模式，登录不可用。`;
      default: break;
    }
  }
  return `${ACCOUNT_TEXTS.notifyPrefixWorkbuddy}登录未完成——${safeUiErrorMessage(reason, "请重新发起登录。")}`;
}

/** WB 池级动作登记（动作 id 与 testid 逐字沿用旧实现）。 */
const workbuddyPoolActions: PagePoolActions = {
  // 旧 L324-342：renewed=false 即凭据仍充裕、本次未轮换（保护客户端副本），走中性提示。
  renew: {
    actionId: "workbuddy-renew-credentials",
    run: () => refreshWorkBuddyAccount(),
    copy: (receipt) => {
      const until = untilText(receipt.access_expires_at_unix_seconds);
      return receipt.renewed
        ? { tone: "success", text: `${ACCOUNT_TEXTS.notifyPrefixWorkbuddy}登录凭据已更新，有效期至 ${until}` }
        : { tone: "info", text: `${ACCOUNT_TEXTS.notifyPrefixWorkbuddy}登录凭据仍充裕（有效期至 ${until}），本次未刷新` };
    },
    failure: { prefix: `${ACCOUNT_TEXTS.notifyPrefixWorkbuddy}刷新登录凭据失败——`, fallback: "凭据续期未完成，请稍后重试。" },
  },
  // 旧 L172-191：只读零写副作用；verified=成功、probe_failed=异常、client_offline=中性。
  health: {
    actionId: "workbuddy-deep-health-button",
    run: () => checkWorkBuddyLoginHealth(),
    copy: (receipt) => {
      const label = liveHealthLabel(receipt.live_status as WorkBuddyDeepHealthDto["live_status"]);
      if (receipt.live_status === "verified") return { tone: "success", text: `${ACCOUNT_TEXTS.notifyPrefixWorkbuddy}健康检测通过——${label}` };
      if (receipt.live_status === "probe_failed") return { tone: "error", text: `${ACCOUNT_TEXTS.notifyPrefixWorkbuddy}健康检测发现异常——${label}` };
      return { tone: "info", text: `${ACCOUNT_TEXTS.notifyPrefixWorkbuddy}${label}` };
    },
    failure: { prefix: `${ACCOUNT_TEXTS.notifyPrefixWorkbuddy}健康检测未完成——`, fallback: "请稍后重试。" },
  },
  // 旧 L198-213 + L574-589：单次确认后执行；同一账号重复保存用最新凭据覆盖旧记录。
  capture: {
    actionId: "workbuddy-capture-button",
    run: () => captureWorkBuddyAccount(),
    confirm: {
      title: "保存当前 WorkBuddy 登录账号",
      lines: [
        "将从本机 WorkBuddy 客户端读取当前登录会话，加密保存到 Trae Sync 的账号池。",
        "只读取登录凭据，不会修改 WorkBuddy 客户端的任何数据。",
        "同一账号重复保存会用最新凭据覆盖旧记录。",
      ],
      confirmLabel: "保存账号",
      busyLabel: "保存中…",
      testId: "workbuddy-capture-dialog",
    },
    successCopy: (receipt) => `${ACCOUNT_TEXTS.notifyPrefixWorkbuddy}已保存账号「${receipt.screen_name || "WorkBuddy 账号"}」到账号池`,
    failure: { prefix: `${ACCOUNT_TEXTS.notifyPrefixWorkbuddy}保存失败——`, fallback: "账号保存未完成，请稍后重试。" },
  },
  // 旧 L288-318：begin（自动拉起系统浏览器）→ complete（阻塞等授权）→ 成功入池刷新；
  // acceptance=failed 只警告不回滚；取消以 workbuddy_login_cancelled 收尾走中性提示。
  login: {
    actionId: "workbuddy-add-account-button",
    cancelActionId: "workbuddy-login-cancel",
    commands: {
      begin: () => beginWorkBuddyLogin(),
      complete: (attemptId) => completeWorkBuddyLogin(attemptId),
      cancel: (attemptId) => cancelWorkBuddyLogin(attemptId),
    },
    successCopy: (receipt) => `${ACCOUNT_TEXTS.notifyPrefixWorkbuddy}账号「${receipt.screen_name}」登录成功，已保存到账号池。`,
    acceptanceWarning: (code) => `账号已保存，但可持续性验收未通过（${code ?? "未知原因"}）。`,
    acceptanceHint: "可稍后重新发起登录获取新凭据。",
    cancelCode: "workbuddy_login_cancelled",
    cancelCopy: "登录已取消；若浏览器未完成授权，可重新发起登录。",
    failureCopy: loginErrorText,
  },
};

/**
 * 导出给 pageAdapters 的产品接缝表登记（PAGE_ADAPTERS.workbuddy.poolActions）。
 * TRAE 页头的额度/凭据/健康检测是**逐账号批量**动作（actionCommands 已接），没有池级
 * 单条回执命令，故不登记——不把批量动作伪装成池级动作；未登记产品即 fail-closed。
 */
export { workbuddyPoolActions };
