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

## K-04 合并页尚未接线的页头动作（P4c-9a 记录 → **P4c-9b-1 已修复**，2026-10-23）

- 现象（发现时）：合并页页头对 WB 面给出「刷新登录凭据 / 健康检测 / 保存当前登录账号 /
  添加账号」四个动作 id，但 `pageAdapters` 只登记了 `refreshLocalActionId`：点击后动作 id
  原样透出、**没有命令执行**。旧 `WorkBuddyPanel` 这四个动作都是实装的。
- 影响（发现时）：仅 WB 账号页（TRAE 面的批量动作已接）。`sectionSlots.accounts.workbuddy`
  当时仍指向旧 `WorkBuddyPanel`，故生产路径未受影响。
- **修复（P4c-9b-1）**：新增 `features/accounts/poolActionCommands.ts`（WB 四条池级动作的
  命令 / 文案 / 确认弹层 / 刷新时机，逐字沿用旧 `WorkBuddyPanel` L172-342 + L574-589）与
  `components/usePoolHeaderActions.tsx`（编排 + 四路独立忙碌位 + 捕获确认层），经
  `pageAdapters` 的 `poolActions` 接缝登记；页头分发与忙碌位抽到
  `components/useAccountsHeaderWiring.ts`（AccountsPage 回到 300 行预算内）。
  呈现纪律：这四个命令都只有**池级 / 客户端级单条回执**，故一律按旧实现的
  `notify.success / info / error` 三档汇报，**不拆成逐账号摘要**（不使用 `notify.summary`）。
  动作 id 与弹层 testid 逐字沿用（`workbuddy-renew-credentials` / `workbuddy-deep-health-button` /
  `workbuddy-capture-button` + `workbuddy-capture-dialog` / `workbuddy-add-account-button` +
  `workbuddy-login-cancel`）。回归护栏：`tests/accountsPagePoolActions.test.tsx`（11 例）。
- **无残留行为差异**：旧实现注释称健康检测「结果常驻健康卡」，但旧 `runDeepHealth` 实际只发通知、
  不写任何状态、不重读数据；合并页同一形态（三档 `liveHealthLabel` 文案逐字保留，零写副作用）。
  四动作的禁用口径、忙碌转圈、「取消等待」按钮时机均与旧页一致。

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

## K-06 WB 槽位切到合并页后的表现差异与未覆盖项（P4c-9b-2 删除旧实现时确认，2026-10-23）

`sectionSlots.accounts.workbuddy` 于 P4c-9b-2 切到 `AccountsPage`（`productId=WORKBUDDY_PRODUCT_ID`），
旧 `WorkBuddyPanel`（617 行）/ `WorkBuddyAccountDetail`（444 行）与 `tests/WorkBuddyPanel.test.tsx`（774 行 / 30 例）
随之删除。以下为逐条核对旧 30 例后确认的**表现差异**与**覆盖缺口**。

1. **列表/卡片双视图切换（U-2）**：与 K-03 差异 1 同因——WB 旧页有分段控件并在
   `accounts.workbuddy.view` 记忆偏好，合并页只渲染宽行、不读写该偏好。
   - 判定：可放弃（同 K-03 差异 1，合并页目标是单一排布）。WB 的排序档（`added` / `name`）
     仍保留且行为一致（`tests/accountsPage.test.tsx`「WB 池面：无过滤档，排序档仅添加顺序与名称」）。
   - 副作用：旧偏好 key `accounts.workbuddy.view` 不再被读取，属惰性残留（不清理，无功能影响）。
2. **WB 行锚点的可见「使用中」chip → accent 徽章 + 隐藏锚点**：旧行渲染可见 chip，合并页由
   `AccountsRows` 统一为徽章（视觉由 `ui/Badge` 表达）+ `workbuddy-pool-current-*` 隐藏锚点承载旧 testid。
   - 判定：可放弃（与 K-03 差异 2 同一族收敛方向）；锚点与语义零变化，e2e/单测照旧可用。
3. **详情页容器锚点改名**：旧详情自带 `workbuddy-account-detail` / `workbuddy-account-detail-back`；
   合并页统一为 `role=region aria-label="账号详情"` + `account-detail-back`。
   - 判定：可放弃（同一页面不该因产品换锚点，与 K-03 差异 3 同口径）。e2e 已按新锚点更新。
4. **详情页字段/动作锚点逐字保留**：`workbuddy-detail-status` / `-credits` / `-alias` /
   `-archived-toggle` / `-participation-toggle` / `-purge` 等 20 余个 testid 与文案全部沿用旧实现
   （`detailView.ts` 逐字对照旧 `WorkBuddyAccountDetail`），由 `tests/accountsDetailView.test.ts` 锁定。

### 覆盖缺口（未补，逐条给出理由）

5. **WB 档案管理的命令级渲染断言（备注名 / 归档 / 参与开关 / 删除）**：旧 30 例中有 4 例断言
   「点保存 → 发出 `set_workbuddy_account_display_name` / `set_workbuddy_account_archived` /
   `set_workbuddy_account_auto_checkin` / `purge_workbuddy_accounts` 且参数形状正确」。
   合并页这条链的实现是 `detailActionCommands`（命令与文案逐字搬自旧实现）+ `useDetailFieldEdits` /
   `useDetailActions`（通用分发，TRAE 面同一条代码路径），但**WB 分支的端到端渲染断言目前缺位**：
   `tests/accountsDetailView.test.ts` 只锁 builder 产出的字段/动作 id 与文案，不点按钮、不断言命令参数。
   - 判定：**真实缺口，不可放弃**（这四条是写操作）。补它需要一条 WB 详情页的命令级用例，
     属独立小票（本票边界为「切槽位 + 删旧实现」，不新增详情页测试面）。
   - 缓解：命令参数形状与文案在 `detailActionCommands.ts` 中逐字对照旧实现，且 TRAE 面已用同一条
     分发路径验证过（`tests/accountsPage.test.tsx` / `accountsDetailView.test.ts`）。
6. **WB 详情「参与开关不可读取」中性态**：旧页在 `list_workbuddy_checkin_accounts` 失败时渲染
   「参与状态暂时不可读取，请返回后重试。」（`workbuddy-detail-participation-unknown`）。
   合并页 builder 保留了该分支，但**无渲染断言**。
   - 判定：可放弃（纯展示分支，无写操作；builder 分支存在且由 `detailView.ts` 的 `wbParticipation` 保证）。
7. **WB 登录四个专码的渲染断言**（`already_in_progress` / `poll_timeout` / `auth_url_untrusted` /
   `real_mode_required`）：旧 30 例只覆盖 `already_in_progress` 一例。合并页的码 → 人话映射在
   `poolActionCommands.ts` 的 `loginErrorText`（逐字搬自旧 `workbuddyLoginErrorText`），
   而 `tests/accountsPagePoolActions.test.tsx` 覆盖了 begin/complete/cancel/acceptance/取消五条主路径，
   **未单独断言这四个专码**。
   - 判定：可放弃（映射表是纯函数且逐字对照旧实现；主路径已有 11 例护栏）。若要补，最省的方式是
     给 `loginErrorText` 加纯函数用例。

## K-07 环境页旧测试迁移后的覆盖缺口（P4d-8b-2a，2026-10-23）

旧 `tests/EnvironmentPage.test.tsx`（38 例）与 `tests/WorkBuddyPages.test.tsx` 的 2 例 WB 环境用例
随旧组件退场，统一迁到 `tests/environmentPage.test.tsx`（22 例，锚点全部指向合并页
`features/environment/EnvironmentPage`）。以下为逐条核对旧 40 例后确认的**未迁项**：
第 1、2、5 项属「多场景合并 / 已有用例间接覆盖」（可放弃）；第 3、4 项是**真实缺口**（须独立票补）。

1. **副环境关闭三态回执**（旧 `force_closed` / `not_running` 各一例）：合并页
   `useEnvironmentActions.closeEnv` 与主库共用同一函数、同一份 `ENV_TEXTS` 三态文案，
   主库关闭已在新用例断言（取消零命令 → 确认 → `notify.success` → 整组重读）。副环境差异
   仅在 `labelOf` 的显示名回退（`cardNameOf` 已按 env_id 取卡片名），属纯展示分支。
   - 判定：可放弃（同一代码路径 + 同文案表；副环境回执分支无独立行为）。新用例已断言
     副环境的「仅运行中才出关闭按钮」（进程未知用例）与卡片动作集合（五项操作用例）。
2. **关闭失败错误码映射**（`environment_close_failed`）：`safeUiErrorMessage` 词表逐字未改，
   且该码**已有独立护栏** `tests/safeUiError.test.ts`（"environment_close_failed 使用稳定提示"）。
   - 判定：可放弃（词表纯函数已单独锁定；页面侧只需断言「调用 `safeUiErrorMessage`」，
     删除被拒用例已覆盖该机制）。
3. **账号捕获两例**（`capture_trae_cn_account` 成功 / `capture_account_already_registered` 去重）：
   合并页 `EnvironmentCapture` + `useEnvironmentActions.captureAccount` 逐字搬旧实现，
   但**新页没有对应的渲染断言**。
   - 判定：**真实缺口，不可放弃**（写操作：捕获会把账号写进池）。属独立小票——
     本票边界是「迁旧页渲染/弹层行为」，捕获块在 P4d-8a-2 已单独自检过
     （`.scratch/cmdr/p4d8a2-sanity.test.tsx`），未沉淀为正式护栏。
4. **环境状态刷新失败仍保留成功结果**（旧 3 例：清理 / 收编 / 登录各一）——**已实测确认是回归，非可放弃项**
   → **P4d-8b-2b 已修复（2026-10-23）**，详见本条末尾「修复落地」。
   `useEnvironmentSnapshot.load()` 内部 `catch` 后**只落 error 状态、不重抛**，故三个调用点的
   `.catch(...)` 分支永不触发：`useEnvironmentLogin.done` 的 `notify.warning(loginRefreshFailed)`、
   `useEnvironmentIncorporation` 的刷新回调均无 catch，`ENV_TEXTS.incorporateRefreshFailed` /
   `refreshFailedSuffix` 全库无消费点（`view.texts` 亦无读者）。
   实测（`.scratch/cmdr/p4d8b2a-refresh-probe*.test.tsx`）：收编成功后环境状态读取失败 →
   弹层直接消失、完成回执「已归入 2 个账号…」与 `incorporate-done` 一并丢失、主库卡被
   整页错误卡替代，且 **notify 三档全空**（成功/错误/警告均未调用）；登录「完成」路径同形
   （无 warning，页面翻为错误卡）。旧页在同样情形下会保留回执并追加「环境状态刷新失败，请手动刷新。」。
   - 判定：**真实缺口，不可放弃**（用户看到的是「操作结果消失 + 整页报错」）。
   - 修复方向（独立票）：`refreshFull` 需要把失败**同时**交回调用方（返回 boolean 或抛错），
     再由三处调用点按旧文案补提示；或在 `EnvironmentPage` 的刷新回调里比对 `error` 状态后补
     `notify.warning`。属行为变更，不在测试迁移票内改。
   - **修复落地（P4d-8b-2b，2026-10-23）**：`useEnvironmentSnapshot` 区分首读与刷新——首读失败仍
     清快照 + 落 `error`（错误卡保留），**已有快照时刷新失败保留快照**，失败经
     `refreshLocal / refreshFull` 的新返回值 `EnvironmentReadOutcome`（`{ ok: false, error }`）交回
     调用方。各「成功 + 重读失败」路径按既有文案补提示、不再翻错误卡：收编在完成态记
     `refreshFailed` 并在弹层内追加 `ENV_TEXTS.incorporateRefreshFailed`（回执与「完成」按钮保留）、
     清理在成功回执尾部追加 `ENV_TEXTS.refreshFailedSuffix`、登录「完成」补
     `notify.warning(ENV_TEXTS.loginRefreshFailed)`、创建/改名/删除成功通知追加
     `refreshFailedSuffix`、修复成功后的重读失败补 `notify.warning`。
     护栏：`tests/environmentPage.test.tsx` 新增 2 例（收编成功但重读失败 / 清理成功但重读失败），
     断言「回执照旧 + 提示追加 + 主库卡保留 + 无整页错误卡 + 无 error 通知」。
5. **副环境「归入」的进度阶段高亮与失败步骤色档**（旧 1 例断言
   `[data-stage="restarting"]` 带 `switch-step--failed`）：新用例已断言
   `role=alert` 的「归入已完成，但环境未能自动启动」文案，**未断言步骤色档类**。
   - 判定：可放弃（类名是视觉实现细节；文案与 role 已锁行为，色档由 CSS 单点决定）。

### 测试数变化

| 项 | 前 | 后 |
| --- | --- | --- |
| 测试文件 | 32 | 32（删 1 增 1，`EnvironmentPage.test.tsx` → `environmentPage.test.tsx`） |
| 用例 | 410 | **392**（-38 旧环境页 +22 新环境页 -2 WB 环境用例） |

P4d-8b-2b 修复 K-07 项 4 时补 2 例（收编 / 清理的「成功但重读失败」），全量 **394**（24 例环境页）。

## K-08 主库消息「思考轨迹」展开列表待接线（2026-10-24 记录）

- 现象：旧库面板的消息轨迹行有 thoughts 展开列表（旧 LibrarySessionsPanel L2207-2213），P4e-3b 的 MessageView 未包含该字段，纯派生层不搬。
- 约定：装配层（P4e-7/8）必须从 SessionMessageDto 取 thoughts 并渲染，或在 P4e-7 扩 config 字段；**不许当作无此功能**。
- 归属：P4e-7/8 接线时闭环，届时删除本条。
