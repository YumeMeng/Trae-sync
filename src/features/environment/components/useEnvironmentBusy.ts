import { useCallback, useLayoutEffect, useRef, useState } from "react";
import { useTauriEvents } from "../../../hooks/useTauriEvents";
import type { MasterIncorporateProgressEvent } from "../../../types/masterLibrary";

/**
 * 环境页忙碌位 + 收编进度 hook（P4d-5）：实例启停 / 收编 / 清理 / 捕获的忙碌位，
 * 与收编进度事件订阅（事件名与校验规则照旧实现：EnvironmentPage L286-333）。
 * 本 hook 只持状态、不发命令（动作编排在 P4d-6）；产品切换即重置，旧产品的
 * 进行中位与进度阶段不得泄露到新首帧。
 */

/** 忙碌位（旧页 L80-104 原样）：布尔位 + 三个按实例 id 的位（环境内单实例）。 */
export interface EnvironmentBusyState {
  readonly launching: boolean; readonly launchingEnvId: string | null; readonly closingEnvId: string | null;
  readonly repairing: boolean; readonly cleaningUp: boolean; readonly capturing: boolean;
  readonly checking: boolean; // WB 深度检测执行中（只读探针，零写）
  readonly incorporatingEnvId: string | null; // 非空 = 收编进行中（主库 "master" / 副环境 env_id）
}
const IDLE: EnvironmentBusyState = { launching: false, launchingEnvId: null, closingEnvId: null,
  repairing: false, cleaningUp: false, capturing: false, checking: false, incorporatingEnvId: null };
/** 阶段白名单：载荷 stage 是阶段判别字段，未知阶段一律丢弃（不猜进度）。 */
const STAGES: ReadonlySet<string> = new Set<MasterIncorporateProgressEvent["stage"]>([
  "closing", "backing_up", "incorporating", "restarting", "done"]);

export interface EnvironmentBusyResult {
  readonly busy: EnvironmentBusyState;
  /** 收编进度阶段（已按目标库过滤）；null = 无进行中收编 / 未收到阶段事件。 */
  readonly incorporateStage: MasterIncorporateProgressEvent["stage"] | null;
  /** 合并写入忙碌位：只改传入的位，其余保持。 */
  readonly patchBusy: (next: Partial<EnvironmentBusyState>) => void;
  /** 收编开关：非 null = 开始（置目标库并清阶段）；null = 结束并清阶段。 */
  readonly setIncorporating: (envId: string | null) => void;
}

/** @param productId 与 registry / 能力表同键的产品；@param active 页面可见才订阅事件。 */
export function useEnvironmentBusy(productId: string, active: boolean): EnvironmentBusyResult {
  const [busy, setBusy] = useState<EnvironmentBusyState>(IDLE);
  const [stage, setStage] = useState<MasterIncorporateProgressEvent["stage"] | null>(null);
  // 事件回调经 ref 取当前目标：目标变化不重订阅，不匹配的载荷在落地时被丢弃。
  const target = useRef<string | null>(null);
  target.current = busy.incorporatingEnvId;
  // 产品切换：绘制前清空全部忙碌位与进度阶段（旧产品位不得阻塞新产品按钮）。
  useLayoutEffect(() => { target.current = null; setBusy(IDLE); setStage(null); }, [productId]);
  const patchBusy = useCallback((next: Partial<EnvironmentBusyState>) => {
    setBusy((current) => ({ ...current, ...next }));
  }, []);
  const setIncorporating = useCallback((envId: string | null) => {
    target.current = envId; // 同一 tick 内即生效：首个阶段事件可能早于重渲染到达
    setBusy((current) => (current.incorporatingEnvId === envId ? current : { ...current, incorporatingEnvId: envId }));
    setStage(null); // 新会话从确认态起步，上一轮的阶段不得残留
  }, []);
  // 进度订阅（旧 L293-303 同一事件名，常挂载不随目标重建，首个阶段事件不会丢）：
  // 页面不可见、无进行中收编、env_id 与目标不符（多库并行互不串台）、stage 不在白名单
  // （未知 kind）四种情况一律丢弃，绝不把未知阶段当进度推进。
  useTauriEvents({ "master-incorporate-progress": (payload: MasterIncorporateProgressEvent) => {
    if (!active || target.current === null || payload.env_id !== target.current || !STAGES.has(payload.stage)) return;
    setStage(payload.stage);
  } }, [active]);
  return { busy, incorporateStage: stage, patchBusy, setIncorporating };
}
