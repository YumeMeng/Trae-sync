import { invoke } from "@tauri-apps/api/core";
import { CURRENT_PRODUCT_ID } from "../types/product";
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
  /** 快照来源产品（账号域内部标识值；用于丢弃切产品后迟到的读取结果）。 */
  readonly productId: string;
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
 * P15-10 单产品塌缩后，AccountCenter 主路径保留完整的账号事务（官方切号/
 * 凭据/额度/健康），本适配器保留融合版账号链的轻量接入面（工具内选择、
 * 健康检测、登录设备），由测试场景注入。
 */
export interface AccountAdapter {
  /** 账号域内部产品标识值（协议值稳定；不是产品切换概念）。 */
  readonly productId: string;
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
  // P15-10 单产品塌缩：身份观察只描述本机客户端证据状态，
  // 不再表述与另一产品（Work CN）的关系（后端 relation_to_work 字段保留不用）。
  const officialIdentityMessage = identity?.identity_status === "recognized"
    ? "已从本机客户端登录数据读取到官方身份。"
    : identity?.identity_status === "conflict"
      ? "本机客户端登录数据存在冲突，请重新登录确认。"
      : "暂时无法从本机客户端登录数据确认身份。";
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
      identity: identityItems.find((item) => item.product_id === CURRENT_PRODUCT_ID),
      accounts,
    };
  };

  return {
    productId: CURRENT_PRODUCT_ID,
    productName,
    capabilities: ["login", "relogin", "toolSelection", "health", "remoteDevices"],
    async loadSnapshot() {
      const { identity, accounts } = await loadRawAccounts();
      const copy = identityCopy(identity);
      return {
        productId: CURRENT_PRODUCT_ID,
        statusLabel: accounts.length > 0 ? `已保存 ${accounts.length} 个账号` : defaultStatusLabel,
        accounts: accounts.map(toAccountView),
        ...copy,
      };
    },
    async login(onAttemptStarted) {
      // 复用的是浏览器网页登录会话，不读取本地凭据或 native storage。
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

/**
 * 当前已实现产品的账号适配器（P15-10 单产品塌缩）：路由参数只剩协议值比较，
 * 完整账号事务路径（AccountCenter 主路径）不经此工厂，适配器由测试注入。
 */
export function accountAdapterFor(productId: string): AccountAdapter | null {
  return productId === CURRENT_PRODUCT_ID ? createTraeCnAccountAdapter() : null;
}
