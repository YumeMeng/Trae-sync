import { useCallback, useState } from "react";
import { EmptyState } from "../../components/shared/EmptyState";
import { ProductBadge } from "../../components/shared/ProductBadge";
import {
  captureTraeCnAccount, closeEnvironmentInstance, createEnvironment, deleteEnvironment, launchEnvironment,
  launchMasterLibrary, readEnvironmentDeletePreview, renameEnvironment, repairMasterCurrentAccount,
} from "../../platform/environment";
import { ENV_TEXTS, type EnvironmentView } from "./config";
import { environmentViewFor, type EnvironmentSnapshot } from "./registry";
import { useEnvironmentBusy } from "./components/useEnvironmentBusy";
import { useEnvironmentSnapshot } from "./components/useEnvironmentSnapshot";
import { MASTER_ENV_ID, useEnvironmentActions, type CaptureNotice } from "./components/useEnvironmentActions";
import { EnvironmentPageShell } from "./components/EnvironmentPageShell";
import { EnvCard, type EnvCardAnchors } from "./components/EnvironmentCards";
import { EnvironmentCheckup } from "./components/EnvironmentCheckup";
import { EnvironmentCapture } from "./components/EnvironmentCapture";
import { useEnvironmentDialogs } from "./components/useEnvironmentDialogs";
import { ASSEMBLERS, busyActionIdsOf, cardNameOf, wbExtrasOf, type PageUi } from "./components/assembleEnvironment";
import {
  CAPTURE_ACTION, CLOSE_PREFIX, CREATE_ACTION, DELETE_PREFIX, LAUNCH_PREFIX, MASTER_CLOSE_ACTION,
  MASTER_DETAIL_ACTION, MASTER_LAUNCH_ACTION, RENAME_PREFIX, REPAIR_ACTION,
} from "./components/environmentActionIds";

/**
 * 环境页装配（P4d-8a-1 读取渲染 + 8a-2 动作接线 + 8a-2b 弹层）。
 * 数据面 useEnvironmentSnapshot、忙碌位 useEnvironmentBusy、动作编排 useEnvironmentActions、
 * 弹层 useEnvironmentDialogs、装配表 assembleEnvironment、视图模型 registry.environmentViewFor。
 * 本文件零 productId 相等判断、零 invoke(、零命令名：命令只在 ACTION_COMMANDS / LIFECYCLE_COMMANDS
 * 里转接 platform 适配层，产品差异全部走声明式装配表（键 = productId）。
 */

/** 首读占位文案：旧 TRAE 页无首读占位、WB 页文案带「客户端」字样，共用骨架不能取任一产品口径。 */
const LOADING_COPY = "正在读取环境状态…";

/** 生命周期命令（创建/改名/删除/删除规模预览）：本页只转接 platform 适配层，弹层在 hook 里。 */
const LIFECYCLE_COMMANDS = {
  create: (name: string) => createEnvironment({ name }),
  rename: (envId: string, name: string) => renameEnvironment({ envId, name }),
  delete: (envId: string) => deleteEnvironment({ envId }),
  preview: (envId: string) => readEnvironmentDeletePreview({ envId }),
} as const;

/** 注入给 useEnvironmentActions 的命令（主库与副环境同一入口，按 envId 转接两条后端命令）。 */
const ACTION_COMMANDS = {
  launch: (envId: string) => (envId === MASTER_ENV_ID ? launchMasterLibrary() : launchEnvironment({ envId })),
  closeInstance: (envId: string) => closeEnvironmentInstance({ envId }),
  repair: () => repairMasterCurrentAccount(),
  capture: () => captureTraeCnAccount(),
} as const;

export interface EnvironmentPageProps {
  /** 页面可见才读取（后台页保持挂载但不发 IPC）。 */
  readonly active: boolean;
  /** 当前产品协议值：只用于查声明式装配表，页面内不做任何产品判断。 */
  readonly productId: string;
  /** 主库「详情」入口（env-master-detail 走它，不再混进 onAction）。 */
  readonly onOpenMasterDetail?: () => void;
  /** 副环境「查看详情」入口是否存在（决定 builder 是否产出 env-detail-<id> 动作）。 */
  readonly onOpenLibraryDetail?: () => void;
  /** 其余动作（登录 / 详情 / 归入 / 清理 / 检测）：本票只透出。 */
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
  // 弹层 hook（P4d-8a-2b）：关闭确认 + 创建/改名/删除；生命周期动作成功后整组重读（旧页 load()）。
  // 关闭仍走 useEnvironmentActions（三态回执文案 + 关闭忙碌位），确认层只负责先弹一次。
  const { dialogs, openCreate, openRename, openDelete, openClose } = useEnvironmentDialogs({
    commands: LIFECYCLE_COMMANDS,
    refresh: refreshFull,
    // 关闭成功即轻刷新运行态（旧页 L251 load() 的等价：本票沿用 8a-2a 的 refreshLocal 口径）。
    confirmClose: async (target) => { if (await closeEnv(target.envId, target.name)) await refreshLocal(); },
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

  // 动作分发（旧页口径）：启动走轻刷新（只校准运行态与列表）；修复、捕获与三条生命周期动作
  // 必须全量重读——修复改变登录健康四项（旧页 L157）、捕获入池改变体检账号名解析（旧页 L354）、
  // 创建/改名/删除改变列表与逐库体检（旧页 L823/L839/L868 的整组 load()）。
  // 关闭一律先弹确认层（旧页 L672/L735 → closeDialogEnv），确认后才发关闭命令。
  const handleAction = (actionId: string) => {
    if (refreshId !== "" && actionId === refreshId) { refresh(); return; }
    if (actionId === MASTER_DETAIL_ACTION) { onOpenMasterDetail?.(); return; }
    if (actionId === MASTER_LAUNCH_ACTION) {
      void launchEnv(MASTER_ENV_ID).then((ok) => { if (ok) void refreshLocal(); });
      return;
    }
    if (actionId === MASTER_CLOSE_ACTION) {
      openClose({ envId: MASTER_ENV_ID, name: view.master?.name ?? ENV_TEXTS.masterName });
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
        // 捕获成功走全量重读（旧页 L354 load()）：新账号入池会改变体检账号名解析（未登记→真实名）。
        if (notice.kind === "ok") void refreshFull();
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
      openClose({ envId, name: cardNameOf(view, envId) ?? ENV_TEXTS.secondaryBadge });
      return;
    }
    // 创建入口按 builder 的 disabledReason 判定（WB 多实例待实验票即置灰）：置灰时不开层，原样透出。
    if (actionId === CREATE_ACTION) {
      if (view.secondary?.createAction.disabledReason !== null) { emit(actionId); return; }
      openCreate();
      return;
    }
    if (actionId.startsWith(RENAME_PREFIX)) {
      const envId = actionId.slice(RENAME_PREFIX.length);
      openRename({ envId, name: cardNameOf(view, envId) ?? ENV_TEXTS.secondaryBadge });
      return;
    }
    if (actionId.startsWith(DELETE_PREFIX)) {
      const envId = actionId.slice(DELETE_PREFIX.length);
      openDelete({ envId, name: cardNameOf(view, envId) ?? ENV_TEXTS.secondaryBadge });
      return;
    }
    emit(actionId); // 其余动作（登录/详情/归入/清理/检测）本票仍只透出
  };

  const wb = wbExtrasOf(view);
  const masterAnchors: EnvCardAnchors | undefined = wb === null ? undefined
    : { statusTestId: wb.clientRuntimeTestId, statusTone: assembled.masterStatusTone };
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
    <>
      <EnvironmentPageShell
        view={view}
        productBadge={<ProductBadge productId={productId} />}
        busy={pending && refreshId !== "" ? [...busyIds, refreshId] : busyIds}
        masterExtra={masterExtra}
        afterMaster={afterMaster}
        masterAnchors={masterAnchors}
        masterLoginState={assembled.masterLoginState}
        secondaryCardExtra={assembled.secondaryCardExtra}
        secondaryLoginStateOf={assembled.secondaryLoginStateOf}
        onAction={handleAction}
      />
      {/* 弹层槽（关闭确认 + 创建/改名/删除）：无打开项时为 null 片段。 */}
      {dialogs}
    </>
  );
}
