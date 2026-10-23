import type { EnvironmentView } from "../config";
import { ENV_TEXTS } from "../config";
import type { CaptureNotice } from "./useEnvironmentActions";
import type { EnvironmentDialogsResult } from "./useEnvironmentDialogs";
import type { EnvironmentIncorporationResult } from "./useEnvironmentIncorporation";
import type { EnvironmentLoginResult } from "./useEnvironmentLogin";
import { MASTER_ENV_ID } from "./useEnvironmentActions";
import { cardNameOf, type Assembled } from "./assembleEnvironment";
import {
  CAPTURE_ACTION, CLEANUP_ACTION, CLOSE_PREFIX, CREATE_ACTION, DELETE_PREFIX, DETAIL_PREFIX, INCORPORATE_ACTION,
  INCORPORATE_PREFIX, LAUNCH_PREFIX, LOGIN_PREFIX, MASTER_CLOSE_ACTION, MASTER_DETAIL_ACTION, MASTER_LAUNCH_ACTION,
  RENAME_PREFIX, REPAIR_ACTION,
} from "./environmentActionIds";

/**
 * 环境页动作分发（P4d-8a-2c 从 EnvironmentPage 抽出，逐字搬移 + 补收编/清理/登录/详情四组）：
 * 一个 actionId → 一次编排调用。本文件零 invoke、零命令名、零产品判断（命令都在页面的 COMMANDS 表里）；
 * 刷新口径逐字照旧页 load()：启动/页头刷新=轻刷新，修复/捕获/创建/改名/删除/关闭/收编/清理/登录=整组重读。
 * 注意：本函数是**纯工厂不是 hook**——页面在「数据面未就绪」的早返回之后才调用它，
 * 用 hook 会违反 hooks 调用顺序（条件调用），故此处不持任何 React 状态。
 */

/** 分发所需的页面侧能力（全部由页面注入；本文件不持有任何状态）。 */
export interface ActionDispatchInput {
  /** 当前视图（取卡片显示名，绝不拿 env_id 当文案）。 */
  readonly view: EnvironmentView;
  /** 装配结果（收编规模与目标账号名按 env_id 取）。 */
  readonly assembled: Assembled;
  /** 页头刷新动作 id（builder 产出；空串 = 无刷新入口）。 */
  readonly refreshId: string;
  readonly refresh: () => void;
  readonly refreshLocal: () => Promise<void>;
  readonly refreshFull: () => Promise<void>;
  /** 未接线的其余动作原样透出。 */
  readonly emit: (actionId: string) => void;
  readonly onOpenMasterDetail?: () => void;
  /** 副环境「查看详情」回调（env-detail-<id>）；缺省时原样透出。 */
  readonly onOpenEnvDetail?: (envId: string) => void;
  readonly launchEnv: (envId: string, name?: string) => Promise<boolean>;
  readonly repairMaster: () => Promise<boolean>;
  readonly captureAccount: () => Promise<CaptureNotice | null>;
  readonly setCaptureNotice: (notice: CaptureNotice | null) => void;
  readonly dialogs: Pick<EnvironmentDialogsResult, "openCreate" | "openRename" | "openDelete" | "openClose">;
  readonly incorporation: Pick<EnvironmentIncorporationResult, "openIncorporate" | "openCleanup">;
  readonly login: Pick<EnvironmentLoginResult, "openLogin">;
}

/** 卡片显示名：副环境缺省回退「辅助环境」（主库名由视图给，绝不显示 env_id）。 */
const secondaryNameOf = (view: EnvironmentView, envId: string): string =>
  cardNameOf(view, envId) ?? ENV_TEXTS.secondaryBadge;

export function environmentActionDispatch(input: ActionDispatchInput): (actionId: string) => void {
  const {
    view, assembled, refreshId, refresh, refreshLocal, refreshFull, emit, onOpenMasterDetail, onOpenEnvDetail,
    launchEnv, repairMaster, captureAccount, setCaptureNotice, dialogs, incorporation, login,
  } = input;

  // 动作分发（旧页口径）：启动与页头刷新只校准运行态与列表；其余动作改变列表/体检聚合，一律整组重读。
  // 关闭、创建、改名、删除、收编、清理、登录都先开层（ADR-0018 单次确认），确认后才发命令。
  return (actionId: string) => {
    if (refreshId !== "" && actionId === refreshId) { refresh(); return; }
    if (actionId === MASTER_DETAIL_ACTION) { onOpenMasterDetail?.(); return; }
    if (actionId === MASTER_LAUNCH_ACTION) {
      void launchEnv(MASTER_ENV_ID).then((ok) => { if (ok) void refreshLocal(); });
      return;
    }
    if (actionId === MASTER_CLOSE_ACTION) {
      dialogs.openClose({ envId: MASTER_ENV_ID, name: view.master?.name ?? ENV_TEXTS.masterName });
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
      dialogs.openClose({ envId: actionId.slice(CLOSE_PREFIX.length), name: secondaryNameOf(view, actionId.slice(CLOSE_PREFIX.length)) });
      return;
    }
    // 收编：主库入口与副环境逐库入口共用同一弹层（旧页 L595 / L744）。
    if (actionId === INCORPORATE_ACTION) {
      incorporation.openIncorporate({
        target: { envId: MASTER_ENV_ID, envName: view.master?.name ?? ENV_TEXTS.masterName, isMaster: true },
        scale: assembled.incorporateScaleOf(MASTER_ENV_ID), accountName: assembled.incorporateAccountNameOf(MASTER_ENV_ID),
      });
      return;
    }
    if (actionId.startsWith(INCORPORATE_PREFIX)) {
      const envId = actionId.slice(INCORPORATE_PREFIX.length);
      incorporation.openIncorporate({
        target: { envId, envName: secondaryNameOf(view, envId), isMaster: false },
        scale: assembled.incorporateScaleOf(envId), accountName: assembled.incorporateAccountNameOf(envId),
      });
      return;
    }
    // 清理空项目：先预览规模（只读），取到才开破坏性确认层（旧页 L580 只开层，预览是本票补的前置）。
    if (actionId === CLEANUP_ACTION) { incorporation.openCleanup(); return; }
    if (actionId.startsWith(LOGIN_PREFIX)) {
      const envId = actionId.slice(LOGIN_PREFIX.length);
      login.openLogin({ envId, envName: secondaryNameOf(view, envId) });
      return;
    }
    if (actionId.startsWith(DETAIL_PREFIX)) {
      const envId = actionId.slice(DETAIL_PREFIX.length);
      if (onOpenEnvDetail !== undefined) { onOpenEnvDetail(envId); return; }
      emit(actionId); return;
    }
    // 创建入口按 builder 的 disabledReason 判定（WB 多实例待实验票即置灰）：置灰时不开层，原样透出。
    if (actionId === CREATE_ACTION) {
      if (view.secondary?.createAction.disabledReason !== null) { emit(actionId); return; }
      dialogs.openCreate();
      return;
    }
    if (actionId.startsWith(RENAME_PREFIX)) {
      const envId = actionId.slice(RENAME_PREFIX.length);
      dialogs.openRename({ envId, name: secondaryNameOf(view, envId) });
      return;
    }
    if (actionId.startsWith(DELETE_PREFIX)) {
      const envId = actionId.slice(DELETE_PREFIX.length);
      dialogs.openDelete({ envId, name: secondaryNameOf(view, envId) });
      return;
    }
    emit(actionId); // 其余动作（WB 深度检测等）原样透出
  };
}