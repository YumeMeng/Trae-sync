import { CheckCircle2, Combine, Trash2, TriangleAlert } from "lucide-react";
import { ConfirmDialog } from "../../../components/shared/ConfirmDialog";
import type { MasterIncorporateProgressEvent } from "../../../types/masterLibrary";
import { ENV_TEXTS, fillEnvText } from "../config";
import { incorporateStageLabels } from "../traeEnvironment";
import type { IncorporatePhase, IncorporateTarget } from "./useIncorporateCleanup";
import type { IncorporateScale } from "./incorporateScale";

/**
 * 收编 / 清理空项目弹层（P4d-8a-2c，纯展示）：逐字搬旧页 IncorporateDialog（L1611-1760）与
 * 清理确认（L794-810）。骨架与 testid 原样（incorporate-dialog / incorporate-confirm / …、
 * env-cleanup-empty-projects-dialog 及其 -cancel/-confirm 后缀）；文案全部取 ENV_TEXTS，
 * 四阶段标签取 builder 的 incorporateStageLabels（唯一来源）。本文件零命令、零产品判断，
 * 确认与关闭只回调；运行中不允许关层（后端事务已开）。
 */

/** 收编弹层：phase=null 即确认态（旧页 phase.kind === "confirm"）；其余三态由 hook 推进。 */
export interface EnvironmentIncorporateDialogProps {
  readonly target: IncorporateTarget;
  /** null = 确认态；running / done / failed 由 useIncorporateCleanup 产出。 */
  readonly phase: IncorporatePhase | null;
  /** 进度事件阶段（useEnvironmentBusy 已按目标库过滤；null = 未收到）。 */
  readonly stage: MasterIncorporateProgressEvent["stage"] | null;
  /** 确认态规模（体检同源口径）。 */
  readonly scale: IncorporateScale;
  /** 归入目标账号显示名（null = 暂不可读，降级「当前账号」）。 */
  readonly accountName: string | null;
  readonly onConfirm: () => void;
  readonly onClose: () => void;
}

/** 收编弹层（确认 → 四阶段进度 → 完成回执 / 失败提示）。 */
export function EnvironmentIncorporateDialog({
  target, phase, stage, scale, accountName, onConfirm, onClose,
}: EnvironmentIncorporateDialogProps) {
  const stages = incorporateStageLabels(target.isMaster, target.envName);
  const running = phase?.kind === "running";
  // 进度口径照旧页 L1630-1641：done 或阶段事件 done 即满格，否则按当前阶段序号推进。
  const allDone = phase?.kind === "done" || (running && stage === "done");
  const stageIndex = allDone ? stages.length : running ? Math.max(0, stages.findIndex((item) => item.stage === stage)) : 0;
  const percent = allDone ? 100 : Math.round(((stageIndex + 1) / stages.length) * 100);
  const targetName = accountName ?? ENV_TEXTS.incorporateTargetFallback;
  const targetLabel = target.isMaster ? ENV_TEXTS.masterName : fillEnvText(ENV_TEXTS.envNameQuoted, { name: target.envName });

  return (
    <div className="switch-veil" role="presentation" onClick={(event) => {
      // 运行中不允许点遮罩中断（后端事务已开）；确认/完成/失败可关闭。
      if (event.target === event.currentTarget && !running) onClose();
    }}>
      <div className="switch-dialog" role="dialog" aria-modal="true" aria-label={ENV_TEXTS.incorporateTitle}
        data-testid="incorporate-dialog">
        <div className="switch-dialog__title">{ENV_TEXTS.incorporateTitle}</div>
        <div className="switch-dialog__target">
          <span className="switch-dialog__avatar" aria-hidden="true">{targetName.slice(0, 1)}</span>
          <div>
            <div className="switch-dialog__target-name" data-testid="incorporate-target">{targetName}</div>
            <div className="switch-dialog__target-meta">
              {fillEnvText(ENV_TEXTS.incorporateTargetMeta, { target: target.isMaster ? ENV_TEXTS.masterName : target.envName })}
            </div>
          </div>
        </div>

        {phase === null ? (
          <>
            <div className="switch-dialog__notice">
              <TriangleAlert size={18} aria-hidden="true" />
              <div>
                <strong>{fillEnvText(ENV_TEXTS.incorporateConfirmHead, { accounts: scale.staleAccounts.length })}</strong>
                <p>
                  {fillEnvText(ENV_TEXTS.incorporateConfirmBody, {
                    sessions: scale.staleSessionTotal, projects: scale.staleProjectTotal, account: targetName, target: targetLabel,
                  })}
                  {scale.orphanProjectCount > 0
                    ? fillEnvText(ENV_TEXTS.incorporateOrphanSuffix, { orphans: scale.orphanProjectCount }) : ""}
                </p>
              </div>
            </div>
            <ul className="switch-dialog__diff" data-testid="incorporate-account-list">
              {scale.staleAccounts.map((account) => (
                <li key={account.id} className="switch-dialog__diff-item"
                  title={fillEnvText(ENV_TEXTS.accountIdTitle, { id: account.id })}>{account.name}</li>
              ))}
            </ul>
            <div className="switch-dialog__actions">
              <button className="btn" type="button" onClick={onClose} data-testid="incorporate-cancel">
                {ENV_TEXTS.cancel}
              </button>
              <button className="btn btn--primary" type="button" onClick={onConfirm} data-testid="incorporate-confirm">
                <Combine size={15} aria-hidden="true" />{ENV_TEXTS.incorporateConfirm}
              </button>
            </div>
          </>
        ) : phase.kind === "failed" ? (
          <>
            <div className="switch-dialog__notice switch-dialog__notice--danger" role="alert">
              <TriangleAlert size={18} aria-hidden="true" />
              <p>{phase.message}</p>
            </div>
            <div className="switch-dialog__actions">
              <button className="btn" type="button" onClick={onClose} data-testid="incorporate-failed-close">
                {ENV_TEXTS.incorporateClose}
              </button>
            </div>
          </>
        ) : (
          <>
            {phase.kind === "done" && (
              <div className="switch-dialog__done" role="status">
                <CheckCircle2 size={18} aria-hidden="true" />
                <span>
                  {fillEnvText(ENV_TEXTS.incorporateDone, {
                    accounts: phase.receipt.merged_accounts, projects: phase.receipt.transferred_projects,
                    sessions: phase.receipt.switched_sessions,
                  })}
                  {phase.receipt.relay_ledger_written ? "" : ENV_TEXTS.incorporateLedgerMissing}
                </span>
              </div>
            )}
            {/* 回执已到但整组重读失败（K-07 项 4）：完成回执照旧在上，这里只追加提示，不翻失败态。 */}
            {phase.kind === "done" && phase.refreshFailed && (
              <div className="switch-dialog__notice switch-dialog__notice--danger" role="alert">
                <TriangleAlert size={18} aria-hidden="true" />
                <p>{ENV_TEXTS.incorporateRefreshFailed}</p>
              </div>
            )}
            {phase.kind === "done" && phase.receipt.relaunch_outcome === "failed" && (
              <div className="switch-dialog__notice switch-dialog__notice--danger" role="alert">
                <TriangleAlert size={18} aria-hidden="true" />
                <p>{ENV_TEXTS.incorporateRelaunchFailed}</p>
              </div>
            )}
            <div className="switch-dialog__bar" aria-hidden="true">
              <div className="switch-dialog__bar-fill" style={{ width: percent + "%" }} />
            </div>
            <ol className="switch-dialog__steps">
              {stages.map((item, index) => {
                const restartFailed = phase.kind === "done" && item.stage === "restarting" && phase.receipt.relaunch_outcome === "failed";
                const state = restartFailed ? "failed"
                  : phase.kind === "done" || index < stageIndex ? "done"
                    : running && index === stageIndex ? "run" : "idle";
                return (
                  <li key={item.stage} className={"switch-step switch-step--" + state} data-stage={item.stage}>
                    <span className="switch-step__icon" aria-hidden="true">
                      {state === "done" ? <CheckCircle2 size={16} />
                        : state === "failed" ? <TriangleAlert size={16} /> : <span className="switch-step__ring" />}
                    </span>
                    <span className="switch-step__label">{item.label}</span>
                  </li>
                );
              })}
            </ol>
            <div className="switch-dialog__actions">
              {phase.kind === "done" && (
                <button className="btn btn--primary" type="button" onClick={onClose} data-testid="incorporate-done">
                  {ENV_TEXTS.incorporateDoneClose}
                </button>
              )}
            </div>
          </>
        )}
      </div>
    </div>
  );
}

/** 清理空项目确认（旧页 L794-810 逐字）：数量为全库空项目总数，与后端删除范围一致。 */
export interface EnvironmentCleanupDialogProps {
  readonly count: number;
  readonly busy: boolean;
  readonly onCancel: () => void;
  readonly onConfirm: () => void;
}

export function EnvironmentCleanupDialog({ count, busy, onCancel, onConfirm }: EnvironmentCleanupDialogProps) {
  return (
    <ConfirmDialog
      title={ENV_TEXTS.cleanupDialogTitle}
      lines={[
        fillEnvText(ENV_TEXTS.cleanupDialogLine1, { count }),
        ENV_TEXTS.cleanupDialogLine2,
        ENV_TEXTS.cleanupDialogLine3,
      ]}
      confirmLabel={ENV_TEXTS.cleanupConfirm}
      confirmIcon={<Trash2 size={15} aria-hidden="true" />}
      busyLabel={ENV_TEXTS.cleanupBusy}
      busy={busy}
      danger
      onCancel={onCancel}
      onConfirm={onConfirm}
      testId="env-cleanup-empty-projects-dialog"
    />
  );
}
