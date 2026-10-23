import { HeartPulse } from "lucide-react";
import { ENV_TEXTS, type EnvActionView, type EnvCheckupAccountRowView, type EnvCheckupView } from "../config";
import { EnvActionButton } from "./EnvironmentCards";

/**
 * 体检卡（P4d-7b，纯展示）：EnvCheckupView → 旧页体检区块结构（主库 L471-621 / 副环境 L1031-1094 同构）。
 * 状态词、逐账号行、滞留/空项目规模句、自检四项与折叠说明的文案全部来自 builder 视图模型与 ENV_TEXTS；
 * 本文件不自创文案、不判断 productId、不读数据、不发命令（动作只回调 onAction(actionId)）。
 */

/** 分组锚（旧页 L479/L533、L1039/L1049）：主库 env-checkup → env-checkup-login/-content，副环境按 env_id 追加后缀。 */
function groupTestIds(block: string): { readonly login: string; readonly content: string } {
  const suffix = block.slice("env-checkup".length);
  return { login: `env-checkup-login${suffix}`, content: `env-checkup-content${suffix}` };
}
/** 视图模型未给的三处静态锚（旧页 L507/L561/L611 逐字）。 */
const DEEP_TEST_ID = "env-self-check-deep";
const EMPTY_SHELL_TEST_ID = "env-empty-shell-list";
const CONTENT_OK_TEST_ID = "env-checkup-content-ok";
/** 清理入口 id 前缀：旧页把清理排在孤儿行之前、归入排在之后（L576-608），故按动作 id 分两段渲染。 */
const CLEANUP_PREFIX = "env-cleanup";
/** 深度检查项 id（自检四项里的第四项；折叠说明只复述它的摘要）。 */
const DEEP_CHECK_ID = "deep";
/** 降级态判定：库内容读不到时 builder 只给一句说明、无行无动作无正常态（旧页 L615-619 用 note 而非 notice 块）。 */
const isDegraded = (content: EnvCheckupView["content"]): boolean =>
  content.staleAccounts.length === 0 && content.emptyShells.length === 0 && content.actions.length === 0 &&
  content.orphanNote === null && content.okNote === null;
/** 逐账号诊断行（旧页 L550-557 / L563-570）：账号名是主信息，user_id 只进 key 与悬浮提示。 */
function AccountRow({ row }: { row: EnvCheckupAccountRowView }) {
  return (
    <li title={ENV_TEXTS.accountIdTitle.replace("{id}", row.id)}>
      <span className="env-checkup__name">{row.name}</span>
      <span className="env-checkup__meta">{row.meta}</span>
    </li>
  );
}
/** 体检动作（清理 / 归入 / 纠正）：kind 决定按钮样式，忙碌与禁用由页面注入的 busy 决定。 */
function CheckupAction({ action, busy, onAction }: {
  action: EnvActionView; busy: readonly string[]; onAction: (actionId: string) => void;
}) {
  return (
    <div className="env-checkup__action">
      <EnvActionButton action={action} busy={busy.includes(action.id)} onClick={() => onAction(action.id)} />
    </div>
  );
}

export interface EnvironmentCheckupProps {
  /** 体检区块视图（主库与副环境同构；builder 无差异时不产出，页面据此不渲染）。 */
  checkup: EnvCheckupView;
  /** 执行中的动作 id（页面按忙碌位注入；缺省=无动作执行中）。 */
  busy?: readonly string[];
  onAction: (actionId: string) => void;
}

/** 体检卡：状态头 + 登录健康分组（自检四项/降级句）+ 库内容分组（规模句/行/动作/孤儿）。 */
export function EnvironmentCheckup({ checkup, busy = [], onAction }: EnvironmentCheckupProps) {
  const { login, content } = checkup;
  const ids = groupTestIds(checkup.testId);
  const cleanupActions = content.actions.filter((action) => action.id.startsWith(CLEANUP_PREFIX));
  const followActions = content.actions.filter((action) => !action.id.startsWith(CLEANUP_PREFIX));
  return (
    <div className="env-checkup" data-testid={checkup.testId}>
      <div className="env-checkup__head">
        <HeartPulse size={16} aria-hidden="true" />
        <div>{checkup.title}</div>
      </div>

      {/* 登录健康分组：自检四项 + 实际登录差异 / 损坏引导；读不到时整组降级为一句说明。 */}
      <div className="env-checkup__group" data-testid={ids.login}>
        <div className="env-checkup__group-title">{login.title}</div>
        {login.unreadableNote !== null ? (
          <p className="env-checkup__note">{login.unreadableNote}</p>
        ) : (
          <>
            {login.notes.map((note) => (
              <p className="env-self-check__note" key={note}>{note}</p>
            ))}
            {login.items.length > 0 && (
              <>
                <ul className="env-self-check__list">
                  {login.items.map((item) => (
                    <li key={item.id} className={`env-self-check__item env-self-check__item--${item.tone}`}>
                      <span className="env-self-check__item-name">{item.name}</span>
                      <span className="env-self-check__item-status">{item.status}</span>
                      <span className="env-self-check__item-summary">{item.summary}</span>
                    </li>
                  ))}
                </ul>
                <details className="env-self-check__details" data-testid={DEEP_TEST_ID}>
                  <summary>{ENV_TEXTS.deepSummary}</summary>
                  <p>{login.items.find((item) => item.id === DEEP_CHECK_ID)?.summary ?? ENV_TEXTS.deepEmpty}</p>
                  <p>{ENV_TEXTS.deepHint}</p>
                </details>
              </>
            )}
            {login.repair !== null && (
              <div className="env-self-check__repair">
                <EnvActionButton action={login.repair} busy={busy.includes(login.repair.id)} onClick={() => onAction(login.repair!.id)} />
              </div>
            )}
          </>
        )}
      </div>

      {/* 库内容分组：规模句 → 滞留行 → 空项目行 → 清理 → 孤儿说明 → 归入；正常态一句提示。 */}
      <div className="env-checkup__group" data-testid={ids.content}>
        <div className="env-checkup__group-title">{content.title}</div>
        {content.okNote !== null ? (
          <p className="env-checkup__note" data-testid={CONTENT_OK_TEST_ID}>{content.okNote}</p>
        ) : isDegraded(content) ? (
          content.noticeLines.map((line) => <p className="env-checkup__note" key={line}>{line}</p>)
        ) : (
          <>
            {content.noticeLines.length > 0 && (
              <div className="env-checkup__notice">
                {content.noticeLines.map((line) => <div key={line}>{line}</div>)}
              </div>
            )}
            {content.staleAccounts.length > 0 && (
              <ul className="env-checkup__list">
                {content.staleAccounts.map((row) => <AccountRow key={row.id} row={row} />)}
              </ul>
            )}
            {content.emptyShells.length > 0 && (
              <ul className="env-checkup__list" data-testid={EMPTY_SHELL_TEST_ID}>
                {content.emptyShells.map((row) => <AccountRow key={row.id} row={row} />)}
              </ul>
            )}
            {cleanupActions.map((action) => <CheckupAction key={action.id} action={action} busy={busy} onAction={onAction} />)}
            {content.orphanNote !== null && <p className="env-checkup__note">{content.orphanNote}</p>}
            {followActions.map((action) => <CheckupAction key={action.id} action={action} busy={busy} onAction={onAction} />)}
          </>
        )}
      </div>
    </div>
  );
}
