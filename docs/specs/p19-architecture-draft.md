# P19 架构草案：产品无关 UI 骨架抽取 + 多平台移植架构

> 架构师（高见远）· 2026-10。基于**实际读码核实**（非转述）撰写。前置输入：P17 调研 spec、P18 双产品后端分支、用户已拍板的八域对齐方案。
>
> **读码证据基线**（本文件所有方案均以此为事实锚点）：

| 文件 | 行数 | 关键结构（已核实） |
|------|------|------|
| `src/components/AccountCenter.tsx` | **1759** | 组件主体 L121-1289（约 30 个 useState：批量进度/登录/手机补录/健康检测/过滤排序视图）；`SharedAccountEntry` L1332（**账号行骨架已存在**，槽位 props 契约）；`AdapterAccountCard` L1436；`AccountCard` L1489（TRAE 双徽章+额度块）；纯函数区 L1616-1759（`credentialRequiresRelogin`/`effectiveCredentialState`/`buildHealthResult`/`tokenRemainingLabel`） |
| `src/components/CheckinPage.tsx` | **717** | `CheckinPage`→`WorkCheckinPage` L76；`CheckinFlowRow` L526（单列演进式：勾选+徽章+phase+行内单签）；`CheckinRow` L599（fixture 模式行）；`useCountdownSeconds` L635 / `InterWaitCountdown` L652；`checkinOutcomeLabel` L690（业务码透出） |
| `src/components/WorkBuddyPanel.tsx` | **805**（P18-B3 后从 601 增长） | `WorkBuddyPanel` L134（账号页：健康卡+账号池+过户/切号/续期/深度健康）；`WorkBuddyCheckinPage` L563（签到页：单账号卡+自动签到设置）。池清单行 L445-493 **手写 `account-list__row`，未复用 `SharedAccountEntry`**；无过滤/排序/双视图/详情页 |
| `src/components/EnvironmentPage.tsx` | **1764** | `EnvironmentPage` L65；`SecondaryEnvCard` L963；四个对话框（Name L1191 / Login L1270 / Delete L1437 / Incorporate L1611） |
| `src/components/SettingsPanel.tsx` | 547 | 备份保留策略 + 自动签到设置（应用级，产品无关） |
| `src/components/LibrarySessionsPanel.tsx` | **2536** | TRAE schema 深度耦合（chat_session/chat_message SQL 直写） |
| `src/components/StatusBadges.tsx` | 495 | `CheckinSlotBadge`/`LoginArchiveSlotBadge`/`WorkBuddyLoginSlotBadge`/`WorkBuddyPoolSlotBadge` + `deriveCheckinSlotState`/`deriveLoginSlotState`/`deriveWorkBuddyPoolLoginState` —— **G10 徽章体系已双产品对齐** |
| `src/platform/accountAdapter.ts` | 197 | `AccountAdapter` 接口（productId/productName/capabilities/loadSnapshot/login/cancelLogin/selectToolAccount/runAction）+ `AccountView`/`AccountSnapshot`/`AccountActionResult` |
| `src/platform/productRegistry.ts` | 116 | `SectionKey` 六区块；`WORKBUDDY_SECTION_STATES`（overview/environment/master-library=pending）；`loadProductEntries`/`setCurrentProduct` |
| `src/App.tsx` | 366 | 产品分支渲染：`isWorkBuddy ? <WorkBuddyPanel/> : <AccountWorkbench/>`（L296-311）；pending 占位 `ProductPendingPage` L38 |
| `src-tauri/crates/infrastructure/src/product_descriptor.rs` | 180 | `ProductDescriptor{product_id, display_name, client: ClientIntegration, user_id_shape, database_relative_path}`；`ALL_PRODUCTS` 注册表；`descriptor_by_product_id` fail-closed |
| `src-tauri/src/oauth_login.rs` | 978 | `LoginProduct{WorkCn, TraeCn}`；共用编排 `begin_product_login` L292 / `complete_product_login_inner` L388 / `cancel_login_inner` L513；浏览器拉起 `open_login_url_in_browser_for_attempt` L208（隔离/本机双模式）；**`complete_trae_cn_login` 已 deprecated**（L470，捕获路线覆盖） |
| `src-tauri/crates/infrastructure/src/master_archive.rs` | **1100** | `apply_master_archive_changes` L236（hidden_status 借用 `voice_discussion`）；`delete_master_sessions_with_backup_dir` L382（备份前置→内容三表→chat_message→chat_session→空壳项目，单事务）；`open_with_key_readwrite` L150（SQLCipher PRAGMA key + busy_timeout 10s）；`verify_key_readable` L179；`authorized_session_ids` L194（owner 重校验） |
| `master_stats.rs` / `master_checkup.rs` / `master_handover.rs` | — | `read_master_stats`/`read_total_scale`；`read_master_checkup`/`stale_rows`/`empty_project_total`/`delete_master_empty_projects`；`backup_master_trio*`/`master_database_path` |
| `src-tauri/src/library_archive.rs` | **8359** | 命令壳层：`list_snapshot`/`backup_now`/`archive_apply`/`merge_projects`/`delete_sessions`/`migrate_backups`（TRAE 主库六能力） |
| `workbuddy_transfer.rs` | 1500 | `db_path`/`projects_path`/`escape_cwd_dir` L733；`observe_client_real`/`close_client_real`/`start_client_real`；`TransferPlan`/`execute_transfer`（B1 过户，19 单测） |
| `workbuddy_checkin.rs` | 718 | `checkin_status`/`checkin_claim`；`WorkBuddyCheckinStatus{today_checked_in, streak_days, daily_credit, total_credits, checkin_dates}`；`renew_pool_credentials`（刷新协议 `POST {domain}/v2/plugin/auth/token/refresh`，refreshToken 一次性轮换） |
| `workbuddy_login_credential.rs` | 734 | `WorkBuddyLoginStore`（明文容器池：save/load/list/migrate_legacy_to_plain） |
| `AGENTS.md` | 79 | **无概念映射章节**（待新增，见 §2） |

---

## §1 产品无关 UI 骨架抽取方案

### 1.1 核心判断

代码里**骨架已经存在一半**：`SharedAccountEntry`（账号行，槽位契约完整）、`StatusBadges`（G10 徽章体系）、`EmptyState`/`ConfirmDialog`/`ProductBadge` 均已产品无关。**真正缺的是三样**：

1. **页骨架层**：`AccountCenter` 的"页头 + 过滤/排序/视图切换 + 列表/详情双视图切换 + 弹层编排"这一整层编排逻辑（L121-1289 约 1100 行）与 TRAE 数据耦合，WB 无法消费；
2. **产品数据契约的 UI 化**：`AccountAdapter` 只覆盖"快照+登录+动作"，没有覆盖过滤口径、详情页字段、批量能力声明；
3. **WB 消费面**：`WorkBuddyPanel` 只对了徽章和 CSS 类名，行结构和页编排是手写复制品（L445-493）。

因此抽取策略是**"抽编排层，不抽数据"**：骨架组件只认 `SharedAccountRecord` + 能力开关 + 插槽，产品 DTO 一律在产品适配层映射后才进骨架。

### 1.2 组件清单与 props 契约

#### A. 已产品无关、只需上移归位（零改动或纯移动）

| 组件 | 现位置 | 动作 |
|------|--------|------|
| `SharedAccountEntry` | AccountCenter.tsx L1332 | **纯移动** → `shared/account/AccountRow.tsx`（连带 `SharedAccountRecord` 接口、`avatarLetter`） |
| `CheckinSlotBadge`/`LoginArchiveSlotBadge` + `derive*` | StatusBadges.tsx | 保留原位（已是共享层语义），可上移到 `shared/badges/` |
| `EmptyState`/`ConfirmDialog`/`ProductBadge`/`MasterSwitchDialog` | components/ 根 | `EmptyState`/`ConfirmDialog`/`ProductBadge` → `shared/`；`MasterSwitchDialog` 是 TRAE 主库切号专属 → 留产品层 |
| `useCountdownSeconds`/`InterWaitCountdown` | CheckinPage.tsx L635/L652 | 纯移动 → `shared/checkin/` |

#### B. 新抽取：页头控件组 `shared/account/AccountListControls.tsx`

从 AccountCenter L1108-1173（过滤 seg-control + 排序 select + 视图切换 seg-control）抽出：

```ts
interface AccountListControlsProps {
  view: AccountView;                 // "list" | "card"
  onViewChange(next: AccountView): void;
  sort: AccountSort;                 // "added" | "name" | "checkin"
  sortOptions: readonly AccountSort[];      // 能力开关：WB 不出 "checkin"
  onSortChange(next: AccountSort): void;
  filter?: {                         // 有 attention 过滤能力才渲染（realCheckinMode 才有的逻辑产品化）
    value: AccountFilter;            // "all" | "attention"
    attentionCount: number;
    onChange(next: AccountFilter): void;
  };
}
```

视图/排序的 localStorage 偏好读写（`readStoredPreference` L109 + `VIEW_STORAGE_KEY`）一并上移为 `shared/account/preferences.ts`（key 加产品前缀：`accounts.<product_id>.view`，避免 WB 与 TRAE 互相污染——现实现是全局 key，**这是一个既有缺陷，随上移修复**）。

#### C. 新抽取：页骨架 `shared/account/AccountPageSkeleton.tsx`

把 AccountCenter 的页面编排（页头动作区 + 进度状态行 + 我的账号区块 + 列表/详情切换）抽成骨架，**通过插槽注入产品差异**：

```ts
interface AccountPageSkeletonProps {
  product: ProductUiContext;              // §1.3，展示名/产品徽章/能力开关
  loading: boolean;
  entries: readonly SharedAccountRecord[];// 已在产品层映射好
  sortView: AccountListControlsProps;     // B 组件的受控数据
  headerActions?: ReactNode;              // 产品特有按钮插槽（TRAE:健康检测/刷新额度/刷新凭据；WB:续期/深度健康/过户）
  batchStatus?: ReactNode;                // 批量进度状态行插槽（P11-3 文案逻辑通用化）
  renderRow(entry: SharedAccountRecord): ReactNode;  // 行插槽（产品层决定徽章组合与 meta）
  detailView?: ReactNode;                 // 详情视图插槽（选中态由骨架管理 selectedProfileId）
  dialogs?: ReactNode;                    // 产品弹层插槽（手机补录/切号/二次确认）
  emptyState: { description: string; action?: ReactNode };
}
```

骨架内部保留：`active` 门控加载、代次收口（`snapshotReadGeneration` 模式 L219）、产品切换清态（L257-267）、`data-page-title` 焦点管理（L269-272）。这些是三处重复的实现细节，收进骨架一次做对。

**不抽进骨架的**：TRAE 的 `needsAttention` 判定口径（L439-449）、`credentialNeedsAttention`、手机号查重三段式——这些留在 `product/trae/`，通过 `filter` 回调与插槽进入。

#### D. 新抽取：签到页骨架 `shared/checkin/CheckinBatchPageSkeleton.tsx`

WB 签到页要从"单账号卡"（WorkBuddyCheckinPage L563，只签到当前登录账号）升级为用户拍板的"**逐账号行 + 一键批量 + 参与集默认全选**"，与 `CheckinFlowRow`（CheckinPage L526）同构。抽取：

```ts
interface CheckinBatchSkeletonProps {
  product: ProductUiContext;
  pool: readonly { id: string; displayName: string; meta?: ReactNode }[];
  selection: readonly string[];             // 参与集
  onSelectionChange(ids: readonly string[]): void;
  actions: { runAll: boolean; runPending: boolean; runSelected: boolean; cancel: boolean };
  onRun(ids: readonly string[]): Promise<void>;          // handleRunIds 通用化
  progress?: { completed: number; total: number };        // 总进度条（L437-449 通用化）
  renderRow(item, phase, result): ReactNode;              // CheckinFlowRow 槽位化
  autoCheckin?: ReactNode;                                // 自动签到设置区插槽
  emptyState: ...;
}
```

`checkinOutcomeLabel`（L690）是 TRAE 业务码透出表——**抽成 per-product 映射**：`shared/checkin/OutcomeLabels.ts` 导出 `outcomeLabelFor(productId, outcome, detailCode)`，TRAE 表原样迁入，WB 表按 `WorkBuddyCheckinError::code()` 词表新建。

#### E. WB 参与自动签到默认空问题的落点

`WorkBuddyCheckinPage` 的参与清单 checkbox 直接绑定 `account.enabled`（L787），后端 `setWorkBuddyAccountAutoCheckin` 默认值取决于存储初值。骨架迁移时改为：**池加载后对"从未显式设置过的账号"默认视为参与**（UI 默认全选 + 首次批量操作时把未设置项批量置 true），后端 `auto_checkin_store` 增加显式 opt-out 语义。此项归批次 2。

### 1.3 产品数据从哪来：descriptor 扩展到 UI 层

后端 `ProductDescriptor`（product_descriptor.rs）目前只有集成事实（进程/路径/身份形态）。UI 层差异声明放**前端静态注册表**（理由：UI 能力是纯前端事实，走 IPC 反而让后端被迫感知按钮文案；后端 descriptor 只在需要 fail-closed 校验的地方继续用）：

```ts
// src/platform/productUiRegistry.ts（新增）
export interface ProductUiContext {
  productId: string;                // 协议值，与后端 descriptor.product_id 同源
  displayName: string;
  capabilities: {
    addAccount: "oauth-browser" | "capture" | "none";       // TRAE=browser / WB=capture（探查票后升 oauth）
    checkin: "batch-rows" | "single" | "none";               // TRAE=batch / WB 现状=single → 升 batch
    creditsColumn: boolean;                                   // TRAE=true / WB=false（一期）
    health: "probe+network" | "deep" | "none";
    attentionFilter: boolean;                                 // G14 过滤
    detailView: "full" | "pool-only" | "none";
    sortCheckin: boolean;                                     // 排序含"签到状态"
  };
  sectionStates: Record<SectionKey, CapabilityState>;         // 现 productRegistry 的 sectionStatesFor 并入
}
export const PRODUCT_UI_REGISTRY: Record<string, ProductUiContext> = { trae_cn: ..., workbuddy: ... };
export function productUi(productId: string): ProductUiContext;  // 未注册 fail-closed 回退 minimal
```

现状 `sectionStatesFor`（productRegistry.ts L63-79）的 switch 就是它的雏形——**并表而不是并列**，避免第三处产品分支。

账号数据流维持既有双通道（已验证可行）：
- **主路径**（TRAE 真实模式）：`invoke("get_checkin_overview")` 等命令 → 产品层映射为 `SharedAccountRecord`；
- **适配器路径**（WB / 测试注入）：`AccountAdapter.loadSnapshot()` → `AccountView` → 映射。WB 侧现有 `workbuddyAdapter.ts` 的函数式 API（`listWorkBuddyAccounts` 等）包装成 `AccountAdapter` 实例即可进骨架。

### 1.4 WorkBuddyPanel 迁移（第一个消费者）

分三步，每步可独立合入：

1. **行结构对齐**（最小）：删除 WorkBuddyPanel L445-493 的手写行，改调 `SharedAccountEntry`（props：`account={id, displayName: screen_name, lastVerifiedAt, isSelected: pool 当前账号}`，`statusContent=<WorkBuddyPoolSlotBadge/>`，`onAction=切号`）。同时自动获得 card 视图能力。
2. **页骨架消费**：`WorkBuddyPanel` 重写为 `AccountPageSkeleton` + WB 插槽（健康卡作为 `headerActions` 下方独立卡保留）。WB 的 `refresh`/`deepHealth`/`renew`/`switch`/`transfer` 状态机原样保留在产品层。
3. **能力升级随批次推进**：`detailView: "pool-only"`（WB 账号详情页，含凭据有效期/设备信息）→ 后续批次开 `attentionFilter`。

### 1.5 目标文件结构与迁移顺序

```
src/
  components/
    shared/                        # 产品无关骨架（新）
      account/
        AccountRow.tsx             # ← SharedAccountEntry 纯移动
        AccountListControls.tsx    # 过滤/排序/视图切换
        AccountPageSkeleton.tsx    # 页编排骨架
        preferences.ts             # localStorage 偏好（按产品分 key）
      checkin/
        CheckinBatchPageSkeleton.tsx
        CheckinFlowRow.tsx         # ← L526 槽位化
        InterWaitCountdown.tsx     # ← L635/L652 纯移动
        OutcomeLabels.ts           # 业务码透出表 per-product
      EmptyState.tsx / ConfirmDialog.tsx / ProductBadge.tsx   # 纯移动
    product/
      trae/
        AccountCenterTrae.tsx      # ← AccountCenter 主体改写为骨架消费者
        AccountCardTrae.tsx        # ← L1489（双徽章+额度块插槽实现）
        AccountDetailTrae.tsx      # 现 AccountDetail.tsx 纯移动
        MasterSwitchDialog.tsx     # TRAE 主库切号专属
      workbuddy/
        AccountPageWb.tsx          # ← WorkBuddyPanel 账号页改写
        CheckinPageWb.tsx          # ← WorkBuddyCheckinPage 改 batch-rows
        WbHealthCard.tsx           # 健康卡（InfoRow 结构保留）
  platform/
    accountAdapter.ts              # 保留，WB 适配器实现补齐
    workbuddyAdapter.ts            # 保留
    productUiRegistry.ts           # 新：UI descriptor
```

**迁移顺序（避免大爆炸，五步各自可独立验收）**：

| 步 | 内容 | 行为变化 | 风险 |
|----|------|----------|------|
| S1 | `SharedAccountEntry`/`EmptyState`/`ConfirmDialog`/`ProductBadge`/`InterWaitCountdown` 纯移动 + 引用改路径 | 零 | 极低（编译器兜底） |
| S2 | 抽 `AccountListControls` + preferences（分产品 key）| localStorage key 变化（偏好重置一次，可接受） | 低 |
| S3 | 抽 `AccountPageSkeleton`；`AccountCenter` 改写为 `product/trae/` 消费者；全量 UI 走查（testId 全保留） | 零（契约按现有 testId 冻结） | 中（本步是主战场，走查清单见下） |
| S4 | `CheckinBatchPageSkeleton` + TRAE CheckinPage 消费 | 零 | 中 |
| S5 | WB 消费（§1.4 三步 + 签到 batch 升级 + 参与集默认全选） | WB 有感升级 | 中低 |

S3 走查锚点（骨架迁移的行为冻结清单）：`data-testid` 全量不变（`account-card-*`/`account-filter-*`/`account-view-*` 等）、G10 徽章口径、G15 刷新语义（本地刷新≠健康检测）、P7-4 取消登录、批量进度文案、stagger 动画容器类名（App.tsx L31 白名单依赖 `account-list`/`account-card-grid`/`checkin-flow` 三个类名——**骨架必须继续输出这些类名**）。

---

## §2 跨产品概念映射表（GLOSSARY）

### 2.1 背景与表结构

P17 的教训：术语未跨产品锚定导致四次"同概念误判为不同"（对话云端/环境/额度/添加账号）。解决方案是在 `CONTEXT.md`（或独立 `docs/GLOSSARY.md`，建议后者并由 CONTEXT.md 链接）新增「跨产品概念映射」章节，表结构五列：

```markdown
| 概念（锚定名） | TRAE 实现（文件:函数） | WorkBuddy 实现（文件:函数） | 扩展判定问题清单 | 逆向验证命令 |
```

- **概念（锚定名）**：统一术语，全团队/全文档/全 UI 只用这一个词（允许别名行）；
- **TRAE/WB 实现**：精确到 文件:函数 或 文件:DTO 字段，**不允许只写文件名**；
- **扩展判定问题清单**：接入新平台时必须逐条回答的问题（布尔问题为主，答案写回本表新列）；
- **逆向验证命令**：可在真机一键复核的命令（探针脚本/PowerShell/sqlite 查询），保证映射不过期。

首版表（基于已核实事实）：

| 概念（锚定名） | TRAE 实现 | WorkBuddy 实现 | 扩展判定问题 | 逆向验证命令 |
|---|---|---|---|---|
| **对话云端**（会话是否在云） | 主库 database.db 全本地；云端=官方服务端（工具不触碰） | sessions 表 `transport='local'/'cloud'` 双态；过户只改本地索引不碰云端（workbuddy_transfer.rs） | ① 库里有无 transport 类列？② 有无"仅云端"的会话形态？③ 工具写操作是否可能触碰云端？ | `sqlite3 ~/.workbuddy/workbuddy.db "select transport,count(*) from sessions group by transport"` |
| **添加账号**（新增账号的路径） | OAuth 浏览器链 `begin_checkin_login`/`complete_checkin_login`（oauth_login.rs）+ 捕获 `account_capture.rs` 双轨 | 捕获单轨 `workbuddy_capture.rs`（读 .info 快照）；OAuth 待 P19 探查 | ① 平台是否有网页授权码流？② 回调形态（本地 socket/轮询）？③ client_id/authorize URL 可否逆向？ | asar 反解 + daemon 日志：`grep -r oauthClientId <asar 解包目录>` |
| **环境**（多实例/多数据根） | storage_roots.rs + environment_registry.rs（主/副库） | 单实例（daemon 常驻）；多实例=实验票 | ① 客户端支持 --user-data-dir 多开吗？② 数据根如何随实例漂移？ | 探针 `GET http://127.0.0.1:18488/workbuddy/probe` |
| **额度**（积分/用量） | `CheckinOverviewEntryDto.usage_remaining_credits`（积分包剩余总和，静态池 200 不展示） | `WorkBuddyCheckinStatus.total_credits/daily_credit`（workbuddy_checkin.rs） | ① 额度是"余额"还是"今日可得"？② 刷新端点是否只读幂等？ | 刷新协议：`POST /v2/plugin/auth/token/refresh`（daemon 日志已实证） |
| **主库**（工具管理的对话库） | SQLCipher database.db（master_archive.rs，PRAGMA key） | workbuddy.db 明文 SQLite WAL（daemon 持有）+ projects/*.jsonl 正文 | ① 库是否被客户端进程长持？② 加密与否？③ 删除语义（软删/硬删）？ | `python scripts/diag-*` / 查 WAL 文件在写 |
| **签到** | checkin.rs（SOLO 通道虚拟设备 + DPAPI 包，status→claim→status） | workbuddy_checkin.rs（客户端会话 token 直连 status/claim） | ① 凭据形态（设备绑定 or token 直连）？② 风控参数（错峰/名额）？ | 真机只读 status 调用 |
| **凭据存储** | CheckinCredentialStore（加密包）+ fused_login_credential | WorkBuddyLoginStore 明文容器池（workbuddy_login_credential.rs）+ .info `$wbEncrypted` 信封 | ① 工具池是明文还是加密？② 与客户端副本的轮换冲突策略？ | 读容器池目录结构 |
| **切号** | 主库五步事务（MasterSwitchDialog）| wbipc RPC logout→saveOauthToken（workbuddy_commands.rs switch） | ① 切号是否需要重启客户端？② 历史可见性是否需过户？ | — |

### 2.2 扩展新平台的强制流程（写入 AGENTS.md）

在 AGENTS.md「项目规则」清单后追加一节（可执行化，每步有产出物与门禁）：

```markdown
## 新产品接入强制流程（P19 起生效）

接入任何新产品（codebuddy / trae International / …）前，按序执行五步，**跳步禁止**：

1. **过表**：在 docs/GLOSSARY.md 概念映射表为新平台新增一列，逐行回答
   「扩展判定问题清单」；答不上来的行标记 `⛔未逆向`，不得进入设计。
2. **逆向**：对 `⛔未逆向` 行执行逆向探查（asar 反解/探针/daemon 日志），
   验证命令写入表中并在真机跑通，产出附到 docs/specs/pXX-*-research.md。
3. **映射**：每个锚定概念给出新平台的「文件:函数」实现；与 TRAE/WB 语义
   不同构的概念必须显式标注 `!=` 并停在该域单独设计，禁止硬套骨架。
4. **骨架复用**：前端只允许消费 src/components/shared/ 骨架 + 新增
   product/<platform>/ 插槽；发现骨架能力缺口时改骨架（S1-S5 同款迁移
   纪律），**禁止 fork 骨架复制页**。后端在 product_descriptor.rs 追加
   descriptor（注册表 fail-closed 自动生效）。
5. **登记**：更新 productRegistry/productUiRegistry、ADR 决策号、
   tickets 计划；AGENTS.md「项目定位」段的产品清单同步。

审查门禁：任何 PR 触碰 product/<platform>/ 之外的产品分支逻辑（新增
`isXxx` 布尔、productId switch 分支）即打回，必须改为 descriptor/注册表声明。
```

---

## §3 OAuth 登录编排复用（CodeBuddy 参数化）

### 3.1 Trae 现有编排的可复用面（已核实 oauth_login.rs）

编排核心已经是**产品参数化的半成品**：`begin_product_login` / `complete_product_login_inner` / `cancel_login_inner` 按 `LoginProduct` 枚举分发，单槽 attempt（`ActiveLoginAttempt`：attempt_id + product + session + control + 浏览器资源）与产品无关。产品差异收在两处：

- `begin_login_for(&profile_id, LoginProduct)`（infrastructure）→ 产出 `login_url` + `LoginSession`（内含本地回调 socket）；
- `complete_login(session, ...)`（infrastructure）→ 回调等待 + token 交换 + 凭据入库 + 首登验收。

P17 调研已定性的 CodeBuddy 协议事实（asar 实读 + 真机验证，第 6/8 节）：

| 差异点 | TRAE（SOLO 通道） | CodeBuddy/WorkBuddy |
|---|---|---|
| 授权 URL | `https://www.trae.cn/authorization?` + PKCE `code_challenge` + `auth_callback_url=http://127.0.0.1:<port>`（oauth_login.rs 测试 L764-766 实证） | `openAuthUrl` 形态存在；authorize URL/client_id 在 daemon 配置/asar（`oauthClientId` 注入 `e.oauth_client_id`），**具体值未逆向——探查票产出** |
| 回调形态 | 本地 socket 回调（`LoginSession` 持有 listener，阻塞等回调，300s 超时） | `openAuthUrl` + `loopGetToken`（**轮询/回调取 token**）+ `web_handoff` 事件（desktop_to_web）——形态待探查定稿是轮询还是回调 |
| token 端点 | complete_login 内部交换（PKCE verifier） | 刷新端点已定稿：`POST https://www.workbuddy.cn/v2/plugin/auth/token/refresh`，头 `X-Domain`+`X-Refresh-Token`+`X-Auth-Refresh-Source: plugin`，空 body，响应 `data.data` 新 auth 对象，refreshToken 一次性轮换；**createSession 换 token 端点未逆向** |
| scope/domain | TRAE 自有 | `openid profile offline_access email`；`domain: www.workbuddy.cn`（Keycloak 形态） |
| 凭据落盘 | CheckinCredentialStore 加密包 + AccountRegistry | `WorkBuddyLoginStore` 明文容器池（与捕获共用）；客户端侧为 .info `$wbEncrypted` 信封 |
| 完成后指针 | TraeCn 链 `ProductAccountStateStore.commit_selection`（带代次竞态保护） | 无需（WB 无工具内选择指针，池化） |

### 3.2 参数化设计

**基础设施层**（infrastructure）：把 `LoginProduct` 扩为三变体 `WorkCn | TraeCn | WorkBuddy`，协议细节以 trait 收口而非继续加 match 分支：

```rust
/// OAuth 协议参数包：每个产品一份静态声明 + 一个回调等待实现。
pub trait OAuthLoginProtocol {
    fn build_authorize_url(&self, pkce: &PkcePair, callback: &CallbackEndpoint) -> Result<String, LoginError>;
    /// 回调等待：TRAE = socket 阻塞读；WB 一期 = 探查定稿（轮询 or socket）。
    fn await_authorization(&self, session: &LoginSession, cancel: &LoginAttemptControl,
                           timeout: Duration, should_abort: &dyn Fn() -> bool)
                           -> Result<AuthorizationCode, LoginError>;
    fn exchange_token(&self, code: AuthorizationCode) -> Result<ProductCredential, LoginError>;
    /// 首登可持续性验收（ADR-0027 同语义，各产品自有换发链）。
    fn acceptance_probe(&self, credential: &ProductCredential) -> LoginAcceptance;
}
```

`begin_login_for` / `complete_login` 改为消费 `&dyn OAuthLoginProtocol`（或泛型），**现有 TRAE 两链路行为零变化**（WorkCn/TraeCn 各自的 impl 从现有函数体平移）。

**命令层**（src-tauri）：`ActiveLoginAttempt` 加 `product: LoginProduct::WorkBuddy` 支持；新增三命令，复用 `begin_product_login`/`cancel_login_inner` 全部编排（单槽互斥、浏览器双模式、profile 清理、取消相位机）：

- `begin_workbuddy_login(use_system_browser)` → 复用浏览器拉起与 URL 展示；
- `complete_workbuddy_login(attempt_id)` → 协议 impl 的 await/exchange → 凭据入 `WorkBuddyLoginStore::save`（**入库走活路线容器池，不重蹈 `complete_trae_cn_login` deprecated 的旧池根覆辙**——deprecation 注释 L470-472 明确教训）→ 复用 `LoginAcceptance` 首登验收；
- `cancel_workbuddy_login(attempt_id)`。

**WB 凭据与客户端副本的轮换冲突**：P17 已定性「refresh 一次性轮换 = 谁刷新谁持有最新对」。OAuth 登录取得的新 token 对写池后，客户端若持旧对会失效——切号/写回必须走 `auth:saveOauthToken` wbipc RPC（既有 A4 链路），登录完成后提示或自动回写客户端。此点在 `complete_workbuddy_login` 编排里做成与切号一致的后处理。

**前置探查票（P0）**：authorize URL / client_id / createSession token 端点 / 回调形态四项未逆向，从 asar `codebuddy-lite-wb.mjs` 的 `createSession`/`openAuthUrl`/`loopGetToken` 实读 + daemon 登录一次日志抓取。探查产出直接回填 §2 GLOSSARY 表。

---

## §4 主库机制移植（master_archive → workbuddy.db）

### 4.1 可参数化复用的模块清单

| master_* 能力 | TRAE 实现 | WB 移植形态 | 复用方式 |
|---|---|---|---|
| **会话浏览** | library_archive.rs `list_snapshot`（chat_session JOIN project 按 user_id） | WB sessions 表按 user_id 隔离 + transport 列 | **模式平移不代码复用**（schema 不同），但快照 DTO 形状（分组/计数/时间）对齐，前端 `LibrarySessionsPanel` 骨架化后共用 |
| **消息读取** | chat_message 表 SQL | `projects/<escape_cwd>/<sessionId>.jsonl` 正文文件（复用 `workbuddy_transfer.rs` 的 `projects_path`/`escape_cwd_dir` L733） | jsonl 读取器新建 `workbuddy_archive.rs`；正文只读 |
| **删除** | `delete_master_sessions_with_backup_dir`：**硬删除**（内容三表→chat_message→chat_session→空壳项目），前置 `backup_master_trio` | **语义对齐拍板结果：UPDATE deleted_at 软删除**（WB `deleted_at IS NOT NULL` 即 UI 隐藏；软删行过户时同转——`transfer_sessions` 的归属改写不得带 `deleted_at IS NULL` 过滤，B1 现实现需核验此点） | 事务纪律复用（见下），SQL 重写 |
| **过户** | master_handover（切号五步事务） | B1 `execute_transfer` 已实现（19 单测） | 已就绪，只需与删除语义对齐 |
| **体检** | master_checkup.rs（stale_rows/empty_project_total/`delete_master_empty_projects`） | WB 版体检：孤儿 jsonl（正文有索引无/索引有正文无）、软删行统计、cwd 转义异常项 | 判定结构（CheckupAccountRow 模式）平移 |
| **统计** | master_stats.rs（trio 体积/total scale） | sessions 计数 + projects 目录体积（`master_trio_size_bytes` 模式 → 目录遍历） | 平移 |
| **cwd 归并** | `merge_projects`（分组合并：改挂 project_id + 空壳清理） | WB 版：同 cwd 不同转义形态的分组归并（`escape_cwd_dir` 的历史版本差异） | 归并事务骨架复用 |
| **备份前置** | `backup_master_trio_to`（滚动单备份铁律） | WB 版：workbuddy.db + 活动正文目录快照，同铁律（触发前旧备份彻底删除） | 策略复用，实现独立 |

### 4.2 事务纪律提取（真正值得抽的公共件）

master_archive.rs 最有复用价值的不是 SQL 而是事务纪律状态机（L236-358 的模式）：

```
预检(库存在/可读) → 归属重校验(authorized_session_ids: 界面 ID 可能过期，
写前必须在库层重新确认归属) → unchecked_transaction → 计数 before → 写入
→ commit(失败=CommitOutcomeUnknown，不得误报已回滚) → 提交后读回校验
(不符=CommittedNeedsAttention)
```

提取为 `infrastructure/archive_tx.rs`：

```rust
pub fn run_archive_transaction<T>(
    conn: Connection,
    verify_after_commit: impl Fn(&Connection) -> Result<bool, ArchiveError>,
    body: impl FnOnce(&rusqlite::Transaction) -> Result<T, ArchiveError>,
) -> Result<T, ArchiveError>;
```

TRAE master_archive 与新 WB archive 共同改用（TRAE 侧行为零变化）。

### 4.3 连接层差异

| | TRAE 主库 | WB workbuddy.db |
|---|---|---|
| 加密 | SQLCipher，`PRAGMA key = "x'...'"`（`open_with_key_readwrite` L150，key 错误延迟暴露需 `verify_key_readable` 真实查询探测 L179） | **明文**，直接 `Connection::open`；无 key 探测步骤 |
| 并发 | 与 TRAE 实例在线写错峰（busy_timeout 10s，绝不长锁） | **daemon 持有的活库**（WAL 模式）——busy_timeout 仍必须，且读可并发、写窗口要短 |
| 打开器抽象 | `fn open(db_path) -> Result<Connection>` 差异点收成一个 opening 函数，事务纪律/上层逻辑完全共用 | |

### 4.4 进程边界约束（WB 库是"客户端活库"）

这是 WB 主库移植与 TRAE 最大的结构差异，三条纪律：

1. **写操作前进程检查**：`observe_client_real()`（workbuddy_transfer.rs L289）——破坏性写（删除/归并/过户）沿用 B1 纪律：**先 `close_client_real()` → 执行 → `start_client_real()`**（transferDialogLines 文案 L111 已向用户承诺此语义）。只读浏览可在线（WAL 读不阻塞 daemon）。
2. **正文文件同理**：jsonl 正文虽是独立文件，但 daemon 可能持有句柄/缓存索引——删除/过户会话时正文操作同样收进关客户端窗口。
3. **命令层编排**：组合根命令（workbuddy_commands.rs 模式）负责关停/恢复与失败时的重启补偿（B1 已有 `receipt.restarted` 回执语义，复用）。

### 4.5 WB 主库页（前端）

一期轻量版（用户拍板"轻量版先行+多实例实验独立票"）：
- `pages/WbLibraryDetail.tsx`：会话列表（分组=cwd 转义目录聚合）→ 会话详情（jsonl 正文渲染，复用 LibrarySessionsPanel 的阅读器部分需骨架化，或一期用简化阅读器）→ 多选删除（软删，二次确认+自动备份前置）→ 体检卡（孤儿/软删统计）→ 统计卡（会话数/体积）→ cwd 归并入口。
- 过户入口保留在账号页（现状），主库页提供"按账号过滤"视图。
- 主/副库概念 WB 一期不引入（单实例），`productUiRegistry` 里 WB 的 `sectionStates["master-library"]` 从 pending 升 ready 时按此范围。

---

## §5 P19 批次拆解、规模与依赖

| 批次 | 内容 | 规模 | 依赖 |
|---|---|---|---|
| **B1 骨架抽取** | §1.5 S1-S4（纯移动→控件→页骨架→签到骨架），TRAE 两页改为消费者；preferences 分产品 key；走查清单冻结 testId | **M**（AccountCenter 1759 行 + CheckinPage 717 行的重排，但多为移动） | 无 |
| **B2 WB 消费迁移** | §1.5 S5 + §1.2E（参与集默认全选修复）+ WB 账号详情页（pool-only）；workbuddyAdapter 包装成 AccountAdapter | **M** | B1 |
| **B3 主库移植** | §4：archive_tx.rs 提取 → workbuddy_archive.rs（浏览/软删/体检/统计/cwd 归并，备份前置+关客户端编排）→ WB 主库页轻量版；B1 过户与软删语义对齐核验 | **L**（后端新模块 + 8359 行命令壳的 WB 对应面 + 前端新页） | 无（可与 B1/B2 并行） |
| **B4 登录编排** | 探查票（asar：authorize URL/client_id/token 端点/回调形态）→ OAuthLoginProtocol trait 提取 → begin/complete/cancel_workbuddy_login → 容器池入库 + saveOauthToken 回写 | **M**（探查票 S，参数化 M，端到端真机验收占大头） | 探查票先行；与 B3 并行 |
| **B5 签到深化** | WB 签到页升 batch-rows（并入 B2 则本批缩水）；checkin_dates 日历（WorkBuddyCheckinStatus 已有字段）；账号页额度列（WB total_credits） | **S** | B1（骨架）、B2（参与集） |
| **B6 总览/环境/备份/插件** | WB 总览工作台统计（读 workbuddy.db 只读统计，复用 B3）；环境页轻量版（单实例状态卡，多实例实验独立票）；WB 备份 UI + Trae 手动删除备份入口（SettingsPanel）；插件探查票（plugin_manifest.rs 盘点，只探不实装） | **M** | B3（总览/备份依赖库访问）；环境/插件探查独立 |

```mermaid
graph TD
    B1[B1 骨架抽取 M] --> B2[B2 WB 消费迁移 M]
    B2 --> B5[B5 签到深化 S]
    B1 --> B5
    B3[B3 主库移植 L] --> B6[B6 总览/环境/备份/插件 M]
    B4[B4 登录编排 M]（探查票先行）
    B3 -.并行.-> B1
    B4 -.并行.-> B1/B3
```

规模合计约 1.5L + 3.5M + 1S。关键路径：B1→B2→B5（UI 线）；B3、B4 为并行线。

---

## §6 需要用户拍板的决策点清单

1. **WB 删除终态**：软删除（UPDATE deleted_at，对齐 WB 语义）是否需要配套"彻底删除"入口（回收站式二次清理），还是软删即终点？铁律"滚动单备份"在 WB 侧的备份对象确认：workbuddy.db + 整个 projects 正文目录（体积可能远大于 TRAE trio）？
2. **WB 主库写操作进程策略**：破坏性写一律"关客户端→写→重启"（同过户纪律），还是对轻量写（软删）尝试在线 busy_timeout 错峰？（建议前者，与既有用户承诺文案一致）
3. **UI descriptor 归属**：前端静态 `productUiRegistry`（本方案建议）vs 后端 descriptor 扩展字段经 `list_products` 下发？
4. **B4 前置探查票授权**：asar 逆向 authorize URL/client_id/createSession 端点 + 真机 daemon 登录日志抓取（需用户配合在 WB 登录一次）。
5. **localStorage 偏好分 key**：S2 迁移会导致用户视图/排序偏好重置一次——是否接受？
6. **WB 账号详情页范围**（B2 的 pool-only 详情）：展示哪些字段（凭据有效期/设备信息/签到日历入口）？
7. **B6 环境页 WB 轻量版边界**：只读状态卡（daemon 存活/版本/数据根体积）是否够，还是要含"重启客户端"动作？
8. **过户与软删核验**：B1 `transfer_sessions` 是否已无 `deleted_at IS NULL` 过滤（软删行随过户转移）——需工程师核验单测覆盖，若缺列为 B3 首任务。

—— 完 ——（架构师 高见远；未修改任何代码）
