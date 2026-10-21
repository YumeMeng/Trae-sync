import { useCallback, useEffect, useState } from "react";
import type { WorkBuddyCheckinParticipationDto } from "../types/product";
import type { WorkBuddyAutoCheckinStatusDto } from "../platform/workbuddyAdapter";
import {
  fetchWorkBuddyAutoCheckinStatus,
  listWorkBuddyCheckinAccounts,
  setWorkBuddyAccountAutoCheckin,
  setWorkBuddyAutoCheckinSettings,
} from "../platform/workbuddyAdapter";
import { AutoCheckinSettingsCard } from "./shared/checkin/AutoCheckinSettingsCard";
import { notify } from "../platform/notify";
import { safeUiErrorMessage } from "../utils/safeUiError";

/**
 * WB 每日自动签到设置（P19.1③ 归位设置页）：总开关/触发时间/台账行（status
 * 命令）+ 参与集行与全部参与（list/set 命令）——对齐 TRAE「每日自动签到在
 * 设置页」的真实排布；签到页只留签到动作区。
 */
export function WorkBuddyAutoCheckinSettings() {
  const [autoStatus, setAutoStatus] = useState<WorkBuddyAutoCheckinStatusDto | null>(null);
  const [participation, setParticipation] = useState<readonly WorkBuddyCheckinParticipationDto[]>([]);
  const [autoBusy, setAutoBusy] = useState(false);

  const refreshAuto = useCallback(async () => {
    const [statusResult, participationResult] = await Promise.allSettled([
      fetchWorkBuddyAutoCheckinStatus(),
      listWorkBuddyCheckinAccounts(),
    ]);
    setAutoStatus(statusResult.status === "fulfilled" ? statusResult.value : null);
    setParticipation(participationResult.status === "fulfilled" ? participationResult.value : []);
  }, []);

  useEffect(() => {
    void refreshAuto();
  }, [refreshAuto]);

  const handleSaveAutoSettings = useCallback(
    (enabled: boolean, dailyTimeHhmm: string) => {
      if (autoBusy) return;
      setAutoBusy(true);
      void setWorkBuddyAutoCheckinSettings(enabled, dailyTimeHhmm)
        .then(refreshAuto)
        .then(() => {
          notify.success(enabled
            ? `WorkBuddy：每日自动签到已开启，每日 ${dailyTimeHhmm} 后自动执行（参与账号逐个错峰）。`
            : "WorkBuddy：每日自动签到已关闭。");
        })
        .catch((reason: unknown) => notify.error(
          `WorkBuddy：自动签到设置保存失败——${safeUiErrorMessage(reason, "请稍后重试。")}`,
        ))
        .finally(() => setAutoBusy(false));
    },
    [autoBusy, refreshAuto],
  );

  const handleToggleAccount = useCallback(
    (profileId: string, enabled: boolean) => {
      if (autoBusy) return;
      setAutoBusy(true);
      // 显式设置写后端 touched+enabled（opt-out 永久尊重，be-engineer B5 契约）。
      void setWorkBuddyAccountAutoCheckin(profileId, enabled)
        .then(refreshAuto)
        .catch((reason: unknown) => notify.error(
          `WorkBuddy：参与状态保存失败——${safeUiErrorMessage(reason, "请稍后重试。")}`,
        ))
        .finally(() => setAutoBusy(false));
    },
    [autoBusy, refreshAuto],
  );

  // 批量置 true 入口：把显式关闭的账号全部重新参与（单次语义，后端持久）。
  const handleParticipateAll = useCallback(() => {
    if (autoBusy) return;
    const targets = participation.filter((account) => !account.enabled);
    if (targets.length === 0) return;
    setAutoBusy(true);
    void (async () => {
      for (const account of targets) {
        await setWorkBuddyAccountAutoCheckin(account.profile_id, true).catch(() => undefined);
      }
      await refreshAuto();
      notify.success(`WorkBuddy：已把 ${targets.length} 个账号全部置为参与自动签到。`);
    })().finally(() => setAutoBusy(false));
  }, [autoBusy, participation, refreshAuto]);

  return (
    <AutoCheckinSettingsCard
      testIdPrefix="workbuddy"
      enabled={autoStatus?.enabled ?? false}
      dailyTimeHhmm={autoStatus?.daily_time_hhmm ?? "09:30"}
      busy={autoBusy}
      toggleSummary="开启后到点自动执行（断连自动续期兜底）"
      ledgerNote={autoStatus?.ledger
        ? autoStatus.ledger.running
          ? `今日执行中（完成 ${autoStatus.ledger.completed}/${autoStatus.ledger.total}${autoStatus.ledger.failed ? `，失败 ${autoStatus.ledger.failed}` : ""}）`
          : `今日已完成（成功 ${autoStatus.ledger.completed}/${autoStatus.ledger.total}${autoStatus.ledger.failed ? `，失败 ${autoStatus.ledger.failed}` : ""}）`
        : null}
      accounts={participation.map((account) => ({
        profileId: account.profile_id,
        displayName: account.screen_name,
        // enabled = 后端有效参与（list 命令；P19-B6 数据源统一）。
        participating: account.enabled,
      }))}
      onSettingsChange={handleSaveAutoSettings}
      onParticipationChange={handleToggleAccount}
      onParticipateAll={handleParticipateAll}
    />
  );
}
