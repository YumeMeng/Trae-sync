import { Activity, DatabaseBackup } from "lucide-react";
import { Badge, type BadgeTone } from "../../../ui/Badge";
import {
  LIBRARY_TEXTS, fillLibraryText,
  type BackupCompareView, type CheckupView, type LibraryActionView, type LibraryInfoView, type LibraryTone,
} from "../config";
import { LibraryActionButton } from "./LibraryShell";

/**
 * 库信息 tab（P4e-7c，纯展示）：五项信息 + 计数条 + 备份对比 + 体检卡（含归并候选组）。
 * 备份对比承接旧 MasterVerificationPanel 的 backupOnly 形态（V=src/components/MasterVerificationPanel.tsx）：
 * 五态（ready 有/无丢失、no_backups、backup_missing、read_failed、no_master_data）的徽章词与摘要句一律由视图给，
 * 本层只按视图渲染（不判断状态、不判断产品）。文案取自视图与 LIBRARY_TEXTS；零 state / 零 invoke，交互只回调。
 */
/** 视图模型未建模的旧页锚点（V180 / V185 / V208 / V220 / V246 / V258；既有约定，不发明）。 */
const BLOCK_TEST_ID = "verification-backup-block", BADGE_TEST_ID = "verification-backup-badge";
const BASE_TEST_ID = "verification-base-select", SUMMARY_TEST_ID = "verification-backup-summary";
const MISSING_TEST_ID = "verification-missing-list", HINT_TEST_ID = "verification-restore-hint";
const COUNTS_TEST_ID = "master-detail-counts";
/** 徽章色档：LibraryTone → Badge 四态（ok=正常态一律中性，UI-DIRECTION §4）。 */
const BADGE_TONE: Record<LibraryTone, BadgeTone> = { neutral: "neutral", ok: "neutral", warn: "warn", danger: "danger", accent: "accent" };

/** 备份对比区块（V169-266）：工具行（重新校验）+ 区块头（标题 + 状态徽章）+ 基准切换 + 摘要 + 丢失候选 + 恢复指引。 */
function BackupCompare({ view, busy, onAction, onBaseChange }: {
  view: BackupCompareView; busy: boolean; onAction: (actionId: string) => void; onBaseChange: (stamp: number | null) => void;
}) {
  return (
    <div className="verification-panel" data-testid={view.testId}>
      <div className="verification-panel__toolbar">
        <LibraryActionButton action={view.refresh} busy={busy} onClick={() => onAction(view.refresh.id)} />
      </div>
      {/* 读取失败行（V96）：不遮挡已取回的报告，也不清已取回数据。 */}
      {view.errorNote !== null && <p className="workbench__error" role="alert">{view.errorNote}</p>}
      {/* 报告未到达（首读中 / 读取失败且无历史报告）：只给工具行与错误行（V98 的报告守卫）。 */}
      {!view.pending && (
      <section className="verification-block" data-testid={BLOCK_TEST_ID}>
        <div className="verification-block__heading">
          <h4>{view.title}</h4>
          <Badge tone={BADGE_TONE[view.badge.tone]} hint={view.badge.title ?? undefined} testId={BADGE_TEST_ID}>{view.badge.text}</Badge>
        </div>
        {/* 基准选择器：仅链上有可选备份点才出（旧 V198 的 backup_stamps.length > 0 守卫）。 */}
        {view.baseOptions.length > 0 && (
          <label className="verification-block__base">
            <span>{view.baseLabel}</span>
            <select value={view.selectedStamp ?? ""} disabled={busy || view.baseOptions.length <= 1}
              onChange={(event) => onBaseChange(event.target.value === "" ? null : Number(event.target.value))}
              data-testid={BASE_TEST_ID}>
              {view.baseOptions.map((option) => <option key={option.stamp} value={option.stamp}>{option.label}</option>)}
            </select>
          </label>
        )}
        <p className="verification-block__summary" data-testid={SUMMARY_TEST_ID}>{view.summary}</p>
        {view.missing.length > 0 && (
          <div className="verification-missing" data-testid={MISSING_TEST_ID}>
            <ul className="verification-issues">
              {view.missing.map((session, index) => (
                <li key={index} className="verification-issues__item">
                  <DatabaseBackup size={14} aria-hidden="true" />
                  <span>{fillLibraryText(LIBRARY_TEXTS.verifyMissingItem, {
                    title: session.title || LIBRARY_TEXTS.verifyUnnamedSession, messages: session.messageCount })}</span>
                </li>
              ))}
            </ul>
            {view.restoreHint !== null && <p className="verification-block__hint" data-testid={HINT_TEST_ID}>{view.restoreHint}</p>}
          </div>
        )}
      </section>
      )}
    </div>
  );
}

/** 归并候选入口的缺省派生（旧 WB447-463）：canonical 在前，锚点取组长度（canonical + variants）。 */
const mergesOf = (view: CheckupView): readonly LibraryActionView[] => view.candidates.map((group) => ({
  id: `merge-${group.canonical}`, label: fillLibraryText(LIBRARY_TEXTS.wbMergeEntry, { cwd: group.canonical }), kind: "ghost",
  testId: `workbuddy-library-merge-${group.variants.length + 1}`, disabledReason: null, busyLabel: null,
  hint: fillLibraryText(LIBRARY_TEXTS.wbMergeHint, { count: group.variants.length, cwd: group.canonical }),
}));

/** 体检卡（WB421-481）：五项 + 归并候选组 + 立即体检；报告未读到只给 idleNote（旧页 `checkup ? … : idleNote`）。 */
function CheckupCard({ view, merges, busy, onAction }: {
  view: CheckupView; merges: readonly LibraryActionView[]; busy: boolean; onAction: (actionId: string) => void;
}) {
  const running = busy || view.action.disabledReason !== null;
  return (
    <div className="workbuddy-card" data-testid={view.testId}>
      <div className="workbuddy-card__title">{view.title}</div>
      {view.items.length > 0 ? (
        <>
          <ul className="account-center__checkin-results">
            {view.items.map((item) => (
              <li key={item.id} title={item.hint ?? undefined}><strong>{item.label}</strong><span>{item.value}</span></li>
            ))}
          </ul>
          {merges.length > 0 && (
            <div className="workbuddy-card__actions">
              {merges.map((merge) => (
                <button key={merge.id} className="btn" type="button" onClick={() => onAction(merge.id)}
                  disabled={busy || merge.disabledReason !== null}
                  title={merge.disabledReason ?? merge.hint ?? undefined} data-testid={merge.testId}>
                  <Activity size={14} aria-hidden="true" />{merge.label}
                </button>
              ))}
            </div>
          )}
        </>
      ) : <p className="account-item__meta">{view.idleNote}</p>}
      <div className="workbuddy-card__actions">
        <button className="btn" type="button" onClick={() => onAction(view.action.id)} disabled={running} data-testid={view.action.testId}>
          <Activity size={15} className={running ? "icon-spin" : undefined} aria-hidden="true" />{view.action.label}
        </button>
      </div>
    </div>
  );
}

export interface LibraryInfoTabProps {
  /** 库信息视图（builder 产出）：五项 + 计数条 + 备份对比（backup=null 即不渲染该区块）。 */
  readonly view: LibraryInfoView;
  /** 体检卡视图（WB 专属；null/缺省即不渲染——TRAE 库无体检）。 */
  readonly checkup?: CheckupView | null;
  /** 归并入口（WbCheckupView.mergeEntries；缺省按 candidates 派生旧锚与文案）。 */
  readonly mergeEntries?: readonly LibraryActionView[];
  /** 执行中的动作 id（备份对比刷新 / 体检 / 归并；缺省=无动作执行中）。 */
  readonly busy?: readonly string[];
  readonly onAction: (actionId: string) => void;
  /** 对比基准切换出口（stamp=null 即最新备份）。 */
  readonly onBackupBase: (stamp: number | null) => void;
}

/** 库信息 tab：信息卡（五项 + 计数条）→ 备份对比 → 体检卡（三块同层，体检卡不套进信息卡里）。 */
export function LibraryInfoTab({ view, checkup = null, mergeEntries, busy = [], onAction, onBackupBase }: LibraryInfoTabProps) {
  return (
    <>
      <div className="master-detail__info" data-testid={view.testId}>
        <div className="master-detail__info-grid">
          {/* value=null 即不渲染（视图契约）；带 titleAttr 的行是路径行（旧 V277-291 的宽行 + 等宽路径）。 */}
          {view.items.filter((item) => item.value !== null).map((item) => (
            <div key={item.id} className={`master-info-item${item.titleAttr !== null ? " master-info-item--wide" : ""}`}>
              <span className="master-info-item__label">{item.label}</span>
              {item.titleAttr !== null
                ? <code className="master-info-item__path" data-testid={item.testId} title={item.titleAttr}>{item.value}</code>
                : <span className="master-info-item__value" data-testid={item.testId}>{item.value}</span>}
            </div>
          ))}
        </div>
        {view.counts.length > 0 && (
          <div className="master-detail__counts" data-testid={COUNTS_TEST_ID}>
            {view.counts.map((count) => (
              <div key={count.id} className="master-count" title={count.hint ?? undefined}><b>{count.value}</b><span>{count.label}</span></div>
            ))}
          </div>
        )}
      </div>
      {view.backup !== null && (
        <BackupCompare view={view.backup} busy={busy.includes(view.backup.refresh.id)} onAction={onAction} onBaseChange={onBackupBase} />
      )}
      {checkup !== null && (
        <CheckupCard view={checkup} merges={mergeEntries ?? mergesOf(checkup)} busy={busy.includes(checkup.action.id)} onAction={onAction} />
      )}
    </>
  );
}
