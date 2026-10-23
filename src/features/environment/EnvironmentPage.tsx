import { useCallback, useState, type ReactNode } from "react";
import { EmptyState } from "../../components/shared/EmptyState";
import { ProductBadge } from "../../components/shared/ProductBadge";
import type { BadgeTone } from "../../ui/Badge";
import type { TraeInstanceLoginState } from "../../types/account_switch";
import type { WorkBuddyDeepHealthDto } from "../../types/product";
import { ENV_TEXTS, type EnvCardView, type EnvironmentView } from "./config";
import { environmentViewFor, type EnvironmentSnapshot } from "./registry";
import { traeSecondaryCheckupView } from "./traeEnvironment";
import { WB_ENV_TEST_IDS, type WorkBuddyEnvironmentView } from "./workbuddyEnvironment";
import { useEnvironmentBusy, type EnvironmentBusyState } from "./components/useEnvironmentBusy";
import { useEnvironmentSnapshot, type EnvironmentData } from "./components/useEnvironmentSnapshot";
import { MASTER_ENV_ID } from "./components/useEnvironmentActions";
import { EnvironmentPageShell } from "./components/EnvironmentPageShell";
import { EnvCard, type EnvCardAnchors } from "./components/EnvironmentCards";
import { EnvironmentCheckup } from "./components/EnvironmentCheckup";

/**
 * 环境页装配（P4d-8a-1：只做读取 + 渲染；动作与弹层留 8a-2）。
 * 数据面 useEnvironmentSnapshot、忙碌位 useEnvironmentBusy、视图模型 registry.environmentViewFor；
 * 产品差异全部走声明式装配表（键 = productId，与 registry / 读取表同法）。
 * 本文件零 productId 相等判断、零 invoke(、零命令名、零弹层：动作一律经 onAction 透出。
 */

/** 首读占位文案：旧 TRAE 页无首读占位、WB 页文案带「客户端」字样，共用骨架不能取任一产品口径。 */
const LOADING_COPY = "正在读取环境状态…";

/** 页面自持字段（合成快照时补进 builder 载荷的那部分）。 */
interface PageUi {
  readonly busyState: EnvironmentBusyState;
  readonly busyIds: readonly string[];
  readonly loading: boolean;
  readonly error: string | null;
  /** 捕获提示：捕获动作归 8a-2，本票恒 null（builder 据此不渲染提示行）。 */
  readonly captureNotice: { readonly kind: "ok" | "error"; readonly text: string } | null;
  readonly canOpenLibraryDetail: boolean;
  readonly onAction: (actionId: string) => void;
}

/** 装配结果：合成快照 + 视图模型之外的登录子态与逐卡槽位（builder 不持这些字段）。 */
interface Assembled {
  readonly snapshot: EnvironmentSnapshot;
  readonly masterLoginState?: TraeInstanceLoginState;
  readonly secondaryLoginStateOf?: (card: EnvCardView) => TraeInstanceLoginState | undefined;
  readonly secondaryCardExtra?: (card: EnvCardView) => ReactNode;
  /** 只读状态卡色档（视图模型无色档字段，WB 运行态徽章按旧页三色给）。 */
  readonly masterStatusTone?: BadgeTone;
}

/** WB 运行态色档（WB L123/L170 三色）：验证通过=强调（运行中）、探针失败=失效、未运行=中性。 */
const wbToneOf = (live: WorkBuddyDeepHealthDto["live_status"] | undefined): BadgeTone =>
  live === "verified" ? "accent" : live === "probe_failed" ? "danger" : "neutral";

/** 装配表：键 = productId，值 = 数据面 → 快照（新增产品只加一行，页面不做产品判断）。 */
const ASSEMBLERS: Record<string, (data: EnvironmentData, ui: PageUi) => Assembled> = {
  trae_cn: (data, ui) => {
    const { trae } = data as Extract<EnvironmentData, { readonly productId: "trae_cn" }>;
    const envOf = (card: EnvCardView) => trae.environments?.find((env) => env.env_id === card.id) ?? null;
    return {
      snapshot: { productId: "trae_cn", trae: { ...trae, captureNotice: ui.captureNotice,
        canOpenLibraryDetail: ui.canOpenLibraryDetail, busy: { launching: ui.busyState.launching,
          launchingEnvId: ui.busyState.launchingEnvId, closingEnvId: ui.busyState.closingEnvId,
          repairing: ui.busyState.repairing, capturing: ui.busyState.capturing } } },
      // 登录子态（P4d-7a 交接 2）：徽章「运行中 · 已登录/待登录」靠它，视图模型不含。
      masterLoginState: trae.state?.login_state,
      secondaryLoginStateOf: (card) => envOf(card)?.login_state,
      // 副环境逐库体检（L1032-1093）：报告为 null / 无差异时 builder 返回 null，即不渲染区块。
      secondaryCardExtra: (card) => {
        const env = envOf(card);
        if (env === null) return null;
        const checkup = traeSecondaryCheckupView(env, trae.secondaryCheckups[env.env_id] ?? null);
        return checkup === null ? null : <EnvironmentCheckup checkup={checkup} busy={ui.busyIds} onAction={ui.onAction} />;
      },
    };
  },
  workbuddy: (data, ui) => {
    const { workbuddy } = data as Extract<EnvironmentData, { readonly productId: "workbuddy" }>;
    return { // 读取态提升为 hook 返回值后须写回载荷（P4d-5 交接 §1 偏差 2），否则 loadingNote / error 恒 null。
      snapshot: { productId: "workbuddy", workbuddy: { ...workbuddy, checking: ui.busyState.checking,
        loading: ui.loading, error: ui.error } },
      masterStatusTone: wbToneOf(workbuddy.deepHealth?.live_status),
    };
  },
};

/** 忙碌位 → 动作 id（builder 约定 testId = id；旧页按钮禁用口径 L682/L685/L1173）：只影响被点的按钮。 */
const busyActionIdsOf = (busy: EnvironmentBusyState): readonly string[] => [
  ...(busy.launching ? ["env-master-launch"] : []),
  ...(busy.launchingEnvId !== null ? [`env-launch-${busy.launchingEnvId}`] : []),
  ...(busy.closingEnvId === MASTER_ENV_ID ? ["env-master-close"]
    : busy.closingEnvId !== null ? [`env-close-${busy.closingEnvId}`] : []),
  ...(busy.repairing ? ["env-self-check-repair"] : []),
  ...(busy.capturing ? ["env-capture-account-entry"] : []),
  ...(busy.checking ? [WB_ENV_TEST_IDS.deepHealthButton] : []),
];

/** WB 扩展块（客户端运行状态锚 + 登录域健康卡）：视图模型带 workbuddy 字段即有，不判断产品。 */
const wbExtrasOf = (view: EnvironmentView): WorkBuddyEnvironmentView["workbuddy"] | null =>
  "workbuddy" in view ? (view as WorkBuddyEnvironmentView).workbuddy : null;

export interface EnvironmentPageProps {
  /** 页面可见才读取（后台页保持挂载但不发 IPC）。 */
  readonly active: boolean;
  /** 当前产品协议值：只用于查声明式装配表，页面内不做任何产品判断。 */
  readonly productId: string;
  /** 副环境「查看详情」入口是否存在（决定 builder 是否产出 env-detail-<id> 动作）。 */
  readonly onOpenLibraryDetail?: () => void;
  /** 其余动作（启停 / 详情 / 登录 / 改名 / 删除 / 纠正 / 归入 / 清理 / 检测）：本票只透出。 */
  readonly onAction?: (actionId: string) => void;
}

/** 首读骨架：页头（标题 + 产品徽标）+ 一行状态 / 错误卡（带重试），绝不用空环境冒充读取失败。 */
function EnvironmentPageSkeleton({ productId, error, onRetry, retrying }: {
  readonly productId: string; readonly error: string | null;
  /** 缺省 = 本产品无读取入口（未登记产品），重试按钮禁用而不是假装能读。 */
  readonly onRetry?: () => void; readonly retrying: boolean;
}) {
  return (
    <section className="environment-page" role="region" aria-label={ENV_TEXTS.regionLabel}>
      <header className="page-header">
        <div className="page-header__copy">
          <h1 data-page-title="environment" tabIndex={-1}>{ENV_TEXTS.pageTitle}</h1>
          <ProductBadge productId={productId} />
        </div>
      </header>
      <div className="environment-page__body" data-testid="environment-skeleton">
        {error === null ? (
          <EmptyState role="status" description={LOADING_COPY} />
        ) : (
          <EmptyState panel role="alert" testId="environment-load-error" description={error}
            action={(
              <button className="btn btn--primary" type="button" onClick={onRetry} disabled={retrying || onRetry === undefined}>
                {ENV_TEXTS.refresh}
              </button>
            )} />
        )}
      </div>
    </section>
  );
}

export function EnvironmentPage({ active, productId, onOpenLibraryDetail, onAction }: EnvironmentPageProps) {
  const { snapshot: data, loading, error, refreshLocal } = useEnvironmentSnapshot(productId, active);
  const { busy } = useEnvironmentBusy(productId, active);
  const [pending, setPending] = useState(false); // 非空即本地刷新在跑（刷新按钮转圈 + 防连点）
  // 刷新是读取不是动作：页面自持（与账号页 handleRefreshLocal 同法），其余动作一律透出。
  const refresh = useCallback(() => {
    if (pending) return;
    setPending(true);
    void refreshLocal().finally(() => setPending(false));
  }, [pending, refreshLocal]);

  const emit = (actionId: string) => onAction?.(actionId);
  const busyIds = busyActionIdsOf(busy);
  const ui: PageUi = {
    busyState: busy, busyIds, loading, error, captureNotice: null,
    canOpenLibraryDetail: onOpenLibraryDetail !== undefined, onAction: emit,
  };
  const registered = ASSEMBLERS[productId];
  // 未登记产品 fail-closed：不猜数据面，也不渲染「没有环境」的空壳（拿 registry 空视图的失败说明）。
  if (registered === undefined) {
    const empty = environmentViewFor({ productId } as unknown as EnvironmentSnapshot);
    return <EnvironmentPageSkeleton productId={productId} retrying={pending}
      error={empty.error?.description ?? ENV_TEXTS.loadFailure} />;
  }
  const assembled = data === null ? null : registered(data, ui);

  // 拿不到数据面（首读中 / 已清态 / 首读失败）：只渲染外壳，失败给错误卡（带重试）。
  if (assembled === null) {
    return <EnvironmentPageSkeleton productId={productId} error={error} onRetry={refresh} retrying={pending} />;
  }
  const view: EnvironmentView = environmentViewFor(assembled.snapshot);

  const refreshId = view.header.refresh?.id ?? "";
  // 页头刷新由页面执行（读操作）；其余动作 id 原样透出给观察者（8a-2 接线）。
  const handleAction = (actionId: string) => {
    if (refreshId !== "" && actionId === refreshId) { refresh(); return; }
    emit(actionId);
  };
  const wb = wbExtrasOf(view);
  const masterAnchors: EnvCardAnchors | undefined = wb === null ? undefined
    : { statusTestId: wb.clientRuntimeTestId, statusTone: assembled?.masterStatusTone };
  // 体检进主库卡内槽（masterExtra）；WB 登录域健康卡进主库卡后置槽（afterMaster，P4d-7a 交接 3/4）。
  const masterExtra = view.checkup === null ? null
    : <EnvironmentCheckup checkup={view.checkup} busy={busyIds} onAction={handleAction} />;
  const afterMaster = wb?.healthCard == null ? null
    : <EnvCard card={wb.healthCard} busy={busyIds} anchors={{ pathTestId: wb.deepResultTestId }}
        onAction={(_cardId: string, actionId: string) => handleAction(actionId)} />;

  return (
    <EnvironmentPageShell
      view={view}
      productBadge={<ProductBadge productId={productId} />}
      busy={pending && refreshId !== "" ? [...busyIds, refreshId] : busyIds}
      masterExtra={masterExtra}
      afterMaster={afterMaster}
      masterAnchors={masterAnchors}
      masterLoginState={assembled?.masterLoginState}
      secondaryCardExtra={assembled?.secondaryCardExtra}
      secondaryLoginStateOf={assembled?.secondaryLoginStateOf}
      onAction={handleAction}
    />
  );
}
