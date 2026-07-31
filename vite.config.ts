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
  },
  envPrefix: ["VITE_", "TAURI_"],
  build: {
    target: "es2022",
    minify: "esbuild",
    sourcemap: false,
  },
});
