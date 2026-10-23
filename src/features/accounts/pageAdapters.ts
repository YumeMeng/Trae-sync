import { beginCheckinLogin, cancelCheckinLogin, completeCheckinLogin, refreshCheckinCredentials, switchTraeCnAccount } from "../../platform/accounts";
import { previewWorkBuddySwitch, switchWorkBuddyAccount } from "../../platform/workbuddyAdapter";
import { workbuddyPoolActions } from "./poolActionCommands";
import type { PagePoolActions } from "./components/usePoolHeaderActions";
import type { AccountSnapshot } from "../../platform/accountAdapter";
import type { CredentialRefreshEntryDto } from "../../types/account_switch";
import { effectiveDisplayName, workbuddyDisplayName } from "../../utils/accountDisplay";
import { ACCOUNT_TEXTS, type AccountRowView, type AccountsView } from "./config";
import type { AccountsSnapshot } from "./registry";
import type { TraeAccountsData, WorkBuddyAccountsData } from "./components/useAccountsSnapshot";
import type { AccountBatchProgressMap } from "./components/useAccountBatchProgress";
import type { CredentialRefreshTarget } from "./components/useCredentialRefresh";
import type { PageLogin } from "./components/useAccountLoginFlow";
import type { PageRowSwitch, RowSwitchTarget } from "./components/useRowActions";
import { traePoolFields, traeRowFace, traeRowTestIds, type TraeAccountsSnapshot } from "./traeAccounts";
import { workbuddyRowTestIds } from "./workbuddyAccounts";

/**
 * 账号页产品接缝表（P4c-7c-1 从 AccountsPage 抽出）：页面唯一的产品差异入口。
 *
 * 抽出动机：详情页接线后 AccountsPage 需保持在 UI-CHARTER 的文件行数预算内（300 行），
 * 本表是「载荷键 / 进度协议值 / 本地刷新 / 列表锚点 / 凭据命令 / 快照合成 / 行锚点 /
 * 行内切号」的内聚声明，零状态、零 JSX。表内键与 registry 的 builder 表同形，未登记即 fail-closed。
 */

/** 页面自持字段：builder 是纯函数，时钟/过滤/排序/忙碌位/适配器面一律由页面注入。 */
export interface PageUi {
  readonly filter: "all" | "attention";
  readonly sort: "added" | "name" | "checkin";
  readonly now: number; // 判定时刻（Unix 秒）：凭据过期与登录槽求态用
  readonly localRefresh: boolean; // 纯本地刷新进行中（只重读本机缓存，不联网）
  /** 三路批量进度：某链有进度即该批量在跑（只用于算禁用）。 */
  readonly progress: AccountBatchProgressMap;
  /** 批量刷新凭据的二次确认执行中（入口在页头；与进度事件同源，任一为真即禁用）。 */
  readonly credentialBusy: boolean;
  /** 页头批量动作在跑（P4c-6b-2b）：无进度事件的产品（WB）只有这一路可表达忙碌。 */
  readonly creditsBusy: boolean;
  readonly healthBusy: boolean;
  /** 登录等待中（P4c-6b-2c）：builder 据此把「添加账号」置忙并换等待文案。 */
  readonly loginBusy: boolean;
  /** 行内切号在跑（P4c-6b-2d）：官方切号弹层进行中 / WB 切号执行中。 */
  readonly switchBusy: boolean;
  /** 池级页头四动作的忙碌位（P4c-9b-1 / K-04）：WB 的续期 / 健康 / 捕获 / 登录。 */
  readonly poolBusy: {
    readonly renewing: boolean;
    readonly health: boolean;
    readonly capturing: boolean;
    readonly login: boolean;
  };
  /** 工具账号选择进行中（TRAE 专有，≠ 官方切号）。 */
  readonly toolBusy: boolean;
  /** WB 切号影响面预览进行中（两段式的第一段）。 */
  readonly previewBusy: boolean;
  /** 适配器面（仅测试注入路径）：undefined = 生产路径，池面恒缺席。 */
  readonly adapterPool?: AccountSnapshot;
}

/** 登录设备面板的产品绑定（协议值 + 归属产品名）：仅 devices 能力开启的产品登记。 */
export interface PageDevices {
  /** 后端路由协议值（work_cn = Work 签到档案链，旧 AccountDetail 逐字沿用）。 */
  readonly productId: string;
  readonly productName: string;
}

/** 单产品接缝：载荷键 / 进度协议值 / 本地刷新动作 id / 列表锚点 / 快照合成 / 行与「使用中」锚点。 */
export interface PageAdapter {
  readonly payloadKey: string;
  readonly progressId: string;
  readonly refreshLocalActionId: string;
  readonly listTestId?: string;
  /** 批量刷新凭据入口（页头动作 id + 命令 + 目标解析）；缺省即本产品不提供（fail-closed）。 */
  readonly credentialRefresh?: {
    readonly actionId: string;
    readonly refresh: (profileIds: readonly string[]) => Promise<readonly CredentialRefreshEntryDto[]>;
    readonly targets: (payload: unknown) => readonly CredentialRefreshTarget[];
  };
  /** 详情页登录设备面板绑定；缺省即本产品不挂该面板（与能力表 devices 同口径）。 */
  readonly devices?: PageDevices;
  /** 添加账号 / OAuth 登录接线（页头动作 id + 命令 + 浏览器模式 + 等待文案）；缺省即本产品
   *  不提供登录弹层（fail-closed：页头动作原样透出，弹层永不出现）。 */
  readonly login?: PageLogin;
  /** 行内切号接线（P4c-6b-2d）：缺省即本产品的行内动作只透出（fail-closed，不猜命令）。 */
  readonly rowSwitch?: PageRowSwitch;
  /** 池级页头动作接线（P4c-9b-1 / K-04）：缺省即本产品页头无池级动作（只透出，不猜命令）。 */
  readonly poolActions?: PagePoolActions;
  readonly compose: (payload: unknown, ui: PageUi) => AccountsSnapshot;
  /** 行锚点与「使用中」不可见锚点：视觉表达仍归 builder 的 accent 徽章，此处只承载旧 testid。 */
  readonly testIds: (row: AccountRowView, snapshot: AccountsSnapshot) => { readonly row: string; readonly selected: string };
}

/** TRAE：数据面 + 页面字段 → builder 快照；批量忙碌位由进度链在跑推导。 */
const composeTrae = (payload: unknown, ui: PageUi): AccountsSnapshot => ({
  productId: "trae_cn",
  trae: {
    ...(payload as TraeAccountsData),
    ...traePoolFields(ui.adapterPool),
    filter: ui.filter, sort: ui.sort, nowUnixSeconds: ui.now,
    busy: {
      login: ui.loginBusy, switching: ui.switchBusy, toolSelection: ui.toolBusy, localRefresh: ui.localRefresh,
      // 额度/健康检测：进度事件与动作忙碌位同源（TRAE 两路都有，任一为真即禁用）。
      credits: ui.progress.credits_refresh !== undefined || ui.creditsBusy,
      credentialRefresh: ui.progress.credential_refresh !== undefined || ui.credentialBusy,
      health: ui.progress.health_check !== undefined || ui.healthBusy,
    },
  },
});

/** WB：无三路进度事件（旧 WorkBuddyPanel 未订阅），忙碌位只认本地刷新与切号两段式。 */
const composeWorkBuddy = (payload: unknown, ui: PageUi): AccountsSnapshot => ({
  productId: "workbuddy",
  workbuddy: {
    ...(payload as WorkBuddyAccountsData),
    sort: ui.sort === "checkin" ? "added" : ui.sort, nowUnixSeconds: ui.now,
    busy: {
      // 池级四动作（P4c-9b-1 / K-04）：忙碌位来自 poolActions 接缝；捕获确认层打开不算忙（旧 L496 同口径）。
      loading: false, renewing: ui.poolBusy.renewing, health: ui.poolBusy.health,
      capturing: ui.poolBusy.capturing, login: ui.poolBusy.login,
      switching: ui.switchBusy, preview: ui.previewBusy,
      refreshing: ui.localRefresh,
    },
  },
});

/** TRAE 锚点：数据面（适配器面 / 真实总览 / fixture 回退）决定前缀，逐字沿用旧 testid。 */
const traeTestIds = (row: AccountRowView, snapshot: AccountsSnapshot) =>
  traeRowTestIds(traeRowFace((snapshot as { readonly trae: TraeAccountsSnapshot }).trae), row.id);
/** WB 锚点只用 profile_id 前缀（积分/归档锚点归详情，本票不渲染）。 */
const wbTestIds = (row: AccountRowView) => workbuddyRowTestIds(row.id, "");

/** 行目标解析（TRAE）：总览 / 池 / 存档三面按 profile_id 取展示名；都找不到即 null（不猜）。 */
const traeSwitchTarget = (payload: unknown, rowId: string): RowSwitchTarget | null => {
  const data = payload as TraeAccountsData;
  const entry = data.overview.find((item) => item.profile_id === rowId);
  if (entry !== undefined) return { profileId: rowId, displayName: effectiveDisplayName(entry), accountId: null };
  const pool = data.traePool?.find((item) => item.profile_id === rowId);
  if (pool !== undefined) return { profileId: rowId, displayName: pool.display_name, accountId: null };
  const saved = data.savedAccounts.find((item) => item.profile_id === rowId);
  return saved === undefined ? null : { profileId: rowId, displayName: saved.display_name, accountId: null };
};
/** 行目标解析（WB）：命令按账号 uid（account_id），展示名走备注名优先。 */
const workbuddySwitchTarget = (payload: unknown, rowId: string): RowSwitchTarget | null => {
  const account = (payload as WorkBuddyAccountsData).pool.find((item) => item.profile_id === rowId);
  return account === undefined ? null
    : { profileId: rowId, displayName: workbuddyDisplayName(account), accountId: account.account_id };
};

/** TRAE 行内接缝（P4c-6b-2d）：官方五步切号弹层 + 工具账号选择指针（旧 L600-619 / L971-974）。 */
const traeRowSwitch: PageRowSwitch = {
  targetOf: traeSwitchTarget,
  // 官方切号：action id = account-switch-<profileId>，只打开弹层（事务全在组件内，一行不改）。
  master: { actionIdPrefix: "account-switch-" },
  // 工具账号选择 ≠ 官方切号：只更新本工具的选择指针，不动客户端登录身份（文案逐字）。
  tool: {
    actionId: "account-adapter-tool-select",
    select: (profileId) => switchTraeCnAccount({ profileId }),
    successCopy: (name) => `已在 Trae Sync 中将“${name || "未命名账号"}”设为工具当前账号；官方客户端登录账号没有改变。`,
    failureCopy: "TRAE 工具账号选择未完成。",
    refreshFailedCopy: "TRAE 的工具账号选择已完成，但页面状态刷新失败，请点击刷新确认。",
  },
};

/** WB 行内接缝（P4c-6b-2d）：预览 → 单次确认 → 写凭据 + 过户 + 冷启动（旧 L215-283）。 */
const workbuddyRowSwitch: PageRowSwitch = {
  targetOf: workbuddySwitchTarget,
  workbuddy: {
    actionIdPrefix: "workbuddy-switch-",
    preview: previewWorkBuddySwitch,
    run: switchWorkBuddyAccount,
    notifyPrefix: ACCOUNT_TEXTS.notifyPrefixWorkbuddy,
  },
};

/** 产品接缝表（页面唯一的产品差异入口，与 registry 的 builder 表同形）；未登记即 fail-closed。 */
export const PAGE_ADAPTERS: Record<string, PageAdapter> = {
  trae_cn: {
    payloadKey: "trae",
    // 账号域批量事件协议值沿用 "work_cn"（ADR-0035：链路标识，不是产品切换概念）。
    progressId: "work_cn",
    refreshLocalActionId: "account-refresh-local",
    // 批量刷新凭据：真实总览面才给入口（builder 同一门禁），目标即总览全部账号。
    credentialRefresh: {
      actionId: "account-refresh-credentials",
      refresh: (profileIds) => refreshCheckinCredentials({ profileIds }),
      targets: (payload) => (payload as TraeAccountsData).overview
        .map((entry) => ({ profileId: entry.profile_id, displayName: effectiveDisplayName(entry) })),
    },
    // 详情页登录设备：旧 AccountDetail L489-497 的 productId/productName 逐字沿用。
    devices: { productId: "work_cn", productName: "TRAE" },
    // 添加账号 / 登录（P4c-6b-2c）：命令即 platform/accounts 的 begin/complete/cancel，
    // 页头动作 id 与等待文案逐字沿用旧 AccountCenterTrae L1109 / L1122-1125。
    login: {
      actionId: "account-add-primary",
      commands: {
        begin: (useSystemBrowser) => beginCheckinLogin({ useSystemBrowser }),
        complete: (attemptId) => completeCheckinLogin({ attemptId }),
        cancel: (attemptId) => cancelCheckinLogin({ attemptId }),
      },
      browserModes: true,
      waitingHint: (useSystemBrowser) => useSystemBrowser
        ? "已在本机浏览器打开 TRAE 登录页（复用系统登录态），完成授权后此处会自动继续；期间无需手动刷新。"
        : "已在隔离浏览器打开 TRAE 登录页，完成登录后此处会自动继续；期间无需手动刷新。",
    },
    rowSwitch: traeRowSwitch,
    compose: composeTrae,
    testIds: traeTestIds,
  },
  workbuddy: {
    payloadKey: "workbuddy",
    progressId: "workbuddy",
    refreshLocalActionId: "workbuddy-refresh",
    // 列表容器锚点：沿用旧 workbuddy-pool-list。
    listTestId: "workbuddy-pool-list",
    // 设备能力关闭（config 能力表 devices=false）：不登记面板绑定，详情页不挂设备插槽。
    rowSwitch: workbuddyRowSwitch,
    // 池级页头四动作（K-04）：旧 WorkBuddyPanel L172-342 实装的续期 / 健康 / 捕获 / 登录。
    poolActions: workbuddyPoolActions,
    compose: composeWorkBuddy,
    testIds: wbTestIds,
  },
};

/** 空态主行动 → 页头对应动作 id：两 builder 都把「添加账号」排在最后一个 primary 动作。 */
export function addActionIdOf(view: AccountsView): string {
  const primaries = view.header.actions.filter((action) => action.kind === "primary");
  return primaries.length === 0 ? "" : primaries[primaries.length - 1].id;
}
