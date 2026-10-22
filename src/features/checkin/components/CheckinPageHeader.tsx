import type { ReactNode } from "react";
import { ProductBadge } from "../../../components/shared/ProductBadge";

/**
 * 签到页页头（P4b-3）：标题 + 产品徽章 + 动作区插槽。
 *
 * 与旧骨架（CheckinPageSkeleton）页头同构：产品徽章由槽位注入的 productId
 * 决定，页面内不做产品判断；标题带 data-page-title 供导航移焦。
 */
export interface CheckinPageHeaderProps {
  /** 当前产品协议值（槽位注入；只用于徽章着色，不做布局分支）。 */
  productId: string;
  /** 页头动作区（刷新等）。 */
  action?: ReactNode;
}

export function CheckinPageHeader({ productId, action }: CheckinPageHeaderProps) {
  return (
    <header className="page-header">
      <div className="page-header__copy">
        <h1 data-page-title="checkin" tabIndex={-1}>签到</h1>
        <ProductBadge productId={productId} />
      </div>
      {action}
    </header>
  );
}
