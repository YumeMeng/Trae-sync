import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { installFakeInvoke, mockRoute, mockCoreInvoke, resetRoutes } from "./helpers/fakeInvoke";

vi.mock("@tauri-apps/api/core", async () => {
  const fake = await import("./helpers/fakeInvoke");
  return { invoke: fake.mockCoreInvoke };
});

vi.mock("../src/platform/notify", () => ({
  notify: {
    success: vi.fn(),
    error: vi.fn(),
    warning: vi.fn(),
    info: vi.fn(),
    summary: vi.fn(),
  },
}));

import { WorkBuddyOverviewPage } from "../src/pages/WorkBuddyOverviewPage";
import type { AppPage } from "../src/components/NavigationRail";

/** P19-B3 库统计回执（serde 命名与 workbuddy_library.rs 同构）。 */
const libraryStats = {
  account_count: 3,
  session_count: 42,
  soft_deleted_session_count: 2,
  project_count: 7,
  projects_dir_bytes: 1024,
  last_active_unix_seconds: 1760000000,
};

function installOverviewRoutes(options?: { failStats?: boolean }) {
  mockRoute("list_workbuddy_checkin_accounts", () => [
    { profile_id: "wb-uid-1", screen_name: "甲账号", enabled: true },
    { profile_id: "wb-uid-2", screen_name: "乙账号", enabled: true },
  ]);
  mockRoute("get_workbuddy_checkin_overview", () => [
    {
      profile_id: "wb-uid-1", screen_name: "甲账号",
      today_checked_in: true, streak_days: 6, daily_credit: 100, total_credits: 700,
      checkin_dates: ["2026-10-20"], error: null,
    },
    {
      profile_id: "wb-uid-2", screen_name: "乙账号",
      today_checked_in: false, streak_days: 1, daily_credit: 100, total_credits: 500,
      checkin_dates: [], error: null,
    },
  ]);
  mockRoute("get_workbuddy_library_stats", () =>
    options?.failStats ? Promise.reject(new Error("no_library")) : libraryStats);
}

beforeEach(() => {
  resetRoutes();
  installFakeInvoke();
  window.localStorage.clear();
});

afterEach(() => {
  window.localStorage.clear();
});

describe("WorkBuddyOverviewPage（P19-B6 总览工作台模板）", () => {
  it("渲染：签到与账号统计卡（账号/今日已签/积分合计）+ 会话统计卡 + 主操作", async () => {
    installOverviewRoutes();
    const pages: AppPage[] = [];
    render(<WorkBuddyOverviewPage active={true} onNavigate={(page) => pages.push(page)} />);

    // 签到与账号卡：账号 2、今日已签 1/2、积分合计 1200。
    expect(await screen.findByTestId("wb-overview-checkin")).toBeInTheDocument();
    expect(screen.getByTestId("wb-overview-checkin").textContent).toContain("签到与账号");
    expect(screen.getByTestId("wb-overview-checkin-cta")).toHaveTextContent("去签到（余 1）");
    // 会话统计卡：库内会话/项目/账号 + 最近活跃。
    expect(screen.getByTestId("wb-overview-stats-library").textContent).toContain("42");
    expect(screen.getByTestId("wb-overview-stats-library").textContent).toContain("库内会话");
    // 主操作：查看对话库。
    fireEvent.click(screen.getByTestId("wb-overview-library-cta"));
    expect(pages).toContainEqual("master-library");
    // 管理账号入口在册。
    fireEvent.click(screen.getByTestId("wb-overview-accounts-cta"));
    expect(pages).toContainEqual("accounts");
  });

  it("降级：库统计读取失败 → 诚实提示行替代统计卡；签到数据失败 → 签到卡隐藏", async () => {
    installOverviewRoutes({ failStats: true });
    mockRoute("list_workbuddy_checkin_accounts", () => Promise.reject(new Error("down")));
    mockRoute("get_workbuddy_checkin_overview", () => Promise.reject(new Error("down")));
    render(<WorkBuddyOverviewPage active={true} onNavigate={() => undefined} />);

    await waitFor(() => {
      expect(screen.getByTestId("wb-overview-library-hint")).toBeInTheDocument();
    });
    expect(screen.queryByTestId("wb-overview-checkin")).not.toBeInTheDocument();
    expect(screen.queryByTestId("wb-overview-stats-library")).not.toBeInTheDocument();
    // 主操作仍在（对话库入口是恒定导航动作）。
    expect(screen.getByTestId("wb-overview-library-cta")).toBeInTheDocument();
  });

  it("积分假 0 修复（P19.2）：全账号未获取 → 占位 + 点击刷新，禁止静默 0", async () => {
    installOverviewRoutes();
    mockRoute("get_workbuddy_checkin_overview", () => [
      errorRow("wb-uid-1", "甲账号"),
      errorRow("wb-uid-2", "乙账号"),
    ]);
    render(<WorkBuddyOverviewPage active={true} onNavigate={() => undefined} />);

    const unknown = await screen.findByTestId("wb-overview-credits-unknown");
    expect(unknown.textContent).toContain("未获取");
    expect(unknown.textContent).not.toMatch(/积分合计\s*0/);
    // 点击刷新后重新拉取（覆盖回成功路由 → 恢复真实合计）。
    mockRoute("get_workbuddy_checkin_overview", () => [
      creditsRow("wb-uid-1", "甲账号", 700),
      creditsRow("wb-uid-2", "乙账号", 500),
    ]);
    fireEvent.click(screen.getByTestId("wb-overview-credits-refresh"));
    await waitFor(() => {
      expect(screen.queryByTestId("wb-overview-credits-unknown")).not.toBeInTheDocument();
    });
    expect(screen.getByTestId("wb-overview-checkin").textContent).toContain("1200");
  });

  it("积分假 0 修复（P19.2）：部分成功 → 仅合计 known 账号并注明 N/M 账号已获取", async () => {
    installOverviewRoutes();
    mockRoute("get_workbuddy_checkin_overview", () => [
      creditsRow("wb-uid-1", "甲账号", 700),
      errorRow("wb-uid-2", "乙账号"),
    ]);
    render(<WorkBuddyOverviewPage active={true} onNavigate={() => undefined} />);

    await screen.findByTestId("wb-overview-checkin");
    // 合计只含获取成功的账号；注明 N/M。
    expect(screen.getByTestId("wb-overview-checkin").textContent).toContain("700");
    expect(screen.getByTestId("wb-overview-checkin").textContent).toContain("1/2 账号已获取");
  });
});

/** P19.2 积分占位测试构造器：全字段 null + error 码行。 */
function errorRow(profileId: string, screenName: string) {
  return {
    profile_id: profileId,
    screen_name: screenName,
    today_checked_in: null,
    streak_days: null,
    daily_credit: null,
    total_credits: null,
    checkin_dates: [] as string[],
    error: "workbuddy_checkin_runtime_error",
  };
}

/** P19.2 积分占位测试构造器：指定积分的 ok 行。 */
function creditsRow(profileId: string, screenName: string, totalCredits: number) {
  return {
    profile_id: profileId,
    screen_name: screenName,
    today_checked_in: true,
    streak_days: 6,
    daily_credit: 100,
    total_credits: totalCredits,
    checkin_dates: ["2026-10-20"] as string[],
    error: null,
  };
}

// ===== P19-B6④ 设置页 WB 变体（备份诚实说明卡） =====

import { SettingsPanel } from "../src/components/SettingsPanel";

describe("SettingsPanel WB 变体（P19-B6 备份区诚实呈现 + P19.1 签到设置卡归位）", () => {
  it("WB 产品：每日自动签到真设置卡归位设置页，TRAE 备份管理面不渲染", async () => {
    mockRoute("get_workbuddy_auto_checkin_status", () => ({
      enabled: true,
      daily_time_hhmm: "09:30",
      ledger: null,
    }));
    mockRoute("list_workbuddy_checkin_accounts", () => [
      { profile_id: "wb-uid-1", screen_name: "甲账号", enabled: true },
      { profile_id: "wb-uid-2", screen_name: "乙账号", enabled: false },
    ]);
    mockRoute("list_workbuddy_backups", () => ({
      backup_dir: "C:" + String.fromCharCode(92) + "data",
      backups: [],
    }));
    render(<SettingsPanel active={true} productId="workbuddy" />);

    // P19.1 归位：真设置卡在册（总开关/触发时间/参与清单；testid 契约随卡迁移不变）。
    expect(await screen.findByTestId("workbuddy-auto-checkin-settings")).toBeInTheDocument();
    expect(screen.getByTestId("workbuddy-auto-checkin-enabled-toggle")).toBeInTheDocument();
    expect(screen.getByTestId("workbuddy-auto-checkin-time-input")).toBeInTheDocument();
    // 参与清单 = 后端有效参与语义（wb-uid-2 被显式关闭 → 未勾选）。
    expect((screen.getByTestId("workbuddy-auto-checkin-account-wb-uid-1") as HTMLInputElement).checked).toBe(true);
    expect((screen.getByTestId("workbuddy-auto-checkin-account-wb-uid-2") as HTMLInputElement).checked).toBe(false);
    // 旧「签到指引卡」退役（P19.1 以 Trae 真机排布为准：设置在设置页）。
    expect(screen.queryByText("WorkBuddy 的自动签到设置在「签到」页")).not.toBeInTheDocument();
    // B8 管理面（P19-B6 缺口闭环）：链/手动备份/删除入口在册。
    expect(await screen.findByTestId("workbuddy-backup-section")).toBeInTheDocument();
    expect(screen.getByTestId("workbuddy-backup-create")).toBeInTheDocument();
    // TRAE 备份管理面不渲染。
    expect(screen.queryByTestId("master-backup-section")).not.toBeInTheDocument();
  });

  it("全部参与入口（设置页）：一键把显式关闭的账号批量重新置为参与", async () => {
    mockRoute("get_workbuddy_auto_checkin_status", () => ({
      enabled: true,
      daily_time_hhmm: "09:30",
      ledger: null,
    }));
    mockRoute("list_workbuddy_checkin_accounts", () => [
      { profile_id: "wb-uid-1", screen_name: "甲账号", enabled: false },
      { profile_id: "wb-uid-2", screen_name: "乙账号", enabled: false },
    ]);
    render(<SettingsPanel active={true} productId="workbuddy" />);

    const button = await screen.findByTestId("workbuddy-auto-checkin-participate-all");
    expect(button).toHaveTextContent("全部参与（2）");
    fireEvent.click(button);
    await waitFor(() => {
      const calls = mockCoreInvoke.mock.calls.filter(([cmd]) => cmd === "set_workbuddy_account_auto_checkin");
      expect(calls).toHaveLength(2);
      expect(calls).toContainEqual(["set_workbuddy_account_auto_checkin", { profileId: "wb-uid-1", enabled: true }]);
      expect(calls).toContainEqual(["set_workbuddy_account_auto_checkin", { profileId: "wb-uid-2", enabled: true }]);
    });
  });

  it("TRAE 产品：备份管理面照常（现有行为不变）", async () => {
    render(<SettingsPanel active={true} productId="trae_cn" />);
    // WB 变体不生效：TRAE 备份分区在数据到达后渲染（此处只断言 WB 卡不存在）。
    expect(screen.queryByTestId("workbuddy-backup-note")).not.toBeInTheDocument();
  });
});

// ===== P19-B8 备份链管理面 备份链管理面 + 探针信息消费 =====

import { notify } from "../src/platform/notify";
import { WorkBuddyBackupSection } from "../src/components/WorkBuddyBackupSection";
import { WorkBuddyEnvironmentPage } from "../src/components/WorkBuddyEnvironmentPage";

const backupChain = {
  backup_dir: "C:\\data\\workbuddy-backups",
  backups: [
    { stamp_unix_seconds: 1760001200, file_count: 12, source_bytes: 2048, compressed_bytes: 1024 },
    { stamp_unix_seconds: 1759000000, file_count: null, source_bytes: null, compressed_bytes: 512 },
  ],
};

function installBackupRoutes() {
  mockRoute("list_workbuddy_backups", () => backupChain);
  mockRoute("create_workbuddy_backup", () => ({
    backup_dir: "C:\\data\\workbuddy-backups", stamp_unix_seconds: 1760002000,
    file_count: 13, source_bytes: 2100, rolled_over: 1,
  }));
  mockRoute("restore_workbuddy_backup", (args: { stamp: number }) => ({
    restored_files: 12, restored_bytes: 2048,
    safety_backup_stamp: 1760001500, restored_stamp: args.stamp,
  }));
  mockRoute("delete_workbuddy_backups", (args: { stamps: number[] }) => ({
    deleted_stamps: args.stamps, missing_stamps: [], remaining: 1,
    backup_dir: "C:\\data\\workbuddy-backups",
  }));
  mockRoute("get_workbuddy_session_health", () => ({
      status: "active", uid: "uid-1", uin: "uid-1", domain: "workbuddy.example",
      access_expires_at_unix_seconds: 1799999999, refresh_expires_at_unix_seconds: 1799999999,
      captured_in_pool: true,
    }));
  mockRoute("get_workbuddy_probe_info", () => ({
    running: true, process_count: 1, app: "WorkBuddy", version: "3.2.0",
    platform: "win32", install_path: "C:\\apps\\workbuddy",
    install_source: "registry", data_root: "C:\\Users\\x\\.workbuddy",
  }));
  mockRoute("check_workbuddy_login_health", () => ({
    status: "active", live_status: "verified",
  }));
}

describe("WorkBuddyBackupSection（P19-B8 备份链管理面）", () => {
  beforeEach(() => {
    resetRoutes();
    installFakeInvoke();
    installBackupRoutes();
  });

  it("渲染：链列表（最新在前）+ 手动备份/删除/恢复入口 + 半成品条目如实标注", async () => {
    render(<WorkBuddyBackupSection />);

    const chain = await screen.findByTestId("workbuddy-backup-chain");
    expect(chain.textContent).toContain("12 个文件");
    // 半成品条目（manifest 不可读）如实标注。
    expect(chain.textContent).toContain("清单缺失");
    expect(screen.getByTestId("workbuddy-backup-create")).toBeEnabled();
    expect(screen.getByTestId("workbuddy-backup-delete")).toBeDisabled();
  });

  it("手动备份：确认弹窗带客户端重启说明，回执滚动清理如实提示", async () => {
    render(<WorkBuddyBackupSection />);

    fireEvent.click(await screen.findByTestId("workbuddy-backup-create"));
    const dialog = await screen.findByTestId("workbuddy-backup-create-confirm");
    expect(dialog).toHaveTextContent("自动重启");
    fireEvent.click(screen.getByTestId("workbuddy-backup-create-confirm-confirm"));
    await waitFor(() => {
      expect(mockCoreInvoke.mock.calls.some(([cmd]) => cmd === "create_workbuddy_backup")).toBe(true);
    });
  });

  it("恢复：确认弹窗展示保链回退说明；恢复调用带链内 stamp", async () => {
    render(<WorkBuddyBackupSection />);

    fireEvent.click(await screen.findByTestId("workbuddy-backup-restore-1760001200"));
    const dialog = await screen.findByTestId("workbuddy-backup-restore-confirm");
    expect(dialog).toHaveTextContent("自动把当前状态备份一份");
    fireEvent.click(screen.getByTestId("workbuddy-backup-restore-confirm-confirm"));
    await waitFor(() => {
      expect(mockCoreInvoke.mock.calls).toContainEqual(["restore_workbuddy_backup", { stamp: 1760001200 }]);
    });
    // safety_backup_stamp 回退点呈现（be 契约：UI 务必展示该字段）。
    expect(vi.mocked(notify.success)).toHaveBeenCalledWith(
      expect.stringContaining("恢复前状态已存为"),
    );
  });

  it("删除：多选 + 单次确认调用 delete（confirmed=true 由适配器承载）", async () => {
    render(<WorkBuddyBackupSection />);

    fireEvent.click(await screen.findByTestId("workbuddy-backup-select-1760001200"));
    fireEvent.click(screen.getByTestId("workbuddy-backup-delete"));
    fireEvent.click(await screen.findByTestId("workbuddy-backup-delete-confirm-confirm"));
    await waitFor(() => {
      expect(mockCoreInvoke.mock.calls).toContainEqual(
        ["delete_workbuddy_backups", { stamps: [1760001200], confirmed: true }],
      );
    });
  });
});

describe("WorkBuddyEnvironmentPage 探针信息（P19-B8 数据面）", () => {
  beforeEach(() => {
    resetRoutes();
    installFakeInvoke();
    installBackupRoutes();
  });

  it("连接卡展示版本/安装路径/数据根（探针 fail-soft 数据面）", async () => {
    render(<WorkBuddyEnvironmentPage active={true} />);

    expect(await screen.findByTestId("workbuddy-env-client-card")).toBeInTheDocument();
    await waitFor(() => {
      expect(screen.getByTestId("workbuddy-env-client-card").textContent).toContain("3.2.0");
    });
    expect(screen.getByTestId("workbuddy-env-client-card").textContent).toContain("C:\\apps\\workbuddy");
  });
});


describe("WorkBuddyEnvironmentPage 探针降级渲染（P19-B8 fail-soft）", () => {
  beforeEach(() => {
    resetRoutes();
    installFakeInvoke();
    installBackupRoutes();
  });

  it("探针降级渲染：客户端未运行（全维度 null）→ 版本/路径/数据根如实显示未知", async () => {
    // 运行状态徽章由深度健康驱动（client_offline = 未运行）；探针只供版本/路径/数据根。
    mockRoute("check_workbuddy_login_health", () => ({
      status: "access_expired", live_status: "client_offline",
    }));
    mockRoute("get_workbuddy_probe_info", () => ({
      running: false, process_count: null, app: null, version: null,
      platform: null, install_path: null, install_source: null, data_root: null,
    }));
    render(<WorkBuddyEnvironmentPage active={true} />);

    const card = await screen.findByTestId("workbuddy-env-client-card");
    // 客户端未运行 = 探针正常返回（不是错误），连接卡恒可渲染。
    await waitFor(() => {
      expect(card.textContent).toContain("未运行");
    });
    expect(card.textContent).toContain("未知");
  });
});
