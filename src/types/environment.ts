import type { TraeInstanceLoginState } from "./account_switch";

export type EnvironmentProcessState = "running" | "not_running" | "unknown";

/**
 * P5-0/P5-2 主库环境状态（get_environment_state 返回）。
 * 字段与 src-tauri/src/lib.rs 的 EnvironmentStateDto（serde 命名）一致。
 */
export interface EnvironmentStateDto {
  /** 环境标识；V1 恒为 "master"（主库）。 */
  readonly env_id: string;
  /** 环境档案记住的账号；身份不可实测时可能是最近一次确认的账号。 */
  readonly current_profile_id: string | null;
  /** 环境档案账号的显示名（备注名优先）；不单独证明实时登录。 */
  readonly current_account_name: string | null;
  /** 本次从 TRAE（融合版）登录材料实测并映射到档案的账号；null = 未能确认当前身份。 */
  readonly observed_profile_id: string | null;
  /** 主库 data_dir 绝对路径（展示与排障用）。 */
  readonly data_dir: string;
  /** 主库 TRAE 实例是否运行中（进程命令行含主库 data_dir 判定）。 */
  readonly running: boolean;
  /** 进程查询的三态结果；兼容字段 running 在 unknown 时为 false。 */
  readonly process_state?: EnvironmentProcessState;
  /** 主库实例登录态（与账号实例同口径）。 */
  readonly login_state: TraeInstanceLoginState;
  readonly created_at_unix_seconds: number;
}

/**
 * P6-4 环境列表项（list_environments 返回；环境页 V2 数据源）。
 * 字段与 lib.rs 的 EnvironmentListItemDto（serde 命名）一致。
 */
export interface EnvironmentListItemDto {
  readonly env_id: string;
  readonly name: string;
  /** 主库环境置顶且不可删除/改名。 */
  readonly is_master: boolean;
  readonly current_profile_id: string | null;
  readonly current_account_name: string | null;
  readonly data_dir: string;
  readonly running: boolean;
  /** 进程查询的三态结果；兼容字段 running 在 unknown 时为 false。 */
  readonly process_state?: EnvironmentProcessState;
  readonly login_state: TraeInstanceLoginState;
  readonly created_at_unix_seconds: number;
  /** 环境体积（include_size=true 时返回；轮询不带，保留旧值）。 */
  readonly size_bytes?: number;
}

/** 环境档案回执（创建/重命名命令返回）。 */
export interface EnvironmentRecordDto {
  readonly env_id: string;
  readonly name: string;
  readonly created_at_unix_seconds: number;
}

/** 环境删除预览（确认弹层数据源：列明将被删除的规模）。 */
export interface EnvironmentDeletePreviewDto {
  /** ready=规模读取成功；no_data=环境从未启动；read_failed=规模未知。 */
  readonly status: "ready" | "no_data" | "read_failed";
  readonly project_count: number;
  readonly session_count: number;
  readonly size_bytes: number;
}

/** 环境登录账号回执（login_environment 返回）。 */
export interface EnvironmentLoginReceiptDto {
  readonly env_id: string;
  readonly profile_id: string;
  /** true = 空环境首登（账号存档移植即登录）；false = 互换登录。 */
  readonly seeded: boolean;
  /** 随行归属的项目行数（单一归属泛化，ADR-0021）。 */
  readonly transferred_projects: number;
  readonly switched_sessions: number;
  readonly removed_mirror_rows: number;
  /** 环境档案是否已记住本次切换的账号。 */
  readonly current_profile_saved: boolean;
}
