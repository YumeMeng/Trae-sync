import { invoke } from "@tauri-apps/api/core";
import type { AutoCheckinStatusDto } from "../types/account_switch";
import type { BackupMigrationDto, BackupRetentionDto } from "../types/masterLibrary";

/**
 * 设置页命令的类型化封装（P4f-1）：features/settings 只引用本文件，页面与 hook 不出现命令名
 * （ui-guard 规则 5：src/features 内禁止 invoke(）。本文件只做 invoke 转接——不实现编排、
 * 不吞错误（失败原样抛出，由调用方按 safeUiError 词表映射文案）、不改后端命令契约。
 *
 * 6 条新封装的真实调用点（2026-10-24 grep 实测）：
 *   get_auto_checkin_settings / set_auto_checkin_settings → components/settings/TraeSettingsVariant.tsx L108 / L247
 *   get_backup_retention / set_backup_retention          → 同上 L98 / L200
 *   migrate_master_backups / set_backup_dir              → 同上 L162 / L178
 * 参数名逐条对照后端签名（checkin_schedule.rs L829 / L852，library_archive.rs L2341 / L2362 /
 * L2407 / L2452）：enabled / dailyTimeHhmm / keep / targetDir / backupDir。
 *
 * 备份链相关只封装命令、不写编排（P4f-design §4 风险点 1–3）：滚动单份语义在后端 create 内，
 * 本层不判断保留策略、不串接"迁移→切换"顺序（顺序约束由调用方 hook 负责）。
 */

/* ============================================================
 * 自动签到调度（TRAE 设置页；WB 的同名命令走 workbuddyAdapter 既有封装）
 * ============================================================ */

/** 读取自动签到设置与今日台账（get_auto_checkin_settings；设置页与签到页状态行共用）。 */
export function readAutoCheckinStatus(): Promise<AutoCheckinStatusDto> {
  return invoke<AutoCheckinStatusDto>("get_auto_checkin_settings");
}

/** 保存自动签到设置（总开关 + 每日触发时间；后端原子写并保留台账，时间格式非法由后端拒绝）。 */
export function saveAutoCheckinSettings(
  { enabled, dailyTimeHhmm }: { readonly enabled: boolean; readonly dailyTimeHhmm: string },
): Promise<void> {
  return invoke<void>("set_auto_checkin_settings", { enabled, dailyTimeHhmm });
}

/* ============================================================
 * 备份保留与备份位置（TRAE 设置页；WB 无此两项能力，见 SETTINGS_CAPABILITIES）
 * ============================================================ */

/** 读取备份保留设置（get_backup_retention；keep 仅为后端契约兼容，语义已退役为滚动单份）。 */
export function readBackupRetention(): Promise<BackupRetentionDto> {
  return invoke<BackupRetentionDto>("get_backup_retention");
}

/** 保存备份保留设置（set_backup_retention；enabled=自动清理开关，keep 原样透传不解释）。 */
export function saveBackupRetention(
  { enabled, keep }: { readonly enabled: boolean; readonly keep: number },
): Promise<void> {
  return invoke<void>("set_backup_retention", { enabled, keep });
}

/** 把现有备份复制到新目录（migrate_master_backups；逐文件校验、原位保留；须在 set_backup_dir 之前调用）。 */
export function migrateMasterBackups(
  { targetDir }: { readonly targetDir: string },
): Promise<BackupMigrationDto> {
  return invoke<BackupMigrationDto>("migrate_master_backups", { targetDir });
}

/** 设置备份位置（set_backup_dir；null=恢复默认主库同目录，既有备份不移动）。 */
export function setBackupDir(
  { backupDir }: { readonly backupDir: string | null },
): Promise<void> {
  return invoke<void>("set_backup_dir", { backupDir });
}

/* ============================================================
 * 复用既有封装（同一命令不重复封装；备份链四件套/三件套与签到参与集均已在 platform 层）
 * ============================================================ */

/** TRAE 备份链三件套：读链 / 创建（includeCredentials）/ 还原（stamp）——environment.ts 已封装。 */
export {
  readMasterBackupChain,
  createMasterBackup,
  restoreMasterBackup,
} from "./environment";

/** WB 备份链四件套（list/create/restore/delete，delete 内含 confirmed）+ WB 自动签到三件套 + 参与集清单。 */
export {
  fetchWorkBuddyBackupChain,
  createWorkBuddyBackup,
  restoreWorkBuddyBackup,
  deleteWorkBuddyBackups,
  fetchWorkBuddyAutoCheckinStatus,
  setWorkBuddyAutoCheckinSettings,
  setWorkBuddyAccountAutoCheckin,
  listWorkBuddyCheckinAccounts,
} from "./workbuddyAdapter";
