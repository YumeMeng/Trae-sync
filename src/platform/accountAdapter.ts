import { invoke } from "@tauri-apps/api/core";
import type { ProductId } from "./productRegistry";
import type {
  ProductIdentityStateDto,
  TraeCnAccountDto,
  TraeCnHealthEntryDto,
  TraeCnLoginReceiptDto,
} from "../types/product";

/** 账号工作台只识别用户可见的能力，不把后端 command 暴露给页面。 */
export type AccountCapability =
  | "login"
  | "relogin"
  | "toolSelection"
  | "checkin"
  | "credits"
  | "health"
  | "remoteDevices";

/** 账号页需要的最小、跨产品账号摘要。 */
export interface AccountView {
  readonly id: string;
  readonly displayName: string;
  readonly status: "active" | "expired" | "unknown";
  readonly lastVerifiedAt: string | null;
  readonly isToolSelected: boolean;
}

/** 适配器返回给共享账号工作台的快照；不携带令牌或本地凭据。 */
export interface AccountSnapshot {
  /** 快照来源产品；用于丢弃切产品后迟到的读取结果。 */
  readonly productId: ProductId;
  readonly statusLabel: string;
  readonly accounts: readonly AccountView[];
  readonly officialIdentityLabel: string;
  readonly officialIdentityMessage: string;
}

export type AccountAction = "checkin" | "credits" | "health";

export interface AccountActionIssue {
  readonly id: string;
  readonly displayName: string;
  readonly errorCode: string | null;
}

export interface AccountActionResult {
  readonly okCount: number;
  readonly issues: readonly AccountActionIssue[];
}

/**
 * 产品差异的最小切口。
 *
 * Work CN 目前仍由 AccountCenter 内部保留成熟的签到/详情事务；Trae CN
 * 先通过此接口接入共享账号工作台。这样不会为了“抽象完整”而复制后端存储或
 * 把 Work 专有 DTO 泄漏到 CN 页面。
 */
export interface AccountAdapter {
  readonly productId: ProductId;
  readonly productName: string;
  readonly capabilities: readonly AccountCapability[];
  loadSnapshot(): Promise<AccountSnapshot>;
  login(onAttemptStarted: (attemptId: number) => void): Promise<{
    readonly displayName: string;
    /** 首登可持续性验收结果（ADR-0027）：failed = 账号已保存但同设备换发未通过。 */
    readonly acceptance: "passed" | "failed";
    /** 验收失败的稳定错误码；通过时为 null。 */
    readonly acceptanceErrorCode: string | null;
  }>;
  cancelLogin(attemptId: number): Promise<void>;
  selectToolAccount(accountId: string): Promise<void>;
  /**
   * targetAccountId：单账号动作目标（ADR-0031 决策 6 远程退出联动健康检查）；
   * 缺省为全部账号。仅 health 动作使用该参数。
   */
  runAction(action: AccountAction, targetAccountId?: string): Promise<AccountActionResult>;
}

function identityCopy(
  identity: ProductIdentityStateDto | undefined,
): Pick<AccountSnapshot, "officialIdentityLabel" | "officialIdentityMessage"> {
  const officialIdentityLabel = identity?.identity_status === "recognized"
    ? "官方客户端身份：已识别"
    : identity?.identity_status === "conflict"
      ? "官方客户端身份：信息冲突"
      : "官方客户端身份：未确认";
  const relation = identity?.identity_status === "recognized" ? identity.relation_to_work : "unknown";
  const officialIdentityMessage = relation === "same_identity"
    ? "从 Trae CN 客户端数据读取到的身份与 Work CN 相同。"
    : relation === "different_identity"
      ? "从 Trae CN 客户端数据读取到的身份与 Work CN 不同。"
      : relation === "current_product"
        ? "这是 Work CN 客户端记录的身份。"
        : "目前无法确认 Trae CN 客户端身份与 Work CN 的关系。";
  return { officialIdentityLabel, officialIdentityMessage };
}

function toAccountView(account: TraeCnAccountDto): AccountView {
  return {
    id: account.profile_id,
    displayName: account.display_name,
    status: account.status,
    lastVerifiedAt: account.last_verified_at,
    isToolSelected: account.is_current,
  };
}

/** Trae CN 的独立 OAuth、账号池和工具内账号选择指针适配器。 */
export function createTraeCnAccountAdapter(
  // 融合版后 TraeCn 变体即官方产品"TRAE"（ADR-0035）。
  productName = "TRAE",
  defaultStatusLabel = "需要授权",
): AccountAdapter {
  const loadRawAccounts = async (): Promise<{
    readonly identity: ProductIdentityStateDto | undefined;
    readonly accounts: readonly TraeCnAccountDto[];
  }> => {
    const [identityItems, accounts] = await Promise.all([
      invoke<ProductIdentityStateDto[]>("get_product_identity_state"),
      invoke<TraeCnAccountDto[]>("list_trae_cn_accounts"),
    ]);
    return {
      identity: identityItems.find((item) => item.product_id === "trae_cn"),
      accounts,
    };
  };

  return {
    productId: "trae_cn",
    productName,
    capabilities: ["login", "relogin", "toolSelection", "health", "remoteDevices"],
    async loadSnapshot() {
      const { identity, accounts } = await loadRawAccounts();
      const copy = identityCopy(identity);
      return {
        productId: "trae_cn",
        statusLabel: accounts.length > 0 ? `已保存 ${accounts.length} 个账号` : defaultStatusLabel,
        accounts: accounts.map(toAccountView),
        ...copy,
      };
    },
    async login(onAttemptStarted) {
      // 复用的是浏览器网页登录会话，不是 Work CN 的本地凭据或 native storage。
      const begin = await invoke<{ attempt_id: number }>("begin_trae_cn_login", { useSystemBrowser: true });
      onAttemptStarted(begin.attempt_id);
      const receipt = await invoke<TraeCnLoginReceiptDto>("complete_trae_cn_login", { attemptId: begin.attempt_id });
      return {
        displayName: receipt.screen_name || "TRAE 账号",
        acceptance: receipt.acceptance,
        acceptanceErrorCode: receipt.acceptance_error_code ?? null,
      };
    },
    async cancelLogin(attemptId) {
      await invoke("cancel_trae_cn_login", { attemptId });
    },
    async selectToolAccount(accountId) {
      // 只更新 Trae Sync 自有选择指针，不切换官方客户端身份。
      await invoke("switch_trae_cn_account", { profileId: accountId });
    },
    async runAction(action, targetAccountId) {
      if (action === "health") {
        const accounts = (await loadRawAccounts()).accounts;
        // 单账号目标：只探测该账号；缺省为全部账号（手动全量检测）。
        const targets = targetAccountId
          ? accounts.filter((account) => account.profile_id === targetAccountId)
          : accounts;
        const results = await invoke<TraeCnHealthEntryDto[]>("check_trae_cn_account_health", {
          profileIds: targets.map((account) => account.profile_id),
        });
        return actionResultFromHealth(results);
      }
      throw new Error(`TRAE 暂不支持“${action}”操作。`);
    },
  };
}

function actionResultFromHealth(results: readonly TraeCnHealthEntryDto[]): AccountActionResult {
  return {
    okCount: results.filter((item) => item.healthy).length,
    issues: results
      .filter((item) => !item.healthy)
      .map((item) => ({
        id: item.profile_id,
        displayName: item.screen_name,
        errorCode: item.error_code,
      })),
  };
}

/** 当前已实现产品的账号适配器；Work 继续走其成熟的原生账号事务。 */
export function accountAdapterFor(productId: ProductId): AccountAdapter | null {
  return productId === "trae_cn" ? createTraeCnAccountAdapter() : null;
}
