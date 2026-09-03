// 工作台状态 DTO：与 Rust 后端 `get_workspace_state` 命令返回的结构保持一致。
// 0.2.1 生产能力限于 RealReadPreview；真实写入、恢复和迁移开关保持关闭。

/** 平台上下文：V1 仅实现 Work CN Adapter 边界。 */
export interface PlatformContextDto {
  /** 平台标识，V1 固定为 "work_cn" */
  readonly platform_id: string;
  /** 平台显示名 */
  readonly display_name: string;
  /** Adapter 是否已开放当前运行模式对应的真实行为。 */
  readonly adapter_implemented: boolean;
}

/** 当前授权流程中的数据位置状态。 */
export interface DataLocationStateDto {
  /** 是否已选择数据位置 */
  readonly selected: boolean;
  /** 显示名（未选择时为 null） */
  readonly display_name: string | null;
  /** 不可写原因（未选择时为 "not_selected"） */
  readonly unavailable_reason: string | null;
}

/** 当前账号的非敏感验证状态。 */
export interface CurrentAccountStateDto {
  /** 是否已检测到当前账号 */
  readonly detected: boolean;
  /** 用户 ID 不可逆指纹（未检测时为 null） */
  readonly user_fingerprint: string | null;
  /** 不可写原因（未检测时为 "not_detected"） */
  readonly unavailable_reason: string | null;
}

/** 本地历史库摘要。 */
export interface HistorySummaryDto {
  readonly account_count: number;
  readonly project_count: number;
  readonly session_count: number;
}

/** 能力开关：生产候选只允许读取与计划预览相关能力。 */
export interface CapabilityFlagsDto {
  readonly scan_enabled: boolean;
  readonly sync_enabled: boolean;
  readonly backup_enabled: boolean;
  readonly restore_enabled: boolean;
}

/** 工作台状态聚合 */
export interface WorkspaceStateDto {
  readonly platform: PlatformContextDto;
  readonly data_location: DataLocationStateDto;
  readonly current_account: CurrentAccountStateDto;
  readonly history: HistorySummaryDto;
  readonly capabilities: CapabilityFlagsDto;
  /** 当前能力和阻断原因的诚实状态文案。 */
  readonly honest_status: string;
}
