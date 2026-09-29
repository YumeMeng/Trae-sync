import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { ArrowLeft, RefreshCw } from "lucide-react";
import type { AppPage } from "../components/NavigationRail";
import { LibrarySessionsPanel, type LibraryRef } from "../components/LibrarySessionsPanel";
import { PluginWorkbench } from "../components/PluginWorkbench";
import { MasterVerificationPanel } from "../components/MasterVerificationPanel";
import type { EnvironmentListItemDto, EnvironmentStateDto } from "../types/environment";
import type { MasterBackupChainDto, MasterLibraryStatsDto } from "../types/masterLibrary";
import { safeUiErrorMessage } from "../utils/safeUiError";
// 统一通知适配层（P11-1 切片②）：读取失败等一次性原因走全局通知。
import { notify } from "../platform/notify";
import { isReadOnlyLibrary, TRAE_CN_LIBRARY_TITLE } from "../platform/productRegistry";

// ============================================================================
// 库详情页 shell（G20 三 tab 平级结构，2026-09-04 重构；P11-5 收官泛化）：
// 顶部只留面包屑式标题（库名 + 返回环境页）+ 刷新；
// tabs = 对话列表（默认，LibrarySessionsPanel embedded 两栏）· 插件 · 库信息
//（原基础信息：路径/大小/账号/统计/最近活跃/最近备份 + 备份对比）。
// 原数据校验 tab 已取消：备份对比移入库信息 tab，接力台账核对由
// P8-5 主库自检（G18）承载。
// P11-5 收官：library 缺省主库；副环境传 { id: envId, displayName } 后
// 统计/备份链/对话列表/校验全部按库解析（ADR-0025）。副环境无插件清单
// 体系（ADR-0023 清单挂在账号宇宙），插件 tab 显示说明而非工作台。
// P11-6（ADR-0032 决策 5）：trae_cn 库 = Trae CN 数据根只读浏览——
// 读路径放行（历史/消息/统计/备份链），写控件与草稿机制整体隐藏，
// 插件 tab 与库信息按只读口径降级说明。
// ============================================================================

/** 详情页 tab：对话列表复用库对话面板（ADR-0025）；插件为 P5-8b；库信息为 G20。 */
type MasterDetailTab = "sessions" | "plugins" | "info";

/** 库信息 tab 的环境档案（当前账号 + 数据目录，主库/副库取数路径不同）。 */
interface LibraryEnvInfo {
  readonly currentAccountName: string | null;
  readonly dataDir: string | null;
}

interface MasterLibraryDetailProps {
  /** 页面可见时才读取数据；隐藏时停止轮询（embedded 历史视图随 active 联动）。 */
  active: boolean;
  /** 返回环境页（详情页不在导航栏，返回是唯一退路）。 */
  onNavigate: (page: AppPage) => void;
  /** 库实例引用（ADR-0025，缺省主库；副环境 id 即 env_id）。 */
  library?: LibraryRef;
}

export function MasterLibraryDetail({ active, onNavigate, library }: MasterLibraryDetailProps) {
  const isMaster = (library?.id ?? "master") === "master";
  const libraryId = library?.id ?? "master";
  // P11-6：trae_cn = Trae CN 数据根只读浏览（ADR-0032 决策 5）。
  const readOnly = isReadOnlyLibrary(libraryId);
  // 库标题：主库固定名；副库 = 环境名（环境页「查看详情」入口传入）；
  // 只读数据根 = 注册表口径「Trae CN 数据根」。
  const libraryTitle = readOnly
    ? TRAE_CN_LIBRARY_TITLE
    : isMaster ? "主库" : library?.displayName ?? "副库";
  const [envInfo, setEnvInfo] = useState<LibraryEnvInfo | null>(null);
  // 统计与备份链读取失败时静默降级为 null（对应分区显示 —），不阻塞页面。
  const [stats, setStats] = useState<MasterLibraryStatsDto | null>(null);
  const [backupChain, setBackupChain] = useState<MasterBackupChainDto | null>(null);
  const [tab, setTab] = useState<MasterDetailTab>("sessions");
  // 懒挂载：App 各页面常驻 DOM（[hidden] 切换），若详情页一启动就渲染内嵌
  // 库对话面板只在详情页首次激活后挂载，避免未访问详情时白白创建两栏视图；
  // 之后保持挂载，切换工作区不会丢失浏览状态。
  const [contentMounted, setContentMounted] = useState(false);

  const load = useCallback(async () => {
    // 环境档案：主库走状态命令；副库从环境列表按 env_id 取行
    //（列表不请求体积——详情页体积由统计接口承载）。
    // 只读数据根（trae_cn）不在环境注册表内，工具也不管理其登录态：
    // 跳过环境档案查询，保持跨产品数据隔离。
    if (readOnly) {
      setEnvInfo(null);
    } else if (isMaster) {
      const nextEnv = await invoke<EnvironmentStateDto>("get_environment_state");
      setEnvInfo({
        currentAccountName: nextEnv.current_account_name ?? null,
        dataDir: nextEnv.data_dir ?? null,
      });
    } else {
      const list = await invoke<EnvironmentListItemDto[]>("list_environments");
      const match = list.find((item) => item.env_id === libraryId) ?? null;
      setEnvInfo(
        match
          ? {
              currentAccountName: match.current_account_name ?? null,
              dataDir: match.data_dir,
            }
          : null,
      );
    }
    // 统计/备份链与状态同读同刷；两者相互独立，任一失败不影响其余展示。
    // 副库带 libraryId（缺省参数向后兼容：主库调用形态不变）。
    // 只读库读路径已放行（P11-6 后端契约），同参数读取。
    const scopedLibraryId = isMaster && !readOnly ? undefined : { libraryId };
    const [nextStats, nextBackups] = await Promise.all([
      invoke<MasterLibraryStatsDto>(
        "get_master_library_stats",
        scopedLibraryId,
      ).catch(() => null),
      invoke<MasterBackupChainDto>(
        "get_master_backup_chain",
        scopedLibraryId,
      ).catch(() => null),
    ]);
    setStats(nextStats);
    setBackupChain(nextBackups);
  }, [isMaster, libraryId, readOnly]);

  useEffect(() => {
    if (!active) return;
    setContentMounted(true);
    let cancelled = false;
    void load().catch((reason: unknown) => {
      if (!cancelled) notify.error(safeUiErrorMessage(reason, "库信息暂时不可读取，请稍后重试。"));
    });
    return () => { cancelled = true; };
  }, [active, load]);

  const latestBackup = backupChain?.backups[0] ?? null;
  const currentName = envInfo?.currentAccountName ?? null;
  // 统计 ready 才有意义；未登录/无库/失败时计数区显示 —。
  const statsReady = stats?.status === "ready";

  return (
    <section
      className="master-detail"
      role="region"
      aria-label={isMaster ? "主库详情" : `${libraryTitle}详情`}
    >
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
            <h1 data-page-title="master-library" tabIndex={-1}>{libraryTitle}</h1>
            {/* 只读边界标识（P11-6）：写操作在只读库一律隐藏，徽章说明浏览形态。 */}
            {readOnly && <span className="env-card__badge">只读浏览</span>}
          </div>
        </div>
        <div className="page-header__actions">
          <button
            className="btn"
            type="button"
            onClick={() => void load().catch(() => undefined)}
            data-testid="master-detail-refresh"
            title={`重新读取${libraryTitle}信息`}
          >
            <RefreshCw size={15} aria-hidden="true" />刷新
          </button>
        </div>
      </header>

      {/* tabs 内容懒挂载：未访问过详情页前不渲染（见 contentMounted 注释）。 */}
      {contentMounted && (
      <>
      <div className="master-detail__tabs" role="tablist" aria-label={isMaster ? "主库内容" : "副库内容"}>
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
          {/* ADR-0025：详情页注入库实例引用（副库同一组件零改动复用）。
              P11-6：只读库传 readOnly——写操作控件隐藏（不是禁用）。 */}
          <LibrarySessionsPanel
            active={active && tab === "sessions"}
            embedded
            onNavigate={onNavigate}
            library={{ id: libraryId, displayName: isMaster ? undefined : libraryTitle }}
            readOnly={readOnly}
          />
        </div>
        {/* 插件 tab（P5-8b）：主库 = 已装清单 + 对账 + 市场浏览 + 装/卸；
            副环境无插件清单体系（ADR-0023 清单挂在账号宇宙），诚实说明；
            只读数据根（P11-6）同为说明卡：插件管理不随只读浏览开放。 */}
        <div className="master-detail__tabpanel" hidden={tab !== "plugins"}>
          {isMaster && !readOnly ? (
            <PluginWorkbench active={active && tab === "plugins"} />
          ) : readOnly ? (
            <div className="master-detail__info" data-testid="master-plugins-readonly-note">
              <p className="env-checkup__note">
                Trae CN 为只读浏览：不提供插件管理与对账。
              </p>
            </div>
          ) : (
            <div className="master-detail__info" data-testid="master-plugins-secondary-note">
              <p className="env-checkup__note">
                副环境不参与插件管理：插件清单跟随账号在主库共享，副环境的对话记录不涉及插件安装与对账。
              </p>
            </div>
          )}
        </div>
        {/* 库信息 tab（G20）：原基础信息 + 备份对比（原数据校验 tab 的
            备份区块；接力台账核对由 P8-5 主库自检承载）。 */}
        <div className="master-detail__tabpanel" hidden={tab !== "info"}>
          <div className="master-detail__info" data-testid="master-detail-info">
            <div className="master-detail__info-grid">
              <div className="master-info-item">
                <span className="master-info-item__label">当前账号</span>
                <span className="master-info-item__value" data-testid="master-info-account">
                  {/* 只读数据根：工具不管理 Trae CN 登录态，中性占位不冒充「未登录」。 */}
                  {readOnly ? "—" : (currentName ?? "未登录")}
                </span>
              </div>
              <div className="master-info-item">
                <span className="master-info-item__label">{isMaster ? "主库大小" : "数据大小"}</span>
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
                <span className="master-info-item__label">{isMaster ? "主库路径" : "数据目录"}</span>
                {envInfo?.dataDir ? (
                  // 完整路径收进悬浮提示，主视野只显示末两段（界面表达纪律）。
                  <code
                    className="master-info-item__path"
                    data-testid="master-info-path"
                    title={envInfo.dataDir}
                  >
                    {dataDirTail(envInfo.dataDir)}
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
          <MasterVerificationPanel
            active={active && tab === "info"}
            backupOnly
            library={{ id: libraryId }}
          />
        </div>
      </div>
      </>
      )}
    </section>
  );
}

/** 库路径展示：只取末两段（完整路径在悬浮提示，界面表达纪律）。 */
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
