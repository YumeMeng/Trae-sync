# Gate E Run Report - 20260803-034500

## Run Metadata
- **Run ID**: 20260803-034500
- **Gate**: Gate E
- **Ticket**: T04 (历史语义稳定性)
- **Baseline**: 74c87f3
- **Evidence Level**: Implemented
- **Conclusion**: PASS
- **Qualification**: NOT_QUALIFIED
- **Scope**: 来源账号与往返观察 — fixture-only 证据，不访问真实 TRAE 数据

## Environment
- **OS**: Microsoft Windows NT 10.0.26200.0
- **PowerShell**: 5.1.26100.8875 (Desktop)
- **Node**: v24.14.1
- **pnpm**: 9.15.9
- **cargo**: cargo 1.97.1 (c980f4866 2026-06-30)
- **rustc**: rustc 1.97.1 (8bab26f4f 2026-07-14)

## Working-Tree Binding
- **HEAD**: 74c87f329a9059e8e11c175f07968bb7ca54c9cd
- **T03/T04 source combined SHA-256**: 91885eea52b46ba87bc879a399dad1641a81e5aea770b07e9b3d2faf5cef97e1
- **git diff --binary SHA-256 (pure, via --output tempfile)**: 09099206a6b196fc82febb4c0ac908571695c547552b92c7c3e409c14a5da9ea
- **T03/T04 source files**: 17
- **Per-file list**: working-tree-hashes.sha256

## Test Summary
- ``ownership-roundtrip``
- ``owner-observation-appends``
- ``assign-source-isolation``
- ``same-title-distinct-sessions``
- ``repeated-scan-no-duplicates``

## Boundary
- Only uses repo-internal synthetic fixture and tempdir
- No access to real %APPDATA%\TRAE SOLO CN, active DB, real logs or Local Storage
- No start or close of TRAE
- No write capability enabled
- Synthetic fixture key not in logs
- Real baseline key not in T03/T04 source (17 files)
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
