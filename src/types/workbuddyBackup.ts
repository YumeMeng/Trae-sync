/**
 * P19-B8 WB 备份链域 DTO（list/create/restore/delete_workbuddy_backups +
 * get_workbuddy_probe_info 返回）。字段与 src-tauri/src/workbuddy_commands.rs
 * 后端契约（be-engineer ed422b4/c1a8495/7f03b80）一致。
 */

/** 备份链单条目（manifest 声明值；manifest 不可读时计数/字节为 null = 半成品目录如实呈现）。 */
export interface WorkBuddyBackupEntryDto {
  readonly stamp_unix_seconds: number;
  readonly file_count: number | null;
  readonly source_bytes: number | null;
  readonly compressed_bytes: number;
}

/** 备份链（目标目录 = storage_root/workbuddy-backups，与 purge 前置同域；最新在前）。 */
export interface WorkBuddyBackupChainDto {
  readonly backup_dir: string;
  readonly backups: readonly WorkBuddyBackupEntryDto[];
}

/** 手动备份回执（进程纪律：完全退出 → 写 → 重启，备份含 workbuddy.db）。 */
export interface WorkBuddyBackupCreateDto {
  readonly backup_dir: string;
  readonly stamp_unix_seconds: number;
  readonly file_count: number;
  readonly source_bytes: number;
  /** 滚动单份：写新份后清掉的旧份数（首份 0）。 */
  readonly rolled_over: number;
}

/** 恢复回执：safety_backup_stamp = 恢复前自动保链备份的时间戳（回退点，UI 必须展示）。 */
export interface WorkBuddyBackupRestoreDto {
  readonly restored_files: number;
  readonly restored_bytes: number;
  readonly safety_backup_stamp: number;
  readonly restored_stamp: number;
}

/** 删除回执（与 delete_master_backups 同构：链外 stamp 落 missing_stamps）。 */
export interface WorkBuddyBackupDeleteDto {
  readonly deleted_stamps: readonly number[];
  readonly missing_stamps: readonly number[];
  readonly remaining: number;
  readonly backup_dir: string;
}

/** 客户端探针信息（只读 fail-soft：全维度独立降级，客户端未运行 = running false + 其余 null）。 */
export interface WorkBuddyProbeInfoDto {
  readonly running: boolean | null;
  readonly process_count: number | null;
  readonly app: string | null;
  readonly version: string | null;
  readonly platform: string | null;
  readonly install_path: string | null;
  readonly install_source: "env_override" | "process_command_line" | "common_path" | "registry" | null;
  readonly data_root: string | null;
}

// ===== P19-B4 WB 登录编排（be-b4-impl 契约：begin/complete/cancel 三命令） =====

/** 发起登录回执：auth_url 已由后端拉起系统浏览器打开，同时返回供展示与手动复制兜底。 */
export interface WorkBuddyLoginBeginDto {
  readonly auth_url: string;
  readonly attempt_id: number;
}

/** 登录完成回执：acceptance 语义与 Trae 登录回执一致（failed 不回滚、不自动重试）。 */
export interface WorkBuddyLoginCompleteDto {
  readonly profile_id: string;
  readonly account_id: string;
  readonly screen_name: string;
  readonly acceptance: "passed" | "failed";
  readonly acceptance_error_code?: string;
}
