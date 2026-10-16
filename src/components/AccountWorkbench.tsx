import { AccountCenter } from "./AccountCenter";

interface AccountWorkbenchProps {
  /** 页面可见时才读取账号状态。 */
  active: boolean;
}

/**
 * 唯一账号工作台入口。
 *
 * P15-10 单产品塌缩：唯一产品固定使用完整账号事务路径（官方切号五步事务、
 * 签到档案健康/额度/凭据都在这里）；适配器注入仅由 AccountCenter 的测试
 * 场景使用，不再由产品切换触达。
 */
export function AccountWorkbench({ active }: AccountWorkbenchProps) {
  return <AccountCenter active={active} />;
}
