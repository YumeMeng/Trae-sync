import { useCallback, useState } from "react";
import { UserRound, UserPlus } from "lucide-react";
import { EmptyState } from "../../components/shared/EmptyState";
import { ProductBadge } from "../../components/shared/ProductBadge";
import { RemoteDevicesPanel } from "../../components/RemoteDevicesPanel";
import type { AccountSnapshot } from "../../platform/accountAdapter";
import type { AccountRowView, AccountsView } from "./config";
import { ACCOUNT_TEXTS } from "./config";
import { AccountsHeader } from "./components/AccountsHeader";
import { AccountsRows, type AccountsRowView } from "./components/AccountsRows";
import { AccountsListControls } from "./components/AccountsListControls";
import { AccountDetailPage } from "./components/AccountDetailPage";
import { CredentialRefreshDialog } from "./components/CredentialRefreshDialog";
import { DetailCredentialRefreshDialog } from "./components/DetailCredentialRefreshDialog";
import { useRetainedAccountsData } from "./components/useRetainedAccountsData";
import { useAccountBatchProgress } from "./components/useAccountBatchProgress";
import { useCredentialRefresh, type CredentialRefreshTarget } from "./components/useCredentialRefresh";
import { useAccountsSnapshot } from "./components/useAccountsSnapshot";
import { useHeaderBatchActions } from "./components/useHeaderBatchActions";
import { useAccountLoginFlow } from "./components/useAccountLoginFlow";
import { AccountLoginDialog } from "./components/AccountLoginDialog";
import { actionWiringFor } from "./actionCommands";
import { accountsViewFor, type AccountsSnapshot } from "./registry";
import { buildDetailView, DETAIL_CREDENTIAL_REFRESH_ACTIONS } from "./detailView";
import { detailSnapshotFor } from "./detailSnapshot";
import { PAGE_ADAPTERS, addActionIdOf, type PageUi } from "./pageAdapters";
import { AccountsSkeleton } from "./AccountsSkeleton";

/**
 * 账号页装配（P4c-6b/6c/7c-1：读取 + 渲染 + 详情接线）。产品差异全部来自声明式配置：
 * 数据面由 useAccountsSnapshot 按产品取，列表视图由 registry.accountsViewFor 查表产出，
 * 产品接缝（载荷键/命令/锚点/设备绑定）在 pageAdapters，详情视图由 detailView 的 builder 产出。
 * 本文件零 productId 相等判断、零 invoke(。
 */

/** 会话内过滤档（G14）：仅 TRAE 真实总览面出现，页面只持值、不判断产品。 */
type PageFilter = "all" | "attention";
/** 排序档（U-2）：WB 无「签到状态」档，builder 已裁掉该选项。 */
type PageSort = "added" | "name" | "checkin";

export interface AccountsPageProps {
  /** 页面可见才读取（后台页保持挂载但不发 IPC）。 */
  readonly active: boolean;
  /** 当前产品协议值：只用于查声明式配置，页面内不做任何产品判断。 */
  readonly productId: string;
  /** 行点击进详情的外部观察者（可选）：页面自身已按 selectedRowId 切到详情视图。 */
  readonly onOpenDetail?: (rowId: string) => void;
  /** 页头其余动作（健康检测 / 额度 / 凭据 / 添加账号 / 登录）：本票只透出，不实现。 */
  readonly onHeaderAction?: (actionId: string) => void;
  /** 行内动作（切号 / 工具账号选择）：本票只透出，不实现。 */
  readonly onRowAction?: (rowId: string, actionId: string) => void;
  /** 详情页动作（P4c-7c-1）：单账号凭据刷新之外的详情动作原样透出，缺省无反应。 */
  readonly onDetailAction?: (rowId: string, actionId: string) => void;
  /** 适配器面数据（旧 adapter 注入路径，仅测试传）：注入 TRAE 快照的 traePool / poolStatusLabel。 */
  readonly adapterPool?: AccountSnapshot;
}

export function AccountsPage(props: AccountsPageProps) {
  const { active, productId, onOpenDetail, onHeaderAction, onRowAction, onDetailAction, adapterPool } = props;
  const adapter = PAGE_ADAPTERS[productId];
  const { snapshot: data, loading, error, refreshLocal } = useAccountsSnapshot(productId, active);
  const { progress, reset: progressReset } = useAccountBatchProgress(adapter?.progressId ?? productId);
  const [filter, setFilter] = useState<PageFilter>("all");
  const [sort, setSort] = useState<PageSort>("added");
  // 正在执行的页头动作 id（非空即本地刷新在跑：页头转圈 + 禁用）。
  const [pending, setPending] = useState<string | null>(null);
  // 详情选择（P4c-7c-1）：非空即渲染详情视图；返回列表 = 清空（旧骨架 selectedProfileId 同法）。
  const [selectedRowId, setSelectedRowId] = useState("");
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

  // 数据面：按载荷键取（表内键与载荷键一一对应）；缺快照/未登记产品即 null。
  const payload = shown === null || adapter === undefined
    ? null : (shown as unknown as Record<string, unknown>)[adapter.payloadKey];

  // 页头批量动作（P4c-6b-2b）：命令装配按产品查表（actionCommands），三链忙碌位与结果摘要由
  // useAccountActions 承担；动作真正跑完后重读本机缓存（旧实现每个动作后 refreshOverview 同法）。
  const wiring = actionWiringFor(productId, payload);
  const batch = useHeaderBatchActions({
    wiring, targets: () => wiring?.targets(selectedRowId) ?? [], onSettled: refreshLocal, resetProgress: progressReset,
  });

  // 批量刷新凭据：入口只开二次确认弹层，确认后换发全部账号并重读缓存（旧 L680-729 同契约）。
  // 目标在确认时才解析（用最新快照），命令与入口 id 来自产品接缝表，缺省即不提供该入口。
  const credential = adapter?.credentialRefresh;
  const listCredential = useCredentialRefresh({
    refresh: credential?.refresh,
    targets: () => credential === undefined || payload === null
      ? [] : credential.targets(payload),
    onRefreshed: refreshLocal,
  });
  // 换发进行中：二次确认执行中，或进度事件链在跑（两者同源表达，任一为真即禁用页头按钮）。
  const credentialConfirmBusy = listCredential.busy || progress.credential_refresh !== undefined;

  // 添加账号 / 登录（P4c-6b-2c）：接缝登记了登录才挂弹层；成功后重读本机缓存（旧
  // refreshOverview 同法），失败/取消的三类收尾提示全在 useAccountLogin 内，页面不重复通知。
  const login = useAccountLoginFlow(adapter?.login ?? null, refreshLocal);

  // 快照合成：数据面 + 页面自持字段（时钟/过滤/排序/忙碌位/适配器面）。
  const now = Math.floor(Date.now() / 1000);
  const ui: PageUi = {
    filter, sort, now, localRefresh: localRefreshBusy, progress,
    credentialBusy: credentialConfirmBusy, creditsBusy: batch.busy.credits, healthBusy: batch.busy.health,
    loginBusy: login.busy,
    adapterPool,
  };
  const composed: AccountsSnapshot | null = payload === null || adapter === undefined
    ? null : adapter.compose(payload, ui);

  // 详情数据面（P4c-7c-1）：选择非空才装配；目标不在本产品数据面内即 null（回列表）。
  const detailSnapshot = composed === null || selectedRowId === ""
    ? null
    : detailSnapshotFor({ productId, payload, rowId: selectedRowId, nowUnixSeconds: now, adapterPool });

  // 详情单账号刷新凭据（P4c-7c-1）：同一 hook 的单账号路径，命令同源、目标只有当前行；
  // 成功回执逐字沿用旧 AccountDetail（走 successMessage 而非批量摘要）。
  const detailCredential = useCredentialRefresh({
    refresh: credential?.refresh,
    targets: (): readonly CredentialRefreshTarget[] => {
      if (detailSnapshot === null) return [];
      const rows = composed === null ? [] : accountsViewFor(composed).rows;
      const row = rows.find((item) => item.id === selectedRowId);
      return row === undefined ? [] : [{ profileId: row.id, displayName: row.displayName }];
    },
    onRefreshed: refreshLocal,
    successMessage: ACCOUNT_TEXTS.credentialRefreshed,
  });
  // 详情页忙碌位：单账号换发进行中时禁用该动作（builder 已给其它禁用原因）。
  const detailBusy = detailCredential.busy ? DETAIL_CREDENTIAL_REFRESH_ACTIONS : [];

  // 页头忙碌位（AccountsHeader 的 busy 契约：给 id 即转圈，禁用原因仍由 builder 算）：
  // 本地刷新 / 批量凭据刷新 / 额度刷新 / 健康检测各自独立表达，互不冒充。
  const headerBusy = [
    ...(pending === null ? [] : [pending]),
    ...(credentialConfirmBusy ? [credential?.actionId ?? ""] : []),
    ...(batch.busy.credits ? [wiring?.actionIds.credits ?? ""] : []),
    ...(batch.busy.health ? [wiring?.actionIds.health ?? ""] : []),
  ].filter((id) => id !== "");

  // 页头动作分发：本地刷新 / 批量凭据刷新 / 批量动作（额度、健康检测，由 actionCommands 登记）
  // 在本页执行，其余动作原样透出给后续票（登录、切号、添加账号）。
  const handleHeaderAction = useCallback((actionId: string) => {
    if (actionId === "") return;
    if (actionId === refreshLocalActionId) { handleRefreshLocal(); return; }
    if (actionId === credential?.actionId) { listCredential.request(); return; }
    if (batch.handle(actionId)) return;
    // 添加账号 → 打开登录弹层（未登记登录接缝的产品不动，动作原样透出）。
    login.handle(actionId);
    // 透出契约不丢：动作 id 仍回给外部观察者（空态主行动与页头按钮同一 id）。
    onHeaderAction?.(actionId);
  }, [batch.handle, credential?.actionId, handleRefreshLocal, listCredential, login, onHeaderAction, refreshLocalActionId]);

  // 详情动作分发（P4c-7c-1）：凭据刷新在本页执行（同一 hook 的单账号路径），其余透出。
  const handleDetailAction = useCallback((actionId: string) => {
    if (actionId === "") return;
    if (DETAIL_CREDENTIAL_REFRESH_ACTIONS.includes(actionId)) { detailCredential.request(); return; }
    onDetailAction?.(selectedRowId, actionId);
  }, [detailCredential, onDetailAction, selectedRowId]);

  // 详情视图：装配成功才渲染（行被删 / 数据面换产品即回列表，不展示陈旧详情）。
  if (detailSnapshot !== null && adapter !== undefined) {
    const view = accountsViewFor(composed as AccountsSnapshot);
    const row: AccountRowView | undefined = view.rows.find((item) => item.id === selectedRowId);
    if (row !== undefined) {
      const detailView = buildDetailView({ productId, row, snapshot: detailSnapshot });
      // 登录设备插槽（P4c-7c-1）：按接缝登记的产品绑定注入，缺省即不挂（WB devices=false）。
      const devices = adapter.devices;
      return (
        <AccountDetailPage
          view={detailView}
          busy={detailBusy}
          onBack={() => setSelectedRowId("")}
          onAction={handleDetailAction}
          devicesSlot={devices === undefined ? undefined : (
            <RemoteDevicesPanel
              productId={devices.productId}
              productName={devices.productName}
              profileId={selectedRowId}
              accountLabel={detailView.header.title}
              onSignedOut={() => void refreshLocal()}
            />
          )}
          dialogsSlot={detailCredential.open ? (
            <DetailCredentialRefreshDialog
              accountName={row.displayName}
              busy={detailCredential.busy}
              onCancel={detailCredential.cancel}
              onConfirm={() => void detailCredential.confirm()}
            />
          ) : null}
        />
      );
    }
  }

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
        busy={headerBusy}
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
            onOpenDetail={(rowId) => {
              setSelectedRowId(rowId);
              onOpenDetail?.(rowId);
            }}
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
      {/* 登录弹层（P4c-6b-2c）：接缝登记了登录才可能挂载；关闭即卸载。 */}
      {login.dialog !== null && <AccountLoginDialog {...login.dialog} />}
      {/* 批量刷新凭据二次确认：仅入口存在且有目标时挂载（挂载即打开，关闭即卸载）。 */}
      {listCredential.open && credential !== undefined && (
        <CredentialRefreshDialog
          count={view.totalCount}
          busy={credentialConfirmBusy}
          onCancel={listCredential.cancel}
          onConfirm={() => void listCredential.confirm()}
        />
      )}
    </section>
  );
}
