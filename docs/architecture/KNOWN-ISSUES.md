# 已知问题（不计入架构例外，逐条跟踪）

> 迁移中「发现但不修」的行为差异记这里：修复需要单独取证与测试，不在 UI 合并中顺手改。
>
> **2026-10-24 P6-2b 收口**：六页 + P5 收敛已全部完成，本条清单同步收敛——**已闭环的条目只保留结论**（不再算待办），**已定案的有意变更**单列一节，仍有效的按风险排序。总索引见 [MIGRATION-STATUS](MIGRATION-STATUS.md) §四。

## 索引

| 编号 | 状态 | 类型 | 一句话 |
|---|---|---|---|
| K-01 | **待修** | 行为缺陷 | 健康检测进度事件 `product_id` 与前端匹配值不一致，进度被丢弃 |
| K-02 | 有效（说明） | 结构 | 适配器池面数据只存在于测试注入 |
| K-05 | 有效（小缺口） | 可见反馈 | 批量「忙碌但未收到首批事件」时缺回退文案 |
| K-06 | **待补测** | 测试缺口（写操作） | WB 详情四写操作缺命令级渲染断言 |
| K-07 | **待补测** | 测试缺口（写操作） | 环境页账号捕获两例缺渲染断言 |
| K-10 | **待补测** | 测试缺口（数据可信度） | 库页 G1/G2/G5 等缺渲染级用例 |
| K-09 | 已定案 | 有意变更 | WB 会话时间格式变化，接受新格式 |
| K-11 | 已定案 | 有意变更 | WB 设置页读取门禁改「可见才读」 |
| K-13 | 已定案 | 有意修复 | 产品徽标颜色恢复为设计靛蓝 |
| K-03 | **已闭环** | 可放弃差异 | 合并页与旧页三处表现差异（判定可放弃） |
| K-04 | **已闭环** | 已修复 | 合并页页头四个 WB 动作未接线 → P4c-9b-1 修复 |
| K-08 | **已闭环** | 已修复 | 主库消息「思考轨迹」展开列表 → P4e-7/8 接线 |
| K-12 | **已闭环** | 已完成 | 设置页旧变体全部删除 → P4f-7 |

---

## 一、仍有效：待修 / 待补测

### K-01 健康检测进度事件的产品值不一致（2026-10-23 发现，**待修**）

- 现象：后端 `health-check-progress` emit 的 `product_id` 是 `trae_cn`（`credential_handover.rs` 的 `check_trae_cn_account_health`），而 `credential-refresh-progress` / `credits-refresh-progress` emit 的是 `work_cn`。
- 旧前端主路径恒传 `work_cn` 做匹配，导致**健康检测的进度事件一直被丢弃**（用户看不到健康检测进度）。
- 现状：P4c-5a 的 `useAccountBatchProgress` **逐字沿用旧口径**，不擅自"修正"，以免在 UI 重构中混入行为变更。P6-2b 复核：该口径仍在（`pageAdapters` 的 `progressId: "work_cn"`）。
- 修复条件：后端三处 emit 统一为 `work_cn`（或前端按产品分别匹配），并补一条单测覆盖"健康检测进度可见"。
- 归属：独立票，不在 UI 重构范围内。

### K-02 适配器（池面）数据只存在于测试注入（2026-10-23 记录）

- 现象：`traePool` / `poolStatusLabel` 来自 `platform/accountAdapter.ts`，生产路径不再使用适配器；`tests/AccountAdapter.test.tsx` 仍需要它。
- 约定：`useAccountsSnapshot` 保持纯数据面；适配器池由页面以**可选 prop** 注入（P4c-6 落地），不塞进 hook。
- P4c-9a 更新：`AccountWorkbench` 已删除（孤儿），适配器面现在只经 `AccountsPage` 的 `adapterPool` 可选 prop 进入。P6-2b 复核：`App.tsx` 传 `AccountsPage` 时不传该 prop，生产路径确实零使用。
- 判定：**结构说明而非缺陷**；随适配器面退役一并清理，不单独开票。

### K-05 批量进度的回退文案在合并页缺失（P4c-9a 发现 → **P4c-9c 已修复主体，残留有意不补**）

- 原缺口（已修复）：旧账号页批量刷新额度/凭据/健康检测时逐账号上屏「正在处理 {completed}/{total}：{name}」，合并页只把 `progress` 用于按钮禁用、不渲染文案。
- 修复（P4c-9c）：`AccountBatchProgressStatus.tsx` 渲染在页头下方（旧骨架 `statusRegion` 同一位置），文案逐字沿用旧实现；渲染闸门 = 该链忙碌 + 已收到该链进度事件。凭据链补回旧实现的 `onStarted`/`onFinished` 清进度，避免残进度把按钮永久禁用。护栏：`tests/accountBatchProgressStatus.test.tsx`（5 例）+ `tests/accountsPageBatchProgress.test.tsx`（2 例）。
- **仍有效的小缺口（有意不补）**：旧实现在「忙碌但尚未收到首批事件」时另有一条回退文案（「正在查询最新额度…」「正在刷新登录凭据，不会打开新的 OAuth 登录…」「正在检测账号健康度…」）。合并页此时不渲染任何文案（页头按钮本身在转圈）。P6-2b 复核：这三句在 `src/` 中零命中，缺口仍存在。要不要补属独立小票，不影响逐账号反馈。

### K-06 WB 详情四写操作缺命令级渲染断言（P4c-9b-2 确认，2026-10-23，**待补测**）

旧 `WorkBuddyPanel`（617 行）/ `WorkBuddyAccountDetail`（444 行）与 `tests/WorkBuddyPanel.test.tsx`（774 行 / 30 例）随 WB 槽位切到合并页删除。逐条核对后：表现差异均可放弃，**只有这一项是真实缺口**。

1. **可放弃的表现差异**（与 K-03 同口径，不再逐条展开）：① 列表/卡片双视图切换消失（合并页目标单一排布；旧偏好 key `accounts.workbuddy.view` 成惰性残留，无功能影响）；② 行内可见「使用中」chip → accent 徽章 + 隐藏锚点；③ 详情容器锚点改名（`workbuddy-account-detail` → `role=region aria-label="账号详情"`）。详情页 20 余个字段/动作 testid 与文案**逐字保留**，由 `tests/accountsDetailView.test.ts` 锁定。
2. **可放弃的覆盖缺口**：WB 详情「参与开关不可读取」中性态无渲染断言（纯展示分支，builder 分支存在）；WB 登录四个专码（`already_in_progress` / `poll_timeout` / `auth_url_untrusted` / `real_mode_required`）未单独断言（映射表 `loginErrorText` 是纯函数且逐字对照旧实现，主路径已有 11 例护栏）。
3. **真实缺口（不可放弃，属独立小票）**：WB 档案管理四写操作——`set_workbuddy_account_display_name` / `set_workbuddy_account_archived` / `set_workbuddy_account_auto_checkin` / `purge_workbuddy_accounts`——的**端到端渲染断言缺位**。`tests/accountsDetailView.test.ts` 只锁 builder 产出的字段/动作 id 与文案，不点按钮、不断言命令参数。
   - 缓解：命令参数形状与文案在 `detailActionCommands.ts` 中逐字对照旧实现，且 TRAE 面已用同一条分发路径验证过。

### K-07 环境页旧测试迁移后的覆盖缺口（P4d-8b-2a，2026-10-23，**待补测 1 项**）

旧 `tests/EnvironmentPage.test.tsx`（38 例）与 `tests/WorkBuddyPages.test.tsx` 的 2 例 WB 环境用例随旧组件退场，统一迁到 `tests/environmentPage.test.tsx`（现 24 例，锚点全部指向合并页）。逐条核对旧 40 例后：

1. **可放弃**（多场景合并 / 已有用例间接覆盖 / 纯展示）：副环境关闭三态回执（与主库共用同一函数与文案表）、`environment_close_failed` 错误码映射（`tests/safeUiError.test.ts` 独立护栏）、副环境「归入」步骤色档类名。
2. **真实缺口（不可放弃，属独立小票）**：**账号捕获两例**——`capture_trae_cn_account` 成功、`capture_account_already_registered` 去重。合并页 `EnvironmentCapture` + `useEnvironmentActions.captureAccount` 逐字搬旧实现（四个错误码前缀映射在 `useEnvironmentActions.ts`），但**新页没有对应的渲染断言**（P6-2b 复核：`tests/` 内 `capture_trae_cn_account` 零命中）。写操作——捕获会把账号写进池。
3. **已修复**：环境状态刷新失败仍保留成功结果（原判回归）→ **P4d-8b-2b**。`useEnvironmentSnapshot` 区分首读与刷新：首读失败仍清快照 + 落 `error`，**已有快照时刷新失败保留快照**，失败经 `EnvironmentReadOutcome` 交回调用方；各「成功 + 重读失败」路径按既有文案补提示、不再翻错误卡（收编 / 清理 / 登录 / 创建 / 改名 / 删除 / 修复七处）。护栏：`tests/environmentPage.test.tsx` 新增 2 例。

### K-10 库页旧测试迁移后的覆盖缺口（P4e-9a/b，2026-10-24，**待补测**）

旧 `LibrarySessionsPanel.test.tsx`（54）/ `MasterLibraryDetail.test.tsx`（15）/ `WorkBuddyLibraryPage.test.tsx`（5）/ `PluginWorkbench.test.tsx`（13）随旧组件退场删除，统一迁到 `tests/libraryPage.test.tsx`（P4e-9a 23 例 → P4e-9b 31 例）。逐条对应表见 `.scratch/cmdr/P4e-9a-report.md` §3。P4e-9b 已补 G3（归档收尾四态与恢复条）4 例、G4（写门禁四态）4 例，两项已关闭。

#### 真实缺口（写操作 / 数据可信度，须独立票补，不可放弃）

1. **G1 切库读取代次隔离**（旧 6 例全缺）：旧页专测「切库后旧列表与查看器立即清空」「迟到历史响应不覆盖新库」「旧库迟到失败不写当前错误态」「旧库迟到台账不污染同 id 查看器」「同 id 下迟到 open / load-older 不覆盖」「同 id 下旧库迟到 load-older 不拼入」。实现是 `useTraeLibrary` 的 `generation`/`viewerTicket` 双票守卫（P4e-5a 已写，P6-2b 复核仍在），但**新页没有任何渲染级用例**用 `rerender` 换 `libraryId` 并注入延迟响应来验证。错库数据串到当前库是数据可信度问题。
   - 补法：单文件用 deferred promise + `rerender`。
2. **G2 轮询指纹**（旧 1 例）：`get_master_history` 带 `previous`/`previousCurrentUserId`、`status="unchanged"` 时不刷台账。新页代码路径（`useTraeLibrary.load(false)` + 5s interval）保留，但无测试；`fake timers` 在合并页尚未用过。风险低（纯读取），归入与 G1 同批。
3. **G5 CN 跨账号归并**（旧 5 例全缺）：`library-foreign-group` / `library-foreign-project-*` / `library-merge-entry-*` 入口条件、`preview_trae_cn_project_merge` → `trae-cn-merge-confirm` 单次确认 → `merge_trae_cn_project`、`trae-cn-merge-progress` 事件推进按钮文案、预览/归并失败的安全通知。实现（`useLibraryMergeActions` 三链 + `useMergeActions` 进度白名单）已在，**无新页用例**；进度事件的 `STAGES` 白名单尤其没有护栏。P6-2b 复核：`tests/` 内 `preview_trae_cn_project_merge` / `merge_trae_cn_project` 零命中。

#### 建议补的低成本项（读取/展示分支）

4. **G7 插件工作台两条**：`plugin_tab_no_account` 的页内文案、卸载 fail-soft 摘要（其余 7 项判定可放弃，映射与文案在 `pluginTab.ts` / `usePluginActions.ts` 逐字搬旧实现）。P6-2b 复核：`tests/` 内 `plugin_tab_no_account` 零命中，缺口仍有效。
5. **G8 WB 软删开关**：`workbuddy-library-include-deleted` 切换后重读带 `includeDeleted: true`（旧锚 `workbuddy-library-messages` 有意不再存在，锚点变化可放弃）。
6. **G9 页面级 `active=false` 与副库标题**：`active=false` 零 IPC、副库标题用环境名 + `libraryId` 解析（统计失败/无备份属纯展示，可放弃）。
7. **G12 选择联动反向取消**：取消会话连带取消项目（`useLibrarySelection` 逐字搬旧实现，新用例只覆盖正向「勾项目连带会话」）。属写操作目标集合的正确性。

#### 判定可放弃（不再逐条跟踪）

G6 消息着色/接力 tab 的 DOM 断言（纯派生 `tests/libraryRelayLegs.test.ts` 8 例已锁）、G10 归并/删除的刷新失败回执（同一 `reload` 辅助，成功回执与警告次序在代码里单点）、G11 空态分支（两档共用锚点，仅文案与按钮有无之差）、G13 合并失败安全文案（`safeUiErrorMessage` 词表有 39 例独立护栏）。G9 中的「环境状态读取失败显示错误提示」在新页**行为已变**：环境档案是旁路（失败落 `null`），信息卡显示「—」而非错误条，属 P4e-5a 的有意降级，不补测试。

#### 测试数变化

| 项 | P4e-9a 前 | P4e-9a 后 | P4e-9b 后 |
| --- | --- | --- | --- |
| 测试文件 | 34 | 31（删 4 增 1） | 31（不变） |
| 用例 | 413 | **349**（-87 旧 +23 新） | **357**（+8：G3 四例 + G4 四例） |

`pnpm test` / `pnpm typecheck` / `pnpm guard`（新增 0）均 exit 0。P6-2b 全量基线：32 文件 / 359 例。

---

## 二、已定案：有意变更，无需处理

### K-09 WB 库页会话时间格式变化（2026-10-24 决定并接受）

- 旧：`toLocaleDateString()` 只到日，随宿主 locale 漂移（同一版本不同机器显示不同）。
- 新：复用 `formatSessionTime`（注入 now），显示「10月24日 03:00」，null 仍为「—」。
- 决定：**接受新格式**——旧格式的 locale 漂移本身就是缺陷；新格式确定、可测、可断言。影响面仅 WB 库页会话行时间；TRAE 侧本就用 `formatSessionTime`。

### K-11 WB 设置页读取门禁变更：挂载即读 → 可见才读（P4f-5，2026-10-24，有意行为变更）

- 变更前：WB 设置页两个子组件（旧 `WorkBuddyAutoCheckinSettings` / `WorkBuddyBackupSection`）**挂载即读**——不接 `active`、无门禁、无读取代次守卫。App 恒挂载全部页面（`hidden` 只是 CSS 隐藏），故用户从未打开设置页时三路 IPC 照旧发出。
- 变更后（P4f-5 合并页）：读取门禁统一取 **TRAE 口径**——`active` 才发起 + 逐路读取代次守卫（晚到的旧快照不覆盖新值）。能力表 `SettingsCapabilities.readGate` 两产品同值 `active-gated-version-guarded`（原 WB 的 `mount-immediately` 档随合并退役）。
- 判定：**有意行为变更**（减少后台 IPC，与 TRAE 面、其余五页门禁口径一致），非回归。设计依据见 `.scratch/cmdr/P4f-design.md` §2 差异表「读取门禁」行与 §4 风险点 5。护栏：`tests/settingsPage.test.tsx`「WB 读取门禁」。
- 随门禁一并变化（可放弃，不补）：① 首次进入设置页**首帧**由「默认值直渲（09:30 / 空链）」变为「先空链壳、读到后填值」；② 设置页不可见期间不再有后台 IPC 回写。

### K-13 产品徽标颜色恢复为设计靛蓝（P5-1b，2026-10-24，有意修复 / 有可见变化）

- 修复前：`src/styles.css` 的 `.product-badge--trae` 写 `var(--color-accent, #3b82f6)`，`.product-badge--neutral` 写 `var(--color-text-secondary, #94a3b8)`。这两个变量**全仓从未定义**，故 fallback **恒生效**：TRAE 徽标实际渲染为 Bootstrap 蓝 #3b82f6、中性徽标为 slate-400 #94a3b8，均与 DESIGN_TOKENS 契约不符（契约：唯一品牌色 `--accent` = #4f46e5 靛蓝；弱文字 `--text-3` = #9aa3b5）。该文件此前不在 `scripts/ui-guard.mjs` 的字面色值扫描域内，属**扫描盲区**。
- 修复（P5-1b）：① guard 字面色值规则新增根聚合入口 `src/styles.css`，盲区并入扫描域；② 9 处字面色值全部 token 化——TRAE 走既有 `--accent`，中性走既有 `--text-3`，WorkBuddy 紫调新增 `--product-wb`（值取自原字面 #a78bfa，视觉零变化）；三处半透明底/描边统一改 `color-mix(in srgb, <token> 14%|32%, transparent)`；③ 两个悬空变量**不再被引用**，故不在 tokens.css 里为错误命名补定义（补定义会让 Bootstrap 蓝/石板灰变成「合法 token」，与契约冲突）。
- **可见变化（有意）**：TRAE 徽标 Bootstrap 蓝 #3b82f6 → 设计靛蓝 **#4f46e5**；中性徽标 slate-400 → `--text-3` **#9aa3b5**（肉眼近乎不可辨）；WorkBuddy 徽标颜色零变化。
- 判定：**有意修复，非回归**。护栏：`node scripts/ui-guard.mjs` 现覆盖该文件（探针实测注入 `#123456` 即报「新增 1 / 既有 0」、exit 1）。**无新增单元测试**（颜色属视觉层，由 guard + DESIGN_TOKENS 契约守护）。

---

## 三、已闭环（保留结论，不再作为待办）

### K-03 合并页与旧页面的三处表现差异（P4c-9a，2026-10-23，**判定可放弃，已闭环**）

删掉 `AccountCenterTrae` / `AccountDetail` / `AdapterAccountDetail` 后，三处旧断言没有等价替代，经核对判定为「可放弃的表现差异」而非行为回归：

1. 列表/卡片双视图切换（U-2）消失——合并页目标单一排布；数据面与行内容完全一致，差异只在排布形态。若要恢复：`AccountsRows` 已支持 `view` prop，缺的是控件与偏好持久化。
2. 徽章样式类 `.slot-badge` → `ui/Badge`（`.status-badge` 族）——**文案与色档语义不变**，只有 class 名变化；旧断言已改为断言徽章文案。
3. 适配器面 region 名「TRAE 账号」→「账号」——同一页面不该因数据来源换标题；e2e 与单测现按 `/^账号$/` 定位。

### K-04 合并页页头四个 WB 动作未接线（P4c-9a 发现 → **P4c-9b-1 已修复**，2026-10-23，**已闭环**）

- 发现时：`pageAdapters` 只登记了 `refreshLocalActionId`，点击后动作 id 原样透出、没有命令执行（当时生产路径未受影响，槽位仍指旧 `WorkBuddyPanel`）。
- 修复：新增 `poolActionCommands.ts`（WB 四条池级动作的命令 / 文案 / 确认弹层 / 刷新时机，逐字沿用旧实现）与 `components/usePoolHeaderActions.tsx`（编排 + 四路独立忙碌位 + 捕获确认层），经 `pageAdapters` 的 `poolActions` 接缝登记。呈现纪律：四个命令都只有池级/客户端级单条回执，一律按 `notify.success / info / error` 三档汇报，不拆成逐账号摘要。动作 id 与弹层 testid 逐字沿用。回归护栏：`tests/accountsPagePoolActions.test.tsx`（11 例）。
- 无残留行为差异：旧 `runDeepHealth` 实际只发通知、不写状态、不重读数据，合并页同一形态；四动作的禁用口径、忙碌转圈、「取消等待」时机均与旧页一致。

### K-08 主库消息「思考轨迹」展开列表（2026-10-24 记录 → **P4e-7/8 已闭环**）

- 发现时：旧库面板消息轨迹行有 thoughts 展开列表，P4e-3b 的 MessageView 未包含该字段。
- 闭环：`traeLibrary.viewerOf` 从 `SessionMessageDto.content.thoughts` 取词，`MessageFlow.MessageBody` 渲染展开列表；`tests/libraryPage.test.tsx`「点会话出查看器」逐条断言「先定位 / 再修复」两条 thoughts 可见。

### K-12 设置页旧变体删除（P4f-5 保留 → P4f-6 部分删除 → **P4f-7 已解决**，2026-10-24）

- **状态：已解决**。设置页五个旧文件全部删除，无任何残留的生产引用或测试引用。

| 票 | 删除文件 | 行数 |
| --- | --- | --- |
| P4f-6 | `TraeSettingsVariant.tsx` / `WorkBuddySettingsVariant.tsx` / `WorkBuddyAutoCheckinSettings.tsx` + 空目录 | 556 + 46 + 109 |
| P4f-7 | `WorkBuddyBackupSection.tsx` | 265 |
| P4f-7 | `shared/checkin/AutoCheckinSettingsCard.tsx` | 134（此前登记 131，实测 134） |

- P4f-6 遗留的唯一阻塞点（旧 WB 备份面 4 例直接 `render(<WorkBuddyBackupSection />)`）已在 P4f-7 解除：4 例**全部迁入** `tests/settingsPage.test.tsx`，改为经 `SettingsPage`（`productId="workbuddy"`）渲染，锚点与文案逐字不变。无案例被删。
- `tests/WorkBuddyPages.test.tsx` 同时移除那个模块级 import 与 4 例（-102 行），该文件现只剩总览/签到页行为。
- 过渡接缝的后续处理**已由 P5-4b 完成**：`sectionSlots` 删除（-152 行）、`SettingsPanel` 直接渲染页面、E-01 例外撤销；`tests/SettingsPanel.test.tsx` 的用例仍按 `SettingsPanel` → 新合并页路径跑，已实际覆盖新页。
