@echo off
chcp 65001 >nul
title Trae Sync 一键启动
cd /d "D:\work\Trae-sync"

echo [1/3] 清理上次残留的 dev 实例（只杀本 App，不影响 TRAE）...
taskkill /IM trae-sync.exe /F >nul 2>&1

echo [2/3] 释放 Vite 端口 1420（仅清理残留的 node 进程）...
powershell -NoProfile -Command "Get-NetTCPConnection -LocalPort 1420 -State Listen -ErrorAction SilentlyContinue | ForEach-Object { $p = Get-Process -Id $_.OwningProcess -ErrorAction SilentlyContinue; if ($p -and $p.ProcessName -eq 'node') { Stop-Process -Id $p.Id -Force } }" >nul 2>&1

echo [3/3] 启动 Trae Sync（增量编译约 10-60 秒；关闭本窗口即退出）...
pnpm tauri dev

echo.
echo Trae Sync 已退出。若上方有报错信息，请截图反馈。
pause
