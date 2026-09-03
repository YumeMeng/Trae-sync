import { defineConfig, devices } from "@playwright/test";

// ============================================================================
// T03/T04 Playwright 配置：Windows 桌面视口，使用 mock 命令边界
// ============================================================================
//
// 关键约束（交接文档 Playwright 章节）：
// - 测试使用确定性 mock 命令边界，绝不启动 Tauri 或访问真实 TRAE 数据
// - 当前产品只验收桌面与紧凑桌面视口，不把移动端作为设计目标
// - 覆盖授权/空/扫描中/成功/失败状态、账号/项目/会话导航、对话预览、搜索结果导航
// - 预览不改变选择
// - 无文本重叠、裁剪或水平溢出
//
// webServer 启动 Vite preview（构建产物），不启动 Tauri 运行时

export default defineConfig({
  testDir: "./e2e",
  fullyParallel: false,
  forbidOnly: !!process.env.CI,
  retries: process.env.CI ? 1 : 0,
  workers: 1,
  reporter: [["list"], ["html", { open: "never" }]],
  timeout: 60_000,
  expect: { timeout: 10_000 },
  use: {
    baseURL: "http://127.0.0.1:4173",
    trace: "retain-on-failure",
    screenshot: "only-on-failure",
  },
  // 使用 vite preview 构建产物作为受测前端，不启动 Tauri
  webServer: {
    command: "pnpm build && pnpm preview --port 4173 --strictPort",
    url: "http://127.0.0.1:4173",
    // 禁止复用旧 preview；验收必须由当前源码重新构建，端口占用时直接失败。
    reuseExistingServer: false,
    timeout: 120_000,
  },
  projects: [
    {
      name: "desktop-chromium",
      use: {
        browserName: "chromium",
        viewport: { width: 1280, height: 800 },
      },
    },
  ],
});
