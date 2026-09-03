import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { RefreshCw } from "lucide-react";
import type { CapabilityFlagsDto } from "../types/workspace";
import { safeUiErrorMessage, safeUiRecommendedAction } from "../utils/safeUiError";
import type {
  CommandErrorDto,
  LockStatusDto,
  OperationFinishedEventDto,
  OperationNeedsAttentionEventDto,
  OperationStateDto,
  OperationSummaryDto,
  OperationStageEventDto,
  OperationProgressEventDto,
  ProgressSnapshotDto,
  ReconcileUnfinishedOperationsDto,
} from "../types/operations";

const REFRESH_INTERVAL_MS = 1_000;

interface OperationsPanelProps {
  capabilities: CapabilityFlagsDto;
  active?: boolean;
  /** 嵌入总览页时收起页面级标题，只保留小节标题。 */
  compact?: boolean;
}

// 最近活动面板只读取后端摘要、锁和进度；不可见时不启动轮询。
export function OperationsPanel({ capabilities, active = true, compact = false }: OperationsPanelProps) {
  const [operations, setOperations] = useState<OperationSummaryDto[]>([]);
  const [lockStatus, setLockStatus] = useState<LockStatusDto | null>(null);
  const [progress, setProgress] = useState<ProgressSnapshotDto | null>(null);
  const [loading, setLoading] = useState(true);
  const [commandUnavailable, setCommandUnavailable] = useState(false);
  const [stateError, setStateError] = useState<CommandErrorDto | null>(null);
  const [reconciling, setReconciling] = useState(false);
  const [reconcileResult, setReconcileResult] =
    useState<ReconcileUnfinishedOperationsDto | null>(null);
  const [reconcileError, setReconcileError] = useState<CommandErrorDto | null>(null);
  const [refreshNonce, setRefreshNonce] = useState(0);
  const activeOperationIdRef = useRef<string | null>(null);
  const operationsRef = useRef<OperationSummaryDto[]>([]);
  const loadVersionRef = useRef(0);
  const attentionEligibleIdsRef = useRef<Set<string>>(new Set());
  const pendingAttentionRef = useRef<Map<string, CommandErrorDto>>(new Map());
  // 终态事件和人工关注事件可能分两次到达；保留刚结束的操作关联，避免关注提示被吞掉。
  const eventEligibleIdsRef = useRef<Set<string>>(new Set());
  const preferredOperationIdRef = useRef<string | null>(null);
  const [attention, setAttention] = useState<{
    operationId: string;
    error: CommandErrorDto;
  } | null>(null);

  useEffect(() => {
    if (!active) return;
    setLoading(true);
    let mounted = true;
    let unlisteners: Array<() => void> = [];

    async function loadOperationState() {
      const loadVersion = ++loadVersionRef.current;
      const [operationsResult, lockResult] = await Promise.all([
        readCommand<OperationSummaryDto[]>("list_operations"),
        readCommand<LockStatusDto>("get_operation_lock_status"),
      ]);
      if (!mounted || loadVersion !== loadVersionRef.current) return;

      // sequence 只在单个 journal 内有时序意义；跨数据位置时先按活动租约收窄。
      const nextOperations = [...(operationsResult.value ?? [])].sort(
        (left, right) => right.sequence - left.sequence,
      );
      const preferredOperationId = preferredOperationIdRef.current;
      const preferredOperation = preferredOperationId
        ? nextOperations.find((operation) => operation.operation_id === preferredOperationId)
        : undefined;
      const operationId =
        preferredOperation?.operation_id ??
        selectActiveOperation(nextOperations, lockResult.value)?.operation_id;
      if (preferredOperation) preferredOperationIdRef.current = null;
      const progressResult = await readCommand<ProgressSnapshotDto | null>(
        "get_progress",
        operationId ? { operationId } : undefined,
      );

      // 终态事件、轮询或手动重查可能并发到达；旧请求只能丢弃自己的结果。
      if (!mounted || loadVersion !== loadVersionRef.current) return;
      activeOperationIdRef.current = operationId ?? null;
      operationsRef.current = nextOperations;
      attentionEligibleIdsRef.current = new Set([
        ...(operationId ? [operationId] : []),
        ...eventEligibleIdsRef.current,
      ]);
      const visibleOperationIds = new Set(nextOperations.map((operation) => operation.operation_id));
      const pendingAttention = [...pendingAttentionRef.current.entries()].find(([id]) =>
        visibleOperationIds.has(id),
      );
      if (pendingAttention) {
        const [pendingOperationId, pendingError] = pendingAttention;
        pendingAttentionRef.current.delete(pendingOperationId);
        eventEligibleIdsRef.current.delete(pendingOperationId);
        attentionEligibleIdsRef.current.add(pendingOperationId);
        setAttention({ operationId: pendingOperationId, error: pendingError });
      }
      setOperations(nextOperations);
      setLockStatus(lockResult.value);
      setProgress(progressResult.value);
      setStateError(operationsResult.error ?? lockResult.error ?? progressResult.error);
      setCommandUnavailable(
        operationsResult.unavailable || lockResult.unavailable || progressResult.unavailable,
      );
      setLoading(false);
    }

    async function subscribeToOperationEvents() {
      const listeners = [
        listen<OperationStageEventDto>("operation-stage", (event) => {
          if (!mounted || activeOperationIdRef.current !== event.payload.operation_id) return;
          setProgress((current) => {
            if (!current || current.operation_id !== event.payload.operation_id) return current;
            return {
              ...current,
              phase: event.payload.phase,
              cancellable: event.payload.cancellable,
            };
          });
        }),
        listen<OperationProgressEventDto>("operation-progress", (event) => {
          if (!mounted || activeOperationIdRef.current !== event.payload.operation_id) return;
          setProgress((current) => {
            if (current && current.operation_id !== event.payload.operation_id) return current;
            return event.payload;
          });
        }),
        listen<OperationNeedsAttentionEventDto>("operation-needs-attention", (event) => {
          const operationId = event.payload.operation_id;
          const listedOperation = operationsRef.current.find(
            (operation) => operation.operation_id === operationId,
          );
          const eventEligible = eventEligibleIdsRef.current.has(operationId);
          const isKnownOperation =
            activeOperationIdRef.current === operationId ||
            attentionEligibleIdsRef.current.has(operationId) ||
            eventEligible ||
            (listedOperation !== undefined && isAttentionEligibleOperation(listedOperation.state));
          // 迟到的旧操作提示不能污染当前操作；空列表时保留唯一的结构化告警，避免吞掉执行前错误。
          if (!mounted) return;
          if (!isKnownOperation) {
            // 只有尚未出现在列表中的操作才需要暂存；已终态操作的迟到事件直接忽略。
            if (!listedOperation) {
              pendingAttentionRef.current.set(operationId, event.payload.error);
              preferredOperationIdRef.current = operationId;
              void loadOperationState();
            }
            return;
          }
          eventEligibleIdsRef.current.delete(operationId);
          pendingAttentionRef.current.delete(operationId);
          attentionEligibleIdsRef.current.add(operationId);
          setAttention({ operationId, error: event.payload.error });
        }),
        listen<OperationFinishedEventDto>("operation-finished", (event) => {
          const operationId = event.payload.operation_id;
          const isKnownOperation =
            activeOperationIdRef.current === operationId ||
            attentionEligibleIdsRef.current.has(operationId) ||
            eventEligibleIdsRef.current.has(operationId) ||
            operationsRef.current.some((operation) => operation.operation_id === operationId);
          if (!mounted) return;
          if (!isKnownOperation) {
            // 先记住事件关联；随后到达的 needs-attention 即使没有持久化列表也不能丢失。
            eventEligibleIdsRef.current.add(operationId);
            preferredOperationIdRef.current = operationId;
            void loadOperationState();
            return;
          }
          eventEligibleIdsRef.current.add(operationId);
          attentionEligibleIdsRef.current.add(operationId);
          // 成功终态先清除旧的关注提示；需要关注的终态随后会再次发出 attention 事件。
          setAttention((current) =>
            current?.operationId === operationId ? null : current,
          );
          // 终态事件立即触发重查，操作列表和持久化进度仍以 command 返回为准。
          void loadOperationState();
        }),
      ];
      const settled = await Promise.allSettled(listeners);
      const activeListeners = settled.flatMap((result) =>
        result.status === "fulfilled" ? [result.value] : [],
      );
      if (!mounted) {
        activeListeners.forEach((unlisten) => unlisten());
        return;
      }
      unlisteners = activeListeners;
    }

    let refreshTimer: number | undefined;

    async function refreshAndSchedule() {
      await loadOperationState();
      if (mounted) {
        // 使用串行 timeout，避免后端 command 较慢时产生重叠请求。
        refreshTimer = window.setTimeout(() => {
          void refreshAndSchedule();
        }, REFRESH_INTERVAL_MS);
      }
    }

    void refreshAndSchedule();
    // 浏览器 mock 或旧版后端可能没有事件插件；此时静默退回轮询，不改变只读边界。
    void subscribeToOperationEvents().catch(() => undefined);
    return () => {
      mounted = false;
      if (refreshTimer !== undefined) window.clearTimeout(refreshTimer);
      unlisteners.forEach((unlisten) => unlisten());
    };
  }, [active, refreshNonce]);

  const hasUnfinishedOperations = operations.some(
    (operation) => !isTerminalOperation(operation.state),
  );

  async function handleReconcileUnfinishedOperations() {
    if (reconciling || !capabilities.sync_enabled || !hasUnfinishedOperations) return;
    setReconciling(true);
    setReconcileResult(null);
    setReconcileError(null);
    try {
      const result = await invoke<ReconcileUnfinishedOperationsDto>(
        "reconcile_unfinished_operations",
      );
      setReconcileResult(result);
      setRefreshNonce((current) => current + 1);
    } catch (error) {
      setReconcileError(parseCommandError(error));
    } finally {
      setReconciling(false);
    }
  }

  const operationsLiveStatus = loading
    ? "正在读取操作状态。"
    : commandUnavailable
      ? "操作状态暂时不可用。"
      : operations.length === 0
        ? "当前没有执行中的操作。"
        : `已加载 ${operations.length} 条操作记录。`;

  return (
    <section
      className={`operations-panel${compact ? " operations-panel--compact" : ""}`}
      role="region"
      aria-label="最近活动"
      aria-busy={loading}
      data-testid="operations-panel"
    >
      <p className="sr-only" role="status" aria-live="polite" aria-atomic="true">
        {operationsLiveStatus}
      </p>
      {compact ? (
        <div className="operations-panel__section-heading">
          <h2>最近活动</h2>
          <span className="section-caption">打开本页时自动更新</span>
        </div>
      ) : (
        <header className="page-header">
          <div className="page-header__copy">
            <h1 data-page-title="operations" tabIndex={-1}>最近活动</h1>
            <p>查看执行进度、操作记录和需要处理的事项。</p>
          </div>
        </header>
      )}
      {loading && (
        <p className="operations-panel__empty" role="status" data-testid="operations-loading">
          正在读取操作状态…
        </p>
      )}
      {commandUnavailable && (
        <p
          className="workbench__error"
          role="status"
          aria-live="polite"
          aria-atomic="true"
          data-testid="operations-unavailable"
        >
          {safeUiErrorMessage(stateError, "操作状态暂不可用。")}
          {stateError
            ? ` ${safeUiRecommendedAction(stateError, "稍后重试。")}`
            : ""}
        </p>
      )}
      {attention && (
        <p className="workbench__error" role="alert" aria-atomic="true" data-testid="operation-attention">
          {safeUiErrorMessage(attention.error, "操作需要重新核对。")}{" "}
          {safeUiRecommendedAction(attention.error, "稍后重试。")}
        </p>
      )}
      {!loading && !commandUnavailable && operations.length === 0 && (
        <p
          className="operations-panel__empty"
          role="status"
          aria-live="polite"
          aria-atomic="true"
          data-testid="operations-empty"
        >
          暂无执行记录；浏览和预览不会产生操作记录。
        </p>
      )}
      {operations.length > 0 && (
        <ul className="operations-panel__list" data-testid="operations-list">
          {operations.map((operation) => (
            <li key={operation.operation_id}>
              <div className="operations-panel__item-main">
                <strong>{renderOperationState(operation.state)}</strong>
                <span>
                  {operation.has_verified_target_file_evidence
                    ? "（已验证）"
                    : "（待验证）"}
                </span>
              </div>
              <details className="operations-panel__item-details">
                <summary>详情</summary>
                <span className="operations-panel__item-meta">
                  操作引用：{operation.operation_id}
                </span>
              </details>
            </li>
          ))}
        </ul>
      )}
      {lockStatus && (operations.length > 0 || lockStatus.reason) && (
        <p
          className="operations-panel__empty"
          role="status"
          aria-live="polite"
          aria-atomic="true"
          data-testid="operation-lock-status"
        >
          {lockStatus.reason
            ? `需要关注：${lockStatus.reason}`
            : lockStatus.catalog_lock_held || lockStatus.data_location_lock_held
              ? "有操作进行中"
              : "空闲"}
        </p>
      )}
      {progress && <ProgressView progress={progress} />}
      {reconcileError && (
        <p
          className="workbench__error"
          role="alert"
          aria-atomic="true"
          data-testid="operation-reconcile-error"
        >
          {safeUiErrorMessage(reconcileError, "未完成操作核对失败。")}{" "}
          {safeUiRecommendedAction(reconcileError, "稍后重试。")}
        </p>
      )}
      {reconcileResult && (
        <p
          className={
            reconcileResult.status === "manual_recovery_required"
              ? "workbench__error"
              : "operations-panel__empty"
          }
          role={reconcileResult.status === "manual_recovery_required" ? "alert" : "status"}
          aria-live="polite"
          aria-atomic="true"
          data-testid="operation-reconcile-result"
        >
          {renderReconciliationResult(reconcileResult)}
        </p>
      )}
      {capabilities.sync_enabled && hasUnfinishedOperations && !commandUnavailable && (
        <div className="operations-panel__actions">
          <button
            type="button"
            className="btn"
            onClick={() => void handleReconcileUnfinishedOperations()}
            disabled={loading || reconciling}
            aria-busy={reconciling}
            data-testid="reconcile-operations-button"
          >
            <RefreshCw
              size={16}
              aria-hidden="true"
              className={reconciling ? "operations-panel__reconcile-icon--busy" : undefined}
            />
            {reconciling ? "正在核对…" : "重新核对未完成操作"}
          </button>
        </div>
      )}
    </section>
  );
}

function renderReconciliationResult(result: ReconcileUnfinishedOperationsDto): string {
  if (result.status === "manual_recovery_required") {
    return `已核对 ${result.inspected_count} 条记录，其中 ${result.manual_recovery_required_count} 条需要人工恢复。失败证据已保留，请勿启动 TRAE。`;
  }
  if (result.status === "other_data_location_pending") {
    return `当前数据位置没有未完成操作；另有 ${result.unrelated_data_location_count} 条其他数据位置记录未处理。`;
  }
  if (result.status === "no_unfinished_operations") {
    return "未发现需要核对的未完成操作。";
  }
  return `核对完成：${result.not_applied_count} 条确认未应用，${result.completed_count} 条完成状态已验证。`;
}

// 操作终态与后端 OperationState::is_terminal 保持一致，避免把历史终态当作当前长任务。
function isTerminalOperation(state: OperationStateDto): boolean {
  return (
    state === "completed" ||
    state === "cancelled_before_write" ||
    state === "failed_safe" ||
    state === "not_applied" ||
    state === "restored_verified" ||
    state === "manual_recovery_required"
  );
}

function isAttentionEligibleOperation(state: OperationStateDto): boolean {
  return !isTerminalOperation(state) || state === "manual_recovery_required";
}

// 当前进度优先绑定仍在进行且持有数据位置锁的操作；手工恢复终态仍可能保留可核对进度。
function selectActiveOperation(
  operations: readonly OperationSummaryDto[],
  lockStatus: LockStatusDto | null,
): OperationSummaryDto | undefined {
  const activeOperations = operations.filter(
    (operation) =>
      !isTerminalOperation(operation.state) || operation.state === "manual_recovery_required",
  );
  if (activeOperations.length === 0) return undefined;

  const lockedOperations = lockStatus?.data_location_id
    ? activeOperations.filter(
        (operation) => operation.data_location_id === lockStatus.data_location_id,
      )
    : [];
  const candidates = lockedOperations.length > 0 ? lockedOperations : activeOperations;
  return [...candidates].sort(
    (left, right) =>
      right.sequence - left.sequence || left.operation_id.localeCompare(right.operation_id),
  )[0];
}

interface CommandReadResult<T> {
  value: T | null;
  unavailable: boolean;
  error: CommandErrorDto | null;
}

// 后端 command 读取失败时只返回不可用标记，不把错误正文展示给用户。
async function readCommand<T>(
  command: string,
  args?: Record<string, unknown>,
): Promise<CommandReadResult<T>> {
  try {
    const value = args === undefined ? await invoke<T>(command) : await invoke<T>(command, args);
    return { value, unavailable: false, error: null };
  } catch (error) {
    return { value: null, unavailable: true, error: parseCommandError(error) };
  }
}

function parseCommandError(error: unknown): CommandErrorDto {
  if (typeof error === "object" && error !== null) {
    const candidate = error as Partial<CommandErrorDto>;
    if (
      typeof candidate.code === "string" &&
      typeof candidate.message === "string" &&
      typeof candidate.recommended_action === "string" &&
      typeof candidate.retryable === "boolean"
    ) {
      return candidate as CommandErrorDto;
    }
  }
  return {
    code: "operation_state_unavailable",
    message: "操作状态暂时不可读取。",
    recommended_action: "稍后重试。",
    retryable: true,
  };
}

// 进度用可视化进度条呈现；总量未知时显示不确定动画。
function ProgressView({ progress }: { progress: ProgressSnapshotDto }) {
  const hasKnownTotal =
    progress.total_bytes !== null && progress.percent_basis_points !== null;
  const percent = hasKnownTotal
    ? Math.round(progress.percent_basis_points! / 100)
    : null;

  return (
    <div
      className="operations-panel__progress"
      role="status"
      aria-live="polite"
      aria-atomic="true"
      data-testid="operation-progress"
    >
      <div className="operations-panel__progress-head">
        <strong>{renderProgressPhase(progress.phase)}</strong>
        <span>
          {formatBytes(progress.completed_bytes)}
          {progress.total_bytes !== null ? ` / ${formatBytes(progress.total_bytes)}` : ""}
        </span>
      </div>
      <div
        className={`operations-panel__progress-track${hasKnownTotal ? "" : " operations-panel__progress-track--indeterminate"}`}
        role="progressbar"
        aria-valuenow={percent ?? undefined}
        aria-valuemin={0}
        aria-valuemax={100}
        aria-label="操作进度"
      >
        {hasKnownTotal && (
          <div
            className="operations-panel__progress-fill"
            style={{ width: `${Math.min(100, Math.max(0, percent!))}%` }}
          />
        )}
      </div>
      {/* 两种取消语义都要明示，用户才能判断当前阶段是否还能安全中止。 */}
      <span className="section-caption">
        {progress.cancellable ? "当前阶段可取消" : "本阶段不可取消"}
      </span>
    </div>
  );
}

function renderOperationState(state: OperationStateDto): string {
  const labels: Record<OperationStateDto, string> = {
    planned: "已计划",
    backing_up: "正在备份",
    backup_verified: "备份已验证",
    target_writing: "正在写入",
    target_committed_unverified: "已提交，待验证",
    target_verifying: "正在验证",
    catalog_reconciling: "正在整理记录",
    verification_inconclusive: "验证未完成",
    failure_preserving: "正在保存失败现场",
    failure_snapshot_verified: "失败现场已验证",
    restore_staging: "正在准备恢复",
    restore_staged: "恢复副本已准备",
    restore_replacing: "正在替换恢复目标",
    restored_verifying: "正在验证恢复结果",
    completed: "已完成",
    cancelled_before_write: "写入前已取消",
    failed_safe: "安全失败",
    not_applied: "未应用",
    restored_verified: "恢复已验证",
    manual_recovery_required: "需要人工恢复",
  };
  return labels[state];
}

function renderProgressPhase(phase: ProgressSnapshotDto["phase"]): string {
  const labels: Record<ProgressSnapshotDto["phase"], string> = {
    preparing: "正在准备",
    copying: "正在复制",
    hashing: "正在计算校验",
    writing: "正在写入",
    verifying: "正在验证",
    recovering: "正在恢复",
    completed: "已完成",
    failed: "失败",
  };
  return labels[phase];
}

// 字节进度只用于用户核对，不推导 ETA 或未知总量百分比。
function formatBytes(value: number): string {
  if (!Number.isFinite(value) || value < 1024) return `${value} B`;
  const units = ["B", "KiB", "MiB", "GiB"];
  const exponent = Math.min(Math.floor(Math.log(value) / Math.log(1024)), units.length - 1);
  return `${(value / 1024 ** exponent).toFixed(exponent === 1 ? 1 : 2)} ${units[exponent]}`;
}
