import { invoke } from "@tauri-apps/api/core";
import type { CheckinCapabilityDto, CheckinOverviewEntryDto } from "../types/account_switch";
import type { EnvironmentStateDto } from "../types/environment";
import type { MasterLibraryStatsDto } from "../types/masterLibrary";
import type {
  WorkBuddyCheckinAccountStatusDto,
  WorkBuddyCheckinParticipationDto,
} from "../types/product";
import type { WorkBuddyLibraryStatsDto } from "../types/workbuddyLibrary";
import {
  fetchWorkBuddyCheckinOverview,
  fetchWorkBuddyLibraryStats,
  listWorkBuddyCheckinAccounts,
} from "./workbuddyAdapter";

/**
 * 总览页只读数据面（P4a）：两个产品的总览数据各封装一次读取，
 * 页面不出现 invoke(、不出现命令名。全部读取 fail-soft——
 * 读不到的区块由 features/overview/stats.ts 的声明式配置降级呈现。
 */

/** TRAE 总览快照：签到能力 + 签到摘要 + 主库账号 + 主库聚合统计。 */
export interface TraeOverviewSnapshot {
  /** 真实签到能力是否开启（false = 不读取签到摘要，不出待签到提示）。 */
  readonly checkinReal: boolean;
  /** 签到摘要行（未开启或读取失败为 null）。 */
  readonly checkinRows: readonly CheckinOverviewEntryDto[] | null;
  /** 主库环境状态（当前登录账号；读取失败为 null）。 */
  readonly environment: EnvironmentStateDto | null;
  /** 主库聚合统计（读取失败为 null）。 */
  readonly masterStats: MasterLibraryStatsDto | null;
}

/** WorkBuddy 总览快照：参与集 + 逐账号签到状态 + 对话库聚合统计。 */
export interface WorkBuddyOverviewSnapshot {
  readonly participation: readonly WorkBuddyCheckinParticipationDto[] | null;
  readonly checkinRows: readonly WorkBuddyCheckinAccountStatusDto[] | null;
  readonly libraryStats: WorkBuddyLibraryStatsDto | null;
}

/** 总览快照（按产品数据面区分；kind 即产品数据域，不是产品分支判断）。 */
export type OverviewSnapshot =
  | { readonly kind: "trae"; readonly trae: TraeOverviewSnapshot }
  | { readonly kind: "workbuddy"; readonly workbuddy: WorkBuddyOverviewSnapshot };

/** 读取 TRAE 总览数据（三路并行；签到摘要按能力开关决定是否发起）。 */
async function readTraeOverview(): Promise<OverviewSnapshot> {
  const [capability, environment, masterStats] = await Promise.all([
    invoke<CheckinCapabilityDto>("get_checkin_capability").catch(() => null),
    invoke<EnvironmentStateDto>("get_environment_state").catch(() => null),
    invoke<MasterLibraryStatsDto>("get_master_library_stats").catch(() => null),
  ]);
  const checkinReal = capability?.real_http_enabled === true;
  const checkinRows = checkinReal
    ? await invoke<readonly CheckinOverviewEntryDto[]>("get_checkin_overview").catch(() => null)
    : null;
  return { kind: "trae", trae: { checkinReal, checkinRows, environment, masterStats } };
}

/** 读取 WorkBuddy 总览数据（三路并行；单路失败只降级该区块）。 */
async function readWorkBuddyOverview(): Promise<OverviewSnapshot> {
  const [participation, checkinRows, libraryStats] = await Promise.allSettled([
    listWorkBuddyCheckinAccounts(),
    fetchWorkBuddyCheckinOverview(null),
    fetchWorkBuddyLibraryStats(),
  ]);
  return {
    kind: "workbuddy",
    workbuddy: {
      participation: participation.status === "fulfilled" ? participation.value : null,
      checkinRows: checkinRows.status === "fulfilled" ? checkinRows.value : null,
      libraryStats: libraryStats.status === "fulfilled" ? libraryStats.value : null,
    },
  };
}

/**
 * 产品 → 总览数据读取器（声明式登记；新增产品只加一行，页面一行不改）。
 * 未登记产品回退首个登记项（与槽位解析的 fail-soft 方向一致）。
 */
const OVERVIEW_READERS: Record<string, () => Promise<OverviewSnapshot>> = {
  trae_cn: readTraeOverview,
  workbuddy: readWorkBuddyOverview,
};

const DEFAULT_READER = OVERVIEW_READERS.trae_cn;

/** 读取指定产品的总览快照（fail-soft：未登记产品按 TRAE 数据面读取）。 */
export function loadOverview(productId: string): Promise<OverviewSnapshot> {
  return (OVERVIEW_READERS[productId] ?? DEFAULT_READER)();
}
