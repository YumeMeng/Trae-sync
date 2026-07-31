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
  },
});
