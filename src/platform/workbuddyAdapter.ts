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
import type {
  WorkBuddyCwdGroupDto,
  WorkBuddyLibraryCheckupDto,
  WorkBuddyLibraryStatsDto,
  WorkBuddySessionMessagesDto,
  WorkBuddySoftDeleteDto,
  WorkBuddyPurgeDto,
  WorkBuddyMergeCwdDto,
} from "../types/workbuddyLibrary";
import type {
  WorkBuddyLoginBeginDto,
  WorkBuddyLoginCompleteDto,
  WorkBuddyBackupChainDto,
  WorkBuddyBackupCreateDto,
  WorkBuddyBackupDeleteDto,
  WorkBuddyBackupRestoreDto,
  WorkBuddyProbeInfoDto,
} from "../types/workbuddyBackup";

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

// ===== P19-B3/B6 WB 对话库域（workbuddy_library 命令面；工具只读 + 单次确认写） =====

/** WB 对话库聚合统计（矩阵 #64：总览库统计卡 + 库页统计卡）。 */
export function fetchWorkBuddyLibraryStats(): Promise<WorkBuddyLibraryStatsDto> {
  return invoke<WorkBuddyLibraryStatsDto>("get_workbuddy_library_stats");
}

/**
 * 会话分组列表（矩阵 #63 视图面）：cwd 分组；userId=null 全库视图；
 * includeDeleted 控制软删行（常规浏览隐藏，体检/恢复视图可见）。
 */
export function fetchWorkBuddyLibraryHistory(
  userId: string | null,
  includeDeleted: boolean,
): Promise<readonly WorkBuddyCwdGroupDto[]> {
  return invoke<readonly WorkBuddyCwdGroupDto[]>("get_workbuddy_library_history", {
    userId,
    includeDeleted,
  });
}

/** 会话消息读取（矩阵 #66；jsonl 正文解析，正文缺失落 body_missing）。 */
export function fetchWorkBuddySessionMessages(
  sessionId: string,
  userId: string | null,
): Promise<WorkBuddySessionMessagesDto> {
  return invoke<WorkBuddySessionMessagesDto>("get_workbuddy_session_messages", {
    sessionId,
    userId,
  });
}

/** 库体检（矩阵 #62：孤儿正文/缺失正文/软删规模/外部目录/归并候选）。 */
export function fetchWorkBuddyLibraryCheckup(): Promise<WorkBuddyLibraryCheckupDto> {
  return invoke<WorkBuddyLibraryCheckupDto>("get_workbuddy_library_checkup");
}

/** 会话软删除（单次确认入口；UPDATE deleted_at 可逆；cloud/他人账号行自动跳过）。 */
export function deleteWorkBuddySessions(
  sessionIds: readonly string[],
  userId: string,
): Promise<WorkBuddySoftDeleteDto> {
  return invoke<WorkBuddySoftDeleteDto>("delete_workbuddy_sessions", {
    sessionIds: [...sessionIds],
    userId,
  });
}

/**
 * 会话彻底删除（决策 7 单独确认；备份前置 = 滚动单份，失败整单拒绝——
 * 不出现无备份的破坏性终点）。
 */
export function purgeWorkBuddySessions(
  sessionIds: readonly string[],
  userId: string,
): Promise<WorkBuddyPurgeDto> {
  return invoke<WorkBuddyPurgeDto>("purge_workbuddy_sessions", {
    sessionIds: [...sessionIds],
    userId,
  });
}

/** cwd 归并（单次确认；canonical 吸收 variants，同一 cwd 多转义形态收敛）。 */
export function mergeWorkBuddyCwdProjects(
  canonicalCwd: string,
  variantCwds: readonly string[],
): Promise<WorkBuddyMergeCwdDto> {
  return invoke<WorkBuddyMergeCwdDto>("merge_workbuddy_cwd_projects", {
    canonicalCwd,
    variantCwds: [...variantCwds],
  });
}

// ===== P19-B8 WB 备份链（be-engineer 契约：storage_root/workbuddy-backups 同域） =====

/** 备份链枚举（只读；最新在前，manifest 不可读条目如实呈现）。 */
export function fetchWorkBuddyBackupChain() {
  return invoke<WorkBuddyBackupChainDto>("list_workbuddy_backups");
}

/** 手动备份（单次确认入口；进程纪律：完全退出 → 写 → 重启）。 */
export function createWorkBuddyBackup() {
  return invoke<WorkBuddyBackupCreateDto>("create_workbuddy_backup");
}

/** 从备份恢复（单次确认入口；恢复前自动保链备份当前态，safety_backup_stamp 为回退点）。 */
export function restoreWorkBuddyBackup(stamp: number) {
  return invoke<WorkBuddyBackupRestoreDto>("restore_workbuddy_backup", { stamp });
}

/** 删除备份条目（单次确认；confirmed 必传 true，链外 stamp 落 missing_stamps）。 */
export function deleteWorkBuddyBackups(stamps: readonly number[]) {
  return invoke<WorkBuddyBackupDeleteDto>("delete_workbuddy_backups", {
    stamps: [...stamps],
    confirmed: true,
  });
}

/** 客户端探针信息（只读 fail-soft；环境页连接卡数据源）。 */
export function fetchWorkBuddyProbeInfo() {
  return invoke<WorkBuddyProbeInfoDto>("get_workbuddy_probe_info");
}

// ===== P19-B4 WB 登录编排（begin/complete/cancel；单槽 attempt） =====

/** 发起登录：后端自动拉起系统浏览器打开授权页；auth_url 同时返回（手动复制兜底）。 */
export function beginWorkBuddyLogin(): Promise<WorkBuddyLoginBeginDto> {
  return invoke<WorkBuddyLoginBeginDto>("begin_workbuddy_login");
}

/**
 * 等待授权完成（阻塞直到轮询命中，1s×5min）：acceptance 语义与 Trae 登录
 * 回执一致——failed 不回滚、前端如实展示、不自动重试、不发起第二次授权。
 */
export function completeWorkBuddyLogin(attemptId: number): Promise<WorkBuddyLoginCompleteDto> {
  return invoke<WorkBuddyLoginCompleteDto>("complete_workbuddy_login", { attemptId });
}

/** 取消等待（complete 会立即以 workbuddy_login_cancelled 返回）。 */
export function cancelWorkBuddyLogin(attemptId: number): Promise<null> {
  return invoke<null>("cancel_workbuddy_login", { attemptId });
}
