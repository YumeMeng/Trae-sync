import { useCallback, useEffect, useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { ArrowDownAZ, ArrowLeftRight, HeartPulse, LayoutGrid, List, RefreshCw, ShieldCheck, UserPlus, UserRound } from "lucide-react";
import type {
  CheckinCapabilityDto,
  CheckinLoginBeginDto,
  CheckinLoginReceiptDto,
  CheckinOverviewEntryDto,
  CredentialRefreshEntryDto,
  CreditsRefreshEntryDto,
  ManagedAccountsViewDto,
  TraeInstanceLoginState,
  TraeInstanceStateDto,
} from "../types/account_switch";
import type { EnvironmentStateDto } from "../types/environment";
import { safeUiErrorMessage } from "../utils/safeUiError";
import { displayMobile, effectiveDisplayName } from "../utils/accountDisplay";
import { AccountDetail } from "./AccountDetail";
import {
  CheckinSlotBadge,
  LoginArchiveSlotBadge,
  deriveCheckinSlotState,
  deriveLoginSlotState,
  type LoginSlotState,
} from "./StatusBadges";
import { OperationResultCard, type OperationResultIssue } from "./OperationResultCard";
import { MasterSwitchDialog, type MasterSwitchTarget } from "./MasterSwitchDialog";
import { ConfirmDialog } from "./ConfirmDialog";

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
// G14 账号过滤：all=全部（默认）；attention=需处理（只显示异常账号）。会话内状态，不持久化。
type AccountFilter = "all" | "attention";

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
  // G9 结果卡：健康检测 / 刷新额度完成后的结构化回执（首行结论 + 异常清单）。
  const [resultCard, setResultCard] = useState<{
    title: string;
    okCount: number;
    issues: readonly OperationResultIssue[];
  } | null>(null);
  // 账号总览（真实模式）：档案 + 积分缓存 + 令牌到期，驱动卡片与详情视图。
  const [overview, setOverview] = useState<readonly CheckinOverviewEntryDto[]>([]);
  const [loginBusy, setLoginBusy] = useState(false);
  // 登录浏览器方式（默认隔离实例）：添加账号时可切换“隔离浏览器 / 本机浏览器”。
  const [loginBrowserMode, setLoginBrowserMode] = useState<LoginBrowserMode>("isolated");
  // 批量刷新积分进行中（按钮禁用 + 进度提示）。
  const [creditsBusy, setCreditsBusy] = useState(false);
  // 批量刷新登录凭据进行中（只换发 token，不签到、不查询额度）。
  const [credentialRefreshBusy, setCredentialRefreshBusy] = useState(false);
  // 一键健康检测进行中（本地检测 + 网络探测，按钮禁用 + 进度提示）。
  const [healthBusy, setHealthBusy] = useState(false);
  // G15 纯本地刷新进行中（毫秒级本地读取，仅驱动按钮图标旋转）。
  const [localRefreshBusy, setLocalRefreshBusy] = useState(false);
  // 选中的账号：非空时进入独立详情视图。
  const [selectedProfileId, setSelectedProfileId] = useState("");
  // U-2 双视图与排序：localStorage 记忆（脏值回退默认）。
  const [accountView, setAccountView] = useState<AccountView>(() =>
    readStoredPreference(VIEW_STORAGE_KEY, "list", ["list", "card"] as const));
  const [accountSort, setAccountSort] = useState<AccountSort>(() =>
    readStoredPreference(SORT_STORAGE_KEY, "added", ["added", "name", "checkin"] as const));
  // G14 需处理过滤：会话内状态（「需处理」是临时排查意图，不写偏好）。
  const [accountFilter, setAccountFilter] = useState<AccountFilter>("all");
  // 登录凭据健康度（profile_id → 条目，P7-5 凭据包实调判定）：
  // login_state 为实调结果（驱动卡片徽章），archive_available 为存档次要信息。
  const [loginStates, setLoginStates] = useState<Record<string, TraeInstanceStateDto>>({});
  // P5-2 切号主面板：主库当前登录账号（环境档案）+ 切换目标（弹层打开中）。
  const [envCurrentProfileId, setEnvCurrentProfileId] = useState<string | null>(null);
  const [switchTarget, setSwitchTarget] = useState<MasterSwitchTarget | null>(null);
  // G11 登录后手机号补录弹层：profileId + 展示名 + 服务端脱敏号（肉眼比对基准）。
  const [mobilePrompt, setMobilePrompt] = useState<{ profileId: string; name: string; masked: string } | null>(null);
  const [mobilePromptValue, setMobilePromptValue] = useState("");
  const [mobilePromptBusy, setMobilePromptBusy] = useState(false);
  const [mobilePromptError, setMobilePromptError] = useState<string | null>(null);
  // 批量刷新凭据二次确认弹层开关（确认后执行，防误触大范围换发）。
  const [credentialConfirmOpen, setCredentialConfirmOpen] = useState(false);
  // G11 查重命中：待确认的重复手机号保存请求（统一确认弹层）。
  const [duplicateMobile, setDuplicateMobile] = useState<{
    profileId: string;
    mobile: string;
    ownerName: string;
    /** 从 G11 补录弹层发起：确认保存成功后一并关闭补录弹层。 */
    closePrompt: boolean;
  } | null>(null);

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

  const refreshOverview = useCallback(async (): Promise<CheckinOverviewEntryDto[] | null> => {
    const entries = await invoke<CheckinOverviewEntryDto[]>("get_checkin_overview").catch(() => null);
    if (entries) setOverview(entries);
    return entries;
  }, []);

  // P6-2 登录存档健康度：真实模式且账号集合变化时读取（无轮询——存档只在
  // 登录/切号/保活写回时变化）。依赖用 profile 集合签名而非数组身份：
  // 纯本地刷新（G15）重读总览不触发凭据重探测（get_trae_instance_states
  // 含逐账号 HTTP 实调，是有网络成本的操作，只有健康检测应触发）。
  const overviewProfileKey = overview.map((entry) => entry.profile_id).join("\n");
  useEffect(() => {
    if (!active || checkinCapability?.real_http_enabled !== true || overviewProfileKey === "") {
      return;
    }
    let cancelled = false;
    const profileIds = overviewProfileKey.split("\n");
    void invoke<TraeInstanceStateDto[]>("get_trae_instance_states", { profileIds })
      .then((states) => {
        if (!cancelled) setLoginStates(toLoginStateMap(states));
      })
      .catch(() => undefined); // 读取失败保持现状徽章（未登录），不阻塞页面。
    return () => { cancelled = true; };
  }, [active, checkinCapability?.real_http_enabled, overviewProfileKey]);

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

  // G15 纯本地刷新：重读账号总览与环境档案（均为本机缓存，零网络请求）。
  // 语义分工：刷新 = 重读缓存；健康检测 = 真实探测（含逐账号网络实调）。
  const handleLocalRefresh = useCallback(() => {
    if (localRefreshBusy) return;
    setLocalRefreshBusy(true);
    // 两个读取内部各自吞错（保持旧数据），完成后收起旋转动画。
    void Promise.all([refreshOverview(), loadEnvironment()]).finally(() => setLocalRefreshBusy(false));
  }, [localRefreshBusy, refreshOverview, loadEnvironment]);

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

  // G14 需处理判定：口径与 G10 槽位一致——登录槽红/琥珀 或 签到槽红。
  // 已知凭据失效/缺失和 access 过期属于明确可处理项，即使健康度读取没有返回也不能隐藏。
  // 未签/未刷新（灰）不算异常；登录槽灰（未登录）仍按“尚未登录”处理。
  const needsAttention = useCallback((entry: CheckinOverviewEntryDto) => {
    const nowUnixSeconds = Math.floor(Date.now() / 1000);
    if (credentialNeedsAttention(entry, nowUnixSeconds)) return true;
    const credentialDead = credentialRequiresRelogin(entry);
    const loginSlot = deriveLoginSlotState({
      login_state: effectiveCredentialState(entry, loginStates[entry.profile_id]),
      relogin_only: credentialDead,
    });
    if (loginSlot === "relogin" || loginSlot === "expired" || loginSlot === "pending") return true;
    return deriveCheckinSlotState(entry) === "failed";
  }, [loginStates]);

  // 角标数字按全量总览计（与当前过滤无关）。
  const attentionCount = useMemo(
    () => overview.filter((entry) => needsAttention(entry)).length,
    [overview, needsAttention],
  );
  const visibleOverview = useMemo(
    () => (accountFilter === "attention" ? sortedOverview.filter((entry) => needsAttention(entry)) : sortedOverview),
    [accountFilter, sortedOverview, needsAttention],
  );

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
      const entries = await refreshOverview();
      setMessage(`账号“${receipt.screen_name}”登录成功，已可用于签到。`);
      // G11：登录成功即引导补录完整手机号（可跳过）；脱敏号取自刷新后的
      // 总览条目（登录时已写入注册表）。已补录过的账号重复登录不再打扰。
      const fresh = entries?.find((entry) => entry.profile_id === receipt.profile_id) ?? null;
      if (!fresh?.mobile_full) {
        setMobilePrompt({
          profileId: receipt.profile_id,
          name: effectiveDisplayName({ display_name: fresh?.display_name ?? null, screen_name: receipt.screen_name }),
          masked: fresh?.masked_mobile ?? "",
        });
      }
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

  // G11 保存补录手机号：第三层查重在前端提示确认（前两层格式/脱敏比对在后端命令）。
  // 空串 = 清除补录（详情页“清空保存”路径；登录弹层保存按钮空值时禁用不会走到）。
  // 返回 false = 查重命中待弹层确认（或用户取消）；错误原样抛出由调用方映射文案。
  const saveMobileBackfill = useCallback(async (profileId: string, mobile: string) => {
    const trimmed = mobile.trim();
    if (trimmed !== "") {
      const owner = overview.find(
        (entry) => entry.profile_id !== profileId && entry.mobile_full === trimmed,
      );
      if (owner) {
        // 查重命中：弹统一确认弹层，返回 false（本次未保存，由弹层确认后续走）。
        setDuplicateMobile({
          profileId,
          mobile: trimmed,
          ownerName: effectiveDisplayName(owner),
          closePrompt: mobilePrompt !== null,
        });
        return false;
      }
    }
    await invoke("set_account_mobile", { profileId, mobile: trimmed === "" ? null : trimmed });
    await refreshOverview();
    return true;
  }, [overview, mobilePrompt, refreshOverview]);

  // 查重确认：坚持把该手机号保存到当前账号。
  const confirmDuplicateMobile = useCallback(() => {
    const request = duplicateMobile;
    if (!request) return;
    setDuplicateMobile(null);
    if (request.closePrompt) setMobilePromptBusy(true);
    void (async () => {
      try {
        await invoke("set_account_mobile", { profileId: request.profileId, mobile: request.mobile });
        await refreshOverview();
        // 列表场景回写成功反馈；详情页场景由数据刷新呈现新号码。
        setMessage("手机号已保存，账号列表与详情页都会显示完整号码。");
        // 补录弹层发起的：保存成功后与原流程一致，一并关闭。
        if (request.closePrompt) setMobilePrompt(null);
      } catch (reason: unknown) {
        // 失败：补录弹层场景回弹层内提示；详情页场景回页面错误条。
        const text = safeUiErrorMessage(reason, "手机号保存未完成，请稍后重试。");
        if (request.closePrompt) setMobilePromptError(text);
        else setError(text);
      } finally {
        if (request.closePrompt) setMobilePromptBusy(false);
      }
    })();
  }, [duplicateMobile, refreshOverview]);

  // G11 补录弹层保存：成功后关弹层；失败保留弹层与输入值，映射后的文案就地展示。
  const handleMobilePromptSave = useCallback(() => {
    if (!mobilePrompt || mobilePromptBusy) return;
    // 空输入不可保存：与保存按钮 disabled 同口径，防止 Enter 旁路触发清除路径
    // （saveMobileBackfill 空串 = 清除已补录手机号）。
    if (mobilePromptValue.trim() === "") return;
    setMobilePromptBusy(true);
    setMobilePromptError(null);
    void (async () => {
      try {
        const saved = await saveMobileBackfill(mobilePrompt.profileId, mobilePromptValue);
        if (saved) {
          setMobilePrompt(null);
          setMobilePromptValue("");
          setMessage("手机号已补全，账号列表与详情页都会显示完整号码。");
        }
      } catch (reason: unknown) {
        setMobilePromptError(safeUiErrorMessage(reason, "手机号保存失败，请稍后重试。"));
      } finally {
        setMobilePromptBusy(false);
      }
    })();
  }, [mobilePrompt, mobilePromptBusy, mobilePromptValue, saveMobileBackfill]);

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
    setResultCard(null);
    void (async () => {
      try {
        const profileIds = overview.map((entry) => entry.profile_id);
        const results = await invoke<CreditsRefreshEntryDto[]>("refresh_checkin_credits", { profileIds });
        await refreshOverview();
        const nameOf = new Map(overview.map((entry) => [entry.profile_id, effectiveDisplayName(entry)]));
        // G9 结果卡：异常账号列名 + 一句人话原因；正常账号不占空间。
        const issues = results
          .filter((item) => item.error_code !== null)
          .map((item) => ({
            key: item.profile_id,
            name: nameOf.get(item.profile_id) ?? item.screen_name,
            reason: safeUiErrorMessage(item.error_code, "网络或服务暂时不可用，请稍后重试。"),
          }));
        setResultCard({ title: "额度刷新", okCount: results.length - issues.length, issues });
      } catch (reason: unknown) {
        setError(safeUiErrorMessage(reason, "额度刷新未完成，请稍后重试。"));
      } finally {
        setCreditsBusy(false);
      }
    })();
  }, [creditsBusy, overview, refreshOverview]);

  // 批量刷新登录凭据：逐账号执行同设备换发，不打开用户 OAuth、不签到、不过问额度。
  // 入口只负责打开二次确认弹层（批量换发写回全部账号本机登录信息，防误触），执行体在下方。
  const handleRefreshAllCredentials = useCallback(() => {
    if (credentialRefreshBusy || overview.length === 0) return;
    setCredentialConfirmOpen(true);
  }, [credentialRefreshBusy, overview.length]);

  // 批量刷新凭据确认后执行。
  const executeRefreshAllCredentials = useCallback(() => {
    setCredentialConfirmOpen(false);
    if (credentialRefreshBusy || overview.length === 0) return;
    setCredentialRefreshBusy(true);
    setError(null);
    setMessage(null);
    setResultCard(null);
    void (async () => {
      try {
        const profileIds = overview.map((entry) => entry.profile_id);
        const results = await invoke<CredentialRefreshEntryDto[]>("refresh_checkin_credentials", { profileIds });
        await refreshOverview();
        const nameOf = new Map(overview.map((entry) => [entry.profile_id, effectiveDisplayName(entry)]));
        const issues = results
          .filter((item) => !item.refreshed)
          .map((item) => ({
            key: item.profile_id,
            name: (nameOf.get(item.profile_id) ?? item.screen_name) || "该账号",
            reason: safeUiErrorMessage(item.error_code, "登录凭据刷新未完成，请稍后重试。"),
          }));
        setResultCard({
          title: "凭据刷新",
          okCount: results.filter((item) => item.refreshed).length,
          issues,
        });
        // 换发结果已完成身份校验；本地徽章无需再次发起逐账号网络探测。
        setLoginStates((previous) => {
          const next = { ...previous };
          for (const item of results) {
            if (item.refreshed) {
              const current = next[item.profile_id];
              next[item.profile_id] = {
                profile_id: item.profile_id,
                login_state: "logged_in",
                archive_available: current?.archive_available ?? false,
              };
            }
          }
          return next;
        });
      } catch (reason: unknown) {
        setError(safeUiErrorMessage(reason, "凭据刷新未完成，请稍后重试。"));
      } finally {
        setCredentialRefreshBusy(false);
      }
    })();
  }, [credentialRefreshBusy, overview, refreshOverview]);

  // 一键健康检测：登录存档深度检测（storage.json 键 + 最近启动日志证据，
  // 秒级）→ 签到会话网络探测（复用 refresh_checkin_credits 只读查询，顺带
  // 刷新额度与令牌时间戳）→ 徽章即时更新 + G9 结果卡。
  const handleHealthCheck = useCallback(() => {
    if (healthBusy || overview.length === 0) return;
    setHealthBusy(true);
    setError(null);
    setMessage(null);
    setResultCard(null);
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
        setResultCard({ title: "健康检测", ...buildHealthResult(states, results, overview) });
      } catch (reason: unknown) {
        setError(safeUiErrorMessage(reason, "健康检测未完成，请稍后重试。"));
      } finally {
        setHealthBusy(false);
      }
    })();
  }, [healthBusy, overview, refreshOverview]);

  // G11 登录后手机号补录弹层（共享 JSX）：列表视图与详情视图都渲染——
  // 从详情页发起重新登录时弹层不再滞留到返回列表后才出现。

  // 批量刷新凭据二次确认弹层（仅列表视图渲染——入口按钮只在列表页头部）。
  const credentialConfirmDialog = credentialConfirmOpen && (
    <ConfirmDialog
      title="刷新全部账号凭据"
      lines={[
        `将为全部 ${overview.length} 个账号逐个向服务端换发新的登录令牌，并更新本机保存的登录信息。`,
        "期间不会打开新的登录页面。",
      ]}
      confirmLabel="刷新凭据"
      busyLabel="刷新中…"
      busy={credentialRefreshBusy}
      onCancel={() => setCredentialConfirmOpen(false)}
      onConfirm={executeRefreshAllCredentials}
      testId="account-refresh-credentials-confirm"
    />
  );

  // G11 查重确认弹层：列表与详情视图都渲染（与 mobilePromptDialog 同模式）。
  const duplicateMobileDialog = duplicateMobile && (
    <ConfirmDialog
      title="手机号已被其他账号使用"
      lines={[
        `该手机号已用于账号“${duplicateMobile.ownerName}”。`,
        "仍要保存到当前账号吗？两个账号将显示同一手机号。",
      ]}
      confirmLabel="仍要保存"
      busyLabel="保存中…"
      busy={mobilePromptBusy}
      onCancel={() => setDuplicateMobile(null)}
      onConfirm={confirmDuplicateMobile}
      testId="account-mobile-duplicate-confirm"
    />
  );

  const mobilePromptDialog = mobilePrompt && (
    <div className="switch-veil" role="presentation">
      <div className="mobile-prompt" role="dialog" aria-modal="true" aria-labelledby="mobile-prompt-title" data-testid="mobile-backfill-dialog">
        <h2 id="mobile-prompt-title" className="mobile-prompt__title">补全手机号</h2>
        <p className="mobile-prompt__lead">
          为账号「{mobilePrompt.name}」补录完整手机号，多账号时更易区分。可跳过，之后随时能在账号详情页补录。
        </p>
        <p className="mobile-prompt__masked">
          服务端记录：{mobilePrompt.masked || "暂未采集（可先跳过，刷新额度后自动补全脱敏号）"}
        </p>
        <input
          className="mobile-prompt__input"
          type="tel"
          inputMode="numeric"
          value={mobilePromptValue}
          maxLength={11}
          placeholder="请输入 11 位完整手机号"
          disabled={mobilePromptBusy}
          data-testid="mobile-backfill-input"
          aria-label="完整手机号"
          onChange={(event) => setMobilePromptValue(event.target.value.replace(/[^\d]/g, ""))}
          onKeyDown={(event) => {
            if (event.key === "Enter") {
              event.preventDefault();
              handleMobilePromptSave();
            }
          }}
        />
        {mobilePromptError && (
          <p className="mobile-prompt__error" role="alert" data-testid="mobile-backfill-error">{mobilePromptError}</p>
        )}
        <div className="mobile-prompt__actions">
          <button
            className="btn"
            type="button"
            disabled={mobilePromptBusy}
            data-testid="mobile-backfill-skip"
            onClick={() => {
              setMobilePrompt(null);
              setMobilePromptValue("");
              setMobilePromptError(null);
            }}
          >
            跳过
          </button>
          <button
            className="btn btn--primary"
            type="button"
            disabled={mobilePromptBusy || mobilePromptValue.trim() === ""}
            data-testid="mobile-backfill-save"
            onClick={handleMobilePromptSave}
          >
            {mobilePromptBusy ? "保存中…" : "保存"}
          </button>
        </div>
      </div>
    </div>
  );

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
      <>
        <AccountDetail
          entry={selectedEntry}
          onRelogin={() => handleLogin(false)}
          loginBusy={loginBusy}
          onDataChanged={async () => {
            await refreshOverview();
          }}
          onSaveMobile={(mobile) => saveMobileBackfill(selectedEntry.profile_id, mobile)}
          onBack={() => setSelectedProfileId("")}
        />
        {mobilePromptDialog}
        {duplicateMobileDialog}
      </>
    );
  }

  const savedAccounts = view?.saved_accounts ?? [];

  return (
    <section className="account-center" role="region" aria-label="账号">
      <header className="page-header">
        <div className="page-header__copy">
          <h1 data-page-title="accounts" tabIndex={-1}>账号</h1>
        </div>
        <div className="page-header__actions">
          <span className="status-badge status-badge--neutral"><ShieldCheck size={14} aria-hidden="true" />登录信息仅本机加密保存</span>
          {realCheckinMode && overview.length > 0 && (
            <>
              {/* G15 纯本地刷新：重读缓存零联网，与「健康检测」的真实探测分工。 */}
              <button
                className="btn"
                type="button"
                onClick={handleLocalRefresh}
                disabled={localRefreshBusy || credentialRefreshBusy}
                data-testid="account-refresh-local"
                title="重读本机缓存的账号信息（不联网）；需要探测真实状态请用健康检测"
              >
                <RefreshCw size={15} className={localRefreshBusy ? "icon-spin" : undefined} aria-hidden="true" />刷新
              </button>
              <button className="btn" type="button" onClick={handleHealthCheck} disabled={healthBusy || creditsBusy || credentialRefreshBusy || loginBusy} data-testid="account-health-check">
                <HeartPulse size={15} aria-hidden="true" />{healthBusy ? "检测中…" : "健康检测"}
              </button>
              <button className="btn" type="button" onClick={handleRefreshAllCredits} disabled={creditsBusy || loginBusy || healthBusy || credentialRefreshBusy} data-testid="account-refresh-credits">
                <RefreshCw size={15} aria-hidden="true" />{creditsBusy ? "查询中…" : "刷新额度"}
              </button>
              <button className="btn btn--primary" type="button" onClick={handleRefreshAllCredentials} disabled={credentialRefreshBusy || loginBusy || healthBusy || creditsBusy} data-testid="account-refresh-credentials">
                <RefreshCw size={15} aria-hidden="true" />{credentialRefreshBusy ? "刷新中…" : "刷新凭据"}
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
                disabled={loginBusy || credentialRefreshBusy || healthBusy || creditsBusy}
                onChange={(event) => setLoginBrowserMode(event.target.value as LoginBrowserMode)}
                data-testid="login-browser-mode"
              >
                <option value="isolated">隔离浏览器</option>
                <option value="system">本机浏览器</option>
              </select>
              <button className="btn btn--primary" type="button" onClick={() => void handleLogin(loginBrowserMode === "system")} disabled={loginBusy || credentialRefreshBusy || healthBusy || creditsBusy} data-testid="account-add-primary">
                <UserPlus size={15} aria-hidden="true" />{loginBusy ? "等待浏览器登录完成…" : "添加账号"}
              </button>
            </div>
          )}
        </div>
      </header>

      {error && <p className="workbench__error" role="alert">{error}</p>}
      {message && <p className="account-center__message" role="status">{message}</p>}
      {/* G9 结果卡：健康检测 / 刷新额度共用形态（首行结论 + 异常清单）。 */}
      {resultCard && (
        <OperationResultCard
          title={resultCard.title}
          okCount={resultCard.okCount}
          issues={resultCard.issues}
        />
      )}
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
      {credentialRefreshBusy && <p className="account-center__meta" role="status">正在刷新登录凭据，不会打开新的 OAuth 登录…</p>}
      {healthBusy && <p className="account-center__meta" role="status">正在检测账号健康度（登录存档 + 签到会话探测）…</p>}

      {/* 我的账号：页面主体。真实模式双视图（U-2：列表宽行默认 / 卡片网格切换，
          分段控件 + localStorage 记忆）；fixture 沿用已保存账号。 */}
      <section className="account-center__profiles" aria-labelledby="account-mine-heading">
        <div className="account-center__section-heading">
          <h3 id="account-mine-heading"><UserRound size={16} aria-hidden="true" />我的账号</h3>
          {realCheckinMode && overview.length > 0 && (
            <div className="account-center__list-controls">
              {/* G14 过滤分段控件：全部 / 需处理（角标=异常账号数，正常时不显示）。 */}
              <div className="seg-control" role="group" aria-label="账号过滤">
                <button
                  type="button"
                  className={`seg-control__item${accountFilter === "all" ? " seg-control__item--active" : ""}`}
                  aria-pressed={accountFilter === "all"}
                  onClick={() => setAccountFilter("all")}
                  data-testid="account-filter-all"
                  title="显示全部账号"
                >
                  全部
                </button>
                <button
                  type="button"
                  className={`seg-control__item${accountFilter === "attention" ? " seg-control__item--active" : ""}`}
                  aria-pressed={accountFilter === "attention"}
                  onClick={() => setAccountFilter("attention")}
                  data-testid="account-filter-attention"
                  title="只显示需要处理的账号（需重登、已过期、待登录或签到失败）"
                >
                  需处理
                  {attentionCount > 0 && <span className="seg-control__count">{attentionCount}</span>}
                </button>
              </div>
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
            visibleOverview.length > 0 ? (
              <ul className={accountView === "list" ? "account-list" : "account-card-grid"}>
                {visibleOverview.map((entry) => (
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
              // G14 过滤空态：有账号但当前过滤无匹配，与「还没有账号」区分。
              <p className="account-center__empty">没有需要处理的账号。</p>
            )
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

      {/* G11 登录后手机号补录弹层：共享变量（详情视图同样渲染）。 */}
      {mobilePromptDialog}
      {duplicateMobileDialog}

      {/* 批量刷新凭据二次确认弹层（入口按钮在列表页头部）。 */}
      {credentialConfirmDialog}
    </section>
  );
}

/**
 * 账号条目（U-2 双视图）：variant=list 横向宽行 / variant=card 卡片网格。
 * 两槽位徽章系统（StatusBadges）：槽位1 签到三态 + 槽位2 登录存档健康度；
 * 登录凭据剩余时间与手机号降级为 meta 文字（正常态安静，异常才亮色）。
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
  const credentialDead = credentialRequiresRelogin(entry);
  // G10 统一状态机：两槽位徽章都先经 derive* 纯函数求枚举态，再交徽章渲染。
  const checkinState = deriveCheckinSlotState(entry);
  const loginSlot = deriveLoginSlotState({ login_state: loginState, relogin_only: credentialDead });
  // 列表/卡片保留手机号与访问令牌剩余时间；精确到期时刻仍在详情页展示。
  const tokenRemaining = tokenRemainingLabel(entry.access_token_expires_at_unix_seconds);

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

  // G12 卡片视图独立积分块：左对齐大数字 + 标题小字（列表侧栏块为右对齐设计，不复用）。
  const cardCredits = (
    <div className="account-card__credits" title="TRAE 真实可用模型额度（积分包剩余总和，与 IDE 内显示一致）">
      {entry.usage_remaining_credits != null ? (
        <>
          <span className="account-card__credits-value" data-testid={`account-usage-${entry.profile_id}`}>
            {formatCreditsValue(entry.usage_remaining_credits)}
          </span>
          <span className="account-card__credits-label">模型积分{entry.usage_cached_at ? ` · ${formatShortDate(entry.usage_cached_at)}` : ""}</span>
        </>
      ) : (
        <span className="account-card__credits-placeholder">未查询</span>
      )}
      {entry.refresh_error_code != null && (
        <span
          className="account-card__credits-failed"
          title={safeUiErrorMessage(entry.refresh_error_code, "上次额度刷新未成功。")}
        >
          刷新失败
        </span>
      )}
    </div>
  );

  // G12 底部切换按钮通栏：当前账号的「使用中」标记已上移至名称行。
  const cardSwitchButton = isCurrentAccount ? null : (
    <button
      className="btn btn--primary account-card__switch"
      type="button"
      disabled={switchBusy}
      onClick={(event) => {
        event.stopPropagation();
        onSwitch();
      }}
      data-testid={`account-switch-${entry.profile_id}`}
      title={`切换主库到 ${displayName}（全部对话记录保持不变）`}
    >
      <ArrowLeftRight size={14} aria-hidden="true" />
      {switchBusy ? "切换中…" : "切换到此账号"}
    </button>
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
                  <CheckinSlotBadge state={checkinState} />
                  <LoginArchiveSlotBadge state={loginSlot} archiveAvailable={archiveAvailable} />
                </div>
                {/* G13：主列表展示手机号与凭据剩余时间；G11 补录后显示全号。 */}
                <div className="account-meta-group">
                  {displayMobile(entry) && <p className="account-item__meta">{displayMobile(entry)}</p>}
                  <p className="account-item__meta account-item__token-expiry">{tokenRemaining}</p>
                </div>
              </div>
            </div>
            <div className="account-item__side">
              {creditsBlock}
              {/* P5-2 切号主面板：当前账号展示「使用中」，其余账号一键切换。 */}
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
                  title={`切换主库到 ${displayName}（全部对话记录保持不变）`}
                >
                  <ArrowLeftRight size={14} aria-hidden="true" />
                  {switchBusy ? "切换中…" : "切换到此账号"}
                </button>
              )}
            </div>
          </>
        ) : (
          <>
            {/* G12 卡片独立纵向层级：头像+名称（含使用中 chip）→ 徽章行 →
                积分大字 → 手机号 → 底部切换按钮通栏。 */}
            <div className="account-card__head">
              <span className="account-card__avatar" aria-hidden="true">{avatarLetter(displayName)}</span>
              <strong className="account-card__name" title={entry.screen_name}>{displayName}</strong>
              {isCurrentAccount && (
                <span className="account-item__current-chip" data-testid={`account-current-${entry.profile_id}`} title="主库当前登录账号">
                  使用中
                </span>
              )}
            </div>
            <div className="account-card__slots">
              <CheckinSlotBadge state={checkinState} />
              <LoginArchiveSlotBadge state={loginSlot} archiveAvailable={archiveAvailable} />
            </div>
            {cardCredits}
            <div className="account-meta-group">
              {displayMobile(entry) && <p className="account-card__meta">{displayMobile(entry)}</p>}
              <p className="account-card__meta account-card__token-expiry">{tokenRemaining}</p>
            </div>
            {cardSwitchButton}
          </>
        )}
      </div>
    </li>
  );
}

/** 登录凭据健康度列表转 profile_id → 条目映射（初始读取/健康检测共用，
 *  P7-5 起保留完整条目：login_state 主信息 + archive_available 次要信息）。 */
function toLoginStateMap(states: readonly TraeInstanceStateDto[]): Record<string, TraeInstanceStateDto> {
  return Object.fromEntries(states.map((entry) => [entry.profile_id, entry]));
}

/** 已确认只能通过重新登录恢复的凭据错误；网络类刷新失败不在此列。 */
const CREDENTIAL_RELOGIN_ERROR_CODES = new Set([
  "credential_refresh_failed",
  "binding_mismatch",
  "auth_mismatch",
  "credential_missing",
  "credential_unavailable",
  "credential_invalid",
  "manual_recovery_required",
]);

function credentialRequiresRelogin(entry: CheckinOverviewEntryDto): boolean {
  return entry.credential_legacy || CREDENTIAL_RELOGIN_ERROR_CODES.has(entry.refresh_error_code ?? "");
}

function accessTokenExpired(entry: CheckinOverviewEntryDto, nowUnixSeconds: number): boolean {
  return (
    entry.access_token_expires_at_unix_seconds != null
    && entry.access_token_expires_at_unix_seconds <= nowUnixSeconds
  );
}

/** 凭据明确需要用户处理：失效标记或 access 已过期；未知状态留给登录槽状态机。 */
function credentialNeedsAttention(entry: CheckinOverviewEntryDto, nowUnixSeconds: number): boolean {
  return credentialRequiresRelogin(entry) || accessTokenExpired(entry, nowUnixSeconds);
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
  if (entry && credentialRequiresRelogin(entry)) return "stale";
  if (entry && accessTokenExpired(entry, Math.floor(Date.now() / 1000))) return "stale";
  return state?.login_state;
}

/**
 * G10 登录槽各态的一句人话原因（结果卡与徽章共用词源）。
 * ok / signed_out 为 null：正常与“从未保存凭据”都不是需处理项
 * （灰=中性无需动作，与 G14 needsAttention 排除 signed_out 同口径）。
 */
const LOGIN_SLOT_REASONS: Record<LoginSlotState, string | null> = {
  ok: null,
  expired: "登录态已过期，等待自动恢复",
  relogin: "登录凭据已失效，请重新登录",
  pending: "登录凭据不可用，请重新登录",
  signed_out: null,
};

/**
 * G9 健康检测结果：逐账号判定异常并求一句人话原因。
 * 异常口径与徽章同源（G10 状态机）：登录槽非正常态 或 签到会话探测失败；
 * 登录问题优先展示（会话异常多为同一根因——凭据坏了探测必然失败）。
 * states 为 null（本地检测读取失败）时只依据会话探测结果，不臆测登录态。
 */
function buildHealthResult(
  states: readonly TraeInstanceStateDto[] | null,
  results: readonly CreditsRefreshEntryDto[],
  overview: readonly CheckinOverviewEntryDto[],
): { okCount: number; issues: OperationResultIssue[] } {
  const nameOf = new Map(overview.map((entry) => [entry.profile_id, effectiveDisplayName(entry)]));
  const stateOf = new Map((states ?? []).map((state) => [state.profile_id, state]));
  const probeOf = new Map(results.map((result) => [result.profile_id, result]));
  // 统计口径同 effectiveCredentialState：本次探测到续期被拒也计入“登录失效”。
  const renewalDead = new Set(
    results
      .filter((item) => CREDENTIAL_RELOGIN_ERROR_CODES.has(item.error_code ?? ""))
      .map((item) => item.profile_id),
  );
  const issues: OperationResultIssue[] = [];
  for (const entry of overview) {
    const state = stateOf.get(entry.profile_id);
    const effective = effectiveCredentialState(
      entry,
      renewalDead.has(entry.profile_id) && state ? { ...state, login_state: "stale" as const } : state,
    );
    // relogin_only 口径与卡片徽章一致：旧通道 / 续期被拒（本次探测或既有标记）。
    const reloginOnly = credentialRequiresRelogin(entry) || renewalDead.has(entry.profile_id);
    const loginReason = states
      ? LOGIN_SLOT_REASONS[deriveLoginSlotState({ login_state: effective, relogin_only: reloginOnly })]
      : null;
    if (loginReason !== null) {
      issues.push({ key: entry.profile_id, name: nameOf.get(entry.profile_id) ?? entry.screen_name, reason: loginReason });
      continue;
    }
    // 登录正常才单独报会话异常（登录失效的探测失败是同一根因，不重复列）。
    const probe = probeOf.get(entry.profile_id);
    if (probe && probe.error_code !== null) {
      issues.push({
        key: entry.profile_id,
        name: nameOf.get(entry.profile_id) ?? entry.screen_name,
        reason: safeUiErrorMessage(probe.error_code, "网络或服务暂时不可用，请稍后重试。"),
      });
    }
  }
  return { okCount: overview.length - issues.length, issues };
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

/** 账号列表展示访问令牌剩余时间；只读总览时间戳，不触发网络请求。 */
function tokenRemainingLabel(expiresAt: number | null): string {
  if (expiresAt == null) return "登录凭据有效期未知";
  const remainingMs = expiresAt * 1000 - Date.now();
  if (remainingMs <= 0) return "登录凭据已过期";
  // 与详情页保持同一口径：不足一天仍显示为 1 天，避免把可用凭据显示为 0 天。
  return `登录凭据剩余 ${Math.ceil(remainingMs / 86400000)} 天`;
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
