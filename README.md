# Trae Sync

[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)

个人使用的 TRAE Work CN 多账号登录管理工具（非官方产品，仅供学习与自用）。

基于 Tauri 2 的 Windows 桌面应用，围绕四个核心需求构建：**账号保存**（OAuth 登录、DPAPI 加密凭据）、**对话主库**（所有账号共用一套历史记录，切号不丢）、**直连签到**（HTTP 直连完成每日签到）、**账号切换**（一键切换、环境管理）。

## 核心功能

- **账号保存**：App 内 OAuth 登录，凭据经 Windows DPAPI 加密落盘；提供凭据健康度与同设备换发。
- **对话主库**：所有账号共用一套历史记录，切号时记录归属随账号交接，不复制、不丢失。
- **直连签到**：HTTP 直连完成每日签到（不启动 TRAE），支持打开时自动签到调度与积分展示。
- **账号切换**：一键切换主库当前登录账号，自动完成备份、记录交接、插件同步与实例重启。
- **会话归档**：把会话或整个项目移出常规列表但不删除（口语：出库/入库），支持批量多选与项目合并。
- **插件管理**：每个环境持有插件清单，安装/卸载即时同步账号云端，切号时自动对账。
- **主库体检**：账号分布、未注册账号标注、空项目诊断与清理，一键把遗留记录归入当前账号。
- **主库数据备份**：手动一键备份 + 切号/批量删除前自动备份，保留策略可调（默认最近 5 份）。

## 主库是什么？

TRAE 官方客户端把你的全部对话记录保存在一个本地数据目录里。这套记录的集合，本项目称为「主库」。

主库不属于任何一个账号。账号只是「当前登录者」：工具把每条记录的归属登记在某个账号名下，切换账号时把记录归属整体交接给新的登录账号。因此所有账号共用同一套历史，切号不丢记录；已归档的会话则作为主库冻结内容留在原处，不参与交接。详见 [docs/GUIDE.md](docs/GUIDE.md) 的「主库概念」章节。

## 当前状态

- 平台：Windows x64 + TRAE Work CN（当前版本仅支持此组合）
- 已发布 0.2.1 安装包；当前源码版本 0.2.2，release 实机验收待完成
- 与第三方 Manager 类工具仅行为参考关系，无代码或数据依赖；账号一律通过 App 内 OAuth 登录

## 快速开始

### 环境要求

- Windows 10/11 x64
- Node.js ≥ 18（建议 20 LTS）与 pnpm ≥ 8
- Rust stable 工具链（含 `cargo`）
- MSVC Build Tools（链接 Tauri 依赖需要）
- Tauri 2 系统依赖：参见 https://tauri.app/v2/guides/prerequisites/

### 安装与开发运行

```powershell
pnpm install --frozen-lockfile
pnpm dev:tauri
```

`pnpm dev:tauri` 同时启动 Vite 与 Rust/Tauri：前端支持热更新，Rust 文件变更自动重新编译。它会把应用数据目录固定到 `.scratch/dev-tauri/`，因此是隔离的空开发预览，不读取真实 TRAE 数据。

仅做前端静态开发可用 `pnpm dev`（不提供完整 Tauri IPC）。

### 构建

```powershell
pnpm build                              # 前端产物（tsc -b && vite build）
pnpm tauri build -- --bundles nsis      # Windows 安装包
```

### 测试

```powershell
pnpm typecheck    # TypeScript 类型检查（仓库根目录执行）
pnpm test         # Vitest 前端单元测试（仓库根目录执行）
pnpm test:e2e     # Playwright 桌面 UI 验收（仓库根目录执行，合成 mock bridge，不启动 TRAE）

cd src-tauri
cargo test --workspace --all-targets   # Rust 工作区测试（在 src-tauri 目录执行）
cargo fmt --all -- --check             # Rust 格式检查
```

## 免责声明

- 本项目与 TRAE 官方无任何关联，亦非官方产品。
- 工具会按你的指令读写 TRAE 本地数据目录与数据库。直接操作本地数据库存在风险：请在执行任何批量操作前，先使用应用内的「主库数据备份」功能；工具自身也会在切号与批量删除前自动创建备份。
- 本项目仅供个人学习与研究使用，请自行承担使用风险。

## 文档导航

| 文档 | 内容 |
|------|------|
| [docs/GUIDE.md](docs/GUIDE.md) | 使用手册：各页面操作、主库概念、数据转移实现、常见问题 |
| [docs/TECHNICAL_BASELINE.md](docs/TECHNICAL_BASELINE.md) | 协议与技术事实基线（解密、数据库结构、迁移契约） |
| [docs/adr/](docs/adr/README.md) | 架构决策记录（ADR 索引） |
| [CONTRIBUTING.md](CONTRIBUTING.md) | 参与贡献指南 |

## License

[MIT](LICENSE) © 2026 YumeMeng
