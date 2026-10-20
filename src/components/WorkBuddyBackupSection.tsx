import { useCallback, useEffect, useState } from "react";
import { DatabaseBackup, Trash2 } from "lucide-react";
import type { WorkBuddyBackupChainDto } from "../types/workbuddyBackup";
import {
  createWorkBuddyBackup,
  deleteWorkBuddyBackups,
  fetchWorkBuddyBackupChain,
  restoreWorkBuddyBackup,
} from "../platform/workbuddyAdapter";
import { ConfirmDialog } from "./shared/ConfirmDialog";
import { EmptyState } from "./shared/EmptyState";
import { notify } from "../platform/notify";
import { safeUiErrorMessage } from "../utils/safeUiError";

/**
 * WB 备份链管理面（P19-B8，消费 list/create/restore/delete_workbuddy_backups）：
 * 链列表（最新在前，半成品条目如实标注）+ 手动备份（单次确认，客户端关闭重启）
 * + 恢复（单次确认，恢复前自动保链备份——safety 回退点必须展示）+ 删除（单次确认）。
 * 滚动单份语义：新份落位后清旧（create 回执 rolled_over 如实提示）。
 */
export function WorkBuddyBackupSection() {
  const [chain, setChain] = useState<WorkBuddyBackupChainDto | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const [busy, setBusy] = useState(false);
  const [selectedStamps, setSelectedStamps] = useState<readonly number[]>([]);
  const [createConfirmOpen, setCreateConfirmOpen] = useState(false);
  const [restoreTarget, setRestoreTarget] = useState<number | null>(null);
  const [deleteConfirmOpen, setDeleteConfirmOpen] = useState(false);

  const refresh = useCallback(async () => {
    setLoading(true);
    try {
      setChain(await fetchWorkBuddyBackupChain());
      setLoadError(null);
    } catch (reason: unknown) {
      setLoadError(safeUiErrorMessage(reason, "备份链暂时不可读取，请稍后重试。"));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  const toggleSelect = useCallback((stamp: number) => {
    setSelectedStamps((current) => current.includes(stamp)
      ? current.filter((item) => item !== stamp)
      : [...current, stamp]);
  }, []);

  const runCreate = useCallback(async () => {
    setCreateConfirmOpen(false);
    setBusy(true);
    try {
      const receipt = await createWorkBuddyBackup();
      notify.success(
        `WorkBuddy：备份已创建${receipt.rolled_over > 0 ? `（滚动清理 ${receipt.rolled_over} 份旧备份）` : ""}。`,
      );
      await refresh();
    } catch (reason: unknown) {
      notify.error(`WorkBuddy：备份未完成——${safeUiErrorMessage(reason, "请稍后重试。")}`);
    } finally {
      setBusy(false);
    }
  }, [refresh]);

  const runRestore = useCallback(async (stamp: number) => {
    setRestoreTarget(null);
    setBusy(true);
    try {
      const receipt = await restoreWorkBuddyBackup(stamp);
      const safetyTime = new Date(receipt.safety_backup_stamp * 1000).toLocaleString();
      notify.success(`WorkBuddy：已恢复到 ${receipt.restored_stamp}（恢复前状态已存为 ${safetyTime} 的备份，可回退）。`);
      await refresh();
    } catch (reason: unknown) {
      notify.error(`WorkBuddy：恢复未完成——${safeUiErrorMessage(reason, "数据未被改动，请稍后重试。")}`);
    } finally {
      setBusy(false);
    }
  }, [refresh]);

  const runDelete = useCallback(async () => {
    setDeleteConfirmOpen(false);
    setBusy(true);
    try {
      const receipt = await deleteWorkBuddyBackups(selectedStamps);
      notify.success(`WorkBuddy：已删除 ${receipt.deleted_stamps.length} 份备份（剩余 ${receipt.remaining} 份）。`);
      setSelectedStamps([]);
      await refresh();
    } catch (reason: unknown) {
      notify.error(`WorkBuddy：备份删除未完成——${safeUiErrorMessage(reason, "请稍后重试。")}`);
    } finally {
      setBusy(false);
    }
  }, [refresh, selectedStamps]);

  const formatStamp = (stamp: number): string => new Date(stamp * 1000).toLocaleString();
  const formatBytes = (bytes: number | null): string =>
    bytes === null ? "—" : bytes > 1024 * 1024 ? `${(bytes / 1024 / 1024).toFixed(1)} MB` : `${Math.round(bytes / 1024)} KB`;

  return (
    <div className="settings-backup" data-testid="workbuddy-backup-section">
      <div className="settings-panel__section-heading">
        <div>
          <span className="workbench__eyebrow">数据安全</span>
          <h3>对话库备份</h3>
        </div>
        {chain && (
          <span className={`status-badge status-badge--${chain.backups.length > 0 ? "safe" : "neutral"}`}>
            现有 {chain.backups.length} 份
          </span>
        )}
      </div>

      {loading ? (
        <p className="account-center__meta">正在读取备份链…</p>
      ) : loadError ? (
        <EmptyState
          role="alert"
          className="workbuddy-page__empty"
          title="无法读取备份链"
          description={loadError}
          action={
            <button type="button" className="btn btn--primary" onClick={() => void refresh()}>
              重新读取
            </button>
          }
        />
      ) : chain ? (
        <>
          <div className="checkin-page__action-row">
            <button
              type="button"
              className="btn btn--primary"
              onClick={() => setCreateConfirmOpen(true)}
              disabled={busy}
              data-testid="workbuddy-backup-create"
            >
              <DatabaseBackup size={15} aria-hidden="true" />立即备份
            </button>
            <button
              type="button"
              className="btn btn--danger"
              onClick={() => setDeleteConfirmOpen(true)}
              disabled={busy || selectedStamps.length === 0}
              data-testid="workbuddy-backup-delete"
            >
              <Trash2 size={15} aria-hidden="true" />删除所选（{selectedStamps.length}）
            </button>
            <span className="account-item__meta">备份目录：{chain.backup_dir}</span>
          </div>

          {chain.backups.length > 0 ? (
            <ul className="account-list" data-testid="workbuddy-backup-chain">
              {chain.backups.map((entry) => (
                <li key={entry.stamp_unix_seconds}>
                  <div className="account-list__row">
                    <div className="account-item__main">
                      <label className="account-center__profile-select">
                        <input
                          type="checkbox"
                          checked={selectedStamps.includes(entry.stamp_unix_seconds)}
                          disabled={busy}
                          onChange={() => toggleSelect(entry.stamp_unix_seconds)}
                          data-testid={`workbuddy-backup-select-${entry.stamp_unix_seconds}`}
                        />
                      </label>
                      <div className="account-item__id">
                        <div className="account-item__name-row">
                          <strong className="account-item__name">{formatStamp(entry.stamp_unix_seconds)}</strong>
                          {entry.file_count === null && (
                            <span className="slot-badge slot-badge--warn" title="备份清单不可读（半成品目录），如实呈现">清单缺失</span>
                          )}
                        </div>
                        <div className="account-meta-group">
                          <p className="account-item__meta">
                            {entry.file_count === null ? "文件数未知" : `${entry.file_count} 个文件`} · 源 {formatBytes(entry.source_bytes)} · 压缩 {formatBytes(entry.compressed_bytes)}
                          </p>
                        </div>
                      </div>
                    </div>
                    <div className="account-item__side">
                      <button
                        type="button"
                        className="btn"
                        onClick={() => setRestoreTarget(entry.stamp_unix_seconds)}
                        disabled={busy}
                        data-testid={`workbuddy-backup-restore-${entry.stamp_unix_seconds}`}
                        title="从这份备份恢复（覆盖前自动保链备份当前状态）"
                      >
                        恢复
                      </button>
                    </div>
                  </div>
                </li>
              ))}
            </ul>
          ) : (
            <EmptyState
              className="workbuddy-page__empty"
              description="还没有备份。彻底删除会话前会自动创建，也可以点击「立即备份」。"
            />
          )}
        </>
      ) : null}

      {/* 手动备份单次确认（进程纪律：客户端会关闭并重启）。 */}
      {createConfirmOpen && (
        <ConfirmDialog
          title="创建对话库备份"
          lines={[
            "将备份 workbuddy.db、projects 正文目录与登录凭据材料（P19 决策 1 全纳入范围）。",
            "执行时 WorkBuddy 客户端会完全关闭并在备份完成后自动重启。",
            "滚动单份：新备份落位后同目录只保留最新一份。",
          ]}
          confirmLabel="立即备份"
          busyLabel="备份中…"
          busy={busy}
          onCancel={() => setCreateConfirmOpen(false)}
          onConfirm={() => void runCreate()}
          testId="workbuddy-backup-create-confirm"
        />
      )}

      {/* 恢复单次确认（覆盖性操作；safety 保链备份说明在册）。 */}
      {restoreTarget !== null && (
        <ConfirmDialog
          title="从备份恢复对话库"
          lines={[
            `将以 ${formatStamp(restoreTarget)} 的备份覆盖当前对话库（workbuddy.db 与正文目录）。`,
            "恢复前会自动把当前状态备份一份（保链不清除），失败可用它回退——恢复完成后会提示回退点。",
            "执行时 WorkBuddy 客户端会完全关闭并在恢复完成后自动重启。",
          ]}
          confirmLabel="确认恢复"
          busyLabel="恢复中…"
          busy={busy}
          danger
          onCancel={() => setRestoreTarget(null)}
          onConfirm={() => void runRestore(restoreTarget)}
          testId="workbuddy-backup-restore-confirm"
        />
      )}

      {/* 删除单次确认。 */}
      {deleteConfirmOpen && (
        <ConfirmDialog
          title="删除备份"
          lines={[
            `将删除所选 ${selectedStamps.length} 份备份，不可恢复。`,
            "建议至少保留一份备份；恢复前的保链备份不受本操作影响（按所选清单执行）。",
          ]}
          confirmLabel="确认删除"
          busyLabel="删除中…"
          busy={busy}
          danger
          onCancel={() => setDeleteConfirmOpen(false)}
          onConfirm={() => void runDelete()}
          testId="workbuddy-backup-delete-confirm"
        />
      )}
    </div>
  );
}
