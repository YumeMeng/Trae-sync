import { invoke } from "@tauri-apps/api/core";
import type { TraeInstanceLoginState } from "../types/account_switch";
import type {
  EnvironmentDeletePreviewDto,
  EnvironmentListItemDto,
  EnvironmentLoginReceiptDto,
  EnvironmentRecordDto,
} from "../types/environment";
import type {
  MasterBackupChainDto,
  MasterCheckupDto,
  MasterEmptyProjectCleanupDto,
  MasterIncorporateResultDto,
  MasterRestoreResultDto,
  MasterSelfCheckDto,
} from "../types/masterLibrary";

/**
 * 环境域命令的类型化封装（P4d-1）：环境页（含主库/副环境、体检、收编清理、
 * 备份）只引用本文件，页面不出现命令名。本文件只做 invoke 转接——不实现
 * 业务编排、不吞错误（失败原样抛出，由调用方按 safeUiError 词表映射文案）、
 * 不改任何后端命令契约（参数名与 DTO 形状逐条对照现有调用点与
 * src-tauri/src 命令签名；Tauri 侧 snake_case 形参对应前端 camelCase 键）。
 */

/**
 * 环境状态与实例登录态读取沿用 accounts.ts 的同一封装（P4c-1a，命令参数已
 * 逐条对照），此处重导出保持环境域单一入口，不重复封装同一命令。
 */
export { readEnvironmentState, readTraeInstanceStates } from "./accounts";

/** 实例启动/聚焦回执（launch_master_library / launch_environment 返回）。 */
export interface EnvironmentLaunchReceipt {
  /** launched=本次新启动；focused=已在运行已带到前台；seeded=新启动并播种登录态。 */
  readonly outcome: "launched" | "focused" | "seeded";
  readonly login_state: TraeInstanceLoginState;
}

/** 关闭实例回执（close_environment_instance 返回）。 */
export interface EnvironmentCloseReceipt {
  /** closed=优雅关闭；force_closed=优雅未成已强制结束；not_running=本就未运行（幂等）。 */
  readonly outcome: "closed" | "force_closed" | "not_running";
}

/** 从客户端捕获账号的回执（capture_trae_cn_account 返回的 JSON 三字段）。 */
export interface CaptureTraeCnAccountReceipt {
  readonly profile_id: string;
  readonly account_id: string;
  readonly screen_name: string;
}

/* ============================================================
 * 环境列表 / 删除预览（只读）
 * ============================================================ */

/** 读取环境列表（主库置顶）；includeSize=true 附带体积，轮询传 false 沿用旧值。 */
export function listEnvironments(
  { includeSize }: { readonly includeSize: boolean },
): Promise<EnvironmentListItemDto[]> {
  return invoke<EnvironmentListItemDto[]>("list_environments", { includeSize });
}

/** 读取环境删除预览规模（status: ready/no_data/read_failed；读不到不许确认删除）。 */
export function readEnvironmentDeletePreview(
  { envId }: { readonly envId: string },
): Promise<EnvironmentDeletePreviewDto> {
  return invoke<EnvironmentDeletePreviewDto>("get_environment_delete_preview", { envId });
}

/* ============================================================
 * 体检 / 自检（只读聚合；轮询红线：5 秒轮询不得调用本组）
 * ============================================================ */

/** 读取库体检报告；libraryId 缺省=主库，副环境传自身 env_id（后端 Option，缺省不传键）。 */
export function readMasterCheckup(
  { libraryId }: { readonly libraryId?: string } = {},
): Promise<MasterCheckupDto> {
  return invoke<MasterCheckupDto>("get_master_checkup", libraryId ? { libraryId } : {});
}

/** 读取主库自检报告（读校验/一致性/可切换/深度四项 + can_repair 纠正入口）。 */
export function readMasterSelfCheck(): Promise<MasterSelfCheckDto> {
  return invoke<MasterSelfCheckDto>("get_master_self_check");
}

/** 按实测登录账号纠正主库工具记录（只改缓存，不动 TRAE 登录数据）；返回是否完成。 */
export function repairMasterCurrentAccount(): Promise<boolean> {
  return invoke<boolean>("repair_master_current_account");
}

/* ============================================================
 * 实例启停（无独立 restart 命令：重启 = 关闭 + 启动，或由收编/清理回执的
 * relaunch_outcome 报告自动重启结果）
 * ============================================================ */

/** 启动/聚焦主库实例（已运行则聚焦既有窗口，不重复开窗）。 */
export function launchMasterLibrary(): Promise<EnvironmentLaunchReceipt> {
  return invoke<EnvironmentLaunchReceipt>("launch_master_library");
}

/** 启动/聚焦副环境实例（--user-data-dir 指向环境目录；环境间并行、环境内单实例）。 */
export function launchEnvironment(
  { envId }: { readonly envId: string },
): Promise<EnvironmentLaunchReceipt> {
  return invoke<EnvironmentLaunchReceipt>("launch_environment", { envId });
}

/** 关闭环境实例（主库/副环境通用；未运行幂等返回 not_running，不报错）。 */
export function closeEnvironmentInstance(
  { envId }: { readonly envId: string },
): Promise<EnvironmentCloseReceipt> {
  return invoke<EnvironmentCloseReceipt>("close_environment_instance", { envId });
}

/* ============================================================
 * 环境生命周期（创建 / 改名 / 删除 / 环境内登录账号）
 * ============================================================ */

/** 创建副环境（空目录 + 档案登记；主库名保留，名称唯一）。 */
export function createEnvironment(
  { name }: { readonly name: string },
): Promise<EnvironmentRecordDto> {
  return invoke<EnvironmentRecordDto>("create_environment", { name });
}

/** 重命名副环境（主库不可改名；只改档案名，不动目录）。 */
export function renameEnvironment(
  { envId, name }: { readonly envId: string; readonly name: string },
): Promise<EnvironmentRecordDto> {
  return invoke<EnvironmentRecordDto>("rename_environment", { envId, name });
}

/** 删除副环境（档案行 + 数据目录；运行中由后端拒绝，确认由调用方单次完成）。 */
export function deleteEnvironment(
  { envId }: { readonly envId: string },
): Promise<boolean> {
  return invoke<boolean>("delete_environment", { envId });
}

/** 环境内切换登录账号（空环境首登=存档移植；否则凭据互换 + 记录交接）。 */
export function loginEnvironment(
  { envId, profileId }: { readonly envId: string; readonly profileId: string },
): Promise<EnvironmentLoginReceiptDto> {
  return invoke<EnvironmentLoginReceiptDto>("login_environment", { envId, profileId });
}

/* ============================================================
 * 收编 / 清理 / 账号捕获（破坏性；单次确认与进度事件由调用方负责）
 * ============================================================ */

/** 单事务收编库内滞留记录；libraryId 缺省=主库，副环境传 env_id（进度事件按 env_id 过滤）。 */
export function incorporateMasterRecords(
  { libraryId }: { readonly libraryId?: string } = {},
): Promise<MasterIncorporateResultDto> {
  return invoke<MasterIncorporateResultDto>(
    "incorporate_master_records",
    libraryId ? { libraryId } : {},
  );
}

/** 清理全库空项目记录（单事务；libraryId 缺省=主库，副环境传 env_id）。 */
export function cleanupMasterEmptyProjects(
  { libraryId }: { readonly libraryId?: string } = {},
): Promise<MasterEmptyProjectCleanupDto> {
  return invoke<MasterEmptyProjectCleanupDto>(
    "cleanup_master_empty_projects",
    libraryId ? { libraryId } : {},
  );
}

/** 从客户端登录态捕获账号入池（读登录态→在线复核→铸造；错误码原样抛出）。 */
export function captureTraeCnAccount(): Promise<CaptureTraeCnAccountReceipt> {
  return invoke<CaptureTraeCnAccountReceipt>("capture_trae_cn_account");
}

/* ============================================================
 * 备份（列表 / 手动备份 / 恢复；滚动单份语义由后端负责）
 * ============================================================ */

/** 读取备份链；libraryId 缺省=主库，副环境传 env_id（枚举该库作用域活动备份目录）。 */
export function readMasterBackupChain(
  { libraryId }: { readonly libraryId?: string } = {},
): Promise<MasterBackupChainDto> {
  return invoke<MasterBackupChainDto>("get_master_backup_chain", libraryId ? { libraryId } : {});
}

/** 手动创建主库备份（主库运行中由后端拒绝）；includeCredentials=连签到凭据子目录整拷。 */
export function createMasterBackup(
  { includeCredentials }: { readonly includeCredentials: boolean },
): Promise<string> {
  return invoke<string>("create_master_backup", { includeCredentials });
}

/** 从备份还原主库（关闭→校验→覆盖→体检复核→重启）；返回还原后体检与重启结果。 */
export function restoreMasterBackup(
  { stamp }: { readonly stamp: number },
): Promise<MasterRestoreResultDto> {
  return invoke<MasterRestoreResultDto>("restore_master_backup", { stamp });
}
