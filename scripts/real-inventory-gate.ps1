param(
    [Parameter(Mandatory = $true)]
    [string]$AppPath,
    [switch]$ConfirmRealRead,
    [int]$CdpPort = 9391,
    [string]$EvidenceRoot = (Join-Path (Resolve-Path (Join-Path $PSScriptRoot "..")) (".scratch\REAL-INVENTORY-GATE-" + (Get-Date -Format "yyyyMMdd-HHmmssfff")))
)

$ErrorActionPreference = "Stop"
$repositoryRoot = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
$resolvedEvidenceRoot = [System.IO.Path]::GetFullPath($EvidenceRoot)
$resolvedAppPath = (Resolve-Path -LiteralPath $AppPath -ErrorAction Stop).Path
$appProcess = $null

function Test-PathWithin([string]$Path, [string]$Root) {
    $normalizedPath = [System.IO.Path]::GetFullPath($Path).TrimEnd('\')
    $normalizedRoot = [System.IO.Path]::GetFullPath($Root).TrimEnd('\')
    return $normalizedPath.StartsWith($normalizedRoot + [System.IO.Path]::DirectorySeparatorChar, [System.StringComparison]::OrdinalIgnoreCase)
}

if (-not $ConfirmRealRead) {
    throw "真实库存 Gate 会读取固定 Work CN 数据并创建 Trae Sync 自有快照；请显式传入 -ConfirmRealRead"
}
if (-not (Test-PathWithin $resolvedEvidenceRoot (Join-Path $repositoryRoot ".scratch"))) {
    throw "EvidenceRoot 必须位于仓库 .scratch 内"
}
if (Test-Path -LiteralPath $resolvedEvidenceRoot) {
    $existing = @(Get-ChildItem -LiteralPath $resolvedEvidenceRoot -Force -ErrorAction SilentlyContinue)
    if ($existing.Count -gt 0) { throw "EvidenceRoot 已存在内容；禁止覆盖旧真实现场" }
} else {
    New-Item -ItemType Directory -Force -Path $resolvedEvidenceRoot | Out-Null
}

function Get-TraeProcesses {
    @(Get-CimInstance Win32_Process | Where-Object {
        $_.Name -notmatch 'trae-sync|traesync' -and (
            $_.Name -match '^Trae( Helper)?\.exe$' -or
            ($_.ExecutablePath -and $_.ExecutablePath -match '\\TRAE( SOLO CN| Work CN)?\\')
        )
    })
}

function Get-SourceWitness {
    $dbRoot = Join-Path $env:APPDATA "TRAE SOLO CN\ModularData\ai-agent"
    $items = foreach ($name in @("database.db", "database.db-wal", "database.db-shm")) {
        $path = Join-Path $dbRoot $name
        if (-not (Test-Path -LiteralPath $path -PathType Leaf)) {
            [ordered]@{ name = $name; exists = $false }
            continue
        }
        $item = Get-Item -LiteralPath $path
        $identityText = (& fsutil file queryfileid $path 2>$null | Out-String)
        [ordered]@{
            name = $name
            exists = $true
            bytes = [int64]$item.Length
            sha256 = (Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash.ToLowerInvariant()
            last_write_utc = $item.LastWriteTimeUtc.ToString("o")
            file_id = [regex]::Match($identityText, '0x[0-9a-fA-F]+').Value.ToLowerInvariant()
        }
    }
    [ordered]@{
        captured_at_utc = [DateTime]::UtcNow.ToString("o")
        trae_process_count = (Get-TraeProcesses).Count
        files = @($items)
        source_path_recorded = $false
        session_content_recorded = $false
        authentication_material_recorded = $false
    }
}

function Write-Json([object]$Value, [string]$Path) {
    $Value | ConvertTo-Json -Depth 10 | Set-Content -LiteralPath $Path -Encoding utf8
}

try {
    if ((Get-TraeProcesses).Count -ne 0) { throw "TRAE 正在运行；为保护源库，Gate 拒绝读取" }
    $before = Get-SourceWitness
    Write-Json $before (Join-Path $resolvedEvidenceRoot "source-before.json")
    # Windows PowerShell 5.1 对单项管道结果没有稳定 Count 属性，先强制收集数组。
    $databaseFiles = @($before.files | Where-Object { $_.name -eq "database.db" -and $_.exists })
    if ($databaseFiles.Count -ne 1) {
        throw "固定 Work CN database.db 不存在"
    }

    $env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = "--remote-debugging-port=$CdpPort --remote-allow-origins=*"
    $appProcess = Start-Process -FilePath $resolvedAppPath -PassThru
    $deadline = [DateTime]::UtcNow.AddSeconds(45)
    $cdpReady = $false
    do {
        try {
            Invoke-RestMethod -Uri "http://127.0.0.1:$CdpPort/json/version" -TimeoutSec 2 | Out-Null
            $cdpReady = $true
            break
        } catch { Start-Sleep -Milliseconds 500 }
    } while ([DateTime]::UtcNow -lt $deadline)
    if (-not $cdpReady) { throw "WebView2 CDP 未在 45 秒内就绪" }

    & node (Join-Path $repositoryRoot "scripts\drive-real-inventory-gate.mjs") --cdp-port $CdpPort --evidence-root $resolvedEvidenceRoot
    if ($LASTEXITCODE -ne 0) { throw "库存扫描 UI 驱动失败" }

    $after = Get-SourceWitness
    Write-Json $after (Join-Path $resolvedEvidenceRoot "source-after.json")
    $beforeComparable = $before.files | ConvertTo-Json -Depth 8 -Compress
    $afterComparable = $after.files | ConvertTo-Json -Depth 8 -Compress
    if ($beforeComparable -ne $afterComparable) { throw "真实 TRAE DB/WAL/SHM witness 发生变化" }
    if ($after.trae_process_count -ne 0) { throw "Gate 期间发现 TRAE 进程，拒绝收口" }

    $summary = [ordered]@{
        verdict = "CURRENT_MACHINE_INVENTORY_PREVALIDATED"
        public_inventory_gate = "OPEN"
        source_write_observed = $false
        repeated_scan_stable = $true
        source_files_unchanged = $true
        evidence_root = $resolvedEvidenceRoot
        source_path_recorded = $false
        session_content_recorded = $false
        authentication_material_recorded = $false
    }
    Write-Json $summary (Join-Path $resolvedEvidenceRoot "inventory-gate-report.json")
    $summary | ConvertTo-Json -Depth 8
} catch {
    $failure = [ordered]@{
        verdict = "FAIL"
        error = ($_ | Out-String).Trim()
        source_write_observed = "UNKNOWN_UNTIL_WITNESS_REVIEW"
        evidence_root = $resolvedEvidenceRoot
    }
    Write-Json $failure (Join-Path $resolvedEvidenceRoot "inventory-gate-report.json")
    throw
} finally {
    if ($appProcess) {
        $appProcess.Refresh()
        if ($appProcess.MainWindowHandle -ne 0) { [void]$appProcess.CloseMainWindow() }
        Wait-Process -Id $appProcess.Id -Timeout 10 -ErrorAction SilentlyContinue
        if (Get-Process -Id $appProcess.Id -ErrorAction SilentlyContinue) {
            # 只结束本脚本启动、且路径与显式 AppPath 完全一致的进程。
            $owned = Get-CimInstance Win32_Process -Filter "ProcessId = $($appProcess.Id)" -ErrorAction SilentlyContinue
            if ($owned -and $owned.ExecutablePath -and ((Resolve-Path $owned.ExecutablePath).Path -eq $resolvedAppPath)) {
                Stop-Process -Id $appProcess.Id -Force -ErrorAction SilentlyContinue
            }
        }
    }
    Remove-Item Env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS -ErrorAction SilentlyContinue
}
