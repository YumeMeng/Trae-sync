import type { ProductId } from "../platform/productRegistry";
import type { CheckinOutcomeDto } from "./account_switch";

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

/**
 * Trae CN 签到状态（P13-1；探针实测字段全集）。
 * business_code 按红线透传不压平；did_checked_in/extra_credits 为 CN 特有
 * 字段（语义待 P13-4 真机验收），缺席为 null。
 */
export interface TraeCnCheckinStatusDto {
  readonly enable: boolean;
  readonly checked_in: boolean;
  readonly credits: number | null;
  readonly did_checked_in: boolean | null;
  readonly extra_credits: number | null;
  readonly message: string;
  readonly business_code: number;
}

/** Trae CN 手动签到回执；outcome 与 Work 结果行同口径（业务码在 detail_code 透传）。 */
export interface TraeCnCheckinResultDto {
  readonly profile_id: string;
  readonly outcome: CheckinOutcomeDto;
  readonly detail_code: string | null;
  readonly credits: number | null;
  readonly extra_credits: number | null;
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
  /** 首次登录可持续性验收结果（ADR-0027）：与 Work 回执同一套语义。 */
  readonly acceptance: "passed" | "failed";
  /** 验收失败的稳定错误码；通过时后端省略该字段。 */
  readonly acceptance_error_code?: string;
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
