param(
    [string]$InstallerPath = (Join-Path (Resolve-Path (Join-Path $PSScriptRoot "..")) "artifacts\Trae Sync_0.2.1_x64-setup-20260819-current.exe"),
    [string]$EvidenceRoot = (Join-Path (Resolve-Path (Join-Path $PSScriptRoot "..")) (".scratch\20260819-CURRENT-MACHINE-PREVALIDATION-" + (Get-Date -Format "yyyyMMdd-HHmmssfff"))),
    [int]$CdpPort = 9368
)

$ErrorActionPreference = "Stop"
$repositoryRoot = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
$resolvedRoot = [System.IO.Path]::GetFullPath($EvidenceRoot)
$installRoot = Join-Path $resolvedRoot "install"
$profileRoot = Join-Path $resolvedRoot "profile"
$reportPath = Join-Path $resolvedRoot "lifecycle-report.json"
$originalEnvironment = @{}
$app = $null
$profileRemoved = $false

function Test-PathWithin([string]$Path, [string]$Root) {
    $normalizedPath = [System.IO.Path]::GetFullPath($Path).TrimEnd('\')
    $normalizedRoot = [System.IO.Path]::GetFullPath($Root).TrimEnd('\')
    return $normalizedPath.StartsWith(
        $normalizedRoot + [System.IO.Path]::DirectorySeparatorChar,
        [System.StringComparison]::OrdinalIgnoreCase
    )
}

if (-not (Test-PathWithin $resolvedRoot (Join-Path $repositoryRoot ".scratch"))) {
    throw "EvidenceRoot 必须位于仓库 .scratch 内"
}

# 证据目录非空时拒绝重跑，避免覆盖旧成功或失败现场。
if (Test-Path -LiteralPath $resolvedRoot -PathType Container) {
    $existingEvidence = @(Get-ChildItem -LiteralPath $resolvedRoot -Force)
    if ($existingEvidence.Count -gt 0) {
        throw "EvidenceRoot 已存在证据；请指定新的 .scratch 目录，禁止覆盖旧现场"
    }
} else {
    New-Item -ItemType Directory -Force -Path $resolvedRoot | Out-Null
}
$installer = (Resolve-Path -LiteralPath $InstallerPath).Path
$installerItem = Get-Item -LiteralPath $installer
$installerHash = (Get-FileHash -LiteralPath $installer -Algorithm SHA256).Hash.ToLowerInvariant()

try {
    # 所有应用环境变量都指向 .scratch，避免预验误触当前用户数据。
    foreach ($name in @("APPDATA", "LOCALAPPDATA", "TEMP", "TMP", "WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS")) {
        $originalEnvironment[$name] = [Environment]::GetEnvironmentVariable($name, "Process")
    }

    $isolatedAppData = Join-Path $profileRoot "Roaming"
    $isolatedLocalAppData = Join-Path $profileRoot "Local"
    $isolatedTemp = Join-Path $profileRoot "Temp"
    New-Item -ItemType Directory -Force -Path $isolatedAppData, $isolatedLocalAppData, $isolatedTemp | Out-Null
    [Environment]::SetEnvironmentVariable("APPDATA", $isolatedAppData, "Process")
    [Environment]::SetEnvironmentVariable("LOCALAPPDATA", $isolatedLocalAppData, "Process")
    [Environment]::SetEnvironmentVariable("TEMP", $isolatedTemp, "Process")
    [Environment]::SetEnvironmentVariable("TMP", $isolatedTemp, "Process")
    [Environment]::SetEnvironmentVariable(
        "WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS",
        "--remote-debugging-port=$CdpPort --remote-allow-origins=*",
        "Process"
    )

    $install = Start-Process -FilePath $installer -ArgumentList @('/S', "/D=$installRoot") -WindowStyle Hidden -Wait -PassThru
    if ($install.ExitCode -ne 0) { throw "installer exit $($install.ExitCode)" }
    $appExe = Join-Path $installRoot "trae-sync.exe"
    if (-not (Test-Path -LiteralPath $appExe -PathType Leaf)) { throw "安装目录缺少 trae-sync.exe" }

    # 通过 CDP 等待 WebView2 真正可检查，避免只看窗口标题造成假阳性。
    $app = Start-Process -FilePath $appExe -PassThru
    $cdpDeadline = [DateTime]::UtcNow.AddSeconds(30)
    $runtime = $null
    do {
        try {
            $runtime = Invoke-RestMethod -Uri "http://127.0.0.1:$CdpPort/json/version" -TimeoutSec 2
            break
        } catch {
            Start-Sleep -Milliseconds 500
        }
    } while ([DateTime]::UtcNow -lt $cdpDeadline)
    if (-not $runtime) { throw "WebView2 CDP 未在 30 秒内就绪" }

    $verifier = Join-Path $repositoryRoot "scripts\verify-packaged-ui.mjs"
    & node $verifier --cdp-port $CdpPort --evidence-root $resolvedRoot
    if ($LASTEXITCODE -ne 0) { throw "CDP 安装包 UI 检查失败，详见 packaged-ui-cdp-report.json" }

    # CDP 检查完成后再走正常窗口关闭和卸载流程。
    $app.Refresh()
    $windowTitle = $app.MainWindowTitle
    $closeRequested = $app.CloseMainWindow()
    if (-not $closeRequested) { throw "应用主窗口拒绝关闭请求" }
    Wait-Process -Id $app.Id -Timeout 10 -ErrorAction SilentlyContinue
    $closed = -not (Get-Process -Id $app.Id -ErrorAction SilentlyContinue)
    if (-not $closed) { throw "应用未在 10 秒内关闭" }
    $app = $null

    $uninstaller = Join-Path $installRoot "uninstall.exe"
    if (-not (Test-Path -LiteralPath $uninstaller -PathType Leaf)) { throw "安装目录缺少卸载器" }
    $uninstall = Start-Process -FilePath $uninstaller -ArgumentList '/S' -WindowStyle Hidden -Wait -PassThru
    if ($uninstall.ExitCode -ne 0) { throw "uninstaller exit $($uninstall.ExitCode)" }
    $installRemoved = -not (Test-Path -LiteralPath $installRoot)
    if (-not $installRemoved) { throw "卸载后安装目录仍存在" }

    $webViewRoots = @(
        (Join-Path ${env:ProgramFiles(x86)} "Microsoft\EdgeWebView\Application"),
        (Join-Path ${env:ProgramFiles} "Microsoft\EdgeWebView\Application")
    ) | Where-Object { $_ -and (Test-Path -LiteralPath $_) }
    $webViewVersions = @(
        foreach ($root in $webViewRoots) {
            Get-ChildItem -LiteralPath $root -Directory -ErrorAction SilentlyContinue | Select-Object -ExpandProperty Name
        }
    ) | Sort-Object -Unique

    $report = [ordered]@{
        verdict = "CURRENT_MACHINE_PREVALIDATED"
        public_rq02 = "OPEN"
        rq03 = "UNSIGNED_CONTROLLED_USE"
        installer = [ordered]@{ path = $installer; bytes = [int64]$installerItem.Length; sha256 = $installerHash }
        webview2 = [ordered]@{ cdp_browser = $runtime.Browser; installed_versions = @($webViewVersions) }
        window = [ordered]@{ title = $windowTitle; close_requested = $closeRequested; closed = $closed }
        lifecycle = [ordered]@{ install_exit = $install.ExitCode; uninstall_exit = $uninstall.ExitCode; install_removed = $installRemoved }
        isolation = [ordered]@{ profile_root = $profileRoot; appdata = $isolatedAppData; localappdata = $isolatedLocalAppData; profile_removed = $profileRemoved; source_access = "not_attempted" }
        source_database_write = "NOT_ATTEMPTED"
        signature = "NOT_SIGNED_BY_POLICY"
    }
    $report | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $reportPath -Encoding utf8
    Write-Output ($report | ConvertTo-Json -Depth 8)
} catch {
    [ordered]@{ verdict = "FAIL"; error = ($_ | Out-String).Trim(); installer = $installer; sha256 = $installerHash } |
        ConvertTo-Json -Depth 5 | Set-Content -LiteralPath $reportPath -Encoding utf8
    throw
} finally {
    # 即使检查失败也只清理本轮隔离 Profile，不触碰真实 TRAE 路径。
    if ($app) {
        Stop-Process -Id $app.Id -Force -ErrorAction SilentlyContinue
    }
    foreach ($name in @("APPDATA", "LOCALAPPDATA", "TEMP", "TMP", "WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS")) {
        [Environment]::SetEnvironmentVariable($name, $originalEnvironment[$name], "Process")
    }
    if (Test-PathWithin $profileRoot $resolvedRoot) {
        Remove-Item -LiteralPath $profileRoot -Recurse -Force -ErrorAction SilentlyContinue
    }
    $profileRemoved = -not (Test-Path -LiteralPath $profileRoot)
    if (Test-Path -LiteralPath $reportPath -PathType Leaf) {
        $finalReport = Get-Content -LiteralPath $reportPath -Raw | ConvertFrom-Json
        if ($finalReport.isolation) {
            $finalReport.isolation | Add-Member -NotePropertyName profile_removed -NotePropertyValue $profileRemoved -Force
        }
        $finalReport | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $reportPath -Encoding utf8
    }
}
