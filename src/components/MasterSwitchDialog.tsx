import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { CheckCircle2, TriangleAlert } from "lucide-react";
import type {
  MasterAccountSwitchDto,
  MasterSwitchErrorCode,
  MasterSwitchHandoverProgress,
  MasterSwitchProgressEvent,
  MasterSwitchRolledBackEvent,
  MasterSwitchStage,
  MasterSwitchPluginPreviewDto,
  PluginCloudSyncDto,
} from "../types/account_switch";
import { safeUiErrorMessage } from "../utils/safeUiError";

/** P7-3 交接进度 → 细目文案（label 为项目名/数据类别，技术 ID 不进主视野）。 */
function handoverProgressText(progress: MasterSwitchHandoverProgress): string {
  const counter = progress.total > 1 ? `（${progress.current}/${progress.total}）` : "";
  switch (progress.phase) {
    case "mapping":
      return `正在整理项目“${progress.label}”的会话${counter}`;
    case "executing":
      return `正在写入${progress.label}${counter}`;
    case "verifying":
      return `正在校验项目“${progress.label}”的会话${counter}`;
  }
}

/** 提取后端原始错误码（invoke 拒绝值为裸字符串码；不映射文案，由弹层分支处理）。 */
function rawErrorCode(reason: unknown): string {
  if (typeof reason === "string") return reason.trim();
  if (reason instanceof Error) return reason.message.trim();
  if (typeof reason === "object" && reason !== null) {
    const candidate = reason as { code?: unknown; message?: unknown };
    const parts = [candidate.code, candidate.message].filter((part): part is string => typeof part === "string");
    if (parts.length > 0) return parts.join(" ").trim();
  }
  return "";
}

/** 切换目标账号（弹层头部展示所需的最小信息）。 */
export interface MasterSwitchTarget {
  readonly profile_id: string;
  /** 展示名（备注名优先）。 */
  readonly display_name: string;
}

interface MasterSwitchDialogProps {
  /** 目标账号；非空即打开弹层并开始切换。 */
  target: MasterSwitchTarget | null;
  /** 切换完成（成功）后通知账号页刷新环境状态与总览。 */
  onFinished: () => Promise<void> | void;
  /** 弹层关闭（完成/取消）时清理目标。 */
  onClose: () => void;
}

/** 切号阶段（Q1.1 顺序）与用户文案；done 是回执不是步骤。 */
const STAGES: ReadonlyArray<{ stage: MasterSwitchStage; label: string }> = [
  { stage: "closing", label: "关闭 TRAE 主库实例" },
  { stage: "backing_up", label: "备份主库数据" },
  { stage: "switching_login", label: "写入目标账号登录态" },
  { stage: "handing_over", label: "转移对话记录到新账号" },
  { stage: "syncing_plugins", label: "同步插件到目标账号" },
  { stage: "restarting", label: "重启 TRAE" },
];

/** 插件对账回执 → 回执行文案；无差异时静默（无事发生即最佳）。 */
function pluginSyncNote(sync: PluginCloudSyncDto): string {
  if (sync.declined) {
    return " · 已按目标账号的插件现状切换";
  }
  if (sync.aborted) {
    return " · 插件未能同步，可在 TRAE 插件市场重新安装";
  }
  if (sync.failed > 0) {
    return ` · 插件同步新增 ${sync.installed} 项、移除 ${sync.removed} 项，${sync.failed} 项失败，可在插件市场处理`;
  }
  if (sync.installed > 0 || sync.removed > 0) {
    return ` · 插件已同步（新增 ${sync.installed} 项、移除 ${sync.removed} 项）`;
  }
  return "";
}

/** 错误码 → 用户文案（独立分支文案；busy 由弹层分支处理，不在此列）。 */
const ERROR_COPY: Record<MasterSwitchErrorCode, string> = {
  master_switch_busy: "主库正在生成回复，暂不能切换。",
  master_switch_db_missing: "主库尚未启动登录过：请先在本页启动主库并在 TRAE 内登录一次。",
  master_login_missing: "主库内没有登录凭据：请先启动主库并登录一次。",
  switch_donor_login_missing: "目标账号的登录凭据不可用（在线验证与本地存档均未通过）：请在账号页重新登录该账号后再切换。",
  switch_same_account: "目标账号已是主库当前登录账号，无需切换。",
  master_switch_conflict: "目标账号名下存在同名项目的非空记录，需要人工决策：请先在 TRAE 中核对该项目后再试。",
  master_switch_close_failed: "关闭主库实例失败，请稍后重试。",
  master_switch_backup_failed: "主库数据备份失败，切换已中止（主库数据未受影响）。",
  switch_verify_failed: "登录信息写入后校验失败，切换已中止；主库数据备份已保留。",
  master_switch_integrity_failed: "对话记录转移后的完整性校验未通过，切换已中止；可用切换前的备份恢复。",
  master_switch_db_open_failed: "主库数据库暂时无法打开（可能仍在写入），请稍后重试。",
  master_switch_rollback_failed: "切换失败，且自动还原未完成：请使用切换前的备份恢复主库数据。",
  switch_auth_failed: "账号登录状态切换失败，已中止。",
  master_switch_join_failed: "切换任务异常结束，请稍后重试。",
  trae_real_mode_required: "当前为演示模式，切换账号需要真实模式。",
  source_key_unavailable: "本机密钥暂不可用，请稍后重试。",
  checkin_registry_invalid: "账号注册表读取失败，请重启应用后重试。",
  trae_profile_invalid: "目标账号档案不存在，请刷新账号页后重试。",
  environment_registry_invalid: "环境档案写入失败，请重启应用后重试。",
};

type DialogPhase =
  | { kind: "previewing" }
  | { kind: "confirm"; preview: MasterSwitchPluginPreviewDto }
  | { kind: "running"; stage: MasterSwitchStage }
  | { kind: "busy" }
  | { kind: "done"; receipt: MasterAccountSwitchDto }
  | { kind: "failed"; message: string };

/**
 * 主库切号进度弹层（P5-2，消费 P5-1 的 master-switch-progress 事件）：
 * 插件差异预检（+N/-M 确认，ADR-0023）→ 五步进度 → 完成回执；
 * `master_switch_busy` 走「等待完成 / 强制切换」分支（Q1.2）。
 */
export function MasterSwitchDialog({ target, onFinished, onClose }: MasterSwitchDialogProps) {
  const [phase, setPhase] = useState<DialogPhase>({ kind: "previewing" });
  // P7-3 交接细粒度进度（仅 handing_over 阶段显示；进入其他阶段自动隐藏）。
  const [handoverProgress, setHandoverProgress] = useState<MasterSwitchHandoverProgress | null>(null);
  // busy 分支记忆目标与插件对账选择（强制切换时无需上层重新传参）。
  const busyTargetRef = useRef<{ target: MasterSwitchTarget; applyPlugins: boolean } | null>(null);
  // 已发起预检的 profile：防止 onFinished 身份变化引起 startSwitch 重建后重复 invoke。
  const startedForRef = useRef<string | null>(null);
  // P7-2 回滚标记：后端在 invoke 拒绝前发 rolled-back 事件（事件先到、
  // 错误后到），失败文案据此附「已自动还原，可安全重试」。
  const rolledBackRef = useRef(false);

  useEffect(() => {
    if (!target) startedForRef.current = null;
  }, [target]);

  const startSwitch = useCallback(async (next: MasterSwitchTarget, force: boolean, applyPlugins: boolean) => {
    setPhase({ kind: "running", stage: "closing" });
    rolledBackRef.current = false;
    setHandoverProgress(null);
    try {
      const receipt = await invoke<MasterAccountSwitchDto>("switch_master_account", {
        profileId: next.profile_id,
        force,
        applyPlugins,
      });
      setPhase({ kind: "done", receipt });
      await onFinished();
    } catch (reason: unknown) {
      const code = rawErrorCode(reason);
      if (code === "master_switch_busy") {
        busyTargetRef.current = { target: next, applyPlugins };
        setPhase({ kind: "busy" });
      } else {
        const base = (code && code in ERROR_COPY ? ERROR_COPY[code as MasterSwitchErrorCode] : null)
          ?? safeUiErrorMessage(reason, "切换未完成，请稍后重试。");
        const copy = rolledBackRef.current
          ? `${base}主库已自动还原到切换前的登录状态，可安全重试。`
          : base;
        setPhase({ kind: "failed", message: copy });
      }
    }
  }, [onFinished]);

  // 插件差异预检（fail-soft）：无差异或预检失败 → 静默直过切号（apply=true，
  // 执行侧对凭据不可用同样会自行中止）；有差异 → +N/-M 确认后再切换。
  const runPreview = useCallback(async (next: MasterSwitchTarget) => {
    setPhase({ kind: "previewing" });
    try {
      const preview = await invoke<MasterSwitchPluginPreviewDto>("preview_master_switch_plugins", {
        profileId: next.profile_id,
      });
      if (!preview.aborted && (preview.install_names.length > 0 || preview.remove_names.length > 0)) {
        setPhase({ kind: "confirm", preview });
        return;
      }
    } catch {
      // 预检失败不阻断切号（与执行侧 fail-soft 语义一致）。
    }
    void startSwitch(next, false, true);
  }, [startSwitch]);

  // 目标账号变化即开始一次切换（上层通过传入新 target 触发）。
  useEffect(() => {
    if (target && startedForRef.current !== target.profile_id) {
      startedForRef.current = target.profile_id;
      void runPreview(target);
    }
  }, [target, runPreview]);

  // 监听后端六阶段进度事件（done 只用于把进度条推满，回执以 invoke 返回为准）。
  useEffect(() => {
    if (!target) return;
    let unlisten: (() => void) | null = null;
    let cancelled = false;
    void listen<MasterSwitchProgressEvent>("master-switch-progress", (event) => {
      const payload = event.payload;
      if (payload.profile_id !== target.profile_id) return;
      // P7-3：交接明细进度随 handing_over 事件携带，进入其他阶段即清空。
      setHandoverProgress(payload.stage === "handing_over" ? (payload.progress ?? null) : null);
      setPhase((current) => current.kind === "running" ? { kind: "running", stage: payload.stage } : current);
    }).then((fn) => {
      if (cancelled) fn();
      else unlisten = fn;
    }).catch(() => undefined);
    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, [target]);

  // P7-2 回滚结果事件：仅记录标记（事件先于 invoke 拒绝到达），
  // 失败文案在 startSwitch 的 catch 分支读取拼装。
  useEffect(() => {
    if (!target) return;
    let unlisten: (() => void) | null = null;
    let cancelled = false;
    void listen<MasterSwitchRolledBackEvent>("master-switch-rolled-back", (event) => {
      if (event.payload.profile_id !== target.profile_id) return;
      rolledBackRef.current = event.payload.rolled_back;
    }).then((fn) => {
      if (cancelled) fn();
      else unlisten = fn;
    }).catch(() => undefined);
    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, [target]);

  if (!target) return null;

  // done 事件先于回执到达时，五步全部置完成（stageIndex = 步数）。
  const allDone = phase.kind === "done" || (phase.kind === "running" && phase.stage === "done");
  const stageIndex = allDone
    ? STAGES.length
    : phase.kind === "running"
      ? Math.max(0, STAGES.findIndex((item) => item.stage === phase.stage))
      : 0;
  const progressPercent = allDone ? 100 : Math.round((stageIndex + 1) / STAGES.length * 100);
  const displayName = (phase.kind === "busy" ? busyTargetRef.current?.target.display_name : null) ?? target.display_name;

  const handleClose = () => {
    busyTargetRef.current = null;
    onClose();
  };

  return (
    <div className="switch-veil" role="presentation" onClick={(event) => {
      // 运行中不允许点遮罩中断（后端事务已开）；完成/失败/忙时可点遮罩关闭。
      if (event.target === event.currentTarget && phase.kind !== "running") handleClose();
    }}>
      <div className="switch-dialog" role="dialog" aria-modal="true" aria-label="正在切换账号" data-testid="master-switch-dialog">
        <div className="switch-dialog__title">正在切换账号</div>
        <div className="switch-dialog__target">
          <span className="switch-dialog__avatar" aria-hidden="true">{displayName.slice(0, 1)}</span>
          <div>
            <div className="switch-dialog__target-name" data-testid="master-switch-target">{displayName}</div>
            <div className="switch-dialog__target-meta">全部对话记录保留 · 自动完成转移</div>
          </div>
        </div>

        {phase.kind === "busy" ? (
          <>
            <div className="switch-dialog__notice">
              <TriangleAlert size={18} aria-hidden="true" />
              <div>
                <strong>TRAE 正在生成回复</strong>
                <p>当前会话的回复尚未完成。强制切换会中断本轮回复——已生成的内容将保留为不完整轮次，随主库记录一起转移。</p>
              </div>
            </div>
            <div className="switch-dialog__actions">
              <button className="btn" type="button" onClick={handleClose} data-testid="master-switch-wait">等待完成</button>
              <button
                className="btn btn--danger"
                type="button"
                onClick={() => {
                  const next = busyTargetRef.current;
                  busyTargetRef.current = null;
                  if (next) void startSwitch(next.target, true, next.applyPlugins);
                }}
                data-testid="master-switch-force"
              >
                强制切换
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
              <button className="btn" type="button" onClick={handleClose}>关闭</button>
            </div>
          </>
        ) : phase.kind === "confirm" ? (
          <>
            <div className="switch-dialog__notice">
              <TriangleAlert size={18} aria-hidden="true" />
              <div>
                <strong>切换前同步插件</strong>
                <p>
                  当前账号与{displayName}的插件存在差异：安装 {phase.preview.install_names.length} 个、移除 {phase.preview.remove_names.length} 个。
                  移除的插件将从目标账号卸载。
                </p>
              </div>
            </div>
            <ul className="switch-dialog__diff" data-testid="master-switch-plugin-diff">
              {phase.preview.install_names.map((name) => (
                <li key={`install-${name}`} className="switch-dialog__diff-item switch-dialog__diff-item--install">+ {name}</li>
              ))}
              {phase.preview.remove_names.map((name) => (
                <li key={`remove-${name}`} className="switch-dialog__diff-item switch-dialog__diff-item--remove">− {name}</li>
              ))}
            </ul>
            <div className="switch-dialog__actions">
              <button
                className="btn"
                type="button"
                onClick={() => void startSwitch(target, false, false)}
                data-testid="master-switch-keep-plugins"
              >
                保留目标账号插件
              </button>
              <button
                className="btn btn--primary"
                type="button"
                onClick={() => void startSwitch(target, false, true)}
                data-testid="master-switch-apply-plugins"
              >
                同步插件并切换
              </button>
            </div>
          </>
        ) : (
          <>
            {phase.kind === "previewing" && (
              <div className="switch-dialog__prepare">正在核对两账号的插件差异…</div>
            )}
            {phase.kind === "done" && (
              <div className="switch-dialog__done" role="status">
                <CheckCircle2 size={18} aria-hidden="true" />
                <span>
                  已切换到 {displayName}：保留项目 {phase.receipt.transferred_projects} 个 · 转移会话 {phase.receipt.switched_sessions} 个
                  {pluginSyncNote(phase.receipt.plugin_sync)}
                </span>
              </div>
            )}
            <div className="switch-dialog__bar" aria-hidden="true">
              <div className="switch-dialog__bar-fill" style={{ width: `${progressPercent}%` }} />
            </div>
            <ol className="switch-dialog__steps">
              {STAGES.map((item, index) => {
                const state = phase.kind === "done" || index < stageIndex ? "done"
                  : phase.kind === "running" && index === stageIndex ? "run" : "idle";
                return (
                  <li key={item.stage} className={`switch-step switch-step--${state}`} data-stage={item.stage}>
                    <span className="switch-step__icon" aria-hidden="true">
                      {state === "done" ? <CheckCircle2 size={16} /> : <span className="switch-step__ring" />}
                    </span>
                    <span className="switch-step__label">{item.label}</span>
                  </li>
                );
              })}
            </ol>
            {phase.kind === "running" && phase.stage === "handing_over" && handoverProgress && (
              <div className="switch-dialog__subprogress" data-testid="master-switch-handover-progress">
                {handoverProgressText(handoverProgress)}
              </div>
            )}
            <div className="switch-dialog__actions">
              {phase.kind === "done" && (
                <button className="btn btn--primary" type="button" onClick={handleClose} data-testid="master-switch-close">完成</button>
              )}
            </div>
          </>
        )}
      </div>
    </div>
  );
}
