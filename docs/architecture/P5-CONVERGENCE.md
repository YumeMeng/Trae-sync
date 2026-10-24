# P5 收敛方案（来自 P5-0 审计）

# P5-0 只读审计：CSS 收敛 + 过渡接缝清理方案

> 分支 `feat/ui-refactor-20261022`。只读：未改 src/tests/e2e，未跑 build/test/e2e（`node scripts/ui-guard.mjs` 是只读扫描，已实跑）。
> 口径：静态解析 12 份 CSS 的规则块/类名/行/字节；全 src 类名消费点扫描（含模板字符串动态前缀、测试钩子、`querySelector` 字符串）。

## 0. 结论速览

- CSS 12 份 = **6,574 行 / 149,163 B**（另有聚合入口 `src/styles.css` 52 行 / 1,543 B）。**死规则 1,396 行 / 31,388 B = 21.0%**；**死类 122 / 560 = 21.8%**。
- 13 处字面色值：guard 实跑与基线逐字一致（accounts 4 / environment 3 / history 5 / workbench 1）。**另有 guard 盲区 9 处**：`src/styles.css` 的 `.product-badge` 家族不在扫描域，且引用了**全仓从未定义**的 `--color-accent` / `--color-text-secondary`（fallback 恒生效）。
- 接缝：六个 section **全部**已是「两槽位同一组件、仅 productId 不同」；`SettingsPanel.tsx` 只剩分发。E-01 可删，3 步（+1 可选）。
- 最大风险：**不是**删死类（e2e 断言与死类零交集），而是四层重排触发的 **cascade 翻转**——3 处跨文件真值冲突 + 2 处形状/色拆分，且无自动化断言覆盖。

## 1. CSS 审计

### 1.1 12 份文件（用途 / 大小 / 被谁引用）

| 文件 | 行 | 字节 | 顶层类 | 死类 | 消费源文件 | 用途与归属层 |
|---|---|---|---|---|---|---|
| tokens.css | 68 | 4,758 | 0（1 个 `:root`） | 0 | 全部 12 份 | **L1** 语义令牌唯一层 |
| base.css | 420 | 10,078 | 22 | 0 | 18 | **L2** 壳层 `app-shell/body/main/page`、`empty-state`、`skip-link`、雾斑+颗粒、窗控热区 |
| titlebar.css | 334 | 8,350 | 21 | 4 | 29 | **L2** `title-bar`(8) + `navigation-rail`(6)；组选择器顺带给 `.status-badge/.badge` 形状 |
| workbench.css | 1,548 | 34,748 | 106 | **72** | 59 | **L3** 共享控件 `btn/badge/status-badge/page-header/settings-panel/section-caption/settings-backup`（551 行）+ 已退役「历史工作台」全家桶（968 行死） |
| notify.css | 123 | 3,674 | 9 | 0 | 1（`platform/notify`） | **L3** sonner 主题，`sonner-` 前缀隔离 |
| accounts.css | 1,409 | 32,560 | 130 | **31** | 45 | **L3+L4 混层**：共享 `account-list/account-card/account-item/seg-control/slot-badge/account-center__profile`（745 行）+ 页面 `account-detail/mobile-prompt/remote-devices`（341 行） |
| checkin.css | 196 | 4,142 | 23 | 0 | 9 | **L3+L4**：`checkin-flow` 15 类跨 checkin/library 复用（122 行）+ `checkin-page/checkin-progress`（66 行） |
| environment.css | 730 | 14,402 | 83 | 7 | 18 | **L3+L4**：`switch-veil/switch-dialog` 被 7 个弹层复用（263 行）+ `env-card/env-checkup/env-self-check/env-login`（419 行） |
| history.css | 912 | 18,342 | 88 | 1 | 25 | **L4** 库对话面板 `lib-project/lib-session/lib-tree/chat-msg/leg-step/viewer-`（`LibrarySessionsPanel` 遗产，现由 `features/library` 消费） |
| master-detail.css | 524 | 11,304 | 50 | 1 | 41 | **L4** 库页壳 `master-detail/plugin-*/verification-*` |
| overview.css | 232 | 4,846 | 20 | 6 | 9 | **L4** 总览；**混入 3 条 `.settings-panel` 规则（错层，须上移）** |
| workbuddy.css | 78 | 1,959 | 8 | 0 | 8 | **L3+L4**：`workbuddy-row` 4 类被 library/settings 借用（33 行）+ 余 4 类（36 行） |
| `src/styles.css` | 52 | 1,543 | 5 | — | 1 | 聚合 `@import` 12 份 + `.product-badge` 家族（**guard 盲区，须移入 L3**） |

### 1.2 四层划分与每份文件归属

| 层 | 内容 | 行数 |
|---|---|---|
| **L1 tokens** | `tokens.css` | 68 |
| **L2 base** | `base.css` + `titlebar.css` | 754 |
| **L3 components** | `workbench.css` 余 551 + `notify.css` 71 + `styles.css` 的 `.product-badge` + 上移段：accounts 745、environment 263、checkin 122、overview 64、workbuddy 33、master-detail 8、history 4 | ≈1,861 |
| **L4 pages** | accounts 341、environment 419、history 881、master-detail 504、overview 104、workbuddy 36、checkin 66 | ≈2,351 |

落盘建议：`styles/{tokens.css, base.css, components.css, pages/{accounts,checkin,environment,history,master-detail,overview,workbuddy}.css}`；`@import` 顺序即层级（L1→L2→L3→L4）。保留「一页一文件」以便一页一 PR；上移 L3 段后各页面文件均在 300–900 行，无预算压力。

### 1.3 重复类名（跨文件 9 个）

**3 处真值冲突**：`.workbench-read`（overview.css:67/89 → workbench.css:29/828/1193，`min-height 100vh-108px` → `-120px` → `0`，终值由 import 顺序决定）；`.settings-panel`（overview.css:66-73 与 88-92 **同文件两段互相覆写**，`padding:24px` 重复声明）；`.settings-panel__section-heading`（overview.css:76 只改 `align-items`，workbench.css:160/170/273/1233 给 flex 骨架）。
**2 处形状/色拆分**：`.status-badge` / `.badge` / `.workbench__read-only-badge`（titlebar.css:99-101 组选择器给形状，workbench.css:3/123/133 给 padding/色；其中 `.badge` 与 `.workbench__read-only-badge` 零引用）。
**4 处良性分工**：`.btn`（基类在 workbench.css:749-825，accounts.css:454/1110 与 master-detail.css:195 只覆写 `margin/padding`）、`.page-header`（基类在 workbench，history.css:16 / master-detail.css:15 只 `flex:none`）、`.title-bar__winctl--close`（base.css:225 收圆角 vs titlebar.css:142/148 hover 红）。
同文件内重复 170 组（基础 + 状态修饰）属正常，不在清理目标内。
**CSS↔TS 跨语言耦合**：`.checkin-flow`（checkin.css:74）、accounts.css:9-25 stagger 规则、`App.tsx:26` `STAGGER_LISTS` 三处硬绑同一类名，改名即静默失效。

### 1.4 死类与死规则

- **workbench.css：72/106 类零引用**（P1 审计口径 68/105，差额来自动态前缀修正与裸 `.workbench`），**136 条纯死规则 / 968 行 / 22,028 B（63.4%）**。三簇：①`workbench__*` 历史工作台全家桶（header/summary/auth/workspace/tree/sessions/preview/node/message/search/select-row，含 2 段 `@media` 内规则）；②`workbench-read__*` 5 类 + `.workbench-read`；③`history-switcher` 全 15 类。另有 **22 条混合规则**（活类与死类同选择器组，如 `.workbench, .settings-panel, .workbench-read`、`.workbench__header-copy h1, .page-header h1`），只能改选择器、不能整块删。
- **accounts.css：31 类 / 42 条纯死规则 / 304 行 / 6,367 B（24.5%）**，集中在 `account-center__header/hero-*/summary/target-*/switch-state/checkin-*/boundary/advanced` 与 `account-card__credits-*`；另 7 条混合规则。
- 其余：environment 7 类/41 行、overview 6 类/54 行、titlebar 4 类/4 行（`.workbench__read-only-badge`、`.title-bar__context`，以及组选择器夹带的 `workbench__node--active/__session--active`）、history 1 类/18 行（`.history-page`）、master-detail 1 类/7 行（`.history-page--embedded`）。
- **测试相关、不进删除清单**：`.overview-checkin`（e2e `data-testid`，非 class）、`.env-self-check`（`env-self-check-repair` testid 前缀）、`.badge`（仅作 `product-badge/slot-badge` 子串）。
- **顺带发现（e2e 空断言）**：`ui-acceptance.spec.ts:94` 的 `metrics.titleItems.every(w>=80)` 取自 `.title-bar__item` —— 该选择器在 src 与 CSS 中**均不存在**，数组恒空、`every()` 恒真。删 CSS 不会让它变红，但它当前是空转断言。

## 2. 13 处字面色值 → token 映射

| # | 位置（选择器） | 现值 | 建议 |
|---|---|---|---|
| 1 | accounts.css:557 `.account-card--clickable:hover` | `0 4px 14px rgba(20,32,42,.1)` | **新 `--shadow-lift`**（与 #4 归一为一档） |
| 2 | accounts.css:643 `.seg-control__item--active` | `0 1px 3px rgba(26,34,51,.1)` | **新 `--shadow-chip`** |
| 3 | accounts.css:655 `.seg-control__count` | `#fff` | 既有 `--text-on-accent`（同值 #ffffff） |
| 4 | accounts.css:727 `.account-list__row--interactive:hover` | `0 4px 14px rgba(26,34,51,.08)` | **新 `--shadow-lift`**（以此值为基准） |
| 5 | environment.css:469 `.switch-veil` | `rgba(26,34,51,.32)` | **新 `--scrim`**（弹层遮罩语义） |
| 6 | environment.css:629 `@keyframes switch-step-pulse` 0%/100% | `rgba(79,70,229,.25)` | **新 `--accent-ring`** |
| 7 | environment.css:630 同上 50% | `rgba(79,70,229,0)` | 改 `transparent`（等价，零新 token） |
| 8 | history.css:672 `.account-avatar` | `#fff` | 既有 `--text-on-accent` |
| 9 | history.css:679 `.account-avatar--c0` | `var(--accent, #4f8cff)` | 删 fallback → `var(--accent)`；**旧蓝 #4f8cff ≠ 现行 #4f46e5，色差须截图确认** |
| 10 | history.css:683 `.account-avatar--c1` | `#34a853` | **新 `--account-tone-1`**（G21 账号色板） |
| 11 | history.css:687 `.account-avatar--c2` | `#f9ab00` | **新 `--account-tone-2`**（c0 走 `--accent`，不新增） |
| 12 | history.css:735 `.leg-step__dot .account-avatar` | `border: 2px solid #fff` | 既有 `--text-on-accent` |
| 13 | workbench.css:764 `.btn` | `0 1px 0 rgba(20,32,42,.05)` | **新 `--shadow-btn`**（或并入 `--shadow-panel` 近似归一） |

新增 **7 个 token**（`--shadow-lift/--shadow-chip/--shadow-btn/--scrim/--accent-ring/--account-tone-1/--account-tone-2`），复用 `--text-on-accent` 3 处，约 14 行写入 tokens.css。
**盲区**：`src/styles.css`:30-44 的 `.product-badge--trae/--workbuddy/--neutral` 有 9 处字面色值，`--color-accent`(#3b82f6) 与 `--color-text-secondary`(#94a3b8) **全仓无定义** → 产品徽标实际是 Bootstrap 蓝/石板灰，不是 `--accent` 靛蓝。修法：新增 `--product-trae/--product-wb/--product-neutral` + 该家族移入 L3。**有可见视觉变化，单独一步 + 截图对照。**

## 3. 过渡接缝现状与删除步骤

- `src/app/transition/sectionSlots.tsx`（152 行）：六区块**全部**「trae + workbuddy 槽位 → 同一组件、仅 productId 不同」——overview 54-59、checkin 64-69、accounts 75-82、environment 88-96、library 102-109、settings 114-122；`SECTION_SLOTS` 六条同构，`resolveSection`(150-152) 是唯一产品选择点。
- `src/components/SettingsPanel.tsx`（33 行）：**只剩分发**（`resolveSection(SECTION_SLOTS.settings, productId)` + 渲染），零分支、零自身逻辑。
- 引用面：`App.tsx`（1 import + 5 次 resolveSection + 1 次 `<SettingsPanel>`）、`SettingsPanel.tsx`（1 import）、`tests/SettingsPanel.test.tsx`（28 次渲染）、`tests/WorkBuddyPages.test.tsx`（3 次，含显式 `productId`）。`docs/` 无代码引用。

删除步骤（每步可单独 typecheck）：

1. **S1 App 内联六槽位**（~20 行）：5 处 `resolveSection(...)` 改为直接渲染页面组件并显式传 `productId={productId}`，**每处补 `key={productId}`**（保重挂载契约，见 §4.3）。sectionSlots 仍被 SettingsPanel 引用 → 此步保留。
2. **S2 SettingsPanel 去接缝**（~10 行）：改直渲染 `<SettingsPage active={active} productId={productId} />`，**保留组件名与 `productId = "trae_cn"` 默认值**（31 处测试渲染零改动）。此时 sectionSlots 已无引用方。
3. **S3 删文件 + 撤例外**（~15 行）：删 `src/app/transition/sectionSlots.tsx`（空目录一并删）→ `docs/architecture/exceptions.md` 移除 E-01 → 清 `App.tsx`/`SettingsPanel.tsx` 的 `// TRANSITION` 与接缝注释。
4. **S4（可选，独立票）删 SettingsPanel.tsx**：App 直渲染 `SettingsPage`；两套测试 31 处改写（~35 行机械替换）。可与 S1-S3 解耦延后。

## 4. 风险

1. **删死类 × e2e 视觉断言：低**。`visual-contract.mjs` 断言 `.title-bar/.navigation-rail/.app-shell::before/.lib-panel/.viewer-panel/.slot-badge/.account-list__row/.account-card--clickable/.seg-control/.account-detail__section--danger/.checkin-flow__row/.leg-step`；`ui-acceptance` 断言 `.app-main/.lib-panel/.viewer-panel/.skip-link` 的 overflow/outline；`layout-diagnostics` 断言 `.master-detail/.lib-panel/.viewer-panel` 矩形——**全部是活类，与死类清单零交集**。真缺口是「无计算样式断言的页面视觉」，删死类不会命中。
2. **四层重排 × cascade 翻转：高**。3 处真值冲突（`.workbench-read`/`.settings-panel`/`__section-heading`）一旦随分层换序，`min-height` 108/120/0 三值胜负反转 → 设置页高度与内边距漂移，**无自动化断言**。必须先做 P5-3a 去重，再动 import 顺序。
3. **删接缝 × 挂载形状：中高**。`TraeAccountsSlot` 与 `WorkBuddyAccountsSlot` 是**两个不同类型**，这正是「切产品即重挂载」的实现方式（sectionSlots 注释明写）。内联为同一类型后切产品不再重挂载 → 页面局部态残留：`AccountsPage` 的 `filter/sort/selectedRowId`、`LibraryPage` 的 `activeTab/selection`、`EnvironmentPage` 的 `busy/stage`。数据面靠 `useEffect [productId]` 与 hook 键仍会重拉，但 UI 态不复位。**修法：`key={productId}`**，语义等价旧重挂载；App.test 切产品 2 例与 `e2e/workbuddy-accounts.spec.ts` 断言不变。
4. **P4d 前车之鉴：中**。P4d-8b-1 曾因「首读骨架与实页是两棵不同子树」导致 App.test 3 例红（region 节点失效 + 焦点掉 body）。S1 **只改 App 的槽位调用点、不碰页面内部渲染分支**，即把该风险关在门外。
5. **基线归零口径：低但易虚**。guard 按「文件+规则」计数，`src/styles.css` 不在扫描域。只修 13 处即得 `entries: []`，但盲区 9 处仍在 → 「归零」名不副实；归零须同时含 §2 盲区。

## 5. 步数与顺序（每步 ≤300 行，可单独验证）

| # | 步 | 改动 | 验证 |
|---|---|---|---|
| P5-1 | tokens.css 补 7 token + 替换 13 处字面色值 | ~35 行 | `pnpm guard`（13→0）+ typecheck + hover/遮罩/头像色板截图 |
| P5-1b | 补盲区：3 个 `--product-*` token + `.product-badge` 移入 L3 | ~55 行 | guard 扫描域覆盖后仍 0；徽标色由 #3b82f6 回到 `--accent` |
| P5-2a | 删 workbench 死规则簇①（`workbench__*` 家族，含 2 段 `@media`） | ~300 行 | guard + `pnpm test`（App/Checkin/Accounts）+ 六页截图 |
| P5-2b | 删簇②（`workbench-read__*`、`history-switcher` 15 类、`settings-panel__fold-content`、`badge--off`）+ 22 条混合规则改选择器 | ~290 行 | 同上 |
| P5-2c | 删 accounts 304 + environment 41 + overview 54 + titlebar 4 + history 18 + master-detail 7 | ~280 行 | 同上 + `e2e/visual-contract.mjs` |
| P5-3a | 去重 3 处真值冲突 + 2 处形状/色拆分（`.settings-panel` 两段合一） | ~60 行 | 无断言，逐页截图人眼对照 |
| P5-3b | 拆 accounts.css → L3 共享 745 / L4 页面 341；environment 263/419 同法 | ~300 行（移动为主） | `pnpm build` + 账号页/详情页/环境页截图 |
| P5-3c | 12 份归入四层目录 + `styles.css` `@import` 改为 L1→L2→L3→L4 | ~60 行 | `pnpm build` + `e2e/ui-acceptance` + `layout-diagnostics` |
| P5-4a | E-01 S1：App 内联六槽位 + `key={productId}` | ~20 行 | typecheck + `pnpm test`（App.test 切产品 2 例）+ `e2e/workbuddy-accounts` |
| P5-4b | E-01 S2：SettingsPanel 直渲染 SettingsPage | ~10 行 | typecheck + `tests/SettingsPanel.test.tsx` 28 例 |
| P5-4c | E-01 S3：删 sectionSlots.tsx + 撤 exceptions E-01 + 清注释 | ~15 行 | typecheck + `pnpm guard`（豁免 14→0 后仍绿） |
| P5-5 | 收缩基线 `pnpm guard -- --update-baseline` | 1 文件 | `entries: []` / `totalViolations: 0` |
| P5-6 | （可选）S4 删 SettingsPanel + 31 处测试改写 | ~35 行 | typecheck + 两套 settings 用例 |

顺序理由：先 token（零视觉风险、guard 立即归零、可单独回滚）→ 再删死类（缩小后续分层改动面）→ 再去重后分层（顺序敏感，去重必须先行）→ 最后删接缝（与 CSS 正交，独立回滚）。P5-1b 与 P5-3a 是全表仅有的两处「可能看见视觉变化」的步，须留截图证据。
