import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { BookOpen, CalendarCheck, Database, RefreshCw, ScanSearch, UserRound } from "lucide-react";
import type { AppPage } from "../components/NavigationRail";
import type { WorkspaceStateDto } from "../types/workspace";
import type {
  CheckinCapabilityDto,
  CheckinOverviewEntryDto,
} from "../types/account_switch";
import type { MasterLibraryStatsDto } from "../types/masterLibrary";
import { OperationsPanel } from "../components/OperationsPanel";
import { renderSafeTargetAccount } from "../utils/accountLabel";

interface OverviewPageProps {
  state: WorkspaceStateDto;
  active: boolean;
  onNavigate: (page: AppPage) => void;
  /** 证据带（原标题栏下沉）：当前账号重新检测的状态与回调 */
  accountRefreshState?: "idle" | "loading" | "error";
  accountRefreshError?: string | null;
  onRedetectAccount?: () => Promise<void> | void;
}

/**
 * 总览页 = 综合门面（U-3 契约）：一眼知道工具的作用与当前状态，不繁杂，可扩展。
 * 结构：问候 + 证据带（当前账号，原标题栏下沉）+ 签到摘要行（四统计卡）
 * + 快捷操作（去签到/添加账号）+ 工作台状态（原有历史统计与操作面板保留）。
 * 签到摘要为区块化可插拔：真实签到能力不可用时该区块整体隐藏（fixture 预览不展示签到数据）。
 */
export function OverviewPage({
  state,
  active,
  onNavigate,
  accountRefreshState = "idle",
  accountRefreshError = null,
  onRedetectAccount,
}: OverviewPageProps) {
  const [checkinOverview, setCheckinOverview] = useState<readonly CheckinOverviewEntryDto[] | null>(null);
  const [checkinRealMode, setCheckinRealMode] = useState(false);
  // P5-4 主库聚合统计：ready 时主库统计卡替换旧工作台统计（fixture 模式
  // 命令报错 → null → 维持旧统计，可插拔语义与签到区块一致）。
  const [masterStats, setMasterStats] = useState<MasterLibraryStatsDto | null>(null);

  // 签到摘要数据：真实签到能力可用时读取（账号总览）。
  // 只读一次（active 翻转时刷新）；细粒度实时性由签到页/账号页承担。
  const loadCheckinSummary = useCallback(async () => {
    try {
      const capability = await invoke<CheckinCapabilityDto>("get_checkin_capability");
      const real = capability.real_http_enabled === true;
      setCheckinRealMode(real);
      if (!real) return;
      const entries = await invoke<CheckinOverviewEntryDto[]>("get_checkin_overview");
      setCheckinOverview(entries);
    } catch {
      // 摘要读取失败不阻塞页面：隐藏签到区块比报错更符合门面定位。
      setCheckinRealMode(false);
    }
  }, []);

  // 主库统计：轻量聚合（4 条 SQL）；主库未登录账号时返回引导状态。
  const loadMasterStats = useCallback(async () => {
    try {
      const stats = await invoke<MasterLibraryStatsDto>("get_master_library_stats");
      setMasterStats(stats);
    } catch {
      // fixture 模式 / 读取失败：维持 null（显示旧工作台统计）。
      setMasterStats(null);
    }
  }, []);

  useEffect(() => {
    if (!active) return;
    void loadCheckinSummary();
    void loadMasterStats();
  }, [active, loadCheckinSummary, loadMasterStats]);

  const hasHistory =
    state.history.account_count + state.history.project_count + state.history.session_count > 0;

  // 签到摘要口径：已签 = 缓存 checked_in === true；
  // 积分合计 = 各账号 usage_remaining_credits 求和（未查询账号不计入）。
  const checkedCount = checkinOverview?.filter((entry) => entry.checked_in === true).length ?? 0;
  const accountCount = checkinOverview?.length ?? 0;
  const creditsTotal = checkinOverview?.reduce(
    (sum, entry) => sum + (entry.usage_remaining_credits ?? 0),
    0,
  ) ?? 0;

  return (
    <section className="overview-page" role="region" aria-label="总览">
      <header className="overview-page__hero">
        <h1 data-page-title="overview" tabIndex={-1}>
          {greeting()}，这里是你的 TRAE 工作台
        </h1>
        <p className="overview-page__hero-sub">
          {state.data_location.selected
            ? "历史数据位置已就绪。"
            : "尚未发现 TRAE 数据位置，去历史页开始扫描。"}
        </p>
      </header>

      {/* 证据带（原标题栏下沉，2026-08-27）：当前账号 + 重新检测。
          只读证据可复核：指纹缺失显示“未检测”，带原因时括注短说明。 */}
      <div className="overview-evidence" data-testid="current-account-context">
        <UserRound size={15} strokeWidth={1.9} aria-hidden="true" />
        <span className="overview-evidence__label">当前账号</span>
        <strong data-testid="current-account-name">
          {renderAccountName(state.current_account, state.platform)}
        </strong>
        {onRedetectAccount && (
          <button
            type="button"
            className="btn btn--quiet overview-evidence__redetect"
            onClick={() => void onRedetectAccount()}
            disabled={accountRefreshState === "loading"}
            data-testid="redetect-account-button"
          >
            <RefreshCw size={13} strokeWidth={2} aria-hidden="true" />
            {accountRefreshState === "loading" ? "检测中…" : "重新检测"}
          </button>
        )}
        {accountRefreshError && (
          <span className="overview-evidence__error" role="status">
            {accountRefreshError}
          </span>
        )}
      </div>

      {/* U-3 签到摘要行（可插拔区块）：真实模式四统计卡 + 快捷操作。 */}
      {checkinRealMode && checkinOverview !== null && (
        <section className="overview-checkin" aria-labelledby="overview-checkin-heading" data-testid="overview-checkin">
          <h2 id="overview-checkin-heading">签到与账号</h2>
          <div className="overview-checkin__stats">
            <div className="overview-checkin__stat">
              <strong>{accountCount}</strong>
              <span>账号</span>
            </div>
            <div className="overview-checkin__stat">
              <strong>{checkedCount}<em>/{accountCount}</em></strong>
              <span>今日签到</span>
            </div>
            <div className="overview-checkin__stat" title="各账号 TRAE 模型积分（真实额度）合计">
              <strong>{creditsTotal > 0 ? formatCredits(creditsTotal) : "—"}</strong>
              <span>模型积分合计</span>
            </div>
          </div>
          <div className="overview-checkin__actions">
            <button
              type="button"
              className="btn btn--primary"
              onClick={() => onNavigate("checkin")}
              data-testid="overview-checkin-cta"
            >
              <CalendarCheck size={15} aria-hidden="true" />
              {checkedCount < accountCount ? `去签到（余 ${accountCount - checkedCount}）` : "查看签到"}
            </button>
            <button
              type="button"
              className="btn"
              onClick={() => onNavigate("accounts")}
            >
              <UserRound size={15} aria-hidden="true" />管理账号
            </button>
          </div>
        </section>
      )}

      {/* 工作台状态：主库统计可用时显示主库聚合（P5-4 联动）；否则回落
          旧历史统计（fixture 预览路径）。两套互斥，避免双份统计数字。 */}
      {masterStats?.status === "ready" ? (
        <div className="overview-page__stats" data-testid="overview-stats-master">
          <div className="overview-stat">
            <span className="overview-stat__value">{masterStats.session_count}</span>
            <span className="overview-stat__label">主库对话</span>
          </div>
          <div className="overview-stat">
            <span className="overview-stat__value">{masterStats.project_count}</span>
            <span className="overview-stat__label">主库项目</span>
          </div>
          <div className="overview-stat" title="主库内出现过的账号数（含历史归属）">
            <span className="overview-stat__value">{masterStats.participating_account_count}</span>
            <span className="overview-stat__label">参与账号</span>
          </div>
          {masterStats.last_active_unix_seconds !== null && (
            <div className="overview-stat">
              <span className="overview-stat__value overview-stat__value--time">
                {formatLastActive(masterStats.last_active_unix_seconds)}
              </span>
              <span className="overview-stat__label">最近活跃</span>
            </div>
          )}
        </div>
      ) : (
        <div className="overview-page__stats" data-testid="overview-stats">
          <div className="overview-stat">
            <span className="overview-stat__value">{state.history.account_count}</span>
            <span className="overview-stat__label">账号</span>
          </div>
          <div className="overview-stat">
            <span className="overview-stat__value">{state.history.project_count}</span>
            <span className="overview-stat__label">项目</span>
          </div>
          <div className="overview-stat">
            <span className="overview-stat__value">{state.history.session_count}</span>
            <span className="overview-stat__label">对话</span>
          </div>
        </div>
      )}

      {/* 每屏一个主操作：主库模型下历史页即主库视图；fixture 预览保留扫描动词 */}
      <div className="overview-page__actions">
        <button
          type="button"
          className="btn btn--primary btn--large"
          onClick={() => onNavigate("history")}
          data-testid="overview-scan-cta"
        >
          {masterStats?.status === "ready" ? (
            <Database size={17} strokeWidth={2} aria-hidden="true" />
          ) : (
            <ScanSearch size={17} strokeWidth={2} aria-hidden="true" />
          )}
          {masterStats?.status === "ready"
            ? "查看主库记录"
            : hasHistory
              ? "扫描本机最新记录"
              : "开始扫描本机记录"}
        </button>
        {!(checkinRealMode && checkinOverview !== null) && (
          <button
            type="button"
            className="btn"
            onClick={() => onNavigate("accounts")}
            data-testid="overview-accounts-cta"
          >
            <UserRound size={16} strokeWidth={2} aria-hidden="true" />
            管理账号
          </button>
        )}
      </div>

      {!hasHistory && (
        <p className="overview-page__empty-hint">
          <BookOpen size={14} strokeWidth={2} aria-hidden="true" />
          还没有任何历史记录。扫描后，这里会显示账号、项目和对话的统计。
        </p>
      )}

      {/* 最近活动：操作记录与执行进度并入总览，不再单独占一页 */}
      <OperationsPanel capabilities={state.capabilities} active={active} compact={true} />
    </section>
  );
}

/// 证据带账号名：生产模式展示不可逆指纹，fixture 预览展示原始值；
/// 指纹缺失回落“未检测”，带原因时括注短说明（逻辑自 TitleBar 迁入）。
function renderAccountName(
  account: WorkspaceStateDto["current_account"],
  platform: WorkspaceStateDto["platform"],
): string {
  const productionMode = platform.adapter_implemented;
  const rawIdentifier = productionMode
    ? renderSafeTargetAccount(account)
    : account.user_fingerprint ?? "";
  // 指纹缺失时回落为空，展示层统一显示“未检测”，避免绕口兜底文案。
  const identifier = rawIdentifier === "安全指纹不可用" ? "" : rawIdentifier;
  if (account.detected && !account.unavailable_reason) return identifier;
  return identifier
    ? `${identifier}（${renderAccountReason(account.unavailable_reason)}）`
    : "未检测";
}

/// 把后端稳定原因码转换为证据带短提示（自 TitleBar 迁入）。
function renderAccountReason(reason: string | null): string {
  switch (reason) {
    case "authorization_required":
    case "authorization_mismatch":
      return "读取授权已失效";
    case "expired":
      return "账号信息已过期";
    case "single_source":
      return "信息不足";
    case "conflict":
      return "账号来源冲突";
    case "fingerprint_changed":
      return "账号信息已变化";
    default:
      return "暂不可用";
  }
}

/// 按本地时间生成问候语，避免时区错位。
function greeting(): string {
  const hour = new Date().getHours();
  if (hour < 5) return "夜深了";
  if (hour < 12) return "早上好";
  if (hour < 18) return "下午好";
  return "晚上好";
}

/// 积分合计格式化：保留 1 位小数（与账号页同口径）。
function formatCredits(value: number): string {
  const rounded = Math.round(value * 10) / 10;
  return Number.isInteger(rounded) ? String(rounded) : rounded.toFixed(1);
}

/// 最近活跃时间：相对时间短文案（刚刚 / N 分钟前 / N 小时前 / N 天前），
/// 超过 30 天回落为本地日期字符串（统计卡空间有限，不给完整时间戳）。
function formatLastActive(unixSeconds: number): string {
  const elapsedSeconds = Math.max(0, Date.now() / 1000 - unixSeconds);
  if (elapsedSeconds < 60) return "刚刚";
  if (elapsedSeconds < 3600) return `${Math.floor(elapsedSeconds / 60)} 分钟前`;
  if (elapsedSeconds < 86400) return `${Math.floor(elapsedSeconds / 3600)} 小时前`;
  if (elapsedSeconds < 86400 * 30) return `${Math.floor(elapsedSeconds / 86400)} 天前`;
  return new Date(unixSeconds * 1000).toLocaleDateString();
}
