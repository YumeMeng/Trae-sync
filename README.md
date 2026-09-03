# Trae Sync

个人使用的 TRAE Work CN 多账号日常管理工作台。四个核心需求：账号保存（凭据持久化）、对话主库（所有账号共用一套历史）、直连签到（HTTP 完成每日签到）、账号切换（集中管理、快速切换、信息与积分显示）。

## 当前状态

- 版本：`0.2.1`（已发布安装包见下；当前进度以 `tickets.new.md` 头部状态为准：四大核心需求已全部落地，Phase 0-7 计划开发项已全部完成，仅余 Phase 5 日常观察等长期项）
- 平台：Windows x64、TRAE Work CN
- 与 `xhrxgr/Trae-Work-CN-Account-Manager` 的关系：仅行为参考，不建立代码或数据依赖；账号一律通过 App 内 OAuth 登录
- 项目方向：2026-08-22 重建，数据红线已解除（ADR-0018），按 `tickets.new.md` Phase 0-7 推进（旧版 `tickets.md` 已于 2026-09-02 删除，git 历史可查）

已发布 0.2.1 提供非敏感账号档案、本地历史库、按账号/项目/会话的浏览搜索。签到链路（OAuth 登录、DPAPI 凭据包、HTTP 直连）此后已完成真实端到端验证（Phase 0 闭环，证据见 `.scratch/checkin-http/`）。安装包位于 `artifacts/Trae Sync_0.2.1_x64-setup-20260819-current.exe`，SHA-256 为 `6dae3dd30e9377874c004eec3f65f104f86f65c2d3f5bfd3c4d6aee3c9cebdd4`。

## 文档

- `docs/DECISIONS-20260822-GRILL.md`：项目方向历史锚点（九问九答全量决策；部分主库/实例决策已被 ADR-0021/0024 取代，见其头部标注）
- `CONTEXT.md`：领域语言、协议事实、架构资产现状
- `tickets.new.md`：Phase 0-7 实施计划（现行唯一计划文件）
- `docs/adr/`：架构决策记录（现行 7 份：0018 红线解除 / 0019 签到虚拟设备 / 0020 主库与实例模型（部分被取代）/ 0021 主库单一归属 / 0022 会话归档 / 0023 环境插件清单 / 0024 账号-环境解耦；状态行为决策权威，见 adr/README.md）
- `docs/TECHNICAL_BASELINE.md`：已验证的解密、数据库结构、迁移事实与 SQLCipher 导出契约
- `.scratch/checkin-http/`：签到 HTTP 直连实验证据与复现手册

## 构建与验证前提

### 工具链

- Node.js ≥ 18（建议 20 LTS）与 pnpm ≥ 8
- Rust stable 工具链（含 `cargo`）
- Windows：MSVC Build Tools（链接 `trae_sync_lib.dll` 需要）
- Tauri 2 系统依赖：参见 https://tauri.app/v2/guides/prerequisites/

### 从锁文件安装

```powershell
pnpm install --frozen-lockfile
```

### 质量命令

`package.json` 没有声明 ESLint；类型、行为、桌面布局与 Rust 格式由下列命令覆盖：

```powershell
pnpm typecheck      # TypeScript 项目引用类型检查
pnpm test           # Vitest 前端单元测试
pnpm build          # tsc -b && vite build（产物用于 Tauri 打包）

$env:Path = "$env:USERPROFILE\.cargo\bin;$env:Path"
Push-Location src-tauri
cargo test --workspace --all-targets # Rust 工作区测试
cargo fmt --all -- --check  # Rust 格式检查
Pop-Location

pnpm test:e2e                 # Playwright 桌面视口和键盘验收
pnpm tauri build -- --bundles nsis
```

### 日常开发桌面

日常改动不需要重新安装 NSIS 包。使用下面命令启动完整 Tauri 桌面开发环境：

```powershell
pnpm dev:tauri
```

该命令同时启动 Vite 和 Rust/Tauri，前端支持热更新，Rust 文件变更会自动重新编译。它会把 `APPDATA`、`LOCALAPPDATA`、`TEMP` 和 `TMP` 固定到 `.scratch/dev-tauri/`，因此默认是隔离的空开发预览，不读取真实 TRAE 数据，也不复用正式安装包的目录库。重复启动会保留这份开发状态，便于连续调试。

其他用途：

- `pnpm dev`：仅启动 Vite；适合前端静态开发，不提供完整 Tauri IPC。
- `pnpm test:e2e`：使用合成 mock bridge 验收桌面 UI，不启动 TRAE。
- `pnpm tauri build`：只在发布候选或安装包验收时使用，无需每次改动都执行。

需要验证当前机器真实 TRAE 数据时，使用已安装候选包（数据红线已按 ADR-0018 解除，破坏性批量操作前单次确认）；`pnpm dev:tauri` 环境变量指向 `.scratch/dev-tauri/`，是隔离的空开发预览，不读取真实 TRAE 数据。

Rust 格式约定：`cargo fmt --all` 已应用；任何提交前需通过 `--check`。
