import { deriveCheckinSlotState } from "../../components/StatusBadges";
import type { CheckinSnapshot } from "../../platform/checkin";
import type { WorkBuddyCheckinAccountStatusDto } from "../../types/product";
import {
  autoStatusLine,
  EMPTY_VIEW,
  type CheckinActionView,
  type CheckinRowBadge,
  type CheckinRowView,
  type CheckinStatusView,
  type CheckinView,
  type CheckinViewInput,
} from "./config";

/**
 * WorkBuddy 签到页的数据面配置（P4b-2）：参与制默认勾选 + 逐账号 fail-soft 行；
 * 后端整批不可中断，故无取消按钮。
 * 本文件只做「快照 → 视图模型」的纯映射，不读数据、不碰 DOM。
 */

/** 行：fail-soft——error 非空时状态列写「状态未知」（原因进悬浮提示），元信息留空。 */
function workbuddyRow(row: WorkBuddyCheckinAccountStatusDto): CheckinRowView {
  const hasError = row.error !== null;
  const badge: CheckinRowBadge = hasError
    ? { kind: "text", word: "状态未知", hint: row.error ?? "" }
    : { kind: "slot", state: deriveCheckinSlotState({ checked_in: row.today_checked_in === true }) };
  const meta = hasError || row.total_credits === null
    ? ""
    : `额度 ${row.total_credits} 积分 · 连签 ${row.streak_days ?? 0} 天`;
  return {
    profileId: row.profile_id,
    displayName: row.screen_name || "WorkBuddy 账号",
    badge,
    meta,
    checkedIn: row.today_checked_in,
    error: row.error,
  };
}

export function workbuddyCheckinView(
  snapshot: CheckinSnapshot,
  input: CheckinViewInput,
  productId: string,
): CheckinView {
  if (snapshot.kind !== "workbuddy") return EMPTY_VIEW;
  const { rows: statusRows, participation, auto, error } = snapshot.workbuddy;
  const rows = (statusRows ?? []).map(workbuddyRow);
  const poolIds = rows.map((row) => row.profileId);
  const pendingIds = rows
    .filter((row) => row.error === null && row.checkedIn === false)
    .map((row) => row.profileId);
  // 默认勾选 = 有效参与集；参与集未加载（命令失败）回退全选，加载但为空保持空选。
  const participating = participation === null
    ? poolIds
    : participation.filter((account) => account.enabled).map((account) => account.profile_id);
  const defaultSelection = participating.filter((id) => poolIds.includes(id));
  const enabled = rows.length > 0 && !input.busy;
  const selectedCount = input.selection.length;

  const statuses: CheckinStatusView[] = [];
  const autoLine = autoStatusLine(auto, "workbuddy-auto-checkin-meta");
  if (autoLine !== null) statuses.push(autoLine);

  const showSelectedRun = selectedCount > 0 && selectedCount < rows.length;
  const actions: CheckinActionView[] = [
    { key: "run-all", label: `一键全签（${rows.length}）`, tone: "primary", enabled, testId: "checkin-run-all" },
    { key: "run-pending", label: `一键补签（${pendingIds.length}）`, tone: "secondary", enabled: enabled && pendingIds.length > 0, testId: "checkin-run-pending" },
    ...(showSelectedRun
      ? [{ key: "run-selected" as const, label: `签到所选（${selectedCount}）`, tone: "secondary" as const, enabled, testId: "checkin-run-selected" }]
      : []),
  ];

  return {
    productId,
    title: "签到",
    statuses,
    error: error === null
      ? null
      : { description: error, retryLabel: "重新读取", testId: "workbuddy-checkin-error" },
    rows,
    columns: { selection: true, badge: true, meta: true, inlineAction: true },
    listVariant: "flow",
    actions,
    defaultSelection,
    pendingIds,
    empty: rows.length === 0 && error === null
      ? {
          description: "还没有可签到的账号。",
          actionLabel: "去账号页添加",
          actionPage: "accounts",
          testId: "workbuddy-checkin-empty",
        }
      : null,
    refresh: {
      label: "刷新",
      enabled: !input.busy,
      hint: "重新读取各账号的今日签到状态",
      testId: "workbuddy-checkin-refresh",
    },
    texts: {
      notificationPrefix: "WorkBuddy",
      runSuccess: "签到批次完成（已签账号幂等跳过，结果见各行）。",
      runPartial: "部分签到未完成，详见各行结果与通知。",
      runFailure: "签到未完成，请稍后重试。",
    },
  };
}
