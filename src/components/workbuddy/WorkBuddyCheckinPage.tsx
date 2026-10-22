import { useCallback, useEffect, useRef, useState } from "react";
import { RefreshCw } from "lucide-react";
import type {
  WorkBuddyCheckinAccountStatusDto,
  WorkBuddyCheckinParticipationDto,
} from "../../types/product";
import {
  claimWorkBuddyCheckinBatch,
  claimWorkBuddyCheckinProfile,
  fetchWorkBuddyAutoCheckinStatus,
  fetchWorkBuddyCheckinOverview,
  listWorkBuddyCheckinAccounts,
  type WorkBuddyAutoCheckinStatusDto,
} from "../../platform/workbuddyAdapter";
import { CheckinSlotBadge, deriveCheckinSlotState } from "../StatusBadges";
import { CheckinPageSkeleton } from "../shared/checkin/CheckinPageSkeleton";
import { CheckinActionsRow } from "../shared/checkin/CheckinActionsRow";
import { CheckinFlowRow } from "../shared/checkin/CheckinFlowRow";
import { productUi } from "../../platform/productUiRegistry";
import { notify } from "../../platform/notify";
import { safeUiErrorMessage } from "../../utils/safeUiError";

/** P18-B3 对齐 CheckinPage 口径：WorkBuddy 自动签到台账文案。 */
function wbAutoLedgerLabel(status: WorkBuddyAutoCheckinStatusDto): string {
  const ledger = status.ledger;
  if (!ledger) return "今日尚未发起";
  const parts = [`成功 ${ledger.completed}/${ledger.total}`];
  if (ledger.failed > 0) parts.push(`失败 ${ledger.failed}`);
  if (ledger.skipped > 0) parts.push(`跳过 ${ledger.skipped}`);
  return ledger.running ? `今日执行中（${parts.join("，")}）` : `今日已完成（${parts.join("，")}）`;
}
// ============================================================================
// 签到页：WorkBuddy 直连签到（status/claim 幂等编排在后端）
// ============================================================================

interface WorkBuddyCheckinPageProps {
  active: boolean;
}

/**
 * P19-B5 后端契约消费（be-engineer edbd20b/9609f9a）：
 * - get_workbuddy_checkin_overview：逐账号状态批查（后端 fail-soft，error 落条目）
 * - workbuddy_checkin_claim_profile / workbuddy_checkin_claim_batch：单签/批量
 *   （幂等 AlreadyCheckedIn → credit=0 verified=true；批量不可中断，无取消）
 * - opt-out 后端化：auto_checkin_store touched 权威，enabled = 有效参与
 *   （未显式设置默认 true，八域对齐 #3）——B2 的前端 touched 集与归一化退役。
 * P19.2：签到历史点阵条删除（用户拍板「显示签到的账号就行」），IA §1.3 签到
 * 日历 WB 侧豁免；checkin_dates 字段保留在 DTO（后端数据面不变）。
 */

export function WorkBuddyCheckinPage({ active }: WorkBuddyCheckinPageProps) {
  // 逐账号签到状态（后端聚合批查，fail-soft 落条目 error）。
  const [rows, setRows] = useState<readonly WorkBuddyCheckinAccountStatusDto[]>([]);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  // 批量选择（默认 = 有效参与集，IA 草案 §1.3；用户可增删，仅作用本次批次）。
  const [selection, setSelection] = useState<readonly string[]>([]);
  const [busy, setBusy] = useState(false);
  // 逐行 claim 结果（outcome 与 CheckinResultDto 同口径，进 OutcomeLabels 通用档）。
  const [rowResults, setRowResults] = useState<Record<string, { outcome: string; detail_code: string | null }>>({});
  // 参与集已加载标记：默认选择用参与集；未加载（命令失败）回退全选。
  const participationLoadedRef = useRef(false);
  // P17-A5 每日自动签到：设置（status 命令）+ 参与集行（list 命令，有效参与）。
  const [autoStatus, setAutoStatus] = useState<WorkBuddyAutoCheckinStatusDto | null>(null);
  const [participation, setParticipation] = useState<readonly WorkBuddyCheckinParticipationDto[]>([]);

  const refresh = useCallback(async (initial: boolean) => {
    if (initial) setLoading(true);
    try {
      const next = await fetchWorkBuddyCheckinOverview(null);
      setRows(next);
      setLoadError(null);
    } catch (reason: unknown) {
      setLoadError(safeUiErrorMessage(reason, "签到状态暂时不可读取，请稍后重试。"));
    } finally {
      setLoading(false);
    }
  }, []);

  const refreshAuto = useCallback(async () => {
    const [statusResult, participationResult] = await Promise.allSettled([
      fetchWorkBuddyAutoCheckinStatus(),
      listWorkBuddyCheckinAccounts(),
    ]);
    setAutoStatus(statusResult.status === "fulfilled" ? statusResult.value : null);
    if (participationResult.status === "fulfilled") {
      participationLoadedRef.current = true;
      setParticipation(participationResult.value);
    } else {
      setParticipation([]);
    }
  }, []);

  useEffect(() => {
    if (!active) return;
    void refresh(true);
    void refreshAuto();
  }, [active, refresh, refreshAuto]);

  // 批量选择默认 = 有效参与集（list 命令）；行集变化时剔除失效项并保留既有
  // 选择；参与集未加载（命令失败）回退全选，加载但为空（全部显式关闭）保持空选。
  useEffect(() => {
    setSelection((current) => {
      const filtered = current.filter((id) => rows.some((row) => row.profile_id === id));
      if (filtered.length > 0) return filtered;
      if (participationLoadedRef.current) {
        return participation.filter((account) => account.enabled).map((account) => account.profile_id);
      }
      return rows.map((row) => row.profile_id);
    });
  }, [rows, participation]);

  const toggle = useCallback((profileId: string) => {
    setSelection((current) => current.includes(profileId)
      ? current.filter((id) => id !== profileId)
      : [...current, profileId]);
  }, []);

  // 行内单签（矩阵 #56）：幂等领取 → 行内结果 + 该行状态回读。
  const handleClaimOne = useCallback(async (profileId: string) => {
    if (busy) return;
    setBusy(true);
    try {
      const claim = await claimWorkBuddyCheckinProfile(profileId);
      setRowResults((current) => ({
        ...current,
        [profileId]: { outcome: claim.credit > 0 ? "claimed" : "already_checked_in", detail_code: null },
      }));
      const refreshed = await fetchWorkBuddyCheckinOverview(profileId).catch(() => null);
      const row = refreshed?.find((item) => item.profile_id === profileId);
      if (row) setRows((current) => current.map((item) => (item.profile_id === profileId ? row : item)));
    } catch (reason: unknown) {
      setRowResults((current) => ({ ...current, [profileId]: { outcome: "runtime_error", detail_code: null } }));
      notify.error(`WorkBuddy：签到未完成——${safeUiErrorMessage(reason, "请稍后重试。")}`);
    } finally {
      setBusy(false);
    }
  }, [busy]);

  // 批量签到（矩阵 #51）：后端顺序执行逐条回执（fail-soft），前端不提供取消。
  const handleRunIds = useCallback(async (ids: readonly string[]) => {
    if (busy || ids.length === 0) return;
    setBusy(true);
    setRowResults({});
    try {
      const items = await claimWorkBuddyCheckinBatch(ids);
      setRowResults(Object.fromEntries(
        items.map((item) => [item.profile_id, { outcome: item.outcome, detail_code: item.detail_code }]),
      ));
      const failed = items.filter((item) => item.outcome !== "claimed" && item.outcome !== "already_checked_in").length;
      if (failed === 0) notify.success("WorkBuddy：签到批次完成（已签账号幂等跳过，结果见各行）。");
      else notify.warning(`WorkBuddy：部分签到未完成（${failed} 个失败），详见各行结果与通知。`);
      await refresh(false);
    } catch (reason: unknown) {
      notify.error(`WorkBuddy：签到未完成——${safeUiErrorMessage(reason, "请稍后重试。")}`);
    } finally {
      setBusy(false);
    }
  }, [busy, refresh]);

  // P19-B5：WB 签到页 = 逐账号行批量形态（IA 草案 §1.3）。
  // 页面编排由 CheckinPageSkeleton 承担；调度设置卡走 content 后置插槽。
  const uiProduct = productUi("workbuddy");
  const pendingIds = rows
    .filter((row) => row.error === null && row.today_checked_in === false)
    .map((row) => row.profile_id);

  return (
    <CheckinPageSkeleton
      product={uiProduct}
      active={active}
      loading={loading}
      headerActions={!loadError ? (
        <div className="page-header__actions">
          <button
            type="button"
            className="btn"
            onClick={() => void refresh(false)}
            disabled={loading || busy}
            data-testid="workbuddy-checkin-refresh"
          >
            <RefreshCw size={15} className={loading ? "icon-spin" : undefined} aria-hidden="true" />
            刷新
          </button>
        </div>
      ) : undefined}
      errorState={loadError ? {
        title: "无法读取签到状态",
        description: loadError,
        action: (
          <button type="button" className="btn btn--primary" onClick={() => void refresh(true)}>
            重新读取
          </button>
        ),
      } : undefined}
      statusRegion={autoStatus ? (
        <p className="checkin-page__meta" role="status" data-testid="workbuddy-auto-checkin-meta">
          自动签到：{autoStatus.enabled ? `每日 ${autoStatus.daily_time_hhmm} 触发` : "已关闭"}
          {autoStatus.enabled ? ` · ${wbAutoLedgerLabel(autoStatus)}` : ""}
        </p>
      ) : null}
      entryCount={rows.length}
      selectedCount={selection.length}
      actions={(
        <CheckinActionsRow
          variant="batch"
          enabled={rows.length > 0}
          busy={busy}
          poolCount={rows.length}
          pendingCount={pendingIds.length}
          selectionCount={selection.length}
          onRunAll={() => void handleRunIds(rows.map((row) => row.profile_id))}
          onRunPending={() => void handleRunIds(pendingIds)}
          onRunSelected={() => void handleRunIds(selection)}
        />
      )}
      rows={rows.map((row) => (
        <CheckinFlowRow
          key={row.profile_id}
          productId="workbuddy"
          profileId={row.profile_id}
          displayName={row.screen_name || "WorkBuddy 账号"}
          badge={row.error
            ? <span className="slot-badge slot-badge--idle" title={row.error}>状态未知</span>
            : <CheckinSlotBadge state={deriveCheckinSlotState({ checked_in: row.today_checked_in === true })} />}
          meta={row.error || row.total_credits === null
            ? ""
            : `额度 ${row.total_credits} 积分 · 连签 ${row.streak_days ?? 0} 天`}
          selected={selection.includes(row.profile_id)}
          onToggle={() => toggle(row.profile_id)}
          disabled={busy}
          phase="idle"
          result={rowResults[row.profile_id] ?? null}
          waitSecs={null}
          onInlineCheckin={() => void handleClaimOne(row.profile_id)}
          inlineEnabled={row.error === null && row.today_checked_in === false}
        />
      ))}
      listVariant="flow"
      content={null}
    />
  );
}
