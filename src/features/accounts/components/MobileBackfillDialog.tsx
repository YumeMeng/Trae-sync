import { useState } from "react";

/**
 * 手机号补录弹层（P4c-6b-2e-2，G11）：登录完成后后端回执没有完整手机号时引导补录。
 *
 * 形态、文案与 testid 逐字沿用旧 AccountCenterTrae L814-871（mobile-backfill-dialog / -input /
 * -error / -skip / -save）：可跳过、Enter 保存、空输入不可保存、失败保留弹层与输入并就地提示。
 * 本文件只渲染、只持草稿；保存出口是 onSave（由 useAccountLoginFlow 注入 set_account_mobile）。
 */
export interface MobileBackfillDialogProps {
  /** 目标账号展示名（备注名优先）。 */
  readonly accountName: string;
  /** 服务端脱敏号；空串 = 暂未采集（文案给「可先跳过」）。 */
  readonly masked: string;
  readonly busy: boolean;
  /** 失败提示（映射后的人话）；null = 无错误。 */
  readonly error: string | null;
  /** 保存：value 已 trim 且非空。 */
  readonly onSave: (value: string) => void;
  readonly onSkip: () => void;
}

export function MobileBackfillDialog({ accountName, masked, busy, error, onSave, onSkip }: MobileBackfillDialogProps) {
  const [draft, setDraft] = useState("");
  const save = () => { if (!busy && draft.trim() !== "") onSave(draft); };
  return (
    <div className="switch-veil" role="presentation">
      <div className="mobile-prompt" role="dialog" aria-modal="true" aria-labelledby="mobile-prompt-title" data-testid="mobile-backfill-dialog">
        <h2 id="mobile-prompt-title" className="mobile-prompt__title">补全手机号</h2>
        <p className="mobile-prompt__lead">
          为账号「{accountName}」补录完整手机号，多账号时更易区分。可跳过，之后随时能在账号详情页补录。
        </p>
        <p className="mobile-prompt__masked">
          服务端记录：{masked || "暂未采集（可先跳过，刷新额度后自动补全脱敏号）"}
        </p>
        <input
          className="mobile-prompt__input"
          type="tel"
          inputMode="numeric"
          value={draft}
          maxLength={11}
          placeholder="请输入 11 位完整手机号"
          disabled={busy}
          data-testid="mobile-backfill-input"
          aria-label="完整手机号"
          onChange={(event) => setDraft(event.target.value.replace(/[^\d]/g, ""))}
          onKeyDown={(event) => { if (event.key === "Enter") { event.preventDefault(); save(); } }}
        />
        {error !== null && <p className="mobile-prompt__error" role="alert" data-testid="mobile-backfill-error">{error}</p>}
        <div className="mobile-prompt__actions">
          <button className="btn" type="button" disabled={busy} data-testid="mobile-backfill-skip" onClick={onSkip}>跳过</button>
          <button className="btn btn--primary" type="button" disabled={busy || draft.trim() === ""} data-testid="mobile-backfill-save" onClick={save}>
            {busy ? "保存中…" : "保存"}
          </button>
        </div>
      </div>
    </div>
  );
}
