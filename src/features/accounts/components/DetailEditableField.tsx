import { useState } from "react";
import type { DetailFieldEditView } from "../config";

/**
 * 行内编辑字段（P4c-6b-2e-2）：展示态点行进入编辑态 → 输入框 + 保存/取消。
 *
 * 交互逐字沿用旧实现（AccountDetail L265-288 备注名 / L334-350 手机号、WorkBuddyAccountDetail
 * L308-334）：草稿初值 = 档案原值（不是展示名）、占位 = 服务端名/脱敏号、保存空串 = 清除、
 * Enter 保存。**取消**与「保存失败保留编辑态」是新页补齐项：保存由外部返回成败，失败不回退草稿。
 * 本文件零命令、零产品判断、零 invoke：保存出口只有 onSave（由页面注入）。
 */
export interface DetailEditableFieldProps {
  /** 字段 id（= 旧 data-testid，dd 恒定锚点；输入框锚点 = id + "-input"）。 */
  readonly id: string;
  readonly label: string;
  /** 展示态文本（备注名=备注名优先、手机号=补录号优先/脱敏号）。 */
  readonly value: string;
  readonly edit: DetailFieldEditView;
  /** 正在保存该字段（按钮换「保存中…」并禁用两侧）。 */
  readonly busy: boolean;
  /** 保存：返回 true = 成功（退出编辑态）；false = 失败（保留编辑态与草稿）。 */
  readonly onSave: (value: string) => Promise<boolean>;
}

export function DetailEditableField({ id, label, value, edit, busy, onSave }: DetailEditableFieldProps) {
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState(edit.draft);
  // 草稿脏判定与旧实现同口径（trim 后比较），保存按钮据此禁用。
  const dirty = draft.trim() !== edit.draft.trim();
  const enter = () => { if (!busy) { setDraft(edit.draft); setEditing(true); } };
  const cancel = () => { if (!busy) { setDraft(edit.draft); setEditing(false); } };
  const save = () => {
    if (busy || !dirty) return;
    void onSave(draft.trim()).then((ok) => { if (ok) setEditing(false); });
  };
  return (
    <div className="account-detail__fact">
      <dt>{label}</dt>
      <dd data-testid={id} onClick={editing ? undefined : enter}>
        {editing ? (
          <div className="account-detail__mobile-row">
            <input
              type={edit.kind === "tel" ? "tel" : "text"}
              inputMode={edit.kind === "tel" ? "numeric" : undefined}
              aria-label={label}
              value={draft}
              placeholder={edit.placeholder}
              maxLength={edit.maxLength}
              disabled={busy}
              data-testid={`${id}-input`}
              onChange={(event) => setDraft(edit.kind === "tel" ? event.target.value.replace(/[^\d]/g, "") : event.target.value)}
              onKeyDown={(event) => {
                if (event.key === "Enter") { event.preventDefault(); save(); }
                else if (event.key === "Escape") { event.preventDefault(); cancel(); }
              }}
            />
            <button className="btn btn--primary" type="button" onClick={save} disabled={!dirty || busy} data-testid={`${id}-save`}>
              {busy ? "保存中…" : "保存"}
            </button>
            <button className="btn btn--quiet" type="button" onClick={cancel} disabled={busy} data-testid={`${id}-cancel`}>
              取消
            </button>
          </div>
        ) : (
          <span>{value}</span>
        )}
      </dd>
    </div>
  );
}
