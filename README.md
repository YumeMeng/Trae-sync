# Trae Sync

[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)

> **纯 AI Vibe Coding**：本项目全部内容（含 README 与代码）均由 AI 生成。作者仅对基础功能做过有限测试，主要确认了**签到**、**账号切换**与**登录设备管理**三条核心链路的可用性；其余功能细节尚未充分验证，使用前请自行评估。

个人使用的 TRAE 渐进适配工作台（非官方产品，仅供学习与自用），当前唯一产品适配目标是融合版 TRAE（Work CN 升级版，产品 ID 沿用 `trae_cn`），16 个 Work 账号已全部迁移至融合版登录凭据。

基于 Tauri 2 的 Windows 桌面应用，围绕四个核心需求构建：**账号保存**（OAuth 登录、DPAPI 加密凭据）、**对话主库**（所有账号共用一套历史记录，切号不丢）、**直连签到**（HTTP 直连完成每日签到）、**账号切换**（一键切换、环境管理）。

## 核心功能

- **账号保存**：App 内 OAuth 登录，凭据经 Windows DPAPI 加密落盘；提供凭据健康度与同设备换发。
- **对话主库**：所有账号共用一套历史记录，切号时记录归属随账号交接，不复制、不丢失。
- **直连签到**：HTTP 直连完成每日签到（不启动 Trae Work CN），支持打开时自动签到调度与积分展示。
- **账号切换**：一键切换主库当前登录账号，自动完成备份、记录交接、插件同步与实例重启。
- **会话归档**：把会话或整个项目移出常规列表但不删除（口语：出库/入库），支持批量多选与项目合并。
- **插件管理**：每个环境持有插件清单，安装/卸载即时同步账号云端，切号时自动对账。
- **主库体检**：账号分布、未注册账号标注、空项目诊断与清理，一键把遗留记录归入当前账号。
- **主库数据备份**：手动一键备份 + 切号/批量删除前自动备份，保留策略可调（默认最近 5 份）。
- **单产品工作台**：融合版 TRAE 唯一产品目标，双通道凭据模型（登录代次 = TRAE 通道融合版容器；签到代次 = SOLO 通道签到凭据包），产品切换器随单产品隐藏；App 内 OAuth 登录与健康检测接入。
- **登录设备管理**：在账号详情页查看该账号的服务端登录设备（占用/上限、绑定产品、本机标记），并可逐台退出不用的设备释放额度；本机设备禁止远程退出（界面隐藏 + 后端双重防线），接近上限时高亮提醒。

## 主库是什么？

融合版 TRAE 官方客户端把你的全部对话记录保存在一个本地数据目录里（`%APPDATA%\Trae CN`）。这套记录的集合，本项目称为「主库」。

主库不属于任何一个账号。账号只是「当前登录者」：工具把每条记录的归属登记在某个账号名下，切换账号时把记录归属整体交接给新的登录账号。因此所有账号共用同一套历史，切号不丢记录；已归档的会话则作为主库冻结内容留在原处，不参与交接。详见 [docs/GUIDE.md](docs/GUIDE.md) 的「主库概念」章节。

## 当前状态

- 平台：Windows x64；2026-10-13 起融合版 TRAE（Work CN 升级版）成为唯一产品适配目标（[ADR-0035](docs/adr/0035-unified-trae-product-and-credential-migration.md)），Work 客户端 2026-10-23 官方停服前完成 16 账号迁移；当前实施入口 Phase 15（[tickets.new.md](tickets.new.md)）
- Phase 15 进展：阶段一 16/16 账号融合版登录凭据铸造完成（DPAPI 容器，不写客户端目录）；阶段二 P15-7 旧 trae_cn 模块移除、切号编排/主库目标切换/双通道凭据接线完成，mock 侧四套件全绿（cargo 串行分段 / typecheck / Vitest 402 / E2E 43）；剩余真实操作验收（首次真实切号、真实启停）待用户在场单次确认执行
- 登录设备管理已通过真机验收（远程退出、本机识别均经生产环境验证）；架构决策见 [docs/adr/](docs/adr/)，实施计划见 [tickets.new.md](tickets.new.md)
- 与第三方 Manager 类工具仅行为参考关系，无代码或数据依赖；账号一律通过 App 内 OAuth 登录

## 快速开始

### 环境要求

- Windows 10/11 x64
- Node.js ≥ 18（建议 20 LTS）与 pnpm ≥ 8
- Rust stable 工具链（含 `cargo`）
- MSVC Build Tools（链接 Tauri 依赖需要）
- Tauri 2 系统依赖：参见 <https://tauri.app/v2/guides/prerequisites/>

### 安装与开发运行

```powershell
pnpm install --frozen-lockfile
pnpm dev:tauri
```

`pnpm dev:tauri` 同时启动 Vite 与 Rust/Tauri：前端支持热更新，Rust 文件变更自动重新编译。它会把应用数据目录固定到 `.scratch/dev-tauri/`，因此是隔离的空开发预览，不读取真实 Trae Work CN 数据。

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
pnpm test:e2e     # Playwright 桌面 UI 验收（仓库根目录执行，合成 mock bridge，不启动 Trae Work CN）

cd src-tauri
cargo test --workspace --all-targets   # Rust 工作区测试（在 src-tauri 目录执行）
cargo fmt --all -- --check             # Rust 格式检查
```

## 免责声明

- 本工具仅用于个人学习与技术研究目的，不得用于任何商业用途或非法用途。
- **签到功能专项提示**：签到功能直接与 Trae Work CN 服务端交互，频控、资格判定与账号风控均由服务端决定；频繁或异常使用可能导致签到失败或账号受限，由此产生的风险由使用者自行承担。
- **数据同步与账号切换专项提示**：工具会按你的指令读写 Trae Work CN 本地数据库并改写记录归属，操作本地数据存在风险。请在使用相关功能前，先使用应用内的「主库数据备份」功能进行备份；工具自身也会在切号与批量删除前自动创建备份。使用过程中可能产生的数据风险由使用者自行承担。
- 本工具与 Trae Work CN 官方无任何关联，非官方产品。
- 使用者需自行承担使用过程中可能产生的一切风险与后果。

## 文档导航

| 文档                                                        | 内容                          |
| --------------------------------------------------------- | --------------------------- |
| [docs/GUIDE.md](docs/GUIDE.md)                            | 使用手册：各页面操作、主库概念、数据转移实现、常见问题 |
| [docs/TECHNICAL\_BASELINE.md](docs/TECHNICAL_BASELINE.md) | 协议与技术事实基线（解密、数据库结构、迁移契约）    |
| [docs/adr/](docs/adr/README.md)                           | 架构决策记录（ADR 索引）              |
| [CONTRIBUTING.md](CONTRIBUTING.md)                        | 参与贡献指南                      |

## License

[MIT](LICENSE) © 2026 YumeMeng
