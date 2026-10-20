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

/** WorkBuddy 深度健康检测实时面结果（P18-B3）。 */
export type WorkBuddyLiveHealthStatus =
  | "verified" // 客户端运行中且 wbipc 只读回读通过
  | "client_offline" // 客户端未运行（仅完成本地检测，正常态）
  | "probe_failed"; // 客户端运行中但探针回读异常

/** WorkBuddy 深度健康检测回执（文件面 + 实时面；只读零写副作用）。 */
export interface WorkBuddyDeepHealthDto {
  readonly status: WorkBuddySessionStatus;
  readonly live_status: WorkBuddyLiveHealthStatus;
}

/** WorkBuddy 会话健康（文件面只读；不含令牌材料）。 */
export interface WorkBuddySessionHealthDto {
  readonly status: WorkBuddySessionStatus;
  /** 当前客户端登录账号的平台 uid（后端 DTO 已有字段；uid 原文不进 UI 主视野，
   *  仅用于与池行 account_id 同源匹配「使用中」标记与积分数据面归属）。 */
  readonly uid: string;
  readonly uin: string;
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
  /** 签到日期（YYYY-MM-DD，倒序）；per-account 命令（P19-B5）返回，单账号旧命令可缺省。 */
  readonly checkin_dates?: readonly string[];
}

/** WorkBuddy 签到领取回执（verified = 复核翻转确认真实入账）。 */
export interface WorkBuddyCheckinClaimDto {
  readonly credit: number;
  readonly streak_days: number;
  readonly verified: boolean;
}

/** WorkBuddy 凭据续期回执（renewed=false = access 仍充裕，未轮换）。 */
/**
 * P17-T6/A4 WorkBuddy 切号回执（switch_workbuddy_account 返回；脱敏）。
 */
export interface WorkBuddySwitchDto {
  /** 切换到的账号展示名（uid 不进主视野）。 */
  readonly screen_name: string;
  /** 本次是否先做了池凭据续期（false = access 充裕，未轮换）。 */
  readonly renewed: boolean;
}

export interface WorkBuddyRefreshStatusDto {
  readonly renewed: boolean;
  readonly access_expires_at_unix_seconds: number;
  readonly refresh_expires_at_unix_seconds: number;
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

/**
 * P18-B1 WorkBuddy 会话过户 DTO（计划 + 执行回执合一；账号一律昵称标签，
 * uid 原文不进前端）。
 */
export interface WorkBuddyTransferDto {
  /** true = 只出转移计划（零副作用）；false = 执行回执。 */
  readonly dry_run: boolean;
  /** 无需转移（无上一账号或待过户为 0）。 */
  readonly nothing_to_transfer: boolean;
  readonly current_label: string;
  readonly previous_label: string | null;
  /** 上一账号名下待过户的本地会话数（含软删除行）。 */
  readonly pending_local_rows: number;
  readonly current_account_rows: number;
  /** 上一账号名下云端会话数（不碰；>0 时 UI 明示）。 */
  readonly cloud_residual_rows: number;
  /** 其余更早账号的本地会话数（不在本次范围）。 */
  readonly other_account_rows: number;
  /** 方向识别来源："log"（日志权威）| "sessions_fallback"（最近活跃回退）。 */
  readonly direction_source: "log" | "sessions_fallback";
  // ---- 执行回执（dry_run=false 时填充） ----
  readonly updated_rows: number | null;
  readonly verified_sessions: number | null;
  readonly missing_body_files: number | null;
  readonly restarted: boolean | null;
  readonly process_count_after_restart: number | null;
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
