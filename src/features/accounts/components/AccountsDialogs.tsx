import type { ReactNode } from "react";
import { AccountLoginDialog } from "./AccountLoginDialog";
import type { AccountLoginFlow } from "./useAccountLoginFlow";

/**
 * 账号页弹层槽（P4c-6b-2e-3 抽出）：登录弹层 + G11 补录弹层 + G11 查重确认层。
 *
 * 旧实现把这三层统一渲染在列表与详情两个视图里（AccountCenterTrae L775-812 / L1196-1198），
 * 本组件承载同一落位；children = 调用方追加层（详情页的凭据换发确认与重置设备确认）。
 * 零状态、零命令、零产品判断：节点全部由调用方（页面/hook）注入。
 */
export function AccountsDialogs({ login, duplicate, children }: {
  /** 登录流程（弹层属性 + 补录弹层节点；未登记产品时两者恒 null）。 */
  readonly login: AccountLoginFlow;
  /** G11 查重确认层节点（未命中即 null）。 */
  readonly duplicate: ReactNode;
  /** 追加层（详情页的凭据换发 / 重置设备确认）。 */
  readonly children?: ReactNode;
}) {
  return (
    <>
      {login.dialog !== null && <AccountLoginDialog {...login.dialog} />}
      {login.backfill}
      {duplicate}
      {children}
    </>
  );
}
