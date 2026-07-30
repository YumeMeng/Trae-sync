# Trae Sync V1 实现规格

版本：`0.2`

状态：基于 D-001 至 D-045 的已确认设计和方案 D UI 体验审计；尚未开始程序实现。

## 1. 目标

Trae Sync 是独立 Windows 桌面工具。它把本机 TRAE Work CN 对话收入中立历史库，并让活动数据库中选定历史跟随当前登录账号。

V1 的“同步”不是复制账号数据，也不是让两个账号各持有一份独立副本。底层只执行两类已验证操作：

```text
目标账号没有同一项目：移动活动项目当前归属
目标账号已有同一项目：把选中活动会话挂到目标项目
```

目标账号原有历史不删除。来源账号会失去转入项目或会话的当前可见归属；中立历史库继续保留来源、快照、版本和写前备份。

## 2. V1 用户承诺

1. 未获扫描授权时，不读取对话数据库。
2. 自动扫描默认关闭；任何 TRAE 数据库写入始终由用户确认。
3. 写入前必须有经过验证的恢复副本；此要求不能关闭。
4. 写后必须完成完整 SQLCipher 和 SQLite 完整性检查，才显示成功。
5. 工具永不自动删除历史、快照、备份或失败证据。
6. 当前账号、数据位置或目标数据库无法可靠确认时停止，不猜测、不允许手工覆盖。
7. TRAE 运行时不读取不稳定的 DB/WAL/SHM，也不写数据库。
8. 写入、验证和恢复均为全有或全无；不能报告半成功。

## 3. V1 范围

### 包含

- Windows x64 桌面应用。
- Tauri 2、Rust、React、TypeScript、Vite。
- TRAE Work CN / TRAE SOLO CN Adapter。
- 标准数据位置自动发现和自定义数据位置添加。
- 当前账号只读检测，不保存凭证。
- 不可变来源快照和 SQLCipher 规范化目录库。
- 账号、项目、完整对话三级历史浏览与自定义同步范围。
- 完全相同去重、严格扩展快进、分叉版本保留。
- 项目归属移动和活动会话重挂。
- 强制写前双备份、写后完整验证、崩溃协调和受控恢复。
- 操作记录、备份管理、手工恢复、手工删除和存储根迁移。
- 可选 `.traesync-recovery` 目录库密钥恢复包。

### 不包含

- TRAE IDE 或其他平台 Adapter。
- 登录、Token 管理或账号切换。
- 两个账号同时保留独立对话副本。
- 从历史快照重建活动库中已不存在的项目或会话图。
- 恢复软删除内容。
- 云同步、跨设备、远程存储、macOS。
- 后台自动写库、强制结束 TRAE、快速验证模式。
- 自动删除或按配额清理历史。

## 4. 核心术语

```text
平台                 一个 TRAE 产品 Adapter；V1 只有 Work CN
数据位置             一个明确的产品数据目录，稳定标识为 data_location_id
活动数据库           当前数据位置正在被 TRAE 使用的 database.db
历史库存储根         Trae Sync 大型数据的唯一权威根目录
来源快照             某次扫描捕获的不可变 DB/WAL/SHM 和元数据
目录库               Trae Sync 的 SQLCipher 规范化历史数据库
逻辑项目             Adapter 可靠判定为同一工作区语义的项目
逻辑会话             (product_history_namespace, original_session_id)
会话版本             一次扫描得到的规范化内容图
同步范围             全部历史或用户自定义选择的活动历史集合
同步计划             固化目标证据、动作、排除项和预期结果的不可变计划
操作 manifest        崩溃恢复的权威状态文件
```

## 5. 用户流程

### 5.1 首次扫描

```text
打开同步工作台
选择已发现的平台和数据位置
查看读取范围并授权
关闭 TRAE，或让工具请求正常关闭
点击“扫描历史”
建立不可变快照
解析并更新目录库
显示账号、项目和对话
```

不创建强制设置向导。默认数据位置有效时直接选中。

### 5.2 应用到当前账号

```text
选择 [全部历史 | 自定义]
刷新活动数据库和当前账号证据
把新历史收入历史库
生成变化预览
用户确认目标账号、数据位置和动作集合
正常关闭 TRAE
重新验证计划证据
创建并验证写前双备份
执行单事务
新连接完整验证
协调目录库和 manifest
按用户设置重新打开 TRAE
```

账号证据或数据库证据变化时废弃旧计划，返回新预览。

### 5.3 账号无法确认

界面只允许浏览已有历史和查看诊断。主要操作是“重新检测”。用户需启动 TRAE 完成登录，让 Adapter 取得新证据，再关闭 TRAE。

历史库中的手工账号选择只改变归类或筛选，不能授权写入。

### 5.4 恢复

操作与备份页显示每次操作的写前状态、执行结果和恢复状态。恢复前先保存当前目标现场，恢复发布后重新验证。证据不足时进入人工恢复，不提供未经证明的覆盖操作。

## 6. 信息架构

持久导航：

```text
历史库
操作与备份
设置
```

标题栏持续显示平台、数据位置和当前账号。平台选择是全局上下文，不作为独立页面。

历史库工作台：

```text
当前 TRAE：平台 / 数据位置 / 当前账号
历史库：账号数 / 项目数 / 对话数                 [查找新历史]
账号与项目树 | 对话列表与内容预览 | 同步计划
同步计划：已选择 / 本次可同步 / 已在当前账号 / 需要处理
                                             [检查并安全同步]
```

复选框只改变同步范围；点击对话标题只打开内容预览。确认页必须再次显示实际目标账号、可同步数量、排除项和来源可见归属变化。

当前账号在标题栏和确认区重复显示是有意的目标复核。数据库参数、加密参数、恢复策略和 manifest 不进入日常主界面。

复杂路径、哈希、schema、SQLCipher 和 manifest 只放在详情或诊断报告。

## 7. 架构边界

```text
React UI
  |
Tauri commands                 只做输入校验、调用和事件桥接
  |
Application services           编排用例、锁、进度和状态
  |
Domain                         身份、版本、范围、计划和状态机
  |
Ports
  |-- ProductAdapter
  |-- CatalogRepository
  |-- SnapshotStore
  |-- ManifestStore
  |-- KeyStore
  |-- ProcessController
  |-- FileIdentityProvider
  |
Infrastructure
  |-- Work CN Adapter
  |-- SQLCipher
  |-- Windows DPAPI/process/filesystem
  |-- Tauri event transport
```

约束：

- Domain 和 application services 不依赖 Tauri。
- 前端不直接访问文件、数据库或认证状态。
- Product Adapter 不决定是否跳过备份、验证或锁。
- SQL 不由 UI 或字符串拼接生成。
- 平台差异不泄漏为核心模型中的 Windows 路径条件。
- 历史库、账号管理和自动化策略不直接调用彼此的基础设施实现；跨模块功能只由 application services 组合公开用例。
- 未来账号切换模块只能发布账号变化结果；同步前必须重新检测目标证据并重新生成不可变计划。
- 未来自动化模块只能触发与手工流程相同的计划和执行用例，不能获得数据库直写通道或关闭备份、确认和验证。
- 原型是一次性交互验证代码，生产实现不得照搬其单组件状态结构。

## 8. 后端模块

### `domain`

保存稳定值对象和纯规则：

```text
PlatformId
DataLocationId
StorageRootId
SnapshotId
OperationId
AccountIdentity
AccountEvidence
ProjectIdentity
SessionIdentity
SessionVersion
ProjectSourceAssignment
SyncScope
SyncPlan
PlanAction
PlanExclusion
StorageDeletionPlan
OperationState
```

### `application`

提供面向用例的服务：

```text
DiscoverProducts
ScanHistory
BrowseHistory
AssignProjectSource
BuildSyncPlan
ApplySyncPlan
ReconcileOperations
RestoreOperation
ManageBackups
PlanStorageDeletion
ApplyStorageDeletion
MoveStorageRoot
ExportRecoveryPackage
ImportRecoveryPackage
```

### `adapters/work_cn`

负责：

- 路径和产品版本识别。
- 进程识别及正常关闭/启动参数。
- 当前账号证据解析。
- SQLCipher 参数、schema 探测和兼容判断。
- 原始表到规范化历史的映射。
- 项目身份比较。
- `PlanAction` 到 Work CN SQL/文件断言的映射。
- Work CN 专属写后逻辑验证。

### `catalog`

负责目录库 schema、事务、投影、搜索索引和旁路升级。目录库只是可重建产品视图，不取代不可变来源快照和操作 manifest。

### `operations`

负责同步、备份、验证、恢复、状态机和跨进程锁。所有目标副作用必须经过此模块。

### `storage`

负责存储根身份、空间预算、文件哈希、不可变发布、迁移和引用校验。

### `platform/windows`

负责 DPAPI、进程和窗口控制、卷/文件身份、原子替换、可用空间和 OS 级锁。

## 9. `ProductAdapter` 契约

接口使用结构化类型；下面是职责规格，不锁定最终 Rust 语法：

```text
descriptor()
discover_data_locations()
validate_data_location(location)
inspect_processes(location)
request_normal_close(location)
launch_product(location)

read_account_evidence(location)
inspect_database(location)
scan_database(read_connection, mapping_version)
compare_project_identity(source, target)

compile_actions(plan_actions, inspected_state)
apply_actions(write_connection, compiled_actions)
assert_expected_state(read_connection, expected_state)
```

Adapter 返回：

- 产品、schema 和映射版本。
- 可验证的数据位置身份。
- 非敏感账号身份及不可逆证据指纹。
- 规范化项目、会话、内容图和原始稳定引用。
- 项目身份判定及证据等级。
- 写动作涉及的表、预期行数和逻辑断言。

Adapter 不返回或持久化 Token、refresh token、cookies 或完整认证正文。

## 10. Work CN Adapter 基线

V1 基线产品：TRAE Work CN / TRAE SOLO CN `1.107.1`。

默认活动数据库：

```text
%APPDATA%\TRAE SOLO CN\ModularData\ai-agent\database.db
```

账号状态与日志路径见 `ACCOUNT_DETECTION_FEASIBILITY.md`。SQLCipher 参数、raw key 和已验证约束见 `TECHNICAL_BASELINE.md`。

启动时必须重新校验：

```text
产品版本
cipher_version
schema 指纹
Adapter mapping_version
关键表、列、索引和唯一约束
```

任何未知或不兼容项使该数据位置只读。

## 11. 数据位置模型

唯一键：

```text
(platform_id, data_location_id)
```

记录至少包含：

```text
data_location_id
platform_id
display_name
configured_path
normalized_path
volume_identity
directory_file_identity
product_identity
first_confirmed_at
last_confirmed_at
availability_state
```

自动发现仅检查 Adapter 标准目录和用户已添加目录。路径移动或文件身份变化时停止并要求重新确认；不得重定向到相似路径。

## 12. 当前账号证据

`AccountEvidence` 至少包含：

```text
data_location_id
user_id
source_events[]
auth_fingerprint
local_storage_user_id
product_version
observed_at
evidence_state
```

有效条件：

1. 严格白名单日志事件至少两类来源一致，或明文兼容认证对象直接给出 `userId`。
2. 认证字段 SHA-256 指纹与该账号证据完成绑定。
3. Local Storage 逻辑状态不与账号冲突；缺失可降级，冲突必须停止。
4. TRAE 关闭后认证指纹仍未变化。

未知密文、过期日志、来源冲突或指纹变化均为不可写状态。第三方账号管理器信息只允许显示为诊断提示。

## 13. 历史库存储布局

大型数据位于一个用户可迁移的存储根：

```text
<root>/
  storage-root.json
  catalog/
    current.json
    generations/<catalog_generation_id>/catalog.db
  snapshots/<snapshot_id>/
    snapshot.json
    database.db
    database.db-wal          按捕获状态可选
    database.db-shm          按捕获状态可选
  backups/<operation_id>/
    before/raw/
    before/logical/database.db
    failure/raw/
  operations/<operation_id>/
    completed-summary.json
    diagnostics/
  staging/
```

固定本地小型状态：

```text
%LOCALAPPDATA%\Trae Sync\
  config.json
  keys/catalog-key.dpapi
  recovery/<operation_id>.json
  locks/
```

固定恢复区保存完整进行中 manifest 和最小逻辑断言，不保存对话正文。存储根缺失或 `storage_root_id` 不匹配时，不创建空库、不扫描、不写入、不恢复。

## 14. 目录库最小模型

最终 SQL schema 在实现中版本化，但必须表达以下实体和约束：

```text
catalog_meta
data_location
seen_account
account_evidence
source_snapshot
snapshot_file
project_identity
project_observation
project_source_assignment
session_identity
session_version
session_version_source
session_projection
message_projection
sync_scope
sync_scope_item
operation_record
backup_set
storage_object_tombstone
```

关键语义：

- `session_identity` 唯一键是 `(product_history_namespace, original_session_id)`。
- `session_version` 保存 `mapping_version`、规范化内容图、内容图哈希和分类状态。
- `session_projection` 指向用户当前浏览版本，不删除其他版本。
- `project_observation` 记录 `first_observed_owner`、历次 owner、当前活动归属和来源快照。
- `project_source_assignment` 保存用户可选的展示与筛选归类；未设置时回退到 `first_observed_owner`，且不修改原始观察或活动库。
- `message_projection` 是可从 `session_version` 重建的浏览/搜索投影。
- FTS 索引位于同一 SQLCipher 目录库内，不生成磁盘明文旁路索引。
- 操作记录镜像 manifest 终态；进行中恢复仍以固定恢复区 manifest 为权威。

## 15. 来源快照

扫描前提：TRAE 已停止，文件集合在捕获前后保持稳定。

`snapshot.json` 记录：

```text
snapshot_id
platform_id
data_location_id
product_version
schema_fingerprint
mapping_version
account_evidence_ref
captured_at
每个 DB/WAL/SHM 的存在性、大小、SHA-256 和文件身份
```

规则：

- `database.db` 必须存在。
- WAL/SHM 按实际存在性捕获，不创建占位文件。
- 捕获前后存在性、大小或身份变化时废弃本次快照。
- 成功发布后快照不可修改。
- 数据指纹未变化时不创建新快照；已经创建的快照独立保存其捕获文件，V1 不做跨快照物理去重。

## 16. 版本分类

Adapter 生成确定性规范化内容图。比较结果：

```text
Identical       内容图完全相同
FastForward     旧节点和关系不变，新图只新增语义内容
Forked          既有语义内容修改、删除、重排或双方分叉
Unclassified    schema 或字段语义不足
```

只有 `Identical` 和 `FastForward` 自动更新当前投影。`Forked` 和 `Unclassified` 保留全部版本，等待历史库内选择；选择不把归档版本写回 TRAE。

## 17. 同步范围

```text
AllHistory
Custom { account_ids, project_ids, session_ids }
```

`AllHistory` 动态包含活动数据库内所有未删除、身份可靠、可同步历史。未知来源账号仍包含。`Custom` 只使用用户明确选择项，新发现内容不自动扩大范围。

一个账号选择展开为其项目和会话；一个项目选择展开为其会话。最终以稳定 ID 去重，每个 `session_id` 最多进入一次计划。

自定义范围只覆盖来源项目内部分活动会话时：目标账号已有可靠匹配的逻辑项目，生成 `AttachSessions`；目标账号没有同一项目，排除为 `PartialProjectRequiresTarget` 并提示选择整个项目。Planner 不得把部分会话选择静默扩大为 `FollowProject`，V1 也不自动创建目标项目行或复制 sandbox 配置。

## 18. 同步计划

`SyncPlan` 是不可变对象，至少包含：

```text
operation_id
created_at
platform_id
data_location_id
current_user_id
account_evidence_fingerprint
target_file_evidence
schema_fingerprint
mapping_version
scope_snapshot
actions[]
exclusions[]
expected_before
expected_after
```

动作：

```text
FollowProject {
  project_id,
  from_user_id,
  to_user_id
}

AttachSessions {
  source_project_id,
  target_project_id,
  session_ids[]
}
```

排除分类：

```text
AlreadyCurrent
ProjectIdentityConflict
ProjectIdentityUnknown
ArchivedOnly
DeletedProject
SchemaIncompatible
SessionVersionUnavailable
PartialProjectRequiresTarget
```

身份不明项默认排除，不提供强制执行入口。计划阶段允许排除；执行阶段对全部动作保持一个事务。

## 19. Work CN 写动作

### `FollowProject`

仅当同步范围覆盖该来源项目的全部活动可见会话，且目标账号没有同一逻辑项目时，事务内更新已验证项目：

```sql
UPDATE project
SET user_id = :target_user_id
WHERE project_id = :project_id
  AND user_id = :expected_source_user_id;
```

实际实现必须使用参数绑定，并验证变更行数和 `(biz_project_id, user_id)` 唯一约束。

### `AttachSessions`

对计划中的稳定 `session_id` 更新：

```text
chat_session.project_id
session_project.project_id
snapshot.project_id                    按 chat_session_id 限定
staging.project_id                     按 chat_session_id 限定
local_artifact.source_project_id/user_id
local_artifact_version.source_project_id
```

消息、turn、task、history、服务端缓存、FTS 和正文 JSON 不换 ID、不复制、不做字符串替换。

存在未知引用表、目标 sandbox 不可用或关系断言不满足时，该动作不能进入事务。

## 20. 执行协议

固定顺序：

1. 获取目录库单写锁。
2. 获取目标 `data_location_id` OS 级排他锁。
3. 协调该位置未完成 manifest。
4. 重新读取账号、文件、schema 和逻辑证据。
5. 证据漂移则废弃计划并返回预览。
6. 预检并实际预留峰值空间。
7. 创建原始证据快照和逻辑恢复副本。
8. 独立验证逻辑恢复副本。
9. 再次检查账号证据。
10. 开始一个 SQLCipher 事务，应用全部动作和事务内断言。
11. 提交前再次读取账号证据；漂移时回滚并结束为 `not_applied`。
12. 提交事务，并持久化 `target_committed_unverified`。
13. 提交后再次读取账号证据；漂移时仍继续验证和协调，但不启动 TRAE、不改变本次已确认目标。
14. 用新连接验证目标。
15. 执行 `cipher_integrity_check` 和 `integrity_check`。
16. 协调目录库观察记录、操作记录和 manifest。
17. 进入终态后释放锁和空间预留。
18. 无账号漂移且同步成功时，按设置启动 TRAE。

进入 `target_writing` 后不接受取消，必须继续到安全终态。

## 21. 操作状态机

唯一状态使用 D-039：

```text
planned
backing_up
backup_verified
target_writing
target_committed_unverified
target_verifying
catalog_reconciling
verification_inconclusive
failure_preserving
failure_snapshot_verified
restore_staging
restore_staged
restore_replacing
restored_verifying
completed
cancelled_before_write
failed_safe
not_applied
restored_verified
manual_recovery_required
```

意图状态先原子落盘，再执行对应副作用；结果状态在副作用返回后落盘。若进程在副作用与结果状态之间终止，重启后读取上一个意图状态和实际物理/逻辑证据协调，不盲目重放事务。

## 22. 写前双备份与恢复

每次写入必须同时创建：

```text
原始证据：捕获时实际存在的 DB/WAL/SHM 原字节和哈希
逻辑副本：SQLite Backup API 生成并由独立连接完整验证的单文件数据库
```

正常恢复优先使用逻辑副本。恢复前保存当前 DB/WAL/SHM 现场；旧 WAL/SHM 隔离保留，不发布回目标目录。恢复 staging 位于目标同卷，验证后原子发布单文件 DB，再让 SQLite 按需创建新 WAL/SHM。

任何目标漂移、备份损坏、现场保存失败或恢复验证失败进入 `manual_recovery_required`。

## 23. 完整性验证

事务内：

- 动作数量和期望变更行数。
- 项目唯一约束。
- 选中会话和项目关系。
- 计划未涉及记录保持不变的最小断言。

提交后新连接：

- key、cipher 版本、schema 和数据位置身份。
- 计划目标归属和提交后账号证据状态；提交后账号漂移单独进入协调，不误判为数据库损坏。
- 项目、会话、消息和软删除行数量。
- `session_project`、artifact、snapshot、staging 和 FTS 关系。
- `cipher_integrity_check` 无错误。
- `integrity_check = ok`。

完整检查结束前不报告成功、不启动 TRAE。

## 24. 并发与漂移

- 锁顺序固定为“目录库单写锁，再获取数据位置锁”。
- 同一位置的扫描、快照、同步、恢复、迁移和删除互斥。
- 第二实例可只读浏览已提交目录库，但不能执行副作用。
- 外部账号切换、路径移动、DB/WAL/SHM 变化或 schema 变化使计划失效。
- 进程异常退出释放 OS 锁，但新实例必须先协调 manifest。

## 25. 进度与取消

用户阶段：

```text
正在准备
正在备份
正在写入
正在验证
已完成 / 正在恢复
```

文件复制和哈希显示真实字节进度。无法得到总量的完整性检查只显示阶段和耗时。后台事件节流，前端保持响应。

读取、规划和目标写入前可取消；已完成备份仍保留。进入写入后取消按钮禁用，窗口关闭请求延迟到当前数据保护步骤完成。

## 26. 空间与存储迁移

每次副作用前分别预算历史库卷和目标卷；同卷时合并计算。安全余量初始值为预算的 20%，且不低于 512 MiB，最终由 Gate L 校准。

默认历史存储警戒线为 5 GB，用户可在存储管理详情中调整。达到警戒线后暂停非必要自动扫描并提示迁移或手工管理；它不是配额，不触发删除。空间不足时停止，不提供忽略按钮，也不自动删除数据。

历史库存储迁移先复制全部内容、逐文件校验、只读打开目录库并完整验证，再原子切换当前指针。旧根保持只读，只有用户明确删除。

### 26.1 手工删除

- 删除是独立操作，不能由扫描、同步、恢复、升级或存储迁移顺带触发。
- 先计算影响清单，明确将失去的原始版本、重新解析能力、恢复能力和占用空间。
- 删除计划绑定 `storage_root_id`、对象稳定 ID、文件哈希和引用图；确认前发生漂移时计划失效并重新预览。
- 首次基线、当前快照、最近成功写前备份、冲突来源和用户固定项默认受保护；用户必须先显式解除保护。
- 被非终态 manifest 引用的对象和当前可写目录库代次不得删除；删除与同一数据位置的其他副作用互斥。
- 已收录快照、备份或旧目录库代次删除后，目录库保留审计墓碑、原哈希和删除操作 ID，并明确标记对应原始重解析或恢复能力已不可用。
- 用户选定精确对象后进行二次确认；执行范围只能包含影响清单中的稳定 ID。
- 删除首个对象前持久化删除 manifest；中断后只显示已删除和剩余对象，由用户明确继续，不把范围扩大到新对象。
- 删除结果写入操作记录，并验证未选对象保持完整、选中对象引用已转为一致的审计墓碑。

## 27. 目录库密钥与恢复包

- 目录库使用独立随机 SQLCipher 密钥。
- Windows DPAPI 保护日常密钥包装。
- 配置文件不保存明文密钥。
- `.traesync-recovery` 只保存密码加密的目录库密钥和版本化元数据，不保存历史正文。
- 恢复时先验证包、目录库 ID、密钥代次和目录库完整性，再重新生成当前用户 DPAPI 包装。
- 具体 KDF 与认证加密参数由 Gate H 基准确定，不暴露为日常设置。

## 28. 目录库升级

目录库不原地升级：复制到同级 staging，逐级迁移，完整验证后原子切换 `current.json`。旧代目录库保持受保护只读。迁移失败继续使用旧库兼容只读模式，并禁止所有写操作。

## 29. 设置

主设置仅保留：

```text
自动查找新历史             默认关闭；仅在已授权且 TRAE 已停止时读取
同步成功后重新打开 TRAE     默认开启
```

存储位置、5 GB 警戒线、恢复包、备份管理和诊断是按需入口，不构成首次配置压力。数据库、加密、备份、恢复、冲突和验证策略没有关闭开关。

## 30. 前后端契约

Tauri command 只返回结构化 DTO，不传递数据库连接、原始认证内容或任意 SQL。建议最小命令集：

```text
get_workspace_state
discover_data_locations
add_data_location
request_scan
query_history
assign_project_source
get_sync_scope
save_sync_scope
build_sync_plan
apply_sync_plan
request_cancel
list_operations
restore_operation
get_storage_state
plan_storage_deletion
apply_storage_deletion
move_storage_root
get_settings
update_settings
export_recovery_package
import_recovery_package
export_diagnostics
```

长任务使用带 `operation_id` 的事件：

```text
operation-stage
operation-progress
operation-needs-attention
operation-finished
```

前端丢失事件后必须能用 `operation_id` 重新查询状态，不能只依赖内存进度。

## 31. 错误分类

用户可见错误至少分为：

```text
NeedsProductClosed
AccountEvidenceUnavailable
DataLocationUnavailable
DataLocationChanged
StorageRootUnavailable
InsufficientSpace
PlanExpired
SchemaUnsupported
BackupFailedSafe
VerificationInconclusive
RestoredAfterFailure
ManualRecoveryRequired
AnotherOperationRunning
ProtectedStorageObject
DeletionPlanExpired
```

普通文案给出一个推荐操作；技术路径、哈希和底层错误放在详情。

## 32. 发布边界

V1 只有对应 Gate 通过后才开放能力：

- Gate A/B 通过前：不能读取真实活动库生成可写计划。
- Gate C/G 通过前：不能对真实目标执行写入。
- Gate F/M 通过前：同项目会话重挂保持关闭。
- Gate H 通过前：不发布恢复包功能。
- Gate I/J 通过前：不宣称版本与软删除统计正确。
- Gate K/L 通过前：不发布大库长任务和存储迁移。
- 目录库升级 Gate 通过前：首版不执行自动 schema 升级。

完整顺序、fixture 和证据要求见 `GATE_PLAN.md`。

## 33. 实现完成定义

一次 V1 候选版本只有同时满足以下条件才可验收：

1. 所有必需 Gate 有可复现证据，不以 mock 替代真实 SQLCipher、文件和 UI 行为。
2. 默认路径完成“扫描、浏览、预览、备份、应用、验证、重开”端到端流程。
3. 两账号往返后来源归类稳定，目标历史正确，无重复计划。
4. 同项目会话合并通过 TRAE 列表、搜索、正文和继续发送消息验证。
5. 每个非终态完成崩溃注入和重启协调，不重复写、不误报成功。
6. 任何验证失败均保存写前和失败后证据；自动恢复或冻结结果可证明。
7. 工具未自动删除任何用户历史或备份。
8. 安装包在干净 Windows x64 环境检测 WebView2、打开目录库并完成只读扫描。
