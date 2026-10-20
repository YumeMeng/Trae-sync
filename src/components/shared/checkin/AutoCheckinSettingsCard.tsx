import type { ReactNode } from "react";

/**
 * 每日自动签到设置卡（P19-B2⑤ 从 WorkBuddyPanel L721-802 抽取为共享控件，
 * IA 草案 §1.3「调度设置」区块的产品无关实现）：
 * 总开关 + 触发时间 + 今日台账行 + 账号参与清单（逐行复选）+ 全部参与入口。
 * 数据语义由产品层注入；行结构与类名全产品一致。
 */
export interface AutoCheckinSettingsCardProps {
  /** testid 前缀（如 workbuddy → workbuddy-auto-checkin-*）。 */
  testIdPrefix: string;
  enabled: boolean;
  dailyTimeHhmm: string;
  busy: boolean;
  /** 开关行说明（产品策略文案）。 */
  toggleSummary: ReactNode;
  /** 今日台账行文案（null = 不渲染）。 */
  ledgerNote: string | null;
  /** 账号参与清单（participating 为 UI 生效参与态）。 */
  accounts: readonly { profileId: string; displayName: string; participating: boolean }[];
  onSettingsChange(enabled: boolean, dailyTimeHhmm: string): void;
  onParticipationChange(profileId: string, enabled: boolean): void;
  /** 批量置 true 入口（P19-B2 参与集默认全参与修复）。 */
  onParticipateAll(): void;
  /** 全部参与按钮文案（缺省「全部参与」）。 */
  participateAllLabel?: string;
}

export function AutoCheckinSettingsCard({
  testIdPrefix,
  enabled,
  dailyTimeHhmm,
  busy,
  toggleSummary,
  ledgerNote,
  accounts,
  onSettingsChange,
  onParticipationChange,
  onParticipateAll,
  participateAllLabel = "全部参与",
}: AutoCheckinSettingsCardProps) {
  const pendingCount = accounts.filter((account) => !account.participating).length;
  return (
    <div className="workbuddy-card" data-testid={`${testIdPrefix}-auto-checkin-settings`}>
      <div className="workbuddy-card__title">每日自动签到</div>
      <div className="account-center__key-grid">
        <label
          className="settings-panel__toggle-row"
          data-testid={`${testIdPrefix}-auto-checkin-enabled-toggle`}
          title="到点后参与账号在 0-15 分钟内随机错峰自动执行。"
        >
          <input
            type="checkbox"
            checked={enabled}
            disabled={busy}
            onChange={(event) => onSettingsChange(event.target.checked, dailyTimeHhmm)}
          />
          <span className="account-center__profile-copy">
            <strong>每日自动签到</strong>
            <span>{toggleSummary}</span>
          </span>
        </label>
        <label className="account-center__target-field">
          <span>触发时间</span>
          <input
            type="time"
            value={dailyTimeHhmm}
            disabled={busy || !enabled}
            onChange={(event) => {
              if (event.target.value) onSettingsChange(enabled, event.target.value);
            }}
            data-testid={`${testIdPrefix}-auto-checkin-time-input`}
          />
        </label>
        {ledgerNote && (
          <span className="account-center__meta" data-testid={`${testIdPrefix}-auto-checkin-ledger`}>
            {ledgerNote}
          </span>
        )}
      </div>
      {accounts.length > 0 && (
        <>
          {/* 参与集批量入口：把未参与账号一键全部置为参与（单次确认语义由
              产品层承担；P19-B2 参与集默认全参与修复的显式入口）。 */}
          <div className="workbuddy-card__actions">
            <button
              type="button"
              className="btn"
              onClick={onParticipateAll}
              disabled={busy || pendingCount === 0}
              data-testid={`${testIdPrefix}-auto-checkin-participate-all`}
              title={pendingCount === 0 ? "全部账号均已参与" : `把 ${pendingCount} 个未参与账号全部置为参与`}
            >
              {participateAllLabel}
              {pendingCount > 0 ? `（${pendingCount}）` : ""}
            </button>
          </div>
          <ul className="account-list" data-testid={`${testIdPrefix}-auto-checkin-accounts`}>
            {accounts.map((account) => (
              <li key={account.profileId}>
                <div className="account-list__row">
                  <div className="account-item__main">
                    <span className="account-card__avatar" aria-hidden="true">
                      {(account.displayName || "W").trim().charAt(0).toUpperCase()}
                    </span>
                    <div className="account-item__id">
                      <div className="account-item__name-row">
                        <strong className="account-item__name">
                          {account.displayName || "WorkBuddy 账号"}
                        </strong>
                      </div>
                    </div>
                  </div>
                  <div className="account-item__side">
                    <label className="account-center__target-field">
                      <span>参与自动签到</span>
                      <input
                        type="checkbox"
                        checked={account.participating}
                        disabled={busy}
                        onChange={(event) => onParticipationChange(account.profileId, event.target.checked)}
                        data-testid={`${testIdPrefix}-auto-checkin-account-${account.profileId}`}
                      />
                    </label>
                  </div>
                </div>
              </li>
            ))}
          </ul>
        </>
      )}
    </div>
  );
}
