import { act, renderHook, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { useArchiveDraft } from "../src/hooks/useArchiveDraft";
import { installFakeInvoke, mockRoute, resetRoutes } from "./helpers/fakeInvoke";

// P13-5：共享 fakeInvoke 路由器（迁移模式见 tests/helpers/fakeInvoke.ts 头注释）。
vi.mock("@tauri-apps/api/core", async () => {
  const fake = await import("./helpers/fakeInvoke");
  return { invoke: fake.mockCoreInvoke };
});
// 草稿迁移/回退路径经 notify 提示；断言调用而非真实 sonner。
vi.mock("../src/platform/notify", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../src/platform/notify")>();
  return {
    ...actual,
    notify: {
      success: vi.fn(),
      error: vi.fn(),
      info: vi.fn(),
      warning: vi.fn(),
      summary: vi.fn(),
    },
  };
});

import { notify } from "../src/platform/notify";

const mockInvoke = vi.mocked(invoke);
const notifyMock = vi.mocked(notify);

const LIBRARY_ID = "master";
const LEGACY_KEY = "trae-sync:archive-draft:master";
/** localStorage 旧草稿（P11-5 迁移源）种子内容。 */
const LEGACY_JSON = JSON.stringify({ archiveSessionIds: ["s1"], restoreSessionIds: ["r1"] });
const LEGACY_DRAFT = { archiveSessionIds: ["s1"], restoreSessionIds: ["r1"] };
const EMPTY_DTO = { archive_session_ids: [], restore_session_ids: [] };
const EMPTY_DRAFT = { archiveSessionIds: [], restoreSessionIds: [] };
const BACKEND_DTO = { archive_session_ids: ["b1"], restore_session_ids: [] };
const BACKEND_DRAFT = { archiveSessionIds: ["b1"], restoreSessionIds: [] };

type BackendGet = "empty" | "stored" | "error";
type BackendSet = "ok" | "error";
type LocalSeed = "local" | "none" | "bad";

/** 配置 get/set_archive_draft mock（get 三态 × set 两态）。 */
function mockBackend(get: BackendGet, set: BackendSet = "ok") {
  mockRoute("get_archive_draft", () => {
    if (get === "error") throw new Error("archive_draft_join_failed");
    if (get === "stored") return BACKEND_DTO;
    return EMPTY_DTO;
  });
  mockRoute("set_archive_draft", () => {
    if (set === "error") throw new Error("archive_draft_store_unavailable");
    return true;
  });
}

/** 预置 localStorage 迁移源：有（合法旧草稿）/ 无 / 坏数据（非法 JSON）。 */
function seedLocal(seed: LocalSeed) {
  if (seed === "local") window.localStorage.setItem(LEGACY_KEY, LEGACY_JSON);
  else if (seed === "bad") window.localStorage.setItem(LEGACY_KEY, "{oops");
}

/** 挂载 hook 并等待加载与迁移/写回的异步链落定。 */
async function renderDraft() {
  const utils = renderHook(() => useArchiveDraft(LIBRARY_ID, { active: true, readOnly: false }));
  await waitFor(() =>
    expect(mockInvoke).toHaveBeenCalledWith("get_archive_draft", { libraryId: LIBRARY_ID }),
  );
  await act(async () => {
    await Promise.resolve();
    await Promise.resolve();
    await Promise.resolve();
  });
  return utils;
}

beforeEach(() => {
  window.localStorage.clear();
  // 等价于原 mockInvoke.mockReset()：清调用历史 + 重装按路由表分发的实现。
  resetRoutes();
  installFakeInvoke();
});

afterEach(() => {
  vi.clearAllMocks();
});

/**
 * 9 格迁移矩阵（后端 空/有/命令失败 × 本地 有/无/坏数据）。
 * 每格断言终态：draft 来源、本地键是否清除、storageError。
 */
describe("useArchiveDraft 9 格迁移矩阵", () => {
  it("1. 后端空 × 本地有：迁移成功——draft 来自 legacy，本地键清除", async () => {
    mockBackend("empty");
    seedLocal("local");

    const { result } = await renderDraft();
    expect(result.current.draft).toEqual(LEGACY_DRAFT);
    expect(result.current.hasChanges).toBe(true);
    expect(result.current.storageError).toBeNull();
    // 迁移：旧草稿写入后端命令，成功后清除旧键（防重复迁移）。
    expect(mockInvoke).toHaveBeenCalledWith("set_archive_draft", {
      libraryId: LIBRARY_ID,
      archiveSessionIds: ["s1"],
      restoreSessionIds: ["r1"],
    });
    expect(window.localStorage.getItem(LEGACY_KEY)).toBeNull();
  });

  it("2. 后端空 × 本地无：空草稿起步，不发起迁移写", async () => {
    mockBackend("empty");
    seedLocal("none");

    const { result } = await renderDraft();
    expect(result.current.draft).toEqual(EMPTY_DRAFT);
    expect(result.current.hasChanges).toBe(false);
    expect(result.current.storageError).toBeNull();
    expect(mockInvoke).not.toHaveBeenCalledWith("set_archive_draft", expect.anything());
  });

  it("3. 后端空 × 本地坏：坏数据按空草稿处理，不迁移且坏键保留", async () => {
    mockBackend("empty");
    seedLocal("bad");

    const { result } = await renderDraft();
    expect(result.current.draft).toEqual(EMPTY_DRAFT);
    expect(result.current.storageError).toBeNull();
    expect(mockInvoke).not.toHaveBeenCalledWith("set_archive_draft", expect.anything());
    // 坏键不触发迁移路径，也不被清除（清除只发生在写回成功时）。
    expect(window.localStorage.getItem(LEGACY_KEY)).toBe("{oops");
  });

  it("4. 后端有 × 本地有：后端草稿优先，写回统一清理本地迁移残留", async () => {
    mockBackend("stored");
    seedLocal("local");

    const { result } = await renderDraft();
    expect(result.current.draft).toEqual(BACKEND_DRAFT);
    expect(result.current.hasChanges).toBe(true);
    expect(result.current.storageError).toBeNull();
    // 加载后写回 effect 对后端草稿幂等写回，并清掉本地旧键。
    expect(mockInvoke).toHaveBeenCalledWith("set_archive_draft", {
      libraryId: LIBRARY_ID,
      archiveSessionIds: ["b1"],
      restoreSessionIds: [],
    });
    expect(window.localStorage.getItem(LEGACY_KEY)).toBeNull();
  });

  it("5. 后端有 × 本地无：直接采用后端草稿", async () => {
    mockBackend("stored");
    seedLocal("none");

    const { result } = await renderDraft();
    expect(result.current.draft).toEqual(BACKEND_DRAFT);
    expect(result.current.storageError).toBeNull();
    expect(window.localStorage.getItem(LEGACY_KEY)).toBeNull();
  });

  it("6. 后端有 × 本地坏：采用后端草稿，写回时清掉坏键", async () => {
    mockBackend("stored");
    seedLocal("bad");

    const { result } = await renderDraft();
    expect(result.current.draft).toEqual(BACKEND_DRAFT);
    expect(result.current.storageError).toBeNull();
    expect(window.localStorage.getItem(LEGACY_KEY)).toBeNull();
  });

  it("7. 后端失败 × 本地有：迁移写失败——draft 回退 legacy，本地键保留，页内无横幅但 notify 警告", async () => {
    mockBackend("error", "error");
    seedLocal("local");

    const { result } = await renderDraft();
    expect(result.current.draft).toEqual(LEGACY_DRAFT);
    expect(result.current.hasChanges).toBe(true);
    // 迁移失败只 notify.warning（本会话照常使用），不升级页内横幅。
    expect(result.current.storageError).toBeNull();
    expect(notifyMock.warning).toHaveBeenCalledWith(
      "归档草稿迁移到新存储失败，本次暂由本机浏览器存储保存，未丢失。",
    );
    // 本地键保留（草稿未丢失），内容为 legacy 草稿。
    expect(window.localStorage.getItem(LEGACY_KEY)).toContain("s1");
  });

  it("8. 后端失败 × 本地无：按空草稿起步，不写后端", async () => {
    mockBackend("error", "error");
    seedLocal("none");

    const { result } = await renderDraft();
    expect(result.current.draft).toEqual(EMPTY_DRAFT);
    expect(result.current.storageError).toBeNull();
    expect(mockInvoke).not.toHaveBeenCalledWith("set_archive_draft", expect.anything());
  });

  it("9. 后端失败 × 本地坏：坏数据按空草稿处理，坏键保留", async () => {
    mockBackend("error", "error");
    seedLocal("bad");

    const { result } = await renderDraft();
    expect(result.current.draft).toEqual(EMPTY_DRAFT);
    expect(result.current.storageError).toBeNull();
    expect(mockInvoke).not.toHaveBeenCalledWith("set_archive_draft", expect.anything());
    expect(window.localStorage.getItem(LEGACY_KEY)).toBe("{oops");
  });
});

describe("useArchiveDraft 写回与守卫", () => {
  it("setDraft 后写回后端命令；reset 清空并写回空草稿", async () => {
    mockBackend("empty");
    seedLocal("none");

    const { result } = await renderDraft();
    act(() => {
      result.current.setDraft({ archiveSessionIds: ["x1"], restoreSessionIds: [] });
    });
    await act(async () => { await Promise.resolve(); });
    expect(result.current.hasChanges).toBe(true);
    expect(mockInvoke).toHaveBeenCalledWith("set_archive_draft", {
      libraryId: LIBRARY_ID,
      archiveSessionIds: ["x1"],
      restoreSessionIds: [],
    });

    act(() => {
      result.current.reset();
    });
    await act(async () => { await Promise.resolve(); });
    expect(result.current.hasChanges).toBe(false);
    expect(mockInvoke).toHaveBeenCalledWith("set_archive_draft", {
      libraryId: LIBRARY_ID,
      archiveSessionIds: [],
      restoreSessionIds: [],
    });
  });

  it("写回失败且 localStorage 回退也失败时置 storageError（双失败显式横幅）", async () => {
    mockBackend("empty", "error");
    seedLocal("none");
    // localStorage 回退也抛错（配额/隐私模式等不可用场景）。jsdom 的 Storage
    // 特殊实现下实例级 spy 不生效，必须 spy 原型（与 LibrarySessionsPanel.test 同法）。
    const setItemSpy = vi.spyOn(Storage.prototype, "setItem").mockImplementation(() => {
      throw new Error("storage_quota");
    });

    try {
      const { result } = await renderDraft();
      act(() => {
        result.current.setDraft({ archiveSessionIds: ["y1"], restoreSessionIds: [] });
      });
      // 等写回链落定：set_archive_draft 拒绝 → localStorage 回退也抛错 → 置横幅。
      await waitFor(() => expect(result.current.storageError).not.toBeNull());

      expect(result.current.storageError).toContain("归档设置暂存失败");
    } finally {
      setItemSpy.mockRestore();
    }
  });

  it("readOnly 库不读取也不写草稿（P11-6）", async () => {
    renderHook(() => useArchiveDraft(LIBRARY_ID, { active: true, readOnly: true }));
    await act(async () => { await Promise.resolve(); await Promise.resolve(); });

    expect(mockInvoke).not.toHaveBeenCalledWith("get_archive_draft", expect.anything());
    expect(mockInvoke).not.toHaveBeenCalledWith("set_archive_draft", expect.anything());
  });

  it("active=false 时不发起草稿读取，转 active 后补读", async () => {
    mockBackend("empty");
    const { rerender } = renderHook(
      ({ active }: { active: boolean }) => useArchiveDraft(LIBRARY_ID, { active, readOnly: false }),
      { initialProps: { active: false } },
    );
    await act(async () => { await Promise.resolve(); });
    expect(mockInvoke).not.toHaveBeenCalledWith("get_archive_draft", expect.anything());

    rerender({ active: true });
    await waitFor(() =>
      expect(mockInvoke).toHaveBeenCalledWith("get_archive_draft", { libraryId: LIBRARY_ID }),
    );
  });
});
