import { ConfirmDialog } from "../../../components/shared/ConfirmDialog";
import { SETTINGS_TEXTS, fillSettingsText, type BackupCapabilities } from "../config";

/**
 * 备份区弹层组（P4f-4b，纯展示）：位置变更弹层（仅 TRAE）+ 恢复确认弹层（两产品文案不同）。
 * 本层只渲染、只回传意图（onConfirm 交回目标目录/迁移勾选、目标 stamp），不执行命令、不读数据、不发通知，
 * 也不判断产品：恢复弹层走两产品唯一差异入口——能力表 safetyChain（none=TRAE / restore-safety-backup=WB）。
 * 文案全部取 SETTINGS_TEXTS，锚点逐字沿用旧实现：
 * 位置弹层 L452-504（backup-location-dialog/-input/-migrate-toggle/-dialog-cancel/-dialog-confirm），
 * 恢复弹层 L505-524（master-restore-dialog）与 WB L227-244（workbuddy-backup-restore-confirm）。
 * 创建/删除确认弹层不在本票范围（留后续票，锚点见 P4f-3 报告 §二）。
 */

/** 位置变更表单草稿（由 hook 持有，本层只回写）：目标目录原文 + 是否迁移现有备份（默认勾选，旧 L42/421）。 */
export interface BackupLocationDialogValue {
  readonly targetDir: string;
  readonly migrateExisting: boolean;
}
/** 位置变更意图：确认时原样交回（targetDir 已 trim），迁移与切换的命令编排归调用方（旧 L156-193）。 */
export interface BackupLocationIntent {
  readonly targetDir: string;
  readonly migrateExisting: boolean;
}

export interface BackupLocationDialogProps {
  /** 当前备份目录（只读展示，完整路径只进悬浮：界面表达纪律，内部路径不进主视野）。 */
  readonly currentDir: string;
  readonly value: BackupLocationDialogValue;
  /** 提交中：两侧按钮与表单全部禁用（旧 locationBusy）。 */
  readonly busy: boolean;
  readonly onChange: (next: BackupLocationDialogValue) => void;
  readonly onCancel: () => void;
  /** 确认出口：只回传意图，不落任何副作用。 */
  readonly onConfirm: (intent: BackupLocationIntent) => void;
}

/**
 * 更改备份位置弹层（TRAE 独有，旧 L452-504）：当前目录 + 目标目录输入 + 「同时迁移现有备份」开关。
 * 与旧实现同骨架（switch-veil/switch-dialog），不额外加遮罩点击关闭与 ESC——旧弹层没有，本票不新造行为。
 */
export function BackupLocationDialog({ currentDir, value, busy, onChange, onCancel, onConfirm }: BackupLocationDialogProps) {
  const targetDir = value.targetDir.trim();
  return (
    <div className="switch-veil" role="presentation">
      <div className="switch-dialog" role="dialog" aria-modal="true" aria-label={SETTINGS_TEXTS.locationDialogTitle}
        data-testid="backup-location-dialog">
        <div className="switch-dialog__title">{SETTINGS_TEXTS.locationDialogTitle}</div>
        <p className="settings-backup__hint">{SETTINGS_TEXTS.locationDialogHint}</p>
        {/* 当前目录：旧弹层未显示，本票按任务书补一行，文案复用既有键（不新增同义句）。 */}
        <p className="account-center__meta">
          {SETTINGS_TEXTS.backupDirLabel}
          <code className="settings-panel__backup-dir" title={currentDir}>{currentDir}</code>
        </p>
        <input className="settings-panel__text-input" type="text" value={value.targetDir}
          placeholder={SETTINGS_TEXTS.locationPlaceholder} disabled={busy} aria-label={SETTINGS_TEXTS.locationAria}
          data-testid="backup-location-input"
          onChange={(event) => onChange({ ...value, targetDir: event.target.value })} />
        <label className="settings-panel__toggle-row" data-testid="backup-location-migrate-toggle">
          <input type="checkbox" checked={value.migrateExisting} disabled={busy}
            onChange={(event) => onChange({ ...value, migrateExisting: event.target.checked })} />
          <span className="account-center__profile-copy">
            <strong>{SETTINGS_TEXTS.locationMigrateStrong}</strong>
            <span>{SETTINGS_TEXTS.locationMigrateSummary}</span>
          </span>
        </label>
        <div className="switch-dialog__actions">
          <button className="btn btn--quiet" type="button" onClick={onCancel} disabled={busy}
            data-testid="backup-location-dialog-cancel">{SETTINGS_TEXTS.cancel}</button>
          <button className="btn btn--primary" type="button" disabled={busy || targetDir === ""}
            data-testid="backup-location-dialog-confirm"
            onClick={() => onConfirm({ targetDir, migrateExisting: value.migrateExisting })}>
            {busy ? SETTINGS_TEXTS.saving : SETTINGS_TEXTS.save}
          </button>
        </div>
      </div>
    </div>
  );
}

export interface BackupRestoreDialogProps {
  /** 目标备份 stamp（确认时原样交回，由调用方决定还原/恢复命令）。 */
  readonly stampUnixSeconds: number;
  /** 目标 stamp 的展示文案（WB 正文首句要插值；格式由调用方按各自口径给，本层不重复格式化）。 */
  readonly stampLabel: string;
  /** 能力表：safetyChain 决定文案与锚点（none=TRAE 覆盖主库 / restore-safety-backup=WB 保链回退点）。 */
  readonly capabilities: BackupCapabilities;
  readonly busy: boolean;
  readonly onCancel: () => void;
  /** 确认出口：只回传目标 stamp，不执行还原。 */
  readonly onConfirm: (stampUnixSeconds: number) => void;
}

/**
 * 恢复确认弹层（两产品各一套文案，逐字保留，不合并成通用话）：
 * TRAE（L505-524）四段＝标题 + 三行后果/边界/体检说明，锚 master-restore-dialog，确认词「确认还原」，非危险色；
 * WB（L227-244）四段＝标题 + 三行（目标 stamp / 恢复前保链备份可回退 / 客户端关闭重启），
 * 锚 workbuddy-backup-restore-confirm，确认词「确认恢复」，危险色。
 * WB 的 safety_backup_stamp 回执上屏属执行回执（hook 侧 notify），弹层只承担「保链说明」这一层。
 */
export function BackupRestoreDialog({ stampUnixSeconds, stampLabel, capabilities, busy, onCancel, onConfirm }: BackupRestoreDialogProps) {
  const safetyChain = capabilities.safetyChain === "restore-safety-backup";
  return (
    <ConfirmDialog
      title={safetyChain ? SETTINGS_TEXTS.wbRestoreTitle : SETTINGS_TEXTS.restoreDialogTitle}
      lines={safetyChain
        ? [fillSettingsText(SETTINGS_TEXTS.wbRestoreLine1, { stamp: stampLabel }),
          SETTINGS_TEXTS.wbRestoreLine2, SETTINGS_TEXTS.wbRestoreLine3]
        : [SETTINGS_TEXTS.restoreLine1, SETTINGS_TEXTS.restoreLine2, SETTINGS_TEXTS.restoreLine3]}
      confirmLabel={safetyChain ? SETTINGS_TEXTS.wbRestoreConfirm : SETTINGS_TEXTS.restoreConfirm}
      busyLabel={safetyChain ? SETTINGS_TEXTS.wbRestoring : SETTINGS_TEXTS.restoring}
      danger={safetyChain} busy={busy} onCancel={onCancel}
      onConfirm={() => onConfirm(stampUnixSeconds)}
      testId={safetyChain ? "workbuddy-backup-restore-confirm" : "master-restore-dialog"} />
  );
}
