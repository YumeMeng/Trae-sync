import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { ArrowLeft, RefreshCw } from "lucide-react";
import type { AppPage } from "../components/NavigationRail";
import { HistoryWorkbench } from "../components/HistoryWorkbench";
import { PluginWorkbench } from "../components/PluginWorkbench";
import { MasterVerificationPanel } from "../components/MasterVerificationPanel";
import type { EnvironmentStateDto } from "../types/environment";
import type { MasterBackupChainDto, MasterLibraryStatsDto } from "../types/masterLibrary";
import { safeUiErrorMessage } from "../utils/safeUiError";

// ============================================================================
// 主库详情页 shell（G20 三 tab 平级结构，2026-09-04 重构）：
// 顶部只留面包屑式标题（主库名 + 返回环境页）+ 刷新；
// tabs = 对话列表（默认，HistoryWorkbench embedded 两栏）· 插件 · 库信息
//（原基础信息：路径/大小/账号/统计/最近活跃/最近备份 + 备份对比）。
// 原数据校验 tab 已取消：备份对比移入库信息 tab，接力台账核对由
// P8-5 主库自检（G18）承载。
// ============================================================================

/** 详情页 tab：对话列表复用历史页；插件为 P5-8b；库信息为 G20。 */
type MasterDetailTab = "sessions" | "plugins" | "info";

interface MasterLibraryDetailProps {
  /** 页面可见时才读取数据；隐藏时停止轮询（embedded 历史视图随 active 联动）。 */
  active: boolean;
  /** 返回环境页（详情页不在导航栏，返回是唯一退路）。 */
  onNavigate: (page: AppPage) => void;
}

export function MasterLibraryDetail({ active, onNavigate }: MasterLibraryDetailProps) {
  const [envState, setEnvState] = useState<EnvironmentStateDto | null>(null);
  // 统计与备份链读取失败时静默降级为 null（对应分区显示 —），不阻塞页面。
  const [stats, setStats] = useState<MasterLibraryStatsDto | null>(null);
  const [backupChain, setBackupChain] = useState<MasterBackupChainDto | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [tab, setTab] = useState<MasterDetailTab>("sessions");
  // 懒挂载：App 各页面常驻 DOM（[hidden] 切换），若详情页一启动就渲染内嵌
  // HistoryWorkbench，会与历史页实例产生重复的 data-testid（测试定位冲突），
  // 也会白白挂载一份两栏视图。首次激活后再挂载，之后保持（切页不丢状态）。
  const [contentMounted, setContentMounted] = useState(false);

  const load = useCallback(async () => {
    const nextEnv = await invoke<EnvironmentStateDto>("get_environment_state");
    setEnvState(nextEnv);
    setError(null);
    // 统计/备份链与状态同读同刷；两者相互独立，任一失败不影响其余展示。
    const [nextStats, nextBackups] = await Promise.all([
      invoke<MasterLibraryStatsDto>("get_master_library_stats").catch(() => null),
      invoke<MasterBackupChainDto>("get_master_backup_chain").catch(() => null),
    ]);
    setStats(nextStats);
    setBackupChain(nextBackups);
  }, []);

  useEffect(() => {
    if (!active) return;
    setContentMounted(true);
    let cancelled = false;
    void load().catch((reason: unknown) => {
      if (!cancelled) setError(safeUiErrorMessage(reason, "主库信息暂时不可读取，请稍后重试。"));
    });
    return () => { cancelled = true; };
  }, [active, load]);

  const latestBackup = backupChain?.backups[0] ?? null;
  const currentName = envState?.current_account_name ?? null;
  // 统计 ready 才有意义；未登录/无库/失败时计数区显示 —。
  const statsReady = stats?.status === "ready";

  return (
    <section className="master-detail" role="region" aria-label="主库详情">
      <header className="page-header">
        <div className="page-header__copy">
          <div className="master-detail__title-row">
            <button
              className="btn btn--quiet"
              type="button"
              onClick={() => onNavigate("environment")}
              data-testid="master-detail-back"
            >
              <ArrowLeft size={15} aria-hidden="true" />返回
            </button>
            <h1 data-page-title="master-library" tabIndex={-1}>主库</h1>
          </div>
        </div>
        <div className="page-header__actions">
          <button
            className="btn"
            type="button"
            onClick={() => void load().catch(() => undefined)}
            data-testid="master-detail-refresh"
            title="重新读取主库信息"
          >
            <RefreshCw size={15} aria-hidden="true" />刷新
          </button>
        </div>
      </header>

      {error && <p className="workbench__error" role="alert">{error}</p>}

      {/* tabs 内容懒挂载：未访问过详情页前不渲染（见 contentMounted 注释）。 */}
      {contentMounted && (
      <>
      <div className="master-detail__tabs" role="tablist" aria-label="主库内容">
        <button
          type="button"
          role="tab"
          aria-selected={tab === "sessions"}
          className={`master-tab ${tab === "sessions" ? "master-tab--active" : ""}`}
          onClick={() => setTab("sessions")}
          data-testid="master-tab-sessions"
        >
          对话列表
        </button>
        <button
          type="button"
          role="tab"
          aria-selected={tab === "plugins"}
          className={`master-tab ${tab === "plugins" ? "master-tab--active" : ""}`}
          onClick={() => setTab("plugins")}
          data-testid="master-tab-plugins"
        >
          插件
        </button>
        <button
          type="button"
          role="tab"
          aria-selected={tab === "info"}
          className={`master-tab ${tab === "info" ? "master-tab--active" : ""}`}
          onClick={() => setTab("info")}
          data-testid="master-tab-info"
        >
          库信息
        </button>
      </div>

      <div className="master-detail__tabpanels">
        <div className="master-detail__tabpanel" hidden={tab !== "sessions"}>
          <HistoryWorkbench active={active && tab === "sessions"} embedded onNavigate={onNavigate} />
        </div>
        {/* 插件 tab（P5-8b）：已装清单 + 对账 + 市场浏览 + 装/卸。 */}
        <div className="master-detail__tabpanel" hidden={tab !== "plugins"}>
          <PluginWorkbench active={active && tab === "plugins"} />
        </div>
        {/* 库信息 tab（G20）：原基础信息 + 备份对比（原数据校验 tab 的
            备份区块；接力台账核对由 P8-5 主库自检承载）。 */}
        <div className="master-detail__tabpanel" hidden={tab !== "info"}>
          <div className="master-detail__info" data-testid="master-detail-info">
            <div className="master-detail__info-grid">
              <div className="master-info-item">
                <span className="master-info-item__label">当前账号</span>
                <span className="master-info-item__value" data-testid="master-info-account">
                  {currentName ?? "未登录"}
                </span>
              </div>
              <div className="master-info-item">
                <span className="master-info-item__label">主库大小</span>
                <span className="master-info-item__value" data-testid="master-info-size">
                  {stats && stats.size_bytes > 0 ? formatBytes(stats.size_bytes) : "—"}
                </span>
              </div>
              <div className="master-info-item">
                <span className="master-info-item__label">最近活跃</span>
                <span className="master-info-item__value" data-testid="master-info-active">
                  {statsReady && stats.last_active_unix_seconds !== null
                    ? formatDateTime(stats.last_active_unix_seconds)
                    : "—"}
                </span>
              </div>
              <div className="master-info-item">
                <span className="master-info-item__label">最近备份</span>
                <span className="master-info-item__value" data-testid="master-info-backup">
                  {latestBackup ? formatDateTime(latestBackup.stamp_unix_seconds) : "还没有备份"}
                </span>
              </div>
              <div className="master-info-item master-info-item--wide">
                <span className="master-info-item__label">主库路径</span>
                {envState?.data_dir ? (
                  // 完整路径收进悬浮提示，主视野只显示末两段（界面表达纪律）。
                  <code
                    className="master-info-item__path"
                    data-testid="master-info-path"
                    title={envState.data_dir}
                  >
                    {dataDirTail(envState.data_dir)}
                  </code>
                ) : (
                  <span className="master-info-item__value">—</span>
                )}
              </div>
            </div>
            {statsReady && (
              <div className="master-detail__counts" data-testid="master-detail-counts">
                <div className="master-count" title="当前账号可见的项目数">
                  <b>{stats.project_count}</b><span>项目</span>
                </div>
                <div className="master-count" title="当前账号可见的会话数">
                  <b>{stats.session_count}</b><span>会话</span>
                </div>
                <div className="master-count" title="当前账号会话的消息总数">
                  <b>{stats.message_count}</b><span>消息</span>
                </div>
              </div>
            )}
          </div>
          <MasterVerificationPanel active={active && tab === "info"} backupOnly />
        </div>
      </div>
      </>
      )}
    </section>
  );
}

/** 主库路径展示：只取末两段（完整路径在悬浮提示，界面表达纪律）。 */
function dataDirTail(path: string): string {
  const segments = path.split(/[\\/]/).filter(Boolean);
  return segments.length > 1 ? segments.slice(-2).join("\\") : path;
}

/** 字节数人性化（详情页体积/备份大小共用）。 */
function formatBytes(value: number): string {
  if (value < 1024) return `${value} B`;
  const units = ["KB", "MB", "GB", "TB"];
  let size = value;
  let unit = -1;
  do {
    size /= 1024;
    unit += 1;
  } while (size >= 1024 && unit < units.length - 1);
  return `${size >= 100 ? Math.round(size) : size.toFixed(1)} ${units[unit]}`;
}

/** 绝对时间（最近活跃/最近备份：与列表相对时间不同，这里要能定位到具体时刻）。 */
function formatDateTime(unixSeconds: number): string {
  const date = new Date(unixSeconds * 1000);
  const now = new Date();
  const pad = (value: number) => String(value).padStart(2, "0");
  const hm = `${pad(date.getHours())}:${pad(date.getMinutes())}`;
  const monthDay = `${date.getMonth() + 1}月${date.getDate()}日`;
  return date.getFullYear() === now.getFullYear() ? `${monthDay} ${hm}` : `${date.getFullYear()}年${monthDay} ${hm}`;
}
