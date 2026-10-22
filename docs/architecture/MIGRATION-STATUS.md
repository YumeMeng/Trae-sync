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

## 账号页剩余票

| 票 | 内容 |
|---|---|
| P4c-6b-2b | 页头批量动作接线：刷新额度 / 健康检测（useAccountActions 注入命令） |
| P4c-6b-2c | 添加账号 / 登录流程（useAccountLogin + 登录弹层） |
| P4c-6b-2d | 行内切号（复用 MasterSwitchDialog）+ 工具账号选择 |
| P4c-6b-2e | 详情动作：单账号签到 / 重新登录 / 重置设备 / 删除 / 备注名 / 手机号补录 |
| P4c-9 | 删旧：AccountCenterTrae(1328) / AccountDetail(522) / AdapterAccountDetail(134) / WorkBuddyAccountDetail(424) / WorkBuddyPanel(593) / AccountWorkbench + 旧测试（AccountCenter 67、WorkBuddyPanel 26 等） |

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
