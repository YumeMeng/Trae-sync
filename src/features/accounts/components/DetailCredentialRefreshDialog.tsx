import { ConfirmDialog } from "../../../components/shared/ConfirmDialog";

/**
 * 详情页单账号「刷新登录凭据」二次确认弹层（P4c-7c-1）。
 *
 * 为什么必须确认：换发会写回该账号本机加密登录凭据（旧 AccountDetail L153-162 同一契约，
 * 文案与 testid 逐字沿用）。批量入口有自己的弹层（CredentialRefreshDialog，
 * testId=account-refresh-credentials-confirm），两者不共用——e2e 与旧单测分别断言。
 * 本组件只承载弹层，执行体与忙碌位由页面注入。
 */
export interface DetailCredentialRefreshDialogProps {
  /** 本次换发的账号显示名（文案里的账号名）。 */
  readonly accountName: string;
  /** 换发进行中：两侧按钮禁用，防重复提交。 */
  readonly busy: boolean;
  readonly onCancel: () => void;
  readonly onConfirm: () => void;
}

export function DetailCredentialRefreshDialog({
  accountName, busy, onCancel, onConfirm,
}: DetailCredentialRefreshDialogProps) {
  return (
    <ConfirmDialog
      title="刷新登录凭据"
      lines={[
        `将为账号“${accountName}”向服务端换发新的登录令牌，并更新本机保存的登录信息。`,
        "期间不会打开新的登录页面。",
      ]}
      confirmLabel="刷新凭据"
      busyLabel="刷新中…"
      busy={busy}
      onCancel={onCancel}
      onConfirm={onConfirm}
      testId="account-detail-credentials-confirm"
    />
  );
}
