// 历史库 DTO：与 Rust 后端 T03/T04 命令返回结构保持一致。
// 所有 newtype 使用 transparent serde，enum 使用 snake_case，与 Rust 约定对齐。

/** 系统时间序列化形态（Rust SystemTime 的 serde_json 默认） */
export interface SystemTimeDto {
  readonly secs_since_epoch: number;
  readonly nanos_since_epoch: number;
}

/** 扫描失败原因：结构化，不携带 secret */
export type ScanFailureReason =
  | "not_authorized"
  | "process_running"
  | "database_missing"
  | "schema_incompatible"
  | "storage_root_unavailable"
  | "source_set_drift"
  | "catalog_transaction_failed"
  | "catalog_key_missing";

/** 快照文件种类 */
export type SnapshotFileKind = "db" | "wal" | "shm";

/** 文件身份：volume + file index */
export interface FileIdentityDto {
  readonly volume_serial: number;
  readonly file_index_high: number;
  readonly file_index_low: number;
}

/** 单个快照文件捕获信息 */
export interface SnapshotFileEntryDto {
  readonly kind: SnapshotFileKind;
  readonly relative_path: string;
  readonly present: boolean;
  readonly size: number;
  readonly sha256: string;
  readonly file_identity: FileIdentityDto | null;
}

/** 来源快照元数据 */
export interface SourceSnapshotMetaDto {
  readonly snapshot_id: string;
  readonly platform_id: string;
  readonly data_location_id: string;
  readonly product_version: string;
  readonly schema_fingerprint: string;
  readonly mapping_version: string;
  readonly account_evidence_ref: string | null;
  readonly captured_at: SystemTimeDto;
  readonly files: readonly SnapshotFileEntryDto[];
  readonly fingerprint: string;
}

/** 扫描结果：tagged union，kind 字段区分 */
export type ScanOutcomeDto =
  | {
      readonly kind: "success";
      readonly snapshot_id: string;
      readonly snapshot_meta: SourceSnapshotMetaDto;
      readonly catalog_updated: boolean;
    }
  | {
      readonly kind: "deduplicated";
      readonly existing_snapshot_id: string;
      readonly fingerprint: string;
    }
  | {
      readonly kind: "failed";
      readonly reason: ScanFailureReason;
    };

/** 会话身份：(product_history_namespace, original_session_id) */
export interface SessionIdentityDto {
  readonly product_history_namespace: string;
  readonly original_session_id: string;
}

/** 版本分类：Gate I 四类 */
export type VersionClassification =
  | "identical"
  | "fast_forward"
  | "forked"
  | "unclassified";

/** 浏览账号节点 */
export interface BrowseAccountNodeDto {
  readonly user_id: string;
  readonly display_label: string;
  readonly project_count: number;
  readonly session_count: number;
}

/** 浏览项目节点 */
export interface BrowseProjectNodeDto {
  readonly project_id: string;
  readonly display_name: string;
  /** 显示归属（user_assigned 优先，否则 first_observed_owner） */
  readonly display_owner: string;
  readonly session_count: number;
}

/** 浏览会话节点 */
export interface BrowseSessionNodeDto {
  readonly session_identity: SessionIdentityDto;
  readonly title: string;
  readonly message_count: number;
  readonly last_captured_at: SystemTimeDto;
  /** 所属项目 ID：用于前端按选中项目筛选会话（方案 D 三级层次） */
  readonly project_id: string;
}

/** 历史浏览摘要：普通统计只计算可见项（Gate J） */
export interface HistoryBrowseSummaryDto {
  readonly visible_account_count: number;
  readonly visible_project_count: number;
  readonly visible_session_count: number;
  readonly soft_deleted_project_count: number;
  readonly soft_deleted_session_count: number;
  readonly soft_deleted_message_count: number;
}

/** 浏览结果：账号树 + 项目 + 会话 + 摘要 */
export interface BrowseResultDto {
  readonly accounts: readonly BrowseAccountNodeDto[];
  readonly projects: readonly BrowseProjectNodeDto[];
  readonly sessions: readonly BrowseSessionNodeDto[];
  readonly summary: HistoryBrowseSummaryDto;
}

/** 消息投影 */
export interface MessageProjectionDto {
  readonly message_id: string;
  readonly session_id: string;
  readonly role: string;
  readonly content_excerpt: string;
  readonly soft_deleted: boolean;
  readonly seq: number;
}

/** 对话预览：完整对话的消息序列 */
export interface ConversationPreviewDto {
  readonly session_identity: SessionIdentityDto;
  readonly title: string;
  readonly messages: readonly MessageProjectionDto[];
  readonly total_message_count: number;
}

/** 搜索命中 */
export interface SearchHitDto {
  readonly session_identity: SessionIdentityDto;
  readonly message_id: string;
  readonly project_id: string;
  readonly title: string;
  readonly content_excerpt: string;
  readonly role: string;
}

/** TRAE 进程运行状态 */
export type ProcessRunningState = "unknown" | "not_running" | "running";

/** T05 同步范围：全部历史或用户明确选择的稳定 ID 并集 */
export type SyncScopeDto =
  | { readonly kind: "all_history" }
  | {
      readonly kind: "custom";
      readonly account_ids: readonly string[];
      readonly project_ids: readonly string[];
      readonly session_ids: readonly SessionIdentityDto[];
    };

export type PlanActionDto =
  | {
      readonly kind: "follow_project";
      readonly project_id: string;
      readonly from_user_id: string;
      readonly to_user_id: string;
    }
  | {
      readonly kind: "attach_sessions";
      readonly source_project_id: string;
      readonly target_project_id: string;
      readonly session_ids: readonly SessionIdentityDto[];
    };

export type PlanExclusionReason =
  | "already_current"
  | "project_identity_conflict"
  | "project_identity_unknown"
  | "archived_only"
  | "deleted_project"
  | "schema_incompatible"
  | "session_version_unavailable"
  | "partial_project_requires_target";

export interface PlanExclusionDto {
  readonly project_id: string;
  readonly session_id: SessionIdentityDto | null;
  readonly reason: PlanExclusionReason;
}

/** T05 只读计划 DTO；执行能力由 T06/T07 提供。 */
export interface SyncPlanDto {
  readonly operation_id: string;
  readonly current_user_id: string;
  readonly scope_snapshot: SyncScopeDto;
  readonly actions: readonly PlanActionDto[];
  readonly exclusions: readonly PlanExclusionDto[];
}
