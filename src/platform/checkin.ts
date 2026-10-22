import { invoke } from "@tauri-apps/api/core";
import type {
  AutoCheckinStatusDto,
  CheckinCapabilityDto,
  CheckinOverviewEntryDto,
  ManagedAccountsViewDto,
} from "../types/account_switch";
import { CURRENT_PRODUCT_ID } from "../types/product";
import type {
  WorkBuddyCheckinAccountStatusDto,
  WorkBuddyCheckinParticipationDto,
} from "../types/product";
import { safeUiErrorMessage } from "../utils/safeUiError";
import {
  claimWorkBuddyCheckinBatch,
  fetchWorkBuddyAutoCheckinStatus,
  fetchWorkBuddyCheckinOverview,
  listWorkBuddyCheckinAccounts,
  type WorkBuddyAutoCheckinStatusDto,
} from "./workbuddyAdapter";

/**
 * 签到页数据面（P4b-2）：两产品的签到读取/执行各封装一次，页面不出现
 * invoke(、不出现命令名。全部读取 fail-soft——读不到由 features/checkin
 * 的声明式配置降级呈现（错误文案已在此映射成用户可读口径）。
 */

/** 统一批次结果（两产品回执归一：逐账号 outcome/detail_code + 计数）。 */
export interface CheckinBatchOutcome {
  readonly total: number;
  readonly completed: number;
  readonly failed: number;
  readonly results: readonly {
    readonly profile_id: string;
    readonly outcome: string;
    readonly detail_code: string | null;
  }[];
}

/** fixture 演示模式的签到池条目（无服务端状态，仅名字）。 */
export interface CheckinFixturePoolEntry {
  readonly profile_id: string;
  readonly display_name: string;
}

/** TRAE 签到快照：能力 + 账号总览（或演示池）+ 自动签到台账。 */
export interface TraeCheckinSnapshot {
  readonly capability: CheckinCapabilityDto | null;
  readonly overview: readonly CheckinOverviewEntryDto[] | null;
  readonly fixturePool: readonly CheckinFixturePoolEntry[] | null;
  readonly auto: AutoCheckinStatusDto | null;
  readonly error: string | null;
}

/** WorkBuddy 签到快照：逐账号状态 + 参与集 + 自动签到台账。 */
export interface WorkBuddyCheckinSnapshot {
  readonly rows: readonly WorkBuddyCheckinAccountStatusDto[] | null;
  readonly participation: readonly WorkBuddyCheckinParticipationDto[] | null;
  readonly auto: WorkBuddyAutoCheckinStatusDto | null;
  readonly error: string | null;
}

/** 签到快照（kind = 数据域，不是布局分支）。 */
export type CheckinSnapshot =
  | { readonly kind: "trae"; readonly trae: TraeCheckinSnapshot }
  | { readonly kind: "workbuddy"; readonly workbuddy: WorkBuddyCheckinSnapshot };

/** 读取 TRAE 签到数据：能力 → 真实总览 / 演示池；自动签到台账并行读取。 */
async function readTraeCheckin(): Promise<CheckinSnapshot> {
  const capability = await invoke<CheckinCapabilityDto>("get_checkin_capability").catch(() => null);
  const real = capability?.real_http_enabled === true;
  const fixture = capability?.transport === "fixture";
  const auto = await invoke<AutoCheckinStatusDto>("get_auto_checkin_settings").catch(() => null);
  let overview: readonly CheckinOverviewEntryDto[] | null = null;
  let fixturePool: readonly CheckinFixturePoolEntry[] | null = null;
  let error: string | null = null;
  if (real) {
    try {
      overview = await invoke<readonly CheckinOverviewEntryDto[]>("get_checkin_overview");
    } catch (reason: unknown) {
      // 读取失败必须可见：静默置空会把真实故障伪装成「还没有账号」。
      error = safeUiErrorMessage(reason, "账号总览读取失败。");
    }
  } else if (fixture) {
    try {
      const view = await invoke<ManagedAccountsViewDto>("get_managed_account_state");
      fixturePool = view.saved_accounts.map((account) => ({
        profile_id: account.profile_id,
        display_name: account.display_name,
      }));
    } catch {
      fixturePool = [];
    }
  }
  return { kind: "trae", trae: { capability, overview, fixturePool, auto, error } };
}

/** 读取 WorkBuddy 签到数据：逐账号状态批查 + 参与集 + 台账，单路失败只降级该区块。 */
async function readWorkBuddyCheckin(): Promise<CheckinSnapshot> {
  const [rows, participation, auto] = await Promise.allSettled([
    fetchWorkBuddyCheckinOverview(null),
    listWorkBuddyCheckinAccounts(),
    fetchWorkBuddyAutoCheckinStatus(),
  ]);
  return {
    kind: "workbuddy",
    workbuddy: {
      rows: rows.status === "fulfilled" ? rows.value : null,
      participation: participation.status === "fulfilled" ? participation.value : null,
      auto: auto.status === "fulfilled" ? auto.value : null,
      error: rows.status === "rejected"
        ? safeUiErrorMessage(rows.reason, "签到状态暂时不可读取，请稍后重试。")
        : null,
    },
  };
}

/** 产品 → 签到读取器（声明式登记；新增产品只加一行，页面一行不改）。 */
const CHECKIN_READERS: Record<string, () => Promise<CheckinSnapshot>> = {
  [CURRENT_PRODUCT_ID]: readTraeCheckin,
  workbuddy: readWorkBuddyCheckin,
};

const DEFAULT_READER = CHECKIN_READERS[CURRENT_PRODUCT_ID];

/** 读取指定产品的签到快照（未登记产品回退首个登记项，与总览同方向）。 */
export function loadCheckinSnapshot(productId: string): Promise<CheckinSnapshot> {
  return (CHECKIN_READERS[productId] ?? DEFAULT_READER)();
}

/** TRAE 批量签到（串行 status→claim→status，后端保证账号间错峰）。 */
async function runTraeCheckin(ids: readonly string[]): Promise<CheckinBatchOutcome> {
  const summary = await invoke<{
    readonly total: number;
    readonly completed: number;
    readonly failed: number;
    readonly results: readonly { profile_id: string; outcome: string; detail_code: string | null }[];
  }>("run_checkin", { profileIds: [...ids] });
  return {
    total: summary.total,
    completed: summary.completed,
    failed: summary.failed,
    results: summary.results,
  };
}

/** WorkBuddy 批量签到（后端顺序执行、fail-soft 逐条回执；单签也走批次通道）。 */
async function runWorkBuddyCheckin(ids: readonly string[]): Promise<CheckinBatchOutcome> {
  const items = await claimWorkBuddyCheckinBatch(ids);
  const failed = items.filter(
    (item) => item.outcome !== "claimed" && item.outcome !== "already_checked_in",
  ).length;
  return {
    total: items.length,
    completed: items.length - failed,
    failed,
    results: items.map((item) => ({
      profile_id: item.profile_id,
      outcome: item.outcome,
      detail_code: item.detail_code,
    })),
  };
}

/** 产品 → 批量签到执行器（声明式登记）。 */
const CHECKIN_RUNNERS: Record<string, (ids: readonly string[]) => Promise<CheckinBatchOutcome>> = {
  [CURRENT_PRODUCT_ID]: runTraeCheckin,
  workbuddy: runWorkBuddyCheckin,
};

/** 执行一次签到批次（ids 顺序即执行顺序）。 */
export function runCheckin(productId: string, ids: readonly string[]): Promise<CheckinBatchOutcome> {
  return (CHECKIN_RUNNERS[productId] ?? CHECKIN_RUNNERS[CURRENT_PRODUCT_ID])(ids);
}

/** 取消未开始的签到任务（后端整体批次不可中断的产品为空操作）。 */
export async function cancelCheckin(productId: string): Promise<void> {
  if (productId === CURRENT_PRODUCT_ID) {
    await invoke("cancel_checkin").catch(() => undefined);
  }
}
