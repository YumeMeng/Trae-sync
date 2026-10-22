import { ConfirmDialog } from "../../../components/shared/ConfirmDialog";
import { ACCOUNT_TEXTS } from "../config";

/**
 * 批量刷新登录凭据的二次确认弹层（P4c-6c-1 接线补齐）。
 *
 * 为什么必须确认：批量换发会逐个写回全部账号的本机登录信息，属防误触入口
 * （旧 AccountCenterTrae L780-795 同一契约，文案逐字沿用）。本组件只承载弹层，
 * 执行体与忙碌位由页面注入——不做任何读取/命令，也不判断产品。
 */
export interface CredentialRefreshDialogProps {
  /** 本次将刷新的账号数（文案里的 N；0 时调用方不应打开弹层）。 */
  readonly count: number;
  /** 换发进行中：两侧按钮禁用，防重复提交。 */
  readonly busy: boolean;
  readonly onCancel: () => void;
  readonly onConfirm: () => void;
}

export function CredentialRefreshDialog({ count, busy, onCancel, onConfirm }: CredentialRefreshDialogProps) {
  return (
    <ConfirmDialog
      title="刷新全部账号凭据"
      lines={[
        `将为全部 ${count} 个账号逐个向服务端换发新的登录令牌，并更新本机保存的登录信息。`,
        "期间不会打开新的登录页面。",
      ]}
      confirmLabel={ACCOUNT_TEXTS.refreshCredentials}
      busyLabel="刷新中…"
      busy={busy}
      onCancel={onCancel}
      onConfirm={onConfirm}
      testId="account-refresh-credentials-confirm"
    />
  );
}
