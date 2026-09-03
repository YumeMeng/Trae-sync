param(
    [string]$InstallerPath = (Join-Path (Resolve-Path (Join-Path $PSScriptRoot "..")) "artifacts\Trae Sync_0.2.1_x64-setup-20260819-current.exe"),
    [string]$SmokeRoot = (Join-Path $env:TEMP "trae-sync-0.2.1-installer-smoke")
)

$ErrorActionPreference = "Stop"
$repositoryRoot = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
$resolvedSmokeRoot = [System.IO.Path]::GetFullPath($SmokeRoot)
$allowedSmokeRoots = @(
    [System.IO.Path]::GetFullPath($env:TEMP),
    [System.IO.Path]::GetFullPath((Join-Path $repositoryRoot ".scratch"))
)

function Test-PathWithin([string]$Path, [string]$Root) {
    $normalizedPath = [System.IO.Path]::GetFullPath($Path).TrimEnd('\')
    $normalizedRoot = [System.IO.Path]::GetFullPath($Root).TrimEnd('\')
    return $normalizedPath.StartsWith(
        $normalizedRoot + [System.IO.Path]::DirectorySeparatorChar,
        [System.StringComparison]::OrdinalIgnoreCase
    )
}

if (-not ($allowedSmokeRoots | Where-Object { Test-PathWithin $resolvedSmokeRoot $_ })) {
    throw "SmokeRoot 必须位于系统临时目录或仓库 .scratch 目录内"
}

$SmokeRoot = $resolvedSmokeRoot
$installRoot = Join-Path $SmokeRoot "install"
$logPath = Join-Path $SmokeRoot "installer-smoke.log"
$profileRoot = Join-Path $SmokeRoot "profile"
$isolatedAppData = Join-Path $profileRoot "Roaming"
$isolatedLocalAppData = Join-Path $profileRoot "Local"
$isolatedTemp = Join-Path $profileRoot "Temp"
$appProcess = $null
$originalEnvironment = @{}

# 删除前先固定并验证绝对路径，避免错误参数扩大清理范围。
Remove-Item -LiteralPath $SmokeRoot -Recurse -Force -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force -Path $SmokeRoot | Out-Null
New-Item -ItemType Directory -Force -Path $isolatedAppData, $isolatedLocalAppData, $isolatedTemp | Out-Null

function Write-SmokeLog([string]$Message) {
    $Message | Add-Content -LiteralPath $logPath -Encoding utf8
}

function Wait-MainWindow([int]$ProcessId, [int]$TimeoutSeconds = 20) {
    $deadline = [DateTime]::UtcNow.AddSeconds($TimeoutSeconds)
    do {
        $process = Get-Process -Id $ProcessId -ErrorAction SilentlyContinue
        if (-not $process) {
            throw "app exited before creating its main window"
        }
        $process.Refresh()
        if ($process.MainWindowHandle -ne 0 -and $process.MainWindowTitle -eq "Trae Sync") {
            return $process
        }
        Start-Sleep -Milliseconds 250
    } while ([DateTime]::UtcNow -lt $deadline)

    throw "app main window did not become ready"
}

try {
    # 安装包、应用和卸载器只继承隔离 Profile，禁止接触当前用户真实应用数据目录。
    foreach ($name in @("APPDATA", "LOCALAPPDATA", "TEMP", "TMP")) {
        $originalEnvironment[$name] = [Environment]::GetEnvironmentVariable($name, "Process")
    }
    [Environment]::SetEnvironmentVariable("APPDATA", $isolatedAppData, "Process")
    [Environment]::SetEnvironmentVariable("LOCALAPPDATA", $isolatedLocalAppData, "Process")
    [Environment]::SetEnvironmentVariable("TEMP", $isolatedTemp, "Process")
    [Environment]::SetEnvironmentVariable("TMP", $isolatedTemp, "Process")

    $installer = (Resolve-Path -LiteralPath $InstallerPath).Path
    Write-SmokeLog("INSTALLER=$installer")
    Write-SmokeLog("PROFILE_ISOLATED=True")
    Write-SmokeLog("APPDATA=$isolatedAppData")
    Write-SmokeLog("LOCALAPPDATA=$isolatedLocalAppData")

    # NSIS 静默安装到临时目录，避免触碰默认用户安装位置。
    $installProcess = Start-Process -FilePath $installer -ArgumentList @('/S', "/D=$installRoot") -Wait -PassThru
    Write-SmokeLog("INSTALL_EXIT=$($installProcess.ExitCode)")
    if ($installProcess.ExitCode -ne 0) {
        throw "installer exit $($installProcess.ExitCode)"
    }

    $appExe = Get-ChildItem -LiteralPath $installRoot -Filter '*.exe' -File -Recurse |
        Where-Object { $_.Name -notmatch '^uninstall' } |
        Select-Object -First 1
    if (-not $appExe) {
        throw "installed app executable not found"
    }
    Write-SmokeLog("APP_EXE=$($appExe.FullName)")

    $appProcess = Start-Process -FilePath $appExe.FullName -PassThru
    $appProcess = Wait-MainWindow -ProcessId $appProcess.Id
    Write-SmokeLog("APP_STARTED=True")
    Write-SmokeLog("WINDOW_TITLE=$($appProcess.MainWindowTitle)")

    $closeRequested = $appProcess.CloseMainWindow()
    Write-SmokeLog("CLOSE_REQUESTED=$closeRequested")
    if (-not $closeRequested) {
        throw "app main window rejected the close request"
    }
    Wait-Process -Id $appProcess.Id -Timeout 10 -ErrorAction SilentlyContinue
    $closed = -not (Get-Process -Id $appProcess.Id -ErrorAction SilentlyContinue)
    Write-SmokeLog("APP_CLOSED=$closed")
    if (-not $closed) {
        throw "app did not close"
    }

    $isolatedApplicationRoot = Join-Path $isolatedLocalAppData "Trae Sync"
    $usedIsolatedProfile = Test-Path -LiteralPath $isolatedApplicationRoot -PathType Container
    Write-SmokeLog("ISOLATED_APP_DATA_CREATED=$usedIsolatedProfile")
    if (-not $usedIsolatedProfile) {
        throw "app did not initialize inside isolated LOCALAPPDATA"
    }

    $uninstaller = Get-ChildItem -LiteralPath $installRoot -Filter 'uninstall*.exe' -File -Recurse |
        Select-Object -First 1
    if (-not $uninstaller) {
        throw "uninstaller not found"
    }

    $uninstallProcess = Start-Process -FilePath $uninstaller.FullName -ArgumentList '/S' -Wait -PassThru
    Write-SmokeLog("UNINSTALL_EXIT=$($uninstallProcess.ExitCode)")
    if ($uninstallProcess.ExitCode -ne 0) {
        throw "uninstaller exit $($uninstallProcess.ExitCode)"
    }

    $deadline = [DateTime]::UtcNow.AddSeconds(10)
    do {
        $installStillExists = Test-Path -LiteralPath $installRoot
        if (-not $installStillExists) {
            break
        }
        Start-Sleep -Milliseconds 250
    } while ([DateTime]::UtcNow -lt $deadline)
    Write-SmokeLog("INSTALL_DIR_EXISTS=$installStillExists")
    if ($installStillExists) {
        throw "install directory still exists after uninstall"
    }
    if (-not (Test-PathWithin $profileRoot $SmokeRoot)) {
        throw "isolated profile escaped SmokeRoot"
    }
    Remove-Item -LiteralPath $profileRoot -Recurse -Force
    Write-SmokeLog("ISOLATED_PROFILE_REMOVED=$(-not (Test-Path -LiteralPath $profileRoot))")
    Write-SmokeLog("SMOKE=PASS")
} catch {
    Write-SmokeLog("SMOKE=FAIL")
    Write-SmokeLog(($_ | Out-String).Trim())
    throw
} finally {
    if ($appProcess) {
        Get-Process -Id $appProcess.Id -ErrorAction SilentlyContinue |
            Stop-Process -Force -ErrorAction SilentlyContinue
    }
    foreach ($name in @("APPDATA", "LOCALAPPDATA", "TEMP", "TMP")) {
        [Environment]::SetEnvironmentVariable($name, $originalEnvironment[$name], "Process")
    }
}

Get-Content -LiteralPath $logPath
