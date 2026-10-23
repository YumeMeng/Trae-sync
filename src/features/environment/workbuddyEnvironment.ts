/**
 * WorkBuddy 环境页 builder（P4d-4）：WB 快照 → EnvironmentView 的纯映射（零 React / 零 JSX / 零命令 / 零 invoke）。
 * 对照 WorkBuddyEnvironmentPage.tsx（简写 WB）：页头 WB 85-97、客户端连接卡 WB 117-152、登录域健康卡 WB 157-196、
 * 多实例 pending WB 199-205。WB 无启停（P19 决策 10 只读状态卡）、无体检、无环境登录；能力只读 envCapabilitiesFor("workbuddy")。
 * 旧页两张卡：客户端连接进 master 槽（共用骨架），登录域健康进 workbuddy.healthCard（config 无第二卡槽，见报告 §1）。
 */
import type { WorkBuddyDeepHealthDto, WorkBuddySessionHealthDto } from "../../types/product";
import type { WorkBuddyProbeInfoDto } from "../../types/workbuddyBackup";
import {
  ENV_TEXTS, envCapabilitiesFor, type EnvActionView, type EnvCardView, type EnvProcessState,
  type EnvSecondarySectionView, type EnvironmentCapabilities, type EnvironmentView,
} from "./config";

/** 旧页 data-testid 逐字（WB L90/117/124/157/171/188）；pending 区锚点为本票新增（旧页 EmptyState 无锚点）。 */
export const WB_ENV_TEST_IDS = {
  refresh: "workbuddy-env-refresh", clientCard: "workbuddy-env-client-card", clientRuntime: "workbuddy-env-client-runtime",
  healthCard: "workbuddy-env-health-card", deepResult: "workbuddy-env-deep-result", deepHealthButton: "workbuddy-env-deep-health-button",
  multiPending: "workbuddy-env-multi-pending",
} as const;

/** 聚合页面需要的所有已取回数据：builder 只消费，不发起任何读取；null 一律按「未读到」降级。 */
export interface WorkBuddyEnvironmentSnapshot {
  readonly sessionHealth: WorkBuddySessionHealthDto | null; // get_workbuddy_session_health（文件面只读，不含令牌）
  readonly deepHealth: WorkBuddyDeepHealthDto | null; // check_workbuddy_login_health（深度健康，只读零写）
  readonly probe: WorkBuddyProbeInfoDto | null; // get_workbuddy_probe_info（全维度独立降级）
  readonly checking: boolean; // 深度检测执行中（按钮禁用 +「检测中…」）
  readonly loading: boolean; // 首读中
  readonly error: string | null; // 首读失败提示（WB L46/L58）
}

/** 视图模型：EnvironmentView + WB 独有的登录域健康卡块与两个内部状态锚点（P4d-8 挂到状态块上）。 */
export interface WorkBuddyEnvironmentView extends EnvironmentView {
  readonly workbuddy: {
    readonly healthCard: EnvCardView | null; // 登录域健康卡（health 未读到即 null）
    readonly clientRuntimeTestId: string; // 客户端运行状态徽章
    readonly deepResultTestId: string; // 深度检测结果徽章
  };
}

/** 动作构造：旧实现 data-testid 与动作 id 同值，故 testId = id；hint 即旧 title；disabledReason 非空即禁用。 */
const act = (id: string, label: string, kind: EnvActionView["kind"], hint: string | null = null): EnvActionView =>
  ({ id, label, kind, testId: id, hint, disabledReason: null, busyLabel: null });
/** 登录文件状态词（WB L67-72 逐字）：unknown 是兜底档。 */
const FILE_WORDS: Record<WorkBuddySessionHealthDto["status"], string> = {
  active: ENV_TEXTS.wbFileActive, access_expired: ENV_TEXTS.wbFileAccessExpired, fully_expired: ENV_TEXTS.wbFileFullyExpired,
  logged_out: ENV_TEXTS.wbFileLoggedOut, file_missing: ENV_TEXTS.wbFileMissing, unknown: ENV_TEXTS.wbFileUnknown,
};
/** 同一 live_status 两套口径逐字保留：客户端运行状态（WB L128-136）/ 深度检测结果（WB L177-186）。 */
const RUNTIME_WORDS: Record<WorkBuddyDeepHealthDto["live_status"], string> = {
  verified: ENV_TEXTS.wbRuntimeVerified, client_offline: ENV_TEXTS.wbRuntimeOffline, probe_failed: ENV_TEXTS.wbRuntimeProbeFailed,
};
const DEEP_WORDS: Record<WorkBuddyDeepHealthDto["live_status"], string> = {
  verified: ENV_TEXTS.wbDeepVerified, client_offline: ENV_TEXTS.wbDeepOffline, probe_failed: ENV_TEXTS.wbDeepFailed,
};
/** 本地登录态词（WB L162-165 逐字）：active=正常 / access_expired=将过期 / 其余（含未读到）=异常。 */
const localWordOf = (health: WorkBuddySessionHealthDto): string =>
  health.status === "active" ? ENV_TEXTS.wbLocalOk : health.status === "access_expired" ? ENV_TEXTS.wbLocalWarn : ENV_TEXTS.wbLocalBad;
/** Unix 秒 → 本地「日期 时:分」；0 = 未知返回 null（WB L210-219 同口径）。 */
const formatUnixTime = (unixSeconds: number): string | null => unixSeconds
  ? new Date(unixSeconds * 1000).toLocaleString(undefined, { year: "numeric", month: "2-digit", day: "2-digit", hour: "2-digit", minute: "2-digit" })
  : null;
/** 运行态三词：runtime=客户端运行状态行、deep=深度检测行（null=未读到不渲染）、state 供状态块色档（WB L122-136）。 */
export function workbuddyLiveLabels(deepHealth: WorkBuddyDeepHealthDto | null, checking: boolean): {
  readonly runtime: string; readonly deep: string | null; readonly state: EnvProcessState } {
  if (deepHealth === null) return { runtime: checking ? ENV_TEXTS.wbChecking : ENV_TEXTS.wbNotChecked, deep: null, state: "unknown" };
  const live = deepHealth.live_status;
  return { runtime: RUNTIME_WORDS[live], deep: DEEP_WORDS[live], state: live === "client_offline" ? "not_running" : "running" };
}
/** 客户端连接卡（WB L117-152）：登录文件状态是主信息，版本在路径位，安装路径/数据根/续期凭据只进悬浮。 */
function clientCardFor(snapshot: WorkBuddyEnvironmentSnapshot, health: WorkBuddySessionHealthDto, labels: ReturnType<typeof workbuddyLiveLabels>): EnvCardView {
  const unknown = ENV_TEXTS.wbUnknown, probe = snapshot.probe;
  const access = formatUnixTime(health.access_expires_at_unix_seconds) ?? unknown;
  const renew = formatUnixTime(health.refresh_expires_at_unix_seconds) ?? unknown;
  const pool = health.captured_in_pool ? ENV_TEXTS.wbCaptured : ENV_TEXTS.wbNotCaptured;
  return {
    id: "workbuddy-client", testId: WB_ENV_TEST_IDS.clientCard, name: ENV_TEXTS.wbClientCard, badge: "", icon: "database", // 旧页客户端卡无徽章
    currentAccountName: FILE_WORDS[health.status], currentAccountMeta: `${ENV_TEXTS.wbAccessExpires} ${access}；${ENV_TEXTS.wbPool}：${pool}`,
    process: { state: labels.state, launchLabel: labels.runtime, launchHint: ENV_TEXTS.wbRuntime }, // 只读：无启停动作
    pathLabel: `${ENV_TEXTS.wbVersion} ${probe?.version ?? unknown}`, actions: [],
    pathTitle: `${ENV_TEXTS.wbInstallPath}：${probe?.install_path ?? unknown}；${ENV_TEXTS.wbDataRoot}：${probe?.data_root ?? unknown}；${ENV_TEXTS.wbRefreshExpires} ${renew}`,
  };
}
/** 登录域健康卡（WB L157-196）：本地登录态是徽章，深度检测结果在路径位，唯一动作是只读检测。 */
function healthCardFor(snapshot: WorkBuddyEnvironmentSnapshot, health: WorkBuddySessionHealthDto, labels: ReturnType<typeof workbuddyLiveLabels>): EnvCardView {
  const detect: EnvActionView = { ...act(WB_ENV_TEST_IDS.deepHealthButton, ENV_TEXTS.wbDetect, "ghost", ENV_TEXTS.wbDetectHint),
    busyLabel: ENV_TEXTS.wbChecking, disabledReason: snapshot.checking ? ENV_TEXTS.wbChecking : null };
  return {
    id: "workbuddy-health", testId: WB_ENV_TEST_IDS.healthCard, name: ENV_TEXTS.wbHealthCard, badge: localWordOf(health), icon: "database",
    currentAccountName: null, currentAccountMeta: "", // 旧页健康卡无账号块
    process: { state: labels.state, launchLabel: "", launchHint: "" },
    pathLabel: labels.deep ?? (snapshot.checking ? ENV_TEXTS.wbChecking : ENV_TEXTS.wbNotChecked),
    pathTitle: ENV_TEXTS.wbDeepLabel, actions: [detect],
  };
}
/** 多实例区（WB L199-205）：实验票未结 → pending 说明代替列表；创建入口沿用 TRAE 的「能力缺失即禁用并给同一句说明」写法。 */
function secondarySectionFor(caps: EnvironmentCapabilities): EnvSecondarySectionView | null {
  if (caps.multiInstance !== "pending") return null;
  const pending = ENV_TEXTS.wbMultiText;
  return { testId: WB_ENV_TEST_IDS.multiPending, title: ENV_TEXTS.wbMultiTitle, emptyTitle: null, emptyText: "", cards: [], pendingNote: pending,
    createAction: { ...act("env-create-entry", ENV_TEXTS.createEntry, "ghost", pending), disabledReason: pending } };
}
/** 快照 → 环境页视图模型（纯函数：同一快照恒得同一视图）。 */
export function buildWorkBuddyEnvironmentView(snapshot: WorkBuddyEnvironmentSnapshot): WorkBuddyEnvironmentView {
  const caps = envCapabilitiesFor("workbuddy"), health = snapshot.sessionHealth, labels = workbuddyLiveLabels(snapshot.deepHealth, snapshot.checking);
  return {
    productId: "workbuddy",
    header: { title: ENV_TEXTS.pageTitle, regionLabel: ENV_TEXTS.regionLabel, pageTitleAttr: "environment", badgeProductId: "workbuddy",
      refresh: act(WB_ENV_TEST_IDS.refresh, ENV_TEXTS.wbRefresh, "ghost", null) }, // WB L90：旧页刷新无 title
    loadingNote: snapshot.loading ? ENV_TEXTS.wbLoading : null,
    error: snapshot.error === null ? null : { title: ENV_TEXTS.wbLoadErrorTitle, description: snapshot.error, retryLabel: ENV_TEXTS.wbReload },
    master: health === null ? null : clientCardFor(snapshot, health, labels), masterHint: "", // 旧页无登录引导句
    checkup: null, capture: null, // 能力表 A 档：WB 无体检、无环境内登录
    secondary: secondarySectionFor(caps),
    stats: null, backups: [], plugins: [], // 统计归详情页 / 备份归设置页 / WB 无插件体系
    texts: { fallbackFailure: ENV_TEXTS.actionFailure, loadFailure: ENV_TEXTS.wbLoadFailure, refreshFailedSuffix: ENV_TEXTS.actionFailure },
    workbuddy: { healthCard: health === null ? null : healthCardFor(snapshot, health, labels),
      clientRuntimeTestId: WB_ENV_TEST_IDS.clientRuntime, deepResultTestId: WB_ENV_TEST_IDS.deepResult },
  };
}
