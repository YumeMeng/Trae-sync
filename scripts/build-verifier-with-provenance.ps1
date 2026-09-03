param(
    [Parameter(Mandatory = $true)]
    [string]$VerifierRoot,
    [Parameter(Mandatory = $true)]
    [string]$RepositoryRoot,
    [Parameter(Mandatory = $true)]
    [string]$OutputDirectory,
    [string]$BinaryName = "traesync-catalog-verifier",
    [ValidateSet("debug", "release")]
    [string]$Profile = "release"
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest
# Cargo 会把编译进度和警告写到 stderr；PowerShell 不应把这些正常输出当成脚本异常。
$PSNativeCommandUseErrorActionPreference = $false

$utf8 = New-Object System.Text.UTF8Encoding($false)

function Write-JsonFile {
    param(
        [Parameter(Mandatory = $true)]
        [object]$Value,
        [Parameter(Mandatory = $true)]
        [string]$Path
    )

    $json = $Value | ConvertTo-Json -Depth 12
    [System.IO.File]::WriteAllText($Path, $json + [Environment]::NewLine, $utf8)
}

function Test-PathInside {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Candidate,
        [Parameter(Mandatory = $true)]
        [string]$Root
    )

    $rootWithSeparator = $Root.TrimEnd('\', '/') + [System.IO.Path]::DirectorySeparatorChar
    return $Candidate.Equals($Root, [System.StringComparison]::OrdinalIgnoreCase) -or
        $Candidate.StartsWith($rootWithSeparator, [System.StringComparison]::OrdinalIgnoreCase)
}

function Get-SourceLabel {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Path,
        [Parameter(Mandatory = $true)]
        [string]$Verifier,
        [Parameter(Mandatory = $true)]
        [string]$Repository
    )

    if (Test-PathInside -Candidate $Path -Root $Verifier) {
        $relative = $Path.Substring($Verifier.Length).TrimStart('\', '/').Replace('\', '/')
        return "verifier/$relative"
    }
    if (Test-PathInside -Candidate $Path -Root $Repository) {
        $relative = $Path.Substring($Repository.Length).TrimStart('\', '/').Replace('\', '/')
        return "repository/$relative"
    }
    throw "verifier_source_outside_declared_roots"
}

function Get-SourceManifest {
    param(
        [Parameter(Mandatory = $true)]
        [System.IO.FileInfo[]]$Files,
        [Parameter(Mandatory = $true)]
        [string]$Verifier,
        [Parameter(Mandatory = $true)]
        [string]$Repository
    )

    $records = foreach ($file in $Files) {
        $metadata = Get-Item -LiteralPath $file.FullName -Force
        if ($metadata.LinkType) {
            throw "verifier_source_symlink_rejected"
        }
        $hash = Get-FileHash -LiteralPath $file.FullName -Algorithm SHA256
        [ordered]@{
            path = Get-SourceLabel -Path $file.FullName -Verifier $Verifier -Repository $Repository
            bytes = [int64]$file.Length
            sha256 = $hash.Hash.ToLowerInvariant()
        }
    }
    return @($records | Sort-Object path)
}

function Compare-SourceManifest {
    param(
        [Parameter(Mandatory = $true)]
        [object[]]$Before,
        [Parameter(Mandatory = $true)]
        [object[]]$After
    )

    $beforeMap = @{}
    foreach ($item in $Before) { $beforeMap[$item.path] = $item }
    $afterMap = @{}
    foreach ($item in $After) { $afterMap[$item.path] = $item }

    $allPaths = @($beforeMap.Keys + $afterMap.Keys | Sort-Object -Unique)
    $changes = foreach ($path in $allPaths) {
        if (-not $beforeMap.ContainsKey($path)) {
            [ordered]@{ path = $path; change = "added" }
        } elseif (-not $afterMap.ContainsKey($path)) {
            [ordered]@{ path = $path; change = "removed" }
        } elseif ($beforeMap[$path].bytes -ne $afterMap[$path].bytes -or
            $beforeMap[$path].sha256 -ne $afterMap[$path].sha256) {
            [ordered]@{ path = $path; change = "modified" }
        }
    }
    return @($changes)
}

function Get-SourceFiles {
    param(
        [Parameter(Mandatory = $true)]
        [string[]]$PackageRoots,
        [Parameter(Mandatory = $true)]
        [string[]]$ExtraInputs
    )

    $files = New-Object System.Collections.Generic.List[System.IO.FileInfo]
    foreach ($packageRoot in $PackageRoots) {
        $pending = New-Object System.Collections.Generic.Stack[System.IO.DirectoryInfo]
        $pending.Push((Get-Item -LiteralPath $packageRoot))
        while ($pending.Count -gt 0) {
            $directory = $pending.Pop()
            foreach ($item in Get-ChildItem -LiteralPath $directory.FullName -Force) {
                if ($item.PSIsContainer -and ($item.Name -eq "target" -or $item.Name -eq ".git")) {
                    continue
                }
                if (($item.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) {
                    throw "verifier_source_symlink_rejected"
                }
                if ($item.PSIsContainer) {
                    $pending.Push($item)
                } else {
                    $files.Add($item)
                }
            }
        }
    }
    foreach ($path in $ExtraInputs) {
        if (Test-Path -LiteralPath $path -PathType Leaf) {
            $files.Add((Get-Item -LiteralPath $path))
        }
    }
    return @(
        $files |
            Group-Object FullName |
            ForEach-Object { $_.Group[0] } |
            Sort-Object FullName
    )
}

$resolvedVerifierRoot = (Resolve-Path -LiteralPath $VerifierRoot).Path.TrimEnd('\', '/')
$resolvedRepositoryRoot = (Resolve-Path -LiteralPath $RepositoryRoot).Path.TrimEnd('\', '/')
$resolvedOutputDirectory = [System.IO.Path]::GetFullPath($OutputDirectory)
$manifestPath = Join-Path $resolvedVerifierRoot "Cargo.toml"
$lockPath = Join-Path $resolvedVerifierRoot "Cargo.lock"

if (-not (Test-Path -LiteralPath $manifestPath -PathType Leaf) -or
    -not (Test-Path -LiteralPath $lockPath -PathType Leaf)) {
    throw "verifier_cargo_inputs_missing"
}
if (Test-PathInside -Candidate $resolvedOutputDirectory -Root $resolvedVerifierRoot) {
    throw "verifier_output_must_be_outside_source_root"
}
if (Test-Path -LiteralPath $resolvedOutputDirectory) {
    if (@(Get-ChildItem -LiteralPath $resolvedOutputDirectory -Force).Count -ne 0) {
        throw "verifier_output_directory_not_empty"
    }
} else {
    New-Item -ItemType Directory -Path $resolvedOutputDirectory | Out-Null
}

$metadataJson = & cargo metadata --format-version=1 --locked --manifest-path $manifestPath
if ($LASTEXITCODE -ne 0) {
    throw "verifier_cargo_metadata_failed"
}
$metadata = ($metadataJson | Out-String) | ConvertFrom-Json
$localPackageRoots = @(
    $metadata.packages |
        Where-Object { $null -eq $_.source } |
        ForEach-Object { Split-Path -Parent $_.manifest_path } |
        Sort-Object -Unique
)

foreach ($packageRoot in $localPackageRoots) {
    $resolvedPackageRoot = (Resolve-Path -LiteralPath $packageRoot).Path.TrimEnd('\', '/')
    if (-not (Test-PathInside -Candidate $resolvedPackageRoot -Root $resolvedVerifierRoot) -and
        -not (Test-PathInside -Candidate $resolvedPackageRoot -Root $resolvedRepositoryRoot)) {
        throw "verifier_local_dependency_outside_declared_roots"
    }
}

$extraCargoInputs = @(
    $lockPath,
    (Join-Path $resolvedRepositoryRoot "src-tauri\Cargo.toml"),
    (Join-Path $resolvedRepositoryRoot "src-tauri\Cargo.lock"),
    (Join-Path $resolvedRepositoryRoot ".cargo\config.toml"),
    (Join-Path $resolvedRepositoryRoot "rust-toolchain.toml"),
    (Join-Path $resolvedRepositoryRoot "rust-toolchain")
)
$uniqueFiles = @(Get-SourceFiles -PackageRoots $localPackageRoots -ExtraInputs $extraCargoInputs)
$before = @(Get-SourceManifest -Files $uniqueFiles -Verifier $resolvedVerifierRoot -Repository $resolvedRepositoryRoot)
$beforePath = Join-Path $resolvedOutputDirectory "source-inputs.before.json"
Write-JsonFile -Path $beforePath -Value ([ordered]@{
    format_version = 1
    generated_at_utc = [DateTime]::UtcNow.ToString("o")
    file_count = $before.Count
    files = $before
})

$cargoVersion = (& cargo -V | Out-String).Trim()
$rustcVersion = (& rustc -Vv | Out-String).Trim()
$targetDirectory = Join-Path $resolvedOutputDirectory "target"
$buildArguments = @("build", "--locked", "--manifest-path", $manifestPath, "--target-dir", $targetDirectory)
if ($Profile -eq "release") {
    $buildArguments += "--release"
}
$buildCommand = "cargo " + (($buildArguments | ForEach-Object {
    if ($_ -match '\s') { '"' + $_.Replace('"', '\"') + '"' } else { $_ }
}) -join " ")

$buildLogPath = Join-Path $resolvedOutputDirectory "build.log"
$buildOutput = & cargo @buildArguments 2>&1
$buildExitCode = $LASTEXITCODE
[System.IO.File]::WriteAllLines($buildLogPath, @($buildOutput | ForEach-Object { $_.ToString() }), $utf8)

$afterFiles = @(Get-SourceFiles -PackageRoots $localPackageRoots -ExtraInputs $extraCargoInputs)
$after = @(Get-SourceManifest -Files $afterFiles -Verifier $resolvedVerifierRoot -Repository $resolvedRepositoryRoot)
$afterPath = Join-Path $resolvedOutputDirectory "source-inputs.after.json"
Write-JsonFile -Path $afterPath -Value ([ordered]@{
    format_version = 1
    generated_at_utc = [DateTime]::UtcNow.ToString("o")
    file_count = $after.Count
    files = $after
})
$drift = @(Compare-SourceManifest -Before $before -After $after)
$driftPath = Join-Path $resolvedOutputDirectory "source-drift.json"
Write-JsonFile -Path $driftPath -Value ([ordered]@{
    format_version = 1
    drift_count = $drift.Count
    changes = $drift
})

$profileDirectory = if ($Profile -eq "release") { "release" } else { "debug" }
$binaryExtension = ".exe"
$builtBinaryPath = Join-Path $targetDirectory "$profileDirectory\$BinaryName$binaryExtension"
$status = "passed"
if ($buildExitCode -ne 0) {
    $status = "build_failed"
} elseif ($drift.Count -ne 0) {
    $status = "source_drift"
} elseif (-not (Test-Path -LiteralPath $builtBinaryPath -PathType Leaf)) {
    $status = "binary_missing"
}

$binaryRecord = $null
if ($status -eq "passed") {
    $frozenDirectory = Join-Path $resolvedOutputDirectory "verifier"
    New-Item -ItemType Directory -Path $frozenDirectory | Out-Null
    $frozenBinaryPath = Join-Path $frozenDirectory "$BinaryName$binaryExtension"
    Copy-Item -LiteralPath $builtBinaryPath -Destination $frozenBinaryPath
    $frozenBinary = Get-Item -LiteralPath $frozenBinaryPath
    $frozenHash = Get-FileHash -LiteralPath $frozenBinaryPath -Algorithm SHA256
    $binaryRecord = [ordered]@{
        path = "verifier/$BinaryName$binaryExtension"
        bytes = [int64]$frozenBinary.Length
        sha256 = $frozenHash.Hash.ToLowerInvariant()
    }
    $builtPdbPath = Join-Path $targetDirectory "$profileDirectory\$BinaryName.pdb"
    if (Test-Path -LiteralPath $builtPdbPath -PathType Leaf) {
        Copy-Item -LiteralPath $builtPdbPath -Destination (Join-Path $frozenDirectory "$BinaryName.pdb")
    }
}

$beforeHash = Get-FileHash -LiteralPath $beforePath -Algorithm SHA256
$afterHash = Get-FileHash -LiteralPath $afterPath -Algorithm SHA256
$provenancePath = Join-Path $resolvedOutputDirectory "verifier-provenance.json"
$provenance = [ordered]@{
    format_version = 1
    generated_at_utc = [DateTime]::UtcNow.ToString("o")
    status = $status
    verifier = [ordered]@{
        binary_name = $BinaryName
        profile = $Profile
    }
    toolchain = [ordered]@{
        cargo = $cargoVersion
        rustc = $rustcVersion
    }
    build = [ordered]@{
        command = $buildCommand
        arguments = $buildArguments
        exit_code = $buildExitCode
        log = "build.log"
    }
    source = [ordered]@{
        before_manifest = "source-inputs.before.json"
        before_manifest_sha256 = $beforeHash.Hash.ToLowerInvariant()
        after_manifest = "source-inputs.after.json"
        after_manifest_sha256 = $afterHash.Hash.ToLowerInvariant()
        file_count = $before.Count
        drift_count = $drift.Count
        drift_report = "source-drift.json"
    }
    binary = $binaryRecord
}
Write-JsonFile -Path $provenancePath -Value $provenance

Write-Output "VERIFIER_PROVENANCE=$provenancePath"
Write-Output "VERIFIER_STATUS=$status"
Write-Output "SOURCE_FILE_COUNT=$($before.Count)"
Write-Output "SOURCE_DRIFT_COUNT=$($drift.Count)"
if ($binaryRecord) {
    Write-Output "VERIFIER_SHA256=$($binaryRecord.sha256)"
}

if ($status -ne "passed") {
    throw "verifier_provenance_failed:$status"
}
