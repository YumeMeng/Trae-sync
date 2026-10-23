import { invoke } from "@tauri-apps/api/core";
import type { InstanceFingerprintDto } from "../types/account_switch";
import type {
  ArchiveDraftDto,
  MasterHistoryDto,
  MasterSessionMessagesDto,
  RelayLedgerEntryDto,
  TraeCnForeignProjectDto,
} from "../types/history";
import type { MasterLibraryStatsDto, MasterVerificationDto } from "../types/masterLibrary";
import type { ProductOperationSnapshotDto } from "../types/product";

/**
 * 库域命令的类型化封装（P4e-1 拆分件）：库页（对话列表 / 库信息 / 插件 / WB
 * 对话库）只引用本文件，页面不出现命令名。只做 invoke 直返——零 try/catch、
 * 零默认值、错误原样抛出（文案映射由调用方按 safeUiError 词表完成）；单次确认、
 * 备份前置、进程纪律与账号归属校验全在后端，本文件不承载编排。
 * 破坏性写命令在 ./libraryDelete，插件四命令在 ./libraryPlugins；同一命令的
 * 既有封装一律 re-export（见文末「复用」区块），不重复包装。
 */

/* ============================================================
 * 会话 / 项目树读取（只读；轮询指纹与切库代次守卫由调用方保管）
 * ============================================================ */

/** 读取库两栏历史（项目 + 会话 + 指纹）；指纹与 previous 相同返回 status=unchanged。 */
export function fetchMasterHistory({
  previous,
  previousCurrentUserId,
  libraryId,
}: {
  readonly previous: InstanceFingerprintDto | null;
  readonly previousCurrentUserId: string | null;
  readonly libraryId: string;
}): Promise<MasterHistoryDto> {
  return invoke<MasterHistoryDto>("get_master_history", {
    previous,
    previousCurrentUserId,
    libraryId,
  });
}

/** 读取单会话消息一页（offset 分页取更早消息；status 三态由调用方渲染）。 */
export function fetchMasterSessionMessages({
  sessionId,
  libraryId,
  offset,
}: {
  readonly sessionId: string;
  readonly libraryId: string;
  readonly offset?: number;
}): Promise<MasterSessionMessagesDto> {
  return invoke<MasterSessionMessagesDto>("get_master_session_messages", {
    sessionId,
    libraryId,
    ...(offset === undefined ? {} : { offset }),
  });
}

/** 读取接力台账（切号轨迹；按 session_id / from_session_id 链回）。 */
export function fetchRelayLedger({
  libraryId,
}: {
  readonly libraryId: string;
}): Promise<readonly RelayLedgerEntryDto[]> {
  return invoke<readonly RelayLedgerEntryDto[]>("get_relay_ledger", { libraryId });
}

/** 读取产品操作就绪快照（写能力门禁：state=ready 才放行对应入口）。 */
export function fetchProductOperationStates({
  productId,
  libraryId,
}: {
  readonly productId: string;
  readonly libraryId: string;
}): Promise<ProductOperationSnapshotDto> {
  return invoke<ProductOperationSnapshotDto>("get_product_operation_states", {
    productId,
    libraryId,
  });
}

/** 读取库聚合统计（status 四态 + 计数 + 体积）；libraryId 缺省=主库。 */
export function fetchMasterLibraryStats({
  libraryId,
}: {
  readonly libraryId?: string;
}): Promise<MasterLibraryStatsDto> {
  return invoke<MasterLibraryStatsDto>("get_master_library_stats", libraryId ? { libraryId } : {});
}

/** 读取备份对比校验报告；backupStamp=null 用最新备份，libraryId 缺省=主库。 */
export function fetchMasterVerification({
  backupStamp,
  libraryId,
}: {
  readonly backupStamp: number | null;
  readonly libraryId?: string;
}): Promise<MasterVerificationDto> {
  return invoke<MasterVerificationDto>("get_master_verification", {
    backupStamp,
    ...(libraryId ? { libraryId } : {}),
  });
}

/** 读取其他账号的可归并项目（Trae CN 只读库专属；归并入口数据源）。 */
export function fetchTraeCnForeignProjects({
  libraryId,
}: {
  readonly libraryId: string;
}): Promise<readonly TraeCnForeignProjectDto[]> {
  return invoke<readonly TraeCnForeignProjectDto[]>("list_trae_cn_foreign_projects", { libraryId });
}

/* ============================================================
 * 归档草稿（正本在 storage_root/archive-drafts.json，每库一键）
 * ============================================================ */

/** 读取库归档草稿（存储损坏 fail-closed 返回空草稿，不删文件）。 */
export function readArchiveDraft({
  libraryId,
}: {
  readonly libraryId: string;
}): Promise<ArchiveDraftDto> {
  return invoke<ArchiveDraftDto>("get_archive_draft", { libraryId });
}

/** 保存库归档草稿（空草稿即清除该库键；存储拒绝时返回稳定错误码）。 */
export function saveArchiveDraft({
  libraryId,
  archiveSessionIds,
  restoreSessionIds,
}: {
  readonly libraryId: string;
  readonly archiveSessionIds: readonly string[];
  readonly restoreSessionIds: readonly string[];
}): Promise<boolean> {
  return invoke<boolean>("set_archive_draft", {
    libraryId,
    archiveSessionIds: [...archiveSessionIds],
    restoreSessionIds: [...restoreSessionIds],
  });
}

/* ============================================================
 * 复用既有封装（同一命令不重复包装）
 * 环境域 4：主库状态 / 环境列表 / 启动主库 / 备份链
 * WB 库域 8：账号池 / 统计 / 分组 / 消息 / 体检 / 软删 / 彻底删 / cwd 归并
 * ============================================================ */

export {
  readEnvironmentState,
  listEnvironments,
  launchMasterLibrary,
  readMasterBackupChain,
} from "./environment";
export {
  deleteWorkBuddySessions,
  fetchWorkBuddyLibraryCheckup,
  fetchWorkBuddyLibraryHistory,
  fetchWorkBuddyLibraryStats,
  fetchWorkBuddySessionMessages,
  listWorkBuddyAccounts,
  mergeWorkBuddyCwdProjects,
  purgeWorkBuddySessions,
} from "./workbuddyAdapter";
