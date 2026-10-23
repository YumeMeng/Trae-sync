import type { MasterCheckupDto } from "../../../types/masterLibrary";
import { ENV_TEXTS } from "../config";
import { deriveCheckupView } from "../traeEnvironment";

/**
 * 收编确认弹层规模（P4d-8a-2c）：旧页 IncorporateDialog 的确认态入参（L1678-1692）。
 * 全部取自 deriveCheckupView（L1576-1591），本文件不另算任何计数——滞留账号/会话/项目与
 * 无归属条目的口径必须与体检区块同源，否则确认句与体检句会对不上。
 */

/** 确认态规模：滞留账号行（账号名是主信息，id 只进 key 与悬浮提示）+ 三项合计 + 无归属。 */
export interface IncorporateScale {
  readonly staleAccounts: readonly { readonly id: string; readonly name: string }[];
  readonly staleSessionTotal: number;
  readonly staleProjectTotal: number;
  readonly orphanProjectCount: number;
}

/** 空规模兜底（未登记产品 / 数据面缺失）：确认句按零渲染，绝不猜数。 */
export const EMPTY_INCORPORATE_SCALE: IncorporateScale = {
  staleAccounts: [], staleSessionTotal: 0, staleProjectTotal: 0, orphanProjectCount: 0,
};

/** 体检报告 → 确认规模；报告不可读（null / 非 ready）按零规模（确认句仍按实际数据渲染）。 */
export function scaleOfCheckup(report: MasterCheckupDto | null): IncorporateScale {
  const view = deriveCheckupView(report);
  return {
    staleAccounts: view.staleAccounts.map((account) => ({
      id: account.user_id, name: account.account_name ?? ENV_TEXTS.accountUnregistered,
    })),
    staleSessionTotal: view.staleSessionTotal,
    staleProjectTotal: view.staleProjectTotal,
    orphanProjectCount: report?.status === "ready" ? report.orphan_project_count : 0,
  };
}
