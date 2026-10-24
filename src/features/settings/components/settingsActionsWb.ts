/**
 * WorkBuddy 设置页动作编排（P4f-5）：命令 + 通知文案，逐字沿用旧实现
 * WorkBuddyBackupSection L53-97 / WorkBuddyAutoCheckinSettings L37-84。
 * 零 React、零状态、零产品判断：刷新时机与忙碌位由 useWorkBuddySettings 持有。
 * 命令一律经 platform/settings（ui-guard 规则 5：features 内禁止 invoke(）。
 */
import type { WorkBuddyCheckinParticipationDto } from "../../../types/product";
import type { WorkBuddyAutoCheckinStatusDto } from "../../../platform/workbuddyAdapter";
import { notify } from "../../../platform/notify";
import { safeUiErrorMessage } from "../../../utils/safeUiError";
import {
  createWorkBuddyBackup, deleteWorkBuddyBackups, fetchWorkBuddyAutoCheckinStatus,
  listWorkBuddyCheckinAccounts, restoreWorkBuddyBackup, setWorkBuddyAccountAutoCheckin,
  setWorkBuddyAutoCheckinSettings,
} from "../../../platform/settings";
import { SETTINGS_TEXTS, fillSettingsText } from "../config";
import type { WbIo } from "./settingsTypes";

/* ===== WorkBuddy（对话库） ===== */

/** 读 WB 自动签到两路（旧 WBA L24-31 的 Promise.allSettled：单路失败各自降级）。 */
export async function readWbAuto(): Promise<{
  readonly status: WorkBuddyAutoCheckinStatusDto | null;
  readonly participation: readonly WorkBuddyCheckinParticipationDto[];
}> {
  const [status, participation] = await Promise.allSettled([
    fetchWorkBuddyAutoCheckinStatus(),
    listWorkBuddyCheckinAccounts(),
  ]);
  return {
    status: status.status === "fulfilled" ? status.value : null,
    participation: participation.status === "fulfilled" ? participation.value : [],
  };
}

/** WB 自动签到设置（旧 WBA L37-54）。 */
export async function saveWbAuto(enabled: boolean, dailyTimeHhmm: string, io: WbIo): Promise<void> {
  try {
    await setWorkBuddyAutoCheckinSettings(enabled, dailyTimeHhmm);
    await io.reloadAuto();
    notify.success(enabled
      ? `WorkBuddy：每日自动签到已开启，每日 ${dailyTimeHhmm} 后自动执行（参与账号逐个错峰）。`
      : "WorkBuddy：每日自动签到已关闭。");
  } catch (reason: unknown) {
    notify.error(`WorkBuddy：自动签到设置保存失败——${safeUiErrorMessage(reason, "请稍后重试。")}`);
  }
}

/** WB 逐账号参与开关（旧 WBA L56-69：显式设置写后端 touched+enabled，opt-out 永久尊重）。 */
export async function toggleWbAccount(profileId: string, enabled: boolean, io: WbIo): Promise<void> {
  try {
    await setWorkBuddyAccountAutoCheckin(profileId, enabled);
    await io.reloadAuto();
  } catch (reason: unknown) {
    notify.error(`WorkBuddy：参与状态保存失败——${safeUiErrorMessage(reason, "请稍后重试。")}`);
  }
}

/** WB 批量置为参与（旧 WBA L72-84：逐个写、失败跳过、完成后一次回执）。 */
export async function participateAllWb(profileIds: readonly string[], io: WbIo): Promise<void> {
  for (const profileId of profileIds) await setWorkBuddyAccountAutoCheckin(profileId, true).catch(() => undefined);
  await io.reloadAuto();
  notify.success(`WorkBuddy：已把 ${profileIds.length} 个账号全部置为参与自动签到。`);
}

/** WB 手动备份（旧 WB L53-67：滚动清理份数如实回执）；返回值 = 是否成功（失败不刷新链）。 */
export async function createWbBackup(io: WbIo): Promise<boolean> {
  try {
    const receipt = await createWorkBuddyBackup();
    notify.success(`WorkBuddy：备份已创建${receipt.rolled_over > 0 ? `（滚动清理 ${receipt.rolled_over} 份旧备份）` : ""}。`);
    await io.reloadChain();
    return true;
  } catch (reason: unknown) {
    notify.error(`WorkBuddy：备份未完成——${safeUiErrorMessage(reason, "请稍后重试。")}`);
    return false;
  }
}

/** WB 恢复（旧 WB L69-82：safety_backup_stamp 回退点必须上屏）。 */
export async function restoreWbBackup(stamp: number, io: WbIo): Promise<boolean> {
  try {
    const receipt = await restoreWorkBuddyBackup(stamp);
    const safetyTime = new Date(receipt.safety_backup_stamp * 1000).toLocaleString();
    notify.success(`WorkBuddy：已恢复到 ${receipt.restored_stamp}（恢复前状态已存为 ${safetyTime} 的备份，可回退）。`);
    await io.reloadChain();
    return true;
  } catch (reason: unknown) {
    notify.error(`WorkBuddy：恢复未完成——${safeUiErrorMessage(reason, "数据未被改动，请稍后重试。")}`);
    return false;
  }
}

/** WB 删除所选（旧 WB L84-97：成功后清空多选并重读链）；返回值 = 是否成功。 */
export async function deleteWbBackups(stamps: readonly number[], io: WbIo): Promise<boolean> {
  try {
    const receipt = await deleteWorkBuddyBackups(stamps);
    notify.success(`WorkBuddy：已删除 ${receipt.deleted_stamps.length} 份备份（剩余 ${receipt.remaining} 份）。`);
    await io.reloadChain();
    return true;
  } catch (reason: unknown) {
    notify.error(`WorkBuddy：备份删除未完成——${safeUiErrorMessage(reason, "请稍后重试。")}`);
    return false;
  }
}

/** WB 创建确认弹层文案（旧 WB L210-225：进程纪律——客户端会关闭并重启）。 */
export function wbCreateConfirmText() {
  return {
    title: SETTINGS_TEXTS.wbCreateTitle,
    lines: [SETTINGS_TEXTS.wbCreateLine1, SETTINGS_TEXTS.wbCreateLine2, SETTINGS_TEXTS.wbCreateLine3],
    confirmLabel: SETTINGS_TEXTS.backupCreate, busyLabel: SETTINGS_TEXTS.backupBusy,
    danger: false, testId: "workbuddy-backup-create-confirm",
  } as const;
}

/** WB 删除确认弹层文案（旧 WB L246-262：所选份数插值，危险色）。 */
export function wbDeleteConfirmText(count: number) {
  return {
    title: SETTINGS_TEXTS.wbDeleteTitle,
    lines: [fillSettingsText(SETTINGS_TEXTS.wbDeleteLine1, { count }), SETTINGS_TEXTS.wbDeleteLine2],
    confirmLabel: SETTINGS_TEXTS.wbDeleteConfirm, busyLabel: SETTINGS_TEXTS.wbDeleting,
    danger: true, testId: "workbuddy-backup-delete-confirm",
  } as const;
}
