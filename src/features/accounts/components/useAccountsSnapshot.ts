import { useCallback, useEffect, useLayoutEffect, useRef, useState } from "react";
import {
  readCheckinCapability, readCheckinOverview, readEnvironmentState, readManagedAccountState, readTraeInstanceStates,
} from "../../../platform/accounts";
import {
  fetchWorkBuddyCheckinOverview, listWorkBuddyAccounts, loadWorkBuddySessionHealth,
} from "../../../platform/workbuddyAdapter";
import { safeUiErrorMessage } from "../../../utils/safeUiError";
import type { TraeAccountsSnapshot } from "../traeAccounts";
import type { WorkBuddyAccountsSnapshot } from "../workbuddyAccounts";

/**
 * 账号页快照读取 hook（P4c-5a）：按产品取数据面、代次守卫防竞态、产品切换或
 * active=false 立即清态、refreshLocal 只重读本机缓存（零网络命令）。只产出数据面
 * ——filter/sort/busy/nowUnixSeconds 由页面与动作 hook 注入后合成 builder 快照。
 * 读取失败不静默成空数组：snapshot 清空、error 如实暴露（旧 L281-286 同纪律）。
 */

/** 数据面 = builder 快照去掉页面自持字段（排序/过滤/忙碌位/判定时刻）。 */
export type TraeAccountsData = Omit<TraeAccountsSnapshot, "filter" | "sort" | "busy" | "nowUnixSeconds">;
export type WorkBuddyAccountsData = Omit<WorkBuddyAccountsSnapshot, "sort" | "busy" | "nowUnixSeconds">;
/** 与 registry.AccountsSnapshot 同形（判别字段 + 同名载荷键）：页面补 UI 字段即得快照。 */
export type AccountsData =
  | { readonly productId: "trae_cn"; readonly trae: TraeAccountsData }
  | { readonly productId: "workbuddy"; readonly workbuddy: WorkBuddyAccountsData };

/** full=完整读取（含网络探测）；local=纯缓存刷新（不触发任何网络命令）。 */
type ReadMode = "full" | "local";

/** TRAE 数据面（旧 AccountCenterTrae L255-313 / L216-226）；traePool 恒 null = 适配器面未启用。 */
async function readTrae(mode: ReadMode, previous: unknown): Promise<AccountsData> {
  const [capability, overview, environment, managed] = await Promise.all([
    readCheckinCapability(), readCheckinOverview(), readEnvironmentState(), readManagedAccountState()]);
  // 登录存档健康度含逐账号凭据实调（有网络成本）：纯缓存刷新保留上次结果，仅
  // 完整读取且真实模式有账号才重探测——旧 L296-313 同一门禁，refreshLocal 不联网。
  const states = mode === "local" || !capability.real_http_enabled || overview.length === 0
    ? null : await readTraeInstanceStates({ profileIds: overview.map((entry) => entry.profile_id) });
  const loginStates = states === null ? (previous as TraeAccountsData | null)?.loginStates ?? {}
    : Object.fromEntries(states.map((state) => [state.profile_id, state]));
  return { productId: "trae_cn", trae: { capability, overview, loginStates,
    observedProfileId: environment.observed_profile_id, traePool: null, poolStatusLabel: null,
    savedAccounts: managed.saved_accounts, savedCurrentProfileId: managed.current_account.profile_id } };
}

/** WB 数据面（旧 WorkBuddyPanel L149-170）。 */
async function readWorkBuddy(mode: ReadMode, previous: unknown): Promise<AccountsData> {
  const [sessionHealth, pool] = await Promise.all([loadWorkBuddySessionHealth(), listWorkBuddyAccounts()]);
  // 逐账号签到/积分面经客户端运行时查询（有网络成本）：纯缓存刷新保留上次结果。
  const checkinOverview = mode === "local"
    ? (previous as WorkBuddyAccountsData | null)?.checkinOverview ?? []
    : await fetchWorkBuddyCheckinOverview(null).catch(() => [] as const);
  return { productId: "workbuddy", workbuddy: { sessionHealth, pool, checkinOverview } };
}

/** 产品读取表：键 = productId，值 = { 载荷键, 读取, 失败兜底文案 }（新增产品只加一行）。 */
const READERS: Record<string, { key: string; read: typeof readTrae; failure: string }> = {
  trae_cn: { key: "trae", read: readTrae, failure: "账号信息暂时不可读取，请稍后重试。" },
  workbuddy: { key: "workbuddy", read: readWorkBuddy, failure: "WorkBuddy 登录状态暂时不可读取，请稍后重试。" },
};

export interface AccountsSnapshotResult {
  /** 已取回数据面；null = 未读到（加载中 / 已清态 / 未登记产品）。 */
  readonly snapshot: AccountsData | null;
  readonly loading: boolean;
  /** 读取失败的人话原因；失败时 snapshot 恒 null（不伪装成「没有账号」）。 */
  readonly error: string | null;
  /** 纯缓存刷新：只重读本机缓存，零网络命令；失败同样落 error。 */
  readonly refreshLocal: () => Promise<void>;
}

/** @param productId 与 registry / 能力表同键的产品；@param active 页面可见才读取。 */
export function useAccountsSnapshot(productId: string, active: boolean): AccountsSnapshotResult {
  const [snapshot, setSnapshot] = useState<AccountsData | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  // 代次守卫：每次读取领票，落地前比对；晚到的旧请求一律丢弃（旧 L204-214 同法）。
  const generation = useRef(0);
  // 渲染期写入的读取快照：load 经 ref 取上一次数据与可见性，state 不进依赖。
  const latest = useRef({ active, snapshot });
  latest.current = { active, snapshot };
  // 产品切换 / 页面隐藏：立即作废在途读取并清态，旧数据与错误不得泄露到新首帧。
  useLayoutEffect(() => {
    generation.current += 1;
    setSnapshot(null);
    setError(null);
    setLoading(false);
  }, [productId, active]);
  const load = useCallback(async (mode: ReadMode) => {
    const reader = READERS[productId];
    if (!reader) return; // 未登记产品 fail-closed：不猜数据面（页面按 registry 空视图渲染）。
    const ticket = ++generation.current;
    // 同产品上次数据（纯缓存刷新沿用字段）经载荷键取，不做产品相等判断。
    const current = latest.current.snapshot;
    const previous = current === null ? null : (current as unknown as Record<string, unknown>)[reader.key];
    setLoading(true);
    setError(null);
    try {
      const next = await reader.read(mode, previous);
      if (ticket === generation.current) setSnapshot(next);
    } catch (reason: unknown) {
      // 读取失败不得静默成空数组：清快照 + 暴露原因，由页面渲染错误态。
      if (ticket === generation.current) {
        setSnapshot(null);
        setError(safeUiErrorMessage(reason, reader.failure));
      }
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
  return { snapshot, loading, error, refreshLocal };
}
