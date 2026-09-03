param(
    [string]$Root,
    [Parameter(Mandatory = $true)]
    [string]$OutputPath,
    [string]$InstallerPath
)

$ErrorActionPreference = "Stop"

if ([string]::IsNullOrWhiteSpace($Root)) {
    $scriptDirectory = $PSScriptRoot
    if ([string]::IsNullOrWhiteSpace($scriptDirectory)) {
        $scriptDirectory = Split-Path -Parent $MyInvocation.MyCommand.Definition
    }
    $Root = Join-Path $scriptDirectory ".."
}

$relativeFiles = @("package.json", "pnpm-lock.yaml", "index.html", "vite.config.ts", "vitest.config.ts", "playwright.config.ts", "tsconfig.json", "tsconfig.app.json", "tsconfig.node.json", "src-tauri/Cargo.toml", "src-tauri/Cargo.lock", "src-tauri/tauri.conf.json", "src-tauri/build.rs", "src-tauri/capabilities/default.json"); # 绑定运行时、依赖和安装包输入。
$rootDirectories = @("src", "e2e", "tests", "src-tauri/src", "src-tauri/crates", "src-tauri/tests", "src-tauri/gen/schemas");

$resolvedRoot = (Resolve-Path $Root).Path
$files = New-Object System.Collections.Generic.List[System.IO.FileInfo]
$utf8 = New-Object System.Text.UTF8Encoding($false)

foreach ($relativeFile in $relativeFiles) {
    $path = Join-Path $resolvedRoot $relativeFile
    if (Test-Path -LiteralPath $path -PathType Leaf) {
        $files.Add((Get-Item -LiteralPath $path))
    }
}

foreach ($relativeRoot in $rootDirectories) {
    $path = Join-Path $resolvedRoot $relativeRoot
    if (-not (Test-Path -LiteralPath $path -PathType Container)) {
        continue
    }
    Get-ChildItem -LiteralPath $path -File -Recurse | Where-Object {
        # 清单拒绝符号链接文件，避免把工作区外内容绑定进发布输入。
        -not $_.LinkType
    } | ForEach-Object { $files.Add($_) }
}

$uniqueFiles = $files |
    Group-Object -Property FullName |
    ForEach-Object { $_.Group[0] } |
    Sort-Object FullName

$manifestFiles = foreach ($file in $uniqueFiles) {
    $relativePath = $file.FullName.Substring($resolvedRoot.Length).TrimStart('\').Replace('\', '/')
    $hash = Get-FileHash -LiteralPath $file.FullName -Algorithm SHA256
    [ordered]@{
        path = $relativePath
        bytes = [int64]$file.Length
        sha256 = $hash.Hash.ToLowerInvariant()
    }
}

$package = [System.IO.File]::ReadAllText((Join-Path $resolvedRoot "package.json"), $utf8) | ConvertFrom-Json
$tauri = [System.IO.File]::ReadAllText((Join-Path $resolvedRoot "src-tauri/tauri.conf.json"), $utf8) | ConvertFrom-Json
$cargoText = [System.IO.File]::ReadAllText((Join-Path $resolvedRoot "src-tauri/Cargo.toml"), $utf8)
$cargoVersion = [regex]::Match($cargoText, '(?ms)^\[package\].*?^version\s*=\s*"([^"]+)"').Groups[1].Value
$gitHead = (& git -C $resolvedRoot rev-parse HEAD 2>$null).Trim()
$gitStatus = (& git -C $resolvedRoot status --short 2>$null | Out-String).Trim()

$installer = $null
if ($InstallerPath) {
    $resolvedInstaller = (Resolve-Path $InstallerPath).Path
    $installerItem = Get-Item -LiteralPath $resolvedInstaller
    $installerHash = Get-FileHash -LiteralPath $resolvedInstaller -Algorithm SHA256
    $installer = [ordered]@{
        path = $resolvedInstaller.Substring($resolvedRoot.Length).TrimStart('\').Replace('\', '/')
        bytes = [int64]$installerItem.Length
        sha256 = $installerHash.Hash.ToLowerInvariant()
    }
}

$result = [ordered]@{
    format_version = 1
    generated_at_utc = [DateTime]::UtcNow.ToString("o")
    root = $resolvedRoot
    git_head = $gitHead
    working_tree_status = $gitStatus
    versions = [ordered]@{
        package_json = $package.version
        tauri = $tauri.version
        cargo = $cargoVersion
    }
    source_scope = "implementation;frontend;rust;tests;lockfiles;build-config;tauri-assets"
    exclusions = "node_modules;src-tauri/target;dist;.scratch;artifacts;evidence;prototypes"
    file_count = @($manifestFiles).Count
    files = @($manifestFiles)
    installer = $installer
}

$outputDirectory = Split-Path -Parent $OutputPath
if ($outputDirectory) {
    New-Item -ItemType Directory -Force -Path $outputDirectory | Out-Null
}
$result | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $OutputPath -Encoding utf8
Write-Output ("PROVENANCE_WRITTEN=" + (Resolve-Path $OutputPath).Path)
Write-Output ("SOURCE_FILE_COUNT=" + $result.file_count)
if ($installer) {
    Write-Output ("INSTALLER_SHA256=" + $installer.sha256)
}
