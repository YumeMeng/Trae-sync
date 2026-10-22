import { useCallback, useEffect, useState } from "react";
import { BookOpen, CalendarCheck, Database, RefreshCw, UserRound } from "lucide-react";
import type { AppPage } from "../../components/NavigationRail";
import type { WorkspaceStateDto } from "../../types/workspace";
import { loadOverview, type OverviewSnapshot } from "../../platform/overview";
import { EMPTY_VIEW, overviewViewFor } from "./stats";
import { Badge } from "../../ui/Badge";
import { EmptyState } from "../../ui/EmptyState";
import { SectionHeader } from "../../ui/SectionHeader";
import { Stat } from "../../ui/Stat";
import { StatStrip } from "../../ui/StatStrip";

/** 总览页属性：槽位接缝按统一形状调用（productId 由接缝注入）。 */
export interface OverviewPageProps {
  /** 工作台状态（TRAE 数据面的空态判定用；WB 数据面不需要）。 */
  state?: WorkspaceStateDto;
  active: boolean;
  /** 当前产品协议值：只用于查声明式配置，页面内不做任何产品判断。 */
  productId: string;
  onNavigate: (page: AppPage) => void;
}

/** 空态图标：配置只给语义键，具体图标在本层映射（不是产品判断）。 */
const EMPTY_ICONS = {
  book: <BookOpen size={14} strokeWidth={2} />,
  database: <Database size={14} strokeWidth={2} />,
} as const;

/** 指标样式钩子：整页统一（两产品共用同一套视觉，差异只在指标集）。 */
const STAT_CLASSES = {
  root: "overview-stat",
  value: "overview-stat__value",
  label: "overview-stat__label",
  note: "overview-stat__label",
} as const;

/**
 * 总览页（P4a 合并页，六页迁移的样板）：TRAE 与 WorkBuddy 共用这一份实现。
 *
 * 结构（UI-DIRECTION §5）：标题「总览」+ 当前登录一行证据
 * + 摘要条（3–5 个指标）+ 唯一主行动 +（仅当有事）一条待处理提示。
 *
 * 产品差异**全部**来自 features/overview 的声明式配置（stats.ts 登记表）：
 * 本文件不出现 productId === / isWorkBuddy 之类判断，也不直接 invoke(——
 * 数据一律经 platform/overview 读取。
 */
export function OverviewPage({ state, active, productId, onNavigate }: OverviewPageProps) {
  const [snapshot, setSnapshot] = useState<OverviewSnapshot | null>(null);
  const [refreshing, setRefreshing] = useState(false);

  const load = useCallback(async () => {
    setSnapshot(await loadOverview(productId));
  }, [productId]);

  // 产品切换即重拉（同一份实现不重挂载，故以 productId 为依赖）。
  useEffect(() => {
    if (!active) return;
    void load();
  }, [active, load]);

  const handleRefresh = useCallback(() => {
    if (refreshing) return;
    setRefreshing(true);
    void load().finally(() => setRefreshing(false));
  }, [load, refreshing]);

  // 快照未到位时按中性空视图渲染：不写「加载中…」，也不假装有数据。
  const view = snapshot === null ? EMPTY_VIEW : overviewViewFor(productId, snapshot, state);

  return (
    <section className="overview-page" role="region" aria-label="总览">
      {/* 标题统一为「总览」：产品名已在标题栏，页面内不再重复；问候语已删。 */}
      <SectionHeader
        title="总览"
        level={1}
        focusAnchor="overview"
        className="overview-page__hero"
      />

      {/* 当前登录：一行证据，不是段落（读不到主库时显示未登录）。 */}
      {view.evidence !== null && (
        <div className="overview-evidence" data-testid="current-account-context">
          <UserRound size={15} strokeWidth={1.9} aria-hidden="true" />
          <span className="overview-evidence__label">当前登录：</span>
          <strong data-testid="current-account-name">{view.evidence.accountName}</strong>
          {view.evidence.lastActive !== null && (
            <span className="overview-evidence__meta">
              （最近活跃 {view.evidence.lastActive}）
            </span>
          )}
        </div>
      )}

      {/* 摘要条：每产品的指标集来自声明式配置（3–5 个）。 */}
      {view.groups.map((group) => (
        <section
          key={group.testId}
          className="overview-page__group"
          data-testid={group.testId}
          aria-label={group.title}
        >
          {group.title !== undefined && <h2 className="page-header__eyebrow">{group.title}</h2>}
          <StatStrip className={group.stripClass} label={group.title}>
            {group.stats.map((stat) => (
              <Stat
                key={stat.key}
                value={stat.value}
                label={stat.label}
                note={stat.note}
                hint={stat.hint}
                testId={stat.testId}
                classes={STAT_CLASSES}
              />
            ))}
          </StatStrip>
        </section>
      ))}

      {/* 空态：一句结论（≤25 字），不给段落。 */}
      {view.empty !== null && (
        <EmptyState
          title={view.empty.title}
          icon={EMPTY_ICONS[view.empty.icon]}
          className="overview-page__empty-hint"
          testId={view.empty.testId}
        />
      )}

      {/* 待处理提示 + 刷新入口：仅当有事才出，最多一条提示。 */}
      {(view.pending !== null || view.refresh !== null) && (
        <div className="overview-page__actions" data-testid="overview-pending-row">
          {view.pending !== null && (
            <>
              <Badge tone="warn">待处理</Badge>
              <button
                type="button"
                className="btn btn--quiet"
                onClick={() => onNavigate(view.pending!.page)}
                data-testid={view.pending.testId}
              >
                <CalendarCheck size={14} aria-hidden="true" />
                {view.pending.label}
              </button>
            </>
          )}
          {view.refresh !== null && (
            <button
              type="button"
              className="btn btn--quiet"
              onClick={handleRefresh}
              disabled={refreshing}
              data-testid={view.refresh.testId}
              title="重新读取各账号积分余额；仍失败可到账号详情查看"
            >
              <RefreshCw
                size={13}
                className={refreshing ? "icon-spin" : undefined}
                aria-hidden="true"
              />
              {view.refresh.label}
            </button>
          )}
        </div>
      )}

      {/* 动作层：唯一主行动 + 一个次要入口。 */}
      <div className="overview-page__actions">
        {view.primary !== null && (
          <button
            type="button"
            className="btn btn--primary btn--large"
            onClick={() => onNavigate(view.primary!.page)}
            data-testid={view.primary.testId}
          >
            <Database size={17} strokeWidth={2} aria-hidden="true" />
            {view.primary.label}
          </button>
        )}
        {view.secondary !== null && (
          <button
            type="button"
            className="btn"
            onClick={() => onNavigate(view.secondary!.page)}
            data-testid={view.secondary.testId}
          >
            <UserRound size={16} strokeWidth={2} aria-hidden="true" />
            {view.secondary.label}
          </button>
        )}
      </div>
    </section>
  );
}