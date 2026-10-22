// 详情视图模型契约（P4c-8a）：六区装配顺序、能力裁剪、testid 逐字固定。
// 纯函数层直接喂快照（零 mock）；唯一渲染用例守「删除动作 id → 确认弹层 id」的对应关系。
import { createElement } from "react";
import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { DetailDangerAction } from "../src/features/accounts/components/DetailDangerAction";
import { ACCOUNT_TEXTS, accountCapabilitiesFor, type AccountRowView, type DetailSectionView } from "../src/features/accounts/config";
import { buildDetailView, type DetailSnapshot } from "../src/features/accounts/detailView";
import type { CheckinOverviewEntryDto } from "../src/types/account_switch";
import type { WorkBuddyAccountEntryDto, WorkBuddyCheckinAccountStatusDto } from "../src/types/product";

const NOW = 1_700_000_000;
const SIX = ["basics", "credential", "checkin", "actions", "danger", "tech"] as const;
const sectionIds = (sections: readonly DetailSectionView[]) => sections.map((s) => s.id);
const fieldIds = (s: DetailSectionView) => s.fields.map((f) => f.id);
const actionIds = (s: DetailSectionView) => s.actions.map((a) => a.id);
const allIds = (sections: readonly DetailSectionView[]) => sections.flatMap((s) => [...fieldIds(s), ...actionIds(s)]);
const pick = (sections: readonly DetailSectionView[], id: DetailSectionView["id"]): DetailSectionView => {
  const found = sections.find((s) => s.id === id);
  if (found === undefined) throw new Error(`缺少分区 ${id}`);
  return found;
};
const ROW: AccountRowView = { id: "p1", displayName: "账号甲", subtitle: "", badges: [{ text: "已签", tone: "neutral" }], metrics: [], actions: [], archived: false, current: false };

function entry(overrides: Partial<CheckinOverviewEntryDto> = {}): CheckinOverviewEntryDto {
  return {
    profile_id: "p1", screen_name: "账号甲", account_id: "acc-1", created_at: "2026-08-01T00:00:00Z",
    last_verified_at: "2026-08-22T00:00:00Z", credits: null, credits_cached_at: null,
    usage_remaining_credits: 120, usage_cached_at: "2026-08-22T01:00:00Z", checked_in: true,
    access_token_expires_at_unix_seconds: NOW + 86400, refresh_token_expires_at_unix_seconds: NOW + 864000,
    device_tail: "8502", device_id: "dev-1", display_name: "备注甲", masked_mobile: "138****0000",
    mobile_full: null, auto_checkin_enabled: true, refresh_error_code: null, credential_legacy: false,
    last_attempt_outcome: null, last_attempt_date: null, ...overrides,
  };
}

const traeReal = (overrides: Partial<CheckinOverviewEntryDto> = {}): DetailSnapshot => ({
  productId: "trae_cn",
  trae: { face: "checkin", entry: entry(overrides), loginState: { profile_id: "p1", login_state: "logged_in", archive_available: false }, currentAccountStatus: "current", nowUnixSeconds: NOW },
});
const traePool = (isToolSelected: boolean): DetailSnapshot => ({
  productId: "trae_cn",
  trae: { face: "pool", toolSelectionBusy: false, account: { id: "p1", displayName: "账号甲", status: "active", lastVerifiedAt: null, isToolSelected } },
});
const overview: WorkBuddyCheckinAccountStatusDto = {
  profile_id: "wb-1", screen_name: "WB账号", today_checked_in: false, streak_days: 3, daily_credit: 10,
  total_credits: 120, credits_balance: 0, credits_total: 500, balance_error: null, checkin_dates: [], masked_phone: null, error: null,
};
const wbSnapshot = (overrides: Partial<WorkBuddyAccountEntryDto> = {}): DetailSnapshot => ({
  productId: "workbuddy",
  workbuddy: {
    account: {
      profile_id: "wb-1", account_id: "acc-wb-1", screen_name: "WB账号", domain: "workbuddy.example",
      access_token_expires_at_unix_seconds: NOW + 86400, refresh_token_expires_at_unix_seconds: NOW + 864000,
      minted_at_unix_seconds: NOW - 86400, last_updated_at_unix_seconds: NOW - 3600,
      display_name: "", archived: false, masked_phone: "133****1304", ...overrides,
    },
    overview, participating: true, isCurrentLogin: false, nowUnixSeconds: NOW,
  },
});

describe("TRAE 真实总览详情", () => {
  // 契约：六区齐全且顺序固定 basics→credential→checkin→actions→danger→tech（旧 AccountDetail 顺序与标题）。
  it("六区齐全且顺序固定", () => {
    const view = buildDetailView({ productId: "trae_cn", row: ROW, snapshot: traeReal() });
    expect(sectionIds(view.sections)).toEqual([...SIX]);
    expect(view.sections.map((s) => s.title)).toEqual(["基础信息", "登录健康度", "自动签到", "操作", ACCOUNT_TEXTS.dangerHeading, "技术细节"]);
    expect(view.header.title).toBe("备注甲");
    expect(pick(view.sections, "danger").hint).toBe(ACCOUNT_TEXTS.dangerHintTrae);
    expect(pick(view.sections, "credential").hint).not.toBeNull();
  });

  // 契约：字段/动作 id 逐字沿用旧 testid 且顺序即渲染顺序；手机补录随 mobileBackfill=true 出现。
  it("字段与动作 id 逐字固定", () => {
    const view = buildDetailView({ productId: "trae_cn", row: ROW, snapshot: traeReal() });
    expect(fieldIds(pick(view.sections, "basics"))).toEqual([
      "account-detail-alias", "account-detail-login-status", "account-detail-current-account", "account-detail-verified",
      "account-detail-mobile", "account-detail-mobile-hint", "account-detail-usage", "account-detail-refresh-status",
      "account-detail-checked-in", "account-detail-created-at", "account-detail-device-tail",
    ]);
    expect(actionIds(pick(view.sections, "basics"))).toEqual(["account-detail-alias-save", "account-detail-mobile-save"]);
    expect(actionIds(pick(view.sections, "actions"))).toEqual([
      "account-detail-refresh-credentials", "account-detail-refresh-credits", "account-detail-checkin", "account-detail-relogin", "account-detail-reset-device",
    ]);
    expect(fieldIds(pick(view.sections, "tech"))).toEqual(["account-detail-tech-profile", "account-detail-tech-account", "account-detail-tech-device"]);
    const checkin = pick(view.sections, "actions").actions.find((a) => a.id === "account-detail-checkin");
    expect(checkin?.label).toBe("今日已签到");
    expect(checkin?.disabled?.reason).toBe("今日已签到（缓存确认），无需重复领取");
  });
});

describe("TRAE 适配器池详情", () => {
  // 契约：池面只有 basics/actions/tech 三区（旧 AdapterAccountDetail 无凭据/签到/危险区，不许硬凑六区）。
  it("只产出三区，不含 credential/checkin/danger", () => {
    const view = buildDetailView({ productId: "trae_cn", row: ROW, snapshot: traePool(false) });
    expect(sectionIds(view.sections)).toEqual(["basics", "actions", "tech"]);
    expect(fieldIds(pick(view.sections, "basics"))).toEqual(["adapter-account-detail-status", "adapter-account-detail-current", "adapter-account-detail-verified"]);
    expect(fieldIds(pick(view.sections, "tech"))).toEqual(["adapter-account-detail-tech-id"]);
    expect(actionIds(pick(view.sections, "actions"))).toEqual(["adapter-account-detail-tool-select"]);
  });

  // 契约：工具内已选时操作区整区不出现（toolSelection 能力 + 选中态双门禁，缺省不占位）。
  it("工具内已选则无操作区", () => {
    expect(sectionIds(buildDetailView({ productId: "trae_cn", row: ROW, snapshot: traePool(true) }).sections)).toEqual(["basics", "tech"]);
  });
});

describe("WorkBuddy 详情", () => {
  // 契约：WB 六区齐全；devices=false 时全视图零设备相关字段/动作（设备只在页面插槽，builder 不臆造）。
  it("六区齐全且零设备相关动作", () => {
    const view = buildDetailView({ productId: "workbuddy", row: ROW, snapshot: wbSnapshot() });
    expect(sectionIds(view.sections)).toEqual([...SIX]);
    expect(view.sections.map((s) => s.title)).toEqual(["基础信息", "凭据健康度", "签到与积分", "档案管理", ACCOUNT_TEXTS.dangerHeading, "技术细节"]);
    expect(allIds(view.sections).filter((id) => id.includes("device"))).toEqual([]);
    expect(accountCapabilitiesFor("workbuddy").devices).toBe(false);
    // 对照：TRAE devices=true → 重置签到设备动作在操作区。
    const trae = buildDetailView({ productId: "trae_cn", row: ROW, snapshot: traeReal() });
    expect(actionIds(pick(trae.sections, "actions"))).toContain("account-detail-reset-device");
  });

  // 契约：签到与积分区字段顺序固定 + 余额 0 照实显示（0 不是「未获取」）。
  it("签到与积分区字段逐字且余额 0 照实", () => {
    const checkin = pick(buildDetailView({ productId: "workbuddy", row: ROW, snapshot: wbSnapshot() }).sections, "checkin");
    expect(fieldIds(checkin)).toEqual([
      "workbuddy-detail-credits", "workbuddy-detail-accumulated", "workbuddy-detail-streak", "workbuddy-detail-today", "workbuddy-detail-participation-toggle",
    ]);
    expect(checkin.fields.map((f) => f.value)).toEqual(["0", "120", "3", "未签到", "参与每日自动签到"]);
  });
});

describe("能力裁剪", () => {
  // 契约：mobileBackfill=false（WB）时手机补录字段与保存动作整组不出现；TRAE 有该能力则出现。
  it("mobileBackfill=false 无手机补录字段与动作", () => {
    const wb = buildDetailView({ productId: "workbuddy", row: ROW, snapshot: wbSnapshot() });
    expect(allIds(wb.sections).filter((id) => id.includes("mobile"))).toEqual([]);
    const trae = buildDetailView({ productId: "trae_cn", row: ROW, snapshot: traeReal() });
    expect(actionIds(pick(trae.sections, "basics"))).toContain("account-detail-mobile-save");
    expect(accountCapabilitiesFor("workbuddy").mobileBackfill).toBe(false);
  });

  // 契约：archiveTag=true 时归档字段进档案管理区，且行徽章（已归档）原样进详情页头。
  it("archiveTag=true 时归档字段与行徽章同源", () => {
    const row: AccountRowView = { ...ROW, archived: true, badges: [{ text: "已归档", tone: "neutral" }] };
    const view = buildDetailView({ productId: "workbuddy", row, snapshot: wbSnapshot({ archived: true }) });
    const actions = pick(view.sections, "actions");
    expect(fieldIds(actions)).toEqual(["workbuddy-detail-alias", "workbuddy-detail-archived-toggle"]);
    expect(actions.fields[1].value).toBe("已归档");
    expect(view.header.badges.map((b) => b.text)).toEqual(["已归档"]);
    expect(actionIds(actions)).toEqual(["workbuddy-detail-alias-save"]);
  });
});

describe("危险动作与确认弹层 testid 契约", () => {
  // 契约：DetailDangerAction 把 action.id 原样透传为弹层容器 testid，取消/确认按钮追加 -cancel/-confirm；
  // 故容器 testid 与按钮同值（不是 id + "-dialog"），两产品删除动作 id 分别固定。
  it("删除动作 id 与弹层 testid 对应关系", () => {
    const danger = pick(buildDetailView({ productId: "trae_cn", row: ROW, snapshot: traeReal() }).sections, "danger");
    expect(actionIds(danger)).toEqual(["account-detail-remove"]);
    expect(danger.actions[0].label).toBe("删除账号");
    expect(danger.actions[0].kind).toBe("danger");
    render(createElement(DetailDangerAction, { action: danger.actions[0], hint: danger.hint, busy: false, onAction: () => undefined }));
    fireEvent.click(screen.getByTestId("account-detail-remove"));
    const dialog = screen.getByRole("dialog");
    expect(dialog.getAttribute("data-testid")).toBe("account-detail-remove");
    expect(dialog.textContent).toContain("删除账号");
    expect(screen.getByTestId("account-detail-remove-cancel")).toBeInTheDocument();
    expect(screen.getByTestId("account-detail-remove-confirm").tagName).toBe("BUTTON");
    expect(actionIds(pick(buildDetailView({ productId: "workbuddy", row: ROW, snapshot: wbSnapshot() }).sections, "danger"))).toEqual(["workbuddy-detail-purge"]);
  });
});
