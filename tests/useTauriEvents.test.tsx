import { act, renderHook } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { useTauriEvents } from "../src/hooks/useTauriEvents";

// jsdom 无 Tauri IPC：mock listen 用内存注册表实现，测试用 emit 手动投递事件。
const { mockListen } = vi.hoisted(() => ({ mockListen: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({ listen: mockListen }));

/** 事件名 → 已注册回调（模拟 Tauri 事件路由）。 */
const registry = new Map<string, Set<(event: { payload: unknown }) => void>>();
/** 每次 listen 成功注册返回的 unlisten spy（按调用顺序）。 */
let unlistenSpies: Array<ReturnType<typeof vi.fn>> = [];

/** 测试中手动投递事件（模拟后端 emit）。 */
function emit(eventName: string, payload: unknown): void {
  for (const handler of registry.get(eventName) ?? []) handler({ payload });
}

/** 等待 hook 的异步注册全部落地（flush microtask 链）。 */
async function flushRegistrations(): Promise<void> {
  await act(async () => {
    await Promise.resolve();
    await Promise.resolve();
  });
}

beforeEach(() => {
  registry.clear();
  unlistenSpies = [];
  mockListen.mockImplementation(async (name: string, handler: (event: { payload: unknown }) => void) => {
    const handlers = registry.get(name) ?? new Set<(event: { payload: unknown }) => void>();
    registry.set(name, handlers);
    handlers.add(handler);
    const unlisten = vi.fn(() => {
      handlers.delete(handler);
    });
    unlistenSpies.push(unlisten);
    return unlisten;
  });
});

afterEach(() => {
  vi.clearAllMocks();
});

describe("useTauriEvents", () => {
  it("挂载期订阅全部事件名，卸载时统一清理", async () => {
    const { unmount } = renderHook(() =>
      useTauriEvents({
        "event-b": (payload: string) => payload,
        "event-a": (payload: number) => payload,
      }),
    );
    await flushRegistrations();

    expect(mockListen).toHaveBeenCalledTimes(2);
    expect(mockListen.mock.calls.map(([name]) => name).sort()).toEqual(["event-a", "event-b"]);

    unmount();
    // 每个 unlisten 恰好被调用一次（统一清理，无遗漏无重复）。
    expect(unlistenSpies).toHaveLength(2);
    expect(unlistenSpies.every((spy) => spy.mock.calls.length === 1)).toBe(true);
  });

  it("handler 为 latest-ref：重渲染传内联函数不重订阅，事件调到最新闭包", async () => {
    const received: string[] = [];
    const { rerender } = renderHook(
      ({ tag }: { tag: string }) =>
        useTauriEvents({ "event-a": (payload: string) => received.push(`${tag}:${payload}`) }),
      { initialProps: { tag: "first" } },
    );
    await flushRegistrations();
    expect(mockListen).toHaveBeenCalledTimes(1);

    // 重渲染：新的内联 handlers 对象（闭包变化），但不重订阅。
    rerender({ tag: "second" });
    await flushRegistrations();
    expect(mockListen).toHaveBeenCalledTimes(1);

    act(() => emit("event-a", "p1"));
    // 调到最新一次渲染的闭包，旧闭包不残留。
    expect(received).toEqual(["second:p1"]);
  });

  it("卸载后事件不再触发 handler", async () => {
    const handler = vi.fn();
    const { unmount } = renderHook(() => useTauriEvents({ "event-a": handler }));
    await flushRegistrations();

    act(() => emit("event-a", "before"));
    expect(handler).toHaveBeenCalledTimes(1);

    unmount();
    act(() => emit("event-a", "after"));
    // 卸载后退订生效：注册表里已无该回调。
    expect(handler).toHaveBeenCalledTimes(1);
  });

  it("异步注册期间卸载：resolve 后立即 unlisten（cancelled 代次守卫）", async () => {
    // 不 flush 注册：render 后立即卸载，此时 listen 的 Promise 尚未 resolve。
    const { unmount } = renderHook(() => useTauriEvents({ "event-a": () => undefined }));
    unmount();

    await flushRegistrations();
    // 注册完成时代次已失效：hook 立即退订，未注册进清理列表也 must 退订。
    expect(unlistenSpies).toHaveLength(1);
    expect(unlistenSpies[0].mock.calls.length).toBe(1);
  });

  it("多事件并存：各自事件路由到各自 handler", async () => {
    const gotA: unknown[] = [];
    const gotB: unknown[] = [];
    renderHook(() =>
      useTauriEvents({
        "event-a": (payload: string) => gotA.push(payload),
        "event-b": (payload: number) => gotB.push(payload),
      }),
    );
    await flushRegistrations();

    act(() => {
      emit("event-a", "x");
      emit("event-b", 7);
      emit("event-a", "y");
    });
    expect(gotA).toEqual(["x", "y"]);
    expect(gotB).toEqual([7]);
  });

  it("deps 变化时重订阅；deps 为值相同的新数组时不重订阅", async () => {
    const { rerender } = renderHook(
      ({ deps }: { deps: unknown[] }) =>
        useTauriEvents({ "event-a": () => undefined }, deps),
      { initialProps: { deps: [1] } },
    );
    await flushRegistrations();
    expect(mockListen).toHaveBeenCalledTimes(1);

    // deps 值变化：旧订阅清理 + 重新注册。
    rerender({ deps: [2] });
    await flushRegistrations();
    expect(mockListen).toHaveBeenCalledTimes(2);
    expect(unlistenSpies[0].mock.calls.length).toBe(1);

    // deps 新数组但浅比较相同：latest-ref 语义，不重订阅。
    rerender({ deps: [2] });
    await flushRegistrations();
    expect(mockListen).toHaveBeenCalledTimes(2);
  });
});
