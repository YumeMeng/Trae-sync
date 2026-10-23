import { invoke } from "@tauri-apps/api/core";
import type {
  MasterArchiveApplyResultDto,
  MasterMergeResultDto,
  TraeCnMergePreviewDto,
  TraeCnMergeResultDto,
} from "../types/history";

/**
 * 库域破坏性写命令的类型化封装（P4e-1 拆分件，自 ./library 拆出）：
 * 删除 / 归档应用 / 项目合并 / Trae CN 归并。只做 invoke 直返——零 try/catch、
 * 零默认值、错误原样抛出。单次确认、备份前置（失败整单拒绝）、关实例与重启
 * 纪律、账号归属校验全在后端，本文件不承载编排。
 */

/** delete_master_sessions 回执（字段与 Rust MasterDeleteResultDto 一致）。 */
export interface MasterDeleteResultDto {
  readonly deleted_sessions: number;
  /** 随会话删除的消息行数（元数据 + 内容三表）。 */
  readonly deleted_messages: number;
  /** 删除后变空壳而被清理的项目行数。 */
  readonly removed_projects: number;
  /** 删除前自动创建的备份路径（人工恢复定位）。 */
  readonly backup_path: string;
}

/** 真实删除会话（备份先行、实例运行中拒绝；单事务删消息三表 + 会话 + 空壳项目）。 */
export function deleteMasterSessions({
  sessionIds,
  libraryId,
}: {
  readonly sessionIds: readonly string[];
  readonly libraryId: string;
}): Promise<MasterDeleteResultDto> {
  return invoke<MasterDeleteResultDto>("delete_master_sessions", {
    sessionIds: [...sessionIds],
    libraryId,
  });
}

/** 一次性提交归档草稿（关实例 → 单事务 → 按库重启；relaunch_outcome 如实上报）。 */
export function applyMasterArchiveChanges({
  archiveSessionIds,
  restoreSessionIds,
  libraryId,
}: {
  readonly archiveSessionIds: readonly string[];
  readonly restoreSessionIds: readonly string[];
  readonly libraryId: string;
}): Promise<MasterArchiveApplyResultDto> {
  return invoke<MasterArchiveApplyResultDto>("apply_master_archive_changes", {
    archiveSessionIds: [...archiveSessionIds],
    restoreSessionIds: [...restoreSessionIds],
    libraryId,
  });
}

/** 合并项目分组（其余项目会话改挂保留项目；后端先备份再单事务）。 */
export function mergeMasterProjects({
  sourceProjectIds,
  targetProjectId,
  libraryId,
}: {
  readonly sourceProjectIds: readonly string[];
  readonly targetProjectId: string;
  readonly libraryId: string;
}): Promise<MasterMergeResultDto> {
  return invoke<MasterMergeResultDto>("merge_master_projects", {
    sourceProjectIds: [...sourceProjectIds],
    targetProjectId,
    libraryId,
  });
}

/** 归并规模预览（只读；同目录各账号行、会话数与消息数）。 */
export function previewTraeCnProjectMerge({
  projectId,
  libraryId,
}: {
  readonly projectId: string;
  readonly libraryId: string;
}): Promise<TraeCnMergePreviewDto> {
  return invoke<TraeCnMergePreviewDto>("preview_trae_cn_project_merge", { projectId, libraryId });
}

/** 归并到当前账号（备份先行 + 单事务 + 台账；进度走 trae-cn-merge-progress 事件）。 */
export function mergeTraeCnProject({
  projectId,
  libraryId,
}: {
  readonly projectId: string;
  readonly libraryId: string;
}): Promise<TraeCnMergeResultDto> {
  return invoke<TraeCnMergeResultDto>("merge_trae_cn_project", { projectId, libraryId });
}
