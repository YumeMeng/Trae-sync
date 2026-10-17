/**
 * 当前产品协议标识（P15-10 单产品塌缩，ADR-0035）：融合版 TRAE 是唯一产品目标，
 * 需要产品标识处使用此常量，不再散落字面量。后端命令参数与 product-state
 * 指针值沿用同一字符串（协议值稳定）。
 */
export const CURRENT_PRODUCT_ID = "trae_cn";

/**
 * WorkBuddy 产品协议标识（P17-T5 并列骨架）：与后端 descriptor 注册表
 * 同源（product_descriptor.rs WORKBUDDY_DESCRIPTOR）；前端分支渲染使用。
 */
export const WORKBUDDY_PRODUCT_ID = "workbuddy";

/** 产品注册表条目（切换器数据源；后端 descriptor 静态清单）。 */
export interface ProductEntryDto {
  readonly product_id: string;
  readonly display_name: string;
}

/** 当前产品指针（product-state 文件；未初始化缺省 trae_cn）。 */
export interface CurrentProductDto {
  readonly product_id: string;
  readonly display_name: string;
}

/** WorkBuddy 客户端登录会话状态（后端 current_session_status 判定）。 */
export type WorkBuddySessionStatus =
  | "active"
  | "access_expired"
  | "fully_expired"
  | "logged_out"
  | "file_missing"
  | "unknown";

/** WorkBuddy 会话健康（文件面只读；不含令牌材料）。 */
export interface WorkBuddySessionHealthDto {
  readonly status: WorkBuddySessionStatus;
  /** 过期时间为 Unix 秒（0 = 未知）。 */
  readonly access_expires_at_unix_seconds: number;
  readonly refresh_expires_at_unix_seconds: number;
  readonly domain: string;
  /** 该账号是否已捕获入工具账号池。 */
  readonly captured_in_pool: boolean;
}

/** WorkBuddy 签到状态（脱敏；不含令牌）。 */
export interface WorkBuddyCheckinStatusDto {
  readonly today_checked_in: boolean;
  readonly streak_days: number;
  readonly daily_credit: number;
  readonly total_credits: number;
}

/** WorkBuddy 签到领取回执（verified = 复核翻转确认真实入账）。 */
export interface WorkBuddyCheckinClaimDto {
  readonly credit: number;
  readonly streak_days: number;
  readonly verified: boolean;
}

/** WorkBuddy 账号池脱敏清单行（不含令牌；profile_id 惯例 wb-{uid}）。 */
export interface WorkBuddyLoginSummaryDto {
  readonly profile_id: string;
  readonly account_id: string;
  readonly screen_name: string;
  readonly domain: string;
  readonly access_token_expires_at_unix_seconds: number;
  readonly refresh_token_expires_at_unix_seconds: number;
  readonly minted_at_unix_seconds: number;
  readonly last_updated_at_unix_seconds: number;
}

export type OperationReadiness = "ready" | "pending" | "absent";

/** checkin.claim 已随 P15-10 惰性注册表条目清理移除（无命令消费）。 */
export type ProductOperationId =
  | "account.health_check"
  | "account.tool_selection"
  | "account.switch"
  | "account.native_activation"
  | "checkin.status"
  | "checkin.schedule"
  | "library.read"
  | "library.archive"
  | "library.delete"
  | "library.work_project_merge"
  | "library.trae_cn_project_merge";

export interface ProductOperationStateDto {
  readonly operation_id: ProductOperationId;
  readonly state: OperationReadiness;
  readonly reason: string;
  readonly evidence_ref: string | null;
}

export interface ProductOperationSnapshotDto {
  /** 产品协议值（后端操作注册表按 product_id + library_id 校验；协议值稳定）。 */
  readonly product_id: string;
  readonly library_id: string | null;
  readonly operations: readonly ProductOperationStateDto[];
}

/** 后端只读身份发现结果；不包含 userId、token 或 native storage。 */
export interface ProductIdentityStateDto {
  readonly product_id: string;
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
