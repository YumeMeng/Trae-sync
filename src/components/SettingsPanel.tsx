import { useCallback, useEffect, useLayoutEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { CalendarClock, DatabaseBackup, FolderSymlink } from "lucide-react";
import type { AutoCheckinStatusDto } from "../types/account_switch";
import type {
  BackupMigrationDto,
  BackupRetentionDto,
  MasterBackupChainDto,
} from "../types/masterLibrary";
import { safeUiErrorMessage } from "../utils/safeUiError";
// 统一通知适配层（P11-1 切片②）：操作结果类提示一律经 notify 发出。
import { notify } from "../platform/notify";
import type { ProductId } from "../platform/productRegistry";
import { EmptyState } from "./EmptyState";

interface SettingsPanelProps {
  /** 页面可见时才读取设置，避免后台 IPC。 */
  active: boolean;
  /** 当前产品；缺省 Work 以保留既有调用行为。 */
  product?: ProductId;
}

// 设置页：自动签到偏好 + 主库备份。
export function SettingsPanel({ active, product = "work_cn" }: SettingsPanelProps) {
  // 自动签到设置（ADR-0019 决策 5 / 2026-08-23 grill）：总开关 + 每日时间点 + 今日台账。
  const [autoStatus, setAutoStatus] = useState<AutoCheckinStatusDto | null>(null);
  const [autoBusy, setAutoBusy] = useState(false);
  // P5-4 主库数据备份：备份链（.switch-bak-*）展示 + 手动创建入口。
  // 读取失败（fixture 模式等）静默降级为 null，分区整体不渲染（可插拔语义）。
  const [backupChain, setBackupChain] = useState<MasterBackupChainDto | null>(null);
  const [backupBusy, setBackupBusy] = useState(false);
  // P11-4② 备份内容可选「包含账号凭据」：默认不勾，避免无意识扩散凭据文件。
  const [includeCredentials, setIncludeCredentials] = useState(false);
  // P11-4① 更改备份位置弹层：目录输入 + 是否同时迁移现有备份（默认勾选）。
  const [locationDialogOpen, setLocationDialogOpen] = useState(false);
  const [locationInput, setLocationInput] = useState("");
  const [migrateExisting, setMigrateExisting] = useState(true);
  const [locationBusy, setLocationBusy] = useState(false);
  // P5-9 备份保留设置：自动清理开关 + 保留份数（与备份链同分区展示）。
  const [retention, setRetention] = useState<BackupRetentionDto | null>(null);
  const [retentionBusy, setRetentionBusy] = useState(false);

  // 产品切换后作废旧操作的界面回写，并重置当前产品的忙状态与弹层。
  const previousProduct = useRef(product);
  const currentProduct = useRef(product);
  const currentActive = useRef(active);
  const productGeneration = useRef(0);
  // 每类设置单独递增读取序号，避免旧快照晚到后覆盖更新值。
  const backupChainReadVersion = useRef(0);
  const retentionReadVersion = useRef(0);
  const autoStatusReadVersion = useRef(0);

  useLayoutEffect(() => {
    currentProduct.current = product;
    currentActive.current = active;
    if (previousProduct.current === product) return;
    previousProduct.current = product;
    productGeneration.current += 1;
    backupChainReadVersion.current += 1;
    retentionReadVersion.current += 1;
    autoStatusReadVersion.current += 1;
    setBackupBusy(false);
    setLocationBusy(false);
    setRetentionBusy(false);
    setAutoBusy(false);
    setLocationDialogOpen(false);
    setLocationInput("");
  }, [active, product]);

  const isCurrentProductGeneration = useCallback(
    (generation: number) => productGeneration.current === generation,
    [],
  );

  const readSnapshot = useCallback(<T,>(
    version: { current: number },
    read: () => Promise<T>,
    apply: (snapshot: T) => void,
    isEffectCurrent: () => boolean = () => true,
  ) => {
    const requestVersion = ++version.current;
    return read().then((snapshot) => {
      if (
        version.current === requestVersion
        && currentProduct.current === "work_cn"
        && currentActive.current
        && isEffectCurrent()
      ) {
        apply(snapshot);
      }
      return snapshot;
    });
  }, []);

  const refreshStaleWorkSnapshot = useCallback(<T,>(
    operationGeneration: number,
    read: () => Promise<T>,
  ) => {
    if (
      isCurrentProductGeneration(operationGeneration)
      || currentProduct.current !== "work_cn"
      || !currentActive.current
    ) return;
    void read()
      .catch(() => undefined);
  }, [isCurrentProductGeneration]);

  const formatWorkNotification = useCallback((origin: ProductId, message: string) => (
    origin === "work_cn" && currentProduct.current !== origin ? `TRAE Work CN：${message}` : message
  ), []);

  const loadBackupChain = useCallback((isEffectCurrent: () => boolean = () => true) => {
    if (!currentActive.current || currentProduct.current !== "work_cn") return Promise.resolve(null);
    return readSnapshot(
      backupChainReadVersion,
      () => invoke<MasterBackupChainDto>("get_master_backup_chain").catch(() => null),
      setBackupChain,
      isEffectCurrent,
    );
  }, [readSnapshot]);

  const loadRetention = useCallback((isEffectCurrent: () => boolean = () => true) => {
    if (!currentActive.current || currentProduct.current !== "work_cn") return Promise.resolve(null);
    return readSnapshot(
      retentionReadVersion,
      () => invoke<BackupRetentionDto>("get_backup_retention"),
      setRetention,
      isEffectCurrent,
    );
  }, [readSnapshot]);

  const loadAutoStatus = useCallback((isEffectCurrent: () => boolean = () => true) => {
    if (!currentActive.current || currentProduct.current !== "work_cn") return Promise.resolve(null);
    return readSnapshot(
      autoStatusReadVersion,
      () => invoke<AutoCheckinStatusDto>("get_auto_checkin_settings"),
      setAutoStatus,
      isEffectCurrent,
    );
  }, [readSnapshot]);

  useEffect(() => {
    if (product === "work_cn") return;
    setAutoStatus(null);
    setBackupChain(null);
    setRetention(null);
  }, [product]);

  useEffect(() => {
    if (!active || product !== "work_cn") return;
    let cancelled = false;
    void loadRetention(() => !cancelled)
      .catch(() => undefined); // 读取失败不阻塞页面；保存时会再次报错
    return () => { cancelled = true; };
  }, [active, loadRetention, product]);

  useEffect(() => {
    if (!active || product !== "work_cn") return;
    let cancelled = false;
    void loadAutoStatus(() => !cancelled)
      .catch(() => undefined); // 读取失败不阻塞页面；保存时会再次报错
    void loadBackupChain(() => !cancelled);
    return () => { cancelled = true; };
  }, [active, product, loadAutoStatus, loadBackupChain]);

  // 手动备份：主库三件套复制为 .switch-bak-{时间戳}（create_new 永不覆盖）。
  // 勾选凭据时后端把签到凭据白名单整拷到 credentials-{同时间戳}/ 子目录。
  // 成功后重读备份链；失败提示稳定原因（如主库运行中），原数据不受影响。
  const handleCreateBackup = useCallback(() => {
    if (product !== "work_cn" || backupBusy) return;
    const origin = product;
    const generation = productGeneration.current;
    setBackupBusy(true);
    void invoke<string>("create_master_backup", { includeCredentials })
      .then(async () => {
        if (!isCurrentProductGeneration(generation)) return;
        await loadBackupChain();
      })
      .then(() =>
        notify.success(
          formatWorkNotification(origin, includeCredentials
            ? "主库数据备份已创建（含账号凭据子目录），原数据未受影响。"
            : "主库数据备份已创建，原数据未受影响。"),
        ),
      )
      .catch((reason: unknown) => notify.error(
        formatWorkNotification(origin, safeUiErrorMessage(reason, "主库数据备份未完成。")),
      ))
      .finally(() => {
        if (isCurrentProductGeneration(generation)) setBackupBusy(false);
        else refreshStaleWorkSnapshot(generation, loadBackupChain);
      });
  }, [backupBusy, formatWorkNotification, includeCredentials, isCurrentProductGeneration, loadBackupChain, product, refreshStaleWorkSnapshot]);

  // P11-4① 提交备份位置变更：先迁移（此刻活动目录仍是旧位置）再切换；
  // 迁移出现失败清单时中止切换（已复制的保留，可排查后重试，迁移幂等）。
  const handleSaveBackupLocation = useCallback(() => {
    if (product !== "work_cn" || locationBusy) return;
    const origin = product;
    const generation = productGeneration.current;
    const targetDir = locationInput.trim();
    if (targetDir === "") return;
    setLocationBusy(true);
    const run: Promise<BackupMigrationDto | null> = migrateExisting
      ? invoke<BackupMigrationDto>("migrate_master_backups", { targetDir })
      : Promise.resolve(null);
    void run
      .then((migration) => {
        if (migration && migration.failures.length > 0) {
          // 部分失败：不切换位置，返回清单（notify.summary 带异常逐行）。
          notify.summary(
            formatWorkNotification(origin, "部分备份未能复制到新位置，备份位置未变更"),
            migration.failures.map((failure) => ({
              displayName: failure.file_name,
              reason: failure.reason,
            })),
            migration.migrated_files,
          );
          return;
        }
        return invoke("set_backup_dir", { backupDir: targetDir }).then(() => {
          notify.success(
            formatWorkNotification(origin, migration
              ? `备份位置已更新，${migration.migrated_files} 个文件已复制到新位置（原位置备份保留，可自行清理）。`
              : "备份位置已更新，之后的备份将写入新目录。"),
          );
        });
      })
      .then(() => {
        if (!isCurrentProductGeneration(generation)) return;
        setLocationDialogOpen(false);
        setLocationInput("");
        return loadBackupChain();
      })
      .catch((reason: unknown) => notify.error(
        formatWorkNotification(origin, safeUiErrorMessage(reason, "备份位置变更未完成，当前设置保持不变。")),
      ))
      .finally(() => {
        if (isCurrentProductGeneration(generation)) setLocationBusy(false);
        else refreshStaleWorkSnapshot(generation, loadBackupChain);
      });
  }, [formatWorkNotification, isCurrentProductGeneration, locationBusy, locationInput, migrateExisting, loadBackupChain, product, refreshStaleWorkSnapshot]);

  // P5-9 保存备份保留设置：开关与保留份数一起提交（后端范围校验 1-50）。
  const handleSaveRetention = useCallback((enabled: boolean, keep: number) => {
    if (product !== "work_cn" || retentionBusy) return;
    const origin = product;
    const generation = productGeneration.current;
    setRetentionBusy(true);
    void invoke("set_backup_retention", { enabled, keep })
      .then(() => {
        if (!isCurrentProductGeneration(generation)) return null;
        setRetention({ enabled, keep });
        return loadRetention().catch(() => undefined);
      })
      .then(() => {
        notify.success(formatWorkNotification(origin, enabled
          ? `备份自动清理已开启，超出 ${keep} 份的旧备份会在新备份生成后自动删除。`
          : "备份自动清理已关闭，旧备份不会再被自动删除。"));
      })
      .catch((reason: unknown) => notify.error(
        formatWorkNotification(origin, safeUiErrorMessage(reason, "备份清理设置保存失败。")),
      ))
      .finally(() => {
        if (isCurrentProductGeneration(generation)) setRetentionBusy(false);
        else refreshStaleWorkSnapshot(generation, loadRetention);
      });
  }, [formatWorkNotification, isCurrentProductGeneration, loadRetention, product, refreshStaleWorkSnapshot, retentionBusy]);

  // 保存自动签到设置：开关与时间一起提交（后端原子写保留台账 + 格式校验）。
  const handleSaveAutoCheckin = useCallback((enabled: boolean, dailyTime: string) => {
    if (product !== "work_cn" || autoBusy) return;
    const origin = product;
    const generation = productGeneration.current;
    setAutoBusy(true);
    void invoke("set_auto_checkin_settings", { enabled, dailyTimeHhmm: dailyTime })
      .then(() => isCurrentProductGeneration(generation)
        ? loadAutoStatus()
        : null)
      .then(() => {
        notify.success(formatWorkNotification(
          origin,
          enabled ? `自动签到已开启，每日 ${dailyTime} 后自动执行。` : "自动签到已关闭。",
        ));
      })
      .catch((reason: unknown) => notify.error(
        formatWorkNotification(origin, safeUiErrorMessage(reason, "自动签到设置保存失败。")),
      ))
      .finally(() => {
        if (isCurrentProductGeneration(generation)) setAutoBusy(false);
        else refreshStaleWorkSnapshot(generation, loadAutoStatus);
      });
  }, [autoBusy, formatWorkNotification, isCurrentProductGeneration, loadAutoStatus, product, refreshStaleWorkSnapshot]);

  if (product !== "work_cn") {
    return (
      <section className="settings-panel" role="region" aria-label="设置">
        <header className="page-header">
          <div className="page-header__copy">
            <span className="page-header__eyebrow">偏好与安全</span>
            <h1 data-page-title="settings" tabIndex={-1}>设置</h1>
          </div>
        </header>
        <EmptyState
          role="status"
          description="当前产品暂时没有可管理的设置。"
          testId="product-settings-empty"
        />
      </section>
    );
  }

  return (
    <section className="settings-panel" role="region" aria-label="设置">
      <header className="page-header">
        <div className="page-header__copy">
          <span className="page-header__eyebrow">偏好与安全</span>
          <h1 data-page-title="settings" tabIndex={-1}>设置</h1>
        </div>
      </header>

      {/* 自动签到：总开关 + 每日时间点；每账号参与开关在账号详情页 */}
      <div className="settings-panel__section-heading">
        <div>
          <span className="workbench__eyebrow">应用行为</span>
          <h3>自动签到</h3>
        </div>
        <span className={`status-badge status-badge--${autoStatus?.enabled ? "safe" : "neutral"}`}>
          {autoStatus?.enabled ? "已开启" : "已关闭"}
        </span>
      </div>
      {autoStatus ? (
        <div className="account-center__key-grid" data-testid="auto-checkin-settings">
          {/* 总开关用 checkbox 形态（与账号详情页的参与开关一致）；错峰与补偿细节收进悬浮提示 */}
          <label
            className="settings-panel__toggle-row"
            data-testid="auto-checkin-enabled-toggle"
            title="到点后各账号在 0-15 分钟内随机错峰执行；迟于设定时间打开应用会自动补偿执行。"
          >
            <input
              type="checkbox"
              checked={autoStatus.enabled}
              disabled={autoBusy}
              onChange={(event) => handleSaveAutoCheckin(event.target.checked, autoStatus.daily_time_hhmm)}
            />
            <span className="account-center__profile-copy">
              <strong>每日自动签到</strong>
              <span>开启后到点自动执行</span>
            </span>
          </label>
          <label className="account-center__target-field">
            <span>触发时间</span>
            <input
              type="time"
              value={autoStatus.daily_time_hhmm}
              disabled={autoBusy || !autoStatus.enabled}
              onChange={(event) => {
                if (event.target.value) handleSaveAutoCheckin(autoStatus.enabled, event.target.value);
              }}
              data-testid="auto-checkin-time-input"
            />
          </label>
          <span className="account-center__meta" data-testid="auto-checkin-ledger">
            <CalendarClock size={14} aria-hidden="true" />{autoLedgerLabel(autoStatus)}
          </span>
        </div>
      ) : (
        <p className="account-center__meta">正在读取自动签到设置…</p>
      )}

      {/* P5-4 主库数据备份（ADR-0018）：备份链展示 + 手动创建 + 人工恢复指引。
          P5-9 备份保留：超出保留数的旧备份自动清理（可关闭）；恢复仍是人工操作，界面只给定位与步骤。 */}
      {backupChain && (
        <>
          <div className="settings-panel__section-heading">
            <div>
              <span className="workbench__eyebrow">数据安全</span>
              <h3>主库数据备份</h3>
            </div>
            <span className={`status-badge status-badge--${backupChain.backups.length > 0 ? "safe" : "neutral"}`}>
              现有 {backupChain.backups.length} 份
            </span>
          </div>
          <div className="settings-backup" data-testid="master-backup-section">
            {retention && (
              <div className="account-center__key-grid" data-testid="backup-retention-settings">
                <label className="account-center__target-field">
                  <span>自动清理旧备份</span>
                  <select
                    value={retention.enabled ? "on" : "off"}
                    disabled={retentionBusy}
                    onChange={(event) => handleSaveRetention(event.target.value === "on", retention.keep)}
                    data-testid="backup-retention-enabled-select"
                  >
                    <option value="on">开启（超出保留数自动删除）</option>
                    <option value="off">关闭（永不自动删除）</option>
                  </select>
                </label>
                <label className="account-center__target-field">
                  <span>保留份数（1-50）</span>
                  <input
                    type="number"
                    min={1}
                    max={50}
                    value={retention.keep}
                    disabled={retentionBusy || !retention.enabled}
                    onChange={(event) => {
                      const next = Number(event.target.value);
                      // 空输入或越界值不保存；1-50 内立即提交。
                      if (Number.isFinite(next) && next >= 1 && next <= 50) {
                        handleSaveRetention(retention.enabled, Math.floor(next));
                      }
                    }}
                    data-testid="backup-retention-keep-input"
                  />
                </label>
                <span className="account-center__meta" data-testid="backup-retention-hint">
                  {retention.enabled
                    ? "超出保留份数的旧备份，会在新备份生成后自动删除"
                    : "当前未开启自动清理，备份不会被自动删除"}
                </span>
              </div>
            )}
            {backupChain.backups.length > 0 ? (
              <ul className="settings-backup__list">
                {backupChain.backups.map((entry) => (
                  <li key={entry.stamp_unix_seconds} className="settings-backup__item" data-testid="master-backup-entry">
                    <DatabaseBackup size={14} aria-hidden="true" />
                    <span className="settings-backup__time">{formatBackupTime(entry.stamp_unix_seconds)}</span>
                    <span className="settings-backup__size">{formatBackupSize(entry.total_bytes)}</span>
                    <span
                      className="settings-backup__kind"
                      title={entry.has_wal ? "备份包含运行附属件，恢复时需一并复制回原位" : "完整数据库快照"}
                    >
                      {entry.has_wal ? "含附属件" : "完整快照"}
                    </span>
                    {entry.has_credentials && (
                      <span className="settings-backup__kind" title="这份备份同时保存了账号凭据（加密密文），恢复时把凭据子目录一并复制回签到材料目录">
                        含凭据
                      </span>
                    )}
                  </li>
                ))}
              </ul>
            ) : (
              <EmptyState
                className="settings-backup__empty"
                description="尚无备份；切换账号时会自动生成，也可以现在手动创建一份。"
              />
            )}
            {/* P11-4② 备份内容可选：凭据文件属敏感材料，默认不勾避免无意识扩散。 */}
            <label className="settings-panel__toggle-row" data-testid="backup-include-credentials-toggle" title="把账号凭据（加密密文）与账号清单一并备份到备份目录下的凭据子目录；仅同一 Windows 用户可恢复。">
              <input
                type="checkbox"
                checked={includeCredentials}
                disabled={backupBusy}
                onChange={(event) => setIncludeCredentials(event.target.checked)}
              />
              <span className="account-center__profile-copy">
                <strong>包含账号凭据</strong>
                <span>备份签到账号的加密凭据与清单（默认不勾）</span>
              </span>
            </label>
            <div className="settings-backup__actions">
              <button
                className="btn btn--quiet"
                type="button"
                onClick={handleCreateBackup}
                disabled={backupBusy}
                data-testid="master-backup-create"
              >
                <DatabaseBackup size={15} aria-hidden="true" />
                {backupBusy ? "备份中…" : "立即备份"}
              </button>
              <button
                className="btn btn--quiet"
                type="button"
                onClick={() => {
                  setLocationInput("");
                  setMigrateExisting(true);
                  setLocationDialogOpen(true);
                }}
                disabled={locationBusy}
                data-testid="backup-location-change"
              >
                <FolderSymlink size={15} aria-hidden="true" />
                更改备份位置
              </button>
            </div>
            {/* P11-4① 当前备份位置：低频信息收悬浮（S5）——主视野单行截断，完整路径悬浮可见。 */}
            <p className="account-center__meta" data-testid="backup-location-current">
              当前备份位置：
              <code className="settings-panel__backup-dir" title={backupChain.backup_dir}>
                {backupChain.backup_dir}
              </code>
            </p>
            <details className="settings-panel__fold settings-backup__location" data-testid="master-backup-location-details">
              <summary>查看恢复步骤</summary>
              <p className="settings-backup__hint" data-testid="master-backup-restore-hint">
                备份保存在上方所示目录（.switch-bak- 前缀）。
                {retention?.enabled
                  ? `超出保留份数的旧备份会自动清理（当前保留 ${retention.keep} 份）。`
                  : "未开启自动清理时，备份不会被自动删除。"}
                人工恢复：先关闭 TRAE，再把所选备份内的文件复制回该目录覆盖原位，然后重新启动。
                {backupChain.backups.some((entry) => entry.has_credentials)
                  ? "带「含凭据」的备份在凭据子目录（credentials- 前缀）中保存了账号加密凭据：恢复账号时把该子目录的内容复制回签到材料目录（checkin）即可。凭据子目录随对应主备份一并保留/清理；无对应主备份的凭据目录可手动清理。"
                  : ""}
              </p>
            </details>
          </div>
          {/* P11-4① 更改备份位置弹层：目录输入 + 迁移勾选（迁移是复制、原位保留）。 */}
          {locationDialogOpen && (
            <div className="switch-veil" role="presentation">
              <div className="switch-dialog" role="dialog" aria-modal="true" aria-label="更改备份位置" data-testid="backup-location-dialog">
                <div className="switch-dialog__title">更改备份位置</div>
                <p className="settings-backup__hint">
                  新备份将写入新目录；已有备份默认复制过去（原位置保留，可自行清理）。
                </p>
                <input
                  className="settings-panel__text-input"
                  type="text"
                  value={locationInput}
                  placeholder="输入新备份目录的完整路径，如 D:\\TraeBackups"
                  disabled={locationBusy}
                  data-testid="backup-location-input"
                  aria-label="新备份目录"
                  onChange={(event) => setLocationInput(event.target.value)}
                />
                <label className="settings-panel__toggle-row" data-testid="backup-location-migrate-toggle">
                  <input
                    type="checkbox"
                    checked={migrateExisting}
                    disabled={locationBusy}
                    onChange={(event) => setMigrateExisting(event.target.checked)}
                  />
                  <span className="account-center__profile-copy">
                    <strong>同时迁移现有备份</strong>
                    <span>复制到新位置并逐文件校验，原位置保留</span>
                  </span>
                </label>
                <div className="switch-dialog__actions">
                  <button
                    className="btn btn--quiet"
                    type="button"
                    onClick={() => setLocationDialogOpen(false)}
                    disabled={locationBusy}
                    data-testid="backup-location-dialog-cancel"
                  >
                    取消
                  </button>
                  <button
                    className="btn btn--primary"
                    type="button"
                    onClick={handleSaveBackupLocation}
                    disabled={locationBusy || locationInput.trim() === ""}
                    data-testid="backup-location-dialog-confirm"
                  >
                    {locationBusy ? "保存中…" : "保存"}
                  </button>
                </div>
              </div>
            </div>
          )}
        </>
      )}
    </section>
  );
}

/** 今日台账状态文案：未发起 / 错峰执行中 / 已完成（含跳过与失败计数）。 */
function autoLedgerLabel(status: AutoCheckinStatusDto): string {
  const ledger = status.ledger;
  if (!ledger) return "今日尚未发起";
  const parts = [`成功 ${ledger.completed}/${ledger.total}`];
  if (ledger.failed > 0) parts.push(`失败 ${ledger.failed}`);
  if (ledger.skipped > 0) parts.push(`跳过 ${ledger.skipped}`);
  return ledger.running ? `今日执行中（${parts.join("，")}）` : `今日已完成（${parts.join("，")}）`;
}

/** 备份时间：本地日期 + 时分（备份链按时间戳排序，日期粒度足够辨识）。 */
function formatBackupTime(unixSeconds: number): string {
  return new Date(unixSeconds * 1000).toLocaleString(undefined, {
    year: "numeric",
    month: "2-digit",
    day: "2-digit",
    hour: "2-digit",
    minute: "2-digit",
  });
}

/** 备份大小：KiB/MiB 短文案（列表空间有限，不展示字节全量）。 */
function formatBackupSize(totalBytes: number): string {
  if (totalBytes >= 1024 * 1024) return `${(totalBytes / (1024 * 1024)).toFixed(1)} MiB`;
  return `${Math.max(1, Math.round(totalBytes / 1024))} KiB`;
}
