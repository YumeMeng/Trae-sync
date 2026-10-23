import { useCallback, useEffect, useLayoutEffect, useRef, useState } from "react";
import { listEnvironments, readEnvironmentState, readMasterCheckup, readMasterSelfCheck } from "../../../platform/environment";
import { checkWorkBuddyLoginHealth, fetchWorkBuddyProbeInfo, loadWorkBuddySessionHealth } from "../../../platform/workbuddyAdapter";
import { safeUiErrorMessage } from "../../../utils/safeUiError";
import type { EnvironmentListItemDto } from "../../../types/environment";
import type { MasterCheckupDto } from "../../../types/masterLibrary";
import { ENV_TEXTS } from "../config";
import type { TraeEnvironmentSnapshot } from "../traeEnvironment";
import type { WorkBuddyEnvironmentSnapshot } from "../workbuddyEnvironment";

/**
 * 环境页快照读取 hook（P4d-5）：按产品取数据面、代次守卫防竞态、产品切换或 active=false
 * 立即清态、refreshLocal 只做轻刷新（不带体积、不重聚合体检）。只产出数据面——busy /
 * captureNotice / canOpenLibraryDetail 由页面与动作 hook 注入后合成快照。读取失败不静默成
 * 空数组：snapshot 清空、error 如实暴露；旁路失败落 null（未读到），绝不用 [] 冒充「没有」。
 */

/** 数据面 = builder 快照去掉页面自持字段（忙碌位 / 捕获提示 / 详情入口可用性）。 */
export type TraeEnvironmentData = Omit<TraeEnvironmentSnapshot, "busy" | "captureNotice" | "canOpenLibraryDetail">;
export type WorkBuddyEnvironmentData = Omit<WorkBuddyEnvironmentSnapshot, "checking" | "loading" | "error">;
/** 与 registry.EnvironmentSnapshot 同形（判别字段 + 同名载荷键）：页面补自持字段即得快照。 */
export type EnvironmentData =
  | { readonly productId: "trae_cn"; readonly trae: TraeEnvironmentData }
  | { readonly productId: "workbuddy"; readonly workbuddy: WorkBuddyEnvironmentData };

type ReadMode = "full" | "local";

/** 体积合并：新列表不带体积时按 env_id 沿用旧值（体积遍历较重，轻刷新不请求）。 */
function mergeSizes(previous: readonly EnvironmentListItemDto[] | null, next: readonly EnvironmentListItemDto[]): readonly EnvironmentListItemDto[] {
  if (previous === null) return next;
  return next.map((item) => {
    if (item.size_bytes !== undefined) return item;
    const before = previous.find((candidate) => candidate.env_id === item.env_id);
    return before?.size_bytes !== undefined ? { ...item, size_bytes: before.size_bytes } : item;
  });
}

/** 副环境逐库体检（仅完整读取时聚合）：单库失败落 null，不阻塞其余库，也不给假空报告。 */
async function readSecondaryCheckups(environments: readonly EnvironmentListItemDto[] | null): Promise<Readonly<Record<string, MasterCheckupDto | null>>> {
  const secondaries = environments === null ? [] : environments.filter((env) => !env.is_master);
  return Object.fromEntries(await Promise.all(secondaries.map(async (env) => [
    env.env_id, await readMasterCheckup({ libraryId: env.env_id }).catch(() => null)] as const)));
}

/** TRAE 数据面（旧 EnvironmentPage L120-150 / L177-197）：运行态是主读取，失败即整页 error。 */
async function readTrae(mode: ReadMode, previous: unknown): Promise<EnvironmentData> {
  const prior = previous as TraeEnvironmentData | null;
  const state = await readEnvironmentState(); // 主读取：抛错上交 load，不伪装成「没有环境」
  // 轻刷新不带体积：列表失败在完整读取下落 null（未读到），轻刷新下沿用上次列表
  // ——5 秒校准不能因一次瞬时 IPC 失败闪掉整个列表。
  const list = await listEnvironments({ includeSize: mode === "full" }).catch(() => null);
  const environments = list !== null ? mergeSizes(prior?.environments ?? null, list)
    : mode === "local" ? prior?.environments ?? null : null;
  // 体检聚合是重活（逐库只读聚合）：轻刷新沿用上次报告，绝不重聚合（轮询性能红线）。
  const carry = mode === "local" ? prior : null;
  const checkup = carry ? carry.checkup : await readMasterCheckup().catch(() => null);
  const selfCheck = carry ? carry.selfCheck : await readMasterSelfCheck().catch(() => null);
  const secondaryCheckups = carry ? carry.secondaryCheckups : await readSecondaryCheckups(environments);
  return { productId: "trae_cn", trae: { state, checkup, selfCheck, environments, secondaryCheckups } };
}

/** WB 数据面（旧 WorkBuddyEnvironmentPage L31-58）：登录文件面是主读取，其余各自降级。 */
async function readWorkBuddy(mode: ReadMode, previous: unknown): Promise<EnvironmentData> {
  const prior = previous as WorkBuddyEnvironmentData | null;
  const sessionHealth = await loadWorkBuddySessionHealth(); // 文件面只读：失败即整页 error
  // 深度健康是客户端实时回执（IPC 探针有成本）：轻刷新沿用上次结果，不重复探针。
  const deepHealth = mode === "local" ? prior?.deepHealth ?? null : await checkWorkBuddyLoginHealth().catch(() => null);
  const probe = await fetchWorkBuddyProbeInfo().catch(() => null); // 探针独立降级（只藏版本/路径行）
  return { productId: "workbuddy", workbuddy: { sessionHealth, deepHealth, probe } };
}

/** 产品读取表：键 = productId，值 = { 载荷键, 读取, 失败兜底文案 }（新增产品只加一行）。 */
const READERS: Record<string, { key: string; read: typeof readTrae; failure: string }> = {
  trae_cn: { key: "trae", read: readTrae, failure: ENV_TEXTS.loadFailure },
  workbuddy: { key: "workbuddy", read: readWorkBuddy, failure: ENV_TEXTS.wbLoadFailure },
};

export interface EnvironmentSnapshotResult {
  /** 已取回数据面；null = 未读到（加载中 / 已清态 / 未登记产品）。 */
  readonly snapshot: EnvironmentData | null;
  readonly loading: boolean;
  /** 读取失败的人话原因；失败时 snapshot 恒 null（不伪装成「没有环境」）。 */
  readonly error: string | null;
  /** 轻刷新：只校准运行态与列表，零体积遍历、零体检聚合；失败同样落 error。 */
  readonly refreshLocal: () => Promise<void>;
  /** 全量读取（带体积 + 重聚合体检）：修复类动作会改变体检聚合口径，必须走这条（旧页 L157 整组重读）。 */
  readonly refreshFull: () => Promise<void>;
}

/** @param productId 与 registry / 能力表同键的产品；@param active 页面可见才读取。 */
export function useEnvironmentSnapshot(productId: string, active: boolean): EnvironmentSnapshotResult {
  const [snapshot, setSnapshot] = useState<EnvironmentData | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  // 代次守卫：每次读取领票，落地前比对；晚到的旧请求一律丢弃（旧 L166-173 同法）。
  const generation = useRef(0);
  // 渲染期写入的读取快照：load 经 ref 取上一次数据与可见性，state 不进依赖。
  const latest = useRef({ active, snapshot });
  latest.current = { active, snapshot };
  // 产品切换 / 页面隐藏：立即作废在途读取并清态，旧数据与错误不得泄露到新首帧。
  useLayoutEffect(() => {
    generation.current += 1; setSnapshot(null); setError(null); setLoading(false);
  }, [productId, active]);
  const load = useCallback(async (mode: ReadMode) => {
    const reader = READERS[productId];
    if (!reader) return; // 未登记产品 fail-closed：不猜数据面（页面按 registry 空视图渲染）。
    const ticket = ++generation.current;
    // 同产品上次数据（轻刷新沿用字段）经载荷键取，不做产品相等判断。
    const current = latest.current.snapshot;
    const previous = current === null ? null : (current as unknown as Record<string, unknown>)[reader.key];
    setLoading(true); setError(null);
    try {
      const next = await reader.read(mode, previous);
      if (ticket === generation.current) setSnapshot(next);
    } catch (reason: unknown) {
      // 读取失败不得静默成空数组：清快照 + 暴露原因，由页面渲染错误态。
      if (ticket === generation.current) { setSnapshot(null); setError(safeUiErrorMessage(reason, reader.failure)); }
    } finally {
      if (ticket === generation.current) setLoading(false);
    }
  }, [productId]);
  useEffect(() => {
    if (!active) return;
    void load("full");
    return () => { generation.current += 1; }; // 卸载 / 换代作废在途读取
  }, [active, productId, load]);
  const refreshLocal = useCallback(async () => {
    if (!latest.current.active) return;
    await load("local");
  }, [load]);
  // 全量重读只给「体检聚合会变」的动作（修复主库记录）：与旧页 load() 同口径。
  const refreshFull = useCallback(async () => {
    if (!latest.current.active) return;
    await load("full");
  }, [load]);
  return { snapshot, loading, error, refreshLocal, refreshFull };
}
