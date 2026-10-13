import type { LibraryRef } from "../components/LibrarySessionsPanel";
import type { WorkspaceStateDto } from "../types/workspace";

/** 产品 ID：产品切换与账号切换使用不同的状态。 */
export type ProductId = "work_cn" | "trae_cn";

/** 当前壳层能识别的导航能力。 */
export type ProductCapability =
  | "overview"
  | "accounts"
  | "checkin"
  | "environment"
  | "settings";

/**
 * 工作区区块键（与导航页面一一对应）。master-library 是详情位：
 * 不生成导航项，从环境页（Work CN）或总览浏览入口（Trae CN）进入。
 */
export type SectionKey =
  | "overview"
  | "accounts"
  | "checkin"
  | "environment"
  | "master-library"
  | "settings";

/**
 * 区块能力三态（ADR-0032 决策 2）：页面本体一致，差异只允许来自能力事实——
 * ready = 已适配，渲染真实数据与操作；
 * pending = 适配在途，渲染诚实说明，不渲染操作；
 * absent = 协议事实不存在，渲染「该产品无此能力」。
 */
export type CapabilityState = "ready" | "pending" | "absent";

/**
 * 每产品区块三态（ADR-0032 决策 2：分支判定依据能力清单与探针结论，
 * 组件内不散落 productId 判断）。
 * 事实依据：Trae CN 账号适配已落地（P10）；签到协议端点已由只读探针
 * 验证存在（.scratch/checkin-http/reports/trae-cn-checkin-probe-20260928.json）
 * 且 P13-1 已实装单账号手动签到（status/claim；claim 形状待 P13-4 真机
 * 验收回写）→ ready；环境体系是 Work CN 产品能力 → absent；
 * Trae CN 数据根只读浏览由后端 library_id 参数化读路径承载 → ready。
 */
export const PRODUCT_SECTION_STATES: Record<ProductId, Record<SectionKey, CapabilityState>> = {
  work_cn: {
    overview: "ready",
    accounts: "ready",
    checkin: "ready",
    environment: "ready",
    "master-library": "ready",
    settings: "ready",
  },
  trae_cn: {
    overview: "pending",
    accounts: "ready",
    checkin: "ready",
    environment: "absent",
    "master-library": "ready",
    settings: "ready",
  },
};

/** 查询某产品某区块的能力三态。 */
export function sectionStateOf(productId: ProductId, section: SectionKey): CapabilityState {
  return PRODUCT_SECTION_STATES[productId][section];
}

/** Trae CN 只读库实例 id（与后端写守卫同名的库引用；ADR-0032 决策 5）。 */
export const TRAE_CN_LIBRARY_ID = "trae_cn";

/** 是否只读库实例：只读库的写操作控件一律隐藏（不是禁用）。 */
export function isReadOnlyLibrary(libraryId: string): boolean {
  return libraryId === TRAE_CN_LIBRARY_ID;
}

/**
 * 主库详情页各产品的库实例（ADR-0032 决策 5）：
 * Trae CN 固定浏览自己的只读数据根；Work CN 沿用环境页入口注入的
 * 库实例（主库/副库），无注入时缺省主库。
 */
export function detailLibraryRefFor(productId: ProductId, injected: LibraryRef | null): LibraryRef {
  if (productId === "trae_cn") return { id: TRAE_CN_LIBRARY_ID };
  return injected ?? { id: "master" };
}

/** 产品凭据的非敏感展示状态。不要把它等同于账号身份。 */
export type ProductCredentialStatus =
  | "verified"
  | "identity_recognized"
  | "need_authorization"
  | "experimental";

export interface ProductDefinition {
  readonly id: ProductId;
  readonly displayName: string;
  readonly capabilities: readonly ProductCapability[];
  readonly credentialStatus: ProductCredentialStatus;
  readonly statusLabel: string;
}

/**
 * 最小产品注册表：产品事实集中在适配边界，页面不再散落 productId 判断。
 * ADR-0032 起导航五页对所有产品开放（页面本体一致、区块按能力三态呈现，
 * 见 PRODUCT_SECTION_STATES）；capabilities 语义 = 该产品可驻留的导航页，
 * 用于产品切换时校验记忆页，不再承担「隐藏页面」职责。
 */
export const PRODUCT_DEFINITIONS: readonly ProductDefinition[] = [
  {
    id: "work_cn",
    displayName: "TRAE Work CN",
    capabilities: ["overview", "accounts", "checkin", "environment", "settings"],
    credentialStatus: "verified",
    statusLabel: "已支持",
  },
  {
    id: "trae_cn",
    // 融合版 TRAE（Work CN 升级版）为唯一产品目标（ADR-0035）：trae_cn
    // 产品 ID 保持稳定（后端/持久化引用不变），展示名用官方产品名。
    displayName: "TRAE",
    capabilities: ["overview", "accounts", "checkin", "environment", "settings"],
    credentialStatus: "verified",
    statusLabel: "已支持",
  },
];

export function productDefinitionOf(productId: ProductId): ProductDefinition {
  return PRODUCT_DEFINITIONS.find((product) => product.id === productId) ?? PRODUCT_DEFINITIONS[0];
}

/** Trae CN 只读库详情页标题（界面表达纪律：描述对象对用户是什么就叫什么）。 */
export const TRAE_CN_LIBRARY_TITLE = `${productDefinitionOf("trae_cn").displayName} 数据根`;

/**
 * 为尚未开放真实数据读取的产品构造诚实的空上下文。
 * 只清空展示状态，不修改后端 Work CN 状态，也不读取另一产品数据库。
 * ADR-0032 起区块级差异由能力三态承载，本函数只负责工作区状态壳层数据。
 */
export function workspaceStateForProduct(
  state: WorkspaceStateDto,
  product: ProductDefinition,
): WorkspaceStateDto {
  if (product.id === "work_cn") return state;

  return {
    ...state,
    platform: {
      platform_id: product.id,
      display_name: product.displayName,
      adapter_implemented: false,
    },
    data_location: {
      selected: false,
      display_name: null,
      unavailable_reason: "product_authorization_required",
    },
    current_account: {
      detected: false,
      user_fingerprint: null,
      unavailable_reason: "need_authorization",
    },
    history: {
      account_count: 0,
      project_count: 0,
      session_count: 0,
    },
    capabilities: {
      scan_enabled: false,
      sync_enabled: false,
      backup_enabled: false,
      restore_enabled: false,
    },
    honest_status: "Trae CN 支持账号管理与数据只读浏览；其余能力按区块适配状态开放。",
  };
}
