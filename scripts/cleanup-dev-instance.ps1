param(
    [switch] $AppOnly,
    [switch] $PortOnly
)

$devExecutable = Join-Path $env:TEMP 'tp14\debug\trae-sync.exe'
$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path

if (-not $PortOnly) {
    # 只结束隔离 target 中的开发版，避免误关正式版。
    Get-Process -Name trae-sync -ErrorAction SilentlyContinue |
        Where-Object { $_.Path -and $_.Path -ieq $devExecutable } |
        Stop-Process -Force -ErrorAction SilentlyContinue

    # 结束本仓库的 Tauri dev 调度进程及其子进程，避免旧调度器重启刚释放的 Vite。
    $tauriDevProcesses = Get-CimInstance Win32_Process -ErrorAction SilentlyContinue |
        Where-Object {
            $_.Name -eq 'node.exe' -and
            $_.CommandLine -and
            $_.CommandLine.IndexOf($repoRoot, [System.StringComparison]::OrdinalIgnoreCase) -ge 0 -and
            $_.CommandLine.IndexOf('@tauri-apps\cli\tauri.js', [System.StringComparison]::OrdinalIgnoreCase) -ge 0 -and
            $_.CommandLine.Contains(([char]34 + 'dev' + [char]34))
        }
    foreach ($process in $tauriDevProcesses) {
        & taskkill.exe /PID $process.ProcessId /T /F 1>$null 2>$null
    }
}

if (-not $AppOnly) {
    # 只释放本仓库 Vite 监听的端口，不影响其他项目的 Node 进程。
    Get-NetTCPConnection -LocalPort 1420 -State Listen -ErrorAction SilentlyContinue |
        ForEach-Object {
            $process = Get-CimInstance Win32_Process -Filter "ProcessId = $($_.OwningProcess)" -ErrorAction SilentlyContinue
            if (
                $process -and
                $process.Name -eq 'node.exe' -and
                $process.CommandLine -and
                $process.CommandLine.IndexOf($repoRoot, [System.StringComparison]::OrdinalIgnoreCase) -ge 0 -and
                $process.CommandLine.IndexOf('vite', [System.StringComparison]::OrdinalIgnoreCase) -ge 0
            ) {
                Stop-Process -Id $process.ProcessId -Force -ErrorAction SilentlyContinue
            }
        }
}
