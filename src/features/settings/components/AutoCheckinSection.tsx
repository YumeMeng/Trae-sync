import { CalendarClock, RefreshCw } from "lucide-react";
import { Badge, type BadgeTone } from "../../../ui/Badge";
import type { AutoCheckinView, SettingsActionView, SettingsTone } from "../config";

/**
 * 自动签到区（P4f-4a，纯展示）：分区标题 + 总开关 + 触发时间 + 今日台账 + 参与清单（WB 独有）。
 * 旧实现对照 TraeSettingsVariant.tsx L269-316（TRAE 内联三件：开关/时间/台账）与
 * shared/checkin/AutoCheckinSettingsCard.tsx L44-131（WB 共享卡：同三件 + 参与清单 + 全部参与）。
 * 两产品调度语义一致，差异只有「参与清单是否存在」「首读占位句」「分区徽章」——全部由视图模型表达。
 * 文案、锚点与禁用原因一律取自 AutoCheckinView；本层不判断产品、不读数据、不发命令。
 */

/** 色档换算：SettingsTone 的 ok 落到徽章的 accent（Badge 无「成功绿」，正常态用强调色）。 */
const badgeTone = (tone: SettingsTone): BadgeTone => (tone === "ok" ? "accent" : tone);

/** 动作按钮（参与清单的「全部参与」用）：disabledReason 非空即禁用并给悬浮说明。 */
function ActionButton({ action, busy, disabled, disabledReason, onClick }: {
  readonly action: SettingsActionView; readonly busy: boolean; readonly disabled: boolean;
  readonly disabledReason: string | null; readonly onClick: () => void;
}) {
  return (
    <button className="btn" type="button" onClick={onClick}
      disabled={disabled || busy || action.disabledReason !== null}
      title={action.disabledReason ?? disabledReason ?? action.hint ?? undefined}
      aria-busy={busy || undefined} data-testid={action.testId}>
      {busy && <RefreshCw size={15} className="icon-spin" aria-hidden="true" />}
      {busy ? action.busyLabel ?? action.label : action.label}
    </button>
  );
}

export interface AutoCheckinSectionProps {
  /** 签到区视图（builder 产出；TRAE 无参与清单、WB 有清单与批量入口）。 */
  readonly view: AutoCheckinView;
  /** 外部禁用（页面门禁/只读）：与 view.busy 合并，原因进悬浮说明（旧实现只有 busy 一种禁用源）。 */
  readonly disabled?: boolean;
  readonly disabledReason?: string | null;
  /** 总开关 / 触发时间变更（旧 L291/L305、CARD L56/L70）。 */
  onToggle: (enabled: boolean) => void;
  onTimeChange: (dailyTimeHhmm: string) => void;
  /** 逐账号参与开关（WB 独有；TRAE 该清单为 null，不渲染）。 */
  onParticipationChange: (profileId: string, participating: boolean) => void;
  /** 「全部参与（N）」批量入口（WB 独有；视图无该动作时本层不渲染）。 */
  onParticipateAll: () => void;
}

/** 签到区：标题（徽章可缺省）+ 开关/时间/台账 + 参与清单；首读中只给占位句（旧 L279/L314 三元）。 */
export function AutoCheckinSection({
  view, disabled = false, disabledReason = null, onToggle, onTimeChange, onParticipationChange, onParticipateAll,
}: AutoCheckinSectionProps) {
  /** 禁用合并：执行中与外部禁用同权；原因优先取动作/分区给出的那句。 */
  const locked = view.busy || disabled;
  const titleOf = (fallback: string | null): string | undefined => disabledReason ?? fallback ?? undefined;
  const participation = view.participation;
  /** 行锚：旧实现为「清单锚单数化 + profileId」（CARD L122）；builder 只给清单前缀，故此处派生。 */
  const rowTestId = (profileId: string): string => participation === null ? profileId
    : `${participation.testId.endsWith("accounts") ? participation.testId.slice(0, -1) : participation.testId}-${profileId}`;
  return (
    <>
      <div className="settings-panel__section-heading">
        <div>
          <span className="workbench__eyebrow">{view.eyebrow}</span>
          <h3>{view.title}</h3>
        </div>
        {/* WB 分区标题无状态徽章（视图给空串），故空串即不占位；不新造。 */}
        {view.badge !== "" && <Badge tone={badgeTone(view.badgeTone)}>{view.badge}</Badge>}
      </div>
      {view.loadingNote !== null ? (
        <p className="account-center__meta">{view.loadingNote}</p>
      ) : (
        <>
          <div className="account-center__key-grid" data-testid={view.testId}>
            <label className="settings-panel__toggle-row" data-testid={view.toggleTestId} title={titleOf(view.toggleTitle)}>
              <input type="checkbox" checked={view.enabled} disabled={locked}
                onChange={(event) => onToggle(event.target.checked)} />
              <span className="account-center__profile-copy">
                <strong>{view.toggleStrong}</strong>
                <span>{view.toggleSummary}</span>
              </span>
            </label>
            <label className="account-center__target-field">
              <span>{view.timeLabel}</span>
              <input type="time" value={view.dailyTimeHhmm} disabled={locked || !view.enabled}
                onChange={(event) => {
                  if (event.target.value) onTimeChange(event.target.value);
                }}
                data-testid={view.timeTestId} />
            </label>
            {/* 台账：TRAE 恒有一句（含「今日尚未发起」），WB 无台账时不渲染该行（视图给空串）。 */}
            {view.ledgerText !== "" && (
              <span className="account-center__meta" data-testid={view.ledgerTestId}>
                <CalendarClock size={14} aria-hidden="true" />{view.ledgerText}
              </span>
            )}
          </div>
          {participation !== null && (
            <>
              {participation.participateAll !== null && (
                <div className="workbuddy-card__actions">
                  <ActionButton action={participation.participateAll} busy={view.busy} disabled={disabled}
                    disabledReason={disabledReason} onClick={onParticipateAll} />
                </div>
              )}
              {/* 清单锚只落一处：builder 的 testId 与 listTestId 同值（旧实现该块只有一个容器锚）。 */}
              <ul className="account-list" data-testid={participation.listTestId}>
                {participation.rows.map((row) => (
                  <li key={row.profileId}>
                    <div className="account-list__row">
                      <div className="account-item__main">
                        <span className="account-card__avatar" aria-hidden="true">
                          {(row.displayName || participation.defaultName).trim().charAt(0).toUpperCase()}
                        </span>
                        <div className="account-item__id">
                          <div className="account-item__name-row">
                            <strong className="account-item__name">{row.displayName || participation.defaultName}</strong>
                          </div>
                        </div>
                      </div>
                      <div className="account-item__side">
                        <label className="account-center__target-field">
                          <span>{participation.label}</span>
                          <input type="checkbox" checked={row.participating} disabled={locked}
                            onChange={(event) => onParticipationChange(row.profileId, event.target.checked)}
                            data-testid={rowTestId(row.profileId)} />
                        </label>
                      </div>
                    </div>
                  </li>
                ))}
              </ul>
            </>
          )}
        </>
      )}
    </>
  );
}
