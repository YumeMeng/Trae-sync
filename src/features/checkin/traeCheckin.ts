import { deriveCheckinSlotState } from "../../components/StatusBadges";
import type { CheckinSnapshot } from "../../platform/checkin";
import type { CheckinOverviewEntryDto } from "../../types/account_switch";
import { effectiveDisplayName } from "../../utils/accountDisplay";
import {
  autoFor,
  autoStatusLine,
  EMPTY_VIEW,
  formatCreditsValue,
  formatShortDate,
  type CheckinActionView,
  type CheckinRowView,
  type CheckinStatusView,
  type CheckinView,
  type CheckinViewInput,
} from "./config";

/**
 * TRAE 签到页的数据面配置（P4b-2）：真实通道四动作 + 本地刷新；
 * 演示（fixture）模式只有「全部签到（所选）」。
 * 本文件只做「快照 → 视图模型」的纯映射，不读数据、不碰 DOM。
 */

/** 行内元信息：模型额度（含缓存时间）+ 最近验证；签到活动积分为静态池值，不展示。 */
function entryMeta(entry: CheckinOverviewEntryDto): string {
  const parts: string[] = [];
  if (entry.usage_remaining_credits !== null) {
    const cached = entry.usage_cached_at
      ? `（${formatShortDate(entry.usage_cached_at)} 缓存）`
      : "";
    parts.push(`额度 ${formatCreditsValue(entry.usage_remaining_credits)}${cached}`);
  }
  if (entry.last_verified_at) parts.push(`最近验证 ${formatShortDate(entry.last_verified_at)}`);
  return parts.join(" · ");
}

/** 真实模式行：签到槽六态由 G10 判定函数求态（已签 > 今日尝试 > 未刷新 > 未签）。 */
function overviewRow(entry: CheckinOverviewEntryDto): CheckinRowView {
  return {
    profileId: entry.profile_id,
    displayName: effectiveDisplayName(entry),
    badge: { kind: "slot", state: deriveCheckinSlotState(entry) },
    meta: entryMeta(entry),
    checkedIn: entry.checked_in,
    error: null,
  };
}

/** 演示模式行：无服务端状态，只有名字（不假装有状态）。 */
function fixtureRow(account: {
  readonly profile_id: string;
  readonly display_name: string;
}): CheckinRowView {
  return {
    profileId: account.profile_id,
    displayName: account.display_name,
    badge: null,
    meta: "",
    checkedIn: null,
    error: null,
  };
}

export function traeCheckinView(
  snapshot: CheckinSnapshot,
  input: CheckinViewInput,
  productId: string,
): CheckinView {
  if (snapshot.kind !== "trae") return EMPTY_VIEW;
  const { capability, overview, fixturePool, auto, error } = snapshot.trae;
  const real = capability?.real_http_enabled === true;
  const fixture = capability?.transport === "fixture";
  const rows = real
    ? (overview ?? []).map(overviewRow)
    : fixture
      ? (fixturePool ?? []).map(fixtureRow)
      : [];
  const poolIds = rows.map((row) => row.profileId);
  const pendingIds = rows.filter((row) => row.checkedIn !== true).map((row) => row.profileId);
  const enabled = capability?.enabled === true && rows.length > 0 && !input.busy;
  const selectedCount = input.selection.length;

  // 常驻状态行只保留两类：自动签到台账 + 演示模式标识 / 存储未就绪异常态。
  const statuses: CheckinStatusView[] = [];
  const autoLine = autoStatusLine(autoFor(input, auto), "auto-checkin-status");
  if (autoLine !== null) statuses.push(autoLine);
  if (fixture) statuses.push({ testId: "checkin-demo-banner", text: "演示模式", tone: "info" });
  if (capability !== null && !fixture && !capability.enabled) {
    statuses.push({ testId: "checkin-unavailable", text: "签到功能不可用：存储未就绪", tone: "warn" });
  }

  const showSelectedRun = selectedCount > 0 && selectedCount < rows.length;
  const actions: CheckinActionView[] = real
    ? [
        { key: "run-all", label: `一键全签（${rows.length}）`, tone: "primary", enabled, testId: "checkin-run-all" },
        { key: "run-pending", label: `一键补签（${pendingIds.length}）`, tone: "secondary", enabled: enabled && pendingIds.length > 0, testId: "checkin-run-pending" },
        ...(showSelectedRun
          ? [{ key: "run-selected" as const, label: `签到所选（${selectedCount}）`, tone: "secondary" as const, enabled, testId: "checkin-run-selected" }]
          : []),
        { key: "cancel", label: "取消未开始任务", tone: "quiet", enabled: input.busy, testId: "checkin-cancel" },
      ]
    : [
        { key: "run-all", label: `全部签到（${selectedCount}）`, tone: "primary", enabled: enabled && selectedCount > 0, testId: "checkin-run-all" },
        { key: "cancel", label: "取消未开始任务", tone: "quiet", enabled: input.busy, testId: "checkin-cancel" },
      ];

  return {
    productId,
    title: "签到",
    statuses,
    error: error === null
      ? null
      : { description: error, retryLabel: "重新读取", testId: "checkin-error" },
    rows,
    columns: { selection: true, badge: real, meta: real, inlineAction: real },
    listVariant: real ? "flow" : "plain",
    actions,
    defaultSelection: poolIds,
    pendingIds,
    empty: rows.length === 0 && error === null
      ? {
          description: "还没有可签到的账号。",
          actionLabel: "去账号页添加",
          actionPage: "accounts",
          testId: "checkin-empty",
        }
      : null,
    refresh: real
      ? {
          label: "刷新",
          enabled: !input.busy,
          hint: "重读本机缓存的账号信息（不联网）",
          testId: "checkin-refresh-local",
        }
      : null,
    texts: {
      notificationPrefix: "TRAE",
      runSuccess: "签到流程已完成。",
      runPartial: "部分签到未完成，请查看逐账号结果。",
      runFailure: "签到未完成，请稍后重试。",
    },
  };
}
