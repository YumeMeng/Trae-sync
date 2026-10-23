import { useCallback, useRef, type MutableRefObject } from "react";
import { notify } from "../../../platform/notify";
import { safeUiErrorMessage } from "../../../utils/safeUiError";
import { ENV_TEXTS } from "../config";
import type { EnvironmentBusyState } from "./useEnvironmentBusy";

/**
 * 环境页动作 hook（P4d-6a）：启停 / 关闭 / 修复 / 捕获四条动作的执行编排与结果汇报。
 * 命令由调用方注入（不 import platform 命令、不 import 任何产品适配器），产品差异留在页面装配层；
 * 本 hook 只做防连点、忙碌位落地、按旧文案走 notify。旧实现对照 EnvironmentPage.tsx（简写 L）：
 * 启动 L214-238、关闭 L243-266、修复 L153-164、捕获 L341-373、统一反馈 L200-211。
 * 成功后不自行重读数据：调用方据返回的布尔值调 refreshLocal（读数据归 useEnvironmentSnapshot）。
 */

/** 主库环境标识（与 builder / 忙碌位同口径）：动作目标键，文案据此取主库专用措辞。 */
export const MASTER_ENV_ID = "master";

/** 注入命令：envId 恒为主库 "master" 或副环境 env_id；回执只声明本 hook 消费的字段。 */
export interface EnvironmentActionCommands {
  /** 启动/聚焦实例（主库与副环境同一入口，由装配层按 envId 转接两条后端命令）。 */
  readonly launch: (envId: string) => Promise<unknown>;
  /** 关闭实例（主库/副环境通用）；回执 outcome 决定提示语气（未运行 = info）。 */
  readonly closeInstance: (envId: string) => Promise<{ readonly outcome: string }>;
  /** 按实际登录账号纠正主库工具记录（只改缓存，不动 TRAE 登录数据）。 */
  readonly repair: () => Promise<unknown>;
  /** 从客户端登录态捕获账号入池（错误码原样抛出，由本 hook 映射文案）。 */
  readonly capture: () => Promise<{ readonly profile_id: string; readonly screen_name: string }>;
}

/** 本 hook 负责的忙碌位：键名与 useEnvironmentBusy 完全一致，故用 Pick 保持单一口径。 */
export type EnvActionBusy = Pick<
  EnvironmentBusyState,
  "launching" | "launchingEnvId" | "closingEnvId" | "repairing" | "capturing"
>;

export interface EnvironmentActionsInput {
  readonly commands: EnvironmentActionCommands;
  /** 结果汇报前缀（TRAE 传空串即逐字沿用旧文案；错误原因按旧口径不带前缀）。 */
  readonly notifyPrefix: string;
  /**
   * 忙碌位落地面（通常传 useEnvironmentBusy 的 patchBusy）：注入后忙碌位只有一处状态真源；
   * 不注入时 busyRef 仍独立防连点（但界面不会因此重渲染）。
   */
  readonly patchBusy?: (next: Partial<EnvironmentBusyState>) => void;
}

/** 捕获结果提示（旧页内联提示同形；notify 之外保留结构化结果，供调用方渲染内联锚点）。 */
export interface CaptureNotice {
  readonly kind: "ok" | "error";
  readonly text: string;
}

export interface EnvironmentActionsResult {
  /** 防连点唯一真源：同步写入，同一 tick 内重复点击也拦得住（state 不参与判定）。 */
  readonly busyRef: MutableRefObject<EnvActionBusy>;
  /** 启动/聚焦；返回 true = 命令成功（调用方据此 refreshLocal）。name 缺省时主库取「主库」。 */
  readonly launchEnv: (envId: string, name?: string) => Promise<boolean>;
  /** 关闭实例；返回 true = 回执已到（含未运行幂等，调用方同样刷新运行态）。 */
  readonly closeEnv: (envId: string, name?: string) => Promise<boolean>;
  /** 纠正主库记录；返回 true = 已纠正（调用方重读体检）。 */
  readonly repairMaster: () => Promise<boolean>;
  /** 捕获账号；返回 null = 忙碌未执行，否则为内联提示内容（ok 时调用方刷新）。 */
  readonly captureAccount: () => Promise<CaptureNotice | null>;
}

const IDLE: EnvActionBusy = {
  launching: false, launchingEnvId: null, closingEnvId: null, repairing: false, capturing: false,
};

/** 文案占位符填充：缺键保留原样，不吞占位符（与 builder 同口径）。 */
const fill = (template: string, values: Readonly<Record<string, string>>): string =>
  template.replace(/\{(\w+)\}/g, (match, key: string) => (key in values ? values[key] : match));

export function useEnvironmentActions(input: EnvironmentActionsInput): EnvironmentActionsResult {
  // 命令与文案前缀经 ref 取最新值：四个动作身份稳定，不因页面重建入参对象而换引用。
  const latest = useRef(input);
  latest.current = input;
  // 防连点唯一真源：先写 ref 再发命令，失败与成功都在 finally 收位。
  const busyRef = useRef<EnvActionBusy>({ ...IDLE });
  const mark = useCallback((next: Partial<EnvActionBusy>) => {
    busyRef.current = { ...busyRef.current, ...next };
    latest.current.patchBusy?.(next); // 同一 tick 内落地：首个按钮禁用态不晚于重渲染
  }, []);
  /** 结果汇报统一加前缀（前缀为空即逐字旧文案）；失败原因不带前缀。 */
  const say = useCallback((text: string) => `${latest.current.notifyPrefix}${text}`, []);
  /** 目标显示名：调用方传名优先；主库回退「主库」，副环境回退「辅助环境」（绝不把 env_id 显示给用户）。 */
  const labelOf = useCallback((envId: string, name?: string) =>
    name?.trim() || (envId === MASTER_ENV_ID ? ENV_TEXTS.masterName : ENV_TEXTS.secondaryBadge), []);

  // 启动/聚焦（L214-238）：主库置 launching，副环境置 launchingEnvId（builder 按位取禁用与执行中文案）。
  const launchEnv = useCallback(async (envId: string, name?: string): Promise<boolean> => {
    if (busyRef.current.launching || busyRef.current.launchingEnvId !== null) return false;
    const isMaster = envId === MASTER_ENV_ID;
    mark(isMaster ? { launching: true } : { launchingEnvId: envId });
    try {
      await latest.current.commands.launch(envId);
      notify.success(say(isMaster ? ENV_TEXTS.launchMasterSuccess : fill(ENV_TEXTS.launchEnvSuccess, { name: labelOf(envId, name) })));
      return true;
    } catch (reason: unknown) {
      notify.error(safeUiErrorMessage(reason, ENV_TEXTS.actionFailure)); // 失败不静默（L207）
      return false;
    } finally {
      mark(isMaster ? { launching: false } : { launchingEnvId: null });
    }
  }, [labelOf, mark, say]);

  // 关闭实例（L243-266）：三态回执逐字——未运行 = info；强制结束 = 成功但说明兜底；其余按已关闭。
  const closeEnv = useCallback(async (envId: string, name?: string): Promise<boolean> => {
    if (busyRef.current.closingEnvId !== null) return false;
    mark({ closingEnvId: envId });
    const label = labelOf(envId, name);
    try {
      const receipt = await latest.current.commands.closeInstance(envId);
      if (receipt.outcome === "not_running") notify.info(say(fill(ENV_TEXTS.closeOutcomeNotRunning, { name: label })));
      else if (receipt.outcome === "force_closed") notify.success(say(fill(ENV_TEXTS.closeOutcomeForce, { name: label })));
      else notify.success(say(fill(ENV_TEXTS.closeOutcomeClosed, { name: label })));
      return true;
    } catch (reason: unknown) {
      notify.error(safeUiErrorMessage(reason, ENV_TEXTS.actionFailure)); // L260 同一兜底句
      return false;
    } finally {
      mark({ closingEnvId: null });
    }
  }, [labelOf, mark, say]);

  // 纠正主库记录（L153-164）：只在用户明确点击后执行，失败按主库专用兜底句。
  const repairMaster = useCallback(async (): Promise<boolean> => {
    if (busyRef.current.repairing) return false;
    mark({ repairing: true });
    try {
      await latest.current.commands.repair();
      notify.success(say(ENV_TEXTS.repairSuccess));
      return true;
    } catch (reason: unknown) {
      notify.error(safeUiErrorMessage(reason, ENV_TEXTS.repairFailure));
      return false;
    } finally {
      mark({ repairing: false });
    }
  }, [mark, say]);

  // 捕获账号（L341-373）：四个错误码前缀逐字映射（后端 account_capture.rs L47-51 同码）；
  // 其余原因按旧模板带出（既有行为，逐字保留）。
  const captureAccount = useCallback(async (): Promise<CaptureNotice | null> => {
    if (busyRef.current.capturing) return null;
    mark({ capturing: true });
    try {
      const receipt = await latest.current.commands.capture();
      const notice: CaptureNotice = { kind: "ok",
        text: fill(ENV_TEXTS.captureSuccess, { name: receipt.screen_name, profileId: receipt.profile_id }) };
      notify.success(say(notice.text));
      return notice;
    } catch (reason: unknown) {
      const raw = typeof reason === "string" ? reason : String(reason);
      const notice: CaptureNotice = { kind: "error", text: raw.startsWith("capture_account_already_registered") ? ENV_TEXTS.captureAlreadyRegistered
        : raw.startsWith("capture_blob_missing") ? ENV_TEXTS.captureBlobMissing
          : raw.startsWith("capture_identity_review_failed") ? ENV_TEXTS.captureReviewFailed
            : fill(ENV_TEXTS.captureFailure, { reason: raw }) };
      notify.error(notice.text); // 失败原因按旧口径不加前缀（逐字旧文案）
      return notice;
    } finally {
      mark({ capturing: false });
    }
  }, [mark, say]);

  return { busyRef, launchEnv, closeEnv, repairMaster, captureAccount };
}
