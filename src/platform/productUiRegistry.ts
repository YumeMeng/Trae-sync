import type { CapabilityState, SectionKey } from "./productRegistry";

/**
 * 前端静态 UI 能力注册表（P19-B1 步骤③，ADR-0036 三原则之三「能力开关」）。
 *
 * 产品差异只体现在"有没有这个数据/功能"，绝不体现在布局与交互：
 * 骨架组件按 capabilities 决定区块渲染形态，禁止 productId 整页分支。
 * UI 能力是纯前端事实，故静态声明不经 IPC；后端 ProductDescriptor
 * 只在需要 fail-closed 校验处使用。区块三态与 productRegistry 的
 * sectionStatesFor 并表（单一事实源，避免第三处产品分支）。
 */

/** 账号页/签到页的 UI 能力开关（骨架按此裁剪区块与控件）。 */
export interface ProductUiCapabilities {
  /** 添加账号路径：oauth-browser=浏览器 OAuth 链；capture=本地捕获；none=未适配。 */
  addAccount: "oauth-browser" | "capture" | "none";
  /** 签到形态：batch-rows=逐账号批量行；single=单账号卡；none=未适配。 */
  checkin: "batch-rows" | "single" | "none";
  /** 账号行是否展示模型积分列（额度块）。 */
  creditsColumn: boolean;
  /** 健康检测形态：probe+network=存档+网络双段；deep=深度检测；none=未适配。 */
  health: "probe+network" | "deep" | "none";
  /** G14 需处理过滤（过滤器行能力开关）。 */
  attentionFilter: boolean;
  /** 账号详情页形态：full=完整详情视图；pool-only=池化精简详情；none=无。 */
  detailView: "full" | "pool-only" | "none";
  /** 排序是否含「签到状态」档。 */
  sortCheckin: boolean;
}

/** 产品 UI 上下文：骨架组件的产品差异化唯一入口。 */
export interface ProductUiContext {
  /** 协议值，与后端 descriptor.product_id 同源。 */
  productId: string;
  displayName: string;
  capabilities: ProductUiCapabilities;
  /** 六区块能力三态（ADR-0032）：与 productRegistry.sectionStatesFor 并表。 */
  sectionStates: Record<SectionKey, CapabilityState>;
}

/** TRAE CN：六区块全 ready；账号页全能力（U-2/U-3/G14/G15 均已落地）。 */
const TRAE_CN_UI: ProductUiContext = {
  productId: "trae_cn",
  displayName: "TRAE",
  capabilities: {
    addAccount: "oauth-browser",
    checkin: "batch-rows",
    creditsColumn: true,
    health: "probe+network",
    attentionFilter: true,
    detailView: "full",
    sortCheckin: true,
  },
  sectionStates: {
    overview: "ready",
    accounts: "ready",
    checkin: "ready",
    environment: "ready",
    "master-library": "ready",
    settings: "ready",
  },
};

/**
 * WorkBuddy（预置条目，P19-B2 消费）：账号/签到 ready（单账号卡形态）；
 * 总览/环境/主库 pending 与 productRegistry 原声明一致。
 * B1 阶段 WB 尚未消费骨架，此处先冻结能力开关声明。
 */
const WORKBUDDY_UI: ProductUiContext = {
  productId: "workbuddy",
  displayName: "WorkBuddy",
  capabilities: {
    addAccount: "capture",
    checkin: "single",
    creditsColumn: false,
    health: "deep",
    attentionFilter: false,
    detailView: "pool-only",
    sortCheckin: false,
  },
  sectionStates: {
    overview: "pending",
    accounts: "ready",
    checkin: "ready",
    environment: "pending",
    "master-library": "pending",
    settings: "ready",
  },
};

export const PRODUCT_UI_REGISTRY: Record<string, ProductUiContext> = {
  trae_cn: TRAE_CN_UI,
  workbuddy: WORKBUDDY_UI,
};

/** 未注册产品 fail-closed 回退：能力全关 + 区块全 pending（settings 除外）。 */
const MINIMAL_CAPABILITIES: ProductUiCapabilities = {
  addAccount: "none",
  checkin: "none",
  creditsColumn: false,
  health: "none",
  attentionFilter: false,
  detailView: "none",
  sortCheckin: false,
};

const MINIMAL_SECTION_STATES: Record<SectionKey, CapabilityState> = {
  overview: "pending",
  accounts: "pending",
  checkin: "pending",
  environment: "pending",
  "master-library": "pending",
  settings: "ready",
};

/** 查询产品 UI 上下文；未注册产品 fail-closed 回退 minimal（不猜测能力）。 */
export function productUi(productId: string): ProductUiContext {
  const registered = PRODUCT_UI_REGISTRY[productId];
  if (registered) return registered;
  return {
    productId,
    displayName: productId,
    capabilities: MINIMAL_CAPABILITIES,
    sectionStates: MINIMAL_SECTION_STATES,
  };
}
