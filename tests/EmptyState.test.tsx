import { describe, expect, it } from "vitest";
import { render, screen } from "@testing-library/react";
import { Layers } from "lucide-react";
import { EmptyState } from "../src/components/shared/EmptyState";

// ============================================================================
// EmptyState（P11-6 任务 4）：各页自持空态的统一轻量组件。
// 图标 + 标题 + 说明 + 可选动作；页面保留原类名钩子以兼容既有选择器。
// ============================================================================

describe("EmptyState 统一空态组件", () => {
  it("渲染说明文本，缺省图标/标题/动作不占位", () => {
    render(<EmptyState description="还没有对话记录" />);
    expect(screen.getByText("还没有对话记录")).toBeInTheDocument();
    expect(document.querySelector(".empty-state__icon")).not.toBeInTheDocument();
    expect(document.querySelector(".empty-state__title")).not.toBeInTheDocument();
    expect(document.querySelector(".empty-state__action")).not.toBeInTheDocument();
  });

  it("渲染图标、标题、说明与动作，且根节点保留页面原类名钩子", () => {
    render(
      <EmptyState
        className="history-empty"
        testId="history-guide"
        icon={<Layers size={26} />}
        title="主库还没有对话数据。"
        description="到环境页启动主库并在 TRAE 内登录一次。"
        action={<button type="button">去环境页启动</button>}
      />,
    );
    const root = screen.getByTestId("history-guide");
    expect(root).toHaveClass("empty-state", "history-empty");
    expect(document.querySelector(".empty-state__icon")).toBeInTheDocument();
    expect(screen.getByText("主库还没有对话数据。")).toBeInTheDocument();
    expect(screen.getByText("到环境页启动主库并在 TRAE 内登录一次。")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "去环境页启动" })).toBeInTheDocument();
  });

  it("panel 形态附加玻璃卡修饰类（三态说明卡与区块级空态用）", () => {
    render(<EmptyState panel testId="panel-empty" title="该能力适配中" />);
    expect(screen.getByTestId("panel-empty")).toHaveClass("empty-state", "empty-state--panel");
  });

  it("role 透传给根节点（loading 态 role=status 兼容既有断言）", () => {
    render(<EmptyState role="status" description="正在读取账号信息…" />);
    expect(screen.getByRole("status")).toHaveTextContent("正在读取账号信息…");
  });
});
