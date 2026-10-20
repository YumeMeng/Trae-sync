/**
 * P19-B3/B6 WB 对话库域 DTO（get_workbuddy_library_stats / history / messages /
 * checkup / delete / purge / merge 返回）。字段与 src-tauri/crates/infrastructure/
 * src/workbuddy_library.rs 及 src-tauri/src/workbuddy_library_commands.rs
 * （serde 命名）一致。
 */

/** WB 对话库聚合统计（总览库统计卡 + 库页统计卡数据源；矩阵 #64）。 */
export interface WorkBuddyLibraryStatsDto {
  readonly account_count: number;
  readonly session_count: number;
  /** 软删除会话数（deleted_at IS NOT NULL；默认视图隐藏）。 */
  readonly soft_deleted_session_count: number;
  /** 项目数（cwd 转义目录维度）。 */
  readonly project_count: number;
  /** 正文目录字节合计。 */
  readonly projects_dir_bytes: number;
  readonly last_active_unix_seconds: number | null;
}

/** 会话行（列表 DTO；不含正文内容）。 */
export interface WorkBuddySessionRowDto {
  readonly session_id: string;
  readonly title: string;
  /** 云同步标记（transport='cloud' 行只读展示，写操作红线不碰）。 */
  readonly transport: string;
  /** 是否软删除（默认视图隐藏）。 */
  readonly deleted: boolean;
  readonly last_active_unix_seconds: number | null;
}

/** cwd 分组（WB 的「项目」= cwd 转义目录维度；对齐 TRAE 项目分组 DTO 形状）。 */
export interface WorkBuddyCwdGroupDto {
  /** 原始 cwd（归并操作入口的稳定键）。 */
  readonly cwd: string;
  /** 转义目录名（projects/<escaped>）。 */
  readonly escaped_dir: string;
  readonly session_count: number;
  readonly last_active_unix_seconds: number | null;
  readonly sessions: readonly WorkBuddySessionRowDto[];
}

/** 单条消息（jsonl 正文解析面）。 */
export interface WorkBuddySessionMessageDto {
  readonly index: number;
  /** 消息种类（后端解析标注；未知 kind 原样透出）。 */
  readonly kind: string;
  readonly role: string | null;
  readonly timestamp_unix_ms: number | null;
  readonly text: string | null;
}

/** 会话消息读取回执（矩阵 #66）。 */
export interface WorkBuddySessionMessagesDto {
  readonly session_id: string;
  readonly messages: readonly WorkBuddySessionMessageDto[];
  /** 无法解析而跳过的正文行数（排障可见）。 */
  readonly skipped_lines: number;
  /** 正文文件缺失（索引有、正文无——体检视角的孤儿索引）。 */
  readonly body_missing: boolean;
}

/** WB 库体检报告（五项；矩阵 #62）。 */
export interface WorkBuddyLibraryCheckupDto {
  /** 孤儿正文（正文有、索引无）：相对路径清单。 */
  readonly orphan_body_files: readonly string[];
  readonly missing_body_count: number;
  readonly missing_body_samples: readonly string[];
  readonly soft_deleted_session_count: number;
  /** 外部目录（cwd 不在已知项目集）：转义目录名清单。 */
  readonly foreign_dirs: readonly string[];
  /** cwd 归并候选（同项目多转义形态分组；每组一组 [canonical, ...variants]）。 */
  readonly cwd_merge_candidates: readonly (readonly string[])[];
}

/** 破坏性写回执公共尾（进程纪律；UI 呈现「已重启客户端」）。 */
export interface WorkBuddyLibraryWriteTailDto {
  /** 本次是否执行了「关闭 → 重启」（写前客户端已在运行的场景为 true）。 */
  readonly restarted: boolean;
  /** 重启后客户端进程数（未重启/探测失败为 null）。 */
  readonly process_count_after_restart: number | null;
}

/** 软删除回执（UPDATE deleted_at；可逆）。 */
export interface WorkBuddySoftDeleteDto {
  readonly deleted_sessions: number;
  readonly skipped_ids: number;
  readonly tail: WorkBuddyLibraryWriteTailDto;
}

/** 彻底删除回执（决策 7 单独确认；备份前置失败整单拒绝）。 */
export interface WorkBuddyPurgeDto {
  readonly purged_sessions: number;
  readonly removed_body_files: number;
  readonly missing_body_files: number;
  readonly failed_body_files: readonly string[];
  /** 前置滚动备份目录（人工恢复定位）。 */
  readonly backup_path: string;
  readonly tail: WorkBuddyLibraryWriteTailDto;
}

/** cwd 归并回执（单次确认；canonical 吸收 variants）。 */
export interface WorkBuddyMergeCwdDto {
  readonly moved_sessions: number;
  readonly moved_body_files: number;
  readonly missing_body_files: number;
  readonly conflict_body_files: number;
  readonly tail: WorkBuddyLibraryWriteTailDto;
}
