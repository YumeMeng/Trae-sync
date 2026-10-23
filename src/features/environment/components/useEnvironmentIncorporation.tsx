import { useCallback, useState, type ReactNode } from "react";
import type { MasterIncorporateProgressEvent } from "../../../types/masterLibrary";
import type { EnvironmentBusyState } from "./useEnvironmentBusy";
import {
  useIncorporateCleanup, type IncorporateCleanupCommands, type IncorporateTarget,
} from "./useIncorporateCleanup";
import { EnvironmentCleanupDialog, EnvironmentIncorporateDialog } from "./EnvironmentIncorporateDialogs";
import { EMPTY_INCORPORATE_SCALE, type IncorporateScale } from "./incorporateScale";
import type { EnvironmentReadOutcome } from "./useEnvironmentSnapshot";

/**
 * 收编 / 清理弹层编排（P4d-8a-2c）：在 useIncorporateCleanup（动作与忙碌位）之上补一层
 * 「确认态」（旧页 IncorporateSession.phase.kind === "confirm"，L1567-1569），并把两只弹层
 * 组装成节点。hook 只做状态编排与刷新回调，命令全部经 useIncorporateCleanup 注入。
 *
 * 旧实现对照 EnvironmentPage.tsx（简写 L）：主库入口 L595-606、副环境入口 L744-752、
 * 收编弹层 L763-791、清理弹层 L794-810；刷新口径为整组 load()（收编 L311、清理 L385）。
 * 确认规模与目标账号在「打开时」由页面按视图取好传入（弹层不读数据面，也不事后重算）。
 */

/** 打开收编确认层所需的展示数据（页面按当前视图取）。 */
export interface IncorporateOpenInput {
  readonly target: IncorporateTarget;
  /** 确认态规模（体检同源口径）。 */
  readonly scale: IncorporateScale;
  /** 归入目标账号显示名（null = 暂不可读，弹层降级「当前账号」）。 */
  readonly accountName: string | null;
}

export interface EnvironmentIncorporationInput {
  readonly commands: IncorporateCleanupCommands;
  /** 结果汇报前缀（TRAE 传空串即逐字沿用旧文案）。 */
  readonly notifyPrefix?: string;
  readonly patchBusy?: (next: Partial<EnvironmentBusyState>) => void;
  /** 收编进度阶段（useEnvironmentBusy 已按目标库过滤）。 */
  readonly stage: MasterIncorporateProgressEvent["stage"] | null;
  /** 成功后的整组重读（旧页 load()）；失败经返回值交回，不改报为动作失败。 */
  readonly refresh: () => Promise<EnvironmentReadOutcome>;
  /** 清理执行中位（来自 useEnvironmentBusy，用于弹层按钮禁用）。 */
  readonly cleaningUp: boolean;
}

export interface EnvironmentIncorporationResult {
  /** 两只弹层节点（无打开项时 null 片段）；挂到页面根节点即可。 */
  readonly dialogs: ReactNode;
  /** 打开收编确认层（env-incorporate-entry / env-incorporate-<id>）；忙碌中不开。 */
  readonly openIncorporate: (input: IncorporateOpenInput) => void;
  /** 清理第一步：先预览规模，取到才开破坏性确认层（env-cleanup-empty-projects-entry）。 */
  readonly openCleanup: () => void;
}

export function useEnvironmentIncorporation(input: EnvironmentIncorporationInput): EnvironmentIncorporationResult {
  const { commands, notifyPrefix, patchBusy, stage, refresh, cleaningUp } = input;
  // 刷新交给 useIncorporateCleanup 内做（旧页「命令 → load() → 回执」同一序列）：
  // 刷新失败由此处按旧文案补提示，不会把弹层完成态/成功回执吞掉。
  const cleanup = useIncorporateCleanup({ commands, notifyPrefix, patchBusy, refresh });
  // 本次会话（打开时取好的规模与目标账号）：确认态用它渲染确认句，运行/完成/失败态用它渲染
  // 目标账号与规模；关闭层才清空。与 useIncorporateCleanup 的 phase 正交（phase 管阶段，本态管数据）。
  const [session, setSession] = useState<IncorporateOpenInput | null>(null);

  // 同一时刻只允许一条收编/清理（busyRef 是唯一真源）；上一轮的完成/失败态先清掉。
  const openIncorporate = useCallback((next: IncorporateOpenInput) => {
    if (cleanup.busyRef.current.incorporatingEnvId !== null || cleanup.busyRef.current.cleaningUp) return;
    cleanup.closeIncorporate();
    setSession(next);
  }, [cleanup]);

  // 确认 → 置运行态（phase 由 hook 推进）→ 回执后整组重读（旧页 L311 的 load() 位置，在 hook 内）。
  const confirmIncorporate = useCallback(() => {
    const opened = session;
    if (opened === null) return;
    void cleanup.incorporate(opened.target);
  }, [cleanup, session]);

  // 关闭层：运行中不关（hook 的 closeIncorporate 内部直接返回，后端事务已开）；其余清会话与阶段。
  const closeIncorporate = useCallback(() => {
    if (cleanup.busyRef.current.incorporatingEnvId !== null) { cleanup.closeIncorporate(); return; }
    setSession(null);
    cleanup.closeIncorporate();
  }, [cleanup]);

  const openCleanup = useCallback(() => { void cleanup.openCleanup(); }, [cleanup]);

  const confirmCleanup = useCallback(() => { void cleanup.confirmCleanup(); }, [cleanup]);

  // 弹层目标：阶段态优先取 hook 的 phase.target（同一 envId），确认态取会话；规模与账号名恒取会话。
  const target = cleanup.phase?.target ?? session?.target ?? null;
  const dialogs = (
    <>
      {target !== null && (
        <EnvironmentIncorporateDialog
          target={target}
          phase={cleanup.phase}
          stage={stage}
          scale={session?.scale ?? EMPTY_INCORPORATE_SCALE}
          accountName={session?.accountName ?? null}
          onConfirm={confirmIncorporate}
          onClose={closeIncorporate}
        />
      )}
      {cleanup.preview !== null && (
        <EnvironmentCleanupDialog count={cleanup.preview.count} busy={cleaningUp}
          onCancel={cleanup.closeCleanup} onConfirm={confirmCleanup} />
      )}
    </>
  );

  return { dialogs, openIncorporate, openCleanup };
}
