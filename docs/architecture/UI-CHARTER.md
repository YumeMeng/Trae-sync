# 前端重构宪章（v1，2026-10-22）

> 本文件是这次重构的**唯一架构契约**。任何与本文件冲突的实现，以本文件为准；
> 需要破例时走 §8 的登记流程。

## 0. 一句话目标

把 `trae_cn` 与 `workbuddy` 两套并行界面，收敛为 **一个壳 + 一份页面 + 能力裁剪**；
同时把复杂度从"巨型文件"转移到**显式层次边界**，并让"不守规矩"在 CI 上直接失败。

## 1. 五条设计原则（冲突时按序裁决）

1. **一份实现**：同一个交互只能有一处代码。产品差异只能是"有没有这个能力"，不能是"两套页面"。
2. **边界优先**：跨层、跨 feature 只能通过显式导出（`public.ts`）。
3. **可删性**：任一模块应能被单独删除而不引起连锁改动；删不掉说明耦合错位。
4. **预算制**：文件、函数、目录、依赖都有硬上限（§3）。
5. **可验证**：每条规则要么有测试，要么有脚本，不靠自觉。

## 2. 目标结构

```
src/
  app/        壳层：路由、布局、全局 Provider、错误边界      （≤ 8 文件）
  ui/         设计系统原子：无业务、无 invoke、无产品概念
  features/   accounts | checkin | environment | library | settings
  platform/   IPC 适配、capabilities、产品注册表、事件桥
  contracts/  DTO 与命令类型（与 Rust 侧一一对应）
  styles/     tokens.css / base.css / components.css / pages.css
```

**依赖方向（只允许单向）**：

```
app  →  features  →  ui
 ↓        ↓         ↑
platform ────────────┘
contracts ← 所有层可用，但 contracts 不得依赖任何层
```

- `ui/` 不得 import `features/`、`platform/`、`contracts/` 的业务类型
- `platform/` 不得 import `ui/`、`features/`
- feature 之间**禁止**深路径 import，只能 `features/x/public.ts`

## 3. 硬预算（由 `scripts/ui-guard.mjs` 强制）

| 对象 | 上限 | 超出处理 |
|---|---|---|
| 组件文件 | 300 行（软 400） | 必须拆分，或在 exceptions 登记 |
| 自定义 hook | 150 行 | 拆分为组合 hook |
| 单函数 | 60 行 | 允许渲染函数例外，但需注释理由 |
| 单个 feature 目录 | 25 文件 | 拆分 feature |
| `ui/` 组件 | 80 行 | 超过说明它承载了业务 |

## 4. 禁止模式（CI 直接失败）

1. `app/`、`features/` 中出现 `isWorkBuddy ?`、`productId === 'workbuddy'`、`WORKBUDDY_PRODUCT_ID`
   —— 能力判断必须走 `capabilities` / `sectionStates`。
2. `styles/tokens.css` 之外出现字面色值（`#hex`、`rgb(`、`hsl(`）。
3. 跨 feature 深路径 import（`features/a/internal/*`）。
4. 组件内直接 `invoke(` —— 必须经 `platform/` 适配层。
5. 新增 `any`（例外需 `// any-exception: 理由` 且登记）。
6. 同一份 UI 在两个 feature 里各写一遍（由评审 + 目录预算兜底）。

## 5. 复杂度治理

- **债务账本**：每次提交说明里写 `debt: +N / -M`，每月统计净变化。
- **删除优先**：新代码必须尽量让旧代码消失；只增不减的 PR 需要说明。
- **一页一 PR**：迁移期间禁止"顺手改另一页"。
- **新能力三件套**：单元测试 + 一条 e2e 冒烟 + capabilities 声明。

## 6. 迁移策略（strangler，禁止大爆炸）

1. **先壳后页**：P2 壳层统一必须先落地，否则页面迁移会二次返工。
2. **一页一迁**：每页独立提交，可单独回滚。
3. **迁完即删**：新实现通过验收后，**当场删除旧实现**，不留"以后清理"。
4. **顺序**：overview → accounts → checkin → environment → library → settings。
5. **每页完成定义**：旧文件删除 + 新 e2e 通过 + 截图对照 + 预算脚本绿。

## 7. 验收标准

- 每页首屏必须有**摘要层 / 主体层 / 动作层**三层
- 首屏底部不得出现 > 160px 纯空白
- 空态必须给"下一步动作"
- 全量：`pnpm typecheck` / `pnpm build` / `pnpm test:e2e` 全绿 + `node scripts/ui-guard.mjs` 绿

## 8. 例外流程

需要破例时，在 `docs/architecture/exceptions.md` 追加一条：

```
- 文件/规则:
- 原因:
- 范围（多久/多大）:
- 退出条件:
- 登记人/日期:
```

未登记的破例视为违规；`ui-guard` 读取该文件作为白名单。
