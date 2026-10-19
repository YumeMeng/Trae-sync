import { invoke } from "@tauri-apps/api/core";
import type {
  WorkBuddyCheckinClaimDto,
  WorkBuddyCheckinStatusDto,
  WorkBuddyLoginSummaryDto,
  WorkBuddyRefreshStatusDto,
  WorkBuddySessionHealthDto,
  WorkBuddySwitchDto,
} from "../types/product";

/**
 * WorkBuddy 域命令的前端封装（P17-T5 并列骨架）。
 *
 * 命令语义全部在组合根 workbuddy_commands.rs：本文件只做 invoke 转接，
 * 不承载任何协议逻辑（解密/签到/捕获编排）。所有命令带真实模式门禁
 * （workbuddy_real_mode_required）——演示/fixture 模式下 WorkBuddy 域
 * 不可用，错误文案经 safeUiError 词表映射（见 utils/safeUiError.ts）。
 */

/** 读取本机客户端登录会话健康（文件面只读）。 */
export function loadWorkBuddySessionHealth(): Promise<WorkBuddySessionHealthDto> {
  return invoke<WorkBuddySessionHealthDto>("get_workbuddy_session_health");
}

/** 读取工具账号池（workbuddy-login 容器脱敏清单）。 */
export function listWorkBuddyAccounts(): Promise<readonly WorkBuddyLoginSummaryDto[]> {
  return invoke<readonly WorkBuddyLoginSummaryDto[]>("list_workbuddy_accounts");
}

/** 查询直连签到状态（借客户端运行时解密，客户端可关可开）。 */
export function fetchWorkBuddyCheckinStatus(): Promise<WorkBuddyCheckinStatusDto> {
  return invoke<WorkBuddyCheckinStatusDto>("workbuddy_checkin_status");
}

/** 领取今日签到（幂等：已签返回 verified=true 且 credit=0）。 */
export function claimWorkBuddyCheckin(): Promise<WorkBuddyCheckinClaimDto> {
  return invoke<WorkBuddyCheckinClaimDto>("workbuddy_checkin_claim");
}

/**
 * 捕获当前客户端登录账号入工具账号池（单次确认入口：UI 弹窗确认后调用）。
 * 幂等可重跑：同账号再次捕获 = 整代次原子替换为最新。
 */
export function captureWorkBuddyAccount(): Promise<WorkBuddyLoginSummaryDto> {
  return invoke<WorkBuddyLoginSummaryDto>("capture_workbuddy_account");
}

/**
 * 手动续期账号池凭据（与 Trae「刷新登录凭据」同语义）：access 充裕时
 * 跳过（不轮换），临近过期自动刷新并原子写回新对。
 */
export function refreshWorkBuddyAccount(): Promise<WorkBuddyRefreshStatusDto> {
  return invoke<WorkBuddyRefreshStatusDto>("refresh_workbuddy_account");
}

/**
 * 切换 WorkBuddy 客户端登录账号（P17-T6/A4，单次确认入口：UI 弹窗确认后
 * 调用）。编排：客户端运行中 wbipc `auth:logout` → `auth:saveOauthToken`
 * （写池内最新代次）→ `auth:getAccount` 回读校验。对话存云端按账号隔离，
 * 切号=换登录态自动跟随。
 */
export function switchWorkBuddyAccount(profileId: string): Promise<WorkBuddySwitchDto> {
  return invoke<WorkBuddySwitchDto>("switch_workbuddy_account", { profileId });
}

// ===== P17-A5 WorkBuddy 自动签到调度 =====

/** WorkBuddy 自动签到状态（设置 + 今日台账 + 账号参与清单）。 */
export interface WorkBuddyAutoCheckinStatusDto {
  readonly enabled: boolean;
  readonly daily_time_hhmm: string;
  readonly ledger: {
    readonly date: string;
    readonly running: boolean;
    readonly total: number;
    readonly completed: number;
    readonly failed: number;
    readonly skipped: number;
  } | null;
  readonly accounts: readonly {
    readonly profile_id: string;
    readonly screen_name: string;
    readonly enabled: boolean;
  }[];
}

/** 读取 WorkBuddy 自动签到状态。 */
export function fetchWorkBuddyAutoCheckinStatus(): Promise<WorkBuddyAutoCheckinStatusDto> {
  return invoke<WorkBuddyAutoCheckinStatusDto>("get_workbuddy_auto_checkin_status");
}

/** 保存 WorkBuddy 自动签到设置（总开关 + 每日触发时间）。 */
export function setWorkBuddyAutoCheckinSettings(
  enabled: boolean,
  dailyTimeHhmm: string,
): Promise<void> {
  return invoke("set_workbuddy_auto_checkin_settings", {
    enabled,
    dailyTimeHhmm,
  });
}

/** 设置单个账号的自动签到参与状态。 */
export function setWorkBuddyAccountAutoCheckin(
  profileId: string,
  enabled: boolean,
): Promise<void> {
  return invoke("set_workbuddy_account_auto_checkin", { profileId, enabled });
}
