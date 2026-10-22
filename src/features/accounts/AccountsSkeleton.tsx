import { EmptyState } from "../../components/shared/EmptyState";
import { ProductBadge } from "../../components/shared/ProductBadge";
import { ACCOUNT_TEXTS } from "./config";

/**
 * 账号页外壳（P4c-6b-1）：快照未到手时只渲染页头 + 一张卡。
 * 加载中 = 「正在读取账号信息…」；读取失败 = 错误卡（带重试），
 * 绝不用空账号冒充读取失败（旧 L281-286 同纪律）。
 * 页标题的 data-page-title 契约沿用旧骨架，App 按此移焦。
 */
export interface AccountsSkeletonProps {
  readonly productId: string;
  /** 读取失败的人话原因；null = 仍在读取（或页面不可见）。 */
  readonly error: string | null;
  /** 失败重试：重新读取本机缓存。 */
  readonly onRetry?: () => void;
  /** 重试进行中（按钮禁用，防重复点击）。 */
  readonly retrying: boolean;
}

export function AccountsSkeleton({ productId, error, onRetry, retrying }: AccountsSkeletonProps) {
  return (
    <section className="account-center" role="region" aria-label={ACCOUNT_TEXTS.pageTitle}>
      <header className="page-header">
        <div className="page-header__copy">
          <h1 data-page-title="accounts" tabIndex={-1}>{ACCOUNT_TEXTS.pageTitle}</h1>
          <ProductBadge productId={productId} />
        </div>
      </header>
      {error === null ? (
        <EmptyState role="status" className="account-center__empty" description={ACCOUNT_TEXTS.loading} />
      ) : (
        <EmptyState
          role="alert"
          panel
          className="account-center__empty"
          testId="account-center__error"
          title="账号信息读取失败"
          description={error}
          action={(
            <button className="btn btn--primary" type="button" onClick={onRetry} disabled={retrying || onRetry === undefined}>
              重新读取
            </button>
          )}
        />
      )}
    </section>
  );
}
