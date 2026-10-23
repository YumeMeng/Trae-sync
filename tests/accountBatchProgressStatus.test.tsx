import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { AccountBatchProgressStatus } from "../src/features/accounts/components/AccountBatchProgressStatus";

// P4c-9c（K-05 回归护栏）：批量进度逐账号文案的渲染层单测。
// 文案逐字沿用旧 AccountCenterTrae（旧 L1141 / L1150 / L1160）：
// `正在处理 ${completed}/${total}：${name}`；无进度时不渲染任何节点。

/** 无忙碌位、无进度的最小入参。 */
const idleBusy = { credits: false, credentials: false, health: false } as const;

describe("AccountBatchProgressStatus（批量进度逐账号文案）", () => {
  it("额度链在跑：上屏「正在处理 1/2：账号名」", () => {
    render(
      <AccountBatchProgressStatus
        busy={{ ...idleBusy, credits: true }}
        progress={{ credits_refresh: { completed: 1, total: 2, name: "额度账号甲" } }}
      />,
    );
    expect(screen.getByText("正在处理 1/2：额度账号甲")).toBeInTheDocument();
  });

  it("凭据链在跑：上屏逐账号文案（与额度链同格式）", () => {
    render(
      <AccountBatchProgressStatus
        busy={{ ...idleBusy, credentials: true }}
        progress={{ credential_refresh: { completed: 2, total: 3, name: "凭据账号乙" } }}
      />,
    );
    expect(screen.getByText("正在处理 2/3：凭据账号乙")).toBeInTheDocument();
  });

  it("健康检测：优先健康链事件，缺失时回退额度链事件（旧 healthProgressShown 同法）", () => {
    const { rerender } = render(
      <AccountBatchProgressStatus
        busy={{ ...idleBusy, health: true }}
        progress={{ health_check: { completed: 1, total: 2, name: "检测账号甲" } }}
      />,
    );
    expect(screen.getByText("正在处理 1/2：检测账号甲")).toBeInTheDocument();
    rerender(
      <AccountBatchProgressStatus
        busy={{ ...idleBusy, health: true }}
        progress={{ credits_refresh: { completed: 1, total: 1, name: "网络步骤账号" } }}
      />,
    );
    expect(screen.getByText("正在处理 1/1：网络步骤账号")).toBeInTheDocument();
  });

  it("无进度：不渲染任何文案（未收到首批事件前不占版面）", () => {
    const { container } = render(
      <AccountBatchProgressStatus busy={{ ...idleBusy, credits: true }} progress={{}} />,
    );
    expect(container.textContent).toBe("");
    expect(screen.queryByRole("status")).not.toBeInTheDocument();
  });

  it("忙碌位归零：即使进度未清也不上屏（旧实现由忙碌位包住状态区）", () => {
    const { container } = render(
      <AccountBatchProgressStatus
        busy={idleBusy}
        progress={{ credits_refresh: { completed: 1, total: 2, name: "收尾账号" } }}
      />,
    );
    expect(container.textContent).toBe("");
  });
});
