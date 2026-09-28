import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { ArrowRight, Check, LogIn, RefreshCw, ShieldCheck, UserRound } from "lucide-react";
import type { ProductDefinition, ProductId } from "../platform/productRegistry";
import type {
  ProductLoginBeginDto,
  ProductIdentityStateDto,
  TraeCnAccountDto,
  TraeCnLoginReceiptDto,
} from "../types/product";
import type { WorkspaceStateDto } from "../types/workspace";
import { safeUiErrorMessage } from "../utils/safeUiError";

interface ProductAccountPageProps {
  product: ProductDefinition;
  state: WorkspaceStateDto;
  onSwitchProduct: (productId: ProductId) => void;
}

function displayTime(value: string | null): string {
  if (!value) return "刚刚";
  const parsed = new Date(value);
  return Number.isNaN(parsed.getTime()) ? "最近" : parsed.toLocaleString("zh-CN");
}

/**
 * Trae CN 迁移行为基线：独立 OAuth、独立加密账号池和独立当前账号指针。
 * 生产路由已统一到 AccountWorkbench；此组件只供迁移测试保留，不再继续扩展 UI。
 */
export function ProductAccountPage({ product, state, onSwitchProduct }: ProductAccountPageProps) {
  const [identityState, setIdentityState] = useState<ProductIdentityStateDto | null>(null);
  const [accounts, setAccounts] = useState<readonly TraeCnAccountDto[]>([]);
  const [loading, setLoading] = useState(true);
  const [loginBusy, setLoginBusy] = useState(false);
  const [switchingProfileId, setSwitchingProfileId] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [message, setMessage] = useState<string | null>(null);

  const loadAccounts = useCallback(async () => {
    const next = await invoke<TraeCnAccountDto[]>("list_trae_cn_accounts");
    setAccounts(next);
  }, []);

  useEffect(() => {
    let cancelled = false;
    setLoading(true);
    setError(null);
    void Promise.all([
      invoke<ProductIdentityStateDto[]>("get_product_identity_state"),
      invoke<TraeCnAccountDto[]>("list_trae_cn_accounts"),
    ])
      .then(([identityItems, accountItems]) => {
        if (cancelled) return;
        setIdentityState(identityItems.find((item) => item.product_id === product.id) ?? null);
        setAccounts(accountItems);
      })
      .catch((reason: unknown) => {
        if (cancelled) return;
        // 后端暂不可用时保持最保守状态，不把失败伪装成已登录。
        setError(safeUiErrorMessage(reason, "Trae CN 账号信息暂时不可读取。"));
        setIdentityState(null);
        setAccounts([]);
      })
      .finally(() => {
        if (!cancelled) setLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [product.id]);

  const currentAccount = accounts.find((account) => account.is_current) ?? null;
  const identityMessage = identityState?.relation_to_work === "same_identity"
    ? "已发现与 Work CN 相同的 TRAE 身份。"
    : identityState?.relation_to_work === "different_identity"
      ? "已发现 Trae CN 当前保存的是另一个 TRAE 身份。"
      : "尚未确认与 Work CN 的 TRAE 身份关系。";
  const identityLabel = currentAccount
    ? `当前账号：${currentAccount.display_name}`
    : state.current_account.detected || identityState?.identity_status === "recognized"
      ? "已发现账号材料"
      : "尚未识别账号";
  const statusLabel = accounts.length > 0 ? `已保存 ${accounts.length} 个账号` : product.statusLabel;

  const handleLogin = useCallback(async () => {
    if (loginBusy) return;
    setLoginBusy(true);
    setError(null);
    setMessage(null);
    try {
      // CN 登录显式使用本机浏览器：复用的是网页登录会话，不是 Work 本地凭据。
      await invoke<ProductLoginBeginDto>("begin_trae_cn_login", { useSystemBrowser: true });
      const receipt = await invoke<TraeCnLoginReceiptDto>("complete_trae_cn_login");
      await loadAccounts();
      setMessage(`账号“${receipt.screen_name || "Trae CN 账号"}”登录成功，已保存到 Trae CN。`);
    } catch (reason: unknown) {
      if (typeof reason === "string" && reason === "login_cancelled") {
        setMessage("登录已取消，可重新发起 Trae CN 授权。");
      } else {
        setError(safeUiErrorMessage(reason, "Trae CN 登录未完成，请重新发起授权。"));
      }
    } finally {
      setLoginBusy(false);
    }
  }, [loadAccounts, loginBusy]);

  const handleSwitch = useCallback(async (profileId: string) => {
    if (switchingProfileId) return;
    setSwitchingProfileId(profileId);
    setError(null);
    setMessage(null);
    try {
      await invoke<TraeCnAccountDto[]>("switch_trae_cn_account", { profileId });
      await loadAccounts();
      setMessage("Trae CN 当前账号已切换。");
    } catch (reason: unknown) {
      setError(safeUiErrorMessage(reason, "Trae CN 账号切换未完成。"));
    } finally {
      setSwitchingProfileId(null);
    }
  }, [loadAccounts, switchingProfileId]);

  return (
    <section className="product-account-page" role="region" aria-label={`${product.displayName} 账号`}>
      <header className="page-header product-account-page__header">
        <div className="page-header__copy">
          <span className="page-header__eyebrow">{product.displayName}</span>
          <h1 data-page-title="accounts" tabIndex={-1}>账号</h1>
          <p>Trae CN 使用独立授权和账号池；浏览器会话可以复用，但不会复制 Work CN 的本地登录材料。</p>
        </div>
        <span className="badge badge--off" data-testid="product-credential-status">
          {statusLabel}
        </span>
      </header>

      {error && <div className="workbench__error" role="alert">{error}</div>}
      {message && <div className="product-account-page__message" role="status">{message}</div>}

      <div className="product-account-page__grid">
        <article className="product-account-page__card">
          <div className="product-account-page__card-icon" aria-hidden="true">
            <UserRound size={20} strokeWidth={1.8} />
          </div>
          <div>
            <h2>当前产品账号</h2>
            <strong>{identityLabel}</strong>
            <p>
              {identityMessage} Trae CN 产品会话需单独授权；Work CN 的登录凭据不会自动写入 Trae CN。
            </p>
          </div>
        </article>

        <article className="product-account-page__card product-account-page__card--note">
          <div className="product-account-page__card-icon" aria-hidden="true">
            <ShieldCheck size={20} strokeWidth={1.8} />
          </div>
          <div>
            <h2>适配状态</h2>
            <p>
              Trae CN 已支持独立登录、账号保存和当前账号切换；签到仍由 Work CN 管理，
              不会在这里重复显示。
            </p>
          </div>
        </article>
      </div>

      <section className="product-account-page__accounts" aria-label="Trae CN 已保存账号">
        <div className="product-account-page__section-header">
          <div>
            <h2>已保存的 Trae CN 账号</h2>
            <p>账号凭据使用系统保护存储，切换只改变 Trae Sync 当前选择。</p>
          </div>
          <button
            type="button"
            className="btn btn--primary"
            onClick={() => void handleLogin()}
            disabled={loginBusy}
            data-testid="trae-cn-login"
          >
            {loginBusy ? <RefreshCw size={15} className="icon-spin" aria-hidden="true" /> : <LogIn size={15} aria-hidden="true" />}
            {loginBusy ? "等待浏览器授权…" : "登录 Trae CN"}
          </button>
        </div>

        {loading ? (
          <p className="product-account-page__empty" role="status">正在读取 Trae CN 账号…</p>
        ) : accounts.length === 0 ? (
          <div className="product-account-page__empty">
            <strong>还没有保存的 Trae CN 账号</strong>
            <p>点击“登录 Trae CN”，在浏览器完成一次独立授权即可保存。</p>
          </div>
        ) : (
          <ul className="product-account-page__account-list">
            {accounts.map((account) => (
              <li key={account.profile_id} className="product-account-page__account-row">
                <div className="product-account-page__account-avatar" aria-hidden="true">
                  <UserRound size={18} strokeWidth={1.8} />
                </div>
                <div className="product-account-page__account-copy">
                  <strong>{account.display_name}</strong>
                  <span>最近授权：{displayTime(account.last_verified_at)}</span>
                </div>
                {account.is_current ? (
                  <span className="product-account-page__current-badge">
                    <Check size={14} aria-hidden="true" /> 当前使用
                  </span>
                ) : (
                  <button
                    type="button"
                    className="btn"
                    onClick={() => void handleSwitch(account.profile_id)}
                    disabled={switchingProfileId !== null}
                    data-testid="trae-cn-switch-account"
                  >
                    {switchingProfileId === account.profile_id ? "切换中…" : "切换"}
                  </button>
                )}
              </li>
            ))}
          </ul>
        )}
      </section>

      <div className="product-account-page__footer">
        <p>如需使用已保存的 Work CN 账号，请切回 Work CN；两边的账号池和授权状态互不覆盖。</p>
        <button
          type="button"
          className="btn"
          onClick={() => onSwitchProduct("work_cn")}
          data-testid="switch-to-work-product"
        >
          回到 Work CN 账号 <ArrowRight size={15} aria-hidden="true" />
        </button>
      </div>
    </section>
  );
}
