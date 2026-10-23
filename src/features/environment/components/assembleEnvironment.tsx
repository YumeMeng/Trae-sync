import type { ReactNode } from "react";
import type { BadgeTone } from "../../../ui/Badge";
import type { TraeInstanceLoginState } from "../../../types/account_switch";
import type { WorkBuddyDeepHealthDto } from "../../../types/product";
import type { EnvCardView, EnvironmentView } from "../config";
import type { EnvironmentSnapshot } from "../registry";
import { traeSecondaryCheckupView } from "../traeEnvironment";
import { WB_ENV_TEST_IDS, type WorkBuddyEnvironmentView } from "../workbuddyEnvironment";
import {
  CAPTURE_ACTION, CLEANUP_ACTION, CLOSE_PREFIX, INCORPORATE_ACTION, INCORPORATE_PREFIX, LAUNCH_PREFIX,
  MASTER_CLOSE_ACTION, MASTER_LAUNCH_ACTION, REPAIR_ACTION,
} from "./environmentActionIds";
import { EMPTY_INCORPORATE_SCALE, scaleOfCheckup, type IncorporateScale } from "./incorporateScale";
import { MASTER_ENV_ID } from "./useEnvironmentActions";
import type { EnvironmentBusyState } from "./useEnvironmentBusy";
import type { EnvironmentData } from "./useEnvironmentSnapshot";
import type { CaptureNotice } from "./useEnvironmentActions";
import { EnvironmentCheckup } from "./EnvironmentCheckup";

/**
 * 环境页装配表（P4d-8a-2b 从 EnvironmentPage 抽出，逐字搬移）：产品快照 → 合成快照 +
 * 视图模型之外的登录子态与逐卡槽位。键 = productId，页面只查表、不做产品判断。
 * 本文件零命令、零 invoke、零 productId 相等判断（表键就是唯一产品差异入口）。
 */

/** 页面自持字段（合成快照时补进 builder 载荷的那部分）。 */
export interface PageUi {
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
export interface Assembled {
  readonly snapshot: EnvironmentSnapshot;
  readonly masterLoginState?: TraeInstanceLoginState;
  readonly secondaryLoginStateOf?: (card: EnvCardView) => TraeInstanceLoginState | undefined;
  readonly secondaryCardExtra?: (card: EnvCardView) => ReactNode;
  /** 只读状态卡色档（视图模型无色档字段，WB 运行态徽章按旧页三色给）。 */
  readonly masterStatusTone?: BadgeTone;
  /** 收编确认态规模（体检同源口径，按 envId 取；P4d-8a-2c）。 */
  readonly incorporateScaleOf: (envId: string) => IncorporateScale;
  /** 归入目标账号显示名（null = 暂不可读，弹层降级「当前账号」）。 */
  readonly incorporateAccountNameOf: (envId: string) => string | null;
}

/** WB 运行态色档（WB L123/L170 三色）：验证通过=强调（运行中）、探针失败=失效、未运行=中性。 */
const wbToneOf = (live: WorkBuddyDeepHealthDto["live_status"] | undefined): BadgeTone =>
  live === "verified" ? "accent" : live === "probe_failed" ? "danger" : "neutral";

/** 装配表：键 = productId，值 = 数据面 → 快照（新增产品只加一行，页面不做产品判断）。 */
export const ASSEMBLERS: Record<string, (data: EnvironmentData, ui: PageUi) => Assembled> = {
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
      // 收编确认规模（旧页 L767-786 逐库取同一份体检报告）：主库取主库报告，副环境取逐库报告。
      incorporateScaleOf: (envId) => scaleOfCheckup(envId === MASTER_ENV_ID ? trae.checkup : trae.secondaryCheckups[envId] ?? null),
      incorporateAccountNameOf: (envId) => envId === MASTER_ENV_ID ? trae.state?.current_account_name ?? null
        : trae.environments?.find((env) => env.env_id === envId)?.current_account_name ?? null,
    };
  },
  workbuddy: (data, ui) => {
    const { workbuddy } = data as Extract<EnvironmentData, { readonly productId: "workbuddy" }>;
    return { // 读取态提升为 hook 返回值后须写回载荷（P4d-5 交接 §1 偏差 2），否则 loadingNote / error 恒 null。
      snapshot: { productId: "workbuddy", workbuddy: { ...workbuddy, checking: ui.busyState.checking,
        loading: ui.loading, error: ui.error } },
      masterStatusTone: wbToneOf(workbuddy.deepHealth?.live_status),
      // WB 无收编/清理能力（能力表 incorporate="cwd-merge" / cleanup=false）：恒零规模、无目标账号。
      incorporateScaleOf: () => EMPTY_INCORPORATE_SCALE, incorporateAccountNameOf: () => null,
    };
  },
};

/** 忙碌位 → 动作 id（builder 约定 testId = id；旧页按钮禁用口径 L682/L685/L1173）：只影响被点的按钮。 */
export const busyActionIdsOf = (busy: EnvironmentBusyState): readonly string[] => [
  ...(busy.launching ? [MASTER_LAUNCH_ACTION] : []),
  ...(busy.launchingEnvId !== null ? [`${LAUNCH_PREFIX}${busy.launchingEnvId}`] : []),
  ...(busy.closingEnvId === MASTER_ENV_ID ? [MASTER_CLOSE_ACTION]
    : busy.closingEnvId !== null ? [`${CLOSE_PREFIX}${busy.closingEnvId}`] : []),
  ...(busy.repairing ? [REPAIR_ACTION] : []),
  ...(busy.capturing ? [CAPTURE_ACTION] : []),
  ...(busy.checking ? [WB_ENV_TEST_IDS.deepHealthButton] : []),
  // 收编运行中禁用本库归入入口（主库 env-incorporate-entry / 副环境 env-incorporate-<id>）。
  ...(busy.incorporatingEnvId === MASTER_ENV_ID ? [INCORPORATE_ACTION]
    : busy.incorporatingEnvId !== null ? [`${INCORPORATE_PREFIX}${busy.incorporatingEnvId}`] : []),
  ...(busy.cleaningUp ? [CLEANUP_ACTION] : []),
];

/** WB 扩展块（客户端运行状态锚 + 登录域健康卡）：视图模型带 workbuddy 字段即有，不判断产品。 */
export const wbExtrasOf = (view: EnvironmentView): WorkBuddyEnvironmentView["workbuddy"] | null =>
  "workbuddy" in view ? (view as WorkBuddyEnvironmentView).workbuddy : null;

/** 动作目标显示名：按卡片 id 从视图取（视图已含主库/副环境名，页面不拿 env_id 当文案）。 */
export const cardNameOf = (view: EnvironmentView, envId: string): string | undefined =>
  view.master?.id === envId ? view.master.name
    : view.secondary?.cards.find((card) => card.id === envId)?.name;
