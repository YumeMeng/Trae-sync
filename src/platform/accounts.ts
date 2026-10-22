import { invoke } from "@tauri-apps/api/core";
import type {
  CheckinBatchSummaryDto,
  CheckinCapabilityDto,
  CheckinLoginReceiptDto,
  CheckinOverviewEntryDto,
  CredentialRefreshEntryDto,
  CreditsRefreshEntryDto,
  ManagedAccountsViewDto,
  MasterAccountSwitchDto,
  MasterSwitchPluginPreviewDto,
  TraeInstanceStateDto,
} from "../types/account_switch";
import type { EnvironmentStateDto } from "../types/environment";
import type {
  ProductIdentityStateDto,
  TraeCnAccountDto,
  TraeCnHealthEntryDto,
  TraeCnLoginReceiptDto,
} from "../types/product";

/**
 * 账号域命令的类型化封装（P4c-1a）：供后续 features/accounts 接入，页面
 * 不出现命令名。本文件只做 invoke 转接——不实现业务逻辑、不吞错误（失败
 * 原样抛出，由调用方按 safeUiError 词表映射文案）、不改任何后端命令契约
 * （参数名与 DTO 形状逐条对照现有调用点与 src-tauri 命令签名）。
 */

/** begin_checkin_login 回执：登录页地址 + 单槽 attempt 号（complete/cancel 都要带）。 */
export interface CheckinLoginBeginReceipt {
  readonly login_url: string;
  readonly attempt_id: number;
}

/* ============================================================
 * TRAE 读取（本机缓存为主；仅 get_trae_instance_states 含网络探测）
 * ============================================================ */

/** 读取签到能力开关：返回 enabled / transport / real_http_enabled 门禁与提示语。 */
export function readCheckinCapability(): Promise<CheckinCapabilityDto> {
  return invoke<CheckinCapabilityDto>("get_checkin_capability");
}

/** 读取账号总览（档案 + 积分缓存 + 令牌到期 + 设备尾号）；失败必须由调用方可见。 */
export function readCheckinOverview(): Promise<readonly CheckinOverviewEntryDto[]> {
  return invoke<readonly CheckinOverviewEntryDto[]>("get_checkin_overview");
}

/** 读取逐账号登录存档健康度（含凭据包实调，有网络成本）；返回 login_state 四态。 */
export function readTraeInstanceStates(
  { profileIds }: { readonly profileIds: readonly string[] },
): Promise<readonly TraeInstanceStateDto[]> {
  return invoke<readonly TraeInstanceStateDto[]>("get_trae_instance_states", {
    profileIds: [...profileIds],
  });
}

/** 读取主库环境状态；返回档案账号、observed_profile_id 实测身份与运行态。 */
export function readEnvironmentState(): Promise<EnvironmentStateDto> {
  return invoke<EnvironmentStateDto>("get_environment_state");
}

/** 读取账号中心状态（已保存账号 + 当前账号证据 + 最近验证 + 切号计划）。 */
export function readManagedAccountState(): Promise<ManagedAccountsViewDto> {
  return invoke<ManagedAccountsViewDto>("get_managed_account_state");
}

/* ============================================================
 * TRAE 写入（登录编排 / 批量刷新 / 单账号档案）
 * ============================================================ */

/** 发起 OAuth 登录：返回 attempt_id 与登录页地址；useSystemBrowser=false 走隔离实例。 */
export function beginCheckinLogin(
  { useSystemBrowser }: { readonly useSystemBrowser: boolean },
): Promise<CheckinLoginBeginReceipt> {
  return invoke<CheckinLoginBeginReceipt>("begin_checkin_login", { useSystemBrowser });
}

/** 阻塞等待 OAuth 回调并入库；返回账号概要 + 首登验收结果（acceptance）。 */
export function completeCheckinLogin(
  { attemptId }: { readonly attemptId: number },
): Promise<CheckinLoginReceiptDto> {
  return invoke<CheckinLoginReceiptDto>("complete_checkin_login", { attemptId });
}

/** 置位登录取消标记；错误码 login_cancelled / login_cancel_too_late 原样抛出。 */
export function cancelCheckinLogin(
  { attemptId }: { readonly attemptId: number },
): Promise<void> {
  return invoke<void>("cancel_checkin_login", { attemptId });
}

/** 刷新模型额度（逐账号只读查询，不消耗签到资格）；返回逐账号回执与 error_code。 */
export function refreshCheckinCredits(
  { profileIds }: { readonly profileIds: readonly string[] },
): Promise<readonly CreditsRefreshEntryDto[]> {
  return invoke<readonly CreditsRefreshEntryDto[]>("refresh_checkin_credits", {
    profileIds: [...profileIds],
  });
}

/** 同设备换发登录凭据并写回本机登录信息；返回逐账号 refreshed 与 error_code。 */
export function refreshCheckinCredentials(
  { profileIds }: { readonly profileIds: readonly string[] },
): Promise<readonly CredentialRefreshEntryDto[]> {
  return invoke<readonly CredentialRefreshEntryDto[]>("refresh_checkin_credentials", {
    profileIds: [...profileIds],
  });
}

/** 执行一次签到批次（ids 顺序即执行顺序）；返回 total/completed/failed 与逐账号 outcome。 */
export function runAccountCheckin(
  { profileIds }: { readonly profileIds: readonly string[] },
): Promise<CheckinBatchSummaryDto> {
  return invoke<CheckinBatchSummaryDto>("run_checkin", { profileIds: [...profileIds] });
}

/** 保存备注名；displayName=null 或空串 = 清除（回退服务端 screen_name）。 */
export function setAccountDisplayName(
  { profileId, displayName }: { readonly profileId: string; readonly displayName: string | null },
): Promise<void> {
  return invoke<void>("set_account_display_name", { profileId, displayName });
}

/** 保存补录手机号；mobile=null 或空串 = 清除（展示回退脱敏号）。 */
export function setAccountMobile(
  { profileId, mobile }: { readonly profileId: string; readonly mobile: string | null },
): Promise<void> {
  return invoke<void>("set_account_mobile", { profileId, mobile });
}

/** 设置该账号是否参与每日自动签到（只影响自动批次，手动签到不受限）。 */
export function setAccountAutoCheckin(
  { profileId, enabled }: { readonly profileId: string; readonly enabled: boolean },
): Promise<void> {
  return invoke<void>("set_account_auto_checkin", { profileId, enabled });
}

/** 重铸签到设备（GetPCAuthCode → ExchangeToken → 凭据写回）；返回新设备 ID。 */
export function resetCheckinDevice(
  { profileId }: { readonly profileId: string },
): Promise<string> {
  return invoke<string>("reset_checkin_device", { profileId });
}

/** 删除账号（档案 + 凭据 + 积分缓存）；破坏性操作，单次确认由调用方负责。 */
export function removeCheckinAccount(
  { profileId }: { readonly profileId: string },
): Promise<void> {
  return invoke<void>("remove_checkin_account", { profileId });
}

/* ============================================================
 * 切号（MasterSwitchDialog 现有 3 条命令；事务步骤与阶段语义不在此简化）
 * ============================================================ */

/** 插件差异预检（fail-soft，永不返回错误）；remove_names 非空才需一次移除确认。 */
export function previewMasterSwitchPlugins(
  { profileId }: { readonly profileId: string },
): Promise<MasterSwitchPluginPreviewDto> {
  return invoke<MasterSwitchPluginPreviewDto>("preview_master_switch_plugins", { profileId });
}

/** 执行主库五步切号事务；force 用于 master_switch_busy 下的强制切换。 */
export function switchMasterAccount(
  { profileId, force }: { readonly profileId: string; readonly force: boolean },
): Promise<MasterAccountSwitchDto> {
  return invoke<MasterAccountSwitchDto>("switch_master_account", { profileId, force });
}

/** 死锁兜底：强制重置主库当前账号（须先关闭主库）；返回是否完成重置。 */
export function forceResetMasterCurrentAccount(
  { profileId }: { readonly profileId: string },
): Promise<boolean> {
  return invoke<boolean>("force_reset_master_current_account", { profileId });
}

/* ============================================================
 * 适配器路径（Trae CN 独立账号池 + 工具账号选择指针；测试注入保留）
 * ============================================================ */

/** 读取本机客户端身份发现结果（recognized / unknown / conflict）；不含 token 或 userId。 */
export function readProductIdentityState(): Promise<readonly ProductIdentityStateDto[]> {
  return invoke<readonly ProductIdentityStateDto[]>("get_product_identity_state");
}

/** 读取 Trae CN 独立账号池的非敏感展示档案（含 is_current 工具选择指针）。 */
export function listTraeCnAccounts(): Promise<readonly TraeCnAccountDto[]> {
  return invoke<readonly TraeCnAccountDto[]>("list_trae_cn_accounts");
}

/** 发起 Trae CN OAuth 登录：返回 attempt_id 与登录页地址。 */
export function beginTraeCnLogin(
  { useSystemBrowser }: { readonly useSystemBrowser: boolean },
): Promise<CheckinLoginBeginReceipt> {
  return invoke<CheckinLoginBeginReceipt>("begin_trae_cn_login", { useSystemBrowser });
}

/** 完成 Trae CN OAuth 回调；返回账号概要 + acceptance 验收结果。 */
export function completeTraeCnLogin(
  { attemptId }: { readonly attemptId: number },
): Promise<TraeCnLoginReceiptDto> {
  return invoke<TraeCnLoginReceiptDto>("complete_trae_cn_login", { attemptId });
}

/** 取消 Trae CN 登录等待；错误码原样抛出（收尾语义由调用方判定）。 */
export function cancelTraeCnLogin(
  { attemptId }: { readonly attemptId: number },
): Promise<void> {
  return invoke<void>("cancel_trae_cn_login", { attemptId });
}

/** 更新 Trae Sync 自有工具账号选择指针；不切换官方客户端登录身份。 */
export function switchTraeCnAccount(
  { profileId }: { readonly profileId: string },
): Promise<readonly TraeCnAccountDto[]> {
  return invoke<readonly TraeCnAccountDto[]>("switch_trae_cn_account", { profileId });
}

/** 探测 Trae CN 账号健康度；profileIds 缺省 = 全部账号，返回逐账号 healthy 与 error_code。 */
export function checkTraeCnAccountHealth(
  { profileIds }: { readonly profileIds?: readonly string[] } = {},
): Promise<readonly TraeCnHealthEntryDto[]> {
  // 后端 profile_ids 是 Option：缺省时不传键，避免把 undefined 序列化进参数。
  return invoke<readonly TraeCnHealthEntryDto[]>(
    "check_trae_cn_account_health",
    profileIds ? { profileIds: [...profileIds] } : {},
  );
}
