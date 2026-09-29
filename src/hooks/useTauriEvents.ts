import { useEffect, useRef } from "react";
import { listen } from "@tauri-apps/api/event";

/**
 * useTauriEvents——Tauri 事件订阅收敛 hook（P12-R2c A 案）。
 *
 * 用法：`useTauriEvents({ "event-name": handler }, deps?)`。
 * - 挂载期（或 deps 变化时）一次性订阅全部事件名；
 * - handler 存 latest-ref：组件每次渲染传入新的内联函数不会触发重订阅，
 *   事件到达时总是调用最新一次渲染的 handler；
 * - unlisten Promise 数组统一清理：卸载与 deps 变化都会退订；
 * - cancelled 代次守卫：异步注册期间组件已卸载时，注册完成后立即退订。
 *
 * 事件载荷类型按事件名由泛型 E 描述，handler 直接收载荷（hook 内部解包
 * Tauri Event 壳）。deps 只做浅比较（Object.is 逐项），调用方传内联数组
 * 也不会导致重订阅。
 */
export function useTauriEvents<E extends Record<string, unknown>>(
  handlers: { readonly [K in keyof E]: (payload: E[K]) => void },
  deps: readonly unknown[] = [],
): void {
  // 最新 handler 表：订阅回调只读引用，内联 handler 变化不重订阅。
  const latestRef = useRef(handlers);
  useEffect(() => {
    latestRef.current = handlers;
  });

  // deps 签名稳定化：浅比较相同则保持引用，effect 依赖签名而非数组身份，
  // 避免调用方每次渲染传内联 [] 导致反复清理重订。
  const depsRef = useRef(deps);
  if (!areDepsEqual(depsRef.current, deps)) depsRef.current = deps;
  const depsSignature = depsRef.current;

  useEffect(() => {
    let cancelled = false;
    const unlisteners: Array<() => void> = [];
    for (const name of Object.keys(handlers)) {
      void listen(name, (event) => {
        const handler = latestRef.current[name] as ((payload: unknown) => void) | undefined;
        handler?.(event.payload);
      }).then((unlisten) => {
        // cancelled 代次守卫：注册完成前已卸载/换代，立即退订，不进清理列表。
        if (cancelled) unlisten();
        else unlisteners.push(unlisten);
      }).catch(() => undefined);
    }
    return () => {
      cancelled = true;
      for (const unlisten of unlisteners) unlisten();
    };
    // handlers 仅在重订阅时取键名，回调永远经 latest-ref 取最新；deps 由
    // depsSignature 代理，二者均由调用方声明。
  }, [depsSignature]);
}

/** deps 浅比较：长度一致且逐项 Object.is 相同。 */
function areDepsEqual(a: readonly unknown[], b: readonly unknown[]): boolean {
  return a.length === b.length && a.every((value, index) => Object.is(value, b[index]));
}
