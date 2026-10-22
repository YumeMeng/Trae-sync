// 登记表契约（P4c-8a）：productId → builder 查表分发；未登记产品 fail-closed 到空视图；
// 能力表是页面与 builder 的唯一产品差异入口，未知产品回退最保守一档。
import { describe, expect, it } from "vitest";
import type { CheckinOverviewEntryDto } from "../src/types/account_switch";
import type { WorkBuddyAccountEntryDto } from "../src/types/product";
import { ACCOUNT_CAPABILITIES, DEFAULT_ACCOUNT_CAPABILITIES, accountCapabilitiesFor } from "../src/features/accounts/config";
import { accountsViewFor, type AccountsSnapshot } from "../src/features/accounts/registry";
import type { TraeAccountsSnapshot } from "../src/features/accounts/traeAccounts";
import type { WorkBuddyAccountsSnapshot } from "../src/features/accounts/workbuddyAccounts";

const NOW = 1_700_000_000; // 判定时刻由快照注入，纯函数不读时钟
const TRAE_BUSY = { login: false, credits: false, credentialRefresh: false, health: false, localRefresh: false, switching: false, toolSelection: false } as const;
const WB_BUSY = { loading: false, refreshing: false, renewing: false, health: false, capturing: false, switching: false, preview: false, login: false } as const;

function overviewEntry(overrides: Partial<CheckinOverviewEntryDto> = {}): CheckinOverviewEntryDto {
  return {
    profile_id: "p1", screen_name: "账号甲", account_id: "acc-1", created_at: null, last_verified_at: null,
    credits: null, credits_cached_at: null, usage_remaining_credits: null, usage_cached_at: null,
    checked_in: true, access_token_expires_at_unix_seconds: NOW + 86400, refresh_token_expires_at_unix_seconds: NOW + 864000,
    device_tail: null, device_id: null, display_name: null, masked_mobile: "138****0000", mobile_full: null,
    auto_checkin_enabled: true, refresh_error_code: null, credential_legacy: false,
    last_attempt_outcome: null, last_attempt_date: null, ...overrides,
  };
}

function traeSnapshot(overrides: Partial<TraeAccountsSnapshot> = {}): TraeAccountsSnapshot {
  return {
    capability: { enabled: true, transport: "http", real_http_enabled: true, message: "" },
    overview: [overviewEntry()], loginStates: {}, observedProfileId: null, traePool: null, poolStatusLabel: null,
    savedAccounts: [], savedCurrentProfileId: null, filter: "all", sort: "added", busy: TRAE_BUSY, nowUnixSeconds: NOW,
    ...overrides,
  };
}

function wbAccount(overrides: Partial<WorkBuddyAccountEntryDto> = {}): WorkBuddyAccountEntryDto {
  return {
    profile_id: "wb-1", account_id: "acc-wb-1", screen_name: "WB账号", domain: "workbuddy.example",
    access_token_expires_at_unix_seconds: NOW + 86400, refresh_token_expires_at_unix_seconds: NOW + 864000,
    minted_at_unix_seconds: NOW - 86400, last_updated_at_unix_seconds: NOW - 3600,
    display_name: "", archived: false, masked_phone: "", ...overrides,
  };
}

function wbSnapshot(overrides: Partial<WorkBuddyAccountsSnapshot> = {}): WorkBuddyAccountsSnapshot {
  return { sessionHealth: null, pool: [wbAccount()], checkinOverview: [], sort: "added", busy: WB_BUSY, nowUnixSeconds: NOW, ...overrides };
}

describe("accountsViewFor 产品登记表", () => {
  // 契约：trae_cn 登记项产出真实签到面视图——页头动作集、行锚点、徽章词逐字固定。
  it("trae_cn 产出非空视图：一行 + 真实面页头动作集", () => {
    const view = accountsViewFor({ productId: "trae_cn", trae: traeSnapshot() });
    expect(view.header.title).toBe("账号");
    expect(view.header.actions.map((action) => action.id)).toEqual([
      "account-refresh-local", "account-health-check", "account-refresh-credits", "account-refresh-credentials", "account-add-primary",
    ]);
    expect(view.totalCount).toBe(1);
    expect(view.rows.map((row) => row.id)).toEqual(["p1"]);
    expect(view.rows[0].badges.map((badge) => badge.text)).toEqual(["已签", "未登录"]);
    expect(view.rows[0].actions.map((action) => action.id)).toEqual(["account-switch-p1"]);
    expect(view.detail).toEqual([]); // 详情分区归 detailView.ts，列表 builder 不臆造
  });

  // 契约：workbuddy 登记项产出池面视图——归档标记进徽章组（archiveTag=true）、单槽凭据、余额指标、无过滤行。
  it("workbuddy 产出非空视图：归档行徽章 + 单槽凭据 + 余额指标", () => {
    const view = accountsViewFor({ productId: "workbuddy", workbuddy: wbSnapshot({ pool: [wbAccount({ archived: true })] }) });
    expect(view.totalCount).toBe(1);
    expect(view.rows[0].id).toBe("wb-1");
    expect(view.rows[0].badges.map((badge) => badge.text)).toEqual(["已归档", "正常"]);
    expect(view.rows[0].metrics.map((metric) => `${metric.label}=${metric.value}`)).toEqual(["积分余额=未获取"]);
    expect(view.rows[0].actions.map((action) => action.id)).toEqual(["workbuddy-switch-wb-1"]);
    expect(view.filters?.options).toEqual([]); // attentionFilter=false
    expect(view.filters?.sortOptions.map((option) => option.key)).toEqual(["added", "name"]); // checkinSort=false
  });

  // 契约：未登记产品 fail-closed——空视图、不抛错、空态文案不冒充任一产品。
  it("未知产品返回空视图且不抛错", () => {
    const unknown = { productId: "future_product", trae: traeSnapshot() } as unknown as AccountsSnapshot;
    expect(() => accountsViewFor(unknown)).not.toThrow();
    expect(accountsViewFor(unknown)).toEqual({
      header: { title: "账号", badges: [], actions: [] },
      stats: null, filters: null, rows: [], totalCount: 0, listActions: [], detail: [],
      empty: { description: "账号信息暂不可用，请稍后重试。", actionLabel: null, testId: "account-center__empty" },
    });
  });
});

describe("accountCapabilitiesFor 能力表", () => {
  // 契约：两产品能力档逐字固定（能力表是唯一产品差异入口，页面禁止再写 productId 分支）。
  it("已登记产品返回各自能力档", () => {
    expect(accountCapabilitiesFor("trae_cn")).toBe(ACCOUNT_CAPABILITIES.trae_cn);
    expect(accountCapabilitiesFor("trae_cn")).toMatchObject({
      badgeSlots: 2, mobileBackfill: true, devices: true, resetDevice: true, attentionFilter: true,
      credentialRefresh: "batch-with-confirm", switchTransaction: "master-five-step", deleteScope: "archive-credential-credits",
    });
    expect(accountCapabilitiesFor("workbuddy")).toMatchObject({
      badgeSlots: 1, mobileBackfill: false, archiveTag: true, devices: false, toolSelection: false,
      credentialRefresh: "pool-renew", switchTransaction: "preview-confirm-coldstart", deleteScope: "pool-record-profile",
    });
  });

  // 契约：未知产品回退最保守一档（单槽、无补录/设备/过滤），不猜能力、不抛错。
  it("未知产品回退到保守默认", () => {
    expect(accountCapabilitiesFor("future_product")).toBe(DEFAULT_ACCOUNT_CAPABILITIES);
    expect(accountCapabilitiesFor("future_product")).toMatchObject({
      badgeSlots: 1, mobileBackfill: false, archiveTag: true, devices: false, resetDevice: false,
      toolSelection: false, attentionFilter: false, checkinSort: false, inlineCheckin: false,
    });
  });
});
