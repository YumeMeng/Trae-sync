# 贡献指南

Trae Sync 是个人使用的自用工具，欢迎通过 issue 反馈问题与功能建议；代码贡献也欢迎，但请保持精准修改，不做顺手重构。

## 反馈问题

提交 issue 时请尽量包含：

- 使用的版本（源码构建请附提交号）与 Windows 版本；
- 复现步骤与预期/实际行为；
- 界面报错文案。不要粘贴凭据、token 或完整手机号等敏感信息。

## 开发环境

Windows x64；Node.js ≥ 18 与 pnpm ≥ 8；Rust stable 与 MSVC Build Tools；Tauri 2 系统依赖。完整步骤见 [README.md](README.md) 的「快速开始」。

## 分支与提交

- fork 仓库后从 `main` 拉出功能分支（如 `fix/xxx`、`feat/xxx`）。
- 一个 PR 聚焦一件事；提交信息用一句话说清动机。
- 不回退工作区内与本次改动无关的内容。

## 提交前自检

以下命令必须全部通过（Rust 命令在 `src-tauri` 目录执行）：

```powershell
pnpm typecheck
pnpm test
cargo test --workspace --all-targets
```

涉及 Rust 代码时另跑 `cargo fmt --all -- --check`；涉及桌面 UI 时建议加跑 `pnpm test:e2e`。

## PR 描述

请说明：改了什么、为什么改、如何验证（跑过哪些测试或手工步骤）；涉及界面文案时附改动前后对照。

## 代码风格

- 注释与文档使用中文；只在代码不自明处写注释。
- 遵循现有架构分层：Rust 侧 domain / application / infrastructure / ports + 组合根；前端按页面组件组织。
- 界面文案用用户能直接理解的说法；底层编号、内部标识与错误码不进主视野。
- 服务端交互（签到 / OAuth / 凭据换发）遵循 `docs/adr/` 中现行 ADR 与 `docs/TECHNICAL_BASELINE.md` 的事实，不凭猜测修改协议行为。

## License

提交贡献即表示同意以 [MIT](LICENSE) 许可证发布。
