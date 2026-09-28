import type { ProductId } from "../platform/productRegistry";

/** 后端只读身份发现结果；不包含 userId、token 或 native storage。 */
export interface ProductIdentityStateDto {
  readonly product_id: ProductId;
  readonly display_name: string;
  readonly identity_status: "recognized" | "unknown" | "conflict";
  readonly credential_status: "managed_by_work" | "need_authorization";
  readonly relation_to_work: "current_product" | "same_identity" | "different_identity" | "unknown";
}

/** Trae CN 独立账号池中的非敏感展示档案。 */
export interface TraeCnAccountDto {
  readonly profile_id: string;
  readonly display_name: string;
  readonly avatar_url: string;
  readonly last_verified_at: string | null;
  readonly status: "active" | "expired" | "unknown";
  readonly is_current: boolean;
}

/** Trae CN 健康检测逐账号回执；不包含令牌或账号 ID。 */
export interface TraeCnHealthEntryDto {
  readonly profile_id: string;
  readonly screen_name: string;
  readonly healthy: boolean;
  readonly error_code: string | null;
}

/** Trae CN 凭据续期逐账号回执；不包含新旧令牌。 */
export interface TraeCnCredentialRefreshEntryDto {
  readonly profile_id: string;
  readonly screen_name: string;
  readonly refreshed: boolean;
  readonly error_code: string | null;
}

/** 产品 OAuth 开始命令的最小返回值。 */
export interface ProductLoginBeginDto {
  readonly login_url: string;
}

/** Trae CN OAuth 完成回执；不把账号 ID 或令牌交给前端。 */
export interface TraeCnLoginReceiptDto {
  readonly profile_id: string;
  readonly screen_name: string;
  readonly avatar_url: string;
}

/**
 * 远程设备行（ADR-0031）：与后端 RemoteDeviceEntryDto 对齐（snake_case）。
 * device_id 仅用于退出定位与内部流转，UI 不渲染（界面表达纪律）。
 */
export interface RemoteDeviceEntry {
  readonly device_id: string;
  readonly device_type: string | null;
  readonly device_name: string | null;
  readonly bound_products: readonly string[];
  /** 服务端原样保留：毫秒时间戳 / 字符串 / null，格式化交给 UI 层。 */
  readonly last_active_at: number | string | null;
  readonly is_local: boolean;
}

/** 远程设备列表快照：账号归属标注 + 全量设备行 + 已用/上限摘要。 */
export interface RemoteDeviceSnapshot {
  readonly profile_id: string;
  readonly account_label: string;
  readonly devices: readonly RemoteDeviceEntry[];
  readonly used_count: number;
  readonly max_count: number | null;
}
