import { useCallback, useEffect, useState } from "react";
import { Activity, RefreshCw } from "lucide-react";
import type { WorkBuddyDeepHealthDto, WorkBuddySessionHealthDto } from "../types/product";
import {
  checkWorkBuddyLoginHealth,
  loadWorkBuddySessionHealth,
} from "../platform/workbuddyAdapter";
import { EmptyState } from "./shared/EmptyState";
import { ProductBadge } from "./shared/ProductBadge";
import { InfoRow } from "./WorkBuddyPanel";
import { notify } from "../platform/notify";
import { safeUiErrorMessage } from "../utils/safeUiError";

/**
 * WorkBuddy 环境页轻量版（P19-B6，IA §1.5 客户端卡 + 健康卡；拍板决策 10：
 * 只读状态卡，不含启停动作）。多实例环境行 = pending（实验票结论前诚实说明）。
 * 数据面：get_workbuddy_session_health（文件面只读）+ check_workbuddy_login_health
 * （深度健康：客户端运行中时 wbipc 只读回读，零写副作用）。
 */
export function WorkBuddyEnvironmentPage({ active }: { active: boolean }) {
  const [health, setHealth] = useState<WorkBuddySessionHealthDto | null>(null);
  const [deepHealth, setDeepHealth] = useState<WorkBuddyDeepHealthDto | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const [checking, setChecking] = useState(false);

  const refresh = useCallback(async (initial: boolean) => {
    if (initial) setLoading(true);
    try {
      const next = await loadWorkBuddySessionHealth();
      setHealth(next);
      setLoadError(null);
      // 深度健康随刷新自动执行一次（只读；客户端未运行落 client_offline）。
      await runDeepHealthReadonly();
    } catch (reason: unknown) {
      setLoadError(safeUiErrorMessage(reason, "WorkBuddy 客户端状态暂时不可读取，请稍后重试。"));
    } finally {
      setLoading(false);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const runDeepHealthReadonly = useCallback(async () => {
    setChecking(true);
    try {
      setDeepHealth(await checkWorkBuddyLoginHealth());
    } catch (reason: unknown) {
      notify.error(`WorkBuddy：健康检测未完成——${safeUiErrorMessage(reason, "请稍后重试。")}`);
    } finally {
      setChecking(false);
    }
  }, []);

  useEffect(() => {
    if (!active) return;
    void refresh(true);
  }, [active, refresh]);

  const fileStatusLabel = (status: WorkBuddySessionHealthDto["status"]): string => {
    switch (status) {
      case "active": return "已登录（登录文件有效）";
      case "access_expired": return "登录已过期（客户端运行中会自动刷新）";
      case "fully_expired": return "登录完全过期（请在客户端重新登录）";
      case "logged_out": return "客户端已登出";
      case "file_missing": return "未找到登录文件（客户端从未登录）";
      default: return "状态未知";
    }
  };

  return (
    <section className="workbuddy-page" role="region" aria-label="环境">
      <header className="page-header">
        <div className="page-header__copy">
          <h1 data-page-title="environment" tabIndex={-1}>环境</h1>
          <ProductBadge productId="workbuddy" />
        </div>
        {!loadError && (
          <div className="page-header__actions">
            <button
              type="button"
              className="btn"
              onClick={() => void refresh(false)}
              disabled={loading || checking}
              data-testid="workbuddy-env-refresh"
            >
              <RefreshCw size={15} className={loading ? "icon-spin" : undefined} aria-hidden="true" />
              刷新
            </button>
          </div>
        )}
      </header>

      {loading ? (
        <EmptyState role="status" className="workbuddy-page__empty" description="正在读取客户端状态…" />
      ) : loadError ? (
        <EmptyState
          role="alert"
          className="workbuddy-page__empty"
          title="无法读取 WorkBuddy 客户端状态"
          description={loadError}
          action={
            <button type="button" className="btn btn--primary" onClick={() => void refresh(true)}>
              重新读取
            </button>
          }
        />
      ) : health ? (
        <>
          {/* 客户端连接卡（IA §1.5；决策 10 只读：版本/安装路径无数据面命令，
              诚实不渲染；不含启停动作）。 */}
          <div className="workbuddy-card" data-testid="workbuddy-env-client-card">
            <div className="workbuddy-card__title">客户端连接</div>
            <InfoRow label="登录文件状态">{fileStatusLabel(health.status)}</InfoRow>
            <InfoRow label="客户端运行状态">
              {deepHealth ? (
                <span
                  className={`slot-badge ${deepHealth.live_status === "verified" ? "slot-badge--ok" : deepHealth.live_status === "probe_failed" ? "slot-badge--danger" : "slot-badge--idle"}`}
                  data-testid="workbuddy-env-client-runtime"
                >
                  <i className="slot-badge__dot" aria-hidden="true" />
                  {deepHealth.live_status === "verified"
                    ? "运行中（凭据实时回读通过）"
                    : deepHealth.live_status === "client_offline"
                      ? "未运行"
                      : "运行中但凭据回读异常"}
                </span>
              ) : checking ? "检测中…" : "未检测"}
            </InfoRow>
            <InfoRow label="登录有效期至">
              {formatUnixTime(health.access_expires_at_unix_seconds) ?? "未知"}
            </InfoRow>
            <InfoRow label="续期凭据有效期至">
              {formatUnixTime(health.refresh_expires_at_unix_seconds) ?? "未知"}
            </InfoRow>
            <InfoRow label="工具账号池">
              {health.captured_in_pool ? "当前登录账号已保存到账号池" : "尚未保存到账号池"}
            </InfoRow>
          </div>

          {/* 登录域健康卡（只读检测；deep 结果常驻）。 */}
          <div className="workbuddy-card" data-testid="workbuddy-env-health-card">
            <div className="workbuddy-card__title">登录域健康</div>
            <InfoRow label="本地登录态检测">
              <span
                className={`slot-badge ${health.status === "active" ? "slot-badge--ok" : health.status === "access_expired" ? "slot-badge--warn" : "slot-badge--danger"}`}
              >
                <i className="slot-badge__dot" aria-hidden="true" />
                {health.status === "active" ? "正常" : health.status === "access_expired" ? "将过期" : "异常"}
              </span>
            </InfoRow>
            {deepHealth && (
              <InfoRow label="深度检测（实时凭据回读）">
                <span
                  className={`slot-badge ${deepHealth.live_status === "verified" ? "slot-badge--ok" : deepHealth.live_status === "probe_failed" ? "slot-badge--danger" : "slot-badge--idle"}`}
                  data-testid="workbuddy-env-deep-result"
                >
                  <i className="slot-badge__dot" aria-hidden="true" />
                  {deepHealth.live_status === "verified"
                    ? "已验证可用"
                    : deepHealth.live_status === "client_offline"
                      ? "客户端未运行，仅本地检测"
                      : "探针失败，建议重启客户端"}
                </span>
              </InfoRow>
            )}
            <div className="workbuddy-card__actions">
              <button
                type="button"
                className="btn"
                onClick={() => void runDeepHealthReadonly()}
                disabled={checking}
                data-testid="workbuddy-env-deep-health-button"
                title="检测本地登录态与客户端实时凭据（只读，不修改任何数据）"
              >
                <Activity size={15} className={checking ? "icon-spin" : undefined} aria-hidden="true" />
                {checking ? "检测中…" : "立即检测"}
              </button>
            </div>
          </div>

          {/* 多实例环境行 = pending（实验票结论前诚实说明；IA §1.5 能力开关）。 */}
          <EmptyState
            panel
            className="workbuddy-page__empty"
            title="多实例环境管理适配中"
            description="WorkBuddy 的多实例（--user-data-dir）可行性待实验票结论，实验通过前此处保持诚实说明。单实例客户端状态已在上方卡片呈现。"
          />
        </>
      ) : null}
    </section>
  );
}

/** Unix 秒 → 本地「日期 时:分」；0 = 未知返回 null（与账号页同口径）。 */
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
