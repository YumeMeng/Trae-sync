import { invoke } from "@tauri-apps/api/core";
import type { ProductId } from "./productRegistry";
import type {
  ProductIdentityStateDto,
  TraeCnCredentialRefreshEntryDto,
  ProductLoginBeginDto,
  TraeCnAccountDto,
  TraeCnHealthEntryDto,
  TraeCnLoginReceiptDto,
} from "../types/product";

/** 账号工作台只识别用户可见的能力，不把后端 command 暴露给页面。 */
export type AccountCapability =
  | "login"
  | "relogin"
  | "switch"
  | "refreshCredential"
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
  readonly isCurrent: boolean;
}

/** 适配器返回给共享账号工作台的快照；不携带令牌或本地凭据。 */
export interface AccountSnapshot {
  readonly productId: ProductId;
  readonly productName: string;
  readonly statusLabel: string;
  readonly accounts: readonly AccountView[];
  readonly currentAccountId: string | null;
  readonly capabilities: readonly AccountCapability[];
  readonly identityLabel: string;
  readonly identityMessage: string;
}

export type AccountAction = "refreshCredential" | "checkin" | "credits" | "health";

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
  login(): Promise<{ readonly displayName: string }>;
  cancelLogin(): Promise<void>;
  switchAccount(accountId: string): Promise<void>;
  /**
   * targetAccountId：单账号动作目标（ADR-0031 决策 6 远程退出联动健康检查）；
   * 缺省为全部账号。仅 health 动作使用该参数。
   */
  runAction(action: AccountAction, targetAccountId?: string): Promise<AccountActionResult>;
}

function identityCopy(
  identity: ProductIdentityStateDto | undefined,
  accounts: readonly TraeCnAccountDto[],
): Pick<AccountSnapshot, "identityLabel" | "identityMessage"> {
  const currentAccount = accounts.find((account) => account.is_current) ?? null;
  const identityLabel = currentAccount
    ? `当前账号：${currentAccount.display_name}`
    : identity?.identity_status === "recognized"
      ? "已发现账号材料"
      : "尚未识别账号";
  const identityMessage = identity?.relation_to_work === "same_identity"
    ? "已发现与 Work CN 相同的 TRAE 身份。"
    : identity?.relation_to_work === "different_identity"
      ? "已发现 Trae CN 当前保存的是另一个 TRAE 身份。"
      : "尚未确认与 Work CN 的 TRAE 身份关系。";
  return { identityLabel, identityMessage };
}

function toAccountView(account: TraeCnAccountDto): AccountView {
  return {
    id: account.profile_id,
    displayName: account.display_name,
    status: account.status,
    lastVerifiedAt: account.last_verified_at,
    isCurrent: account.is_current,
  };
}

/** Trae CN 的独立 OAuth、账号池和当前账号指针适配器。 */
export function createTraeCnAccountAdapter(
  productName = "Trae CN",
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
    capabilities: ["login", "relogin", "switch", "health", "refreshCredential", "remoteDevices"],
    async loadSnapshot() {
      const { identity, accounts } = await loadRawAccounts();
      const current = accounts.find((account) => account.is_current) ?? null;
      const copy = identityCopy(identity, accounts);
      return {
        productId: "trae_cn",
        productName,
        statusLabel: accounts.length > 0 ? `已保存 ${accounts.length} 个账号` : defaultStatusLabel,
        accounts: accounts.map(toAccountView),
        currentAccountId: current?.profile_id ?? null,
        capabilities: ["login", "relogin", "switch", "health", "refreshCredential", "remoteDevices"],
        ...copy,
      };
    },
    async login() {
      // 复用的是浏览器网页登录会话，不是 Work CN 的本地凭据或 native storage。
      await invoke<ProductLoginBeginDto>("begin_trae_cn_login", { useSystemBrowser: true });
      const receipt = await invoke<TraeCnLoginReceiptDto>("complete_trae_cn_login");
      return { displayName: receipt.screen_name || "Trae CN 账号" };
    },
    async cancelLogin() {
      await invoke("cancel_trae_cn_login");
    },
    async switchAccount(accountId) {
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
      if (action === "refreshCredential") {
        const results = await invoke<TraeCnCredentialRefreshEntryDto[]>("refresh_trae_cn_credentials", {
          profileIds: (await loadRawAccounts()).accounts.map((account) => account.profile_id),
        });
        return actionResultFromCredentialRefresh(results);
      }
      throw new Error(`Trae CN 暂不支持“${action}”操作。`);
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

function actionResultFromCredentialRefresh(
  results: readonly TraeCnCredentialRefreshEntryDto[],
): AccountActionResult {
  return {
    okCount: results.filter((item) => item.refreshed).length,
    issues: results
      .filter((item) => !item.refreshed)
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
