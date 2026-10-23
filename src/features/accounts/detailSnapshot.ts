import type { AccountSnapshot } from "../../platform/accountAdapter";
import type { CheckinOverviewEntryDto } from "../../types/account_switch";
import type { WorkBuddyAccountEntryDto } from "../../types/product";
import type { TraeAccountsData, WorkBuddyAccountsData } from "./components/useAccountsSnapshot";
import type { DetailSnapshot } from "./detailView";

/**
 * 详情数据面装配（P4c-7c-1）：账号页已取回的产品数据 → buildDetailView 要的快照。
 *
 * 纯函数、零 React、零命令：页面只把「载荷键取出的数据面 + 行 id + 判定时刻」交给本文件，
 * 产品差异全部在文件末尾登记表（与 detailView 的 builder 表同形）。找不到目标账号一律
 * 返回 null（不猜数据面），页面据此回列表——与旧骨架「详情查不到就回列表」同口径。
 */

/** 装配入参：payload = 按产品载荷键取出的数据面；adapterPool = 仅测试注入的适配器面。 */
export interface DetailSnapshotInput {
  readonly productId: string;
  readonly payload: unknown;
  readonly rowId: string;
  readonly nowUnixSeconds: number;
  readonly adapterPool?: AccountSnapshot;
}

/** 适配器池面：池内账号 → 旧 AdapterAccountDetail 的入参（工具账号选择忙碌位恒空闲）。 */
function traeDetail(input: DetailSnapshotInput, data: TraeAccountsData): DetailSnapshot | null {
  const poolAccount = input.adapterPool?.accounts.find((account) => account.id === input.rowId);
  if (poolAccount !== undefined) {
    // 池面优先（旧 AccountCenterTrae L886：adapter 存在即走 AdapterAccountDetail）。
    return {
      productId: "trae_cn",
      trae: {
        face: "pool",
        account: poolAccount,
        toolSelectionBusy: false,
      },
    };
  }
  const entry: CheckinOverviewEntryDto | undefined =
    data.overview.find((item) => item.profile_id === input.rowId);
  if (entry === undefined) return null;
  // 当前账号判定（旧 L893-897）：未确认 / 使用中 / 不是当前账号 三态，不臆测。
  const observed = data.observedProfileId;
  return {
    productId: "trae_cn",
    trae: {
      face: "checkin",
      entry,
      loginState: data.loginStates[entry.profile_id],
      currentAccountStatus: observed === null ? "unknown" : observed === entry.profile_id ? "current" : "other",
      nowUnixSeconds: input.nowUnixSeconds,
    },
  };
}

/** WB 数据面：池条目 + 逐账号签到面 + 自动签到参与集（旧 WorkBuddyAccountDetail L62-77 同一数据面）。 */
function workbuddyDetail(input: DetailSnapshotInput, data: WorkBuddyAccountsData): DetailSnapshot | null {
  const account: WorkBuddyAccountEntryDto | undefined =
    data.pool.find((item) => item.profile_id === input.rowId);
  if (account === undefined) return null;
  return {
    productId: "workbuddy",
    workbuddy: {
      account,
      overview: data.checkinOverview.find((row) => row.profile_id === account.profile_id) ?? null,
      // 参与集判定（旧 L69-77 逐字同口径）：在 list_workbuddy_checkin_accounts 结果里按
      // profile_id 命中即取该行 enabled（后端 effective_enabled = 未显式设置默认参与、显式关闭永久 false）；
      // 参与集未读到（null/缺省）或行缺失 = 不可读取，显示中性说明（不猜参与态）。
      participating: data.participation?.find((row) => row.profile_id === account.profile_id)?.enabled ?? null,
      // 当前客户端登录账号（health.uid 与池 account_id 同源）。
      isCurrentLogin: data.sessionHealth !== null && data.sessionHealth.uid === account.account_id,
      nowUnixSeconds: input.nowUnixSeconds,
    },
  };
}

/** 查表分发：键 = productId（与 registry / detailView 同形）；未登记产品 fail-closed 到 null。 */
const BUILDERS: Record<string, (input: DetailSnapshotInput, payload: unknown) => DetailSnapshot | null> = {
  trae_cn: (input, payload) => traeDetail(input, payload as TraeAccountsData),
  workbuddy: (input, payload) => workbuddyDetail(input, payload as WorkBuddyAccountsData),
};

/** 数据面 + 行 id → 详情快照；未登记产品或目标不在本产品数据面内即 null。 */
export function detailSnapshotFor(input: DetailSnapshotInput): DetailSnapshot | null {
  return BUILDERS[input.productId]?.(input, input.payload) ?? null;
}
