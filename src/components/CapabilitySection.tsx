import { CircleSlash, Hourglass } from "lucide-react";
import type { ReactNode } from "react";
import type { CapabilityState, ProductId } from "../platform/productRegistry";
import { EmptyState } from "./EmptyState";

// ============================================================================
// 区块能力三态渲染（P11-6，ADR-0032 决策 2）：
// ready 渲染真实内容；pending/absent 渲染诚实说明卡（毛玻璃 + 靛蓝点缀），
// 卡上带 data-state / data-product / data-testid 供三态断言。
// 分支判定依据产品注册表能力清单，由调用方传入 state，本组件不做产品判断。
// ============================================================================

/** pending/absent 的缺省卡文案与图标（标题可由调用方覆盖）。 */
const STATE_COPY: Record<
  Exclude<CapabilityState, "ready">,
  { icon: typeof Hourglass; title: string }
> = {
  pending: { icon: Hourglass, title: "该能力适配中" },
  absent: { icon: CircleSlash, title: "该产品无此能力" },
};

export interface CapabilitySectionProps {
  /** 当前产品在该区块的能力三态（来自 sectionStateOf）。 */
  state: CapabilityState;
  /** 当前产品（承载在卡的数据属性上，供每产品每区块断言）。 */
  product: ProductId;
  /** pending/absent 卡标题覆盖（如签到区块的「签到能力适配中」）。 */
  title?: string;
  /** pending/absent 的补充说明行（可选）。 */
  note?: string;
  children?: ReactNode;
}

export function CapabilitySection({ state, product, title, note, children }: CapabilitySectionProps) {
  // ready：无包裹直接渲染，避免额外容器破坏页面网格布局。
  if (state === "ready") return <>{children}</>;

  const copy = STATE_COPY[state];
  const Icon = copy.icon;
  return (
    <div
      className="capability-section"
      data-state={state}
      data-product={product}
      data-testid={`capability-${state}`}
    >
      <EmptyState panel icon={<Icon size={26} strokeWidth={1.6} />} title={title ?? copy.title} description={note} />
    </div>
  );
}
