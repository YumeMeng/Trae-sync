# 迁移状态（2026-10-23 07:10，分支 feat/ui-refactor-20261022）

> 本文件是重构进度的事实来源。新会话接手时先读它 + `UI-CHARTER.md` + `UI-DIRECTION.md` + `KNOWN-ISSUES.md`。

## 已完成（均有提交、均绿）

| 阶段 | 内容 | 提交 |
|---|---|---|
| P2a | 页面槽位接缝（App.tsx 五处产品分支收敛为 resolveSection） | bbe95d8 |
| P2b | 债务闸门 ui-guard（基线 13 处 CSS 字面色值）+ WB 两条 e2e | 9ffabe3 |
| P4a | 总览页合并（删旧 504 行），诞生 src/ui 五个原子 | 50b4f15 |
| P4b-1..4 | 签到页合并（删旧 1,014 行，单测 457） | fb04f4d..08b260a |
| P4c-1a | platform/accounts.ts：26 条命令封装 | 2aff041 |
| P4c-2 | 视图模型 + 18 字段能力表 + 21 条文案 | 1501d77 |
| P4c-3/4 | TRAE/WB builder + 登记表 | d857640 / 739882a |
| P4c-5a/5b | 数据 hook + 动作/登录 hook | e7c7041 / ef4bf80 |
| P4c-6a..7c | 账号列表 + 详情接线，**e2e 41/41** | 34865f5..89863cb |
| P4c-8a/8b | 纯函数单测 14 例 + 页面集成单测 14 例（**485 全绿**） | 7ee62ca / 待提交 |

## 当前架构（账号页）

```
src/features/accounts/
  AccountsPage.tsx            261 行  页面装配（PAGE_ADAPTERS 表驱动）
  config.ts                   201 行  视图模型 + 能力表 + 文案
  registry.ts                  34 行  AccountsSnapshot → AccountsView
  traeAccounts.ts             179 行  TRAE builder
  workbuddyAccounts.ts        157 行  WB builder
  detailView.ts               150 行  详情六区构建
  detailSnapshot.ts            75 行  详情数据面装配
  pageAdapters.ts             121 行  产品接缝表
  AccountsSkeleton.tsx         49 行
  components/                  （Header/Rows/DetailPage/各 hook/弹层）
src/platform/accounts.ts      229 行  26 条命令
```

## 账号页：已完成（P4c-9b-2）

- TRAE 与 WorkBuddy **共用** `features/accounts/AccountsPage`，槽位只绑定 productId
- 旧实现全部删除（累计约 6,675 行）：AccountCenterTrae 1528 / AccountDetail 576 / AdapterAccountDetail 148 / AccountWorkbench 17 / WorkBuddyPanel 617 / WorkBuddyAccountDetail 444 + 5 个旧测试文件
- 当前基线：**410 单测 / 41 e2e / guard 新增 0**
- 遗留：K-06 缺口 5（WB 档案四写操作的命令级断言缺位，建议一票补齐）

## 环境页：已完成（P4d-8b-2c）

- 新旧全部收敛到 `features/environment/`（页面 183 行），旧 1,764 + 220 行已删除
- 累计删除：EnvironmentPage 1764 / WorkBuddyEnvironmentPage 220 / 旧测试 38 例（迁为 22 例 + 后续 2 例）
- 修复了 K-07#4（刷新失败丢回执的真回归）：读取层返回 outcome，五个调用点补提示
- 当前基线：**394 单测 / 41 e2e / guard 新增 0**
- 已知遗留：K-07 项 3（账号捕获渲染断言）

## 主库页：已完成（P4e-9c）

- 收敛到 `features/library/`（页面 286 行 + 33 文件拆分），旧 5 文件 **4,183 行**已删除
- 纯逻辑先抽（sessionTree 218 / relayLegs 200）+ 19 例单测锁定
- 修复安全缺口：草稿存储越权（G4）、归档重试忙碌位（G3）
- 当前基线：**357 单测 / 41 e2e / guard 新增 0**
- 遗留：K-10 中 G1/G5 与 G3 四项低风险分支

## 设置页：已完成（P4f-7）

- `features/settings/` 13 文件，旧 5 文件 **1,103 行**删除（TraeSettingsVariant 556 / WorkBuddySettingsVariant 46 / WorkBuddyAutoCheckinSettings 109 / WorkBuddyBackupSection 265 / AutoCheckinSettingsCard 134）
- 唯一的“零测试改动”迁移：旧 26 例设置测试一行未改即全绿
- 当前基线：**359 单测 / 41 e2e / guard 新增 0**
- 已知：K-11（WB 读取门禁变更有意）保留；K-12 已解决

## 六页完成后的收敛阶段（P5/P6）

- P5-0 CSS + E-01 只读审计
- P5-1..n：CSS 四层、13 处字面色值换 token、ui-guard 基线归零、删过渡接缝（sectionSlots + SettingsPanel 包装）
- P6：MODULE_MAP / CONTEXT / tickets / ADR 索引回填 + CI 接入 pnpm check

## 之后

- P4d environment（1,683 行，先拆巨石）
- P4e library（2,419 + 540 行）
- P4f settings（已拆 Trae/WB 变体，合一即可）
- P5 收敛：CSS 四层、清 13 处字面色值、基线归零、删 E-01
- P6 文档 + CI 接入 pnpm check

## 铁律（每票都适用）

1. 一票 ≤300 行新代码（Trae 执行者输出上限 16k token，超了就零产出）
2. 每票必须自己跑：typecheck / test / guard；接线票加 build + e2e
3. 不许删断言、不许放宽条件；改断言必须说明“语义等价”
4. 切号事务、DPAPI、登录三类收尾、设备 productId="work_cn"、批量凭据二次确认——不可简化
5. 完成定义：旧文件已删 + e2e 绿 + guard 绿 + 截图自检 5 问

## 验证命令

```powershell
cd D:\work\Trae-sync
pnpm typecheck; pnpm build; pnpm test; node scripts/ui-guard.mjs; pnpm test:e2e
```
