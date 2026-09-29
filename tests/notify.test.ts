import { beforeEach, describe, expect, it, vi } from "vitest";
import { render } from "@testing-library/react";
import type { ReactElement, ReactNode } from "react";

// mock sonner：具名导出 toast（sonner 2.x 无默认导出）为可调用函数，
// 并挂 success/error/warning 三个语义方法；notify.info 按约定走 toast 本体。
vi.mock("sonner", () => {
  const toastFn = Object.assign(vi.fn(), {
    success: vi.fn(),
    error: vi.fn(),
    warning: vi.fn(),
    info: vi.fn(),
  });
  // Toaster 仅供 App 挂载，单测无需真实渲染。
  return { toast: toastFn, Toaster: () => null };
});

import { toast } from "sonner";
import { notify } from "../src/platform/notify";

const toastMock = vi.mocked(toast);

/** 取最近一次 toast 调用的 (标题, 选项)，供断言用。 */
function lastToastCall(): [string, { description?: ReactNode; duration?: number }] {
  const call = toastMock.mock.calls.at(-1);
  expect(call).toBeDefined();
  return call as unknown as [string, { description?: ReactNode; duration?: number }];
}

describe("notify 统一通知适配层", () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it("success 走 sonner 的 success 且 4 秒自动消失", () => {
    notify.success("已保存");
    expect(toastMock.success).toHaveBeenCalledTimes(1);
    expect(toastMock.success).toHaveBeenCalledWith(
      "已保存",
      expect.objectContaining({ duration: 4000 }),
    );
  });

  it("success 透传描述内容", () => {
    notify.success("已保存", "2 个账号已更新");
    expect(toastMock.success).toHaveBeenCalledWith(
      "已保存",
      expect.objectContaining({ duration: 4000, description: "2 个账号已更新" }),
    );
  });

  it("error 走 sonner 的 error 且 6 秒自动消失，并透传描述", () => {
    notify.error("签到失败", "登录已失效");
    expect(toastMock.error).toHaveBeenCalledTimes(1);
    expect(toastMock.error).toHaveBeenCalledWith(
      "签到失败",
      expect.objectContaining({ duration: 6000, description: "登录已失效" }),
    );
  });

  it("info 走 toast 本体且 4 秒自动消失", () => {
    notify.info("开始同步");
    expect(toastMock).toHaveBeenCalledTimes(1);
    const [message, options] = lastToastCall();
    expect(message).toBe("开始同步");
    expect(options.duration).toBe(4000);
  });

  it("warning 走 sonner 的 warning 且 5 秒自动消失", () => {
    notify.warning("额度即将用尽");
    expect(toastMock.warning).toHaveBeenCalledTimes(1);
    expect(toastMock.warning).toHaveBeenCalledWith(
      "额度即将用尽",
      expect.objectContaining({ duration: 5000 }),
    );
  });

  it("summary 全部成功：只报成功数，4 秒自动消失", () => {
    notify.summary("一键签到完成", [], 5);
    expect(toastMock).toHaveBeenCalledTimes(1);
    const [message, options] = lastToastCall();
    expect(message).toBe("一键签到完成");
    expect(options.duration).toBe(4000);
    const { container } = render(options.description as ReactElement);
    expect(container.textContent).toContain("成功 5 项");
    // 无异常时不应出现异常清单结构
    expect(container.querySelector("ul")).toBeNull();
  });

  it("summary 带异常：成功数与逐行「名称：原因」齐全，6 秒供阅读", () => {
    notify.summary(
      "一键签到",
      [
        { displayName: "工作号 A", reason: "登录已失效" },
        { displayName: "工作号 B", reason: "网络超时" },
      ],
      3,
    );
    expect(toastMock).toHaveBeenCalledTimes(1);
    const [message, options] = lastToastCall();
    expect(message).toBe("一键签到");
    expect(options.duration).toBe(6000);
    const { container } = render(options.description as ReactElement);
    expect(container.textContent).toContain("成功 3 项");
    expect(container.textContent).toContain("工作号 A：登录已失效");
    expect(container.textContent).toContain("工作号 B：网络超时");
  });
});
