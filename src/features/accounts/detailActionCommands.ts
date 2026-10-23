import { resetCheckinDevice, removeCheckinAccount, refreshCheckinCredits, runAccountCheckin } from "../../platform/accounts";
import { purgeWorkBuddyAccounts } from "../../platform/workbuddyAdapter";
import type { CheckinBatchSummaryDto, CreditsRefreshEntryDto } from "../../types/account_switch";
import type { WorkBuddyAccountEntryDto } from "../../types/product";
import { effectiveDisplayName, workbuddyDisplayName } from "../../utils/accountDisplay";
import { ACCOUNT_TEXTS } from "./config";
import type { TraeAccountsData, WorkBuddyAccountsData } from "./components/useAccountsSnapshot";

/**
 * 详情动作的命令装配（P4c-6b-2e-1）：按产品给出 useDetailActions 需要的动作登记。
 *
 * 页面只查表、不判断产品；**没有对应命令的动作不登记**（fail-closed，不把单条回执拆成
 * 逐账号结果、不发明旧实现没有的文案）。命令与文案逐字沿用旧实现：
 * TRAE = AccountDetail L117-244 / L142-215；WB = WorkBuddyAccountDetail L131-157。
 * WB 详情当前没有「刷新额度 / 立即签到 / 重新登录 / 重置设备」四个入口（builder 不产出
 * 这些动作 id），故只登记删除一项；删除的二次确认由危险区 DetailDangerAction 承担
 * （ADR-0018 单次确认纪律，不在本表再登记一份确认文案）。
 */

/** 详情目标（按行 id 从本产品数据面解析）：screenName 供旧确认文案，accountId 供按 uid 的命令。 */
export interface DetailActionTarget {
  readonly profileId: string;
  readonly displayName: string;
  readonly screenName: string;
  readonly accountId: string | null;
}

/** 二次确认文案（逐字旧实现，按目标账号名生成）；弹层容器锚点 = 动作 id（新页同族约定）。 */
export interface DetailConfirmCopy {
  readonly title: string;
  readonly lines: readonly string[];
  readonly confirmLabel: string;
  readonly busyLabel: string;
  readonly danger: boolean;
  /** 失败提示前缀（旧 WB 带「WorkBuddy：」）；缺省无前缀。 */
  readonly failurePrefix?: string;
  /** 失败兜底文案；缺省 = 通用「操作未完成，请稍后重试。」 */
  readonly failureFallback?: string;
}

/** 删除成功回执（null = 旧实现无回执，如 TRAE 删除后直接回列表）。 */
export interface DetailRemoveReceipt {
  readonly tone: "success" | "warning";
  readonly text: string;
}

/** 单动作登记：id + 命令（缺省即该动作不注册，动作原样透出给观察者）。 */
export interface DetailRefreshCreditsAction {
  readonly id: string;
  readonly run: (profileId: string) => Promise<readonly CreditsRefreshEntryDto[]>;
}
export interface DetailCheckinAction {
  readonly id: string;
  readonly run: (profileId: string) => Promise<CheckinBatchSummaryDto>;
}
export interface DetailResetDeviceAction {
  readonly id: string;
  readonly run: (profileId: string) => Promise<unknown>;
  readonly confirm: (target: DetailActionTarget) => DetailConfirmCopy;
}
export interface DetailRemoveAction {
  readonly id: string;
  readonly run: (target: DetailActionTarget) => Promise<DetailRemoveReceipt | null>;
  /** 危险区二次确认文案（旧实现逐字的删除确认正文）；缺省 = 沿用危险区 hint。 */
  readonly confirm?: (target: DetailActionTarget) => DetailConfirmCopy;
}

/** 产品接缝：目标解析 + 五类动作登记（各自的 id 与命令成对给出）。 */
export interface PageDetailActions {
  readonly targetOf: (payload: unknown, rowId: string) => DetailActionTarget | null;
  readonly refreshCredits?: DetailRefreshCreditsAction;
  readonly checkin?: DetailCheckinAction;
  /** 重新登录 = 打开 6b-2c 登录弹层（旧 AccountCenterTrae L898 onRelogin=handleLogin(false)）。 */
  readonly relogin?: string;
  readonly resetDevice?: DetailResetDeviceAction;
  readonly remove?: DetailRemoveAction;
}

/** TRAE 目标（旧 AccountDetail 的 entry）：备注名优先，服务端名进确认文案。 */
const traeTarget = (payload: unknown, rowId: string): DetailActionTarget | null => {
  const entry = (payload as TraeAccountsData).overview.find((item) => item.profile_id === rowId);
  return entry === undefined ? null : {
    profileId: entry.profile_id, displayName: effectiveDisplayName(entry),
    screenName: entry.screen_name, accountId: null,
  };
};

/** WB 目标（旧 WorkBuddyAccountDetail 的 account）：命令按账号 uid，展示名走备注名优先。 */
const workbuddyTarget = (payload: unknown, rowId: string): DetailActionTarget | null => {
  const account: WorkBuddyAccountEntryDto | undefined =
    (payload as WorkBuddyAccountsData).pool.find((item) => item.profile_id === rowId);
  return account === undefined ? null : {
    profileId: account.profile_id, displayName: workbuddyDisplayName(account),
    screenName: account.screen_name, accountId: account.account_id,
  };
};

/** TRAE 五类动作（旧 testid 逐字）：额度 / 签到 / 重登 / 重置设备 / 删除。 */
const traeDetailActions = (): PageDetailActions => ({
  targetOf: traeTarget,
  refreshCredits: { id: "account-detail-refresh-credits", run: (profileId) => refreshCheckinCredits({ profileIds: [profileId] }) },
  checkin: { id: "account-detail-checkin", run: (profileId) => runAccountCheckin({ profileIds: [profileId] }) },
  relogin: "account-detail-relogin",
  resetDevice: {
    id: "account-detail-reset-device",
    run: (profileId) => resetCheckinDevice({ profileId }),
    // 旧 AccountDetail L163-172 逐字（容器锚点沿用旧 id，确认按钮即旧 -confirm-confirm）。
    confirm: (target) => ({
      title: "重置签到设备",
      lines: [
        `确定为账号“${target.screenName}”重置签到设备吗？`,
        "将通过服务端生成全新设备并更新登录信息；不影响账号与其他数据。",
      ],
      confirmLabel: "重置设备", busyLabel: "重置中…", danger: false,
    }),
  },
  remove: {
    id: "account-detail-remove",
    // 旧 L209-215：成功后重读缓存 + 回列表；旧实现无成功回执（回执 null 即不发明文案）。
    run: async (target) => { await removeCheckinAccount({ profileId: target.profileId }); return null; },
    // 旧 L173-183 逐字：删除确认正文比危险区 hint 更具体。
    confirm: (target) => ({
      title: "删除账号",
      lines: [
        `确定删除账号“${target.screenName}”吗？`,
        "将移除：账号档案、本机加密登录凭据、积分缓存。",
        "不影响其他账号与历史数据，删除后可通过重新登录找回。",
      ],
      confirmLabel: "删除", busyLabel: "删除中…", danger: true,
    }),
  },
});

/** WB 已登记动作：删除（旧 WorkBuddyAccountDetail L131-157 回执与文案逐字）。 */
const workbuddyDetailActions = (): PageDetailActions => ({
  targetOf: workbuddyTarget,
  remove: {
    id: "workbuddy-detail-purge",
    // 旧 L399-406 逐字：删除确认正文四行 + 「确认删除」。
    confirm: (target) => ({
      title: "删除 WorkBuddy 账号",
      lines: [
        `确定从工具账号池删除「${target.displayName}」吗？`,
        "将移除：工具侧账号池记录、账号档案备注（备注名/归档）。",
        "不会改动 WorkBuddy 客户端的登录状态（不登出、不碰客户端数据）。",
        "删除后该账号不可再切换登录或直连签到；重新保存或登录可恢复。",
      ],
      confirmLabel: "确认删除", busyLabel: "删除中…", danger: true,
      failurePrefix: ACCOUNT_TEXTS.notifyPrefixWorkbuddy,
      failureFallback: "账号数据未被改动，请稍后重试。",
    }),
    run: async (target) => {
      const receipt = await purgeWorkBuddyAccounts([target.accountId ?? target.profileId], true);
      if (receipt.profile_cleanup_failed.length > 0) {
        return { tone: "warning", text: "账号已从池中删除，但档案备注清理未完成（残留无害，可忽略）。" };
      }
      return { tone: "success", text: `${ACCOUNT_TEXTS.notifyPrefixWorkbuddy}账号「${target.displayName}」已删除（剩余 ${receipt.remaining_accounts} 个账号）。` };
    },
  },
});

/** 产品登记表：键 = productId，未登记即 null（页面给 fail-closed 接缝，动作只透出）。 */
const DETAIL_ACTIONS: Record<string, () => PageDetailActions> = {
  trae_cn: traeDetailActions,
  workbuddy: workbuddyDetailActions,
};

/** 取本产品的详情动作接缝；未登记产品 → null。 */
export function detailActionsFor(productId: string): PageDetailActions | null {
  return DETAIL_ACTIONS[productId]?.() ?? null;
}
