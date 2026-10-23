import { useCallback, useRef, useState } from "react";
import { EmptyState } from "../../components/shared/EmptyState";
import { ProductBadge } from "../../components/shared/ProductBadge";
import {
  captureTraeCnAccount, cleanupMasterEmptyProjects, closeEnvironmentInstance, createEnvironment, deleteEnvironment,
  incorporateMasterRecords, launchEnvironment, launchMasterLibrary, loginEnvironment, readEnvironmentDeletePreview,
  readMasterCheckup, renameEnvironment, repairMasterCurrentAccount,
} from "../../platform/environment";
import { readManagedAccountState } from "../../platform/accounts";
import { ENV_TEXTS, type EnvironmentView } from "./config";
import { deriveCheckupView } from "./traeEnvironment";
import { environmentViewFor, type EnvironmentSnapshot } from "./registry";
import { useEnvironmentBusy } from "./components/useEnvironmentBusy";
import { useEnvironmentSnapshot } from "./components/useEnvironmentSnapshot";
import { MASTER_ENV_ID, useEnvironmentActions, type CaptureNotice } from "./components/useEnvironmentActions";
import { EnvironmentPageBody, EnvironmentPageFrame } from "./components/EnvironmentPageShell";
import { EnvCard, type EnvCardAnchors } from "./components/EnvironmentCards";
import { EnvironmentCheckup } from "./components/EnvironmentCheckup";
import { EnvironmentCapture } from "./components/EnvironmentCapture";
import { useEnvironmentDialogs } from "./components/useEnvironmentDialogs";
import { useEnvironmentIncorporation } from "./components/useEnvironmentIncorporation";
import { useEnvironmentLogin } from "./components/useEnvironmentLogin";
import { ASSEMBLERS, busyActionIdsOf, wbExtrasOf, type PageUi } from "./components/assembleEnvironment";
import { environmentActionDispatch } from "./components/environmentActionDispatch";

/**
 * 环境页装配（P4d-8a-1 读取渲染 + 8a-2 动作接线 + 8a-2b 弹层 + 8a-2c 收编/清理/登录收尾）。
 * 数据面 useEnvironmentSnapshot、忙碌位 useEnvironmentBusy、动作编排 useEnvironmentActions、
 * 生命周期弹层 useEnvironmentDialogs、收编清理 useEnvironmentIncorporation、登录 useEnvironmentLogin、
 * 装配表 assembleEnvironment、视图模型 registry.environmentViewFor。
 * 本文件零 productId 相等判断、零 invoke(、零命令名：命令只在四张 COMMANDS 表里转接 platform 适配层。
 */

/** 首读占位文案：旧 TRAE 页无首读占位、WB 页文案带「客户端」字样，共用骨架不能取任一产品口径。 */
const LOADING_COPY = "正在读取环境状态…";

/** 收编 / 清理命令（8a-2c）：收编一条单事务、清理预览与执行分开，绝不合并（旧页 L304 / L380）。 */
const INCORPORATE_CLEANUP_COMMANDS = {
  incorporate: (envId: string) => incorporateMasterRecords(envId === MASTER_ENV_ID ? {} : { libraryId: envId }),
  // 预览只读：清理范围是全库空项目（旧页 L796 的 emptyProjectTotal），故读主库体检的空项目合计。
  cleanupPreview: () => readMasterCheckup().then((report) => ({
    count: report.status === "ready" ? deriveCheckupView(report).emptyProjectTotal : 0,
  })),
  cleanup: () => cleanupMasterEmptyProjects(),
} as const;

/** 登录命令（8a-2c）：账号列表只读 + 登录到环境（旧页 L1296 / L1314）。 */
const LOGIN_COMMANDS = {
  listAccounts: () => readManagedAccountState(),
  login: (envId: string, profileId: string) => loginEnvironment({ envId, profileId }),
} as const;

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
  /** 副环境详情回调（env-detail-<id> 携带 envId；缺省时该动作原样透出到 onAction）。 */
  readonly onOpenEnvDetail?: (envId: string) => void;
  /** 其余动作（WB 深度检测等未接线项）：原样透出。 */
  readonly onAction?: (actionId: string) => void;
}

/** 骨架页头（无刷新动作）：与实页共用 Frame，故标题节点跨态复用、键盘焦点不丢。 */
const SKELETON_HEADER: EnvironmentView["header"] = {
  title: ENV_TEXTS.pageTitle, regionLabel: ENV_TEXTS.regionLabel, pageTitleAttr: "environment",
  badgeProductId: null, refresh: null,
};
/** 骨架态无动作可分发（无刷新入口），给空实现以满足 Frame 的必填出口。 */
const NO_ACTION = () => undefined;

/**
 * 首读正文：一行状态 / 错误卡（带重试），绝不用空环境冒充读取失败。
 * 只替换 Frame 的**正文**——Frame 与页头由页面唯一 return 持有，故「首读中 →
 * 读到数据」不会重建 section / h1（导航移焦契约，App.test 锁定）。
 */
function EnvironmentSkeletonBody({ error, onRetry, retrying }: {
  readonly error: string | null;
  /** 缺省 = 本产品无读取入口（未登记产品），重试按钮禁用而不是假装能读。 */
  readonly onRetry?: () => void; readonly retrying: boolean;
}) {
  return (
    <div data-testid="environment-skeleton">
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
  );
}

export function EnvironmentPage({
  active, productId, onOpenMasterDetail, onOpenLibraryDetail, onOpenEnvDetail, onAction,
}: EnvironmentPageProps) {
  const { snapshot: data, loading, error, refreshLocal, refreshFull } = useEnvironmentSnapshot(productId, active);
  const { busy, incorporateStage, patchBusy } = useEnvironmentBusy(productId, active);
  const [pending, setPending] = useState(false); // 非空即本地刷新在跑（刷新按钮转圈 + 防连点）
  const [captureNotice, setCaptureNotice] = useState<CaptureNotice | null>(null);
  // 动作 hook：命令经 ACTION_COMMANDS 注入，忙碌位就地写回 useEnvironmentBusy（单一状态真源）。
  const { launchEnv, closeEnv, repairMaster, captureAccount } = useEnvironmentActions({
    commands: ACTION_COMMANDS, notifyPrefix: "", patchBusy,
  });
  // 生命周期弹层（8a-2b）：关闭确认 + 创建/改名/删除；成功后整组重读（旧页 load()）。
  // 关闭走 useEnvironmentActions（三态回执文案 + 关闭忙碌位），确认层只负责先弹一次；
  // 关闭成功即整组重读（旧页 L251 load()，8a-2c 口径对齐）。
  const { dialogs, openCreate, openRename, openDelete, openClose } = useEnvironmentDialogs({
    commands: LIFECYCLE_COMMANDS, refresh: refreshFull,
    confirmClose: async (target) => { if (await closeEnv(target.envId, target.name)) await refreshFull(); },
  });
  // 收编 / 清理（8a-2c）：确认态 + 四阶段进度 + 清理先预览再确认；成功后整组重读（旧页 L311 / L385）。
  const incorporation = useEnvironmentIncorporation({
    commands: INCORPORATE_CLEANUP_COMMANDS, notifyPrefix: "", patchBusy,
    stage: incorporateStage, refresh: refreshFull, cleaningUp: busy.cleaningUp,
  });
  // 登录账号（8a-2c）：选已有账号登录到环境；「完成」才整组重读（旧页 L850-856）。
  const login = useEnvironmentLogin({ commands: LOGIN_COMMANDS, refresh: refreshFull });
  // 刷新是读取不是动作：页面自持（与账号页 handleRefreshLocal 同法）。
  const refresh = useCallback(() => {
    if (pending) return;
    setPending(true);
    void refreshLocal().finally(() => setPending(false));
  }, [pending, refreshLocal]);

  const emit = (actionId: string) => onAction?.(actionId);
  // 装配表在 handleAction 之前就要拿到动作出口（副环境体检块由装配表产出）：
  // 经 ref 转接，先填「原样透出」，算好 handleAction 后再换成完整分发（子组件渲染在本体之后）。
  const dispatchRef = useRef<(actionId: string) => void>(emit);
  dispatchRef.current = emit;
  const busyIds = busyActionIdsOf(busy);
  const ui: PageUi = {
    busyState: busy, busyIds, loading, error, captureNotice,
    canOpenLibraryDetail: onOpenLibraryDetail !== undefined, onAction: (actionId: string) => dispatchRef.current(actionId),
  };
  const registered = ASSEMBLERS[productId];
  // 未登记产品 fail-closed：不猜数据面，也不渲染「没有环境」的空壳（拿 registry 空视图的失败说明）。
  const unregistered = registered === undefined;
  const assembled = registered === undefined || data === null ? null : registered(data, ui);

  // 拿不到数据面（首读中 / 已清态 / 首读失败 / 未登记产品）：页头 + 状态/错误卡。
  // 关键：这里与实页共用同一个 return 的同一个 Frame，只换正文——section 与 h1
  // 不重建，导航移焦（App 的 data-page-title 聚焦）不会被首读打断。
  if (assembled === null) {
    const failure = unregistered
      ? environmentViewFor({ productId } as unknown as EnvironmentSnapshot).error?.description ?? ENV_TEXTS.loadFailure
      : error;
    // 根节点与实页同为 Fragment（实页另有弹层槽），Frame 才是两者共同的 children[0]：
    // React 据此复用 section / h1，而不是整棵重建。
    return (
      <>
        <EnvironmentPageFrame
          header={SKELETON_HEADER}
          productBadge={<ProductBadge productId={productId} />}
          onAction={NO_ACTION}
        >
          {/* 未登记产品无读取入口：不给重试（禁用而不是假装能读）。 */}
          <EnvironmentSkeletonBody error={failure} onRetry={unregistered ? undefined : refresh} retrying={pending} />
        </EnvironmentPageFrame>
      </>
    );
  }
  const view: EnvironmentView = environmentViewFor(assembled.snapshot);
  const refreshId = view.header.refresh?.id ?? "";

  // 动作分发（旧页口径，见 environmentActionDispatch）：启动/页头刷新=轻刷新，其余一律整组重读。
  const handleAction = environmentActionDispatch({
    view, assembled, refreshId, refresh, refreshLocal, refreshFull, emit, onOpenMasterDetail, onOpenEnvDetail,
    launchEnv, repairMaster, captureAccount, setCaptureNotice,
    dialogs: { openCreate, openRename, openDelete, openClose }, incorporation, login,
  });
  // 装配表产出的区块（副环境体检）自此走完整分发，不再原样透出。
  dispatchRef.current = handleAction;

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
      {/* Frame 与骨架态同一个组件：h1 节点跨「首读中 → 读到数据」复用，导航移焦不丢。 */}
      <EnvironmentPageFrame
        header={view.header}
        productBadge={<ProductBadge productId={productId} />}
        busy={pending && refreshId !== "" ? [...busyIds, refreshId] : busyIds}
        onAction={handleAction}
      >
        <EnvironmentPageBody
          view={view}
          busy={pending && refreshId !== "" ? [...busyIds, refreshId] : busyIds}
          masterExtra={masterExtra}
          afterMaster={afterMaster}
          masterAnchors={masterAnchors}
          masterLoginState={assembled.masterLoginState}
          secondaryCardExtra={assembled.secondaryCardExtra}
          secondaryLoginStateOf={assembled.secondaryLoginStateOf}
          onAction={handleAction}
        />
      </EnvironmentPageFrame>
      {/* 弹层槽：生命周期（关闭/创建/改名/删除）+ 收编/清理 + 登录；无打开项时为 null 片段。 */}
      {dialogs}
      {incorporation.dialogs}
      {login.dialog}
    </>
  );
}
