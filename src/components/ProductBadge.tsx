import type { ReactNode } from "react";

/**
 * 产品徽标（P17-T5 二期/A6：账号页统一视图）——页头等位置的当前产品
 * 标识。产品名用官方名（界面表达纪律），按 product_id 分色：
 * - trae_cn → TRAE 蓝调；
 * - workbuddy → WorkBuddy 紫调；
 * - 未知产品 → 中性灰（fail-soft，不猜测）。
 */
export function ProductBadge({
  productId,
  children,
}: {
  productId: string;
  /** 徽标内文本（缺省 = 产品官方名）；测试可注入短文案。 */
  children?: ReactNode;
}) {
  const tone =
    productId === "trae_cn"
      ? "product-badge--trae"
      : productId === "workbuddy"
        ? "product-badge--workbuddy"
        : "product-badge--neutral";
  const label =
    productId === "trae_cn"
      ? "TRAE"
      : productId === "workbuddy"
        ? "WorkBuddy"
        : productId;
  return (
    <span className={`product-badge ${tone}`} data-testid={`product-badge-${productId}`}>
      {children ?? label}
    </span>
  );
}
