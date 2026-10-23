import { useCallback, useRef, useState, type MutableRefObject } from "react";
import { notify } from "../../../platform/notify";
import { safeUiErrorMessage } from "../../../utils/safeUiError";
import type { MasterEmptyProjectCleanupDto, MasterIncorporateResultDto } from "../../../types/masterLibrary";
import { ENV_TEXTS } from "../config";
import { incorporateStageLabels } from "../traeEnvironment";
import type { EnvironmentBusyState } from "./useEnvironmentBusy";
import type { EnvironmentReadOutcome } from "./useEnvironmentSnapshot";

/**
 * 环境页动作 hook 第二组（P4d-6b）：收编（一键归入当前账号）与空项目清理的编排与结果汇报。
 * 命令全部由调用方注入（本文件零 invoke、零产品判断、零命令名、零 JSX）；骨架照 P4d-6a 的
 * useEnvironmentActions：busyRef 防连点 + mark 落地忙碌位 + say 加通知前缀 + 文案逐字取 ENV_TEXTS。
 * 旧实现对照 EnvironmentPage.tsx（简写 L）：收编 L287-333、收编弹层关闭 L336-338、清理 L376-409。
 *
 * 收编「关实例 → 单事务 → 重启」由注入的单条 incorporate 命令承载（后端 library_archive.rs
 * L927-974 即此三拍 + 逐阶段进度事件）；本 hook 按旧顺序推进前端三拍（置运行态 → 发命令 →
 * 落完成/失败），既不拆成多次调用（会重复关实例/二次重启并改写 relaunch_outcome），也不与清理
 * 合并成一条命令。进度阶段文案唯一来源是 builder 的 incorporateStageLabels，本文件不自造。
 * 成功后返回 boolean 交调用方刷新（旧页 L311 / L385 的 load() 位置）；失败一律 notify.error 不静默。
 *
 * P4d-8b-2b（K-07 项 4 回归修复）：刷新就在本 hook 内做（旧页「命令 → load() → 回执」同一序列），
 * 且刷新失败**不覆盖成功结果**——收编把 refreshFailed 记进完成态（弹层内追加
 * ENV_TEXTS.incorporateRefreshFailed），清理把 ENV_TEXTS.refreshFailedSuffix 追加到成功回执尾部。
 */

/** 收编目标：envId 给命令；envName / isMaster 给阶段文案与失败兜底句。 */
export interface IncorporateTarget {
  readonly envId: string;
  readonly envName: string;
  readonly isMaster: boolean;
}

/** 清理预览（只读）：将删除的全库空项目数，与后端删除范围同口径（体检空项目总数）。 */
export interface CleanupPreview {
  readonly count: number;
}

/** 注入命令：收编一条、清理两条（预览与执行必须分开，绝不合并）。 */
export interface IncorporateCleanupCommands {
  /** 收编单事务（内部即「关实例 → 单事务 → 重启」）；envId 缺省语义由装配层处理。 */
  readonly incorporate: (envId: string) => Promise<MasterIncorporateResultDto>;
  /** 清理预览（只读、零副作用）：取不到规模就不许确认。 */
  readonly cleanupPreview: () => Promise<CleanupPreview>;
  /** 清理执行（破坏性、单事务删）：确认后的唯一入口。 */
  readonly cleanup: () => Promise<MasterEmptyProjectCleanupDto>;
}

/** 本 hook 负责的忙碌位：键名与 useEnvironmentBusy 完全一致，故用 Pick 保持单一口径。 */
export type IncorporateCleanupBusy = Pick<EnvironmentBusyState, "incorporatingEnvId" | "cleaningUp">;

export interface IncorporateCleanupInput {
  readonly commands: IncorporateCleanupCommands;
  /** 结果汇报前缀（TRAE 传空串即逐字沿用旧文案；失败原因按旧口径不带前缀）。 */
  readonly notifyPrefix?: string;
  /** 忙碌位落地面（通常传 useEnvironmentBusy 的 patchBusy）；不注入时 busyRef 仍独立防连点。 */
  readonly patchBusy?: (next: Partial<EnvironmentBusyState>) => void;
  /** 成功后的整组重读（旧页 L311 / L385 的 load()）；失败经返回值交回，不改报为动作失败。 */
  readonly refresh: () => Promise<EnvironmentReadOutcome>;
}

/** 收编会话态（旧页 IncorporatePhase L1546-1549 三态；refreshFailed 逐字保留旧页 done 态字段）。 */
export type IncorporatePhase =
  | { readonly kind: "running"; readonly target: IncorporateTarget }
  | { readonly kind: "done"; readonly target: IncorporateTarget; readonly receipt: MasterIncorporateResultDto;
    /** 回执已到但后续整组重读失败：完成结果照旧展示，弹层内追加「归入已完成，但环境状态刷新失败」。 */
    readonly refreshFailed: boolean }
  | { readonly kind: "failed"; readonly target: IncorporateTarget; readonly message: string };

export interface IncorporateCleanupResult {
  /** 防连点唯一真源：同步写入，同一 tick 内重复点击也拦得住（state 不参与判定）。 */
  readonly busyRef: MutableRefObject<IncorporateCleanupBusy>;
  /** 执行收编；true = 回执已到（调用方据此刷新），false = 忙碌拦截或失败（失败已 notify）。 */
  readonly incorporate: (target: IncorporateTarget) => Promise<boolean>;
  /** 清理第一步：只读取规模后开弹层；false = 忙碌 / 已打开 / 取不到规模 / 无空项目。 */
  readonly openCleanup: () => Promise<boolean>;
  /** 清理第二步：必须已有预览才执行；true = 删除已提交（调用方据此刷新）。 */
  readonly confirmCleanup: () => Promise<boolean>;
  /** 关闭清理弹层（执行中不关：后端事务已开）。 */
  readonly closeCleanup: () => void;
  /** 清理预览；非 null 即弹层打开中（与旧页 cleanupDialogOpen 同义）。 */
  readonly preview: CleanupPreview | null;
  /** 增量①：收编弹层的运行/完成/失败态（旧页 setIncorporating 的等价物）。 */
  readonly phase: IncorporatePhase | null;
  /** 增量②：收编弹层关闭（旧 L336-338）；运行中不关，完成/失败可关。 */
  readonly closeIncorporate: () => void;
  /** 增量③：四阶段文案（唯一来源 builder 的 incorporateStageLabels，本文件只转出）。 */
  readonly stages: ReturnType<typeof incorporateStageLabels>;
}

const IDLE: IncorporateCleanupBusy = { incorporatingEnvId: null, cleaningUp: false };

/** 文案占位符填充：缺键保留原样，不吞占位符（与 builder 同口径）。 */
const fill = (template: string, values: Readonly<Record<string, string | number>>): string =>
  template.replace(/\{(\w+)\}/g, (match, key: string) => (key in values ? String(values[key]) : match));

export function useIncorporateCleanup(input: IncorporateCleanupInput): IncorporateCleanupResult {
  // 命令与文案前缀经 ref 取最新值：动作身份稳定，不因页面重建入参对象而换引用。
  const latest = useRef(input);
  latest.current = input;
  const busyRef = useRef<IncorporateCleanupBusy>({ ...IDLE });
  const [phase, setPhase] = useState<IncorporatePhase | null>(null);
  const [preview, setPreview] = useState<CleanupPreview | null>(null);
  // 预览持一份 ref（同 tick 连点按真源判定）+ 取数锁（预览请求不重复发）。
  const previewRef = useRef<CleanupPreview | null>(null), previewBusyRef = useRef(false);
  const openPreview = useCallback((next: CleanupPreview | null) => {
    previewRef.current = next;
    setPreview(next);
  }, []);
  const mark = useCallback((next: Partial<IncorporateCleanupBusy>) => {
    busyRef.current = { ...busyRef.current, ...next };
    latest.current.patchBusy?.(next); // 同一 tick 内落地：首个按钮禁用态不晚于重渲染
  }, []);
  /** 结果汇报统一加前缀（前缀缺省即逐字旧文案）；失败原因不带前缀。 */
  const say = useCallback((text: string) => `${latest.current.notifyPrefix ?? ""}${text}`, []);
  // 收编与清理都要关实例再重启：任一进行中都不许开另一条（同一实例单动作纪律）。
  const busyNow = useCallback(() => busyRef.current.incorporatingEnvId !== null || busyRef.current.cleaningUp, []);

  // 收编（L287-333）：置运行态 → 单条命令（后端三拍）→ 整组重读 → 完成态 / 失败态。
  // 刷新失败不覆盖回执（旧页 L309-316 同法）：完成态照旧落地，只是记 refreshFailed 供弹层追加提示。
  const incorporate = useCallback(async (target: IncorporateTarget): Promise<boolean> => {
    if (busyNow()) return false;
    mark({ incorporatingEnvId: target.envId });
    setPhase({ kind: "running", target });
    try {
      const receipt = await latest.current.commands.incorporate(target.envId);
      const refreshed = await latest.current.refresh();
      setPhase({ kind: "done", target, receipt, refreshFailed: !refreshed.ok });
      return true;
    } catch (reason: unknown) {
      // 三个冲突/未知提交码（conflict / committed_needs_attention / commit_outcome_unknown）
      // 由 safeUiError 词表逐字映射旧文案，此处不另造。
      const message = safeUiErrorMessage(reason,
        target.isMaster ? ENV_TEXTS.incorporateFailureMaster : ENV_TEXTS.incorporateFailureSecondary);
      setPhase({ kind: "failed", target, message });
      notify.error(message); // 失败不静默（旧页只在弹层内展示，这里补全局通知）
      return false;
    } finally {
      mark({ incorporatingEnvId: null });
    }
  }, [busyNow, mark]);

  // 收编弹层关闭（L336-338）：运行中不允许——后端事务已开；完成/失败可关。
  const closeIncorporate = useCallback(() => {
    if (busyRef.current.incorporatingEnvId !== null) return;
    setPhase(null);
  }, []);

  // 清理第一步（预览，旧页无此步）：只读规模；无空项目按旧文案 info 收口，不开破坏性弹层。
  const openCleanup = useCallback(async (): Promise<boolean> => {
    if (busyNow() || previewBusyRef.current || previewRef.current !== null) return false;
    previewBusyRef.current = true;
    try {
      const next = await latest.current.commands.cleanupPreview();
      if (next.count <= 0) { notify.info(say(ENV_TEXTS.cleanupNothing)); return false; }
      openPreview(next);
      return true;
    } catch (reason: unknown) {
      notify.error(safeUiErrorMessage(reason, ENV_TEXTS.actionFailure)); // 取不到规模如实报错，不弹层
      return false;
    } finally {
      previewBusyRef.current = false;
    }
  }, [busyNow, openPreview, say]);

  // 清理第二步（L376-409）：必须已有预览；回执按删除数与重启结果据实提示。
  const confirmCleanup = useCallback(async (): Promise<boolean> => {
    if (busyNow() || previewRef.current === null) return false; // 未预览不许确认
    mark({ cleaningUp: true });
    try {
      const receipt = await latest.current.commands.cleanup();
      openPreview(null); // 旧页 L381：回执一到即关弹层
      // 刷新在回执之后、提示之前（旧页 L383-400 同序）：失败只在回执尾部追加后缀，不改报清理失败。
      const refreshed = await latest.current.refresh();
      const suffix = refreshed.ok ? "" : ENV_TEXTS.refreshFailedSuffix;
      if (receipt.deleted_projects > 0) {
        notify.success(say(fill(
          receipt.relaunch_outcome === "failed" ? ENV_TEXTS.cleanupSuccessRelaunchFailed : ENV_TEXTS.cleanupSuccess,
          { count: receipt.deleted_projects })) + suffix);
      } else {
        notify.info(say(ENV_TEXTS.cleanupNothing) + suffix);
      }
      return true;
    } catch (reason: unknown) {
      openPreview(null); // 旧页 L404：失败也关弹层，错误走全局通知（弹层无内嵌错误槽）
      notify.error(safeUiErrorMessage(reason, ENV_TEXTS.cleanupFailure));
      return false;
    } finally {
      mark({ cleaningUp: false });
    }
  }, [busyNow, mark, openPreview, say]);

  // 取消清理（L807）：执行中不关（ConfirmDialog busy 契约同法）。
  const closeCleanup = useCallback(() => {
    if (busyRef.current.cleaningUp) return;
    openPreview(null);
  }, [openPreview]);

  // 四阶段文案按当前收编目标取 builder 的同一函数（旧页 L1630 同法），本文件不自造标签。
  const stages = phase === null ? [] : incorporateStageLabels(phase.target.isMaster, phase.target.envName);

  return { busyRef, incorporate, openCleanup, confirmCleanup, closeCleanup, preview, phase, closeIncorporate, stages };
}
