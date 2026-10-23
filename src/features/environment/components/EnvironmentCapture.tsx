import type { EnvActionView } from "../config";
import { EnvActionButton } from "./EnvironmentCards";

/**
 * 账号捕获块（P4d-8a-2，纯展示）：旧页 L625-646 同形——分组标题 + 捕获入口 + 结果提示行。
 * 文案全部来自 builder 的视图模型（entry.label / notice.text 已按 ENV_TEXTS 填好）；
 * 本文件不自创文案、不判断 productId、不读数据、不发命令（动作只回调 onAction(actionId)）。
 */

/** 提示行锚（旧页 L640 逐字）：成功与失败同锚，语气由 tone 决定。 */
const NOTICE_TEST_ID = "env-capture-account-notice";

export interface EnvironmentCaptureProps {
  /** 区块锚（builder 给 env-capture-account-group）。 */
  readonly testId: string;
  readonly title: string;
  /** 捕获入口（label 与 busyLabel 由 builder 按忙碌位切换）。 */
  readonly entry: EnvActionView;
  /** 结果提示：null = 尚未捕获；tone=ok 正常色、danger 走危险色（旧页内联 style 同口径）。 */
  readonly notice: { readonly tone: "ok" | "danger"; readonly text: string } | null;
  /** 执行中的动作 id（页面按忙碌位注入）。 */
  readonly busy?: readonly string[];
  readonly onAction: (actionId: string) => void;
}

/** 捕获块：失败提示用 --danger token 着色（旧页内联色值的等价替换，不新增 CSS）。 */
export function EnvironmentCapture({ testId, title, entry, notice, busy = [], onAction }: EnvironmentCaptureProps) {
  return (
    <div className="env-checkup__group" data-testid={testId}>
      <div className="env-checkup__group-title">{title}</div>
      <div className="env-checkup__action">
        <EnvActionButton action={entry} busy={busy.includes(entry.id)} onClick={() => onAction(entry.id)} />
      </div>
      {notice !== null && (
        <p
          className="env-checkup__note"
          data-testid={NOTICE_TEST_ID}
          role={notice.tone === "danger" ? "alert" : undefined}
          style={notice.tone === "danger" ? { color: "var(--danger)" } : undefined}
        >
          {notice.text}
        </p>
      )}
    </div>
  );
}
