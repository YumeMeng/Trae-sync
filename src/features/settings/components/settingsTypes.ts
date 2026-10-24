/**
 * 设置页动作层的公共形状（P4f-5）：刷新出口、弹层状态、动作出口。
 * 零 React、零命令：两产品的动作实现分别在 settingsActionsTrae.ts / settingsActionsWb.ts，
 * 页面与两 hook 只依赖本文件与 SettingsPageData，故三者都不必知道对方产品。
 */

/** 动作可用的刷新出口：读取代次与 active 门禁都在 useSettings 里，本层只调用。 */
export interface SettingsIo {
  readonly reloadAuto: () => Promise<unknown>;
  readonly reloadChain: () => Promise<unknown>;
  readonly reloadRetention: () => Promise<unknown>;
}
/** WB 无保留设置（滚动单份在 create 内），故只给两路刷新。 */
export interface WbIo {
  readonly reloadAuto: () => Promise<unknown>;
  readonly reloadChain: () => Promise<unknown>;
}

/* ===== 位置变更弹层 / 单次确认弹层 / 恢复弹层的页面状态形状（页面只渲染，不判断产品） ===== */

/** 位置变更弹层状态：null = 未打开（WB 无该能力，恒 null）。 */
export interface LocationDialogState {
  readonly value: { readonly targetDir: string; readonly migrateExisting: boolean };
  readonly busy: boolean;
  readonly onChange: (next: { readonly targetDir: string; readonly migrateExisting: boolean }) => void;
  readonly onCancel: () => void;
  readonly onConfirm: (intent: { readonly targetDir: string; readonly migrateExisting: boolean }) => void;
}
/** 恢复确认弹层状态：null = 未打开；stampLabel 由页面按视图行取（两产品时间格式不同，本层不格式化）。 */
export interface RestoreDialogState {
  readonly stamp: number;
  readonly busy: boolean;
  readonly onCancel: () => void;
  readonly onConfirm: () => void;
}
/** 通用单次确认弹层状态（WB 创建/删除；TRAE 恒 null）——字段与 ConfirmDialog props 同名。 */
export interface ConfirmDialogState {
  readonly title: string; readonly lines: readonly string[]; readonly confirmLabel: string;
  readonly busyLabel: string; readonly busy: boolean; readonly danger: boolean;
  readonly onCancel: () => void; readonly onConfirm: () => void; readonly testId: string;
}
/** 页面统一动作出口：两产品同形，页面零产品分支（无该能力的动作在各自 hook 里为空实现）。 */
export interface SettingsActions {
  readonly toggleAuto: (enabled: boolean) => void;
  readonly changeTime: (dailyTimeHhmm: string) => void;
  readonly toggleParticipation: (profileId: string, participating: boolean) => void;
  readonly participateAll: () => void;
  readonly createBackup: () => void;
  readonly startRestore: (stamp: number) => void;
  readonly toggleSelect: (stamp: number, selected: boolean) => void;
  readonly deleteSelected: () => void;
  readonly openLocation: () => void;
  readonly toggleCredentials: (checked: boolean) => void;
  readonly changeRetention: (enabled: boolean) => void;
  readonly retry: () => void;
}
