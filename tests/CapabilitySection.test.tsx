import { describe, expect, it } from "vitest";
import { render, screen } from "@testing-library/react";
import { CapabilitySection } from "../src/components/CapabilitySection";

// ============================================================================
// CapabilitySection（P11-6 / ADR-0032 决策 2）：区块能力三态渲染。
// ready 渲染真实内容；pending 渲染诚实说明卡；absent 渲染「无此能力」卡。
// ============================================================================

describe("CapabilitySection 三态渲染", () => {
  it("ready：直接渲染 children，不渲染说明卡", () => {
    render(
      <CapabilitySection state="ready" product="work_cn">
        <div data-testid="real-content">真实区块内容</div>
      </CapabilitySection>,
    );
    expect(screen.getByTestId("real-content")).toBeInTheDocument();
    expect(screen.queryByTestId("capability-ready")).not.toBeInTheDocument();
    expect(screen.queryByTestId("capability-pending")).not.toBeInTheDocument();
    expect(screen.queryByTestId("capability-absent")).not.toBeInTheDocument();
  });

  it("pending：渲染「该能力适配中」说明卡，不渲染 children", () => {
    render(
      <CapabilitySection state="pending" product="trae_cn" note="协议端点已验证，实装待验收。">
        <div data-testid="real-content">不应出现的真实内容</div>
      </CapabilitySection>,
    );
    const card = screen.getByTestId("capability-pending");
    expect(card).toBeInTheDocument();
    expect(card).toHaveTextContent("该能力适配中");
    expect(card).toHaveTextContent("协议端点已验证，实装待验收。");
    expect(screen.queryByTestId("real-content")).not.toBeInTheDocument();
    // 数据属性承载产品与状态（每产品每区块三态断言的选择器锚点）。
    expect(card).toHaveAttribute("data-product", "trae_cn");
    expect(card).toHaveAttribute("data-state", "pending");
  });

  it("pending：标题可用调用方覆盖（签到区块用「签到能力适配中」避免与默认标题重复）", () => {
    render(
      <CapabilitySection
        state="pending"
        product="trae_cn"
        title="签到能力适配中"
        note="协议端点已验证，实装待验收。"
      />,
    );
    const card = screen.getByTestId("capability-pending");
    expect(card).toHaveTextContent("签到能力适配中");
    expect(card).not.toHaveTextContent("该能力适配中");
    expect(card).toHaveTextContent("协议端点已验证，实装待验收。");
  });

  it("absent：渲染「该产品无此能力」卡", () => {
    render(<CapabilitySection state="absent" product="trae_cn" />);
    const card = screen.getByTestId("capability-absent");
    expect(card).toBeInTheDocument();
    expect(card).toHaveTextContent("该产品无此能力");
    expect(card).toHaveAttribute("data-product", "trae_cn");
    expect(card).toHaveAttribute("data-state", "absent");
  });
});
