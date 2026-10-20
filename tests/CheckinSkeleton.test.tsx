import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { CheckinActionsRow } from "../src/components/shared/checkin/CheckinActionsRow";
import { CheckinFlowRow } from "../src/components/shared/checkin/CheckinFlowRow";
import { CheckinSelectRow } from "../src/components/shared/checkin/CheckinRow";
import { outcomeLabelFor, flowResultTone, selectResultTone } from "../src/components/shared/checkin/OutcomeLabels";
import { productUi } from "../src/platform/productUiRegistry";
import { CheckinPageSkeleton } from "../src/components/shared/checkin/CheckinPageSkeleton";

const noop = () => undefined;

describe("CheckinActionsRow（P19-B2④ 共享批量按钮组）", () => {
  it("batch 形态：全签/补签计数与禁用口径 + 勾选子集非全量时出现「签到所选」", () => {
    const { rerender } = render(
      <CheckinActionsRow
        variant="batch"
        enabled={true}
        busy={false}
        poolCount={4}
        pendingCount={2}
        selectionCount={2}
        onRunAll={noop}
        onRunPending={noop}
        onRunSelected={noop}
        onCancel={noop}
      />,
    );
    expect(screen.getByTestId("checkin-run-all")).toHaveTextContent("一键全签（4）");
    expect(screen.getByTestId("checkin-run-pending")).toHaveTextContent("一键补签（2）");
    expect(screen.getByTestId("checkin-run-selected")).toHaveTextContent("签到所选（2）");

    // 全量勾选（4/4）：「签到所选」不出现（全签已覆盖）。
    rerender(
      <CheckinActionsRow
        variant="batch"
        enabled={true}
        busy={false}
        poolCount={4}
        pendingCount={2}
        selectionCount={4}
        onRunAll={noop}
        onRunPending={noop}
        onRunSelected={noop}
        onCancel={noop}
      />,
    );
    expect(screen.queryByTestId("checkin-run-selected")).not.toBeInTheDocument();
    // 执行中：主按钮文案翻转。
    rerender(
      <CheckinActionsRow
        variant="batch"
        enabled={true}
        busy={true}
        poolCount={4}
        pendingCount={2}
        selectionCount={0}
        onRunAll={noop}
        onRunPending={noop}
        onRunSelected={noop}
        onCancel={noop}
      />,
    );
    expect(screen.getByTestId("checkin-run-all")).toHaveTextContent("签到执行中…");
    expect(screen.getByRole("button", { name: "取消未开始任务" })).toBeEnabled();
  });

  it("select 形态（fixture）：全部签到（所选语义），未选时禁用", () => {
    const { rerender } = render(
      <CheckinActionsRow
        variant="select"
        enabled={true}
        busy={false}
        poolCount={3}
        pendingCount={0}
        selectionCount={0}
        onRunAll={noop}
        onRunPending={noop}
        onRunSelected={noop}
        onCancel={noop}
      />,
    );
    expect(screen.getByRole("button", { name: "全部签到（0）" })).toBeDisabled();
    rerender(
      <CheckinActionsRow
        variant="select"
        enabled={true}
        busy={false}
        poolCount={3}
        pendingCount={0}
        selectionCount={3}
        onRunAll={noop}
        onRunPending={noop}
        onRunSelected={noop}
        onCancel={noop}
      />,
    );
    expect(screen.getByRole("button", { name: "全部签到（3）" })).toBeEnabled();
  });
});

describe("OutcomeLabels（P19-B2④ per-product 结果文案表）", () => {
  it("TRAE 表：业务码人话 + 未知业务码原样透出（ADR-0019）", () => {
    expect(outcomeLabelFor("trae_cn", "failed", "business_9095")).toBe("该设备今日名额已用（9095），明日再试");
    expect(outcomeLabelFor("trae_cn", "failed", "business_9999")).toBe("服务端拒绝（9999）");
    expect(outcomeLabelFor("work_cn", "claimed")).toBe("签到成功");
  });

  it("未注册产品回退通用档 + 语气换算助手", () => {
    expect(outcomeLabelFor("unknown_product", "claimed")).toBe("签到成功");
    expect(outcomeLabelFor("unknown_product", "network_error")).toBe("网络错误");
    expect(flowResultTone("already_checked_in")).toBe("ok");
    expect(flowResultTone("claimed")).toBe("ok");
    expect(flowResultTone("network_error")).toBe("warn");
    expect(selectResultTone("already_checked_in")).toBe("neutral");
    expect(selectResultTone("claimed")).toBe("safe");
    expect(selectResultTone(null)).toBe("warning");
  });
});

describe("CheckinFlowRow / CheckinSelectRow（P19-B2④ 槽位化签到行）", () => {
  it("演进式行：running 高亮签到中 + idle 结果文案 + 已签账号行内单签禁用", () => {
    const { rerender } = render(
      <CheckinFlowRow
        productId="trae_cn"
        profileId="p1"
        displayName="账号甲"
        badge={<span>徽章</span>}
        meta="额度 12.5"
        selected={false}
        onToggle={noop}
        disabled={false}
        phase="running"
        result={null}
        waitSecs={null}
        onInlineCheckin={noop}
        inlineEnabled={true}
      />,
    );
    expect(screen.getByText("签到中…")).toBeInTheDocument();
    expect(screen.getByTestId("checkin-inline-p1")).toBeEnabled();

    rerender(
      <CheckinFlowRow
        productId="trae_cn"
        profileId="p1"
        displayName="账号甲"
        badge={<span>徽章</span>}
        meta="额度 12.5"
        selected={true}
        onToggle={noop}
        disabled={false}
        phase="idle"
        result={{ outcome: "already_checked_in", detail_code: null }}
        waitSecs={null}
        onInlineCheckin={noop}
        inlineEnabled={false}
      />,
    );
    expect(screen.getByText("今日已签到")).toBeInTheDocument();
    expect(screen.getByTestId("checkin-inline-p1")).toBeDisabled();
  });

  it("选择行：复选与结果徽章语气联动", () => {
    render(
      <CheckinSelectRow
        name="账号乙"
        meta=""
        badge={null}
        selected={true}
        onToggle={noop}
        disabled={false}
        resultLabel="签到成功"
        resultTone="safe"
      />,
    );
    expect(screen.getByRole("checkbox")).toBeChecked();
    expect(screen.getByText("签到成功").className).toContain("status-badge--safe");
  });
});

describe("CheckinPageSkeleton（P19-B2④ 签到页编排骨架）", () => {
  it("门控渲染：loading 保留页头标题与产品徽章，不渲染主体", () => {
    render(<CheckinPageSkeleton product={productUi("trae_cn")} active={true} loading={true} />);
    expect(screen.getByTestId("product-badge-trae_cn")).toBeInTheDocument();
    expect(screen.getByText("正在读取签到信息…")).toBeInTheDocument();
    expect(screen.queryByTestId("checkin-progress")).not.toBeInTheDocument();
  });

  it("批量区块：计数 caption + 进度条 + 行列表容器（checkin-flow 类名冻结）", () => {
    render(
      <CheckinPageSkeleton
        product={productUi("trae_cn")}
        active={true}
        loading={false}
        entryCount={3}
        selectedCount={2}
        progress={{ completed: 1, total: 3 }}
        rows={[<li key="a">行A</li>, <li key="b">行B</li>]}
        listVariant="flow"
      />,
    );
    expect(screen.getByText("已选 2/3")).toBeInTheDocument();
    expect(screen.getByTestId("checkin-progress")).toHaveTextContent("1/3");
    expect(document.querySelector("ul.checkin-flow")).not.toBeNull();
  });

  it("single 形态：content 插槽渲染时不走批量区块与空态", () => {
    render(
      <CheckinPageSkeleton
        product={productUi("workbuddy")}
        active={true}
        loading={false}
        content={<div data-testid="wb-content">内容</div>}
        emptyState={{ description: "空" }}
      />,
    );
    expect(screen.getByTestId("wb-content")).toBeInTheDocument();
    expect(screen.queryByText("空")).not.toBeInTheDocument();
    expect(screen.getByTestId("product-badge-workbuddy")).toBeInTheDocument();
  });

  it("空态：无条目且无 content 时渲染产品层空态（含动作）", () => {
    render(
      <CheckinPageSkeleton
        product={productUi("trae_cn")}
        active={true}
        loading={false}
        entryCount={0}
        emptyState={{
          className: "checkin-page__empty-actions",
          description: "还没有可签到的账号。",
          action: <button type="button">去账号页添加</button>,
        }}
      />,
    );
    expect(screen.getByText("还没有可签到的账号。")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "去账号页添加" }));
  });
});
