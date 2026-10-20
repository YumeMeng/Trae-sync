import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
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

import { WorkBuddyLibraryPage } from "../src/pages/WorkBuddyLibraryPage";

// ===== P19-B6③ WB 库页（WbLibraryDetail；B3 七命令消费） =====

const historyGroups = [
  {
    cwd: "D:\\work\\proj-a",
    escaped_dir: "d--work-proj-a",
    session_count: 2,
    last_active_unix_seconds: 1760000000,
    sessions: [
      { session_id: "s-1", title: "会话一", transport: "local", deleted: false, last_active_unix_seconds: 1760000000 },
      { session_id: "s-2", title: "会话二", transport: "cloud", deleted: false, last_active_unix_seconds: 1759000000 },
    ],
  },
];

const sessionMessages = {
  session_id: "s-1",
  messages: [
    { index: 0, kind: "message", role: "user", timestamp_unix_ms: 1760000000000, text: "你好" },
    { index: 1, kind: "message", role: "assistant", timestamp_unix_ms: 1760000001000, text: "你好，有什么可以帮你？" },
  ],
  skipped_lines: 0,
  body_missing: false,
};

function installLibraryRoutes() {
  mockRoute("get_workbuddy_library_stats", () => ({
    account_count: 1, session_count: 2, soft_deleted_session_count: 0,
    project_count: 1, projects_dir_bytes: 2048, last_active_unix_seconds: 1760000000,
  }));
  mockRoute("get_workbuddy_library_history", () => historyGroups);
  mockRoute("list_workbuddy_accounts", () => [
    { profile_id: "wb-uid-1", account_id: "uid-1", screen_name: "甲账号", domain: "x",
      access_token_expires_at_unix_seconds: 1, refresh_token_expires_at_unix_seconds: 1,
      minted_at_unix_seconds: 1, last_updated_at_unix_seconds: 1 },
  ]);
  mockRoute("get_workbuddy_session_messages", () => sessionMessages);
  mockRoute("get_workbuddy_library_checkup", () => ({
    orphan_body_files: [],
    missing_body_count: 1,
    missing_body_samples: ["s-9"],
    soft_deleted_session_count: 0,
    foreign_dirs: [],
    cwd_merge_candidates: [["d--work-proj-a", "d--work-proj-a-v2"]],
  }));
  mockRoute("delete_workbuddy_sessions", () => ({
    deleted_sessions: 1, skipped_ids: 0, tail: { restarted: true, process_count_after_restart: 1 },
  }));
  mockRoute("purge_workbuddy_sessions", () => ({
    purged_sessions: 1, removed_body_files: 1, missing_body_files: 0, failed_body_files: [],
    backup_path: "C:\\backup\\workbuddy-data-1", tail: { restarted: true, process_count_after_restart: 1 },
  }));
  mockRoute("merge_workbuddy_cwd_projects", () => ({
    moved_sessions: 2, moved_body_files: 2, missing_body_files: 0, conflict_body_files: 0,
    tail: { restarted: true, process_count_after_restart: 1 },
  }));
}

beforeEach(() => {
  resetRoutes();
  installFakeInvoke();
  window.localStorage.clear();
  installLibraryRoutes();
});

describe("WorkBuddyLibraryPage（P19-B6 库页；B3 七命令消费）", () => {
  it("渲染：统计卡 + cwd 分组浏览（账号过滤/软删开关）+ 体检卡五项", async () => {
    render(<WorkBuddyLibraryPage active={true} />);

    expect(await screen.findByTestId("workbuddy-library-stats")).toBeInTheDocument();
    expect(screen.getByTestId("workbuddy-library-groups")).toBeInTheDocument();
    expect(screen.getByText("D:\\work\\proj-a")).toBeInTheDocument();
    // 体检五项在册。
    expect(screen.getByTestId("workbuddy-library-checkup").textContent).toContain("孤儿正文");
    expect(screen.getByTestId("workbuddy-library-checkup").textContent).toContain("归并候选");
  });

  it("会话消息查看：点击本地会话行展开时间线；cloud 行只读不可选", async () => {
    render(<WorkBuddyLibraryPage active={true} />);

    await screen.findByTestId("workbuddy-library-groups");
    // cloud 行：复选禁用（写红线）。
    expect(screen.getByTestId("workbuddy-library-select-s-2")).toBeDisabled();
    // 本地行：点击展开消息。
    fireEvent.click(screen.getByText("会话一"));
    expect(await screen.findByTestId("workbuddy-library-messages")).toHaveTextContent("你好，有什么可以帮你？");
  });

  it("软删除：账号过滤激活才可用；确认弹窗后调用 delete 命令", async () => {
    render(<WorkBuddyLibraryPage active={true} />);

    await screen.findByTestId("workbuddy-library-groups");
    // 全库视图（未过滤）：删除按钮禁用（归属校验锚）。
    expect(screen.getByTestId("workbuddy-library-soft-delete")).toBeDisabled();
    // 切账号过滤。
    fireEvent.change(screen.getByTestId("workbuddy-library-uid-filter"), { target: { value: "uid-1" } });
    fireEvent.click(await screen.findByTestId("workbuddy-library-select-s-1"));
    fireEvent.click(screen.getByTestId("workbuddy-library-soft-delete"));
    fireEvent.click(await screen.findByTestId("workbuddy-library-soft-delete-confirm-confirm"));
    await waitFor(() => {
      expect(mockCoreInvoke.mock.calls).toContainEqual(
        ["delete_workbuddy_sessions", { sessionIds: ["s-1"], userId: "uid-1" }],
      );
    });
  });

  it("彻底删除：单独确认（决策 7），备份前置说明在册", async () => {
    render(<WorkBuddyLibraryPage active={true} />);

    await screen.findByTestId("workbuddy-library-groups");
    fireEvent.change(screen.getByTestId("workbuddy-library-uid-filter"), { target: { value: "uid-1" } });
    fireEvent.click(await screen.findByTestId("workbuddy-library-select-s-1"));
    fireEvent.click(screen.getByTestId("workbuddy-library-purge"));
    const dialog = await screen.findByTestId("workbuddy-library-purge-confirm");
    expect(dialog).toHaveTextContent("滚动备份");
    fireEvent.click(screen.getByTestId("workbuddy-library-purge-confirm-confirm"));
    await waitFor(() => {
      expect(mockCoreInvoke.mock.calls).toContainEqual(
        ["purge_workbuddy_sessions", { sessionIds: ["s-1"], userId: "uid-1" }],
      );
    });
  });

  it("cwd 归并：候选组入口 + 单次确认调用 merge 命令", async () => {
    render(<WorkBuddyLibraryPage active={true} />);

    const mergeButton = await screen.findByTestId("workbuddy-library-merge-2");
    fireEvent.click(mergeButton);
    const dialog = await screen.findByTestId("workbuddy-library-merge-confirm");
    expect(dialog).toHaveTextContent("d--work-proj-a-v2");
    fireEvent.click(screen.getByTestId("workbuddy-library-merge-confirm-confirm"));
    await waitFor(() => {
      expect(mockCoreInvoke.mock.calls).toContainEqual(
        ["merge_workbuddy_cwd_projects", { canonicalCwd: "d--work-proj-a", variantCwds: ["d--work-proj-a-v2"] }],
      );
    });
  });
});
