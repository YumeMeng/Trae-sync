import { createElement, type ReactNode } from "react";
import { CircleCheck, CircleX, Info, TriangleAlert } from "lucide-react";
// sonner 2.x 只有具名导出（无默认导出）
import { toast, Toaster } from "sonner";

/* ============================================================
 * 统一通知适配层（P11-1 切片①）
 *
 * 「唯一入口」约束：本项目规定 sonner 是被适配的通知库，任何组件
 * 不得直接 import sonner（含 toast 与 Toaster），一律经由本模块
 * 发通知、取得 Toaster。sonner 的类型也不对外导出，公开 API 只用
 * React 自身类型，未来更换通知实现时组件层零改动。
 * ============================================================ */

/** 批量操作摘要里的异常条目（只暴露用户可读字段，不含错误码原文）。 */
export interface NotifyIssue {
  readonly displayName: string;
  readonly reason: string;
}

/**
 * 自动消失时长约定（毫秒）：
 * 成功/中性 4s；警告 5s；失败与带异常的摘要 6s（异常清单需要读完）。
 */
const DURATION_NORMAL = 4000;
const DURATION_WARNING = 5000;
const DURATION_ERROR = 6000;

/** 通知语义类型，决定图标与样式类名。 */
type NotifyKind = "success" | "error" | "info" | "warning";

/** 各语义对应的 lucide 图标（描边继承 currentColor，颜色由 CSS 按语义给出）。 */
const KIND_ICONS = {
  success: CircleCheck,
  error: CircleX,
  info: Info,
  warning: TriangleAlert,
} as const;

/** 按语义组装 sonner 选项：时长、图标与 sonner- 前缀样式类名。 */
function optionsOf(
  kind: NotifyKind,
  duration: number,
  description?: ReactNode,
) {
  return {
    duration,
    description,
    icon: createElement(KIND_ICONS[kind], { size: 16, "aria-hidden": true }),
    classNames: {
      toast: `sonner-toast sonner-toast--${kind}`,
      title: "sonner-toast__title",
      description: "sonner-toast__description",
    },
  };
}

/** 批量结果摘要的富内容描述：成功数 + 异常逐行「名称：原因」。 */
function summaryDescription(issues: readonly NotifyIssue[], okCount: number): ReactNode {
  return createElement(
    "div",
    { className: "sonner-summary" },
    createElement("span", { className: "sonner-summary__ok" }, `成功 ${okCount} 项`),
    issues.length > 0
      ? createElement(
          "ul",
          { className: "sonner-summary__issues" },
          issues.map((issue, index) =>
            createElement(
              "li",
              { key: `${issue.displayName}-${index}`, className: "sonner-summary__issue" },
              `${issue.displayName}：${issue.reason}`,
            ),
          ),
        )
      : null,
  );
}

/** 统一通知门面：页面与组件只允许通过 notify.* 发通知。 */
export const notify = {
  /** 成功提示，4 秒自动消失。 */
  success(message: string, description?: ReactNode) {
    toast.success(message, optionsOf("success", DURATION_NORMAL, description));
  },
  /** 失败/错误提示，6 秒自动消失。 */
  error(message: string, description?: ReactNode) {
    toast.error(message, optionsOf("error", DURATION_ERROR, description));
  },
  /** 中性信息提示，4 秒自动消失。 */
  info(message: string, description?: ReactNode) {
    toast(message, optionsOf("info", DURATION_NORMAL, description));
  },
  /** 警告提示，5 秒自动消失。 */
  warning(message: string, description?: ReactNode) {
    toast.warning(message, optionsOf("warning", DURATION_WARNING, description));
  },
  /**
   * 批量操作结果摘要：标题 + 成功数 + 异常清单。
   * 无异常按成功档 4s；带异常按警告琥珀表达（需要动手），但时长取
   * 错误档 6s，保证异常清单读完。
   */
  summary(title: string, issues: readonly NotifyIssue[], okCount: number) {
    const kind: NotifyKind = issues.length > 0 ? "warning" : "success";
    const duration = issues.length > 0 ? DURATION_ERROR : DURATION_NORMAL;
    toast(title, optionsOf(kind, duration, summaryDescription(issues, okCount)));
  },
};

/** 通知容器：App 挂载用；经此再导出保证 sonner 只在适配层被引用。 */
export { Toaster };
