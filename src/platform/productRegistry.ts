import type { WorkspaceStateDto } from "../types/workspace";

/** 产品 ID：产品切换与账号切换使用不同的状态。 */
export type ProductId = "work_cn" | "trae_cn";

/** 当前壳层能识别的导航能力。能力不足时不生成入口。 */
export type ProductCapability =
  | "overview"
  | "accounts"
  | "checkin"
  | "environment"
  | "settings";

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
 * Trae CN 已开放独立账号适配；官方客户端会话恢复未通过隔离实验前不开放
 * 历史、签到和环境等依赖原生客户端状态的能力，避免误读 Work 数据。
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
    displayName: "Trae CN",
    capabilities: ["accounts"],
    credentialStatus: "need_authorization",
    statusLabel: "需要授权",
  },
];

export function productDefinitionOf(productId: ProductId): ProductDefinition {
  return PRODUCT_DEFINITIONS.find((product) => product.id === productId) ?? PRODUCT_DEFINITIONS[0];
}

/**
 * 为尚未开放真实数据读取的产品构造诚实的空上下文。
 * 只清空展示状态，不修改后端 Work CN 状态，也不读取另一产品数据库。
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
    honest_status: "当前产品只开放账号适配；跨产品凭据恢复仍需授权。",
  };
}
