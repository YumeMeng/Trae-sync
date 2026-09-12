// P5-3 主库对话视图 DTO：项目/会话两栏 + 接力台账 + 主库消息预览。
// 与 Rust 后端 get_master_history / get_master_session_messages /
// get_relay_ledger 命令返回结构保持一致（snake_case wire 格式）。
// 旧 T03/T04 扫描/浏览类型已随 U-6 骨架退役（环境模型取代账号中心方案）。

import type {
  InstanceFingerprintDto,
  SessionMessageDto,
} from "./account_switch";

/** P5-3 主库历史状态（get_master_history.status）。 */
export type MasterHistoryStatus =
  | "ready" // 读取成功（projects/sessions 有效）
  | "unchanged" // 指纹与 previous 一致：维持前端现有列表（轮询预检）
  | "no_master_data" // 主库从未启动过（无 database.db）
  | "no_current_account" // 主库尚未登记登录账号
  | "read_failed"; // 打开或读取失败（key 不匹配、文件损坏等）

/** 左栏项目条目（正常项目 + 含全局归档内容的项目）。 */
export interface MasterProjectEntryDto {
  readonly project_id: string;
  /** 项目展示名；哈希/空名已回退路径尾段，仍不可读为空串（前端占位「未关联文件夹」）。 */
  readonly name: string;
  /** 项目文件夹绝对路径（悬浮提示展示）；缺失为 null。 */
  readonly absolute_path: string | null;
}

/** 右栏会话条目（chat_session 摘要 + project_id 联动筛选键）。 */
export interface MasterSessionEntryDto {
  readonly session_id: string;
  readonly project_id: string;
  readonly title: string;
  readonly message_count: number;
  /** unix 秒（源库毫秒已归一化）；缺失为 null。 */
  readonly updated_at_unix_seconds: number | null;
  readonly deleted: boolean;
  /**
   * TRAE 原生隐藏状态：`voice_discussion` 借用为归档（ADR-0022），
   * `scheduled_task` 为原生过滤值；null = 正常显示。
   * 主列表排除非空值（与 TRAE 侧栏白名单一致），归档视图收纳
   * `voice_discussion`。
   */
  readonly hidden_status: string | null;
  /** 会话模式（code/work，会话列优先回退项目列）；归档视图分层键。 */
  readonly work_mode: string | null;
}

/** get_master_history 返回（两栏数据源 + 轮询指纹）。 */
export interface MasterHistoryDto {
  readonly status: MasterHistoryStatus;
  /** 主库当前登录账号的 TRAE user_id（接力轨迹账号对齐用）；未登记为 null。 */
  readonly current_user_id: string | null;
  readonly projects: readonly MasterProjectEntryDto[];
  readonly sessions: readonly MasterSessionEntryDto[];
  /** 读取时刻的三件套 stat 指纹（前端保存为下一轮 previous）。 */
  readonly fingerprint: InstanceFingerprintDto;
}

/** P5-3 主库单会话消息读取状态。 */
export type MasterSessionMessagesStatus =
  | "ready"
  | "no_master_data"
  | "read_failed";

/** get_master_session_messages 返回（预览弹层数据源）。 */
export interface MasterSessionMessagesDto {
  readonly session_id: string;
  readonly status: MasterSessionMessagesStatus;
  readonly messages: readonly SessionMessageDto[];
  /** 当前页之后是否还有更早消息；查看器滚动到顶部时继续加载。 */
  readonly has_more: boolean;
}

/** merge_master_projects 返回（P5-8c 分组合并回执）。 */
export interface MasterMergeResultDto {
  /** 改挂到目标分组的会话数（含归档会话）。 */
  readonly moved_sessions: number;
  /** 被清理的空壳源分组行数。 */
  readonly removed_projects: number;
  /** 合并前自动创建的备份路径（人工恢复定位）。 */
  readonly backup_path: string;
}

/** apply_master_archive_changes 一次性提交归档草稿的回执。 */
export interface MasterArchiveApplyResultDto {
  readonly archived_sessions: number;
  readonly restored_sessions: number;
  /** launched/focused/failed；failed 表示数据库已提交但实例待恢复。 */
  readonly relaunch_outcome: string;
}

/** get_relay_ledger 逐条记录（前端按 session_id / from_session_id 链回轨迹）。 */
export interface RelayLedgerEntryDto {
  /** 新腿 session_id（换腿后身份）。 */
  readonly session_id: string;
  /** 旧腿 session_id（链回上一跳）；早期条目缺失为 null（轨迹在此自然断链）。 */
  readonly from_session_id: string | null;
  readonly project_id: string;
  /** 交接前账号的 TRAE user_id。 */
  readonly from_user_id: string;
  /** 交接前账号显示名（注册表反查）；账号已移除为 null。 */
  readonly from_account_name: string | null;
  /** 接收账号的 TRAE user_id。 */
  readonly to_user_id: string;
  readonly to_account_name: string | null;
  /** 交接时会话累计消息数（相邻条目差值 = 该腿新增）。 */
  readonly message_count_at_switch: number;
  readonly switched_at_unix_seconds: number;
}
