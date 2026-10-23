/**
 * TRAE 环境页 builder（P4d-3）：快照 → EnvironmentView 的纯映射（零 React / 零 JSX / 零命令 / 零 invoke）。
 * 对照 EnvironmentPage.tsx（简写 L）：主库卡 L434-697、体检 L471-621、捕获 L625-646、辅助环境 L700-759、副环境卡 L963-1179；deriveCheckupView（L1576-1591）与 incorporateStageLabels（L1598-1605）原样搬入（禁止自算滞留/空项目/孤儿），能力只读 envCapabilitiesFor("trae_cn")。
 */
import type { EnvironmentListItemDto, EnvironmentStateDto } from "../../types/environment";
import type { MasterCheckupAccountDto, MasterCheckupDto, MasterSelfCheckDto } from "../../types/masterLibrary";
import { ENV_TEXTS, envCapabilitiesFor, type EnvActionView, type EnvCardView, type EnvCheckupAccountRowView, type EnvCheckupItemView,
  type EnvCheckupView, type EnvProcessState, type EnvProcessView, type EnvSecondarySectionView, type EnvTone, type EnvironmentCapabilities,
  type EnvironmentView } from "./config";
/** 忙碌态：builder 只用来算 disabled 与执行中文案；动作编排（invoke）留在页面。 */
export interface TraeEnvironmentBusy { readonly launching: boolean; readonly launchingEnvId: string | null; readonly closingEnvId: string | null;
  readonly repairing: boolean; readonly capturing: boolean; }
/** 聚合页面需要的所有已取回数据：builder 只消费，不发起任何读取；null 一律按「未读到」降级。 */
export interface TraeEnvironmentSnapshot { readonly state: EnvironmentStateDto | null; readonly checkup: MasterCheckupDto | null;
  readonly selfCheck: MasterSelfCheckDto | null; readonly environments: readonly EnvironmentListItemDto[] | null;
  readonly secondaryCheckups: Readonly<Record<string, MasterCheckupDto | null>>; readonly busy: TraeEnvironmentBusy;
  readonly canOpenLibraryDetail: boolean; // 副环境「查看详情」入口可用（L739-743）
  readonly captureNotice: { readonly kind: "ok" | "error"; readonly text: string } | null; }
/** 文案占位符填充：缺键保留原样，不吞占位符。 */
const fill = (template: string, values: Readonly<Record<string, string | number>>): string => template.replace(/\{(\w+)\}/g, (match, key: string) => (key in values ? String(values[key]) : match));
/** 动作构造：旧实现 data-testid 与动作 id 逐字同值，故 testId = id；hint 即旧 title；disabledReason 非空即禁用。 */
const act = (id: string, label: string, kind: EnvActionView["kind"], hint: string | null = null): EnvActionView => ({ id, label, kind, testId: id, hint, disabledReason: null, busyLabel: null });
const busyAct = (base: EnvActionView, busyLabel: string, disabledReason: string | null): EnvActionView => ({ ...base, busyLabel, disabledReason });
/** 进程三态（L49-54 原样）：process_state 缺失回退 running 兼容字段，都缺失即 unknown。 */
const processStateOf = (env: { readonly process_state?: EnvProcessState; readonly running: boolean } | null): EnvProcessState => env === null ? "unknown" : env.process_state ?? (env.running ? "running" : "not_running");
/** 自检词表与色档（L920-955）：状态词逐字，色档只表达等级；库内容降级说明（L905-918）read_failed 与 default 同文。 */
const LEVELS: Record<MasterSelfCheckDto["level"], string> = { healthy: ENV_TEXTS.levelHealthy, self_healable: ENV_TEXTS.levelSelfHealable, needs_manual: ENV_TEXTS.levelNeedsManual };
const KEYS: Record<MasterSelfCheckDto["checks"][number]["key"], string> = { read: ENV_TEXTS.checkRead, consistency: ENV_TEXTS.checkConsistency, switchability: ENV_TEXTS.checkSwitchability, deep: ENV_TEXTS.checkDeep };
const STATUS: Record<MasterSelfCheckDto["checks"][number]["status"], readonly [string, EnvTone]> = { passed: [ENV_TEXTS.statusPassed, "ok"], attention: [ENV_TEXTS.statusAttention, "warn"], failed: [ENV_TEXTS.statusFailed, "danger"], blocked: [ENV_TEXTS.statusBlocked, "neutral"] };
const CONTENT_NOTES: Record<MasterCheckupDto["status"], string> = { ready: "", no_current_account: ENV_TEXTS.checkupNoCurrentAccount, no_master_data: ENV_TEXTS.checkupNoMasterData, read_failed: ENV_TEXTS.checkupUnreadable, not_applicable: ENV_TEXTS.checkupNotApplicable };
const contentNote = (status: MasterCheckupDto["status"] | undefined): string => (status === undefined ? "" : CONTENT_NOTES[status]) || ENV_TEXTS.checkupUnreadable;
/** 体检报告派生（L1576-1591 原样搬入）：滞留/空项目/收编规模，本文件不另算任何计数。 */
export function deriveCheckupView(report: MasterCheckupDto | null) {
  const ready = report?.status === "ready", accounts = ready ? report.accounts : [];
  // 滞留账号（非当前且存在正常会话的分布行）——收编目标；空项目只作诊断展示，不进入收编规模（归档-only 不算空项目）。
  const staleAccounts = accounts.filter((account) => !account.current && account.session_count > 0);
  const emptyShellAccounts = accounts.filter((account) => !account.current && account.empty_project_count > 0);
  const staleSessionTotal = staleAccounts.reduce((sum, a) => sum + a.session_count, 0);
  const staleProjectTotal = staleAccounts.reduce((sum, a) => sum + a.project_count, 0);
  // 清理与提示行都按全库口径统计空项目（含当前账号名下的残留行）。
  const emptyProjectTotal = accounts.reduce((sum, a) => sum + a.empty_project_count, 0);
  return { staleAccounts, emptyShellAccounts, staleSessionTotal, staleProjectTotal, emptyProjectTotal, hasNotice: staleAccounts.length > 0 || emptyProjectTotal > 0 };
}
/** 收编四阶段文案（L1598-1605 原样搬入；backing_up 为已登记的前后端不一致，见 ENV_KNOWN_COPY_MISMATCHES）。 */
export function incorporateStageLabels(isMaster: boolean, envName: string): readonly { readonly stage: string; readonly label: string }[] {
  return [
    { stage: "closing", label: isMaster ? ENV_TEXTS.incorporateStageClosingMaster : fill(ENV_TEXTS.incorporateStageClosingEnv, { name: envName }) },
    { stage: "backing_up", label: ENV_TEXTS.incorporateStageBackingUp },
    { stage: "incorporating", label: ENV_TEXTS.incorporateStageIncorporating },
    { stage: "restarting", label: isMaster ? ENV_TEXTS.incorporateStageRestartingMaster : fill(ENV_TEXTS.incorporateStageRestartingEnv, { name: envName }) },
  ];
}
/** 运行态视图（L686-693 / L1166-1173）：三态文案逐字，unknown 时启动动作禁用并给同一句说明。 */
function processViewOf(state: EnvProcessState, launching: boolean, isMaster: boolean): EnvProcessView {
  const pick = (whenRunning: string, whenIdle: string) => (state === "running" ? whenRunning : whenIdle);
  const launchLabel = launching ? ENV_TEXTS.launchBusy : state === "unknown" ? ENV_TEXTS.launchUnknown
    : isMaster ? pick(ENV_TEXTS.launchMasterRunning, ENV_TEXTS.launchMasterIdle) : pick(ENV_TEXTS.launchFocusShort, ENV_TEXTS.launchIdleShort);
  const launchHint = state === "unknown" ? (isMaster ? ENV_TEXTS.launchHintUnknown : ENV_TEXTS.launchEnvHintUnknown)
    : isMaster ? pick(ENV_TEXTS.launchHintRunning, ENV_TEXTS.launchHintIdle) : pick(ENV_TEXTS.launchEnvHintRunning, ENV_TEXTS.launchEnvHintIdle);
  return { state, launchLabel, launchHint };
}
/** 体积格式化（L1533-1543 原样）：KB/MB/GB/TB，10 以上取整。 */
function formatBytes(value: number): string {
  if (!Number.isFinite(value) || value <= 0) return "0 MB";
  const units = ["KB", "MB", "GB", "TB"];
  let size = value / 1024, unitIndex = 0;
  while (size >= 1024 && unitIndex < units.length - 1) { size /= 1024; unitIndex += 1; }
  return `${size >= 10 || unitIndex === 0 ? Math.round(size) : size.toFixed(1)} ${units[unitIndex]}`;
}
/** 诊断账号行（L550-557 / L563-570）：账号名是主信息，user_id 只进 key 与悬浮提示。 */
const staleRow = (account: MasterCheckupAccountDto): EnvCheckupAccountRowView => ({
  id: account.user_id, name: account.account_name ?? ENV_TEXTS.accountUnregistered,
  meta: fill(ENV_TEXTS.accountRowMeta, { sessions: account.session_count, projects: account.project_count }),
});
const emptyShellRow = (account: MasterCheckupAccountDto): EnvCheckupAccountRowView => ({
  id: `empty-${account.user_id}`, name: account.account_name ?? ENV_TEXTS.accountUnregistered,
  meta: fill(ENV_TEXTS.emptyShellMeta, { projects: account.empty_project_count }),
});
/** 规模句（L539-546 / L1051-1062）：滞留句按库口径传模板，空项目句两库同文。 */
const noticeLinesOf = (view: ReturnType<typeof deriveCheckupView>, staleTemplate: string): readonly string[] => [
  ...(view.staleAccounts.length > 0 ? [fill(staleTemplate, { accounts: view.staleAccounts.length, sessions: view.staleSessionTotal })] : []),
  ...(view.emptyProjectTotal > 0 ? [fill(ENV_TEXTS.emptyProjectLine, { projects: view.emptyProjectTotal })] : []),
];
/** 主库卡（L434-697）：默认环境置顶；详情常驻，关闭仅运行中，启动按三态（unknown 禁用）。 */
function masterCardFor(snapshot: TraeEnvironmentSnapshot, process: EnvProcessView, caps: EnvironmentCapabilities): EnvCardView {
  const actions: EnvActionView[] = [act("env-master-detail", ENV_TEXTS.masterDetail, "ghost", ENV_TEXTS.masterDetailHint)];
  if (caps.instanceControl) {
    if (process.state === "running") actions.push(busyAct(act("env-master-close", ENV_TEXTS.close, "ghost", ENV_TEXTS.closeHint), ENV_TEXTS.close, snapshot.busy.closingEnvId !== null ? ENV_TEXTS.closeBusy : null));
    actions.push(busyAct(act("env-master-launch", process.launchLabel, "primary", process.launchHint), ENV_TEXTS.launchBusy, snapshot.busy.launching || process.state === "unknown" ? process.launchHint : null));
  }
  const dataDir = snapshot.state?.data_dir ?? null;
  return { id: "master", testId: "env-master-card", name: ENV_TEXTS.masterName, badge: ENV_TEXTS.masterBadge, icon: "database", process, actions,
    currentAccountName: snapshot.state?.current_account_name ?? null, currentAccountMeta: ENV_TEXTS.currentMeta,
    pathLabel: dataDir ? ENV_TEXTS.dataDirLabel : "—", // L655：无目录渲染「—」
    pathTitle: dataDir ? fill(ENV_TEXTS.dataDirTitle, { path: dataDir }) : "" };
}
/** 库内容分组（L533-620）：ready 才给规模句与行；不可读给降级句；动作按能力裁剪。 */
function contentGroupFor(snapshot: TraeEnvironmentSnapshot, view: ReturnType<typeof deriveCheckupView>, caps: EnvironmentCapabilities): EnvCheckupView["content"] {
  const ready = snapshot.checkup?.status === "ready", orphans = ready ? snapshot.checkup?.orphan_project_count ?? 0 : 0;
  const actions: EnvActionView[] = [];
  if (ready && view.hasNotice) {
    if (caps.cleanup && view.emptyProjectTotal > 0) actions.push(act("env-cleanup-empty-projects-entry", ENV_TEXTS.cleanupEntry, "ghost"));
    if (caps.incorporate === "single-transaction" && view.staleAccounts.length > 0) actions.push(act("env-incorporate-entry", ENV_TEXTS.incorporateEntry, "primary"));
  }
  return { title: ENV_TEXTS.contentTitle, actions, staleAccounts: view.staleAccounts.map(staleRow), emptyShells: view.emptyShellAccounts.map(emptyShellRow),
    noticeLines: ready ? noticeLinesOf(view, ENV_TEXTS.staleLine) : [contentNote(snapshot.checkup?.status)],
    orphanNote: orphans > 0 ? fill(ENV_TEXTS.orphanLine, { orphans }) : null,
    okNote: ready && !view.hasNotice ? ENV_TEXTS.checkupOk : null }; // L611 正常态
}
/** 体检锚点（L472/479/533 主库、L1032/1039/1049 副环境）：envId=null 为主库三件，否则按 env_id 拼接。 */
export function traeCheckupTestIds(envId: string | null): { readonly block: string; readonly login: string; readonly content: string } {
  return envId === null
    ? { block: "env-checkup", login: "env-checkup-login", content: "env-checkup-content" }
    : { block: `env-checkup-${envId}`, login: `env-checkup-login-${envId}`, content: `env-checkup-content-${envId}` };
}
/** 主库体检区块（L471-621）：两组各自降级；都不可读即整块静默（返回 null）。 */
function masterCheckupFor(snapshot: TraeEnvironmentSnapshot, caps: EnvironmentCapabilities): EnvCheckupView | null {
  const { selfCheck, checkup } = snapshot;
  if (selfCheck === null && checkup === null) return null; // L471：两组都不可读不渲染
  const readCheck = selfCheck?.checks.find((check) => check.key === "read");
  const loginCorrupted = readCheck?.status === "failed" && readCheck.summary.includes("损坏"); // L284
  const notes: string[] = [];
  if (selfCheck !== null && selfCheck.observed_account_name !== null && selfCheck.current_account_name !== selfCheck.observed_account_name) {
    notes.push(fill(ENV_TEXTS.observedMismatch, { observed: selfCheck.observed_account_name, recorded: selfCheck.current_account_name ?? ENV_TEXTS.notAvailable }));
  }
  if (loginCorrupted) notes.push(ENV_TEXTS.loginCorrupted);
  const items: EnvCheckupItemView[] = selfCheck === null ? [] : selfCheck.checks.map((check) => ({
    id: check.key, name: KEYS[check.key], status: STATUS[check.status][0], tone: STATUS[check.status][1], summary: check.summary }));
  const repair: EnvActionView | null = selfCheck?.can_repair === true && caps.loginHealth === "self-check"
    ? busyAct(act("env-self-check-repair", ENV_TEXTS.repair, "primary"), ENV_TEXTS.repairBusy, snapshot.busy.repairing ? ENV_TEXTS.repairBusy : null) : null;
  return { testId: traeCheckupTestIds(null).block, title: ENV_TEXTS.checkupTitle, content: contentGroupFor(snapshot, deriveCheckupView(checkup), caps),
    login: { title: selfCheck === null ? ENV_TEXTS.loginHealthTitle : `${ENV_TEXTS.loginHealthTitle}：${LEVELS[selfCheck.level]}`,
      notes, items, repair, unreadableNote: selfCheck === null ? ENV_TEXTS.loginHealthUnreadable : null } };
}
/** 账号捕获（L625-646）：TRAE 常驻；WB 在账号页（能力表判定）。 */
function captureFor(snapshot: TraeEnvironmentSnapshot, caps: EnvironmentCapabilities): EnvironmentView["capture"] {
  if (caps.accountCapture !== "environment-page") return null;
  const notice = snapshot.captureNotice, capturing = snapshot.busy.capturing;
  return { testId: "env-capture-account-group", title: ENV_TEXTS.captureTitle,
    entry: busyAct(act("env-capture-account-entry", capturing ? ENV_TEXTS.captureBusy : ENV_TEXTS.captureEntry, "ghost"), ENV_TEXTS.captureBusy, capturing ? ENV_TEXTS.captureBusy : null),
    notice: notice === null ? null : { tone: notice.kind === "ok" ? "ok" : "danger", text: notice.text } };
}
/** 副环境体检区块（L1032-1093）：有差异才返回；testid 按 env_id 拼接；口径与主库同源（deriveCheckupView）。 */
export function traeSecondaryCheckupView(env: EnvironmentListItemDto, report: MasterCheckupDto | null): EnvCheckupView | null {
  const view = deriveCheckupView(report), orphans = report?.status === "ready" ? report.orphan_project_count : 0;
  if (report?.status !== "ready" || !(view.hasNotice || orphans > 0)) return null; // L996-998：无差异静默
  return { testId: traeCheckupTestIds(env.env_id).block, title: fill(ENV_TEXTS.secondaryCheckupTitle, { name: env.name }),
    // 副环境无实测语义：只显示注册表账号与待登录态（L1040-1045）。
    login: { title: ENV_TEXTS.loginHealthTitle, items: [], repair: null, unreadableNote: null,
      notes: [env.current_account_name === null ? ENV_TEXTS.secondaryLoginMissing : fill(ENV_TEXTS.secondaryLoginNote, { name: env.current_account_name })] },
    content: { title: ENV_TEXTS.contentTitle, noticeLines: noticeLinesOf(view, ENV_TEXTS.secondaryStaleLine),
      staleAccounts: view.staleAccounts.map(staleRow), emptyShells: [], // 副环境只有规模句，不渲染空壳清单（L1058-1062）
      orphanNote: orphans > 0 ? fill(ENV_TEXTS.orphanLine, { orphans }) : null, okNote: null,
      actions: view.staleAccounts.length > 0 ? [act(`env-incorporate-${env.env_id}`, ENV_TEXTS.secondaryIncorporate, "primary")] : [] } };
}
/** 副环境卡（L963-1179）：名称/体积/运行态 + 详情·登录·改名·删除·关闭·启动。 */
function secondaryCardFor(env: EnvironmentListItemDto, snapshot: TraeEnvironmentSnapshot, caps: EnvironmentCapabilities): EnvCardView {
  const process = processViewOf(processStateOf(env), snapshot.busy.launchingEnvId === env.env_id, false);
  const unknown = process.state === "unknown", id = env.env_id, actions: EnvActionView[] = [];
  if (snapshot.canOpenLibraryDetail) actions.push(act(`env-detail-${id}`, ENV_TEXTS.secondaryDetail, "ghost", fill(ENV_TEXTS.secondaryDetailHint, { name: env.name })));
  if (caps.login === "environment-login") actions.push({ ...act(`env-login-${id}`, ENV_TEXTS.loginEntry, "ghost", unknown ? ENV_TEXTS.loginDisabledHint : ENV_TEXTS.loginHint), disabledReason: unknown ? ENV_TEXTS.loginDisabledHint : null });
  if (caps.environmentLifecycle) {
    actions.push(act(`env-rename-${id}`, ENV_TEXTS.renameEntry, "ghost", ENV_TEXTS.renameHint));
    actions.push({ ...act(`env-delete-${id}`, ENV_TEXTS.deleteEntry, "danger", unknown ? ENV_TEXTS.deleteDisabledHint : ENV_TEXTS.deleteHint), disabledReason: unknown ? ENV_TEXTS.deleteDisabledHint : null });
  }
  if (caps.instanceControl) {
    if (process.state === "running") actions.push(busyAct(act(`env-close-${id}`, ENV_TEXTS.close, "ghost", ENV_TEXTS.secondaryCloseHint), ENV_TEXTS.closeBusy, snapshot.busy.closingEnvId === id ? ENV_TEXTS.closeBusy : null));
    actions.push(busyAct(act(`env-launch-${id}`, process.launchLabel, "primary", process.launchHint), ENV_TEXTS.launchBusy, snapshot.busy.launchingEnvId === id || unknown ? process.launchHint : null));
  }
  // 归入入口只在体检区块内容组出现（L1082，由 traeSecondaryCheckupView 提供），卡片动作不重复渲染。
  return { id, testId: `env-card-${id}`, name: env.name, badge: ENV_TEXTS.secondaryBadge, icon: "layers", process, actions,
    currentAccountName: env.current_account_name, currentAccountMeta: ENV_TEXTS.currentMeta,
    pathLabel: env.size_bytes === undefined ? ENV_TEXTS.dataDirLabel : `${formatBytes(env.size_bytes)}${ENV_TEXTS.volumeSuffix}`, // L1103
    pathTitle: fill(ENV_TEXTS.secondaryPathTitle, { path: env.data_dir }) };
}
/** 辅助环境区（L700-759）：列表 null 时不渲染空态；pending 档不渲染卡列表（WB 待实验票）。 */
function secondarySectionFor(snapshot: TraeEnvironmentSnapshot, caps: EnvironmentCapabilities): EnvSecondarySectionView | null {
  if (caps.multiInstance !== "full-lifecycle") return null;
  const list = snapshot.environments;
  const cards = list === null ? [] : list.filter((env) => !env.is_master).map((env) => secondaryCardFor(env, snapshot, caps));
  return { testId: "env-secondary-section", title: ENV_TEXTS.secondaryTitle, emptyText: ENV_TEXTS.placeholderText, cards, pendingNote: null,
    createAction: busyAct(act("env-create-entry", ENV_TEXTS.createEntry, "ghost", ENV_TEXTS.createHint), ENV_TEXTS.nameBusy, caps.environmentLifecycle ? null : ENV_TEXTS.createHint),
    emptyTitle: list !== null && cards.length === 0 ? ENV_TEXTS.placeholderTitle : null }; // L714：空态只在列表已读且为空
}
/** 快照 → 环境页视图模型（纯函数：同一快照恒得同一视图）。 */
export function buildTraeEnvironmentView(snapshot: TraeEnvironmentSnapshot): EnvironmentView {
  const caps = envCapabilitiesFor("trae_cn");
  const process = processViewOf(processStateOf(snapshot.state), snapshot.busy.launching, true);
  return {
    productId: "trae_cn", loadingNote: null, error: null, // 旧页无首读占位；页级失败走全局通知（L170/L207）
    header: { title: ENV_TEXTS.pageTitle, regionLabel: ENV_TEXTS.regionLabel, pageTitleAttr: "environment", badgeProductId: null,
      refresh: act("environment-refresh", ENV_TEXTS.refresh, "ghost", ENV_TEXTS.refreshHint) },
    master: masterCardFor(snapshot, process, caps),
    masterHint: snapshot.state?.current_profile_id ? ENV_TEXTS.masterHintLoggedIn : ENV_TEXTS.masterHintLoggedOut, // L462-464
    checkup: masterCheckupFor(snapshot, caps), capture: captureFor(snapshot, caps), secondary: secondarySectionFor(snapshot, caps),
    stats: null, // 主库统计归详情页（G5 断言锁定）
    backups: [], plugins: [], // 备份链归设置页 / 插件归库详情 tab
    texts: { fallbackFailure: ENV_TEXTS.actionFailure, loadFailure: ENV_TEXTS.loadFailure, refreshFailedSuffix: ENV_TEXTS.refreshFailedSuffix },
  };
}
