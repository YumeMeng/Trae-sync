import { useEffect, useState } from "react";
import type { AutoCheckinStatusDto } from "../../../types/account_switch";
import type { BackupRetentionDto, MasterBackupChainDto } from "../../../types/masterLibrary";
import {
  readAutoCheckinStatus, readBackupRetention, readMasterBackupChain,
} from "../../../platform/settings";
import type { TraeSettingsSnapshot } from "../traeSettings";
import { useSlot, type SettingsPageData } from "./useSettings";
import {
  changeTraeLocation, createTraeBackup, restoreTraeBackup, saveTraeAuto, saveTraeRetention,
} from "./settingsActionsTrae";
import type { LocationDialogState, SettingsActions } from "./settingsTypes";

/**
 * TRAE 设置页读取与动作 hook（P4f-5）：自动签到 / 备份链 / 保留三路独立读取（逐路代次守卫，旧
 * TraeSettingsVariant L84-131）+ 五条动作（旧 L136-258，命令经 platform/settings）。
 * 本层只管忙碌位、弹层开关与刷新时机；读取失败一律静默降级（链 null 整区消失、保存时再报错）。
 */
export function useTraeSettings(active: boolean): SettingsPageData {
  const [auto, loadAuto] = useSlot<AutoCheckinStatusDto>(active);
  // 链槽允许 null 值：读取失败落地 null（可插拔降级），与「尚未读到」同形（旧 L37 初值即 null）。
  const [chain, loadChain] = useSlot<MasterBackupChainDto | null>(active);
  const [retention, loadRetention] = useSlot<BackupRetentionDto>(active);
  const [includeCredentials, setIncludeCredentials] = useState(false);
  const [busyAuto, setBusyAuto] = useState(false);
  const [busyCreate, setBusyCreate] = useState(false);
  const [busyRetention, setBusyRetention] = useState(false);
  const [restoreStamp, setRestoreStamp] = useState<number | null>(null);
  const [busyRestore, setBusyRestore] = useState(false);
  const [location, setLocation] = useState<LocationDialogState | null>(null);
  // 三路各读一次（旧实现：保留独立一效、签到与链同效）；页面失活即不发起。
  useEffect(() => {
    if (!active) return;
    void loadRetention(readBackupRetention).catch(() => undefined);
  }, [active, loadRetention]);
  useEffect(() => {
    if (!active) return;
    void loadAuto(readAutoCheckinStatus).catch(() => undefined);
    void loadChain(readChain);
  }, [active, loadAuto, loadChain]);
  // 备份链读取失败按 null 落地（旧 L88 的 .catch(() => null)）：链不可读即整区消失（可插拔降级），
  // 与保留/签到两路不同——后两者失败保持上次值、保存时再报错（旧 L94-112 无 catch）。
  const readChain = () => readMasterBackupChain().catch(() => null);
  const io = {
    reloadAuto: () => loadAuto(readAutoCheckinStatus),
    reloadChain: () => loadChain(readChain),
    reloadRetention: () => loadRetention(readBackupRetention),
  };
  const actions: SettingsActions = {
    toggleAuto: (enabled) => {
      setBusyAuto(true);
      void saveTraeAuto(enabled, auto?.daily_time_hhmm ?? "", io).finally(() => setBusyAuto(false));
    },
    changeTime: (time) => {
      setBusyAuto(true);
      void saveTraeAuto(auto?.enabled === true, time, io).finally(() => setBusyAuto(false));
    },
    toggleParticipation: () => undefined, // TRAE 逐账号参与开关在账号详情页（设置页无该清单）
    participateAll: () => undefined,
    createBackup: () => {
      setBusyCreate(true);
      void createTraeBackup(includeCredentials, io).finally(() => setBusyCreate(false));
    },
    startRestore: (stamp) => setRestoreStamp(stamp),
    toggleSelect: () => undefined, // TRAE 无链上多选删除（delete_master_backups 前端零引用）
    deleteSelected: () => undefined,
    openLocation: () => setLocation({
      value: { targetDir: "", migrateExisting: true }, busy: false,
      onChange: (value) => setLocation((current) => (current === null ? null : { ...current, value })),
      onCancel: () => setLocation(null),
      onConfirm: (intent) => {
        setLocation((current) => (current === null ? null : { ...current, busy: true }));
        void changeTraeLocation(intent, io).then((done) => {
          setLocation((current) => (done || current === null ? null : { ...current, busy: false }));
        });
      },
    }),
    toggleCredentials: setIncludeCredentials,
    changeRetention: (enabled) => {
      if (retention === null || busyRetention) return;
      setBusyRetention(true);
      void saveTraeRetention(enabled, retention.keep, io).finally(() => setBusyRetention(false));
    },
    retry: () => { void loadChain(readChain); },
  };
  // 忙碌动作 id：创建按钮据此改词「备份中…」，保留下拉据此禁用（旧 L338/L414 两个独立 busy）。
  const busyIds = [
    ...(busyCreate ? ["master-backup-create"] : []),
    ...(busyRetention ? ["backup-retention-enabled-select"] : []),
  ];
  const trae: TraeSettingsSnapshot = {
    autoCheckin: auto, backupChain: chain, retention, includeCredentials,
    busy: { autoCheckin: busyAuto, backupCreate: busyCreate, location: location?.busy === true, restore: busyRestore },
  };
  return {
    trae, workbuddy: null, actions, location, confirm: null, busyIds,
    restore: restoreStamp === null ? null : {
      stamp: restoreStamp, busy: busyRestore,
      onCancel: () => { if (!busyRestore) setRestoreStamp(null); },
      onConfirm: () => {
        setBusyRestore(true);
        void restoreTraeBackup(restoreStamp, io)
          .then((done) => { if (done) setRestoreStamp(null); })
          .finally(() => setBusyRestore(false));
      },
    },
  };
}
