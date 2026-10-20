import { invoke } from "@tauri-apps/api/core";
import type {
  WorkBuddyCheckinAccountStatusDto,
  WorkBuddyCheckinBatchItemDto,
  WorkBuddyCheckinClaimDto,
  WorkBuddyCheckinParticipationDto,
  WorkBuddyCheckinStatusDto,
  WorkBuddyDeepHealthDto,
  WorkBuddyLoginSummaryDto,
  WorkBuddyRefreshStatusDto,
  WorkBuddySessionHealthDto,
  WorkBuddySwitchDto,
  WorkBuddyTransferDto,
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

/**
 * 深度健康检测（P18-B3）：文件面登录态 + 客户端运行中时的 wbipc 只读
 * 会话验证（零写副作用；客户端未运行时 live_status=client_offline）。
 */
export function checkWorkBuddyLoginHealth(): Promise<WorkBuddyDeepHealthDto> {
  return invoke<WorkBuddyDeepHealthDto>("check_workbuddy_login_health");
}

/** 读取工具账号池（workbuddy-login 容器脱敏清单）。 */
export function listWorkBuddyAccounts(): Promise<readonly WorkBuddyLoginSummaryDto[]> {
  return invoke<readonly WorkBuddyLoginSummaryDto[]>("list_workbuddy_accounts");
}

/** 查询直连签到状态（借客户端运行时解密，客户端可关可开）。 */
export function fetchWorkBuddyCheckinStatus(): Promise<WorkBuddyCheckinStatusDto> {
  return invoke<WorkBuddyCheckinStatusDto>("workbuddy_checkin_status");
}

/**
 * 逐账号签到状态批查（P19-B5；矩阵 #43 聚合 + #48 单账号刷新）。
 * profileId=null = 全池批查（后端 fail-soft：单账号失败落条目 error，不阻塞他账号）；
 * 具体 id = 单账号刷新；不在池内 → 整体 Err workbuddy_auto_checkin_account_missing。
 */
export function fetchWorkBuddyCheckinOverview(
  profileId: string | null,
): Promise<readonly WorkBuddyCheckinAccountStatusDto[]> {
  return invoke<readonly WorkBuddyCheckinAccountStatusDto[]>("get_workbuddy_checkin_overview", {
    profileId,
  });
}

/** 参与集清单（矩阵 #46）：enabled = 有效参与（未显式设置默认 true；opt-out 永久尊重）。 */
export function listWorkBuddyCheckinAccounts(): Promise<readonly WorkBuddyCheckinParticipationDto[]> {
  return invoke<readonly WorkBuddyCheckinParticipationDto[]>("list_workbuddy_checkin_accounts");
}

/** 按账号领取今日签到（矩阵 #56；幂等：AlreadyCheckedIn → credit=0 verified=true）。 */
export function claimWorkBuddyCheckinProfile(profileId: string): Promise<WorkBuddyCheckinClaimDto> {
  return invoke<WorkBuddyCheckinClaimDto>("workbuddy_checkin_claim_profile", { profileId });
}

/**
 * 批量签到（矩阵 #51 一键签到）：后端顺序执行、fail-soft 逐条回执
 * （池外 id 落条目报错不整体拒绝）。批量不可中断，前端不提供取消。
 */
export function claimWorkBuddyCheckinBatch(
  profileIds: readonly string[],
): Promise<readonly WorkBuddyCheckinBatchItemDto[]> {
  return invoke<readonly WorkBuddyCheckinBatchItemDto[]>("workbuddy_checkin_claim_batch", {
    profileIds: [...profileIds],
  });
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
 * 调用）。切号只换客户端登录态；本地会话索引按账号隔离（workbuddy.db
 * sessions.user_id），切号后如看不到历史会话，用「转移历史会话」过户
 * 恢复（P18-B1）。
 */
export function switchWorkBuddyAccount(profileId: string): Promise<WorkBuddySwitchDto> {
  return invoke<WorkBuddySwitchDto>("switch_workbuddy_account", { profileId });
}

// ===== P18-B1 会话过户 =====

/**
 * 生成会话转移计划（dry_run，零副作用）：识别「上一个登录账号 → 当前登录
 * 账号」方向并统计待过户本地会话数；不关闭客户端、不写库。
 */
export function planWorkBuddySessionTransfer(): Promise<WorkBuddyTransferDto> {
  return invoke<WorkBuddyTransferDto>("transfer_workbuddy_sessions", { dryRun: true });
}

/**
 * 执行会话转移（单次确认入口：UI 弹窗确认后调用）。编排：完全退出
 * WorkBuddy（进程状态未知时拒绝写副作用）→ 事务内改写本地会话索引归属
 * （仅本地会话，不删除任何会话、不触碰云端会话）→ 正文抽验 → 重启客户端。
 */
export function transferWorkBuddySessions(): Promise<WorkBuddyTransferDto> {
  return invoke<WorkBuddyTransferDto>("transfer_workbuddy_sessions", { dryRun: false });
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
