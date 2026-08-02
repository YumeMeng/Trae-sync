# Gate A Run Report - 20260802-220000

## Run Metadata
- **Run ID**: 20260802-220000
- **Gate**: Gate A
- **Ticket**: T02 (Work CN read-only entry)
- **Baseline**: f560abd
- **Repair**: R4 (T02 Repair 4: evidence closure)
- **Evidence Level**: Implemented
- **Conclusion**: PASS
- **Qualification**: NOT QUALIFIED
- **Scope**: Evidence closure. No production code or test changes. Fixes pure git diff hash (via --output tempfile) and adds closure marker.

## Environment
- **OS**: Microsoft Windows NT 10.0.26200.0
- **PowerShell**: 5.1.26100.8875 (Desktop)
- **Node**: v24.14.1
- **pnpm**: 9.15.9
- **cargo**: cargo 1.97.1 (c980f4866 2026-06-30)
- **rustc**: rustc 1.97.1 (8bab26f4f 2026-07-14)

## Working-Tree Binding (R2-4 / R4)
- **HEAD**: f560abdf851d5af78041922c889a0719f8a75221
- **T02 source combined SHA-256**: 5980bd5fbb38229000cebd28b92e879e468d1379f6c42455566653d93c4ed999
- **git diff --binary SHA-256 (pure, via --output tempfile)**: 2ce682fde5f6de7334b6435275896695777c06f57fd85fe5eb32ecf3201b3d8e
- **Hash method**: git diff --binary --output=<tempfile>; Get-FileHash on tempfile (no stderr contamination)
- **T02 source files**: 12
- **Per-file list**: working-tree-hashes.sha256

## Scope
Embedded SQLCipher read-only probe (T02 Repair 4, evidence closure):
1. Fixed Rust SQLCipher dependency (bundled-sqlcipher-vendored-openssl)
2. R1 read-only open zero-write evidence (correct/wrong key)
3. R5 cipher_version compatibility check (4.5.x)
4. R5 schema constraint (table/column/index/unique-constraint)
5. R2-1 fixture_paths symlink/junction escape (directory junction, no silent skip)
6. R2-3 domain serde JSON contract (UserId/AuthFingerprint/SchemaFingerprint bare strings)
7. R2-4 working-tree binding (per-file SHA-256 + pure git diff --binary SHA-256 via --output tempfile)
8. R4 pure git diff hash (no stderr ErrorRecord contamination)
9. R4 closure marker (PHASE1_COMPLETE + logs/closure.log with ALL_STEPS_PASSED)
10. Two-layer integrity check; random key catalog create/reopen

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
- Real baseline key not in full T02 source (12 files)
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
