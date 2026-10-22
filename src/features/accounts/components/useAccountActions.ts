import { useCallback, useRef, useState } from "react";
import { notify, type NotifyIssue } from "../../../platform/notify";
import type { CredentialRefreshEntryDto, CreditsRefreshEntryDto } from "../../../types/account_switch";
import { safeUiErrorMessage } from "../../../utils/safeUiError";

/**
 * 账号批量动作 hook（P4c-5b）：额度刷新 / 凭据刷新 / 健康检测的忙碌位与结果汇报。命令由
 * 调用方注入（不 import platform/accounts 或 workbuddyAdapter），产品差异留在页面边缘；这里
 * 只管并发保护、二次确认与 notify.summary 汇总。失败必须可见：异常账号逐条列名 + 人话原因
 * （旧 AccountCenterTrae L636-640 / L664-670 / L700-706 / L763-764 同一口径），不静默吞掉。
 */

/** 健康检测逐账号回执的最小结构面（TraeCnHealthEntryDto 与适配器回执均满足）。 */
export interface AccountHealthEntry {
  readonly profile_id: string;
  readonly screen_name: string;
  readonly healthy: boolean;
  readonly error_code: string | null;
}

/** 注入命令与页面侧文案钩子；notifyPrefix 为摘要标题前缀（TRAE 传空串即保留旧文案）。 */
export interface AccountActionCommands {
  refreshCredits(profileIds: string[]): Promise<readonly CreditsRefreshEntryDto[]>;
  refreshCredentials(profileIds: string[]): Promise<readonly CredentialRefreshEntryDto[]>;
  checkHealth(profileIds: string[]): Promise<readonly AccountHealthEntry[]>;
  readonly notifyPrefix: string;
  /** 凭据刷新二次确认：false = 放弃执行；缺省时一律不执行（fail-closed）。 */
  confirm?(): boolean | Promise<boolean>;
  /** 展示名解析（备注名优先）；缺省回退回执里的 screen_name。 */
  displayNameOf?(profileId: string, fallback: string): string;
}

export type AccountActionKind = "credits" | "credentials" | "health";
/** 三链各自独立的忙碌位（页面据此分别禁用按钮）。 */
export type AccountActionBusy = Readonly<Record<AccountActionKind, boolean>>;

export interface AccountActionsResult {
  readonly busy: AccountActionBusy;
  /** 凭据刷新必须二次确认（旧 L683-686 只开弹层，确认后才由 L689 执行）。 */
  readonly needsCredentialConfirm: true;
  /** 返回 null = 未执行（忙碌 / 空目标 / 确认被拒 / 失败已通知）。 */
  readonly runRefreshCredits: (profileIds: readonly string[]) => Promise<readonly CreditsRefreshEntryDto[] | null>;
  readonly runRefreshCredentials: (profileIds: readonly string[]) => Promise<readonly CredentialRefreshEntryDto[] | null>;
  readonly runHealthCheck: (profileIds: readonly string[]) => Promise<readonly AccountHealthEntry[] | null>;
}

const IDLE_BUSY: AccountActionBusy = { credits: false, credentials: false, health: false };

export function useAccountActions(commands: AccountActionCommands): AccountActionsResult {
  // 命令与文案钩子经 ref 取最新值：runner 保持稳定身份，不因页面重建入参对象而换引用。
  const commandsRef = useRef(commands);
  commandsRef.current = commands;
  // 忙碌位以 ref 为准（同一 tick 内重复点击也拦得住），state 只驱动渲染。
  const busyRef = useRef<Record<AccountActionKind, boolean>>({ ...IDLE_BUSY });
  const confirmingRef = useRef(false); // 二次确认弹层打开期间挡重复确认（不占忙碌位）
  const [busy, setBusy] = useState<AccountActionBusy>(IDLE_BUSY);
  const mark = useCallback((kind: AccountActionKind, value: boolean) => {
    busyRef.current[kind] = value;
    setBusy((previous) => (previous[kind] === value ? previous : { ...previous, [kind]: value }));
  }, []);
  // 摘要异常条目：名称优先取页面注入的展示名（备注名），原因只出人话。
  const issueOf = useCallback((id: string, screenName: string, reason: string): NotifyIssue =>
    ({ displayName: commandsRef.current.displayNameOf?.(id, screenName) || screenName || "该账号", reason }), []);

  // 额度刷新（旧 L653-678）：逐账号只读查询，error_code 非空即异常。
  const runRefreshCredits = useCallback(async (profileIds: readonly string[]) => {
    if (busyRef.current.credits || profileIds.length === 0) return null;
    mark("credits", true);
    try {
      const results = await commandsRef.current.refreshCredits([...profileIds]);
      const issues = results.filter((item) => item.error_code !== null).map((item) =>
        issueOf(item.profile_id, item.screen_name, safeUiErrorMessage(item.error_code, "网络或服务暂时不可用，请稍后重试。")));
      notify.summary(`${commandsRef.current.notifyPrefix}额度刷新完成`, issues, results.length - issues.length);
      return results;
    } catch (reason: unknown) {
      notify.error(safeUiErrorMessage(reason, "额度刷新未完成，请稍后重试。"));
      return null;
    } finally {
      mark("credits", false);
    }
  }, [issueOf, mark]);

  // 凭据刷新（旧 L689-729）：批量换发写回全部账号本机登录信息，必须先确认。确认在置忙
  // 之前完成（旧 L683-686 只开弹层、不动忙碌位），故用 confirmingRef 单独挡并发确认。
  const runRefreshCredentials = useCallback(async (profileIds: readonly string[]) => {
    if (busyRef.current.credentials || confirmingRef.current || profileIds.length === 0) return null;
    const confirm = commandsRef.current.confirm;
    if (!confirm) return null; // fail-closed：未注入确认入口一律不执行
    confirmingRef.current = true;
    let approved = false;
    try { approved = await confirm(); } finally { confirmingRef.current = false; }
    if (!approved || busyRef.current.credentials) return null;
    mark("credentials", true);
    try {
      const results = await commandsRef.current.refreshCredentials([...profileIds]);
      const issues = results.filter((item) => !item.refreshed).map((item) =>
        issueOf(item.profile_id, item.screen_name,
          safeUiErrorMessage(item.error_code, "登录凭据刷新未完成，请稍后重试。")));
      notify.summary(`${commandsRef.current.notifyPrefix}凭据刷新完成`, issues,
        results.filter((item) => item.refreshed).length);
      return results;
    } catch (reason: unknown) {
      notify.error(safeUiErrorMessage(reason, "凭据刷新未完成，请稍后重试。"));
      return null;
    } finally {
      mark("credentials", false);
    }
  }, [issueOf, mark]);

  // 健康检测（旧 L736-773）：healthy=false 或 error_code 非空即异常。
  const runHealthCheck = useCallback(async (profileIds: readonly string[]) => {
    if (busyRef.current.health || profileIds.length === 0) return null;
    mark("health", true);
    try {
      const results = await commandsRef.current.checkHealth([...profileIds]);
      const issues = results.filter((item) => !item.healthy || item.error_code !== null).map((item) =>
        issueOf(item.profile_id, item.screen_name,
          safeUiErrorMessage(item.error_code, "网络或服务暂时不可用，请稍后重试。")));
      notify.summary(`${commandsRef.current.notifyPrefix}健康检测完成`, issues, results.length - issues.length);
      return results;
    } catch (reason: unknown) {
      notify.error(safeUiErrorMessage(reason, "健康检测未完成，请稍后重试。"));
      return null;
    } finally {
      mark("health", false);
    }
  }, [issueOf, mark]);

  return { busy, needsCredentialConfirm: true, runRefreshCredits, runRefreshCredentials, runHealthCheck };
}
