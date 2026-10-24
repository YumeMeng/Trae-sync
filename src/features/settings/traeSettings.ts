/**
 * TRAE 设置页 builder（P4f-2）：快照 → SettingsView 的纯映射（零 React / 零 JSX / 零命令 / 零 invoke）。
 * 对照 TraeSettingsVariant.tsx（简写 L，556 行）：签到区 L269-316、备份区 L318-451、位置弹层 L452-504、
 * 还原弹层 L505-524、台账 L532-539、时间/体积格式化 L542-556。能力只读 settingsCapabilitiesFor("trae_cn")。
 * 弹层（位置/还原）不进视图模型：其开关与表单状态归 P4f-4 的 BackupDialogs + hook 持有。
 */
import type { AutoCheckinStatusDto } from "../../types/account_switch";
import type { BackupRetentionDto, MasterBackupChainDto, MasterBackupEntryDto } from "../../types/masterLibrary";
import { SETTINGS_TEXTS, fillSettingsText, settingsCapabilitiesFor, settingsLedgerText,
  type AutoCheckinView, type BackupChainRowView, type BackupSectionView, type SettingsActionView,
  type SettingsBadgeView, type SettingsCapabilities, type SettingsView } from "./config";

/** 忙碌态：builder 只用来算 disabled 与执行中文案；动作编排（invoke）留在页面/hook。 */
export interface TraeSettingsBusy {
  readonly autoCheckin: boolean; readonly backupCreate: boolean; readonly location: boolean; readonly restore: boolean;
}
// 保留下拉与凭据勾选的禁用态（旧 L338/L397 的 retentionBusy/backupBusy）在本视图模型里没有对应字段
// （config.ts 的 retention 与 credentialsToggle 只表达值，不表达禁用）：不塞进 busy 造假入口，
// 由 P4f-4 的 hook 在接线时补字段，本 builder 不提前臆造。
/** 聚合页面需要的所有已取回数据：builder 只消费，不发起任何读取；null 一律按旧实现降级。 */
export interface TraeSettingsSnapshot {
  readonly autoCheckin: AutoCheckinStatusDto | null; // get_auto_checkin_settings；null = 首读中（L314-315 占位句）
  readonly backupChain: MasterBackupChainDto | null; // get_master_backup_chain；null = 分区整体不渲染（L320）
  readonly retention: BackupRetentionDto | null; // get_backup_retention；null = 保留块不渲染（L332）
  readonly includeCredentials: boolean; // 凭据勾选（默认 false，L40：避免无意识扩散凭据文件）
  readonly busy: TraeSettingsBusy;
}
/** 完整快照的悬浮句（L362 字面量，未进文案表；文案表只收徽章词）。 */
const FULL_SNAPSHOT_TITLE = "完整数据库快照";
// 禁用原因：旧实现（L376/L410/L424）只置 disabled、无 title，故这三句为本层补的诚实说明，不改按钮词。
const RESTORE_BUSY_REASON = "还原正在进行中";
const CREATE_BUSY_REASON = "备份正在进行中";
const LOCATION_BUSY_REASON = "备份位置变更进行中";
/** 动作构造：旧实现 data-testid 与动作 id 逐字同值（L377/L411/L425），故 testId = id；hint 即旧 title。 */
const act = (id: string, label: string, hint: string | null, busyLabel: string | null = null, disabledReason: string | null = null): SettingsActionView =>
  ({ id, label, testId: id, hint, disabledReason, busyLabel });

/** 备份时间（L542-550 原样）：本地日期 + 时分；只格式化注入的时间戳，不读时钟。 */
const fmtTime = (unixSeconds: number): string => new Date(unixSeconds * 1000).toLocaleString(undefined,
  { year: "numeric", month: "2-digit", day: "2-digit", hour: "2-digit", minute: "2-digit" });
/** 备份大小（L553-556 原样）：MiB 一位小数，其余按 KiB 取整且不小于 1。 */
const fmtSize = (totalBytes: number): string => totalBytes >= 1024 * 1024
  ? fillSettingsText(SETTINGS_TEXTS.sizeMiB, { value: (totalBytes / (1024 * 1024)).toFixed(1) })
  : fillSettingsText(SETTINGS_TEXTS.sizeKiB, { value: Math.max(1, Math.round(totalBytes / 1024)) });

/** 链行（L355-383）：时间/大小 + 附属件·凭据徽章 + 还原动作；TRAE 无勾选、无删除。 */
function rowFor(entry: MasterBackupEntryDto, busy: TraeSettingsBusy): BackupChainRowView {
  const badges: SettingsBadgeView[] = [{
    text: entry.has_wal ? SETTINGS_TEXTS.backupWalBadge : SETTINGS_TEXTS.backupSnapshotBadge, tone: "neutral",
    title: entry.has_wal ? SETTINGS_TEXTS.backupWalTitle : FULL_SNAPSHOT_TITLE,
  }];
  // 含凭据徽章只在真正带凭据子目录时出现（L366-370），不美化也不补默认。
  if (entry.has_credentials) badges.push({ text: SETTINGS_TEXTS.backupCredBadge, tone: "neutral", title: SETTINGS_TEXTS.backupCredTitle });
  const stamp = entry.stamp_unix_seconds;
  return { stampUnixSeconds: stamp, stampLabel: fmtTime(stamp), sizeLabel: fmtSize(entry.total_bytes), badges,
    selectTestId: null, selected: false, // 链上多选删除是 WB 能力（backup.deletion=false）
    restore: act(`master-backup-restore-${stamp}`, SETTINGS_TEXTS.backupRestore, SETTINGS_TEXTS.backupRestoreTitle,
      null, busy.restore ? RESTORE_BUSY_REASON : null) }; // 旧 L376 只禁用不改词，故 busyLabel 恒 null
}
/** 保留块（L332-352）：能力开启且读到数据才出现；keep 只作句内插值（滚动单份已定案）。 */
function retentionFor(snapshot: TraeSettingsSnapshot, caps: SettingsCapabilities): BackupSectionView["retention"] {
  if (!caps.backup.retention || snapshot.retention === null) return null;
  const enabled = snapshot.retention.enabled;
  return { testId: "backup-retention-settings", label: SETTINGS_TEXTS.retentionLabel,
    selectTestId: "backup-retention-enabled-select", enabled,
    onLabel: SETTINGS_TEXTS.retentionOn, offLabel: SETTINGS_TEXTS.retentionOff, hintTestId: "backup-retention-hint",
    hint: enabled ? SETTINGS_TEXTS.retentionHintOn : SETTINGS_TEXTS.retentionHintOff };
}
/** 恢复步骤（L440-449）：首句 + 保留句 + 人工恢复句 +（任一份带凭据时的）凭据子目录句，顺序与拼接逐字。 */
function restoreHintFor(snapshot: TraeSettingsSnapshot, chain: MasterBackupChainDto): BackupSectionView["restoreHint"] {
  const retention = snapshot.retention;
  const steps = retention?.enabled === true
    ? fillSettingsText(SETTINGS_TEXTS.retentionStepsOn, { keep: retention.keep })
    : SETTINGS_TEXTS.retentionStepsOff;
  const credentials = chain.backups.some((entry) => entry.has_credentials) ? SETTINGS_TEXTS.backupStepsCredentials : "";
  return { testId: "master-backup-restore-hint", summary: SETTINGS_TEXTS.backupStepsSummary,
    text: `${SETTINGS_TEXTS.backupStepsHead}${steps}${SETTINGS_TEXTS.backupStepsManual}${credentials}` };
}
/** 备份区（L318-451）：链为 null 即整区不渲染（旧实现可插拔降级）；TRAE 无删除、无保链、无读取失败态。 */
function backupFor(snapshot: TraeSettingsSnapshot, caps: SettingsCapabilities): BackupSectionView | null {
  const chain = snapshot.backupChain;
  if (chain === null) return null;
  const busy = snapshot.busy, count = chain.backups.length;
  return {
    testId: "master-backup-section", eyebrow: SETTINGS_TEXTS.backupEyebrow, title: SETTINGS_TEXTS.backupTitleTrae,
    countBadge: fillSettingsText(SETTINGS_TEXTS.backupCount, { count }), countTone: count > 0 ? "ok" : "neutral",
    loadingNote: null, // 旧实现读取期不占位：失败即整区消失，不报错
    backupDirLabel: SETTINGS_TEXTS.backupDirLabel, backupDir: chain.backup_dir,
    retention: retentionFor(snapshot, caps), // 保留块不消费 busy：见文件头 busy 字段注释
    rows: chain.backups.map((entry) => rowFor(entry, busy)), emptyText: SETTINGS_TEXTS.backupEmptyTrae,
    credentialsToggle: caps.backup.credentials === "toggle-default-off"
      ? { testId: "backup-include-credentials-toggle", title: SETTINGS_TEXTS.credentialsTitle,
        strong: SETTINGS_TEXTS.credentialsStrong, summary: SETTINGS_TEXTS.credentialsSummary,
        checked: snapshot.includeCredentials } : null,
    create: act("master-backup-create", SETTINGS_TEXTS.backupCreate, null,
      SETTINGS_TEXTS.backupBusy, busy.backupCreate ? CREATE_BUSY_REASON : null), // L414 执行中改词「备份中…」
    deleteSelected: null, // TRAE 无删除入口（delete_master_backups 前端零引用，接入即新功能）
    changeLocation: caps.backup.location
      ? act("backup-location-change", SETTINGS_TEXTS.backupChangeLocation, null, null, busy.location ? LOCATION_BUSY_REASON : null)
      : null,
    restoreHint: restoreHintFor(snapshot, chain),
    loadError: null, // 旧实现无错误态与重试入口
  };
}
/** 签到区（L269-316）：内联三件 + 今日台账；TRAE 无参与清单（逐账号参与开关在账号详情页）。 */
function autoCheckinFor(snapshot: TraeSettingsSnapshot): AutoCheckinView {
  const status = snapshot.autoCheckin, enabled = status?.enabled === true;
  return {
    testId: "auto-checkin-settings", eyebrow: SETTINGS_TEXTS.autoEyebrow, title: SETTINGS_TEXTS.autoTitle,
    badge: enabled ? SETTINGS_TEXTS.autoOn : SETTINGS_TEXTS.autoOff, badgeTone: enabled ? "ok" : "neutral",
    enabled, dailyTimeHhmm: status?.daily_time_hhmm ?? "", busy: snapshot.busy.autoCheckin,
    toggleTestId: "auto-checkin-enabled-toggle", toggleTitle: SETTINGS_TEXTS.autoToggleTitle,
    toggleStrong: SETTINGS_TEXTS.autoToggleStrong, toggleSummary: SETTINGS_TEXTS.autoToggleSummary,
    timeLabel: SETTINGS_TEXTS.autoTimeLabel, timeTestId: "auto-checkin-time-input",
    ledgerTestId: "auto-checkin-ledger", ledgerText: settingsLedgerText(status?.ledger ?? null),
    loadingNote: status === null ? SETTINGS_TEXTS.autoLoading : null, // L315 首读占位句
    participation: null, // accountParticipation=accounts-detail-only：参与清单不在设置页
  };
}
/** 快照 → 设置页视图模型（纯函数：同一快照恒得同一视图）。 */
export function buildTraeSettingsView(snapshot: TraeSettingsSnapshot): SettingsView {
  const caps = settingsCapabilitiesFor("trae_cn");
  return {
    productId: "trae_cn",
    header: { eyebrow: SETTINGS_TEXTS.eyebrow, title: SETTINGS_TEXTS.pageTitle,
      regionLabel: SETTINGS_TEXTS.regionLabel, pageTitleAttr: "settings" }, // L261/264/265
    autoCheckin: autoCheckinFor(snapshot), // 页头与徽章常驻，数据未到时给 loadingNote
    backup: backupFor(snapshot, caps),
    // 通知前缀取产品展示名（同 checkin/traeCheckin.ts L137 口径）；页面级兜底句逐字沿用 L255。
    texts: { notificationPrefix: "TRAE", fallbackFailure: SETTINGS_TEXTS.fallbackFailure },
  };
}
