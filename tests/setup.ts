import "@testing-library/jest-dom";
import { configure } from "@testing-library/react";

// P4c-QA-1：全量并行时 vitest 默认 12 个 worker 会与 jsdom/React 渲染抢 CPU，
// App 级用例的单次证据渲染在满载下可超过 1s，而 testing-library 默认
// asyncUtilTimeout = 1000ms（findBy*/waitFor 共用）。这里统一放宽到 5s：
// 只放宽"等待上限"，不改断言语义，也不改变任何用例的通过条件——
// 真正失败的用例仍然失败（只是失败判定不再受机器负载干扰）。
configure({ asyncUtilTimeout: 5000 });

// jsdom 不实现 matchMedia，给个空实现避免组件报错
if (!window.matchMedia) {
  Object.defineProperty(window, "matchMedia", {
    writable: true,
    value: (query: string) => ({
      matches: false,
      media: query,
      onchange: null,
      addListener: () => {},
      removeListener: () => {},
      addEventListener: () => {},
      removeEventListener: () => {},
      dispatchEvent: () => false,
    }),
  });
}
