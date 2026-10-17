import { invoke } from "@tauri-apps/api/core";
import type {
  WorkBuddyCheckinClaimDto,
  WorkBuddyCheckinStatusDto,
  WorkBuddyLoginSummaryDto,
  WorkBuddySessionHealthDto,
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
