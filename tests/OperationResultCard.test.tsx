import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { OperationResultCard } from "../src/components/OperationResultCard";

// G9 健康检测/刷新额度共用结果卡：首行结论 + 只列异常账号（名字+一句原因），
// 正常账号不占空间。

describe("OperationResultCard 结果卡", () => {
  it("全部正常：只显示结论行，不渲染异常清单", () => {
    render(<OperationResultCard title="健康检测" okCount={12} issues={[]} />);
    expect(screen.getByText("健康检测完成：12 个账号正常。")).toBeInTheDocument();
    expect(screen.queryByRole("list")).not.toBeInTheDocument();
  });

  it("存在异常：结论行带需处理数，并逐行列出名字与原因", () => {
    render(
      <OperationResultCard
        title="额度刷新"
        okCount={2}
        issues={[
          { key: "p1", name: "账号甲", reason: "登录凭据已失效，请重新登录" },
          { key: "p2", name: "账号乙", reason: "网络或服务暂时不可用，请稍后重试。" },
        ]}
      />,
    );
    expect(screen.getByText("额度刷新完成：2 个账号正常，2 个需要处理。")).toBeInTheDocument();
    expect(screen.getByText("账号甲")).toBeInTheDocument();
    expect(screen.getByText("登录凭据已失效，请重新登录")).toBeInTheDocument();
    expect(screen.getByText("账号乙")).toBeInTheDocument();
    expect(screen.getByText("网络或服务暂时不可用，请稍后重试。")).toBeInTheDocument();
  });

  it("健康检测与刷新额度共用同一卡片形态（title 只影响结论行前缀）", () => {
    const { unmount } = render(<OperationResultCard title="健康检测" okCount={1} issues={[]} />);
    expect(screen.getByText("健康检测完成：1 个账号正常。")).toBeInTheDocument();
    unmount();
    render(<OperationResultCard title="额度刷新" okCount={1} issues={[]} />);
    expect(screen.getByText("额度刷新完成：1 个账号正常。")).toBeInTheDocument();
  });
});
