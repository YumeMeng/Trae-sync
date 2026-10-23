/**
 * 环境页视图模型 / 能力表 / 文案表（P4d-2）：纯类型与声明式配置，零 UI、零 JSX、零 invoke、零 import。
 * builder（traeEnvironment / workbuddyEnvironment，P4d-3/4）产出 EnvironmentView；页面只读本文件、不判断 productId。
 * 文案逐字沿用旧页面（tests/EnvironmentPage 38 例 + 61 个 env-* 锚点锁定）。行号缩写：L = EnvironmentPage.tsx，
 * WB = WorkBuddyEnvironmentPage.tsx；每组文案的行号按声明顺序列在组注释里，占位符统一 {name}，由 builder 填充。
 */

// ===== 视图模型（全字段只读） =====

/** 色档：neutral=安静、ok=正常、warn=需处理、danger=失效、accent=当前/强调；进程三态 unknown=查询失败不猜。 */
export type EnvTone = "neutral" | "ok" | "warn" | "danger" | "accent";
export type EnvActionKind = "primary" | "ghost" | "danger";
export type EnvProcessState = "running" | "not_running" | "unknown";
/** 动作（页头/卡片/弹层共用）：hint 即旧 title；disabledReason 非空即禁用并给悬浮说明；busyLabel=执行中文案。 */
export interface EnvActionView {
  readonly id: string; readonly label: string; readonly kind: EnvActionKind; readonly testId: string;
  readonly hint: string | null; readonly disabledReason: string | null; readonly busyLabel: string | null;
}
/** 运行态：仅 running 才渲染关闭按钮；unknown 时启停禁用（旧 L684/L1121）；launchLabel=聚焦/启动/状态未知。 */
export interface EnvProcessView { readonly state: EnvProcessState; readonly launchLabel: string; readonly launchHint: string }
/** 环境卡（主库卡与副环境卡同构；副环境多登录/改名/删除/详情/关闭入口）。 */
export interface EnvCardView {
  readonly id: string; readonly testId: string; // "master" / env_id；env-master-card / env-card-<env_id>
  readonly name: string; readonly badge: string; readonly icon: "database" | "layers"; // 默认环境 / 辅助环境
  readonly currentAccountName: string | null; readonly currentAccountMeta: string; // null = 不渲染当前账号块
  readonly process: EnvProcessView; readonly pathLabel: string; readonly pathTitle: string; // 路径只进悬浮提示
  readonly actions: readonly EnvActionView[];
}
/** 自检单项（读校验/一致性/可切换/深度）：名称、状态词、摘要均由 builder 逐字给出。 */
export interface EnvCheckupItemView {
  readonly id: string; readonly name: string; readonly status: string; readonly tone: EnvTone; readonly summary: string;
}
/** 体检账号行（滞留/空项目诊断）：账号名是主信息，user_id 只进 key 与悬浮提示。 */
export interface EnvCheckupAccountRowView { readonly id: string; readonly name: string; readonly meta: string }
/** 体检区块（登录健康 + 库内容两组；各自降级互不阻塞，都不可读即不渲染）。 */
export interface EnvCheckupView {
  readonly testId: string; readonly title: string; // env-checkup / env-checkup-<env_id>；主库体检 / <名称>体检
  readonly login: {
    readonly title: string; readonly notes: readonly string[]; // 「登录健康：可自愈」；实际登录差异 / 损坏引导
    readonly items: readonly EnvCheckupItemView[]; readonly unreadableNote: string | null; // 读取失败降级
    readonly repair: EnvActionView | null; // can_repair 才给纠正入口
  };
  readonly content: {
    readonly title: string; readonly noticeLines: readonly string[]; // 滞留/空项目规模句
    readonly staleAccounts: readonly EnvCheckupAccountRowView[]; readonly emptyShells: readonly EnvCheckupAccountRowView[];
    readonly orphanNote: string | null; readonly okNote: string | null; // 无归属只报告；无差异正常态
    readonly actions: readonly EnvActionView[]; // 归入 / 清理空项目
  };
}
/** 辅助环境区：创建入口 + 空态 + 卡列表；pendingNote 非 null 即代替列表（WB 多实例待实验票）。 */
export interface EnvSecondarySectionView {
  readonly testId: string; readonly title: string; readonly createAction: EnvActionView;
  readonly emptyTitle: string | null; readonly emptyText: string; // null = 不渲染空态（已有环境 / 无列表能力）
  readonly cards: readonly EnvCardView[]; readonly pendingNote: string | null;
}
/** 主库统计（详情页数据面；环境页主库卡不渲染统计，G5 断言锁定）。 */
export interface MasterStatsView {
  readonly projectCount: number; readonly sessionCount: number; readonly messageCount: number;
  readonly participatingAccountCount: number; readonly lastActiveLabel: string | null; readonly sizeLabel: string;
}
/** 一份备份（设置页备份链；环境页不渲染，本票只登记形状，P4f 使用）。 */
export interface BackupEntryView {
  readonly stampUnixSeconds: number; readonly stampLabel: string; readonly sizeLabel: string;
  readonly hasWal: boolean; readonly hasCredentials: boolean; readonly badge: string; // 「含凭据」等
}
/** 插件行（库详情 plugins tab；环境页不渲染，P4e 使用）。 */
export interface PluginRowView {
  readonly id: string; readonly name: string; readonly version: string | null;
  readonly accounts: readonly string[]; readonly tone: EnvTone; readonly action: EnvActionView | null; // 账号宇宙清单
}
/** 环境页视图模型：页面唯一渲染输入（WB 无 master/checkup/capture/secondary）。 */
export interface EnvironmentView {
  readonly productId: string;
  readonly header: { readonly title: string; readonly regionLabel: string; readonly pageTitleAttr: string;
    readonly badgeProductId: string | null; readonly refresh: EnvActionView | null };
  readonly loadingNote: string | null; // 首读中
  readonly error: { readonly title: string; readonly description: string; readonly retryLabel: string } | null;
  readonly master: EnvCardView | null; readonly masterHint: string; // 登录/未登录两态引导句
  readonly checkup: EnvCheckupView | null;
  readonly capture: { readonly testId: string; readonly title: string; readonly entry: EnvActionView;
    readonly notice: { readonly tone: "ok" | "danger"; readonly text: string } | null } | null;
  readonly secondary: EnvSecondarySectionView | null;
  readonly stats: MasterStatsView | null; // 环境页恒 null（统计归详情页）
  readonly backups: readonly BackupEntryView[]; // 环境页恒空（备份归设置页）
  readonly plugins: readonly PluginRowView[]; // 环境页恒空（插件归库页）
  readonly texts: { readonly fallbackFailure: string; readonly loadFailure: string; readonly refreshFailedSuffix: string };
}

// ===== 能力表（照 P4d-design §2 的 A/B/C 结论；A=缺失、B=表现不同、C=一致） =====

/** 环境页能力开关：页面与 builder 只读本表，禁止再写 productId 分支。 */
export interface EnvironmentCapabilities {
  readonly instanceControl: boolean; // A 启停（启动/聚焦/关闭）：仅 TRAE（WB 只读状态卡，P19 决策 10）
  readonly liveStats: "five-second-poll" | "manual-only"; // B 运行态轮询：TRAE 5 秒 / WB 仅手动刷新
  readonly multiInstance: "full-lifecycle" | "pending"; // A 多实例列表：TRAE 全生命周期 / WB 待实验票
  readonly environmentLifecycle: boolean; // A 创建/改名/删除（含删除规模预览）：仅 TRAE
  readonly login: "environment-login" | "accounts-page-only"; // A 环境内登录账号：仅 TRAE
  readonly loginHealth: "self-check" | "deep-health"; // B 登录健康口径：TRAE 自检四项 / WB deep health
  readonly contentCheckup: "stale-empty-orphan" | "library-five-item"; // B 库内容体检：口径不同不可共用
  readonly incorporate: "single-transaction" | "cwd-merge"; // B 收编：TRAE 单事务 / WB cwd 归并
  readonly cleanup: boolean; // A 空项目清理（全库口径）：仅 TRAE
  readonly accountCapture: "environment-page" | "accounts-page"; // B 账号捕获入口位置
  readonly secondaryCheckup: boolean; // A 副环境逐库体检区块：仅 TRAE
  readonly plugins: "library-tab" | "absent"; // A 插件管理：TRAE 库详情 tab / WB 无插件体系
  readonly backup: "settings-page"; // C 备份链均在设置页（滚动单份），本票不碰、不搬
  readonly relaunchOutcome: "receipt-reported" | "absent"; // A 收编/清理回执的重启结果：仅 TRAE
  readonly pageRegion: "environment"; // C 页头/region 契约（「环境」+ data-page-title="environment"）
  readonly capabilityState: "collapsed-ready"; // C 三态塌缩后恒 ready（sectionStateOf 语义不复活）
}
export const ENV_CAPABILITIES: Record<string, EnvironmentCapabilities> = {
  trae_cn: {
    instanceControl: true, liveStats: "five-second-poll", multiInstance: "full-lifecycle", environmentLifecycle: true,
    login: "environment-login", loginHealth: "self-check", contentCheckup: "stale-empty-orphan", cleanup: true,
    incorporate: "single-transaction", accountCapture: "environment-page", secondaryCheckup: true, plugins: "library-tab",
    backup: "settings-page", relaunchOutcome: "receipt-reported", pageRegion: "environment", capabilityState: "collapsed-ready",
  },
  workbuddy: {
    instanceControl: false, liveStats: "manual-only", multiInstance: "pending", environmentLifecycle: false,
    login: "accounts-page-only", loginHealth: "deep-health", contentCheckup: "library-five-item", cleanup: false,
    incorporate: "cwd-merge", accountCapture: "accounts-page", secondaryCheckup: false, plugins: "absent",
    backup: "settings-page", relaunchOutcome: "absent", pageRegion: "environment", capabilityState: "collapsed-ready",
  },
};
/** 未登记产品回退最保守一档（无启停/无多实例/无清理），不猜能力。 */
export const DEFAULT_ENV_CAPABILITIES: EnvironmentCapabilities = ENV_CAPABILITIES.workbuddy;
/** 按产品取能力表（页面与 builder 的唯一产品差异入口）。 */
export function envCapabilitiesFor(productId: string): EnvironmentCapabilities {
  return ENV_CAPABILITIES[productId] ?? DEFAULT_ENV_CAPABILITIES;
}
// ===== 文案表（逐字保留旧页面口径；行号见每组注释，L=EnvironmentPage.tsx，WB=WorkBuddyEnvironmentPage.tsx） =====

export const ENV_TEXTS = {
  // 页头/全局失败：L417/414/427/425/170/207
  pageTitle: "环境", regionLabel: "环境", refresh: "刷新", refreshHint: "重新读取环境状态与体积", loadFailure: "环境状态暂时不可读取，请稍后重试。", actionFailure: "环境操作未能完成，请稍后重试。",
  // 主库卡：L439/440/455/655/652/665/663/677/675/693/687/689/690/463/464
  masterName: "主库", masterBadge: "默认环境", currentMeta: "当前登录", dataDirLabel: "数据目录", dataDirTitle: "主库数据目录：{path}", masterDetail: "详情", close: "关闭", masterDetailHint: "查看主库详情（对话记录、统计与备份）", closeHint: "关闭主库 TRAE 实例",
  launchBusy: "处理中…", launchUnknown: "状态未知", launchMasterRunning: "聚焦主库", launchMasterIdle: "启动主库", launchHintUnknown: "无法确认主库 TRAE 是否正在运行，实例操作暂不可用", launchHintRunning: "把主库 TRAE 窗口带到前台", launchHintIdle: "启动主库 TRAE 实例",
  masterHintLoggedIn: "全部对话记录都在主库内共享；在「账号」页切换账号，全部记录都会保留。", masterHintLoggedOut: "主库尚未登录账号：首次启动后请在 TRAE 窗口内登录一次，此后切换账号全自动完成。",
  // 主库体检：L475/481/534/528/493/488-489/508/511/513/523/158/160/541/545/588/612/912/552/555/568/583/607
  checkupTitle: "主库体检", loginHealthTitle: "登录健康", contentTitle: "库内容", loginHealthUnreadable: "登录健康暂时无法读取。", loginCorrupted: "登录数据已损坏，需重新登录。", observedMismatch: "实际登录：{observed}；工具记录：{recorded}",
  notAvailable: "暂无", // L489（工具记录未登记时的占位词）
  deepSummary: "查看深度检查说明", deepEmpty: "暂无深度检查结果。", deepHint: "需要核对具体记录时，请在主库详情的库信息中展开备份对比。", repair: "纠正为实际登录账号", repairBusy: "纠正中…", repairSuccess: "已按实际登录账号纠正主库记录。", repairFailure: "主库记录未能纠正，请稍后重试。",
  staleLine: "有 {accounts} 个账号的 {sessions} 个会话在主库中，未随当前账号展示", emptyProjectLine: "另有 {projects} 个空项目记录未进入归入范围", orphanLine: "另有 {orphans} 条无归属记录不会被归入。", checkupOk: "暂未发现滞留记录，账号记录分布正常。", checkupUnreadable: "库内容暂时无法读取。",
  accountUnregistered: "未登记账号", accountRowMeta: "{sessions} 个会话 · {projects} 个项目", emptyShellMeta: "{projects} 个空项目", cleanupEntry: "清理空项目", incorporateEntry: "一键归入当前账号",
  // 自检词表与体检降级（L920-955 等级/状态/检查项；L905-918 状态说明；L550/L563/L1680 账号标识悬浮）
  levelHealthy: "健康", levelSelfHealable: "可自愈", levelNeedsManual: "需人工处理",
  checkRead: "读校验", checkConsistency: "一致性", checkSwitchability: "可切换", checkDeep: "深度检查",
  statusPassed: "正常", statusAttention: "需关注", statusFailed: "需人工", statusBlocked: "暂不可用",
  checkupNoCurrentAccount: "尚未确认主库当前登录账号，无法统计库内容。",
  checkupNoMasterData: "主库尚未产生对话数据。", checkupNotApplicable: "该产品无此能力：只读浏览的库不适用体检。",
  accountIdTitle: "账号标识：{id}",
  // 账号捕获：L626/635/351/363/365/367/368
  captureTitle: "账号捕获", captureEntry: "从客户端捕获账号", captureBusy: "正在捕获…", captureSuccess: "已捕获账号「{name}」入池（{profileId}），切号/签到/历史能力即刻可用。", captureAlreadyRegistered: "该账号已在账号池中，无需重复捕获。",
  captureBlobMissing: "未在 Trae 客户端检测到新的登录态：请先在客户端登录要捕获的账号，再点击捕获。", captureReviewFailed: "在线复核身份未通过，未做任何改动；请稍后重试。", captureFailure: "捕获未完成：{reason}（数据保持操作前状态）。",
  // 辅助环境：L702/710/708/718/719/1007/1035/1089/1043/1044/1054/1100/1103/1114/1112
  secondaryTitle: "辅助环境", createEntry: "新建环境", createHint: "创建一个新的辅助环境（独立数据目录）", placeholderTitle: "还没有辅助环境", secondaryBadge: "辅助环境", secondaryCheckupTitle: "{name}体检",
  placeholderText: "辅助环境是独立的数据目录，可与主库并行使用；各环境的对话记录互相独立。", secondaryIncorporate: "归入当前账号", secondaryLoginNote: "当前登录：{name}。", secondaryLoginMissing: "尚未登录账号，请先登录后再使用该环境。",
  secondaryStaleLine: "有 {accounts} 个账号的 {sessions} 个会话在此环境中，未随当前账号展示", secondaryPathTitle: "数据目录：{path}", volumeSuffix: " · 数据目录", secondaryDetail: "查看详情", secondaryDetailHint: "查看「{name}」详情（对话记录、统计与备份）",
  // 副环境动作：L1125/1123/1134/1132/1144/1142/1154/1157/1173/1167/1169/1170
  loginEntry: "登录账号", loginHint: "选择一个账号登录该环境", loginDisabledHint: "无法确认该环境是否正在运行，暂不能登录", renameEntry: "重命名", renameHint: "重命名该环境", deleteEntry: "删除", deleteHint: "删除该环境（删除前会列出数据规模）",
  deleteDisabledHint: "无法确认该环境是否正在运行，暂不能删除", secondaryCloseHint: "关闭该环境的 TRAE 实例", closeBusy: "关闭中…", launchFocusShort: "聚焦", launchIdleShort: "启动", launchEnvHintUnknown: "无法确认该环境是否正在运行，实例操作暂不可用",
  launchEnvHintRunning: "把该环境的 TRAE 窗口带到前台", launchEnvHintIdle: "启动该环境的 TRAE 实例",
  // 名称/删除/关闭弹层：L816/817/831/832/1236/1252/1261/824/840/1494-1496/1509/1519/1484-1489/1474/869/877-884/253-257
  // P4d-8a-2b 补两键（弹层兜底句，旧页内联字面量）：nameFailure=L1215、deletePreviewFailure=L1461
  createDialogTitle: "新建环境", createConfirm: "创建", renameDialogTitle: "重命名环境", renameConfirm: "保存", namePlaceholder: "环境名称（1-64 个字符）", cancel: "取消", nameBusy: "处理中…", createdNotice: "已创建环境「{name}」。",
  renamedNotice: "已重命名为「{name}」。", deleteDialogTitle: "删除环境「{name}」", deleteConfirm: "删除环境", deleteBusy: "删除中…", deleteIrreversible: "删除后无法恢复", deleteLoading: "正在读取环境数据规模…",
  deleteScaleNoData: "该环境从未启动，没有对话数据；将删除环境目录（约 {size}）。", deleteScaleReadFailed: "环境数据规模暂时无法读取；删除将移除整个环境目录（约 {size}），请确认其中没有需要保留的内容。", deleteScaleEmpty: "该环境没有对话记录；将删除环境目录（约 {size}）。",
  deleteScaleReady: "该环境内有 {projects} 个项目、{sessions} 个会话（约 {size}），删除后无法恢复。", deleteFailure: "环境删除未完成，请稍后重试。", deletePreviewFailure: "环境信息暂时不可读取，请稍后重试。", nameFailure: "操作未完成，请稍后重试。", deletedNotice: "已删除环境「{name}」。", closeDialogTitle: "关闭「{name}」",
  closeDialogLine1: "将关闭「{name}」正在运行的 TRAE 实例。", closeDialogLine2: "实例内未保存的内容可能丢失，请确认已保存所需工作。", closeDialogConfirm: "关闭实例", closeDialogBusy: "正在关闭…", closeOutcomeNotRunning: "「{name}」当前没有运行中的实例。",
  closeOutcomeForce: "「{name}」未能自动退出，已强制结束进程。", closeOutcomeClosed: "已关闭「{name}」的 TRAE 实例。",
  // 收编弹层：L1654/1640/1660/1670/1672-1673/1674/1690/1702/1755/1712/1713/1720/1726/325/326/1600/1601⚠①/1602/1603
  incorporateTitle: "归入当前账号", incorporateTargetFallback: "当前账号", incorporateTargetMeta: "{target} · 滞留记录全部归入 · 自动完成转移", incorporateConfirmHead: "将归入 {accounts} 个账号的记录",
  incorporateConfirmBody: "共 {sessions} 个会话、{projects} 个项目将归入{account}；{target}会自动关闭并重启，归入前会创建数据备份。", incorporateOrphanSuffix: "另有 {orphans} 条无归属记录不会被归入。", incorporateConfirm: "确认归入", incorporateClose: "关闭",
  incorporateDoneClose: "完成", incorporateDone: "已归入 {accounts} 个账号：保留项目 {projects} 个 · 转移会话 {sessions} 个", incorporateLedgerMissing: "（接力轨迹记录暂缺，不影响数据）", incorporateRefreshFailed: "归入已完成，但环境状态刷新失败，请手动刷新。",
  incorporateRelaunchFailed: "归入已完成，但环境未能自动启动；可从环境页手动启动。", incorporateFailureMaster: "归入未完成，主库数据保持操作前状态；请稍后重试。", incorporateFailureSecondary: "归入未完成，环境数据保持操作前状态；请稍后重试。", incorporateStageClosingMaster: "关闭主库",
  incorporateStageClosingEnv: "关闭「{name}」", incorporateStageBackingUp: "创建备份", incorporateStageIncorporating: "归入记录", incorporateStageRestartingMaster: "重启主库", incorporateStageRestartingEnv: "重启「{name}」",
  // 清理空项目：L796/802/804/798⚠②/799/800/395⚠②/394⚠②/399
  cleanupDialogTitle: "清理空项目", cleanupConfirm: "删除", cleanupBusy: "清理中…", cleanupDialogLine1: "将删除 {count} 个空项目记录，删除前会自动备份。", cleanupDialogLine2: "空项目记录不含任何对话，删除不影响对话历史。", cleanupDialogLine3: "无归属记录不会被清理。",
  cleanupSuccess: "已删除 {count} 个空项目记录（已自动备份）。", cleanupSuccessRelaunchFailed: "已删除 {count} 个空项目记录（已自动备份）；主库 TRAE 未能自动启动，可从环境页重新打开。", cleanupNothing: "没有可清理的空项目记录。", refreshFailedSuffix: "环境状态刷新失败，请手动刷新。",
  // 启动与清理失败（L221 主库启动回执、L234 副环境启动回执、L405 清理失败兜底）
  launchMasterSuccess: "主库已启动（或已聚焦到既有窗口）。", launchEnvSuccess: "「{name}」已启动（或已聚焦到既有窗口）。",
  cleanupFailure: "清理未完成，主库数据保持操作前状态；请稍后重试。",
  // 环境登录账号弹层：L1343/1340/1376/1379/1383/1350/1352/1353/1359/1369/1426/1326/855
  loginDialogTitle: "登录账号到「{name}」", loginDialogAria: "登录账号到环境", loginPrepare: "选择要登录到该环境的账号；环境内的对话记录会随登录归到所选账号名下。", loginLoading: "正在读取账号列表…", loginNoAccounts: "还没有已保存的账号；请先在「账号」页添加账号。",
  loginReceiptPrefix: "已登录账号 {name}", loginReceiptSeeded: "；环境首次登录完成，可直接启动使用。", loginReceiptSwitched: "；随行项目 {projects} 个 · 会话 {sessions} 个", loginNotRemembered: "账号已切换，但应用未能记住当前账号；请刷新环境页，问题持续时重启应用。", loginDone: "完成",
  loginConfirm: "登录", loginBusy: "处理中…", loginFailure: "环境账号切换未完成，请稍后重试。", loginRefreshFailed: "账号已切换，但环境状态刷新失败，请手动刷新。",
  // WorkBuddy 环境页：WB 94/100/105/109/42/118-120/128/130/131/133/136/135/138/141/142/145-151/158/159/164/168/175/177/178/192/189/67-72/201/202/54
  wbRefresh: "刷新", wbLoading: "正在读取客户端状态…", wbLoadFailure: "WorkBuddy 客户端状态暂时不可读取，请稍后重试。", wbLoadErrorTitle: "无法读取 WorkBuddy 客户端状态", wbReload: "重新读取", wbClientCard: "客户端连接", wbLoginFile: "登录文件状态", wbRuntime: "客户端运行状态",
  wbRuntimeVerified: "运行中（凭据实时回读通过）", wbRuntimeOffline: "未运行", wbRuntimeProbeFailed: "运行中但凭据回读异常", wbChecking: "检测中…", wbNotChecked: "未检测", wbUnknown: "未知", wbAccessExpires: "登录有效期至", wbRefreshExpires: "续期凭据有效期至",
  wbPool: "工具账号池", wbCaptured: "当前登录账号已保存到账号池", wbNotCaptured: "尚未保存到账号池", wbVersion: "客户端版本", wbInstallPath: "安装路径", wbDataRoot: "数据根目录", wbHealthCard: "登录域健康", wbLocalStatus: "本地登录态检测", wbLocalOk: "正常",
  wbLocalWarn: "将过期", wbLocalBad: "异常", wbDeepLabel: "深度检测（实时凭据回读）", wbDeepVerified: "已验证可用", wbDeepOffline: "客户端未运行，仅本地检测", wbDeepFailed: "探针失败，建议重启客户端", wbDetect: "立即检测", wbDetectHint: "检测本地登录态与客户端实时凭据（只读，不修改任何数据）",
  wbFileActive: "已登录（登录文件有效）", wbFileAccessExpired: "登录已过期（客户端运行中会自动刷新）", wbFileFullyExpired: "登录完全过期（请在客户端重新登录）", wbFileLoggedOut: "客户端已登出", wbFileMissing: "未找到登录文件（客户端从未登录）", wbFileUnknown: "状态未知",
  wbMultiTitle: "多实例环境管理适配中", wbMultiText: "WorkBuddy 的多实例（--user-data-dir）可行性待实验票结论，实验通过前此处保持诚实说明。单实例客户端状态已在上方卡片呈现。", wbHealthFailure: "WorkBuddy：健康检测未完成——{reason}", wbNotifyPrefix: "WorkBuddy：",
} as const;

/** 文案占位符填充（builder 与本页弹层共用）：缺键保留原样，不吞占位符。 */
export const fillEnvText = (template: string, values: Readonly<Record<string, string | number>>): string =>
  template.replace(/\{(\w+)\}/g, (match, key: string) => (key in values ? String(values[key]) : match));

/** 文案键类型（builder 取文案时获得字面量补全）。 */
export type EnvTextKey = keyof typeof ENV_TEXTS;

// ===== 已知前后端文案不一致（只登记、不在 P4d 修；逐字保留是既有断言护栏） =====

/**
 * ① 收编四阶段仍渲染「创建备份」：后端 P17-A3 已退役 backing_up，实际只发 closing / incorporating / restarting / done
 *    （library_archive.rs L935/L1501/L4250）。登记位置 ENV_TEXTS.incorporateStageBackingUp（L1601）；护栏 tests/EnvironmentPage L548。
 * ② 清理空项目确认与回执仍写「删除前会自动备份」/「（已自动备份）」：后端已改为不自动备份，回执 backup_path
 *    恒空串（library_archive.rs L1501）。登记位置 ENV_TEXTS.cleanupDialogLine1（L798）/ cleanupSuccess（L395）/
 *    cleanupSuccessRelaunchFailed（L394）；护栏 tests/EnvironmentPage L362/L381/L418/L494。
 */
export const ENV_KNOWN_COPY_MISMATCHES = {
  incorporateBackupStage: { textKey: "incorporateStageBackingUp", sourceLine: 1601 },
  cleanupAutoBackup: {
    textKeys: ["cleanupDialogLine1", "cleanupSuccess", "cleanupSuccessRelaunchFailed"],
    sourceLines: [798, 395, 394],
  },
} as const;