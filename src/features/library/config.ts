/**
 * 库页视图模型 / 能力表 / 文案表（P4e-2）：纯类型与声明式配置，零 UI、零 JSX、零 invoke、零 import。
 * builder（traeLibrary / workbuddyLibrary，P4e-4）产出 LibraryView；页面只读本文件、不判断 productId。能力字段照
 * .scratch/cmdr/P4e-design.md §2 的 18 行 A/B/C（A=缺失、B=表现不同、C=一致）。文案逐字沿用旧页面（54 例 + 45 个 testid），
 * 行号缩写：L=LibrarySessionsPanel、D=MasterLibraryDetail、WB=WorkBuddyLibraryPage、P=PluginWorkbench、V=MasterVerificationPanel。
 */
// ===== 视图模型（全字段只读） =====
/** 色档：neutral=安静、ok=正常、warn=需处理、danger=失效、accent=当前/强调。 */
export type LibraryTone = "neutral" | "ok" | "warn" | "danger" | "accent";
export type LibraryActionKind = "primary" | "ghost" | "danger";
/** 徽章：text=展示词（云端/已软删除/内置/切号保留），tone 定槽位，title=悬浮说明（null 不渲染）。 */
export interface LibraryBadgeView { readonly text: string; readonly tone: LibraryTone; readonly title: string | null }
/** 动作（页头/树行/批量栏/弹层共用）：hint 即旧 title；disabledReason 非空即禁用并给悬浮说明。 */
export interface LibraryActionView { readonly id: string; readonly label: string; readonly kind: LibraryActionKind; readonly testId: string; readonly hint: string | null; readonly disabledReason: string | null; readonly busyLabel: string | null; }
/** tab（详情页三 tab 平级；testId 逐字沿用旧锚点 master-tab-*）。 */
export type LibraryTabId = "sessions" | "plugins" | "info";
export interface LibraryTabView { readonly id: LibraryTabId; readonly label: string; readonly testId: string; readonly active: boolean }
/** 页头：标题 + 只读徽章 + 产品徽章 + 返回/刷新；regionLabel 与 pageTitleAttr 逐字沿用旧页。 */
export interface LibraryHeaderView { readonly title: string; readonly regionLabel: string; readonly pageTitleAttr: string; readonly readonlyBadge: string | null; readonly badgeProductId: string | null; readonly back: LibraryActionView | null; readonly refresh: LibraryActionView | null; }
/** 会话行：title 已由 builder 回退「未命名会话」；selectable=false（cloud 行）禁用勾选。 */
export interface SessionRowView {
  readonly id: string; readonly testId: string; readonly title: string; readonly meta: string; readonly badges: readonly LibraryBadgeView[];
  readonly selectable: boolean; readonly selected: boolean; readonly archived: boolean; readonly quickArchive: LibraryActionView | null;
}
/** 左栏节点：named 具名项目 / unlinked 未关联末位组 / foreign 其他账号项目 / cwd WB 组 / mode 归档模式层。 */
export type ProjectNodeKind = "named" | "unlinked" | "foreign" | "cwd" | "mode";
export interface ProjectNodeView {
  readonly id: string; readonly testId: string; readonly kind: ProjectNodeKind; readonly title: string; readonly titleAttr: string | null;
  readonly meta: string; readonly expanded: boolean; readonly selected: boolean; readonly selectable: boolean;
  readonly actions: readonly LibraryActionView[]; readonly sessions: readonly SessionRowView[]; readonly children: readonly ProjectNodeView[];
  /** 归档视图折叠键「模式\u0000项目」（L1571-1572）；缺省 = 该节点不在归档视图，展开态由页面本地集合管。 */
  readonly collapseKey?: string;
}
/** 消息：user 按归属腿着色（accountName + colorIndex），assistant 中性色，trace=任务轨迹摘要行。 */
export interface MessageView { readonly id: string; readonly testId: string; readonly role: "user" | "assistant" | "trace"; readonly text: string; readonly accountName: string | null; readonly colorIndex: number; readonly stepCount: number | null; }
/** 接力腿：spanLabel 由 builder 给（两端 null = 最早/至今，不臆测时刻）；current=进行中腿。 */
export interface RelayLegView { readonly userId: string; readonly accountName: string; readonly spanLabel: string; readonly messages: number; readonly current: boolean; readonly colorIndex: number; }
/** 右栏查看器：tab 消息/接力；messageOffset=已加载窗口条数（「已加载最近 N 条」提示同源）。 */
export interface ViewerPaneView {
  readonly ariaLabel: string; readonly tabsAriaLabel: string; readonly activeTab: "messages" | "relay";
  readonly status: "empty" | "loading" | "ready" | "error"; readonly messages: readonly MessageView[];
  readonly legs: readonly RelayLegView[]; readonly hasMoreBefore: boolean; readonly messageOffset: number;
  readonly loadingOlder: boolean; readonly emptyText: string;
}
/** 统计卡：TRAE=status 四态 + 项目/会话/消息；WB=五项（会话/项目/账号/软删/正文字节）。 */
export interface LibraryStatView { readonly id: string; readonly label: string; readonly value: string; readonly hint: string | null }
export interface LibraryStatsView { readonly testId: string; readonly title: string; readonly items: readonly LibraryStatView[]; readonly unreadableNote: string | null; }
/** 账号过滤档（WB 删除归属锚）：selected=null 即未过滤 → 写按钮禁用并给一句说明。 */
export interface AccountFilterView { readonly testId: string; readonly label: string; readonly allLabel: string; readonly options: readonly { readonly id: string; readonly label: string }[]; readonly selected: string | null; readonly requiredNote: string | null; }
/** 删除规模预览（决策 7）：TRAE 真删一次确认；WB 软删与彻底删分开确认，彻底删备份前置。 */
export interface DeletePreviewView {
  readonly testId: string; readonly title: string; readonly confirmLabel: string; readonly busyLabel: string;
  readonly danger: boolean; readonly lines: readonly string[];
  readonly sessions: readonly { readonly id: string; readonly title: string; readonly messageCount: number }[];
  readonly sessionCount: number; readonly messageCount: number; readonly backupRequired: boolean; readonly backupNote: string | null;
}
/** 插件行（已装/市场共用）：badge=归属标签（内置/切号保留/仅此账号/已安装）；action=null 即不可操作。 */
export interface PluginRowView { readonly id: string; readonly testId: string; readonly title: string; readonly meta: string | null; readonly category: string | null; readonly badge: LibraryBadgeView | null; readonly action: LibraryActionView | null; readonly hoverTitle: string; }
/** 插件 tab：段切换 + 搜索 + 分类 chips + 行列表 + 空态；卸载单次确认（全账号传播）。 */
export type PluginSegmentId = "installed" | "market";
export interface PluginSegmentView { readonly id: PluginSegmentId; readonly label: string; readonly testId: string; readonly active: boolean }
/** 插件 tab 两形态：工作台（主库）/ 说明卡（只读库与副环境，旧 D232-243）。kind 是判别字段。 */
export type PluginTabView = PluginWorkbenchView | PluginNoteView;
/** 说明卡：只给一句说明，不渲染段 / 搜索 / 列表 / 确认层。 */
export interface PluginNoteView { readonly kind: "note"; readonly testId: string; readonly ariaLabel: string; readonly text: string }
export interface PluginWorkbenchView {
  readonly kind: "workbench";
  readonly testId: string; readonly ariaLabel: string; readonly refresh: LibraryActionView;
  readonly segments: readonly PluginSegmentView[]; readonly activeSegment: PluginSegmentId;
  readonly search: { readonly testId: string; readonly placeholder: string; readonly value: string } | null;
  readonly categories: readonly string[]; readonly activeCategory: string | null; readonly rows: readonly PluginRowView[];
  readonly emptyText: string | null;
  /** 空态锚（旧 P421/P426）；null = 当前是加载句（旧 P409-413 的 EmptyState 无锚）。 */
  readonly emptyTestId: string | null;
  readonly errorNote: string | null;
  readonly uninstall: { readonly title: string; readonly confirmLabel: string; readonly lines: readonly string[]; readonly testId: string } | null;
}
/** 库体检五项（WB 专属）：孤儿正文/缺失正文/软删规模/外部目录/归并候选；candidates 每项 canonical 在前。 */
export interface CheckupItemView { readonly id: string; readonly label: string; readonly value: string; readonly hint: string | null }
export interface CheckupView { readonly testId: string; readonly title: string; readonly items: readonly CheckupItemView[]; readonly candidates: readonly { readonly canonical: string; readonly variants: readonly string[] }[]; readonly action: LibraryActionView; readonly idleNote: string; }
/** 库信息行：value=null 即不渲染（只读库不冒充「未登录」）；titleAttr=悬浮完整路径。 */
export interface LibraryInfoItemView { readonly id: string; readonly testId: string; readonly label: string; readonly value: string | null; readonly titleAttr: string | null; }
/** 备份对比区块（只读）：基准切换 + 差异摘要 + 丢失候选 + 恢复指引；不自动改动任何数据。 */
export interface BackupCompareView {
  readonly testId: string; readonly title: string; readonly baseLabel: string;
  readonly baseOptions: readonly { readonly stamp: number; readonly label: string }[];
  readonly selectedStamp: number | null; readonly badge: LibraryBadgeView; readonly summary: string;
  readonly missing: readonly { readonly title: string; readonly messageCount: number }[];
  readonly restoreHint: string | null; readonly refresh: LibraryActionView;
  /** 报告尚未到达（进 tab 首读中 / 读取失败且无历史报告）：只渲染工具行与错误句（旧 V96-106）。 */
  readonly pending: boolean;
  /** 读取失败原因（旧 V96 页内错误行）：不遮挡已取回的报告，也不清已取回数据。 */
  readonly errorNote: string | null;
}
/** 库信息 tab：账号/大小/路径/最近活跃/最近备份 + 计数条 + 备份对比（backupOnly 形态）。 */
export interface LibraryInfoView { readonly testId: string; readonly items: readonly LibraryInfoItemView[]; readonly counts: readonly LibraryStatView[]; readonly backup: BackupCompareView | null; }
/** 页内提示：一句话 + 旧页锚点（null = 旧页无锚，不发明 testid）。 */
export interface LibraryNoticeView { readonly testId: string | null; readonly text: string }
/** 页面级兜底文案（builder 与弹层共用同一份事实，不各写一遍）。 */
export interface LibraryViewTexts { readonly receiptPrefix: string; readonly actionFailure: string; readonly refreshFailed: string; readonly readonlyBadge: string; }
/** 归档收尾的四种需就地恢复形态（旧 L149-152 / L958-980）：relaunch=实例未启动、refresh=列表刷新失败、
 *  commit-unknown=提交结果无法确认、committed-attention=已提交但写后校验未通过。 */
export type ArchiveRecoveryKind = "relaunch" | "refresh" | "commit-unknown" | "committed-attention";
/** 恢复条（带一句原因 + 重试按钮）：文案由动作侧按后端回执填好，builder 只透传与选按钮词。 */
export interface ArchiveRecoveryView { readonly kind: ArchiveRecoveryKind; readonly message: string }
/** 归档页头/页内动作区（旧 L1506-1557、L1668-1683）：入口、返回、恢复条与草稿存储告警；WB 整块为 null。 */
export interface ArchiveChromeView {
  /** 当前是否归档视图（模式 → 项目 → 会话，树灰显）。 */
  readonly view: boolean;
  /** 归档/恢复写门禁（操作就绪 + 写能力）；false 时草稿不加载、应用按钮禁用。 */
  readonly canArchive: boolean;
  /** 「已归档 N」入口（仅在正常视图且确有归档会话时给）。 */
  readonly entry: { readonly action: LibraryActionView; readonly count: number } | null;
  /** 归档视图「返回对话列表」（仅在归档视图给）。 */
  readonly back: LibraryActionView | null;
  /** 需就地恢复的异常收尾（实例未启动 / 列表刷新失败 / 提交结果待核对）。 */
  readonly recovery: ArchiveRecoveryView | null;
  /** 恢复条按钮词（relaunch → 重试启动；其余 → 刷新历史列表）。 */
  readonly retryLabel: string | null;
  /** 草稿持久化双重失败告警（旧 L1549-1557）。 */
  readonly draftError: string | null;
  /** Work 库应用前的关闭客户端确认单（CN 只读库无此单；null = 直接应用）。 */
  readonly confirm: { readonly title: string; readonly lines: readonly string[]; readonly confirmLabel: string; readonly testId: string } | null;
}
/** 库页视图模型：页面唯一渲染输入（零产品判断；差异全部由 builder + 能力表给出）。 */
export interface LibraryView {
  readonly productId: string; readonly libraryId: string;
  readonly header: LibraryHeaderView; readonly tabs: readonly LibraryTabView[]; readonly loadingNote: string | null;
  /** 读取失败卡（testId 沿用旧锚 history-error；null = 旧页无该锚，如 WB）。 */
  readonly error: { readonly title: string; readonly description: string; readonly retryLabel: string; readonly testId: string | null } | null;
  /** 整页空态（testId 沿用旧锚 history-guide；actionId 是空态引导的动作 id，由 App 决定去向）。 */
  readonly empty: { readonly title: string; readonly hint: string; readonly actionLabel: string | null;
    readonly actionId: string; readonly actionTestId: string | null; readonly testId: string } | null;
  readonly stats: LibraryStatsView | null; readonly filter: AccountFilterView | null;
  readonly includeDeleted: { readonly testId: string; readonly label: string; readonly hint: string; readonly checked: boolean } | null;
  readonly search: { readonly testId: string; readonly placeholder: string; readonly ariaLabel: string; readonly value: string } | null;
  readonly tree: readonly ProjectNodeView[]; readonly treeEmpty: string; readonly viewer: ViewerPaneView | null;
  readonly batchBar: { readonly testId: string; readonly ariaLabel: string; readonly summary: string; readonly actions: readonly LibraryActionView[] } | null;
  /** 选择入口（左栏底部「选择/取消」）：null = 该产品无此入口（WB 逐会话复选常显，不用开关）。 */
  readonly selectionEntry: { readonly label: string; readonly cancelLabel: string } | null;
  readonly draftBar: {
    readonly testId: string; readonly ariaLabel: string; readonly summary: string;
    readonly discard: LibraryActionView; readonly apply: LibraryActionView;
  } | null;
  /** 归档动作区（入口 / 返回 / 恢复条 / 草稿告警 / 应用前确认）：null = 该产品无归档（WB）。 */
  readonly archive: ArchiveChromeView | null;
  readonly checkup: CheckupView | null; readonly plugins: PluginTabView | null; readonly info: LibraryInfoView | null;
  /** 页内提示（写门禁说明 / 归档原因）：testId 沿用旧页锚点（library-work-write-pending 等）。 */
  readonly notices: readonly LibraryNoticeView[]; readonly texts: LibraryViewTexts;
}
// ===== 能力表（照 P4e-design §2 的 18 行 A/B/C；A=缺失、B=表现不同、C=一致） =====

/** 库页能力开关：页面与 builder 只读本表，禁止再写 productId 分支。 */
export interface LibraryCapabilities {
  readonly sessionGrouping: "project-tree" | "cwd-groups"; // B §2-1 左栏分组：项目树 / cwd 转义目录逐组
  readonly projectSearch: boolean; // A §2-2 标题搜索（命中项目自动展开）
  readonly selectionMode: "g22-linked" | "per-session"; // B §2-3 选择模式：项目↔会话联动 / 逐会话复选
  readonly viewer: "relay-paged" | "inline-timeline"; // B §2-4/5 查看器（消息 DTO 不同，渲染不可共用）
  readonly deleteMode: "hard-with-backup" | "soft-plus-purge"; // B §2-6 删除语义：真删 / 软删 + 彻底删
  readonly softDeleteSeparate: boolean; // B §2-6 决策 7：软删与彻底删分开确认，不合并成一个确认层
  readonly archive: "draft-apply" | "absent"; // A §2-7 归档（草稿持久化 + 关实例→单事务→重启）
  readonly projectMerge: "master-remap" | "absent"; // B §2-8 项目归并（TRAE：会话改挂 + CN 跨账号）
  readonly cwdMerge: "canonical-variants" | "absent"; // B §2-8 cwd 归并（WB 专属，不可与项目归并互换）
  readonly accountFilter: boolean; // A §2-9 账号过滤（WB 删除归属锚；TRAE 无下拉）
  readonly stats: "four-state" | "library-five"; // B §2-10 统计口径（四态 + 计数 / WB 五项）
  readonly checkupItems: number; // A §2-11 库体检项数（TRAE 0 / WB 5）
  readonly pluginTab: "workbench" | "absent"; // A §2-12 插件 tab（已装/市场 + 跨账号卸载）
  readonly libraryInfoTab: "info-and-backup" | "absent"; // A §2-13 库信息 tab（含备份对比）
  readonly writeDoubleGate: "capability-and-ready" | "command-real-mode"; // B §2-14 写能力门禁
  readonly readOnlyLibrary: "readonly-root" | "cloud-rows"; // B §2-15 只读边界（CN 整页 / cloud 行）
  readonly pageRegion: "master-library"; // B §2-16 页头/region 契约（data-page-title="master-library"）
  readonly relaunchOutcome: "relaunch-outcome" | "write-tail"; // B §2-17 进程纪律回执（relaunch_outcome / tail.restarted）
  readonly backupChain: "latest-only" | "absent"; // A §2-18 备份链（库页只读最新一份，管理归设置页）
}
export const LIBRARY_CAPABILITIES: Record<string, LibraryCapabilities> = {
  trae_cn: {
    sessionGrouping: "project-tree", projectSearch: true, selectionMode: "g22-linked", viewer: "relay-paged", deleteMode: "hard-with-backup", softDeleteSeparate: false, archive: "draft-apply", projectMerge: "master-remap",
    cwdMerge: "absent", accountFilter: false, stats: "four-state", checkupItems: 0, pluginTab: "workbench", libraryInfoTab: "info-and-backup", writeDoubleGate: "capability-and-ready", readOnlyLibrary: "readonly-root",
    pageRegion: "master-library", relaunchOutcome: "relaunch-outcome", backupChain: "latest-only",
  },
  workbuddy: {
    sessionGrouping: "cwd-groups", projectSearch: false, selectionMode: "per-session", viewer: "inline-timeline", deleteMode: "soft-plus-purge", softDeleteSeparate: true, archive: "absent", projectMerge: "absent",
    cwdMerge: "canonical-variants", accountFilter: true, stats: "library-five", checkupItems: 5, pluginTab: "absent", libraryInfoTab: "absent", writeDoubleGate: "command-real-mode", readOnlyLibrary: "cloud-rows", pageRegion: "master-library", relaunchOutcome: "write-tail", backupChain: "absent",
  },
};
/** 未登记产品回退最保守一档（无搜索/无归档/无插件/无体检），不猜能力。 */
export const DEFAULT_LIBRARY_CAPABILITIES: LibraryCapabilities = LIBRARY_CAPABILITIES.workbuddy;
/** 按产品取能力表（页面与 builder 的唯一产品差异入口）。 */
export function libraryCapabilitiesFor(productId: string): LibraryCapabilities {
  return LIBRARY_CAPABILITIES[productId] ?? DEFAULT_LIBRARY_CAPABILITIES;
}
// ===== 文案表（逐字保留旧页面口径，防单测/e2e 断言漂移；行号见每组注释） =====

export const LIBRARY_TEXTS = {
  // 左栏树 / 引导（L1337-1545）
  unlinkedGroup: "未关联文件夹", foreignGroup: "其他账号的项目", foreignMerge: "归并到当前账号", retry: "重试", loading: "正在读取库记录…", emptyNoDataTitle: "{lib}还没有对话数据。", emptyNoDataHint: "到环境页启动{lib}并在 TRAE 内登录一次，对话记录会自动出现在这里。", emptyNoDataAction: "去环境页启动",
  emptyNoAccountTitle: "{lib}尚未登记登录账号。", emptyNoAccountHint: "在环境页确认{lib}已登录账号；切换账号后记录仍会保留。", emptyNoAccountAction: "去环境页查看", loadFailure: "{lib}记录暂时无法读取，请稍后重试。", backToTree: "返回对话列表", searchPlaceholder: "搜索会话标题…", searchAria: "搜索会话标题",
  busyShort: "处理中…", retryLaunch: "重试启动", refreshHistory: "刷新历史列表", emptyNoMatch: "没有匹配的会话", emptyNoSessions: "还没有对话记录", emptyArchived: "没有已归档的会话",
  // 底部固定区 / 批量栏 / 归档草稿条（L1680-1892）
  regionLibrary: "历史", regionSessions: "对话列表", regionViewer: "对话查看器", regionBatch: "批量操作", regionDraft: "待提交的归档设置", regionViewerTabs: "对话内容", archivedEntry: "已归档", cancel: "取消", select: "选择", previewReadonly: "选择模式：预览只读", viewerEmpty: "从左侧选择一个会话查看对话",
  selectionSummary: "已选 {sessions} 个会话 · {projects} 个项目", restore: "恢复", deleteAction: "删除", archiveAction: "归档", mergeAction: "合并到…", mergeDisabled: "至少选择 2 个有名称的项目才能合并", draftSummary: "待提交：归档 {archive} 个 · 恢复 {restore} 个", draftDiscard: "放弃设置", draftApply: "应用归档设置", draftApplying: "正在应用…",
  workArchiveTitle: "应用 Work 归档设置", workArchiveLine1: "操作会尝试关闭 Work 客户端，应用完成后再尝试重新启动。", workArchiveLine2: "请先保存 Work 客户端中尚未提交的内容。", workArchiveLine3: "如果启动失败，可以在当前页面重试。", workArchiveConfirm: "关闭并应用",
  // 会话行 / 查看器（L1982-2152）
  unnamedSession: "未命名会话", quickArchiveHint: "归档这个会话", quickArchiveAria: "归档会话：{title}", messageCount: "{count} 条消息", lastActive: "最后活动 {time}", closeViewer: "关闭对话", tabMessages: "消息", tabRelay: "接力", messagesLoading: "正在读取消息…", messagesError: "消息暂时无法读取，请稍后重试。", messagesEmpty: "该会话没有可展示的消息。",
  loadingOlder: "正在加载更早消息…", loadedWindow: "已加载最近 {offset} 条消息，还可继续查看更早内容。",
  // 接力时间线 / 消息归属（L2178-2250）
  removedAccount: "已移除账号", traceSteps: "任务轨迹 · {steps} 步", relayHint: "这条会话被以下账号先后使用，按时间顺序排列。", currentLeg: "当前", legMeta: "{span} · {messages} 条消息",
  // TRAE 删除确认（L2278-2294：真删 + 备份前置，一次确认）
  deleteTitle: "删除 {count} 个会话", deleteConfirm: "确认删除", deleteBusy: "删除中…", deleteItemSuffix: " · {count} 条消息", deleteBody: "共 {messages} 条消息，删除前会自动备份。删除后这些会话将永久移除，无法在应用内恢复（备份文件保留，可人工恢复）。",
  // 合并确认（L2329-2339）
  mergeTitle: "合并 {count} 个项目", mergeConfirm: "确认合并", mergeBusy: "合并中…", mergeBody: "{moved} 个会话将移入保留的项目（含已归档），合并前会自动备份。选择要保留的项目：其余项目的全部会话都会移入它，移空的项目会被清理。误合并可用备份人工恢复。",
  // CN 归并确认（L2388-2417）
  cnMergeTitle: "归并「{project}」到当前账号", cnMergeConfirm: "确认归并", cnMergeBusy: "归并中…", unregisteredAccount: "未登记账号", cnMergeBody: "将把其他账号在这个文件夹下的 {sessions} 个会话（共 {messages} 条消息）归并给 {target}，归并前会自动备份。",
  cnMergeTargetNote: "当前账号在这个文件夹已有 {count} 个会话，将合并在一起。", cnMergeItemSuffix: " · {sessions} 个会话 · {messages} 条消息",
  // 写门禁 / 归档草稿回执（L196-217、L897-1042）
  gateUnknown: "暂时无法确认当前库的操作状态，写操作暂不可用。", gatePending: "{operations}暂不可用。{reason}", gateUnknownCn: "暂时无法确认 Trae CN 的操作状态，写操作暂不可用。", gateArchiveLabel: "归档与恢复", gateMergeLabel: "项目归并", gateDeleteLabel: "删除",
  draftQueuedArchive: "已加入待归档设置，共 {count} 个会话。完成选择后点击“应用归档设置”。", draftQueuedRestore: "已加入待恢复设置，共 {count} 个会话。完成选择后点击“应用归档设置”。", applyRelaunchFailed: "主库已更新（归档 {archived} 个、恢复 {restored} 个），实例尚未启动，请重试启动。", applyCnDone: "已应用归档设置：归档 {archived} 个、恢复 {restored} 个会话。TRAE CN 客户端未由此操作启动，需要查看时请手动启动。",
  applyDone: "已应用归档设置：归档 {archived} 个、恢复 {restored} 个会话。", commitUnknown: "提交结果无法确认。先刷新历史列表并核对状态，再决定是否重新应用；本批次不会自动重试。", committedAttention: "归档操作已提交，但写后校验未通过。先刷新历史列表检查结果，不要重新应用本批次。", draftKept: "归档设置未提交，草稿仍保留。", draftClearedSubmitted: "已清除本地归档草稿；归档操作已经提交，请以历史列表当前状态为准。",
  draftClearedCheck: "已清除本地归档草稿；请以刷新后的历史列表确认数据库状态。", draftDiscarded: "已放弃未提交的归档设置，主库没有变化。", draftApplyBlocked: "先按上方提示刷新并核对归档状态，再决定是否应用。",
  // 归档恢复条（L958-1028）：需就地重试的四种收尾与重试回执
  applyRefreshFailedSuffix: " 历史列表刷新失败，请手动刷新。", recoveryRetryRefreshFailed: "历史列表刷新失败，请先刷新并核对归档状态。",
  recoveryRetryRelaunchOk: "主库实例已重新启动，历史列表已刷新。", recoveryRetryRefreshOk: "历史列表已刷新。",
  recoveryRetryCommittedOk: "历史列表已刷新；归档操作已经提交，请核对当前状态后再继续。", recoveryRetryUnknownOk: "历史列表已刷新；请核对当前状态后再决定是否重新应用保留的草稿。",
  recoveryRelaunchFailed: "实例仍未启动，请稍后重试。", recoveryRefreshFailed: "历史列表刷新失败，请稍后重试。",
  // 回执 / 刷新失败 / 归并进度（L726-1178、L2525-2535）
  deleteReceipt: "已删除 {count} 个会话。", mergeReceipt: "已把 {moved} 个会话并入「{target}」，清理 {removed} 个空分组。", cnMergeReceipt: "已把 {moved} 个会话归并到当前账号（共 {messages} 条消息）", cnMergeReceiptCleanup: "，并清理 {removed} 个空项目。", actionFailure: "操作未完成，请稍后重试。", refreshFailedAfterAction: "操作已完成，但列表刷新失败，请手动刷新。",
  mergePreviewFailure: "归并预览暂时不可用，请稍后重试。", cnMergeFailure: "归并未完成，数据保持操作前状态；请稍后重试。", mergeRefreshFailed: "归并已完成，但项目列表刷新失败，请手动刷新。", stageBackingUp: "正在备份数据…", stageMapping: "正在整理（{current}/{total}）…", stageExecuting: "正在归并：{label}…", stageVerifying: "正在校验（{current}/{total}）…", stageDone: "完成",
  // 时间 / 模式（L585、L2482-2518）
  timeUnknown: "时间未知", justNow: "刚刚", minutesAgo: "{count} 分钟前", hoursAgo: "{count} 小时前", daysAgo: "{count} 天前", todayAt: "今天 {time}", yesterdayAt: "昨天 {time}", spanEarliest: "最早", spanOngoing: "至今", modeWork: "Work 模式", modeCode: "Code 模式", modeUnlabeledMode: "未标注模式", modeUnlabeledRaw: "未标注",
  // 详情页（D142-302）
  detailRegionMaster: "主库详情", detailRegionSuffix: "{name}详情", detailBack: "返回", detailRefresh: "刷新", detailRefreshHint: "重新读取{name}信息", readonlyBadge: "只读浏览", detailTabsAriaMaster: "主库内容", detailTabsAriaSecondary: "副库内容", detailTabSessions: "对话列表", detailTabPlugins: "插件", detailTabInfo: "库信息",
  pluginsReadonlyNote: "Trae CN 为只读浏览：不提供插件管理与对账。", pluginsSecondaryNote: "副环境不参与插件管理：插件清单跟随账号在主库共享，副环境的对话记录不涉及插件安装与对账。",
  infoCurrentAccount: "当前账号", infoSizeMaster: "主库大小", infoSizeSecondary: "数据大小", infoLastActive: "最近活跃", infoLatestBackup: "最近备份", infoPathMaster: "主库路径", infoPathSecondary: "数据目录",
  infoNotLoggedIn: "未登录", infoNoBackup: "还没有备份", infoDash: "—", infoProjectCount: "项目", infoSessionCount: "会话", infoMessageCount: "消息", infoLoadFailure: "库信息暂时不可读取，请稍后重试。",
  // WB 对话库（WB183-418）
  wbRegion: "对话库", wbRefresh: "刷新", wbRefreshHint: "重读会话索引与统计（只读）", wbLoading: "正在读取对话库…", wbErrorTitle: "无法读取对话库", wbReload: "重新读取", wbLoadFailure: "对话库暂不可读取，请稍后重试。",
  wbStatsTitle: "库内统计", wbStatSessions: "会话", wbStatProjects: "项目（cwd）", wbStatAccounts: "账号", wbStatSoftDeleted: "软删除", wbStatBytes: "正文字节", wbSoftDeletedHint: "软删除会话数（可恢复）",
  wbBrowserTitle: "会话浏览", wbAccountFilter: "账号过滤", wbAllAccounts: "全部账号", wbAccountFallback: "WorkBuddy 账号", wbIncludeDeleted: "显示软删除会话", wbIncludeDeletedHint: "软删行可在此视图彻底删除",
  wbGroupMeta: "{sessions} 条会话 · 最近活跃 {time}", wbSessionMeta: "最近活跃 {time}", wbCloudTitle: "云端会话（只读展示，工具不碰云端）", wbOpenHint: "查看会话消息", wbCloudSelectHint: "云端会话不可删除", wbSelectHint: "加入选择", wbCloudBadge: "云端", wbCloudBadgeTitle: "云同步会话：工具只读展示，写操作红线不碰", wbSoftDeletedBadge: "已软删除", wbViewing: "查看中",
  wbMessagesTitle: "会话消息（{count} 条{bodyMissing}{skipped}）", wbBodyMissing: "；正文缺失", wbSkippedLines: "；跳过 {lines} 行未解析", wbMessagesLoading: "正在读取正文…", wbNoText: "（无文本内容）", wbNoContent: "暂无内容。", wbEmptyGroups: "库中无会话（或当前过滤无匹配）。", wbSoftDelete: "软删除（{count}）", wbPurge: "彻底删除（{count}）",
  wbSoftDeleteHint: "软删除所选会话（可逆）", wbFilterFirstHint: "先按账号过滤，再执行删除（归属校验需要账号范围）", wbPurgeHint: "彻底删除所选会话（含正文；备份前置，不可逆）", wbFilterRequiredNote: "删除操作需先按账号过滤（归属校验锚）。",
  // WB 库体检（WB421-481）
  wbCheckupTitle: "库体检", wbCheckupOrphan: "孤儿正文", wbCheckupMissing: "缺失正文", wbCheckupSoftDeleted: "软删除规模", wbCheckupForeign: "外部目录", wbCheckupCandidates: "归并候选", wbCheckupOrphanValue: "{count} 份（正文有、索引无）", wbCheckupMissingValue: "{count} 条（索引有、正文无{sample}）", wbCheckupSoftDeletedValue: "{count} 条",
  wbCheckupForeignValue: "{count} 个（cwd 不在已知项目集）", wbCheckupCandidatesValue: "{count} 组（同一 cwd 多转义形态）", wbCheckupIdle: "尚未体检。", wbCheckupBusy: "体检中…", wbCheckupAction: "立即体检（只读）", wbCheckupFailure: "库体检未完成——{reason}", wbMergeEntry: "归并：{cwd}", wbMergeHint: "把 {count} 个变体目录归并到 {cwd}",
  // WB 软删除确认（WB486-500）：可逆，单次确认，云端行由后端跳过
  wbSoftConfirmTitle: "软删除会话", wbSoftConfirmLine1: "将把所选 {count} 条会话标记为已删除（可恢复）。", wbSoftConfirmLine2: "云端会话不会受影响；执行时如客户端在运行会先关闭再重启。", wbSoftConfirm: "软删除", wbSoftConfirmBusy: "删除中…", wbSoftConfirmTestId: "workbuddy-library-soft-delete-confirm",
  // WB 彻底删除确认（WB502-519，决策 7 与软删分开确认；备份失败整单拒绝）
  wbPurgeConfirmTitle: "彻底删除会话", wbPurgeConfirmLine1: "将把所选 {count} 条会话连同正文文件一并删除，不可恢复。", wbPurgeConfirmLine2: "执行前会自动创建滚动备份（含 workbuddy.db 与正文目录），备份失败则整单拒绝。",
  wbPurgeConfirmLine3: "执行时如客户端在运行会先关闭再重启。", wbPurgeConfirm: "彻底删除", wbPurgeConfirmBusy: "删除中…", wbPurgeConfirmTestId: "workbuddy-library-purge-confirm",
  // WB cwd 归并确认（WB521-537）
  wbMergeConfirmTitle: "归并项目目录", wbMergeConfirmLine1: "将把以下 {count} 个变体目录的会话归并到 {cwd}（同一 cwd 的历史转义形态收敛）：{variants}。", wbMergeConfirmLine2: "归并只改写本机索引归属与正文目录位置，不删除任何会话。", wbMergeConfirmLine3: "执行时如客户端在运行会先关闭再重启。",
  wbMergeConfirm: "确认归并", wbMergeConfirmBusy: "归并中…", wbMergeConfirmTestId: "workbuddy-library-merge-confirm",
  // WB 回执（WB132-173；tail.restarted 追加「已重启客户端」）
  wbSoftDeleteReceipt: "已软删除 {count} 条会话（可逆）{restarted}。", wbPurgeReceipt: "已彻底删除 {count} 条会话（删除正文 {bodies} 份，前置备份 {backup}）{restarted}。", wbMergeReceipt: "已归并 {moved} 条会话到 {cwd}（正文迁移 {bodies} 份）{restarted}。", wbRestarted: "，已重启客户端", wbFailureSuffix: "数据未被改动，请稍后重试。",
  wbMessagesFailure: "消息读取未完成——{reason}", wbNotifyPrefix: "WorkBuddy：",
  // 插件 tab（P27-460）
  pluginRegion: "主库插件", pluginSegmentsAria: "插件视图", pluginInstalled: "已装插件", pluginMarket: "插件市场", pluginSearch: "搜索插件", pluginRefresh: "刷新", pluginRefreshHint: "重新读取插件状态",
  pluginChipsAria: "按分类筛选", pluginChipAll: "全部", pluginUncategorized: "其他", pluginLoadingInstalled: "正在读取已装插件…", pluginLoadingMarket: "正在读取插件市场…", pluginEmptyInstalled: "当前账号还没有已装插件；可到插件市场浏览安装。", pluginEmptyMarket: "没有符合条件的插件，可调整分类或搜索词。",
  pluginBadgeBuiltin: "内置", pluginBadgeBound: "切号保留", pluginBadgeDrift: "仅此账号", pluginBadgeInstalled: "已安装", pluginVersion: "版本 {version}", pluginHoverTitle: "插件标识：{name} · 来源：{registry}", pluginHoverTitlePlain: "插件标识：{name}", pluginUninstall: "移除", pluginInstall: "安装",
  pluginUninstallHint: "从所有已登录账号移除该插件", pluginBuiltinHint: "内置插件，无需移除",
  // 插件卸载确认（P447-460，ADR-0026 决策 2：单次确认 + 全账号传播，N = known_account_count）
  pluginUninstallTitle: "移除插件", pluginUninstallConfirm: "确认移除", pluginUninstallTestId: "plugin-uninstall-confirm", pluginUninstallLine: "将同时从 {count} 个账号移除；移除后切换账号不再带走该插件。插件本体文件不受影响，可随时重新安装。",
  pluginInstalledNotice: "已安装 {name}。", pluginRemovedNotice: "已移除 {title}", pluginInstallFailure: "安装未完成，请稍后重试。", pluginUninstallFailure: "移除未完成，请稍后重试。", pluginStateFailure: "插件状态暂时不可读取，请稍后重试。", pluginMarketFailure: "插件市场暂时不可读取，请稍后重试。", pluginAccountIssue: "移除失败，可稍后重试。",
  // 库信息备份对比（V82-261；backupOnly 形态）
  verifyRefresh: "重新校验", verifyRefreshing: "校验中…", verifyBackupTitle: "备份对比", verifyBaseLabel: "对比基准", verifyBaseLatest: "最新备份（{stamp}）",
  verifyBadgeMissing: "{count} 个丢失候选", verifyBadgeNoLoss: "未发现丢失", verifyBadgeNoBackups: "暂无备份", verifyBadgeUnavailable: "备份不可用", verifyBadgeReadFailed: "读取失败", verifyBadgeNoData: "主库未启动",
  verifySummaryOk: "与备份一致的会话 {common} 个；切换后新增 {added} 个（正常）。", verifySummaryMissing: "与备份一致的会话 {common} 个；新增 {added} 个（正常）；{missing} 个会话只在备份中存在。", verifyRelayedAway: " {count} 个会话经接力换成了新会话（正常）。", verifyNoBackups: "还没有备份可对比；切换账号或手动备份后，这里会用最新备份核对会话是否完整。",
  verifyBackupMissing: "所选备份文件已不存在（可能已被清理），请换一个基准。", verifyReadFailed: "备份或主库暂时无法读取，稍后重新校验即可。", verifyNoData: "主库还没有数据，暂时没有可对比的内容。", verifyMissingItem: "「{title}」还在备份中（{messages} 条消息），当前主库里没有。", verifyRestoreHint: "如需找回：先关闭 TRAE，到备份目录把所选备份内的文件复制回原位覆盖，再重新启动；或保留现状继续使用（丢失候选只提示，不会自动改动任何数据）。",
  verifyLoadFailure: "备份对比暂时无法执行，请稍后重试。", verifyUnnamedSession: "未命名会话",
} as const;
/** 文案占位符填充（builder 与弹层共用）：缺键保留原样，不吞占位符。 */
export const fillLibraryText = (template: string, values: Readonly<Record<string, string | number>>): string => template.replace(/\{(\w+)\}/g, (match, key: string) => (key in values ? String(values[key]) : match));
/** 文案键类型（builder 取文案时获得字面量补全）。 */
export type LibraryTextKey = keyof typeof LIBRARY_TEXTS;

/** 通知前缀与页面兜底文案登记表（TRAE 回执无前缀；WB 一律「WorkBuddy：」）。 */
export const LIBRARY_VIEW_TEXTS: Record<string, LibraryViewTexts> = {
  trae_cn: { receiptPrefix: "", actionFailure: LIBRARY_TEXTS.actionFailure, refreshFailed: LIBRARY_TEXTS.refreshFailedAfterAction, readonlyBadge: LIBRARY_TEXTS.readonlyBadge },
  workbuddy: { receiptPrefix: LIBRARY_TEXTS.wbNotifyPrefix, actionFailure: LIBRARY_TEXTS.wbFailureSuffix, refreshFailed: LIBRARY_TEXTS.refreshFailedAfterAction, readonlyBadge: LIBRARY_TEXTS.readonlyBadge },
};
