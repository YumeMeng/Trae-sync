import { useCallback, useState, type ReactNode } from "react";
import { EmptyState } from "../../components/shared/EmptyState";
import { ProductBadge } from "../../components/shared/ProductBadge";
import type { BadgeTone } from "../../ui/Badge";
import type { TraeInstanceLoginState } from "../../types/account_switch";
import type { WorkBuddyDeepHealthDto } from "../../types/product";
import {
  captureTraeCnAccount, closeEnvironmentInstance, launchEnvironment, launchMasterLibrary, repairMasterCurrentAccount,
} from "../../platform/environment";
import { ENV_TEXTS, type EnvCardView, type EnvironmentView } from "./config";
import { environmentViewFor, type EnvironmentSnapshot } from "./registry";
import { traeSecondaryCheckupView } from "./traeEnvironment";
import { WB_ENV_TEST_IDS, type WorkBuddyEnvironmentView } from "./workbuddyEnvironment";
import { useEnvironmentBusy, type EnvironmentBusyState } from "./components/useEnvironmentBusy";
import { useEnvironmentSnapshot, type EnvironmentData } from "./components/useEnvironmentSnapshot";
import { MASTER_ENV_ID, useEnvironmentActions, type CaptureNotice } from "./components/useEnvironmentActions";
import { EnvironmentPageShell } from "./components/EnvironmentPageShell";
import { EnvCard, type EnvCardAnchors } from "./components/EnvironmentCards";
import { EnvironmentCheckup } from "./components/EnvironmentCheckup";
import { EnvironmentCapture } from "./components/EnvironmentCapture";

/**
 * 环境页装配（P4d-8a-1 读取渲染 + 8a-2 动作接线）。
 * 数据面 useEnvironmentSnapshot、忙碌位 useEnvironmentBusy、动作编排 useEnvironmentActions、
 * 视图模型 registry.environmentViewFor；产品差异全部走声明式装配表（键 = productId）。
 * 本文件零 productId 相等判断、零 invoke(、零命令名：命令只在 ACTION_COMMANDS 里转接 platform 适配层。
 */

/** 首读占位文案：旧 TRAE 页无首读占位、WB 页文案带「客户端」字样，共用骨架不能取任一产品口径。 */
const LOADING_COPY = "正在读取环境状态…";

/** 动作 id 常量（builder 约定 testId = id；副环境卡动作 = env-<动词>-<env_id>）。 */
const MASTER_DETAIL_ACTION = "env-master-detail";
const MASTER_LAUNCH_ACTION = "env-master-launch";
const MASTER_CLOSE_ACTION = "env-master-close";
const REPAIR_ACTION = "env-self-check-repair";
const CAPTURE_ACTION = "env-capture-account-entry";
const LAUNCH_PREFIX = "env-launch-";
const CLOSE_PREFIX = "env-close-";

/** 注入给 useEnvironmentActions 的命令（主库与副环境同一入口，按 envId 转接两条后端命令）。 */
const ACTION_COMMANDS = {
  launch: (envId: string) => (envId === MASTER_ENV_ID ? launchMasterLibrary() : launchEnvironment({ envId })),
  closeInstance: (envId: string) => closeEnvironmentInstance({ envId }),
  repair: () => repairMasterCurrentAccount(),
  capture: () => captureTraeCnAccount(),
} as const;

/** 页面自持字段（合成快照时补进 builder 载荷的那部分）。 */
interface PageUi {
  readonly busyState: EnvironmentBusyState;
  readonly busyIds: readonly string[];
  readonly loading: boolean;
  readonly error: string | null;
  /** 捕获提示：动作 hook 产出的内联结果（ok 绿/danger 红），null = 尚未捕获。 */
  readonly captureNotice: CaptureNotice | null;
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
  ...(busy.launching ? [MASTER_LAUNCH_ACTION] : []),
  ...(busy.launchingEnvId !== null ? [`${LAUNCH_PREFIX}${busy.launchingEnvId}`] : []),
  ...(busy.closingEnvId === MASTER_ENV_ID ? [MASTER_CLOSE_ACTION]
    : busy.closingEnvId !== null ? [`${CLOSE_PREFIX}${busy.closingEnvId}`] : []),
  ...(busy.repairing ? [REPAIR_ACTION] : []),
  ...(busy.capturing ? [CAPTURE_ACTION] : []),
  ...(busy.checking ? [WB_ENV_TEST_IDS.deepHealthButton] : []),
];

/** WB 扩展块（客户端运行状态锚 + 登录域健康卡）：视图模型带 workbuddy 字段即有，不判断产品。 */
const wbExtrasOf = (view: EnvironmentView): WorkBuddyEnvironmentView["workbuddy"] | null =>
  "workbuddy" in view ? (view as WorkBuddyEnvironmentView).workbuddy : null;

/** 动作目标显示名：按卡片 id 从视图取（视图已含主库/副环境名，页面不拿 env_id 当文案）。 */
const cardNameOf = (view: EnvironmentView, envId: string): string | undefined =>
  view.master?.id === envId ? view.master.name
    : view.secondary?.cards.find((card) => card.id === envId)?.name;

export interface EnvironmentPageProps {
  /** 页面可见才读取（后台页保持挂载但不发 IPC）。 */
  readonly active: boolean;
  /** 当前产品协议值：只用于查声明式装配表，页面内不做任何产品判断。 */
  readonly productId: string;
  /** 主库「详情」入口（env-master-detail 走它，不再混进 onAction）。 */
  readonly onOpenMasterDetail?: () => void;
  /** 副环境「查看详情」入口是否存在（决定 builder 是否产出 env-detail-<id> 动作）。 */
  readonly onOpenLibraryDetail?: () => void;
  /** 其余动作（详情 / 登录 / 改名 / 删除 / 归入 / 清理 / 检测）：本票只透出。 */
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

export function EnvironmentPage({ active, productId, onOpenMasterDetail, onOpenLibraryDetail, onAction }: EnvironmentPageProps) {
  const { snapshot: data, loading, error, refreshLocal, refreshFull } = useEnvironmentSnapshot(productId, active);
  const { busy, patchBusy } = useEnvironmentBusy(productId, active);
  const [pending, setPending] = useState(false); // 非空即本地刷新在跑（刷新按钮转圈 + 防连点）
  const [captureNotice, setCaptureNotice] = useState<CaptureNotice | null>(null);
  // 动作 hook：命令经 ACTION_COMMANDS 注入，忙碌位就地写回 useEnvironmentBusy（单一状态真源）。
  const { launchEnv, closeEnv, repairMaster, captureAccount } = useEnvironmentActions({
    commands: ACTION_COMMANDS, notifyPrefix: "", patchBusy,
  });
  // 刷新是读取不是动作：页面自持（与账号页 handleRefreshLocal 同法）。
  const refresh = useCallback(() => {
    if (pending) return;
    setPending(true);
    void refreshLocal().finally(() => setPending(false));
  }, [pending, refreshLocal]);

  const emit = (actionId: string) => onAction?.(actionId);
  const busyIds = busyActionIdsOf(busy);
  const ui: PageUi = {
    busyState: busy, busyIds, loading, error, captureNotice,
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

  // 动作分发（旧页口径）：启动/关闭/捕获成功走轻刷新（只校准运行态与列表）；
  // 修复必须全量重读——体检聚合（登录健康四项 + 库内容）会随记录纠正而变（旧页 L157 整组重读）。
  const handleAction = (actionId: string) => {
    if (refreshId !== "" && actionId === refreshId) { refresh(); return; }
    if (actionId === MASTER_DETAIL_ACTION) { onOpenMasterDetail?.(); return; }
    if (actionId === MASTER_LAUNCH_ACTION) {
      void launchEnv(MASTER_ENV_ID).then((ok) => { if (ok) void refreshLocal(); });
      return;
    }
    if (actionId === MASTER_CLOSE_ACTION) {
      void closeEnv(MASTER_ENV_ID).then((ok) => { if (ok) void refreshLocal(); });
      return;
    }
    if (actionId === REPAIR_ACTION) {
      void repairMaster().then((ok) => { if (ok) void refreshFull(); });
      return;
    }
    if (actionId === CAPTURE_ACTION) {
      setCaptureNotice(null); // 旧页 L342：每次点击先清上一轮结果
      void captureAccount().then((notice) => {
        if (notice === null) return; // 忙碌未执行：保留上一轮提示
        setCaptureNotice(notice);
        if (notice.kind === "ok") void refreshLocal(); // 捕获成功即刷新（失败零副作用，不刷新）
      });
      return;
    }
    if (actionId.startsWith(LAUNCH_PREFIX)) {
      const envId = actionId.slice(LAUNCH_PREFIX.length);
      void launchEnv(envId, cardNameOf(view, envId)).then((ok) => { if (ok) void refreshLocal(); });
      return;
    }
    if (actionId.startsWith(CLOSE_PREFIX)) {
      const envId = actionId.slice(CLOSE_PREFIX.length);
      void closeEnv(envId, cardNameOf(view, envId)).then((ok) => { if (ok) void refreshLocal(); });
      return;
    }
    emit(actionId); // 其余动作（登录/改名/删除/归入/清理/检测）本票仍只透出
  };

  const wb = wbExtrasOf(view);
  const masterAnchors: EnvCardAnchors | undefined = wb === null ? undefined
    : { statusTestId: wb.clientRuntimeTestId, statusTone: assembled?.masterStatusTone };
  // 体检进主库卡内槽（masterExtra），其后接账号捕获块（旧页 L471-646 同序）；
  // WB 登录域健康卡进主库卡后置槽（afterMaster，P4d-7a 交接 3/4）。
  const checkupNode = view.checkup === null ? null
    : <EnvironmentCheckup checkup={view.checkup} busy={busyIds} onAction={handleAction} />;
  const captureNode = view.capture === null ? null
    : <EnvironmentCapture testId={view.capture.testId} title={view.capture.title} entry={view.capture.entry}
        notice={view.capture.notice} busy={busyIds} onAction={handleAction} />;
  const masterExtra = checkupNode === null && captureNode === null ? null : <>{checkupNode}{captureNode}</>;
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
