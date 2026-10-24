/**
 * TRAE 设置页动作编排（P4f-5）：命令 + 通知文案，逐字沿用旧实现 TraeSettingsVariant L136-258。
 * 零 React、零状态、零产品判断：刷新时机与忙碌位由 useTraeSettings 持有，本层只做「命令 → 回执 → 通知」。
 * 命令一律经 platform/settings（ui-guard 规则 5：features 内禁止 invoke(）。
 */
import { notify } from "../../../platform/notify";
import { safeUiErrorMessage } from "../../../utils/safeUiError";
import {
  createMasterBackup, migrateMasterBackups, restoreMasterBackup, saveAutoCheckinSettings,
  saveBackupRetention, setBackupDir,
} from "../../../platform/settings";
import type { SettingsIo } from "./settingsTypes";

/* ===== TRAE（主库） ===== */

/** 手动备份：勾选凭据时后端整拷凭据子目录；成功后重读备份链再报回执（旧 L136-152 次序）。 */
export async function createTraeBackup(includeCredentials: boolean, io: SettingsIo): Promise<void> {
  try {
    await createMasterBackup({ includeCredentials });
    await io.reloadChain();
    notify.success(includeCredentials
      ? "主库数据备份已创建（含账号凭据子目录），原数据未受影响。"
      : "主库数据备份已创建，原数据未受影响。");
  } catch (reason: unknown) {
    notify.error(safeUiErrorMessage(reason, "主库数据备份未完成。"));
  }
}

/** 保存自动签到：开关与时间一起提交（后端原子写保留台账 + 格式校验，旧 L244-258）。 */
export async function saveTraeAuto(enabled: boolean, dailyTimeHhmm: string, io: SettingsIo): Promise<void> {
  try {
    await saveAutoCheckinSettings({ enabled, dailyTimeHhmm });
    await io.reloadAuto();
    notify.success(enabled ? `自动签到已开启，每日 ${dailyTimeHhmm} 后自动执行。` : "自动签到已关闭。");
  } catch (reason: unknown) {
    notify.error(safeUiErrorMessage(reason, "自动签到设置保存失败。"));
  }
}

/** 保存保留开关：keep 原样透传（P17-A3 已退役为滚动单份，旧 L197-214）。 */
export async function saveTraeRetention(enabled: boolean, keep: number, io: SettingsIo): Promise<void> {
  try {
    await saveBackupRetention({ enabled, keep });
    await io.reloadRetention();
    notify.success(enabled
      ? "备份自动清理已开启：同目录只保留最新一份备份，更早的会在新备份生成后删除。"
      : "备份自动清理已关闭，旧备份不会再被自动删除。");
  } catch (reason: unknown) {
    notify.error(safeUiErrorMessage(reason, "备份清理设置保存失败。"));
  }
}

/**
 * 更改备份位置（旧 L156-193）：先迁移（此刻活动目录仍是旧位置）再切换；
 * 迁移出现失败清单时中止切换（已复制的保留，可排查后重试，迁移幂等）。
 * 返回值 = 是否关闭弹层（部分失败也关；命令失败保持打开，与旧实现同）。
 */
export async function changeTraeLocation(
  { targetDir, migrateExisting }: { readonly targetDir: string; readonly migrateExisting: boolean },
  io: SettingsIo,
): Promise<boolean> {
  try {
    const migration = migrateExisting ? await migrateMasterBackups({ targetDir }) : null;
    if (migration !== null && migration.failures.length > 0) {
      notify.summary(
        "部分备份未能复制到新位置，备份位置未变更",
        migration.failures.map((failure) => ({ displayName: failure.file_name, reason: failure.reason })),
        migration.migrated_files,
      );
    } else {
      await setBackupDir({ backupDir: targetDir });
      notify.success(migration !== null
        ? `备份位置已更新，${migration.migrated_files} 个文件已复制到新位置（原位置备份保留，可自行清理）。`
        : "备份位置已更新，之后的备份将写入新目录。");
    }
    await io.reloadChain();
    return true;
  } catch (reason: unknown) {
    notify.error(safeUiErrorMessage(reason, "备份位置变更未完成，当前设置保持不变。"));
    return false;
  }
}

/** 从备份还原主库（旧 L218-241）：体检 ready 报成功，否则摘要告警；失败原数据未动。 */
export async function restoreTraeBackup(stamp: number, io: SettingsIo): Promise<boolean> {
  try {
    const receipt = await restoreMasterBackup({ stamp });
    if (receipt.checkup_status === "ready") {
      notify.success("主库已从备份还原，体检复核通过，TRAE 已重新启动。");
    } else {
      notify.summary(
        "主库已还原，但恢复后体检未通过，请到库详情页复核",
        [{ displayName: "恢复后体检", reason: receipt.checkup_status === "read_failed"
          ? "数据库无法读取（密钥或文件异常）" : "未检出主库数据" }],
        0,
      );
    }
    await io.reloadChain();
    return true;
  } catch (reason: unknown) {
    notify.error(safeUiErrorMessage(reason, "主库还原未完成，原数据未受影响。"));
    return false;
  }
}
