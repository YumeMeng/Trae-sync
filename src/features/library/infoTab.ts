/**
 * 库信息 tab 的备份对比视图 builder（P4e-8b-3）：get_master_verification 报告 → BackupCompareView。
 * 旧实现逐条对照 src/components/MasterVerificationPanel.tsx（下称 V）的 backupOnly 形态：五态徽章词（V188-195）、
 * 摘要句（V220-243）、基准选择器（V198-217）、丢失候选（V245-262）与读取错误行（V96）。
 * 纯映射：零 React / 零 JSX / 零 invoke / 零命令；状态判定与文案全在本层，展示层只按字段渲染。
 */
import type { BackupComparisonDto, MasterVerificationDto } from "../../types/masterLibrary";
import { LIBRARY_TEXTS, fillLibraryText, type BackupCompareView, type LibraryActionView, type LibraryBadgeView, type LibraryTone } from "./config";

/** 页面持有的校验态（进 tab 首读 / 基准切换 / 重新校验；读取时机归 useLibraryVerification）。 */
export interface VerificationState {
  /** 报告（null = 尚未读到：进 tab 首读中，或读取失败且无历史报告）。 */
  readonly report: MasterVerificationDto | null;
  /** 当前对比基准（null = 最新备份；后端回执会回显实际使用的基准）。 */
  readonly stamp: number | null;
  readonly loading: boolean;
  /** 读取失败原因（V96 页内错误行；不遮挡已取回的报告）。 */
  readonly error: string | null;
}
/** 徽章词与色档（V186-195）：正常=中性（UI-DIRECTION §4 不设成功绿）、丢失候选=琥珀、不可用/失败=灰或红。 */
const badgeOf = (backup: BackupComparisonDto): LibraryBadgeView => {
  const text = backup.status === "ready"
    ? (backup.missing.length > 0 ? fillLibraryText(LIBRARY_TEXTS.verifyBadgeMissing, { count: backup.missing.length }) : LIBRARY_TEXTS.verifyBadgeNoLoss)
    : backup.status === "no_backups" ? LIBRARY_TEXTS.verifyBadgeNoBackups
    : backup.status === "backup_missing" ? LIBRARY_TEXTS.verifyBadgeUnavailable
    : backup.status === "read_failed" ? LIBRARY_TEXTS.verifyBadgeReadFailed : LIBRARY_TEXTS.verifyBadgeNoData;
  const tone: LibraryTone = backup.status === "ready" ? (backup.missing.length > 0 ? "warn" : "neutral")
    : backup.status === "read_failed" ? "danger" : "neutral";
  return { text, tone, title: null };
};
/** 摘要句（V219-243）：ready 给一致/新增/丢失三数并按需补「接力换新会话」；其余四态各一句。 */
const summaryOf = (backup: BackupComparisonDto): string => {
  if (backup.status !== "ready") {
    return backup.status === "no_backups" ? LIBRARY_TEXTS.verifyNoBackups
      : backup.status === "backup_missing" ? LIBRARY_TEXTS.verifyBackupMissing
      : backup.status === "read_failed" ? LIBRARY_TEXTS.verifyReadFailed : LIBRARY_TEXTS.verifyNoData;
  }
  const base = backup.missing.length === 0
    ? fillLibraryText(LIBRARY_TEXTS.verifySummaryOk, { common: backup.common_count, added: backup.added_count })
    : fillLibraryText(LIBRARY_TEXTS.verifySummaryMissing,
      { common: backup.common_count, added: backup.added_count, missing: backup.missing.length });
  return backup.relayed_away_count > 0 ? base + fillLibraryText(LIBRARY_TEXTS.verifyRelayedAway, { count: backup.relayed_away_count }) : base;
};
/** 基准点短文案（V289-296）：日期 + 时分粒度足够辨识；手工格式化，不随宿主 locale 漂移。 */
export function formatStamp(unixSeconds: number): string {
  const date = new Date(unixSeconds * 1000), pad = (value: number) => String(value).padStart(2, "0");
  return `${date.getMonth() + 1}月${date.getDate()}日 ${pad(date.getHours())}:${pad(date.getMinutes())}`;
}
/** 重新校验动作（V84-93）：执行中给「校验中…」并禁用（展示层按 busy 位驱动）。 */
const refreshAction = (loading: boolean): LibraryActionView => ({
  id: "verification-refresh", label: LIBRARY_TEXTS.verifyRefresh, kind: "ghost", testId: "master-verification-refresh",
  hint: null, disabledReason: loading ? LIBRARY_TEXTS.verifyRefreshing : null, busyLabel: LIBRARY_TEXTS.verifyRefreshing,
});
/** 校验态 → 备份对比区块（恒返回：工具行与错误行在报告未到前也要渲染，旧 V82-106 同口径）。 */
export function buildBackupCompareView(state: VerificationState): BackupCompareView {
  const backup = state.report?.backup ?? null;
  // 基准选项（V198-217）：只有 ready 且有链上备份点才出；首项标注「最新备份」。
  const stamps = backup !== null && backup.status === "ready" ? backup.backup_stamps : [];
  return {
    testId: "master-verification", title: LIBRARY_TEXTS.verifyBackupTitle, baseLabel: LIBRARY_TEXTS.verifyBaseLabel,
    baseOptions: stamps.map((stamp, index) => ({ stamp,
      label: index === 0 ? fillLibraryText(LIBRARY_TEXTS.verifyBaseLatest, { stamp: formatStamp(stamp) }) : formatStamp(stamp) })),
    selectedStamp: backup?.backup_stamp ?? state.stamp,
    badge: backup === null ? { text: LIBRARY_TEXTS.verifyBadgeNoData, tone: "neutral", title: null } : badgeOf(backup),
    summary: backup === null ? "" : summaryOf(backup),
    missing: (backup?.missing ?? []).map((session) => ({ title: session.title?.trim() || LIBRARY_TEXTS.verifyUnnamedSession, messageCount: session.message_count })),
    restoreHint: backup !== null && backup.missing.length > 0 ? LIBRARY_TEXTS.verifyRestoreHint : null,
    refresh: refreshAction(state.loading), pending: state.report === null, errorNote: state.error,
  };
}
