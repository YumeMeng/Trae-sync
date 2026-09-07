import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import {
  CheckinSlotBadge,
  LoginArchiveSlotBadge,
  deriveCheckinSlotState,
  deriveLoginSlotState,
  type CheckinSlotState,
  type LoginSlotState,
} from "../src/components/StatusBadges";

// 固定时间基准（本地时区正午，跨时区运行也远离午夜边界，保证衍生时间不跨日）。
const NOW = new Date("2026-09-04T12:00:00");
/** 基准时刻 1 小时前（必为今日）。 */
const TODAY_ISO = new Date(NOW.getTime() - 3600_000).toISOString();
/** 基准时刻 26 小时前（必为昨日；签到状态按 CST 午夜重置，昨日尝试不残留）。 */
const YESTERDAY_ISO = new Date(NOW.getTime() - 26 * 3600_000).toISOString();

describe("deriveCheckinSlotState 签到槽判定（G10 六态）", () => {
  it("服务端确认已签 → checked_in", () => {
    expect(deriveCheckinSlotState({ checked_in: true }, NOW)).toBe("checked_in");
  });

  it("服务端确认未签且今日无尝试 → unchecked", () => {
    expect(
      deriveCheckinSlotState({ checked_in: false, last_attempt_outcome: null, last_attempt_date: null }, NOW),
    ).toBe("unchecked");
  });

  it("今日尝试且业务性失败（business: 前缀）→ failed", () => {
    expect(
      deriveCheckinSlotState(
        { checked_in: false, last_attempt_outcome: "business:9074", last_attempt_date: TODAY_ISO },
        NOW,
      ),
    ).toBe("failed");
  });

  it("今日尝试且传输性失败（transport: 前缀）→ retry_pending", () => {
    expect(
      deriveCheckinSlotState(
        { checked_in: false, last_attempt_outcome: "transport:network_error", last_attempt_date: TODAY_ISO },
        NOW,
      ),
    ).toBe("retry_pending");
  });

  it("今日尝试被业务拒绝（not_eligible）→ not_eligible", () => {
    expect(
      deriveCheckinSlotState(
        { checked_in: false, last_attempt_outcome: "not_eligible", last_attempt_date: TODAY_ISO },
        NOW,
      ),
    ).toBe("not_eligible");
  });

  it("今日无服务端数据（checked_in 为 null/undefined）→ not_refreshed", () => {
    expect(deriveCheckinSlotState({ checked_in: null }, NOW)).toBe("not_refreshed");
    expect(deriveCheckinSlotState({ checked_in: undefined }, NOW)).toBe("not_refreshed");
  });

  it("传输性失败时即使 checked_in 未确认（null）也按待重试展示，不丢失败信息", () => {
    expect(
      deriveCheckinSlotState(
        { checked_in: null, last_attempt_outcome: "transport:timeout", last_attempt_date: TODAY_ISO },
        NOW,
      ),
    ).toBe("retry_pending");
  });

  it("服务端确认已签优先于今日失败尝试（成功翻转会覆盖失败残留）", () => {
    expect(
      deriveCheckinSlotState(
        { checked_in: true, last_attempt_outcome: "business:9074", last_attempt_date: TODAY_ISO },
        NOW,
      ),
    ).toBe("checked_in");
  });

  it("昨日尝试结果不残留（新的一天状态重置）", () => {
    expect(
      deriveCheckinSlotState(
        { checked_in: false, last_attempt_outcome: "business:9074", last_attempt_date: YESTERDAY_ISO },
        NOW,
      ),
    ).toBe("unchecked");
  });

  it("今日尝试但结果码未归类 → 保守按待重试（需关注）", () => {
    expect(
      deriveCheckinSlotState(
        { checked_in: false, last_attempt_outcome: "weird_code", last_attempt_date: TODAY_ISO },
        NOW,
      ),
    ).toBe("retry_pending");
  });

  it("后端 DTO 的本地日期串形态（YYYY-MM-DD）与 RFC3339 同判定", () => {
    // 后端 last_attempt_date 输出本地日期串：今日串 → 失败态可达（IPC 契约，
    // 2026-09-04 修复后端缺字段导致失败态永不显示）；昨日串 → 不残留。
    const todayText = "2026-09-04";
    expect(
      deriveCheckinSlotState(
        { checked_in: false, last_attempt_outcome: "business:9074", last_attempt_date: todayText },
        NOW,
      ),
    ).toBe("failed");
    expect(
      deriveCheckinSlotState(
        { checked_in: false, last_attempt_outcome: "business:9074", last_attempt_date: "2026-09-03" },
        NOW,
      ),
    ).toBe("unchecked");
  });

  it("结果码 ok（checked_in 未确认的竞态窗口）→ 已签，不落入琥珀兜底", () => {
    expect(
      deriveCheckinSlotState(
        { checked_in: null, last_attempt_outcome: "ok", last_attempt_date: TODAY_ISO },
        NOW,
      ),
    ).toBe("checked_in");
  });
});

describe("deriveLoginSlotState 登录槽判定（G10 五态）", () => {
  it("logged_in → ok", () => {
    expect(deriveLoginSlotState({ login_state: "logged_in" })).toBe("ok");
  });

  it("stale 可自动恢复 → expired（琥珀）", () => {
    expect(deriveLoginSlotState({ login_state: "stale", relogin_only: false })).toBe("expired");
  });

  it("stale 且不可自动恢复（旧通道/续期被拒）→ relogin（红）", () => {
    expect(deriveLoginSlotState({ login_state: "stale", relogin_only: true })).toBe("relogin");
  });

  it("logged_out → pending（待登录）", () => {
    expect(deriveLoginSlotState({ login_state: "logged_out" })).toBe("pending");
  });

  it("undefined / null / uninitialized → signed_out（未登录）", () => {
    expect(deriveLoginSlotState({})).toBe("signed_out");
    expect(deriveLoginSlotState({ login_state: undefined })).toBe("signed_out");
    expect(deriveLoginSlotState({ login_state: null })).toBe("signed_out");
    expect(deriveLoginSlotState({ login_state: "uninitialized" })).toBe("signed_out");
  });
});

describe("CheckinSlotBadge 签到槽渲染（词 × 色）", () => {
  const cases: ReadonlyArray<[CheckinSlotState, string, string]> = [
    ["checked_in", "已签", "slot-badge--ok"],
    ["unchecked", "未签", "slot-badge--idle"],
    ["failed", "签到失败", "slot-badge--danger"],
    ["retry_pending", "待重试", "slot-badge--warn"],
    ["not_eligible", "不可领取", "slot-badge--idle"],
    ["not_refreshed", "未刷新", "slot-badge--unloaded"],
  ];

  it.each(cases)("状态 %s → 词「%s」+ 类 %s", (state, word, className) => {
    const { unmount } = render(<CheckinSlotBadge state={state} />);
    const badge = screen.getByText(word);
    expect(badge).toBeInTheDocument();
    expect(badge).toHaveClass(className);
    unmount();
  });

  it("任何状态下都不再出现「状态未知」", () => {
    for (const [state] of cases) {
      const { unmount } = render(<CheckinSlotBadge state={state} />);
      expect(screen.queryByText("状态未知")).not.toBeInTheDocument();
      unmount();
    }
  });
});

describe("LoginArchiveSlotBadge 登录槽渲染（词 × 色）", () => {
  const cases: ReadonlyArray<[LoginSlotState, string, string]> = [
    ["ok", "正常", "slot-badge--ok"],
    ["expired", "已过期", "slot-badge--warn"],
    ["relogin", "需重登", "slot-badge--danger"],
    ["pending", "待登录", "slot-badge--warn"],
    ["signed_out", "未登录", "slot-badge--idle"],
  ];

  it.each(cases)("状态 %s → 词「%s」+ 类 %s", (state, word, className) => {
    const { unmount } = render(<LoginArchiveSlotBadge state={state} />);
    const badge = screen.getByText(word);
    expect(badge).toBeInTheDocument();
    expect(badge).toHaveClass(className);
    unmount();
  });

  it("存档可用性收进悬浮提示，不占主视野（ok 态）", () => {
    render(<LoginArchiveSlotBadge state="ok" archiveAvailable={true} />);
    expect(screen.getByText("正常")).toHaveAttribute("title", expect.stringContaining("历史登录存档"));
  });

  it("需重登的悬浮提示直接指向重新登录动作", () => {
    render(<LoginArchiveSlotBadge state="relogin" />);
    expect(screen.getByText("需重登")).toHaveAttribute("title", expect.stringContaining("重新登录"));
  });
});

describe("G10c 页面接入口径（DTO 形态输入 → derive → 渲染）", () => {
  // 页面（账号页/签到页）以 CheckinOverviewEntryDto 直接喂 derive 函数；
  // 这里用同形态对象验证接入链路，替代已删除的旧 props 过渡路径。
  it("今日业务性失败的账号渲染「签到失败」", () => {
    // 传固定基准 NOW：TODAY_ISO 相对 NOW 生成，用真实当前日期会跨日误判（时间敏感）。
    const state = deriveCheckinSlotState(
      {
        checked_in: false,
        last_attempt_outcome: "business:9074",
        last_attempt_date: TODAY_ISO,
      },
      NOW,
    );
    render(<CheckinSlotBadge state={state} />);
    expect(screen.getByText("签到失败")).toBeInTheDocument();
    expect(screen.queryByText("状态未知")).not.toBeInTheDocument();
  });

  it("旧通道凭据账号渲染登录槽「需重登」", () => {
    const state = deriveLoginSlotState({ login_state: "stale", relogin_only: true });
    render(<LoginArchiveSlotBadge state={state} />);
    expect(screen.getByText("需重登")).toBeInTheDocument();
  });

  it("实调登录有效且无本地修正渲染登录槽「正常」", () => {
    const state = deriveLoginSlotState({ login_state: "logged_in", relogin_only: false });
    render(<LoginArchiveSlotBadge state={state} />);
    expect(screen.getByText("正常")).toBeInTheDocument();
  });
});
