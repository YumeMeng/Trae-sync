$ErrorActionPreference = "Stop"
$repo = Resolve-Path (Join-Path $PSScriptRoot "..\..\..\..")
$logs = Join-Path $PSScriptRoot "logs"
New-Item -ItemType Directory -Force -Path $logs | Out-Null
Set-Location $repo

function Run-Step([string]$Name, [scriptblock]$Command) {
    $previousPreference = $ErrorActionPreference
    $ErrorActionPreference = "SilentlyContinue"
    & $Command 2>&1 | Tee-Object -FilePath (Join-Path $logs "$Name.log")
    $exitCode = $LASTEXITCODE
    $ErrorActionPreference = $previousPreference
    if ($exitCode -ne 0) { throw "$Name failed with exit code $exitCode" }
}

# Gate F 仅运行 fixture/mock 测试，不访问真实 TRAE 数据。
Run-Step "01-project-identity" { cargo test --manifest-path src-tauri/Cargo.toml -p traesync-domain project_identity }
Run-Step "02-sync-plan" { cargo test --manifest-path src-tauri/Cargo.toml -p traesync-domain sync_plan }
Run-Step "03-application-plan" { cargo test --manifest-path src-tauri/Cargo.toml -p traesync-application build_sync_plan }
Run-Step "04-frontend-plan" { pnpm test -- -t "T05" }
Run-Step "05-playwright-plan" { pnpm exec playwright test e2e/history-workbench.spec.ts --grep T05 }
Run-Step "06-typecheck" { pnpm typecheck }
Run-Step "07-format" { cargo fmt --all --manifest-path src-tauri/Cargo.toml --check }
Run-Step "08-diff-check" { git diff --check }

"=== ALL_STEPS_PASSED ===" | Set-Content -Encoding utf8 (Join-Path $logs "closure.log")
