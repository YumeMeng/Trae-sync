import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { HeartPulse, RefreshCw, ShieldCheck, UserPlus } from "lucide-react";
import type {
  CheckinCapabilityDto,
  CheckinLoginReceiptDto,
  CheckinOverviewEntryDto,
  CredentialRefreshEntryDto,
  CreditsRefreshEntryDto,
  ManagedAccountsViewDto,
  TraeInstanceLoginState,
  TraeInstanceStateDto,
} from "../../../types/account_switch";
import type { BatchProgressEvent, BatchProgressKind } from "../../../types/batchProgress";
import { useTauriEvents } from "../../../hooks/useTauriEvents";
import type { EnvironmentStateDto } from "../../../types/environment";
import { safeUiErrorMessage } from "../../../utils/safeUiError";
import { displayMobile, effectiveDisplayName } from "../../../utils/accountDisplay";
// 统一通知适配层（P11-1 切片②）：操作结果类提示一律经 notify 发出。
import { notify, type NotifyIssue } from "../../../platform/notify";
import { AccountDetail } from "../../AccountDetail";
import { AdapterAccountDetail } from "../../AdapterAccountDetail";
import {
  CheckinSlotBadge,
  LoginArchiveSlotBadge,
  deriveCheckinSlotState,
  deriveLoginSlotState,
  type LoginSlotState,
} from "../../StatusBadges";
import { MasterSwitchDialog, type MasterSwitchTarget } from "../../MasterSwitchDialog";
import { ConfirmDialog } from "../../shared/ConfirmDialog";
import { SharedAccountEntry, type AccountView, type SharedAccountRecord } from "../../shared/account/AccountRow";
import { ACCOUNT_SORT_OPTIONS, type AccountFilter, type AccountSort } from "../../shared/account/AccountListControls";
import { AccountPageSkeleton } from "../../shared/account/AccountPageSkeleton";
import { accountPreferenceKey, readStoredPreference, writeStoredPreference } from "../../shared/account/preferences";
import { productUi } from "../../../platform/productUiRegistry";
import type {
  AccountAction,
  AccountActionResult,
  AccountAdapter,
  AccountSnapshot,
  AccountView as AdapterAccountView,
} from "../../../platform/accountAdapter";

interface AccountCenterProps {
  /** 页面可见时才发起账号状态读取，避免后台 IPC。 */
  active: boolean;
  /** 适配器注入仅用于测试场景；生产固定走完整账号事务（P15-10 单产品塌缩）。 */
  adapter?: AccountAdapter | null;
}

/**
 * Work 签到档案链的协议产品标识（P15-10）：后端批量命令与进度事件载荷沿用
 * "work_cn"（协议值稳定，ADR-0035）；它是账号域内部链路标识，不是产品切换概念。
 */
const WORK_ARCHIVE_PRODUCT_ID = "work_cn";

/** 批量操作进度文案的最小展示状态：第 i/N 个账号名。 */
interface BatchProgress {
  completed: number;
  total: number;
  name: string;
}

/**
 * 批量进度事件解析（P12-R2 Q2 统一契约）：三链载荷统一为 BatchProgressEvent。
 * 载荷暂无 kind（R2b 未落地）时按事件名降级推断——即视为该订阅的预期链；
 * 带 kind 时校验，未知/不匹配 kind 静默丢弃。返回进度文案所需的最小状态。
 */
function resolveBatchProgress(
  payload: BatchProgressEvent,
  expected: BatchProgressKind,
  productId: string,
): BatchProgress | null {
  if (payload.product_id !== productId) return null;
  if (payload.kind !== undefined && payload.kind !== expected) return null;
  return { completed: payload.completed, total: payload.total, name: payload.screen_name };
}

/** 首登验收失败原因码 → 括号内的一句人话；未覆盖码回退凭据错误码词表。 */
const ACCEPTANCE_REASON_SHORT: ReadonlyArray<readonly [RegExp, string]> = [
  [/credential_refresh_failed/i, "换发新登录凭据未通过"],
  [/credential_missing|credential_unavailable|credential_invalid/i, "登录凭据保存后不可用"],
  [/binding_mismatch|auth_mismatch/i, "登录凭据与账号绑定不一致"],
  [/manual_recovery_required/i, "登录凭据需要人工恢复"],
];

/** 验收失败原因不外露原文：短词表优先，兜底走 safeUiErrorMessage 的人话映射。 */
function acceptanceReasonText(errorCode: string | null | undefined): string {
  const text = errorCode ?? "";
  for (const [pattern, message] of ACCEPTANCE_REASON_SHORT) {
    if (pattern.test(text)) return message;
  }
  return errorCode ? safeUiErrorMessage(errorCode, "原因未知").replace(/。$/, "") : "原因未知";
}

// 登录浏览器方式：isolated=隔离实例（默认，与本机登录态互不影响）；
// system=本机浏览器默认 profile（复用系统已登录会话，一键授权当前账号）。
type LoginBrowserMode = "isolated" | "system";
// U-2 双视图/排序/过滤类型与控件已抽取到 shared/account/（P19-B1 步骤②）：
// AccountView ← AccountRow；AccountSort/AccountFilter + 控件 ← AccountListControls；
// localStorage 偏好读写 ← preferences（key 按产品分前缀）。

// 账号页只做一件事：账号档案。卡片矩阵（五要素）+ 独立详情视图（全量信息与操作）；
// 每日签到在「签到」页，密钥工具在「设置」页；切号由 P5-2 弹层事务承担，
// 专属实例启动/关闭入口已随 P6-2 退役（多开由「环境」概念承接，见环境页）。
export function AccountCenter({ active, adapter = null }: AccountCenterProps) {
  const [view, setView] = useState<ManagedAccountsViewDto | null>(null);
  const [adapterSnapshot, setAdapterSnapshot] = useState<AccountSnapshot | null>(null);
  const snapshotReadGeneration = useRef(0);
  const environmentReadGeneration = useRef(0);
  const currentProductId = adapter?.productId ?? null;
  // 视图/排序偏好的产品前缀：主路径（无适配器）即 TRAE 产品。
  const preferenceProductId = adapter?.productId ?? "trae_cn";
  const progressProductId: string = adapter?.productId ?? WORK_ARCHIVE_PRODUCT_ID;
  const currentProductIdRef = useRef(currentProductId);
  const activeRef = useRef(active);
  useLayoutEffect(() => {
    currentProductIdRef.current = currentProductId;
    activeRef.current = active;
    if (!active) {
      environmentReadGeneration.current += 1;
      setEnvObservedProfileId(null);
    }
  }, [active, currentProductId]);
  const [loading, setLoading] = useState(true);
  const [checkinCapability, setCheckinCapability] = useState<CheckinCapabilityDto | null>(null);
  // 账号总览（真实模式）：档案 + 积分缓存 + 令牌到期，驱动卡片与详情视图。
  const [overview, setOverview] = useState<readonly CheckinOverviewEntryDto[]>([]);
  const [loginBusy, setLoginBusy] = useState(false);
  const [loginAttemptId, setLoginAttemptId] = useState<number | null>(null);
  const loginAdapterRef = useRef<AccountAdapter | null>(null);
  // 登录浏览器方式（默认隔离实例）：添加账号时可切换“隔离浏览器 / 本机浏览器”。
  const [loginBrowserMode, setLoginBrowserMode] = useState<LoginBrowserMode>("isolated");
  // 批量操作仍全局互斥；记录来源链路，区分「本链路正在执行」和「等待上一批完成」。
  const [creditsBusyProductId, setCreditsBusyProductId] = useState<string | null>(null);
  const creditsBusy = creditsBusyProductId !== null;
  const creditsBusyForCurrentProduct = creditsBusyProductId === progressProductId;
  const [credentialRefreshBusyProductId, setCredentialRefreshBusyProductId] = useState<string | null>(null);
  const credentialRefreshBusy = credentialRefreshBusyProductId !== null;
  const credentialRefreshBusyForCurrentProduct = credentialRefreshBusyProductId === progressProductId;
  // 健康检测保持全局串行；记录来源链路，用于切页后准确显示等待状态。
  const [healthBusyProductId, setHealthBusyProductId] = useState<string | null>(null);
  const healthBusy = healthBusyProductId !== null;
  const healthBusyForCurrentProduct = healthBusyProductId === progressProductId;
  // 批量操作逐账号进度（P11-3，P12-R2c 收敛为单 state 按 kind 判别）：
  // 事件实时驱动「正在处理 i/N：账号名」文案；操作开始时清零，命令返回
  // 走既有结果摘要后随 busy 一起清除。三链按钮互斥不并发，渲染处按链取派生值。
  const [batchProgress, setBatchProgress] = useState<Partial<Record<BatchProgressKind, BatchProgress>>>({});
  // 额度刷新逐账号进度：refresh_checkin_credits 同时服务「刷新额度」与
  // 「健康检测网络步骤」，同一事件源且两流程按钮互斥不并发，单状态即可。
  const credentialProgress = batchProgress.credential_refresh ?? null;
  const healthProgress = batchProgress.health_check ?? null;
  const creditsProgress = batchProgress.credits_refresh ?? null;
  // 按 kind 写入/清除单条进度（null = 清除）；同值短路避免多余渲染。
  const setKindProgress = useCallback((kind: BatchProgressKind, progress: BatchProgress | null) => {
    setBatchProgress((current) => {
      const previous = current[kind] ?? null;
      if (previous === progress) return current;
      const next = { ...current };
      if (progress) next[kind] = progress;
      else delete next[kind];
      return next;
    });
  }, []);
  // 在浏览器绘制切换后的页面前清理上一产品进度，避免首帧泄露旧账号名。
  useLayoutEffect(() => {
    setBatchProgress((current) => Object.keys(current).length > 0 ? {} : current);
  }, [progressProductId]);
  // G15 纯本地刷新进行中（毫秒级本地读取，仅驱动按钮图标旋转）。
  const [localRefreshBusy, setLocalRefreshBusy] = useState(false);
  // 选中的账号（详情视图）由 AccountPageSkeleton 骨架管理（P19-B1 步骤③）。
  // U-2 双视图与排序：localStorage 记忆（脏值回退默认；key 按产品分前缀）。
  const [accountView, setAccountView] = useState<AccountView>(() =>
    readStoredPreference(accountPreferenceKey(preferenceProductId, "view"), "list", ["list", "card"] as const));
  const [accountSort, setAccountSort] = useState<AccountSort>(() =>
    readStoredPreference(accountPreferenceKey(preferenceProductId, "sort"), "added", ACCOUNT_SORT_OPTIONS));
  // G14 需处理过滤：会话内状态（「需处理」是临时排查意图，不写偏好）。
  const [accountFilter, setAccountFilter] = useState<AccountFilter>("all");
  // 登录凭据健康度（profile_id → 条目，P7-5 凭据包实调判定）：
  // login_state 为实调结果（驱动卡片徽章），archive_available 为存档次要信息。
  const [loginStates, setLoginStates] = useState<Record<string, TraeInstanceStateDto>>({});
  // 账号页只把本次实测到的官方登录身份标为「使用中」，环境缓存不作为登录证据。
  const [envObservedProfileId, setEnvObservedProfileId] = useState<string | null>(null);
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
  // 这里只跟踪 Trae Sync 内的账号选择，不代表官方客户端账号切换。
  const [toolAccountSelectionId, setToolAccountSelectionId] = useState<string | null>(null);

  const loadAdapterSnapshot = useCallback(async (sourceAdapter: AccountAdapter) => {
    if (currentProductIdRef.current !== sourceAdapter.productId) return;
    const generation = ++snapshotReadGeneration.current;
    const snapshot = await sourceAdapter.loadSnapshot();
    if (
      generation === snapshotReadGeneration.current &&
      currentProductIdRef.current === sourceAdapter.productId
    ) {
      setAdapterSnapshot(snapshot);
    }
  }, []);

  const load = useCallback(async () => {
    if (adapter) {
      await loadAdapterSnapshot(adapter);
      return;
    }
    const generation = ++snapshotReadGeneration.current;
    const nextView = await invoke<ManagedAccountsViewDto>("get_managed_account_state");
    if (generation === snapshotReadGeneration.current && currentProductIdRef.current === null) {
      setView(nextView);
    }
  }, [adapter, loadAdapterSnapshot]);

  useEffect(() => {
    if (!active) return;
    let cancelled = false;
    setLoading(true);
    void load().catch((reason: unknown) => {
      if (!cancelled) notify.error(safeUiErrorMessage(reason, "账号信息暂时不可读取，请稍后重试。"));
    }).finally(() => {
      if (!cancelled) setLoading(false);
    });
    return () => {
      cancelled = true;
      snapshotReadGeneration.current += 1;
    };
  }, [active, load]);

  // 产品变化时清空旧产品快照与实测身份（详情选中态由骨架按 productId 清理）。
  useEffect(() => {
    setAdapterSnapshot(null);
    environmentReadGeneration.current += 1;
    setEnvObservedProfileId(null);
  }, [currentProductId]);

  // 产品身份变化时取消未提交的批量确认，避免旧确认改由新适配器执行。
  useEffect(() => {
    setCredentialConfirmOpen(false);
  }, [currentProductId]);

  useEffect(() => {
    if (!active) return;
    let cancelled = false;
    if (adapter) return;
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
  }, [active, adapter]);

  // 真实模式：账号总览只在能力确认为真实 transport 后加载。
  useEffect(() => {
    if (adapter || !active || checkinCapability?.real_http_enabled !== true) return;
    let cancelled = false;
    void invoke<CheckinOverviewEntryDto[]>("get_checkin_overview")
      .then((entries) => { if (!cancelled) setOverview(entries); })
      .catch((reason: unknown) => {
        // 读取失败必须可见：静默置空会把真实故障伪装成“还没有账号”。
        if (!cancelled) {
          setOverview([]);
          notify.error(safeUiErrorMessage(reason, "账号总览读取失败。"));
        }
      });
    return () => { cancelled = true; };
  }, [active, adapter, checkinCapability?.real_http_enabled]);

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
    if (adapter || !active || checkinCapability?.real_http_enabled !== true || overviewProfileKey === "") {
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
  }, [active, adapter, checkinCapability?.real_http_enabled, overviewProfileKey]);

  // 批量操作逐账号进度事件（P11-3，P12-R2c 迁 useTauriEvents）：挂载期订阅、
  // 卸载清理。页面以 hidden 切换仍保持挂载，事件在页面隐藏期间也会到达并更新
  // 状态，重新显示即为最新进度；结果摘要由命令返回后的既有 notify.summary
  // 路径发布，这里只管进度文案。载荷统一 BatchProgressEvent：按事件名对应
  // 预期 kind 校验（暂无 kind 的旧载荷降级接受），未知 kind 静默丢弃。
  useTauriEvents({
    "credential-refresh-progress": (payload: BatchProgressEvent) => {
      const progress = resolveBatchProgress(payload, "credential_refresh", currentProductIdRef.current ?? WORK_ARCHIVE_PRODUCT_ID);
      if (progress) setKindProgress("credential_refresh", progress);
    },
    "health-check-progress": (payload: BatchProgressEvent) => {
      const progress = resolveBatchProgress(payload, "health_check", currentProductIdRef.current ?? WORK_ARCHIVE_PRODUCT_ID);
      if (progress) setKindProgress("health_check", progress);
    },
    "credits-refresh-progress": (payload: BatchProgressEvent) => {
      const progress = resolveBatchProgress(payload, "credits_refresh", currentProductIdRef.current ?? WORK_ARCHIVE_PRODUCT_ID);
      if (progress) setKindProgress("credits_refresh", progress);
    },
  }, []);

  // 账号卡片的「使用中」只依据本次从 TRAE 登录材料实测到的身份。
  const loadEnvironment = useCallback(async () => {
    const generation = ++environmentReadGeneration.current;
    if (!active || !activeRef.current || adapter || currentProductIdRef.current !== null) {
      setEnvObservedProfileId(null);
      return;
    }
    setEnvObservedProfileId(null);
    const env = await invoke<EnvironmentStateDto>("get_environment_state").catch(() => null);
    if (
      generation !== environmentReadGeneration.current ||
      !activeRef.current ||
      currentProductIdRef.current !== null
    ) return;
    setEnvObservedProfileId(env?.observed_profile_id ?? null);
  }, [active, adapter]);

  useEffect(() => {
    if (!active) return;
    void loadEnvironment();
  }, [active, loadEnvironment]);

  // 切号完成（弹层回执后）：重读实测身份与账号总览，卡片状态随之更新。
  const handleSwitchFinished = useCallback(async () => {
    if (adapter) return;
    await loadEnvironment();
    await refreshOverview();
  }, [adapter, loadEnvironment, refreshOverview]);

  // 共享账号工作台的本地刷新：适配器只重读自己的快照，不触碰 Work 数据。
  const refreshAdapterSnapshot = useCallback(async () => {
    if (!adapter) return;
    await loadAdapterSnapshot(adapter);
  }, [adapter, loadAdapterSnapshot]);

  // G15 纯本地刷新：重读账号总览与环境档案（均为本机缓存，零网络请求）。
  // 语义分工：刷新 = 重读缓存；健康检测 = 真实探测（含逐账号网络实调）。
  const handleLocalRefresh = useCallback(() => {
    if (localRefreshBusy) return;
    setLocalRefreshBusy(true);
    // 两个读取内部各自吞错（保持旧数据），完成后收起旋转动画。
    const refresh = adapter
      ? refreshAdapterSnapshot()
      : Promise.all([refreshOverview(), loadEnvironment()]).then(() => undefined);
    void refresh
      .catch((reason: unknown) => {
        if (adapter) notify.error(safeUiErrorMessage(reason, `${adapter.productName} 账号信息刷新失败。`));
      })
      .finally(() => setLocalRefreshBusy(false));
  }, [adapter, localRefreshBusy, refreshAdapterSnapshot, refreshOverview, loadEnvironment]);

  const realCheckinMode = !adapter && checkinCapability?.real_http_enabled === true;
  // 添加账号是核心需求：除 fixture 测试模式外常驻显示，不依赖签到能力开关。
  const canAddAccount = adapter
    ? adapter.capabilities.includes("login")
    : checkinCapability?.transport !== "fixture";

  // U-2 视图/排序偏好持久化（每次变更即写，量小无需防抖；key 按产品分前缀）。
  const changeAccountView = useCallback((next: AccountView) => {
    setAccountView(next);
    writeStoredPreference(accountPreferenceKey(preferenceProductId, "view"), next);
  }, [preferenceProductId]);
  const changeAccountSort = useCallback((next: AccountSort) => {
    setAccountSort(next);
    writeStoredPreference(accountPreferenceKey(preferenceProductId, "sort"), next);
  }, [preferenceProductId]);

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
    loginAdapterRef.current = adapter;
    try {
      if (adapter) {
        const receipt = await adapter.login(setLoginAttemptId);
        await refreshAdapterSnapshot();
        notify.success(`账号“${receipt.displayName}”登录成功，已保存到${adapter.productName}。`);
        // P11-3 首登验收：failed = 账号已保存但同设备换发验收未通过；如实
        // 警告、不自动重试也不发起第二次授权，passed 由成功提示覆盖不重复打扰。
        if (receipt.acceptance === "failed") {
          notify.warning(
            `账号“${receipt.displayName}”已保存，但可持续性验收未通过（${acceptanceReasonText(receipt.acceptanceErrorCode)}）。`,
            "可稍后在账号详情手动刷新凭据或重新登录。",
          );
        }
        return;
      }
      const begin = await invoke<{ attempt_id: number }>("begin_checkin_login", { useSystemBrowser });
      setLoginAttemptId(begin.attempt_id);
      const receipt = await invoke<CheckinLoginReceiptDto>("complete_checkin_login", { attemptId: begin.attempt_id });
      const entries = await refreshOverview();
      notify.success(`账号“${receipt.screen_name}”登录成功，已可用于签到。`);
      // P11-3 首登验收：failed = 凭据已入库但同设备换发验收未通过；如实警告，
      // 不自动重试、不发起第二次授权，passed 由成功提示覆盖不重复打扰。
      if (receipt.acceptance === "failed") {
        notify.warning(
          `账号“${receipt.screen_name}”已保存，但可持续性验收未通过（${acceptanceReasonText(receipt.acceptance_error_code)}）。`,
          "可稍后在账号详情手动刷新凭据或重新登录。",
        );
      }
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
        notify.info("登录已取消；若浏览器已关闭或未完成授权，可重新发起登录。");
      } else {
        notify.error(safeUiErrorMessage(reason, "登录未完成，请重新发起登录。"));
      }
    } finally {
      setLoginAttemptId(null);
      loginAdapterRef.current = null;
      setLoginBusy(false);
    }
  }, [adapter, loginBusy, refreshAdapterSnapshot, refreshOverview]);

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
        notify.success("手机号已保存，账号列表与详情页都会显示完整号码。");
        // 补录弹层发起的：保存成功后与原流程一致，一并关闭。
        if (request.closePrompt) setMobilePrompt(null);
      } catch (reason: unknown) {
        // 失败：补录弹层场景回弹层内提示；详情页场景走全局通知。
        const text = safeUiErrorMessage(reason, "手机号保存未完成，请稍后重试。");
        if (request.closePrompt) setMobilePromptError(text);
        else notify.error(text);
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
          notify.success("手机号已补全，账号列表与详情页都会显示完整号码。");
        }
      } catch (reason: unknown) {
        setMobilePromptError(safeUiErrorMessage(reason, "手机号保存失败，请稍后重试。"));
      } finally {
        setMobilePromptBusy(false);
      }
    })();
  }, [mobilePrompt, mobilePromptBusy, mobilePromptValue, saveMobileBackfill]);

  // 取消进行中的登录（P7-4）：置位后端取消标记，等待中的 complete 会以
  // login_cancelled 收尾。取消请求失败时警告提示（成功取消不需要打扰；
  // 等待仍有 complete 的超时与浏览器退出检测兜底，不会悬挂）。
  const handleCancelLogin = useCallback(async () => {
    if (loginAttemptId === null) return;
    try {
      await (loginAdapterRef.current
        ? loginAdapterRef.current.cancelLogin(loginAttemptId)
        : invoke("cancel_checkin_login", { attemptId: loginAttemptId }));
    } catch (reason: unknown) {
      if (typeof reason === "string" && reason === "login_cancel_too_late") {
        notify.info("登录已进入收尾阶段，正在等待真实结果。");
      } else if (
        typeof reason === "string" &&
        (reason === "login_already_finished" || reason === "login_not_started")
      ) {
        // complete 已结束而账号列表仍在刷新时，不要误报登录仍在等待。
        notify.info("登录已结束或正在收尾，请等待页面结果更新。");
      } else {
        notify.warning("取消登录未成功，登录等待将继续；完成或超时后会自动收尾。");
      }
    }
  }, [adapter, loginAttemptId]);

  const handleToolAccountSelection = useCallback(async (accountId: string, displayName: string) => {
    const sourceAdapter = adapter;
    if (!sourceAdapter || toolAccountSelectionId) return;
    setToolAccountSelectionId(accountId);
    let selectionCommitted = false;
    try {
      await sourceAdapter.selectToolAccount(accountId);
      selectionCommitted = true;
      await refreshAdapterSnapshot();
      notify.success(`已在 Trae Sync 中将“${displayName || "未命名账号"}”设为工具当前账号；官方客户端登录账号没有改变。`);
    } catch (reason: unknown) {
      if (selectionCommitted) {
        notify.warning(`${sourceAdapter.productName} 的工具账号选择已完成，但页面状态刷新失败，请点击刷新确认。`);
      } else {
        notify.error(safeUiErrorMessage(reason, `${sourceAdapter.productName} 工具账号选择未完成。`));
      }
    } finally {
      setToolAccountSelectionId(null);
    }
  }, [adapter, refreshAdapterSnapshot, toolAccountSelectionId]);

  // 适配器动作复用批量摘要通知与忙碌状态；具体协议只留在产品适配器内。
  // ADR-0031 决策 6：targetAccountId 非空时健康检测只针对该账号（远程退出联动）。
  const runAdapterAction = useCallback(async (action: AccountAction, targetAccountId?: string) => {
    if (!adapter) return;
    const isHealth = action === "health";
    if (isHealth ? healthBusy : credentialRefreshBusy) return;
    // 操作开始清掉上一批残留进度，避免新批次串显旧账号名。
    if (isHealth) { setKindProgress("health_check", null); setHealthBusyProductId(adapter.productId); }
    else {
      setKindProgress("credential_refresh", null);
      setCredentialRefreshBusyProductId(adapter.productId);
    }
    try {
      const result: AccountActionResult = await adapter.runAction(action, targetAccountId);
      await refreshAdapterSnapshot();
      const issues: NotifyIssue[] = result.issues.map((issue) => ({
        displayName: issue.displayName || "该账号",
        reason: safeUiErrorMessage(issue.errorCode, "网络或服务暂时不可用，请稍后重试。"),
      }));
      notify.summary(isHealth ? "健康检测完成" : "凭据刷新完成", issues, result.okCount);
    } catch (reason: unknown) {
      notify.error(safeUiErrorMessage(reason, isHealth ? "健康检测未完成，请稍后重试。" : "凭据刷新未完成，请稍后重试。"));
    } finally {
      if (isHealth) { setHealthBusyProductId(null); setKindProgress("health_check", null); }
      else {
        setCredentialRefreshBusyProductId(null);
        setKindProgress("credential_refresh", null);
      }
    }
  }, [adapter, credentialRefreshBusy, healthBusy, refreshAdapterSnapshot]);

  // 批量刷新额度：逐账号只读查询（后端串行、无间隔实时完成），刷新全部卡片。
  const handleRefreshAllCredits = useCallback(() => {
    if (creditsBusy || overview.length === 0) return;
    setKindProgress("credits_refresh", null);
    setCreditsBusyProductId(WORK_ARCHIVE_PRODUCT_ID);
    void (async () => {
      try {
        const profileIds = overview.map((entry) => entry.profile_id);
        const results = await invoke<CreditsRefreshEntryDto[]>("refresh_checkin_credits", { profileIds });
        await refreshOverview();
        const nameOf = new Map(overview.map((entry) => [entry.profile_id, effectiveDisplayName(entry)]));
        // 批量摘要通知：异常账号列名 + 一句人话原因；正常账号只计入成功数。
        const issues = results
          .filter((item) => item.error_code !== null)
          .map((item) => ({
            displayName: nameOf.get(item.profile_id) ?? item.screen_name,
            reason: safeUiErrorMessage(item.error_code, "网络或服务暂时不可用，请稍后重试。"),
          }));
        notify.summary("额度刷新完成", issues, results.length - issues.length);
      } catch (reason: unknown) {
        notify.error(safeUiErrorMessage(reason, "额度刷新未完成，请稍后重试。"));
      } finally {
        setCreditsBusyProductId(null);
        setKindProgress("credits_refresh", null);
      }
    })();
  }, [creditsBusy, overview, refreshOverview]);

  // 批量刷新登录凭据入口：只负责打开二次确认弹层（批量换发写回全部账号
  // 本机登录信息，防误触），执行体在下方。P15-7：适配器（融合版）路径不再
  // 提供凭据刷新——登录代次由融合版客户端自动续期（ADR-0035 决策 7）。
  const handleRefreshAllCredentials = useCallback(() => {
    if (credentialRefreshBusy || overview.length === 0) return;
    setCredentialConfirmOpen(true);
  }, [credentialRefreshBusy, overview.length]);

  // 批量刷新凭据确认后执行。
  const executeRefreshAllCredentials = useCallback(() => {
    setCredentialConfirmOpen(false);
    if (credentialRefreshBusy || overview.length === 0) return;
    setKindProgress("credential_refresh", null);
    setCredentialRefreshBusyProductId(WORK_ARCHIVE_PRODUCT_ID);
    void (async () => {
      try {
        const profileIds = overview.map((entry) => entry.profile_id);
        const results = await invoke<CredentialRefreshEntryDto[]>("refresh_checkin_credentials", { profileIds });
        await refreshOverview();
        const nameOf = new Map(overview.map((entry) => [entry.profile_id, effectiveDisplayName(entry)]));
        const issues = results
          .filter((item) => !item.refreshed)
          .map((item) => ({
            displayName: (nameOf.get(item.profile_id) ?? item.screen_name) || "该账号",
            reason: safeUiErrorMessage(item.error_code, "登录凭据刷新未完成，请稍后重试。"),
          }));
        notify.summary("凭据刷新完成", issues, results.filter((item) => item.refreshed).length);
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
        notify.error(safeUiErrorMessage(reason, "凭据刷新未完成，请稍后重试。"));
      } finally {
        setCredentialRefreshBusyProductId(null);
        setKindProgress("credential_refresh", null);
      }
    })();
  }, [credentialRefreshBusy, overview, refreshOverview]);

  // 一键健康检测：登录存档深度检测（storage.json 键 + 最近启动日志证据，
  // 秒级）→ 签到会话网络探测（复用 refresh_checkin_credits 只读查询，顺带
  // 刷新额度与令牌时间戳）→ 徽章即时更新 + G9 结果卡。
  // ADR-0031 决策 6：远程退出设备后传目标 profile_id，只检测该账号；
  // 手动全量检测不传参，行为与之前一致。
  const handleHealthCheck = useCallback((targetProfileId?: string | null) => {
    if (adapter) {
      if ((adapterSnapshot?.accounts.length ?? 0) === 0) return;
      void runAdapterAction("health", targetProfileId ?? undefined);
      return;
    }
    // 单账号目标：范围收缩为该账号（检测范围与结果卡都只含目标）。
    const targets = targetProfileId
      ? overview.filter((entry) => entry.profile_id === targetProfileId)
      : overview;
    if (healthBusy || targets.length === 0) return;
    setKindProgress("health_check", null);
    setKindProgress("credits_refresh", null);
    setHealthBusyProductId(WORK_ARCHIVE_PRODUCT_ID);
    void (async () => {
      try {
        const profileIds = targets.map((entry) => entry.profile_id);
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
        const health = buildHealthResult(states, results, targets);
        notify.summary("健康检测完成", health.issues, health.okCount);
      } catch (reason: unknown) {
        notify.error(safeUiErrorMessage(reason, "健康检测未完成，请稍后重试。"));
      } finally {
        setHealthBusyProductId(null);
        setKindProgress("health_check", null);
        setKindProgress("credits_refresh", null);
      }
    })();
  }, [adapter, adapterSnapshot?.accounts.length, healthBusy, overview, refreshOverview, runAdapterAction]);

  // G11 登录后手机号补录弹层（共享 JSX）：列表视图与详情视图都渲染——
  // 从详情页发起重新登录时弹层不再滞留到返回列表后才出现。

  const credentialRefreshCount = adapter ? adapterSnapshot?.accounts.length ?? 0 : overview.length;

  // 批量刷新凭据二次确认弹层（仅列表视图渲染——入口按钮只在列表页头部）。
  const credentialConfirmDialog = credentialConfirmOpen && (
    <ConfirmDialog
      title="刷新全部账号凭据"
      lines={[
        `将为全部 ${credentialRefreshCount} 个账号逐个向服务端换发新的登录令牌，并更新本机保存的登录信息。`,
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

  // —— P19-B1 步骤③：AccountCenter 退化为 TRAE 产品插槽提供方，页面编排
  // （门控渲染/页头/控件行/列表/空态/详情切换）由 AccountPageSkeleton 承担
  // （ADR-0036 铁律：禁止骨架外的产品分支布局）。—— 

  const adapterSnapshotStale = Boolean(
    adapter && adapterSnapshot && adapterSnapshot.productId !== adapter.productId,
  );
  const uiProduct = productUi(adapter?.productId ?? "trae_cn");
  const savedAccounts = view?.saved_accounts ?? [];
  const adapterAccounts = adapterSnapshot?.accounts ?? [];

  // 详情视图插槽：真实模式与适配器模式各自命中（profile_id 空间按产品隔离）。
  const renderDetailView = (profileId: string, ops: { onBack(): void }) => {
    if (!adapter) {
      const entry = overview.find((item) => item.profile_id === profileId) ?? null;
      if (!entry) return null;
      return (
        <AccountDetail
          entry={entry}
          loginState={loginStates[entry.profile_id]}
          currentAccountStatus={envObservedProfileId === null
            ? "unknown"
            : envObservedProfileId === entry.profile_id
              ? "current"
              : "other"}
          onRelogin={() => handleLogin(false)}
          loginBusy={loginBusy}
          onDataChanged={async () => {
            await refreshOverview();
          }}
          onSaveMobile={(mobile) => saveMobileBackfill(entry.profile_id, mobile)}
          onBack={ops.onBack}
        />
      );
    }
    const account = adapterAccounts.find((item) => item.id === profileId) ?? null;
    if (!account) return null;
    return (
      <AdapterAccountDetail
        account={account}
        productId={adapter.productId}
        productName={adapter.productName}
        canSelectToolAccount={adapter.capabilities.includes("toolSelection")}
        toolSelectionBusy={toolAccountSelectionId !== null}
        onSelectToolAccount={() => void handleToolAccountSelection(account.id, account.displayName)}
        // 退出登录设备后联动该账号健康检测（handleHealthCheck 自带忙碌守卫）。
        onSignedOut={() => handleHealthCheck(account.id)}
        onBack={ops.onBack}
      />
    );
  };

  const visibleAdapterAccounts = accountSort === "name"
    ? [...adapterAccounts].sort((a, b) => a.displayName.localeCompare(b.displayName, "zh-Hans-CN"))
    : adapterAccounts;
  const sortedSavedAccounts = accountSort === "name"
    ? [...savedAccounts].sort((a, b) => a.display_name.localeCompare(b.display_name, "zh-Hans-CN"))
    : savedAccounts;
  const hasAccountEntries = realCheckinMode
    ? overview.length > 0
    : adapterAccounts.length > 0 || savedAccounts.length > 0;
  const hasHealthCapability = realCheckinMode || adapter?.capabilities.includes("health") === true;
  // P15-7：融合版登录代次由客户端自动续期，凭据刷新仅服务 Work 签到档案。
  const hasCredentialRefreshCapability = realCheckinMode;

  // 健康检测进度展示源：CN 路径事件优先，Work 网络步骤事件兜底（见渲染区注释）。
  const healthProgressShown = healthProgress ?? creditsProgress;

  // 行插槽：产品层决定徽章组合与 meta（DTO→SharedAccountRecord 的映射留在产品层）。
  const overviewById = new Map(overview.map((entry) => [entry.profile_id, entry]));
  const savedById = new Map(savedAccounts.map((account) => [account.profile_id, account]));
  const renderRow = (record: SharedAccountRecord, ops: { onOpen(): void }) => {
    if (adapter) {
      const account = adapterAccounts.find((item) => item.id === record.id);
      if (!account) return null;
      return (
        <AdapterAccountCard
          account={account}
          variant={accountView}
          toolSelectionBusy={toolAccountSelectionId !== null}
          canSelectToolAccount={adapter.capabilities.includes("toolSelection")}
          onOpen={ops.onOpen}
          onSelectToolAccount={() => void handleToolAccountSelection(account.id, account.displayName)}
        />
      );
    }
    if (realCheckinMode) {
      const entry = overviewById.get(record.id);
      if (!entry) return null;
      return (
        <AccountCard
          entry={entry}
          variant={accountView}
          loginState={effectiveCredentialState(entry, loginStates[entry.profile_id])}
          archiveAvailable={loginStates[entry.profile_id]?.archive_available}
          isCurrentAccount={envObservedProfileId === entry.profile_id}
          switchBusy={switchTarget !== null}
          onOpen={ops.onOpen}
          onSwitch={() => setSwitchTarget({
            profile_id: entry.profile_id,
            display_name: effectiveDisplayName(entry),
          })}
        />
      );
    }
    const saved = savedById.get(record.id);
    if (!saved) return null;
    return (
      <SharedAccountEntry
        account={record}
        variant={accountView}
        testId={`account-saved-card-${saved.profile_id}`}
        actionTestId={`account-saved-switch-${saved.profile_id}`}
        selectedTestId={`account-saved-current-${saved.profile_id}`}
        statusContent={<span className="slot-badge slot-badge--idle">状态待验证</span>}
        listMeta={<p className="account-item__meta">最近验证：{formatDateTime(saved.last_verified_at)}{saved.region ? ` · ${saved.region.toUpperCase()}` : ""}</p>}
        cardMeta={<p className="account-card__meta">最近验证：{formatDateTime(saved.last_verified_at)}{saved.region ? ` · ${saved.region.toUpperCase()}` : ""}</p>}
        actionBusy={false}
        hasAction={false}
      />
    );
  };

  // 列表条目（已排序/过滤，产品层映射为 SharedAccountRecord）。
  const entries: readonly SharedAccountRecord[] = adapter
    ? visibleAdapterAccounts.map((account) => ({
      id: account.id,
      displayName: account.displayName,
      lastVerifiedAt: account.lastVerifiedAt,
      isSelected: account.isToolSelected,
    }))
    : realCheckinMode
      ? visibleOverview.map((entry) => ({
        id: entry.profile_id,
        displayName: effectiveDisplayName(entry),
        lastVerifiedAt: entry.last_verified_at,
        isSelected: envObservedProfileId === entry.profile_id,
      }))
      : sortedSavedAccounts.map((account) => ({
        id: account.profile_id,
        displayName: account.display_name,
        lastVerifiedAt: account.last_verified_at,
        isSelected: account.profile_id === view?.current_account.profile_id,
      }));

  // 空态：产品层按数据形态给文案与动作（与迁移前逐字一致）。
  const emptyState = adapter
    ? {
      className: "account-center__empty-actions",
      description: "尚未保存账号。点击右上角“添加账号”通过浏览器登录。",
      action: (
        <button className="btn btn--primary" type="button" onClick={() => void handleLogin(true)} disabled={loginBusy}>
          <UserPlus size={15} aria-hidden="true" />{loginBusy ? "等待浏览器登录完成…" : "添加账号"}
        </button>
      ),
    }
    : realCheckinMode
      ? (overview.length === 0
        ? {
          className: "account-center__empty-actions",
          description: "还没有账号。通过浏览器登录添加第一个账号，登录成功后即可签到。",
          action: (
            <button className="btn btn--primary" type="button" onClick={() => void handleLogin(loginBrowserMode === "system")} disabled={loginBusy}>
              <UserPlus size={15} aria-hidden="true" />{loginBusy ? "等待浏览器登录完成…" : "添加账号"}
            </button>
          ),
        }
        : // G14 过滤空态：有账号但当前过滤无匹配，与「还没有账号」区分。
        { className: "account-center__empty", description: "没有需要处理的账号。" })
      : { className: "account-center__empty", description: "尚未保存账号。点击右上角“添加账号”通过浏览器登录。" };

  return (
    <AccountPageSkeleton
      product={uiProduct}
      active={active}
      loading={loading || adapterSnapshotStale}
      ariaLabel={adapter ? `${adapter.productName} 账号` : "账号"}
      entries={entries}
      renderRow={renderRow}
      headerActions={(
        <>
          <span className="status-badge status-badge--neutral"><ShieldCheck size={14} aria-hidden="true" />登录信息仅本机加密保存</span>
          {adapter && (
            <span className="badge badge--off" data-testid="product-credential-status">
              {adapterSnapshot?.statusLabel ?? "正在读取"}
            </span>
          )}
          {hasAccountEntries && (
            <>
              {/* G15 纯本地刷新：重读缓存零联网，与「健康检测」的真实探测分工。 */}
              <button
                className="btn"
                type="button"
                onClick={handleLocalRefresh}
                disabled={localRefreshBusy || credentialRefreshBusy}
                data-testid="account-refresh-local"
                title="刷新本地数据，不联网检测"
              >
                <RefreshCw size={15} className={localRefreshBusy ? "icon-spin" : undefined} aria-hidden="true" />刷新
              </button>
              {(hasHealthCapability || hasCredentialRefreshCapability || realCheckinMode) && (
                <>
                  {hasHealthCapability && (
                    <button className="btn" type="button" onClick={() => handleHealthCheck()} disabled={healthBusy || creditsBusy || credentialRefreshBusy || loginBusy} data-testid="account-health-check">
                      <HeartPulse size={15} aria-hidden="true" />{healthBusy ? healthBusyForCurrentProduct ? "检测中…" : "等待检测完成…" : "健康检测"}
                    </button>
                  )}
                  {realCheckinMode && (
                    <button className="btn" type="button" onClick={handleRefreshAllCredits} disabled={creditsBusy || loginBusy || healthBusy || credentialRefreshBusy} data-testid="account-refresh-credits">
                      <RefreshCw size={15} aria-hidden="true" />{creditsBusy ? creditsBusyForCurrentProduct ? "查询中…" : "等待查询完成…" : "刷新额度"}
                    </button>
                  )}
                  {hasCredentialRefreshCapability && (
                    <button className="btn btn--primary" type="button" onClick={handleRefreshAllCredentials} disabled={credentialRefreshBusy || loginBusy || healthBusy || creditsBusy} data-testid="account-refresh-credentials">
                      <RefreshCw size={15} aria-hidden="true" />{credentialRefreshBusy ? credentialRefreshBusyForCurrentProduct ? "刷新中…" : "等待刷新完成…" : "刷新凭据"}
                    </button>
                  )}
                </>
              )}
            </>
          )}
          {canAddAccount && (
            <div className="page-header__actions-group" data-testid="account-add-group">
              {/* 登录方式选择：隔离浏览器（默认，与本机登录态互不影响）/
                  本机浏览器（复用系统已登录会话，一键授权当前账号）。 */}
              {!adapter && <select
                aria-label="登录浏览器方式"
                className="page-header__select"
                value={loginBrowserMode}
                disabled={loginBusy || credentialRefreshBusy || healthBusy || creditsBusy}
                onChange={(event) => setLoginBrowserMode(event.target.value as LoginBrowserMode)}
                data-testid="login-browser-mode"
              >
                <option value="isolated">隔离浏览器</option>
                <option value="system">本机浏览器</option>
              </select>}
              <button className="btn btn--primary" type="button" onClick={() => void handleLogin(loginBrowserMode === "system")} disabled={loginBusy || credentialRefreshBusy || healthBusy || creditsBusy} data-testid="account-add-primary">
                <UserPlus size={15} aria-hidden="true" />{loginBusy ? "等待浏览器登录完成…" : `添加${adapter ? `${adapter.productName} ` : ""}账号`}
              </button>
            </div>
          )}
        </>
      )}
      statusRegion={(
        <>
          {loginBusy && (
            <div className="account-center__login-waiting" role="status">
              <p className="account-center__meta">
                {adapter
                  ? `已在本机浏览器打开 ${adapter.productName} 登录页（复用网页登录会话），完成授权后此处会自动继续；期间无需手动刷新。`
                  : loginBrowserMode === "system"
                  ? "已在本机浏览器打开 TRAE 登录页（复用系统登录态），完成授权后此处会自动继续；期间无需手动刷新。"
                  : "已在隔离浏览器打开 TRAE 登录页，完成登录后此处会自动继续；期间无需手动刷新。"}
              </p>
              {/* P7-4：等待不再只能干等 5 分钟——随时可取消（关闭隔离浏览器窗口同样会收尾） */}
              <button
                className="btn btn--quiet"
                type="button"
                onClick={() => void handleCancelLogin()}
                disabled={loginAttemptId === null}
                data-testid="account-cancel-login"
              >
                取消登录
              </button>
            </div>
          )}
          {creditsBusy && <p className="account-center__meta" role="status">
            {creditsProgress
              ? `正在处理 ${creditsProgress.completed}/${creditsProgress.total}：${creditsProgress.name}`
              : creditsBusyForCurrentProduct
                ? "正在查询最新额度…"
                : "账号额度查询正在进行，完成后可继续。"}
          </p>}
          {/* P11-3：批量凭据刷新收到逐账号进度事件时升级为「正在处理 i/N：账号名」；
              未收到事件（首批账号尚在处理）时保留原说明文案。 */}
          {credentialRefreshBusy && <p className="account-center__meta" role="status">
            {credentialProgress
              ? `正在处理 ${credentialProgress.completed}/${credentialProgress.total}：${credentialProgress.name}`
              : credentialRefreshBusyForCurrentProduct
                ? adapter ? `正在刷新 ${adapter.productName} 登录凭据，不会打开新的 OAuth 登录…` : "正在刷新登录凭据，不会打开新的 OAuth 登录…"
                : "账号凭据刷新正在进行，完成后可继续。"}
          </p>}
          {/* 健康检测进度：health-check-progress 由 CN 适配路径推送；Work 的网络
              步骤（refresh_checkin_credits）复用 credits-refresh-progress——先看
              CN 事件、再看网络步骤事件，均未到达（本地检测阶段）保留原回退文案。 */}
          {healthBusy && <p className="account-center__meta" role="status">
            {healthProgressShown
              ? `正在处理 ${healthProgressShown.completed}/${healthProgressShown.total}：${healthProgressShown.name}`
              : healthBusyForCurrentProduct
                ? adapter ? `正在检测 ${adapter.productName} 账号健康度…` : "正在检测账号健康度（登录存档 + 签到会话探测）…"
                : "账号健康检测正在进行，完成后可继续。"}
          </p>}
        </>
      )}
      entryCount={adapter ? adapterAccounts.length : realCheckinMode ? overview.length : savedAccounts.length}
      showControls={hasAccountEntries}
      controls={{
        view: accountView,
        onViewChange: changeAccountView,
        sort: accountSort,
        sortOptions: realCheckinMode ? ACCOUNT_SORT_OPTIONS : ACCOUNT_SORT_OPTIONS.filter((option) => option !== "checkin"),
        onSortChange: changeAccountSort,
        filter: realCheckinMode
          ? { value: accountFilter, attentionCount, onChange: setAccountFilter }
          : undefined,
      }}
      listPrelude={adapter && adapterSnapshot ? (
        <div className="account-center__current" data-testid="account-adapter-identity">
          <strong>{adapterSnapshot.officialIdentityLabel}</strong>
          <span>{adapterSnapshot.officialIdentityMessage}</span>
        </div>
      ) : null}
      emptyState={emptyState}
      detailView={renderDetailView}
      selectionResetKey={adapter?.productId ?? "-"}
      dialogs={(
        <>
          {/* P5-2 切号进度弹层：target 非空即打开（五步事务由后端编排，见 MasterSwitchDialog）。 */}
          <MasterSwitchDialog
            target={switchTarget}
            onFinished={handleSwitchFinished}
            onClose={() => setSwitchTarget(null)}
          />
          {/* G11 登录后手机号补录弹层与查重确认（详情视图同样渲染）。 */}
          {mobilePromptDialog}
          {duplicateMobileDialog}
          {/* 批量刷新凭据二次确认弹层（入口按钮在列表页头部）。 */}
          {credentialConfirmDialog}
        </>
      )}
    />
  );
}

// SharedAccountEntry / SharedAccountRecord / avatarLetter 已纯移动到
// shared/account/AccountRow.tsx（P19-B1 步骤①：产品无关组件归位共享层）。

function AdapterAccountCard({
  account,
  variant,
  toolSelectionBusy,
  canSelectToolAccount,
  onOpen,
  onSelectToolAccount,
}: {
  account: AdapterAccountView;
  variant: AccountView;
  toolSelectionBusy: boolean;
  canSelectToolAccount: boolean;
  /** 点击卡片进入账号详情（P10-5：设备管理绑定在详情页）。 */
  onOpen: () => void;
  onSelectToolAccount: () => void;
}) {
  const statusClass = account.status === "active" ? "slot-badge--ok" : account.status === "expired" ? "slot-badge--danger" : "slot-badge--idle";
  const statusLabel = account.status === "active" ? "登录有效" : account.status === "expired" ? "登录已过期" : "状态待验证";
  const statusContent = (
    <span className={`slot-badge ${statusClass}`} title="TRAE 当前保存凭据的状态">
      {statusLabel}
    </span>
  );
  const listMeta = <p className="account-item__meta">最近验证：{formatDateTime(account.lastVerifiedAt)}</p>;
  const cardMeta = <p className="account-card__meta">最近验证：{formatDateTime(account.lastVerifiedAt)}</p>;
  return (
    <SharedAccountEntry
      account={{
        id: account.id,
        displayName: account.displayName,
        lastVerifiedAt: account.lastVerifiedAt,
        isSelected: account.isToolSelected,
      }}
      variant={variant}
      testId={`account-adapter-card-${account.id}`}
      actionTestId="account-adapter-tool-select"
      selectedTestId={`account-adapter-current-${account.id}`}
      statusContent={statusContent}
      listMeta={listMeta}
      cardMeta={cardMeta}
      actionBusy={toolSelectionBusy}
      hasAction={canSelectToolAccount}
      onOpen={onOpen}
      onAction={onSelectToolAccount}
      selectedLabel="工具内已选"
      selectedTitle="这是 Trae Sync 中选中的账号，不代表官方客户端账号已切换。"
      actionLabel="设为工具当前账号"
      actionBusyLabel="设置中…"
      actionTitle="仅更新 Trae Sync 中的账号选择，不会切换官方客户端登录账号。"
    />
  );
}

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
  /** 本次从 TRAE 登录材料实测确认的主库账号：展示「使用中」并隐藏切换入口。 */
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

  const statusContent = (
    <>
      <CheckinSlotBadge state={checkinState} />
      <LoginArchiveSlotBadge state={loginSlot} archiveAvailable={archiveAvailable} />
    </>
  );
  const listMeta = (
    <>
      <p className="account-item__meta">最近验证：{formatDateTime(entry.last_verified_at)}</p>
      {displayMobile(entry) && <p className="account-item__meta">{displayMobile(entry)}</p>}
      <p className="account-item__meta account-item__token-expiry">{tokenRemaining}</p>
    </>
  );
  const cardMeta = (
    <>
      <p className="account-card__meta">最近验证：{formatDateTime(entry.last_verified_at)}</p>
      {displayMobile(entry) && <p className="account-card__meta">{displayMobile(entry)}</p>}
      <p className="account-card__meta account-card__token-expiry">{tokenRemaining}</p>
    </>
  );

  return (
    <SharedAccountEntry
      account={{
        id: entry.profile_id,
        displayName,
        lastVerifiedAt: entry.last_verified_at,
        isSelected: isCurrentAccount,
      }}
      variant={variant}
      testId={`account-card-${entry.profile_id}`}
      actionTestId={`account-switch-${entry.profile_id}`}
      selectedTestId={`account-current-${entry.profile_id}`}
      statusContent={statusContent}
      listMeta={listMeta}
      cardMeta={cardMeta}
      listMetric={creditsBlock}
      cardMetric={cardCredits}
      actionBusy={switchBusy}
      onOpen={onOpen}
      onAction={onSwitch}
      actionTitle={`切换主库到 ${displayName}（全部对话记录保持不变）`}
      selectedTitle="TRAE 登录材料中识别到的账号；不代表客户端进程正在运行。"
    />
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
 * 结果经 notify.summary 发布（批量摘要通知：成功数 + 异常清单）。
 */
function buildHealthResult(
  states: readonly TraeInstanceStateDto[] | null,
  results: readonly CreditsRefreshEntryDto[],
  overview: readonly CheckinOverviewEntryDto[],
): { okCount: number; issues: NotifyIssue[] } {
  const nameOf = new Map(overview.map((entry) => [entry.profile_id, effectiveDisplayName(entry)]));
  const stateOf = new Map((states ?? []).map((state) => [state.profile_id, state]));
  const probeOf = new Map(results.map((result) => [result.profile_id, result]));
  // 统计口径同 effectiveCredentialState：本次探测到续期被拒也计入“登录失效”。
  const renewalDead = new Set(
    results
      .filter((item) => CREDENTIAL_RELOGIN_ERROR_CODES.has(item.error_code ?? ""))
      .map((item) => item.profile_id),
  );
  const issues: NotifyIssue[] = [];
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
      issues.push({ displayName: nameOf.get(entry.profile_id) ?? entry.screen_name, reason: loginReason });
      continue;
    }
    // 登录正常才单独报会话异常（登录失效的探测失败是同一根因，不重复列）。
    const probe = probeOf.get(entry.profile_id);
    if (probe && probe.error_code !== null) {
      issues.push({
        displayName: nameOf.get(entry.profile_id) ?? entry.screen_name,
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
