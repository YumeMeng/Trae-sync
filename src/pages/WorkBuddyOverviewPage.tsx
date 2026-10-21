import { useCallback, useEffect, useState } from "react";
import { CalendarCheck, Database, RefreshCw, UserRound } from "lucide-react";
import type { AppPage } from "../components/NavigationRail";
import type { WorkBuddyCheckinAccountStatusDto, WorkBuddyCheckinParticipationDto } from "../types/product";
import type { WorkBuddyLibraryStatsDto } from "../types/workbuddyLibrary";
import {
  fetchWorkBuddyCheckinOverview,
  fetchWorkBuddyLibraryStats,
  listWorkBuddyCheckinAccounts,
} from "../platform/workbuddyAdapter";

interface WorkBuddyOverviewPageProps {
  active: boolean;
  onNavigate: (page: AppPage) => void;
}

/**
 * WorkBuddy 总览页（P19-B6，IA 草案 §1.1 工作台模板）：
 * 问候横条 + 签到与账号统计卡（参与集 + 逐账号状态聚合）+ 会话统计卡
 * （WB 对话库 workbuddy.db 聚合统计，P19-B3 命令面）+ 查看对话库入口。
 * 数据读取失败静默降级（隐藏对应区块），与总览门面定位一致。
 */
export function WorkBuddyOverviewPage({ active, onNavigate }: WorkBuddyOverviewPageProps) {
  // 签到与账号：参与集（账号数）+ 逐账号状态（今日已签/积分合计）。
  const [participation, setParticipation] = useState<readonly WorkBuddyCheckinParticipationDto[] | null>(null);
  const [checkinRows, setCheckinRows] = useState<readonly WorkBuddyCheckinAccountStatusDto[] | null>(null);
  // 会话统计卡（P19-B3 get_workbuddy_library_stats）。
  const [libraryStats, setLibraryStats] = useState<WorkBuddyLibraryStatsDto | null>(null);
  // 积分未获取占位行的刷新动作（重新拉取三路 summary）。
  const [refreshing, setRefreshing] = useState(false);

  const loadSummary = useCallback(async () => {
    const [participationResult, overviewResult, statsResult] = await Promise.allSettled([
      listWorkBuddyCheckinAccounts(),
      fetchWorkBuddyCheckinOverview(null),
      fetchWorkBuddyLibraryStats(),
    ]);
    setParticipation(participationResult.status === "fulfilled" ? participationResult.value : null);
    setCheckinRows(overviewResult.status === "fulfilled" ? overviewResult.value : null);
    setLibraryStats(statsResult.status === "fulfilled" ? statsResult.value : null);
  }, []);

  const handleRefreshSummary = useCallback(() => {
    if (refreshing) return;
    setRefreshing(true);
    void loadSummary().finally(() => setRefreshing(false));
  }, [loadSummary, refreshing]);

  useEffect(() => {
    if (!active) return;
    void loadSummary();
  }, [active, loadSummary]);

  const accountCount = participation?.length ?? 0;
  const knownRows = checkinRows?.filter((row) => row.error === null) ?? [];
  const checkedCount = knownRows.filter((row) => row.today_checked_in === true).length;
  // P19.2③ 积分余额对齐（be 5dc3b72）：主展示 = credits_balance（billing 资源域
  // 真实余额）；total_credits 语义已纠正为「本期签到累计」（活动域，只增不减），
  // 次要展示。两行各自独立占位化（P19.2② 假 0 纪律），禁止静默 0。
  const balanceKnownCount = knownRows.filter((row) => row.credits_balance !== null).length;
  const balanceTotal = knownRows.reduce((sum, row) => sum + (row.credits_balance ?? 0), 0);
  const balancePartial = balanceKnownCount > 0 && balanceKnownCount < accountCount;
  const creditsKnownCount = knownRows.filter((row) => row.total_credits !== null).length;
  const creditsTotal = knownRows.reduce((sum, row) => sum + (row.total_credits ?? 0), 0);
  const creditsPartial = creditsKnownCount > 0 && creditsKnownCount < accountCount;

  return (
    <section className="overview-page" role="region" aria-label="总览">
      <header className="overview-page__hero">
        <h1 data-page-title="overview" tabIndex={-1}>
          {greeting()}，这里是你的 WorkBuddy 工作台
        </h1>
        <p className="overview-page__hero-sub">
          对话库与签到状态概览。数据来自本机 workbuddy.db 与账号池，工具只读不碰云端。
        </p>
      </header>

      {/* 签到与账号统计卡（IA §1.1 签到统计卡；参与集 + 逐账号状态聚合）。 */}
      {participation !== null && checkinRows !== null && (
        <section className="overview-checkin" aria-labelledby="wb-overview-checkin-heading" data-testid="wb-overview-checkin">
          <h2 id="wb-overview-checkin-heading">签到与账号</h2>
          <div className="overview-checkin__stats">
            <div className="overview-checkin__stat">
              <strong>{accountCount}</strong>
              <span>账号</span>
            </div>
            <div className="overview-checkin__stat">
              <strong>{checkedCount}<em>/{accountCount}</em></strong>
              <span>今日签到</span>
            </div>
            {/* P19.2③ 拆两行：积分余额（credits_balance，主展示）+ 本期签到累计
                （total_credits，活动域语义）。各自独立占位化，禁止静默 0。
                P19.2⑤：全部余额查询失败 → 占位注明失败账号计数（balance_error 汇总）。 */}
            {accountCount > 0 && balanceKnownCount === 0 ? (
              <div className="overview-checkin__stat" data-testid="wb-overview-credits-unknown">
                <strong>余额查询失败</strong>
                <span>积分余额（{accountCount} 个账号均未获取到余额）</span>
                <button
                  type="button"
                  className="btn btn--quiet"
                  onClick={handleRefreshSummary}
                  disabled={refreshing}
                  data-testid="wb-overview-credits-refresh"
                  title="重新读取各账号积分余额；仍失败的原因可逐账号到账号详情页查看"
                >
                  <RefreshCw size={13} className={refreshing ? "icon-spin" : undefined} aria-hidden="true" />
                  点击刷新
                </button>
              </div>
            ) : (
              <div className="overview-checkin__stat" title="各账号积分余额合计（billing 资源域，随对话消耗减少）">
                <strong>{accountCount > 0 ? balanceTotal : "—"}</strong>
                <span>积分余额{balancePartial ? `（${balanceKnownCount}/${accountCount} 账号已获取）` : ""}</span>
              </div>
            )}
            {accountCount > 0 && creditsKnownCount === 0 ? (
              <div className="overview-checkin__stat" data-testid="wb-overview-accum-unknown">
                <strong>未获取</strong>
                <span>本期签到累计</span>
              </div>
            ) : (
              <div className="overview-checkin__stat" title="本期签到活动累计获得（只增不减，非余额）">
                <strong>{accountCount > 0 ? creditsTotal : "—"}</strong>
                <span>本期签到累计{creditsPartial ? `（${creditsKnownCount}/${accountCount} 账号已获取）` : ""}</span>
              </div>
            )}
          </div>
          <div className="overview-checkin__actions">
            <button
              type="button"
              className="btn btn--primary"
              onClick={() => onNavigate("checkin")}
              data-testid="wb-overview-checkin-cta"
            >
              <CalendarCheck size={15} aria-hidden="true" />
              {checkedCount < accountCount ? `去签到（余 ${accountCount - checkedCount}）` : "查看签到"}
            </button>
            <button
              type="button"
              className="btn"
              onClick={() => onNavigate("accounts")}
              data-testid="wb-overview-accounts-cta"
            >
              <UserRound size={15} aria-hidden="true" />管理账号
            </button>
          </div>
        </section>
      )}

      {/* 会话统计卡（IA §1.1 库统计卡）：WB 对话库聚合 + 查看对话库入口。 */}
      {libraryStats !== null && (
        <div className="overview-page__stats" data-testid="wb-overview-stats-library">
          <div className="overview-stat">
            <span className="overview-stat__value">{libraryStats.session_count}</span>
            <span className="overview-stat__label">库内会话</span>
          </div>
          <div className="overview-stat">
            <span className="overview-stat__value">{libraryStats.project_count}</span>
            <span className="overview-stat__label">项目（cwd）</span>
          </div>
          <div className="overview-stat">
            <span className="overview-stat__value">{libraryStats.account_count}</span>
            <span className="overview-stat__label">库内账号</span>
          </div>
          {libraryStats.last_active_unix_seconds !== null && (
            <div className="overview-stat">
              <span className="overview-stat__value overview-stat__value--time">
                {formatLastActive(libraryStats.last_active_unix_seconds)}
              </span>
              <span className="overview-stat__label">最近活跃</span>
            </div>
          )}
        </div>
      )}

      {libraryStats === null && (
        <p className="overview-page__empty-hint" data-testid="wb-overview-library-hint">
          <Database size={14} strokeWidth={2} aria-hidden="true" />
          对话库统计暂不可读取（库未生成或 WorkBuddy 数据目录缺失）。
        </p>
      )}

      {/* 每屏一个主操作：查看对话库（进入库页）。 */}
      <div className="overview-page__actions">
        <button
          type="button"
          className="btn btn--primary btn--large"
          onClick={() => onNavigate("master-library")}
          data-testid="wb-overview-library-cta"
        >
          <Database size={17} strokeWidth={2} aria-hidden="true" />
          查看对话库
        </button>
      </div>
    </section>
  );
}

/// 按本地时间生成问候语（与 TRAE 总览页同一口径）。
function greeting(): string {
  const hour = new Date().getHours();
  if (hour < 5) return "夜深了";
  if (hour < 12) return "早上好";
  if (hour < 18) return "下午好";
  return "晚上好";
}

/// 最近活跃时间：相对时间短文案（与 TRAE 总览页同一口径）。
function formatLastActive(unixSeconds: number): string {
  const elapsedSeconds = Math.max(0, Date.now() / 1000 - unixSeconds);
  if (elapsedSeconds < 60) return "刚刚";
  if (elapsedSeconds < 3600) return `${Math.floor(elapsedSeconds / 60)} 分钟前`;
  if (elapsedSeconds < 86400) return `${Math.floor(elapsedSeconds / 3600)} 小时前`;
  if (elapsedSeconds < 86400 * 30) return `${Math.floor(elapsedSeconds / 86400)} 天前`;
  return new Date(unixSeconds * 1000).toLocaleDateString();
}
