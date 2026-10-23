/**
 * 账号页视图模型与能力表（P4c-2）：纯类型 + 声明式配置，零 UI、零 import、零 invoke。
 * 两个 builder（traeAccounts / workbuddyAccounts）产出 AccountsView；页面只读本文件
 * 的视图模型与 ACCOUNT_CAPABILITIES，**不再判断 productId**。
 */

// ===== 视图模型（全字段只读） =====

/** 色档：neutral=安静、warn=需处理、danger=失效、accent=当前/强调。 */
export type AccountTone = "neutral" | "warn" | "danger" | "accent";

/** 徽章：TRAE 两槽位（签到槽 + 登录存档五态）/ WB 单槽（凭据三态）+ 归档标记。 */
export interface AccountBadgeView { readonly text: string; readonly tone: AccountTone }

/** 指标格：列表侧栏与卡片视图共用（label 小字 + value 主数字）。 */
export interface AccountMetricView { readonly label: string; readonly value: string; readonly tone: AccountTone }

export type AccountActionKind = "primary" | "ghost" | "danger";

/** 动作（行内/批量/详情共用）：disabled.reason 非空即禁用并给悬浮说明。 */
export interface AccountActionView {
  readonly id: string;
  readonly label: string;
  readonly kind: AccountActionKind;
  readonly disabled?: { readonly reason: string };
}

/** 单个账号行：两产品数据面归一后的展示输入；subtitle 空串 / archived=false 即不渲染。 */
export interface AccountRowView {
  readonly id: string;
  readonly displayName: string;
  readonly subtitle: string;
  readonly badges: readonly AccountBadgeView[];
  readonly metrics: readonly AccountMetricView[];
  readonly actions: readonly AccountActionView[];
  readonly archived: boolean;
  /** 本产品「使用中」账号（徽章与不可见锚点同源）：页面据此给 selectedTestId，不再渲染第二个徽章。 */
  readonly current: boolean;
}

export type AccountDetailSectionId = "basics" | "credential" | "checkin" | "actions" | "danger" | "tech";

/** 详情字段：value=null = 本产品不渲染该行（诚实缺席，不占位不臆测）。 */
export interface AccountDetailFieldView { readonly id: string; readonly label: string; readonly value: string | null }

/** 详情分区装配描述：字段与动作声明式给出，组件按 id 映射渲染；hint=null 即无说明。 */
export interface DetailSectionView {
  readonly id: AccountDetailSectionId;
  readonly title: string;
  readonly fields: readonly AccountDetailFieldView[];
  readonly actions: readonly AccountActionView[];
  readonly hint: string | null;
}

/** 页头：标题 + 徽章（本机加密/凭据状态）+ 动作集（刷新/健康/额度/凭据/添加）。 */
export interface AccountsHeaderView {
  readonly title: string;
  readonly badges: readonly AccountBadgeView[];
  readonly actions: readonly AccountActionView[];
}

/** 控件行选项（过滤档/排序档）；count=null = 不显示计数。 */
export interface AccountsOptionView { readonly key: string; readonly label: string; readonly count: number | null }

/** 筛选档：null = 本产品无过滤行（WB attentionFilter=false）。 */
export interface AccountsFiltersView {
  readonly options: readonly AccountsOptionView[];
  readonly active: string;
  readonly sortOptions: readonly AccountsOptionView[];
  readonly sort: string;
}

/** 空态：actionLabel=null = 不给动作（过滤空态不给「添加账号」）。 */
export interface AccountsEmptyView {
  readonly description: string;
  readonly actionLabel: string | null;
  readonly testId: string;
}

/** 账号页视图模型：页面唯一渲染输入（stats=null 即不渲染摘要）。 */
export interface AccountsView {
  readonly header: AccountsHeaderView;
  readonly stats: readonly AccountMetricView[] | null;
  readonly filters: AccountsFiltersView | null;
  readonly rows: readonly AccountRowView[];
  /** 账号总数（本产品口径）：rows 只是本页可见行（过滤后），计数文案必须用它而非 rows.length。 */
  readonly totalCount: number;
  readonly listActions: readonly AccountActionView[];
  readonly empty: AccountsEmptyView;
  readonly detail: readonly DetailSectionView[];
}

// ===== 能力表（照 P4c-design §2 的 A/B/C 结论；A=缺失、B=表现不同、C=一致） =====

/** 账号页能力开关：页面与 builder 只读本表，禁止再写 productId 分支。 */
export interface AccountCapabilities {
  readonly badgeSlots: 1 | 2; // B 徽章槽：TRAE 两槽 / WB 单槽
  readonly creditsColumn: boolean; // B 积分列：TRAE 额度剩余 / WB 余额
  readonly creditsKind: "quota-remaining" | "billing-balance"; // B 积分口径
  readonly mobileBackfill: boolean; // A 手机号补录 + 查重（WB 仅服务端脱敏）
  readonly archiveTag: boolean; // A 归档标记/开关：仅 WB
  readonly attentionFilter: boolean; // A G14 需处理过滤：仅 TRAE
  readonly checkinSort: boolean; // A 排序「签到状态」档：仅 TRAE
  readonly inlineCheckin: boolean; // A 详情内「立即签到」：仅 TRAE
  readonly devices: boolean; // A 登录设备管理：仅 TRAE（work_cn 路由，不得改写）
  readonly resetDevice: boolean; // A 重置签到设备：仅 TRAE
  readonly toolSelection: boolean; // A 工具账号选择（≠ 官方切号）：仅 TRAE
  readonly switchTransaction: "master-five-step" | "preview-confirm-coldstart"; // B 切号事务
  readonly switchProgress: boolean; // A 切号进度/回滚事件：仅 TRAE
  readonly credentialRefresh: "batch-with-confirm" | "pool-renew"; // B 凭据刷新粒度
  readonly capture: boolean; // A 客户端捕获入池：仅 WB
  readonly loginBrowser: "isolated-or-system" | "system-only"; // B 登录浏览器
  readonly autoCheckinToggle: boolean; // C 自动签到参与开关（两产品都有）
  readonly deleteScope: "archive-credential-credits" | "pool-record-profile"; // B 删除边界
}

export const ACCOUNT_CAPABILITIES: Record<string, AccountCapabilities> = {
  trae_cn: {
    badgeSlots: 2, creditsColumn: true, creditsKind: "quota-remaining",
    mobileBackfill: true, archiveTag: false, attentionFilter: true,
    checkinSort: true, inlineCheckin: true, devices: true, resetDevice: true,
    toolSelection: true, switchProgress: true, autoCheckinToggle: true,
    switchTransaction: "master-five-step", credentialRefresh: "batch-with-confirm",
    capture: false, loginBrowser: "isolated-or-system",
    deleteScope: "archive-credential-credits",
  },
  workbuddy: {
    badgeSlots: 1, creditsColumn: true, creditsKind: "billing-balance",
    mobileBackfill: false, archiveTag: true, attentionFilter: false,
    checkinSort: false, inlineCheckin: false, devices: false, resetDevice: false,
    toolSelection: false, switchProgress: false, autoCheckinToggle: true,
    switchTransaction: "preview-confirm-coldstart", credentialRefresh: "pool-renew",
    capture: true, loginBrowser: "system-only",
    deleteScope: "pool-record-profile",
  },
};

/** 未登记产品回退最保守一档（单槽、无补录/设备/过滤），不猜能力。 */
export const DEFAULT_ACCOUNT_CAPABILITIES: AccountCapabilities = ACCOUNT_CAPABILITIES.workbuddy;

/** 按产品取能力表（页面与 builder 的唯一产品差异入口）。 */
export function accountCapabilitiesFor(productId: string): AccountCapabilities {
  return ACCOUNT_CAPABILITIES[productId] ?? DEFAULT_ACCOUNT_CAPABILITIES;
}

// ===== 文案表（逐字保留旧页面口径，防 e2e/单测断言漂移；来源见每项注释） =====

export const ACCOUNT_TEXTS = {
  pageTitle: "账号", // AccountPageSkeleton L103/131/152（骨架硬编码，冻结）
  loading: "正在读取账号信息…", // 同上
  emptyTrae: "尚未保存账号。点击右上角“添加账号”通过浏览器登录。", // AccountCenterTrae L1022/1042
  emptyTraeCheckin: "还没有账号。通过浏览器登录添加第一个账号，登录成功后即可签到。", // L1033
  emptyFiltered: "没有需要处理的账号。", // L1041（G14 过滤空态）
  emptyWorkbuddy: "还没有保存 WorkBuddy 账号。点击右上角「添加账号」通过浏览器登录，或在客户端登录后点「保存当前登录账号」。", // WorkBuddyPanel L555
  addAccount: "添加账号", // AccountCenterTrae L1110 / WorkBuddyPanel L514
  addAccountBusy: "等待浏览器登录完成…",
  refreshLocal: "刷新",
  healthCheck: "健康检测",
  refreshCredits: "刷新额度",
  refreshCredentials: "刷新凭据", // AccountCenterTrae L1087
  dangerHeading: "危险操作", // AccountDetail L501 / WorkBuddyAccountDetail L358
  dangerHintTrae: "删除将移除账号档案、本机加密登录凭据与积分缓存；不影响其他账号与历史数据，删除后可通过重新登录找回。", // AccountDetail L504
  dangerHintWorkbuddy: "删除范围 = 工具侧账号池记录与档案备注（备注名/归档）；不会改动 WorkBuddy 客户端的登录状态（客户端如需退出登录请在客户端操作）。删除后该账号不可再切换登录或直连签到；重新保存或登录可恢复。", // WorkBuddyAccountDetail L361-363
  deleteTitleTrae: "删除账号", // AccountDetail L174
  deleteTitleWorkbuddy: "删除 WorkBuddy 账号", // WorkBuddyAccountDetail L399
  deleteConfirmLabelTrae: "删除", // AccountDetail L180
  deleteConfirmLabelWorkbuddy: "确认删除", // WorkBuddyAccountDetail L406
  deleteBusyLabel: "删除中…", // AccountDetail L181 / WorkBuddyAccountDetail L407
  failureFallback: "操作未完成，请稍后重试。", // AccountDetail L91（详情操作失败兜底）
  credentialRefreshed: "登录凭据已更新，无需重新登录。", // AccountDetail L201（详情单账号换发成功回执）
  checkinClaimed: "签到成功，奖励已发放。", // AccountDetail L227（签到 claimed 回执）
  checkinAlready: "今日已签到，无需重复领取。", // AccountDetail L229（already_checked_in 回执）
  deviceReset: "签到设备已重置，下次签到将使用新设备。", // AccountDetail L207（重置设备成功回执）
  refreshFailureKept: "刷新未完成，正在显示上次读取到的账号信息。", // 已有数据时刷新失败：保留旧数据 + 通知（旧 G15 语义）
  notifyPrefixWorkbuddy: "WorkBuddy：", // WorkBuddyPanel L208 等（全角冒号）
  switchBusyLabel: "切换中…", // 旧 AccountCard 的 actionBusyLabel 缺省（SharedAccountEntry L73）
  toolSelectBusyLabel: "设置中…", // 旧 AdapterAccountCard L1257
} as const;

/**
 * 动作执行中的按钮文案（P4c-6b-2d）：键 = 动作 id 或 id 前缀，值逐字沿用旧实现。
 * 行内按钮忙碌时只换文案并转圈（禁用原因仍由 builder 的 disabled 给），不改任何命令语义。
 */
const ACTION_BUSY_LABELS: ReadonlyArray<readonly [RegExp, string]> = [
  [/^account-switch-/, ACCOUNT_TEXTS.switchBusyLabel],
  [/^workbuddy-switch-/, ACCOUNT_TEXTS.switchBusyLabel],
  [/^account-adapter-tool-select$/, ACCOUNT_TEXTS.toolSelectBusyLabel],
];

/** 按动作 id 取忙碌文案；null = 该动作无忙碌文案（沿用 builder 的 label，不发明说法）。 */
export function actionBusyLabelOf(actionId: string): string | null {
  for (const [pattern, label] of ACTION_BUSY_LABELS) if (pattern.test(actionId)) return label;
  return null;
}

/** 文案键类型（builder 取文案时获得字面量补全）。 */
export type AccountTextKey = keyof typeof ACCOUNT_TEXTS;

// ===== 详情页视图模型（P4c-7a）：字段 / 动作 / 整体；六区声明复用上方 DetailSectionView =====

/** 详情字段渲染附加信息：tone 只表达需处理程度（不提供成功绿）；hint 非空即给悬浮/行内说明。 */
export interface DetailFieldView {
  readonly label: string;
  readonly value: string;
  readonly tone?: "neutral" | "warn" | "danger";
  readonly hint?: string;
  /** 行内编辑（P4c-6b-2e-2）：非空即该字段点击进入编辑态；保存动作 id = 字段 id + "-save"。 */
  readonly edit?: DetailFieldEditView;
  /** 开关字段（P4c-6b-2e-2）：非空即渲染成 checkbox，on = 权威开关态（乐观翻转由页面覆盖）。 */
  readonly toggle?: DetailFieldToggleView;
}

/** 行内编辑描述：草稿初值与占位一律由 builder 给（旧实现以档案原值为准，不回退展示名）。 */
export interface DetailFieldEditView {
  readonly kind: "text" | "tel";
  readonly maxLength: number;
  /** 进入编辑态时的初始草稿（保存空串 = 清除，回退服务端值）。 */
  readonly draft: string;
  /** 输入框占位（备注名 = 服务端名；手机号 = 脱敏号）。 */
  readonly placeholder: string;
}

/**
 * 开关字段描述：on = 后端权威态，onLabel/offLabel = 两态文案（乐观翻转时据此换词，
 * 不从 value 反推）；value 仍由 builder 按 on 取同一份文案，展示契约不变。
 */
export interface DetailFieldToggleView {
  readonly on: boolean;
  readonly onLabel: string;
  readonly offLabel: string;
}

/** 详情动作：disabledReason 非空即禁用并给悬浮说明（与页头动作的 disabled.reason 同一口径）。 */
export interface DetailActionView {
  readonly id: string;
  readonly label: string;
  readonly kind: AccountActionKind;
  readonly disabledReason?: string;
}

/** 详情页视图模型：页头 + 六区（basics→credential→checkin→actions→danger→tech，缺省不出现）。 */
export interface DetailViewModel {
  readonly header: { readonly title: string; readonly subtitle?: string; readonly badges: readonly AccountBadgeView[] };
  readonly sections: readonly DetailSectionView[];
  /** 进行中的动作 id：按钮只加忙碌态，文案覆盖由 builder 的 label 决定。 */
  readonly busy?: readonly string[];
}
