import { useCallback, useEffect, useMemo, useState } from "react";
import type { ReactNode } from "react";
import { invoke } from "@tauri-apps/api/core";
import { ArrowDownAZ, ArrowLeftRight, HeartPulse, LayoutGrid, List, RefreshCw, ShieldCheck, UserPlus, UserRound } from "lucide-react";
import type {
  CheckinCapabilityDto,
  CheckinLoginBeginDto,
  CheckinLoginReceiptDto,
  CheckinOverviewEntryDto,
  CreditsRefreshEntryDto,
  ManagedAccountsViewDto,
  TraeInstanceLoginState,
  TraeInstanceStateDto,
} from "../types/account_switch";
import type { EnvironmentStateDto } from "../types/environment";
import { safeUiErrorMessage } from "../utils/safeUiError";
import { effectiveDisplayName } from "../utils/accountDisplay";
import { AccountDetail } from "./AccountDetail";
import { CheckinSlotBadge, LoginArchiveSlotBadge, tokenHealthText } from "./StatusBadges";
import { MasterSwitchDialog, type MasterSwitchTarget } from "./MasterSwitchDialog";

interface AccountCenterProps {
  /** 页面可见时才发起账号状态读取，避免后台 IPC。 */
  active: boolean;
}

// 登录浏览器方式：isolated=隔离实例（默认，与本机登录态互不影响）；
// system=本机浏览器默认 profile（复用系统已登录会话，一键授权当前账号）。
type LoginBrowserMode = "isolated" | "system";
// U-2 账号页双视图：list=横向宽行（默认，多账号扫描对比最快）；card=卡片网格。
type AccountView = "list" | "card";
// U-2 排序：added=添加序（注册表顺序）；name=名称；checkin=签到状态（未签在前）。
type AccountSort = "added" | "name" | "checkin";

const VIEW_STORAGE_KEY = "accounts.view";
const SORT_STORAGE_KEY = "accounts.sort";

/** 读取本地偏好；非法值回退默认（list / added），保证 localStorage 脏数据不炸页面。 */
function readStoredPreference<T extends string>(key: string, fallback: T, allowed: readonly T[]): T {
  try {
    const value = window.localStorage.getItem(key);
    return allowed.includes(value as T) ? (value as T) : fallback;
  } catch {
    return fallback;
  }
}

// 账号页只做一件事：账号档案。卡片矩阵（五要素）+ 独立详情视图（全量信息与操作）；
// 每日签到在「签到」页，密钥工具在「设置」页；切号由 P5-2 弹层事务承担，
// 专属实例启动/关闭入口已随 P6-2 退役（多开由「环境」概念承接，见环境页）。
export function AccountCenter({ active }: AccountCenterProps) {
  const [view, setView] = useState<ManagedAccountsViewDto | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [message, setMessage] = useState<string | null>(null);
  const [checkinCapability, setCheckinCapability] = useState<CheckinCapabilityDto | null>(null);
  // 账号总览（真实模式）：档案 + 积分缓存 + 令牌到期，驱动卡片与详情视图。
  const [overview, setOverview] = useState<readonly CheckinOverviewEntryDto[]>([]);
  const [loginBusy, setLoginBusy] = useState(false);
  // 登录浏览器方式（默认隔离实例）：添加账号时可切换“隔离浏览器 / 本机浏览器”。
  const [loginBrowserMode, setLoginBrowserMode] = useState<LoginBrowserMode>("isolated");
  // 批量刷新积分进行中（按钮禁用 + 进度提示）。
  const [creditsBusy, setCreditsBusy] = useState(false);
  // 一键健康检测进行中（本地检测 + 网络探测，按钮禁用 + 进度提示）。
  const [healthBusy, setHealthBusy] = useState(false);
  // 选中的账号：非空时进入独立详情视图。
  const [selectedProfileId, setSelectedProfileId] = useState("");
  // U-2 双视图与排序：localStorage 记忆（脏值回退默认）。
  const [accountView, setAccountView] = useState<AccountView>(() =>
    readStoredPreference(VIEW_STORAGE_KEY, "list", ["list", "card"] as const));
  const [accountSort, setAccountSort] = useState<AccountSort>(() =>
    readStoredPreference(SORT_STORAGE_KEY, "added", ["added", "name", "checkin"] as const));
  // 登录凭据健康度（profile_id → 条目，P7-5 凭据包实调判定）：
  // login_state 为实调结果（驱动卡片徽章），archive_available 为存档次要信息。
  const [loginStates, setLoginStates] = useState<Record<string, TraeInstanceStateDto>>({});
  // P5-2 切号主面板：主库当前登录账号（环境档案）+ 切换目标（弹层打开中）。
  const [envCurrentProfileId, setEnvCurrentProfileId] = useState<string | null>(null);
  const [switchTarget, setSwitchTarget] = useState<MasterSwitchTarget | null>(null);

  const load = useCallback(async () => {
    const nextView = await invoke<ManagedAccountsViewDto>("get_managed_account_state");
    setView(nextView);
  }, []);

  useEffect(() => {
    if (!active) return;
    let cancelled = false;
    setLoading(true);
    void load().catch((reason: unknown) => {
      if (!cancelled) setError(safeUiErrorMessage(reason, "账号信息暂时不可读取，请稍后重试。"));
    }).finally(() => {
      if (!cancelled) setLoading(false);
    });
    return () => { cancelled = true; };
  }, [active, load]);

  useEffect(() => {
    if (!active) return;
    let cancelled = false;
    void invoke<CheckinCapabilityDto>("get_checkin_capability")
      .then((capability) => { if (!cancelled) setCheckinCapability(capability); })
      .catch(() => {
        if (!cancelled) {
          setCheckinCapability({
            enabled: false,
            transport: "disabled",
            real_http_enabled: false,
            message: "签到能力当前不可读取。",
          });
        }
      });
    return () => { cancelled = true; };
  }, [active]);

  // 真实模式：账号总览只在能力确认为真实 transport 后加载。
  useEffect(() => {
    if (!active || checkinCapability?.real_http_enabled !== true) return;
    let cancelled = false;
    void invoke<CheckinOverviewEntryDto[]>("get_checkin_overview")
      .then((entries) => { if (!cancelled) setOverview(entries); })
      .catch((reason: unknown) => {
        // 读取失败必须可见：静默置空会把真实故障伪装成“还没有账号”。
        if (!cancelled) {
          setOverview([]);
          setError(safeUiErrorMessage(reason, "账号总览读取失败。"));
        }
      });
    return () => { cancelled = true; };
  }, [active, checkinCapability?.real_http_enabled]);

  const refreshOverview = useCallback(async () => {
    const entries = await invoke<CheckinOverviewEntryDto[]>("get_checkin_overview").catch(() => null);
    if (entries) setOverview(entries);
  }, []);

  // P6-2 登录存档健康度：真实模式且有账号时读取一次（无轮询——存档只在
  // 登录/切号/保活写回时变化，页面重新可见或总览刷新时本 effect 自然重跑）。
  useEffect(() => {
    if (!active || checkinCapability?.real_http_enabled !== true || overview.length === 0) {
      return;
    }
    let cancelled = false;
    const profileIds = overview.map((entry) => entry.profile_id);
    void invoke<TraeInstanceStateDto[]>("get_trae_instance_states", { profileIds })
      .then((states) => {
        if (!cancelled) setLoginStates(toLoginStateMap(states));
      })
      .catch(() => undefined); // 读取失败保持现状徽章（未登录），不阻塞页面。
    return () => { cancelled = true; };
  }, [active, checkinCapability?.real_http_enabled, overview]);

  // P5-2 环境档案：主库当前登录账号驱动「使用中」标记与切换按钮分布。
  // fixture 模式后端返回空档案（无当前账号），全部账号展示切换入口。
  const loadEnvironment = useCallback(async () => {
    const env = await invoke<EnvironmentStateDto>("get_environment_state").catch(() => null);
    setEnvCurrentProfileId(env?.current_profile_id ?? null);
  }, []);

  useEffect(() => {
    if (!active) return;
    void loadEnvironment();
  }, [active, loadEnvironment]);

  // 切号完成（弹层回执后）：重读环境档案与账号总览，卡片「使用中」随之转移。
  const handleSwitchFinished = useCallback(async () => {
    await loadEnvironment();
    await refreshOverview();
  }, [loadEnvironment, refreshOverview]);

  const realCheckinMode = checkinCapability?.real_http_enabled === true;
  // 添加账号是核心需求：除 fixture 测试模式外常驻显示，不依赖签到能力开关。
  const canAddAccount = checkinCapability?.transport !== "fixture";

  // U-2 视图/排序偏好持久化（每次变更即写，量小无需防抖）。
  const changeAccountView = useCallback((next: AccountView) => {
    setAccountView(next);
    try { window.localStorage.setItem(VIEW_STORAGE_KEY, next); } catch { /* 存储不可用时仅本次会话生效 */ }
  }, []);
  const changeAccountSort = useCallback((next: AccountSort) => {
    setAccountSort(next);
    try { window.localStorage.setItem(SORT_STORAGE_KEY, next); } catch { /* 同上 */ }
  }, []);

  // U-2 排序视图：name 按展示名（备注优先）本地化比较；checkin 未签在前
  // （补签优先级），已签/未签内部再按名称稳定排序；added 保持注册表顺序。
  const sortedOverview = useMemo(() => {
    if (accountSort === "added") return overview;
    const byName = (a: CheckinOverviewEntryDto, b: CheckinOverviewEntryDto) =>
      effectiveDisplayName(a).localeCompare(effectiveDisplayName(b), "zh-Hans-CN");
    if (accountSort === "name") return [...overview].sort(byName);
    return [...overview].sort((a, b) => {
      const aPending = a.checked_in !== true;
      const bPending = b.checked_in !== true;
      if (aPending !== bPending) return aPending ? -1 : 1;
      return byName(a, b);
    });
  }, [overview, accountSort]);

  // OAuth 登录：后端按所选模式打开浏览器（隔离实例=默认；本机浏览器=复用
  // 系统已登录会话），随后阻塞等待回调（最长约 5 分钟）。
  // begin 失败时无副作用；complete 成功后凭据已加密入库（同一账号重复登录覆盖更新）。
  // P7-4：等待期间可取消（用户点「取消登录」或关闭隔离浏览器窗口都会即时收尾）。
  const handleLogin = useCallback(async (useSystemBrowser: boolean) => {
    if (loginBusy) return;
    setLoginBusy(true);
    setError(null);
    setMessage(null);
    try {
      await invoke<CheckinLoginBeginDto>("begin_checkin_login", { useSystemBrowser });
      const receipt = await invoke<CheckinLoginReceiptDto>("complete_checkin_login");
      await refreshOverview();
      setMessage(`账号“${receipt.screen_name}”登录成功，已可用于签到。`);
    } catch (reason: unknown) {
      // 取消不是错误：以中性提示收尾（浏览器被关闭同样走此路径）。
      if (typeof reason === "string" && reason === "login_cancelled") {
        setMessage("登录已取消；若浏览器已关闭或未完成授权，可重新发起登录。");
      } else {
        setError(safeUiErrorMessage(reason, "登录未完成，请重新发起登录。"));
      }
    } finally {
      setLoginBusy(false);
    }
  }, [loginBusy, refreshOverview]);

  // 取消进行中的登录（P7-4）：置位后端取消标记，等待中的 complete 会以
  // login_cancelled 收尾。取消请求本身失败不打断等待（仍有超时兜底）。
  const handleCancelLogin = useCallback(async () => {
    try {
      await invoke("cancel_checkin_login");
    } catch {
      /* 取消失败不额外提示：complete 的超时与浏览器退出检测仍会收尾 */
    }
  }, []);

  // 批量刷新额度：逐账号只读查询（后端串行、无间隔实时完成），刷新全部卡片。
  const handleRefreshAllCredits = useCallback(() => {
    if (creditsBusy || overview.length === 0) return;
    setCreditsBusy(true);
    setError(null);
    setMessage(null);
    void (async () => {
      try {
        const profileIds = overview.map((entry) => entry.profile_id);
        const results = await invoke<CreditsRefreshEntryDto[]>("refresh_checkin_credits", { profileIds });
        await refreshOverview();
        const okCount = results.filter((item) => item.error_code === null).length;
        const failedEntries = results.filter((item) => item.error_code !== null);
        if (failedEntries.length > 0) {
          // 失败账号直接列名（与健康检测汇总同款式）：错误详情见各账号卡片“刷新失败”标记与详情页。
          const nameOf = new Map(overview.map((entry) => [entry.profile_id, effectiveDisplayName(entry)]));
          const names = failedEntries
            .map((item) => nameOf.get(item.profile_id) ?? item.screen_name)
            .join("、");
          setMessage(`额度已刷新：${okCount} 个成功，${failedEntries.length} 个失败（${names}）。`);
        } else {
          setMessage(`全部 ${okCount} 个账号额度已更新。`);
        }
      } catch (reason: unknown) {
        setError(safeUiErrorMessage(reason, "额度刷新未完成，请稍后重试。"));
      } finally {
        setCreditsBusy(false);
      }
    })();
  }, [creditsBusy, overview, refreshOverview]);

  // 一键健康检测：登录存档深度检测（storage.json 键 + 最近启动日志证据，
  // 秒级）→ 签到会话网络探测（复用 refresh_checkin_credits 只读查询，顺带
  // 刷新额度与令牌时间戳）→ 徽章即时更新 + 汇总消息。
  const handleHealthCheck = useCallback(() => {
    if (healthBusy || overview.length === 0) return;
    setHealthBusy(true);
    setError(null);
    setMessage(null);
    void (async () => {
      try {
        const profileIds = overview.map((entry) => entry.profile_id);
        // 本地深度检测：升级后的四态判定（含日志证据）立即刷新全部徽章。
        const states = await invoke<TraeInstanceStateDto[]>("get_trae_instance_states", {
          profileIds,
        }).catch(() => null);
        if (states) {
          setLoginStates(toLoginStateMap(states));
        }
        // 网络探测：逐账号只读查询，error_code 非空即签到会话异常。
        const results = await invoke<CreditsRefreshEntryDto[]>("refresh_checkin_credits", { profileIds });
        await refreshOverview();
        setMessage(summarizeHealth(states, results, overview));
      } catch (reason: unknown) {
        setError(safeUiErrorMessage(reason, "健康检测未完成，请稍后重试。"));
      } finally {
        setHealthBusy(false);
      }
    })();
  }, [healthBusy, overview, refreshOverview]);

  if (!active || loading) {
    return (
      <section className="account-center" role="region" aria-label="账号">
        {/* 加载中也保留页面标题，保证键盘导航与切页焦点一致 */}
        <header className="page-header">
          <div className="page-header__copy">
            <h1 data-page-title="accounts" tabIndex={-1}>账号</h1>
          </div>
        </header>
        <p className="account-center__empty" role="status">正在读取账号信息…</p>
      </section>
    );
  }

  // 独立详情视图：总览里仍能找到该账号时展示（删除后 overview 刷新即回列表）。
  const selectedEntry = selectedProfileId
    ? overview.find((entry) => entry.profile_id === selectedProfileId) ?? null
    : null;
  if (selectedEntry) {
    return (
      <AccountDetail
        entry={selectedEntry}
        onRelogin={() => handleLogin(false)}
        loginBusy={loginBusy}
        onDataChanged={refreshOverview}
        onBack={() => setSelectedProfileId("")}
      />
    );
  }

  const savedAccounts = view?.saved_accounts ?? [];

  return (
    <section className="account-center" role="region" aria-label="账号">
      <header className="page-header">
        <div className="page-header__copy">
          <h1 data-page-title="accounts" tabIndex={-1}>账号</h1>
          <p>切换账号会自动完成登录切换，全部对话记录保留。每日签到在「签到」页，密钥工具在「设置」页。</p>
        </div>
        <div className="page-header__actions">
          <span className="status-badge status-badge--neutral"><ShieldCheck size={14} aria-hidden="true" />登录信息仅本机加密保存</span>
          {realCheckinMode && overview.length > 0 && (
            <>
              <button className="btn" type="button" onClick={handleHealthCheck} disabled={healthBusy || creditsBusy || loginBusy} data-testid="account-health-check">
                <HeartPulse size={15} aria-hidden="true" />{healthBusy ? "检测中…" : "健康检测"}
              </button>
              <button className="btn" type="button" onClick={handleRefreshAllCredits} disabled={creditsBusy || loginBusy || healthBusy} data-testid="account-refresh-credits">
                <RefreshCw size={15} aria-hidden="true" />{creditsBusy ? "查询中…" : "刷新额度"}
              </button>
            </>
          )}
          {canAddAccount && (
            <div className="page-header__actions-group" data-testid="account-add-group">
              {/* 登录方式选择：隔离浏览器（默认，与本机登录态互不影响）/
                  本机浏览器（复用系统已登录会话，一键授权当前账号）。 */}
              <select
                aria-label="登录浏览器方式"
                className="page-header__select"
                value={loginBrowserMode}
                disabled={loginBusy}
                onChange={(event) => setLoginBrowserMode(event.target.value as LoginBrowserMode)}
                data-testid="login-browser-mode"
              >
                <option value="isolated">隔离浏览器</option>
                <option value="system">本机浏览器</option>
              </select>
              <button className="btn btn--primary" type="button" onClick={() => void handleLogin(loginBrowserMode === "system")} disabled={loginBusy} data-testid="account-add-primary">
                <UserPlus size={15} aria-hidden="true" />{loginBusy ? "等待浏览器登录完成…" : "添加账号"}
              </button>
            </div>
          )}
        </div>
      </header>

      {error && <p className="workbench__error" role="alert">{error}</p>}
      {message && <p className="account-center__message" role="status">{message}</p>}
      {loginBusy && (
        <div className="account-center__login-waiting" role="status">
          <p className="account-center__meta">
            {loginBrowserMode === "system"
              ? "已在本机浏览器打开 TRAE 登录页（复用系统登录态），完成授权后此处会自动继续；期间无需手动刷新。"
              : "已在隔离浏览器打开 TRAE 登录页，完成登录后此处会自动继续；期间无需手动刷新。"}
          </p>
          {/* P7-4：等待不再只能干等 5 分钟——随时可取消（关闭隔离浏览器窗口同样会收尾） */}
          <button
            className="btn btn--quiet"
            type="button"
            onClick={() => void handleCancelLogin()}
            data-testid="account-cancel-login"
          >
            取消登录
          </button>
        </div>
      )}
      {creditsBusy && <p className="account-center__meta" role="status">正在查询最新额度…</p>}
      {healthBusy && <p className="account-center__meta" role="status">正在检测账号健康度（登录存档 + 签到会话探测）…</p>}

      {/* 我的账号：页面主体。真实模式双视图（U-2：列表宽行默认 / 卡片网格切换，
          分段控件 + localStorage 记忆）；fixture 沿用已保存账号。 */}
      <section className="account-center__profiles" aria-labelledby="account-mine-heading">
        <div className="account-center__section-heading">
          <h3 id="account-mine-heading"><UserRound size={16} aria-hidden="true" />我的账号</h3>
          {realCheckinMode && overview.length > 0 && (
            <div className="account-center__list-controls">
              {/* 排序：添加序 / 名称 / 签到状态（未签在前，补签场景优先）。 */}
              <label className="account-center__sort-field">
                <ArrowDownAZ size={14} aria-hidden="true" />
                <span className="sr-only">排序方式</span>
                <select
                  value={accountSort}
                  onChange={(event) => changeAccountSort(event.target.value as AccountSort)}
                  data-testid="account-sort"
                >
                  <option value="added">添加顺序</option>
                  <option value="name">名称</option>
                  <option value="checkin">签到状态</option>
                </select>
              </label>
              {/* 视图切换分段控件：列表 / 卡片（记忆 + 平滑过渡）。 */}
              <div className="seg-control" role="group" aria-label="视图切换">
                <button
                  type="button"
                  className={`seg-control__item${accountView === "list" ? " seg-control__item--active" : ""}`}
                  aria-pressed={accountView === "list"}
                  onClick={() => changeAccountView("list")}
                  data-testid="account-view-list"
                  title="列表视图：横向宽行，扫描对比最快"
                >
                  <List size={14} aria-hidden="true" />列表
                </button>
                <button
                  type="button"
                  className={`seg-control__item${accountView === "card" ? " seg-control__item--active" : ""}`}
                  aria-pressed={accountView === "card"}
                  onClick={() => changeAccountView("card")}
                  data-testid="account-view-card"
                  title="卡片视图：网格布局，信息分层展示"
                >
                  <LayoutGrid size={14} aria-hidden="true" />卡片
                </button>
              </div>
            </div>
          )}
          <span className="section-caption">{realCheckinMode ? overview.length : savedAccounts.length} 个</span>
        </div>
        {realCheckinMode ? (
          overview.length > 0 ? (
            <ul className={accountView === "list" ? "account-list" : "account-card-grid"}>
              {sortedOverview.map((entry) => (
                <AccountCard
                  key={entry.profile_id}
                  entry={entry}
                  variant={accountView}
                  loginState={effectiveCredentialState(entry, loginStates[entry.profile_id])}
                  archiveAvailable={loginStates[entry.profile_id]?.archive_available}
                  isCurrentAccount={envCurrentProfileId === entry.profile_id}
                  switchBusy={switchTarget !== null}
                  onOpen={() => setSelectedProfileId(entry.profile_id)}
                  onSwitch={() => setSwitchTarget({
                    profile_id: entry.profile_id,
                    display_name: effectiveDisplayName(entry),
                  })}
                />
              ))}
            </ul>
          ) : (
            <div className="account-center__empty-actions">
              <p className="account-center__empty">还没有账号。通过浏览器登录添加第一个账号，登录成功后即可签到。</p>
              <button className="btn btn--primary" type="button" onClick={() => void handleLogin(loginBrowserMode === "system")} disabled={loginBusy}>
                <UserPlus size={15} aria-hidden="true" />{loginBusy ? "等待浏览器登录完成…" : "添加账号"}
              </button>
            </div>
          )
        ) : savedAccounts.length > 0 ? (
          <ul className="account-center__profile-list">
            {savedAccounts.map((account) => (
              <li key={account.profile_id} className="account-center__profile">
                <div className="account-center__profile-select">
                  <span className="account-card__avatar account-card__avatar--inline" aria-hidden="true">{avatarLetter(account.display_name)}</span>
                  <span className="account-center__profile-copy">
                    <strong>{account.display_name}</strong>
                    <span>最近验证：{formatDateTime(account.last_verified_at)}{account.region ? ` · ${account.region.toUpperCase()}` : ""}</span>
                  </span>
                </div>
              </li>
            ))}
          </ul>
        ) : (
          <p className="account-center__empty">尚未保存账号。点击右上角“添加账号”通过浏览器登录。</p>
        )}
      </section>

      {/* P5-2 切号进度弹层：target 非空即打开（五步事务由后端编排，见 MasterSwitchDialog）。 */}
      <MasterSwitchDialog
        target={switchTarget}
        onFinished={handleSwitchFinished}
        onClose={() => setSwitchTarget(null)}
      />
    </section>
  );
}

/**
 * 账号条目（U-2 双视图）：variant=list 横向宽行 / variant=card 卡片网格。
 * 两槽位徽章系统（StatusBadges）：槽位1 签到三态 + 槽位2 登录存档健康度；
 * 令牌健康/设备尾号/手机号降级为 meta 文字（正常态安静，异常才亮色）。
 * 点击条目进入详情视图；切换按钮不冒泡。
 * 注：签到活动 credits 为静态池值（实证恒 200），不展示以免误导。
 */
function AccountCard({
  entry,
  variant,
  loginState,
  archiveAvailable,
  isCurrentAccount,
  switchBusy,
  onOpen,
  onSwitch,
}: {
  entry: CheckinOverviewEntryDto;
  variant: AccountView;
  /** 登录凭据健康度：undefined=未初始化（未保存过登录凭据，徽章显示「未登录」）。 */
  loginState?: TraeInstanceLoginState;
  /** 登录存档存在（切换账号的备用方式可用）：徽章悬浮提示次要信息。 */
  archiveAvailable?: boolean;
  /** 主库当前登录账号（环境档案）：展示「使用中」并隐藏切换入口。 */
  isCurrentAccount: boolean;
  /** 切号弹层进行中：全部卡片的切换按钮禁用（同时只有一个切换事务）。 */
  switchBusy: boolean;
  onOpen: () => void;
  /** P5-2 主库切号入口（Q6：账号页即切号主面板）。 */
  onSwitch: () => void;
}) {
  const displayName = effectiveDisplayName(entry);
  // 徽章失效后的恢复路径提示：旧通道凭据/续期被拒不会自动恢复（重新登录是唯一出路）。
  const credentialDead = entry.credential_legacy || entry.refresh_error_code === "credential_refresh_failed";
  // meta 文字段：手机号 · 令牌 N 天 · 设备尾号（令牌 <=7 天整段转琥珀）。
  const token = tokenHealthText(entry.access_token_expires_at_unix_seconds);
  const refreshHint = entry.refresh_token_expires_at_unix_seconds !== null
    ? `刷新令牌剩 ${Math.max(0, Math.ceil((entry.refresh_token_expires_at_unix_seconds - Date.now() / 1000) / 86400))} 天；过期后需重新登录`
    : "";
  const metaParts = [
    entry.masked_mobile || null,
    <span key="token" className={token.warn ? "account-item__meta-warn" : undefined} title={`${token.title}${refreshHint ? `；${refreshHint}` : ""}`}>{token.text}</span>,
    entry.device_tail ? <span key="device" title="签到绑定设备的尾号（完整 ID 见详情）">设备 …{entry.device_tail}</span> : null,
  ].filter(Boolean);

  const creditsBlock = (
    <div className="account-item__credits" title="TRAE 真实可用模型额度（积分包剩余总和，与 IDE 内显示一致）">
      {entry.usage_remaining_credits != null ? (
        <>
          <strong data-testid={`account-usage-${entry.profile_id}`}>{formatCreditsValue(entry.usage_remaining_credits)}</strong>
          <span className="account-item__credits-label">模型积分{entry.usage_cached_at ? ` · ${formatShortDate(entry.usage_cached_at)}` : ""}</span>
        </>
      ) : (
        <span className="account-item__credits-placeholder">未查询</span>
      )}
      {/* 持续失败标记：最近一次刷新失败（缓存持久化，成功后清除）；详情见悬浮提示。 */}
      {entry.refresh_error_code != null && (
        <span
          className="account-item__credits-failed"
          title={safeUiErrorMessage(entry.refresh_error_code, "上次额度刷新未成功。") }
        >
          刷新失败
        </span>
      )}
    </div>
  );

  const actionButtons = (
    <div className="account-item__actions">
      {/* P5-2 切号主面板：当前账号展示「使用中」，其余账号一键切换（弹层见 MasterSwitchDialog）。 */}
      {isCurrentAccount ? (
        <span className="account-item__current-chip" data-testid={`account-current-${entry.profile_id}`} title="主库当前登录账号">
          使用中
        </span>
      ) : (
        <button
          className="btn btn--primary"
          type="button"
          disabled={switchBusy}
          onClick={(event) => {
            event.stopPropagation();
            onSwitch();
          }}
          data-testid={`account-switch-${entry.profile_id}`}
          title={`切换主库到 ${displayName}（全部对话记录随行）`}
        >
          <ArrowLeftRight size={14} aria-hidden="true" />
          {switchBusy ? "切换中…" : "切换到此账号"}
        </button>
      )}
    </div>
  );

  return (
    <li>
      {/* 条目容器是 div（内部含切换按钮，不能嵌套 button）；键盘可达性用 role+tabIndex 保持。
          列表与卡片共用同一 testid，测试与外部引用不受视图切换影响。 */}
      <div
        className={variant === "list" ? "account-list__row account-item" : "account-card account-card--clickable account-item"}
        role="button"
        tabIndex={0}
        onClick={onOpen}
        onKeyDown={(event) => {
          // 仅条目本体自身按键进入详情；操作按钮的按键事件不冒泡触发。
          if (event.target === event.currentTarget && (event.key === "Enter" || event.key === " ")) {
            event.preventDefault();
            onOpen();
          }
        }}
        data-testid={`account-card-${entry.profile_id}`}
        title={`查看 ${displayName} 的详情`}
      >
        {variant === "list" ? (
          <>
            <div className="account-item__main">
              <span className="account-card__avatar" aria-hidden="true">{avatarLetter(displayName)}</span>
              <div className="account-item__id">
                <div className="account-item__name-row">
                  <strong className="account-item__name" title={entry.screen_name}>{displayName}</strong>
                  <CheckinSlotBadge checkedIn={entry.checked_in} />
                  <LoginArchiveSlotBadge loginState={loginState} archiveAvailable={archiveAvailable} reloginOnly={credentialDead} />
                </div>
                <p className="account-item__meta">{intercalate(metaParts)}</p>
              </div>
            </div>
            <div className="account-item__side">
              {creditsBlock}
              {actionButtons}
            </div>
          </>
        ) : (
          <>
            <div className="account-card__head">
              <span className="account-card__avatar" aria-hidden="true">{avatarLetter(displayName)}</span>
              <div className="account-item__name-row">
                <strong className="account-item__name" title={entry.screen_name}>{displayName}</strong>
              </div>
            </div>
            <div className="account-card__slots">
              <CheckinSlotBadge checkedIn={entry.checked_in} />
              <LoginArchiveSlotBadge loginState={loginState} archiveAvailable={archiveAvailable} reloginOnly={credentialDead} />
            </div>
            {creditsBlock}
            <p className="account-item__meta">{intercalate(metaParts)}</p>
            {actionButtons}
          </>
        )}
      </div>
    </li>
  );
}

/** meta 分隔符拼接：元素之间以「 · 」相连（内容不含分隔符时原样返回）。 */
function intercalate(parts: ReactNode[]): ReactNode {
  return parts.reduce<ReactNode[]>((acc, part, index) => {
    if (index === 0) return [part];
    return [...acc, <span key={`sep-${index}`} className="account-item__meta-sep" aria-hidden="true"> · </span>, part];
  }, []);
}

/** 登录凭据健康度列表转 profile_id → 条目映射（初始读取/健康检测共用，
 *  P7-5 起保留完整条目：login_state 主信息 + archive_available 次要信息）。 */
function toLoginStateMap(states: readonly TraeInstanceStateDto[]): Record<string, TraeInstanceStateDto> {
  return Object.fromEntries(states.map((entry) => [entry.profile_id, entry]));
}

/**
 * 徽章生效登录态 = 实调结果 × 两层本地修正（2026-09-02 徽章修复决策）：
 * 1. 本地判定（零网络，页面加载即生效）：旧通道凭据（client_id 非 SOLO）
 *    永远无法自动续期，直接按“登录失效”处理，重新登录是唯一恢复方式。
 * 2. 探测融合（持久化）：最近一次刷新探测到续期被服务端拒绝
 *    （credential_refresh_failed，写入额度缓存，成功后清除），
 *    可发现“token 未过期但续期已死”的账号（如刷新令牌被吊销）。
 */
function effectiveCredentialState(
  entry: CheckinOverviewEntryDto | undefined,
  state?: TraeInstanceStateDto,
): TraeInstanceLoginState | undefined {
  if (entry?.credential_legacy) return "stale";
  if (entry?.refresh_error_code === "credential_refresh_failed") return "stale";
  return state?.login_state;
}

/**
 * 健康检测汇总文案：登录存档四态分布 + 签到会话网络探测结果。
 * 存档部分未初始化=尚未保存登录凭据（预期状态而非异常）；
 * 网络部分按 error_code 区分正常/异常，异常账号附名字方便定位。
 */
function summarizeHealth(
  states: readonly TraeInstanceStateDto[] | null,
  results: readonly CreditsRefreshEntryDto[],
  overview: readonly CheckinOverviewEntryDto[],
): string {
  const nameOf = new Map(overview.map((entry) => [entry.profile_id, entry.screen_name]));
  // 汇总数字必须与卡片徽章逐个对应：统计口径同 effectiveCredentialState
  // （旧通道本地判定 + 本次续期探测失败都计入“登录失效”）。
  const entryOf = new Map(overview.map((entry) => [entry.profile_id, entry]));
  const renewalDead = new Set(
    results.filter((item) => item.error_code === "credential_refresh_failed").map((item) => item.profile_id),
  );
  const loginCount = { valid: 0, stale: 0, pending: 0 };
  let uninitialized = 0;
  if (states) {
    for (const state of states) {
      const effective = effectiveCredentialState(
        entryOf.get(state.profile_id),
        renewalDead.has(state.profile_id)
          ? { ...state, login_state: "stale" as const }
          : state,
      );
      if (effective === "logged_in") loginCount.valid += 1;
      else if (effective === "stale") loginCount.stale += 1;
      else if (effective === "logged_out") loginCount.pending += 1;
      else uninitialized += 1;
    }
  }
  const sessionOk = results.filter((item) => item.error_code === null).length;
  const sessionFailed = results.filter((item) => item.error_code !== null);
  // 分段拼装：登录存档段（仅在有存档数据时）+ 签到会话段。
  // 词表与 LoginArchiveSlotBadge 一致（登录有效/待登录/登录失效/未登录），
  // 保证汇总数字能逐个对应到列表行徽章。
  const segments: string[] = [];
  if (states) {
    const parts: string[] = [];
    if (loginCount.valid > 0) parts.push(`${loginCount.valid} 登录有效`);
    if (loginCount.stale > 0) parts.push(`${loginCount.stale} 登录失效`);
    if (loginCount.pending > 0) parts.push(`${loginCount.pending} 待登录`);
    if (uninitialized > 0) parts.push(`${uninitialized} 未登录`);
    segments.push(`登录存档：${parts.join("、")}`);
  }
  if (sessionFailed.length > 0) {
    const names = sessionFailed.map((item) => nameOf.get(item.profile_id) ?? item.screen_name).join("、");
    segments.push(`签到会话：${sessionOk} 正常、${sessionFailed.length} 异常（${names}）`);
  } else {
    segments.push(`签到会话：全部 ${sessionOk} 个正常`);
  }
  return `健康检测完成。${segments.join("；")}。`;
}

/** 额度数值格式化：保留 1 位小数（整数不带小数点），如 23.8604 -> 23.9、240 -> 240。 */
function formatCreditsValue(value: number): string {
  const rounded = Math.round(value * 10) / 10;
  return Number.isInteger(rounded) ? String(rounded) : rounded.toFixed(1);
}

function formatDateTime(value: string | null): string {
  if (!value) return "尚未验证";
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return "时间不可用";
  return new Intl.DateTimeFormat("zh-CN", { year: "numeric", month: "2-digit", day: "2-digit", hour: "2-digit", minute: "2-digit", hour12: false }).format(date);
}
function formatShortDate(value: string): string {
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return "时间不可用";
  return new Intl.DateTimeFormat("zh-CN", { month: "2-digit", day: "2-digit", hour: "2-digit", minute: "2-digit", hour12: false }).format(date);
}
function avatarLetter(name: string | null | undefined): string {
  const text = (name ?? "").trim();
  if (text.length === 0) return "?";
  return text.charAt(0).toUpperCase();
}
