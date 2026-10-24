import type { ReactNode } from "react";
import { DatabaseBackup, FolderSymlink, RefreshCw, Trash2 } from "lucide-react";
import { EmptyState } from "../../../components/shared/EmptyState";
import { Badge, type BadgeTone } from "../../../ui/Badge";
import type { BackupChainRowView, BackupSectionView, SettingsActionView, SettingsTone } from "../config";

/**
 * 备份区（P4f-4a，纯展示）：分区壳 + 备份链 + 空态/读取失败态 + 保留块 + 凭据开关 + 动作行 + 恢复步骤。
 * 旧实现对照 TraeSettingsVariant.tsx L318-451（TRAE：整区随链可插拔消失）与 WorkBuddyBackupSection.tsx
 * L103-207（WB：壳恒在，读取中/失败/空态三档）。七项能力分化（删除/保链/保留/位置/凭据/附属件/回执）
 * 全部由视图模型表达，本层不判断产品、不读数据、不发命令：view 为 null 即整区不渲染（TRAE 口径）。
 * WB 的删除入口与链上多选只渲染（锚 workbuddy-backup-delete / -select-{stamp} 逐字保留），
 * 确认弹层与动作编排归 P4f-4b；本层不弹窗、不发命令。
 */

/** 色档换算：SettingsTone 的 ok 落到徽章 accent（Badge 无「成功绿」，正常态用强调色）。 */
const badgeTone = (tone: SettingsTone): BadgeTone => (tone === "ok" ? "accent" : tone);
/**
 * 行锚（旧 L356）：TRAE 每行容器是 master-backup-entry；WB 旧行只有裸 li（WB L158），无行锚——不新造。
 * 视图模型无产品字段，故用「是否可勾选」作判据：可勾选（selectTestId 非空）即 WB 行。
 */
const rowAnchor = (row: BackupChainRowView): string | undefined =>
  (row.selectTestId === null ? "master-backup-entry" : undefined);

/** 动作按钮：disabledReason 非空即禁用并给悬浮说明；busy 时换执行中文案（缺省沿用原词）。 */
function ActionButton({ action, busy, disabled, disabledReason, icon, danger = false, onClick }: {
  readonly action: SettingsActionView; readonly busy: boolean; readonly disabled: boolean;
  readonly disabledReason: string | null; readonly icon: ReactNode; readonly danger?: boolean;
  readonly onClick: () => void;
}) {
  return (
    <button className={danger ? "btn btn--danger" : "btn"} type="button" onClick={onClick}
      disabled={disabled || action.disabledReason !== null}
      title={action.disabledReason ?? disabledReason ?? action.hint ?? undefined}
      aria-busy={busy || undefined} data-testid={action.testId}>
      {busy ? <RefreshCw size={15} className="icon-spin" aria-hidden="true" /> : icon}
      {busy ? action.busyLabel ?? action.label : action.label}
    </button>
  );
}

/** 单行备份（旧 L355-383 / WB L157-198）：勾选（WB 独有）+ 时间 + 体积/明细 + 徽章 + 恢复动作。 */
function ChainRow({ row, busy, disabled, disabledReason, onRestore, onToggleSelect }: {
  readonly row: BackupChainRowView; readonly busy: boolean; readonly disabled: boolean;
  readonly disabledReason: string | null; readonly onRestore: (stamp: number) => void;
  readonly onToggleSelect: (stamp: number, selected: boolean) => void;
}) {
  return (
    <li className="settings-backup__item" data-testid={rowAnchor(row)}>
      {/* 勾选框（WB 多选删除，WB L161-169）：视图不给 selectTestId 即 TRAE 行，无勾选。 */}
      {row.selectTestId !== null && (
        <label className="account-center__profile-select">
          <input type="checkbox" checked={row.selected} data-testid={row.selectTestId}
            disabled={disabled || row.restore.disabledReason !== null}
            onChange={(event) => onToggleSelect(row.stampUnixSeconds, event.target.checked)} />
        </label>
      )}
      <DatabaseBackup size={14} aria-hidden="true" />
      <span className="settings-backup__time">{row.stampLabel}</span>
      <span className="settings-backup__size">{row.sizeLabel}</span>
      {row.badges.map((badge) => (
        <Badge key={badge.text} tone={badgeTone(badge.tone)} hint={badge.title ?? undefined}>{badge.text}</Badge>
      ))}
      <ActionButton action={row.restore} busy={busy} disabled={disabled} disabledReason={disabledReason}
        icon={<RefreshCw size={14} aria-hidden="true" />} onClick={() => onRestore(row.stampUnixSeconds)} />
    </li>
  );
}

export interface BackupSectionProps {
  /** 备份区视图；null = 整区不渲染（TRAE 链未读到，旧 L320 可插拔降级）。 */
  readonly view: BackupSectionView | null;
  /** 执行中的动作 id 集合（页面按忙碌位注入，同 EnvCard 口径）：创建换「备份中…」，其余只禁用。 */
  readonly busy?: readonly string[];
  /** 外部禁用（页面门禁/只读）：与视图内的 disabledReason 合并，原因进悬浮说明。 */
  readonly disabled?: boolean;
  readonly disabledReason?: string | null;
  onCreate: () => void;
  onRestore: (stampUnixSeconds: number) => void;
  /** 链上多选（WB 独有；TRAE 行无勾选框，视图不给 selectTestId）。 */
  onToggleSelect: (stampUnixSeconds: number, selected: boolean) => void;
  /** 删除所选（WB 独有；TRAE 视图恒 null）。 */
  onDeleteSelected: () => void;
  onChangeLocation: () => void;
  onToggleCredentials: (checked: boolean) => void;
  onRetentionChange: (enabled: boolean) => void;
  /** 读取失败态的重试（WB L126；TRAE 无该态，视图不给即不渲染）。 */
  onRetry: () => void;
}

export function BackupSection({
  view, busy = [], disabled = false, disabledReason = null,
  onCreate, onRestore, onToggleSelect, onDeleteSelected, onChangeLocation, onToggleCredentials, onRetentionChange, onRetry,
}: BackupSectionProps) {
  if (view === null) return null;
  const isBusy = (id: string): boolean => busy.includes(id);
  /**
   * 禁用判据全部来自数据，不设「整体锁定」：①外部 disabled（页面门禁/只读）；②页面注入的 busy 动作 id；
   * ③动作自身的 disabledReason（builder 已把 busy 折算进去，如 WB 的「备份操作进行中」）。
   * 视图模型没有保留/凭据各自的忙碌位（P4f-2 报告记的诚实缺口）：保留块走 ①②，凭据开关再叠加创建原因。
   */
  const locked = (testId: string, reason: string | null = null): boolean =>
    disabled || reason !== null || isBusy(testId);
  /** 目录行：WB 的标签已含目录（单行口径），TRAE 是前缀 + code（L432-437）——按数据判断，不按产品。 */
  const inlineDir = view.backupDir !== "" && view.backupDirLabel.includes(view.backupDir);
  // 链可读：WB 未读到时份数徽章为空串（不冒充 0 份），旧实现此时正文为 null（WB L207），只留标题；
  // TRAE 份数徽章恒有值（链 null 时视图整体为 null，已在上方早返回）。
  const chainReadable = view.countBadge !== "";
  // 链容器锚：旧 WB 的 ul 带 workbuddy-backup-chain（WB L156），旧 TRAE 的 ul 无容器锚（L354）——
  // 按「行是否可勾选」派生（视图模型无产品字段，可勾选即 WB 删除能力）。空链两侧都不给容器锚。
  const chainTestId = view.rows.some((row) => row.selectTestId !== null) ? "workbuddy-backup-chain" : undefined;
  return (
    <div className="settings-backup" data-testid={view.testId}>
      <div className="settings-panel__section-heading">
        <div>
          <span className="workbench__eyebrow">{view.eyebrow}</span>
          <h3>{view.title}</h3>
        </div>
        {/* 份数徽章：空串即不占位（WB 链未读到，不冒充 0 份）。 */}
        {view.countBadge !== "" && <Badge tone={badgeTone(view.countTone)}>{view.countBadge}</Badge>}
      </div>
      {view.loadingNote !== null ? (
        <p className="account-center__meta">{view.loadingNote}</p>
      ) : view.loadError !== null ? (
        <EmptyState role="alert" className="settings-backup__empty" title={view.loadError.title}
          description={view.loadError.description}
          action={<button className="btn" type="button" onClick={onRetry} disabled={disabled}
            title={disabledReason ?? undefined}>{view.loadError.retryLabel}</button>} />
      ) : !chainReadable ? null : (
        <>
          {/* 保留块（TRAE 独有，旧 L332-352）：能力与数据齐备时视图才给，本层不补默认。 */}
          {view.retention !== null && (
            <div className="account-center__key-grid" data-testid={view.retention.testId}>
              <label className="account-center__target-field">
                <span>{view.retention.label}</span>
                <select value={view.retention.enabled ? "on" : "off"} data-testid={view.retention.selectTestId}
                  disabled={locked(view.retention.selectTestId)}
                  onChange={(event) => onRetentionChange(event.target.value === "on")}>
                  <option value="on">{view.retention.onLabel}</option>
                  <option value="off">{view.retention.offLabel}</option>
                </select>
              </label>
              <span className="account-center__meta" data-testid={view.retention.hintTestId}>{view.retention.hint}</span>
            </div>
          )}
          {view.rows.length > 0 ? (
            <ul className="settings-backup__list" data-testid={chainTestId}>
              {view.rows.map((row) => (
                <ChainRow key={row.stampUnixSeconds} row={row} busy={isBusy(row.restore.id)} disabled={disabled}
                  disabledReason={disabledReason} onRestore={onRestore} onToggleSelect={onToggleSelect} />
              ))}
            </ul>
          ) : (
            <EmptyState className="settings-backup__empty" description={view.emptyText} />
          )}
          {/* 凭据开关（TRAE 独有，默认不勾，旧 L392-404）：WB 固定含凭据，视图恒 null。 */}
          {view.credentialsToggle !== null && (
            <label className="settings-panel__toggle-row" data-testid={view.credentialsToggle.testId}
              title={view.credentialsToggle.title}>
              <input type="checkbox" checked={view.credentialsToggle.checked}
                disabled={locked(view.credentialsToggle.testId, view.create.disabledReason)}
                onChange={(event) => onToggleCredentials(event.target.checked)} />
              <span className="account-center__profile-copy">
                <strong>{view.credentialsToggle.strong}</strong>
                <span>{view.credentialsToggle.summary}</span>
              </span>
            </label>
          )}
          <div className="settings-backup__actions">
            <ActionButton action={view.create} busy={isBusy(view.create.id)} disabled={disabled}
              disabledReason={disabledReason} icon={<DatabaseBackup size={15} aria-hidden="true" />} onClick={onCreate} />
            {/* 删除所选（WB 独有，WB L143-150）：TRAE 视图恒 null（不给删除入口）。 */}
            {view.deleteSelected !== null && (
              <ActionButton action={view.deleteSelected} busy={false} disabled={disabled} danger
                disabledReason={disabledReason} icon={<Trash2 size={15} aria-hidden="true" />}
                onClick={onDeleteSelected} />
            )}
            {/* 位置变更（TRAE 独有，旧 L416-429）：WB 固定备份目录，视图恒 null。 */}
            {view.changeLocation !== null && (
              <ActionButton action={view.changeLocation} busy={isBusy(view.changeLocation.id)} disabled={disabled}
                disabledReason={disabledReason} icon={<FolderSymlink size={15} aria-hidden="true" />}
                onClick={onChangeLocation} />
            )}
          </div>
          {/* 当前备份位置（旧锚 backup-location-current，L432）：完整路径只进悬浮，主视野单行。 */}
          <p className="account-center__meta" data-testid="backup-location-current">
            {inlineDir ? view.backupDirLabel : (
              <>
                {view.backupDirLabel}
                <code className="settings-panel__backup-dir" title={view.backupDir}>{view.backupDir}</code>
              </>
            )}
          </p>
          {/* 恢复步骤折叠区（TRAE 独有，旧 L438-450）：摘要与正文都由视图给。 */}
          {view.restoreHint !== null && (
            <details className="settings-panel__fold settings-backup__location" data-testid="master-backup-location-details">
              <summary>{view.restoreHint.summary}</summary>
              <p className="settings-backup__hint" data-testid={view.restoreHint.testId}>{view.restoreHint.text}</p>
            </details>
          )}
        </>
      )}
    </div>
  );
}
