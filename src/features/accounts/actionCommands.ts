import { deriveLoginSlotState, type LoginSlotState } from "../../components/StatusBadges";
import { readTraeInstanceStates, refreshCheckinCredits, refreshCheckinCredentials } from "../../platform/accounts";
import { fetchWorkBuddyCheckinOverview } from "../../platform/workbuddyAdapter";
import type { CheckinOverviewEntryDto, CreditsRefreshEntryDto, TraeInstanceStateDto } from "../../types/account_switch";
import type { WorkBuddyAccountEntryDto } from "../../types/product";
import { effectiveDisplayName, workbuddyDisplayName } from "../../utils/accountDisplay";
import { ACCOUNT_TEXTS } from "./config";
import type { AccountActionCommands, AccountHealthEntry } from "./components/useAccountActions";

/**
 * 页头批量动作的命令装配（P4c-6b-2b）：按产品给出 useAccountActions 需要的三条命令、
 * 页头动作 id 与目标解析。页面只查表、不判断产品；**没有对应命令的动作不登记**
 * （fail-closed，不把单条回执拆成逐账号结果、不造假）。命令口径逐字沿用旧实现：
 * TRAE = AccountCenterTrae L653-678 / L689-729 / L736-773；WB = WorkBuddyPanel 的逐账号余额面。
 */

/** 装配结果：命令 + 页头动作 id（缺省 = 本产品页头无该入口）+ 目标解析。 */
export interface ActionWiring {
  readonly commands: AccountActionCommands;
  readonly actionIds: { readonly credits?: string; readonly health?: string };
  /** 目标：优先当前选中行（须在本产品数据面内），否则全量（旧实现手动检测不传参 = 全量）。 */
  readonly targets: (selectedRowId: string) => readonly string[];
}

/** 无命令占位：页面只在登记了动作 id 时才调用，故永不执行；误调也如实失败，不伪装成功。 */
const unsupported = (): Promise<never> => Promise.reject(new Error("action_unsupported"));
export const UNAVAILABLE_ACTION_COMMANDS: AccountActionCommands = { refreshCredits: unsupported, refreshCredentials: unsupported, checkHealth: unsupported, notifyPrefix: "" };
const pick = (all: readonly string[], selected: string): readonly string[] =>
  selected === "" ? all : all.includes(selected) ? [selected] : [];
const idsOf = (rows: readonly { readonly profile_id: string }[]): readonly string[] => rows.map((row) => row.profile_id);

/** 已确认只能重新登录恢复的凭据错误（旧 L1397-1409 同一词表）。 */
const RELOGIN_CODES = new Set(["credential_refresh_failed", "binding_mismatch", "auth_mismatch", "credential_missing", "credential_unavailable", "credential_invalid", "manual_recovery_required"]);
/** 登录槽 → 回执错误码（正常/未登录不算异常）；原因文案统一由 safeUiError 词表给，码不进界面。 */
const LOGIN_ISSUE_CODE: Record<LoginSlotState, string | null> = { ok: null, signed_out: null, relogin: "credential_invalid", pending: "credential_unavailable", expired: "credential_refresh_failed" };

/**
 * TRAE 健康检测两步（旧 L736-773）：本地深度检测（读取失败不阻断）→ 签到会话网络探测
 * → 逐账号合成。登录问题优先（会话异常多为同一根因），登录正常才单独报探测失败；
 * 本地检测读取失败时只按探测结果判定，不臆测登录态。
 */
async function checkTraeHealth(
  overview: readonly CheckinOverviewEntryDto[], profileIds: string[],
): Promise<readonly AccountHealthEntry[]> {
  const states: readonly TraeInstanceStateDto[] | null = await readTraeInstanceStates({ profileIds }).catch(() => null);
  const results = await refreshCheckinCredits({ profileIds });
  const now = Math.floor(Date.now() / 1000);
  const entryOf = new Map(overview.map((entry) => [entry.profile_id, entry]));
  const stateOf = new Map((states ?? []).map((state) => [state.profile_id, state]));
  const probeOf = new Map(results.map((result) => [result.profile_id, result]));
  // 逐目标出条目（旧 buildHealthResult 遍历目标集合同法）：探测缺该行即不报异常，不臆测。
  return profileIds.map((profileId) => {
    const probe = probeOf.get(profileId);
    const entry = entryOf.get(profileId);
    const renewalDead = RELOGIN_CODES.has(probe?.error_code ?? "");
    const reloginOnly = renewalDead
      || (entry !== undefined && (entry.credential_legacy || RELOGIN_CODES.has(entry.refresh_error_code ?? "")));
    const expired = entry?.access_token_expires_at_unix_seconds != null
      && entry.access_token_expires_at_unix_seconds <= now;
    // 徽章同源口径（旧 effectiveCredentialState）：旧通道 / 续期被拒 / access 过期一律按 stale。
    const loginState = reloginOnly || expired ? "stale" : stateOf.get(profileId)?.login_state;
    const loginCode = states === null ? null
      : LOGIN_ISSUE_CODE[deriveLoginSlotState({ login_state: loginState, relogin_only: reloginOnly })];
    const code = loginCode ?? probe?.error_code ?? null;
    return { profile_id: profileId, screen_name: probe?.screen_name ?? entry?.screen_name ?? "", healthy: code === null, error_code: code };
  });
}

/** WB 额度刷新（旧 WorkBuddyPanel 逐账号余额面）：单目标走单账号查询，否则全池批查。 */
async function refreshWorkBuddyCredits(profileIds: string[]): Promise<readonly CreditsRefreshEntryDto[]> {
  const rows = await fetchWorkBuddyCheckinOverview(profileIds.length === 1 ? profileIds[0] : null);
  return rows.map((row): CreditsRefreshEntryDto => ({ profile_id: row.profile_id, screen_name: row.screen_name,
    credits: row.total_credits, checked_in: row.today_checked_in, usage_remaining_credits: row.credits_balance,
    // 整行读取失败看 error，仅余额查询失败看 balance_error——两者都不伪装成成功。
    error_code: row.error ?? row.balance_error }));
}

/** TRAE：额度 / 凭据 / 健康检测三条命令（旧 L653-678 / L689-729 / L736-773）。 */
const traeWiring = (payload: unknown): ActionWiring => {
  const overview = (payload as { readonly overview: readonly CheckinOverviewEntryDto[] }).overview;
  const nameOf = new Map(overview.map((entry) => [entry.profile_id, effectiveDisplayName(entry)]));
  return {
    commands: {
      refreshCredits: (ids) => refreshCheckinCredits({ profileIds: ids }),
      refreshCredentials: (ids) => refreshCheckinCredentials({ profileIds: ids }),
      checkHealth: (ids) => checkTraeHealth(overview, ids),
      notifyPrefix: "", // 旧摘要标题逐字（无产品前缀）
      displayNameOf: (profileId, fallback) => nameOf.get(profileId) ?? fallback,
    },
    actionIds: { credits: "account-refresh-credits", health: "account-health-check" },
    targets: (selected) => pick(idsOf(overview), selected),
  };
};

/**
 * WB：额度刷新走现有逐账号余额查询（get_workbuddy_checkin_overview）。凭据续期
 * （refresh_workbuddy_account）与深度健康检测（check_workbuddy_login_health）都只返回
 * 池级/客户端级单条回执，没有逐账号 healthy/error_code，故不登记这两个动作
 * （fail-closed，不造假逐账号结果）；WB 页头当前亦无「刷新额度」入口。
 */
const workbuddyWiring = (payload: unknown): ActionWiring => {
  const pool = (payload as { readonly pool: readonly WorkBuddyAccountEntryDto[] }).pool;
  const nameOf = new Map(pool.map((account) => [account.profile_id, workbuddyDisplayName(account)]));
  return {
    commands: { refreshCredits: refreshWorkBuddyCredits, refreshCredentials: unsupported, checkHealth: unsupported,
      notifyPrefix: ACCOUNT_TEXTS.notifyPrefixWorkbuddy, // 旧 WB 通知同一前缀
      displayNameOf: (profileId, fallback) => nameOf.get(profileId) ?? fallback },
    actionIds: {},
    targets: (selected) => pick(idsOf(pool), selected),
  };
};

/** 产品装配表：键 = productId，未登记即 null（页面给永不执行的占位命令）。 */
const WIRINGS: Record<string, (payload: unknown) => ActionWiring> = { trae_cn: traeWiring, workbuddy: workbuddyWiring };
/** 取本产品的动作装配；无数据面或未登记产品 → null（fail-closed，不猜命令）。 */
export function actionWiringFor(productId: string, payload: unknown): ActionWiring | null {
  return payload === null ? null : WIRINGS[productId]?.(payload) ?? null;
}
