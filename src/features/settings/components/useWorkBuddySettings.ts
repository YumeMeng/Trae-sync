import { useCallback, useEffect, useRef, useState } from "react";
import type { WorkBuddyBackupChainDto } from "../../../types/workbuddyBackup";
import type { WorkBuddyAutoCheckinStatusDto } from "../../../platform/workbuddyAdapter";
import type { WorkBuddyCheckinParticipationDto } from "../../../types/product";
import { fetchWorkBuddyBackupChain } from "../../../platform/settings";
import { safeUiErrorMessage } from "../../../utils/safeUiError";
import type { WorkBuddySettingsSnapshot } from "../workbuddySettings";
import type { SettingsPageData } from "./useSettings";
import {
  createWbBackup, deleteWbBackups, participateAllWb, readWbAuto, restoreWbBackup, saveWbAuto,
  toggleWbAccount, wbCreateConfirmText, wbDeleteConfirmText,
} from "./settingsActionsWb";
import type { ConfirmDialogState, SettingsActions } from "./settingsTypes";

/**
 * WB 设置页读取与动作 hook（P4f-5）：自动签到两路（status + 参与集，旧
 * WorkBuddyAutoCheckinSettings L24-31）+ 备份链（旧 WorkBuddyBackupSection L31-45）。
 * 读取门禁取 TRAE 口径——**旧 WB 是挂载即读，本票改为可见才读**（有意行为变更，见 KNOWN-ISSUES K-11）。
 * 四个备份动作、逐账号参与与批量参与均经 settingsActionsWb（命令经 platform/settings）。
 */
export function useWorkBuddySettings(active: boolean): SettingsPageData {
  const [auto, setAuto] = useState<WorkBuddyAutoCheckinStatusDto | null>(null);
  const [participation, setParticipation] = useState<readonly WorkBuddyCheckinParticipationDto[]>([]);
  const [chain, setChain] = useState<WorkBuddyBackupChainDto | null>(null);
  const [loading, setLoading] = useState(false);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [selectedStamps, setSelectedStamps] = useState<readonly number[]>([]);
  const [busyAuto, setBusyAuto] = useState(false);
  const [busyBackup, setBusyBackup] = useState(false);
  const [restoreStamp, setRestoreStamp] = useState<number | null>(null);
  const [confirm, setConfirm] = useState<ConfirmDialogState | null>(null);
  // 自动签到两路的读取代次：晚到的旧响应不覆盖新值（与 TRAE 槽同法）。
  const generation = useRef(0);
  const loadAuto = useCallback(async () => {
    const ticket = ++generation.current;
    const next = await readWbAuto();
    if (ticket === generation.current) { setAuto(next.status); setParticipation(next.participation); }
  }, []);
  const loadChain = useCallback(async () => {
    setLoading(true);
    try {
      setChain(await fetchWorkBuddyBackupChain());
      setLoadError(null);
    } catch (reason: unknown) {
      setLoadError(safeUiErrorMessage(reason, "备份链暂时不可读取，请稍后重试。"));
    } finally {
      setLoading(false);
    }
  }, []);
  useEffect(() => {
    if (!active) return;
    void loadAuto().catch(() => undefined);
    void loadChain();
  }, [active, loadAuto, loadChain]);
  const io = { reloadAuto: loadAuto, reloadChain: loadChain };
  const pending = participation.filter((account) => !account.enabled).map((account) => account.profile_id);
  /** 备份动作收尾：成功才清多选与恢复目标（失败保留现场供重试）。 */
  const runBackup = (run: () => Promise<boolean>) => {
    setBusyBackup(true);
    void run().then((done) => {
      if (done) { setSelectedStamps([]); setRestoreStamp(null); }
    }).finally(() => setBusyBackup(false));
  };
  const actions: SettingsActions = {
    toggleAuto: (enabled) => {
      setBusyAuto(true);
      void saveWbAuto(enabled, auto?.daily_time_hhmm ?? "09:30", io).finally(() => setBusyAuto(false));
    },
    changeTime: (time) => {
      setBusyAuto(true);
      void saveWbAuto(auto?.enabled === true, time, io).finally(() => setBusyAuto(false));
    },
    toggleParticipation: (profileId, enabled) => {
      setBusyAuto(true);
      void toggleWbAccount(profileId, enabled, io).finally(() => setBusyAuto(false));
    },
    participateAll: () => {
      if (pending.length === 0 || busyAuto) return;
      setBusyAuto(true);
      void participateAllWb(pending, io).finally(() => setBusyAuto(false));
    },
    createBackup: () => setConfirm({
      ...wbCreateConfirmText(), busy: busyBackup,
      onCancel: () => { if (!busyBackup) setConfirm(null); },
      onConfirm: () => { setConfirm(null); runBackup(() => createWbBackup(io)); },
    }),
    startRestore: (stamp) => setRestoreStamp(stamp),
    toggleSelect: (stamp, selected) => setSelectedStamps((current) => (selected
      ? current.includes(stamp) ? current : [...current, stamp]
      : current.filter((item) => item !== stamp))),
    deleteSelected: () => {
      if (selectedStamps.length === 0) return;
      setConfirm({
        ...wbDeleteConfirmText(selectedStamps.length), busy: busyBackup,
        onCancel: () => { if (!busyBackup) setConfirm(null); },
        onConfirm: () => { setConfirm(null); runBackup(() => deleteWbBackups(selectedStamps, io)); },
      });
    },
    openLocation: () => undefined, // WB 固定 storage_root/workbuddy-backups（无位置变更能力）
    toggleCredentials: () => undefined, // WB 固定含凭据（P19 决策 1）
    changeRetention: () => undefined, // WB 滚动单份在 create 内（无保留设置）
    retry: () => { void loadChain(); },
  };
  const workbuddy: WorkBuddySettingsSnapshot = {
    autoCheckin: auto, participation, backupChain: chain, backupLoading: loading,
    backupLoadError: loadError, selectedStamps, busy: { autoCheckin: busyAuto, backup: busyBackup },
  };
  return {
    trae: null, workbuddy, actions, location: null, confirm, busyIds: [],
    restore: restoreStamp === null ? null : {
      stamp: restoreStamp, busy: busyBackup,
      onCancel: () => { if (!busyBackup) setRestoreStamp(null); },
      // 旧 WB 确认即关弹层（旧 WB L70 setRestoreTarget(null) 先于执行）；TRAE 才是弹层留到完成。
      onConfirm: () => { setRestoreStamp(null); runBackup(() => restoreWbBackup(restoreStamp, io)); },
    },
  };
}
