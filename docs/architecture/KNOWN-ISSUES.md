# 已知问题（不计入架构例外，逐条跟踪）

> 迁移中"发现但不修"的行为差异记这里：修复需要单独取证与测试，不在 UI 合并中顺手改。

## K-01 健康检测进度事件的产品值不一致（2026-10-23 发现）

- 现象：后端 `health-check-progress` emit 的 `product_id` 是 `trae_cn`，而 `credential-refresh-progress` / `credits-refresh-progress` emit 的是 `work_cn`。
- 旧前端主路径恒传 `work_cn` 做匹配，导致**健康检测的进度事件一直被丢弃**（用户看不到健康检测进度）。
- 现状：P4c-5a 的 `useAccountBatchProgress` **逐字沿用旧口径**，不擅自"修正"，以免在 UI 重构中混入行为变更。
- 修复条件：后端三处 emit 统一为 `work_cn`（或前端按产品分别匹配），并补一条单测覆盖"健康检测进度可见"。
- 归属：独立票，不在 P4c 范围内。

## K-02 适配器（池面）数据只存在于测试注入（2026-10-23 记录）

- 现象：`traePool` / `poolStatusLabel` 来自 `platform/accountAdapter.ts`，生产路径不再使用适配器；`tests/AccountAdapter.test.tsx` 仍需要它。
- 约定：`useAccountsSnapshot` 保持纯数据面；适配器池由页面以**可选 prop** 注入（P4c-6 落地），不塞进 hook。
- P4c-9a 更新：`AccountWorkbench` 已删除（孤儿），适配器面现在只经 `AccountsPage` 的 `adapterPool` 可选 prop 进入。

## K-03 合并页与旧页面的三处表现差异（P4c-9a 删除旧实现时确认，2026-10-23）

删掉 `AccountCenterTrae` / `AccountDetail` / `AdapterAccountDetail` 后，以下三处**旧断言没有等价替代**，
经核对判定为「可放弃的表现差异」而非行为回归，记录在此以免被当成漏测：

1. **列表/卡片双视图切换（U-2）**：旧页有分段控件在宽行与卡片网格间切换并按产品记忆偏好
   （`shared/account/preferences`）；合并页只渲染宽行（`AccountsRows` 默认 `view="list"`），
   无切换入口、不读写偏好。数据面与行内容完全一致，差异只在排布形态。
   - 判定：可放弃。合并页目标是单一排布（UI-CHARTER 收敛），双视图属旧页遗留选项。
   - 若要恢复：`AccountsRows` 已支持 `view` prop，缺的是控件与偏好持久化。
2. **徽章样式类（`.slot-badge` / `.status-badge`）**：旧适配器卡片用 `.slot-badge` 类，合并页统一走
   `ui/Badge`（`.status-badge` 族）。**文案与色档语义不变**，只有 class 名变化。
   - 判定：可放弃（视觉族统一是 P4a/P5 的既定方向）。旧断言 `card.querySelector(".slot-badge")`
     已从 `tests/AccountAdapter.test.tsx` 移除，改为断言徽章文案。
3. **适配器面 region 名「TRAE 账号」→「账号」**：旧 `AccountCenter` 在适配器注入时把 region 名换成
   「TRAE 账号」，合并页统一用 `ACCOUNT_TEXTS.pageTitle`（「账号」，与真实面、WB 面一致）。
   - 判定：可放弃（同一页面不该因数据来源换标题）。e2e 与单测现按 `/^账号$/` 定位。

## K-04 合并页尚未接线的页头动作（P4c-9a 记录，2026-10-23）

合并页页头对 WB 面给出「刷新登录凭据 / 健康检测 / 保存当前登录账号 / 添加账号」四个动作 id，
但 `pageAdapters` 只登记了 `refreshLocalActionId`：点击后动作 id 原样透出、**没有命令执行**。
旧 `WorkBuddyPanel` 这四个动作都是实装的。

- 影响范围：仅 WB 账号页（TRAE 面的批量动作已接）。
- 现状：`sectionSlots.accounts.workbuddy` 仍指向旧 `WorkBuddyPanel`，故**生产路径不受影响**；
  待 WB 槽位切到合并页前必须先补这四条接线，否则会丢功能。
- 归属：WB 槽位迁移票（P4c-9b 或后续），不在 P4c-9a 边界内。

## K-05 批量进度的逐账号文案在合并页缺失（P4c-9a 发现 → **P4c-9c 已修复**，2026-10-23）

- 现象（发现时）：旧账号页在批量刷新额度/凭据/健康检测时，逐账号上屏
  「正在处理 {completed}/{total}：{name}」（旧 `AccountCenterTrae` L1140-1161 状态区；
  K-05 初记的「第 N/M 个」是转述，旧实现原文并无「第…个」字样）。合并页的
  `useAccountBatchProgress` **已取到** `progress`（含 `completed/total/name`），
  但 `AccountsPage` 只把它用于**算按钮禁用**（`pageAdapters` L90-92），不渲染任何进度文案。
- 影响（发现时）：批量操作期间用户只看得到按钮转圈，看不到「跑到第几个、正在处理谁」。
  这是 P4c 合并过程中**真实丢掉的可见反馈**，不是可放弃的表现差异。
- **修复（P4c-9c）**：新增 `features/accounts/components/AccountBatchProgressStatus.tsx`，
  由 `AccountsPage` 渲染在页头下方（旧骨架 `statusRegion` 同一位置），文案逐字沿用旧实现
  `正在处理 {completed}/{total}：{name}`；渲染闸门 = 该链忙碌 + 已收到该链进度事件，
  无进度即不渲染。健康链沿用旧 `healthProgressShown`（先健康链、再回退额度链）。
  凭据链补回旧实现的开批/收尾清进度（`useCredentialRefresh` 的 `onStarted`/`onFinished`），
  避免残进度把页头按钮永久禁用。回归护栏：`tests/accountBatchProgressStatus.test.tsx`（渲染层
  5 例）+ `tests/accountsPageBatchProgress.test.tsx`（事件 → 页面文案 2 例）。
- **残留差异（有意不补）**：旧实现在「忙碌但尚未收到首批事件」时另有一条回退文案
  （「正在查询最新额度…」「正在刷新登录凭据，不会打开新的 OAuth 登录…」「正在检测账号健康度…」）。
  合并页此时不渲染任何文案（页头按钮本身在转圈）。要不要补属独立小票，不影响逐账号反馈。
