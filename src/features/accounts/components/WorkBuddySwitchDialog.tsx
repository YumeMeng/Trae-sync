import { ConfirmDialog } from "../../../components/shared/ConfirmDialog";
import type { WorkBuddySwitchPreviewDto } from "../../../types/product";

/**
 * WorkBuddy 切号单次确认弹层（P4c-6b-2d，纯展示）：正文逐字沿用旧 WorkBuddyPanel
 * L591-611 的四行影响面说明与按钮文案，骨架复用共享 ConfirmDialog（与旧实现同一个）。
 * 预览数据由调用方先经 preview_workbuddy_switch 取到；本文件零命令、零状态、零产品判断。
 */
export interface WorkBuddySwitchDialogProps {
  readonly preview: WorkBuddySwitchPreviewDto;
  /** 切号执行中：两侧按钮禁用、确认按钮换「切换中…」。 */
  readonly busy: boolean;
  readonly onCancel: () => void;
  readonly onConfirm: () => void;
}

export function WorkBuddySwitchDialog({ preview, busy, onCancel, onConfirm }: WorkBuddySwitchDialogProps) {
  const cloud = preview.cloud_residual_rows ?? 0;
  return (
    <ConfirmDialog
      title="切换 WorkBuddy 登录账号"
      lines={[
        `将让 WorkBuddy 客户端退出当前账号「${preview.current_label}」并登录到「${preview.target_label ?? "所选账号"}」（写入客户端登录态）。`,
        `切号成功即自动过户：${preview.pending_local_rows ?? 0} 条本地会话（${preview.pending_project_count ?? 0} 个项目）随行转移，无需手动操作。`,
        ...(cloud > 0 ? [`注意：当前账号还有 ${cloud} 条云端会话，不在转移范围（本工具不碰云端数据）。`] : []),
        "需要 WorkBuddy 客户端处于运行状态；切换完成后客户端会自动重启读取新账号。",
      ]}
      confirmLabel="确认切换"
      busyLabel="切换中…"
      busy={busy}
      onCancel={onCancel}
      onConfirm={onConfirm}
      testId="workbuddy-switch-dialog"
    />
  );
}
