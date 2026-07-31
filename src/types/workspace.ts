// 工作台状态 DTO：与 Rust 后端 `get_workspace_state` 命令返回的结构保持一致。
// T01 阶段所有“真实能力”均处于禁用状态，DTO 只表达空工作台的诚实状态。

/** 平台上下文：V1 仅保留 Work CN 边界，不实现 Adapter 行为 */
export interface PlatformContextDto {
  /** 平台标识，V1 固定为 "work_cn" */
  readonly platform_id: string;
  /** 平台显示名 */
  readonly display_name: string;
  /** Adapter 是否已实现真实行为（T01 阶段始终为 false） */
  readonly adapter_implemented: boolean;
}

/** 数据位置状态：T01 不发现真实位置 */
export interface DataLocationStateDto {
  /** 是否已选择数据位置 */
  readonly selected: boolean;
  /** 显示名（未选择时为 null） */
  readonly display_name: string | null;
  /** 不可写原因（未选择时为 "not_selected"） */
  readonly unavailable_reason: string | null;
}

/** 当前账号状态：T01 不读取真实账号证据 */
export interface CurrentAccountStateDto {
  /** 是否已检测到当前账号 */
  readonly detected: boolean;
  /** 用户 ID 不可逆指纹（未检测时为 null） */
  readonly user_fingerprint: string | null;
  /** 不可写原因（未检测时为 "not_detected"） */
  readonly unavailable_reason: string | null;
}

/** 历史库摘要：T01 始终为空 */
export interface HistorySummaryDto {
  readonly account_count: number;
  readonly project_count: number;
  readonly session_count: number;
}

/** 能力开关：T01 全部为 false，诚实表达“真实能力尚未启用” */
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
  /** 诚实状态文案：用于 UI 显示“真实能力尚未启用” */
  readonly honest_status: string;
}
