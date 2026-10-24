/**
 * WorkBuddy 设置页 builder（P4f-3）：快照 → SettingsView 的纯映射（零 React / 零 JSX / 零命令 / 零 invoke）。
 * 对照 WorkBuddySettingsVariant.tsx（46 行，页头 + 两分区标题）、WorkBuddyBackupSection.tsx（WB，261 行）、
 * WorkBuddyAutoCheckinSettings.tsx（WBA，109 行）、shared/checkin/AutoCheckinSettingsCard.tsx（CARD，131 行）。
 * WB 独有三块：参与清单（list_workbuddy_checkin_accounts）、备份四件套（list/create/restore/delete，含
 * safety_backup_stamp 回退点）、自动签到三件套（status/set/set_account）。能力只读
 * settingsCapabilitiesFor("workbuddy")。
 * 弹层（创建/恢复/删除）与回执不进视图模型：开关与表单状态归 P4f-4 的 BackupDialogs + hook 持有。
 */
import type { WorkBuddyCheckinParticipationDto } from "../../types/product";
import type { WorkBuddyBackupChainDto, WorkBuddyBackupEntryDto } from "../../types/workbuddyBackup";
import type { WorkBuddyAutoCheckinStatusDto } from "../../platform/workbuddyAdapter";
import { SETTINGS_TEXTS, fillSettingsText, settingsCapabilitiesFor,
  type AutoCheckinView, type BackupChainRowView, type BackupSectionView, type SettingsActionView,
  type SettingsBadgeView, type SettingsCapabilities, type SettingsView } from "./config";

/** 忙碌态：WB 旧实现两个独立 busy（WBA autoBusy / WB 备份 busy），builder 只用来算禁用与说明。 */
export interface WorkBuddySettingsBusy { readonly autoCheckin: boolean; readonly backup: boolean }
/** 聚合 WB 设置页需要的已取回数据：builder 只消费、不发起读取；null/空一律按旧实现降级。 */
export interface WorkBuddySettingsSnapshot {
  readonly autoCheckin: WorkBuddyAutoCheckinStatusDto | null; // get_workbuddy_auto_checkin_status；null = 读失败（WBA L29）
  readonly participation: readonly WorkBuddyCheckinParticipationDto[]; // list_workbuddy_checkin_accounts；[] = 读失败或空清单
  readonly backupChain: WorkBuddyBackupChainDto | null; // list_workbuddy_backups；null = 首读中或读失败（WB L117-131）
  readonly backupLoading: boolean; // 旧 loading 位：读取中优先于链数据（WB L117）
  readonly backupLoadError: string | null; // 已按 safeUiErrorMessage 映射的文案（WB L37）；null = 无错误
  readonly selectedStamps: readonly number[]; // 链上多选（WB 独有删除入口）
  readonly busy: WorkBuddySettingsBusy;
}
/** 每日触发时间缺省（WBA L90 原样）：状态未读到时旧实现就按 09:30 渲染控件。 */
const DEFAULT_DAILY_TIME = "09:30";
// 禁用原因：旧实现只置 disabled、无 title（WB L138/L147/L189、CARD L90），故这几句为本层补的诚实说明。
const BACKUP_BUSY_REASON = "备份操作进行中";
const AUTO_BUSY_REASON = "自动签到操作进行中";
const NO_SELECTION_REASON = "尚未选择备份";
/** 动作构造：WB 旧实现 data-testid 与动作 id 逐字同值（WB L139/L148/L190、CARD L91），故 testId = id。 */
const act = (id: string, label: string, hint: string | null, busyLabel: string | null = null, disabledReason: string | null = null): SettingsActionView =>
  ({ id, label, testId: id, hint, disabledReason, busyLabel });

/** 备份时间（WB L99 原样）：toLocaleString 默认格式；TRAE 显式给 options，两产品各自逐字保留。 */
const fmtTime = (unixSeconds: number): string => new Date(unixSeconds * 1000).toLocaleString();
/** 体积（WB L100-101 原样）：>1 MiB 一位小数 MB，否则取整 KB；null = 「—」（清单缺失时如实呈现）。 */
const fmtBytes = (bytes: number | null): string => bytes === null ? SETTINGS_TEXTS.wbBytesUnknown
  : bytes > 1024 * 1024 ? `${(bytes / 1024 / 1024).toFixed(1)} MB` : `${Math.round(bytes / 1024)} KB`;

/** 今日台账（WBA L93-97 原样）：null = 不渲染该行（WB 与 TRAE 不同，不写「今日尚未发起」）。 */
function ledgerTextFor(status: WorkBuddyAutoCheckinStatusDto | null): string {
  const ledger = status?.ledger;
  if (!ledger) return "";
  const failed = ledger.failed > 0 ? fillSettingsText(SETTINGS_TEXTS.wbAutoLedgerFailed, { failed: ledger.failed }) : "";
  const frame = ledger.running ? SETTINGS_TEXTS.wbAutoLedgerRunning : SETTINGS_TEXTS.wbAutoLedgerDone;
  return fillSettingsText(frame, { completed: ledger.completed, total: ledger.total, failed });
}

/** 链行（WB L157-198）：可勾选 + 时间 + 文件数/源/压缩 + 半成品标注 + 恢复动作。 */
function rowFor(entry: WorkBuddyBackupEntryDto, busy: boolean, selected: readonly number[]): BackupChainRowView {
  const stamp = entry.stamp_unix_seconds;
  // 清单不可读（file_count=null）如实标「清单缺失」，不美化也不补默认（WB L173-175）。
  const badges: SettingsBadgeView[] = entry.file_count === null
    ? [{ text: SETTINGS_TEXTS.backupManifestMissing, tone: "warn", title: SETTINGS_TEXTS.backupManifestMissingTitle }]
    : [];
  const files = entry.file_count === null
    ? SETTINGS_TEXTS.wbFileCountUnknown
    : fillSettingsText(SETTINGS_TEXTS.wbFileCount, { count: entry.file_count });
  return { stampUnixSeconds: stamp, stampLabel: fmtTime(stamp),
    sizeLabel: fillSettingsText(SETTINGS_TEXTS.wbRowMeta,
      { files, source: fmtBytes(entry.source_bytes), compressed: fmtBytes(entry.compressed_bytes) }),
    badges, selectTestId: `workbuddy-backup-select-${stamp}`, selected: selected.includes(stamp),
    // 旧按钮文字恒「恢复」（WB L185-194），故 busyLabel 恒 null；hint 即旧 title。
    restore: act(`workbuddy-backup-restore-${stamp}`, SETTINGS_TEXTS.backupRestoreWb, SETTINGS_TEXTS.backupRestoreWbTitle,
      null, busy ? BACKUP_BUSY_REASON : null) };
}

/** 备份区（WB L103-207）：分区壳恒在（无链时只有标题与空态）；WB 无保留/位置/凭据开关/恢复步骤折叠区。 */
function backupFor(snapshot: WorkBuddySettingsSnapshot, caps: SettingsCapabilities): BackupSectionView {
  const chain = snapshot.backupChain, busy = snapshot.busy.backup, selected = snapshot.selectedStamps;
  const count = chain?.backups.length ?? 0, dir = chain?.backup_dir ?? "";
  const loadError = snapshot.backupLoadError;
  return {
    testId: "workbuddy-backup-section", eyebrow: SETTINGS_TEXTS.backupEyebrow, title: SETTINGS_TEXTS.backupTitleWb,
    // 份数徽章只在链可读时出现（WB L110-114）：无链不冒充 0 份。
    countBadge: chain === null ? "" : fillSettingsText(SETTINGS_TEXTS.backupCount, { count }),
    countTone: count > 0 ? "ok" : "neutral",
    loadingNote: snapshot.backupLoading ? SETTINGS_TEXTS.backupLoadingWb : null, // WB L117 读取中优先
    // WB 旧实现单行「备份目录：{dir}」（WB L152）；TRAE 是前缀 + code，两产品各自逐字保留。
    backupDirLabel: fillSettingsText(SETTINGS_TEXTS.backupDirLabelWb, { dir }), backupDir: dir,
    retention: null, // WB 无保留设置（caps.backup.retention=false；滚动单份在 create 内，回执 rolled_over）
    rows: chain?.backups.map((entry) => rowFor(entry, busy, selected)) ?? [],
    emptyText: SETTINGS_TEXTS.backupEmptyWb, // WB L200-205 空态
    credentialsToggle: null, // WB 固定含凭据（caps.backup.credentials=always-included，P19 决策 1）
    // 旧按钮文字恒「立即备份」（WB L141），故 busyLabel 恒 null（TRAE 才改词「备份中…」）。
    create: act("workbuddy-backup-create", SETTINGS_TEXTS.backupCreate, null, null, busy ? BACKUP_BUSY_REASON : null),
    deleteSelected: caps.backup.deletion
      ? act("workbuddy-backup-delete", fillSettingsText(SETTINGS_TEXTS.backupDeleteSelected, { count: selected.length }),
        null, null, busy ? BACKUP_BUSY_REASON : selected.length === 0 ? NO_SELECTION_REASON : null)
      : null,
    changeLocation: caps.backup.location
      ? act("backup-location-change", SETTINGS_TEXTS.backupChangeLocation, null, null, busy ? BACKUP_BUSY_REASON : null)
      : null, // WB 固定 storage_root/workbuddy-backups：无位置变更
    restoreHint: null, // WB 无「查看恢复步骤」折叠区（保链说明在恢复确认弹层，归 P4f-4）
    // 读取失败态（WB L119-130）：标题 + 已映射文案 + 重试入口；无错误时整块 null。
    loadError: loadError === null ? null : { title: SETTINGS_TEXTS.wbLoadErrorTitle,
      description: loadError || SETTINGS_TEXTS.wbLoadErrorFallback, retryLabel: SETTINGS_TEXTS.retry },
  };
}

/** 签到区（WBA L19-107 + CARD）：共享卡三件 + 参与清单 + 全部参与；参与清单是 WB 独有。 */
function autoCheckinFor(snapshot: WorkBuddySettingsSnapshot, caps: SettingsCapabilities): AutoCheckinView {
  const status = snapshot.autoCheckin;
  const rows = snapshot.participation.map((account) => ({
    profileId: account.profile_id, displayName: account.screen_name,
    participating: account.enabled, // 后端有效参与（list 命令；P19-B6 数据源统一）
  }));
  const pending = rows.filter((row) => !row.participating).length;
  return {
    testId: "workbuddy-auto-checkin-settings", eyebrow: SETTINGS_TEXTS.autoEyebrow, title: SETTINGS_TEXTS.autoTitle,
    badge: "", badgeTone: "neutral", // WB 分区标题无状态徽章（WorkBuddySettingsVariant L29-34）：不新造
    enabled: status?.enabled === true, dailyTimeHhmm: status?.daily_time_hhmm ?? DEFAULT_DAILY_TIME,
    busy: snapshot.busy.autoCheckin,
    toggleTestId: "workbuddy-auto-checkin-enabled-toggle", toggleTitle: SETTINGS_TEXTS.wbAutoToggleTitle,
    // 「每日自动签到」与 TRAE 逐字同句（CARD L59），故复用同一文案键，不新增同义键。
    toggleStrong: SETTINGS_TEXTS.autoToggleStrong, toggleSummary: SETTINGS_TEXTS.wbAutoToggleSummary,
    timeLabel: SETTINGS_TEXTS.autoTimeLabel, timeTestId: "workbuddy-auto-checkin-time-input",
    ledgerTestId: "workbuddy-auto-checkin-ledger", ledgerText: ledgerTextFor(status),
    loadingNote: null, // WB 旧实现挂载即渲染默认值（WBA L87-97），无首读占位句
    // 参与清单只在有账号时渲染（CARD L81）；旧实现该块只有 ul 一个容器 testid，故两字段同值，不新造锚点。
    participation: rows.length === 0 ? null : {
      testId: "workbuddy-auto-checkin-accounts", listTestId: "workbuddy-auto-checkin-accounts",
      label: SETTINGS_TEXTS.wbParticipationLabel, defaultName: SETTINGS_TEXTS.wbAccountFallback, rows,
      participateAll: caps.participateAll ? act("workbuddy-auto-checkin-participate-all",
        pending > 0 ? `${SETTINGS_TEXTS.wbParticipateAll}（${pending}）` : SETTINGS_TEXTS.wbParticipateAll,
        pending === 0 ? SETTINGS_TEXTS.wbParticipateAllNone
          : fillSettingsText(SETTINGS_TEXTS.wbParticipateAllHint, { count: pending }),
        null, snapshot.busy.autoCheckin ? AUTO_BUSY_REASON : pending === 0 ? SETTINGS_TEXTS.wbParticipateAllNone : null)
        : null,
    },
  };
}

/** 快照 → 设置页视图模型（纯函数：同一快照恒得同一视图）。 */
export function buildWorkBuddySettingsView(snapshot: WorkBuddySettingsSnapshot): SettingsView {
  const caps = settingsCapabilitiesFor("workbuddy");
  return {
    productId: "workbuddy",
    // 页头与 TRAE 逐字相同（WorkBuddySettingsVariant L21-27）：role=region aria-label="设置" + h1[data-page-title=settings]。
    header: { eyebrow: SETTINGS_TEXTS.eyebrow, title: SETTINGS_TEXTS.pageTitle,
      regionLabel: SETTINGS_TEXTS.regionLabel, pageTitleAttr: "settings" },
    autoCheckin: autoCheckinFor(snapshot, caps),
    backup: backupFor(snapshot, caps),
    // 通知前缀取产品展示名（同 workbuddyAdapter 回执口径）；兜底句逐字沿用 WBA L49「请稍后重试。」。
    texts: { notificationPrefix: "WorkBuddy", fallbackFailure: "请稍后重试。" },
  };
}
