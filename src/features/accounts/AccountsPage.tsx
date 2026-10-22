import { useCallback, useState } from "react";
import { UserRound, UserPlus } from "lucide-react";
import { EmptyState } from "../../components/shared/EmptyState";
import { ProductBadge } from "../../components/shared/ProductBadge";
import type { AccountSnapshot } from "../../platform/accountAdapter";
import type { AccountRowView, AccountsView } from "./config";
import { AccountsHeader } from "./components/AccountsHeader";
import { AccountsRows, type AccountsRowView } from "./components/AccountsRows";
import { AccountsListControls } from "./components/AccountsListControls";
import { useRetainedAccountsData } from "./components/useRetainedAccountsData";
import { useAccountBatchProgress, type AccountBatchProgressMap } from "./components/useAccountBatchProgress";
import { CredentialRefreshDialog } from "./components/CredentialRefreshDialog";
import { useCredentialRefresh, type CredentialRefreshTarget } from "./components/useCredentialRefresh";
import { refreshCheckinCredentials } from "../../platform/accounts";
import type { CredentialRefreshEntryDto } from "../../types/account_switch";
import { effectiveDisplayName } from "../../utils/accountDisplay";
import { useAccountsSnapshot, type TraeAccountsData, type WorkBuddyAccountsData } from "./components/useAccountsSnapshot";
import { accountsViewFor, type AccountsSnapshot } from "./registry";
import { traePoolFields, traeRowFace, traeRowTestIds, type TraeAccountsSnapshot } from "./traeAccounts";
import { workbuddyRowTestIds } from "./workbuddyAccounts";
import { AccountsSkeleton } from "./AccountsSkeleton";

/**
 * 账号页装配（P4c-6b-1/6b-2a：读取 + 渲染）。产品差异全部来自 features/accounts 的声明式配置：
 * 数据面由 useAccountsSnapshot 按产品取，视图模型由 registry.accountsViewFor 查表产出，本文件零
 * productId 相等判断、零 invoke(。动作语义（切号/登录/凭据确认/详情）归 6b-2、6c、7，本票只透出。
 */

/** 会话内过滤档（G14）：仅 TRAE 真实总览面出现，页面只持值、不判断产品。 */
type PageFilter = "all" | "attention";
/** 排序档（U-2）：WB 无「签到状态」档，builder 已裁掉该选项。 */
type PageSort = "added" | "name" | "checkin";

/** 页面自持字段：builder 是纯函数，时钟/过滤/排序/忙碌位/适配器面一律由页面注入。 */
interface PageUi {
  readonly filter: PageFilter;
  readonly sort: PageSort;
  readonly now: number; // 判定时刻（Unix 秒）：凭据过期与登录槽求态用
  readonly localRefresh: boolean; // 纯本地刷新进行中（只重读本机缓存，不联网）
  /** 三路批量进度：某链有进度即该批量在跑（只用于算禁用）。 */
  readonly progress: AccountBatchProgressMap;
  /** 批量刷新凭据的二次确认执行中（入口在页头；与进度事件同源，任一为真即禁用）。 */
  readonly credentialBusy: boolean;
  /** 适配器面（仅测试注入路径）：undefined = 生产路径，池面恒缺席。 */
  readonly adapterPool?: AccountSnapshot;
}

/** 单产品接缝：载荷键 / 进度协议值 / 本地刷新动作 id / 列表锚点 / 快照合成 / 行与「使用中」锚点。 */
interface PageAdapter {
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
const PAGE_ADAPTERS: Record<string, PageAdapter> = {
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
    compose: composeTrae,
    testIds: traeTestIds,
  },
  workbuddy: {
    payloadKey: "workbuddy",
    progressId: "workbuddy",
    refreshLocalActionId: "workbuddy-refresh",
    // 列表容器锚点：沿用旧 workbuddy-pool-list。
    listTestId: "workbuddy-pool-list",
    compose: composeWorkBuddy,
    testIds: wbTestIds,
  },
};

/** 空态主行动 → 页头对应动作 id：两 builder 都把「添加账号」排在最后一个 primary 动作。 */
function addActionIdOf(view: AccountsView): string {
  const primaries = view.header.actions.filter((action) => action.kind === "primary");
  return primaries.length === 0 ? "" : primaries[primaries.length - 1].id;
}

export interface AccountsPageProps {
  /** 页面可见才读取（后台页保持挂载但不发 IPC）。 */
  readonly active: boolean;
  /** 当前产品协议值：只用于查声明式配置，页面内不做任何产品判断。 */
  readonly productId: string;
  /** 行点击进详情（6c/7 接入）：缺省时行仍可点，但不发生跳转。 */
  readonly onOpenDetail?: (rowId: string) => void;
  /** 页头其余动作（健康检测 / 额度 / 凭据 / 添加账号 / 登录）：本票只透出，不实现。 */
  readonly onHeaderAction?: (actionId: string) => void;
  /** 行内动作（切号 / 工具账号选择）：本票只透出，不实现。 */
  readonly onRowAction?: (rowId: string, actionId: string) => void;
  /** 适配器面数据（旧 adapter 注入路径，仅测试传）：注入 TRAE 快照的 traePool / poolStatusLabel。 */
  readonly adapterPool?: AccountSnapshot;
}

export function AccountsPage(props: AccountsPageProps) {
  const { active, productId, onOpenDetail, onHeaderAction, onRowAction, adapterPool } = props;
  const adapter = PAGE_ADAPTERS[productId];
  const { snapshot: data, loading, error, refreshLocal } = useAccountsSnapshot(productId, active);
  const { progress } = useAccountBatchProgress(adapter?.progressId ?? productId);
  const [filter, setFilter] = useState<PageFilter>("all");
  const [sort, setSort] = useState<PageSort>("added");
  // 正在执行的页头动作 id（非空即本地刷新在跑：页头转圈 + 禁用）。
  const [pending, setPending] = useState<string | null>(null);
  const refreshLocalActionId = adapter?.refreshLocalActionId ?? "";
  const localRefreshBusy = pending !== null;
  // 数据面保留位：刷新失败时继续渲染上次读到的数据（首次加载失败才落错误卡）。
  const { shown } = useRetainedAccountsData(
    data, adapter?.payloadKey ?? "", `${productId}|${active}`, error, localRefreshBusy,
  );

  // 纯本地刷新：只重读本机缓存（零网络命令）；失败由保留位的通知路径处理，不清数据。
  const handleRefreshLocal = useCallback(() => {
    if (localRefreshBusy) return;
    setPending(refreshLocalActionId);
    void refreshLocal().finally(() => setPending(null));
  }, [localRefreshBusy, refreshLocal, refreshLocalActionId]);
  // 批量刷新凭据：入口只开二次确认弹层，确认后换发全部账号并重读缓存（旧 L680-729 同契约）。
  // 目标在确认时才解析（用最新快照），命令与入口 id 来自产品接缝表，缺省即不提供该入口。
  const credential = adapter?.credentialRefresh;
  const credentialRefresh = useCredentialRefresh({
    refresh: credential?.refresh,
    targets: () => credential === undefined || adapter === undefined || shown === null
      ? [] : credential.targets((shown as unknown as Record<string, unknown>)[adapter.payloadKey]),
    onRefreshed: refreshLocal,
  });
  // 换发进行中：二次确认执行中，或进度事件链在跑（两者同源表达，任一为真即禁用页头按钮）。
  const credentialConfirmBusy = credentialRefresh.busy || progress.credential_refresh !== undefined;

  // 快照合成：数据面按载荷键取（表内键与载荷键一一对应），再补页面自持字段。
  const ui: PageUi = {
    filter, sort, now: Math.floor(Date.now() / 1000), localRefresh: localRefreshBusy,
    progress, credentialBusy: credentialConfirmBusy, adapterPool,
  };

  // 页头动作分发：刷新（纯本地）与批量刷新凭据在本页执行，其余动作原样透出给后续票。
  const handleHeaderAction = useCallback((actionId: string) => {
    if (actionId === "") return;
    if (actionId === refreshLocalActionId) { handleRefreshLocal(); return; }
    if (actionId === credential?.actionId) { credentialRefresh.request(); return; }
    onHeaderAction?.(actionId);
  }, [credential?.actionId, credentialRefresh, handleRefreshLocal, onHeaderAction, refreshLocalActionId]);

  const composed: AccountsSnapshot | null = shown === null || adapter === undefined
    ? null
    : adapter.compose((shown as unknown as Record<string, unknown>)[adapter.payloadKey], ui);

  // 视图模型：有快照即查表产出；未登记产品走 registry 的 fail-closed 空视图（不猜数据面）。
  const view: AccountsView | null = composed !== null
    ? accountsViewFor(composed)
    : adapter === undefined ? accountsViewFor({ productId } as unknown as AccountsSnapshot) : null;

  // 拿不到快照（首次加载中 / 已清态 / 首次读取失败）：只渲染外壳，读取失败给错误卡
  // （带重试），绝不用空账号冒充读取失败。
  if (view === null) return (
    <AccountsSkeleton productId={productId} error={error} onRetry={handleRefreshLocal} retrying={loading} />
  );

  const { filters } = view;
  // 遗留档位若不在本产品选项内即回退首档，select 不出现无效值（旧控件同口径）。
  const sortValue = filters === null || filters.sortOptions.some((option) => option.key === filters.sort)
    ? filters?.sort ?? "" : filters.sortOptions[0]?.key ?? "";
  // 行锚点与「使用中」锚点由产品接缝补（builder 视图模型无 testid 字段）；视觉仍只有 accent 徽章。
  const rows: readonly AccountsRowView[] = composed === null || adapter === undefined
    ? []
    : view.rows.map((row) => {
      const ids = adapter.testIds(row, composed);
      return { ...row, testId: ids.row, selectedTestId: row.current ? ids.selected : undefined };
    });
  // 控件行只在「有账号」时出现（旧骨架 showControls 同口径）：无行但有过滤档 = 过滤空态，
  // 仍要给用户切回「全部」的入口；其余无行即真没有账号，不给空控件行。
  const showControls = filters !== null && (rows.length > 0 || filters.active !== "all");

  return (
    <section className="account-center" role="region" aria-label={view.header.title}>
      <AccountsHeader
        view={view.header}
        productBadge={<ProductBadge productId={productId} />}
        busy={credentialConfirmBusy ? [credential?.actionId ?? ""] : pending === null ? [] : [pending]}
        onAction={handleHeaderAction}
      />
      <section className="account-center__profiles" aria-labelledby="account-mine-heading">
        <div className="account-center__section-heading">
          <h3 id="account-mine-heading"><UserRound size={16} aria-hidden="true" />我的账号</h3>
          {showControls && filters !== null && (
            <AccountsListControls
              filters={filters}
              sortValue={sortValue}
              onFilterChange={(key) => setFilter(key as PageFilter)}
              onSortChange={(key) => setSort(key as PageSort)}
            />
          )}
          {/* 计数口径 = 账号总数（builder 的 totalCount），不是本页可见行数。 */}
          <span className="section-caption">{view.totalCount} 个</span>
        </div>

        {rows.length > 0 ? (
          <AccountsRows
            rows={rows}
            listTestId={adapter?.listTestId}
            onRowAction={(rowId, actionId) => onRowAction?.(rowId, actionId)}
            onOpenDetail={(rowId) => onOpenDetail?.(rowId)}
          />
        ) : (
          // 空态：文案与锚点来自 builder；actionLabel=null 即不给动作（过滤空态不冒充「添加账号」）。
          <EmptyState
            className={view.empty.testId}
            testId={view.empty.testId}
            description={view.empty.description}
            action={view.empty.actionLabel === null ? undefined : (
              <button className="btn btn--primary" type="button" onClick={() => handleHeaderAction(addActionIdOf(view))}>
                <UserPlus size={15} aria-hidden="true" />
                {view.empty.actionLabel}
              </button>
            )}
          />
        )}
      </section>
      {/* 批量刷新凭据二次确认：仅入口存在且有目标时挂载（挂载即打开，关闭即卸载）。 */}
      {credentialRefresh.open && credential !== undefined && (
        <CredentialRefreshDialog
          count={view.totalCount}
          busy={credentialConfirmBusy}
          onCancel={credentialRefresh.cancel}
          onConfirm={() => void credentialRefresh.confirm()}
        />
      )}
    </section>
  );
}