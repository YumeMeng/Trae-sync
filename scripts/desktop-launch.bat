@echo off
chcp 65001 >nul
title Trae Sync 一键启动
cd /d "D:\work\Trae-sync"

echo [1/3] 清理上次残留的 dev 实例（只杀本 App，不影响 TRAE）...
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0cleanup-dev-instance.ps1" -AppOnly

echo [2/3] 释放 Vite 端口 1420（仅清理残留的 node 进程）...
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0cleanup-dev-instance.ps1" -PortOnly

echo [3/3] 启动 Trae Sync（增量编译约 10-60 秒；关闭本窗口即退出）...
rem 复用本机 Rust 开发缓存，避免重复编译 OpenSSL 等依赖。
set "CARGO_TARGET_DIR=%TEMP%\tp14"
pnpm dev:tauri

echo.
echo Trae Sync 已退出。若上方有报错信息，请截图反馈。
pause
