# Gate 0 Run Report - 20260731-233000

## Run Metadata
- **Run ID**: 20260731-233000
- **Gate**: Gate 0
- **Ticket**: T01 (protected desktop app skeleton)
- **Baseline**: f9eda71
- **Repair Round**: 7
- **Evidence Level**: Implemented
- **Conclusion**: PASS
- **Script Started**: see logs/00-transcript.log
- **Script Ended**: see logs/00-transcript.log
- **Two-Phase Closure**: Phase 1 (transcript on) runs all commands + generates reports + phase1 hash; Phase 2 (transcript off) appends transcript hash + generates complete hashes.sha256 + verifies all.

## Environment
- **OS**: Microsoft Windows NT 10.0.26200.0
- **PowerShell**: 5.1.26100.8875 (Desktop)
- **Node**: v22.23.1
- **pnpm**: 9.15.9
- **cargo**: cargo 1.97.1 (c980f4866 2026-06-30)
- **rustc**: rustc 1.97.1 (8bab26f4f 2026-07-14)

## Repair Summary

### R1: OperationId strict generation/parsing boundary + real compile-fail tests
- Removed public from_validated() - only new() is public, no string injection entry
- Removed OperationIdError enum (only served from_validated)
- 4 trybuild compile-fail tests in tests/ui/ verify: from_validated absent, OperationId field private, LogEvent no Deserialize, LogEvent fields private
- 8 external counterexample tests in logging_integration.rs verify from external perspective

### R2: Script-generated reports + complete transcript + two-phase evidence closure
- Script generates REPORT.md, assertions.json, environment.json (no agent post-write)
- Two-phase closure: Phase 1 hashes all files except transcript; Phase 2 stops transcript, appends its hash, generates complete hashes.sha256, verifies all
- Transcript contains script start, all commands, report generation, phase1 hash verification, PHASE1_COMPLETE marker

### R3: Client-area screenshot via GetClientRect + GetDC(hWnd) + BitBlt from (0,0)
- Originally fixed PrintWindow blank client area for WebView2 (separate composition layer)
- Current method: GetClientRect for client dimensions, GetDC(hWnd) for client-area DC, BitBlt from (0,0)
- Client area 1280x800 matches tauri.conf.json; no DWM shadow/border/transparency captured
- All Win32 return values checked (FindWindow, SetForegroundWindow, GetClientRect, GetDC, BitBlt, ReleaseDC)
- Pixel assertions: colorCount > 10 and nonWhitePct > 5% (prevents white screen)
- Screenshot shows actual workbench content (history library skeleton, disabled capabilities, honest status)

### R4: Name-level /XD exclusion + pre-install recursive scan (clean copy boundary)
- robocopy /XD accepts directory names, excludes same-named dirs at any depth (node_modules, dist, target, .git, gen)
- Covers both d:\work\Trae-sync\target and d:\work\Trae-sync\src-tauri\target (previous round missed the latter)
- Pre-install recursive scan asserts: C:\Users\12732\AppData\Local\Temp\trae-sync-clean-copy-20260731-233000\src-tauri\target absent, C:\Users\12732\AppData\Local\Temp\trae-sync-clean-copy-20260731-233000\src-tauri\gen absent, C:\Users\12732\AppData\Local\Temp\trae-sync-clean-copy-20260731-233000\node_modules absent, C:\Users\12732\AppData\Local\Temp\trae-sync-clean-copy-20260731-233000\dist absent
- Recursive scan by directory name at any depth; *.tsbuildinfo scan
- FORBIDDEN ARTIFACT COUNT = 0
- 6 commands pass in clean copy (cargo test from scratch: 653s vs main repo 23s, 28x proves no target cache)
- README unchanged; all history dirs untouched

## Test Summary

### Frontend (vitest)
- 8 tests pass (see logs/03-pnpm-test.log)

### Rust (cargo test --workspace)
- compile_fail: 1 (4 ui tests)
- dependency_direction: 6
- fixture_paths_integration: 13
- logging_integration: 8
- traesync_commands: 1
- traesync_domain: 5
- traesync_infrastructure: 22
- Total: 56 tests pass (see logs/05-cargo-test-workspace.log)

### Parallel regression
- 20 rounds fixture_paths_integration all pass (see logs/07-fixture-paths-round-1-20.log)

## Binary Hashes
    A87C5546C72F2773AC1340864E9FA4D4E137A7B3A00AEA9C3812842FC4ADA34E  src-tauri\target\release\trae-sync.exe     803FFE9A91547A4796A6CD719CAD2D4708FEF9FB0217464D10226B8B5E5E8ED6  src-tauri\target\release\bundle\msi\Trae Sync_0.1.0_x64_zh-CN.msi     5CD3F015D902B913E413B49FC3D27D73AD686F8764EFE45192DA02D3587CAA7B  src-tauri\target\release\bundle\nsis\Trae Sync_0.1.0_x64-setup.exe
(see logs/binary-hashes.txt)

## Window Screenshot
- File: tauri-empty-workbench.png
- Dimensions: 1280x800
- Size: 39224 bytes
- SHA256: BF57EE5ECEAC05DD23A8FE845607A5598991E345B4BFD9C015F2EEC35D63F66D
- Capture method: GetClientRect + GetDC(hWnd) + BitBlt from (0,0) - client area only, no DWM shadow/border

## Clean Copy Verification
- Excludes: node_modules, dist, target, .git, gen, *.tsbuildinfo
- 6 commands pass (see logs/11-16)
- Verification log: logs/10-clean-verify-no-artifacts.txt

## Evidence File List
- commands.ps1 - reproducible script
- logs/00-transcript.log - top-level transcript (Phase 1)
- logs/01-09 - main repo command logs
- logs/07-fixture-paths-round-1-20.log - 20 rounds regression
- logs/10-clean-exclude-list.txt + logs/10-clean-verify-no-artifacts.txt
- logs/11-16 - clean copy command logs
- logs/binary-hashes.txt - 3 binary SHA256
- logs/sample-redacted-log.json - R1 redacted log sample
- tauri-empty-workbench.png - client-area screenshot (GetClientRect + GetDC + BitBlt)
- hashes.sha256 - complete evidence hash (Phase 2)
- assertions.json - acceptance assertions
- environment.json - environment info
- REPORT.md - this report

## Two-Phase Closure Description
Phase 1 (transcript running): All commands execute, reports generated, phase1 hash computes and verifies all files except 00-transcript.log and hashes.sha256. PHASE1_COMPLETE marker written.
Phase 2 (transcript stopped): 00-transcript.log hash computed, complete hashes.sha256 generated covering all files except itself, line count verified (must equal total files - 1), each hash independently recomputed. Script exit code reflects closure failure.
