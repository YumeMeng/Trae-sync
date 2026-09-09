import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { BookOpen, CalendarCheck, Database, UserRound } from "lucide-react";
import type { AppPage } from "../components/NavigationRail";
import type { EnvironmentStateDto } from "../types/environment";
import type { WorkspaceStateDto } from "../types/workspace";
import type {
  CheckinCapabilityDto,
  CheckinOverviewEntryDto,
} from "../types/account_switch";
import type { MasterLibraryStatsDto } from "../types/masterLibrary";

interface OverviewPageProps {
  state: WorkspaceStateDto;
  active: boolean;
  onNavigate: (page: AppPage) => void;
}

/**
 * 总览页 = 综合门面（U-3 契约）：一眼知道工具的作用与当前状态，不繁杂，可扩展。
 * 结构：问候 + 证据带（当前账号，原标题栏下沉）+ 签到摘要行（四统计卡）
 * + 快捷操作（去签到/添加账号）+ 工作台状态（统计卡与空态三分支，见下方注释）。
 * 签到摘要为区块化可插拔：真实签到能力不可用时该区块整体隐藏（fixture 预览不展示签到数据）。
 */
export function OverviewPage({
  state,
  active,
  onNavigate,
}: OverviewPageProps) {
  const [checkinOverview, setCheckinOverview] = useState<readonly CheckinOverviewEntryDto[] | null>(null);
  const [checkinRealMode, setCheckinRealMode] = useState(false);
  // P5-4 主库聚合统计：ready 时主库统计卡替换旧工作台统计（fixture 模式
  // 命令报错 → null → 维持旧统计，可插拔语义与签到区块一致）。
  const [masterStats, setMasterStats] = useState<MasterLibraryStatsDto | null>(null);
  // G7 主库真实当前账号：与环境页共用 get_environment_state 读路径。
  const [masterState, setMasterState] = useState<EnvironmentStateDto | null>(null);

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

  // 主库证据：当前账号来自环境真实状态，最近活跃时间来自聚合统计。
  const loadMasterEvidence = useCallback(async () => {
    const [environment, stats] = await Promise.all([
      invoke<EnvironmentStateDto>("get_environment_state").catch(() => null),
      invoke<MasterLibraryStatsDto>("get_master_library_stats").catch(() => null),
    ]);
    setMasterState(environment);
    setMasterStats(stats);
  }, []);

  useEffect(() => {
    if (!active) return;
    void loadCheckinSummary();
    void loadMasterEvidence();
  }, [active, loadCheckinSummary, loadMasterEvidence]);

  const hasHistory =
    state.history.account_count + state.history.project_count + state.history.session_count > 0;

  // —— 空态三分支判定（G1）——
  // ① 有数据：主库统计 ready 且任一计数 > 0（生产常态）；主库不可读时回落旧历史统计。
  // ② 无数据 + 目录就绪：中性空态，引导去环境页查看主库。
  // ③ 目录缺失：真实异常态（如 TRAE 未安装），只在 hero 副文案提示，不出统计与引导。
  const hasMasterData =
    masterStats !== null &&
    masterStats.status === "ready" &&
    (masterStats.session_count > 0 ||
      masterStats.project_count > 0 ||
      masterStats.participating_account_count > 0);
  const hasOverviewData = masterStats?.status === "ready" ? hasMasterData : hasHistory;

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
            : "未找到 TRAE 数据目录，请确认 TRAE 已安装。"}
        </p>
      </header>

      {/* G7 证据带：当前登录账号与主库最近活跃时间，读不到主库时显示未登录。 */}
      <div className="overview-evidence" data-testid="current-account-context">
        <UserRound size={15} strokeWidth={1.9} aria-hidden="true" />
        <span className="overview-evidence__label">当前登录：</span>
        <strong data-testid="current-account-name">
          {masterState?.current_account_name ?? "未登录"}
        </strong>
        {masterStats?.last_active_unix_seconds !== null && masterStats?.last_active_unix_seconds !== undefined && (
          <span className="overview-evidence__meta">
            （最近活跃 {formatLastActive(masterStats.last_active_unix_seconds)}）
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

      {/* 工作台状态（G1 三分支）：有数据 → 统计卡（主库聚合优先，回落旧历史
          统计）；无数据或目录缺失 → 不出统计，空态引导见下方空态行（互斥不同屏）。 */}
      {state.data_location.selected && hasOverviewData ? (
        masterStats?.status === "ready" ? (
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
        )
      ) : null}

      {/* 空态行：目录就绪但无对话（与统计卡互斥），引导去环境页查看主库。 */}
      {state.data_location.selected && !hasOverviewData && (
        <p className="overview-page__empty-hint" data-testid="overview-empty-hint">
          <BookOpen size={14} strokeWidth={2} aria-hidden="true" />
          主库就绪，暂无对话。
        </p>
      )}

      {/* 每屏一个主操作：有数据 → 查看记录（主库模型下历史页即主库视图，
          fixture 预览回落查看历史记录）；无数据但目录就绪 → 去环境页查看主库；
          目录缺失 → 不出主操作（G1：异常态不加引导按钮）。 */}
      <div className="overview-page__actions">
        {state.data_location.selected &&
          (hasOverviewData ? (
            <button
              type="button"
              className="btn btn--primary btn--large"
              onClick={() => onNavigate("history")}
              data-testid="overview-scan-cta"
            >
              {masterStats?.status === "ready" ? (
                <Database size={17} strokeWidth={2} aria-hidden="true" />
              ) : (
                <BookOpen size={17} strokeWidth={2} aria-hidden="true" />
              )}
              {masterStats?.status === "ready" ? "查看主库记录" : "查看历史记录"}
            </button>
          ) : (
            <button
              type="button"
              className="btn btn--primary btn--large"
              onClick={() => onNavigate("environment")}
              data-testid="overview-master-cta"
            >
              <Database size={17} strokeWidth={2} aria-hidden="true" />
              去环境页查看主库
            </button>
          ))}
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
    </section>
  );
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
