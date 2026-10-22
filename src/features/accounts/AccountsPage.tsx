import { useCallback, useState } from "react";
import { ArrowDownAZ, UserRound, UserPlus } from "lucide-react";
import { EmptyState } from "../../components/shared/EmptyState";
import { ProductBadge } from "../../components/shared/ProductBadge";
import type { AccountRowView, AccountsView } from "./config";
import { AccountsHeader } from "./components/AccountsHeader";
import { AccountsRows, type AccountsRowView } from "./components/AccountsRows";
import { useAccountBatchProgress, type AccountBatchProgressMap } from "./components/useAccountBatchProgress";
import {
  useAccountsSnapshot, type TraeAccountsData, type WorkBuddyAccountsData,
} from "./components/useAccountsSnapshot";
import { accountsViewFor, type AccountsSnapshot } from "./registry";
import { traeRowFace, traeRowTestIds, type TraeAccountsSnapshot } from "./traeAccounts";
import { workbuddyRowTestIds } from "./workbuddyAccounts";
import { AccountsSkeleton } from "./AccountsSkeleton";

/**
 * 账号页装配（P4c-6b-1：只做「读取 + 渲染」）。产品差异全部来自 features/accounts 的
 * 声明式配置：数据面由 useAccountsSnapshot 按产品取，视图模型由 registry.accountsViewFor
 * 查表产出，本文件零 productId 相等判断、零 invoke(。
 *
 * 本票**不处理**动作语义：切号弹层 / 登录弹层 / 凭据二次确认 / 详情页分别是 6b-2、6c、7 的
 * 范围；页头只接「纯本地刷新」，其余页头动作与行内动作分别经 onHeaderAction / onRowAction
 * 原样透出给上层编排。
 */

/** 会话内过滤档（G14）：仅 TRAE 真实总览面出现，页面只持值、不判断产品。 */
type PageFilter = "all" | "attention";
/** 排序档（U-2）：WB 无「签到状态」档，builder 已裁掉该选项。 */
type PageSort = "added" | "name" | "checkin";

/** 页面自持字段：builder 是纯函数，时钟/过滤/排序/忙碌位一律由页面注入。 */
interface PageUi {
  readonly filter: PageFilter;
  readonly sort: PageSort;
  /** 判定时刻（Unix 秒）：凭据过期与登录槽求态用。 */
  readonly now: number;
  /** 纯本地刷新进行中（只重读本机缓存，不联网）。 */
  readonly localRefresh: boolean;
  /** 三路批量进度：某链有进度即该批量在跑（只用于算禁用）。 */
  readonly progress: AccountBatchProgressMap;
}

/** 单产品的页面接缝：载荷键 / 进度协议值 / 本地刷新动作 id / 列表锚点 / 快照合成 / 行锚点。 */
interface PageAdapter {
  readonly payloadKey: string;
  readonly progressId: string;
  readonly refreshLocalActionId: string;
  readonly listTestId?: string;
  readonly compose: (payload: unknown, ui: PageUi) => AccountsSnapshot;
  readonly rowTestId: (row: AccountRowView, snapshot: AccountsSnapshot) => string;
}

/** TRAE：数据面 + 页面字段 → builder 快照；批量忙碌位由进度链在跑推导。 */
const composeTrae = (payload: unknown, ui: PageUi): AccountsSnapshot => ({
  productId: "trae_cn",
  trae: {
    ...(payload as TraeAccountsData),
    filter: ui.filter,
    sort: ui.sort,
    nowUnixSeconds: ui.now,
    busy: {
      // 登录 / 切号 / 工具账号选择在 6b-2 接入，本票恒空闲。
      login: false, switching: false, toolSelection: false,
      localRefresh: ui.localRefresh,
      credits: ui.progress.credits_refresh !== undefined,
      credentialRefresh: ui.progress.credential_refresh !== undefined,
      health: ui.progress.health_check !== undefined,
    },
  },
});

/** WB：无三路进度事件（旧 WorkBuddyPanel 未订阅），忙碌位只认本地刷新。 */
const composeWorkBuddy = (payload: unknown, ui: PageUi): AccountsSnapshot => ({
  productId: "workbuddy",
  workbuddy: {
    ...(payload as WorkBuddyAccountsData),
    sort: ui.sort === "checkin" ? "added" : ui.sort,
    nowUnixSeconds: ui.now,
    busy: {
      // 续期 / 健康 / 捕获 / 切号 / 登录在 6b-2 接入，本票恒空闲。
      loading: false, renewing: false, health: false, capturing: false,
      switching: false, preview: false, login: false,
      refreshing: ui.localRefresh,
    },
  },
});

/** TRAE 行锚点：数据面（适配器面 / 真实总览 / fixture 回退）决定前缀，逐字沿用旧 testid。 */
const rowTestIdTrae = (row: AccountRowView, snapshot: AccountsSnapshot): string =>
  traeRowTestIds(traeRowFace((snapshot as { readonly trae: TraeAccountsSnapshot }).trae), row.id).row;
/** WB 行锚点只用 profile_id 前缀（积分/归档锚点归详情，本票不渲染）。 */
const rowTestIdWorkBuddy = (row: AccountRowView): string => workbuddyRowTestIds(row.id, "").row;

/** 产品接缝表（页面唯一的产品差异入口，与 registry 的 builder 表同形）；未登记即 fail-closed。 */
const PAGE_ADAPTERS: Record<string, PageAdapter> = {
  trae_cn: {
    payloadKey: "trae",
    // 账号域批量事件协议值沿用 "work_cn"（ADR-0035：链路标识，不是产品切换概念）。
    progressId: "work_cn",
    refreshLocalActionId: "account-refresh-local",
    compose: composeTrae,
    rowTestId: rowTestIdTrae,
  },
  workbuddy: {
    payloadKey: "workbuddy",
    progressId: "workbuddy",
    refreshLocalActionId: "workbuddy-refresh",
    // 列表容器锚点：沿用旧 workbuddy-pool-list（builder 视图模型暂无该字段，待 6b-2 回填）。
    listTestId: "workbuddy-pool-list",
    compose: composeWorkBuddy,
    rowTestId: rowTestIdWorkBuddy,
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
  /** 适配器面数据（旧 adapter 注入路径）：本票不消费，6b-2 接入（当前数据面 traePool 恒 null）。 */
  readonly adapterPool?: unknown;
}

export function AccountsPage({ active, productId, onOpenDetail, onHeaderAction, onRowAction }: AccountsPageProps) {
  const adapter = PAGE_ADAPTERS[productId];
  const { snapshot: data, loading, error, refreshLocal } = useAccountsSnapshot(productId, active);
  const { progress } = useAccountBatchProgress(adapter?.progressId ?? productId);
  const [filter, setFilter] = useState<PageFilter>("all");
  const [sort, setSort] = useState<PageSort>("added");
  // 正在执行的页头动作 id（非空即本地刷新在跑：页头转圈 + 禁用）。
  const [pending, setPending] = useState<string | null>(null);
  const refreshLocalActionId = adapter?.refreshLocalActionId ?? "";
  const localRefreshBusy = pending !== null;

  // 纯本地刷新：只重读本机缓存（零网络命令）；失败同样落 hook 的 error。
  const handleRefreshLocal = useCallback(() => {
    if (localRefreshBusy) return;
    setPending(refreshLocalActionId);
    void refreshLocal().finally(() => setPending(null));
  }, [localRefreshBusy, refreshLocal, refreshLocalActionId]);

  // 页头动作分发：本票只实现「刷新」（纯本地），其余动作原样透出给 6b-2。
  const handleHeaderAction = useCallback((actionId: string) => {
    if (actionId === "") return;
    if (actionId === refreshLocalActionId) { handleRefreshLocal(); return; }
    onHeaderAction?.(actionId);
  }, [handleRefreshLocal, onHeaderAction, refreshLocalActionId]);

  // 快照合成：数据面按载荷键取（表内键与载荷键一一对应），再补页面自持字段。
  const ui: PageUi = { filter, sort, now: Math.floor(Date.now() / 1000), localRefresh: localRefreshBusy, progress };
  const composed: AccountsSnapshot | null = data === null || adapter === undefined
    ? null
    : adapter.compose((data as unknown as Record<string, unknown>)[adapter.payloadKey], ui);

  // 视图模型：有快照即查表产出；未登记产品走 registry 的 fail-closed 空视图（不猜数据面）。
  const view: AccountsView | null = composed !== null
    ? accountsViewFor(composed)
    : adapter === undefined ? accountsViewFor({ productId } as unknown as AccountsSnapshot) : null;

  // 拿不到快照（加载中 / 已清态 / 读取失败）：只渲染外壳，
  // 读取失败给错误卡（带重试），绝不用空账号冒充读取失败。
  if (view === null) {
    return (
      <AccountsSkeleton
        productId={productId}
        error={error}
        onRetry={handleRefreshLocal}
        retrying={loading}
      />
    );
  }

  const { filters } = view;
  // 遗留档位若不在本产品选项内即回退首档，select 不出现无效值（旧控件同口径）。
  const sortValue = filters === null ? ""
    : filters.sortOptions.some((option) => option.key === filters.sort) ? filters.sort : filters.sortOptions[0]?.key ?? "";
  // 行锚点由产品接缝补（builder 视图模型无 testId 字段）；未登记产品无行可渲染。
  const rows: readonly AccountsRowView[] = composed === null || adapter === undefined
    ? []
    : view.rows.map((row) => ({ ...row, testId: adapter.rowTestId(row, composed) }));
  // 控件行只在「有账号」时出现（旧骨架 showControls 同口径）：无行但有过滤档 = 过滤空态，
  // 仍要给用户切回「全部」的入口；其余无行即真没有账号，不给空控件行。
  const showControls = filters !== null && (rows.length > 0 || filters.active !== "all");

  return (
    <section className="account-center" role="region" aria-label={view.header.title}>
      <AccountsHeader
        view={view.header}
        productBadge={<ProductBadge productId={productId} />}
        busy={pending === null ? [] : [pending]}
        onAction={handleHeaderAction}
      />

      <section className="account-center__profiles" aria-labelledby="account-mine-heading">
        <div className="account-center__section-heading">
          <h3 id="account-mine-heading"><UserRound size={16} aria-hidden="true" />我的账号</h3>
          {showControls && filters !== null && (
            <div className="account-center__list-controls">
              {/* 过滤档：选项、文案、计数与 testid 键全部来自 builder（testid = account-filter-<key>）。 */}
              {filters.options.length > 0 && (
                <div className="seg-control" role="group" aria-label="账号过滤">
                  {filters.options.map((option) => (
                    <button
                      key={option.key}
                      type="button"
                      className={`seg-control__item${filters.active === option.key ? " seg-control__item--active" : ""}`}
                      aria-pressed={filters.active === option.key}
                      data-testid={`account-filter-${option.key}`}
                      onClick={() => setFilter(option.key as PageFilter)}
                    >
                      {option.label}
                      {option.count !== null && option.count > 0 && <span className="seg-control__count">{option.count}</span>}
                    </button>
                  ))}
                </div>
              )}
              {/* 排序档：只渲染 builder 给本产品的选项（WB 无「签到状态」）。 */}
              <label className="account-center__sort-field">
                <ArrowDownAZ size={14} aria-hidden="true" />
                <span className="sr-only">排序方式</span>
                <select value={sortValue} data-testid="account-sort" onChange={(event) => setSort(event.target.value as PageSort)}>
                  {filters.sortOptions.map((option) => (
                    <option key={option.key} value={option.key}>{option.label}</option>
                  ))}
                </select>
              </label>
            </div>
          )}
          <span className="section-caption">{rows.length} 个</span>
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
    </section>
  );
}
