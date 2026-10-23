import type { ReactNode } from "react";
import { RemoteDevicesPanel } from "../../../components/RemoteDevicesPanel";
import type { DetailViewModel } from "../config";
import { AccountDetailPage, type DangerConfirmCopy } from "./AccountDetailPage";
import type { PageDevices } from "../pageAdapters";

/**
 * 详情视图插槽（P4c-6b-2e-1 从 AccountsPage 抽出）：页内唯一的产品相关落位——登录设备面板
 * 按接缝登记注入（旧 AccountDetail L488-497 的位置由 AccountDetailPage 自己保证）。
 * 抽出动机：AccountsPage 需守住 UI-CHARTER 的 300 行预算；本文件零状态、零命令、零产品判断。
 */
export interface DetailViewSlotProps {
  readonly view: DetailViewModel;
  /** 进行中的动作 id（只影响被点的按钮）。 */
  readonly busy: readonly string[];
  readonly onBack: () => void;
  readonly onAction: (actionId: string) => void;
  readonly dangerConfirm: (actionId: string) => DangerConfirmCopy | null;
  /** 设备面板绑定；undefined = 本产品无该能力（WB devices=false），不占位。 */
  readonly devices: PageDevices | undefined;
  readonly profileId: string;
  readonly onSignedOut: () => void;
  /** 弹层槽（凭据刷新确认 + 重置设备确认）。 */
  readonly dialogs: ReactNode;
}

export function DetailViewSlot(props: DetailViewSlotProps) {
  const { view, busy, onBack, onAction, dangerConfirm, devices, profileId, onSignedOut, dialogs } = props;
  return (
    <AccountDetailPage
      view={view}
      busy={busy}
      onBack={onBack}
      onAction={onAction}
      dangerConfirm={dangerConfirm}
      devicesSlot={devices === undefined ? undefined : (
        <RemoteDevicesPanel
          productId={devices.productId}
          productName={devices.productName}
          profileId={profileId}
          accountLabel={view.header.title}
          onSignedOut={onSignedOut}
        />
      )}
      dialogsSlot={dialogs}
    />
  );
}
