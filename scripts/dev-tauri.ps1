param(
    [Parameter(ValueFromRemainingArguments = $true)]
    [string[]] $TauriArgument
)

$ErrorActionPreference = "Stop"

# 开发桌面始终使用仓库内独立运行根，避免读取真实 TRAE 或污染正式 Trae Sync 数据。
# 使用脚本自身路径定位仓库；某些 pnpm/PowerShell 组合下当前 Location 为空。
$scriptPath = $PSCommandPath
if ([string]::IsNullOrEmpty($scriptPath)) { $scriptPath = $MyInvocation.MyCommand.Path }
$scriptDirectory = Split-Path -Parent $scriptPath
$repoRoot = Split-Path -Parent $scriptDirectory
$devRoot = Join-Path $repoRoot ".scratch\dev-tauri"
$appData = Join-Path $devRoot "appdata"
$localAppData = Join-Path $devRoot "localappdata"
$temp = Join-Path $devRoot "temp"

foreach ($directory in @($appData, $localAppData, $temp)) {
    New-Item -ItemType Directory -Force -Path $directory | Out-Null
}

$env:APPDATA = $appData
$env:LOCALAPPDATA = $localAppData
$env:TEMP = $temp
$env:TMP = $temp

# 外部环境若残留 fixture 变量，清除后才进入隔离的空生产预览模式。
Remove-Item Env:TRAE_SYNC_FIXTURE_RAW_KEY -ErrorAction SilentlyContinue
Remove-Item Env:TRAE_SYNC_FIXTURE_STORAGE_ROOT -ErrorAction SilentlyContinue

& pnpm tauri dev @TauriArgument
exit $LASTEXITCODE
