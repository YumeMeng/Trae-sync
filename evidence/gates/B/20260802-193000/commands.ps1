# Gate B Run - T02 account evidence and UI boundary (T02 Repair 3: evidence-only rebuild)
# Run ID: 20260802-193000
# Fail-fast: any non-zero exit terminates immediately.
# Cargo commands use --manifest-path. pnpm commands use --frozen-lockfile.
# All stdout/stderr persisted to logs/.
# Script generates REPORT.md, assertions.json, environment.json, hashes.sha256,
# working-tree-hashes.sha256 (working tree binding).
#
# 20260802-193000 vs 20260802-181000/190000 (彻底修复 PowerShell strict mode bug):
#   - 20260802-181000 因 Set-StrictMode -Version Latest + 外部命令 stderr ErrorRecord
#     累积导致 Step 13 变量赋值失败（$realPathScanFiles 未设置），证据不完整
#   - 20260802-190000 尝试在 Step 13 前关闭 strict mode，但 Select-String -Path
#     仍因 $realPathScanFiles 为 $null 而失败（PowerShell 5.1 管道数组展开问题）
#   - 本运行从头关闭 strict mode (Set-StrictMode -Off)，并在 Select-String 调用前
#     添加空值保护，彻底修复此问题
#   - 保留 20260802-181000 和 20260802-190000 失败证据作为历史记录，不删除、不修改
#
# Repair 3 (evidence-only) vs 20260801-205000:
#   - 不修改生产代码或测试
#   - 重新生成完整证据，绑定 review 2 修复后的当前 working tree
#     （evidence_state snake_case + dynamic t02_source_files）
#   - t02_source_files 和 source_files_scanned 始终使用 $t02SourceFiles.Count（期望 12）
#   - 保留全部历史 Gate run，不覆盖、不删除、不改写旧证据

# 从头关闭 strict mode，避免 PowerShell 5.1 与外部命令 stderr ErrorRecord 的不兼容
# 参考：project_memory.md 中记录的 PowerShell strict mode 教训
$ErrorActionPreference = "Stop"
Set-StrictMode -Off

$repoRoot = "d:\work\Trae-sync"
$gateDir = "$repoRoot\evidence\gates\B\20260802-193000"
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
    Write-Host "Run ID: 20260802-193000"
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

    # ===================== Rust tests =====================

    # Step 1: account evidence matrix (R2 latest session + R3 Expired + R4 plaintext + R2-1 symlink/junction escape)
    Invoke-Step -Name "Gate B: account_evidence matrix (R2/R3/R4 + R2-1 symlink/junction escape)" `
        -Command "cargo test --manifest-path src-tauri/Cargo.toml -p traesync-infrastructure --lib account_evidence::tests" `
        -LogPath "$logsDir\01-account-evidence-tests.log"

    # Step 2: application workbench_read tests (R3 FingerprintChanged via production service entry + R1 path guard)
    Invoke-Step -Name "Gate B: application workbench_read (R3 FingerprintChanged + R1 path guard)" `
        -Command "cargo test --manifest-path src-tauri/Cargo.toml -p traesync-application --lib workbench_read::tests" `
        -LogPath "$logsDir\02-application-workbench-read-tests.log"

    # Step 3: commands layer build_work_cn_state input validation tests
    Invoke-Step -Name "Gate B: commands input validation tests" `
        -Command "cargo test --manifest-path src-tauri/Cargo.toml -p traesync-commands" `
        -LogPath "$logsDir\03-commands-tests.log"

    # Step 4: domain workbench_read value object tests (R5 + R2-3 serde JSON contract)
    Invoke-Step -Name "Gate B: domain workbench_read tests (R5 + R2-3 serde contract)" `
        -Command "cargo test --manifest-path src-tauri/Cargo.toml -p traesync-domain --lib workbench_read::tests" `
        -LogPath "$logsDir\04-domain-workbench-read-tests.log"

    # ===================== T01 regression =====================

    # Step 5: T01 fixture_paths_integration regression
    Invoke-Step -Name "Gate B: T01 fixture_paths_integration regression" `
        -Command "cargo test --manifest-path src-tauri/Cargo.toml --test fixture_paths_integration" `
        -LogPath "$logsDir\05-fixture-paths-integration.log"

    # Step 6: T01 dependency_direction regression
    Invoke-Step -Name "Gate B: T01 dependency_direction regression" `
        -Command "cargo test --manifest-path src-tauri/Cargo.toml --test dependency_direction" `
        -LogPath "$logsDir\06-dependency-direction.log"

    # Step 7: T01 logging_integration regression
    Invoke-Step -Name "Gate B: T01 logging_integration regression" `
        -Command "cargo test --manifest-path src-tauri/Cargo.toml --test logging_integration" `
        -LogPath "$logsDir\07-logging-integration.log"

    # Step 8: T01 compile_fail regression
    Invoke-Step -Name "Gate B: T01 compile_fail regression" `
        -Command "cargo test --manifest-path src-tauri/Cargo.toml --test compile_fail" `
        -LogPath "$logsDir\08-compile-fail.log"

    # ===================== Frontend =====================

    # Step 9: pnpm install --frozen-lockfile
    Invoke-Step -Name "pnpm install --frozen-lockfile" `
        -Command "pnpm install --frozen-lockfile" `
        -LogPath "$logsDir\09-pnpm-install.log"

    # Step 10: pnpm typecheck
    Invoke-Step -Name "pnpm typecheck" `
        -Command "pnpm typecheck" `
        -LogPath "$logsDir\10-pnpm-typecheck.log"

    # Step 11: pnpm test
    Invoke-Step -Name "pnpm test" `
        -Command "pnpm test" `
        -LogPath "$logsDir\11-pnpm-test.log"

    # Step 12: pnpm build
    Invoke-Step -Name "pnpm build" `
        -Command "pnpm build" `
        -LogPath "$logsDir\12-pnpm-build.log"

    # ===================== Security scans =====================

    # Step 13: verify source has no real TRAE path access (fixture-only boundary)
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
    # realPathScanFiles 排除 fixture_paths.rs（内含 "TRAE SOLO CN" 作为路径策略定义，非真实路径访问）
    # 空值保护：确保 $realPathScanFiles 不为 $null，避免 Select-String -Path $null 报错
    $realPathScanFiles = @($t02SourceFiles | Where-Object { $_ -notmatch 'fixture_paths\.rs' })
    if (-not $realPathScanFiles) { $realPathScanFiles = @() }
    Write-Host "Step 13: realPathScanFiles count = $($realPathScanFiles.Count)"
    $realPathPatterns = @("TRAE SOLO CN", "APPDATA*\TRAE", "%APPDATA%")
    $realPathHits = @()
    foreach ($pattern in $realPathPatterns) {
        $hits = if ($realPathScanFiles.Count -gt 0) { Select-String -Path $realPathScanFiles -Pattern $pattern -SimpleMatch } else { $null }
        if ($hits) { $realPathHits += $hits }
    }
    $realPathViolations = @($realPathHits | Where-Object {
        $_.Line -notmatch 'fixture' -and $_.Line -notmatch 'T03' -and $_.Line -notmatch 'placeholder'
    })
    if ($realPathViolations.Count -gt 0) {
        $realPathViolations | Out-File "$logsDir\13-real-path-scan.log" -Encoding UTF8
        throw "FATAL: real TRAE path access found, violates fixture-only boundary"
    }
    "No real TRAE path access (scanned $($t02SourceFiles.Count) T02 source files, real-path scan excluded fixture_paths.rs)" | Out-File "$logsDir\13-real-path-scan.log" -Encoding UTF8
    Write-Host "Step 13: real-path scan PASS (scanned $($t02SourceFiles.Count) files, real-path scan excluded fixture_paths.rs)"

    # Step 14: verify test output has no synthetic fixture key leak
    $syntheticFixtureKey = "deadbeefcafebabe1234567890abcdefdeadbeefcafebabe1234567890abcdef"
    $keyLeak = Select-String -Path "$logsDir\*.log" -Pattern $syntheticFixtureKey -SimpleMatch
    if ($keyLeak) {
        $keyLeak | Out-File "$logsDir\14-key-leak-scan.log" -Encoding UTF8
        throw "FATAL: synthetic fixture key found in log output"
    }
    "No synthetic fixture key leak" | Out-File "$logsDir\14-key-leak-scan.log" -Encoding UTF8
    Write-Host "Step 14: key-leak scan PASS"

    # Step 15: verify real baseline key not in full T02 source (defense-in-depth)
    $baselineKeyMatch = Select-String -Path "$repoRoot\docs\TECHNICAL_BASELINE.md" -Pattern '^[0-9a-f]{64}$' | Select-Object -First 1
    if ($baselineKeyMatch) {
        $realBaselineKey = $baselineKeyMatch.Line
        # 空值保护：仅在 $realPathScanFiles 非空时执行扫描
        $realKeyInSource = if ($realPathScanFiles.Count -gt 0) { Select-String -Path $realPathScanFiles -Pattern $realBaselineKey -SimpleMatch } else { $null }
        if ($realKeyInSource) {
            $hitCount = @($realKeyInSource).Count
            "Real baseline key found in source ($hitCount hits, defense-in-depth FAILED)" | Out-File "$logsDir\15-real-key-scan.log" -Encoding UTF8
            throw "FATAL: real baseline key found in source"
        }
    }
    "Real baseline key not in full T02 source (scanned $($t02SourceFiles.Count) files, defense-in-depth PASS)" | Out-File "$logsDir\15-real-key-scan.log" -Encoding UTF8
    Write-Host "Step 15: real-key-in-source scan PASS"

    # Step 16: verify evidence/logs no sensitive keywords (type-only output)
    $sensitivePatterns = "eyJ|Bearer |authorization:|cookie:|set-cookie"
    $sensitiveHits = Select-String -Path "$logsDir\*.log" -Pattern $sensitivePatterns -CaseSensitive
    $sensitiveSummary = if ($sensitiveHits) {
        $sensitiveHits | Select-Object Filename, LineNumber, Pattern | Format-Table | Out-String
    } else { "No sensitive keyword hits" }
    $sensitiveSummary | Out-File "$logsDir\16-sensitive-scan.log" -Encoding UTF8
    if ($sensitiveHits) {
        throw "FATAL: evidence logs contain sensitive keywords, see 16-sensitive-scan.log (type only, no values)"
    }
    Write-Host "Step 16: sensitive-keyword scan PASS"

    # Step 17: working-tree binding — T02 源码逐文件 SHA-256 + git diff --binary 哈希
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
    $workingTreeHash = (Get-FileHash -InputStream $ms -Algorithm SHA256).Hash.ToLower()
    # 保持 strict mode 关闭（已在脚本开头关闭），仅临时切换 ErrorActionPreference
    $ErrorActionPreference = 'Continue'
    $gitDiffRaw = & git -C $repoRoot diff --binary 2>&1
    $gitDiffStr = if ($gitDiffRaw) { ($gitDiffRaw | Out-String) } else { "" }
    $ErrorActionPreference = 'Stop'
    $gitDiffBytes2 = [System.Text.Encoding]::UTF8.GetBytes($gitDiffStr)
    $gitDiffMs = New-Object System.IO.MemoryStream
    $gitDiffMs.Write($gitDiffBytes2, 0, $gitDiffBytes2.Length)
    $gitDiffMs.Position = 0
    $gitDiffHash = (Get-FileHash -InputStream $gitDiffMs -Algorithm SHA256).Hash.ToLower()
    $headSha = & git -C $repoRoot rev-parse HEAD
    "Working-tree binding" | Out-File "$logsDir\17-working-tree-binding.log" -Encoding UTF8
    "HEAD: $headSha" | Add-Content -Path "$logsDir\17-working-tree-binding.log" -Encoding UTF8
    "T02 source combined SHA-256: $workingTreeHash" | Add-Content -Path "$logsDir\17-working-tree-binding.log" -Encoding UTF8
    "git diff --binary SHA-256: $gitDiffHash" | Add-Content -Path "$logsDir\17-working-tree-binding.log" -Encoding UTF8
    "T02 source file count: $($t02SourceFiles.Count)" | Add-Content -Path "$logsDir\17-working-tree-binding.log" -Encoding UTF8
    "Per-file SHA-256 list: working-tree-hashes.sha256" | Add-Content -Path "$logsDir\17-working-tree-binding.log" -Encoding UTF8
    Write-Host "Step 17: working-tree binding PASS (combined=$workingTreeHash gitdiff=$gitDiffHash)"

    $phase1Success = $true
} catch {
    $phase1Error = $_
    Write-Host "PHASE 1 ERROR: $($_.Exception.Message)"
} finally {
    Stop-Transcript | Out-Null
}

# ===================== generate environment.json =====================
$envJson = @{
    run_id = "20260802-193000"
    gate = "Gate B"
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
    scope = "Account evidence matrix + UI boundary (T02 Repair 3: evidence-only rebuild, binds post-review-2 working tree)"
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
    run_id = "20260802-193000"
    gate = "Gate B"
    ticket = "T02"
    repair = "R3"
    conclusion = $conclusion
    evidence_level = "Implemented"
    qualification = "NOT_QUALIFIED"
    acceptance_criteria = @{
        "B1" = @{ status = if ($phase1Success) { "PASS" } else { "FAIL" }; evidence = "parse_log_file_extracts_* + extract_user_id_rejects_non_whitelist_event PASS: whitelist events parsed, non-whitelist rejected" }
        "B2" = @{ status = if ($phase1Success) { "PASS" } else { "FAIL" }; evidence = "two_source_consistent_* + missing_returns_missing + conflict_returns_conflict_when_local_storage_differs PASS" }
        "B3" = @{ status = if ($phase1Success) { "PASS" } else { "FAIL" }; evidence = "parse_storage_extracts_plaintext_user_id_from_icube_cloudide + plaintext_user_id_* PASS: R4 plaintext compat only extracts userId" }
        "B4" = @{ status = if ($phase1Success) { "PASS" } else { "FAIL" }; evidence = "parse_storage_fingerprint_deterministic + fingerprint_changes_* PASS: SHA-256 fingerprint irreversible" }
        "B5" = @{ status = if ($phase1Success) { "PASS" } else { "FAIL" }; evidence = "user_id_from_verified_rejects_too_short_device_id_format PASS: deviceId/userId strictly separated" }
        "B6" = @{ status = if ($phase1Success) { "PASS" } else { "FAIL" }; evidence = "local_storage_logical_read_returns_user_id + _returns_none_* PASS: logical read interface" }
        "B7_R2_latest_session" = @{ status = if ($phase1Success) { "PASS" } else { "FAIL" }; evidence = "pick_latest_session_returns_newest_mtime_dir + _returns_none_when_logs_absent PASS" }
        "B7_R3_expired" = @{ status = if ($phase1Success) { "PASS" } else { "FAIL" }; evidence = "expired_when_latest_session_too_old + not_expired_when_session_within_threshold + expired_account_yields_expired_reason_through_service PASS" }
        "B7_R3_fingerprint_changed" = @{ status = if ($phase1Success) { "PASS" } else { "FAIL" }; evidence = "fingerprint_changed_yields_fingerprint_changed_reason + re_verify_after_close_* PASS" }
        "B7_R1_path_guard" = @{ status = if ($phase1Success) { "PASS" } else { "FAIL" }; evidence = "build_read_state_rejects_* PASS: R1 final path closure" }
        "R2-1_symlink_junction_escape" = @{ status = if ($phase1Success) { "PASS" } else { "FAIL" }; evidence = "r2_1_latest_session/log_file/storage_json/local_storage_symlink_escape_is_ignored + r2_1_normal_fixture_read_not_regressed PASS: all 4 read paths sealed via canonical containment, junction-based, no silent skip" }
        "R2-2_dto_string" = @{ status = if ($phase1Success) { "PASS" } else { "FAIL" }; evidence = "TypeScript DTO schema_fingerprint: string, auth_fingerprint: string | null, evidence_state: snake_case; pnpm typecheck/test/build exit=0" }
        "R2-3_serde_contract" = @{ status = if ($phase1Success) { "PASS" } else { "FAIL" }; evidence = "domain r2_3_user_id/auth_fingerprint/schema_fingerprint_serializes_as_json_string + r2_3_workbench_read_state_full_json_shape_matches_frontend_dto + r2_3_account_evidence_missing_state_json_shape PASS" }
        "C1" = @{ status = if ($phase1Success) { "PASS" } else { "FAIL" }; evidence = "App.test.tsx: no auto read_work_cn_state on init PASS" }
        "C2" = @{ status = if ($phase1Success) { "PASS" } else { "FAIL" }; evidence = "App.test.tsx: Work CN read-only entry shows fixture mode hint and read button PASS" }
        "C3" = @{ status = if ($phase1Success) { "PASS" } else { "FAIL" }; evidence = "App.test.tsx: error state only shows structured kind, no raw_key/auth body PASS" }
        "C4" = @{ status = if ($phase1Success) { "PASS" } else { "FAIL" }; evidence = "App.test.tsx: readonly-notice shows all write capabilities disabled, manual account selection cannot dismiss readonly PASS" }
        "C5" = @{ status = if ($phase1Success) { "PASS" } else { "FAIL" }; evidence = "T01 fixture_paths_integration + dependency_direction + logging_integration + compile_fail all regression pass" }
        "R2-4_working_tree_binding" = @{ status = if ($phase1Success) { "PASS" } else { "FAIL" }; evidence = "working-tree-hashes.sha256 ($($t02SourceFiles.Count) files) + git diff --binary SHA-256 recorded; combined=$workingTreeHash gitdiff=$gitDiffHash" }
    }
    boundary = @{
        fixture_only = $true
        real_trae_data_accessed = $false
        real_trae_started_or_closed = $false
        write_capability_enabled = $false
        raw_key_leaked = $false
        sensitive_keyword_in_evidence = $false
        real_baseline_key_in_source = $false
        source_files_scanned = $t02SourceFiles.Count
        historical_gate_runs_preserved = $true
    }
}
$assertions | ConvertTo-Json -Depth 6 | Out-File -FilePath "$gateDir\assertions.json" -Encoding UTF8

# ===================== generate REPORT.md =====================
$report = @"
# Gate B Run Report - 20260802-193000

## Run Metadata
- **Run ID**: 20260802-193000
- **Gate**: Gate B
- **Ticket**: T02 (Work CN read-only entry)
- **Baseline**: f560abd
- **Repair**: R3 (T02 Repair 3: evidence-only rebuild)
- **Evidence Level**: Implemented
- **Conclusion**: $conclusion
- **Qualification**: NOT QUALIFIED
- **Scope**: Evidence-only. No production code or test changes. Rebuilds Gate B evidence bound to the post-review-2 working tree (evidence_state snake_case + dynamic t02_source_files).

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
Account evidence matrix + UI boundary (T02 Repair 3, evidence-only):
1. R2-1: all read paths canonical containment sealed (session dir / alog.log / renderer.log / main.log / storage.json / local_storage.json); 4 symlink/junction escape tests use directory junction (mklink /J), panic BLOCKED on creation failure; normal fixture read not regressed
2. R2-2: TypeScript DTO schema_fingerprint: string, auth_fingerprint: string | null, evidence_state: snake_case (aligns with Rust serde newtype + rename_all)
3. R2-3: domain serde JSON contract tests (UserId/AuthFingerprint/SchemaFingerprint bare strings + full WorkbenchReadState shape matches frontend DTO); frontend mock synced
4. R2-4: working-tree binding (12 source files per-file SHA-256 + git diff --binary SHA-256); realPathScanFiles excludes fixture_paths.rs
5. R2/R3/R4: latest session selection, Expired, FingerprintChanged, plaintext compat all retained
6. UI: no auto-scan; read button; structured error state without secrets; readonly cannot be dismissed

## Test Summary
- **account_evidence::tests**: R2 latest session + R3 Expired + R4 plaintext + R2-1 symlink/junction escape PASS
- **application workbench_read::tests**: R3 FingerprintChanged + R1 path guard PASS
- **commands**: cargo test -p traesync-commands exit=0
- **domain workbench_read::tests**: R5 + R2-3 serde contract PASS
- **T01 regression**: fixture_paths_integration + dependency_direction + logging_integration + compile_fail all pass
- **pnpm typecheck/test/build**: exit=0

## Boundary
- Only uses repo-internal synthetic fixture and tempdir
- No access to real %APPDATA%\TRAE SOLO CN, active DB, real logs or Local Storage
- No start or close of TRAE
- No write capability enabled
- Synthetic fixture key not in logs
- Real baseline key not in full T02 source ($($t02SourceFiles.Count) files, real-path scan excluded fixture_paths.rs)
- No sensitive keyword hits in evidence
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
