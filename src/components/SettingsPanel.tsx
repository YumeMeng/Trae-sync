import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { CalendarClock, DatabaseBackup } from "lucide-react";
import type { AutoCheckinStatusDto, KeyStatusDto } from "../types/account_switch";
import type { BackupRetentionDto, MasterBackupChainDto } from "../types/masterLibrary";
import { safeUiErrorMessage } from "../utils/safeUiError";

interface SettingsPanelProps {
  /** 页面可见时才读取密钥状态，避免后台 IPC。 */
  active: boolean;
}

// 设置页：自动签到偏好 + 密钥维护（自账号页迁入，账号页专注账号档案）+ 主库备份。
// 原始密钥只在后端内存中流转；界面只显示版本与探测结论，永不回显正文。
export function SettingsPanel({ active }: SettingsPanelProps) {
  const [keyStatus, setKeyStatus] = useState<KeyStatusDto | null>(null);
  const [candidateKey, setCandidateKey] = useState("");
  const [candidateProductVersion, setCandidateProductVersion] = useState("TRAE Work CN");
  const [keyBusy, setKeyBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [message, setMessage] = useState<string | null>(null);
  // 自动签到设置（ADR-0019 决策 5 / 2026-08-23 grill）：总开关 + 每日时间点 + 今日台账。
  const [autoStatus, setAutoStatus] = useState<AutoCheckinStatusDto | null>(null);
  const [autoBusy, setAutoBusy] = useState(false);
  // P5-4 主库数据备份：备份链（.switch-bak-*）展示 + 手动创建入口。
  // 读取失败（fixture 模式等）静默降级为 null，分区整体不渲染（可插拔语义）。
  const [backupChain, setBackupChain] = useState<MasterBackupChainDto | null>(null);
  const [backupBusy, setBackupBusy] = useState(false);
  // P5-9 备份保留设置：自动清理开关 + 保留份数（与备份链同分区展示）。
  const [retention, setRetention] = useState<BackupRetentionDto | null>(null);
  const [retentionBusy, setRetentionBusy] = useState(false);

  const loadBackupChain = useCallback(async () => {
    try {
      const chain = await invoke<MasterBackupChainDto>("get_master_backup_chain");
      setBackupChain(chain);
    } catch {
      setBackupChain(null);
    }
  }, []);

  useEffect(() => {
    if (!active) return;
    let cancelled = false;
    void invoke<BackupRetentionDto>("get_backup_retention")
      .then((next) => { if (!cancelled) setRetention(next); })
      .catch(() => undefined); // 读取失败不阻塞页面；保存时会再次报错
    return () => { cancelled = true; };
  }, [active]);

  useEffect(() => {
    if (!active) return;
    let cancelled = false;
    void invoke<KeyStatusDto>("get_key_status")
      .then((next) => { if (!cancelled) setKeyStatus(next); })
      .catch((reason: unknown) => {
        if (!cancelled) setError(safeUiErrorMessage(reason, "密钥状态暂时不可读取。"));
      });
    void invoke<AutoCheckinStatusDto>("get_auto_checkin_settings")
      .then((next) => { if (!cancelled) setAutoStatus(next); })
      .catch(() => undefined); // 读取失败不阻塞页面；保存时会再次报错
    void loadBackupChain();
    return () => { cancelled = true; };
  }, [active, loadBackupChain]);

  // 手动备份：主库三件套复制为 .switch-bak-{时间戳}（create_new 永不覆盖）。
  // 成功后重读备份链；失败提示稳定原因（如主库运行中），原数据不受影响。
  const handleCreateBackup = useCallback(() => {
    if (backupBusy) return;
    setBackupBusy(true);
    setError(null);
    setMessage(null);
    void invoke<string>("create_master_backup")
      .then(() => loadBackupChain())
      .then(() => setMessage("主库数据备份已创建，原数据未受影响。"))
      .catch((reason: unknown) => setError(safeUiErrorMessage(reason, "主库数据备份未完成。")))
      .finally(() => setBackupBusy(false));
  }, [backupBusy, loadBackupChain]);

  // P5-9 保存备份保留设置：开关与保留份数一起提交（后端范围校验 1-50）。
  const handleSaveRetention = useCallback((enabled: boolean, keep: number) => {
    if (retentionBusy) return;
    setRetentionBusy(true);
    setError(null);
    setMessage(null);
    void invoke("set_backup_retention", { enabled, keep })
      .then(() => {
        setRetention({ enabled, keep });
        setMessage(enabled
          ? `备份自动清理已开启，超出 ${keep} 份的旧备份会在新备份生成后自动删除。`
          : "备份自动清理已关闭，旧备份不会再被自动删除。");
      })
      .catch((reason: unknown) => setError(safeUiErrorMessage(reason, "备份清理设置保存失败。")))
      .finally(() => setRetentionBusy(false));
  }, [retentionBusy]);

  // 保存自动签到设置：开关与时间一起提交（后端原子写保留台账 + 格式校验）。
  const handleSaveAutoCheckin = useCallback((enabled: boolean, dailyTime: string) => {
    if (autoBusy) return;
    setAutoBusy(true);
    setError(null);
    setMessage(null);
    void invoke("set_auto_checkin_settings", { enabled, dailyTimeHhmm: dailyTime })
      .then(() => invoke<AutoCheckinStatusDto>("get_auto_checkin_settings"))
      .then((next) => {
        setAutoStatus(next);
        setMessage(enabled ? `自动签到已开启，每日 ${dailyTime} 后自动执行。` : "自动签到已关闭。");
      })
      .catch((reason: unknown) => setError(safeUiErrorMessage(reason, "自动签到设置保存失败。")))
      .finally(() => setAutoBusy(false));
  }, [autoBusy]);

  const handleProbeKey = useCallback(() => {
    if (keyBusy) return;
    setKeyBusy(true);
    setError(null);
    setMessage(null);
    void invoke<KeyStatusDto>("probe_source_key")
      .then((next) => {
        setKeyStatus(next);
        setMessage(next.probe_state === "verified" ? "当前密钥只读探测通过。" : "当前密钥未通过只读探测，已保持阻断。");
      })
      .catch((reason: unknown) => setError(safeUiErrorMessage(reason, "密钥探测未完成。")))
      .finally(() => setKeyBusy(false));
  }, [keyBusy]);

  const handleRegisterKey = useCallback(() => {
    const trimmedKey = candidateKey.trim();
    if (!trimmedKey || keyBusy) {
      if (!trimmedKey) setError("请输入候选 source key。");
      return;
    }
    setKeyBusy(true);
    setError(null);
    setMessage(null);
    void invoke<KeyStatusDto>("register_source_key_candidate", {
      candidateKey: trimmedKey,
      productVersion: candidateProductVersion.trim() || undefined,
    })
      .then((next) => {
        setKeyStatus(next);
        setCandidateKey("");
        setMessage("候选 source key 已通过只读验证并登记，下一次启动才会激活。");
      })
      .catch((reason: unknown) => setError(safeUiErrorMessage(reason, "候选密钥登记未完成。")))
      .finally(() => setKeyBusy(false));
  }, [candidateKey, candidateProductVersion, keyBusy]);

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
            className="account-center__profile-select"
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

      {error && <p className="workbench__error" role="alert">{error}</p>}
      {message && <p className="settings-panel__key-message" role="status">{message}</p>}

      {/* G3：密钥主视野只留结论，探测与候选维护动作默认收起。 */}
      <details className="settings-panel__fold" data-testid="settings-key-details">
        <summary>
          <span>
            <span className="workbench__eyebrow">安全维护</span>
            <strong>密钥</strong>
          </span>
          <span className={`status-badge status-badge--${keyConclusion(keyStatus) === "密钥正常" ? "safe" : "neutral"}`}>
            {keyConclusion(keyStatus)}
          </span>
        </summary>
        <div className="settings-panel__fold-content">
          <div className="settings-panel__section-heading">
            <div>
              <span className="workbench__eyebrow">当前状态</span>
              <h3>密钥维护</h3>
            </div>
            <span className={`status-badge status-badge--${keyStatus?.probe_state === "verified" || keyStatus?.probe_state === "verified_pending" ? "safe" : "neutral"}`}>
              {keyProbeLabel(keyStatus?.probe_state)}
            </span>
          </div>
      <div className="account-center__key-grid">
        <span>来源密钥：{keyStatus?.source_key_configured ? keyStatus.source_key_version : "不可用"}</span>
        <span>历史库密钥：{keyStatus?.catalog_key_configured ? `代次 ${keyStatus.catalog_key_generation ?? "当前"}` : "不可用"}</span>
        {keyStatus?.source_key_pending_version && <span>待激活候选：{keyStatus.source_key_pending_version}</span>}
        <button className="btn btn--quiet" type="button" onClick={handleProbeKey} disabled={keyBusy || !keyStatus?.source_key_configured}>
          {keyBusy ? "探测中…" : "重新探测密钥"}
        </button>
      </div>
      <div className="account-center__key-grid" data-testid="source-key-candidate-form">
        <label className="account-center__target-field">
          <span>候选 source key</span>
          <input type="password" value={candidateKey} onChange={(event) => setCandidateKey(event.target.value)} autoComplete="off" data-testid="source-key-candidate-input" />
        </label>
        <label className="account-center__target-field">
          <span>产品版本</span>
          <input type="text" value={candidateProductVersion} onChange={(event) => setCandidateProductVersion(event.target.value)} data-testid="source-key-product-version-input" />
        </label>
        <button className="btn btn--quiet" type="button" onClick={handleRegisterKey} disabled={keyBusy || candidateKey.trim().length === 0}>
          {keyBusy ? "登记中…" : "登记候选密钥"}
        </button>
      </div>
      {keyStatus?.source_key_activation_pending && (
        <p className="account-center__meta" data-testid="source-key-pending-notice">候选密钥已登记，下一次启动激活；当前运行继续使用已激活版本。</p>
      )}
        </div>
      </details>

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
                  </li>
                ))}
              </ul>
            ) : (
              <p className="settings-backup__empty">尚无备份；切换账号时会自动生成，也可以现在手动创建一份。</p>
            )}
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
            <details className="settings-panel__fold settings-backup__location" data-testid="master-backup-location-details">
              <summary>查看备份位置与恢复步骤</summary>
              <p className="settings-backup__hint" data-testid="master-backup-restore-hint">
                备份保存在 <code>{backupChain.backup_dir}</code> 目录（.switch-bak- 前缀）。
                {retention?.enabled
                  ? `超出保留份数的旧备份会自动清理（当前保留 ${retention.keep} 份）。`
                  : "未开启自动清理时，备份不会被自动删除。"}
                人工恢复：先关闭 TRAE，再把所选备份内的文件复制回该目录覆盖原位，然后重新启动。
              </p>
            </details>
          </div>
        </>
      )}
    </section>
  );
}

function keyProbeLabel(state: KeyStatusDto["probe_state"] | undefined | null): string {
  switch (state) {
    case "verified": return "探测通过";
    case "verified_pending": return "候选已登记";
    case "rejected": return "探测拒绝";
    default: return "未探测";
  }
}

function keyConclusion(status: KeyStatusDto | null): "密钥正常" | "密钥异常需维护" {
  const verified = status?.probe_state === "verified" || status?.probe_state === "verified_pending";
  return status?.source_key_configured && status.catalog_key_configured && verified
    ? "密钥正常"
    : "密钥异常需维护";
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
