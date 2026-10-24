/**
 * 设置页视图模型 / 能力表 / 文案表（P4f-1）：纯类型与声明式配置——零 UI、零 JSX、零 invoke、零 import
 * （含零 import features）。builder（P4f-2）产出 SettingsView；页面只读本文件与能力表，不判断 productId。
 * 文案逐字沿用旧实现（SettingsPanel 20 例 + WorkBuddyPages 设置 3 例 + 备份段 4 例锁定）；行号缩写
 * L=settings/TraeSettingsVariant.tsx、WB=WorkBuddyBackupSection.tsx、WBA=WorkBuddyAutoCheckinSettings.tsx、
 * CARD=shared/checkin/AutoCheckinSettingsCard.tsx。占位符统一 {name}，由 fillSettingsText 填充。
 */

// ===== 视图模型（全字段只读） =====

/** 色档：neutral=安静、ok=正常、warn=需处理、danger=失效。 */
export type SettingsTone = "neutral" | "ok" | "warn" | "danger";
/** 链行徽章（「含附属件」「完整快照」「含凭据」「清单缺失」）：title 非空即悬浮说明。 */
export interface SettingsBadgeView { readonly text: string; readonly tone: SettingsTone; readonly title: string | null }
/** 动作（分区/链行/弹层共用）：disabledReason 非空即禁用并给悬浮说明；busyLabel=执行中文案。 */
export interface SettingsActionView {
  readonly id: string; readonly label: string; readonly testId: string;
  readonly hint: string | null; readonly disabledReason: string | null; readonly busyLabel: string | null;
}
/** 今日自动批次台账（两产品字段同构；null = 今日尚未发起）。 */
export interface SettingsLedgerView {
  readonly running: boolean; readonly total: number; readonly completed: number; readonly failed: number; readonly skipped: number;
}
/** 自动签到区：总开关 + 每日时间 + 今日台账；参与清单仅 WB（participateAll 为批量入口）。 */
export interface AutoCheckinView {
  readonly testId: string; readonly eyebrow: string; readonly title: string; readonly badge: string; readonly badgeTone: SettingsTone;
  readonly enabled: boolean; readonly dailyTimeHhmm: string; readonly busy: boolean;
  readonly toggleTestId: string; readonly toggleTitle: string; readonly toggleStrong: string; readonly toggleSummary: string;
  readonly timeLabel: string; readonly timeTestId: string; readonly ledgerTestId: string; readonly ledgerText: string;
  readonly loadingNote: string | null; // 首读中（数据未到位）
  readonly participation: {
    readonly testId: string; readonly listTestId: string; readonly label: string; readonly defaultName: string;
    readonly rows: readonly { readonly profileId: string; readonly displayName: string; readonly participating: boolean }[];
    readonly participateAll: SettingsActionView | null; // WB 独有；TRAE 恒 null
  } | null;
}
/** 一份备份（链行；TRAE 无勾选/无删除，WB 可多选删除）。 */
export interface BackupChainRowView {
  readonly stampUnixSeconds: number; readonly stampLabel: string; readonly sizeLabel: string;
  readonly badges: readonly SettingsBadgeView[]; readonly selectTestId: string | null; readonly selected: boolean;
  readonly restore: SettingsActionView;
}
/** 备份区：保留块/凭据开关/位置变更/恢复步骤折叠区按能力出现（缺能力即 null）。 */
export interface BackupSectionView {
  readonly testId: string; readonly eyebrow: string; readonly title: string; readonly countBadge: string; readonly countTone: SettingsTone;
  readonly loadingNote: string | null; readonly backupDirLabel: string; readonly backupDir: string;
  readonly retention: {
    readonly testId: string; readonly label: string; readonly selectTestId: string; readonly enabled: boolean;
    readonly onLabel: string; readonly offLabel: string; readonly hintTestId: string; readonly hint: string;
  } | null;
  readonly rows: readonly BackupChainRowView[]; readonly emptyText: string;
  readonly credentialsToggle: { readonly testId: string; readonly title: string; readonly strong: string;
    readonly summary: string; readonly checked: boolean } | null;
  readonly create: SettingsActionView; readonly deleteSelected: SettingsActionView | null;
  readonly changeLocation: SettingsActionView | null;
  readonly restoreHint: { readonly testId: string; readonly summary: string; readonly text: string } | null;
  readonly loadError: { readonly title: string; readonly description: string; readonly retryLabel: string } | null;
}
/** 设置页视图模型：页面唯一渲染输入（无环境/插件/关于分区——两产品均无此能力）。 */
export interface SettingsView {
  readonly productId: string;
  readonly header: { readonly eyebrow: string; readonly title: string; readonly regionLabel: string; readonly pageTitleAttr: string };
  readonly autoCheckin: AutoCheckinView | null; readonly backup: BackupSectionView | null;
  readonly texts: SettingsTexts;
}
/** 页面级文案（操作结果一律经 notify 发出；前缀取产品展示名）。 */
export interface SettingsTexts { readonly notificationPrefix: string; readonly fallbackFailure: string }

// ===== 能力表（照 P4f-design §2 八项差异；A=缺失、B=表现不同、C=一致） =====

/** 备份七项能力分化（删除/保链/保留/位置/凭据/附属件/回执）——不统一成通用开关（§4 风险点 1–3）。 */
export interface BackupCapabilities {
  readonly deletion: boolean; // A 链上多选删除：仅 WB（TRAE delete_master_backups 前端零引用）
  readonly safetyChain: "restore-safety-backup" | "none"; // A 恢复前自动保链：仅 WB（safety_backup_stamp 必上屏）
  readonly retention: boolean; // A 保留开关（滚动单份）：仅 TRAE（WB 滚动在 create 内，回执 rolled_over）
  readonly location: boolean; // A 更改备份位置（迁移勾选）：仅 TRAE（WB 固定 storage_root/workbuddy-backups）
  readonly credentials: "toggle-default-off" | "always-included"; // B 凭据：TRAE 可选默认不勾 / WB 固定含（P19 决策 1）
  readonly attachments: "wal-badge" | "file-count"; // B 链行明细：TRAE 附属件徽章 / WB 文件数与压缩体积
  readonly receipt: "checkup-relaunch" | "rolled-over-safety"; // B 回执：TRAE 体检+重启 / WB 滚动份数+回退点
}
/** 设置页能力开关：页面与 builder 只读本表，禁止再写 productId 分支。 */
export interface SettingsCapabilities {
  readonly accountParticipation: "accounts-detail-only" | "settings-and-detail"; // B 逐账号参与开关位置（WB 多设置页清单）
  readonly participateAll: boolean; // A 批量「全部参与（N）」入口：仅 WB
  readonly autoCheckin: "inline" | "shared-card-with-participation"; // B 调度语义一致，WB 多参与清单
  readonly environmentSection: false; // C 两侧 absent（数据根/客户端启停归环境页）
  readonly backup: BackupCapabilities;
  readonly pluginsEntry: "library-tab" | "absent"; // C 两侧 absent（插件工作台在库页；WB 无插件体系）
  readonly aboutEntry: false; // C 两侧 absent（版本仅见环境页 WB 客户端卡与包 version）
  // B 读取门禁：P4f-5 合并页统一取 TRAE 口径（active 门禁 + 逐路代次守卫）。旧 WB 是「挂载即读」，
  // 本票有意改为可见才读（减少后台 IPC），行为变更记 docs/architecture/KNOWN-ISSUES.md K-11。
  readonly readGate: "active-gated-version-guarded";
  readonly pageRegion: "settings"; // C 页头契约（role=region aria-label="设置" + h1[data-page-title=settings]）
}
export const SETTINGS_CAPABILITIES: Record<string, SettingsCapabilities> = {
  trae_cn: {
    accountParticipation: "accounts-detail-only", participateAll: false, autoCheckin: "inline", environmentSection: false,
    backup: { deletion: false, safetyChain: "none", retention: true, location: true,
      credentials: "toggle-default-off", attachments: "wal-badge", receipt: "checkup-relaunch" },
    pluginsEntry: "library-tab", aboutEntry: false, readGate: "active-gated-version-guarded", pageRegion: "settings",
  },
  workbuddy: {
    accountParticipation: "settings-and-detail", participateAll: true, autoCheckin: "shared-card-with-participation",
    environmentSection: false,
    backup: { deletion: true, safetyChain: "restore-safety-backup", retention: false, location: false,
      credentials: "always-included", attachments: "file-count", receipt: "rolled-over-safety" },
    pluginsEntry: "absent", aboutEntry: false, readGate: "active-gated-version-guarded", pageRegion: "settings",
  },
};
/** 未登记产品回退最保守一档（无删除/无保链/无保留/无位置，不猜能力）。 */
export const DEFAULT_SETTINGS_CAPABILITIES: SettingsCapabilities = SETTINGS_CAPABILITIES.workbuddy;
/** 按产品取能力表（页面与 builder 的唯一产品差异入口）。 */
export const settingsCapabilitiesFor = (productId: string): SettingsCapabilities =>
  SETTINGS_CAPABILITIES[productId] ?? DEFAULT_SETTINGS_CAPABILITIES;

// ===== 文案表（逐字保留旧实现；行号见每组注释） =====

export const SETTINGS_TEXTS = {
  // 页头/页面级兜底：L261/264/265/255（WB 页头逐字相同）
  regionLabel: "设置", pageTitle: "设置", eyebrow: "偏好与安全", fallbackFailure: "自动签到设置保存失败。",
  // TRAE 自动签到内联三件：L272/273/276/285/294/295/299/315
  autoEyebrow: "应用行为", autoTitle: "自动签到", autoOn: "已开启", autoOff: "已关闭",
  autoToggleTitle: "到点后各账号在 0-15 分钟内随机错峰执行；迟于设定时间打开应用会自动补偿执行。",
  autoToggleStrong: "每日自动签到", autoToggleSummary: "开启后到点自动执行", autoTimeLabel: "触发时间", autoLoading: "正在读取自动签到设置…",
  // 今日台账（L532-539 autoLedgerLabel）：三句骨架 + 三个计数片段
  ledgerNone: "今日尚未发起", ledgerRunning: "今日执行中（{parts}）", ledgerDone: "今日已完成（{parts}）",
  ledgerSuccess: "成功 {completed}/{total}", ledgerFailed: "失败 {failed}", ledgerSkipped: "跳过 {skipped}", ledgerJoin: "，",
  // WB 自动签到（WBA L45/46/49/64/82/92；CARD L40/45/50/59/64/92/109/116）
  wbAutoToggleSummary: "开启后到点自动执行（断连自动续期兜底）",
  wbAutoToggleTitle: "到点后参与账号在 0-15 分钟内随机错峰自动执行。",
  wbAutoLedgerRunning: "今日执行中（完成 {completed}/{total}{failed}）", wbAutoLedgerDone: "今日已完成（成功 {completed}/{total}{failed}）",
  wbAutoLedgerFailed: "，失败 {failed}", wbParticipateAll: "全部参与", wbParticipateAllNone: "全部账号均已参与",
  wbParticipateAllHint: "把 {count} 个未参与账号全部置为参与",
  wbParticipationLabel: "参与自动签到", wbAccountFallback: "WorkBuddy 账号",
  // 备份区共用壳：L324/325/328（TRAE）· WB L107/108/112/118
  backupEyebrow: "数据安全", backupTitleTrae: "主库数据备份", backupTitleWb: "对话库备份", backupCount: "现有 {count} 份",
  backupDirLabel: "当前备份位置：", backupDirLabelWb: "备份目录：{dir}", backupLoadingWb: "正在读取备份链…",
  // TRAE 保留块：L335/342/343/348/349/443
  retentionLabel: "自动清理旧备份", retentionOn: "开启（只保留最新一份）", retentionOff: "关闭（旧备份不自动删除）",
  retentionHintOn: "同一时刻只保留最新一份备份；新备份生成后，更早的会自动删除", retentionHintOff: "当前未开启自动清理，备份不会被自动删除",
  retentionStepsOn: "超出保留份数的旧备份会自动清理（当前保留 {keep} 份）。", retentionStepsOff: "未开启自动清理时，备份不会被自动删除。",
  // TRAE 链行与凭据：L362/364/367/369/378/381/554/555
  backupWalTitle: "备份包含运行附属件，恢复时需一并复制回原位", backupWalBadge: "含附属件", backupSnapshotBadge: "完整快照",
  backupCredTitle: "这份备份同时保存了账号凭据（加密密文），恢复时把凭据子目录一并复制回签到材料目录", backupCredBadge: "含凭据",
  backupRestoreTitle: "用这份备份覆盖当前主库数据（需先关闭 TRAE，系统会自动关闭）", backupRestore: "还原",
  sizeMiB: "{value} MiB", sizeKiB: "{value} KiB",
  // TRAE 空态/凭据开关/动作：L389/393/401/402/414/428/433/439
  backupEmptyTrae: "尚无备份；切换账号时会自动生成，也可以现在手动创建一份。",
  credentialsTitle: "把账号凭据（加密密文）与账号清单一并备份到备份目录下的凭据子目录；仅同一 Windows 用户可恢复。",
  credentialsStrong: "包含账号凭据", credentialsSummary: "备份签到账号的加密凭据与清单（默认不勾）",
  backupCreate: "立即备份", backupBusy: "备份中…", backupChangeLocation: "更改备份位置", backupStepsSummary: "查看恢复步骤",
  // TRAE 恢复步骤折叠区：L441/445/447/448
  backupStepsHead: "备份保存在上方所示目录（.switch-bak- 前缀）。",
  backupStepsManual: "人工恢复：先关闭 TRAE，再把所选备份内的文件复制回该目录覆盖原位，然后重新启动。",
  backupStepsCredentials: "带「含凭据」的备份在凭据子目录（credentials- 前缀）中保存了账号加密凭据：恢复账号时把该子目录的内容复制回签到材料目录（checkin）即可。凭据子目录随对应主备份一并保留/清理；无对应主备份的凭据目录可手动清理。",
  // TRAE 更改位置弹层：L455/457/458/464/467/478/479/490/499
  locationDialogTitle: "更改备份位置", locationDialogHint: "新备份将写入新目录；已有备份默认复制过去（原位置保留，可自行清理）。",
  locationPlaceholder: "输入新备份目录的完整路径，如 D:\\TraeBackups", locationAria: "新备份目录",
  locationMigrateStrong: "同时迁移现有备份", locationMigrateSummary: "复制到新位置并逐文件校验，原位置保留",
  cancel: "取消", save: "保存", saving: "保存中…",
  // TRAE 还原确认弹层：L510-513/515/516
  restoreDialogTitle: "从备份还原主库", restoreLine1: "将用所选备份覆盖当前主库数据：备份之后的对话、账号变化会丢失。",
  restoreLine2: "系统会先自动关闭 TRAE，校验备份完整后再覆盖；校验不通过则原数据不受影响。",
  restoreLine3: "还原完成后会自动做一次数据库体检并重新启动 TRAE。", restoreConfirm: "确认还原", restoring: "还原中…",
  // WB 链行与动作：WB L141/150/152/174/179/191/193/203/101
  backupDeleteSelected: "删除所选（{count}）", backupManifestMissing: "清单缺失", backupManifestMissingTitle: "备份清单不可读（半成品目录），如实呈现",
  backupRestoreWb: "恢复", backupRestoreWbTitle: "从这份备份恢复（覆盖前自动保链备份当前状态）",
  backupEmptyWb: "还没有备份。彻底删除会话前会自动创建，也可以点击「立即备份」。",
  wbFileCount: "{count} 个文件", wbFileCountUnknown: "文件数未知",
  wbRowMeta: "{files} · 源 {source} · 压缩 {compressed}", wbBytesUnknown: "—",
  // WB 三弹层：L212-219/230-237/249-255
  wbCreateTitle: "创建对话库备份",
  wbCreateLine1: "将备份 workbuddy.db、projects 正文目录与登录凭据材料（P19 决策 1 全纳入范围）。",
  wbCreateLine2: "执行时 WorkBuddy 客户端会完全关闭并在备份完成后自动重启。",
  wbCreateLine3: "滚动单份：新备份落位后同目录只保留最新一份。",
  wbRestoreTitle: "从备份恢复对话库", wbRestoreLine1: "将以 {stamp} 的备份覆盖当前对话库（workbuddy.db 与正文目录）。",
  wbRestoreLine2: "恢复前会自动把当前状态备份一份（保链不清除），失败可用它回退——恢复完成后会提示回退点。",
  wbRestoreLine3: "执行时 WorkBuddy 客户端会完全关闭并在恢复完成后自动重启。",
  wbRestoreConfirm: "确认恢复", wbRestoring: "恢复中…", wbDeleteTitle: "删除备份",
  wbDeleteLine1: "将删除所选 {count} 份备份，不可恢复。",
  wbDeleteLine2: "建议至少保留一份备份；恢复前的保链备份不受本操作影响（按所选清单执行）。",
  wbDeleteConfirm: "确认删除", wbDeleting: "删除中…",
  // WB 读取失败态：WB L119-130
  wbLoadErrorTitle: "无法读取备份链", wbLoadErrorFallback: "备份链暂时不可读取，请稍后重试。", retry: "重新读取",
} as const;

/** 文案占位符填充（builder 与弹层共用）：缺键保留原样，不吞占位符。 */
export const fillSettingsText = (template: string, values: Readonly<Record<string, string | number>>): string =>
  template.replace(/\{(\w+)\}/g, (match, key: string) => (key in values ? String(values[key]) : match));

/** 文案键类型（builder 取文案时获得字面量补全）。 */
export type SettingsTextKey = keyof typeof SETTINGS_TEXTS;

/** 今日台账文案（TRAE 口径 L532-539）：null=今日尚未发起；有台账按 running 分执行中/已完成。 */
export function settingsLedgerText(ledger: SettingsLedgerView | null): string {
  if (ledger === null) return SETTINGS_TEXTS.ledgerNone;
  const parts = [fillSettingsText(SETTINGS_TEXTS.ledgerSuccess, { completed: ledger.completed, total: ledger.total })];
  if (ledger.failed > 0) parts.push(fillSettingsText(SETTINGS_TEXTS.ledgerFailed, { failed: ledger.failed }));
  if (ledger.skipped > 0) parts.push(fillSettingsText(SETTINGS_TEXTS.ledgerSkipped, { skipped: ledger.skipped }));
  const frame = ledger.running ? SETTINGS_TEXTS.ledgerRunning : SETTINGS_TEXTS.ledgerDone;
  return fillSettingsText(frame, { parts: parts.join(SETTINGS_TEXTS.ledgerJoin) });
}
