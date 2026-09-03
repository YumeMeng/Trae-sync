// 操作、长任务和存储根只读 DTO：字段与后端最小 command 契约保持结构化对齐。

/** 操作 manifest 的状态集合。 */
export type OperationStateDto =
  | "planned"
  | "backing_up"
  | "backup_verified"
  | "target_writing"
  | "target_committed_unverified"
  | "target_verifying"
  | "catalog_reconciling"
  | "verification_inconclusive"
  | "failure_preserving"
  | "failure_snapshot_verified"
  | "restore_staging"
  | "restore_staged"
  | "restore_replacing"
  | "restored_verifying"
  | "completed"
  | "cancelled_before_write"
  | "failed_safe"
  | "not_applied"
  | "restored_verified"
  | "manual_recovery_required";

/** 操作列表中的最小 DTO，不携带路径、密钥或数据库正文。 */
export interface OperationDto {
  readonly operation_id: string;
  readonly state: OperationStateDto;
  readonly data_location_id: string;
  readonly sequence: number;
  readonly has_verified_target_file_evidence: boolean;
}

/** 兼容既有组件命名；新契约统一称为 OperationDto。 */
export type OperationSummaryDto = OperationDto;

/** 未完成操作协调的白名单结果；不包含路径、哈希或备份内部引用。 */
export interface ReconcileUnfinishedOperationsDto {
  readonly inspected_count: number;
  readonly reconciled_count: number;
  readonly not_applied_count: number;
  readonly completed_count: number;
  readonly manual_recovery_required_count: number;
  readonly unrelated_data_location_count: number;
  readonly status:
    | "no_unfinished_operations"
    | "reconciled"
    | "manual_recovery_required"
    | "other_data_location_pending";
}

/** Tauri command 的稳定错误 DTO；不显示底层路径或原始错误正文。 */
export interface CommandErrorDto {
  readonly code: string;
  readonly message: string;
  readonly recommended_action: string;
  readonly retryable: boolean;
}

/** 当前目录库或数据位置锁状态。 */
export interface LockStatusDto {
  readonly data_location_id: string | null;
  readonly catalog_lock_held: boolean;
  readonly data_location_lock_held: boolean;
  readonly write_allowed: boolean;
  readonly reason: string | null;
}

/** 长任务进度快照；总量未知时不生成百分比。 */
export type ProgressPhaseDto =
  | "preparing"
  | "copying"
  | "hashing"
  | "writing"
  | "verifying"
  | "recovering"
  | "completed"
  | "failed";

export interface ProgressSnapshotDto {
  readonly operation_id: string;
  readonly phase: ProgressPhaseDto;
  readonly completed_bytes: number;
  readonly total_bytes: number | null;
  readonly percent_basis_points: number | null;
  readonly cancellable: boolean;
}

/** operation-stage 事件载荷。 */
export interface OperationStageEventDto {
  readonly operation_id: string;
  readonly phase: ProgressPhaseDto;
  readonly cancellable: boolean;
}

/** operation-progress 事件载荷，与持久化进度快照同构。 */
export type OperationProgressEventDto = ProgressSnapshotDto;

/** 同步执行结果；只表达状态和影响数量，不携带数据库正文。 */
export type SyncPlanExecutionOutcomeDto =
  | { readonly kind: "completed"; readonly affected_rows: number }
  | { readonly kind: "plan_expired"; readonly backups_preserved: boolean }
  | { readonly kind: "cancelled_before_write"; readonly backups_preserved: boolean }
  | { readonly kind: "failed_before_write"; readonly backups_preserved: boolean }
  | { readonly kind: "failed_after_write"; readonly backups_preserved: boolean }
  | { readonly kind: "manual_recovery_required"; readonly backups_preserved: boolean }
  | { readonly kind: "unsupported_plan" };

/** operation-needs-attention 事件载荷。 */
export interface OperationNeedsAttentionEventDto {
  readonly operation_id: string;
  readonly error: CommandErrorDto;
}

/** operation-finished 事件载荷。 */
export interface OperationFinishedEventDto {
  readonly operation_id: string;
  readonly outcome: SyncPlanExecutionOutcomeDto;
}

/** 存储根只读状态；路径仅用于用户核对，不提供迁移或写入入口。 */
export interface StorageRootStateDto {
  readonly configured: boolean;
  readonly available: boolean;
  readonly storage_root_id: string | null;
  readonly canonical_path: string | null;
  readonly catalog_id: string | null;
  readonly write_enabled: boolean;
  readonly used_bytes: number | null;
  readonly warning_threshold_bytes: number;
  readonly warning_active: boolean;
  readonly automatic_scan_paused: boolean;
  readonly reason: string | null;
}
