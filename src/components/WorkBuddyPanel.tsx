import { useCallback, useEffect, useState } from "react";
import { CheckCircle2, Loader2, RefreshCw, ShieldCheck } from "lucide-react";
import type { ReactNode } from "react";
import type {
  WorkBuddySessionHealthDto,
  WorkBuddyCheckinStatusDto,
  WorkBuddyLoginSummaryDto,
} from "../types/product";
import {
  captureWorkBuddyAccount,
  claimWorkBuddyCheckin,
  fetchWorkBuddyCheckinStatus,
  listWorkBuddyAccounts,
  loadWorkBuddySessionHealth,
  refreshWorkBuddyAccount,
  switchWorkBuddyAccount,
} from "../platform/workbuddyAdapter";
import { ConfirmDialog } from "./ConfirmDialog";
import { EmptyState } from "./EmptyState";
import { notify } from "../platform/notify";
import { safeUiErrorMessage } from "../utils/safeUiError";

/**
 * WorkBuddy 产品域页面（P17-T5 并列骨架一期）。
 *
 * 两个页面槽位组件：
 * - WorkBuddyPanel：账号页（会话健康 + 捕获入池 + 工具账号池清单）；
 * - WorkBuddyCheckinPage：签到页（直连签到状态 + 领取）。
 *
 * 数据语义全部在后端组合根命令中（workbuddy_commands.rs）；本文件只做
 * 读取时机（active 才发起 IPC）、加载/错误三态与单次确认弹窗编排。
 * 界面表达纪律：不展示 uid/uin/profile_id 等内部标识，主信息只用
 * 状态、名称、时间与动作结果。
 */

/** Unix 秒 → 本地「日期 时:分」；0 = 未知返回 null。 */
function formatUnixTime(unixSeconds: number): string | null {
  if (!unixSeconds) return null;
  return new Date(unixSeconds * 1000).toLocaleString(undefined, {
    year: "numeric",
    month: "2-digit",
    day: "2-digit",
    hour: "2-digit",
    minute: "2-digit",
  });
}

/** 健康状态 → 展示文案（后端 current_session_status 判定结果的人话映射）。 */
function healthStatusLabel(status: WorkBuddySessionHealthDto["status"]): string {
  switch (status) {
    case "active":
      return "已登录";
    case "access_expired":
      return "登录已过期（可用刷新凭据）";
    case "fully_expired":
      return "登录已失效";
    case "logged_out":
      return "已在 WorkBuddy 中登出";
    case "file_missing":
      return "未检测到 WorkBuddy 登录记录";
    default:
      return "状态未知";
  }
}

/** 健康状态 → 徽章色调档位（正常 / 提示 / 异常）。 */
type HealthTone = "ok" | "warn" | "bad";

function healthStatusTone(status: WorkBuddySessionHealthDto["status"]): HealthTone {
  if (status === "active") return "ok";
  if (status === "access_expired" || status === "unknown") return "warn";
  return "bad";
}

/** 信息行：label + value 的两列展示（卡内统一行结构）。 */
function InfoRow({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div className="workbuddy-row">
      <span className="workbuddy-row__label">{label}</span>
      <span className="workbuddy-row__value">{children}</span>
    </div>
  );
}

// ============================================================================
// 账号页：WorkBuddy 会话健康 + 捕获入池 + 工具账号池
// ============================================================================

interface WorkBuddyPanelProps {
  /** 页面可见时才读取（hidden 槽位常驻，不 visible 不发 IPC）。 */
  active: boolean;
}

export function WorkBuddyPanel({ active }: WorkBuddyPanelProps) {
  const [health, setHealth] = useState<WorkBuddySessionHealthDto | null>(null);
  const [pool, setPool] = useState<readonly WorkBuddyLoginSummaryDto[]>([]);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const [refreshing, setRefreshing] = useState(false);
  // 捕获：单次确认弹窗 → 执行 → 刷新健康与账号池。
  const [captureDialogOpen, setCaptureDialogOpen] = useState(false);
  const [capturing, setCapturing] = useState(false);
  // 凭据续期（与 Trae「刷新登录凭据」同语义）：access 充裕跳过不轮换。
  const [renewing, setRenewing] = useState(false);
  // P17-T6/A4 切号：单次确认弹窗（目标 profile_id）→ 执行 → 刷新健康与池。
  const [switchTarget, setSwitchTarget] = useState<string | null>(null);
  const [switching, setSwitching] = useState(false);

  const refresh = useCallback(async (initial: boolean) => {
    if (initial) setLoading(true);
    else setRefreshing(true);
    try {
      const [nextHealth, nextPool] = await Promise.all([
        loadWorkBuddySessionHealth(),
        listWorkBuddyAccounts(),
      ]);
      setHealth(nextHealth);
      setPool(nextPool);
      setLoadError(null);
    } catch (reason: unknown) {
      setLoadError(safeUiErrorMessage(reason, "WorkBuddy 登录状态暂时不可读取，请稍后重试。"));
    } finally {
      setLoading(false);
      setRefreshing(false);
    }
  }, []);

  useEffect(() => {
    if (!active) return;
    void refresh(true);
  }, [active, refresh]);

  const runCapture = useCallback(async () => {
    setCapturing(true);
    try {
      const saved = await captureWorkBuddyAccount();
      const label = saved.screen_name || "WorkBuddy 账号";
      notify.success(`WorkBuddy：已保存账号「${label}」到账号池`);
      setCaptureDialogOpen(false);
      await refresh(false);
    } catch (reason: unknown) {
      notify.error(
        `WorkBuddy：保存失败——${safeUiErrorMessage(reason, "账号保存未完成，请稍后重试。")}`,
      );
    } finally {
      setCapturing(false);
    }
  }, [refresh]);

  // 客户端未登录时捕获必然失败（后端第一步就是会话 Active 校验），UI 提前禁用。
  const captureDisabled = health !== null && health.status !== "active";

  // 凭据续期：renewed=true 已换新钥匙；false = 仍充裕（不轮换，保护客户端副本）。
  const runRenew = useCallback(async () => {
    setRenewing(true);
    try {
      const status = await refreshWorkBuddyAccount();
      const until = formatUnixTime(status.access_expires_at_unix_seconds) ?? "未知";
      if (status.renewed) {
        notify.success(`WorkBuddy：登录凭据已更新，有效期至 ${until}`);
      } else {
        notify.info(`WorkBuddy：登录凭据仍充裕（有效期至 ${until}），本次未刷新`);
      }
      await refresh(false);
    } catch (reason: unknown) {
      notify.error(
        `WorkBuddy：刷新登录凭据失败——${safeUiErrorMessage(reason, "凭据续期未完成，请稍后重试。")}`,
      );
    } finally {
      setRenewing(false);
    }
  }, [refresh]);

  // P17-T6/A4 切号：wbipc RPC 编排在后端（logout → saveOauthToken →
  // 回读校验）；对话存云端按账号隔离，切号=换登录态自动跟随。
  const runSwitch = useCallback(async () => {
    if (!switchTarget) return;
    setSwitching(true);
    try {
      const receipt = await switchWorkBuddyAccount(switchTarget);
      const label = receipt.screen_name || "WorkBuddy 账号";
      notify.success(
        receipt.renewed
          ? `WorkBuddy：已切换到「${label}」（凭据已先续期为最新）`
          : `WorkBuddy：已切换到「${label}」`,
      );
      setSwitchTarget(null);
      await refresh(false);
    } catch (reason: unknown) {
      notify.error(
        `WorkBuddy：切换未完成——${safeUiErrorMessage(reason, "切号未完成，客户端可能停在登出状态，请在客户端重新登录或重试。")}`,
      );
    } finally {
      setSwitching(false);
    }
  }, [refresh, switchTarget]);

  return (
    <section className="workbuddy-page" role="region" aria-label="账号">
      <header className="page-header">
        <div className="page-header__copy">
          <h1 data-page-title="accounts" tabIndex={-1}>账号</h1>
        </div>
        <div className="page-header__actions">
          <button
            type="button"
            className="btn"
            onClick={() => void refresh(false)}
            disabled={refreshing || loading}
            data-testid="workbuddy-refresh"
          >
            <RefreshCw size={15} className={refreshing ? "icon-spin" : undefined} aria-hidden="true" />
            刷新
          </button>
          <button
            type="button"
            className="btn"
            onClick={() => void runRenew()}
            disabled={renewing || loading || pool.length === 0}
            data-testid="workbuddy-renew-credentials"
            title={pool.length === 0 ? "还没有已保存的账号" : "续期账号池中的登录凭据（有效期充裕时自动跳过）"}
          >
            <RefreshCw size={15} className={renewing ? "icon-spin" : undefined} aria-hidden="true" />
            刷新登录凭据
          </button>
          <button
            type="button"
            className="btn btn--primary"
            onClick={() => setCaptureDialogOpen(true)}
            disabled={captureDisabled || capturing || loading}
            data-testid="workbuddy-capture-button"
            title={captureDisabled ? "WorkBuddy 客户端当前未登录，请先在客户端中登录" : undefined}
          >
            <ShieldCheck size={15} aria-hidden="true" />
            保存当前登录账号
          </button>
        </div>
      </header>

      {loading ? (
        <EmptyState role="status" className="workbuddy-page__empty" description="正在读取 WorkBuddy 登录状态…" />
      ) : loadError ? (
        <EmptyState
          role="alert"
          className="workbuddy-page__empty"
          title="无法读取 WorkBuddy 登录状态"
          description={loadError}
          action={
            <button type="button" className="btn btn--primary" onClick={() => void refresh(true)}>
              重新读取
            </button>
          }
        />
      ) : (
        <>
          {health && (
            <div className="workbuddy-card" data-testid="workbuddy-health-card">
              <div className="workbuddy-card__title">客户端登录状态</div>
              <InfoRow label="登录状态">
                <span
                  className={`workbuddy-badge workbuddy-badge--${healthStatusTone(health.status)}`}
                  data-testid="workbuddy-health-status"
                >
                  {healthStatusLabel(health.status)}
                </span>
              </InfoRow>
              <InfoRow label="账号池">
                {health.captured_in_pool
                  ? "当前登录账号已保存到 Trae Sync 账号池"
                  : "尚未保存到 Trae Sync 账号池"}
              </InfoRow>
              <InfoRow label="登录有效期至">
                {formatUnixTime(health.access_expires_at_unix_seconds) ?? "未知"}
              </InfoRow>
              <InfoRow label="续期凭据有效期至">
                {formatUnixTime(health.refresh_expires_at_unix_seconds) ?? "未知"}
              </InfoRow>
            </div>
          )}

          <div className="workbuddy-card">
            <div className="workbuddy-card__title">账号池（{pool.length}）</div>
            {pool.length === 0 ? (
              <EmptyState
                className="workbuddy-page__empty"
                description="还没有保存 WorkBuddy 账号。在 WorkBuddy 客户端登录后，点击右上角「保存当前登录账号」。"
              />
            ) : (
              <ul className="workbuddy-pool" data-testid="workbuddy-pool-list">
                {pool.map((account) => (
                  <li key={account.profile_id} className="workbuddy-pool__item">
                    <span className="workbuddy-pool__name">
                      {account.screen_name || "WorkBuddy 账号"}
                    </span>
                    <span className="workbuddy-pool__meta">
                      登录有效期至 {formatUnixTime(account.access_token_expires_at_unix_seconds) ?? "未知"}
                      {" · "}
                      更新于 {formatUnixTime(account.last_updated_at_unix_seconds) ?? "未知"}
                    </span>
                    {/* P17-T6/A4 切号：单次确认（覆盖客户端登录态）。 */}
                    <button
                      type="button"
                      className="btn btn--quiet"
                      onClick={() => setSwitchTarget(account.profile_id)}
                      disabled={switching}
                      data-testid={`workbuddy-switch-${account.profile_id}`}
                      title="让 WorkBuddy 客户端切换登录到此账号（对话记录存云端，跟随账号自动切换）"
                    >
                      切换到此账号
                    </button>
                  </li>
                ))}
              </ul>
            )}
          </div>
        </>
      )}

      {captureDialogOpen && (
        <ConfirmDialog
          title="保存当前 WorkBuddy 登录账号"
          lines={[
            "将从本机 WorkBuddy 客户端读取当前登录会话，加密保存到 Trae Sync 的账号池。",
            "只读取登录凭据，不会修改 WorkBuddy 客户端的任何数据。",
            "同一账号重复保存会用最新凭据覆盖旧记录。",
          ]}
          confirmLabel="保存账号"
          busyLabel="保存中…"
          busy={capturing}
          onCancel={() => setCaptureDialogOpen(false)}
          onConfirm={() => void runCapture()}
          testId="workbuddy-capture-dialog"
        />
      )}

      {/* P17-T6/A4 切号单次确认（覆盖性操作：改写客户端登录态）。 */}
      {switchTarget && (
        <ConfirmDialog
          title="切换 WorkBuddy 登录账号"
          lines={[
            "将让 WorkBuddy 客户端退出当前账号并登录到所选账号（写入客户端登录态）。",
            "对话记录保存在云端并按账号隔离，切换后自动跟随新账号。",
            "需要 WorkBuddy 客户端处于运行状态；切换完成后客户端即已登录新账号。",
          ]}
          confirmLabel="确认切换"
          busyLabel="切换中…"
          busy={switching}
          onCancel={() => {
            if (!switching) setSwitchTarget(null);
          }}
          onConfirm={() => void runSwitch()}
          testId="workbuddy-switch-dialog"
        />
      )}
    </section>
  );
}

// ============================================================================
// 签到页：WorkBuddy 直连签到（status/claim 幂等编排在后端）
// ============================================================================

interface WorkBuddyCheckinPageProps {
  active: boolean;
}

export function WorkBuddyCheckinPage({ active }: WorkBuddyCheckinPageProps) {
  const [status, setStatus] = useState<WorkBuddyCheckinStatusDto | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const [claiming, setClaiming] = useState(false);

  const refresh = useCallback(async (initial: boolean) => {
    if (initial) setLoading(true);
    try {
      const next = await fetchWorkBuddyCheckinStatus();
      setStatus(next);
      setLoadError(null);
    } catch (reason: unknown) {
      setLoadError(safeUiErrorMessage(reason, "签到状态暂时不可读取，请稍后重试。"));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    if (!active) return;
    void refresh(true);
  }, [active, refresh]);

  const runClaim = useCallback(async () => {
    setClaiming(true);
    try {
      const claim = await claimWorkBuddyCheckin();
      notify.success(
        `WorkBuddy：签到成功，获得 ${claim.credit} 积分，已连续签到 ${claim.streak_days} 天`,
      );
      await refresh(false);
    } catch (reason: unknown) {
      notify.error(
        `WorkBuddy：签到未完成——${safeUiErrorMessage(reason, "签到未完成，请稍后重试。")}`,
      );
    } finally {
      setClaiming(false);
    }
  }, [refresh]);

  return (
    <section className="workbuddy-page" role="region" aria-label="签到">
      <header className="page-header">
        <div className="page-header__copy">
          <h1 data-page-title="checkin" tabIndex={-1}>签到</h1>
        </div>
        {status && (
          <div className="page-header__actions">
            <button
              type="button"
              className="btn"
              onClick={() => void refresh(false)}
              disabled={loading || claiming}
              data-testid="workbuddy-checkin-refresh"
            >
              <RefreshCw size={15} className={loading ? "icon-spin" : undefined} aria-hidden="true" />
              刷新
            </button>
          </div>
        )}
      </header>

      {loading ? (
        <EmptyState role="status" className="workbuddy-page__empty" description="正在读取签到状态…" />
      ) : loadError ? (
        <EmptyState
          role="alert"
          className="workbuddy-page__empty"
          title="无法读取签到状态"
          description={loadError}
          action={
            <button type="button" className="btn btn--primary" onClick={() => void refresh(true)}>
              重新读取
            </button>
          }
        />
      ) : status ? (
        <div className="workbuddy-card" data-testid="workbuddy-checkin-card">
          <div className="workbuddy-card__title">每日签到</div>
          <InfoRow label="今日状态">
            {status.today_checked_in ? (
              <span className="workbuddy-badge workbuddy-badge--ok">
                <CheckCircle2 size={13} aria-hidden="true" />
                今日已签到
              </span>
            ) : (
              <span className="workbuddy-badge workbuddy-badge--warn">今日未签到</span>
            )}
          </InfoRow>
          <InfoRow label="已连续签到">{status.streak_days} 天</InfoRow>
          <InfoRow label="今日可得">+{status.daily_credit} 积分</InfoRow>
          <InfoRow label="积分余额">{status.total_credits} 积分</InfoRow>
          <div className="workbuddy-card__actions">
            <button
              type="button"
              className="btn btn--primary"
              onClick={() => void runClaim()}
              disabled={status.today_checked_in || claiming}
              data-testid="workbuddy-checkin-button"
            >
              {claiming && <Loader2 size={15} className="icon-spin" aria-hidden="true" />}
              {status.today_checked_in ? "今日已签到" : claiming ? "签到中…" : "立即签到"}
            </button>
          </div>
        </div>
      ) : null}
    </section>
  );
}
