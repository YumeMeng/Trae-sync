import { defineConfig } from "vitest/config";
import react from "@vitejs/plugin-react";

// 前端单元测试配置：使用 jsdom 环境模拟浏览器
export default defineConfig({
  plugins: [react()],
  test: {
    environment: "jsdom",
    globals: true,
    setupFiles: ["./tests/setup.ts"],
    include: ["tests/**/*.{test,spec}.{ts,tsx}"],
    // P4c-QA-1：单用例超时放宽（默认 5s）。全量并行时 12 个 worker 抢 CPU，
    // App 级用例（多次点击 + 异步等待）在满载下会超过 5s——这是机器负载，
    // 不是用例真的卡死；断言与等待条件一律不动，失败判定保持原样。
    testTimeout: 20000,
  },
});
