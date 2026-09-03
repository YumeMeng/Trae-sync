import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { DatabaseBackup, RefreshCw, ShieldAlert } from "lucide-react";
import type {
  BackupComparisonDto,
  LedgerVerificationDto,
  MasterVerificationDto,
} from "../types/masterLibrary";
import { safeUiErrorMessage } from "../utils/safeUiError";

// ============================================================================
// P5-7 主库「数据校验」tab（只读排查，发现异常只报告不修复）：
// 区块一 接力记录核对（台账条目 vs 库内实际归属）；
// 区块二 备份对比（可切换链上任一备份点为基准，会话级差异）。
// 进 tab 自动执行 + 刷新按钮（P5-5 体检同模式；tab 懒挂载由详情页负责）。
// ============================================================================

interface MasterVerificationPanelProps {
  /** tab 可见时才读取；不可见不发起请求（与详情页其他分区同口径）。 */
  active: boolean;
}

export function MasterVerificationPanel({ active }: MasterVerificationPanelProps) {
  const [report, setReport] = useState<MasterVerificationDto | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  // 对比基准（备份时间戳）：null = 最新备份；用户可切换链上其他备份点。
  const [baseStamp, setBaseStamp] = useState<number | null>(null);
  const mountedRef = useRef(true);

  useEffect(() => {
    mountedRef.current = true;
    return () => {
      mountedRef.current = false;
    };
  }, []);

  const load = useCallback(async (stamp: number | null) => {
    setLoading(true);
    try {
      const next = await invoke<MasterVerificationDto>("get_master_verification", {
        backupStamp: stamp,
      });
      if (!mountedRef.current) return;
      setReport(next);
      // 回显实际使用的基准（stamp 无效时后端回退最新）。
      setBaseStamp(next.backup.backup_stamp);
      setError(null);
    } catch (reason: unknown) {
      if (!mountedRef.current) return;
      setError(safeUiErrorMessage(reason, "数据校验暂时无法执行，请稍后重试。"));
    } finally {
      if (mountedRef.current) setLoading(false);
    }
  }, []);

  useEffect(() => {
    if (!active) return;
    void load(baseStamp);
    // 进 tab 自动执行；baseStamp 变化（用户切基准）时重查。
  }, [active, baseStamp, load]);

  /** 切换对比基准：重置报告并触发 load（useEffect 依赖 baseStamp）。 */
  const chooseBase = useCallback((stamp: number | null) => {
    setReport(null);
    setBaseStamp(stamp);
  }, []);

  return (
    <div className="verification-panel" data-testid="master-verification">
      <div className="verification-panel__toolbar">
        <button
          className="btn btn--quiet"
          type="button"
          onClick={() => void load(baseStamp)}
          disabled={loading}
          data-testid="master-verification-refresh"
        >
          <RefreshCw size={15} aria-hidden="true" />
          {loading ? "校验中…" : "重新校验"}
        </button>
      </div>

      {error && <p className="workbench__error" role="alert">{error}</p>}

      {report && (
        <>
          <LedgerBlock ledger={report.ledger} />
          <BackupBlock backup={report.backup} onChooseBase={chooseBase} loading={loading} />
        </>
      )}
    </div>
  );
}

/** 区块一：接力记录核对（正常徽章 / 异常列表）。 */
function LedgerBlock({ ledger }: { ledger: LedgerVerificationDto }) {
  const badge = ledgerBadge(ledger);
  return (
    <section className="verification-block" data-testid="verification-ledger-block">
      <div className="verification-block__heading">
        <h4>接力记录核对</h4>
        <span
          className={`slot-badge ${badge.badge}`}
          data-testid="verification-ledger-badge"
        >
          <i className={`slot-badge__dot ${badge.dot}`} aria-hidden="true" />
          {ledger.status === "ready" && ledger.issues.length === 0 && "未发现异常"}
          {ledger.status === "ready" && ledger.issues.length > 0 && `${ledger.issues.length} 项待关注`}
          {ledger.status === "no_ledger" && "无接力记录"}
          {ledger.status === "no_master_data" && "主库未启动"}
          {ledger.status === "read_failed" && "读取失败"}
        </span>
      </div>
      {ledger.status === "ready" && (
        <p className="verification-block__summary">
          {ledger.issues.length === 0
            ? `已核对 ${ledger.checked_count} 个会话的接力记录，全部正常。`
            : `已核对 ${ledger.checked_count} 个会话的接力记录，发现 ${ledger.issues.length} 项待关注。`}
          {ledger.relayed_away_count > 0 &&
            ` 其中 ${ledger.relayed_away_count} 个会话已在后续切换中接力到新会话（正常）。`}
        </p>
      )}
      {ledger.status === "no_ledger" && (
        <p className="verification-block__summary">
          还没有切换过账号，没有接力记录可核对；首次切换后会自动开始记录。
        </p>
      )}
      {ledger.status === "no_master_data" && (
        <p className="verification-block__summary">主库还没有数据，暂时没有可核对的内容。</p>
      )}
      {ledger.status === "read_failed" && (
        <p className="verification-block__summary">
          主库暂时无法读取（可能正在生成回复），稍后重新校验即可。
        </p>
      )}
      {ledger.issues.length > 0 && (
        <ul className="verification-issues" data-testid="verification-ledger-issues">
          {ledger.issues.map((issue, index) => (
            <li
              key={`${issue.kind}-${issue.session_id}-${index}`}
              className="verification-issues__item"
              title={`会话标识：${issue.session_id}`}
            >
              <ShieldAlert size={14} aria-hidden="true" />
              <span>{issue.detail}</span>
            </li>
          ))}
        </ul>
      )}
    </section>
  );
}

/** 区块二：备份对比（基准切换 + 差异摘要 + 丢失候选列表 + 恢复指引）。 */
function BackupBlock({
  backup,
  onChooseBase,
  loading,
}: {
  backup: BackupComparisonDto;
  onChooseBase: (stamp: number | null) => void;
  loading: boolean;
}) {
  const badge = backupBadge(backup);
  return (
    <section className="verification-block" data-testid="verification-backup-block">
      <div className="verification-block__heading">
        <h4>备份对比</h4>
        <span
          className={`slot-badge ${badge.badge}`}
          data-testid="verification-backup-badge"
        >
          <i className={`slot-badge__dot ${badge.dot}`} aria-hidden="true" />
          {backup.status === "ready" && (backup.missing.length > 0
            ? `${backup.missing.length} 个丢失候选`
            : "未发现丢失")}
          {backup.status === "no_backups" && "暂无备份"}
          {backup.status === "backup_missing" && "备份不可用"}
          {backup.status === "read_failed" && "读取失败"}
          {backup.status === "no_master_data" && "主库未启动"}
        </span>
      </div>

      {backup.status === "ready" && backup.backup_stamps.length > 0 && (
        <label className="verification-block__base">
          <span>对比基准</span>
          <select
            value={backup.backup_stamp ?? ""}
            disabled={loading || backup.backup_stamps.length <= 1}
            onChange={(event) => {
              const value = event.target.value;
              onChooseBase(value === "" ? null : Number(value));
            }}
            data-testid="verification-base-select"
          >
            {backup.backup_stamps.map((stamp, index) => (
              <option key={stamp} value={stamp}>
                {index === 0 ? `最新备份（${formatStamp(stamp)}）` : formatStamp(stamp)}
              </option>
            ))}
          </select>
        </label>
      )}

      {backup.status === "ready" && (
        <p className="verification-block__summary" data-testid="verification-backup-summary">
          {backup.missing.length === 0
            ? `与备份一致的会话 ${backup.common_count} 个；切换后新增 ${backup.added_count} 个（正常）。`
            : `与备份一致的会话 ${backup.common_count} 个；新增 ${backup.added_count} 个（正常）；${backup.missing.length} 个会话只在备份中存在。`}
          {backup.relayed_away_count > 0 &&
            ` ${backup.relayed_away_count} 个会话经接力换成了新会话（正常）。`}
        </p>
      )}
      {backup.status === "no_backups" && (
        <p className="verification-block__summary">
          还没有备份可对比；切换账号或手动备份后，这里会用最新备份核对会话是否完整。
        </p>
      )}
      {backup.status === "backup_missing" && (
        <p className="verification-block__summary">所选备份文件已不存在（可能已被清理），请换一个基准。</p>
      )}
      {backup.status === "read_failed" && (
        <p className="verification-block__summary">
          备份或主库暂时无法读取，稍后重新校验即可。
        </p>
      )}
      {backup.status === "no_master_data" && (
        <p className="verification-block__summary">主库还没有数据，暂时没有可对比的内容。</p>
      )}

      {backup.missing.length > 0 && (
        <div className="verification-missing" data-testid="verification-missing-list">
          <ul className="verification-issues">
            {backup.missing.map((session, index) => (
              <li key={index} className="verification-issues__item">
                <DatabaseBackup size={14} aria-hidden="true" />
                <span>
                  「{session.title ?? "未命名会话"}」还在备份中（{session.message_count} 条消息），
                  当前主库里没有。
                </span>
              </li>
            ))}
          </ul>
          <p className="verification-block__hint" data-testid="verification-restore-hint">
            如需找回：先关闭 TRAE，到备份目录把所选备份内的文件复制回原位覆盖，再重新启动；
            或保留现状继续使用（丢失候选只提示，不会自动改动任何数据）。
          </p>
        </div>
      )}
    </section>
  );
}

/** 徽章语义（两槽位契约）：正常=idle+空心点，待关注=warn+琥珀点，空态=unknown+灰点。 */
function ledgerBadge(ledger: LedgerVerificationDto): { badge: string; dot: string } {
  if (ledger.status !== "ready") {
    return { badge: "slot-badge--unknown", dot: "slot-badge__dot--muted" };
  }
  return ledger.issues.length === 0
    ? { badge: "slot-badge--idle", dot: "slot-badge__dot--hollow" }
    : { badge: "slot-badge--warn", dot: "slot-badge__dot--warn" };
}

/** 备份对比徽章：丢失候选>0 = warn（需用户动手找回），否则 idle / unknown。 */
function backupBadge(backup: BackupComparisonDto): { badge: string; dot: string } {
  if (backup.status !== "ready") {
    return { badge: "slot-badge--unknown", dot: "slot-badge__dot--muted" };
  }
  return backup.missing.length === 0
    ? { badge: "slot-badge--idle", dot: "slot-badge__dot--hollow" }
    : { badge: "slot-badge--warn", dot: "slot-badge__dot--warn" };
}

/** 备份时间戳短文案（基准选择器；日期 + 时分粒度足够辨识）。 */
function formatStamp(unixSeconds: number): string {
  return new Date(unixSeconds * 1000).toLocaleString(undefined, {
    month: "2-digit",
    day: "2-digit",
    hour: "2-digit",
    minute: "2-digit",
  });
}
