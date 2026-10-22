import { refreshCheckinCredentials } from "../../platform/accounts";
import type { AccountSnapshot } from "../../platform/accountAdapter";
import type { CredentialRefreshEntryDto } from "../../types/account_switch";
import { effectiveDisplayName } from "../../utils/accountDisplay";
import type { AccountRowView, AccountsView } from "./config";
import type { AccountsSnapshot } from "./registry";
import type { TraeAccountsData, WorkBuddyAccountsData } from "./components/useAccountsSnapshot";
import type { AccountBatchProgressMap } from "./components/useAccountBatchProgress";
import type { CredentialRefreshTarget } from "./components/useCredentialRefresh";
import { traePoolFields, traeRowFace, traeRowTestIds, type TraeAccountsSnapshot } from "./traeAccounts";
import { workbuddyRowTestIds } from "./workbuddyAccounts";

/**
 * 账号页产品接缝表（P4c-7c-1 从 AccountsPage 抽出）：页面唯一的产品差异入口。
 *
 * 抽出动机：详情页接线后 AccountsPage 需保持在 UI-CHARTER 的文件行数预算内（300 行），
 * 本表是「载荷键 / 进度协议值 / 本地刷新 / 列表锚点 / 凭据命令 / 快照合成 / 行锚点」
 * 的内聚声明，零状态、零 JSX。表内键与 registry 的 builder 表同形，未登记即 fail-closed。
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
      // 登录 / 切号 / 工具账号选择在 6b-2 接入，本票恒空闲。
      login: false, switching: false, toolSelection: false, localRefresh: ui.localRefresh,
      credits: ui.progress.credits_refresh !== undefined,
      credentialRefresh: ui.progress.credential_refresh !== undefined || ui.credentialBusy,
      health: ui.progress.health_check !== undefined,
    },
  },
});

/** WB：无三路进度事件（旧 WorkBuddyPanel 未订阅），忙碌位只认本地刷新。 */
const composeWorkBuddy = (payload: unknown, ui: PageUi): AccountsSnapshot => ({
  productId: "workbuddy",
  workbuddy: {
    ...(payload as WorkBuddyAccountsData),
    sort: ui.sort === "checkin" ? "added" : ui.sort, nowUnixSeconds: ui.now,
    busy: {
      // 续期 / 健康 / 捕获 / 切号 / 登录在 6b-2 接入，本票恒空闲。
      loading: false, renewing: false, health: false, capturing: false, switching: false, preview: false,
      login: false, refreshing: ui.localRefresh,
    },
  },
});

/** TRAE 锚点：数据面（适配器面 / 真实总览 / fixture 回退）决定前缀，逐字沿用旧 testid。 */
const traeTestIds = (row: AccountRowView, snapshot: AccountsSnapshot) =>
  traeRowTestIds(traeRowFace((snapshot as { readonly trae: TraeAccountsSnapshot }).trae), row.id);
/** WB 锚点只用 profile_id 前缀（积分/归档锚点归详情，本票不渲染）。 */
const wbTestIds = (row: AccountRowView) => workbuddyRowTestIds(row.id, "");

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
    compose: composeWorkBuddy,
    testIds: wbTestIds,
  },
};

/** 空态主行动 → 页头对应动作 id：两 builder 都把「添加账号」排在最后一个 primary 动作。 */
export function addActionIdOf(view: AccountsView): string {
  const primaries = view.header.actions.filter((action) => action.kind === "primary");
  return primaries.length === 0 ? "" : primaries[primaries.length - 1].id;
}
