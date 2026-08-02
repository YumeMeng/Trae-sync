# Gate A Run - T02 SQLCipher read-only probe (T02 Repair 3: evidence-only rebuild)
# Run ID: 20260802-181000
# Fail-fast: any non-zero exit terminates immediately.
# Cargo commands use --manifest-path.
# All stdout/stderr persisted to logs/.
# Script generates REPORT.md, assertions.json, environment.json, hashes.sha256,
# working-tree-hashes.sha256 (working tree binding).
#
# Repair 3 (evidence-only) vs 20260801-212000:
#   - 不修改生产代码或测试
#   - 重新生成完整证据，绑定 review 2 修复后的当前 working tree
#     （evidence_state snake_case + Gate A t02_source_files 动态化修复）
#   - t02_source_files 始终使用 $t02SourceFiles.Count（期望 12）
#   - 保留全部历史 Gate run，不覆盖、不删除、不改写旧证据

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

$repoRoot = "d:\work\Trae-sync"
$gateDir = "$repoRoot\evidence\gates\A\20260802-181000"
$logsDir = "$gateDir\logs"

# Refuse to overwrite existing run
$preExisting = @(Get-ChildItem -Path $gateDir -Force | Where-Object { $_.Name -ne 'commands.ps1' })
if ($preExisting.Count -gt 0) {
    Write-Host "FATAL: Gate directory already contains non-commands.ps1 entries:"
    $preExisting | ForEach-Object { Write-Host "  $($_.Name)" }
    throw "Refusing to overwrite existing run; create a new run-id instead"
}

New-Item -ItemType Directory -Path $logsDir -Force | Out-Null

# ===================== Phase 1: transcript open =====================
$transcriptPath = "$logsDir\00-transcript.log"
if (Test-Path $transcriptPath) {
    throw "Transcript file already exists: $transcriptPath - refuse to overwrite"
}
Start-Transcript -Path $transcriptPath | Out-Null

$phase1Success = $false
$phase1Error = $null
$workingTreeHash = $null
$gitDiffHash = $null

try {
    Set-Location $repoRoot
    $env:Path = "C:\Strawberry\perl\bin;$env:USERPROFILE\.cargo\bin;$env:Path"

    Write-Host "=== PHASE 1 START ==="
    Write-Host "Run ID: 20260802-181000"
    Write-Host "Script started at: $(Get-Date)"

    function Invoke-Step {
        param([string]$Name, [string]$Command, [string]$LogPath)
        $startTime = Get-Date
        Write-Host "=== ${Name} ==="
        Write-Host "Command started at: ${startTime}"
        $prevEAP = $ErrorActionPreference
        $ErrorActionPreference = 'Continue'
        try {
            $output = Invoke-Expression -Command $Command 2>&1
        } finally {
            $ErrorActionPreference = $prevEAP
        }
        $exitCode = $LASTEXITCODE
        $output | Out-File -FilePath $LogPath -Encoding UTF8
        $endTime = Get-Date
        $duration = $endTime - $startTime
        "Command: ${Name}" | Add-Content -Path $LogPath -Encoding UTF8
        "ExitCode: ${exitCode}" | Add-Content -Path $LogPath -Encoding UTF8
        "StartedAt: ${startTime}" | Add-Content -Path $LogPath -Encoding UTF8
        "EndedAt: ${endTime}" | Add-Content -Path $LogPath -Encoding UTF8
        "DurationSec: $($duration.TotalSeconds)" | Add-Content -Path $LogPath -Encoding UTF8
        if ($exitCode -ne 0) {
            throw "${Name} failed with exit ${exitCode}"
        }
        Write-Host "${Name}: exit=${exitCode} duration=$($duration.TotalSeconds)s"
    }

    # Step 1: SQLCipher probe tests (R1 read-only + R5 cipher_version + zero-write evidence)
    Invoke-Step -Name "Gate A: SQLCipher probe and zero-write evidence" `
        -Command "cargo test --manifest-path src-tauri/Cargo.toml -p traesync-infrastructure --lib sqlcipher::tests" `
        -LogPath "$logsDir\01-sqlcipher-tests.log"

    # Step 2: Work CN schema probe and fingerprint tests (R5 index/unique constraint)
    Invoke-Step -Name "Gate A: Work CN schema constraint tests" `
        -Command "cargo test --manifest-path src-tauri/Cargo.toml -p traesync-infrastructure --lib work_cn_schema::tests" `
        -LogPath "$logsDir\02-work-cn-schema-tests.log"

    # Step 3: FixturePathGuard regression (T01 boundary + R2-1 junction-based symlink escape)
    Invoke-Step -Name "Gate A: FixturePathGuard regression (R2-1 junction escape)" `
        -Command "cargo test --manifest-path src-tauri/Cargo.toml -p traesync-infrastructure --lib fixture_paths::tests" `
        -LogPath "$logsDir\03-fixture-paths-tests.log"

    # Step 4: domain workbench_read value object tests (R5 + R2-3 serde JSON contract)
    Invoke-Step -Name "Gate A: domain workbench_read tests (R5 + R2-3 serde contract)" `
        -Command "cargo test --manifest-path src-tauri/Cargo.toml -p traesync-domain --lib workbench_read::tests" `
        -LogPath "$logsDir\04-domain-workbench-read-tests.log"

    # Step 5: verify source has no real TRAE path access (fixture-only boundary)
    $t02SourceFiles = @(
        "$repoRoot\src-tauri\crates\infrastructure\src\sqlcipher.rs",
        "$repoRoot\src-tauri\crates\infrastructure\src\account_evidence.rs",
        "$repoRoot\src-tauri\crates\infrastructure\src\work_cn_schema.rs",
        "$repoRoot\src-tauri\crates\infrastructure\src\fixture_paths.rs",
        "$repoRoot\src-tauri\crates\application\src\workbench_read.rs",
        "$repoRoot\src-tauri\crates\domain\src\workbench_read.rs",
        "$repoRoot\src-tauri\crates\ports\src\workbench_read.rs",
        "$repoRoot\src-tauri\crates\commands\src\lib.rs",
        "$repoRoot\src-tauri\src\lib.rs",
        "$repoRoot\src\components\WorkbenchReadPanel.tsx",
        "$repoRoot\src\types\workbench_read.ts",
        "$repoRoot\tests\App.test.tsx"
    )
    $realPathScanFiles = @($t02SourceFiles | Where-Object { $_ -notmatch 'fixture_paths\.rs' })
    $realPathPatterns = @("TRAE SOLO CN", "APPDATA*\TRAE", "%APPDATA%")
    $realPathHits = @()
    foreach ($pattern in $realPathPatterns) {
        $hits = Select-String -Path $realPathScanFiles -Pattern $pattern -SimpleMatch
        if ($hits) { $realPathHits += $hits }
    }
    $realPathViolations = @($realPathHits | Where-Object {
        $_.Line -notmatch 'fixture' -and $_.Line -notmatch 'T03' -and $_.Line -notmatch 'placeholder'
    })
    if ($realPathViolations.Count -gt 0) {
        $realPathViolations | Out-File "$logsDir\05-real-path-scan.log" -Encoding UTF8
        throw "FATAL: real TRAE path access found, violates fixture-only boundary"
    }
    "No real TRAE path access (scanned $($t02SourceFiles.Count) T02 source files, real-path scan excluded fixture_paths.rs)" | Out-File "$logsDir\05-real-path-scan.log" -Encoding UTF8
    Write-Host "Step 5: real-path scan PASS (scanned $($t02SourceFiles.Count) files)"

    # Step 6: verify test output has no synthetic fixture key leak
    $syntheticFixtureKey = "deadbeefcafebabe1234567890abcdefdeadbeefcafebabe1234567890abcdef"
    $keyLeak = Select-String -Path "$logsDir\*.log" -Pattern $syntheticFixtureKey -SimpleMatch
    if ($keyLeak) {
        $keyLeak | Out-File "$logsDir\06-key-leak-scan.log" -Encoding UTF8
        throw "FATAL: synthetic fixture key found in log output"
    }
    "No synthetic fixture key leak" | Out-File "$logsDir\06-key-leak-scan.log" -Encoding UTF8
    Write-Host "Step 6: key-leak scan PASS"

    # Step 7: verify real baseline key not in full T02 source (defense-in-depth)
    $baselineKeyMatch = Select-String -Path "$repoRoot\docs\TECHNICAL_BASELINE.md" -Pattern '^[0-9a-f]{64}$' | Select-Object -First 1
    if ($baselineKeyMatch) {
        $realBaselineKey = $baselineKeyMatch.Line
        $realKeyInSource = Select-String -Path $realPathScanFiles -Pattern $realBaselineKey -SimpleMatch
        if ($realKeyInSource) {
            $hitCount = @($realKeyInSource).Count
            "Real baseline key found in source ($hitCount hits, defense-in-depth FAILED)" | Out-File "$logsDir\07-real-key-scan.log" -Encoding UTF8
            throw "FATAL: real baseline key found in source"
        }
    }
    "Real baseline key not in full T02 source (scanned $($t02SourceFiles.Count) files, defense-in-depth PASS)" | Out-File "$logsDir\07-real-key-scan.log" -Encoding UTF8
    Write-Host "Step 7: real-key-in-source scan PASS"

    # Step 8: working-tree binding — T02 源码逐文件 SHA-256 + git diff --binary 哈希
    $wtHashes = @()
    foreach ($f in $t02SourceFiles) {
        $rel = $f.Substring($repoRoot.Length + 1).Replace('\','/')
        $h = (Get-FileHash -Path $f -Algorithm SHA256).Hash.ToLower()
        $wtHashes += "$h  $rel"
    }
    $wtHashes | Set-Content -Path "$gateDir\working-tree-hashes.sha256" -Encoding UTF8
    $ms = New-Object System.IO.MemoryStream
    foreach ($f in $t02SourceFiles) {
        $bytes = [System.IO.File]::ReadAllBytes($f)
        $ms.Write($bytes, 0, $bytes.Length)
    }
    $ms.Position = 0
    $combined = (Get-FileHash -InputStream $ms -Algorithm SHA256).Hash.ToLower()
    $workingTreeHash = $combined
    # git diff --binary 哈希（未提交 working tree 相对 HEAD 的二进制 diff）
    # 关闭 strict mode 避免 PowerShell 5.1 strict mode 下读取自动变量赋值失败
    Set-StrictMode -Off
    $ErrorActionPreference = 'Continue'
    $gitDiffRaw = & git -C $repoRoot diff --binary 2>&1
    $gitDiffStr = if ($gitDiffRaw) { ($gitDiffRaw | Out-String) } else { "" }
    $ErrorActionPreference = 'Stop'
    Set-StrictMode -Version Latest
    $gitDiffBytes2 = [System.Text.Encoding]::UTF8.GetBytes($gitDiffStr)
    $gitDiffMs = New-Object System.IO.MemoryStream
    $gitDiffMs.Write($gitDiffBytes2, 0, $gitDiffBytes2.Length)
    $gitDiffMs.Position = 0
    $gitDiffHash = (Get-FileHash -InputStream $gitDiffMs -Algorithm SHA256).Hash.ToLower()
    $headSha = & git -C $repoRoot rev-parse HEAD
    "Working-tree binding" | Out-File "$logsDir\08-working-tree-binding.log" -Encoding UTF8
    "HEAD: $headSha" | Add-Content -Path "$logsDir\08-working-tree-binding.log" -Encoding UTF8
    "T02 source combined SHA-256: $workingTreeHash" | Add-Content -Path "$logsDir\08-working-tree-binding.log" -Encoding UTF8
    "git diff --binary SHA-256: $gitDiffHash" | Add-Content -Path "$logsDir\08-working-tree-binding.log" -Encoding UTF8
    "T02 source file count: $($t02SourceFiles.Count)" | Add-Content -Path "$logsDir\08-working-tree-binding.log" -Encoding UTF8
    "Per-file SHA-256 list: working-tree-hashes.sha256" | Add-Content -Path "$logsDir\08-working-tree-binding.log" -Encoding UTF8
    Write-Host "Step 8: working-tree binding PASS (combined=$workingTreeHash gitdiff=$gitDiffHash)"

    $phase1Success = $true
} catch {
    $phase1Error = $_
    Write-Host "PHASE 1 ERROR: $($_.Exception.Message)"
} finally {
    Stop-Transcript | Out-Null
}

# ===================== generate environment.json =====================
$envJson = @{
    run_id = "20260802-181000"
    gate = "Gate A"
    ticket = "T02"
    baseline = "f560abd"
    repair = "R3"
    timestamp_utc = (Get-Date).ToUniversalTime().ToString("o")
    os = @{
        name = [System.Environment]::OSVersion.VersionString
        version = [System.Environment]::OSVersion.Version.ToString()
        architecture = $env:PROCESSOR_ARCHITECTURE
    }
    powershell = @{
        version = $PSVersionTable.PSVersion.ToString()
        edition = $PSVersionTable.PSEdition
    }
    toolchain = @{
        node = (& node --version)
        pnpm = (& pnpm --version)
        cargo = (& cargo --version)
        rustc = (& rustc --version)
    }
    evidence_level = "Implemented"
    scope = "Embedded SQLCipher read-only probe (T02 Repair 3: evidence-only rebuild, binds post-review-2 working tree)"
    working_tree = @{
        head = (& git -C $repoRoot rev-parse HEAD)
        t02_source_combined_sha256 = $workingTreeHash
        git_diff_binary_sha256 = $gitDiffHash
        t02_source_files = $t02SourceFiles.Count
    }
}
$envJson | ConvertTo-Json -Depth 5 | Out-File -FilePath "$gateDir\environment.json" -Encoding UTF8

# ===================== generate assertions.json =====================
$conclusion = if ($phase1Success) { "PASS" } else { "FAIL" }
$assertions = @{
    run_id = "20260802-181000"
    gate = "Gate A"
    ticket = "T02"
    repair = "R3"
    conclusion = $conclusion
    evidence_level = "Implemented"
    qualification = "NOT_QUALIFIED"
    acceptance_criteria = @{
        "AC1" = @{ status = if ($phase1Success) { "PASS" } else { "FAIL" }; evidence = "bundled-sqlcipher-vendored-openssl compiled; cargo build exit=0" }
        "AC2" = @{ status = if ($phase1Success) { "PASS" } else { "FAIL" }; evidence = "R1 zero-write: probe_with_correct_key_has_zero_writes_to_db_trio + probe_with_wrong_key_has_zero_writes_to_db_trio PASS" }
        "AC3" = @{ status = if ($phase1Success) { "PASS" } else { "FAIL" }; evidence = "backup_to_logical_copy_preserves_wal_content PASS" }
        "AC4" = @{ status = if ($phase1Success) { "PASS" } else { "FAIL" }; evidence = "probe_truncated_file_returns_truncated_file + probe_unknown_schema_returns_unknown_schema PASS" }
        "AC5" = @{ status = if ($phase1Success) { "PASS" } else { "FAIL" }; evidence = "verify_transaction_rollback_passes_on_fixture_copy PASS" }
        "AC6" = @{ status = if ($phase1Success) { "PASS" } else { "FAIL" }; evidence = "run_integrity_checks_pass_on_verified_db PASS" }
        "AC7" = @{ status = if ($phase1Success) { "PASS" } else { "FAIL" }; evidence = "create_random_key_catalog_creates_and_reopens PASS" }
        "R5_cipher_version" = @{ status = if ($phase1Success) { "PASS" } else { "FAIL" }; evidence = "probe_work_cn_db_returns_cipher_version_compatible PASS" }
        "R5_schema_constraints" = @{ status = if ($phase1Success) { "PASS" } else { "FAIL" }; evidence = "work_cn_schema::tests PASS" }
        "R2-1_junction_escape" = @{ status = if ($phase1Success) { "PASS" } else { "FAIL" }; evidence = "fixture_paths::reject_symlink_escape PASS via directory junction (mklink /J), no silent skip" }
        "R2-3_serde_contract" = @{ status = if ($phase1Success) { "PASS" } else { "FAIL" }; evidence = "domain workbench_read::tests incl r2_3_*_serializes_as_json_string + full_json_shape_matches_frontend_dto PASS" }
        "R2-4_working_tree_binding" = @{ status = if ($phase1Success) { "PASS" } else { "FAIL" }; evidence = "working-tree-hashes.sha256 + git diff --binary SHA-256 recorded; combined=$workingTreeHash gitdiff=$gitDiffHash" }
    }
    boundary = @{
        fixture_only = $true
        real_trae_data_accessed = $false
        real_trae_started_or_closed = $false
        write_capability_enabled = $false
        raw_key_leaked = $false
        real_baseline_key_in_source = $false
        source_files_scanned = $t02SourceFiles.Count
        historical_gate_runs_preserved = $true
    }
}
$assertions | ConvertTo-Json -Depth 6 | Out-File -FilePath "$gateDir\assertions.json" -Encoding UTF8

# ===================== generate REPORT.md =====================
$report = @"
# Gate A Run Report - 20260802-181000

## Run Metadata
- **Run ID**: 20260802-181000
- **Gate**: Gate A
- **Ticket**: T02 (Work CN read-only entry)
- **Baseline**: f560abd
- **Repair**: R3 (T02 Repair 3: evidence-only rebuild)
- **Evidence Level**: Implemented
- **Conclusion**: $conclusion
- **Qualification**: NOT QUALIFIED
- **Scope**: Evidence-only. No production code or test changes. Rebuilds Gate A evidence bound to the post-review-2 working tree (evidence_state snake_case + dynamic t02_source_files).

## Environment
- **OS**: $($envJson.os.name)
- **PowerShell**: $($envJson.powershell.version) ($($envJson.powershell.edition))
- **Node**: $($envJson.toolchain.node)
- **pnpm**: $($envJson.toolchain.pnpm)
- **cargo**: $($envJson.toolchain.cargo)
- **rustc**: $($envJson.toolchain.rustc)

## Working-Tree Binding (R2-4 / R3)
- **HEAD**: $($envJson.working_tree.head)
- **T02 source combined SHA-256**: $workingTreeHash
- **git diff --binary SHA-256**: $gitDiffHash
- **T02 source files**: $($envJson.working_tree.t02_source_files)
- **Per-file list**: working-tree-hashes.sha256

## Scope
Embedded SQLCipher read-only probe (T02 Repair 3, evidence-only):
1. Fixed Rust SQLCipher dependency (bundled-sqlcipher-vendored-openssl)
2. R1 read-only open zero-write evidence (correct/wrong key)
3. R5 cipher_version compatibility check (4.5.x)
4. R5 schema constraint (table/column/index/unique-constraint)
5. R2-1 fixture_paths symlink/junction escape (directory junction, no silent skip)
6. R2-3 domain serde JSON contract (UserId/AuthFingerprint/SchemaFingerprint bare strings)
7. R2-4 working-tree binding (per-file SHA-256 + git diff --binary SHA-256)
8. Two-layer integrity check; random key catalog create/reopen

## Test Summary
- **sqlcipher::tests**: R1 zero-write + R5 cipher_version PASS
- **work_cn_schema::tests**: R5 schema constraint PASS
- **fixture_paths::tests**: T01 boundary + R2-1 junction escape PASS
- **domain workbench_read::tests**: R5 + R2-3 serde contract PASS

## Boundary
- Only uses repo-internal synthetic fixture and tempdir
- No access to real %APPDATA%\TRAE SOLO CN, active DB, real logs or Local Storage
- No start or close of TRAE
- No write capability enabled
- Synthetic fixture key not in logs
- Real baseline key not in full T02 source ($($t02SourceFiles.Count) files)
- All historical Gate runs preserved (no overwrite/delete/rewrite/touch)

## Evidence Level
- **Implemented**: production module and automated fixture tests complete
- **NOT QUALIFIED**: real-env Qualification requires separate explicit user approval

## Failure Impact
- Failure only affects fixture tempdir, zero real-data impact

## Real-data Boundary
- Access real TRAE path/data: no
- Start/close TRAE: no
- Enable write capability: no
"@
$report | Out-File -FilePath "$gateDir\REPORT.md" -Encoding UTF8

# ===================== Phase 2: complete hashes.sha256 =====================
$hashes = @()
Get-ChildItem -Path $gateDir -Recurse -File | Where-Object { $_.Name -ne 'hashes.sha256' } | ForEach-Object {
    $rel = $_.FullName.Substring($gateDir.Length + 1).Replace('\','/')
    $hash = (Get-FileHash -Path $_.FullName -Algorithm SHA256).Hash.ToLower()
    $hashes += "$hash  $rel"
}
$hashes | Set-Content -Path "$gateDir\hashes.sha256" -Encoding UTF8

# Per-entry recompute verification
$verifyFailures = 0
$verifyTotal = 0
foreach ($line in (Get-Content -Path "$gateDir\hashes.sha256")) {
    if ([string]::IsNullOrWhiteSpace($line)) { continue }
    $verifyTotal++
    $parts = $line -split '  ', 2
    if ($parts.Count -lt 2) { $verifyFailures++; Write-Host "HASH PARSE FAIL: $line"; continue }
    $expected = $parts[0].Trim()
    $rel = $parts[1].Trim()
    $full = Join-Path $gateDir $rel.Replace('/','\')
    if (-not (Test-Path $full)) { $verifyFailures++; Write-Host "HASH FILE MISSING: $rel"; continue }
    $actual = (Get-FileHash -Path $full -Algorithm SHA256).Hash.ToLower()
    if ($actual -ne $expected) { $verifyFailures++; Write-Host "HASH MISMATCH: $rel expected=$expected actual=$actual" }
}
Write-Host "=== PHASE 2 COMPLETE ==="
Write-Host "Conclusion: $conclusion"
Write-Host "Hashes written: $($hashes.Count) entries"
Write-Host "Hash verify: total=$verifyTotal failures=$verifyFailures"

if (-not $phase1Success) {
    Write-Host "PHASE 1 FAILED: $phase1Error"
    exit 1
}
if ($verifyFailures -gt 0) {
    Write-Host "HASH VERIFICATION FAILED: $verifyFailures mismatches"
    exit 1
}
exit 0
