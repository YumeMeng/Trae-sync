# 开源模式学习：多产品/多账号统一管理 → Trae Sync 统一档案池方案

> 2026-10-18。回应决策：**"功能一样的地方应当是扩展实现保持一致，而不是账号系统 Trae 一套、WorkBuddy 一套"**——本报告从成熟开源项目提炼模式，给出统一使用逻辑的具体方案，供确认后落 ADR v2。

## 1. 对标项目与提炼的模式

| 开源项目 | 架构要点 | 对 Trae Sync 的启示 |
|----------|----------|--------------------|
| **Endcord**（Discord 多账号终端工具） | **Profile = 命名容器**（token + 元数据：name/last_used/storage_source）；**双存储后端**（keyring 推荐 / plaintext 兼容）；**统一 profile manager UI** 管理全部 profile；profile 切换即激活 | **统一档案模型**：一个 profile = 凭据 + 元数据 + 后端声明；**产品作为 profile 的属性而非两套 registry** |
| **aws-vault / gcloud config** | CLI 单入口 + **profile 切换**（`--profile xxx`）；凭证按 profile 隔离存储；激活即把该 profile 的凭据注入会话 | **统一池 + 当前指针**：Trae Sync 已有的"当前账号选择指针"模式可平移到统一池（每产品一个当前指针） |
| **credential-vault**（Claude Code skill 生态） | **统一 vault 引擎 + 项目命名空间分区**（[project-a]/[project-b]）——引擎一套、隔离靠命名空间；泄漏扫描/轮换/强度策略**全池统一** | **引擎统一、命名空间分区**：凭据读写/轮换/泄漏扫描等**横切能力只写一次**，产品是命名空间维度 |
| **Chromium / VSCode profile 模式** | 一个壳 + 多 profile **各自独立数据目录**，**profile 管理器统一**；能力（扩展/主题）按 profile 各自装配 | **容器格式产品私有，管理接口统一**——与 descriptor 架构完全一致 |
| **age-vault**（Rust） | **分层：统一 Vault 编排 + 存储模块 + 加密模块 + 账号模块**；账号是统一表，密钥加密方式按账号声明 | **trait 化的凭据读写**：`CredentialStore` trait 统一签名，`TraeCnStore`/`WorkBuddyStore` 各自实现 |

## 2. 模式合流：统一档案池方案（v2）

**核心原则：管理接口统一一套，容器格式按产品各自私有，能力按 descriptor 声明三态。**

```
统一档案池（AccountProfile，一套管理逻辑）
├── profile_id（统一格式：{product}-{hash}）
├── product: ProductId（descriptor 引用 → 差异点全部由 descriptor 声明）
├── platform_account_id（Trae 16 位 / WorkBuddy 20 位——位数由 descriptor.user_id_shape 校验）
├── display（screen_name / display_name / avatar —— 通用字段）
├── credentials: CredentialRef
│     ├── TraeCn → FusedLoginStore 容器（TSFUSED1 DPAPI）+ CheckinCredentialBundle
│     └── WorkBuddy → WBSESSN1 容器（WBSESSN1 DPAPI）
│         （两者实现同一 CredentialStore trait：save/load/validate/summary/renew）
└── capabilities（由 descriptor 声明：checkin ✓ / switch ✓ / history ✗[未实施]）

统一消费面（一套逻辑）
├── 账号列表：遍历统一池 → 按 product 分组渲染（前端一张账号页，卡片带产品徽标）
├── 签到调度：遍历 capabilities.checkin == true 的档案 → 按各自协议模块执行
├── 续期：CredentialStore trait 的 renew()——轮换/写回纪律在 trait 内统一
├── 切号：按产品各自的 RPC/编排（能力按 descriptor 声明），入口统一
└── 备份/闸门/纪律：全池统一（现有骨架不变）
```

## 3. 与现状的差异（迁移清单）

| 现状 | 目标 | 迁移动作 |
|------|------|----------|
| WorkBuddy 独立池（workbuddy-login 根，无 registry） | 统一池注册 | WorkBuddy 捕获编排落盘容器后**同时写统一池档案**（profile_id 补统一格式） |
| Trae 的 AccountRegistry（16 档案，checkin-* 格式） | 统一池迁移或双读 | **存量 16 档案一次性迁移器**（读 AccountRegistry → 写统一池，纯数据搬运）——Trae 写路径切统一池 |
| AccountRegistry 直读的消费点（切号判定/签到调度/历史身份） | 统一池 + capabilities 过滤 | 消费点改读统一池，`product == trae_cn` 过滤保持语义（回归测试兜底） |
| 前端账号页（Trae 专用） | 统一账号页（产品分组） | T5 二期的统一账号卡片 |
| AccountRegistry 本体 | 退役为 Trae 侧凭据容器（保留）| registry 的**管理职责**上收统一池；容器文件不动 |

## 4. 风险与护栏

1. **Trae 回归风险**：统一池切换影响 Trae 切号/签到判定——**护栏**：迁移器写完后 Trae 侧全量回归（921/0 + 220/0 基线）+ 切号隔离演练复跑（P15-5 模式 16/16）。
2. **跨产品凭据复制禁令**（T5 不变量）：统一池**不合并凭据本体**——凭证仍按产品各自容器存储，统一池只存**引用与元数据**（禁令不破）。
3. **20 位/16 位 ID 校验**：统一池写入时按 descriptor.user_id_shape 校验（防串池）。
4. **profile_id 兼容**：存量 Trae profile_id（checkin-*）保留原值迁移（历史身份判定依赖），新档案用统一格式——**双格式共存由统一池承接**。

## 5. 待确认（grill）

1. 统一池方案确认（本报告第 2 节）——确认后落 ADR v2 转正。
2. 迁移时机：立即迁移 16 个 Trae 存量档案，还是统一池先双读过渡（Trae 写路径暂不动）？
3. WorkBuddy 多账号存并（allAccounts 数组）是否纳入统一池（一个客户端多账号）——影响统一池的账号计数与面板。
