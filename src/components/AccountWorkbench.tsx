import type { AccountAdapter } from "../platform/accountAdapter";
import { AccountCenter } from "./AccountCenter";

interface AccountWorkbenchProps {
  /** 页面可见时才读取当前产品的账号状态。 */
  active: boolean;
  /** 产品差异由适配器承载；null 表示沿用 Work 的成熟账号事务。 */
  adapter: AccountAdapter | null;
}

/**
 * 唯一账号工作台入口。
 *
 * AccountCenter 仍保留文件名以兼容现有测试与 Work 详情逻辑；产品切换不再
 * 在 App 中选择两套账号页面，Trae CN 只通过 AccountAdapter 注入账号数据和动作。
 */
export function AccountWorkbench({ active, adapter }: AccountWorkbenchProps) {
  return <AccountCenter active={active} adapter={adapter} />;
}
