# Trae Sync

独立的 TRAE 系列本地对话历史管理与同步桌面工具。

## 当前状态

- 阶段：V1 实现中，T01 受保护的桌面应用骨架已实现，Gate 0 待 Codex 验收
- 状态：D-001 至 D-045 已确认；T01 骨架代码（Tauri 2 + Rust 分层 workspace + React UI）已生成
- 当前工作目录：`D:\work\Trae-sync`
- 与 `Trae-Account-Manager-main` 的关系：仅参考，不建立代码依赖

设计已经收敛。T01 骨架已落地，Gate 0 待 Codex 验收；后续 ticket 按 `tickets.md` 依赖前沿推进，不直接写真实同步功能。

## 文档

- `docs/DESIGN_DISCUSSION.md`：逐项设计决策及收敛状态
- `docs/TECHNICAL_BASELINE.md`：已验证的解密、数据库结构和迁移事实
- `docs/DESIGN_AUDIT.md`：V1 设计审计、问题结论和对应技术 Gate
- `docs/SESSION_MERGE_FEASIBILITY.md`：单条对话重挂与同项目历史合并的真实副本验证
- `docs/ACCOUNT_DETECTION_FEASIBILITY.md`：当前账号证据、真实切换验证和 Gate B 结论
- `docs/IMPLEMENTATION_SPEC.md`：V1 功能、模块、数据、执行和恢复规格
- `docs/GATE_PLAN.md`：Gate 依赖、fixture、通过条件和证据要求
- `docs/UI_PROTOTYPE_REVIEW.md`：融合 UI 的体验结论、信息取舍和未来功能扩展边界

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

### T01 真正要求的质量命令

`package.json` 没有声明 ESLint；T01 阶段类型与格式约束由下列命令覆盖：

```powershell
pnpm typecheck      # TypeScript 项目引用类型检查
pnpm test           # Vitest 前端单元测试
pnpm build          # tsc -b && vite build（产物用于 Tauri 打包）

$env:Path = "$env:USERPROFILE\.cargo\bin;$env:Path"
Push-Location src-tauri
cargo test --workspace      # Rust 工作区测试
cargo fmt --all -- --check  # Rust 格式检查
Pop-Location

pnpm tauri build             # 仅在需要本地安装包时执行
```

Rust 格式约定：`cargo fmt --all` 已应用；任何提交前需通过 `--check`。


完整逆向记录另存于：

```text
E:\系统存储\桌面\TraeWork对话同步
```
