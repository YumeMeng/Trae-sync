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

## 环境页进度（P4d，8 步）

- P4d-1 ✅ platform/environment.ts（24 条命令）+ environmentPlugins.ts（4 条）
- P4d-2 ✅ config.ts（16 类型 / 16 字段能力表 / 220 条文案，含两处已知前后端不一致，隔离在 ENV_KNOWN_COPY_MISMATCHES）
- P4d-3 ✅ traeEnvironment.ts（200 行）+ registry.ts（26 行），口径原样搬入、61 个 testid 登记
- P4d-4 ✅ workbuddyEnvironment.ts（两卡扩展）+ registry 补 workbuddy
- P4d-5 ✅ 数据 hook ×2（快照代次守卫 / 忙碌位与收编进度校验）
- P4d-6 动作 hook（6a 启停/修复/捕获 · 6b 收编/清理） / P4d-7 展示组件 ×5 / P4d-8 弹层+接线+删旧
- 关键风险：收编/清理编排（关实例→单事务→重启 + env_id 过滤 + 三个冲突错误码）不可简化

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
