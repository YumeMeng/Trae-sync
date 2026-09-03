import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// Tauri 期望前端 dev server 监听在固定端口，且允许任意主机访问（用于 WebView2 调试）
export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    host: "127.0.0.1",
    // 排除 src-tauri（官方 Tauri 模板同款配置）：target/ 内有数万个 cargo 构建产物，
    // chokidar 初始扫描会拖垮事件循环，导致首屏模块请求排队数十秒（表现为窗口长时间纯白）
    watch: {
      ignored: ["**/src-tauri/**"],
    },
  },
  envPrefix: ["VITE_", "TAURI_"],
  build: {
    target: "es2022",
    minify: "esbuild",
    sourcemap: false,
  },
});
